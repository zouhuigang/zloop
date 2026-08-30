//! `zloop doctor`：只读体检——回答「这个项目的 `.zloop` 有没有问题」。
//!
//! 为什么值得单开一条命令：`goal list` 只显示得出"损坏"这一种毛病（见
//! `docs/GOALS-REVIEW.md` 的 F4 / L4），而真正会把人卡住的是**不报错的不一致**——
//! `goals/` 里 id 和文件名对不上、两个文件抢同一个 id、tick 指着的日志文件被删了。
//! 它们平时一声不吭，只是让某条命令有一天突然不听话（`goal switch` 说"对上了 2 个目标"、
//! `zloop doc` 少了一节）。loopx 在这一层写了 `collect_global_registry_health`，
//! 每条 finding 都带"下一步跑什么"；这里照抄那个**形状**，但不引入 health 子系统：
//! 一个函数扫一遍文件，逐条报"问题 + 建议动作"。
//!
//! **只读是硬约束**：doctor 不修、不删、不动任何文件——连 `daemon::running` 都不能调
//! （它会顺手删掉过期的 pid 文件）。体检和治疗分开，人才敢在任何时候、任何状态下跑它。

use crate::goals;
use crate::state::{self, parse_iso, State, STATE_DIR};
use chrono::{DateTime, FixedOffset};
use serde::Serialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    /// 已经坏了或马上会坏：有命令因此不工作
    Error,
    /// 还能跑，但迟早咬人 / 信息已经丢了
    Warn,
}

#[derive(Debug, Clone, Serialize)]
pub struct Finding {
    /// 稳定的类别标识，给脚本用（人看的在 `what`）
    pub kind: &'static str,
    pub level: Level,
    /// 一句话说清楚问题在哪
    pub what: String,
    /// 下一步该做什么（一条能敲的命令，或一句处置说明）
    pub fix: String,
}

impl Finding {
    fn err(kind: &'static str, what: String, fix: String) -> Finding {
        Finding { kind, level: Level::Error, what, fix }
    }
    fn warn(kind: &'static str, what: String, fix: String) -> Finding {
        Finding { kind, level: Level::Warn, what, fix }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    /// 体检过的目标文件数（当前 + 停放）
    pub goals: usize,
    /// 归档里的目标文件数（`compact-*.json` 不算，它不是一份目标）
    pub archived: usize,
    pub errors: usize,
    pub warnings: usize,
    pub findings: Vec<Finding>,
}

impl Report {
    pub fn ok(&self) -> bool {
        self.findings.is_empty()
    }
}

/// 一份目标文件：读得出就带上 state，读不出留 `None`（那本身就是一条 finding）。
struct GoalFile {
    path: PathBuf,
    /// `goals/<stem>.json` 的 stem；当前目标是 `state`
    stem: String,
    current: bool,
    state: Option<State>,
}

impl GoalFile {
    /// 报告里怎么称呼它：读得出用目标 id，读不出只能用文件名
    fn label(&self) -> String {
        match &self.state {
            Some(st) => st.goal.id.clone(),
            None => self.stem.clone(),
        }
    }
}

/// `.zloop/goals/a.json` 这样的相对路径：报告里贴绝对路径没人想看
fn rel(root: &Path, path: &Path) -> String {
    path.strip_prefix(root).unwrap_or(path).display().to_string()
}

/// 目录里排好序的 `*.json`；目录不存在就是空的（不是错误）
fn json_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = fs::read_dir(dir) else { return Vec::new() };
    let mut paths: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().map(|e| e == "json").unwrap_or(false))
        .collect();
    paths.sort();
    paths
}

fn goal_files(root: &Path) -> Vec<GoalFile> {
    let mut out = Vec::new();
    let cur = state::state_path(root);
    if cur.is_file() {
        out.push(GoalFile { stem: "state".into(), current: true, state: state::load(&cur).ok(), path: cur });
    }
    for p in json_files(&goals::goals_dir(root)) {
        let stem = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        out.push(GoalFile { state: state::load(&p).ok(), stem, current: false, path: p });
    }
    out
}

/// 这个目录像不像一个 zloop 项目：有当前目标，或者目标全停着（headless）都算。
pub fn is_project(root: &Path) -> bool {
    state::state_path(root).is_file() || goals::goals_dir(root).is_dir()
}

