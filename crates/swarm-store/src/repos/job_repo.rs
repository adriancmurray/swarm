//! `JobRepo` trait — Phase-1 repository abstraction for job records.
//!
//! Moved from agent-swarm's `repos/job_repo.rs` in P5-S2.
//! Uses `swarm_contracts::jobs::*` and `swarm_contracts::ids::JobId` directly.

pub use swarm_core::{JobRepo, JobSpec};

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use swarm_contracts::ids::JobId;
use swarm_contracts::jobs::{JobRecord, JobStatus};

use crate::job::{
    create_tracking_record_in, list_job_records_in, read_job_record_in, write_job_record_in,
};
use crate::repos::{ProcessLiveness, RepoError};
use crate::store::{new_job_id, now_ms};

fn map_job_err(id: &str, e: String) -> RepoError {
    if e.contains("os error 2") || e.contains("No such file") {
        RepoError::NotFound(id.to_string())
    } else {
        RepoError::Io(io::Error::other(e))
    }
}

fn mark_lost(record: &mut JobRecord) {
    record.status = JobStatus::Lost;
    record.completed_at_ms = Some(now_ms());
    record.exit_code = Some(1);
}

fn should_flip_to_lost(record: &JobRecord, liveness: &dyn ProcessLiveness) -> bool {
    if record.status != JobStatus::Running {
        return false;
    }
    match record.pid {
        None => false,
        Some(0) => true,
        Some(p) => !liveness.is_alive(p),
    }
}

// ── FileJobRepo ───────────────────────────────────────────────────────────────

pub struct FileJobRepo {
    base_dir: PathBuf,
}

impl FileJobRepo {
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: base_dir.into(),
        }
    }
}

impl JobRepo for FileJobRepo {
    fn create(&self, spec: JobSpec) -> Result<JobRecord, RepoError> {
        create_tracking_record_in(
            &self.base_dir,
            spec.agent,
            spec.model,
            spec.mode,
            &spec.cwd,
            &spec.prompt_preview,
            &spec.prompt_text,
            spec.timeout_secs,
            spec.allow_recursive_codex,
        )
        .map_err(|e| RepoError::Io(io::Error::other(e)))
    }

    fn get(&self, id: &JobId) -> Result<JobRecord, RepoError> {
        let id_str = id.as_str();
        read_job_record_in(&self.base_dir, id_str).map_err(|e| map_job_err(id_str, e))
    }

    fn list(&self) -> Result<Vec<JobRecord>, RepoError> {
        list_job_records_in(&self.base_dir).map_err(|e| RepoError::Io(io::Error::other(e)))
    }

    fn save(&self, record: &JobRecord) -> Result<(), RepoError> {
        write_job_record_in(&self.base_dir, record).map_err(|e| RepoError::Io(io::Error::other(e)))
    }

    fn latest(&self) -> Result<Option<JobRecord>, RepoError> {
        let mut records =
            list_job_records_in(&self.base_dir).map_err(|e| RepoError::Io(io::Error::other(e)))?;
        records.sort_by(|a, b| {
            a.created_at_ms
                .cmp(&b.created_at_ms)
                .then_with(|| a.id.as_str().cmp(b.id.as_str()))
        });
        Ok(records.pop())
    }

    fn reconcile_liveness(
        &self,
        liveness: &dyn ProcessLiveness,
    ) -> Result<Vec<JobRecord>, RepoError> {
        let records =
            list_job_records_in(&self.base_dir).map_err(|e| RepoError::Io(io::Error::other(e)))?;
        let mut flipped: Vec<JobRecord> = Vec::new();
        for mut record in records {
            if should_flip_to_lost(&record, liveness) {
                mark_lost(&mut record);
                let stderr = Path::new(&record.stderr_path);
                if fs::metadata(stderr).is_err() {
                    let _ = crate::store::write_text_atomic(
                        stderr,
                        "Worker process exited before updating the job record.\n",
                    );
                }
                write_job_record_in(&self.base_dir, &record)
                    .map_err(|e| RepoError::Io(io::Error::other(e)))?;
                flipped.push(record);
            }
        }
        flipped.sort_by(|a, b| {
            a.created_at_ms
                .cmp(&b.created_at_ms)
                .then_with(|| a.id.as_str().cmp(b.id.as_str()))
        });
        Ok(flipped)
    }
}

