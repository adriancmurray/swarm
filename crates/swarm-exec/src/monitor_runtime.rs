//! Live observation runtime: the monitor poll loop and the filesystem watch
//! loop. `cmd_monitor`/`cmd_monitor_once`/`cmd_monitor_start`/`cmd_monitor_status`
//! drive the RSS / stale-activity sampler (`SwarmMonitor`), and `cmd_watch`
//! streams job/session/monitor store changes over NDJSON. Durable monitor
//! state lives in `monitor_store`; this module owns only the live timing,
//! process sampling, and watch wiring. Read-only alert display (`cmd_alerts`)
//! stays in `main.rs` as a CLI read command.

use std::collections::{HashMap, HashSet};
use std::env;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc as std_mpsc;
use std::thread;
use std::time::Duration;

use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use sysinfo::{ProcessRefreshKind, RefreshKind, System};

use swarm_kernel::args::parse_u64_arg;
use swarm_kernel::config::{load_config, SwarmConfig};
use swarm_kernel::format::json_text;
use swarm_kernel::process::{detach_background_command, pid_to_u32, process_is_alive};
use swarm_store::job::JobRecord;
use swarm_store::monitor_store::{
    append_monitor_alert, file_modified_ms, monitor_alerts_path, monitor_status_path,
    monitor_store_dir, read_monitor_pid, write_monitor_status, MonitorOptions,
    DEFAULT_MONITOR_INTERVAL_SECS, DEFAULT_MONITOR_RSS_MB, DEFAULT_MONITOR_SPIKE_FACTOR,
    DEFAULT_MONITOR_STALE_SECS,
};
use swarm_store::repos::job_repo::{FileJobRepo, JobRepo};
use swarm_store::store::{job_store_dir, now_ms, session_dir, session_store_dir};
use swarm_store::OsProcessLiveness;

use crate::session::list_sessions;
use crate::synthesis::preview_for_event;

// ==== extracted cluster body below (verbatim from main.rs seam 25) ====
#[derive(Debug, Clone)]
struct MonitorProcessState {
    baseline_bytes: u64,
    first_seen_ms: u128,
}

#[derive(Debug, Clone)]
struct AgentProcessSample {
    pid: u32,
    label: String,
    command: String,
    rss_bytes: u64,
}

pub fn cmd_monitor(raw: &[String]) -> Result<i32, String> {
    let options = parse_monitor_options(raw)?;
    let mut monitor = SwarmMonitor::new(options);
    loop {
        monitor.tick(false)?;
        thread::sleep(Duration::from_secs(monitor.options.interval_secs));
    }
}

pub fn cmd_monitor_once(raw: &[String]) -> Result<i32, String> {
    let options = parse_monitor_options(raw)?;
    let mut monitor = SwarmMonitor::new(options);
    monitor.tick(true)?;
    Ok(0)
}

