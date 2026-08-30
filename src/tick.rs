//! The scheduler: `decide()` answers "should we run now, and on which todo?".
//!
//! State ladder (highest wins):
//!     paused/done > unplanned/all_done > user_gate/blocked > fail_streak > throttled > ready

use crate::session::HostSession;
use crate::state::{format_iso, now, parse_iso, State, Tick, Todo};
use crate::todo;
use anyhow::{bail, Result};
use chrono::{DateTime, Duration, FixedOffset};
use serde_json::{json, Map, Value};

pub const COUNTED: [&str; 3] = ["done", "progress", "fail"];
/// 宿主**真的把这一轮结掉了**的四种 outcome：`zloop done` 的四个出口。
///
/// 别用「这段时间里账本长没长」代替这个判断：`feedback` / `edit` / `replan` / `reflect`
/// 也会往 `ticks` 里加东西，其中 `feedback` 和 `edit` 是**人在另一个终端**敲的。
/// runner 用长度差判结算时，人插一句 `zloop feedback` 就能让一轮失败的宿主被记成
/// 「写回了」——fail 不记、`fail_streak` 恒为 0、连续失败停机整个失效（A-17）。
pub const WRITEBACK: [&str; 4] = ["done", "progress", "fail", "block"];
/// `feedback` 和 `edit` 是两个**人写的** outcome（`zloop feedback` / `zloop edit`）：
/// agent 自述之外的另一路信号。它们不计入 `COUNTED`（不吃配额、不推进轮次），
/// 会打断 `noop_streak`——人开口说话正是"停下来等人"该等到的东西。
///
/// 但 fail / progress 那两条**停机**闸不一样：无条件清零等于给无头 runner 拆保险丝，
/// 人一句话插进两次 fail（或两轮 progress）中间，计数就永远数不到上限。规矩收窄成两条，
/// 理由见 `fail_streak` / `progress_streak`：
/// 1. **循环已经停在那条 streak 上**时，人的任何一句话都算回应，清零；
/// 2. 还在跑的时候，只有「`edit` 改的就是正在失败 / 正在原地踏步的那条 todo」才清零——
///    活真的换了，之前的结果不再算数。改 backlog 里**别的** todo 不算（A-20）。
pub const OUTCOMES: [&str; 9] = ["done", "progress", "fail", "block", "noop", "edit", "feedback", "reflect", "replan"];
pub const FEEDBACK: &str = "feedback";
/// 回看的那一轮：不做 todo，只读账本 + 经验 + 反馈，给出整理建议。
///
/// 它对三条 streak **透明**（和 `noop` 一样）——插一轮反思不代表"失败被解决了"，
/// 否则 fail / fail / reflect / fail / fail 会让循环永远停不下来。
pub const REFLECT: &str = "reflect";
/// 重估计划的那一轮：不做 todo，只对着最终目标看剩下的任务还对不对。
/// 和 `reflect` 一样对三条 streak 透明——插一轮重估不代表失败被解决了。
pub const REPLAN: &str = "replan";

/// streak 计数时要跳过的轮次：它们不代表干活的结果。
fn transparent(outcome: &str) -> bool {
    outcome == "noop" || outcome == REFLECT || outcome == REPLAN
}

/// 这条 tick 是宿主结掉一轮活留下的吗？——runner 判「写回了没有」只认这个。
pub fn is_writeback(outcome: &str) -> bool {
    WRITEBACK.contains(&outcome)
}

/// 上一轮干活之后才到的反馈——也就是下一轮**必须先处理**的那些。
/// 更早的反馈留在 `ticks` 和 `zloop doc` 里，不再往交接包里堆。
pub fn pending_feedback(state: &State) -> Vec<&Tick> {
    let last_work = state.ticks.iter().rposition(|t| t.outcome == "done" || t.outcome == "progress");
    state
        .ticks
        .iter()
        .enumerate()
        .filter(|(i, t)| t.outcome == FEEDBACK && last_work.is_none_or(|k| *i > k))
        .map(|(_, t)| t)
        .collect()
}