// ── MemJobRepo ────────────────────────────────────────────────────────────────

pub struct MemJobRepo {
    base_dir: PathBuf,
    records: Mutex<HashMap<JobId, JobRecord>>,
}

impl MemJobRepo {
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: base_dir.into(),
            records: Mutex::new(HashMap::new()),
        }
    }
}

impl JobRepo for MemJobRepo {
    fn create(&self, spec: JobSpec) -> Result<JobRecord, RepoError> {
        let id = new_job_id();
        let base = &self.base_dir;
        let created_at_ms = now_ms();
        let record = JobRecord {
            prompt_path: base.join(format!("{id}.prompt.md")).display().to_string(),
            stdout_path: base.join(format!("{id}.stdout.log")).display().to_string(),
            stderr_path: base.join(format!("{id}.stderr.log")).display().to_string(),
            result_path: base.join(format!("{id}.result.txt")).display().to_string(),
            id,
            status: JobStatus::Running,
            agent: spec.agent,
            model: spec.model,
            mode: spec.mode,
            cwd: spec.cwd.display().to_string(),
            prompt_preview: spec.prompt_preview,
            timeout_secs: spec.timeout_secs,
            created_at_ms,
            started_at_ms: Some(created_at_ms),
            completed_at_ms: None,
            pid: Some(std::process::id()),
            exit_code: None,
            allow_recursive_codex: spec.allow_recursive_codex,
        };
        self.records
            .lock()
            .unwrap()
            .insert(record.id.clone(), record.clone());
        Ok(record)
    }

    fn get(&self, id: &JobId) -> Result<JobRecord, RepoError> {
        self.records
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or_else(|| RepoError::NotFound(id.to_string()))
    }

    fn list(&self) -> Result<Vec<JobRecord>, RepoError> {
        Ok(self.records.lock().unwrap().values().cloned().collect())
    }

    fn save(&self, record: &JobRecord) -> Result<(), RepoError> {
        self.records
            .lock()
            .unwrap()
            .insert(record.id.clone(), record.clone());
        Ok(())
    }

    fn latest(&self) -> Result<Option<JobRecord>, RepoError> {
        let mut records: Vec<JobRecord> = self.records.lock().unwrap().values().cloned().collect();
        records.sort_by(|a, b| {
            a.created_at_ms
                .cmp(&b.created_at_ms)
                .then_with(|| a.id.as_str().cmp(b.id.as_str()))
        });
        Ok(records.pop())
    }

