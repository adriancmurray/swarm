//! JSON-RPC/MCP dispatch for the Agent Swarm binary.
//!
//! This module owns the request loop, method routing, tool dispatch, and the
//! thin MCP-to-CLI adapters. Background worker mechanics stay in `main.rs`.

use std::env;
use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use swarm_core::JobRepo;
use swarm_kernel::args::{config_path, DEFAULT_TIMEOUT_SECS};
use swarm_kernel::conductor;
use swarm_kernel::config::{read_settings_at, write_settings_at, Settings};
use swarm_kernel::context::context_gather_json;
use swarm_kernel::format::{json_text, prompt_preview};
use swarm_kernel::ids::{JobId, PresetId};
use swarm_kernel::job_types::{JobAgent, JobMode, JobStatus};
use swarm_kernel::process::{detach_background_command, process_is_alive};
use swarm_kernel::resolver::home_dir;
use swarm_kernel::{profiles, telemetry};
use swarm_store::job::JobRecord;
use swarm_store::monitor_store::{
    alerts_json, monitor_alerts_path, monitor_status_path, read_monitor_pid,
};
use swarm_store::repos::job_repo::FileJobRepo;
use swarm_store::store::{job_store_dir, new_job_id, now_ms, write_text_atomic};

use crate::manifest::manifest_payload;
use crate::mcp_helpers::{
    mcp_error, mcp_result, mcp_tool_text_result, optional_bool_arg, optional_string_arg,
    optional_string_array_arg, optional_u64_arg, push_common_mcp_cli_args, required_arg,
    run_self_for_mcp, McpToolOutput,
};
use crate::mcp_schema::mcp_tools;
use crate::report::{
    runtime_processes_json, session_artifacts_json, session_list_json, session_summary_json,
};

pub fn cmd_mcp() -> Result<i32, String> {
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    for line in stdin.lock().lines() {
        let line = line.map_err(|err| format!("Error reading MCP stdin: {err}"))?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let response = match serde_json::from_str::<serde_json::Value>(line) {
            Ok(request) => handle_mcp_request(request),
            Err(err) => Some(mcp_error(
                serde_json::Value::Null,
                -32700,
                &format!("parse error: {err}"),
            )),
        };

        if let Some(response) = response {
            let encoded = serde_json::to_string(&response)
                .map_err(|err| format!("Error serializing MCP response: {err}"))?;
            writeln!(stdout, "{encoded}")
                .map_err(|err| format!("Error writing MCP response: {err}"))?;
            stdout
                .flush()
                .map_err(|err| format!("Error flushing MCP response: {err}"))?;
        }
    }

    Ok(0)
}

pub fn handle_mcp_request(request: serde_json::Value) -> Option<serde_json::Value> {
    let id = request.get("id").cloned();
    let method = request.get("method").and_then(|value| value.as_str());

    let id = id?;

    match method {
        Some("initialize") => Some(mcp_result(
            id,
            serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {"tools": {}},
                "serverInfo": {
                    "name": "agent-swarm",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }),
        )),
        Some("ping") => Some(mcp_result(id, serde_json::json!({}))),
        Some("tools/list") => Some(mcp_result(id, serde_json::json!({"tools": mcp_tools()}))),
        Some("tools/call") => {
            let params = request
                .get("params")
                .and_then(|value| value.as_object())
                .cloned()
                .unwrap_or_default();
            let name = params.get("name").and_then(|value| value.as_str());
            let arguments = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({}));
            match name {
                Some(name) => Some(route_mcp_tool_call(name, &arguments).map_or_else(
                    |err| mcp_tool_text_result(id.clone(), err, true),
                    |output| mcp_tool_text_result(id.clone(), output.text, output.is_error),
                )),
                None => Some(mcp_error(id, -32602, "tools/call requires params.name")),
            }
        }
        Some(other) => Some(mcp_error(id, -32601, &format!("unknown method: {other}"))),
        None => Some(mcp_error(id, -32600, "request missing method")),
    }
}