/// 这个目标失败 / 被挡过的地方，最近的在前。
///
/// 循环因为连续失败停下来是对的，但"停下来"不等于"学到"——如果失败的原因没有结构化落点，
/// 下一轮（甚至下一个会话）会把同一个坑再踩一遍。所以把 fail / block 轮次连同它们记下的坑
/// 一起摆进交接包。
pub fn failures(state: &State) -> Vec<&Tick> {
    state.ticks.iter().rev().filter(|t| t.outcome == "fail" || t.outcome == "block").collect()
}

/// 全部反馈条数（含已经被后续轮次消化掉的）。
pub fn feedback_count(state: &State) -> usize {
    state.ticks.iter().filter(|t| t.outcome == FEEDBACK).count()
}

#[derive(Debug, Clone, PartialEq)]
pub struct Decision {
    pub should_run: bool,
    pub reason: String,
    pub todo: Option<Todo>,
    /// `None` means "stop, wait for a human".
    pub interval_min: Option<u32>,
}

impl Decision {
    fn stop(reason: &str) -> Self {
        Decision { should_run: false, reason: reason.into(), todo: None, interval_min: None }
    }
}

// --- streaks & accounting -------------------------------------------------

/// Trailing consecutive `fail` ticks; `noop` ticks are transparent, anything else breaks it.
///
/// `feedback` 只在**循环已经因连续失败停下**的时候才清零（`forgive_at` = `max_fail_streak`）。
/// 无条件清零看着更"人说了算"，实际是给无头 runner 拆了保险丝：`zloop feedback` 是文档
/// 教人「跟正在跑的循环说话」的那条路，人在另一个终端补一句「先别动 x.rs」，跟"这一轮
/// 失败被解决了"没有任何关系，可它一插进两次 fail 中间，连续失败就永远数不到上限——
/// 宿主接着一轮一轮地烧（A-17 的后半截）。
///
/// 「人开口说话就该让循环继续」这条语义没丢，只是收窄到它本来说的那个场景：**停下来
/// 等人之后**人才开的口（README 里那段实测——连着 3 次 fail → `WAIT (fail_streak)` →
/// `zloop feedback …` → `RUN`——一字不差地照旧成立）。
///
/// `edit` 是同一形状的第二条（A-20）：`edit` tick 全仓只有 `zloop edit` 记（cli.rs），
/// 也就是**人在另一个终端**敲的。改的是**正在失败的那条活**才算"活变了、之前的失败
/// 不再算数"；顺手把 backlog 里另一条 todo 改个名、推后一条 P2，跟这串失败没有任何
/// 关系，不该把闸拆了。改别的活只有在**循环已经停在 `fail_streak` 上**时才清零——
/// 那时候人是在回应一个停着的循环，和 `feedback` 同一条规矩。
pub fn fail_streak(state: &State) -> usize {
    fails_in_a_row(&state.ticks, state.policy.max_fail_streak)
}

fn fails_in_a_row(ticks: &[Tick], forgive_at: usize) -> usize {
    // 从头往后走，不是从尾往前：要判「这条 feedback / edit 写下的当时循环停没停」，
    // 得知道它**前面**攒了几条 fail。
    let mut n = 0;
    // 这串连续失败落在哪几条 todo 上——`edit` 改的是不是其中之一，决定它算不算数
    let mut failing: Vec<&str> = Vec::new();
    for t in ticks {
        // 循环这会儿是不是已经停在 fail_streak 上（停了，人的任何一句话都算回应）
        let stopped = forgive_at > 0 && n >= forgive_at;
        match t.outcome.as_str() {
            "fail" => {
                n += 1;
                if let Some(id) = t.todo.as_deref() {
                    if !failing.contains(&id) {
                        failing.push(id);
                    }
                }
            }
            o if transparent(o) => {}
            FEEDBACK => {
                if stopped {
                    n = 0; // 人回应的是一个已经停在那儿的循环——放它再试
                    failing.clear();
                }
            }
            "edit" => {
                // 没记 todo 的 edit（手改过的账本）当成"改的是别的活"：宁可多认不可漏认
                let touches_failing = t.todo.as_deref().is_some_and(|id| failing.contains(&id));
                if touches_failing || stopped {
                    n = 0;
                    failing.clear();
                }
            }
            _ => {
                n = 0; // done / progress / block：这一轮有别的结果了
                failing.clear();
            }
        }
    }
    n
}

