//! Headless runner: drive `claude -p` / `codex exec` one bounded round at a time.
//!
//! The scheduler (`tick::decide`) owns every stop condition; the runner only
//! executes with a timeout, checks that the host wrote back, and sleeps.
//! Long-run rules (see docs/LONG-RUN-AUDIT.md):
//!   * a hung host is killed after `timeout_min` and recorded as `fail`;
//!   * waiting on a human (user_gate / blocked) polls at the slowest interval
//!     forever instead of exiting — nothing is spent while polling;
//!   * host rate limits are not failures: sleep and retry, no tick recorded;
//!   * sessions are resumed per todo (new todo → fresh session) unless `--resume all`.

use crate::session::{Host, HostSession};
use crate::state::{self, State};
use crate::{prompt, session, tick};
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

/// Set by SIGTERM/SIGINT (`zloop stop`, Ctrl-C); the loop finishes the current step and exits cleanly.
static STOP_REQUESTED: AtomicBool = AtomicBool::new(false);

#[cfg(unix)]
fn install_signal_handlers() {
    extern "C" {
        fn signal(sig: i32, handler: usize) -> usize;
    }
    extern "C" fn on_term(_: i32) {
        STOP_REQUESTED.store(true, Ordering::SeqCst);
    }
    let h = on_term as extern "C" fn(i32) as usize;
    unsafe {
        signal(15, h); // SIGTERM
        signal(2, h); // SIGINT
    }
}
#[cfg(not(unix))]
fn install_signal_handlers() {}

fn stop_requested() -> bool {
    STOP_REQUESTED.load(Ordering::SeqCst)
}

