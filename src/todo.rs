//! Todo queue: `[Pn] text` parsing, ordering, executability, status transitions.

use crate::state::{now_iso, State, Todo};
use anyhow::{anyhow, bail, Result};
use serde_json::Map;

pub const STATUSES: [&str; 4] = ["open", "blocked", "deferred", "done"];
pub const DEFAULT_PRIORITY: u8 = 1;
/// Special `blocked_by` marker: waiting on a human.
pub const USER: &str = "user";

pub fn is_terminal(status: &str) -> bool {
    matches!(status, "done" | "deferred")
}

/// 这条 todo 还有没有机会走到 `done`——「等它的那条会不会永远轮不到」就看这个。
///
/// `open` 会被派出去；`blocked` 等的是人，人一答 `edit --status open` 就回到队列里；
/// `done` 本来就满足依赖。剩下的两种走不到：`deferred` 被 [`open_ordered`] 过滤掉，
/// 手改进来的野状态（`cancelled`）过不了 [`is_executable`] 的 `status == "open"`。
///
/// `doctor` 的 `dead_blocked_by` 和 `status` 清单里的「等不到」共用这一个定义——
/// 分成两份写过一次就会走散：一块屏幕报警、另一块说一切正常。
pub fn can_still_finish(status: &str) -> bool {
    matches!(status, "open" | "blocked" | "done")
}

/// 这条 todo 等的依赖里，哪些已经永远变不成 `done`——**它就永远轮不到**。
///
/// 三种死法，判据和 `doctor` 的 `dangling_blocked_by` + `dead_blocked_by` 同源：
/// 依赖压根不在清单里（`compact` 把它搬走了）、依赖已延后、依赖的状态被手改成
/// zloop 不认的词。[`is_executable`] 要依赖 `done` 才放行，这三种都再也派不出去。
///
/// 终态的 todo 返回空：`done` / `deferred` 的那条不在等谁，给它印「等不到」是噪音。
/// `user` 也不算——等人不是等 todo，人答一句 `edit --status open` 就放行。
///
/// **四个读者共用这一个判据**（`status` 的清单、`context` 的交接包、`status --md`、
/// `edit` 的回显）：分开写就会走散。t36 只在 `status` 里判、判的还只是"第一条没 done
/// 的依赖"，于是 `blocked_by [t1(open), t4(deferred)]` 这种 doctor 退 1 大喊永远轮不到、
/// status 照旧印「⏳ 等 t1」。
pub fn dead_deps<'a>(state: &'a State, t: &'a Todo) -> Vec<&'a str> {
    if is_terminal(&t.status) {
        return Vec::new();
    }
    let mut out: Vec<&str> = Vec::new();
    for d in &t.blocked_by {
        let d = d.as_str();
        // 重复 id 只报一次：`blocked_by t4,t4` 印成「等不到 t4,t4」是噪音
        if d == USER || out.contains(&d) {
            continue;
        }
        // 重复 id 只认第一条，和 `index_of` / `is_executable` 保持一致
        let dead = match state.todos.iter().find(|x| x.id == d) {
            None => true,
            Some(dep) => !can_still_finish(&dep.status),
        };
        if dead {
            out.push(d);
        }
    }
    out
}

/// 死等的出口命令。方向不能反：依赖还在清单里就把**依赖**捡回来，已经不在了
/// 只能把**这条**的依赖断开——指着人去 `edit` 一条不存在的 todo 是死路。
pub fn dead_dep_fix(state: &State, t: &Todo, dep: &str) -> String {
    if state.todos.iter().any(|x| x.id == dep) {
        format!("zloop edit {dep} --status open")
    } else {
        format!("zloop edit {} --blocked-by ''", t.id)
    }
}

/// Parse one plan line: optional bullet, optional `[Pn]` prefix, then text.
pub fn parse_line(line: &str, default_priority: u8) -> Option<(u8, String)> {
    let mut text = line.trim();
    if text.is_empty() || text.starts_with('#') {
        return None;
    }
    if let Some(rest) = text.strip_prefix('-').or_else(|| text.strip_prefix('*')) {
        if rest.starts_with(char::is_whitespace) {
            text = rest.trim_start();
        }
    }
    let mut priority = default_priority;
    if let Some(rest) = text.strip_prefix('[') {
        if let Some(end) = rest.find(']') {
            let inner = rest[..end].trim();
            let mut chars = inner.chars();
            if let (Some(p), Some(d), None) = (chars.next(), chars.next(), chars.next()) {
                if (p == 'P' || p == 'p') && d.is_ascii_digit() && d <= '4' {
                    priority = d.to_digit(10).unwrap() as u8;
                    text = rest[end + 1..].trim_start();
                }
            }
        }
    }
    let body = text.trim();
    if body.is_empty() {
        None
    } else {
        Some((priority, body.to_string()))
    }
}

pub fn parse_plan(text: &str, default_priority: u8) -> Vec<(u8, String)> {
    text.lines().filter_map(|l| parse_line(l, default_priority)).collect()
}

