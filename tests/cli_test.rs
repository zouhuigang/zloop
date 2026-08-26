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

    let o = zloop(d, &["done", "t1", "--note", "DESIGN.md written", "--next", "review design", "--evidence", "line1\nline2"], None, &[]);
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
    assert!(o.out.contains("goal (active)"));
    assert!(o.out.contains("round 1"));

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
    assert!(body.contains("## Evidence") && body.contains("line2"));
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
    let o = zloop(d, &["done", "t9"], None, &[]);
    assert_eq!(o.code, 2);
    assert!(o.err.contains("unknown todo id"));
    zloop(d, &["done", "t1"], None, &[]);
    let o = zloop(d, &["done", "t1"], None, &[]);
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
    let o = zloop(d, &["done", "t1", "--note", "x"], None, &[("CLAUDE_CODE_SESSION_ID", "11111111-2222-3333-4444-555555555555")]);
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
    zloop(d, &["done", "t1", "--note", "done first"], None, &[("CLAUDE_CODE_SESSION_ID", "sess-1")]);
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
    zloop(d, &["done", "t1"], None, &[]);
    let o = zloop(d, &["hook-stop"], Some("{}"), &[]);
    assert_eq!(o.code, 0);
    assert_eq!(o.out, "");
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
    let o = zloop(d, &["status"], None, &[]);
    assert!(o.out.contains("phase: idle · next would run t1"), "{}", o.out);
    zloop(d, &["next", "--peek"], None, &[]);
    assert!(zloop(d, &["status"], None, &[]).out.contains("phase: idle"));
    let o = zloop(d, &["next", "--json"], None, &[("CLAUDE_CODE_SESSION_ID", "sess-p")]);
    let v: serde_json::Value = serde_json::from_str(&o.out).unwrap();
    assert!(v["phase"].as_str().unwrap().starts_with("executing t1 · round 1"), "{}", v["phase"]);
    assert!(v.as_object().unwrap().len() <= 10);
    let st = state::load(&state::state_path(d)).unwrap();
    let ip = st.in_progress.as_ref().unwrap();
    assert_eq!((ip.todo.as_str(), ip.via.as_str(), ip.host.as_deref(), ip.session.as_deref()), ("t1", "next", Some("claude"), Some("sess-p")));
    let o = zloop(d, &["status"], None, &[]);
    assert!(o.out.contains("phase: executing t1") && o.out.contains("host claude · via next"), "{}", o.out);
    assert!(zloop(d, &["context"], None, &[]).out.contains("阶段：executing t1"));
    zloop(d, &["done", "t1", "--note", "ok"], None, &[]);
    assert!(state::load(&state::state_path(d)).unwrap().in_progress.is_none());
    assert!(zloop(d, &["status"], None, &[]).out.contains("phase: idle · next would run t2"));
    zloop(d, &["done", "t2", "--block", "?"], None, &[]);
    assert!(zloop(d, &["status"], None, &[]).out.contains("phase: waiting (user_gate) · retry in 10 min"));
    for _ in 0..3 { zloop(d, &["next"], None, &[]); }
    assert!(zloop(d, &["status"], None, &[]).out.contains("phase: stopped (user_gate)"));
    let j = d.join(".zloop").join("runner");
    fs::create_dir_all(&j).unwrap();
    let until = (chrono::Local::now() + chrono::Duration::minutes(5)).to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
    fs::write(j.join("journal.jsonl"), format!("{{\"event\":\"sleep\",\"until\":\"{until}\",\"reason\":\"ready\",\"at\":\"x\"}}\n")).unwrap();
    assert!(zloop(d, &["status"], None, &[]).out.contains("phase: runner sleeping until"));
    fs::write(j.join("journal.jsonl"), "{\"event\":\"begin\",\"round\":4,\"todo\":\"t2\",\"host\":\"claude\",\"at\":\"2026-08-27T00:00:00+08:00\"}\n").unwrap();
    assert!(zloop(d, &["status"], None, &[]).out.contains("runner round 4 on t2"));
}
