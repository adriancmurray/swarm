//! CLI command handlers for read-only status and session display.

use std::fs;
use std::thread;
use std::time::Duration;

use swarm_exec::session::list_sessions;
use swarm_kernel::args::parse_u64_arg;
use swarm_kernel::format::{json_text, print_job_status};
use swarm_kernel::ids::JobId;
use swarm_kernel::job_types::JobStatus;
use swarm_kernel::process::{force_terminate_pid, process_is_alive, terminate_pid};
use swarm_mcp::report::runtime_processes_json;
use swarm_store::job::JobRecord;
use swarm_store::monitor_store::alerts_json;
use swarm_store::repos::job_repo::{FileJobRepo, JobRepo};
use swarm_store::store::{
    job_store_dir, now_ms, read_text_tail, session_dir, MAX_ARTIFACT_TEXT_BYTES,
    MAX_SESSION_EVENTS_TAIL_BYTES,
};
use swarm_store::OsProcessLiveness;

const CANCEL_EXIT_CODE: i32 = 130;

enum CancelDecision {
    Already { status: String },
    Cancelled(Box<JobRecord>),
}

fn cancel_job_record(mut record: JobRecord, mut terminate: impl FnMut(u32)) -> CancelDecision {
    if record.status != JobStatus::Queued && record.status != JobStatus::Running {
        return CancelDecision::Already {
            status: record.status.to_string(),
        };
    }
    if let Some(pid) = record.pid {
        terminate(pid);
    }
    record.status = JobStatus::Cancelled;
    record.completed_at_ms = Some(now_ms());
    record.exit_code = Some(CANCEL_EXIT_CODE);
    CancelDecision::Cancelled(Box::new(record))
}

pub(crate) fn cmd_status(raw: &[String]) -> Result<i32, String> {
    let repo = FileJobRepo::new(job_store_dir()?);
    if let Some(id) = raw.first() {
        repo.reconcile_liveness(&OsProcessLiveness)
            .map_err(|e| e.to_string())?;
        let record = repo
            .get(&JobId::from(id.as_str()))
            .map_err(|e| e.to_string())?;
        print_job_status(&record);
        return Ok(0);
    }

    repo.reconcile_liveness(&OsProcessLiveness)
        .map_err(|e| e.to_string())?;
    let mut records = repo.list().map_err(|e| e.to_string())?;
    records.sort_by_key(|record| record.created_at_ms);
    records.reverse();

    if records.is_empty() {
        println!("No partner jobs found.");
        return Ok(0);
    }

    for record in records.iter().take(20) {
        print_job_status(record);
    }
    Ok(0)
}

pub(crate) fn cmd_result(raw: &[String]) -> Result<i32, String> {
    let repo = FileJobRepo::new(job_store_dir()?);
    repo.reconcile_liveness(&OsProcessLiveness)
        .map_err(|e| e.to_string())?;
    let record = if let Some(id) = raw.first() {
        repo.get(&JobId::from(id.as_str()))
            .map_err(|e| e.to_string())?
    } else {
        repo.latest()
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "Error: no partner jobs found".to_string())?
    };

    print_job_status(&record);
    println!();
    if record.status == JobStatus::Queued || record.status == JobStatus::Running {
        println!("Job is still {}.", record.status);
        return Ok(0);
    }

    let result = fs::read_to_string(&record.result_path).unwrap_or_default();
    if !result.trim().is_empty() {
        print!("{result}");
        if !result.ends_with('\n') {
            println!();
        }
    }

    let stderr = fs::read_to_string(&record.stderr_path).unwrap_or_default();
    if record.status != JobStatus::Completed && !stderr.trim().is_empty() {
        eprintln!("{stderr}");
    }
    Ok(record.exit_code.unwrap_or(0))
}

pub(crate) fn cmd_sessions() -> Result<i32, String> {
    let mut sessions = list_sessions()?;
    sessions.sort_by_key(|session| session.created_at_ms);
    sessions.reverse();
    if sessions.is_empty() {
        println!("No discussion sessions found.");
        return Ok(0);
    }
    for session in sessions.iter().take(20) {
        println!(
            "{}  {:10} {}  {}",
            session.id, session.status, session.created_at_ms, session.prompt_preview
        );
    }
    Ok(0)
}