pub fn noop_streak(ticks: &[Tick]) -> usize {
    ticks
        .iter()
        .rev()
        .filter(|t| !transparent(&t.outcome) || t.outcome == "noop")
        .take_while(|t| t.outcome == "noop")
        .count()
}

/// Trailing consecutive `progress` ticks on `todo_id`; `noop` is transparent, anything else breaks it.
///
/// `forgive_at` = `policy.max_progress_streak`，和 `fail_streak` 同一条规矩（A-21）：
/// 人写的两种 tick（`feedback` / 改**别的** todo 的 `edit`）只有在**循环已经停在
/// `progress_streak` 上**时才清零。还在跑的时候补一句「先别动 x.rs」跟"这条活不再
/// 原地踏步了"没有关系——无条件清零等于给无头 runner 拆保险丝：反馈一插进两轮
/// progress 中间，尾部连续数就永远到不了上限，同一条 todo 一轮一轮地推、一轮一轮
/// 地烧（实测 8 轮 progress 不停，见 `scripts/repro-a20-a21-…`）。
///
/// `edit` 改的**就是这条 todo** 是例外，照旧无条件清零：README 给 `progress_streak`
/// 开的出口就是 `zloop edit t3 --text "更小的一步"`——活真的换了，之前的原地踏步不算数。
pub fn progress_streak(ticks: &[Tick], todo_id: &str, forgive_at: usize) -> usize {
    // 和 fails_in_a_row 一样从头往后走：要判一条 feedback 写下时循环停没停
    let mut n = 0;
    for t in ticks {
        let stopped = forgive_at > 0 && n >= forgive_at;
        match t.outcome.as_str() {
            o if transparent(o) => {}
            "progress" if t.todo.as_deref() == Some(todo_id) => n += 1,
            "edit" if t.todo.as_deref() == Some(todo_id) => n = 0,
            FEEDBACK | "edit" => {
                if stopped {
                    n = 0;
                }
            }
            _ => n = 0,
        }
    }
    n
}

/// 这条活是不是**另一个会话**刚领走、还在做？
///
/// `next` 曾经无条件覆盖 `in_progress`，于是两个 Claude 会话会同时领到同一条 todo：
/// 谁都以为自己拿着，两个 agent 改同一批文件，先写回的那个还把另一个的在飞状态一起清掉。
/// 判断只在**交互式派活**（`via == "next"`）之间做：runner 自己设 `in_progress`，
/// 不走这条路，所以无头循环不会被自己挡住。这一条是**有意的、必须保留**——
/// runner 设完 `in_progress` 才去起 `claude -p`，那个子进程自己会敲 `zloop next`、
/// 带的是它自己的新 session id；`via == "runner"` 要是也算数，runner 就会把自家的
/// 子进程挡在门外。
///
/// **所以这个函数挡不住 runner，别指望它去做那件事**（#14 的原话是「复用 next 的
/// held_by_other 判断」，照抄会得到一个看起来对、实际不挡的补丁）。实测四种在场组合：
///
/// | `in_progress` 的持有者 | 另一个会话问 `next` |
/// |---|---|
/// | `via=runner` / 任意 session | 放行——就是上面这条 |
/// | `via=next` / 别人的 session | **挡住**：这道防线唯一真正生效的情形 |
/// | `via=next` / 没有 session   | 放行——见下面「分不出是谁就不拦」 |
/// | `via=next` / 我自己          | 放行 |
///
/// 「runner 在跑的时候别催交互会话」要用另一个判据：`daemon::running()`。
/// 那个判据不会误伤 runner 自己的子进程，因为子进程在 `cmd_hook_stop` 开头就被
/// `ZLOOP_RUNNER` 环境变量挡下、提前返回了，走不到那一步。
///
/// `policy.stale_after_min` 决定"多久没动静就算被丢下了"——过期的派活照旧可以重派，
/// 设成 0 等于关掉这个保护。
/// `next` 撞上别人的派活时给出的决定：不跑，但过一会儿可以再来问——
/// 派活会因 `stale_after_min` 过期，所以自动续跑的循环能自己恢复，不必等人。
pub fn hold_decision(state: &State) -> Decision {
    Decision { should_run: false, reason: "held_by_other".into(), todo: None, interval_min: Some(interval(state, 1)) }
}

