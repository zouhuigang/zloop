//! State storage: one JSON file, atomic writes, sibling lock file.
//!
//! The JSON file is the only source of truth. Unknown keys are preserved on
//! round-trip so the Python implementation and this one can share a file.

use anyhow::{anyhow, Result};
use chrono::{DateTime, FixedOffset, Local, NaiveDate, NaiveDateTime, SecondsFormat, TimeZone};
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
/// 480 = one round every 3 minutes for a whole day; `0` disables the brake.
fn d_max_runs() -> usize {
    480
}
fn d_fail() -> usize {
    3
}
fn d_noop() -> usize {
    3
}
/// Consecutive `progress` ticks on the same todo before the loop stops for a human.
fn d_progress() -> usize {
    8
}
/// Minutes after which an unfinished `in_progress` hand-out is flagged stale.
fn d_stale() -> i64 {
    120
}
fn d_intervals() -> Vec<u32> {
    vec![3, 10, 30]
}
fn d_require_doc() -> bool {
    true
}
fn d_require_pitfall() -> bool {
    true
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
    #[serde(default = "d_progress")]
    pub max_progress_streak: usize,
    #[serde(default = "d_stale")]
    pub stale_after_min: i64,
    #[serde(default = "d_intervals")]
    pub intervals_min: Vec<u32>,
    /// Lifetime spend cap for this goal in USD (summed from host-reported `cost_usd`); `0` = unlimited.
    #[serde(default)]
    pub max_total_usd: f64,
    /// Webhook to POST notifications to (Feishu/Lark custom-bot format is detected from the URL).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notify_url: Option<String>,
    /// Shell command to run for notifications; the event JSON arrives on stdin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notify_cmd: Option<String>,
    /// Shell command the runner executes before every round (e.g. `./init.sh && cargo test`);
    /// a non-zero exit records a `fail` tick instead of calling the host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preflight_cmd: Option<String>,
    /// Refuse `done` (outcome=done) unless it carries `--approach`, so every finished todo
    /// leaves a real technical document. `--no-doc` overrides one call.
    #[serde(default = "d_require_doc")]
    pub require_doc: bool,
    /// 同理，`--outcome fail` 必须带 `--pitfall`：失败最该留下的是"为什么不行"，
    /// 否则同一个坑下一轮还会再踩一次。`--no-doc` 同样能绕过一次。
    #[serde(default = "d_require_pitfall")]
    pub require_pitfall: bool,
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
            max_progress_streak: d_progress(),
            stale_after_min: d_stale(),
            intervals_min: d_intervals(),
            max_total_usd: 0.0,
            notify_url: None,
            notify_cmd: None,
            preflight_cmd: None,
            require_doc: d_require_doc(),
            require_pitfall: d_require_pitfall(),
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
    /// How to verify this todo is really done (`plan` line syntax: `text :: acceptance`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acceptance: Option<String>,
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
    /// Host-reported spend for the round that produced this tick (claude -p `total_cost_usd`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub num_turns: Option<u64>,
    /// Did this round leave an 实现思路 in its log? (None for rounds that write no log.)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub documented: Option<bool>,
    /// 这一轮记下的坑。**账本里也存一份**（日志文件里有渲染版）：`zloop context` 要能
    /// 直接读出"这个目标失败过的地方"，而不必回头解析一堆 Markdown；账本跟着目标走，
    /// 日志目录是项目级的——这一点在多目标下已经吃过亏（见 GOALS-REVIEW.md 的 F5）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pitfalls: Vec<String>,
    /// 这一轮学到的东西**动摇了后续计划**——写清哪条前提不成立了。
    ///
    /// 和 `pitfalls` 的区别：坑是"这条路上有个石头"，rethink 是"这条路本身不通往目标"。
    /// 和 `block` 的区别：block 要人来回话，rethink 不需要——它只是给重估一个理由。
    ///
    /// 为什么必须由写回的人主动说：zloop 读不出"策略走不通"。那一轮可能**完全成功**——
    /// 没失败、没停滞、没返工、没被挡，五个偏离信号一个都不响，可它的结论已经把剩下几条
    /// 的前提推翻了（见 `docs/ADAPTIVE-REPLAN.md` §6 缺口二的实测）。知道这件事的只有
    /// 刚干完活的那个 agent，所以给它一个说出口的地方。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rethink: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// The todo currently being worked on: set when `next` hands it out (or the runner
/// starts a round), cleared by `done`. This is what `phase` reports as "executing".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InProgress {
    pub todo: String,
    pub started_at: String,
    pub round: u64,
    /// "next" (interactive round) or "runner"
    pub via: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_progress: Option<InProgress>,
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

/// Parse a point in time the way a person types one on the command line:
/// `2h` / `30m` / `7d` (that long ago), `2026-08-29` (local midnight), or a full ISO timestamp.
/// The relative form is the one people actually reach for（`--since 2h`），所以它排在最前面。
pub fn parse_when(value: &str) -> Result<DateTime<FixedOffset>> {
    let v = value.trim();
    if let Some(digits) = v.strip_suffix(['m', 'h', 'd']) {
        if let Ok(n) = digits.parse::<i64>() {
            let span = match v.chars().last() {
                Some('m') => chrono::Duration::minutes(n),
                Some('h') => chrono::Duration::hours(n),
                _ => chrono::Duration::days(n),
            };
            return Ok(now() - span);
        }
    }
    if let Ok(date) = NaiveDate::parse_from_str(v, "%Y-%m-%d") {
        return Local
            .from_local_datetime(&date.and_hms_opt(0, 0, 0).expect("midnight is a valid time"))
            .single()
            .map(|dt| dt.fixed_offset())
            .ok_or_else(|| anyhow!("ambiguous local date {value:?}"));
    }
    parse_iso(v).map_err(|_| anyhow!("看不懂的时间 {value:?}：用 2h / 30m / 7d、2026-08-29，或完整的 ISO 时间戳"))
}

/// Walk up from `start` (default: cwd) to the directory holding `.zloop/state.json`.
pub fn find_root(start: Option<&Path>) -> PathBuf {
    let base = match start {
        Some(p) => p.to_path_buf(),
        None => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
    };
    let base = base.canonicalize().unwrap_or(base);
    for candidate in base.ancestors() {
        let dir = candidate.join(STATE_DIR);
        // `.zloop/goals/` 也算认领：目标全停着（没有当前目标）时项目仍然要找得到，
        // 否则从子目录连 `zloop goal list` 都看不见自己的目标。
        if dir.join(STATE_FILE).is_file() || dir.join(crate::goals::GOALS_DIR).is_dir() {
            return candidate.to_path_buf();
        }
    }
    base
}

/// 停在 `.zloop/goals/` 的目标数量；用于在"没有当前目标"时给出能用的下一步。
fn parked_count(state_file: &Path) -> usize {
    let Some(dir) = state_file.parent().map(|p| p.join(crate::goals::GOALS_DIR)) else { return 0 };
    fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.path().extension().map(|x| x == "json").unwrap_or(false))
                .count()
        })
        .unwrap_or(0)
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
        in_progress: None,
        extra: Map::new(),
    }
}