pub fn check(root: &Path) -> Report {
    let mut f: Vec<Finding> = Vec::new();
    let files = goal_files(root);

    check_headless(root, &files, &mut f);
    check_goal_files(root, &files, &mut f);
    check_duplicate_ids(root, &files, &mut f);
    let archived = check_archive(root, &mut f);
    // 一次读钟，所有目标共用：同一份报告里两个目标按不同的"现在"体检会自相矛盾。
    let now = state::now();
    for gf in &files {
        if let Some(st) = &gf.state {
            check_ledger(root, gf, st, &mut f);
            check_dep_cycles(gf, st, &mut f);
            check_policy(gf, st, &mut f);
            check_future_timestamps(gf, st, now, &mut f);
        }
    }
    check_pid(root, &mut f);
    check_leftovers(root, &mut f);
    check_notes(root, &mut f);

    // 先要修的，后留意的；同级保持发现顺序（大致是从目标清单到账本再到运行时）
    f.sort_by_key(|x| match x.level {
        Level::Error => 0,
        Level::Warn => 1,
    });
    Report {
        goals: files.len(),
        archived,
        errors: f.iter().filter(|x| x.level == Level::Error).count(),
        warnings: f.iter().filter(|x| x.level == Level::Warn).count(),
        findings: f,
    }
}

/// 没有当前目标：搬家中断，或刚归档掉了当前那个。除了 `goal list` / `goal switch`，
/// 其余命令全部报"没有目标"，而目标其实一份没丢——这条最值得第一时间说清楚。
fn check_headless(root: &Path, files: &[GoalFile], f: &mut Vec<Finding>) {
    if state::state_path(root).is_file() {
        return;
    }
    let parked = files.len();
    if parked == 0 {
        return; // 干净的空项目，不是病
    }
    let hint = files.first().map(|gf| gf.label()).unwrap_or_default();
    f.push(Finding::err(
        "headless",
        format!("当前没有目标在开着（.zloop/state.json 不在），{parked} 个目标停在 .zloop/goals/"),
        format!("zloop goal switch {hint}"),
    ));
}

fn check_goal_files(root: &Path, files: &[GoalFile], f: &mut Vec<Finding>) {
    for gf in files {
        let Some(st) = &gf.state else {
            // 读不出来的目标文件：`goal list` 会显示成"损坏"，但不会告诉你怎么办
            let fix = if gf.current {
                "看一眼文件（是不是被手改坏了）；要把它挪开就 `zloop goal new \"新目标\"`，它会被停到 .zloop/goals/"
                    .to_string()
            } else {
                format!("看一眼文件；确认不要了就 `zloop goal rm {}`（只搬到 .zloop/archive/，不删）", gf.stem)
            };
            f.push(Finding::err("broken_goal", format!("目标文件读不出来：{}", rel(root, &gf.path)), fix));
            continue;
        };
        if gf.current {
            continue;
        }
        // id 和文件名对不上：`park` 是按 id 取文件名的，于是下一次停放会在同一个 id 上
        // 再造一个文件——两份目标一个 id，`goal switch` 从此说"对上了 2 个目标"。
        if st.goal.id != gf.stem {
            f.push(Finding::err(
                "id_filename_mismatch",
                format!("{} 里的 id 是 {:?}，和文件名对不上", rel(root, &gf.path), st.goal.id),
                format!("mv {} {}/{}/{}.json", rel(root, &gf.path), STATE_DIR, goals::GOALS_DIR, st.goal.id),
            ));
        }
    }
}

/// 两个文件同一个 id：loopx 那边叫 route_collision。zloop 的 `resolve` 命中多个就 bail，
/// 于是 `goal switch <id>` / `goal rm <id>` 全都点不动这个 id。
fn check_duplicate_ids(root: &Path, files: &[GoalFile], f: &mut Vec<Finding>) {
    let mut by_id: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for gf in files {
        if let Some(st) = &gf.state {
            by_id.entry(st.goal.id.clone()).or_default().push(rel(root, &gf.path));
        }
    }
    for (id, paths) in by_id {
        if paths.len() > 1 {
            f.push(Finding::err(
                "duplicate_goal_id",
                format!("id {id:?} 有 {} 份：{}", paths.len(), paths.join(" / ")),
                format!("`zloop goal switch {id}` 会因此拒绝执行；打开其中一份把 goal.id 改掉，文件名跟着改成一样的"),
            ));
        }
    }
}