pub fn held_by_other(state: &State, who: &HostSession, at: DateTime<FixedOffset>) -> Option<crate::state::InProgress> {
    let ip = state.in_progress.as_ref()?;
    if ip.via != "next" || state.policy.stale_after_min <= 0 {
        return None;
    }
    // 分不出是谁就不拦：裸 CLI 没有 session id，拦了只会把人锁在门外
    let (Some(held), Some(mine)) = (ip.session.as_deref(), who.session.as_deref()) else {
        return None;
    };
    if held == mine && ip.host.as_deref() == Some(who.host.as_str()) {
        return None;
    }
    let stale = parse_iso(&ip.started_at).map(|s| (at - s).num_minutes() >= state.policy.stale_after_min).unwrap_or(true);
    (!stale).then(|| ip.clone())
}

pub fn current_round(ticks: &[Tick]) -> u64 {
    ticks.iter().filter(|t| t.outcome == "done" || t.outcome == "progress").count() as u64
}

/// **第几轮**：盖在每条新 tick 上、印在交接包「round N」那一格里的那个编号。
///
/// 和 `current_round` 差的就是归档走的那些（T29）。这个数必须**只增不减**：它是编号，
/// 不是余额。只数现有 tick 的话，一次 `zloop compact` 就让它掉回去——账本里从此有两条
/// 「round 7」，而交接包一边写「跑了 40 轮」一边写「round 1」，自己跟自己打架。
pub fn round_number(state: &State) -> u64 {
    current_round(&state.ticks) + (state.archived.count("done") + state.archived.count("progress")) as u64
}

/// 「跑了几轮」：干活的轮次（`COUNTED`：done / progress / fail），失败也算跑过。
///
/// `status` 和 `stats` 必须共用这一个定义，否则同一份账本会报出两个数——
/// 曾经 `status` 只排除 `noop`，于是 3 条 todo + 1 次回看被它算成 4 轮，而 `stats` 报 3 轮。
/// reflect / replan / feedback / edit / block 都不是"跑了一轮活"，不进这个数。
pub fn rounds(ticks: &[Tick]) -> usize {
    ticks.iter().filter(|t| COUNTED.contains(&t.outcome.as_str())).count()
}

/// 这个目标**一辈子**跑了几轮：账本里现有的 + 已被 `compact` 归档走的（T29）。
///
/// 和 `spent_total` 是同一件事的两个面。只数现有 tick 的话，一次例行整理就让
/// 「跑了 N 轮」掉回去：`status` 印「跑了 0 轮 · 0%」，`zloop stats` 更狠——它在
/// `rounds == 0` 时直接印「还没有跑过任何一轮 · zloop next 开始」然后返回，
/// 一个跑了几十轮、完成过一半 todo 的目标被说成从没开工。
///
/// 凡是回答「这个目标跑得怎么样」的地方都走这一个函数；只回答「账本里还剩什么」的
/// 地方（`stats` 的 todo 清单、`log` 的轮次列表）才用 `rounds(&state.ticks)`。
pub fn rounds_total(state: &State) -> usize {
    rounds(&state.ticks) + state.archived.rounds()
}

