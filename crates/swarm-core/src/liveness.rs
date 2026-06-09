//! `ProcessLiveness` trait + test doubles `NeverAlive` / `AlwaysAlive`.
//!
//! Moved from `agent-swarm::repos` in P5-S1 (behavior-preserving copy).
//!
//! `OsProcessLiveness` is NOT moved — it calls `crate::process::process_is_alive`
//! which is agent-swarm-local. It stays in `agent-swarm::repos::OsProcessLiveness`
//! and will move to `swarm-store` in P5-S3.

// ── ProcessLiveness ──────────────────────────────────────────────────────────

/// Injected dependency for liveness checks.
///
/// Production code uses `OsProcessLiveness` (remains in `agent-swarm`);
/// test doubles use `NeverAlive` or `AlwaysAlive` to control outcomes without
/// touching the filesystem or OS.
pub trait ProcessLiveness: Send + Sync {
    fn is_alive(&self, pid: u32) -> bool;
}

// ── NeverAlive ───────────────────────────────────────────────────────────────

/// Test double that always returns `false` — simulates every process dead.
pub struct NeverAlive;

impl ProcessLiveness for NeverAlive {
    fn is_alive(&self, _pid: u32) -> bool {
        false
    }
}

// ── AlwaysAlive ──────────────────────────────────────────────────────────────

/// Test double that always returns `true` — simulates every process alive.
pub struct AlwaysAlive;

impl ProcessLiveness for AlwaysAlive {
    fn is_alive(&self, _pid: u32) -> bool {
        true
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn never_alive_always_false() {
        assert!(!NeverAlive.is_alive(0));
        assert!(!NeverAlive.is_alive(1));
        assert!(!NeverAlive.is_alive(u32::MAX));
    }

    #[test]
    fn always_alive_always_true() {
        assert!(AlwaysAlive.is_alive(0));
        assert!(AlwaysAlive.is_alive(1));
        assert!(AlwaysAlive.is_alive(u32::MAX));
    }
}
