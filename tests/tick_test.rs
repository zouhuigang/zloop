mod common;

use chrono::{DateTime, Duration};
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

/// A-10：`policy` 里五个阈值，「写 0 = 关掉这个检查」必须是同一个口径。
///
/// 修复前只有 `max_runs` / `max_total_usd` / `max_progress_streak` 三个有 `> 0` 守卫；
/// 另外两个写 0 得到的是**相反**的效果——照着前三个的先例关闸的人，把目标关死了：
/// - `max_fail_streak = 0`：`0 >= 0` 恒真，一次失败都没有的全新目标第一次 `next`
///   就是 `fail_streak` + `interval=None`（永久停机，而且账本上一条 fail 都没有）；
/// - `max_noop_streak = 0`：`exhausted` 恒真，`should_run` 不变所以看不出来，
///   但 `blocked` / `user_gate` 两支的 `interval_min` 从「10 分钟后再看」变成
///   `None`＝停下等人——无头 runner 就此不再自己醒来。
#[test]
fn zero_turns_a_threshold_off_the_same_way_for_all_five() {
    // 1. max_fail_streak = 0：一次失败都没有的全新目标照常派活
    let mut st = fresh(&["[P0] a"]);
    st.policy.max_fail_streak = 0;
    let d = tick::decide(&st, now_utc());
    assert_eq!((d.should_run, d.reason.as_str()), (true, "ready"), "0 该是「关掉」而不是「永远触发」");
    // 关掉就是真关掉：连着失败也不停（默认 3 早该停了）
    for _ in 0..5 {
        outcome(&mut st, "t1", "fail", "boom");
    }
    assert!(tick::decide(&st, now_utc()).should_run, "关掉了这道闸就不该再拦: {:?}", tick::decide(&st, now_utc()));
    // 而写正数照旧管用（别把闸修没了）
    st.policy.max_fail_streak = 3;
    assert_eq!(tick::decide(&st, now_utc()).reason, "fail_streak");

    // 2. max_noop_streak = 0：非终态出口照旧给间隔，不是「停下等人」
    let mut st = fresh(&["[P0] a", "[P0] b"]);
    st.todos[0].blocked_by = vec![todo::USER.into()];
    st.todos[1].blocked_by = vec![todo::USER.into()];
    let base = tick::decide(&st, now_utc());
    assert_eq!((base.reason.as_str(), base.interval_min), ("user_gate", Some(10)), "默认值下的样子");
    st.policy.max_noop_streak = 0;
    let d = tick::decide(&st, now_utc());
    assert_eq!((d.reason.as_str(), d.interval_min), ("user_gate", Some(10)), "0 = 关掉，间隔不该塌成 None");
    // 写正数时那条退避语义照旧：攒够 noop 就真的停下等人
    st.policy.max_noop_streak = 2;
    for _ in 0..2 {
        tick_at(&mut st, "noop", None, None);
    }
    assert_eq!(tick::decide(&st, now_utc()).interval_min, None, "攒够 noop 该停下等人");

    // 3. 另外三个原本就是这个口径，一起钉住，免得哪天被改歪
    let mut st = fresh(&["[P0] a"]);
    st.policy.max_runs = 0;
    st.policy.max_total_usd = 0.0;
    st.policy.max_progress_streak = 0;
    st.policy.max_fail_streak = 0;
    st.policy.max_noop_streak = 0;
    for _ in 0..20 {
        outcome(&mut st, "t1", "progress", "又推了一点");
    }
    let d = tick::decide(&st, now_utc());
    assert_eq!((d.should_run, d.reason.as_str()), (true, "ready"), "五个全关＝什么都不拦: {d:?}");
}

#[test]
fn noop_ticks_do_not_break_fail_streak() {
    let mut st = fresh(&["[P0] a"]);
    outcome(&mut st, "t1", "fail", "");
    tick_at(&mut st, "noop", None, None);
    outcome(&mut st, "t1", "fail", "");
    assert_eq!(tick::fail_streak(&st), 2);
}

