//! Backend fallback-chain construction and the retry/fallback state machine.
//!
//! All of this is pure and unit-tested. The spawning driver
//! (`executor::execute_with_fallback`) walks a chain from [`build_fallback_chain`]
//! and consults [`next_action`] after each attempt — keeping the sequencing
//! logic (where reliability bugs hide) testable without ever launching an agent.
//!
//! Policy, per the 2026-06-01 backend-reliability work: **retry once on ANY
//! failure, then fall to the next backend.** We deliberately do NOT classify
//! errors as transient-vs-hard — the failure that motivated this
//! (`"model not supported on this account"`) looked hard but was spurious under
//! concurrent load, and codex worked seconds later. Retry is cheap; a
//! string-matching classifier that drops codex on that error is not.

use crate::agent::AgentSpec;
use crate::args::parse_agent_spec_struct;
use crate::config::SwarmConfig;

/// Built-in fallback order when a role has neither a `[routes.<role>]` entry nor
/// a global `[reliability].fallback_chain`. Claude-led by default, with Codex as
/// the cross-backend fallback. Keep this backend-generic; config overrides it
/// entirely, and any other backend can be put first via `fallback_chain`.
const DEFAULT_FALLBACK_CHAIN: [&str; 2] = ["claude:sonnet", "codex"];

/// What to do after an attempt finishes. Pure output of [`next_action`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NextAction {
    /// Retry the current backend after sleeping `backoff_ms` (jittered).
    RetrySame { backoff_ms: u64 },
    /// Move to the next backend in the chain.
    FallbackNext,
    /// Stop — either the attempt succeeded, or the chain is exhausted.
    Done,
}

/// Builds the ordered backend chain for a worker: the primary (caller-chosen)
/// spec first, then the role's configured `[routes.<role>].preferred` list, or
/// the global `[reliability].fallback_chain` when the role has no route.
/// De-duplicated by backend (not model) — falling from `claude:sonnet` to
/// `claude:opus` is not a real fallback and would re-hit the same outage.
pub fn build_fallback_chain(
    role: &str,
    primary: &AgentSpec,
    config: &SwarmConfig,
) -> Vec<AgentSpec> {
    let mut chain = vec![primary.clone()];

    let route_preferred = config
        .routes
        .get(role)
        .map(|route| route.preferred.as_slice())
        .filter(|preferred| !preferred.is_empty());
    let global = config.reliability.fallback_chain.as_slice();
    let candidates: Vec<&str> = if let Some(preferred) = route_preferred {
        preferred.iter().map(String::as_str).collect()
    } else if !global.is_empty() {
        global.iter().map(String::as_str).collect()
    } else {
        DEFAULT_FALLBACK_CHAIN.to_vec()
    };

    for raw in candidates {
        if let Ok(spec) = parse_agent_spec_struct(raw) {
            if !chain.iter().any(|existing| same_backend(existing, &spec)) {
                chain.push(spec);
            }
        }
    }
    chain
}

/// Two specs collide if they share a backend, regardless of model — falling
/// back within the same backend buys no resilience.
fn same_backend(a: &AgentSpec, b: &AgentSpec) -> bool {
    a.agent == b.agent
}

/// Pure retry → fallback sequencing.
///
/// - `succeeded`: the attempt produced a usable result.
/// - `retries_used`: retries already spent on the CURRENT backend (0 on first try).
/// - `max_retries`: `[reliability].retry_attempts`.
/// - `chain_pos` / `chain_len`: position in the fallback chain.
#[allow(clippy::too_many_arguments)]
pub fn next_action(
    succeeded: bool,
    retries_used: u32,
    max_retries: u32,
    chain_pos: usize,
    chain_len: usize,
    base_backoff_ms: u64,
    role: &str,
) -> NextAction {
    if succeeded {
        return NextAction::Done;
    }
    if retries_used < max_retries {
        return NextAction::RetrySame {
            backoff_ms: backoff_with_jitter(base_backoff_ms, role, retries_used),
        };
    }
    if chain_pos + 1 < chain_len {
        return NextAction::FallbackNext;
    }
    NextAction::Done
}

