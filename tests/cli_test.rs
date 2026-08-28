mod common;

use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use zloop::hosts;
use zloop::state;

struct Out {
    code: i32,
    out: String,
    err: String,
}

fn zloop(dir: &Path, args: &[&str], stdin: Option<&str>, env: &[(&str, &str)]) -> Out {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_zloop"));
    cmd.current_dir(dir).args(args);
    cmd.env_remove("CLAUDE_CODE_SESSION_ID").env_remove("CODEX_THREAD_ID");
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() });
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("spawn zloop");
    if let Some(s) = stdin {
        child.stdin.take().unwrap().write_all(s.as_bytes()).unwrap();
    }
    let o = child.wait_with_output().unwrap();
    Out {
        code: o.status.code().unwrap_or(-1),
        out: String::from_utf8_lossy(&o.stdout).into_owned(),
        err: String::from_utf8_lossy(&o.stderr).into_owned(),
    }
}

#[test]
fn end_to_end() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let o = zloop(d, &["init", "Ship zloop v0"], None, &[]);
    assert_eq!(o.code, 0, "{}", o.err);
    assert!(o.out.contains("initialized"));

    let o = zloop(d, &["init", "other"], None, &[]);
    assert_eq!(o.code, 1);
    assert!(o.err.contains("already initialized"));

    let o = zloop(d, &["plan"], Some("[P0] design\n[P1] build\n[P2] docs\n"), &[]);
    assert_eq!(o.code, 0, "{}", o.err);
    assert_eq!(o.out.lines().collect::<Vec<_>>(), ["t1 [P0] design", "t2 [P1] build", "t3 [P2] docs"]);

    let o = zloop(d, &["next", "--json"], None, &[]);
    let payload: serde_json::Value = serde_json::from_str(&o.out).unwrap();
    assert_eq!(payload["should_run"], true);
    assert_eq!(payload["todo"]["id"], "t1");
    assert_eq!(payload["remaining"], 3);
    assert_eq!(payload["round"], 0);
    assert_eq!(payload["interval_min"], 3);

    let o = zloop(d, &["done", "t1", "--note", "DESIGN.md written", "--next", "review design", "--evidence", "line1\nline2", "--no-doc"], None, &[]);
    assert_eq!(o.code, 0, "{}", o.err);
    assert!(o.out.starts_with("t1 done: DESIGN.md written"));
    assert!(o.out.contains("next: t4 [P0] review design"));
    assert!(o.out.contains("log: .zloop/log/"));

    let o = zloop(d, &["done", "t4", "--outcome", "fail", "--note", "reviewer away"], None, &[]);
    assert_eq!(o.code, 0);
    assert!(o.out.contains("t4 fail"));

    let o = zloop(d, &["done", "t4", "--block", "need product sign-off"], None, &[]);
    assert_eq!(o.code, 0);
    assert!(o.out.contains("t4 block"));
    // blocked P0 does not stop P1 from running
    let o = zloop(d, &["next", "--json"], None, &[]);
    let payload: serde_json::Value = serde_json::from_str(&o.out).unwrap();
    assert_eq!(payload["todo"]["id"], "t2");

    let o = zloop(d, &["edit", "t4", "--status", "open", "--priority", "2"], None, &[]);
    assert_eq!(o.code, 0, "{}", o.err);
    assert!(o.out.contains("t4 [P2] open"));

    let o = zloop(d, &["status"], None, &[]);
    assert!(o.out.contains("Ship zloop v0"), "{}", o.out);
    assert!(o.out.contains("1 轮"), "{}", o.out);

    let o = zloop(d, &["status", "--md"], None, &[]);
    assert!(o.out.starts_with("# zloop"));
    assert!(o.out.contains("`t2`"));

    let st = state::load(&state::state_path(d)).unwrap();
    let outcomes: Vec<&str> = st.ticks.iter().map(|t| t.outcome.as_str()).collect();
    assert_eq!(outcomes, ["done", "fail", "block", "edit"]);
    assert_eq!(st.ticks[0].host.as_deref(), Some("cli"));
    assert!(st.ticks[0].log.as_deref().unwrap().starts_with("log/"));

    // logs
    let o = zloop(d, &["log"], None, &[]);
    assert!(o.out.contains("-t1-done.md"));
    let files = zloop::log::entries(d, Some("t1"), 10).unwrap();
    assert_eq!(files.len(), 1);
    let body = fs::read_to_string(&files[0]).unwrap();
    assert!(body.contains("## 验证证据") && body.contains("line2"));
    let o = zloop(d, &["log", "--show", files[0].file_name().unwrap().to_str().unwrap()], None, &[]);
    assert!(o.out.contains("- note: DESIGN.md written"));
}

#[test]
fn next_records_noop_unless_peek() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    zloop(d, &["init", "g"], None, &[]);
    zloop(d, &["plan", "--add", "[P0] a"], None, &[]);
    zloop(d, &["done", "t1", "--block", "?"], None, &[]);
    zloop(d, &["next", "--peek"], None, &[]);
    zloop(d, &["next"], None, &[]);
    let o = zloop(d, &["next"], None, &[]);
    assert!(o.out.contains("WAIT (user_gate)"));
    let st = state::load(&state::state_path(d)).unwrap();
    let outcomes: Vec<&str> = st.ticks.iter().map(|t| t.outcome.as_str()).collect();
    assert_eq!(outcomes, ["block", "noop", "noop"]);
}

