//! `BackendRegistry` resolves agent backends by stable string id. It is seeded
//! with the four built-in backends (under their canonical ids and legacy
//! aliases) and accepts descriptors from config. No id is hardcoded in the
//! dispatch path — adding a backend is a registry insertion, which is what lets
//! anyone add an agent without touching engine code.

use std::collections::HashMap;

use swarm_kernel::backend_descriptor::{BackendDescriptor, BackendKind};

use crate::cli_backend::CliBackend;
use crate::executor::{AgentBackend, ClaudeBackend, CodexBackend};
use swarm_kernel::backend_abi::{
    BackendCaps, BackendError, BackendRequest, BackendSink, RunOutcome,
};

/// A backend that resolves but fails loudly at execute time. Used for kinds
/// that parse but cannot run in this build — a feature is off, or the kind is
/// not yet implemented. Never silently succeeds (no-silent-fallback rule).
struct UnavailableBackend {
    id: String,
    reason: String,
}

impl UnavailableBackend {
    fn new(id: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            reason: reason.into(),
        }
    }
}

impl AgentBackend for UnavailableBackend {
    fn id(&self) -> &str {
        &self.id
    }

    fn ready(&self) -> Result<(), BackendError> {
        Err(BackendError::NotReady(format!(
            "backend `{}` is unavailable: {}.",
            self.id, self.reason
        )))
    }

    fn run(
        &self,
        _req: &BackendRequest,
        _sink: &mut dyn BackendSink,
    ) -> Result<RunOutcome, BackendError> {
        Err(BackendError::NotReady(format!(
            "backend `{}` is unavailable: {}.",
            self.id, self.reason
        )))
    }

    fn capabilities(&self) -> BackendCaps {
        BackendCaps {
            streaming: false,
            cancellation: false,
        }
    }
}

pub struct BackendRegistry {
    backends: HashMap<String, Box<dyn AgentBackend>>,
}

fn builtin_backend(canonical: &str) -> Box<dyn AgentBackend> {
    match canonical {
        "codex" => Box::new(CodexBackend),
        "claude" => Box::new(ClaudeBackend),
        _ => unreachable!("non-canonical id `{canonical}` passed to builtin_backend"),
    }
}

impl BackendRegistry {
    pub fn new() -> Self {
        Self {
            backends: HashMap::new(),
        }
    }

    /// Registry seeded with built-ins plus every `[backend.<id>]` descriptor
    /// from config — the startup registry the dispatch path uses. A descriptor
    /// sharing a built-in id shadows it (config wins), which lets an operator
    /// re-point a name at their own wrapper without touching engine code.
    pub fn from_config(config: &swarm_kernel::config::SwarmConfig) -> Self {
        let mut reg = Self::with_builtins();
        for (id, descriptor) in &config.backend {
            reg.register_descriptor(id.clone(), descriptor.clone());
        }
        reg
    }

    /// Registry seeded with the built-in backends under canonical ids plus the
    /// aliases accepted by `parse_agent_choice`, so existing configs resolve.
    pub fn with_builtins() -> Self {
        let mut reg = Self::new();
        // (id, canonical) — canonical ids map to themselves; the rest are aliases
        // mirroring parse_agent_choice in args.rs.
        let entries: &[(&str, &str)] = &[
            ("codex", "codex"),
            ("claude", "claude"),
            ("openai", "codex"),
            ("anthropic", "claude"),
        ];
        for (id, canonical) in entries {
            reg.backends
                .insert(id.to_string(), builtin_backend(canonical));
        }
        reg
    }

    /// Add (or replace) a descriptor-driven backend under `id`, constructing the
    /// concrete backend from `descriptor.kind`. Infallible: kinds that cannot
    /// run in this build resolve to an [`UnavailableBackend`] that errors at
    /// execute time (consistent with how a `cli` descriptor with a missing
    /// binary, or a built-in with an absent CLI, error lazily).
    pub fn register_descriptor(&mut self, id: impl Into<String>, descriptor: BackendDescriptor) {
        let id = id.into();
        let backend: Box<dyn AgentBackend> = match &descriptor.kind {
            BackendKind::Cli => Box::new(CliBackend::new(id.clone(), descriptor)),
            BackendKind::OpenAiCompatible => {
                #[cfg(feature = "openai")]
                {
                    Box::new(crate::openai_backend::OpenAiCompatibleBackend::new(
                        id.clone(),
                        descriptor,
                    ))
                }
                #[cfg(not(feature = "openai"))]
                {
                    Box::new(UnavailableBackend::new(
                        id.clone(),
                        "the `openai` cargo feature is not compiled in",
                    ))
                }
            }
            BackendKind::Native => {
                #[cfg(feature = "native")]
                {
                    Box::new(crate::native_backend::NativeBackend::new(
                        id.clone(),
                        descriptor,
                    ))
                }
                #[cfg(not(feature = "native"))]
                {
                    Box::new(UnavailableBackend::new(
                        id.clone(),
                        "the `native` cargo feature is not compiled in",
                    ))
                }
            }
        };
        self.backends.insert(id, backend);
    }

