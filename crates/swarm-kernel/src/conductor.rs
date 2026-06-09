//! Harness-neutral conductor activity records.
//!
//! Consumers read `<swarm home>/conductor-sessions/*/records.jsonl` (e.g.
//! `~/.swarm/conductor-sessions/...`; see `swarm_store::store::swarm_home`).
//! Claude Code hooks are only one producer of that stream; this module provides
//! a neutral writer so Codex, Gemini, app runtimes, and future agents can emit
//! the same live topology records without copying Claude hook payload shapes.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Map, Value};

use swarm_store::store::{swarm_home, swarm_home_err};

pub const CONDUCTOR_RECORD_SCHEMA: &str = "agent-conductor/record/v1";

const LABEL_MAX: usize = 100;
const PROMPT_PREVIEW_MAX: usize = 200;
const DEBUG_LOG_MAX_BYTES: u64 = 256 * 1024;
const POLICY_MAX_BYTES: u64 = 64 * 1024;
const SESSION_DEPTH_MAX_BYTES: u64 = 512 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivityRecordResult {
    pub session_id: String,
    pub node_id: String,
    pub path: PathBuf,
}

pub fn record_activity(arguments: &Value) -> Result<ActivityRecordResult, String> {
    let session_id = required_string(arguments, "session_id")?;
    let node_id = optional_string(arguments, "node_id").unwrap_or_else(|| session_id.clone());
    let parent_id = optional_string(arguments, "parent_id");
    let depth =
        optional_i64(arguments, "depth").unwrap_or_else(|| if parent_id.is_some() { 1 } else { 0 });
    let status = optional_string(arguments, "status").unwrap_or_else(|| "running".to_string());
    let label = optional_string(arguments, "label")
        .or_else(|| optional_string(arguments, "prompt_preview"))
        .unwrap_or_else(|| node_id.clone());

    let mut record = Map::new();
    record.insert("schema".into(), json!(CONDUCTOR_RECORD_SCHEMA));
    record.insert("node_id".into(), json!(node_id));
    record.insert(
        "parent_id".into(),
        parent_id.map(Value::String).unwrap_or(Value::Null),
    );
    record.insert("depth".into(), json!(depth));
    record.insert("label".into(), json!(preview(&label, LABEL_MAX)));
    record.insert("status".into(), json!(status));
    record.insert("ts_ms".into(), json!(now_ms()));

    for key in [
        "cwd",
        "agent_type",
        "prompt_preview",
        "council_session_ref",
        "tool_use_id",
        "agent_id",
        "transcript_path",
        "cullable_handle",
        "policy_state",
        "slice_id",
        "node_kind",
        "role",
        "purpose",
        "capacity",
        "persona_id",
        "persona_name",
        "model",
        "provider",
        "progress",
        "activity_source",
        "chat_ref",
    ] {
        if let Some(value) = optional_string(arguments, key) {
            let bounded = if key == "prompt_preview"
                || key == "purpose"
                || key == "capacity"
                || key == "progress"
            {
                preview(&value, PROMPT_PREVIEW_MAX)
            } else {
                value
            };
            record.insert(key.into(), json!(bounded));
        }
    }
    for key in [
        "total_tokens",
        "total_duration_ms",
        "progress_percent",
        "child_count",
        "member_count",
    ] {
        if let Some(value) = optional_i64(arguments, key) {
            record.insert(key.into(), json!(value));
        }
    }
    if let Some(paths) = optional_string_array(arguments, "claimed_paths") {
        record.insert("claimed_paths".into(), json!(paths));
    }

    append_record_value(&session_id, Value::Object(record))
}

pub fn handle_hook_stdin(payload_text: &str) -> Option<Value> {
    if payload_text.trim().is_empty() {
        debug_log("invoked event=<none> reason=empty-stdin");
        return None;
    }

    let Ok(payload) = serde_json::from_str::<Value>(payload_text) else {
        debug_log("invoked reason=parse-error");
        return None;
    };
    let Some(payload) = payload.as_object() else {
        debug_log("invoked reason=non-object-payload");
        return None;
    };

    let event = string_value(payload.get("hook_event_name"));
    let session_id = string_value(payload.get("session_id"));
    let tool_name = string_value(payload.get("tool_name"));
    debug_log(&format!(
        "invoked event={} session_id={} tool_name={}",
        if event.is_empty() { "?" } else { &event },
        if session_id.is_empty() {
            "MISSING".to_string()
        } else {
            format!("yes:{}", session_id.chars().take(8).collect::<String>())
        },
        if tool_name.is_empty() {
            "-"
        } else {
            &tool_name
        },
    ));

    if let Some(record) = record_for_hook_payload(payload) {
        match append_record_value(&session_id, record) {
            Ok(result) => debug_log(&format!("  -> wrote node_id={}", result.node_id)),
            Err(err) => debug_log(&format!("  -> write error: {err}")),
        }
    } else {
        debug_log(&format!(
            "  -> no record for event={event} tool={tool_name}"
        ));
    }

    match policy_decision(payload) {
        Some(decision) => {
            let label = decision["hookSpecificOutput"]["permissionDecision"]
                .as_str()
                .unwrap_or("?");
            debug_log(&format!("  -> policy decision={label}"));
            Some(decision)
        }
        None => None,
    }
}

