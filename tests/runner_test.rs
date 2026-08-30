//! Runner behaviour with fake hosts on PATH (no real model calls).
mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
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
    cmd.current_dir(d).args(args);
    common::scrub_ambient_env(&mut cmd);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let o = cmd.output().unwrap();
    (o.status.code().unwrap_or(-1), String::from_utf8_lossy(&o.stdout).into_owned(), String::from_utf8_lossy(&o.stderr).into_owned())
}

/// 和 `run` 一样，但带上限：到点还没退出就 SIGKILL 再 panic（带上它最后说了什么）。
///
/// 「本该退出的 runner 不退出」这一类回归（A-5）用 `run` 是测不出来的：撤掉修复之后
/// 测试**挂住**而不是变红，而挂住的测试没人当成失败——它只是让 `cargo test` 永远跑不完。
fn run_within(d: &Path, args: &[&str], env: &[(&str, &str)], limit: Duration) -> (i32, String, String) {
    let mut cmd = Command::new(zloop_bin());
    cmd.current_dir(d).args(args).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
    common::scrub_ambient_env(&mut cmd);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let mut ch = cmd.spawn().unwrap();
    let deadline = std::time::Instant::now() + limit;
    let mut overran = false;
    while ch.try_wait().unwrap().is_none() {
        if std::time::Instant::now() >= deadline {
            let _ = Command::new("kill").args(["-KILL", &ch.id().to_string()]).status();
            let _ = ch.wait();
            overran = true;
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    let o = ch.wait_with_output().unwrap();
    let (out, err) = (String::from_utf8_lossy(&o.stdout).into_owned(), String::from_utf8_lossy(&o.stderr).into_owned());
    assert!(!overran, "`zloop {}` 过了 {limit:?} 还没自己退出\n--- stdout ---\n{out}\n--- stderr ---\n{err}", args.join(" "));
    (o.status.code().unwrap_or(-1), out, err)
}

/// `hook-stop` 读 stdin（内容不用，但会读），测试里必须把它接成空管道，
/// 否则在终端下跑 `cargo test` 会卡在等输入。
fn hook_stop(d: &Path, env: &[(&str, &str)]) -> (i32, String) {
    let mut cmd = Command::new(zloop_bin());
    cmd.current_dir(d).arg("hook-stop").stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped());
    common::scrub_ambient_env(&mut cmd);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let mut ch = cmd.spawn().unwrap();
    drop(ch.stdin.take());
    let o = ch.wait_with_output().unwrap();
    (o.status.code().unwrap_or(-1), String::from_utf8_lossy(&o.stdout).into_owned())
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
    // --exit-on-wait: old behaviour, exits at once
    let (_, out, _) =
        run_within(&d, &["run", "--host", "claude", "--fast", "--exit-on-wait"], &[("PATH", &with_fake_path(&fake))], Duration::from_secs(20));
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

/// A-5：`--exit-on-wait` 走真实路径必须生效。
///
/// 原来它挂在 `decide()` 返回 `interval=None` 上，而那要 `noop_streak >= max_noop_streak`；
/// runner 在 `!should_run` 那一支只写 journal 的 sleep，**一条 noop tick 都不记**，
/// 所以真 runner 自己永远到不了那个状态——这个标志在等人路径上是死代码。上面那条测试
/// 原本靠先敲 3 次 `zloop next` 把 streak 顶满，把死代码测成了绿的（实景抓到过一个带着
/// `--exit-on-wait` 的 runner 在 user_gate 上转了 20 小时 24 分、写了 1849 条 journal）。
///
/// 这一条一次手工搓状态都没有：init/plan 之后，是**宿主自己**在第一轮 `--block` 把 todo
/// 交回给人，runner 下一轮撞上 user_gate。
#[test]
fn exit_on_wait_stops_the_first_time_the_runner_itself_hits_a_human_gate() {
    let fake = fake_host(
        r#"id=$(printf "%s" "$2" | sed -n "s/.*当前 todo：\(t[0-9]*\) .*/\1/p" | head -1)
zloop done "$id" --note "要人拍板" --approach "fake host round" --block "用哪个库？" >/dev/null 2>&1
echo '{"session_id":"a5","is_error":false,"result":"blocked"}'"#,
    );
    let d = project(&["[P0] gated"]);
    let (code, out, _) = run_within(
        &d,
        &["run", "--host", "claude", "--fast", "--exit-on-wait", "--no-replan"],
        &[("PATH", &with_fake_path(&fake))],
        Duration::from_secs(25),
    );
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("runner: round 1"), "第一轮得真的跑过，等人这件事才是 runner 自己走到的：{out}");
    assert!(out.contains("runner: stop (user_gate)"), "带着 --exit-on-wait 撞上 user_gate 就该退出：{out}");
    assert!(!out.contains("polling until a human unblocks"), "退出模式下一次都不该睡：{out}");
    let st = state::load(&state::state_path(&d)).unwrap();
    // 这条断言是上面那段历史的钉子：等人那一支一条 noop 都不记，所以 `--exit-on-wait`
    // 不能挂在 noop_streak 上——挂上去就等于关掉。
    assert!(!st.ticks.iter().any(|t| t.outcome == "noop"), "runner 在等人时不记 noop tick：{:?}", st.ticks);
    assert!(!journal(&d).iter().any(|e| e["event"] == "sleep"), "退出模式下不该有 sleep：{:?}", journal(&d));
}

/// A-16：`max_noop_streak` 是交互式 `zloop next` 的退避提示，**不是 runner 的停机开关**。
///
/// 上一条测试钉住了「runner 自己一条 noop tick 都不记」，但账本是共用的——`zloop next`
/// 记下的 noop，runner 的 `decide` 照样读得到。`decide` 在 `noop_streak >= max_noop_streak`
/// 时会把 `throttled` 那一支的 `interval_min` 翻成 `None`，而 `wait_plan` 原来把 `None`
/// 一律当「停」。于是人在终端里敲三下 `zloop next`（就想看一眼现在什么情况），就把
/// 「睡到配额窗口放开再接着跑」变成了「runner 拒绝启动」。配额窗口自己会滑过去，
/// `decide` 连还差几分钟都算出来了，这一支没有任何理由退出。
#[test]
fn interactive_next_pokes_cannot_kill_a_throttled_runner() {
    let fake = fake_host(
        r#"id=$(printf "%s" "$2" | sed -n "s/.*当前 todo：\(t[0-9]*\) .*/\1/p" | head -1)
zloop done "$id" --note ok --approach "fake host round" >/dev/null 2>&1
echo '{"session_id":"a16","is_error":false,"result":"ok"}'"#,
    );
    let d = project(&["[P0] a", "[P1] b"]);
    // 跑一轮就撞满配额窗口；t2 还开着，所以判断确实会走到 throttled 那一支（而不是 all_done）
    let p = state::state_path(&d);
    let mut st = state::load(&p).unwrap();
    st.policy.max_runs = 1;
    state::save(&p, &mut st).unwrap();

    let tools = fake_power_tools(true);
    let path = format!("{}:{}", tools.display(), with_fake_path(&fake));
    let e = awake_env();
    let vars = awake_vars(&e, &path);

    let (code, out, err) = run(&d, &["run", "--host", "claude", "--fast", "--no-replan", "--max-rounds", "1"], &vars);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("runner: round 1 written back"), "{out}");

    // 人在终端里敲三下 `zloop next`
    for _ in 0..3 {
        let (_, out, _) = run(&d, &["next"], &[]);
        assert!(out.contains("WAIT (throttled)"), "{out}");
    }
    let st = state::load(&p).unwrap();
    assert_eq!(st.ticks.iter().filter(|t| t.outcome == "noop").count(), 3, "{:?}", st.ticks);
    // 前提：账本确实把 throttled 那一支的 interval_min 翻成了 null。不先钉住这一条，
    // 下面的断言在 `max_noop_streak` 改了默认值之后会变成在测空气。
    let (_, out, _) = run(&d, &["next", "--peek", "--json"], &[]);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!((v["reason"].as_str(), v["interval_min"].is_null()), (Some("throttled"), true), "{out}");

    // runner 照常起来，睡到窗口放开为止
    let (code, out, err) = run(&d, &["start", "--fast", "--timeout-min", "120"], &vars);
    assert_eq!(code, 0, "配额窗口会自己滑过去，start 不该拒绝：{out}{err}");
    assert!(out.contains("runner started in the background (pid"), "{out}");
    let mut slept = false;
    for _ in 0..60 {
        if journal(&d).iter().any(|e| e["event"] == "sleep" && e["reason"].as_str().unwrap_or("").starts_with("throttled")) {
            slept = true;
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    run(&d, &["stop"], &vars);
    let j = journal(&d);
    assert!(slept, "起来之后该睡在 throttled 上：{j:?}");
    assert!(!j.iter().any(|e| e["event"] == "stop" && e["reason"] == "throttled"), "配额满不是终态，不该 stop：{j:?}");
}

/// A-17 回归：人在另一个终端敲一句 `zloop feedback`，不能让一轮**失败**的宿主
/// 被记成「写回了」。
///
/// 结算那一步以前问的是「这段时间里账本长没长」（`t.outcome != "noop"`），而
/// `feedback` / `edit` / `replan` 都会让它长——于是失败轮次不记 fail、`fail_streak`
/// 恒为 0、连续失败停机整个失效，`--git-commit` 还会把那一轮的半成品当成果提交掉。
///
/// 这条测试里的「人」就是宿主自己在退出前敲的那句 feedback：时序确定（反馈落在
/// runner 记 fail 之前），不靠另一个线程去抢。
#[test]
fn a_humans_feedback_cannot_mask_a_failed_round() {
    let fake = fake_host(
        r#"id=$(printf "%s" "$2" | sed -n "s/.*当前 todo：\(t[0-9]*\) .*/\1/p" | head -1)
echo "half-finished work for $id" > "$id.txt"
# 人在另一个终端补一句（不是写回：不改 todo 状态、不结算这一轮）
zloop feedback "$id" "先别动 x.rs" >/dev/null 2>&1
echo "host blew up" >&2
exit 1"#,
    );
    let d = project(&["[P0] a"]);
    let p = state::state_path(&d);
    let mut st = state::load(&p).unwrap();
    st.policy.max_fail_streak = 2;
    state::save(&p, &mut st).unwrap();
    let git = git_repo(&d);

    let (code, out, err) = run_within(
        &d,
        &["run", "--host", "claude", "--fast", "--no-replan", "--git-commit", "--timeout-min", "30"],
        &[("PATH", &with_fake_path(&fake))],
        Duration::from_secs(60),
    );
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("runner: stop (fail_streak)"), "人插一句话不该拆掉连续失败这道闸：{out}");

    let st = state::load(&p).unwrap();
    assert_eq!(st.ticks.iter().filter(|t| t.outcome == "fail").count(), 2, "失败的轮次要记进账本：{:?}", st.ticks);
    assert_eq!(st.ticks.iter().filter(|t| t.outcome == "feedback").count(), 2, "{:?}", st.ticks);
    assert_eq!(zloop::tick::fail_streak(&st), 2, "{:?}", st.ticks);

    let j = journal(&d);
    assert!(j.iter().filter(|e| e["event"] == "end").all(|e| e["wrote_back"] == false), "{j:?}");
    // --git-commit：失败的轮次没有成果，checkpoint 一个都不该有
    assert!(!j.iter().any(|e| e["event"] == "commit"), "失败轮次不该产生 checkpoint：{j:?}");
    assert!(!out.contains("runner: git checkpoint"), "{out}");
    let log = String::from_utf8_lossy(&git(&["log", "--oneline"]).stdout).to_string();
    assert_eq!(log.lines().count(), 1, "树上只该有那条 init：{log}");
    // 半成品留在工作树里等下一轮认领，没被提交也没被删
    assert!(d.join("t1.txt").exists(), "没写回不等于要把产物扔掉");
}

/// A-17 的第二个后果：结算时把这一轮的花费/轮数/日志挂到 `ticks.last_mut()` 上。
/// 人在宿主写回之后补的那句 `zloop feedback` 排在后面，于是花费记到了**人**那条 tick 上。
#[test]
fn the_rounds_cost_lands_on_the_write_back_not_on_a_humans_note() {
    let fake = fake_host(
        r#"id=$(printf "%s" "$2" | sed -n "s/.*当前 todo：\(t[0-9]*\) .*/\1/p" | head -1)
zloop done "$id" --note ok --approach "fake host round" >/dev/null 2>&1
# 人在写回之后才开口——这条 tick 排在写回后面，但它不是这一轮的结算
zloop feedback "$id" "顺便说一句：下次先跑 lint" >/dev/null 2>&1
echo '{"session_id":"c2","is_error":false,"result":"ok","total_cost_usd":0.4321,"num_turns":9,"duration_ms":8100}'"#,
    );
    let d = project(&["[P0] a"]);
    let (code, out, err) = run(&d, &["run", "--host", "claude", "--fast", "--no-replan"], &[("PATH", &with_fake_path(&fake))]);
    assert_eq!(code, 0, "{out}{err}");
    let st = state::load(&state::state_path(&d)).unwrap();
    let kinds: Vec<&str> = st.ticks.iter().map(|t| t.outcome.as_str()).collect();
    assert_eq!(kinds, vec!["done", "feedback"], "{:?}", st.ticks);
    assert_eq!(
        (st.ticks[0].cost_usd, st.ticks[0].num_turns, st.ticks[0].duration_ms),
        (Some(0.4321), Some(9), Some(8100)),
        "花费该记在结掉这一轮的那条上"
    );
    assert_eq!(
        (st.ticks[1].cost_usd, st.ticks[1].num_turns, st.ticks[1].duration_ms),
        (None, None, None),
        "人说的那句话不吃这一轮的账"
    );
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

/// `start` 先按子进程 pid 写一次 pid 文件，runner 起来后又用自己的 pid 覆写同一个文件——
/// 两次写的是同一个数，但覆写不是原子的：`status` 正好读在截断之后、写入之前，就读到空文件，
/// 解析失败当成「没有 runner 在跑」。这是 start 之后立刻 status 偶发看不到 runner 的原因。
#[test]
fn pid_file_is_never_seen_empty_while_being_rewritten() {
    let d = tempfile::tempdir().unwrap().keep();
    let me = std::process::id();
    zloop::daemon::write_pid(&d, me).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let writer = {
        let (d, stop) = (d.clone(), stop.clone());
        thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                zloop::daemon::write_pid(&d, me).unwrap();
            }
        })
    };
    // 自己这个进程一直活着，所以每一次探测都必须看得见它
    let mut misses = 0;
    for _ in 0..20_000 {
        if zloop::daemon::running(&d).is_none() {
            misses += 1;
        }
    }
    stop.store(true, Ordering::Relaxed);
    writer.join().unwrap();
    assert_eq!(misses, 0, "20000 次探测里有 {misses} 次报「没有 runner」，可 pid {me} 一直活着");
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

