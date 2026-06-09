//! `EventRepo` trait — dual-backend (file-JSONL + in-memory) event storage.
//!
//! Moved from agent-swarm's `repos/event_repo.rs` in P5-S2.
//!
//! Inlines two pure functions previously sourced from agent-swarm:
//!   - `preview_for_event` (from synthesis.rs — pure string function, no deps)
//!   - `compact_event_payload` (from session.rs — depends on preview_for_event only)
//!
//! # Cursor contract
//!
//! `Cursor` (from `repos::Cursor`) is an opaque resumption token:
//! - **JSONL backend (`FileEventRepo`)**: byte offset of the last confirmed
//!   complete `\n`.
//! - **In-memory backend (`MemEventRepo`)**: Vec index of the last returned event.
//!
//! `Cursor::start()` == `Cursor(0)` means "no events seen — return from the beginning."

pub use swarm_core::{EventContext, EventRepo, LayerReportSpec, StoredEvent};

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read as IoRead, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Mutex;

use swarm_contracts::events::{EventKind, SessionEventV2};
use swarm_contracts::ids::SessionId;
use swarm_contracts::package::LayerReportEnvelope;

use crate::repos::{Cursor, RepoError};
use crate::store::{now_ms, EVENT_LOG_LOCK, EVENT_SEQ_COUNTER, MAX_EVENT_LINE_BYTES};

// ── Inlined pure helpers ──────────────────────────────────────────────────────

/// Compact and truncate a string for event log previews.
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

/// Compact an oversized event payload to fit within `MAX_EVENT_LINE_BYTES`.
/// Verbatim copy from agent-swarm's `session::compact_event_payload`.
fn compact_event_payload(payload: &serde_json::Value, original_bytes: usize) -> serde_json::Value {
    let mut compact = serde_json::Map::new();
    compact.insert("truncated".to_string(), serde_json::Value::Bool(true));
    compact.insert(
        "original_event_bytes".to_string(),
        serde_json::json!(original_bytes),
    );
    match payload {
        serde_json::Value::Object(map) => {
            for (key, value) in map {
                let compact_value = match value {
                    serde_json::Value::String(text) => {
                        serde_json::Value::String(preview_for_event(text, 1800))
                    }
                    serde_json::Value::Number(_)
                    | serde_json::Value::Bool(_)
                    | serde_json::Value::Null => value.clone(),
                    other => serde_json::Value::String(preview_for_event(
                        &serde_json::to_string(other).unwrap_or_default(),
                        1800,
                    )),
                };
                compact.insert(key.clone(), compact_value);
            }
        }
        other => {
            compact.insert(
                "preview".to_string(),
                serde_json::Value::String(preview_for_event(
                    &serde_json::to_string(other).unwrap_or_default(),
                    1800,
                )),
            );
        }
    }
    serde_json::Value::Object(compact)
}

// ── FileEventRepo ─────────────────────────────────────────────────────────────

/// File-backed `EventRepo` implementation.
///
/// Wraps the session store directory. All append operations acquire
/// `EVENT_LOG_LOCK` (the same global used by session.rs) for backward
/// compatibility with concurrent callers.
pub struct FileEventRepo {
    base_dir: PathBuf,
}

impl FileEventRepo {
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: base_dir.into(),
        }
    }

    fn events_path(&self, session: &SessionId) -> PathBuf {
        self.base_dir.join(session.as_str()).join("events.jsonl")
    }

    fn layer_reports_path(&self, session: &SessionId) -> PathBuf {
        self.base_dir
            .join(session.as_str())
            .join("layer-reports.jsonl")
    }

    fn session_dir(&self, session: &SessionId) -> PathBuf {
        self.base_dir.join(session.as_str())
    }

    /// Append one serialized event line assuming the caller already holds `EVENT_LOG_LOCK`.
    fn append_event_locked(
        &self,
        events_path: &Path,
        kind: EventKind,
        payload: serde_json::Value,
        ctx: &EventContext,
        session: &SessionId,
    ) -> Result<Cursor, RepoError> {
        let ts_ms = now_ms();
        let envelope = SessionEventV2 {
            schema: "agent-swarm/event/v2".to_owned(),
            ts_ms,
            session_id: session.as_str().to_owned(),
            run_id: session.as_str().to_owned(),
            parent_id: ctx.parent_id.clone(),
            agent_id: ctx.agent_id.clone(),
            role: ctx.role.clone(),
            phase: ctx.phase.clone(),
            kind,
            seq: EVENT_SEQ_COUNTER.fetch_add(1, Ordering::SeqCst),
            payload,
        };
        let mut event_val = serde_json::to_value(&envelope).map_err(RepoError::Serialize)?;
        let mut line = serde_json::to_string(&event_val).map_err(RepoError::Serialize)?;

        if line.len() > MAX_EVENT_LINE_BYTES {
            if let Some(obj) = event_val.as_object_mut() {
                let original = obj
                    .get("payload")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                obj.insert(
                    "payload".to_string(),
                    compact_event_payload(&original, line.len()),
                );
            }
            line = serde_json::to_string(&event_val).map_err(RepoError::Serialize)?;
        }

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(events_path)
            .map_err(RepoError::Io)?;
        writeln!(file, "{line}").map_err(RepoError::Io)?;
        let cursor_pos = file.metadata().map_err(RepoError::Io)?.len();
        Ok(Cursor::new(cursor_pos))
    }
}