#[test]
fn done_errors() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    zloop(d, &["init", "g"], None, &[]);
    zloop(d, &["plan", "--add", "[P0] a"], None, &[]);
    let o = zloop(d, &["done", "t9", "--no-doc"], None, &[]);
    assert_eq!(o.code, 2);
    assert!(o.err.contains("unknown todo id"));
    zloop(d, &["done", "t1", "--no-doc"], None, &[]);
    let o = zloop(d, &["done", "t1", "--no-doc"], None, &[]);
    assert_eq!(o.code, 2);
    assert!(o.err.contains("already done"));
}

#[test]
fn missing_state_is_a_clean_error() {
    let dir = tempfile::tempdir().unwrap();
    let o = zloop(dir.path(), &["next"], None, &[]);
    assert_eq!(o.code, 1);
    assert!(o.err.contains("no zloop state"));
}

#[test]
fn heartbeat_hosts_and_budget() {
    let dir = tempfile::tempdir().unwrap();
    zloop(dir.path(), &["init", "a goal"], None, &[]);
    for host in ["claude", "codex-app", "codex-cli"] {
        let o = zloop(dir.path(), &["heartbeat", "--host", host], None, &[]);
        assert_eq!(o.code, 0);
        assert!(o.out.contains("zloop next --json"));
        assert!(o.out.chars().count() <= 1300, "{host}: {}", o.out.chars().count());
    }
}

#[test]
fn session_is_captured_from_host_env() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    zloop(d, &["init", "g"], None, &[]);
    zloop(d, &["plan", "--add", "[P0] a", "--add", "[P1] b"], None, &[]);
    let o = zloop(d, &["done", "t1", "--note", "x", "--no-doc"], None, &[("CLAUDE_CODE_SESSION_ID", "11111111-2222-3333-4444-555555555555")]);
    assert_eq!(o.code, 0, "{}", o.err);
    let o = zloop(d, &["done", "t2", "--outcome", "progress", "--note", "y"], None, &[("CODEX_THREAD_ID", "thread-abc")]);
    assert_eq!(o.code, 0, "{}", o.err);
    let o = zloop(d, &["sessions", "--json"], None, &[]);
    let rows: serde_json::Value = serde_json::from_str(&o.out).unwrap();
    let rows = rows.as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["host"], "claude");
    assert_eq!(rows[0]["resume"], "claude --resume 11111111-2222-3333-4444-555555555555");
    assert_eq!(rows[1]["host"], "codex");
    assert_eq!(rows[1]["resume"], "codex resume thread-abc");
    let o = zloop(d, &["sessions", "--host", "codex"], None, &[]);
    assert!(o.out.contains("codex resume thread-abc"));
    assert!(!o.out.contains("claude --resume"));
    let o = zloop(d, &["status", "--md"], None, &[]);
    assert!(o.out.contains("## Sessions"));
}

#[test]
fn context_respects_budget_and_names_next() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    zloop(d, &["init", "Long goal text for the context packet"], None, &[]);
    zloop(d, &["plan", "--add", "[P0] first thing", "--add", "[P1] second thing"], None, &[]);
    zloop(d, &["done", "t1", "--note", "done first", "--no-doc"], None, &[("CLAUDE_CODE_SESSION_ID", "sess-1")]);
    let o = zloop(d, &["context", "--for", "codex"], None, &[]);
    assert!(o.out.contains("## 下一条") && o.out.contains("t2 [P1] second thing"));
    assert!(o.out.contains("claude --resume sess-1"));
    assert!(o.out.contains("在 Codex 里"));
    let o = zloop(d, &["context", "--budget", "300"], None, &[]);
    assert!(o.out.chars().count() <= 301, "{}", o.out.chars().count());
    assert!(o.out.contains("## 目标"));
}

#[test]
fn install_is_idempotent_and_refuses_unmanaged() {
    let home = tempfile::tempdir().unwrap();
    let results = hosts::install(true, true, true, home.path()).unwrap();
    assert!(results.iter().all(|(_, changed)| *changed));
    let again = hosts::install(true, true, true, home.path()).unwrap();
    assert!(again.iter().all(|(_, changed)| !*changed));
    let skill = home.path().join(".claude/skills/zloop/SKILL.md");
    let text = fs::read_to_string(&skill).unwrap();
    assert!(text.starts_with("---\nname: \"zloop\""));
    assert!(text.contains(hosts::MANAGED_MARK));
    assert!(text.contains("zloop context"));
    let settings: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(home.path().join(".claude/settings.json")).unwrap()).unwrap();
    assert_eq!(settings["hooks"]["Stop"][0]["hooks"][0]["command"], hosts::HOOK_COMMAND);
    fs::write(&skill, "# my own file\n").unwrap();
    assert!(hosts::install_claude(home.path()).is_err());
}

#[test]
fn hook_stop_blocks_only_when_runnable() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    zloop(d, &["init", "g"], None, &[]);
    zloop(d, &["plan", "--add", "[P0] a"], None, &[]);
    let o = zloop(d, &["hook-stop"], Some("{}"), &[]);
    assert_eq!(o.code, 0);
    let v: serde_json::Value = serde_json::from_str(&o.out).unwrap();
    assert_eq!(v["decision"], "block");
    zloop(d, &["done", "t1", "--no-doc"], None, &[]);
    let o = zloop(d, &["hook-stop"], Some("{}"), &[]);
    assert_eq!(o.code, 0);
    assert_eq!(o.out, "");
}