pub(crate) fn route_mcp_tool_call(
    name: &str,
    arguments: &serde_json::Value,
) -> Result<McpToolOutput, String> {
    match name {
        "agent_swarm_manifest" => Ok(McpToolOutput {
            text: json_text(manifest_payload()),
            is_error: false,
        }),
        "agent_swarm_insights" => Ok(McpToolOutput {
            text: json_text(telemetry::insights_json()),
            is_error: false,
        }),
        "agent_swarm_profiles" => Ok(McpToolOutput {
            text: json_text(profiles::profiles_json()),
            is_error: false,
        }),
        "agent_swarm_automation_hooks" => Ok(McpToolOutput {
            text: json_text(profiles::automation_hooks_json()),
            is_error: false,
        }),
        "agent_swarm_presets" => Ok(McpToolOutput {
            text: json_text(telemetry::presets_json()),
            is_error: false,
        }),
        "agent_swarm_preset" => mcp_cli_preset(arguments),
        "agent_swarm_recommend" => match required_arg(arguments, "prompt") {
            Ok(prompt) => Ok(McpToolOutput {
                text: json_text(telemetry::recommendation_json(&prompt)),
                is_error: false,
            }),
            Err(err) => Err(err),
        },
        "agent_swarm_feedback" => {
            let role = required_arg(arguments, "role");
            let agent = required_arg(arguments, "agent");
            let outcome = required_arg(arguments, "outcome");
            match (role, agent, outcome) {
                (Ok(role), Ok(agent), Ok(outcome)) => {
                    let payload = telemetry::feedback_json(
                        optional_string_arg(arguments, "session_id"),
                        role,
                        agent,
                        outcome,
                        optional_string_arg(arguments, "note"),
                    );
                    match payload {
                        Ok(value) => Ok(McpToolOutput {
                            text: json_text(value),
                            is_error: false,
                        }),
                        Err(err) => Err(err),
                    }
                }
                (Err(err), _, _) | (_, Err(err), _) | (_, _, Err(err)) => Err(err),
            }
        }
        "agent_swarm_proposals" => Ok(McpToolOutput {
            text: json_text(telemetry::proposals_json()),
            is_error: false,
        }),
        "agent_swarm_proposal_record" => {
            let title = required_arg(arguments, "title");
            let body = required_arg(arguments, "body");
            match (title, body) {
                (Ok(title), Ok(body)) => match telemetry::proposal_json(
                    optional_string_arg(arguments, "session_id"),
                    title,
                    body,
                    optional_string_arg(arguments, "proposed_by"),
                    optional_string_array_arg(arguments, "tags"),
                ) {
                    Ok(value) => Ok(McpToolOutput {
                        text: json_text(value),
                        is_error: false,
                    }),
                    Err(err) => Err(err),
                },
                (Err(err), _) | (_, Err(err)) => Err(err),
            }
        }
        "agent_swarm_proposal_vote" => {
            let proposal_id = required_arg(arguments, "proposal_id");
            let voter = required_arg(arguments, "voter");
            let vote = required_arg(arguments, "vote");
            match (proposal_id, voter, vote) {
                (Ok(proposal_id), Ok(voter), Ok(vote)) => match telemetry::proposal_vote_json(
                    proposal_id.into(),
                    voter,
                    vote,
                    optional_string_arg(arguments, "rationale"),
                ) {
                    Ok(value) => Ok(McpToolOutput {
                        text: json_text(value),
                        is_error: false,
                    }),
                    Err(err) => Err(err),
                },
                (Err(err), _, _) | (_, Err(err), _) | (_, _, Err(err)) => Err(err),
            }
        }
        "agent_swarm_activity_record" => mcp_activity_record(arguments),
        "agent_swarm_run" => mcp_cli_run(arguments),
        "agent_swarm_swarm" => mcp_cli_swarm(arguments),
        "agent_swarm_fanout" => mcp_cli_swarm(arguments),
        "agent_swarm_discuss" => mcp_cli_discuss(arguments),
        "agent_swarm_audit" => mcp_cli_audit(arguments),
        "agent_swarm_discuss_start" => mcp_cli_discuss_start(arguments),
        "agent_swarm_audit_start" => mcp_cli_audit_start(arguments),
        "agent_swarm_design" => mcp_cli_design(arguments),
        "agent_swarm_job_start" => mcp_cli_job_start(arguments),
        "agent_swarm_job_status" => mcp_cli_job_status(arguments),
        "agent_swarm_job_result" => mcp_cli_job_result(arguments),
        "agent_swarm_job_cancel" => mcp_cli_job_cancel(arguments),
        "agent_swarm_session_list" => mcp_cli_sessions(),
        "agent_swarm_session_events" => mcp_cli_session_events(arguments),
        "agent_swarm_session_transcript" => mcp_cli_session_transcript(arguments),
        "agent_swarm_session_summary" => mcp_cli_session_summary(arguments),
        "agent_swarm_session_artifacts" => mcp_cli_session_artifacts(arguments),
        "agent_swarm_runtime_processes" => mcp_runtime_processes(),
        "agent_swarm_monitor_status" => mcp_monitor_status(),
        "agent_swarm_monitor_start" => mcp_monitor_start(arguments),
        "agent_swarm_alerts" => mcp_alerts(arguments),
        "agent_swarm_context_gather" => mcp_context_gather(arguments),
        "agent_swarm_overview" => mcp_overview(),
        "agent_swarm_settings_get" => mcp_settings_get(),
        "agent_swarm_settings_set" => mcp_settings_set(arguments),
        other => Err(format!("unknown tool: {other}")),
    }
}

