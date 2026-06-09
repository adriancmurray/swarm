//! `swarm-store` — file-backed store machinery for the swarm runtime.
//!
//! Populated in P5-S2. This crate holds:
//! - Low-level store primitives (lock statics, counters, path helpers) — `store`
//! - Monitor state / alert storage — `monitor_store`
//! - Job record helpers — `job`
//! - File*/Mem* repo implementations — `repos`
//! - `OsProcessLiveness` — production liveness oracle
//!
//! # Dependency constraint (Gate-2)
//!
//! This crate depends **only** on:
//! - `swarm-core` (repo traits + companion types)
//! - `swarm-contracts` (wire types: SessionId, EventKind, JobRecord, etc.)
//! - `serde` + `serde_json`
//!
//! No external-system deps.
//!
//! # Encapsulation invariant
//!
//! The three write-synchronization statics (`EVENT_LOG_LOCK`, `ATOMIC_WRITE_COUNTER`,
//! `EVENT_SEQ_COUNTER`) are `pub(crate)` in `store` and NOT re-exported from this
//! lib. Only code inside this crate (event_repo, session_repo) acquires them.
//! The visibility assertion in `ci/law-checks.sh` CHECK 7 proves this structurally.

pub mod job;
pub mod monitor_store;
pub mod repos;
pub mod store;

// Private helpers — zero public surface.
mod ledger_helpers;
mod process_helpers;
mod telemetry_helpers;

// Flat re-exports for agent-swarm shim convenience.
pub use repos::event_repo::{FileEventRepo, MemEventRepo};
pub use repos::job_repo::{FileJobRepo, MemJobRepo};
pub use repos::ledger_repo::{default_file_ledger_repo, FileLedgerRepo, MemLedgerRepo};
pub use repos::session_repo::{FileSessionRepo, MemSessionRepo};
pub use repos::telemetry_repo::{default_file_telemetry_repo, FileTelemetryRepo, MemTelemetryRepo};
pub use repos::OsProcessLiveness;
