//! Typed discriminants and record for job persistence.
//!
//! # Wire contract
//!
//! Each unit variant serializes to its exact historical snake_case wire string.
//! `Other(String)` serializes as a bare JSON string (NOT `{"Other":"..."}`),
//! so unrecognized on-disk values survive the read-mutate-write cycle without
//! data loss. There is deliberately no `From<&str>` on any enum — `Other`
//! construction must stay visually loud so stringly call sites cannot creep
//! back in.
//!
//! `JobRecord` is persisted as pretty-printed JSON (`serde_json::to_string_pretty`
//! plus a trailing newline). The lockbox replay tests use compact JSON for structural
//! equivalence checks only — production serialization is pretty-printed.

use serde::de;
use std::fmt;

use crate::ids::JobId;

/// Default job timeout in seconds (matches `agent-swarm::DEFAULT_TIMEOUT_SECS`).
pub const DEFAULT_TIMEOUT_SECS: u64 = 300;

// ── JobStatus ─────────────────────────────────────────────────────────────────

/// Status field of a [`JobRecord`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobStatus {
    Running,
    Queued,
    Completed,
    Failed,
    Lost,
    Cancelled,
    TimedOut,
    /// Forward-compatibility catch-all. Carries the raw wire string verbatim.
    Other(String),
}

impl JobStatus {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Running => "running",
            Self::Queued => "queued",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Lost => "lost",
            Self::Cancelled => "cancelled",
            Self::TimedOut => "timed_out",
            Self::Other(s) => s.as_str(),
        }
    }
}

impl serde::Serialize for JobStatus {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

impl<'de> serde::Deserialize<'de> for JobStatus {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> de::Visitor<'de> for V {
            type Value = JobStatus;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a job status string")
            }
            fn visit_str<E: de::Error>(self, s: &str) -> Result<JobStatus, E> {
                Ok(match s {
                    "running" => JobStatus::Running,
                    "queued" => JobStatus::Queued,
                    "completed" => JobStatus::Completed,
                    "failed" => JobStatus::Failed,
                    "lost" => JobStatus::Lost,
                    "cancelled" => JobStatus::Cancelled,
                    "timed_out" => JobStatus::TimedOut,
                    other => JobStatus::Other(other.to_string()),
                })
            }
        }
        d.deserialize_str(V)
    }
}

impl fmt::Display for JobStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

// ── JobAgent ──────────────────────────────────────────────────────────────────

/// Agent field of a [`JobRecord`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobAgent {
    Gemini,
    Claude,
    Codex,
    Auto,
    /// Synthetic label for swarm/discussion tracking records.
    Swarm,
    /// Forward-compatibility catch-all. Carries the raw wire string verbatim.
    Other(String),
}

impl JobAgent {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Gemini => "gemini",
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Auto => "auto",
            Self::Swarm => "swarm",
            Self::Other(s) => s.as_str(),
        }
    }

    /// Convert from an agent-name string. No `From<&str>` to keep `Other`
    /// construction visually loud.
    pub fn from_agent_name(name: &str) -> Self {
        match name {
            "gemini" => Self::Gemini,
            "claude" => Self::Claude,
            "codex" => Self::Codex,
            "auto" => Self::Auto,
            "swarm" => Self::Swarm,
            other => Self::Other(other.to_string()),
        }
    }
}

impl serde::Serialize for JobAgent {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

impl<'de> serde::Deserialize<'de> for JobAgent {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> de::Visitor<'de> for V {
            type Value = JobAgent;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a job agent string")
            }
            fn visit_str<E: de::Error>(self, s: &str) -> Result<JobAgent, E> {
                Ok(match s {
                    "gemini" => JobAgent::Gemini,
                    "claude" => JobAgent::Claude,
                    "codex" => JobAgent::Codex,
                    "auto" => JobAgent::Auto,
                    "swarm" => JobAgent::Swarm,
                    other => JobAgent::Other(other.to_string()),
                })
            }
        }
        d.deserialize_str(V)
    }
}

impl fmt::Display for JobAgent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

