//! P5-S5 — Relocated from agent-swarm/tests/repo_contract.rs.
//!
//! External integration test for all 6 repo traits (Gate-3 dual-backend
//! contract). Tests now run against the swarm-* crates directly.
//!
//! # Coverage
//!
//! - `TelemetryRepo`       — Mem + File, FIFO order, accumulation
//! - `PackageRepo`         — Static (single backend in Phase 1), 3 backends,
//!   non-empty presets + profiles
//! - `JobRepo`             — Mem + File, CRUD, liveness reconcile (NeverAlive /
//!   AlwaysAlive / pid=None / pid=0), stderr breadcrumb
//! - `RoutingMemoryRepo`   — Mem + File, empty-state defaults, seeded math
//! - `EventRepo`           — Mem + File, append/cursor/tail/latest_kind,
//!   torn-line recovery (File only)
//! - `SessionRepo`         — Mem + File, create/write_metadata/open/list/summary/
//!   artifacts, `SessionStatusDeriver` derivation rules

use swarm_cli::package_repo::{PackageRepo, StaticPackageRepo};
use swarm_cli::routing_repo::{RoutingMemory, RoutingMemoryRepo, RECOMMENDATION_ROLES};
use swarm_core::{AlwaysAlive, Cursor, NeverAlive, RepoError};
use swarm_store::repos::event_repo::{
    EventContext, EventRepo, FileEventRepo, LayerReportSpec, MemEventRepo,
};
use swarm_store::repos::job_repo::{FileJobRepo, JobRepo, JobSpec, MemJobRepo};
use swarm_store::repos::session_repo::{
    FileSessionRepo, MemSessionRepo, SessionIndexRecord, SessionMeta, SessionRepo, SessionSpec,
    SessionStatus, SessionStatusDeriver,
};
use swarm_store::repos::telemetry_repo::{FileTelemetryRepo, MemTelemetryRepo, TelemetryRepo};

use swarm_contracts::ids::{JobId, ProposalId, SessionId};
use swarm_kernel::events::EventKind;
use swarm_kernel::job_types::{JobAgent, JobMode, JobStatus};
use swarm_kernel::telemetry::{AgentFeedback, AgentObservation, AgentProposal, AgentProposalVote};

// ── TelemetryRepo contract ─────────────────────────────────────────────────────

fn sample_observation(n: u64) -> AgentObservation {
    AgentObservation {
        schema: "agent-swarm/observation/v1".into(),
        ts_ms: n as u128,
        mode: "consult".into(),
        session_id: None,
        role: format!("role-{n}"),
        agent: "claude:sonnet".into(),
        cwd: "/tmp".into(),
        status: "completed".into(),
        exit_code: 0,
        timed_out: false,
        duration_ms: 1000 + n as u128,
        prompt_bytes: 100,
        stdout_bytes: 200,
        stderr_bytes: 0,
        input_tokens: None,
        output_tokens: None,
    }
}

fn sample_feedback(n: u64) -> AgentFeedback {
    AgentFeedback {
        schema: "agent-swarm/feedback/v1".into(),
        ts_ms: n as u128,
        session_id: None,
        role: format!("role-{n}"),
        agent: "gemini".into(),
        outcome: "win".into(),
        note: Some(format!("note-{n}")),
        weight: 1.0,
    }
}

fn sample_proposal(n: u64) -> AgentProposal {
    AgentProposal {
        schema: "agent-swarm/proposal/v1".into(),
        id: ProposalId::from(format!("proposal-{n:x}")),
        ts_ms: n as u128,
        session_id: None,
        title: format!("title-{n}"),
        body: format!("body-{n}"),
        proposed_by: "user".into(),
        status: "open".into(),
        tags: vec![format!("tag-{n}")],
    }
}

fn sample_vote(n: u64) -> AgentProposalVote {
    AgentProposalVote {
        schema: "agent-swarm/proposal-vote/v1".into(),
        ts_ms: n as u128,
        proposal_id: ProposalId::from(format!("proposal-{n:x}")),
        voter: format!("voter-{n}"),
        vote: "approve".into(),
        rationale: None,
        weight: 1.0,
    }
}

