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

/// 空清单 ≠ 全部完成：前者是"还没规划"（去 plan），后者是"活干完了"（去开新目标）。
/// 共用 `all_done` 时读的人会照"已完成"那一支走，把刚建的目标 goal new 成重名的（#5）。
#[test]
fn empty_plan_says_unplanned_not_all_done() {
    let st = fresh(&[]);
    let d = tick::decide(&st, now_utc());
    assert_eq!((d.should_run, d.reason.as_str(), d.interval_min), (false, "unplanned", None));

    // 有过 todo、只是都了结了：还是 all_done，这条路不受影响
    let mut st = fresh(&["[P0] a"]);
    done(&mut st, "t1");
    st.goal.status = "active".into(); // apply_done 会顺手把目标标成 done，这里只看清单那一层
    assert_eq!(tick::decide(&st, now_utc()).reason, "all_done");
}

#[test]
fn all_done_marks_goal_done() {
    let mut st = fresh(&["[P0] a"]);
    done(&mut st, "t1");
    assert_eq!(st.goal.status, "done");
    let d = tick::decide(&st, now_utc());
    assert!(!d.should_run && d.reason == "done");
}

/// 全部延后 ≠ 全部完成（B-3）：`is_terminal` 把 done 和 deferred 一视同仁，于是"一条都没做完、
/// 全推到了以后"在调度器眼里和"活干完了"长得一样。出口动作是相反的——一个该把延后的捡回来，
/// 一个才该开新目标——所以它得有自己的 reason。
#[test]
fn all_deferred_is_not_all_done() {
    let mut st = fresh(&["[P0] a", "[P0] b"]);
    todo::set_status(&mut st, "t1", "deferred", None).unwrap();
    todo::set_status(&mut st, "t2", "deferred", None).unwrap();
    let d = tick::decide(&st, now_utc());
    assert_eq!((d.should_run, d.reason.as_str()), (false, "all_deferred"), "全部延后要有自己的 reason");

    // 只要有一条真做完了，就还是 all_done —— 这条路不受影响
    let mut st = fresh(&["[P0] a", "[P0] b"]);
    todo::set_status(&mut st, "t2", "deferred", None).unwrap();
    done(&mut st, "t1");
    st.goal.status = "active".into(); // apply_done 会顺手标 done，这里只看清单那一层
    assert_eq!(tick::decide(&st, now_utc()).reason, "all_done");
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
fn progress_streak_on_one_todo_stops_the_loop() {
    let mut st = fresh(&["[P0] a", "[P1] b"]);
    st.policy.max_progress_streak = 3;
    for _ in 0..2 {
        outcome(&mut st, "t1", "progress", "still going");
    }
    tick_at(&mut st, "noop", None, None); // noop is transparent
    assert!(tick::decide(&st, now_utc()).should_run);
    outcome(&mut st, "t1", "progress", "still going");
    let d = tick::decide(&st, now_utc());
    assert_eq!((d.should_run, d.reason.as_str(), d.interval_min), (false, "progress_streak", None));
    // progress on a different todo breaks the streak
    outcome(&mut st, "t2", "progress", "other");
    assert!(tick::decide(&st, now_utc()).should_run);
    // disabled with 0
    st.policy.max_progress_streak = 0;
    for _ in 0..5 {
        outcome(&mut st, "t1", "progress", "x");
    }
    assert!(tick::decide(&st, now_utc()).should_run);
}

#[test]
fn max_runs_zero_disables_the_window_brake() {
    let mut st = fresh(&["[P0] a"]);
    st.policy.max_runs = 0;
    for _ in 0..50 {
        outcome(&mut st, "t1", "progress", "x");
        st.policy.max_progress_streak = 0;
    }
    assert!(tick::decide(&st, now_utc()).should_run);
    assert_eq!(zloop::state::Policy::default().max_runs, 480);
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

#[test]
fn budget_cap_stops_when_spent_reaches_max_total_usd() {
    let mut st = fresh(&["[P0] a", "[P1] b"]);
    st.policy.max_total_usd = 1.0;
    outcome(&mut st, "t1", "progress", "x");
    st.ticks.last_mut().unwrap().cost_usd = Some(0.6);
    assert!(tick::decide(&st, now_utc()).should_run);
    outcome(&mut st, "t1", "progress", "y");
    st.ticks.last_mut().unwrap().cost_usd = Some(0.45);
    let d = tick::decide(&st, now_utc());
    assert_eq!((d.should_run, d.reason.as_str(), d.interval_min), (false, "budget", None));
    assert!((tick::spent_usd(&st.ticks) - 1.05).abs() < 1e-9);
    st.policy.max_total_usd = 0.0;
    assert!(tick::decide(&st, now_utc()).should_run);
}