/// 0 待办时 `start` 曾经照常报告「runner started in the background」，然后 runner 第一次
/// `decide` 就 stop(all_done) 秒退——看着像起来了，控制台里只有一句 reason。现在 `start` 走
/// 一遍和 runner 第一轮一样的判断（decide + wait_plan），会秒退就当场拒绝并说下一步。(#6)
#[test]
fn start_refuses_to_launch_a_runner_that_would_exit_immediately() {
    let d = tempfile::tempdir().unwrap().keep();
    run(&d, &["init", "start precheck"], &[]);

    // 一条待办都没有：拒绝、退出码 1、不留 pid 和控制台日志，也不许说 started
    let (code, out, err) = run(&d, &["start", "--fast"], &[]);
    assert_eq!(code, 1, "{out}{err}");
    assert!(err.contains("没启动") && err.contains("一条待办都没有") && err.contains("zloop plan"), "{err}");
    assert!(!out.contains("started"), "拒绝了就不能再报告启动成功: {out}");
    assert!(!d.join(".zloop/runner/pid").exists(), "没有 runner 被拉起来");
    assert!(!d.join(".zloop/runner/console.log").exists());

    // 有待办：行为不变，照常起来（假 host 只睡觉，起来就够了）
    let slow = fake_host(r#"sleep 60; echo '{"session_id":"s","is_error":false,"result":"late"}'"#);
    let tools = fake_power_tools(true);
    let path = format!("{}:{}", tools.display(), with_fake_path(&slow));
    let e = awake_env();
    let vars = awake_vars(&e, &path);
    run(&d, &["plan", "--add", "[P0] a"], &[]);
    let (code, out, err) = run(&d, &["start", "--fast", "--timeout-min", "120"], &vars);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("runner started in the background (pid"), "{out}");
    run(&d, &["stop"], &vars);

    // 别拦过头：唯一的待办被人挡着，但 runner 是挂着轮询等人（不是秒退），这种照常起
    run(&d, &["edit", "t1", "--blocked-by", "user"], &[]);
    let (code, out, err) = run(&d, &["start", "--fast", "--timeout-min", "120"], &vars);
    assert_eq!(code, 0, "等人是轮询不是秒退，这种就该照常起来: {out}{err}");
    assert!(out.contains("runner started in the background (pid"), "{out}");
    run(&d, &["stop"], &vars);

    // 全做完之后：也拦，但说的是「目标结束了」，不是「去 plan」
    run(&d, &["edit", "t1", "--blocked-by", ""], &[]);
    run(&d, &["done", "t1", "--note", "ok", "--approach", "fake"], &[]);
    let (code, _, err) = run(&d, &["start", "--fast"], &vars);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("目标已经结束") && err.contains("zloop goal new"), "{err}");
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
    // exit-on-wait: one "stop" notification
    let (_, out, _) =
        run_within(&d, &["run", "--host", "claude", "--fast", "--exit-on-wait"], &[("PATH", &with_fake_path(&fake))], Duration::from_secs(20));
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