/// 归档目录。`compact-*.json` 不是目标（`zloop compact` 搬出去的老 tick），跳过。
fn check_archive(root: &Path, f: &mut Vec<Finding>) -> usize {
    let dir = root.join(STATE_DIR).join(goals::ARCHIVE_DIR);
    let mut count = 0;
    let mut by_id: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for p in json_files(&dir) {
        let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        if name.starts_with("compact-") {
            continue;
        }
        count += 1;
        match state::load(&p) {
            Ok(st) => by_id.entry(st.goal.id).or_default().push(name),
            Err(_) => f.push(Finding::warn(
                "broken_archive",
                format!("归档文件读不出来：{}", rel(root, &p)),
                // 归档也参与"这份日志属于谁"的判定（log.rs 的 logs_of_other_goals），
                // 读不出就等于它名下的日志全部无主，会被当成当前目标的历史列出来
                "只影响翻旧账和 `zloop log` 的归属判断；确认不要了直接删掉这一个文件".into(),
            )),
        }
    }
    for (id, names) in by_id {
        if names.len() > 1 {
            f.push(Finding::warn(
                "archive_id_collision",
                format!("归档里有 {} 份都叫 {id:?}：{}", names.len(), names.join(" / ")),
                "不影响当前运行（归档不参与 goal 解析）；翻旧账时按文件名开头的时间戳区分".into(),
            ));
        }
    }
    count
}