impl EventRepo for FileEventRepo {
    fn append(
        &self,
        session: &SessionId,
        kind: EventKind,
        payload: serde_json::Value,
        ctx: EventContext,
    ) -> Result<Cursor, RepoError> {
        let events_path = self.events_path(session);
        if let Some(parent) = events_path.parent() {
            fs::create_dir_all(parent).map_err(RepoError::Io)?;
        }
        let _guard = EVENT_LOG_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        self.append_event_locked(&events_path, kind, payload, &ctx, session)
    }

    fn append_layer_report(
        &self,
        session: &SessionId,
        spec: LayerReportSpec,
    ) -> Result<Cursor, RepoError> {
        let ts_ms = now_ms();
        let preview = preview_for_event(&spec.text, 420);
        let session_dir = self.session_dir(session);
        let reports_dir = session_dir.join("layer-reports");
        fs::create_dir_all(&reports_dir).map_err(RepoError::Io)?;

        let report_file = format!(
            "{}-{}-{}.md",
            ts_ms,
            safe_slug(&spec.layer),
            safe_slug(&spec.role)
        );
        let report_path = reports_dir.join(&report_file);
        let report_text = format!(
            "# {} - {}\n\n- Agent: {}\n- Status: {}\n- Parent: {}\n\n{}\n",
            spec.layer,
            spec.role,
            spec.agent,
            spec.status,
            spec.parent_role.as_deref().unwrap_or("none"),
            if spec.text.trim().is_empty() {
                "(no output)"
            } else {
                spec.text.trim()
            }
        );
        crate::store::write_text_atomic(&report_path, report_text)
            .map_err(|e| RepoError::Io(std::io::Error::other(e)))?;

        let lr = LayerReportEnvelope {
            schema: "agent-swarm/layer-report/v1".to_owned(),
            ts_ms,
            session_id: session.as_str().to_owned(),
            layer: spec.layer.clone(),
            role: spec.role.clone(),
            agent: spec.agent.clone(),
            parent_role: spec.parent_role.clone(),
            status: spec.status.clone(),
            preview: preview.clone(),
            file: format!("layer-reports/{report_file}"),
            text_bytes: spec.text.len(),
        };
        let lr_encoded = serde_json::to_string(&lr).map_err(RepoError::Serialize)?;

        let layer_reports_path = self.layer_reports_path(session);
        if let Some(parent) = layer_reports_path.parent() {
            fs::create_dir_all(parent).map_err(RepoError::Io)?;
        }

        let _guard = EVENT_LOG_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|p| p.into_inner());

