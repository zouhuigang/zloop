//! 一个项目里多个目标：**当前**目标始终躺在 `.zloop/state.json`，其余的停在 `.zloop/goals/<id>.json`。
//!
//! 为什么是"换车"而不是"同时加载多份"：`next` / `done` / `status` / runner / Stop hook / fd-lock
//! 全都认 `state.json` 这一个入口，切换只是把当前那份停走、把目标那份开进来，于是
//! "同一时刻只有一个目标在跑"这条不变量一行都不用改。loopx 用 registry 记多个 goal，
//! 代价是 goal 身份、路由冲突、跨项目同步一整套；这里只要文件换个位置。
//!
//! 归档（`.zloop/archive/`）和停放是两件事：停放的还在 `zloop goal list` 里、可以切回来；
//! 归档的是"不打算再回去了"，只留给事后翻。

use crate::state::{self, STATE_DIR};
use anyhow::{bail, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

pub const GOALS_DIR: &str = "goals";
pub const ARCHIVE_DIR: &str = "archive";

#[derive(Debug, Clone)]
pub struct Row {
    pub id: String,
    pub text: String,
    pub status: String,
    pub done: usize,
    pub total: usize,
    /// 最近一次 tick 的时间；没跑过就是创建时间
    pub last: String,
    pub current: bool,
    pub path: PathBuf,
}

pub fn goals_dir(root: &Path) -> PathBuf {
    root.join(STATE_DIR).join(GOALS_DIR)
}

/// 文件名安全的 id：只留 ASCII 字母数字和 `._-`，其余压成 `-`。
pub fn sanitize_id(raw: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in raw.trim().chars() {
        let keep = ch.is_ascii_alphanumeric() || ch == '.' || ch == '_' || ch == '-';
        if keep {
            out.push(ch.to_ascii_lowercase());
            last_dash = ch == '-';
        } else if !last_dash && !out.is_empty() {
            out.push('-');
            last_dash = true;
        }
    }
    let out = out.trim_matches(['-', '.']).to_string();
    out.chars().take(40).collect()
}

/// 目标文字里的 ASCII 词能拼出可读 id 就用它（`keep-awake`），中文目标拼不出就交给 `g<N>`。
fn slug_from_text(text: &str) -> Option<String> {
    let words: Vec<String> = text
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| w.len() > 1 && w.chars().any(|c| c.is_ascii_alphabetic()))
        .take(3)
        .map(|w| w.to_ascii_lowercase())
        .collect();
    let slug = sanitize_id(&words.join("-"));
    (slug.len() >= 3).then_some(slug)
}

fn taken(root: &Path) -> Vec<String> {
    let mut ids: Vec<String> = parked(root).into_iter().map(|r| r.id).collect();
    if let Ok(st) = state::load(&state::state_path(root)) {
        ids.push(st.goal.id);
    }
    ids
}

/// 项目内没被占用的 id：优先目标文字里的 ASCII 词，否则 `g1` / `g2` / …
pub fn fresh_id(root: &Path, text: &str) -> String {
    let used = taken(root);
    if let Some(slug) = slug_from_text(text) {
        if !used.iter().any(|u| u == &slug) {
            return slug;
        }
    }
    (1..)
        .map(|n| format!("g{n}"))
        .find(|c| !used.iter().any(|u| u == c))
        .unwrap_or_else(|| "g".into())
}

fn row_of(path: &Path, current: bool) -> Option<Row> {
    let st = state::load(path).ok()?;
    Some(Row {
        id: st.goal.id.clone(),
        text: st.goal.text.clone(),
        status: st.goal.status.clone(),
        done: st.todos.iter().filter(|t| t.status == "done").count(),
        total: st.todos.len(),
        last: st.ticks.last().map(|t| t.at.clone()).unwrap_or_else(|| st.goal.created_at.clone()),
        current,
        path: path.to_path_buf(),
    })
}

/// 停着的目标，按最近活动倒序。
pub fn parked(root: &Path) -> Vec<Row> {
    let dir = goals_dir(root);
    let Ok(entries) = fs::read_dir(&dir) else { return Vec::new() };
    let mut rows: Vec<Row> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().map(|e| e == "json").unwrap_or(false))
        .filter_map(|p| row_of(&p, false))
        .collect();
    rows.sort_by(|a, b| b.last.cmp(&a.last));
    rows
}

/// 当前目标在最前，其余按最近活动倒序。
pub fn list(root: &Path) -> Vec<Row> {
    let mut rows = Vec::new();
    if let Some(cur) = row_of(&state::state_path(root), true) {
        rows.push(cur);
    }
    rows.extend(parked(root));
    rows
}

/// id 精确 → id 前缀 → 目标文字包含。命中多个就报错，让用户说清楚。
pub fn resolve(root: &Path, needle: &str) -> Result<Row> {
    let needle = needle.trim();
    if needle.is_empty() {
        bail!("要切到哪个目标？`zloop goal list` 看有哪些");
    }
    let rows = list(root);
    if rows.is_empty() {
        bail!("这个项目还没有任何目标：`zloop init \"目标\"`");
    }
    let lower = needle.to_lowercase();
    for pick in [
        rows.iter().filter(|r| r.id == needle).collect::<Vec<_>>(),
        rows.iter().filter(|r| r.id.to_lowercase().starts_with(&lower)).collect(),
        rows.iter().filter(|r| r.text.to_lowercase().contains(&lower)).collect(),
    ] {
        match pick.len() {
            0 => continue,
            1 => return Ok(pick[0].clone()),
            _ => {
                let names: Vec<String> = pick.iter().map(|r| format!("{} ({})", r.id, crate::style::truncate(&r.text, 24))).collect();
                bail!("{needle:?} 对上了 {} 个目标：{}。用 id 说清楚", pick.len(), names.join(" / "));
            }
        }
    }
    bail!("没有目标匹配 {needle:?}：`zloop goal list` 看有哪些")
}

