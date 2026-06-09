//! RoutingMemoryRepo — thin read-model over TelemetryRepo.
//!
//! Slice T8 — Phase-1 Gate-3 dual-backend contract test.
//!
//! RoutingMemoryRepo is a pure read-model: each call reads observations and
//! feedback from an underlying TelemetryRepo and runs the existing aggregation
//! math from `telemetry.rs`. There is no durable scoring cache — that is Phase
//! 2. The struct is generic over T: TelemetryRepo (owned, not &dyn) to avoid
//! lifetime parameters while still giving both MemTelemetryRepo and
//! FileTelemetryRepo a path through the same blanket impl.
//!
//! Naming note: "routing" here refers to *agent-routing memory* (which agent
//! performed best for which role), NOT to `routing.rs` (the Piece-1 backend-
//! fallback chain). Do not wire this module into `routing.rs`.
#![allow(dead_code)]

use swarm_core::RepoError;
use swarm_kernel::telemetry::{aggregate_stats, best_agent_for_role, AgentStats};
use swarm_store::repos::telemetry_repo::TelemetryRepo;

// ── Return types ─────────────────────────────────────────────────────────────

/// A role → agent recommendation derived from the aggregated telemetry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoleRecommendation {
    pub role: String,
    pub agent: String,
}

// ── RoutingMemoryRepo trait ───────────────────────────────────────────────────

/// Read-model over TelemetryRepo: computes agent statistics and role
/// recommendations from raw observations and feedback.
///
/// All methods are pure reads — they aggregate on every call without caching.
/// Implementations must return `RepoError` on any I/O or deserialization
/// failure propagated from the underlying TelemetryRepo.
pub trait RoutingMemoryRepo: Send + Sync {
    /// Aggregated per-(role, agent) statistics derived from observations and
    /// feedback. Returns an empty `Vec` when the telemetry store is empty.
    fn agent_stats(&self) -> Result<Vec<AgentStats>, RepoError>;

    /// Return the best agent for a given `role` string, falling back to the
    /// default agent for that role when no telemetry data is available.
    fn best_agent_for_role(&self, role: &str) -> Result<String, RepoError>;

    /// Return a recommendation (role → best agent) for each of the standard
    /// routing roles: architecture, hardening, product-design, api-docs,
    /// manager.
    fn recommendations(&self) -> Result<Vec<RoleRecommendation>, RepoError>;
}

// ── RoutingMemory<T> — blanket read-model impl ────────────────────────────────

/// Read-model parameterized over any TelemetryRepo backend.
///
/// The two "Gate-3 backends" from §5.1 of the design spec are:
/// - `RoutingMemory<MemTelemetryRepo>` (in-memory, test-fast)
/// - `RoutingMemory<FileTelemetryRepo>` (JSONL-on-disk, file parity)
///
/// Both are covered by the single blanket `impl<T: TelemetryRepo>
/// RoutingMemoryRepo for RoutingMemory<T>` below.
pub struct RoutingMemory<T: TelemetryRepo> {
    telemetry: T,
}

impl<T: TelemetryRepo> RoutingMemory<T> {
    pub fn new(telemetry: T) -> Self {
        Self { telemetry }
    }
}

/// Standard roles for `recommendations()`. Mirrors the role list in
/// `telemetry.rs::recommendations_from_stats`.
pub const RECOMMENDATION_ROLES: &[&str] = &[
    "architecture",
    "hardening",
    "product-design",
    "api-docs",
    "manager",
];

impl<T: TelemetryRepo> RoutingMemoryRepo for RoutingMemory<T> {
    fn agent_stats(&self) -> Result<Vec<AgentStats>, RepoError> {
        let observations = self.telemetry.observations()?;
        let feedback = self.telemetry.feedback()?;
        Ok(aggregate_stats(&observations, &feedback))
    }

    fn best_agent_for_role(&self, role: &str) -> Result<String, RepoError> {
        let observations = self.telemetry.observations()?;
        let feedback = self.telemetry.feedback()?;
        Ok(best_agent_for_role(role, &observations, &feedback))
    }

    fn recommendations(&self) -> Result<Vec<RoleRecommendation>, RepoError> {
        let observations = self.telemetry.observations()?;
        let feedback = self.telemetry.feedback()?;
        Ok(RECOMMENDATION_ROLES
            .iter()
            .map(|&role| RoleRecommendation {
                role: role.to_string(),
                agent: best_agent_for_role(role, &observations, &feedback),
            })
            .collect())
    }
}