fn telemetry_repo_contract<R: TelemetryRepo>(repo: R) {
    // empty state
    assert!(repo.observations().unwrap().is_empty());
    assert!(repo.feedback().unwrap().is_empty());
    assert!(repo.proposals().unwrap().is_empty());
    assert!(repo.proposal_votes().unwrap().is_empty());

    // observations round-trip — 3 items, FIFO
    for n in 1u64..=3 {
        repo.record_observation(sample_observation(n)).unwrap();
    }
    let obs = repo.observations().unwrap();
    assert_eq!(obs.len(), 3);
    assert_eq!(obs[0].ts_ms, 1);
    assert_eq!(obs[1].ts_ms, 2);
    assert_eq!(obs[2].ts_ms, 3);
    assert_eq!(obs[0].role, "role-1");

    // feedback round-trip
    for n in 1u64..=3 {
        repo.record_feedback(sample_feedback(n)).unwrap();
    }
    let fb = repo.feedback().unwrap();
    assert_eq!(fb.len(), 3);
    assert_eq!(fb[0].ts_ms, 1);
    assert_eq!(fb[2].ts_ms, 3);
    assert_eq!(fb[0].outcome, "win");

    // proposals round-trip
    for n in 1u64..=3 {
        repo.record_proposal(sample_proposal(n)).unwrap();
    }
    let props = repo.proposals().unwrap();
    assert_eq!(props.len(), 3);
    assert_eq!(props[0].ts_ms, 1);
    assert_eq!(props[2].title, "title-3");
    assert_eq!(props[0].status, "open");

    // proposal_votes round-trip
    for n in 1u64..=3 {
        repo.record_proposal_vote(sample_vote(n)).unwrap();
    }
    let votes = repo.proposal_votes().unwrap();
    assert_eq!(votes.len(), 3);
    assert_eq!(votes[0].ts_ms, 1);
    assert_eq!(votes[0].vote, "approve");

    // accumulation
    repo.record_observation(sample_observation(99)).unwrap();
    assert_eq!(repo.observations().unwrap().len(), 4);
}

#[test]
fn telemetry_repo_contract_mem() {
    telemetry_repo_contract(MemTelemetryRepo::new());
}

#[test]
fn telemetry_repo_contract_file() {
    let dir = tempfile::tempdir().unwrap();
    telemetry_repo_contract(FileTelemetryRepo::new(dir.path().to_path_buf()));
}

// ── PackageRepo contract ───────────────────────────────────────────────────────

fn package_repo_contract<R: PackageRepo>(repo: R) {
    // manifest() — supported backend capabilities
    let manifest = repo.manifest().unwrap();
    let capabilities = manifest
        .get("capabilities")
        .and_then(|v| v.as_array())
        .expect("manifest must have capabilities array");
    let backends = capabilities
        .iter()
        .filter_map(|cap| cap.as_str())
        .filter(|cap| cap.starts_with("backend."))
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        backends,
        std::collections::BTreeSet::from(["backend.claude", "backend.codex"])
    );

    // presets() — non-empty array
    let presets = repo.presets().unwrap();
    let preset_list = presets
        .get("presets")
        .and_then(|v| v.as_array())
        .expect("presets must have presets array");
    assert!(!preset_list.is_empty(), "presets must not be empty");

    // profiles() — non-empty
    let profiles = repo.profiles().unwrap();
    assert!(!profiles.is_empty(), "profiles must not be empty");
}

#[test]
fn package_repo_contract_static() {
    // Static is both Mem and File for Phase 1 (compiled-in data, no disk).
    package_repo_contract(StaticPackageRepo);
}

// ── JobRepo contract ───────────────────────────────────────────────────────────

