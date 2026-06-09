//! Typed `agent-swarm/event/v2` contract types.
//!
//! # Wire contract
//!
//! `EventKind` serializes as a bare JSON string. Every known variant maps to
//! its exact historical snake_case wire string. `Other(String)` serializes to
//! the bare string it carries (NOT a `{"Other": ...}` tagged object). This is
//! the forward-compatibility escape hatch — unrecognized on-disk event kinds
//! survive read-mutate-write cycles without data loss.
//!
//! `SessionEventV2` is the `agent-swarm/event/v2` envelope. Fields are
//! declared in alphabetical order so that struct serialization (field-declaration
//! order) produces byte-identical JSON to agent-swarm's output path, which
//! goes through `serde_json::to_value` → `BTreeMap` → alphabetical key order.

use serde::de;
use std::fmt;

// ── EventKind ─────────────────────────────────────────────────────────────────

/// The `kind` discriminant of an `agent-swarm/event/v2` envelope.
///
/// Serializes to a bare string (see `as_str()`). The `Deserialize` impl is the
/// exact inverse: every known wire string maps to its variant; any other string
/// maps to `Other(String)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventKind {
    Created,
    SessionStarted,
    SessionCompleted,
    FanoutStarted,
    PreflightStarted,
    PreflightCompleted,
    PreflightFailed,
    ManagerStarted,
    ManagerCompleted,
    ManagerFailed,
    WorkerStarted,
    WorkerCompleted,
    WorkerFailed,
    /// A backend produced no usable output and the chain advanced to the next
    /// backend (visible degrade — never a silent fallback).
    WorkerFallback,
    /// A backend failed and is being retried after a jittered backoff.
    BackendRetry,
    HelperStarted,
    HelperCompleted,
    HelperFailed,
    DocsStarted,
    DocsCompleted,
    DocsFailed,
    TurnStarted,
    TurnCompleted,
    TurnFailed,
    TurnHeartbeat,
    TurnChunk,
    TurnHealthCheck,
    AgentMessage,
    ProfileAssigned,
    LayerReport,
    DiscussionDigestUpdated,
    /// Forward-compatibility / test-isolation escape hatch. Carries an
    /// arbitrary kind string verbatim onto the wire.
    ///
    /// Reserved API surface: currently constructed only in tests and on the
    /// read path when an unrecognized kind is encountered. The production
    /// write path should always use a named variant.
    Other(String),
}

impl EventKind {
    /// The exact wire string for this kind. Unit variants map to their
    /// historical snake_case token; `Other(s)` returns `s` unchanged.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Created => "created",
            Self::SessionStarted => "session_started",
            Self::SessionCompleted => "session_completed",
            Self::FanoutStarted => "fanout_started",
            Self::PreflightStarted => "preflight_started",
            Self::PreflightCompleted => "preflight_completed",
            Self::PreflightFailed => "preflight_failed",
            Self::ManagerStarted => "manager_started",
            Self::ManagerCompleted => "manager_completed",
            Self::ManagerFailed => "manager_failed",
            Self::WorkerStarted => "worker_started",
            Self::WorkerCompleted => "worker_completed",
            Self::WorkerFailed => "worker_failed",
            Self::WorkerFallback => "worker_fallback",
            Self::BackendRetry => "backend_retry",
            Self::HelperStarted => "helper_started",
            Self::HelperCompleted => "helper_completed",
            Self::HelperFailed => "helper_failed",
            Self::DocsStarted => "docs_started",
            Self::DocsCompleted => "docs_completed",
            Self::DocsFailed => "docs_failed",
            Self::TurnStarted => "turn_started",
            Self::TurnCompleted => "turn_completed",
            Self::TurnFailed => "turn_failed",
            Self::TurnHeartbeat => "turn_heartbeat",
            Self::TurnChunk => "turn_chunk",
            Self::TurnHealthCheck => "turn_health_check",
            Self::AgentMessage => "agent_message",
            Self::ProfileAssigned => "profile_assigned",
            Self::LayerReport => "layer_report",
            Self::DiscussionDigestUpdated => "discussion_digest_updated",
            Self::Other(value) => value.as_str(),
        }
    }
}