fn mcp_activity_record(arguments: &serde_json::Value) -> Result<McpToolOutput, String> {
    let result = conductor::record_activity(arguments)?;
    Ok(McpToolOutput {
        text: json_text(serde_json::json!({
            "schema": "agent-swarm/activity-record-result/v1",
            "session_id": result.session_id,
            "node_id": result.node_id,
            "path": result.path.display().to_string(),
        })),
        is_error: false,
    })
}

fn mcp_cli_run(arguments: &serde_json::Value) -> Result<McpToolOutput, String> {
    let prompt = required_arg(arguments, "prompt")?;
    let mut args = vec!["run".to_string()];
    push_common_mcp_cli_args(&mut args, arguments, true);
    args.push(prompt);
    run_self_for_mcp(args)
}

fn mcp_cli_preset(arguments: &serde_json::Value) -> Result<McpToolOutput, String> {
    let preset_id = PresetId::from(required_arg(arguments, "preset_id")?);
    let prompt = required_arg(arguments, "prompt")?;
    let mut args = vec!["preset".to_string(), preset_id.to_string()];
    push_common_mcp_cli_args(&mut args, arguments, false);
    if optional_bool_arg(arguments, "helpers").unwrap_or(false)
        || optional_bool_arg(arguments, "profile_helpers").unwrap_or(false)
    {
        args.push("--helpers".to_string());
    }
    args.push(prompt);
    run_self_for_mcp(args)
}

fn mcp_cli_swarm(arguments: &serde_json::Value) -> Result<McpToolOutput, String> {
    let prompt = required_arg(arguments, "prompt")?;
    let mut args = vec!["swarm".to_string()];
    if let Some(manager) = optional_string_arg(arguments, "manager") {
        args.push("--manager".to_string());
        args.push(manager);
    }
    if let Some(workers) = arguments.get("workers").and_then(|value| value.as_array()) {
        for worker in workers.iter().filter_map(|value| value.as_str()) {
            args.push("--worker".to_string());
            args.push(worker.to_string());
        }
    }
    push_common_mcp_cli_args(&mut args, arguments, false);
    args.push(prompt);
    run_self_for_mcp(args)
}

fn mcp_cli_discuss(arguments: &serde_json::Value) -> Result<McpToolOutput, String> {
    let prompt = required_arg(arguments, "prompt")?;
    let mut args = build_discussion_cli_args("discuss", arguments)?;
    args.push(prompt);
    run_self_for_mcp(args)
}

fn build_discussion_cli_args(
    command: &str,
    arguments: &serde_json::Value,
) -> Result<Vec<String>, String> {
    let mut args = vec![command.to_string()];
    if let Some(focus) = optional_string_arg(arguments, "focus") {
        args.push("--focus".to_string());
        args.push(focus);
    }
    if let Some(manager) = optional_string_arg(arguments, "manager") {
        args.push("--manager".to_string());
        args.push(manager);
    }
    if let Some(participants) = arguments
        .get("participants")
        .or_else(|| arguments.get("workers"))
        .and_then(|value| value.as_array())
    {
        for participant in participants.iter().filter_map(|value| value.as_str()) {
            args.push("--participant".to_string());
            args.push(participant.to_string());
        }
    }
    if let Some(rounds) = optional_u64_arg(arguments, "rounds") {
        args.push("--rounds".to_string());
        args.push(rounds.to_string());
    }
    if optional_bool_arg(arguments, "docs").unwrap_or(false)
        || optional_bool_arg(arguments, "api_docs").unwrap_or(false)
    {
        args.push("--docs".to_string());
    }
    if optional_bool_arg(arguments, "helpers").unwrap_or(false)
        || optional_bool_arg(arguments, "profile_helpers").unwrap_or(false)
    {
        args.push("--helpers".to_string());
    }
    if let Some(docs_agent) = optional_string_arg(arguments, "docs_agent") {
        args.push("--docs-agent".to_string());
        args.push(docs_agent);
    }
    push_common_mcp_cli_args(&mut args, arguments, false);
    Ok(args)
}

