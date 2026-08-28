//! Bounded handoff packet: what another host or a fresh session needs to continue.
//!
//! Sections are filled in priority order and trimmed from the tail when the
//! character budget is exceeded (the loopx handoff-packet rule).

use crate::session::{self, Host};
use crate::state::State;
use crate::tick;
use crate::todo;
use chrono::{DateTime, FixedOffset};
use std::path::Path;

pub const DEFAULT_BUDGET: usize = 4000;

fn resume_hint(host: Option<Host>) -> &'static str {
    match host {
        Some(Host::Codex) => "在 Codex 里：先 `zloop context`，再 `zloop next --json`，做完 `zloop done …`。无头续跑：`zloop run --host codex`。",
        Some(Host::Claude) | None => "在 Claude Code 里：`/zloop` 跑一轮，`/loop /zloop` 自动续跑。无头续跑：`zloop run --host claude`。",
        Some(Host::Cli) => "在终端里：`zloop next --json` → 做 → `zloop done …`。",
    }
}

pub fn build(state: &State, root: &Path, budget: usize, for_host: Option<Host>, at: DateTime<FixedOffset>) -> String {
    let mut sections: Vec<String> = Vec::new();

    let spent = tick::spent_usd(&state.ticks);
    let spent_line = if spent > 0.0 || state.policy.max_total_usd > 0.0 {
        format!(
            "\n已花费：${spent:.2}{}",
            if state.policy.max_total_usd > 0.0 { format!(" / 上限 ${:.2}", state.policy.max_total_usd) } else { String::new() }
        )
    } else {
        String::new()
    };
    sections.push(format!(
        "## 目标\n{}\n项目目录：{}\n阶段：{}{}",
        state.goal.text,
        root.display(),
        crate::phase::compute(state, root, at).summary,
        spent_line
    ));

    let recent: Vec<String> = state
        .ticks
        .iter()
        .rev()
        .filter(|t| t.outcome != "noop")
        .take(3)
        .map(|t| {
            let who = t.todo.as_deref().unwrap_or("-");
            let note = if t.note.is_empty() { String::new() } else { format!("：{}", t.note) };
            format!("- {} {who} {}{}", t.at, t.outcome, note)
        })
        .collect();
    sections.push(format!(
        "## 当前判断（最近 3 次执行）\n{}",
        if recent.is_empty() { "- 尚未开始".to_string() } else { recent.join("\n") }
    ));

    let decision = tick::decide(state, at);
    let next = match &decision.todo {
        Some(t) => format!("{} [P{}] {}", t.id, t.priority, t.text),
        None => format!("（{}）", decision.reason),
    };
    sections.push(format!("## 下一条\n{next}"));

    let open: Vec<String> = todo::open_ordered(state)
        .into_iter()
        .take(5)
        .map(|i| {
            let t = &state.todos[i];
            let deps = if t.blocked_by.is_empty() { String::new() } else { format!(" ⏳{}", t.blocked_by.join(",")) };
            let note = if t.status == "blocked" && !t.note.is_empty() { format!(" — {}", t.note) } else { String::new() };
            let acc = t.acceptance.as_deref().map(|a| format!(" ｜验收：{}", a.chars().take(80).collect::<String>())).unwrap_or_default();
            format!("- {} {} [P{}] {}{}{}{}", crate::prompt::checkbox(&t.status), t.id, t.priority, t.text, deps, note, acc)
        })
        .collect();
    sections.push(format!(
        "## 待办（前 5 条，共 {} 条未完成）\n{}",
        todo::remaining(state),
        if open.is_empty() { "- 全部完成".to_string() } else { open.join("\n") }
    ));

    let sessions = session::summarize(state, root);
    if !sessions.is_empty() {
        let mut lines = Vec::new();
        for host in ["claude", "codex", "cli"] {
            if let Some(s) = sessions.iter().rev().find(|s| s.host == host) {
                if let Some(cmd) = &s.resume {
                    lines.push(format!("- {}（最近 {}，{} ticks）：`{}`", host, s.last, s.ticks, cmd));
                }
            }
        }
        if !lines.is_empty() {
            sections.push(format!("## 会话（可回看细节）\n{}", lines.join("\n")));
        }
    }

    let lessons = crate::notes::recent(root, 5);
    if !lessons.is_empty() {
        sections.push(format!(
            "## 经验（zloop remember，最近 {} 条）\n{}",
            lessons.len(),
            lessons.iter().map(|l| format!("- {l}")).collect::<Vec<_>>().join("\n")
        ));
    }

    sections.push(format!("## 怎么继续\n{}", resume_hint(for_host)));

    // Trim from the tail (keep 目标 / 当前判断 / 下一条 as long as possible).
    let render = |secs: &[String]| secs.join("\n\n");
    let mut kept = sections.clone();
    while kept.len() > 3 && render(&kept).chars().count() > budget {
        let drop_idx = kept.len() - 2; // keep the last "怎么继续" line, drop the one before it
        kept.remove(drop_idx);
    }
    let mut out = render(&kept);
    if out.chars().count() > budget {
        out = out.chars().take(budget.saturating_sub(1)).collect::<String>() + "…";
    }
    out
}
