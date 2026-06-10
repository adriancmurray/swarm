//! `swarm doctor` — config / backend / provider health check.
//!
//! Walks the same paths a real run takes: parse config, build the
//! `BackendRegistry`, probe every backend's `ready()`, sanity-check every
//! backend descriptor, scan routing strings for ids that resolve nowhere,
//! and report each stored provider's credential status.
//!
//! Exit code contract: `0` when there are no blocking issues, `1` otherwise.
//! Blocking = a backend whose `ready()` fails, or a routing string naming an
//! id that neither parses as a built-in agent nor resolves in the registry.
//! Descriptor shape complaints and credential gaps are warnings only.

use std::io::{self, Write};
use std::path::PathBuf;

use swarm_exec::backend_registry::BackendRegistry;
use swarm_kernel::args::{config_path, parse_agent_choice};
use swarm_kernel::backend_descriptor::BackendKind;
use swarm_kernel::config::SwarmConfig;
use swarm_manager::{ProviderConfig, ProviderRegistry};
use swarm_store::store::providers_dir;

use crate::provider_commands::key_status_label;

/// How the config file resolved at startup.
pub(crate) enum ConfigStatus {
    /// Parsed cleanly from the path shown.
    Parsed(PathBuf),
    /// No config file present — built-in defaults (normal).
    Missing,
    /// File present but malformed — defaults in effect, parse error attached.
    ParseError(PathBuf, String),
}

/// Read and parse the active config, reporting how it resolved instead of
/// warning on stderr the way `load_config` does.
fn load_config_with_status() -> (SwarmConfig, ConfigStatus) {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return (SwarmConfig::default(), ConfigStatus::Missing);
    };
    let path = config_path(&home);
    let Ok(content) = std::fs::read_to_string(&path) else {
        return (SwarmConfig::default(), ConfigStatus::Missing);
    };
    match toml::from_str::<SwarmConfig>(&content) {
        Ok(config) => (config, ConfigStatus::Parsed(path)),
        Err(err) => (
            SwarmConfig::default(),
            ConfigStatus::ParseError(path, err.to_string()),
        ),
    }
}

/// Entry point for `swarm doctor [--data-dir PATH]`.
pub(crate) fn cmd_doctor(raw: &[String]) -> Result<i32, String> {
    let mut data_dir: Option<PathBuf> = None;
    let mut iter = raw.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--data-dir" => {
                data_dir =
                    Some(PathBuf::from(iter.next().ok_or_else(|| {
                        "Error: --data-dir requires a path.".to_string()
                    })?));
            }
            other => return Err(format!("Error: unknown `doctor` argument `{other}`.")),
        }
    }

    let (config, status) = load_config_with_status();
    let registry = BackendRegistry::from_config(&config);
    let providers = match data_dir.or_else(providers_dir) {
        Some(dir) => ProviderRegistry::open(&dir)
            .map_err(|e| format!("Error opening provider registry: {e}"))?
            .list()
            .map_err(|e| format!("Error reading provider registry: {e}"))?,
        None => Vec::new(),
    };

    let mut out = io::stdout();
    run_doctor(&config, &status, &registry, &providers, &mut out)
}

