//! Tell a human when the loop needs one: waiting on a decision, stopped, rate-limited.
//!
//! Two channels, both optional, configured in `policy`:
//!   * `notify_url` — POST JSON via `curl`. Feishu / Lark custom-bot URLs get the
//!     `{"msg_type":"text","content":{"text":…}}` shape, everything else `{"text":…,"event":…}`.
//!   * `notify_cmd` — run through `sh -c`; the event JSON arrives on stdin and as
//!     `ZLOOP_EVENT` / `ZLOOP_TEXT` environment variables.
//! Zero dependencies: the surveyed harnesses (Ralph, Anthropic's, Codex /goal) all assume
//! the human comes back on their own; a loop that runs overnight needs to call them.

use crate::state::{self, State};
use anyhow::Result;
use serde_json::json;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

pub fn configured(state: &State) -> bool {
    state.policy.notify_url.is_some() || state.policy.notify_cmd.is_some()
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
        match Command::new("curl")
            .args(["-sS", "-m", "10", "-X", "POST", "-H", "Content-Type: application/json", "--data-binary", "@-", url])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(mut child) => {
                if let Some(mut stdin) = child.stdin.take() {
                    let _ = stdin.write_all(body.as_bytes());
                }
                match child.wait_with_output() {
                    Ok(o) if o.status.success() => sent = true,
                    Ok(o) => eprintln!("notify: webhook failed: {}", String::from_utf8_lossy(&o.stderr).trim()),
                    Err(e) => eprintln!("notify: webhook error: {e}"),
                }
            }
            Err(e) => eprintln!("notify: cannot run curl: {e}"),
        }
    }
    if let Some(cmd) = &state.policy.notify_cmd {
        let event = json!({"event": kind, "text": text, "goal": state.goal.id, "root": root.display().to_string(), "at": state::now_iso()}).to_string();
        match Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .env("ZLOOP_EVENT", kind)
            .env("ZLOOP_TEXT", text)
            .env("ZLOOP_ROOT", root)
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(mut child) => {
                if let Some(mut stdin) = child.stdin.take() {
                    let _ = stdin.write_all(event.as_bytes());
                }
                match child.wait_with_output() {
                    Ok(o) if o.status.success() => sent = true,
                    Ok(o) => eprintln!("notify: command failed: {}", String::from_utf8_lossy(&o.stderr).trim()),
                    Err(e) => eprintln!("notify: command error: {e}"),
                }
            }
            Err(e) => eprintln!("notify: cannot run sh: {e}"),
        }
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