impl serde::Serialize for EventKind {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> serde::Deserialize<'de> for EventKind {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> de::Visitor<'de> for V {
            type Value = EventKind;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("an event kind string")
            }
            fn visit_str<E: de::Error>(self, s: &str) -> Result<EventKind, E> {
                Ok(match s {
                    "created" => EventKind::Created,
                    "session_started" => EventKind::SessionStarted,
                    "session_completed" => EventKind::SessionCompleted,
                    "fanout_started" => EventKind::FanoutStarted,
                    "preflight_started" => EventKind::PreflightStarted,
                    "preflight_completed" => EventKind::PreflightCompleted,
                    "preflight_failed" => EventKind::PreflightFailed,
                    "manager_started" => EventKind::ManagerStarted,
                    "manager_completed" => EventKind::ManagerCompleted,
                    "manager_failed" => EventKind::ManagerFailed,
                    "worker_started" => EventKind::WorkerStarted,
                    "worker_completed" => EventKind::WorkerCompleted,
                    "worker_failed" => EventKind::WorkerFailed,
                    "worker_fallback" => EventKind::WorkerFallback,
                    "backend_retry" => EventKind::BackendRetry,
                    "helper_started" => EventKind::HelperStarted,
                    "helper_completed" => EventKind::HelperCompleted,
                    "helper_failed" => EventKind::HelperFailed,
                    "docs_started" => EventKind::DocsStarted,
                    "docs_completed" => EventKind::DocsCompleted,
                    "docs_failed" => EventKind::DocsFailed,
                    "turn_started" => EventKind::TurnStarted,
                    "turn_completed" => EventKind::TurnCompleted,
                    "turn_failed" => EventKind::TurnFailed,
                    "turn_heartbeat" => EventKind::TurnHeartbeat,
                    "turn_chunk" => EventKind::TurnChunk,
                    "turn_health_check" => EventKind::TurnHealthCheck,
                    "agent_message" => EventKind::AgentMessage,
                    "profile_assigned" => EventKind::ProfileAssigned,
                    "layer_report" => EventKind::LayerReport,
                    "discussion_digest_updated" => EventKind::DiscussionDigestUpdated,
                    other => EventKind::Other(other.to_string()),
                })
            }
        }
        d.deserialize_str(V)
    }
}

// ── SessionEventV2 ────────────────────────────────────────────────────────────

