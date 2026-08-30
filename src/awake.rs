//! Keep the Mac awake while a runner is alive; restore the default when none is (docs/KEEP-AWAKE.md).
//!
//! Two layers, both tied to the runner's pid:
//!   * `caffeinate -i -s -w <pid>` — no privileges; holds off idle / on-AC sleep, dies with the runner;
//!   * `sudo -n pmset -a disablesleep 1` — the only thing that survives a closed lid; needs a sudoers
//!     rule (`zloop install --sudoers`). Holders are counted in `~/.zloop/awake/<pid>` so several
//!     runners can share the setting, and a detached watchdog runs `zloop awake reconcile` when the
//!     runner dies, so even `kill -9` restores the default.
//! On non-macOS platforms every function is a no-op.

use crate::runner::{CapturedBytes, Group, Stop};
use anyhow::{bail, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

pub const SUDOERS_FILE: &str = "/etc/sudoers.d/zloop-pmset";

pub fn supported() -> bool {
    cfg!(target_os = "macos")
}

pub fn holders_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".zloop").join("awake"))
}

/// 一条电源命令允许跑多久。`pmset` / `sudo -n` 正常都是几十毫秒；5 秒已经宽松到
/// 「还没回来就是不打算回来了」。**不复用 `--timeout-min`**（那是给宿主的，动辄几十分钟），
/// 也不复用 `git_timeout()`（那 60 秒是留给 `pre-commit` 钩子的，这里没有钩子这回事）。
fn probe_timeout() -> Duration {
    crate::runner::env_secs("ZLOOP_AWAKE_TIMEOUT_SECS", 5)
}

/// 跑一条电源命令，**带闸**（`runner::run_capture`）。
///
/// 这一层为什么需要闸：`pmset` 要跟 `powerd` 说话，`sudo` 要过一遍 sudoers 解析和目录服务
/// （公司 Mac 绑了 AD/LDAP 时 `sudo` 会等网络）。任何一处 stall 都会让裸 `.output()` 无限期
/// 等下去，而这几条命令**全在收尾路径上**：`stop()` 第一件事就是 `awake::release()`，
/// 它挂住 = runner 不记 `stop`、不清 pid 文件、退不出去，跟通知那一下挂住是同一种死法
/// （A-14 / T21）。
///
/// `Stop::Ignore` 不是疏忽，是这里的**必要条件**：见 [`Stop`]，`zloop stop` 的 SIGTERM 先置位、
/// 之后才走到这儿，认叫停就等于永远恢复不了默认值。`Group::Own` 同理——被外面整组 `killpg`
/// 时，这条「把设置改回去」的命令必须能从那一刀底下活下来把活干完。
///
/// `None` = 这一次没跑成（起不来 / 超时）。调用方一律当「读不出来 / 没成功」——
/// 每个调用方本来就有这个分支。
fn power_cmd(cmd: Command, what: &str) -> Option<CapturedBytes> {
    match crate::runner::run_capture(cmd, probe_timeout(), Group::Own, Stop::Ignore, None) {
        Ok(c) if c.timed_out => {
            eprintln!("awake: `{what}` 超过 {:?} 没回来，已整组收掉；这次当读不出来", probe_timeout());
            None
        }
        Ok(c) => Some(c),
        Err(e) => {
            eprintln!("awake: `{what}` 起不来：{e}");
            None
        }
    }
}

/// Current `SleepDisabled` value from `pmset -g` (None when unknown / not macOS).
pub fn sleep_disabled() -> Option<bool> {
    if !supported() {
        return None;
    }
    let mut c = Command::new("pmset");
    c.arg("-g");
    let out = power_cmd(c, "pmset -g")?;
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
    let mut c = Command::new("sudo");
    c.args(["-n", "pmset", "-g"]);
    power_cmd(c, "sudo -n pmset -g").and_then(|c| c.status).map(|s| s.success()).unwrap_or(false)
}

fn set_sleep_disabled(on: bool) -> bool {
    let mut c = Command::new("sudo");
    c.args(["-n", "pmset", "-a", "disablesleep", if on { "1" } else { "0" }]);
    // 超时 → false = 「没改成」。宁可让调用方走「改不动」那条分支（撤掉 holder、打提示），
    // 也不能报告一个没发生的改动。
    power_cmd(c, "sudo -n pmset -a disablesleep").and_then(|c| c.status).map(|s| s.success()).unwrap_or(false)
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
        return Acquired {
            caffeinate_pid,
            lid: false,
            hint: Some("could not record awake holder; lid-close sleep left unchanged".into()),
        };
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
    Acquired {
        caffeinate_pid,
        lid,
        hint: if lid { None } else { Some("`sudo pmset -a disablesleep 1` failed; lid-close sleep left unchanged".into()) },
    }
}

