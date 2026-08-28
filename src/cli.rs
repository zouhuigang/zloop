//! Command line: init · plan · next · done · edit · status · heartbeat · install
//!               · sessions · context · log · run   (+ hook-stop for Claude Code)

use crate::session::{self, Host};
use crate::state::{self, StateError};
use crate::{context, daemon, hosts, log, notify, phase, prompt, runner, tick, todo};
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
    /// Start the runner in the background (detached; log in .zloop/runner/console.log)
    Start(RunArgs),
    /// Stop the background runner
    Stop,
    /// Run the runner in the foreground: drive claude -p / codex exec round after round
    Run(RunArgs),
    /// (internal) Claude Code Stop-hook entry; reads hook JSON on stdin
    HookStop,
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
        Cmd::Status { json, md } => cmd_status(&root, &path, json, md),
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
    let id = root.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "goal".into());
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
        let documented = doc.is_complete();
        if let Some(last) = st.ticks.last_mut() {
            last.log = Some(rel.clone());
            last.documented = Some(documented);
        }
        tick_rec.log = Some(rel);
        tick_rec.documented = Some(documented);
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

fn cmd_status(root: &Path, path: &Path, json: bool, md: bool) -> Result<i32> {
    let st = state::load(path)?;
    if json {
        print_json(&serde_json::to_value(&st)?);
        return Ok(0);
    }
    if md {
        print!("{}", prompt::render_md(&st, root, 10));
        return Ok(0);
    }
    let d = tick::decide(&st, state::now());
    println!("goal ({}): {}", st.goal.status, st.goal.text);
    println!("phase: {}", phase::compute(&st, root, state::now()).summary);
    match daemon::running(root) {
        Some(pid) => println!("runner: running in background (pid {pid}) · log {}", daemon::log_path(root).display()),
        None => println!("runner: not running · start with `zloop start`"),
    }
    if crate::awake::supported() {
        println!("{}", crate::awake::describe());
    }
    println!("state: {}", path.display());
    let head = if d.should_run {
        format!("RUN {}", d.todo.as_ref().map(|t| t.id.as_str()).unwrap_or("-"))
    } else {
        format!("WAIT {}", d.reason)
    };
    println!("round {} · remaining {} · {}", tick::current_round(&st.ticks), todo::remaining(&st), head);
    let spent = tick::spent_usd(&st.ticks);
    if spent > 0.0 || st.policy.max_total_usd > 0.0 {
        println!(
            "spent: ${spent:.2}{}",
            if st.policy.max_total_usd > 0.0 { format!(" / max ${:.2}", st.policy.max_total_usd) } else { " (no cap)".into() }
        );
    }
    for i in todo::open_ordered(&st) {
        let t = &st.todos[i];
        let deps = if t.blocked_by.is_empty() { String::new() } else { format!(" ⏳{}", t.blocked_by.join(",")) };
        let acc = t.acceptance.as_deref().map(|a| format!("\n      验收：{}", a)).unwrap_or_default();
        println!("  {} {}{}{}", prompt::checkbox(&t.status), fmt_todo(t), deps, acc);
    }
    let done = st.todos.iter().filter(|t| todo::is_terminal(&t.status)).count();
    if done > 0 {
        println!("  … {done} done/deferred");
    }
    let undocumented = st.ticks.iter().filter(|t| t.documented == Some(false)).count();
    if undocumented > 0 {
        println!("docs: {undocumented} 轮只有结果记录、没有实现思路（`zloop log` 里带 ⚠）");
    }
    let sessions = session::summarize(&st, root);
    if let Some(last) = sessions.last() {
        if let Some(cmd) = &last.resume {
            println!("last session: {} · `{}`", last.host, cmd);
        }
    }
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
        let mark = if log::file_is_documented(f) {
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