/// Sleep in half-second slices so a stop request is noticed quickly. Returns false when interrupted.
fn sleep_interruptible(total: Duration) -> bool {
    let end = Instant::now() + total;
    while Instant::now() < end {
        if stop_requested() {
            return false;
        }
        thread::sleep(Duration::from_millis(500).min(end.saturating_duration_since(Instant::now())));
    }
    !stop_requested()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumeMode {
    /// Resume the last session that worked on the *same todo*; a new todo starts fresh.
    Todo,
    /// Always resume the host's most recent session.
    All,
    /// Never resume.
    None,
}

impl ResumeMode {
    pub fn parse(s: &str) -> Option<ResumeMode> {
        match s {
            "todo" => Some(ResumeMode::Todo),
            "all" => Some(ResumeMode::All),
            "none" => Some(ResumeMode::None),
            _ => Option::None,
        }
    }
}

pub struct Options {
    pub host: Host,
    pub max_rounds: u32,
    pub fast: bool,
    pub allow_all: bool,
    pub resume: ResumeMode,
    /// Per-round wall-clock limit for the host (minutes; seconds when `fast`).
    pub timeout_min: u32,
    /// Exit instead of polling when the scheduler is waiting on a human.
    pub exit_on_wait: bool,
    /// Passed through to `claude -p --max-budget-usd`.
    pub max_budget_usd: Option<String>,
    /// Commit the working tree (excluding `.zloop/`) after every round that wrote back.
    pub git_commit: bool,
    /// Keep the Mac awake (caffeinate + lid-close protection) while this runner lives.
    pub keep_awake: bool,
    /// 关掉「写回之后按信号插一轮重估」（默认开）。
    ///
    /// 和 `reflect_every` 的固定节奏不同，重估是**信号触发**的：账本里读不出偏离信号就完全不跑
    /// （见 `docs/ADAPTIVE-REPLAN.md` §2——每轮都重规划会制造计划抖动）。
    /// 无头模式下没人点头，所以它**只把建议记进账本，绝不自己改 todo**。
    pub no_replan: bool,
    /// 让重估轮次**真的改计划**（默认关）。
    ///
    /// 关着的时候（默认）重估只把建议记进账本，等人回来看——这是 zloop 一直以来的红线。
    /// 打开之后，重估那一轮被允许把新清单交给 `zloop replan --apply`，护栏由
    /// `replan::apply` 在代码里强制（见 `docs/ADAPTIVE-REPLAN.md` §8）。
    ///
    /// 这是唯一一处 agent 无人看管地改自己的待办，所以额外压两条闸：
    /// 单次运行最多改 `MAX_AUTO_REPLANS` 次；连着两次都把清单改长就算发散。
    /// 两者任一触顶都**停机等人**，而不是安静地接着跑。
    pub auto_replan: bool,
    /// 每 N 个 todo 轮次插一轮「回看」；0 = 关。
    ///
    /// 形状照 Warp 的 scheduled agent：**按计划跑一段不同的 prompt**，不是新子系统
    /// （见 `docs/SELF-IMPROVEMENT.md` 1.1）。回看那一轮不做 todo、不推进轮次、
    /// 对三条 streak 透明，也不动 `.zloop/NOTES.md`——无头模式下没人点头，
    /// 所以它只把建议记进账本，等人回来看。
    pub reflect_every: u32,
}

const JOURNAL: &str = "runner/journal.jsonl";

/// 单次运行最多自主改几次计划。
///
/// 文献那条"far fewer replans"说的就是这个：能改计划的循环最容易死在
/// replan → 新 todo → replan → …… 永不收敛上。三次之后还没走上正轨，
/// 多半不是计划的问题，该让人看一眼。
pub const MAX_AUTO_REPLANS: u32 = 3;
const RATE_LIMIT_MARKERS: [&str; 8] =
    ["rate limit", "rate_limit", "overloaded", "429", "capacity", "quota", "too many requests", "usage limit"];

fn journal_path(root: &Path) -> PathBuf {
    root.join(state::STATE_DIR).join(JOURNAL)
}

fn journal_append(root: &Path, entry: &Value) -> Result<()> {
    let p = journal_path(root);
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut f = OpenOptions::new().create(true).append(true).open(p)?;
    writeln!(f, "{}", entry)?;
    Ok(())
}

fn last_journal_event(root: &Path) -> Option<Value> {
    let raw = fs::read_to_string(journal_path(root)).ok()?;
    serde_json::from_str(raw.lines().rev().find(|l| !l.trim().is_empty())?).ok()
}

/// Every runner exit is journaled as `stop`; a start whose last event is not `stop` is a restart.
/// Humans are notified for every stop except the one they asked for (`--max-rounds`).
fn stop(root: &Path, reason: &str) -> Result<i32> {
    let r = crate::awake::release(std::process::id());
    if r.changed.is_some() || r.holders > 0 {
        journal_append(root, &json!({"event": "awake_off", "holders_left": r.holders, "restored_default": r.changed == Some(false), "at": state::now_iso()}))?;
    }
    journal_append(root, &json!({"event": "stop", "reason": reason, "at": state::now_iso()}))?;
    crate::daemon::clear_pid(root);
    if reason != "max_rounds" && reason != "sigterm" {
        if let Ok(st) = state::load(&state::state_path(root)) {
            let hint = match reason {
                "done" => "全部 todo 完成".to_string(),
                "fail_streak" => "连续失败，`zloop log` 看原因，`zloop edit` 后重启".to_string(),
                "progress_streak" => "同一 todo 原地踏步太久，拆小它".to_string(),
                "budget" => format!("已达花费上限 ${:.2}（policy.max_total_usd）", st.policy.max_total_usd),
                "user_gate" | "blocked" => "等你决定（--exit-on-wait 模式）".to_string(),
                other => other.to_string(),
            };
            notify(root, &st, "stop", &format!("{reason} — {hint}"));
        }
    }
    println!("runner: stop ({reason})");
    Ok(0)
}

fn notify(root: &Path, st: &State, kind: &str, detail: &str) {
    if !crate::notify::configured(st) {
        return;
    }
    let text = crate::notify::text_for(kind, st, root, detail);
    match crate::notify::send(st, root, kind, &text) {
        Ok(true) => {
            let _ = journal_append(root, &json!({"event": "notify", "kind": kind, "at": state::now_iso()}));
        }
        Ok(false) => {}
        Err(e) => eprintln!("runner: notify failed: {e}"),
    }
}

/// Run `policy.preflight_cmd`; Ok(summary) when it passes, Err(tail) when it fails.
fn preflight(root: &Path, cmd: &str, timeout: Duration) -> std::result::Result<String, String> {
    let mut c = Command::new("sh");
    c.arg("-c").arg(cmd).current_dir(root);
    isolate_child_env(&mut c, false);
    match run_with_timeout(c, timeout, "sh") {
        Ok(cap) => {
            let combined = format!("{}\n{}", cap.stdout, cap.stderr);
            let tail: String = combined.lines().rev().filter(|l| !l.trim().is_empty()).take(5).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join(" | ");
            if cap.timed_out {
                Err(format!("preflight timed out: {}", tail.chars().take(200).collect::<String>()))
            } else if cap.status.map(|s| s.success()).unwrap_or(false) {
                Ok(tail.chars().take(200).collect())
            } else {
                Err(format!("preflight failed: {}", tail.chars().take(200).collect::<String>()))
            }
        }
        Err(e) => Err(format!("preflight could not start: {e}")),
    }
}

/// Stage everything except `.zloop/` and commit, only inside a repo with real changes. Returns the short sha.
/// Everything dirty outside `.zloop/` at one instant: path → identity (size:mtime, or the
/// porcelain code once the file is gone). Comparing two of these across a round separates
/// what the host just did from work-in-progress that was already sitting in the tree.
type DirtySnapshot = std::collections::BTreeMap<String, String>;

fn git_dirty(root: &Path) -> DirtySnapshot {
    let mut snap = DirtySnapshot::new();
    // -uall lists untracked files one by one (plain porcelain collapses them to "?? dir/");
    // -z leaves paths unquoted and NUL-separated, so spaces and unicode survive intact.
    let Some(out) = Command::new("git").args(["status", "--porcelain", "-z", "-uall"]).current_dir(root).output().ok() else {
        return snap;
    };
    if !out.status.success() {
        return snap;
    }
    let mut fields = out.stdout.split(|b| *b == 0);
    while let Some(entry) = fields.next() {
        if entry.len() < 4 {
            continue;
        }
        let (code, path) = entry.split_at(3);
        let code = String::from_utf8_lossy(code).trim().to_string();
        if code.starts_with('R') || code.starts_with('C') {
            fields.next(); // a rename/copy carries its source in the next field
        }
        // A path git prints in bytes we cannot name back is left out entirely: feeding a
        // mangled pathspec to `git add` fails the *whole* checkpoint, and one unnameable
        // file is not worth losing the round's commit over.
        let Ok(path) = std::str::from_utf8(path) else { continue };
        if path == ".zloop" || path.starts_with(".zloop/") {
            continue;
        }
        snap.insert(path.to_string(), file_id(&root.join(path), &code));
    }
    snap
}

/// Size + mtime. Anything the host wrote during the round differs; anything nobody touched matches.
fn file_id(p: &Path, code: &str) -> String {
    match fs::metadata(p).and_then(|m| Ok((m.len(), m.modified()?))) {
        Ok((len, t)) => {
            let ns = t.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
            format!("{len}:{ns}")
        }
        Err(_) => format!("gone:{code}"),
    }
}

/// What a round's checkpoint took, and what it deliberately refused to take.
#[derive(Default)]
struct Checkpoint {
    sha: Option<String>,
    files: usize,
    /// Paths that were already dirty before this runner started *and* changed during the round.
    /// Someone else's edits and ours are interleaved in the same file and cannot be split, so
    /// they stay out of a commit whose message names this todo.
    held_back: Vec<String>,
}

/// One commit per round holding **only** what changed since `baseline`.
///
/// This used to be `git add -A -- .`, which swept the entire work tree in: a concurrent session's
/// half-written (or non-compiling) edits landed under "zloop tN: <our note>", and the runner
/// printed nothing but a sha. Now the baseline says what was already dirty, and the commit names
/// its paths explicitly — which also keeps anything a foreign session left *staged* out of it.
/// On success `baseline` is refreshed: whatever is still dirty after the commit is not ours.
fn git_checkpoint(root: &Path, todo_id: &str, note: &str, baseline: &mut DirtySnapshot) -> Checkpoint {
    let mut cp = Checkpoint::default();
    let git = |args: &[&str]| Command::new("git").args(args).current_dir(root).output().ok();
    if !git(&["rev-parse", "--is-inside-work-tree"]).is_some_and(|o| o.status.success()) {
        return cp;
    }
    let now = git_dirty(root);
    let mut ours: Vec<&str> = Vec::new();
    for (path, id) in &now {
        match baseline.get(path) {
            None => ours.push(path),                    // appeared while we were driving → ours
            Some(before) if before == id => {}          // foreign WIP nobody touched → leave it dirty
            Some(_) => cp.held_back.push(path.clone()), // foreign WIP the round also wrote → unsplittable
        }
    }
    if ours.is_empty() {
        return cp;
    }
    let pathspec: Vec<u8> = ours.iter().flat_map(|p| p.as_bytes().iter().copied().chain([0])).collect();
    // --pathspec-from-file has no argv limit and needs no quoting; .zloop never reaches it
    // (git exits 1 when an ignored path is named explicitly).
    if !git_pathspec(root, &["add", "--pathspec-from-file=-", "--pathspec-file-nul"], &pathspec) {
        return cp;
    }
    let msg = format!("zloop {todo_id}: {}", if note.is_empty() { "round" } else { note });
    if !git_pathspec(root, &["commit", "-q", "-m", &msg, "--pathspec-from-file=-", "--pathspec-file-nul"], &pathspec) {
        return cp;
    }
    cp.files = ours.len();
    cp.sha = git(&["rev-parse", "--short", "HEAD"])
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
    *baseline = git_dirty(root);
    cp
}

/// Runs git with a NUL-separated pathspec on stdin. The list is small enough that the write
/// never fills the pipe, so writing before reading cannot deadlock.
fn git_pathspec(root: &Path, args: &[&str], paths: &[u8]) -> bool {
    let child = Command::new("git")
        .args(args)
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn();
    let Ok(mut child) = child else { return false };
    if let Some(mut w) = child.stdin.take() {
        if w.write_all(paths).is_err() {
            return false;
        }
    }
    match child.wait_with_output() {
        Ok(o) if o.status.success() => true,
        Ok(o) => {
            let why = String::from_utf8_lossy(&o.stderr);
            if let Some(line) = why.lines().find(|l| !l.trim().is_empty()) {
                eprintln!("runner: git {} 失败：{line}", args[0]);
            }
            false
        }
        Err(_) => false,
    }
}

/// Releases the keep-awake hold on *every* way out of `run()` — clean return, `?` error, or panic.
/// `stop()` normally does it first; a second release is a no-op (unregister and reconcile are idempotent).
struct AwakeGuard {
    root: PathBuf,
    armed: bool,
}

impl Drop for AwakeGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let r = crate::awake::release(std::process::id());
        if r.changed == Some(false) {
            let _ = journal_append(
                &self.root,
                &json!({"event": "awake_off", "holders_left": r.holders, "restored_default": true, "via": "guard", "at": state::now_iso()}),
            );
        }
    }
}