pub(crate) fn cmd_runtime_processes(raw: &[String]) -> Result<i32, String> {
    let json_mode = raw.iter().any(|arg| arg == "--json");
    let cull = raw.iter().any(|arg| arg == "--cull");
    let data = runtime_processes_json()?;
    if cull {
        return cmd_runtime_processes_cull(data, json_mode);
    }
    if json_mode {
        println!("{}", json_text(data));
        return Ok(0);
    }

    let processes = data
        .get("processes")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "Error: invalid processes format".to_string())?;

    if processes.is_empty() {
        println!("No running processes found.");
        return Ok(0);
    }

    println!(
        "{:<8} {:<28} {:<10} {:<6} {:<6} {:<10} DETAILS/PREVIEW",
        "KIND", "ID", "STATUS", "PID", "ALIVE", "ELAPSED"
    );
    println!("{}", "-".repeat(95));

    for p in processes {
        let kind = p.get("kind").and_then(|v| v.as_str()).unwrap_or("-");
        let id = p.get("id").and_then(|v| v.as_str()).unwrap_or("-");
        let status = p.get("status").and_then(|v| v.as_str()).unwrap_or("-");
        let pid = p
            .get("pid")
            .and_then(|v| v.as_u64())
            .map(|v| v.to_string())
            .unwrap_or_else(|| "-".to_string());
        let alive = p
            .get("alive")
            .and_then(|v| v.as_bool())
            .map(|b| if b { "true" } else { "false" })
            .unwrap_or("false");

        let elapsed_ms = p.get("elapsed_ms").and_then(|v| v.as_u64());
        let elapsed = match elapsed_ms {
            Some(ms) => {
                let secs = ms / 1000;
                if secs < 60 {
                    format!("{secs}s")
                } else {
                    let mins = secs / 60;
                    let remaining_secs = secs % 60;
                    format!("{mins}m {remaining_secs}s")
                }
            }
            None => "-".to_string(),
        };

        let details = if kind == "job" {
            let agent = p.get("agent").and_then(|v| v.as_str()).unwrap_or("");
            let mode = p.get("mode").and_then(|v| v.as_str()).unwrap_or("");
            let cwd = p.get("cwd").and_then(|v| v.as_str()).unwrap_or("");
            let preview = p
                .get("prompt_preview")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if !agent.is_empty() && !mode.is_empty() {
                format!(
                    "[{} / {}] {}",
                    agent,
                    mode,
                    if !cwd.is_empty() { cwd } else { preview }
                )
            } else {
                preview.to_string()
            }
        } else {
            p.get("prompt_preview")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        };

        let details_preview = if details.len() > 60 {
            format!("{}...", &details[..57])
        } else {
            details
        };

        println!(
            "{:<8} {:<28} {:<10} {:<6} {:<6} {:<10} {}",
            kind, id, status, pid, alive, elapsed, details_preview
        );
    }

    Ok(0)
}

