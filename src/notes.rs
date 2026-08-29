//! `zloop remember` / `zloop reflect` 的落脚点：`.zloop/NOTES.md`，**项目级**，分两层。
//!
//! ```markdown
//! ## 约定（每轮都带）      ← 全量注入交接包，不轮换。这是"下一轮真的会照做"的那一层
//! - done 之前一定要跑 cargo test
//!
//! ## 经验（最近 5 条会带）  ← 会轮换，写多了老的就看不到了
//! - 2026-08-29T07:00:00+08:00 bench.sh 要在 release 模式下跑
//! ```
//!
//! **为什么分两层**：Warp 的自改进回路里，改进落在 base skill 上——那是下一轮一定会读的东西
//! （见 `docs/SELF-IMPROVEMENT.md`）。zloop 的 SKILL.md 却是**全局**的，把某个项目的约定写进去
//! 会污染别的项目；而 NOTES 只带最新 5 条，写到第 20 条时前 15 条对模型等于不存在。
//! 所以真正缺的不是"写进 skill"，而是一个**项目级、每轮必读、不轮换**的位置——就是「约定」这一层。
//!
//! 老格式（没有小标题的一串 `- `）照旧能读：全部当成经验。

use crate::state::{now_iso, STATE_DIR};
use anyhow::Result;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

pub const NOTES_FILE: &str = "NOTES.md";
/// `zloop context` 每轮带的**经验**条数（约定不受这个限制）；
/// `reflect` 的体检按同一个数判断"有多少条模型永远看不到"。
pub const WINDOW: usize = 5;

pub const RULES_HEAD: &str = "## 约定（每轮都带）";
pub const LESSONS_HEAD: &str = "## 经验（最近 5 条会带）";
const PREAMBLE: &str = "# zloop notes\n\n_`zloop remember` 写经验，`zloop reflect` 整理；约定每轮都带，经验只带最新几条_\n";

pub fn path(root: &Path) -> PathBuf {
    root.join(STATE_DIR).join(NOTES_FILE)
}

/// 一条经验：**原始时间戳**（RFC3339，没有就是空串）+ 正文。
///
/// 存完整时间戳而不是 `MM-DD`：显示时只用得上日期，但每次重写文件都要把它写回去——
/// 只留日期的话，`remember --rule` 这种顺手的重写会把所有经验的时刻抹成 00:00。
/// 要显示的日期用 `day_of()` 现算。
pub type Lesson = (String, String);

/// 时间戳 → `MM-DD`（给人看的那一半）。
pub fn day_of(stamp: &str) -> String {
    crate::state::parse_iso(stamp).map(|d| d.format("%m-%d").to_string()).unwrap_or_default()
}

#[derive(Debug, Default, Clone)]
pub struct Notes {
    /// 每轮都注入，不轮换
    pub rules: Vec<String>,
    /// 只注入最新几条
    pub lessons: Vec<Lesson>,
}

/// 把 `- <RFC3339> 正文` 拆成（时间, 正文）。
///
/// 写进去时带了时间戳，给人和模型看时只要正文——比较两条经验像不像时尤其如此，
/// 日期字符会把相似度整体抬高（踩过）。
fn split_stamp(line: &str) -> (Option<String>, String) {
    match line.split_once(' ') {
        Some((head, rest)) if crate::state::parse_iso(head).is_ok() => (Some(head.to_string()), rest.trim().to_string()),
        _ => (None, line.to_string()),
    }
}

/// 读整份 NOTES：认小标题分两层；没有小标题的老文件全算经验。
pub fn read(root: &Path) -> Notes {
    let Ok(raw) = fs::read_to_string(path(root)) else { return Notes::default() };
    parse(&raw)
}

/// 解析两层格式。`## 约定` / `## 经验` 认前缀，所以标题后面写什么都行。
pub fn parse(raw: &str) -> Notes {
    let mut n = Notes::default();
    let mut in_rules = false;
    for line in raw.lines() {
        let t = line.trim();
        if t.starts_with("## ") {
            in_rules = t.starts_with("## 约定");
            continue;
        }
        let Some(item) = t.strip_prefix("- ") else { continue };
        let item = item.trim();
        if item.is_empty() {
            continue;
        }
        if in_rules {
            n.rules.push(split_stamp(item).1);
        } else {
            let (stamp, text) = split_stamp(item);
            n.lessons.push((stamp.unwrap_or_default(), text));
        }
    }
    n
}