fn record_for_hook_payload(payload: &Map<String, Value>) -> Option<Value> {
    let event = string_value(payload.get("hook_event_name"));
    let session_id = string_value(payload.get("session_id"));
    let cwd = optional_string_map(payload, "cwd");
    let tool_name = string_value(payload.get("tool_name")).to_ascii_lowercase();
    let empty_input = Map::new();
    let empty_response = Map::new();
    let tool_input = payload
        .get("tool_input")
        .and_then(Value::as_object)
        .unwrap_or(&empty_input);
    let tool_response = payload
        .get("tool_response")
        .and_then(Value::as_object)
        .unwrap_or(&empty_response);
    let ts_ms = now_ms();

    if session_id.is_empty() {
        return None;
    }

    match event.as_str() {
        "SessionStart" => Some(json!({
            "schema": CONDUCTOR_RECORD_SCHEMA,
            "node_id": session_id,
            "parent_id": Value::Null,
            "depth": 0,
            "label": preview(&string_value(tool_input.get("prompt")).if_empty(&session_id), LABEL_MAX),
            "status": "running",
            "ts_ms": ts_ms,
            "cwd": cwd,
        })),
        "PreToolUse" if tool_name == "agent" || tool_name == "task" => {
            let tool_use_id = string_value(payload.get("tool_use_id"));
            if tool_use_id.is_empty() {
                return None;
            }
            let prompt = optional_string_map(tool_input, "prompt");
            let description = optional_string_map(tool_input, "description");
            Some(json!({
                "schema": CONDUCTOR_RECORD_SCHEMA,
                "node_id": tool_use_id,
                "parent_id": session_id,
                "depth": 1,
                "label": preview(&description.clone().or(prompt.clone()).unwrap_or_else(|| "sub-conductor".into()), LABEL_MAX),
                "status": "running",
                "ts_ms": ts_ms,
                "cwd": cwd,
                "agent_type": optional_string_map(tool_input, "subagent_type"),
                "prompt_preview": prompt.map(|text| preview(&text, PROMPT_PREVIEW_MAX)),
                "tool_use_id": tool_use_id,
            }))
        }
        "PostToolUse" if tool_name == "agent" || tool_name == "task" => {
            let tool_use_id = string_value(payload.get("tool_use_id"));
            if tool_use_id.is_empty() {
                return None;
            }
            let agent_id = optional_string_map(tool_response, "agentId");
            let prompt = optional_string_map(tool_input, "prompt");
            let description = optional_string_map(tool_input, "description");
            Some(json!({
                "schema": CONDUCTOR_RECORD_SCHEMA,
                "node_id": agent_id.clone().unwrap_or_else(|| tool_use_id.clone()),
                "parent_id": session_id,
                "depth": 1,
                "label": preview(&description.clone().or(prompt.clone()).or(agent_id.clone()).unwrap_or_else(|| "sub-conductor".into()), LABEL_MAX),
                "status": optional_string_map(tool_response, "status").unwrap_or_else(|| "completed".into()),
                "ts_ms": ts_ms,
                "cwd": cwd,
                "agent_type": optional_string_map(tool_response, "agentType")
                    .or_else(|| optional_string_map(tool_input, "subagent_type")),
                "prompt_preview": description.map(|text| preview(&text, PROMPT_PREVIEW_MAX)),
                "tool_use_id": tool_use_id,
                "agent_id": agent_id,
                "total_tokens": optional_i64_map(tool_response, "totalTokens"),
                "total_duration_ms": optional_i64_map(tool_response, "totalDurationMs")
                    .or_else(|| optional_i64_map(payload, "duration_ms")),
                "cullable_handle": optional_string_map(tool_response, "cullableHandle"),
                "policy_state": optional_string_map(tool_response, "policyState"),
            }))
        }
        "SubagentStop" => {
            let agent_id = string_value(payload.get("agent_id"));
            if agent_id.is_empty() {
                return None;
            }
            Some(json!({
                "schema": CONDUCTOR_RECORD_SCHEMA,
                "node_id": agent_id,
                "parent_id": session_id,
                "depth": 1,
                "label": preview(&optional_string_map(tool_input, "description").unwrap_or_else(|| agent_id.clone()), LABEL_MAX),
                "status": "completed",
                "ts_ms": ts_ms,
                "cwd": cwd,
                "agent_type": optional_string_map(payload, "agent_type"),
                "prompt_preview": optional_string_map(payload, "last_assistant_message").map(|text| preview(&text, PROMPT_PREVIEW_MAX)),
                "agent_id": agent_id,
                "transcript_path": optional_string_map(payload, "agent_transcript_path"),
            }))
        }
        "UserPromptSubmit" => Some(director_record(&session_id, "running", ts_ms, cwd)),
        "Stop" => Some(director_record(&session_id, "idle", ts_ms, cwd)),
        "SessionEnd" => Some(director_record(&session_id, "completed", ts_ms, cwd)),
        _ => None,
    }
}