/// Total host-reported spend recorded on ticks (USD).
pub fn spent_usd(ticks: &[Tick]) -> f64 {
    ticks.iter().filter_map(|t| t.cost_usd).sum()
}

/// 这个目标**一辈子**花了多少：账本里现有的 tick + 已被 `compact` 归档走的那些。
///
/// 预算闸和所有显示花费的地方都必须走这一个函数。只数现有 tick 的话，一次
/// `zloop compact` 就把 `max_total_usd`（「这个目标一共只准花这么多」）悄悄改成
/// 「最近 keep_days 天只准花这么多」——而且没有任何痕迹（A-18）。
pub fn spent_total(state: &State) -> f64 {
    spent_usd(&state.ticks) + state.archived.cost_usd
}

/// `policy.window_hours` 的合法上限：一年。
///
/// 配额窗口比这还长就等于没有窗口（`max_runs` 变成终身配额），而代价是每一处
/// `now ± window_hours` 都要在 chrono 的边界上跳舞。
pub const WINDOW_HOURS_MAX: i64 = 24 * 365;

/// 配额窗口的长度，**取值先钳到 `0..=WINDOW_HOURS_MAX` 再交给 chrono**。
///
/// `.zloop/state.json` 不是内部文件——zloop 自己就在教人去手改那个 `policy` 块
/// （`start` 撞到预算上限时的提示就是「改大 policy.max_total_usd」），隔壁字段被顺手
/// 写错只是时间问题。而 `Duration::hours(n)` 和 `at - span` 对越界的 n 都是 **panic**：
/// `window_hours = 99999999999` 时 `status` / `context` 一起退 101，
/// 再大一位连 chrono 内部的 `TimeDelta::hours out of bounds` 都出来了，
/// 整个项目目录就此敲不动（A-7）。钳一下的代价是「按 1 年算」，比崩掉强得多；
/// 真被钳到了由 `doctor` 的 `bad_policy` 说出来，不会悄没声。
pub fn window_span(policy: &crate::state::Policy) -> Duration {
    Duration::hours(policy.window_hours.clamp(0, WINDOW_HOURS_MAX))
}

pub fn window_ticks(state: &State, at: DateTime<FixedOffset>) -> Vec<&Tick> {
    // 钳完还有 `checked_`：`at` 也可能是从账本里读来的（不是 now），谁都不信一遍
    let span = window_span(&state.policy);
    let since = at.checked_sub_signed(span).unwrap_or(at);
    state
        .ticks
        .iter()
        .filter(|t| COUNTED.contains(&t.outcome.as_str()))
        .filter(|t| parse_iso(&t.at).map(|ts| ts >= since).unwrap_or(false))
        .collect()
}

/// 一档退避间隔的合法上限：7 天。
///
/// 间隔的语义是「多久回来再看一眼」，不是排期——真要停很久有 `defer` 和 `pause`。
/// 一周还回不来一次的循环，已经和停了没区别。
pub const INTERVAL_MIN_MAX: u32 = 7 * 24 * 60;

