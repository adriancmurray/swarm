use std::fs;
use std::path::{Path, PathBuf};

use swarm_core::{EventContext, EventRepo, LayerReportSpec, SessionRepo, SessionStatusDeriver};
use swarm_kernel::agent::{describe_spec, AgentSpec};
use swarm_kernel::args::{DiscussArgs, SwarmArgs};
use swarm_kernel::events::EventKind;
use swarm_kernel::ids::SessionId;
use swarm_kernel::profiles;
use swarm_store::repos::event_repo::FileEventRepo;
use swarm_store::repos::session_repo::FileSessionRepo;
use swarm_store::store::{new_session_id, now_ms, session_store_dir, write_text_atomic};
use swarm_store::OsProcessLiveness;

#[derive(Debug, Clone)]
pub struct DiscussionTurn {
    pub round: u32,
    pub role: String,
    pub spec: AgentSpec,
    pub code: i32,
    pub timed_out: bool,
    pub text: String,
    pub stderr: String,
}

#[derive(Debug, Clone, serde::Serialize)]
struct SessionRecord {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    parent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    slice: Option<String>,
}

#[derive(Clone)]
pub struct DiscussionSession {
    pub id: SessionId,
    pub dir: PathBuf,
    pub events_path: PathBuf,
    pub transcript_path: PathBuf,
    pub summary_path: PathBuf,
    pub digest_path: PathBuf,
    pub docs_path: PathBuf,
    pub layer_reports_path: PathBuf,
}

impl DiscussionSession {
    pub fn create_swarm(args: &SwarmArgs) -> Result<Self, String> {
        let base = session_store_dir()?;
        fs::create_dir_all(&base)
            .map_err(|err| format!("Error creating session directory {}: {err}", base.display()))?;
        let id = new_session_id();
        let dir = base.join(id.as_str());
        fs::create_dir_all(&dir)
            .map_err(|err| format!("Error creating session {}: {err}", dir.display()))?;
        let session = Self {
            id,
            events_path: dir.join("events.jsonl"),
            transcript_path: dir.join("transcript.md"),
            summary_path: dir.join("summary.md"),
            digest_path: dir.join("digest.md"),
            docs_path: dir.join("api-docs.md"),
            layer_reports_path: dir.join("layer-reports.jsonl"),
            dir,
        };
        session.append_event(
            EventKind::Created,
            serde_json::json!({
                "cwd": args.cwd.display().to_string(),
                "mode": "fanout"
            }),
        )?;
        Ok(session)
    }

    pub fn create(args: &DiscussArgs) -> Result<Self, String> {
        let base = session_store_dir()?;
        fs::create_dir_all(&base)
            .map_err(|err| format!("Error creating session directory {}: {err}", base.display()))?;
        let id = new_session_id();
        let dir = base.join(id.as_str());
        fs::create_dir_all(&dir)
            .map_err(|err| format!("Error creating session {}: {err}", dir.display()))?;
        let session = Self {
            id,
            events_path: dir.join("events.jsonl"),
            transcript_path: dir.join("transcript.md"),
            summary_path: dir.join("summary.md"),
            digest_path: dir.join("digest.md"),
            docs_path: dir.join("api-docs.md"),
            layer_reports_path: dir.join("layer-reports.jsonl"),
            dir,
        };
        session.append_event(
            EventKind::Created,
            serde_json::json!({
                "cwd": args.cwd.display().to_string()
            }),
        )?;
        Ok(session)
    }

    pub fn write_swarm_metadata(&self, args: &SwarmArgs) -> Result<(), String> {
        let mut metadata = serde_json::json!({
            "id": self.id,
            "created_at_ms": now_ms(),
            "pid": std::process::id(),
            "prompt": &args.prompt,
            "cwd": args.cwd.display().to_string(),
            "rounds": 1,
            "manager": describe_spec(&args.manager),
            "participants": args.workers.iter().map(|worker| {
                serde_json::json!({
                    "role": &worker.role,
                    "agent": describe_spec(&worker.spec)
                })
            }).collect::<Vec<_>>(),
            "docs": false,
            "docs_agent": "",
            "mode": "fanout",
            "events_path": self.events_path.display().to_string(),
            "transcript_path": self.transcript_path.display().to_string(),
            "summary_path": self.summary_path.display().to_string(),
            "digest_path": self.digest_path.display().to_string(),
            "docs_path": self.docs_path.display().to_string(),
            "layer_reports_path": self.layer_reports_path.display().to_string()
        })
        .as_object()
        .cloned()
        .unwrap_or_default();
        let record = SessionRecord {
            parent: args.parent.clone(),
            slice: args.slice.clone(),
        };
        let record_fields = serde_json::to_value(record)
            .map_err(|err| format!("Error serializing session record: {err}"))?
            .as_object()
            .cloned()
            .unwrap_or_default();
        metadata.extend(record_fields);

        let text = serde_json::to_string_pretty(&metadata)
            .map_err(|err| format!("Error serializing swarm metadata: {err}"))?;
        write_text_atomic(&self.dir.join("session.json"), format!("{text}\n"))
    }

