//! Command line: init · plan · next · done · edit · status · heartbeat · install
//!               · sessions · context · log · run   (+ hook-stop for Claude Code)

use crate::session::{self, Host};
use crate::state::{self, StateError};
use crate::{context, daemon, hosts, log, notify, phase, prompt, runner, style, tick, todo};
use anyhow::{bail, Result};
use clap::{Parser, Subcommand};
use std::io::{IsTerminal, Read};
use std::path::{Path, PathBuf};

#[derive(Parser, Debug)]
#[command(name = "zloop", version, about = "Minimal goal loop: one JSON file, a dozen commands.")]
pub struct Cli {
    /// Project directory (default: nearest .zloop upward from cwd)
    #[arg(long, global = true)]
    pub dir: Option<PathBuf>,
    /// Never colourise output (also honoured: NO_COLOR, and any non-tty stdout)
    #[arg(long = "no-color", global = true)]
    pub no_color: bool,
    #[command(subcommand)]
    pub cmd: Cmd,
}

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// Create .zloop/state.json for a goal
    Init {
        #[arg(allow_hyphen_values = true)]
        goal: String,
        /// Replace an existing state file
        #[arg(long)]
        force: bool,
    },
    /// Append ordered todos ([P0]/[P1]/[P2] text per line)
    Plan {
        /// One todo line; repeatable
        #[arg(long, value_name = "LINE", allow_hyphen_values = true)]
        add: Vec<String>,
        /// Read todo lines from a file
        #[arg(long)]
        file: Option<PathBuf>,
        /// Drop open todos before adding
        #[arg(long)]
        replace: bool,
        /// Import open checkbox todos from a loopx ACTIVE_GOAL_STATE.md
        #[arg(long = "from-loopx", value_name = "ACTIVE_GOAL_STATE.md")]
        from_loopx: Option<PathBuf>,
    },
    /// Should we run now, and on which todo?
    Next {
        #[arg(long)]
        json: bool,
        /// Do not record a noop tick when idle
        #[arg(long)]
        peek: bool,
    },
    /// The only write-back: record outcome, move the todo, write the log
    Done {
        id: String,
        /// One-line result
        #[arg(long, allow_hyphen_values = true)]
        note: Option<String>,
        #[arg(long, default_value = "done", value_parser = ["done", "progress", "fail"])]
        outcome: String,
        /// Mark the todo blocked on the user
        #[arg(long, value_name = "QUESTION", allow_hyphen_values = true)]
        block: Option<String>,
        /// Insert a successor todo right after this one
        #[arg(long, value_name = "LINE", allow_hyphen_values = true)]
        next: Option<String>,
        /// Details for the log file: literal text or @path
        #[arg(long, value_name = "TEXT|@FILE", allow_hyphen_values = true)]
        evidence: Option<String>,
        /// 实现思路：怎么做的、为什么这么做（literal text or @path）。outcome=done 时必填
        #[arg(long, value_name = "TEXT|@FILE", allow_hyphen_values = true)]
        approach: Option<String>,
        /// 关键决策 / 取舍，可重复
        #[arg(long, value_name = "TEXT", allow_hyphen_values = true)]
        decision: Vec<String>,
        /// 遇到的坑与结论，可重复
        #[arg(long, value_name = "TEXT", allow_hyphen_values = true)]
        pitfall: Vec<String>,
        /// 这一轮的结论**动摇了后续计划**：写清哪条前提不成立了（会触发一次重估）
        #[arg(long, value_name = "TEXT", allow_hyphen_values = true)]
        rethink: Option<String>,
        /// 这一轮不写技术文档（绕过 policy.require_doc）
        #[arg(long = "no-doc")]
        no_doc: bool,
        /// 派活来自别的目标时也照记到当前目标
        #[arg(long)]
        force: bool,
    },
    /// Change a todo's text, status, priority or dependencies
    Edit {
        id: String,
        #[arg(long, allow_hyphen_values = true)]
        text: Option<String>,
        #[arg(long, value_parser = todo::STATUSES)]
        status: Option<String>,
        #[arg(long, value_parser = clap::value_parser!(u8).range(0..=4))]
        priority: Option<u8>,
        /// Comma-separated todo ids or 'user'; '' clears
        #[arg(long = "blocked-by", value_name = "IDS")]
        blocked_by: Option<String>,
        /// How to verify the todo is done; '' clears
        #[arg(long, allow_hyphen_values = true)]
        acceptance: Option<String>,
    },
    /// Send a notification through policy.notify_url / notify_cmd (use it to test your webhook)
    Notify {
        /// Message text (default: a test message)
        #[arg(allow_hyphen_values = true)]
        text: Option<String>,
    },
    /// 人对某一轮的回应：`zloop feedback t3 "方向不对，别走这条路"`（下一轮的 context 会带上）
    Feedback {
        /// 反馈针对哪条 todo
        id: String,
        /// 你要说的话
        #[arg(allow_hyphen_values = true)]
        text: String,
    },
    /// Write a lesson to .zloop/NOTES.md; the newest few appear in `zloop context`
    Remember {
        #[arg(allow_hyphen_values = true)]
        text: String,
        /// 钉成**约定**而不是经验：每轮都带给模型、不会被最新几条挤掉
        #[arg(long)]
        rule: bool,
    },
    /// Pause the goal: `next` stops, the runner exits at the next check
    Pause,
    /// Resume a paused goal
    Resume,
    /// Move old done/deferred todos and their ticks into .zloop/archive/ to keep state.json small
    Compact {
        /// Keep todos finished within this many days
        #[arg(long = "keep-days", default_value_t = 7)]
        keep_days: i64,
    },
    /// 重估一次：对着最终目标看剩下的任务还对不对，提最小改动（改不改由你点头）
    Replan {
        /// 从 stdin 读新的待办清单（一行一条，`[P0] 文本 :: 验收`）落地；
        /// 已完成的和等你回话的原样保留，旧账本自动备份
        #[arg(long)]
        apply: bool,
        /// 为什么要这么改（`--apply` 时必填，会记进账本）
        #[arg(long, value_name = "TEXT", allow_hyphen_values = true)]
        why: Option<String>,
    },
    /// 回看一次：把账本 + 经验 + 用户反馈摆齐，让模型给出整理建议（`--apply` 从 stdin 落地）
    Reflect {
        /// 从 stdin 读整理后的经验清单（一行一条）重写 .zloop/NOTES.md；旧文件自动备份
        #[arg(long)]
        apply: bool,
        /// 约定超过几条就在体检里提一句（约定每轮全量进交接包，不轮换）
        #[arg(long = "max-rules", default_value_t = crate::notes::RULE_LIMIT)]
        max_rules: usize,
    },
    /// 这个目标跑得顺不顺：轮次、返工率、一次过、花费（`--json` 给脚本）
    Stats {
        #[arg(long)]
        json: bool,
    },
    /// Read-only overview
    Status {
        /// Dump the whole state
        #[arg(long)]
        json: bool,
        /// Render the Markdown projection
        #[arg(long)]
        md: bool,
    },
    /// Print the per-round protocol for a host
    Heartbeat {
        #[arg(long, default_value = "claude", value_parser = prompt::HOSTS)]
        host: String,
    },
    /// Install the /zloop skill into a host
    Install {
        /// ~/.claude/skills/zloop/SKILL.md
        #[arg(long)]
        claude: bool,
        /// ~/.codex/skills/zloop/ (SKILL.md + agents/openai.yaml)
        #[arg(long)]
        codex: bool,
        /// Add a Stop hook to ~/.claude/settings.json (experimental)
        #[arg(long = "claude-stop-hook")]
        claude_stop_hook: bool,
        /// macOS: write /etc/sudoers.d/zloop-pmset so the runner can disable lid-close sleep (asks for your password once)
        #[arg(long)]
        sudoers: bool,
        /// 托管区被手改过时也照写（会丢掉那些改动；`<!-- zloop:user -->` 之后的内容一律保留）
        #[arg(long)]
        force: bool,
    },
    /// macOS sleep protection: `status` (default) or `reconcile` (restore the default when no runner is alive)
    Awake {
        #[arg(default_value = "status", value_parser = ["status", "reconcile"])]
        action: String,
    },
    /// Host sessions seen so far, with resume commands
    Sessions {
        #[arg(long, value_parser = ["claude", "codex", "cli"])]
        host: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Bounded handoff packet for another host or a fresh session
    Context {
        #[arg(long, default_value_t = context::DEFAULT_BUDGET)]
        budget: usize,
        /// Tailor the "how to continue" line
        #[arg(long = "for", value_parser = ["claude", "codex", "cli"])]
        for_host: Option<String>,
    },
    /// Assemble the technical document: one todo's rounds, or `--all` for the whole goal
    Doc {
        /// Todo id (omit with --all)
        todo: Option<String>,
        /// Every todo in the goal
        #[arg(long)]
        all: bool,
        /// Only the most recent N rounds
        #[arg(long, value_name = "N")]
        last: Option<usize>,
        /// Only rounds at or after this time: 2h / 30m / 7d, 2026-08-29, or an ISO timestamp
        #[arg(long, value_name = "TIME")]
        since: Option<String>,
        /// Only rounds at or before this time (same formats as --since)
        #[arg(long, value_name = "TIME")]
        until: Option<String>,
        /// Write to a file instead of stdout
        #[arg(long, value_name = "FILE")]
        out: Option<PathBuf>,
    },
    /// List or show execution logs
    Log {
        #[arg(long)]
        todo: Option<String>,
        #[arg(long, default_value_t = 20)]
        last: usize,
        /// Print one log file (path or bare file name)
        #[arg(long, value_name = "FILE")]
        show: Option<String>,
    },
    /// 只读体检：.zloop 里有没有对不上的地方（目标清单 / 账本 / 日志 / pid），逐条给建议动作
    Doctor {
        #[arg(long)]
        json: bool,
    },
    /// 多目标：列出 / 新建 / 切换 / 归档（当前目标在 .zloop/state.json，其余停在 .zloop/goals/）
    #[command(visible_alias = "goals")]
    Goal {
        #[command(subcommand)]
        cmd: Option<GoalCmd>,
    },
    /// Start the runner in the background (detached; log in .zloop/runner/console.log)
    Start(RunArgs),
    /// Stop the background runner
    Stop,
    /// Run the runner in the foreground: drive claude -p / codex exec round after round
    Run(RunArgs),
    /// (internal) Claude Code Stop-hook entry; reads hook JSON on stdin
    HookStop,
}

#[derive(Subcommand, Debug)]
pub enum GoalCmd {
    /// 列出这个项目的全部目标（不带子命令时的默认动作）
    List {
        #[arg(long)]
        json: bool,
    },
    /// 新目标：把当前目标停走，开一个新的（旧的还能切回来）
    New {
        #[arg(allow_hyphen_values = true)]
        text: String,
        /// 自己指定 id（默认从目标文字里的英文词取，取不到就 g1/g2/…）
        #[arg(long)]
        id: Option<String>,
        /// runner 在跑 / 有轮次没写回时也照做
        #[arg(long)]
        force: bool,
    },
    /// 切到另一个目标：id、id 前缀，或目标文字里的片段
    Switch {
        needle: String,
        #[arg(long)]
        force: bool,
    },
    /// 归档一个停着的目标：搬到 .zloop/archive/，不删文件
    #[command(visible_alias = "archive")]
    Rm {
        needle: String,
        /// 别问了直接归档（不加时，只有精确 id 才免问）
        #[arg(long, short = 'y')]
        yes: bool,
    },
}