pub fn load(path: &Path) -> Result<State> {
    if !path.is_file() {
        let parked = parked_count(path);
        if parked > 0 {
            return Err(StateError(format!(
                "当前没有目标（{parked} 个停在 .zloop/goals/）：`zloop goal list` 看有哪些，\n                 `zloop goal switch <id>` 挑一个开进来"
            ))
            .into());
        }
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

/// 写命令等锁的上限。只读命令（`status` / `context` / `log` / `doctor` …）走 `load`，**根本不上锁**：
/// `save` 是 tmp + rename，读者只会看见换过去之前或之后的完整一份，所以它们的等待是 0，
/// 不会被一个跑着的 runner 挡住。改读路径的人请先看 `tests/lock_test.rs` 里钉这条的用例。
pub const LOCK_WAIT: Duration = Duration::from_secs(5);

/// 持有者记录：谁（pid）、在干什么（操作名）、什么时候拿到的。
///
/// 写在锁文件**旁边**（`state.json.lock.holder`）而不是锁文件里：锁文件的内容没有锁保护，
/// 就地覆写是「先截断再写」，等锁的人正好读在中间就只能读到半条 JSON。旁边这份走 tmp + rename，
/// 读者要么读到完整的旧记录、要么读到完整的新记录。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockHolder {
    pub pid: u32,
    pub op: String,
    pub at: String,
}

pub fn holder_path(path: &Path) -> PathBuf {
    let lock = lock_path(path);
    lock.with_file_name(format!(
        "{}.holder",
        lock.file_name().map(|s| s.to_string_lossy()).unwrap_or_default()
    ))
}

/// 本进程正在做的事，进持有者记录用。`cli::run` 每次开头按子命令设一次，runner 每轮再细化成轮号。
static OPERATION: std::sync::RwLock<String> = std::sync::RwLock::new(String::new());

pub fn set_operation(op: impl Into<String>) {
    if let Ok(mut cur) = OPERATION.write() {
        *cur = op.into();
    }
}

pub fn operation() -> String {
    match OPERATION.read() {
        Ok(s) if !s.is_empty() => s.clone(),
        _ => "zloop".into(),
    }
}

/// 拿到锁之后写；失败不影响正事（记录只是给人看的）。
fn write_holder(state_path: &Path) {
    let p = holder_path(state_path);
    let rec = LockHolder { pid: std::process::id(), op: operation(), at: now_iso() };
    let Ok(body) = serde_json::to_string(&rec) else { return };
    let tmp = p.with_file_name(format!(
        "{}.tmp.{}",
        p.file_name().map(|s| s.to_string_lossy()).unwrap_or_default(),
        std::process::id()
    ));
    if fs::write(&tmp, body).is_err() || fs::rename(&tmp, &p).is_err() {
        let _ = fs::remove_file(&tmp);
    }
}

/// 必须在**释放锁之前**清掉，否则可能删掉下一个持有者刚写的那份。
fn clear_holder(state_path: &Path) {
    let _ = fs::remove_file(holder_path(state_path));
}

/// 出作用域就清记录——正常返回、`?` 提前返回、闭包 panic 展开都算。
/// 它在 `locked` 里比 fd 锁的 guard **后**声明，所以先清记录、再放锁（顺序见 `clear_holder`）。
struct HolderGuard<'a>(&'a Path);

