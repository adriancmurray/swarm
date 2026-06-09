//! `TelemetryRepo` trait.
//!
//! Moved from `agent-swarm::repos::telemetry_repo` in P5-S1 (trait only;
//! `FileTelemetryRepo`, `MemTelemetryRepo`, and `default_file_telemetry_repo`
//! stay in `agent-swarm` until P5-S2).
//!
//! All four telemetry types (`AgentObservation`, `AgentFeedback`, `AgentProposal`,
//! `AgentProposalVote`) are `pub use swarm_contracts::telemetry::*` shims in
//! `agent-swarm` — they are the canonical swarm-contracts types imported directly here.

use swarm_contracts::telemetry::{
    AgentFeedback, AgentObservation, AgentProposal, AgentProposalVote,
};

use crate::error::RepoError;

// ── TelemetryRepo trait ───────────────────────────────────────────────────────

/// Append-only storage for agent telemetry: observations, feedback,
/// proposals, and votes.
///
/// All write methods append an entry; reads return the full collection in
/// append order (FIFO).
pub trait TelemetryRepo: Send + Sync {
    fn record_observation(&self, obs: AgentObservation) -> Result<(), RepoError>;
    fn record_feedback(&self, fb: AgentFeedback) -> Result<(), RepoError>;
    fn record_proposal(&self, prop: AgentProposal) -> Result<(), RepoError>;
    fn record_proposal_vote(&self, vote: AgentProposalVote) -> Result<(), RepoError>;

    fn observations(&self) -> Result<Vec<AgentObservation>, RepoError>;
    fn feedback(&self) -> Result<Vec<AgentFeedback>, RepoError>;
    fn proposals(&self) -> Result<Vec<AgentProposal>, RepoError>;
    fn proposal_votes(&self) -> Result<Vec<AgentProposalVote>, RepoError>;
}