#[derive(clap::Args, Debug, Clone)]
pub struct RunArgs {
    /// Which host executes each round
    #[arg(long, default_value = "claude", value_parser = ["claude", "codex"])]
    pub host: String,
    /// Stop after this many rounds (0 = until the scheduler stops)
    #[arg(long = "max-rounds", default_value_t = 0)]
    pub max_rounds: u32,
    /// Treat interval minutes as seconds (for demos)
    #[arg(long)]
    pub fast: bool,
    /// Bypass host permission prompts (claude --dangerously-skip-permissions / codex danger-full-access)
    #[arg(long = "allow-all")]
    pub allow_all: bool,
    /// Session reuse: todo = resume only for the same todo (default), all = always resume, none = fresh each round
    #[arg(long, default_value = "todo", value_parser = ["todo", "all", "none"])]
    pub resume: String,
    /// Kill the host after this many minutes per round (seconds with --fast)
    #[arg(long = "timeout-min", default_value_t = 30)]
    pub timeout_min: u32,
    /// Exit when waiting on a human instead of polling at the slowest interval
    #[arg(long = "exit-on-wait")]
    pub exit_on_wait: bool,
    /// Per-round spend cap passed to `claude -p --max-budget-usd`
    #[arg(long = "max-budget-usd")]
    pub max_budget_usd: Option<String>,
    /// After a round that wrote back, `git add -A ':!.zloop' && git commit` (skipped when not a repo or clean)
    #[arg(long = "git-commit")]
    pub git_commit: bool,
    /// Do not touch sleep settings (default: caffeinate + lid-close protection while the runner lives)
    #[arg(long = "no-keep-awake")]
    pub no_keep_awake: bool,
    /// 每 N 个 todo 轮次插一轮「回看」（读账本 + 经验 + 反馈给建议，不做 todo；0 = 关）
    #[arg(long = "reflect-every", default_value_t = 0)]
    pub reflect_every: u32,
    /// 关掉「写回之后按信号重估计划」（默认开；命中信号才跑，只产出建议不改 todo）
    #[arg(long = "no-replan")]
    pub no_replan: bool,
    /// 让重估那一轮**真的改计划**（默认关）。护栏在代码里强制；
    /// 单次运行最多改几次、清单连着变长就停机等人
    #[arg(long = "auto-replan")]
    pub auto_replan: bool,
}

impl RunArgs {
    fn options(&self) -> runner::Options {
        runner::Options {
            host: Host::parse(&self.host).unwrap_or(Host::Claude),
            max_rounds: self.max_rounds,
            fast: self.fast,
            allow_all: self.allow_all,
            resume: runner::ResumeMode::parse(&self.resume).unwrap_or(runner::ResumeMode::Todo),
            timeout_min: self.timeout_min,
            exit_on_wait: self.exit_on_wait,
            max_budget_usd: self.max_budget_usd.clone(),
            git_commit: self.git_commit,
            keep_awake: !self.no_keep_awake,
            reflect_every: self.reflect_every,
            no_replan: self.no_replan,
            auto_replan: self.auto_replan,
        }
    }

    /// Re-serialize for the detached child process.
    fn to_argv(&self) -> Vec<String> {
        let mut v = vec!["--host".into(), self.host.clone(), "--max-rounds".into(), self.max_rounds.to_string(),
                         "--resume".into(), self.resume.clone(), "--timeout-min".into(), self.timeout_min.to_string()];
        if self.fast { v.push("--fast".into()); }
        if self.allow_all { v.push("--allow-all".into()); }
        if self.exit_on_wait { v.push("--exit-on-wait".into()); }
        if self.git_commit { v.push("--git-commit".into()); }
        if self.no_keep_awake { v.push("--no-keep-awake".into()); }
        if let Some(b) = &self.max_budget_usd { v.push("--max-budget-usd".into()); v.push(b.clone()); }
        v
    }
}

fn root_of(dir: &Option<PathBuf>) -> PathBuf {
    state::find_root(dir.as_deref())
}

fn fmt_todo(t: &state::Todo) -> String {
    format!("{} [P{}] {}", t.id, t.priority, t.text)
}

fn print_json(v: &serde_json::Value) {
    println!("{}", serde_json::to_string_pretty(v).unwrap_or_default());
}

/// 进持有者记录的「操作名」：等锁超时的人靠 pid + 这个名字判断被谁挡住了（见 `state::locked`）。
/// 带上 todo id，因为「pid 51234 · done t16」比「pid 51234 · done」更容易对上是哪一轮。
fn cmd_label(cmd: &Cmd) -> String {
    match cmd {
        Cmd::Init { .. } => "init".into(),
        Cmd::Plan { .. } => "plan".into(),
        Cmd::Next { .. } => "next".into(),
        Cmd::Done { id, .. } => format!("done {id}"),
        Cmd::Edit { id, .. } => format!("edit {id}"),
        Cmd::Notify { .. } => "notify".into(),
        Cmd::Feedback { id, .. } => format!("feedback {id}"),
        Cmd::Remember { .. } => "remember".into(),
        Cmd::Pause => "pause".into(),
        Cmd::Resume => "resume".into(),
        Cmd::Compact { .. } => "compact".into(),
        Cmd::Replan { .. } => "replan".into(),
        Cmd::Reflect { .. } => "reflect".into(),
        Cmd::Stats { .. } => "stats".into(),
        Cmd::Status { .. } => "status".into(),
        Cmd::Heartbeat { .. } => "heartbeat".into(),
        Cmd::Install { .. } => "install".into(),
        Cmd::Awake { .. } => "awake".into(),
        Cmd::Sessions { .. } => "sessions".into(),
        Cmd::Context { .. } => "context".into(),
        Cmd::Doc { .. } => "doc".into(),
        Cmd::Log { .. } => "log".into(),
        Cmd::Doctor { .. } => "doctor".into(),
        Cmd::Goal { cmd } => match cmd {
            None | Some(GoalCmd::List { .. }) => "goal list".into(),
            Some(GoalCmd::New { .. }) => "goal new".into(),
            Some(GoalCmd::Switch { .. }) => "goal switch".into(),
            Some(GoalCmd::Rm { .. }) => "goal rm".into(),
        },
        Cmd::Start(_) => "start".into(),
        Cmd::Stop => "stop".into(),
        Cmd::Run(_) => "run".into(),
        Cmd::HookStop => "hook-stop".into(),
    }
}

/// Returns the process exit code.
pub fn run(cli: Cli) -> Result<i32> {
    let root = root_of(&cli.dir);
    let path = state::state_path(&root);
    state::set_operation(cmd_label(&cli.cmd));
    match cli.cmd {
        Cmd::Init { goal, force } => cmd_init(&cli.dir, &goal, force),
        Cmd::Plan { add, file, replace, from_loopx } => cmd_plan(&path, add, file, replace, from_loopx),
        Cmd::Next { json, peek } => cmd_next(&root, &path, json, peek),
        Cmd::Done { id, note, outcome, block, next, evidence, approach, decision, pitfall, rethink, no_doc, force } => {
            cmd_done(&root, &path, &id, note, &outcome, block, next, DoneDoc { evidence, approach, decision, pitfall, rethink, no_doc }, force, style::Style::detect(cli.no_color))
        }
        Cmd::Replan { apply, why } => cmd_replan(&root, &path, apply, why),
        Cmd::Reflect { apply, max_rules } => cmd_reflect(&root, &path, apply, max_rules, style::Style::detect(cli.no_color)),
        Cmd::Stats { json } => cmd_stats(&path, json, style::Style::detect(cli.no_color)),
        Cmd::Doc { todo, all, last, since, until, out } => cmd_doc(&root, &path, todo, all, last, since, until, out),
        Cmd::Edit { id, text, status, priority, blocked_by, acceptance } => {
            cmd_edit(&path, &id, text, status, priority, blocked_by, acceptance)
        }
        Cmd::Feedback { id, text } => cmd_feedback(&path, &id, &text),
        Cmd::Remember { text, rule } => {
            state::load(&path)?;
            if text.trim().is_empty() {
                eprintln!("remember: 要记点什么");
                return Ok(2);
            }
            if rule {
                let (p, already) = crate::notes::add_rule(&root, &text)?;
                let n = crate::notes::read(&root).rules.len();
                if already {
                    println!("这条约定已经在了（共 {n} 条）→ {}", p.display());
                } else {
                    println!("约定 +1（共 {n} 条，每轮都带给模型）→ {}", p.display());
                }
            } else {
                let p = crate::notes::remember(&root, &text)?;
                println!("remembered → {}", p.display());
            }
            Ok(0)
        }
        Cmd::Pause => {
            let status = state::transaction(&path, |st| {
                if st.goal.status == "active" {
                    st.goal.status = "paused".into();
                }
                Ok(st.goal.status.clone())
            })?;
            println!("goal is now {status}");
            Ok(0)
        }
        Cmd::Resume => {
            let status = state::transaction(&path, |st| {
                if st.goal.status == "paused" {
                    st.goal.status = "active".into();
                }
                Ok(st.goal.status.clone())
            })?;
            println!("goal is now {status}");
            Ok(0)
        }
        Cmd::Compact { keep_days } => cmd_compact(&root, &path, keep_days),
        Cmd::Notify { text } => {
            let st = state::load(&path)?;
            if !notify::configured(&st) {
                eprintln!("notify: nothing configured — set policy.notify_url and/or policy.notify_cmd in {}", path.display());
                return Ok(2);
            }
            let text = text.unwrap_or_else(|| "zloop 通知测试：收到这条说明配置正确".into());
            let msg = notify::text_for("test", &st, &root, &text);
            let ok = notify::send(&st, &root, "test", &msg)?;
            println!("{}", if ok { "notification sent" } else { "notification failed (see stderr)" });
            Ok(if ok { 0 } else { 1 })
        }
        Cmd::Status { json, md } => cmd_status(&root, &path, json, md, style::Style::detect(cli.no_color)),
        Cmd::Heartbeat { host } => {
            let st = state::load(&path)?;
            println!("{}", prompt::heartbeat(&st, &host, &root)?);
            Ok(0)
        }
        Cmd::Install { claude, codex, claude_stop_hook, sudoers, force } => {
            if !(claude || codex || claude_stop_hook || sudoers) {
                eprintln!("install: choose --claude, --codex, --claude-stop-hook and/or --sudoers");
                return Ok(2);
            }
            let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("no home directory"))?;
            for w in hosts::install(claude, codex, claude_stop_hook, &home, force)? {
                println!("{}{}", if w.changed { "wrote  " } else { "kept   " }, w.path.display());
                if w.kept_user > 0 {
                    println!("       保留了你的自定义段落（{} 之后 {} 字节）", hosts::USER_MARK, w.kept_user);
                }
                if w.migrated {
                    println!("       这一份是旧版装的，现在加上了改动保护：写在 {} 之后的内容以后不会被覆盖", hosts::USER_MARK);
                }
            }
            if sudoers {
                let p = crate::awake::install_sudoers()?;
                println!("wrote  {}\n{}", p.display(), crate::awake::describe());
            }
            Ok(0)
        }
        Cmd::Awake { action } => {
            if action == "reconcile" {
                let r = crate::awake::reconcile();
                println!(
                    "reconcile: holders={} SleepDisabled was {} → {}",
                    r.holders,
                    r.before.map(|b| if b { "1" } else { "0" }).unwrap_or("?"),
                    match r.changed {
                        Some(true) => "set to 1".to_string(),
                        Some(false) => "restored to 0".to_string(),
                        None if !r.sudo && r.before == Some(true) && r.holders == 0 => "unchanged (no passwordless sudo — run `sudo pmset -a disablesleep 0` by hand)".to_string(),
                        None => "unchanged".to_string(),
                    }
                );
            }
            println!("{}", crate::awake::describe());
            for (pid, root) in crate::awake::live_holders() {
                println!("  holder pid {pid} · {root}");
            }
            Ok(0)
        }
        Cmd::Sessions { host, json } => cmd_sessions(&root, &path, host, json),
        Cmd::Context { budget, for_host } => {
            let st = state::load(&path)?;
            let host = for_host.as_deref().and_then(Host::parse);
            println!("{}", context::build(&st, &root, budget, host, state::now()));
            // 包后面喊，不是前面：4000 字的包会把开头那行顶出屏幕，而读的人（和模型）
            // 停在的是最后一行。护栏丢了这件事得跟"怎么继续"挨着。
            if let Some(w) = context::notes_warning(&root) {
                eprintln!("{w}");
            }
            Ok(0)
        }
        Cmd::Log { todo, last, show } => cmd_log(&root, todo, last, show),
        Cmd::Doctor { json } => cmd_doctor(&root, json, style::Style::detect(cli.no_color)),
        Cmd::Goal { cmd } => cmd_goal(&root, cmd.unwrap_or(GoalCmd::List { json: false }), style::Style::detect(cli.no_color)),
        Cmd::Run(args) => runner::run(&root, args.options()),
        Cmd::Start(args) => {
            let st = state::load(&path)?; // fail early with the usual "no zloop state" message
            // 起来就秒退比不起来更糟：控制台只留一句 reason，`start` 却报告「started」。
            if let Some(reason) = runner::immediate_stop_reason(&st, &args.options(), state::now()) {
                eprintln!("{}", start_refusal(&st, &reason));
                return Ok(1);
            }
            let _ = crate::awake::reconcile(); // clean up anything a previous run left behind
            let pid = daemon::start(&root, &args.to_argv())?;
            println!(
                "runner started in the background (pid {pid}, host {})\nlog:    {}\nwatch:  zloop status\nstop:   zloop stop",
                args.host,
                daemon::log_path(&root).display()
            );
            Ok(0)
        }
        Cmd::Stop => {
            let stopped = daemon::stop(&root)?;
            let r = crate::awake::reconcile();
            match stopped {
                Some(pid) => println!("stopped runner (pid {pid})"),
                None => println!("no runner is running for {}", root.display()),
            }
            if r.changed == Some(false) {
                println!("sleep: restored the default (SleepDisabled=0)");
            }
            Ok(0)
        }
        Cmd::HookStop => cmd_hook_stop(&root, &path),
    }
}

