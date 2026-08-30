//! Bounded handoff packet: what another host or a fresh session needs to continue.
//!
//! Sections are filled in priority order and trimmed from the tail when the
//! character budget is exceeded (the loopx handoff-packet rule).

use crate::session::{self, Host};
use crate::state::State;
use crate::tick;
use crate::todo;
use chrono::{DateTime, FixedOffset};
use std::path::Path;

pub const DEFAULT_BUDGET: usize = 4000;

/// 交接包这一轮少掉了「约定」和「经验」两整节时，要往 stderr 喊的那一句；读得出来就是 `None`。
///
/// [`build`] 用的是宽容版 `notes::read`：NOTES.md 读不出来就当"什么都没记过"，两节一声不吭地
/// 消失，命令照样 exit 0。`zloop doctor` 已经会报这一条（`unreadable_notes`），但**doctor 只在
/// 有人敲的时候才说话**——无头 runner 一轮都不会跑它。护栏丢在哪一轮，就得在哪一轮听得见，
/// 所以 `zloop context` 自己也得出这个声。
///
/// 只出声不改退出码：交接包是每轮的第一条命令，让它 exit 非 0 会把整轮劝退，
/// 而"没有约定"是能降级干活的——能读到的那部分照旧交付。
pub fn notes_warning(root: &Path) -> Option<String> {
    let e = crate::notes::read_error(root)?;
    let p = crate::notes::path(root);
    let p = p.strip_prefix(root).unwrap_or(&p).display().to_string();
    Some(format!(
        "⚠ {p} 读不出来（{e}）：这一轮的交接包里没有「约定」和「经验」两节，项目护栏这一轮是缺的——\
         照它干活等于没看过项目约定。`zloop doctor` 里有怎么修"
    ))
}

fn resume_hint(host: Option<Host>) -> &'static str {
    match host {
        Some(Host::Codex) => "在 Codex 里：先 `zloop context`，再 `zloop next --json`，做完 `zloop done …`。无头续跑：`zloop run --host codex`。",
        Some(Host::Claude) | None => "在 Claude Code 里：`/zloop` 跑一轮，`/loop /zloop` 自动续跑。无头续跑：`zloop run --host claude`。",
        Some(Host::Cli) => "在终端里：`zloop next --json` → 做 → `zloop done …`。",
    }
}

