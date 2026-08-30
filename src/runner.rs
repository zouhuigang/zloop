//! Headless runner: drive `claude -p` / `codex exec` one bounded round at a time.
//!
//! The scheduler (`tick::decide`) owns every stop condition; the runner only
//! executes with a timeout, checks that the host wrote back, and sleeps.
//! Long-run rules (see docs/LONG-RUN-AUDIT.md):
//!   * a hung host is killed after `timeout_min` and recorded as `fail`;
//!   * waiting on a human (user_gate / blocked) polls at the slowest interval
//!     forever instead of exiting — nothing is spent while polling;
//!   * host rate limits are not failures: sleep and retry, no tick recorded;
//!   * sessions are resumed per todo (new todo → fresh session) unless `--resume all`.

use crate::session::{Host, HostSession};
use crate::state::{self, State};
use crate::{prompt, session, tick};
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

/// Set by SIGTERM/SIGINT (`zloop stop`, Ctrl-C); the loop finishes the current step and exits cleanly.
static STOP_REQUESTED: AtomicBool = AtomicBool::new(false);

#[cfg(unix)]
fn install_signal_handlers() {
    extern "C" {
        fn signal(sig: i32, handler: usize) -> usize;
    }
    extern "C" fn on_term(_: i32) {
        STOP_REQUESTED.store(true, Ordering::SeqCst);
    }
    let h = on_term as extern "C" fn(i32) as usize;
    unsafe {
        signal(15, h); // SIGTERM
        signal(2, h); // SIGINT
    }
}
#[cfg(not(unix))]
fn install_signal_handlers() {}

fn stop_requested() -> bool {
    STOP_REQUESTED.load(Ordering::SeqCst)
}

