//! Runner behaviour with fake hosts on PATH (no real model calls).
mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::Duration;
use zloop::state;

fn zloop_bin() -> &'static str {
    env!("CARGO_BIN_EXE_zloop")
}

/// A fake `claude` on PATH. `$2` is the prompt (argv: -p <prompt> ...).
fn fake_host(script_body: &str) -> PathBuf {
    let dir = tempfile::tempdir().unwrap().keep();
    let p = dir.join("claude");
    fs::write(&p, format!("#!/bin/sh\n{script_body}\n")).unwrap();
    fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
    dir
}

fn project(lines: &[&str]) -> PathBuf {
    let d = tempfile::tempdir().unwrap().keep();
    run(&d, &["init", "runner test"], &[]);
    let joined = lines.join("\n");
    let mut args = vec!["plan"];
    for l in joined.lines() {
        args.push("--add");
        args.push(l);
    }
    run(&d, &args, &[]);
    // speed up: all intervals 1 second under --fast
    let p = state::state_path(&d);
    let mut st = state::load(&p).unwrap();
    st.policy.intervals_min = vec![1, 1, 2];
    state::save(&p, &mut st).unwrap();
    d
}

fn run(d: &Path, args: &[&str], env: &[(&str, &str)]) -> (i32, String, String) {
    let mut cmd = Command::new(zloop_bin());
    cmd.current_dir(d).args(args).env_remove("CLAUDE_CODE_SESSION_ID").env_remove("CODEX_THREAD_ID");
    for (k, v) in env {
        cmd.env(k, v);
    }
    let o = cmd.output().unwrap();
    (o.status.code().unwrap_or(-1), String::from_utf8_lossy(&o.stdout).into_owned(), String::from_utf8_lossy(&o.stderr).into_owned())
}

fn with_fake_path(fake: &Path) -> String {
    // fake host first, then the freshly built zloop (so `zloop done` inside the fake host uses
    // this build, not an older installed one), then the ambient PATH
    let exe_dir = Path::new(zloop_bin()).parent().unwrap().display().to_string();
    format!("{}:{}:{}", fake.display(), exe_dir, std::env::var("PATH").unwrap_or_default())
}