// ── JobMode ───────────────────────────────────────────────────────────────────

/// Mode field of a [`JobRecord`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobMode {
    Agent,
    Consult,
    Swarm,
    Discussion,
    /// Forward-compatibility catch-all. Carries the raw wire string verbatim.
    Other(String),
}

impl JobMode {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Agent => "agent",
            Self::Consult => "consult",
            Self::Swarm => "swarm",
            Self::Discussion => "discussion",
            Self::Other(s) => s.as_str(),
        }
    }

    /// Convert from a wire string. No `From<&str>` to keep `Other` loud.
    pub fn from_wire_str(s: &str) -> Self {
        match s {
            "agent" => Self::Agent,
            "consult" => Self::Consult,
            "swarm" => Self::Swarm,
            "discussion" => Self::Discussion,
            other => Self::Other(other.to_string()),
        }
    }
}

impl serde::Serialize for JobMode {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

impl<'de> serde::Deserialize<'de> for JobMode {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> de::Visitor<'de> for V {
            type Value = JobMode;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a job mode string")
            }
            fn visit_str<E: de::Error>(self, s: &str) -> Result<JobMode, E> {
                Ok(match s {
                    "agent" => JobMode::Agent,
                    "consult" => JobMode::Consult,
                    "swarm" => JobMode::Swarm,
                    "discussion" => JobMode::Discussion,
                    other => JobMode::Other(other.to_string()),
                })
            }
        }
        d.deserialize_str(V)
    }
}

impl fmt::Display for JobMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

// ── JobRecord ─────────────────────────────────────────────────────────────────

/// A persisted job record.
///
/// Stored as a JSON file named `{id}.json` in the job store directory.
/// Production serialization is `serde_json::to_string_pretty` + trailing `\n`.
///
/// # Wire compat notes
///
/// - `timeout_secs` uses `#[serde(default = "default_timeout_secs")]` so
///   legacy records without this field parse as `DEFAULT_TIMEOUT_SECS`.
/// - `allow_recursive_codex` uses `#[serde(default)]` for the same reason.
/// - All other `Option<T>` fields serialize as `null` when `None`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct JobRecord {
    pub id: JobId,
    pub status: JobStatus,
    pub agent: JobAgent,
    pub model: Option<String>,
    pub mode: JobMode,
    pub cwd: String,
    pub prompt_preview: String,
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
    pub created_at_ms: u128,
    pub started_at_ms: Option<u128>,
    pub completed_at_ms: Option<u128>,
    pub pid: Option<u32>,
    pub exit_code: Option<i32>,
    pub prompt_path: String,
    pub stdout_path: String,
    pub stderr_path: String,
    pub result_path: String,
    #[serde(default)]
    pub allow_recursive_codex: bool,
}

