//! `zloop reflect`：攒够信号之后回看一次——不做 todo，只读账本、经验、反馈，给出整理建议。
//!
//! 形状照抄 Warp（见 `docs/design/SELF-IMPROVEMENT.md`）：他们的 improver 是**按计划跑的**观察者，
//! 数据模型只有 cron + prompt + enabled + last_spawn_error 六个字段——所以反思不需要新子系统，
//! 它就是"隔一阵子换一段 prompt 跑一轮"。这里对应的是 `zloop reflect`（手动）和
//! `zloop run --reflect-every N`（自动）。
//!
//! **zloop 自己不产生判断**：它只把材料摆齐、做几项机械体检，判断交给模型，落地要人点头
//! （`zloop reflect --apply`）。Warp 那边人审的形态是 PR review；zloop 没有 PR，所以是这条命令。

use crate::state::State;
use crate::stats;
use crate::tick;
use std::path::Path;

/// 两条经验像不像"同一件事"：去掉标点空格后按字符集合算**重合系数**
/// （交集 ÷ 较短那条的长度 ≥ 0.8，即"短的那条基本被长的那条包住"）。
///
/// 用重合系数而不是 Jaccard，是因为真实场景多半是"同一条经验后来写得更细了"——
/// 长度差得多，Jaccard 会把这种明显重复判成不像。宁可多提示：**这里只是给候选，
/// 合不合并由模型判断、由人点头**，误报的代价只是模型多看一眼。
fn similar(a: &str, b: &str) -> bool {
    let norm = |s: &str| -> Vec<char> {
        let mut v: Vec<char> = s.chars().filter(|c| c.is_alphanumeric()).collect();
        v.sort_unstable();
        v.dedup();
        v
    };
    let (x, y) = (norm(a), norm(b));
    if x.len() < 8 || y.len() < 8 {
        return false;
    }
    let common = x.iter().filter(|c| y.contains(c)).count();
    common * 10 >= x.len().min(y.len()) * 8
}

/// 机械体检：能用代码判断的那几条（约定太多、经验重复、被交接包挡在窗口外的数量）。
///
/// 三项都只给**候选**，判断仍旧是模型的事、落地仍旧要人点头。
pub fn checks(n: &crate::notes::Notes, window: usize, rule_limit: usize) -> Vec<String> {
    let mut out = Vec::new();
    // 约定这一层没有窗口兜底：它每轮全量进交接包，攒多了就是在挤别的节。
    // 所以除了条数，把它实际占的篇幅也算出来——「11 条」听着不多，「占默认预算 8%」才是代价。
    if n.rules.len() > rule_limit {
        let chars: usize = n.rules.iter().map(|r| r.chars().count() + 3).sum(); // "- " + 换行
        out.push(format!(
            "共 {} 条约定，超过 {rule_limit} 条——约定不轮换，每轮全量进交接包（约 {chars} 字，占默认预算 {}%），挑几条降回经验或删掉",
            n.rules.len(),
            chars * 100 / crate::context::DEFAULT_BUDGET
        ));
    }
    let notes: Vec<&str> = n.lessons.iter().map(|(_, t)| t.as_str()).collect();
    for (i, a) in notes.iter().enumerate() {
        for (j, b) in notes.iter().enumerate().skip(i + 1) {
            if similar(a, b) {
                out.push(format!("第 {} 条和第 {} 条像是同一件事，考虑合并", i + 1, j + 1));
            }
        }
    }
    if notes.len() > window {
        out.push(format!(
            "共 {} 条经验，但 `zloop context` 每轮只带最新 {window} 条——前 {} 条模型永远看不到，该合并或删掉",
            notes.len(),
            notes.len() - window
        ));
    }
    out
}

/// 一次"我说了什么 / 人回了什么"的配对。
pub struct Pair {
    pub todo: String,
    pub at: String,
    /// agent 那一轮的自述：一句话结果 +（有的话）实现思路的开头
    pub said: Option<String>,
    /// 人的原话
    pub replied: String,
}

/// 把每条用户反馈配到**它之前、同一条 todo 上最近的那个已写回轮次**。
///
/// Warp 的 improver 读的正是这个差——"agent 建议了什么"对上"人最后怎么回应"。
/// 分成两栏各列一遍是看不出差的，必须配对。
pub fn pair_feedback(state: &State, root: &Path) -> Vec<Pair> {
    let mut out = Vec::new();
    for (i, fb) in state.ticks.iter().enumerate() {
        if fb.outcome != tick::FEEDBACK {
            continue;
        }
        let todo = fb.todo.clone().unwrap_or_else(|| "-".into());
        let said =
            state.ticks[..i].iter().rev().find(|t| t.todo == fb.todo && tick::COUNTED.contains(&t.outcome.as_str())).map(
                |t| {
                    let approach = t
                        .log
                        .as_deref()
                        .and_then(|rel| crate::log::read_section(root, rel, "实现思路"))
                        .map(|a| format!("（实现思路：{}）", crate::style::truncate(&a, 120)))
                        .unwrap_or_default();
                    let note = if t.note.is_empty() { format!("[{}]", t.outcome) } else { t.note.clone() };
                    format!("{note}{approach}")
                },
            );
        out.push(Pair { todo, at: fb.at.clone(), said, replied: fb.note.clone() });
    }
    out.reverse(); // 最近的在前
    out
}

