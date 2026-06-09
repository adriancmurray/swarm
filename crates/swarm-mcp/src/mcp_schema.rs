//! MCP tool schema metadata for Agent Swarm.

use swarm_contracts::mcp::McpToolDescriptor;

/// Declarative descriptor table — single source of truth for all MCP tools.
///
/// Both `mcp_tools()` (wire JSON) and `gen_mcp_tools` (frozen artifact) derive
/// from this function. Helper schema fns (`empty_mcp_schema`, `session_schema`,
/// etc.) are private to this module and used only here.
pub fn mcp_tool_descriptors() -> Vec<McpToolDescriptor> {
    vec![
        McpToolDescriptor {
            name: "agent_swarm_manifest".into(),
            description: "Return Agent Swarm's package manifest and detected backend availability.".into(),
            input_schema: empty_mcp_schema(),
        },
        McpToolDescriptor {
            name: "agent_swarm_insights".into(),
            description: "Return lazy aggregate routing insights derived from append-only agent outcome telemetry.".into(),
            input_schema: empty_mcp_schema(),
        },
        McpToolDescriptor {
            name: "agent_swarm_profiles".into(),
            description: "List dedicated agent profiles with role matches, helper agents, automation hooks, and deterministic checks.".into(),
            input_schema: empty_mcp_schema(),
        },
        McpToolDescriptor {
            name: "agent_swarm_automation_hooks".into(),
            description: "List deterministic host-only automation hooks that profiles can request without baking codegen tools into the substrate.".into(),
            input_schema: empty_mcp_schema(),
        },
        McpToolDescriptor {
            name: "agent_swarm_presets".into(),
            description: "List common swarm presets for architecture councils, audits, UI polish, regression hunts, and API docs follow-up.".into(),
            input_schema: empty_mcp_schema(),
        },
        McpToolDescriptor {
            name: "agent_swarm_preset".into(),
            description: "Execute a named common swarm preset by id.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "preset_id": {"type": "string", "enum": ["architecture-council", "codebase-audit", "ui-polish", "regression-hunt", "api-docs-followup"]},
                    "prompt": {"type": "string"},
                    "cwd": {"type": "string"},
                    "timeout_secs": {"type": "integer", "minimum": 1},
                    "helpers": {"type": "boolean", "default": true, "description": "Enable profile-selected one-layer helper agents."}
                },
                "required": ["preset_id", "prompt"]
            }),
        },
        McpToolDescriptor {
            name: "agent_swarm_recommend".into(),
            description: "Recommend manager and participant specs for a task using learned telemetry plus safe defaults.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "prompt": {"type": "string", "description": "Task or objective to route"}
                },
                "required": ["prompt"]
            }),
        },
        McpToolDescriptor {
            name: "agent_swarm_feedback".into(),
            description: "Record explicit user feedback for an agent/role so future routing can learn over time.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "role": {"type": "string", "description": "Routing role, such as architecture, hardening, api-docs, or manager"},
                    "agent": {"type": "string", "description": "Agent spec that received feedback, such as gemini or claude:sonnet"},
                    "outcome": {"type": "string", "enum": ["win", "loss", "success", "failure", "helpful", "unhelpful"]},
                    "session_id": {"type": "string"},
                    "note": {"type": "string"}
                },
                "required": ["role", "agent", "outcome"]
            }),
        },
        McpToolDescriptor {
            name: "agent_swarm_proposals".into(),
            description: "List open learning-layer proposals and agent votes.".into(),
            input_schema: empty_mcp_schema(),
        },
        McpToolDescriptor {
            name: "agent_swarm_proposal_record".into(),
            description: "Record a proposal for future swarm behavior, routing, UI, or architecture changes.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "title": {"type": "string"},
                    "body": {"type": "string"},
                    "session_id": {"type": "string"},
                    "proposed_by": {"type": "string", "description": "Agent, user, or system that proposed the change."},
                    "tags": {"type": "array", "items": {"type": "string"}}
                },
                "required": ["title", "body"]
            }),
        },
        McpToolDescriptor {
            name: "agent_swarm_proposal_vote".into(),
            description: "Append an agent or user vote to a proposal. Votes are advisory and visible in insights.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "proposal_id": {"type": "string"},
                    "voter": {"type": "string", "description": "Agent spec, role, or user label."},
                    "vote": {"type": "string", "enum": ["approve", "reject", "defer", "yes", "no", "+1", "-1"]},
                    "rationale": {"type": "string"}
                },
                "required": ["proposal_id", "voter", "vote"]
            }),
        },
        McpToolDescriptor {
            name: "agent_swarm_activity_record".into(),
            description: "Append a harness-neutral live activity record so any runtime can appear in the Agent Swarm graph.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "session_id": {"type": "string", "description": "Stable activity/session id that groups related nodes."},
                    "node_id": {"type": "string", "description": "Stable node id. Defaults to session_id for a director/root record."},
                    "parent_id": {"type": "string", "description": "Parent node id when this is a child worker/sub-agent."},
                    "depth": {"type": "integer", "minimum": 0, "description": "Graph depth. Defaults to 0 without parent_id, otherwise 1."},
                    "label": {"type": "string"},
                    "status": {"type": "string", "description": "running, idle, completed, failed, lost, or another future-safe status."},
                    "cwd": {"type": "string"},
                    "agent_type": {"type": "string", "description": "codex, claude, gemini, agent, app, etc."},
                    "prompt_preview": {"type": "string", "description": "Bounded preview only; do not send full prompts or secrets."},
                    "cullable_handle": {"type": "string"},
                    "policy_state": {"type": "string"},
                    "slice_id": {"type": "string"},
                    "tool_use_id": {"type": "string"},
                    "agent_id": {"type": "string"},
                    "total_tokens": {"type": "integer", "minimum": 0},
                    "total_duration_ms": {"type": "integer", "minimum": 0}
                },
                "required": ["session_id"]
            }),
        },
        McpToolDescriptor {
            name: "agent_swarm_run".into(),
            description: "Dispatch one prompt to Gemini, Claude, Codex, or auto-selected backend. Defaults to consult/read-only mode.".into(),
            input_schema: common_run_schema(true),
        },
        McpToolDescriptor {
            name: "agent_swarm_swarm".into(),
            description: "Run a tracked manager/worker fan-out swarm and return the manager synthesis.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "prompt": {"type": "string", "description": "Task for the swarm"},
                    "manager": {"type": "string", "description": "Manager backend, e.g. claude:sonnet"},
                    "workers": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Worker specs such as architecture=gemini or implementation=codex"
                    },
                    "cwd": {"type": "string"},
                    "timeout_secs": {"type": "integer", "minimum": 1}
                },
                "required": ["prompt"]
            }),
        },
        McpToolDescriptor {
            name: "agent_swarm_fanout".into(),
            description: "Alias for agent_swarm_swarm; run parallel workers with session events for live UIs.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "prompt": {"type": "string", "description": "Task for the swarm"},
                    "manager": {"type": "string", "description": "Manager backend, e.g. claude:sonnet"},
                    "workers": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Worker specs such as architecture=gemini or implementation=codex"
                    },
                    "cwd": {"type": "string"},
                    "timeout_secs": {"type": "integer", "minimum": 1}
                },
                "required": ["prompt"]
            }),
        },
        McpToolDescriptor {
            name: "agent_swarm_discuss".into(),
            description: "Run a bounded multi-round discussion between agents, emit JSONL events, and return manager synthesis.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "prompt": {"type": "string", "description": "Task for the discussion"},
                    "manager": {"type": "string", "description": "Manager backend, e.g. claude:sonnet"},
                    "participants": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Participant specs such as architecture=gemini or code-quality=claude:sonnet"
                    },
                    "rounds": {"type": "integer", "minimum": 1, "default": 2},
                    "docs": {"type": "boolean", "default": false, "description": "Run a trailing API-docs recommendation subagent."},
                    "helpers": {"type": "boolean", "default": false, "description": "Enable profile-selected one-layer helper agents."},
                    "docs_agent": {"type": "string", "description": "Backend for the docs follow-up, e.g. claude:sonnet"},
                    "cwd": {"type": "string"},
                    "timeout_secs": {"type": "integer", "minimum": 1}
                },
                "required": ["prompt"]
            }),
        },
        McpToolDescriptor {
            name: "agent_swarm_audit".into(),
            description: "Run a preset codebase audit discussion with local peer context, manager synthesis, session events, and optional API-docs follow-up.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "prompt": {"type": "string", "description": "Audit objective"},
                    "focus": {
                        "type": "string",
                        "enum": ["all", "simplify", "harden", "architecture", "api-docs", "tests"],
                        "default": "all",
                        "description": "Audit emphasis."
                    },
                    "manager": {"type": "string", "description": "Manager backend, e.g. claude:sonnet"},
                    "participants": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Override participant specs such as architecture=gemini"
                    },
                    "rounds": {"type": "integer", "minimum": 1, "default": 2},
                    "docs": {"type": "boolean", "default": true, "description": "Run a trailing API-docs recommendation subagent."},
                    "no_docs": {"type": "boolean", "default": false, "description": "Disable the default docs follow-up."},
                    "helpers": {"type": "boolean", "default": false, "description": "Enable profile-selected one-layer helper agents."},
                    "docs_agent": {"type": "string", "description": "Backend for the docs follow-up, e.g. claude:sonnet"},
                    "cwd": {"type": "string"},
                    "timeout_secs": {"type": "integer", "minimum": 1}
                },
                "required": ["prompt"]
            }),
        },
        McpToolDescriptor {
            name: "agent_swarm_discuss_start".into(),
            description: "Start a multi-agent discussion in a local background process and return immediately with a job id for live monitoring.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "prompt": {"type": "string", "description": "Task for the discussion"},
                    "manager": {"type": "string", "description": "Manager backend, e.g. claude:sonnet"},
                    "participants": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Participant specs such as architecture=gemini or code-quality=claude:sonnet"
                    },
                    "rounds": {"type": "integer", "minimum": 1, "default": 2},
                    "docs": {"type": "boolean", "default": false},
                    "helpers": {"type": "boolean", "default": false},
                    "docs_agent": {"type": "string"},
                    "cwd": {"type": "string"},
                    "timeout_secs": {"type": "integer", "minimum": 1}
                },
                "required": ["prompt"]
            }),
        },
        McpToolDescriptor {
            name: "agent_swarm_audit_start".into(),
            description: "Start a codebase audit council in a local background process and return immediately with a job id for live monitoring.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "prompt": {"type": "string", "description": "Audit objective"},
                    "focus": {
                        "type": "string",
                        "enum": ["all", "simplify", "harden", "architecture", "api-docs", "tests"],
                        "default": "all"
                    },
                    "manager": {"type": "string"},
                    "participants": {
                        "type": "array",
                        "items": {"type": "string"}
                    },
                    "rounds": {"type": "integer", "minimum": 1, "default": 2},
                    "docs": {"type": "boolean", "default": true},
                    "no_docs": {"type": "boolean", "default": false},
                    "helpers": {"type": "boolean", "default": false},
                    "docs_agent": {"type": "string"},
                    "cwd": {"type": "string"},
                    "timeout_secs": {"type": "integer", "minimum": 1}
                },
                "required": ["prompt"]
            }),
        },
        McpToolDescriptor {
            name: "agent_swarm_design".into(),
            description: "Run a design-centered review council for product UI, motion, interaction, accessibility, and implementation planning.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "prompt": {"type": "string", "description": "Design objective"},
                    "focus": {
                        "type": "string",
                        "enum": ["all", "visual-system", "motion", "interaction", "accessibility", "implementation"],
                        "default": "all",
                        "description": "Design review emphasis."
                    },
                    "manager": {"type": "string", "description": "Manager backend, e.g. claude:sonnet"},
                    "participants": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Override participant specs such as product-design=gemini"
                    },
                    "rounds": {"type": "integer", "minimum": 1, "default": 2},
                    "docs": {"type": "boolean", "default": false, "description": "Run a trailing API-docs recommendation subagent."},
                    "helpers": {"type": "boolean", "default": false, "description": "Enable profile-selected one-layer helper agents."},
                    "docs_agent": {"type": "string", "description": "Backend for the docs follow-up, e.g. claude:sonnet"},
                    "cwd": {"type": "string"},
                    "timeout_secs": {"type": "integer", "minimum": 1}
                },
                "required": ["prompt"]
            }),
        },
        McpToolDescriptor {
            name: "agent_swarm_job_start".into(),
            description: "Start a lightweight local Agent Swarm background job. Defaults to consult/read-only mode unless quiet=false.".into(),
            input_schema: common_run_schema(true),
        },
        McpToolDescriptor {
            name: "agent_swarm_job_status".into(),
            description: "List recent Agent Swarm background jobs or inspect one by id.".into(),
            input_schema: optional_job_schema(),
        },
        McpToolDescriptor {
            name: "agent_swarm_job_result".into(),
            description: "Return the result for one Agent Swarm background job, or the latest job when omitted.".into(),
            input_schema: optional_job_schema(),
        },
        McpToolDescriptor {
            name: "agent_swarm_job_cancel".into(),
            description: "Cancel a queued or running Agent Swarm background job.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "job_id": {"type": "string", "description": "Job id to cancel"}
                },
                "required": ["job_id"]
            }),
        },
        McpToolDescriptor {
            name: "agent_swarm_session_list".into(),
            description: "List recent multi-agent discussion sessions.".into(),
            input_schema: empty_mcp_schema(),
        },
        McpToolDescriptor {
            name: "agent_swarm_session_events".into(),
            description: "Return the JSONL event stream for a discussion session.".into(),
            input_schema: session_schema(),
        },
        McpToolDescriptor {
            name: "agent_swarm_session_transcript".into(),
            description: "Return the Markdown transcript for a discussion session.".into(),
            input_schema: session_schema(),
        },
        McpToolDescriptor {
            name: "agent_swarm_session_summary".into(),
            description: "Return bounded structured status, digest, and artifact pointers for one session.".into(),
            input_schema: session_schema(),
        },
        McpToolDescriptor {
            name: "agent_swarm_session_artifacts".into(),
            description: "Return filesystem artifact paths for one session without reconstructing paths in the client.".into(),
            input_schema: session_schema(),
        },
        McpToolDescriptor {
            name: "agent_swarm_runtime_processes".into(),
            description: "List tracked live/lost session and background-job processes for swarm monitoring.".into(),
            input_schema: empty_mcp_schema(),
        },
        McpToolDescriptor {
            name: "agent_swarm_monitor_status".into(),
            description: "Return whether the Agent Swarm resource monitor sidecar is running and where alerts are stored.".into(),
            input_schema: empty_mcp_schema(),
        },
        McpToolDescriptor {
            name: "agent_swarm_monitor_start".into(),
            description: "Start or replace the lightweight Agent Swarm monitor sidecar for RSS spike, stale job, and heartbeat alerts.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "interval_secs": {"type": "integer", "minimum": 1, "default": 5},
                    "rss_mb": {"type": "integer", "minimum": 1, "default": 4096},
                    "spike_factor": {"type": "number", "minimum": 1.1, "default": 2.5},
                    "stale_secs": {"type": "integer", "minimum": 30, "default": 300},
                    "replace": {"type": "boolean", "default": false, "description": "Restart the sidecar when it is already running so new thresholds take effect."}
                }
            }),
        },
        McpToolDescriptor {
            name: "agent_swarm_alerts".into(),
            description: "Return recent monitor alerts and heartbeat events for agents and dashboards.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "since_ts_ms": {"type": "integer", "minimum": 0},
                    "limit": {"type": "integer", "minimum": 1, "maximum": 500, "default": 50}
                }
            }),
        },
        McpToolDescriptor {
            name: "agent_swarm_context_gather".into(),
            description: "Gather bounded read-only local context for a query, preferring ranked paths and excerpts over manifest dumps.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string"},
                    "cwd": {"type": "string"},
                    "budget_tokens": {"type": "integer", "minimum": 128, "default": 1200}
                },
                "required": ["query"]
            }),
        },
        McpToolDescriptor {
            name: "agent_swarm_overview".into(),
            description: "Return a single aggregated digest of all current swarm activity: running/recent sessions, running/recent jobs, active monitor alerts, and summary counts. One call instead of polling sessions + jobs + alerts separately.".into(),
            input_schema: empty_mcp_schema(),
        },
        McpToolDescriptor {
            name: "agent_swarm_settings_get".into(),
            description: "Return the current [settings] section of the Agent Swarm config as JSON. Settings are adjustable via agent_swarm_settings_set and take effect on the next invocation.".into(),
            input_schema: empty_mcp_schema(),
        },
        McpToolDescriptor {
            name: "agent_swarm_settings_set".into(),
            description: "Write a new [settings] section to the Agent Swarm config. All other config content is preserved (default_timeout, default_agent, [reliability], [routes], etc.). Changes take effect on the next invocation since config is loaded once per process.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "docs_default": {
                        "type": "boolean",
                        "description": "Enable the API-docs follow-up worker by default in discuss/design runs without --docs. Audit's built-in docs-ON is unaffected."
                    }
                }
            }),
        },
    ]
}