fn blocked_summary(st: &State) -> String {
    st.todos
        .iter()
        .filter(|t| t.status == "blocked" && t.blocked_by.iter().any(|d| d == crate::todo::USER))
        .map(|t| format!("- {} [P{}] {}：{}", t.id, t.priority, t.text, t.note))
        .collect::<Vec<_>>()
        .join("\n")
}

fn pick_session(state: &State, host: Host, todo_id: &str, mode: ResumeMode) -> Option<String> {
    let same_host = |t: &&state::Tick| t.host.as_deref() == Some(host.as_str()) && t.session.is_some();
    match mode {
        ResumeMode::None => None,
        ResumeMode::All => state.ticks.iter().rev().find(same_host).and_then(|t| t.session.clone()),
        ResumeMode::Todo => state
            .ticks
            .iter()
            .rev()
            .filter(same_host)
            .find(|t| t.todo.as_deref() == Some(todo_id))
            .and_then(|t| t.session.clone()),
    }
}

/// The child must not think it is inside *this* host session, and it must be able
/// to find a `zloop` binary. Our own directory is *appended* to PATH as a fallback:
/// prepending it would shadow the user's `claude` / `codex` when zloop lives next
/// to them (e.g. all in `~/.local/bin`).
/// runner 允许这一轮改计划时，额外放行的环境变量（见 `isolate_child_env`）。
pub const AUTO_REPLAN_ENV: &str = "ZLOOP_AUTO_REPLAN";

fn isolate_child_env(cmd: &mut Command, may_replan: bool) {
    cmd.env_remove("CLAUDE_CODE_SESSION_ID").env_remove("CLAUDECODE").env_remove("CODEX_THREAD_ID");
    // `claude -p` loads the project's hooks, including our own Stop hook. Mark the child so
    // `zloop hook-stop` lets the host exit after exactly one todo instead of chaining them.
    cmd.env("ZLOOP_RUNNER", "1");
    // 默认情况下无头轮次**不许改计划**。这条红线以前只写在提示词里——而这整个功能的前提
    // 就是"提示词管不住模型"（回归测试里那个假宿主真的抗命跑了一次 `replan --apply`，
    // 而且成功了）。所以改成代码闸：子进程里没有这个变量，`replan --apply` 直接拒绝。
    if may_replan {
        cmd.env(AUTO_REPLAN_ENV, "1");
    } else {
        cmd.env_remove(AUTO_REPLAN_ENV);
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let old = std::env::var("PATH").unwrap_or_default();
            cmd.env("PATH", format!("{old}:{}", dir.display()));
        }
    }
}