/// 把一档间隔钳进 `1..=INTERVAL_MIN_MAX`。
///
/// 上限那边挡的是**睡死**：`window_hours`（A-7）和未来时间戳（A-11）之后，
/// `intervals_min` 是第三处「一个数写歪就让循环永远醒不过来」的地方，而且是唯一
/// 没有任何封顶的一处——`throttled` 那一支有窗口封顶挡着，`user_gate` / `blocked`
/// 这一支直接把文件里的数交给了 runner。实测 `intervals_min = [4294967295]`：
/// debug 构建在 `phase::human_minutes` 的 `m + 720` 上 panic（`next` / `status` /
/// `context` 一起退 101），release 构建不 panic，但 `interval_min` 原样吐出
/// 4294967295 分钟（8171 年）——而面板上因为同一处加法回绕，写的是**"约 0 天后重试"**。
/// 也就是说不封顶时，睡死的表现是「一切正常」。
///
/// 下限是 1 不是 0：`intervals_min = [0]` 时 runner 每轮 sleep 0 秒、立刻再拉起一个
/// host 会话，那是烧钱的忙等，不是快。
///
/// 钳过就等于文件里写的数没生效，这件事由 `doctor` 的 `bad_policy` 说出来。
pub fn clamp_interval(m: u32) -> u32 {
    m.clamp(1, INTERVAL_MIN_MAX)
}

fn interval(state: &State, level: usize) -> u32 {
    let iv = &state.policy.intervals_min;
    if iv.is_empty() {
        return 3;
    }
    clamp_interval(iv[level.min(iv.len() - 1)])
}

// --- the decision ---------------------------------------------------------

pub fn decide(state: &State, at: DateTime<FixedOffset>) -> Decision {
    let goal = &state.goal;
    let policy = &state.policy;
    let ticks = &state.ticks;

    if goal.status != "active" {
        return Decision::stop(&goal.status);
    }
    let open = todo::open_ordered(state);
    if open.is_empty() {
        // 「一条都还没规划」「全部做完了」「全被延后了」是三件事，出口动作各不相同：
        // 第一个要 `zloop plan`，第二个才该开新目标，第三个要把延后的捡回来。共用
        // `all_done` 这一个词时，skill 的「已完成 → goal new」那一支会把刚 `goal new`
        // 出来的空目标当成做完的，再新建一个重名目标把它停放掉（#5）；同样地，一条没做完
        // 全推到以后的目标会被当成收工，两条延后的活就此没人再看（B-3）。
        return Decision::stop(if state.todos.is_empty() {
            "unplanned"
        } else if todo::all_deferred(state) {
            "all_deferred"
        } else {
            "all_done"
        });
    }
    let noops = noop_streak(ticks);
    // `> 0` 是「0 = 关掉这个检查」，五个阈值同一个口径（A-10）。少了这个守卫时
    // `max_noop_streak = 0` 让 `exhausted` 恒真：`should_run` 不变，但下面两支
    // 非终态出口的 `interval_min` 从「10 分钟后再看」变成 `None`＝停下等人，
    // 一个**没跑过任何一轮**的目标当场不再自己醒来。
    let exhausted = policy.max_noop_streak > 0 && noops >= policy.max_noop_streak;

    let runnable = todo::executable(state);
    if runnable.is_empty() {
        let waiting_on_user = open.iter().any(|&i| state.todos[i].blocked_by.iter().any(|d| d == todo::USER));
        let reason = if waiting_on_user { "user_gate" } else { "blocked" };
        return Decision {
            should_run: false,
            reason: reason.into(),
            todo: None,
            interval_min: if exhausted { None } else { Some(interval(state, 1 + noops)) },
        };
    }
    // 同上（A-10），而这一支更狠：`max_fail_streak = 0` 时 `0 >= 0` 恒真，一次失败都
    // 没有的全新目标第一次 `next` 就返回 `fail_streak` + `interval=None`——想关掉这道闸
    // 的人照另外三个阈值的先例写了 0，拿到的是「目标当场永久停机」。
    if policy.max_fail_streak > 0 && fail_streak(state) >= policy.max_fail_streak {
        return Decision::stop("fail_streak");
    }
    if policy.max_total_usd > 0.0 && spent_total(state) >= policy.max_total_usd {
        return Decision::stop("budget");
    }
    let candidate = &state.todos[runnable[0]];
    if policy.max_progress_streak > 0
        && progress_streak(ticks, &candidate.id, policy.max_progress_streak) >= policy.max_progress_streak
    {
        return Decision::stop("progress_streak");
    }
    let counted = window_ticks(state, at);
    if policy.max_runs > 0 && counted.len() >= policy.max_runs {
        let oldest = counted.iter().filter_map(|t| parse_iso(&t.at).ok()).min().unwrap_or(at);
        let span = window_span(policy);
        // `oldest` 是账本里读来的时间戳、`span` 是人手改的 policy 算出来的：两个都不可信，
        // 加起来越界就按「等满一个窗口」处理（下面还会再钳一次）。
        let frees_in = oldest.checked_add_signed(span).map(|free_at| free_at - at).unwrap_or(span);
        // 等待有上限：一条 tick 最多在窗口里待 `window_hours`，等得比这更久没有任何道理。
        // 少了这个封顶，一条**落在未来**的 tick 就能让 runner 睡到下个世纪：`oldest` 在未来
        // 时 `frees_in` 是个天文数字，实测 `interval_min=38048610`（72 年，A-11）。造出未来
        // 时间戳不需要有人手改文件——NTP 校时、改时区、虚拟机挂起恢复、笔记本电池耗尽后
        // 时钟重置，都会让已有的 tick 落在"未来"。封顶之后最坏也只是每 `window_hours`
        // 醒一次重新判断（未来时间戳本身由 doctor 的 `future_timestamp` 报出来）。
        let cap = policy.window_hours.clamp(1, WINDOW_HOURS_MAX) * 60;
        let minutes = (frees_in.num_seconds().div_euclid(60) + 1).clamp(1, cap) as u32;
        return Decision {
            should_run: false,
            reason: "throttled".into(),
            todo: None,
            interval_min: if exhausted { None } else { Some(minutes) },
        };
    }
    Decision {
        should_run: true,
        reason: "ready".into(),
        todo: Some(state.todos[runnable[0]].clone()),
        interval_min: Some(interval(state, 0)),
    }
}

