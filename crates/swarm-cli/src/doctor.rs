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
use swarm_exec::executor::execute_partner;
use swarm_exec::preflight::{classify_error, suggested_action_for_error};
use swarm_kernel::agent::AgentChoice;
use swarm_kernel::args::{config_path, parse_agent_choice, Args};
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

/// Entry point for `swarm doctor [--probe] [--data-dir PATH]`.
pub(crate) fn cmd_doctor(raw: &[String]) -> Result<i32, String> {
    let (data_dir, probe) = parse_doctor_flags(raw)?;

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
    run_doctor(&config, &status, &registry, &providers, probe, &mut out)
}

/// Parse `doctor` flags: `--data-dir PATH` and the opt-in `--probe`.
fn parse_doctor_flags(raw: &[String]) -> Result<(Option<PathBuf>, bool), String> {
    let mut data_dir: Option<PathBuf> = None;
    let mut probe = false;
    let mut iter = raw.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--data-dir" => {
                data_dir =
                    Some(PathBuf::from(iter.next().ok_or_else(|| {
                        "Error: --data-dir requires a path.".to_string()
                    })?));
            }
            "--probe" => probe = true,
            other => return Err(format!("Error: unknown `doctor` argument `{other}`.")),
        }
    }
    Ok((data_dir, probe))
}

/// The testable core: pure function of an already-loaded config, registry,
/// and provider rows. Returns the process exit code (0 healthy, 1 blocked).
pub(crate) fn run_doctor(
    config: &SwarmConfig,
    config_status: &ConfigStatus,
    registry: &BackendRegistry,
    providers: &[ProviderConfig],
    probe: bool,
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
    if !subprocess_backend_ids(config, registry).is_empty() {
        w(
            "note: for CLI agents, ready means the binary/command was located — authentication is NOT verified. Use `swarm doctor --probe`."
                .to_string(),
        )?;
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
        // Stored model ids that are now legacy aliases still load, but new
        // work should move off them — warn (non-blocking) and point at the
        // suggestion command.
        let legacy = provider.provider_type.legacy_model_aliases();
        for model in &provider.models {
            if legacy.contains(&model.as_str()) {
                warnings += 1;
                w(format!(
                    "! {}: model `{model}` is a legacy alias — see `swarm provider models {}` for current suggestions",
                    provider.id,
                    provider.provider_type.as_str()
                ))?;
            }
        }
    }

    // ── Probe (opt-in): one tiny live request per subprocess backend ─────
    if probe {
        w("\n== probe ==".to_string())?;
        w("--probe sends one real (tiny) request to each CLI agent through the normal dispatch path.".to_string())?;
        let candidates = subprocess_backend_ids(config, registry);
        if candidates.is_empty() {
            w("(no subprocess-backed backends to probe)".to_string())?;
        }
        for id in candidates {
            let ready = registry
                .resolve(&id)
                .ok()
                .map(|backend| backend.ready().is_ok())
                .unwrap_or(false);
            if !ready {
                w(format!("- {id}: skipped (static check already failed)"))?;
                continue;
            }
            match probe_backend(registry, &id) {
                Ok(()) => w(format!("✓ {id}: responded"))?,
                Err(err) => {
                    // Probe failures warn loudly but never block: the static
                    // gate stays authoritative for the exit code.
                    warnings += 1;
                    let category = classify_error(&err);
                    w(format!("! {id}: probe failed ({category}): {err}"))?;
                    w(format!("  → {}", suggested_action_for_error(&err)))?;
                }
            }
        }
    }

    // ── Summary ──────────────────────────────────────────────────────────
    w(format!(
        "\nsummary: {blocking} blocking issue(s), {warnings} warning(s)"
    ))?;
    Ok(if blocking == 0 { 0 } else { 1 })
}

/// Backends that dispatch by spawning a subprocess, in stable order: built-in
/// CLI agents (when registered and not shadowed by a descriptor) plus every
/// `kind = "cli"` descriptor. `openai-compatible` / `native` kinds are skipped
/// — their credential status is already covered by the provider section.
fn subprocess_backend_ids(config: &SwarmConfig, registry: &BackendRegistry) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    for id in ["claude", "codex"] {
        if !config.backend.contains_key(id) && registry.resolve(id).is_ok() {
            ids.push(id.to_string());
        }
    }
    let mut descriptor_ids: Vec<&String> = config
        .backend
        .iter()
        .filter(|(_, d)| d.kind == BackendKind::Cli)
        .map(|(id, _)| id)
        .collect();
    descriptor_ids.sort_unstable();
    ids.extend(descriptor_ids.into_iter().cloned());
    ids
}

/// Fixed prompt + timeout for `--probe`: small enough to be cheap, distinctive
/// enough that any response counts as proof the agent is authenticated.
const PROBE_PROMPT: &str = "Reply with the single word: pong";
const PROBE_TIMEOUT_SECS: u64 = 60;