fn make_job_spec(preview: &str) -> JobSpec {
    JobSpec {
        agent: JobAgent::Claude,
        model: Some("sonnet".to_string()),
        mode: JobMode::Consult,
        cwd: std::path::PathBuf::from("/tmp"),
        prompt_preview: preview.to_string(),
        prompt_text: format!("full text for: {preview}"),
        timeout_secs: 300,
        allow_recursive_codex: false,
    }
}

fn job_repo_contract<R: JobRepo>(repo: &R, base_dir: &std::path::Path) {
    // 1. create → invariants
    let created = repo.create(make_job_spec("alpha")).unwrap();
    assert_eq!(created.status, JobStatus::Running);
    assert_eq!(created.started_at_ms, Some(created.created_at_ms));
    assert!(created.pid.is_some());
    assert_eq!(created.prompt_preview, "alpha");

    // 2. get round-trip
    let fetched = repo.get(&created.id).unwrap();
    assert_eq!(fetched, created);

    // 3. path suffixes
    assert!(created.prompt_path.ends_with(".prompt.md"));
    assert!(created.stdout_path.ends_with(".stdout.log"));
    assert!(created.stderr_path.ends_with(".stderr.log"));
    assert!(created.result_path.ends_with(".result.txt"));
    let base_str = base_dir.display().to_string();
    assert!(created.prompt_path.starts_with(&base_str));

    // 4. save additional records
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

    // 5. list() is pure — no implicit liveness check
    let pre_update_list = repo.list().unwrap();
    assert!(pre_update_list
        .iter()
        .all(|r| r.status == JobStatus::Running));

    // 6. save updates a record
    let mut to_update = fetched.clone();
    to_update.status = JobStatus::Completed;
    to_update.completed_at_ms = Some(1_999_999);
    to_update.exit_code = Some(0);
    repo.save(&to_update).unwrap();
    let after_save = repo.get(&created.id).unwrap();
    assert_eq!(after_save.status, JobStatus::Completed);
    assert_eq!(after_save.exit_code, Some(0));

    // 7. latest() — max (created_at_ms, id)
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

    // 8. get(unknown) → NotFound
    let missing = JobId::from("job-does-not-exist-external-contract");
    let err = repo.get(&missing).unwrap_err();
    assert!(matches!(err, RepoError::NotFound(_)));
}

fn liveness_contract<R: JobRepo>(repo: &R) {
    let alive_record = repo.create(make_job_spec("liveness-alpha")).unwrap();
    let pid = alive_record.pid.unwrap();
    assert!(pid > 0);

    // NeverAlive: Running + pid>0 → Lost
    let flipped = repo.reconcile_liveness(&NeverAlive).unwrap();
    assert_eq!(flipped.len(), 1);
    let flipped_rec = &flipped[0];
    assert_eq!(flipped_rec.id, alive_record.id);
    assert_eq!(flipped_rec.status, JobStatus::Lost);
    assert!(flipped_rec.completed_at_ms.is_some());
    assert_eq!(flipped_rec.exit_code, Some(1));

    let post_reconcile = repo.get(&alive_record.id).unwrap();
    assert_eq!(post_reconcile.status, JobStatus::Lost);

    // Idempotent
    let flipped2 = repo.reconcile_liveness(&NeverAlive).unwrap();
    assert!(flipped2.is_empty());

    // AlwaysAlive: stays Running
    let still_alive = repo.create(make_job_spec("liveness-beta")).unwrap();
    let no_change = repo.reconcile_liveness(&AlwaysAlive).unwrap();
    assert!(no_change.is_empty());
    let after_alive = repo.get(&still_alive.id).unwrap();
    assert_eq!(after_alive.status, JobStatus::Running);

    // pid=None: stays Running with NeverAlive
    let mut no_pid_record = repo.create(make_job_spec("liveness-no-pid")).unwrap();
    no_pid_record.pid = None;
    repo.save(&no_pid_record).unwrap();
    let flipped_no_pid = repo.reconcile_liveness(&NeverAlive).unwrap();
    assert!(!flipped_no_pid.iter().any(|r| r.id == no_pid_record.id));
    let no_pid_after = repo.get(&no_pid_record.id).unwrap();
    assert_eq!(no_pid_after.status, JobStatus::Running);

    // pid=Some(0): Lost even with AlwaysAlive
    let mut zero_pid_record = repo.create(make_job_spec("liveness-zero-pid")).unwrap();
    zero_pid_record.pid = Some(0);
    repo.save(&zero_pid_record).unwrap();
    let flipped_zero = repo.reconcile_liveness(&AlwaysAlive).unwrap();
    assert!(flipped_zero.iter().any(|r| r.id == zero_pid_record.id));
    let zero_after = repo.get(&zero_pid_record.id).unwrap();
    assert_eq!(zero_after.status, JobStatus::Lost);
}

