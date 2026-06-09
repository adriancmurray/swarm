//! Platform process-liveness helpers — inlined from agent-swarm's `process.rs`.
//!
//! `process_is_alive` shells out to `/bin/kill -0` (unix) or `tasklist` (windows).
//! Kept as a private crate module so swarm-store does NOT depend on agent-swarm.
//!
//! # Sync note
//!
//! This is a verbatim copy of the `process_is_alive` body from
//! `tools/agent-swarm/rust/src/process.rs`. agent-swarm's copy stays in place
//! (it has 6 other callers in staying modules). If the implementation changes,
//! update both copies. P5-S5 will consolidate these into swarm-contracts.

use std::process::{Command, Stdio};

#[cfg(unix)]
pub(crate) fn process_is_alive(pid: u32) -> bool {
    Command::new("/bin/kill")
        .arg("-0")
        .arg(pid.to_string())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

#[cfg(not(unix))]
pub(crate) fn process_is_alive(pid: u32) -> bool {
    Command::new("tasklist")
        .arg("/FI")
        .arg(format!("PID eq {pid}"))
        .output()
        .map(|output| String::from_utf8_lossy(&output.stdout).contains(&pid.to_string()))
        .unwrap_or(false)
}
