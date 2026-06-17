# swarm — API Docs Root Manifest

`swarm` is a command-line engine that orchestrates other coding agents. It fans a
task out to parallel workers (`fanout`), runs structured multi-role debates
(`discuss`), iterates rounds to convergence (`converge`), and lets a manager plan
or synthesize from supplied context (`metadirector`). An agent backend can be a
**CLI subprocess** (`codex`, `claude`, any wrapped command), an
**OpenAI-compatible HTTP endpoint**, or an **in-process native harness**. Backends
are wired in TOML config, not source code — adding an agent is a config block, not
a code change. All work is persisted to a file-backed store (sessions, jobs,
events, telemetry) under the swarm home directory, and the whole surface is also
exposed over MCP.

> **Start here.** This file is the always-start-here index for the whole
> workspace. Use the discovery flow below; for most tasks steps 1–3 are enough and
> you never read source.

## Discovery flow (index → concept → API → source)

1. **Index** — read this manifest for the full map: the [workspace map](#workspace-map),
   the [concept → location index](#concept--location-index), and the
   [keyword index](#keyword-index-find-code-by-intent).
2. **Concept** — find your concept in the index and follow its **path** (the
   crate doc `./<crate>.md`, then the exact source file).
3. **API** — open the crate doc for the surface: purpose, when to use, signatures,
   and `Related:` pointers in `[[crate#anchor]]` form.
4. **Related** — follow `Related` links to connected concepts across crates.
5. **Source** — read the `.rs` file only as a last resort, for implementation
   details. Every path in this manifest is a real file in `crates/`.

Cross-references use the `[[crate#anchor]]` form, e.g. `[[swarm-exec#run_swarm]]`
points at the `run_swarm` anchor in `./swarm-exec.md`. Each crate doc is the
package-level index for that crate.

---

## Workspace map

The dependency DAG flows low → high; each crate may depend only on crates to its
left. `swarm-manager` is a standalone leaf (no sibling deps); `swarm-registrar` is
an optional hook with no required dependents.

```
swarm-contracts → swarm-core → swarm-store → swarm-kernel → swarm-exec → swarm-mcp
                                                                       ↘  swarm-cli
swarm-manager   (standalone single-agent harness; pulled into swarm-exec via `native` feature, and by swarm-cli for the provider registry)
swarm-registrar (optional JSON service-registry hook; off by default)
```

| Crate | One-liner | Doc | Depends on |
| --- | --- | --- | --- |
| **swarm-contracts** | Canonical wire-contract types (serde-only, compiles standalone): ids, events, jobs, MCP descriptor, telemetry, package. | [./swarm-contracts.md](./swarm-contracts.md) | — (serde only) |
| **swarm-core** | Pure repo-trait substrate: `SessionRepo`/`JobRepo`/`EventRepo`/`LedgerRepo`/`TelemetryRepo` traits, `Cursor`, `RepoError`, `ProcessLiveness`. | [./swarm-core.md](./swarm-core.md) | swarm-contracts |
| **swarm-store** | File-backed + in-memory repo implementations, store primitives (paths, atomic writes, id minting), monitor/alert storage, OS liveness oracle. | [./swarm-store.md](./swarm-store.md) | swarm-core, swarm-contracts |
| **swarm-kernel** | Stateless leaf modules: CLI arg parsing, config, agent model, backend ABI + descriptors, fallback routing, prompts, profiles, telemetry aggregation, task classifier. | [./swarm-kernel.md](./swarm-kernel.md) | swarm-store, swarm-core, swarm-contracts |
| **swarm-exec** | The orchestration engine: `run_swarm`/`run_discussion`/`run_converge`, single-agent executor + retry/fallback, synthesis, sessions, preflight, background/monitor runtimes, backend registry + concrete backends. | [./swarm-exec.md](./swarm-exec.md) | swarm-kernel, swarm-store, swarm-core, swarm-contracts (+ optional `native`/`openai`) |
| **swarm-mcp** | MCP server layer: hand-written stdio JSON-RPC loop, dispatch, tool-descriptor schema (36 tools), manifest, overview, report assemblers; optional `rmcp` server. | [./swarm-mcp.md](./swarm-mcp.md) | swarm-exec, swarm-kernel, swarm-store, swarm-core, swarm-contracts (+ optional `rmcp`/`registry`) |
| **swarm-cli** | User-facing CLI: command dispatch table, `run()`/`run_dispatch()`, doctor, provider/skills/scaffold commands, package + routing-memory repos. Built into the `swarm` binary. | [./swarm-cli.md](./swarm-cli.md) | swarm-exec, swarm-mcp, swarm-kernel, swarm-store, swarm-core, swarm-contracts, swarm-manager |
| **swarm-manager** | Standalone single-agent harness: `Provider` trait + registry, encrypted credential vault, agent presets, agent loop, tool registry + built-in tools, skills loader. | [./swarm-manager.md](./swarm-manager.md) | — (external deps only; no sibling crates) |
| **swarm-registrar** | Optional generic self-registration into a JSON service registry at a caller-chosen path; atomic writes; off by default. | [./swarm-registrar.md](./swarm-registrar.md) | — (serde only) |

**Build & test:** `cargo build --release && cargo test`. Optional features:
`swarm-exec`/`--features native` (in-process harness via swarm-manager),
`swarm-exec`/`--features openai` (HTTP backend), `swarm-mcp`/`--features rmcp`
(typed MCP server), `swarm-mcp`/`--features registry` (service-registry hook).

---

## Concept → location index

Grouped by crate. Each entry: **concept** — `path` — one-line contract.

### swarm-contracts — wire types
- **SessionId / JobId / ProposalId / PresetId** — `crates/swarm-contracts/src/ids.rs` — string-newtype identifiers with `as_str`, `Display`, `From<String>`/`From<&str>`.
- **EventKind** — `crates/swarm-contracts/src/events.rs` — closed enum of 31 named event kinds (`Created`, `WorkerStarted`, `WorkerFallback`, `BackendRetry`, `LayerReport`, …) plus `Other(String)` escape hatch; `as_str()` is the wire token.
- **SessionEventV2** — `crates/swarm-contracts/src/events.rs` — the event-envelope struct written to `events.jsonl`.
- **JobStatus / JobAgent / JobMode** — `crates/swarm-contracts/src/jobs.rs` — job lifecycle enums; `JobMode::Consult` vs `Agent`; `JobAgent::from_agent_name`; `DEFAULT_TIMEOUT_SECS = 300`.
- **JobRecord** — `crates/swarm-contracts/src/jobs.rs` — the persisted per-job tracking record (paths, status, timing, exit code).
- **McpToolDescriptor** — `crates/swarm-contracts/src/mcp.rs` — shared MCP tool-descriptor type (name, description, JSON input schema).
- **AgentObservation / AgentFeedback / AgentProposal / AgentProposalVote** — `crates/swarm-contracts/src/telemetry.rs` — learned-routing + feedback record types.
- **LayerReportEnvelope** — `crates/swarm-contracts/src/package.rs` — structured layer-report payload envelope.

### swarm-core — repo traits + companions
- **RepoError** — `crates/swarm-core/src/error.rs` — the unified repo error enum.
- **Cursor** — `crates/swarm-core/src/cursor.rs` — opaque `u64` pagination/tail cursor for event reads.
- **ProcessLiveness / NeverAlive / AlwaysAlive** — `crates/swarm-core/src/liveness.rs` — liveness oracle trait + two test doubles (real oracle lives in swarm-store).
- **SessionRepo (+ SessionSpec/Handle/Meta/Status/Summary/Artifact/StatusDeriver/IndexRecord)** — `crates/swarm-core/src/session_repo.rs` — the session persistence contract and its companion types.
- **JobRepo / JobSpec** — `crates/swarm-core/src/job_repo.rs` — job-record persistence contract; `JobSpec` is the create request.
- **EventRepo (+ EventContext/StoredEvent/LayerReportSpec)** — `crates/swarm-core/src/event_repo.rs` — append/read contract for session events.
- **LedgerRepo (+ LedgerStatus/LedgerTask, fold_tasks, LEDGER_TASK_SCHEMA)** — `crates/swarm-core/src/ledger_repo.rs` — task-ledger contract; `fold_tasks` collapses snapshot history to current state.
- **TelemetryRepo** — `crates/swarm-core/src/telemetry_repo.rs` — observation/feedback/proposal persistence contract.

### swarm-store — store implementations + primitives
- **Store primitives** — `crates/swarm-store/src/store.rs` — `swarm_home`/`providers_dir`/`skills_dir`, `job_store_dir`/`session_store_dir`/`session_dir`, `new_job_id`/`new_session_id`, `now_ms`, `write_text_atomic`, `read_text_tail`, `validate_store_id`; tail/artifact byte caps.
- **FileSessionRepo / MemSessionRepo** — `crates/swarm-store/src/repos/session_repo.rs` — file-backed and in-memory `SessionRepo` impls.
- **FileJobRepo / MemJobRepo** — `crates/swarm-store/src/repos/job_repo.rs` — `JobRepo` impls.
- **FileEventRepo / MemEventRepo** — `crates/swarm-store/src/repos/event_repo.rs` — `EventRepo` impls (atomic appends under the event-log lock).
- **FileLedgerRepo / MemLedgerRepo (+ default_file_ledger_repo)** — `crates/swarm-store/src/repos/ledger_repo.rs` — `LedgerRepo` impls.
- **FileTelemetryRepo / MemTelemetryRepo (+ default_file_telemetry_repo)** — `crates/swarm-store/src/repos/telemetry_repo.rs` — `TelemetryRepo` impls.
- **OsProcessLiveness** — `crates/swarm-store/src/repos/mod.rs` — production `ProcessLiveness` oracle backed by OS process checks.
- **Job record helpers** — `crates/swarm-store/src/job.rs` — `create_tracking_record_in`, `list/read/write_job_record_in`.
- **Monitor store** — `crates/swarm-store/src/monitor_store.rs` — `MonitorOptions`, status/alerts paths, `write_monitor_status`/`read_monitor_pid`/`append_monitor_alert`/`alerts_json`; monitor defaults (interval/RSS/spike/stale).

### swarm-kernel — stateless leaves
- **Args + verb arg structs** — `crates/swarm-kernel/src/args.rs` — `Args`, `SwarmArgs`, `DiscussArgs`, `ConvergeArgs`, `WorkerSpec`; `parse_args` / `parse_swarm_args` / `parse_discuss_args` / `parse_converge_args` / `parse_audit_args` / `parse_design_args`; `print_help`, default participant/worker builders, `parse_agent_spec*`.
- **AgentSpec / AgentChoice** — `crates/swarm-kernel/src/agent.rs` — the agent model; `agent_name`, `describe_spec`.
- **BackendDescriptor / BackendKind / PromptDelivery** — `crates/swarm-kernel/src/backend_descriptor.rs` — declarative `[backend.<id>]` config schema (`cli` / `openai-compatible` / `native`); the no-code way to add an agent.
- **Backend ABI** — `crates/swarm-kernel/src/backend_abi.rs` — `BackendRequest`, `BackendCaps`, `BackendSink`/`NullSink`/`ClosureSink`, `RunOutcome`, `TokenUsage`, `BackendError` (typed causes), `CancelToken`, `EnvPolicy`.
- **SwarmConfig + sections** — `crates/swarm-kernel/src/config.rs` — `SwarmConfig`, `RouteConfig`, `ReliabilityConfig`, `SwarmDefaults`, `DiscussionDefaults`, `ContextConfig`, `Settings`, `PresetConfig`; `load_config`, `read_settings_at`/`write_settings_at`, `resolve_docs`.
- **Fallback routing** — `crates/swarm-kernel/src/routing.rs` — `build_fallback_chain` (config → learned → static), `next_action` (`NextAction`: retry/fallback/done), `backoff_with_jitter`.
- **Binary resolution** — `crates/swarm-kernel/src/resolver.rs` — `resolve_agent`, `agent_available`, `locate_codex`/`locate_claude`, `running_inside_codex`, `home_dir`.
- **Role profiles** — `crates/swarm-kernel/src/profiles.rs` — `AgentProfile`, `ProfileHelper`; `profile_for_role`, `profiles_json`, `automation_hooks_json`.
- **Prompt builders** — `crates/swarm-kernel/src/prompts.rs` — `build_audit_prompt`, `build_design_prompt`, `COMPACT_HANDOFF_CONTRACT`.
- **Telemetry aggregation** — `crates/swarm-kernel/src/telemetry.rs` — `AgentStats`, `record_observation`/`record_feedback`/`record_proposal*`, `aggregate_stats`, `best_agent_for_role`, `learned_candidates_for_role`, `insights_json`/`recommendation_json`/`proposals_json`.
- **Task classifier** — `crates/swarm-kernel/src/task_classifier.rs` — `classify_task` → `TaskClassification`; `DEFAULT_CLASSIFIER_PROVIDER`/`_MODEL`.
- **Conductor records** — `crates/swarm-kernel/src/conductor.rs` — `record_activity`, `handle_hook_stdin`, `CONDUCTOR_RECORD_SCHEMA`.
- **Process/format helpers** — `crates/swarm-kernel/src/process.rs`, `crates/swarm-kernel/src/format.rs`, `crates/swarm-kernel/src/context.rs` — pid signalling, status-line formatting, workspace context gathering.

### swarm-exec — the engine
- **run_swarm (fanout)** — `crates/swarm-exec/src/orchestration.rs` — parallel workers then manager synthesis; the `fanout`/`swarm` verb.
- **run_discussion (discuss/audit/design)** — `crates/swarm-exec/src/orchestration.rs` — structured multi-participant rounds + digest + optional docs follow-up.
- **run_converge** — `crates/swarm-exec/src/orchestration.rs` — iterate participant rounds toward convergence.
- **run_partner_foreground / cmd_preset** — `crates/swarm-exec/src/orchestration.rs` — single routed run; config-defined preset pipelines.
- **AgentBackend trait** — `crates/swarm-exec/src/executor.rs` — per-backend single-attempt execution (`id`/`ready`/`run`/`capabilities`); built-ins `CodexBackend`, `ClaudeBackend`.
- **Retry/fallback engine** — `crates/swarm-exec/src/executor.rs` — `execute_with_fallback(_chunks)`, `FallbackAttempt`/`FallbackOutcome`, `execute_partner`, `record_agent_observation`/`record_agent_error`, `parse_claude_json_output`, `current_swarm_depth`.
- **BackendRegistry** — `crates/swarm-exec/src/backend_registry.rs` — resolve backends by string id; seeded with built-ins + aliases, plus every `[backend.<id>]` descriptor (config shadows built-ins).
- **CliBackend / OpenAiCompatibleBackend / NativeBackend** — `crates/swarm-exec/src/cli_backend.rs`, `openai_backend.rs` (`openai` feature), `native_backend.rs` (`native` feature) — concrete descriptor-driven backends.
- **Synthesis** — `crates/swarm-exec/src/synthesis.rs` — `assess_worker_output`/`WorkerEvidenceGate` (citation/evidence gate), `build_worker_prompt`/`build_manager_prompt`/`build_direct_persona_prompt`, `build_swarm_result_artifact`/`build_swarm_transcript`, discussion digest/turn prompts, `verify_metadirector_contract`.
- **Sessions** — `crates/swarm-exec/src/session.rs` — `DiscussionSession`, `DiscussionTurn`, `list_sessions(_from_base)`.
- **Preflight** — `crates/swarm-exec/src/preflight.rs` — `run_session_preflight`, `classify_error`, `suggested_action_for_error`, classified error payloads.
- **Background runtime** — `crates/swarm-exec/src/background_runtime.rs` — `start_background_job`, `cmd_command_worker`, `cmd_job_worker`.
- **Monitor runtime** — `crates/swarm-exec/src/monitor_runtime.rs` — `cmd_monitor(_once/_start/_status)`, `cmd_watch`.

### swarm-mcp — MCP server layer
- **Tool schema** — `crates/swarm-mcp/src/mcp_schema.rs` — `mcp_tool_descriptors()` (the 36-tool table), `mcp_tools_pretty_json()` (frozen artifact serializer).
- **Stdio dispatch** — `crates/swarm-mcp/src/mcp_dispatch.rs` — `cmd_mcp()` (the stdio JSON-RPC loop), `handle_mcp_request`.
- **MCP helpers** — `crates/swarm-mcp/src/mcp_helpers.rs` — arg extractors, `mcp_result`/`mcp_error`/`mcp_tool_text_result`, `run_self_for_mcp`, `invoked_as_mcp_binary`.
- **Manifest** — `crates/swarm-mcp/src/manifest.rs` — `manifest_payload()` (`swarm.manifest/v1`), `mcp_tool_names()`.
- **Overview** — `crates/swarm-mcp/src/overview.rs` — `overview_json()` (`agent-swarm/overview/v1`).
- **Report assemblers** — `crates/swarm-mcp/src/report.rs` — `session_summary_json`, `session_artifacts_json`, `session_list_json`, `runtime_processes_json`.
- **rmcp server** — `crates/swarm-mcp/src/rmcp_server.rs` (`rmcp` feature) — `SwarmMcpServer` typed transport.
- **Registry hook** — `crates/swarm-mcp/src/registry_hook.rs` (`registry` feature) — `register_with` (opt-in self-registration).

### swarm-cli — the CLI
- **Entry points** — `crates/swarm-cli/src/lib.rs` — `run()` and `run_dispatch(args)`; the `swarm` binary is a thin shim.
- **SwarmService** — `crates/swarm-cli/src/service.rs` — top-level dispatcher: parses argv, mints the job record, handles stdin/persona, routes to background or foreground.
- **Command table** — `crates/swarm-cli/src/cli.rs` — `CliCommand` enum + `CLI_COMMANDS` token table (`status`/`result`/`fanout`/`discuss`/`converge`/`metadirector`/`audit`/`design`/`sessions`/`events`/`transcript`/`provider`/`doctor`/`skills`/`mcp`/…).
- **Read/inspect commands** — `crates/swarm-cli/src/cli_commands.rs`, `cli_read_commands.rs` — `cmd_status`/`cmd_result`/`cmd_sessions`/`cmd_cancel`/`cmd_runtime_processes`/`cmd_session_events`/`cmd_session_transcript`/`cmd_alerts`/`cmd_ledger`.
- **Doctor** — `crates/swarm-cli/src/doctor.rs` — one-pass config/backend/routing/credential health check.
- **Provider/skills commands** — `crates/swarm-cli/src/provider_commands.rs`, `skills_commands.rs` — provider registry CRUD; `skills list` + unknown-skill warnings.
- **Scaffold** — `crates/swarm-cli/src/scaffold.rs` — `scaffold_backend`/`cmd_scaffold_backend` (generate a backend trait-impl skeleton).
- **PackageRepo** — `crates/swarm-cli/src/package_repo.rs` — `PackageRepo` trait + `StaticPackageRepo` (agent-profile catalog).
- **RoutingMemoryRepo** — `crates/swarm-cli/src/routing_repo.rs` — `RoutingMemoryRepo` trait, `RoutingMemory<T>`, `RoleRecommendation`, `RECOMMENDATION_ROLES`.

### swarm-manager — single-agent harness (standalone)
- **Provider trait + types** — `crates/swarm-manager/src/provider/mod.rs`, `types.rs` — `Provider`, `ProviderType`, `create_provider`, `Message`/`ToolCall`/`LLMResponse`/`Usage`, `ProviderError`, `classify_http_error`.
- **Concrete providers** — `crates/swarm-manager/src/provider/{anthropic,openai,gemini,ollama,lmstudio,mock}.rs` (HTTP ones gated by `http` feature) — per-vendor `Provider` impls.
- **ProviderRegistry + ProviderConfig** — `crates/swarm-manager/src/provider/registry.rs` — persistent provider registry (`open`/`list`/`get`/`add`/`upsert`/`update`/`delete`/`set_models`); `KeyStatus`.
- **Credential vault** — `crates/swarm-manager/src/provider/crypto.rs` — `KeychainVault` (ChaCha20-Poly1305 at rest, OS keychain key, env-var fallback `SWARM_PROVIDER_KEY_*`); `encrypt`/`decrypt`/`is_value_encrypted`.
- **Presets** — `crates/swarm-manager/src/preset/mod.rs` — `Preset`, `PresetStore`, `AgentConfig`, `ConfigUpdate`, `PresetRequest`, `ConsumerPolicy`, `AgentOwner`.
- **Agent loop** — `crates/swarm-manager/src/agent/mod.rs` (`runtime` feature) — `Agent::process`/`run_blocking`, `AgentTurn`, `ToolInvocation`, `AgentError`.
- **Tools** — `crates/swarm-manager/src/tools/` — `Tool` trait, `ToolRegistry`; built-in `exec`/`file`/`output`/`web` tools.
- **Skills** — `crates/swarm-manager/src/skills/mod.rs` — `parse_skill`/`load_skills`, `Skill`, `SkillSet` (`compose_system_prompt`, `allowed_tools`), load/selection issue types.

### swarm-registrar — optional registry hook
- **ServiceRegistrar / JsonFileRegistrar / NoopRegistrar** — `crates/swarm-registrar/src/lib.rs` — `register`/`deregister` into a JSON `id → record` map; atomic temp-file+rename; additive merge; `ServiceRecord` (`with_tags`, flattened `extra`).

---

## Keyword index (find code by intent)

Natural-language term → concept(s) above. Use this to jump straight from "what I
want to do" to the right file.

- **"add a new agent / new backend (no code)"** → BackendDescriptor (`swarm-kernel/src/backend_descriptor.rs`); registered by BackendRegistry (`swarm-exec/src/backend_registry.rs`). See also `docs/authoring-a-backend.md`.
- **"add a backend in Rust / custom backend trait"** → AgentBackend trait (`swarm-exec/src/executor.rs`); scaffold skeleton via `scaffold_backend` (`swarm-cli/src/scaffold.rs`).
- **"run agents in parallel / fanout / swarm"** → `run_swarm` (`swarm-exec/src/orchestration.rs`), `SwarmArgs` (`swarm-kernel/src/args.rs`).
- **"multi-agent debate / discussion / rounds"** → `run_discussion` + `DiscussionSession` (`swarm-exec/src/orchestration.rs`, `session.rs`), `DiscussArgs` (`swarm-kernel/src/args.rs`).
- **"iterate to convergence"** → `run_converge` (`swarm-exec/src/orchestration.rs`), `ConvergeArgs` (`swarm-kernel/src/args.rs`).
- **"manager plans/synthesizes from context / metadirector"** → manager prompts + `verify_metadirector_contract` (`swarm-exec/src/synthesis.rs`).
- **"code review / audit / design review with a focus lens"** → `parse_audit_args`/`parse_design_args` (`swarm-kernel/src/args.rs`), `build_audit_prompt`/`build_design_prompt` (`swarm-kernel/src/prompts.rs`), `run_discussion` (`swarm-exec/src/orchestration.rs`).
- **"single routed run / bare prompt / consult"** → `run_dispatch`/`SwarmService` (`swarm-cli/src/service.rs`), `run_partner_foreground` (`swarm-exec/src/orchestration.rs`), `JobMode::Consult` (`swarm-contracts/src/jobs.rs`).
- **"retry / fallback / backoff when a backend fails"** → `build_fallback_chain`/`next_action`/`backoff_with_jitter` (`swarm-kernel/src/routing.rs`), `execute_with_fallback` (`swarm-exec/src/executor.rs`), `[reliability]` (`swarm-kernel/src/config.rs`).
- **"learned routing / pick the best agent from telemetry"** → `learned_candidates_for_role`/`best_agent_for_role`/`aggregate_stats` (`swarm-kernel/src/telemetry.rs`), `learned` override in `SwarmArgs`/`DiscussArgs` (`swarm-kernel/src/args.rs`), `[reliability].learned_routing` (`swarm-kernel/src/config.rs`).
- **"per-role route / which agent for which role"** → `RouteConfig` (`swarm-kernel/src/config.rs`), `RoutingMemory`/`RoleRecommendation` (`swarm-cli/src/routing_repo.rs`), `profile_for_role` (`swarm-kernel/src/profiles.rs`).
- **"classify a task / what roles does this need"** → `classify_task`/`TaskClassification` (`swarm-kernel/src/task_classifier.rs`).
- **"is a worker's answer trustworthy / evidence gate / citations"** → `assess_worker_output`/`WorkerEvidenceGate` (`swarm-exec/src/synthesis.rs`).
- **"parse CLI args / add a flag"** → `parse_args` and the per-verb parsers (`swarm-kernel/src/args.rs`); `print_help`.
- **"config file / TOML / settings / load_config"** → `SwarmConfig`/`load_config`/`Settings`/`read_settings_at` (`swarm-kernel/src/config.rs`).
- **"presets / named pipeline"** → `PresetConfig` (`swarm-kernel/src/config.rs`) + `cmd_preset` (`swarm-exec/src/orchestration.rs`); harness presets `Preset`/`PresetStore` (`swarm-manager/src/preset/mod.rs`).
- **"where is work stored / swarm home / store paths"** → store primitives (`swarm-store/src/store.rs`): `swarm_home`, `session_store_dir`, `job_store_dir`.
- **"atomic file write / safe persist"** → `write_text_atomic` (`swarm-store/src/store.rs`); registry atomic write (`swarm-registrar/src/lib.rs`).
- **"sessions / list runs / inspect a session"** → `SessionRepo` (`swarm-core/src/session_repo.rs`), `FileSessionRepo` (`swarm-store/src/repos/session_repo.rs`), `list_sessions` (`swarm-exec/src/session.rs`), `cmd_sessions` (`swarm-cli/src/cli_commands.rs`).
- **"events log / events.jsonl / event kinds"** → `EventKind`/`SessionEventV2` (`swarm-contracts/src/events.rs`), `EventRepo` (`swarm-core/src/event_repo.rs`), `FileEventRepo` (`swarm-store/src/repos/event_repo.rs`).
- **"transcript / what the agents said"** → `build_swarm_transcript` (`swarm-exec/src/synthesis.rs`), `cmd_session_transcript` (`swarm-cli/src/cli_commands.rs`), `session_summary_json` (`swarm-mcp/src/report.rs`).
- **"job status / result / cancel a run"** → `JobRecord`/`JobStatus` (`swarm-contracts/src/jobs.rs`), `JobRepo` (`swarm-core/src/job_repo.rs`), `cmd_status`/`cmd_result`/`cmd_cancel` (`swarm-cli/src/cli_commands.rs`).
- **"background job / run async / detach"** → `start_background_job`/`cmd_job_worker` (`swarm-exec/src/background_runtime.rs`), `JobMode::Agent`.
- **"monitor running processes / alerts / resource spike"** → monitor store (`swarm-store/src/monitor_store.rs`), `cmd_monitor*`/`cmd_watch` (`swarm-exec/src/monitor_runtime.rs`), `runtime_processes_json` (`swarm-mcp/src/report.rs`).
- **"is a process still alive / pid check"** → `ProcessLiveness` (`swarm-core/src/liveness.rs`), `OsProcessLiveness` (`swarm-store/src/repos/mod.rs`), `process_is_alive`/`terminate_pid` (`swarm-kernel/src/process.rs`).
- **"task ledger / track sub-tasks"** → `LedgerRepo`/`LedgerTask`/`fold_tasks` (`swarm-core/src/ledger_repo.rs`), `cmd_ledger` (`swarm-cli/src/cli_read_commands.rs`).
- **"MCP server / expose tools over MCP / stdio loop"** → `cmd_mcp`/`handle_mcp_request` (`swarm-mcp/src/mcp_dispatch.rs`), `mcp_tool_descriptors` (`swarm-mcp/src/mcp_schema.rs`).
- **"MCP tool list / tool schema / what tools exist"** → `mcp_tool_descriptors`/`mcp_tools_pretty_json` (`swarm-mcp/src/mcp_schema.rs`), `mcp_tool_names` (`swarm-mcp/src/manifest.rs`).
- **"manifest / overview / capability discovery"** → `manifest_payload` (`swarm-mcp/src/manifest.rs`), `overview_json` (`swarm-mcp/src/overview.rs`).
- **"LLM provider / call an API model / OpenAI/Anthropic/Gemini/Ollama"** → `Provider`/`ProviderType`/`create_provider` (`swarm-manager/src/provider/mod.rs`) and per-vendor impls (`swarm-manager/src/provider/*.rs`).
- **"OpenAI-compatible HTTP endpoint as a backend"** → `OpenAiCompatibleBackend` (`swarm-exec/src/openai_backend.rs`, `openai` feature); descriptor `kind = openai-compatible` (`swarm-kernel/src/backend_descriptor.rs`).
- **"in-process / native agent (no subprocess)"** → `NativeBackend` (`swarm-exec/src/native_backend.rs`, `native` feature) wrapping `Agent` (`swarm-manager/src/agent/mod.rs`); descriptor `kind = native`.
- **"store / manage API keys / credential vault / encryption"** → `KeychainVault` (`swarm-manager/src/provider/crypto.rs`), `ProviderRegistry` (`swarm-manager/src/provider/registry.rs`), `provider` CLI (`swarm-cli/src/provider_commands.rs`).
- **"agent tools / exec / file / web tool / tool registry"** → `Tool`/`ToolRegistry` (`swarm-manager/src/tools/`), built-ins under `swarm-manager/src/tools/{exec,file,web,output}.rs`.
- **"skills / SKILL.md / system-prompt composition / allowed-tools"** → `parse_skill`/`load_skills`/`SkillSet` (`swarm-manager/src/skills/mod.rs`), `skills` CLI (`swarm-cli/src/skills_commands.rs`), `BackendDescriptor.skills` (`swarm-kernel/src/backend_descriptor.rs`).
- **"agent profiles / personas / roles"** → `AgentProfile`/`profile_for_role`/`profiles_json` (`swarm-kernel/src/profiles.rs`), `build_direct_persona_prompt` (`swarm-exec/src/synthesis.rs`), `StaticPackageRepo` (`swarm-cli/src/package_repo.rs`).
- **"telemetry / observations / feedback / proposals / vote"** → `swarm-kernel/src/telemetry.rs`, `AgentObservation`/`AgentFeedback`/`AgentProposal` (`swarm-contracts/src/telemetry.rs`), `TelemetryRepo` (`swarm-core/src/telemetry_repo.rs`).
- **"recommendations / insights / which agent is best"** → `insights_json`/`recommendation_json`/`recommendations_from_stats` (`swarm-kernel/src/telemetry.rs`), `recommend` command (`swarm-cli/src/cli.rs`).
- **"health check / something's broken / diagnose config"** → doctor (`swarm-cli/src/doctor.rs`); preflight + error classification (`swarm-exec/src/preflight.rs`).
- **"cancel a run mid-flight / cancellation token"** → `CancelToken` (`swarm-kernel/src/backend_abi.rs`), `cmd_cancel` (`swarm-cli/src/cli_commands.rs`).
- **"timeout / how long a run can take"** → `DEFAULT_TIMEOUT_SECS` (`swarm-contracts/src/jobs.rs` / `swarm-kernel/src/args.rs`), `BackendRequest.timeout` (`swarm-kernel/src/backend_abi.rs`).
- **"locate the codex/claude binary / is the CLI installed"** → `locate_codex`/`locate_claude`/`agent_available`/`resolve_agent` (`swarm-kernel/src/resolver.rs`).
- **"backend error / why did it fail / retryable?"** → `BackendError` + `is_retryable` (`swarm-kernel/src/backend_abi.rs`), `classify_error`/`suggested_action_for_error` (`swarm-exec/src/preflight.rs`).
- **"register this process for discovery / service registry"** → `ServiceRegistrar`/`JsonFileRegistrar` (`swarm-registrar/src/lib.rs`), `register_with` (`swarm-mcp/src/registry_hook.rs`, `registry` feature).
- **"conductor / activity hook / record agent activity"** → `record_activity`/`handle_hook_stdin` (`swarm-kernel/src/conductor.rs`).
- **"auto-inject workspace context into prompts"** → `ContextConfig` (`swarm-kernel/src/config.rs`), `context_gather_json` (`swarm-kernel/src/context.rs`), `render_context_block` (`swarm-exec/src/synthesis.rs`).
- **"scaffold / generate boilerplate for a backend"** → `scaffold_backend`/`cmd_scaffold_backend` (`swarm-cli/src/scaffold.rs`).