#[test]
fn init_force_archives_the_previous_goal() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    zloop(d, &["init", "first goal"], None, &[]);
    zloop(d, &["plan", "--add", "[P0] a"], None, &[]);
    zloop(d, &["done", "t1", "--note", "x", "--no-doc"], None, &[]);
    let o = zloop(d, &["init", "--force", "second goal"], None, &[]);
    assert_eq!(o.code, 0, "{}", o.err);
    assert!(o.out.contains("archived previous state → "), "{}", o.out);
    let archive = d.join(".zloop").join("archive");
    let files: Vec<_> = fs::read_dir(&archive).unwrap().flatten().collect();
    assert_eq!(files.len(), 1);
    let old: serde_json::Value = serde_json::from_str(&fs::read_to_string(files[0].path()).unwrap()).unwrap();
    assert_eq!(old["goal"]["text"], "first goal");
    assert_eq!(old["ticks"].as_array().unwrap().len(), 1);
    let st = state::load(&state::state_path(d)).unwrap();
    assert_eq!(st.goal.text, "second goal");
    assert!(st.todos.is_empty() && st.ticks.is_empty());
    // logs from the first goal are untouched
    assert!(!zloop::log::entries(d, None, 10).unwrap().is_empty());
}

#[test]
fn stale_in_progress_is_flagged() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    zloop(d, &["init", "g"], None, &[]);
    zloop(d, &["plan", "--add", "[P0] a"], None, &[]);
    zloop(d, &["next"], None, &[]);
    let p = state::state_path(d);
    let mut st = state::load(&p).unwrap();
    st.in_progress.as_mut().unwrap().started_at = "2026-08-27T00:00:00+08:00".into();
    state::save(&p, &mut st).unwrap();
    assert!(zloop(d, &["context"], None, &[]).out.contains("⚠ stale (>120m"));
    let o = zloop(d, &["status"], None, &[]);
    assert!(o.out.contains("⚠ 超过 120m 没动静"), "{}", o.out);
}

#[test]
fn plan_from_loopx_state_file() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    zloop(d, &["init", "g"], None, &[]);
    let f = d.join("ACTIVE_GOAL_STATE.md");
    fs::write(&f, "## Agent Todo\n\n- [x] [P1] done one\n- [ ] [P0] open one <!-- loopx:todo x=y -->\n").unwrap();
    let o = zloop(d, &["plan", "--from-loopx", f.to_str().unwrap()], None, &[]);
    assert_eq!(o.code, 0, "{}", o.err);
    assert_eq!(o.out.trim(), "t1 [P0] open one");
}

#[test]
fn phase_tracks_the_round() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    zloop(d, &["init", "g"], None, &[]);
    zloop(d, &["plan", "--add", "[P0] a", "--add", "[P1] b"], None, &[]);
    // 完整的 phase 句子是 `zloop context` 的契约；status 只显示压缩版，改样式不该动契约。
    assert!(zloop(d, &["context"], None, &[]).out.contains("阶段：idle · next would run t1"));
    assert!(zloop(d, &["status"], None, &[]).out.contains("就绪"));
    zloop(d, &["next", "--peek"], None, &[]);
    assert!(zloop(d, &["context"], None, &[]).out.contains("阶段：idle · next would run"));
    let o = zloop(d, &["next", "--json"], None, &[("CLAUDE_CODE_SESSION_ID", "sess-p")]);
    let v: serde_json::Value = serde_json::from_str(&o.out).unwrap();
    assert!(v["phase"].as_str().unwrap().starts_with("executing t1 · round 1"), "{}", v["phase"]);
    assert!(v.as_object().unwrap().len() <= 10);
    let st = state::load(&state::state_path(d)).unwrap();
    let ip = st.in_progress.as_ref().unwrap();
    assert_eq!((ip.todo.as_str(), ip.via.as_str(), ip.host.as_deref(), ip.session.as_deref()), ("t1", "next", Some("claude"), Some("sess-p")));
    let o = zloop(d, &["status"], None, &[]);
    assert!(o.out.contains("执行中") && o.out.contains("claude 正在做 t1") && o.out.contains("第 1 轮"), "{}", o.out);
    assert!(zloop(d, &["context"], None, &[]).out.contains("阶段：executing t1"));
    assert!(zloop(d, &["context"], None, &[]).out.contains("host claude · via next"));
    zloop(d, &["done", "t1", "--note", "ok", "--no-doc"], None, &[]);
    assert!(state::load(&state::state_path(d)).unwrap().in_progress.is_none());
    assert!(zloop(d, &["context"], None, &[]).out.contains("阶段：idle · next would run t2"));
    zloop(d, &["done", "t2", "--block", "?"], None, &[]);
    assert!(zloop(d, &["context"], None, &[]).out.contains("阶段：waiting (user_gate) · retry in 10 min"));
    assert!(zloop(d, &["status"], None, &[]).out.contains("等你回答 · 10 分钟后重试"));
    for _ in 0..3 { zloop(d, &["next"], None, &[]); }
    assert!(zloop(d, &["context"], None, &[]).out.contains("阶段：stopped (user_gate)"));
    assert!(zloop(d, &["status"], None, &[]).out.contains("等你决定"));
    let j = d.join(".zloop").join("runner");
    fs::create_dir_all(&j).unwrap();
    let until = (chrono::Local::now() + chrono::Duration::minutes(5)).to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
    fs::write(j.join("journal.jsonl"), format!("{{\"event\":\"sleep\",\"until\":\"{until}\",\"reason\":\"ready\",\"at\":\"x\"}}\n")).unwrap();
    assert!(zloop(d, &["context"], None, &[]).out.contains("runner sleeping until"));
    // 此刻所有 todo 都在等人回话，所以标题让位给「等你决定」，休眠时间退到明细行。
    let o = zloop(d, &["status"], None, &[]);
    assert!(o.out.contains("等你决定") && o.out.contains("睡到"), "{}", o.out);
    zloop(d, &["edit", "t2", "--status", "open"], None, &[]);
    assert!(zloop(d, &["status"], None, &[]).out.contains("休眠中"), "有活可干时才轮到休眠当标题");
    fs::write(j.join("journal.jsonl"), "{\"event\":\"begin\",\"round\":4,\"todo\":\"t2\",\"host\":\"claude\",\"at\":\"2026-08-27T00:00:00+08:00\"}\n").unwrap();
    assert!(zloop(d, &["context"], None, &[]).out.contains("runner round 4 on t2"));
    assert!(zloop(d, &["status"], None, &[]).out.contains("第 4 轮做 t2"), "{}", zloop(d, &["status"], None, &[]).out);
}

