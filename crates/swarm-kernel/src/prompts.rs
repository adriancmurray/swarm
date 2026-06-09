//! Pure prompt-builder functions for audit and design session dispatch.
//!
//! Moved from `synthesis.rs` in P5-S2.5 so `args.rs` can call these without
//! an up-import into the exec layer.
//!
//! `peer_context()` is a private helper that reads an OPT-IN service registry
//! (`$SWARM_SERVICES_REGISTRY`) and package directory (`$SWARM_PACKAGES_DIR`)
//! at call-time; it is NOT pure `format!`. With neither env var set it returns
//! a static no-context line and touches no filesystem.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

pub const COMPACT_HANDOFF_CONTRACT: &str = "\
Compact handoff packet rules:
Do not narrate file reads, directory traversal, or tool plans.
Stay under 12 bullets total; each bullet under 24 words.
Required sections, in order: Findings, Risks, Steps, Blockers, Tests.
Findings: 1-4 bullets with citations when source was inspected.
Risks: 0-3 bullets.
Steps: 1-3 bullets naming exact files/functions when known.
Blockers: 0-3 bullets; write NEEDS_EVIDENCE with the missing anchor instead of guessing.
Tests: 1-3 bullets naming deterministic checks.
Stop after the Tests section. Do not append analysis, verdicts, preambles, or final answers outside the required sections.";

/// Build the prompt for a read-only codebase audit session.
pub fn build_audit_prompt(task: &str, focus: &str, cwd: &Path) -> String {
    format!(
        "Run a read-only codebase audit.\n\
         Focus: {focus}.\n\
         Working directory: {}.\n\n\
         Audit objective:\n{task}\n\n\
         Available local peer context:\n{}\n\n\
         Required output:\n\
         {COMPACT_HANDOFF_CONTRACT}\n\
         Within that contract, cover simplification/hardening opportunities, files or modules to inspect first, risks/tradeoffs/test gaps, and follow-up subagent or tool abilities that would improve future audits.\n\
         Do not edit files.",
        cwd.display(),
        peer_context()
    )
}

/// Build the prompt for a design-centered product review session.
pub fn build_design_prompt(task: &str, focus: &str, cwd: &Path) -> String {
    format!(
        "Run a design-centered product review and implementation planning session.\n\
         Focus: {focus}.\n\
         Working directory: {}.\n\n\
         Product context:\n\
         - Audience: technical operators watching local agent swarms, live processes, session history, documents, and tool output.\n\
         - Use case: understand what multiple agents are doing, compare sessions, inspect artifacts, and trust the system while work is in flight.\n\
         - Desired tone: polished desktop application, calm, precise, professional, with the restraint of Apple/Anthropic and the clarity of modern Google work surfaces.\n\n\
         Design objective:\n{task}\n\n\
         Design principles to apply:\n\
         - Establish clear hierarchy before decoration; elevation should clarify structure.\n\
         - Prefer dense but calm application layouts over marketing-style cards or oversized hero treatments.\n\
         - Motion should explain state changes quickly; stagger by data order, keep exits shorter than entrances, and respect reduced motion.\n\
         - Trackpad/touch interactions should feel native: pan, zoom, selection, and focus states must not fight scrolling.\n\
         - Avoid generic AI aesthetics: no purple-blue gradient dependence, vague glow, unexplained rails, or decorative charts.\n\
         - Validate visually with the actual browser target or a captured screenshot artifact before declaring polish done.\n\n\
         Available local peer context:\n{}\n\n\
         Required output:\n\
         {COMPACT_HANDOFF_CONTRACT}\n\
         Within that contract, cover visual direction, concrete component/files to inspect first, motion and interaction specs, accessibility/responsive/reduced-motion risks, and a QA checklist for browser or screenshot verification.\n\
         Do not edit files.",
        cwd.display(),
        peer_context()
    )
}

/// Static result returned by `peer_context()` when no registry env vars are
/// configured. Also asserted by tests.
const NO_PEER_CONTEXT: &str = "No peer registry configured; set SWARM_SERVICES_REGISTRY \
     and/or SWARM_PACKAGES_DIR to inject peer context.";

