//! MCP tool-descriptor wire type shared across swarm MCP consumers.
//!
//! # Design
//!
//! `McpToolDescriptor` is the shared seam for Phase-5 agent-swarm crate split:
//! - **D1** replaces the 59 hand-written `json!` entries in `agent-swarm/mcp_schema.rs`
//!   with a declarative descriptor table referencing this type.
//! - **W4** builds binary-SDK consumers whose traits expose a
//!   `tools() -> Vec<McpToolDescriptor>` method.
//!
//! Both reference `swarm_contracts::McpToolDescriptor` to prevent divergence.
//!
//! # Wire shape
//!
//! The MCP `tools/list` response emits exactly three keys per entry (confirmed at
//! `tools/agent-swarm/rust/src/mcp_schema.rs` lines 9-13):
//!
//! ```json
//! {
//!   "name":        "...",
//!   "description": "...",
//!   "inputSchema": { "type": "object", "properties": { ... } }
//! }
//! ```
//!
//! `inputSchema` is camelCase. All three keys are always present; no top-level
//! field is ever omitted. Future optional MCP fields (`title`, `annotations`,
//! `outputSchema`) can be added as `Option<…>` +
//! `#[serde(skip_serializing_if = "Option::is_none")]` without breaking byte-identity
//! for current consumers.

use serde::{Deserialize, Serialize};

/// One MCP tool entry as returned by `tools/list`.
///
/// Serialises to / deserialises from the exact three-key wire shape used by
/// agent-swarm: `name`, `description`, and `inputSchema` (camelCase).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct McpToolDescriptor {
    /// Tool name (e.g. `"agent_swarm_manifest"`).
    pub name: String,
    /// Human-readable description surfaced to the LLM client.
    pub description: String,
    /// JSON Schema for the tool's input parameters.
    ///
    /// No-arg tools use `{"type":"object","properties":{}}` (no `required` key).
    #[serde(rename = "inputSchema")]
    pub input_schema: serde_json::Value,
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Wire-parity proof: McpToolDescriptor for `agent_swarm_manifest` (a no-arg
    /// tool) serialises to exactly the same JSON Value as the `json!` literal in
    /// `tools/agent-swarm/rust/src/mcp_schema.rs` lines 9-13.
    ///
    /// Confirmed wire shape: three keys, `inputSchema` camelCase, no `required` key
    /// for no-arg tools (omitted, not `[]`).
    #[test]
    fn wire_parity_agent_swarm_manifest() {
        let descriptor = McpToolDescriptor {
            name: "agent_swarm_manifest".to_string(),
            description: "Return Agent Swarm's package manifest and detected backend availability."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {}
            }),
        };

        let got = serde_json::to_value(&descriptor).expect("serialization must succeed");
        let expected = json!({
            "name": "agent_swarm_manifest",
            "description": "Return Agent Swarm's package manifest and detected backend availability.",
            "inputSchema": {
                "type": "object",
                "properties": {}
            }
        });

        assert_eq!(
            got, expected,
            "McpToolDescriptor must serialise wire-identically"
        );
    }

    /// Round-trip proof: serialize → deserialize → equal.
    #[test]
    fn round_trip_no_arg_tool() {
        let original = McpToolDescriptor {
            name: "agent_swarm_manifest".to_string(),
            description: "Return Agent Swarm's package manifest and detected backend availability."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {}
            }),
        };

        let json_str = serde_json::to_string(&original).expect("serialize must succeed");
        let roundtripped: McpToolDescriptor =
            serde_json::from_str(&json_str).expect("deserialize must succeed");

        assert_eq!(original, roundtripped, "round-trip must be identity");
    }

    /// Round-trip proof for a tool with a non-trivial inputSchema (required + properties).
    #[test]
    fn round_trip_tool_with_input_schema() {
        let original = McpToolDescriptor {
            name: "agent_swarm_recommend".to_string(),
            description: "Recommend manager and participant specs for a task using learned telemetry plus safe defaults.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "prompt": {"type": "string", "description": "Task or objective to route"}
                },
                "required": ["prompt"]
            }),
        };

        let json_str = serde_json::to_string(&original).expect("serialize must succeed");
        let roundtripped: McpToolDescriptor =
            serde_json::from_str(&json_str).expect("deserialize must succeed");

        assert_eq!(
            original, roundtripped,
            "round-trip with inputSchema must be identity"
        );
    }

    /// Confirms `inputSchema` camelCase survives the round-trip —
    /// not `input_schema` snake_case.
    #[test]
    fn input_schema_key_is_camel_case() {
        let descriptor = McpToolDescriptor {
            name: "test_tool".to_string(),
            description: "Test.".to_string(),
            input_schema: json!({"type": "object", "properties": {}}),
        };

        let serialized = serde_json::to_string(&descriptor).unwrap();
        assert!(
            serialized.contains("\"inputSchema\""),
            "wire key must be camelCase 'inputSchema', got: {serialized}"
        );
        assert!(
            !serialized.contains("\"input_schema\""),
            "wire key must not be snake_case 'input_schema', got: {serialized}"
        );
    }
}
