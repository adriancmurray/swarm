# swarm-exec

The orchestration engine: parallel fan-out, multi-agent discussion, convergence, single-agent execution with retry/fallback, prompt/transcript synthesis, sessions, preflight, the backend registry + concrete backends, and the background/monitor runtimes.

## Overview

`swarm-exec` is the mechanism layer of the swarm engine. Everything below it is data (`swarm-contracts`), storage contracts (`swarm-core`), storage implementations (`swarm-store`), or stateless leaf logic (`swarm-kernel`). This crate is where those pieces are *driven*: it spawns agent backends, walks retry/fallback chains, threads parallel workers, synthesizes manager prompts and artifacts, persists session events, and runs the background-job and monitor sidecars.

It owns:

- **Orchestrators** (`orchestration`) — `run_swarm` (fanout), `run_discussion` (discuss/audit/design), `run_converge` (iterate to convergence), `run_partner_foreground` (single routed run), `cmd_preset` (expand a named pipeline).
- **The backend execution contract** (`executor`) — the `AgentBackend` trait, built-in `CodexBackend`/`ClaudeBackend`, the `execute_partner`/`execute_with_fallback` family, the shared retry→fallback driver, and telemetry recording.
- **The backend registry** (`backend_registry`) — resolve any backend by stable string id; seeded with built-ins + aliases and every `[backend.<id>]` config descriptor.
- **Concrete descriptor-driven backends** — `CliBackend` (`cli_backend`), `OpenAiCompatibleBackend` (`openai_backend`, `openai` feature), `NativeBackend` (`native_backend`, `native` feature).
- **Synthesis** (`synthesis`) — prompt builders, the worker evidence gate, transcript/artifact builders, the metadirector contract verifier.
- **Sessions** (`session`) — `DiscussionSession` (the on-disk session directory + event/layer-report appenders), `DiscussionTurn`, `list_sessions`.
- **Preflight** (`preflight`) — backend-availability checks and classified error payloads.
- **Background runtime** (`background_runtime`) — enqueue a detached partner job and the worker entry points the detached process re-execs into.
- **Monitor runtime** (`monitor_runtime`) — the RSS/stale-activity sampler loop and the filesystem-watch NDJSON streamer.

The policy layer (`main()` / `run()` / `run_dispatch()`, depth/prompt guards, stdin handling) lives in `swarm-cli`; this crate is the mechanism it delegates to.

## Dependency position

```
swarm-contracts ← swarm-core ← swarm-store ← swarm-kernel ← swarm-exec
```

`swarm-exec` sits directly above `swarm-kernel` and is consumed by `swarm-mcp` and `swarm-cli`. Its Gate-2 isolation invariant: it depends ONLY on `swarm-kernel` (stateless leaves), `swarm-store` (file-backed store machinery), `swarm-core` (repo traits + companions), `swarm-contracts` (wire types), and a small set of external crates (serde, serde_json, toml, sysinfo, notify, libc on unix; plus ureq under `openai` and `swarm-manager` under `native`). No external-system deps.

Optional features:
- `native` — compiles `native_backend` (in-process single-agent loop via `swarm-manager`).
- `openai` — compiles `openai_backend` (HTTP OpenAI-compatible endpoint via ureq).

When a feature is off, its descriptor kind still *resolves* in the registry but errors loudly at `ready()`/`run()` (an `UnavailableBackend`) — never a silent no-op.

## Concepts

Bullet index of every public item, grouped by module. Each links to its API entry.

