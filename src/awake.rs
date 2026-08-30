//! Keep the Mac awake while a runner is alive; restore the default when none is (docs/KEEP-AWAKE.md).
//!
//! Two layers, both tied to the runner's pid:
//!   * `caffeinate -i -s -w <pid>` — no privileges; holds off idle / on-AC sleep, dies with the runner;
//!   * `sudo -n pmset -a disablesleep 1` — the only thing that survives a closed lid; needs a sudoers
//!     rule (`zloop install --sudoers`). Holders are counted in `~/.zloop/awake/<pid>` so several
//!     runners can share the setting, and a detached watchdog runs `zloop awake reconcile` when the
//!     runner dies, so even `kill -9` restores the default.
//!
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

/// 规则文件的暂存处：一个**只有自己进得去**的 0700 目录，里面放一个 0600 的文件。
/// `Drop` 负责清场，所以中途 `bail!`（visudo 拒绝、sudo 失败）也不会在临时目录里留东西——
/// 修之前那条路只在 `sudo install` 之后删一次，visudo 拒绝那一支的文件是漏着的。
pub struct StagedRule {
    dir: PathBuf,
    file: PathBuf,
}

impl StagedRule {
    /// 交给 `visudo -c -f` / `sudo install` 的那条路径。
    pub fn file(&self) -> &Path {
        &self.file
    }
    /// 装着它的私有目录（只有自己能进）。
    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

impl Drop for StagedRule {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.file);
        let _ = fs::remove_dir(&self.dir);
    }
}

/// 把 sudoers 规则暂存到 `base` 底下，交给调用方去 `visudo -c` / `sudo install`。
///
/// **为什么不能直接写 `base/zloop-pmset.<pid>`（T43）**：那条路径名字可猜（pid），而
/// `fs::write` 既不 `O_EXCL` 也不 `O_NOFOLLOW`。`TMPDIR` 指向共享目录时
/// （`export TMPDIR=/tmp` 是常见的手工设置，`/tmp` 是 1777），别的 uid 能提前占住这个名字：
/// 占成他自己的 0666 普通文件（我们只是 truncate 后写一遍，属主还是他），或占成一条软链接
/// （我们顺着写进他的文件）。随后 `sudo install` 从**同一条路径**重新读一次，装进
/// `/etc/sudoers.d/` 的就是他那一份 —— 一条 `NOPASSWD: ALL` 就是把 root 送出去。
/// 窗口也不止「write 到 install」那一瞬：中间还夹着一次交互式密码输入，人想多久它就有多久。
/// 实测两种占名方式见 `scripts/repro-t43-sudoers-tmp-swap.sh`。
///
/// 现在的做法：`mkdir` 一个随机名的 0700 目录，文件用 `create_new` + 0600 建在里面。
/// `mkdir` 是原子的——名字被占就是 `EEXIST`，我们换个名字重来，绝不会悄悄接手别人的目录。
/// 于是安全边界不再取决于 `TMPDIR` 指向哪儿：父目录是我们自己刚建的，别人进不去也换不掉里面的东西。
/// 名字随机是为了让「预先把候选名字占满」这种拒绝服务也不成立，它不是边界本身。
pub fn stage_rule_in(base: &Path, rule: &str) -> Result<StagedRule> {
    use std::io::Write;
    for _ in 0..8 {
        let dir = base.join(format!("zloop-sudoers.{:016x}", random_tag()));
        match private_dir(&dir) {
            Ok(()) => {
                // 先接管，后面写文件失败也能靠 Drop 把目录清掉
                let staged = StagedRule { file: dir.join("zloop-pmset"), dir };
                private_file(&staged.file)
                    .and_then(|mut f| f.write_all(rule.as_bytes()))
                    .with_context(|| format!("writing {}", staged.file.display()))?;
                return Ok(staged);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e).with_context(|| format!("creating {}", dir.display())),
        }
    }
    bail!("could not create a private staging directory under {}", base.display())
}

/// 随机后缀。撞名不是安全问题（`mkdir` 会 `EEXIST`，我们重来），所以 `/dev/urandom` 读不到时
/// 退到 `RandomState`（进程起来时由内核播种）+ 时间戳就够了。
fn random_tag() -> u64 {
    use std::io::Read;
    let mut b = [0u8; 8];
    if fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut b)).is_ok() {
        return u64::from_ne_bytes(b);
    }
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0);
    h.write_u64(nanos);
    h.write_u32(std::process::id());
    h.finish()
}

/// `mkdir` 一个 0700 的新目录；已经存在（哪怕是别人的软链接）就是 `AlreadyExists`，绝不复用。
/// umask 只会**再削**权限位，不会加，所以拿到的目录不可能比 0700 更松。
#[cfg(unix)]
fn private_dir(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    fs::DirBuilder::new().mode(0o700).create(path)
}
#[cfg(not(unix))]
fn private_dir(path: &Path) -> std::io::Result<()> {
    fs::DirBuilder::new().create(path)
}

/// 新建一个 0600 的文件：`create_new` = `O_EXCL`，撞上任何已存在的东西都失败而不是跟着写。
#[cfg(unix)]
fn private_file(path: &Path) -> std::io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path)
}
#[cfg(not(unix))]
fn private_file(path: &Path) -> std::io::Result<fs::File> {
    fs::OpenOptions::new().write(true).create_new(true).open(path)
}

/// Write the sudoers rule (validates with `visudo -c`, then `sudo install`; prompts for a password once).
pub fn install_sudoers() -> Result<PathBuf> {
    if !supported() {
        bail!("--sudoers is only meaningful on macOS");
    }
    let user = std::env::var("USER").or_else(|_| std::env::var("LOGNAME")).context("cannot determine current user")?;
    let rule = sudoers_rule(&user);
    // `sudo install` 会从这条路径**重新读一遍**，所以这条路径必须是别人够不着的（见 stage_rule_in）
    let staged = stage_rule_in(&std::env::temp_dir(), &rule)?;
    let mut c = Command::new("visudo");
    c.args(["-c", "-f"]).arg(staged.file());
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
        .arg(staged.file())
        .arg(SUDOERS_FILE)
        .status()
        .context("running sudo install")?;
    // 清场交给 `staged` 的 Drop：走哪条出口都清，包括上面 visudo 拒绝时的 `bail!`
    if !status.success() {
        bail!("sudo install failed; you can install the rule by hand: sudo visudo -f {SUDOERS_FILE}");
    }
    Ok(PathBuf::from(SUDOERS_FILE))
}
