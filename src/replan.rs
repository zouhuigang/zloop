//! 自适应重规划：做完一条之后，看看剩下的任务还对不对。
//!
//! 设计依据在 `docs/ADAPTIVE-REPLAN.md`。最关键的一条是**别每轮都重规划**：
//! 文献（Bayesian partner modelling）明确说选择性触发能用远少于启发式/LLM 触发的重规划次数
//! 拿到相当收益；每轮调模型重估不但贵，还会制造计划抖动。
//!
//! 所以这里分两层：
//! 1. **便宜的体检**（`signals()`，纯代码、不调模型）每轮 `done` 之后跑一次，**没命中就一声不吭**；
//! 2. 命中了才提示升级成 `zloop replan` —— 那一步才把材料摆给模型，而且只要**最小改动**。
//!
//! 信号全部取自账本里已有的东西，一个都不用新造（W1 的反馈、W4 的坑、W5 的返工率）。

use crate::state::State;
use crate::stats;
use crate::tick;
use crate::todo;

pub struct Signal {
    /// 机读的类别名
    pub kind: &'static str,
    /// 给人看的一句话
    pub detail: String,
}

/// 连续在同一条 todo 上 progress 了几轮就算"停滞"。
///
/// 比 `policy.max_progress_streak`（默认 8，那是**停下来等人**的阈值）早得多——
/// 提醒该便宜、该早；真要停下来是另一回事。
const STALL_AT: usize = 2;
/// 连续失败几次就该怀疑方法而不只是这一条难。
const FAIL_AT: usize = 2;
/// 返工率超过这个数就值得看一眼颗粒度（要有足够轮次才算数）。
const REWORK_AT: f64 = 0.5;
const REWORK_MIN_ROUNDS: usize = 3;

/// 这条 todo 连着几轮没做完。
///
/// 和 `tick::progress_streak` **故意不共用**：那个是"要不要停下来等人"的闸，人一开口就该清零
/// （给它带着新信息再试一次）；这里问的是"这条是不是在拖"——人说了句话并不会让它不拖。
/// 同一个数，两个问题，口径不该混。
fn dragging(state: &State, todo_id: &str) -> usize {
    let mut n = 0;
    for t in state.ticks.iter().rev() {
        match t.outcome.as_str() {
            "progress" if t.todo.as_deref() == Some(todo_id) => n += 1,
            // 不是"干活"的轮次一律透明：它们既不推进也不打断"在拖"这个事实
            o if !tick::COUNTED.contains(&o) => continue,
            _ => break,
        }
    }
    n
}

