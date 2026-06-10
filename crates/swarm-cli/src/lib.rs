//! `swarm-cli` — CLI dispatch layer. Populated in P5-S5.
//!
//! Contains all user-facing command handlers plus the top-level `run()` /
//! `run_dispatch()` entry points. Everything lives here so `agent-swarm`
//! can be an ~8-line shim that calls `swarm_cli::run()`.
//!
//! DAG position: swarm-cli (this crate) ← [[bin]] agent-swarm

mod cli;
mod cli_commands;
mod cli_read_commands;
pub mod package_repo;
mod provider_commands;
pub mod routing_repo;
pub mod scaffold;
pub mod service;

pub use service::SwarmService;

use swarm_kernel::args::Args;

pub fn run() -> Result<i32, String> {
    SwarmService::new().run()
}

pub fn run_dispatch(args: Args) -> Result<i32, String> {
    SwarmService::new().run_dispatch(args)
}
