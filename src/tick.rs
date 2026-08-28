//! The scheduler: `decide()` answers "should we run now, and on which todo?".
//!
//! State ladder (highest wins):
//!     paused/done > all_done > user_gate/blocked > fail_streak > throttled > ready

use crate::session::HostSession;
use crate::state::{format_iso, now, parse_iso, State, Tick, Todo};
use crate::todo;
use anyhow::{bail, Result};
use chrono::{DateTime, Duration, FixedOffset};
use serde_json::{json, Map, Value};

pub const COUNTED: [&str; 3] = ["done", "progress", "fail"];
pub const OUTCOMES: [&str; 6] = ["done", "progress", "fail", "block", "noop", "edit"];

#[derive(Debug, Clone, PartialEq)]
pub struct Decision {
    pub should_run: bool,
    pub reason: String,
    pub todo: Option<Todo>,
    /// `None` means "stop, wait for a human".
    pub interval_min: Option<u32>,
}

impl Decision {
    fn stop(reason: &str) -> Self {
        Decision { should_run: false, reason: reason.into(), todo: None, interval_min: None }
    }
}

// --- streaks & accounting -------------------------------------------------

/// Trailing consecutive `fail` ticks; `noop` ticks are transparent, anything else breaks it.
pub fn fail_streak(ticks: &[Tick]) -> usize {
    let mut n = 0;
    for t in ticks.iter().rev() {
        match t.outcome.as_str() {
            "fail" => n += 1,
            "noop" => continue,
            _ => break,
        }
    }
    n
}

pub fn noop_streak(ticks: &[Tick]) -> usize {
    ticks.iter().rev().take_while(|t| t.outcome == "noop").count()
}

/// Trailing consecutive `progress` ticks on `todo_id`; `noop` is transparent, anything else breaks it.
pub fn progress_streak(ticks: &[Tick], todo_id: &str) -> usize {
    let mut n = 0;
    for t in ticks.iter().rev() {
        match t.outcome.as_str() {
            "noop" => continue,
            "progress" if t.todo.as_deref() == Some(todo_id) => n += 1,
            _ => break,
        }
    }
    n
}

pub fn current_round(ticks: &[Tick]) -> u64 {
    ticks.iter().filter(|t| t.outcome == "done" || t.outcome == "progress").count() as u64
}

/// Total host-reported spend recorded on ticks (USD).
pub fn spent_usd(ticks: &[Tick]) -> f64 {
    ticks.iter().filter_map(|t| t.cost_usd).sum()
}

pub fn window_ticks<'a>(state: &'a State, at: DateTime<FixedOffset>) -> Vec<&'a Tick> {
    let since = at - Duration::hours(state.policy.window_hours);
    state
        .ticks
        .iter()
        .filter(|t| COUNTED.contains(&t.outcome.as_str()))
        .filter(|t| parse_iso(&t.at).map(|ts| ts >= since).unwrap_or(false))
        .collect()
}

fn interval(state: &State, level: usize) -> u32 {
    let iv = &state.policy.intervals_min;
    if iv.is_empty() {
        return 3;
    }
    iv[level.min(iv.len() - 1)]
}

// --- the decision ---------------------------------------------------------

pub fn decide(state: &State, at: DateTime<FixedOffset>) -> Decision {
    let goal = &state.goal;
    let policy = &state.policy;
    let ticks = &state.ticks;

    if goal.status != "active" {
        return Decision::stop(&goal.status);
    }
    let open = todo::open_ordered(state);
    if open.is_empty() {
        return Decision::stop("all_done");
    }
    let noops = noop_streak(ticks);
    let exhausted = noops >= policy.max_noop_streak;

    let runnable = todo::executable(state);
    if runnable.is_empty() {
        let waiting_on_user = open
            .iter()
            .any(|&i| state.todos[i].blocked_by.iter().any(|d| d == todo::USER));
        let reason = if waiting_on_user { "user_gate" } else { "blocked" };
        return Decision {
            should_run: false,
            reason: reason.into(),
            todo: None,
            interval_min: if exhausted { None } else { Some(interval(state, 1 + noops)) },
        };
    }
    if fail_streak(ticks) >= policy.max_fail_streak {
        return Decision::stop("fail_streak");
    }
    if policy.max_total_usd > 0.0 && spent_usd(ticks) >= policy.max_total_usd {
        return Decision::stop("budget");
    }
    let candidate = &state.todos[runnable[0]];
    if policy.max_progress_streak > 0 && progress_streak(ticks, &candidate.id) >= policy.max_progress_streak {
        return Decision::stop("progress_streak");
    }
    let counted = window_ticks(state, at);
    if policy.max_runs > 0 && counted.len() >= policy.max_runs {
        let oldest = counted
            .iter()
            .filter_map(|t| parse_iso(&t.at).ok())
            .min()
            .unwrap_or(at);
        let frees_in = oldest + Duration::hours(policy.window_hours) - at;
        let minutes = (frees_in.num_seconds().div_euclid(60) + 1).max(1) as u32;
        return Decision {
            should_run: false,
            reason: "throttled".into(),
            todo: None,
            interval_min: if exhausted { None } else { Some(minutes) },
        };
    }
    Decision {
        should_run: true,
        reason: "ready".into(),
        todo: Some(state.todos[runnable[0]].clone()),
        interval_min: Some(interval(state, 0)),
    }
}

