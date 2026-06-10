# Authoring a backend

A *backend* is one way the swarm runs an agent: a command-line tool, an HTTP
endpoint, or a built-in in-process harness. This guide takes you from "I want
to add `my-cli`" to a working backend, start to finish.

There are two paths, in order of preference:

1. **Descriptor** — a config block, no code. Covers ~90% of cases.
2. **Trait impl** — write Rust when a descriptor can't express the behavior.

Start with the descriptor. Drop to the trait impl only when you hit a wall.

---

## 1. The descriptor path (the 90% case)

Most agents are either a command-line tool or an OpenAI-compatible HTTP
endpoint. For those you write **no code** — you add a `[backend.<name>]` block
to your backend config and you're done.

### The three kinds

| `kind`               | What it wraps                                              |
| -------------------- | --------------------------------------------------------- |
| `cli`                | A command-line agent, run as a subprocess.                |
| `openai-compatible`  | Any HTTP endpoint that speaks `/v1/chat/completions`.     |
| `native`             | A built-in in-process harness, selected by `provider`.    |

### `cli` — wrap a command-line agent

This is the common case. Point at an executable and describe how the prompt
and model reach it:

```toml
[backend.my-cli]
kind = "cli"

# The executable to run. Must be on PATH or an absolute path.
command = "my-cli"

# Arguments passed to `command`. Run-time tokens (see Templating) are
# substituted before the process is spawned.
args = ["--print", "--model", "{model}"]

# How the prompt reaches the process: "stdin" (default) or "arg".
prompt = "stdin"
```

### Templating

Two tokens are substituted into `args` at run time:

- `{model}` — the model id chosen for this run. Drop it if the tool has no
  model flag.
- `{prompt}` — the prompt text. You only template this when `prompt = "arg"`.
  With the default `prompt = "stdin"`, the prompt is piped to the process's
  standard input and must **not** appear in `args`.

So a stdin agent templates only `{model}`:

```toml
args = ["--print", "--model", "{model}"]   # prompt arrives on stdin
```

…while an arg-style agent templates both:

```toml
prompt = "arg"
args = ["--model", "{model}", "{prompt}"]  # prompt is the final positional arg
```

### `openai-compatible` — talk to an HTTP endpoint

For an HTTP endpoint, name the **environment variables** that hold the base URL
and the API key. Secrets are read from the environment by name — never written
into config:

```toml
[backend.some-api]
kind = "openai-compatible"
base_url_env = "SOME_API_BASE_URL"   # env var holding the endpoint base URL
api_key_env  = "SOME_API_KEY"        # env var holding the API key (a secret)
default_model = "some-model"         # used when a run specifies no model
```

At run time the backend reads `$SOME_API_BASE_URL` and `$SOME_API_KEY` from the
process environment. Keep keys out of the repo — set them in your shell, a
`.env` you don't commit, or your secret manager.

### `native` — select a built-in harness

If the engine ships an in-process harness, select it by provider id:

```toml
[backend.local]
kind = "native"
provider = "some-provider"
```

### Env-based secrets, in one line

No secret ever lives in a descriptor. The `cli` kind inherits the ambient
process environment; the `openai-compatible` kind reads named env vars. Either
way, you export the secret in your environment and reference it by name.

---

## 2. The trait-impl escape hatch

When a descriptor can't express what you need — a custom handshake, bespoke
streaming, non-standard auth, multi-step orchestration — implement the
`AgentBackend` trait in Rust.

### When you actually need this

Reach for the trait only if the descriptor kinds genuinely can't model your
agent. Examples: the agent needs a login dance before each run; output framing
isn't line-delimited stdout; you must merge several upstream calls into one
attempt. If a `cli` or `openai-compatible` block *can* do it, use that instead.

### The trait

`AgentBackend` has exactly four methods:

```rust
pub trait AgentBackend: Send + Sync {
    /// Stable identifier (e.g. "my-cli"). Used in logs, routing, telemetry.
    fn id(&self) -> &str;

    /// Gate: can this run now? Binary on PATH, key present, endpoint reachable?
    /// Return Ok(()) when ready, or BackendError::NotReady(detail) on a miss.
    fn ready(&self) -> Result<(), BackendError>;

    /// Run one attempt. Read everything from the borrowed `req`
    /// (prompt/model/cwd/timeout/quiet/bypass/env/cancel), stream output via
    /// `sink.stdout_chunk(..)` / `sink.stderr_chunk(..)`, and return a
    /// RunOutcome — or a typed BackendError.
    fn run(
        &self,
        req: &BackendRequest,
        sink: &mut dyn BackendSink,
    ) -> Result<RunOutcome, BackendError>;

    /// What this backend can do (streaming? cancellation?).
    fn capabilities(&self) -> BackendCaps;
}
```

The four methods, in plain terms:

- **`id`** — return a short, stable, lowercase string. It shows up everywhere a
  backend is named.
- **`ready`** — probe your dependency (locate the binary, check the key env
  var, ping the endpoint) and return `Ok(())` or
  `BackendError::NotReady("actionable message")`. A good `NotReady` message
  tells the operator how to fix it.
- **`run`** — do one attempt. Pull inputs from `req`, push output through
  `sink` as it arrives, and finish with a `RunOutcome`. On failure, return a
  **typed** `BackendError` (`Timeout`, `Spawn`, `Protocol`, `Upstream { .. }`,
  …) so the retry/fallback machine branches on cause — never on error text.
- **`capabilities`** — declare whether you stream and whether you honor
  `req.cancel`. `BackendCaps::default()` reports streaming without cancellation.

### Register it

Trait impls are the **library-embedding** path: programs that use the engine
as a Rust library construct their own registry and insert the backend with
[`BackendRegistry::register`]:

```rust
use swarm_exec::backend_registry::BackendRegistry;

let mut registry = BackendRegistry::from_config(&config); // builtins + your [backend.*] blocks
registry.register("my-backend", Box::new(MyBackend::new()));
// `my-backend` now resolves and dispatches like any other id.
```

(The stock `swarm` CLI binary extends via config descriptors only — it cannot
load external Rust code at runtime. If a descriptor can't express your backend
and you want it in the CLI, embed the engine in your own thin binary as above.)

---

## 3. Scaffold both with one command

Don't hand-write the boilerplate. The CLI emits both starter files for you:

```bash
agent-swarm scaffold-backend my-cli            # writes into the current dir
agent-swarm scaffold-backend my-cli --out ./backends
```

It writes two files named after your `<name>`:

- `my-cli.backend.toml` — a commented descriptor stub, defaulting to the `cli`
  kind, with the templating tokens explained inline. Start here.
- `my-cli_backend.rs` — a commented `AgentBackend` trait-impl skeleton with all
  four methods stubbed and `todo!()` placeholders. Use this only if you take
  the escape hatch.

Re-running the command regenerates both files from scratch (it overwrites), so
move any edits out of the generated files before you re-run.

Then: fill in the descriptor (most cases) or the trait impl (escape hatch), and
you have a working backend.
