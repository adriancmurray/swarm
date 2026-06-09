//! Telemetry wire types for the swarm runtime.
//!
//! These structs are persisted as JSONL lines in the telemetry store:
//! - `AgentObservation` → `observations.jsonl`
//! - `AgentFeedback` → `feedback.jsonl`
//! - `AgentProposal` → `proposals.jsonl`
//! - `AgentProposalVote` → `votes.jsonl`
//!
//! # Wire contract
//!
//! - `input_tokens` and `output_tokens` on `AgentObservation` use
//!   `#[serde(default, skip_serializing_if = "Option::is_none")]` so the
//!   common case (Gemini/Codex, or any parse failure) does not emit these
//!   fields. Do NOT remove the `skip_serializing_if` — doing so diverges the
//!   wire for existing observations.

use crate::ids::ProposalId;

/// A single agent run observation appended to `observations.jsonl`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AgentObservation {
    pub schema: String,
    pub ts_ms: u128,
    pub mode: String,
    pub session_id: Option<String>,
    pub role: String,
    pub agent: String,
    pub cwd: String,
    pub status: String,
    pub exit_code: i32,
    pub timed_out: bool,
    pub duration_ms: u128,
    pub prompt_bytes: usize,
    pub stdout_bytes: usize,
    pub stderr_bytes: usize,
    /// LLM input token count (Claude only; `None` for Gemini/Codex or parse failure).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    /// LLM output token count (Claude only; `None` for Gemini/Codex or parse failure).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
}

/// A routing feedback record appended to `feedback.jsonl`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AgentFeedback {
    pub schema: String,
    pub ts_ms: u128,
    pub session_id: Option<String>,
    pub role: String,
    pub agent: String,
    pub outcome: String,
    pub note: Option<String>,
    pub weight: f64,
}

/// A telemetry proposal appended to `proposals.jsonl`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AgentProposal {
    pub schema: String,
    pub id: ProposalId,
    pub ts_ms: u128,
    pub session_id: Option<String>,
    pub title: String,
    pub body: String,
    pub proposed_by: String,
    pub status: String,
    pub tags: Vec<String>,
}

/// A proposal vote appended to `votes.jsonl`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AgentProposalVote {
    pub schema: String,
    pub ts_ms: u128,
    pub proposal_id: ProposalId,
    pub voter: String,
    pub vote: String,
    pub rationale: Option<String>,
    pub weight: f64,
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Wire-equivalence proof: `AgentObservation` without token fields (Gemini run).
    const FIXTURE_OBSERVATION_GEMINI: &str = r#"{"schema":"agent-swarm/observation/v1","ts_ms":1780000001000,"mode":"agent","session_id":null,"role":"worker","agent":"gemini","cwd":"/tmp","status":"completed","exit_code":0,"timed_out":false,"duration_ms":5000,"prompt_bytes":120,"stdout_bytes":800,"stderr_bytes":0}"#;

    /// Wire-equivalence proof: `AgentObservation` with token fields (Claude run).
    const FIXTURE_OBSERVATION_CLAUDE: &str = r#"{"schema":"agent-swarm/observation/v1","ts_ms":1780000002000,"mode":"agent","session_id":"session-abc","role":"manager","agent":"claude","cwd":"/tmp","status":"completed","exit_code":0,"timed_out":false,"duration_ms":12000,"prompt_bytes":240,"stdout_bytes":1600,"stderr_bytes":32,"input_tokens":1500,"output_tokens":600}"#;

    /// Wire-equivalence proof: `AgentProposal` round-trip.
    const FIXTURE_PROPOSAL: &str = r#"{"schema":"agent-swarm/proposal/v1","id":"proposal-x1","ts_ms":1780000003000,"session_id":"session-abc","title":"Use typed contracts","body":"Replace bare strings with newtypes.","proposed_by":"claude","status":"open","tags":["contracts","typify"]}"#;

    /// Wire-equivalence proof: `AgentProposalVote` round-trip.
    const FIXTURE_VOTE: &str = r#"{"schema":"agent-swarm/vote/v1","ts_ms":1780000004000,"proposal_id":"proposal-x1","voter":"gemini","vote":"win","rationale":"Strong pattern.","weight":1.0}"#;

    #[test]
    fn lockbox_observation_gemini_no_tokens_parses() {
        let obs: AgentObservation = serde_json::from_str(FIXTURE_OBSERVATION_GEMINI)
            .expect("gemini observation fixture must parse");
        assert_eq!(obs.agent, "gemini");
        assert!(obs.input_tokens.is_none());
        assert!(obs.output_tokens.is_none());
    }

    #[test]
    fn lockbox_observation_gemini_no_tokens_byte_identical() {
        // With skip_serializing_if, None token fields must be absent from output.
        let obs: AgentObservation = serde_json::from_str(FIXTURE_OBSERVATION_GEMINI).unwrap();
        let re_encoded = serde_json::to_string(&obs).unwrap();
        assert_eq!(
            re_encoded, FIXTURE_OBSERVATION_GEMINI,
            "AgentObservation (no tokens) must re-serialize byte-identically"
        );
    }

    #[test]
    fn lockbox_observation_claude_with_tokens_byte_identical() {
        let obs: AgentObservation = serde_json::from_str(FIXTURE_OBSERVATION_CLAUDE)
            .expect("claude observation fixture must parse");
        assert_eq!(obs.agent, "claude");
        assert_eq!(obs.input_tokens, Some(1500));
        assert_eq!(obs.output_tokens, Some(600));
        let re_encoded = serde_json::to_string(&obs).unwrap();
        assert_eq!(
            re_encoded, FIXTURE_OBSERVATION_CLAUDE,
            "AgentObservation (with tokens) must re-serialize byte-identically"
        );
    }

    #[test]
    fn lockbox_proposal_byte_identical() {
        let proposal: AgentProposal =
            serde_json::from_str(FIXTURE_PROPOSAL).expect("proposal fixture must parse");
        assert_eq!(proposal.id, ProposalId::from("proposal-x1"));
        assert_eq!(proposal.status, "open");
        let re_encoded = serde_json::to_string(&proposal).unwrap();
        assert_eq!(
            re_encoded, FIXTURE_PROPOSAL,
            "AgentProposal must re-serialize byte-identically"
        );
    }

    #[test]
    fn lockbox_vote_byte_identical() {
        let vote: AgentProposalVote =
            serde_json::from_str(FIXTURE_VOTE).expect("vote fixture must parse");
        assert_eq!(vote.proposal_id, ProposalId::from("proposal-x1"));
        assert_eq!(vote.vote, "win");
        let re_encoded = serde_json::to_string(&vote).unwrap();
        assert_eq!(
            re_encoded, FIXTURE_VOTE,
            "AgentProposalVote must re-serialize byte-identically"
        );
    }
}
