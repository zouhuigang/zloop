//! One line that says where the loop is right now.
//!
//! Sources, in priority order:
//! 1. `state.in_progress` — a todo was handed out by `next` / the runner and not yet written back;
//! 2. the runner journal — sleeping until a known time, or a round that began and never ended;
//! 3. `tick::decide()` — idle (would run X), waiting (retry in N min), or stopped (reason).
//!
//! loopx spreads the same information over `lifecycle_phase`, `waiting_on`, `quota.state`,
//! `scheduler_hint.execution_phase` and the turn journal; here it is a single string.

use crate::state::{self, parse_iso, State};
use crate::tick;
use chrono::{DateTime, FixedOffset};
use serde_json::Value;
use std::path::Path;

pub const JOURNAL_REL: &str = "runner/journal.jsonl";

#[derive(Debug, Clone, PartialEq)]
pub struct Phase {
    /// executing | sleeping | idle | waiting | stopped
    pub kind: &'static str,
    /// The full sentence, for `zloop context` / `zloop next` — the machine-facing contract.
    pub summary: String,
    /// The same thing minus the state word, for `zloop status`, whose headline already says
    /// *what* state this is; empty when the headline says everything there is to say.
    pub detail: String,
}

fn hhmm(ts: &str) -> String {
    parse_iso(ts).map(|dt| dt.format("%H:%M").to_string()).unwrap_or_else(|_| ts.to_string())
}

fn elapsed(from: &str, now: DateTime<FixedOffset>) -> String {
    let Ok(start) = parse_iso(from) else { return String::new() };
    let secs = (now - start).num_seconds().max(0);
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else {
        format!("{}h{:02}m", secs / 3600, (secs % 3600) / 60)
    }
}

fn last_journal_event(root: &Path) -> Option<Value> {
    let raw = std::fs::read_to_string(root.join(state::STATE_DIR).join(JOURNAL_REL)).ok()?;
    let line = raw.lines().rev().find(|l| !l.trim().is_empty())?;
    serde_json::from_str(line).ok()
}

/// Decision reasons read as jargon in a Chinese dashboard line; `zloop context` keeps the raw word.
/// 90 分钟以上就别让人自己换算了。
pub fn human_minutes(m: u32) -> String {
    match m {
        0..=89 => format!("{m} 分钟"),
        90..=1439 => format!("约 {} 小时", (m + 30) / 60),
        _ => format!("约 {} 天", (m + 720) / 1440),
    }
}

pub fn reason_zh(r: &str) -> String {
    match r {
        "user_gate" => "等你回答".into(),
        "blocked" => "等依赖".into(),
        "fail_streak" => "连续失败".into(),
        "progress_streak" => "同一条 todo 只有进展没有完成".into(),
        "budget" => "到花费上限".into(),
        "throttled" => "本窗口次数用完".into(),
        "all_done" | "done" => "全部完成".into(),
        "all_deferred" => "待办全被延后了".into(),
        "unplanned" => "还没有待办".into(),
        "paused" => "已暂停".into(),
        other => other.into(),
    }
}

pub fn compute(state: &State, root: &Path, now: DateTime<FixedOffset>) -> Phase {
    if let Some(ip) = &state.in_progress {
        let host = ip.host.as_deref().unwrap_or("cli");
        let age_min = parse_iso(&ip.started_at).map(|s| (now - s).num_minutes()).unwrap_or(0);
        let stale = if state.policy.stale_after_min > 0 && age_min >= state.policy.stale_after_min {
            format!(" ⚠ stale (>{}m, the session that took it may be gone; next `zloop next` re-hands it out)", state.policy.stale_after_min)
        } else {
            String::new()
        };
        let stale_short = if stale.is_empty() {
            String::new()
        } else {
            format!(" ⚠ 超过 {}m 没动静", state.policy.stale_after_min)
        };
        return Phase {
            kind: "executing",
            summary: format!(
                "executing {} · round {} · since {} ({} ago) · host {} · via {}{}",
                ip.todo,
                ip.round,
                hhmm(&ip.started_at),
                elapsed(&ip.started_at, now),
                host,
                ip.via,
                stale
            ),
            detail: format!(
                "{} 正在做 {} · 第 {} 轮 · 已跑 {}{}",
                host,
                ip.todo,
                ip.round,
                elapsed(&ip.started_at, now),
                stale_short
            ),
        };
    }
    if let Some(ev) = last_journal_event(root) {
        let kind = ev.get("event").and_then(Value::as_str).unwrap_or("");
        if kind == "sleep" {
            if let Some(until) = ev.get("until").and_then(Value::as_str) {
                if let Ok(until_dt) = parse_iso(until) {
                    if until_dt > now {
                        let left = (until_dt - now).num_seconds();
                        return Phase {
                            kind: "sleeping",
                            summary: format!(
                                "runner sleeping until {} ({}m{:02}s left) · reason {}",
                                hhmm(until),
                                left / 60,
                                left % 60,
                                ev.get("reason").and_then(Value::as_str).unwrap_or("ready")
                            ),
                            detail: format!("两轮之间的休息 · 睡到 {} 醒，还有 {}m{:02}s", hhmm(until), left / 60, left % 60),
                        };
                    }
                }
            }
        }
        if kind == "begin" {
            let todo = ev.get("todo").and_then(Value::as_str).unwrap_or("?");
            let at = ev.get("at").and_then(Value::as_str).unwrap_or("");
            let round = ev.get("round").and_then(Value::as_u64).unwrap_or(0);
            return Phase {
                kind: "executing",
                summary: format!(
                    "runner round {} on {} since {} ({} ago) — no end recorded (process may have died)",
                    round,
                    todo,
                    hhmm(at),
                    elapsed(at, now)
                ),
                detail: format!("第 {round} 轮做 {todo} · 已跑 {} · ⚠ 没有结束记录，进程可能死了", elapsed(at, now)),
            };
        }
    }
    let d = tick::decide(state, now);
    if d.should_run {
        let t = d.todo.as_ref().unwrap();
        // The status headline says "就绪" and marks the todo with ▶; nothing left to add.
        return Phase {
            kind: "idle",
            summary: format!("idle · next would run {} [P{}] {}", t.id, t.priority, t.text),
            detail: String::new(),
        };
    }
    match d.interval_min {
        None => Phase {
            kind: "stopped",
            summary: format!("stopped ({})", d.reason),
            // "已完成 / 已暂停 / 待规划 / 全部延后" is already the headline word; any other reason is news.
            detail: match d.reason.as_str() {
                "all_done" | "done" | "paused" | "unplanned" | "all_deferred" => String::new(),
                other => format!("{}，已停下等你处理", reason_zh(other)),
            },
        },
        Some(m) => Phase {
            kind: "waiting",
            summary: format!("waiting ({}) · retry in {} min", d.reason, m),
            detail: format!("{} · {}后重试", reason_zh(&d.reason), human_minutes(m)),
        },
    }
}