/// Send the tiny probe prompt through the REAL dispatch path
/// (`execute_partner` with `agent_custom` pinned to the backend id). Success
/// is a clean exit; anything else comes back as a classifiable error string.
fn probe_backend(registry: &BackendRegistry, id: &str) -> Result<(), String> {
    let args = Args {
        prompt: PROBE_PROMPT.to_string(),
        cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        timeout_secs: PROBE_TIMEOUT_SECS,
        quiet: true,
        agent: AgentChoice::Claude, // placeholder; agent_custom wins at dispatch
        agent_custom: Some(id.to_string()),
        model: None,
        persona: None,
        background: false,
        allow_bypass_permissions: false,
    };
    let outcome = execute_partner(registry, args.agent, &args, PROBE_PROMPT)?;
    if outcome.timed_out {
        return Err(format!(
            "timed out after {PROBE_TIMEOUT_SECS}s waiting for a response"
        ));
    }
    match outcome.exit_status {
        Some(0) | None => Ok(()),
        Some(code) => {
            let stderr = outcome.stderr.trim();
            let detail: String = stderr.chars().take(300).collect();
            Err(format!("exit code {code}: {detail}"))
        }
    }
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
            false,
            &mut out,
        )
        .unwrap();
        (code, String::from_utf8(out).unwrap())
    }

    fn doctor_probe(config: &SwarmConfig, registry: &BackendRegistry) -> (i32, String) {
        let mut out = Vec::new();
        let code = run_doctor(
            config,
            &ConfigStatus::Missing,
            registry,
            &[],
            true,
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
            false,
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
    fn doctor_flags_parse_probe_and_data_dir() {
        let raw: Vec<String> = ["--probe", "--data-dir", "/tmp/x"]
            .iter()
            .map(ToString::to_string)
            .collect();
        let (dir, probe) = parse_doctor_flags(&raw).unwrap();
        assert!(probe);
        assert_eq!(dir, Some(PathBuf::from("/tmp/x")));
        let (dir, probe) = parse_doctor_flags(&[]).unwrap();
        assert!(!probe);
        assert_eq!(dir, None);
        assert!(parse_doctor_flags(&["--frob".to_string()]).is_err());
    }

    #[test]
    fn backends_section_footnotes_that_ready_does_not_verify_auth() {
        let config = config_from(
            r#"
            [backend.echo]
            kind = "cli"
            command = "printf"
            "#,
        );
        let registry = registry_from(&config);
        let (_, report) = doctor(&config, &registry, &[]);
        assert!(
            report.contains("authentication is NOT verified"),
            "{report}"
        );
        assert!(report.contains("swarm doctor --probe"), "{report}");
        // One footnote, not per-row spam.
        assert_eq!(report.matches("authentication is NOT verified").count(), 1);
    }

    #[test]
    fn probe_reports_responding_cli_backend_as_success() {
        let config = config_from(
            r#"
            [backend.echo]
            kind = "cli"
            command = "printf"
            args = ["pong"]
            prompt = "stdin"
            "#,
        );
        let registry = registry_from(&config);
        let (code, report) = doctor_probe(&config, &registry);
        assert_eq!(code, 0, "{report}");
        assert!(report.contains("== probe =="), "{report}");
        assert!(report.contains("real (tiny) request"), "{report}");
        assert!(report.contains("✓ echo: responded"), "{report}");
        assert!(
            report.contains("0 blocking issue(s), 0 warning(s)"),
            "{report}"
        );
    }

    #[test]
    fn probe_classifies_auth_sounding_failure_as_warning_not_blocking() {
        let config = config_from(
            r#"
            [backend.locked]
            kind = "cli"
            command = "sh"
            args = ["-c", "echo 'error: not logged in (credentials live in the macOS login keychain)' >&2; exit 1"]
            prompt = "stdin"
            "#,
        );
        let registry = registry_from(&config);
        let (code, report) = doctor_probe(&config, &registry);
        assert_eq!(code, 0, "probe failures never block: {report}");
        assert!(
            report.contains("! locked: probe failed (auth-or-permission)"),
            "{report}"
        );
        assert!(report.contains("not logged in"), "{report}");
        assert!(report.contains("run it interactively once"), "{report}");
        assert!(report.contains("1 warning(s)"), "{report}");
    }

    #[test]
    fn probe_skips_kinds_whose_keys_the_provider_section_covers() {
        let config = config_from(
            r#"
            [backend.api]
            kind = "openai-compatible"
            "#,
        );
        let registry = registry_from(&config);
        let (_, report) = doctor_probe(&config, &registry);
        assert!(
            report.contains("(no subprocess-backed backends to probe)"),
            "{report}"
        );
    }

    #[test]
    fn provider_with_legacy_model_warns_and_points_at_models_command() {
        let mut provider = ProviderConfig::new(
            "Old".to_string(),
            ProviderType::DeepSeek,
            None,
            Some("sk-x".to_string()),
        );
        provider.id = "old".to_string();
        provider.models = vec!["deepseek-chat".to_string(), "deepseek-v4-pro".to_string()];

        let config = SwarmConfig::default();
        let registry = BackendRegistry::new();
        let (code, report) = doctor(&config, &registry, &[provider]);
        assert_eq!(code, 0, "legacy models warn, never block: {report}");
        assert!(
            report.contains("! old: model `deepseek-chat` is a legacy alias"),
            "{report}"
        );
        assert!(
            report.contains("swarm provider models deepseek"),
            "{report}"
        );
        assert!(
            !report.contains("`deepseek-v4-pro` is a legacy"),
            "{report}"
        );
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
