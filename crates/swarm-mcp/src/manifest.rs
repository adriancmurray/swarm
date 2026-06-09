//! Package manifest builders for Agent Swarm.

use std::path::PathBuf;

use swarm_kernel::resolver::{locate_claude, locate_codex};

/// Builds the `swarm.manifest/v1` manifest JSON.
///
/// Backend availability is resolved at call time from PATH and fallback
/// locations. Tests should assert the shape of backend entries, not the local
/// boolean value or resolved path.
pub fn manifest_payload() -> serde_json::Value {
    serde_json::json!({
        "$schema": "swarm.manifest/v1",
        "id": "agent-swarm",
        "kind": "agent-orchestrator",
        "version": env!("CARGO_PKG_VERSION"),
        "display_name": "Agent Swarm",
        "description": "Routes work to local frontier agent CLIs and orchestrates manager/worker swarms.",
        "entrypoints": {
            "cli": "agent-swarm",
            "mcp": "agent-swarm-mcp"
        },
        "capabilities": [
            "agent.consult",
            "agent.delegate",
            "agent.swarm",
            "agent.fanout",
            "agent.discussion",
            "agent.audit",
            "agent.design_review",
            "agent.background_jobs",
            "agent.session_events",
            "agent.monitoring",
            "agent.alerts",
            "agent.watch",
            "agent.api_docs_followup",
            "agent.lazy_telemetry",
            "agent.routing_insights",
            "agent.routing_feedback",
            "agent.profiles",
            "agent.profile_helpers",
            "agent.bidirectional_events",
            "agent.deterministic_hooks",
            "agent.common_presets",
            "agent.executable_presets",
            "agent.activity_records",
            "backend.claude",
            "backend.codex"
        ],
        "commands": {
            "run": "agent-swarm run --agent auto --quiet \"<task>\"",
            "fanout": "agent-swarm fanout --manager claude:sonnet --worker architecture=claude:sonnet --worker implementation=codex --worker review=claude:haiku \"<task>\"",
            "swarm": "agent-swarm swarm --manager claude:sonnet --worker architecture=claude:sonnet --worker implementation=codex --worker review=claude:haiku \"<task>\"",
            "discuss": "agent-swarm discuss --rounds 1 --manager claude:sonnet --participant architecture=codex --participant code-quality=claude:haiku --participant implementation=codex \"<task>\"",
            "design": "agent-swarm design --focus all --rounds 2 \"<design objective>\"",
            "audit": "agent-swarm audit --focus all --rounds 2 --docs \"<audit objective>\"",
            "status": "agent-swarm status [JOB_ID]",
            "result": "agent-swarm result [JOB_ID]",
            "cancel": "agent-swarm cancel JOB_ID",
            "sessions": "agent-swarm sessions",
            "events": "agent-swarm events SESSION_ID",
            "transcript": "agent-swarm transcript SESSION_ID",
            "insights": "agent-swarm insights",
            "profiles": "agent-swarm profiles",
            "hooks": "agent-swarm hooks",
            "presets": "agent-swarm presets",
            "preset": "agent-swarm preset codebase-audit \"<task>\"",
            "recommend": "agent-swarm recommend --classifier auto \"<task>\"",
            "eval-metadirector": "agent-swarm eval-metadirector --arm all --classifier auto --classifier-threshold 80 --packet-budget 300 --manager-prompt-limit 2000",
            "ledger": "agent-swarm ledger add --id ID --intent \"<intent>\" | agent-swarm ledger working-set",
            "feedback": "agent-swarm feedback --role ROLE --agent AGENT --outcome win|loss [--session SESSION_ID] [--note NOTE]",
            "activity-record": "agent-swarm activity-record --session SESSION --node NODE --parent PARENT --depth 1 --label \"Worker\" --status running",
            "monitor-start": "agent-swarm monitor-start",
            "monitor-status": "agent-swarm monitor-status",
            "alerts": "agent-swarm alerts [--since TS_MS] [--limit N]",
            "watch": "agent-swarm watch [--heartbeat-secs 30]",
            "manifest": "agent-swarm manifest"
        },
        "mcp": {
            "transport": "stdio",
            "endpoint": "~/.swarm/bin/agent-swarm-mcp",
            "command": "agent-swarm mcp",
            "service_name": "agent-swarm",
            "tags": ["agent", "swarm", "mcp"],
            "tools": mcp_tool_names()
        },
        "stores": [
            {
                "id": "partner_jobs",
                "path": "~/.swarm/jobs",
                "description": "Local background job records and captured outputs."
            },
            {
                "id": "agent_swarm_sessions",
                "path": "~/.swarm/sessions",
                "description": "Discussion session metadata, JSONL event streams, transcripts, summaries, and docs follow-up outputs."
            },
            {
                "id": "agent_swarm_telemetry",
                "path": "~/.swarm/telemetry",
                "description": "Append-only agent outcome observations. Insights are derived lazily so future UIs can consume the same contract."
            },
            {
                "id": "agent_swarm_monitor",
                "path": "~/.swarm/monitor",
                "description": "Sidecar monitor heartbeat, RSS spike alerts, stale job/session alerts, and bounded alert history."
            }
        ],
        "integration": {
            "package_registry_path": "~/.swarm/packages/agent-swarm.json",
            "peers": {
                "uses_services_json": true,
                "service_name": "agent-swarm",
                "mcp_endpoint": "~/.swarm/bin/agent-swarm-mcp",
                "tags": ["agent", "swarm", "mcp"],
                "reason": "agent-swarm is an invoked stdio MCP endpoint, not an always-running TCP daemon."
            },
            "agent_tools": [
                "agent_job_start",
                "agent_job_status",
                "agent_job_result",
                "agent_job_cancel"
            ],
            "discovery": {
                "discovery": "Read ~/.swarm/packages/agent-swarm.json, run `agent-swarm manifest`, or connect to the registered agent-swarm MCP service.",
                "launch": "Invoke the CLI entrypoint from the manifest or call the stdio MCP tools."
            }
        },
        "skills": [
            {
                "harness": "claude-code",
                "path": "~/.claude/skills/agent-swarm",
                "skill_file": "SKILL.md"
            },
            {
                "harness": "codex",
                "path": "~/.codex/skills/agent-swarm",
                "skill_file": "SKILL.md",
                "agent_descriptor": "agents/openai.yaml"
            }
        ],
        "backends": [
            backend_manifest("claude", "claude", locate_claude()),
            backend_manifest("codex", "codex", locate_codex())
        ]
    })
}

