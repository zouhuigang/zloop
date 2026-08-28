//! Command line: init · plan · next · done · edit · status · heartbeat · install
//!               · sessions · context · log · run   (+ hook-stop for Claude Code)

use crate::session::{self, Host};
use crate::state::{self, StateError};
use crate::{context, daemon, hosts, log, notify, phase, prompt, runner, style, tick, todo};
use anyhow::Result;
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
        goal: String,
        /// Replace an existing state file
        #[arg(long)]
        force: bool,
    },
    /// Append ordered todos ([P0]/[P1]/[P2] text per line)
    Plan {
        /// One todo line; repeatable
        #[arg(long, value_name = "LINE")]
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
        #[arg(long)]
        note: Option<String>,
        #[arg(long, default_value = "done", value_parser = ["done", "progress", "fail"])]
        outcome: String,
        /// Mark the todo blocked on the user
        #[arg(long, value_name = "QUESTION")]
        block: Option<String>,
        /// Insert a successor todo right after this one
        #[arg(long, value_name = "LINE")]
        next: Option<String>,
        /// Details for the log file: literal text or @path
        #[arg(long, value_name = "TEXT|@FILE")]
        evidence: Option<String>,
        /// 实现思路：怎么做的、为什么这么做（literal text or @path）。outcome=done 时必填
        #[arg(long, value_name = "TEXT|@FILE")]
        approach: Option<String>,
        /// 关键决策 / 取舍，可重复
        #[arg(long, value_name = "TEXT")]
        decision: Vec<String>,
        /// 遇到的坑与结论，可重复
        #[arg(long, value_name = "TEXT")]
        pitfall: Vec<String>,
        /// 这一轮不写技术文档（绕过 policy.require_doc）
        #[arg(long = "no-doc")]
        no_doc: bool,
    },
    /// Change a todo's text, status, priority or dependencies
    Edit {
        id: String,
        #[arg(long)]
        text: Option<String>,
        #[arg(long, value_parser = todo::STATUSES)]
        status: Option<String>,
        #[arg(long, value_parser = clap::value_parser!(u8).range(0..=4))]
        priority: Option<u8>,
        /// Comma-separated todo ids or 'user'; '' clears
        #[arg(long = "blocked-by", value_name = "IDS")]
        blocked_by: Option<String>,
        /// How to verify the todo is done; '' clears
        #[arg(long)]
        acceptance: Option<String>,
    },
    /// Send a notification through policy.notify_url / notify_cmd (use it to test your webhook)
    Notify {
        /// Message text (default: a test message)
        text: Option<String>,
    },
    /// Write a lesson to .zloop/NOTES.md; the newest few appear in `zloop context`
    Remember {
        text: String,
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
    Rm { needle: String },
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

/// Returns the process exit code.
pub fn run(cli: Cli) -> Result<i32> {
    let root = root_of(&cli.dir);
    let path = state::state_path(&root);
    match cli.cmd {
        Cmd::Init { goal, force } => cmd_init(&cli.dir, &goal, force),
        Cmd::Plan { add, file, replace, from_loopx } => cmd_plan(&path, add, file, replace, from_loopx),
        Cmd::Next { json, peek } => cmd_next(&root, &path, json, peek),
        Cmd::Done { id, note, outcome, block, next, evidence, approach, decision, pitfall, no_doc } => {
            cmd_done(&root, &path, &id, note, &outcome, block, next, DoneDoc { evidence, approach, decision, pitfall, no_doc })
        }
        Cmd::Doc { todo, all, out } => cmd_doc(&root, &path, todo, all, out),
        Cmd::Edit { id, text, status, priority, blocked_by, acceptance } => {
            cmd_edit(&path, &id, text, status, priority, blocked_by, acceptance)
        }
        Cmd::Remember { text } => {
            state::load(&path)?;
            let p = crate::notes::remember(&root, &text)?;
            println!("remembered → {}", p.display());
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
        Cmd::Install { claude, codex, claude_stop_hook, sudoers } => {
            if !(claude || codex || claude_stop_hook || sudoers) {
                eprintln!("install: choose --claude, --codex, --claude-stop-hook and/or --sudoers");
                return Ok(2);
            }
            let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("no home directory"))?;
            for (p, changed) in hosts::install(claude, codex, claude_stop_hook, &home)? {
                println!("{}{}", if changed { "wrote  " } else { "kept   " }, p.display());
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
            Ok(0)
        }
        Cmd::Log { todo, last, show } => cmd_log(&root, todo, last, show),
        Cmd::Goal { cmd } => cmd_goal(&root, cmd.unwrap_or(GoalCmd::List { json: false }), style::Style::detect(cli.no_color)),
        Cmd::Run(args) => runner::run(&root, args.options()),
        Cmd::Start(args) => {
            state::load(&path)?; // fail early with the usual "no zloop state" message
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
    state::locked(&path, std::time::Duration::from_secs(5), || state::save(&path, &mut st))?;
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
    }
    Ok(0)
}

/// The documentation half of `zloop done`.
pub struct DoneDoc {
    pub evidence: Option<String>,
    pub approach: Option<String>,
    pub decision: Vec<String>,
    pub pitfall: Vec<String>,
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
) -> Result<i32> {
    let who = session::detect();

    // Every finished todo must explain itself. Checked before any state write so a rejected
    // call changes nothing and can simply be retried with the missing text.
    let policy_requires = state::load(path)?.policy.require_doc;
    let finishing = outcome == "done" && block.is_none();
    let has_approach = input.approach.as_deref().map(|s| !s.trim().is_empty()).unwrap_or(false);
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
        if let Some(last) = st.ticks.last_mut() {
            last.log = Some(rel.clone());
            last.documented = documented;
        }
        tick_rec.log = Some(rel);
        tick_rec.documented = documented;
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
        let open = !todo::open_ordered(st).is_empty();
        if st.goal.status == "done" && open {
            st.goal.status = "active".into();
        } else if !open {
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
            let st_w = rows.iter().map(|r| style::width(goal_status_zh(&r.status))).max().unwrap_or(6);
            let pg_w = rows.iter().map(|r| format!("{}/{}", r.done, r.total).len()).max().unwrap_or(3);
            println!();
            println!("  {}", c.dim(&format!("共 {} 个目标 · ▸ 是当前那个", rows.len())));
            for r in &rows {
                let progress = format!("{}/{}", r.done, r.total);
                let head = format!(
                    "{} {}{}  {}{}  {}{}  {}",
                    if r.current { "▸" } else { " " },
                    r.id,
                    " ".repeat(id_w - style::width(&r.id)),
                    goal_status_zh(&r.status),
                    " ".repeat(st_w - style::width(goal_status_zh(&r.status))),
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
            println!("  {}  {}", c.dim("切换"), c.bold("zloop goal switch <id 或目标里的片段>"));
            println!("  {}  {}", c.dim("新建"), c.bold("zloop goal new \"新目标\""));
            println!();
            Ok(0)
        }
        GoalCmd::New { text, id, force } => {
            crate::goals::ensure_idle(root, force)?;
            let (parked, cur) = crate::goals::create(root, &text, id.as_deref())?;
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
            }
            println!("新目标 [{}] {}", cur.id, cur.text);
            println!("下一步：{}", c.bold("zloop plan   # 每行一条 `[P0] 文本`，从 stdin 读"));
            Ok(0)
        }
        GoalCmd::Switch { needle, force } => {
            crate::goals::ensure_idle(root, force)?;
            let sw = crate::goals::switch(root, &needle)?;
            match sw.parked {
                Some(p) => println!(
                    "停放「{}」[{}] · 切回：{}",
                    style::truncate(&p.text, 30),
                    p.id,
                    c.bold(&format!("zloop goal switch {}", p.id))
                ),
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
        GoalCmd::Rm { needle } => {
            let (row, target) = crate::goals::archive(root, &needle)?;
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
    let (icon, word, code): (&str, &str, &str) = match () {
        // 刚开的目标还没有待办：说"待规划"，别说"全部完成"（decide 对空清单返回 all_done）
        _ if total == 0 => ("◦", "待规划", "34"),
        _ if st.goal.status == "done" => ("✅", "完成", "32"),
        _ if st.goal.status == "paused" => ("⏸", "已暂停", "33"),
        _ if matches!(d.reason.as_str(), "fail_streak" | "progress_streak" | "budget") => ("⛔", "已停", "31"),
        _ if matches!(d.reason.as_str(), "user_gate" | "blocked") => ("⏳", "等你决定", "33"),
        _ if ph.kind == "executing" => ("🔄", "执行中", "36"),
        _ if ph.kind == "sleeping" => ("💤", "休眠中", "34"),
        _ if d.reason == "throttled" => ("⏱", "限流中", "33"),
        _ if d.should_run => ("▶", "就绪", "34"),
        _ => ("•", "空闲", "2"),
    };
    let pct = if total > 0 { finished * 100 / total } else { 0 };
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
    let bar = if w >= 70 { format!("{} ", style::bar(finished, total, 16, c)) } else { String::new() };
    println!();
    println!(
        "  {icon} {head}{bar}{}  {}",
        c.bold(&format!("{pct}%")),
        // Every recorded round, failures included — `tick::current_round` counts only the
        // productive ones, which reads as "0 轮" right after three failures.
        // 「几条待办」交给下面的步骤清单说，标题只留轮数和花费。
        c.dim(&format!("跑了 {} 轮{money}", st.ticks.iter().filter(|t| t.outcome != "noop").count()))
    );
    println!("  {}    {}", c.dim("目标"), style::truncate(&st.goal.text, text.saturating_sub(8)));

    // ---- 步骤清单：做过的、正在做的、还没做的，一张勾选表 ----
    // 顺序用 state.todos 的原始顺序（= 步骤顺序，也对应 t1/t2/t3），执行顺序由「下一个」标出来，
    // 因为 `next` 是按优先级挑的，不一定是清单的下一行。
    const MAX_ROWS: usize = 15;
    if !st.todos.is_empty() {
        let next_id = d.todo.as_ref().map(|t| t.id.clone());
        let step_of: std::collections::HashMap<&str, usize> =
            st.todos.iter().enumerate().map(|(i, t)| (t.id.as_str(), i + 1)).collect();

        struct Row {
            n: usize,
            text: String,
            /// 右栏：id（做完的不用）+ 图标 + 状态词
            meta: String,
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
            let (icon, word, paint) = if t.status == "done" {
                ("✅", String::new(), 1)
            } else if t.status == "deferred" {
                ("⏭", "已延后".into(), 0)
            } else if running_now {
                ("🔄", "执行中".into(), 2)
            } else if waiting_on_you {
                ("!", "等你回话".into(), 3)
            } else if let Some(dep) = pending_dep {
                let label = match step_of.get(dep.as_str()) {
                    Some(n) => format!("等第 {n} 步"),
                    None => format!("等 {dep}"),
                };
                ("⏳", label, 0)
            } else if is_next {
                ("▶", "下一个".into(), 2)
            } else {
                ("○", "排队中".into(), 0)
            };
            // 做完的那些不需要 id——需要敲命令的才需要。
            let meta = match (t.status.as_str(), word.is_empty()) {
                ("done", _) => icon.to_string(),
                (_, true) => format!("{} {icon}", t.id),
                (_, false) => format!("{} {icon} {word}", t.id),
            };
            let mut sub = Vec::new();
            if waiting_on_you && !t.note.is_empty() {
                sub.push(("↳".into(), t.note.clone(), 3));
            }
            if waiting_on_you {
                sub.push(("答完敲".into(), format!("zloop edit {} --status open", t.id), 4));
            }
            if let Some(a) = &t.acceptance {
                if t.status != "done" {
                    sub.push(("验收：".into(), a.clone(), 0));
                }
            }
            rows.push(Row { n: i + 1, text: t.text.clone(), meta, finished: todo::is_terminal(&t.status), paint, sub });
        }

        // 太长就折叠：没做完的全留着，前面垫 3 步做过的当上下文，其余收成一行。
        let first_open = rows.iter().position(|r| !r.finished).unwrap_or(rows.len());
        let dropped = if rows.len() <= MAX_ROWS { 0 } else { first_open.saturating_sub(3).min(rows.len().saturating_sub(1)) };
        let mut shown = &rows[dropped..];
        let mut tail = 0;
        if shown.len() > MAX_ROWS {
            tail = shown.len() - MAX_ROWS;
            shown = &shown[..MAX_ROWS];
        }
        let meta_w = shown.iter().map(|r| style::width(&r.meta)).max().unwrap_or(0);
        let num_w = shown.iter().map(|r| r.n.to_string().len()).max().unwrap_or(1);
        let longest = shown.iter().map(|r| style::width(&r.text)).max().unwrap_or(0);
        // 文本列按实际最长的一条收窄（省掉一大片空白），和右栏之间留 4 列。
        let gap = 4;
        let text_w = text.saturating_sub(num_w + 2 + meta_w + gap).max(12).min(longest.max(12));

        println!();
        println!(
            "  {}{}{}",
            c.dim("步骤"),
            " ".repeat(4),
            c.bold(&format!("{finished}/{total} 完成"))
        );
        if dropped > 0 {
            println!("  {}", c.dim(&format!("…  前 {dropped} 步已收起 · zloop log 里有它们的记录")));
        }
        for r in shown {
            let body = style::truncate(&r.text, text_w);
            let pad = " ".repeat(text_w.saturating_sub(style::width(&body)) + gap + meta_w.saturating_sub(style::width(&r.meta)));
            let (body, meta) = match r.paint {
                1 => (c.dim(&body), c.green(&r.meta)),
                2 => (c.bold(&body), c.cyan(&r.meta)),
                3 => (c.yellow(&body), c.yellow(&r.meta)),
                _ => (c.dim(&body), c.dim(&r.meta)),
            };
            println!("  {:>num_w$}. {body}{pad}{meta}", r.n);
            for (prefix, content, paint) in &r.sub {
                let indent = " ".repeat(num_w + 4);
                let room = text.saturating_sub(num_w + 4 + style::width(prefix) + 1);
                // 命令不裁：半条命令比折行更糟。
                let content = if *paint == 4 { content.clone() } else { style::truncate(content, room) };
                let line = match paint {
                    3 => format!("{} {}", c.yellow(prefix), c.yellow(&content)),
                    4 => format!("{} {}", c.dim(prefix), c.bold(&content)),
                    _ => c.dim(&format!("{prefix}{content}")),
                };
                println!("  {indent}{line}");
            }
        }
        if tail > 0 {
            println!("  {}", c.dim(&format!("…  后面还有 {tail} 步 · zloop status --json 看全部")));
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
    } else if st.goal.status == "done" || d.reason == "all_done" {
        format!("{total} 条待办全部完成，目标结束")
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
    rows.push((
        "后台",
        match running {
            Some(pid) => c.dim(&style::truncate(&format!("runner 在跑（pid {pid}）· 日志 .zloop/runner/console.log"), val)),
            None => c.dim("没有 runner 在跑"),
        },
    ));
    if let Some((s, warn)) = crate::awake::brief() {
        let s = style::truncate(&s, val.saturating_sub(2));
        rows.push(("睡眠", if warn { c.yellow(&format!("⚠ {s}")) } else { c.dim(&s) }));
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
    if let Some(cmd) = sessions.last().and_then(|s| s.resume.as_deref()) {
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
    let files = log::entries(root, todo.as_deref(), last)?;
    if files.is_empty() {
        println!("no logs yet (written by `zloop done`)");
        return Ok(0);
    }
    let mut undocumented = 0;
    for f in &files {
        let rel = f.strip_prefix(root).unwrap_or(f);
        let expects_doc = f.file_name().map(|n| n.to_string_lossy().ends_with("-done.md")).unwrap_or(false);
        let mark = if !expects_doc || log::file_is_documented(f) {
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
    Ok(0)
}

fn cmd_doc(root: &Path, path: &Path, todo: Option<String>, all: bool, out: Option<PathBuf>) -> Result<i32> {
    let st = state::load(path)?;
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
    let text = log::assemble(root, &st, &ids);
    match out {
        Some(p) => {
            if let Some(parent) = p.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&p, &text)?;
            println!("wrote {} ({} 行, {} 条 todo)", p.display(), text.lines().count(), ids.len());
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

fn cmd_hook_stop(root: &Path, path: &Path) -> Result<i32> {
    let mut buf = String::new();
    let _ = std::io::stdin().read_to_string(&mut buf); // payload is informational only
    // Under the runner each `claude -p` call is exactly one round; never chain todos there.
    if std::env::var_os("ZLOOP_RUNNER").is_some() {
        return Ok(0);
    }
    let st = match state::load(path) {
        Ok(s) => s,
        Err(_) => return Ok(0),
    };
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