pub fn cmd_monitor_start(raw: &[String]) -> Result<i32, String> {
    let start_options = parse_monitor_start_options(raw)?;
    let options = start_options.options;
    let dir = monitor_store_dir()?;
    fs::create_dir_all(&dir)
        .map_err(|err| format!("Error creating monitor directory {}: {err}", dir.display()))?;
    let mut replaced_pid = None;
    if let Some(pid) = read_monitor_pid().ok().flatten() {
        if process_is_alive(pid) {
            if start_options.replace {
                terminate_monitor_pid(pid);
                wait_for_monitor_exit(pid)?;
                replaced_pid = Some(pid);
            } else {
                println!(
                    "{}",
                    json_text(serde_json::json!({
                        "schema": "agent-swarm/monitor-start/v1",
                        "status": "already_running",
                        "pid": pid,
                        "alerts_path": monitor_alerts_path()?.display().to_string()
                    }))
                );
                return Ok(0);
            }
        }
    }

    let current_exe =
        env::current_exe().map_err(|err| format!("Error locating current executable: {err}"))?;
    let stdout_path = dir.join("monitor.stdout.log");
    let stderr_path = dir.join("monitor.stderr.log");
    let stdout = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&stdout_path)
        .map_err(|err| {
            format!(
                "Error opening monitor stdout {}: {err}",
                stdout_path.display()
            )
        })?;
    let stderr = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&stderr_path)
        .map_err(|err| {
            format!(
                "Error opening monitor stderr {}: {err}",
                stderr_path.display()
            )
        })?;
    let mut command = Command::new(current_exe);
    command
        .arg("monitor")
        .arg("--interval")
        .arg(options.interval_secs.to_string())
        .arg("--rss-mb")
        .arg((options.rss_threshold_bytes / 1024 / 1024).to_string())
        .arg("--spike-factor")
        .arg(options.spike_factor.to_string())
        .arg("--stale-secs")
        .arg(options.stale_secs.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    detach_background_command(&mut command);
    let child = command
        .spawn()
        .map_err(|err| format!("Error starting monitor sidecar: {err}"))?;
    let pid = child.id();
    write_monitor_status(pid, &options)?;
    let status = if replaced_pid.is_some() {
        "restarted"
    } else {
        "started"
    };
    println!(
        "{}",
        json_text(serde_json::json!({
            "schema": "agent-swarm/monitor-start/v1",
            "status": status,
            "pid": pid,
            "replaced_pid": replaced_pid,
            "alerts_path": monitor_alerts_path()?.display().to_string(),
            "stdout_path": stdout_path.display().to_string(),
            "stderr_path": stderr_path.display().to_string()
        }))
    );
    Ok(0)
}

#[derive(Debug, Clone)]
struct MonitorStartOptions {
    options: MonitorOptions,
    replace: bool,
}

fn parse_monitor_start_options(raw: &[String]) -> Result<MonitorStartOptions, String> {
    let mut replace = false;
    let mut monitor_args = Vec::with_capacity(raw.len());
    for part in raw {
        if part == "--replace" {
            replace = true;
        } else {
            monitor_args.push(part.clone());
        }
    }
    Ok(MonitorStartOptions {
        options: parse_monitor_options(&monitor_args)?,
        replace,
    })
}

#[cfg(unix)]
fn terminate_monitor_pid(pid: u32) {
    let _ = Command::new("/bin/kill")
        .arg("-TERM")
        .arg(pid.to_string())
        .status();
}

#[cfg(not(unix))]
fn terminate_monitor_pid(pid: u32) {
    let _ = Command::new("taskkill")
        .arg("/PID")
        .arg(pid.to_string())
        .arg("/T")
        .status();
}

fn wait_for_monitor_exit(pid: u32) -> Result<(), String> {
    for _ in 0..20 {
        if !process_is_alive(pid) {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(100));
    }
    Err(format!(
        "Error: existing monitor sidecar pid {pid} did not exit after --replace"
    ))
}

pub fn cmd_monitor_status() -> Result<i32, String> {
    let pid = read_monitor_pid()?.filter(|pid| process_is_alive(*pid));
    println!(
        "{}",
        json_text(serde_json::json!({
            "schema": "agent-swarm/monitor-status/v1",
            "running": pid.is_some(),
            "pid": pid,
            "alerts_path": monitor_alerts_path()?.display().to_string(),
            "status_path": monitor_status_path()?.display().to_string()
        }))
    );
    Ok(0)
}

