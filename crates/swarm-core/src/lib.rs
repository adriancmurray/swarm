//! `swarm-core` — pure repo-trait substrate for the swarm runtime.
//!
//! Populated in P5-S1. This crate is the extraction keystone: it holds the
//! repo-trait definitions and their companion types, leaving all concrete
//! File*/Mem* implementations in `agent-swarm` (they move to `swarm-store`
//! in P5-S2).
//!
//! # Dependency constraint
//!
//! This crate depends **only** on:
//! - `swarm-contracts` (wire types: `SessionId`, `EventKind`, `JobRecord`, etc.)
//! - `serde` + `serde_json` (de/serialization primitives)
//!
//! No external-system types.
//!
//! # Split-identity law (Gate-2)
//!
//! `swarm-core` must resolve from a single `local-path` source in the
//! Cargo build graph. `ci/law-checks.sh` CHECK5 enforces this.
//!
//! # What moved from `agent-swarm` in P5-S1
//!
//! - Primitives: `RepoError`, `Cursor`, `ProcessLiveness`, `NeverAlive`, `AlwaysAlive`
//! - Trait + companions: `SessionRepo` family, `JobRepo` family,
//!   `EventRepo` family, `TelemetryRepo`
//!
//! # What stays in `agent-swarm` (temporarily)
//!
//! - `OsProcessLiveness` — impl calling `crate::process::process_is_alive`;
//!   scheduled for `swarm-store` in P5-S3.
//! - `PackageRepo` — signature uses `AgentProfile` (agent-swarm-local, `&'static str` lifetime).
//! - `RoutingMemoryRepo` — signature uses `AgentStats` (agent-swarm-local business type).
//! - All 6 `File*/Mem*` repo impls — move to `swarm-store` in P5-S2.

pub mod cursor;
pub mod error;
pub mod event_repo;
pub mod job_repo;
pub mod ledger_repo;
pub mod liveness;
pub mod session_repo;
pub mod telemetry_repo;

// Crate-root flat re-exports so agent-swarm's repos/mod.rs shim can do
// `pub use swarm_core::{RepoError, Cursor, ...}` without qualifying
// every submodule path.

pub use cursor::Cursor;
pub use error::RepoError;
pub use event_repo::{EventContext, EventRepo, LayerReportSpec, StoredEvent};
pub use job_repo::{JobRepo, JobSpec};
pub use ledger_repo::{fold_tasks, LedgerRepo, LedgerStatus, LedgerTask, LEDGER_TASK_SCHEMA};
pub use liveness::{AlwaysAlive, NeverAlive, ProcessLiveness};
pub use session_repo::{
    SessionArtifact, SessionHandle, SessionIndexRecord, SessionMeta, SessionRepo, SessionSpec,
    SessionStatus, SessionStatusDeriver, SessionSummary,
};
pub use telemetry_repo::TelemetryRepo;