/// `zloop start` 拒绝启动时说的话：一句原因 + 一条能直接敲的下一步。
///
/// `reason` 就是 runner 停下来时打印的那个词（`tick::decide` 给的），这里只负责翻译成人话，
/// 不重新判断一遍——两套判断迟早会漂开。
fn start_refusal(st: &state::State, reason: &str) -> String {
    let p = &st.policy;
    let (why, next) = match reason {
        "unplanned" => (
            "这个目标一条待办都没有".to_string(),
            "zloop plan --add \"[P0] 第一件事\"（或在 Claude Code 里 `/zloop <目标>` 让它规划）".to_string(),
        ),
        "all_done" => (
            format!("{} 条待办全做完了", st.todos.len()),
            "zloop plan --add \"[P0] 下一件事\" 续上，或 zloop goal new \"<新目标>\" 开一个新的".to_string(),
        ),
        // 这条别学 all_done 说"去开新目标"：活一条都没做，只是全被推到了以后。
        "all_deferred" => (
            format!("{} 条待办全被延后了，没有能跑的", st.todos.len()),
            "zloop edit <id> --status open 把要做的那条捡回来，或 zloop plan --add \"[P0] 下一件事\"".to_string(),
        ),
        "paused" => ("当前目标是暂停着的".to_string(), "zloop resume 继续，或 zloop goal switch <id> 换一个".to_string()),
        "done" => ("当前目标已经结束了".to_string(), "zloop goal new \"<新目标>\"".to_string()),
        "fail_streak" => (
            format!("连着 {} 轮失败，到了 policy.max_fail_streak 上限", tick::fail_streak(&st.ticks)),
            "zloop log 看失败原因，zloop edit 改掉那条 todo（或 zloop feedback 留一句），再 start".to_string(),
        ),
        "progress_streak" => (
            "同一条 todo 连着 progress 太多轮，到了 policy.max_progress_streak 上限".to_string(),
            "zloop edit 把它拆小，再 start".to_string(),
        ),
        "budget" => (
            format!("已花 ${:.2}，到了 policy.max_total_usd（${:.2}）上限", tick::spent_usd(&st.ticks), p.max_total_usd),
            format!("改大 {}/{} 里的 policy.max_total_usd，再 start", state::STATE_DIR, state::STATE_FILE),
        ),
        "user_gate" | "blocked" => (
            "能跑的待办一条都没有（都在等人或被挡着），而 --exit-on-wait 让 runner 等不了".to_string(),
            "去掉 --exit-on-wait 就会挂着轮询等你；或者 zloop edit <id> --blocked-by \"\" 解开再 start".to_string(),
        ),
        "throttled" => (
            format!("{} 小时窗口里已经跑满 policy.max_runs（{}）轮", p.window_hours, p.max_runs),
            "等窗口滑过去，或改大 policy.max_runs，再 start".to_string(),
        ),
        other => (format!("调度器说 {other}"), "zloop next 看当前判断".to_string()),
    };
    format!("start: 没启动——runner 起来第一轮就会退出（{reason}）。\n原因：{why}。\n下一步：{next}")
}

fn cmd_init(dir: &Option<PathBuf>, goal: &str, force: bool) -> Result<i32> {
    let root = dir.clone().unwrap_or_else(|| PathBuf::from("."));
    let root = root.canonicalize().unwrap_or(root);
    let path = state::state_path(&root);
    let mut archived: Option<PathBuf> = None;
    if path.exists() {
        if !force {
            let cur = state::load(&path)?;
            eprintln!("already initialized ({}): {}\nuse --force to replace", cur.goal.status, cur.goal.text);
            return Ok(1);
        }
        // Never lose a goal's history: park the old state under .zloop/archive/.
        let stamp = state::load(&path)
            .map(|s| format!("{}-{}", s.goal.created_at.replace([':', '+'], ""), s.goal.id))
            .unwrap_or_else(|_| format!("{}-corrupt", state::now_iso().replace([':', '+'], "")));
        let dir = root.join(state::STATE_DIR).join("archive");
        std::fs::create_dir_all(&dir)?;
        let mut target = dir.join(format!("{stamp}.json"));
        let mut n = 1;
        while target.exists() {
            n += 1;
            target = dir.join(format!("{stamp}-{n}.json"));
        }
        std::fs::rename(&path, &target)?;
        archived = Some(target);
    }
    // id 从目标文字取（多目标下目录名会让每个目标的 id 都一样）
    let id = crate::goals::fresh_id(&root, goal.trim());
    let mut st = state::default_state(goal.trim(), &id);
    state::locked(&path, state::LOCK_WAIT, || state::save(&path, &mut st))?;
    if let Some(a) = archived {
        println!("archived previous state → {}", a.display());
    }
    println!(
        "initialized {}\ngoal: {}\nnext: `zloop plan` with one `[P0] text` line per todo on stdin",
        path.display(),
        goal.trim()
    );
    Ok(0)
}

fn cmd_plan(path: &Path, add: Vec<String>, file: Option<PathBuf>, replace: bool, from_loopx: Option<PathBuf>) -> Result<i32> {
    let items = if let Some(p) = from_loopx {
        todo::parse_loopx_state(&std::fs::read_to_string(&p)?)
    } else {
        let raw = if !add.is_empty() {
            add.join("\n")
        } else if let Some(f) = file {
            std::fs::read_to_string(&f)?
        } else if !std::io::stdin().is_terminal() {
            let mut s = String::new();
            std::io::stdin().read_to_string(&mut s)?;
            s
        } else {
            eprintln!("plan: give todos via --add, --file, --from-loopx, or stdin (one `[P0] text` per line)");
            return Ok(2);
        };
        todo::parse_plan(&raw, todo::DEFAULT_PRIORITY)
    };
    if items.is_empty() {
        eprintln!("plan: no todo lines found");
        return Ok(2);
    }
    let created = state::transaction(path, |st| {
        let created = todo::add(st, &items, replace);
        if st.goal.status == "done" {
            st.goal.status = "active".into();
        }
        Ok(created)
    })?;
    for t in created {
        match &t.acceptance {
            Some(a) => println!("{} :: {a}", fmt_todo(&t)),
            None => println!("{}", fmt_todo(&t)),
        }
    }
    Ok(0)
}

fn cmd_next(root: &Path, path: &Path, json: bool, peek: bool) -> Result<i32> {
    let who = session::detect();
    let (decision, mut payload) = state::transaction(path, |st| {
        let now = state::now();
        // 别人正拿着这一轮：不派活、不记 tick、不动 in_progress——抢占过来只会让两个 agent 撞车
        if !peek {
            if let Some(ip) = tick::held_by_other(st, &who, now) {
                let d = tick::hold_decision(st);
                let mut payload = tick::to_json(&d, st);
                if let Some(obj) = payload.as_object_mut() {
                    obj.insert(
                        "held_by".into(),
                        serde_json::json!({
                            "todo": ip.todo, "round": ip.round, "since": ip.started_at,
                            "host": ip.host, "session": ip.session,
                        }),
                    );
                }
                return Ok((d, payload));
            }
        }
        let d = tick::decide(st, now);
        if !peek {
            if d.should_run {
                // Hand the todo out: from now until `done`, phase reports "executing".
                let t = d.todo.as_ref().unwrap();
                st.in_progress = Some(state::InProgress {
                    todo: t.id.clone(),
                    started_at: state::format_iso(&now),
                    round: tick::current_round(&st.ticks) + 1,
                    via: "next".into(),
                    host: Some(who.host.as_str().to_string()),
                    session: who.session.clone(),
                });
            } else {
                st.in_progress = None;
                tick::record(st, "noop", None, &d.reason, &who)?;
            }
        }
        let payload = tick::to_json(&d, st);
        Ok((d, payload))
    })?;
    let st = state::load(path)?;
    let ph = phase::compute(&st, root, state::now());
    if let Some(obj) = payload.as_object_mut() {
        obj.insert("phase".into(), serde_json::Value::String(ph.summary.clone()));
    }
    if json {
        print_json(&payload);
    } else if decision.should_run {
        let t = decision.todo.as_ref().unwrap();
        println!("RUN  {}", fmt_todo(t));
        if let Some(a) = &t.acceptance {
            println!("     acceptance: {a}");
        }
        println!("     writeback: {}", payload["writeback"].as_str().unwrap_or(""));
        println!("     interval: {} min · remaining {}", payload["interval_min"], payload["remaining"]);
        println!("     phase: {}", ph.summary);
    } else {
        let interval = match decision.interval_min {
            None => "stop".to_string(),
            Some(m) => format!("{m} min"),
        };
        println!("WAIT ({}) remaining {} · retry in {}", decision.reason, payload["remaining"], interval);
        if decision.reason == "held_by_other" {
            let h = &payload["held_by"];
            println!(
                "     {} 已经派给了 {}（第 {} 轮，{} 开始）：等它写回，或 `zloop edit {} --status open` 放回去",
                h["todo"].as_str().unwrap_or("?"),
                h["session"].as_str().unwrap_or("另一个会话"),
                h["round"],
                h["since"].as_str().unwrap_or("?"),
                h["todo"].as_str().unwrap_or("?"),
            );
        }
    }
    Ok(0)
}

/// The documentation half of `zloop done`.
pub struct DoneDoc {
    pub evidence: Option<String>,
    pub approach: Option<String>,
    pub decision: Vec<String>,
    pub pitfall: Vec<String>,
    pub rethink: Option<String>,
    pub no_doc: bool,
}

