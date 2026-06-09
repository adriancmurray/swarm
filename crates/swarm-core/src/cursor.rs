//! `Cursor` — opaque resumption token for `EventRepo::events_since`.
//!
//! Moved from `agent-swarm::repos` in P5-S1 (behavior-preserving copy).
//!
//! # Visibility change vs original
//!
//! `Cursor::new` and `Cursor::get` were `pub(crate)` in `agent-swarm` because
//! only the in-crate impls needed them. Now that the trait lives here and the
//! impls stay in `agent-swarm` (until P5-S2), both accessors must be `pub` so
//! `FileEventRepo` and `MemEventRepo` in `agent-swarm` can still construct and
//! read cursors. Council ruling (architecture=codex, review=claude:sonnet):
//! this is policy enforcement, not a type invariant, and `swarm-core` is
//! an internal crate with no published semver concern.

// ── Cursor ───────────────────────────────────────────────────────────────────

/// Opaque resumption token for `EventRepo::events_since`.
///
/// Each backend interprets the inner `u64` naturally:
/// - JSONL backend: byte offset of the last confirmed complete `\n`
/// - In-memory backend: Vec index of the last returned event
///
/// `Cursor::start()` (== `Cursor(0)`) means "no events seen — return from
/// the beginning." Cursors are per-session and per-instance. Never share
/// across sessions. Never serialized into event JSON (YAGNI).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Cursor(u64);

impl Cursor {
    /// The initial cursor value — pass to `events_since` to start from
    /// the beginning of the log.
    pub fn start() -> Self {
        Self(0)
    }

    /// Construct a cursor from a raw backend position value.
    ///
    /// Promoted to `pub` (was `pub(crate)`) so that `FileEventRepo` and
    /// `MemEventRepo` in `agent-swarm` can construct cursors after P5-S1.
    pub fn new(v: u64) -> Self {
        Self(v)
    }

    /// Extract the raw backend position value.
    ///
    /// Promoted to `pub` (was `pub(crate)`) so that `FileEventRepo` in
    /// `agent-swarm` can read byte offsets after P5-S1.
    pub fn get(self) -> u64 {
        self.0
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn cursor_start_is_zero() {
        assert_eq!(Cursor::start().get(), 0);
    }

    #[test]
    fn cursor_new_round_trips() {
        assert_eq!(Cursor::new(42).get(), 42);
    }

    #[test]
    fn cursor_ordering() {
        assert!(Cursor::new(1) > Cursor::start());
        assert!(Cursor::new(100) > Cursor::new(99));
        assert_eq!(Cursor::new(5), Cursor::new(5));
    }

    #[test]
    fn cursor_copy_and_hash() {
        let a = Cursor::new(7);
        let b = a; // Copy — must compile without clone()
        let mut set = HashSet::new();
        set.insert(a);
        assert!(set.contains(&b));
    }
}