/// Serialize the descriptor table to pretty-printed JSON with a trailing newline.
///
/// This is the **single serialization path** shared by both `gen_mcp_tools` (for the
/// frozen artifact) and the idempotency test. Using one function guarantees byte-identity:
/// same 2-space indent, same key ordering (insertion order via `serde_json::json!`),
/// same trailing newline. `serde_json::to_string_pretty` does NOT emit a trailing `\n`
/// on its own — the `+ "\n"` here is intentional.
pub fn mcp_tools_pretty_json() -> String {
    serde_json::to_string_pretty(&mcp_tool_descriptors())
        .expect("McpToolDescriptor serialization is infallible")
        + "\n"
}

/// Builds the tool objects returned by MCP `tools/list`.
///
/// Derives from `mcp_tool_descriptors()` — each descriptor is serialized to
/// `serde_json::Value` to produce the exact same wire shape as before.
pub(crate) fn mcp_tools() -> Vec<serde_json::Value> {
    mcp_tool_descriptors()
        .into_iter()
        .map(|d| serde_json::to_value(d).expect("McpToolDescriptor serialization is infallible"))
        .collect()
}

/// Schema stub for MCP tools that accept no arguments.
///
/// The `required` key is intentionally absent, not an empty array.
fn empty_mcp_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {}
    })
}

