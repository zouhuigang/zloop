//! zloop — minimal goal-loop scheduler for Claude Code and Codex.
//!
//! One JSON file (`.zloop/state.json`), a dozen commands, one static binary.

pub mod awake;
pub mod cli;
pub mod context;
pub mod daemon;
pub mod doctor;
pub mod goals;
pub mod hosts;
pub mod log;
pub mod notes;
pub mod notify;
pub mod phase;
pub mod prompt;
pub mod reflect;
pub mod replan;
pub mod runner;
pub mod session;
pub mod state;
pub mod stats;
pub mod style;
pub mod tick;
pub mod todo;
