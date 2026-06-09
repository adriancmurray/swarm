//! Leaf helpers for MCP argument parsing and JSON-RPC response envelopes.

use std::env;
use std::path::Path;
use std::process::{Command, Stdio};

pub struct McpToolOutput {
    pub text: String,
    pub is_error: bool,
}

pub fn invoked_as_mcp_binary() -> bool {
    env::args()
        .next()
        .and_then(|arg| {
            Path::new(&arg)
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
        })
        .map(|name| name == "agent-swarm-mcp")
        .unwrap_or(false)
}

pub fn required_arg(arguments: &serde_json::Value, name: &str) -> Result<String, String> {
    optional_string_arg(arguments, name)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("missing required argument `{name}`"))
}

pub fn optional_string_arg(arguments: &serde_json::Value, name: &str) -> Option<String> {
    arguments
        .get(name)
        .and_then(|value| value.as_str())
        .map(ToString::to_string)
}

pub fn optional_bool_arg(arguments: &serde_json::Value, name: &str) -> Option<bool> {
    arguments.get(name).and_then(|value| value.as_bool())
}

pub fn optional_u64_arg(arguments: &serde_json::Value, name: &str) -> Option<u64> {
    arguments.get(name).and_then(|value| value.as_u64())
}

pub fn optional_string_array_arg(arguments: &serde_json::Value, name: &str) -> Vec<String> {
    arguments
        .get(name)
        .and_then(|value| value.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|value| value.as_str().map(ToString::to_string))
                .collect()
        })
        .unwrap_or_default()
}

pub fn mcp_result(id: serde_json::Value, result: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result
    })
}

pub fn mcp_error(id: serde_json::Value, code: i64, message: &str) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": code,
            "message": message
        }
    })
}

pub fn mcp_tool_text_result(
    id: serde_json::Value,
    text: String,
    is_error: bool,
) -> serde_json::Value {
    mcp_result(
        id,
        serde_json::json!({
            "content": [{"type": "text", "text": text}],
            "isError": is_error
        }),
    )
}

pub fn push_common_mcp_cli_args(
    args: &mut Vec<String>,
    arguments: &serde_json::Value,
    default_quiet: bool,
) {
    if optional_bool_arg(arguments, "quiet").unwrap_or(default_quiet) {
        args.push("--quiet".to_string());
    }
    if let Some(agent) = optional_string_arg(arguments, "agent") {
        args.push("--agent".to_string());
        args.push(agent);
    }
    if let Some(model) = optional_string_arg(arguments, "model") {
        args.push("--model".to_string());
        args.push(model);
    }
    if let Some(cwd) = optional_string_arg(arguments, "cwd") {
        args.push("--cwd".to_string());
        args.push(cwd);
    }
    if let Some(timeout) = optional_u64_arg(arguments, "timeout_secs") {
        args.push("--timeout".to_string());
        args.push(timeout.to_string());
    }
    if optional_bool_arg(arguments, "allow_bypass_permissions").unwrap_or(false) {
        args.push("--allow-bypass-permissions".to_string());
    }
}

pub fn run_self_for_mcp(args: Vec<String>) -> Result<McpToolOutput, String> {
    let current_exe =
        env::current_exe().map_err(|err| format!("Error locating current executable: {err}"))?;
    let output = Command::new(current_exe)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|err| format!("Error executing agent-swarm: {err}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let mut text = String::new();
    if !stdout.trim().is_empty() {
        text.push_str(stdout.trim_end());
    }
    if !stderr.trim().is_empty() {
        if !text.is_empty() {
            text.push_str("\n\n");
        }
        text.push_str("stderr:\n");
        text.push_str(stderr.trim_end());
    }
    if text.is_empty() {
        text = format!(
            "agent-swarm exited with code {}",
            output.status.code().unwrap_or(1)
        );
    }

    Ok(McpToolOutput {
        text,
        is_error: !output.status.success(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn required_arg_rejects_missing_and_blank_values() {
        let args = serde_json::json!({
            "blank": "  ",
            "present": "ok"
        });

        assert_eq!(required_arg(&args, "present").unwrap(), "ok");
        assert!(required_arg(&args, "missing")
            .unwrap_err()
            .contains("missing"));
        assert!(required_arg(&args, "blank").unwrap_err().contains("blank"));
    }

    #[test]
    fn optional_string_array_arg_filters_non_strings() {
        let args = serde_json::json!({
            "name": "agent",
            "number": 7,
            "items": ["a", 2, "b", false]
        });

        assert_eq!(optional_string_arg(&args, "name").as_deref(), Some("agent"));
        assert_eq!(optional_string_arg(&args, "number"), None);
        assert_eq!(optional_string_arg(&args, "missing"), None);
        assert_eq!(
            optional_string_array_arg(&args, "items"),
            vec!["a".to_string(), "b".to_string()]
        );
    }

    #[test]
    fn mcp_result_and_error_keep_json_rpc_envelopes_distinct() {
        let result = mcp_result(
            serde_json::json!("request-1"),
            serde_json::json!({"ok": true}),
        );
        assert_eq!(result["jsonrpc"], "2.0");
        assert_eq!(result["id"], "request-1");
        assert_eq!(result["result"]["ok"], true);
        assert!(result.get("error").is_none());

        let error = mcp_error(serde_json::json!("request-2"), -32601, "missing method");
        assert_eq!(error["jsonrpc"], "2.0");
        assert_eq!(error["id"], "request-2");
        assert_eq!(error["error"]["code"], -32601);
        assert_eq!(error["error"]["message"], "missing method");
        assert!(error.get("result").is_none());
    }

    #[test]
    fn mcp_tool_text_result_wraps_content_and_error_flag() {
        let result = mcp_tool_text_result(serde_json::json!(7), "hello".to_string(), true);

        assert_eq!(result["jsonrpc"], "2.0");
        assert_eq!(result["id"], 7);
        assert_eq!(result["result"]["isError"], true);
        assert_eq!(result["result"]["content"][0]["type"], "text");
        assert_eq!(result["result"]["content"][0]["text"], "hello");
    }

    #[test]
    fn push_common_mcp_cli_args_preserves_expected_option_order() {
        let args_json = serde_json::json!({
            "quiet": true,
            "agent": "claude",
            "model": "sonnet",
            "cwd": "/tmp/example",
            "timeout_secs": 12,
            "allow_bypass_permissions": true
        });
        let mut args = Vec::new();

        push_common_mcp_cli_args(&mut args, &args_json, false);

        assert_eq!(
            args,
            vec![
                "--quiet",
                "--agent",
                "claude",
                "--model",
                "sonnet",
                "--cwd",
                "/tmp/example",
                "--timeout",
                "12",
                "--allow-bypass-permissions"
            ]
        );
    }
}