/// 复现 t16：runner 跑着的时候工作树里有别人的在制品（还编译不过），
/// checkpoint 不许把它卷进「zloop tN: <我的 note>」，而且卷不进去的要说出来。
#[test]
fn git_checkpoint_leaves_foreign_work_in_progress_out() {
    let fake = fake_host(
        r#"id=$(printf "%s" "$2" | sed -n "s/.*当前 todo：\(t[0-9]*\) .*/\1/p" | head -1)
echo "work for $id" > "$id.txt"
# 第二轮我们也去动那个已经脏了的文件：两个人的改动缠在同一个文件里，拆不开
if [ "$id" = t2 ]; then echo "// ours too" >> shared.rs; fi
zloop done "$id" --note "wrote $id.txt" --approach "fake host round" >/dev/null 2>&1
echo '{"session_id":"g","is_error":false,"result":"ok"}'"#,
    );
    let d = project(&["[P0] a", "[P0] b"]);
    let git = |args: &[&str]| Command::new("git").args(args).current_dir(&d).output().unwrap();
    git(&["init", "-q", "-b", "main"]);
    git(&["config", "user.email", "t@example.com"]);
    git(&["config", "user.name", "t"]);
    fs::write(d.join(".gitignore"), ".zloop/\n").unwrap();
    fs::write(d.join("shared.rs"), "fn ok() {}\n").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "init"]);

    // 起跑前留下别人的在制品：一处未跟踪的坏改动 + 一处已跟踪文件的改动 + 一处已经 add 过的
    fs::write(d.join("broken.rs"), "fn nope( <<< this does not compile\n").unwrap();
    fs::write(d.join("shared.rs"), "fn ok() {}\nfn half_written(\n").unwrap();
    fs::write(d.join("staged.txt"), "someone else staged this\n").unwrap();
    git(&["add", "staged.txt"]);

    let (code, out, _) = run(&d, &["run", "--host", "claude", "--fast", "--git-commit"], &[("PATH", &with_fake_path(&fake))]);
    assert_eq!(code, 0, "{out}");
    assert_eq!(out.matches("runner: git checkpoint").count(), 2, "{out}");

    // 两条 checkpoint 都只装自己那一轮的产物
    let files = |rev: &str| String::from_utf8_lossy(&git(&["show", "--name-only", "--format=", rev]).stdout).to_string();
    assert_eq!(files("HEAD~1").split_whitespace().collect::<Vec<_>>(), ["t1.txt"], "{}", files("HEAD~1"));
    assert_eq!(files("HEAD").split_whitespace().collect::<Vec<_>>(), ["t2.txt"], "{}", files("HEAD"));

    // 别人的东西一条都没进历史（staged.txt 还留在索引里等它的主人提交），也没被顺手改掉
    let committed = String::from_utf8_lossy(&git(&["log", "--format=", "--name-only"]).stdout).to_string();
    assert!(!committed.contains("broken.rs") && !committed.contains("staged.txt"), "{committed}");
    assert!(String::from_utf8_lossy(&git(&["ls-files"]).stdout).contains("staged.txt"), "别人 add 过的不该被 reset 掉");
    assert_eq!(fs::read_to_string(d.join("shared.rs")).unwrap(), "fn ok() {}\nfn half_written(\n// ours too\n");
    assert_eq!(
        String::from_utf8_lossy(&git(&["show", "HEAD:shared.rs"]).stdout).to_string(),
        "fn ok() {}\n",
        "别人半截的改动不该被提交"
    );

    // 拆不开的那次要出声，而且要进账本
    assert!(out.contains("runner: 没提交 shared.rs"), "{out}");
    let j = journal(&d);
    let held: Vec<_> = j.iter().filter(|e| e["event"] == "commit_held_back").collect();
    assert_eq!(held.len(), 1, "{j:?}");
    assert_eq!(held[0]["paths"][0], "shared.rs");
    assert_eq!(j.iter().filter(|e| e["event"] == "commit").count(), 2);
}

/// 复现 t17：t16 的基线是**起跑那一刻**拍的，之后再没重拍过。于是邻居在两轮之间
/// （runner 正在睡、一个宿主都没跑）新建的文件，因为「基线里没有」被下一轮的
/// checkpoint 认成我们的，提进「zloop tN: <我的 note>」。
#[test]
fn git_checkpoint_leaves_out_a_file_a_neighbour_creates_between_rounds() {
    let fake = fake_host(
        r#"id=$(printf "%s" "$2" | sed -n "s/.*当前 todo：\(t[0-9]*\) .*/\1/p" | head -1)
echo "work for $id" > "$id.txt"
zloop done "$id" --note "wrote $id.txt" --approach "fake host round" >/dev/null 2>&1
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
    // 两轮之间睡 3 秒（--fast 下 1 分 = 1 秒），邻居有足够的窗口落笔
    let p = state::state_path(&d);
    let mut st = state::load(&p).unwrap();
    st.policy.intervals_min = vec![3, 3, 3];
    state::save(&p, &mut st).unwrap();

    // 邻居：等 runner 自己在账本上写下「我要睡了」（第一轮收尾之后才有这条，
    // 那时 checkpoint 早已跑完、基线也刷过了），再趁它睡着新建一个文件。
    // 别拿「commit 出现在 git log 里」当信号：commit 落地到基线刷新之间只有几毫秒，
    // 抢进那道缝里写，文件会被算进新基线——测的就不是要测的那件事了。
    let neighbour_dir = d.clone();
    let neighbour = thread::spawn(move || {
        for _ in 0..1200 {
            let asleep = fs::read_to_string(neighbour_dir.join(".zloop/runner/journal.jsonl"))
                .unwrap_or_default()
                .lines()
                .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
                .any(|e| e["event"] == "sleep");
            if asleep {
                fs::write(neighbour_dir.join("neighbour.rs"), "fn theirs( <<< still typing\n").unwrap();
                return true;
            }
            thread::sleep(Duration::from_millis(10));
        }
        false
    });
    let (code, out, _) = run(&d, &["run", "--host", "claude", "--fast", "--git-commit"], &[("PATH", &with_fake_path(&fake))]);
    assert!(neighbour.join().unwrap(), "邻居没等到 runner 入睡，这个用例没测到东西");
    assert_eq!(code, 0, "{out}");
    assert_eq!(out.matches("runner: git checkpoint").count(), 2, "{out}");

    // 第二轮只装自己的产物；邻居那半截还留在树里等它的主人
    let files = |rev: &str| String::from_utf8_lossy(&git(&["show", "--name-only", "--format=", rev]).stdout).to_string();
    assert_eq!(files("HEAD").split_whitespace().collect::<Vec<_>>(), ["t2.txt"], "{} · {out}", files("HEAD"));
    let committed = String::from_utf8_lossy(&git(&["log", "--format=", "--name-only"]).stdout).to_string();
    assert!(!committed.contains("neighbour.rs"), "邻居在两轮之间新建的文件被卷进了提交：{committed}");
    assert!(d.join("neighbour.rs").exists(), "也不该把它删了或改了");
    // 它是别人的在制品，我们没碰过 → 不属于「拆不开」那一类，不该报
    assert!(!out.contains("没提交 neighbour.rs"), "{out}");
    // 提交了哪几个要说出来（轮内并发判不了，只能靠事后能认出来）
    assert!(out.contains("git checkpoint") && out.contains("t2.txt"), "{out}");
    let commits: Vec<_> = journal(&d).into_iter().filter(|e| e["event"] == "commit").collect();
    assert_eq!(commits.len(), 2);
    assert_eq!(commits[1]["paths"][0], "t2.txt");
}

/// 上一轮没写回（没 checkpoint）留下的改动是**我们自己的**，下一轮要认领回来，
/// 不能因为「这一轮开始时它就脏着」被当成别人的在制品扔掉。
/// 这条正好是上面那条的反面：基线只有在「上一轮结清了」才允许重拍。
#[test]
fn git_checkpoint_reclaims_work_left_by_a_round_that_never_wrote_back() {
    let fake = fake_host(
        r#"id=$(printf "%s" "$2" | sed -n "s/.*当前 todo：\(t[0-9]*\) .*/\1/p" | head -1)
# 第一轮干了活却没写回 → runner 记 fail、不 checkpoint，产物留在树里；第二轮才写回
if [ -e first_round_done ]; then
  zloop done "$id" --note "wrote $id.txt" --approach "fake host round" >/dev/null 2>&1
else
  echo "work for $id" > "$id.txt"; touch first_round_done
fi
echo '{"session_id":"g","is_error":false,"result":"ok"}'"#,
    );
    let d = project(&["[P0] a"]);
    let git = |args: &[&str]| Command::new("git").args(args).current_dir(&d).output().unwrap();
    git(&["init", "-q", "-b", "main"]);
    git(&["config", "user.email", "t@example.com"]);
    git(&["config", "user.name", "t"]);
    fs::write(d.join(".gitignore"), ".zloop/\n").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "init"]);
    let (_, out, _) = run(&d, &["run", "--host", "claude", "--fast", "--git-commit"], &[("PATH", &with_fake_path(&fake))]);
    assert!(out.contains("NO WRITEBACK") && out.contains("runner: git checkpoint"), "{out}");
    // 第一轮的产物没随第一轮提交（那一轮没写回），第二轮的 checkpoint 要把它认领回来
    let head = String::from_utf8_lossy(&git(&["show", "--name-only", "--format=", "HEAD"]).stdout).to_string();
    assert!(head.contains("t1.txt"), "{head} · {out}");
    assert!(!out.contains("没提交"), "{out}");
}

