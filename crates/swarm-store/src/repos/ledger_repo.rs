//! `LedgerRepo` impls: `FileLedgerRepo` (append-only NDJSON) + `MemLedgerRepo`.
//!
//! Mirrors `telemetry_repo`. Both backends store raw snapshots and collapse to
//! current state via `swarm_core::fold_tasks` on read, so File and Mem
//! share identical fold semantics (exercised by the shared contract test).

#![allow(dead_code)]

pub use swarm_core::LedgerRepo;

use std::path::PathBuf;
use std::sync::Mutex;

use swarm_core::{fold_tasks, LedgerTask};

use crate::ledger_helpers::{read_tasks_in_dir, record_task_in_dir};
use crate::repos::RepoError;

fn str_err(s: String) -> RepoError {
    RepoError::Io(std::io::Error::other(s))
}

// ── FileLedgerRepo ─────────────────────────────────────────────────────────────

pub struct FileLedgerRepo {
    dir: PathBuf,
}

impl FileLedgerRepo {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    pub fn dir(&self) -> &PathBuf {
        &self.dir
    }
}

/// Constructs a `FileLedgerRepo` pointed at the default ledger directory:
///   `<swarm home>/ledger` (see `crate::store::swarm_home`)
pub fn default_file_ledger_repo() -> Option<FileLedgerRepo> {
    crate::store::swarm_home().map(|home| FileLedgerRepo::new(home.join("ledger")))
}

impl LedgerRepo for FileLedgerRepo {
    fn record_task(&self, task: LedgerTask) -> Result<(), RepoError> {
        record_task_in_dir(&self.dir, task).map_err(str_err)
    }

    fn tasks(&self) -> Result<Vec<LedgerTask>, RepoError> {
        read_tasks_in_dir(&self.dir)
            .map(fold_tasks)
            .map_err(str_err)
    }
}

// ── MemLedgerRepo ──────────────────────────────────────────────────────────────

pub struct MemLedgerRepo {
    snapshots: Mutex<Vec<LedgerTask>>,
}

impl MemLedgerRepo {
    pub fn new() -> Self {
        Self {
            snapshots: Mutex::new(Vec::new()),
        }
    }
}

impl Default for MemLedgerRepo {
    fn default() -> Self {
        Self::new()
    }
}

impl LedgerRepo for MemLedgerRepo {
    fn record_task(&self, task: LedgerTask) -> Result<(), RepoError> {
        self.snapshots.lock().unwrap().push(task);
        Ok(())
    }

    fn tasks(&self) -> Result<Vec<LedgerTask>, RepoError> {
        Ok(fold_tasks(self.snapshots.lock().unwrap().clone()))
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use swarm_core::LedgerStatus;

    struct TestDir(PathBuf);

    impl TestDir {
        fn new(tag: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "agent-swarm-ledger-{}-{}-{}",
                tag,
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis()
            ));
            std::fs::create_dir_all(&path).expect("TestDir: create_dir_all failed");
            Self(path)
        }

        fn path(&self) -> &PathBuf {
            &self.0
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn ledger_repo_contract<R: LedgerRepo>(repo: R) {
        assert!(repo.tasks().unwrap().is_empty());

        // Distinct ids accumulate in first-seen order.
        repo.record_task(LedgerTask::new("t-1", "first", 1))
            .unwrap();
        repo.record_task(LedgerTask::new("t-2", "second", 2))
            .unwrap();
        repo.record_task(LedgerTask::new("t-3", "third", 3))
            .unwrap();
        let tasks = repo.tasks().unwrap();
        assert_eq!(tasks.len(), 3);
        assert_eq!(tasks[0].id, "t-1");
        assert_eq!(tasks[2].intent, "third");
        assert!(tasks.iter().all(|t| t.status == LedgerStatus::Open));

        // Re-recording an id folds to the latest snapshot, order preserved.
        let mut claimed = LedgerTask::new("t-2", "second", 2);
        claimed.status = LedgerStatus::Claimed;
        claimed.owner_agent = Some("gemini".to_string());
        repo.record_task(claimed).unwrap();
        let tasks = repo.tasks().unwrap();
        assert_eq!(tasks.len(), 3, "fold keeps one row per id");
        assert_eq!(tasks[1].id, "t-2", "first-seen order preserved on update");
        assert_eq!(tasks[1].status, LedgerStatus::Claimed);
        assert_eq!(tasks[1].owner_agent.as_deref(), Some("gemini"));

        // verified_done with anchor leaves the active working set.
        let mut verified = LedgerTask::new("t-1", "first", 1);
        verified.status = LedgerStatus::VerifiedDone;
        verified.validation_anchor = Some("test:foo".to_string());
        verified.closed_at_ms = Some(10);
        repo.record_task(verified).unwrap();
        let tasks = repo.tasks().unwrap();
        let active = tasks.iter().filter(|t| t.status.is_active()).count();
        assert_eq!(active, 2, "verified_done task leaves the working set");
        let t1 = tasks.iter().find(|t| t.id == "t-1").unwrap();
        assert!(t1.is_verified());
    }

    #[test]
    fn ledger_repo_contract_mem() {
        ledger_repo_contract(MemLedgerRepo::new());
    }

    #[test]
    fn ledger_repo_contract_file() {
        let dir = TestDir::new("contract");
        ledger_repo_contract(FileLedgerRepo::new(dir.path().to_path_buf()));
    }
}