struct Captured {
    status: Option<ExitStatus>,
    stdout: String,
    stderr: String,
    timed_out: bool,
    interrupted: bool,
}

/// Spawn, drain stdout/stderr on threads, and kill the child when the deadline passes.
fn run_with_timeout(mut cmd: Command, timeout: Duration, what: &str) -> Result<Captured> {
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().with_context(|| format!("spawning `{what}` (is it on PATH?)"))?;
    let mut out = child.stdout.take().expect("piped stdout");
    let mut err = child.stderr.take().expect("piped stderr");
    let h_out = thread::spawn(move || {
        let mut s = String::new();
        let _ = out.read_to_string(&mut s);
        s
    });
    let h_err = thread::spawn(move || {
        let mut s = String::new();
        let _ = err.read_to_string(&mut s);
        s
    });
    let deadline = Instant::now() + timeout;
    let mut timed_out = false;
    let mut interrupted = false;
    let status = loop {
        if let Some(st) = child.try_wait()? {
            break Some(st);
        }
        if stop_requested() {
            let _ = child.kill();
            let _ = child.wait();
            interrupted = true;
            break None;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            timed_out = true;
            break None;
        }
        thread::sleep(Duration::from_millis(200));
    };
    Ok(Captured {
        status,
        stdout: h_out.join().unwrap_or_default(),
        stderr: h_err.join().unwrap_or_default(),
        timed_out,
        interrupted,
    })
}

struct HostResult {
    session: Option<String>,
    exit_ok: bool,
    timed_out: bool,
    interrupted: bool,
    rate_limited: bool,
    /// 宿主这一轮说的话，**全文**（拿不到 result 就退回 stderr）。
    ///
    /// 别在这里截断：回看 / 重估那两种轮次不写回账本，宿主的输出就是它们**唯一**的产物，
    /// 截在 300 字会把建议清单砍掉大半。要摘要的地方（tick.note、控制台）自己截。
    output: String,
    cost_usd: Option<f64>,
    num_turns: Option<u64>,
    duration_ms: Option<u64>,
}

/// 落进 tick.note 的那一句：账本只存摘要，全文在 `.zloop/log/` 里。
fn ledger_note(output: &str, max: usize) -> String {
    crate::style::truncate(&output.replace('\n', " "), max)
}

fn looks_rate_limited(text: &str) -> bool {
    let lower = text.to_lowercase();
    RATE_LIMIT_MARKERS.iter().any(|m| lower.contains(m))
}

fn run_claude(root: &Path, prompt: &str, resume: Option<&str>, opts: &Options, timeout: Duration, may_replan: bool) -> Result<HostResult> {
    let mut cmd = Command::new("claude");
    cmd.current_dir(root).arg("-p").arg(prompt).arg("--output-format").arg("json");
    if let Some(sid) = resume {
        cmd.arg("--resume").arg(sid);
    }
    if let Some(b) = &opts.max_budget_usd {
        cmd.arg("--max-budget-usd").arg(b);
    }
    if opts.allow_all {
        cmd.arg("--dangerously-skip-permissions");
    } else {
        cmd.arg("--allowedTools").arg("Bash(zloop:*),Read,Edit,Write,MultiEdit,Glob,Grep");
        cmd.arg("--permission-mode").arg("acceptEdits");
    }
    isolate_child_env(&mut cmd, may_replan);
    let started = Instant::now();
    let cap = run_with_timeout(cmd, timeout, "claude")?;
    let elapsed_ms = started.elapsed().as_millis() as u64;
    let parsed: Option<Value> = serde_json::from_str(cap.stdout.trim()).ok();
    let get = |k: &str| parsed.as_ref().and_then(|v| v.get(k).cloned());
    let session = get("session_id").and_then(|v| v.as_str().map(str::to_string));
    let ok_status = cap.status.map(|s| s.success()).unwrap_or(false);
    let is_error = get("is_error").and_then(|v| v.as_bool()).unwrap_or(!ok_status);
    let result_text = get("result").and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
    let rate_limited = (is_error || !ok_status) && looks_rate_limited(&format!("{result_text}\n{}", cap.stderr));
    let output = if !result_text.is_empty() { result_text } else { cap.stderr };
    Ok(HostResult {
        session,
        exit_ok: ok_status && !is_error,
        timed_out: cap.timed_out,
        interrupted: cap.interrupted,
        rate_limited,
        output,
        cost_usd: get("total_cost_usd").and_then(|v| v.as_f64()),
        num_turns: get("num_turns").and_then(|v| v.as_u64()),
        duration_ms: get("duration_ms").and_then(|v| v.as_u64()).or(Some(elapsed_ms)),
    })
}

fn run_codex(root: &Path, prompt: &str, resume: Option<&str>, opts: &Options, timeout: Duration, may_replan: bool) -> Result<HostResult> {
    let last_msg = root.join(state::STATE_DIR).join("runner").join("codex-last-message.txt");
    if let Some(p) = last_msg.parent() {
        fs::create_dir_all(p)?;
    }
    let _ = fs::remove_file(&last_msg);
    let mut cmd = Command::new("codex");
    cmd.current_dir(root).arg("exec");
    if let Some(sid) = resume {
        cmd.arg("resume").arg(sid);
    }
    cmd.arg("--json").arg("--skip-git-repo-check").arg("-C").arg(root).arg("--output-last-message").arg(&last_msg);
    if opts.allow_all {
        cmd.arg("--dangerously-bypass-approvals-and-sandbox");
    } else {
        cmd.arg("--sandbox").arg("workspace-write");
    }
    cmd.arg(prompt);
    isolate_child_env(&mut cmd, may_replan);
    let started = Instant::now();
    let cap = run_with_timeout(cmd, timeout, "codex")?;
    let elapsed_ms = started.elapsed().as_millis() as u64;
    let mut session = None;
    for line in cap.stdout.lines() {
        if let Ok(v) = serde_json::from_str::<Value>(line) {
            if let Some(id) = v.get("thread_id").and_then(Value::as_str) {
                if session.is_none() || v.get("type").and_then(Value::as_str) == Some("thread.started") {
                    session = Some(id.to_string());
                }
            }
        }
    }
    let last = fs::read_to_string(&last_msg).unwrap_or_default();
    let ok_status = cap.status.map(|s| s.success()).unwrap_or(false);
    let rate_limited = !ok_status && looks_rate_limited(&format!("{}\n{}", cap.stderr, cap.stdout));
    let output = if !last.trim().is_empty() { last } else { cap.stderr };
    Ok(HostResult {
        session,
        exit_ok: ok_status,
        timed_out: cap.timed_out,
        interrupted: cap.interrupted,
        rate_limited,
        output,
        cost_usd: None,
        num_turns: None,
        duration_ms: Some(elapsed_ms),
    })
}