pub fn cmd_watch(raw: &[String]) -> Result<i32, String> {
    let heartbeat_secs = parse_watch_heartbeat(raw)?;
    let watched_paths = prepare_watch_paths()?;
    let (tx, rx) = std_mpsc::channel();
    let mut watcher: RecommendedWatcher = notify::recommended_watcher(move |event| {
        let _ = tx.send(event);
    })
    .map_err(|err| format!("Error creating swarm watcher: {err}"))?;

    for path in &watched_paths {
        watcher
            .watch(path, RecursiveMode::Recursive)
            .map_err(|err| format!("Error watching {}: {err}", path.display()))?;
    }

    print_json_line(serde_json::json!({
        "schema": "agent-swarm/watch-event/v1",
        "ts_ms": now_ms(),
        "kind": "watch_started",
        "paths": watched_paths.iter().map(|path| path.display().to_string()).collect::<Vec<_>>(),
        "heartbeat_secs": heartbeat_secs
    }))?;

    loop {
        match rx.recv_timeout(Duration::from_secs(heartbeat_secs)) {
            Ok(Ok(event)) => {
                print_json_line(serde_json::json!({
                    "schema": "agent-swarm/watch-event/v1",
                    "ts_ms": now_ms(),
                    "kind": "store_changed",
                    "event_kind": format!("{:?}", event.kind),
                    "paths": event.paths.iter().map(|path| path.display().to_string()).collect::<Vec<_>>()
                }))?;
            }
            Ok(Err(err)) => {
                print_json_line(serde_json::json!({
                    "schema": "agent-swarm/watch-event/v1",
                    "ts_ms": now_ms(),
                    "kind": "watch_error",
                    "severity": "warning",
                    "message": err.to_string()
                }))?;
            }
            Err(std_mpsc::RecvTimeoutError::Timeout) => {
                print_json_line(serde_json::json!({
                    "schema": "agent-swarm/watch-event/v1",
                    "ts_ms": now_ms(),
                    "kind": "heartbeat",
                    "monitor_running": read_monitor_pid()?.map(process_is_alive).unwrap_or(false)
                }))?;
            }
            Err(std_mpsc::RecvTimeoutError::Disconnected) => {
                return Err("Error: swarm watcher channel disconnected".to_string());
            }
        }
    }
}

fn parse_watch_heartbeat(raw: &[String]) -> Result<u64, String> {
    let mut heartbeat_secs = 30u64;
    let mut index = 0;
    while index < raw.len() {
        match raw[index].as_str() {
            "--heartbeat" | "--heartbeat-secs" => {
                index += 1;
                heartbeat_secs = parse_u64_arg(raw.get(index), "heartbeat-secs")?;
            }
            other => return Err(format!("Error: unknown watch option `{other}`")),
        }
        index += 1;
    }
    Ok(heartbeat_secs.max(1))
}

fn prepare_watch_paths() -> Result<Vec<PathBuf>, String> {
    let paths = vec![job_store_dir()?, session_store_dir()?, monitor_store_dir()?];
    for path in &paths {
        fs::create_dir_all(path)
            .map_err(|err| format!("Error creating watch path {}: {err}", path.display()))?;
    }
    Ok(paths)
}

fn print_json_line(value: serde_json::Value) -> Result<(), String> {
    let encoded = serde_json::to_string(&value)
        .map_err(|err| format!("Error serializing watch event: {err}"))?;
    println!("{encoded}");
    io::stdout()
        .flush()
        .map_err(|err| format!("Error flushing watch event: {err}"))
}

struct SwarmMonitor {
    options: MonitorOptions,
    system: System,
    processes: HashMap<u32, MonitorProcessState>,
    emitted: HashSet<String>,
    last_heartbeat_ms: u128,
    /// `(label, command)` pairs for config-declared CLI backends, resolved
    /// once at startup so per-tick sampling stays cheap.
    configured_commands: Vec<(String, String)>,
}

impl SwarmMonitor {
    fn new(options: MonitorOptions) -> Self {
        let refresh = RefreshKind::new().with_processes(ProcessRefreshKind::everything());
        Self {
            options,
            system: System::new_with_specifics(refresh),
            processes: HashMap::new(),
            emitted: HashSet::new(),
            last_heartbeat_ms: 0,
            configured_commands: configured_agent_commands(&load_config()),
        }
    }

