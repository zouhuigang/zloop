//! Execution records: one Markdown file per non-noop tick under `.zloop/log/`.

use crate::session::{self, Host};
use crate::state::{parse_iso, State, Tick, Todo, STATE_DIR};
use anyhow::{Context, Result};
use chrono::{DateTime, FixedOffset};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

pub const LOG_DIR: &str = "log";

/// Resolve `--evidence` input: `@path` reads a file, anything else is literal text.
pub fn resolve_evidence(raw: Option<&str>) -> Result<Option<String>> {
    match raw {
        None => Ok(None),
        Some(s) if s.starts_with('@') => {
            let path = &s[1..];
            let body = fs::read_to_string(path).with_context(|| format!("reading evidence file {path}"))?;
            Ok(Some(body))
        }
        Some(s) => Ok(Some(s.to_string())),
    }
}

/// What a round is expected to leave behind: not just "it worked", but how and what bit us.
#[derive(Debug, Default, Clone)]
pub struct Doc {
    /// 实现思路：why this approach, how it works.
    pub approach: Option<String>,
    /// 关键决策 / 取舍.
    pub decisions: Vec<String>,
    /// 遇到的坑 and what the conclusion was.
    pub pitfalls: Vec<String>,
    /// 验证证据: command output, test names, measurements.
    pub evidence: Option<String>,
    /// Filled in automatically from git.
    pub changed_files: Option<String>,
}

impl Doc {
    /// A round counts as documented once it explains *how* it was done.
    pub fn is_complete(&self) -> bool {
        self.approach.as_deref().map(|s| !s.trim().is_empty()).unwrap_or(false)
    }

    pub fn is_empty(&self) -> bool {
        !self.is_complete() && self.decisions.is_empty() && self.pitfalls.is_empty() && self.evidence.is_none()
    }
}

