mod common;

use chrono::Duration;
use common::*;
use zloop::tick;
use zloop::todo;

#[test]
fn ready_picks_highest_priority_then_write_order() {
    let st = fresh(&["[P1] b", "[P0] c", "[P0] a"]);
    let d = tick::decide(&st, now_utc());
    assert!(d.should_run && d.reason == "ready");
    assert_eq!(d.todo.unwrap().text, "c");
    assert_eq!(d.interval_min, Some(3));
}

#[test]
fn paused_goal_stops() {
    let mut st = fresh(&["[P0] a"]);
    st.goal.status = "paused".into();
    let d = tick::decide(&st, now_utc());
    assert_eq!((d.should_run, d.reason.as_str(), d.interval_min), (false, "paused", None));
}

#[test]
fn all_done_marks_goal_done() {
    let mut st = fresh(&["[P0] a"]);
    done(&mut st, "t1");
    assert_eq!(st.goal.status, "done");
    let d = tick::decide(&st, now_utc());
    assert!(!d.should_run && d.reason == "done");
}

#[test]
fn blocked_by_dependency_and_backoff_ladder() {
    let mut st = fresh(&["[P0] a", "[P0] b"]);
    st.todos[1].blocked_by = vec!["t1".into()];
    st.todos[0].status = "blocked".into();
    let d = tick::decide(&st, now_utc());
    assert_eq!((d.should_run, d.reason.as_str(), d.interval_min), (false, "blocked", Some(10)));
    tick_at(&mut st, "noop", None, None);
    assert_eq!(tick::decide(&st, now_utc()).interval_min, Some(30));
    tick_at(&mut st, "noop", None, None);
    assert_eq!(tick::decide(&st, now_utc()).interval_min, Some(30));
    tick_at(&mut st, "noop", None, None);
    assert_eq!(tick::decide(&st, now_utc()).interval_min, None);
}

#[test]
fn user_gate_when_waiting_on_human() {
    let mut st = fresh(&["[P0] a"]);
    tick::apply_done(&mut st, "t1", "done", "", Some("which db?"), None, &cli_who()).unwrap();
    let d = tick::decide(&st, now_utc());
    assert_eq!(d.reason, "user_gate");
    assert!(!d.should_run);
    assert_eq!(st.todos[0].status, "blocked");
    assert!(st.todos[0].blocked_by.contains(&"user".to_string()));
}

#[test]
fn dependency_satisfied_after_done() {
    let mut st = fresh(&["[P0] a", "[P0] b"]);
    st.todos[1].blocked_by = vec!["t1".into()];
    assert_eq!(tick::decide(&st, now_utc()).todo.unwrap().id, "t1");
    done(&mut st, "t1");
    assert_eq!(tick::decide(&st, now_utc()).todo.unwrap().id, "t2");
}

#[test]
fn fail_streak_stops_and_edit_resets() {
    let mut st = fresh(&["[P0] a"]);
    for _ in 0..3 {
        outcome(&mut st, "t1", "fail", "boom");
    }
    let d = tick::decide(&st, now_utc());
    assert_eq!((d.should_run, d.reason.as_str(), d.interval_min), (false, "fail_streak", None));
    tick_at(&mut st, "edit", Some("t1"), None);
    assert!(tick::decide(&st, now_utc()).should_run);
}

#[test]
fn noop_ticks_do_not_break_fail_streak() {
    let mut st = fresh(&["[P0] a"]);
    outcome(&mut st, "t1", "fail", "");
    tick_at(&mut st, "noop", None, None);
    outcome(&mut st, "t1", "fail", "");
    assert_eq!(tick::fail_streak(&st.ticks), 2);
}

#[test]
fn throttled_by_rolling_window() {
    let mut st = fresh(&["[P0] a"]);
    st.policy.max_runs = 2;
    let now = now_utc();
    tick_at(&mut st, "progress", Some("t1"), Some(now - Duration::hours(2)));
    tick_at(&mut st, "progress", Some("t1"), Some(now - Duration::hours(1)));
    let d = tick::decide(&st, now);
    assert_eq!(d.reason, "throttled");
    assert!(!d.should_run);
    assert_eq!(d.interval_min, Some(22 * 60 + 1));
    assert!(tick::decide(&st, now + Duration::hours(23)).should_run);
}

#[test]
fn progress_keeps_todo_open_and_counts_round() {
    let mut st = fresh(&["[P0] a"]);
    outcome(&mut st, "t1", "progress", "half");
    assert_eq!(st.todos[0].status, "open");
    assert_eq!(st.todos[0].note, "half");
    assert_eq!(tick::current_round(&st.ticks), 1);
}

#[test]
fn done_with_next_inserts_successor_after() {
    let mut st = fresh(&["[P0] a", "[P2] z"]);
    tick::apply_done(&mut st, "t1", "done", "ok", None, Some("[P1] follow-up"), &cli_who()).unwrap();
    let ids: Vec<&str> = st.todos.iter().map(|t| t.id.as_str()).collect();
    assert_eq!(ids, ["t1", "t3", "t2"]);
    assert_eq!(st.todos[1].priority, 1);
    assert_eq!(tick::decide(&st, now_utc()).todo.unwrap().id, "t3");
}

#[test]
fn done_twice_is_rejected() {
    let mut st = fresh(&["[P0] a"]);
    done(&mut st, "t1");
    let err = tick::apply_done(&mut st, "t1", "done", "", None, None, &cli_who()).unwrap_err();
    assert!(err.to_string().contains("already done"));
}

#[test]
fn to_json_has_at_most_ten_fields() {
    let st = fresh(&["[P0] a", "[P1] b"]);
    let payload = tick::to_json(&tick::decide(&st, now_utc()), &st);
    let obj = payload.as_object().unwrap();
    assert!(obj.len() <= 10);
    assert_eq!(payload["todo"]["id"], "t1");
    assert_eq!(payload["remaining"], 2);
    assert!(payload["writeback"].as_str().unwrap().starts_with("zloop done t1"));
}

#[test]
fn record_captures_host_and_session() {
    let mut st = fresh(&["[P0] a"]);
    let who = zloop::session::HostSession { host: zloop::session::Host::Claude, session: Some("abc".into()) };
    let t = tick::record(&mut st, "progress", Some("t1"), "x", &who).unwrap();
    assert_eq!(t.host.as_deref(), Some("claude"));
    assert_eq!(t.session.as_deref(), Some("abc"));
    assert_eq!(todo::remaining(&st), 1);
}