/// 一份目标账本内部对不对得上。这些都不会让命令报错，只会让它**默默少做一件事**。
fn check_ledger(root: &Path, gf: &GoalFile, st: &State, f: &mut Vec<Finding>) {
    let who = gf.label();
    let ids: Vec<&str> = st.todos.iter().map(|t| t.id.as_str()).collect();
    // 停放中的目标：`zloop edit` / `zloop done` 只认当前目标，照抄建议动作会改错账本
    let scope = if gf.current {
        String::new()
    } else {
        format!("（这条 todo 在停放的 {who} 里：先 `zloop goal switch {who}`）")
    };

    // 1. tick 指着的日志文件不在了：`zloop doc` / `zloop log` 会静默跳过那几轮
    let mut missing: Vec<(&str, &str)> = Vec::new();
    for t in &st.ticks {
        if let Some(relpath) = &t.log {
            if !root.join(STATE_DIR).join(relpath).is_file() {
                missing.push((t.todo.as_deref().unwrap_or("-"), relpath.as_str()));
            }
        }
    }
    if !missing.is_empty() {
        let (todo_id, first) = missing[0];
        f.push(Finding::warn(
            "missing_log",
            format!("[{who}] {} 轮的日志文件不在了（最早 {todo_id} → {first}）", missing.len()),
            "误删就从 git / 备份恢复；不恢复也能跑，只是 `zloop doc` 少了这几轮".into(),
        ));
    }

    // 2. 在飞的派活指着一条不存在的 todo：`zloop done` 认不出这个 id，它会一直挂在那儿，
    //    `goal switch` 也会因为"有轮次没写回"拒绝换目标
    if let Some(ip) = &st.in_progress {
        if !ids.contains(&ip.todo.as_str()) {
            f.push(Finding::err(
                "dangling_in_progress",
                format!("[{who}] 第 {} 轮派出去的 {} 已经不在待办里了", ip.round, ip.todo),
                format!("`zloop done` 认不出这个 id；手工把 state.json 里的 in_progress 删掉，或 `zloop goal switch --force` 绕开{scope}"),
            ));
        } else if let Some(t) = st.todos.iter().find(|t| t.id == ip.todo) {
            // 2b. 派活指着一条**已经了结**的 todo（T42）。`zloop edit --status deferred/done`
            //     从头到尾不碰 `in_progress`，所以人一判「这条不做了」，指针就留在原地。
            //     没人报的时候它是这样咬人的：`status` 一边把它印成「⏭ 已延后」一边说
            //     「正在做 t1」，`compact` / `goal new` / `goal switch` 全被 `ensure_idle`
            //     拦下要 `--force`——而唯一一个专门回答「哪儿不对」的命令一声不吭地退 0。
            //     判 warn 不判 err：一条正常的 `zloop done` 就收得掉，循环也照跑
            //     （下一次 `zloop next` 会重新派活、顺手盖掉这个指针）。
            if crate::todo::is_terminal(&t.status) {
                f.push(Finding::warn(
                    "settled_in_progress",
                    format!("[{who}] 第 {} 轮派出去的 {} 已经是 {} 了，派活指针还挂在它上面", ip.round, ip.todo, t.status),
                    format!(
                        "`zloop done {} --note \"…\" --approach \"…\"` 收尾（状态不会被改回去），或 `zloop edit {} --status open` 放回去接着做{scope}",
                        ip.todo, ip.todo
                    ),
                ));
            }
        }
    }

    // 3. 依赖指向不存在的 todo：这条永远等不到，`next` 会一直跳过它
    for t in &st.todos {
        if crate::todo::is_terminal(&t.status) {
            continue;
        }
        let dead: Vec<&str> =
            t.blocked_by.iter().map(|s| s.as_str()).filter(|d| *d != crate::todo::USER && !ids.contains(d)).collect();
        if !dead.is_empty() {
            f.push(Finding::err(
                "dangling_blocked_by",
                format!("[{who}] {} 依赖 {}，但没有这条 todo——它永远轮不到", t.id, dead.join(" / ")),
                format!("zloop edit {} --blocked-by ''   # 或改成真实存在的 id{scope}", t.id),
            ));
        }
    }

    // 3b. 依赖的 todo 在，但它永远变不成 done：延后的那条不进 `open_ordered`（`is_terminal`
    //     把 deferred 和 done 一视同仁），从此再也派不出去；状态被手改成 zloop 不认的词
    //     （`cancelled`）也一样——`is_executable` 只放行 `status == "open"`。
    //     两种都和 `dangling_blocked_by` 是同一后果，只是依赖那条还在清单里，所以以前没人报。
    for t in &st.todos {
        if crate::todo::is_terminal(&t.status) {
            continue;
        }
        let dead: Vec<String> = t
            .blocked_by
            .iter()
            .filter(|d| d.as_str() != crate::todo::USER)
            // 重复 id 只认第一条，和 `todo::index_of` / `is_executable` 保持一致
            .filter_map(|d| st.todos.iter().find(|x| &x.id == d))
            .filter(|dep| !crate::todo::can_still_finish(&dep.status))
            .map(|dep| {
                if dep.status == "deferred" {
                    format!("{}（已延后）", dep.id)
                } else {
                    format!("{}（状态 {:?}，不是 zloop 认的四种）", dep.id, dep.status)
                }
            })
            .collect();
        if let Some(first) = dead.first() {
            // `t1（已延后）` → `t1`：建议动作里要能直接抄的那个 id
            let dep_id = first.split('（').next().unwrap_or(first).to_string();
            f.push(Finding::err(
                "dead_blocked_by",
                format!(
                    "[{who}] {} 依赖 {}——依赖要 done 才放行，而它已经派不出去了，{} 就永远轮不到",
                    t.id,
                    dead.join(" / "),
                    t.id
                ),
                format!(
                    "把依赖捡回来：zloop edit {dep_id} --status open   # 或断开：zloop edit {} --blocked-by ''{scope}",
                    t.id
                ),
            ));
        }
    }

    // 4. todo id 重复：`done` / `edit` 只会改到第一条
    let mut seen: BTreeMap<&str, usize> = BTreeMap::new();
    for id in &ids {
        *seen.entry(id).or_insert(0) += 1;
    }
    for (id, n) in seen {
        if n > 1 {
            f.push(Finding::err(
                "duplicate_todo_id",
                format!("[{who}] todo id {id} 有 {n} 条"),
                format!("`zloop done {id}` / `zloop edit {id}` 只会改到第一条；手工把 state.json 里重复的那条改个 id"),
            ));
        }
    }

    // 5. next_id 已经被用过：下一次 `zloop plan` 会造出一个重复 id
    let used_max = st.todos.iter().filter_map(|t| t.id.strip_prefix('t')).filter_map(|n| n.parse::<u64>().ok()).max();
    if let Some(max) = used_max {
        if st.next_id <= max {
            f.push(Finding::err(
                "next_id_reuse",
                format!("[{who}] next_id={} 但已经有 t{max}——下一条 plan 会撞上现成的 id", st.next_id),
                format!("把 state.json 的 next_id 改成 {}", max + 1),
            ));
        }
    }
}