/// 账本里能直接读出来的偏离信号。空的就代表"计划看着还行，别打扰"。
pub fn signals(state: &State) -> Vec<Signal> {
    let mut out = Vec::new();

    // 人明确说了「这样不对」——最强的信号。
    //
    // 注意范围是「**还没做完**的 todo 上出现过反馈」，而不是 `pending_feedback`（只算最近一轮之后的）：
    // 人说完方向不对、agent 又干了一轮，`pending` 就空了——可那正是最该重估的时刻。
    let pending: Vec<&str> = tick::pending_feedback(state).iter().filter_map(|t| t.todo.as_deref()).collect();
    for t in &state.todos {
        if todo::is_terminal(&t.status) {
            continue;
        }
        let n = state.ticks.iter().filter(|k| k.outcome == tick::FEEDBACK && k.todo.as_deref() == Some(t.id.as_str())).count();
        if n == 0 {
            continue;
        }
        let how = if pending.contains(&t.id.as_str()) { "还没消化" } else { "（已经过了一轮）" };
        let many = if n > 1 { format!("{n} 条") } else { String::new() };
        out.push(Signal { kind: "feedback", detail: format!("{} 有你的{many}反馈{how}", t.id) });
    }

    // 停滞：同一条 todo 连着做不完
    for t in &state.todos {
        if todo::is_terminal(&t.status) {
            continue;
        }
        let n = dragging(state, &t.id);
        if n >= STALL_AT {
            out.push(Signal { kind: "stalled", detail: format!("{} 连续 {n} 轮没做完", t.id) });
        }
    }

    // 连续失败：可能不是这一条难，是方法错了
    let fails = tick::fail_streak(state);
    if fails >= FAIL_AT {
        out.push(Signal { kind: "fail_streak", detail: format!("连续 {fails} 轮失败") });
    }

    // 返工率高：多半是某条 todo 塞了太多事
    let s = stats::compute(state);
    if s.rounds >= REWORK_MIN_ROUNDS && s.rework_rate >= REWORK_AT {
        let who = stats::roughest(&s).map(|r| format!("（最费劲的是 {}）", r.id)).unwrap_or_default();
        out.push(Signal {
            kind: "rework",
            detail: format!("返工率 {}%{who}", (s.rework_rate * 100.0).round() as i64),
        });
    }

    // 计划被自己推翻了：某一轮写回时说了「后续走不通」（`zloop done --rethink`）。
    //
    // 这是唯一一路**不问"有没有出岔子"、而问"还到得了目标吗"**的信号。其余五个全是偏离
    // 信号，而最该重规划的那种场景恰恰不偏离：那一轮**顺利完成**，可它的结论把剩下几条的
    // 前提推翻了（`docs/ADAPTIVE-REPLAN.md` §6 缺口二有实测——两轮全绿，零信号）。
    //
    // zloop 判断不了「策略走不通」，所以不猜：只认刚干完活的那个 agent 主动说出口的那一句。
    //
    // **边沿不是锁存**：只认最近一次重估之后新说的。踩过——`blocked` 当年就是个锁存，
    // 一条挂着的 todo 让一次 4 小时长跑里 5 次重估全由同一个信号触发，占掉两成多的花费。
    let since = state.ticks.iter().rposition(|t| t.outcome == tick::REPLAN).map_or(0, |i| i + 1);
    for t in &state.ticks[since..] {
        if let Some(r) = t.rethink.as_deref().map(str::trim).filter(|r| !r.is_empty()) {
            let who = t.todo.as_deref().unwrap_or("-");
            out.push(Signal { kind: "rethink", detail: format!("{who} 那一轮说后续走不通：{}", crate::style::truncate(r, 80)) });
        }
    }

    // 被挡：计划里有需要人决定却没拆出来的分叉。
    //
    // **只看还没了结的**：`blocked_by` 是履历不是现状——todo 做完之后这一栏原样留着，
    // 用来记「这条当初卡过人」。不排除终态的话，一条早就 done 的 todo 会让这个信号
    // 永远响下去（踩过：t21 完成后每次写回都还在提示「t21 在等你回话」）。
    let blocked: Vec<&str> = state
        .todos
        .iter()
        .filter(|t| !todo::is_terminal(&t.status) && t.blocked_by.iter().any(|b| b == todo::USER))
        .map(|t| t.id.as_str())
        .collect();
    if !blocked.is_empty() {
        out.push(Signal { kind: "blocked", detail: format!("{} 在等你回话", blocked.join("、")) });
    }

    out
}

/// `done` 之后那一行提示；没有信号就返回 None（**沉默是默认**）。
pub fn hint(state: &State) -> Option<String> {
    let s = signals(state);
    if s.is_empty() {
        return None;
    }
    Some(s.iter().map(|x| x.detail.clone()).collect::<Vec<_>>().join(" · "))
}