/// Sleep in half-second slices so a stop request is noticed quickly. Returns false when interrupted.
fn sleep_interruptible(total: Duration) -> bool {
    let end = Instant::now() + total;
    while Instant::now() < end {
        if stop_requested() {
            return false;
        }
        thread::sleep(Duration::from_millis(500).min(end.saturating_duration_since(Instant::now())));
    }
    !stop_requested()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumeMode {
    /// Resume the last session that worked on the *same todo*; a new todo starts fresh.
    Todo,
    /// Always resume the host's most recent session.
    All,
    /// Never resume.
    None,
}

impl ResumeMode {
    pub fn parse(s: &str) -> Option<ResumeMode> {
        match s {
            "todo" => Some(ResumeMode::Todo),
            "all" => Some(ResumeMode::All),
            "none" => Some(ResumeMode::None),
            _ => Option::None,
        }
    }
}

pub struct Options {
    pub host: Host,
    pub max_rounds: u32,
    pub fast: bool,
    pub allow_all: bool,
    pub resume: ResumeMode,
    /// Per-round wall-clock limit for the host (minutes; seconds when `fast`).
    pub timeout_min: u32,
    /// Exit instead of polling when the scheduler is waiting on a human.
    pub exit_on_wait: bool,
    /// Passed through to `claude -p --max-budget-usd`.
    pub max_budget_usd: Option<String>,
    /// Commit the working tree (excluding `.zloop/`) after every round that wrote back.
    pub git_commit: bool,
    /// Keep the Mac awake (caffeinate + lid-close protection) while this runner lives.
    pub keep_awake: bool,
    /// 关掉「写回之后按信号插一轮重估」（默认开）。
    ///
    /// 和 `reflect_every` 的固定节奏不同，重估是**信号触发**的：账本里读不出偏离信号就完全不跑
    /// （见 `docs/ADAPTIVE-REPLAN.md` §2——每轮都重规划会制造计划抖动）。
    /// 无头模式下没人点头，所以它**只把建议记进账本，绝不自己改 todo**。
    pub no_replan: bool,
    /// 让重估轮次**真的改计划**（默认关）。
    ///
    /// 关着的时候（默认）重估只把建议记进账本，等人回来看——这是 zloop 一直以来的红线。
    /// 打开之后，重估那一轮被允许把新清单交给 `zloop replan --apply`，护栏由
    /// `replan::apply` 在代码里强制（见 `docs/ADAPTIVE-REPLAN.md` §8）。
    ///
    /// 这是唯一一处 agent 无人看管地改自己的待办，所以额外压两条闸：
    /// 单次运行最多改 `MAX_AUTO_REPLANS` 次；连着两次都把清单改长就算发散。
    /// 两者任一触顶都**停机等人**，而不是安静地接着跑。
    pub auto_replan: bool,
    /// 每 N 个 todo 轮次插一轮「回看」；0 = 关。
    ///
    /// 形状照 Warp 的 scheduled agent：**按计划跑一段不同的 prompt**，不是新子系统
    /// （见 `docs/SELF-IMPROVEMENT.md` 1.1）。回看那一轮不做 todo、不推进轮次、
    /// 对三条 streak 透明，也不动 `.zloop/NOTES.md`——无头模式下没人点头，
    /// 所以它只把建议记进账本，等人回来看。
    pub reflect_every: u32,
}

const JOURNAL: &str = "runner/journal.jsonl";

/// 单次运行最多自主改几次计划。
///
/// 文献那条"far fewer replans"说的就是这个：能改计划的循环最容易死在
/// replan → 新 todo → replan → …… 永不收敛上。三次之后还没走上正轨，
/// 多半不是计划的问题，该让人看一眼。
pub const MAX_AUTO_REPLANS: u32 = 3;
const RATE_LIMIT_MARKERS: [&str; 8] =
    ["rate limit", "rate_limit", "overloaded", "429", "capacity", "quota", "too many requests", "usage limit"];

fn journal_path(root: &Path) -> PathBuf {
    root.join(state::STATE_DIR).join(JOURNAL)
}

fn journal_append(root: &Path, entry: &Value) -> Result<()> {
    let p = journal_path(root);
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut f = OpenOptions::new().create(true).append(true).open(p)?;
    writeln!(f, "{}", entry)?;
    Ok(())
}

fn last_journal_event(root: &Path) -> Option<Value> {
    let raw = fs::read_to_string(journal_path(root)).ok()?;
    serde_json::from_str(raw.lines().rev().find(|l| !l.trim().is_empty())?).ok()
}

/// Every runner exit is journaled as `stop`; a start whose last event is not `stop` is a restart.
/// Humans are notified for every stop except the one they asked for (`--max-rounds`).
fn stop(root: &Path, reason: &str) -> Result<i32> {
    let r = crate::awake::release(std::process::id());
    if r.changed.is_some() || r.holders > 0 {
        journal_append(
            root,
            &json!({"event": "awake_off", "holders_left": r.holders, "restored_default": r.changed == Some(false), "at": state::now_iso()}),
        )?;
    }
    journal_append(root, &json!({"event": "stop", "reason": reason, "at": state::now_iso()}))?;
    crate::daemon::clear_pid(root);
    if reason != "max_rounds" && reason != "sigterm" {
        if let Ok(st) = state::load(&state::state_path(root)) {
            let hint = match reason {
                "done" => "全部 todo 完成".to_string(),
                "fail_streak" => "连续失败，`zloop log` 看原因，`zloop edit` 后重启".to_string(),
                "progress_streak" => "同一 todo 原地踏步太久，拆小它".to_string(),
                "budget" => format!("已达花费上限 ${:.2}（policy.max_total_usd）", st.policy.max_total_usd),
                "user_gate" | "blocked" => "等你决定（--exit-on-wait 模式）".to_string(),
                other => other.to_string(),
            };
            notify(root, &st, "stop", &format!("{reason} — {hint}"));
        }
    }
    println!("runner: stop ({reason})");
    Ok(0)
}

fn notify(root: &Path, st: &State, kind: &str, detail: &str) {
    if !crate::notify::configured(st) {
        return;
    }
    let text = crate::notify::text_for(kind, st, root, detail);
    match crate::notify::send(st, root, kind, &text) {
        Ok(true) => {
            let _ = journal_append(root, &json!({"event": "notify", "kind": kind, "at": state::now_iso()}));
        }
        Ok(false) => {}
        Err(e) => eprintln!("runner: notify failed: {e}"),
    }
}

/// Run `policy.preflight_cmd`; Ok(summary) when it passes, Err(tail) when it fails.
fn preflight(root: &Path, cmd: &str, timeout: Duration) -> std::result::Result<String, String> {
    let mut c = Command::new("sh");
    c.arg("-c").arg(cmd).current_dir(root);
    isolate_child_env(&mut c, false);
    match run_with_timeout(c, timeout, "sh") {
        Ok(cap) => {
            let combined = format!("{}\n{}", cap.stdout, cap.stderr);
            let tail: String = combined
                .lines()
                .rev()
                .filter(|l| !l.trim().is_empty())
                .take(5)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join(" | ");
            if cap.timed_out {
                Err(format!("preflight timed out: {}", tail.chars().take(200).collect::<String>()))
            } else if cap.status.map(|s| s.success()).unwrap_or(false) {
                Ok(tail.chars().take(200).collect())
            } else {
                Err(format!("preflight failed: {}", tail.chars().take(200).collect::<String>()))
            }
        }
        Err(e) => Err(format!("preflight could not start: {e}")),
    }
}

/// Everything dirty outside `.zloop/` at one instant: path → identity (size:mtime, or the
/// porcelain code once the file is gone). Comparing two of these across a round separates
/// what the host just did from work-in-progress that was already sitting in the tree.
type DirtySnapshot = std::collections::BTreeMap<String, String>;

/// `None` = 这一刻读不出工作树（git 起不来 / 报错 / 挂住被闸收掉）。**不要**把它当成
/// 「树是干净的」：空快照会让 checkpoint 把树里所有脏东西都认成自己的，那才是真会
/// 把邻居的活提交进去。每个调用方都要为 `None` 挑一条保守的路。
fn git_dirty(root: &Path) -> Option<DirtySnapshot> {
    let mut snap = DirtySnapshot::new();
    // -uall lists untracked files one by one (plain porcelain collapses them to "?? dir/");
    // -z leaves paths unquoted and NUL-separated, so spaces and unicode survive intact.
    let stdout = git_capture(root, &["status", "--porcelain", "-z", "-uall"], None)?;
    let mut fields = stdout.split(|b| *b == 0);
    while let Some(entry) = fields.next() {
        if entry.len() < 4 {
            continue;
        }
        let (code, path) = entry.split_at(3);
        let code = String::from_utf8_lossy(code).trim().to_string();
        if code.starts_with('R') || code.starts_with('C') {
            fields.next(); // a rename/copy carries its source in the next field
        }
        // A path git prints in bytes we cannot name back is left out entirely: feeding a
        // mangled pathspec to `git add` fails the *whole* checkpoint, and one unnameable
        // file is not worth losing the round's commit over.
        let Ok(path) = std::str::from_utf8(path) else { continue };
        if path == ".zloop" || path.starts_with(".zloop/") {
            continue;
        }
        snap.insert(path.to_string(), file_id(&root.join(path), &code));
    }
    Some(snap)
}

/// Size + mtime. Anything the host wrote during the round differs; anything nobody touched matches.
fn file_id(p: &Path, code: &str) -> String {
    match fs::metadata(p).and_then(|m| Ok((m.len(), m.modified()?))) {
        Ok((len, t)) => {
            let ns = t.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
            format!("{len}:{ns}")
        }
        Err(_) => format!("gone:{code}"),
    }
}

/// What a round's checkpoint took, and what it deliberately refused to take.
#[derive(Default)]
struct Checkpoint {
    sha: Option<String>,
    files: Vec<String>,
    /// Paths that were already dirty before the baseline was taken *and* changed during the round.
    /// Someone else's edits and ours are interleaved in the same file and cannot be split, so
    /// they stay out of a commit whose message names this todo.
    held_back: Vec<String>,
    /// True when the work tree holds nothing of ours anymore: everything that appeared since the
    /// baseline is either committed, or there was nothing to commit. Only then may the caller
    /// re-take the baseline — see the round-start re-snapshot in `run`.
    settled: bool,
}

/// One commit per round holding **only** what changed since `baseline`.
///
/// This used to be `git add -A -- .`, which swept the entire work tree in: a concurrent session's
/// half-written (or non-compiling) edits landed under "zloop tN: <our note>", and the runner
/// printed nothing but a sha. Now the baseline says what was already dirty, and the commit names
/// its paths explicitly — which also keeps anything a foreign session left *staged* out of it.
/// On success `baseline` is refreshed: whatever is still dirty after the commit is not ours.
///
/// The rule "not in the baseline ⇒ ours" is only as good as how fresh the baseline is, so the
/// caller re-takes it at the top of every round it safely can (`Checkpoint::settled`). What no
/// snapshot can settle is a file a neighbour creates *while our host is running*: both wrote in
/// the same window and the tree records no author. That one is committed as ours — which is why
/// the committed paths get printed and journalled, so it can at least be spotted afterwards.
fn git_checkpoint(root: &Path, todo_id: &str, note: &str, baseline: &mut DirtySnapshot) -> Checkpoint {
    let mut cp = Checkpoint::default();
    if git_capture(root, &["rev-parse", "--is-inside-work-tree"], None).is_none() {
        return cp;
    }
    // 读不出工作树就一步都别往下走：`settled` 留在 false，产物躺在树里等下一轮认领。
    let Some(now) = git_dirty(root) else {
        eprintln!("runner: 读不出工作树，这一轮不提交（产物留给下一轮认领）");
        return cp;
    };
    let mut ours: Vec<&str> = Vec::new();
    for (path, id) in &now {
        match baseline.get(path) {
            None => ours.push(path),                    // appeared while we were driving → ours
            Some(before) if before == id => {}          // foreign WIP nobody touched → leave it dirty
            Some(_) => cp.held_back.push(path.clone()), // foreign WIP the round also wrote → unsplittable
        }
    }
    if ours.is_empty() {
        cp.settled = true; // 没有一件是我们的 → 树里也没有我们欠着的东西
        return cp;
    }
    let pathspec: Vec<u8> = ours.iter().flat_map(|p| p.as_bytes().iter().copied().chain([0])).collect();
    // --pathspec-from-file has no argv limit and needs no quoting; .zloop never reaches it
    // (git exits 1 when an ignored path is named explicitly). 路径按**字节**喂进 stdin：
    // 叫不出名字的路径在 `git_dirty` 里就被剔掉了，这里剩下的都是 UTF-8。
    if git_capture(root, &["add", "--pathspec-from-file=-", "--pathspec-file-nul"], Some(pathspec.clone())).is_none() {
        return cp;
    }
    let msg = format!("zloop {todo_id}: {}", if note.is_empty() { "round" } else { note });
    let commit = &["commit", "-q", "-m", &msg, "--pathspec-from-file=-", "--pathspec-file-nul"];
    if git_capture(root, commit, Some(pathspec)).is_none() {
        return cp;
    }
    cp.files = ours.iter().map(|p| p.to_string()).collect();
    cp.sha =
        git_capture(root, &["rev-parse", "--short", "HEAD"], None).map(|o| String::from_utf8_lossy(&o).trim().to_string());
    // 提交成功之后基线才有资格重拍。重拍不成（git 这会儿挂了）就留着旧的：我们的东西
    // 已经进了 commit，树里不再有我们欠着的，`settled` 照样为真，下一轮开工前会再试一次。
    if let Some(fresh) = git_dirty(root) {
        *baseline = fresh;
    }
    cp.settled = true;
    cp
}

/// 给 git 子进程的闸。正常仓库里 status/add/commit 都是亚秒级，超大仓库的 `status`
/// 也就十几秒——60 秒足够宽松。**不复用 `--timeout-min`**：那是给宿主的，动辄几十分钟，
/// 装了等于没装。挂住的来源不是索引锁争用（那是秒失败），是 `pre-commit` 钩子、
/// `core.fsmonitor` 钩子、网络文件系统 stall（见 `docs/CODE-AUDIT.md` A-14）。
///
/// `log::changed_files`（`zloop done` 写回时读工作树）也用这一份：同一类挂法、同一个仓库，
/// 没有理由让人记两个旋钮。
pub(crate) fn git_timeout() -> Duration {
    env_secs("ZLOOP_GIT_TIMEOUT_SECS", 60)
}

/// 环境变量里的秒数，非法或 0 一律退回默认值。
pub(crate) fn env_secs(key: &str, default: u64) -> Duration {
    let n = std::env::var(key).ok().and_then(|s| s.trim().parse::<u64>().ok()).filter(|n| *n > 0).unwrap_or(default);
    Duration::from_secs(n)
}

/// 跑一条 git，**带闸**：超时或 `zloop stop` 时整组 TERM→KILL 收掉（`run_capture`），
/// 而不是像以前那样裸 `.output()` 无限期等下去。
///
/// 返回 `None` = 这一次 git 没跑成（起不来 / 超时 / 被叫停 / 非零退出）。四种失败故意合成
/// 一种：调用方对它们的处置都一样——**这一轮不提交**，产物留在树里等下一轮认领。
/// 挂住和被叫停会额外记一条账本（`git_stalled`），因为那两种事外面看不出来。
fn git_capture(root: &Path, args: &[&str], stdin_bytes: Option<Vec<u8>>) -> Option<Vec<u8>> {
    let mut c = Command::new("git");
    c.args(args).current_dir(root);
    let cap = match run_capture(c, git_timeout(), Group::Own, Stop::Honor, stdin_bytes) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("runner: git {} 起不来：{e}", args[0]);
            return None;
        }
    };
    if cap.timed_out || cap.interrupted {
        let how = if cap.timed_out { "timeout" } else { "interrupted" };
        eprintln!(
            "runner: git {} {}，已经整组收掉（{:?} 的闸）",
            args[0],
            if cap.timed_out { "超过闸没回来" } else { "被叫停" },
            git_timeout()
        );
        let lock = index_lock_left(root);
        let _ = journal_append(
            root,
            &json!({"event": "git_stalled", "cmd": args[0], "how": how,
                    "timeout_secs": git_timeout().as_secs(), "index_lock_left": lock, "at": state::now_iso()}),
        );
        return None;
    }
    if !cap.status.map(|s| s.success()).unwrap_or(false) {
        let why = String::from_utf8_lossy(&cap.stderr);
        if let Some(line) = why.lines().find(|l| !l.trim().is_empty()) {
            eprintln!("runner: git {} 失败：{line}", args[0]);
        }
        return None;
    }
    Some(cap.stdout)
}