    pub fn write_metadata(&self, args: &DiscussArgs) -> Result<(), String> {
        let mut metadata = serde_json::json!({
            "id": self.id,
            "created_at_ms": now_ms(),
            "pid": std::process::id(),
            "prompt": &args.prompt,
            "cwd": args.cwd.display().to_string(),
            "rounds": args.rounds,
            "manager": describe_spec(&args.manager),
            "participants": args.participants.iter().map(|worker| {
                serde_json::json!({
                    "role": &worker.role,
                    "agent": describe_spec(&worker.spec),
                    "profile": profiles::profile_id_for_role(&worker.role)
                })
            }).collect::<Vec<_>>(),
            "docs": args.docs.unwrap_or(false),
            "docs_agent": describe_spec(&args.docs_agent),
            "profile_helpers": args.profile_helpers,
            "events_path": self.events_path.display().to_string(),
            "transcript_path": self.transcript_path.display().to_string(),
            "summary_path": self.summary_path.display().to_string(),
            "digest_path": self.digest_path.display().to_string(),
            "docs_path": self.docs_path.display().to_string(),
            "layer_reports_path": self.layer_reports_path.display().to_string()
        })
        .as_object()
        .cloned()
        .unwrap_or_default();
        let record = SessionRecord {
            parent: args.parent.clone(),
            slice: args.slice.clone(),
        };
        let record_fields = serde_json::to_value(record)
            .map_err(|err| format!("Error serializing session record: {err}"))?
            .as_object()
            .cloned()
            .unwrap_or_default();
        metadata.extend(record_fields);

        let text = serde_json::to_string_pretty(&metadata)
            .map_err(|err| format!("Error serializing session metadata: {err}"))?;
        write_text_atomic(&self.dir.join("session.json"), format!("{text}\n"))
    }

    pub fn append_event(&self, kind: EventKind, payload: serde_json::Value) -> Result<(), String> {
        self.append_event_with_context(kind, payload, None, "auto", "participant", "discussion")
    }

    /// Writes an `agent-swarm/event/v2` envelope to the session event log.
    ///
    /// Delegates to `FileEventRepo` which acquires `EVENT_LOG_LOCK` internally.
    /// `parent_id` is intentionally present as JSON null when there is no
    /// parent, and `seq` comes from the shared process-global store counter.
    pub fn append_event_with_context(
        &self,
        kind: EventKind,
        payload: serde_json::Value,
        parent_id: Option<&str>,
        agent_id: &str,
        role: &str,
        phase: &str,
    ) -> Result<(), String> {
        let base = self.dir.parent().unwrap_or(&self.dir);
        let repo = FileEventRepo::new(base);
        let ctx = EventContext {
            parent_id: parent_id.map(str::to_owned),
            agent_id: agent_id.to_owned(),
            role: role.to_owned(),
            phase: phase.to_owned(),
        };
        repo.append(&self.id, kind, payload, ctx)
            .map(|_| ())
            .map_err(|e| format!("Error writing session event log: {e}"))
    }

    pub fn append_layer_report(
        &self,
        layer: &str,
        role: &str,
        agent: &str,
        parent_role: Option<&str>,
        status: &str,
        text: &str,
    ) -> Result<(), String> {
        let base = self.dir.parent().unwrap_or(&self.dir);
        let repo = FileEventRepo::new(base);
        let spec = LayerReportSpec {
            layer: layer.to_owned(),
            role: role.to_owned(),
            agent: agent.to_owned(),
            parent_role: parent_role.map(str::to_owned),
            status: status.to_owned(),
            text: text.to_owned(),
        };
        repo.append_layer_report(&self.id, spec)
            .map(|_| ())
            .map_err(|e| format!("Error writing layer report: {e}"))
    }
}