pub fn build(state: &State, root: &Path, budget: usize, for_host: Option<Host>, at: DateTime<FixedOffset>) -> String {
    let mut sections: Vec<String> = Vec::new();

    let spent = tick::spent_total(state);
    let spent_line = if spent > 0.0 || state.policy.max_total_usd > 0.0 {
        format!(
            "\n已花费：${spent:.2}{}",
            if state.policy.max_total_usd > 0.0 { format!(" / 上限 ${:.2}", state.policy.max_total_usd) } else { String::new() }
        )
    } else {
        String::new()
    };
    sections.push(format!(
        "## 目标\n{}\n项目目录：{}\n阶段：{}{}",
        state.goal.text,
        root.display(),
        crate::phase::compute(state, root, at).summary,
        spent_line
    ));

    // 约定紧跟目标：它是"每一轮都该照做"的那一层，既不轮换也不该被篇幅挤掉
    let notes = crate::notes::read(root);
    if !notes.rules.is_empty() {
        sections.push(format!(
            "## 本项目的约定（每轮都要遵守）\n{}",
            notes.rules.iter().map(|r| format!("- {r}")).collect::<Vec<_>>().join("\n")
        ));
    }

    let recent: Vec<String> = state
        .ticks
        .iter()
        .rev()
        // feedback 有自己那一节，不在这里重复
        .filter(|t| t.outcome != "noop" && t.outcome != crate::tick::FEEDBACK)
        .take(3)
        .map(|t| {
            let who = t.todo.as_deref().unwrap_or("-");
            let note = if t.note.is_empty() { String::new() } else { format!("：{}", t.note) };
            format!("- {} {who} {}{}", t.at, t.outcome, note)
        })
        .collect();
    sections.push(format!(
        "## 当前判断（最近 3 次执行）\n{}",
        if recent.is_empty() { "- 尚未开始".to_string() } else { recent.join("\n") }
    ));

    // 人说的话排在"下一条"之前：下一轮先看到要处理什么，再看到该做哪条
    let pending = tick::pending_feedback(state);
    let has_feedback = !pending.is_empty();
    if has_feedback {
        let lines: Vec<String> = pending
            .iter()
            .rev()
            .take(3)
            .map(|t| format!("- {} {}：{}", t.at, t.todo.as_deref().unwrap_or("-"), t.note))
            .collect();
        let earlier = tick::feedback_count(state) - pending.len();
        let tail = if earlier > 0 {
            format!("\n（另有 {earlier} 条更早的反馈已被后续轮次处理过，在 `zloop doc` 里能翻到）")
        } else {
            String::new()
        };
        sections.push(format!("## 用户对上一轮的反馈（先处理这些）\n{}{}", lines.join("\n"), tail));
    }

    // 失败过的地方排在"下一条"之前：先知道哪儿踩过坑，再看要做什么
    let failures = tick::failures(state);
    let has_failures = !failures.is_empty();
    if has_failures {
        let mut lines = Vec::new();
        for t in failures.iter().take(3) {
            let who = t.todo.as_deref().unwrap_or("-");
            let word = if t.outcome == "block" { "卡住" } else { "失败" };
            let note = if t.note.is_empty() { String::new() } else { format!("：{}", t.note) };
            // 不印轮次：fail 不推进轮次计数（`record` 只给 done/progress 加一），
            // 印出来会是"第 0 轮失败"这种读不通的话。时间戳更有用。
            lines.push(format!("- {} {who} {word}{note}", t.at));
            for p in t.pitfalls.iter().take(2) {
                lines.push(format!("  ↳ 坑：{}", p.replace('\n', " ")));
            }
        }
        let more = failures.len().saturating_sub(3);
        let tail = if more > 0 { format!("\n（更早还有 {more} 次，`zloop log` 里有全部记录）") } else { String::new() };
        sections.push(format!("## 本目标失败过的地方（别重复踩）\n{}{}", lines.join("\n"), tail));
    }

    let decision = tick::decide(state, at);
    let next = match &decision.todo {
        Some(t) => format!("{} [P{}] {}", t.id, t.priority, t.text),
        // `unplanned` / `all_deferred` 这两个停机词的字面意思和读者该做的事是相反的：
        // 不是没活了，一个是活还没写下来，一个是活全被推到了以后。直说下一步，
        // 省得被当成"目标已完成"就去开新目标（#5 / B-3）。
        None if decision.reason == "unplanned" => "（unplanned：还没有待办，先 zloop plan 规划几条，别新建目标）".to_string(),
        None if decision.reason == "all_deferred" => {
            "（all_deferred：待办全被延后了，一条都没做完。要么 zloop edit <id> --status open 捡回来，要么 zloop plan 加新的，别当成目标已完成）".to_string()
        }
        None => format!("（{}）", decision.reason),
    };
    sections.push(format!("## 下一条\n{next}"));
    // 到这里为止的都不裁：目标 / 约定 / 当前判断 / 用户反馈 / 失败过的地方 / 下一条。
    // 用"位置"而不是数字，以后再插一节也不用回头改这里（之前就是靠数字算的，加一节就得改一次）。
    let protected = sections.len();

    let open: Vec<String> = todo::open_ordered(state)
        .into_iter()
        .take(5)
        .map(|i| {
            let t = &state.todos[i];
            let deps = if t.blocked_by.is_empty() { String::new() } else { format!(" ⏳{}", t.blocked_by.join(",")) };
            let note = if t.status == "blocked" && !t.note.is_empty() { format!(" — {}", t.note) } else { String::new() };
            let acc = t.acceptance.as_deref().map(|a| format!(" ｜验收：{}", a.chars().take(80).collect::<String>())).unwrap_or_default();
            format!("- {} {} [P{}] {}{}{}{}", crate::prompt::checkbox(&t.status), t.id, t.priority, t.text, deps, note, acc)
        })
        .collect();
    sections.push(format!(
        "## 待办（前 5 条，共 {} 条未完成）\n{}",
        todo::remaining(state),
        match () {
            // 空清单有三种：还没规划过（去 plan）、全做完了（去开新目标）、全被延后了
            // （去捡回来）。写死"全部完成"会让另外两种看着像收工了，读的人就去 goal new
            // 一个重名的（#5 / B-3）。
            _ if !open.is_empty() => open.join("\n"),
            _ if state.todos.is_empty() => "- 还没有待办：先 zloop plan".to_string(),
            _ if todo::all_deferred(state) => {
                format!("- 全被延后了（{} 条），一条都没完成", state.todos.len())
            }
            _ => "- 全部完成".to_string(),
        }
    ));

    let sessions = session::summarize(state, root);
    if !sessions.is_empty() {
        let mut lines = Vec::new();
        for host in ["claude", "codex", "cli"] {
            if let Some(s) = session::latest(&sessions, Some(host)) {
                if let Some(cmd) = &s.resume {
                    lines.push(format!("- {}（最近 {}，{} ticks）：`{}`", host, s.last, s.ticks, cmd));
                }
            }
        }
        if !lines.is_empty() {
            sections.push(format!("## 会话（可回看细节）\n{}", lines.join("\n")));
        }
    }

    let mut lessons: Vec<String> = notes.lessons.iter().map(|(_, t)| t.clone()).collect();
    let dropped = lessons.len().saturating_sub(crate::notes::WINDOW);
    lessons.drain(..dropped);
    if !lessons.is_empty() {
        let more = if dropped > 0 { format!("，另有 {dropped} 条更早的没带上") } else { String::new() };
        sections.push(format!(
            "## 经验（zloop remember，最近 {} 条{more}）\n{}",
            lessons.len(),
            lessons.iter().map(|l| format!("- {l}")).collect::<Vec<_>>().join("\n")
        ));
    }

    sections.push(format!("## 怎么继续\n{}", resume_hint(for_host)));

    // Trim from the tail (keep 目标 / 当前判断 / 下一条 as long as possible).
    // 丢的顺序：先保护区之外的（"怎么继续"是其中最后一个丢的），实在放不下才从
    // 保护区尾部往前丢，"## 目标"永远留着。整节整节地丢，不留半节。
    let render = |secs: &[String]| secs.join("\n\n");
    let mut kept = sections.clone();
    while kept.len() > 1 && render(&kept).chars().count() > budget {
        // 保护区之外还剩两节以上时留住末尾的"怎么继续"；否则就丢最后一节
        let drop_idx = if kept.len() >= protected + 2 { kept.len() - 2 } else { kept.len() - 1 };
        kept.remove(drop_idx);
    }
    let out = render(&kept);
    // 连"## 目标"一节都放不下（预算小到几十个字符）：按整行截，
    // 宁可少一行也不留半行或者一个被切断的"##"标题。
    if out.chars().count() > budget { head_lines(&out, budget) } else { out }
}

/// `text` 里能放进 `budget` 个字符的前若干**完整**行，一行都放不下就返回空串。
fn head_lines(text: &str, budget: usize) -> String {
    let mut used = 0usize;
    let mut end = 0usize; // 已接受的部分在 text 里的字节长度
    for line in text.split_inclusive('\n') {
        let n = line.chars().count();
        if used + n > budget {
            // 末尾那个换行符本身可以不算：整行正好卡在预算边界上时仍然收下
            let bare = line.strip_suffix('\n');
            match bare {
                Some(b) if used + b.chars().count() <= budget => end += b.len(),
                _ => {}
            }
            break;
        }
        used += n;
        end += line.len();
    }
    let mut out = text[..end].trim_end();
    // 光剩一个小标题、底下一行内容都没有，那就连标题一起丢：宁可空着，也不给半个章节
    while out.rsplit('\n').next().is_some_and(|l| l.starts_with("## ")) {
        out = out.rsplit_once('\n').map_or("", |(head, _)| head).trim_end();
    }
    out.to_string()
}