        let mut lr_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&layer_reports_path)
            .map_err(RepoError::Io)?;
        writeln!(lr_file, "{lr_encoded}").map_err(RepoError::Io)?;
        drop(lr_file);

        let events_path = self.events_path(session);
        if let Some(parent) = events_path.parent() {
            fs::create_dir_all(parent).map_err(RepoError::Io)?;
        }
        let ctx = EventContext {
            parent_id: spec.parent_role.clone(),
            agent_id: spec.agent.clone(),
            role: spec.role.clone(),
            phase: spec.layer.clone(),
        };
        let layer_reports_path_str = layer_reports_path.display().to_string();
        let payload = serde_json::json!({
            "layer": spec.layer,
            "role": spec.role,
            "agent": spec.agent,
            "parent_role": spec.parent_role,
            "status": spec.status,
            "text": preview,
            "file": format!("layer-reports/{report_file}"),
            "path": layer_reports_path_str,
        });
        self.append_event_locked(&events_path, EventKind::LayerReport, payload, &ctx, session)
    }

    fn events_since(
        &self,
        session: &SessionId,
        after: Cursor,
        limit: usize,
    ) -> Result<(Vec<StoredEvent>, Cursor), RepoError> {
        let events_path = self.events_path(session);

        if !events_path.exists() {
            return Ok((Vec::new(), Cursor::start()));
        }

        let mut file = File::open(&events_path).map_err(RepoError::Io)?;
        let file_len = file.metadata().map_err(RepoError::Io)?.len();

        let start = after.get();
        if start >= file_len {
            return Ok((Vec::new(), after));
        }

        file.seek(SeekFrom::Start(start)).map_err(RepoError::Io)?;
        let mut buf = Vec::new();
        file.read_to_end(&mut buf).map_err(RepoError::Io)?;

        let text = String::from_utf8_lossy(&buf);
        let ends_with_newline = text.ends_with('\n');
        let lines_iter = text.lines();
        let lines: Vec<&str> = if ends_with_newline {
            lines_iter.collect()
        } else {
            let all: Vec<&str> = lines_iter.collect();
            if all.is_empty() {
                Vec::new()
            } else {
                all[..all.len() - 1].to_vec()
            }
        };

        let mut events = Vec::new();
        let mut last_line_end_offset = start;

        for line in lines {
            if line.trim().is_empty() {
                last_line_end_offset += line.len() as u64 + 1;
                continue;
            }
            if let Ok(event) = serde_json::from_str::<StoredEvent>(line) {
                events.push(event);
            }
            last_line_end_offset += line.len() as u64 + 1;
            if events.len() >= limit {
                break;
            }
        }

        let new_cursor = Cursor::new(last_line_end_offset);
        Ok((events, new_cursor))
    }

    fn tail(&self, session: &SessionId, limit: usize) -> Result<Vec<StoredEvent>, RepoError> {
        let events_path = self.events_path(session);
        if !events_path.exists() {
            return Ok(Vec::new());
        }
        let text =
            crate::store::read_text_tail(&events_path, crate::store::MAX_SESSION_EVENTS_TAIL_BYTES)
                .map_err(|e| RepoError::Io(std::io::Error::other(e)))?;

        let mut events: Vec<StoredEvent> = text
            .lines()
            .rev()
            .take(limit)
            .filter_map(|line| serde_json::from_str::<StoredEvent>(line).ok())
            .collect();
        events.reverse();
        Ok(events)
    }

    fn latest_kind(&self, session: &SessionId) -> Result<Option<EventKind>, RepoError> {
        let events_path = self.events_path(session);
        if !events_path.exists() {
            return Ok(None);
        }
        let text =
            crate::store::read_text_tail(&events_path, crate::store::MAX_SESSION_EVENTS_TAIL_BYTES)
                .map_err(|e| RepoError::Io(std::io::Error::other(e)))?;
        let kind = text.lines().rev().find_map(|line| {
            let v = serde_json::from_str::<serde_json::Value>(line).ok()?;
            let kind_str = v.get("kind")?.as_str()?;
            Some(
                serde_json::from_value::<EventKind>(serde_json::Value::String(
                    kind_str.to_string(),
                ))
                .unwrap_or(EventKind::Other(kind_str.to_string())),
            )
        });
        Ok(kind)
    }
}

// ── MemEventRepo ──────────────────────────────────────────────────────────────

pub struct MemEventRepo {
    sessions: Mutex<HashMap<String, Vec<StoredEvent>>>,
}

impl Default for MemEventRepo {
    fn default() -> Self {
        Self::new()
    }
}

impl MemEventRepo {
    pub fn new() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
        }
    }
}

impl EventRepo for MemEventRepo {
    fn append(
        &self,
        session: &SessionId,
        kind: EventKind,
        payload: serde_json::Value,
        ctx: EventContext,
    ) -> Result<Cursor, RepoError> {
        let mut guard = self.sessions.lock().unwrap_or_else(|p| p.into_inner());
        let events = guard.entry(session.as_str().to_string()).or_default();
        events.push(StoredEvent {
            session_id: session.as_str().to_string(),
            kind,
            payload,
            ts_ms: now_ms(),
            seq: EVENT_SEQ_COUNTER.fetch_add(1, Ordering::SeqCst),
            parent_id: ctx.parent_id,
            agent_id: ctx.agent_id,
            role: ctx.role,
            phase: ctx.phase,
        });
        Ok(Cursor::new(events.len() as u64))
    }

