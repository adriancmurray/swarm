//! Low-level store primitives shared across session, job, and report code.
//!
//! `EVENT_LOG_LOCK` serializes NDJSON appends. `ATOMIC_WRITE_COUNTER` provides
//! two guarantees from one global source: monotonic sequence IDs in event
//! envelopes and unique temp-file nonces in `write_text_atomic`.
//!
//! # Encapsulation invariant
//!
//! The three statics are `pub(crate)` — private to this crate. They are NOT
//! re-exported by `swarm_store::lib`. All lock acquisition happens inside
//! this crate's `event_repo` and `session_repo` modules. ci/law-checks.sh
//! CHECK 7 asserts this structurally.

use std::fs::{self, remove_file, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Mutex, OnceLock,
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub(crate) static EVENT_LOG_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

/// Shared nonce for `write_text_atomic` temp-file names.
///
/// Monotonically increasing; provides uniqueness for temp paths but is NOT
/// used for event sequence numbers (see `EVENT_SEQ_COUNTER`).
pub(crate) static ATOMIC_WRITE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Dedicated sequence counter for `agent-swarm/event/v2` `.seq` fields.
///
/// Separate from `ATOMIC_WRITE_COUNTER` so that event sequence numbers are
/// contiguous — no gaps from temp-file nonce increments interleaving between
/// event appends.
pub(crate) static EVENT_SEQ_COUNTER: AtomicU64 = AtomicU64::new(0);

pub const MAX_SESSION_EVENTS_TAIL_BYTES: usize = 512 * 1024;
pub const MAX_ARTIFACT_TEXT_BYTES: usize = 2 * 1024 * 1024;

/// Maximum serialized event line size before the payload is compacted.
/// Moved from agent-swarm lib.rs (was `const MAX_EVENT_LINE_BYTES`).
pub(crate) const MAX_EVENT_LINE_BYTES: usize = 128 * 1024;

/// Resolve `$HOME`. Inlined from agent-swarm's `resolver::home_dir` to avoid
/// a back-dep. resolver.rs has other functions that depend on agent-swarm types.
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Resolve the swarm data home — the single root for all runtime data.
///
/// `$SWARM_HOME` (an absolute path) wins when set; otherwise `$HOME/.swarm`.
/// Returns `None` only when neither env var is available.
///
/// Data layout under the home:
///
/// ```text
/// <swarm home>/
///   jobs/                    background job records + captured outputs
///   sessions/                discussion session metadata, events, transcripts
///   monitor/                 sidecar monitor heartbeat + alert history
///   ledger/                  append-only task ledger snapshots
///   telemetry/               agent outcome observations + feedback
///   evals/                   eval run summaries + scorecards
///   providers/               native-backend provider registry
///   conductor-sessions/      conductor activity records (records.jsonl per session)
///   conductor-policy.json    spawn-policy document
///   bin/                     installed binaries (e.g. agent-swarm-mcp)
/// ```
pub fn swarm_home() -> Option<PathBuf> {
    if let Some(value) = std::env::var_os("SWARM_HOME") {
        if !value.is_empty() {
            return Some(PathBuf::from(value));
        }
    }
    home_dir().map(|home| home.join(".swarm"))
}

/// Resolve the provider-registry data directory: `<swarm home>/providers`.
///
/// Single source of truth shared by the native backend and the `provider`
/// CLI so both always operate on the same registry.
pub fn providers_dir() -> Option<PathBuf> {
    swarm_home().map(|home| home.join("providers"))
}

/// Resolve the skills directory: `<swarm home>/skills`.
///
/// The home (user-global) source of `SKILL.md` skills. A native run also
/// consults a project-local `<cwd>/.swarm/skills` that overrides this one.
pub fn skills_dir() -> Option<PathBuf> {
    swarm_home().map(|home| home.join("skills"))
}

/// Shared "cannot resolve the data root" error for `swarm_home()` callers.
pub fn swarm_home_err() -> String {
    "Error: cannot resolve the swarm home (set SWARM_HOME or HOME)".to_string()
}

pub fn read_text_tail(path: &Path, max_bytes: usize) -> Result<String, String> {
    let mut file = File::open(path)
        .map_err(|err| format!("Error opening text file {}: {err}", path.display()))?;
    let len = file
        .metadata()
        .map_err(|err| format!("Error reading text file metadata {}: {err}", path.display()))?
        .len() as usize;
    let start = len.saturating_sub(max_bytes);
    file.seek(SeekFrom::Start(start as u64))
        .map_err(|err| format!("Error seeking text file {}: {err}", path.display()))?;
    let mut text = String::new();
    file.read_to_string(&mut text)
        .map_err(|err| format!("Error reading text file {}: {err}", path.display()))?;
    if start > 0 {
        if let Some(next_line) = text.find('\n') {
            text = text[next_line + 1..].to_string();
        }
    }
    Ok(text)
}

/// Validates ids before joining them into store paths.
/// Accepts only ASCII letters, digits, `_`, and `-`; rejects separators and dots.
pub fn validate_store_id(id: &str) -> Result<(), String> {
    if id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        Ok(())
    } else {
        Err(format!("Error: invalid id `{id}`"))
    }
}

