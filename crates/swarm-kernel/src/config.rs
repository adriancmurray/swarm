//! Typed `config.toml` model + loader.
//!
//! The legacy line-scanners (`load_default_timeout`, `load_default_agent` in
//! `args.rs`) still own those two flat keys. This module adds the structured
//! surface — `[routes.*]`, `[swarm]`, `[reliability]`, `[context]`,
//! `[settings]` — that drives backend routing, the retry/fallback chain,
//! optional auto-context injection, and UI-adjustable settings.
//! `#[serde(default)]` throughout means a sparse or partial config never fails
//! to load; a *malformed* config warns loudly (never a silent fallback) and
//! falls back to built-in defaults.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::backend_descriptor::BackendDescriptor;

/// The structured view of `config.toml`. Unknown keys (e.g. `lean`,
/// `[auto_scale]`, `[design]`) are ignored by serde; missing keys take the
/// field default. Construct literals directly in tests — never call
/// [`load_config`] from a test (it reads the user's real config file).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct SwarmConfig {
    #[serde(default)]
    pub routes: HashMap<String, RouteConfig>,
    #[serde(default)]
    pub swarm: SwarmDefaults,
    #[serde(default)]
    pub discussion: DiscussionDefaults,
    #[serde(default)]
    pub design: DiscussionDefaults,
    #[serde(default)]
    pub reliability: ReliabilityConfig,
    #[serde(default)]
    pub context: ContextConfig,
    #[serde(default)]
    pub settings: Settings,
    /// `[backend.<id>]` blocks — declarative agent backends loaded into the
    /// `BackendRegistry` at dispatch. Adding an agent is config, not code:
    /// the id becomes selectable wherever a backend name is accepted
    /// (`--agent <id>`, worker specs, fallback chains). BTreeMap so listings
    /// are deterministic.
    #[serde(default)]
    pub backend: BTreeMap<String, BackendDescriptor>,
}

/// `[swarm]`: defaults for fanout/swarm runs when CLI flags are omitted.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct SwarmDefaults {
    #[serde(default)]
    pub default_manager: Option<String>,
    #[serde(default)]
    pub default_workers: Vec<String>,
}

/// `[discussion]` / `[design]`: defaults for discussion-style runs.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct DiscussionDefaults {
    #[serde(default)]
    pub default_rounds: Option<u32>,
    #[serde(default)]
    pub default_manager: Option<String>,
    #[serde(default)]
    pub default_participants: Vec<String>,
    #[serde(default)]
    pub docs_agent: Option<String>,
}

/// `[settings]`: UI-adjustable settings for Agent Swarm behaviour.
///
/// Every field has a `#[serde(default)]` so the whole `[settings]` section is
/// optional — existing `config.toml` files that omit it continue to work.
/// New fields must also carry `#[serde(default)]` to remain forward-compatible.
///
/// Settings are adjustable at runtime through the `agent_swarm_settings_get`
/// and `agent_swarm_settings_set` MCP tools; changes take effect on the next
/// invocation (config is loaded once per process).
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Settings {
    /// Enable the API-docs follow-up worker by default in `discuss`, `design`,
    /// and `audit` runs without requiring `--docs` on every invocation.
    ///
    /// Precedence: `--docs`/`--no-docs` CLI flag > this value > built-in
    /// `false`.  Audit's built-in default remains `true` regardless of this
    /// setting (i.e. `--no-docs` is still needed to suppress it for audit).
    #[serde(default)]
    pub docs_default: bool,

    /// Optional default persona/preprompt wrapper for direct `agent-swarm run`
    /// invocations. CLI flags still win:
    /// `--persona NAME` sets a persona for one run, `--no-persona` disables it.
    ///
    /// Supported built-ins are implemented by the exec layer
    /// (`compact-manager`, `compact-worker`) and profile ids/roles from
    /// `agent-swarm profiles`.
    #[serde(default)]
    pub direct_persona: Option<String>,
}