/// Lightweight in-memory record derived from scanning a session directory.
#[derive(Debug)]
pub struct SessionIndexRecord {
    pub id: String,
    pub created_at_ms: u128,
    pub status: String,
    pub prompt_preview: String,
}

pub fn list_sessions() -> Result<Vec<SessionIndexRecord>, String> {
    let base = session_store_dir()?;
    list_sessions_from_base(&base)
}

/// List sessions from `base`, deriving status via `SessionStatusDeriver`.
///
/// Replaces the old scan+`derive_session_index_status` loop (S8). Uses
/// `FileSessionRepo::list()` for the raw records (pid, created_at_ms,
/// prompt_preview), then per-record derives status by calling
/// `FileEventRepo::latest_kind()` + `SessionStatusDeriver::derive()`.
pub fn list_sessions_from_base(base: &Path) -> Result<Vec<SessionIndexRecord>, String> {
    let repo = FileSessionRepo::new(base);
    let event_repo = FileEventRepo::new(base);
    let liveness = OsProcessLiveness;
    let raw_records = repo.list().map_err(|e| e.to_string())?;
    let now = now_ms();
    let mut sessions = Vec::new();
    for rec in raw_records {
        let latest_kind = event_repo.latest_kind(&rec.id).unwrap_or(None);
        let status = SessionStatusDeriver::derive(&rec, latest_kind.as_ref(), &liveness, now);
        sessions.push(SessionIndexRecord {
            id: rec.id.as_str().to_string(),
            created_at_ms: rec.created_at_ms,
            status: status.as_str().to_string(),
            prompt_preview: rec.prompt_preview,
        });
    }
    Ok(sessions)
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use swarm_core::{AlwaysAlive, NeverAlive, RepoError, SessionMeta, SessionSpec, SessionStatus};
    use swarm_store::repos::session_repo::{
        FileSessionRepo, SessionIndexRecord as RepoIndexRecord, SessionStatusDeriver,
    };
    use swarm_store::store::{now_ms, validate_store_id};

    // Test-local nonce counter — ATOMIC_WRITE_COUNTER is now pub inside
    // swarm-store and NOT accessible here (encapsulation invariant P5-S2).
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    // ── RAII temp dir ──────────────────────────────────────────────────────────

    struct TestDir(std::path::PathBuf);
    impl TestDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "session-s8-test-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::SeqCst)
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn make_rec(pid: Option<u32>, created_at_ms: u128) -> RepoIndexRecord {
        RepoIndexRecord {
            id: swarm_kernel::ids::SessionId::from(format!("session-test-{}", now_ms())),
            created_at_ms,
            pid,
            prompt_preview: String::new(),
        }
    }

    // ── Status-matrix: full 5-rule coverage ──────────────────────────────────
    //
    // Rule 1: latest_kind == SessionCompleted → Completed (regardless of pid/liveness)
    // Rule 2: pid present + alive → Running
    // Rule 3: pid present + dead → Lost
    // Rule 4: no pid + age > 5 min → Incomplete
    // Rule 5: no pid + age <= 5 min → Running

    #[test]
    fn status_matrix_rule1_completed_event_overrides_alive_pid() {
        let rec = make_rec(Some(std::process::id()), now_ms());
        let status = SessionStatusDeriver::derive(
            &rec,
            Some(&EventKind::SessionCompleted),
            &AlwaysAlive,
            now_ms(),
        );
        assert_eq!(
            status,
            SessionStatus::Completed,
            "rule 1: completed wins over alive pid"
        );
    }

    #[test]
    fn status_matrix_rule1_completed_event_overrides_dead_pid() {
        let rec = make_rec(Some(99_999_999), now_ms());
        let status = SessionStatusDeriver::derive(
            &rec,
            Some(&EventKind::SessionCompleted),
            &NeverAlive,
            now_ms(),
        );
        assert_eq!(
            status,
            SessionStatus::Completed,
            "rule 1: completed wins over dead pid"
        );
    }

    #[test]
    fn status_matrix_rule1_completed_event_overrides_no_pid() {
        let old_ts = now_ms().saturating_sub(10 * 60 * 1_000);
        let rec = make_rec(None, old_ts);
        let status = SessionStatusDeriver::derive(
            &rec,
            Some(&EventKind::SessionCompleted),
            &NeverAlive,
            now_ms(),
        );
        assert_eq!(
            status,
            SessionStatus::Completed,
            "rule 1: completed wins over old no-pid"
        );
    }

    #[test]
    fn status_matrix_rule2_pid_alive_returns_running() {
        let rec = make_rec(Some(std::process::id()), now_ms());
        // non-completed event kind
        let status =
            SessionStatusDeriver::derive(&rec, Some(&EventKind::TurnChunk), &AlwaysAlive, now_ms());
        assert_eq!(
            status,
            SessionStatus::Running,
            "rule 2: alive pid → running"
        );
    }

    #[test]
    fn status_matrix_rule2_pid_alive_no_events_returns_running() {
        let rec = make_rec(Some(99_999_999), now_ms());
        let status = SessionStatusDeriver::derive(&rec, None, &AlwaysAlive, now_ms());
        assert_eq!(
            status,
            SessionStatus::Running,
            "rule 2: alive pid, no events → running"
        );
    }

    #[test]
    fn status_matrix_rule3_pid_dead_returns_lost() {
        let rec = make_rec(Some(99_999_999), now_ms());
        let status =
            SessionStatusDeriver::derive(&rec, Some(&EventKind::TurnChunk), &NeverAlive, now_ms());
        assert_eq!(status, SessionStatus::Lost, "rule 3: dead pid → lost");
    }

    #[test]
    fn status_matrix_rule3_pid_dead_no_events_returns_lost() {
        let rec = make_rec(Some(99_999_999), now_ms());
        let status = SessionStatusDeriver::derive(&rec, None, &NeverAlive, now_ms());
        assert_eq!(
            status,
            SessionStatus::Lost,
            "rule 3: dead pid, no events → lost"
        );
    }

    #[test]
    fn status_matrix_rule4_no_pid_old_session_returns_incomplete() {
        let old_ts = now_ms().saturating_sub(10 * 60 * 1_000);
        let rec = make_rec(None, old_ts);
        let status = SessionStatusDeriver::derive(&rec, None, &NeverAlive, now_ms());
        assert_eq!(
            status,
            SessionStatus::Incomplete,
            "rule 4: old no-pid → incomplete"
        );
    }

    #[test]
    fn status_matrix_rule4_no_pid_exactly_at_threshold_is_incomplete() {
        // 5 min + 1ms → incomplete
        let threshold = now_ms().saturating_sub(5 * 60 * 1_000 + 1);
        let rec = make_rec(None, threshold);
        let status = SessionStatusDeriver::derive(&rec, None, &NeverAlive, now_ms());
        assert_eq!(
            status,
            SessionStatus::Incomplete,
            "rule 4: exactly past threshold"
        );
    }

    #[test]
    fn status_matrix_rule5_no_pid_recent_session_returns_running() {
        let recent_ts = now_ms().saturating_sub(30 * 1_000);
        let rec = make_rec(None, recent_ts);
        let status = SessionStatusDeriver::derive(&rec, None, &NeverAlive, now_ms());
        assert_eq!(
            status,
            SessionStatus::Running,
            "rule 5: recent no-pid → running"
        );
    }

    #[test]
    fn status_matrix_rule5_no_pid_just_created_returns_running() {
        let rec = make_rec(None, now_ms());
        let status = SessionStatusDeriver::derive(&rec, None, &NeverAlive, now_ms());
        assert_eq!(
            status,
            SessionStatus::Running,
            "rule 5: brand-new no-pid → running"
        );
    }

    // ── Wire strings are stable ──────────────────────────────────────────────

    #[test]
    fn session_status_wire_strings_match_legacy_strings() {
        assert_eq!(SessionStatus::Completed.as_str(), "completed");
        assert_eq!(SessionStatus::Running.as_str(), "running");
        assert_eq!(SessionStatus::Lost.as_str(), "lost");
        assert_eq!(SessionStatus::Incomplete.as_str(), "incomplete");
    }

    // ── RepoError::Io display (error-text identity, S8 constraint) ───────────

    #[test]
    fn repo_error_io_display_starts_with_io_error() {
        // preflight::classify_error string-matches "io error:" — this test
        // asserts the exact prefix is preserved through the RepoError→String path.
        let e = RepoError::Io(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "access denied",
        ));
        let s = e.to_string();
        assert!(
            s.starts_with("io error:"),
            "RepoError::Io must start with 'io error:' for classify_error; got: {s:?}"
        );
    }

    // ── new_session_id format matches FileSessionRepo::create ────────────────

    #[test]
    fn new_session_id_format_matches_file_repo_create() {
        // Both store.rs::new_session_id and FileSessionRepo::create now produce
        // "session-{hex_ms}-{pid}-{counter}".  Assert the shared prefix and
        // that the id is valid for validate_store_id (ASCII alphanumeric/-/_).
        let id = new_session_id();
        let s = id.as_str();
        assert!(s.starts_with("session-"), "must start with 'session-'");
        let parts: Vec<&str> = s.splitn(4, '-').collect();
        assert_eq!(
            parts.len(),
            4,
            "expected 4 hyphen-delimited parts, got: {s:?}"
        );
        assert_eq!(parts[0], "session");
        // parts[1] = hex timestamp, parts[2] = pid, parts[3] = counter
        assert!(
            u128::from_str_radix(parts[1], 16).is_ok(),
            "second part must be hex timestamp; got: {:?}",
            parts[1]
        );
        assert!(
            parts[2].parse::<u32>().is_ok(),
            "third part must be numeric pid; got: {:?}",
            parts[2]
        );
        assert!(
            parts[3].parse::<u64>().is_ok(),
            "fourth part must be numeric counter; got: {:?}",
            parts[3]
        );
        // Must be valid as a store path component.
        validate_store_id(s).expect("new_session_id must pass validate_store_id");
    }

    #[test]
    fn new_session_id_uniqueness_across_rapid_calls() {
        // Two calls in the same millisecond must produce different ids.
        let id1 = new_session_id();
        let id2 = new_session_id();
        assert_ne!(id1, id2, "two rapid new_session_id calls must be distinct");
    }

    // ── list_sessions_from_base integration ──────────────────────────────────

    #[test]
    fn list_sessions_from_base_includes_created_session() {
        let dir = TestDir::new();
        let repo = FileSessionRepo::new(&dir.0);

        let spec = SessionSpec {
            cwd: dir.0.clone(),
            mode: "discussion".to_string(),
            prompt: "Test prompt for listing".to_string(),
        };
        let handle = repo.create(spec).unwrap();
        let id = handle.id.clone();

        // Write full metadata (required before list can read session.json).
        let meta = SessionMeta {
            id: id.as_str().to_string(),
            created_at_ms: now_ms(),
            pid: std::process::id(),
            prompt: "Test prompt for listing".to_string(),
            cwd: dir.0.clone(),
            mode: "discussion".to_string(),
        };
        repo.write_metadata(&id, &meta).unwrap();

        // Use the session-level function that now goes through the repo.
        let records = list_sessions_from_base(&dir.0).unwrap();
        let found = records.iter().find(|r| r.id == id.as_str());
        assert!(
            found.is_some(),
            "list_sessions_from_base must include the created session"
        );
        let rec = found.unwrap();
        assert!(rec.created_at_ms > 0, "created_at_ms must be nonzero");
        assert!(
            !rec.prompt_preview.is_empty(),
            "prompt_preview must not be empty"
        );
        // Status must be one of the four wire strings.
        assert!(
            ["completed", "running", "lost", "incomplete"].contains(&rec.status.as_str()),
            "status must be a valid wire string; got: {:?}",
            rec.status
        );
    }
}