#[allow(clippy::too_many_arguments)]
fn cmd_done(
    root: &Path,
    path: &Path,
    id: &str,
    note: Option<String>,
    outcome: &str,
    block: Option<String>,
    next: Option<String>,
    input: DoneDoc,
    force: bool,
    c: style::Style,
) -> Result<i32> {
    let who = session::detect();
    let st_now = state::load(path)?;

    // 派活来自另一个（现在停着的）目标：写回会记错目标，先让用户切回去。
    // 和文档检查一样放在任何写入之前，被拒的调用什么都不改。
    if !force {
        if let Some(from) = crate::goals::parked_holder(root, id, &who) {
            eprintln!(
                "done: {id} 是停放中的目标「{}」[{}] 派给这个会话的，当前目标是「{}」。\n\
                 直接写回会把成果记到当前目标头上：先 `zloop goal switch {}` 再写回；\n\
                 确实要记在当前目标：加 --force。",
                style::truncate(&from.text, 30),
                from.id,
                style::truncate(&st_now.goal.text, 30),
                from.id
            );
            return Ok(2);
        }
    }

    // Every finished todo must explain itself. Checked before any state write so a rejected
    // call changes nothing and can simply be retried with the missing text.
    let policy_requires = st_now.policy.require_doc;
    let finishing = outcome == "done" && block.is_none();
    let has_approach = input.approach.as_deref().map(|s| !s.trim().is_empty()).unwrap_or(false);

    // 失败最该留下的是"为什么不行"。连续失败会让循环停下等人，而如果原因没有落点，
    // 下一轮（或下一个会话）会把同一个坑再踩一遍——那正是"停下来"和"学到"的差别。
    let failing = outcome == "fail" && block.is_none();
    let has_pitfall = input.pitfall.iter().any(|p| !p.trim().is_empty());
    if st_now.policy.require_pitfall && failing && !input.no_doc && !has_pitfall {
        eprintln!(
            "done: {id} 这一轮失败了，得留下踩到的坑（policy.require_pitfall），否则下次还会踩。带上 --pitfall 再重试：\n\n\
             \x20 zloop done {id} --outcome fail --note \"<一句话：卡在哪>\" \\\n\
             \x20   --pitfall \"<试了什么、为什么不行、下次该从哪切入>\" \\\n\
             \x20   --evidence \"<报错输出，或 @文件>\"\n\n\
             真没什么可记的：加 --no-doc；想永久关闭：把 .zloop/state.json 的 policy.require_pitfall 设为 false。"
        );
        return Ok(2);
    }
    if policy_requires && finishing && !input.no_doc && !has_approach {
        eprintln!(
            "done: {id} 完成时需要留下技术文档（policy.require_doc）。带上实现思路再重试，例如：\n\n\
             \x20 zloop done {id} --note \"<一句话结果>\" \\\n\
             \x20   --approach \"<怎么做的、为什么这么做>\" \\\n\
             \x20   --decision \"<关键取舍>\" \\\n\
             \x20   --pitfall \"<踩过的坑与结论>\" \\\n\
             \x20   --evidence \"<验证输出，或 @文件>\"\n\n\
             确实不需要文档：加 --no-doc；想永久关闭：把 .zloop/state.json 的 policy.require_doc 设为 false。"
        );
        return Ok(2);
    }

    let doc = log::Doc {
        approach: log::resolve_evidence(input.approach.as_deref())?,
        decisions: input.decision.iter().filter(|d| !d.trim().is_empty()).cloned().collect(),
        pitfalls: input.pitfall.iter().filter(|p| !p.trim().is_empty()).cloned().collect(),
        evidence: log::resolve_evidence(input.evidence.as_deref())?,
        changed_files: log::changed_files(root),
    };
    let evidence = doc.evidence.clone();
    let note = note.unwrap_or_default();
    let result = state::transaction(path, |st| {
        let (mut tick_rec, idx) =
            match tick::apply_done(st, id, outcome, &note, block.as_deref(), next.as_deref(), &who) {
                Ok(v) => v,
                Err(e) => return Ok(Err(e)),
            };
        let todo_snapshot = st.todos[idx].clone();
        let rel = log::write(root, st, &tick_rec, &todo_snapshot, &doc)?;
        // Only a finished todo owes a technical document; progress / fail / block rounds are exempt,
        // so they carry no verdict at all rather than showing up as "undocumented".
        let documented = (tick_rec.outcome == "done").then(|| doc.is_complete());
        let rethink = input.rethink.as_deref().map(str::trim).filter(|r| !r.is_empty()).map(str::to_string);
        if let Some(last) = st.ticks.last_mut() {
            last.log = Some(rel.clone());
            last.documented = documented;
            last.pitfalls = doc.pitfalls.clone();
            last.rethink = rethink.clone();
        }
        tick_rec.log = Some(rel);
        tick_rec.documented = documented;
        tick_rec.pitfalls = doc.pitfalls.clone();
        tick_rec.rethink = rethink;
        st.in_progress = None; // the round is written back; phase goes back to idle/stopped
        let d = tick::decide(st, state::now());
        Ok(Ok((tick_rec, d, todo::remaining(st), todo_snapshot.acceptance.clone())))
    })?;
    let (tick_rec, decision, remaining, acceptance) = match result {
        Ok(v) => v,
        Err(e) => {
            eprintln!("done: {e}");
            return Ok(2);
        }
    };
    let note = if tick_rec.note.is_empty() { String::new() } else { format!(": {}", tick_rec.note) };
    println!("{id} {}{}", tick_rec.outcome, note);
    if let (Some(a), true, None) = (&acceptance, tick_rec.outcome == "done", evidence.as_deref()) {
        println!("hint: {id} 有验收标准但这次 done 没带 --evidence —— 验收：{a}");
    }
    if tick_rec.documented == Some(false) && tick_rec.outcome == "done" {
        println!("hint: 这一轮没有实现思路，日志只是结果记录（下次带 --approach）");
    }
    let following = match &decision.todo {
        Some(t) => fmt_todo(t),
        None => decision.reason.clone(),
    };
    println!("remaining {remaining} · next: {following}");
    if let Some(l) = &tick_rec.log {
        println!("log: .zloop/{l}");
    }
    // 便宜的体检：账本里读得出偏离信号才提一句。**没命中就一声不吭**——
    // 每轮都催着重规划会制造计划抖动，代价比漏提一次大（见 docs/ADAPTIVE-REPLAN.md §2）。
    if remaining > 0 {
        if let Some(why) = crate::replan::hint(&state::load(path)?) {
            println!("\n⚠ 计划可能要调整：{why}\n  想清楚剩下的任务还对不对：{}", c.bold("zloop replan"));
        }
    }
    Ok(0)
}

/// `zloop feedback`：把**人说的话**记进账本。
///
/// 为什么要单独一条命令而不是塞进 `done --note`：`note` / `approach` / `pitfall` 全是 agent 自述，
/// 而"人怎么回应"是另一路信号——没有它就算不出"agent 建议的"和"人接受的"之间的差，
/// 也就没有任何东西可以拿来改进下一轮（Warp 的 improver 读的正是这个差）。
fn cmd_feedback(path: &Path, id: &str, text: &str) -> Result<i32> {
    let text = text.trim();
    if text.is_empty() {
        eprintln!("feedback: 反馈不能是空的");
        return Ok(2);
    }
    let who = session::detect();
    let result = state::transaction(path, |st| {
        let idx = match todo::index_of(st, id) {
            Ok(i) => i,
            Err(e) => return Ok(Err(e)),
        };
        let status = st.todos[idx].status.clone();
        // 不动 todo 状态、不清 in_progress：反馈是信号，不是写回
        tick::record(st, tick::FEEDBACK, Some(id), text, &who)?;
        Ok(Ok((status, tick::pending_feedback(st).len())))
    })?;
    let (status, pending) = match result {
        Ok(v) => v,
        Err(e) => {
            eprintln!("feedback: {e}");
            return Ok(2);
        }
    };
    println!("feedback → {id}：{}", style::truncate(text, 60));
    println!("下一轮的 `zloop context` 会带上{}", if pending > 1 { format!("（共 {pending} 条待处理）") } else { String::new() });
    if todo::is_terminal(&status) {
        println!("（{id} 已经是 {status}；要让它重做：`zloop edit {id} --status open`）");
    }
    Ok(0)
}

fn cmd_edit(
    path: &Path,
    id: &str,
    text: Option<String>,
    status: Option<String>,
    priority: Option<u8>,
    blocked_by: Option<String>,
    acceptance: Option<String>,
) -> Result<i32> {
    if text.is_none() && status.is_none() && priority.is_none() && blocked_by.is_none() && acceptance.is_none() {
        eprintln!("edit: nothing to change (use --text/--status/--priority/--blocked-by/--acceptance)");
        return Ok(2);
    }
    let who = session::detect();
    let result = state::transaction(path, |st| {
        let idx = match todo::index_of(st, id) {
            Ok(i) => i,
            Err(e) => return Ok(Err(e)),
        };
        if let Some(t) = &text {
            st.todos[idx].text = t.trim().to_string();
        }
        if let Some(p) = priority {
            st.todos[idx].priority = p;
        }
        if let Some(a) = &acceptance {
            st.todos[idx].acceptance = if a.trim().is_empty() { None } else { Some(a.trim().to_string()) };
        }
        if let Some(raw) = &blocked_by {
            let deps: Vec<String> = raw.split(',').map(|d| d.trim().to_string()).filter(|d| !d.is_empty()).collect();
            let unknown: Vec<&String> = deps
                .iter()
                .filter(|d| d.as_str() != todo::USER && !st.todos.iter().any(|t| &t.id == *d))
                .collect();
            if !unknown.is_empty() {
                return Ok(Err(anyhow::anyhow!(
                    "unknown blocked_by ids: {}",
                    unknown.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
                )));
            }
            st.todos[idx].blocked_by = deps;
        }
        if let Some(s) = &status {
            if let Err(e) = todo::set_status(st, id, s, None) {
                return Ok(Err(e));
            }
        }
        st.todos[idx].updated_at = state::now_iso();
        tick::record(st, "edit", Some(id), "edit", &who)?;
        // 没活可跑 ≠ 目标结束：把最后一条 todo 延后也会清空 open 列表，但一条都没做完，
        // 这时标 done 会让 `decide` 在 goal.status 那一关就返回 done，`status` 说"目标结束"，
        // `start` 让人去 goal new——两条被推到以后的活就此没人再看（B-3）。
        let finished = todo::open_ordered(st).is_empty() && !todo::all_deferred(st);
        if st.goal.status == "done" && !finished {
            st.goal.status = "active".into();
        } else if finished {
            st.goal.status = "done".into();
        }
        Ok(Ok(st.todos[idx].clone()))
    })?;
    match result {
        Ok(t) => {
            let deps = if t.blocked_by.is_empty() { String::new() } else { format!(" ⏳{}", t.blocked_by.join(",")) };
            println!("{} [P{}] {} {}{}", t.id, t.priority, t.status, t.text, deps);
            Ok(0)
        }
        Err(e) => {
            eprintln!("edit: {e}");
            Ok(2)
        }
    }
}

/// 清单里的状态词：停着的目标不叫"进行中"（没人会派活给它），坏掉的直接说坏了。
fn row_status_zh(r: &crate::goals::Row) -> &str {
    match r.status.as_str() {
        crate::goals::BROKEN => "损坏",
        "active" if !r.current => "停放",
        other => goal_status_zh(other),
    }
}

fn goal_status_zh(s: &str) -> &str {
    match s {
        "active" => "进行中",
        "done" => "完成",
        "paused" => "暂停",
        other => other,
    }
}

fn short_time(iso: &str) -> String {
    state::parse_iso(iso).map(|dt| dt.format("%m-%d %H:%M").to_string()).unwrap_or_else(|_| iso.chars().take(11).collect())
}