fn secs(units: u32, fast: bool) -> u64 {
    if fast { units as u64 } else { units as u64 * 60 }
}

/// Record when the runner will wake up so `zloop status` can show "sleeping until …".
fn journal_sleep(root: &Path, units: u32, fast: bool, reason: &str) -> Result<()> {
    let until = state::now() + chrono::Duration::seconds(secs(units, fast) as i64);
    journal_append(root, &json!({"event": "sleep", "until": state::format_iso(&until), "reason": reason, "at": state::now_iso()}))
}

fn slowest_interval(state: &State) -> u32 {
    state.policy.intervals_min.last().copied().unwrap_or(30)
}

/// Decide how long to sleep for a non-running decision, or `None` to stop the runner.
pub fn wait_plan(state: &State, d: &tick::Decision, opts: &Options) -> Option<(u32, String)> {
    match d.interval_min {
        Some(m) => Some((m, d.reason.clone())),
        None => {
            let human = d.reason == "user_gate" || d.reason == "blocked";
            if human && !opts.exit_on_wait {
                Some((slowest_interval(state), format!("{} (polling until a human unblocks)", d.reason)))
            } else {
                None
            }
        }
    }
}

/// 启动前体检：如果 runner 第一轮就会直接退出，返回那个 reason。
///
/// 走的是 `run` 循环里一模一样的两步（`tick::decide` → `wait_plan`），不另立一套规则：
/// 另写一份判断迟早会和调度器漂开，那时 `start` 要么拦错、要么又开始秒退。
pub fn immediate_stop_reason(state: &State, opts: &Options, at: chrono::DateTime<chrono::FixedOffset>) -> Option<String> {
    let d = tick::decide(state, at);
    if d.should_run {
        return None;
    }
    wait_plan(state, &d, opts).is_none().then_some(d.reason)
}

