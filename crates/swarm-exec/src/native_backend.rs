//! `NativeBackend` runs the in-process single-agent loop from the manager core
//! — no external CLI, no subprocess. A `native` descriptor names a stored
//! provider configuration by id; this backend resolves that configuration from
//! the provider registry, builds a provider + the built-in tool set, and drives
//! the agent loop to a final answer, which it emits as a single stdout chunk.
//!
//! Secrets are never logged, written to captured output, or placed in any error
//! string: the registry resolves the key into the in-memory `ProviderConfig`
//! and it travels only as far as the provider's HTTP request.
//!
//! Gated behind the `native` cargo feature so the default build pulls in no
//! manager / async-runtime / HTTP dependency.

use std::path::PathBuf;
use std::sync::Arc;

use swarm_manager::{
    create_provider, Agent, AgentConfig, AgentError, KeyStatus, Provider, ProviderConfig,
    ProviderError, ProviderRegistry, ToolRegistry,
};

use swarm_kernel::backend_abi::{
    BackendCaps, BackendError, BackendRequest, BackendSink, RunOutcome, TokenUsage,
};
use swarm_kernel::backend_descriptor::BackendDescriptor;

use crate::executor::AgentBackend;

/// Default system prompt for the native backend when none is configured.
const DEFAULT_SYSTEM_PROMPT: &str = "You are a helpful AI assistant.";

/// Where the native backend resolves a `NativeBackend::new` provider registry
/// when no explicit data dir is injected. Aligns with the swarm's single
/// `swarm_home()` data-root convention used elsewhere in the engine.
fn default_data_dir() -> PathBuf {
    swarm_store::store::swarm_home()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("providers")
}

/// How a [`NativeBackend`] obtains its provider.
enum Source {
    /// Resolve `descriptor.provider` against a registry rooted at `data_dir`.
    /// The production path.
    Descriptor {
        descriptor: BackendDescriptor,
        data_dir: PathBuf,
    },
    /// A pre-built provider, injected for network-free unit tests. The
    /// `default_model` mirrors the descriptor field a real run would honour.
    Provider {
        provider: Arc<dyn Provider>,
        default_model: Option<String>,
    },
}

/// A descriptor-driven backend that runs the manager's single-agent loop in
/// process.
pub struct NativeBackend {
    id: String,
    source: Source,
}

impl NativeBackend {
    /// Production constructor: resolve the descriptor's provider against the
    /// default data dir.
    pub fn new(id: impl Into<String>, descriptor: BackendDescriptor) -> Self {
        Self::with_data_dir(id, descriptor, default_data_dir())
    }

    /// Construct against an explicit provider-registry data dir. The seam used
    /// by `ready()`-path tests (point a registry at a temp dir).
    pub fn with_data_dir(
        id: impl Into<String>,
        descriptor: BackendDescriptor,
        data_dir: PathBuf,
    ) -> Self {
        Self {
            id: id.into(),
            source: Source::Descriptor {
                descriptor,
                data_dir,
            },
        }
    }

    /// Test seam: drive `run` with an injected provider so the
    /// `AgentTurn` → `RunOutcome`/sink mapping is exercised without a network
    /// or a registry. `default_model` stands in for the descriptor's
    /// `default_model` when the request supplies none.
    pub fn from_provider(
        id: impl Into<String>,
        provider: Arc<dyn Provider>,
        default_model: Option<String>,
    ) -> Self {
        Self {
            id: id.into(),
            source: Source::Provider {
                provider,
                default_model,
            },
        }
    }

    /// Open the provider registry and resolve the descriptor's named provider
    /// configuration. Used by both `ready()` and the descriptor `run` path.
    fn resolve_config(
        &self,
        descriptor: &BackendDescriptor,
        data_dir: &PathBuf,
    ) -> Result<ProviderConfig, BackendError> {
        let provider_id = descriptor.provider.as_deref().ok_or_else(|| {
            BackendError::NotReady(format!(
                "backend `{}` has no `provider` set in its native descriptor.",
                self.id
            ))
        })?;

        let registry = ProviderRegistry::open(data_dir).map_err(|e| {
            BackendError::NotReady(format!(
                "backend `{}` could not open its provider registry: {e}",
                self.id
            ))
        })?;

        let config = registry
            .get(provider_id)
            .map_err(|e| {
                BackendError::NotReady(format!(
                    "backend `{}` failed to read provider `{provider_id}`: {e}",
                    self.id
                ))
            })?
            .ok_or_else(|| {
                BackendError::NotReady(format!(
                    "backend `{}` references provider `{provider_id}`, which is not registered.",
                    self.id
                ))
            })?;

        match config.key_status() {
            KeyStatus::Healthy => Ok(config),
            KeyStatus::Absent => Err(BackendError::NotReady(format!(
                "backend `{}` provider `{provider_id}` has no usable API key.",
                self.id
            ))),
            KeyStatus::Stranded => Err(BackendError::NotReady(format!(
                "backend `{}` provider `{provider_id}` has a stored key that could not be \
                 decrypted (stranded); reconfigure the provider.",
                self.id
            ))),
        }
    }

