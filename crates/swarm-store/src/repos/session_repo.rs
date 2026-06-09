//! `SessionRepo` trait — dual-backend (file-on-disk + in-memory) session storage.
//!
//! Moved from agent-swarm's `repos/session_repo.rs` in P5-S2.
//!
//! Inlines three pure helpers previously sourced from agent-swarm:
//!   - `prompt_preview`              (from format.rs — depends on job::JobRecord)
//!   - `session_summary_json_from_dir`  (from report.rs — depends on session.rs agent types)
//!   - `session_artifacts_json_from_dir` (from report.rs — pure file reading)
//!
//! `session.rs` STAYS in agent-swarm (it depends on AgentSpec/SwarmArgs/DiscussArgs).

pub use swarm_core::{
    SessionArtifact, SessionHandle, SessionIndexRecord, SessionMeta, SessionRepo, SessionSpec,
    SessionStatus, SessionStatusDeriver, SessionSummary,
};

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use swarm_contracts::ids::SessionId;

use crate::repos::event_repo::{EventContext, EventRepo, FileEventRepo, MemEventRepo};
use crate::repos::{OsProcessLiveness, RepoError};
use crate::store::{now_ms, ATOMIC_WRITE_COUNTER};

// ── Inlined pure helpers ──────────────────────────────────────────────────────

/// Truncate a prompt to a short preview string.
/// Verbatim copy from agent-swarm's `format::prompt_preview`.
fn prompt_preview(prompt: &str) -> String {
    let normalized = prompt.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() <= 72 {
        normalized
    } else {
        let prefix: String = normalized.chars().take(69).collect();
        format!("{prefix}...")
    }
}

/// Compact and truncate a string for session summary previews.
/// Verbatim copy from agent-swarm's `synthesis::preview_for_event`.
fn preview_for_event(value: &str, max: usize) -> String {
    let compact = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() <= max {
        compact
    } else {
        format!(
            "{}...",
            compact
                .chars()
                .take(max.saturating_sub(3))
                .collect::<String>()
        )
    }
}

/// Assemble a session summary JSON value from a session directory.
/// Derived from agent-swarm's `report::session_summary_json_from_dir`.
/// The runtime_processes section (sysinfo) is omitted — that stays in report.rs.
fn session_summary_json_from_dir(id: &str, dir: &Path) -> Result<serde_json::Value, String> {
    let metadata_path = dir.join("session.json");
    let metadata_text = fs::read_to_string(&metadata_path).map_err(|err| {
        format!(
            "Error reading session metadata {}: {err}",
            metadata_path.display()
        )
    })?;
    let metadata: serde_json::Value = serde_json::from_str(&metadata_text).map_err(|err| {
        format!(
            "Error parsing session metadata {}: {err}",
            metadata_path.display()
        )
    })?;
    let created_at_ms = metadata
        .get("created_at_ms")
        .and_then(|v| v.as_u64())
        .map(u128::from)
        .unwrap_or_default();
    let pid = metadata
        .get("pid")
        .and_then(|v| v.as_u64())
        .and_then(|v| u32::try_from(v).ok());
    let sid = SessionId::from(id);
    let base_dir = dir.parent().unwrap_or(dir);
    let event_repo = FileEventRepo::new(base_dir);
    let latest_kind = event_repo.latest_kind(&sid).unwrap_or(None);
    let rec = SessionIndexRecord {
        id: sid,
        created_at_ms,
        pid,
        prompt_preview: String::new(),
    };
    let status =
        SessionStatusDeriver::derive(&rec, latest_kind.as_ref(), &OsProcessLiveness, now_ms());
    let status = status.as_str().to_string();

    let events = read_session_events_value(dir, 12)?;
    let digest = fs::read_to_string(dir.join("digest.md")).unwrap_or_default();
    let summary_text = fs::read_to_string(dir.join("summary.md")).unwrap_or_default();
    let docs = fs::read_to_string(dir.join("api-docs.md")).unwrap_or_default();

    Ok(serde_json::json!({
        "schema": "agent-swarm/session-summary/v1",
        "session_id": id,
        "status": status,
        "created_at_ms": created_at_ms,
        "updated_at_ms": events.last().and_then(|e| e.get("ts_ms")).and_then(|v| v.as_u64()).unwrap_or(created_at_ms as u64),
        "prompt_preview": metadata.get("prompt").and_then(|v| v.as_str()).map(prompt_preview).unwrap_or_default(),
        "cwd": metadata.get("cwd").and_then(|v| v.as_str()).unwrap_or(""),
        "manager": metadata.get("manager").cloned().unwrap_or(serde_json::Value::Null),
        "participants": metadata.get("participants").cloned().unwrap_or_else(|| serde_json::json!([])),
        "digest": preview_for_event(&digest, 4000),
        "summary_preview": preview_for_event(&summary_text, 2200),
        "docs_preview": preview_for_event(&docs, 1200),
        "recent_events": events,
        "artifacts": session_artifacts_json_from_dir(id, dir)?["artifacts"].clone()
    }))
}

