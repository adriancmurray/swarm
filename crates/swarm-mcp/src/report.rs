//! Session report JSON assemblers.

use std::collections::HashSet;
use std::fs;
use std::path::Path;

use sysinfo::{ProcessRefreshKind, RefreshKind, System};

use swarm_core::event_repo::EventRepo;
use swarm_core::{JobRepo, SessionStatusDeriver};
use swarm_exec::session::{list_sessions, list_sessions_from_base};
use swarm_kernel::format::{preview_for_event, prompt_preview};
use swarm_kernel::ids::SessionId;
use swarm_kernel::process::{pid_to_u32, process_is_alive};
use swarm_store::monitor_store::read_monitor_pid;
use swarm_store::repos::event_repo::FileEventRepo;
use swarm_store::repos::job_repo::FileJobRepo;
use swarm_store::store::{
    job_store_dir, now_ms, read_text_tail, session_dir, session_store_dir,
    MAX_SESSION_EVENTS_TAIL_BYTES,
};
use swarm_store::OsProcessLiveness;

pub fn session_summary_json(id: &str) -> Result<serde_json::Value, String> {
    let dir = session_dir(id)?;
    session_summary_json_from_dir(id, &dir)
}

pub fn session_summary_json_from_dir(id: &str, dir: &Path) -> Result<serde_json::Value, String> {
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
        .and_then(|value| value.as_u64())
        .map(u128::from)
        .unwrap_or_default();
    let pid = metadata
        .get("pid")
        .and_then(|v| v.as_u64())
        .and_then(|v| u32::try_from(v).ok());
    let sid = SessionId::from(id);
    // Derive session base dir for the event repo — the parent of `dir`.
    let base_dir = dir.parent().unwrap_or(dir);
    let event_repo = FileEventRepo::new(base_dir);
    let latest_kind = event_repo.latest_kind(&sid).unwrap_or(None);
    let rec = swarm_core::session_repo::SessionIndexRecord {
        id: sid,
        created_at_ms,
        pid,
        prompt_preview: String::new(), // not used for status derivation
    };
    let status =
        SessionStatusDeriver::derive(&rec, latest_kind.as_ref(), &OsProcessLiveness, now_ms());
    let status = status.as_str().to_string();
    let events = read_session_events_value(dir, 12)?;
    let digest = fs::read_to_string(dir.join("digest.md")).unwrap_or_default();
    let summary = fs::read_to_string(dir.join("summary.md")).unwrap_or_default();
    let docs = fs::read_to_string(dir.join("api-docs.md")).unwrap_or_default();
    Ok(serde_json::json!({
        "schema": "agent-swarm/session-summary/v1",
        "session_id": id,
        "status": status,
        "created_at_ms": created_at_ms,
        "updated_at_ms": events.last().and_then(|event| event.get("ts_ms")).and_then(|value| value.as_u64()).unwrap_or(created_at_ms as u64),
        "prompt_preview": metadata.get("prompt").and_then(|value| value.as_str()).map(prompt_preview).unwrap_or_default(),
        "cwd": metadata.get("cwd").and_then(|value| value.as_str()).unwrap_or(""),
        "manager": metadata.get("manager").cloned().unwrap_or(serde_json::Value::Null),
        "participants": metadata.get("participants").cloned().unwrap_or_else(|| serde_json::json!([])),
        "digest": preview_for_event(&digest, 4000),
        "summary_preview": preview_for_event(&summary, 2200),
        "docs_preview": preview_for_event(&docs, 1200),
        "recent_events": events,
        "artifacts": session_artifacts_json_from_dir(id, dir)?["artifacts"].clone()
    }))
}

pub fn session_artifacts_json(id: &str) -> Result<serde_json::Value, String> {
    let dir = session_dir(id)?;
    session_artifacts_json_from_dir(id, &dir)
}