/// Returns the flat MCP tool name list advertised in the package manifest.
///
/// Keep this in sync with `mcp_tools`; contract tests assert parity.
pub fn mcp_tool_names() -> Vec<&'static str> {
    vec![
        "agent_swarm_manifest",
        "agent_swarm_run",
        "agent_swarm_swarm",
        "agent_swarm_fanout",
        "agent_swarm_discuss",
        "agent_swarm_audit",
        "agent_swarm_discuss_start",
        "agent_swarm_audit_start",
        "agent_swarm_design",
        "agent_swarm_insights",
        "agent_swarm_profiles",
        "agent_swarm_automation_hooks",
        "agent_swarm_presets",
        "agent_swarm_preset",
        "agent_swarm_recommend",
        "agent_swarm_feedback",
        "agent_swarm_proposals",
        "agent_swarm_proposal_record",
        "agent_swarm_proposal_vote",
        "agent_swarm_activity_record",
        "agent_swarm_job_start",
        "agent_swarm_job_status",
        "agent_swarm_job_result",
        "agent_swarm_job_cancel",
        "agent_swarm_session_list",
        "agent_swarm_session_events",
        "agent_swarm_session_transcript",
        "agent_swarm_session_summary",
        "agent_swarm_session_artifacts",
        "agent_swarm_runtime_processes",
        "agent_swarm_monitor_status",
        "agent_swarm_monitor_start",
        "agent_swarm_alerts",
        "agent_swarm_context_gather",
        "agent_swarm_overview",
        "agent_swarm_settings_get",
        "agent_swarm_settings_set",
    ]
}