impl Drop for HolderGuard<'_> {
    fn drop(&mut self) {
        clear_holder(self.0);
    }
}

pub fn read_holder(state_path: &Path) -> Option<LockHolder> {
    serde_json::from_str(&fs::read_to_string(holder_path(state_path)).ok()?).ok()
}

/// 超时时的那句话：等谁、等多久了、那个进程还在不在，以及下一步该干什么。
fn timeout_error(state_path: &Path, timeout: Duration) -> anyhow::Error {
    let lock = lock_path(state_path);
    let mut msg = format!("could not lock {} within {:.1}s", lock.display(), timeout.as_secs_f64());
    match read_holder(state_path) {
        Some(h) => {
            let held = parse_iso(&h.at)
                .ok()
                .map(|t| format!("，拿到锁 {:.1} 秒了", (now() - t).num_milliseconds() as f64 / 1000.0))
                .unwrap_or_default();
            if crate::daemon::pid_alive(h.pid as i32) {
                msg.push_str(&format!("\n持有者：pid {} · {}{held}（进程还活着）", h.pid, h.op));
                msg.push_str(&format!(
                    "\n下一步：先看它在干什么 `ps -p {} -o command=`；确认真卡死了再 `kill {}`；\
                     \n        别删锁文件——内核锁才是权威，删了只会让两个进程同时写 state.json",
                    h.pid, h.pid
                ));
            } else {
                msg.push_str(&format!(
                    "\n持有者：记录里是 pid {} · {}{held}，但这个进程已经不在了——这条是旧记录，\
                     \n        真正持锁的是另一个进程（`lsof {}` 能看到是谁）",
                    h.pid,
                    h.op,
                    lock.display()
                ));
            }
        }
        None => msg.push_str(&format!(
            "\n持有者：没有持有者记录（旧版 zloop 持的锁，或者进程被强杀没来得及留）\
             \n下一步：`lsof {}` 看谁开着它；别删锁文件",
            lock.display()
        )),
    }
    StateError(msg).into()
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
                    return Err(timeout_error(path, timeout));
                }
                thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(e.into()),
        }
    };
    write_holder(path);
    let _holder = HolderGuard(path);
    let result = f();
    drop(_holder); // 先清记录（还在锁里），再放锁
    drop(guard);
    result
}

/// Lock, load, mutate, save.
pub fn transaction<T>(path: &Path, f: impl FnOnce(&mut State) -> Result<T>) -> Result<T> {
    locked(path, LOCK_WAIT, || {
        let mut state = load(path)?;
        let out = f(&mut state)?;
        save(path, &mut state)?;
        Ok(out)
    })
}
