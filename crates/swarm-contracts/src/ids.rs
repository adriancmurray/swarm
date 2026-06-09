//! Typed identifier newtypes for the swarm runtime wire contract.
//!
//! Each newtype wraps a `String` and serializes transparently as a plain JSON
//! string (`#[serde(transparent)]`). This keeps persisted records byte-identical
//! with the pre-newtype stringly-typed values while adding compile-time safety.
//!
//! The wire contract: every `From<String>` / `From<&str>` is provided so call
//! sites can construct these without going through JSON. There is deliberately
//! no `From<&str>` that would make the construction visually identical to bare
//! string usage — callers always spell out the newtype name.

use std::fmt;

use serde::{Deserialize, Serialize};

// ── SessionId ─────────────────────────────────────────────────────────────────

/// Newtype wrapper for a discussion session identifier.
///
/// Stored in session metadata (`session.json`) and in every
/// `agent-swarm/event/v2` envelope as `session_id` and `run_id`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionId(String);

impl SessionId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl From<String> for SessionId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for SessionId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

// ── JobId ─────────────────────────────────────────────────────────────────────

/// Newtype wrapper for a job identifier.
///
/// Stored in `JobRecord.id` and used as the key for job files on disk.
/// Serialises transparently as a plain JSON string so existing persisted
/// records are byte-identical before and after this change.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct JobId(String);

impl JobId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for JobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl From<String> for JobId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for JobId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

// ── ProposalId ────────────────────────────────────────────────────────────────

/// Newtype wrapper for a telemetry proposal identifier.
///
/// Stored in `AgentProposal.id` and cross-referenced in
/// `AgentProposalVote.proposal_id`. Both structs serialize to/from disk
/// (`.jsonl`), so the transparent serde impl keeps on-disk bytes byte-identical.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProposalId(String);

impl ProposalId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ProposalId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl From<String> for ProposalId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for ProposalId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

// ── PresetId ──────────────────────────────────────────────────────────────────

/// Newtype wrapper for an orchestration preset identifier.
///
/// Identifies which named preset (e.g. `"architecture-council"`) to expand into
/// swarm or discussion orchestration. Arrives as a CLI argument or MCP tool
/// parameter and is matched against the known preset enum; it is never
/// persisted as a structured field on disk.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PresetId(String);

impl PresetId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PresetId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl From<String> for PresetId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for PresetId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_id_transparent_round_trip() {
        let id = JobId::from("job-abc123");
        let encoded = serde_json::to_string(&id).unwrap();
        assert_eq!(encoded, r#""job-abc123""#);
        let decoded: JobId = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, id);
    }

    #[test]
    fn session_id_transparent_round_trip() {
        let id = SessionId::from("session-deadbeef");
        let encoded = serde_json::to_string(&id).unwrap();
        assert_eq!(encoded, r#""session-deadbeef""#);
        let decoded: SessionId = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, id);
    }

    #[test]
    fn proposal_id_transparent_round_trip() {
        let id = ProposalId::from("proposal-abc");
        let encoded = serde_json::to_string(&id).unwrap();
        assert_eq!(encoded, r#""proposal-abc""#);
        let decoded: ProposalId = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, id);
    }

    #[test]
    fn preset_id_transparent_round_trip() {
        let id = PresetId::from("architecture-council");
        let encoded = serde_json::to_string(&id).unwrap();
        assert_eq!(encoded, r#""architecture-council""#);
        let decoded: PresetId = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, id);
    }

    #[test]
    fn all_id_types_display_match_inner_string() {
        assert_eq!(JobId::from("job-x").to_string(), "job-x");
        assert_eq!(SessionId::from("session-x").to_string(), "session-x");
        assert_eq!(ProposalId::from("proposal-x").to_string(), "proposal-x");
        assert_eq!(PresetId::from("preset-x").to_string(), "preset-x");
    }

    #[test]
    fn from_string_and_str_are_equivalent() {
        assert_eq!(JobId::from("x"), JobId::from("x".to_string()));
        assert_eq!(SessionId::from("x"), SessionId::from("x".to_string()));
        assert_eq!(ProposalId::from("x"), ProposalId::from("x".to_string()));
        assert_eq!(PresetId::from("x"), PresetId::from("x".to_string()));
    }
}
