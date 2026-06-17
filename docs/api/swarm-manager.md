# swarm-manager

Standalone, mesh-free single-agent harness: a `Provider` abstraction over LLM
backends, a persistent provider registry with an at-rest credential vault, agent
presets, a tool registry plus built-in tools, the agent tool-loop, and a skills
loader.

## Overview

`swarm-manager` is the in-process "native" agent the rest of the workspace wraps
when it needs to run an LLM turn without shelling out to an external CLI. It owns
five concerns:

- **Providers** — the async [`Provider`](#trait-provider) trait (one chat-completion
  turn), the [`ProviderType`](#enum-providertype) kind enum, value types
  ([`Message`](#struct-message) / [`ToolCall`](#struct-toolcall) /
  [`LLMResponse`](#struct-llmresponse) / [`Usage`](#struct-usage)), typed
  [`ProviderError`](#enum-providererror)s with pure HTTP classification, the
  [`create_provider`](#fn-create_provider) factory, and per-vendor HTTP impls
  (OpenAI / Anthropic / Gemini / Ollama / LM Studio, behind the `http` feature).
- **Registry + vault** — [`ProviderRegistry`](#struct-providerregistry), a
  SQLite-backed store of [`ProviderConfig`](#struct-providerconfig) rows whose API
  keys are encrypted at rest by [`KeychainVault`](#struct-keychainvault)
  (ChaCha20-Poly1305, master key in the OS keychain) or, when no keychain exists,
  resolved at read time from `SWARM_PROVIDER_KEY_<ID>`. [`KeyStatus`](#enum-keystatus)
  gives a loud three-way health view (absent / healthy / stranded).
- **Presets** — [`Preset`](#struct-preset) (a saved named agent config) and
  [`PresetStore`](#struct-presetstore) (JSON-backed collection with an active +
  default pointer), plus the runtime [`AgentConfig`](#struct-agentconfig) the loop
  consumes.
- **Tools** — the [`Tool`](#trait-tool) trait, the deterministic
  [`ToolRegistry`](#struct-toolregistry), and built-in tools: `exec`, file
  read/write/edit/list, and web search/fetch.
- **Agent loop** — [`Agent`](#struct-agent), the chat → tool-dispatch → chat loop
  capped at `max_tool_iterations` (`runtime` feature).
- **Skills** — [`parse_skill`](#fn-parse_skill) / [`load_skills`](#fn-load_skills)
  over `SKILL.md` files and [`SkillSet`](#struct-skillset) for system-prompt
  composition + tool gating.

Why it exists: it isolates "be an agent" from "orchestrate agents". The crate is a
clean standalone leaf so the engine can embed it (as the `native` backend) or the
CLI can manage credentials through it, without dragging in the orchestration
stack.

## Dependency position

```
swarm-manager   (standalone leaf — no sibling workspace crates)
```

`swarm-manager` sits **off** the main DAG. It depends only on external crates
(serde, serde_json, rusqlite, chacha20poly1305/hkdf/sha2, rand, hex, thiserror,
anyhow, async-trait, uuid, chrono, dirs, and per-platform `keyring`; optionally
tokio/regex behind `runtime` and reqwest behind `http`). It deliberately depends
on **no** workspace crate. Higher crates pull *it* in:

- `swarm-exec` wraps [`Agent`](#struct-agent) as its `NativeBackend` under the
  `native` feature. See [[swarm-exec#native-backend]].
- `swarm-cli` uses [`ProviderRegistry`](#struct-providerregistry) for the
  `provider` command group and [`load_skills`](#fn-load_skills) for `skills list`.
  See [[swarm-cli#provider-skills-commands]].

### Feature gates

- **default** — data types + traits only; no async runtime, no HTTP, no TLS.
- **`runtime`** — pulls tokio + regex; enables the [`Agent`](#struct-agent) loop and
  the runtime-backed built-in tools ([`ExecTool`](#struct-exectool),
  [`ReadFileTool`](#file-tools)/`WriteFileTool`/`EditFileTool`/`ListDirTool`).
- **`http`** (implies `runtime`) — pulls reqwest (rustls TLS); enables the concrete
  HTTP [`Provider`](#trait-provider) impls, a real [`create_provider`](#fn-create_provider),
  and the web tools ([`WebSearchTool`](#web-tools)/`WebFetchTool`).

## Concepts

Bullet index of the public surface re-exported from `lib.rs` (and the public
module items behind it).

### Providers (`provider/`)
- [`ProviderType`](#enum-providertype) — enum of provider kinds + their defaults (endpoint, models, key requirement).
- [`Provider`](#trait-provider) — async trait: one chat-completion turn.
- [`create_provider`](#fn-create_provider) — factory: build a `Provider` from a `ProviderConfig`.
- [`Message`](#struct-message) — a chat message (role + content + tool fields) with role constructors.
- [`ToolCall`](#struct-toolcall) — a flat tool-call request (id, name, JSON arguments).
- [`LLMResponse`](#struct-llmresponse) — a provider's completion (content, tool calls, finish reason, usage).
- [`Usage`](#struct-usage) — prompt/completion/total token counts.
- [`ProviderError`](#enum-providererror) — typed provider failures.
- [`classify_http_error`](#fn-classify_http_error) — pure status/header/body → `ProviderError`.
- [`AnthropicProvider` / `OpenAIProvider` / `GeminiProvider` / `OllamaProvider` / `LmStudioProvider`](#concrete-providers) — per-vendor HTTP impls (`http` feature).

### Registry + vault (`provider/registry.rs`, `provider/crypto.rs`)
- [`ProviderConfig`](#struct-providerconfig) — a configured provider instance (a registry row).
- [`ProviderRegistry`](#struct-providerregistry) — SQLite-backed CRUD over provider configs; keys encrypted at rest.
- [`KeyStatus`](#enum-keystatus) — three-way API-key health (absent / healthy / stranded).
- [`KeychainVault`](#struct-keychainvault) — ChaCha20-Poly1305 at-rest encryption keyed from the OS keychain.
- [`env_key_for`](#fn-env_key_for) — build the `SWARM_PROVIDER_KEY_<ID>` env-var name (vault fallback).

### Presets (`preset/`)
- [`Preset`](#struct-preset) — a saved, named agent configuration.
- [`PresetStore`](#struct-presetstore) — JSON-backed preset collection with active/default pointers.
- [`PresetRequest`](#struct-presetrequest) — create/update request for a preset.
- [`AgentConfig`](#struct-agentconfig) — the runtime config the agent loop consumes.
- [`ConfigUpdate`](#struct-configupdate) — partial in-place update to an `AgentConfig`.
- [`ConsumerPolicy`](#enum-consumerpolicy) — visibility marker (public / allowlist / internal).
- [`AgentOwner`](#enum-agentowner) — ownership marker (user / node).

### Tools (`tools/`)
- [`Tool`](#trait-tool) — trait every tool implements (name, description, params, execute).
- [`ToolRegistry`](#struct-toolregistry) — name-keyed, deterministically ordered tool registry.
- [`ExecTool`](#struct-exectool) — `exec`: run a shell command inside a workspace root (`runtime`).
- [`ReadFileTool` / `WriteFileTool` / `EditFileTool` / `ListDirTool`](#file-tools) — workspace-confined file tools (`runtime`).
- [`WebSearchTool` / `WebFetchTool`](#web-tools) — Brave search + SSRF-guarded fetch (`http`).

### Agent loop (`agent/`, `runtime` feature)
- [`Agent`](#struct-agent) — the single-agent chat/tool loop.
- [`AgentTurn`](#struct-agentturn) — the outcome of running the loop to a final message.
- [`ToolInvocation`](#struct-toolinvocation) — one executed tool call + its result.
- [`AgentError`](#enum-agenterror) — typed loop failures.

### Skills (`skills/`)
- [`parse_skill`](#fn-parse_skill) — parse one `SKILL.md` into a `Skill`.
- [`load_skills`](#fn-load_skills) — scan dirs for `<name>/SKILL.md`, with override-by-name.
- [`Skill`](#struct-skill) — a loaded skill (metadata + body).
- [`SkillSet`](#struct-skillset) — resolved selection: compose prompt + gate tools.
- [`SkillError`](#enum-skillerror) — frontmatter/structure parse failure.
- [`SkillLoadIssue`](#struct-skillloadissue) — a file that failed to load, with reason.
- [`SkillSelectionIssue`](#struct-skillselectionissue) — a requested name that resolved to nothing.

---

## API surface

### `enum ProviderType`

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ProviderType {
    None, OpenAI, #[default] Ollama, Gemini, LMStudio, MLX, Anthropic, OpenRouter, DeepSeek,
}

impl ProviderType {
    pub fn as_str(&self) -> &'static str;
    pub fn from_str(s: &str) -> Self;            // infallible; unknown → None
    pub fn requires_api_key(&self) -> bool;
    pub fn default_endpoint(&self) -> Option<&'static str>;
    pub fn suggested_models(&self) -> &'static [&'static str];
    pub fn legacy_model_aliases(&self) -> &'static [&'static str];
}
```

**Purpose:** the closed set of provider kinds and their static defaults. Default
is `Ollama` (local, no key). `from_str` is infallible — an unrecognized string
yields `None` rather than an error (and is not the fallible `FromStr` shape).
`requires_api_key` is true only for the cloud providers (OpenAI, Gemini,
Anthropic, OpenRouter, DeepSeek); local providers (Ollama, LM Studio, MLX) need
none. `suggested_models` powers new-preset suggestions; `legacy_model_aliases`
are still loadable but not first-choice.

**When to use:** to label a provider, look up its default endpoint/models, or
decide whether a key is required before configuring a [`ProviderConfig`](#struct-providerconfig).

Related: [[swarm-manager#struct-providerconfig]], [[swarm-manager#fn-create_provider]]

### `trait Provider`

```rust
#[async_trait]
pub trait Provider: Send + Sync {
    async fn chat(
        &self,
        messages: Vec<Message>,
        tools: Option<Vec<serde_json::Value>>,
    ) -> Result<LLMResponse, ProviderError>;
}
```

**Purpose:** the single capability of an LLM backend — perform one chat-completion
turn. `tools` is an optional OpenAI-format tools array (pass `None` for no tools).

**Params/returns:** the full conversation `messages` (system first) and optional
tool definitions → an [`LLMResponse`](#struct-llmresponse) or a typed
[`ProviderError`](#enum-providererror).

**When to use:** implement it for a new backend; call it (usually indirectly via
[`Agent`](#struct-agent)) to run a turn. Build a concrete instance with
[`create_provider`](#fn-create_provider).

Related: [[swarm-manager#fn-create_provider]], [[swarm-manager#struct-agent]], [[swarm-manager#struct-llmresponse]]

### `fn create_provider`

```rust
pub fn create_provider(config: &ProviderConfig) -> Result<Arc<dyn Provider>, ProviderError>;
```

**Purpose:** build a concrete `Provider` from a [`ProviderConfig`](#struct-providerconfig).

**Behaviour:** with the `http` feature on, constructs the HTTP-backed impl for
`config.provider_type` — model from `config.models.first()` (empty → provider
default), endpoint from `config.effective_endpoint()`, temperature `0.7`,
`max_tokens = None` (sampling knobs ride the agent/preset layer, not the registry
config). Cloud types with no resolvable key fail with
`ProviderError::NotConfigured`; `ProviderType::None` likewise. Without the `http`
feature, **every** type fails loudly with `ProviderError::NotImplemented` — never a
silent no-op.

**When to use:** when you have a registry row and want a callable provider. To run
a turn, hand the returned `Arc<dyn Provider>` to [`Agent::new`](#struct-agent).

Related: [[swarm-manager#trait-provider]], [[swarm-manager#struct-providerconfig]], [[swarm-manager#enum-providererror]]

### `struct Message`

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: Option<String>,
    pub reasoning_content: Option<String>,
    pub tool_calls: Option<Vec<ToolCall>>,
    pub tool_call_id: Option<String>,
}

impl Message {
    pub fn user(content: impl Into<String>) -> Self;
    pub fn assistant(content: impl Into<String>) -> Self;
    pub fn system(content: impl Into<String>) -> Self;
    pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self;
}
```

**Purpose:** one entry in a chat transcript. `role` is `"user"` / `"assistant"` /
`"system"` / `"tool"`. `tool_calls` carries an assistant turn's requested calls;
`tool_call_id` ties a `tool` message back to the call it answers. None-valued
optional fields are skipped on serialize.

**When to use:** to build the `messages` vector for [`Provider::chat`](#trait-provider).
Prefer the role constructors over hand-filling the struct.

Related: [[swarm-manager#struct-toolcall]], [[swarm-manager#trait-provider]]

### `struct ToolCall`

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

impl ToolCall {
    pub fn get_arguments(&self) -> serde_json::Value; // clone of `arguments`
}
```

**Purpose:** a flattened tool-call request the model emitted — the tool `name`, an
`id` to correlate the result, and JSON `arguments`.

**When to use:** read it off [`LLMResponse::tool_calls`](#struct-llmresponse) to
dispatch through [`ToolRegistry::execute`](#struct-toolregistry); the loop pairs
each with a [`Message::tool_result`](#struct-message).

Related: [[swarm-manager#struct-llmresponse]], [[swarm-manager#struct-toolregistry]]

### `struct LLMResponse`

```rust
#[derive(Debug, Clone)]
pub struct LLMResponse {
    pub content: Option<String>,
    pub reasoning_content: Option<String>,
    pub tool_calls: Vec<ToolCall>,
    pub finish_reason: String,
    pub usage: Usage,
}
```

**Purpose:** the result of one [`Provider::chat`](#trait-provider) turn. A non-empty
`tool_calls` means the model wants tools run before continuing; an empty
`tool_calls` with `content` is a final answer.

**When to use:** returned by every `Provider` impl; consumed by the
[`Agent`](#struct-agent) loop to decide tool-dispatch vs. finalize.

Related: [[swarm-manager#trait-provider]], [[swarm-manager#struct-agent]], [[swarm-manager#struct-usage]]

### `struct Usage`

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}
```

**Purpose:** token accounting for a completion. The [`Agent`](#struct-agent) loop
sums these (saturating) across every model turn into [`AgentTurn::usage`](#struct-agentturn).

**When to use:** read after a turn to report cost/throughput.

Related: [[swarm-manager#struct-agentturn]]

### `enum ProviderError`

```rust
#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
pub enum ProviderError {
    NotConfigured(String),
    NotImplemented(String),
    InvalidApiKey,
    ModelNotFound { model: String },
    RateLimited { retry_after_seconds: Option<u64> },
    Network(String),
    Upstream { status: u16, message: String },
    ParseResponse(String),
}
```

**Purpose:** typed provider failures. `NotConfigured` (missing key / `None` type),
`NotImplemented` (feature-off factory), and the HTTP-derived variants
(`InvalidApiKey` for 401/403, `ModelNotFound` for a model-shaped 404,
`RateLimited` for 429, `Upstream` for everything else, plus `Network` /
`ParseResponse`). Error text never contains credentials.

**When to use:** match on it to drive retry/fallback (e.g. `RateLimited` →
backoff) or to surface a clear cause.

Related: [[swarm-manager#fn-create_provider]], [[swarm-manager#fn-classify_http_error]], [[swarm-manager#enum-agenterror]]

### `fn classify_http_error`

```rust
pub fn classify_http_error(
    status: u16,
    headers: &[(String, String)],
    body: &str,
    model: &str,
) -> ProviderError;
```

**Purpose:** pure, HTTP-client-free classification of an error response into a
[`ProviderError`](#enum-providererror). Header-name matching is case-insensitive
(`Retry-After` for `RateLimited`); a 404 maps to `ModelNotFound` only when the
body/message looks model-shaped (`model` + `not found` / `does not exist`),
otherwise `Upstream`. The error message is extracted from `error.message` /
`message` in a JSON body, falling back to the trimmed body.

**When to use:** inside a `Provider` impl (or any caller holding a raw status +
body) to turn a failed response into a typed error.

Related: [[swarm-manager#enum-providererror]]

### Concrete providers

```rust
// All gated behind the `http` feature.
impl OpenAIProvider {
    pub fn new(api_key: String, base_url: String, model: String,
               temperature: f32, max_tokens: Option<usize>) -> Self;
    pub fn deepseek(api_key: String, base_url: String, model: String,
                    temperature: f32, max_tokens: Option<usize>) -> Self;
}
impl AnthropicProvider {
    pub fn new(api_key: String, base_url: String, model: String,
               temperature: f32, max_tokens: Option<usize>) -> Self;
}
impl GeminiProvider     { pub fn new(api_key: String, model: String) -> Self; }      // pub base_url
impl OllamaProvider     { pub fn new(base_url: String, model: String) -> Self; }     // pub base_url
impl LmStudioProvider   { pub fn new(base_url: String, model: Option<String>) -> Self; } // pub base_url
```

**Purpose:** the per-vendor [`Provider`](#trait-provider) implementations.
`OpenAIProvider` also serves OpenRouter (`new`) and DeepSeek (`deepseek`, which
enables the reasoning-content fields). `LmStudioProvider` backs both LM Studio and
MLX (OpenAI-compatible local servers). Each exposes a `pub base_url` (and the
cloud ones `pub api_key` / `pub model` / `pub temperature` / `pub max_tokens` /
`pub client`) for inspection or override.

**When to use:** rarely directly — [`create_provider`](#fn-create_provider) builds
the right one from a [`ProviderConfig`](#struct-providerconfig). Construct one by
hand only when bypassing the registry.

Related: [[swarm-manager#fn-create_provider]], [[swarm-manager#enum-providertype]]

### `struct ProviderConfig`

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub id: String,
    pub name: String,
    pub provider_type: ProviderType,
    pub endpoint: Option<String>,
    pub api_key: Option<String>,      // in-memory only; never serialized to disk here
    pub models: Vec<String>,
    pub is_local: bool,
    pub enabled: bool,
    pub created_at: i64,
    pub has_encrypted_key: bool,      // populated by `list()`; not on the wire
}

impl ProviderConfig {
    pub fn new(name: String, provider_type: ProviderType,
               endpoint: Option<String>, api_key: Option<String>) -> Self;
    pub fn effective_endpoint(&self) -> String;   // override else provider default
    pub fn key_status(&self) -> KeyStatus;
}
```

**Purpose:** a single user-configured provider instance — the registry row.
`new` generates a v4-UUID `id`, derives `is_local` from the type, and stamps
`created_at`. The `api_key` lives only in memory (decrypted/resolved by the
registry on read); it is never serialized to disk by this struct.
`has_encrypted_key` is set by [`ProviderRegistry::list`](#struct-providerregistry)
and feeds `key_status`.

**When to use:** the shape you `add`/`update` in [`ProviderRegistry`](#struct-providerregistry)
and feed to [`create_provider`](#fn-create_provider).

Related: [[swarm-manager#struct-providerregistry]], [[swarm-manager#enum-keystatus]], [[swarm-manager#fn-create_provider]]

### `struct ProviderRegistry`

```rust
pub struct ProviderRegistry { /* db_path, lock, vault */ }

impl ProviderRegistry {
    pub fn open(data_dir: &PathBuf) -> anyhow::Result<Self>;
    pub fn open_with_vault(data_dir: &PathBuf, vault: KeychainVault) -> anyhow::Result<Self>;
    pub fn list(&self) -> anyhow::Result<Vec<ProviderConfig>>;
    pub fn get(&self, id: &str) -> anyhow::Result<Option<ProviderConfig>>;
    pub fn add(&self, config: ProviderConfig) -> anyhow::Result<String>;
    pub fn upsert(&self, config: ProviderConfig) -> anyhow::Result<String>;
    pub fn update(&self, config: ProviderConfig) -> anyhow::Result<()>;
    pub fn delete(&self, id: &str) -> anyhow::Result<()>;
    pub fn set_models(&self, id: &str, models: Vec<String>) -> anyhow::Result<()>;
}
```

**Purpose:** persistent CRUD over provider configs in a `providers.db` SQLite file
under `data_dir`. `open` uses a real [`KeychainVault`](#struct-keychainvault);
`open_with_vault` injects one (tests / embedders). On write, the API key is
encrypted via the vault before storage — and if **no** keychain is available the
key is **not** persisted (a loud stderr warning points at the
`SWARM_PROVIDER_KEY_<ID>` fallback; plaintext is never written). On read,
`list`/`get` decrypt stored ciphertext, falling back to the env var only when no
ciphertext is present (a present-but-undecryptable key is *stranded*, never
silently overridden by env). All mutations are serialized by an internal mutex.

**When to use:** the credential store backing the CLI `provider` commands and
native dispatch. List then [`create_provider`](#fn-create_provider) the chosen row.

Related: [[swarm-manager#struct-providerconfig]], [[swarm-manager#struct-keychainvault]], [[swarm-manager#enum-keystatus]], [[swarm-cli#provider-skills-commands]]

### `enum KeyStatus`

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum KeyStatus { Absent, Healthy, Stranded }
```

**Purpose:** the three-way health of a row's API key after the registry resolves
it. `Absent` (no ciphertext and no env var), `Healthy` (a usable key — decrypted
ciphertext OR env fallback), `Stranded` (ciphertext exists on disk but could not
be decrypted, e.g. the master key rotated). `Stranded` is a loud error path, never
a silent fallback.

**When to use:** to report credential health (e.g. in `doctor` / `provider list`)
without exposing the key itself.

Related: [[swarm-manager#struct-providerconfig]], [[swarm-manager#struct-providerregistry]]

### `struct KeychainVault`

```rust
pub struct KeychainVault { /* master_key: Option<[u8; 32]> */ }

impl KeychainVault {
    pub fn new() -> Self;                       // load/create master key in OS keychain
    pub fn without_keychain() -> Self;          // no master key (env-var fallback path)
    pub fn with_key(key: [u8; 32]) -> Self;     // injected key (tests / embedders)
    pub fn is_encrypted(&self) -> bool;         // master key present?
    pub fn encrypt(&self, plaintext: &str) -> Result<String, CryptoError>;
    pub fn decrypt(&self, data: &str) -> Result<String, CryptoError>;
    pub fn is_value_encrypted(data: &str) -> bool;   // associated fn
}
```

**Purpose:** at-rest encryption for provider keys. ChaCha20-Poly1305 with a key
derived (HKDF-SHA256) from a 32-byte master held in the OS keychain (macOS
Keychain / Windows Credential Manager / Linux keyutils). Output is hex of
`[0x01 version tag][12-byte nonce][ciphertext]` — safe for a SQLite TEXT column,
and each `encrypt` uses a fresh random nonce (so the same plaintext yields
different ciphertext). When no master key is present, `encrypt` errors with
`CryptoError::KeychainUnavailable` rather than ever returning plaintext for
storage. `is_value_encrypted` checks the version tag after hex-decode.

**When to use:** indirectly via [`ProviderRegistry`](#struct-providerregistry). Use
`with_key` to inject a deterministic key in tests, or `without_keychain` to force
the env-var fallback path.

Related: [[swarm-manager#struct-providerregistry]], [[swarm-manager#fn-env_key_for]]

### `fn env_key_for`

```rust
pub const ENV_KEY_PREFIX: &str = "SWARM_PROVIDER_KEY_";
pub fn env_key_for(id: &str) -> String;
```

**Purpose:** build the environment-variable name that supplies a provider's key
when no keychain is available. Every non-alphanumeric character of `id` becomes
`_` and the result is uppercased and prefixed, e.g. UUID
`3f2504e0-...` → `SWARM_PROVIDER_KEY_3F2504E0_...`.

**When to use:** to tell a user which env var to set, or to set it yourself before
reading a key from a vault-less registry.

Related: [[swarm-manager#struct-keychainvault]], [[swarm-manager#struct-providerregistry]]

### `struct AgentConfig`

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    pub provider: ProviderType,
    pub model: String,
    pub endpoint: Option<String>,
    pub system_prompt: String,
    pub max_tool_iterations: usize,     // default 20
    pub temperature: f32,               // default 0.7
    pub max_tokens: Option<usize>,
    pub api_key: Option<String>,        // in-memory only (#[serde(skip)])
}

impl AgentConfig {
    pub fn effective_endpoint(&self) -> Option<String>;  // override else provider default
    pub fn is_configured(&self) -> bool;                 // provider != None
    pub fn apply_update(&mut self, update: ConfigUpdate);
}
```

**Purpose:** the runtime shape the [`Agent`](#struct-agent) loop consumes — what
provider/model/endpoint to drive, the system prompt, sampling params, and the
tool-iteration cap. The API key is held only in memory (`#[serde(skip)]`), never
persisted. `Default` is Ollama + `llama3.2`. Usually produced from a
[`Preset`](#struct-preset) via [`Preset::to_config`](#struct-preset).

**When to use:** to construct an [`Agent`](#struct-agent).

Related: [[swarm-manager#struct-agent]], [[swarm-manager#struct-preset]], [[swarm-manager#struct-configupdate]]

### `struct ConfigUpdate`

```rust
#[derive(Debug, Default, Deserialize)]
pub struct ConfigUpdate {
    pub provider: Option<ProviderType>,
    pub model: Option<String>,
    pub endpoint: Option<String>,
    pub system_prompt: Option<String>,
    pub max_tool_iterations: Option<usize>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<usize>,
    pub api_key: Option<String>,
}
```

**Purpose:** a partial patch for an [`AgentConfig`](#struct-agentconfig) — present
fields overwrite, absent fields are left unchanged. Applied via
`AgentConfig::apply_update`. (Both `endpoint` and `max_tokens`, being `Option`
fields, are overwritten whenever the update carries `Some`.)

**When to use:** to apply an incremental config change (e.g. from a deserialized
patch request).

Related: [[swarm-manager#struct-agentconfig]]

### `struct Preset`

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Preset {
    pub id: String,
    pub name: String,
    pub owner: AgentOwner,
    pub provider_config_id: Option<String>,   // ref into the provider registry
    pub provider: ProviderType,
    pub model: String,
    pub endpoint: Option<String>,
    pub system_prompt: String,
    pub enabled_skills: Vec<String>,          // persisted; not yet honoured at dispatch
    pub permission_overrides: Option<serde_json::Value>,
    pub temperature: f32,                     // default 0.7
    pub max_tokens: Option<usize>,
    pub max_tool_iterations: usize,           // default 20
    pub is_default: bool,
    pub consumer_policy: ConsumerPolicy,
}

impl Preset {
    pub fn new(name: String, provider: ProviderType, model: String,
               endpoint: Option<String>, system_prompt: String,
               owner: Option<AgentOwner>, temperature: f32,
               max_tokens: Option<usize>, provider_config_id: Option<String>) -> Self;
    pub fn to_config(&self) -> AgentConfig;   // api_key always None
}
```

**Purpose:** a saved, named agent configuration. When `provider_config_id` is set,
the runtime resolves the provider from the registry and the inline
provider/model/endpoint fields act as a fallback. `new` mints a UUID id and
applies safe defaults. `to_config` projects it into the runtime
[`AgentConfig`](#struct-agentconfig); the API key is never carried from a preset.

> Note: `enabled_skills` is persisted for forward compatibility but is **not**
> honoured at dispatch time — live skill injection rides the native backend
> descriptor's `skills` field through [`SkillSet`](#struct-skillset).
> `permission_overrides` is opaque and interpreted by a later permission layer.

**When to use:** the unit you store in a [`PresetStore`](#struct-presetstore) and
turn into a runtime config.

Related: [[swarm-manager#struct-presetstore]], [[swarm-manager#struct-agentconfig]], [[swarm-manager#enum-agentowner]], [[swarm-manager#enum-consumerpolicy]]

### `struct PresetStore`

```rust
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct PresetStore {
    pub presets: Vec<Preset>,
    pub active_preset_id: Option<String>,
}

impl PresetStore {
    pub fn load(data_dir: &Path) -> Self;                       // presets.json; empty on miss
    pub fn save(&self, data_dir: &Path) -> Result<(), std::io::Error>;
    pub fn add(&mut self, request: PresetRequest) -> Preset;
    pub fn update(&mut self, id: &str, request: PresetRequest) -> Option<Preset>;
    pub fn delete(&mut self, id: &str) -> bool;                 // clears active if matched
    pub fn delete_safe(&mut self, id: &str) -> Result<bool, String>; // refuses the default
    pub fn get(&self, id: &str) -> Option<&Preset>;
    pub fn set_active(&mut self, id: Option<String>);
    pub fn get_active(&self) -> Option<&Preset>;
    pub fn list(&self) -> &[Preset];
    pub fn default_id(&self) -> Option<String>;
    pub fn ensure_single_default(&mut self);                    // exactly one default
}
```

**Purpose:** a JSON-backed (`presets.json`) collection of presets with an
active-preset pointer. `load` returns an empty store when the file is absent or
unreadable (never panics); `save` creates the directory and writes pretty JSON.
`add`/`update` build from a [`PresetRequest`](#struct-presetrequest);
`delete_safe` rejects deleting the default; `ensure_single_default` keeps exactly
one preset flagged default (first wins; head marked if none).

**When to use:** to persist and manage the user's named agent configs.

Related: [[swarm-manager#struct-preset]], [[swarm-manager#struct-presetrequest]]

### `struct PresetRequest`

```rust
#[derive(Debug, Default, Deserialize)]
pub struct PresetRequest {
    pub name: String,
    pub provider: ProviderType,
    pub model: String,
    pub endpoint: Option<String>,
    pub system_prompt: String,
    pub enabled_skills: Option<Vec<String>>,
    pub temperature: f32,                       // default 0.7
    pub max_tokens: Option<usize>,
    pub owner: Option<AgentOwner>,
    pub permission_overrides: Option<serde_json::Value>,
    pub provider_config_id: Option<String>,
    pub consumer_policy: Option<ConsumerPolicy>,   // default Allowlist
    pub max_tool_iterations: Option<usize>,        // default 20
}
```

**Purpose:** the create/update payload for a preset. Deserializable from a config
or request body; defaults fill the optional knobs. Consumed by
[`PresetStore::add`](#struct-presetstore) / `update`.

**When to use:** to create or edit a [`Preset`](#struct-preset) without hand-building
the full struct (id/default flags are managed by the store).

Related: [[swarm-manager#struct-presetstore]], [[swarm-manager#struct-preset]]

### `enum ConsumerPolicy`

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ConsumerPolicy { Public, #[default] Allowlist, Internal }
```

**Purpose:** a visibility marker for which callers may dispatch a preset. In v1
these are plain markers describing intent — **not** enforced by the manager core;
a later arc can attach enforcement.

**When to use:** to record dispatch intent on a [`Preset`](#struct-preset).

Related: [[swarm-manager#struct-preset]]

### `enum AgentOwner`

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AgentOwner { User, #[default] Node }
```

**Purpose:** a marker for where an agent conceptually lives — `User` (personal,
portable across devices) vs. `Node` (tied to this workstation). Plain marker in
v1; no sync behaviour attached.

**When to use:** to tag a [`Preset`](#struct-preset)'s ownership.

Related: [[swarm-manager#struct-preset]]

### `trait Tool`

```rust
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters(&self) -> serde_json::Value;             // JSON Schema
    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<String>;
    async fn execute_approved(&self, args: serde_json::Value) -> anyhow::Result<String>; // defaults to execute
    fn to_openai_tool(&self) -> serde_json::Value;          // {type:function, function:{...}}
}
```

**Purpose:** the contract every tool implements — identity, a JSON-Schema
parameter spec, and an async executor returning a textual result. `to_openai_tool`
(provided) renders the OpenAI tool-calling shape sent to the model.
`execute_approved` (provided) is the post-permission-broker hook; the v1 broker is
permissive so it defaults to plain `execute`.

**When to use:** implement it to add a tool; register the impl in a
[`ToolRegistry`](#struct-toolregistry).

Related: [[swarm-manager#struct-toolregistry]], [[swarm-manager#struct-exectool]], [[swarm-manager#file-tools]]

### `struct ToolRegistry`

```rust
pub struct ToolRegistry { /* BTreeMap<String, Arc<dyn Tool>> */ }

impl ToolRegistry {
    pub fn new() -> Self;                                      // also Default
    pub fn register(&mut self, tool: Arc<dyn Tool>);          // same name replaces
    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>>;
    pub fn list(&self) -> Vec<&str>;                          // name-sorted
    pub fn retain(&mut self, keep: impl Fn(&str) -> bool);    // gate against allowed-tools
    pub fn to_openai_tools(&self) -> Vec<serde_json::Value>;  // name-sorted
    pub async fn execute(&self, name: &str, args: serde_json::Value) -> anyhow::Result<String>;
}
```

**Purpose:** a name-keyed collection of tools. Backed by a `BTreeMap` so `list`
and `to_openai_tools` are **deterministically name-ordered** — the tools array is
part of the model prompt, and a stable order keeps prompts reproducible and avoids
perturbing weak-model tool choice. `retain` drops tools whose name fails the
predicate, the mechanism for gating against a skill's `allowed-tools`. `execute`
dispatches by name (v1 dispatch is direct; the method shape leaves room for a real
permission broker later).

**When to use:** build it, `register` the built-ins, optionally `retain` to a
[`SkillSet::allowed_tools`](#struct-skillset) gate, then hand it to
[`Agent::new`](#struct-agent).

Related: [[swarm-manager#trait-tool]], [[swarm-manager#struct-skillset]], [[swarm-manager#struct-agent]]

### `struct ExecTool`

```rust
// `runtime` feature. Tool name: "exec".
pub struct ExecTool { /* timeout, workspace_root, max_output_chars */ }

impl ExecTool {
    pub fn new(timeout_secs: u64) -> Self;                       // root = current dir
    pub fn workspace(workspace_root: impl Into<PathBuf>) -> Self; // 30s default timeout
    pub fn with_output_limit(mut self, max_output_chars: usize) -> Self;
}
```

**Purpose:** the `exec` tool — run a shell command (`sh -c`) inside a workspace
root. The command runs **unwrapped** (no sandbox) but the `workdir` argument is
confined to the workspace root (escapes are rejected), the run is wall-clock
timed, and output is secret-scrubbed and head/tail truncated before being returned
to the model. Args: `command` (required), optional `workdir` and `timeout`.

**When to use:** register it when the agent needs to run commands. Prefer
`workspace(root)` to pin the execution boundary; `with_output_limit` to cap output
size.

Related: [[swarm-manager#trait-tool]], [[swarm-manager#struct-toolregistry]]

### File tools

```rust
// `runtime` feature. Tool names: "read_file", "write_file", "edit_file", "list_dir".
pub struct ReadFileTool  { /* boundary */ }
pub struct WriteFileTool { /* boundary */ }
pub struct EditFileTool  { /* boundary */ }
pub struct ListDirTool   { /* boundary */ }

// Each exposes the same constructors (and `Default` = `direct()`):
impl ReadFileTool  { pub fn direct() -> Self; pub fn workspace(root: impl Into<PathBuf>) -> Self; }
// ... identical pair on WriteFileTool / EditFileTool / ListDirTool.
```

**Purpose:** the workspace-confined filesystem tools.

- `read_file` — read a file (optional 1-indexed `offset` + `limit` lines).
- `write_file` — write a file, creating parent dirs.
- `edit_file` — replace one exact, unique `old_text` occurrence with `new_text`
  (errors if zero or multiple matches).
- `list_dir` — list a directory (`d`/`f` prefixed entries).

All resolve paths against a configurable root, rejecting `..` escapes. `direct()`
pins the root to the current working directory; `workspace(root)` pins it
explicitly. There is no sandbox — paths resolve on the real filesystem, confined
to the root.

**When to use:** register the subset the agent needs for file work; pass the
session workspace as the root via `workspace`.

Related: [[swarm-manager#trait-tool]], [[swarm-manager#struct-toolregistry]]

### Web tools

```rust
// `http` feature. Tool names: "web_search", "web_fetch".
pub struct WebSearchTool { /* api_key, endpoint, client */ }
pub struct WebFetchTool  { /* client */ }

impl WebSearchTool {
    pub fn new(api_key: String) -> Self;                              // Brave search endpoint
    pub fn with_endpoint(api_key: String, endpoint: String) -> Self; // custom endpoint (tests)
}
impl WebFetchTool { pub fn new() -> Self; }                          // also Default
```

**Purpose:**

- `web_search` — query the Brave Search API and return titled URL + snippet
  results (args: `query` required, `count` 1–10 default 5). Built with the Brave
  API key.
- `web_fetch` — fetch a URL and strip HTML to readable text, with SSRF guarding
  (private/loopback addresses blocked) and output truncation.

**When to use:** register them (under `http`) when the agent needs live web access.

Related: [[swarm-manager#trait-tool]], [[swarm-manager#struct-toolregistry]]

### `struct Agent`

```rust
// `runtime` feature.
pub struct Agent { /* provider, tools, config, messages */ }

impl Agent {
    pub fn new(provider: Arc<dyn Provider>, tools: ToolRegistry, config: AgentConfig) -> Self;
    pub fn messages(&self) -> &[Message];
    pub async fn process(&mut self, input: &str) -> Result<AgentTurn, AgentError>;
    pub fn run_blocking(&mut self, input: &str) -> Result<AgentTurn, AgentError>;
}
```

**Purpose:** the single-agent tool loop. `new` seeds the transcript with the
config's system prompt. `process` appends a user message and drives
chat → tool-dispatch → chat until the model returns a tool-call-free message,
accumulating [`Usage`](#struct-usage) across every turn and recording each
[`ToolInvocation`](#struct-toolinvocation). A tool failure is folded into an
`Error: …` string fed back to the model (the loop continues), not a hard abort.
Hitting `max_tool_iterations` is a loud `AgentError::MaxIterationsExceeded` —
never a silent stop. `run_blocking` builds a private current-thread tokio runtime
and `block_on`s `process` for synchronous callers (must not run inside an existing
runtime). `messages` exposes the current transcript.

**When to use:** to actually run an agent turn in-process. This is what
`swarm-exec`'s native backend wraps.

Related: [[swarm-manager#trait-provider]], [[swarm-manager#struct-toolregistry]], [[swarm-manager#struct-agentconfig]], [[swarm-manager#struct-agentturn]], [[swarm-exec#native-backend]]

### `struct AgentTurn`

```rust
#[derive(Debug, Clone)]
pub struct AgentTurn {
    pub text: String,                       // "(no response)" if empty + no tools
    pub tool_calls: Vec<ToolInvocation>,    // in execution order
    pub usage: Usage,                       // summed across all model turns
}
```

**Purpose:** the result of running the [`Agent`](#struct-agent) loop to a final
message — the final assistant text, every tool call executed, and aggregate token
usage.

**When to use:** the return value of [`Agent::process`](#struct-agent) /
`run_blocking`.

Related: [[swarm-manager#struct-agent]], [[swarm-manager#struct-toolinvocation]], [[swarm-manager#struct-usage]]

### `struct ToolInvocation`

```rust
#[derive(Debug, Clone)]
pub struct ToolInvocation {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
    pub result: String,   // on tool error, the "Error: …" string fed back
}
```

**Purpose:** a record of one executed tool call within an [`AgentTurn`](#struct-agentturn)
— the model's call id/name/arguments and the textual result returned to it (an
`Error: …` string on failure, matching the feed-back-and-continue behaviour).

**When to use:** inspect [`AgentTurn::tool_calls`](#struct-agentturn) to see what
the agent did.

Related: [[swarm-manager#struct-agentturn]], [[swarm-manager#struct-toolcall]]

### `enum AgentError`

```rust
#[derive(thiserror::Error, Debug)]
pub enum AgentError {
    Provider(#[from] ProviderError),
    Tool(String),
    MaxIterationsExceeded(usize),
}
```

**Purpose:** typed failures from the loop. `Provider` wraps an underlying
[`ProviderError`](#enum-providererror); `MaxIterationsExceeded(cap)` fires when the
model never finalizes within `max_tool_iterations`. `Tool` is reserved for callers
that opt into surfacing tool failures as hard errors (the loop itself feeds tool
errors back as result strings).

**When to use:** match on the result of [`Agent::process`](#struct-agent) /
`run_blocking`.

Related: [[swarm-manager#struct-agent]], [[swarm-manager#enum-providererror]]

### `fn parse_skill`

```rust
pub fn parse_skill(text: &str, source: impl Into<PathBuf>) -> Result<Skill, SkillError>;
```

**Purpose:** parse one `SKILL.md`'s text into a [`Skill`](#struct-skill). The file
must open with a `---` fence, carry `key: value` frontmatter lines, and close with
a second `---`; everything after the closing fence is the body. Only `name`
(required), `description`, and `allowed-tools` are honoured; unknown keys are
ignored. `allowed-tools` accepts `a, b, c` or `[a, b, c]`. Handles `\n` and
`\r\n`. Errors are typed [`SkillError`](#enum-skillerror)s — always loud.

**When to use:** to parse skill text directly (e.g. validating a single file).
Usually called via [`load_skills`](#fn-load_skills).

Related: [[swarm-manager#struct-skill]], [[swarm-manager#enum-skillerror]], [[swarm-manager#fn-load_skills]]

### `fn load_skills`

```rust
pub fn load_skills(dirs: &[PathBuf]) -> (Vec<Skill>, Vec<SkillLoadIssue>);
// helper:
pub fn skill_file_path(dir: &Path, name: &str) -> PathBuf;  // <dir>/<name>/SKILL.md
```

**Purpose:** scan each directory for `<dir>/<name>/SKILL.md` and load every skill.
Later directories override earlier ones by skill name (so `[home, project]` lets
the project win). Malformed files become [`SkillLoadIssue`](#struct-skillloadissue)s
rather than aborting the scan; missing/unreadable dirs are skipped silently;
loaded skills come back name-sorted. `skill_file_path` resolves the canonical file
path for a name (helper for CLI reporting/location).

**When to use:** to discover the available skills before resolving a
[`SkillSet`](#struct-skillset). Backs the `skills list` CLI command.

Related: [[swarm-manager#struct-skill]], [[swarm-manager#struct-skillset]], [[swarm-manager#struct-skillloadissue]], [[swarm-cli#provider-skills-commands]]

### `struct Skill`

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub allowed_tools: Option<Vec<String>>,   // None = no restriction
    pub body: String,                          // prompt fragment after the fence
    pub source: PathBuf,
}
```

**Purpose:** one loaded skill — frontmatter metadata plus the markdown body
injected into the system prompt. `allowed_tools: None` means "do not restrict the
tool set"; `Some(set)` gates to exactly those names.

**When to use:** the unit produced by [`parse_skill`](#fn-parse_skill) /
[`load_skills`](#fn-load_skills) and selected into a [`SkillSet`](#struct-skillset).

Related: [[swarm-manager#fn-parse_skill]], [[swarm-manager#struct-skillset]]

### `struct SkillSet`

```rust
#[derive(Debug, Clone, Default)]
pub struct SkillSet { /* selected, unknown */ }

impl SkillSet {
    pub fn resolve(loaded: &[Skill], requested: &[String]) -> Self;
    pub fn selected(&self) -> &[Skill];
    pub fn unknown(&self) -> &[SkillSelectionIssue];
    pub fn is_empty(&self) -> bool;
    pub fn compose_system_prompt(&self, base: &str) -> String;
    pub fn allowed_tools(&self) -> Option<HashSet<String>>;
}
```

**Purpose:** a resolved selection of skills, ready to compose a prompt and report
the gated tool set. `resolve` matches `requested` names against `loaded` skills in
request order (de-duplicated, first-occurrence-wins); unmatched names become
recoverable [`SkillSelectionIssue`](#struct-skillselectionissue)s.
`compose_system_prompt` appends each selected body under a `## Skill: <name>`
header in stable order (base returned unchanged when nothing is selected).
`allowed_tools` returns the **union** of every restricting skill's tools, or
`None` when no selected skill restricts — a non-restricting skill never widens the
gate back to "all".

**When to use:** after [`load_skills`](#fn-load_skills): resolve the requested
names, compose the system prompt for [`AgentConfig`](#struct-agentconfig), and
`retain` the [`ToolRegistry`](#struct-toolregistry) against `allowed_tools`.

Related: [[swarm-manager#fn-load_skills]], [[swarm-manager#struct-toolregistry]], [[swarm-manager#struct-agentconfig]], [[swarm-manager#struct-skillselectionissue]]

### `enum SkillError`

```rust
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SkillError { MissingOpeningFence, MissingClosingFence, MissingName }
```

**Purpose:** a typed frontmatter/structure parse failure from
[`parse_skill`](#fn-parse_skill) — no opening `---`, no closing `---`, or a missing
/ empty required `name`. Always loud, never a silent fallback.

**When to use:** match on a `parse_skill` error, or read it off a
[`SkillLoadIssue`](#struct-skillloadissue).

Related: [[swarm-manager#fn-parse_skill]], [[swarm-manager#struct-skillloadissue]]

### `struct SkillLoadIssue`

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillLoadIssue { pub source: PathBuf, pub error: SkillError }
```

**Purpose:** a `SKILL.md` that could not be loaded, paired with the reason.
Collected by [`load_skills`](#fn-load_skills) rather than thrown, so one bad skill
never blocks the rest.

**When to use:** to surface skill-loading problems to the user (e.g. the `doctor`
/ `skills list` warnings).

Related: [[swarm-manager#fn-load_skills]], [[swarm-manager#enum-skillerror]]

### `struct SkillSelectionIssue`

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillSelectionIssue { pub requested: String }
```

**Purpose:** a requested skill name that did not resolve to any loaded skill —
collected by [`SkillSet::resolve`](#struct-skillset), never silently dropped.

**When to use:** report unknown requested skill names back to the caller.

Related: [[swarm-manager#struct-skillset]]