// ── S7 AC regression guards (relocated from agent-swarm lib.rs P5-S5) ───────
//
// These tests enforce byte-identity of append_event_with_context output.
// They are S7 Acceptance Criteria:
//   Divergence A: oversized payloads must be compacted (truncated:true).
//   Divergence B: layer-report events must carry a non-empty `path` field.
//
// NOTE: the `strip_seq`/`canonical_json` helpers and discussion session tests
// were also in agent-swarm lib.rs but are in-lined here for self-containment.

#[cfg(test)]
mod s7_regression_guards {
    use super::*;
    use std::collections::BTreeSet;
    use swarm_kernel::ids::SessionId;
    use swarm_store::store::now_ms;

    fn make_session(tag: &str) -> (DiscussionSession, std::path::PathBuf) {
        let base = std::env::temp_dir();
        let test_id = format!("test-s7-{}-{}", tag, now_ms());
        let dir = base.join(&test_id);
        std::fs::create_dir_all(&dir).unwrap();
        let session = DiscussionSession {
            id: SessionId::from(test_id.clone()),
            dir: dir.clone(),
            events_path: dir.join("events.jsonl"),
            transcript_path: dir.join("transcript.md"),
            summary_path: dir.join("summary.md"),
            digest_path: dir.join("digest.md"),
            docs_path: dir.join("api-docs.md"),
            layer_reports_path: dir.join("layer-reports.jsonl"),
        };
        (session, dir)
    }