// --- writes ---------------------------------------------------------------

pub fn record(state: &mut State, outcome: &str, todo_id: Option<&str>, note: &str, who: &HostSession) -> Result<Tick> {
    if !OUTCOMES.contains(&outcome) {
        bail!("invalid outcome {outcome:?}");
    }
    let bump = matches!(outcome, "done" | "progress") as u64;
    let tick = Tick {
        at: format_iso(&now()),
        round: round_number(state) + bump,
        todo: todo_id.map(str::to_string),
        outcome: outcome.to_string(),
        note: note.to_string(),
        host: Some(who.host.as_str().to_string()),
        session: who.session.clone(),
        log: None,
        cost_usd: None,
        duration_ms: None,
        num_turns: None,
        documented: None,
        pitfalls: Vec::new(),
        rethink: None,
        extra: Map::new(),
    };
    state.ticks.push(tick.clone());
    Ok(tick)
}

/// `apply_done` 的结果：这一轮记了什么、动的是哪条 todo、以及**有没有不动它的状态**。
#[derive(Debug)]
pub struct Written {
    pub tick: Tick,
    pub idx: usize,
    /// `Some(status)` = 这条 todo 早就了结了，这次写回只收了这一轮，状态原样留着。
    /// 回显那句「状态没动」只渲染这个字段，不再自己判一次——判断和回显共用一份数据。
    pub kept_status: Option<String>,
}

