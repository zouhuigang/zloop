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
}

const JOURNAL: &str = "runner/journal.jsonl";
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
    isolate_child_env(&mut c);
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
fn git_checkpoint(root: &Path, todo_id: &str, note: &str) -> Option<String> {
    let git = |args: &[&str]| Command::new("git").args(args).current_dir(root).output().ok();
    if !git(&["rev-parse", "--is-inside-work-tree"])?.status.success() {
        return None;
    }
    // Anything changed outside .zloop/? (porcelain lines look like "?? path" / " M path")
    let status = git(&["status", "--porcelain"])?;
    let dirty = String::from_utf8_lossy(&status.stdout)
        .lines()
        .any(|l| l.len() > 3 && !l[3..].trim_start_matches('"').starts_with(".zloop"));
    if !dirty {
        return None;
    }
    // Never name .zloop in a pathspec: git exits 1 when an ignored path is mentioned explicitly.
    git(&["add", "-A", "--", "."])?;
    let _ = git(&["reset", "-q", "--", ".zloop"]); // unstage it if it was not ignored
    if git(&["diff", "--cached", "--quiet"])?.status.success() {
        return None; // nothing staged after all
    }
    let msg = format!("zloop {todo_id}: {}", if note.is_empty() { "round" } else { note });
    if !git(&["commit", "-q", "-m", &msg])?.status.success() {
        return None;
    }
    let sha = git(&["rev-parse", "--short", "HEAD"])?;
    Some(String::from_utf8_lossy(&sha.stdout).trim().to_string())
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
fn isolate_child_env(cmd: &mut Command) {
    cmd.env_remove("CLAUDE_CODE_SESSION_ID").env_remove("CLAUDECODE").env_remove("CODEX_THREAD_ID");
    // `claude -p` loads the project's hooks, including our own Stop hook. Mark the child so
    // `zloop hook-stop` lets the host exit after exactly one todo instead of chaining them.
    cmd.env("ZLOOP_RUNNER", "1");
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
    summary: String,
    cost_usd: Option<f64>,
    num_turns: Option<u64>,
    duration_ms: Option<u64>,
}

fn looks_rate_limited(text: &str) -> bool {
    let lower = text.to_lowercase();
    RATE_LIMIT_MARKERS.iter().any(|m| lower.contains(m))
}

fn run_claude(root: &Path, prompt: &str, resume: Option<&str>, opts: &Options, timeout: Duration) -> Result<HostResult> {
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
    isolate_child_env(&mut cmd);
    let started = Instant::now();
    let cap = run_with_timeout(cmd, timeout, "claude")?;
    let elapsed_ms = started.elapsed().as_millis() as u64;
    let parsed: Option<Value> = serde_json::from_str(cap.stdout.trim()).ok();
    let get = |k: &str| parsed.as_ref().and_then(|v| v.get(k).cloned());
    let session = get("session_id").and_then(|v| v.as_str().map(str::to_string));
    let ok_status = cap.status.map(|s| s.success()).unwrap_or(false);
    let is_error = get("is_error").and_then(|v| v.as_bool()).unwrap_or(!ok_status);
    let result_text = get("result").and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
    let summary: String = if !result_text.is_empty() { result_text.chars().take(300).collect() } else { cap.stderr.chars().take(300).collect() };
    let rate_limited = (is_error || !ok_status) && looks_rate_limited(&format!("{result_text}\n{}", cap.stderr));
    Ok(HostResult {
        session,
        exit_ok: ok_status && !is_error,
        timed_out: cap.timed_out,
        interrupted: cap.interrupted,
        rate_limited,
        summary,
        cost_usd: get("total_cost_usd").and_then(|v| v.as_f64()),
        num_turns: get("num_turns").and_then(|v| v.as_u64()),
        duration_ms: get("duration_ms").and_then(|v| v.as_u64()).or(Some(elapsed_ms)),
    })
}

fn run_codex(root: &Path, prompt: &str, resume: Option<&str>, opts: &Options, timeout: Duration) -> Result<HostResult> {
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
    isolate_child_env(&mut cmd);
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
    let summary: String = if !last.trim().is_empty() { last.chars().take(300).collect() } else { cap.stderr.chars().take(300).collect() };
    let ok_status = cap.status.map(|s| s.success()).unwrap_or(false);
    let rate_limited = !ok_status && looks_rate_limited(&format!("{}\n{}", cap.stderr, cap.stdout));
    Ok(HostResult {
        session,
        exit_ok: ok_status,
        timed_out: cap.timed_out,
        interrupted: cap.interrupted,
        rate_limited,
        summary,
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
fn wait_plan(state: &State, d: &tick::Decision, opts: &Options) -> Option<(u32, String)> {
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
    let mut notified: Option<String> = None; // dedupe: one notification per distinct wait/limit situation
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
        let todo = d.todo.clone().expect("ready decision carries a todo");
        let round_no = tick::current_round(&st.ticks) + 1;
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
            Host::Codex => run_codex(root, &text, resume_sid.as_deref(), &opts, timeout)?,
            _ => run_claude(root, &text, resume_sid.as_deref(), &opts, timeout)?,
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
                    format!("runner: host failed: {}", result.summary.replace('\n', " "))
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
                     if opts.fast { "s" } else { "min" }, result.summary.lines().next().unwrap_or("").chars().take(100).collect::<String>());
            if notified.as_deref() != Some("rate_limited") {
                notify(root, &st, "rate_limited", &format!("{} {} 后重试：{}", m, if opts.fast { "秒" } else { "分钟" },
                    result.summary.lines().next().unwrap_or("").chars().take(120).collect::<String>()));
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
            result.summary.lines().next().unwrap_or("").chars().take(120).collect::<String>()
        );
        if let Some(sid) = &result.session {
            if let Some(cmd) = session::resume_command(opts.host, sid) {
                println!("runner: session → {cmd}");
            }
        }
        if opts.git_commit && wrote_back {
            let st = state::load(&path)?;
            let note = st.ticks.last().map(|t| t.note.clone()).unwrap_or_default();
            if let Some(sha) = git_checkpoint(root, &todo.id, &note) {
                println!("runner: git checkpoint {sha}");
                journal_append(root, &json!({"event": "commit", "round": round_no, "todo": todo.id, "sha": sha, "at": state::now_iso()}))?;
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