fn default_timeout_secs() -> u64 {
    DEFAULT_TIMEOUT_SECS
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_status_wire_strings_are_stable() {
        let cases = [
            (JobStatus::Running, "running"),
            (JobStatus::Queued, "queued"),
            (JobStatus::Completed, "completed"),
            (JobStatus::Failed, "failed"),
            (JobStatus::Lost, "lost"),
            (JobStatus::Cancelled, "cancelled"),
            (JobStatus::TimedOut, "timed_out"),
        ];
        for (variant, wire) in cases {
            assert_eq!(variant.as_str(), wire, "as_str mismatch for {wire:?}");
            let json = format!("\"{wire}\"");
            assert_eq!(
                serde_json::to_string(&variant).unwrap(),
                json,
                "serialize mismatch for {wire:?}"
            );
            assert_eq!(
                serde_json::from_str::<JobStatus>(&json).unwrap(),
                variant,
                "deserialize mismatch for {wire:?}"
            );
        }
    }

    #[test]
    fn job_status_other_round_trips_byte_identically() {
        let wire = "\"some_future_state\"";
        let decoded: JobStatus = serde_json::from_str(wire).unwrap();
        assert_eq!(decoded, JobStatus::Other("some_future_state".into()));
        assert_eq!(serde_json::to_string(&decoded).unwrap(), wire);
    }

    #[test]
    fn job_agent_wire_strings_are_stable() {
        let cases = [
            (JobAgent::Gemini, "gemini"),
            (JobAgent::Claude, "claude"),
            (JobAgent::Codex, "codex"),
            (JobAgent::Auto, "auto"),
            (JobAgent::Swarm, "swarm"),
        ];
        for (variant, wire) in cases {
            assert_eq!(variant.as_str(), wire, "as_str mismatch for {wire:?}");
            let json = format!("\"{wire}\"");
            assert_eq!(
                serde_json::to_string(&variant).unwrap(),
                json,
                "serialize mismatch for {wire:?}"
            );
            assert_eq!(
                serde_json::from_str::<JobAgent>(&json).unwrap(),
                variant,
                "deserialize mismatch for {wire:?}"
            );
        }
    }

    #[test]
    fn job_agent_other_round_trips_byte_identically() {
        let wire = "\"unknown_bot\"";
        let decoded: JobAgent = serde_json::from_str(wire).unwrap();
        assert_eq!(decoded, JobAgent::Other("unknown_bot".into()));
        assert_eq!(serde_json::to_string(&decoded).unwrap(), wire);
    }

    #[test]
    fn job_mode_wire_strings_are_stable() {
        let cases = [
            (JobMode::Agent, "agent"),
            (JobMode::Consult, "consult"),
            (JobMode::Swarm, "swarm"),
            (JobMode::Discussion, "discussion"),
        ];
        for (variant, wire) in cases {
            assert_eq!(variant.as_str(), wire, "as_str mismatch for {wire:?}");
            let json = format!("\"{wire}\"");
            assert_eq!(
                serde_json::to_string(&variant).unwrap(),
                json,
                "serialize mismatch for {wire:?}"
            );
            assert_eq!(
                serde_json::from_str::<JobMode>(&json).unwrap(),
                variant,
                "deserialize mismatch for {wire:?}"
            );
        }
    }

    #[test]
    fn job_mode_other_round_trips_byte_identically() {
        let wire = "\"fanout_v2\"";
        let decoded: JobMode = serde_json::from_str(wire).unwrap();
        assert_eq!(decoded, JobMode::Other("fanout_v2".into()));
        assert_eq!(serde_json::to_string(&decoded).unwrap(), wire);
    }

    // ── Lockbox-replay: JobRecord wire-equivalence proof ──────────────────────
    //
    // These JSON strings are drawn from the golden fixtures in agent-swarm's
    // test suite (job.rs tests) and prove structural wire-equivalence.
    // JobRecord production serialization is pretty-printed, so compact JSON
    // is used here for fixture-style structural checks (field presence, values).
    //
    // The `Other` round-trip tests prove that unknown future values survive the
    // read-mutate-write cycle without data loss.

    #[test]
    fn lockbox_job_record_running_parses_correctly() {
        let json = r#"{
            "id": "job-test-123",
            "status": "running",
            "agent": "gemini",
            "model": "flash",
            "mode": "agent",
            "cwd": "/tmp",
            "prompt_preview": "hello",
            "timeout_secs": 450,
            "created_at_ms": 100000,
            "started_at_ms": 100005,
            "completed_at_ms": null,
            "pid": 9999,
            "exit_code": null,
            "prompt_path": "/tmp/job-test-123.prompt.md",
            "stdout_path": "/tmp/job-test-123.stdout.log",
            "stderr_path": "/tmp/job-test-123.stderr.log",
            "result_path": "/tmp/job-test-123.result.txt",
            "allow_recursive_codex": true
        }"#;
        let record: JobRecord = serde_json::from_str(json).unwrap();
        assert_eq!(record.id, JobId::from("job-test-123"));
        assert_eq!(record.status, JobStatus::Running);
        assert_eq!(record.agent, JobAgent::Gemini);
        assert_eq!(record.mode, JobMode::Agent);
        assert_eq!(record.timeout_secs, 450);
        assert!(record.allow_recursive_codex);
        // Structural round-trip: re-parse re-serialized output should match.
        let re_encoded = serde_json::to_string(&record).unwrap();
        let re_decoded: JobRecord = serde_json::from_str(&re_encoded).unwrap();
        assert_eq!(record, re_decoded);
    }

    #[test]
    fn lockbox_job_record_legacy_defaults() {
        // Legacy record without timeout_secs or allow_recursive_codex must parse
        // with DEFAULT_TIMEOUT_SECS and false respectively.
        let legacy_json = r#"{
            "id": "job-test-legacy",
            "status": "running",
            "agent": "gemini",
            "model": null,
            "mode": "agent",
            "cwd": "/tmp",
            "prompt_preview": "hello",
            "created_at_ms": 100000,
            "started_at_ms": null,
            "completed_at_ms": null,
            "pid": null,
            "exit_code": null,
            "prompt_path": "/tmp/job.prompt.md",
            "stdout_path": "/tmp/job.stdout.log",
            "stderr_path": "/tmp/job.stderr.log",
            "result_path": "/tmp/job.result.txt"
        }"#;
        let legacy: JobRecord = serde_json::from_str(legacy_json).unwrap();
        assert_eq!(legacy.timeout_secs, DEFAULT_TIMEOUT_SECS);
        assert!(!legacy.allow_recursive_codex);
        assert_eq!(legacy.status, JobStatus::Running);
        assert_eq!(legacy.agent, JobAgent::Gemini);
        assert_eq!(legacy.mode, JobMode::Agent);
    }

    #[test]
    fn lockbox_job_record_swarm_discussion_round_trips() {
        // swarm agent + discussion mode must round-trip without data loss.
        let swarm_json = r#"{
            "id": "job-test-swarm",
            "status": "queued",
            "agent": "swarm",
            "model": null,
            "mode": "discussion",
            "cwd": "/tmp",
            "prompt_preview": "council task",
            "timeout_secs": 450,
            "created_at_ms": 200000,
            "started_at_ms": null,
            "completed_at_ms": null,
            "pid": null,
            "exit_code": null,
            "prompt_path": "/tmp/job2.prompt.md",
            "stdout_path": "/tmp/job2.stdout.log",
            "stderr_path": "/tmp/job2.stderr.log",
            "result_path": "/tmp/job2.result.txt"
        }"#;
        let record: JobRecord = serde_json::from_str(swarm_json).unwrap();
        assert_eq!(record.agent, JobAgent::Swarm);
        assert_eq!(record.status, JobStatus::Queued);
        assert_eq!(record.mode, JobMode::Discussion);
        let re_encoded = serde_json::to_string(&record).unwrap();
        assert!(re_encoded.contains("\"swarm\""));
        assert!(re_encoded.contains("\"queued\""));
        assert!(re_encoded.contains("\"discussion\""));
    }

    #[test]
    fn lockbox_job_record_unknown_fields_round_trip_via_other() {
        // Unknown status/agent values must round-trip via Other without data loss.
        let future_json = r#"{
            "id": "job-test-future",
            "status": "some_future_state",
            "agent": "unknown_bot",
            "model": null,
            "mode": "agent",
            "cwd": "/tmp",
            "prompt_preview": "future task",
            "timeout_secs": 300,
            "created_at_ms": 300000,
            "started_at_ms": null,
            "completed_at_ms": null,
            "pid": null,
            "exit_code": null,
            "prompt_path": "/tmp/job3.prompt.md",
            "stdout_path": "/tmp/job3.stdout.log",
            "stderr_path": "/tmp/job3.stderr.log",
            "result_path": "/tmp/job3.result.txt"
        }"#;
        let record: JobRecord = serde_json::from_str(future_json).unwrap();
        assert_eq!(record.status, JobStatus::Other("some_future_state".into()));
        assert_eq!(record.agent, JobAgent::Other("unknown_bot".into()));
        let re_encoded = serde_json::to_string(&record).unwrap();
        assert!(re_encoded.contains("\"some_future_state\""));
        assert!(re_encoded.contains("\"unknown_bot\""));
    }
}