#[test]
fn job_repo_contract_mem() {
    let base = std::path::PathBuf::from("/mem/partner-jobs");
    let repo = MemJobRepo::new(base.clone());
    job_repo_contract(&repo, &base);
}

#[test]
fn job_repo_liveness_contract_mem() {
    let repo = MemJobRepo::new("/mem/partner-jobs");
    liveness_contract(&repo);
}

#[test]
fn job_repo_contract_file() {
    let dir = tempfile::tempdir().unwrap();
    let repo = FileJobRepo::new(dir.path());
    job_repo_contract(&repo, dir.path());
}

#[test]
fn job_repo_liveness_contract_file() {
    let dir = tempfile::tempdir().unwrap();
    let repo = FileJobRepo::new(dir.path());
    liveness_contract(&repo);
}

#[test]
fn file_job_repo_flip_writes_stderr_breadcrumb() {
    let dir = tempfile::tempdir().unwrap();
    let repo = FileJobRepo::new(dir.path());
    let record = repo.create(make_job_spec("breadcrumb-test")).unwrap();
    let stderr_path = std::path::PathBuf::from(&record.stderr_path);
    assert!(!stderr_path.exists());
    repo.reconcile_liveness(&NeverAlive).unwrap();
    assert!(stderr_path.exists());
    let content = std::fs::read_to_string(&stderr_path).unwrap();
    assert!(content.contains("Worker process exited"));
}

// ── RoutingMemoryRepo contract ─────────────────────────────────────────────────

fn arch_sonnet_observation(n: u64, fail: bool) -> AgentObservation {
    AgentObservation {
        schema: "agent-swarm/observation/v1".into(),
        ts_ms: n as u128,
        mode: "consult".into(),
        session_id: None,
        role: "architecture".into(),
        agent: "claude:sonnet".into(),
        cwd: "/tmp".into(),
        status: if fail { "failed" } else { "completed" }.into(),
        exit_code: if fail { 1 } else { 0 },
        timed_out: false,
        duration_ms: 5_000,
        prompt_bytes: 100,
        stdout_bytes: 200,
        stderr_bytes: 0,
        input_tokens: None,
        output_tokens: None,
    }
}

fn arch_gemini_observation(n: u64) -> AgentObservation {
    AgentObservation {
        schema: "agent-swarm/observation/v1".into(),
        ts_ms: n as u128,
        mode: "consult".into(),
        session_id: None,
        role: "architecture".into(),
        agent: "gemini".into(),
        cwd: "/tmp".into(),
        status: "completed".into(),
        exit_code: 0,
        timed_out: false,
        duration_ms: 4_000,
        prompt_bytes: 100,
        stdout_bytes: 200,
        stderr_bytes: 0,
        input_tokens: None,
        output_tokens: None,
    }
}

fn arch_sonnet_win_feedback(n: u64) -> AgentFeedback {
    AgentFeedback {
        schema: "agent-swarm/feedback/v1".into(),
        ts_ms: n as u128,
        session_id: None,
        role: "architecture".into(),
        agent: "claude:sonnet".into(),
        outcome: "win".into(),
        note: None,
        weight: 1.0,
    }
}