    /// Build the built-in tool set the native agent offers. File tools resolve
    /// against the request's working directory; there is no sandbox.
    fn build_tools(req: &BackendRequest) -> ToolRegistry {
        use swarm_manager::tools::{
            ExecTool, ListDirTool, ReadFileTool, WebFetchTool, WriteFileTool,
        };

        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(ExecTool::default()));
        registry.register(Arc::new(ReadFileTool::workspace(req.cwd.to_path_buf())));
        registry.register(Arc::new(WriteFileTool::workspace(req.cwd.to_path_buf())));
        registry.register(Arc::new(ListDirTool::workspace(req.cwd.to_path_buf())));
        // Web search needs an API key, so it is omitted here; web fetch needs
        // none. The scripted-provider tests do not exercise tools, but a real
        // run gets a functional set.
        registry.register(Arc::new(WebFetchTool::default()));
        registry
    }

    /// Run the agent loop against a concrete provider and map the result onto
    /// the backend ABI. Shared by the descriptor and injected-provider paths so
    /// both exercise the same mapping.
    fn run_with_provider(
        &self,
        provider: Arc<dyn Provider>,
        model: Option<String>,
        req: &BackendRequest,
        sink: &mut dyn BackendSink,
    ) -> Result<RunOutcome, BackendError> {
        let mut config = AgentConfig {
            system_prompt: DEFAULT_SYSTEM_PROMPT.to_string(),
            ..AgentConfig::default()
        };
        // The agent loop bakes the model into the provider via
        // `create_provider`; `AgentConfig.model` is not consulted by the loop.
        // We keep it in sync for any future reader, but selection happened when
        // the provider was built.
        if let Some(m) = model {
            config.model = m;
        }

        let tools = Self::build_tools(req);
        let mut agent = Agent::new(provider, tools, config);

        let turn = agent
            .run_blocking(req.prompt)
            .map_err(|e| self.map_agent_error(e))?;

        sink.stdout_chunk(&turn.text);

        let token_usage = match (turn.usage.prompt_tokens, turn.usage.completion_tokens) {
            (0, 0) => None,
            (input, output) => Some(TokenUsage {
                input: Some(input as u64),
                output: Some(output as u64),
            }),
        };

        Ok(RunOutcome {
            exit_status: None,
            stdout: turn.text,
            stderr: String::new(),
            timed_out: false,
            retryable: false,
            token_usage,
        })
    }

    /// Map a manager [`AgentError`] / [`ProviderError`] onto a typed
    /// [`BackendError`]. No key material can reach these strings — the manager
    /// errors never carry it.
    fn map_agent_error(&self, err: AgentError) -> BackendError {
        match err {
            AgentError::Provider(p) => self.map_provider_error(p),
            // The loop folds tool failures back to the model itself; a `Tool`
            // error reaching here is a harness-level failure (e.g. runtime
            // build), which is a protocol-level problem, not a transient one.
            AgentError::Tool(detail) => {
                BackendError::Protocol(format!("backend `{}` agent error: {detail}", self.id))
            }
            AgentError::MaxIterationsExceeded(n) => BackendError::Protocol(format!(
                "backend `{}` agent stopped after {n} tool iterations without a final response.",
                self.id
            )),
        }
    }

    fn map_provider_error(&self, err: ProviderError) -> BackendError {
        match err {
            ProviderError::NotConfigured(detail) | ProviderError::NotImplemented(detail) => {
                BackendError::NotReady(format!("backend `{}`: {detail}", self.id))
            }
            ProviderError::InvalidApiKey => BackendError::NotReady(format!(
                "backend `{}`: provider rejected the API key.",
                self.id
            )),
            ProviderError::ModelNotFound { model } => BackendError::Upstream {
                status: Some(404),
                retryable: false,
                detail: format!(
                    "backend `{}`: model `{model}` not found on provider.",
                    self.id
                ),
            },
            ProviderError::RateLimited {
                retry_after_seconds,
            } => BackendError::Upstream {
                status: Some(429),
                retryable: true,
                detail: format!(
                    "backend `{}`: rate limited (retry after {retry_after_seconds:?}s).",
                    self.id
                ),
            },
            ProviderError::Network(detail) => BackendError::Upstream {
                status: None,
                retryable: true,
                detail: format!("backend `{}`: network failure: {detail}", self.id),
            },
            ProviderError::Upstream { status, message } => BackendError::Upstream {
                status: Some(status),
                retryable: status >= 500,
                detail: format!("backend `{}`: upstream {status}: {message}", self.id),
            },
            ProviderError::ParseResponse(detail) => BackendError::Protocol(format!(
                "backend `{}`: failed to parse provider response: {detail}",
                self.id
            )),
        }
    }
}