fn cmd_runtime_processes_cull(data: serde_json::Value, json_mode: bool) -> Result<i32, String> {
    let processes = data
        .get("processes")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "Error: invalid processes format".to_string())?;
    let candidates = processes
        .iter()
        .filter(|process| {
            process.get("status").and_then(|v| v.as_str()) == Some("stuck")
                && process.get("cullable").and_then(|v| v.as_bool()) == Some(true)
        })
        .filter_map(|process| {
            let pid = process
                .get("pid")?
                .as_u64()
                .and_then(|pid| u32::try_from(pid).ok())?;
            Some((
                pid,
                process
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("untracked")
                    .to_string(),
                process
                    .get("process_state")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string(),
            ))
        })
        .collect::<Vec<_>>();

    let mut results = Vec::new();
    for (pid, id, process_state) in candidates {
        terminate_pid(pid);
        thread::sleep(Duration::from_millis(150));
        if process_is_alive(pid) {
            force_terminate_pid(pid);
            thread::sleep(Duration::from_millis(150));
        }
        let pid_still_exists = process_is_alive(pid);
        let reap_required = process_state == "Dead" && pid_still_exists;
        results.push(serde_json::json!({
            "id": id,
            "pid": pid,
            "process_state": process_state,
            "culled": !pid_still_exists,
            "pid_still_exists": pid_still_exists,
            "reap_required": reap_required,
            "reboot_required": pid_still_exists && !reap_required,
        }));
    }

    let payload = serde_json::json!({
        "schema": "agent-swarm/runtime-cull/v1",
        "attempted": results.len(),
        "results": results,
    });
    if json_mode {
        println!("{}", json_text(payload));
    } else if payload["attempted"].as_u64().unwrap_or(0) == 0 {
        println!("No stuck untracked agent-swarm processes found.");
    } else {
        for result in payload["results"].as_array().unwrap_or(&Vec::new()) {
            let pid = result.get("pid").and_then(|v| v.as_u64()).unwrap_or(0);
            let id = result.get("id").and_then(|v| v.as_str()).unwrap_or("-");
            let state = result
                .get("process_state")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            if result
                .get("culled")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                println!("Culled {id} pid={pid} state={state}.");
            } else {
                println!("Could not cull {id} pid={pid} state={state}; reboot required.");
            }
        }
    }
    Ok(0)
}

pub(crate) fn cmd_cancel(raw: &[String]) -> Result<i32, String> {
    let id = raw
        .first()
        .ok_or_else(|| "Error: cancel requires a job id".to_string())?;
    let repo = FileJobRepo::new(job_store_dir()?);
    repo.reconcile_liveness(&OsProcessLiveness)
        .map_err(|e| e.to_string())?;
    let record = repo
        .get(&JobId::from(id.as_str()))
        .map_err(|e| e.to_string())?;
    match cancel_job_record(record, terminate_pid) {
        CancelDecision::Already { status } => {
            println!("Job {id} is already {status}.");
        }
        CancelDecision::Cancelled(record) => {
            repo.save(&record).map_err(|e| e.to_string())?;
            println!("Cancelled partner job {id}.");
        }
    }
    Ok(0)
}

pub(crate) fn cmd_session_events(raw: &[String]) -> Result<i32, String> {
    let id = raw
        .first()
        .ok_or_else(|| "Error: events requires a session id".to_string())?;
    let path = session_dir(id)?.join("events.jsonl");
    let text = read_text_tail(&path, MAX_SESSION_EVENTS_TAIL_BYTES)?;
    print!("{text}");
    Ok(0)
}

pub(crate) fn cmd_session_transcript(raw: &[String]) -> Result<i32, String> {
    let id = raw
        .first()
        .ok_or_else(|| "Error: transcript requires a session id".to_string())?;
    let path = session_dir(id)?.join("transcript.md");
    let text = read_text_tail(&path, MAX_ARTIFACT_TEXT_BYTES)?;
    print!("{text}");
    Ok(0)
}

pub(crate) fn cmd_alerts(raw: &[String]) -> Result<i32, String> {
    let (since_ts_ms, limit) = parse_alert_args(raw)?;
    println!("{}", json_text(alerts_json(since_ts_ms, limit)?));
    Ok(0)
}