/// 收掉一个挂住的 git 之后看一眼索引锁还在不在。
///
/// SIGTERM 的话 git 一般自己清掉了（A-14 实测），SIGKILL 的话会留在原地——留着的时候
/// **这个仓库后续所有 git 写操作都会失败**，包括人自己敲的。所以必须说出来。
/// 但**不自动删**：这把锁也可能是别人正在跑的 git 拿着的，删掉会毁掉对方的操作。
fn index_lock_left(root: &Path) -> bool {
    let lock = root.join(".git").join("index.lock");
    if !lock.exists() {
        return false;
    }
    eprintln!(
        "runner: {} 还在 —— 这个仓库后面所有 git 写操作都会失败。确认没有别的 git 在跑之后手动删掉它（zloop 不替你删）",
        lock.display()
    );
    true
}

/// Releases the keep-awake hold on *every* way out of `run()` — clean return, `?` error, or panic.
/// `stop()` normally does it first; a second release is a no-op (unregister and reconcile are idempotent).
struct AwakeGuard {
    root: PathBuf,
    armed: bool,
}

impl Drop for AwakeGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let r = crate::awake::release(std::process::id());
        if r.changed == Some(false) {
            let _ = journal_append(
                &self.root,
                &json!({"event": "awake_off", "holders_left": r.holders, "restored_default": true, "via": "guard", "at": state::now_iso()}),
            );
        }
    }
}

fn blocked_summary(st: &State) -> String {
    st.todos
        .iter()
        .filter(|t| t.status == "blocked" && t.blocked_by.iter().any(|d| d == crate::todo::USER))
        .map(|t| format!("- {} [P{}] {}：{}", t.id, t.priority, t.text, t.note))
        .collect::<Vec<_>>()
        .join("\n")
}

/// 上一轮**真的干过活的**宿主会话——`--resume` 接上去的就是它。
///
/// 判据是「这条 tick 是宿主结掉一轮留下的吗」（`tick::is_writeback`），不是「host 对不对」：
/// `ticks` 里带 session id 的不止宿主轮次。`zloop feedback` / `zloop edit` 把**人在另一个
/// 终端**的 `CLAUDE_CODE_SESSION_ID` 原样记进 tick（`session::detect()`），于是"人给这条
/// todo 留了句话" = "把自己的会话挂到这条 todo 名下"；只看 host + todo 的话，下一轮无头
/// runner 正好去捡它，`claude -p --resume <人的会话>` 让这一轮的提示词接在人那段对话
/// **后面**跑——上下文全是不相干的、token 按整段转录计费、产出写进人的转录里，人正开着
/// 那个会话就是两边同时往一条对话里写（A-19）。
///
/// 同一条过滤顺带挡掉另外两种「有 session、但不是上一轮干活的那个」：`zloop next` 在
/// should_run=false 时记的 `noop`（人敲的，`todo` 为空，只在 `All` 模式下够得着），
/// 以及 runner 自己插的 `reflect` / `replan` 轮次——那两轮本来就是 `--resume None` 起的
/// 一次性会话，不该成为下一轮工作的上文。
///
/// 宿主超时/失败时 runner 自己补的那条 `fail`（`tick::record("fail", …, who)`）用的是
/// 本轮宿主报回来的 session，是 `WRITEBACK` 成员——谱系不会因为一轮没写回就断掉。
fn pick_session(state: &State, host: Host, todo_id: &str, mode: ResumeMode) -> Option<String> {
    let host_round =
        |t: &&state::Tick| t.host.as_deref() == Some(host.as_str()) && t.session.is_some() && tick::is_writeback(&t.outcome);
    match mode {
        ResumeMode::None => None,
        ResumeMode::All => state.ticks.iter().rev().find(host_round).and_then(|t| t.session.clone()),
        ResumeMode::Todo => state
            .ticks
            .iter()
            .rev()
            .filter(host_round)
            .find(|t| t.todo.as_deref() == Some(todo_id))
            .and_then(|t| t.session.clone()),
    }
}

/// The child must not think it is inside *this* host session, and it must be able
/// to find a `zloop` binary. Our own directory is *appended* to PATH as a fallback:
/// prepending it would shadow the user's `claude` / `codex` when zloop lives next
/// to them (e.g. all in `~/.local/bin`).
/// runner 允许这一轮改计划时，额外放行的环境变量（见 `isolate_child_env`）。
pub const AUTO_REPLAN_ENV: &str = "ZLOOP_AUTO_REPLAN";

fn isolate_child_env(cmd: &mut Command, may_replan: bool) {
    cmd.env_remove("CLAUDE_CODE_SESSION_ID").env_remove("CLAUDECODE").env_remove("CODEX_THREAD_ID");
    // `claude -p` loads the project's hooks, including our own Stop hook. Mark the child so
    // `zloop hook-stop` lets the host exit after exactly one todo instead of chaining them.
    cmd.env("ZLOOP_RUNNER", "1");
    // 默认情况下无头轮次**不许改计划**。这条红线以前只写在提示词里——而这整个功能的前提
    // 就是"提示词管不住模型"（回归测试里那个假宿主真的抗命跑了一次 `replan --apply`，
    // 而且成功了）。所以改成代码闸：子进程里没有这个变量，`replan --apply` 直接拒绝。
    if may_replan {
        cmd.env(AUTO_REPLAN_ENV, "1");
    } else {
        cmd.env_remove(AUTO_REPLAN_ENV);
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let old = std::env::var("PATH").unwrap_or_default();
            cmd.env("PATH", format!("{old}:{}", dir.display()));
        }
    }
}

struct Captured {
    status: Option<ExitStatus>,
    stdout: String,
    stderr: String,
    timed_out: bool,
    interrupted: bool,
}

/// 子进程走了之后还等多久管道 EOF。**杀掉直接子进程不等于管道关了**：它留下的孙进程
/// 继承了同一个写端，只要孙进程还活着，`read` 就永远等不到 EOF（A-6）。所以排水必须有上限——
/// 宁可少记一段 stdout，也不能把 runner 钉死在这里，`--timeout-min` 和 `zloop stop` 都指望它。
/// 直接子进程一旦收掉，它自己的输出**已经全在管道缓冲里**，这 2 秒只是读出来的时间，够用。
const DRAIN_GRACE: Duration = Duration::from_secs(2);
/// 给整组的收尾时间：先 SIGTERM 让它自己收拾，过了这个点就 SIGKILL。
const GROUP_TERM_GRACE: Duration = Duration::from_millis(500);

/// 子进程放进哪个进程组——**这是个取舍，两边都会漏掉一类进程**：
///
/// * `Own`：单开一组，超时/叫停时 `killpg` 整组，连它 fork 出来的孙进程（钩子、后台任务）
///   一起收掉。代价是**跟调用者的组脱钩**：调用者自己被上层 `killpg` 收掉时，这个子进程
///   收不到信号，会变成挂着不动的孤儿。
/// * `Inherit`：跟着调用者的组，调用者被整组收掉时它一起走，不留孤儿。代价是收不掉孙进程。
///
/// 按调用者的命长挑：runner 是长命进程、自己装了信号处置、还要防孙进程占管道 → `Own`；
/// 短命的 CLI（`zloop done` 跑在宿主里，随时可能被 `--timeout-min` 的 `killpg` 收掉）→ `Inherit`。
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Group {
    Own,
    Inherit,
}