/// Deterministic per-`(role, attempt)` jitter in `[base, base + 50%]`, so that
/// workers that fail simultaneously (the diagnosed thundering-herd) don't all
/// retry in lockstep and re-collide. Deterministic (FNV-1a over role+attempt)
/// to keep it dependency-free and reproducible — different roles spread apart,
/// which is all we need.
pub fn backoff_with_jitter(base_ms: u64, role: &str, attempt: u32) -> u64 {
    if base_ms == 0 {
        return 0;
    }
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in role.bytes() {
        hash = (hash ^ u64::from(byte)).wrapping_mul(0x100_0000_01b3);
    }
    hash = (hash ^ u64::from(attempt)).wrapping_mul(0x100_0000_01b3);
    let span = base_ms / 2 + 1;
    base_ms + (hash % span)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::AgentChoice;

    fn spec(raw: &str) -> AgentSpec {
        parse_agent_spec_struct(raw).unwrap()
    }

    fn config_with_route(role: &str, preferred: &[&str], global: &[&str]) -> SwarmConfig {
        let mut cfg = SwarmConfig::default();
        if !preferred.is_empty() {
            cfg.routes.insert(
                role.to_string(),
                crate::config::RouteConfig {
                    preferred: preferred.iter().map(|s| s.to_string()).collect(),
                },
            );
        }
        cfg.reliability.fallback_chain = global.iter().map(|s| s.to_string()).collect();
        cfg
    }

    #[test]
    fn chain_puts_primary_first_then_route_preferred() {
        let cfg = config_with_route("review", &["claude:sonnet", "codex"], &["claude"]);
        let chain = build_fallback_chain("review", &spec("auto"), &cfg);
        let backends: Vec<AgentChoice> = chain.iter().map(|s| s.agent).collect();
        assert_eq!(
            backends,
            vec![AgentChoice::Auto, AgentChoice::Claude, AgentChoice::Codex]
        );
    }

    #[test]
    fn chain_dedups_primary_backend_ignoring_model() {
        // primary codex:gpt-5.5 + a "codex" in the preferred list -> no duplicate.
        let cfg = config_with_route("impl", &["codex", "claude:sonnet"], &[]);
        let chain = build_fallback_chain("impl", &spec("codex:gpt-5.5"), &cfg);
        let backends: Vec<AgentChoice> = chain.iter().map(|s| s.agent).collect();
        assert_eq!(backends, vec![AgentChoice::Codex, AgentChoice::Claude]);
    }

    #[test]
    fn chain_falls_back_to_global_when_role_has_no_route() {
        let cfg = config_with_route("unmatched", &[], &["claude:sonnet", "codex"]);
        let chain = build_fallback_chain("some-other-role", &spec("auto"), &cfg);
        let backends: Vec<AgentChoice> = chain.iter().map(|s| s.agent).collect();
        assert_eq!(
            backends,
            vec![AgentChoice::Auto, AgentChoice::Claude, AgentChoice::Codex]
        );
    }

    #[test]
    fn chain_uses_built_in_default_with_no_config() {
        // No routes, no global fallback_chain -> the claude-led built-in default,
        // with the auto primary preserved at the front.
        let cfg = SwarmConfig::default();
        let chain = build_fallback_chain("any", &spec("auto"), &cfg);
        let backends: Vec<AgentChoice> = chain.iter().map(|s| s.agent).collect();
        assert_eq!(
            backends,
            vec![AgentChoice::Auto, AgentChoice::Claude, AgentChoice::Codex]
        );
    }

    #[test]
    fn next_action_done_on_success() {
        assert_eq!(next_action(true, 0, 1, 0, 3, 1500, "r"), NextAction::Done);
    }

    #[test]
    fn next_action_retries_same_while_attempts_remain() {
        match next_action(false, 0, 1, 0, 3, 1500, "review") {
            NextAction::RetrySame { backoff_ms } => assert!(backoff_ms >= 1500),
            other => panic!("expected RetrySame, got {other:?}"),
        }
    }

    #[test]
    fn next_action_falls_back_after_retries_exhausted() {
        // retries spent (retries_used == max) and another backend is available.
        assert_eq!(
            next_action(false, 1, 1, 0, 3, 1500, "review"),
            NextAction::FallbackNext
        );
    }

    #[test]
    fn next_action_done_when_chain_exhausted() {
        // failed on the last backend with no retries left -> give up.
        assert_eq!(
            next_action(false, 1, 1, 2, 3, 1500, "review"),
            NextAction::Done
        );
    }

    #[test]
    fn backoff_jitter_stays_within_band_and_varies_by_role() {
        let a = backoff_with_jitter(1500, "architecture", 0);
        let b = backoff_with_jitter(1500, "review", 0);
        for value in [a, b] {
            assert!((1500..=2250).contains(&value), "out of band: {value}");
        }
        assert_ne!(a, b, "distinct roles should not retry in lockstep");
        assert_eq!(backoff_with_jitter(0, "review", 0), 0);
    }
}
