# swarm-mcp

MCP server layer for the swarm runtime: a hand-written stdio JSON-RPC loop, tool
dispatch, the frozen tool-descriptor schema (36 tools), and the manifest /
overview / report JSON assemblers — plus an optional typed `rmcp` transport and an
optional service-registry hook.

## Overview

`swarm-mcp` is the crate that turns the swarm engine into an MCP server. It owns
the wire surface — what tools exist, what their JSON schemas are, how a
`tools/call` is routed, and the exact JSON shapes returned to clients (manifest,
overview, session/job/process reports). It does **not** own orchestration: most
tool handlers are thin adapters that either call already-public functions in
`swarm-exec` / `swarm-kernel` / `swarm-store`, or re-invoke the `swarm` binary
itself as a subprocess (`run_self_for_mcp`) and wrap the captured output.

The crate is built around one single source of truth: `mcp_tool_descriptors()`.
Both the live wire schema (`tools/list`) and the committed, byte-frozen
`mcp-tools.json` artifact derive from it through one serializer
(`mcp_tools_pretty_json`), and an idempotency test asserts they stay byte-identical.
A parallel hand-maintained list, `mcp_tool_names()` in `manifest`, advertises the
same names in the package manifest; a parity test asserts the two name-sets are
equal.

Three transports / extension points exist:
- the default **hand-written stdio loop** (`cmd_mcp` / `handle_mcp_request`), always built;
- an optional **typed `rmcp` server** (`SwarmMcpServer`, `rmcp` feature) reusing the same dispatch;
- an optional **service-registry hook** (`register_with`, `registry` feature), off by default and a no-op unless a registrar is supplied.

## Dependency position

```
… → swarm-kernel → swarm-exec → swarm-mcp → swarm-cli
```

`swarm-mcp` sits one layer above `swarm-exec`. It depends on `swarm-exec`,
`swarm-kernel`, `swarm-store`, `swarm-core`, and `swarm-contracts` (plus
`serde`/`serde_json`/`sysinfo`), and is consumed by `swarm-cli`. The MCP layer
holds **no external-system dependencies** in its default build.

Optional features:
- `rmcp` — pulls `rmcp 1.7` and compiles `rmcp_server` (typed MCP transport over stdio).
- `registry` — pulls the optional `swarm-registrar` crate and compiles `registry_hook`; even then, registration is opt-in (a `None` registrar is a no-op).

