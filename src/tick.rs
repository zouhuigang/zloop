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
/// `feedback` 是唯一一个**人写的** outcome（`zloop feedback`）：agent 自述之外的另一路信号。
/// 它不计入 `COUNTED`（不吃配额、不推进轮次），但会打断 fail / noop / progress 三条 streak——
/// 人开口说话正是"停下来等人"该等到的东西，等到了就该让循环继续。
pub const OUTCOMES: [&str; 9] =
    ["done", "progress", "fail", "block", "noop", "edit", "feedback", "reflect", "replan"];
pub const FEEDBACK: &str = "feedback";
/// 回看的那一轮：不做 todo，只读账本 + 经验 + 反馈，给出整理建议。
///
/// 它对三条 streak **透明**（和 `noop` 一样）——插一轮反思不代表"失败被解决了"，
/// 否则 fail / fail / reflect / fail / fail 会让循环永远停不下来。
pub const REFLECT: &str = "reflect";
/// 重估计划的那一轮：不做 todo，只对着最终目标看剩下的任务还对不对。
/// 和 `reflect` 一样对三条 streak 透明——插一轮重估不代表失败被解决了。
pub const REPLAN: &str = "replan";

/// streak 计数时要跳过的轮次：它们不代表干活的结果。
fn transparent(outcome: &str) -> bool {
    outcome == "noop" || outcome == REFLECT || outcome == REPLAN
}

/// 上一轮干活之后才到的反馈——也就是下一轮**必须先处理**的那些。
/// 更早的反馈留在 `ticks` 和 `zloop doc` 里，不再往交接包里堆。
pub fn pending_feedback(state: &State) -> Vec<&Tick> {
    let last_work = state.ticks.iter().rposition(|t| t.outcome == "done" || t.outcome == "progress");
    state
        .ticks
        .iter()
        .enumerate()
        .filter(|(i, t)| t.outcome == FEEDBACK && last_work.is_none_or(|k| *i > k))
        .map(|(_, t)| t)
        .collect()
}

/// 这个目标失败 / 被挡过的地方，最近的在前。
///
/// 循环因为连续失败停下来是对的，但"停下来"不等于"学到"——如果失败的原因没有结构化落点，
/// 下一轮（甚至下一个会话）会把同一个坑再踩一遍。所以把 fail / block 轮次连同它们记下的坑
/// 一起摆进交接包。
pub fn failures(state: &State) -> Vec<&Tick> {
    state.ticks.iter().rev().filter(|t| t.outcome == "fail" || t.outcome == "block").collect()
}

/// 全部反馈条数（含已经被后续轮次消化掉的）。
pub fn feedback_count(state: &State) -> usize {
    state.ticks.iter().filter(|t| t.outcome == FEEDBACK).count()
}

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
            o if transparent(o) => continue,
            _ => break,
        }
    }
    n
}

pub fn noop_streak(ticks: &[Tick]) -> usize {
    ticks.iter().rev().filter(|t| !transparent(&t.outcome) || t.outcome == "noop").take_while(|t| t.outcome == "noop").count()
}

/// Trailing consecutive `progress` ticks on `todo_id`; `noop` is transparent, anything else breaks it.
pub fn progress_streak(ticks: &[Tick], todo_id: &str) -> usize {
    let mut n = 0;
    for t in ticks.iter().rev() {
        match t.outcome.as_str() {
            o if transparent(o) => continue,
            "progress" if t.todo.as_deref() == Some(todo_id) => n += 1,
            _ => break,
        }
    }
    n
}

/// 这条活是不是**另一个会话**刚领走、还在做？
///
/// `next` 曾经无条件覆盖 `in_progress`，于是两个 Claude 会话会同时领到同一条 todo：
/// 谁都以为自己拿着，两个 agent 改同一批文件，先写回的那个还把另一个的在飞状态一起清掉。
/// 判断只在**交互式派活**（`via == "next"`）之间做：runner 自己设 `in_progress`，
/// 不走这条路，所以无头循环不会被自己挡住。
///
/// `policy.stale_after_min` 决定"多久没动静就算被丢下了"——过期的派活照旧可以重派，
/// 设成 0 等于关掉这个保护。
/// `next` 撞上别人的派活时给出的决定：不跑，但过一会儿可以再来问——
/// 派活会因 `stale_after_min` 过期，所以自动续跑的循环能自己恢复，不必等人。
pub fn hold_decision(state: &State) -> Decision {
    Decision {
        should_run: false,
        reason: "held_by_other".into(),
        todo: None,
        interval_min: Some(interval(state, 1)),
    }
}

pub fn held_by_other(
    state: &State,
    who: &HostSession,
    at: DateTime<FixedOffset>,
) -> Option<crate::state::InProgress> {
    let ip = state.in_progress.as_ref()?;
    if ip.via != "next" || state.policy.stale_after_min <= 0 {
        return None;
    }
    // 分不出是谁就不拦：裸 CLI 没有 session id，拦了只会把人锁在门外
    let (Some(held), Some(mine)) = (ip.session.as_deref(), who.session.as_deref()) else {
        return None;
    };
    if held == mine && ip.host.as_deref() == Some(who.host.as_str()) {
        return None;
    }
    let stale = parse_iso(&ip.started_at)
        .map(|s| (at - s).num_minutes() >= state.policy.stale_after_min)
        .unwrap_or(true);
    (!stale).then(|| ip.clone())
}

pub fn current_round(ticks: &[Tick]) -> u64 {
    ticks.iter().filter(|t| t.outcome == "done" || t.outcome == "progress").count() as u64
}

/// 「跑了几轮」：干活的轮次（`COUNTED`：done / progress / fail），失败也算跑过。
///
/// `status` 和 `stats` 必须共用这一个定义，否则同一份账本会报出两个数——
/// 曾经 `status` 只排除 `noop`，于是 3 条 todo + 1 次回看被它算成 4 轮，而 `stats` 报 3 轮。
/// reflect / replan / feedback / edit / block 都不是"跑了一轮活"，不进这个数。
pub fn rounds(ticks: &[Tick]) -> usize {
    ticks.iter().filter(|t| COUNTED.contains(&t.outcome.as_str())).count()
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
        pitfalls: Vec::new(),
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