// ---------- A-14：git / notify 子进程的闸 ----------

/// 跑一条 zloop 命令，但**给它一个墙钟上限**。
///
/// 下面这几条要证明的正是「不会挂住」——挂住的时候 `cargo test` 应该当场变红说清楚，
/// 而不是安安静静地卡在那里等到有人来按 Ctrl-C（撤掉修复重跑时就是这个区别）。
fn run_bounded(d: &Path, args: &[&str], env: &[(&str, &str)], limit: Duration) -> (i32, String, String) {
    let mut cmd = Command::new(zloop_bin());
    cmd.current_dir(d)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    common::scrub_ambient_env(&mut cmd);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let child = cmd.spawn().unwrap();
    let pid = child.id();
    let (tx, rx) = std::sync::mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });
    match rx.recv_timeout(limit) {
        Ok(Ok(o)) => (
            o.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&o.stdout).into_owned(),
            String::from_utf8_lossy(&o.stderr).into_owned(),
        ),
        Ok(Err(e)) => panic!("zloop {args:?} 起不来：{e}"),
        Err(_) => {
            let _ = Command::new("kill").arg("-9").arg(pid.to_string()).status();
            panic!("zloop {args:?} 过了 {limit:?} 还没退出 —— 子进程没有闸（A-14 的死法）");
        }
    }
}

/// 一个能提交的空仓库。
fn git_repo(d: &Path) -> impl Fn(&[&str]) -> std::process::Output + '_ {
    let git = move |args: &[&str]| Command::new("git").args(args).current_dir(d).output().unwrap();
    git(&["init", "-q", "-b", "main"]);
    git(&["config", "user.email", "t@example.com"]);
    git(&["config", "user.name", "t"]);
    fs::write(d.join(".gitignore"), ".zloop/\n").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "init"]);
    git
}

/// 只挂住第一次调用的钩子：后面几次秒过。这样一条测试里既踩得到闸，又验得到闸之后仍能干活。
fn hook_that_hangs_once(path: &Path, tail: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mark = path.with_extension("fired");
    fs::write(path, format!("#!/bin/sh\n[ -e '{}' ] || {{ : > '{}'; sleep 30; }}\n{tail}\n", mark.display(), mark.display())).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

/// A-14 回归：`git commit` 卡在 pre-commit 钩子上（husky / lefthook 的默认落点）。
/// 以前 runner 跟着无限期挂住，`--timeout-min` 和 SIGTERM 都叫不动它。
/// 现在超过闸就整组收掉，这一轮按「checkpoint 失败」处理——产物留在树里，下一轮认领回来。
#[test]
fn hung_git_commit_is_cut_off_and_the_work_is_reclaimed_next_round() {
    let fake = fake_host(
        r#"id=$(printf "%s" "$2" | sed -n "s/.*当前 todo：\(t[0-9]*\) .*/\1/p" | head -1)
echo "work for $id" > "$id.txt"
zloop done "$id" --note "wrote $id.txt" --approach "fake host round" >/dev/null 2>&1
echo '{"session_id":"g","is_error":false,"result":"ok"}'"#,
    );
    let d = project(&["[P0] a", "[P0] b"]);
    let git = git_repo(&d);
    hook_that_hangs_once(&d.join(".git/hooks/pre-commit"), "exit 0");

    let (code, out, err) = run_bounded(
        &d,
        &["run", "--host", "claude", "--fast", "--git-commit", "--max-rounds", "2"],
        &[("PATH", &with_fake_path(&fake)), ("ZLOOP_GIT_TIMEOUT_SECS", "2")],
        Duration::from_secs(90),
    );
    assert_eq!(code, 0, "{out}{err}");
    // 第一轮的 commit 被闸掐掉，只有第二轮提交成功
    assert_eq!(out.matches("runner: git checkpoint").count(), 1, "out={out} err={err}");
    assert!(err.contains("runner: git commit 超过闸没回来"), "{err}");
    let stalled: Vec<_> = journal(&d).into_iter().filter(|e| e["event"] == "git_stalled").collect();
    assert_eq!(stalled.len(), 1, "超时那一轮账本要记一条：{stalled:?}");
    assert_eq!((stalled[0]["cmd"].as_str(), stalled[0]["how"].as_str()), (Some("commit"), Some("timeout")));
    assert!(stalled[0]["index_lock_left"].is_boolean(), "锁还在不在要说出来：{stalled:?}");
    // `settled` 保持 false ⇒ 基线没重拍 ⇒ 第一轮的产物没被划给别人，跟着第二轮一起进历史
    let head = String::from_utf8_lossy(&git(&["show", "--name-only", "--format=", "HEAD"]).stdout).to_string();
    assert!(head.contains("t1.txt") && head.contains("t2.txt"), "两轮的产物一个都不能丢：{head}");
    assert_eq!(String::from_utf8_lossy(&git(&["log", "--oneline"]).stdout).lines().count(), 2, "只该有 init + 一次 checkpoint");
}

/// A-14 回归：开工前那次 `git status` 卡在 `core.fsmonitor` 上（网络文件系统 stall 同一格）。
/// 以前它卡在宿主起跑**之前**，账本上一个字都没有；现在闸收掉它，这一轮照常干活，
/// 而且**沿用上一张基线**——绝不能退化成空快照，那会把树里所有脏东西都认成自己的。
#[test]
fn hung_git_status_does_not_wedge_the_round() {
    let fake = fake_host(
        r#"id=$(printf "%s" "$2" | sed -n "s/.*当前 todo：\(t[0-9]*\) .*/\1/p" | head -1)
echo "work for $id" > "$id.txt"
zloop done "$id" --note "wrote $id.txt" --approach "fake host round" >/dev/null 2>&1
echo '{"session_id":"g","is_error":false,"result":"ok"}'"#,
    );
    let d = project(&["[P0] a"]);
    let git = git_repo(&d);
    hook_that_hangs_once(&d.join(".git/hooks/fsmonitor-slow"), r"printf '/\0'");
    git(&["config", "core.fsmonitor", ".git/hooks/fsmonitor-slow"]);

    let (code, out, err) = run_bounded(
        &d,
        &["run", "--host", "claude", "--fast", "--git-commit", "--max-rounds", "1"],
        &[("PATH", &with_fake_path(&fake)), ("ZLOOP_GIT_TIMEOUT_SECS", "2")],
        Duration::from_secs(90),
    );
    assert_eq!(code, 0, "{out}{err}");
    assert!(err.contains("runner: git status 超过闸没回来"), "{err}");
    assert!(err.contains("沿用上一张基线"), "读不出工作树时不许退回空快照：{err}");
    assert!(out.contains("runner: round 1 written back"), "闸收掉 git 之后这一轮照样要干完：{out}");
    let stalled: Vec<_> = journal(&d).into_iter().filter(|e| e["event"] == "git_stalled").collect();
    assert_eq!(stalled.len(), 1, "{stalled:?}");
    assert_eq!((stalled[0]["cmd"].as_str(), stalled[0]["how"].as_str()), (Some("status"), Some("timeout")));
    let head = String::from_utf8_lossy(&git(&["show", "--name-only", "--format=", "HEAD"]).stdout).to_string();
    assert!(head.contains("t1.txt"), "{head}");
}

/// A-14 的修法把 git 的 stdout 从 `String` 换成了**字节**；这一条守住换回去就会破的东西。
///
/// `git status -z` 里的路径可能不是 UTF-8。过一遍 `from_utf8_lossy`，一个叫不出名字的路径
/// 会变成「叫得出但是错的」（`bad\xff.txt` → `bad\u{FFFD}.txt`），拿它去 `git add` 就是
/// `fatal: pathspec … did not match any files`——**整一轮的 checkpoint 一起陪葬**，
/// 只因为树里有一个谁都没在动的文件。按字节读的话它被整条跳过，这一轮照常提交。
///
/// 那个路径是用 git 底层命令塞进索引的：APFS 自己就拒绝非 UTF-8 文件名（Errno 92），
/// 但 git 存的是字节，所以索引里放得下，`status` 也照样把它原样打出来。
#[test]
fn a_path_git_cannot_name_back_does_not_sink_the_whole_checkpoint() {
    let fake = fake_host(
        r#"id=$(printf "%s" "$2" | sed -n "s/.*当前 todo：\(t[0-9]*\) .*/\1/p" | head -1)
echo "work for $id" > "$id.txt"
# 这一轮里冒出来一个文件系统根本落不了地的路径（基线里没有它 → 会被当成「我们的」）
blob=$(printf 'x\n' | git hash-object -w --stdin)
git update-index --add --cacheinfo "100644,$blob,$(printf 'bad\377.txt')"
zloop done "$id" --note "wrote $id.txt" --approach "fake host round" >/dev/null 2>&1
echo '{"session_id":"g","is_error":false,"result":"ok"}'"#,
    );
    let d = project(&["[P0] a"]);
    let git = git_repo(&d);
    let (code, out, err) = run_bounded(
        &d,
        &["run", "--host", "claude", "--fast", "--git-commit", "--max-rounds", "1"],
        &[("PATH", &with_fake_path(&fake))],
        Duration::from_secs(60),
    );
    assert_eq!(code, 0, "{out}{err}");
    assert_eq!(out.matches("runner: git checkpoint").count(), 1, "叫不出名字的路径不该拖垮整轮：out={out} err={err}");
    let head = String::from_utf8_lossy(&git(&["show", "--name-only", "--format=", "HEAD"]).stdout).to_string();
    assert_eq!(head.split_whitespace().collect::<Vec<_>>(), ["t1.txt"], "{head}");
    // 它留在索引里没被动过（更没被用一个错名字提交进去）
    // -z 才拿得到原始字节：`ls-files` 默认会把非 ASCII 路径转义成 "bad\377.txt" 打出来
    assert!(git(&["ls-files", "-z"]).stdout.contains(&0xFF), "那条路径该原样留在索引里");
    assert!(!String::from_utf8_lossy(&git(&["log", "--format=", "--name-only"]).stdout).contains("bad"), "不该进历史");
}

/// A-14 回归：`notify_cmd` 挂住在**收尾**那一下。活全干完了，`stop()` 卡在发通知上，
/// 于是退不出去——「干完就停」这句承诺卡在最后一米。通知发不出去从来不该拖垮 runner。
#[test]
fn hung_notify_cmd_does_not_wedge_the_stop() {
    let fake = fake_host(
        r#"id=$(printf "%s" "$2" | sed -n "s/.*当前 todo：\(t[0-9]*\) .*/\1/p" | head -1)
zloop done "$id" --note ok --approach "fake host round" >/dev/null 2>&1
echo '{"session_id":"n","is_error":false,"result":"ok"}'"#,
    );
    let d = project(&["[P0] a"]);
    let p = state::state_path(&d);
    let mut st = state::load(&p).unwrap();
    st.policy.notify_cmd = Some("sleep 30".into());
    state::save(&p, &mut st).unwrap();

    let (code, out, err) = run_bounded(
        &d,
        &["run", "--host", "claude", "--fast"],
        &[("PATH", &with_fake_path(&fake)), ("ZLOOP_NOTIFY_TIMEOUT_SECS", "2")],
        Duration::from_secs(60),
    );
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("runner: stop (done)"), "{out}");
    assert!(err.contains("notify: command 超过"), "{err}");
    assert_eq!(journal(&d).last().unwrap()["event"], "stop");
}

