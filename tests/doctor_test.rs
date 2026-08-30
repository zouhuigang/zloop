//! `zloop doctor` 的回归测试。
//!
//! 每条都尽量**用 CLI 走到那个坏状态**，而不是直接摆一个手写的 state.json：
//! 只有真能走到的不一致才值得体检。手写文件只用在"文件被人改坏了"这类本来就来自外部的场景。

mod common;

use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

struct Out {
    code: i32,
    out: String,
    err: String,
}

fn zloop(dir: &Path, args: &[&str]) -> Out {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_zloop"));
    cmd.current_dir(dir).args(args);
    common::scrub_ambient_env(&mut cmd);
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let o = cmd.output().expect("spawn zloop");
    Out {
        code: o.status.code().unwrap_or(-1),
        out: String::from_utf8_lossy(&o.stdout).into_owned(),
        err: String::from_utf8_lossy(&o.stderr).into_owned(),
    }
}

fn plan(dir: &Path, lines: &str) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_zloop"));
    cmd.current_dir(dir).arg("plan");
    common::scrub_ambient_env(&mut cmd);
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    use std::io::Write;
    child.stdin.take().unwrap().write_all(lines.as_bytes()).unwrap();
    let o = child.wait_with_output().unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
}

/// doctor --json 的 findings 里都有哪些 kind
fn kinds(dir: &Path) -> (Vec<String>, i32) {
    let o = zloop(dir, &["doctor", "--json"]);
    let v: serde_json::Value = serde_json::from_str(&o.out).unwrap_or_else(|e| panic!("bad json {e}: {}{}", o.out, o.err));
    let ks = v["findings"].as_array().unwrap().iter().map(|f| f["kind"].as_str().unwrap().to_string()).collect();
    (ks, o.code)
}

fn init(dir: &Path, goal: &str) {
    let o = zloop(dir, &["init", goal]);
    assert_eq!(o.code, 0, "{}", o.err);
}

#[test]
fn healthy_project_says_nothing_is_wrong() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    init(d, "alpha");
    plan(d, "[P0] design\n[P1] build\n");

    let o = zloop(d, &["doctor"]);
    assert_eq!(o.code, 0, "{}", o.err);
    assert!(o.out.contains("没发现问题"), "{}", o.out);

    let o = zloop(d, &["doctor", "--json"]);
    let v: serde_json::Value = serde_json::from_str(&o.out).unwrap();
    assert_eq!(v["errors"], 0);
    assert_eq!(v["warnings"], 0);
    assert_eq!(v["goals"], 1);
    assert!(v["findings"].as_array().unwrap().is_empty(), "{}", o.out);
}

#[test]
fn not_a_zloop_project_exits_1() {
    let tmp = tempfile::tempdir().unwrap();
    let o = zloop(tmp.path(), &["doctor"]);
    assert_eq!(o.code, 1);
    assert!(o.err.contains("no zloop state"), "{}", o.err);
}

/// `blocked_by` 指着一条不在清单里的 todo：`is_executable` 要求依赖"存在且 done"，
/// 于是这条 todo 从此永远排不上——不报错，只是不动。
///
/// 这个状态**以前是 `zloop compact` 自己造出来的**（搬走做完的 t1，t2 的 `blocked_by`
/// 还指着它）；t39 之后 compact 会把还有人等的那条留下，所以来源只剩手改过的 state
/// 和老版本 zloop 留下的文件——检查本身照旧要有，坏文件不会因为新版本不再生产就消失。
#[test]
fn a_dependency_that_is_not_in_the_list_leaves_a_todo_that_can_never_run() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    init(d, "alpha");
    plan(d, "[P0] first\n[P1] second\n");
    assert_eq!(zloop(d, &["edit", "t2", "--blocked-by", "t1"]).code, 0);
    // 手改文件把 t1 抹掉：等价于老版本 compact 搬走它之后留下的 state
    let p = d.join(".zloop/state.json");
    let mut st: serde_json::Value = serde_json::from_str(&fs::read_to_string(&p).unwrap()).unwrap();
    st["todos"].as_array_mut().unwrap().retain(|t| t["id"] != "t1");
    fs::write(&p, serde_json::to_string_pretty(&st).unwrap()).unwrap();

    let (ks, code) = kinds(d);
    assert!(ks.contains(&"dangling_blocked_by".to_string()), "{ks:?}");
    assert_eq!(code, 1, "依赖永远满足不了是要修的问题，不是留意");
    let o = zloop(d, &["doctor"]);
    assert!(o.out.contains("t2 依赖 t1"), "{}", o.out);
    assert!(o.out.contains("zloop edit t2 --blocked-by ''"), "建议动作要能直接抄：{}", o.out);
}

