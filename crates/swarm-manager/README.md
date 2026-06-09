# swarm-manager

An embeddable, single-agent harness in pure Rust. Give it a provider and a
prompt and it runs a tool-using agent loop to a final answer — **no external
agent CLI required**. The crate is standalone and self-contained: it bundles a
provider abstraction, a persistent provider registry, an at-rest credential
vault, agent presets, and a set of built-in tools.

It is the "run an agent in-process" building block for the swarm orchestrator,
but it has no orchestration, networking-mesh, or host coupling of its own — you
can drop it into any Rust program.

## What's in the box

- **Provider abstraction** — a small async `Provider` trait plus a
  `ProviderType` enum (OpenAI, Anthropic, Gemini, Ollama, LM Studio, MLX,
  OpenRouter, DeepSeek) and a `create_provider` factory that builds a concrete
  HTTP-backed implementation from a `ProviderConfig`.
- **Provider registry** — `ProviderRegistry`, a SQLite-backed store of
  configured provider instances. Each row carries a type, endpoint, model list,
  and an encrypted API key.
- **Credential vault** — `KeychainVault` encrypts API keys at rest with
  ChaCha20-Poly1305 using a master key held in the OS keyring, with an
  environment-variable fallback when no keyring is available (see below).
- **Presets** — `Preset` / `PresetStore`, a JSON-backed collection of named
  agent configurations (provider, model, system prompt, sampling parameters).
  `Preset::to_config()` produces the `AgentConfig` the loop consumes.
- **Tools** — a `Tool` trait, a `ToolRegistry`, and built-in `exec`, file
  (read / write / edit / list-dir), and `web` tools.
- **Agent loop** — `Agent`, which drives `chat → tool dispatch → chat` until
  the model returns a final, tool-call-free message. Hitting the iteration cap
  is a loud, typed error, never a silent stop.

## No external CLI required

The agent loop is implemented in this crate. You do not shell out to any
third-party agent binary: you construct an `Agent` from an `Arc<dyn Provider>`,
a `ToolRegistry`, and an `AgentConfig`, then call `process` (async) or
`run_blocking` (sync). The model's tool calls are dispatched against the tools
you registered, in-process.

## Cargo features

The default build is intentionally light — data types and traits only, no async
runtime, no HTTP client.

| Feature   | Enables                                                                                  | Pulls in            |
|-----------|------------------------------------------------------------------------------------------|---------------------|
| (default) | `Provider` trait, `ProviderType`, `ProviderRegistry`, `KeychainVault`, presets, the `Tool` trait + `ToolRegistry`. | —                   |
| `runtime` | The async-runtime-backed pieces: the `Agent` loop and the built-in `exec` / file tools.  | `tokio`, `regex`    |
| `http`    | Everything in `runtime`, plus the concrete HTTP provider implementations (`create_provider` builds them) and the `web` tool. | `reqwest` (rustls)  |

`http` builds on `runtime`. TLS is rustls (pure Rust) — no OpenSSL or
system-native TLS dependency.

## Credentials

API keys are never written to disk in plaintext and never logged.

1. **OS keyring (preferred).** When a keyring is available, `KeychainVault`
   stores a random master key there and encrypts each provider's API key with a
   key derived from it (HKDF-SHA256 → ChaCha20-Poly1305). The ciphertext lives
   in the registry's SQLite database; the plaintext only ever exists in memory.
2. **Environment-variable fallback.** When no keyring is available, keys are
   **not** persisted. Instead, supply each provider's key at runtime via
   `SWARM_PROVIDER_KEY_<ID>`, where `<ID>` is the provider config's id
   uppercased with every non-alphanumeric character replaced by `_`. The
   registry resolves this variable at read time.

A stored-but-undecryptable key (for example after the master key rotates) is
surfaced as a distinct `Stranded` status rather than silently falling back to
an environment variable — so a broken key fails loudly.

## Quickstart

Build a provider, register the built-in tools, and run one prompt:

```rust
use std::sync::Arc;

use swarm_manager::tools::ExecTool;
use swarm_manager::{
    create_provider, Agent, AgentConfig, ProviderConfig, ProviderType, ToolRegistry,
};

fn main() -> anyhow::Result<()> {
    let api_key = std::env::var("OPENAI_API_KEY")?;

    let mut config = ProviderConfig::new(
        "OpenAI".to_string(),
        ProviderType::OpenAI,
        None, // default endpoint
        Some(api_key),
    );
    config.models = vec!["gpt-4o-mini".to_string()];
    let provider = create_provider(&config)?;

    let mut tools = ToolRegistry::new();
    tools.register(Arc::new(ExecTool::new(30)));

    let agent_config = AgentConfig {
        system_prompt: "You are a concise assistant.".to_string(),
        ..AgentConfig::default()
    };
    let mut agent = Agent::new(provider, tools, agent_config);

    // `run_blocking` owns its own runtime, so call it from a plain `fn main`
    // (never inside `#[tokio::main]`).
    let turn = agent.run_blocking("Say hello in one sentence.")?;
    println!("{}", turn.text);
    Ok(())
}
```

Run it (requires the `http` feature and a key in the environment):

```bash
OPENAI_API_KEY=sk-... \
  cargo run --example simple_agent -p swarm-manager --features http
```

See [`examples/simple_agent.rs`](examples/simple_agent.rs) for the full version
that registers the file and web tools too.

### Persisting providers with the registry

For a longer-lived program, store provider configs in the registry instead of
constructing them inline. The registry encrypts keys at rest and resolves the
`SWARM_PROVIDER_KEY_<ID>` fallback for you:

```rust
use swarm_manager::{ProviderConfig, ProviderRegistry, ProviderType};

let registry = ProviderRegistry::open(&data_dir)?;
let config = ProviderConfig::new(
    "OpenAI".to_string(),
    ProviderType::OpenAI,
    None,
    Some(api_key), // encrypted before storage when a keyring is available
);
let id = registry.add(config)?;

// Later: keys are resolved in memory (decrypted ciphertext or env fallback).
let resolved = registry.get(&id)?;
```

## Roadmap

The following are intentionally out of scope for this release and handled
elsewhere or in later work:

- **Unioning tools from external tool servers** — v1 ships only the built-in
  tools. Aggregating tools advertised over a tool-server protocol is future
  work.
- **Permission broker** — the v1 broker is permissive (always-allow). The
  dispatch path keeps the admission hook so a real broker can be swapped in
  without changing callers.
- **Sandboxing** — file tools currently run unwrapped. A write-sandbox layer is
  a later addition.

## License

MIT.