/// A-14 的同类，落在**写回**那一头（A-15）：`zloop done` 在存盘之前会跑三次 git 去列
/// 「这一轮改了哪些文件」。以前那三次是裸 `.output()`——`core.fsmonitor` 一挂住，
/// `zloop done` 跟着无限期挂住，而且它挂在 `state::transaction` **之前**：
/// 这一轮的 note / approach / evidence 一个字都没落盘，整轮白干。
///
/// 现在闸收掉它，写回照常完成，只是日志少一节「改动文件」——而且**这件事要说出来**，
/// 否则「git 挂住了」和「这不是个仓库」在日志里长得一模一样。
#[test]
fn hung_git_in_write_back_does_not_swallow_the_round() {
    let d = project(&["[P0] a"]);
    let _git = git_repo(&d);
    hook_that_hangs_once(&d.join(".git/hooks/fsmonitor-slow"), r"printf '/\0'");
    Command::new("git").args(["config", "core.fsmonitor", ".git/hooks/fsmonitor-slow"]).current_dir(&d).output().unwrap();
    fs::write(d.join("keep.txt"), "round output\n").unwrap();

    let (code, out, err) = run_bounded(
        &d,
        &["done", "t1", "--note", "活干完了", "--approach", "这一轮的技术文档"],
        &[("ZLOOP_GIT_TIMEOUT_SECS", "2")],
        // 闸 2s + 排水 2s ⇒ 5s 内该退干净。上限**必须压在钩子那 30 秒之下**：
        // 挂住的样子在测试里只能挂 30 秒，60 秒的上限会让裸 `.output()` 也「通过」。
        Duration::from_secs(20),
    );
    assert_eq!(code, 0, "{out}{err}");
    assert!(err.contains("读工作树的 git 超过"), "少了一节就得当场说一句：{err}");
    // 真正要守住的不是那一节清单，是**写回本身**：这一轮的账和文档必须落盘
    let st = state::load(&state::state_path(&d)).unwrap();
    assert_eq!(st.ticks.len(), 1, "写回必须完成：{:?}", st.ticks);
    assert_eq!(st.ticks[0].note, "活干完了");
    let body = fs::read_to_string(d.join(".zloop").join(st.ticks[0].log.as_deref().unwrap())).unwrap();
    assert!(body.contains("这一轮的技术文档"), "approach 必须进日志：{body}");
    assert!(!body.contains("## 改动文件"), "读不出来就别编一节出来：{body}");
}

