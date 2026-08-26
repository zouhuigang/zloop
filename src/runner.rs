//! Headless runner: drive `claude -p` / `codex exec` one bounded round at a time.
//!
//! The scheduler (`tick::decide`) owns every stop condition; the runner only
//! executes, checks that the host wrote back, and sleeps.

use crate::session::{Host, HostSession};
use crate::state::{self, State};
use crate::{prompt, session, tick};
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

pub struct Options {
    pub host: Host,
    pub max_rounds: u32,
    pub fast: bool,
    pub allow_all: bool,
    pub resume: bool,
}

const JOURNAL: &str = "runner/journal.jsonl";

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

/// True when the last journal line is a `begin` without a matching `end`.
fn journal_dangling(root: &Path) -> Option<Value> {
    let raw = fs::read_to_string(journal_path(root)).ok()?;
    let last: Value = serde_json::from_str(raw.lines().rev().find(|l| !l.trim().is_empty())?).ok()?;
    (last.get("event")?.as_str()? == "begin").then_some(last)
}

fn last_session(state: &State, host: Host) -> Option<String> {
    state
        .ticks
        .iter()
        .rev()
        .find(|t| t.host.as_deref() == Some(host.as_str()) && t.session.is_some())
        .and_then(|t| t.session.clone())
}

/// The child must not think it is inside *this* host session, and it must be able
/// to find a `zloop` binary. Our own directory is *appended* to PATH as a fallback:
/// prepending it would shadow the user's `claude` / `codex` when zloop lives next
/// to them (e.g. all in `~/.local/bin`).
fn isolate_child_env(cmd: &mut Command) {
    cmd.env_remove("CLAUDE_CODE_SESSION_ID").env_remove("CLAUDECODE").env_remove("CODEX_THREAD_ID");
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let old = std::env::var("PATH").unwrap_or_default();
            cmd.env("PATH", format!("{old}:{}", dir.display()));
        }
    }
}

struct HostResult {
    session: Option<String>,
    exit_ok: bool,
    summary: String,
}

fn run_claude(root: &Path, prompt: &str, resume: Option<&str>, allow_all: bool) -> Result<HostResult> {
    let mut cmd = Command::new("claude");
    cmd.current_dir(root)
        .arg("-p")
        .arg(prompt)
        .arg("--output-format")
        .arg("json");
    if let Some(sid) = resume {
        cmd.arg("--resume").arg(sid);
    }
    if allow_all {
        cmd.arg("--dangerously-skip-permissions");
    } else {
        cmd.arg("--allowedTools").arg("Bash(zloop:*),Read,Edit,Write,MultiEdit,Glob,Grep");
        cmd.arg("--permission-mode").arg("acceptEdits");
    }
    isolate_child_env(&mut cmd);
    let out = cmd.stdin(Stdio::null()).output().context("spawning `claude` (is it on PATH?)")?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let parsed: Option<Value> = serde_json::from_str(stdout.trim()).ok();
    let session = parsed.as_ref().and_then(|v| v.get("session_id")).and_then(Value::as_str).map(str::to_string);
    let is_error = parsed.as_ref().and_then(|v| v.get("is_error")).and_then(Value::as_bool).unwrap_or(!out.status.success());
    let summary = parsed
        .as_ref()
        .and_then(|v| v.get("result"))
        .and_then(Value::as_str)
        .map(|s| s.chars().take(300).collect())
        .unwrap_or_else(|| String::from_utf8_lossy(&out.stderr).chars().take(300).collect());
    Ok(HostResult { session, exit_ok: out.status.success() && !is_error, summary })
}

fn run_codex(root: &Path, prompt: &str, resume: Option<&str>, allow_all: bool) -> Result<HostResult> {
    let last_msg = root.join(state::STATE_DIR).join("runner").join("codex-last-message.txt");
    if let Some(p) = last_msg.parent() {
        fs::create_dir_all(p)?;
    }
    let mut cmd = Command::new("codex");
    cmd.current_dir(root).arg("exec");
    if let Some(sid) = resume {
        cmd.arg("resume").arg(sid);
    }
    cmd.arg("--json")
        .arg("--skip-git-repo-check")
        .arg("-C")
        .arg(root)
        .arg("--output-last-message")
        .arg(&last_msg);
    if allow_all {
        cmd.arg("--dangerously-bypass-approvals-and-sandbox");
    } else {
        cmd.arg("--sandbox").arg("workspace-write");
    }
    cmd.arg(prompt);
    isolate_child_env(&mut cmd);
    let out = cmd.stdin(Stdio::null()).output().context("spawning `codex` (is it on PATH?)")?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut session = None;
    for line in stdout.lines() {
        if let Ok(v) = serde_json::from_str::<Value>(line) {
            let kind = v.get("type").and_then(Value::as_str).unwrap_or("");
            if kind == "thread.started" {
                session = v.get("thread_id").and_then(Value::as_str).map(str::to_string);
            }
            if session.is_none() {
                if let Some(id) = v.get("thread_id").and_then(Value::as_str) {
                    session = Some(id.to_string());
                }
            }
        }
    }
    let summary = fs::read_to_string(&last_msg)
        .ok()
        .map(|s| s.chars().take(300).collect())
        .unwrap_or_else(|| String::from_utf8_lossy(&out.stderr).chars().take(300).collect());
    Ok(HostResult { session, exit_ok: out.status.success(), summary })
}

