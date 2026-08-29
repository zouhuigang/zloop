//! `zloop start` / `zloop stop`: run the runner as a detached background process.
//!
//! No tmux, no launchd: `start` re-executes this binary with `run …` in its own
//! session (setsid) with stdio redirected to `.zloop/runner/console.log`, and
//! records the pid in `.zloop/runner/pid`. `stop` sends SIGTERM (then SIGKILL).

use anyhow::{bail, Context, Result};
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::state::STATE_DIR;

pub fn pid_path(root: &Path) -> PathBuf {
    root.join(STATE_DIR).join("runner").join("pid")
}

pub fn log_path(root: &Path) -> PathBuf {
    root.join(STATE_DIR).join("runner").join("console.log")
}

#[cfg(unix)]
mod sys {
    extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
        fn setsid() -> i32;
    }
    pub const SIGTERM: i32 = 15;
    pub const SIGKILL: i32 = 9;
    pub fn alive(pid: i32) -> bool {
        pid > 0 && unsafe { kill(pid, 0) } == 0
    }
    pub fn signal(pid: i32, sig: i32) -> bool {
        unsafe { kill(pid, sig) == 0 }
    }
    /// Detach from the controlling terminal so closing it does not take the runner down.
    pub fn detach(cmd: &mut std::process::Command) {
        use std::os::unix::process::CommandExt;
        unsafe {
            cmd.pre_exec(|| {
                setsid();
                Ok(())
            });
        }
    }
}

#[cfg(not(unix))]
mod sys {
    pub const SIGTERM: i32 = 15;
    pub const SIGKILL: i32 = 9;
    pub fn alive(_pid: i32) -> bool {
        false
    }
    pub fn signal(_pid: i32, _sig: i32) -> bool {
        false
    }
    pub fn detach(_cmd: &mut std::process::Command) {}
}

/// Is a process with this pid alive?
pub fn pid_alive(pid: i32) -> bool {
    sys::alive(pid)
}

/// Detach a command from the controlling terminal (own session).
pub fn detach_cmd(cmd: &mut Command) {
    sys::detach(cmd);
}

/// Pid of a live runner for this project, if any. Stale pid files are removed.
pub fn running(root: &Path) -> Option<i32> {
    let p = pid_path(root);
    let pid: i32 = fs::read_to_string(&p).ok()?.trim().parse().ok()?;
    if sys::alive(pid) {
        Some(pid)
    } else {
        let _ = fs::remove_file(&p);
        None
    }
}

/// 写 pid 文件。必须是原子替换：`start` 先按子进程 pid 写一次，runner 起来后又用自己的 pid
/// 覆写同一个文件，而 `fs::write` 是「先截断再写」——`zloop status` 正好读在这两步中间就读到空
/// 文件，把活着的 runner 误报成「没有 runner 在跑」。同目录写临时文件再 rename，读者就只看得见
/// 旧值或新值，看不见中间态。
pub fn write_pid(root: &Path, pid: u32) -> Result<()> {
    let p = pid_path(root);
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = p.with_extension(format!("tmp.{}", std::process::id()));
    fs::write(&tmp, format!("{pid}\n"))?;
    if let Err(e) = fs::rename(&tmp, &p) {
        let _ = fs::remove_file(&tmp);
        return Err(e).context("replacing the runner pid file");
    }
    Ok(())
}

pub fn clear_pid(root: &Path) {
    let p = pid_path(root);
    if let Ok(s) = fs::read_to_string(&p) {
        if s.trim().parse::<u32>().ok() == Some(std::process::id()) {
            let _ = fs::remove_file(&p);
        }
    }
}

/// Spawn `zloop --dir <root> run <args…>` detached; returns the child pid.
pub fn start(root: &Path, run_args: &[String]) -> Result<u32> {
    if let Some(pid) = running(root) {
        bail!("runner already running (pid {pid}); use `zloop stop` first or `zloop status` to watch it");
    }
    let exe = std::env::current_exe().context("locating the zloop binary")?;
    let log = log_path(root);
    if let Some(parent) = log.parent() {
        fs::create_dir_all(parent)?;
    }
    let out = OpenOptions::new().create(true).append(true).open(&log)?;
    let err = out.try_clone()?;
    let mut cmd = Command::new(exe);
    cmd.arg("--dir").arg(root).arg("run").args(run_args).current_dir(root);
    cmd.stdin(Stdio::null()).stdout(Stdio::from(out)).stderr(Stdio::from(err));
    sys::detach(&mut cmd);
    let child = cmd.spawn().context("spawning the background runner")?;
    let pid = child.id();
    write_pid(root, pid)?;
    Ok(pid)
}

/// SIGTERM the running runner (SIGKILL after 5 s). Returns the pid that was stopped.
pub fn stop(root: &Path) -> Result<Option<i32>> {
    let Some(pid) = running(root) else { return Ok(None) };
    sys::signal(pid, sys::SIGTERM);
    let deadline = Instant::now() + Duration::from_secs(5);
    while sys::alive(pid) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(100));
    }
    if sys::alive(pid) {
        sys::signal(pid, sys::SIGKILL);
        thread::sleep(Duration::from_millis(200));
    }
    let _ = fs::remove_file(pid_path(root));
    Ok(Some(pid))
}