#[test]
fn hook_stop_passes_through_under_runner() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    zloop(d, &["init", "g"], None, &[]);
    zloop(d, &["plan", "--add", "[P0] a"], None, &[]);
    let o = zloop(d, &["hook-stop"], Some("{}"), &[("ZLOOP_RUNNER", "1")]);
    assert_eq!((o.code, o.out.as_str()), (0, ""));
    let o = zloop(d, &["hook-stop"], Some("{}"), &[]);
    assert!(o.out.contains("\"block\""));
}

#[test]
fn acceptance_shows_up_and_done_without_evidence_hints() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    zloop(d, &["init", "g"], None, &[]);
    let o = zloop(d, &["plan", "--add", "[P0] ship :: tests green"], None, &[]);
    assert_eq!(o.out.trim(), "t1 [P0] ship :: tests green");
    let o = zloop(d, &["next", "--json"], None, &[]);
    let v: serde_json::Value = serde_json::from_str(&o.out).unwrap();
    assert_eq!(v["todo"]["acceptance"], "tests green");
    assert!(zloop(d, &["status"], None, &[]).out.contains("验收：tests green"));
    assert!(zloop(d, &["context"], None, &[]).out.contains("验收：tests green"));
    let o = zloop(d, &["done", "t1", "--note", "ok", "--no-doc"], None, &[]);
    assert!(o.out.contains("hint: t1 有验收标准"), "{}", o.out);
    zloop(d, &["plan", "--add", "[P0] b"], None, &[]);
    zloop(d, &["edit", "t2", "--acceptance", "lint passes"], None, &[]);
    let o = zloop(d, &["done", "t2", "--note", "ok", "--evidence", "lint output clean", "--no-doc"], None, &[]);
    assert!(!o.out.contains("有验收标准"), "evidence given → no acceptance hint: {}", o.out);
    let logs = zloop::log::entries(d, Some("t2"), 5).unwrap();
    assert!(fs::read_to_string(&logs[0]).unwrap().contains("- acceptance: lint passes"));
}

#[test]
fn status_shows_spend_and_notify_cmd_receives_events() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    zloop(d, &["init", "g"], None, &[]);
    zloop(d, &["plan", "--add", "[P0] a"], None, &[]);
    let o = zloop(d, &["notify"], None, &[]);
    assert_eq!(o.code, 2, "nothing configured yet");
    zloop(d, &["done", "t1", "--outcome", "progress", "--note", "x"], None, &[]);
    let p = state::state_path(d);
    let mut st = state::load(&p).unwrap();
    st.ticks[0].cost_usd = Some(0.25);
    st.policy.max_total_usd = 2.0;
    st.policy.notify_cmd = Some(format!("cat >> {}", d.join("notify.log").display()));
    state::save(&p, &mut st).unwrap();
    let o = zloop(d, &["status"], None, &[]);
    assert!(o.out.contains("花了 $0.25（上限 $2.00）"), "{}", o.out);
    assert!(zloop(d, &["context"], None, &[]).out.contains("已花费：$0.25 / 上限 $2.00"));
    let o = zloop(d, &["notify", "hello there"], None, &[]);
    assert_eq!(o.code, 0, "{}", o.err);
    let log = fs::read_to_string(d.join("notify.log")).unwrap();
    assert!(log.contains("hello there") && log.contains("\"event\":\"test\""), "{log}");
}