/// `[context]`: opt-in auto-context injection for the fanout (run_swarm) path.
/// When `auto_inject` is true, a bounded local filesystem context summary is
/// gathered once (before workers are spawned) and prepended to worker + manager
/// prompts. Default is `false` — opt-in only, never implicit.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ContextConfig {
    /// Enable auto-context injection. Overridable per-run with `--context` /
    /// `--no-context`. Default: false.
    #[serde(default)]
    pub auto_inject: bool,
}

/// A `[routes.<role>]` entry. `preferred` is the per-role fallback order. The
/// advisory `mode` key in config is ignored for now and would re-appear here
/// when consumed; `#[serde(default)]` keeps unknown route keys from failing.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct RouteConfig {
    #[serde(default)]
    pub preferred: Vec<String>,
}

/// `[reliability]`: retry + cross-backend fallback policy. `fallback_chain` is
/// the global backend order used when a worker's role has no `[routes.<role>]`
/// entry.
#[derive(Debug, Clone, Deserialize)]
pub struct ReliabilityConfig {
    #[serde(default = "default_retry_attempts")]
    pub retry_attempts: u32,
    #[serde(default = "default_retry_backoff_ms")]
    pub retry_backoff_ms: u64,
    #[serde(default)]
    pub fallback_chain: Vec<String>,
}

impl Default for ReliabilityConfig {
    fn default() -> Self {
        Self {
            retry_attempts: default_retry_attempts(),
            retry_backoff_ms: default_retry_backoff_ms(),
            fallback_chain: Vec::new(),
        }
    }
}

fn default_retry_attempts() -> u32 {
    1
}

fn default_retry_backoff_ms() -> u64 {
    1_500
}

/// Returns the current `[settings]` section from a config file at `path`.
///
/// A missing file yields built-in defaults. A malformed file warns and yields
/// defaults. Intended for the `agent_swarm_settings_get` MCP handler.
pub fn read_settings_at(path: &Path) -> Settings {
    let Ok(content) = std::fs::read_to_string(path) else {
        return Settings::default();
    };
    match toml::from_str::<SwarmConfig>(&content) {
        Ok(config) => config.settings,
        Err(err) => {
            eprintln!(
                "agent-swarm: warning: could not parse {} ({err}); returning built-in defaults",
                path.display()
            );
            Settings::default()
        }
    }
}

/// Writes a new `[settings]` section into `path` using a section-merge
/// strategy: all other content in the file is preserved. Only the
/// `[settings]` table is replaced.
///
/// Strategy: read the file as a raw `toml::Value`, overwrite the `settings`
/// key, then re-serialize. This preserves `default_timeout`, `default_agent`,
/// `[reliability]`, `[routes]`, `[context]`, and any unmodeled sections that
/// serde ignores. Comments and key ordering in other sections are preserved in
/// the toml round-trip; comments inside the old `[settings]` block are lost
/// (acceptable — `[settings]` is the only section agent-swarm writes).
///
/// Takes effect on the **next** invocation; config is loaded once per process.
pub fn write_settings_at(path: &Path, settings: &Settings) -> Result<(), String> {
    // Load existing toml as a generic Value so unmodeled keys survive.
    let existing: toml::Value = if let Ok(content) = std::fs::read_to_string(path) {
        match toml::from_str::<toml::Value>(&content) {
            Ok(value) => value,
            Err(err) => {
                eprintln!(
                    "agent-swarm: warning: could not parse {} ({err}); overwriting with defaults + new settings",
                    path.display()
                );
                toml::Value::Table(toml::map::Map::new())
            }
        }
    } else {
        toml::Value::Table(toml::map::Map::new())
    };

    let mut root = match existing {
        toml::Value::Table(map) => map,
        other => {
            return Err(format!(
                "config file {} is not a TOML table: {other:?}",
                path.display()
            ));
        }
    };

    // Replace only the `[settings]` key.
    let settings_value = toml::Value::try_from(settings)
        .map_err(|err| format!("Error serializing settings: {err}"))?;
    root.insert("settings".to_string(), settings_value);

    let serialized = toml::to_string(&toml::Value::Table(root))
        .map_err(|err| format!("Error serializing config: {err}"))?;

    swarm_store::store::write_text_atomic(path, serialized.as_bytes())
        .map_err(|err| format!("Error writing config {}: {err}", path.display()))
}

