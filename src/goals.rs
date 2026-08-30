//! 一个项目里多个目标：**当前**目标始终躺在 `.zloop/state.json`，其余的停在 `.zloop/goals/<id>.json`。
//!
//! 为什么是"换车"而不是"同时加载多份"：`next` / `done` / `status` / runner / Stop hook / fd-lock
//! 全都认 `state.json` 这一个入口，切换只是把当前那份停走、把目标那份开进来，于是
//! "同一时刻只有一个目标在跑"这条不变量一行都不用改。loopx 用 registry 记多个 goal，
//! 代价是 goal 身份、路由冲突、跨项目同步一整套；这里只要文件换个位置。
//!
//! 归档（`.zloop/archive/`）和停放是两件事：停放的还在 `zloop goal list` 里、可以切回来；
//! 归档的是"不打算再回去了"，只留给事后翻。

// `LOCK_WAIT`：搬家事务等锁的上限，和 `state::transaction` 用的是同一档——写命令彼此排队而不是各自开工。
use crate::state::{self, LOCK_WAIT, STATE_DIR};
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
    let out: String = out.chars().take(40).collect();
    out.trim_matches(['-', '.']).to_string()
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
    (1..).map(|n| format!("g{n}")).find(|c| !used.iter().any(|u| u == c)).unwrap_or_else(|| "g".into())
}

/// 读不出来的目标（损坏 / 版本不匹配）**也要出现在清单里**：静默隐藏会让用户以为目标丢了。
pub const BROKEN: &str = "broken";

fn row_of(path: &Path, current: bool) -> Option<Row> {
    if !path.is_file() {
        return None;
    }
    let st = match state::load(path) {
        Ok(st) => st,
        Err(_) => {
            return Some(Row {
                id: path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(),
                text: format!(
                    "(读不出来，文件还在 .zloop/…/{})",
                    path.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default()
                ),
                status: BROKEN.into(),
                done: 0,
                total: 0,
                last: String::new(),
                current,
                path: path.to_path_buf(),
            })
        }
    };
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

/// `resolve` 是靠哪一档对上的。
///
/// 只有 `Id` 是用户**准确说出了**要动哪一个；另外两档是这里替他猜的——猜对了也只是猜对了。
/// 会搬文件的动作（`goal rm`）拿这个区分要不要先让人看一眼，`switch` 不用（切错了再切回来就行）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Match {
    /// id 一字不差
    Id,
    /// id 前缀
    IdPrefix,
    /// 目标文字里包含这个片段
    Text,
}

impl Match {
    /// 除了精确 id 都算"猜的"。
    pub fn is_fuzzy(self) -> bool {
        self != Match::Id
    }

    pub fn zh(self) -> &'static str {
        match self {
            Match::Id => "精确 id",
            Match::IdPrefix => "id 前缀",
            Match::Text => "目标文字片段",
        }
    }
}

/// id 精确 → id 前缀 → 目标文字包含。命中多个就报错，让用户说清楚。
pub fn resolve(root: &Path, needle: &str) -> Result<Row> {
    resolve_match(root, needle).map(|(row, _)| row)
}

/// 同 [`resolve`]，另外告诉调用方是靠哪一档对上的。
pub fn resolve_match(root: &Path, needle: &str) -> Result<(Row, Match)> {
    let needle = needle.trim();
    if needle.is_empty() {
        bail!("要切到哪个目标？`zloop goal list` 看有哪些");
    }
    let rows = list(root);
    if rows.is_empty() {
        bail!("这个项目还没有任何目标：`zloop init \"目标\"`");
    }
    let lower = needle.to_lowercase();
    for (how, pick) in [
        (Match::Id, rows.iter().filter(|r| r.id == needle).collect::<Vec<_>>()),
        (Match::IdPrefix, rows.iter().filter(|r| r.id.to_lowercase().starts_with(&lower)).collect()),
        (Match::Text, rows.iter().filter(|r| r.text.to_lowercase().contains(&lower)).collect()),
    ] {
        match pick.len() {
            0 => continue,
            1 => return Ok((pick[0].clone(), how)),
            _ => {
                let names: Vec<String> =
                    pick.iter().map(|r| format!("{} ({})", r.id, crate::style::truncate(&r.text, 24))).collect();
                bail!("{needle:?} 对上了 {} 个目标：{}。用 id 说清楚", pick.len(), names.join(" / "));
            }
        }
    }
    bail!("没有目标匹配 {needle:?}：`zloop goal list` 看有哪些")
}

