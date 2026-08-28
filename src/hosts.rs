//! Host installers: the /zloop skill for Claude Code and Codex, plus an optional Stop hook.
//!
//! Files we write carry MANAGED_MARK; we never overwrite a file that lacks it.

use anyhow::{bail, Result};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

pub const MANAGED_MARK: &str = "<!-- zloop-managed:v1 -->";
pub const HOOK_COMMAND: &str = "zloop hook-stop";

const FRONTMATTER: &str = r#"---
name: "zloop"
description: "围着一个目标持续干活：有参数时初始化目标并规划 todo，无参数时执行一轮（context → next → 做 → done）。"
argument-hint: "[goal text]"
---
"#;

const BODY: &str = r#"
# zloop /zloop

先在项目根目录运行 `zloop context`。报错 "no zloop state" 说明尚未初始化。

## 参数是子命令名（`status` / `context` / `sessions` / `log` / `next` / `goal` / `goals`）

直接运行 `zloop <参数>` 并把输出讲给用户，不要 init、不要规划。

## 参数是目标文本（其余非空参数）

1. 定目标：
   - 未初始化 → `zloop init "$ARGUMENTS"`
   - 当前目标已完成，或新输入明显是**另一件事** → `zloop goal new "$ARGUMENTS"`：当前目标原地停放，`zloop goal switch <id>` 随时切回，什么都不丢。**别把新活挂到旧目标名下**，那会让目标文字和实际做的事对不上。
   - 当前目标还有没做完的 todo，而新输入像是它的延伸 → 先告知当前目标 + 剩余步骤，问用户"接着做"还是"开新目标"；说开新的就 `zloop goal new`。
   - 不要用 `zloop init --force`：它归档旧目标且切不回来。
2. 把目标拆成 2–5 条**可验证**的 todo，每行 `[P0]`/`[P1]`/`[P2]` + 文本，按执行顺序通过 stdin 交给 `zloop plan`。
3. 立刻执行一轮（见下）。
4. 最后告诉用户：{resume_hint}

## 无参数 = 执行一轮

运行 `zloop heartbeat --host {host}`，严格按它打印的 5 条协议做：`zloop context` → `zloop next --json` → 只做那一条 todo → `zloop done <id> …`（把关键证据放进 `--evidence`）→ 两三句话汇报。
`should_run=false` 时按 reason 简短说明后停止，不要找别的事做。
"#;

const OPENAI_YAML: &str = r#"interface:
  display_name: "zloop"
  short_description: "围着一个目标持续干活的最小调度器"
  default_prompt: "执行一轮 zloop：context → next → 做 → done"
policy:
  allow_implicit_invocation: false
"#;

fn resume_hint(host: &str) -> &'static str {
    match host {
        "claude" => "输入 `/loop /zloop` 可让它按 interval_min 自动续跑；或在终端运行 `zloop run --host claude` 无头续跑。",
        _ => "可用 automation_update 建 automation（body = `zloop heartbeat --host codex-app` 的输出，初始 3 分钟）自动续跑；或在终端运行 `zloop run --host codex`。",
    }
}

pub fn skill_markdown(host: &str) -> String {
    let body = BODY.replace("{host}", host).replace("{resume_hint}", resume_hint(host));
    format!("{FRONTMATTER}\n{MANAGED_MARK}{body}")
}

/// Write `content` unless an unmanaged file is already there. Returns true when changed.
fn write_managed(path: &Path, content: &str) -> Result<bool> {
    if path.exists() {
        let current = fs::read_to_string(path)?;
        if !current.contains(MANAGED_MARK) && content.trim_start().starts_with("---") {
            bail!("{} exists and is not managed by zloop; remove it first", path.display());
        }
        if current == content {
            return Ok(false);
        }
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, content)?;
    Ok(true)
}

pub fn install_claude(home: &Path) -> Result<Vec<(PathBuf, bool)>> {
    let path = home.join(".claude").join("skills").join("zloop").join("SKILL.md");
    let changed = write_managed(&path, &skill_markdown("claude"))?;
    Ok(vec![(path, changed)])
}

pub fn install_codex(home: &Path) -> Result<Vec<(PathBuf, bool)>> {
    let base = home.join(".codex").join("skills").join("zloop");
    let skill = base.join("SKILL.md");
    let yaml = base.join("agents").join("openai.yaml");
    let mut out = vec![(skill.clone(), write_managed(&skill, &skill_markdown("codex-app"))?)];
    let same = yaml.exists() && fs::read_to_string(&yaml)? == OPENAI_YAML;
    if !same {
        if let Some(p) = yaml.parent() {
            fs::create_dir_all(p)?;
        }
        fs::write(&yaml, OPENAI_YAML)?;
    }
    out.push((yaml, !same));
    Ok(out)
}

/// Append a Stop hook running `zloop hook-stop` to ~/.claude/settings.json (idempotent).
pub fn install_claude_stop_hook(home: &Path) -> Result<Vec<(PathBuf, bool)>> {
    let settings_path = home.join(".claude").join("settings.json");
    let mut settings: Value = if settings_path.exists() {
        let raw = fs::read_to_string(&settings_path)?;
        if raw.trim().is_empty() { json!({}) } else { serde_json::from_str(&raw)? }
    } else {
        json!({})
    };
    let hooks = settings
        .as_object_mut()
        .expect("settings object")
        .entry("hooks")
        .or_insert_with(|| json!({}));
    let stop = hooks
        .as_object_mut()
        .expect("hooks object")
        .entry("Stop")
        .or_insert_with(|| json!([]));
    let arr = stop.as_array_mut().expect("Stop array");
    let present = arr.iter().any(|entry| {
        entry
            .get("hooks")
            .and_then(Value::as_array)
            .map(|hs| hs.iter().any(|h| h.get("command").and_then(Value::as_str) == Some(HOOK_COMMAND)))
            .unwrap_or(false)
    });
    if present {
        return Ok(vec![(settings_path, false)]);
    }
    arr.push(json!({"hooks": [{"type": "command", "command": HOOK_COMMAND}]}));
    if let Some(p) = settings_path.parent() {
        fs::create_dir_all(p)?;
    }
    fs::write(&settings_path, serde_json::to_string_pretty(&settings)? + "\n")?;
    Ok(vec![(settings_path, true)])
}

pub fn install(claude: bool, codex: bool, stop_hook: bool, home: &Path) -> Result<Vec<(PathBuf, bool)>> {
    let mut out = Vec::new();
    if claude {
        out.extend(install_claude(home)?);
    }
    if codex {
        out.extend(install_codex(home)?);
    }
    if stop_hook {
        out.extend(install_claude_stop_hook(home)?);
    }
    Ok(out)
}
