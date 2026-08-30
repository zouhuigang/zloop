#![allow(dead_code)]

use chrono::{DateTime, FixedOffset};
use std::process::Command;
use zloop::session::{Host, HostSession};
use zloop::state::{self, State, Tick};
use zloop::tick;
use zloop::todo;

/// Tests must not inherit the ambient session: a `cargo test` run started *inside* a host
/// session (or inside `zloop run`, which sets `ZLOOP_RUNNER=1` on its children) would leak
/// that identity into every spawned `zloop` and quietly change what it does — e.g.
/// `hook-stop` takes its pass-through branch and prints nothing. Each test decides its own
/// environment; anything meaningful is set explicitly by the test itself.
pub fn scrub_ambient_env(cmd: &mut Command) -> &mut Command {
    cmd.env_remove("CLAUDE_CODE_SESSION_ID")
        .env_remove("CLAUDECODE")
        .env_remove("CODEX_THREAD_ID")
        .env_remove("ZLOOP_RUNNER")
}

pub fn now_utc() -> DateTime<FixedOffset> {
    DateTime::parse_from_rfc3339("2026-08-26T12:00:00+00:00").unwrap()
}

pub fn cli_who() -> HostSession {
    HostSession { host: Host::Cli, session: None }
}

pub fn fresh(lines: &[&str]) -> State {
    let mut st = state::default_state("goal", "g");
    let items = todo::parse_plan(&lines.join("\n"), todo::DEFAULT_PRIORITY);
    todo::add(&mut st, &items, false);
    st
}

pub fn tick_at(st: &mut State, outcome: &str, todo_id: Option<&str>, at: Option<DateTime<FixedOffset>>) -> Tick {
    let t = tick::record(st, outcome, todo_id, "", &cli_who()).unwrap();
    if let Some(at) = at {
        let last = st.ticks.last_mut().unwrap();
        last.at = state::format_iso(&at);
        return last.clone();
    }
    t
}

pub fn done(st: &mut State, id: &str) {
    tick::apply_done(st, id, "done", "", None, None, &cli_who()).unwrap();
}

pub fn outcome(st: &mut State, id: &str, outcome: &str, note: &str) {
    tick::apply_done(st, id, outcome, note, None, None, &cli_who()).unwrap();
}
