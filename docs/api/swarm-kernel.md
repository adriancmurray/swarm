# swarm-kernel

Stateless leaf modules for the swarm runtime: CLI argument parsing, the typed `config.toml` model, the agent/backend model, the backend ABI + declarative descriptors, the pure fallback-routing state machine, prompt builders, role profiles, telemetry aggregation, the task classifier, conductor activity records, and small process/format/context helpers.

## Overview

`swarm-kernel` owns the **pure, side-effect-light building blocks** that the orchestration engine (`swarm-exec`) composes into runs. Everything here is either a plain data type, a pure function, or a thin filesystem helper — there is no agent spawning, no networking, and no long-lived state. The split exists so the logic where reliability bugs hide (arg parsing, fallback sequencing, config precedence, telemetry scoring) is unit-testable without ever launching a subprocess.

What lives here, grouped by concern:

- **Agent model** — `AgentSpec` / `AgentChoice`: which backend (built-in or config-defined) and which model a run targets.
- **CLI surface** — `Args` and the per-verb arg structs (`SwarmArgs`, `DiscussArgs`, `ConvergeArgs`, `WorkerSpec`) plus their parsers and `print_help`.
- **Config** — the typed `SwarmConfig` view of `config.toml` and its loader, plus the `[settings]` read/write surface.
- **Backend declaration** — `BackendDescriptor` / `BackendKind` / `PromptDelivery`: the no-code way to add an agent via config.
- **Backend ABI** — `BackendRequest`, `RunOutcome`, `BackendError`, `BackendSink`, `CancelToken`, etc.: the process-agnostic execution contract that concrete backends in `swarm-exec` implement.
- **Routing** — `build_fallback_chain` / `next_action` / `backoff_with_jitter`: the pure retry-then-fallback machine.
- **Resolution** — `resolve_agent` / `agent_available` / `locate_*` / `home_dir`: binary discovery on disk.
- **Prompts** — `build_audit_prompt` / `build_design_prompt` and the `COMPACT_HANDOFF_CONTRACT`.
- **Profiles** — `AgentProfile` / `ProfileHelper` and the built-in role profile catalog.
- **Telemetry** — `AgentStats`, observation/feedback/proposal recorders + readers, aggregation, and the learned-routing read side.
- **Classifier** — `classify_task` → `TaskClassification`: a deterministic keyword router.
- **Conductor** — `record_activity` / `handle_hook_stdin`: harness-neutral live-topology records.
- **Helpers** — `process`, `format`, `context`.
- **Re-exports** — `ids`, `job_types`, `events` re-export the canonical types from `swarm-contracts` so `crate::ids::*` etc. keep working.

## Dependency position

```
swarm-contracts → swarm-core → swarm-store → swarm-kernel → swarm-exec → swarm-mcp / swarm-cli
```

`swarm-kernel` sits **above** `swarm-store` and **below** `swarm-exec`. It depends only on:

- `swarm-store` — store primitives (`swarm_home`, atomic writes, `JobRecord`).
- `swarm-core` — repo traits + companion types.
- `swarm-contracts` — wire types (re-exported through `ids`, `job_types`, `events`, and the telemetry types).
- External crates: `serde`, `serde_json`, `toml`, `sysinfo`, `libc` (unix).

Gate-2 invariant: **no external-system dependencies** (no HTTP client, no agent SDK). Anything that spawns a process or talks to a network lives in `swarm-exec` or higher.

Related: [[swarm-contracts]] · [[swarm-store]] · [[swarm-exec]]

## Concepts

