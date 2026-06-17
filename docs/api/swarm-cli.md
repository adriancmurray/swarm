# swarm-cli

User-facing CLI dispatch layer for the swarm engine: the `run()`/`run_dispatch()`
entry points, the `SwarmService` dispatcher, the subcommand token table, and a
handful of CLI-only repos (package catalog, routing memory, backend scaffold).
Built into the `swarm` binary.

## Overview

`swarm-cli` is the top of the engine DAG — the layer a human (or a skill wrapper
driving the CLI) actually touches. It owns:

- **The command surface.** A frozen token table maps argv verbs
  (`fanout`, `discuss`, `converge`, `metadirector`, `audit`, `design`, `status`,
  `result`, `sessions`, `events`, `transcript`, `provider`, `doctor`, `skills`,
  `mcp`, …) to handlers. Adding/removing/renaming a token is a deliberate act
  guarded by a stability test, because the skill wrappers depend on the exact
  bytes.
- **Dispatch.** `SwarmService` parses argv, mints a job tracking record, folds in
  stdin and an optional persona, enforces the prompt-size and swarm-depth limits,
  and routes the request to either a background job or a foreground run in
  [[swarm-exec]].
- **CLI-only diagnostics and management.** `doctor` (config/backend/routing/
  credential health), `provider` (credential vault CRUD), `skills` (list loadable
  `SKILL.md` skills), and `scaffold-backend` (emit starter files for a new
  backend).