/// 写回那三次 git 走的是 `Group::Inherit`（跟着调用者的进程组），**不是** `Own`。
///
/// 因为 `zloop done` 在 runner 场景里跑在宿主进程里，宿主超时是整组 `killpg` 收掉的。
/// 单开一组的话这条 git 会从那一刀底下逃走：没人再管它的闸（管闸的父进程已经死了），
/// 它就永远挂在那儿——而挂着的 git 可能正拿着 `.git/index.lock`，那把锁留下来的话，
/// 这个仓库之后所有 git 写操作（包括人自己敲的）全部失败。改成 `Own` 这条就变红。
#[test]
fn write_back_git_dies_with_its_caller() {
    use std::os::unix::process::CommandExt;
    let d = project(&["[P0] a"]);
    let _git = git_repo(&d);
    hook_that_hangs_once(&d.join(".git/hooks/fsmonitor-slow"), r"printf '/\0'");
    Command::new("git").args(["config", "core.fsmonitor", ".git/hooks/fsmonitor-slow"]).current_dir(&d).output().unwrap();
    fs::write(d.join("keep.txt"), "round output\n").unwrap();

    // 闸开到远大于这条测试的时长：要看的是「上层一刀下来它跟不跟着走」，不是它自己的超时
    let mut cmd = Command::new(zloop_bin());
    cmd.current_dir(&d)
        .args(["done", "t1", "--note", "n", "--approach", "a"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    common::scrub_ambient_env(&mut cmd);
    cmd.env("ZLOOP_GIT_TIMEOUT_SECS", "120");
    cmd.process_group(0); // 让它自己当组长，好模拟「上层对宿主整组下刀」
    let mut child = cmd.spawn().unwrap();
    let pid = child.id();

    let git_pid = (0..100)
        .find_map(|_| {
            let o = Command::new("pgrep").args(["-P", &pid.to_string()]).output().unwrap();
            let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if s.is_empty() {
                thread::sleep(Duration::from_millis(100));
                None
            } else {
                s.lines().next().and_then(|l| l.trim().parse::<u32>().ok())
            }
        })
        .expect("挂住的 git 子进程该出现");

    Command::new("kill").args(["-TERM", &format!("-{pid}")]).output().unwrap(); // killpg，整组
    let _ = child.wait();
    thread::sleep(Duration::from_millis(1500));
    let alive = Command::new("kill").args(["-0", &git_pid.to_string()]).output().unwrap().status.success();
    if alive {
        let _ = Command::new("kill").args(["-9", &git_pid.to_string()]).output();
    }
    assert!(!alive, "git {git_pid} 从调用者的组里逃走了，变成没人管的孤儿（Group::Own 的死法）");
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
    // 回看那一轮的输出**故意超过 300 字**：无头回看不写回账本，这份全文是它唯一的产物，
    // 曾经 runner 在 300 字处截断，建议清单的后半截就此消失。
    let fake = fake_host(
        r#"case "$2" in
  *"回看一次"*) echo "$2" > "$TMPDIR_MARK/reflect-prompt"
     body="建议：第 1、2 条合并"
     i=1; while [ $i -le 40 ]; do body="$body ·第 $i 条要点写清楚"; i=$((i+1)); done
     printf '{"session_id":"r","is_error":false,"result":"%s 最后一条：TAIL-MARKER-END"}\n' "$body" ;;
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
    assert!(!r.note.contains("TAIL-MARKER-END"), "账本里只留摘要，不塞全文: {}", r.note);

    // 全文落盘：日志里一个字都不能少（截断过的版本到不了 TAIL-MARKER-END）
    let rel = r.log.as_ref().expect("回看要留日志");
    assert!(rel.contains("reflect"), "{rel}");
    let body = fs::read_to_string(d.join(".zloop").join(rel)).unwrap();
    assert!(body.contains("TAIL-MARKER-END"), "回看全文被截断了，尾巴没落盘：{body}");
    assert!(body.contains("·第 40 条要点写清楚"), "中间也不能缺：{body}");
    assert!(body.chars().count() > 400, "只有 {} 字，像是被截过", body.chars().count());

    // 不占 todo 轮次：三条 todo 该做完的照样做完
    assert_eq!(st.todos.iter().filter(|t| t.status == "done").count(), 3, "{:?}", st.todos);
    // 回看那一轮不推进轮次编号
    assert_eq!(zloop::tick::current_round(&st.ticks), 3);
    // 「跑了几轮」只有一个定义：status 和 stats 报同一个数，回看不算一轮
    assert_eq!(zloop::tick::rounds(&st.ticks), 3, "{:?}", st.ticks.iter().map(|t| &t.outcome).collect::<Vec<_>>());
    let status = run(&d, &["status"], &[]).1;
    assert!(status.contains("跑了 3 轮"), "status 把回看也算成一轮了：{status}");
    let stats = run(&d, &["stats", "--json"], &[]).1;
    assert!(stats.contains("\"rounds\": 3"), "{stats}");
    // 它也没动经验文件
    assert!(!d.join(".zloop/NOTES.md").exists(), "无头回看不该自己落地");
    // 材料包确实是回看用的那一份
    let prompt = fs::read_to_string(mark.join("reflect-prompt")).unwrap();
    assert!(prompt.contains("现有经验") && prompt.contains("不要**运行 `zloop reflect --apply`"), "{prompt}");
}

/// 无头模式下按信号插一轮重估：**只产出建议，绝不自己改 todo**。
/// 计划是人和 agent 共同定稿的东西——没人点头的时候，runner 最多只能提议。
#[test]
fn the_stop_hook_goes_quiet_while_a_runner_owns_the_queue() {
    // #14：runner 在跑的时候，任何开着的交互会话每结束一轮对话都会被 Stop hook
    // 推去做**同一条** todo。源码文件没有锁，两个 agent 同时改就是互相覆盖。
    // 2026-08-29 那次 4 小时长跑里每一轮都在发生。
    let d = project(&["[P0] a", "[P0] b"]);
    let slow = fake_host(r#"sleep 60; echo '{"session_id":"s","is_error":false,"result":"late"}'"#);
    let tools = fake_power_tools(true);
    let path = format!("{}:{}", tools.display(), with_fake_path(&slow));
    let e = awake_env();
    let vars = awake_vars(&e, &path);

    // runner 不在：行为一字不变，照常催
    let (code, out) = hook_stop(&d, &[("CLAUDE_CODE_SESSION_ID", "另一个会话")]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("\"block\""), "runner 不在时该照常催: {out}");

    // runner 在跑：闭嘴
    let (code, out, err) = run(&d, &["start", "--fast", "--timeout-min", "120"], &vars);
    assert_eq!(code, 0, "{out}{err}");
    assert!(d.join(".zloop/runner/pid").exists(), "runner 该起来了: {out}{err}");
    let (code, out) = hook_stop(&d, &[("CLAUDE_CODE_SESSION_ID", "另一个会话")]);
    assert_eq!((code, out.as_str()), (0, ""), "runner 在跑时不能再催别的会话");

    // 换一个会话 id 问同样问题：一样闭嘴（挡的是「有 runner」，不是「是谁」）
    let (code, out) = hook_stop(&d, &[("CLAUDE_CODE_SESSION_ID", "第三个会话")]);
    assert_eq!((code, out.as_str()), (0, ""));

    // runner 自己的子进程走的是另一道闸（ZLOOP_RUNNER），行为不变
    let (code, out) = hook_stop(&d, &[("ZLOOP_RUNNER", "1"), ("CLAUDE_CODE_SESSION_ID", "runner 的孩子")]);
    assert_eq!((code, out.as_str()), (0, ""));

    // 轮次之间休息时也要闭嘴：判据是「有没有 runner」不是「它此刻忙不忙」。
    // 上面那几次撞上的是「正在某一轮」，这里造一个确定性的「活着但手上没活」——
    // runner 醒来就会接着领活，这会儿放交互会话进去只是换个时刻撞车。
    run(&d, &["stop"], &vars);
    let mut st = state::load(&state::state_path(&d)).unwrap();
    st.in_progress = None;
    state::save(&state::state_path(&d), &mut st).unwrap();
    zloop::daemon::write_pid(&d, std::process::id()).unwrap(); // 测试进程自己就是个活着的 pid
    assert!(zloop::daemon::running(&d).is_some());
    let (code, out) = hook_stop(&d, &[("CLAUDE_CODE_SESSION_ID", "另一个会话")]);
    assert_eq!((code, out.as_str()), (0, ""), "runner 在轮次之间睡觉时同样不能催");
    fs::remove_file(d.join(".zloop/runner/pid")).unwrap();

    // runner 停了：立刻恢复催活，别把人永远锁在门外
    run(&d, &["stop"], &vars);
    assert!(!d.join(".zloop/runner/pid").exists(), "stop 之后 pid 文件该没了");
    let (code, out) = hook_stop(&d, &[("CLAUDE_CODE_SESSION_ID", "另一个会话")]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("\"block\""), "runner 停了就该恢复原样: {out}");
}

#[test]
fn a_standing_block_only_triggers_one_replan_until_someone_else_gets_stuck() {
    // `blocked` 是这几个信号里唯一的**锁存**：其余四个从近期活动推出来、会自然衰减，
    // 而它一旦挂上，无头模式下没人能来解，于是每一轮都会放炮。踩过：一次 4 小时的
    // 长跑里 5 次重估全由同一条「t21 在等你回话」触发。所以按**边沿**处理。
    let d = project(&["[P0] 要人拍板的一条", "[P0] 第二条", "[P0] 第三条", "[P0] 第四条"]);
    let mark = tempfile::tempdir().unwrap().keep();
    // 干活的轮次一律写回 done：progress 会把返工率推上去，`rework` 一响就成了另一个信号，
    // 那时重估是**该**跑的，测不出锁存这一条。第一轮把 t1 挂到人身上，
    // 之后每一轮 `blocked` 都成立，但等的始终是同一个人。
    let fake = fake_host(
        r#"case "$2" in
  *"重估一次"*)
     echo x >> "$TMPDIR_MARK/replan-count"
     echo '{"session_id":"rp","is_error":false,"result":"建议：先等人回话"}' ;;
  *) id=$(printf "%s" "$2" | sed -n "s/.*当前 todo：\(t[0-9]*\) .*/\1/p" | head -1)
     if [ ! -f "$TMPDIR_MARK/blocked" ]; then
       touch "$TMPDIR_MARK/blocked"
       zloop done "$id" --outcome progress --block "这条要你拍板" --note "挂起" >/dev/null 2>&1
     else
       zloop done "$id" --note "做完了" --approach "假宿主一轮" --no-doc >/dev/null 2>&1
     fi
     echo '{"session_id":"s","is_error":false,"result":"ok"}' ;;
esac"#,
    );
    let (code, out, _) = run(
        &d,
        &["run", "--host", "claude", "--fast", "--max-rounds", "4"],
        &[("PATH", &with_fake_path(&fake)), ("TMPDIR_MARK", mark.to_str().unwrap())],
    );
    assert_eq!(code, 0, "{out}");

    let st = state::load(&state::state_path(&d)).unwrap();
    let work = st.ticks.iter().filter(|t| t.outcome == "done").count();
    let replans = st.ticks.iter().filter(|t| t.outcome == "replan").count();
    assert!(work >= 3, "得真跑几轮才测得出重复: {:?}", st.ticks.iter().map(|t| &t.outcome).collect::<Vec<_>>());
    assert!(st.todos[0].blocked_by.contains(&"user".to_string()), "t1 全程挂在人身上");
    // 挂起那一轮响一次；后面几轮等的还是同一个人，不该再烧模型轮次
    assert_eq!(
        replans, 1,
        "同一批人一直在被等，只该重估一次（实到 {replans} 次 / {work} 轮活）: {:?}",
        st.ticks.iter().map(|t| &t.outcome).collect::<Vec<_>>()
    );
    let fired = fs::read_to_string(mark.join("replan-count")).unwrap_or_default().lines().count();
    assert_eq!(fired, 1, "宿主也只该被叫去重估一次");
}

#[test]
fn auto_replan_swaps_the_route_mid_run_and_keeps_going() {
    // 用户要的那一幕，端到端：5 条 todo，做到第 2 条发现整条路线的前提没了，
    // 重估那一轮**真的把清单换掉**，然后接着把新清单跑完。
    let d = project(&["[P0] 量最慢三处 :: 有数", "[P0] 加缓存 :: 快 500ms", "[P0] 复测 :: 基准过",
                      "[P1] 补基准 :: bench 跑得出", "[P1] 写文档 :: README 有一节"]);
    let mark = tempfile::tempdir().unwrap().keep();
    // 干活轮次：第 2 条写回时说「后续走不通」；重估轮次：真的调 replan --apply
    let fake = fake_host(
        r#"case "$2" in
  *"重估一次"*)
     echo "$2" > "$TMPDIR_MARK/replan-prompt"
     if [ -f "$TMPDIR_MARK/replanned" ]; then
       echo '{"session_id":"rp","is_error":false,"result":"不用再改了"}'; exit 0
     fi
     touch "$TMPDIR_MARK/replanned"
     printf '%s\n'        '[P0] 量反序列化耗时 :: 有逐字段表'        '[P0] 换零拷贝路径 :: 快 300ms'        '[P0] 惰性加载 :: 只解析用得到的'        '[P1] 复测 :: 端到端 1 秒内'        | zloop replan --apply --why "实测瓶颈在反序列化，加缓存整条路线作废" >> "$TMPDIR_MARK/apply.log" 2>&1
     echo '{"session_id":"rp","is_error":false,"result":"照新现状重排了"}' ;;
  *) id=$(printf "%s" "$2" | sed -n "s/.*当前 todo：\(t[0-9]*\) .*/\1/p" | head -1)
     if [ "$id" = t2 ]; then
       zloop done "$id" --note "只省 30ms" --approach "LRU" --no-doc          --rethink "瓶颈在反序列化，后三条前提没了" >/dev/null 2>&1
     else
       zloop done "$id" --note ok --approach "假宿主一轮" --no-doc >/dev/null 2>&1
     fi
     echo '{"session_id":"s","is_error":false,"result":"ok"}' ;;
esac"#,
    );
    let (code, out, err) = run(
        &d,
        &["run", "--host", "claude", "--fast", "--auto-replan", "--max-rounds", "8"],
        &[("PATH", &with_fake_path(&fake)), ("TMPDIR_MARK", mark.to_str().unwrap())],
    );
    assert_eq!(code, 0, "{out}{err}");
    let apply_log = fs::read_to_string(mark.join("apply.log")).unwrap_or_default();
    assert!(apply_log.contains("replan applied"), "重估那一轮该真的落地: {apply_log}");
    assert!(out.contains("计划改了"), "runner 该看出计划动了: {out}");

    let st = state::load(&state::state_path(&d)).unwrap();
    // 做过的两条原样留着，被换掉的三条不在了，新路线跑完了
    assert_eq!(st.todos.iter().filter(|t| t.id == "t1" || t.id == "t2").filter(|t| t.status == "done").count(), 2,
               "做过的原样留着: {:?}", st.todos);
    assert!(!st.todos.iter().any(|t| t.id == "t3"), "被换掉的不该还在: {:?}", st.todos);
    assert!(st.todos.iter().any(|t| t.text.contains("零拷贝")), "新路线排上了: {:?}", st.todos);
    assert!(st.todos.iter().all(|t| matches!(t.status.as_str(), "done" | "deferred")), "新清单也跑完了: {:?}", st.todos);
    assert_eq!(st.goal.text, "runner test", "目标文字不许被改");

    // journal 里留得下"计划在第几轮被改成几条"
    let applied: Vec<_> = journal(&d).into_iter().filter(|e| e["event"] == "replan_applied").collect();
    assert_eq!(applied.len(), 1, "改了一次: {applied:?}");
    assert!(applied[0]["open_after"].as_u64().unwrap() > applied[0]["open_before"].as_u64().unwrap(),
            "3 条换成 4 条: {applied:?}");

    // 提示词里要给出落地的命令和护栏，否则模型不知道怎么落地
    let prompt = fs::read_to_string(mark.join("replan-prompt")).unwrap();
    assert!(prompt.contains("zloop replan --apply"), "{prompt}");
    assert!(prompt.contains("不改是完全合格的结论"), "别为了改而改: {prompt}");
    assert!(prompt.contains("单次运行最多"), "要让它知道有上限: {prompt}");
}