    /// Add (or replace) a custom [`AgentBackend`] trait implementation under
    /// `id` — the library-embedding escape hatch for backends a descriptor
    /// can't express. The `swarm` CLI itself extends via config descriptors;
    /// this method is for programs that embed the engine as a library and
    /// construct their own registry. The backend's own `id()` should agree
    /// with the registry key for coherent error messages.
    pub fn register(&mut self, id: impl Into<String>, backend: Box<dyn AgentBackend>) {
        self.backends.insert(id.into(), backend);
    }

    /// All registered backend ids, sorted. Used by health checks (`doctor`)
    /// that walk every backend.
    pub fn ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.backends.keys().cloned().collect();
        ids.sort_unstable();
        ids
    }

    /// Resolve a backend by id, or a clear error listing the available ids.
    pub fn resolve(&self, id: &str) -> Result<&dyn AgentBackend, String> {
        self.backends.get(id).map(|b| b.as_ref()).ok_or_else(|| {
            let mut ids: Vec<&str> = self.backends.keys().map(String::as_str).collect();
            ids.sort_unstable();
            format!(
                "Error: backend `{id}` not registered; available: {}.",
                ids.join(", ")
            )
        })
    }
}

impl Default for BackendRegistry {
    fn default() -> Self {
        Self::with_builtins()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use swarm_kernel::backend_descriptor::{BackendDescriptor, BackendKind, PromptDelivery};

    #[test]
    fn resolves_builtin_ids_including_legacy_aliases() {
        let reg = BackendRegistry::with_builtins();
        for id in ["codex", "claude"] {
            assert!(
                reg.resolve(id).is_ok(),
                "expected builtin `{id}` to resolve"
            );
        }
        assert!(reg.resolve("anthropic").is_ok());
        assert!(reg.resolve("openai").is_ok());
    }

    #[test]
    fn ids_lists_every_registered_backend_sorted() {
        let mut reg = BackendRegistry::with_builtins();
        reg.register_descriptor(
            "aaa-first",
            BackendDescriptor {
                kind: BackendKind::Cli,
                command: Some("printf".to_string()),
                ..Default::default()
            },
        );
        let ids = reg.ids();
        assert_eq!(
            ids,
            vec!["aaa-first", "anthropic", "claude", "codex", "openai"]
        );
    }

    #[test]
    fn unknown_id_lists_available_backends() {
        let reg = BackendRegistry::with_builtins();
        let err = match reg.resolve("nope") {
            Err(e) => e,
            Ok(_) => panic!("expected resolve to fail for unknown id `nope`"),
        };
        assert!(err.contains("nope"));
        assert!(
            err.contains("available"),
            "error should list available ids: {err}"
        );
        // The available-ids list must be sorted (spec requirement).
        let available_section = err
            .split("available: ")
            .nth(1)
            .expect("error should have an 'available:' section");
        let listed: Vec<&str> = available_section
            .trim_end_matches('.')
            .split(", ")
            .collect();
        assert!(
            listed.windows(2).all(|w| w[0] <= w[1]),
            "available ids should be sorted: {err}"
        );
    }

    #[test]
    fn registers_a_cli_descriptor() {
        let mut reg = BackendRegistry::with_builtins();
        reg.register_descriptor(
            "ollama",
            BackendDescriptor {
                kind: BackendKind::Cli,
                command: Some("ollama".to_string()),
                args: vec!["run".to_string(), "{model}".to_string()],
                prompt: PromptDelivery::Stdin,
                ..Default::default()
            },
        );
        assert!(reg.resolve("ollama").is_ok());
    }

    #[test]
    fn from_config_loads_descriptor_blocks_alongside_builtins() {
        let config: swarm_kernel::config::SwarmConfig = toml::from_str(
            r#"
            [backend.local-echo]
            kind = "cli"
            command = "printf"
            "#,
        )
        .unwrap();
        let reg = BackendRegistry::from_config(&config);
        assert!(reg.resolve("local-echo").is_ok());
        assert!(reg.resolve("codex").is_ok(), "builtins still present");
    }

    #[test]
    fn config_descriptor_shadows_builtin_id() {
        // Re-pointing a built-in name at a descriptor is allowed (config wins).
        // A `native` descriptor is deterministic here: without the feature it
        // resolves to an UnavailableBackend whose ready() errs, regardless of
        // which CLIs the host has installed.
        let config: swarm_kernel::config::SwarmConfig = toml::from_str(
            r#"
            [backend.codex]
            kind = "native"
            "#,
        )
        .unwrap();
        let reg = BackendRegistry::from_config(&config);
        let backend = reg.resolve("codex").unwrap();
        assert_eq!(backend.id(), "codex");
        #[cfg(not(feature = "native"))]
        assert!(backend.ready().is_err(), "descriptor shadowed the builtin");
    }

    #[test]
    fn registers_openai_descriptor_resolves() {
        let mut reg = BackendRegistry::with_builtins();
        reg.register_descriptor(
            "api",
            BackendDescriptor {
                kind: BackendKind::OpenAiCompatible,
                api_key_env: Some("SOME_KEY_VAR".into()),
                default_model: Some("gpt-4o-mini".into()),
                ..Default::default()
            },
        );
        assert!(reg.resolve("api").is_ok());
    }

    /// Without the `openai` feature, an openai descriptor still resolves but
    /// fails loudly at execute time — never a silent no-op.
    #[cfg(not(feature = "openai"))]
    #[test]
    fn openai_without_feature_errors_loudly() {
        let mut reg = BackendRegistry::with_builtins();
        reg.register_descriptor(
            "api",
            BackendDescriptor {
                kind: BackendKind::OpenAiCompatible,
                ..Default::default()
            },
        );
        let backend = reg.resolve("api").unwrap();
        let err = backend.ready().unwrap_err().to_string();
        assert!(
            err.to_lowercase().contains("openai") && err.contains("api"),
            "feature-off openai backend should error loudly naming the feature/id: {err}"
        );
    }

    /// Without the `native` feature, a native descriptor still resolves but
    /// fails loudly at ready/execute time — never a silent no-op.
    #[cfg(not(feature = "native"))]
    #[test]
    fn native_without_feature_errors_loudly() {
        let mut reg = BackendRegistry::with_builtins();
        reg.register_descriptor(
            "local",
            BackendDescriptor {
                kind: BackendKind::Native,
                provider: Some("api".into()),
                ..Default::default()
            },
        );
        let backend = reg.resolve("local").unwrap();
        let err = backend.ready().unwrap_err().to_string();
        assert!(
            err.to_lowercase().contains("native") && err.contains("local"),
            "feature-off native backend should error loudly naming the feature/id: {err}"
        );
    }

    /// With the `native` feature on, a native descriptor resolves to a real
    /// `NativeBackend` (registry resolve Ok).
    #[cfg(feature = "native")]
    #[test]
    fn native_descriptor_resolves_to_native_backend() {
        let mut reg = BackendRegistry::with_builtins();
        reg.register_descriptor(
            "local",
            BackendDescriptor {
                kind: BackendKind::Native,
                provider: Some("api".into()),
                ..Default::default()
            },
        );
        assert!(
            reg.resolve("local").is_ok(),
            "native descriptor should resolve under the `native` feature"
        );
    }

    /// The library-embedding escape hatch: a custom `AgentBackend` trait impl
    /// registers directly and runs through the same resolve path descriptors
    /// use — no descriptor, no engine code change.
    #[test]
    fn register_accepts_custom_trait_impl() {
        use swarm_kernel::backend_abi::{
            BackendCaps, BackendError, BackendRequest, BackendSink, RunOutcome,
        };

        struct CannedBackend;
        impl AgentBackend for CannedBackend {
            fn id(&self) -> &str {
                "canned"
            }
            fn ready(&self) -> Result<(), BackendError> {
                Ok(())
            }
            fn run(
                &self,
                req: &BackendRequest,
                sink: &mut dyn BackendSink,
            ) -> Result<RunOutcome, BackendError> {
                let text = format!("canned:{}", req.prompt);
                sink.stdout_chunk(&text);
                Ok(RunOutcome {
                    stdout: text,
                    ..Default::default()
                })
            }
            fn capabilities(&self) -> BackendCaps {
                BackendCaps::default()
            }
        }

        let mut reg = BackendRegistry::with_builtins();
        reg.register("canned", Box::new(CannedBackend));
        let backend = reg.resolve("canned").unwrap();
        assert!(backend.ready().is_ok());

        let cwd = std::env::temp_dir();
        let req = BackendRequest {
            prompt: "hello",
            model: None,
            cwd: &cwd,
            timeout: std::time::Duration::from_secs(5),
            quiet: true,
            allow_bypass_permissions: false,
            env_policy: swarm_kernel::backend_abi::EnvPolicy::Inherit,
            cancel: swarm_kernel::backend_abi::CancelToken::new(),
        };
        let mut sink = swarm_kernel::backend_abi::NullSink;
        let out = backend.run(&req, &mut sink).unwrap();
        assert_eq!(out.stdout, "canned:hello");
    }
}