/// 这个子进程认不认 `zloop stop`——**收尾动作必须不认**。
///
/// * `Honor`：`stop_requested()` 一置位就整组收掉。**干活的**子进程都该这样，`zloop stop`
///   才叫得动（A-6 / A-14）：宿主、git 检查点、通知，都是这一档。
/// * `Ignore`：只认超时，不认叫停。**恢复系统设置这一类收尾动作只能这样**——`zloop stop`
///   发的 SIGTERM 先把标志置上，之后才走到 `stop()` → `awake::release()`。那几条 `pmset`
///   探针要是也认叫停，会在第一次轮询里被自己杀掉：`sleep_disabled()` 读不出来、
///   `set_sleep_disabled()` 报失败，于是 `SleepDisabled=1` 原地留着没人恢复——
///   「装了闸」把功能弄没了。超时那一半仍然在，所以它照样挂不住。
///
/// 判据是**这条命令是不是在替我们收拾现场**：是 → `Ignore`，其余一律 `Honor`。
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Stop {
    Honor,
    Ignore,
}

/// 往一个 pid（正数）或**整个进程组**（负数）发信号。
#[cfg(unix)]
fn signal_to(target: i32, sig: i32) {
    extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    unsafe {
        kill(target, sig);
    }
}
#[cfg(not(unix))]
fn signal_to(_target: i32, _sig: i32) {}

/// 收掉这一轮起的进程：SIGTERM → 等一会儿 → SIGKILL → 收尸。
///
/// **先 TERM 再 KILL 不是客套**：git 收到 TERM 会自己清掉 `.git/index.lock`，被 KILL 掉则
/// 把锁留在原地，之后这个仓库所有 git 写操作（包括人自己敲的）全部失败（A-14 实测）。
///
/// `Own` 时信号发给整组（负 pid），`Inherit` 时只发给直接子进程——后者的 pid 不是组 id，
/// 拿它当组 id 去 `killpg` 是在赌运气，孙进程只能留给调用者的组去收。
fn stop_group(child: &mut std::process::Child, group: Group) {
    let pid = child.id() as i32;
    let target = if group == Group::Own { -pid } else { pid };
    signal_to(target, 15);
    let grace = Instant::now() + GROUP_TERM_GRACE;
    while Instant::now() < grace {
        match child.try_wait() {
            Ok(Some(_)) | Err(_) => break,
            Ok(None) => thread::sleep(Duration::from_millis(20)),
        }
    }
    // 孙进程可以不理 SIGTERM，没走干净的一律 SIGKILL；
    // 组信号送不到时（非 unix）至少还有 `kill()` 收掉直接子进程。
    signal_to(target, 9);
    let _ = child.kill();
    let _ = child.wait();
}

/// 一根管道的排水线程：边读边往共享缓冲里堆。放弃等 EOF 时，已经读到的那半截照样拿得走。
struct Drain {
    handle: thread::JoinHandle<()>,
    buf: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
}

fn drain_pipe<R: Read + Send + 'static>(mut r: R) -> Drain {
    let buf = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = std::sync::Arc::clone(&buf);
    let handle = thread::spawn(move || {
        let mut chunk = [0u8; 8192];
        loop {
            match r.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => sink.lock().unwrap_or_else(|e| e.into_inner()).extend_from_slice(&chunk[..n]),
            }
        }
    });
    Drain { handle, buf }
}

impl Drain {
    /// 等 EOF，最多等到 `deadline`。返回 (读到的内容, 是否读全)。
    ///
    /// 没读全就把线程扔在那儿不 join：它还堵在孙进程占着的管道上，join 就是重新挂死。
    /// 代价是一个线程 + 一个 fd 留到进程退出——比 runner 永远不结束便宜。
    fn collect(self, deadline: Instant) -> (Vec<u8>, bool) {
        while !self.handle.is_finished() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        let complete = self.handle.is_finished();
        let bytes = self.buf.lock().unwrap_or_else(|e| e.into_inner()).clone();
        (bytes, complete)
    }
}

/// 一次带闸的子进程调用的原始结果。
pub(crate) struct CapturedBytes {
    pub status: Option<ExitStatus>,
    /// **字节**，不是 `String`。`git status -z` 的路径可能不是 UTF-8，过一遍 `from_utf8_lossy`
    /// 会把「叫不出名字的路径」变成「叫得出但是错的」，拿它去 `git add` 会让整个 checkpoint
    /// 失败（见 `git_dirty` 里那段注释）。要文本的调用方自己转。
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub timed_out: bool,
    pub interrupted: bool,
    /// 管道读到 EOF 了没有。false = 输出是**截断**的（孙进程还占着写端）。
    pub drained: bool,
}

