//! Host installers: the /zloop skill for Claude Code and Codex, plus an optional Stop hook.
//!
//! 我们写出去的文件都带 MANAGED_PREFIX 标记，没有这个标记的文件一律不碰。
//!
//! **为什么标记里还带一个指纹**：skill 是给人改的——Warp 那边 skill 就是改进的载体，
//! 走 PR 审核合进去，下一轮 agent 就继承（见 `docs/SELF-IMPROVEMENT.md`）。zloop 以前
//! 只认"有没有标记"，于是自己装的每一份在下次 `install` 时都被无条件重写，用户的改动
//! 静默消失、输出只有一行 `wrote`。现在分两层：
//!
//! - `<!-- zloop:user -->` **之后**的内容是用户的地盘，install 原样搬过去；
//! - 标记之前的托管区带指纹，被手改过就**停下报错**，让用户把改动搬到用户区，或显式 `--force`。
//!
//! 参照 Warp 的 `skills-lock.json` 安装器：目标不明确就失败、两处都有就报错、
//! 可能静默覆盖就阻止、装完要校验。这里做的是同一件事的最小版本。

use anyhow::{bail, Result};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

/// 标记行的前缀；实际写出去的形如 `<!-- zloop-managed:v1 fp=1a2b3c4d5e6f7089 -->`。
pub const MANAGED_PREFIX: &str = "<!-- zloop-managed:v1";
/// 这一行之后归用户，`install` 永不改动。
pub const USER_MARK: &str = "<!-- zloop:user -->";
/// YAML 里的同一套（注释形式）。
pub const YAML_MANAGED_PREFIX: &str = "# zloop-managed:v1";
pub const HOOK_COMMAND: &str = "zloop hook-stop";

const USER_BLOCK: &str = r#"
<!-- zloop:user -->
<!-- 这行以下归你：`zloop install` 不会动它。 -->
<!-- 这份 skill 是全局的，所以只写跨项目都成立的话；某个仓库特有的规矩（"done 之前跑 cargo test"）该写进那个项目的 .zloop/NOTES.md 约定。 -->
"#;