/// 切换 / 新建之前的安全检查：runner 在跑，或有会话拿着 todo 没写回，都先别动。
pub fn ensure_idle(root: &Path, force: bool) -> Result<()> {
    if force {
        return Ok(());
    }
    if let Some(pid) = crate::daemon::running(root) {
        bail!("runner 正在跑（pid {pid}）：换目标会让它中途换活。先 `zloop stop`，或加 --force");
    }
    if let Ok(st) = state::load(&state::state_path(root)) {
        if let Some(ip) = &st.in_progress {
            bail!(
                "有会话正拿着 {} 第 {} 轮还没写回：先 `zloop done {}` 收尾（或 `zloop edit {} --status open` 放回去），或加 --force",
                ip.todo,
                ip.round,
                ip.todo,
                ip.todo
            );
        }
    }
    Ok(())
}

/// 把当前目标停到 `goals/<id>.json`。没有 state.json 就什么都不做。
pub fn park(root: &Path) -> Result<Option<Row>> {
    let cur = state::state_path(root);
    if !cur.exists() {
        return Ok(None);
    }
    let mut st = state::load(&cur)?;
    let dir = goals_dir(root);
    fs::create_dir_all(&dir)?;
    // id 要和文件名一一对应：空的、带怪字符的、或者撞了停车位的，都换一个。
    let clean = sanitize_id(&st.goal.id);
    let id = if clean.is_empty() || dir.join(format!("{clean}.json")).exists() {
        fresh_id(root, &st.goal.text)
    } else {
        clean
    };
    let target = dir.join(format!("{id}.json"));
    if id == st.goal.id {
        fs::rename(&cur, &target).with_context(|| format!("停放 {} → {}", cur.display(), target.display()))?;
    } else {
        st.goal.id = id.clone();
        state::save(&target, &mut st)?;
        fs::remove_file(&cur)?;
    }
    Ok(row_of(&target, false))
}

/// 把停着的那份开进 `state.json`（要求当前位置是空的）。
fn engage(root: &Path, from: &Path) -> Result<()> {
    let cur = state::state_path(root);
    if cur.exists() {
        bail!("当前目标还没停走，内部状态不一致：{}", cur.display());
    }
    fs::rename(from, &cur).with_context(|| format!("开进 {} → {}", from.display(), cur.display()))?;
    Ok(())
}

pub struct Switched {
    pub parked: Option<Row>,
    pub current: Row,
}

/// 停当前 + 开目标那份。已经是当前目标就什么都不做。
pub fn switch(root: &Path, needle: &str) -> Result<Switched> {
    let want = resolve(root, needle)?;
    if want.current {
        return Ok(Switched { parked: None, current: want });
    }
    let parked_row = park(root)?;
    engage(root, &want.path)?;
    let current = row_of(&state::state_path(root), true).context("切换后读不出状态")?;
    Ok(Switched { parked: parked_row, current })
}

/// 新目标：停走当前的，在 `state.json` 上开一个新的。
pub fn create(root: &Path, text: &str, id: Option<&str>) -> Result<(Option<Row>, Row)> {
    let text = text.trim();
    if text.is_empty() {
        bail!("目标不能是空的");
    }
    let parked_row = park(root)?;
    let id = match id {
        Some(raw) => {
            let clean = sanitize_id(raw);
            if clean.is_empty() {
                bail!("--id {raw:?} 里没有可用字符（只留 a-z 0-9 . _ -）");
            }
            if goals_dir(root).join(format!("{clean}.json")).exists() {
                bail!("id {clean:?} 已经有人用了：`zloop goal list`");
            }
            clean
        }
        None => fresh_id(root, text),
    };
    let path = state::state_path(root);
    let mut st = state::default_state(text, &id);
    state::locked(&path, std::time::Duration::from_secs(5), || state::save(&path, &mut st))?;
    let current = row_of(&path, true).context("新目标写完读不出来")?;
    Ok((parked_row, current))
}

/// 归档一个停着的目标：搬到 `.zloop/archive/`，从 `goal list` 里消失，但文件还在。
pub fn archive(root: &Path, needle: &str) -> Result<(Row, PathBuf)> {
    let row = resolve(root, needle)?;
    if row.current {
        bail!("{} 是当前目标：先 `zloop goal switch <别的>` 再归档它", row.id);
    }
    let st = state::load(&row.path)?;
    let dir = root.join(STATE_DIR).join(ARCHIVE_DIR);
    fs::create_dir_all(&dir)?;
    let stamp = st.goal.created_at.replace([':', '+'], "");
    let mut target = dir.join(format!("{stamp}-{}.json", row.id));
    let mut n = 1;
    while target.exists() {
        n += 1;
        target = dir.join(format!("{stamp}-{}-{n}.json", row.id));
    }
    fs::rename(&row.path, &target)?;
    Ok((row, target))
}