#[test]
fn a_plan_that_keeps_growing_stops_in_front_of_a_human() {
    // 能改自己计划的循环最容易死在 replan → 新 todo → replan → …… 永不收敛上。
    // 造一个每次重估都把清单改长的宿主，验证它**停下来等人**，而不是一直跑。
    let d = project(&["[P0] a :: 验a", "[P0] b :: 验b"]);
    let mark = tempfile::tempdir().unwrap().keep();
    let fake = fake_host(
        r#"case "$2" in
  *"重估一次"*)
     n=$(cat "$TMPDIR_MARK/n" 2>/dev/null || echo 0); n=$((n+1)); echo $n > "$TMPDIR_MARK/n"
     # 每次都比现在多排两条 —— 典型的发散
     : > "$TMPDIR_MARK/plan"
     i=1; while [ $i -le $((n+3)) ]; do echo "[P0] 第${n}轮第${i}条 :: 验" >> "$TMPDIR_MARK/plan"; i=$((i+1)); done
     zloop replan --apply --why "再拆细一点" < "$TMPDIR_MARK/plan" >> "$TMPDIR_MARK/apply.log" 2>&1
     echo '{"session_id":"rp","is_error":false,"result":"又拆细了"}' ;;
  *) id=$(printf "%s" "$2" | sed -n "s/.*当前 todo：\(t[0-9]*\) .*/\1/p" | head -1)
     zloop done "$id" --note ok --approach x --no-doc --rethink "还是走不通，再拆" >/dev/null 2>&1
     echo '{"session_id":"s","is_error":false,"result":"ok"}' ;;