fn director_record(session_id: &str, status: &str, ts_ms: u64, cwd: Option<String>) -> Value {
    json!({
        "schema": CONDUCTOR_RECORD_SCHEMA,
        "node_id": session_id,
        "parent_id": Value::Null,
        "depth": 0,
        "label": session_id,
        "status": status,
        "ts_ms": ts_ms,
        "cwd": cwd,
    })
}

fn append_record_value(session_id: &str, record: Value) -> Result<ActivityRecordResult, String> {
    if session_id.is_empty() {
        return Err("session_id is required".into());
    }
    validate_segment(session_id)?;
    let node_id = record
        .get("node_id")
        .and_then(Value::as_str)
        .unwrap_or(session_id)
        .to_string();
    let path = conductor_records_path(session_id)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| format!("Error creating conductor dir {}: {err}", parent.display()))?;
    }
    let line = serde_json::to_string(&record)
        .map_err(|err| format!("Error serializing conductor record: {err}"))?;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|err| format!("Error opening conductor records {}: {err}", path.display()))?;
    writeln!(file, "{line}")
        .map_err(|err| format!("Error writing conductor records {}: {err}", path.display()))?;
    Ok(ActivityRecordResult {
        session_id: session_id.to_string(),
        node_id,
        path,
    })
}

fn policy_decision(payload: &Map<String, Value>) -> Option<Value> {
    let event = string_value(payload.get("hook_event_name"));
    let tool_name = string_value(payload.get("tool_name")).to_ascii_lowercase();
    if event != "PreToolUse" || (tool_name != "agent" && tool_name != "task") {
        return None;
    }

    let policy = load_conductor_policy();
    let native_max = policy.native_max_depth?;
    let session_id = string_value(payload.get("session_id"));
    let child_depth = session_depth(&session_id, &session_id);
    if child_depth <= native_max {
        return None;
    }

    let interactive = std::env::var("SWARM_DEPTH")
        .map(|value| value.is_empty() || value == "0")
        .unwrap_or(true);
    let decision = if interactive {
        breach_decision(&policy.interactive_breach)
    } else {
        breach_decision(&policy.noninteractive_breach)
    };
    Some(json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": decision,
            "permissionDecisionReason": format!(
                "swarm.spawn depth gate: would-be depth {child_depth} exceeds native_max_depth {native_max} (set via the agent governance panel)."
            ),
        }
    }))
}

#[derive(Debug, Clone)]
struct SpawnPolicy {
    native_max_depth: Option<i64>,
    interactive_breach: String,
    noninteractive_breach: String,
}

fn load_conductor_policy() -> SpawnPolicy {
    let default = SpawnPolicy {
        native_max_depth: None,
        interactive_breach: "ask".into(),
        noninteractive_breach: "deny".into(),
    };
    let Some(home) = swarm_home() else {
        return default;
    };
    let path = home.join("conductor-policy.json");
    let Ok(metadata) = fs::metadata(&path) else {
        return default;
    };
    if metadata.len() > POLICY_MAX_BYTES {
        return default;
    }
    let Ok(text) = fs::read_to_string(&path) else {
        return default;
    };
    let Ok(decoded) = serde_json::from_str::<Value>(&text) else {
        return default;
    };
    let Some(spawn) = decoded.get("spawn").and_then(Value::as_object) else {
        return default;
    };
    SpawnPolicy {
        native_max_depth: optional_i64_map(spawn, "native_max_depth"),
        interactive_breach: optional_string_map(spawn, "interactive_breach")
            .unwrap_or_else(|| "ask".into()),
        noninteractive_breach: optional_string_map(spawn, "noninteractive_breach")
            .unwrap_or_else(|| "deny".into()),
    }
}

