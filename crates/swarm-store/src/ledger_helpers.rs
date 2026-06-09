//! Ledger `_in_dir` helpers — append-only NDJSON task log.
//!
//! Mirrors `telemetry_helpers`: pure file-I/O with no agent-swarm-local
//! dependencies. `FileLedgerRepo` uses these directly. The current-state fold
//! (latest snapshot per id) lives in `swarm_core::fold_tasks`; these
//! helpers only append and read raw snapshots.

use std::fs::{self, OpenOptions};
use std::io::Write;

use swarm_core::LedgerTask;

pub(crate) fn record_task_in_dir(dir: &std::path::Path, task: LedgerTask) -> Result<(), String> {
    fs::create_dir_all(dir)
        .map_err(|err| format!("Error creating ledger directory {}: {err}", dir.display()))?;
    let encoded = serde_json::to_string(&task)
        .map_err(|err| format!("Error serializing ledger task: {err}"))?;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("tasks.jsonl"))
        .map_err(|err| format!("Error opening ledger log: {err}"))?;
    writeln!(file, "{encoded}").map_err(|err| format!("Error writing ledger log: {err}"))
}

pub(crate) fn read_tasks_in_dir(dir: &std::path::Path) -> Result<Vec<LedgerTask>, String> {
    let path = dir.join("tasks.jsonl");
    let Ok(text) = fs::read_to_string(&path) else {
        return Ok(Vec::new());
    };
    Ok(text
        .lines()
        .filter_map(|line| serde_json::from_str::<LedgerTask>(line).ok())
        .collect())
}