/// Reads and parses the active `config.toml`. A missing file is normal and
/// yields defaults silently; a *malformed* file warns on stderr (per the
/// no-silent-fallbacks rule) and yields defaults rather than aborting a run.
///
/// Not called from tests — pure consumers take `&SwarmConfig` so coverage stays
/// hermetic and independent of the user's real config file.
pub fn load_config() -> SwarmConfig {
    let Some(home) = crate::resolver::home_dir() else {
        return SwarmConfig::default();
    };
    let path = crate::args::config_path(&home);
    let Ok(content) = std::fs::read_to_string(&path) else {
        return SwarmConfig::default();
    };
    match toml::from_str::<SwarmConfig>(&content) {
        Ok(config) => config,
        Err(err) => {
            eprintln!(
                "agent-swarm: warning: could not parse {} ({err}); using built-in defaults",
                path.display()
            );
            SwarmConfig::default()
        }
    }
}

/// Resolves the effective docs-enabled flag using the three-level precedence
/// chain:
///
/// 1. Explicit CLI flag (`--docs` → `Some(true)`, `--no-docs` → `Some(false)`)
///    overrides everything.
/// 2. `config.settings.docs_default` is used when no CLI flag was supplied.
/// 3. The built-in default is `false`; pass it as `config_default` when no
///    config is loaded.
///
/// `parse_audit_args` hard-codes `Some(true)` so audit's built-in default of
/// docs-ON is preserved regardless of this config setting.
pub fn resolve_docs(cli: Option<bool>, config_default: bool) -> bool {
    cli.unwrap_or(config_default)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_routes_and_reliability() {
        // The input deliberately carries an unmodeled `mode` key and a `[swarm]`
        // section to confirm they are ignored, not rejected.
        let cfg: SwarmConfig = toml::from_str(
            r#"
            lean = "heavy"
            default_timeout = 300

            [routes.implementation]
            preferred = ["gemini", "claude:sonnet", "codex"]
            mode = "agent"

            [swarm]
            default_manager = "claude:sonnet"
            default_workers = ["architecture=gemini", "review=claude:sonnet"]

            [discussion]
            default_rounds = 1
            default_manager = "gemini"
            default_participants = ["architecture=gemini"]
            docs_agent = "gemini"

            [reliability]
            retry_attempts = 2
            retry_backoff_ms = 800
            fallback_chain = ["gemini", "claude:sonnet"]
            "#,
        )
        .unwrap();

        assert_eq!(
            cfg.routes.get("implementation").unwrap().preferred,
            vec!["gemini", "claude:sonnet", "codex"]
        );
        assert_eq!(cfg.reliability.retry_attempts, 2);
        assert_eq!(cfg.reliability.retry_backoff_ms, 800);
        assert_eq!(
            cfg.reliability.fallback_chain,
            vec!["gemini", "claude:sonnet"]
        );
        assert_eq!(cfg.swarm.default_manager.as_deref(), Some("claude:sonnet"));
        assert_eq!(
            cfg.swarm.default_workers,
            vec!["architecture=gemini", "review=claude:sonnet"]
        );
        assert_eq!(cfg.discussion.default_rounds, Some(1));
        assert_eq!(cfg.discussion.default_manager.as_deref(), Some("gemini"));
        assert_eq!(
            cfg.discussion.default_participants,
            vec!["architecture=gemini"]
        );
        assert_eq!(cfg.discussion.docs_agent.as_deref(), Some("gemini"));
    }

    #[test]
    fn parses_backend_descriptor_blocks() {
        use crate::backend_descriptor::{BackendKind, PromptDelivery};

        let cfg: SwarmConfig = toml::from_str(
            r#"
            [backend.local-echo]
            kind = "cli"
            command = "printf"
            args = ["%s", "{prompt}"]
            prompt = "arg"

            [backend.api]
            kind = "openai-compatible"
            api_key_env = "MY_API_KEY"
            default_model = "gpt-4o-mini"
            "#,
        )
        .unwrap();

        let echo = cfg.backend.get("local-echo").expect("local-echo present");
        assert!(matches!(echo.kind, BackendKind::Cli));
        assert_eq!(echo.command.as_deref(), Some("printf"));
        assert!(matches!(echo.prompt, PromptDelivery::Arg));

        let api = cfg.backend.get("api").expect("api present");
        assert!(matches!(api.kind, BackendKind::OpenAiCompatible));
        assert_eq!(api.api_key_env.as_deref(), Some("MY_API_KEY"));
        assert_eq!(api.default_model.as_deref(), Some("gpt-4o-mini"));
    }

    #[test]
    fn backend_blocks_default_to_empty() {
        let cfg: SwarmConfig = toml::from_str("").unwrap();
        assert!(cfg.backend.is_empty());
    }

    #[test]
    fn empty_config_uses_built_in_defaults() {
        // A sparse config (no [reliability]) must still load, with default
        // retry policy and an empty fallback chain.
        let cfg: SwarmConfig = toml::from_str("default_agent = \"gemini\"\n").unwrap();
        assert!(cfg.routes.is_empty());
        assert_eq!(cfg.reliability.retry_attempts, 1);
        assert_eq!(cfg.reliability.retry_backoff_ms, 1_500);
        assert!(cfg.reliability.fallback_chain.is_empty());
    }

    #[test]
    fn unknown_sections_are_ignored() {
        // Forward-compat: a config carrying sections this binary doesn't model
        // (auto_scale and future nested fields) must not fail to parse.
        let cfg: SwarmConfig = toml::from_str(
            r#"
            [auto_scale]
            enabled = true
            threshold = 60

            [design]
            default_rounds = 2

            [reliability]
            fallback_chain = ["claude:sonnet"]
            "#,
        )
        .unwrap();
        assert_eq!(cfg.reliability.fallback_chain, vec!["claude:sonnet"]);
        assert_eq!(cfg.reliability.retry_attempts, 1);
        assert_eq!(cfg.design.default_rounds, Some(2));
    }

    #[test]
    fn context_auto_inject_defaults_false() {
        // Default config must have auto_inject = false (opt-in only).
        let cfg: SwarmConfig = toml::from_str("").unwrap();
        assert!(!cfg.context.auto_inject);
    }

    #[test]
    fn context_auto_inject_parses_true() {
        let cfg: SwarmConfig = toml::from_str(
            r#"
            [context]
            auto_inject = true
            "#,
        )
        .unwrap();
        assert!(cfg.context.auto_inject);
    }

    #[test]
    fn context_section_absent_does_not_fail() {
        // A config with no [context] section at all must not fail to parse.
        let cfg: SwarmConfig = toml::from_str(
            r#"
            [reliability]
            retry_attempts = 2
            "#,
        )
        .unwrap();
        assert!(!cfg.context.auto_inject);
    }

    // ------------------------------------------------------------------
    // [settings] section tests
    // ------------------------------------------------------------------

    #[test]
    fn settings_docs_default_defaults_to_false() {
        // Built-in default: docs_default = false (opt-in, not opt-out).
        let cfg: SwarmConfig = toml::from_str("").unwrap();
        assert!(!cfg.settings.docs_default);
        assert!(cfg.settings.direct_persona.is_none());
    }

    #[test]
    fn settings_docs_default_parses_true() {
        let cfg: SwarmConfig = toml::from_str(
            r#"
            [settings]
            docs_default = true
            direct_persona = "compact-manager"
            "#,
        )
        .unwrap();
        assert!(cfg.settings.docs_default);
        assert_eq!(
            cfg.settings.direct_persona.as_deref(),
            Some("compact-manager")
        );
    }

    #[test]
    fn settings_section_absent_does_not_fail() {
        // A config with no [settings] section must still parse cleanly.
        let cfg: SwarmConfig = toml::from_str(
            r#"
            [reliability]
            retry_attempts = 3
            "#,
        )
        .unwrap();
        assert!(!cfg.settings.docs_default);
    }

    // ------------------------------------------------------------------
    // read_settings_at / write_settings_at round-trip
    // ------------------------------------------------------------------

    #[test]
    fn settings_read_at_missing_file_returns_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nonexistent.toml");
        let settings = read_settings_at(&path);
        assert!(!settings.docs_default);
        assert!(settings.direct_persona.is_none());
    }

    #[test]
    fn settings_write_then_read_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");

        let s = Settings {
            docs_default: true,
            direct_persona: Some("compact-manager".to_string()),
        };
        write_settings_at(&path, &s).unwrap();

        let loaded = read_settings_at(&path);
        assert!(loaded.docs_default);
        assert_eq!(loaded.direct_persona.as_deref(), Some("compact-manager"));
    }

    #[test]
    fn settings_write_preserves_unrelated_keys() {
        // Critical regression guard: writing [settings] must NOT clobber
        // default_timeout, default_agent, or other unmodeled sections.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");

        // Seed a config with a flat key + an extra section + no [settings].
        std::fs::write(
            &path,
            r#"
default_timeout = 120

[swarm]
default_manager = "claude:sonnet"

[reliability]
retry_attempts = 3
"#,
        )
        .unwrap();

        let s = Settings {
            docs_default: true,
            direct_persona: Some("systems-architect".to_string()),
        };
        write_settings_at(&path, &s).unwrap();

        // The unrelated keys must still be present and parseable.
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(
            content.contains("default_timeout"),
            "default_timeout was clobbered:\n{content}"
        );
        // The new settings section must be set correctly.
        let cfg: SwarmConfig = toml::from_str(&content).unwrap();
        assert!(cfg.settings.docs_default);
        assert_eq!(
            cfg.settings.direct_persona.as_deref(),
            Some("systems-architect")
        );
        assert_eq!(cfg.reliability.retry_attempts, 3);
    }

    #[test]
    fn settings_write_idempotent() {
        // Writing twice with the same value produces a stable result.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");

        let s = Settings {
            docs_default: false,
            direct_persona: None,
        };
        write_settings_at(&path, &s).unwrap();
        write_settings_at(&path, &s).unwrap();

        let loaded = read_settings_at(&path);
        assert!(!loaded.docs_default);
    }

    // ------------------------------------------------------------------
    // resolve_docs helper
    // ------------------------------------------------------------------

    #[test]
    fn resolve_docs_cli_on_wins_over_config_false() {
        assert!(super::resolve_docs(Some(true), false));
    }

    #[test]
    fn resolve_docs_cli_off_wins_over_config_true() {
        assert!(!super::resolve_docs(Some(false), true));
    }

    #[test]
    fn resolve_docs_absent_falls_back_to_config_true() {
        assert!(super::resolve_docs(None, true));
    }

    #[test]
    fn resolve_docs_absent_falls_back_to_config_false() {
        assert!(!super::resolve_docs(None, false));
    }

    #[test]
    fn resolve_docs_built_in_default_is_false() {
        // No CLI flag, config default (false) => false.
        let cfg = SwarmConfig::default();
        assert!(!super::resolve_docs(None, cfg.settings.docs_default));
    }
}
