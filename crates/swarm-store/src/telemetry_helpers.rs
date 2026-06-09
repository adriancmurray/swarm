//! Telemetry `_in_dir` helpers — private copy from agent-swarm's `telemetry.rs`.
//!
//! These are pure file-I/O functions with no agent-swarm-local dependencies.
//! `FileTelemetryRepo` uses them directly; the public telemetry API in
//! agent-swarm's `telemetry.rs` continues to use its own copy.
//!
//! # Sync note
//!
//! Verbatim copy of the 8 `*_in_dir` fns from
//! `tools/agent-swarm/rust/src/telemetry.rs`. If those change, update here too.
//! P5-S5 will move these into swarm-contracts as the canonical location.

use std::fs::{self, OpenOptions};
use std::io::Write;

use swarm_contracts::telemetry::{
    AgentFeedback, AgentObservation, AgentProposal, AgentProposalVote,
};

pub(crate) fn record_observation_in_dir(
    dir: &std::path::Path,
    observation: AgentObservation,
) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|err| {
        format!(
            "Error creating telemetry directory {}: {err}",
            dir.display()
        )
    })?;
    let encoded = serde_json::to_string(&observation)
        .map_err(|err| format!("Error serializing telemetry observation: {err}"))?;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("observations.jsonl"))
        .map_err(|err| format!("Error opening telemetry log: {err}"))?;
    writeln!(file, "{encoded}").map_err(|err| format!("Error writing telemetry log: {err}"))
}

pub(crate) fn record_feedback_in_dir(
    dir: &std::path::Path,
    feedback: AgentFeedback,
) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|err| {
        format!(
            "Error creating telemetry directory {}: {err}",
            dir.display()
        )
    })?;
    let encoded = serde_json::to_string(&feedback)
        .map_err(|err| format!("Error serializing routing feedback: {err}"))?;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("feedback.jsonl"))
        .map_err(|err| format!("Error opening routing feedback log: {err}"))?;
    writeln!(file, "{encoded}").map_err(|err| format!("Error writing routing feedback log: {err}"))
}

pub(crate) fn record_proposal_in_dir(
    dir: &std::path::Path,
    proposal: AgentProposal,
) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|err| {
        format!(
            "Error creating telemetry directory {}: {err}",
            dir.display()
        )
    })?;
    let encoded = serde_json::to_string(&proposal)
        .map_err(|err| format!("Error serializing routing proposal: {err}"))?;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("proposals.jsonl"))
        .map_err(|err| format!("Error opening routing proposals log: {err}"))?;
    writeln!(file, "{encoded}").map_err(|err| format!("Error writing routing proposals log: {err}"))
}

pub(crate) fn record_proposal_vote_in_dir(
    dir: &std::path::Path,
    vote: AgentProposalVote,
) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|err| {
        format!(
            "Error creating telemetry directory {}: {err}",
            dir.display()
        )
    })?;
    let encoded = serde_json::to_string(&vote)
        .map_err(|err| format!("Error serializing routing proposal vote: {err}"))?;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("proposal-votes.jsonl"))
        .map_err(|err| format!("Error opening routing proposal votes log: {err}"))?;
    writeln!(file, "{encoded}")
        .map_err(|err| format!("Error writing routing proposal votes log: {err}"))
}

pub(crate) fn read_observations_in_dir(
    dir: &std::path::Path,
) -> Result<Vec<AgentObservation>, String> {
    let path = dir.join("observations.jsonl");
    let Ok(text) = fs::read_to_string(&path) else {
        return Ok(Vec::new());
    };
    Ok(text
        .lines()
        .filter_map(|line| serde_json::from_str::<AgentObservation>(line).ok())
        .collect())
}

pub(crate) fn read_feedback_in_dir(dir: &std::path::Path) -> Result<Vec<AgentFeedback>, String> {
    let path = dir.join("feedback.jsonl");
    let Ok(text) = fs::read_to_string(&path) else {
        return Ok(Vec::new());
    };
    Ok(text
        .lines()
        .filter_map(|line| serde_json::from_str::<AgentFeedback>(line).ok())
        .collect())
}

pub(crate) fn read_proposals_in_dir(dir: &std::path::Path) -> Result<Vec<AgentProposal>, String> {
    let path = dir.join("proposals.jsonl");
    let Ok(text) = fs::read_to_string(&path) else {
        return Ok(Vec::new());
    };
    Ok(text
        .lines()
        .filter_map(|line| serde_json::from_str::<AgentProposal>(line).ok())
        .collect())
}

pub(crate) fn read_proposal_votes_in_dir(
    dir: &std::path::Path,
) -> Result<Vec<AgentProposalVote>, String> {
    let path = dir.join("proposal-votes.jsonl");
    let Ok(text) = fs::read_to_string(&path) else {
        return Ok(Vec::new());
    };
    Ok(text
        .lines()
        .filter_map(|line| serde_json::from_str::<AgentProposalVote>(line).ok())
        .collect())
}