/// The testable core: pure function of an already-loaded config, registry,
/// and provider rows. Returns the process exit code (0 healthy, 1 blocked).
pub(crate) fn run_doctor(
    config: &SwarmConfig,
    config_status: &ConfigStatus,
    registry: &BackendRegistry,
    providers: &[ProviderConfig],
    out: &mut dyn Write,
) -> Result<i32, String> {
    let mut w =
        |line: String| writeln!(out, "{line}").map_err(|e| format!("Error writing output: {e}"));
    let mut blocking = 0usize;
    let mut warnings = 0usize;

    // ── Config ───────────────────────────────────────────────────────────
    w("== config ==".to_string())?;
    match config_status {
        ConfigStatus::Parsed(path) => w(format!("✓ config parsed: {}", path.display()))?,
        ConfigStatus::Missing => w("✓ no config file; using built-in defaults".to_string())?,
        ConfigStatus::ParseError(path, err) => {
            warnings += 1;
            w(format!(
                "! config {} failed to parse; using built-in defaults: {err}",
                path.display()
            ))?;
        }
    }

    // ── Backends: resolve + ready() for every registered id ─────────────
    w("\n== backends ==".to_string())?;
    for id in registry.ids() {
        match registry.resolve(&id) {
            Ok(backend) => match backend.ready() {
                Ok(()) => w(format!("✓ {id}: ready"))?,
                Err(err) => {
                    blocking += 1;
                    w(format!("✗ {id}: {err}"))?;
                }
            },
            Err(err) => {
                blocking += 1;
                w(format!("✗ {id}: {err}"))?;
            }
        }
    }

    // ── Descriptors: shape checks ────────────────────────────────────────
    if !config.backend.is_empty() {
        w("\n== backend descriptors ==".to_string())?;
        for (id, descriptor) in &config.backend {
            let kind = match descriptor.kind {
                BackendKind::Cli => "cli",
                BackendKind::OpenAiCompatible => "openai-compatible",
                BackendKind::Native => "native",
            };
            if descriptor.kind == BackendKind::Cli && descriptor.command.is_none() {
                warnings += 1;
                w(format!(
                    "! [backend.{id}] kind={kind} has no `command` — it will never be ready"
                ))?;
            } else {
                w(format!("✓ [backend.{id}] kind={kind}"))?;
            }
        }
    }

    // ── Routing strings: every name must resolve somewhere ──────────────
    let mut route_refs: Vec<(String, String)> = Vec::new();
    let mut route_names: Vec<&String> = config.routes.keys().collect();
    route_names.sort_unstable();
    for role in route_names {
        for spec in &config.routes[role].preferred {
            route_refs.push((format!("routes.{role}.preferred"), spec.clone()));
        }
    }
    for spec in &config.reliability.fallback_chain {
        route_refs.push(("reliability.fallback_chain".to_string(), spec.clone()));
    }
    if let Some(spec) = &config.swarm.default_manager {
        route_refs.push(("swarm.default_manager".to_string(), spec.clone()));
    }
    for spec in &config.swarm.default_workers {
        route_refs.push(("swarm.default_workers".to_string(), spec.clone()));
    }
    if !route_refs.is_empty() {
        w("\n== routing ==".to_string())?;
        for (source, spec) in &route_refs {
            // Worker specs are ROLE=NAME[:MODEL]; the rest are NAME[:MODEL].
            let after_role = spec.rsplit('=').next().unwrap_or(spec);
            let name = after_role.split(':').next().unwrap_or(after_role).trim();
            let known = parse_agent_choice(name).is_ok() || registry.resolve(name).is_ok();
            if known {
                w(format!("✓ {source}: `{spec}`"))?;
            } else {
                blocking += 1;
                w(format!(
                    "✗ {source}: `{spec}` names `{name}`, which is neither a built-in agent nor a registered backend"
                ))?;
            }
        }
    }

    // ── Providers: credential status ─────────────────────────────────────
    w("\n== providers ==".to_string())?;
    if providers.is_empty() {
        w("(none configured)".to_string())?;
    }
    for provider in providers {
        let label = key_status_label(provider);
        let marker = match label {
            "healthy" | "env" => "✓",
            _ => "!",
        };
        if marker == "!" {
            warnings += 1;
        }
        w(format!(
            "{marker} {} ({}): key {label}",
            provider.id,
            provider.provider_type.as_str()
        ))?;
    }

    // ── Summary ──────────────────────────────────────────────────────────
    w(format!(
        "\nsummary: {blocking} blocking issue(s), {warnings} warning(s)"
    ))?;
    Ok(if blocking == 0 { 0 } else { 1 })
}

#[cfg(test)]
mod tests {
    use super::*;
    use swarm_manager::provider::crypto::KeychainVault;
    use swarm_manager::ProviderType;
    use tempfile::tempdir;

    fn config_from(toml_str: &str) -> SwarmConfig {
        toml::from_str(toml_str).unwrap()
    }

    /// Registry built ONLY from descriptors (no PATH-dependent built-ins) so
    /// the pass/fail classification is deterministic on any machine.
    fn registry_from(config: &SwarmConfig) -> BackendRegistry {
        let mut reg = BackendRegistry::new();
        for (id, descriptor) in &config.backend {
            reg.register_descriptor(id.clone(), descriptor.clone());
        }
        reg
    }

    fn doctor(
        config: &SwarmConfig,
        registry: &BackendRegistry,
        providers: &[ProviderConfig],
    ) -> (i32, String) {
        let mut out = Vec::new();
        let code = run_doctor(
            config,
            &ConfigStatus::Missing,
            registry,
            providers,
            &mut out,
        )
        .unwrap();
        (code, String::from_utf8(out).unwrap())
    }

