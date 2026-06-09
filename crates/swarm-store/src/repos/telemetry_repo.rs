//! TelemetryRepo trait + two backends (FileTelemetryRepo, MemTelemetryRepo).
//!
//! Moved from agent-swarm's `repos/telemetry_repo.rs` in P5-S2.
//! Uses `crate::telemetry_helpers::*_in_dir` instead of `crate::telemetry::*_in_dir`.

#![allow(dead_code)]

pub use swarm_core::TelemetryRepo;

use std::path::PathBuf;
use std::sync::Mutex;

use swarm_contracts::telemetry::{
    AgentFeedback, AgentObservation, AgentProposal, AgentProposalVote,
};

use crate::repos::RepoError;
use crate::telemetry_helpers::{
    read_feedback_in_dir, read_observations_in_dir, read_proposal_votes_in_dir,
    read_proposals_in_dir, record_feedback_in_dir, record_observation_in_dir,
    record_proposal_in_dir, record_proposal_vote_in_dir,
};

// ── FileTelemetryRepo ─────────────────────────────────────────────────────────

pub struct FileTelemetryRepo {
    dir: PathBuf,
}

impl FileTelemetryRepo {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    pub fn dir(&self) -> &PathBuf {
        &self.dir
    }
}

/// Constructs a `FileTelemetryRepo` pointed at the default telemetry directory:
///   `<swarm home>/telemetry` (see `crate::store::swarm_home`)
pub fn default_file_telemetry_repo() -> Option<FileTelemetryRepo> {
    crate::store::swarm_home().map(|home| FileTelemetryRepo::new(home.join("telemetry")))
}

fn str_err(s: String) -> RepoError {
    RepoError::Io(std::io::Error::other(s))
}

impl TelemetryRepo for FileTelemetryRepo {
    fn record_observation(&self, obs: AgentObservation) -> Result<(), RepoError> {
        record_observation_in_dir(&self.dir, obs).map_err(str_err)
    }

    fn record_feedback(&self, fb: AgentFeedback) -> Result<(), RepoError> {
        record_feedback_in_dir(&self.dir, fb).map_err(str_err)
    }

    fn record_proposal(&self, prop: AgentProposal) -> Result<(), RepoError> {
        record_proposal_in_dir(&self.dir, prop).map_err(str_err)
    }

    fn record_proposal_vote(&self, vote: AgentProposalVote) -> Result<(), RepoError> {
        record_proposal_vote_in_dir(&self.dir, vote).map_err(str_err)
    }

    fn observations(&self) -> Result<Vec<AgentObservation>, RepoError> {
        read_observations_in_dir(&self.dir).map_err(str_err)
    }

    fn feedback(&self) -> Result<Vec<AgentFeedback>, RepoError> {
        read_feedback_in_dir(&self.dir).map_err(str_err)
    }

    fn proposals(&self) -> Result<Vec<AgentProposal>, RepoError> {
        read_proposals_in_dir(&self.dir).map_err(str_err)
    }

    fn proposal_votes(&self) -> Result<Vec<AgentProposalVote>, RepoError> {
        read_proposal_votes_in_dir(&self.dir).map_err(str_err)
    }
}

// ── MemTelemetryRepo ──────────────────────────────────────────────────────────

pub struct MemTelemetryRepo {
    observations: Mutex<Vec<AgentObservation>>,
    feedback: Mutex<Vec<AgentFeedback>>,
    proposals: Mutex<Vec<AgentProposal>>,
    votes: Mutex<Vec<AgentProposalVote>>,
}

impl Default for MemTelemetryRepo {
    fn default() -> Self {
        Self::new()
    }
}

impl MemTelemetryRepo {
    pub fn new() -> Self {
        Self {
            observations: Mutex::new(Vec::new()),
            feedback: Mutex::new(Vec::new()),
            proposals: Mutex::new(Vec::new()),
            votes: Mutex::new(Vec::new()),
        }
    }
}

impl TelemetryRepo for MemTelemetryRepo {
    fn record_observation(&self, obs: AgentObservation) -> Result<(), RepoError> {
        self.observations.lock().unwrap().push(obs);
        Ok(())
    }

    fn record_feedback(&self, fb: AgentFeedback) -> Result<(), RepoError> {
        self.feedback.lock().unwrap().push(fb);
        Ok(())
    }

    fn record_proposal(&self, prop: AgentProposal) -> Result<(), RepoError> {
        self.proposals.lock().unwrap().push(prop);
        Ok(())
    }

    fn record_proposal_vote(&self, vote: AgentProposalVote) -> Result<(), RepoError> {
        self.votes.lock().unwrap().push(vote);
        Ok(())
    }

    fn observations(&self) -> Result<Vec<AgentObservation>, RepoError> {
        Ok(self.observations.lock().unwrap().clone())
    }

