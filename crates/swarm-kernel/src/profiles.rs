use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct AgentProfile {
    pub id: &'static str,
    pub title: &'static str,
    pub roles: &'static [&'static str],
    pub purpose: &'static str,
    pub default_agent: &'static str,
    pub helpers: &'static [ProfileHelper],
    pub automation_hooks: &'static [&'static str],
    pub deterministic_checks: &'static [&'static str],
}

#[derive(Debug, Clone, Serialize)]
pub struct ProfileHelper {
    pub role: &'static str,
    pub agent: &'static str,
    pub purpose: &'static str,
}

pub fn profiles_json() -> serde_json::Value {
    serde_json::json!({
        "schema": "agent-swarm/profiles/v1",
        "profiles": profiles()
    })
}

pub fn automation_hooks_json() -> serde_json::Value {
    serde_json::json!({
        "schema": "agent-swarm/automation-hooks/v1",
        "policy": {
            "llm_in_path": false,
            "host_only": true,
            "description": "Hooks are deterministic host capabilities that can be offered to profiles without granting agents arbitrary tools."
        },
        "hooks": [
            {
                "id": "context-map",
                "title": "Context Map",
                "purpose": "Collect codebase context, symbols, and related files before prompting agents.",
                "suggested_profiles": ["systems-architect", "code-simplifier"],
                "deterministic": true
            },
            {
                "id": "browser-screenshot",
                "title": "Browser Screenshot",
                "purpose": "Capture the local graph/dashboard state for visual QA.",
                "suggested_profiles": ["frontend-polisher"],
                "deterministic": true
            },
            {
                "id": "overflow-scan",
                "title": "Overflow Scan",
                "purpose": "Check rendered DOM for unintended scroll and clipped text.",
                "suggested_profiles": ["frontend-polisher"],
                "deterministic": true
            },
            {
                "id": "schema-extract",
                "title": "Schema Extract",
                "purpose": "Generate a contract inventory from CLI/MCP/static manifests.",
                "suggested_profiles": ["docs-cartographer", "systems-architect"],
                "deterministic": true
            },
            {
                "id": "test-target-suggest",
                "title": "Test Target Suggest",
                "purpose": "Map touched files to likely validation commands.",
                "suggested_profiles": ["code-simplifier", "harness-hardener"],
                "deterministic": true
            }
        ]
    })
}

pub fn profile_for_role(role: &str) -> AgentProfile {
    profile_by_id_or_role(role).unwrap_or_else(|| profiles()[0].clone())
}

pub fn profile_by_id_or_role(role: &str) -> Option<AgentProfile> {
    let normalized = normalize(role);
    profiles().into_iter().find(|profile| {
        normalize(profile.id) == normalized
            || normalize(profile.title) == normalized
            || profile
                .roles
                .iter()
                .any(|candidate| normalize(candidate) == normalized)
    })
}

pub fn profile_id_for_role(role: &str) -> &'static str {
    profile_for_role(role).id
}

pub fn helpers_for_role(role: &str) -> &'static [ProfileHelper] {
    profile_for_role(role).helpers
}

