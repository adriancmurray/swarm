//! `SessionRepo` trait + companion types.
//!
//! Moved from `agent-swarm::repos::session_repo` in P5-S1 (trait + companion
//! types only; `FileSessionRepo` and `MemSessionRepo` stay in `agent-swarm`
//! until P5-S2).
//!
//! `SessionStatusDeriver::derive` references `EventKind` (from `swarm-contracts`
//! directly) — it was previously `crate::events::EventKind`, a
//! `pub use swarm_contracts::events::EventKind` shim. Same concrete type.

use std::path::PathBuf;

use swarm_contracts::events::EventKind;
use swarm_contracts::ids::SessionId;

use crate::error::RepoError;
use crate::event_repo::EventRepo;
use crate::liveness::ProcessLiveness;

// ── SessionStatus ─────────────────────────────────────────────────────────────

/// Computed lifecycle status for a session.
///
/// Produced by `SessionStatusDeriver` from a `SessionIndexRecord`, a
/// `ProcessLiveness` oracle, and the last `EventKind` in the session log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionStatus {
    /// A `session_completed` event was the last in the log.
    Completed,
    /// Pid is alive according to the liveness oracle.
    Running,
    /// Pid existed but the liveness oracle says it is dead.
    Lost,
    /// Pid unknown or absent, and the session is older than 5 minutes.
    Incomplete,
}

impl SessionStatus {
    /// The wire string used by existing MCP consumers.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Completed => "completed",
            Self::Running => "running",
            Self::Lost => "lost",
            Self::Incomplete => "incomplete",
        }
    }
}

// ── SessionSpec ───────────────────────────────────────────────────────────────

/// Input to `SessionRepo::create`.
///
/// Minimal — covers what the repo needs (mode + initial prompt) without
/// importing `DiscussArgs` or `SwarmArgs` into the trait layer.
#[derive(Debug, Clone)]
pub struct SessionSpec {
    /// Working directory at session start.
    pub cwd: PathBuf,
    /// Human-readable mode identifier (e.g. "fanout", "discussion").
    pub mode: String,
    /// The prompt text — full text; callers derive the preview.
    pub prompt: String,
}

// ── SessionHandle ─────────────────────────────────────────────────────────────

/// Owned handle to an open session.
///
/// `E` is the concrete `EventRepo` implementation.  The handle owns it so
/// that callers can append events through `self.events` without going through
/// the repo again.
pub struct SessionHandle<E: EventRepo> {
    /// The session identifier.
    pub id: SessionId,
    /// Owned event repo bound to this session's storage.
    pub events: E,
}

// ── SessionMeta ───────────────────────────────────────────────────────────────

/// Typed metadata written to `session.json`.
///
/// Kept minimal — the full structured metadata (manager, participants, docs,
/// etc.) is still written by `DiscussionSession::write_metadata` /
/// `write_swarm_metadata`.
#[derive(Debug, Clone)]
pub struct SessionMeta {
    /// Session identifier (mirrors the directory name).
    pub id: String,
    /// Milliseconds since UNIX epoch at which the session was created.
    pub created_at_ms: u128,
    /// OS pid of the process that created the session.
    pub pid: u32,
    /// Full prompt text.
    pub prompt: String,
    /// Working directory.
    pub cwd: PathBuf,
    /// Mode string (e.g. "fanout", "discussion").
    pub mode: String,
}

// ── SessionIndexRecord ────────────────────────────────────────────────────────

/// Lightweight record returned by `SessionRepo::list()`.
///
/// Contains only fields that can be read without touching the event tail or
/// the liveness oracle. Status derivation is handled by `SessionStatusDeriver`.
#[derive(Debug, Clone)]
pub struct SessionIndexRecord {
    /// Session identifier string.
    pub id: SessionId,
    /// Milliseconds since UNIX epoch at creation time.
    pub created_at_ms: u128,
    /// OS pid of the session process, if written to `session.json`.
    pub pid: Option<u32>,
    /// Pre-truncated preview of the prompt (72 chars).
    pub prompt_preview: String,
}

// ── SessionSummary ────────────────────────────────────────────────────────────

/// Rich session summary returned by `SessionRepo::summary`.
///
/// Wraps the `session-summary/v1` JSON value produced by `report.rs`.
#[derive(Debug, Clone)]
pub struct SessionSummary {
    /// The full `agent-swarm/session-summary/v1` payload.
    pub value: serde_json::Value,
}

// ── SessionArtifact ───────────────────────────────────────────────────────────

/// A single artifact entry from `SessionRepo::artifacts`.
#[derive(Debug, Clone)]
pub struct SessionArtifact {
    /// Human-readable label (e.g. "events", "transcript", "layer-report").
    pub label: String,
    /// Absolute path to the artifact file.
    pub path: PathBuf,
    /// MIME type string.
    pub mime: String,
    /// File size in bytes.
    pub bytes: u64,
}

// ── SessionRepo trait ─────────────────────────────────────────────────────────

/// Pure storage abstraction for sessions.
///
/// All methods return `Result<_, RepoError>`. Liveness status derivation is
/// performed separately by `SessionStatusDeriver`.
pub trait SessionRepo: Send + Sync {
    /// The concrete `EventRepo` implementation owned by handles from this repo.
    type Events: EventRepo;

    /// Create a new session directory and emit an initial `Created` event.
    fn create(&self, spec: SessionSpec) -> Result<SessionHandle<Self::Events>, RepoError>;