/// 内容指纹：只回答"这段文字还是我写下的那段吗"，不做安全用途。
///
/// 自己实现 FNV-1a 是为了**跨 Rust 版本和平台都不变**——`DefaultHasher` 的取值没有这个保证，
/// 一旦 rustc 升级换了种子，每次 install 都会误判成"用户改过"。
fn fingerprint(s: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// 托管区的可比较形式：去掉标记行（否则指纹会算进自己），并抹平尾部空白。
///
/// 抹尾部是必须的：读回来的托管区切到 `USER_MARK` 为止，末尾会多带一个换行，
/// 而生成时没有——不归一化的话每次 install 都误判成"用户改过"（实测踩过）。
fn canonical(text: &str, prefix: &str) -> String {
    text.lines()
        .filter(|l| !l.trim_start().starts_with(prefix))
        .collect::<Vec<_>>()
        .join("\n")
        .trim_end()
        .to_string()
}

/// 一段托管文字的指纹。**写入侧和读取侧必须都走这个函数**，否则算出来的值不可比。
fn fp_of(text: &str, prefix: &str) -> String {
    fingerprint(&canonical(text, prefix))
}

/// 从标记行里读回上次写入时记下的指纹；老版本装的文件没有这一段，返回 None。
fn recorded_fp(text: &str, prefix: &str) -> Option<String> {
    let line = text.lines().find(|l| l.trim_start().starts_with(prefix))?;
    let rest = line.split("fp=").nth(1)?;
    let fp: String = rest.chars().take_while(|c| c.is_ascii_hexdigit()).collect();
    (fp.len() == 16).then_some(fp)
}

/// 一次安装的结果。`kept_user` 是保留下来的用户区字节数，`migrated` 表示这次刚给老文件加上保护。
#[derive(Debug, Clone)]
pub struct Written {
    pub path: PathBuf,
    pub changed: bool,
    pub kept_user: usize,
    pub migrated: bool,
}

const FRONTMATTER: &str = r#"---
name: "zloop"
description: "围着一个目标持续干活：有参数时初始化目标并规划 todo，无参数时执行一轮（context → next → 做 → done）。"
argument-hint: "[goal text]"
---
"#;

const BODY: &str = r#"
# zloop /zloop

先在项目根目录运行 `zloop context`。报错 "no zloop state" 说明尚未初始化。

## 参数是子命令名（`status` / `context` / `sessions` / `log` / `next` / `goal` / `goals`）

直接运行 `zloop <参数>` 并把输出讲给用户，不要 init、不要规划。

## 参数是目标文本（其余非空参数）

1. 定目标：
   - 未初始化 → `zloop init "$ARGUMENTS"`
   - 当前目标**存在但一条 todo 都没有**（`zloop status` 显示 `◦ 待规划` / "还没有待办"）→ **不要 `goal new`**，直接进第 2 步给它规划；只有新输入明显是另一件事时才 `goal new`。
     `zloop context` / `next --json` 对这种目标报的 reason 是 `unplanned`（"还没有待办：先 zloop plan"），只有 `all_done` 才是真做完了——两个词别混着读；实在拿不准就看 `zloop status`，**0 轮 + 0 条待办 = 没规划过**。
   - 当前目标**已完成**（跑过轮次、todo 全部 done），或新输入明显是**另一件事** → `zloop goal new "$ARGUMENTS"`：当前目标原地停放，`zloop goal switch <id>` 随时切回，什么都不丢。**别把新活挂到旧目标名下**，那会让目标文字和实际做的事对不上。
   - 当前目标还有没做完的 todo，而新输入像是它的延伸 → 先告知当前目标 + 剩余步骤，问用户"接着做"还是"开新目标"；说开新的就 `zloop goal new`。
   - 不要用 `zloop init --force`：它归档旧目标且切不回来。
2. 把目标拆成 2–5 条**可验证**的 todo，每行 `[P0]`/`[P1]`/`[P2]` + 文本，按执行顺序通过 stdin 交给 `zloop plan`。
   验收标准写进 ` :: ` 后面那半——它决定了这条能不能自己判完成，**这是无头跑不用回来问人的前提**。
3. **默认无头**：`zloop start`，然后只汇报「跑起来了 + 怎么看进度」。
   交互轮次（见下）只在**人主动坐在这儿**、或者只有一两步就完的时候才用。
   为什么是默认：交互轮次靠 Stop hook 推，而它只在模型结束一次发言时触发——**人一走开就零轮次**。
   开跑前把这次的规矩钉进约定层（`zloop remember --rule "…"`，每轮自动注入），典型的是
   「只本地 commit 不 push」「改动必须测试全过才写回」「一轮只做一条」。
4. 最后告诉用户：`zloop status` 看进度、`tail -f .zloop/runner/console.log` 实时看、`zloop stop` 叫停。
   {resume_hint}

**别用问句结束一轮。**「要我推吗 / 继续？」这类话每问一次就是一个停车位。有明确下一步就直接做；
真要人拍板的（push、关 issue、改 todo 清单）用 `zloop done --block "<问题>"` 记进账本再停。

## 无参数 = 执行一轮

运行 `zloop heartbeat --host {host}`，严格按它打印的 5 条协议做：`zloop context` → `zloop next --json` → 只做那一条 todo → `zloop done <id> …`（把关键证据放进 `--evidence`）→ 两三句话汇报。
`should_run=false` 时按 reason 简短说明后停止，不要找别的事做。
"#;

const OPENAI_YAML: &str = r#"interface:
  display_name: "zloop"
  short_description: "围着一个目标持续干活的最小调度器"
  default_prompt: "执行一轮 zloop：context → next → 做 → done"
policy:
  allow_implicit_invocation: false
"#;

fn resume_hint(host: &str) -> &'static str {
    match host {
        "claude" => "输入 `/loop /zloop` 可让它按 interval_min 自动续跑；或在终端运行 `zloop run --host claude` 无头续跑。",
        _ => "可用 automation_update 建 automation（body = `zloop heartbeat --host codex-app` 的输出，初始 3 分钟）自动续跑；或在终端运行 `zloop run --host codex`。",
    }
}

/// 完整的 SKILL.md：托管区（带指纹标记）+ 用户区。
pub fn skill_markdown(host: &str) -> String {
    skill_with_user(host, USER_BLOCK)
}

fn skill_with_user(host: &str, user: &str) -> String {
    let body = BODY.replace("{host}", host).replace("{resume_hint}", resume_hint(host));
    // 指纹算在"托管区在文件里的样子"上；标记行会被 canonical 剔掉，所以里面先填什么都无所谓
    let fp = fp_of(&format!("{FRONTMATTER}\n{MANAGED_PREFIX} fp=0 -->{body}"), MANAGED_PREFIX);
    format!("{FRONTMATTER}\n{MANAGED_PREFIX} fp={fp} -->{body}{user}")
}

