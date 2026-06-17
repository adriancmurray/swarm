# swarm-contracts

Canonical wire-contract types for the swarm runtime — serde-only, compiles standalone, the IDL source of truth shared across every swarm crate.

## Overview

`swarm-contracts` owns the **wire-stable types** that cross crate boundaries and
hit disk: typed identifiers, the `agent-swarm/event/v2` envelope and its kind
discriminant, the persisted `JobRecord` and its enums, the shared MCP
tool-descriptor, telemetry records, the layer-report envelope, and the
event-stream canonicalizer.

It exists so that one definition — not several drifting copies — governs the
JSON bytes written to `events.jsonl`, `*.json` job files, the `*.jsonl`
telemetry/layer-report logs, and the MCP `tools/list` response. The crate's
governing constraint is **byte-identity**: every type is engineered so that
`deserialize → serialize` reproduces the original bytes (alphabetical field
order on structs, transparent newtypes, bare-string enums, explicit `null`
emission, and `skip_serializing_if` only where the historical wire omitted a
field). The crate's own test suite is a "lockbox" of real fixture lines that
assert this round-trip parity.

It carries no heavyweight dependencies (only `serde` + `serde_json`) and must
compile on its own, so it sits at the very bottom of the dependency DAG and can
be referenced by every higher crate without pulling in runtime concerns.

## Dependency position

Lowest crate in the workspace DAG:

```
swarm-contracts → swarm-core → swarm-store → swarm-kernel → swarm-exec → swarm-mcp / swarm-cli
```

- **Depends on:** nothing in the workspace — only `serde` (with `derive`) and
  `serde_json`.
- **Depended on by:** every other crate. `swarm-core` builds its repo traits on
  these records; `swarm-store` reads/writes them; `swarm-kernel` aggregates
  telemetry from them; `swarm-exec` emits events and persists jobs with them;
  `swarm-mcp` returns `McpToolDescriptor` over the wire.

The library target is named `swarm_contracts`.

## Concepts

Index of every public item. Each entry: **item** — one-line contract — anchor.

