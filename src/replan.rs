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
    let fails = tick::fail_streak(&state.ticks);
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

    out.push_str("\n## 你要做的\n\n");
    out.push_str("1. 对着**最终目标**看剩下这些任务：还能把目标做成吗？漏了什么？哪条已经没意义了？\n");
    out.push_str("2. 只提**最小改动**——改哪条的文本 / 加哪条 / 删（延后）哪条 / 把哪条拆开。\
                  **别重开一张清单**：原计划里还成立的部分要留着（plan repair 优于 full replan）。\n");
    out.push_str("3. 逐条说清为什么改，讲给用户听。\n");
    out.push_str("4. **人点头之后**才动，用现成的命令：\n");
    out.push_str("   - 加：`zloop plan --add \"[P1] 新任务 :: 验收标准\"`\n");
    out.push_str("   - 改：`zloop edit <id> --text \"…\"` / `--acceptance \"…\"` / `--priority 0`\n");
    out.push_str("   - 不做了：`zloop edit <id> --status deferred`\n");
    out.push_str("5. 觉得不用改就明说\"不用改\"——**不改是完全合格的结论**，别为了改而改。\n");
    out.push_str("6. 这一轮不做任何 todo，也不要改代码。\n");
    out
}