/// 重估材料包：目标 + 刚做完的 + 剩下的 + 触发的信号，交给模型提**最小改动**。
pub fn packet(state: &State) -> String {
    let mut out = String::new();
    out.push_str(&format!("# 重估一次：{}\n\n", state.goal.text));

    if let Some(last) = state.ticks.iter().rev().find(|t| tick::COUNTED.contains(&t.outcome.as_str())) {
        out.push_str(&format!(
            "刚做完的一轮：{} {}{}\n",
            last.todo.as_deref().unwrap_or("-"),
            last.outcome,
            if last.note.is_empty() { String::new() } else { format!("：{}", last.note) }
        ));
        for p in &last.pitfalls {
            out.push_str(&format!("  ↳ 坑：{}\n", p.replace('\n', " ")));
        }
    }

    let open = todo::open_ordered(state);
    out.push_str(&format!("\n## 剩下的 {} 条\n\n", open.len()));
    if open.is_empty() {
        out.push_str("_没有了。剩下的问题是：这个目标真的达成了吗？没达成就该补任务。_\n");
    } else {
        for &i in &open {
            let t = &state.todos[i];
            let acc = t.acceptance.as_deref().map(|a| format!("\n   验收：{a}")).unwrap_or_default();
            out.push_str(&format!("- {} [P{}] {}{acc}\n", t.id, t.priority, t.text));
        }
    }

    let sig = signals(state);
    if !sig.is_empty() {
        out.push_str("\n## 触发的信号（账本里读出来的）\n\n");
        for s in &sig {
            out.push_str(&format!("- [{}] {}\n", s.kind, s.detail));
        }
    }

    // 只说"有反馈"没用，得把原话给出来——模型要判断的正是"人说的和计划的差在哪"
    let open_ids: Vec<&str> = open.iter().map(|&i| state.todos[i].id.as_str()).collect();
    let voices: Vec<&crate::state::Tick> = state
        .ticks
        .iter()
        .filter(|t| t.outcome == tick::FEEDBACK && t.todo.as_deref().is_some_and(|id| open_ids.contains(&id)))
        .collect();
    if !voices.is_empty() {
        out.push_str("\n## 你在这些任务上说过的话（原话）\n\n");
        for v in voices.iter().rev().take(5) {
            out.push_str(&format!("- {}：{}\n", v.todo.as_deref().unwrap_or("-"), v.note));
        }
    }

    // 只说"有人说走不通"没用，得把原话给出来——这句话就是重规划的全部依据
    let since = state.ticks.iter().rposition(|t| t.outcome == tick::REPLAN).map_or(0, |i| i + 1);
    let doubts: Vec<&crate::state::Tick> =
        state.ticks[since..].iter().filter(|t| t.rethink.as_deref().is_some_and(|r| !r.trim().is_empty())).collect();
    if !doubts.is_empty() {
        out.push_str("\n## 干活的人说后续走不通（原话）\n\n");
        out.push_str("_这几轮**可能全都成功了**——走不通的不是那一轮，是它推翻的那个前提。_\n");
        for t in &doubts {
            out.push_str(&format!("- {}：{}\n", t.todo.as_deref().unwrap_or("-"), t.rethink.as_deref().unwrap_or("").replace('\n', " ")));
        }
    }

    out.push_str("\n## 你要做的\n\n");
    out.push_str("1. 对着**最终目标**看剩下这些任务：还能把目标做成吗？漏了什么？哪条已经没意义了？\n");
    out.push_str("2. 默认只提**最小改动**——改哪条的文本 / 加哪条 / 删（延后）哪条 / 把哪条拆开。\
                  **别重开一张清单**：原计划里还成立的部分要留着（plan repair 优于 full replan）。\n\
                  \x20  **唯一的例外**：上面「后续走不通」那一节里，被推翻的如果是整条路线的前提，\
                  那就照新的现状重排——给一条死路打补丁不叫最小改动。\
                  是打补丁还是重排，说清你按哪种判断的。\n");
    out.push_str("3. 逐条说清为什么改，讲给用户听。\n");
    out.push_str("4. **人点头之后**才动，用现成的命令：\n");
    out.push_str("   - 加：`zloop plan --add \"[P1] 新任务 :: 验收标准\"`\n");
    out.push_str("   - 改：`zloop edit <id> --text \"…\"` / `--acceptance \"…\"` / `--priority 0`\n");
    out.push_str("   - 不做了：`zloop edit <id> --status deferred`\n");
    out.push_str("5. 觉得不用改就明说\"不用改\"——**不改是完全合格的结论**，别为了改而改。\n");
    out.push_str("6. 这一轮不做任何 todo，也不要改代码。\n");
    out
}

// ── 落地：`zloop replan --apply` ────────────────────────────────────────────
//
// 这是这个项目里**第一个让 agent 改自己待办**的能力。护栏必须在代码里强制，不能只写进
// 提示词——提示词管不住模型（`a_headless_replan_round_suggests_but_never_edits_the_plan`
// 那条回归测试就专门演一个"不守规矩的模型"）。每条护栏为什么存在，见
// `docs/ADAPTIVE-REPLAN.md` §8「不加会怎样」。

/// 重排之后未完成条数的绝对上限。防"一次重规划炸出两百条 todo，跑到天荒地老"。
pub const MAX_OPEN: usize = 30;

