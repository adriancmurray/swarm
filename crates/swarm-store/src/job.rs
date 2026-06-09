//! Job record helpers — moved from agent-swarm's `job.rs` in P5-S2.
//!
//! Uses `swarm_contracts::jobs::*` directly instead of `crate::job_types::*`
//! (job_types in agent-swarm is just a re-export shim of swarm_contracts::jobs).

use std::fs;
use std::path::Path;

use swarm_contracts::jobs::{JobAgent, JobMode, JobStatus};

use crate::store::{
    new_job_id, now_ms, validate_store_id, write_text_atomic, MAX_ARTIFACT_TEXT_BYTES,
};

pub use swarm_contracts::jobs::JobRecord;

/// `prompt_for_preview` is stored verbatim — no truncation is applied here.
/// Callers (e.g. `JobRepo::create` via `JobSpec`) must pass an already-truncated
/// preview.
// Public API inherited from the original job store; callers already pass these
// fields positionally and a params struct would duplicate `JobSpec` for no clarity.
#[allow(clippy::too_many_arguments)]
pub fn create_tracking_record_in(
    job_dir: &Path,
    agent: JobAgent,
    model: Option<String>,
    mode: JobMode,
    cwd: &Path,
    prompt_for_preview: &str,
    prompt_for_file: &str,
    timeout_secs: u64,
    allow_recursive_codex: bool,
) -> Result<JobRecord, String> {
    fs::create_dir_all(job_dir)
        .map_err(|err| format!("Error creating job directory {}: {err}", job_dir.display()))?;

    let id = new_job_id();
    let prompt_path = job_dir.join(format!("{id}.prompt.md"));
    let stdout_path = job_dir.join(format!("{id}.stdout.log"));
    let stderr_path = job_dir.join(format!("{id}.stderr.log"));
    let result_path = job_dir.join(format!("{id}.result.txt"));

    write_text_atomic(&prompt_path, prompt_for_file)?;

    let created_at_ms = now_ms();
    let record = JobRecord {
        id,
        status: JobStatus::Running,
        agent,
        model,
        mode,
        cwd: cwd.display().to_string(),
        prompt_preview: prompt_for_preview.to_string(),
        timeout_secs,
        created_at_ms,
        started_at_ms: Some(created_at_ms),
        completed_at_ms: None,
        pid: Some(std::process::id()),
        exit_code: None,
        prompt_path: prompt_path.display().to_string(),
        stdout_path: stdout_path.display().to_string(),
        stderr_path: stderr_path.display().to_string(),
        result_path: result_path.display().to_string(),
        allow_recursive_codex,
    };
    write_job_record_in(job_dir, &record)?;
    Ok(record)
}

/// Reads all job records from `dir`, skipping unparseable or oversized files.
pub fn list_job_records_in(dir: &Path) -> Result<Vec<JobRecord>, String> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Ok(Vec::new());
    };
    let mut records = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        if fs::metadata(&path)
            .map(|metadata| metadata.len() as usize > MAX_ARTIFACT_TEXT_BYTES)
            .unwrap_or(false)
        {
            continue;
        }
        if let Ok(content) = fs::read_to_string(&path) {
            if let Ok(record) = serde_json::from_str::<JobRecord>(&content) {
                records.push(record);
            }
        }
    }
    Ok(records)
}

/// Read a single job record by id from `dir`.
///
/// Returns a `String` error whose message encodes the failure: ENOENT produces
/// "Error reading job record metadata ...: No such file or directory (os error 2)",
/// which `map_job_err` in `job_repo.rs` maps to `RepoError::NotFound`.
pub fn read_job_record_in(dir: &Path, id: &str) -> Result<JobRecord, String> {
    validate_store_id(id)?;
    let path = dir.join(format!("{id}.json"));
    let len = fs::metadata(&path)
        .map_err(|err| {
            format!(
                "Error reading job record metadata {}: {err}",
                path.display()
            )
        })?
        .len() as usize;
    if len > MAX_ARTIFACT_TEXT_BYTES {
        return Err(format!(
            "Error reading job record {}: record is too large ({len} bytes)",
            path.display()
        ));
    }
    let content = fs::read_to_string(&path)
        .map_err(|err| format!("Error reading job record {}: {err}", path.display()))?;
    serde_json::from_str(&content)
        .map_err(|err| format!("Error parsing job record {}: {err}", path.display()))
}

