//! `EventRepo` trait + companion types.
//!
//! Moved from `agent-swarm::repos::event_repo` in P5-S1 (trait + types only;
//! `FileEventRepo` and `MemEventRepo` stay in `agent-swarm` until P5-S2).
//!
//! See the original `event_repo.rs` doc-comment for the full cursor contract
//! and layer-report lock fix rationale.

use serde::{Deserialize, Serialize};

use swarm_contracts::events::EventKind;
use swarm_contracts::ids::SessionId;

use crate::cursor::Cursor;
use crate::error::RepoError;

// ── EventContext ──────────────────────────────────────────────────────────────

/// Contextual metadata attached to each emitted event.
///
/// Mirrors the `parent_id / agent_id / role / phase` fields on the
/// `agent-swarm/event/v2` wire envelope.
#[derive(Debug, Clone)]
pub struct EventContext {
    pub parent_id: Option<String>,
    pub agent_id: String,
    pub role: String,
    pub phase: String,
}

impl Default for EventContext {
    fn default() -> Self {
        Self {
            parent_id: None,
            agent_id: "auto".to_string(),
            role: "participant".to_string(),
            phase: "discussion".to_string(),
        }
    }
}

// ── LayerReportSpec ───────────────────────────────────────────────────────────

/// Input to `EventRepo::append_layer_report`.
///
/// Carries everything needed to write the `.md` sidecar, the
/// `layer_reports.jsonl` index line, and the associated `LayerReport` event
/// — all under one lock acquisition.
#[derive(Debug, Clone)]
pub struct LayerReportSpec {
    pub layer: String,
    pub role: String,
    pub agent: String,
    pub parent_role: Option<String>,
    pub status: String,
    pub text: String,
}

// ── StoredEvent ───────────────────────────────────────────────────────────────

/// Owned read model for an `agent-swarm/event/v2` event line.
///
/// Deserializes from the existing `events.jsonl` JSONL format without changing
/// the wire bytes. `payload` stays `serde_json::Value` (typed payload union is
/// deferred to a later slice).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredEvent {
    /// Session that owns this event.
    pub session_id: String,
    /// Typed event kind — `Other(String)` is the forward-compat arm.
    pub kind: EventKind,
    /// Raw payload — kept as `Value` until the typed-payload slice lands.
    pub payload: serde_json::Value,
    /// Milliseconds since UNIX epoch.
    pub ts_ms: u128,
    /// Contiguous sequence number from `EVENT_SEQ_COUNTER`.
    pub seq: u64,
    /// Parent event id, if any.
    pub parent_id: Option<String>,
    /// Agent that emitted this event.
    pub agent_id: String,
    /// Role of the participant.
    pub role: String,
    /// Current orchestration phase.
    pub phase: String,
}

// ── EventRepo trait ───────────────────────────────────────────────────────────

/// Pure storage abstraction for session event appends and reads.
///
/// All mutating methods return a `Cursor` positioned past the written event so
/// that callers can immediately resume reading from that point.
pub trait EventRepo: Send + Sync {
    /// Append a typed event to a session's event log.
    ///
    /// Returns the cursor positioned after the written event.
    fn append(
        &self,
        session: &SessionId,
        kind: EventKind,
        payload: serde_json::Value,
        ctx: EventContext,
    ) -> Result<Cursor, RepoError>;

    /// Append a layer report and its associated `LayerReport` event atomically
    /// within one lock acquisition.
    fn append_layer_report(
        &self,
        session: &SessionId,
        spec: LayerReportSpec,
    ) -> Result<Cursor, RepoError>;

    /// Return events strictly after `after`, up to `limit`.
    ///
    /// Returns `(events, new_cursor)`. See module doc for full cursor contract.
    fn events_since(
        &self,
        session: &SessionId,
        after: Cursor,
        limit: usize,
    ) -> Result<(Vec<StoredEvent>, Cursor), RepoError>;

    /// Return the last `limit` events from the log (most-recent tail).
    fn tail(&self, session: &SessionId, limit: usize) -> Result<Vec<StoredEvent>, RepoError>;

    /// Return the kind of the most recent event in the session log.
    fn latest_kind(&self, session: &SessionId) -> Result<Option<EventKind>, RepoError>;
}