/// 问一句 y/N。答 `y` / `yes` 才算同意，别的（含直接回车）都算不同意。
///
/// **故意不看 stdin 是不是终端**：测试和脚本给的都是管道，一旦按 TTY 分叉，这条路就只剩
/// 人手能走、测不到。stdin 读到 EOF 说明根本没人接话（`</dev/null` 起的、runner 里跑的），
/// 这时候不能当成"默认不同意"悄悄退——那样脚本只会看到一个没解释的非零码，所以直接报错
/// 并把免问的写法给出来。
fn confirm(question: &str) -> Result<bool> {
    print!("{question} [y/N] ");
    std::io::Write::flush(&mut std::io::stdout())?;
    let mut line = String::new();
    if std::io::BufRead::read_line(&mut std::io::stdin().lock(), &mut line)? == 0 {
        bail!("这一步要确认，但 stdin 没有输入可读：用精确 id 重来，或者加 --yes");
    }
    Ok(matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes"))
}

/// `--force` 把带着在飞派活的目标停走了：那个会话再写回会被 `done` 拦下，先说清楚。
fn warn_parked_handout(p: &crate::goals::Row, c: style::Style) {
    let Ok(st) = state::load(&p.path) else { return };
    let Some(ip) = st.in_progress else { return };
    println!(
        "  {} {} 第 {} 轮还在别的会话手里；它写回时会被拦下，让它先 `zloop goal switch {}`",
        c.dim("⚠"),
        ip.todo,
        ip.round,
        p.id
    );
}

/// `zloop doctor`：只读，什么都不动。退出码 1 只在有"要修"的问题时给出，
/// "留意"级别照样退 0——否则 CI 里挂一个删掉的旧日志就红一片。
fn cmd_doctor(root: &Path, json: bool, c: style::Style) -> Result<i32> {
    if !crate::doctor::is_project(root) {
        // 和其他命令同一个口径：不是 zloop 项目就按 StateError 退 1
        return Err(state::StateError(format!(
            "no zloop state at {} (run `zloop init \"<goal>\"` first)",
            state::state_path(root).display()
        ))
        .into());
    }
    let report = crate::doctor::check(root);
    if json {
        print_json(&serde_json::to_value(&report)?);
        return Ok(if report.errors > 0 { 1 } else { 0 });
    }
    println!();
    println!("  {}", c.dim(&format!("体检 {} · 目标 {} 个 · 归档 {} 份", root.join(state::STATE_DIR).display(), report.goals, report.archived)));
    if report.ok() {
        println!("  {}", c.green("没发现问题"));
        println!();
        return Ok(0);
    }
    println!();
    for f in &report.findings {
        let (mark, head) = match f.level {
            crate::doctor::Level::Error => ("✗", c.red(&f.what)),
            crate::doctor::Level::Warn => ("!", c.yellow(&f.what)),
        };
        println!("  {mark} {head}");
        println!("    {} {}", c.dim("→"), c.bold(&f.fix));
    }
    println!();
    println!(
        "  {}",
        c.dim(&format!("{} 个问题：{} 个要修、{} 个留意（doctor 只读，一个字都没改）", report.findings.len(), report.errors, report.warnings))
    );
    println!();
    Ok(if report.errors > 0 { 1 } else { 0 })
}

fn cmd_goal(root: &Path, cmd: GoalCmd, c: style::Style) -> Result<i32> {
    match cmd {
        GoalCmd::List { json } => {
            let rows = crate::goals::list(root);
            if json {
                let v: Vec<serde_json::Value> = rows
                    .iter()
                    .map(|r| {
                        serde_json::json!({
                            "id": r.id, "text": r.text, "status": r.status, "done": r.done,
                            "total": r.total, "last": r.last, "current": r.current,
                        })
                    })
                    .collect();
                print_json(&serde_json::Value::Array(v));
                return Ok(0);
            }
            if rows.is_empty() {
                println!("这个项目还没有目标：`zloop init \"目标\"`");
                return Ok(0);
            }
            let w = style::term_width().clamp(46, 110);
            let id_w = rows.iter().map(|r| style::width(&r.id)).max().unwrap_or(2).max(2);
            let st_w = rows.iter().map(|r| style::width(row_status_zh(r))).max().unwrap_or(6);
            let pg_w = rows.iter().map(|r| format!("{}/{}", r.done, r.total).len()).max().unwrap_or(3);
            let has_current = rows.iter().any(|r| r.current);
            println!();
            if has_current {
                println!("  {}", c.dim(&format!("共 {} 个目标 · ▸ 是当前那个", rows.len())));
            } else {
                // 没有当前目标（上一次搬家中断，或刚归档掉了当前那个）：别让图例指着不存在的 ▸
                println!("  {}", c.dim(&format!("共 {} 个目标 · 当前没有目标在开着", rows.len())));
            }
            for r in &rows {
                let progress = format!("{}/{}", r.done, r.total);
                let head = format!(
                    "{} {}{}  {}{}  {}{}  {}",
                    if r.current { "▸" } else { " " },
                    r.id,
                    " ".repeat(id_w - style::width(&r.id)),
                    row_status_zh(r),
                    " ".repeat(st_w - style::width(row_status_zh(r))),
                    " ".repeat(pg_w - progress.len()),
                    progress,
                    short_time(&r.last),
                );
                let text = style::truncate(&r.text, w.saturating_sub(style::width(&head) + 4));
                if r.current {
                    println!("  {}  {}", c.cyan(&head), c.bold(&text));
                } else {
                    println!("  {}  {}", c.dim(&head), c.dim(&text));
                }
            }
            println!();
            if !has_current {
                println!("  {}  {}", c.dim("开一个"), c.bold("zloop goal switch <id>   # 先把一个开进来，其余命令才能用"));
            }
            println!("  {}  {}", c.dim("切换"), c.bold("zloop goal switch <id 或目标里的片段>"));
            println!("  {}  {}", c.dim("新建"), c.bold("zloop goal new \"新目标\""));
            println!();
            Ok(0)
        }
        GoalCmd::New { text, id, force } => {
            let (parked, cur) = crate::goals::create(root, &text, id.as_deref(), force)?;
            if let Some(p) = parked {
                println!(
                    "停放「{}」[{}] {} {}/{} · 切回：{}",
                    style::truncate(&p.text, 30),
                    p.id,
                    goal_status_zh(&p.status),
                    p.done,
                    p.total,
                    c.bold(&format!("zloop goal switch {}", p.id))
                );
                warn_parked_handout(&p, c);
            }
            println!("新目标 [{}] {}", cur.id, cur.text);
            println!("下一步：{}", c.bold("zloop plan   # 每行一条 `[P0] 文本`，从 stdin 读"));
            Ok(0)
        }
        GoalCmd::Switch { needle, force } => {
            let sw = crate::goals::switch(root, &needle, force)?;
            match sw.parked {
                Some(p) => {
                    println!(
                        "停放「{}」[{}] · 切回：{}",
                        style::truncate(&p.text, 30),
                        p.id,
                        c.bold(&format!("zloop goal switch {}", p.id))
                    );
                    warn_parked_handout(&p, c);
                }
                None => println!("已经是当前目标"),
            }
            println!(
                "当前目标 [{}] {} · {} {}/{}",
                sw.current.id,
                sw.current.text,
                goal_status_zh(&sw.current.status),
                sw.current.done,
                sw.current.total
            );
            Ok(0)
        }
        GoalCmd::Rm { needle, yes } => {
            let (row, how) = crate::goals::resolve_match(root, &needle)?;
            // 能不能归档要在问之前定：等人敲完 y 再说"其实不能"是最难受的顺序
            crate::goals::ensure_archivable(&row)?;
            if how.is_fuzzy() && !yes {
                println!(
                    "将要归档 [{}] {} · {} {}/{}",
                    row.id,
                    row.text,
                    goal_status_zh(&row.status),
                    row.done,
                    row.total
                );
                println!("（{needle:?} 是按 {} 对上的，不是精确 id；免问：{}）", how.zh(), c.bold(&format!("zloop goal rm {} --yes", row.id)));
                if !confirm("确认归档？")? {
                    println!("已取消，一个文件都没动");
                    return Ok(1);
                }
            }
            let target = crate::goals::archive(root, &row)?;
            println!("已归档 [{}] {} → {}", row.id, style::truncate(&row.text, 40), target.display());
            println!("（文件还在，只是不再出现在 `zloop goal list` 里）");
            Ok(0)
        }
    }
}

fn cmd_status(root: &Path, path: &Path, json: bool, md: bool, st_style: style::Style) -> Result<i32> {
    let st = state::load(path)?;
    if json {
        print_json(&serde_json::to_value(&st)?);
        return Ok(0);
    }
    if md {
        print!("{}", prompt::render_md(&st, root, 10));
        return Ok(0);
    }
    let c = &st_style;
    let now = state::now();
    let d = tick::decide(&st, now);
    let ph = phase::compute(&st, root, now);
    let running = daemon::running(root);

    // Fit the real terminal and truncate every line to it. A line that wraps loses the left
    // gutter, and that — not the colours — is what made the fixed-width layout look ragged.
    let w = style::term_width().clamp(46, 96);
    let text = w.saturating_sub(4); // after the two-space gutter
    let val = w.saturating_sub(12); // after gutter + eight-column label

    // ---- the verdict, in one word ----
    let finished = st.todos.iter().filter(|t| t.status == "done").count();
    let total = st.todos.len();
    // 延后的 todo 在调度器眼里已经了结（`todo::is_terminal` 含 deferred），所以它不能留在
    // 进度的分母里——否则会出现"✅ 完成 + 8 条全部完成 + 75%"这种自相矛盾的一屏。
    let deferred = st.todos.iter().filter(|t| t.status == "deferred").count();
    let planned = total - deferred;
    let later = if deferred > 0 { format!(" · {deferred} 条延后") } else { String::new() };
    let first_deferred = st.todos.iter().find(|t| t.status == "deferred").map(|t| t.id.clone());
    let (icon, word, code): (&str, &str, &str) = match () {
        // 刚开的目标还没有待办：说"待规划"，别说"全部完成"（decide 对空清单返回 all_done）
        _ if total == 0 => ("◦", "待规划", "34"),
        _ if st.goal.status == "done" => ("✅", "完成", "32"),
        _ if st.goal.status == "paused" => ("⏸", "已暂停", "33"),
        // 一条没做完、全推到了以后：这不是"完成"，也不是"空闲"，别让它跟正常收工同一个词
        _ if d.reason == "all_deferred" => ("⏭", "全部延后", "33"),
        _ if matches!(d.reason.as_str(), "fail_streak" | "progress_streak" | "budget") => ("⛔", "已停", "31"),
        _ if matches!(d.reason.as_str(), "user_gate" | "blocked") => ("⏳", "等你决定", "33"),
        _ if ph.kind == "executing" => ("🔄", "执行中", "36"),
        _ if ph.kind == "sleeping" => ("💤", "休眠中", "34"),
        _ if d.reason == "throttled" => ("⏱", "限流中", "33"),
        _ if d.should_run => ("▶", "就绪", "34"),
        _ => ("•", "空闲", "2"),
    };
    let pct = (finished * 100).checked_div(planned).unwrap_or(0);
    let spent = tick::spent_usd(&st.ticks);
    let money = if spent > 0.0 {
        let cap = if st.policy.max_total_usd > 0.0 { format!("（上限 ${:.2}）", st.policy.max_total_usd) } else { String::new() };
        format!(" · 花了 ${spent:.2}{cap}")
    } else {
        String::new()
    };
    let icon = format!("{icon}{}", " ".repeat(2usize.saturating_sub(style::width(icon))));
    let head = format!(
        "{}{}",
        if code == "2" { c.dim(word) } else { c.banner(code, word) },
        " ".repeat(10usize.saturating_sub(style::width(word)).max(1))
    );
    // The bar is the first thing to go in a narrow window; the percentage carries the same news.
    let bar = if w >= 70 { format!("{} ", style::bar(finished, planned, 16, c)) } else { String::new() };
    println!();
    println!(
        "  {icon} {head}{bar}{}  {}",
        c.bold(&format!("{pct}%")),
        // 干活的轮次，失败也算 —— `tick::current_round` 只数成事的那些，
        // 连着失败三轮之后会读成"0 轮"。和 `zloop stats` 共用 `tick::rounds` 的定义。
        // 「几条待办」交给下面的步骤清单说，标题只留轮数和花费。
        c.dim(&format!("跑了 {} 轮{money}", tick::rounds(&st.ticks)))
    );
    println!("  {}    {}", c.dim("目标"), style::truncate(&st.goal.text, text.saturating_sub(8)));

    // ---- 清单：做过的、正在做的、还没做的，一张带框的表 ----
    // 两列编号是两回事，都要显示：**步骤**是执行顺序（`state.todos` 的数组顺序），
    // **id** 是创建时发的（t1…tN），而 `done --next` 会把后继插在当前这条后面——
    // 于是第 4 步可能是 t8。以前只给没做完的行显示 id，看的人只能猜，正是误解的来源。
    /// 清单最多印这么多行（**行**不是条：带验收的那条占两行）。
    /// 给得宽松——用户要的是全貌，截尾是最后手段。
    const MAX_LINES: usize = 40;
    if !st.todos.is_empty() {
        let next_id = d.todo.as_ref().map(|t| t.id.clone());
        struct Row {
            n: usize,
            id: String,
            text: String,
            /// 「进展」列：图标 + 状态词
            stat: String,
            finished: bool,
            paint: u8, // 0 dim, 1 done, 2 active, 3 wait
            sub: Vec<(String, String, u8)>, // (前缀, 内容, paint)
        }
        let mut rows: Vec<Row> = Vec::new();
        for (i, t) in st.todos.iter().enumerate() {
            let running_now = st.in_progress.as_ref().map(|ip| ip.todo.as_str()) == Some(t.id.as_str());
            let waiting_on_you = t.blocked_by.iter().any(|b| b == todo::USER);
            // 和 todo::is_executable 同一口径：依赖还没 done 就算被挡着，与 status 是 open 还是 blocked 无关。
            let pending_dep = t.blocked_by.iter().find(|dep| {
                dep.as_str() != todo::USER && !st.todos.iter().any(|x| &x.id == *dep && x.status == "done")
            });
            let is_next = next_id.as_deref() == Some(t.id.as_str());
            let (icon, word, paint): (&str, String, u8) = if t.status == "done" {
                ("✅", "完成".into(), 1)
            } else if t.status == "deferred" {
                ("⏭", "已延后".into(), 0)
            } else if running_now {
                ("🔄", "执行中".into(), 2)
            } else if waiting_on_you {
                ("❗", "等你回话".into(), 3)
            } else if let Some(dep) = pending_dep {
                // id 现在每行都看得见，所以直接说等哪条，不用再绕一层步骤号
                ("⏳", format!("等 {dep}"), 0)
            } else if is_next {
                ("▶", "下一个".into(), 2)
            } else {
                ("○", "排队中".into(), 0)
            };
            // 附注缩进两格并挂一个 `↳`：不缩的话它和上一行一样顶格，看上去是两条并列的
            // 记录，而不是同一条的下半截。缩进**在这里**加不在打印时加——列宽是照
            // `sub` 的内容算的，打印时才加前缀的话，算出来的宽度装不下自己（踩过：
            // 「验收：tests green」被截成「验收：tests …」）。
            const IND: &str = "  ↳ ";
            let mut sub = Vec::new();
            if waiting_on_you && !t.note.is_empty() {
                sub.push(("  ↳".into(), t.note.clone(), 3));
            }
            if waiting_on_you {
                sub.push((format!("{IND}答完敲"), format!("zloop edit {} --status open", t.id), 4));
            }
            if let Some(a) = &t.acceptance {
                if t.status != "done" {
                    sub.push((format!("{IND}验收："), a.clone(), 0));
                }
            }
            rows.push(Row {
                n: i + 1,
                id: t.id.clone(),
                text: t.text.clone(),
                stat: format!("{icon} {word}"),
                finished: todo::is_terminal(&t.status),
                paint,
                sub,
            });
        }

        // 只有清单大到一屏塞不下才截尾，**而且只截尾**——做完的那些照样列出来。
        // 用户要看的就是全貌；把「完成 完成 完成」收起来是替他做决定，不是帮忙。
        // 预算按**印出来的行数**算，不按 todo 条数：带验收的一条占两行，
        // 按条数记账会算漏（旧的 MAX_ROWS=15 遇上正好 15 条时一行不截，却印了 22 行）。
        let lines_of = |r: &Row| 1 + r.sub.len();
        let mut shown = &rows[..];
        let mut tail = 0;
        let mut budget = MAX_LINES;
        let mut keep = 0;
        for r in shown {
            let need = lines_of(r);
            if keep > 0 && budget < need + 1 {
                break; // +1 给「后面还有 N 步」那一行留位置
            }
            budget = budget.saturating_sub(need);
            keep += 1;
        }
        if keep < shown.len() {
            tail = shown.len() - keep;
            shown = &shown[..keep];
        }

        // 列宽：中文和 emoji 都按 style::width 的两列口径算，否则框线会歪
        let head = ["步骤", "id", "这一步做什么", "进展"];
        let w_n = shown.iter().map(|r| r.n.to_string().len()).max().unwrap_or(1).max(style::width(head[0]));
        let w_id = shown.iter().map(|r| style::width(&r.id)).max().unwrap_or(2).max(style::width(head[1]));
        let w_st = shown.iter().map(|r| style::width(&r.stat)).max().unwrap_or(0).max(style::width(head[3]));
        let widest = shown
            .iter()
            .map(|r| style::width(&r.text))
            .chain(shown.iter().flat_map(|r| r.sub.iter()).map(|(p, ct, _)| {
                style::width(p) + usize::from(!p.ends_with('：')) + style::width(ct)
            }))
            .max()
            .unwrap_or(0);
        // 框线开销：4 列各占 `│ 内容 ` = 宽度 + 3，末尾再一个 `│`
        let w_tx = text.saturating_sub(w_n + w_id + w_st + 13).max(12).min(widest.max(style::width(head[2])));

        let bar_ch = c.dim("│");
        let rule = |l: &str, m: &str, r: &str| {
            c.dim(&format!(
                "{l}{}{m}{}{m}{}{m}{}{r}",
                "─".repeat(w_n + 2),
                "─".repeat(w_id + 2),
                "─".repeat(w_tx + 2),
                "─".repeat(w_st + 2)
            ))
        };
        let pad = |s: &str, w: usize| " ".repeat(w.saturating_sub(style::width(s)));
        // 附注和折叠提示只占「这一步做什么」那一格，别的格留空——框线才不会错位
        let note_row = |body: &str| {
            let text = style::truncate(body, w_tx);
            println!(
                "  {bar_ch} {} {bar_ch} {} {bar_ch} {}{} {bar_ch} {} {bar_ch}",
                " ".repeat(w_n),
                " ".repeat(w_id),
                c.dim(&text),
                pad(&text, w_tx),
                " ".repeat(w_st),
            );
        };

        println!();
        println!("  {}    {}", c.dim("清单"), c.bold(&format!("{finished}/{planned} 完成{later}")));
        println!("  {}", rule("┌", "┬", "┐"));
        println!(
            "  {bar_ch} {}{} {bar_ch} {}{} {bar_ch} {}{} {bar_ch} {}{} {bar_ch}",
            c.dim(head[0]),
            pad(head[0], w_n),
            c.dim(head[1]),
            pad(head[1], w_id),
            c.dim(head[2]),
            pad(head[2], w_tx),
            c.dim(head[3]),
            pad(head[3], w_st),
        );
        println!("  {}", rule("├", "┼", "┤"));
        // 表格宽度装不下的命令：半条命令比放到表外更糟，所以攒起来印在表下面
        let mut spill: Vec<(String, String)> = Vec::new();
        // 已完成的那一段和「从这里往下还没做」之间画一道线。
        // 15 行同样粗细的框线连在一起就是一堵墙——眼睛需要一个落点，
        // 而这个落点天然就是"做完的到此为止"。
        let split = shown.iter().position(|r| !r.finished).filter(|&i| i > 0 && i < shown.len());
        for (i, r) in shown.iter().enumerate() {
            if split == Some(i) {
                println!("  {}", rule("├", "┼", "┤"));
            }
            let body = style::truncate(&r.text, w_tx);
            let n = r.n.to_string();
            let (body_p, stat_p) = (pad(&body, w_tx), pad(&r.stat, w_st));
            let (body_c, stat_c) = match r.paint {
                1 => (c.dim(&body), c.green(&r.stat)),
                2 => (c.bold(&body), c.cyan(&r.stat)),
                3 => (c.yellow(&body), c.yellow(&r.stat)),
                _ => (c.dim(&body), c.dim(&r.stat)),
            };
            println!(
                "  {bar_ch} {}{} {bar_ch} {}{} {bar_ch} {body_c}{body_p} {bar_ch} {stat_c}{stat_p} {bar_ch}",
                " ".repeat(w_n.saturating_sub(n.len())),
                if r.paint == 2 { c.cyan(&n) } else { c.dim(&n) },
                if r.paint == 2 { c.cyan(&r.id) } else { c.dim(&r.id) },
                pad(&r.id, w_id),
            );
            for (prefix, content, paint) in &r.sub {
                let room = w_tx.saturating_sub(style::width(prefix) + usize::from(!prefix.ends_with('：')));
                if *paint == 4 && style::width(content) > room {
                    spill.push((r.id.clone(), content.clone()));
                    continue;
                }
                let content = style::truncate(content, room);
                // 「验收：」这类前缀自带冒号，再加空格就成了两道分隔
                let sep = if prefix.ends_with('：') { "" } else { " " };
                let plain = format!("{prefix}{sep}{content}");
                let painted = match paint {
                    3 => format!("{}{sep}{}", c.yellow(prefix), c.yellow(&content)),
                    4 => format!("{}{sep}{}", c.dim(prefix), c.bold(&content)),
                    _ => c.dim(&plain),
                };
                println!(
                    "  {bar_ch} {} {bar_ch} {} {bar_ch} {painted}{} {bar_ch} {} {bar_ch}",
                    " ".repeat(w_n),
                    " ".repeat(w_id),
                    pad(&plain, w_tx),
                    " ".repeat(w_st),
                );
            }
        }
        if tail > 0 {
            note_row(&format!("… 后面还有 {tail} 步 · zloop status --json 看全部"));
        }
        println!("  {}", rule("└", "┴", "┘"));
        for (id, cmd) in &spill {
            println!("  {} {}", c.dim(&format!("{id} 答完敲")), c.bold(cmd));
        }
    }

    // ---- facts worth a line, i.e. the ones the headline does not already state ----
    let mut rows: Vec<(&str, String)> = Vec::new();
    // 「现在在哪一步」是用户最想要的一行，所以它常驻：phase 没有新消息时，
    // 就由状态本身兜底成一句人话，而不是让这一行消失。
    let stage = if total == 0 {
        "还没有待办 · 先用 zloop plan 加几条".to_string()
    } else if !ph.detail.is_empty() {
        ph.detail.clone()
    } else if d.reason == "all_deferred" {
        format!("{deferred} 条待办全被延后了，一条都没完成 · 目标没结束，只是没活可跑")
    } else if st.goal.status == "done" || d.reason == "all_done" {
        format!("{planned} 条待办全部完成，目标结束{}", if deferred > 0 { format!("（另有 {deferred} 条延后）") } else { String::new() })
    } else if st.goal.status == "paused" {
        "你按了暂停，待办原地保留".into()
    } else if d.should_run {
        match (running, d.todo.as_ref()) {
            (Some(_), Some(t)) => format!("runner 在跑，下一轮做 {}", t.id),
            (None, Some(t)) => format!("没人在跑 · 下一条是 {}", t.id),
            _ => "等着开跑".into(),
        }
    } else {
        phase::reason_zh(&d.reason)
    };
    rows.push(("阶段", style::truncate(&stage, val)));
    // 后台也常驻：不说"没在跑"，就分不清是没人跑还是你忘了看。
    // 「合盖不休眠」讲的就是这个 runner 的运行时状态，正常时并进「后台」这一行，
    // 别为一句不需要动作的话单占一行；只有它反常（要人处理）时才单列出来喊一声。
    let awake = crate::awake::brief();
    let awake_inline = awake.as_ref().filter(|(_, warn)| !warn).map(|(s, _)| s.clone());
    rows.push((
        "后台",
        match running {
            Some(pid) => {
                // 日志路径是能直接敲的东西，排在前面：窄屏时先被截掉的该是
                // 「合盖不休眠」这种只是让人安心、不需要动作的话
                let mut line = format!("runner 在跑（pid {pid}）· 日志 .zloop/runner/console.log");
                if let Some(a) = &awake_inline {
                    line.push_str(&format!(" · {}", a.split(" · ").next().unwrap_or(a)));
                }
                c.dim(&style::truncate(&line, val))
            }
            None => c.dim("没有 runner 在跑"),
        },
    ));
    if let Some((s, true)) = awake.as_ref().map(|(s, w)| (s.clone(), *w)) {
        rows.push(("睡眠", c.yellow(&format!("⚠ {}", style::truncate(&s, val.saturating_sub(2))))));
    }
    // 人说过的话要在人自己的视图里也能看见，否则"我说了它没反应"无从判断
    let pending = crate::tick::pending_feedback(&st);
    if let Some(last) = pending.last() {
        let head = if pending.len() > 1 { format!("{} 条待处理，最近：", pending.len()) } else { String::new() };
        let text = style::truncate(&last.note, val.saturating_sub(style::width(&head) + 2));
        rows.push(("反馈", c.yellow(&format!("{head}{text}"))));
    }
    let undocumented = st.ticks.iter().filter(|t| t.documented == Some(false)).count();
    if undocumented > 0 {
        rows.push(("文档", c.yellow(&format!("{undocumented} 轮缺实现思路 · zloop log 里带 ⚠"))));
    }
    let parked = crate::goals::parked(root).len();
    if parked > 0 {
        rows.push(("其他", c.dim(&format!("另有 {parked} 个目标停着 · zloop goal list"))));
    }
    let sessions = session::summarize(&st, root);
    // 最近干活的那个会话，不是"最近第一次露面"的那个（summarize 按首次出现排序）
    if let Some(cmd) = session::latest(&sessions, None).and_then(|s| s.resume.as_deref()) {
        // Never truncated: a half-copied resume command is worse than a line that wraps.
        rows.push(("会话", cmd.to_string()));
    }

    // ---- and what you can type next ----
    let mut acts: Vec<(&str, String)> = Vec::new();
    if total == 0 {
        acts.push(("加待办", "zloop plan --add \"[P0] 第一步\" --add \"[P1] 第二步\"".into()));
        acts.push(("换目标", "zloop goal new \"另一个目标\"".into()));
    } else if st.goal.status == "paused" {
        acts.push(("继续", "zloop resume".into()));
    } else if d.reason == "all_deferred" {
        // 第一位是"捡回来"而不是"换目标"：延后的活还在清单里，别引着人把它们丢掉
        acts.push(("捡回来", format!("zloop edit {} --status open", first_deferred.as_deref().unwrap_or("<id>"))));
        acts.push(("加活", "zloop plan --add \"[P0] 下一件事\"".into()));
    } else if st.goal.status == "done" || d.reason == "all_done" {
        acts.push(("加活", "zloop plan --add \"[P0] 下一件事\"".into()));
        acts.push(("换目标", "zloop goal new \"新目标\"".into()));
        if st.ticks.iter().any(|t| t.log.is_some()) {
            acts.push(("出文档", "zloop doc --all".into()));
        }
    } else {
        // 解锁命令已经贴在被挡住的那条 todo 下面了，页脚不再重复。
        match d.reason.as_str() {
            "blocked" => acts.push(("查依赖", "zloop status --json".into())),
            "throttled" => acts.push(("放宽", "改 .zloop/state.json 的 policy.max_runs（0 = 不限）".into())),
            "fail_streak" => {
                acts.push(("看失败", "zloop log".into()));
                acts.push(("重跑", "zloop start".into()));
            }
            "progress_streak" => acts.push(("拆小", format!("zloop edit {} --text \"更小的一步\"", d.todo.as_ref().map(|t| t.id.as_str()).unwrap_or("<id>")))),
            "budget" => acts.push(("提额", "改 .zloop/state.json 的 policy.max_total_usd".into())),
            _ => {}
        }
        match running {
            Some(_) => {
                acts.push(("看日志", "zloop log".into()));
                acts.push(("停止", "zloop stop".into()));
            }
            // 有会话正拿着这条 todo：该敲的不是"再开一个 runner"，而是把这一轮写回去。
            None if ph.kind == "executing" => {
                if let Some(ip) = &st.in_progress {
                    acts.push(("写回", format!("zloop done {} --note \"<一句话结果>\" --approach \"<怎么做的>\"", ip.todo)));
                }
            }
            // 休眠说明有 runner 在跑，只是它是前台 `zloop run`（不写 pid 文件）——
            // 这时劝人再开一个是错的，给"看日志"。
            None if ph.kind == "sleeping" => acts.push(("看日志", "zloop log".into())),
            None if d.should_run => acts.push(("开跑", "zloop start".into())),
            None => {}
        }
    }

    let line = |label: &str, value: &str, label_color: fn(&style::Style, &str) -> String| {
        let pad = " ".repeat(8usize.saturating_sub(style::width(label)));
        println!("  {}{pad}{value}", label_color(c, label));
    };
    if !rows.is_empty() {
        println!();
        for (label, value) in rows {
            line(label, &value, |c, s| c.dim(s));
        }
    }
    if !acts.is_empty() {
        println!();
        for (label, cmd) in acts {
            line(label, &c.bold(&cmd), |c, s| c.cyan(s)); // commands are never truncated either
        }
    }
    println!();
    Ok(0)
}

fn cmd_sessions(root: &Path, path: &Path, host: Option<String>, json: bool) -> Result<i32> {
    let st = state::load(path)?;
    let rows: Vec<_> = session::summarize(&st, root)
        .into_iter()
        .filter(|r| host.as_deref().map(|h| r.host == h).unwrap_or(true))
        .collect();
    if json {
        let v: Vec<serde_json::Value> = rows
            .iter()
            .map(|r| {
                serde_json::json!({
                    "host": r.host, "session": r.session, "ticks": r.ticks, "first": r.first, "last": r.last,
                    "todos": r.todos, "resume": r.resume,
                    "transcript": r.transcript.as_ref().map(|p| p.display().to_string()),
                    "transcript_exists": r.transcript.as_ref().map(|p| p.exists()).unwrap_or(false),
                })
            })
            .collect();
        print_json(&serde_json::Value::Array(v));
        return Ok(0);
    }
    if rows.is_empty() {
        println!("no host sessions recorded yet (ticks written outside Claude Code / Codex have none)");
        return Ok(0);
    }
    for r in rows {
        let exists = match &r.transcript {
            Some(p) if p.exists() => "✓ transcript",
            Some(_) => "transcript missing",
            None => "",
        };
        println!(
            "{:<7}{}  ticks {:<3} {} → {}  todos {}  {}",
            r.host,
            r.session,
            r.ticks,
            r.first,
            r.last,
            r.todos.join(","),
            exists
        );
        if let Some(cmd) = r.resume {
            println!("        {cmd}");
        }
    }
    Ok(0)
}

fn cmd_log(root: &Path, todo: Option<String>, last: usize, show: Option<String>) -> Result<i32> {
    if let Some(name) = show {
        let mut p = PathBuf::from(&name);
        if !p.exists() {
            p = root.join(state::STATE_DIR).join(log::LOG_DIR).join(&name);
        }
        if !p.exists() {
            p = root.join(state::STATE_DIR).join(&name);
        }
        if !p.exists() {
            eprintln!("log: {name} not found");
            return Ok(2);
        }
        print!("{}", std::fs::read_to_string(p)?);
        return Ok(0);
    }
    let st = state::load(&state::state_path(root))?;
    let (files, hidden) = log::entries(root, &st, todo.as_deref(), last)?;
    if files.is_empty() {
        if hidden > 0 {
            println!("这个目标还没有日志（另有 {hidden} 份属于别的目标 · zloop goal list）");
        } else {
            println!("no logs yet (written by `zloop done`)");
        }
        return Ok(0);
    }
    let mut undocumented = 0;
    for (f, tick) in &files {
        let rel = f.strip_prefix(root).unwrap_or(f);
        // 这一轮是不是"完成"、有没有留实现思路，都认 tick 的账；无主文件才退回文件名 / 读文件
        let expects_doc = match tick {
            Some(t) => t.outcome == "done",
            None => log::name_is_done(f),
        };
        let documented = match tick.as_ref().and_then(|t| t.documented) {
            Some(v) => v,
            None => log::file_is_documented(f),
        };
        let mark = if !expects_doc || documented {
            "  "
        } else {
            undocumented += 1;
            "⚠ "
        };
        println!("{mark}{}  {}", rel.display(), log::first_line(f));
    }
    if undocumented > 0 {
        println!("\n⚠ = 只有结果记录，没有实现思路；`zloop doc <todo>` 汇总时会标出来");
    }
    if hidden > 0 {
        println!("\n另有 {hidden} 份日志属于别的目标，没有列出来（`zloop goal list`）");
    }
    Ok(0)
}

/// `zloop reflect`：不做 todo 的那一轮——把材料摆齐给模型，或者把人点头后的结果落地。
///
/// zloop 自己不产生判断：它只汇材料 + 做几项机械体检。判断是模型的事，落地要人点头
/// （Warp 那边人审的形态是 PR review，zloop 没有 PR，所以是 `--apply` 这一步）。
fn cmd_reflect(root: &Path, path: &Path, apply: bool, max_rules: usize, c: style::Style) -> Result<i32> {
    let st = state::load(path)?;
    if !apply {
        print!("{}", crate::reflect::packet(&st, root, crate::notes::WINDOW, max_rules));
        return Ok(0);
    }
    let mut raw = String::new();
    std::io::Read::read_to_string(&mut std::io::stdin(), &mut raw)?;
    // 模型抄回来的清单：容忍编号（"1. "、"R1. "）、各种项目符号、以及有没有小标题
    let cleaned: String = raw
        .lines()
        .map(|l| {
            let t = l.trim();
            if t.starts_with("## ") {
                return t.to_string();
            }
            let t = t.trim_start_matches(['-', '*', '·', 'R']).trim();
            let t = match t.split_once(". ") {
                Some((n, rest)) if n.chars().all(|ch| ch.is_ascii_digit()) => rest,
                _ => t,
            };
            if t.is_empty() { String::new() } else { format!("- {t}") }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let next = crate::notes::parse(&cleaned);
    if next.rules.is_empty() && next.lessons.is_empty() {
        eprintln!("reflect --apply: stdin 是空的。要清空请显式给一行占位，或者直接删 .zloop/NOTES.md");
        return Ok(2);
    }
    let before = crate::notes::read(root);
    let (p, backup) = crate::notes::replace(root, &next)?;
    println!(
        "约定 {} → {} 条 · 经验 {} → {} 条：{}",
        before.rules.len(),
        next.rules.len(),
        before.lessons.len(),
        next.lessons.len(),
        p.display()
    );
    println!("  {} {}", c.dim("旧的备份在"), backup.display());
    println!("  {} {}", c.dim("下一轮的"), c.bold("zloop context：约定全带，经验带最新几条"));
    Ok(0)
}

/// `zloop stats`：把账本里已经记着的东西汇成"这个目标跑得顺不顺"。
///
/// 和 `status` 分工：`status` 回答"还剩什么、我该敲什么"，`stats` 回答"跑得怎么样"——
/// 返工率、一次过、哪一步最费劲。它同时是 reflect（W2/W6）的输入，
/// 因为 Warp 那条回路是"跑 → 打分 → 自改进"，打分得先有人算出来。
fn cmd_stats(path: &Path, json: bool, c: style::Style) -> Result<i32> {
    let st = state::load(path)?;
    let s = crate::stats::compute(&st);
    if json {
        print_json(&serde_json::to_value(&s)?);
        return Ok(0);
    }
    let w = style::term_width().clamp(46, 96);
    let text = w.saturating_sub(4);
    let pct = |a: usize, b: usize| (a * 100).checked_div(b).map(|v| format!("{v}%")).unwrap_or_else(|| "—".into());

    println!();
    println!("  {}    {}", c.dim("统计"), style::truncate(&s.goal, text.saturating_sub(8)));
    println!();
    if s.rounds == 0 {
        println!("  {}", c.dim("还没有跑过任何一轮 · zloop next 开始"));
        return Ok(0);
    }
    let mut rows: Vec<(&str, String)> = vec![
        ("轮次", format!("{} 轮 · 返工 {}（{}）· 失败 {}", s.rounds, s.rework, pct(s.rework, s.rounds), s.fails)),
        (
            "质量",
            format!(
                "一次过 {}/{} 条 · 无文档 {} 轮 · 被挡 {} 次 · 用户反馈 {} 条{}",
                s.first_try,
                s.done,
                s.undocumented,
                s.blocks,
                s.feedback,
                if s.reflects > 0 { format!(" · 回看 {} 次", s.reflects) } else { String::new() }
            ),
        ),
    ];
    if s.cost_usd > 0.0 || s.duration_ms > 0 {
        let mut v = Vec::new();
        if s.cost_usd > 0.0 {
            v.push(format!("${:.2}", s.cost_usd));
        }
        if s.duration_ms > 0 {
            v.push(format!("宿主累计 {}m", s.duration_ms / 60_000));
        }
        rows.push(("花费", v.join(" · ")));
    }
    if let Some(r) = crate::stats::roughest(&s) {
        rows.push((
            "最费劲",
            format!("{} 返工 {} 次{}", r.id, r.rework, if r.blocks > 0 { format!("、被挡 {} 次", r.blocks) } else { String::new() }),
        ));
    }
    for (k, v) in &rows {
        println!("  {}{}{}", c.dim(k), " ".repeat(8usize.saturating_sub(style::width(k))), style::truncate(v, text.saturating_sub(8)));
    }

    let show_cost = s.cost_usd > 0.0;
    let mut head: Vec<&str> = vec!["步骤", "id", "这一步做什么", "轮次", "返工", "文档"];
    let mut align = vec![
        style::Align::Right,
        style::Align::Left,
        style::Align::Left,
        style::Align::Right,
        style::Align::Right,
        style::Align::Left,
    ];
    if show_cost {
        head.push("花费");
        align.push(style::Align::Right);
    }
    head.push("结果");
    align.push(style::Align::Left);

    let body: Vec<Vec<String>> = s
        .todos
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let mut r = vec![
                (i + 1).to_string(),
                t.id.clone(),
                t.text.clone(),
                if t.rounds == 0 { "—".into() } else { t.rounds.to_string() },
                if t.rework == 0 { "—".into() } else { t.rework.to_string() },
                if t.status != "done" {
                    "—".into()
                } else if t.documented {
                    "有".into()
                } else {
                    "缺".into()
                },
            ];
            if show_cost {
                r.push(if t.cost_usd > 0.0 { format!("${:.2}", t.cost_usd) } else { "—".into() });
            }
            r.push(match (t.status.as_str(), t.first_try) {
                ("done", true) => "一次过",
                ("done", false) => "完成",
                ("deferred", _) => "已延后",
                ("blocked", _) => "等你回话",
                _ if t.rounds > 0 => "在做",
                _ => "没开始",
            }
            .to_string());
            r
        })
        .collect();

    println!();
    for line in style::table(&head, &body, &align, 2, text, &c) {
        println!("  {line}");
    }
    println!();
    println!("  {}  {}", c.dim("看细节"), c.bold("zloop log · zloop doc --all"));
    Ok(0)
}

#[allow(clippy::too_many_arguments)]
fn cmd_doc(
    root: &Path,
    path: &Path,
    todo: Option<String>,
    all: bool,
    last: Option<usize>,
    since: Option<String>,
    until: Option<String>,
    out: Option<PathBuf>,
) -> Result<i32> {
    let st = state::load(path)?;
    let when = |raw: Option<String>| -> Result<Option<chrono::DateTime<chrono::FixedOffset>>> {
        raw.map(|s| state::parse_when(&s)).transpose()
    };
    let range = log::Range {
        last,
        since: match when(since) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("doc: --since {e}");
                return Ok(2);
            }
        },
        until: match when(until) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("doc: --until {e}");
                return Ok(2);
            }
        },
    };
    if let (Some(s), Some(u)) = (range.since, range.until) {
        if s > u {
            eprintln!("doc: --since 比 --until 还晚，这个区间是空的");
            return Ok(2);
        }
    }
    let ids: Vec<String> = if all {
        st.todos.iter().map(|t| t.id.clone()).collect()
    } else {
        match todo {
            Some(id) => {
                if !st.todos.iter().any(|t| t.id == id) {
                    eprintln!("doc: unknown todo id {id:?}");
                    return Ok(2);
                }
                vec![id]
            }
            None => {
                eprintln!("doc: name a todo (`zloop doc t3`) or pass --all");
                return Ok(2);
            }
        }
    };
    let text = log::assemble(root, &st, &ids, &range);
    match out {
        Some(p) => {
            if let Some(parent) = p.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&p, &text)?;
            // 限了范围时没有轮次的 todo 整章不出，所以数正文里的章标题，别数 `ids`。
            let chapters = text.matches("\n## ").count();
            println!("wrote {} ({} 行, {} 条 todo)", p.display(), text.lines().count(), chapters);
        }
        None => print!("{text}"),
    }
    Ok(0)
}