fn journal(d: &Path) -> Vec<serde_json::Value> {
    fs::read_to_string(d.join(".zloop/runner/journal.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[test]
fn hung_host_is_killed_and_recorded_as_fail() {
    let fake = fake_host("sleep 30; echo '{\"session_id\":\"slow\",\"is_error\":false,\"result\":\"late\"}'");
    let d = project(&["[P0] hang"]);
    let (code, out, _) = run(&d, &["run", "--host", "claude", "--fast", "--timeout-min", "1", "--max-rounds", "1"], &[("PATH", &with_fake_path(&fake))]);
    assert_eq!(code, 0);
    assert!(out.contains("TIMED OUT (recorded fail)"), "{out}");
    let st = state::load(&state::state_path(&d)).unwrap();
    assert_eq!(st.ticks.len(), 1);
    assert_eq!(st.ticks[0].outcome, "fail");
    assert!(st.ticks[0].note.contains("timed out"));
    assert!(st.in_progress.is_none());
    let j = journal(&d);
    let last_end = j.iter().rev().find(|e| e["event"] == "end").unwrap();
    assert_eq!(last_end["timed_out"], true);
    assert_eq!(j.last().unwrap()["event"], "stop", "every runner exit is journaled");
}

#[test]
fn rate_limit_is_not_a_failure_and_is_retried() {
    // First call: rate limited. Second call: writes back done.
    let fake = fake_host(
        r#"c="$TMPDIR_MARK/count"; n=$(cat "$c" 2>/dev/null || echo 0); echo $((n+1)) > "$c"
if [ "$n" = "0" ]; then echo '{"session_id":"rl","is_error":true,"result":"API Error: 429 rate limit reached, retry later"}'; exit 1; fi
id=$(printf "%s" "$2" | sed -n "s/.*当前 todo：\(t[0-9]*\) .*/\1/p" | head -1)
zloop done "$id" --note "after backoff" --approach "fake host round"  >/dev/null 2>&1
echo '{"session_id":"rl2","is_error":false,"result":"done"}'"#,
    );
    let d = project(&["[P0] one"]);
    let mark = tempfile::tempdir().unwrap().keep();
    let (code, out, _) = run(
        &d,
        &["run", "--host", "claude", "--fast"],
        &[("PATH", &with_fake_path(&fake)), ("TMPDIR_MARK", mark.to_str().unwrap())],
    );
    assert_eq!(code, 0);
    assert!(out.contains("host rate-limited · not counted"), "{out}");
    assert!(out.contains("runner: stop (done)"), "{out}");
    let st = state::load(&state::state_path(&d)).unwrap();
    let outcomes: Vec<&str> = st.ticks.iter().map(|t| t.outcome.as_str()).collect();
    assert_eq!(outcomes, ["done"], "no fail tick for the rate-limited round");
    let j = journal(&d);
    assert!(j.iter().any(|e| e["event"] == "sleep" && e["reason"] == "host_rate_limited"));
    assert!(j.iter().any(|e| e["event"] == "end" && e["rate_limited"] == true));
}

#[test]
fn sessions_follow_todo_lineage_by_default() {
    // Log argv, return a session id derived from the todo, and write back done.
    let fake = fake_host(
        r#"r=none; case "$*" in *--resume*) r=$(printf "%s" "$*" | tr "\n" " " | sed -n "s/.*--resume \([^ ]*\).*/\1/p");; esac
echo "resume=$r" >> "$TMPDIR_MARK/argv.log"
id=$(printf "%s" "$2" | sed -n "s/.*当前 todo：\(t[0-9]*\) .*/\1/p" | head -1)
zloop done "$id" --outcome progress --note "step" >/dev/null 2>&1
if [ -f "$TMPDIR_MARK/$id.seen" ]; then zloop done "$id" --note "finished" --approach "fake host round"  >/dev/null 2>&1; fi
touch "$TMPDIR_MARK/$id.seen"
echo "{\"session_id\":\"sess-$id\",\"is_error\":false,\"result\":\"ok\"}""#,
    );
    // default: same todo resumes, new todo starts fresh
    let d = project(&["[P0] a", "[P0] b"]);
    let mark = tempfile::tempdir().unwrap().keep();
    // --no-replan：这条测的是会话谱系，别让信号触发的重估轮次混进 argv.log
    let (code, out, _) = run(&d, &["run", "--host", "claude", "--fast", "--no-replan"], &[("PATH", &with_fake_path(&fake)), ("TMPDIR_MARK", mark.to_str().unwrap())]);
    assert_eq!(code, 0, "{out}");
    let argv = fs::read_to_string(mark.join("argv.log")).unwrap();
    let calls: Vec<&str> = argv.lines().collect();
    assert_eq!(calls.len(), 4, "{argv}");
    assert_eq!(calls, ["resume=none", "resume=sess-t1", "resume=none", "resume=sess-t2"], "{argv}");

    // --resume all: keeps one session across todos
    let d2 = project(&["[P0] a", "[P0] b"]);
    let mark2 = tempfile::tempdir().unwrap().keep();
    run(&d2, &["run", "--host", "claude", "--fast", "--resume", "all", "--no-replan"], &[("PATH", &with_fake_path(&fake)), ("TMPDIR_MARK", mark2.to_str().unwrap())]);
    let argv2 = fs::read_to_string(mark2.join("argv.log")).unwrap();
    let calls2: Vec<&str> = argv2.lines().collect();
    assert_eq!(calls2[2], "resume=sess-t1", "with --resume all, t2 continues t1's session: {argv2}");

    // --resume none: never resumes
    let d3 = project(&["[P0] a"]);
    let mark3 = tempfile::tempdir().unwrap().keep();
    run(&d3, &["run", "--host", "claude", "--fast", "--resume", "none"], &[("PATH", &with_fake_path(&fake)), ("TMPDIR_MARK", mark3.to_str().unwrap())]);
    let argv3 = fs::read_to_string(mark3.join("argv.log")).unwrap();
    assert!(argv3.lines().all(|l| l == "resume=none"), "{argv3}");
}

#[test]
fn waiting_on_a_human_polls_instead_of_exiting() {
    let fake = fake_host(
        r#"id=$(printf "%s" "$2" | sed -n "s/.*当前 todo：\(t[0-9]*\) .*/\1/p" | head -1)
zloop done "$id" --note "unblocked and done" --approach "fake host round"  >/dev/null 2>&1
echo '{"session_id":"w","is_error":false,"result":"ok"}'"#,
    );
    let d = project(&["[P0] gated"]);
    run(&d, &["done", "t1", "--block", "waiting for a decision"], &[]);
    for _ in 0..3 {
        run(&d, &["next"], &[]); // exhaust noop streak so decide() says interval=None
    }
    // --exit-on-wait: old behaviour, exits at once
    let (_, out, _) = run(&d, &["run", "--host", "claude", "--fast", "--exit-on-wait"], &[("PATH", &with_fake_path(&fake))]);
    assert!(out.contains("runner: stop (user_gate)"), "{out}");

    // default: keeps polling; unblock from "another terminal" after 2.5s and it finishes the todo
    let d2 = d.clone();
    let unblocker = thread::spawn(move || {
        thread::sleep(Duration::from_millis(2500));
        run(&d2, &["edit", "t1", "--status", "open"], &[]);
    });
    let (code, out, _) = run(&d, &["run", "--host", "claude", "--fast"], &[("PATH", &with_fake_path(&fake))]);
    unblocker.join().unwrap();
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("polling until a human unblocks"), "{out}");
    assert!(out.contains("runner: stop (done)"), "{out}");
    let st = state::load(&state::state_path(&d)).unwrap();
    assert_eq!(st.goal.status, "done");
    assert!(journal(&d).iter().any(|e| e["event"] == "sleep" && e["reason"].as_str().unwrap().starts_with("user_gate")));
}

#[test]
fn max_budget_flag_is_passed_to_claude() {
    let fake = fake_host(
        r#"echo "$@" >> "$TMPDIR_MARK/argv.log"
id=$(printf "%s" "$2" | sed -n "s/.*当前 todo：\(t[0-9]*\) .*/\1/p" | head -1)
zloop done "$id" --note ok --approach "fake host round"  >/dev/null 2>&1
echo '{"session_id":"b","is_error":false,"result":"ok"}'"#,
    );
    let d = project(&["[P0] a"]);
    let mark = tempfile::tempdir().unwrap().keep();
    run(&d, &["run", "--host", "claude", "--fast", "--max-budget-usd", "0.50"], &[("PATH", &with_fake_path(&fake)), ("TMPDIR_MARK", mark.to_str().unwrap())]);
    let argv = fs::read_to_string(mark.join("argv.log")).unwrap();
    assert!(argv.contains("--max-budget-usd 0.50"), "{argv}");
    assert!(argv.contains("--allowedTools Bash(zloop:*),Read,Edit,Write,MultiEdit,Glob,Grep"), "{argv}");
}

#[test]
fn start_runs_detached_and_stop_kills_it() {
    let fake = fake_host(
        r#"sleep 2
id=$(printf "%s" "$2" | sed -n "s/.*当前 todo：\(t[0-9]*\) .*/\1/p" | head -1)
zloop done "$id" --note "bg" --approach "fake host round"  >/dev/null 2>&1
echo '{"session_id":"bg","is_error":false,"result":"ok"}'"#,
    );
    let d = project(&["[P0] a", "[P0] b", "[P0] c", "[P0] d"]);
    let (code, out, err) = run(&d, &["start", "--fast"], &[("PATH", &with_fake_path(&fake))]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("runner started in the background (pid"), "{out}");
    let pid_file = d.join(".zloop/runner/pid");
    assert!(pid_file.exists());
    let (_, out, _) = run(&d, &["status"], &[]);
    assert!(out.contains("runner 在跑（pid ") && out.contains(".zloop/runner/console.log"), "{out}");
    // starting twice is refused
    let (code, _, err) = run(&d, &["start", "--fast"], &[("PATH", &with_fake_path(&fake))]);
    assert_eq!(code, 2);
    assert!(err.contains("already running"), "{err}");
    // wait (up to 15 s) for at least one round to land, then stop
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    while std::time::Instant::now() < deadline {
        if state::load(&state::state_path(&d)).unwrap().ticks.iter().any(|t| t.outcome == "done") { break; }
        thread::sleep(Duration::from_millis(250));
    }
    let (code, out, _) = run(&d, &["stop"], &[]);
    assert_eq!(code, 0);
    assert!(out.contains("stopped runner (pid"), "{out}");
    assert!(!pid_file.exists());
    // 停了以后「后台」这一行还在，但明说没人在跑——分不清"没人跑"和"忘了看"才是问题。
    let (_, out, _) = run(&d, &["status"], &[]);
    assert!(out.contains("没有 runner 在跑"), "{out}");
    let st = state::load(&state::state_path(&d)).unwrap();
    assert!(st.ticks.iter().any(|t| t.outcome == "done"), "background runner made progress: {:?}", st.ticks);
    assert!(fs::read_to_string(d.join(".zloop/runner/console.log")).unwrap().contains("runner: round 1"));
    let (_, out, _) = run(&d, &["stop"], &[]);
    assert!(out.contains("no runner is running"));
    // `zloop stop` is a SIGTERM: the runner must exit cleanly and journal it
    let j = journal(&d);
    let last = j.last().unwrap();
    assert_eq!((last["event"].as_str(), last["reason"].as_str()), (Some("stop"), Some("sigterm")), "{j:?}");
}

#[test]
fn runner_records_cost_and_marks_child_env() {
    let fake = fake_host(
        r#"echo "runner=$ZLOOP_RUNNER" >> "$TMPDIR_MARK/env.log"
id=$(printf "%s" "$2" | sed -n "s/.*当前 todo：\(t[0-9]*\) .*/\1/p" | head -1)
zloop done "$id" --note ok --approach "fake host round"  >/dev/null 2>&1
echo '{"session_id":"c","is_error":false,"result":"ok","total_cost_usd":0.1234,"num_turns":7,"duration_ms":4200}'"#,
    );
    let d = project(&["[P0] a"]);
    let mark = tempfile::tempdir().unwrap().keep();
    let (code, out, _) = run(&d, &["run", "--host", "claude", "--fast"], &[("PATH", &with_fake_path(&fake)), ("TMPDIR_MARK", mark.to_str().unwrap())]);
    assert_eq!(code, 0, "{out}");
    let st = state::load(&state::state_path(&d)).unwrap();
    let t = &st.ticks[0];
    assert_eq!((t.outcome.as_str(), t.cost_usd, t.num_turns, t.duration_ms), ("done", Some(0.1234), Some(7), Some(4200)));
    assert!(fs::read_to_string(mark.join("env.log")).unwrap().contains("runner=1"));
    let (_, out, _) = run(&d, &["status"], &[]);
    assert!(out.contains("$0.12"), "{out}");
    let st = zloop::state::load(&zloop::state::state_path(&d)).unwrap();
    let (logs, _) = zloop::log::entries(&d, &st, None, 5).unwrap();
    assert!(fs::read_to_string(&logs[0].0).unwrap().contains("- cost: $0.1234   turns: 7   duration: 4s   (runner settlement)"));
}

#[test]
fn wait_and_stop_trigger_notifications() {
    let fake = fake_host(r#"echo '{"session_id":"n","is_error":false,"result":"noop"}'"#);
    let d = project(&["[P0] gated"]);
    let p = state::state_path(&d);
    let mut st = state::load(&p).unwrap();
    st.policy.notify_cmd = Some(format!("cat >> {}", d.join("notify.log").display()));
    state::save(&p, &mut st).unwrap();
    run(&d, &["done", "t1", "--block", "which db?"], &[]);
    for _ in 0..3 {
        run(&d, &["next"], &[]);
    }
    // exit-on-wait: one "stop" notification
    let (_, out, _) = run(&d, &["run", "--host", "claude", "--fast", "--exit-on-wait"], &[("PATH", &with_fake_path(&fake))]);
    assert!(out.contains("runner: stop (user_gate)"), "{out}");
    let log = fs::read_to_string(d.join("notify.log")).unwrap();
    assert!(log.contains("\"event\":\"stop\"") && log.contains("user_gate"), "{log}");
    // polling mode: one "wait" notification, then the human finishes the todo and a "stop" (done) follows
    fs::remove_file(d.join("notify.log")).unwrap();
    let d2 = d.clone();
    let finisher = thread::spawn(move || {
        thread::sleep(Duration::from_millis(1500));
        run(&d2, &["edit", "t1", "--status", "done"], &[]);
    });
    let (_, out, _) = run(&d, &["run", "--host", "claude", "--fast"], &[("PATH", &with_fake_path(&fake))]);
    finisher.join().unwrap();
    assert!(out.contains("runner: stop (done)"), "{out}");
    let log = fs::read_to_string(d.join("notify.log")).unwrap();
    let waits = log.matches("\"event\":\"wait\"").count();
    assert_eq!(waits, 1, "exactly one wait notification: {log}");
    assert!(log.contains("which db?") && log.contains("\"event\":\"stop\""), "{log}");
    let j = journal(&d);
    assert!(j.iter().any(|e| e["event"] == "notify" && e["kind"] == "wait"));
}

#[test]
fn git_commit_checkpoints_each_round_excluding_zloop_dir() {
    let fake = fake_host(
        r#"id=$(printf "%s" "$2" | sed -n "s/.*当前 todo：\(t[0-9]*\) .*/\1/p" | head -1)
echo "work for $id" > "$id.txt"
zloop done "$id" --note "wrote $id.txt" --approach "fake host round"  >/dev/null 2>&1
echo '{"session_id":"g","is_error":false,"result":"ok"}'"#,
    );
    let d = project(&["[P0] a", "[P0] b"]);
    let git = |args: &[&str]| Command::new("git").args(args).current_dir(&d).output().unwrap();
    git(&["init", "-q", "-b", "main"]);
    git(&["config", "user.email", "t@example.com"]);
    git(&["config", "user.name", "t"]);
    fs::write(d.join(".gitignore"), ".zloop/\n").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "init"]);
    let (code, out, _) = run(&d, &["run", "--host", "claude", "--fast", "--git-commit"], &[("PATH", &with_fake_path(&fake))]);
    assert_eq!(code, 0, "{out}");
    assert_eq!(out.matches("runner: git checkpoint").count(), 2, "{out}");
    let log = String::from_utf8_lossy(&git(&["log", "--oneline"]).stdout).to_string();
    assert!(log.contains("zloop t1: wrote t1.txt") && log.contains("zloop t2: wrote t2.txt"), "{log}");
    let tracked = String::from_utf8_lossy(&git(&["ls-files"]).stdout).to_string();
    assert!(tracked.contains("t1.txt") && !tracked.contains(".zloop/"), "{tracked}");
    assert!(journal(&d).iter().filter(|e| e["event"] == "commit").count() == 2);
}

#[test]
fn preflight_failure_records_fail_and_success_reaches_the_host() {
    let fake = fake_host(
        r#"id=$(printf "%s" "$2" | sed -n "s/.*当前 todo：\(t[0-9]*\) .*/\1/p" | head -1)
case "$2" in *环境自检*通过*) echo yes > "$TMPDIR_MARK/preflight_seen";; esac
zloop done "$id" --note ok --approach "fake host round"  >/dev/null 2>&1
echo '{"session_id":"p","is_error":false,"result":"ok"}'"#,
    );
    // failing preflight → 3 fail ticks → stop (fail_streak), host never called
    let d = project(&["[P0] a"]);
    let p = state::state_path(&d);
    let mut st = state::load(&p).unwrap();
    st.policy.preflight_cmd = Some("echo broken env >&2; exit 1".into());
    state::save(&p, &mut st).unwrap();
    let mark = tempfile::tempdir().unwrap().keep();
    let (_, out, _) = run(&d, &["run", "--host", "claude", "--fast"], &[("PATH", &with_fake_path(&fake)), ("TMPDIR_MARK", mark.to_str().unwrap())]);
    assert!(out.contains("runner: stop (fail_streak)"), "{out}");
    let st = state::load(&p).unwrap();
    assert_eq!(st.ticks.iter().filter(|t| t.outcome == "fail" && t.note.contains("preflight failed") && t.note.contains("broken env")).count(), 3);
    assert!(!mark.join("preflight_seen").exists(), "host must not run when preflight fails");
    assert!(journal(&d).iter().any(|e| e["event"] == "preflight_failed"));
    // passing preflight → its summary reaches the host prompt
    let d2 = project(&["[P0] a"]);
    let p2 = state::state_path(&d2);
    let mut st = state::load(&p2).unwrap();
    st.policy.preflight_cmd = Some("echo env ok".into());
    state::save(&p2, &mut st).unwrap();
    let mark2 = tempfile::tempdir().unwrap().keep();
    let (_, out, _) = run(&d2, &["run", "--host", "claude", "--fast"], &[("PATH", &with_fake_path(&fake)), ("TMPDIR_MARK", mark2.to_str().unwrap())]);
    assert!(out.contains("runner: stop (done)"), "{out}");
    assert!(mark2.join("preflight_seen").exists(), "prompt should carry the preflight summary");
}