/// A-17 的后半截：人在另一个终端补一句话，不该把还没停下的循环的保险丝拆掉。
///
/// `zloop feedback` 是文档教人「跟正在跑的循环说话」的那条路。它一插进两次 fail 中间，
/// 连续失败就永远数不到上限——无头 runner 一轮一轮地失败、一轮一轮地烧，谁都不叫停。
#[test]
fn feedback_mid_run_does_not_disarm_the_fail_brake() {
    let mut st = fresh(&["[P0] a"]);
    st.policy.max_fail_streak = 3;
    for i in 0..2 {
        outcome(&mut st, "t1", "fail", "boom");
        // 每次失败之后人都补一句——但循环还没停在人面前，这句话不是"失败被解决了"
        tick_at(&mut st, "feedback", Some("t1"), None);
        assert_eq!(tick::fail_streak(&st), i + 1, "第 {} 句反馈把失败计数清零了", i + 1);
        assert!(tick::decide(&st, now_utc()).should_run, "还没到上限，不该停");
    }
    outcome(&mut st, "t1", "fail", "boom");
    let d = tick::decide(&st, now_utc());
    assert_eq!((d.should_run, d.reason.as_str()), (false, "fail_streak"), "该停在连续失败上");

    // 停下之后人再开口，才是这道闸等的东西：放它再试（README 里那段实测的语义）
    tick_at(&mut st, "feedback", Some("t1"), None);
    assert_eq!(tick::fail_streak(&st), 0);
    assert!(tick::decide(&st, now_utc()).should_run, "停下来等人之后，人说话该让循环继续");
}

