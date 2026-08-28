//! Keep the Mac awake while a runner is alive; restore the default when none is (docs/KEEP-AWAKE.md).
//!
//! Two layers, both tied to the runner's pid:
//!   * `caffeinate -i -s -w <pid>` — no privileges; holds off idle / on-AC sleep, dies with the runner;
//!   * `sudo -n pmset -a disablesleep 1` — the only thing that survives a closed lid; needs a sudoers
//!     rule (`zloop install --sudoers`). Holders are counted in `~/.zloop/awake/<pid>` so several
//!     runners can share the setting, and a detached watchdog runs `zloop awake reconcile` when the
//!     runner dies, so even `kill -9` restores the default.
//! On non-macOS platforms every function is a no-op.

use anyhow::{bail, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub const SUDOERS_FILE: &str = "/etc/sudoers.d/zloop-pmset";

pub fn supported() -> bool {
    cfg!(target_os = "macos")
}

pub fn holders_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".zloop").join("awake"))
}

/// Current `SleepDisabled` value from `pmset -g` (None when unknown / not macOS).
pub fn sleep_disabled() -> Option<bool> {
    if !supported() {
        return None;
    }
    let out = Command::new("pmset").arg("-g").output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    text.lines().find_map(|l| {
        let l = l.trim();
        l.strip_prefix("SleepDisabled").map(|rest| rest.trim() == "1")
    })
}