fn session_depth(session_id: &str, parent_id: &str) -> i64 {
    if session_id.is_empty() || parent_id.is_empty() {
        return 1;
    }
    let Ok(path) = conductor_records_path(session_id) else {
        return 1;
    };
    let Ok(metadata) = fs::metadata(&path) else {
        return 1;
    };
    let Ok(mut text) = fs::read_to_string(&path) else {
        return 1;
    };
    if metadata.len() > SESSION_DEPTH_MAX_BYTES {
        let keep = SESSION_DEPTH_MAX_BYTES as usize;
        let start = text.len().saturating_sub(keep);
        text = text[start..].to_string();
        if let Some(pos) = text.find('\n') {
            text = text[pos + 1..].to_string();
        }
    }
    let mut parent_depth = 0;
    for line in text.lines() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(object) = value.as_object() else {
            continue;
        };
        if string_value(object.get("node_id")) == parent_id {
            parent_depth = optional_i64_map(object, "depth").unwrap_or(0);
        }
    }
    parent_depth + 1
}

fn conductor_records_path(session_id: &str) -> Result<PathBuf, String> {
    validate_segment(session_id)?;
    let home = swarm_home().ok_or_else(swarm_home_err)?;
    Ok(home
        .join("conductor-sessions")
        .join(session_id)
        .join("records.jsonl"))
}

fn debug_log(message: &str) {
    let Some(home) = swarm_home() else {
        return;
    };
    let path = home.join("conductor-sessions/_hook-debug.log");
    let Some(parent) = path.parent() else {
        return;
    };
    if fs::create_dir_all(parent).is_err() {
        return;
    }
    if fs::metadata(&path)
        .map(|metadata| metadata.len() > DEBUG_LOG_MAX_BYTES)
        .unwrap_or(false)
    {
        let _ = fs::rename(&path, path.with_file_name("_hook-debug.log.1"));
    }
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(file, "{} {message}", debug_stamp());
    }
}

fn debug_stamp() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| Duration::from_secs(0))
        .as_secs();
    secs.to_string()
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| Duration::from_secs(0))
        .as_millis() as u64
}

fn required_string(arguments: &Value, key: &str) -> Result<String, String> {
    optional_string(arguments, key).ok_or_else(|| format!("{key} is required"))
}

fn optional_string(arguments: &Value, key: &str) -> Option<String> {
    arguments
        .as_object()
        .and_then(|map| optional_string_map(map, key))
}

fn optional_string_map(map: &Map<String, Value>, key: &str) -> Option<String> {
    map.get(key).and_then(|value| match value {
        Value::String(text) if !text.trim().is_empty() => Some(text.trim().to_string()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(value) => Some(value.to_string()),
        _ => None,
    })
}

fn string_value(value: Option<&Value>) -> String {
    value
        .and_then(|value| match value {
            Value::String(text) => Some(text.to_string()),
            Value::Number(number) => Some(number.to_string()),
            Value::Bool(value) => Some(value.to_string()),
            _ => None,
        })
        .unwrap_or_default()
}

fn optional_i64(arguments: &Value, key: &str) -> Option<i64> {
    arguments
        .as_object()
        .and_then(|map| optional_i64_map(map, key))
}

fn optional_i64_map(map: &Map<String, Value>, key: &str) -> Option<i64> {
    map.get(key).and_then(|value| match value {
        Value::Number(number) => number
            .as_i64()
            .or_else(|| number.as_u64().map(|v| v as i64)),
        Value::String(text) => text.parse().ok(),
        _ => None,
    })
}

fn optional_string_array(arguments: &Value, key: &str) -> Option<Vec<String>> {
    let values = arguments.as_object()?.get(key)?.as_array()?;
    let bounded = values
        .iter()
        .filter_map(|value| match value {
            Value::String(text) if !text.trim().is_empty() => Some(preview(text, 240)),
            Value::Number(number) => Some(number.to_string()),
            Value::Bool(value) => Some(value.to_string()),
            _ => None,
        })
        .take(20)
        .collect::<Vec<_>>();
    if bounded.is_empty() {
        None
    } else {
        Some(bounded)
    }
}

fn preview(value: &str, limit: usize) -> String {
    let compact = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() <= limit {
        compact
    } else {
        format!(
            "{}...",
            compact
                .chars()
                .take(limit.saturating_sub(3))
                .collect::<String>()
        )
    }
}

fn breach_decision(value: &str) -> &'static str {
    match value.to_ascii_lowercase().as_str() {
        "ask" => "ask",
        "deny" => "deny",
        _ => "deny",
    }
}