/// A-20：`edit` tick 全仓只有 `zloop edit` 记，也就是**人在另一个终端**敲的。
/// 顺手把 backlog 里另一条 todo 改个名，跟正在失败的这条活没有半点关系，
/// 不该把连续失败停机那道闸拆了（实测：runner 从 2 轮就停变成 20 秒都不停）。
#[test]
fn an_edit_on_another_todo_does_not_disarm_the_fail_brake() {
    let mut st = fresh(&["[P0] a", "[P1] 另一条不相干的活"]);
    st.policy.max_fail_streak = 3;
    for i in 0..2 {
        outcome(&mut st, "t1", "fail", "boom");
        tick_at(&mut st, "edit", Some("t2"), None); // 人在整理 backlog，改的是 t2
        assert_eq!(tick::fail_streak(&st), i + 1, "第 {} 次改别的 todo 把失败计数清零了", i + 1);
        assert!(tick::decide(&st, now_utc()).should_run, "还没到上限，不该停");
    }
    outcome(&mut st, "t1", "fail", "boom");
    let d = tick::decide(&st, now_utc());
    assert_eq!((d.should_run, d.reason.as_str()), (false, "fail_streak"), "该停在连续失败上");

    // 停下来等人之后，人改哪条 todo 都算回应（和 feedback 同一条规矩）
    tick_at(&mut st, "edit", Some("t2"), None);
    assert!(tick::decide(&st, now_utc()).should_run, "停下之后人动了计划，该放它再试");

    // 改的就是**正在失败的那条活**：活换了，之前的失败不算数——还没停也照旧清零
    let mut st2 = fresh(&["[P0] a"]);
    st2.policy.max_fail_streak = 3;
    outcome(&mut st2, "t1", "fail", "boom");
    tick_at(&mut st2, "edit", Some("t1"), None);
    assert_eq!(tick::fail_streak(&st2), 0, "改的是失败的那条活，README 教的出口不能堵上");
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

/// A-11：一条**落在未来**的 tick 撞上配额，会让 runner 睡到下个世纪。
///
/// throttle 分支拿"窗口里最老那条 tick"算还要等多久，`oldest` 在未来时那是个天文数字——
/// 实测 `interval_min=38048610`（72 年），而 `zloop status` 上只印 `睡到 00:00`，和正常的
/// 轮次间隔长得一模一样。造出这条 tick 不需要有人手改文件：NTP 校时、改时区、虚拟机挂起
/// 恢复、笔记本电池耗尽后时钟重置，都会让已经写下的 tick 落在"未来"。
///
/// 封顶的依据：一条 tick 最多在窗口里待 `window_hours`，等得比这更久没有任何道理。
#[test]
fn a_future_tick_cannot_stretch_the_throttle_wait_past_the_window() {
    let mut st = fresh(&["[P0] a"]);
    st.policy.max_runs = 1;
    let now = now_utc();
    let clock_jumped = DateTime::parse_from_rfc3339("2099-01-01T00:00:00+00:00").unwrap();
    tick_at(&mut st, "progress", Some("t1"), Some(clock_jumped));

    let d = tick::decide(&st, now);
    assert_eq!((d.should_run, d.reason.as_str()), (false, "throttled"));
    let cap = st.policy.window_hours as u32 * 60;
    assert_eq!(d.interval_min, Some(cap), "等得比配额窗口本身还久没有道理（撤掉封顶：38048610 分钟 ≈ 72 年）");
    // runner 把它换算成秒（`secs(units, false) = units * 60`）：封顶后是一天，不封顶是 22 亿秒
    assert!(d.interval_min.unwrap() as u64 * 60 <= 24 * 3600, "睡的秒数也得跟着封住");

    // 正常的（过去的）配额窗口不受影响：还是精确算到分钟，不会被封顶抹平
    let mut past = fresh(&["[P0] a"]);
    past.policy.max_runs = 1;
    tick_at(&mut past, "progress", Some("t1"), Some(now - Duration::hours(2)));
    assert_eq!(tick::decide(&past, now).interval_min, Some(22 * 60 + 1));
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

/// A-21：和 A-17 后半截一模一样的形状，只是换了一条 streak。人在另一个终端补一句
/// `zloop feedback`，同一条 todo 原地踏步那道闸就永远数不到上限——实测 8 轮 progress
/// 一直不停（`scripts/repro-a20-a21-another-terminal-disarms-the-brakes.sh` 场景 D）。
#[test]
fn feedback_mid_run_does_not_disarm_the_progress_brake() {
    let mut st = fresh(&["[P0] a", "[P1] 另一条不相干的活"]);
    st.policy.max_progress_streak = 3;
    for i in 0..2 {
        outcome(&mut st, "t1", "progress", "还在推");
        // 循环还没停在人面前，这句话不是"这条活不再原地踏步了"
        tick_at(&mut st, "feedback", Some("t1"), None);
        assert_eq!(tick::progress_streak(&st.ticks, "t1", 3), i + 1, "第 {} 句反馈把原地踏步计数清零了", i + 1);
        assert!(tick::decide(&st, now_utc()).should_run, "还没到上限，不该停");
    }
    outcome(&mut st, "t1", "progress", "还在推");
    let d = tick::decide(&st, now_utc());
    assert_eq!((d.should_run, d.reason.as_str()), (false, "progress_streak"), "该停在原地踏步上");

    // 停下来等人之后人再开口：放它再试（和 fail_streak 同一条规矩）
    tick_at(&mut st, "feedback", Some("t1"), None);
    assert!(tick::decide(&st, now_utc()).should_run, "停下来等人之后，人说话该让循环继续");

    // README 给这道闸开的出口是「拆小它」：改的就是这条 todo，还没停也清零
    let mut st2 = fresh(&["[P0] a", "[P1] 另一条不相干的活"]);
    st2.policy.max_progress_streak = 3;
    for _ in 0..2 {
        outcome(&mut st2, "t1", "progress", "还在推");
    }
    tick_at(&mut st2, "edit", Some("t2"), None); // 改别的活不算（A-20 同一条规矩）
    assert_eq!(tick::progress_streak(&st2.ticks, "t1", 3), 2, "改别的 todo 不该清掉这条的原地踏步计数");
    tick_at(&mut st2, "edit", Some("t1"), None); // 拆小它——活换了
    assert_eq!(tick::progress_streak(&st2.ticks, "t1", 3), 0, "把这条 todo 改小了，之前的原地踏步不算数");
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

/// A-7：`policy.window_hours` 手滑写大一位，每轮都要走的三条命令全 panic。
///
/// `.zloop/state.json` 是**给人改的**——zloop 自己就在教人改隔壁的 `policy.max_total_usd`。
/// 而 `at - Duration::hours(n)` 对越界的 n 是 panic 不是报错：`window_hours = 99999999999`
/// 时 `zloop status` / `zloop context` 一起退 101（`tick.rs` 的 `DateTime - TimeDelta
/// overflowed`），再大一位连 chrono 内部的 `TimeDelta::hours out of bounds` 都出来，
/// 整个项目目录就此敲不动。
///
/// 修法是取值先钳进 `0..=WINDOW_HOURS_MAX` 再交给 chrono：钳过的语义是"按一年算"，
/// 循环照跑；钳过这件事本身由 `doctor` 的 `bad_policy` 说出来（见 doctor_test）。
#[test]
fn an_out_of_range_window_hours_gets_clamped_instead_of_panicking() {
    let now = now_utc();
    // 撤掉钳位（`at - Duration::hours(policy.window_hours)`）时，下面每一个取值都会 panic
    for hours in [99_999_999_999i64, -99_999_999_999, 999_999_999_999_999_999, i64::MAX, i64::MIN] {
        let mut st = fresh(&["[P0] a"]);
        st.policy.window_hours = hours;
        st.policy.max_runs = 1;
        tick_at(&mut st, "progress", Some("t1"), Some(now - Duration::hours(1)));

        // 1. 收窗口不炸：正数钳到一年（一小时前那条当然还在窗口里），负数钳到 0（窗口空掉）
        let counted = tick::window_ticks(&st, now).len();
        assert_eq!(counted, usize::from(hours > 0), "window_hours={hours} 该按钳过的值收窗口");

        // 2. throttle 那一支也不炸，而且等待照旧封在窗口以内
        let d = tick::decide(&st, now);
        if hours > 0 {
            assert_eq!((d.should_run, d.reason.as_str()), (false, "throttled"), "window_hours={hours}");
            // 那条 tick 一小时前写下，钳过的窗口是一年 → 还要等「一年差一小时」，正好在封顶以内
            let cap = zloop::tick::WINDOW_HOURS_MAX as u32 * 60;
            let want = (zloop::tick::WINDOW_HOURS_MAX as u32 - 1) * 60 + 1;
            assert_eq!(d.interval_min, Some(want.min(cap)), "window_hours={hours}：等待要按钳过的窗口算");
        } else {
            assert!(d.should_run, "window_hours={hours}：窗口被钳成 0，配额里一条都没有，该放行");
        }
    }
}

/// 钳位只对越界的值动手：默认值和边界值必须一分不差地按原样生效，
/// 否则这道"防越界"的闸就顺手把正常配置也改了。
#[test]
fn in_range_window_hours_is_left_alone() {
    let now = now_utc();
    for hours in [1i64, 24, zloop::tick::WINDOW_HOURS_MAX] {
        // 边界内侧一分钟 / 外侧一分钟：窗口长度但凡被改动过，这两条就有一条数错
        for (offset_min, inside) in [(hours * 60 - 1, true), (hours * 60 + 1, false)] {
            let mut st = fresh(&["[P0] a"]);
            st.policy.window_hours = hours;
            tick_at(&mut st, "progress", Some("t1"), Some(now - Duration::minutes(offset_min)));
            assert_eq!(tick::window_ticks(&st, now).len(), usize::from(inside), "window_hours={hours} offset={offset_min}m");
        }
    }
    // 0 的语义是"窗口空掉"，它本来就合法（钳位前后都一样），单独钉一下别被顺手改掉
    let mut st = fresh(&["[P0] a"]);
    st.policy.window_hours = 0;
    tick_at(&mut st, "progress", Some("t1"), Some(now - Duration::minutes(1)));
    assert_eq!(tick::window_ticks(&st, now).len(), 0, "window_hours=0：窗口里一条都不该有");
}

/// A-7 / A-11 的第三次重演：`policy.intervals_min` 写歪一个数，循环再也醒不过来。
///
/// 前两次分别是 `window_hours` 越界 panic 和未来时间戳把 throttle 拖到下个世纪。这一次的
/// 字段以前**只被查过空不空**，取值一路原样交给 runner。实测 `intervals_min = [4294967295]`
/// 且有 todo 卡在人手里：debug 构建在 `phase::human_minutes` 的 `m + 720` 上 panic
/// （`next` / `status` / `context` 一起退 101），release 构建不 panic，
/// `interval_min` 原样吐 4294967295 分钟 = 8171 年，而面板上因为同一处加法回绕
/// 印的是"约 0 天后重试"——**睡死的表现是一切正常**。
///
/// `throttled` 那一支有窗口封顶挡着（A-11），剩下四支一处封顶都没有，所以每一支都要
/// 单独走一遍：只验 `ready` 的话，撤掉封顶这条测试照样是绿的。
#[test]
fn an_out_of_range_interval_cannot_put_the_loop_to_sleep_forever() {
    let now = now_utc();
    let cap = zloop::tick::INTERVAL_MIN_MAX;
    // 0 是另一头：runner 每轮 sleep 0 秒、立刻再拉起一个 host 会话，是烧钱的忙等
    for (bad, want) in [(u32::MAX, cap), (cap + 1, cap), (525_600, cap), (0, 1)] {
        // 1. ready：每一轮正常派活都带着它
        let mut st = fresh(&["[P0] a"]);
        st.policy.intervals_min = vec![bad];
        let d = tick::decide(&st, now);
        assert_eq!((d.should_run, d.reason.as_str()), (true, "ready"), "intervals_min=[{bad}]");
        assert_eq!(d.interval_min, Some(want), "ready：intervals_min=[{bad}]");

        // 2. user_gate：todo 卡在人手里——原始复现走的就是这一支
        let mut st = fresh(&["[P0] a"]);
        st.policy.intervals_min = vec![bad];
        tick::apply_done(&mut st, "t1", "done", "", Some("which db?"), None, &cli_who()).unwrap();
        let d = tick::decide(&st, now);
        assert_eq!(d.reason, "user_gate", "fixture 防空跑：这一支必须真的走到");
        assert_eq!(d.interval_min, Some(want), "user_gate：intervals_min=[{bad}]");

        // 3. blocked：等的是另一条 todo
        let mut st = fresh(&["[P0] a", "[P0] b"]);
        st.policy.intervals_min = vec![bad];
        st.todos[1].blocked_by = vec!["t1".into()];
        st.todos[0].status = "blocked".into();
        let d = tick::decide(&st, now);
        assert_eq!(d.reason, "blocked", "fixture 防空跑");
        assert_eq!(d.interval_min, Some(want), "blocked：intervals_min=[{bad}]");

        // 4. held_by_other：派活在别的会话手上，过一会儿再来问
        let mut st = fresh(&["[P0] a"]);
        st.policy.intervals_min = vec![bad];
        assert_eq!(tick::hold_decision(&st).interval_min, Some(want), "held_by_other：intervals_min=[{bad}]");

        // 5. runner 在 `decide` 不给间隔时退回"最慢的那一档"，那是这个字段的第二个读者，
        //    绕过了 `interval()`。它换算成秒去 sleep，不封顶就是 8171 年。
        let secs = zloop::tick::clamp_interval(bad) as u64 * 60;
        assert!(secs <= cap as u64 * 60, "睡的秒数也得跟着封住：{secs}");
    }

    // 只歪最慢那一档：`decide` 给的 3 是对的，睡死藏在退避阶梯的末尾
    let mut st = fresh(&["[P0] a"]);
    st.policy.intervals_min = vec![3, 10, u32::MAX];
    assert_eq!(tick::decide(&st, now).interval_min, Some(3), "第一档没歪，不该被动");
    assert_eq!(zloop::tick::clamp_interval(*st.policy.intervals_min.last().unwrap()), cap);

    // 显示层是第二道：`human_minutes` 收的是同一个数，回绕会把面板变成假的
    assert_ne!(zloop::phase::human_minutes(u32::MAX), "约 0 天", "回绕后 8171 年印成 0 天");
    assert_ne!(zloop::phase::human_minutes(u32::MAX - 100), "约 0 天");
    assert_eq!(zloop::phase::human_minutes(1_439), "约 24 小时"); // 边界值不能被顺手改
    assert_eq!(zloop::phase::human_minutes(1_440), "约 1 天");
}

/// 钳位只对越界的值动手：合法的阶梯必须一分不差地按原样生效。
#[test]
fn in_range_intervals_are_left_alone() {
    let now = now_utc();
    for m in [1u32, 3, 30, 1440, zloop::tick::INTERVAL_MIN_MAX] {
        let mut st = fresh(&["[P0] a"]);
        st.policy.intervals_min = vec![m];
        assert_eq!(tick::decide(&st, now).interval_min, Some(m), "intervals_min=[{m}] 是合法的，不该被改");
    }
    // 默认的三档阶梯（3/10/30）逐级退避这件事由 blocked_by_dependency_and_backoff_ladder 钉着
    let st = fresh(&["[P0] a"]);
    assert_eq!(st.policy.intervals_min, vec![3, 10, 30], "默认阶梯变了的话上面那条测试的期望值也要跟着改");
}