// ---------- keep-awake (macOS sleep protection) with fake sudo / pmset / caffeinate ----------

/// Fake power tools. `pmset` keeps its state in $FAKE_PM_STATE and appends every write to $FAKE_PM_LOG.
fn fake_power_tools(sudo_ok: bool) -> PathBuf {
    let dir = tempfile::tempdir().unwrap().keep();
    let write = |name: &str, body: &str| {
        let p = dir.join(name);
        fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
    };
    write(
        "pmset",
        r#"if [ "$1" = "-g" ]; then v=$(cat "$FAKE_PM_STATE" 2>/dev/null || echo 0); printf 'System-wide power settings:\n SleepDisabled\t\t%s\n sleep                1\n' "$v"; exit 0; fi
if [ "$1" = "-a" ] && [ "$2" = "disablesleep" ]; then echo "$3" > "$FAKE_PM_STATE"; echo "disablesleep $3" >> "$FAKE_PM_LOG"; exit 0; fi
exit 0"#,
    );
    if sudo_ok {
        write("sudo", r#"[ "$1" = "-n" ] && shift; exec "$@""#);
    } else {
        write("sudo", r#"echo "sudo: a password is required" >&2; exit 1"#);
    }
    write("caffeinate", r#"for last; do :; done; while kill -0 "$last" 2>/dev/null; do sleep 0.2; done"#);
    dir
}

struct AwakeEnv {
    home: PathBuf,
    state: PathBuf,
    log: PathBuf,
}

fn awake_env() -> AwakeEnv {
    let home = tempfile::tempdir().unwrap().keep();
    AwakeEnv { state: home.join("pm.state"), log: home.join("pm.log"), home }
}

fn awake_vars<'a>(e: &'a AwakeEnv, path: &'a str) -> Vec<(&'a str, &'a str)> {
    vec![
        ("PATH", path),
        ("HOME", e.home.to_str().unwrap()),
        ("FAKE_PM_STATE", e.state.to_str().unwrap()),
        ("FAKE_PM_LOG", e.log.to_str().unwrap()),
        ("ZLOOP_AWAKE_POLL_SECS", "1"),
    ]
}

fn pm_log(e: &AwakeEnv) -> String {
    fs::read_to_string(&e.log).unwrap_or_default()
}

fn pm_state(e: &AwakeEnv) -> String {
    fs::read_to_string(&e.state).unwrap_or_else(|_| "0".into()).trim().to_string()
}

#[test]
fn runner_disables_lid_sleep_while_alive_and_restores_after() {
    let fake = fake_host(
        r#"id=$(printf "%s" "$2" | sed -n "s/.*当前 todo：\(t[0-9]*\) .*/\1/p" | head -1)
zloop done "$id" --note ok --approach "fake host round"  >/dev/null 2>&1
echo '{"session_id":"a","is_error":false,"result":"ok"}'"#,
    );
    let tools = fake_power_tools(true);
    let path = format!("{}:{}", tools.display(), with_fake_path(&fake));
    let e = awake_env();
    let d = project(&["[P0] a"]);
    let (code, out, _) = run(&d, &["run", "--host", "claude", "--fast"], &awake_vars(&e, &path));
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("keep-awake: lid-close sleep disabled while this runner lives"), "{out}");
    assert_eq!(pm_log(&e).trim(), "disablesleep 1\ndisablesleep 0", "enable at start, restore at stop");
    assert_eq!(pm_state(&e), "0");
    let j = journal(&d);
    assert!(j.iter().any(|x| x["event"] == "awake_on" && x["lid"] == true));
    assert!(j.iter().any(|x| x["event"] == "awake_off" && x["restored_default"] == true));
    assert!(fs::read_dir(e.home.join(".zloop/awake")).map(|r| r.count() == 0).unwrap_or(true), "no holder left");
    // --no-keep-awake leaves pmset untouched
    let e2 = awake_env();
    let d2 = project(&["[P0] a"]);
    let (_, out, _) = run(&d2, &["run", "--host", "claude", "--fast", "--no-keep-awake"], &awake_vars(&e2, &path));
    assert!(!out.contains("keep-awake"), "{out}");
    assert_eq!(pm_log(&e2), "");
}

#[test]
fn without_passwordless_sudo_runner_degrades_to_caffeinate_with_a_hint() {
    let fake = fake_host(
        r#"id=$(printf "%s" "$2" | sed -n "s/.*当前 todo：\(t[0-9]*\) .*/\1/p" | head -1)
zloop done "$id" --note ok --approach "fake host round"  >/dev/null 2>&1
echo '{"session_id":"b","is_error":false,"result":"ok"}'"#,
    );
    let tools = fake_power_tools(false);
    let path = format!("{}:{}", tools.display(), with_fake_path(&fake));
    let e = awake_env();
    let d = project(&["[P0] a"]);
    let (code, out, _) = run(&d, &["run", "--host", "claude", "--fast"], &awake_vars(&e, &path));
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("run `zloop install --sudoers` once"), "{out}");
    assert_eq!(pm_log(&e), "", "pmset must not be called without sudo");
    assert!(journal(&d).iter().any(|x| x["event"] == "awake_on" && x["lid"] == false));
    let (_, out, _) = run(&d, &["awake"], &awake_vars(&e, &path));
    assert!(out.contains("lid-close protection unavailable"), "{out}");
}

