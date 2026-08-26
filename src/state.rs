//! State storage: one JSON file, atomic writes, sibling lock file.
//!
//! The JSON file is the only source of truth. Unknown keys are preserved on
//! round-trip so the Python implementation and this one can share a file.

use anyhow::{anyhow, Result};
use chrono::{DateTime, FixedOffset, Local, NaiveDateTime, SecondsFormat, TimeZone};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

pub const VERSION: u64 = 1;
pub const STATE_DIR: &str = ".zloop";
pub const STATE_FILE: &str = "state.json";

/// Raised when the state file is missing, corrupt, or locked. Maps to exit code 1.
#[derive(Debug)]
pub struct StateError(pub String);

impl fmt::Display for StateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for StateError {}

fn d_window() -> i64 {
    24
}
fn d_max_runs() -> usize {
    60
}
fn d_fail() -> usize {
    3
}
fn d_noop() -> usize {
    3
}
fn d_intervals() -> Vec<u32> {
    vec![3, 10, 30]
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Goal {
    pub id: String,
    pub text: String,
    pub status: String,
    pub created_at: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Policy {
    #[serde(default = "d_window")]
    pub window_hours: i64,
    #[serde(default = "d_max_runs")]
    pub max_runs: usize,
    #[serde(default = "d_fail")]
    pub max_fail_streak: usize,
    #[serde(default = "d_noop")]
    pub max_noop_streak: usize,
    #[serde(default = "d_intervals")]
    pub intervals_min: Vec<u32>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for Policy {
    fn default() -> Self {
        Policy {
            window_hours: d_window(),
            max_runs: d_max_runs(),
            max_fail_streak: d_fail(),
            max_noop_streak: d_noop(),
            intervals_min: d_intervals(),
            extra: Map::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Todo {
    pub id: String,
    pub text: String,
    pub priority: u8,
    pub status: String,
    #[serde(default)]
    pub blocked_by: Vec<String>,
    #[serde(default)]
    pub note: String,
    pub updated_at: String,
    #[serde(default)]
    pub done_at: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tick {
    pub at: String,
    pub round: u64,
    pub todo: Option<String>,
    pub outcome: String,
    #[serde(default)]
    pub note: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct State {
    pub version: u64,
    pub goal: Goal,
    #[serde(default)]
    pub policy: Policy,
    pub todos: Vec<Todo>,
    pub ticks: Vec<Tick>,
    pub next_id: u64,
    #[serde(default)]
    pub updated_at: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

pub fn now() -> DateTime<FixedOffset> {
    Local::now().fixed_offset()
}

pub fn now_iso() -> String {
    now().to_rfc3339_opts(SecondsFormat::Secs, false)
}

pub fn format_iso(dt: &DateTime<FixedOffset>) -> String {
    dt.to_rfc3339_opts(SecondsFormat::Secs, false)
}

/// Parse an ISO-8601 timestamp; naive values are interpreted as local time.
pub fn parse_iso(value: &str) -> Result<DateTime<FixedOffset>> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(value) {
        return Ok(dt);
    }
    let naive = NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S%.f")
        .or_else(|_| NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S"))
        .map_err(|e| anyhow!("bad timestamp {value:?}: {e}"))?;
    Local
        .from_local_datetime(&naive)
        .single()
        .map(|dt| dt.fixed_offset())
        .ok_or_else(|| anyhow!("ambiguous local timestamp {value:?}"))
}

/// Walk up from `start` (default: cwd) to the directory holding `.zloop/state.json`.
pub fn find_root(start: Option<&Path>) -> PathBuf {
    let base = match start {
        Some(p) => p.to_path_buf(),
        None => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
    };
    let base = base.canonicalize().unwrap_or(base);
    for candidate in base.ancestors() {
        if candidate.join(STATE_DIR).join(STATE_FILE).is_file() {
            return candidate.to_path_buf();
        }
    }
    base
}

pub fn state_path(root: &Path) -> PathBuf {
    root.join(STATE_DIR).join(STATE_FILE)
}

pub fn default_state(goal_text: &str, goal_id: &str) -> State {
    let ts = now_iso();
    State {
        version: VERSION,
        goal: Goal {
            id: goal_id.to_string(),
            text: goal_text.to_string(),
            status: "active".into(),
            created_at: ts.clone(),
            extra: Map::new(),
        },
        policy: Policy::default(),
        todos: Vec::new(),
        ticks: Vec::new(),
        next_id: 1,
        updated_at: ts,
        extra: Map::new(),
    }
}

pub fn load(path: &Path) -> Result<State> {
    if !path.is_file() {
        return Err(StateError(format!(
            "no zloop state at {} (run `zloop init \"<goal>\"` first)",
            path.display()
        ))
        .into());
    }
    let raw = fs::read_to_string(path)?;
    let value: Value = serde_json::from_str(&raw)
        .map_err(|e| StateError(format!("corrupt state file {}: {e}", path.display())))?;
    let version = value.get("version").and_then(Value::as_u64);
    if version != Some(VERSION) {
        return Err(StateError(format!(
            "unsupported state version in {} (expected {VERSION})",
            path.display()
        ))
        .into());
    }
    let missing: Vec<&str> = ["goal", "policy", "todos", "ticks", "next_id"]
        .into_iter()
        .filter(|k| value.get(k).is_none())
        .collect();
    if !missing.is_empty() {
        return Err(StateError(format!(
            "state file {} is missing keys: {}",
            path.display(),
            missing.join(", ")
        ))
        .into());
    }
    serde_json::from_value(value)
        .map_err(|e| StateError(format!("invalid state file {}: {e}", path.display())).into())
}

/// Atomic write: temp file + fsync + rename.
pub fn save(path: &Path, state: &mut State) -> Result<()> {
    state.updated_at = now_iso();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_file_name(format!(
        "{}.tmp",
        path.file_name().map(|s| s.to_string_lossy()).unwrap_or_default()
    ));
    {
        let mut fh = fs::File::create(&tmp)?;
        let mut body = serde_json::to_string_pretty(state)?;
        body.push('\n');
        fh.write_all(body.as_bytes())?;
        fh.sync_all()?;
    }
    fs::rename(&tmp, path)?;
    Ok(())
}

pub fn lock_path(path: &Path) -> PathBuf {
    path.with_file_name(format!(
        "{}.lock",
        path.file_name().map(|s| s.to_string_lossy()).unwrap_or_default()
    ))
}

/// Run `f` while holding an exclusive advisory lock on the sibling `.lock` file.
pub fn locked<T>(path: &Path, timeout: Duration, f: impl FnOnce() -> Result<T>) -> Result<T> {
    let lock_path = lock_path(path);
    if let Some(parent) = lock_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)?;
    let mut lock = fd_lock::RwLock::new(file);
    let deadline = Instant::now() + timeout;
    let guard = loop {
        match lock.try_write() {
            Ok(g) => break g,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Err(StateError(format!(
                        "could not lock {} within {:.1}s",
                        lock_path.display(),
                        timeout.as_secs_f64()
                    ))
                    .into());
                }
                thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(e.into()),
        }
    };
    let result = f();
    drop(guard);
    result
}

/// Lock, load, mutate, save.
pub fn transaction<T>(path: &Path, f: impl FnOnce(&mut State) -> Result<T>) -> Result<T> {
    locked(path, Duration::from_secs(5), || {
        let mut state = load(path)?;
        let out = f(&mut state)?;
        save(path, &mut state)?;
        Ok(out)
    })
}