pub fn remember(root: &Path, text: &str) -> Result<PathBuf> {
    let p = path(root);
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent)?;
    }
    let fresh = !p.exists();
    let mut f = fs::OpenOptions::new().create(true).append(true).open(&p)?;
    if fresh {
        // 经验区放最后，所以以后每次追加都落在它下面
        writeln!(f, "{PREAMBLE}\n{LESSONS_HEAD}\n")?;
    }
    let one_line = text.trim().replace('\n', " ");
    writeln!(f, "- {} {}", now_iso(), one_line)?;
    Ok(p)
}

/// 交接包每轮要带的：全部约定 + 最新 `n` 条经验（正文，去掉时间戳）。
pub fn recent(root: &Path, n: usize) -> Vec<String> {
    let mut items: Vec<String> = read(root).lessons.into_iter().map(|(_, t)| t).collect();
    let keep = items.len().saturating_sub(n);
    items.drain(..keep);
    items
}

/// 全部经验的正文，最早的在前。
pub fn all(root: &Path) -> Vec<String> {
    read(root).lessons.into_iter().map(|(_, t)| t).collect()
}

/// 渲染成文件内容。约定在前、经验在后——经验放最后，`remember` 的追加才会落在它下面。
fn render(n: &Notes) -> String {
    let mut out = String::from(PREAMBLE);
    if !n.rules.is_empty() {
        out.push_str(&format!("\n{RULES_HEAD}\n\n"));
        for r in &n.rules {
            out.push_str(&format!("- {}\n", r.trim()));
        }
    }
    out.push_str(&format!("\n{LESSONS_HEAD}\n\n"));
    let today = now_iso();
    for (stamp, text) in &n.lessons {
        // 有原始时间戳就原样写回；模型整理时新写的那些按今天记
        let stamp = if stamp.is_empty() { &today } else { stamp };
        out.push_str(&format!("- {stamp} {}\n", text.trim()));
    }
    out
}

fn write(root: &Path, n: &Notes) -> Result<PathBuf> {
    let p = path(root);
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = p.with_extension("md.tmp");
    fs::write(&tmp, render(n))?;
    fs::rename(&tmp, &p)?;
    Ok(p)
}

/// 直接钉一条约定（`zloop remember --rule`）。返回（文件路径, 是不是本来就有了）。
///
/// **不备份**：加一条是纯增量，和 `reflect --apply` 的重写不是一回事——
/// 后者会删掉东西，所以那边必须留原件。这边每次都备份只会把 `.zloop/` 塞满。
pub fn add_rule(root: &Path, text: &str) -> Result<(PathBuf, bool)> {
    let one_line = text.trim().replace('\n', " ");
    let mut n = read(root);
    if n.rules.iter().any(|r| r == &one_line) {
        return Ok((path(root), true));
    }
    n.rules.push(one_line);
    Ok((write(root, &n)?, false))
}

/// 整理之后重写整份 NOTES.md：**先备份再写**。
///
/// 这是 zloop 里唯一一个"删掉用户内容"的操作，所以照 Warp 的 `mutate_global_registry` 那样，
/// 改之前把原件留一份（`NOTES.md.bak-<时间戳>`），返回备份路径让调用方讲给用户听。
///
/// 升格成约定的那些丢掉时间戳（它们不再轮换，日期没有意义）；留在经验里的**原样保留**，
/// 模型新写的按今天记——否则整理一次就把"这条多老了"抹平了。
pub fn replace(root: &Path, n: &Notes) -> Result<(PathBuf, PathBuf)> {
    let p = path(root);
    let backup = p.with_file_name(format!("NOTES.md.bak-{}", now_iso().replace([':', '+'], "")));
    if p.exists() {
        fs::copy(&p, &backup)?;
    }
    Ok((write(root, n)?, backup))
}