/// 依赖成了环：`t1 ← t2 ← t1`。
///
/// `dangling_blocked_by`（依赖指向不存在的 todo）已经在报"这条永远轮不到"的一种；
/// 环是同一后果的另一种，而且是**用产品命令就能走到**的那一种——
/// `zloop edit t1 --blocked-by t2` + `zloop edit t2 --blocked-by t1`，两条都被接受。
/// 此后 `next` 一直返回 `blocked` 并按退避档重试，谁都不会先 done（`is_executable`
/// 要依赖 status == done），循环永远原地打转。修复前 doctor 对这两种环都一声不吭。
///
/// **边只在「依赖还没 done」时才算数**：依赖做完的那条线已经不挡任何人，
/// 把它算进去只会报出一堆解释不清的假环（`t2 ← t1`、t1 已完成，是最常见的正常形状）。
/// 于是环上每个点都必然没做完，剩下的只是它还活着（open/blocked）还是已经了结
/// （deferred/cancelled）——前者是循环**现在**就卡着，报 Error；后者只是埋着，报 Warn。
fn check_dep_cycles(gf: &GoalFile, st: &State, f: &mut Vec<Finding>) {
    // 重复 id 只认第一条，和 `todo::index_of` 保持一致（重复本身由 duplicate_todo_id 报）
    let mut idx: BTreeMap<&str, usize> = BTreeMap::new();
    for (i, t) in st.todos.iter().enumerate() {
        idx.entry(t.id.as_str()).or_insert(i);
    }
    // 三色 DFS，显式栈——todos 是从文件里读来的，链有多长不由我们说了算，不能递归
    const WHITE: u8 = 0;
    const GRAY: u8 = 1;
    const BLACK: u8 = 2;
    let mut color = vec![WHITE; st.todos.len()];
    let mut seen: Vec<std::collections::BTreeSet<&str>> = Vec::new();
    let mut cycles: Vec<Vec<&str>> = Vec::new();
    for start in 0..st.todos.len() {
        if color[start] != WHITE {
            continue;
        }
        color[start] = GRAY;
        // `path` 和 `stack` 同进同退：path[k] 依赖 path[k+1]
        let mut path: Vec<usize> = vec![start];
        let mut stack: Vec<(usize, usize)> = vec![(start, 0)];
        while let Some(top) = stack.last_mut() {
            let (node, cursor) = (top.0, top.1);
            let deps = &st.todos[node].blocked_by;
            if cursor >= deps.len() {
                color[node] = BLACK;
                path.pop();
                stack.pop();
                continue;
            }
            top.1 += 1;
            let dep = deps[cursor].as_str();
            // `user` 不是 todo（它等的是人，不是环）；指不到的 id 归 dangling_blocked_by
            let Some(&j) = idx.get(dep) else { continue };
            if dep == crate::todo::USER || st.todos[j].status == "done" {
                continue;
            }
            match color[j] {
                WHITE => {
                    color[j] = GRAY;
                    path.push(j);
                    stack.push((j, 0));
                }
                GRAY => {
                    // 回边：当前路径上从 j 到栈顶正好是环上的一圈
                    let at = path.iter().position(|&p| p == j).unwrap_or(0);
                    let ring: Vec<&str> = path[at..].iter().map(|&i| st.todos[i].id.as_str()).collect();
                    // 同一圈可以从不同的回边被撞见两次（多条边指回同一个点），按点集去重
                    let key: std::collections::BTreeSet<&str> = ring.iter().copied().collect();
                    if !seen.contains(&key) {
                        seen.push(key);
                        cycles.push(ring);
                    }
                }
                _ => {}
            }
        }
    }
    let who = gf.label();
    for ring in cycles {
        let live = ring.iter().any(|id| st.todos.iter().any(|t| &t.id == id && !crate::todo::is_terminal(&t.status)));
        let chain = format!("{} → {}", ring.join(" → "), ring[0]);
        let what = if ring.len() == 1 {
            format!("[{who}] {} 依赖自己——它永远轮不到（依赖要 done，而 done 得先派出去）", ring[0])
        } else {
            format!("[{who}] 依赖成环：{chain}（→ 读作「依赖」）——环上每条都在等下一条先做完，谁都不会先动")
        };
        let fix = format!(
            "断开环上任意一条：zloop edit {} --blocked-by ''   # 或改成环外真实存在的 id{}",
            ring[0],
            if live { "" } else { "（环上没有还活着的 todo，暂时卡不住谁，但捡回来就会）" }
        );
        f.push(if live { Finding::err("dep_cycle", what, fix) } else { Finding::warn("dep_cycle", what, fix) });
    }
}

