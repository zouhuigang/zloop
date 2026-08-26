//! Command line: init · plan · next · done · edit · status · heartbeat · install
//!               · sessions · context · log · run   (+ hook-stop for Claude Code)

use crate::session::{self, Host};
use crate::state::{self, StateError};
use crate::{context, hosts, log, prompt, runner, tick, todo};
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
    /// Headless runner: drive claude -p / codex exec round after round
    Run {
        #[arg(long, value_parser = ["claude", "codex"])]
        host: String,
        /// Stop after this many rounds (0 = until the scheduler stops)
        #[arg(long = "max-rounds", default_value_t = 0)]
        max_rounds: u32,
        /// Treat interval minutes as seconds (for demos)
        #[arg(long)]
        fast: bool,
        /// Bypass host permission prompts (claude --dangerously-skip-permissions / codex danger-full-access)
        #[arg(long = "allow-all")]
        allow_all: bool,
        /// Do not resume the previous host session; start each round fresh
        #[arg(long = "no-resume")]
        no_resume: bool,
    },
    /// (internal) Claude Code Stop-hook entry; reads hook JSON on stdin
    HookStop,
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
        Cmd::Next { json, peek } => cmd_next(&path, json, peek),
        Cmd::Done { id, note, outcome, block, next, evidence } => {
            cmd_done(&root, &path, &id, note, &outcome, block, next, evidence)
        }
        Cmd::Edit { id, text, status, priority, blocked_by } => cmd_edit(&path, &id, text, status, priority, blocked_by),
        Cmd::Status { json, md } => cmd_status(&root, &path, json, md),
        Cmd::Heartbeat { host } => {
            let st = state::load(&path)?;
            println!("{}", prompt::heartbeat(&st, &host, &root)?);
            Ok(0)
        }
        Cmd::Install { claude, codex, claude_stop_hook } => {
            if !(claude || codex || claude_stop_hook) {
                eprintln!("install: choose --claude, --codex and/or --claude-stop-hook");
                return Ok(2);
            }
            let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("no home directory"))?;
            for (p, changed) in hosts::install(claude, codex, claude_stop_hook, &home)? {
                println!("{}{}", if changed { "wrote  " } else { "kept   " }, p.display());
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
        Cmd::Run { host, max_rounds, fast, allow_all, no_resume } => {
            let host = Host::parse(&host).unwrap_or(Host::Claude);
            runner::run(&root, runner::Options { host, max_rounds, fast, allow_all, resume: !no_resume })
        }
        Cmd::HookStop => cmd_hook_stop(&root, &path),
    }
}

fn cmd_init(dir: &Option<PathBuf>, goal: &str, force: bool) -> Result<i32> {
    let root = dir.clone().unwrap_or_else(|| PathBuf::from("."));
    let root = root.canonicalize().unwrap_or(root);
    let path = state::state_path(&root);
    if path.exists() && !force {
        let cur = state::load(&path)?;
        eprintln!("already initialized ({}): {}\nuse --force to replace", cur.goal.status, cur.goal.text);
        return Ok(1);
    }
    let id = root.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "goal".into());
    let mut st = state::default_state(goal.trim(), &id);
    state::locked(&path, std::time::Duration::from_secs(5), || state::save(&path, &mut st))?;
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
        println!("{}", fmt_todo(&t));
    }
    Ok(0)
}

fn cmd_next(path: &Path, json: bool, peek: bool) -> Result<i32> {
    let who = session::detect();
    let (decision, payload) = state::transaction(path, |st| {
        let d = tick::decide(st, state::now());
        if !d.should_run && !peek {
            tick::record(st, "noop", None, &d.reason, &who)?;
        }
        let payload = tick::to_json(&d, st);
        Ok((d, payload))
    })?;
    if json {
        print_json(&payload);
    } else if decision.should_run {
        let t = decision.todo.as_ref().unwrap();
        println!("RUN  {}", fmt_todo(t));
        println!("     writeback: {}", payload["writeback"].as_str().unwrap_or(""));
        println!("     interval: {} min · remaining {}", payload["interval_min"], payload["remaining"]);
    } else {
        let interval = match decision.interval_min {
            None => "stop".to_string(),
            Some(m) => format!("{m} min"),
        };
        println!("WAIT ({}) remaining {} · retry in {}", decision.reason, payload["remaining"], interval);
    }
    Ok(0)
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
    evidence: Option<String>,
) -> Result<i32> {
    let who = session::detect();
    let evidence = log::resolve_evidence(evidence.as_deref())?;
    let note = note.unwrap_or_default();
    let result = state::transaction(path, |st| {
        let (mut tick_rec, idx) =
            match tick::apply_done(st, id, outcome, &note, block.as_deref(), next.as_deref(), &who) {
                Ok(v) => v,
                Err(e) => return Ok(Err(e)),
            };
        let todo_snapshot = st.todos[idx].clone();
        let rel = log::write(root, st, &tick_rec, &todo_snapshot, evidence.as_deref())?;
        if let Some(last) = st.ticks.last_mut() {
            last.log = Some(rel.clone());
        }
        tick_rec.log = Some(rel);
        let d = tick::decide(st, state::now());
        Ok(Ok((tick_rec, d, todo::remaining(st))))
    })?;
    let (tick_rec, decision, remaining) = match result {
        Ok(v) => v,
        Err(e) => {
            eprintln!("done: {e}");
            return Ok(2);
        }
    };
    let note = if tick_rec.note.is_empty() { String::new() } else { format!(": {}", tick_rec.note) };
    println!("{id} {}{}", tick_rec.outcome, note);
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
) -> Result<i32> {
    if text.is_none() && status.is_none() && priority.is_none() && blocked_by.is_none() {
        eprintln!("edit: nothing to change (use --text/--status/--priority/--blocked-by)");
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
    println!("state: {}", path.display());
    let head = if d.should_run {
        format!("RUN {}", d.todo.as_ref().map(|t| t.id.as_str()).unwrap_or("-"))
    } else {
        format!("WAIT {}", d.reason)
    };
    println!("round {} · remaining {} · {}", tick::current_round(&st.ticks), todo::remaining(&st), head);
    for i in todo::open_ordered(&st) {
        let t = &st.todos[i];
        let deps = if t.blocked_by.is_empty() { String::new() } else { format!(" ⏳{}", t.blocked_by.join(",")) };
        println!("  {} {}{}", prompt::checkbox(&t.status), fmt_todo(t), deps);
    }
    let done = st.todos.iter().filter(|t| todo::is_terminal(&t.status)).count();
    if done > 0 {
        println!("  … {done} done/deferred");
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
    for f in files {
        let rel = f.strip_prefix(root).unwrap_or(&f);
        println!("{}  {}", rel.display(), log::first_line(&f));
    }
    Ok(0)
}

fn cmd_hook_stop(root: &Path, path: &Path) -> Result<i32> {
    let mut buf = String::new();
    let _ = std::io::stdin().read_to_string(&mut buf); // payload is informational only
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