esac"#,
    );
    let (code, out, err) = run(
        &d,
        &["run", "--host", "claude", "--fast", "--auto-replan", "--max-rounds", "30"],
        &[("PATH", &with_fake_path(&fake)), ("TMPDIR_MARK", mark.to_str().unwrap())],
    );
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("停下来等人"), "跑飞了要停在人面前: {out}");
    assert!(out.contains("在发散，不是在收敛"), "要说清为什么停: {out}");

    let j = journal(&d);
    let applied = j.iter().filter(|e| e["event"] == "replan_applied").count();
    assert!(applied <= zloop::runner::MAX_AUTO_REPLANS as usize, "最多改 {} 次就该停，实际 {applied} 次", zloop::runner::MAX_AUTO_REPLANS);
    assert_eq!(j.iter().filter(|e| e["event"] == "replan_giveup").count(), 1, "要留下放弃记录: {j:?}");
    assert!(j.iter().any(|e| e["event"] == "stop" && e["reason"] == "replan_diverged"), "停机理由要写清: {j:?}");
    // 关键：真的停了，不是跑满 30 轮
    let rounds = j.iter().filter(|e| e["event"] == "begin").count();
    assert!(rounds < 30, "该提前停，不该跑满 30 轮（实际 {rounds} 轮）");
    // 停下来的时候计划还在，没被改成半截
    let st = state::load(&state::state_path(&d)).unwrap();
    assert!(st.todos.iter().filter(|t| !matches!(t.status.as_str(), "done" | "deferred")).count() > 0, "停机时清单还在: {:?}", st.todos);
}

#[test]
fn without_the_flag_a_replan_round_still_never_touches_the_plan() {
    // 默认关：行为一字不变——哪怕宿主试图落地也不该有落地的入口被提到
    let d = project(&["[P0] a :: 验a", "[P0] b :: 验b", "[P0] c :: 验c"]);
    let mark = tempfile::tempdir().unwrap().keep();
    let fake = fake_host(
        r#"case "$2" in
  *"重估一次"*)
     echo "$2" > "$TMPDIR_MARK/replan-prompt"
     printf '%s\n' '[P0] 偷偷换掉 :: 验' | zloop replan --apply --why "不守规矩" >> "$TMPDIR_MARK/apply.log" 2>&1
     echo '{"session_id":"rp","is_error":false,"result":"建议：换个路线"}' ;;
  *) id=$(printf "%s" "$2" | sed -n "s/.*当前 todo：\(t[0-9]*\) .*/\1/p" | head -1)
     zloop done "$id" --note ok --approach x --no-doc --rethink "后续走不通" >/dev/null 2>&1
     echo '{"session_id":"s","is_error":false,"result":"ok"}' ;;
esac"#,
    );
    let (code, out, err) = run(
        &d,
        &["run", "--host", "claude", "--fast", "--max-rounds", "3"],
        &[("PATH", &with_fake_path(&fake)), ("TMPDIR_MARK", mark.to_str().unwrap())],
    );
    assert_eq!(code, 0, "{out}{err}");
    let prompt = fs::read_to_string(mark.join("replan-prompt")).unwrap();
    assert!(prompt.contains("不要**运行任何会改 todo 的命令"), "默认还是红线那套: {prompt}");
    assert!(!prompt.contains("zloop replan --apply"), "默认不该告诉它怎么落地: {prompt}");
    assert!(out.contains("没有动任何 todo"), "{out}");
    assert!(!out.contains("计划改了"), "{out}");
    // 宿主抗命硬跑了 --apply —— 该被**代码**挡住，不是被提示词劝住
    let applied = fs::read_to_string(mark.join("apply.log")).unwrap_or_default();
    assert!(applied.contains("护栏「无头默认不改计划」"), "抗命的 --apply 要被拒绝并说清原因: {applied}");
    assert!(!applied.contains("replan applied"), "一次都不许成功: {applied}");
    assert!(journal(&d).into_iter().all(|e| e["event"] != "replan_applied"), "默认模式不该记落地事件");
}

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

// ---------- A-6: 超时那一轮要连孙进程一起收，超时窗口里 SIGTERM 要叫得动 runner ----------

/// A-6 上半：`--timeout-min` 曾经管不住留下后台进程的那一轮。
///
/// `run_with_timeout` 到点只 `child.kill()` 直接子进程，可**孙进程继承了同一个管道写端**——
/// 只要它还活着，排水线程的 `read` 就等不到 EOF，`join()` 一直挂着。实测那一轮的耗时跟着
/// 孙进程的寿命走，跟 `--timeout-min` 没关系；换成真守护进程就是永远不结束。
/// 现在子进程单开一个进程组，超时时 `killpg` 整组。
#[test]
fn timeout_collects_the_background_grandchildren_too() {
    let fake = fake_host("echo '{\"session_id\":\"gc\",\"is_error\":false,\"result\":\"ok\"}'");
    let d = project(&["[P0] a"]);
    let mark = tempfile::tempdir().unwrap().keep();
    let survived = mark.join("grandchild_survived");
    let p = state::state_path(&d);
    let mut st = state::load(&p).unwrap();
    // 后台孙进程活 4 秒后按下手印，前台再挂 8 秒：2 秒的闸到点时两个都还在。
    st.policy.preflight_cmd = Some(format!("sh -c 'sleep 4; : > {}' & sleep 8", survived.display()));
    st.policy.max_fail_streak = 1; // 一轮就够，超时判定在第一轮就发生
    state::save(&p, &mut st).unwrap();

    let started = std::time::Instant::now();
    let (_, out, _) = run(&d, &["run", "--host", "claude", "--fast", "--timeout-min", "2"], &[("PATH", &with_fake_path(&fake))]);
    let elapsed = started.elapsed();

    assert!(out.contains("preflight timed out"), "{out}");
    // 闸是 2 秒（+ 整组 SIGTERM 的 0.5 秒宽限 + 排水）；旧代码要等前台那个 sleep 8 咽气。
    assert!(elapsed < Duration::from_secs(6), "超时那一轮走了 {elapsed:?}，`--timeout-min 2` 没兜住");
    // 孙进程死在按手印之前：等过它本来的寿命再看，手印始终不该出现。
    assert!(!survived.exists(), "runner 刚退出时孙进程就已经按上手印了？");
    thread::sleep(Duration::from_secs(5));
    assert!(!survived.exists(), "超时那一轮的后台孙进程活过了 runner：{}", survived.display());
}

/// A-6 下半：卡在排水上的那段时间里，runner 谁也叫不动。
///
/// `stop_requested()` 只在 `try_wait` 循环里查，`join()` 上没人查——`zloop stop` 发的 SIGTERM
/// 要等孙进程自己咽气才生效，只剩 SIGKILL 一条路，而 SIGKILL 会跳过 `AwakeGuard` 的 `Drop`，
/// keep-awake 就此漏在系统里。现在排水有 deadline，超时那一轮宁可少记一段 stdout。
#[test]
fn sigterm_reaches_the_runner_while_a_grandchild_holds_the_pipe() {
    let fake = fake_host("echo '{\"session_id\":\"gc2\",\"is_error\":false,\"result\":\"ok\"}'");
    let d = project(&["[P0] a"]);
    let mark = tempfile::tempdir().unwrap().keep();
    let entered = mark.join("preflight_entered");
    let p = state::state_path(&d);
    let mut st = state::load(&p).unwrap();
    // 起一个活 20 秒的后台孙进程，前台也挂住：runner 会老老实实在超时窗口里等。
    st.policy.preflight_cmd = Some(format!("MARK={} ; : > $MARK ; sh -c 'sleep 20' & sleep 20", entered.display()));
    state::save(&p, &mut st).unwrap();

    let mut cmd = Command::new(zloop_bin());
    cmd.current_dir(&d)
        .args(["run", "--host", "claude", "--fast", "--timeout-min", "60"])
        .env("PATH", with_fake_path(&fake))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    common::scrub_ambient_env(&mut cmd);
    let mut child = cmd.spawn().unwrap();

    // 等 preflight 真的进去了（孙进程已经起来），再叫停
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while !entered.exists() && std::time::Instant::now() < deadline {
        thread::sleep(Duration::from_millis(50));
    }
    assert!(entered.exists(), "preflight 一直没跑起来");
    thread::sleep(Duration::from_millis(300));

    let signalled = std::time::Instant::now();
    assert!(Command::new("kill").args(["-TERM", &child.id().to_string()]).status().unwrap().success());
    let mut exited = false;
    while signalled.elapsed() < Duration::from_secs(6) {
        if child.try_wait().unwrap().is_some() {
            exited = true;
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    let took = signalled.elapsed();
    if !exited {
        let _ = child.kill();
    }
    let _ = child.wait();
    assert!(exited, "SIGTERM 之后 6 秒 runner 还活着：它卡在排水上，只能 SIGKILL（A-6）");
    assert!(took < Duration::from_secs(6), "SIGTERM 到退出走了 {took:?}");
    assert!(
        journal(&d).iter().any(|e| e["event"] == "stop" && e["reason"] == "sigterm"),
        "干净退出要记 journal：{:?}",
        journal(&d)
    );
}