/// `policy` 里的数值写出了范围。
///
/// 这个块是**给人改的**（`start` 撞上预算时 zloop 自己就在说「改大 policy.max_total_usd」），
/// 所以隔壁字段被顺手写错只是时间问题。`window_hours` 越界以前是直接 panic——炸掉的正好是
/// 每轮都要走的 `next` / `status` / `context`，而唯一一个专门回答「哪儿不对」的命令
/// 一声不吭地 exit 0（A-7）。现在取值会被钳进合法区间，循环照跑；
/// 但**钳过就等于你写的那个数没生效**，得有人说这一句，这条就是那一句。
fn check_policy(gf: &GoalFile, st: &State, f: &mut Vec<Finding>) {
    let who = gf.label();
    let p = &st.policy;
    let max = crate::tick::WINDOW_HOURS_MAX;
    if p.window_hours < 0 || p.window_hours > max {
        f.push(Finding::err(
            "bad_policy",
            format!(
                "[{who}] policy.window_hours = {}，不在 0..={max} 里——配额窗口按 {} 小时算，你写的那个数没生效",
                p.window_hours,
                p.window_hours.clamp(0, max)
            ),
            format!("把 {}/{} 的 policy.window_hours 改回 24（默认）或别的 0..={max} 的数", STATE_DIR, state::STATE_FILE),
        ));
    }
    if p.max_total_usd < 0.0 {
        f.push(Finding::err(
            "bad_policy",
            format!("[{who}] policy.max_total_usd = {:.2} 是负数——花费只增不减，这个目标一轮都跑不了", p.max_total_usd),
            format!("把 {}/{} 的 policy.max_total_usd 改成 0（不限）或一个正数", STATE_DIR, state::STATE_FILE),
        ));
    }
    if p.intervals_min.is_empty() {
        f.push(Finding::warn(
            "bad_policy",
            format!("[{who}] policy.intervals_min 是空的——间隔退回写死的 3 分钟，退避那一档等于没有"),
            format!("把 {}/{} 的 policy.intervals_min 写成 [3, 10, 30] 这样的递增列表", STATE_DIR, state::STATE_FILE),
        ));
    }
    // 空不空只是这个字段最浅的那种写错法。真正让循环停摆的是**取值**：写大了 runner 睡死
    // （见 `tick::clamp_interval`），写成 0 是每轮 sleep 0 秒的忙等。两边都被钳住了，
    // 所以循环照跑——正因为照跑，没人会来查，得由这条说出"你写的那个数没生效"。
    let imax = crate::tick::INTERVAL_MIN_MAX;
    let bad: Vec<u32> = p.intervals_min.iter().copied().filter(|&m| m == 0 || m > imax).collect();
    if let Some(&first) = bad.first() {
        let list = bad.iter().map(u32::to_string).collect::<Vec<_>>().join(", ");
        f.push(Finding::err(
            "bad_policy",
            format!(
                "[{who}] policy.intervals_min 里有 {} 档不在 1..={imax} 里（{list}）——按 {} 分钟算，你写的那个数没生效",
                bad.len(),
                crate::tick::clamp_interval(first)
            ),
            format!("把 {}/{} 的 policy.intervals_min 改回 [3, 10, 30] 这样的递增列表", STATE_DIR, state::STATE_FILE),
        ));
    }
    // 每一档都合法，不代表这个字段写对了：它是一条**退避阶梯**——第 0 档给正常派活，
    // 之后每积累一次 noop 往后挪一档（`tick::interval`）。写成 [30, 10, 3] 时每个数都在
    // 1..=imax 里，上面那条一声不吭，而三件事同时反过来（实测）：正常派活按**最慢**的
    // 30 分钟等（吞吐掉到 1/10）、blocked/user_gate 的退避是 10 → 3 → 3（越不出活退得越快，
    // 最该慢下来的那一支反而polling 得最凶）、退避耗尽之后 runner 睡的那一档
    // （`tick::ladder_tail`，走到头就是末档）也跟着变成最短的 3 分钟。
    // 三件都不报错、不改值，面板上一切正常——只是循环变笨了。
    //
    // 「阶梯尽头 = 末档」是 runner 那边定死的（T34：照末档，不取最大值），所以阶梯写反的代价
    // 只能由**这一条**说出来——runner 不会替人把顺序纠正过来。
    //
    // 只报**往回走**，不报"没有严格递增"：[10, 10, 10] 是有人存心不要退避，那是合法写法。
    //
    // 比的是**钳过之后**的值，那才是真正生效的阶梯。写了越界值时上面那条 error 已经说过
    // "你写的数没生效"，这里再按原值判一次，只会把同一个根因拆成两条互相矛盾的报告。
    let eff: Vec<u32> = p.intervals_min.iter().copied().map(crate::tick::clamp_interval).collect();
    if let Some(i) = (1..eff.len()).find(|&i| eff[i] < eff[i - 1]) {
        let list = eff.iter().map(u32::to_string).collect::<Vec<_>>().join(", ");
        f.push(Finding::warn(
            "bad_policy",
            format!(
                "[{who}] policy.intervals_min = [{list}] 是往回走的：第 {} 档 {} 分钟比第 {} 档的 {} 分钟还短。\
                 这是退避阶梯，越不出活该等得越久——写反了就是越卡越使劲问，而正常派活按第一档等 {} 分钟",
                i + 1,
                eff[i],
                i,
                eff[i - 1],
                eff[0]
            ),
            format!(
                "把 {}/{} 的 policy.intervals_min 改成不往回走的 [3, 10, 30]（每档持平也行，那是存心不退避）",
                STATE_DIR,
                state::STATE_FILE
            ),
        ));
    }
}

