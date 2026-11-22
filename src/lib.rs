//! poo: find and safely free disk space eaten by dev builds, worktrees, caches,
//! simulators and the rest. Every piece of knowledge lives in mods (`rules/core`).

pub mod agent;
pub mod ask;
pub mod backup;
pub mod bulk;
pub mod catalog;
pub mod clean;
pub mod config;
pub mod contrib;
pub mod delete;
pub mod fsevents;
pub mod fsutil;
pub mod growth;
pub mod hooks;
pub mod mcp;
pub mod ops;
pub mod render;
pub mod model;
pub mod plan;
pub mod procs;
pub mod providers;
pub mod scan;
pub mod schedule;
pub mod telemetry;
pub mod tui;
pub mod update;
pub mod util;
pub mod watch;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const REPO: &str = "morajabi/poo";