/// Called by the runner on every exit path.
pub fn release(pid: u32) -> Reconcile {
    unregister(pid);
    reconcile()
}

/// **故意不走 `run_capture`**：这个进程要活到 runner 死为止（`-w <pid>`），在这里等它退出
/// 就是在这里等到长跑结束。detach 出去、拿到 pid 就走，是它唯一正确的起法。
/// 它也不需要闸——我们从不等它，挂不住我们。
fn spawn_caffeinate(pid: u32) -> Option<u32> {
    let mut cmd = Command::new("caffeinate");
    cmd.args(["-i", "-s", "-w", &pid.to_string()]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    crate::daemon::detach_cmd(&mut cmd);
    cmd.spawn().ok().map(|c| c.id())
}

/// `while kill -0 <pid>; do sleep N; done; zloop awake reconcile` — detached so it outlives the runner.
///
/// 和 `spawn_caffeinate` 同理，**故意不走 `run_capture`**：它的全部职责就是比 runner 活得久
/// （`kill -9` 之后替我们恢复默认值）。等它退出 = 等到它已经没用了。
fn spawn_watchdog(pid: u32) -> Option<u32> {
    let poll = std::env::var("ZLOOP_AWAKE_POLL_SECS").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "15".into());
    let exe = std::env::current_exe().ok()?;
    let script = format!(
        "while kill -0 \"$1\" 2>/dev/null; do sleep {poll}; done; exec \"{}\" awake reconcile >/dev/null 2>&1",
        exe.display()
    );
    let mut cmd = Command::new("sh");
    cmd.args(["-c", &script, "zloop-awake-watchdog", &pid.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
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

/// One short line for `zloop status`, or `None` when the default behaviour needs no comment.
/// `zloop awake` still prints the full sentence — the dashboard only speaks up when it matters.
pub fn brief() -> Option<(String, bool)> {
    if !supported() {
        return None;
    }
    let holders = live_holders().len();
    match (sleep_disabled(), holders) {
        (Some(true), 0) => Some(("系统被设为不休眠，但没有 runner 在跑".into(), true)),
        // 「不用你操心」这半句是必须的：这行以前只说"合盖不休眠"，人看了不知道
        // 是自己该去开，还是已经替他开好了（真被问过）。
        (Some(true), n) => {
            let batt = on_battery();
            // 合盖不休眠 + 电池 = 整夜满速跑到没电关机，跑通宵的活正好死在这上面。
            // 警告放**最前面**：这一行会按终端宽度截断，挂在末尾的警告等于没有（踩过）。
            let head = if batt { "正用电池，跑长活记得插电 · " } else { "" };
            Some((format!("{head}合盖、息屏都不会停（zloop 自动开的，{n} 个 runner 跑完自动恢复）"), batt))
        }
        (Some(false), 0) => (!sudo_ok()).then(|| ("合盖会休眠 · 跑一次 `zloop install --sudoers` 开启保护".into(), false)),
        (Some(false), n) => Some((format!("记了 {n} 个 runner 却没生效"), true)),
        (None, _) => None,
    }
}

/// 现在靠电池吗？读不出来就当没在（宁可不说，也别瞎警告）——超时也算读不出来。
fn on_battery() -> bool {
    let mut c = Command::new("pmset");
    c.args(["-g", "batt"]);
    power_cmd(c, "pmset -g batt")
        .filter(|o| o.status.map(|s| s.success()).unwrap_or(false))
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("'Battery Power'"))
        .unwrap_or(false)
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
    let mut c = Command::new("visudo");
    c.args(["-c", "-f"]).arg(&tmp);
    // `visudo -c` 只读一个文件、不问任何问题，所以它走闸；读不出来（超时/起不来）就当
    // 「没能检查」，跟原来 `Err` 那条分支一样放行——真正的判官是下面的 `sudo install`。
    if let Some(o) = power_cmd(c, "visudo -c") {
        if !o.status.map(|s| s.success()).unwrap_or(false) {
            let msg = String::from_utf8_lossy(&o.stderr);
            if !msg.contains("permission") && !msg.contains("Permission") {
                bail!("visudo rejected the rule: {}", msg.trim());
            }
        }
    }
    println!("installing {SUDOERS_FILE} (you may be asked for your password):\n{rule}");
    // **故意不走 `run_capture`**：这一下会在终端上问密码。闸会把 stdin 接到 `/dev/null`、
    // 把 stdout/stderr 接成管道，密码提示到不了人眼前，人打的字也进不去——装上闸等于把
    // `zloop install --sudoers` 弄坏。「人在键盘前想多久」也没有合理的超时可定。
    // 它只在人手敲这一条命令时跑，不在 runner 的任何路径上，挂住也只挂住人自己那个终端。
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