### orchestration — verb entry points
- [`run_swarm`](#run_swarm) — parallel worker fan-out then manager synthesis (the `fanout`/`swarm` verb).
- [`run_discussion`](#run_discussion) — bounded multi-participant rounds + rolling digest + optional docs follow-up (`discuss`/`audit`/`design`).
- [`run_converge`](#run_converge) — iterate participant rounds, feeding each manager synthesis back as the next baseline.
- [`run_partner_foreground`](#run_partner_foreground) — single routed agent run, foreground, with job-record tracking.
- [`cmd_preset`](#cmd_preset) — expand a named preset (config or built-in) into a swarm/discussion/converge run.

### executor — backend execution + retry/fallback
- [`AgentBackend`](#agentbackend) — the per-backend single-attempt execution trait (`id`/`ready`/`run`/`capabilities`).
- [`CodexBackend`](#codexbackend) — built-in backend dispatching via `codex exec`.
- [`ClaudeBackend`](#claudebackend) — built-in backend dispatching via `claude --print`.
- [`execute_partner`](#execute_partner) — resolve a backend by id and run one attempt (no fallback).
- [`execute_partner_with_chunks`](#execute_partner_with_chunks) — `execute_partner` with a streaming chunk callback.
- [`ChunkCallback`](#chunkcallback) — the `(stream, chunk)` closure type for streaming sinks.
- [`execute_with_fallback`](#execute_with_fallback) — run a backend fallback chain (non-chunked) with retry + fallback.
- [`execute_with_fallback_chunks`](#execute_with_fallback_chunks) — fallback chain with live chunk streaming.
- [`FallbackAttempt`](#fallbackattempt) — one recorded attempt within a fallback chain.
- [`FallbackOutcome`](#fallbackoutcome) — the result of walking a fallback chain (used spec, result, attempt trail).
- [`parse_claude_json_output`](#parse_claude_json_output) — parse the `claude --output-format json` envelope into text + token counts.
- [`print_partner_output`](#print_partner_output) — print a `RunOutcome` to stdout/stderr and return the exit code.
- [`output_status_code`](#output_status_code) — exit code for a `RunOutcome` (124 on timeout).
- [`output_record_status`](#output_record_status) — map a `RunOutcome` + code to a `JobStatus`.
- [`record_agent_observation`](#record_agent_observation) — write a success/finished telemetry observation.
- [`record_agent_error`](#record_agent_error) — write a failed telemetry observation.
- [`current_swarm_depth`](#current_swarm_depth) — read the recursion-depth counter from `SWARM_DEPTH`.

### backend_registry — id → backend resolution
- [`BackendRegistry`](#backendregistry) — resolve agent backends by stable string id; built-ins + aliases + config descriptors + custom trait impls.

### cli_backend — descriptor-driven subprocess backend
- [`CliBackend`](#clibackend) — run a `kind = cli` descriptor as a subprocess with token substitution.

### openai_backend — HTTP backend (`openai` feature)
- [`OpenAiCompatibleBackend`](#openaicompatiblebackend) — talk to any OpenAI-compatible `/chat/completions` endpoint (streaming or one-shot).

### native_backend — in-process backend (`native` feature)
- [`NativeBackend`](#nativebackend) — run the manager's single-agent loop in process via `swarm-manager`.

### synthesis — prompts, gates, artifacts
- [`assess_worker_output`](#assess_worker_output) — score one worker's output into a `WorkerEvidenceGate`.
- [`WorkerEvidenceGate`](#workerevidencegate) — the citation/evidence/health verdict for a worker turn.
- [`render_context_block`](#render_context_block) — render auto-gathered context JSON into a bounded prompt block.
- [`build_worker_prompt`](#build_worker_prompt) — the per-worker fan-out prompt.
- [`build_direct_persona_prompt`](#build_direct_persona_prompt) — wrap a direct task in a persona/profile contract.
- [`build_manager_prompt`](#build_manager_prompt) — the fan-out manager synthesis prompt.
- [`build_swarm_result_artifact`](#build_swarm_result_artifact) — the `# Agent Swarm Result` markdown artifact.
- [`build_swarm_transcript`](#build_swarm_transcript) — the fan-out transcript markdown.
- [`build_profile_helper_prompt`](#build_profile_helper_prompt) — one-layer helper agent prompt.
- [`build_discussion_turn_prompt`](#build_discussion_turn_prompt) — a participant's per-round discussion prompt.
- [`build_discussion_digest`](#build_discussion_digest) — the rolling discussion digest (bounded).
- [`build_discussion_manager_prompt`](#build_discussion_manager_prompt) — the discussion manager synthesis prompt.
- [`capped_manager_output`](#capped_manager_output) — cap manager output to the compact byte budget.
- [`build_docs_prompt`](#build_docs_prompt) — the API-docs follow-up subagent prompt.
- [`verify_metadirector_contract`](#verify_metadirector_contract) — deterministic check that output has the required metadirector sections.
- [`COMPACT_HANDOFF_CONTRACT`](#compact_handoff_contract) — re-export of the shared compact-packet contract string.
- [`preview_for_event`](#preview_for_event) — re-export of the kernel's bounded-preview helper.

### session — on-disk session + events
- [`DiscussionSession`](#discussionsession) — a session directory plus its event/layer-report/metadata appenders.
- [`DiscussionTurn`](#discussionturn) — one participant turn captured for digest/synthesis.
- [`SessionIndexRecord`](#sessionindexrecord) — lightweight record from scanning a session dir.
- [`list_sessions`](#list_sessions) — list all sessions under the default store dir.
- [`list_sessions_from_base`](#list_sessions_from_base) — list sessions under an explicit base, deriving status.

### preflight — availability + error classification
- [`run_session_preflight`](#run_session_preflight) — verify the manager + every participant backend is runnable; emit preflight events.
- [`classified_agent_error_payload`](#classified_agent_error_payload) — build a classified error event for a failed participant.
- [`classified_error_payload`](#classified_error_payload) — build a classified error event for a labeled failure.
- [`classify_error`](#classify_error) — map an error string to a category (auth / missing-backend / timeout / runtime).
- [`suggested_action_for_error`](#suggested_action_for_error) — the actionable next step for an error's category.

### background_runtime — detached jobs
- [`start_background_job`](#start_background_job) — write a queued job, spawn a detached worker, flip to running.
- [`cmd_job_worker`](#cmd_job_worker) — the `__job-worker` entry point: run a persisted partner job to terminal status.
- [`cmd_command_worker`](#cmd_command_worker) — the `__command-worker` entry point: re-exec a captured subcommand to terminal status.

### monitor_runtime — live observation
- [`cmd_monitor`](#cmd_monitor) — run the monitor poll loop forever.
- [`cmd_monitor_once`](#cmd_monitor_once) — run a single monitor tick with a forced heartbeat.
- [`cmd_monitor_start`](#cmd_monitor_start) — spawn a detached monitor sidecar (with `--replace`).
- [`cmd_monitor_status`](#cmd_monitor_status) — print the monitor sidecar's running state as JSON.
- [`cmd_watch`](#cmd_watch) — stream job/session/monitor store changes as NDJSON.

---

## API surface

### run_swarm

```rust
pub fn run_swarm(args: SwarmArgs) -> Result<i32, String>
```

Run the fan-out swarm: spawn one thread per worker (each walking its own fallback chain), join them, then run the manager to synthesize a final answer from all worker outputs.

- **Params:** `args` — parsed `SwarmArgs` (prompt, cwd, manager spec, worker specs, timeout, `inject_context`/`learned` overrides, optional `parent`/`slice`). Requires at least one worker.
- **Returns:** process exit code — `0` on success, `1` if any worker or the manager timed out / exited non-zero. `Err` only on hard setup failures (empty workers, preflight failure, store errors).
- **Side effects:** creates a `DiscussionSession`, writes swarm metadata + `session.json`, runs preflight, mints a tracking `JobRecord`, optionally auto-gathers context, emits `FanoutStarted`/`WorkerStarted`/`WorkerCompleted`/`ManagerCompleted`/`SessionCompleted` events + layer reports, records telemetry observations, and writes `summary.md` + `transcript.md`.
- **When to use:** the `fanout` / `swarm` CLI verb — parallel specialists, then one synthesis pass. Use [`run_discussion`](#run_discussion) instead when participants should debate across rounds.

Related: [[swarm-kernel#SwarmArgs]], [[swarm-exec#DiscussionSession]], [[swarm-exec#execute_with_fallback]], [[swarm-exec#build_manager_prompt]], [[swarm-exec#run_session_preflight]], [[swarm-kernel#build_fallback_chain]]

### run_discussion

```rust
pub fn run_discussion(args: DiscussArgs) -> Result<i32, String>
```

Run a bounded multi-participant discussion: for each of `args.rounds`, every participant takes a turn (streaming chunks, optional profile helpers, optional epistemic red-team falsification) seeded by a rolling digest of prior turns; then the manager synthesizes a final recommendation and an optional API-docs follow-up runs.

- **Params:** `args` — parsed `DiscussArgs` (prompt, cwd, manager, participants, `rounds`, `docs`/`docs_agent`, `profile_helpers`, timeout, `learned`). Requires ≥1 participant and `rounds ≥ 1`.
- **Returns:** exit code — `0` clean, `1` if any turn/manager/docs step failed. `Err` on hard setup failure.
- **Side effects:** session creation + metadata, preflight, tracking job, per-turn `ProfileAssigned`/`AgentMessage`/`TurnStarted`/`TurnChunk`/`TurnHeartbeat`/`TurnCompleted`/`TurnFailed`/`TurnHealthCheck` events, `DiscussionDigestUpdated`, manager + docs events, layer reports, telemetry, and `transcript.md`/`digest.md`/`summary.md`/`api-docs.md` writes.
- **When to use:** the `discuss`/`audit`/`design` verbs — when participants should react to each other over rounds rather than answer in isolation.

Related: [[swarm-kernel#DiscussArgs]], [[swarm-exec#DiscussionTurn]], [[swarm-exec#build_discussion_turn_prompt]], [[swarm-exec#build_discussion_digest]], [[swarm-exec#build_discussion_manager_prompt]], [[swarm-exec#execute_with_fallback_chunks]]

### run_converge

```rust
pub fn run_converge(args: ConvergeArgs) -> Result<i32, String>
```

Run a convergence pipeline: for each of `args.iterations`, every participant proposes/refines a plan from its persona, then the "Director" (manager) integrates the proposals into a single baseline. The director's output becomes the next iteration's baseline; the final iteration emits the unified plan.

- **Params:** `args` — parsed `ConvergeArgs` (prompt, cwd, manager, participants, `iterations`, `profile_helpers`, timeout, `learned`).
- **Returns:** `Ok(0)` on completion; `Err` if a manager synthesis step fails (the loop aborts).
- **Side effects:** creates a converge session, optional context gather, per-iteration worker threads with telemetry + layer reports, prints iteration progress and convergence output to stdout. Note: this verb does not mint a tracking `JobRecord` (unlike swarm/discussion).
- **When to use:** the `converge` verb — when you want multiple personas to iteratively reconcile toward one agreed implementation plan.

Related: [[swarm-kernel#ConvergeArgs]], [[swarm-exec#build_direct_persona_prompt]], [[swarm-exec#execute_with_fallback]], [[swarm-kernel#build_fallback_chain]]

### run_partner_foreground

```rust
pub fn run_partner_foreground(
    args: &Args,
    prompt: &str,
    tracking: Option<JobRecord>,
    job_repo: &FileJobRepo,
) -> Result<i32, String>
```

Run a single routed agent in the foreground (no fan-out, no fallback chain): resolve the agent (built-in or custom), dispatch once via [`execute_partner`](#execute_partner), print the output, and finish the caller-provided tracking record.

- **Params:** `args` — the single-run `Args`; `prompt` — the resolved prompt text; `tracking` — an optional pre-created `JobRecord` to finalize; `job_repo` — the repo that created it (shared so both sides use one instance).
- **Returns:** the partner's exit code (124 on timeout via [`print_partner_output`](#print_partner_output)); `Err` on dispatch failure (the record is marked failed first).
- **When to use:** a bare prompt / consult run — one agent, no orchestration. Called by `swarm-cli`'s dispatcher for the foreground path.

Related: [[swarm-exec#execute_partner]], [[swarm-exec#print_partner_output]], [[swarm-contracts#JobRecord]], [[swarm-store#FileJobRepo]]

### cmd_preset

```rust
pub fn cmd_preset(raw: &[String]) -> Result<i32, String>
```

Expand a named preset into a full orchestration run. Parses `<preset-id> [--cwd/--timeout/...] <prompt>`, looks the id up in `[preset.<id>]` config first (honoring its `orchestrator`: `discussion`/`swarm`/`fanout`/`converge`/`audit`/`design`), then falls back to a small set of built-in presets (`architecture-council`, `codebase-audit`, `ui-polish`, `regression-hunt`, `api-docs-followup`).

- **Params:** `raw` — the argv tail after the `preset` verb (preset id, flags, prompt words). Profile helpers default ON unless a helper flag is present.
- **Returns:** the chosen orchestrator's exit code; `Err` on missing prompt, unknown preset, or unknown configured `orchestrator`.
- **When to use:** the `preset` verb — run a curated manager/worker/round configuration by name. Distinct from `cmd_presets` (a read-only listing command in `swarm-cli`).

Related: [[swarm-exec#run_swarm]], [[swarm-exec#run_discussion]], [[swarm-exec#run_converge]], [[swarm-kernel#SwarmConfig]]

### AgentBackend

```rust
pub trait AgentBackend: Send + Sync {
    fn id(&self) -> &str;
    fn ready(&self) -> Result<(), BackendError>;
    fn run(&self, req: &BackendRequest, sink: &mut dyn BackendSink) -> Result<RunOutcome, BackendError>;
    fn capabilities(&self) -> BackendCaps;
}
```

The process-agnostic single-attempt execution contract every backend implements. A backend reports a stable `id`, gates on `ready()` (binary located / key present), runs one attempt via `run` (building from the borrowed `BackendRequest`, streaming output through `sink.stdout_chunk`/`stderr_chunk`), and reports its `capabilities`. Failures are typed `BackendError` so the retry/fallback machine branches on cause, not on error text.

- **When to use:** implement this to add a backend that a descriptor can't express, then register it via [`BackendRegistry::register`](#backendregistry). Implementors in this crate: [`CodexBackend`](#codexbackend), [`ClaudeBackend`](#claudebackend), [`CliBackend`](#clibackend), [`OpenAiCompatibleBackend`](#openaicompatiblebackend), [`NativeBackend`](#nativebackend).

Related: [[swarm-kernel#Backend ABI]], [[swarm-exec#BackendRegistry]], [[swarm-cli#scaffold_backend]]

### CodexBackend

```rust
pub struct CodexBackend;
```

Built-in `AgentBackend` (id `"codex"`) that dispatches via `codex exec`. `ready()` locates the `codex` binary; `run` invokes `codex exec --cd <cwd> --sandbox <read-only|workspace-write> --color never --output-last-message <file> --skip-git-repo-check [--model M] -`, feeds the prompt on stdin, and returns the captured last message as stdout.

- **When to use:** never construct directly in normal flow — it is seeded into the registry under `"codex"` and `"openai"`. Selected by `--agent codex`.

Related: [[swarm-exec#AgentBackend]], [[swarm-exec#BackendRegistry]], [[swarm-kernel#Binary resolution]]

### ClaudeBackend

```rust
pub struct ClaudeBackend;
```

Built-in `AgentBackend` (id `"claude"`) that dispatches via `claude --print`. `ready()` locates the `claude` binary; `run` invokes `claude --print --output-format <json|text> --add-dir <cwd> [--model M]` (json when the sink does not want streaming, to capture token counts via [`parse_claude_json_output`](#parse_claude_json_output)), adding `--tools Read,Grep,Glob,LS` in quiet mode or `--permission-mode bypassPermissions` when bypass is allowed.

- **When to use:** seeded into the registry under `"claude"` and `"anthropic"`. Selected by `--agent claude`.

Related: [[swarm-exec#AgentBackend]], [[swarm-exec#parse_claude_json_output]], [[swarm-exec#BackendRegistry]]

### execute_partner

```rust
pub fn execute_partner(
    registry: &BackendRegistry,
    agent: AgentChoice,
    args: &Args,
    prompt: &str,
) -> Result<RunOutcome, String>
```

Resolve a backend by dispatch id (the custom-backend id if set, else the built-in's canonical name) and run a single attempt with no streaming. Thin wrapper over [`execute_partner_with_chunks`](#execute_partner_with_chunks) with no callback.

- **Returns:** the backend's `RunOutcome`, or a `String` error (registry miss → a clear "available ids" message; `ready()`/`run` failure → `Error: ...`).
- **When to use:** one-shot dispatch when you don't need a fallback chain — single foreground runs, background workers.

Related: [[swarm-exec#execute_partner_with_chunks]], [[swarm-exec#BackendRegistry]], [[swarm-kernel#Backend ABI]]

### execute_partner_with_chunks

```rust
pub fn execute_partner_with_chunks(
    registry: &BackendRegistry,
    agent: AgentChoice,
    args: &Args,
    prompt: &str,
    on_chunk: Option<ChunkCallback<'_>>,
) -> Result<RunOutcome, String>
```

Like [`execute_partner`](#execute_partner), but when `on_chunk` is `Some`, output streams live through a [`ClosureSink`] as it arrives; `None` uses a `NullSink` (the backend may then choose a non-streaming, token-capturing path).

- **When to use:** when a caller wants to surface partial output as it streams (e.g. discussion turn chunks).

Related: [[swarm-exec#ChunkCallback]], [[swarm-exec#execute_partner]], [[swarm-kernel#Backend ABI]]

### ChunkCallback

```rust
pub type ChunkCallback<'a> = &'a mut dyn FnMut(&str, &str);
```

The streaming chunk callback type: `(stream_name, chunk)` where `stream_name` is `"stdout"` or `"stderr"`. Fed to [`ClosureSink`] by the chunked execution paths.

Related: [[swarm-exec#execute_partner_with_chunks]], [[swarm-kernel#Backend ABI]]

### execute_with_fallback

```rust
pub fn execute_with_fallback(
    registry: &BackendRegistry,
    chain: &[AgentSpec],
    base_args: &Args,
    prompt: &str,
    role: &str,
    reliability: &ReliabilityConfig,
) -> FallbackOutcome
```

Non-chunked fallback execution: walk a fallback `chain` of specs, retrying each per `reliability` and advancing to the next on failure, consulting the pure `swarm_kernel::routing::next_action` state machine after each attempt. "Usable output" = ran, did not time out, exited 0, produced non-empty stdout — a missing binary, a timeout, or *any* non-zero exit (even one that printed an error to stdout) advances the chain.

- **Returns:** a [`FallbackOutcome`](#fallbackoutcome) — the spec that ultimately ran, its result, and the full attempt trail (for `worker_fallback`/`backend_retry` events).
- **When to use:** any orchestrated agent run where degradation must be tolerated and made visible. Build `chain` with `build_fallback_chain`.

Related: [[swarm-exec#FallbackOutcome]], [[swarm-exec#execute_with_fallback_chunks]], [[swarm-kernel#Fallback routing]], [[swarm-kernel#SwarmConfig]]

### execute_with_fallback_chunks

```rust
pub fn execute_with_fallback_chunks(
    registry: &BackendRegistry,
    chain: &[AgentSpec],
    base_args: &Args,
    prompt: &str,
    role: &str,
    reliability: &ReliabilityConfig,
    on_chunk: &mut dyn FnMut(&str, &str),
) -> FallbackOutcome
```

Identical fallback semantics to [`execute_with_fallback`](#execute_with_fallback), but each attempt streams output through `on_chunk`. Chunks from a failed attempt may reach the UI before the `worker_fallback` event and the next backend's chunks — acceptable, since a failed attempt produces little or no stdout.

- **When to use:** live-streaming discussion turns where degradation must still fall back.

Related: [[swarm-exec#execute_with_fallback]], [[swarm-exec#FallbackOutcome]], [[swarm-exec#ChunkCallback]]

### FallbackAttempt

```rust
pub struct FallbackAttempt {
    pub spec: AgentSpec,
    pub retries: u32,
    pub succeeded: bool,
    pub reason: Option<String>,
}
```

One attempt within a fallback chain, recorded for visible event emission. `retries` is the count spent on this backend before it resolved (0 = first try settled it); `reason` is a brief failure reason when the attempt did not produce usable output.

Related: [[swarm-exec#FallbackOutcome]], [[swarm-exec#execute_with_fallback]]

### FallbackOutcome

```rust
pub struct FallbackOutcome {
    pub used: AgentSpec,
    pub result: Result<RunOutcome, String>,
    pub attempts: Vec<FallbackAttempt>,
}

impl FallbackOutcome {
    pub fn fell_back(&self) -> bool;
}
```

The result of walking a backend fallback chain: the backend whose output is returned (`used`), that output (`result`), and the full attempt trail. `fell_back()` is true when a backend other than the chain's primary ultimately ran (i.e. more than one attempt was recorded), which drives the `worker_fallback` vs `backend_retry` event choice.

Related: [[swarm-exec#FallbackAttempt]], [[swarm-exec#execute_with_fallback]], [[swarm-kernel#AgentSpec / AgentChoice]]

### parse_claude_json_output

```rust
pub fn parse_claude_json_output(raw: &str) -> (String, Option<u64>, Option<u64>)
```

Parse the JSON envelope `claude --print --output-format json` emits (`{"type":"result","result":"<text>","usage":{"input_tokens":N,"output_tokens":M}}`) into `(text, input_tokens, output_tokens)`. On any parse failure the raw string is returned as text with `None` tokens — so error stdout (e.g. "selected model may not exist") is preserved for failure-reason reporting.

- **When to use:** internal to [`ClaudeBackend`](#claudebackend); public for reuse/testing.

Related: [[swarm-exec#ClaudeBackend]]

### print_partner_output

```rust
pub fn print_partner_output(agent: AgentChoice, args: &Args, output: RunOutcome) -> Result<i32, String>
```

Print a `RunOutcome` to stdout (and stderr unless it's a clean Codex success with content), emit a timeout diagnostic when timed out, and return the exit code (124 on timeout).

- **When to use:** the terminal step of a foreground single-agent run.

Related: [[swarm-exec#run_partner_foreground]], [[swarm-kernel#Backend ABI]]

### output_status_code

```rust
pub fn output_status_code(output: &RunOutcome) -> i32
```

The effective exit code for a `RunOutcome`: `124` if it timed out, else `exit_status` (or `1` when the process was signal-killed / has no code).

Related: [[swarm-exec#output_record_status]], [[swarm-kernel#Backend ABI]]

### output_record_status

```rust
pub fn output_record_status(output: &RunOutcome, code: i32) -> JobStatus
```

Map a `RunOutcome` + its code to a `JobStatus`: `TimedOut` if timed out, `Completed` if `code == 0`, else `Failed`.

Related: [[swarm-exec#output_status_code]], [[swarm-contracts#JobStatus / JobAgent / JobMode]]

### record_agent_observation

```rust
pub fn record_agent_observation(
    mode: &str,
    session_id: Option<&str>,
    role: &str,
    spec: &AgentSpec,
    cwd: &Path,
    prompt: &str,
    output: &RunOutcome,
    duration: Duration,
)
```

Write a learned-routing telemetry observation for a finished run (status, exit code, byte/token counts, duration) to the default file telemetry repo. Best-effort: silently no-ops if no telemetry home is resolvable. Args mirror the flat `AgentObservation` schema field-for-field.

- **When to use:** after every orchestrated agent run that produced a `RunOutcome`, so learned routing can rank agents per role.

Related: [[swarm-exec#record_agent_error]], [[swarm-kernel#Telemetry aggregation]], [[swarm-store#FileTelemetryRepo / MemTelemetryRepo (+ default_file_telemetry_repo)]]

### record_agent_error

```rust
pub fn record_agent_error(
    mode: &str,
    session_id: Option<&str>,
    role: &str,
    spec: &AgentSpec,
    cwd: &Path,
    prompt: &str,
    error: &str,
    duration: Duration,
)
```

Write a `status: "failed"` telemetry observation when a run errored before producing a `RunOutcome` (timed-out flag is derived from [`classify_error`](#classify_error)). Best-effort, same store as [`record_agent_observation`](#record_agent_observation).

Related: [[swarm-exec#record_agent_observation]], [[swarm-exec#classify_error]], [[swarm-kernel#Telemetry aggregation]]

### current_swarm_depth

```rust
pub fn current_swarm_depth() -> u32
```

Read the recursion-depth counter from the `SWARM_DEPTH` env var (`0` if unset/unparseable). Each spawned agent command inherits `SWARM_DEPTH = current + 1`, so depth guards can refuse runaway recursion.

- **When to use:** depth guards in the policy layer before launching another swarm.

Related: [[swarm-exec#AgentBackend]]

### BackendRegistry

```rust
pub struct BackendRegistry { /* private */ }

impl BackendRegistry {
    pub fn new() -> Self;
    pub fn with_builtins() -> Self;
    pub fn from_config(config: &SwarmConfig) -> Self;
    pub fn register_descriptor(&mut self, id: impl Into<String>, descriptor: BackendDescriptor);
    pub fn register(&mut self, id: impl Into<String>, backend: Box<dyn AgentBackend>);
    pub fn ids(&self) -> Vec<String>;
    pub fn resolve(&self, id: &str) -> Result<&dyn AgentBackend, String>;
}
impl Default for BackendRegistry; // == with_builtins()
```

Resolves agent backends by stable string id. No id is hardcoded in the dispatch path — adding a backend is a registry insertion, which is what lets anyone add an agent without touching engine code.

- **`new`** — empty registry.
- **`with_builtins`** — seeded with the built-ins under canonical ids and `parse_agent_choice` aliases: `codex`/`openai` → `CodexBackend`, `claude`/`anthropic` → `ClaudeBackend`.
- **`from_config`** — `with_builtins` plus every `[backend.<id>]` descriptor; a descriptor sharing a built-in id shadows it (config wins). This is the startup registry the dispatch path uses.
- **`register_descriptor`** — add/replace a descriptor-driven backend, constructing the concrete backend from `descriptor.kind` (`Cli` → `CliBackend`; `OpenAiCompatible`/`Native` → the feature backend, or an `UnavailableBackend` that errors loudly when the feature is off). Infallible.
- **`register`** — add/replace a custom `AgentBackend` trait impl — the library-embedding escape hatch for backends a descriptor can't express.
- **`ids`** — all registered ids, sorted (used by `doctor`).
- **`resolve`** — look up by id, or a clear error listing available ids.

Related: [[swarm-exec#AgentBackend]], [[swarm-kernel#BackendDescriptor / BackendKind / PromptDelivery]], [[swarm-exec#CliBackend]], [[swarm-exec#OpenAiCompatibleBackend]], [[swarm-exec#NativeBackend]]

### CliBackend

```rust
pub struct CliBackend { /* private */ }

impl CliBackend {
    pub fn new(id: impl Into<String>, descriptor: BackendDescriptor) -> Self;
}
// impl AgentBackend for CliBackend
```

A descriptor-driven `AgentBackend` that runs a `kind = cli` `BackendDescriptor` as a subprocess, reusing the same capture/stream/timeout path as the built-in CLI backends. Args support single-pass token substitution of `{prompt}` / `{model}` / `{cwd}` (literal braces in the prompt are never re-scanned); the prompt is delivered per `descriptor.prompt` (`Stdin` writes it to stdin; `Arg` appends it as a positional unless an arg already contains `{prompt}`). `ready()` requires a `command` to be set.

- **When to use:** wrap any local command (e.g. `ollama run {model}`) as an agent backend via a `[backend.<id>]` config block — no Rust code. Constructed by [`BackendRegistry::register_descriptor`](#backendregistry).

Related: [[swarm-exec#BackendRegistry]], [[swarm-kernel#BackendDescriptor / BackendKind / PromptDelivery]], [[swarm-exec#AgentBackend]]

### OpenAiCompatibleBackend

```rust
// feature = "openai"
pub struct OpenAiCompatibleBackend { /* private */ }

impl OpenAiCompatibleBackend {
    pub fn new(id: impl Into<String>, descriptor: BackendDescriptor) -> Self;
}
// impl AgentBackend for OpenAiCompatibleBackend
```

A descriptor-driven `AgentBackend` (compiled only under the `openai` feature) that talks HTTP to any OpenAI-compatible `/v1/chat/completions` endpoint. The base URL and API key come from env vars named by the descriptor (`base_url_env` defaulting to `https://api.openai.com/v1`; `api_key_env` required). A streaming sink requests `stream: true` and forwards each SSE `choices[0].delta.content` chunk as a stdout chunk; a one-shot sink requests `stream: false` and returns `choices[0].message.content` plus token usage. The key is sent only as a bearer token upstream and never appears in captured output or any error string; upstream 5xx/429 are flagged retryable, transport timeouts surface as a timed-out outcome.

- **When to use:** point the engine at a hosted OpenAI-compatible model via a `kind = openai-compatible` descriptor. Resolved through [`BackendRegistry`](#backendregistry).

Related: [[swarm-exec#BackendRegistry]], [[swarm-kernel#BackendDescriptor / BackendKind / PromptDelivery]], [[swarm-exec#AgentBackend]]

### NativeBackend

```rust
// feature = "native"
pub struct NativeBackend { /* private */ }

impl NativeBackend {
    pub fn new(id: impl Into<String>, descriptor: BackendDescriptor) -> Self;
    pub fn with_data_dir(id: impl Into<String>, descriptor: BackendDescriptor, data_dir: PathBuf) -> Self;
    pub fn from_provider(id: impl Into<String>, provider: Arc<dyn Provider>, default_model: Option<String>) -> Self;
}
// impl AgentBackend for NativeBackend
```

A descriptor-driven `AgentBackend` (compiled only under the `native` feature) that runs `swarm-manager`'s single-agent loop in process — no external CLI, no subprocess. A `kind = native` descriptor names a stored provider configuration by id; `ready()`/`run` resolve it from the provider registry (gating on `KeyStatus::Healthy`), build a provider + the built-in tool set (gated by any requested skills), drive the agent loop to a final answer, and emit it as a single stdout chunk with token usage. Secrets never reach logs, captured output, or error strings.

- **`new`** — production constructor (resolve against the default providers data dir).
- **`with_data_dir`** — resolve against an explicit registry dir (test seam).
- **`from_provider`** — inject a pre-built provider for network-free tests.

Skill/tool/error mapping: requested skills are loaded from `<home>/skills` layered under `<cwd>/.swarm/skills` (project wins); malformed/unknown skills warn on stderr (never silently dropped); manager `AgentError`/`ProviderError` map onto typed `BackendError` (rate-limit/5xx → retryable upstream, model-not-found → 404, parse/tool failures → protocol).

- **When to use:** run a model entirely in process with the bundled tools/skills, via a `kind = native` descriptor naming a registered provider.

Related: [[swarm-exec#BackendRegistry]], [[swarm-manager#Agent loop]], [[swarm-manager#ProviderRegistry + ProviderConfig]], [[swarm-manager#Skills]], [[swarm-kernel#BackendDescriptor / BackendKind / PromptDelivery]]

### assess_worker_output

```rust
pub fn assess_worker_output(exit_code: i32, timed_out: bool, stdout: &str, stderr: &str) -> WorkerEvidenceGate
```

Score a single worker's output into a [`WorkerEvidenceGate`](#workerevidencegate): count citations and evidence-gap markers, then raise flags for timeouts, non-zero exits, empty output, missing citations, evidence gaps, oversized output, missing packet sections (Findings/Risks/Steps/Blockers/Tests), missing proof-of-work (no cited `exit_code: 0`), and blocker signals. A worker is `verified` only when no flags fired.

- **When to use:** before injecting a worker's text into a manager prompt, to label it verified vs unverified and cap it accordingly.

Related: [[swarm-exec#WorkerEvidenceGate]], [[swarm-exec#build_manager_prompt]], [[swarm-exec#build_discussion_manager_prompt]]

### WorkerEvidenceGate

```rust
pub struct WorkerEvidenceGate {
    pub verified: bool,
    pub has_blockers: bool,
    pub citation_count: usize,
    pub evidence_gap_count: usize,
    pub stdout_bytes: usize,
    pub flags: Vec<String>,
}
```

The runtime evidence verdict for one worker turn, produced by [`assess_worker_output`](#assess_worker_output). `verified` is true only when `flags` is empty; `flags` carries the specific failures (e.g. `TIMED_OUT`, `NO_CITATIONS`, `EVIDENCE_GAP`, `MISSING_PACKET_SECTIONS`). Drives how aggressively a worker's text is capped and whether the manager is told to treat it as unverified.

Related: [[swarm-exec#assess_worker_output]]

### render_context_block

```rust
pub fn render_context_block(context: &serde_json::Value) -> Option<String>
```

Render a pre-gathered context JSON value (from `context_gather_json`) into a bounded plain-text block headed `--- local context (auto-gathered, cwd: <cwd>) ---` and closed by `--- end context ---`, one `<path>: <excerpt>` line per symbol. Returns `None` when there are no symbols.

- **When to use:** the orchestrators call this when auto-context injection is enabled, then append the block to worker/manager prompts.

Related: [[swarm-kernel#Process/format helpers]], [[swarm-exec#build_worker_prompt]], [[swarm-exec#build_manager_prompt]], [[swarm-kernel#SwarmConfig + sections]]

### build_worker_prompt

```rust
pub fn build_worker_prompt(task: &str, role: &str, context: Option<&str>) -> String
```

Build the per-worker fan-out prompt: a role-scoped instruction header plus the compact handoff contract and the task. When `context` is `Some`, the auto-gathered block is appended after the task (the `None` path is byte-identical to the pre-injection default).

Related: [[swarm-exec#render_context_block]], [[swarm-exec#COMPACT_HANDOFF_CONTRACT]], [[swarm-exec#run_swarm]]

### build_direct_persona_prompt

```rust
pub fn build_direct_persona_prompt(task: &str, persona: &str) -> Result<String, String>
```

Wrap a direct task in a persona/profile contract. Recognizes built-in persona families (`compact-manager`/`manager`/`metadirector`, `gemini-manager`/`large-context-manager`, `compact-worker`/`worker`/`handoff`) and otherwise resolves `persona` against the role-profile catalog. `none`/`off`/`disabled` (or empty) returns the task unchanged. Errors on an unknown persona.

- **When to use:** direct single-agent runs and the converge pipeline, to give one agent a structured output contract.

Related: [[swarm-kernel#Role profiles]], [[swarm-exec#run_converge]], [[swarm-exec#verify_metadirector_contract]]

### build_manager_prompt

```rust
pub fn build_manager_prompt(
    task: &str,
    results: &[(WorkerSpec, i32, RunOutcome)],
    context: Option<&str>,
) -> String
```

Build the fan-out manager synthesis prompt: the synthesis instruction, optional context block, then each worker's health line (via [`assess_worker_output`](#assess_worker_output) — gate verdict, blockers, citations, flags) plus its (capped) stdout/stderr. Unverified workers are capped more aggressively and flagged "treat as unverified".

Related: [[swarm-exec#assess_worker_output]], [[swarm-exec#run_swarm]], [[swarm-kernel#Args + verb arg structs]]

### build_swarm_result_artifact

```rust
pub fn build_swarm_result_artifact(results: &[(WorkerSpec, i32, RunOutcome)], manager: &RunOutcome) -> String
```

Build the `# Agent Swarm Result` markdown artifact: the manager synthesis (and stderr), then a `## Worker Outputs` section with each worker's exit/timeout and captured stdout/stderr. Written as the swarm job's result/stdout.

Related: [[swarm-exec#build_swarm_transcript]], [[swarm-exec#run_swarm]]

### build_swarm_transcript

```rust
pub fn build_swarm_transcript(task: &str, results: &[(WorkerSpec, i32, RunOutcome)], manager: &RunOutcome) -> String
```

Build the `# Agent Swarm Fan-Out` transcript markdown: the task, a `## Workers` section per worker, then the `## Manager` synthesis. Written to `transcript.md`.

Related: [[swarm-exec#build_swarm_result_artifact]], [[swarm-exec#run_swarm]]

### build_profile_helper_prompt

```rust
pub fn build_profile_helper_prompt(
    task: &str,
    discussion_context: &str,
    round: u32,
    participant: &WorkerSpec,
    helper: &profiles::ProfileHelper,
) -> String
```

Build the prompt for a one-layer helper agent assisting a discussion participant: the helper's role/purpose, the round, the compact handoff contract, the task, and the bounded discussion context. The helper returns only information that improves the parent participant's turn.

Related: [[swarm-kernel#Role profiles]], [[swarm-exec#build_discussion_turn_prompt]], [[swarm-exec#COMPACT_HANDOFF_CONTRACT]]

### build_discussion_turn_prompt

```rust
pub fn build_discussion_turn_prompt(
    task: &str,
    discussion_context: &str,
    round: u32,
    participant: &WorkerSpec,
    helper_context: &str,
) -> String
```

Build a participant's per-round discussion prompt: speak to the other agents, build on prior points, the compact handoff contract, the task, the bounded discussion context, and (when non-empty) the one-layer helper context.

Related: [[swarm-exec#build_discussion_digest]], [[swarm-exec#build_profile_helper_prompt]], [[swarm-exec#run_discussion]]

### build_discussion_digest

```rust
pub fn build_discussion_digest(task: &str, turns: &[DiscussionTurn], max_bytes: usize) -> String
```

Build the rolling `# Rolling Discussion Digest`: the task preview, a `## Current State` section for the latest round's turns, and a `## Prior Rounds` summary of earlier turns. Truncated to `max_bytes` with a marker. This digest seeds the next round's turn prompts.

Related: [[swarm-exec#DiscussionTurn]], [[swarm-exec#build_discussion_turn_prompt]], [[swarm-exec#run_discussion]]

### build_discussion_manager_prompt

```rust
pub fn build_discussion_manager_prompt(task: &str, discussion_digest: &str, turns: &[DiscussionTurn]) -> String
```

Build the discussion manager synthesis prompt: the synthesis instruction, the task, the rolling digest, then per-turn health lines (gate verdict via [`assess_worker_output`](#assess_worker_output)) with capped latest outputs.

Related: [[swarm-exec#assess_worker_output]], [[swarm-exec#build_discussion_digest]], [[swarm-exec#run_discussion]]

### capped_manager_output

```rust
pub fn capped_manager_output(text: &str) -> String
```

Trim and cap manager output to the compact synthesis byte budget (3000 bytes), appending a truncation marker if exceeded. Applied to manager output before it is persisted/printed.

Related: [[swarm-exec#build_manager_prompt]], [[swarm-exec#run_swarm]]

### build_docs_prompt

```rust
pub fn build_docs_prompt(task: &str, transcript: &str, synthesis: &str) -> String
```

Build the prompt for the API-documentation follow-up subagent: review the task, transcript, and manager synthesis, and return documentation recommendations only (no file edits).

- **When to use:** the optional docs step at the end of [`run_discussion`](#run_discussion) when docs are enabled.

Related: [[swarm-exec#run_discussion]], [[swarm-exec#COMPACT_HANDOFF_CONTRACT]]

### verify_metadirector_contract

```rust
pub fn verify_metadirector_contract(text: &str) -> Result<(), Vec<String>>
```

Deterministically verify that metadirector output contains the required sections — Source Map; Verdict or What Changed; Accepted Facts or Verification; Rejected Claims or Risks/Gaps; Next Slice; Tests (synonyms and hyphenation accepted). Returns `Ok(())` if all present, else `Err(missing_section_names)`.

- **When to use:** gate or grade a metadirector/manager response against its output contract.

Related: [[swarm-exec#build_direct_persona_prompt]], [[swarm-exec#build_manager_prompt]]

### COMPACT_HANDOFF_CONTRACT

```rust
pub use swarm_kernel::prompts::COMPACT_HANDOFF_CONTRACT;
```

Re-export of the shared compact handoff-packet contract string (Findings/Risks/Steps/Blockers/Tests, bullet caps, `NEEDS_EVIDENCE` rule). Embedded into every worker/helper/discussion/docs prompt so all agents return the same compact, gate-checkable shape.

Related: [[swarm-kernel#Prompt builders]], [[swarm-exec#build_worker_prompt]]

### preview_for_event

```rust
pub use swarm_kernel::format::preview_for_event;
```

Re-export of the kernel's bounded-preview helper (truncate text to N chars/bytes for event payloads). Re-exported here so `orchestration` and `monitor_runtime` can reference it as `crate::synthesis::preview_for_event`.

Related: [[swarm-kernel#Process/format helpers]]

### DiscussionSession

```rust
pub struct DiscussionSession {
    pub id: SessionId,
    pub dir: PathBuf,
    pub events_path: PathBuf,
    pub transcript_path: PathBuf,
    pub summary_path: PathBuf,
    pub digest_path: PathBuf,
    pub docs_path: PathBuf,
    pub layer_reports_path: PathBuf,
}

impl DiscussionSession {
    pub fn create(args: &DiscussArgs) -> Result<Self, String>;
    pub fn create_swarm(args: &SwarmArgs) -> Result<Self, String>;
    pub fn create_converge(args: &ConvergeArgs) -> Result<Self, String>;
    pub fn write_metadata(&self, args: &DiscussArgs) -> Result<(), String>;
    pub fn write_swarm_metadata(&self, args: &SwarmArgs) -> Result<(), String>;
    pub fn append_event(&self, kind: EventKind, payload: serde_json::Value) -> Result<(), String>;
    pub fn append_event_with_context(
        &self,
        kind: EventKind,
        payload: serde_json::Value,
        parent_id: Option<&str>,
        agent_id: &str,
        role: &str,
        phase: &str,
    ) -> Result<(), String>;
    pub fn append_layer_report(
        &self,
        layer: &str,
        role: &str,
        agent: &str,
        parent_role: Option<&str>,
        status: &str,
        text: &str,
    ) -> Result<(), String>;
}
```

A live session: a directory under the session store plus the canonical paths for its events log, transcript, summary, digest, api-docs, and layer-reports index. `Clone` so worker threads can each hold a handle.

- **`create`/`create_swarm`/`create_converge`** — mint a new session id, create the directory, write a `Created` event tagged with the mode, and return the handle.
- **`write_metadata`/`write_swarm_metadata`** — write `session.json` (id, pid, prompt, cwd, participants, paths, parent/slice).
- **`append_event`** — write an `agent-swarm/event/v2` envelope with default context (`auto`/`participant`/`discussion`).
- **`append_event_with_context`** — same, with explicit `parent_id`/`agent_id`/`role`/`phase`; delegates to `FileEventRepo` (acquires the event-log lock; preserves a JSON-null `parent_id` when none; compacts oversized payloads).
- **`append_layer_report`** — append a `agent-swarm/layer-report/v1` index entry + sidecar file and emit a `LayerReport` event carrying the index path.

- **When to use:** the orchestrators construct one per run and thread clones to worker threads to record progress.

Related: [[swarm-exec#DiscussionTurn]], [[swarm-exec#list_sessions]], [[swarm-store#FileEventRepo / MemEventRepo]], [[swarm-contracts#EventKind]], [[swarm-core#EventRepo (+ EventContext/StoredEvent/LayerReportSpec)]]

### DiscussionTurn

```rust
pub struct DiscussionTurn {
    pub round: u32,
    pub role: String,
    pub spec: AgentSpec,
    pub code: i32,
    pub timed_out: bool,
    pub text: String,
    pub stderr: String,
}
```

One participant turn captured during a discussion: round, role, the agent spec that ran, exit code, timeout flag, and the turn's stdout/stderr text. Fed into [`build_discussion_digest`](#build_discussion_digest) and [`build_discussion_manager_prompt`](#build_discussion_manager_prompt).

Related: [[swarm-exec#build_discussion_digest]], [[swarm-exec#run_discussion]], [[swarm-kernel#AgentSpec / AgentChoice]]

### SessionIndexRecord

```rust
pub struct SessionIndexRecord {
    pub id: String,
    pub created_at_ms: u128,
    pub status: String,
    pub prompt_preview: String,
}
```

A lightweight in-memory record derived from scanning a session directory: id, creation time, derived status string (`completed`/`running`/`lost`/`incomplete`), and a prompt preview. Returned by the listing functions.

Related: [[swarm-exec#list_sessions]], [[swarm-core#SessionRepo (+ SessionSpec/Handle/Meta/Status/Summary/Artifact/StatusDeriver/IndexRecord)]]

### list_sessions

```rust
pub fn list_sessions() -> Result<Vec<SessionIndexRecord>, String>
```

List all sessions under the default session store directory. Convenience wrapper over [`list_sessions_from_base`](#list_sessions_from_base).

Related: [[swarm-exec#list_sessions_from_base]], [[swarm-exec#SessionIndexRecord]], [[swarm-store#Store primitives]]

### list_sessions_from_base

```rust
pub fn list_sessions_from_base(base: &Path) -> Result<Vec<SessionIndexRecord>, String>
```

List sessions under an explicit `base` directory: read raw records via `FileSessionRepo::list()`, then per-record derive status from the latest event kind + pid liveness via `SessionStatusDeriver`.

- **When to use:** listing/inspecting sessions; pass an explicit base in tests.

Related: [[swarm-exec#list_sessions]], [[swarm-store#FileSessionRepo / MemSessionRepo]], [[swarm-core#ProcessLiveness / NeverAlive / AlwaysAlive]]

### run_session_preflight

```rust
pub fn run_session_preflight(
    session: &DiscussionSession,
    registry: &BackendRegistry,
    manager: &AgentSpec,
    participants: &[WorkerSpec],
) -> Result<(), String>
```

Verify every distinct backend (manager + each participant) is runnable before a session begins: built-ins must locate their CLI; custom specs must resolve in the registry and pass `ready()`. Emits `PreflightStarted` and then `PreflightCompleted` (ok) or `PreflightFailed` (with per-issue category/severity/suggested-action). Returns `Err` with a joined summary when any backend is blocking.

- **When to use:** the first step of [`run_swarm`](#run_swarm)/[`run_discussion`](#run_discussion) — fail fast with actionable diagnostics rather than mid-run.

Related: [[swarm-exec#classify_error]], [[swarm-exec#BackendRegistry]], [[swarm-exec#DiscussionSession]], [[swarm-kernel#Args + verb arg structs]]

### classified_agent_error_payload

```rust
pub fn classified_agent_error_payload(round: u32, participant: &WorkerSpec, error: &str) -> serde_json::Value
```

Build a classified error event payload for a failed participant turn: round, role, agent, error text, [`classify_error`](#classify_error) category, severity (`high` for timeouts, else `blocking`), and a suggested action.

Related: [[swarm-exec#classify_error]], [[swarm-exec#suggested_action_for_error]], [[swarm-exec#run_discussion]]

### classified_error_payload

```rust
pub fn classified_error_payload(label: &str, error: &str) -> serde_json::Value
```

Build a classified error event payload for a labeled (non-participant) failure, e.g. a `thread-panic`: label, error, category, `blocking` severity, suggested action.

Related: [[swarm-exec#classify_error]], [[swarm-exec#suggested_action_for_error]]

### classify_error

```rust
pub fn classify_error(error: &str) -> &'static str
```

Map an error string to a stable category: `"auth-or-permission"` (checked first — auth/permission/credential/keychain/login/unauthorized/api-key), `"missing-backend"` (could-not-locate / not-found), `"timeout"` (timed-out/timeout), else `"runtime"`. Substring matching is case-insensitive; the categories drive severity and suggested actions across preflight and telemetry.

Related: [[swarm-exec#suggested_action_for_error]], [[swarm-exec#record_agent_error]], [[swarm-kernel#Backend ABI]]

### suggested_action_for_error

```rust
pub fn suggested_action_for_error(error: &str) -> &'static str
```

The actionable next step for an error, keyed off [`classify_error`](#classify_error)'s category (install/expose the CLI; increase `--timeout`/split the task; authenticate the CLI interactively; or inspect stderr and rerun narrower).

Related: [[swarm-exec#classify_error]]

### start_background_job

```rust
pub fn start_background_job(args: Args, prompt: String) -> Result<i32, String>
```

Enqueue a partner run as a detached background job: resolve the agent, write the prompt + a `Queued` `JobRecord`, spawn a detached `__job-worker <id>` re-exec of the current executable, then flip the record to `Running` with the child pid. Prints the queued-job summary (status/result commands) and returns `0`.

- **When to use:** the background path of a single agent run (the `--background` flag). The detached child runs [`cmd_job_worker`](#cmd_job_worker).

Related: [[swarm-exec#cmd_job_worker]], [[swarm-contracts#JobRecord]], [[swarm-store#FileJobRepo / MemJobRepo]], [[swarm-kernel#Args + verb arg structs]]

### cmd_job_worker

```rust
pub fn cmd_job_worker(raw: &[String]) -> Result<i32, String>
```

The `__job-worker` entry point that the detached background process re-execs into: load the persisted `JobRecord`, mark it running, read the prompt, run an in-process partner via [`execute_partner`](#execute_partner), write stdout/stderr/result files, and persist terminal status (`Completed`/`Failed`/`TimedOut`). Returns the job's exit code.

- **Params:** `raw` — the argv tail; `raw[0]` is the job id (errors if absent).

Related: [[swarm-exec#start_background_job]], [[swarm-exec#execute_partner]], [[swarm-contracts#JobRecord]]

### cmd_command_worker

```rust
pub fn cmd_command_worker(raw: &[String]) -> Result<i32, String>
```

The `__command-worker` entry point: load the job record, mark it running, re-exec the current executable with the captured subcommand args, capture stdout/stderr, and persist terminal status. Returns the captured command's exit code.

- **Params:** `raw` — `[job_id, command_args...]` (errors if the id or the args are missing).
- **When to use:** background execution of a captured swarm subcommand (a job started elsewhere as a command rather than a partner run).

Related: [[swarm-exec#cmd_job_worker]], [[swarm-exec#start_background_job]], [[swarm-contracts#JobRecord]]

### cmd_monitor

```rust
pub fn cmd_monitor(raw: &[String]) -> Result<i32, String>
```

Run the monitor poll loop forever: parse monitor options (interval / RSS threshold / spike factor / stale seconds, from flags or `AGENT_SWARM_MONITOR_*` env), then sample agent processes and stale jobs/sessions each tick, emitting alerts. Never returns normally.

- **When to use:** the foreground `monitor` verb, and what [`cmd_monitor_start`](#cmd_monitor_start) launches as a sidecar.

Related: [[swarm-exec#cmd_monitor_once]], [[swarm-exec#cmd_monitor_start]], [[swarm-store#Monitor store]]

### cmd_monitor_once

```rust
pub fn cmd_monitor_once(raw: &[String]) -> Result<i32, String>
```

Run a single monitor tick (with a forced heartbeat) and return `0` — one sample of process RSS/spikes and stale jobs/sessions, emitting any alerts. The non-looping form of [`cmd_monitor`](#cmd_monitor).

Related: [[swarm-exec#cmd_monitor]], [[swarm-store#Monitor store]]

### cmd_monitor_start

```rust
pub fn cmd_monitor_start(raw: &[String]) -> Result<i32, String>
```

Spawn a detached monitor sidecar (re-exec `monitor` with the resolved options), writing its pid + options to the monitor status file and logging stdout/stderr. Honors `--replace` to terminate and replace an already-running sidecar; otherwise reports `already_running`. Prints a `agent-swarm/monitor-start/v1` JSON status.

Related: [[swarm-exec#cmd_monitor]], [[swarm-exec#cmd_monitor_status]], [[swarm-store#Monitor store]]

### cmd_monitor_status

```rust
pub fn cmd_monitor_status() -> Result<i32, String>
```

Print the monitor sidecar's running state as `agent-swarm/monitor-status/v1` JSON (running flag, pid if alive, alerts/status paths). Returns `0`.

Related: [[swarm-exec#cmd_monitor_start]], [[swarm-store#Monitor store]]

### cmd_watch

```rust
pub fn cmd_watch(raw: &[String]) -> Result<i32, String>
```

Stream store changes as NDJSON: watch the job, session, and monitor store directories (recursively) via `notify`, emitting `agent-swarm/watch-event/v1` lines for `watch_started`, `store_changed`, `watch_error`, and periodic `heartbeat` (interval set by `--heartbeat[-secs]`, default 30s, floored at 1). Never returns normally; errors only on a disconnected watcher channel.

- **When to use:** a live tail of swarm activity for an external UI/agent that consumes NDJSON.

Related: [[swarm-exec#cmd_monitor]], [[swarm-store#Store primitives]], [[swarm-store#Monitor store]]