`McpToolDescriptor` (the descriptor type itself) is owned upstream by
`swarm-contracts` — see [[swarm-contracts#McpToolDescriptor]] — so this crate only
populates and serializes it.

## Concepts

Public items exported from `lib.rs` (and the public functions of each module). Each
links to its API entry below.

**`mcp_schema` — tool descriptor table & serializers**
- [mcp_tool_descriptors](#mcp_tool_descriptors) — the declarative 36-tool descriptor table; single source of truth.
- [mcp_tools_pretty_json](#mcp_tools_pretty_json) — the one serializer (pretty JSON + trailing newline) shared by the frozen artifact and its idempotency test.

**`mcp_dispatch` — stdio loop & tool routing**
- [cmd_mcp](#cmd_mcp) — the blocking stdio JSON-RPC read/dispatch/write loop.
- [handle_mcp_request](#handle_mcp_request) — route one parsed JSON-RPC request (`initialize`/`ping`/`tools/list`/`tools/call`).

**`mcp_helpers` — argument extractors & JSON-RPC envelopes**
- [McpToolOutput](#mcptooloutput) — the `{ text, is_error }` result of a tool handler.
- [invoked_as_mcp_binary](#invoked_as_mcp_binary) — detect whether argv[0] is the MCP entrypoint name.
- [required_arg](#required_arg) — extract a non-blank required string argument.
- [optional_string_arg](#optional_string_arg) / [optional_bool_arg](#optional_bool_arg) / [optional_u64_arg](#optional_u64_arg) / [optional_string_array_arg](#optional_string_array_arg) — typed optional argument extractors.
- [mcp_result](#mcp_result) / [mcp_error](#mcp_error) / [mcp_tool_text_result](#mcp_tool_text_result) — JSON-RPC envelope builders.
- [push_common_mcp_cli_args](#push_common_mcp_cli_args) — translate common MCP args into CLI flags.
- [run_self_for_mcp](#run_self_for_mcp) — re-invoke the swarm binary and capture stdout/stderr into an `McpToolOutput`.

**`manifest` — package manifest**
- [manifest_payload](#manifest_payload) — build the `swarm.manifest/v1` package manifest.
- [mcp_tool_names](#mcp_tool_names) — the flat tool-name list advertised in the manifest (parity-checked against the schema).

**`overview` — aggregated activity digest**
- [overview_json](#overview_json) — assemble the `agent-swarm/overview/v1` one-call digest (sessions + jobs + alerts).

**`report` — session/job/process JSON assemblers**
- [session_summary_json](#session_summary_json) — bounded status + digest + artifact pointers for one session.
- [session_artifacts_json](#session_artifacts_json) — filesystem artifact paths for one session.
- [session_list_json](#session_list_json) — the recent-sessions list.
- [runtime_processes_json](#runtime_processes_json) — tracked live/lost session & job processes (plus untracked system scan).

**Optional features**
- [SwarmMcpServer](#swarmmcpserver) (`rmcp`) — typed `rmcp` server handler reusing the same dispatch.
- [serve_stdio](#serve_stdio) (`rmcp`) — serve the `rmcp` transport over stdio.
- [register_with](#register_with) (`registry`) — opt-in self-registration into a JSON service registry.

**Binary**
- [gen_mcp_tools](#gen_mcp_tools-binary) — regenerate the frozen `mcp-tools.json` artifact.

---

## API surface

### mcp_tool_descriptors

```rust
pub fn mcp_tool_descriptors() -> Vec<McpToolDescriptor>
```

The declarative descriptor table — the single source of truth for every MCP tool.
Returns one `McpToolDescriptor` (`name`, `description`, `input_schema`) per tool;
the table currently holds **36 tools** under the `agent_swarm_*` namespace
(manifest/insights/profiles/presets, run/swarm/fanout/discuss/audit/design and
their `_start` background variants, job and session inspection, monitor/alerts,
context-gather, overview, and settings get/set).

- **Returns:** the full descriptor vector, in insertion order.
- **When to use:** read the catalog of tools and their JSON input schemas, or as the upstream feed for any wire serialization (`tools/list`, the frozen artifact, the `rmcp` `Tool` list). Helper schema builders (`empty_mcp_schema`, `session_schema`, `common_run_schema`, …) are private to the module; callers consume the assembled descriptors only.
- **Contract note:** adding/removing a tool requires updating **both** this table and [mcp_tool_names](#mcp_tool_names); a parity test fails otherwise.

Related: [[swarm-contracts#McpToolDescriptor]], [mcp_tools_pretty_json](#mcp_tools_pretty_json), [mcp_tool_names](#mcp_tool_names), [handle_mcp_request](#handle_mcp_request).

### mcp_tools_pretty_json

```rust
pub fn mcp_tools_pretty_json() -> String
```

Serializes the descriptor table to pretty-printed JSON (2-space indent, insertion
key order) with a single trailing `\n` appended. This is the **single
serialization path** shared by the `gen_mcp_tools` binary (which writes the frozen
`mcp-tools.json`) and the idempotency test that diffs the committed file against a
fresh render — using one function guarantees byte-identity.

- **Returns:** the canonical JSON text for the whole tool table.
- **When to use:** generate or validate the frozen `mcp-tools.json` artifact; do not re-implement the formatting (`serde_json::to_string_pretty` omits the trailing newline, which this function adds deliberately).

Related: [mcp_tool_descriptors](#mcp_tool_descriptors), [gen_mcp_tools](#gen_mcp_tools-binary).

### cmd_mcp

```rust
pub fn cmd_mcp() -> Result<i32, String>
```

The hand-written MCP server loop. Reads line-delimited JSON-RPC requests from
stdin; for each non-empty line it parses the JSON, dispatches via
[handle_mcp_request](#handle_mcp_request), and writes the response (when one is
produced) as a single line to stdout, flushing after each. Malformed JSON yields a
`-32700` parse-error response. Returns when stdin closes.

- **Returns:** `Ok(0)` on clean EOF, or `Err(String)` on an I/O error reading/writing/serializing.
- **When to use:** this is the process entrypoint behind `agent-swarm mcp` (the default stdio transport). Call it from the CLI dispatcher; you generally do not call it from library code.

Related: [handle_mcp_request](#handle_mcp_request), [serve_stdio](#serve_stdio).

### handle_mcp_request

```rust
pub fn handle_mcp_request(request: serde_json::Value) -> Option<serde_json::Value>
```

Routes one parsed JSON-RPC request and returns its response. Handles
`initialize` (returns `protocolVersion` `2024-11-05`, tools capability, and
`serverInfo` `agent-swarm`/`CARGO_PKG_VERSION`), `ping` (empty result),
`tools/list` (the serialized descriptor table), and `tools/call` (extracts
`params.name` + `params.arguments`, delegates to the internal tool router, and
wraps the outcome as a tool text result with an `isError` flag). Unknown methods
return `-32601`; a missing `name` on `tools/call` returns `-32602`; a missing
`method` returns `-32600`.

- **Params:** `request` — a parsed JSON-RPC request object.
- **Returns:** `Some(response_value)` for any request carrying an `id`; **`None`** for notifications (requests with no `id`), so the loop emits nothing for them.
- **When to use:** to dispatch a single request without owning the stdin loop — e.g. tests, an alternate transport, or embedding. The `rmcp` server reuses the same underlying tool router rather than this function directly.

Related: [cmd_mcp](#cmd_mcp), [mcp_tool_descriptors](#mcp_tool_descriptors), [mcp_tool_text_result](#mcp_tool_text_result), [SwarmMcpServer](#swarmmcpserver).

### McpToolOutput

```rust
pub struct McpToolOutput {
    pub text: String,
    pub is_error: bool,
}
```

The result type every tool handler returns: a text payload plus an error flag.
`text` becomes the MCP `content[0].text`; `is_error` becomes `isError` in the
`tools/call` result envelope.

- **When to use:** the return shape for any handler routed through the tool dispatcher; constructed directly for handlers that produce JSON in-process, or via [run_self_for_mcp](#run_self_for_mcp) for handlers that shell out.

Related: [mcp_tool_text_result](#mcp_tool_text_result), [run_self_for_mcp](#run_self_for_mcp).

### invoked_as_mcp_binary

```rust
pub fn invoked_as_mcp_binary() -> bool
```

Returns `true` when the current process's argv[0] file name is
`agent-swarm-mcp` — i.e. the binary was launched under the MCP entrypoint name.

- **Returns:** `true` if invoked as the MCP binary, else `false`.
- **When to use:** at startup, to decide whether to enter the MCP server loop automatically based on how the executable was invoked.

Related: [cmd_mcp](#cmd_mcp).

### required_arg

```rust
pub fn required_arg(arguments: &serde_json::Value, name: &str) -> Result<String, String>
```

Extracts a required string argument from a tool's `arguments` object, rejecting
both missing keys and blank/whitespace-only values.

- **Params:** `arguments` — the tool arguments object; `name` — the key.
- **Returns:** `Ok(value)` when present and non-blank; `Err("missing required argument \`name\`")` otherwise.
- **When to use:** the first line of any handler that has a mandatory argument (e.g. `prompt`, `session_id`, `job_id`).

Related: [optional_string_arg](#optional_string_arg).

### optional_string_arg

```rust
pub fn optional_string_arg(arguments: &serde_json::Value, name: &str) -> Option<String>
```

Extracts an optional string argument; returns `None` when the key is absent or not
a JSON string.

Related: [required_arg](#required_arg), [push_common_mcp_cli_args](#push_common_mcp_cli_args).

### optional_bool_arg

```rust
pub fn optional_bool_arg(arguments: &serde_json::Value, name: &str) -> Option<bool>
```

Extracts an optional boolean argument; `None` when absent or not a JSON bool.

Related: [push_common_mcp_cli_args](#push_common_mcp_cli_args).

### optional_u64_arg

```rust
pub fn optional_u64_arg(arguments: &serde_json::Value, name: &str) -> Option<u64>
```

Extracts an optional unsigned integer argument; `None` when absent or not a JSON
unsigned integer. Used for `timeout_secs`, `rounds`, `limit`, `since_ts_ms`,
`budget_tokens`, etc.

Related: [push_common_mcp_cli_args](#push_common_mcp_cli_args).

### optional_string_array_arg

```rust
pub fn optional_string_array_arg(arguments: &serde_json::Value, name: &str) -> Vec<String>
```

Extracts an optional array-of-strings argument, silently dropping non-string
elements; returns an empty vec when absent. Used for `tags`, `workers`,
`participants`.

- **When to use:** collecting list-valued arguments where non-string entries should be ignored rather than error.

### mcp_result

```rust
pub fn mcp_result(id: serde_json::Value, result: serde_json::Value) -> serde_json::Value
```

Builds a JSON-RPC success envelope: `{ "jsonrpc": "2.0", "id", "result" }`.

Related: [mcp_error](#mcp_error), [mcp_tool_text_result](#mcp_tool_text_result).

### mcp_error

```rust
pub fn mcp_error(id: serde_json::Value, code: i64, message: &str) -> serde_json::Value
```

Builds a JSON-RPC error envelope: `{ "jsonrpc": "2.0", "id", "error": { code, message } }`.

- **When to use:** protocol-level failures (parse error `-32700`, missing method
  `-32600`, unknown method `-32601`, bad params `-32602`). Tool-level failures use
  [mcp_tool_text_result](#mcp_tool_text_result) with `is_error = true` instead.

Related: [mcp_result](#mcp_result).

### mcp_tool_text_result

```rust
pub fn mcp_tool_text_result(
    id: serde_json::Value,
    text: String,
    is_error: bool,
) -> serde_json::Value
```

Builds a `tools/call` success envelope whose `result` carries
`content: [{ "type": "text", "text" }]` and an `isError` flag — the standard MCP
tool-result shape. Note that a tool *failure* is still a JSON-RPC success at the
protocol level, surfaced through `isError: true`.

- **When to use:** wrap any [McpToolOutput](#mcptooloutput) for return from `tools/call`.

Related: [McpToolOutput](#mcptooloutput), [mcp_result](#mcp_result).

### push_common_mcp_cli_args

```rust
pub fn push_common_mcp_cli_args(
    args: &mut Vec<String>,
    arguments: &serde_json::Value,
    default_quiet: bool,
)
```

Translates the common MCP argument set into swarm CLI flags, appended in a fixed
order: `--quiet` (when `quiet` is true, defaulting to `default_quiet`),
`--agent`, `--model`, `--cwd`, `--timeout` (from `timeout_secs`), and
`--allow-bypass-permissions`. The fixed order keeps generated command lines
deterministic (and testable).

- **Params:** `args` — the in-progress CLI argv being built; `arguments` — the tool arguments; `default_quiet` — the `quiet` default when the caller doesn't supply it (consult-mode tools pass `true`).
- **When to use:** inside any tool handler that re-invokes the binary for a run/swarm/discuss/etc., before pushing the positional prompt.

Related: [run_self_for_mcp](#run_self_for_mcp).

### run_self_for_mcp

```rust
pub fn run_self_for_mcp(args: Vec<String>) -> Result<McpToolOutput, String>
```

Re-invokes the current executable (`std::env::current_exe`) with `args` and a null
stdin, captures stdout and stderr, and folds them into an
[McpToolOutput](#mcptooloutput): trimmed stdout, then a `stderr:`-prefixed block if
stderr is non-empty, with a fallback exit-code message if both are empty.
`is_error` mirrors the child's exit status.

- **Params:** `args` — the CLI argv (subcommand + flags + positional) to run.
- **Returns:** the captured output, or `Err` if the executable couldn't be located/spawned.
- **When to use:** the standard adapter for tools that map onto a CLI subcommand (`run`, `swarm`, `discuss`, `audit`, `design`, `status`, `result`, `cancel`, `events`, `transcript`, `monitor-start`). Tools that assemble JSON in-process (manifest, insights, overview, reports, settings) skip this and build their `McpToolOutput` directly.

Related: [push_common_mcp_cli_args](#push_common_mcp_cli_args), [McpToolOutput](#mcptooloutput).

### manifest_payload

```rust
pub fn manifest_payload() -> serde_json::Value
```

Builds the `swarm.manifest/v1` package manifest: identity (`id` `agent-swarm`,
`kind`, version, display name, description), `entrypoints` (`cli` / `mcp`), the
`capabilities` list, example `commands`, the `mcp` block (stdio transport,
endpoint, command, service name, tags, and the tool-name list), declared `stores`
under `~/.swarm/*`, an `integration` block (registry path, peers, agent tools,
discovery hints), `skills` descriptors, and a `backends` array. Backend
availability is resolved **at call time** from PATH/fallback locations via
`locate_claude` / `locate_codex`.

- **Returns:** the manifest as a `serde_json::Value`.
- **When to use:** serve the `agent_swarm_manifest` tool, the `agent-swarm manifest` CLI command, and any package-discovery flow. Treat backend `available`/`path` as host-dependent; assert on shape, not the local value.

Related: [mcp_tool_names](#mcp_tool_names), [[swarm-kernel#binary-resolution]].

### mcp_tool_names

```rust
pub fn mcp_tool_names() -> Vec<&'static str>
```

Returns the flat list of MCP tool names advertised in the manifest's `mcp.tools`
field — a hand-maintained list that must match the names in
[mcp_tool_descriptors](#mcp_tool_descriptors). A contract test asserts the two
name-sets are equal, so adding a tool means editing both.

- **Returns:** the 36 `agent_swarm_*` tool names.
- **When to use:** advertise the tool surface in the manifest without serializing full schemas; also the parity anchor for the schema table.

Related: [manifest_payload](#manifest_payload), [mcp_tool_descriptors](#mcp_tool_descriptors).

### overview_json

```rust
pub fn overview_json() -> Result<serde_json::Value, String>
```

Assembles the `agent-swarm/overview/v1` digest — one call that merges what would
otherwise be three polls. It pulls the recent session list (classifying
running/incomplete/lost as "running", keeping the 5 most recent), loads jobs
(reconciling liveness, bucketing queued/running/lost as running and keeping the 5
most-recently-created), and the last 10 monitor alerts plus monitor liveness. The
payload carries a `generated_at_ms`, a `counts` block (six counters), and the
`running_sessions` / `recent_sessions` / `running_jobs` / `recent_jobs` /
`active_alerts` arrays.

- **Returns:** the digest value, or `Err` on a store error.
- **When to use:** dashboards and meta-conductors that want a single snapshot of current swarm activity instead of polling sessions + jobs + alerts separately. Serves the `agent_swarm_overview` tool.

Related: [session_list_json](#session_list_json), [runtime_processes_json](#runtime_processes_json), [[swarm-store#monitor-store]].

### session_summary_json

```rust
pub fn session_summary_json(id: &str) -> Result<serde_json::Value, String>
```

Builds the `agent-swarm/session-summary/v1` payload for one session: derived status
(via `SessionStatusDeriver` over the latest event kind + process liveness),
timestamps, prompt preview, cwd, manager/participants, **bounded** previews of the
digest/summary/api-docs files, the last 12 events from the tail of `events.jsonl`,
and the artifact pointer list. Reads from the resolved session directory; an
internal `_from_dir` variant takes an explicit path (used by fixture tests).

- **Params:** `id` — the session id.
- **Returns:** the summary value, or `Err` if metadata is missing/unparseable.
- **When to use:** serve `agent_swarm_session_summary`; show a structured, size-capped session overview without the client reconstructing paths or reading whole files.

Related: [session_artifacts_json](#session_artifacts_json), [[swarm-exec#sessions]], [[swarm-core#sessionrepo--sessionspechandlemetastatussummaryartifactstatusderiverindexrecord]].

### session_artifacts_json

```rust
pub fn session_artifacts_json(id: &str) -> Result<serde_json::Value, String>
```

Builds the `agent-swarm/session-artifacts/v1` payload: for each known session file
that exists (`session.json`, `events.jsonl`, `transcript.md`, `summary.md`,
`digest.md`, `api-docs.md`, `layer-reports.jsonl`) it emits `{ label, path, mime,
bytes }`, then enumerates the `layer-reports/` directory for `.md` files
(sorted, capped at 80) so the prefix is stable and deterministic.

- **Params:** `id` — the session id.
- **Returns:** the artifacts value, or `Err` on a path-resolution error.
- **When to use:** serve `agent_swarm_session_artifacts`; give the client real filesystem paths to a session's outputs without reconstructing them.

Related: [session_summary_json](#session_summary_json).

### session_list_json

```rust
pub fn session_list_json() -> Result<serde_json::Value, String>
```

Builds the `agent-swarm/session-list/v1` payload: the recent discussion sessions
(sorted by creation time descending, capped at 20), each as `{ id, status,
created_at_ms, prompt_preview }`. An internal `_from_base` variant reads from an
explicit base directory (used by `overview_json` and fixture tests).

- **Returns:** the list value, or `Err` on a store error.
- **When to use:** serve `agent_swarm_session_list`; also the session feed for [overview_json](#overview_json).

Related: [overview_json](#overview_json), [[swarm-exec#sessions]].

### runtime_processes_json

```rust
pub fn runtime_processes_json() -> Result<serde_json::Value, String>
```

Builds the `agent-swarm/runtime-processes/v1` payload: tracked live/lost processes
for monitoring. It reconciles job liveness then includes queued/running/lost jobs
and running/incomplete/lost sessions (each with `pid`, `alive`, timing, preview),
tracking their PIDs. It then scans the OS process table (via `sysinfo`) for
`agent-swarm` work commands not already tracked and emits them as `untracked-<pid>`
entries with derived `cullable` / `reap_required` / `reboot_required_if_uncullable`
hints (work commands are distinguished from read-only/maintenance subcommands,
which are ignored).

- **Returns:** the processes value, or `Err` on a store error.
- **When to use:** serve `agent_swarm_runtime_processes`; show what swarm work is actually live (including stray/untracked subprocesses) for monitoring and cleanup.

Related: [overview_json](#overview_json), [[swarm-store#monitor-store]], [[swarm-core#processliveness--neveralive--alwaysalive]].

### SwarmMcpServer

*(feature `rmcp`)*

```rust
#[derive(Clone, Default)]
pub struct SwarmMcpServer;

impl rmcp::ServerHandler for SwarmMcpServer { /* get_info, list_tools, call_tool */ }
```

A typed `rmcp` handler for the same Agent Swarm tool surface. `get_info` advertises
tools capability and `agent-swarm` server info; `list_tools` maps each
[mcp_tool_descriptor](#mcp_tool_descriptors) into an `rmcp::Tool`; `call_tool`
delegates to the **same** internal tool router as the hand-written loop, returning
an `rmcp` success/error result keyed on `is_error`.

- **When to use:** to expose the tools over the typed `rmcp` transport instead of the hand-written stdio loop. Construct with `SwarmMcpServer::default()` and serve via [serve_stdio](#serve_stdio).
- **Caveat (from source):** there is not yet an `rmcp`-wire parity test against the frozen artifact; `rmcp`'s `Tool` serde may rename/reorder fields, so this transport is not the default.

Related: [serve_stdio](#serve_stdio), [mcp_tool_descriptors](#mcp_tool_descriptors), [handle_mcp_request](#handle_mcp_request).

### serve_stdio

*(feature `rmcp`)*

```rust
pub async fn serve_stdio() -> Result<(), String>
```

Serves a default [SwarmMcpServer](#swarmmcpserver) over the `rmcp` stdio transport
and awaits its completion.

- **Returns:** `Ok(())` on clean shutdown, or `Err(String)` if the service fails to start or exits with an error.
- **When to use:** the async entrypoint for running the typed `rmcp` server (vs. the synchronous [cmd_mcp](#cmd_mcp) hand-written loop).

Related: [SwarmMcpServer](#swarmmcpserver), [cmd_mcp](#cmd_mcp).

### register_with

*(feature `registry`)*

```rust
pub fn register_with(
    registrar: Option<&dyn ServiceRegistrar>,
    record: &ServiceRecord,
) -> std::io::Result<()>
```

The opt-in service-registry seam. With `Some(registrar)` it delegates to
`registrar.register(record)`; with `None` — the off state — it returns `Ok(())`
and never touches the filesystem. The default MCP dispatch loop does **not** call
this hook; wiring it in is a consumer-side decision. The module also re-exports
`JsonFileRegistrar`, `NoopRegistrar`, `ServiceRecord`, and `ServiceRegistrar` from
`swarm-registrar` for convenience.

- **Params:** `registrar` — `Some(&impl)` to register, `None` for a guaranteed no-op; `record` — the service record to publish.
- **Returns:** `Ok(())` (always, for `None`), or the registrar's I/O result.
- **When to use:** let a consumer register this MCP endpoint into a JSON service registry of its choosing, while keeping the default build free of the dependency and any filesystem writes.

Related: [[swarm-registrar#serviceregistrar--jsonfileregistrar--noopregistrar]], [manifest_payload](#manifest_payload).

### gen_mcp_tools (binary)

```sh
cargo run -p swarm-mcp --bin gen_mcp_tools [-- --output <path>]
```

A helper binary that writes the frozen `mcp-tools.json` artifact from the
descriptor table. With no arguments it writes to
`<CARGO_MANIFEST_DIR>/mcp-tools.json` (commit the result). `--output <path>`
redirects the write — CI uses this to render into a temp file and diff against the
checked-in artifact without mutating the working tree. It calls
[mcp_tools_pretty_json](#mcp_tools_pretty_json), the same single serialization path
the idempotency test asserts against, guaranteeing byte-identity.

- **When to use:** after any change to [mcp_tool_descriptors](#mcp_tool_descriptors), regenerate and commit `mcp-tools.json`; the `mcp_tools_json_matches_descriptor_table` test fails until you do.

Related: [mcp_tools_pretty_json](#mcp_tools_pretty_json), [mcp_tool_descriptors](#mcp_tool_descriptors).