/// 起一个子进程并**把闸装上**：进程组 + 超时 + `zloop stop` + 排水上限，一处实现。
///
/// **每一个 zloop 起的、要等它退出的子进程都必须走这里。** 裸 `.output()` /
/// `wait_with_output()` / `.status()` 是无限期的阻塞等待，既不看超时也不看
/// `stop_requested()`——git 钩子一挂住，runner 就跟着挂住，而且 SIGTERM 叫不动
/// （A-6 / A-14 是同一种死法的两条路）。
///
/// 只有两类子进程有资格留在外面，而且必须在原地写明理由（见 `awake.rs`，docs/CODE-AUDIT.md §18）：
/// **detach 出去、故意比我们活得久的**（`caffeinate`、看门狗——这里等它退出就是等到天亮），
/// 和**要人手在终端上打字的**（`sudo install` 那一下要输密码：这里 stdin 是 `/dev/null`，
/// stdout/stderr 是管道，密码提示根本到不了人眼前，超时也无从定）。
///
/// `stdin_bytes` 为 `Some` 时把这些字节喂给子进程的 stdin 再关掉（EOF）。
/// `group` 决定超时/叫停时收得掉谁、收不掉谁，见 [`Group`]；`stop` 决定它认不认
/// `zloop stop`，见 [`Stop`]。
pub(crate) fn run_capture(
    mut cmd: Command,
    timeout: Duration,
    group: Group,
    stop: Stop,
    stdin_bytes: Option<Vec<u8>>,
) -> Result<CapturedBytes> {
    let what = cmd.get_program().to_string_lossy().into_owned();
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    if stdin_bytes.is_some() {
        cmd.stdin(Stdio::piped());
    } else {
        cmd.stdin(Stdio::null());
    }
    // 单开一个进程组，这样超时/被叫停时可以 `killpg` 整组，把子进程留下的后台孙进程一起收掉。
    // 副作用是终端的 Ctrl-C 不再直接送到子进程——runner 自己装了 SIGINT 处置，会替它收（≤200ms）。
    #[cfg(unix)]
    if group == Group::Own {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut child = cmd.spawn().with_context(|| format!("spawning `{what}` (is it on PATH?)"))?;
    // stdin 在**另一个线程**上写，不在这儿等：子进程完全可以一个字节都不读（钩子先挂住了），
    // 那样一个阻塞的 `write_all` 会绕过下面这个闸，装了白装。写完线程结束、管道关闭 = EOF。
    if let Some(bytes) = stdin_bytes {
        if let Some(mut w) = child.stdin.take() {
            thread::spawn(move || {
                let _ = w.write_all(&bytes);
            });
        }
    }
    let d_out = drain_pipe(child.stdout.take().expect("piped stdout"));
    let d_err = drain_pipe(child.stderr.take().expect("piped stderr"));
    let deadline = Instant::now() + timeout;
    let mut timed_out = false;
    let mut interrupted = false;
    let status = loop {
        if let Some(st) = child.try_wait()? {
            break Some(st);
        }
        if stop == Stop::Honor && stop_requested() {
            stop_group(&mut child, group);
            interrupted = true;
            break None;
        }
        if Instant::now() >= deadline {
            stop_group(&mut child, group);
            timed_out = true;
            break None;
        }
        thread::sleep(Duration::from_millis(200));
    };
    let drain_deadline = Instant::now() + DRAIN_GRACE;
    let (stdout, out_ok) = d_out.collect(drain_deadline);
    let (stderr, err_ok) = d_err.collect(drain_deadline);
    Ok(CapturedBytes { status, stdout, stderr, timed_out, interrupted, drained: out_ok && err_ok })
}

/// `run_capture` 的文本版，给宿主用：stdout/stderr 转成 `String`（宿主输出本来就是 JSON/文本）。
fn run_with_timeout(cmd: Command, timeout: Duration, what: &str) -> Result<Captured> {
    let cap = run_capture(cmd, timeout, Group::Own, Stop::Honor, None)?;
    if !cap.drained {
        // 说出来：这一轮的输出是**截断**的，别让下游把半截 JSON 当成宿主的完整回话。
        eprintln!("runner: `{what}` 退出后管道还被它留下的后台进程占着，这一轮的输出只记到 {DRAIN_GRACE:?} 为止");
    }
    Ok(Captured {
        status: cap.status,
        stdout: String::from_utf8_lossy(&cap.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&cap.stderr).into_owned(),
        timed_out: cap.timed_out,
        interrupted: cap.interrupted,
    })
}

struct HostResult {
    session: Option<String>,
    exit_ok: bool,
    timed_out: bool,
    interrupted: bool,
    rate_limited: bool,
    /// 宿主这一轮说的话，**全文**（拿不到 result 就退回 stderr）。
    ///
    /// 别在这里截断：回看 / 重估那两种轮次不写回账本，宿主的输出就是它们**唯一**的产物，
    /// 截在 300 字会把建议清单砍掉大半。要摘要的地方（tick.note、控制台）自己截。
    output: String,
    cost_usd: Option<f64>,
    num_turns: Option<u64>,
    duration_ms: Option<u64>,
}

/// 落进 tick.note 的那一句：账本只存摘要，全文在 `.zloop/log/` 里。
fn ledger_note(output: &str, max: usize) -> String {
    crate::style::truncate(&output.replace('\n', " "), max)
}

fn looks_rate_limited(text: &str) -> bool {
    let lower = text.to_lowercase();
    RATE_LIMIT_MARKERS.iter().any(|m| lower.contains(m))
}

fn run_claude(
    root: &Path,
    prompt: &str,
    resume: Option<&str>,
    opts: &Options,
    timeout: Duration,
    may_replan: bool,
) -> Result<HostResult> {
    let mut cmd = Command::new("claude");
    cmd.current_dir(root).arg("-p").arg(prompt).arg("--output-format").arg("json");
    if let Some(sid) = resume {
        cmd.arg("--resume").arg(sid);
    }
    if let Some(b) = &opts.max_budget_usd {
        cmd.arg("--max-budget-usd").arg(b);
    }
    if opts.allow_all {
        cmd.arg("--dangerously-skip-permissions");
    } else {
        cmd.arg("--allowedTools").arg("Bash(zloop:*),Read,Edit,Write,MultiEdit,Glob,Grep");
        cmd.arg("--permission-mode").arg("acceptEdits");
    }
    isolate_child_env(&mut cmd, may_replan);
    let started = Instant::now();
    let cap = run_with_timeout(cmd, timeout, "claude")?;
    let elapsed_ms = started.elapsed().as_millis() as u64;
    let parsed: Option<Value> = serde_json::from_str(cap.stdout.trim()).ok();
    let get = |k: &str| parsed.as_ref().and_then(|v| v.get(k).cloned());
    let session = get("session_id").and_then(|v| v.as_str().map(str::to_string));
    let ok_status = cap.status.map(|s| s.success()).unwrap_or(false);
    let is_error = get("is_error").and_then(|v| v.as_bool()).unwrap_or(!ok_status);
    let result_text = get("result").and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
    let rate_limited = (is_error || !ok_status) && looks_rate_limited(&format!("{result_text}\n{}", cap.stderr));
    let output = if !result_text.is_empty() { result_text } else { cap.stderr };
    Ok(HostResult {
        session,
        exit_ok: ok_status && !is_error,
        timed_out: cap.timed_out,
        interrupted: cap.interrupted,
        rate_limited,
        output,
        cost_usd: get("total_cost_usd").and_then(|v| v.as_f64()),
        num_turns: get("num_turns").and_then(|v| v.as_u64()),
        duration_ms: get("duration_ms").and_then(|v| v.as_u64()).or(Some(elapsed_ms)),
    })
}

fn run_codex(
    root: &Path,
    prompt: &str,
    resume: Option<&str>,
    opts: &Options,
    timeout: Duration,
    may_replan: bool,
) -> Result<HostResult> {
    let last_msg = root.join(state::STATE_DIR).join("runner").join("codex-last-message.txt");
    if let Some(p) = last_msg.parent() {
        fs::create_dir_all(p)?;
    }
    let _ = fs::remove_file(&last_msg);
    let mut cmd = Command::new("codex");
    cmd.current_dir(root).arg("exec");
    if let Some(sid) = resume {
        cmd.arg("resume").arg(sid);
    }
    cmd.arg("--json").arg("--skip-git-repo-check").arg("-C").arg(root).arg("--output-last-message").arg(&last_msg);
    if opts.allow_all {
        cmd.arg("--dangerously-bypass-approvals-and-sandbox");
    } else {
        cmd.arg("--sandbox").arg("workspace-write");
    }
    cmd.arg(prompt);
    isolate_child_env(&mut cmd, may_replan);
    let started = Instant::now();
    let cap = run_with_timeout(cmd, timeout, "codex")?;
    let elapsed_ms = started.elapsed().as_millis() as u64;
    let mut session = None;
    for line in cap.stdout.lines() {
        if let Ok(v) = serde_json::from_str::<Value>(line) {
            if let Some(id) = v.get("thread_id").and_then(Value::as_str) {
                if session.is_none() || v.get("type").and_then(Value::as_str) == Some("thread.started") {
                    session = Some(id.to_string());
                }
            }
        }
    }
    let last = fs::read_to_string(&last_msg).unwrap_or_default();
    let ok_status = cap.status.map(|s| s.success()).unwrap_or(false);
    let rate_limited = !ok_status && looks_rate_limited(&format!("{}\n{}", cap.stderr, cap.stdout));
    let output = if !last.trim().is_empty() { last } else { cap.stderr };
    Ok(HostResult {
        session,
        exit_ok: ok_status,
        timed_out: cap.timed_out,
        interrupted: cap.interrupted,
        rate_limited,
        output,
        cost_usd: None,
        num_turns: None,
        duration_ms: Some(elapsed_ms),
    })
}

fn secs(units: u32, fast: bool) -> u64 {
    if fast {
        units as u64
    } else {
        units as u64 * 60
    }
}

/// Record when the runner will wake up so `zloop status` can show "sleeping until …".
fn journal_sleep(root: &Path, units: u32, fast: bool, reason: &str) -> Result<()> {
    let until = state::now() + chrono::Duration::seconds(secs(units, fast) as i64);
    journal_append(
        root,
        &json!({"event": "sleep", "until": state::format_iso(&until), "reason": reason, "at": state::now_iso()}),
    )
}

/// 最慢的那一档，用在 `decide` 没给间隔的两处（等人 / 被限流）。
///
/// 走 `clamp_interval` 而不是直接读 `last()`：这是 `policy.intervals_min` 的**第二个读者**，
/// 绕过了 `tick::interval` 的封顶。`intervals_min = [3, 4294967295]` 时 `decide` 给的间隔
/// 是正常的 3，而这里给出 4294967295 分钟 → `secs()` 换算成 8171 年的 sleep，
/// 同一份数据的两个读者要过同一道闸。
fn slowest_interval(state: &State) -> u32 {
    tick::clamp_interval(state.policy.intervals_min.last().copied().unwrap_or(30))
}

/// Decide how long to sleep for a non-running decision, or `None` to stop the runner.
///
/// `--exit-on-wait` 在 `interval_min` **之前**判：「等人要不要退出」是标志说了算，不是
/// noop 计数说了算。原来的顺序（先 match `interval_min`，`Some` 直接返回、只有 `None`
/// 那一支才问标志）让这个标志在真 runner 上变成死代码——`decide` 只有在
/// `noop_streak >= max_noop_streak` 时才给 `None`，而 runner 在 `!should_run` 那一支
/// 只写 journal 的 sleep，一条 noop tick 都不记，所以 `noop_streak` 恒为 0。实测抓到过
/// 一个带着 `--exit-on-wait` 的 runner 在 user_gate 上转了 20 小时（A-5）。
///
/// `throttled` 同理，而且它是最后一处把 noop 计数当停机开关的地方（A-16）。配额窗口
/// 是**自己会滑过去**的：`decide` 连还差几分钟都算出来了，等下去一定等得到，退出等于
/// 把长跑掐了。而 `decide` 在 `noop_streak >= max_noop_streak` 时会把这一支的
/// `interval_min` 翻成 `None`——runner 自己一条 noop 都不记，但 **`zloop next` 记**，
/// 两边读的是同一本账。于是人在终端里敲三下 `zloop next`（就想看一眼现在什么情况），
/// 就把「runner 睡到窗口放开再接着跑」变成了「runner 拒绝启动 / 下次醒来直接退出」。
/// 实测 A/B：不敲 → `sleep until 11:57 (throttled)`；敲三下 → `stop (throttled)`，
/// 连 `zloop start` 都当场拒绝。所以这里和等人一样：说法由 runner 定，不由计数定。
///
/// 修完之后 `max_noop_streak` 对 runner 的调度**再无任何影响**（三条非终态出口
/// user_gate / blocked / throttled 全部改成必睡），README 里那句「runner 不受此影响」
/// 才是真的。它只剩一个消费者：交互式 `zloop next` 的退避提示。
pub fn wait_plan(state: &State, d: &tick::Decision, opts: &Options) -> Option<(u32, String)> {
    let human = d.reason == "user_gate" || d.reason == "blocked";
    if human {
        if opts.exit_on_wait {
            return None;
        }
        // 等人时的说法只有一种：不管 `decide` 给的是哪一档间隔（还是压根没给），
        // runner 在做的都是同一件事——替人守着这条 todo，等人回来解开。
        let m = d.interval_min.unwrap_or_else(|| slowest_interval(state));
        return Some((m, format!("{} (polling until a human unblocks)", d.reason)));
    }
    if d.reason == "throttled" {
        // 等的是时间，不是人，所以 `--exit-on-wait` 不管这一支。
        let m = d.interval_min.unwrap_or_else(|| slowest_interval(state));
        return Some((m, format!("{} (sleeping until the quota window frees)", d.reason)));
    }
    d.interval_min.map(|m| (m, d.reason.clone()))
}

/// 启动前体检：如果 runner 第一轮就会直接退出，返回那个 reason。
///
/// 走的是 `run` 循环里一模一样的两步（`tick::decide` → `wait_plan`），不另立一套规则：
/// 另写一份判断迟早会和调度器漂开，那时 `start` 要么拦错、要么又开始秒退。
pub fn immediate_stop_reason(state: &State, opts: &Options, at: chrono::DateTime<chrono::FixedOffset>) -> Option<String> {
    let d = tick::decide(state, at);
    if d.should_run {
        return None;
    }
    wait_plan(state, &d, opts).is_none().then_some(d.reason)
}

pub fn run(root: &Path, opts: Options) -> Result<i32> {
    let path = state::state_path(root);
    let host_label = match opts.host {
        Host::Codex => "codex-cli",
        _ => "claude",
    };
    let timeout = Duration::from_secs(secs(opts.timeout_min.max(1), opts.fast));
    install_signal_handlers();
    crate::daemon::write_pid(root, std::process::id())?;
    if let Some(last) = last_journal_event(root) {
        let kind = last.get("event").and_then(Value::as_str).unwrap_or("").to_string();
        if kind != "stop" {
            if kind == "begin" {
                eprintln!(
                    "runner: previous run ended mid-round (round {}); continuing from current state",
                    last.get("round").unwrap_or(&json!(null))
                );
            } else {
                eprintln!("runner: previous run did not stop cleanly (last event: {kind}); continuing from current state");
            }
            journal_append(root, &json!({"event": "restart", "after": kind, "at": state::now_iso()}))?;
        }
    }
    let mut awake_guard = AwakeGuard { root: root.to_path_buf(), armed: false };
    if opts.keep_awake && crate::awake::supported() {
        let acq = crate::awake::acquire(root, std::process::id());
        awake_guard.armed = true;
        journal_append(
            root,
            &json!({"event": "awake_on", "lid": acq.lid, "caffeinate_pid": acq.caffeinate_pid, "at": state::now_iso()}),
        )?;
        match (&acq.hint, acq.lid) {
            (Some(h), _) => println!("runner: keep-awake: {h}"),
            (None, true) => println!(
                "runner: keep-awake: lid-close sleep disabled while this runner lives (caffeinate pid {:?})",
                acq.caffeinate_pid
            ),
            (None, false) => {}
        }
    }
    let _awake_guard = awake_guard;
    let mut rounds_done: u32 = 0;
    let mut last_reflect: Option<u32> = None;
    let mut replan_at: Option<u64> = None;
    // 上一次重估时「在等人回话」的那批 todo。`blocked` 是这几个信号里唯一的**锁存**：
    // 其余四个都从近期活动推出来、会自然衰减，而它一旦挂上，在无头模式下没人能来解，
    // 于是每一轮都会放炮。踩过：一次 4 小时的长跑里 5 次重估全由同一条 `t21 在等你回话`
    // 触发，占掉全程花费的两成多。所以对它按**边沿**处理——有新的 todo 开始等人才响。
    // 不是「只响一次」：那次实测里第 16 轮只给出判断、第 17 轮才产出重算窗口的证据表。
    let mut replan_blocked: Option<String> = None;
    // 自主改计划的账：改过几次、上一次改完还剩几条（用来看清单是不是在越改越长）
    let mut auto_replans: u32 = 0;
    let mut grew_in_a_row: u32 = 0;
    let mut stop_after_replan: Option<String> = None;
    let mut notified: Option<String> = None; // dedupe: one notification per distinct wait/limit situation
                                             // 基线以外的脏东西 = 不是我们干的。每轮 checkpoint 只提交这条线之后的变化，
                                             // 别人的在制品不会被卷进「zloop tN: <我的 note>」。
    let mut git_baseline = DirtySnapshot::new();
    // 基线是不是「结清」的：树里已经没有我们欠着没提交的东西。为真才允许重拍基线。
    // 起跑那一刻按结清算——第一轮开工前会拍下第一张。
    let mut git_baseline_settled = opts.git_commit;
    loop {
        if stop_requested() {
            return stop(root, "sigterm");
        }
        let st = state::load(&path)?;
        let d = tick::decide(&st, state::now());
        if !d.should_run {
            match wait_plan(&st, &d, &opts) {
                None => return stop(root, &d.reason),
                Some((m, reason)) => {
                    println!("runner: wait ({reason}) · sleeping {} {}", m, if opts.fast { "s" } else { "min" });
                    if (d.reason == "user_gate" || d.reason == "blocked") && notified.as_deref() != Some("wait") {
                        notify(root, &st, "wait", &blocked_summary(&st));
                        notified = Some("wait".into());
                    }
                    journal_sleep(root, m, opts.fast, &reason)?;
                    if !sleep_interruptible(Duration::from_secs(secs(m, opts.fast))) {
                        return stop(root, "sigterm");
                    }
                    continue;
                }
            }
        }
        notified = None; // a runnable round resets the dedupe window

        // 开工前重拍一次基线。上一轮收尾到这一刻这段时间里（睡眠、等人、回看之间），
        // 我们一个宿主都没在跑，所以这时候新冒出来的脏东西一件都不是我们的。
        // 不重拍的话，起跑时拍的那一张会一直用到停机：邻居在长跑中途新建的文件，
        // 因为「基线里没有」就被下一次 checkpoint 认成我们的（t16 只挡住了起跑前就脏着的）。
        //
        // **只在上一轮结清了才重拍**，这是这里唯一的取舍：某一轮没写回、或者 add/commit
        // 失败，那一轮的产物还躺在树里等下一轮认领，基线一重拍它们就被永远划给别人、
        // 再也提交不了。宁可多认（提错了还能从 git 里挑出来），不可漏认（活直接丢）。
        if git_baseline_settled {
            match git_dirty(root) {
                Some(snap) => {
                    git_baseline = snap;
                    git_baseline_settled = false; // 马上就要放宿主进来写了
                }
                // 读不出来（git 挂住被闸收掉，或者报错）：**留着上一张基线**，下一轮再试。
                // 换成空快照会把树里所有脏东西都认成我们的——那才是真会把邻居的活提交进去。
                None => eprintln!("runner: 开工前读不出工作树，沿用上一张基线（checkpoint 会更保守）"),
            }
        }
        // git 挂住时的 SIGTERM 是在上面那几个 git 子进程里被看见的，别接着往下起宿主。
        if stop_requested() {
            return stop(root, "sigterm");
        }

        // 攒够 N 轮就回看一次。只在两轮 todo 之间插入，所以它不占 todo 轮次，
        // 也不会因为 `rounds_done` 没变而连着触发（`last_reflect` 记住上次是在第几轮插的）。
        if opts.reflect_every > 0
            && rounds_done > 0
            && rounds_done.is_multiple_of(opts.reflect_every)
            && last_reflect != Some(rounds_done)
        {
            last_reflect = Some(rounds_done);
            let text = format!(
                "{}\n\n---\n\n**这一轮由 zloop runner 无头驱动，没有人在旁边点头**：所以只输出建议清单，\
                 **不要**运行 `zloop reflect --apply`，也不要改任何代码或 todo。你的输出会原样记进账本，等人回来看。\n",
                crate::reflect::packet(&st, root, crate::notes::WINDOW, crate::notes::RULE_LIMIT)
            );
            journal_append(root, &json!({"event": "reflect", "after_round": rounds_done, "at": state::now_iso()}))?;
            println!("runner: 第 {rounds_done} 轮之后插一轮回看（不占轮次）");
            let result = match opts.host {
                Host::Codex => run_codex(root, &text, None, &opts, timeout, false)?,
                _ => run_claude(root, &text, None, &opts, timeout, false)?,
            };
            let who = HostSession { host: opts.host, session: result.session.clone() };
            // 回看不写回账本：这份全文就是它唯一的产物，一个字都不能少。
            let body = result.output.trim().to_string();
            let summary = body.lines().find(|l| !l.trim().is_empty()).unwrap_or("(没有输出)");
            let rel = crate::log::write_raw(root, "reflect", &format!("# 回看 · 第 {rounds_done} 轮之后\n\n{body}\n"))?;
            state::transaction(&path, |st| {
                let t = tick::record(st, tick::REFLECT, None, &crate::style::truncate(summary, 200), &who)?;
                if let Some(last) = st.ticks.last_mut() {
                    last.log = Some(rel.clone());
                    last.cost_usd = result.cost_usd;
                    last.duration_ms = result.duration_ms;
                }
                Ok(t)
            })?;
            println!("runner: 回看写进账本 · {rel}");
            continue;
        }

        let todo = d.todo.clone().expect("ready decision carries a todo");
        let round_no = tick::current_round(&st.ticks) + 1;
        // 持有者记录里写「run 第 N 轮」而不是光一个 run：被挡住的人一眼知道是哪一轮在写回。
        state::set_operation(format!("run 第 {round_no} 轮"));
        let ticks_before = st.ticks.len();
        let resume_sid = pick_session(&st, opts.host, &todo.id, opts.resume);

        // Preflight (Anthropic harness: verify the environment before touching code).
        let mut preflight_note = String::new();
        if let Some(cmd) = st.policy.preflight_cmd.clone() {
            match preflight(root, &cmd, timeout) {
                Ok(summary) => preflight_note = format!("\n环境自检（{cmd}）通过：{summary}"),
                Err(why) => {
                    println!("runner: round {round_no} {why}");
                    let who = HostSession { host: opts.host, session: None };
                    state::transaction(&path, |st| {
                        tick::record(st, "fail", Some(&todo.id), &format!("runner: {why}"), &who)?;
                        Ok(())
                    })?;
                    journal_append(
                        root,
                        &json!({"event": "preflight_failed", "round": round_no, "todo": todo.id, "at": state::now_iso()}),
                    )?;
                    let st = state::load(&path)?;
                    let d = tick::decide(&st, state::now());
                    match wait_plan(&st, &d, &opts) {
                        None => return stop(root, &d.reason),
                        Some((m, reason)) => {
                            journal_sleep(root, m, opts.fast, &reason)?;
                            if !sleep_interruptible(Duration::from_secs(secs(m, opts.fast))) {
                                return stop(root, "sigterm");
                            }
                            continue;
                        }
                    }
                }
            }
        }

        let mut text = prompt::heartbeat(&st, host_label, root)?;
        text.push_str(&preflight_note);
        text.push_str(&format!(
            "\n\n本轮由 zloop runner 无头驱动。当前 todo：{} [P{}] {}\n本轮结束前必须执行写回命令 `zloop done {} …`（或 --outcome progress/fail、--block）。不要询问用户，无法继续就用 --block 说明。",
            todo.id, todo.priority, todo.text, todo.id
        ));
        journal_append(
            root,
            &json!({"event": "begin", "round": round_no, "todo": todo.id, "host": opts.host.as_str(),
                    "resume": resume_sid, "at": state::now_iso()}),
        )?;
        state::transaction(&path, |st| {
            st.in_progress = Some(state::InProgress {
                todo: todo.id.clone(),
                started_at: state::now_iso(),
                round: round_no,
                via: "runner".into(),
                host: Some(opts.host.as_str().to_string()),
                session: resume_sid.clone(),
            });
            Ok(())
        })?;
        println!(
            "runner: round {round_no} → {} [{}]{}",
            todo.id,
            opts.host.as_str(),
            resume_sid.as_deref().map(|s| format!(" resume {s}")).unwrap_or_default()
        );

        let result = match opts.host {
            Host::Codex => run_codex(root, &text, resume_sid.as_deref(), &opts, timeout, false)?,
            _ => run_claude(root, &text, resume_sid.as_deref(), &opts, timeout, false)?,
        };

        // Settlement: did the host write back?
        let who = HostSession { host: opts.host, session: result.session.clone() };
        let (wrote_back, rate_limited) = state::transaction(&path, |st| {
            let mut wrote = false;
            for i in ticks_before..st.ticks.len() {
                let t = &mut st.ticks[i];
                // 只认宿主真结掉一轮的四种 outcome。问「账本长没长」会把人在另一个
                // 终端敲的 `zloop feedback` / `edit`（还有本轮自己插的 replan/reflect）
                // 当成宿主的写回，于是失败轮次不记 fail、fail_streak 恒为 0（A-17）。
                if tick::is_writeback(&t.outcome) {
                    wrote = true;
                }
                if t.session.is_none() {
                    t.host = Some(opts.host.as_str().to_string());
                    t.session = who.session.clone();
                }
            }
            let rate_limited = !wrote && !result.timed_out && result.rate_limited;
            if !wrote && !rate_limited && !result.interrupted {
                let note = if result.timed_out {
                    format!("runner: host timed out after {} {}", opts.timeout_min, if opts.fast { "s" } else { "min" })
                } else if result.exit_ok {
                    "runner: host finished without writing back".to_string()
                } else {
                    format!("runner: host failed: {}", ledger_note(&result.output, 300))
                };
                tick::record(st, "fail", Some(&todo.id), &note, &who)?;
            }
            // Attach what the host reported about this round to the tick that closes it.
            // **结掉这一轮的那条**，不是「最后一条」：人在宿主退出后补的 `zloop feedback`
            // 会排在写回后面，`ticks.last_mut()` 就把这一轮的花费/轮数/日志挂到人那条上，
            // 账本从此对不上号（A-17 的第二个后果）。一条都没有 = 这一轮没人结算，别乱挂。
            let closer = st.ticks.iter().rposition(|t| tick::is_writeback(&t.outcome)).filter(|i| *i >= ticks_before);
            if let Some(closing) = closer.map(|i| &mut st.ticks[i]) {
                if closing.cost_usd.is_none() {
                    closing.cost_usd = result.cost_usd;
                }
                if closing.num_turns.is_none() {
                    closing.num_turns = result.num_turns;
                }
                if closing.duration_ms.is_none() {
                    closing.duration_ms = result.duration_ms;
                }
                if let Some(rel) = closing.log.clone() {
                    let line = format!(
                        "- cost: {}   turns: {}   duration: {}   (runner settlement)",
                        closing.cost_usd.map(|c| format!("${c:.4}")).unwrap_or_else(|| "-".into()),
                        closing.num_turns.map(|n| n.to_string()).unwrap_or_else(|| "-".into()),
                        closing.duration_ms.map(|d| format!("{}s", d / 1000)).unwrap_or_else(|| "-".into()),
                    );
                    let _ = crate::log::append(root, &rel, &line);
                }
            }
            st.in_progress = None; // round settled either way
            Ok((wrote, rate_limited))
        })?;
        journal_append(
            root,
            &json!({"event": "end", "round": round_no, "todo": todo.id, "wrote_back": wrote_back,
                    "exit_ok": result.exit_ok, "timed_out": result.timed_out, "rate_limited": rate_limited,
                    "interrupted": result.interrupted, "session": result.session, "at": state::now_iso()}),
        )?;
        if result.interrupted {
            println!("runner: round {round_no} interrupted by stop request");
            return stop(root, "sigterm");
        }
        if rate_limited {
            let st = state::load(&path)?;
            let m = slowest_interval(&st);
            println!(
                "runner: round {round_no} host rate-limited · not counted · sleeping {} {} · {}",
                m,
                if opts.fast { "s" } else { "min" },
                result.output.lines().next().unwrap_or("").chars().take(100).collect::<String>()
            );
            if notified.as_deref() != Some("rate_limited") {
                notify(
                    root,
                    &st,
                    "rate_limited",
                    &format!(
                        "{} {} 后重试：{}",
                        m,
                        if opts.fast { "秒" } else { "分钟" },
                        result.output.lines().next().unwrap_or("").chars().take(120).collect::<String>()
                    ),
                );
                notified = Some("rate_limited".into());
            }
            journal_sleep(root, m, opts.fast, "host_rate_limited")?;
            if !sleep_interruptible(Duration::from_secs(secs(m, opts.fast))) {
                return stop(root, "sigterm");
            }
            continue;
        }
        println!(
            "runner: round {round_no} {} · {}",
            if wrote_back {
                "written back"
            } else if result.timed_out {
                "TIMED OUT (recorded fail)"
            } else {
                "NO WRITEBACK (recorded fail)"
            },
            result.output.lines().next().unwrap_or("").chars().take(120).collect::<String>()
        );
        if let Some(sid) = &result.session {
            if let Some(cmd) = session::resume_command(opts.host, sid) {
                println!("runner: session → {cmd}");
            }
        }
        if opts.git_commit && wrote_back {
            let st = state::load(&path)?;
            let note = st.ticks.last().map(|t| t.note.clone()).unwrap_or_default();
            let cp = git_checkpoint(root, &todo.id, &note, &mut git_baseline);
            git_baseline_settled = cp.settled;
            if !cp.held_back.is_empty() {
                let shown: Vec<&str> = cp.held_back.iter().take(5).map(String::as_str).collect();
                let more = if cp.held_back.len() > 5 { format!(" 等 {} 个", cp.held_back.len()) } else { String::new() };
                println!("runner: 没提交 {}{more} · runner 起跑前它们就是改过的，别人的在制品拆不开", shown.join(" "));
                journal_append(
                    root,
                    &json!({"event": "commit_held_back", "round": round_no, "todo": todo.id,
                                             "paths": cp.held_back, "at": state::now_iso()}),
                )?;
            }
            if let Some(sha) = cp.sha {
                // 提交了哪几个，不只是提交了几个：轮内并发那一格没法靠快照判，
                // 邻居的文件万一混进来，只有这行和账本能让人事后认出来。
                let shown: Vec<&str> = cp.files.iter().take(5).map(String::as_str).collect();
                let more = if cp.files.len() > 5 { format!(" 等 {} 个", cp.files.len()) } else { String::new() };
                println!("runner: git checkpoint {sha} · {} 个文件：{}{more}", cp.files.len(), shown.join(" "));
                journal_append(
                    root,
                    &json!({"event": "commit", "round": round_no, "todo": todo.id, "sha": sha,
                                             "files": cp.files.len(), "paths": cp.files, "at": state::now_iso()}),
                )?;
            }
        }
        // 同上：checkpoint 里的 git 被 SIGTERM 收掉时，叫停这件事只有这儿看得见——
        // 宿主那一轮早跑完了，再往下走会白起一轮重估。
        if stop_requested() {
            return stop(root, "sigterm");
        }
        // 写回之后按信号插一轮重估：只在账本读得出偏离时跑，一轮活最多跟一次，
        // 而且**只产出建议**——改 todo 要人点头，无头模式里没有人。
        if !opts.no_replan && wrote_back && replan_at != Some(round_no) {
            let st = state::load(&path)?;
            let sig = crate::replan::signals(&st);
            // 全部信号都是 blocked、而且等的还是上次那批人——不重复烧一轮模型
            let blocked_now = sig.iter().find(|s| s.kind == "blocked").map(|s| s.detail.clone());
            let latched = sig.iter().all(|s| s.kind == "blocked") && blocked_now.is_some() && blocked_now == replan_blocked;
            if !sig.is_empty() && !latched && crate::todo::remaining(&st) > 0 {
                replan_at = Some(round_no);
                replan_blocked = blocked_now;
                let why: Vec<String> = sig.iter().map(|s| s.detail.clone()).collect();
                println!("runner: 第 {round_no} 轮之后重估计划（{}）", why.join(" · "));
                let open_before = crate::todo::remaining(&st);
                let tail = if opts.auto_replan {
                    format!(
                        "\n\n---\n\n**这一轮由 zloop runner 无头驱动，`--auto-replan` 开着：你可以真的改计划。**\n\n\
                         想好之后，把**新的待办清单**（只列还没做的，一行一条 `[P0] 文本 :: 怎么验`）\
                         从 stdin 交给：\n\n\
                         \x20   `printf '%s\\n' '[P0] …' '[P1] …' | zloop replan --apply --why \"<为什么这么改>\"`\n\n\
                         做完的和等人回话的会自动留着，你不用列。护栏由代码强制，违反会整体拒绝并告诉你是哪条：\
                         清单不能空、每条都要带 `:: 验收`、`--why` 必填、规模最多放大到三倍多一点（且 ≤ 30 条）。\n\n\
                         **判断不用改就什么都别跑**——不改是完全合格的结论。这是第 {} 次自主改计划，\
                         单次运行最多 {} 次，用完会停机等人。别改代码，只改计划。\n",
                        auto_replans + 1,
                        MAX_AUTO_REPLANS
                    )
                } else {
                    "\n\n---\n\n**这一轮由 zloop runner 无头驱动，没有人在旁边点头**：只输出建议清单，\
                     **不要**运行任何会改 todo 的命令（plan / edit / done 一律不要），也不要改代码。\
                     你的输出会原样记进账本，等人回来看。\n"
                        .to_string()
                };
                let text = format!("{}{tail}", crate::replan::packet(&st));
                journal_append(
                    root,
                    &json!({"event": "replan", "round": round_no, "signals": why, "at": state::now_iso()}),
                )?;
                let result = match opts.host {
                    Host::Codex => run_codex(root, &text, None, &opts, timeout, opts.auto_replan)?,
                    _ => run_claude(root, &text, None, &opts, timeout, opts.auto_replan)?,
                };
                let who = HostSession { host: opts.host, session: result.session.clone() };
                // 同上：重估也不写回账本，全文落盘。
                let body = result.output.trim().to_string();
                let summary = body.lines().find(|l| !l.trim().is_empty()).unwrap_or("(没有输出)");
                let rel = crate::log::write_raw(
                    root,
                    "replan",
                    &format!("# 重估 · 第 {round_no} 轮之后\n\n信号：{}\n\n{body}\n", why.join(" · ")),
                )?;
                state::transaction(&path, |st| {
                    let t = tick::record(st, tick::REPLAN, None, &crate::style::truncate(summary, 200), &who)?;
                    if let Some(last) = st.ticks.last_mut() {
                        last.log = Some(rel.clone());
                        last.cost_usd = result.cost_usd;
                        last.duration_ms = result.duration_ms;
                    }
                    Ok(t)
                })?;
                // 计划到底动没动，不听宿主自称，看账本。
                let after = state::load(&path)?;
                let open_after = crate::todo::remaining(&after);
                let changed = after
                    .todos
                    .iter()
                    .filter(|t| !crate::todo::is_terminal(&t.status))
                    .any(|t| !st.todos.iter().any(|o| o.id == t.id));
                if !changed {
                    println!("runner: 重估建议写进账本 · {rel}（没有动任何 todo）");
                } else {
                    auto_replans += 1;
                    grew_in_a_row = if open_after > open_before { grew_in_a_row + 1 } else { 0 };
                    println!(
                        "runner: 计划改了 · {open_before} 条 → {open_after} 条（第 {auto_replans}/{MAX_AUTO_REPLANS} 次自主重排）· {rel}"
                    );
                    journal_append(
                        root,
                        &json!({"event": "replan_applied", "round": round_no, "open_before": open_before,
                                "open_after": open_after, "nth": auto_replans, "at": state::now_iso()}),
                    )?;
                    // 两条闸，任一触顶就**停在人面前**，别安静地接着跑。
                    if grew_in_a_row >= 2 {
                        stop_after_replan = Some(format!(
                            "连着 {grew_in_a_row} 次重排都把清单改长了（这次 {open_before} → {open_after}）——在发散，不是在收敛"
                        ));
                    } else if auto_replans >= MAX_AUTO_REPLANS {
                        stop_after_replan = Some(format!("自主改了 {auto_replans} 次计划还没走上正轨，多半不是计划的问题"));
                    }
                }
                if let Some(reason) = stop_after_replan.take() {
                    println!("runner: 停下来等人 —— {reason}");
                    journal_append(
                        root,
                        &json!({"event": "replan_giveup", "round": round_no, "why": reason, "at": state::now_iso()}),
                    )?;
                    stop(root, "replan_diverged")?;
                    return Ok(0);
                }
            }
        }

        rounds_done += 1;
        if opts.max_rounds > 0 && rounds_done >= opts.max_rounds {
            println!("runner: max rounds reached");
            return stop(root, "max_rounds");
        }
        let st = state::load(&path)?;
        let d = tick::decide(&st, state::now());
        match wait_plan(&st, &d, &opts) {
            None => return stop(root, &d.reason),
            Some((m, reason)) => {
                journal_sleep(root, m, opts.fast, &reason)?;
                if !sleep_interruptible(Duration::from_secs(secs(m, opts.fast))) {
                    return stop(root, "sigterm");
                }
            }
        }
    }
}