/// A-9：依赖成环 = 永久卡死，而修复前 doctor 一声不吭。
///
/// 二元环用**真命令**就能造出来（`edit` 只挡自依赖，多跳环不挡），这正是它值得体检的理由：
/// 不需要有人手改文件。造出来之后 `next` 一直返回 `blocked` + "隔一阵重试"，
/// 而重试到天荒地老也不会有人先 done。
#[test]
fn a_dependency_cycle_is_reported_instead_of_retried_forever() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    init(d, "alpha");
    plan(d, "[P0] first\n[P0] second\n");
    assert_eq!(zloop(d, &["edit", "t1", "--blocked-by", "t2"]).code, 0);
    assert_eq!(zloop(d, &["edit", "t2", "--blocked-by", "t1"]).code, 0);

    // 先钉住"卡死"这个前提：没有它，下面报的就只是个没后果的形状
    let o = zloop(d, &["next", "--peek", "--json"]);
    let v: serde_json::Value = serde_json::from_str(&o.out).unwrap();
    assert_eq!((v["should_run"].as_bool(), v["reason"].as_str()), (Some(false), Some("blocked")), "{}", o.out);

    let (ks, code) = kinds(d);
    assert!(ks.contains(&"dep_cycle".to_string()), "{ks:?}");
    assert_eq!(code, 1, "环上还有活着的 todo = 循环现在就卡着，是要修的问题");
    let o = zloop(d, &["doctor"]);
    assert!(o.out.contains("t1 → t2 → t1"), "要把环本身印出来，人才知道断哪条：{}", o.out);
    assert!(o.out.contains("zloop edit t1 --blocked-by ''"), "建议动作要能直接抄：{}", o.out);

    // 断开一条，doctor 立刻闭嘴（否则这条测试连"报的是不是这个环"都证不出来）
    assert_eq!(zloop(d, &["edit", "t2", "--blocked-by", ""]).code, 0);
    let (ks, code) = kinds(d);
    assert!(!ks.contains(&"dep_cycle".to_string()), "环断了还在报：{ks:?}");
    assert_eq!(code, 0);
}

/// t35：依赖一条**延后**的 todo，和依赖一条不存在的 todo 是同一个后果——
/// deferred 不进 `open_ordered`，永远派不出去，依赖它的那条也就永远等不到 done。
/// 两条真命令（`edit --blocked-by` + `edit --status deferred`）就能走到，而修复前
/// `next` 只说 `blocked` + "10 分钟后重试"，doctor 一声不吭退 0。
#[test]
fn depending_on_a_deferred_todo_is_reported_like_a_dangling_one() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    init(d, "alpha");
    plan(d, "[P0] first\n[P0] second\n");
    assert_eq!(zloop(d, &["edit", "t2", "--blocked-by", "t1"]).code, 0);
    assert_eq!(zloop(d, &["edit", "t1", "--status", "deferred"]).code, 0);

    // 先钉住"卡死"这个前提：没有它，下面报的就只是个没后果的形状
    let o = zloop(d, &["next", "--peek", "--json"]);
    let v: serde_json::Value = serde_json::from_str(&o.out).unwrap();
    assert_eq!((v["should_run"].as_bool(), v["reason"].as_str()), (Some(false), Some("blocked")), "{}", o.out);

    let (ks, code) = kinds(d);
    assert!(ks.contains(&"dead_blocked_by".to_string()), "{ks:?}");
    assert_eq!(code, 1, "还活着的 t2 在等一条派不出去的依赖 = 要修的问题");
    let o = zloop(d, &["doctor"]);
    assert!(o.out.contains("t2 依赖 t1（已延后）"), "要说清依赖死在哪一步：{}", o.out);
    assert!(o.out.contains("zloop edit t1 --status open"), "建议动作要能直接抄：{}", o.out);

    // 把 t1 捡回来，doctor 立刻闭嘴，next 也重新有活干
    assert_eq!(zloop(d, &["edit", "t1", "--status", "open"]).code, 0);
    let (ks, code) = kinds(d);
    assert!(!ks.contains(&"dead_blocked_by".to_string()), "依赖捡回来了还在报：{ks:?}");
    assert_eq!(code, 0);
    let o = zloop(d, &["next", "--peek", "--json"]);
    let v: serde_json::Value = serde_json::from_str(&o.out).unwrap();
    assert_eq!(v["should_run"].as_bool(), Some(true), "{}", o.out);
}