fn optional_job_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "job_id": {"type": "string", "description": "Optional job id"}
        }
    })
}

fn session_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "session_id": {"type": "string", "description": "Discussion session id"}
        },
        "required": ["session_id"]
    })
}

fn common_run_schema(default_quiet: bool) -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "prompt": {"type": "string", "description": "Task or question for the backend agent"},
            "agent": {
                "type": "string",
                "enum": ["auto", "gemini", "claude", "codex"],
                "description": "Backend agent. Also accepts AGENT:MODEL strings at runtime."
            },
            "model": {"type": "string", "description": "Optional backend model hint; currently used by Claude"},
            "cwd": {"type": "string", "description": "Working directory context"},
            "timeout_secs": {"type": "integer", "minimum": 1},
            "quiet": {
                "type": "boolean",
                "default": default_quiet,
                "description": "Consult mode when true; agent/autonomous mode when false."
            },
            "allow_bypass_permissions": {
                "type": "boolean",
                "default": false,
                "description": "Explicitly allow Claude Code permission bypass in non-quiet mode."
            }
        },
        "required": ["prompt"]
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use crate::manifest::mcp_tool_names;
    use crate::mcp_dispatch::handle_mcp_request;
    use crate::mcp_schema::mcp_tools;

    fn object_keys(value: &serde_json::Value) -> BTreeSet<String> {
        value
            .as_object()
            .unwrap()
            .keys()
            .map(|key| key.to_string())
            .collect()
    }

    fn string_array(value: &serde_json::Value) -> Vec<&str> {
        value
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item.as_str().unwrap())
            .collect()
    }

    #[test]
    fn mcp_lists_agent_swarm_tools() {
        let response = handle_mcp_request(serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/list",
            "params": {}
        }))
        .unwrap();
        let tools = response["result"]["tools"].as_array().unwrap();
        let names = tools
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect::<BTreeSet<_>>();
        let expected = mcp_tool_names().into_iter().collect::<BTreeSet<_>>();
        assert_eq!(names, expected);

        for tool in tools {
            assert!(!tool["name"].as_str().unwrap().is_empty());
            assert!(!tool["description"].as_str().unwrap().is_empty());
            assert_eq!(tool["inputSchema"]["type"], "object");
            let properties = tool["inputSchema"]["properties"].as_object().unwrap();
            if let Some(required) = tool["inputSchema"].get("required") {
                assert!(required.is_array());
                for name in string_array(required) {
                    assert!(
                        properties.contains_key(name),
                        "required key {name} missing from properties for {}",
                        tool["name"]
                    );
                }
            }
            assert_eq!(
                object_keys(tool),
                BTreeSet::from([
                    "description".to_string(),
                    "inputSchema".to_string(),
                    "name".to_string(),
                ])
            );
        }

        let fanout = tools
            .iter()
            .find(|tool| tool["name"] == "agent_swarm_fanout")
            .unwrap();
        assert_eq!(
            object_keys(&fanout["inputSchema"]["properties"]),
            BTreeSet::from([
                "cwd".to_string(),
                "manager".to_string(),
                "prompt".to_string(),
                "timeout_secs".to_string(),
                "workers".to_string(),
            ])
        );
        assert_eq!(
            fanout["inputSchema"]["required"],
            serde_json::json!(["prompt"])
        );
    }

    /// W2 parity tripwire: the name-set of `mcp_tools()` (schema source-of-truth)
    /// must exactly equal the name-set of `mcp_tool_names()` (manifest advertisement).
    ///
    /// These two lists are hand-maintained in separate files. This test asserts
    /// their equality directly — without going through `handle_mcp_request` — so
    /// the guard survives any future dispatch refactoring.
    ///
    /// To add a new tool: update BOTH `mcp_tool_descriptors()` in mcp_schema.rs AND
    /// `mcp_tool_names()` in manifest.rs, then verify this test still passes.
    #[test]
    fn mcp_tool_names_and_schema_names_are_in_parity() {
        let schema_names: BTreeSet<String> = mcp_tools()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap().to_owned())
            .collect();
        let manifest_names: BTreeSet<String> =
            mcp_tool_names().iter().map(|s| s.to_string()).collect();

        let only_in_schema: Vec<&String> = schema_names.difference(&manifest_names).collect();
        let only_in_manifest: Vec<&String> = manifest_names.difference(&schema_names).collect();

        assert!(
            only_in_schema.is_empty() && only_in_manifest.is_empty(),
            "MCP tool-list parity failure:\n  in mcp_tools() only: {only_in_schema:?}\n  in mcp_tool_names() only: {only_in_manifest:?}\n\nUpdate BOTH lists to add/remove a tool."
        );
    }

    /// D1-01 idempotency gate: checked-in mcp-tools.json must match the descriptor table.
    ///
    /// Both are derived from the same `mcp_tools_pretty_json()` function, so byte-identity
    /// is guaranteed as long as the file is regenerated after each descriptor change.
    ///
    /// If this test fails: run `cargo run -p swarm-mcp --bin gen_mcp_tools` from
    /// the repo root to regenerate mcp-tools.json, then commit the updated artifact.
    #[test]
    fn mcp_tools_json_matches_descriptor_table() {
        // P5-S6 cleanup: the gen_mcp_tools binary and the frozen artifact now both live
        // in swarm-mcp (the crate that owns the schema). CARGO_MANIFEST_DIR is
        // swarm-mcp/, so the artifact sits right beside the manifest.
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("mcp-tools.json");
        let on_disk = std::fs::read_to_string(&path).unwrap_or_else(|e| {
            panic!(
                "mcp-tools.json not found at {path}: {e}\n\
                 Run `cargo run -p swarm-mcp --bin gen_mcp_tools` from the repo root to generate it.",
                path = path.display()
            )
        });
        let fresh = super::mcp_tools_pretty_json();
        assert_eq!(
            on_disk, fresh,
            "mcp-tools.json is stale — run `cargo run -p swarm-mcp --bin gen_mcp_tools` from the repo root"
        );
    }
}