    fn feedback(&self) -> Result<Vec<AgentFeedback>, RepoError> {
        Ok(self.feedback.lock().unwrap().clone())
    }

    fn proposals(&self) -> Result<Vec<AgentProposal>, RepoError> {
        Ok(self.proposals.lock().unwrap().clone())
    }

    fn proposal_votes(&self) -> Result<Vec<AgentProposalVote>, RepoError> {
        Ok(self.votes.lock().unwrap().clone())
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use swarm_contracts::ids::ProposalId;

    struct TestDir(PathBuf);

    impl TestDir {
        fn new(tag: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "agent-swarm-telemetry-{}-{}-{}",
                tag,
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis()
            ));
            std::fs::create_dir_all(&path).expect("TestDir: create_dir_all failed");
            Self(path)
        }

        fn path(&self) -> &PathBuf {
            &self.0
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn sample_observation(n: u64) -> AgentObservation {
        AgentObservation {
            schema: "agent-swarm/observation/v1".into(),
            ts_ms: n as u128,
            mode: "consult".into(),
            session_id: None,
            role: format!("role-{n}"),
            agent: "claude:sonnet".into(),
            cwd: "/tmp".into(),
            status: "completed".into(),
            exit_code: 0,
            timed_out: false,
            duration_ms: 1000 + n as u128,
            prompt_bytes: 100,
            stdout_bytes: 200,
            stderr_bytes: 0,
            input_tokens: None,
            output_tokens: None,
        }
    }

    fn sample_feedback(n: u64) -> AgentFeedback {
        AgentFeedback {
            schema: "agent-swarm/feedback/v1".into(),
            ts_ms: n as u128,
            session_id: None,
            role: format!("role-{n}"),
            agent: "gemini".into(),
            outcome: "win".into(),
            note: Some(format!("note-{n}")),
            weight: 1.0,
        }
    }

    fn sample_proposal(n: u64) -> AgentProposal {
        AgentProposal {
            schema: "agent-swarm/proposal/v1".into(),
            id: ProposalId::from(format!("proposal-{n:x}")),
            ts_ms: n as u128,
            session_id: None,
            title: format!("title-{n}"),
            body: format!("body-{n}"),
            proposed_by: "user".into(),
            status: "open".into(),
            tags: vec![format!("tag-{n}")],
        }
    }

    fn sample_vote(n: u64) -> AgentProposalVote {
        AgentProposalVote {
            schema: "agent-swarm/proposal-vote/v1".into(),
            ts_ms: n as u128,
            proposal_id: ProposalId::from(format!("proposal-{n:x}")),
            voter: format!("voter-{n}"),
            vote: "approve".into(),
            rationale: None,
            weight: 1.0,
        }
    }

    fn telemetry_repo_contract<R: TelemetryRepo>(repo: R) {
        assert!(repo.observations().unwrap().is_empty());
        assert!(repo.feedback().unwrap().is_empty());
        assert!(repo.proposals().unwrap().is_empty());
        assert!(repo.proposal_votes().unwrap().is_empty());

        for n in 1u64..=3 {
            repo.record_observation(sample_observation(n)).unwrap();
        }
        let obs = repo.observations().unwrap();
        assert_eq!(obs.len(), 3);
        assert_eq!(obs[0].ts_ms, 1);
        assert_eq!(obs[1].ts_ms, 2);
        assert_eq!(obs[2].ts_ms, 3);
        assert_eq!(obs[0].role, "role-1");
        assert_eq!(obs[2].agent, "claude:sonnet");

        for n in 1u64..=3 {
            repo.record_feedback(sample_feedback(n)).unwrap();
        }
        let fb = repo.feedback().unwrap();
        assert_eq!(fb.len(), 3);
        assert_eq!(fb[0].ts_ms, 1);
        assert_eq!(fb[0].outcome, "win");

        for n in 1u64..=3 {
            repo.record_proposal(sample_proposal(n)).unwrap();
        }
        let props = repo.proposals().unwrap();
        assert_eq!(props.len(), 3);
        assert_eq!(props[0].status, "open");
        assert_eq!(props[2].title, "title-3");

        for n in 1u64..=3 {
            repo.record_proposal_vote(sample_vote(n)).unwrap();
        }
        let votes = repo.proposal_votes().unwrap();
        assert_eq!(votes.len(), 3);
        assert_eq!(votes[0].vote, "approve");

        repo.record_observation(sample_observation(99)).unwrap();
        assert_eq!(repo.observations().unwrap().len(), 4);
    }

    #[test]
    fn telemetry_repo_contract_mem() {
        telemetry_repo_contract(MemTelemetryRepo::new());
    }

    #[test]
    fn telemetry_repo_contract_file() {
        let dir = TestDir::new("contract");
        telemetry_repo_contract(FileTelemetryRepo::new(dir.path().to_path_buf()));
    }
}