fn read_session_events_value(
    dir: &Path,
    limit_from_tail: usize,
) -> Result<Vec<serde_json::Value>, String> {
    let path = dir.join("events.jsonl");
    let text = crate::store::read_text_tail(&path, crate::store::MAX_SESSION_EVENTS_TAIL_BYTES)
        .unwrap_or_default();
    let mut events = text
        .lines()
        .rev()
        .take(limit_from_tail)
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .collect::<Vec<_>>();
    events.reverse();
    Ok(events)
}

/// Assemble the artifacts list for a session directory.
/// Verbatim copy from agent-swarm's `report::session_artifacts_json_from_dir`.
fn session_artifacts_json_from_dir(id: &str, dir: &Path) -> Result<serde_json::Value, String> {
    let mut artifacts = Vec::new();
    for (label, file, mime) in [
        ("metadata", "session.json", "application/json"),
        ("events", "events.jsonl", "application/x-ndjson"),
        ("transcript", "transcript.md", "text/markdown"),
        ("summary", "summary.md", "text/markdown"),
        ("digest", "digest.md", "text/markdown"),
        ("api-docs", "api-docs.md", "text/markdown"),
        (
            "layer-reports",
            "layer-reports.jsonl",
            "application/x-ndjson",
        ),
    ] {
        let path = dir.join(file);
        if let Ok(metadata) = fs::metadata(&path) {
            artifacts.push(serde_json::json!({
                "label": label,
                "path": path.display().to_string(),
                "mime": mime,
                "bytes": metadata.len()
            }));
        }
    }
    let reports_dir = dir.join("layer-reports");
    if let Ok(entries) = fs::read_dir(&reports_dir) {
        for entry in entries.flatten().take(80) {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) == Some("md") {
                if let Ok(metadata) = fs::metadata(&path) {
                    artifacts.push(serde_json::json!({
                        "label": "layer-report",
                        "path": path.display().to_string(),
                        "mime": "text/markdown",
                        "bytes": metadata.len()
                    }));
                }
            }
        }
    }
    Ok(serde_json::json!({
        "schema": "agent-swarm/session-artifacts/v1",
        "session_id": id,
        "artifacts": artifacts
    }))
}

// ── FileSessionRepo ───────────────────────────────────────────────────────────

pub struct FileSessionRepo {
    base_dir: PathBuf,
}

impl FileSessionRepo {
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: base_dir.into(),
        }
    }

    fn session_dir(&self, id: &SessionId) -> PathBuf {
        self.base_dir.join(id.as_str())
    }
}

impl SessionRepo for FileSessionRepo {
    type Events = FileEventRepo;

    fn create(&self, spec: SessionSpec) -> Result<SessionHandle<FileEventRepo>, RepoError> {
        use std::sync::atomic::Ordering;
        fs::create_dir_all(&self.base_dir).map_err(RepoError::Io)?;
        let counter = ATOMIC_WRITE_COUNTER.fetch_add(1, Ordering::SeqCst);
        let id = SessionId::from(format!(
            "session-{:x}-{}-{}",
            now_ms(),
            std::process::id(),
            counter
        ));
        let session_dir = self.base_dir.join(id.as_str());
        fs::create_dir_all(&session_dir).map_err(RepoError::Io)?;

        let events_repo = FileEventRepo::new(&self.base_dir);
        events_repo.append(
            &id,
            swarm_contracts::events::EventKind::Created,
            serde_json::json!({
                "cwd": spec.cwd.display().to_string(),
                "mode": spec.mode
            }),
            EventContext::default(),
        )?;

        Ok(SessionHandle {
            id,
            events: events_repo,
        })
    }