    fn tick(&mut self, force_heartbeat: bool) -> Result<(), String> {
        self.system.refresh_processes();
        let now = now_ms();
        let samples = self.agent_process_samples();
        self.processes
            .retain(|pid, _| samples.iter().any(|sample| sample.pid == *pid));

        let mut alerts = Vec::new();
        for sample in &samples {
            let state = self
                .processes
                .entry(sample.pid)
                .or_insert_with(|| MonitorProcessState {
                    baseline_bytes: sample.rss_bytes.max(1),
                    first_seen_ms: now,
                })
                .clone();
            let over_absolute = sample.rss_bytes >= self.options.rss_threshold_bytes;
            let over_spike =
                sample.rss_bytes as f64 >= state.baseline_bytes as f64 * self.options.spike_factor;
            if over_absolute && self.emit_once(format!("rss-high:{}", sample.pid)) {
                alerts.push(monitor_alert(
                    "rss_high",
                    "warning",
                    Some(sample),
                    format!(
                        "{} is using {} MB RSS",
                        sample.label,
                        sample.rss_bytes / 1024 / 1024
                    ),
                    serde_json::json!({
                        "rss_bytes": sample.rss_bytes,
                        "threshold_bytes": self.options.rss_threshold_bytes,
                        "baseline_bytes": state.baseline_bytes,
                        "age_ms": now.saturating_sub(state.first_seen_ms)
                    }),
                ));
            }
            if over_absolute && over_spike && self.emit_once(format!("rss-spike:{}", sample.pid)) {
                alerts.push(monitor_alert(
                    "rss_spike",
                    "critical",
                    Some(sample),
                    format!(
                        "{} RSS grew from {} MB to {} MB",
                        sample.label,
                        state.baseline_bytes / 1024 / 1024,
                        sample.rss_bytes / 1024 / 1024
                    ),
                    serde_json::json!({
                        "rss_bytes": sample.rss_bytes,
                        "baseline_bytes": state.baseline_bytes,
                        "spike_factor": self.options.spike_factor,
                        "threshold_bytes": self.options.rss_threshold_bytes,
                        "age_ms": now.saturating_sub(state.first_seen_ms)
                    }),
                ));
            }
        }

        alerts.extend(self.stale_job_alerts()?);
        alerts.extend(self.stale_session_alerts()?);

        if force_heartbeat || now.saturating_sub(self.last_heartbeat_ms) >= 30_000 {
            self.last_heartbeat_ms = now;
            alerts.push(serde_json::json!({
                "schema": "agent-swarm/monitor-alert/v1",
                "ts_ms": now,
                "kind": "heartbeat",
                "severity": "info",
                "source": "agent-swarm-monitor",
                "message": "Agent Swarm monitor alive.",
                "data": {
                    "processes": samples.len(),
                    "interval_secs": self.options.interval_secs,
                    "rss_threshold_bytes": self.options.rss_threshold_bytes,
                    "stale_secs": self.options.stale_secs
                }
            }));
        }

        for alert in alerts {
            append_monitor_alert(&alert)?;
            if alert.get("kind").and_then(|value| value.as_str()) != Some("heartbeat") {
                eprintln!(
                    "agent-swarm monitor: {}",
                    alert
                        .get("message")
                        .and_then(|value| value.as_str())
                        .unwrap_or("alert")
                );
            }
        }
        Ok(())
    }

    fn emit_once(&mut self, key: String) -> bool {
        self.emitted.insert(key)
    }

    fn agent_process_samples(&self) -> Vec<AgentProcessSample> {
        let self_pid = std::process::id();
        self.system
            .processes()
            .iter()
            .filter_map(|(pid, process)| {
                let pid_u32 = pid_to_u32(*pid)?;
                if pid_u32 == self_pid {
                    return None;
                }
                let command = process.cmd().join(" ");
                let name = process.name().to_string();
                let label = agent_process_label(&name, &command, &self.configured_commands)?;
                Some(AgentProcessSample {
                    pid: pid_u32,
                    label,
                    command,
                    rss_bytes: process.memory(),
                })
            })
            .collect()
    }

