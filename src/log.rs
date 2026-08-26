//! Execution records: one Markdown file per non-noop tick under `.zloop/log/`.

use crate::session::{self, Host};
use crate::state::{parse_iso, State, Tick, Todo, STATE_DIR};
use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

pub const LOG_DIR: &str = "log";

/// Resolve `--evidence` input: `@path` reads a file, anything else is literal text.
pub fn resolve_evidence(raw: Option<&str>) -> Result<Option<String>> {
    match raw {
        None => Ok(None),
        Some(s) if s.starts_with('@') => {
            let path = &s[1..];
            let body = fs::read_to_string(path).with_context(|| format!("reading evidence file {path}"))?;
            Ok(Some(body))
        }
        Some(s) => Ok(Some(s.to_string())),
    }
}

/// Write the log file for a tick; returns the path relative to `.zloop/`.
pub fn write(root: &Path, state: &State, tick: &Tick, todo: &Todo, evidence: Option<&str>) -> Result<String> {
    let dir = root.join(STATE_DIR).join(LOG_DIR);
    fs::create_dir_all(&dir)?;
    let stamp = parse_iso(&tick.at)
        .map(|dt| dt.format("%Y%m%d-%H%M%S").to_string())
        .unwrap_or_else(|_| tick.at.replace([':', '-', 'T', '+'], ""));
    let base = format!("{stamp}-{}-{}", todo.id, tick.outcome);
    let mut name = format!("{base}.md");
    let mut n = 1;
    while dir.join(&name).exists() {
        n += 1;
        name = format!("{base}-{n}.md");
    }
    let host = tick.host.clone().unwrap_or_else(|| "cli".into());
    let session = tick.session.clone().unwrap_or_else(|| "-".into());
    let resume = tick
        .session
        .as_deref()
        .and_then(|s| Host::parse(&host).and_then(|h| session::resume_command(h, s)))
        .map(|c| format!("`{c}`"))
        .unwrap_or_else(|| "-".into());
    let mut body = String::new();
    body.push_str(&format!("# {} · {} · {}\n\n", todo.id, tick.outcome, tick.at));
    body.push_str(&format!("- goal: {}\n", state.goal.text));
    body.push_str(&format!("- todo: [P{}] {}\n", todo.priority, todo.text));
    body.push_str(&format!("- outcome: {}   round: {}\n", tick.outcome, tick.round));
    body.push_str(&format!("- host: {host}   session: {session}   resume: {resume}\n"));
    if !tick.note.is_empty() {
        body.push_str(&format!("- note: {}\n", tick.note));
    }
    if let Some(ev) = evidence {
        let ev = ev.trim_end();
        if !ev.is_empty() {
            body.push_str("\n## Evidence\n\n");
            body.push_str(ev);
            body.push('\n');
        }
    }
    fs::write(dir.join(&name), body)?;
    Ok(format!("{LOG_DIR}/{name}"))
}

/// Log files, newest first, optionally filtered by todo id.
pub fn entries(root: &Path, todo: Option<&str>, last: usize) -> Result<Vec<PathBuf>> {
    let dir = root.join(STATE_DIR).join(LOG_DIR);
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut files: Vec<PathBuf> = fs::read_dir(&dir)?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().map(|e| e == "md").unwrap_or(false))
        .filter(|p| match todo {
            Some(id) => p
                .file_name()
                .map(|n| n.to_string_lossy().contains(&format!("-{id}-")))
                .unwrap_or(false),
            None => true,
        })
        .collect();
    files.sort();
    files.reverse();
    files.truncate(last);
    Ok(files)
}

pub fn first_line(path: &Path) -> String {
    fs::read_to_string(path)
        .ok()
        .and_then(|s| s.lines().next().map(|l| l.trim_start_matches('#').trim().to_string()))
        .unwrap_or_default()
}