/// Archive todos finished more than `keep_days` ago, together with their ticks.
fn cmd_compact(root: &Path, path: &Path, keep_days: i64) -> Result<i32> {
    let cutoff = state::now() - chrono::Duration::days(keep_days.max(0));
    let (moved_todos, moved_ticks, archive) = state::transaction(path, |st| {
        let old_ids: std::collections::HashSet<String> = st
            .todos
            .iter()
            .filter(|t| todo::is_terminal(&t.status))
            .filter(|t| {
                let stamp = t.done_at.as_deref().unwrap_or(&t.updated_at);
                state::parse_iso(stamp).map(|d| d < cutoff).unwrap_or(false)
            })
            .map(|t| t.id.clone())
            .collect();
        if old_ids.is_empty() {
            return Ok((0usize, 0usize, None));
        }
        let (gone_todos, kept_todos): (Vec<_>, Vec<_>) = st.todos.drain(..).partition(|t| old_ids.contains(&t.id));
        let (gone_ticks, kept_ticks): (Vec<_>, Vec<_>) =
            st.ticks.drain(..).partition(|t| t.todo.as_ref().map(|id| old_ids.contains(id)).unwrap_or(false));
        st.todos = kept_todos;
        st.ticks = kept_ticks;
        let dir = root.join(state::STATE_DIR).join("archive");
        std::fs::create_dir_all(&dir)?;
        let target = dir.join(format!("compact-{}.json", state::now_iso().replace([':', '+'], "")));
        let payload = serde_json::json!({"compacted_at": state::now_iso(), "keep_days": keep_days, "todos": gone_todos, "ticks": gone_ticks});
        std::fs::write(&target, serde_json::to_string_pretty(&payload)? + "\n")?;
        Ok((gone_todos.len(), gone_ticks.len(), Some(target)))
    })?;
    match archive {
        Some(p) => println!("compacted {moved_todos} todos and {moved_ticks} ticks → {}", p.display()),
        None => println!("nothing to compact (no done/deferred todos older than {keep_days} days)"),
    }
    Ok(0)
}

