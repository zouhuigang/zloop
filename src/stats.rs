//! `zloop stats`：这个目标跑得怎么样——不是"还剩几条"，而是"跑得顺不顺"。
//!
//! 为什么需要它：Warp 的自改进回路是 **跑 → 打分 → 自改进**，`RunScorer` 就在自改进的前一环
//! （见 `docs/SELF-IMPROVEMENT.md` 1.5）。zloop 此前只有 `tick.documented` 这一个布尔值，
//! 是最原始的打分器。reflect（W2/W6）要读的"这个目标哪里不顺"，得先有人把它算出来。
//!
//! 全部数字都从 `state.ticks` 现推——账本本来就记着每一轮。**唯一的例外**是
//! `compact` 搬走的那些：ticks 没了，累计量得从 `state.archived` 的汇总里补回来
//! （A-18 是花费，T29 是轮次/返工/失败/无文档/耗时）。哪些数是「这个目标一辈子」、
//! 哪些只是「账本里还剩的」，见 `compute` 里那两段注释。

use crate::state::State;
use crate::tick;
use crate::todo;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct TodoStat {
    pub id: String,
    pub text: String,
    pub status: String,
    /// 计数轮次：done / progress / fail（noop、edit、feedback 不算）
    pub rounds: usize,
    /// 返工：这条 todo 上的 progress + fail 轮数
    pub rework: usize,
    pub fails: usize,
    pub blocks: usize,
    pub feedback: usize,
    pub cost_usd: f64,
    pub duration_ms: u64,
    /// 完成的那一轮留下实现思路了吗
    pub documented: bool,
    /// 一轮做完、没返工过
    pub first_try: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Stats {
    pub goal: String,
    pub todos: Vec<TodoStat>,
    pub rounds: usize,
    pub rework: usize,
    pub fails: usize,
    pub blocks: usize,
    pub feedback: usize,
    /// 回看过几次（`zloop reflect` / runner 的 --reflect-every）
    pub reflects: usize,
    /// 重估过几次（`zloop replan` / runner 的信号触发）
    pub replans: usize,
    pub undocumented: usize,
    pub cost_usd: f64,
    pub duration_ms: u64,
    /// 已完成的 todo 数
    pub done: usize,
    /// 其中一次过的
    pub first_try: usize,
    /// 返工率 = 返工轮数 ÷ 计数轮次（没有轮次时是 0）。分子分母都含归档的那部分。
    pub rework_rate: f64,
    /// 上面那些汇总里，有几轮是 `compact` 归档走的（口径：账本里已经没有它们的 tick 了）。
    /// 拿它可以回答「清单上只剩 2 条，怎么写着跑了 40 轮」。
    pub archived_rounds: usize,
    /// 归档走的 tick 总条数（含 noop / edit / feedback 这些不算轮次的）。
    pub archived_ticks: usize,
    /// 老版本 compact 留下的账：搬走过 tick 却没记 outcome，那些轮次补不回来。
    /// 这时 `archived_rounds` 是 0，但它**不代表**归档里没有轮次——别把它当 0 用。
    pub archived_rounds_unknown: bool,
}

fn round_of(pct: f64) -> f64 {
    (pct * 1000.0).round() / 1000.0
}

pub fn compute(state: &State) -> Stats {
    let mut todos: Vec<TodoStat> = Vec::new();
    for t in &state.todos {
        let mine: Vec<_> = state.ticks.iter().filter(|k| k.todo.as_deref() == Some(t.id.as_str())).collect();
        let rounds = mine.iter().filter(|k| tick::COUNTED.contains(&k.outcome.as_str())).count();
        let rework = mine.iter().filter(|k| k.outcome == "progress" || k.outcome == "fail").count();
        let fails = mine.iter().filter(|k| k.outcome == "fail").count();
        let blocks = mine.iter().filter(|k| k.outcome == "block").count();
        let feedback = mine.iter().filter(|k| k.outcome == tick::FEEDBACK).count();
        // 文档看的是"完成的那一轮"：progress / fail 轮次本来就不欠实现思路
        let documented = mine.iter().any(|k| k.outcome == "done" && k.documented == Some(true));
        todos.push(TodoStat {
            id: t.id.clone(),
            text: t.text.clone(),
            status: t.status.clone(),
            rounds,
            rework,
            fails,
            blocks,
            feedback,
            cost_usd: mine.iter().filter_map(|k| k.cost_usd).sum(),
            duration_ms: mine.iter().filter_map(|k| k.duration_ms).sum(),
            documented,
            first_try: t.status == "done" && rounds == 1 && rework == 0,
        });
    }

    // 上面那张 todo 清单只能讲账本里还剩的那些（归档走的 todo 连同 id 都不在了）。
    // 下面这些**汇总**不一样，它们回答的是「这个目标跑得怎么样」——一辈子的账，
    // `compact` 搬走的那部分必须加回来（A-18 是花费那一项，T29 是其余各项）。
    let counted = |o: &str| state.ticks.iter().filter(|k| k.outcome == o).count() + state.archived.count(o);
    let rounds = tick::rounds_total(state);
    let rework = counted("progress") + counted("fail");
    let done = state.todos.iter().filter(|t| t.status == "done").count();
    Stats {
        goal: state.goal.text.clone(),
        rounds,
        rework,
        fails: counted("fail"),
        blocks: counted("block"),
        feedback: counted(tick::FEEDBACK),
        reflects: counted(tick::REFLECT),
        replans: counted(tick::REPLAN),
        undocumented: state.ticks.iter().filter(|k| k.documented == Some(false)).count() + state.archived.undocumented,
        cost_usd: tick::spent_total(state),
        duration_ms: state.ticks.iter().filter_map(|k| k.duration_ms).sum::<u64>() + state.archived.duration_ms,
        done,
        first_try: todos.iter().filter(|t| t.first_try).count(),
        // 分子分母同源：只补分母（或只补分子）都会让一次整理把返工率冲歪，而
        // `replan` 拿这个数当「该重估了」的信号（`replan::signals` 的 rework 一路）。
        rework_rate: if rounds > 0 { round_of(rework as f64 / rounds as f64) } else { 0.0 },
        archived_rounds: state.archived.rounds(),
        archived_ticks: state.archived.ticks,
        archived_rounds_unknown: state.archived.rounds_unknown(),
        todos,
    }
}

/// 最费劲的那条：返工最多，其次失败最多。用来回答"哪一步最不顺"。
pub fn roughest(stats: &Stats) -> Option<&TodoStat> {
    stats.todos.iter().filter(|t| t.rework > 0 || t.blocks > 0).max_by_key(|t| (t.rework, t.fails, t.blocks))
}

/// 还没做完的条数（和 `status` 用同一口径：deferred 算了结）。
pub fn remaining(state: &State) -> usize {
    state.todos.iter().filter(|t| !todo::is_terminal(&t.status)).count()
}