/// The `agent-swarm/event/v2` envelope persisted to `events.jsonl`.
///
/// # Wire contract — field order
///
/// Fields are declared in alphabetical order so that struct serialization
/// (Serde writes fields in declaration order) produces byte-identical JSON to
/// agent-swarm's production output, which goes through
/// `serde_json::to_value(SessionEvent{..})` → `serde_json::Value::Object`
/// (BTreeMap, alphabetically sorted keys) → `serde_json::to_string`.
///
/// # `parent_id` null behaviour
///
/// `parent_id` has no `skip_serializing_if` attribute. When `None`, it emits
/// `"parent_id":null` — this is intentional and matches the production wire.
/// Do not add `skip_serializing_if` here.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SessionEventV2 {
    pub agent_id: String,
    pub kind: EventKind,
    /// `None` serializes as JSON `null` — do NOT add `skip_serializing_if`.
    pub parent_id: Option<String>,
    pub payload: serde_json::Value,
    pub phase: String,
    pub role: String,
    pub run_id: String,
    pub schema: String,
    pub seq: u64,
    pub session_id: String,
    pub ts_ms: u128,
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_kind_wire_strings_are_stable() {
        let cases: &[(EventKind, &str)] = &[
            (EventKind::Created, "created"),
            (EventKind::SessionStarted, "session_started"),
            (EventKind::SessionCompleted, "session_completed"),
            (EventKind::FanoutStarted, "fanout_started"),
            (EventKind::PreflightStarted, "preflight_started"),
            (EventKind::PreflightCompleted, "preflight_completed"),
            (EventKind::PreflightFailed, "preflight_failed"),
            (EventKind::ManagerStarted, "manager_started"),
            (EventKind::ManagerCompleted, "manager_completed"),
            (EventKind::ManagerFailed, "manager_failed"),
            (EventKind::WorkerStarted, "worker_started"),
            (EventKind::WorkerCompleted, "worker_completed"),
            (EventKind::WorkerFailed, "worker_failed"),
            (EventKind::WorkerFallback, "worker_fallback"),
            (EventKind::BackendRetry, "backend_retry"),
            (EventKind::HelperStarted, "helper_started"),
            (EventKind::HelperCompleted, "helper_completed"),
            (EventKind::HelperFailed, "helper_failed"),
            (EventKind::DocsStarted, "docs_started"),
            (EventKind::DocsCompleted, "docs_completed"),
            (EventKind::DocsFailed, "docs_failed"),
            (EventKind::TurnStarted, "turn_started"),
            (EventKind::TurnCompleted, "turn_completed"),
            (EventKind::TurnFailed, "turn_failed"),
            (EventKind::TurnHeartbeat, "turn_heartbeat"),
            (EventKind::TurnChunk, "turn_chunk"),
            (EventKind::TurnHealthCheck, "turn_health_check"),
            (EventKind::AgentMessage, "agent_message"),
            (EventKind::ProfileAssigned, "profile_assigned"),
            (EventKind::LayerReport, "layer_report"),
            (
                EventKind::DiscussionDigestUpdated,
                "discussion_digest_updated",
            ),
        ];
        for (kind, wire) in cases {
            assert_eq!(kind.as_str(), *wire, "as_str mismatch for {wire:?}");
            assert_eq!(
                serde_json::to_value(kind).unwrap(),
                serde_json::json!(wire),
                "serialize mismatch for {wire:?}"
            );
        }
    }

    #[test]
    fn other_kind_passes_through_verbatim() {
        for raw in ["test_kind", "manager_synthesis"] {
            let kind = EventKind::Other(raw.to_string());
            assert_eq!(kind.as_str(), raw);
            assert_eq!(
                serde_json::to_value(&kind).unwrap(),
                serde_json::json!(raw),
                "Other({raw:?}) must serialize as a bare string"
            );
        }
    }

    #[test]
    fn event_kind_round_trips_for_all_known_variants() {
        let cases: &[(EventKind, &str)] = &[
            (EventKind::Created, "created"),
            (EventKind::SessionStarted, "session_started"),
            (EventKind::SessionCompleted, "session_completed"),
            (EventKind::FanoutStarted, "fanout_started"),
            (EventKind::PreflightStarted, "preflight_started"),
            (EventKind::PreflightCompleted, "preflight_completed"),
            (EventKind::PreflightFailed, "preflight_failed"),
            (EventKind::ManagerStarted, "manager_started"),
            (EventKind::ManagerCompleted, "manager_completed"),
            (EventKind::ManagerFailed, "manager_failed"),
            (EventKind::WorkerStarted, "worker_started"),
            (EventKind::WorkerCompleted, "worker_completed"),
            (EventKind::WorkerFailed, "worker_failed"),
            (EventKind::WorkerFallback, "worker_fallback"),
            (EventKind::BackendRetry, "backend_retry"),
            (EventKind::HelperStarted, "helper_started"),
            (EventKind::HelperCompleted, "helper_completed"),
            (EventKind::HelperFailed, "helper_failed"),
            (EventKind::DocsStarted, "docs_started"),
            (EventKind::DocsCompleted, "docs_completed"),
            (EventKind::DocsFailed, "docs_failed"),
            (EventKind::TurnStarted, "turn_started"),
            (EventKind::TurnCompleted, "turn_completed"),
            (EventKind::TurnFailed, "turn_failed"),
            (EventKind::TurnHeartbeat, "turn_heartbeat"),
            (EventKind::TurnChunk, "turn_chunk"),
            (EventKind::TurnHealthCheck, "turn_health_check"),
            (EventKind::AgentMessage, "agent_message"),
            (EventKind::ProfileAssigned, "profile_assigned"),
            (EventKind::LayerReport, "layer_report"),
            (
                EventKind::DiscussionDigestUpdated,
                "discussion_digest_updated",
            ),
        ];
        for (variant, wire) in cases {
            let json = format!("\"{wire}\"");
            let decoded: EventKind = serde_json::from_str(&json)
                .unwrap_or_else(|e| panic!("deserialize failed for {wire:?}: {e}"));
            assert_eq!(
                &decoded, variant,
                "deserialize mismatch for {wire:?}: expected {variant:?}, got {decoded:?}"
            );
            let re_encoded = serde_json::to_string(variant).unwrap();
            let round_tripped: EventKind = serde_json::from_str(&re_encoded).unwrap();
            assert_eq!(&round_tripped, variant, "round-trip mismatch for {wire:?}");
        }
    }

    #[test]
    fn event_kind_unknown_string_deserializes_to_other() {
        for raw in ["manager_synthesis", "some_future_event_kind"] {
            let json = format!("\"{raw}\"");
            let decoded: EventKind = serde_json::from_str(&json)
                .unwrap_or_else(|e| panic!("deserialize failed for {raw:?}: {e}"));
            assert_eq!(
                decoded,
                EventKind::Other(raw.to_string()),
                "expected Other({raw:?}), got {decoded:?}"
            );
            assert_eq!(
                serde_json::to_string(&decoded).unwrap(),
                json,
                "Other({raw:?}) must re-serialize to the original wire string"
            );
        }
    }

    // ── Lockbox-replay: SessionEventV2 wire-equivalence proof ─────────────────
    //
    // These tests parse real fixture lines from the checked-in
    // `crates/swarm-mcp/tests/fixtures/session-store/session-fixture-completed/events.jsonl`
    // through `SessionEventV2` and assert:
    //   1. parse succeeds (schema is structurally compatible)
    //   2. re-serialization is byte-identical to the original fixture line
    //
    // Byte-identity holds because:
    //   - agent-swarm produces fixtures via `serde_json::to_value` →
    //     `serde_json::to_string`, which sorts object keys alphabetically
    //     (BTreeMap default; no `preserve_order` feature)
    //   - `SessionEventV2` declares its fields in alphabetical order, so struct
    //     serialization (declaration order) matches that alphabetical output
    //   - `payload` is a `serde_json::Value` which round-trips unchanged
    //   - `parent_id: None` emits `"parent_id":null` (no `skip_serializing_if`)

    /// Line 1 from fixture events.jsonl — session_started event.
    const FIXTURE_SESSION_STARTED: &str = r#"{"agent_id":"auto","kind":"session_started","parent_id":null,"payload":{"cwd":"/tmp/swarm-fixture"},"phase":"discussion","role":"manager","run_id":"session-fixture-completed","schema":"agent-swarm/event/v2","seq":1,"session_id":"session-fixture-completed","ts_ms":1780000000100}"#;

    /// Line 2 from fixture events.jsonl — manager_synthesis event (kind → Other).
    const FIXTURE_MANAGER_SYNTHESIS: &str = r#"{"agent_id":"claude:sonnet","kind":"manager_synthesis","parent_id":null,"payload":{"summary":"Fixture manager synthesis."},"phase":"discussion","role":"manager","run_id":"session-fixture-completed","schema":"agent-swarm/event/v2","seq":2,"session_id":"session-fixture-completed","ts_ms":1780000000200}"#;

    /// Line 3 from fixture events.jsonl — session_completed event.
    const FIXTURE_SESSION_COMPLETED: &str = r#"{"agent_id":"auto","kind":"session_completed","parent_id":null,"payload":{"status":"completed"},"phase":"discussion","role":"manager","run_id":"session-fixture-completed","schema":"agent-swarm/event/v2","seq":3,"session_id":"session-fixture-completed","ts_ms":1780000000300}"#;

    #[test]
    fn lockbox_session_started_parses_correctly() {
        let event: SessionEventV2 = serde_json::from_str(FIXTURE_SESSION_STARTED)
            .expect("session_started fixture must parse");
        assert_eq!(event.schema, "agent-swarm/event/v2");
        assert_eq!(event.kind, EventKind::SessionStarted);
        assert_eq!(event.session_id, "session-fixture-completed");
        assert_eq!(event.agent_id, "auto");
        assert!(event.parent_id.is_none());
        assert_eq!(event.seq, 1);
    }

    #[test]
    fn lockbox_session_started_byte_identical_round_trip() {
        let event: SessionEventV2 =
            serde_json::from_str(FIXTURE_SESSION_STARTED).expect("parse must succeed");
        let re_encoded = serde_json::to_string(&event).expect("serialize must succeed");
        assert_eq!(
            re_encoded, FIXTURE_SESSION_STARTED,
            "SessionEventV2 must re-serialize byte-identically to the fixture"
        );
    }

    #[test]
    fn lockbox_manager_synthesis_is_other_kind_byte_identical() {
        // manager_synthesis is not a named variant — it must deserialize to
        // Other("manager_synthesis") and re-serialize byte-identically.
        let event: SessionEventV2 = serde_json::from_str(FIXTURE_MANAGER_SYNTHESIS)
            .expect("manager_synthesis fixture must parse");
        assert_eq!(
            event.kind,
            EventKind::Other("manager_synthesis".to_string())
        );
        assert_eq!(event.agent_id, "claude:sonnet");
        let re_encoded = serde_json::to_string(&event).expect("serialize must succeed");
        assert_eq!(
            re_encoded, FIXTURE_MANAGER_SYNTHESIS,
            "manager_synthesis event must re-serialize byte-identically"
        );
    }

    #[test]
    fn lockbox_session_completed_byte_identical_round_trip() {
        let event: SessionEventV2 = serde_json::from_str(FIXTURE_SESSION_COMPLETED)
            .expect("session_completed fixture must parse");
        assert_eq!(event.kind, EventKind::SessionCompleted);
        let re_encoded = serde_json::to_string(&event).expect("serialize must succeed");
        assert_eq!(
            re_encoded, FIXTURE_SESSION_COMPLETED,
            "session_completed event must re-serialize byte-identically"
        );
    }

    #[test]
    fn lockbox_all_fixture_lines_byte_identical() {
        // All three fixture lines in one table-driven test.
        let fixtures = [
            FIXTURE_SESSION_STARTED,
            FIXTURE_MANAGER_SYNTHESIS,
            FIXTURE_SESSION_COMPLETED,
        ];
        for fixture in fixtures {
            let event: SessionEventV2 = serde_json::from_str(fixture)
                .unwrap_or_else(|e| panic!("parse failed for fixture: {e}\nfixture: {fixture}"));
            let re_encoded =
                serde_json::to_string(&event).unwrap_or_else(|e| panic!("serialize failed: {e}"));
            assert_eq!(
                re_encoded, fixture,
                "byte-identity failed for fixture:\n  expected: {fixture}\n  got:      {re_encoded}"
            );
        }
    }
}