#[test]
fn remember_pause_resume_and_compact() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    zloop(d, &["init", "g"], None, &[]);
    zloop(d, &["plan", "--add", "[P0] a", "--add", "[P1] b"], None, &[]);
    // remember → NOTES.md → context
    let o = zloop(d, &["remember", "run cargo test before done; the fmt check is flaky"], None, &[]);
    assert_eq!(o.code, 0, "{}", o.err);
    assert!(d.join(".zloop/NOTES.md").exists());
    let o = zloop(d, &["context"], None, &[]);
    assert!(o.out.contains("## 经验") && o.out.contains("fmt check is flaky"), "{}", o.out);
    // pause / resume
    let o = zloop(d, &["pause"], None, &[]);
    assert!(o.out.contains("paused"));
    assert!(zloop(d, &["context"], None, &[]).out.contains("阶段：stopped (paused)"));
    let o = zloop(d, &["status"], None, &[]);
    assert!(o.out.contains("已暂停") && o.out.contains("zloop resume"), "{}", o.out);
    let o = zloop(d, &["resume"], None, &[]);
    assert!(o.out.contains("active"));
    assert!(zloop(d, &["context"], None, &[]).out.contains("阶段：idle · next would run"));
    // compact: nothing old yet
    let o = zloop(d, &["compact"], None, &[]);
    assert!(o.out.contains("nothing to compact"));
    zloop(d, &["done", "t1", "--note", "old work", "--no-doc"], None, &[]);
    let p = state::state_path(d);
    let mut st = state::load(&p).unwrap();
    st.todos[0].done_at = Some("2026-01-01T00:00:00+08:00".into());
    st.ticks[0].at = "2026-01-01T00:00:00+08:00".into();
    state::save(&p, &mut st).unwrap();
    let o = zloop(d, &["compact", "--keep-days", "30"], None, &[]);
    assert!(o.out.contains("compacted 1 todos and 1 ticks"), "{}", o.out);
    let st = state::load(&p).unwrap();
    assert_eq!(st.todos.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), ["t2"]);
    assert!(st.ticks.is_empty());
    let archives: Vec<_> = fs::read_dir(d.join(".zloop/archive")).unwrap().flatten().collect();
    assert_eq!(archives.len(), 1);
    let a: serde_json::Value = serde_json::from_str(&fs::read_to_string(archives[0].path()).unwrap()).unwrap();
    assert_eq!(a["todos"][0]["id"], "t1");
    // t2 still runnable; the goal stays active
    assert!(zloop(d, &["next", "--peek", "--json"], None, &[]).out.contains("\"id\": \"t2\""));
}

// ---------- 每轮技术文档 ----------

#[test]
fn done_refuses_to_finish_without_a_technical_document() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    zloop(d, &["init", "g"], None, &[]);
    zloop(d, &["plan", "--add", "[P0] a", "--add", "[P1] b", "--add", "[P2] c"], None, &[]);

    // finishing without --approach is rejected, and nothing is written
    let o = zloop(d, &["done", "t1", "--note", "ok"], None, &[]);
    assert_eq!(o.code, 2, "{}{}", o.out, o.err);
    assert!(o.err.contains("需要留下技术文档"), "{}", o.err);
    assert!(o.err.contains("--approach") && o.err.contains("--pitfall") && o.err.contains("--no-doc"), "{}", o.err);
    let st = state::load(&state::state_path(d)).unwrap();
    assert_eq!(st.todos[0].status, "open", "rejected call must not change state");
    assert!(st.ticks.is_empty());

    // progress / fail / block are exempt — a round that did not finish cannot document a finished approach
    let o = zloop(d, &["done", "t1", "--outcome", "progress", "--note", "half"], None, &[]);
    assert_eq!(o.code, 0, "{}", o.err);
    let o = zloop(d, &["done", "t1", "--outcome", "fail", "--note", "boom"], None, &[]);
    assert_eq!(o.code, 0, "{}", o.err);
    let o = zloop(d, &["done", "t3", "--block", "which db?"], None, &[]);
    assert_eq!(o.code, 0, "{}", o.err);

    // with the document it goes through and every section is rendered
    let o = zloop(
        d,
        &["done", "t1", "--note", "done at last",
          "--approach", "先量基线再改：bench.sh 跑 3 次取中位数，只对最慢的一步做懒加载",
          "--decision", "不引入缓存层，成本高于收益",
          "--decision", "懒加载放在入口而不是每个 use 处",
          "--pitfall", "release 与 debug 差 3 倍，基线必须用 release",
          "--evidence", "cargo test 64 passed"],
        None,
        &[],
    );
    assert_eq!(o.code, 0, "{}", o.err);
    assert!(!o.out.contains("hint: 这一轮没有实现思路"), "{}", o.out);
    let st = state::load(&state::state_path(d)).unwrap();
    let last = st.ticks.last().unwrap();
    assert_eq!((last.outcome.as_str(), last.documented), ("done", Some(true)));
    let body = fs::read_to_string(d.join(".zloop").join(last.log.as_deref().unwrap())).unwrap();
    assert!(body.contains("## 实现思路") && body.contains("bench.sh 跑 3 次取中位数"), "{body}");
    assert!(body.contains("## 关键决策") && body.contains("- 不引入缓存层") && body.contains("- 懒加载放在入口"), "{body}");
    assert!(body.contains("## 遇到的坑") && body.contains("release 与 debug 差 3 倍"), "{body}");
    assert!(body.contains("## 验证证据") && body.contains("cargo test 64 passed"), "{body}");
    assert!(!body.contains("⚠ 这一轮没有留下实现思路"), "{body}");
}