pub fn write_job_record_in(dir: &Path, record: &JobRecord) -> Result<(), String> {
    validate_store_id(record.id.as_str())?;
    let path = dir.join(format!("{}.json", record.id.as_str()));
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| format!("Error creating job directory {}: {err}", parent.display()))?;
    }
    let tmp = path.with_extension("json.tmp");
    let json = serde_json::to_string_pretty(record)
        .map_err(|err| format!("Error serializing job record: {err}"))?;
    fs::write(&tmp, format!("{json}\n"))
        .map_err(|err| format!("Error writing job record {}: {err}", tmp.display()))?;
    fs::rename(&tmp, &path)
        .map_err(|err| format!("Error replacing job record {}: {err}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_record_roundtrip_serialize_deserialize() {
        let original = JobRecord {
            id: "job-test-123".into(),
            status: JobStatus::Running,
            agent: JobAgent::Gemini,
            model: Some("flash".to_string()),
            mode: JobMode::Agent,
            cwd: "/tmp".to_string(),
            prompt_preview: "hello".to_string(),
            timeout_secs: 450,
            created_at_ms: 100_000,
            started_at_ms: Some(100_005),
            completed_at_ms: None,
            pid: Some(9999),
            exit_code: None,
            prompt_path: "/tmp/job-test-123.prompt.md".to_string(),
            stdout_path: "/tmp/job-test-123.stdout.log".to_string(),
            stderr_path: "/tmp/job-test-123.stderr.log".to_string(),
            result_path: "/tmp/job-test-123.result.txt".to_string(),
            allow_recursive_codex: true,
        };

        let encoded = serde_json::to_string(&original).unwrap();
        let decoded: JobRecord = serde_json::from_str(&encoded).unwrap();
        assert_eq!(original, decoded);

        // Legacy record without timeout_secs or allow_recursive_codex — must still parse.
        // DEFAULT_TIMEOUT_SECS = 300 (from agent-swarm args.rs).
        let legacy_json = r#"{
            "id": "job-test-legacy",
            "status": "running",
            "agent": "gemini",
            "model": null,
            "mode": "agent",
            "cwd": "/tmp",
            "prompt_preview": "hello",
            "created_at_ms": 100000,
            "started_at_ms": null,
            "completed_at_ms": null,
            "pid": null,
            "exit_code": null,
            "prompt_path": "/tmp/job.prompt.md",
            "stdout_path": "/tmp/job.stdout.log",
            "stderr_path": "/tmp/job.stderr.log",
            "result_path": "/tmp/job.result.txt"
        }"#;
        let legacy: JobRecord = serde_json::from_str(legacy_json).unwrap();
        assert_eq!(legacy.timeout_secs, 300); // DEFAULT_TIMEOUT_SECS value
        assert!(!legacy.allow_recursive_codex);
        assert_eq!(legacy.status, JobStatus::Running);
        assert_eq!(legacy.agent, JobAgent::Gemini);
        assert_eq!(legacy.mode, JobMode::Agent);

        // Record with swarm agent — must round-trip without losing the value.
        let swarm_json = r#"{
            "id": "job-test-swarm",
            "status": "queued",
            "agent": "swarm",
            "model": null,
            "mode": "discussion",
            "cwd": "/tmp",
            "prompt_preview": "council task",
            "timeout_secs": 450,
            "created_at_ms": 200000,
            "started_at_ms": null,
            "completed_at_ms": null,
            "pid": null,
            "exit_code": null,
            "prompt_path": "/tmp/job2.prompt.md",
            "stdout_path": "/tmp/job2.stdout.log",
            "stderr_path": "/tmp/job2.stderr.log",
            "result_path": "/tmp/job2.result.txt"
        }"#;
        let swarm: JobRecord = serde_json::from_str(swarm_json).unwrap();
        assert_eq!(swarm.agent, JobAgent::Swarm);
        assert_eq!(swarm.status, JobStatus::Queued);
        assert_eq!(swarm.mode, JobMode::Discussion);
        let re_encoded = serde_json::to_string(&swarm).unwrap();
        assert!(re_encoded.contains("\"swarm\""));
        assert!(re_encoded.contains("\"queued\""));
        assert!(re_encoded.contains("\"discussion\""));

        // Record with unknown status — must deserialize to Other and re-serialize identically.
        let future_json = r#"{
            "id": "job-test-future",
            "status": "some_future_state",
            "agent": "unknown_bot",
            "model": null,
            "mode": "agent",
            "cwd": "/tmp",
            "prompt_preview": "future task",
            "timeout_secs": 300,
            "created_at_ms": 300000,
            "started_at_ms": null,
            "completed_at_ms": null,
            "pid": null,
            "exit_code": null,
            "prompt_path": "/tmp/job3.prompt.md",
            "stdout_path": "/tmp/job3.stdout.log",
            "stderr_path": "/tmp/job3.stderr.log",
            "result_path": "/tmp/job3.result.txt"
        }"#;
        let future: JobRecord = serde_json::from_str(future_json).unwrap();
        assert_eq!(future.status, JobStatus::Other("some_future_state".into()));
        assert_eq!(future.agent, JobAgent::Other("unknown_bot".into()));
        let re_future = serde_json::to_string(&future).unwrap();
        assert!(re_future.contains("\"some_future_state\""));
        assert!(re_future.contains("\"unknown_bot\""));
    }

    #[test]
    fn create_tracking_record_in_stores_preview_verbatim() {
        let dir = tempfile::tempdir().unwrap();
        let long_preview = "x".repeat(200);
        let rec = create_tracking_record_in(
            dir.path(),
            JobAgent::Gemini,
            None,
            JobMode::Consult,
            dir.path(),
            &long_preview,
            "full prompt text",
            60,
            false,
        )
        .unwrap();
        assert_eq!(
            rec.prompt_preview, long_preview,
            "create_tracking_record_in must store the preview verbatim (no truncation)"
        );
    }
}
