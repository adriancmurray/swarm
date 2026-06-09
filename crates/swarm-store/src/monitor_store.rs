//! Durable monitor state and alert storage helpers.
//!
//! Moved from agent-swarm's `monitor_store.rs` in P5-S2.
//! Inlines three leaf utilities that stay in agent-swarm:
//!   - `home_dir`          (from resolver.rs — other fns there have agent deps)
//!   - `json_text`         (from format.rs — depends on job::JobRecord)
//!   - `process_is_alive`  (from process.rs — 6 other callers in staying files)

use std::fs::{self, remove_file, File, OpenOptions};
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use crate::process_helpers::process_is_alive;
use crate::store::{
    now_ms, read_text_tail, swarm_home, swarm_home_err, write_text_atomic, MAX_ARTIFACT_TEXT_BYTES,
};

pub const DEFAULT_MONITOR_INTERVAL_SECS: u64 = 5;
pub const DEFAULT_MONITOR_RSS_MB: u64 = 4 * 1024;
pub const DEFAULT_MONITOR_SPIKE_FACTOR: f64 = 2.5;
pub const DEFAULT_MONITOR_STALE_SECS: u64 = 300;
const MONITOR_ALERT_LIMIT: usize = 2_000;

#[derive(Debug, Clone)]
pub struct MonitorOptions {
    pub interval_secs: u64,
    pub rss_threshold_bytes: u64,
    pub spike_factor: f64,
    pub stale_secs: u64,
}

/// Serialize a `serde_json::Value` to pretty-printed JSON.
/// Inlined from agent-swarm's `format::json_text` (that module depends on JobRecord).
fn json_text(value: serde_json::Value) -> String {
    serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string())
}

pub fn monitor_store_dir() -> Result<PathBuf, String> {
    let home = swarm_home().ok_or_else(swarm_home_err)?;
    Ok(home.join("monitor"))
}

pub fn monitor_alerts_path() -> Result<PathBuf, String> {
    Ok(monitor_store_dir()?.join("alerts.ndjson"))
}

pub fn monitor_status_path() -> Result<PathBuf, String> {
    Ok(monitor_store_dir()?.join("monitor.json"))
}

pub fn write_monitor_status(pid: u32, options: &MonitorOptions) -> Result<(), String> {
    write_text_atomic(
        &monitor_status_path()?,
        format!(
            "{}\n",
            json_text(serde_json::json!({
                "schema": "agent-swarm/monitor-status/v1",
                "pid": pid,
                "started_at_ms": now_ms(),
                "interval_secs": options.interval_secs,
                "rss_threshold_bytes": options.rss_threshold_bytes,
                "spike_factor": options.spike_factor,
                "stale_secs": options.stale_secs,
                "alerts_path": monitor_alerts_path()?.display().to_string()
            }))
        ),
    )
}

pub fn read_monitor_pid() -> Result<Option<u32>, String> {
    let path = monitor_status_path()?;
    let Ok(text) = fs::read_to_string(&path) else {
        return Ok(None);
    };
    decode_monitor_pid_from_text(&text, &path)
}

fn decode_monitor_pid_from_text(text: &str, path: &Path) -> Result<Option<u32>, String> {
    let decoded = serde_json::from_str::<serde_json::Value>(text)
        .map_err(|err| format!("Error parsing monitor status {}: {err}", path.display()))?;
    Ok(decoded
        .get("pid")
        .and_then(|value| value.as_u64())
        .and_then(|value| u32::try_from(value).ok()))
}

pub fn append_monitor_alert(alert: &serde_json::Value) -> Result<(), String> {
    let path = monitor_alerts_path()?;
    rotate_monitor_alerts_if_needed(&path)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| {
            format!(
                "Error creating monitor directory {}: {err}",
                parent.display()
            )
        })?;
    }
    let encoded = serde_json::to_string(alert)
        .map_err(|err| format!("Error serializing monitor alert: {err}"))?;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|err| format!("Error opening monitor alerts {}: {err}", path.display()))?;
    writeln!(file, "{encoded}")
        .map_err(|err| format!("Error writing monitor alerts {}: {err}", path.display()))
}