#[test]
fn no_doc_escape_hatch_is_marked_everywhere() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    zloop(d, &["init", "g"], None, &[]);
    zloop(d, &["plan", "--add", "[P0] a", "--add", "[P1] b"], None, &[]);
    let o = zloop(d, &["done", "t1", "--note", "trivial", "--no-doc"], None, &[]);
    assert_eq!(o.code, 0, "{}", o.err);
    assert!(o.out.contains("hint: 这一轮没有实现思路"), "{}", o.out);
    let st = state::load(&state::state_path(d)).unwrap();
    assert_eq!(st.ticks.last().unwrap().documented, Some(false));
    let body = fs::read_to_string(d.join(".zloop").join(st.ticks.last().unwrap().log.as_deref().unwrap())).unwrap();
    assert!(body.contains("⚠ 这一轮没有留下实现思路"), "{body}");
    let o = zloop(d, &["log"], None, &[]);
    assert!(o.out.contains("⚠ .zloop/log/"), "{}", o.out);
    assert!(o.out.contains("只有结果记录"), "{}", o.out);
    let o = zloop(d, &["status"], None, &[]);
    assert!(o.out.contains("1 轮缺实现思路"), "{}", o.out);
    // policy off → plain done works again
    let p = state::state_path(d);
    let mut st = state::load(&p).unwrap();
    st.policy.require_doc = false;
    state::save(&p, &mut st).unwrap();
    assert_eq!(zloop(d, &["done", "t2", "--note", "no policy"], None, &[]).code, 0);
}

#[test]
fn doc_assembles_rounds_into_one_document() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    zloop(d, &["init", "把启动时间降到 1 秒"], None, &[]);
    zloop(d, &["plan", "--add", "[P0] 量基线 :: bench.sh 连跑 3 次", "--add", "[P1] 懒加载"], None, &[]);
    zloop(d, &["done", "t1", "--outcome", "progress", "--note", "第一步", "--approach", "先写 bench.sh"], None, &[("CLAUDE_CODE_SESSION_ID", "sess-doc")]);
    zloop(d, &["done", "t1", "--note", "基线 3.2s", "--approach", "取中位数避免抖动", "--pitfall", "debug 模式差 3 倍"], None, &[("CLAUDE_CODE_SESSION_ID", "sess-doc")]);
    zloop(d, &["done", "t2", "--note", "懒加载完成", "--no-doc"], None, &[]);

    let o = zloop(d, &["doc", "t1"], None, &[]);
    assert_eq!(o.code, 0, "{}", o.err);
    assert!(o.out.starts_with("# 技术文档 · "), "{}", o.out);
    assert!(o.out.contains("**目标**：把启动时间降到 1 秒"));
    assert!(o.out.contains("## t1 [P0] 量基线"));
    assert!(o.out.contains("- 验收标准：bench.sh 连跑 3 次"));
    assert_eq!(o.out.matches("### 轮次").count(), 2, "both rounds of t1: {}", o.out);
    assert!(o.out.contains("#### 实现思路"), "sections demoted under the round: {}", o.out);
    assert!(o.out.contains("取中位数避免抖动") && o.out.contains("debug 模式差 3 倍"));
    assert!(o.out.contains("claude --resume sess-doc"), "resume command carried over: {}", o.out);
    assert!(!o.out.contains("## t2 "), "only the requested todo");

    // --all covers every todo and flags the undocumented round
    let out_file = d.join("docs").join("TECH.md");
    let o = zloop(d, &["doc", "--all", "--out", out_file.to_str().unwrap()], None, &[]);
    assert_eq!(o.code, 0, "{}", o.err);
    assert!(o.out.contains("wrote ") && o.out.contains("2 条 todo"), "{}", o.out);
    let text = fs::read_to_string(&out_file).unwrap();
    assert!(text.contains("## t1 [P0]") && text.contains("## t2 [P1]"));
    assert!(text.contains("（这一轮没有实现思路，只有结果记录）"), "{text}");

    assert_eq!(zloop(d, &["doc", "t99"], None, &[]).code, 2);
    assert_eq!(zloop(d, &["doc"], None, &[]).code, 2);
}

#[test]
fn changed_files_are_captured_from_git() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let git = |args: &[&str]| Command::new("git").args(args).current_dir(d).output().unwrap();
    git(&["init", "-q", "-b", "main"]);
    git(&["config", "user.email", "t@example.com"]);
    git(&["config", "user.name", "t"]);
    fs::write(d.join("keep.txt"), "one\n").unwrap();
    fs::write(d.join(".gitignore"), ".zloop/\n").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "init"]);

    zloop(d, &["init", "g"], None, &[]);
    zloop(d, &["plan", "--add", "[P0] a"], None, &[]);
    fs::write(d.join("keep.txt"), "one\ntwo\n").unwrap(); // modified
    fs::write(d.join("brand-new.rs"), "fn main() {}\n").unwrap(); // untracked
    let o = zloop(d, &["done", "t1", "--note", "touched files", "--approach", "改了两个文件"], None, &[]);
    assert_eq!(o.code, 0, "{}", o.err);
    let st = state::load(&state::state_path(d)).unwrap();
    let body = fs::read_to_string(d.join(".zloop").join(st.ticks[0].log.as_deref().unwrap())).unwrap();
    assert!(body.contains("## 改动文件"), "{body}");
    assert!(body.contains("keep.txt"), "modified file listed: {body}");
    assert!(body.contains("brand-new.rs (new)"), "untracked file listed: {body}");
    assert!(!body.contains(".zloop/"), "zloop's own state must not be listed: {body}");
}

