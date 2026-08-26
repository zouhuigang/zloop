#![allow(dead_code)]

use chrono::{DateTime, FixedOffset};
use zloop::session::{Host, HostSession};
use zloop::state::{self, State, Tick};
use zloop::tick;
use zloop::todo;

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
