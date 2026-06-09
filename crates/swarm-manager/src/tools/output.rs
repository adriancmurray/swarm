//! Tool-output post-processing: secret scrubbing and head/tail truncation.

use regex::Regex;

pub(crate) const DEFAULT_MAX_OUTPUT_CHARS: usize = 20_000;

pub(crate) fn prepare_tool_output(raw: &str, max_chars: usize) -> String {
    let scrubbed = scrub_secrets(raw);
    truncate_with_head_tail(&scrubbed, max_chars)
}

pub(crate) fn scrub_secrets(raw: &str) -> String {
    let mut out = raw.to_string();
    let patterns = [
        (
            r#"(?i)(api[_-]?key|token|secret|password)\s*[:=]\s*["']?([A-Za-z0-9_\-./+=]{12,})"#,
            "$1=[REDACTED]",
        ),
        (
            r#"(?i)(authorization:\s*bearer\s+)([A-Za-z0-9_\-./+=]{12,})"#,
            "$1[REDACTED]",
        ),
        (r#"sk-[A-Za-z0-9_\-]{20,}"#, "[REDACTED_API_KEY]"),
        (r#"gh[pousr]_[A-Za-z0-9_]{20,}"#, "[REDACTED_GITHUB_TOKEN]"),
    ];

    for (pattern, replacement) in patterns {
        if let Ok(regex) = Regex::new(pattern) {
            out = regex.replace_all(&out, replacement).to_string();
        }
    }

    out
}

fn truncate_with_head_tail(raw: &str, max_chars: usize) -> String {
    let max_chars = max_chars.max(256);
    let total_chars = raw.chars().count();
    if total_chars <= max_chars {
        return raw.to_string();
    }

    let marker_budget = 160.min(max_chars / 3);
    let visible_budget = max_chars.saturating_sub(marker_budget).max(96);
    let head_chars = visible_budget / 2;
    let tail_chars = visible_budget - head_chars;
    let omitted = total_chars.saturating_sub(head_chars + tail_chars);

    let head: String = raw.chars().take(head_chars).collect();
    let tail: String = raw
        .chars()
        .rev()
        .take(tail_chars)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();

    format!(
        "{head}\n[output truncated: showing first {head_chars} and last {tail_chars} of {total_chars} chars; {omitted} chars omitted]\n{tail}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncation_preserves_head_tail_metadata() {
        let raw = format!("{}{}", "a".repeat(400), "z".repeat(400));
        let out = prepare_tool_output(&raw, 300);

        assert!(out.starts_with("aaa"));
        assert!(out.ends_with("zzz"));
        assert!(out.contains("[output truncated: showing first"));
        assert!(out.contains("chars omitted"));
    }

    #[test]
    fn scrub_secrets_removes_common_key_shapes() {
        let out = prepare_tool_output(
            "OPENAI_API_KEY=sk-testsecretsecretsecretsecret\nAuthorization: Bearer abcdefghijklmnopqrstuvwxyz",
            1_000,
        );

        assert!(!out.contains("sk-testsecret"));
        assert!(!out.contains("abcdefghijklmnopqrstuvwxyz"));
        assert!(out.contains("[REDACTED"));
    }
}