/// Can we run pmset as root without a password prompt?
pub fn sudo_ok() -> bool {
    if !supported() {
        return false;
    }
    Command::new("sudo")
        .args(["-n", "pmset", "-g"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn set_sleep_disabled(on: bool) -> bool {
    Command::new("sudo")
        .args(["-n", "pmset", "-a", "disablesleep", if on { "1" } else { "0" }])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Runners currently holding the awake setting: (pid, project root). Dead pids are cleaned up.
pub fn live_holders() -> Vec<(u32, String)> {
    let Some(dir) = holders_dir() else { return Vec::new() };
    let Ok(entries) = fs::read_dir(&dir) else { return Vec::new() };
    let mut out = Vec::new();
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let Ok(pid) = name.parse::<u32>() else { continue };
        if crate::daemon::pid_alive(pid as i32) {
            out.push((pid, fs::read_to_string(e.path()).unwrap_or_default().trim().to_string()));
        } else {
            let _ = fs::remove_file(e.path());
        }
    }
    out.sort();
    out
}

fn register(pid: u32, root: &Path) -> Result<()> {
    let dir = holders_dir().context("no home directory")?;
    fs::create_dir_all(&dir)?;
    fs::write(dir.join(pid.to_string()), format!("{}\n", root.display()))?;
    Ok(())
}

fn unregister(pid: u32) {
    if let Some(dir) = holders_dir() {
        let _ = fs::remove_file(dir.join(pid.to_string()));
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Reconcile {
    pub holders: usize,
    pub before: Option<bool>,
    /// Some(true) = turned sleep-disable on, Some(false) = turned it off, None = nothing to do
    pub changed: Option<bool>,
    pub sudo: bool,
}

/// Make `SleepDisabled` match reality: on while any live holder exists, off otherwise.
pub fn reconcile() -> Reconcile {
    let holders = live_holders().len();
    let before = sleep_disabled();
    let sudo = sudo_ok();
    let want = holders > 0;
    let changed = match (before, sudo) {
        (Some(cur), true) if cur != want => {
            if set_sleep_disabled(want) {
                Some(want)
            } else {
                None
            }
        }
        _ => None,
    };
    Reconcile { holders, before, changed, sudo }
}

pub struct Acquired {
    pub caffeinate_pid: Option<u32>,
    /// True when lid-close sleep is disabled for this runner.
    pub lid: bool,
    pub hint: Option<String>,
}

/// Called by the runner at start-up.
pub fn acquire(root: &Path, pid: u32) -> Acquired {
    if !supported() {
        return Acquired { caffeinate_pid: None, lid: false, hint: None };
    }
    let caffeinate_pid = spawn_caffeinate(pid);
    if !sudo_ok() {
        return Acquired {
            caffeinate_pid,
            lid: false,
            hint: Some(
                "lid-close sleep is NOT disabled (needs passwordless `sudo pmset`); run `zloop install --sudoers` once to enable it. Idle sleep is held off by caffeinate."
                    .into(),
            ),
        };
    }
    if register(pid, root).is_err() {
        return Acquired { caffeinate_pid, lid: false, hint: Some("could not record awake holder; lid-close sleep left unchanged".into()) };
    }
    let lid = match sleep_disabled() {
        Some(true) => true,
        _ => set_sleep_disabled(true),
    };
    if lid {
        spawn_watchdog(pid);
    } else {
        unregister(pid);
    }
    Acquired { caffeinate_pid, lid, hint: if lid { None } else { Some("`sudo pmset -a disablesleep 1` failed; lid-close sleep left unchanged".into()) } }
}

/// Called by the runner on every exit path.
pub fn release(pid: u32) -> Reconcile {
    unregister(pid);
    reconcile()
}

fn spawn_caffeinate(pid: u32) -> Option<u32> {
    let mut cmd = Command::new("caffeinate");
    cmd.args(["-i", "-s", "-w", &pid.to_string()]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    crate::daemon::detach_cmd(&mut cmd);
    cmd.spawn().ok().map(|c| c.id())
}

/// `while kill -0 <pid>; do sleep N; done; zloop awake reconcile` — detached so it outlives the runner.
fn spawn_watchdog(pid: u32) -> Option<u32> {
    let poll = std::env::var("ZLOOP_AWAKE_POLL_SECS").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "15".into());
    let exe = std::env::current_exe().ok()?;
    let script = format!(
        "while kill -0 \"$1\" 2>/dev/null; do sleep {poll}; done; exec \"{}\" awake reconcile >/dev/null 2>&1",
        exe.display()
    );
    let mut cmd = Command::new("sh");
    cmd.args(["-c", &script, "zloop-awake-watchdog", &pid.to_string()]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    crate::daemon::detach_cmd(&mut cmd);
    cmd.spawn().ok().map(|c| c.id())
}

/// One line for `zloop status` / `zloop awake`.
pub fn describe() -> String {
    if !supported() {
        return "sleep: n/a (not macOS)".into();
    }
    let holders = live_holders();
    match (sleep_disabled(), holders.len()) {
        (Some(true), 0) => "sleep: ⚠ SleepDisabled=1 but no zloop runner alive → `zloop awake reconcile` (or `zloop stop`) restores the default".into(),
        (Some(true), n) => format!("sleep: lid-close sleep disabled by zloop ({n} runner{}) · restores when they stop", if n > 1 { "s" } else { "" }),
        (Some(false), 0) => {
            if sudo_ok() {
                "sleep: default (lid-close protection ready; a running runner will enable it)".into()
            } else {
                "sleep: default · lid-close protection unavailable — run `zloop install --sudoers` once".into()
            }
        }
        (Some(false), n) => format!("sleep: default (⚠ {n} runner registered but SleepDisabled=0 — `zloop awake reconcile`)"),
        (None, _) => "sleep: unknown (pmset -g unreadable)".into(),
    }
}

pub fn sudoers_rule(user: &str) -> String {
    format!(
        "# zloop: let the runner keep the Mac awake while a task runs, and restore the default afterwards\n\
         {user} ALL=(root) NOPASSWD: /usr/bin/pmset -a disablesleep 1, /usr/bin/pmset -a disablesleep 0, /usr/bin/pmset -g\n"
    )
}

/// Write the sudoers rule (validates with `visudo -c`, then `sudo install`; prompts for a password once).
pub fn install_sudoers() -> Result<PathBuf> {
    if !supported() {
        bail!("--sudoers is only meaningful on macOS");
    }
    let user = std::env::var("USER").or_else(|_| std::env::var("LOGNAME")).context("cannot determine current user")?;
    let rule = sudoers_rule(&user);
    let tmp = std::env::temp_dir().join(format!("zloop-pmset.{}", std::process::id()));
    fs::write(&tmp, &rule)?;
    let check = Command::new("visudo").args(["-c", "-f"]).arg(&tmp).output();
    if let Ok(o) = check {
        if !o.status.success() {
            let msg = String::from_utf8_lossy(&o.stderr);
            if !msg.contains("permission") && !msg.contains("Permission") {
                bail!("visudo rejected the rule: {}", msg.trim());
            }
        }
    }
    println!("installing {SUDOERS_FILE} (you may be asked for your password):\n{rule}");
    let status = Command::new("sudo")
        .args(["install", "-o", "root", "-g", "wheel", "-m", "0440"])
        .arg(&tmp)
        .arg(SUDOERS_FILE)
        .status()
        .context("running sudo install")?;
    let _ = fs::remove_file(&tmp);
    if !status.success() {
        bail!("sudo install failed; you can install the rule by hand: sudo visudo -f {SUDOERS_FILE}");
    }
    Ok(PathBuf::from(SUDOERS_FILE))
}
