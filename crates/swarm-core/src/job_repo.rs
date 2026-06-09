//! `JobRepo` trait + companion types.
//!
//! Moved from `agent-swarm::repos::job_repo` in P5-S1 (trait + types only;
//! `FileJobRepo` and `MemJobRepo` stay in `agent-swarm` until P5-S2).

use std::path::PathBuf;

use swarm_contracts::ids::JobId;
use swarm_contracts::jobs::{JobAgent, JobMode, JobRecord};

use crate::error::RepoError;
use crate::liveness::ProcessLiveness;

// ── JobSpec ───────────────────────────────────────────────────────────────────

/// Typed input to `JobRepo::create()`.
///
/// Replaces the parameter explosion in `create_tracking_record()`.
///
/// `JobAgent` and `JobMode` appear local but are `pub use swarm_contracts::jobs::*`
/// shims in `agent-swarm`; they are the same concrete swarm-contracts types here.
pub struct JobSpec {
    pub agent: JobAgent,
    pub model: Option<String>,
    pub mode: JobMode,
    pub cwd: PathBuf,
    /// Pre-truncated display string stored verbatim — NOT re-truncated by `create()`.
    pub prompt_preview: String,
    /// Full prompt text written to the `.prompt.md` sidecar.
    pub prompt_text: String,
    pub timeout_secs: u64,
    pub allow_recursive_codex: bool,
}

// ── JobRepo trait ─────────────────────────────────────────────────────────────

/// Pure storage trait for job records.
///
/// All methods return `Result<_, RepoError>`.  No method performs liveness
/// checks; use `reconcile_liveness` explicitly before reads that need current
/// process status.
pub trait JobRepo: Send + Sync {
    /// Create and persist a new job record in the `Running` state.
    fn create(&self, spec: JobSpec) -> Result<JobRecord, RepoError>;

    /// Read a single job record by id.
    fn get(&self, id: &JobId) -> Result<JobRecord, RepoError>;

    /// List all job records. **Pure** — no liveness side effects.
    fn list(&self) -> Result<Vec<JobRecord>, RepoError>;

    /// Persist an updated job record (used by finish and prompt-update paths).
    fn save(&self, record: &JobRecord) -> Result<(), RepoError>;

    /// Return the most-recently-created job record, without liveness checks.
    fn latest(&self) -> Result<Option<JobRecord>, RepoError>;

    /// Check all Running records against the liveness oracle.
    ///
    /// Returns only the records that transitioned to Lost (empty = nothing changed).
    fn reconcile_liveness(
        &self,
        liveness: &dyn ProcessLiveness,
    ) -> Result<Vec<JobRecord>, RepoError>;
}