impl AgentBackend for NativeBackend {
    fn id(&self) -> &str {
        &self.id
    }

    fn ready(&self) -> Result<(), BackendError> {
        match &self.source {
            Source::Descriptor {
                descriptor,
                data_dir,
            } => self.resolve_config(descriptor, data_dir).map(|_| ()),
            // An injected provider is, by construction, ready.
            Source::Provider { .. } => Ok(()),
        }
    }

    fn run(
        &self,
        req: &BackendRequest,
        sink: &mut dyn BackendSink,
    ) -> Result<RunOutcome, BackendError> {
        match &self.source {
            Source::Descriptor {
                descriptor,
                data_dir,
            } => {
                let mut config = self.resolve_config(descriptor, data_dir)?;
                // Model selection: explicit request override, else the
                // descriptor's default. The loop bakes the model into the
                // provider via `create_provider`, which reads
                // `ProviderConfig.models.first()` — so the override must land
                // there, not on `AgentConfig`.
                let model = req
                    .model
                    .map(str::to_string)
                    .or_else(|| descriptor.default_model.clone());
                if let Some(m) = model.clone() {
                    config.models = vec![m];
                }
                let provider = create_provider(&config).map_err(|e| self.map_provider_error(e))?;
                self.run_with_provider(provider, model, req, sink)
            }
            Source::Provider {
                provider,
                default_model,
            } => {
                let model = req
                    .model
                    .map(str::to_string)
                    .or_else(|| default_model.clone());
                self.run_with_provider(Arc::clone(provider), model, req, sink)
            }
        }
    }