    fn append_layer_report(
        &self,
        session: &SessionId,
        spec: LayerReportSpec,
    ) -> Result<Cursor, RepoError> {
        let preview = preview_for_event(&spec.text, 420);
        let ctx = EventContext {
            parent_id: spec.parent_role.clone(),
            agent_id: spec.agent.clone(),
            role: spec.role.clone(),
            phase: spec.layer.clone(),
        };
        let payload = serde_json::json!({
            "layer": spec.layer,
            "role": spec.role,
            "agent": spec.agent,
            "parent_role": spec.parent_role,
            "status": spec.status,
            "text": preview,
        });
        self.append(session, EventKind::LayerReport, payload, ctx)
    }

    fn events_since(
        &self,
        session: &SessionId,
        after: Cursor,
        limit: usize,
    ) -> Result<(Vec<StoredEvent>, Cursor), RepoError> {
        let guard = self.sessions.lock().unwrap_or_else(|p| p.into_inner());
        let events = match guard.get(session.as_str()) {
            Some(v) => v,
            None => return Ok((Vec::new(), Cursor::start())),
        };
        let start = after.get() as usize;
        if start >= events.len() {
            return Ok((Vec::new(), Cursor::new(events.len() as u64)));
        }
        let slice: Vec<StoredEvent> = events[start..].iter().take(limit).cloned().collect();
        let new_cursor = Cursor::new((start + slice.len()) as u64);
        Ok((slice, new_cursor))
    }

    fn tail(&self, session: &SessionId, limit: usize) -> Result<Vec<StoredEvent>, RepoError> {
        let guard = self.sessions.lock().unwrap_or_else(|p| p.into_inner());
        let events = match guard.get(session.as_str()) {
            Some(v) => v,
            None => return Ok(Vec::new()),
        };
        let skip = events.len().saturating_sub(limit);
        Ok(events[skip..].to_vec())
    }