/// `zloop replan`：默认只把材料摆出来（只读）；`--apply` 才真的改计划。
fn cmd_replan(root: &Path, path: &Path, apply: bool, why: Option<String>) -> Result<i32> {
    if !apply {
        print!("{}", crate::replan::packet(&state::load(path)?));
        return Ok(0);
    }
    // 无头轮次里默认**不许**改计划。这条红线以前只写在提示词里，而这整个功能的前提就是
    // "提示词管不住模型"——回归测试里那个假宿主真的抗命跑了一次 `--apply` 并且成功了。
    // runner 只在 `--auto-replan` 打开、且正是重估那一轮时，才给子进程放行这个变量。
    if std::env::var_os("ZLOOP_RUNNER").is_some() && std::env::var_os(crate::runner::AUTO_REPLAN_ENV).is_none() {
        eprintln!(
            "replan --apply 拒绝了这次改动：\n\
             护栏「无头默认不改计划」：这一轮由 runner 驱动，但没开 --auto-replan（或这不是重估轮次）。\n\
             把建议写进输出就行，人会看到；真要让它自己改，起 runner 时加 --auto-replan。"
        );
        return Ok(2);
    }
    let mut raw = String::new();
    std::io::stdin().read_to_string(&mut raw)?;
    let items = todo::parse_plan(&raw, todo::DEFAULT_PRIORITY);
    let why = why.unwrap_or_default();
    let out = state::transaction(path, |st| {
        Ok(match crate::replan::apply(st, path, &items, &why) {
            Ok(a) => {
                let note = format!(
                    "重排：换掉 {} 条、新排 {} 条、保留 {} 条 · {why}",
                    a.dropped.len(),
                    a.added.len(),
                    a.kept.len()
                );
                let who = session::detect();
                let _ = tick::record(st, tick::REPLAN, None, &style::truncate(&note, 300), &who);
                Ok(a)
            }
            Err(e) => Err(e),
        })
    })?;
    let a = match out {
        Ok(a) => a,
        Err(e) => {
            eprintln!("replan --apply 拒绝了这次改动：\n{e}");
            return Ok(2);
        }
    };
    println!(
        "replan applied: 换掉 {} 条、新排 {} 条、保留 {} 条（已完成和等你回话的没动）",
        a.dropped.len(),
        a.added.len(),
        a.kept.len()
    );
    if !a.dropped.is_empty() {
        println!("  换掉：{}", a.dropped.join(" "));
    }
    println!("  新排：{}", a.added.join(" "));
    println!("  旧账本备份在 {}", a.backup.display());
    let _ = root;
    if let Some(h) = crate::replan::hint(&state::load(path)?) {
        println!("\n⚠ 还有信号没消：{h}");
    }
    Ok(0)
}