fn strip_html_comments(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find("<!--") {
        out.push_str(&rest[..start]);
        match rest[start..].find("-->") {
            Some(end) => rest = &rest[start + end + 3..],
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Import open checkbox todos from a loopx `ACTIVE_GOAL_STATE.md`.
///
/// Keeps `- [ ] [Pn] text` lines, drops done `[x]` / deferred `[-]` ones and the
/// `<!-- loopx:todo … -->` metadata comments. Priority prefixes carry over.
pub fn parse_loopx_state(text: &str) -> Vec<(u8, String)> {
    let mut items = Vec::new();
    for line in text.lines() {
        let s = line.trim_start();
        let Some(rest) = s.strip_prefix('-').or_else(|| s.strip_prefix('*')) else { continue };
        if !rest.starts_with(char::is_whitespace) {
            continue;
        }
        let rest = rest.trim_start();
        let mut chars = rest.chars();
        let (Some('['), Some(mark), Some(']')) = (chars.next(), chars.next(), chars.next()) else { continue };
        if !matches!(mark, ' ' | 'x' | 'X' | '-') {
            continue;
        }
        let after = chars.as_str();
        if !after.starts_with(char::is_whitespace) {
            continue;
        }
        if mark != ' ' {
            continue;
        }
        let body = strip_html_comments(after.trim());
        if let Some(item) = parse_line(&body, DEFAULT_PRIORITY) {
            items.push(item);
        }
    }
    items
}

/// Split `text :: acceptance` into the todo text and its optional acceptance criteria.
pub fn split_acceptance(raw: &str) -> (String, Option<String>) {
    match raw.split_once(" :: ") {
        Some((text, acc)) if !acc.trim().is_empty() && !text.trim().is_empty() => {
            (text.trim().to_string(), Some(acc.trim().to_string()))
        }
        _ => (raw.trim().to_string(), None),
    }
}

fn new_todo(state: &mut State, text: &str, priority: u8) -> Todo {
    let id = format!("t{}", state.next_id);
    state.next_id += 1;
    let (text, acceptance) = split_acceptance(text);
    Todo {
        id,
        text,
        priority,
        status: "open".into(),
        blocked_by: Vec::new(),
        note: String::new(),
        updated_at: now_iso(),
        done_at: None,
        acceptance,
        extra: Map::new(),
    }
}

/// Append todos in plan order. `replace` drops every non-terminal todo first.
pub fn add(state: &mut State, items: &[(u8, String)], replace: bool) -> Vec<Todo> {
    if replace {
        state.todos.retain(|t| is_terminal(&t.status));
    }
    let created: Vec<Todo> = items.iter().map(|(p, text)| new_todo(state, text, *p)).collect();
    state.todos.extend(created.iter().cloned());
    created
}

pub fn index_of(state: &State, id: &str) -> Result<usize> {
    state.todos.iter().position(|t| t.id == id).ok_or_else(|| anyhow!("unknown todo id {id:?}"))
}

pub fn insert_after(state: &mut State, after_id: &str, text: &str, priority: Option<u8>) -> Result<Todo> {
    let idx = index_of(state, after_id)?;
    let priority = priority.unwrap_or(state.todos[idx].priority);
    let todo = new_todo(state, text, priority);
    state.todos.insert(idx + 1, todo.clone());
    Ok(todo)
}

pub fn is_executable(todo: &Todo, state: &State) -> bool {
    if todo.status != "open" {
        return false;
    }
    todo.blocked_by.iter().all(|dep| dep != USER && state.todos.iter().any(|t| &t.id == dep && t.status == "done"))
}

/// Indices of non-terminal todos sorted by (priority, write order).
pub fn open_ordered(state: &State) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..state.todos.len()).filter(|&i| !is_terminal(&state.todos[i].status)).collect();
    idx.sort_by_key(|&i| (state.todos[i].priority, i));
    idx
}

pub fn executable(state: &State) -> Vec<usize> {
    open_ordered(state).into_iter().filter(|&i| is_executable(&state.todos[i], state)).collect()
}

pub fn remaining(state: &State) -> usize {
    state.todos.iter().filter(|t| !is_terminal(&t.status)).count()
}

/// 清单不空、一条都没做完，剩下的全被推到了以后。
///
/// `is_terminal` 把 done 和 deferred 一视同仁，所以这种状态在"还有没有活"这一层
/// 和"全做完了"长得一模一样——但出口动作是相反的：一个该开新目标，一个该
/// `zloop edit <id> --status open` 把活捡回来。别让它俩共用一个词（同 `unplanned`）。
pub fn all_deferred(state: &State) -> bool {
    !state.todos.is_empty() && state.todos.iter().all(|t| t.status == "deferred")
}

pub fn set_status(state: &mut State, id: &str, status: &str, note: Option<&str>) -> Result<usize> {
    if !STATUSES.contains(&status) {
        bail!("invalid status {status:?}; expected one of {}", STATUSES.join(", "));
    }
    let idx = index_of(state, id)?;
    let todo = &mut state.todos[idx];
    todo.status = status.to_string();
    todo.updated_at = now_iso();
    todo.done_at = if status == "done" { Some(todo.updated_at.clone()) } else { None };
    if status == "open" {
        todo.blocked_by.retain(|d| d != USER);
    }
    if let Some(n) = note {
        todo.note = n.to_string();
    }
    Ok(idx)
}