pub fn session_artifacts_json_from_dir(id: &str, dir: &Path) -> Result<serde_json::Value, String> {
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

fn read_session_events_value(
    dir: &Path,
    limit_from_tail: usize,
) -> Result<Vec<serde_json::Value>, String> {
    let path = dir.join("events.jsonl");
    let text = read_text_tail(&path, MAX_SESSION_EVENTS_TAIL_BYTES)?;
    let mut events = text
        .lines()
        .rev()
        .take(limit_from_tail)
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .collect::<Vec<_>>();
    events.reverse();
    Ok(events)
}

pub fn session_list_json() -> Result<serde_json::Value, String> {
    session_list_json_from_base(&session_store_dir()?)
}

pub fn session_list_json_from_base(base: &Path) -> Result<serde_json::Value, String> {
    let mut sessions = list_sessions_from_base(base)?;
    sessions.sort_by_key(|session| session.created_at_ms);
    sessions.reverse();
    let sessions = sessions
        .into_iter()
        .take(20)
        .map(|session| {
            serde_json::json!({
                "id": session.id,
                "status": session.status,
                "created_at_ms": session.created_at_ms,
                "prompt_preview": session.prompt_preview,
            })
        })
        .collect::<Vec<_>>();
    Ok(serde_json::json!({
        "schema": "agent-swarm/session-list/v1",
        "sessions": sessions
    }))
}

pub fn runtime_processes_json() -> Result<serde_json::Value, String> {
    let mut processes = Vec::new();
    let mut tracked_pids = HashSet::new();

    let self_pid = std::process::id();
    tracked_pids.insert(self_pid);

    if let Ok(Some(monitor_pid)) = read_monitor_pid() {
        if process_is_alive(monitor_pid) {
            tracked_pids.insert(monitor_pid);
        }
    }

    let job_repo = FileJobRepo::new(job_store_dir().map_err(|e| e.to_string())?);
    let _ = job_repo.reconcile_liveness(&OsProcessLiveness);
    for job in job_repo.list().map_err(|e| e.to_string())? {
        if matches!(job.status.as_str(), "queued" | "running" | "lost") {
            let alive = job.pid.map(process_is_alive).unwrap_or(false);
            if let Some(pid) = job.pid {
                if alive {
                    tracked_pids.insert(pid);
                }
            }
            processes.push(serde_json::json!({
                "kind": "job",
                "id": job.id,
                "status": job.status,
                "pid": job.pid,
                "alive": alive,
                "agent": job.agent,
                "mode": job.mode,
                "cwd": job.cwd,
                "started_at_ms": job.started_at_ms,
                "elapsed_ms": job.started_at_ms.map(|started| now_ms().saturating_sub(started))
            }));
        }
    }

    for session in list_sessions()? {
        if matches!(session.status.as_str(), "running" | "incomplete" | "lost") {
            let dir = session_dir(&session.id)?;
            let metadata = fs::read_to_string(dir.join("session.json"))
                .ok()
                .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
                .unwrap_or_else(|| serde_json::json!({}));
            let pid = metadata
                .get("pid")
                .and_then(|value| value.as_u64())
                .and_then(|value| u32::try_from(value).ok());
            let alive = pid.map(process_is_alive).unwrap_or(false);
            if let Some(p) = pid {
                if alive {
                    tracked_pids.insert(p);
                }
            }
            processes.push(serde_json::json!({
                "kind": "session",
                "id": session.id,
                "status": session.status,
                "pid": pid,
                "alive": alive,
                "prompt_preview": session.prompt_preview,
                "started_at_ms": session.created_at_ms,
                "elapsed_ms": now_ms().saturating_sub(session.created_at_ms)
            }));
        }
    }

    let system_pids = list_system_agent_swarm_pids();
    for process in system_pids {
        let pid = process.pid;
        if !tracked_pids.contains(&pid) {
            let kind = if process.command.contains(" swarm ")
                || process.command.contains(" fanout ")
                || process.command.contains(" discuss ")
                || process.command.contains(" design ")
                || process.command.contains(" audit ")
            {
                "session"
            } else {
                "job"
            };
            let elapsed_ms = process.runtime_secs.saturating_mul(1000);
            let stuck = process.runtime_secs > 2 * 60;
            let dead = process.state == "Dead";
            processes.push(serde_json::json!({
                "kind": kind,
                "id": format!("untracked-{pid}"),
                "status": if dead { "reap-needed" } else if stuck { "stuck" } else { "running" },
                "pid": pid,
                "alive": !dead,
                "process_state": process.state,
                "cullable": stuck && !dead,
                "reap_required": dead,
                "reboot_required_if_uncullable": stuck && !dead,
                "agent": "swarm",
                "mode": "untracked",
                "cwd": "",
                "started_at_ms": serde_json::Value::Null,
                "elapsed_ms": elapsed_ms,
                "prompt_preview": process.command
            }));
        }
    }

    Ok(serde_json::json!({
        "schema": "agent-swarm/runtime-processes/v1",
        "processes": processes
    }))
}

struct SystemAgentProcess {
    pid: u32,
    command: String,
    runtime_secs: u64,
    state: String,
}

fn list_system_agent_swarm_pids() -> Vec<SystemAgentProcess> {
    let refresh = RefreshKind::new().with_processes(ProcessRefreshKind::everything());
    let mut system = System::new_with_specifics(refresh);
    system.refresh_processes();
    let mut pids = Vec::new();
    for (pid, process) in system.processes() {
        if let Some(pid_u32) = pid_to_u32(*pid) {
            let command = process.cmd().join(" ");
            let name = process.name().to_string();
            let haystack = format!("{name} {command}").to_ascii_lowercase();
            if is_runtime_agent_swarm_work_command(&haystack) {
                pids.push(SystemAgentProcess {
                    pid: pid_u32,
                    command,
                    runtime_secs: process.run_time(),
                    state: format!("{:?}", process.status()),
                });
            }
        }
    }
    pids
}

fn is_runtime_agent_swarm_work_command(haystack: &str) -> bool {
    if !haystack.contains("agent-swarm") {
        return false;
    }
    if haystack.contains("agent-swarm-tripwire") {
        return false;
    }
    let ignored = [
        " monitor",
        " monitor-",
        " manifest",
        " sessions",
        " status",
        " result",
        " alerts",
        " watch",
        " profiles",
        " hooks",
        " presets",
        " proposals",
        " propose",
        " proposal-vote",
        " recommend",
        " feedback",
        " runtime-processes",
        "runtime_processes",
    ];
    !ignored.iter().any(|needle| haystack.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_artifacts_json_from_dir_enumerates_present_files() {
        let dir = std::env::temp_dir().join(format!("agent-swarm-report-artifacts-{}", now_ms()));
        fs::create_dir_all(dir.join("layer-reports")).unwrap();
        fs::write(dir.join("session.json"), "{}\n").unwrap();
        fs::write(dir.join("events.jsonl"), "{}\n").unwrap();
        fs::write(dir.join("layer-reports/report.md"), "# report\n").unwrap();

        let payload = session_artifacts_json_from_dir("test-session", &dir).unwrap();
        let labels = payload["artifacts"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|artifact| artifact["label"].as_str())
            .collect::<std::collections::BTreeSet<_>>();

        assert!(labels.contains("metadata"));
        assert!(labels.contains("events"));
        assert!(labels.contains("layer-report"));
        assert!(!labels.contains("summary"));

        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn read_session_events_value_respects_tail_limit() {
        let dir = std::env::temp_dir().join(format!("agent-swarm-report-events-{}", now_ms()));
        fs::create_dir_all(&dir).unwrap();
        let lines = (0..20)
            .map(|index| serde_json::json!({"kind": "event", "index": index}).to_string())
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(dir.join("events.jsonl"), format!("{lines}\n")).unwrap();

        let events = read_session_events_value(&dir, 4).unwrap();
        let indexes = events
            .iter()
            .filter_map(|event| event["index"].as_i64())
            .collect::<Vec<_>>();
        assert_eq!(indexes, vec![16, 17, 18, 19]);

        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn runtime_processes_json_schema_field_is_present() {
        let payload = runtime_processes_json().unwrap();
        assert_eq!(payload["schema"], "agent-swarm/runtime-processes/v1");
        assert!(payload["processes"].is_array());
    }

    // ── Fixture-session contract tests (P5-S6 restore) ─────────────────────
    //
    // Originally lived in `tools/agent-swarm/rust/src/lib.rs mod tests` at
    // commit 298202ad. S5 deleted lib.rs without relocating these tests.
    // S6 restores them here in `swarm-mcp::report` where
    // `session_list_json_from_base` and `session_summary_json_from_dir` live.
    // The fixture files are at `tests/fixtures/session-store/` (restored in S6).

    /// Serializes JSON with sorted object keys while preserving arrays and nulls.
    ///
    /// `{"a":null}` and `{}` must remain distinct.
    fn canonical_json(value: &serde_json::Value) -> String {
        fn canonicalize(value: &serde_json::Value) -> serde_json::Value {
            match value {
                serde_json::Value::Object(map) => {
                    let ordered = map
                        .iter()
                        .map(|(key, value)| (key.clone(), canonicalize(value)))
                        .collect::<std::collections::BTreeMap<_, _>>();
                    serde_json::Value::Object(ordered.into_iter().collect())
                }
                serde_json::Value::Array(items) => {
                    serde_json::Value::Array(items.iter().map(canonicalize).collect())
                }
                _ => value.clone(),
            }
        }
        serde_json::to_string(&canonicalize(value)).unwrap()
    }

    fn object_keys(value: &serde_json::Value) -> std::collections::BTreeSet<String> {
        value
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>()
    }

    fn fixture_session_store_dir() -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/session-store")
    }

    fn fixture_session_dir() -> std::path::PathBuf {
        fixture_session_store_dir().join("session-fixture-completed")
    }

    #[test]
    fn fixture_session_list_contract_shape_is_stable() {
        let payload = session_list_json_from_base(&fixture_session_store_dir()).unwrap();
        assert_eq!(payload["schema"], "agent-swarm/session-list/v1");
        assert_eq!(
            object_keys(&payload),
            std::collections::BTreeSet::from(["schema".to_string(), "sessions".to_string()])
        );

        let sessions = payload["sessions"].as_array().unwrap();
        assert_eq!(sessions.len(), 1);
        let session = &sessions[0];
        assert_eq!(
            object_keys(session),
            std::collections::BTreeSet::from([
                "created_at_ms".to_string(),
                "id".to_string(),
                "prompt_preview".to_string(),
                "status".to_string(),
            ])
        );
        assert_eq!(session["id"], "session-fixture-completed");
        assert_eq!(session["status"], "completed");
        assert_eq!(session["created_at_ms"], 1780000000000_u64);
        assert_eq!(
            session["prompt_preview"],
            "Run a checked-in fixture review for session contracts."
        );

        assert_eq!(
            canonical_json(&payload),
            canonical_json(&serde_json::json!({
                "schema": "agent-swarm/session-list/v1",
                "sessions": [{
                    "id": "session-fixture-completed",
                    "status": "completed",
                    "created_at_ms": 1780000000000_u64,
                    "prompt_preview": "Run a checked-in fixture review for session contracts."
                }]
            }))
        );
    }

    #[test]
    fn fixture_session_summary_contract_shape_is_stable() {
        let payload =
            session_summary_json_from_dir("session-fixture-completed", &fixture_session_dir())
                .unwrap();
        assert_eq!(payload["schema"], "agent-swarm/session-summary/v1");
        assert_eq!(payload["session_id"], "session-fixture-completed");
        assert_eq!(payload["status"], "completed");
        assert_eq!(payload["created_at_ms"], 1780000000000_u64);
        assert_eq!(payload["updated_at_ms"], 1780000000300_u64);
        assert_eq!(payload["cwd"], "/tmp/swarm-fixture");
        assert_eq!(payload["manager"], "claude:sonnet");
        assert_eq!(payload["participants"].as_array().unwrap().len(), 1);
        assert!(payload["digest"]
            .as_str()
            .unwrap()
            .contains("Fixture digest"));
        assert!(payload["summary_preview"]
            .as_str()
            .unwrap()
            .contains("session summary fixture"));
        assert!(payload["docs_preview"]
            .as_str()
            .unwrap()
            .contains("session-summary contracts"));
        assert_eq!(
            object_keys(&payload),
            std::collections::BTreeSet::from([
                "artifacts".to_string(),
                "created_at_ms".to_string(),
                "cwd".to_string(),
                "digest".to_string(),
                "docs_preview".to_string(),
                "manager".to_string(),
                "participants".to_string(),
                "prompt_preview".to_string(),
                "recent_events".to_string(),
                "schema".to_string(),
                "session_id".to_string(),
                "status".to_string(),
                "summary_preview".to_string(),
                "updated_at_ms".to_string(),
            ])
        );

        let events = payload["recent_events"].as_array().unwrap();
        assert_eq!(events.len(), 3);
        assert_eq!(events.last().unwrap()["kind"], "session_completed");
        for event in events {
            assert_eq!(
                object_keys(event),
                std::collections::BTreeSet::from([
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
        }

        let artifacts = payload["artifacts"].as_array().unwrap();
        let labels = artifacts
            .iter()
            .filter_map(|artifact| artifact["label"].as_str())
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            labels,
            std::collections::BTreeSet::from([
                "api-docs",
                "digest",
                "events",
                "layer-report",
                "layer-reports",
                "metadata",
                "summary",
                "transcript",
            ])
        );
        for artifact in artifacts {
            assert_eq!(
                object_keys(artifact),
                std::collections::BTreeSet::from([
                    "bytes".to_string(),
                    "label".to_string(),
                    "mime".to_string(),
                    "path".to_string(),
                ])
            );
            assert!(artifact["bytes"].as_u64().unwrap() > 0);
            assert!(artifact["path"]
                .as_str()
                .unwrap()
                .contains("session-fixture-completed"));
        }
    }
}