fn cmd_hook_stop(root: &Path, path: &Path) -> Result<i32> {
    let mut buf = String::new();
    let _ = std::io::stdin().read_to_string(&mut buf); // payload is informational only
    // Under the runner each `claude -p` call is exactly one round; never chain todos there.
    if std::env::var_os("ZLOOP_RUNNER").is_some() {
        return Ok(0);
    }
    // 无头 runner 在跑的时候，别催**别的**会话去抢它手上的活：源码文件没有锁，
    // 两个 agent 同时改一批文件就是互相覆盖（#14，2026-08-29 那次 4 小时长跑里
    // 每一轮都在发生）。
    //
    // 这里不能用 `tick::held_by_other`：那个函数对 runner 是放行的，而且**必须**放行，
    // 否则 runner 会把自家的 `claude -p` 子进程挡在门外（原因见 `tick::held_by_other`
    // 的注释）。所以换个判据——进程还在不在。
    //
    // 两道闸不会打架：runner 自己的子进程带着 `ZLOOP_RUNNER`，上面那一步就返回了，
    // 走不到这里。也不只挡「正在跑某一轮」的那几分钟——runner 在轮次之间睡觉时同样闭嘴，
    // 它醒来就会接着领活，这会儿放交互会话进去只是换个时刻撞车。
    if crate::daemon::running(root).is_some() {
        return Ok(0);
    }
    let st = match state::load(path) {
        Ok(s) => s,
        Err(_) => return Ok(0),
    };
    // 另一个交互会话正拿着这一轮：同理闭嘴。`next` 早就走这道闸了（它会返回
    // `held_by_other` 而不是派活），但 hook 一直没走，于是「`next` 说不给你」和
    // 「hook 催你去做」同时成立——人照着 hook 的话敲下去就撞上了。
    //
    // 这道闸对 runner 无效（见上面那段），两条各挡各的一半。
    let who = session::detect();
    if tick::held_by_other(&st, &who, state::now()).is_some() {
        return Ok(0);
    }
    let d = tick::decide(&st, state::now());
    if d.should_run {
        let reason = format!(
            "{}\n\n当前 todo：{}",
            prompt::heartbeat(&st, "claude", root)?,
            d.todo.as_ref().map(fmt_todo).unwrap_or_default()
        );
        print_json(&serde_json::json!({"decision": "block", "reason": reason}));
    }
    Ok(0)
}

/// Map an error to the exit code Python used: 1 for state problems, 2 for everything else.
pub fn exit_code_for(err: &anyhow::Error) -> i32 {
    if err.downcast_ref::<StateError>().is_some() {
        1
    } else {
        2
    }
}