fn backend_manifest(id: &str, binary: &str, path: Option<PathBuf>) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "binary": binary,
        "available": path.is_some(),
        "path": path.map(|path| path.display().to_string())
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::PathBuf;

    use super::{backend_manifest, manifest_payload};

    fn object_keys(value: &serde_json::Value) -> BTreeSet<String> {
        value
            .as_object()
            .unwrap()
            .keys()
            .map(|key| key.to_string())
            .collect()
    }

    #[test]
    fn manifest_exposes_package_contract() {
        let manifest = manifest_payload();
        assert_eq!(manifest["$schema"], "swarm.manifest/v1");
        assert_eq!(manifest["id"], "agent-swarm");
        assert_eq!(
            object_keys(&manifest),
            BTreeSet::from([
                "$schema".to_string(),
                "backends".to_string(),
                "capabilities".to_string(),
                "commands".to_string(),
                "description".to_string(),
                "display_name".to_string(),
                "entrypoints".to_string(),
                "id".to_string(),
                "integration".to_string(),
                "kind".to_string(),
                "mcp".to_string(),
                "skills".to_string(),
                "stores".to_string(),
                "version".to_string(),
            ])
        );
        assert_eq!(manifest["mcp"]["transport"], "stdio");
        assert_eq!(manifest["mcp"]["command"], "agent-swarm mcp");
        assert_eq!(
            object_keys(&manifest["mcp"]),
            BTreeSet::from([
                "command".to_string(),
                "endpoint".to_string(),
                "service_name".to_string(),
                "tags".to_string(),
                "tools".to_string(),
                "transport".to_string(),
            ])
        );
        assert_eq!(manifest["entrypoints"]["cli"], "agent-swarm");
        assert_eq!(
            object_keys(&manifest["entrypoints"]),
            BTreeSet::from(["cli".to_string(), "mcp".to_string()])
        );
        assert!(manifest["capabilities"].as_array().unwrap().len() >= 5);
        assert!(manifest["capabilities"]
            .as_array()
            .unwrap()
            .contains(&serde_json::Value::String("agent.fanout".to_string())));
        assert!(manifest["commands"]["fanout"]
            .as_str()
            .unwrap()
            .contains("agent-swarm fanout"));
        assert!(manifest["commands"]["design"]
            .as_str()
            .unwrap()
            .contains("agent-swarm design"));
        assert_eq!(manifest["integration"]["peers"]["uses_services_json"], true);
        let backends = manifest["backends"].as_array().unwrap();
        assert_eq!(backends.len(), 2);
        for backend in backends {
            assert_eq!(
                object_keys(backend),
                BTreeSet::from([
                    "available".to_string(),
                    "binary".to_string(),
                    "id".to_string(),
                    "path".to_string(),
                ])
            );
            assert!(backend["id"].is_string());
            assert!(backend["binary"].is_string());
            assert!(backend["available"].is_boolean());
            assert!(backend["path"].is_string() || backend["path"].is_null());
        }
    }

    #[test]
    fn manifest_integration_block_uses_generic_keys() {
        let manifest = manifest_payload();
        assert_eq!(
            object_keys(&manifest["integration"]),
            BTreeSet::from([
                "package_registry_path".to_string(),
                "peers".to_string(),
                "agent_tools".to_string(),
                "discovery".to_string(),
            ])
        );
        assert_eq!(
            manifest["integration"]["peers"]["service_name"],
            "agent-swarm"
        );
        assert!(manifest["integration"]["agent_tools"]
            .as_array()
            .unwrap()
            .iter()
            .all(|tool| tool.as_str().unwrap().starts_with("agent_job_")));
    }

    #[test]
    fn backend_manifest_records_available_and_missing_paths() {
        let present = backend_manifest("claude", "claude", Some(PathBuf::from("/usr/bin/claude")));
        assert_eq!(present["id"], "claude");
        assert_eq!(present["binary"], "claude");
        assert_eq!(present["available"], true);
        assert_eq!(present["path"], "/usr/bin/claude");

        let absent = backend_manifest("codex", "codex", None);
        assert_eq!(absent["id"], "codex");
        assert_eq!(absent["binary"], "codex");
        assert_eq!(absent["available"], false);
        assert!(absent["path"].is_null());
    }
}