    fn open(&self, id: &SessionId) -> Result<SessionHandle<FileEventRepo>, RepoError> {
        let session_dir = self.session_dir(id);
        if !session_dir.exists() {
            return Err(RepoError::NotFound(id.as_str().to_string()));
        }
        Ok(SessionHandle {
            id: id.clone(),
            events: FileEventRepo::new(&self.base_dir),
        })
    }

    fn list(&self) -> Result<Vec<SessionIndexRecord>, RepoError> {
        let read_dir = match fs::read_dir(&self.base_dir) {
            Ok(d) => d,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Vec::new());
            }
            Err(e) => return Err(RepoError::Io(e)),
        };

        let mut records = Vec::new();
        for entry in read_dir.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let Some(id_str) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let metadata_path = path.join("session.json");
            let Ok(content) = fs::read_to_string(&metadata_path) else {
                continue;
            };
            let Ok(metadata) = serde_json::from_str::<serde_json::Value>(&content) else {
                continue;
            };
            let created_at_ms = metadata
                .get("created_at_ms")
                .and_then(|v| v.as_u64())
                .map(u128::from)
                .unwrap_or_default();
            let pid = metadata
                .get("pid")
                .and_then(|v| v.as_u64())
                .and_then(|v| u32::try_from(v).ok());
            let prompt = metadata
                .get("prompt")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            records.push(SessionIndexRecord {
                id: SessionId::from(id_str),
                created_at_ms,
                pid,
                prompt_preview: prompt_preview(prompt),
            });
        }
        Ok(records)
    }

    fn write_metadata(&self, id: &SessionId, meta: &SessionMeta) -> Result<(), RepoError> {
        let session_dir = self.session_dir(id);
        fs::create_dir_all(&session_dir).map_err(RepoError::Io)?;
        let value = serde_json::json!({
            "id": meta.id,
            "created_at_ms": meta.created_at_ms,
            "pid": meta.pid,
            "prompt": meta.prompt,
            "cwd": meta.cwd.display().to_string(),
            "mode": meta.mode,
        });
        let text = serde_json::to_string_pretty(&value).map_err(RepoError::Serialize)?;
        let contents = format!("{text}\n");
        crate::store::write_text_atomic(&session_dir.join("session.json"), contents)
            .map_err(|e| RepoError::Io(std::io::Error::other(e)))
    }

    fn summary(&self, id: &SessionId) -> Result<SessionSummary, RepoError> {
        let session_dir = self.session_dir(id);
        let value = session_summary_json_from_dir(id.as_str(), &session_dir)
            .map_err(|e| RepoError::Io(std::io::Error::other(e)))?;
        Ok(SessionSummary { value })
    }

    fn artifacts(&self, id: &SessionId) -> Result<Vec<SessionArtifact>, RepoError> {
        let session_dir = self.session_dir(id);
        let raw = session_artifacts_json_from_dir(id.as_str(), &session_dir)
            .map_err(|e| RepoError::Io(std::io::Error::other(e)))?;
        let artifacts = raw["artifacts"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| {
                        Some(SessionArtifact {
                            label: v["label"].as_str()?.to_string(),
                            path: PathBuf::from(v["path"].as_str()?),
                            mime: v["mime"].as_str()?.to_string(),
                            bytes: v["bytes"].as_u64()?,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(artifacts)
    }
}

// ── MemSessionRepo ────────────────────────────────────────────────────────────

struct MemSessionEntry {
    id: SessionId,
    created_at_ms: u128,
    pid: Option<u32>,
    prompt_preview: String,
    metadata: Option<serde_json::Value>,
}

pub struct MemSessionRepo {
    sessions: Mutex<HashMap<String, MemSessionEntry>>,
}

impl Default for MemSessionRepo {
    fn default() -> Self {
        Self::new()
    }
}

impl MemSessionRepo {
    pub fn new() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
        }
    }
}

impl SessionRepo for MemSessionRepo {
    type Events = MemEventRepo;

    fn create(&self, spec: SessionSpec) -> Result<SessionHandle<MemEventRepo>, RepoError> {
        use std::sync::atomic::Ordering;
        let counter = ATOMIC_WRITE_COUNTER.fetch_add(1, Ordering::SeqCst);
        let id = SessionId::from(format!(
            "session-{:x}-{}-{}",
            now_ms(),
            std::process::id(),
            counter
        ));
        let ts_ms = now_ms();
        let pp = prompt_preview(&spec.prompt);
        let events = MemEventRepo::new();

        events.append(
            &id,
            swarm_contracts::events::EventKind::Created,
            serde_json::json!({
                "cwd": spec.cwd.display().to_string(),
                "mode": spec.mode
            }),
            EventContext::default(),
        )?;

        let mut guard = self.sessions.lock().unwrap_or_else(|p| p.into_inner());
        guard.insert(
            id.as_str().to_string(),
            MemSessionEntry {
                id: id.clone(),
                created_at_ms: ts_ms,
                pid: None,
                prompt_preview: pp,
                metadata: None,
            },
        );
        Ok(SessionHandle { id, events })
    }

    fn open(&self, id: &SessionId) -> Result<SessionHandle<MemEventRepo>, RepoError> {
        let guard = self.sessions.lock().unwrap_or_else(|p| p.into_inner());
        if !guard.contains_key(id.as_str()) {
            return Err(RepoError::NotFound(id.as_str().to_string()));
        }
        Ok(SessionHandle {
            id: id.clone(),
            events: MemEventRepo::new(),
        })
    }

    fn list(&self) -> Result<Vec<SessionIndexRecord>, RepoError> {
        let guard = self.sessions.lock().unwrap_or_else(|p| p.into_inner());
        let records = guard
            .values()
            .map(|entry| SessionIndexRecord {
                id: entry.id.clone(),
                created_at_ms: entry.created_at_ms,
                pid: entry.pid,
                prompt_preview: entry.prompt_preview.clone(),
            })
            .collect();
        Ok(records)
    }

    fn write_metadata(&self, id: &SessionId, meta: &SessionMeta) -> Result<(), RepoError> {
        let mut guard = self.sessions.lock().unwrap_or_else(|p| p.into_inner());
        let entry = guard
            .get_mut(id.as_str())
            .ok_or_else(|| RepoError::NotFound(id.as_str().to_string()))?;
        entry.pid = Some(meta.pid);
        entry.prompt_preview = prompt_preview(&meta.prompt);
        entry.metadata = Some(serde_json::json!({
            "id": meta.id,
            "created_at_ms": meta.created_at_ms,
            "pid": meta.pid,
            "prompt": meta.prompt,
            "cwd": meta.cwd.display().to_string(),
            "mode": meta.mode,
        }));
        Ok(())
    }

    fn summary(&self, id: &SessionId) -> Result<SessionSummary, RepoError> {
        let guard = self.sessions.lock().unwrap_or_else(|p| p.into_inner());
        let entry = guard
            .get(id.as_str())
            .ok_or_else(|| RepoError::NotFound(id.as_str().to_string()))?;
        let base_meta = entry.metadata.clone().unwrap_or_else(|| {
            serde_json::json!({
                "id": entry.id.as_str(),
                "created_at_ms": entry.created_at_ms,
            })
        });
        Ok(SessionSummary {
            value: serde_json::json!({
                "schema": "agent-swarm/session-summary/v1",
                "session_id": entry.id.as_str(),
                "created_at_ms": entry.created_at_ms,
                "prompt_preview": entry.prompt_preview,
                "metadata": base_meta,
                "recent_events": [],
            }),
        })
    }

    fn artifacts(&self, id: &SessionId) -> Result<Vec<SessionArtifact>, RepoError> {
        let guard = self.sessions.lock().unwrap_or_else(|p| p.into_inner());
        if !guard.contains_key(id.as_str()) {
            return Err(RepoError::NotFound(id.as_str().to_string()));
        }
        Ok(Vec::new())
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repos::event_repo::StoredEvent;
    use crate::repos::{AlwaysAlive, NeverAlive};
    use std::sync::atomic::Ordering;

    struct TestDir(PathBuf);
    impl TestDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "session-repo-test-{}-{}",
                std::process::id(),
                ATOMIC_WRITE_COUNTER.fetch_add(1, Ordering::SeqCst)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }
    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn test_spec() -> SessionSpec {
        SessionSpec {
            cwd: PathBuf::from("/tmp/test"),
            mode: "discussion".to_string(),
            prompt: "What is the answer?".to_string(),
        }
    }

    fn test_meta(id: &SessionId) -> SessionMeta {
        SessionMeta {
            id: id.as_str().to_string(),
            created_at_ms: now_ms(),
            pid: std::process::id(),
            prompt: "What is the answer?".to_string(),
            cwd: PathBuf::from("/tmp/test"),
            mode: "discussion".to_string(),
        }
    }

    fn session_repo_contract<R: SessionRepo>(repo: &R)
    where
        R::Events: EventRepo,
    {
        let handle = repo.create(test_spec()).expect("create must succeed");
        let id = handle.id.clone();
        assert!(!id.as_str().is_empty());

        let meta = test_meta(&id);
        repo.write_metadata(&id, &meta)
            .expect("write_metadata must succeed");

        let reopened = repo.open(&id).expect("open must succeed");
        assert_eq!(reopened.id, id);

        let bad_id = SessionId::from("session-does-not-exist");
        let open_result = repo.open(&bad_id);
        assert!(matches!(open_result, Err(RepoError::NotFound(_))));

        let records = repo.list().expect("list must succeed");
        let found = records.iter().find(|r| r.id == id);
        assert!(found.is_some());
        let rec = found.unwrap();
        assert!(rec.created_at_ms > 0);
        assert!(!rec.prompt_preview.is_empty());

        let summary = repo.summary(&id).expect("summary must succeed");
        assert_eq!(
            summary.value["schema"],
            serde_json::json!("agent-swarm/session-summary/v1"),
        );
        let sid = summary.value["session_id"].as_str().unwrap_or("");
        assert_eq!(sid, id.as_str());

        let artifacts = repo.artifacts(&id).expect("artifacts must succeed");
        let _count = artifacts.len();

        handle
            .events
            .append(
                &id,
                swarm_contracts::events::EventKind::TurnChunk,
                serde_json::json!({ "text": "hello" }),
                EventContext::default(),
            )
            .expect("append via handle must succeed");
    }

    #[test]
    fn session_repo_contract_mem() {
        let repo = MemSessionRepo::new();
        session_repo_contract(&repo);
    }

    #[test]
    fn session_repo_contract_file() {
        let dir = TestDir::new();
        let repo = FileSessionRepo::new(dir.path());
        session_repo_contract(&repo);
    }

    #[test]
    fn file_session_artifacts_lists_events_file() {
        let dir = TestDir::new();
        let repo = FileSessionRepo::new(dir.path());
        let handle = repo.create(test_spec()).unwrap();
        let id = handle.id.clone();
        repo.write_metadata(&id, &test_meta(&id)).unwrap();

        let artifacts = repo.artifacts(&id).unwrap();
        let has_events = artifacts.iter().any(|a| a.label == "events");
        assert!(has_events);
    }

    #[test]
    fn mem_session_repo_list_returns_all_sessions() {
        let repo = MemSessionRepo::new();
        let h1 = repo.create(test_spec()).unwrap();
        let h2 = repo.create(test_spec()).unwrap();
        repo.write_metadata(&h1.id, &test_meta(&h1.id)).unwrap();
        repo.write_metadata(&h2.id, &test_meta(&h2.id)).unwrap();

        let records = repo.list().unwrap();
        assert!(records.len() >= 2);
        let ids: Vec<_> = records.iter().map(|r| r.id.clone()).collect();
        assert!(ids.contains(&h1.id));
        assert!(ids.contains(&h2.id));
    }

    #[test]
    fn mem_write_metadata_updates_pid() {
        let repo = MemSessionRepo::new();
        let handle = repo.create(test_spec()).unwrap();
        let id = handle.id.clone();
        repo.write_metadata(&id, &test_meta(&id)).unwrap();

        let records = repo.list().unwrap();
        let rec = records.iter().find(|r| r.id == id).unwrap();
        assert_eq!(rec.pid, Some(std::process::id()));
    }

    fn make_rec(id: &str, pid: Option<u32>, created_at_ms: u128) -> SessionIndexRecord {
        SessionIndexRecord {
            id: SessionId::from(id),
            created_at_ms,
            pid,
            prompt_preview: String::new(),
        }
    }

    #[test]
    fn status_deriver_completed_event_overrides_pid() {
        let rec = make_rec("session-test-completed", Some(99_999_999), now_ms());
        let status = SessionStatusDeriver::derive(
            &rec,
            Some(&swarm_contracts::events::EventKind::SessionCompleted),
            &AlwaysAlive,
            now_ms(),
        );
        assert_eq!(status, SessionStatus::Completed);
    }

    #[test]
    fn status_deriver_pid_alive_returns_running() {
        let rec = make_rec("session-test-running", Some(99_999_999), now_ms());
        let status = SessionStatusDeriver::derive(
            &rec,
            Some(&swarm_contracts::events::EventKind::TurnChunk),
            &AlwaysAlive,
            now_ms(),
        );
        assert_eq!(status, SessionStatus::Running);
    }

    #[test]
    fn status_deriver_pid_dead_returns_lost() {
        let rec = make_rec("session-test-lost", Some(99_999_999), now_ms());
        let status = SessionStatusDeriver::derive(
            &rec,
            Some(&swarm_contracts::events::EventKind::TurnChunk),
            &NeverAlive,
            now_ms(),
        );
        assert_eq!(status, SessionStatus::Lost);
    }

    #[test]
    fn status_deriver_no_pid_old_session_returns_incomplete() {
        let old_ts = now_ms().saturating_sub(10 * 60 * 1_000);
        let rec = make_rec("session-test-incomplete", None, old_ts);
        let status = SessionStatusDeriver::derive(&rec, None, &NeverAlive, now_ms());
        assert_eq!(status, SessionStatus::Incomplete);
    }

    #[test]
    fn status_deriver_no_pid_recent_session_returns_running() {
        let recent_ts = now_ms().saturating_sub(30 * 1_000);
        let rec = make_rec("session-test-recent", None, recent_ts);
        let status = SessionStatusDeriver::derive(&rec, None, &NeverAlive, now_ms());
        assert_eq!(status, SessionStatus::Running);
    }

    #[test]
    fn session_status_wire_strings_are_stable() {
        assert_eq!(SessionStatus::Completed.as_str(), "completed");
        assert_eq!(SessionStatus::Running.as_str(), "running");
        assert_eq!(SessionStatus::Lost.as_str(), "lost");
        assert_eq!(SessionStatus::Incomplete.as_str(), "incomplete");
    }

    #[test]
    fn file_list_does_not_call_process_is_alive() {
        let dir = TestDir::new();
        let repo = FileSessionRepo::new(dir.path());
        let handle = repo.create(test_spec()).unwrap();
        let id = handle.id.clone();
        repo.write_metadata(&id, &test_meta(&id)).unwrap();
        let records = repo.list().unwrap();
        let rec = records.iter().find(|r| r.id == id).unwrap();
        assert!(rec.pid.is_some());
    }

    #[test]
    fn mem_summary_returns_schema_field_even_without_write_metadata() {
        let repo = MemSessionRepo::new();
        let handle = repo.create(test_spec()).unwrap();
        let id = handle.id.clone();
        let summary = repo.summary(&id).unwrap();
        assert_eq!(
            summary.value["schema"],
            serde_json::json!("agent-swarm/session-summary/v1"),
        );
    }

    #[test]
    fn stored_event_is_accessible_via_event_repo_tail() {
        let repo = MemEventRepo::new();
        let id = SessionId::from("session-stored-event-smoke");
        repo.append(
            &id,
            swarm_contracts::events::EventKind::TurnChunk,
            serde_json::json!({}),
            EventContext::default(),
        )
        .unwrap();
        let tail: Vec<StoredEvent> = repo.tail(&id, 1).unwrap();
        assert_eq!(tail.len(), 1);
    }
}