Agent model (`agent.rs`):
- [`AgentSpec`](#agentspec) — a backend selection (built-in or config-defined) plus optional model.
- [`AgentChoice`](#agentchoice) — the three built-in backends: `Codex`, `Claude`, `Auto`.
- [`agent_name`](#agent_name) — stable tracking name for a built-in choice.
- [`describe_spec`](#describe_spec) — render a spec as `id` or `id:model`.

CLI arguments (`args.rs`):
- [`Args`](#args) — parsed direct-run arguments.
- [`SwarmArgs`](#swarmargs) — parsed fanout/swarm arguments.
- [`DiscussArgs`](#discussargs) — parsed discuss/audit/design arguments.
- [`ConvergeArgs`](#convergeargs) — parsed converge arguments.
- [`WorkerSpec`](#workerspec) — a role + agent spec + optional timeout.
- [`DEFAULT_TIMEOUT_SECS`](#default_timeout_secs) — built-in timeout constant.
- [`parse_args`](#parse_args) — parse direct-run argv.
- [`parse_swarm_args`](#parse_swarm_args) — parse fanout/swarm argv.
- [`parse_discuss_args`](#parse_discuss_args) — parse discuss argv.
- [`parse_converge_args`](#parse_converge_args) — parse converge argv.
- [`parse_audit_args`](#parse_audit_args) — parse audit argv (docs-ON default).
- [`parse_design_args`](#parse_design_args) — parse design argv.
- [`print_help`](#print_help) — print the usage banner.
- [`load_default_timeout`](#load_default_timeout) — read `default_timeout` from config.
- [`config_path`](#config_path) — resolve the active `config.toml` path.
- [`parse_agent_choice`](#parse_agent_choice) — parse a built-in agent name.
- [`parse_agent_spec`](#parse_agent_spec) — parse `NAME[:MODEL]` into a built-in choice + model.
- [`parse_agent_spec_struct`](#parse_agent_spec_struct) — parse into an `AgentSpec` (custom backends allowed).
- [`parse_worker_spec`](#parse_worker_spec) — parse a worker/participant spec string.
- [`default_workers`](#default_workers) / [`default_discussion_participants`](#default_discussion_participants) / [`default_audit_participants`](#default_audit_participants) / [`default_design_participants`](#default_design_participants) — built-in staffing.
- [`parse_u64_arg`](#parse_u64_arg) — small numeric-arg helper.

Config (`config.rs`):
- [`SwarmConfig`](#swarmconfig) — the typed `config.toml` view.
- [`SwarmDefaults`](#swarmdefaults) — `[swarm]` defaults.
- [`DiscussionDefaults`](#discussiondefaults) — `[discussion]` / `[design]` defaults.
- [`Settings`](#settings) — `[settings]` UI-adjustable behaviour.
- [`ContextConfig`](#contextconfig) — `[context]` auto-injection toggle.
- [`RouteConfig`](#routeconfig) — a `[routes.<role>]` entry.
- [`ReliabilityConfig`](#reliabilityconfig) — `[reliability]` retry + fallback policy.
- [`PresetConfig`](#presetconfig) — a `[preset.<id>]` pipeline.
- [`load_config`](#load_config) — read + parse the active config.
- [`read_settings_at`](#read_settings_at) / [`write_settings_at`](#write_settings_at) — `[settings]` get/set.
- [`resolve_docs`](#resolve_docs) — three-level docs-flag precedence.

Backend declaration (`backend_descriptor.rs`):
- [`BackendDescriptor`](#backenddescriptor) — declarative `[backend.<id>]` schema.
- [`BackendKind`](#backendkind) — `cli` / `openai-compatible` / `native`.
- [`PromptDelivery`](#promptdelivery) — `stdin` / `arg`.

Backend ABI (`backend_abi.rs`):
- [`BackendRequest`](#backendrequest) — full execution context (borrowed).
- [`RunOutcome`](#runoutcome) — process-agnostic run result.
- [`TokenUsage`](#tokenusage) — optional token accounting.
- [`BackendError`](#backenderror) — typed backend failure.
- [`BackendCaps`](#backendcaps) — reported backend capabilities.
- [`BackendSink`](#backendsink) — streaming output sink trait.
- [`NullSink`](#nullsink) — a discarding sink.
- [`ClosureSink`](#closuresink) — adapter from `FnMut(&str, &str)`.
- [`CancelToken`](#canceltoken) — cooperative cancellation flag.
- [`EnvPolicy`](#envpolicy) — environment inheritance policy.

Routing (`routing.rs`):
- [`build_fallback_chain`](#build_fallback_chain) — ordered backend chain for a role.
- [`next_action`](#next_action) — pure retry → fallback sequencing.
- [`NextAction`](#nextaction) — the decision: retry / fallback / done.
- [`backoff_with_jitter`](#backoff_with_jitter) — deterministic per-role jitter.

Resolution (`resolver.rs`):
- [`resolve_agent`](#resolve_agent) — collapse `Auto` to a concrete choice.
- [`agent_available`](#agent_available) — is the backend runnable?
- [`agent_invocation_available`](#agent_invocation_available) — availability as a `Result`.
- [`locate_codex`](#locate_codex) / [`locate_claude`](#locate_claude) — find the binaries.
- [`home_dir`](#home_dir) — `$HOME` / `%USERPROFILE%`.
- [`running_inside_codex`](#running_inside_codex) — detect a Codex host shell.

Prompts (`prompts.rs`):
- [`build_audit_prompt`](#build_audit_prompt) — read-only audit prompt.
- [`build_design_prompt`](#build_design_prompt) — design-review prompt.
- [`COMPACT_HANDOFF_CONTRACT`](#compact_handoff_contract) — the shared output contract.

Profiles (`profiles.rs`):
- [`AgentProfile`](#agentprofile) — a role profile.
- [`ProfileHelper`](#profilehelper) — a profile's helper agent.
- [`profiles`](#profiles) — the built-in profile catalog.
- [`profile_for_role`](#profile_for_role) / [`profile_by_id_or_role`](#profile_by_id_or_role) / [`profile_id_for_role`](#profile_id_for_role) / [`helpers_for_role`](#helpers_for_role) — lookups.
- [`profiles_json`](#profiles_json) / [`automation_hooks_json`](#automation_hooks_json) — JSON renderings.

Telemetry (`telemetry.rs`):
- [`AgentStats`](#agentstats) — aggregated per-(role, agent) stats.
- [`AgentObservation` / `AgentFeedback` / `AgentProposal` / `AgentProposalVote`](#telemetry-wire-types) — re-exported wire types.
- [`record_observation`](#record_observation) / [`record_observation_in_dir`](#record_observation_in_dir) — append a run observation.
- [`record_feedback`](#record_feedback) / [`record_feedback_in_dir`](#record_feedback_in_dir) — append routing feedback.
- [`record_proposal`](#record_proposal) / [`record_proposal_in_dir`](#record_proposal_in_dir) / [`record_proposal_vote`](#record_proposal_vote) / [`record_proposal_vote_in_dir`](#record_proposal_vote_in_dir) — append proposals/votes.
- [`read_observations_in_dir`](#read_observations_in_dir) / [`read_feedback_in_dir`](#read_feedback_in_dir) / [`read_proposals_in_dir`](#read_proposals_in_dir) / [`read_proposal_votes_in_dir`](#read_proposal_votes_in_dir) — readers.
- [`aggregate_stats`](#aggregate_stats) — observations + feedback → `AgentStats`.
- [`best_agent_for_role`](#best_agent_for_role) — pick the single best agent.
- [`learned_candidates_for_role`](#learned_candidates_for_role) — ranked candidates (learned-routing read side).
- [`recommendations_from_stats`](#recommendations_from_stats) — per-role recommendation JSON.
- [`proposal_summaries`](#proposal_summaries) — proposals + vote tallies.
- [`insights_json`](#insights_json) / [`recommendation_json`](#recommendation_json) / [`presets_json`](#presets_json) / [`proposals_json`](#proposals_json) — assembled JSON views.
- [`feedback_json`](#feedback_json) / [`proposal_json`](#proposal_json) / [`proposal_vote_json`](#proposal_vote_json) — validate-and-record handlers.

Task classifier (`task_classifier.rs`):
- [`classify_task`](#classify_task) — deterministic task → roles router.
- [`TaskClassification`](#taskclassification) / [`ClassifierInfo`](#classifierinfo) — its output.
- [`DEFAULT_CLASSIFIER_PROVIDER` / `DEFAULT_CLASSIFIER_MODEL`](#classifier-constants) — advertised provider/model.

Conductor (`conductor.rs`):
- [`record_activity`](#record_activity) — write a neutral activity record.
- [`handle_hook_stdin`](#handle_hook_stdin) — parse a Claude-hook payload, record, and decide policy.
- [`ActivityRecordResult`](#activityrecordresult) — what a write returns.
- [`CONDUCTOR_RECORD_SCHEMA`](#conductor_record_schema) — the record schema string.

Process helpers (`process.rs`):
- [`terminate_pid`](#terminate_pid) / [`force_terminate_pid`](#force_terminate_pid) — signal a process group.
- [`process_is_alive`](#process_is_alive) — liveness check.
- [`detach_background_command`](#detach_background_command) — `setsid` a child.
- [`stdin_ready`](#stdin_ready) — poll stdin readiness.
- [`exit_code`](#exit_code) — normalise an `ExitStatus` to an `i32`.
- [`pid_to_u32`](#pid_to_u32) — convert a sysinfo `Pid`.

Format helpers (`format.rs`):
- [`json_text`](#json_text) — pretty-print a JSON value.
- [`job_status_line`](#job_status_line) / [`print_job_status`](#print_job_status) — render a job row.
- [`prompt_preview`](#prompt_preview) — 72-char compacted preview.
- [`preview_for_event`](#preview_for_event) — bounded event-value preview.

Context (`context.rs`):
- [`context_gather_json`](#context_gather_json) — score and select workspace files for a query.

Re-export modules:
- [`ids`](#ids-re-exports) — `JobId`, `SessionId`, `ProposalId`, `PresetId`.
- [`job_types`](#job_types-re-exports) — `JobStatus`, `JobAgent`, `JobMode`.
- [`events`](#events-re-exports) — `EventKind`.

---

## API surface

### `AgentSpec`

```rust
pub struct AgentSpec {
    pub agent: AgentChoice,
    pub model: Option<String>,
    pub custom: Option<String>,
}

impl AgentSpec {
    pub fn builtin(agent: AgentChoice, model: Option<String>) -> Self;
    pub fn for_custom(id: impl Into<String>, model: Option<String>) -> Self;
    pub fn backend_id(&self) -> &str;
}
```

A single backend selection. **Invariant:** when `custom` is `Some`, `agent` is a placeholder (`AgentChoice::Auto`) and must not drive dispatch — consumers check `custom` first. `builtin` selects one of the three built-ins; `for_custom` selects a config-defined `[backend.<id>]` descriptor (validity is checked at dispatch by the `BackendRegistry`, not here). `backend_id` returns the dispatch id: the custom id when present, else the built-in's canonical name.

Use when you need to carry "which backend + which model" through staffing and dispatch. Derives `Debug, Clone, PartialEq, Eq`.

Related: [[swarm-kernel#agentchoice]] · [[swarm-kernel#parse_agent_spec_struct]] · [[swarm-kernel#describe_spec]] · [[swarm-exec]] (BackendRegistry)

### `AgentChoice`

```rust
pub enum AgentChoice { Codex, Claude, Auto }

impl AgentChoice {
    pub fn display_name(self) -> &'static str;  // "Codex" / "Claude" / "auto-selected agent"
    pub fn command_name(self) -> &'static str;  // "codex" / "claude" / "agent"
}
```

The three built-in backends. `Auto` means "resolve at dispatch to whichever of Claude/Codex is installed" (see [`resolve_agent`](#resolve_agent)). `Copy`-able. `display_name` is for human-facing banners; `command_name` is the binary/verb name.

Related: [[swarm-kernel#agent_name]] · [[swarm-kernel#resolve_agent]]

### `agent_name`

```rust
pub fn agent_name(agent: AgentChoice) -> &'static str;  // "codex" / "claude" / "auto"
```

Stable lowercase tracking name for a built-in choice — used as the dispatch id for built-ins and in telemetry. Distinct from `command_name` only for `Auto` (`"auto"` vs `"agent"`).

### `describe_spec`

```rust
pub fn describe_spec(spec: &AgentSpec) -> String;
```

Renders a spec as its `backend_id`, or `backend_id:model` when a model is set (e.g. `"claude:sonnet"`, `"api:gpt-5.4-mini"`). The canonical human/log rendering of a spec.

Related: [[swarm-kernel#agentspec]]

---

### `Args`

```rust
pub struct Args {
    pub prompt: String,
    pub cwd: PathBuf,
    pub timeout_secs: u64,
    pub quiet: bool,
    pub agent: AgentChoice,
    pub agent_custom: Option<String>,
    pub model: Option<String>,
    pub persona: Option<String>,
    pub background: bool,
    pub allow_bypass_permissions: bool,
}
```

Parsed arguments for a direct single-agent run (`agent-swarm [run] ...`). `agent_custom` is set when `--agent <id>` named a config-defined backend rather than a built-in; it takes precedence over `agent` at dispatch. `persona` wraps the run with a compact preprompt/profile.

Related: [[swarm-kernel#parse_args]] · [[swarm-kernel#agentchoice]]

### `SwarmArgs`

```rust
pub struct SwarmArgs {
    pub prompt: String,
    pub cwd: PathBuf,
    pub timeout_secs: u64,
    pub manager: AgentSpec,
    pub workers: Vec<WorkerSpec>,
    pub parent: Option<String>,
    pub slice: Option<String>,
    pub inject_context: Option<bool>,
    pub learned: Option<bool>,
}
```

Parsed arguments for a fanout/swarm run. `inject_context` overrides `[context].auto_inject` (`--context` / `--no-context`); `learned` overrides `[reliability].learned_routing` (`--learned` / `--no-learned`). `None` on either means "use the config key". `parent` / `slice` thread session lineage.

Related: [[swarm-kernel#parse_swarm_args]] · [[swarm-kernel#workerspec]] · [[swarm-exec]] (run_swarm)

### `DiscussArgs`

```rust
pub struct DiscussArgs {
    pub prompt: String,
    pub cwd: PathBuf,
    pub timeout_secs: u64,
    pub manager: AgentSpec,
    pub participants: Vec<WorkerSpec>,
    pub rounds: u32,
    pub parent: Option<String>,
    pub slice: Option<String>,
    pub docs: Option<bool>,
    pub docs_agent: AgentSpec,
    pub profile_helpers: bool,
    pub learned: Option<bool>,
}
```

Parsed arguments for a discuss-style run — shared by `discuss`, `audit`, and `design` (the latter two pre-build their prompt). `docs` is the opt-in API-docs follow-up worker override (`--docs` / `--no-docs`); `None` means "use `config.settings.docs_default`" (default false), while `parse_audit_args` hard-codes `Some(true)`. `profile_helpers` enables per-role helper agents.

Related: [[swarm-kernel#parse_discuss_args]] · [[swarm-kernel#parse_audit_args]] · [[swarm-kernel#parse_design_args]] · [[swarm-kernel#resolve_docs]]

### `ConvergeArgs`

```rust
pub struct ConvergeArgs {
    pub prompt: String,
    pub cwd: PathBuf,
    pub timeout_secs: u64,
    pub manager: AgentSpec,
    pub participants: Vec<WorkerSpec>,
    pub iterations: u32,
    pub profile_helpers: bool,
    pub learned: Option<bool>,
}
```

Parsed arguments for a converge run (iterate participants toward a stable result). `iterations` defaults to 3.

Related: [[swarm-kernel#parse_converge_args]]

### `WorkerSpec`

```rust
pub struct WorkerSpec {
    pub role: String,
    pub spec: AgentSpec,
    pub timeout_secs: Option<u64>,
}
```

One staffed participant: a role name, the backend spec for it, and an optional per-worker timeout override. `timeout_secs` comes from the `@timeout=N` modifier on a worker string.

Related: [[swarm-kernel#parse_worker_spec]] · [[swarm-kernel#agentspec]]

### `DEFAULT_TIMEOUT_SECS`

```rust
pub const DEFAULT_TIMEOUT_SECS: u64 = 300;
```

The built-in fallback timeout used when neither a CLI flag nor `default_timeout` in config supplies one.

### `parse_args`

```rust
pub fn parse_args<I>(raw: I) -> Result<Args, String>
where I: IntoIterator<Item = String>;
```

Parses direct-run argv into [`Args`](#args). Loads config for defaults (default agent, default timeout, `direct_persona`). Recognises `--quiet`, `--background`, `--allow-bypass-permissions`, `--persona`/`--profile`/`--no-persona`, `--agent`/`--backend`, `--model`, `--cwd`, `--timeout`, and `--` (force the next token as the prompt). On `-h`/`--help` it prints help and `exit(0)`. Returns the first error string on any malformed input (unknown option, missing value, extra positional, unknown persona). A persona that names a profile sets the default agent unless `--agent` was explicit.

Related: [[swarm-kernel#args]] · [[swarm-kernel#parse_agent_spec_struct]] · [[swarm-kernel#profiles]]

### `parse_swarm_args`

```rust
pub fn parse_swarm_args<I>(raw: I) -> Result<SwarmArgs, String>
where I: IntoIterator<Item = String>;
```

Parses fanout/swarm argv into [`SwarmArgs`](#swarmargs). Recognises `--manager`, repeated `--worker`, `--parent`, `--slice`, `--context`/`--no-context`, `--learned`/`--no-learned`, `--cwd`, `--timeout`. Falls back to `[swarm]` config defaults, then to [`default_workers`](#default_workers) when no `--worker` is given.

Related: [[swarm-kernel#swarmargs]] · [[swarm-kernel#default_workers]]

### `parse_discuss_args`

```rust
pub fn parse_discuss_args<I>(raw: I) -> Result<DiscussArgs, String>
where I: IntoIterator<Item = String>;
```

Parses discuss argv into [`DiscussArgs`](#discussargs). Recognises `--manager`, `--participant`/`--worker`, `--rounds`, `--docs`/`--api-docs`/`--no-docs`, `--docs-agent`, `--helpers`/`--no-helpers`, `--parent`, `--slice`, `--learned`/`--no-learned`, `--cwd`, `--timeout`. `rounds` defaults to `[discussion].default_rounds` or 2; participants fall back to config then [`default_discussion_participants`](#default_discussion_participants). `docs` defaults to `None` (inherit `docs_default`).

Related: [[swarm-kernel#discussargs]] · [[swarm-kernel#default_discussion_participants]]

### `parse_converge_args`

```rust
pub fn parse_converge_args<I>(raw: I) -> Result<ConvergeArgs, String>
where I: IntoIterator<Item = String>;
```

Parses converge argv into [`ConvergeArgs`](#convergeargs). Like `parse_discuss_args` minus docs, plus `--iterations` (default 3). Uses the `[discussion]` config section for manager/participant defaults.

Related: [[swarm-kernel#convergeargs]]

### `parse_audit_args`

```rust
pub fn parse_audit_args<I>(raw: I) -> Result<DiscussArgs, String>
where I: IntoIterator<Item = String>;
```

Parses audit argv into [`DiscussArgs`](#discussargs). Adds `--focus` (default `"all"`) and runs the prompt through [`build_audit_prompt`](#build_audit_prompt). **`docs` defaults to `Some(true)`** — audit ships docs-ON regardless of `config.settings.docs_default`; `--no-docs` flips it off. Participants default to [`default_audit_participants`](#default_audit_participants). `parent`/`slice` are always `None`.

Related: [[swarm-kernel#discussargs]] · [[swarm-kernel#build_audit_prompt]] · [[swarm-kernel#default_audit_participants]]

### `parse_design_args`

```rust
pub fn parse_design_args<I>(raw: I) -> Result<DiscussArgs, String>
where I: IntoIterator<Item = String>;
```

Parses design argv into [`DiscussArgs`](#discussargs). Adds `--focus` (default `"all"`) and runs the prompt through [`build_design_prompt`](#build_design_prompt). Reads the `[design]` config section, falling back to `[discussion]` for manager/rounds/docs-agent. `docs` defaults to `None`. Participants default to [`default_design_participants`](#default_design_participants).

Related: [[swarm-kernel#discussargs]] · [[swarm-kernel#build_design_prompt]] · [[swarm-kernel#default_design_participants]]

### `print_help`

```rust
pub fn print_help(default_timeout_secs: u64);
```

Prints the full multi-verb usage banner (run/swarm/fanout/discuss/audit/design/status/result/.../mcp) to stdout. `default_timeout_secs` is interpolated into the `--timeout` line. Called by the parsers on `-h`/`--help`.

### `load_default_timeout`

```rust
pub fn load_default_timeout() -> u64;
```

Line-scans the active `config.toml` for a flat `default_timeout = N` key, returning [`DEFAULT_TIMEOUT_SECS`](#default_timeout_secs) when absent or unparsable. A deliberately minimal scanner (no full TOML parse) so the two flat legacy keys load cheaply.

Related: [[swarm-kernel#config_path]]

### `config_path`

```rust
pub fn config_path(home: &Path) -> PathBuf;
```

Resolves the active `config.toml` path under `home`, preferring `.codex/skills/agent-swarm/config.toml` when [`running_inside_codex`](#running_inside_codex) and it exists, else `.claude/skills/agent-swarm/config.toml`, with legacy `gemini-partner` fallbacks. Returns the codex path as the last resort even if nothing exists.

Related: [[swarm-kernel#load_config]] · [[swarm-kernel#running_inside_codex]]

### `parse_agent_choice`

```rust
pub fn parse_agent_choice(value: &str) -> Result<AgentChoice, String>;
```

Parses a built-in agent name (case-insensitive). Accepts `codex`/`openai` → `Codex`, `claude`/`anthropic` → `Claude`, `auto` → `Auto`. Errors on anything else.

Related: [[swarm-kernel#agentchoice]]

### `parse_agent_spec`

```rust
pub fn parse_agent_spec(value: &str) -> Result<(AgentChoice, Option<String>), String>;
```

Parses `NAME[:MODEL]` into a **built-in** choice plus optional model. Errors when `NAME` is not a built-in. Used for the flat `default_agent` config key; most callers want [`parse_agent_spec_struct`](#parse_agent_spec_struct) instead.

Related: [[swarm-kernel#parse_agent_spec_struct]]

### `parse_agent_spec_struct`

```rust
pub fn parse_agent_spec_struct(value: &str) -> Result<AgentSpec, String>;
```

Parses `NAME[:MODEL]` into an [`AgentSpec`](#agentspec). A built-in name yields `AgentSpec::builtin`; any other non-empty, whitespace-free name is accepted as a config-defined backend id (`AgentSpec::for_custom`) — validity is the registry's job at dispatch. Empty or whitespace-containing names are rejected. This is the canonical spec parser for `--agent`, manager, worker, and fallback-chain strings.

Related: [[swarm-kernel#agentspec]] · [[swarm-kernel#parse_worker_spec]]

### `parse_worker_spec`

```rust
pub fn parse_worker_spec(value: &str) -> Result<WorkerSpec, String>;
```

Parses a worker/participant spec into [`WorkerSpec`](#workerspec). Accepts `ROLE=AGENT[:MODEL]` or the legacy `AGENT[:MODEL]:ROLE` shape, plus an optional `@key=value` modifier tail (only `timeout`/`timeout_secs` is recognised). Empty roles and unknown modifiers are rejected.

Related: [[swarm-kernel#workerspec]] · [[swarm-kernel#parse_agent_spec_struct]]

### `default_workers`

```rust
pub fn default_workers() -> Vec<WorkerSpec>;
```

Built-in fanout staffing: `architecture=claude:sonnet`, `implementation=codex`, `review=claude:sonnet`, filtered to backends that are actually installed via [`agent_available`](#agent_available).

Related: [[swarm-kernel#agent_available]]

### `default_discussion_participants`

```rust
pub fn default_discussion_participants() -> Vec<WorkerSpec>;
```

Built-in discuss staffing: `architecture=claude:sonnet`, `code-quality=claude:sonnet`, `implementation=codex` (availability-filtered).

### `default_audit_participants`

```rust
pub fn default_audit_participants() -> Vec<WorkerSpec>;
```

Built-in audit staffing: `architecture`, `simplify`, `hardening`, all `claude:sonnet` (availability-filtered).

### `default_design_participants`

```rust
pub fn default_design_participants() -> Vec<WorkerSpec>;
```

Built-in design staffing: `product-design=claude:sonnet`, `interaction-motion=claude:sonnet`, `frontend-implementation=codex`, `accessibility-qa=claude:sonnet` (availability-filtered).

### `parse_u64_arg`

```rust
pub fn parse_u64_arg(value: Option<&String>, label: &str) -> Result<u64, String>;
```

Small helper: parse an optional argument value into a `u64`, with `--<label>`-flavored error messages on missing/non-integer input. Used by ad-hoc verb parsing outside the main loops.

---

### `SwarmConfig`

```rust
pub struct SwarmConfig {
    pub routes: HashMap<String, RouteConfig>,
    pub swarm: SwarmDefaults,
    pub discussion: DiscussionDefaults,
    pub design: DiscussionDefaults,
    pub reliability: ReliabilityConfig,
    pub context: ContextConfig,
    pub settings: Settings,
    pub backend: BTreeMap<String, BackendDescriptor>,
    pub preset: BTreeMap<String, PresetConfig>,
}
```

The structured view of `config.toml`. Every field has `#[serde(default)]`, so a sparse or partial config never fails to load; unknown keys/sections are ignored (forward-compat). `backend` and `preset` are `BTreeMap`s for deterministic listings. Construct literals directly in tests — do not call [`load_config`](#load_config) (it reads the user's real file).

Related: [[swarm-kernel#load_config]] · [[swarm-kernel#routeconfig]] · [[swarm-kernel#reliabilityconfig]] · [[swarm-kernel#backenddescriptor]]

### `SwarmDefaults`

```rust
pub struct SwarmDefaults {
    pub default_manager: Option<String>,
    pub default_workers: Vec<String>,
}
```

The `[swarm]` section: manager and worker spec strings used by [`parse_swarm_args`](#parse_swarm_args) when CLI flags are omitted.

### `DiscussionDefaults`

```rust
pub struct DiscussionDefaults {
    pub default_rounds: Option<u32>,
    pub default_manager: Option<String>,
    pub default_participants: Vec<String>,
    pub docs_agent: Option<String>,
}
```

Backs both the `[discussion]` and `[design]` sections (the same struct). Supplies defaults for discuss/converge/audit/design parsing.

### `Settings`

```rust
pub struct Settings {
    pub docs_default: bool,
    pub direct_persona: Option<String>,
}
```

The `[settings]` section — UI-adjustable behaviour (`Serialize` + `Deserialize`, so it round-trips through [`write_settings_at`](#write_settings_at)). `docs_default` enables the API-docs follow-up worker without `--docs` per run (precedence: CLI flag > this > built-in `false`; audit is always-on). `direct_persona` is the default persona wrapper for `agent-swarm run`. Every field carries `#[serde(default)]` to stay forward-compatible.

Related: [[swarm-kernel#read_settings_at]] · [[swarm-kernel#write_settings_at]] · [[swarm-kernel#resolve_docs]]

### `ContextConfig`

```rust
pub struct ContextConfig { pub auto_inject: bool }
```

The `[context]` section. When `auto_inject` is true the fanout path gathers a bounded local context summary and prepends it to worker + manager prompts. Default `false` — opt-in only, overridable per-run via `--context`/`--no-context`.

Related: [[swarm-kernel#context_gather_json]]

### `RouteConfig`

```rust
pub struct RouteConfig { pub preferred: Vec<String> }
```

A `[routes.<role>]` entry: the per-role preferred backend order consumed by [`build_fallback_chain`](#build_fallback_chain). The advisory `mode` key is ignored.

Related: [[swarm-kernel#build_fallback_chain]]

### `ReliabilityConfig`

```rust
pub struct ReliabilityConfig {
    pub retry_attempts: u32,             // default 1
    pub retry_backoff_ms: u64,           // default 1_500
    pub fallback_chain: Vec<String>,     // default empty
    pub learned_routing: bool,           // default true
    pub learned_min_observations: u32,   // default 3
}
```

The `[reliability]` section: retry + cross-backend fallback policy. `fallback_chain` is the global backend order when a role has no `[routes.<role>]`. `learned_routing` enables the Phase-2 telemetry read-back; `learned_min_observations` is the evidence floor below which an agent is ignored. Has a hand-written `Default`.

Related: [[swarm-kernel#build_fallback_chain]] · [[swarm-kernel#next_action]] · [[swarm-kernel#learned_candidates_for_role]]

### `PresetConfig`

```rust
pub struct PresetConfig {
    pub orchestrator: String,   // "discussion" | "swarm" | "converge" | "audit" | "design"
    pub manager: Option<String>,
    pub workers: Option<Vec<String>>,
    pub participants: Option<Vec<String>>,
    pub rounds: Option<u32>,
    pub iterations: Option<u32>,
    pub focus: Option<String>,
    pub docs: Option<bool>,
}
```

A `[preset.<id>]` block — a predefined orchestration pipeline invokable via `agent-swarm preset <id>`. Fields map onto the chosen orchestrator's args.

### `load_config`

```rust
pub fn load_config() -> SwarmConfig;
```

Reads and parses the active `config.toml` (resolved via [`config_path`](#config_path)). A missing file yields defaults silently; a **malformed** file warns on stderr (no silent fallback) and yields defaults rather than aborting. Not called from tests.

Related: [[swarm-kernel#swarmconfig]] · [[swarm-kernel#config_path]]

### `read_settings_at`

```rust
pub fn read_settings_at(path: &Path) -> Settings;
```

Returns the `[settings]` section from the config at `path`. Missing file → defaults; malformed file → warn + defaults. Backs the `agent_swarm_settings_get` MCP handler.

Related: [[swarm-kernel#settings]]

### `write_settings_at`

```rust
pub fn write_settings_at(path: &Path, settings: &Settings) -> Result<(), String>;
```

Writes a new `[settings]` section into `path` using a **section-merge**: reads the file as a raw `toml::Value`, replaces only the `settings` key, and re-serializes atomically — preserving `default_timeout`, `default_agent`, `[reliability]`, `[routes]`, and any unmodeled sections. Comments inside the old `[settings]` block are lost. Takes effect on the next invocation. Backs `agent_swarm_settings_set`.

Related: [[swarm-kernel#settings]] · [[swarm-kernel#read_settings_at]] · [[swarm-store]] (write_text_atomic)

### `resolve_docs`

```rust
pub fn resolve_docs(cli: Option<bool>, config_default: bool) -> bool;
```

Three-level docs precedence: an explicit CLI flag (`Some`) wins; otherwise `config_default` (pass `config.settings.docs_default`, or `false` when no config). Audit hard-codes `Some(true)` upstream so it ignores this.

Related: [[swarm-kernel#settings]] · [[swarm-kernel#discussargs]]

---

### `BackendDescriptor`

```rust
pub struct BackendDescriptor {
    pub kind: BackendKind,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub prompt: PromptDelivery,
    pub base_url_env: Option<String>,
    pub api_key_env: Option<String>,
    pub default_model: Option<String>,
    pub provider: Option<String>,
    pub skills: Vec<String>,
}
```

A declarative `[backend.<id>]` block — the no-code way to add an agent. Fields are per-kind and unused ones stay empty without erroring, so any kind round-trips from a sparse config:

- `kind = cli` — `command` (required at run time) + `args` shape the subprocess; `{model}` / `{prompt}` tokens in `args` are substituted at run time.
- `kind = openai-compatible` — `base_url_env` / `api_key_env` name the env vars holding the endpoint and key (secrets are read from the environment only); `default_model` is the fallback model.
- `kind = native` — `provider` selects the in-process harness; `skills` names `SKILL.md` files to compose into the system prompt (gating tools to their `allowed-tools`).

`Default` is an empty `cli` descriptor. `Serialize` + `Deserialize`.

Related: [[swarm-kernel#backendkind]] · [[swarm-kernel#promptdelivery]] · [[swarm-kernel#swarmconfig]] · [[swarm-exec]] (BackendRegistry)

### `BackendKind`

```rust
pub enum BackendKind { Cli, OpenAiCompatible, Native }   // serde: kebab-case; OpenAiCompatible -> "openai-compatible"
```

The three backend execution strategies. Default is `Cli`. The explicit rename keeps `OpenAiCompatible` from serializing as `open-ai`.

### `PromptDelivery`

```rust
pub enum PromptDelivery { Stdin, Arg }   // serde: kebab-case; default Stdin
```

How a `cli` backend receives the prompt: written to the child's stdin (default) or appended as the final positional argument.

---

### `BackendRequest`

```rust
pub struct BackendRequest<'a> {
    pub prompt: &'a str,
    pub model: Option<&'a str>,
    pub cwd: &'a Path,
    pub timeout: Duration,
    pub quiet: bool,
    pub allow_bypass_permissions: bool,
    pub env_policy: EnvPolicy,
    pub cancel: CancelToken,
}
```

The full execution context handed to a backend up front (borrowed, so callers keep ownership). `allow_bypass_permissions` is a per-request policy bit (e.g. Claude's `--permission-mode bypassPermissions`); backends without such a mode ignore it. The process-agnostic successor to ad-hoc per-call parameters.

Related: [[swarm-kernel#runoutcome]] · [[swarm-kernel#canceltoken]] · [[swarm-kernel#envpolicy]] · [[swarm-exec]] (AgentBackend)

### `RunOutcome`

```rust
pub struct RunOutcome {
    pub exit_status: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
    pub retryable: bool,
    pub token_usage: Option<TokenUsage>,
}
```

A process-agnostic run result. `exit_status` is `Option<i32>` (not `ExitStatus`) so HTTP backends that never spawn a process set `None`. `Default` is empty + not-retryable. `Serialize` + `Deserialize`.

Related: [[swarm-kernel#tokenusage]] · [[swarm-kernel#backenderror]]

### `TokenUsage`

```rust
pub struct TokenUsage { pub input: Option<u64>, pub output: Option<u64> }
```

LLM token accounting when the backend reports it; both fields `None` by default.

### `BackendError`

```rust
pub enum BackendError {
    NotReady(String),
    Timeout,
    Cancelled,
    Spawn(String),
    Protocol(String),
    Upstream { status: Option<u16>, retryable: bool, detail: String },
}

impl BackendError { pub fn is_retryable(&self) -> bool; }
```

Typed backend failure — callers branch on cause, never on error text. `is_retryable` is true only for `Timeout` and `Upstream { retryable: true, .. }`. Implements `Display` and `std::error::Error`.

Related: [[swarm-kernel#runoutcome]] · [[swarm-kernel#next_action]]

### `BackendCaps`

```rust
pub struct BackendCaps { pub streaming: bool, pub cancellation: bool }
```

What a backend can do, reported by `AgentBackend::capabilities`. `Default` is `streaming: true, cancellation: false` (today's backends stream but do not yet wire cancellation). `Copy`-able.

### `BackendSink`

```rust
pub trait BackendSink {
    fn stdout_chunk(&mut self, text: &str);
    fn stderr_chunk(&mut self, text: &str);
    fn final_answer(&mut self, _text: &str) {}        // default no-op
    fn wants_streaming(&self) -> bool { true }        // default true
}
```

Streaming output sink — backends push chunks here rather than returning a monolithic blob, and may emit a structured `final_answer`. `wants_streaming` lets a backend that can run either a streaming or a one-shot path choose: a discarding sink reports `false` (take the richer one-shot path), chunk-forwarding sinks report `true`.

Related: [[swarm-kernel#nullsink]] · [[swarm-kernel#closuresink]]

### `NullSink`

```rust
pub struct NullSink;   // BackendSink: discards all chunks, wants_streaming() == false
```

A sink that discards everything and reports `wants_streaming() == false` — used when output isn't needed and the backend should take its one-shot path.

Related: [[swarm-kernel#backendsink]]

### `ClosureSink`

```rust
pub struct ClosureSink<'a> { /* … */ }
impl<'a> ClosureSink<'a> { pub fn new(inner: &'a mut dyn FnMut(&str, &str)) -> Self; }
```

Bridges a `BackendSink` to a legacy `FnMut(&str, &str)` consumer (`on_chunk("stdout"|"stderr", text)`): `stdout_chunk` calls it with `("stdout", text)`, `stderr_chunk` with `("stderr", text)`; `final_answer` stays a no-op. Reports `wants_streaming() == true`.

Related: [[swarm-kernel#backendsink]]

### `CancelToken`

```rust
pub struct CancelToken(/* Arc<AtomicBool> */);
impl CancelToken {
    pub fn new() -> Self;
    pub fn cancel(&self);
    pub fn is_cancelled(&self) -> bool;
}
```

A cheap-to-clone cooperative cancellation flag. Clones share the same underlying flag, so cancelling any clone is observed by all. `cancel` is idempotent. `Default` + `Clone`.

Related: [[swarm-kernel#backendrequest]]

### `EnvPolicy`

```rust
pub enum EnvPolicy { Inherit, Deny }   // default Inherit
```

How a backend treats the ambient process environment: `Inherit` the parent env (today's behavior) or `Deny` (run clean). `Copy`-able.

Related: [[swarm-kernel#backendrequest]]

---

### `build_fallback_chain`

```rust
pub fn build_fallback_chain(
    role: &str,
    primary: &AgentSpec,
    config: &SwarmConfig,
    learned: Option<&[String]>,
) -> Vec<AgentSpec>;
```

Builds the ordered backend chain for a worker: the caller-chosen `primary` first, then the role's `[routes.<role>].preferred`, or the global `[reliability].fallback_chain` when the role has no route. **De-duplicated by backend, not model** (falling from `claude:sonnet` to `claude:opus` is no real fallback). `learned` candidates fill the gap **only** when config is silent for the role (no route and no global chain), in the supplied order, before the static `["claude:sonnet", "codex"]` default. Pure — never touches storage; the caller decides whether learning applies and supplies the telemetry-derived list. Pass `None` for Phase-1 behavior.

Related: [[swarm-kernel#next_action]] · [[swarm-kernel#reliabilityconfig]] · [[swarm-kernel#routeconfig]] · [[swarm-kernel#learned_candidates_for_role]] · [[swarm-exec]] (execute_with_fallback)

### `next_action`

```rust
pub fn next_action(
    succeeded: bool,
    retries_used: u32,
    max_retries: u32,
    chain_pos: usize,
    chain_len: usize,
    base_backoff_ms: u64,
    role: &str,
) -> NextAction;
```

Pure retry → fallback sequencing, consulted after each attempt. Policy: **retry once on any failure, then fall to the next backend** (no transient-vs-hard classification). Returns `Done` on success or chain exhaustion, `RetrySame { backoff_ms }` while retries remain (backoff via [`backoff_with_jitter`](#backoff_with_jitter)), else `FallbackNext`.

Related: [[swarm-kernel#nextaction]] · [[swarm-kernel#build_fallback_chain]] · [[swarm-kernel#backoff_with_jitter]]

### `NextAction`

```rust
pub enum NextAction {
    RetrySame { backoff_ms: u64 },
    FallbackNext,
    Done,
}
```

The pure decision returned by [`next_action`](#next_action): retry the current backend after sleeping `backoff_ms`, move to the next backend, or stop.

### `backoff_with_jitter`

```rust
pub fn backoff_with_jitter(base_ms: u64, role: &str, attempt: u32) -> u64;
```

Deterministic per-`(role, attempt)` jitter in `[base, base + 50%]`, so workers that fail simultaneously don't retry in lockstep. Uses FNV-1a over `role` + `attempt` (dependency-free, reproducible). Returns 0 when `base_ms` is 0.

Related: [[swarm-kernel#next_action]]

---

### `resolve_agent`

```rust
pub fn resolve_agent(choice: AgentChoice) -> Result<AgentChoice, String>;
```

Collapses `Auto` to a concrete choice by probing the filesystem: Claude if present, else Codex, else an error telling the user to install one or pass `--agent`. `Codex`/`Claude` pass through unchanged.

Related: [[swarm-kernel#agentchoice]] · [[swarm-kernel#locate_claude]]

### `agent_available`

```rust
pub fn agent_available(agent: AgentChoice) -> bool;
```

Whether a built-in backend is runnable (binary located). `Auto` is available if either Claude or Codex is. Used by the default-staffing builders to drop uninstalled backends.

Related: [[swarm-kernel#agent_invocation_available]] · [[swarm-kernel#default_workers]]

### `agent_invocation_available`

```rust
pub fn agent_invocation_available(agent: AgentChoice) -> Result<(), String>;
```

Same check as [`agent_available`](#agent_available) but as a `Result` with a "could not locate the `claude`/`codex` binary" error message. `Auto` always returns `Ok`.

### `locate_codex`

```rust
pub fn locate_codex() -> Option<PathBuf>;
```

Finds the `codex` binary on `PATH`, then in common install dirs (`~/.local/bin`, `~/.npm-global/bin`, `~/.yarn/bin`, `/opt/homebrew/bin`, `/usr/local/bin`), checking the executable bit on unix.

### `locate_claude`

```rust
pub fn locate_claude() -> Option<PathBuf>;
```

Finds the `claude` binary using the same `PATH`-then-common-dirs search as [`locate_codex`](#locate_codex).

### `home_dir`

```rust
pub fn home_dir() -> Option<PathBuf>;
```

The user's home: `$HOME`, falling back to `%USERPROFILE%`. The basis for [`config_path`](#config_path) and binary discovery.

### `running_inside_codex`

```rust
pub fn running_inside_codex() -> bool;
```

Detects a Codex host shell via `CODEX_SHELL` / `CODEX_THREAD_ID` env vars or the `com.openai.codex` bundle id. Used by [`config_path`](#config_path) to prefer the codex config location.

Related: [[swarm-kernel#config_path]]

---

### `build_audit_prompt`

```rust
pub fn build_audit_prompt(task: &str, focus: &str, cwd: &Path) -> String;
```

Builds the read-only codebase-audit prompt: scopes the run ("do not edit files"), injects `focus`, the working directory, opt-in peer context, and the [`COMPACT_HANDOFF_CONTRACT`](#compact_handoff_contract) output contract. Called by [`parse_audit_args`](#parse_audit_args). Reads the optional peer registry (env-gated) at call time.

Related: [[swarm-kernel#parse_audit_args]] · [[swarm-kernel#compact_handoff_contract]]

### `build_design_prompt`

```rust
pub fn build_design_prompt(task: &str, focus: &str, cwd: &Path) -> String;
```

Builds the design-centered product-review prompt: product context (audience/use-case/tone), explicit design principles (hierarchy, density, motion, native interaction, anti-generic-AI), peer context, and the shared output contract. Called by [`parse_design_args`](#parse_design_args).

Related: [[swarm-kernel#parse_design_args]] · [[swarm-kernel#compact_handoff_contract]]

### `COMPACT_HANDOFF_CONTRACT`

```rust
pub const COMPACT_HANDOFF_CONTRACT: &str = "…";
```

The shared output contract embedded in audit/design prompts: a mandatory `<scratchpad>` falsification pass, then the fixed sections Findings / Risks / Steps / Blockers / Tests, with bullet/word caps and `NEEDS_EVIDENCE` for missing anchors. Reusable wherever a compact, citation-disciplined handoff is wanted.

---

### `AgentProfile`

```rust
pub struct AgentProfile {
    pub id: &'static str,
    pub title: &'static str,
    pub roles: &'static [&'static str],
    pub purpose: &'static str,
    pub default_agent: &'static str,
    pub helpers: &'static [ProfileHelper],
    pub automation_hooks: &'static [&'static str],
    pub deterministic_checks: &'static [&'static str],
}
```

A role profile: its canonical id/title, the role names it answers to, its purpose, its default agent spec, its helper agents, the deterministic automation hooks it may use, and the deterministic checks it must satisfy. `Serialize`. Built-ins live in [`profiles`](#profiles).

Related: [[swarm-kernel#profilehelper]] · [[swarm-kernel#profile_for_role]]

### `ProfileHelper`

```rust
pub struct ProfileHelper { pub role: &'static str, pub agent: &'static str, pub purpose: &'static str }
```

A profile's helper agent: a sub-role, the agent spec to run it, and what it contributes (e.g. context-scout, risk-check, epistemic-red-team). `Serialize`.

### `profiles`

```rust
pub fn profiles() -> Vec<AgentProfile>;
```

The built-in profile catalog: `systems-architect`, `gemini-large-context-manager`, `harness-hardener`, `frontend-polisher`, `code-simplifier`, `docs-cartographer`. The first entry is the fallback for unknown roles.

Related: [[swarm-kernel#agentprofile]]

### `profile_for_role`

```rust
pub fn profile_for_role(role: &str) -> AgentProfile;
```

Looks up a profile by id, title, or role (normalized), falling back to the first profile (`systems-architect`) when nothing matches.

Related: [[swarm-kernel#profile_by_id_or_role]]

### `profile_by_id_or_role`

```rust
pub fn profile_by_id_or_role(role: &str) -> Option<AgentProfile>;
```

Like [`profile_for_role`](#profile_for_role) but returns `None` on no match (no fallback). Used by persona resolution in [`parse_args`](#parse_args).

### `profile_id_for_role`

```rust
pub fn profile_id_for_role(role: &str) -> &'static str;
```

The id of the profile matching `role` (with fallback), for tagging telemetry/records.

### `helpers_for_role`

```rust
pub fn helpers_for_role(role: &str) -> &'static [ProfileHelper];
```

The helper agents for the profile matching `role`.

### `profiles_json`

```rust
pub fn profiles_json() -> serde_json::Value;
```

The full profile catalog as `{ "schema": "agent-swarm/profiles/v1", "profiles": [...] }`. Backs the `profiles` CLI/MCP command.

### `automation_hooks_json`

```rust
pub fn automation_hooks_json() -> serde_json::Value;
```

The deterministic host-only automation-hook catalog (`agent-swarm/automation-hooks/v1`): context-map, browser-screenshot, overflow-scan, schema-extract, test-target-suggest, reflection-pause, each with suggested profiles. The policy block asserts `llm_in_path: false, host_only: true` — hooks are deterministic host capabilities, never arbitrary agent tools.

---

### `AgentStats`

```rust
pub struct AgentStats {
    pub agent: String,
    pub role: String,
    pub runs: u64,
    pub failures: u64,
    pub timeouts: u64,
    pub feedback_wins: u64,
    pub feedback_losses: u64,
    pub avg_duration_ms: u128,
    pub avg_stdout_bytes: u128,
    pub score: f64,
}
```

Aggregated per-`(role, agent)` telemetry, including a `[0,1]` `score` blending success rate (Laplace-smoothed), a speed penalty, and a timeout penalty. Produced by [`aggregate_stats`](#aggregate_stats). `Serialize`.

Related: [[swarm-kernel#aggregate_stats]] · [[swarm-kernel#best_agent_for_role]]

### Telemetry wire types

```rust
pub use swarm_contracts::telemetry::{AgentFeedback, AgentObservation, AgentProposal, AgentProposalVote};
```

The JSONL-persisted wire records, re-exported from [[swarm-contracts]]:

- `AgentObservation` — one agent run (`observations.jsonl`): mode, role, agent, exit_code, timed_out, duration, byte/token counts. `input_tokens`/`output_tokens` are `Option` with `skip_serializing_if`.
- `AgentFeedback` — explicit routing feedback (`feedback.jsonl`): role, agent, `outcome`, weight.
- `AgentProposal` — a learning-layer proposal (`proposals.jsonl`): `id: ProposalId`, title, body, status, tags.
- `AgentProposalVote` — a vote (`proposal-votes.jsonl`): `proposal_id: ProposalId`, voter, vote, weight.

Related: [[swarm-contracts]] · [[swarm-kernel#record_observation]]

### `record_observation`

```rust
pub fn record_observation(observation: AgentObservation) -> Result<(), String>;
```

Appends an observation JSONL line to the default telemetry dir (`<swarm_home>/telemetry/observations.jsonl`), creating the dir as needed.

Related: [[swarm-kernel#record_observation_in_dir]]

### `record_observation_in_dir`

```rust
pub fn record_observation_in_dir(dir: &Path, observation: AgentObservation) -> Result<(), String>;
```

Same as [`record_observation`](#record_observation) but writes under an explicit `dir` — the hermetic form used by tests.

### `record_feedback`

```rust
pub fn record_feedback(feedback: AgentFeedback) -> Result<(), String>;
```

Appends a feedback record to the default telemetry dir.

### `record_feedback_in_dir`

```rust
pub fn record_feedback_in_dir(dir: &Path, feedback: AgentFeedback) -> Result<(), String>;
```

Explicit-dir variant of [`record_feedback`](#record_feedback).

### `record_proposal`

```rust
pub fn record_proposal(proposal: AgentProposal) -> Result<(), String>;
```

Appends a proposal to `proposals.jsonl` in the default telemetry dir. Has a `record_proposal_in_dir(dir, …)` explicit variant.

### `record_proposal_in_dir`

```rust
pub fn record_proposal_in_dir(dir: &Path, proposal: AgentProposal) -> Result<(), String>;
```

Explicit-dir variant of [`record_proposal`](#record_proposal).

### `record_proposal_vote`

```rust
pub fn record_proposal_vote(vote: AgentProposalVote) -> Result<(), String>;
```

Appends a vote to `proposal-votes.jsonl` in the default telemetry dir.

### `record_proposal_vote_in_dir`

```rust
pub fn record_proposal_vote_in_dir(dir: &Path, vote: AgentProposalVote) -> Result<(), String>;
```

Explicit-dir variant of [`record_proposal_vote`](#record_proposal_vote).

### `read_observations_in_dir`

```rust
pub fn read_observations_in_dir(dir: &Path) -> Result<Vec<AgentObservation>, String>;
```

Reads and parses `observations.jsonl` from `dir`, silently skipping unparsable lines and returning an empty vec when the file is absent.

### `read_feedback_in_dir`

```rust
pub fn read_feedback_in_dir(dir: &Path) -> Result<Vec<AgentFeedback>, String>;
```

Reads `feedback.jsonl` from `dir` (same lenient semantics).

### `read_proposals_in_dir`

```rust
pub fn read_proposals_in_dir(dir: &Path) -> Result<Vec<AgentProposal>, String>;
```

Reads `proposals.jsonl` from `dir`.

### `read_proposal_votes_in_dir`

```rust
pub fn read_proposal_votes_in_dir(dir: &Path) -> Result<Vec<AgentProposalVote>, String>;
```

Reads `proposal-votes.jsonl` from `dir`.

### `aggregate_stats`

```rust
pub fn aggregate_stats(observations: &[AgentObservation], feedback: &[AgentFeedback]) -> Vec<AgentStats>;
```

Groups observations + feedback by `(role, agent)` and computes [`AgentStats`](#agentstats), including the blended `score`. Pure — takes slices, never reads disk. The shared basis for ranking.

Related: [[swarm-kernel#agentstats]] · [[swarm-kernel#best_agent_for_role]] · [[swarm-kernel#learned_candidates_for_role]]

### `best_agent_for_role`

```rust
pub fn best_agent_for_role(role: &str, observations: &[AgentObservation], feedback: &[AgentFeedback]) -> String;
```

Picks the single best agent spec for `role` by quantized score, then more `runs`, then lexically-smallest name (deterministic). Falls back to a built-in default spec for the role when there's no data.

Related: [[swarm-kernel#aggregate_stats]] · [[swarm-kernel#recommendation_json]]

### `learned_candidates_for_role`

```rust
pub fn learned_candidates_for_role(
    role: &str,
    observations: &[AgentObservation],
    feedback: &[AgentFeedback],
    min_observations: u32,
) -> Vec<String>;
```

The **read side of learned routing**: ranked agent specs for `role` (descending score, tie-broken by runs then name), filtered to agents with at least `min_observations` runs. A cold store yields nothing, so the caller falls back to the static chain unchanged. Dispatch feeds the result into [`build_fallback_chain`](#build_fallback_chain)'s `learned` argument.

Related: [[swarm-kernel#build_fallback_chain]] · [[swarm-kernel#reliabilityconfig]]

### `recommendations_from_stats`

```rust
pub fn recommendations_from_stats(observations: &[AgentObservation], feedback: &[AgentFeedback]) -> Vec<serde_json::Value>;
```

Per-role `{ "role", "agent" }` recommendations for a fixed role set (architecture, hardening, product-design, api-docs, manager), each via [`best_agent_for_role`](#best_agent_for_role). Embedded in [`insights_json`](#insights_json).

### `proposal_summaries`

```rust
pub fn proposal_summaries(proposals: &[AgentProposal], votes: &[AgentProposalVote]) -> Vec<serde_json::Value>;
```

Renders each proposal with a body preview and tallied approve/reject/defer/total vote counts plus the latest vote timestamp.

### `insights_json`

```rust
pub fn insights_json() -> serde_json::Value;
```

Assembles the full insights view (`agent-swarm/insights/v1`): store path, observation/feedback/proposal/vote counts, aggregated `agents` stats, per-role `recommendations`, and `proposals`. Reads the default telemetry dir. Backs the `insights` command.

### `recommendation_json`

```rust
pub fn recommendation_json(task: &str) -> serde_json::Value;
```

Recommends a manager + participants for `task`: classifies it via [`classify_task`](#classify_task), then picks the best agent per classified role from telemetry. Returns `agent-swarm/recommendation/v1` with the classification and a basis block. Backs the `recommend` command.

Related: [[swarm-kernel#classify_task]] · [[swarm-kernel#best_agent_for_role]]

### `presets_json`

```rust
pub fn presets_json() -> serde_json::Value;
```

The built-in preset catalog (`agent-swarm/presets/v1`): architecture-council, codebase-audit, ui-polish, regression-hunt, api-docs-followup, each with command, manager, participants/workers, rounds, and docs flag. Backs the `presets` command.

### `proposals_json`

```rust
pub fn proposals_json() -> serde_json::Value;
```

The proposals view (`agent-swarm/proposals/v1`): store path, counts, and [`proposal_summaries`](#proposal_summaries). Reads the default telemetry dir.

### `feedback_json`

```rust
pub fn feedback_json(
    session_id: Option<String>,
    role: String,
    agent: String,
    outcome: String,
    note: Option<String>,
) -> Result<serde_json::Value, String>;
```

Validates `outcome` (normalizing synonyms to `win`/`loss`), records an `AgentFeedback`, and returns the recorded record (`agent-swarm/feedback-recorded/v1`). Backs the `feedback` command.

### `proposal_json`

```rust
pub fn proposal_json(
    session_id: Option<String>,
    title: String,
    body: String,
    proposed_by: Option<String>,
    tags: Vec<String>,
) -> Result<serde_json::Value, String>;
```

Validates non-empty title/body, mints a `ProposalId` (`proposal-<hex-ts>-<pid>`), records the `AgentProposal` (status `open`), and returns it. Backs the `propose` command.

Related: [[swarm-kernel#ids-re-exports]]

### `proposal_vote_json`

```rust
pub fn proposal_vote_json(
    proposal_id: ProposalId,
    voter: String,
    vote: String,
    rationale: Option<String>,
) -> Result<serde_json::Value, String>;
```

Validates `vote` (normalizing to `approve`/`reject`/`defer`) and non-empty id/voter, records the `AgentProposalVote`, and returns it. Backs the `proposal-vote` command.

---

### `classify_task`

```rust
pub fn classify_task(task: &str) -> TaskClassification;
```

A deterministic, dependency-free keyword router that maps a task description to a `task_type`, a confidence, and a list of roles. Categories (checked in order): `model-provider`, `docs`, `ui-design`, `audit`, else `implementation`. Short keywords match whole tokens; longer phrases match substrings (so "ui" doesn't fire inside "rebuilds"). Advertises the would-be classifier provider/model but runs the Rust fallback (`mode`/`status` = `"deterministic-rust-fallback"`).

Related: [[swarm-kernel#taskclassification]] · [[swarm-kernel#recommendation_json]]

### `TaskClassification`

```rust
pub struct TaskClassification {
    pub task_type: &'static str,
    pub confidence: u8,
    pub roles: Vec<&'static str>,
    pub classifier: ClassifierInfo,
}
```

The output of [`classify_task`](#classify_task): the chosen category, a confidence score, the roles it implies, and classifier metadata. `Serialize`.

### `ClassifierInfo`

```rust
pub struct ClassifierInfo {
    pub provider: &'static str,
    pub model: &'static str,
    pub mode: &'static str,
    pub status: &'static str,
}
```

Which classifier produced a classification — provider/model plus `mode`/`status` (currently `"deterministic-rust-fallback"`). `Serialize`.

### Classifier constants

```rust
pub const DEFAULT_CLASSIFIER_PROVIDER: &str = "mlx";
pub const DEFAULT_CLASSIFIER_MODEL: &str = "mlx-community/gemma-4-e2b-it-OptiQ-4bit";
```

The advertised default classifier provider and model (the small local model the deterministic fallback stands in for).

---

### `record_activity`

```rust
pub fn record_activity(arguments: &serde_json::Value) -> Result<ActivityRecordResult, String>;
```

Writes a single harness-neutral conductor activity record (schema [`CONDUCTOR_RECORD_SCHEMA`](#conductor_record_schema)) to `<swarm_home>/conductor-sessions/<session_id>/records.jsonl`. Pulls `session_id` (required), plus optional node/parent/depth/status/label and a bounded set of string and numeric metadata fields. Lets Codex, Gemini, app runtimes, and future agents emit the same live-topology stream Claude hooks produce. `session_id` is path-validated.

Related: [[swarm-kernel#activityrecordresult]] · [[swarm-kernel#handle_hook_stdin]] · [[swarm-store]] (swarm_home)

### `handle_hook_stdin`

```rust
pub fn handle_hook_stdin(payload_text: &str) -> Option<serde_json::Value>;
```

Parses a Claude Code hook payload (the JSON written to a hook's stdin), records the corresponding conductor record for the recognised events (`SessionStart`, `PreToolUse`/`PostToolUse` on agent/task tools, `SubagentStop`, `UserPromptSubmit`, `Stop`, `SessionEnd`), and — on a spawn `PreToolUse` — evaluates the depth-gate policy (`conductor-policy.json`), returning a `hookSpecificOutput` permission decision (`ask`/`deny`) when the would-be spawn depth exceeds `native_max_depth`. Returns `None` when there's nothing to decide. Writes a bounded debug log alongside the records.

Related: [[swarm-kernel#record_activity]] · [[swarm-kernel#conductor_record_schema]]

### `ActivityRecordResult`

```rust
pub struct ActivityRecordResult { pub session_id: String, pub node_id: String, pub path: PathBuf }
```

What a successful [`record_activity`](#record_activity) returns: the session id, the resolved node id, and the records file path written to.

### `CONDUCTOR_RECORD_SCHEMA`

```rust
pub const CONDUCTOR_RECORD_SCHEMA: &str = "agent-conductor/record/v1";
```

The schema string stamped on every conductor record.

---

### `terminate_pid`

```rust
pub fn terminate_pid(pid: u32);
```

Sends `SIGTERM` to the process **group** `-pid` (so children die too), falling back to the bare pid; on non-unix it shells out to `taskkill /T`. Fire-and-forget.

Related: [[swarm-kernel#force_terminate_pid]]

### `force_terminate_pid`

```rust
pub fn force_terminate_pid(pid: u32);
```

[`terminate_pid`](#terminate_pid) then `SIGKILL` to the group (`taskkill /T /F` on non-unix) — the escalation for a process that ignored `SIGTERM`.

### `process_is_alive`

```rust
pub fn process_is_alive(pid: u32) -> bool;
```

Liveness check: `kill -0` on unix, `tasklist` on non-unix. The kernel-level primitive behind the store's OS liveness oracle.

Related: [[swarm-store]] (OsProcessLiveness)

### `detach_background_command`

```rust
pub fn detach_background_command(command: &mut Command);
```

Configures a `std::process::Command` to start a new session via `setsid` in a `pre_exec` hook (unix), detaching the child from the parent's controlling terminal/process group. No-op on non-unix.

### `stdin_ready`

```rust
pub fn stdin_ready(timeout: Duration) -> bool;
```

Polls stdin for readability within `timeout` (unix `poll` on `POLLIN | POLLHUP`); always `false` on non-unix. Used to decide whether a piped payload is waiting.

### `exit_code`

```rust
pub fn exit_code(status: Option<ExitStatus>) -> i32;
```

Normalises an optional `ExitStatus` to an `i32`: the process code, or `128 + signal` for signal deaths (unix), or `1` when none. The canonical exit-code mapping for run records.

### `pid_to_u32`

```rust
pub fn pid_to_u32(pid: sysinfo::Pid) -> Option<u32>;
```

Converts a sysinfo `Pid` to a `u32` via its string form (sysinfo exposes no direct accessor). Used by the monitor sampler and runtime-process report.

---

### `json_text`

```rust
pub fn json_text(value: serde_json::Value) -> String;
```

Pretty-prints a JSON value (two-space indent), falling back to compact form if pretty-printing fails. The standard rendering for CLI/MCP JSON output.

### `job_status_line`

```rust
pub fn job_status_line(record: &JobRecord) -> String;
```

Formats one [`JobRecord`](swarm-store.md) as a fixed-width status row: id, status, agent, mode, pid, exit code, and prompt preview (`-` for absent pid/exit).

Related: [[swarm-store]] (JobRecord) · [[swarm-kernel#print_job_status]]

### `print_job_status`

```rust
pub fn print_job_status(record: &JobRecord);
```

Prints [`job_status_line`](#job_status_line) to stdout.

### `prompt_preview`

```rust
pub fn prompt_preview(prompt: &str) -> String;
```

Whitespace-compacts a prompt and truncates to 72 characters (ellipsised), for `prompt_preview` fields on job/session records.

### `preview_for_event`

```rust
pub fn preview_for_event(value: &str, max: usize) -> String;
```

Whitespace-compacts and truncates an arbitrary string to `max` characters (ellipsised). The general-purpose bounded preview used by event values and context excerpts.

Related: [[swarm-kernel#context_gather_json]]

---

### `context_gather_json`

```rust
pub fn context_gather_json(cwd: &Path, query: &str, budget_tokens: u64) -> Result<serde_json::Value, String>;
```

Walks the workspace under `cwd` (skipping hidden dirs and `node_modules`/`target`/`build`/`.svelte-kit`/`dist`, capped at depth 6 and 320 files), scores source/doc files by query-term matches (path matches ×4, content matches ×1, with an architecture-docs boost), sorts by score, and selects the top files within an approximate byte budget (`budget_tokens × 4`, max 24 files). Returns `agent-swarm/context-gather/v1` with the selected `symbols` (path/score/excerpt) and a `truncated` flag. Backs opt-in auto-context injection.

Related: [[swarm-kernel#contextconfig]] · [[swarm-kernel#preview_for_event]]

---

### `ids` re-exports

```rust
pub use swarm_contracts::ids::{JobId, PresetId, ProposalId, SessionId};
```

Typed identifier newtypes re-exported so `crate::ids::*` keeps working. Each wraps a `String`, serializes `#[serde(transparent)]` (bare JSON string), implements `Display` and `From<String>`/`From<&str>`, and exposes `as_str()`. The single source of truth lives in [[swarm-contracts]].

Related: [[swarm-contracts]]

### `job_types` re-exports

```rust
pub use swarm_contracts::jobs::{JobAgent, JobMode, JobStatus};
```

Typed discriminants for the three stringly fields of [`JobRecord`](swarm-store.md). Each unit variant serializes to its historical snake_case wire string; an `Other(String)` variant round-trips unknown on-disk values byte-identically (and is deliberately the only construction path — there is no `From<&str>`). Source of truth in [[swarm-contracts]].

Related: [[swarm-contracts]] · [[swarm-store]] (JobRecord)

### `events` re-exports

```rust
pub use swarm_contracts::events::EventKind;
```

The typed `agent-swarm/event/v2` event-kind identity. Serializes to a bare JSON string; known variants map to their historical snake_case strings, and `Other(String)` passes through verbatim (forward-compatible). Source of truth in [[swarm-contracts]].

Related: [[swarm-contracts]]
