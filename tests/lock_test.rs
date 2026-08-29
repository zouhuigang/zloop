//! 锁的回归测试：超时那句话要说清「被谁挡住了」，只读命令不许被挡住。
//!
//! 这里的持有者都是**真拿着锁**的线程/进程，不是手摆出来的文件——只有真能出现的现场才值得钉。
//! 唯一手写的是「持有者记录里的 pid 已经死了」那条：它本来就来自外部（进程被强杀）。

mod common;

use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};
use zloop::state;

/// `state::set_operation` 是进程级的：同一个测试二进制里的用例并行跑会互相改掉操作名。
/// 凡是要断言操作名的用例都先拿这把锁，串起来跑。
static SEQ: Mutex<()> = Mutex::new(());

fn seq() -> std::sync::MutexGuard<'static, ()> {
    SEQ.lock().unwrap_or_else(|e| e.into_inner())
}

fn zloop(dir: &Path, args: &[&str]) -> (i32, String, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_zloop"));
    cmd.current_dir(dir).args(args);
    common::scrub_ambient_env(&mut cmd);
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let o = cmd.output().expect("spawn zloop");
    (
        o.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&o.stdout).into_owned(),
        String::from_utf8_lossy(&o.stderr).into_owned(),
    )
}

/// 一个真的死 pid：起一个进程、等它退干净，再用它的号。
fn dead_pid() -> u32 {
    let mut c = Command::new("/bin/echo").stdout(Stdio::null()).spawn().unwrap();
    let pid = c.id();
    c.wait().unwrap();
    pid
}

/// 在另一个线程上等锁（不同的 fd，等价于另一个进程），返回它拿到的错误。
fn other_thread_waits(path: &Path, wait: Duration) -> String {
    let p = path.to_path_buf();
    let h = thread::spawn(move || state::locked(&p, wait, || Ok(())));
    h.join().unwrap().unwrap_err().to_string()
}

fn fresh_state(dir: &Path) -> std::path::PathBuf {
    let path = state::state_path(dir);
    let mut st = state::default_state("g", "p");
    state::save(&path, &mut st).unwrap();
    path
}

#[test]
fn timeout_names_the_live_holder() {
    let _seq = seq();
    let dir = tempfile::tempdir().unwrap();
    let path = fresh_state(dir.path());
    state::set_operation("done t16");
    state::locked(&path, state::LOCK_WAIT, || {
        let err = other_thread_waits(&path, Duration::from_millis(200));
        assert!(err.contains("could not lock"), "{err}");
        assert!(err.contains(&format!("pid {}", std::process::id())), "看不到持有者 pid：{err}");
        assert!(err.contains("done t16"), "看不到操作名：{err}");
        assert!(err.contains("进程还活着"), "没说进程是不是还在：{err}");
        assert!(err.contains("拿到锁"), "没说持有多久：{err}");
        assert!(err.contains("别删锁文件"), "没给处置步骤：{err}");
        Ok(())
    })
    .unwrap();
}

#[test]
fn holder_record_is_written_while_held_and_cleared_after() {
    let _seq = seq();
    let dir = tempfile::tempdir().unwrap();
    let path = fresh_state(dir.path());
    state::set_operation("goal switch");
    state::locked(&path, state::LOCK_WAIT, || {
        let h = state::read_holder(&path).expect("持锁期间应该有持有者记录");
        assert_eq!(h.pid, std::process::id());
        assert_eq!(h.op, "goal switch");
        assert!(state::parse_iso(&h.at).is_ok(), "acquired_at 不是合法时间：{}", h.at);
        Ok(())
    })
    .unwrap();
    // 释放之后不能留下一份「有人持锁」的假象
    assert!(state::read_holder(&path).is_none(), "锁放掉了，持有者记录还在");
    assert!(!state::holder_path(&path).exists());
}