    fn stale_job_alerts(&mut self) -> Result<Vec<serde_json::Value>, String> {
        let now = now_ms();
        let stale_ms = u128::from(self.options.stale_secs) * 1000;
        let mut alerts = Vec::new();
        let job_repo = FileJobRepo::new(job_store_dir()?);
        let _ = job_repo.reconcile_liveness(&OsProcessLiveness);
        for record in job_repo.list().map_err(|e| e.to_string())? {
            if !matches!(record.status.as_str(), "queued" | "running") {
                continue;
            }
            let latest = latest_job_activity_ms(&record);
            if now.saturating_sub(latest) <= stale_ms {
                continue;
            }
            if self.emit_once(format!("stale-job:{}", record.id)) {
                alerts.push(serde_json::json!({
                    "schema": "agent-swarm/monitor-alert/v1",
                    "ts_ms": now,
                    "kind": "stale_job",
                    "severity": "warning",
                    "source": "agent-swarm-monitor",
                    "job_id": record.id,
                    "pid": record.pid,
                    "message": format!("Job has had no output/status activity for {}s.", self.options.stale_secs),
                    "data": {
                        "agent": record.agent,
                        "mode": record.mode,
                        "status": record.status,
                        "latest_activity_ms": latest,
                        "stale_secs": self.options.stale_secs
                    }
                }));
            }
        }
        Ok(alerts)
    }

    fn stale_session_alerts(&mut self) -> Result<Vec<serde_json::Value>, String> {
        let now = now_ms();
        let stale_ms = u128::from(self.options.stale_secs) * 1000;
        let mut alerts = Vec::new();
        for session in list_sessions()? {
            if !matches!(session.status.as_str(), "running" | "created") {
                continue;
            }
            let dir = session_dir(&session.id)?;
            let latest = file_modified_ms(&dir.join("events.jsonl"))
                .or_else(|| file_modified_ms(&dir.join("transcript.md")))
                .unwrap_or(session.created_at_ms);
            if now.saturating_sub(latest) <= stale_ms {
                continue;
            }
            if self.emit_once(format!("stale-session:{}", session.id)) {
                alerts.push(serde_json::json!({
                    "schema": "agent-swarm/monitor-alert/v1",
                    "ts_ms": now,
                    "kind": "stale_session",
                    "severity": "warning",
                    "source": "agent-swarm-monitor",
                    "session_id": session.id,
                    "message": format!("Session has had no event activity for {}s.", self.options.stale_secs),
                    "data": {
                        "status": session.status,
                        "latest_activity_ms": latest,
                        "stale_secs": self.options.stale_secs
                    }
                }));
            }
        }
        Ok(alerts)
    }
}

fn parse_monitor_options(raw: &[String]) -> Result<MonitorOptions, String> {
    let mut interval_secs =
        env_u64("AGENT_SWARM_MONITOR_INTERVAL_SECS").unwrap_or(DEFAULT_MONITOR_INTERVAL_SECS);
    let mut rss_mb = env_u64("AGENT_SWARM_MONITOR_RSS_MB").unwrap_or(DEFAULT_MONITOR_RSS_MB);
    let mut spike_factor =
        env_f64("AGENT_SWARM_MONITOR_SPIKE_FACTOR").unwrap_or(DEFAULT_MONITOR_SPIKE_FACTOR);
    let mut stale_secs =
        env_u64("AGENT_SWARM_MONITOR_STALE_SECS").unwrap_or(DEFAULT_MONITOR_STALE_SECS);
    let mut index = 0;
    while index < raw.len() {
        match raw[index].as_str() {
            "--interval" | "--interval-secs" => {
                index += 1;
                interval_secs = parse_u64_arg(raw.get(index), "interval")?;
            }
            "--rss-mb" => {
                index += 1;
                rss_mb = parse_u64_arg(raw.get(index), "rss-mb")?;
            }
            "--spike-factor" => {
                index += 1;
                spike_factor = raw
                    .get(index)
                    .ok_or_else(|| "Error: --spike-factor requires a value".to_string())?
                    .parse::<f64>()
                    .map_err(|_| "Error: --spike-factor must be a number".to_string())?;
            }
            "--stale-secs" => {
                index += 1;
                stale_secs = parse_u64_arg(raw.get(index), "stale-secs")?;
            }
            other => return Err(format!("Error: unknown monitor option `{other}`")),
        }
        index += 1;
    }
    Ok(MonitorOptions {
        interval_secs: interval_secs.max(1),
        rss_threshold_bytes: rss_mb.max(1) * 1024 * 1024,
        spike_factor: spike_factor.max(1.1),
        stale_secs: stale_secs.max(30),
    })
}

