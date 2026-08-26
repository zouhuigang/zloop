//! Host + session detection, resume commands, transcript locations.
//!
//! Inside a Claude Code session every subprocess sees `CLAUDE_CODE_SESSION_ID`;
//! inside Codex it sees `CODEX_THREAD_ID`. Recording those per tick is what makes
//! `claude --resume <id>` / `codex resume <id>` possible later.

use crate::state::State;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Host {
    Claude,
    Codex,
    Cli,
}

impl Host {
    pub fn as_str(&self) -> &'static str {
        match self {
            Host::Claude => "claude",
            Host::Codex => "codex",
            Host::Cli => "cli",
        }
    }
    pub fn parse(s: &str) -> Option<Host> {
        match s {
            "claude" | "claude-code" => Some(Host::Claude),
            "codex" | "codex-app" | "codex-cli" => Some(Host::Codex),
            "cli" => Some(Host::Cli),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct HostSession {
    pub host: Host,
    pub session: Option<String>,
}

pub fn detect() -> HostSession {
    if let Ok(id) = std::env::var("CLAUDE_CODE_SESSION_ID") {
        if !id.trim().is_empty() {
            return HostSession { host: Host::Claude, session: Some(id) };
        }
    }
    if let Ok(id) = std::env::var("CODEX_THREAD_ID") {
        if !id.trim().is_empty() {
            return HostSession { host: Host::Codex, session: Some(id) };
        }
    }
    HostSession { host: Host::Cli, session: None }
}

pub fn resume_command(host: Host, session: &str) -> Option<String> {
    match host {
        Host::Claude => Some(format!("claude --resume {session}")),
        Host::Codex => Some(format!("codex resume {session}")),
        Host::Cli => None,
    }
}

/// Claude Code stores transcripts under `~/.claude/projects/<cwd with '/' -> '-'>/<id>.jsonl`.
pub fn transcript_path(host: Host, session: &str, root: &Path) -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    match host {
        Host::Claude => {
            let projects = home.join(".claude").join("projects");
            let file = format!("{session}.jsonl");
            // 1) the project root's slug, 2) the current working directory's slug,
            // 3) any project directory (the session may have been started elsewhere).
            let mut candidates = vec![root.to_path_buf()];
            if let Ok(cwd) = std::env::current_dir() {
                candidates.push(cwd);
            }
            for dir in candidates {
                let slug = dir.to_string_lossy().replace('/', "-");
                let p = projects.join(&slug).join(&file);
                if p.exists() {
                    return Some(p);
                }
            }
            if let Ok(entries) = std::fs::read_dir(&projects) {
                for e in entries.flatten() {
                    let p = e.path().join(&file);
                    if p.exists() {
                        return Some(p);
                    }
                }
            }
            let slug = root.to_string_lossy().replace('/', "-");
            Some(projects.join(slug).join(file))
        }
        Host::Codex => {
            // ~/.codex/sessions/YYYY/MM/DD/rollout-<ts>-<id>.jsonl
            let base = home.join(".codex").join("sessions");
            let needle = format!("-{session}.jsonl");
            for year in std::fs::read_dir(base).ok()?.flatten() {
                for month in std::fs::read_dir(year.path()).ok()?.flatten() {
                    for day in std::fs::read_dir(month.path()).ok()?.flatten() {
                        for f in std::fs::read_dir(day.path()).ok()?.flatten() {
                            if f.file_name().to_string_lossy().ends_with(&needle) {
                                return Some(f.path());
                            }
                        }
                    }
                }
            }
            None
        }
        Host::Cli => None,
    }
}

#[derive(Debug, Clone)]
pub struct SessionRow {
    pub host: String,
    pub session: String,
    pub ticks: usize,
    pub first: String,
    pub last: String,
    pub todos: Vec<String>,
    pub resume: Option<String>,
    pub transcript: Option<PathBuf>,
}

/// Distinct (host, session) pairs seen in ticks, oldest first.
pub fn summarize(state: &State, root: &Path) -> Vec<SessionRow> {
    let mut rows: BTreeMap<(String, String), SessionRow> = BTreeMap::new();
    let mut order: Vec<(String, String)> = Vec::new();
    for tick in &state.ticks {
        let (Some(host), Some(session)) = (&tick.host, &tick.session) else { continue };
        let key = (host.clone(), session.clone());
        let row = rows.entry(key.clone()).or_insert_with(|| {
            order.push(key.clone());
            let h = Host::parse(host).unwrap_or(Host::Cli);
            SessionRow {
                host: host.clone(),
                session: session.clone(),
                ticks: 0,
                first: tick.at.clone(),
                last: tick.at.clone(),
                todos: Vec::new(),
                resume: resume_command(h, session),
                transcript: transcript_path(h, session, root),
            }
        });
        row.ticks += 1;
        row.last = tick.at.clone();
        if let Some(t) = &tick.todo {
            if !row.todos.contains(t) {
                row.todos.push(t.clone());
            }
        }
    }
    order.into_iter().filter_map(|k| rows.remove(&k)).collect()
}