#[test]
fn watchdog_restores_default_after_kill_9_and_holders_are_reference_counted() {
    let slow = fake_host(r#"sleep 60; echo '{"session_id":"s","is_error":false,"result":"late"}'"#);
    let tools = fake_power_tools(true);
    let path = format!("{}:{}", tools.display(), with_fake_path(&slow));
    let e = awake_env();
    let vars = awake_vars(&e, &path);
    // two projects, two background runners
    let a = project(&["[P0] a"]);
    let b = project(&["[P0] b"]);
    let (code, out, err) = run(&a, &["start", "--fast", "--timeout-min", "120"], &vars);
    assert_eq!(code, 0, "{out}{err}");
    let (code, out, err) = run(&b, &["start", "--fast", "--timeout-min", "120"], &vars);
    assert_eq!(code, 0, "{out}{err}");
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while std::time::Instant::now() < deadline && fs::read_dir(e.home.join(".zloop/awake")).map(|r| r.count()).unwrap_or(0) < 2 {
        thread::sleep(Duration::from_millis(200));
    }
    assert_eq!(pm_state(&e), "1");
    let (_, out, _) = run(&a, &["awake"], &vars);
    assert!(out.contains("disabled by zloop (2 runners)"), "{out}");
    // stop B: A still alive → stays 1
    run(&b, &["stop"], &vars);
    thread::sleep(Duration::from_millis(500));
    assert_eq!(pm_state(&e), "1", "other runner still needs it: {}", pm_log(&e));
    // kill -9 A: the watchdog (poll 1s) must restore the default
    let pid: i32 = fs::read_to_string(a.join(".zloop/runner/pid")).unwrap().trim().parse().unwrap();
    Command::new("kill").args(["-9", &pid.to_string()]).status().unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while std::time::Instant::now() < deadline && pm_state(&e) != "0" {
        thread::sleep(Duration::from_millis(250));
    }
    assert_eq!(pm_state(&e), "0", "watchdog restored default: {}", pm_log(&e));
    let (_, out, _) = run(&a, &["awake"], &vars);
    assert!(out.contains("default (lid-close protection ready"), "{out}");
    assert_eq!(fs::read_dir(e.home.join(".zloop/awake")).map(|r| r.count()).unwrap_or(0), 0);
}

#[test]
fn awake_reconcile_fixes_a_stale_setting() {
    let tools = fake_power_tools(true);
    let path = format!("{}:{}", tools.display(), std::env::var("PATH").unwrap_or_default());
    let e = awake_env();
    fs::write(&e.state, "1\n").unwrap(); // left behind by an earlier run / reboot
    let d = project(&["[P0] a"]);
    let (_, out, _) = run(&d, &["awake"], &awake_vars(&e, &path));
    assert!(out.contains("⚠ SleepDisabled=1 but no zloop runner alive"), "{out}");
    let (_, out, _) = run(&d, &["awake", "reconcile"], &awake_vars(&e, &path));
    assert!(out.contains("restored to 0"), "{out}");
    assert_eq!(pm_state(&e), "0");
    // 完整措辞在 `zloop awake` 里；status 在一切正常时不再唠叨这一行。
    let (_, out, _) = run(&d, &["awake"], &awake_vars(&e, &path));
    assert!(out.contains("default (lid-close protection ready"), "{out}");
    let (_, out, _) = run(&d, &["status"], &awake_vars(&e, &path));
    assert!(!out.contains("睡眠"), "{out}");
}

/// The user's scenario, spelled out: sleep stays disabled for as long as the runner lives
/// (closing/opening the lid changes nothing — no code path listens for lid or wake events),
/// and the default comes back **by itself** when the task finishes. No `zloop stop` needed.
#[test]
fn sleep_stays_disabled_until_the_task_finishes_by_itself() {
    let fake = fake_host(
        r#"sleep 1
id=$(printf "%s" "$2" | sed -n "s/.*当前 todo：\(t[0-9]*\) .*/\1/p" | head -1)
zloop done "$id" --note "round for $id" --approach "fake host round"  >/dev/null 2>&1
echo '{"session_id":"lid","is_error":false,"result":"ok"}'"#,
    );
    let tools = fake_power_tools(true);
    let path = format!("{}:{}", tools.display(), with_fake_path(&fake));
    let e = awake_env();
    let vars = awake_vars(&e, &path);
    let d = project(&["[P0] a", "[P0] b", "[P0] c"]);

    let (code, out, err) = run(&d, &["start", "--fast", "--timeout-min", "120"], &vars);
    assert_eq!(code, 0, "{out}{err}");

    // Wait until the hold is in place, then watch it for several seconds while rounds run.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while std::time::Instant::now() < deadline && pm_state(&e) != "1" {
        thread::sleep(Duration::from_millis(200));
    }
    assert_eq!(pm_state(&e), "1", "hold taken: {}", pm_log(&e));

    // "Lid closed for a while, then opened" = nobody touches zloop. Sleep must stay disabled
    // and the runner must stay alive across round boundaries.
    let pid: i32 = fs::read_to_string(d.join(".zloop/runner/pid")).unwrap().trim().parse().unwrap();
    let mut samples = 0;
    for _ in 0..8 {
        thread::sleep(Duration::from_millis(400));
        assert!(zloop::daemon::pid_alive(pid), "runner still alive across rounds");
        if pm_state(&e) == "1" {
            samples += 1;
        } else {
            break; // finished early; the assertions after the loop cover it
        }
    }
    assert!(samples >= 3, "sleep stayed disabled while the runner worked (samples={samples}, log={})", pm_log(&e));

    // Now let it finish on its own — no `zloop stop`, no lid event, nothing.
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    while std::time::Instant::now() < deadline && zloop::daemon::running(&d).is_some() {
        thread::sleep(Duration::from_millis(250));
    }
    assert!(zloop::daemon::running(&d).is_none(), "runner exited by itself");
    assert_eq!(pm_state(&e), "0", "default restored without any user action: {}", pm_log(&e));
    assert_eq!(pm_log(&e).trim(), "disablesleep 1\ndisablesleep 0", "exactly one enable and one restore");
    assert_eq!(fs::read_dir(e.home.join(".zloop/awake")).map(|r| r.count()).unwrap_or(0), 0);

    let st = state::load(&state::state_path(&d)).unwrap();
    assert_eq!(st.goal.status, "done");
    assert_eq!(st.ticks.iter().filter(|t| t.outcome == "done").count(), 3);
    let j = journal(&d);
    let last = j.last().unwrap();
    assert_eq!((last["event"].as_str(), last["reason"].as_str()), (Some("stop"), Some("done")));
    assert!(j.iter().any(|x| x["event"] == "awake_off" && x["restored_default"] == true));
    let (_, out, _) = run(&d, &["awake"], &vars);
    assert!(out.contains("default (lid-close protection ready"), "{out}");
}

