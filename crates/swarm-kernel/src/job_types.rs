//! Typed discriminants for [`crate::job::JobRecord`]'s three stringly-typed
//! fields — re-exported from [`swarm_contracts::jobs`].
//!
//! All call sites using `crate::job_types::{JobStatus, JobAgent, JobMode}`
//! continue to work unchanged; the types are now the canonical swarm-contracts
//! definitions (single source of truth, Phase-2 cutover).
//!
//! # Wire contract (unchanged)
//!
//! Each unit variant serializes to its exact historical snake_case wire string.
//! `Other(String)` serializes as a bare JSON string (NOT `{"Other":"..."}`), so
//! unrecognized on-disk values survive the read-mutate-write cycle without data
//! loss. There is deliberately no `From<&str>` on any enum — `Other` construction
//! must stay visually loud so stringly call sites cannot creep back in.

pub use swarm_contracts::jobs::{JobAgent, JobMode, JobStatus};

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
}
