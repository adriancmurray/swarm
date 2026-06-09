//! CLI/MCP text formatting helpers.

use swarm_store::job::JobRecord;

pub fn json_text(value: serde_json::Value) -> String {
    serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string())
}

pub fn job_status_line(record: &JobRecord) -> String {
    let pid = record
        .pid
        .map(|pid| pid.to_string())
        .unwrap_or_else(|| "-".to_string());
    let exit = record
        .exit_code
        .map(|code| code.to_string())
        .unwrap_or_else(|| "-".to_string());
    format!(
        "{}  {:10} {:6} {:7} pid={} exit={}  {}",
        record.id, record.status, record.agent, record.mode, pid, exit, record.prompt_preview
    )
}

pub fn print_job_status(record: &JobRecord) {
    println!("{}", job_status_line(record));
}

pub fn prompt_preview(prompt: &str) -> String {
    let normalized = prompt.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() <= 72 {
        normalized
    } else {
        let prefix: String = normalized.chars().take(69).collect();
        format!("{prefix}...")
    }
}

/// Compact and truncate a raw event value to `max` characters.
///
/// Moved from `synthesis.rs` so `context.rs` can use it without an up-import.
/// All callers in staying exec modules reach it via the agent-swarm shim:
/// `pub use swarm_kernel::format::preview_for_event;`
pub fn preview_for_event(value: &str, max: usize) -> String {
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
    use crate::job_types::{JobAgent, JobMode, JobStatus};

    fn job_record() -> JobRecord {
        JobRecord {
            id: "job-1".into(),
            status: JobStatus::Running,
            agent: JobAgent::Gemini,
            model: None,
            mode: JobMode::Agent,
            cwd: "/tmp/work".to_string(),
            prompt_preview: "Inspect the seam".to_string(),
            timeout_secs: 300,
            created_at_ms: 1,
            started_at_ms: Some(2),
            completed_at_ms: None,
            pid: Some(42),
            exit_code: None,
            prompt_path: "/tmp/prompt".to_string(),
            stdout_path: "/tmp/stdout".to_string(),
            stderr_path: "/tmp/stderr".to_string(),
            result_path: "/tmp/result".to_string(),
            allow_recursive_codex: false,
        }
    }

    #[test]
    fn json_text_pretty_prints_values() {
        let text = json_text(serde_json::json!({"b": [1, 2], "a": true}));

        assert!(text.contains('\n'));
        let reparsed: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(reparsed["a"], true);
        assert_eq!(reparsed["b"], serde_json::json!([1, 2]));
    }

    #[test]
    fn job_status_line_includes_core_fields_and_defaults() {
        let mut record = job_record();
        record.pid = None;
        record.exit_code = Some(124);

        let line = job_status_line(&record);

        assert!(line.contains("job-1"));
        assert!(line.contains("running"));
        assert!(line.contains("gemini"));
        assert!(line.contains("pid=-"));
        assert!(line.contains("exit=124"));
        assert!(line.contains("Inspect the seam"));
    }

    #[test]
    fn prompt_preview_normalizes_and_preserves_short_prompts() {
        assert_eq!(prompt_preview("  ask   a   scout  "), "ask a scout");
    }

    #[test]
    fn prompt_preview_preserves_exact_limit() {
        let prompt = "x".repeat(72);

        assert_eq!(prompt_preview(&prompt), prompt);
    }

    #[test]
    fn prompt_preview_truncates_long_prompts_to_72_chars() {
        let preview = prompt_preview(&"x".repeat(80));

        assert_eq!(preview.chars().count(), 72);
        assert!(preview.ends_with("..."));
    }

    #[test]
    fn preview_for_event_compacts_and_truncates() {
        // Relocated from synthesis.rs in P5-S2.5 (preview_for_event moved here).
        assert_eq!(preview_for_event("one\n  two\tthree", 20), "one two three");
        assert_eq!(preview_for_event("abcdef", 5), "ab...");
    }
}