    fn strip_seq(v: &mut serde_json::Value) {
        if let Some(obj) = v.as_object_mut() {
            obj.remove("seq");
            obj.remove("ts_ms");
        }
    }

    fn object_keys(value: &serde_json::Value) -> BTreeSet<String> {
        value
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>()
    }

    #[test]
    fn discussion_session_appends_v2_envelope_event() {
        let (session, dir) = make_session("v2");
        session
            .append_event_with_context(
                EventKind::Other("test_kind".into()),
                serde_json::json!({"foo": "bar"}),
                Some("test-parent-456"),
                "mock-agent",
                "mock-role",
                "mock-phase",
            )
            .unwrap();

        let content = std::fs::read_to_string(&session.events_path).unwrap();
        let event: serde_json::Value = serde_json::from_str(&content).unwrap();

        assert_eq!(event["schema"], "agent-swarm/event/v2");
        assert_eq!(event["session_id"], session.id.as_str());
        assert_eq!(event["run_id"], session.id.as_str());
        assert_eq!(event["parent_id"], "test-parent-456");
        assert_eq!(event["agent_id"], "mock-agent");
        assert_eq!(event["role"], "mock-role");
        assert_eq!(event["phase"], "mock-phase");
        assert_eq!(event["kind"], "test_kind");
        assert_eq!(event["payload"]["foo"], "bar");
        assert!(event["seq"].is_i64());
        assert!(event["ts_ms"].is_number());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn discussion_session_preserves_null_parent_id_key() {
        let (session, dir) = make_session("null-parent");
        session
            .append_event_with_context(
                EventKind::Other("test_kind".into()),
                serde_json::json!({"foo": "bar"}),
                None,
                "mock-agent",
                "mock-role",
                "mock-phase",
            )
            .unwrap();

        let content = std::fs::read_to_string(&session.events_path).unwrap();
        let event: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert!(event.as_object().unwrap().contains_key("parent_id"));
        assert!(event["parent_id"].is_null());

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── S7 Divergence A: oversized-payload compaction ──────────────────────────

    #[test]
    fn wire_byte_parity_oversized_event_is_compacted() {
        // Divergence A: oversized events must be compacted on the new path just
        // like the old `append_event_inner` path. Without this fix, the new path
        // would write an un-compacted line while the old path writes `{truncated:true}`.
        let (session, dir) = make_session("oversized");

        // Build a payload that will exceed MAX_EVENT_LINE_BYTES (128 KiB).
        let big_text = "x".repeat(130 * 1024);
        session
            .append_event_with_context(
                EventKind::TurnChunk,
                serde_json::json!({ "text": big_text }),
                None,
                "auto",
                "participant",
                "discussion",
            )
            .unwrap();

        let raw = std::fs::read_to_string(&session.events_path).unwrap();
        let line = raw.trim_end_matches('\n').trim();
        let event: serde_json::Value = serde_json::from_str(line).unwrap();
        assert_eq!(
            event["payload"]["truncated"], true,
            "oversized event payload must be compacted (truncated:true); got: {}",
            event["payload"]
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── S7 Divergence B: layer-report event carries path field ─────────────────

    #[test]
    fn wire_byte_parity_layer_report_event_has_path_field() {
        // Divergence B: the LayerReport event must carry a `"path"` field
        // (the absolute path to layer_reports.jsonl).
        let (session, dir) = make_session("lr-path");

        session
            .append_layer_report(
                "round1",
                "architecture",
                "gemini",
                Some("manager"),
                "completed",
                "Design is solid.",
            )
            .unwrap();

        let raw = std::fs::read_to_string(&session.events_path).unwrap();
        let lr_event_line = raw
            .lines()
            .find(|l| l.contains("\"layer_report\""))
            .expect("events.jsonl must contain a layer_report event");
        let event: serde_json::Value = serde_json::from_str(lr_event_line).unwrap();
        let path_val = event["payload"]["path"].as_str().unwrap_or("");
        assert!(
            !path_val.is_empty(),
            "LayerReport event payload must contain a non-empty 'path' field; got: {}",
            event["payload"]
        );
        assert!(
            path_val.ends_with("layer-reports.jsonl"),
            "LayerReport event 'path' must end with layer-reports.jsonl; got: {path_val}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── layer_report_writes_index_and_sidecar ──────────────────────────────────

    #[test]
    fn layer_report_writes_index_and_sidecar() {
        let (session, dir) = make_session("lr-index");
        std::fs::create_dir_all(dir.join("layer-reports")).unwrap();

        session
            .append_layer_report(
                "helper",
                "risk/check",
                "claude:sonnet",
                Some("architecture"),
                "completed",
                "Full helper report body.",
            )
            .unwrap();

        let index = std::fs::read_to_string(&session.layer_reports_path).unwrap();
        assert!(index.contains("\"schema\":\"agent-swarm/layer-report/v1\""));
        assert!(index.contains("\"file\":\"layer-reports/"));
        let entry: serde_json::Value = serde_json::from_str(index.lines().next().unwrap()).unwrap();
        let sidecar = dir.join(entry["file"].as_str().unwrap());
        let sidecar_text = std::fs::read_to_string(sidecar).unwrap();
        assert!(sidecar_text.contains("Full helper report body."));

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── wire_byte_parity_small_event ──────────────────────────────────────────

    #[test]
    fn wire_byte_parity_small_event() {
        let (session, dir) = make_session("small-event");
        session
            .append_event_with_context(
                EventKind::TurnChunk,
                serde_json::json!({ "text": "hello" }),
                Some("parent-abc"),
                "agent-gemini",
                "architecture",
                "round1",
            )
            .unwrap();

        let raw = std::fs::read_to_string(&session.events_path).unwrap();
        let line = raw.trim_end_matches('\n').trim();
        let mut event: serde_json::Value = serde_json::from_str(line).unwrap();
        strip_seq(&mut event);

        assert_eq!(event["schema"], "agent-swarm/event/v2");
        assert_eq!(event["session_id"], session.id.as_str());
        assert_eq!(event["run_id"], session.id.as_str());
        assert_eq!(event["parent_id"], "parent-abc");
        assert_eq!(event["agent_id"], "agent-gemini");
        assert_eq!(event["role"], "architecture");
        assert_eq!(event["phase"], "round1");
        assert_eq!(event["kind"], "turn_chunk");
        assert_eq!(event["payload"]["text"], "hello");
        assert!(!event.as_object().unwrap().contains_key("seq"));
        assert!(!event.as_object().unwrap().contains_key("ts_ms"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── object_keys helper coverage ────────────────────────────────────────────

    #[test]
    fn object_keys_covers_full_v2_envelope() {
        let (session, dir) = make_session("envelope-keys");
        session
            .append_event_with_context(
                EventKind::Other("test_kind".into()),
                serde_json::json!({"foo": "bar"}),
                None,
                "mock-agent",
                "mock-role",
                "mock-phase",
            )
            .unwrap();

        let content = std::fs::read_to_string(&session.events_path).unwrap();
        let event: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(
            object_keys(&event),
            BTreeSet::from([
                "agent_id".to_string(),
                "kind".to_string(),
                "parent_id".to_string(),
                "payload".to_string(),
                "phase".to_string(),
                "role".to_string(),
                "run_id".to_string(),
                "schema".to_string(),
                "seq".to_string(),
                "session_id".to_string(),
                "ts_ms".to_string(),
            ])
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}

// P5-S2: compact_event_payload has been moved to swarm-store's event_repo
// as a private inline function. The function is no longer needed here — it was
// only called from repos/event_repo.rs which now lives in swarm-store.