/// 账本里有落在**未来**的时间戳。
///
/// 造出它不需要有人手改文件：NTP 校时、改时区、虚拟机挂起恢复、笔记本电池耗尽后时钟重置，
/// 都会让已经写下的 tick 落在未来。后果不是报错，是**静悄悄地不干活**：
/// `tick::window_ticks` 按「时间戳 ≥ now − window_hours」收配额窗口，未来的 tick 永远满足
/// 这个条件，于是它**永远占着一个配额位**——`max_runs` 一满，窗口就再也滑不开。
/// 等待时间本身已经封顶（A-11），所以最坏是每 `window_hours` 醒一次白跑一趟；
/// 但配额不会自己恢复，得有人来看一眼。这条就是那个"来看一眼"的信号。
fn check_future_timestamps(gf: &GoalFile, st: &State, now: DateTime<FixedOffset>, f: &mut Vec<Finding>) {
    // 几分钟的偏差属于正常的机器间时钟漂移（tick 可能是另一台机器写的），不值得报。
    let cutoff = now + chrono::Duration::minutes(5);
    let future: Vec<&state::Tick> =
        st.ticks.iter().filter(|t| parse_iso(&t.at).map(|ts| ts > cutoff).unwrap_or(false)).collect();
    if future.is_empty() {
        return;
    }
    let who = gf.label();
    // 按解析出来的时刻取最远的一条，不按字符串比——两条 tick 的时区偏移可以不一样
    let farthest = future
        .iter()
        .filter_map(|t| parse_iso(&t.at).ok().map(|ts| (ts, t.at.as_str())))
        .max_by_key(|(ts, _)| *ts)
        .map(|(_, s)| s)
        .unwrap_or("");
    let counted = future.iter().filter(|t| crate::tick::COUNTED.contains(&t.outcome.as_str())).count();
    // 光这几条就把配额窗口填满了 = 循环已经永久限流，不是"迟早咬人"
    let jammed = st.policy.max_runs > 0 && counted >= st.policy.max_runs;
    let what = format!("[{who}] {} 条 tick 的时间戳在未来（最远 {farthest}）——机器时钟跳过一次就会这样", future.len());
    let fix = format!(
        "先校准系统时钟；再把 {}/{} 里那几条 tick 的 at 改成真实时间（或删掉它们）。\
         未来的 tick 永远落在配额窗口里、永远占着一个位子：{}",
        STATE_DIR,
        state::STATE_FILE,
        if jammed {
            format!("现在光它们就占满了 policy.max_runs（{}），循环已经限流住了", st.policy.max_runs)
        } else {
            format!("其中 {counted} 条计入配额（policy.max_runs = {}）", st.policy.max_runs)
        }
    );
    f.push(if jammed { Finding::err("future_timestamp", what, fix) } else { Finding::warn("future_timestamp", what, fix) });
}