// ── Gate-3 dual-backend contract test ────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use swarm_kernel::telemetry::{AgentFeedback, AgentObservation};
    use swarm_store::repos::telemetry_repo::{FileTelemetryRepo, MemTelemetryRepo};

    // ── RAII temp directory (no external dep) ────────────────────────────────

    struct TestDir(PathBuf);

    impl TestDir {
        fn new(tag: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "agent-swarm-routing-{}-{}-{}",
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

    // ── Sample constructors ──────────────────────────────────────────────────

    /// Two observations for "architecture" role with "claude:sonnet" agent:
    /// - one success (exit_code=0), one failure (exit_code=1)
    fn arch_sonnet_observation(n: u64, fail: bool) -> AgentObservation {
        AgentObservation {
            schema: "agent-swarm/observation/v1".into(),
            ts_ms: n as u128,
            mode: "consult".into(),
            session_id: None,
            role: "architecture".into(),
            agent: "claude:sonnet".into(),
            cwd: "/tmp".into(),
            status: if fail { "failed" } else { "completed" }.into(),
            exit_code: if fail { 1 } else { 0 },
            timed_out: false,
            duration_ms: 5_000,
            prompt_bytes: 100,
            stdout_bytes: 200,
            stderr_bytes: 0,
            input_tokens: None,
            output_tokens: None,
        }
    }

    /// One observation for "architecture" role with "gemini" agent (success).
    fn arch_gemini_observation(n: u64) -> AgentObservation {
        AgentObservation {
            schema: "agent-swarm/observation/v1".into(),
            ts_ms: n as u128,
            mode: "consult".into(),
            session_id: None,
            role: "architecture".into(),
            agent: "gemini".into(),
            cwd: "/tmp".into(),
            status: "completed".into(),
            exit_code: 0,
            timed_out: false,
            duration_ms: 4_000,
            prompt_bytes: 100,
            stdout_bytes: 200,
            stderr_bytes: 0,
            input_tokens: None,
            output_tokens: None,
        }
    }

    /// Explicit "win" feedback for claude:sonnet on "architecture".
    fn arch_sonnet_win_feedback(n: u64) -> AgentFeedback {
        AgentFeedback {
            schema: "agent-swarm/feedback/v1".into(),
            ts_ms: n as u128,
            session_id: None,
            role: "architecture".into(),
            agent: "claude:sonnet".into(),
            outcome: "win".into(),
            note: None,
            weight: 1.0,
        }
    }

    // ── Generic Gate-3 contract ──────────────────────────────────────────────

    /// Exercises all three RoutingMemoryRepo methods against any backend.
    ///
    /// Seed known observations + feedback so the aggregation math is
    /// deterministic. Assertions cover:
    /// - empty telemetry → empty agent_stats, defaults from best_agent_for_role
    /// - seeded telemetry → agent_stats reports correct run/failure counts
    /// - seeded telemetry → best_agent_for_role returns highest-scoring agent
    /// - recommendations returns one entry per RECOMMENDATION_ROLES item
    fn routing_memory_repo_contract<R: RoutingMemoryRepo>(repo: R) {
        // ── empty state ──────────────────────────────────────────────────────
        // No observations or feedback → aggregate_stats returns empty vec.
        let stats = repo.agent_stats().unwrap();
        assert!(
            stats.is_empty(),
            "empty telemetry must yield empty agent_stats, got {stats:?}"
        );

        // No telemetry → falls back to default_agent_for_role("architecture")
        // which is "gemini" per telemetry.rs::default_agent_for_role.
        let best_empty = repo.best_agent_for_role("architecture").unwrap();
        assert_eq!(
            best_empty, "gemini",
            "empty telemetry for architecture role must fall back to gemini default"
        );

        // Recommendations list is always RECOMMENDATION_ROLES.len() entries.
        let recs_empty = repo.recommendations().unwrap();
        assert_eq!(
            recs_empty.len(),
            RECOMMENDATION_ROLES.len(),
            "recommendations must always return one entry per RECOMMENDATION_ROLES item"
        );
        // All fall back to defaults when empty.
        for rec in &recs_empty {
            assert!(
                !rec.role.is_empty(),
                "every recommendation must have a non-empty role"
            );
            assert!(
                !rec.agent.is_empty(),
                "every recommendation must have a non-empty agent"
            );
        }
    }

    /// Extended contract that seeds known telemetry and asserts computed values.
    ///
    /// This is a second-pass contract exercised only on the mem backend
    /// (seeding file backend separately would duplicate the math; the first-
    /// pass contract already validates file-backend I/O). The file backend
    /// contract (`routing_memory_repo_contract_file`) seeds the same data via
    /// its own FileTelemetryRepo path.
    fn routing_memory_repo_contract_seeded<T: TelemetryRepo>(telemetry: T) {
        // Seed: 2 claude:sonnet observations (1 fail) + 1 gemini observation
        //       + 1 explicit win feedback for claude:sonnet.
        telemetry
            .record_observation(arch_sonnet_observation(1, false))
            .unwrap();
        telemetry
            .record_observation(arch_sonnet_observation(2, true))
            .unwrap();
        telemetry
            .record_observation(arch_gemini_observation(3))
            .unwrap();
        telemetry
            .record_feedback(arch_sonnet_win_feedback(4))
            .unwrap();

        let repo = RoutingMemory::new(telemetry);

        // ── agent_stats ──────────────────────────────────────────────────────
        let stats = repo.agent_stats().unwrap();
        // BTreeMap ordering in aggregate_stats: sorted by (role, agent).
        // "architecture"/"claude:sonnet" < "architecture"/"gemini" lexically.
        assert_eq!(stats.len(), 2, "expected stats for 2 (role,agent) pairs");

        let sonnet_stat = stats
            .iter()
            .find(|s| s.agent == "claude:sonnet")
            .expect("expected claude:sonnet stats");
        assert_eq!(sonnet_stat.role, "architecture");
        assert_eq!(sonnet_stat.runs, 2, "claude:sonnet had 2 observations");
        assert_eq!(sonnet_stat.failures, 1, "claude:sonnet had 1 failure");
        assert_eq!(
            sonnet_stat.feedback_wins, 1,
            "claude:sonnet had 1 win feedback"
        );

        let gemini_stat = stats
            .iter()
            .find(|s| s.agent == "gemini")
            .expect("expected gemini stats");
        assert_eq!(gemini_stat.role, "architecture");
        assert_eq!(gemini_stat.runs, 1, "gemini had 1 observation");
        assert_eq!(gemini_stat.failures, 0, "gemini had 0 failures");
        assert_eq!(gemini_stat.feedback_wins, 0, "gemini had no win feedback");

        // ── best_agent_for_role ──────────────────────────────────────────────
        // claude:sonnet has 1 win feedback (boosted numerator) so it should
        // score higher despite 1 failure. The scoring formula:
        //   success_rate = (wins + 1) / (attempts + 2)  [Laplace smoothing]
        //   score = success_rate - speed_penalty - timeout_penalty
        //
        // claude:sonnet: runs=2, failures=1, fb_wins=1, fb_losses=0
        //   wins = (2-1) + 1 = 2, attempts = 2+1+0 = 3
        //   sr = (2+1)/(3+2) = 0.6, speed = 5000/120000*0.15 ≈ 0.00625, t/o=0
        //   score ≈ 0.594
        //
        // gemini: runs=1, failures=0, fb_wins=0, fb_losses=0
        //   wins = (1-0) + 0 = 1, attempts = 1+0+0 = 1
        //   sr = (1+1)/(1+2) = 0.667, speed = 4000/120000*0.15 ≈ 0.005
        //   score ≈ 0.662
        //
        // gemini's score is higher (~0.662 vs ~0.594) due to lower failure
        // rate and fewer penalty terms. With this seed, gemini wins.
        let best = repo.best_agent_for_role("architecture").unwrap();
        assert_eq!(
            best, "gemini",
            "with this telemetry seed, gemini scores higher for architecture"
        );

        // ── recommendations ──────────────────────────────────────────────────
        let recs = repo.recommendations().unwrap();
        assert_eq!(recs.len(), RECOMMENDATION_ROLES.len());

        // The architecture recommendation must match best_agent_for_role.
        let arch_rec = recs
            .iter()
            .find(|r| r.role == "architecture")
            .expect("architecture must appear in recommendations");
        assert_eq!(arch_rec.agent, "gemini");

        // All other roles have no telemetry → fall back to defaults.
        let hardening_rec = recs
            .iter()
            .find(|r| r.role == "hardening")
            .expect("hardening must appear");
        assert_eq!(
            hardening_rec.agent, "claude:sonnet",
            "hardening defaults to claude:sonnet"
        );

        let manager_rec = recs
            .iter()
            .find(|r| r.role == "manager")
            .expect("manager must appear");
        assert_eq!(
            manager_rec.agent, "claude:sonnet",
            "manager defaults to claude:sonnet"
        );
    }

    // ── Two Gate-3 instantiations (the parity proof) ─────────────────────────

    #[test]
    fn routing_memory_repo_contract_mem() {
        // Empty-state contract against MemTelemetryRepo.
        routing_memory_repo_contract(RoutingMemory::new(MemTelemetryRepo::new()));
    }

    #[test]
    fn routing_memory_repo_contract_file() {
        // Empty-state contract against FileTelemetryRepo (tempdir).
        let dir = TestDir::new("contract");
        routing_memory_repo_contract(RoutingMemory::new(FileTelemetryRepo::new(
            dir.path().to_path_buf(),
        )));
        // TestDir::drop cleans up the temp directory.
    }

    #[test]
    fn routing_memory_repo_seeded_contract_mem() {
        // Seeded-data math assertions on MemTelemetryRepo.
        routing_memory_repo_contract_seeded(MemTelemetryRepo::new());
    }

    #[test]
    fn routing_memory_repo_seeded_contract_file() {
        // Seeded-data math assertions on FileTelemetryRepo — file-backend parity.
        let dir = TestDir::new("seeded");
        routing_memory_repo_contract_seeded(FileTelemetryRepo::new(dir.path().to_path_buf()));
    }
}