// ---------- status 的观感 ----------

#[test]
fn status_headline_names_the_state_and_colour_is_opt_in() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    zloop(d, &["init", "把启动时间降到 1 秒"], None, &[]);
    zloop(d, &["plan", "--add", "[P0] a", "--add", "[P1] b"], None, &[]);

    // 就绪：有活可做
    let o = zloop(d, &["status"], None, &[]);
    assert!(o.out.contains("就绪"), "{}", o.out);
    assert!(o.out.contains("░"), "progress bar: {}", o.out);
    assert!(o.out.contains("目标") && o.out.contains("把启动时间降到 1 秒"), "目标单独一行: {}", o.out);
    // 步骤清单：编号 + 文本 + 右栏（id + 图标 + 状态词）
    assert!(o.out.contains("步骤") && o.out.contains("0/2 完成"), "步骤进度: {}", o.out);
    assert!(o.out.contains("1. a") && o.out.contains("2. b"), "每一步都编号列出: {}", o.out);
    assert!(o.out.contains("t1 ▶ 下一个") && o.out.contains("t2 ○ 排队中"), "每一步自己说清状态: {}", o.out);
    assert!(o.out.contains("开跑") && o.out.contains("zloop start"), "next action spelled out: {}", o.out);
    assert!(!o.out.contains('\u{1b}'), "piped output carries no escape codes: {:?}", o.out);

    // 不换行才是关键：折行会丢掉左边的槽位，那正是“乱”的来源。
    for cols in [46usize, 60, 80, 100] {
        let o = zloop(d, &["status"], None, &[("COLUMNS", &cols.to_string())]);
        for line in o.out.lines() {
            assert!(zloop::style::width(line) <= cols, "{cols} 列下这行超宽 ({}): {line:?}", zloop::style::width(line));
        }
    }

    // 管道无色，CLICOLOR_FORCE 有色，--no-color 强制无色
    let forced = zloop(d, &["status"], None, &[("CLICOLOR_FORCE", "1")]);
    assert!(forced.out.contains('\u{1b}'), "CLICOLOR_FORCE=1 colourises: {:?}", forced.out);
    let off = zloop(d, &["status", "--no-color"], None, &[("CLICOLOR_FORCE", "1")]);
    assert!(!off.out.contains('\u{1b}'), "--no-color wins: {:?}", off.out);
    let no_color_env = zloop(d, &["status"], None, &[("CLICOLOR_FORCE", "1"), ("NO_COLOR", "1")]);
    assert!(!no_color_env.out.contains('\u{1b}'), "NO_COLOR wins: {:?}", no_color_env.out);

    // 等你决定
    zloop(d, &["done", "t1", "--block", "用哪个库？"], None, &[]);
    zloop(d, &["done", "t2", "--block", "要上线吗？"], None, &[]);
    let o = zloop(d, &["status"], None, &[]);
    assert!(o.out.contains("等你决定"), "{}", o.out);
    assert!(o.out.contains("↳ 用哪个库？"), "the blocking question is shown inline: {}", o.out);
    assert!(o.out.contains("等你回话") && o.out.contains("答完敲 zloop edit t1 --status open"), "解锁命令贴在那条 todo 自己下面: {}", o.out);
    assert!(o.out.contains("答完敲 zloop edit t2 --status open"), "每条被挡住的都有自己的命令: {}", o.out);
    // 被 --block 的轮次不欠文档
    assert!(!o.out.contains("只有结果记录"), "block rounds owe no document: {}", o.out);

    // 已暂停
    zloop(d, &["pause"], None, &[]);
    assert!(zloop(d, &["status"], None, &[]).out.contains("已暂停"));
    zloop(d, &["resume"], None, &[]);

    // 完成
    zloop(d, &["edit", "t1", "--status", "open"], None, &[]);
    zloop(d, &["edit", "t2", "--status", "open"], None, &[]);
    zloop(d, &["done", "t1", "--note", "x", "--approach", "怎么做的"], None, &[]);
    zloop(d, &["done", "t2", "--note", "y", "--approach", "怎么做的"], None, &[]);
    let o = zloop(d, &["status"], None, &[]);
    assert!(o.out.contains("完成"), "{}", o.out);
    assert!(o.out.contains("2/2 完成") && o.out.contains("100%"), "{}", o.out);
    // 做完的步骤要留在清单上打勾——「做过哪几步」是复盘时最想看的
    assert_eq!(o.out.matches('✅').count(), 3, "标题一个 ✅ + 两步各一个: {}", o.out);
    assert!(o.out.contains("1. a") && o.out.contains("2. b"), "完成后清单还在: {}", o.out);
    // 换目标走 goal new（停放旧的、可切回），不再是 init --force（归档、切不回来）
    assert!(o.out.contains("zloop plan --add") && o.out.contains("zloop goal new"), "what to do next: {}", o.out);
    assert!(!o.out.contains("init --force"), "别再教用户覆盖目标: {}", o.out);
    assert!(o.out.contains("zloop doc --all"), "and how to collect the documents: {}", o.out);
    assert!(!o.out.contains('░'), "a finished bar is entirely full: {}", o.out);
}

