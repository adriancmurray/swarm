//! Package and layer-report wire types.
//!
//! `LayerReportEnvelope` is the `agent-swarm/layer-report/v1` record appended
//! to `layer-reports.jsonl` alongside the human-readable `.md` file.

/// An `agent-swarm/layer-report/v1` envelope persisted to `layer-reports.jsonl`.
///
/// Each entry records one agent layer's contribution for a discussion round.
/// The full text is stored in a separate `.md` file; `preview` and `file`
/// are stored inline for fast list-mode rendering.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LayerReportEnvelope {
    pub schema: String,
    pub ts_ms: u128,
    pub session_id: String,
    pub layer: String,
    pub role: String,
    pub agent: String,
    pub parent_role: Option<String>,
    pub status: String,
    pub preview: String,
    pub file: String,
    pub text_bytes: usize,
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Wire-equivalence fixture: `LayerReportEnvelope` with null parent_role.
    const FIXTURE_LAYER_REPORT_NO_PARENT: &str = r#"{"schema":"agent-swarm/layer-report/v1","ts_ms":1780000005000,"session_id":"session-fixture-completed","layer":"qa","role":"qa","agent":"claude:sonnet","parent_role":null,"status":"completed","preview":"The implementation looks correct.","file":"layer-reports/1780000005000-qa-qa.md","text_bytes":512}"#;

    /// Wire-equivalence fixture: `LayerReportEnvelope` with parent_role set.
    const FIXTURE_LAYER_REPORT_WITH_PARENT: &str = r#"{"schema":"agent-swarm/layer-report/v1","ts_ms":1780000006000,"session_id":"session-fixture-completed","layer":"implementation","role":"coder","agent":"codex","parent_role":"manager","status":"completed","preview":"Implemented the feature.","file":"layer-reports/1780000006000-implementation-coder.md","text_bytes":1024}"#;

    #[test]
    fn lockbox_layer_report_no_parent_parses() {
        let report: LayerReportEnvelope = serde_json::from_str(FIXTURE_LAYER_REPORT_NO_PARENT)
            .expect("layer report fixture must parse");
        assert_eq!(report.schema, "agent-swarm/layer-report/v1");
        assert_eq!(report.layer, "qa");
        assert!(report.parent_role.is_none());
    }

    #[test]
    fn lockbox_layer_report_no_parent_byte_identical() {
        let report: LayerReportEnvelope =
            serde_json::from_str(FIXTURE_LAYER_REPORT_NO_PARENT).unwrap();
        let re_encoded = serde_json::to_string(&report).unwrap();
        assert_eq!(
            re_encoded, FIXTURE_LAYER_REPORT_NO_PARENT,
            "LayerReportEnvelope (null parent) must re-serialize byte-identically"
        );
    }

    #[test]
    fn lockbox_layer_report_with_parent_byte_identical() {
        let report: LayerReportEnvelope =
            serde_json::from_str(FIXTURE_LAYER_REPORT_WITH_PARENT).unwrap();
        assert_eq!(report.parent_role.as_deref(), Some("manager"));
        let re_encoded = serde_json::to_string(&report).unwrap();
        assert_eq!(
            re_encoded, FIXTURE_LAYER_REPORT_WITH_PARENT,
            "LayerReportEnvelope (with parent) must re-serialize byte-identically"
        );
    }
}
