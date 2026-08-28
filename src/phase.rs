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
    pub summary: String,
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

pub fn compute(state: &State, root: &Path, now: DateTime<FixedOffset>) -> Phase {
    if let Some(ip) = &state.in_progress {
        let host = ip.host.as_deref().unwrap_or("cli");
        let age_min = parse_iso(&ip.started_at).map(|s| (now - s).num_minutes()).unwrap_or(0);
        let stale = if state.policy.stale_after_min > 0 && age_min >= state.policy.stale_after_min {
            format!(" ⚠ stale (>{}m, the session that took it may be gone; next `zloop next` re-hands it out)", state.policy.stale_after_min)
        } else {
            String::new()
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
                        };
                    }
                }
            }
        }
        if kind == "begin" {
            let todo = ev.get("todo").and_then(Value::as_str).unwrap_or("?");
            let at = ev.get("at").and_then(Value::as_str).unwrap_or("");
            return Phase {
                kind: "executing",
                summary: format!(
                    "runner round {} on {} since {} ({} ago) — no end recorded (process may have died)",
                    ev.get("round").and_then(Value::as_u64).unwrap_or(0),
                    todo,
                    hhmm(at),
                    elapsed(at, now)
                ),
            };
        }
    }
    let d = tick::decide(state, now);
    if d.should_run {
        let t = d.todo.as_ref().unwrap();
        return Phase { kind: "idle", summary: format!("idle · next would run {} [P{}] {}", t.id, t.priority, t.text) };
    }
    match d.interval_min {
        None => Phase { kind: "stopped", summary: format!("stopped ({})", d.reason) },
        Some(m) => Phase { kind: "waiting", summary: format!("waiting ({}) · retry in {} min", d.reason, m) },
    }
}
