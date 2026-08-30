//! Tell a human when the loop needs one: waiting on a decision, stopped, rate-limited.
//!
//! Two channels, both optional, configured in `policy`:
//!   * `notify_url` — POST JSON via `curl`. Feishu / Lark custom-bot URLs get the
//!     `{"msg_type":"text","content":{"text":…}}` shape, everything else `{"text":…,"event":…}`.
//!   * `notify_cmd` — run through `sh -c`; the event JSON arrives on stdin and as
//!     `ZLOOP_EVENT` / `ZLOOP_TEXT` environment variables.
//!
//! Zero dependencies: the surveyed harnesses (Ralph, Anthropic's, Codex /goal) all assume
//! the human comes back on their own; a loop that runs overnight needs to call them.

use crate::state::{self, State};
use anyhow::Result;
use serde_json::json;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

pub fn configured(state: &State) -> bool {
    state.policy.notify_url.is_some() || state.policy.notify_cmd.is_some()
}

/// 通知那一下的闸。发通知是**收尾**动作：`stop()` 里那一下卡住，runner 就不记 `stop`、
/// 不清 pid 文件、退不出去——「干完就停」的承诺卡在最后一米（A-14）。所以这里也必须有闸。
/// 30 秒对一条 webhook / 一句 `osascript` 都足够宽松了。
fn timeout() -> Duration {
    crate::runner::env_secs("ZLOOP_NOTIFY_TIMEOUT_SECS", 30)
}

/// 把一条带闸的子进程结果翻成「发出去了没有」，顺带把失败原因打到 stderr。
/// 通道失败从来不是致命的：通知发不出去不该把 runner 拖下水。
fn accepted(what: &str, cap: anyhow::Result<crate::runner::CapturedBytes>) -> bool {
    match cap {
        Ok(c) if c.timed_out || c.interrupted => {
            let how = if c.timed_out { format!("超过 {:?} 没回来", timeout()) } else { "被叫停".to_string() };
            eprintln!("notify: {what} {how}，已经整组收掉（通知发没发出去不知道）");
            false
        }
        Ok(c) if c.status.map(|s| s.success()).unwrap_or(false) => true,
        Ok(c) => {
            eprintln!("notify: {what} failed: {}", String::from_utf8_lossy(&c.stderr).trim());
            false
        }
        Err(e) => {
            eprintln!("notify: cannot run {what}: {e}");
            false
        }
    }
}

fn payload_for(url: &str, kind: &str, text: &str, state: &State, root: &Path) -> String {
    let lower = url.to_lowercase();
    if lower.contains("feishu") || lower.contains("larksuite") || lower.contains("lark") {
        json!({"msg_type": "text", "content": {"text": text}}).to_string()
    } else {
        json!({"text": text, "event": kind, "goal": state.goal.id, "root": root.display().to_string(), "at": state::now_iso()}).to_string()
    }
}

/// Send one notification. Returns Ok(true) if at least one channel accepted it,
/// Ok(false) if nothing is configured. Channel failures are reported on stderr, not fatal.
pub fn send(state: &State, root: &Path, kind: &str, text: &str) -> Result<bool> {
    let mut sent = false;
    if let Some(url) = &state.policy.notify_url {
        let body = payload_for(url, kind, text, state, root);
        let mut c = Command::new("curl");
        c.args(["-sS", "-m", "10", "-X", "POST", "-H", "Content-Type: application/json", "--data-binary", "@-", url]);
        // curl 自己的 `-m 10` 只管它自己：DNS 卡住、代理不回、curl 被 stop 掉的孙进程留着管道，
        // 都还得靠外面这个闸。带闸的那份实现只有一处，就是 `runner::run_capture`。
        sent |= accepted(
            "webhook",
            crate::runner::run_capture(
                c,
                timeout(),
                crate::runner::Group::Own,
                crate::runner::Stop::Honor,
                Some(body.into_bytes()),
            ),
        );
    }
    if let Some(cmd) = &state.policy.notify_cmd {
        let event = json!({"event": kind, "text": text, "goal": state.goal.id, "root": root.display().to_string(), "at": state::now_iso()}).to_string();
        let mut c = Command::new("sh");
        c.arg("-c").arg(cmd).env("ZLOOP_EVENT", kind).env("ZLOOP_TEXT", text).env("ZLOOP_ROOT", root).current_dir(root);
        sent |= accepted(
            "command",
            crate::runner::run_capture(
                c,
                timeout(),
                crate::runner::Group::Own,
                crate::runner::Stop::Honor,
                Some(event.into_bytes()),
            ),
        );
    }
    Ok(sent)
}

/// Human-readable message for a runner event.
pub fn text_for(kind: &str, state: &State, root: &Path, detail: &str) -> String {
    let head = format!("[zloop · {}]", state.goal.id);
    let goal = &state.goal.text;
    let dir = root.display();
    match kind {
        "wait" => format!("{head} 等你决定\n目标：{goal}\n{detail}\n回答后：zloop edit <id> --status open（runner 会自动续跑）\n目录：{dir}"),
        "rate_limited" => format!("{head} 宿主限流，稍后自动重试\n{detail}\n目录：{dir}"),
        "stop" => format!("{head} runner 已停：{detail}\n目标：{goal}\n看一眼：zloop status · zloop log\n目录：{dir}"),
        _ => format!("{head} {kind}\n{detail}\n目录：{dir}"),
    }
}