/// Working-tree changes at write-back time: `git diff --stat` plus untracked files, `.zloop/` excluded.
/// `None` outside a repo or when nothing changed. Capped so a huge round cannot bloat the doc.
pub fn changed_files(root: &Path) -> Option<String> {
    use std::process::Command;
    let git = |args: &[&str]| Command::new("git").args(args).current_dir(root).output().ok();
    if !git(&["rev-parse", "--is-inside-work-tree"])?.status.success() {
        return None;
    }
    let mut out = String::new();
    if let Some(o) = git(&["diff", "--stat", "HEAD", "--", "."]) {
        for line in String::from_utf8_lossy(&o.stdout).lines() {
            if !line.trim_start().starts_with(".zloop") {
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    if let Some(o) = git(&["ls-files", "--others", "--exclude-standard", "--", "."]) {
        let new: Vec<String> = String::from_utf8_lossy(&o.stdout)
            .lines()
            .filter(|l| !l.starts_with(".zloop"))
            .map(|l| format!("  {l} (new)"))
            .collect();
        if !new.is_empty() {
            out.push_str(&new.join("\n"));
            out.push('\n');
        }
    }
    let out = out.trim_end().to_string();
    if out.is_empty() {
        return None;
    }
    let capped: String = out.lines().take(40).collect::<Vec<_>>().join("\n");
    Some(if out.lines().count() > 40 { format!("{capped}\n  … (truncated)") } else { capped })
}

/// Write the log file for a tick; returns the path relative to `.zloop/`.
pub fn write(root: &Path, state: &State, tick: &Tick, todo: &Todo, doc: &Doc) -> Result<String> {
    let dir = root.join(STATE_DIR).join(LOG_DIR);
    fs::create_dir_all(&dir)?;
    let stamp = parse_iso(&tick.at)
        .map(|dt| dt.format("%Y%m%d-%H%M%S").to_string())
        .unwrap_or_else(|_| tick.at.replace([':', '-', 'T', '+'], ""));
    let base = format!("{stamp}-{}-{}", todo.id, tick.outcome);
    let mut name = format!("{base}.md");
    let mut n = 1;
    while dir.join(&name).exists() {
        n += 1;
        name = format!("{base}-{n}.md");
    }
    let host = tick.host.clone().unwrap_or_else(|| "cli".into());
    let session = tick.session.clone().unwrap_or_else(|| "-".into());
    let resume = tick
        .session
        .as_deref()
        .and_then(|s| Host::parse(&host).and_then(|h| session::resume_command(h, s)))
        .map(|c| format!("`{c}`"))
        .unwrap_or_else(|| "-".into());
    let mut body = String::new();
    body.push_str(&format!("# {} · {} · {}\n\n", todo.id, tick.outcome, tick.at));
    body.push_str(&format!("- goal: {}\n", state.goal.text));
    body.push_str(&format!("- todo: [P{}] {}\n", todo.priority, todo.text));
    if let Some(acc) = &todo.acceptance {
        body.push_str(&format!("- acceptance: {acc}\n"));
    }
    body.push_str(&format!("- outcome: {}   round: {}\n", tick.outcome, tick.round));
    body.push_str(&format!("- host: {host}   session: {session}   resume: {resume}\n"));
    if tick.cost_usd.is_some() || tick.duration_ms.is_some() {
        body.push_str(&format!(
            "- cost: {}   turns: {}   duration: {}\n",
            tick.cost_usd.map(|c| format!("${c:.4}")).unwrap_or_else(|| "-".into()),
            tick.num_turns.map(|n| n.to_string()).unwrap_or_else(|| "-".into()),
            tick.duration_ms.map(|d| format!("{}s", d / 1000)).unwrap_or_else(|| "-".into()),
        ));
    }
    if !tick.note.is_empty() {
        body.push_str(&format!("- note: {}\n", tick.note));
    }

    let section = |body: &mut String, title: &str, text: &str| {
        let text = text.trim_end();
        if !text.is_empty() {
            body.push_str(&format!("\n## {title}\n\n{text}\n"));
        }
    };
    if let Some(a) = &doc.approach {
        section(&mut body, "实现思路", a);
    }
    if !doc.decisions.is_empty() {
        let list = doc.decisions.iter().map(|d| format!("- {}", d.trim())).collect::<Vec<_>>().join("\n");
        section(&mut body, "关键决策", &list);
    }
    if !doc.pitfalls.is_empty() {
        let list = doc.pitfalls.iter().map(|p| format!("- {}", p.trim())).collect::<Vec<_>>().join("\n");
        section(&mut body, "遇到的坑", &list);
    }
    if let Some(ev) = &doc.evidence {
        section(&mut body, "验证证据", ev);
    }
    if let Some(files) = &doc.changed_files {
        section(&mut body, "改动文件", &format!("```\n{}\n```", files.trim_end()));
    }
    if !doc.is_complete() && tick.outcome == "done" {
        body.push_str("\n> ⚠ 这一轮没有留下实现思路（`--approach`），只有结果记录。\n");
    }
    fs::write(dir.join(&name), body)?;
    Ok(format!("{LOG_DIR}/{name}"))
}

/// 从一份日志里读回某个小节的正文（`## <title>` 到下一个 `##` 之间）。
///
/// `--approach` 这些只落在日志文件里，不在账本上；`reflect` 要把"我当时怎么说"和
/// "人怎么回的"配对起来展示，就得回头取一次。只在有反馈的那几轮上调用，量很小。
pub fn read_section(root: &Path, rel: &str, title: &str) -> Option<String> {
    let text = fs::read_to_string(root.join(STATE_DIR).join(rel)).ok()?;
    let mut body = String::new();
    let mut inside = false;
    for line in text.lines() {
        if let Some(h) = line.strip_prefix("## ") {
            if inside {
                break;
            }
            inside = h.trim() == title;
            continue;
        }
        if inside {
            body.push_str(line.trim());
            body.push(' ');
        }
    }
    let body = body.trim().to_string();
    (!body.is_empty()).then_some(body)
}

/// Does this log file carry an 实现思路 section?
pub fn file_is_documented(path: &Path) -> bool {
    fs::read_to_string(path).map(|s| s.contains("\n## 实现思路\n")).unwrap_or(false)
}

/// 文档范围。全空 = 全部轮次，也就是 `zloop doc` 一直以来的行为。
#[derive(Debug, Default, Clone)]
pub struct Range {
    /// 只要最近 N 轮：跨所选 todo 一起按时间排，取最新的 N 轮。
    pub last: Option<usize>,
    /// 这个时刻（含）之后的轮次。
    pub since: Option<DateTime<FixedOffset>>,
    /// 这个时刻（含）之前的轮次。
    pub until: Option<DateTime<FixedOffset>>,
}

impl Range {
    pub fn is_full(&self) -> bool {
        self.last.is_none() && self.since.is_none() && self.until.is_none()
    }

    fn covers(&self, tick: &Tick) -> bool {
        let Ok(at) = parse_iso(&tick.at) else { return true };
        self.since.map(|s| at >= s).unwrap_or(true) && self.until.map(|u| at <= u).unwrap_or(true)
    }

    /// 人能看懂的一行范围说明。
    fn describe(&self) -> String {
        let mut parts = Vec::new();
        if let Some(n) = self.last {
            parts.push(format!("最近 {n} 轮"));
        }
        match (&self.since, &self.until) {
            (Some(s), Some(u)) => parts.push(format!("{} 到 {}", crate::state::format_iso(s), crate::state::format_iso(u))),
            (Some(s), None) => parts.push(format!("{} 之后", crate::state::format_iso(s))),
            (None, Some(u)) => parts.push(format!("{} 之前", crate::state::format_iso(u))),
            (None, None) => {}
        }
        parts.join(" · ")
    }
}

/// Assemble one document from a set of round logs: the file bodies, headings demoted one level.
pub fn assemble(root: &Path, state: &State, todo_ids: &[String], range: &Range) -> String {
    // 先把范围算出来：一份只覆盖部分轮次的文档必须在开头说清楚它省了什么，
    // 否则它看上去和一份完整文档一模一样——那就是在骗读它的人。
    let mine: Vec<usize> = state
        .ticks
        .iter()
        .enumerate()
        .filter(|(_, t)| todo_ids.iter().any(|id| t.todo.as_deref() == Some(id.as_str())))
        .filter(|(_, t)| t.log.is_some() || t.outcome == crate::tick::FEEDBACK)
        .map(|(i, _)| i)
        .collect();
    let mut keep: Vec<usize> = mine.iter().copied().filter(|&i| range.covers(&state.ticks[i])).collect();
    if let Some(n) = range.last {
        // ticks 本来就按时间追加，取尾部 N 条就是"最近 N 轮"。
        let drop = keep.len().saturating_sub(n);
        keep.drain(..drop);
    }
    let (n_kept, dropped) = (keep.len(), mine.len() - keep.len());
    let keep: HashSet<usize> = keep.into_iter().collect();

    let mut out = String::new();
    out.push_str(&format!("# 技术文档 · {}\n\n", state.goal.id));
    out.push_str(&format!("**目标**：{}\n\n", state.goal.text));
    out.push_str(&format!(
        "生成于 {} · 目标状态 {} · 共 {} 条 todo\n",
        crate::state::now_iso(),
        state.goal.status,
        state.todos.len()
    ));
    if !range.is_full() {
        out.push_str(&format!(
            "\n> **范围**：{} —— 收录 {} 轮，省略 {} 轮（`zloop doc` 不带范围参数出全文）\n",
            range.describe(),
            n_kept,
            dropped
        ));
    }

    for id in todo_ids {
        let Some(todo) = state.todos.iter().find(|t| &t.id == id) else { continue };
        // 反馈没有日志文件（就一句话），但它必须和 agent 自述并排出现在时间线上——
        // 只有把"我当时怎么想"和"人当时怎么说"放在一起，这份文档才说得清事情为什么变。
        let rounds: Vec<&crate::state::Tick> = state
            .ticks
            .iter()
            .enumerate()
            .filter(|(i, t)| t.todo.as_deref() == Some(id.as_str()) && keep.contains(i))
            .map(|(_, t)| t)
            .collect();
        // 限了范围就只出范围内有轮次的 todo：否则 `--all --last 3` 还是会摊开几十章空标题。
        if rounds.is_empty() && !range.is_full() {
            continue;
        }
        out.push_str(&format!("\n---\n\n## {} [P{}] {}\n\n", todo.id, todo.priority, todo.text));
        out.push_str(&format!("- 状态：{}\n", todo.status));
        if let Some(a) = &todo.acceptance {
            out.push_str(&format!("- 验收标准：{a}\n"));
        }
        if rounds.is_empty() {
            out.push_str("\n_这条 todo 还没有留下任何轮次记录。_\n");
            continue;
        }
        for tick in rounds {
            if tick.outcome == crate::tick::FEEDBACK {
                out.push_str(&format!("\n### 用户反馈 · {}\n\n> {}\n", tick.at, tick.note));
                continue;
            }
            let rel = tick.log.as_deref().unwrap_or_default();
            let path = root.join(STATE_DIR).join(rel);
            out.push_str(&format!("\n### 轮次 {} · {} · {}\n\n", tick.round, tick.outcome, tick.at));
            match fs::read_to_string(&path) {
                Ok(text) => {
                    let documented = text.contains("\n## 实现思路\n");
                    for line in text.lines() {
                        // drop the file's own title, demote its sections under this round
                        if line.starts_with("# ") {
                            continue;
                        }
                        if let Some(rest) = line.strip_prefix("## ") {
                            out.push_str(&format!("#### {rest}\n"));
                        } else {
                            out.push_str(line);
                            out.push('\n');
                        }
                    }
                    if !documented {
                        out.push_str("\n_（这一轮没有实现思路，只有结果记录）_\n");
                    }
                }
                Err(_) => out.push_str(&format!("_日志文件缺失：{rel}_\n")),
            }
        }
    }
    out
}

/// `.zloop/log/` 是项目级的，而每个目标的 todo id 都从 `t1` 重新开始——所以光看文件名
/// 分不出一份日志属于哪个目标。归属的权威来源是 tick 上记的路径（`tick.log`），它跟着
/// 目标的 state 文件走，`zloop doc` 一直用的就是它。
///
/// 规则：当前目标 tick 里出现过的 → 列；**别的目标**（停放中的，或已归档的）tick 里出现过的 → 不列；
/// 两边都没提到的 → 列（宁可多列，不要把自己的历史藏起来）。
///
/// 最后那一档主要是 `zloop compact` 造出来的：它把老 tick 搬进 `.zloop/archive/compact-*.json`，
/// 那些轮次的日志就此无主。这些几乎总是当前目标自己的过去，所以按"列"处理。
/// `compact-*.json` 不是一份完整 state，`state::load` 会解析失败，正好被下面的 `.ok()` 跳过。
fn logs_of_other_goals(root: &Path) -> HashSet<String> {
    let dirs = [crate::goals::goals_dir(root), root.join(STATE_DIR).join(crate::goals::ARCHIVE_DIR)];
    dirs.iter()
        .filter_map(|d| fs::read_dir(d).ok())
        .flat_map(|rd| rd.flatten().map(|e| e.path()).collect::<Vec<_>>())
        .filter(|p| p.extension().map(|e| e == "json").unwrap_or(false))
        .filter_map(|p| crate::state::load(&p).ok())
        .flat_map(|st| st.ticks.into_iter().filter_map(|t| t.log))
        .collect()
}

/// 一条清单行：文件 + 写它的那一轮（无主文件是 `None`，只能靠文件名猜）。
pub type Entry = (PathBuf, Option<Tick>);

/// 当前目标的日志文件，最新在前；`hidden` 是因为属于别的目标而没列出来的数量。
pub fn entries(root: &Path, state: &State, todo: Option<&str>, last: usize) -> Result<(Vec<Entry>, usize)> {
    let dir = root.join(STATE_DIR).join(LOG_DIR);
    if !dir.is_dir() {
        return Ok((Vec::new(), 0));
    }
    let mine: HashMap<String, Tick> = state
        .ticks
        .iter()
        .filter_map(|t| t.log.clone().map(|rel| (rel, t.clone())))
        .collect();
    let theirs = logs_of_other_goals(root);

    let mut hidden = 0;
    let mut rows: Vec<Entry> = Vec::new();
    let mut paths: Vec<PathBuf> = fs::read_dir(&dir)?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().map(|e| e == "md").unwrap_or(false))
        .collect();
    paths.sort();
    paths.reverse();
    for path in paths {
        let Some(name) = path.file_name().map(|n| n.to_string_lossy().to_string()) else { continue };
        let rel = format!("{LOG_DIR}/{name}");
        let tick = mine.get(&rel);
        if tick.is_none() && theirs.contains(&rel) {
            hidden += 1;
            continue;
        }
        let keep = match (todo, tick) {
            // 认账本：这一轮记在哪条 todo 上是 tick 说的，不是文件名说的
            (Some(id), Some(t)) => t.todo.as_deref() == Some(id),
            (Some(id), None) => name.contains(&format!("-{id}-")),
            (None, _) => true,
        };
        if keep {
            rows.push((path, tick.cloned()));
            if rows.len() >= last {
                break;
            }
        }
    }
    Ok((rows, hidden))
}

/// 无主文件只能靠文件名判断这一轮是不是"完成"：`<stamp>-<todo>-<outcome>[-n].md`。
pub fn name_is_done(path: &Path) -> bool {
    let Some(name) = path.file_name().map(|n| n.to_string_lossy().to_string()) else { return false };
    let stem = name.strip_suffix(".md").unwrap_or(&name);
    // 去掉重名时加的 `-2` / `-3`：它们让 `ends_with("-done.md")` 这种判断全部失效
    let stem = match stem.rsplit_once('-') {
        Some((head, tail)) if tail.chars().all(|c| c.is_ascii_digit()) && !tail.is_empty() => head,
        _ => stem,
    };
    stem.ends_with("-done")
}

pub fn first_line(path: &Path) -> String {
    fs::read_to_string(path)
        .ok()
        .and_then(|s| s.lines().next().map(|l| l.trim_start_matches('#').trim().to_string()))
        .unwrap_or_default()
}

/// 写一份不属于任何 todo 的日志（回看那一轮），返回相对 `.zloop/` 的路径。
///
/// 和 `write` 分开：那个是"某条 todo 的某一轮"，要 Todo 和 Doc；这个只是一段正文。
pub fn write_raw(root: &Path, stem: &str, body: &str) -> Result<String> {
    let dir = root.join(STATE_DIR).join(LOG_DIR);
    fs::create_dir_all(&dir)?;
    let stamp = crate::state::now().format("%Y%m%d-%H%M%S").to_string();
    let base = format!("{stamp}-{stem}");
    let mut name = format!("{base}.md");
    let mut n = 1;
    while dir.join(&name).exists() {
        n += 1;
        name = format!("{base}-{n}.md");
    }
    fs::write(dir.join(&name), body)?;
    Ok(format!("{LOG_DIR}/{name}"))
}

/// Append lines to an existing log file (relative to `.zloop/`), e.g. host cost known only after settlement.
pub fn append(root: &Path, rel: &str, lines: &str) -> Result<()> {
    use std::io::Write;
    let path = root.join(STATE_DIR).join(rel);
    let mut f = fs::OpenOptions::new().append(true).open(&path).with_context(|| format!("appending to {}", path.display()))?;
    f.write_all(lines.as_bytes())?;
    if !lines.ends_with('\n') {
        f.write_all(b"\n")?;
    }
    Ok(())
}