fn validate_segment(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.contains('/')
        || value.contains('\\')
        || value.contains("..")
        || value.starts_with('.')
    {
        Err(format!("invalid conductor id `{value}`"))
    } else {
        Ok(())
    }
}

trait IfEmpty {
    fn if_empty(self, fallback: &str) -> String;
}

impl IfEmpty for String {
    fn if_empty(self, fallback: &str) -> String {
        if self.is_empty() {
            fallback.to_string()
        } else {
            self
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::sync::{Mutex, OnceLock};

    /// Points `SWARM_HOME` at a fresh tempdir for the duration of `f`, so
    /// tests never touch the real `~/.swarm`. Serialized via `ENV_LOCK`
    /// because env vars are process-global.
    fn with_swarm_home<T>(f: impl FnOnce(&Path) -> T) -> T {
        static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        let _guard = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let previous = std::env::var_os("SWARM_HOME");
        std::env::set_var("SWARM_HOME", temp.path());
        let result = f(temp.path());
        match previous {
            Some(value) => std::env::set_var("SWARM_HOME", value),
            None => std::env::remove_var("SWARM_HOME"),
        }
        result
    }

    #[test]
    fn neutral_activity_record_writes_conductor_stream() {
        with_swarm_home(|home| {
            let result = record_activity(&json!({
                "session_id": "codex-session-1",
                "node_id": "codex-worker-1",
                "parent_id": "codex-session-1",
                "depth": 1,
                "label": "Codex worker",
                "status": "running",
                "agent_type": "codex",
                "role": "classifier",
                "purpose": "categorize incoming task slices",
                "capacity": "fast local text classification",
                "persona_name": "Gemma classifier",
                "provider": "mlx",
                "model": "mlx-community/gemma-4-e2b-it-OptiQ-4bit",
                "progress": "reading dispatch metadata",
                "progress_percent": 35,
                "activity_source": "codex",
                "child_count": 2,
                "member_count": 1,
                "claimed_paths": ["packages/panels/kard_agent_swarm"],
                "prompt_preview": "inspect the generalized swarm activity bridge",
            }))
            .unwrap();

            assert_eq!(result.session_id, "codex-session-1");
            assert_eq!(result.node_id, "codex-worker-1");
            let text =
                fs::read_to_string(home.join("conductor-sessions/codex-session-1/records.jsonl"))
                    .unwrap();
            let line: Value = serde_json::from_str(text.trim()).unwrap();
            assert_eq!(line["schema"], CONDUCTOR_RECORD_SCHEMA);
            assert_eq!(line["node_id"], "codex-worker-1");
            assert_eq!(line["parent_id"], "codex-session-1");
            assert_eq!(line["agent_type"], "codex");
            assert_eq!(line["role"], "classifier");
            assert_eq!(line["provider"], "mlx");
            assert_eq!(line["model"], "mlx-community/gemma-4-e2b-it-OptiQ-4bit");
            assert_eq!(line["progress_percent"], 35);
            assert_eq!(line["claimed_paths"][0], "packages/panels/kard_agent_swarm");
        });
    }

    #[test]
    fn hook_payload_records_pre_tool_use_and_denies_when_depth_exceeds_policy() {
        with_swarm_home(|home| {
            fs::write(
                home.join("conductor-policy.json"),
                json!({
                    "schema": "swarm.conductor-policy.v1",
                    "spawn": {
                        "native_max_depth": 0,
                        "interactive_breach": "ask",
                        "noninteractive_breach": "deny"
                    }
                })
                .to_string(),
            )
            .unwrap();
            let payload = json!({
                "hook_event_name": "PreToolUse",
                "session_id": "session-1",
                "tool_name": "Agent",
                "tool_use_id": "tool-1",
                "cwd": "/tmp",
                "tool_input": {
                    "description": "spawn worker",
                    "prompt": "do work",
                    "subagent_type": "worker"
                }
            });
            std::env::set_var("SWARM_DEPTH", "2");

            let decision = handle_hook_stdin(&payload.to_string()).unwrap();

            assert_eq!(decision["hookSpecificOutput"]["permissionDecision"], "deny");
            let text = fs::read_to_string(home.join("conductor-sessions/session-1/records.jsonl"))
                .unwrap();
            let line: Value = serde_json::from_str(text.trim()).unwrap();
            assert_eq!(line["node_id"], "tool-1");
            assert_eq!(line["status"], "running");
            std::env::remove_var("SWARM_DEPTH");
        });
    }
}
