//! Event-log canonicalizer — the P0 "parity harness" foundation.
//!
//! Two runs of the *same logical work* produce byte-different `events.jsonl`:
//! each record carries a process-global `seq`, a wall-clock `ts_ms`, a per-run
//! `session_id`/`run_id`, heartbeats whose count tracks machine speed, and
//! thread-interleaved ordering. Payloads also embed durations, pids, absolute
//! paths, and byte counts. The *functional* content — which roles emitted which
//! kinds, with what stable payload fields — is what should match.
//!
//! [`canonicalize`] folds the raw stream into that functional spine: drop
//! ephemeral kinds → project to `{kind, role, payload}` → scrub volatile payload
//! fields → sort into a total canonical order. It is pure (no clock/IO/RNG),
//! deterministic, idempotent, and order-independent, so equality, diffing, and
//! [`canonical_run_hash`] become meaningful across runs.
//!
//! See `docs/specs/canonicalizer-spec.md` for the per-variant catalogue and the
//! volatile-field rules this module implements.

use crate::events::{EventKind, SessionEventV2};
use serde_json::Value;

/// One canonical event: kind + role + scrubbed payload.
///
/// Volatile-free and projection-equal across runs of the same logical work.
/// Serializes to a stable `{kind, role, payload}` object (fields in declaration
/// order; payload object keys sort via serde's BTreeMap).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CanonicalEvent {
    pub kind: String,
    pub role: String,
    pub payload: Value,
}

/// Fold a raw event stream into the deterministic functional spine.
///
/// Drops ephemeral kinds ([`EventKind::TurnHeartbeat`], [`EventKind::TurnChunk`]),
/// projects each surviving event to `{kind, role, payload}`, scrubs volatile
/// payload fields ([`scrub_volatile`]), then sorts into the total canonical
/// order `(kind, role, payload-json)`.
///
/// Pure: no clock, IO, or RNG. Deterministic, idempotent, and order-independent
/// — any permutation of `events` yields the same output.
pub fn canonicalize(events: &[SessionEventV2]) -> Vec<CanonicalEvent> {
    let mut out: Vec<CanonicalEvent> = events
        .iter()
        .filter(|e| !is_ephemeral(&e.kind))
        .map(|e| {
            let mut payload = e.payload.clone();
            scrub_volatile(&mut payload);
            CanonicalEvent {
                kind: e.kind.as_str().to_owned(),
                role: e.role.clone(),
                payload,
            }
        })
        .collect();

    // Total canonical order: (kind, role, scrubbed-payload-json). The payload
    // string is the final tiebreak, so the order is total even when two events
    // share (kind, role) — and it is stable across runs because every component
    // derives only from functional data after the volatile scrub.
    out.sort_by(|a, b| {
        (&a.kind, &a.role, payload_key(&a.payload)).cmp(&(
            &b.kind,
            &b.role,
            payload_key(&b.payload),
        ))
    });
    out
}

/// Stable, dependency-free hash over the serialized canonical projection.
///
/// FNV-1a/64 (hex), mirroring the style at `swarm-kernel/src/routing.rs:135` so
/// this adds no dependency. Equal iff two runs produced the same functional
/// canonical projection.
pub fn canonical_run_hash(events: &[SessionEventV2]) -> String {
    let canonical = canonicalize(events);
    // serde_json on a Vec<CanonicalEvent> is deterministic: declaration-order
    // struct fields + BTreeMap-sorted payload keys. ponytail: serialization
    // cannot fail here (no non-string map keys, no NaN), so we fall back to an
    // empty string rather than panicking.
    let bytes = serde_json::to_string(&canonical).unwrap_or_default();
    fnv1a_hex(bytes.as_bytes())
}

/// `true` for kinds whose presence/count is machine- or provider-dependent and
/// therefore carries no functional signal: heartbeats (6 s wall-clock loop) and
/// stream chunks (provider-dependent split; final text survives in `*_completed`).
fn is_ephemeral(kind: &EventKind) -> bool {
    matches!(kind, EventKind::TurnHeartbeat | EventKind::TurnChunk)
}

