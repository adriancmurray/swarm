//! Backend availability checks and classified error payloads.

use std::collections::HashSet;

use swarm_kernel::agent::{describe_spec, AgentSpec};
use swarm_kernel::args::WorkerSpec;
use swarm_kernel::events::EventKind;
use swarm_kernel::resolver::agent_invocation_available;

use crate::backend_registry::BackendRegistry;
use crate::session::DiscussionSession;

/// Availability for one spec: built-ins locate their CLI; custom specs resolve
/// against the registry (unknown id is the blocking issue) and then `ready()`.
fn spec_available(spec: &AgentSpec, registry: &BackendRegistry) -> Result<(), String> {
    match &spec.custom {
        Some(id) => registry
            .resolve(id)
            .and_then(|backend| backend.ready().map_err(|e| format!("Error: {e}"))),
        None => agent_invocation_available(spec.agent),
    }
}

pub fn run_session_preflight(
    session: &DiscussionSession,
    registry: &BackendRegistry,
    manager: &AgentSpec,
    participants: &[WorkerSpec],
) -> Result<(), String> {
    session.append_event(
        EventKind::PreflightStarted,
        serde_json::json!({
            "manager": describe_spec(manager),
            "participants": participants.iter().map(|participant| {
                serde_json::json!({
                    "role": &participant.role,
                    "agent": describe_spec(&participant.spec)
                })
            }).collect::<Vec<_>>()
        }),
    )?;

    let mut seen = HashSet::new();
    let mut issues = Vec::new();
    for spec in std::iter::once(manager).chain(participants.iter().map(|worker| &worker.spec)) {
        let key = describe_spec(spec);
        if !seen.insert(key.clone()) {
            continue;
        }
        if let Err(err) = spec_available(spec, registry) {
            issues.push(serde_json::json!({
                "agent": key,
                "category": classify_error(&err),
                "severity": "blocking",
                "error": err,
                "suggested_action": suggested_action_for_error(&err)
            }));
        }
    }

    if issues.is_empty() {
        session.append_event(
            EventKind::PreflightCompleted,
            serde_json::json!({"ok": true}),
        )?;
        Ok(())
    } else {
        let summary = issues
            .iter()
            .filter_map(|issue| issue.get("error").and_then(|value| value.as_str()))
            .collect::<Vec<_>>()
            .join("; ");
        session.append_event(
            EventKind::PreflightFailed,
            serde_json::json!({
                "ok": false,
                "issues": issues,
                "error": summary,
                "category": "preflight",
                "severity": "blocking",
                "suggested_action": "Adjust participants or start/authenticate the missing backend, then rerun the swarm."
            }),
        )?;
        Err(format!("Preflight failed: {summary}"))
    }
}

pub fn classified_agent_error_payload(
    round: u32,
    participant: &WorkerSpec,
    error: &str,
) -> serde_json::Value {
    let category = classify_error(error);
    serde_json::json!({
        "round": round,
        "role": &participant.role,
        "agent": describe_spec(&participant.spec),
        "error": error,
        "category": category,
        "severity": if category == "timeout" { "high" } else { "blocking" },
        "suggested_action": suggested_action_for_error(error)
    })
}

pub fn classified_error_payload(label: &str, error: &str) -> serde_json::Value {
    let category = classify_error(error);
    serde_json::json!({
        "label": label,
        "error": error,
        "category": category,
        "severity": "blocking",
        "suggested_action": suggested_action_for_error(error)
    })
}

pub fn classify_error(error: &str) -> &'static str {
    let lower = error.to_ascii_lowercase();
    if lower.contains("could not locate") || lower.contains("not found") {
        "missing-backend"
    } else if lower.contains("timed out") || lower.contains("timeout") {
        "timeout"
    } else if lower.contains("auth") || lower.contains("permission") || lower.contains("credential")
    {
        "auth-or-permission"
    } else {
        "runtime"
    }
}

fn suggested_action_for_error(error: &str) -> &'static str {
    match classify_error(error) {
        "missing-backend" => "Install or expose the selected agent CLI on PATH, then rerun preflight.",
        "timeout" => "Inspect the transcript, increase --timeout, or split the task into smaller parallel workers.",
        "auth-or-permission" => "Authenticate the selected agent CLI and rerun the swarm.",
        _ => "Inspect stderr/transcript, then rerun the failed participant with a narrower prompt.",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use swarm_kernel::agent::{AgentChoice, AgentSpec};

    fn participant() -> WorkerSpec {
        WorkerSpec {
            role: "qa".to_string(),
            spec: AgentSpec::builtin(AgentChoice::Claude, Some("sonnet".to_string())),
            timeout_secs: None,
        }
    }

    #[test]
    fn classify_error_maps_known_categories() {
        assert_eq!(
            classify_error("could not locate some-agent"),
            "missing-backend"
        );
        assert_eq!(classify_error("timed out waiting"), "timeout");
        assert_eq!(classify_error("permission denied"), "auth-or-permission");
        assert_eq!(classify_error("unexpected stderr"), "runtime");
    }

    #[test]
    fn classified_agent_error_payload_marks_timeout_high() {
        let payload = classified_agent_error_payload(2, &participant(), "timeout");

        assert_eq!(payload["round"], 2);
        assert_eq!(payload["role"], "qa");
        assert_eq!(payload["agent"], "claude:sonnet");
        assert_eq!(payload["category"], "timeout");
        assert_eq!(payload["severity"], "high");
        assert!(payload["suggested_action"]
            .as_str()
            .unwrap()
            .contains("increase --timeout"));
    }

    #[test]
    fn classified_error_payload_uses_blocking_default() {
        let payload = classified_error_payload("thread-panic", "participant thread panicked");

        assert_eq!(payload["label"], "thread-panic");
        assert_eq!(payload["category"], "runtime");
        assert_eq!(payload["severity"], "blocking");
    }
}