pub fn run(root: &Path, opts: Options) -> Result<i32> {
    let path = state::state_path(root);
    let host_label = match opts.host {
        Host::Claude => "claude",
        Host::Codex => "codex-cli",
        Host::Cli => "claude",
    };
    if let Some(dangling) = journal_dangling(root) {
        eprintln!(
            "runner: previous run ended mid-round (round {}); continuing from current state",
            dangling.get("round").unwrap_or(&json!(null))
        );
        journal_append(root, &json!({"event": "restart", "at": state::now_iso()}))?;
    }
    let mut rounds_done: u32 = 0;
    loop {
        let st = state::load(&path)?;
        let d = tick::decide(&st, state::now());
        if !d.should_run {
            match d.interval_min {
                None => {
                    println!("runner: stop ({})", d.reason);
                    return Ok(0);
                }
                Some(m) => {
                    println!("runner: wait ({}) · sleeping {} {}", d.reason, m, if opts.fast { "s" } else { "min" });
                    journal_sleep(root, m, opts.fast, &d.reason)?;
                    sleep_interval(m, opts.fast);
                    continue;
                }
            }
        }
        let todo = d.todo.clone().expect("ready decision carries a todo");
        let round_no = tick::current_round(&st.ticks) + 1;
        let ticks_before = st.ticks.len();
        let resume_sid = if opts.resume { last_session(&st, opts.host) } else { None };
        let mut text = prompt::heartbeat(&st, host_label, root)?;
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
        println!("runner: round {round_no} → {} [{}]{}", todo.id, opts.host.as_str(),
                 resume_sid.as_deref().map(|s| format!(" resume {s}")).unwrap_or_default());

        let result = match opts.host {
            Host::Codex => run_codex(root, &text, resume_sid.as_deref(), opts.allow_all)?,
            _ => run_claude(root, &text, resume_sid.as_deref(), opts.allow_all)?,
        };

        // Settlement: did the host write back?
        let who = HostSession { host: opts.host, session: result.session.clone() };
        let wrote_back = state::transaction(&path, |st| {
            let new_ticks: Vec<usize> = (ticks_before..st.ticks.len()).collect();
            let mut wrote = false;
            for i in new_ticks {
                let t = &mut st.ticks[i];
                if t.outcome != "noop" {
                    wrote = true;
                }
                if t.session.is_none() {
                    t.host = Some(opts.host.as_str().to_string());
                    t.session = who.session.clone();
                }
            }
            if !wrote {
                let note = if result.exit_ok {
                    "runner: host finished without writing back".to_string()
                } else {
                    format!("runner: host failed: {}", result.summary.replace('\n', " "))
                };
                tick::record(st, "fail", Some(&todo.id), &note, &who)?;
            }
            st.in_progress = None; // round settled either way
            Ok(wrote)
        })?;
        journal_append(
            root,
            &json!({"event": "end", "round": round_no, "todo": todo.id, "wrote_back": wrote_back,
                    "exit_ok": result.exit_ok, "session": result.session, "at": state::now_iso()}),
        )?;
        println!(
            "runner: round {round_no} {} · {}",
            if wrote_back { "written back" } else { "NO WRITEBACK (recorded fail)" },
            result.summary.lines().next().unwrap_or("").chars().take(120).collect::<String>()
        );
        if let Some(sid) = &result.session {
            if let Some(cmd) = session::resume_command(opts.host, sid) {
                println!("runner: session → {cmd}");
            }
        }
        rounds_done += 1;
        if opts.max_rounds > 0 && rounds_done >= opts.max_rounds {
            println!("runner: max rounds reached");
            return Ok(0);
        }
        let st = state::load(&path)?;
        let d = tick::decide(&st, state::now());
        match d.interval_min {
            None => {
                println!("runner: stop ({})", d.reason);
                return Ok(0);
            }
            Some(m) => {
                journal_sleep(root, m, opts.fast, &d.reason)?;
                sleep_interval(m, opts.fast);
            }
        }
    }
}

fn sleep_secs(minutes: u32, fast: bool) -> u64 {
    if fast { minutes as u64 } else { minutes as u64 * 60 }
}

/// Record when the runner will wake up so `zloop status` can show "sleeping until …".
fn journal_sleep(root: &Path, minutes: u32, fast: bool, reason: &str) -> Result<()> {
    let until = state::now() + chrono::Duration::seconds(sleep_secs(minutes, fast) as i64);
    journal_append(
        root,
        &json!({"event": "sleep", "until": state::format_iso(&until), "reason": reason, "at": state::now_iso()}),
    )
}

fn sleep_interval(minutes: u32, fast: bool) {
    thread::sleep(Duration::from_secs(sleep_secs(minutes, fast)));
}