/// Recursively remove volatile keys and normalize path-like string values.
///
/// See `docs/specs/canonicalizer-spec.md` §5 for the full rule table. Objects:
/// drop volatile keys, recurse into survivors. Arrays: recurse into elements.
/// Strings: replace whole-value absolute paths with `"<path>"`.
pub fn scrub_volatile(value: &mut Value) {
    match value {
        Value::Object(map) => {
            map.retain(|k, _| !is_volatile_key(k));
            for v in map.values_mut() {
                scrub_volatile(v);
            }
        }
        Value::Array(items) => {
            for item in items {
                scrub_volatile(item);
            }
        }
        Value::String(s) => {
            if is_absolute_path(s) {
                *s = "<path>".to_owned();
            }
        }
        _ => {}
    }
}

/// `true` if `key` names a run-to-run-volatile field (timestamp, duration,
/// counter, byte count, pid, or absolute path). Suffix rules (`_ms`, `_bytes`,
/// `_path`) future-proof new fields; the exact set covers the rest.
fn is_volatile_key(key: &str) -> bool {
    const EXACT: &[&str] = &[
        // counters / ids
        "seq",
        "session_id",
        "run_id",
        "pid",
        // timestamps / durations
        "ts",
        "ts_ms",
        "started_at",
        "completed_at",
        "created_at",
        "elapsed",
        "duration",
        // sizes
        "bytes",
        // paths / per-run filenames
        "path",
        "file",
        "cwd",
    ];
    EXACT.contains(&key)
        || key.ends_with("_ms")
        || key.ends_with("_bytes")
        || key.ends_with("_path")
}

/// Whole-value absolute-path heuristic (conservative — only unambiguous paths).
///
/// ponytail ceiling: matches a value that is *entirely* an absolute path; paths
/// embedded inside prose are not normalized. Upgrade path: a regex sweep over
/// string values, deferred because it risks mangling functional text.
fn is_absolute_path(s: &str) -> bool {
    // Unix absolute: starts with '/' and has at least one more '/'.
    if let Some(rest) = s.strip_prefix('/') {
        if rest.contains('/') {
            return true;
        }
    }
    // Windows drive absolute: `C:\` or `C:/`.
    let bytes = s.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/')
}

/// Stable string key for sort tiebreaks. serde_json on a `Value` sorts object
/// keys (BTreeMap), so this is deterministic.
fn payload_key(payload: &Value) -> String {
    serde_json::to_string(payload).unwrap_or_default()
}