/// `--reflect-every N`：每 N 个 todo 轮次插一轮回看——不做 todo、不推进轮次、
/// 不动 NOTES.md（无头模式没人点头），只把建议记进账本。
#[test]
fn reflect_every_inserts_a_round_that_does_not_consume_a_todo() {
    let d = project(&["[P0] a", "[P0] b", "[P0] c"]);
    // 回看那一轮的 prompt 里有「回看一次」；todo 轮次的 prompt 里有「本轮由 zloop runner」。
    // 假宿主据此分辨自己被叫来干嘛：回看只回话，todo 轮次才写回。
    let fake = fake_host(
        r#"case "$2" in
  *"回看一次"*) echo "$2" > "$TMPDIR_MARK/reflect-prompt"
     echo '{"session_id":"r","is_error":false,"result":"建议：第 1、2 条合并"}' ;;
  *) id=$(zloop next --json | sed -n 's/.*"id": "\([^"]*\)".*/\1/p' | head -1)
     zloop done "$id" --note "done by fake" --approach "fake host round" >/dev/null 2>&1
     echo '{"session_id":"s","is_error":false,"result":"ok"}' ;;
esac"#,
    );
    let mark = tempfile::tempdir().unwrap().keep();
    let (code, out, _) = run(
        &d,
        &["run", "--host", "claude", "--fast", "--reflect-every", "2", "--max-rounds", "3"],
        &[("PATH", &with_fake_path(&fake)), ("TMPDIR_MARK", mark.to_str().unwrap())],
    );
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("插一轮回看"), "{out}");

    let st = state::load(&state::state_path(&d)).unwrap();
    let reflects: Vec<_> = st.ticks.iter().filter(|t| t.outcome == "reflect").collect();
    assert_eq!(reflects.len(), 1, "3 轮里应该正好插一次: {:?}", st.ticks.iter().map(|t| &t.outcome).collect::<Vec<_>>());
    let r = reflects[0];
    assert!(r.todo.is_none(), "回看不挂在任何 todo 上");
    assert!(r.note.contains("建议"), "宿主的输出要记进账本: {}", r.note);
    assert!(r.log.as_ref().is_some_and(|l| l.contains("reflect")), "完整输出留在日志里: {r:?}");

    // 不占 todo 轮次：三条 todo 该做完的照样做完
    assert_eq!(st.todos.iter().filter(|t| t.status == "done").count(), 3, "{:?}", st.todos);
    // 回看那一轮不推进轮次编号
    assert_eq!(zloop::tick::current_round(&st.ticks), 3);
    // 它也没动经验文件
    assert!(!d.join(".zloop/NOTES.md").exists(), "无头回看不该自己落地");
    // 材料包确实是回看用的那一份
    let prompt = fs::read_to_string(mark.join("reflect-prompt")).unwrap();
    assert!(prompt.contains("现有经验") && prompt.contains("不要**运行 `zloop reflect --apply`"), "{prompt}");
}