/// Read an OPT-IN service registry and package directory for context
/// injection into audit and design prompts, so the swarm knows which peer
/// agents and packages are installed.
///
/// Both reads are gated on env vars — `$SWARM_SERVICES_REGISTRY` (path to a
/// services JSON file) and `$SWARM_PACKAGES_DIR` (directory of package JSON
/// manifests). When neither is set, this returns [`NO_PEER_CONTEXT`] without
/// touching the filesystem. There is no default path.
///
/// This is NOT a pure function — it reads the filesystem at call-time when
/// the env vars are set. It is private to this module; callers use
/// `build_audit_prompt` / `build_design_prompt` as the public surface.
fn peer_context() -> String {
    let services_path = env::var_os("SWARM_SERVICES_REGISTRY").map(PathBuf::from);
    let packages_dir = env::var_os("SWARM_PACKAGES_DIR").map(PathBuf::from);
    if services_path.is_none() && packages_dir.is_none() {
        return NO_PEER_CONTEXT.to_string();
    }
    let mut lines = Vec::new();
    if let Some(services_path) = services_path {
        match fs::read_to_string(&services_path) {
            Ok(content) => {
                lines.push(format!("services registry: {}", services_path.display()));
                if let Ok(decoded) = serde_json::from_str::<serde_json::Value>(&content) {
                    if let Some(services) =
                        decoded.get("services").and_then(|value| value.as_array())
                    {
                        for service in services {
                            let name = service
                                .get("name")
                                .and_then(|value| value.as_str())
                                .unwrap_or("unknown");
                            let endpoint = service
                                .get("mcp_endpoint")
                                .and_then(|value| value.as_str())
                                .unwrap_or("-");
                            let tags = service
                                .get("tags")
                                .and_then(|value| value.as_array())
                                .map(|tags| {
                                    tags.iter()
                                        .filter_map(|value| value.as_str())
                                        .collect::<Vec<_>>()
                                        .join(",")
                                })
                                .unwrap_or_default();
                            lines
                                .push(format!("- service {name}: endpoint={endpoint} tags={tags}"));
                        }
                    }
                } else {
                    lines.push(
                        "- services registry exists but could not be parsed as JSON.".to_string(),
                    );
                }
            }
            Err(_) => lines.push(format!(
                "services registry not found or unreadable at {}",
                services_path.display()
            )),
        }
    }

    if let Some(packages_dir) = packages_dir {
        if let Ok(entries) = fs::read_dir(&packages_dir) {
            lines.push(format!("packages: {}", packages_dir.display()));
            for entry in entries.flatten().take(20) {
                let path = entry.path();
                if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                    continue;
                }
                if let Ok(content) = fs::read_to_string(&path) {
                    if let Ok(decoded) = serde_json::from_str::<serde_json::Value>(&content) {
                        let id = decoded
                            .get("id")
                            .and_then(|value| value.as_str())
                            .unwrap_or("unknown");
                        let kind = decoded
                            .get("kind")
                            .and_then(|value| value.as_str())
                            .unwrap_or("unknown");
                        lines.push(format!("- package {id}: kind={kind}"));
                    }
                }
            }
        }
    }

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    /// Serializes tests that read or mutate the peer-registry env vars, and
    /// guarantees both are UNSET on entry (restored on drop). Env vars are
    /// process-global, so unserialized mutation would race.
    fn with_registry_env<T>(
        services: Option<&Path>,
        packages: Option<&Path>,
        f: impl FnOnce() -> T,
    ) -> T {
        static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        let _guard = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
        let previous_services = env::var_os("SWARM_SERVICES_REGISTRY");
        let previous_packages = env::var_os("SWARM_PACKAGES_DIR");
        match services {
            Some(path) => env::set_var("SWARM_SERVICES_REGISTRY", path),
            None => env::remove_var("SWARM_SERVICES_REGISTRY"),
        }
        match packages {
            Some(path) => env::set_var("SWARM_PACKAGES_DIR", path),
            None => env::remove_var("SWARM_PACKAGES_DIR"),
        }
        let result = f();
        match previous_services {
            Some(value) => env::set_var("SWARM_SERVICES_REGISTRY", value),
            None => env::remove_var("SWARM_SERVICES_REGISTRY"),
        }
        match previous_packages {
            Some(value) => env::set_var("SWARM_PACKAGES_DIR", value),
            None => env::remove_var("SWARM_PACKAGES_DIR"),
        }
        result
    }

    #[test]
    fn audit_and_design_prompts_include_scope_and_context_heading() {
        with_registry_env(None, None, || {
            let cwd = std::path::Path::new("/tmp/swarm");
            let audit = build_audit_prompt("inspect", "harden", cwd);
            let design = build_design_prompt("polish", "motion", cwd);

            assert!(audit.contains("Focus: harden."));
            assert!(audit.contains("Working directory: /tmp/swarm."));
            assert!(audit.contains("Available local peer context:"));
            assert!(audit.contains("Compact handoff packet rules:"));
            assert!(audit.contains("NEEDS_EVIDENCE"));
            assert!(audit.contains("simplification/hardening opportunities"));
            assert!(design.contains("Audience: technical operators"));
            assert!(design.contains("Focus: motion."));
            assert!(design.contains("Findings, Risks, Steps, Blockers, Tests"));
            assert!(design.contains("motion and interaction specs"));
        });
    }

    #[test]
    fn peer_context_without_env_vars_returns_static_no_context_line() {
        with_registry_env(None, None, || {
            assert_eq!(peer_context(), NO_PEER_CONTEXT);
            let audit = build_audit_prompt("inspect", "harden", Path::new("/tmp/swarm"));
            assert!(audit.contains(NO_PEER_CONTEXT));
            assert!(!audit.contains("services registry:"));
            assert!(!audit.contains("packages:"));
        });
    }

    #[test]
    fn peer_context_reads_opt_in_registry_and_packages() {
        let temp = tempfile::tempdir().unwrap();
        let services_path = temp.path().join("services.json");
        std::fs::write(
            &services_path,
            serde_json::json!({
                "services": [
                    {"name": "demo", "mcp_endpoint": "tcp:127.0.0.1:9000", "tags": ["agent"]}
                ]
            })
            .to_string(),
        )
        .unwrap();
        let packages_dir = temp.path().join("packages");
        std::fs::create_dir_all(&packages_dir).unwrap();
        std::fs::write(
            packages_dir.join("demo.json"),
            serde_json::json!({"id": "demo-pkg", "kind": "agent-orchestrator"}).to_string(),
        )
        .unwrap();

        with_registry_env(Some(&services_path), Some(&packages_dir), || {
            let context = peer_context();
            assert!(context.contains("- service demo: endpoint=tcp:127.0.0.1:9000 tags=agent"));
            assert!(context.contains("- package demo-pkg: kind=agent-orchestrator"));
            assert!(!context.contains(NO_PEER_CONTEXT));
        });
    }
}
