//! Background job runtime: enqueue a partner job as a detached worker process
//! and the two worker entry points that the detached process re-execs into.
//!
//! `start_background_job` writes a queued `JobRecord`, then spawns a detached
//! `__job-worker <id>` (or, for command jobs started elsewhere, a
//! `__command-worker` token) re-exec of the current executable and flips the
//! record to running. The worker entry points (`cmd_job_worker`,
//! `cmd_command_worker`) run inside that detached child: they load the record,
//! perform the work (an in-process partner run or a captured subcommand
//! re-exec), and persist terminal status. Every dependency here lives in an
//! already-extracted module; this seam pulls no swarm/discussion orchestration.

use std::env;
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use swarm_kernel::agent::agent_name;
use swarm_kernel::args::{parse_agent_choice, Args};
use swarm_kernel::format::prompt_preview;
use swarm_kernel::ids::JobId;
use swarm_kernel::job_types::{JobAgent, JobMode, JobStatus};
use swarm_kernel::process::{detach_background_command, exit_code};
use swarm_kernel::resolver::resolve_agent;
use swarm_store::job::JobRecord;
use swarm_store::repos::job_repo::{FileJobRepo, JobRepo};
use swarm_store::store::{job_store_dir, new_job_id, now_ms, write_text_atomic};

use crate::executor::execute_partner;

pub fn start_background_job(args: Args, prompt: String) -> Result<i32, String> {
    let agent = resolve_agent(args.agent)?;
    let job_dir = job_store_dir()?;
    fs::create_dir_all(&job_dir)
        .map_err(|err| format!("Error creating job directory {}: {err}", job_dir.display()))?;

    let id = new_job_id();
    let prompt_path = job_dir.join(format!("{id}.prompt.md"));
    let stdout_path = job_dir.join(format!("{id}.stdout.log"));
    let stderr_path = job_dir.join(format!("{id}.stderr.log"));
    let result_path = job_dir.join(format!("{id}.result.txt"));

    write_text_atomic(&prompt_path, prompt)?;

    // Convert AgentChoice → JobAgent for persistence.
    let job_agent = JobAgent::from_agent_name(agent_name(agent));
    // Background jobs start in Queued state (pre-spawn), then flip to Running
    // after the child process is launched. We construct the record manually and
    // use repo.save() because FileJobRepo::create() always emits Running status.
    let repo = FileJobRepo::new(job_dir.clone());
    let record = JobRecord {
        id: id.clone(),
        status: JobStatus::Queued,
        agent: job_agent,
        model: args.model.clone(),
        mode: if args.quiet {
            JobMode::Consult
        } else {
            JobMode::Agent
        },
        cwd: args.cwd.display().to_string(),
        // PROMPT_PREVIEW: pre-truncate before storing.
        prompt_preview: prompt_preview(&args.prompt),
        timeout_secs: args.timeout_secs,
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
    repo.save(&record).map_err(|e| e.to_string())?;

    let current_exe =
        env::current_exe().map_err(|err| format!("Error locating current executable: {err}"))?;
    let worker_stderr = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&stderr_path)
        .map_err(|err| {
            format!(
                "Error opening worker stderr {}: {err}",
                stderr_path.display()
            )
        })?;
    let mut command = Command::new(current_exe);
    command
        .arg("__job-worker")
        .arg(id.as_str())
        .current_dir(&args.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(worker_stderr));
    detach_background_command(&mut command);
    let child = command
        .spawn()
        .map_err(|err| format!("Error starting background job: {err}"))?;

    if let Ok(mut latest) = repo.get(&id) {
        if latest.status == JobStatus::Queued {
            latest.status = JobStatus::Running;
            latest.started_at_ms = Some(now_ms());
            latest.pid = Some(child.id());
            repo.save(&latest).map_err(|e| e.to_string())?;
        }
    }

    println!("Queued partner job {id}");
    println!("  agent:  {}", agent.display_name());
    if let Some(model) = args.model.as_deref() {
        println!("  model:  {model}");
    }
    println!("  mode:   {}", if args.quiet { "consult" } else { "agent" });
    println!("  cwd:    {}", args.cwd.display());
    println!("  status: agent-swarm status {id}");
    println!("  result: agent-swarm result {id}");
    Ok(0)
}