    /// Load a handle for an existing session.
    fn open(&self, id: &SessionId) -> Result<SessionHandle<Self::Events>, RepoError>;

    /// Return all known session index records — **pure**, no liveness side effects.
    fn list(&self) -> Result<Vec<SessionIndexRecord>, RepoError>;

    /// Write or update `session.json` for the given session.
    fn write_metadata(&self, id: &SessionId, meta: &SessionMeta) -> Result<(), RepoError>;

    /// Read the full session summary (metadata + recent events + digest).
    fn summary(&self, id: &SessionId) -> Result<SessionSummary, RepoError>;

    /// List artifact paths for a session.
    fn artifacts(&self, id: &SessionId) -> Result<Vec<SessionArtifact>, RepoError>;
}

// ── SessionStatusDeriver ──────────────────────────────────────────────────────

/// Derives `SessionStatus` from raw record fields and a liveness oracle.
///
/// Does NOT touch the filesystem or call OS APIs directly — those are injected
/// via `&dyn ProcessLiveness` and `latest_event_kind: Option<&EventKind>`.
///
/// # Derivation rules
///
/// 1. If `latest_event_kind == SessionCompleted` → `Completed`.
/// 2. If `pid` present and `liveness.is_alive(pid)` → `Running`.
/// 3. If `pid` present and NOT alive → `Lost`.
/// 4. If `pid` absent and `now_ms - created_at_ms > 5 min` → `Incomplete`.
/// 5. Otherwise (pid absent, session is recent) → `Running`.
pub struct SessionStatusDeriver;

impl SessionStatusDeriver {
    /// Derive the status for a session index record.
    ///
    /// - `record`: the raw index record from `SessionRepo::list()`.
    /// - `latest_event_kind`: the last `EventKind` in the session's event log.
    /// - `liveness`: oracle for pid liveness.
    /// - `now_ms`: current time in milliseconds (injected for testability).
    pub fn derive(
        record: &SessionIndexRecord,
        latest_event_kind: Option<&EventKind>,
        liveness: &dyn ProcessLiveness,
        now_ms: u128,
    ) -> SessionStatus {
        // Rule 1: completed event in the tail
        if matches!(latest_event_kind, Some(EventKind::SessionCompleted)) {
            return SessionStatus::Completed;
        }

        // Rules 2-3: pid present — ask the oracle
        if let Some(pid) = record.pid {
            if liveness.is_alive(pid) {
                return SessionStatus::Running;
            }
            return SessionStatus::Lost;
        }

        // Rules 4-5: no pid — age-based fallback
        const FIVE_MINUTES_MS: u128 = 5 * 60 * 1_000;
        if now_ms.saturating_sub(record.created_at_ms) > FIVE_MINUTES_MS {
            SessionStatus::Incomplete
        } else {
            SessionStatus::Running
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::liveness::{AlwaysAlive, NeverAlive};

    fn make_rec(id: &str, pid: Option<u32>, created_at_ms: u128) -> SessionIndexRecord {
        SessionIndexRecord {
            id: SessionId::from(id),
            created_at_ms,
            pid,
            prompt_preview: String::new(),
        }
    }

    fn now_ms() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis()
    }

    #[test]
    fn status_deriver_completed_event_overrides_pid() {
        let rec = make_rec("session-test-completed", Some(99_999_999), now_ms());
        let status = SessionStatusDeriver::derive(
            &rec,
            Some(&EventKind::SessionCompleted),
            &AlwaysAlive,
            now_ms(),
        );
        assert_eq!(status, SessionStatus::Completed);
    }

    #[test]
    fn status_deriver_pid_alive_returns_running() {
        let rec = make_rec("session-test-running", Some(99_999_999), now_ms());
        let status =
            SessionStatusDeriver::derive(&rec, Some(&EventKind::TurnChunk), &AlwaysAlive, now_ms());
        assert_eq!(status, SessionStatus::Running);
    }

    #[test]
    fn status_deriver_pid_dead_returns_lost() {
        let rec = make_rec("session-test-lost", Some(99_999_999), now_ms());
        let status =
            SessionStatusDeriver::derive(&rec, Some(&EventKind::TurnChunk), &NeverAlive, now_ms());
        assert_eq!(status, SessionStatus::Lost);
    }

    #[test]
    fn status_deriver_no_pid_old_session_returns_incomplete() {
        let old_ts = now_ms().saturating_sub(10 * 60 * 1_000);
        let rec = make_rec("session-test-incomplete", None, old_ts);
        let status = SessionStatusDeriver::derive(&rec, None, &NeverAlive, now_ms());
        assert_eq!(status, SessionStatus::Incomplete);
    }

    #[test]
    fn status_deriver_no_pid_recent_session_returns_running() {
        let recent_ts = now_ms().saturating_sub(30 * 1_000);
        let rec = make_rec("session-test-recent", None, recent_ts);
        let status = SessionStatusDeriver::derive(&rec, None, &NeverAlive, now_ms());
        assert_eq!(status, SessionStatus::Running);
    }

    #[test]
    fn session_status_wire_strings_are_stable() {
        assert_eq!(SessionStatus::Completed.as_str(), "completed");
        assert_eq!(SessionStatus::Running.as_str(), "running");
        assert_eq!(SessionStatus::Lost.as_str(), "lost");
        assert_eq!(SessionStatus::Incomplete.as_str(), "incomplete");
    }
}