/// 同一条检查的另一半：状态被手改成 zloop 不认的词（`cancelled` 不在 `STATUSES` 里）。
/// 这种 todo 既不是 terminal（还占着 `remaining`）、又过不了 `is_executable` 的
/// `status == "open"`，自己跑不了、还把依赖它的那条一起钉死。
#[test]
fn depending_on_a_todo_with_an_unknown_status_is_reported_too() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    init(d, "alpha");
    plan(d, "[P0] first\n[P0] second\n");
    assert_eq!(zloop(d, &["edit", "t2", "--blocked-by", "t1"]).code, 0);
    let p = d.join(".zloop/state.json");
    let mut st: serde_json::Value = serde_json::from_str(&fs::read_to_string(&p).unwrap()).unwrap();
    st["todos"][0]["status"] = serde_json::json!("cancelled");
    fs::write(&p, serde_json::to_string_pretty(&st).unwrap()).unwrap();

    let o = zloop(d, &["next", "--peek", "--json"]);
    let v: serde_json::Value = serde_json::from_str(&o.out).unwrap();
    assert_eq!(v["reason"].as_str(), Some("blocked"), "{}", o.out);

    let (ks, code) = kinds(d);
    assert!(ks.contains(&"dead_blocked_by".to_string()), "{ks:?}");
    assert_eq!(code, 1);
    let o = zloop(d, &["doctor"]);
    assert!(o.out.contains("\"cancelled\""), "要把那个野状态原样印出来：{}", o.out);
}

/// 正常形状一个字都不该报：依赖还开着（迟早会被派出去）、依赖在等人（人一答就回队列）、
/// 依赖已经做完。少了这一条，`dead_blocked_by` 会在跑得好好的项目上退 1。
#[test]
fn a_live_dependency_is_not_a_dead_end() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    init(d, "alpha");
    plan(d, "[P0] first\n[P0] second\n");
    // t2 依赖 t1，t1 还开着
    assert_eq!(zloop(d, &["edit", "t2", "--blocked-by", "t1"]).code, 0);
    let (ks, code) = kinds(d);
    assert!(!ks.contains(&"dead_blocked_by".to_string()), "{ks:?}");
    assert_eq!(code, 0);

    // t1 被派出去后卡在"等人回答"上：依赖没死，人一答就继续
    assert_eq!(zloop(d, &["next"]).code, 0);
    let o = zloop(d, &["done", "t1", "--note", "得问", "--approach", "问了 a", "--block", "选哪个方案？"]);
    assert_eq!(o.code, 0, "{}", o.err);
    let (ks, code) = kinds(d);
    assert!(!ks.contains(&"dead_blocked_by".to_string()), "等人不等于死路：{ks:?}");
    assert_eq!(code, 0);

    // 依赖被延后的那条 todo 自己也是 terminal 时同样不报（谁都没在等它）
    assert_eq!(zloop(d, &["edit", "t1", "--status", "deferred"]).code, 0);
    assert_eq!(zloop(d, &["edit", "t2", "--status", "deferred"]).code, 0);
    let (ks, code) = kinds(d);
    assert!(!ks.contains(&"dead_blocked_by".to_string()), "两条都了结了，卡不住谁：{ks:?}");
    assert_eq!(code, 0);
}

/// 环的两个边界：自依赖（`edit` 已经挡住，只能从手改的文件进来）报，
/// 而"依赖已经做完"的正常形状不报——后者是每个用了 `--blocked-by` 的项目的日常。
#[test]
fn a_self_dependency_from_a_hand_edited_file_is_reported_but_a_finished_dep_is_not() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    init(d, "alpha");
    plan(d, "[P0] first\n[P0] second\n");
    // t2 依赖 t1，t1 做完 → t2 可以跑。这不是环，一个字都不该报。
    assert_eq!(zloop(d, &["edit", "t2", "--blocked-by", "t1"]).code, 0);
    assert_eq!(zloop(d, &["next"]).code, 0);
    assert_eq!(zloop(d, &["done", "t1", "--note", "ok", "--approach", "做了 a 因为 b"]).code, 0);
    let (ks, code) = kinds(d);
    assert!(!ks.contains(&"dep_cycle".to_string()), "依赖做完了就不挡人，不是环：{ks:?}");
    assert_eq!(code, 0);

    // 手改文件造自依赖（`edit` 那条路已经被挡住了）
    let p = d.join(".zloop/state.json");
    let mut st: serde_json::Value = serde_json::from_str(&fs::read_to_string(&p).unwrap()).unwrap();
    st["todos"][1]["blocked_by"] = serde_json::json!(["t2"]);
    fs::write(&p, serde_json::to_string_pretty(&st).unwrap()).unwrap();

    let (ks, code) = kinds(d);
    assert!(ks.contains(&"dep_cycle".to_string()), "{ks:?}");
    assert_eq!(code, 1);
    let o = zloop(d, &["doctor"]);
    assert!(o.out.contains("t2 依赖自己"), "{}", o.out);
}

