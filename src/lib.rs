//! zloop — minimal goal-loop scheduler for Claude Code and Codex.
//!
//! One JSON file (`.zloop/state.json`), a dozen commands, zero runtime deps.

pub mod cli;
pub mod context;
pub mod hosts;
pub mod log;
pub mod prompt;
pub mod runner;
pub mod session;
pub mod state;
pub mod tick;
pub mod todo;