    fn reconcile_liveness(
        &self,
        liveness: &dyn ProcessLiveness,
    ) -> Result<Vec<JobRecord>, RepoError> {
        let mut map = self.records.lock().unwrap();
        let mut flipped: Vec<JobRecord> = Vec::new();
        for record in map.values_mut() {
            if should_flip_to_lost(record, liveness) {
                mark_lost(record);
                flipped.push(record.clone());
            }
        }
        flipped.sort_by(|a, b| {
            a.created_at_ms
                .cmp(&b.created_at_ms)
                .then_with(|| a.id.as_str().cmp(b.id.as_str()))
        });
        Ok(flipped)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repos::{AlwaysAlive, NeverAlive};
    use std::path::Path;
    use swarm_contracts::jobs::{JobAgent, JobMode};

    fn make_spec(preview: &str) -> JobSpec {
        JobSpec {
            agent: JobAgent::Claude,
            model: Some("sonnet".to_string()),
            mode: JobMode::Consult,
            cwd: PathBuf::from("/tmp"),
            prompt_preview: preview.to_string(),
            prompt_text: format!("full text for: {preview}"),
            timeout_secs: 300,
            allow_recursive_codex: false,
        }
    }

    fn job_repo_contract<R: JobRepo>(repo: &R, base_dir: &Path) {
        let created = repo.create(make_spec("alpha")).unwrap();
        assert_eq!(created.status, JobStatus::Running);
        assert_eq!(created.started_at_ms, Some(created.created_at_ms));
        assert!(created.pid.is_some());
        assert_eq!(created.prompt_preview, "alpha");

        let fetched = repo.get(&created.id).unwrap();
        assert_eq!(fetched, created);

        assert!(created.prompt_path.ends_with(".prompt.md"));
        assert!(created.stdout_path.ends_with(".stdout.log"));
        assert!(created.stderr_path.ends_with(".stderr.log"));
        assert!(created.result_path.ends_with(".result.txt"));
        let base_str = base_dir.display().to_string();
        assert!(created.prompt_path.starts_with(&base_str));

        let mut b = created.clone();
        b.id = JobId::from("job-contract-b-00000001");
        b.created_at_ms = 1_000_001;
        b.started_at_ms = Some(1_000_001);
        b.prompt_preview = "beta".to_string();
        repo.save(&b).unwrap();

        let mut c = created.clone();
        c.id = JobId::from("job-contract-c-00000002");
        c.created_at_ms = 1_000_002;
        c.started_at_ms = Some(1_000_002);
        c.prompt_preview = "gamma".to_string();
        repo.save(&c).unwrap();

        let all = repo.list().unwrap();
        assert_eq!(all.len(), 3);

        let pre_update_list = repo.list().unwrap();
        assert!(pre_update_list
            .iter()
            .all(|r| r.status == JobStatus::Running));

        let mut to_update = fetched.clone();
        to_update.status = JobStatus::Completed;
        to_update.completed_at_ms = Some(crate::store::now_ms());
        to_update.exit_code = Some(0);
        repo.save(&to_update).unwrap();
        let after_save = repo.get(&created.id).unwrap();
        assert_eq!(after_save.status, JobStatus::Completed);
        assert_eq!(after_save.exit_code, Some(0));

        let all_now = repo.list().unwrap();
        let expected_latest_id = all_now
            .iter()
            .max_by(|a, b_rec| {
                a.created_at_ms
                    .cmp(&b_rec.created_at_ms)
                    .then_with(|| a.id.as_str().cmp(b_rec.id.as_str()))
            })
            .map(|r| r.id.clone())
            .unwrap();
        let latest = repo.latest().unwrap().unwrap();
        assert_eq!(latest.id, expected_latest_id);

        let missing = JobId::from("job-does-not-exist-contract-test");
        let err = repo.get(&missing).unwrap_err();
        assert!(matches!(err, RepoError::NotFound(_)));
    }

    fn liveness_contract<R: JobRepo>(repo: &R) {
        let alive_record = repo.create(make_spec("liveness-alpha")).unwrap();
        let pid = alive_record.pid.unwrap();
        assert!(pid > 0);

        let flipped = repo.reconcile_liveness(&NeverAlive).unwrap();
        assert_eq!(flipped.len(), 1);
        let flipped_rec = &flipped[0];
        assert_eq!(flipped_rec.id, alive_record.id);
        assert_eq!(flipped_rec.status, JobStatus::Lost);
        assert!(flipped_rec.completed_at_ms.is_some());
        assert_eq!(flipped_rec.exit_code, Some(1));

        let post_reconcile = repo.get(&alive_record.id).unwrap();
        assert_eq!(post_reconcile.status, JobStatus::Lost);

        let flipped2 = repo.reconcile_liveness(&NeverAlive).unwrap();
        assert!(flipped2.is_empty());

        let still_alive = repo.create(make_spec("liveness-beta")).unwrap();
        let no_change = repo.reconcile_liveness(&AlwaysAlive).unwrap();
        assert!(no_change.is_empty());
        let after_alive = repo.get(&still_alive.id).unwrap();
        assert_eq!(after_alive.status, JobStatus::Running);

        let mut no_pid_record = repo.create(make_spec("liveness-no-pid")).unwrap();
        no_pid_record.pid = None;
        repo.save(&no_pid_record).unwrap();
        let flipped_no_pid = repo.reconcile_liveness(&NeverAlive).unwrap();
        assert!(!flipped_no_pid.iter().any(|r| r.id == no_pid_record.id));
        let no_pid_after = repo.get(&no_pid_record.id).unwrap();
        assert_eq!(no_pid_after.status, JobStatus::Running);

        let mut zero_pid_record = repo.create(make_spec("liveness-zero-pid")).unwrap();
        zero_pid_record.pid = Some(0);
        repo.save(&zero_pid_record).unwrap();
        let flipped_zero = repo.reconcile_liveness(&AlwaysAlive).unwrap();
        assert!(flipped_zero.iter().any(|r| r.id == zero_pid_record.id));
        let zero_after = repo.get(&zero_pid_record.id).unwrap();
        assert_eq!(zero_after.status, JobStatus::Lost);
    }

    #[test]
    fn mem_job_repo_contract() {
        let base = PathBuf::from("/mem/partner-jobs");
        let repo = MemJobRepo::new(base.clone());
        job_repo_contract(&repo, &base);
    }

    #[test]
    fn mem_job_repo_liveness_parity() {
        let repo = MemJobRepo::new("/mem/partner-jobs");
        liveness_contract(&repo);
    }

    #[test]
    fn file_job_repo_contract() {
        let dir = tempfile::tempdir().unwrap();
        let repo = FileJobRepo::new(dir.path());
        job_repo_contract(&repo, dir.path());
    }

    #[test]
    fn file_job_repo_liveness_parity() {
        let dir = tempfile::tempdir().unwrap();
        let repo = FileJobRepo::new(dir.path());
        liveness_contract(&repo);
    }

    #[test]
    fn file_job_repo_flip_writes_stderr_breadcrumb() {
        let dir = tempfile::tempdir().unwrap();
        let repo = FileJobRepo::new(dir.path());
        let record = repo.create(make_spec("breadcrumb-test")).unwrap();
        let stderr_path = PathBuf::from(&record.stderr_path);
        assert!(!stderr_path.exists());
        repo.reconcile_liveness(&NeverAlive).unwrap();
        assert!(stderr_path.exists());
        let content = fs::read_to_string(&stderr_path).unwrap();
        assert!(content.contains("Worker process exited"));
    }

    #[test]
    fn guard_prompt_preview_stored_truncated_for_long_input() {
        // Truncation happens at the call site (JobSpec.prompt_preview is pre-truncated).
        // This test verifies FileJobRepo stores what it's given verbatim.
        let dir = tempfile::tempdir().unwrap();
        let repo = FileJobRepo::new(dir.path());
        let raw_prompt = "x".repeat(200);
        // Simulate caller truncation (matches format::prompt_preview behavior)
        let normalized = raw_prompt.split_whitespace().collect::<Vec<_>>().join(" ");
        let preview = if normalized.chars().count() <= 72 {
            normalized
        } else {
            format!("{}...", normalized.chars().take(69).collect::<String>())
        };
        let spec = JobSpec {
            agent: JobAgent::Claude,
            model: None,
            mode: JobMode::Consult,
            cwd: PathBuf::from("/tmp"),
            prompt_preview: preview,
            prompt_text: raw_prompt.clone(),
            timeout_secs: 300,
            allow_recursive_codex: false,
        };
        let record = repo.create(spec).unwrap();
        assert!(
            record.prompt_preview.chars().count() <= 72,
            "stored prompt_preview must be <= 72 chars"
        );
    }

    #[test]
    fn guard_classify_error_sees_io_error_prefix() {
        // RepoError::Io must display with "io error: " prefix.
        let io_err = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "permission denied");
        let repo_err = RepoError::Io(io_err);
        let displayed = repo_err.to_string();
        assert!(
            displayed.starts_with("io error: "),
            "RepoError::Io must display with 'io error: ' prefix, got: {displayed:?}"
        );
        // Verify "permission denied" is visible (agents string-match this)
        assert!(
            displayed.contains("permission denied") || displayed.contains("Permission denied"),
            "error text must include the cause"
        );
    }

    #[test]
    fn guard_lost_status_wire_string_is_lost() {
        assert_eq!(serde_json::to_string(&JobStatus::Lost).unwrap(), "\"lost\"");
        let decoded: JobStatus = serde_json::from_str("\"lost\"").unwrap();
        assert_eq!(decoded, JobStatus::Lost);
    }

    #[test]
    fn guard_map_job_err_eperm_is_not_not_found() {
        let eperm_str =
            "Error reading job record metadata /tmp/job-x.json: Permission denied (os error 13)"
                .to_string();
        let result = map_job_err("job-x", eperm_str);
        assert!(matches!(result, RepoError::Io(_)));

        let enoent_str = "Error reading job record metadata /tmp/job-y.json: No such file or directory (os error 2)".to_string();
        let result2 = map_job_err("job-y", enoent_str);
        assert!(matches!(result2, RepoError::NotFound(_)));
    }
}