/// 单次重排的相对上限：最多放大到现在的三倍多一点。
///
/// 绝对上限单独不够——清单只有 2 条时，一次跳到 30 条同样是失控；相对上限单独也不够——
/// 从 20 条翻到 60 条每一步都"只是三倍"。两条都要。
pub fn cap(open_now: usize) -> usize {
    (open_now * 3 + 5).min(MAX_OPEN)
}

/// 哪些 todo **不许**被重排动。
///
/// - 终态（done / deferred）：动了就等于抹历史，`zloop doc` 和长程审计全部失真；
/// - 等人回话的（`blocked_by` 含 `user`）：它身上挂着一个**给人的问题**，
///   agent 没资格替人把问题删掉。
fn is_pinned(t: &crate::state::Todo) -> bool {
    todo::is_terminal(&t.status) || t.blocked_by.iter().any(|b| b == todo::USER)
}

#[derive(Debug)]
pub struct Applied {
    /// 原样留下的（终态 + 等人回话）
    pub kept: Vec<String>,
    /// 被这次重排换掉的
    pub dropped: Vec<String>,
    /// 新建的
    pub added: Vec<String>,
    pub backup: std::path::PathBuf,
}

/// 把新清单落到账本上。**只换未完成且没在等人的那部分**，其余原样保留。
///
/// 违反护栏就整体拒绝并指名是哪一条——半途改一半的计划比不改更糟。
pub fn apply(
    state: &mut State,
    path: &std::path::Path,
    items: &[(u8, String)],
    why: &str,
) -> anyhow::Result<Applied> {
    // 护栏：得有理由。审计的时候要能看出这次改动想解决什么。
    if why.trim().is_empty() {
        anyhow::bail!("护栏「说清为什么」：--why 不能是空的——事后没人看得出这次重排想解决什么");
    }
    // 护栏：有轮次在飞就不改。那个 agent 手上拿着的 todo 可能正要被换掉。
    if let Some(ip) = &state.in_progress {
        anyhow::bail!(
            "护栏「不动在飞的轮次」：{} 正在被执行（{} 起，via {}）。等它写回再重排",
            ip.todo,
            ip.started_at,
            ip.via
        );
    }
    if items.is_empty() {
        anyhow::bail!("护栏「清单不能空」：重排后一条待办都没有，等于悄悄放弃目标。真要收工就把 todo 逐条 done 或 defer");
    }
    // 护栏：每条都要可验证。这是这个仓库里"为什么这条通向目标"的既有表达方式——
    // 说不出怎么验，就是没想清楚它凭什么算一步。
    let naked: Vec<String> = items
        .iter()
        .filter(|(_, raw)| todo::split_acceptance(raw).1.is_none())
        .map(|(_, raw)| crate::style::truncate(raw, 40))
        .collect();
    if !naked.is_empty() {
        anyhow::bail!(
            "护栏「每条都要可验证」：这 {} 条没写验收标准（用 `文本 :: 怎么验`）：\n  - {}",
            naked.len(),
            naked.join("\n  - ")
        );
    }
    // 护栏：规模上限。
    let open_now = state.todos.iter().filter(|t| !is_pinned(t)).count();
    let limit = cap(open_now);
    if items.len() > limit {
        anyhow::bail!(
            "护栏「规模上限」：现在 {open_now} 条未完成，一次最多排到 {limit} 条（三倍多一点，且总数不超过 {MAX_OPEN}），这次给了 {} 条。\n\
             真要这么多，分两次重排，中间跑几轮看看方向对不对",
            items.len()
        );
    }

    // 改之前先留一份。这是账本里唯一一处"批量丢弃用户可见内容"的操作。
    let backup = path.with_file_name(format!(
        "state.json.bak-{}",
        crate::state::now_iso().replace([':', '+'], "")
    ));
    std::fs::copy(path, &backup)?;

    let kept: Vec<String> = state.todos.iter().filter(|t| is_pinned(t)).map(|t| t.id.clone()).collect();
    let dropped: Vec<String> = state.todos.iter().filter(|t| !is_pinned(t)).map(|t| t.id.clone()).collect();
    state.todos.retain(is_pinned);
    // id 从 `next_id` 继续发，**不复用**——复用会让老 tick 挂到新 todo 上，账本当场对不上。
    let added: Vec<String> = todo::add(state, items, false).into_iter().map(|t| t.id).collect();
    Ok(Applied { kept, dropped, added, backup })
}