/// 反思材料包：给模型看的一整页。
pub fn packet(state: &State, root: &Path, window: usize, rule_limit: usize) -> String {
    let n = crate::notes::read(root);
    let s = stats::compute(state);
    let mut out = String::new();

    out.push_str(&format!("# 回看一次：{}\n\n", state.goal.text));
    out.push_str(&format!(
        "跑了 {} 轮 · 返工 {}（{}）· 失败 {} · 被挡 {} 次 · 无文档 {} 轮 · 用户反馈 {} 条\n",
        s.rounds,
        s.rework,
        (s.rework * 100).checked_div(s.rounds).map(|v| format!("{v}%")).unwrap_or_else(|| "—".into()),
        s.fails,
        s.blocks,
        s.undocumented,
        s.feedback
    ));
    // 口径：上面那行是「这个目标一辈子」，底下的清单和日志只有账本里还剩的。
    // 不说明，模型会拿「跑了 40 轮」去对一张只有 2 条 todo 的清单，然后开始编解释（T29）。
    if s.archived_ticks > 0 {
        let how = if s.archived_rounds_unknown {
            format!("{} 条记录（老版本没记轮次，没算进上面的轮数）", s.archived_ticks)
        } else {
            format!("其中 {} 轮", s.archived_rounds)
        };
        out.push_str(&format!("（{how}已被 `zloop compact` 归档，下面的清单和日志只有还在账本里的那些）\n"));
    }
    if let Some(r) = stats::roughest(&s) {
        out.push_str(&format!("最费劲的是 {}：返工 {} 次\n", r.id, r.rework));
    }

    out.push_str("\n## 现有约定（`.zloop/NOTES.md`，**每轮都带给模型**）\n\n");
    if n.rules.is_empty() {
        out.push_str("_还没有。真正该每轮都遵守的规矩，应该升格到这里——它不轮换。_\n");
    } else {
        for (i, r) in n.rules.iter().enumerate() {
            out.push_str(&format!("R{}. {r}\n", i + 1));
        }
    }

    out.push_str(&format!("\n## 现有经验（全部 {} 条，但每轮只带最新 {window} 条）\n\n", n.lessons.len()));
    if n.lessons.is_empty() {
        out.push_str("_还没有。这一轮如果学到什么，用 `zloop remember` 记下来。_\n");
    } else {
        for (i, (stamp, text)) in n.lessons.iter().enumerate() {
            let day = crate::notes::day_of(stamp);
            let when = if day.is_empty() { String::new() } else { format!("[{day}] ") };
            let cold = if i + window < n.lessons.len() { "（窗口外，模型看不到）" } else { "" };
            out.push_str(&format!("{}. {when}{text}{cold}\n", i + 1));
        }
    }

    let failures = tick::failures(state);
    if !failures.is_empty() {
        out.push_str("\n## 失败与卡住过的地方\n\n");
        for t in failures.iter().take(10) {
            let who = t.todo.as_deref().unwrap_or("-");
            let word = if t.outcome == "block" { "卡住" } else { "失败" };
            out.push_str(&format!("- {} {who} {word}：{}\n", t.at, t.note));
            for p in &t.pitfalls {
                out.push_str(&format!("  ↳ 坑：{}\n", p.replace('\n', " ")));
            }
        }
    }

    let pairs = pair_feedback(state, root);
    if !pairs.is_empty() {
        out.push_str("\n## 我当时怎么说 vs 你怎么回的（**要改进的就是这个差**）\n\n");
        out.push_str("_只列有人回过话的轮次；没人回的轮次不占版面。_\n");
        for p in pairs.iter().take(8) {
            out.push_str(&format!("\n### {} · {}\n", p.todo, p.at));
            match &p.said {
                Some(said) => out.push_str(&format!("- 我当时说：{said}\n")),
                None => out.push_str("- 我当时说：（这条反馈之前没有已写回的轮次）\n"),
            }
            out.push_str(&format!("- 你回的：{}\n", p.replied));
        }
    }

    let checks = checks(&n, window, rule_limit);
    if !checks.is_empty() {
        out.push_str("\n## 机械体检（代码能看出来的）\n\n");
        for c in &checks {
            out.push_str(&format!("- {c}\n"));
        }
    }

    out.push_str(&format!(
        "\n## 你要做的\n\n\
         1. 逐条判断：该**升格成约定**（每轮都带、不轮换）、**留作经验**（会轮换）、**合并**，还是**删掉**。\n\
         \x20  - 升格的标准：这条是不是**每一轮都该照做**的规矩（比如「done 之前先跑测试」）。\
         只在某个阶段有用的观察，留作经验就行。\n\
         \x20  - 反复踩的坑、用户反复说过的同一件事，通常就是该升格的那种。\n\
         \x20  - 约定要少：它每轮都占交接包的篇幅，超过 {rule_limit} 条就该反省是不是塞了不该塞的。\n\
         2. 把整理后的**完整清单**讲给用户看，说清每条为什么升格 / 为什么留 / 为什么删。\n\
         3. **人点头之后**才落地：把下面这个形状从 stdin 交给 `zloop reflect --apply`（旧文件会自动备份）：\n\n\
         \x20  ```\n\
         \x20  ## 约定\n\
         \x20  - 每轮都要遵守的那几条\n\
         \x20  ## 经验\n\
         \x20  - 会轮换的那些\n\
         \x20  ```\n\n\
         \x20  不写小标题的话，全部按经验处理。人没点头就什么都不要写。\n\
         4. 这一轮不做任何 todo，也不要改代码。\n",
    ));
    out
}