/// 环上全是了结掉的 todo：现在卡不住谁（没人在等它们），但捡回来就会——报 Warn 不报 Error。
/// 少了这一档，`doctor` 会在一个跑得好好的项目上退 1，把"要修"这个词用滥。
#[test]
fn a_cycle_among_deferred_todos_is_only_a_warning() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    init(d, "alpha");
    plan(d, "[P0] first\n[P0] second\n[P0] third\n");
    assert_eq!(zloop(d, &["edit", "t1", "--blocked-by", "t2"]).code, 0);
    assert_eq!(zloop(d, &["edit", "t2", "--blocked-by", "t1"]).code, 0);
    assert_eq!(zloop(d, &["edit", "t1", "--status", "deferred"]).code, 0);
    assert_eq!(zloop(d, &["edit", "t2", "--status", "deferred"]).code, 0);

    let o = zloop(d, &["doctor", "--json"]);
    let v: serde_json::Value = serde_json::from_str(&o.out).unwrap();
    let cycles: Vec<&serde_json::Value> =
        v["findings"].as_array().unwrap().iter().filter(|f| f["kind"] == "dep_cycle").collect();
    assert_eq!(cycles.len(), 1, "{}", o.out);
    assert_eq!(cycles[0]["level"], "warn", "环上没有活着的 todo，这会儿卡不住谁：{}", o.out);
    assert_eq!(o.code, 0, "只有 warn 时 doctor 该退 0");
}

/// 停放的目标文件被改名（或 park 时换过 id），id 和文件名就对不上——
/// 下一次 park 会按 id 再造一个同名文件，两份目标抢一个 id。
#[test]
fn goal_id_out_of_sync_with_its_filename() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    init(d, "alpha");
    assert_eq!(zloop(d, &["goal", "new", "beta"]).code, 0);
    let parked = d.join(".zloop/goals/alpha.json");
    assert!(parked.is_file(), "goal new 应该把 alpha 停到 goals/alpha.json");
    fs::rename(&parked, d.join(".zloop/goals/renamed.json")).unwrap();

    let (ks, code) = kinds(d);
    assert_eq!(ks, vec!["id_filename_mismatch"], "{ks:?}");
    assert_eq!(code, 1);
    let o = zloop(d, &["doctor"]);
    assert!(o.out.contains("mv .zloop/goals/renamed.json .zloop/goals/alpha.json"), "{}", o.out);
}

/// 两个文件都自称同一个 id：`resolve` 命中多个就 bail，这个 id 从此谁也点不动。
#[test]
fn two_goal_files_claiming_the_same_id() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    init(d, "alpha");
    assert_eq!(zloop(d, &["goal", "new", "beta"]).code, 0);
    fs::copy(d.join(".zloop/goals/alpha.json"), d.join(".zloop/goals/copy.json")).unwrap();

    let (ks, code) = kinds(d);
    assert!(ks.contains(&"duplicate_goal_id".to_string()), "{ks:?}");
    assert_eq!(code, 1);
    // doctor 说的和 goal switch 的实际行为要对得上：这个 id 确实点不动了
    let o = zloop(d, &["goal", "switch", "alpha"]);
    assert_ne!(o.code, 0, "{}{}", o.out, o.err);
    assert!(o.err.contains("对上了 2 个目标"), "{}", o.err);
}

#[test]
fn broken_goal_file_gets_a_next_step() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    init(d, "alpha");
    assert_eq!(zloop(d, &["goal", "new", "beta"]).code, 0);
    fs::write(d.join(".zloop/goals/alpha.json"), "{ not json").unwrap();

    let (ks, code) = kinds(d);
    assert_eq!(ks, vec!["broken_goal"], "{ks:?}");
    assert_eq!(code, 1);
    let o = zloop(d, &["doctor"]);
    assert!(o.out.contains("zloop goal rm alpha"), "{}", o.out);
}

