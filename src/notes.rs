//! `zloop remember`: durable lessons for future rounds (Beads `bd remember`, Anthropic progress notes).
//!
//! Plain Markdown bullets in `.zloop/NOTES.md`; the newest few are shown in `zloop context`.

use crate::state::{now_iso, STATE_DIR};
use anyhow::Result;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

pub const NOTES_FILE: &str = "NOTES.md";

pub fn path(root: &Path) -> PathBuf {
    root.join(STATE_DIR).join(NOTES_FILE)
}

pub fn remember(root: &Path, text: &str) -> Result<PathBuf> {
    let p = path(root);
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent)?;
    }
    let fresh = !p.exists();
    let mut f = fs::OpenOptions::new().create(true).append(true).open(&p)?;
    if fresh {
        writeln!(f, "# zloop notes\n\n_written by `zloop remember`; newest entries appear in `zloop context`_\n")?;
    }
    let one_line = text.trim().replace('\n', " ");
    writeln!(f, "- {} {}", now_iso(), one_line)?;
    Ok(p)
}

/// The newest `n` remembered lines (without the timestamp prefix), oldest first.
pub fn recent(root: &Path, n: usize) -> Vec<String> {
    let Ok(raw) = fs::read_to_string(path(root)) else { return Vec::new() };
    let mut items: Vec<String> = raw
        .lines()
        .filter(|l| l.starts_with("- "))
        .map(|l| l[2..].trim().to_string())
        .collect();
    let keep = items.len().saturating_sub(n);
    items.drain(..keep);
    items
}