/// 写 SKILL.md：保留用户区，托管区被手改过就拒绝（除非 `force`）。
fn write_skill(path: &Path, host: &str, force: bool) -> Result<Written> {
    let mut kept_user = 0;
    let mut migrated = false;
    let mut user = USER_BLOCK.to_string();

    if path.exists() {
        let current = fs::read_to_string(path)?;
        if !current.contains(MANAGED_PREFIX) {
            bail!("{} exists and is not managed by zloop; remove it first", path.display());
        }
        // 用户区原样搬过去；托管区拿指纹对一遍
        let (managed_now, user_now) = match current.find(USER_MARK) {
            // 标记行前面那个换行也算用户区：不这样切，"什么都没改"的重装会差一个字节，
            // 于是每次都报 wrote，看着像它动了文件
            Some(i) => {
                let cut = current[..i].strip_suffix('\n').map(str::len).unwrap_or(i);
                (&current[..cut], Some(current[cut..].to_string()))
            }
            None => (current.as_str(), None),
        };
        match recorded_fp(managed_now, MANAGED_PREFIX) {
            Some(rec) => {
                let actual = fp_of(managed_now, MANAGED_PREFIX);
                if rec != actual && !force {
                    bail!(
                        "{} 的托管区被改过（指纹 {rec} → {actual}），install 不会悄悄盖掉它。\n\
                         把你的改动移到 `{USER_MARK}` 之后（那一段永远保留），或者加 --force 用模板覆盖。",
                        path.display()
                    );
                }
            }
            // 老版本装的文件没记指纹，分不清是模板升级还是手改；这次照旧覆盖，并把保护加上
            None => migrated = true,
        }
        if let Some(u) = user_now {
            kept_user = u.len();
            user = u;
        }
    }

    let content = skill_with_user(host, &user);
    let changed = !path.exists() || fs::read_to_string(path)? != content;
    if changed {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, &content)?;
    }
    Ok(Written { path: path.to_path_buf(), changed, kept_user, migrated })
}

/// 写 agents/openai.yaml：同一套指纹保护，但没有用户区（它是配置不是提示词）。
fn write_yaml(path: &Path, force: bool) -> Result<Written> {
    let fp = fp_of(&format!("{YAML_MANAGED_PREFIX} fp=0\n{OPENAI_YAML}"), YAML_MANAGED_PREFIX);
    let content = format!("{YAML_MANAGED_PREFIX} fp={fp}\n{OPENAI_YAML}");
    let mut migrated = false;
    if path.exists() {
        let current = fs::read_to_string(path)?;
        match recorded_fp(&current, YAML_MANAGED_PREFIX) {
            Some(rec) => {
                let actual = fp_of(&current, YAML_MANAGED_PREFIX);
                if rec != actual && !force {
                    bail!(
                        "{} 被改过（指纹 {rec} → {actual}），install 不会悄悄盖掉它。\n\
                         想用模板覆盖就加 --force，或者先把它挪走。",
                        path.display()
                    );
                }
            }
            None => migrated = true,
        }
        if current == content {
            return Ok(Written { path: path.to_path_buf(), changed: false, kept_user: 0, migrated });
        }
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, &content)?;
    Ok(Written { path: path.to_path_buf(), changed: true, kept_user: 0, migrated })
}

pub fn install_claude(home: &Path, force: bool) -> Result<Vec<Written>> {
    let path = home.join(".claude").join("skills").join("zloop").join("SKILL.md");
    Ok(vec![write_skill(&path, "claude", force)?])
}

pub fn install_codex(home: &Path, force: bool) -> Result<Vec<Written>> {
    let base = home.join(".codex").join("skills").join("zloop");
    Ok(vec![
        write_skill(&base.join("SKILL.md"), "codex-app", force)?,
        write_yaml(&base.join("agents").join("openai.yaml"), force)?,
    ])
}

/// Append a Stop hook running `zloop hook-stop` to ~/.claude/settings.json (idempotent).
pub fn install_claude_stop_hook(home: &Path) -> Result<Vec<(PathBuf, bool)>> {
    let settings_path = home.join(".claude").join("settings.json");
    let mut settings: Value = if settings_path.exists() {
        let raw = fs::read_to_string(&settings_path)?;
        if raw.trim().is_empty() { json!({}) } else { serde_json::from_str(&raw)? }
    } else {
        json!({})
    };
    let hooks = settings
        .as_object_mut()
        .expect("settings object")
        .entry("hooks")
        .or_insert_with(|| json!({}));
    let stop = hooks
        .as_object_mut()
        .expect("hooks object")
        .entry("Stop")
        .or_insert_with(|| json!([]));
    let arr = stop.as_array_mut().expect("Stop array");
    let present = arr.iter().any(|entry| {
        entry
            .get("hooks")
            .and_then(Value::as_array)
            .map(|hs| hs.iter().any(|h| h.get("command").and_then(Value::as_str) == Some(HOOK_COMMAND)))
            .unwrap_or(false)
    });
    if present {
        return Ok(vec![(settings_path, false)]);
    }
    arr.push(json!({"hooks": [{"type": "command", "command": HOOK_COMMAND}]}));
    if let Some(p) = settings_path.parent() {
        fs::create_dir_all(p)?;
    }
    fs::write(&settings_path, serde_json::to_string_pretty(&settings)? + "\n")?;
    Ok(vec![(settings_path, true)])
}

pub fn install(claude: bool, codex: bool, stop_hook: bool, home: &Path, force: bool) -> Result<Vec<Written>> {
    let mut out = Vec::new();
    if claude {
        out.extend(install_claude(home, force)?);
    }
    if codex {
        out.extend(install_codex(home, force)?);
    }
    if stop_hook {
        out.extend(install_claude_stop_hook(home)?.into_iter().map(|(path, changed)| Written {
            path,
            changed,
            kept_user: 0,
            migrated: false,
        }));
    }
    Ok(out)
}
