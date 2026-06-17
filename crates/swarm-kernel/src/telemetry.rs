use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

use serde::Serialize;

use crate::ids::ProposalId;
use crate::task_classifier::classify_task;

pub use swarm_contracts::telemetry::{
    AgentFeedback, AgentObservation, AgentProposal, AgentProposalVote,
};

#[derive(Debug, Default, Clone, Serialize)]
pub struct AgentStats {
    pub agent: String,
    pub role: String,
    pub runs: u64,
    pub failures: u64,
    pub timeouts: u64,
    pub feedback_wins: u64,
    pub feedback_losses: u64,
    pub avg_duration_ms: u128,
    pub avg_stdout_bytes: u128,
    pub score: f64,
}

pub fn record_observation_in_dir(
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

pub fn record_observation(observation: AgentObservation) -> Result<(), String> {
    record_observation_in_dir(&telemetry_dir()?, observation)
}

pub fn record_feedback_in_dir(
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

pub fn record_feedback(feedback: AgentFeedback) -> Result<(), String> {
    record_feedback_in_dir(&telemetry_dir()?, feedback)
}

pub fn insights_json() -> serde_json::Value {
    let observations = read_observations().unwrap_or_default();
    let feedback = read_feedback().unwrap_or_default();
    let proposals = read_proposals().unwrap_or_default();
    let votes = read_proposal_votes().unwrap_or_default();
    let stats = aggregate_stats(&observations, &feedback);
    serde_json::json!({
        "schema": "agent-swarm/insights/v1",
        "store": telemetry_dir().ok().map(|path| path.display().to_string()),
        "observation_count": observations.len(),
        "feedback_count": feedback.len(),
        "proposal_count": proposals.len(),
        "proposal_vote_count": votes.len(),
        "agents": stats,
        "recommendations": recommendations_from_stats(&observations, &feedback),
        "proposals": proposal_summaries(&proposals, &votes)
    })
}

pub fn presets_json() -> serde_json::Value {
    serde_json::json!({
        "schema": "agent-swarm/presets/v1",
        "presets": [
            {
                "id": "architecture-council",
                "description": "Broad design review before large implementation work.",
                "command": "discuss",
                "manager": "claude:sonnet",
                "participants": ["systems=gemini", "tradeoffs=claude:sonnet", "migration=gemini"],
                "rounds": 2,
                "docs": false
            },
            {
                "id": "codebase-audit",
                "description": "Find simplification, hardening, and architecture risks.",
                "command": "audit",
                "focus": "all",
                "manager": "claude:sonnet",
                "participants": ["architecture=gemini", "simplify=claude:sonnet", "hardening=claude:sonnet"],
                "rounds": 1,
                "docs": true
            },
            {
                "id": "ui-polish",
                "description": "Design-system and interaction pass for local operator tooling.",
                "command": "design",
                "focus": "implementation",
                "manager": "claude:sonnet",
                "participants": ["product-design=gemini", "motion-accessibility=claude:sonnet", "component-architecture=gemini"],
                "rounds": 1,
                "docs": false
            },
            {
                "id": "regression-hunt",
                "description": "Parallel diagnosis for flaky behavior or UI glitches.",
                "command": "fanout",
                "manager": "claude:sonnet",
                "workers": ["repro=gemini", "root-cause=claude:sonnet", "tests=gemini"]
            },
            {
                "id": "api-docs-followup",
                "description": "Trailing documentation pass after an implementation or audit.",
                "command": "discuss",
                "manager": "claude:sonnet",
                "participants": ["api-docs=claude:sonnet", "examples=gemini"],
                "rounds": 1,
                "docs": false
            }
        ]
    })
}

pub fn recommendation_json(task: &str) -> serde_json::Value {
    let observations = read_observations().unwrap_or_default();
    let feedback = read_feedback().unwrap_or_default();
    let classification = classify_task(task);
    let participants = classification
        .roles
        .iter()
        .map(|role| {
            format!(
                "{role}={}",
                best_agent_for_role(role, &observations, &feedback)
            )
        })
        .collect::<Vec<_>>();
    serde_json::json!({
        "schema": "agent-swarm/recommendation/v1",
        "task_preview": preview(task, 160),
        "classification": classification,
        "manager": best_agent_for_role("manager", &observations, &feedback),
        "participants": participants,
        "basis": {
            "observation_count": observations.len(),
            "feedback_count": feedback.len(),
            "strategy": "shared deterministic task classification plus lazy aggregate success/duration scoring and explicit user feedback"
        }
    })
}

pub fn feedback_json(
    session_id: Option<String>,
    role: String,
    agent: String,
    outcome: String,
    note: Option<String>,
) -> Result<serde_json::Value, String> {
    let normalized = normalize_outcome(&outcome)?;
    let feedback = AgentFeedback {
        schema: "agent-swarm/feedback/v1".to_string(),
        ts_ms: now_ms(),
        session_id,
        role,
        agent,
        outcome: normalized,
        note,
        weight: 1.0,
    };
    record_feedback(feedback.clone())?;
    Ok(serde_json::json!({
        "schema": "agent-swarm/feedback-recorded/v1",
        "feedback": feedback
    }))
}

pub fn proposals_json() -> serde_json::Value {
    let proposals = read_proposals().unwrap_or_default();
    let votes = read_proposal_votes().unwrap_or_default();
    serde_json::json!({
        "schema": "agent-swarm/proposals/v1",
        "store": telemetry_dir().ok().map(|path| path.display().to_string()),
        "proposal_count": proposals.len(),
        "vote_count": votes.len(),
        "proposals": proposal_summaries(&proposals, &votes)
    })
}

pub fn proposal_json(
    session_id: Option<String>,
    title: String,
    body: String,
    proposed_by: Option<String>,
    tags: Vec<String>,
) -> Result<serde_json::Value, String> {
    if title.trim().is_empty() {
        return Err("Error: proposal title is required".to_string());
    }
    if body.trim().is_empty() {
        return Err("Error: proposal body is required".to_string());
    }
    let proposal = AgentProposal {
        schema: "agent-swarm/proposal/v1".to_string(),
        id: ProposalId::from(format!("proposal-{:x}-{}", now_ms(), std::process::id())),
        ts_ms: now_ms(),
        session_id,
        title: title.trim().to_string(),
        body: body.trim().to_string(),
        proposed_by: proposed_by
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "user".to_string()),
        status: "open".to_string(),
        tags: tags
            .into_iter()
            .map(|tag| tag.trim().to_string())
            .filter(|tag| !tag.is_empty())
            .collect(),
    };
    record_proposal(proposal.clone())?;
    Ok(serde_json::json!({
        "schema": "agent-swarm/proposal-recorded/v1",
        "proposal": proposal
    }))
}

pub fn proposal_vote_json(
    proposal_id: ProposalId,
    voter: String,
    vote: String,
    rationale: Option<String>,
) -> Result<serde_json::Value, String> {
    let normalized = normalize_vote(&vote)?;
    if proposal_id.as_str().trim().is_empty() {
        return Err("Error: proposal_id is required".to_string());
    }
    if voter.trim().is_empty() {
        return Err("Error: voter is required".to_string());
    }
    let vote = AgentProposalVote {
        schema: "agent-swarm/proposal-vote/v1".to_string(),
        ts_ms: now_ms(),
        proposal_id,
        voter,
        vote: normalized,
        rationale,
        weight: 1.0,
    };
    record_proposal_vote(vote.clone())?;
    Ok(serde_json::json!({
        "schema": "agent-swarm/proposal-vote-recorded/v1",
        "vote": vote
    }))
}

pub fn read_observations_in_dir(dir: &std::path::Path) -> Result<Vec<AgentObservation>, String> {
    let path = dir.join("observations.jsonl");
    let Ok(text) = fs::read_to_string(&path) else {
        return Ok(Vec::new());
    };
    Ok(text
        .lines()
        .filter_map(|line| serde_json::from_str::<AgentObservation>(line).ok())
        .collect())
}

fn read_observations() -> Result<Vec<AgentObservation>, String> {
    read_observations_in_dir(&telemetry_dir()?)
}

pub fn read_feedback_in_dir(dir: &std::path::Path) -> Result<Vec<AgentFeedback>, String> {
    let path = dir.join("feedback.jsonl");
    let Ok(text) = fs::read_to_string(&path) else {
        return Ok(Vec::new());
    };
    Ok(text
        .lines()
        .filter_map(|line| serde_json::from_str::<AgentFeedback>(line).ok())
        .collect())
}

fn read_feedback() -> Result<Vec<AgentFeedback>, String> {
    read_feedback_in_dir(&telemetry_dir()?)
}

pub fn record_proposal_in_dir(
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

pub fn record_proposal(proposal: AgentProposal) -> Result<(), String> {
    record_proposal_in_dir(&telemetry_dir()?, proposal)
}

pub fn record_proposal_vote_in_dir(
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

pub fn record_proposal_vote(vote: AgentProposalVote) -> Result<(), String> {
    record_proposal_vote_in_dir(&telemetry_dir()?, vote)
}

pub fn read_proposals_in_dir(dir: &std::path::Path) -> Result<Vec<AgentProposal>, String> {
    let path = dir.join("proposals.jsonl");
    let Ok(text) = fs::read_to_string(&path) else {
        return Ok(Vec::new());
    };
    Ok(text
        .lines()
        .filter_map(|line| serde_json::from_str::<AgentProposal>(line).ok())
        .collect())
}

fn read_proposals() -> Result<Vec<AgentProposal>, String> {
    read_proposals_in_dir(&telemetry_dir()?)
}

pub fn read_proposal_votes_in_dir(dir: &std::path::Path) -> Result<Vec<AgentProposalVote>, String> {
    let path = dir.join("proposal-votes.jsonl");
    let Ok(text) = fs::read_to_string(&path) else {
        return Ok(Vec::new());
    };
    Ok(text
        .lines()
        .filter_map(|line| serde_json::from_str::<AgentProposalVote>(line).ok())
        .collect())
}

fn read_proposal_votes() -> Result<Vec<AgentProposalVote>, String> {
    read_proposal_votes_in_dir(&telemetry_dir()?)
}

#[derive(Default)]
struct StatsAccumulator<'a> {
    observations: Vec<&'a AgentObservation>,
    feedback: Vec<&'a AgentFeedback>,
}

pub fn aggregate_stats(
    observations: &[AgentObservation],
    feedback: &[AgentFeedback],
) -> Vec<AgentStats> {
    let mut grouped: BTreeMap<(String, String), StatsAccumulator<'_>> = BTreeMap::new();
    for observation in observations {
        grouped
            .entry((observation.role.clone(), observation.agent.clone()))
            .or_default()
            .observations
            .push(observation);
    }
    for item in feedback {
        grouped
            .entry((item.role.clone(), item.agent.clone()))
            .or_default()
            .feedback
            .push(item);
    }
    grouped
        .into_iter()
        .map(|((role, agent), items)| {
            let runs = items.observations.len() as u64;
            let failures = items
                .observations
                .iter()
                .filter(|item| item.exit_code != 0)
                .count() as u64;
            let timeouts = items
                .observations
                .iter()
                .filter(|item| item.timed_out)
                .count() as u64;
            let feedback_wins = items
                .feedback
                .iter()
                .filter(|item| item.outcome == "win")
                .count() as u64;
            let feedback_losses = items
                .feedback
                .iter()
                .filter(|item| item.outcome == "loss")
                .count() as u64;
            let avg_duration_ms = average(items.observations.iter().map(|item| item.duration_ms));
            let avg_stdout_bytes = average(
                items
                    .observations
                    .iter()
                    .map(|item| item.stdout_bytes as u128),
            );
            let attempts = runs + feedback_wins + feedback_losses;
            let wins = runs.saturating_sub(failures) + feedback_wins;
            let success_rate = (wins + 1) as f64 / (attempts + 2) as f64;
            let speed_penalty = (avg_duration_ms as f64 / 120_000.0).min(1.0) * 0.15;
            let timeout_penalty = if runs == 0 {
                0.0
            } else {
                timeouts as f64 / runs as f64 * 0.25
            };
            AgentStats {
                agent,
                role,
                runs,
                failures,
                timeouts,
                feedback_wins,
                feedback_losses,
                avg_duration_ms,
                avg_stdout_bytes,
                score: (success_rate - speed_penalty - timeout_penalty).clamp(0.0, 1.0),
            }
        })
        .collect()
}

pub fn recommendations_from_stats(
    observations: &[AgentObservation],
    feedback: &[AgentFeedback],
) -> Vec<serde_json::Value> {
    [
        "architecture",
        "hardening",
        "product-design",
        "api-docs",
        "manager",
    ]
    .iter()
    .map(|role| {
        serde_json::json!({
            "role": role,
            "agent": best_agent_for_role(role, observations, feedback)
        })
    })
    .collect()
}

/// Quantize a `[0.0, 1.0]` agent score to a total-orderable integer at 6
/// decimal places. This is the single comparator key used by both ranking
/// sites below: it removes the `partial_cmp` `None`/NaN edge (which silently
/// collapsed to `Ordering::Equal` and made the winner order-dependent) and
/// folds away float noise so two effectively-equal scores tie cleanly into the
/// explicit `(runs, agent)` tiebreak. NaN — not reachable today, since
/// `aggregate_stats` builds `score` from a finite ratio clamped to `[0,1]` —
/// maps to the floor so a malformed stat can never win a ranking.
fn score_key(score: f64) -> i64 {
    if score.is_nan() {
        return 0;
    }
    (score * 1_000_000.0).round() as i64
}

pub fn best_agent_for_role(
    role: &str,
    observations: &[AgentObservation],
    feedback: &[AgentFeedback],
) -> String {
    let stats = aggregate_stats(observations, feedback);
    stats
        .iter()
        .filter(|stat| stat.role == role || (role == "manager" && stat.role == "manager"))
        // Total order via quantized score, then the same tiebreak priority as
        // `learned_candidates_for_role`: more runs win, then lexically-smaller
        // agent name. `max_by` keeps the *last* of equal-comparing elements, so
        // the name tiebreak is reversed (`b.cmp(&a)`) to make the smallest name
        // the winner regardless of `aggregate_stats` ordering.
        .max_by(|a, b| {
            score_key(a.score)
                .cmp(&score_key(b.score))
                .then_with(|| a.runs.cmp(&b.runs))
                .then_with(|| b.agent.cmp(&a.agent))
        })
        .map(|stat| stat.agent.clone())
        .unwrap_or_else(|| default_agent_for_role(role).to_string())
}

/// Ranked agent candidates for a role, learned from observed outcomes and
/// explicit feedback. This is the read side of the Phase-2 learned-routing
/// cutover (see `routing_repo.rs:1-15`): dispatch calls this when a role's
/// fallback chain is otherwise unset, and uses the result to fill the gap.
///
/// Returns agents in descending score order, filtered to those with at least
/// `min_observations` runs — so a cold telemetry store yields nothing and the
/// caller falls back to the static `DEFAULT_FALLBACK_CHAIN` unchanged. Ties on
/// score are broken by agent name (ascending) for deterministic output across
/// runs and tests; `runs` is a secondary tiebreaker to favor more-tested agents
/// when scores are within floating-point noise of each other.
///
/// Note the role match mirrors [`best_agent_for_role`]: only an exact role
/// match counts (the `"manager"` special-case there is redundant since it is
/// already an exact match, and is intentionally not replicated here).
pub fn learned_candidates_for_role(
    role: &str,
    observations: &[AgentObservation],
    feedback: &[AgentFeedback],
    min_observations: u32,
) -> Vec<String> {
    let mut stats = aggregate_stats(observations, feedback)
        .into_iter()
        .filter(|stat| stat.role == role && stat.runs >= min_observations as u64)
        .collect::<Vec<_>>();
    // Sort by score desc; tie-break on more runs, then agent name asc for
    // deterministic output regardless of BTreeMap iteration order. Score is
    // compared via the quantized `score_key` so the ordering is total (no
    // `partial_cmp` NaN/`None` edge) and float noise can't perturb the tiebreak.
    stats.sort_by(|a, b| {
        score_key(b.score)
            .cmp(&score_key(a.score))
            .then_with(|| b.runs.cmp(&a.runs))
            .then_with(|| a.agent.cmp(&b.agent))
    });
    stats.into_iter().map(|stat| stat.agent).collect()
}

fn default_agent_for_role(role: &str) -> &'static str {
    match role {
        "manager" | "review" | "hardening" | "simplify" | "api-docs" | "motion-accessibility" => {
            "claude:sonnet"
        }
        "implementation" | "implementation-plan" => "claude:sonnet",
        _ => "gemini",
    }
}

fn normalize_outcome(value: &str) -> Result<String, String> {
    match value.to_ascii_lowercase().as_str() {
        "win" | "success" | "good" | "helpful" => Ok("win".to_string()),
        "loss" | "failure" | "bad" | "unhelpful" => Ok("loss".to_string()),
        other => Err(format!(
            "Error: feedback outcome must be win or loss, got {other:?}"
        )),
    }
}

fn normalize_vote(value: &str) -> Result<String, String> {
    match value.to_ascii_lowercase().as_str() {
        "approve" | "yes" | "win" | "up" | "+1" => Ok("approve".to_string()),
        "reject" | "no" | "loss" | "down" | "-1" => Ok("reject".to_string()),
        "defer" | "needs-work" | "needs_work" | "hold" => Ok("defer".to_string()),
        other => Err(format!(
            "Error: proposal vote must be approve, reject, or defer, got {other:?}"
        )),
    }
}

pub fn proposal_summaries(
    proposals: &[AgentProposal],
    votes: &[AgentProposalVote],
) -> Vec<serde_json::Value> {
    proposals
        .iter()
        .map(|proposal| {
            let proposal_votes = votes
                .iter()
                .filter(|vote| vote.proposal_id == proposal.id)
                .collect::<Vec<_>>();
            let approvals = proposal_votes
                .iter()
                .filter(|vote| vote.vote == "approve")
                .count();
            let rejections = proposal_votes
                .iter()
                .filter(|vote| vote.vote == "reject")
                .count();
            let deferrals = proposal_votes
                .iter()
                .filter(|vote| vote.vote == "defer")
                .count();
            serde_json::json!({
                "id": proposal.id,
                "ts_ms": proposal.ts_ms,
                "session_id": proposal.session_id,
                "title": proposal.title,
                "body_preview": preview(&proposal.body, 220),
                "proposed_by": proposal.proposed_by,
                "status": proposal.status,
                "tags": proposal.tags,
                "votes": {
                    "approve": approvals,
                    "reject": rejections,
                    "defer": deferrals,
                    "total": proposal_votes.len()
                },
                "latest_vote_ms": proposal_votes.iter().map(|vote| vote.ts_ms).max()
            })
        })
        .collect()
}

fn average(values: impl Iterator<Item = u128>) -> u128 {
    let mut count = 0u128;
    let mut sum = 0u128;
    for value in values {
        count += 1;
        sum += value;
    }
    sum.checked_div(count).unwrap_or(0)
}

fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}