pub fn profiles() -> Vec<AgentProfile> {
    vec![
        AgentProfile {
            id: "systems-architect",
            title: "Systems Architect",
            roles: &["architecture", "systems", "migration", "tradeoffs", "implementation-plan"],
            purpose: "Map boundaries, risks, contracts, and staged implementation order.",
            default_agent: "gemini",
            helpers: &[
                ProfileHelper {
                    role: "context-scout",
                    agent: "gemini",
                    purpose: "Collect relevant files, contracts, and prior decisions without editing.",
                },
                ProfileHelper {
                    role: "risk-check",
                    agent: "claude:sonnet",
                    purpose: "Name coupling, migration, and failure-mode risks before the main turn.",
                },
            ],
            automation_hooks: &["context-map", "contract-diff", "dependency-scan"],
            deterministic_checks: &["schema documented", "migration path named", "rollback path named"],
        },
        AgentProfile {
            id: "gemini-large-context-manager",
            title: "Gemini Large Context Manager",
            roles: &[
                "large-context",
                "wide-context",
                "gemini-manager",
                "metadirector",
                "context-synthesis",
            ],
            purpose: "Use Gemini for wide-context intake and packetized synthesis while preserving cited, compact manager decisions.",
            default_agent: "gemini",
            helpers: &[
                ProfileHelper {
                    role: "evidence-scout",
                    agent: "gemini",
                    purpose: "Map broad source and session context into cited packets without making final decisions.",
                },
                ProfileHelper {
                    role: "quality-verifier",
                    agent: "deepseek:deepseek-v4-pro",
                    purpose: "Check the Gemini synthesis against the evidence map and flag uncited or stale claims.",
                },
                ProfileHelper {
                    role: "risk-check",
                    agent: "claude:haiku",
                    purpose: "Name escalation triggers, missing tests, and places where a larger verifier is still warranted.",
                },
            ],
            automation_hooks: &[
                "context-map",
                "schema-extract",
                "test-target-suggest",
                "stale-session-scan",
            ],
            deterministic_checks: &[
                "source map present",
                "uncited claims rejected",
                "next slice bounded",
                "verifier named for risky changes",
            ],
        },
        AgentProfile {
            id: "harness-hardener",
            title: "Harness Hardener",
            roles: &["hardening", "harness-hardening", "security", "runtime", "root-cause"],
            purpose: "Find process, filesystem, timeout, permission, and recovery failures.",
            default_agent: "claude:sonnet",
            helpers: &[
                ProfileHelper {
                    role: "failure-repro",
                    agent: "gemini",
                    purpose: "Search for concrete reproduction paths and stale-state hazards.",
                },
            ],
            automation_hooks: &["error-classifier", "process-tree-check", "stale-session-scan"],
            deterministic_checks: &["failure class named", "human-visible recovery named", "timeout bounded"],
        },
        AgentProfile {
            id: "frontend-polisher",
            title: "Frontend Polisher",
            roles: &[
                "product-design",
                "motion-accessibility",
                "component-architecture",
                "visual-state",
                "motion-system",
                "accessibility",
            ],
            purpose: "Improve visual hierarchy, motion semantics, accessibility, and component boundaries.",
            default_agent: "claude:sonnet",
            helpers: &[
                ProfileHelper {
                    role: "motion-scout",
                    agent: "gemini",
                    purpose: "Propose state-driven animation and reduced-motion behavior.",
                },
                ProfileHelper {
                    role: "accessibility-check",
                    agent: "claude:sonnet",
                    purpose: "Check keyboard, focus, contrast, and screen-reader fallbacks.",
                },
            ],
            automation_hooks: &["browser-screenshot", "overflow-scan", "motion-reduced-check"],
            deterministic_checks: &["screenshot verified", "no unintended overflow", "reduced motion handled"],
        },
        AgentProfile {
            id: "code-simplifier",
            title: "Code Simplifier",
            roles: &["simplify", "decomposition", "code-quality", "review", "tests"],
            purpose: "Reduce god files, duplicated logic, and fragile tests while keeping behavior stable.",
            default_agent: "claude:sonnet",
            helpers: &[
                ProfileHelper {
                    role: "surface-map",
                    agent: "gemini",
                    purpose: "List high-coupling functions and the smallest testable extraction.",
                },
            ],
            automation_hooks: &["symbol-map", "test-target-suggest", "public-contract-freeze"],
            deterministic_checks: &["tests named", "public behavior unchanged", "diff scope bounded"],
        },
        AgentProfile {
            id: "docs-cartographer",
            title: "Docs Cartographer",
            roles: &["api-docs", "examples", "docs"],
            purpose: "Turn implementation contracts into readable API docs and examples.",
            default_agent: "claude:sonnet",
            helpers: &[
                ProfileHelper {
                    role: "example-scout",
                    agent: "gemini",
                    purpose: "Find likely consumer examples and missing contract details.",
                },
            ],
            automation_hooks: &["schema-extract", "readme-sync", "example-smoke"],
            deterministic_checks: &["schema path linked", "example runnable", "MCP/CLI parity stated"],
        },
    ]
}

fn normalize(value: &str) -> String {
    value.trim().to_ascii_lowercase().replace(['_', ' '], "-")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_map_hook_id_is_generic_and_referenced_by_profiles() {
        let hooks = automation_hooks_json();
        let ids: Vec<&str> = hooks["hooks"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|hook| hook["id"].as_str())
            .collect();
        assert!(ids.contains(&"context-map"), "hook ids: {ids:?}");

        let architect = profile_by_id_or_role("systems-architect").unwrap();
        assert!(architect.automation_hooks.contains(&"context-map"));
    }

    #[test]
    fn gemini_large_context_manager_profile_is_discoverable() {
        let profile = profile_by_id_or_role("wide-context").unwrap();

        assert_eq!(profile.id, "gemini-large-context-manager");
        assert_eq!(profile.default_agent, "gemini");
        assert!(profile
            .helpers
            .iter()
            .any(|helper| helper.agent == "deepseek:deepseek-v4-pro"));
        assert!(profile
            .deterministic_checks
            .contains(&"uncited claims rejected"));
    }
}