- **Two thin repository read-models** unique to this crate: a static package/
  profile catalog ([`PackageRepo`](#packagerepo)) and an agent-routing memory
  read-model over telemetry ([`RoutingMemoryRepo`](#routingmemoryrepo)).

Everything lives in the library so the `swarm` binary is an ~8-line shim
(`std::process::exit(swarm_cli::SwarmService::new().run()…)`).

The actual orchestration verbs (`run_swarm`, `run_discussion`, `run_converge`,
`cmd_preset`, `run_partner_foreground`) and all read/inspect command bodies are
**not defined here** — they live in [[swarm-exec]], [[swarm-mcp]], and
[[swarm-kernel]] and are merely wired into the dispatch table. This crate is the
plumbing that turns argv into one of those calls.

## Dependency position

Top of the DAG, alongside [[swarm-mcp]]:

```
swarm-contracts → swarm-core → swarm-store → swarm-kernel → swarm-exec → swarm-cli
swarm-manager   (pulled in for the provider registry + credential vault)
```

Depends on **swarm-exec** (orchestration verbs, background/monitor runtimes,
executor, backend registry, synthesis), **swarm-mcp** (`cmd_mcp`, manifest,
MCP-binary detection), **swarm-kernel** (arg parsing, config, agent model,
profiles, telemetry aggregation), **swarm-store** (job repo, store paths, atomic
writes, provider/skills dirs), **swarm-core** (`RepoError`), **swarm-contracts**
(job types), and **swarm-manager** (`ProviderRegistry`, `KeychainVault`,
`load_skills` — default features only, no async runtime or HTTP client). Nothing
depends on `swarm-cli`; it is a leaf consumed only by the `swarm` binary.

## Concepts

Public API surface (everything reachable from `lib.rs`):

- [`run()`](#run) — process entry point; build a `SwarmService` and dispatch `env::args()`.
- [`run_dispatch(args)`](#run_dispatch) — dispatch an already-parsed `Args` (used by re-entrant verbs).
- [`SwarmService`](#swarmservice) — the top-level dispatcher (parse argv → job record → background/foreground).
- [`SwarmService::new()`](#swarmservicenew) — construct the (stateless) dispatcher.
- [`SwarmService::run()`](#swarmservicerun) — read argv, enforce depth, route a verb or fall through to `run_dispatch`.
- [`SwarmService::run_dispatch()`](#swarmservicerun_dispatch) — the single-run path: mint a job, fold stdin/persona, size-check, run.
- [`PackageRepo`](#packagerepo) — read-only trait over package manifest, presets, and agent profiles.
- [`StaticPackageRepo`](#staticpackagerepo) — compiled-in `PackageRepo` impl (the only Phase-1 backend).
- [`RoutingMemoryRepo`](#routingmemoryrepo) — read-model trait: agent stats + role recommendations from telemetry.
- [`RoutingMemory<T>`](#routingmemoryt) — blanket `RoutingMemoryRepo` impl over any `TelemetryRepo`.
- [`RoleRecommendation`](#rolerecommendation) — a `role → agent` recommendation row.
- [`RECOMMENDATION_ROLES`](#recommendation_roles) — the standard role list for `recommendations()`.
- [`scaffold_backend()`](#scaffold_backend) — pure function: emit a backend Rust skeleton + descriptor stub.
- [`cmd_scaffold_backend()`](#cmd_scaffold_backend) — thin CLI wrapper around `scaffold_backend`.

Crate-internal surface (private modules; reachable only through the dispatcher,
not exported — listed for discovery):

- [Command token table](#command-token-table) — `CliCommand` enum + `CLI_COMMANDS` (`cli.rs`, crate-private).
- [Internal command handlers](#internal-command-handlers) — `cmd_doctor`/`run_doctor`, `cmd_provider`/`run_provider`, `cmd_skills`, and the read/inspect `cmd_*` handlers.

## API surface

### run

```rust
pub fn run() -> Result<i32, String>
```

Process entry point. Constructs a fresh [`SwarmService`](#swarmservice) and calls
its [`run()`](#swarmservicerun), which reads `std::env::args()`. Returns the
intended process exit code (or an `Err(String)` the caller is expected to print
and turn into exit 1).

**When to use:** from a binary `main` when you want swarm to drive itself off the
real process argv. The `swarm` binary does exactly this. If you already have a
parsed `Args`, use [`run_dispatch`](#run_dispatch) instead.

Related: [[swarm-cli#swarmservicerun]], [[swarm-cli#run_dispatch]]

### run_dispatch

```rust
pub fn run_dispatch(args: Args) -> Result<i32, String>
```

Dispatch an already-parsed [`Args`](../api/swarm-kernel.md#args) through a fresh
`SwarmService`, skipping argv parsing and verb routing — it goes straight to the
single-run path ([`SwarmService::run_dispatch`](#swarmservicerun_dispatch)).

**Params:** `args` — a fully parsed `Args` (agent, model, prompt, cwd, flags).
**Returns:** the intended process exit code, or `Err(String)`.

**When to use:** when a verb needs to re-enter dispatch with a synthetic argv —
e.g. the `metadirector` verb rewrites its args (`--agent gemini --persona … --quiet`)
and calls back through this path. Library callers embedding swarm that have built
their own `Args` also use this.

Related: [[swarm-cli#swarmservicerun_dispatch]], [[swarm-kernel#args]]

### SwarmService

```rust
#[derive(Default)]
pub struct SwarmService;
```

The top-level CLI dispatcher. A zero-sized, stateless unit struct — all state is
read from argv/env/store at call time, so it is cheap to construct and holds no
handles. Re-exported at the crate root (`swarm_cli::SwarmService`).

Responsibilities: parse argv into a verb + tail; route known verbs to their
handlers in [[swarm-exec]]/[[swarm-mcp]]/the internal command modules; for a bare
prompt (no recognized verb), parse it as [`Args`](../api/swarm-kernel.md#args) and
fall through to the single-run path, which mints a job record, folds in stdin and
an optional persona, enforces the prompt-byte and swarm-depth ceilings, and runs
foreground or background.

**When to use:** the standard handle for embedding the CLI. `SwarmService::new().run()`
drives off real argv; `.run_dispatch(args)` drives off a pre-built `Args`.

Related: [[swarm-cli#run]], [[swarm-cli#command-token-table]], [[swarm-exec#run_partner_foreground]]

### SwarmService::new

```rust
impl SwarmService {
    pub fn new() -> Self
}
```

Construct the dispatcher. No-op beyond returning the unit struct (equivalent to
`SwarmService::default()`); present so call sites read as `SwarmService::new().run()`.

Related: [[swarm-cli#swarmservice]]

### SwarmService::run

```rust
impl SwarmService {
    pub fn run(&self) -> Result<i32, String>
}
```

Drive a full dispatch from the real process argv (`env::args().skip(1)`).

Flow, in order:
1. **Depth guard.** If `current_swarm_depth()` (from `SWARM_DEPTH`, via
   [[swarm-exec]]) is already at the ceiling (`MAX_SWARM_DEPTH = 3`), refuse to
   recurse and return exit `124` (`EXIT_DEPTH_LIMIT_EXCEEDED`).
2. **Help.** Any `-h`/`--help` anywhere in argv prints `print_help` and returns 0.
3. **Bare MCP.** Empty argv while invoked as the MCP binary (`invoked_as_mcp_binary()`)
   runs the stdio MCP loop `cmd_mcp()`.
4. **Verb dispatch.** If `argv[0]` parses via [`CliCommand::parse`](#command-token-table),
   route to the matching handler (each verb parses its own tail). `fanout`/`swarm`
   → `run_swarm`; `discuss`/`audit`/`design` → `run_discussion`; `converge` →
   `run_converge`; `metadirector` → re-entrant `run_dispatch` with rewritten args;
   read/inspect/management verbs → their `cmd_*` handlers.
5. **Bare prompt.** No recognized verb → `parse_args(argv)` then
   [`run_dispatch`](#swarmservicerun_dispatch).

**Returns:** the process exit code, or `Err(String)` for parse/IO failures.

**When to use:** indirectly, via [`run()`](#run). Call directly only when
embedding and you want swarm to parse the real argv itself.

Related: [[swarm-cli#command-token-table]], [[swarm-exec#run_swarm]], [[swarm-exec#run_discussion]], [[swarm-exec#run_converge]], [[swarm-mcp#cmd_mcp]]

### SwarmService::run_dispatch

```rust
impl SwarmService {
    pub fn run_dispatch(&self, args: Args) -> Result<i32, String>
}
```

The single-run path: turn one parsed [`Args`](../api/swarm-kernel.md#args) into a
tracked agent/consult run.

Flow:
1. **Job record.** For foreground runs, mint a `JobRecord` via `FileJobRepo`
   ([[swarm-store]]) — resolves the agent, derives `JobMode` (`Consult` when
   `--quiet`, else `Agent`), and records model/cwd/timeout/prompt preview.
   Background runs defer record creation to the background runtime.
2. **Stdin fold.** If stdin is piped (not a TTY) and ready within ~250ms, read it
   and prepend it to the prompt as a fenced `Context:` block.
3. **Persona.** If `args.persona` is set, wrap the prompt via
   `build_direct_persona_prompt` ([[swarm-exec]]).
4. **Persist prompt.** Write the final prompt to the job's `prompt_path` (atomic)
   and refresh the preview.
5. **Size guard.** If the prompt exceeds `MAX_PROMPT_BYTES` (180 000), fail the
   job record with exit `2` and return.
6. **Route.** `args.background` → `start_background_job`; otherwise
   `run_partner_foreground` ([[swarm-exec]]).

**Params:** `args` — the parsed run request.
**Returns:** the run's exit code, or `Err(String)`.

**When to use:** the path every non-verb invocation lands on, and the target of
the public [`run_dispatch`](#run_dispatch) free function and the re-entrant
`metadirector` verb.

Related: [[swarm-exec#run_partner_foreground]], [[swarm-exec#background-runtime]], [[swarm-store#file-job-repo]], [[swarm-contracts#jobstatus--jobagent--jobmode]]

### PackageRepo

```rust
pub trait PackageRepo: Send + Sync {
    fn manifest(&self) -> Result<serde_json::Value, RepoError>;
    fn presets(&self) -> Result<serde_json::Value, RepoError>;
    fn profiles(&self) -> Result<Vec<AgentProfile>, RepoError>;
}
```

Read-only access to the package manifest, named presets, and agent profiles. A
trait boundary placed in front of today's compiled-in (hardcoded) catalog data so
callers depend on the contract rather than the concrete free functions; the write
path (preset installation, version management) is explicitly Phase-2 and out of
scope.

**Methods:**
- `manifest()` — the `swarm.manifest/v1` JSON (same shape as
  `swarm_mcp::manifest::manifest_payload()`).
- `presets()` — the named-presets JSON (same shape as
  `swarm_kernel::telemetry::presets_json()`).
- `profiles()` — the agent [`AgentProfile`](../api/swarm-kernel.md#role-profiles)
  list (compiled-in in Phase 1).

All return `RepoError` ([[swarm-core#repoerror]]) on failure.

**When to use:** when a consumer needs the package catalog and you want to keep it
testable / future-swappable. The only Phase-1 impl is
[`StaticPackageRepo`](#staticpackagerepo).

Related: [[swarm-cli#staticpackagerepo]], [[swarm-mcp#manifest]], [[swarm-kernel#role-profiles]], [[swarm-core#repoerror]]

### StaticPackageRepo

```rust
pub struct StaticPackageRepo;

impl PackageRepo for StaticPackageRepo { /* manifest / presets / profiles */ }
```

The compiled-in [`PackageRepo`](#packagerepo) implementation: each method
delegates to the existing hardcoded data sources (`manifest_payload`,
`presets_json`, `profiles`). Reads no files and performs no writes, so it doubles
as both the production backend and the in-memory test double (they are identical
in Phase 1, where there is no durable package store).

**When to use:** the default `PackageRepo` everywhere a package catalog is needed.

Related: [[swarm-cli#packagerepo]]

### RoutingMemoryRepo

```rust
pub trait RoutingMemoryRepo: Send + Sync {
    fn agent_stats(&self) -> Result<Vec<AgentStats>, RepoError>;
    fn best_agent_for_role(&self, role: &str) -> Result<String, RepoError>;
    fn recommendations(&self) -> Result<Vec<RoleRecommendation>, RepoError>;
}
```

A pure **read-model** over a [`TelemetryRepo`](../api/swarm-core.md#telemetryrepo):
every call reads raw observations + feedback and runs the aggregation math from
`swarm_kernel::telemetry`. There is no durable scoring cache (Phase 2) — each call
re-aggregates from scratch.

> **Naming caution.** "Routing" here means *agent-routing memory* (which agent
> performed best for which role), **not** the backend-fallback chain in
> [[swarm-kernel#fallback-routing]]. This module is intentionally not wired into
> `routing.rs`.

**Methods:**
- `agent_stats()` — aggregated per-`(role, agent)`
  [`AgentStats`](../api/swarm-kernel.md#telemetry-aggregation); empty `Vec` when
  the store is empty.
- `best_agent_for_role(role)` — the highest-scoring agent for `role`, falling back
  to that role's default agent when there is no telemetry.
- `recommendations()` — one [`RoleRecommendation`](#rolerecommendation) per role in
  [`RECOMMENDATION_ROLES`](#recommendation_roles).

All return `RepoError` ([[swarm-core#repoerror]]) on any underlying I/O or
deserialization failure.

**When to use:** to surface telemetry-derived routing recommendations behind a
trait boundary (e.g. for the `recommend` command, or for a future learned-routing
consumer). For the underlying scoring math see [[swarm-kernel#telemetry-aggregation]].

Related: [[swarm-cli#routingmemoryt]], [[swarm-cli#rolerecommendation]], [[swarm-kernel#telemetry-aggregation]], [[swarm-core#telemetryrepo]]

### RoutingMemory&lt;T&gt;

```rust
pub struct RoutingMemory<T: TelemetryRepo> { /* telemetry: T */ }

impl<T: TelemetryRepo> RoutingMemory<T> {
    pub fn new(telemetry: T) -> Self;
}

impl<T: TelemetryRepo> RoutingMemoryRepo for RoutingMemory<T> { /* ... */ }
```

The single blanket [`RoutingMemoryRepo`](#routingmemoryrepo) implementation,
generic over any owned [`TelemetryRepo`](../api/swarm-core.md#telemetryrepo)
backend (owned, not `&dyn`, to avoid lifetime parameters). The two intended
instantiations are `RoutingMemory<MemTelemetryRepo>` (in-memory, test-fast) and
`RoutingMemory<FileTelemetryRepo>` (JSONL-on-disk); both flow through this one
impl.

**`new(telemetry)`** — wrap a telemetry backend. Each trait method then reads its
`observations()` + `feedback()` and feeds them to `aggregate_stats` /
`best_agent_for_role` from [[swarm-kernel#telemetry-aggregation]].

**When to use:** the concrete way to obtain a `RoutingMemoryRepo` — pick the
telemetry backend (file or mem) and wrap it.

Related: [[swarm-cli#routingmemoryrepo]], [[swarm-store#filetelemetryrepo--memtelemetryrepo]], [[swarm-kernel#telemetry-aggregation]]

### RoleRecommendation

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoleRecommendation {
    pub role: String,
    pub agent: String,
}
```

A single `role → agent` recommendation row produced by
[`RoutingMemoryRepo::recommendations`](#routingmemoryrepo): the best agent for the
named `role` per current telemetry (or that role's default when telemetry is
empty).

**When to use:** the return element of `recommendations()`. Match on `role` to find
a specific row.

Related: [[swarm-cli#routingmemoryrepo]], [[swarm-cli#recommendation_roles]]

### RECOMMENDATION_ROLES

```rust
pub const RECOMMENDATION_ROLES: &[&str] = &[
    "architecture",
    "hardening",
    "product-design",
    "api-docs",
    "manager",
];
```

The standard role list `recommendations()` iterates. Mirrors the role list in
`telemetry.rs::recommendations_from_stats` — so the CLI read-model and the kernel
aggregation agree on which roles get a recommendation row.

**When to use:** to know which roles `recommendations()` always returns (one entry
each), or to iterate the same canonical role set yourself.

Related: [[swarm-cli#routingmemoryrepo]], [[swarm-cli#rolerecommendation]]

### scaffold_backend

```rust
pub fn scaffold_backend(name: &str, out_dir: &Path) -> io::Result<Vec<PathBuf>>
```

Pure function (templates → files) that emits the two starter files for a new agent
backend into `out_dir`:
- `<name>_backend.rs` — a commented [`AgentBackend`](../api/swarm-exec.md#agentbackend-trait)
  trait-impl skeleton with all four methods (`id`/`ready`/`run`/`capabilities`)
  stubbed, signatures matching the real trait. The escape hatch for when a
  descriptor can't express the behavior.
- `<name>.backend.toml` — a commented [`BackendDescriptor`](../api/swarm-kernel.md#backenddescriptor--backendkind--promptdelivery)
  stub defaulting to the `cli` kind. The no-code (90%) way to add an agent.

**Params:** `name` — the backend id (PascalCased for the Rust struct prefix;
`---`-only names fall back to `Custom`); `out_dir` — created if missing.
**Returns:** the written paths in stable order — Rust skeleton first, descriptor
second.

**Overwrite behavior:** files are written unconditionally; re-running clobbers prior
output for the same `name`. All emitted identifiers derive from `name` only, so the
output carries no project-specific strings (enforced by a born-clean test).

**When to use:** programmatically generate a backend starting point. For the CLI
wrapper, use [`cmd_scaffold_backend`](#cmd_scaffold_backend).

Related: [[swarm-cli#cmd_scaffold_backend]], [[swarm-exec#agentbackend-trait]], [[swarm-kernel#backenddescriptor--backendkind--promptdelivery]]

### cmd_scaffold_backend

```rust
pub fn cmd_scaffold_backend(args: &[String]) -> Result<i32, String>
```

Thin CLI wrapper around [`scaffold_backend`](#scaffold_backend) for the
`scaffold-backend <name> [--out DIR]` verb. Parses the name and optional `--out`
directory (default: cwd), handles `-h`/`--help`, writes the files, and prints the
written paths plus a pointer to `docs/authoring-a-backend.md`.

**Params:** `args` — the verb tail (everything after `scaffold-backend`).
**Returns:** `0` on success, or `Err(String)` for a missing name / unknown flag /
write failure.

**When to use:** dispatched by [`SwarmService::run`](#swarmservicerun) for the
`scaffold-backend` token; not normally called directly.

Related: [[swarm-cli#scaffold_backend]], [[swarm-cli#command-token-table]]

---

## Crate-internal surface

These items are **not exported** from `lib.rs` (their modules are private `mod`
declarations). They are documented here only to serve the index → concept flow;
reach them through [`SwarmService::run`](#swarmservicerun) dispatch, not as a
library API.

### Command token table

`crates/swarm-cli/src/cli.rs` (crate-private)

```rust
pub(crate) enum CliCommand { Status, Result, Cancel, Manifest, /* … */ ScaffoldBackend }

const CLI_COMMANDS: &[(&str, CliCommand)] = &[ ("status", …), ("result", …), … ];

impl CliCommand {
    pub(crate) fn parse(token: &str) -> Option<Self>;
}
```

The frozen subcommand vocabulary. `CLI_COMMANDS` is the ordered `(token, variant)`
table; `CliCommand::parse` does a linear lookup. The full token set (order is
contractual):

`status`, `result`, `cancel`, `manifest`, `insights`, `profiles`, `hooks`,
`automation-hooks`, `presets`, `recommend`, `feedback`, `proposals`, `propose`,
`proposal-vote`, `preset`, `eval-metadirector`, `ledger`, `monitor`,
`monitor-once`, `monitor-start`, `monitor-status`, `alerts`, `watch`, `mcp`,
`swarm`, `fanout`, `discuss`, `converge`, `metadirector`, `design`, `audit`,
`sessions`, `runtime-processes`, `runtime_processes` (underscore alias),
`events`, `transcript`, `conductor-hook`, `activity-record`, `__job-worker`,
`__command-worker` (the two `__`-prefixed worker re-exec entry points),
`run`, `overview`, `provider`, `skills`, `doctor`, `scaffold-backend`.

A `#[test]` (`cli_subcommand_token_contract_is_stable`) pins this exact list,
asserts uniqueness, and asserts every token dispatches — the skill wrappers that
drive swarm depend on these exact bytes, so any change is deliberate.

Related: [[swarm-cli#swarmservicerun]]

### Internal command handlers

Crate-private `cmd_*` (and testable `run_*`) functions, each
`fn(&[String]) -> Result<i32, String>` (some take no args), reached only through
the dispatch table:

- **Doctor** — `crates/swarm-cli/src/doctor.rs`: `cmd_doctor` (parses
  `[--probe] [--data-dir PATH]`) and the testable core
  `run_doctor(config, status, registry, providers, probe, out)`. One pass over the
  same paths a real run takes — parse config, build the
  [`BackendRegistry`](../api/swarm-exec.md#backendregistry), probe every backend's
  `ready()`, sanity-check descriptors, scan routing strings for unresolvable ids,
  and report each stored provider's credential status. Exit `0` when no blocking
  issues, `1` otherwise (blocking = a `ready()` failure or a routing id that
  resolves nowhere; descriptor/credential gaps are warnings).
- **Provider** — `crates/swarm-cli/src/provider_commands.rs`: `cmd_provider` and the
  testable `run_provider(registry, args, key_input, out)`, dispatching
  `add` / `models` / `list` / `key set` / `key check` / `remove` against
  swarm-manager's [`ProviderRegistry`](../api/swarm-manager.md#providerregistry--providerconfig)
  rooted at `swarm_home()/providers`. Key material is never an argv token — `key set`
  reads stdin or copies from a named env var, and only a masked length is echoed.
  `key_status_label(config)` maps a `ProviderConfig` to a human credential label.
- **Skills** — `crates/swarm-cli/src/skills_commands.rs`: `cmd_skills` (only
  subcommand: `list [--dir PATH]…`). Scans the same dirs a native run consults —
  home skills dir under the project-local `<cwd>/.swarm/skills` (which overrides by
  name) — via `swarm_manager::load_skills`, printing each skill's name,
  description, source, and allowed-tools; malformed skills are reported as issues.
- **Read / inspect / write commands** — `crates/swarm-cli/src/cli_commands.rs` and
  `cli_read_commands.rs`: `cmd_status`, `cmd_result`, `cmd_cancel`, `cmd_sessions`,
  `cmd_runtime_processes`, `cmd_session_events`, `cmd_session_transcript`,
  `cmd_alerts`, `cmd_ledger`, `cmd_overview`, `cmd_insights`, `cmd_profiles`,
  `cmd_automation_hooks`, `cmd_presets`, `cmd_manifest`, `cmd_conductor_hook`,
  `cmd_activity_record`, `cmd_recommend`, `cmd_eval_metadirector`, `cmd_feedback`,
  `cmd_proposals`, `cmd_propose`, `cmd_proposal_vote`. These mostly format JSON
  produced upstream ([[swarm-mcp]] report assemblers, [[swarm-kernel]] telemetry/
  profiles, [[swarm-store]] job/session/monitor/ledger stores).

Related: [[swarm-cli#command-token-table]], [[swarm-exec#backendregistry]], [[swarm-manager#providerregistry--providerconfig]], [[swarm-manager#skills]]
