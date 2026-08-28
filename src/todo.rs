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
    text.lines()
        .filter_map(|l| parse_line(l, default_priority))
        .collect()
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
    let created: Vec<Todo> = items
        .iter()
        .map(|(p, text)| new_todo(state, text, *p))
        .collect();
    state.todos.extend(created.iter().cloned());
    created
}

pub fn index_of(state: &State, id: &str) -> Result<usize> {
    state
        .todos
        .iter()
        .position(|t| t.id == id)
        .ok_or_else(|| anyhow!("unknown todo id {id:?}"))
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
    todo.blocked_by.iter().all(|dep| {
        dep != USER
            && state
                .todos
                .iter()
                .any(|t| &t.id == dep && t.status == "done")
    })
}

/// Indices of non-terminal todos sorted by (priority, write order).
pub fn open_ordered(state: &State) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..state.todos.len())
        .filter(|&i| !is_terminal(&state.todos[i].status))
        .collect();
    idx.sort_by_key(|&i| (state.todos[i].priority, i));
    idx
}

pub fn executable(state: &State) -> Vec<usize> {
    open_ordered(state)
        .into_iter()
        .filter(|&i| is_executable(&state.todos[i], state))
        .collect()
}

pub fn remaining(state: &State) -> usize {
    state.todos.iter().filter(|t| !is_terminal(&t.status)).count()
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