fn env_u64(name: &str) -> Option<u64> {
    env::var(name).ok()?.parse::<u64>().ok()
}

fn env_f64(name: &str) -> Option<f64> {
    env::var(name).ok()?.parse::<f64>().ok()
}

/// Built-in CLI agent invocations the monitor recognizes without any config:
/// `(label, process name, consult-mode command substring)`. Config-declared
/// backends contribute their descriptor `command` via
/// `configured_agent_commands` and are matched in `agent_process_label`.
const BUILTIN_AGENT_PATTERNS: &[(&str, &str, &str)] = &[
    ("claude", "claude", "claude --print"),
    ("codex", "codex", "codex exec"),
];

/// Collect `(label, command)` pairs for backends declared in config, so the
/// monitor detects the same commands the dispatch registry can run. The label
/// is the backend id; the match key is the descriptor's `command` (only the
/// `cli` kind carries one).
fn configured_agent_commands(config: &SwarmConfig) -> Vec<(String, String)> {
    config
        .backend
        .iter()
        .filter_map(|(id, descriptor)| {
            descriptor
                .command
                .as_ref()
                .map(|command| (id.clone(), command.to_ascii_lowercase()))
        })
        .collect()
}

fn agent_process_label(
    name: &str,
    command: &str,
    configured: &[(String, String)],
) -> Option<String> {
    let haystack = format!("{name} {command}").to_ascii_lowercase();
    if haystack.contains("agent-swarm monitor") {
        return None;
    }
    for (label, process_name, command_fragment) in BUILTIN_AGENT_PATTERNS {
        if haystack.contains(command_fragment) || name == *process_name {
            return Some((*label).to_string());
        }
    }
    let lower_name = name.to_ascii_lowercase();
    for (label, agent_command) in configured {
        let file_name = Path::new(agent_command)
            .file_name()
            .and_then(|file_name| file_name.to_str())
            .unwrap_or(agent_command);
        if lower_name == file_name || haystack.contains(agent_command.as_str()) {
            return Some(label.clone());
        }
    }
    if haystack.contains("agent-swarm") {
        return Some("agent-swarm".to_string());
    }
    None
}

fn monitor_alert(
    kind: &str,
    severity: &str,
    sample: Option<&AgentProcessSample>,
    message: String,
    data: serde_json::Value,
) -> serde_json::Value {
    serde_json::json!({
        "schema": "agent-swarm/monitor-alert/v1",
        "ts_ms": now_ms(),
        "kind": kind,
        "severity": severity,
        "source": "agent-swarm-monitor",
        "pid": sample.map(|sample| sample.pid),
        "agent": sample.map(|sample| sample.label.as_str()),
        "command": sample.map(|sample| preview_for_event(&sample.command, 240)),
        "message": message,
        "data": data
    })
}

fn latest_job_activity_ms(record: &JobRecord) -> u128 {
    [
        file_modified_ms(Path::new(&record.stdout_path)),
        file_modified_ms(Path::new(&record.stderr_path)),
        file_modified_ms(Path::new(&record.result_path)),
    ]
    .into_iter()
    .flatten()
    .max()
    .unwrap_or(record.started_at_ms.unwrap_or(record.created_at_ms))
}

#[cfg(test)]
mod tests {
    use super::{
        agent_process_label, configured_agent_commands, parse_monitor_options,
        parse_monitor_start_options,
    };
    use swarm_kernel::backend_descriptor::BackendDescriptor;
    use swarm_kernel::config::SwarmConfig;

