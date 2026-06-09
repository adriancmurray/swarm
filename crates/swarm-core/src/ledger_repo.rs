//! `LedgerRepo` trait + task-ledger types (T2 of the metadirector
//! virtual-context design).
//!
//! See `docs/agents/metadirector-virtual-context.md`. The ledger is an
//! append-only event log of task snapshots; current state is the latest
//! snapshot per task `id` (see [`fold_tasks`]). Per the spec (lines 53-55) a
//! task may only be trusted at `verified_done` when it carries a validation
//! anchor — see [`LedgerTask::is_verified`] / [`LedgerTask::is_unverified_done`].
//! The loud write-path enforcement of that invariant lives at the CLI layer.

use serde::{Deserialize, Serialize};

use crate::error::RepoError;

pub const LEDGER_TASK_SCHEMA: &str = "agent-swarm/ledger-task/v1";

/// Lifecycle status of a ledger task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LedgerStatus {
    Open,
    Claimed,
    ClaimedDone,
    VerifiedDone,
}

impl LedgerStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            LedgerStatus::Open => "open",
            LedgerStatus::Claimed => "claimed",
            LedgerStatus::ClaimedDone => "claimed_done",
            LedgerStatus::VerifiedDone => "verified_done",
        }
    }

    pub fn parse(token: &str) -> Option<Self> {
        match token {
            "open" => Some(LedgerStatus::Open),
            "claimed" => Some(LedgerStatus::Claimed),
            "claimed_done" => Some(LedgerStatus::ClaimedDone),
            "verified_done" => Some(LedgerStatus::VerifiedDone),
            _ => None,
        }
    }

    /// Active tasks still occupy the metadirector's working set — everything
    /// that is not `verified_done`.
    pub fn is_active(self) -> bool {
        !matches!(self, LedgerStatus::VerifiedDone)
    }
}

/// A single task row. Append-only: a status change appends a new snapshot with
/// the same `id`; [`fold_tasks`] collapses to the latest per id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedgerTask {
    pub schema: String,
    pub id: String,
    pub intent: String,
    pub status: LedgerStatus,
    #[serde(default)]
    pub owner_agent: Option<String>,
    #[serde(default)]
    pub depends_on: Vec<String>,
    pub created_at_ms: u128,
    #[serde(default)]
    pub closed_at_ms: Option<u128>,
    #[serde(default)]
    pub validation_anchor: Option<String>,
    #[serde(default)]
    pub synthesis_depth: u32,
}

impl LedgerTask {
    /// A fresh `open` task with no owner, dependencies, or anchor.
    pub fn new(id: impl Into<String>, intent: impl Into<String>, created_at_ms: u128) -> Self {
        Self {
            schema: LEDGER_TASK_SCHEMA.to_string(),
            id: id.into(),
            intent: intent.into(),
            status: LedgerStatus::Open,
            owner_agent: None,
            depends_on: Vec::new(),
            created_at_ms,
            closed_at_ms: None,
            validation_anchor: None,
            synthesis_depth: 0,
        }
    }

    /// Spec lines 53-55: `verified_done` is only trustworthy with an anchor.
    pub fn is_verified(&self) -> bool {
        self.status == LedgerStatus::VerifiedDone && self.has_anchor()
    }

    /// A task reported done but lacking a validation anchor — surfaced as
    /// UNVERIFIED rather than trusted.
    pub fn is_unverified_done(&self) -> bool {
        matches!(
            self.status,
            LedgerStatus::ClaimedDone | LedgerStatus::VerifiedDone
        ) && !self.has_anchor()
    }

    fn has_anchor(&self) -> bool {
        self.validation_anchor
            .as_deref()
            .map(|a| !a.trim().is_empty())
            .unwrap_or(false)
    }
}

/// Collapse an append-only snapshot log to current state: the latest snapshot
/// per `id`, preserving first-seen order.
pub fn fold_tasks(snapshots: Vec<LedgerTask>) -> Vec<LedgerTask> {
    use std::collections::HashMap;
    let mut order: Vec<String> = Vec::new();
    let mut latest: HashMap<String, LedgerTask> = HashMap::new();
    for task in snapshots {
        if !latest.contains_key(&task.id) {
            order.push(task.id.clone());
        }
        latest.insert(task.id.clone(), task);
    }
    order
        .into_iter()
        .filter_map(|id| latest.remove(&id))
        .collect()
}

// ── LedgerRepo trait ──────────────────────────────────────────────────────────

/// Append-only task ledger (T2). Writes append a snapshot; [`LedgerRepo::tasks`]
/// returns current state (latest snapshot per id, in first-seen order).
pub trait LedgerRepo: Send + Sync {
    /// Append a task snapshot. Append-only — never mutates prior rows.
    fn record_task(&self, task: LedgerTask) -> Result<(), RepoError>;

    /// Current state: latest snapshot per id, first-seen order.
    fn tasks(&self) -> Result<Vec<LedgerTask>, RepoError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_str_roundtrips() {
        for status in [
            LedgerStatus::Open,
            LedgerStatus::Claimed,
            LedgerStatus::ClaimedDone,
            LedgerStatus::VerifiedDone,
        ] {
            assert_eq!(LedgerStatus::parse(status.as_str()), Some(status));
        }
        assert_eq!(LedgerStatus::parse("bogus"), None);
    }

    #[test]
    fn status_active_excludes_only_verified_done() {
        assert!(LedgerStatus::Open.is_active());
        assert!(LedgerStatus::Claimed.is_active());
        assert!(LedgerStatus::ClaimedDone.is_active());
        assert!(!LedgerStatus::VerifiedDone.is_active());
    }

    #[test]
    fn fold_keeps_latest_per_id_in_first_seen_order() {
        let mut t2_claimed = LedgerTask::new("t-2", "second", 2);
        t2_claimed.status = LedgerStatus::Claimed;
        let folded = fold_tasks(vec![
            LedgerTask::new("t-1", "first", 1),
            LedgerTask::new("t-2", "second", 2),
            LedgerTask::new("t-3", "third", 3),
            t2_claimed,
        ]);
        assert_eq!(folded.len(), 3);
        assert_eq!(folded[0].id, "t-1");
        assert_eq!(
            folded[1].id, "t-2",
            "first-seen order is preserved on update"
        );
        assert_eq!(folded[2].id, "t-3");
        assert_eq!(folded[1].status, LedgerStatus::Claimed);
    }

    #[test]
    fn verified_requires_non_empty_anchor() {
        let mut task = LedgerTask::new("t-1", "intent", 1);
        task.status = LedgerStatus::VerifiedDone;
        assert!(
            !task.is_verified(),
            "verified_done without anchor is not verified"
        );
        assert!(task.is_unverified_done());

        task.validation_anchor = Some("  ".to_string());
        assert!(!task.is_verified(), "whitespace anchor does not count");

        task.validation_anchor = Some("test:foo".to_string());
        assert!(task.is_verified());
        assert!(!task.is_unverified_done());
    }
}