pub fn job_store_dir() -> Result<PathBuf, String> {
    let home = swarm_home().ok_or_else(swarm_home_err)?;
    Ok(home.join("jobs"))
}

pub fn session_store_dir() -> Result<PathBuf, String> {
    let home = swarm_home().ok_or_else(swarm_home_err)?;
    Ok(home.join("sessions"))
}

pub fn session_dir(id: &str) -> Result<PathBuf, String> {
    validate_store_id(id)?;
    Ok(session_store_dir()?.join(id))
}

pub fn new_job_id() -> swarm_contracts::ids::JobId {
    swarm_contracts::ids::JobId::from(format!("job-{:x}-{}", now_ms(), std::process::id()))
}

pub fn new_session_id() -> swarm_contracts::ids::SessionId {
    // session-{hex_ms}-{pid}-{counter}
    // Counter suffix avoids collisions when two sessions are created in the
    // same millisecond (common in tests). The id remains OPAQUE.
    let counter = ATOMIC_WRITE_COUNTER.fetch_add(1, Ordering::SeqCst);
    swarm_contracts::ids::SessionId::from(format!(
        "session-{:x}-{}-{}",
        now_ms(),
        std::process::id(),
        counter
    ))
}

pub fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| Duration::from_secs(0))
        .as_millis()
}

/// Writes `contents` to `path` atomically via a sibling temp file.
pub fn write_text_atomic(path: &Path, contents: impl AsRef<[u8]>) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| format!("Error creating directory {}: {err}", parent.display()))?;
    }
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .ok_or_else(|| {
            format!(
                "Error writing atomic file with no file name: {}",
                path.display()
            )
        })?
        .to_string_lossy();
    let nonce = ATOMIC_WRITE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp = parent.join(format!(
        ".{file_name}.{}.{}.{}.tmp",
        std::process::id(),
        now_ms(),
        nonce
    ));
    fs::write(&tmp, contents.as_ref())
        .map_err(|err| format!("Error writing atomic temp file {}: {err}", tmp.display()))?;
    fs::rename(&tmp, path).map_err(|err| {
        let _ = remove_file(&tmp);
        format!("Error replacing file {}: {err}", path.display())
    })
}

// ── Tests recovered from agent-swarm lib.rs (P5-S6) ─────────────────────────
//
// Originally lived in `tools/agent-swarm/rust/src/lib.rs mod tests` at
// commit 298202ad. S5 deleted lib.rs; the atomic-write test was not
// relocated. S6 restores it here in `swarm-store::store` where
// `write_text_atomic` and `now_ms` now live.
#[cfg(test)]
mod tests {
    use super::*;

    /// Exercises the `swarm_home()` resolver against tempdirs only — never
    /// the real `~/.swarm`. Both branches run inside ONE test so the
    /// process-global SWARM_HOME/HOME mutations cannot race a sibling test.
    #[test]
    fn swarm_home_prefers_env_override_then_falls_back_to_home_dot_swarm() {
        let previous_swarm_home = std::env::var_os("SWARM_HOME");
        let previous_home = std::env::var_os("HOME");
        let override_dir = std::env::temp_dir().join(format!("test-swarm-home-{}", now_ms()));
        let home_dir = std::env::temp_dir().join(format!("test-home-{}", now_ms()));

        // SWARM_HOME set → used verbatim; subdirs hang directly off it.
        std::env::set_var("SWARM_HOME", &override_dir);
        assert_eq!(swarm_home(), Some(override_dir.clone()));
        assert_eq!(job_store_dir().unwrap(), override_dir.join("jobs"));
        assert_eq!(session_store_dir().unwrap(), override_dir.join("sessions"));
        assert_eq!(providers_dir(), Some(override_dir.join("providers")));
        assert_eq!(skills_dir(), Some(override_dir.join("skills")));

        // SWARM_HOME unset → $HOME/.swarm.
        std::env::remove_var("SWARM_HOME");
        std::env::set_var("HOME", &home_dir);
        assert_eq!(swarm_home(), Some(home_dir.join(".swarm")));
        assert_eq!(
            session_store_dir().unwrap(),
            home_dir.join(".swarm/sessions")
        );

        match previous_swarm_home {
            Some(value) => std::env::set_var("SWARM_HOME", value),
            None => std::env::remove_var("SWARM_HOME"),
        }
        match previous_home {
            Some(value) => std::env::set_var("HOME", value),
            None => std::env::remove_var("HOME"),
        }
    }

    #[test]
    fn atomic_write_replaces_file_without_leaving_temp_artifacts() {
        let temp_dir = std::env::temp_dir().join(format!("test-atomic-write-{}", now_ms()));
        fs::create_dir_all(&temp_dir).unwrap();
        let path = temp_dir.join("session.json");

        write_text_atomic(&path, "{\"id\":\"one\"}\n").unwrap();
        write_text_atomic(&path, "{\"id\":\"two\"}\n").unwrap();

        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content, "{\"id\":\"two\"}\n");
        let leftovers = fs::read_dir(&temp_dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .count();
        assert_eq!(leftovers, 0);

        fs::remove_dir_all(temp_dir).ok();
    }
}
