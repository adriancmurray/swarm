//! Aggregated swarm overview assembler.
//!
//! Returns a single `agent-swarm/overview/v1` digest by merging:
//!   - recent/running sessions from `session_list_json`
//!   - running/recent jobs from `list_job_records` (via `runtime_processes_json`)
//!   - active monitor alerts from `alerts_json`
//!
//! Intended for dashboards and meta-conductors that want one call instead of
//! polling three endpoints separately.

use swarm_core::JobRepo;
use swarm_kernel::process::process_is_alive;
use swarm_store::monitor_store::{alerts_json, read_monitor_pid};
use swarm_store::store::job_store_dir;
use swarm_store::store::now_ms;
use swarm_store::FileJobRepo;
use swarm_store::OsProcessLiveness;

use crate::report::session_list_json;

/// Assembles and returns the `agent-swarm/overview/v1` digest.
pub fn overview_json() -> Result<serde_json::Value, String> {
    let generated_at_ms = now_ms();

    // Sessions: pull the full recent list (up to 20), then classify into
    // running vs recent-completed buckets.
    let session_list = session_list_json()?;
    let all_sessions = session_list["sessions"]
        .as_array()
        .cloned()
        .unwrap_or_default();

    let running_sessions: Vec<_> = all_sessions
        .iter()
        .filter(|s| {
            matches!(
                s["status"].as_str().unwrap_or(""),
                "running" | "incomplete" | "lost"
            )
        })
        .cloned()
        .collect();

    let recent_sessions: Vec<_> = all_sessions.iter().take(5).cloned().collect();

    // Jobs: load, refresh staleness, bucket by status.
    let job_repo = FileJobRepo::new(job_store_dir().map_err(|e| e.to_string())?);
    let _ = job_repo.reconcile_liveness(&OsProcessLiveness);
    let jobs = job_repo.list().map_err(|e| e.to_string())?;
    let running_jobs: Vec<_> = jobs
        .iter()
        .filter(|j| matches!(j.status.as_str(), "queued" | "running" | "lost"))
        .map(|j| {
            serde_json::json!({
                "id": j.id,
                "status": j.status,
                "agent": j.agent,
                "mode": j.mode,
                "cwd": j.cwd,
                "pid": j.pid,
                "alive": j.pid.map(process_is_alive).unwrap_or(false),
                "started_at_ms": j.started_at_ms,
                "elapsed_ms": j.started_at_ms.map(|s| now_ms().saturating_sub(s)),
                "prompt_preview": j.prompt_preview,
            })
        })
        .collect();

    let mut recent_jobs: Vec<_> = jobs
        .iter()
        .map(|j| {
            serde_json::json!({
                "id": j.id,
                "status": j.status,
                "agent": j.agent,
                "mode": j.mode,
                "created_at_ms": j.created_at_ms,
                "prompt_preview": j.prompt_preview,
            })
        })
        .collect();
    // Sort descending by creation time, keep the 5 most recent.
    recent_jobs.sort_by(|a, b| {
        let ta = a["created_at_ms"].as_u64().unwrap_or(0);
        let tb = b["created_at_ms"].as_u64().unwrap_or(0);
        tb.cmp(&ta)
    });
    recent_jobs.truncate(5);

    // Alerts: last 10 from the monitor store; include monitor running status.
    let alerts_payload = alerts_json(None, 10)?;
    let active_alerts = alerts_payload["alerts"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let monitor_running = alerts_payload["running"].as_bool().unwrap_or(false);

    // Monitor process liveness (redundant double-check if monitor_running is already set, but
    // guards against a stale monitor.json that hasn't been GC'd yet).
    let monitor_pid = read_monitor_pid().ok().flatten();
    let monitor_alive = monitor_pid.map(process_is_alive).unwrap_or(false);

    // Summary counts.
    let counts = serde_json::json!({
        "running_sessions": running_sessions.len(),
        "recent_sessions": recent_sessions.len(),
        "running_jobs": running_jobs.len(),
        "recent_jobs": recent_jobs.len(),
        "active_alerts": active_alerts.len(),
        "monitor_running": monitor_running || monitor_alive,
    });

    Ok(serde_json::json!({
        "schema": "agent-swarm/overview/v1",
        "generated_at_ms": generated_at_ms,
        "counts": counts,
        "running_sessions": running_sessions,
        "recent_sessions": recent_sessions,
        "running_jobs": running_jobs,
        "recent_jobs": recent_jobs,
        "active_alerts": active_alerts,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn object_keys(value: &serde_json::Value) -> BTreeSet<String> {
        value
            .as_object()
            .unwrap()
            .keys()
            .map(|k| k.to_string())
            .collect()
    }

    #[test]
    fn overview_json_schema_and_shape_are_stable() {
        // Works against whatever state the local store has (empty or live).
        let payload = overview_json().unwrap();

        assert_eq!(payload["schema"], "agent-swarm/overview/v1");
        assert!(payload["generated_at_ms"].is_number());
        assert_eq!(
            object_keys(&payload),
            BTreeSet::from([
                "schema".to_string(),
                "generated_at_ms".to_string(),
                "counts".to_string(),
                "running_sessions".to_string(),
                "recent_sessions".to_string(),
                "running_jobs".to_string(),
                "recent_jobs".to_string(),
                "active_alerts".to_string(),
            ])
        );

        // counts block has exactly the six keys we expect.
        assert_eq!(
            object_keys(&payload["counts"]),
            BTreeSet::from([
                "running_sessions".to_string(),
                "recent_sessions".to_string(),
                "running_jobs".to_string(),
                "recent_jobs".to_string(),
                "active_alerts".to_string(),
                "monitor_running".to_string(),
            ])
        );
        assert!(payload["counts"]["running_sessions"].is_number());
        assert!(payload["counts"]["recent_sessions"].is_number());
        assert!(payload["counts"]["running_jobs"].is_number());
        assert!(payload["counts"]["recent_jobs"].is_number());
        assert!(payload["counts"]["active_alerts"].is_number());
        assert!(payload["counts"]["monitor_running"].is_boolean());

        // Arrays are arrays (even if empty).
        assert!(payload["running_sessions"].is_array());
        assert!(payload["recent_sessions"].is_array());
        assert!(payload["running_jobs"].is_array());
        assert!(payload["recent_jobs"].is_array());
        assert!(payload["active_alerts"].is_array());
    }
}