#[test]
fn headless_project_points_at_goal_switch() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    init(d, "alpha");
    assert_eq!(zloop(d, &["goal", "new", "beta"]).code, 0);
    fs::remove_file(d.join(".zloop/state.json")).unwrap(); // 搬家中断 / 手工删掉当前目标

    let (ks, code) = kinds(d);
    assert!(ks.contains(&"headless".to_string()), "{ks:?}");
    assert_eq!(code, 1);
    let o = zloop(d, &["doctor"]);
    assert!(o.out.contains("zloop goal switch alpha"), "{}", o.out);
    // 别的命令这时全都报"没有目标"，doctor 必须还能跑——它就是给这一刻用的
    assert!(o.out.contains("当前没有目标在开着"), "{}", o.out);
}

/// tick 记着日志路径，文件却被删了。信息已经没了，但循环照跑——所以是"留意"，退出码 0。
#[test]
fn missing_log_file_is_a_warning_not_an_error() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    init(d, "alpha");
    plan(d, "[P0] first\n");
    assert_eq!(zloop(d, &["next"]).code, 0);
    let o = zloop(d, &["done", "t1", "--note", "ok", "--approach", "做了 a 因为 b"]);
    assert_eq!(o.code, 0, "{}", o.err);

    let st: serde_json::Value = serde_json::from_str(&fs::read_to_string(d.join(".zloop/state.json")).unwrap()).unwrap();
    let rel = st["ticks"].as_array().unwrap().iter().find_map(|t| t["log"].as_str()).expect("done 应该写了一份日志");
    fs::remove_file(d.join(".zloop").join(rel)).unwrap();

    let (ks, code) = kinds(d);
    assert_eq!(ks, vec!["missing_log"], "{ks:?}");
    assert_eq!(code, 0, "日志没了不该让 CI 变红");
    let o = zloop(d, &["doctor"]);
    assert!(o.out.contains("1 轮的日志文件不在了"), "{}", o.out);
}

/// pid 文件指着一个不存在的进程。顺便钉住"只读"：`zloop status` 会顺手删掉它，doctor 不许删。
#[test]
fn stale_pid_is_reported_and_doctor_changes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    init(d, "alpha");
    plan(d, "[P0] first\n");
    let pid = d.join(".zloop/runner/pid");
    fs::create_dir_all(pid.parent().unwrap()).unwrap();
    fs::write(&pid, "999999\n").unwrap(); // macOS 的 pid 上限是 99998，这个不可能活着

    let before = fs::read(d.join(".zloop/state.json")).unwrap();
    let (ks, code) = kinds(d);
    assert_eq!(ks, vec!["stale_pid"], "{ks:?}");
    assert_eq!(code, 0);

    assert!(pid.is_file(), "doctor 只读：不许像 status 那样顺手清掉 pid 文件");
    assert_eq!(fs::read(d.join(".zloop/state.json")).unwrap(), before, "doctor 不许写 state.json（连 noop tick 都不能记）");

    // 对照组：status 才是会清它的那个
    assert_eq!(zloop(d, &["status"]).code, 0);
    assert!(!pid.is_file());
}

/// 归档里多份同名：`zloop goal rm` 的文件名自带时间戳所以不会互相覆盖，
/// 但翻旧账时两份都叫同一个 id。不影响运行 → 留意级。
#[test]
fn archive_name_collision_is_only_a_warning() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    init(d, "alpha");
    assert_eq!(zloop(d, &["goal", "new", "beta"]).code, 0);
    assert_eq!(zloop(d, &["goal", "rm", "alpha"]).code, 0);
    let archived: Vec<_> = fs::read_dir(d.join(".zloop/archive")).unwrap().flatten().map(|e| e.path()).collect();
    assert_eq!(archived.len(), 1);
    fs::copy(&archived[0], archived[0].with_file_name("20200101T000000-alpha.json")).unwrap();

    let o = zloop(d, &["doctor", "--json"]);
    let v: serde_json::Value = serde_json::from_str(&o.out).unwrap();
    assert_eq!(v["archived"], 2);
    assert_eq!(v["findings"][0]["kind"], "archive_id_collision");
    assert_eq!(v["errors"], 0);
    assert_eq!(o.code, 0);
}

/// `zloop compact` 往 archive/ 里写的 `compact-*.json` 不是一份目标（故意的：它只装老 tick）。
/// 体检不能把它当成"读不出来的归档"报出来——那是一条永远清不掉的假问题。
#[test]
fn compact_dumps_are_not_mistaken_for_broken_archives() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    init(d, "alpha");
    plan(d, "[P0] first\n");
    assert_eq!(zloop(d, &["next"]).code, 0);
    assert_eq!(zloop(d, &["done", "t1", "--note", "ok", "--approach", "做了 a"]).code, 0);
    assert_eq!(zloop(d, &["compact", "--keep-days", "0"]).code, 0);
    assert!(fs::read_dir(d.join(".zloop/archive"))
        .unwrap()
        .flatten()
        .any(|e| e.file_name().to_string_lossy().starts_with("compact-")));

    let (ks, code) = kinds(d);
    assert!(ks.is_empty(), "{ks:?}");
    assert_eq!(code, 0);
}