/// The single write-back: record a tick, move the todo, optionally append a successor.
///
/// 一条 todo 已经是终态、而 `in_progress` 还指着它，写回**照样收得了尾**（T42）：
/// `zloop edit --status done/deferred` 从头到尾不碰 `in_progress`（`cli.rs` 的 `cmd_edit`），
/// 所以人一判「这条不做了」，派活指针就留在一条 `deferred` 的 todo 上。以前这时
/// `done` 退 2「is already deferred」，而 `ensure_idle` 的报错、`status` 的写回提示、
/// `compact` 的 in-flight 提示印的正是这条命令——三处出口一起变成**保证失败的命令**。
///
/// 收尾时状态**不改**：人已经判过了，写回只负责记这一轮 + 让 `cmd_done` 清掉指针。
/// 反过来把 `deferred` 改回 `done` 就是拿一次机械的写回覆盖人的决定。
pub fn apply_done(
    state: &mut State,
    id: &str,
    outcome: &str,
    note: &str,
    block: Option<&str>,
    next_text: Option<&str>,
    who: &HostSession,
) -> Result<Written> {
    let idx = todo::index_of(state, id)?;
    let status = state.todos[idx].status.clone();
    // 闸只对**在飞的那一条**开：`in_progress` 不指着它，就还是老规矩——第二次 `done`
    // 退 2「already done」，账目不会重复（`done_twice_is_rejected` / `done_errors`）。
    let settled = todo::is_terminal(&status) && state.in_progress.as_ref().is_some_and(|ip| ip.todo == id);
    if todo::is_terminal(&status) && !settled {
        bail!("{id} is already {status}");
    }
    let kept_status = settled.then(|| status.clone());
    let tick = if let Some(question) = block {
        if !settled {
            todo::set_status(state, id, "blocked", Some(question))?;
            let t = &mut state.todos[idx];
            if !t.blocked_by.iter().any(|d| d == todo::USER) {
                t.blocked_by.push(todo::USER.to_string());
            }
        }
        record(state, "block", Some(id), question, who)?
    } else if outcome == "done" {
        if !settled {
            todo::set_status(state, id, "done", Some(note))?;
        }
        record(state, "done", Some(id), note, who)?
    } else if outcome == "progress" || outcome == "fail" {
        if !settled {
            let t = &mut state.todos[idx];
            if !note.is_empty() {
                t.note = note.to_string();
            }
            t.updated_at = crate::state::now_iso();
        }
        record(state, outcome, Some(id), note, who)?
    } else {
        bail!("invalid outcome {outcome:?}; expected done, progress or fail");
    };

    if let Some(text) = next_text {
        let priority = state.todos[idx].priority;
        if let Some((p, body)) = todo::parse_line(text, priority) {
            todo::insert_after(state, id, &body, Some(p))?;
        }
    }
    // 和 `edit` 同一口径：清单空但一条没完成（全被延后）不算目标结束。走 done 这条路时
    // 手上这条刚被标成 done，`all_deferred` 本来就是假的——写在这里是为了两处别漂开。
    if todo::open_ordered(state).is_empty() && !todo::all_deferred(state) {
        state.goal.status = "done".into();
    }
    Ok(Written { tick, idx, kept_status })
}

// --- projection -----------------------------------------------------------

pub fn last_summary(state: &State) -> Option<String> {
    state.ticks.iter().rev().find(|t| t.outcome != "noop").map(|t| {
        let head = match &t.todo {
            Some(id) => format!("{id} {}", t.outcome),
            None => t.outcome.clone(),
        };
        if t.note.is_empty() {
            head
        } else {
            format!("{head}: {}", t.note)
        }
    })
}

pub fn to_json(decision: &Decision, state: &State) -> Value {
    let todo = decision.todo.as_ref();
    json!({
        "goal": state.goal.text,
        "round": round_number(state),
        "should_run": decision.should_run,
        "reason": decision.reason,
        "todo": todo.map(|t| {
            let mut o = json!({"id": t.id, "text": t.text, "priority": t.priority});
            if let Some(a) = &t.acceptance {
                o["acceptance"] = Value::String(a.clone());
            }
            o
        }),
        "remaining": todo::remaining(state),
        "last": last_summary(state),
        "writeback": todo.map(|t| format!("zloop done {} --note '<一句话结果>'", t.id)),
        "interval_min": decision.interval_min,
    })
}