fn routing_memory_repo_contract<R: RoutingMemoryRepo>(repo: R) {
    // empty state
    let stats = repo.agent_stats().unwrap();
    assert!(stats.is_empty());

    let best_empty = repo.best_agent_for_role("architecture").unwrap();
    assert_eq!(best_empty, "gemini");

    let recs_empty = repo.recommendations().unwrap();
    assert_eq!(recs_empty.len(), RECOMMENDATION_ROLES.len());
    for rec in &recs_empty {
        assert!(!rec.role.is_empty());
        assert!(!rec.agent.is_empty());
    }
}

fn routing_memory_repo_seeded<T: TelemetryRepo>(telemetry: T) {
    telemetry
        .record_observation(arch_sonnet_observation(1, false))
        .unwrap();
    telemetry
        .record_observation(arch_sonnet_observation(2, true))
        .unwrap();
    telemetry
        .record_observation(arch_gemini_observation(3))
        .unwrap();
    telemetry
        .record_feedback(arch_sonnet_win_feedback(4))
        .unwrap();

    let repo = RoutingMemory::new(telemetry);

    let stats = repo.agent_stats().unwrap();
    assert_eq!(stats.len(), 2);

    let sonnet_stat = stats
        .iter()
        .find(|s| s.agent == "claude:sonnet")
        .expect("expected claude:sonnet stats");
    assert_eq!(sonnet_stat.role, "architecture");
    assert_eq!(sonnet_stat.runs, 2);
    assert_eq!(sonnet_stat.failures, 1);
    assert_eq!(sonnet_stat.feedback_wins, 1);

    let gemini_stat = stats
        .iter()
        .find(|s| s.agent == "gemini")
        .expect("expected gemini stats");
    assert_eq!(gemini_stat.runs, 1);
    assert_eq!(gemini_stat.failures, 0);
    assert_eq!(gemini_stat.feedback_wins, 0);

    let best = repo.best_agent_for_role("architecture").unwrap();
    assert_eq!(best, "gemini");

    let recs = repo.recommendations().unwrap();
    assert_eq!(recs.len(), RECOMMENDATION_ROLES.len());
    let arch_rec = recs.iter().find(|r| r.role == "architecture").unwrap();
    assert_eq!(arch_rec.agent, "gemini");
}

#[test]
fn routing_repo_contract_mem() {
    routing_memory_repo_contract(RoutingMemory::new(MemTelemetryRepo::new()));
}

#[test]
fn routing_repo_contract_file() {
    let dir = tempfile::tempdir().unwrap();
    routing_memory_repo_contract(RoutingMemory::new(FileTelemetryRepo::new(
        dir.path().to_path_buf(),
    )));
}

#[test]
fn routing_repo_seeded_mem() {
    routing_memory_repo_seeded(MemTelemetryRepo::new());
}

#[test]
fn routing_repo_seeded_file() {
    let dir = tempfile::tempdir().unwrap();
    routing_memory_repo_seeded(FileTelemetryRepo::new(dir.path().to_path_buf()));
}

// ── EventRepo contract ─────────────────────────────────────────────────────────

fn default_ctx() -> EventContext {
    EventContext::default()
}

fn event_repo_contract<R: EventRepo>(repo: &R, session: &SessionId) {
    // 1. empty log
    let (events, _cursor) = repo.events_since(session, Cursor::start(), 100).unwrap();
    assert!(events.is_empty());

    // 2. append 5 events; read back all 5 in FIFO order
    for i in 0u32..5 {
        repo.append(
            session,
            EventKind::TurnChunk,
            serde_json::json!({ "i": i }),
            default_ctx(),
        )
        .unwrap();
    }
    let (events, cursor_after_5) = repo.events_since(session, Cursor::start(), 100).unwrap();
    assert_eq!(events.len(), 5);
    for (idx, event) in events.iter().enumerate() {
        let i_val = event.payload.get("i").and_then(|v| v.as_u64()).unwrap();
        assert_eq!(i_val as usize, idx);
    }

    // 3. cursor from (2) — only 1 new event
    repo.append(
        session,
        EventKind::TurnCompleted,
        serde_json::json!({ "done": true }),
        default_ctx(),
    )
    .unwrap();
    let (new_events, _) = repo.events_since(session, cursor_after_5, 100).unwrap();
    assert_eq!(new_events.len(), 1);
    assert_eq!(new_events[0].kind, EventKind::TurnCompleted);

    // 4. append_layer_report — no deadlock
    let spec = LayerReportSpec {
        layer: "worker".to_string(),
        role: "architecture".to_string(),
        agent: "gemini".to_string(),
        parent_role: Some("manager".to_string()),
        status: "completed".to_string(),
        text: "Design looks solid.".to_string(),
    };
    let _ = repo.append_layer_report(session, spec).unwrap();

    let (lr_events, _) = repo.events_since(session, Cursor::start(), 100).unwrap();
    assert!(lr_events.len() > 6);
    let last_kind = &lr_events.last().unwrap().kind;
    assert_eq!(*last_kind, EventKind::LayerReport);
}