### `ids` — typed identifiers
- **SessionId** — transparent `String` newtype for a discussion-session id — [#sessionid](#sessionid)
- **JobId** — transparent `String` newtype for a job id — [#jobid](#jobid)
- **ProposalId** — transparent `String` newtype for a telemetry-proposal id — [#proposalid](#proposalid)
- **PresetId** — transparent `String` newtype for an orchestration-preset id — [#presetid](#presetid)

### `events` — the event-log contract
- **EventKind** — bare-string enum of 31 named event kinds + `Other(String)` escape hatch — [#eventkind](#eventkind)
- **SessionEventV2** — the `agent-swarm/event/v2` envelope written to `events.jsonl` — [#sessioneventv2](#sessioneventv2)

### `canonical` — deterministic event-stream projection
- **CanonicalEvent** — one volatile-free `{kind, role, payload}` projected event — [#canonicalevent](#canonicalevent)
- **canonicalize** — fold a raw event stream into the deterministic functional spine — [#canonicalize](#canonicalize)
- **canonical_run_hash** — stable FNV-1a/64 hash over the canonical projection — [#canonical_run_hash](#canonical_run_hash)
- **scrub_volatile** — recursively strip volatile keys / normalize absolute paths in a payload — [#scrub_volatile](#scrub_volatile)

### `jobs` — job persistence contract
- **DEFAULT_TIMEOUT_SECS** — default job timeout constant (`300`) — [#default_timeout_secs](#default_timeout_secs)
- **JobStatus** — bare-string job-lifecycle enum + `Other(String)` — [#jobstatus](#jobstatus)
- **JobAgent** — bare-string job-agent enum + `from_agent_name` + `Other(String)` — [#jobagent](#jobagent)
- **JobMode** — bare-string job-mode enum + `from_wire_str` + `Other(String)` — [#jobmode](#jobmode)
- **JobRecord** — the persisted per-job tracking record — [#jobrecord](#jobrecord)

### `mcp` — MCP tool descriptor
- **McpToolDescriptor** — one MCP `tools/list` entry (`name`, `description`, `inputSchema`) — [#mcptooldescriptor](#mcptooldescriptor)

### `telemetry` — learned-routing records
- **AgentObservation** — one agent-run observation (`observations.jsonl`) — [#agentobservation](#agentobservation)
- **AgentFeedback** — a routing-feedback record (`feedback.jsonl`) — [#agentfeedback](#agentfeedback)
- **AgentProposal** — a telemetry proposal (`proposals.jsonl`) — [#agentproposal](#agentproposal)
- **AgentProposalVote** — a proposal vote (`votes.jsonl`) — [#agentproposalvote](#agentproposalvote)

### `package` — layer-report envelope
- **LayerReportEnvelope** — the `agent-swarm/layer-report/v1` record (`layer-reports.jsonl`) — [#layerreportenvelope](#layerreportenvelope)

---

## API surface

### Identifier newtypes (`ids`)

All four ids share the same contract: a single-field tuple struct wrapping
`String`, derived `Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize`,
and `#[serde(transparent)]` so each serializes/deserializes as a **plain JSON
string** (no wrapper object). Each provides `as_str(&self) -> &str`, a
`Display` impl that forwards to the inner string, and both `From<String>` and
`From<&str>`. There is deliberately no other constructor — call sites always
spell out the newtype name, keeping construction visually distinct from bare
string usage.

#### SessionId

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionId(String);

impl SessionId { pub fn as_str(&self) -> &str; }
impl fmt::Display for SessionId { /* forwards to inner */ }
impl From<String> for SessionId { /* … */ }
impl From<&str>   for SessionId { /* … */ }
```

**Purpose.** A discussion-session identifier. Stored in session metadata
(`session.json`) and carried in every `agent-swarm/event/v2` envelope as both
`session_id` and `run_id`.

**When to use.** Construct with `SessionId::from(...)` whenever you mint or pass
a session id; use `.as_str()` to interop with string APIs.

**Related:** [[swarm-contracts#sessioneventv2]] (carries it as `session_id`/`run_id`),
[[swarm-core#session_repo]], [[swarm-store#FileSessionRepo]].

#### JobId

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct JobId(String);

impl JobId { pub fn as_str(&self) -> &str; }
impl fmt::Display for JobId { /* forwards to inner */ }
impl From<String> for JobId { /* … */ }
impl From<&str>   for JobId { /* … */ }
```

**Purpose.** A job identifier. Stored in [`JobRecord.id`](#jobrecord) and used as
the key for job files on disk (`{id}.json`).

**When to use.** When creating or looking up a job-tracking record.

**Related:** [[swarm-contracts#jobrecord]], [[swarm-core#job_repo]],
[[swarm-store#FileJobRepo]].

#### ProposalId

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProposalId(String);

impl ProposalId { pub fn as_str(&self) -> &str; }
impl fmt::Display for ProposalId { /* forwards to inner */ }
impl From<String> for ProposalId { /* … */ }
impl From<&str>   for ProposalId { /* … */ }
```

**Purpose.** A telemetry-proposal identifier. Stored in
[`AgentProposal.id`](#agentproposal) and cross-referenced by
[`AgentProposalVote.proposal_id`](#agentproposalvote).

**When to use.** When recording a proposal or matching a vote back to its
proposal.

**Related:** [[swarm-contracts#agentproposal]], [[swarm-contracts#agentproposalvote]].

#### PresetId

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PresetId(String);

impl PresetId { pub fn as_str(&self) -> &str; }
impl fmt::Display for PresetId { /* forwards to inner */ }
impl From<String> for PresetId { /* … */ }
impl From<&str>   for PresetId { /* … */ }
```

**Purpose.** An orchestration-preset identifier (e.g.
`"architecture-council"`). Arrives as a CLI argument or MCP tool parameter and
is matched against the known preset set; it is never persisted as a structured
field on disk.

**When to use.** When a caller names a preset to expand into swarm or discussion
orchestration.

**Related:** `PresetConfig` ([[swarm-kernel#config]]), `cmd_preset` ([[swarm-exec#cmd_preset]]).

---

### EventKind

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventKind {
    Created,
    SessionStarted, SessionCompleted,
    FanoutStarted,
    PreflightStarted, PreflightCompleted, PreflightFailed,
    ManagerStarted, ManagerCompleted, ManagerFailed,
    WorkerStarted, WorkerCompleted, WorkerFailed,
    WorkerFallback,   // visible degrade to next backend (never silent)
    BackendRetry,     // jittered-backoff retry of a failed backend
    HelperStarted, HelperCompleted, HelperFailed,
    DocsStarted, DocsCompleted, DocsFailed,
    TurnStarted, TurnCompleted, TurnFailed,
    TurnHeartbeat, TurnChunk, TurnHealthCheck,
    AgentMessage,
    ProfileAssigned,
    LayerReport,
    DiscussionDigestUpdated,
    Other(String),    // forward-compat / test escape hatch
}

impl EventKind {
    /// Exact wire string for this kind; `Other(s)` returns `s` unchanged.
    pub fn as_str(&self) -> &str;
}
// hand-written Serialize (bare string) + Deserialize (inverse) impls.
```

**Purpose.** The `kind` discriminant of a [`SessionEventV2`](#sessioneventv2).
31 named variants cover the orchestration lifecycle (session / fanout /
preflight / manager / worker / helper / docs / turn / message / profile /
layer-report / digest), plus `Other(String)` as a forward-compatibility escape
hatch.

**Wire contract.** Serializes as a **bare JSON string**, not a tagged object.
Each unit variant maps to its exact historical snake_case token via `as_str()`
(e.g. `WorkerFallback` → `"worker_fallback"`, `DiscussionDigestUpdated` →
`"discussion_digest_updated"`). `Deserialize` is the exact inverse: any string
not matching a known token becomes `Other(that_string)`, and `Other(s)`
serializes back to the bare `s` — so unrecognized on-disk kinds survive
read-mutate-write without data loss. `Other` is intended for the read path and
tests; the production write path should always use a named variant.

**Params/returns.** `as_str(&self) -> &str` — borrow of the wire token.

**When to use.** Match on it to react to lifecycle events; serialize it as the
`kind` field of an emitted event. Reach for `Other` only when forwarding an
unknown kind read from disk.

**Related:** [[swarm-contracts#sessioneventv2]],
[[swarm-contracts#canonicalize]] (drops `TurnHeartbeat`/`TurnChunk` as ephemeral).

### SessionEventV2

```rust
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SessionEventV2 {
    pub agent_id: String,
    pub kind: EventKind,
    pub parent_id: Option<String>,   // None serializes as JSON null (no skip)
    pub payload: serde_json::Value,
    pub phase: String,
    pub role: String,
    pub run_id: String,
    pub schema: String,
    pub seq: u64,
    pub session_id: String,
    pub ts_ms: u128,
}
```

**Purpose.** The `agent-swarm/event/v2` envelope persisted one-per-line to
`events.jsonl`.

**Wire contract — field order.** Fields are declared in **alphabetical order**
on purpose. Serde writes struct fields in declaration order, and agent-swarm's
production path emits objects through `serde_json::to_value` →
`Value::Object` (a `BTreeMap`, alphabetically sorted keys). Declaring fields
alphabetically makes struct serialization byte-identical to that output. Do not
reorder the fields.

**`parent_id` null behaviour.** `parent_id` has **no** `skip_serializing_if`; a
`None` emits `"parent_id":null`. This matches the production wire — do not add
`skip_serializing_if`.

**Fields.** `agent_id` — emitting agent (e.g. `"claude:sonnet"`, `"auto"`);
`kind` — the [`EventKind`](#eventkind); `parent_id` — optional parent event /
role; `payload` — free-form event-specific JSON; `phase` — orchestration phase
(e.g. `"discussion"`, `"turn"`); `role` — emitter role (e.g. `"manager"`,
`"qa"`); `run_id` / `session_id` — the run identifier (equal in current
output); `schema` — always `"agent-swarm/event/v2"`; `seq` — monotonic
per-process sequence; `ts_ms` — wall-clock milliseconds.

**When to use.** Construct and serialize one per emitted event on the write
path; deserialize each line when replaying or inspecting a session log.

**Related:** [[swarm-contracts#eventkind]], [[swarm-contracts#canonicalevent]],
[[swarm-core#event_repo]], [[swarm-store#FileEventRepo]].

---

### CanonicalEvent

```rust
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CanonicalEvent {
    pub kind: String,
    pub role: String,
    pub payload: serde_json::Value,
}
```

**Purpose.** One canonical event — a [`SessionEventV2`](#sessioneventv2)
projected down to its functional spine: `kind` + `role` + a volatile-scrubbed
`payload`. Serialize-only (no `Deserialize`); it is an output type.

**Wire contract.** Serializes to a stable `{kind, role, payload}` object (fields
in declaration order; payload object keys sort via serde's `BTreeMap`), so two
runs of the same logical work produce identical bytes.

**When to use.** As the unit of the slice returned by
[`canonicalize`](#canonicalize) — for cross-run equality, diffing, or feeding
[`canonical_run_hash`](#canonical_run_hash).

**Related:** [[swarm-contracts#canonicalize]], [[swarm-contracts#canonical_run_hash]].

### canonicalize

```rust
pub fn canonicalize(events: &[SessionEventV2]) -> Vec<CanonicalEvent>;
```

**Purpose.** Fold a raw event stream into a deterministic functional projection.
Two runs of the same logical work produce byte-different `events.jsonl` (per-run
`seq`/`ts_ms`/`session_id`, machine-dependent heartbeat counts, thread-shuffled
order, embedded durations/pids/paths/byte-counts). `canonicalize` strips all of
that so the *functional* content — which roles emitted which kinds with what
stable payload fields — can be compared.

**Behaviour.** Drops ephemeral kinds ([`EventKind::TurnHeartbeat`](#eventkind),
`TurnChunk`); projects each surviving event to `{kind, role, payload}`; scrubs
volatile payload fields via [`scrub_volatile`](#scrub_volatile); then sorts into
the total canonical order `(kind, role, scrubbed-payload-json)`. It is **pure**
(no clock / IO / RNG), **deterministic**, **idempotent**, and
**order-independent** — any permutation of `events` yields the same output.

**Params/returns.** `events` — the raw event slice (any order); returns a
sorted `Vec<CanonicalEvent>` with ephemerals removed.

**When to use.** As the foundation of the parity harness — to assert that two
runs did the same logical work, or to diff what differed. See
`docs/specs/canonicalizer-spec.md` for the per-variant rules.

**Related:** [[swarm-contracts#canonicalevent]], [[swarm-contracts#scrub_volatile]],
[[swarm-contracts#canonical_run_hash]], [[swarm-contracts#eventkind]].

### canonical_run_hash

```rust
pub fn canonical_run_hash(events: &[SessionEventV2]) -> String;
```

**Purpose.** A stable, dependency-free hash over the serialized canonical
projection. Equal iff two runs produced the same functional canonical
projection.

**Behaviour.** Runs [`canonicalize`](#canonicalize), serializes the result, and
hashes the bytes with FNV-1a/64, returning 16 lowercase hex chars. Serialization
of `Vec<CanonicalEvent>` is deterministic (declaration-order fields +
BTreeMap-sorted payload keys); on the impossible serialization failure it falls
back to an empty string rather than panicking.

**Params/returns.** `events` — the raw event slice; returns a 16-char hex
digest.

**When to use.** A compact run fingerprint for equality checks, golden-snapshot
pinning, or detecting that two runs diverged functionally.

**Related:** [[swarm-contracts#canonicalize]], [[swarm-contracts#canonicalevent]].

### scrub_volatile

```rust
pub fn scrub_volatile(value: &mut serde_json::Value);
```

**Purpose.** Recursively remove run-to-run-volatile keys and normalize
absolute-path string values, in place.

**Behaviour.** On objects: drop volatile keys, then recurse into survivors. On
arrays: recurse into elements. On strings: replace a value that is *entirely* an
absolute path (Unix `/a/b` or Windows `C:\` / `C:/`) with `"<path>"`; paths
embedded inside prose are left untouched. Volatile keys are an exact set (`seq`,
`session_id`, `run_id`, `pid`, `ts`, `ts_ms`, `started_at`, `completed_at`,
`created_at`, `elapsed`, `duration`, `bytes`, `path`, `file`, `cwd`) plus the
suffix rules `_ms`, `_bytes`, `_path` to future-proof new fields. See
`docs/specs/canonicalizer-spec.md` §5.

**Params/returns.** `value` — mutated in place; no return value.

**When to use.** Primarily called internally by
[`canonicalize`](#canonicalize); exposed publicly so callers can scrub a single
payload `Value` directly.

**Related:** [[swarm-contracts#canonicalize]].

---

### DEFAULT_TIMEOUT_SECS

```rust
pub const DEFAULT_TIMEOUT_SECS: u64 = 300;
```

**Purpose.** The default job timeout in seconds. Mirrors
`agent-swarm::DEFAULT_TIMEOUT_SECS`. Used as the serde default for
[`JobRecord.timeout_secs`](#jobrecord) so legacy records without the field parse
to `300`.

**Related:** [[swarm-contracts#jobrecord]], `BackendRequest.timeout` ([[swarm-kernel#backend_abi]]).

### JobStatus

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobStatus {
    Running, Queued, Completed, Failed, Lost, Cancelled, TimedOut,
    Other(String),   // forward-compat catch-all
}

impl JobStatus { pub fn as_str(&self) -> &str; }
impl fmt::Display for JobStatus { /* forwards to as_str */ }
// hand-written Serialize (bare string) + Deserialize (inverse) impls.
```

**Purpose.** The `status` field of a [`JobRecord`](#jobrecord) — the job
lifecycle state.

**Wire contract.** Each unit variant serializes to its exact snake_case token
(`Running`→`"running"`, `TimedOut`→`"timed_out"`, …). `Other(s)` serializes as
the bare string `s`, and any unknown wire string deserializes to `Other`, so
future states survive read-mutate-write. There is no `From<&str>` — `Other`
construction stays visually loud.

**Params/returns.** `as_str(&self) -> &str` — the wire token; `Display` forwards
to it.

**When to use.** Set/read a job's lifecycle state.

**Related:** [[swarm-contracts#jobrecord]], [[swarm-core#job_repo]].

### JobAgent

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobAgent {
    Gemini, Claude, Codex, Auto,
    Swarm,           // synthetic label for swarm/discussion tracking records
    Other(String),   // forward-compat catch-all
}

impl JobAgent {
    pub fn as_str(&self) -> &str;
    /// Parse from an agent-name string (no From<&str>, to keep Other loud).
    pub fn from_agent_name(name: &str) -> Self;
}
impl fmt::Display for JobAgent { /* forwards to as_str */ }
// hand-written Serialize (bare string) + Deserialize (inverse) impls.
```

**Purpose.** The `agent` field of a [`JobRecord`](#jobrecord) — which agent the
job ran (or `Swarm`, the synthetic label for swarm/discussion tracking records).

**Wire contract.** Same bare-string + `Other` forward-compat contract as
[`JobStatus`](#jobstatus). `from_agent_name` is the explicit string constructor
(unknown names → `Other`).

**Params/returns.** `as_str(&self) -> &str` — wire token; `from_agent_name(name:
&str) -> Self`.

**When to use.** Record/read the agent on a job; use `from_agent_name` to map a
CLI/config agent string into the enum.

**Related:** [[swarm-contracts#jobrecord]], `AgentSpec` ([[swarm-kernel#agent]]).

### JobMode

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobMode {
    Agent, Consult, Swarm, Discussion,
    Other(String),   // forward-compat catch-all
}

impl JobMode {
    pub fn as_str(&self) -> &str;
    /// Parse from a wire string (no From<&str>, to keep Other loud).
    pub fn from_wire_str(s: &str) -> Self;
}
impl fmt::Display for JobMode { /* forwards to as_str */ }
// hand-written Serialize (bare string) + Deserialize (inverse) impls.
```

**Purpose.** The `mode` field of a [`JobRecord`](#jobrecord) — how the job ran:
`Agent` (background agent run), `Consult` (single routed prompt), `Swarm`
(fanout), or `Discussion` (multi-round debate).

**Wire contract.** Same bare-string + `Other` forward-compat contract as the
other job enums. `from_wire_str` is the explicit string constructor.

**Params/returns.** `as_str(&self) -> &str`; `from_wire_str(s: &str) -> Self`.

**When to use.** Record/read the run mode on a job; distinguish a background
agent run from a consult/swarm/discussion run.

**Related:** [[swarm-contracts#jobrecord]], `run_swarm`/`run_discussion`
([[swarm-exec#orchestration]]).

### JobRecord

```rust
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct JobRecord {
    pub id: JobId,
    pub status: JobStatus,
    pub agent: JobAgent,
    pub model: Option<String>,
    pub mode: JobMode,
    pub cwd: String,
    pub prompt_preview: String,
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
    pub created_at_ms: u128,
    pub started_at_ms: Option<u128>,
    pub completed_at_ms: Option<u128>,
    pub pid: Option<u32>,
    pub exit_code: Option<i32>,
    pub prompt_path: String,
    pub stdout_path: String,
    pub stderr_path: String,
    pub result_path: String,
    #[serde(default)]
    pub allow_recursive_codex: bool,
}
```

**Purpose.** The persisted per-job tracking record, stored as a JSON file named
`{id}.json` in the job-store directory.

**Wire contract.** Production serialization is `serde_json::to_string_pretty` +
a trailing `\n`. Two fields use serde defaults so **legacy records parse**:
`timeout_secs` defaults to [`DEFAULT_TIMEOUT_SECS`](#default_timeout_secs) when
absent, and `allow_recursive_codex` defaults to `false`. All other `Option<T>`
fields emit `null` when `None`.

**Fields.** `id`/`status`/`agent`/`mode` — the typed
[`JobId`](#jobid)/[`JobStatus`](#jobstatus)/[`JobAgent`](#jobagent)/[`JobMode`](#jobmode);
`model` — optional model name; `cwd` — working directory; `prompt_preview` —
truncated prompt for listings; `timeout_secs` — run timeout; `created_at_ms` /
`started_at_ms` / `completed_at_ms` — lifecycle timestamps; `pid` — OS process
id while running; `exit_code` — process exit code; `prompt_path` / `stdout_path`
/ `stderr_path` / `result_path` — on-disk artifact paths;
`allow_recursive_codex` — whether a nested codex invocation is permitted.

**When to use.** Read it to report a job's status/result; write it to persist or
update a tracked job.

**Related:** [[swarm-contracts#jobid]], [[swarm-contracts#jobstatus]],
[[swarm-contracts#jobagent]], [[swarm-contracts#jobmode]], [[swarm-core#job_repo]],
[[swarm-store#FileJobRepo]].

---

### McpToolDescriptor

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct McpToolDescriptor {
    pub name: String,
    pub description: String,
    #[serde(rename = "inputSchema")]
    pub input_schema: serde_json::Value,
}
```

**Purpose.** One MCP tool entry as returned by the `tools/list` response — the
shared descriptor type that prevents the MCP-schema table and any binary-SDK
consumers from diverging.

**Wire contract.** Exactly three keys per entry, always present, never omitted:
`name`, `description`, and `inputSchema` (camelCase — note the `serde(rename)`).
No-arg tools use `{"type":"object","properties":{}}` as the schema (no
`required` key). Any future optional MCP fields (`title`, `annotations`,
`outputSchema`) can be added as `Option<…>` + `skip_serializing_if` without
breaking byte-identity for current consumers.

**Fields.** `name` — tool name (e.g. `"agent_swarm_manifest"`); `description` —
human-readable text surfaced to the LLM client; `input_schema` — the JSON Schema
for the tool's input parameters (serialized as `inputSchema`).

**When to use.** Build the `tools/list` table; declare a tool's input contract;
consume a tool descriptor from a remote server.

**Related:** `mcp_tool_descriptors` ([[swarm-mcp#tool-schema]]),
`mcp_tool_names` ([[swarm-mcp#manifest]]).

---

### AgentObservation

```rust
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AgentObservation {
    pub schema: String,
    pub ts_ms: u128,
    pub mode: String,
    pub session_id: Option<String>,
    pub role: String,
    pub agent: String,
    pub cwd: String,
    pub status: String,
    pub exit_code: i32,
    pub timed_out: bool,
    pub duration_ms: u128,
    pub prompt_bytes: usize,
    pub stdout_bytes: usize,
    pub stderr_bytes: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
}
```

**Purpose.** A single agent-run observation, appended as a JSONL line to
`observations.jsonl`. The raw signal that learned-routing aggregation consumes.

**Wire contract.** `input_tokens` / `output_tokens` carry
`#[serde(default, skip_serializing_if = "Option::is_none")]` so the common case
(Gemini/Codex, or any token-parse failure) omits these fields entirely. Do not
remove the `skip_serializing_if` — it would diverge the wire for existing
observations. `schema` is `"agent-swarm/observation/v1"`.

**Fields.** Run identity (`mode`, `session_id`, `role`, `agent`, `cwd`),
outcome (`status`, `exit_code`, `timed_out`), cost (`duration_ms`,
`prompt_bytes`, `stdout_bytes`, `stderr_bytes`), and optional LLM token counts
(`input_tokens`, `output_tokens`; Claude only).

**When to use.** Emit one per agent run; read the log to aggregate per-agent
statistics.

**Related:** [[swarm-kernel#telemetry]] (`record_observation`, `aggregate_stats`),
[[swarm-core#telemetry_repo]].

### AgentFeedback

```rust
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AgentFeedback {
    pub schema: String,
    pub ts_ms: u128,
    pub session_id: Option<String>,
    pub role: String,
    pub agent: String,
    pub outcome: String,
    pub note: Option<String>,
    pub weight: f64,
}
```

**Purpose.** A routing-feedback record appended to `feedback.jsonl` — a weighted
judgement that an agent did well/poorly in a role, feeding learned routing.
`schema` is `"agent-swarm/feedback/v1"`.

**Fields.** `session_id` (optional), `role`, `agent`, `outcome` (e.g. a
win/loss label), `note` (optional rationale), and `weight` (how much this
feedback counts).

**When to use.** Record explicit feedback on a role/agent pairing to influence
future routing.

**Related:** [[swarm-kernel#telemetry]] (`record_feedback`),
[[swarm-core#telemetry_repo]].

### AgentProposal

```rust
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AgentProposal {
    pub schema: String,
    pub id: ProposalId,
    pub ts_ms: u128,
    pub session_id: Option<String>,
    pub title: String,
    pub body: String,
    pub proposed_by: String,
    pub status: String,
    pub tags: Vec<String>,
}
```

**Purpose.** A telemetry proposal appended to `proposals.jsonl` — a structured
suggestion (title + body) that agents can later vote on. `schema` is
`"agent-swarm/proposal/v1"`.

**Fields.** `id` — the [`ProposalId`](#proposalid); `session_id` (optional);
`title` / `body`; `proposed_by` — author; `status` (e.g. `"open"`); `tags` —
free-form labels.

**When to use.** Record a proposal; list open proposals for voting.

**Related:** [[swarm-contracts#proposalid]], [[swarm-contracts#agentproposalvote]],
[[swarm-kernel#telemetry]] (`record_proposal*`, `proposals_json`).

### AgentProposalVote

```rust
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AgentProposalVote {
    pub schema: String,
    pub ts_ms: u128,
    pub proposal_id: ProposalId,
    pub voter: String,
    pub vote: String,
    pub rationale: Option<String>,
    pub weight: f64,
}
```

**Purpose.** A vote on a proposal, appended to `votes.jsonl`. `schema` is
`"agent-swarm/vote/v1"`.

**Fields.** `proposal_id` — the [`ProposalId`](#proposalid) this vote targets;
`voter`; `vote` (e.g. `"win"`); `rationale` (optional); `weight`.

**When to use.** Record a vote against an existing [`AgentProposal`](#agentproposal).

**Related:** [[swarm-contracts#agentproposal]], [[swarm-contracts#proposalid]],
[[swarm-kernel#telemetry]].

---

### LayerReportEnvelope

```rust
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LayerReportEnvelope {
    pub schema: String,
    pub ts_ms: u128,
    pub session_id: String,
    pub layer: String,
    pub role: String,
    pub agent: String,
    pub parent_role: Option<String>,
    pub status: String,
    pub preview: String,
    pub file: String,
    pub text_bytes: usize,
}
```

**Purpose.** An `agent-swarm/layer-report/v1` envelope, appended to
`layer-reports.jsonl` — one entry per agent layer's contribution in a discussion
round. The full text lives in a separate `.md` file; `preview` and `file` are
stored inline for fast list-mode rendering.

**Wire contract.** `parent_role` has **no** `skip_serializing_if`; when `None`
it emits `"parent_role":null` (matches the production wire). `schema` is
`"agent-swarm/layer-report/v1"`.

**Fields.** `session_id`; `layer` (e.g. `"qa"`, `"implementation"`); `role`;
`agent` (e.g. `"claude:sonnet"`, `"codex"`); `parent_role` (optional parent in
the layer hierarchy); `status` (e.g. `"completed"`); `preview` — short text
excerpt; `file` — relative path to the full `.md` report; `text_bytes` — size of
the full text.

**When to use.** Record a layer's output during a discussion round; read the log
to render a session's layer reports without loading every `.md`.

**Related:** [[swarm-contracts#sessioneventv2]] (`EventKind::LayerReport`),
`build_swarm_transcript` ([[swarm-exec#synthesis]]),
`session_summary_json` ([[swarm-mcp#report-assemblers]]).