/// 动账本之前的安全检查：runner 在跑，或有会话拿着 todo 没写回，都先别动。
///
/// `why` 说清楚"为什么这时候不该动"——切目标是"会让它中途换活"，`compact` 是"会动它
/// 正在读的轮次记录"。装闸的判据是**改的东西 runner 下一轮要不要读**，不是命令叫什么
/// 名字：`compact` 删 tick 就等于改 `fail_streak` / `progress_streak` / 花费 / 配额窗口
/// 这四道闸的输入（A-18）。
pub fn ensure_idle(root: &Path, force: bool, why: &str) -> Result<()> {
    if force {
        return Ok(());
    }
    if let Some(pid) = crate::daemon::running(root) {
        bail!("runner 正在跑（pid {pid}）：{why}。先 `zloop stop`，或加 --force");
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
///
/// **必须在 `state::locked` 内调用**：搬家和后续的开进 / 新建是一个事务，中间被别的进程
/// 插一次 load-modify-save 就会让同一个目标同时出现在两个位置。
///
/// 读不出来的当前目标（损坏 / 版本不匹配）也照搬——多目标的价值之一就是"把坏的停到一边，
/// 开个干净的接着干"，这条路不能被一次解析失败堵死。
pub fn park(root: &Path) -> Result<Option<Row>> {
    let cur = state::state_path(root);
    if !cur.exists() {
        return Ok(None);
    }
    let loaded = state::load(&cur);
    let (id_hint, text_hint) = match &loaded {
        Ok(st) => (st.goal.id.clone(), st.goal.text.clone()),
        Err(_) => (String::new(), String::new()),
    };
    let dir = goals_dir(root);
    fs::create_dir_all(&dir)?;
    // id 要和文件名一一对应：空的、带怪字符的、或者撞了停车位的，都换一个。
    let clean = sanitize_id(&id_hint);
    let id = if clean.is_empty() || dir.join(format!("{clean}.json")).exists() { fresh_id(root, &text_hint) } else { clean };
    let target = dir.join(format!("{id}.json"));
    match loaded {
        Ok(mut st) if id != id_hint => {
            // 换了 id 就得把文件里的 id 一起改，否则 id 和文件名对不上
            st.goal.id = id.clone();
            state::save(&target, &mut st)?;
            fs::remove_file(&cur)?;
        }
        _ => {
            fs::rename(&cur, &target).with_context(|| format!("停放 {} → {}", cur.display(), target.display()))?;
        }
    }
    Ok(row_of(&target, false))
}

/// park 的反向操作：把刚停走的那份搬回 `state.json`。
///
/// 只在"park 之后、这一步失败了"时调用。同一文件系统内的 rename 是原子的，所以这就是回滚：
/// 项目要么停在旧目标上，要么开在新目标上，不会两头都没有。
fn unpark(root: &Path, parked: &Option<Row>) {
    let Some(row) = parked else { return };
    let cur = state::state_path(root);
    if cur.exists() {
        return;
    }
    let _ = fs::rename(&row.path, &cur);
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
///
/// 整段在 state 锁内完成：先把该拒的全拒掉（目标不存在、runner 在跑、有轮次没写回），
/// 再动文件；开进失败就把停走的那份搬回来。
pub fn switch(root: &Path, needle: &str, force: bool) -> Result<Switched> {
    let path = state::state_path(root);
    state::locked(&path, LOCK_WAIT, || {
        let want = resolve(root, needle)?;
        if want.current {
            return Ok(Switched { parked: None, current: want });
        }
        ensure_idle(root, force, "换目标会让它中途换活")?;
        let parked_row = park(root)?;
        if let Err(e) = engage(root, &want.path) {
            unpark(root, &parked_row);
            return Err(e);
        }
        match row_of(&path, true) {
            Some(current) => Ok(Switched { parked: parked_row, current }),
            None => {
                unpark(root, &parked_row);
                bail!("切换后读不出状态：{}", path.display())
            }
        }
    })
}

/// 新目标：停走当前的，在 `state.json` 上开一个新的。
///
/// 顺序是**先校验完再动文件**：id 不合法、id 撞了、runner 在跑、有轮次没写回，都要在 park 之前拒掉。
/// 反过来（老的实现）会让一次参数打错就把项目留在"没有当前目标"的状态。
pub fn create(root: &Path, text: &str, id: Option<&str>, force: bool) -> Result<(Option<Row>, Row)> {
    let text = text.trim();
    if text.is_empty() {
        bail!("目标不能是空的");
    }
    let path = state::state_path(root);
    state::locked(&path, LOCK_WAIT, || {
        // 早拒：不合法或已被占用的 --id，在动任何文件之前就 bail
        let requested = match id {
            Some(raw) => {
                let clean = sanitize_id(raw);
                if clean.is_empty() {
                    bail!("--id {raw:?} 里没有可用字符（只留 a-z 0-9 . _ -）");
                }
                // 当前目标的 id 也算被占：它马上就要停到 goals/<id>.json 去
                if taken(root).iter().any(|u| u == &clean) {
                    bail!("id {clean:?} 已经有人用了：`zloop goal list`");
                }
                Some(clean)
            }
            None => None,
        };
        ensure_idle(root, force, "换目标会让它中途换活")?;
        let parked_row = park(root)?;
        // id 要在 park **之后**定：当前目标读不出来时 `taken()` 数不到它，停走的那份可能
        // 刚占掉一个 `g<N>`，这里再看一次才不会和它撞成同一个 id。
        let id = match requested {
            Some(clean) => {
                if taken(root).iter().any(|u| u == &clean) {
                    unpark(root, &parked_row);
                    bail!("id {clean:?} 已经有人用了：`zloop goal list`");
                }
                clean
            }
            None => fresh_id(root, text),
        };
        let mut st = state::default_state(text, &id);
        if let Err(e) = state::save(&path, &mut st) {
            unpark(root, &parked_row);
            return Err(e);
        }
        match row_of(&path, true) {
            Some(current) => Ok((parked_row, current)),
            None => {
                unpark(root, &parked_row);
                bail!("新目标写完读不出来：{}", path.display())
            }
        }
    })
}

/// 这条 todo 是不是某个**停放中**的目标派给"我"的？
///
/// `goal switch --force` 会把带着在飞派活的目标一起停走，而 `done` 只认 todo id + 当前
/// `state.json`：于是那个会话的写回会落到**新目标**头上——新目标的同名 todo 被标成完成、
/// note 是旧目标的成果，旧目标的账本一条记录都没有。判断条件收得很紧（同一个 session、
/// 同一个 todo id、派活还没过期），所以只会在真的串了目标时拦人。
pub fn parked_holder(root: &Path, todo_id: &str, who: &crate::session::HostSession) -> Option<Row> {
    let mine = who.session.as_deref()?;
    let now = state::now();
    for row in parked(root) {
        let Ok(st) = state::load(&row.path) else { continue };
        let Some(ip) = &st.in_progress else { continue };
        if ip.todo != todo_id || ip.session.as_deref() != Some(mine) {
            continue;
        }
        if st.policy.stale_after_min <= 0 {
            continue; // 保护被关掉了
        }
        let stale =
            state::parse_iso(&ip.started_at).map(|s| (now - s).num_minutes() >= st.policy.stale_after_min).unwrap_or(true);
        if !stale {
            return Some(row);
        }
    }
    None
}

/// 归档之前要拒的：当前目标不能就地归档。
///
/// 单独拎出来是为了让调用方能在**问用户之前**先把这种拒掉——先弹一句"确认归档？"、
/// 等人敲完 y 再说"其实这个不能归档"是最难受的顺序。
pub fn ensure_archivable(row: &Row) -> Result<()> {
    if row.current {
        bail!("{} 是当前目标：先 `zloop goal switch <别的>` 再归档它", row.id);
    }
    Ok(())
}

/// 归档一个停着的目标：搬到 `.zloop/archive/`，从 `goal list` 里消失，但文件还在。
///
/// 收 `&Row` 而不是 needle，是因为"对上了谁"要先给用户看过（见 `cmd_goal` 的 `Rm` 分支）：
/// 这里再 resolve 一次就有可能搬走另一个（两次 resolve 之间目标清单可能变了）。
pub fn archive(root: &Path, row: &Row) -> Result<PathBuf> {
    ensure_archivable(row)?;
    let dir = root.join(STATE_DIR).join(ARCHIVE_DIR);
    fs::create_dir_all(&dir)?;
    // 读不出来的目标也要能归档掉，否则 `goal list` 里那行"损坏"永远清不掉
    let created = state::load(&row.path).map(|st| st.goal.created_at).unwrap_or_else(|_| state::now_iso());
    let stamp = created.replace([':', '+'], "");
    let mut target = dir.join(format!("{stamp}-{}.json", row.id));
    let mut n = 1;
    while target.exists() {
        n += 1;
        target = dir.join(format!("{stamp}-{}-{n}.json", row.id));
    }
    fs::rename(&row.path, &target)?;
    Ok(target)
}