fn telemetry_dir() -> Result<PathBuf, String> {
    let home = swarm_store::store::swarm_home().ok_or_else(swarm_store::store::swarm_home_err)?;
    Ok(home.join("telemetry"))
}

fn preview(value: &str, max: usize) -> String {
    let compact = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() <= max {
        compact
    } else {
        format!(
            "{}...",
            compact
                .chars()
                .take(max.saturating_sub(3))
                .collect::<String>()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Confirm that an `AgentProposalVote` record with a bare-string `proposal_id`
    /// (the on-disk format that existed before this newtype) still deserializes
    /// correctly.  The `#[serde(transparent)]` on `ProposalId` guarantees this,
    /// but an explicit test keeps the contract visible.
    #[test]
    fn proposal_vote_proposal_id_round_trip_from_raw_json() {
        let raw = r#"{
            "schema": "agent-swarm/proposal-vote/v1",
            "ts_ms": 12345,
            "proposal_id": "proposal-abc123",
            "voter": "user",
            "vote": "approve",
            "rationale": null,
            "weight": 1.0
        }"#;
        let vote: AgentProposalVote =
            serde_json::from_str(raw).expect("deserializes from disk JSON");
        assert_eq!(vote.proposal_id.as_str(), "proposal-abc123");
        // Re-serialise: the field must still be a bare string, not an object.
        let re_encoded = serde_json::to_string(&vote).unwrap();
        let val: serde_json::Value = serde_json::from_str(&re_encoded).unwrap();
        assert_eq!(val["proposal_id"], "proposal-abc123");
    }

    /// Old observations that lack token fields must still deserialize (back-compat
    /// guarantee for the Option + serde default annotation).
    #[test]
    fn agent_observation_round_trips_without_token_fields() {
        let raw = r#"{
            "schema": "agent-swarm/observation/v1",
            "ts_ms": 1000,
            "mode": "fanout",
            "session_id": null,
            "role": "worker",
            "agent": "claude:sonnet",
            "cwd": "/tmp",
            "status": "completed",
            "exit_code": 0,
            "timed_out": false,
            "duration_ms": 5000,
            "prompt_bytes": 42,
            "stdout_bytes": 128,
            "stderr_bytes": 0
        }"#;
        let obs: AgentObservation =
            serde_json::from_str(raw).expect("deserializes without token fields");
        assert!(obs.input_tokens.is_none());
        assert!(obs.output_tokens.is_none());
        // Re-serialise: token fields must NOT appear (skip_serializing_if = Option::is_none).
        let re_encoded = serde_json::to_string(&obs).unwrap();
        let val: serde_json::Value = serde_json::from_str(&re_encoded).unwrap();
        assert!(
            val.get("input_tokens").is_none(),
            "input_tokens must be absent when None"
        );
        assert!(
            val.get("output_tokens").is_none(),
            "output_tokens must be absent when None"
        );
    }

    /// New observations with token fields round-trip correctly.
    #[test]
    fn agent_observation_round_trips_with_token_fields() {
        let raw = r#"{
            "schema": "agent-swarm/observation/v1",
            "ts_ms": 2000,
            "mode": "fanout",
            "session_id": "sess-abc",
            "role": "worker",
            "agent": "claude:sonnet",
            "cwd": "/tmp",
            "status": "completed",
            "exit_code": 0,
            "timed_out": false,
            "duration_ms": 8000,
            "prompt_bytes": 100,
            "stdout_bytes": 200,
            "stderr_bytes": 0,
            "input_tokens": 512,
            "output_tokens": 128
        }"#;
        let obs: AgentObservation =
            serde_json::from_str(raw).expect("deserializes with token fields");
        assert_eq!(obs.input_tokens, Some(512));
        assert_eq!(obs.output_tokens, Some(128));
        // Re-serialise: token fields must be present and correct.
        let re_encoded = serde_json::to_string(&obs).unwrap();
        let val: serde_json::Value = serde_json::from_str(&re_encoded).unwrap();
        assert_eq!(val["input_tokens"], 512u64);
        assert_eq!(val["output_tokens"], 128u64);
    }

    /// Confirm that an `AgentProposal` record with a bare-string `id` (on-disk
    /// format) still deserializes and re-serializes byte-identically via the
    /// transparent newtype.
    #[test]
    fn proposal_id_round_trip_from_raw_json() {
        let raw = r#"{
            "schema": "agent-swarm/proposal/v1",
            "id": "proposal-deadbeef",
            "ts_ms": 999,
            "session_id": null,
            "title": "Test proposal",
            "body": "Body text",
            "proposed_by": "user",
            "status": "open",
            "tags": []
        }"#;
        let proposal: AgentProposal =
            serde_json::from_str(raw).expect("deserializes from disk JSON");
        assert_eq!(proposal.id.as_str(), "proposal-deadbeef");
        let re_encoded = serde_json::to_string(&proposal).unwrap();
        let val: serde_json::Value = serde_json::from_str(&re_encoded).unwrap();
        assert_eq!(val["id"], "proposal-deadbeef");
    }

    // ── learned_candidates_for_role ─────────────────────────────────────────

    /// Minimal observation builder for the learned-routing tests. `fail` flips
    /// exit_code; `duration_ms` influences the speed penalty.
    fn obs(role: &str, agent: &str, n: u128, fail: bool, duration_ms: u128) -> AgentObservation {
        AgentObservation {
            schema: "agent-swarm/observation/v1".into(),
            ts_ms: n,
            mode: "consult".into(),
            session_id: None,
            role: role.into(),
            agent: agent.into(),
            cwd: "/tmp".into(),
            status: if fail { "failed" } else { "completed" }.into(),
            exit_code: if fail { 1 } else { 0 },
            timed_out: false,
            duration_ms,
            prompt_bytes: 100,
            stdout_bytes: 200,
            stderr_bytes: 0,
            input_tokens: None,
            output_tokens: None,
        }
    }

    /// Cold-start: no telemetry at all → empty candidate list (caller falls
    /// back to the static DEFAULT_FALLBACK_CHAIN, unchanged from today).
    #[test]
    fn learned_candidates_empty_when_no_telemetry() {
        let got = learned_candidates_for_role("architecture", &[], &[], 3);
        assert!(got.is_empty(), "cold telemetry must yield no candidates");
    }

    /// Below the min_observations threshold → empty. This keeps a single lucky
    /// run from promoting an agent over the configured default.
    #[test]
    fn learned_candidates_empty_below_min_observations() {
        // Only 2 runs for claude:sonnet, but min_observations=3.
        let observations = vec![
            obs("architecture", "claude:sonnet", 1, false, 5_000),
            obs("architecture", "claude:sonnet", 2, false, 5_000),
        ];
        let got = learned_candidates_for_role("architecture", &observations, &[], 3);
        assert!(got.is_empty(), "below-threshold data must yield no candidates");
    }

    /// Seeded telemetry → candidates sorted by score descending. Gemini wins
    /// here (1/1 success, no feedback) over claude:sonnet (1 fail in 3 runs),
    /// so the order is [gemini, claude:sonnet].
    #[test]
    fn learned_candidates_sorted_by_score_descending() {
        let observations = vec![
            obs("architecture", "claude:sonnet", 1, false, 5_000),
            obs("architecture", "claude:sonnet", 2, true, 5_000),
            obs("architecture", "claude:sonnet", 3, false, 5_000),
            obs("architecture", "gemini", 4, false, 4_000),
            obs("architecture", "gemini", 5, false, 4_000),
            obs("architecture", "gemini", 6, false, 4_000),
        ];
        let got = learned_candidates_for_role("architecture", &observations, &[], 3);
        assert_eq!(
            got,
            vec!["gemini".to_string(), "claude:sonnet".to_string()],
            "higher-scoring agent must come first"
        );
    }

    /// Other roles are excluded — only the requested role's agents appear.
    #[test]
    fn learned_candidates_filters_by_role() {
        let observations = vec![
            obs("architecture", "gemini", 1, false, 4_000),
            obs("architecture", "gemini", 2, false, 4_000),
            obs("architecture", "gemini", 3, false, 4_000),
            obs("hardening", "claude:sonnet", 4, false, 4_000),
            obs("hardening", "claude:sonnet", 5, false, 4_000),
            obs("hardening", "claude:sonnet", 6, false, 4_000),
        ];
        let got = learned_candidates_for_role("architecture", &observations, &[], 3);
        assert_eq!(got, vec!["gemini".to_string()]);
    }

    /// Deterministic tie-break: two agents with identical score and runs come
    /// back in ascending agent-name order, regardless of input order.
    #[test]
    fn learned_candidates_tie_break_is_deterministic() {
        // Both agents: 3/3 success, identical duration → identical score.
        // Expect alphabetical: claude:sonnet before gemini.
        let observations_reversed = vec![
            obs("architecture", "gemini", 1, false, 4_000),
            obs("architecture", "gemini", 2, false, 4_000),
            obs("architecture", "gemini", 3, false, 4_000),
            obs("architecture", "claude:sonnet", 4, false, 4_000),
            obs("architecture", "claude:sonnet", 5, false, 4_000),
            obs("architecture", "claude:sonnet", 6, false, 4_000),
        ];
        let got = learned_candidates_for_role("architecture", &observations_reversed, &[], 3);
        assert_eq!(
            got,
            vec!["claude:sonnet".to_string(), "gemini".to_string()],
            "ties must break by agent name ascending, deterministically"
        );
    }

    /// `score_key` is total and quantized: distinct scores keep their order,
    /// float noise below the 1e-6 quantum collapses to a tie, and NaN floors
    /// instead of poisoning the comparison (the old `partial_cmp` path returned
    /// `None` → `Ordering::Equal`).
    #[test]
    fn score_key_is_total_and_quantized() {
        assert!(score_key(0.9) > score_key(0.1));
        assert_eq!(score_key(0.5), score_key(0.5));
        assert_eq!(
            score_key(0.5),
            score_key(0.5 + 1e-9),
            "sub-quantum float noise must fold to a tie"
        );
        assert!(
            score_key(f64::NAN) <= score_key(0.0),
            "NaN must never outrank a real score"
        );
    }

    /// On an exact score+runs tie, `best_agent_for_role` resolves to the
    /// lexically-smaller agent name — matching `learned_candidates_for_role`.
    /// The pre-quantization `max_by` had no tiebreak and kept the BTreeMap-last
    /// (lexically-larger) agent, so this would have returned "gemini".
    #[test]
    fn best_agent_for_role_breaks_ties_by_smallest_name() {
        let forward = vec![
            obs("architecture", "claude:sonnet", 1, false, 4_000),
            obs("architecture", "claude:sonnet", 2, false, 4_000),
            obs("architecture", "claude:sonnet", 3, false, 4_000),
            obs("architecture", "gemini", 4, false, 4_000),
            obs("architecture", "gemini", 5, false, 4_000),
            obs("architecture", "gemini", 6, false, 4_000),
        ];
        let mut reversed = forward.clone();
        reversed.reverse();
        let from_forward = best_agent_for_role("architecture", &forward, &[]);
        let from_reversed = best_agent_for_role("architecture", &reversed, &[]);
        assert_eq!(
            from_forward, from_reversed,
            "winner must not depend on input order"
        );
        assert_eq!(
            from_forward, "claude:sonnet",
            "exact tie must resolve to the lexically-smaller name"
        );
    }
}