// --- writes ---------------------------------------------------------------

pub fn record(
    state: &mut State,
    outcome: &str,
    todo_id: Option<&str>,
    note: &str,
    who: &HostSession,
) -> Result<Tick> {
    if !OUTCOMES.contains(&outcome) {
        bail!("invalid outcome {outcome:?}");
    }
    let bump = matches!(outcome, "done" | "progress") as u64;
    let tick = Tick {
        at: format_iso(&now()),
        round: current_round(&state.ticks) + bump,
        todo: todo_id.map(str::to_string),
        outcome: outcome.to_string(),
        note: note.to_string(),
        host: Some(who.host.as_str().to_string()),
        session: who.session.clone(),
        log: None,
        cost_usd: None,
        duration_ms: None,
        num_turns: None,
        documented: None,
        extra: Map::new(),
    };
    state.ticks.push(tick.clone());
    Ok(tick)
}

/// The single write-back: record a tick, move the todo, optionally append a successor.
/// Returns the tick and the index of the affected todo.
pub fn apply_done(
    state: &mut State,
    id: &str,
    outcome: &str,
    note: &str,
    block: Option<&str>,
    next_text: Option<&str>,
    who: &HostSession,
) -> Result<(Tick, usize)> {
    let idx = todo::index_of(state, id)?;
    let status = state.todos[idx].status.clone();
    if todo::is_terminal(&status) {
        bail!("{id} is already {status}");
    }
    let tick = if let Some(question) = block {
        todo::set_status(state, id, "blocked", Some(question))?;
        let t = &mut state.todos[idx];
        if !t.blocked_by.iter().any(|d| d == todo::USER) {
            t.blocked_by.push(todo::USER.to_string());
        }
        record(state, "block", Some(id), question, who)?
    } else if outcome == "done" {
        todo::set_status(state, id, "done", Some(note))?;
        record(state, "done", Some(id), note, who)?
    } else if outcome == "progress" || outcome == "fail" {
        {
            let t = &mut state.todos[idx];
            if !note.is_empty() {
                t.note = note.to_string();
            }
            t.updated_at = crate::state::now_iso();
        }
        record(state, outcome, Some(id), note, who)?
    } else {
        bail!("invalid outcome {outcome:?}; expected done, progress or fail");
    };

    if let Some(text) = next_text {
        let priority = state.todos[idx].priority;
        if let Some((p, body)) = todo::parse_line(text, priority) {
            todo::insert_after(state, id, &body, Some(p))?;
        }
    }
    if todo::open_ordered(state).is_empty() {
        state.goal.status = "done".into();
    }
    Ok((tick, idx))
}

// --- projection -----------------------------------------------------------

pub fn last_summary(state: &State) -> Option<String> {
    state.ticks.iter().rev().find(|t| t.outcome != "noop").map(|t| {
        let head = match &t.todo {
            Some(id) => format!("{id} {}", t.outcome),
            None => t.outcome.clone(),
        };
        if t.note.is_empty() {
            head
        } else {
            format!("{head}: {}", t.note)
        }
    })
}

pub fn to_json(decision: &Decision, state: &State) -> Value {
    let todo = decision.todo.as_ref();
    json!({
        "goal": state.goal.text,
        "round": current_round(&state.ticks),
        "should_run": decision.should_run,
        "reason": decision.reason,
        "todo": todo.map(|t| {
            let mut o = json!({"id": t.id, "text": t.text, "priority": t.priority});
            if let Some(a) = &t.acceptance {
                o["acceptance"] = Value::String(a.clone());
            }
            o
        }),
        "remaining": todo::remaining(state),
        "last": last_summary(state),
        "writeback": todo.map(|t| format!("zloop done {} --note '<一句话结果>'", t.id)),
        "interval_min": decision.interval_min,
    })
}