/// pid 文件指着一个已经不在的进程。`status` / `goal switch` 会顺手清掉它，
/// 但在那之前，任何读到它的人都以为 runner 还活着。
/// 上次写入被打断留下的半截临时文件。
///
/// 账本的写法是"写 `<名字>.tmp` → `sync_all` → `rename`"，所以进程被杀不会损坏正本
/// （实测 386 次 SIGKILL 一次没坏）——但那个 `.tmp` 会**永远留着**，没人清。
/// 它不影响正确性（下一次 `save` 用 `File::create` 覆盖它），可 `.zloop/` 里躺着一个
/// 半截 JSON，人翻进去只会疑心账本坏了。doctor 的活就是说这一句。
fn check_leftovers(root: &Path, f: &mut Vec<Finding>) {
    let dirs = [root.join(state::STATE_DIR), crate::goals::goals_dir(root)];
    let mut hits: Vec<String> = Vec::new();
    for dir in dirs.iter() {
        let Ok(entries) = fs::read_dir(dir) else { continue };
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            // `<x>.tmp` 和 `<x>.tmp.<pid>`（持有者记录用后者）都算
            if name.contains(".tmp") && e.path().is_file() {
                hits.push(name);
            }
        }
    }
    if hits.is_empty() {
        return;
    }
    hits.sort();
    let n = hits.len();
    f.push(Finding::warn(
        "leftover_tmp",
        format!("有 {n} 个上次写入没写完就被打断的临时文件：{}", hits.join(" / ")),
        "账本正本没事（写法是 tmp → rename，正本要么是旧的要么是新的）。这些残留可以直接删".into(),
    ));
}

/// NOTES.md 在，但读不出来（非 UTF-8、权限、IO 错）。
///
/// 写路径已经会当场拒绝（`remember --rule` / `reflect --apply` 走 `notes::try_read`，
/// 见 A-4），坏在明处。**纯读路径不是**：`zloop context` 用宽容版 `notes::read`，
/// 读失败就当成"什么都没记过"——交接包里的「约定」「经验」两整节一声不吭地消失，
/// 命令照样 exit 0。下一轮的 agent 于是在没有任何项目护栏的情况下开工，而且不自知。
/// 这一条就是替那条静默的读路径把话说出来。
fn check_notes(root: &Path, f: &mut Vec<Finding>) {
    let Some(e) = crate::notes::read_error(root) else { return };
    let p = crate::notes::path(root);
    f.push(Finding::err(
        "unreadable_notes",
        format!("{} 读不出来（{e}）：`zloop context` 会静默少掉「约定」和「经验」两整节", rel(root, &p)),
        format!(
            "交接包的护栏就这么没了，命令还 exit 0——先看一眼文件（多半是被写进了非 UTF-8 字节），\
             修好它。在那之前 `zloop remember --rule` / `zloop reflect --apply` 会拒绝写入（原件不会被盖掉），\
             而 `zloop remember` 照旧往末尾追加——追进去的那几条同样没人读得到。\
             实在救不回来：`mv {p} {p}.bad`，从头再记",
            p = rel(root, &p)
        ),
    ));
}

fn check_pid(root: &Path, f: &mut Vec<Finding>) {
    let p = crate::daemon::pid_path(root);
    let Ok(raw) = fs::read_to_string(&p) else { return };
    let Ok(pid) = raw.trim().parse::<i32>() else {
        f.push(Finding::warn(
            "bad_pid_file",
            format!("{} 里不是一个 pid：{:?}", rel(root, &p), raw.trim()),
            "删掉这个文件；`zloop start` 会重新写".into(),
        ));
        return;
    };
    if !crate::daemon::pid_alive(pid) {
        f.push(Finding::warn(
            "stale_pid",
            format!("{} 指着 pid {pid}，这个进程已经不在了", rel(root, &p)),
            "`zloop status` 或 `zloop stop` 会顺手清掉它（doctor 只读，不动文件）".into(),
        ));
    }
}
