//! Workspace context gathering and scoring.

use std::fs;
use std::path::Path;

pub fn context_gather_json(
    cwd: &Path,
    query: &str,
    budget_tokens: u64,
) -> Result<serde_json::Value, String> {
    let root = cwd
        .canonicalize()
        .map_err(|err| format!("Error resolving cwd {}: {err}", cwd.display()))?;
    let terms = query
        .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_' && ch != '-')
        .map(|term| term.to_ascii_lowercase())
        .filter(|term| term.len() > 2)
        .collect::<Vec<_>>();
    let mut candidates = Vec::new();
    gather_context_files(&root, &root, &terms, &mut candidates, 0, 320)?;
    candidates.sort_by(|a, b| {
        b.get("score")
            .and_then(|value| value.as_i64())
            .cmp(&a.get("score").and_then(|value| value.as_i64()))
    });
    let approx_bytes = (budget_tokens as usize).saturating_mul(4).max(512);
    let mut used = 0usize;
    let mut selected = Vec::new();
    for mut candidate in candidates {
        let bytes = candidate
            .get("excerpt")
            .and_then(|value| value.as_str())
            .map(str::len)
            .unwrap_or(0);
        if used + bytes > approx_bytes && !selected.is_empty() {
            break;
        }
        used += bytes;
        selected.push(candidate.take());
        if selected.len() >= 24 {
            break;
        }
    }
    Ok(serde_json::json!({
        "schema": "agent-swarm/context-gather/v1",
        "cwd": root.display().to_string(),
        "query": query,
        "budget_tokens": budget_tokens,
        "symbols": selected,
        "truncated": used >= approx_bytes
    }))
}

fn gather_context_files(
    root: &Path,
    dir: &Path,
    terms: &[String],
    out: &mut Vec<serde_json::Value>,
    depth: usize,
    max_files: usize,
) -> Result<(), String> {
    if depth > 6 || out.len() >= max_files {
        return Ok(());
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        if out.len() >= max_files {
            break;
        }
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.')
            || matches!(
                name.as_str(),
                "node_modules" | "target" | "build" | ".svelte-kit" | "dist"
            )
        {
            continue;
        }
        if path.is_dir() {
            gather_context_files(root, &path, terms, out, depth + 1, max_files)?;
            continue;
        }
        if !is_context_file(&path) {
            continue;
        }
        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .display()
            .to_string();
        let path_lc = rel.to_ascii_lowercase();
        let mut score = terms
            .iter()
            .filter(|term| path_lc.contains(term.as_str()))
            .count() as i64
            * 4;
        let text = fs::read_to_string(&path).unwrap_or_default();
        let excerpt_source = text.chars().take(16_000).collect::<String>();
        let excerpt_lc = excerpt_source.to_ascii_lowercase();
        for term in terms {
            if excerpt_lc.contains(term) {
                score += 1;
            }
        }
        if score == 0 && !terms.is_empty() {
            continue;
        }
        out.push(serde_json::json!({
            "path": rel,
            "score": score,
            "excerpt": crate::format::preview_for_event(&excerpt_source, 900)
        }));
    }
    Ok(())
}