pub fn alerts_json(since_ts_ms: Option<u128>, limit: usize) -> Result<serde_json::Value, String> {
    alerts_json_from_path(&monitor_alerts_path()?, since_ts_ms, limit)
}

fn alerts_json_from_path(
    path: &Path,
    since_ts_ms: Option<u128>,
    limit: usize,
) -> Result<serde_json::Value, String> {
    let text = read_text_tail(path, MAX_ARTIFACT_TEXT_BYTES).unwrap_or_default();
    let mut alerts = text
        .lines()
        .rev()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|alert| {
            since_ts_ms
                .map(|since| {
                    alert
                        .get("ts_ms")
                        .and_then(|value| value.as_u64())
                        .map(u128::from)
                        .unwrap_or_default()
                        >= since
                })
                .unwrap_or(true)
        })
        .take(limit)
        .collect::<Vec<_>>();
    alerts.reverse();
    Ok(serde_json::json!({
        "schema": "agent-swarm/monitor-alerts/v1",
        "path": path.display().to_string(),
        "running": read_monitor_pid()?.map(process_is_alive).unwrap_or(false),
        "alerts": alerts
    }))
}

pub fn file_modified_ms(path: &Path) -> Option<u128> {
    fs::metadata(path)
        .ok()?
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_millis())
}

fn rotate_monitor_alerts_if_needed(path: &Path) -> Result<(), String> {
    let Ok(file) = File::open(path) else {
        return Ok(());
    };
    let line_count = io::BufReader::new(file).lines().count();
    if line_count <= MONITOR_ALERT_LIMIT {
        return Ok(());
    }
    let rotated = path.with_extension("ndjson.1");
    let _ = remove_file(&rotated);
    fs::rename(path, &rotated)
        .map_err(|err| format!("Error rotating monitor alerts {}: {err}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_modified_ms_returns_some_for_existing_file() {
        let dir = std::env::temp_dir().join(format!(
            "agent-swarm-monitor-store-test-{}",
            std::process::id()
        ));
        let path = dir.join("file.txt");
        fs::create_dir_all(&dir).unwrap();
        fs::write(&path, "hello").unwrap();

        assert!(file_modified_ms(&path).is_some());
        assert!(file_modified_ms(&dir.join("missing.txt")).is_none());

        let _ = fs::remove_file(path);
        let _ = fs::remove_dir(dir);
    }

    #[test]
    fn decode_monitor_pid_reads_valid_pid_and_rejects_invalid_json() {
        let path = Path::new("monitor.json");

        assert_eq!(
            decode_monitor_pid_from_text(r#"{"pid":42}"#, path).unwrap(),
            Some(42)
        );
        assert_eq!(
            decode_monitor_pid_from_text(r#"{"status":"missing"}"#, path).unwrap(),
            None
        );
        assert!(decode_monitor_pid_from_text("not-json", path).is_err());
    }

    #[test]
    fn alerts_json_from_path_filters_since_and_preserves_order() {
        let dir = std::env::temp_dir().join(format!(
            "agent-swarm-monitor-alerts-test-{}",
            std::process::id()
        ));
        let path = dir.join("alerts.ndjson");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            &path,
            concat!(
                "{\"ts_ms\":1,\"kind\":\"old\"}\n",
                "{\"ts_ms\":3,\"kind\":\"new\"}\n",
                "not-json\n",
                "{\"ts_ms\":5,\"kind\":\"newer\"}\n"
            ),
        )
        .unwrap();

        let payload = alerts_json_from_path(&path, Some(3), 10).unwrap();
        let alerts = payload["alerts"].as_array().unwrap();

        assert_eq!(payload["schema"], "agent-swarm/monitor-alerts/v1");
        assert_eq!(alerts.len(), 2);
        assert_eq!(alerts[0]["kind"], "new");
        assert_eq!(alerts[1]["kind"], "newer");

        let _ = fs::remove_file(path);
        let _ = fs::remove_dir(dir);
    }
}