/// 无头模式下按信号插一轮重估：**只产出建议，绝不自己改 todo**。
/// 计划是人和 agent 共同定稿的东西——没人点头的时候，runner 最多只能提议。
#[test]
fn a_headless_replan_round_suggests_but_never_edits_the_plan() {
    let d = project(&["[P0] 会拖的一条", "[P0] 另一条", "[P1] 第三条"]);
    let mark = tempfile::tempdir().unwrap().keep();
    // 重估轮次的 prompt 里有「重估一次」；干活轮次里有「本轮由 zloop runner」。
    // 假宿主扮演一个不守规矩的模型：重估时**试图**改计划，用来验证「就算它改了也不算数」这条防线
    // ——真正的防线是 runner 自己不改，且提示词明说不要改。
    let fake = fake_host(
        r#"case "$2" in
  *"重估一次"*)
     echo "$2" > "$TMPDIR_MARK/replan-prompt"
     echo '{"session_id":"rp","is_error":false,"result":"建议：t1 拆成两条，先量再改"}' ;;
  *) id=$(printf "%s" "$2" | sed -n "s/.*当前 todo：\(t[0-9]*\) .*/\1/p" | head -1)
     zloop done "$id" --outcome progress --note "又没做完" >/dev/null 2>&1
     echo '{"session_id":"s","is_error":false,"result":"ok"}' ;;