    fn capabilities(&self) -> BackendCaps {
        // The loop returns a single final answer, not a token stream, and does
        // not honour the cancel token yet.
        BackendCaps {
            streaming: false,
            cancellation: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::Mutex;
    use std::time::Duration;
    use swarm_kernel::backend_abi::{CancelToken, EnvPolicy};
    use swarm_kernel::backend_descriptor::BackendKind;
    use swarm_manager::{LLMResponse, Message, Usage};

    /// A `BackendRequest` over a temp cwd with a fixed model.
    fn req<'a>(prompt: &'a str, cwd: &'a std::path::Path) -> BackendRequest<'a> {
        BackendRequest {
            prompt,
            model: Some("test-model"),
            cwd,
            timeout: Duration::from_secs(30),
            quiet: true,
            allow_bypass_permissions: false,
            env_policy: EnvPolicy::Inherit,
            cancel: CancelToken::new(),
        }
    }

    /// Collects every stdout chunk; used to assert the final answer is emitted.
    struct CollectingSink {
        stdout: Vec<String>,
    }

    impl CollectingSink {
        fn new() -> Self {
            Self { stdout: Vec::new() }
        }
    }

    impl BackendSink for CollectingSink {
        fn stdout_chunk(&mut self, text: &str) {
            self.stdout.push(text.to_string());
        }
        fn stderr_chunk(&mut self, _text: &str) {}
    }

    /// A provider that returns a single scripted final answer. No network.
    struct ScriptedProvider {
        response: Mutex<Option<LLMResponse>>,
    }

    impl ScriptedProvider {
        fn final_text(text: &str, usage: Usage) -> Self {
            Self {
                response: Mutex::new(Some(LLMResponse {
                    content: Some(text.to_string()),
                    reasoning_content: None,
                    tool_calls: Vec::new(),
                    finish_reason: "stop".to_string(),
                    usage,
                })),
            }
        }
    }

    #[async_trait]
    impl Provider for ScriptedProvider {
        async fn chat(
            &self,
            _messages: Vec<Message>,
            _tools: Option<Vec<serde_json::Value>>,
        ) -> Result<LLMResponse, ProviderError> {
            self.response
                .lock()
                .unwrap()
                .take()
                .ok_or_else(|| ProviderError::ParseResponse("no scripted response".to_string()))
        }
    }

    #[test]
    fn capabilities_are_non_streaming_non_cancelling() {
        let backend = NativeBackend::new(
            "local",
            BackendDescriptor {
                kind: BackendKind::Native,
                provider: Some("api".into()),
                ..Default::default()
            },
        );
        let caps = backend.capabilities();
        assert!(!caps.streaming);
        assert!(!caps.cancellation);
    }

    #[test]
    fn ready_is_not_ready_when_provider_absent() {
        // Registry pointed at an empty temp dir: the named provider does not
        // exist, so `ready()` must report NotReady — born-clean, no network.
        let dir = tempfile::tempdir().unwrap();
        let backend = NativeBackend::with_data_dir(
            "local",
            BackendDescriptor {
                kind: BackendKind::Native,
                provider: Some("missing-provider".into()),
                ..Default::default()
            },
            dir.path().to_path_buf(),
        );
        let err = backend.ready().unwrap_err();
        match err {
            BackendError::NotReady(detail) => {
                assert!(
                    detail.contains("missing-provider") && detail.contains("not registered"),
                    "should name the missing provider: {detail}"
                );
            }
            other => panic!("expected NotReady, got {other:?}"),
        }
    }

    #[test]
    fn ready_is_not_ready_when_provider_key_absent() {
        // The named provider IS registered, but has no usable key (no
        // ciphertext, no env var): `ready()` must gate on KeyStatus::Absent.
        // Real registry at a temp dir — born-clean, no network.
        use swarm_manager::{ProviderConfig, ProviderRegistry, ProviderType};
        let dir = tempfile::tempdir().unwrap();
        let registry = ProviderRegistry::open(&dir.path().to_path_buf()).unwrap();
        // `api_key = None` ⇒ nothing is written to disk; KeyStatus::Absent.
        let id = registry
            .add(ProviderConfig::new(
                "Keyless".into(),
                ProviderType::OpenAI,
                None,
                None,
            ))
            .unwrap();
        let backend = NativeBackend::with_data_dir(
            "local",
            BackendDescriptor {
                kind: BackendKind::Native,
                provider: Some(id),
                ..Default::default()
            },
            dir.path().to_path_buf(),
        );
        let err = backend.ready().unwrap_err();
        match err {
            BackendError::NotReady(detail) => assert!(
                detail.contains("no usable API key"),
                "absent key should surface as NotReady naming the missing key: {detail}"
            ),
            other => panic!("expected NotReady, got {other:?}"),
        }
    }

    #[test]
    fn ready_is_not_ready_when_descriptor_has_no_provider() {
        let dir = tempfile::tempdir().unwrap();
        let backend = NativeBackend::with_data_dir(
            "local",
            BackendDescriptor {
                kind: BackendKind::Native,
                provider: None,
                ..Default::default()
            },
            dir.path().to_path_buf(),
        );
        assert!(matches!(backend.ready(), Err(BackendError::NotReady(_))));
    }

    #[test]
    fn run_maps_final_answer_to_stdout_chunk_and_outcome() {
        // Inject a scripted provider so the AgentTurn → RunOutcome/sink mapping
        // is exercised with no network and no registry.
        let provider = Arc::new(ScriptedProvider::final_text(
            "the final answer",
            Usage {
                prompt_tokens: 12,
                completion_tokens: 8,
                total_tokens: 20,
            },
        ));
        let backend = NativeBackend::from_provider("local", provider, Some("default-model".into()));

        let cwd = tempfile::tempdir().unwrap();
        let mut sink = CollectingSink::new();
        let outcome = backend.run(&req("hello", cwd.path()), &mut sink).unwrap();

        // The final text is emitted as exactly one stdout chunk...
        assert_eq!(sink.stdout, vec!["the final answer".to_string()]);
        // ...and is also the RunOutcome's stdout.
        assert_eq!(outcome.stdout, "the final answer");
        assert_eq!(outcome.exit_status, None);
        assert!(!outcome.timed_out);
        assert!(!outcome.retryable);

        let usage = outcome.token_usage.expect("token usage mapped");
        assert_eq!(usage.input, Some(12));
        assert_eq!(usage.output, Some(8));
    }
}