    // Tests pass explicit flags, which override the env-derived base values, so
    // these assertions are independent of any `AGENT_SWARM_MONITOR_*` env vars
    // that may be set in the test environment. No sleep / spawn / sysinfo here —
    // the live monitor loop and sampler are not unit-testable in isolation.
    fn args(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|part| part.to_string()).collect()
    }

    #[test]
    fn interval_clamps_to_at_least_one() {
        let opts = parse_monitor_options(&args(&["--interval", "0"])).unwrap();
        assert_eq!(opts.interval_secs, 1);
    }

    #[test]
    fn rss_mb_clamps_to_at_least_one_megabyte() {
        let opts = parse_monitor_options(&args(&["--rss-mb", "0"])).unwrap();
        assert_eq!(opts.rss_threshold_bytes, 1024 * 1024);
    }

    #[test]
    fn spike_factor_clamps_to_floor() {
        let opts = parse_monitor_options(&args(&["--spike-factor", "1.0"])).unwrap();
        assert!((opts.spike_factor - 1.1).abs() < 1e-9);
    }

    #[test]
    fn stale_secs_clamps_to_at_least_thirty() {
        let opts = parse_monitor_options(&args(&["--stale-secs", "10"])).unwrap();
        assert_eq!(opts.stale_secs, 30);
    }

    #[test]
    fn unknown_flag_is_rejected_and_named() {
        let err = parse_monitor_options(&args(&["--bogus"])).unwrap_err();
        assert!(
            err.contains("unknown monitor option"),
            "unexpected error: {err}"
        );
        assert!(err.contains("bogus"), "error should name the flag: {err}");
    }

    #[test]
    fn monitor_start_accepts_replace_without_passing_it_to_monitor_loop() {
        let opts = parse_monitor_start_options(&args(&[
            "--replace",
            "--rss-mb",
            "8192",
            "--interval",
            "2",
        ]))
        .unwrap();
        assert!(opts.replace);
        assert_eq!(opts.options.rss_threshold_bytes, 8192 * 1024 * 1024);
        assert_eq!(opts.options.interval_secs, 2);
    }

    #[test]
    fn missing_flag_value_is_rejected() {
        let err = parse_monitor_options(&args(&["--interval"])).unwrap_err();
        assert!(err.contains("requires a value"), "unexpected error: {err}");
    }

    #[test]
    fn builtin_agents_are_detected_by_name_or_command() {
        let none: &[(String, String)] = &[];
        assert_eq!(
            agent_process_label("claude", "", none).as_deref(),
            Some("claude")
        );
        assert_eq!(
            agent_process_label("node", "claude --print hello", none).as_deref(),
            Some("claude")
        );
        assert_eq!(
            agent_process_label("sh", "codex exec --json", none).as_deref(),
            Some("codex")
        );
    }

    #[test]
    fn own_monitor_process_is_excluded_and_swarm_binary_labeled() {
        let none: &[(String, String)] = &[];
        assert_eq!(
            agent_process_label("agent-swarm", "agent-swarm monitor --interval 5", none),
            None
        );
        assert_eq!(
            agent_process_label("agent-swarm", "agent-swarm run --quiet task", none).as_deref(),
            Some("agent-swarm")
        );
    }

    #[test]
    fn unrelated_process_is_not_labeled() {
        let none: &[(String, String)] = &[];
        assert_eq!(
            agent_process_label("Safari", "/Applications/Safari", none),
            None
        );
    }

    #[test]
    fn config_declared_commands_are_detected_with_backend_id_label() {
        let configured = vec![("my-agent".to_string(), "some-agent-cli".to_string())];
        assert_eq!(
            agent_process_label("some-agent-cli", "", &configured).as_deref(),
            Some("my-agent")
        );
        assert_eq!(
            agent_process_label("sh", "some-agent-cli --print task", &configured).as_deref(),
            Some("my-agent")
        );
        assert_eq!(
            agent_process_label("other", "irrelevant", &configured),
            None
        );
    }

    #[test]
    fn configured_agent_commands_take_cli_descriptor_commands() {
        let mut config = SwarmConfig::default();
        config.backend.insert(
            "my-agent".to_string(),
            BackendDescriptor {
                command: Some("Some-Agent-CLI".to_string()),
                ..BackendDescriptor::default()
            },
        );
        config
            .backend
            .insert("remote".to_string(), BackendDescriptor::default());

        let commands = configured_agent_commands(&config);
        assert_eq!(
            commands,
            vec![("my-agent".to_string(), "some-agent-cli".to_string())]
        );
    }
}
