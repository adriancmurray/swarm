# swarm

A multi-agent orchestrator in pure Rust. `swarm` routes tasks to coding-agent
backends and composes them into higher-order runs: **fan-out** (parallel
workers), **discuss** (multi-round structured discussion), and
**manager-synthesis** (a manager agent verifies and synthesizes worker
output). It also ships a **native single-agent harness** (`swarm-manager`) —
provider registry, encrypted credential vault, and an agent loop — for running
a model directly over HTTP without any external CLI.

## Layout

```
crates/
  swarm-contracts   # wire-stable contract types (ids, events, jobs, telemetry) — serde only
  swarm-core        # pure repo-trait substrate
  swarm-store       # filesystem-backed stores (jobs, sessions, telemetry)
  swarm-kernel      # stateless leaf modules (resolver, classifier, backend ABI)
  swarm-exec        # the engine: executor, orchestration, synthesis, sessions, monitor
  swarm-mcp         # MCP server layer (stdio loop, schema, manifest)
  swarm-cli         # CLI command dispatch
  swarm-manager     # single-agent harness: providers, credential vault, agent loop
  swarm-registrar   # optional generic JSON service-registry hook
```

## Quickstart

```sh
cargo build
cargo test
```

Copy `examples/config.example.toml` to `~/.swarm/config.toml` and edit it to
declare your backends. Backends are **descriptor-first** — a backend is a TOML
descriptor, not a code change:

- **`cli`** — wraps any local agent CLI (e.g. `claude --print`, `codex exec`):
  command, args, output parsing declared in config.
- **`openai`** — any OpenAI-compatible HTTP endpoint (build with
  `--features openai` on `swarm-exec`).
- **`native`** — the in-process `swarm-manager` agent loop (build with
  `--features native` on `swarm-exec`).

### Feature flags

| Feature | Crate | What it adds |
| --- | --- | --- |
| `openai` | `swarm-exec` | OpenAI-compatible HTTP backend (`ureq` + rustls) |
| `native` | `swarm-exec` | In-process single-agent backend via `swarm-manager` |
| `runtime` / `http` | `swarm-manager` | Async built-in tools / HTTP providers (`reqwest` + rustls) |
| `registry` | `swarm-mcp` | Optional JSON service-registry self-registration |
| `rmcp` | `swarm-mcp` | rmcp-based MCP transport |

The default build is dependency-light: no async runtime, no HTTP, no TLS.

## Threat model — read before deploying

`swarm` is **not a sandbox**:

- It **runs whatever your config says**. A `cli` backend descriptor executes
  an arbitrary local command with your user's privileges. Treat your config
  file like you treat your shell profile.
- **Prompts leave your machine** when you use API-backed backends (`openai`,
  `native` with HTTP providers). Task text, file excerpts, and worker output
  are sent to whatever endpoint the descriptor points at.
- There is **no permission broker in v1**. Backend CLIs run with their own
  native permission systems (or none); swarm does not interpose on file or
  network access.
- Credentials stored by `swarm-manager` are encrypted at rest (OS keychain
  with env-var fallback), but anything a backend process can read, it can
  exfiltrate. Vet your backends.

## Extending

To add a new backend kind (beyond what a descriptor can express), read
[`docs/authoring-a-backend.md`](docs/authoring-a-backend.md) and use the
`scaffold-backend` command to generate the starting point.

## License

Licensed under either of [MIT](LICENSE-MIT) or
[Apache-2.0](LICENSE-APACHE), at your option.