/// NOTES.md 里混进非 UTF-8 字节。写路径已经会当场拒绝（A-4），**读路径不会**：
/// `zloop context` 用宽容版 read，读失败就当"什么都没记过"，「约定」「经验」两整节
/// 一声不吭地消失、命令还 exit 0——下一轮的 agent 就这么在没有护栏的情况下开工。
/// 这个测试先复现那个静默，再钉住 doctor 必须把它说出来。
#[test]
fn unreadable_notes_reported_because_context_drops_the_rules_silently() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    init(d, "alpha");
    plan(d, "[P0] first\n");
    assert_eq!(zloop(d, &["remember", "--rule", "done 之前必须 cargo test 全过"]).code, 0);
    assert_eq!(zloop(d, &["remember", "bench 要在 release 下跑"]).code, 0);
    // 对照组：好好的时候约定进得了交接包，doctor 也不该报
    assert!(zloop(d, &["context"]).out.contains("done 之前必须 cargo test 全过"));
    assert!(zloop(d, &["doctor"]).out.contains("没发现问题"));

    // 真实来路：编辑器存错编码、别的工具往里追加了二进制。read_to_string 从此直接失败
    let notes = d.join(".zloop/NOTES.md");
    let mut raw = fs::read(&notes).unwrap();
    raw.extend_from_slice(&[0xff, 0xfe]);
    fs::write(&notes, &raw).unwrap();

    // 先复现坏结果本身：整节消失，没有一个字提到出了事，退出码还是 0
    let o = zloop(d, &["context"]);
    assert_eq!(o.code, 0, "{}{}", o.out, o.err);
    assert!(!o.out.contains("本项目的约定"), "复现前提变了，约定还在：{}", o.out);
    assert!(!o.out.contains("bench 要在 release"), "经验也一起没了才是这个 bug：{}", o.out);

    // doctor 就是替这条静默路径出声的那个
    let (ks, code) = kinds(d);
    assert!(ks.contains(&"unreadable_notes".to_string()), "{ks:?}");
    assert_eq!(code, 1, "护栏整节没了是要修的问题，不是留意");
    let o = zloop(d, &["doctor"]);
    assert!(o.out.contains(".zloop/NOTES.md 读不出来"), "得指名道姓是哪个文件：{}", o.out);
    assert!(o.out.contains("约定") && o.out.contains("经验"), "得说清楚丢的是什么：{}", o.out);

    // 修好就该闭嘴：文件恢复成能读的，体检回到干净
    fs::write(&notes, &raw[..raw.len() - 2]).unwrap();
    assert!(zloop(d, &["doctor"]).out.contains("没发现问题"), "修好之后不该还在报");
    assert!(zloop(d, &["context"]).out.contains("done 之前必须 cargo test 全过"));
}

/// 没有 NOTES.md 是**合法**状态（还没记过任何东西），不是病。
#[test]
fn a_project_that_never_recorded_notes_is_healthy() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    init(d, "alpha");
    assert!(!d.join(".zloop/NOTES.md").exists());
    let (ks, code) = kinds(d);
    assert!(ks.is_empty(), "{ks:?}");
    assert_eq!(code, 0);
}

#[test]
fn leftover_temp_files_get_reported() {
    // 账本的写法是 tmp → sync → rename，所以进程被杀不会损坏正本（实测 386 次 SIGKILL
    // 一次没坏），但那个 .tmp 会永远留着没人清。doctor 以前对它完全沉默，说"没发现问题"。
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    zloop(d, &["init", "残留检查"]);
    assert!(zloop(d, &["doctor"]).out.contains("没发现问题"), "干净的项目不该报");

    std::fs::write(d.join(".zloop/state.json.tmp"), "{\"半截").unwrap();
    std::fs::create_dir_all(d.join(".zloop/goals")).unwrap();
    std::fs::write(d.join(".zloop/goals/g1.json.tmp"), "{").unwrap();

    let out = zloop(d, &["doctor"]).out;
    assert!(out.contains("上次写入没写完就被打断"), "{out}");
    assert!(out.contains("state.json.tmp") && out.contains("g1.json.tmp"), "两个都要列出来: {out}");
    assert!(out.contains("正本没事"), "别把人吓着——正本是好的: {out}");
    assert!(out.contains("留意"), "这是留意不是必修: {out}");

    let r = zloop::doctor::check(d);
    assert_eq!(r.errors, 0, "残留不算错误");
    assert!(r.findings.iter().any(|f| f.kind == "leftover_tmp"), "{:?}", r.findings);
}