esac"#,
    );
    let (code, out, _) = run(
        &d,
        &["run", "--host", "claude", "--fast", "--max-rounds", "3"],
        &[("PATH", &with_fake_path(&fake)), ("TMPDIR_MARK", mark.to_str().unwrap())],
    );
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("重估计划"), "{out}");
    assert!(out.contains("没有动任何 todo"), "{out}");

    let st = state::load(&state::state_path(&d)).unwrap();
    // 计划一个字没变：还是三条，文本和优先级都没动
    assert_eq!(st.todos.len(), 3, "重估不能加减 todo: {:?}", st.todos);
    assert_eq!(st.todos[0].text, "会拖的一条");
    assert!(st.todos.iter().all(|t| t.status != "deferred"), "也不能擅自延后: {:?}", st.todos);

    // 建议进了账本
    let replans: Vec<_> = st.ticks.iter().filter(|t| t.outcome == "replan").collect();
    assert!(!replans.is_empty(), "至少重估过一次: {:?}", st.ticks.iter().map(|t| &t.outcome).collect::<Vec<_>>());
    assert!(replans[0].todo.is_none(), "重估不挂在任何 todo 上");
    assert!(replans[0].note.contains("建议"), "宿主的建议要记下来: {}", replans[0].note);
    assert!(replans[0].log.as_ref().is_some_and(|l| l.contains("replan")), "{:?}", replans[0]);

    // 一轮活最多跟一次重估
    let work = st.ticks.iter().filter(|t| t.outcome == "progress").count();
    assert!(replans.len() <= work, "重估不能比干活的轮次还多: {} vs {work}", replans.len());

    // 提示词里明确禁止改计划
    let prompt = fs::read_to_string(mark.join("replan-prompt")).unwrap();
    assert!(prompt.contains("触发的信号") && prompt.contains("最小改动"), "{prompt}");
    assert!(prompt.contains("不要**运行任何会改 todo 的命令"), "无头必须明说不许改: {prompt}");

    // --no-replan 能关掉
    let d2 = project(&["[P0] 会拖的一条", "[P0] 另一条"]);
    let mark2 = tempfile::tempdir().unwrap().keep();
    run(
        &d2,
        &["run", "--host", "claude", "--fast", "--max-rounds", "3", "--no-replan"],
        &[("PATH", &with_fake_path(&fake)), ("TMPDIR_MARK", mark2.to_str().unwrap())],
    );
    let st2 = state::load(&state::state_path(&d2)).unwrap();
    assert!(st2.ticks.iter().all(|t| t.outcome != "replan"), "--no-replan 应该完全不跑");
}