fn parse_alert_args(raw: &[String]) -> Result<(Option<u128>, usize), String> {
    let mut since = None;
    let mut limit = 50usize;
    let mut index = 0;
    while index < raw.len() {
        match raw[index].as_str() {
            "--since" | "--since-ts-ms" => {
                index += 1;
                since = Some(u128::from(parse_u64_arg(raw.get(index), "since")?));
            }
            "--limit" => {
                index += 1;
                limit = parse_u64_arg(raw.get(index), "limit")? as usize;
            }
            other => return Err(format!("Error: unknown alerts option `{other}`")),
        }
        index += 1;
    }
    Ok((since, limit.clamp(1, 500)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use swarm_kernel::job_types::{JobAgent, JobMode, JobStatus};

    #[test]
    fn session_events_requires_session_id() {
        let err = cmd_session_events(&[]).unwrap_err();

        assert!(err.contains("requires a session id"));
    }

    #[test]
    fn session_transcript_requires_session_id() {
        let err = cmd_session_transcript(&[]).unwrap_err();

        assert!(err.contains("requires a session id"));
    }

    #[test]
    fn status_rejects_unknown_job_id() {
        let err = cmd_status(&["missing-job-for-cli-commands-test".to_string()]).unwrap_err();

        // After FileJobRepo migration: repo.get() on a missing id returns
        // RepoError::NotFound, which displays as "not found: <id>".
        // (Before migration the error was "Error reading job record metadata …".)
        assert!(err.contains("not found"), "unexpected error: {err}");
    }

    #[test]
    fn result_rejects_unknown_job_id() {
        let err = cmd_result(&["missing-job-for-cli-commands-test".to_string()]).unwrap_err();

        // After FileJobRepo migration: repo.get() returns RepoError::NotFound → "not found: <id>".
        assert!(err.contains("not found"), "unexpected error: {err}");
    }

    #[test]
    fn cancel_requires_job_id() {
        let err = cmd_cancel(&[]).unwrap_err();

        assert!(err.contains("cancel requires a job id"));
    }

    #[test]
    fn cancel_leaves_terminal_record_unchanged() {
        let decision = cancel_job_record(sample_record("completed", Some(999)), |_| {
            panic!("terminal jobs should not be signalled")
        });

        match decision {
            CancelDecision::Already { status } => assert_eq!(status, "completed"),
            CancelDecision::Cancelled(_) => panic!("terminal job should not be cancelled"),
        }
        // Note: CancelDecision::Already.status is a String (via .to_string()) so "completed" comparison works.
    }

    #[test]
    fn cancel_running_record_signals_pid_and_updates_status() {
        let mut signalled = Vec::new();
        let decision = cancel_job_record(sample_record("running", Some(42)), |pid| {
            signalled.push(pid);
        });

        let CancelDecision::Cancelled(record) = decision else {
            panic!("running job should be cancelled");
        };
        assert_eq!(signalled, vec![42]);
        assert_eq!(record.status, JobStatus::Cancelled);
        assert_eq!(record.exit_code, Some(CANCEL_EXIT_CODE));
        assert!(record.completed_at_ms.unwrap_or_default() > 0);
    }

    #[test]
    fn cancel_queued_record_without_pid_updates_status() {
        let decision = cancel_job_record(sample_record("queued", None), |_| {
            panic!("queued jobs without pid should not be signalled")
        });

        let CancelDecision::Cancelled(record) = decision else {
            panic!("queued job should be cancelled");
        };
        assert_eq!(record.pid, None);
        assert_eq!(record.status, JobStatus::Cancelled);
        assert_eq!(record.exit_code, Some(CANCEL_EXIT_CODE));
        assert!(record.completed_at_ms.unwrap_or_default() > 0);
    }

    fn sample_record(status: &str, pid: Option<u32>) -> JobRecord {
        let parsed_status: JobStatus = serde_json::from_str(&format!("\"{status}\"")).unwrap();
        JobRecord {
            id: "job-cli-cancel-test".into(),
            status: parsed_status,
            agent: JobAgent::Gemini,
            model: None,
            mode: JobMode::Agent,
            cwd: "/tmp".to_string(),
            prompt_preview: "preview".to_string(),
            timeout_secs: 300,
            created_at_ms: 1,
            started_at_ms: Some(1),
            completed_at_ms: None,
            pid,
            exit_code: None,
            prompt_path: "/tmp/job.prompt.md".to_string(),
            stdout_path: "/tmp/job.stdout.log".to_string(),
            stderr_path: "/tmp/job.stderr.log".to_string(),
            result_path: "/tmp/job.result.txt".to_string(),
            allow_recursive_codex: false,
        }
    }
}