fn mcp_cli_audit(arguments: &serde_json::Value) -> Result<McpToolOutput, String> {
    let prompt = required_arg(arguments, "prompt")?;
    let mut args = build_discussion_cli_args("audit", arguments)?;
    if optional_bool_arg(arguments, "docs").unwrap_or(true)
        && !optional_bool_arg(arguments, "no_docs").unwrap_or(false)
        && !args.iter().any(|arg| arg == "--docs")
    {
        args.push("--docs".to_string());
    }
    args.push(prompt);
    run_self_for_mcp(args)
}

fn mcp_cli_discuss_start(arguments: &serde_json::Value) -> Result<McpToolOutput, String> {
    let prompt = required_arg(arguments, "prompt")?;
    let mut args = build_discussion_cli_args("discuss", arguments)?;
    args.push(prompt.clone());
    let cwd = optional_string_arg(arguments, "cwd")
        .map(PathBuf::from)
        .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let timeout = optional_u64_arg(arguments, "timeout_secs").unwrap_or(DEFAULT_TIMEOUT_SECS);
    Ok(McpToolOutput {
        text: json_text(start_background_command_job(
            "discussion",
            &cwd,
            &prompt,
            timeout,
            args,
        )?),
        is_error: false,
    })
}

fn mcp_cli_audit_start(arguments: &serde_json::Value) -> Result<McpToolOutput, String> {
    let prompt = required_arg(arguments, "prompt")?;
    let mut args = build_discussion_cli_args("audit", arguments)?;
    if optional_bool_arg(arguments, "docs").unwrap_or(true)
        && !optional_bool_arg(arguments, "no_docs").unwrap_or(false)
        && !args.iter().any(|arg| arg == "--docs")
    {
        args.push("--docs".to_string());
    }
    args.push(prompt.clone());
    let cwd = optional_string_arg(arguments, "cwd")
        .map(PathBuf::from)
        .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let timeout = optional_u64_arg(arguments, "timeout_secs").unwrap_or(DEFAULT_TIMEOUT_SECS);
    Ok(McpToolOutput {
        text: json_text(start_background_command_job(
            "audit", &cwd, &prompt, timeout, args,
        )?),
        is_error: false,
    })
}