    #[test]
    fn all_ready_backends_and_known_routes_exit_zero() {
        let config = config_from(
            r#"
            [backend.echo]
            kind = "cli"
            command = "printf"

            [routes.implementation]
            preferred = ["echo", "echo:some-model"]

            [reliability]
            fallback_chain = ["echo"]

            [swarm]
            default_manager = "echo:big"
            default_workers = ["review=echo"]
            "#,
        );
        let registry = registry_from(&config);
        let (code, report) = doctor(&config, &registry, &[]);
        assert_eq!(code, 0, "{report}");
        assert!(report.contains("✓ echo: ready"), "{report}");
        assert!(report.contains("✓ [backend.echo] kind=cli"), "{report}");
        assert!(
            report.contains("routes.implementation.preferred"),
            "{report}"
        );
        assert!(report.contains("swarm.default_workers"), "{report}");
        assert!(report.contains("0 blocking issue(s)"), "{report}");
    }

    #[test]
    fn not_ready_backend_is_blocking() {
        // cli kind with no command: resolves, but ready() errors — and the
        // descriptor section flags the missing command.
        let config = config_from(
            r#"
            [backend.broken]
            kind = "cli"
            "#,
        );
        let registry = registry_from(&config);
        let (code, report) = doctor(&config, &registry, &[]);
        assert_eq!(code, 1, "{report}");
        assert!(report.contains("✗ broken:"), "{report}");
        assert!(report.contains("no `command`"), "{report}");
        assert!(report.contains("1 blocking issue(s)"), "{report}");
    }

    #[test]
    fn unknown_route_name_is_blocking_and_names_the_source() {
        let config = config_from(
            r#"
            [routes.review]
            preferred = ["ghost-agent:fast"]
            "#,
        );
        let registry = registry_from(&config);
        let (code, report) = doctor(&config, &registry, &[]);
        assert_eq!(code, 1, "{report}");
        assert!(report.contains("✗ routes.review.preferred"), "{report}");
        assert!(report.contains("`ghost-agent`"), "{report}");
    }

    #[test]
    fn builtin_agent_names_in_routes_pass_without_registry_entries() {
        // `claude`/`codex`/`auto` parse as built-ins, so the route check
        // passes even with an empty registry (readiness is checked
        // separately, per backend).
        let config = config_from(
            r#"
            [reliability]
            fallback_chain = ["claude:sonnet", "codex", "auto"]
            "#,
        );
        let registry = BackendRegistry::new();
        let (code, report) = doctor(&config, &registry, &[]);
        assert_eq!(code, 0, "{report}");
        assert!(
            report.contains("✓ reliability.fallback_chain: `claude:sonnet`"),
            "{report}"
        );
    }

    #[test]
    fn config_parse_error_is_a_warning_not_blocking() {
        let config = SwarmConfig::default();
        let registry = BackendRegistry::new();
        let mut out = Vec::new();
        let code = run_doctor(
            &config,
            &ConfigStatus::ParseError(PathBuf::from("/tmp/config.toml"), "boom".to_string()),
            &registry,
            &[],
            &mut out,
        )
        .unwrap();
        let report = String::from_utf8(out).unwrap();
        assert_eq!(code, 0, "{report}");
        assert!(report.contains("failed to parse"), "{report}");
        assert!(report.contains("boom"), "{report}");
        assert!(report.contains("1 warning(s)"), "{report}");
    }

    #[test]
    fn provider_section_reports_key_status_without_blocking() {
        let dir = tempdir().unwrap();
        let vault = {
            let mut key = [0u8; 32];
            for (i, byte) in key.iter_mut().enumerate() {
                *byte = i as u8;
            }
            KeychainVault::with_key(key)
        };
        let registry_store =
            ProviderRegistry::open_with_vault(&dir.path().to_path_buf(), vault).unwrap();
        let mut keyed = ProviderConfig::new(
            "Keyed".to_string(),
            ProviderType::OpenAI,
            None,
            Some("sk-x".to_string()),
        );
        keyed.id = "keyed".to_string();
        registry_store.add(keyed).unwrap();
        let mut keyless =
            ProviderConfig::new("Keyless".to_string(), ProviderType::Anthropic, None, None);
        keyless.id = "keyless".to_string();
        registry_store.add(keyless).unwrap();
        let providers = registry_store.list().unwrap();

        let config = SwarmConfig::default();
        let backend_registry = BackendRegistry::new();
        let (code, report) = doctor(&config, &backend_registry, &providers);
        assert_eq!(code, 0, "absent keys are warnings, not blocking: {report}");
        assert!(report.contains("✓ keyed (openai): key healthy"), "{report}");
        assert!(
            report.contains("! keyless (anthropic): key absent"),
            "{report}"
        );
        assert!(report.contains("1 warning(s)"), "{report}");
    }
}