#[test]
fn event_repo_contract_mem() {
    let repo = MemEventRepo::new();
    let session = SessionId::from("ext-contract-event-mem");
    event_repo_contract(&repo, &session);
}

#[test]
fn event_repo_contract_file() {
    let dir = tempfile::tempdir().unwrap();
    let repo = FileEventRepo::new(dir.path());
    let session = SessionId::from("ext-contract-event-file");
    std::fs::create_dir_all(dir.path().join(session.as_str())).unwrap();
    event_repo_contract(&repo, &session);
}

/// Torn-line recovery (FileEventRepo only).
#[test]
fn file_event_repo_torn_line_recovery() {
    use std::io::Write as IoWrite;
    let dir = tempfile::tempdir().unwrap();
    let repo = FileEventRepo::new(dir.path());
    let session = SessionId::from("ext-torn-line-session");
    std::fs::create_dir_all(dir.path().join(session.as_str())).unwrap();

    // 1. Append one complete event.
    repo.append(
        &session,
        EventKind::Created,
        serde_json::json!({ "cwd": "/tmp", "mode": "test" }),
        default_ctx(),
    )
    .unwrap();

    // 2. Write torn bytes (no trailing `\n`).
    let events_path = dir.path().join(session.as_str()).join("events.jsonl");
    {
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&events_path)
            .unwrap();
        f.write_all(b"{\"kind\":\"torn_no_newline\"").unwrap();
    }

    // 3. Must return only 1 complete event.
    let (events, _cursor) = repo.events_since(&session, Cursor::start(), 100).unwrap();
    assert_eq!(
        events.len(),
        1,
        "torn-line recovery: expected 1 event, got {}",
        events.len()
    );
    assert_eq!(events[0].kind, EventKind::Created);
}

// ── SessionRepo contract ───────────────────────────────────────────────────────

fn test_session_spec() -> SessionSpec {
    SessionSpec {
        cwd: std::path::PathBuf::from("/tmp/test"),
        mode: "discussion".to_string(),
        prompt: "What is the answer?".to_string(),
    }
}

fn test_session_meta(id: &SessionId) -> SessionMeta {
    SessionMeta {
        id: id.as_str().to_string(),
        created_at_ms: 1_700_000_000_000,
        pid: std::process::id(),
        prompt: "What is the answer?".to_string(),
        cwd: std::path::PathBuf::from("/tmp/test"),
        mode: "discussion".to_string(),
    }
}

