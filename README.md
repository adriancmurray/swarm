# swarm

**Orchestrate multi-agent swarms in pure Rust.**

`swarm` routes tasks to specialized coding agents and composes them into
higher-order workflows: parallel fan-out, structured discussion, and manager
synthesis. It is dependency-light, local-first, and built around descriptor
configuration instead of hard-coded backend choices.

[Website](https://swarm.dech.app) ·
[Pages Preview](https://swarm-v57.pages.dev) ·
[Authoring a Backend](docs/authoring-a-backend.md)

```sh
cargo run -p swarm-cli -- fanout "Review the auth module"
cargo run -p swarm-cli -- discuss "Review the session model"
cargo run -p swarm-cli -- metadirector "Plan the next verified slice"
```

## Designed for Complex Agent Coordination

Single-agent tasks have limits. `swarm` coordinates diverse coding models
through explicit roles, session records, and manager synthesis.

| Pattern | What it does |
| --- | --- |
| **fan-out** | Sends the same task to independent workers, then gives their outputs to a manager for synthesis. Useful for architecture, implementation, and review perspectives. |
| **discuss** | Runs one or more rounds of role-based reasoning. The session produces inspectable transcripts and digest artifacts. |
| **manager synthesis** | Uses a manager agent to compare worker outputs, separate accepted facts from risky claims, and return a compact decision packet. |

## Engine Features

- **Descriptor-first backends**: wire agents in TOML config, not source code.
- **Native in-process harness**: `swarm-manager` provides an embedded agent loop,
  provider registry, credential vault, presets, and built-in tools.
- **MCP server layer**: expose reports, manifests, sessions, events,
  transcripts, and dispatch surfaces through the MCP crate.
- **Dependency-light defaults**: no async runtime, HTTP client, or TLS unless
  you opt into the feature flags that need them.
- **Inspectable sessions**: review prior work through `sessions`, `events`,
  `transcript`, and `overview`.

## Descriptor-First Backends

Backends are ordinary descriptors. A CLI agent, hosted API, or native
in-process provider can all be selected by the same routing rules:

```toml
[backend.codex]
kind        = "cli"
command     = "codex"
args        = ["exec", "--model", "{model}"]
prompt      = "stdin"
stream      = "stdout-lines"
ready_check = { binary = "codex" }

[routes.implementation]
preferred = ["codex", "claude:sonnet"]
```

There are three backend kinds:

| Kind | What it wraps |
| --- | --- |
| `cli` | Any command-line agent, run as a subprocess. |
| `openai-compatible` | Any HTTP endpoint that speaks `/v1/chat/completions`. |
| `native` | The in-process `swarm-manager` harness. |

Most integrations should be descriptors only. Drop into Rust only when you need
a custom handshake, bespoke streaming protocol, non-standard auth, or
multi-step orchestration that a descriptor cannot express.

## Modular Crate Architecture

```text
crates/
  swarm-contracts   wire-stable contract types: ids, events, jobs, telemetry
  swarm-core        repository traits and pure domain substrate
  swarm-store       filesystem-backed jobs, sessions, telemetry, and ledgers
  swarm-kernel      stateless routing, config, backend ABI, and classification
  swarm-exec        executor, orchestration, synthesis, sessions, and monitors
  swarm-mcp         MCP server layer, schemas, manifests, reports, dispatch
  swarm-cli         command parsing and CLI command dispatch
  swarm-manager     native single-agent harness, providers, vault, tools
  swarm-registrar   optional generic JSON service-registry hook
```

The workspace keeps contracts, storage, routing, execution, transport, and
native agent concerns separate so the engine stays easy to embed, test, and
extend.

## Quickstart

Compile the workspace:

```sh
cargo build --release
cargo test
```

Create a config:

```sh
mkdir -p ~/.swarm
cp examples/config.example.toml ~/.swarm/config.toml
```

Define backend descriptors in `~/.swarm/config.toml`:

```toml
[backend.claude]
kind        = "cli"
command     = "claude"
args        = ["--print", "--model", "{model}"]
prompt      = "stdin"
ready_check = { binary = "claude" }

[settings]
default_agent = "claude"
```

Run a structured discussion:

```sh
cargo run -p swarm-cli -- discuss \
  --participant architecture=claude:sonnet \
  --participant review=codex \
  "Analyze auth.rs for timing vulnerabilities"
```

With no config, the engine uses built-in defaults and resolves public `claude`
and `codex` CLIs when they are installed.

On a fresh install, `swarm doctor` checks the whole setup in one pass: config
parse, every backend's readiness, routing strings that point nowhere, and
provider credential status. It exits non-zero when something would block a run.

### Providers & credentials

Stored provider configurations (for `native` backends) live in an encrypted
registry under `~/.swarm/providers`. API keys are encrypted at rest with a
master key held in the OS keyring; when no keyring is available, keys are
never written to disk and are read at runtime from the
`SWARM_PROVIDER_KEY_<ID>` environment variable instead.

```sh
swarm provider add api --type openai --models gpt-5.5
pbpaste | swarm provider key set api    # key is read from stdin — never argv
swarm provider list                      # id, type, endpoint, models, key status
swarm provider models openai             # suggested model ids (verify against provider docs)
swarm provider key check api
```

`key set` never accepts the key as a command-line argument (argv leaks into
process listings and shell history) and never echoes it back — the only
acknowledgment is a masked length. `--from-env VAR` copies a key from an
environment variable into the vault.

## Core Commands

| Command | Use it for |
| --- | --- |
| `run` | Run a single routed task. |
| `fanout` / `swarm` | Send a task to parallel workers and synthesize the results. |
| `discuss` | Run a structured multi-participant discussion. |
| `metadirector` | Ask a manager agent to plan or synthesize from supplied context. |
| `mcp` | Start the MCP server layer. |
| `sessions`, `events`, `transcript`, `overview` | Inspect prior work and runtime output. |
| `scaffold-backend` | Generate a descriptor and Rust trait skeleton for a new backend. |
| `provider` | Manage stored providers and their credentials (vault + env fallback). |
| `doctor` | Health-check config, backends, routing, and provider credentials. |

## Feature Flags

| Feature | Crate | What it adds |
| --- | --- | --- |
| `openai` | `swarm-exec` | OpenAI-compatible HTTP backend via `ureq` and rustls. |
| `native` | `swarm-exec` | In-process single-agent backend through `swarm-manager`. |
| `runtime` | `swarm-manager` | Async agent loop and built-in exec/file tools. |
| `http` | `swarm-manager` | HTTP providers and web tool through `reqwest` and rustls. |
| `registry` | `swarm-mcp` | Optional JSON service-registry self-registration. |
| `rmcp` | `swarm-mcp` | rmcp-based MCP transport. |

## Threat Model & Safety Boundaries

`swarm` is an orchestrator, not a sandbox.

- A `cli` backend executes the command in your config with your user's
  privileges.
- API-backed prompts and outputs leave your machine and go to the endpoint you
  configure.
- Backend CLIs keep their own permission systems, if they have them. `swarm`
  does not broker file or network access in v1.
- `swarm-manager` can encrypt credentials at rest, but subprocess backends can
  still read whatever their process permissions allow.

Treat backend config like a shell profile: review it, keep secrets out of the
repo, and only run agents you trust in directories you are willing to expose.

## Development

```sh
cargo fmt --all
cargo test
cargo test -p swarm-cli
cargo test -p swarm-manager --features http
```

The CLI command surface is guarded by tests because external wrappers depend on
exact command tokens. When changing a command name, update the stability guard
intentionally.

## License

Licensed under either of [MIT](LICENSE-MIT) or
[Apache-2.0](LICENSE-APACHE), at your option.