// ---------- 多目标 ----------

#[test]
fn goals_park_switch_and_archive_without_losing_anything() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    zloop(d, &["init", "把冷启动降到 1 秒"], None, &[]);
    zloop(d, &["plan", "--add", "[P0] 找最慢的三处", "--add", "[P1] 加 tracing"], None, &[]);
    zloop(d, &["done", "t1", "--note", "定位到 3 处", "--approach", "tracing 打点"], None, &[]);

    // 新目标：旧的原地停放，不是覆盖
    let o = zloop(d, &["goal", "new", "让 keep-awake 支持外接显示器"], None, &[]);
    assert_eq!(o.code, 0, "{}{}", o.out, o.err);
    assert!(o.out.contains("停放") && o.out.contains("zloop goal switch"), "{}", o.out);
    assert!(o.out.contains("新目标"), "{}", o.out);
    let st = state::load(&state::state_path(d)).unwrap();
    assert_eq!((st.goal.text.as_str(), st.todos.len()), ("让 keep-awake 支持外接显示器", 0), "新目标是干净的");
    assert_eq!(st.goal.id, "keep-awake", "id 从目标文字里的英文词取: {}", st.goal.id);

    // 两个都在，当前那个带 ▸
    let o = zloop(d, &["goal", "list"], None, &[]);
    assert!(o.out.contains("共 2 个目标"), "{}", o.out);
    assert!(o.out.contains("▸ keep-awake") && o.out.contains("让 keep-awake 支持外接显示器"), "{}", o.out);
    assert!(o.out.contains("把冷启动降到 1 秒"), "停放的也列出来: {}", o.out);
    // status 里能看见还有别的目标
    assert!(zloop(d, &["status"], None, &[]).out.contains("另有 1 个目标停着"), "{}", o.out);
    // 空目标不说「全部完成」
    let o = zloop(d, &["status"], None, &[]);
    assert!(o.out.contains("待规划") && o.out.contains("还没有待办"), "{}", o.out);

    // 用目标文字的片段切回去，进度一条不少
    let o = zloop(d, &["goal", "switch", "冷启动"], None, &[]);
    assert_eq!(o.code, 0, "{}{}", o.out, o.err);
    let st = state::load(&state::state_path(d)).unwrap();
    assert_eq!(st.goal.text, "把冷启动降到 1 秒");
    assert_eq!(st.todos.len(), 2);
    assert_eq!(st.todos[0].status, "done");
    assert_eq!(st.ticks.len(), 1, "tick 账本跟着目标走");
    assert!(zloop(d, &["status"], None, &[]).out.contains("1/2 完成"), "步骤进度还在");

    // 归档：从 list 里消失，文件搬到 archive/
    let o = zloop(d, &["goal", "rm", "keep-awake"], None, &[]);
    assert_eq!(o.code, 0, "{}{}", o.out, o.err);
    assert!(o.out.contains("已归档"), "{}", o.out);
    assert!(!zloop(d, &["goal", "list"], None, &[]).out.contains("keep-awake"));
    let archived: Vec<_> = fs::read_dir(d.join(".zloop/archive")).unwrap().flatten().collect();
    assert_eq!(archived.len(), 1, "归档只是搬家，不是删除");
    // 当前目标不能被归档
    let o = zloop(d, &["goal", "rm", "冷启动"], None, &[]);
    assert_eq!(o.code, 2);
    assert!(o.err.contains("是当前目标"), "{}", o.err);
}

#[test]
fn switching_goals_is_refused_while_work_is_in_flight() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    zloop(d, &["init", "目标一"], None, &[]);
    zloop(d, &["plan", "--add", "[P0] a"], None, &[]);
    zloop(d, &["goal", "new", "目标二"], None, &[]);
    zloop(d, &["goal", "switch", "目标一"], None, &[]);

    // 有会话拿着 todo 没写回
    zloop(d, &["next"], None, &[]);
    let o = zloop(d, &["goal", "switch", "目标二"], None, &[]);
    assert_eq!(o.code, 2, "{}{}", o.out, o.err);
    assert!(o.err.contains("还没写回"), "{}", o.err);
    // --force 才放行
    assert_eq!(zloop(d, &["goal", "switch", "目标二", "--force"], None, &[]).code, 0);
    zloop(d, &["goal", "switch", "目标一", "--force"], None, &[]);

    // runner 在跑（pid 文件指向一个活着的进程）
    zloop(d, &["done", "t1", "--note", "ok", "--no-doc"], None, &[]);
    fs::create_dir_all(d.join(".zloop/runner")).unwrap();
    fs::write(d.join(".zloop/runner/pid"), format!("{}\n", std::process::id())).unwrap();
    let o = zloop(d, &["goal", "switch", "目标二"], None, &[]);
    assert_eq!(o.code, 2, "{}{}", o.out, o.err);
    assert!(o.err.contains("runner 正在跑"), "{}", o.err);
    fs::remove_file(d.join(".zloop/runner/pid")).unwrap();

    // 片段对上多个目标时要求说清楚
    zloop(d, &["goal", "new", "目标三"], None, &[]);
    let o = zloop(d, &["goal", "switch", "目标"], None, &[]);
    assert_eq!(o.code, 2);
    assert!(o.err.contains("对上了 3 个目标"), "{}", o.err);
}