    fn latest_kind(&self, session: &SessionId) -> Result<Option<EventKind>, RepoError> {
        let guard = self.sessions.lock().unwrap_or_else(|p| p.into_inner());
        let kind = guard
            .get(session.as_str())
            .and_then(|v| v.last())
            .map(|e| e.kind.clone());
        Ok(kind)
    }
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn safe_slug(value: &str) -> String {
    let slug: String = value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string();
    if slug.is_empty() {
        "report".to_string()
    } else {
        slug
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::ATOMIC_WRITE_COUNTER;
    use std::io::Write as IoWrite;

    struct TestDir(PathBuf);
    impl TestDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "event-repo-test-{}-{}",
                std::process::id(),
                ATOMIC_WRITE_COUNTER.fetch_add(1, Ordering::SeqCst)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn test_session() -> SessionId {
        SessionId::from(format!(
            "session-test-{}",
            ATOMIC_WRITE_COUNTER.fetch_add(1, Ordering::SeqCst)
        ))
    }

    fn default_ctx() -> EventContext {
        EventContext::default()
    }

    fn event_repo_contract<R: EventRepo>(repo: &R, session: &SessionId) {
        let (events, cursor) = repo
            .events_since(session, Cursor::start(), 100)
            .expect("events_since on empty session must not error");
        assert!(events.is_empty());
        let _ = cursor;

        for i in 0u32..5 {
            repo.append(
                session,
                EventKind::TurnChunk,
                serde_json::json!({ "i": i }),
                default_ctx(),
            )
            .unwrap_or_else(|e| panic!("append {i} failed: {e}"));
        }
        let (events, cursor_after_5) = repo
            .events_since(session, Cursor::start(), 100)
            .expect("events_since after 5 appends must not error");
        assert_eq!(events.len(), 5);
        for (idx, event) in events.iter().enumerate() {
            let i_val = event
                .payload
                .get("i")
                .and_then(|v| v.as_u64())
                .expect("payload must have 'i' field");
            assert_eq!(i_val as usize, idx);
        }

        repo.append(
            session,
            EventKind::TurnCompleted,
            serde_json::json!({ "done": true }),
            default_ctx(),
        )
        .unwrap();
        let (new_events, _) = repo
            .events_since(session, cursor_after_5, 100)
            .expect("events_since with advanced cursor must not error");
        assert_eq!(new_events.len(), 1);
        assert_eq!(new_events[0].kind, EventKind::TurnCompleted);

        let spec = LayerReportSpec {
            layer: "worker".to_string(),
            role: "architecture".to_string(),
            agent: "gemini".to_string(),
            parent_role: Some("manager".to_string()),
            status: "completed".to_string(),
            text: "Design looks solid.".to_string(),
        };
        let _ = repo
            .append_layer_report(session, spec)
            .expect("append_layer_report must not deadlock or error");

        let (lr_events, _) = repo
            .events_since(session, Cursor::start(), 100)
            .expect("events_since after layer_report must not error");
        assert!(lr_events.len() > 6);
        let last_kind = &lr_events.last().unwrap().kind;
        assert_eq!(*last_kind, EventKind::LayerReport);
    }

    #[test]
    fn event_repo_contract_mem() {
        let repo = MemEventRepo::new();
        let session = test_session();
        event_repo_contract(&repo, &session);
    }

    #[test]
    fn event_repo_contract_file() {
        let dir = TestDir::new();
        let repo = FileEventRepo::new(dir.path());
        let session = test_session();
        fs::create_dir_all(dir.path().join(session.as_str())).unwrap();
        event_repo_contract(&repo, &session);
    }

    #[test]
    fn file_event_repo_skips_torn_trailing_line() {
        let dir = TestDir::new();
        let repo = FileEventRepo::new(dir.path());
        let session = test_session();
        fs::create_dir_all(dir.path().join(session.as_str())).unwrap();

        repo.append(
            &session,
            EventKind::Created,
            serde_json::json!({ "cwd": "/tmp", "mode": "test" }),
            default_ctx(),
        )
        .unwrap();

        let events_path = dir.path().join(session.as_str()).join("events.jsonl");
        {
            let mut f = OpenOptions::new().append(true).open(&events_path).unwrap();
            f.write_all(b"{\"kind\":\"torn_line_no_newline\"").unwrap();
        }

        let (events, _cursor) = repo
            .events_since(&session, Cursor::start(), 100)
            .expect("events_since must not error on torn line");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, EventKind::Created);
    }

    #[test]
    fn mem_tail_returns_last_n() {
        let repo = MemEventRepo::new();
        let session = test_session();
        for i in 0u32..10 {
            repo.append(
                &session,
                EventKind::TurnChunk,
                serde_json::json!({ "i": i }),
                default_ctx(),
            )
            .unwrap();
        }
        let tail = repo.tail(&session, 3).unwrap();
        assert_eq!(tail.len(), 3);
        for (pos, event) in tail.iter().enumerate() {
            let i_val = event.payload["i"].as_u64().unwrap();
            assert_eq!(i_val as usize, 7 + pos);
        }
    }

    #[test]
    fn mem_latest_kind_empty_returns_none() {
        let repo = MemEventRepo::new();
        let session = test_session();
        assert!(repo.latest_kind(&session).unwrap().is_none());
    }

    #[test]
    fn mem_latest_kind_returns_last_appended() {
        let repo = MemEventRepo::new();
        let session = test_session();
        repo.append(
            &session,
            EventKind::Created,
            serde_json::json!({}),
            default_ctx(),
        )
        .unwrap();
        repo.append(
            &session,
            EventKind::SessionCompleted,
            serde_json::json!({}),
            default_ctx(),
        )
        .unwrap();
        assert_eq!(
            repo.latest_kind(&session).unwrap(),
            Some(EventKind::SessionCompleted)
        );
    }

    #[test]
    fn file_latest_kind_empty_returns_none() {
        let dir = TestDir::new();
        let repo = FileEventRepo::new(dir.path());
        let session = test_session();
        assert!(repo.latest_kind(&session).unwrap().is_none());
    }

    #[test]
    fn file_events_since_empty_file_returns_start_cursor() {
        let dir = TestDir::new();
        let repo = FileEventRepo::new(dir.path());
        let session = test_session();
        fs::create_dir_all(dir.path().join(session.as_str())).unwrap();
        let (events, cursor) = repo.events_since(&session, Cursor::start(), 10).unwrap();
        assert!(events.is_empty());
        assert_eq!(cursor, Cursor::start());
    }

    #[test]
    fn file_tail_returns_most_recent() {
        let dir = TestDir::new();
        let repo = FileEventRepo::new(dir.path());
        let session = test_session();
        fs::create_dir_all(dir.path().join(session.as_str())).unwrap();
        for i in 0u32..8 {
            repo.append(
                &session,
                EventKind::TurnChunk,
                serde_json::json!({ "i": i }),
                default_ctx(),
            )
            .unwrap();
        }
        let tail = repo.tail(&session, 3).unwrap();
        assert_eq!(tail.len(), 3);
        for (pos, event) in tail.iter().enumerate() {
            let i_val = event.payload["i"].as_u64().unwrap();
            assert_eq!(i_val as usize, 5 + pos);
        }
    }

    #[test]
    fn safe_slug_produces_filesystem_safe_output() {
        assert_eq!(safe_slug("worker"), "worker");
        assert_eq!(safe_slug("api-docs"), "api-docs");
        assert_eq!(safe_slug("some role"), "some-role");
        assert_eq!(safe_slug(""), "report");
        assert_eq!(safe_slug("---"), "report");
    }

    #[test]
    fn concurrent_append_produces_valid_jsonl() {
        use std::sync::atomic::{AtomicBool, Ordering as AO};
        use std::sync::Arc;

        const N_THREADS: usize = 8;
        const N_ITERS: usize = 200;
        const TIMEOUT_SECS: u64 = 30;

        let dir = TestDir::new();
        let session = test_session();
        fs::create_dir_all(dir.path().join(session.as_str())).unwrap();

        let done_flag = Arc::new(AtomicBool::new(false));
        {
            let done_flag = Arc::clone(&done_flag);
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(TIMEOUT_SECS));
                if !done_flag.load(AO::Relaxed) {
                    eprintln!(
                        "concurrent_append_produces_valid_jsonl: TIMEOUT after {}s",
                        TIMEOUT_SECS
                    );
                    std::process::exit(1);
                }
            });
        }

        let dir_path = Arc::new(dir.path().to_owned());
        let session_arc = Arc::new(session.clone());

        let handles: Vec<_> = (0..N_THREADS)
            .map(|t| {
                let dir_path = Arc::clone(&dir_path);
                let session_arc = Arc::clone(&session_arc);
                std::thread::spawn(move || {
                    let repo = FileEventRepo::new(dir_path.as_ref());
                    for i in 0..N_ITERS {
                        if i % 5 == 0 {
                            let spec = LayerReportSpec {
                                layer: format!("layer-{t}"),
                                role: format!("role-{t}-{i}"),
                                agent: "test-agent".to_string(),
                                parent_role: Some("manager".to_string()),
                                status: "completed".to_string(),
                                text: format!("Worker {t} iteration {i} — {}", "x".repeat(200)),
                            };
                            repo.append_layer_report(session_arc.as_ref(), spec)
                                .expect("append_layer_report must not fail");
                        } else {
                            repo.append(
                                session_arc.as_ref(),
                                EventKind::TurnChunk,
                                serde_json::json!({ "t": t, "i": i }),
                                default_ctx(),
                            )
                            .expect("append must not fail");
                        }
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().expect("thread panicked");
        }

        done_flag.store(true, AO::Relaxed);

        let events_path = dir.path().join(session.as_str()).join("events.jsonl");
        let events_text = fs::read_to_string(&events_path).expect("events.jsonl must exist");
        let mut event_count = 0usize;
        for (lineno, line) in events_text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            serde_json::from_str::<serde_json::Value>(line).unwrap_or_else(|e| {
                panic!(
                    "events.jsonl line {} not valid JSON: {e}\nline: {line}",
                    lineno + 1
                )
            });
            event_count += 1;
        }
        assert!(event_count > 0);

        let lr_path = dir
            .path()
            .join(session.as_str())
            .join("layer-reports.jsonl");
        if lr_path.exists() {
            let lr_text =
                fs::read_to_string(&lr_path).expect("layer-reports.jsonl must be readable");
            let mut lr_count = 0usize;
            for (lineno, line) in lr_text.lines().enumerate() {
                if line.trim().is_empty() {
                    continue;
                }
                serde_json::from_str::<serde_json::Value>(line).unwrap_or_else(|e| {
                    panic!(
                        "layer-reports.jsonl line {} not valid JSON: {e}\nline: {line}",
                        lineno + 1
                    )
                });
                lr_count += 1;
            }
            assert!(lr_count > 0);
        }
    }
}