/// 闭包 panic 了也不能留下「有人持锁」的记录：锁被展开放掉了，记录还在的话，
/// 下一个超时的人会看到一个 pid 还活着的假持有者（同一个进程还在跑）。
#[test]
fn holder_record_is_cleared_even_if_the_closure_panics() {
    let _seq = seq();
    let dir = tempfile::tempdir().unwrap();
    let path = fresh_state(dir.path());
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {})); // 这条 panic 是故意的，别刷屏
    let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        state::locked::<()>(&path, state::LOCK_WAIT, || panic!("boom")).unwrap()
    }));
    std::panic::set_hook(hook);
    assert!(out.is_err(), "闭包该 panic 的");
    assert!(state::read_holder(&path).is_none(), "panic 之后还留着持有者记录");
}

#[test]
fn stale_holder_record_is_called_out_instead_of_believed() {
    let _seq = seq();
    let dir = tempfile::tempdir().unwrap();
    let path = fresh_state(dir.path());
    let gone = dead_pid();
    state::locked(&path, state::LOCK_WAIT, || {
        // 进程被强杀：锁早被内核放了，记录却留了下来。下一个人写了新记录，这里手动摆回旧的。
        std::fs::write(
            state::holder_path(&path),
            format!(r#"{{"pid":{gone},"op":"run 第 3 轮","at":"{}"}}"#, state::now_iso()),
        )
        .unwrap();
        let err = other_thread_waits(&path, Duration::from_millis(200));
        assert!(err.contains("已经不在了"), "把死进程当成持有者报了：{err}");
        assert!(err.contains("lsof"), "没告诉人怎么找真正的持有者：{err}");
        Ok(())
    })
    .unwrap();
}

#[test]
fn missing_holder_record_still_says_what_to_do() {
    let _seq = seq();
    let dir = tempfile::tempdir().unwrap();
    let path = fresh_state(dir.path());
    state::locked(&path, state::LOCK_WAIT, || {
        std::fs::remove_file(state::holder_path(&path)).unwrap(); // 旧版 zloop 持的锁
        let err = other_thread_waits(&path, Duration::from_millis(200));
        assert!(err.contains("没有持有者记录"), "{err}");
        assert!(err.contains("lsof"), "{err}");
        Ok(())
    })
    .unwrap();
}

/// 端到端：外面真有人持锁时，写命令报出持有者，只读命令一秒都不等。
///
/// 只读命令走 `state::load`（不上锁，读的是 `save` 用 rename 换过去的完整一份），所以它们的等待
/// 是 0 而不只是「更短」。哪天有人往读路径上加锁，这条会先红。
#[test]
fn write_waits_and_reports_while_reads_go_straight_through() {
    let _seq = seq();
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    assert_eq!(zloop(d, &["init", "g"]).0, 0);

    let path = state::state_path(d);
    state::set_operation("done t16");
    let (hold_tx, hold_rx) = mpsc::channel::<()>();
    let (ready_tx, ready_rx) = mpsc::channel::<()>();
    let p = path.clone();
    let holder = thread::spawn(move || {
        state::locked(&p, state::LOCK_WAIT, || {
            ready_tx.send(()).unwrap();
            let _ = hold_rx.recv_timeout(Duration::from_secs(30));
            Ok(())
        })
        .unwrap();
    });
    ready_rx.recv_timeout(Duration::from_secs(5)).expect("持锁线程没起来");

    // 只读：三条命令都必须马上返回（写命令那一档是 5 秒）
    for cmd in [vec!["status"], vec!["context"], vec!["log"]] {
        let t0 = Instant::now();
        let (code, _, err) = zloop(d, &cmd);
        let took = t0.elapsed();
        assert_eq!(code, 0, "zloop {cmd:?} 在别人持锁时失败了：{err}");
        assert!(took < Duration::from_secs(2), "zloop {cmd:?} 等了 {took:?}——只读命令不该等锁");
    }

    // 写：等满一档再报，且报出是谁
    let t0 = Instant::now();
    let (code, _, err) = zloop(d, &["pause"]);
    let took = t0.elapsed();
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("could not lock"), "{err}");
    assert!(err.contains(&format!("pid {}", std::process::id())), "{err}");
    assert!(err.contains("done t16"), "{err}");
    assert!(took >= state::LOCK_WAIT, "没等满 {:?} 就放弃了：{took:?}", state::LOCK_WAIT);

    let _ = hold_tx.send(());
    holder.join().unwrap();
    assert_eq!(zloop(d, &["pause"]).0, 0, "锁放掉之后写命令应该恢复正常");
}