fn start_background_command_job(
    mode: &str,
    cwd: &Path,
    prompt_preview_text: &str,
    timeout_secs: u64,
    command_args: Vec<String>,
) -> Result<serde_json::Value, String> {
    if command_args.is_empty() {
        return Err("Error: background command requires arguments".to_string());
    }
    let job_dir = job_store_dir()?;
    fs::create_dir_all(&job_dir)
        .map_err(|err| format!("Error creating job directory {}: {err}", job_dir.display()))?;

    let id = new_job_id();
    let prompt_path = job_dir.join(format!("{id}.prompt.md"));
    let stdout_path = job_dir.join(format!("{id}.stdout.log"));
    let stderr_path = job_dir.join(format!("{id}.stderr.log"));
    let result_path = job_dir.join(format!("{id}.result.txt"));
    write_text_atomic(&prompt_path, command_args.join(" "))?;

    let record = JobRecord {
        id: id.clone(),
        status: JobStatus::Queued,
        agent: JobAgent::Swarm,
        model: None,
        mode: JobMode::from_wire_str(mode),
        cwd: cwd.display().to_string(),
        prompt_preview: prompt_preview(prompt_preview_text),
        timeout_secs,
        created_at_ms: now_ms(),
        started_at_ms: None,
        completed_at_ms: None,
        pid: None,
        exit_code: None,
        prompt_path: prompt_path.display().to_string(),
        stdout_path: stdout_path.display().to_string(),
        stderr_path: stderr_path.display().to_string(),
        result_path: result_path.display().to_string(),
        allow_recursive_codex: false,
    };
    let job_repo = FileJobRepo::new(&job_dir);
    job_repo
        .save(&record)
        .map_err(|e| format!("Error writing job record: {e}"))?;

    let current_exe =
        env::current_exe().map_err(|err| format!("Error locating current executable: {err}"))?;
    let worker_stderr = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&stderr_path)
        .map_err(|err| {
            format!(
                "Error opening command worker stderr {}: {err}",
                stderr_path.display()
            )
        })?;
    let mut command = Command::new(current_exe);
    command
        .arg("__command-worker")
        .arg(id.as_str())
        .args(&command_args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(worker_stderr));
    detach_background_command(&mut command);
    let child = command
        .spawn()
        .map_err(|err| format!("Error starting background command job: {err}"))?;

    if let Ok(mut latest) = job_repo.get(&JobId::from(id.as_str())) {
        latest.status = JobStatus::Running;
        latest.started_at_ms = Some(now_ms());
        latest.pid = Some(child.id());
        job_repo
            .save(&latest)
            .map_err(|e| format!("Error updating job record: {e}"))?;
    }

    Ok(serde_json::json!({
        "schema": "agent-swarm/background-command/v1",
        "job_id": id,
        "status": "running",
        "mode": mode,
        "command": command_args
    }))
}

fn mcp_cli_design(arguments: &serde_json::Value) -> Result<McpToolOutput, String> {
    let prompt = required_arg(arguments, "prompt")?;
    let mut args = vec!["design".to_string()];
    if let Some(focus) = optional_string_arg(arguments, "focus") {
        args.push("--focus".to_string());
        args.push(focus);
    }
    if let Some(manager) = optional_string_arg(arguments, "manager") {
        args.push("--manager".to_string());
        args.push(manager);
    }
    if let Some(participants) = arguments
        .get("participants")
        .or_else(|| arguments.get("workers"))
        .and_then(|value| value.as_array())
    {
        for participant in participants.iter().filter_map(|value| value.as_str()) {
            args.push("--participant".to_string());
            args.push(participant.to_string());
        }
    }
    if let Some(rounds) = optional_u64_arg(arguments, "rounds") {
        args.push("--rounds".to_string());
        args.push(rounds.to_string());
    }
    if optional_bool_arg(arguments, "docs").unwrap_or(false)
        || optional_bool_arg(arguments, "api_docs").unwrap_or(false)
    {
        args.push("--docs".to_string());
    }
    if optional_bool_arg(arguments, "helpers").unwrap_or(false)
        || optional_bool_arg(arguments, "profile_helpers").unwrap_or(false)
    {
        args.push("--helpers".to_string());
    }
    if let Some(docs_agent) = optional_string_arg(arguments, "docs_agent") {
        args.push("--docs-agent".to_string());
        args.push(docs_agent);
    }
    push_common_mcp_cli_args(&mut args, arguments, false);
    args.push(prompt);
    run_self_for_mcp(args)
}

fn mcp_cli_job_start(arguments: &serde_json::Value) -> Result<McpToolOutput, String> {
    let prompt = required_arg(arguments, "prompt")?;
    let mut args = vec!["run".to_string(), "--background".to_string()];
    push_common_mcp_cli_args(&mut args, arguments, true);
    args.push(prompt);
    run_self_for_mcp(args)
}

fn mcp_cli_job_status(arguments: &serde_json::Value) -> Result<McpToolOutput, String> {
    let mut args = vec!["status".to_string()];
    if let Some(job_id) = optional_string_arg(arguments, "job_id") {
        args.push(job_id);
    }
    run_self_for_mcp(args)
}

fn mcp_cli_job_result(arguments: &serde_json::Value) -> Result<McpToolOutput, String> {
    let mut args = vec!["result".to_string()];
    if let Some(job_id) = optional_string_arg(arguments, "job_id") {
        args.push(job_id);
    }
    run_self_for_mcp(args)
}

fn mcp_cli_job_cancel(arguments: &serde_json::Value) -> Result<McpToolOutput, String> {
    let job_id = required_arg(arguments, "job_id")?;
    run_self_for_mcp(vec!["cancel".to_string(), job_id])
}