pub fn cmd_command_worker(raw: &[String]) -> Result<i32, String> {
    let (id, command_args) = raw
        .split_first()
        .ok_or_else(|| "Error: __command-worker requires a job id".to_string())?;
    if command_args.is_empty() {
        return Err("Error: __command-worker requires command arguments".to_string());
    }
    let repo = FileJobRepo::new(job_store_dir()?);
    let mut record = repo
        .get(&JobId::from(id.as_str()))
        .map_err(|e| e.to_string())?;
    record.status = JobStatus::Running;
    record.started_at_ms.get_or_insert_with(now_ms);
    record.pid = Some(std::process::id());
    repo.save(&record).map_err(|e| e.to_string())?;

    let current_exe =
        env::current_exe().map_err(|err| format!("Error locating current executable: {err}"))?;
    let output = Command::new(current_exe)
        .args(command_args)
        .current_dir(&record.cwd)
        .stdin(Stdio::null())
        .output()
        .map_err(|err| format!("Error executing background command: {err}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let code = exit_code(Some(output.status));
    write_text_atomic(Path::new(&record.stdout_path), &stdout)?;
    write_text_atomic(Path::new(&record.stderr_path), &stderr)?;
    write_text_atomic(Path::new(&record.result_path), &stdout)?;
    record.status = if code == 0 {
        JobStatus::Completed
    } else {
        JobStatus::Failed
    };
    record.exit_code = Some(code);
    record.completed_at_ms = Some(now_ms());
    repo.save(&record).map_err(|e| e.to_string())?;
    Ok(code)
}

pub fn cmd_job_worker(raw: &[String]) -> Result<i32, String> {
    let id = raw
        .first()
        .ok_or_else(|| "Error: __job-worker requires a job id".to_string())?;
    let repo = FileJobRepo::new(job_store_dir()?);
    let mut record = repo
        .get(&JobId::from(id.as_str()))
        .map_err(|e| e.to_string())?;
    record.status = JobStatus::Running;
    record.started_at_ms.get_or_insert_with(now_ms);
    record.pid = Some(std::process::id());
    repo.save(&record).map_err(|e| e.to_string())?;

    let prompt = fs::read_to_string(&record.prompt_path)
        .map_err(|err| format!("Error reading job prompt {}: {err}", record.prompt_path))?;
    // JobAgent → AgentChoice via as_str() — parse_agent_choice handles the &str boundary.
    let agent = parse_agent_choice(record.agent.as_str())?;
    let args = Args {
        prompt: prompt.clone(),
        cwd: PathBuf::from(&record.cwd),
        timeout_secs: record.timeout_secs,
        quiet: record.mode == JobMode::Consult,
        agent,
        // Persisted jobs carry a built-in JobAgent today; custom-backend
        // background jobs are a follow-up (needs a wire-format field).
        agent_custom: None,
        model: record.model.clone(),
        persona: None,
        background: false,
        allow_bypass_permissions: false,
    };
    let agent = resolve_agent(agent)?;
    let registry =
        crate::backend_registry::BackendRegistry::from_config(&swarm_kernel::config::load_config());
    let output = execute_partner(&registry, agent, &args, &prompt);

    match output {
        Ok(output) => {
            let code = output.exit_status.unwrap_or(1);
            write_text_atomic(Path::new(&record.stdout_path), &output.stdout)?;
            write_text_atomic(Path::new(&record.stderr_path), &output.stderr)?;
            write_text_atomic(Path::new(&record.result_path), &output.stdout)?;
            record.status = if output.timed_out {
                JobStatus::TimedOut
            } else if code == 0 {
                JobStatus::Completed
            } else {
                JobStatus::Failed
            };
            record.exit_code = Some(if output.timed_out { 124 } else { code });
        }
        Err(err) => {
            write_text_atomic(Path::new(&record.stderr_path), &err)?;
            record.status = JobStatus::Failed;
            record.exit_code = Some(1);
        }
    }

    record.completed_at_ms = Some(now_ms());
    repo.save(&record).map_err(|e| e.to_string())?;
    Ok(record.exit_code.unwrap_or(1))
}

#[cfg(test)]
mod tests {
    use super::{cmd_command_worker, cmd_job_worker};

    // These guard-clause branches return before any filesystem read, process
    // re-exec, or partner spawn — the only paths through these functions that
    // are safe to exercise in isolation. The happy paths always re-exec
    // `env::current_exe()` (which is the test harness under `cargo test`, not
    // agent-swarm) or spawn a live partner, so they are deliberately not
    // unit-tested here.

    #[test]
    fn cmd_job_worker_requires_a_job_id() {
        let err = cmd_job_worker(&[]).unwrap_err();
        assert!(err.contains("requires a job id"), "unexpected error: {err}");
    }

    #[test]
    fn cmd_command_worker_requires_a_job_id() {
        let err = cmd_command_worker(&[]).unwrap_err();
        assert!(err.contains("requires a job id"), "unexpected error: {err}");
    }

    #[test]
    fn cmd_command_worker_requires_command_arguments() {
        let err = cmd_command_worker(&["job-123".to_string()]).unwrap_err();
        assert!(
            err.contains("requires command arguments"),
            "unexpected error: {err}"
        );
    }
}