/// A-7 的体检面：`policy` 里的数值写出了范围，得有人说一句。
///
/// `window_hours` 越界以前是直接 panic——炸掉的正好是每轮都要走的 `next` / `status` /
/// `context`，而唯一一个专门回答"哪儿不对"的命令 exit 0 说"没发现问题"。
/// 现在取值会被钳进合法区间，循环照跑；但**钳过就等于人写的那个数没生效**，
/// 静悄悄地按别的数跑比崩掉更难查。这条就是那一句。
#[test]
fn policy_numbers_written_out_of_range_are_reported() {
    // (改哪个字段, 写成什么, 是不是 error 级)
    // error = 人写的取值被无声地换掉了；warn = 有个说得过去的兜底（intervals_min 空掉退回 3 分钟）
    let cases = [
        ("window_hours", serde_json::json!(99_999_999_999i64), true),
        ("window_hours", serde_json::json!(-1), true),
        ("window_hours", serde_json::json!(i64::MAX), true),
        ("max_total_usd", serde_json::json!(-5.0), true),
        ("intervals_min", serde_json::json!([]), false),
        // 空不空是这个字段最浅的写错法；写歪的**取值**才是让 runner 睡死 / 忙等的那种，
        // 而它以前一条都不报（doctor --json 的 findings 是空的）
        ("intervals_min", serde_json::json!([4_294_967_295u32]), true),
        ("intervals_min", serde_json::json!([3, 10, 4_294_967_295u32]), true), // 只歪了最慢那一档
        ("intervals_min", serde_json::json!([0]), true),                       // sleep 0 秒的忙等
        // 每一档都合法、但阶梯写反了：没有任何值被改掉，所以只是 warn（详见
        // a_backwards_backoff_ladder_is_reported）
        ("intervals_min", serde_json::json!([30, 10, 3]), false),
        ("intervals_min", serde_json::json!([3, 30, 10]), false), // 只有末尾那一档往回走
    ];
    for (field, value, fatal) in cases {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        init(d, "alpha");
        plan(d, "[P0] one\n");
        let p = d.join(".zloop/state.json");
        let mut v: serde_json::Value = serde_json::from_str(&fs::read_to_string(&p).unwrap()).unwrap();
        v["policy"][field] = value.clone();
        fs::write(&p, serde_json::to_string(&v).unwrap()).unwrap();

        let (ks, code) = kinds(d);
        assert!(ks.contains(&"bad_policy".to_string()), "policy.{field} = {value} 该被报出来，实际 {ks:?}");
        assert_eq!(code != 0, fatal, "policy.{field} = {value}：doctor 的退出码级别不对");
        let o = zloop(d, &["doctor"]);
        assert!(o.out.contains(field), "报告里得点名是哪个字段：{}", o.out);
        assert!(!o.out.contains("没发现问题"), "policy.{field} = {value}：{}", o.out);
    }
    // 合法取值一条都不能被误伤（边界值也算合法）
    for (field, value) in [
        ("window_hours", serde_json::json!(0)),
        ("window_hours", serde_json::json!(24)),
        ("window_hours", serde_json::json!(24 * 365)),
        ("max_total_usd", serde_json::json!(0.0)),
        ("max_total_usd", serde_json::json!(12.5)),
        ("intervals_min", serde_json::json!([3, 10, 30])),
        ("intervals_min", serde_json::json!([1])),
        ("intervals_min", serde_json::json!([zloop::tick::INTERVAL_MIN_MAX])),
        // 持平不是写错：那是有人存心不要退避（runner 的 fixture 就这么写）。
        // 「往回走」的那条报的是往回走，不是「没有严格递增」——这几条是它的假阳性防线
        ("intervals_min", serde_json::json!([10, 10, 10])),
        ("intervals_min", serde_json::json!([1, 1, 2])),
        ("intervals_min", serde_json::json!([3, 3, 3])),
    ] {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        init(d, "alpha");
        plan(d, "[P0] one\n");
        let p = d.join(".zloop/state.json");
        let mut v: serde_json::Value = serde_json::from_str(&fs::read_to_string(&p).unwrap()).unwrap();
        v["policy"][field] = value.clone();
        fs::write(&p, serde_json::to_string(&v).unwrap()).unwrap();
        let (ks, _) = kinds(d);
        assert!(!ks.contains(&"bad_policy".to_string()), "policy.{field} = {value} 是合法的，不该报：{ks:?}");
    }
}

