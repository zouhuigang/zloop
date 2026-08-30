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
    /// 的前提推翻了（见 `docs/design/ADAPTIVE-REPLAN.md` §6 缺口二的实测）。知道这件事的只有
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

/// `compact` 搬走的 tick 留下的汇总。**归档只该让账本变小，不该让账变少。**
///
/// 花费记在每条 tick 的 `cost_usd` 上，而 `policy.max_total_usd` 的语义是「这个目标一共
/// 只准花这么多」。`compact` 把老 todo 名下的 tick 连同它们的花费一起搬进 `archive/`，
/// 没有这份汇总，一次例行整理就是一次**静默提额**：预算闸复位成「最近 keep_days 天只准
/// 花这么多」，而 `status` 连花过钱这件事都不再显示（A-18）。
///
/// 花费只是**第一个**被搬走的累计量，不是唯一一个：`status` 的「跑了 N 轮」、`stats` 的
/// 轮次/返工/失败、`replan` 的返工率信号全都是从 `state.ticks` 现算的，搬走一次就一起
/// 掉下来（T29）。所以这里存的不是「几条 tick」而是**按 outcome 分的计数**：
/// 凡是「从 ticks 现算的累计量」都能从这一份汇总里补回来，不用每发现一个就加一个字段。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Archived {
    /// 累计搬走了多少条 tick（只为让人看懂这份汇总是从哪来的）。
    #[serde(default)]
    pub ticks: usize,
    /// 搬走的那些 tick 按 `outcome` 分的条数（done / progress / fail / …）。
    ///
    /// **老状态文件里没有这一项**（T29 之前的 `compact` 只记 `ticks` 和 `cost_usd`）：
    /// 那些轮次补不回来了，`stats` 会把这件事说出来，而不是把它们算成 0 轮悄悄带过。
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub outcomes: std::collections::BTreeMap<String, usize>,
    /// 搬走的那些 tick 里「完成却没留下实现思路」的条数（`stats` 的「无文档 N 轮」）。
    #[serde(default)]
    pub undocumented: usize,
    /// 累计搬走了多少条 todo。和 `ticks` 同一个作用：让 `statuses` 有个总数对照，
    /// 也是「这份汇总记没记 todo」的判据（`todos_unknown`）。
    #[serde(default)]
    pub todos: usize,
    /// 搬走的那些 todo 按 `status` 分的条数（done / deferred / …）。
    ///
    /// 和 `outcomes` 同一个理由：`status` 的百分比、`stats` 的「一次过 X/Y 条」
    /// 都是从 `state.todos` 现数的，搬走一次就一起掉（T44）。存**按状态分的原料**，
    /// 而不是「已完成 N 条」这样一个数——下一个从 todo 现算的数不用再加字段。
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub statuses: std::collections::BTreeMap<String, usize>,
    /// 搬走的那些 todo 里「一次过」的条数（`stats::first_try` 的判据，见那里）。
    /// 这一个补不出来：它要的是每条 todo 名下的轮数，而 tick 已经不在账本里了。
    #[serde(default)]
    pub first_try: usize,
    /// 搬走的那些 tick 上记的宿主耗时之和（毫秒）。
    #[serde(default)]
    pub duration_ms: u64,
    /// 搬走的那些 tick 上记的花费之和（USD）。`tick::spent_total` 会把它加回来。
    #[serde(default)]
    pub cost_usd: f64,
    /// 最后一次 compact 的时间。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Archived {
    /// 从没整理过的目标不该因为这个字段多出一段 JSON。
    pub fn is_empty(&self) -> bool {
        self.ticks == 0
            && self.outcomes.is_empty()
            && self.undocumented == 0
            && self.todos == 0
            && self.statuses.is_empty()
            && self.first_try == 0
            && self.duration_ms == 0
            && self.cost_usd == 0.0
            && self.at.is_none()
            && self.extra.is_empty()
    }

    /// 归档里某种 outcome 的条数。
    pub fn count(&self, outcome: &str) -> usize {
        self.outcomes.get(outcome).copied().unwrap_or(0)
    }

    /// 归档里「干活的轮次」（`tick::COUNTED`：done / progress / fail），和 `tick::rounds`
    /// 同一个定义——两处必须共用，否则整理过的目标会报出两个数。
    pub fn rounds(&self) -> usize {
        crate::tick::COUNTED.iter().map(|o| self.count(o)).sum()
    }

    /// 归档里的返工轮数（progress + fail）。返工率的分子和分母必须同源：
    /// 只把分母补上去，整理一次就把返工率冲淡成 0。
    pub fn rework(&self) -> usize {
        self.count("progress") + self.count("fail")
    }

    /// 老版本 compact 留下的汇总：搬走过 tick，却没记它们的 outcome。
    /// 这时上面那些数只能报 0，而这**不等于**那些轮次不存在——由调用方说明白。
    pub fn rounds_unknown(&self) -> bool {
        self.ticks > 0 && self.outcomes.is_empty()
    }

    /// 归档里某种 status 的 todo 条数。
    pub fn todo_count(&self, status: &str) -> usize {
        self.statuses.get(status).copied().unwrap_or(0)
    }

    /// 归档里**已完成**的 todo 条数（进度的分子）。
    pub fn done(&self) -> usize {
        self.todo_count("done")
    }

    /// 归档里算进进度分母的 todo 条数：延后的不算，和 `status` 里 `planned` 同一口径
    /// （`todo::is_terminal` 眼里 deferred 已了结，把它留在分母里会印出「全部完成 75%」）。
    pub fn planned(&self) -> usize {
        self.todos.saturating_sub(self.todo_count("deferred"))
    }

    /// T44 之前的 compact 只搬 todo 不记 todo：`at` 有值说明整理过（一次成功的整理至少
    /// 搬走一条 todo），而 `todos` 还是 0，就说明这份汇总里的 todo 数补不回来。
    /// 同 `rounds_unknown`：**不许把「不知道」印成「一条都没完成」**。
    pub fn todos_unknown(&self) -> bool {
        self.at.is_some() && self.todos == 0
    }
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
    /// 已被 `compact` 归档走的 tick 的汇总（见 `Archived`）。
    #[serde(default, skip_serializing_if = "Archived::is_empty")]
    pub archived: Archived,
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
            // `try_*` + `checked_sub_signed`：作者想到了「这串东西可能不是数字」，
            // 没想到「是数字但算不出来」——`--since 99999999999d` 以前直接 panic，
            // 而位数再多一点（i64 都装不下）反而落到下面那条好错误上（A-8）。
            // 越界和"根本不是时间"是同一类输入错误，这里不 return，让它掉到同一条路上。
            let span = match v.chars().last() {
                Some('m') => chrono::Duration::try_minutes(n),
                Some('h') => chrono::Duration::try_hours(n),
                _ => chrono::Duration::try_days(n),
            };
            if let Some(dt) = span.and_then(|s| now().checked_sub_signed(s)) {
                return Ok(dt);
            }
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
        .map(|rd| rd.flatten().filter(|e| e.path().extension().map(|x| x == "json").unwrap_or(false)).count())
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
        archived: Archived::default(),
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
        return Err(StateError(format!("no zloop state at {} (run `zloop init \"<goal>\"` first)", path.display())).into());
    }
    let raw = fs::read_to_string(path)?;
    let value: Value =
        serde_json::from_str(&raw).map_err(|e| StateError(format!("corrupt state file {}: {e}", path.display())))?;
    let version = value.get("version").and_then(Value::as_u64);
    if version != Some(VERSION) {
        return Err(StateError(format!("unsupported state version in {} (expected {VERSION})", path.display())).into());
    }
    let missing: Vec<&str> =
        ["goal", "policy", "todos", "ticks", "next_id"].into_iter().filter(|k| value.get(k).is_none()).collect();
    if !missing.is_empty() {
        return Err(StateError(format!("state file {} is missing keys: {}", path.display(), missing.join(", "))).into());
    }
    serde_json::from_value(value).map_err(|e| StateError(format!("invalid state file {}: {e}", path.display())).into())
}

/// Atomic write: temp file + fsync + rename.
pub fn save(path: &Path, state: &mut State) -> Result<()> {
    state.updated_at = now_iso();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_file_name(format!("{}.tmp", path.file_name().map(|s| s.to_string_lossy()).unwrap_or_default()));
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
    path.with_file_name(format!("{}.lock", path.file_name().map(|s| s.to_string_lossy()).unwrap_or_default()))
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
    lock.with_file_name(format!("{}.holder", lock.file_name().map(|s| s.to_string_lossy()).unwrap_or_default()))
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
    let file = OpenOptions::new().create(true).read(true).write(true).truncate(false).open(&lock_path)?;
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
