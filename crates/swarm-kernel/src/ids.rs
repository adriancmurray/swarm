//! Typed identifier newtypes — re-exported from [`swarm_contracts::ids`].
//!
//! All call sites using `crate::ids::{JobId, SessionId, ProposalId, PresetId}`
//! continue to work unchanged; the types are now the canonical swarm-contracts
//! definitions (single source of truth, Phase-2 cutover).

pub use swarm_contracts::ids::{JobId, PresetId, ProposalId, SessionId};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_id_transparent_round_trip() {
        let id = JobId::from("job-abc123");
        let encoded = serde_json::to_string(&id).unwrap();
        // Must be a bare JSON string, not a wrapped object.
        assert_eq!(encoded, r#""job-abc123""#);
        let decoded: JobId = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, id);
    }

    #[test]
    fn session_id_transparent_round_trip() {
        let id = SessionId::from("session-deadbeef");
        let encoded = serde_json::to_string(&id).unwrap();
        assert_eq!(encoded, r#""session-deadbeef""#);
        let decoded: SessionId = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, id);
    }

    #[test]
    fn job_id_display_matches_inner_string() {
        let id = JobId::from("job-display-test");
        assert_eq!(id.to_string(), "job-display-test");
        assert_eq!(format!("{id}.prompt.md"), "job-display-test.prompt.md");
    }

    #[test]
    fn session_id_display_matches_inner_string() {
        let id = SessionId::from("session-display-test");
        assert_eq!(id.to_string(), "session-display-test");
    }

    #[test]
    fn job_id_from_string_and_str_are_equivalent() {
        let from_str = JobId::from("job-xyz");
        let from_string = JobId::from("job-xyz".to_string());
        assert_eq!(from_str, from_string);
    }

    #[test]
    fn session_id_from_string_and_str_are_equivalent() {
        let from_str = SessionId::from("session-xyz");
        let from_string = SessionId::from("session-xyz".to_string());
        assert_eq!(from_str, from_string);
    }

    #[test]
    fn proposal_id_transparent_round_trip() {
        let id = ProposalId::from("proposal-abc");
        let encoded = serde_json::to_string(&id).unwrap();
        // Must be a bare JSON string, not a wrapped object.
        assert_eq!(encoded, r#""proposal-abc""#);
        let decoded: ProposalId = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, id);
    }

    #[test]
    fn proposal_id_display_matches_inner_string() {
        let id = ProposalId::from("proposal-display-test");
        assert_eq!(id.to_string(), "proposal-display-test");
    }

    #[test]
    fn proposal_id_from_string_and_str_are_equivalent() {
        let from_str = ProposalId::from("proposal-xyz");
        let from_string = ProposalId::from("proposal-xyz".to_string());
        assert_eq!(from_str, from_string);
    }

    #[test]
    fn preset_id_transparent_round_trip() {
        let id = PresetId::from("architecture-council");
        let encoded = serde_json::to_string(&id).unwrap();
        // Must be a bare JSON string, not a wrapped object.
        assert_eq!(encoded, r#""architecture-council""#);
        let decoded: PresetId = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, id);
    }

    #[test]
    fn preset_id_display_matches_inner_string() {
        let id = PresetId::from("ui-polish");
        assert_eq!(id.to_string(), "ui-polish");
    }

    #[test]
    fn preset_id_from_string_and_str_are_equivalent() {
        let from_str = PresetId::from("codebase-audit");
        let from_string = PresetId::from("codebase-audit".to_string());
        assert_eq!(from_str, from_string);
    }
}