/// 阶梯写反了：每一档都在合法区间里，所以取值那条查不出来，而退避整个反过来。
///
/// 前半段先把**失败场景**跑出来（不依赖 doctor），后半段才验 doctor 说了这一句：
/// 只验 doctor 的话，这条报警报的是什么、值不值得报，后人无从判断。
#[test]
fn a_backwards_backoff_ladder_is_reported() {
    use zloop::tick;

    // --- 失败场景：intervals_min = [30, 10, 3]，每个数都在 1..=INTERVAL_MIN_MAX 里 ---
    let ladder = |iv: Vec<u32>| {
        let mut st = common::fresh(&["[P0] a", "[P0] b"]);
        st.policy.intervals_min = iv;
        st.todos[1].blocked_by = vec!["t1".into()];
        st.todos[0].status = "blocked".into();
        let mut seq = vec![];
        for _ in 0..3 {
            let d = tick::decide(&st, common::now_utc());
            assert_eq!(d.reason, "blocked", "fixture 防空跑：退避那一支必须真的走到");
            seq.push(d.interval_min.unwrap());
            common::tick_at(&mut st, "noop", None, None);
        }
        seq
    };
    // 写对了：越不出活等得越久
    assert_eq!(ladder(vec![3, 10, 30]), vec![10, 30, 30]);
    // 写反了：越不出活退得越快——最该慢下来的那一支反而 polling 得最凶
    assert_eq!(ladder(vec![30, 10, 3]), vec![10, 3, 3], "阶梯写反时退避是往回走的");

    // 而正常派活（level 0）用的是**最慢**的那一档：有活干的时候反倒每 30 分钟才动一次
    let mut st = common::fresh(&["[P0] a"]);
    st.policy.intervals_min = vec![30, 10, 3];
    let d = tick::decide(&st, common::now_utc());
    assert_eq!((d.should_run, d.interval_min), (true, Some(30)), "ready 拿的是第一档");

    // 第三处：退避耗尽之后 runner 睡的那一档（`tick::ladder_tail`）——阶梯走到头就是**末档**，
    // 写反时那是**最快**的一档（3），不是最大的 30。这是 T34 定死的口径：runner 照末档睡，
    // 不替人把顺序纠正过来，所以「写反了要付什么代价」只能由下面这条 warn 说出来。
    assert_eq!(zloop::tick::ladder_tail(&st), 3, "末档就是 3");
    assert_eq!(st.policy.intervals_min.iter().copied().max(), Some(30), "末档不是最大值");

    // --- doctor 得说这一句（撤掉 check_policy 里那一段，下面全红）---
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    init(d, "alpha");
    plan(d, "[P0] one\n");
    let p = d.join(".zloop/state.json");
    let mut v: serde_json::Value = serde_json::from_str(&fs::read_to_string(&p).unwrap()).unwrap();
    v["policy"]["intervals_min"] = serde_json::json!([30, 10, 3]);
    fs::write(&p, serde_json::to_string(&v).unwrap()).unwrap();

    let (ks, code) = kinds(d);
    assert!(ks.contains(&"bad_policy".to_string()), "阶梯写反了该被报出来：{ks:?}");
    assert_eq!(code, 0, "没有任何值被改掉，循环照跑——这是 warn 不是 error");

    let o = zloop(d, &["doctor"]);
    assert!(o.out.contains("intervals_min"), "报告里得点名是哪个字段：{}", o.out);
    assert!(o.out.contains("往回走"), "得说清楚是哪种写错法：{}", o.out);
    assert!(!o.out.contains("没发现问题"), "{}", o.out);

    // 报的是"往回走"，不是"没有严格递增"：持平是合法写法，不能被误伤
    let tmp2 = tempfile::tempdir().unwrap();
    let d2 = tmp2.path();
    init(d2, "alpha");
    plan(d2, "[P0] one\n");
    let p2 = d2.join(".zloop/state.json");
    let mut v2: serde_json::Value = serde_json::from_str(&fs::read_to_string(&p2).unwrap()).unwrap();
    v2["policy"]["intervals_min"] = serde_json::json!([10, 10, 10]);
    fs::write(&p2, serde_json::to_string(&v2).unwrap()).unwrap();
    assert!(!kinds(d2).0.contains(&"bad_policy".to_string()), "持平是存心不退避，不该报");
}