fn mcp_cli_sessions() -> Result<McpToolOutput, String> {
    Ok(McpToolOutput {
        text: json_text(session_list_json()?),
        is_error: false,
    })
}

fn mcp_cli_session_events(arguments: &serde_json::Value) -> Result<McpToolOutput, String> {
    let session_id = required_arg(arguments, "session_id")?;
    run_self_for_mcp(vec!["events".to_string(), session_id])
}

fn mcp_cli_session_transcript(arguments: &serde_json::Value) -> Result<McpToolOutput, String> {
    let session_id = required_arg(arguments, "session_id")?;
    run_self_for_mcp(vec!["transcript".to_string(), session_id])
}

fn mcp_cli_session_summary(arguments: &serde_json::Value) -> Result<McpToolOutput, String> {
    let session_id = required_arg(arguments, "session_id")?;
    Ok(McpToolOutput {
        text: json_text(session_summary_json(&session_id)?),
        is_error: false,
    })
}

fn mcp_cli_session_artifacts(arguments: &serde_json::Value) -> Result<McpToolOutput, String> {
    let session_id = required_arg(arguments, "session_id")?;
    Ok(McpToolOutput {
        text: json_text(session_artifacts_json(&session_id)?),
        is_error: false,
    })
}

fn mcp_runtime_processes() -> Result<McpToolOutput, String> {
    Ok(McpToolOutput {
        text: json_text(runtime_processes_json()?),
        is_error: false,
    })
}

fn mcp_monitor_status() -> Result<McpToolOutput, String> {
    Ok(McpToolOutput {
        text: json_text(serde_json::json!({
            "schema": "agent-swarm/monitor-status/v1",
            "running": read_monitor_pid()?.map(process_is_alive).unwrap_or(false),
            "pid": read_monitor_pid()?,
            "alerts_path": monitor_alerts_path()?.display().to_string(),
            "status_path": monitor_status_path()?.display().to_string()
        })),
        is_error: false,
    })
}

fn mcp_monitor_start(arguments: &serde_json::Value) -> Result<McpToolOutput, String> {
    let mut args = vec!["monitor-start".to_string()];
    if optional_bool_arg(arguments, "replace").unwrap_or(false) {
        args.push("--replace".to_string());
    }
    if let Some(interval) = optional_u64_arg(arguments, "interval_secs") {
        args.push("--interval".to_string());
        args.push(interval.to_string());
    }
    if let Some(rss_mb) = optional_u64_arg(arguments, "rss_mb") {
        args.push("--rss-mb".to_string());
        args.push(rss_mb.to_string());
    }
    if let Some(stale_secs) = optional_u64_arg(arguments, "stale_secs") {
        args.push("--stale-secs".to_string());
        args.push(stale_secs.to_string());
    }
    if let Some(spike_factor) = arguments
        .get("spike_factor")
        .and_then(|value| value.as_f64())
    {
        args.push("--spike-factor".to_string());
        args.push(spike_factor.to_string());
    }
    run_self_for_mcp(args)
}

fn mcp_alerts(arguments: &serde_json::Value) -> Result<McpToolOutput, String> {
    let since = optional_u64_arg(arguments, "since_ts_ms").map(u128::from);
    let limit = optional_u64_arg(arguments, "limit").unwrap_or(50) as usize;
    Ok(McpToolOutput {
        text: json_text(alerts_json(since, limit.clamp(1, 500))?),
        is_error: false,
    })
}

fn mcp_context_gather(arguments: &serde_json::Value) -> Result<McpToolOutput, String> {
    let query = required_arg(arguments, "query")?;
    let cwd = optional_string_arg(arguments, "cwd")
        .map(PathBuf::from)
        .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let budget_tokens = optional_u64_arg(arguments, "budget_tokens").unwrap_or(1200);
    Ok(McpToolOutput {
        text: json_text(context_gather_json(&cwd, &query, budget_tokens)?),
        is_error: false,
    })
}

fn mcp_overview() -> Result<McpToolOutput, String> {
    Ok(McpToolOutput {
        text: json_text(crate::overview::overview_json()?),
        is_error: false,
    })
}