fn session_repo_contract<R: SessionRepo>(repo: &R)
where
    R::Events: EventRepo,
{
    // 1. create
    let handle = repo.create(test_session_spec()).unwrap();
    let id = handle.id.clone();
    assert!(!id.as_str().is_empty());

    // 2. write_metadata
    let meta = test_session_meta(&id);
    repo.write_metadata(&id, &meta).unwrap();

    // 3. open existing
    let reopened = repo.open(&id).unwrap();
    assert_eq!(reopened.id, id);

    // open nonexistent → NotFound
    let bad_id = SessionId::from("session-does-not-exist-ext");
    let open_result = repo.open(&bad_id);
    assert!(matches!(open_result, Err(RepoError::NotFound(_))));

    // 4. list — includes created session
    let records = repo.list().unwrap();
    let found = records.iter().find(|r| r.id == id);
    assert!(found.is_some());
    let rec = found.unwrap();
    assert!(rec.created_at_ms > 0);
    assert!(!rec.prompt_preview.is_empty());

    // 5. summary — schema field present
    let summary = repo.summary(&id).unwrap();
    assert_eq!(
        summary.value["schema"],
        serde_json::json!("agent-swarm/session-summary/v1")
    );
    let sid = summary.value["session_id"].as_str().unwrap_or("");
    assert_eq!(sid, id.as_str());

    // 6. artifacts — must not panic
    let artifacts = repo.artifacts(&id).unwrap();
    let _count = artifacts.len();

    // 7. append via handle
    handle
        .events
        .append(
            &id,
            EventKind::TurnChunk,
            serde_json::json!({ "text": "hello" }),
            EventContext::default(),
        )
        .unwrap();
}

#[test]
fn session_repo_contract_mem() {
    let repo = MemSessionRepo::new();
    session_repo_contract(&repo);
}

#[test]
fn session_repo_contract_file() {
    let dir = tempfile::tempdir().unwrap();
    let repo = FileSessionRepo::new(dir.path());
    session_repo_contract(&repo);
}

// ── SessionStatusDeriver ──────────────────────────────────────────────────────

fn make_index_rec(id: &str, pid: Option<u32>, created_at_ms: u128) -> SessionIndexRecord {
    SessionIndexRecord {
        id: SessionId::from(id),
        created_at_ms,
        pid,
        prompt_preview: String::new(),
    }
}

#[test]
fn status_deriver_completed_event_overrides_pid() {
    let rec = make_index_rec("ext-status-completed", Some(99_999_999), 1_700_000_000_000);
    let status = SessionStatusDeriver::derive(
        &rec,
        Some(&EventKind::SessionCompleted),
        &AlwaysAlive,
        1_700_000_001_000,
    );
    assert_eq!(status, SessionStatus::Completed);
}

#[test]
fn status_deriver_alive_pid_returns_running() {
    let rec = make_index_rec("ext-status-running", Some(99_999_999), 1_700_000_000_000);
    let status = SessionStatusDeriver::derive(
        &rec,
        Some(&EventKind::TurnChunk),
        &AlwaysAlive,
        1_700_000_001_000,
    );
    assert_eq!(status, SessionStatus::Running);
}

#[test]
fn status_deriver_dead_pid_returns_lost() {
    let rec = make_index_rec("ext-status-lost", Some(99_999_999), 1_700_000_000_000);
    let status = SessionStatusDeriver::derive(
        &rec,
        Some(&EventKind::TurnChunk),
        &NeverAlive,
        1_700_000_001_000,
    );
    assert_eq!(status, SessionStatus::Lost);
}

#[test]
fn status_deriver_no_pid_old_session_returns_incomplete() {
    let old_ts = 1_700_000_000_000u128;
    let now = old_ts + 10 * 60 * 1_000; // 10 minutes later
    let rec = make_index_rec("ext-status-incomplete", None, old_ts);
    let status = SessionStatusDeriver::derive(&rec, None, &NeverAlive, now);
    assert_eq!(status, SessionStatus::Incomplete);
}

#[test]
fn status_deriver_no_pid_recent_session_returns_running() {
    let recent_ts = 1_700_000_000_000u128;
    let now = recent_ts + 30 * 1_000; // 30 seconds later
    let rec = make_index_rec("ext-status-recent", None, recent_ts);
    let status = SessionStatusDeriver::derive(&rec, None, &NeverAlive, now);
    assert_eq!(status, SessionStatus::Running);
}

#[test]
fn session_status_wire_strings() {
    assert_eq!(SessionStatus::Completed.as_str(), "completed");
    assert_eq!(SessionStatus::Running.as_str(), "running");
    assert_eq!(SessionStatus::Lost.as_str(), "lost");
    assert_eq!(SessionStatus::Incomplete.as_str(), "incomplete");
}
