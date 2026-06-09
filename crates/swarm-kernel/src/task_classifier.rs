use serde::Serialize;

pub const DEFAULT_CLASSIFIER_PROVIDER: &str = "mlx";
pub const DEFAULT_CLASSIFIER_MODEL: &str = "mlx-community/gemma-4-e2b-it-OptiQ-4bit";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaskClassification {
    pub task_type: &'static str,
    pub confidence: u8,
    pub roles: Vec<&'static str>,
    pub classifier: ClassifierInfo,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ClassifierInfo {
    pub provider: &'static str,
    pub model: &'static str,
    pub mode: &'static str,
    pub status: &'static str,
}

pub fn classify_task(task: &str) -> TaskClassification {
    let normalized = task.to_ascii_lowercase();
    let tokens = normalized
        .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_' && ch != '-')
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>();
    let (task_type, confidence, roles) = if any_keyword(
        &normalized,
        &tokens,
        &[
            "model",
            "provider",
            "deepseek",
            "gemma",
            "mlx",
            "ollama",
            "lm studio",
        ],
    ) {
        (
            "model-provider",
            84,
            vec!["architecture", "implementation-plan", "review"],
        )
    } else if any_keyword(
        &normalized,
        &tokens,
        &[
            "doc",
            "docs",
            "api",
            "readme",
            "contract",
            "install verification",
        ],
    ) {
        ("docs", 78, vec!["api-docs", "examples"])
    } else if any_keyword(
        &normalized,
        &tokens,
        &[
            "ui",
            "ux",
            "visual design",
            "product design",
            "flutter",
            "svelte",
            "layout",
            "animation",
            "wire",
            "node",
            "graph",
        ],
    ) {
        (
            "ui-design",
            86,
            vec![
                "product-design",
                "motion-accessibility",
                "component-architecture",
            ],
        )
    } else if any_keyword(
        &normalized,
        &tokens,
        &[
            "audit",
            "harden",
            "security",
            "risk",
            "review",
            "memory",
            "context",
            "compaction",
            "metadirector",
            "handoff",
            "token",
        ],
    ) {
        ("audit", 82, vec!["architecture", "simplify", "hardening"])
    } else {
        (
            "implementation",
            62,
            vec!["architecture", "implementation-plan", "review"],
        )
    };

    TaskClassification {
        task_type,
        confidence,
        roles,
        classifier: ClassifierInfo {
            provider: DEFAULT_CLASSIFIER_PROVIDER,
            model: DEFAULT_CLASSIFIER_MODEL,
            mode: "deterministic-rust-fallback",
            status: "deterministic-rust-fallback",
        },
    }
}

fn any_keyword(haystack: &str, tokens: &[&str], needles: &[&str]) -> bool {
    needles.iter().any(|needle| {
        if needle.len() <= 4 && needle.chars().all(|ch| ch.is_ascii_alphanumeric()) {
            tokens.iter().any(|token| token == needle)
        } else {
            haystack.contains(needle)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_ui_graph_tasks_for_design_roles() {
        let classification = classify_task("fix the graph node layout and wire animation");
        assert_eq!(classification.task_type, "ui-design");
        assert_eq!(
            classification.roles,
            vec![
                "product-design",
                "motion-accessibility",
                "component-architecture"
            ]
        );
        assert_eq!(classification.classifier.provider, "mlx");
        assert_eq!(classification.classifier.model, DEFAULT_CLASSIFIER_MODEL);
    }

    #[test]
    fn classifies_provider_model_tasks() {
        let classification = classify_task("wire DeepSeek and Gemma MLX providers");
        assert_eq!(classification.task_type, "model-provider");
        assert_eq!(
            classification.classifier.status,
            "deterministic-rust-fallback"
        );
    }

    #[test]
    fn short_ui_keyword_does_not_match_inside_rebuilds() {
        let classification = classify_task("Update install verification docs after rebuilds");
        assert_eq!(classification.task_type, "docs");
    }

    #[test]
    fn context_compaction_routes_to_audit() {
        let classification =
            classify_task("Design deterministic handoff packets for context compaction");
        assert_eq!(classification.task_type, "audit");
    }

    #[test]
    fn audit_action_verb_does_not_override_ui_boundary() {
        let classification =
            classify_task("Audit a feature that touches Flutter graph UI and Rust telemetry");
        assert_eq!(classification.task_type, "ui-design");
    }
}