/// `agent_swarm_settings_get` — return the current `[settings]` section as JSON.
fn mcp_settings_get() -> Result<McpToolOutput, String> {
    let settings = match home_dir() {
        Some(home) => {
            let path = config_path(&home);
            read_settings_at(&path)
        }
        None => Settings::default(),
    };
    let settings_json = match serde_json::to_value(&settings) {
        Ok(value) => value,
        Err(err) => return Err(format!("Error serializing settings: {err}")),
    };
    Ok(McpToolOutput {
        text: json_text(settings_json),
        is_error: false,
    })
}

/// `agent_swarm_settings_set` — merge `[settings]` into config.toml and return
/// the updated settings as confirmation.
fn mcp_settings_set(arguments: &serde_json::Value) -> Result<McpToolOutput, String> {
    // Load current settings so unspecified fields keep their current value.
    let mut settings = match home_dir() {
        Some(home) => {
            let path = config_path(&home);
            read_settings_at(&path)
        }
        None => Settings::default(),
    };

    // Apply any supplied fields.
    if let Some(v) = arguments.get("docs_default").and_then(|v| v.as_bool()) {
        settings.docs_default = v;
    }

    // Write back to config.
    let write_result = match home_dir() {
        Some(home) => {
            let path = config_path(&home);
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            write_settings_at(&path, &settings)
        }
        None => Err("Error: could not determine home directory".to_string()),
    };

    match write_result {
        Ok(()) => {
            let settings_json = match serde_json::to_value(&settings) {
                Ok(value) => value,
                Err(err) => return Err(format!("Error serializing updated settings: {err}")),
            };
            Ok(McpToolOutput {
                text: json_text(serde_json::json!({
                    "schema": "agent-swarm/settings/v1",
                    "status": "updated",
                    "settings": settings_json,
                    "note": "Changes take effect on the next invocation; config is loaded once per process."
                })),
                is_error: false,
            })
        }
        Err(err) => Err(err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notifications_without_id_do_not_emit_responses() {
        let response = handle_mcp_request(serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        }));

        assert!(response.is_none());
    }

    #[test]
    fn initialize_returns_server_metadata() {
        let response = handle_mcp_request(serde_json::json!({
            "jsonrpc": "2.0",
            "id": "init-1",
            "method": "initialize"
        }))
        .unwrap();

        assert_eq!(response["jsonrpc"], "2.0");
        assert_eq!(response["id"], "init-1");
        assert_eq!(response["result"]["protocolVersion"], "2024-11-05");
        assert_eq!(response["result"]["serverInfo"]["name"], "agent-swarm");
    }

    #[test]
    fn tool_call_requires_name() {
        let response = handle_mcp_request(serde_json::json!({
            "jsonrpc": "2.0",
            "id": 9,
            "method": "tools/call",
            "params": {
                "arguments": {}
            }
        }))
        .unwrap();

        assert_eq!(response["error"]["code"], -32602);
        assert!(response["error"]["message"]
            .as_str()
            .unwrap()
            .contains("params.name"));
    }

    #[test]
    fn unknown_tool_returns_tool_error_payload() {
        let response = handle_mcp_request(serde_json::json!({
            "jsonrpc": "2.0",
            "id": 10,
            "method": "tools/call",
            "params": {
                "name": "agent_swarm_missing_tool",
                "arguments": {}
            }
        }))
        .unwrap();

        assert_eq!(response["result"]["isError"], true);
        assert!(response["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("unknown tool"));
    }

    #[test]
    fn test_build_discussion_cli_args_reuses_push_common() {
        let args_json = serde_json::json!({
            "focus": "simplify",
            "manager": "claude",
            "participants": ["architecture=gemini", "review=claude"],
            "rounds": 3,
            "docs": true,
            "helpers": true,
            "docs_agent": "gemini",
            "cwd": "/tmp/test",
            "timeout_secs": 15
        });

        let args = build_discussion_cli_args("discuss", &args_json).unwrap();

        assert_eq!(
            args,
            vec![
                "discuss",
                "--focus",
                "simplify",
                "--manager",
                "claude",
                "--participant",
                "architecture=gemini",
                "--participant",
                "review=claude",
                "--rounds",
                "3",
                "--docs",
                "--helpers",
                "--docs-agent",
                "gemini",
                "--cwd",
                "/tmp/test",
                "--timeout",
                "15"
            ]
        );
    }
}