pub fn run(root: &Path, opts: Options) -> Result<i32> {
    let path = state::state_path(root);
    let host_label = match opts.host {
        Host::Codex => "codex-cli",
        _ => "claude",
    };
    let timeout = Duration::from_secs(secs(opts.timeout_min.max(1), opts.fast));
    install_signal_handlers();
    crate::daemon::write_pid(root, std::process::id())?;
    if let Some(last) = last_journal_event(root) {
        let kind = last.get("event").and_then(Value::as_str).unwrap_or("").to_string();
        if kind != "stop" {
            if kind == "begin" {
                eprintln!(
                    "runner: previous run ended mid-round (round {}); continuing from current state",
                    last.get("round").unwrap_or(&json!(null))
                );
            } else {
                eprintln!("runner: previous run did not stop cleanly (last event: {kind}); continuing from current state");
            }
            journal_append(root, &json!({"event": "restart", "after": kind, "at": state::now_iso()}))?;
        }
    }
    let mut awake_guard = AwakeGuard { root: root.to_path_buf(), armed: false };
    if opts.keep_awake && crate::awake::supported() {
        let acq = crate::awake::acquire(root, std::process::id());
        awake_guard.armed = true;
        journal_append(root, &json!({"event": "awake_on", "lid": acq.lid, "caffeinate_pid": acq.caffeinate_pid, "at": state::now_iso()}))?;
        match (&acq.hint, acq.lid) {
            (Some(h), _) => println!("runner: keep-awake: {h}"),
            (None, true) => println!("runner: keep-awake: lid-close sleep disabled while this runner lives (caffeinate pid {:?})", acq.caffeinate_pid),
            (None, false) => {}
        }
    }
    let _awake_guard = awake_guard;
    let mut rounds_done: u32 = 0;
    let mut last_reflect: Option<u32> = None;
    let mut replan_at: Option<u64> = None;
    // 上一次重估时「在等人回话」的那批 todo。`blocked` 是这几个信号里唯一的**锁存**：
    // 其余四个都从近期活动推出来、会自然衰减，而它一旦挂上，在无头模式下没人能来解，
    // 于是每一轮都会放炮。踩过：一次 4 小时的长跑里 5 次重估全由同一条 `t21 在等你回话`
    // 触发，占掉全程花费的两成多。所以对它按**边沿**处理——有新的 todo 开始等人才响。
    // 不是「只响一次」：那次实测里第 16 轮只给出判断、第 17 轮才产出重算窗口的证据表。
    let mut replan_blocked: Option<String> = None;
    // 自主改计划的账：改过几次、上一次改完还剩几条（用来看清单是不是在越改越长）
    let mut auto_replans: u32 = 0;
    let mut grew_in_a_row: u32 = 0;
    let mut stop_after_replan: Option<String> = None;
    let mut notified: Option<String> = None; // dedupe: one notification per distinct wait/limit situation
    // 起跑那一刻工作树里已经脏的东西 = 不是我们干的。每轮 checkpoint 只提交这条线之后的变化，
    // 别人的在制品不会被卷进「zloop tN: <我的 note>」。**只在 commit 成功后**刷新：
    // 某一轮没写回、没提交，那一轮的改动要留着给下一轮认领，不能当成外人的。
    let mut git_baseline = if opts.git_commit { git_dirty(root) } else { DirtySnapshot::new() };
    loop {
        if stop_requested() {
            return stop(root, "sigterm");
        }
        let st = state::load(&path)?;
        let d = tick::decide(&st, state::now());
        if !d.should_run {
            match wait_plan(&st, &d, &opts) {
                None => return stop(root, &d.reason),
                Some((m, reason)) => {
                    println!("runner: wait ({reason}) · sleeping {} {}", m, if opts.fast { "s" } else { "min" });
                    if (d.reason == "user_gate" || d.reason == "blocked") && notified.as_deref() != Some("wait") {
                        notify(root, &st, "wait", &blocked_summary(&st));
                        notified = Some("wait".into());
                    }
                    journal_sleep(root, m, opts.fast, &reason)?;
                    if !sleep_interruptible(Duration::from_secs(secs(m, opts.fast))) {
                        return stop(root, "sigterm");
                    }
                    continue;
                }
            }
        }
        notified = None; // a runnable round resets the dedupe window

        // 攒够 N 轮就回看一次。只在两轮 todo 之间插入，所以它不占 todo 轮次，
        // 也不会因为 `rounds_done` 没变而连着触发（`last_reflect` 记住上次是在第几轮插的）。
        if opts.reflect_every > 0 && rounds_done > 0 && rounds_done.is_multiple_of(opts.reflect_every) && last_reflect != Some(rounds_done)
        {
            last_reflect = Some(rounds_done);
            let text = format!(
                "{}\n\n---\n\n**这一轮由 zloop runner 无头驱动，没有人在旁边点头**：所以只输出建议清单，\
                 **不要**运行 `zloop reflect --apply`，也不要改任何代码或 todo。你的输出会原样记进账本，等人回来看。\n",
                crate::reflect::packet(&st, root, crate::notes::WINDOW, crate::notes::RULE_LIMIT)
            );
            journal_append(root, &json!({"event": "reflect", "after_round": rounds_done, "at": state::now_iso()}))?;
            println!("runner: 第 {rounds_done} 轮之后插一轮回看（不占轮次）");
            let result = match opts.host {
                Host::Codex => run_codex(root, &text, None, &opts, timeout, false)?,
                _ => run_claude(root, &text, None, &opts, timeout, false)?,
            };
            let who = HostSession { host: opts.host, session: result.session.clone() };
            // 回看不写回账本：这份全文就是它唯一的产物，一个字都不能少。
            let body = result.output.trim().to_string();
            let summary = body.lines().find(|l| !l.trim().is_empty()).unwrap_or("(没有输出)");
            let rel = crate::log::write_raw(root, "reflect", &format!("# 回看 · 第 {rounds_done} 轮之后\n\n{body}\n"))?;
            state::transaction(&path, |st| {
                let t = tick::record(st, tick::REFLECT, None, &crate::style::truncate(summary, 200), &who)?;
                if let Some(last) = st.ticks.last_mut() {
                    last.log = Some(rel.clone());
                    last.cost_usd = result.cost_usd;
                    last.duration_ms = result.duration_ms;
                }
                Ok(t)
            })?;
            println!("runner: 回看写进账本 · {rel}");
            continue;
        }

        let todo = d.todo.clone().expect("ready decision carries a todo");
        let round_no = tick::current_round(&st.ticks) + 1;
        // 持有者记录里写「run 第 N 轮」而不是光一个 run：被挡住的人一眼知道是哪一轮在写回。
        state::set_operation(format!("run 第 {round_no} 轮"));
        let ticks_before = st.ticks.len();
        let resume_sid = pick_session(&st, opts.host, &todo.id, opts.resume);

        // Preflight (Anthropic harness: verify the environment before touching code).
        let mut preflight_note = String::new();
        if let Some(cmd) = st.policy.preflight_cmd.clone() {
            match preflight(root, &cmd, timeout) {
                Ok(summary) => preflight_note = format!("\n环境自检（{cmd}）通过：{summary}"),
                Err(why) => {
                    println!("runner: round {round_no} {why}");
                    let who = HostSession { host: opts.host, session: None };
                    state::transaction(&path, |st| {
                        tick::record(st, "fail", Some(&todo.id), &format!("runner: {why}"), &who)?;
                        Ok(())
                    })?;
                    journal_append(root, &json!({"event": "preflight_failed", "round": round_no, "todo": todo.id, "at": state::now_iso()}))?;
                    let st = state::load(&path)?;
                    let d = tick::decide(&st, state::now());
                    match wait_plan(&st, &d, &opts) {
                        None => return stop(root, &d.reason),
                        Some((m, reason)) => {
                            journal_sleep(root, m, opts.fast, &reason)?;
                            if !sleep_interruptible(Duration::from_secs(secs(m, opts.fast))) {
                        return stop(root, "sigterm");
                    }
                            continue;
                        }
                    }
                }
            }
        }

        let mut text = prompt::heartbeat(&st, host_label, root)?;
        text.push_str(&preflight_note);
        text.push_str(&format!(
            "\n\n本轮由 zloop runner 无头驱动。当前 todo：{} [P{}] {}\n本轮结束前必须执行写回命令 `zloop done {} …`（或 --outcome progress/fail、--block）。不要询问用户，无法继续就用 --block 说明。",
            todo.id, todo.priority, todo.text, todo.id
        ));
        journal_append(
            root,
            &json!({"event": "begin", "round": round_no, "todo": todo.id, "host": opts.host.as_str(),
                    "resume": resume_sid, "at": state::now_iso()}),
        )?;
        state::transaction(&path, |st| {
            st.in_progress = Some(state::InProgress {
                todo: todo.id.clone(),
                started_at: state::now_iso(),
                round: round_no,
                via: "runner".into(),
                host: Some(opts.host.as_str().to_string()),
                session: resume_sid.clone(),
            });
            Ok(())
        })?;
        println!(
            "runner: round {round_no} → {} [{}]{}",
            todo.id,
            opts.host.as_str(),
            resume_sid.as_deref().map(|s| format!(" resume {s}")).unwrap_or_default()
        );

        let result = match opts.host {
            Host::Codex => run_codex(root, &text, resume_sid.as_deref(), &opts, timeout, false)?,
            _ => run_claude(root, &text, resume_sid.as_deref(), &opts, timeout, false)?,
        };

        // Settlement: did the host write back?
        let who = HostSession { host: opts.host, session: result.session.clone() };
        let (wrote_back, rate_limited) = state::transaction(&path, |st| {
            let mut wrote = false;
            for i in ticks_before..st.ticks.len() {
                let t = &mut st.ticks[i];
                if t.outcome != "noop" {
                    wrote = true;
                }
                if t.session.is_none() {
                    t.host = Some(opts.host.as_str().to_string());
                    t.session = who.session.clone();
                }
            }
            let rate_limited = !wrote && !result.timed_out && result.rate_limited;
            if !wrote && !rate_limited && !result.interrupted {
                let note = if result.timed_out {
                    format!("runner: host timed out after {} {}", opts.timeout_min, if opts.fast { "s" } else { "min" })
                } else if result.exit_ok {
                    "runner: host finished without writing back".to_string()
                } else {
                    format!("runner: host failed: {}", ledger_note(&result.output, 300))
                };
                tick::record(st, "fail", Some(&todo.id), &note, &who)?;
            }
            // Attach what the host reported about this round to the tick that closes it.
            if st.ticks.len() > ticks_before {
                if let Some(last) = st.ticks.last_mut() {
                    if last.cost_usd.is_none() {
                        last.cost_usd = result.cost_usd;
                    }
                    if last.num_turns.is_none() {
                        last.num_turns = result.num_turns;
                    }
                    if last.duration_ms.is_none() {
                        last.duration_ms = result.duration_ms;
                    }
                    if let Some(rel) = last.log.clone() {
                        let line = format!(
                            "- cost: {}   turns: {}   duration: {}   (runner settlement)",
                            last.cost_usd.map(|c| format!("${c:.4}")).unwrap_or_else(|| "-".into()),
                            last.num_turns.map(|n| n.to_string()).unwrap_or_else(|| "-".into()),
                            last.duration_ms.map(|d| format!("{}s", d / 1000)).unwrap_or_else(|| "-".into()),
                        );
                        let _ = crate::log::append(root, &rel, &line);
                    }
                }
            }
            st.in_progress = None; // round settled either way
            Ok((wrote, rate_limited))
        })?;
        journal_append(
            root,
            &json!({"event": "end", "round": round_no, "todo": todo.id, "wrote_back": wrote_back,
                    "exit_ok": result.exit_ok, "timed_out": result.timed_out, "rate_limited": rate_limited,
                    "interrupted": result.interrupted, "session": result.session, "at": state::now_iso()}),
        )?;
        if result.interrupted {
            println!("runner: round {round_no} interrupted by stop request");
            return stop(root, "sigterm");
        }
        if rate_limited {
            let st = state::load(&path)?;
            let m = slowest_interval(&st);
            println!("runner: round {round_no} host rate-limited · not counted · sleeping {} {} · {}", m,
                     if opts.fast { "s" } else { "min" }, result.output.lines().next().unwrap_or("").chars().take(100).collect::<String>());
            if notified.as_deref() != Some("rate_limited") {
                notify(root, &st, "rate_limited", &format!("{} {} 后重试：{}", m, if opts.fast { "秒" } else { "分钟" },
                    result.output.lines().next().unwrap_or("").chars().take(120).collect::<String>()));
                notified = Some("rate_limited".into());
            }
            journal_sleep(root, m, opts.fast, "host_rate_limited")?;
            if !sleep_interruptible(Duration::from_secs(secs(m, opts.fast))) {
                        return stop(root, "sigterm");
                    }
            continue;
        }
        println!(
            "runner: round {round_no} {} · {}",
            if wrote_back { "written back" } else if result.timed_out { "TIMED OUT (recorded fail)" } else { "NO WRITEBACK (recorded fail)" },
            result.output.lines().next().unwrap_or("").chars().take(120).collect::<String>()
        );
        if let Some(sid) = &result.session {
            if let Some(cmd) = session::resume_command(opts.host, sid) {
                println!("runner: session → {cmd}");
            }
        }
        if opts.git_commit && wrote_back {
            let st = state::load(&path)?;
            let note = st.ticks.last().map(|t| t.note.clone()).unwrap_or_default();
            let cp = git_checkpoint(root, &todo.id, &note, &mut git_baseline);
            if !cp.held_back.is_empty() {
                let shown: Vec<&str> = cp.held_back.iter().take(5).map(String::as_str).collect();
                let more = if cp.held_back.len() > 5 { format!(" 等 {} 个", cp.held_back.len()) } else { String::new() };
                println!("runner: 没提交 {}{more} · runner 起跑前它们就是改过的，别人的在制品拆不开", shown.join(" "));
                journal_append(root, &json!({"event": "commit_held_back", "round": round_no, "todo": todo.id,
                                             "paths": cp.held_back, "at": state::now_iso()}))?;
            }
            if let Some(sha) = cp.sha {
                println!("runner: git checkpoint {sha} · {} 个文件", cp.files);
                journal_append(root, &json!({"event": "commit", "round": round_no, "todo": todo.id, "sha": sha,
                                             "files": cp.files, "at": state::now_iso()}))?;
            }
        }
        // 写回之后按信号插一轮重估：只在账本读得出偏离时跑，一轮活最多跟一次，
        // 而且**只产出建议**——改 todo 要人点头，无头模式里没有人。
        if !opts.no_replan && wrote_back && replan_at != Some(round_no) {
            let st = state::load(&path)?;
            let sig = crate::replan::signals(&st);
            // 全部信号都是 blocked、而且等的还是上次那批人——不重复烧一轮模型
            let blocked_now = sig.iter().find(|s| s.kind == "blocked").map(|s| s.detail.clone());
            let latched = sig.iter().all(|s| s.kind == "blocked") && blocked_now.is_some() && blocked_now == replan_blocked;
            if !sig.is_empty() && !latched && crate::todo::remaining(&st) > 0 {
                replan_at = Some(round_no);
                replan_blocked = blocked_now;
                let why: Vec<String> = sig.iter().map(|s| s.detail.clone()).collect();
                println!("runner: 第 {round_no} 轮之后重估计划（{}）", why.join(" · "));
                let open_before = crate::todo::remaining(&st);
                let tail = if opts.auto_replan {
                    format!(
                        "\n\n---\n\n**这一轮由 zloop runner 无头驱动，`--auto-replan` 开着：你可以真的改计划。**\n\n\
                         想好之后，把**新的待办清单**（只列还没做的，一行一条 `[P0] 文本 :: 怎么验`）\
                         从 stdin 交给：\n\n\
                         \x20   `printf '%s\\n' '[P0] …' '[P1] …' | zloop replan --apply --why \"<为什么这么改>\"`\n\n\
                         做完的和等人回话的会自动留着，你不用列。护栏由代码强制，违反会整体拒绝并告诉你是哪条：\
                         清单不能空、每条都要带 `:: 验收`、`--why` 必填、规模最多放大到三倍多一点（且 ≤ 30 条）。\n\n\
                         **判断不用改就什么都别跑**——不改是完全合格的结论。这是第 {} 次自主改计划，\
                         单次运行最多 {} 次，用完会停机等人。别改代码，只改计划。\n",
                        auto_replans + 1,
                        MAX_AUTO_REPLANS
                    )
                } else {
                    "\n\n---\n\n**这一轮由 zloop runner 无头驱动，没有人在旁边点头**：只输出建议清单，\
                     **不要**运行任何会改 todo 的命令（plan / edit / done 一律不要），也不要改代码。\
                     你的输出会原样记进账本，等人回来看。\n"
                        .to_string()
                };
                let text = format!("{}{tail}", crate::replan::packet(&st));
                journal_append(root, &json!({"event": "replan", "round": round_no, "signals": why, "at": state::now_iso()}))?;
                let result = match opts.host {
                    Host::Codex => run_codex(root, &text, None, &opts, timeout, opts.auto_replan)?,
                    _ => run_claude(root, &text, None, &opts, timeout, opts.auto_replan)?,
                };
                let who = HostSession { host: opts.host, session: result.session.clone() };
                // 同上：重估也不写回账本，全文落盘。
                let body = result.output.trim().to_string();
                let summary = body.lines().find(|l| !l.trim().is_empty()).unwrap_or("(没有输出)");
                let rel = crate::log::write_raw(
                    root,
                    "replan",
                    &format!("# 重估 · 第 {round_no} 轮之后\n\n信号：{}\n\n{body}\n", why.join(" · ")),
                )?;
                state::transaction(&path, |st| {
                    let t = tick::record(st, tick::REPLAN, None, &crate::style::truncate(summary, 200), &who)?;
                    if let Some(last) = st.ticks.last_mut() {
                        last.log = Some(rel.clone());
                        last.cost_usd = result.cost_usd;
                        last.duration_ms = result.duration_ms;
                    }
                    Ok(t)
                })?;
                // 计划到底动没动，不听宿主自称，看账本。
                let after = state::load(&path)?;
                let open_after = crate::todo::remaining(&after);
                let changed = after.todos.iter().filter(|t| !crate::todo::is_terminal(&t.status)).any(|t| {
                    !st.todos.iter().any(|o| o.id == t.id)
                });
                if !changed {
                    println!("runner: 重估建议写进账本 · {rel}（没有动任何 todo）");
                } else {
                    auto_replans += 1;
                    grew_in_a_row = if open_after > open_before { grew_in_a_row + 1 } else { 0 };
                    println!(
                        "runner: 计划改了 · {open_before} 条 → {open_after} 条（第 {auto_replans}/{MAX_AUTO_REPLANS} 次自主重排）· {rel}"
                    );
                    journal_append(
                        root,
                        &json!({"event": "replan_applied", "round": round_no, "open_before": open_before,
                                "open_after": open_after, "nth": auto_replans, "at": state::now_iso()}),
                    )?;
                    // 两条闸，任一触顶就**停在人面前**，别安静地接着跑。
                    if grew_in_a_row >= 2 {
                        stop_after_replan = Some(format!(
                            "连着 {grew_in_a_row} 次重排都把清单改长了（这次 {open_before} → {open_after}）——在发散，不是在收敛"
                        ));
                    } else if auto_replans >= MAX_AUTO_REPLANS {
                        stop_after_replan =
                            Some(format!("自主改了 {auto_replans} 次计划还没走上正轨，多半不是计划的问题"));
                    }
                }
                if let Some(reason) = stop_after_replan.take() {
                    println!("runner: 停下来等人 —— {reason}");
                    journal_append(root, &json!({"event": "replan_giveup", "round": round_no, "why": reason, "at": state::now_iso()}))?;
                    stop(root, "replan_diverged")?;
                    return Ok(0);
                }
            }
        }

        rounds_done += 1;
        if opts.max_rounds > 0 && rounds_done >= opts.max_rounds {
            println!("runner: max rounds reached");
            return stop(root, "max_rounds");
        }
        let st = state::load(&path)?;
        let d = tick::decide(&st, state::now());
        match wait_plan(&st, &d, &opts) {
            None => return stop(root, &d.reason),
            Some((m, reason)) => {
                journal_sleep(root, m, opts.fast, &reason)?;
                if !sleep_interruptible(Duration::from_secs(secs(m, opts.fast))) {
                        return stop(root, "sigterm");
                    }
            }
        }
    }
}
