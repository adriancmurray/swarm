//! Typed `agent-swarm/event/v2` event-kind identity — re-exported from
//! [`swarm_contracts::events`].
//!
//! All call sites using `crate::events::EventKind` continue to work unchanged;
//! the type is now the canonical swarm-contracts definition (single source of truth,
//! Phase-2 cutover).
//!
//! # Wire contract (unchanged)
//!
//! `EventKind` serializes to a bare JSON string. Every known variant maps to its
//! exact historical snake_case wire string. `Other(String)` serializes to the bare
//! string it carries. The forward-compatibility contract is identical to the
//! pre-cutover local definition.

pub use swarm_contracts::events::EventKind;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_kind_wire_strings_are_stable() {
        // Every production variant must serialize to its exact historical wire
        // string. A mismatch here means a `kind` regression that the presence-
        // only lockbox in `main.rs` would not catch.
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
        // `test_kind` is emitted by the main.rs lockbox tests; `manager_synthesis`
        // appears in the checked-in fixture event log. Both must serialize as a
        // bare string with no tagging — the forward-compatibility contract.
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
        // Deserialize is the exact inverse of Serialize: for every known variant,
        // deserializing its wire string must recover the original variant.
        // This test reuses the same variant→wire mapping that locks the Serialize
        // side, so a missing arm in visit_str will fail here loudly.
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
            // Also verify serialize→deserialize round-trip
            let re_encoded = serde_json::to_string(variant).unwrap();
            let round_tripped: EventKind = serde_json::from_str(&re_encoded).unwrap();
            assert_eq!(&round_tripped, variant, "round-trip mismatch for {wire:?}");
        }
    }

    #[test]
    fn event_kind_unknown_string_deserializes_to_other() {
        // Unknown wire strings (e.g. fixture historical kinds) must deserialize
        // to Other(String) rather than failing — the forward-compatibility contract.
        for raw in ["manager_synthesis", "some_future_event_kind"] {
            let json = format!("\"{raw}\"");
            let decoded: EventKind = serde_json::from_str(&json)
                .unwrap_or_else(|e| panic!("deserialize failed for {raw:?}: {e}"));
            assert_eq!(
                decoded,
                EventKind::Other(raw.to_string()),
                "expected Other({raw:?}), got {decoded:?}"
            );
            // Other round-trips byte-identically
            assert_eq!(
                serde_json::to_string(&decoded).unwrap(),
                json,
                "Other({raw:?}) must re-serialize to the original wire string"
            );
        }
    }
}
