//! Repository implementations for swarm-store.
//!
//! # P5-S2: moved from agent-swarm
//!
//! `FileEventRepo`, `MemEventRepo`, `FileSessionRepo`, `MemSessionRepo`,
//! `FileJobRepo`, `MemJobRepo`, `FileTelemetryRepo`, `MemTelemetryRepo`
//! now live here. `OsProcessLiveness` is also moved here.
//!
//! `PackageRepo` and `RoutingMemoryRepo` STAY in agent-swarm (their traits use
//! `AgentProfile`/`AgentStats` which are agent-swarm-local types).

pub mod event_repo;
pub mod job_repo;
pub mod ledger_repo;
pub mod session_repo;
pub mod telemetry_repo;

// Re-export all trait + companion types from swarm-core for caller convenience.
pub use swarm_core::{
    fold_tasks, AlwaysAlive, Cursor, EventContext, EventRepo, JobRepo, JobSpec, LayerReportSpec,
    LedgerRepo, LedgerStatus, LedgerTask, NeverAlive, ProcessLiveness, RepoError, SessionArtifact,
    SessionHandle, SessionIndexRecord, SessionMeta, SessionRepo, SessionSpec, SessionStatus,
    SessionStatusDeriver, SessionSummary, StoredEvent, TelemetryRepo, LEDGER_TASK_SCHEMA,
};

// ── OsProcessLiveness ────────────────────────────────────────────────────────
//
// Production liveness oracle. Delegates to `crate::process_helpers::process_is_alive`.
// Moved from agent-swarm's `repos/mod.rs` where it had a `// TODO P5-S3` comment.

/// Production liveness oracle — delegates to the inlined `process_is_alive` helper.
pub struct OsProcessLiveness;

impl ProcessLiveness for OsProcessLiveness {
    fn is_alive(&self, pid: u32) -> bool {
        crate::process_helpers::process_is_alive(pid)
    }
}