fn is_context_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|ext| ext.to_str()),
        Some(
            "rs" | "ts"
                | "tsx"
                | "js"
                | "svelte"
                | "dart"
                | "md"
                | "toml"
                | "yaml"
                | "yml"
                | "json"
        )
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

    fn temp_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "agent-swarm-context-{name}-{}-{}",
            swarm_store::store::now_ms(),
            TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).expect("create temp root");
        root
    }

    #[test]
    fn is_context_file_accepts_all_known_extensions() {
        for ext in [
            "rs", "ts", "tsx", "js", "svelte", "dart", "md", "toml", "yaml", "yml", "json",
        ] {
            assert!(is_context_file(Path::new(&format!("file.{ext}"))));
        }
    }

    #[test]
    fn is_context_file_rejects_unknown_extensions() {
        for file in ["Cargo.lock", "image.png", "binary", "archive.zip"] {
            assert!(!is_context_file(Path::new(file)));
        }
    }

    #[test]
    fn context_gather_json_returns_correct_schema_key() {
        let root = temp_root("schema");

        let value = context_gather_json(&root, "runtime", 256).expect("gather context");

        assert_eq!(
            value.get("schema").and_then(|value| value.as_str()),
            Some("agent-swarm/context-gather/v1")
        );
        assert_eq!(
            value.get("query").and_then(|value| value.as_str()),
            Some("runtime")
        );

        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn gather_context_files_empty_dir_returns_empty() {
        let root = temp_root("empty");
        let mut out = Vec::new();

        gather_context_files(&root, &root, &["runtime".to_string()], &mut out, 0, 320)
            .expect("gather context files");

        assert!(out.is_empty());
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn gather_context_files_skips_hidden_dirs() {
        let root = temp_root("hidden");
        fs::create_dir_all(root.join(".git")).expect("create hidden dir");
        fs::write(root.join(".git/needle.rs"), "needle").expect("write hidden file");
        let mut out = Vec::new();

        gather_context_files(&root, &root, &["needle".to_string()], &mut out, 0, 320)
            .expect("gather context files");

        assert!(out.is_empty());
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn gather_context_files_skips_excluded_dirs() {
        let root = temp_root("excluded");
        for dir in ["node_modules", "target", "build", ".svelte-kit", "dist"] {
            fs::create_dir_all(root.join(dir)).expect("create excluded dir");
            fs::write(root.join(dir).join("needle.rs"), "needle").expect("write excluded file");
        }
        let mut out = Vec::new();

        gather_context_files(&root, &root, &["needle".to_string()], &mut out, 0, 320)
            .expect("gather context files");

        assert!(out.is_empty());
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn gather_context_files_skips_non_context_files() {
        let root = temp_root("non-context");
        fs::write(root.join("needle.png"), "needle").expect("write unsupported file");
        let mut out = Vec::new();

        gather_context_files(&root, &root, &["needle".to_string()], &mut out, 0, 320)
            .expect("gather context files");

        assert!(out.is_empty());
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn gather_context_files_path_match_scores_4_per_term() {
        let root = temp_root("path-score");
        fs::write(root.join("runtime_agent.rs"), "").expect("write source");
        let mut out = Vec::new();

        gather_context_files(
            &root,
            &root,
            &["runtime".to_string(), "agent".to_string()],
            &mut out,
            0,
            320,
        )
        .expect("gather context files");

        assert_eq!(out.len(), 1);
        assert_eq!(
            out[0].get("score").and_then(|value| value.as_i64()),
            Some(8)
        );
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn gather_context_files_content_match_scores_1_per_term() {
        let root = temp_root("content-score");
        fs::write(root.join("notes.md"), "Runtime agent details").expect("write notes");
        let mut out = Vec::new();

        gather_context_files(
            &root,
            &root,
            &["runtime".to_string(), "agent".to_string()],
            &mut out,
            0,
            320,
        )
        .expect("gather context files");

        assert_eq!(out.len(), 1);
        assert_eq!(
            out[0].get("score").and_then(|value| value.as_i64()),
            Some(2)
        );
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn gather_context_files_zero_score_excluded_when_terms_nonempty() {
        let root = temp_root("zero-score");
        fs::write(root.join("notes.md"), "other content").expect("write notes");
        let mut out = Vec::new();

        gather_context_files(&root, &root, &["needle".to_string()], &mut out, 0, 320)
            .expect("gather context files");

        assert!(out.is_empty());
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn gather_context_files_includes_all_when_terms_empty() {
        let root = temp_root("empty-query");
        fs::write(root.join("a.rs"), "").expect("write a");
        fs::write(root.join("b.md"), "").expect("write b");
        let mut out = Vec::new();

        gather_context_files(&root, &root, &[], &mut out, 0, 320).expect("gather context files");

        assert_eq!(out.len(), 2);
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn gather_context_files_respects_max_files_cap() {
        let root = temp_root("max-files");
        for index in 0..3 {
            fs::write(root.join(format!("file-{index}.rs")), "").expect("write source");
        }
        let mut out = Vec::new();

        gather_context_files(&root, &root, &[], &mut out, 0, 2).expect("gather context files");

        assert_eq!(out.len(), 2);
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn gather_context_files_stops_at_depth_7() {
        let root = temp_root("depth");
        let mut deep = root.clone();
        for index in 1..=7 {
            deep = deep.join(format!("d{index}"));
        }
        fs::create_dir_all(&deep).expect("create deep dir");
        fs::write(deep.join("needle.rs"), "needle").expect("write deep source");
        let mut out = Vec::new();

        gather_context_files(&root, &root, &["needle".to_string()], &mut out, 0, 320)
            .expect("gather context files");

        assert!(out.is_empty());
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn context_gather_json_results_sorted_by_score_descending() {
        let root = temp_root("ranking");
        fs::create_dir_all(root.join("src")).expect("create src");
        fs::write(
            root.join("src/runtime_agent.rs"),
            "Runtime agent details live here.",
        )
        .expect("write matching source");
        fs::write(root.join("notes.md"), "agent notes only").expect("write notes");

        let value = context_gather_json(&root, "runtime agent", 256).expect("gather context");
        let symbols = value
            .get("symbols")
            .and_then(|value| value.as_array())
            .expect("symbols array");

        assert_eq!(
            symbols
                .first()
                .and_then(|symbol| symbol.get("path"))
                .and_then(|value| value.as_str()),
            Some("src/runtime_agent.rs")
        );
        assert!(
            symbols
                .first()
                .and_then(|symbol| symbol.get("score"))
                .and_then(|value| value.as_i64())
                .unwrap_or_default()
                >= 10
        );
        assert!(symbols
            .first()
            .and_then(|symbol| symbol.get("excerpt"))
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .contains("Runtime agent"));

        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn context_gather_json_budget_truncation_sets_truncated_flag() {
        let root = temp_root("budget");
        for index in 0..2 {
            fs::write(
                root.join(format!("needle-{index}.rs")),
                format!("needle {}", "x".repeat(700)),
            )
            .expect("write large source");
        }

        let value = context_gather_json(&root, "needle", 1).expect("gather context");
        let symbols = value
            .get("symbols")
            .and_then(|value| value.as_array())
            .expect("symbols array");

        assert_eq!(symbols.len(), 1);
        assert_eq!(
            value.get("truncated").and_then(|value| value.as_bool()),
            Some(true)
        );

        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn context_gather_json_caps_symbols_at_24() {
        let root = temp_root("symbol-cap");
        for index in 0..30 {
            fs::write(root.join(format!("file-{index}.rs")), "").expect("write source");
        }

        let value = context_gather_json(&root, "", 10_000).expect("gather context");
        let symbols = value
            .get("symbols")
            .and_then(|value| value.as_array())
            .expect("symbols array");

        assert_eq!(symbols.len(), 24);
        fs::remove_dir_all(root).ok();
    }
}