/// FNV-1a/64 over `bytes`, lowercase hex. Mirrors `routing.rs:135`.
fn fnv1a_hex(bytes: &[u8]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &byte in bytes {
        hash = (hash ^ u64::from(byte)).wrapping_mul(0x100_0000_01b3);
    }
    format!("{hash:016x}")
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Build a `SessionEventV2` with the given functional axes; the volatile
    /// envelope fields are passed explicitly so tests can vary them.
    #[allow(clippy::too_many_arguments)]
    fn ev(
        kind: EventKind,
        role: &str,
        payload: Value,
        seq: u64,
        ts_ms: u128,
        session_id: &str,
        agent_id: &str,
        parent_id: Option<&str>,
        phase: &str,
    ) -> SessionEventV2 {
        SessionEventV2 {
            agent_id: agent_id.to_owned(),
            kind,
            parent_id: parent_id.map(str::to_owned),
            payload,
            phase: phase.to_owned(),
            role: role.to_owned(),
            run_id: session_id.to_owned(),
            schema: "agent-swarm/event/v2".to_owned(),
            seq,
            session_id: session_id.to_owned(),
            ts_ms,
        }
    }

    /// Run A: a plausible discussion spine with run-A volatile values + one
    /// emission order.
    fn run_a() -> Vec<SessionEventV2> {
        vec![
            ev(
                EventKind::SessionStarted,
                "manager",
                json!({"prompt": "do the thing", "rounds": 1, "cwd": "/Users/alice/proj", "manager": "claude:sonnet"}),
                1,
                1_000,
                "session-AAAA",
                "auto",
                None,
                "discussion",
            ),
            ev(
                EventKind::TurnHeartbeat,
                "qa",
                json!({"round": 1, "role": "qa", "agent": "claude:sonnet", "elapsed_ms": 6000}),
                2,
                7_000,
                "session-AAAA",
                "claude:sonnet",
                Some("manager"),
                "turn",
            ),
            ev(
                EventKind::TurnChunk,
                "qa",
                json!({"round": 1, "role": "qa", "stream": "stdout", "text": "partial..."}),
                3,
                7_500,
                "session-AAAA",
                "claude:sonnet",
                Some("manager"),
                "turn",
            ),
            ev(
                EventKind::TurnCompleted,
                "qa",
                json!({"round": 1, "role": "qa", "agent": "claude:sonnet", "exit_code": 0, "timed_out": false, "text": "done"}),
                4,
                9_000,
                "session-AAAA",
                "claude:sonnet",
                None,
                "discussion",
            ),
            ev(
                EventKind::SessionCompleted,
                "manager",
                json!({"failed": false, "summary_path": "/Users/alice/proj/.swarm/sessions/session-AAAA/summary.md", "events_path": "/Users/alice/proj/.swarm/sessions/session-AAAA/events.jsonl"}),
                5,
                10_000,
                "session-AAAA",
                "auto",
                None,
                "discussion",
            ),
        ]
    }

    /// Run B: SAME logical work as run A, but different volatile values
    /// (seq/ts_ms/session_id/agent_id/parent_id/phase/paths), an EXTRA heartbeat
    /// (slower machine), and a SHUFFLED emission order.
    fn run_b() -> Vec<SessionEventV2> {
        vec![
            // shuffled: completed first
            ev(
                EventKind::TurnCompleted,
                "qa",
                json!({"round": 1, "role": "qa", "agent": "claude:sonnet", "exit_code": 0, "timed_out": false, "text": "done"}),
                104,
                99_000,
                "session-ZZZZ",
                "claude:sonnet",
                Some("manager"),
                "turn",
            ),
            ev(
                EventKind::SessionCompleted,
                "manager",
                json!({"failed": false, "summary_path": "/home/bob/work/.swarm/sessions/session-ZZZZ/summary.md", "events_path": "/home/bob/work/.swarm/sessions/session-ZZZZ/events.jsonl"}),
                105,
                99_900,
                "session-ZZZZ",
                "auto",
                None,
                "discussion",
            ),
            // two heartbeats this run, with different elapsed values
            ev(
                EventKind::TurnHeartbeat,
                "qa",
                json!({"round": 1, "role": "qa", "agent": "claude:sonnet", "elapsed_ms": 6001}),
                102,
                70_010,
                "session-ZZZZ",
                "claude:sonnet",
                Some("manager"),
                "turn",
            ),
            ev(
                EventKind::TurnHeartbeat,
                "qa",
                json!({"round": 1, "role": "qa", "agent": "claude:sonnet", "elapsed_ms": 12002}),
                103,
                76_020,
                "session-ZZZZ",
                "claude:sonnet",
                Some("manager"),
                "turn",
            ),
            ev(
                EventKind::TurnChunk,
                "qa",
                json!({"round": 1, "role": "qa", "stream": "stdout", "text": "different chunk boundary"}),
                106,
                70_100,
                "session-ZZZZ",
                "claude:sonnet",
                Some("manager"),
                "turn",
            ),
            ev(
                EventKind::SessionStarted,
                "manager",
                json!({"prompt": "do the thing", "rounds": 1, "cwd": "/home/bob/work", "manager": "claude:sonnet"}),
                101,
                60_000,
                "session-ZZZZ",
                "auto",
                None,
                "discussion",
            ),
        ]
    }

    #[test]
    fn volatile_and_order_differences_canonicalize_equal() {
        let a = canonicalize(&run_a());
        let b = canonicalize(&run_b());
        assert_eq!(
            a, b,
            "two runs of the same logical work must canonicalize equal\nA={a:#?}\nB={b:#?}"
        );
        assert_eq!(
            canonical_run_hash(&run_a()),
            canonical_run_hash(&run_b()),
            "equal projections must produce equal run-hashes"
        );
    }

    #[test]
    fn ephemeral_kinds_are_dropped() {
        let canon = canonicalize(&run_a());
        assert!(
            canon
                .iter()
                .all(|e| e.kind != "turn_heartbeat" && e.kind != "turn_chunk"),
            "heartbeats and chunks must not survive: {canon:#?}"
        );
        // run_a has 5 raw events, 2 ephemeral → 3 functional.
        assert_eq!(canon.len(), 3, "expected 3 functional events, got {canon:#?}");
    }

    #[test]
    fn volatile_payload_fields_are_scrubbed() {
        let canon = canonicalize(&run_a());
        let started = canon
            .iter()
            .find(|e| e.kind == "session_started")
            .expect("session_started must survive");
        assert!(
            started.payload.get("cwd").is_none(),
            "cwd must be scrubbed: {:?}",
            started.payload
        );
        assert_eq!(started.payload.get("prompt").unwrap(), "do the thing");

        let completed = canon
            .iter()
            .find(|e| e.kind == "session_completed")
            .expect("session_completed must survive");
        assert!(
            completed.payload.get("summary_path").is_none()
                && completed.payload.get("events_path").is_none(),
            "*_path keys must be scrubbed: {:?}",
            completed.payload
        );
        assert_eq!(completed.payload.get("failed").unwrap(), false);
    }

    #[test]
    fn different_functional_event_changes_projection_and_hash() {
        let base = run_a();
        let base_hash = canonical_run_hash(&base);

        // (a) different stable payload field (exit_code 0 → 1).
        let mut diff_payload = run_a();
        diff_payload[3].payload = json!({"round": 1, "role": "qa", "agent": "claude:sonnet", "exit_code": 1, "timed_out": false, "text": "done"});
        assert_ne!(
            canonicalize(&base),
            canonicalize(&diff_payload),
            "different exit_code must change the projection"
        );
        assert_ne!(base_hash, canonical_run_hash(&diff_payload));

        // (b) different role.
        let mut diff_role = run_a();
        diff_role[3].role = "architect".to_owned();
        assert_ne!(base_hash, canonical_run_hash(&diff_role));

        // (c) different kind.
        let mut diff_kind = run_a();
        diff_kind[3].kind = EventKind::TurnFailed;
        assert_ne!(base_hash, canonical_run_hash(&diff_kind));
    }

    #[test]
    fn canonicalize_is_idempotent_and_order_independent() {
        let a = run_a();
        // Idempotent: re-canonicalizing the same input is stable.
        assert_eq!(canonicalize(&a), canonicalize(&a));

        // Order-independent: reversing the input yields the same projection.
        let mut reversed = a.clone();
        reversed.reverse();
        assert_eq!(
            canonicalize(&a),
            canonicalize(&reversed),
            "canonicalize must be order-independent"
        );
        assert_eq!(canonical_run_hash(&a), canonical_run_hash(&reversed));
    }

    #[test]
    fn scrub_normalizes_whole_value_paths_but_not_prose() {
        let mut v = json!({
            "file": "layer-reports/123-qa-qa.md",   // volatile key → removed
            "text": "wrote output to /Users/x/out.md", // prose → untouched
            "loc": "/Users/x/out.md",                // whole-value path → "<path>"
            "rel": "src/main.rs"                      // relative → untouched
        });
        scrub_volatile(&mut v);
        assert!(v.get("file").is_none(), "volatile key 'file' must be removed");
        assert_eq!(v.get("loc").unwrap(), "<path>");
        assert_eq!(v.get("text").unwrap(), "wrote output to /Users/x/out.md");
        assert_eq!(v.get("rel").unwrap(), "src/main.rs");
    }

    #[test]
    fn other_kinds_pass_through_scrubbed() {
        let events = vec![ev(
            EventKind::Other("learned_routing_applied".to_owned()),
            "qa",
            json!({"role": "qa", "agents": ["claude:sonnet"], "min_observations": 3, "source": "telemetry", "elapsed_ms": 42}),
            1,
            1_000,
            "session-AAAA",
            "auto",
            None,
            "discussion",
        )];
        let canon = canonicalize(&events);
        assert_eq!(canon.len(), 1);
        assert_eq!(canon[0].kind, "learned_routing_applied");
        assert!(
            canon[0].payload.get("elapsed_ms").is_none(),
            "nested *_ms must be scrubbed from Other-kind payloads"
        );
        assert_eq!(canon[0].payload.get("source").unwrap(), "telemetry");
    }

    #[test]
    fn run_hash_is_stable_hex() {
        // Pin the format: 16 lowercase hex chars, and deterministic for a fixed
        // input (guards accidental nondeterminism in serialization).
        let h = canonical_run_hash(&run_a());
        assert_eq!(h.len(), 16, "FNV-1a/64 hex is 16 chars: {h}");
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(h, canonical_run_hash(&run_a()), "hash must be deterministic");
    }
}
