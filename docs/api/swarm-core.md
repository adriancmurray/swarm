# swarm-core

Pure repo-trait substrate for the swarm runtime: storage-contract traits
(`SessionRepo` / `JobRepo` / `EventRepo` / `LedgerRepo` / `TelemetryRepo`) and
their companion value types, with zero concrete I/O.

## Overview

`swarm-core` owns the **persistence contracts** — the trait definitions every
storage backend must satisfy and the small value types those traits exchange. It
is the seam between the wire types (`swarm-contracts`) and the file/in-memory
implementations (`swarm-store`). Nothing here touches the filesystem, the OS, or
the network: the only effects are pure data transformation (`fold_tasks`,
`SessionStatusDeriver::derive`) and the error/cursor primitives that flow through
every repo call.

This crate exists so that the orchestration engine can depend on storage by
*interface*, not implementation: `swarm-exec` and friends program against
`SessionRepo`/`EventRepo`/etc., and the real `File*`/`Mem*` impls in
`swarm-store` are swapped in (or replaced by test doubles like `NeverAlive` /
`AlwaysAlive`) without changing callers. The companion types deliberately stay
minimal — e.g. `SessionSpec` carries only `cwd`/`mode`/`prompt` rather than
pulling `SwarmArgs` into the trait layer — to keep the contract free of
higher-level business types.

Everything is split-identity-safe: the crate must resolve from a single
`local-path` source in the build graph (enforced by `ci/law-checks.sh` CHECK5),
and it depends only on `swarm-contracts` plus serde.

## Dependency position

```
swarm-contracts → swarm-core → swarm-store → swarm-kernel → swarm-exec → swarm-mcp / swarm-cli
```

- **Depends on:** `swarm-contracts` (wire types: `SessionId`, `JobId`,
  `EventKind`, `JobRecord`, `JobAgent`, `JobMode`, the telemetry records) and
  `serde` / `serde_json`. **No external-system types.**
- **Depended on by:** `swarm-store` (which provides the concrete `File*`/`Mem*`
  impls and the production `OsProcessLiveness` oracle), and transitively by every
  higher crate that reads or writes the store.

The trait/impl split is intentional: traits + companions live here; all concrete
`File*`/`Mem*` repos and the OS liveness oracle live one layer up in
`swarm-store`.

## Concepts

Every public item exported from `lib.rs`, with a one-line summary and an anchor.

### Primitives
- [`RepoError`](#repoerror) — the unified error enum returned by every repo method.
- [`Cursor`](#cursor) — opaque `u64` resumption token for event reads.
- [`ProcessLiveness`](#processliveness) — injected oracle: is a pid still alive?
- [`NeverAlive`](#neveralive) — test double; every process is dead.
- [`AlwaysAlive`](#alwaysalive) — test double; every process is alive.

### Session contract
- [`SessionRepo`](#sessionrepo) — storage trait for sessions (create/open/list/summary/artifacts).
- [`SessionSpec`](#sessionspec) — create-request input to `SessionRepo::create`.
- [`SessionHandle`](#sessionhandle) — owned handle bundling a session id + its `EventRepo`.
- [`SessionMeta`](#sessionmeta) — typed metadata written to `session.json`.
- [`SessionIndexRecord`](#sessionindexrecord) — lightweight per-session row from `list()`.
- [`SessionSummary`](#sessionsummary) — rich `session-summary/v1` JSON wrapper.
- [`SessionArtifact`](#sessionartifact) — one artifact entry (label/path/mime/bytes).
- [`SessionStatus`](#sessionstatus) — computed lifecycle status enum + `as_str`.
- [`SessionStatusDeriver`](#sessionstatusderiver) — pure status derivation from record + liveness + last event.

### Job contract
- [`JobRepo`](#jobrepo) — storage trait for job records.
- [`JobSpec`](#jobspec) — create-request input to `JobRepo::create`.

### Event contract
- [`EventRepo`](#eventrepo) — append/read trait for session events.
- [`EventContext`](#eventcontext) — per-event context metadata (parent/agent/role/phase).
- [`LayerReportSpec`](#layerreportspec) — input to `append_layer_report`.
- [`StoredEvent`](#storedevent) — owned read model for an `event/v2` line.

### Ledger contract
- [`LedgerRepo`](#ledgerrepo) — append-only task-ledger trait.
- [`LedgerTask`](#ledgertask) — one task snapshot row + verification helpers.
- [`LedgerStatus`](#ledgerstatus) — task lifecycle enum (`open`/`claimed`/`claimed_done`/`verified_done`).
- [`fold_tasks`](#fold_tasks) — collapse snapshot history to current per-id state.
- [`LEDGER_TASK_SCHEMA`](#ledger_task_schema) — schema string constant for ledger rows.

### Telemetry contract
- [`TelemetryRepo`](#telemetryrepo) — append-only storage for observations/feedback/proposals/votes.

## API surface

### RepoError

```rust
#[derive(Debug)]
pub enum RepoError {
    Io(std::io::Error),
    Serialize(serde_json::Error),
    NotFound(String),
    InvalidId(String),
    TooLarge { path: PathBuf, bytes: usize },
}
```

The single error type returned by every method on every repo trait in this
crate. `Io` and `Serialize` wrap their inner error and expose it through the
`std::error::Error::source()` chain; the three leaf variants (`NotFound`,
`InvalidId`, `TooLarge`) have no source. Implements `Display`, `Error`, and
`From<std::io::Error>` / `From<serde_json::Error>` so impls can use `?` directly.

- **When to use:** as the `Err` arm of any repo call. Match `NotFound` to map a
  missing id to a 404-style response, `InvalidId` for store-id validation
  failures, and `TooLarge { path, bytes }` for tail/artifact byte-cap rejections.
- **Note:** `Clone` is *not* derived — `io::Error` is not `Clone`.

Related: every trait below returns `Result<_, RepoError>`.

### Cursor

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Cursor(u64);

impl Cursor {
    pub fn start() -> Self;   // == Cursor(0)
    pub fn new(v: u64) -> Self;
    pub fn get(self) -> u64;
}
```

Opaque resumption token for `EventRepo::events_since`. The inner `u64` is
interpreted naturally by each backend — byte offset of the last complete `\n`
for the JSONL backend, `Vec` index for the in-memory backend. `Cursor::start()`
means "no events seen — return from the beginning."

- **`start()`** — the initial value; pass to `events_since` to read from the top.
- **`new(v)`** — construct from a raw backend position (used by store impls to
  hand back a resume point).
- **`get()`** — read the raw position back out (used by the JSONL backend to seek).
- **When to use:** thread the returned cursor from one `events_since` /
  `append` call into the next to tail a session incrementally. Cursors are
  per-session and per-instance — never share across sessions, never serialize
  into event JSON.

Related: [[swarm-core#eventrepo]].

### ProcessLiveness

```rust
pub trait ProcessLiveness: Send + Sync {
    fn is_alive(&self, pid: u32) -> bool;
}
```

Injected dependency for liveness checks: given an OS pid, is that process still
running? Lets status-derivation logic stay pure and testable — production code
passes the real oracle (`OsProcessLiveness`, which lives in `swarm-store`), tests
pass [`NeverAlive`](#neveralive) / [`AlwaysAlive`](#alwaysalive).

- **When to use:** pass an `&dyn ProcessLiveness` into
  [`SessionStatusDeriver::derive`](#sessionstatusderiver) or
  [`JobRepo::reconcile_liveness`](#jobrepo) to resolve `Running` vs `Lost`
  without coupling those call sites to the OS.

Related: [[swarm-core#sessionstatusderiver]], [[swarm-core#jobrepo]], [[swarm-store#osprocessliveness]].

### NeverAlive

```rust
pub struct NeverAlive;
impl ProcessLiveness for NeverAlive { /* is_alive → false */ }
```

Test double whose `is_alive` always returns `false` — simulates every process
dead. Use to force the `Lost` / `Incomplete` derivation paths in tests.

Related: [[swarm-core#processliveness]].

### AlwaysAlive

```rust
pub struct AlwaysAlive;
impl ProcessLiveness for AlwaysAlive { /* is_alive → true */ }
```

Test double whose `is_alive` always returns `true` — simulates every process
alive. Use to force the `Running` derivation path in tests.

Related: [[swarm-core#processliveness]].

### SessionRepo

```rust
pub trait SessionRepo: Send + Sync {
    type Events: EventRepo;

    fn create(&self, spec: SessionSpec) -> Result<SessionHandle<Self::Events>, RepoError>;
    fn open(&self, id: &SessionId) -> Result<SessionHandle<Self::Events>, RepoError>;
    fn list(&self) -> Result<Vec<SessionIndexRecord>, RepoError>;
    fn write_metadata(&self, id: &SessionId, meta: &SessionMeta) -> Result<(), RepoError>;
    fn summary(&self, id: &SessionId) -> Result<SessionSummary, RepoError>;
    fn artifacts(&self, id: &SessionId) -> Result<Vec<SessionArtifact>, RepoError>;
}
```

Pure storage abstraction for sessions. The associated type `Events` ties a repo
to the concrete `EventRepo` its handles own, so callers can append events through
`handle.events` without round-tripping the session repo.

- **`create(spec)`** — make a new session directory and emit the initial
  `Created` event; returns an owned `SessionHandle`.
- **`open(id)`** — load a handle for an existing session.
- **`list()`** — return all `SessionIndexRecord`s. **Pure**: no liveness side
  effects; status is derived separately via
  [`SessionStatusDeriver`](#sessionstatusderiver).
- **`write_metadata(id, meta)`** — write/update `session.json`.
- **`summary(id)`** — read the rich `session-summary/v1` payload (metadata +
  recent events + digest).
- **`artifacts(id)`** — list the session's artifact files.
- **When to use:** as the storage interface for any session read/write; bind the
  concrete impl (e.g. `FileSessionRepo`) at the edge and program against this
  trait everywhere else.

Related: [[swarm-core#sessionspec]], [[swarm-core#sessionhandle]], [[swarm-core#sessionstatusderiver]], [[swarm-core#eventrepo]], [[swarm-store#filesessionrepo--memsessionrepo]].

### SessionSpec

```rust
#[derive(Debug, Clone)]
pub struct SessionSpec {
    pub cwd: PathBuf,
    pub mode: String,    // e.g. "fanout", "discussion"
    pub prompt: String,  // full prompt text; callers derive the preview
}
```

Input to [`SessionRepo::create`](#sessionrepo). Deliberately minimal — only what
the repo needs to start a session — so the trait layer never imports
higher-level arg types.

Related: [[swarm-core#sessionrepo]].

### SessionHandle

```rust
pub struct SessionHandle<E: EventRepo> {
    pub id: SessionId,
    pub events: E,
}
```

Owned handle to an open session, returned by `create`/`open`. Bundles the
session id with an owned `EventRepo` bound to this session's storage, so the
caller appends events directly via `handle.events`.

Related: [[swarm-core#sessionrepo]], [[swarm-core#eventrepo]].

### SessionMeta

```rust
#[derive(Debug, Clone)]
pub struct SessionMeta {
    pub id: String,
    pub created_at_ms: u128,
    pub pid: u32,
    pub prompt: String,
    pub cwd: PathBuf,
    pub mode: String,
}
```

Typed metadata written to `session.json` via
[`SessionRepo::write_metadata`](#sessionrepo). Kept minimal — the full
structured metadata (manager, participants, docs) is written separately by the
orchestration layer's `write_metadata` / `write_swarm_metadata`.

Related: [[swarm-core#sessionrepo]].

### SessionIndexRecord

```rust
#[derive(Debug, Clone)]
pub struct SessionIndexRecord {
    pub id: SessionId,
    pub created_at_ms: u128,
    pub pid: Option<u32>,
    pub prompt_preview: String,  // pre-truncated, 72 chars
}
```

Lightweight per-session row returned by [`SessionRepo::list`](#sessionrepo).
Contains only fields readable without touching the event tail or the liveness
oracle; feed it (plus the last `EventKind` and a `ProcessLiveness`) to
[`SessionStatusDeriver::derive`](#sessionstatusderiver) to compute status.

Related: [[swarm-core#sessionrepo]], [[swarm-core#sessionstatusderiver]].

### SessionSummary

```rust
#[derive(Debug, Clone)]
pub struct SessionSummary {
    pub value: serde_json::Value,  // agent-swarm/session-summary/v1
}
```

Rich summary returned by [`SessionRepo::summary`](#sessionrepo): a thin wrapper
around the `session-summary/v1` JSON value assembled by the report layer.

Related: [[swarm-core#sessionrepo]].

### SessionArtifact

```rust
#[derive(Debug, Clone)]
pub struct SessionArtifact {
    pub label: String,  // e.g. "events", "transcript", "layer-report"
    pub path: PathBuf,  // absolute
    pub mime: String,
    pub bytes: u64,
}
```

A single artifact entry returned by [`SessionRepo::artifacts`](#sessionrepo).

Related: [[swarm-core#sessionrepo]].

### SessionStatus

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionStatus {
    Completed,   // "completed" — last event was session_completed
    Running,     // "running"   — pid alive, or pid-absent but recent
    Lost,        // "lost"      — pid existed but liveness says dead
    Incomplete,  // "incomplete" — no pid and older than 5 minutes
}

impl SessionStatus {
    pub fn as_str(&self) -> &str;  // stable wire string
}
```

Computed lifecycle status of a session. `as_str()` returns the stable wire
token (`"completed"` / `"running"` / `"lost"` / `"incomplete"`) consumed by MCP
clients. Produced only by [`SessionStatusDeriver`](#sessionstatusderiver) — never
stored directly.

Related: [[swarm-core#sessionstatusderiver]].

### SessionStatusDeriver

```rust
pub struct SessionStatusDeriver;

impl SessionStatusDeriver {
    pub fn derive(
        record: &SessionIndexRecord,
        latest_event_kind: Option<&EventKind>,
        liveness: &dyn ProcessLiveness,
        now_ms: u128,
    ) -> SessionStatus;
}
```

Pure derivation of [`SessionStatus`](#sessionstatus) from raw inputs — touches
neither the filesystem nor OS APIs (liveness and time are injected). Derivation
rules, in order:

1. `latest_event_kind == SessionCompleted` → `Completed`.
2. `pid` present and `liveness.is_alive(pid)` → `Running`.
3. `pid` present and not alive → `Lost`.
4. `pid` absent and `now_ms - created_at_ms > 5 min` → `Incomplete`.
5. Otherwise (pid absent, session recent) → `Running`.

- **Params:** `record` from [`SessionRepo::list`](#sessionrepo);
  `latest_event_kind` from [`EventRepo::latest_kind`](#eventrepo); `liveness`
  oracle; `now_ms` current time (injected for testability).
- **When to use:** compute display status after listing sessions, keeping the
  list call pure and the time/liveness inputs mockable.

Related: [[swarm-core#sessionstatus]], [[swarm-core#sessionindexrecord]], [[swarm-core#processliveness]], [[swarm-core#eventrepo]], [[swarm-contracts#eventkind]].

### JobRepo

```rust
pub trait JobRepo: Send + Sync {
    fn create(&self, spec: JobSpec) -> Result<JobRecord, RepoError>;
    fn get(&self, id: &JobId) -> Result<JobRecord, RepoError>;
    fn list(&self) -> Result<Vec<JobRecord>, RepoError>;
    fn save(&self, record: &JobRecord) -> Result<(), RepoError>;
    fn latest(&self) -> Result<Option<JobRecord>, RepoError>;
    fn reconcile_liveness(
        &self,
        liveness: &dyn ProcessLiveness,
    ) -> Result<Vec<JobRecord>, RepoError>;
}
```

Pure storage trait for job records. No method performs liveness checks
implicitly — call `reconcile_liveness` explicitly before any read that needs
current process status.

- **`create(spec)`** — persist a new record in the `Running` state.
- **`get(id)`** / **`list()`** / **`latest()`** — read one / all / most-recent
  records. `list`/`latest` are **pure** (no liveness side effects).
- **`save(record)`** — persist an updated record (finish, prompt-update paths).
- **`reconcile_liveness(liveness)`** — check every `Running` record against the
  oracle and flip dead ones to `Lost`; returns only the records that
  transitioned (empty means nothing changed).
- **When to use:** track background/foreground job lifecycle; reconcile before
  surfacing job status to a user.

Related: [[swarm-core#jobspec]], [[swarm-core#processliveness]], [[swarm-contracts#jobrecord]], [[swarm-store#filejobrepo--memjobrepo]].

### JobSpec

```rust
pub struct JobSpec {
    pub agent: JobAgent,
    pub model: Option<String>,
    pub mode: JobMode,
    pub cwd: PathBuf,
    pub prompt_preview: String,  // stored verbatim — NOT re-truncated
    pub prompt_text: String,     // full text → .prompt.md sidecar
    pub timeout_secs: u64,
    pub allow_recursive_codex: bool,
}
```

Typed create-request for [`JobRepo::create`](#jobrepo) — replaces a long
positional parameter list. `prompt_preview` is stored as-given; `prompt_text`
goes to the `.prompt.md` sidecar file.

Related: [[swarm-core#jobrepo]], [[swarm-contracts#jobstatus--jobagent--jobmode]].

### EventRepo

```rust
pub trait EventRepo: Send + Sync {
    fn append(
        &self,
        session: &SessionId,
        kind: EventKind,
        payload: serde_json::Value,
        ctx: EventContext,
    ) -> Result<Cursor, RepoError>;

    fn append_layer_report(
        &self,
        session: &SessionId,
        spec: LayerReportSpec,
    ) -> Result<Cursor, RepoError>;

    fn events_since(
        &self,
        session: &SessionId,
        after: Cursor,
        limit: usize,
    ) -> Result<(Vec<StoredEvent>, Cursor), RepoError>;

    fn tail(&self, session: &SessionId, limit: usize) -> Result<Vec<StoredEvent>, RepoError>;

    fn latest_kind(&self, session: &SessionId) -> Result<Option<EventKind>, RepoError>;
}
```

Pure storage abstraction for session event appends and reads. Mutating methods
return a [`Cursor`](#cursor) positioned past the written event so the caller can
resume reading immediately.

- **`append(session, kind, payload, ctx)`** — append a typed event; returns the
  post-write cursor.
- **`append_layer_report(session, spec)`** — write the `.md` sidecar, the
  `layer_reports.jsonl` index line, and the matching `LayerReport` event
  atomically under one lock acquisition.
- **`events_since(session, after, limit)`** — return events strictly after
  `after`, up to `limit`, plus a new cursor; the incremental tail primitive.
- **`tail(session, limit)`** — return the last `limit` events (most-recent tail).
- **`latest_kind(session)`** — the most recent event's kind, feeding
  [`SessionStatusDeriver::derive`](#sessionstatusderiver).
- **When to use:** any time you write to or read from a session's event log;
  thread the returned cursor for live tailing.

Related: [[swarm-core#eventcontext]], [[swarm-core#layerreportspec]], [[swarm-core#storedevent]], [[swarm-core#cursor]], [[swarm-contracts#eventkind]], [[swarm-store#fileeventrepo--memeventrepo]].

### EventContext

```rust
#[derive(Debug, Clone)]
pub struct EventContext {
    pub parent_id: Option<String>,
    pub agent_id: String,
    pub role: String,
    pub phase: String,
}
// Default: { parent_id: None, agent_id: "auto", role: "participant", phase: "discussion" }
```

Contextual metadata attached to each emitted event, mirroring the
`parent_id`/`agent_id`/`role`/`phase` fields on the `agent-swarm/event/v2` wire
envelope. Pass to [`EventRepo::append`](#eventrepo); `Default` supplies sensible
auto values.

Related: [[swarm-core#eventrepo]], [[swarm-core#storedevent]].

### LayerReportSpec

```rust
#[derive(Debug, Clone)]
pub struct LayerReportSpec {
    pub layer: String,
    pub role: String,
    pub agent: String,
    pub parent_role: Option<String>,
    pub status: String,
    pub text: String,
}
```

Input to [`EventRepo::append_layer_report`](#eventrepo). Carries everything
needed to write the markdown sidecar, the index line, and the `LayerReport`
event in one locked operation.

Related: [[swarm-core#eventrepo]].

### StoredEvent

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredEvent {
    pub session_id: String,
    pub kind: EventKind,
    pub payload: serde_json::Value,
    pub ts_ms: u128,
    pub seq: u64,
    pub parent_id: Option<String>,
    pub agent_id: String,
    pub role: String,
    pub phase: String,
}
```

Owned read model for one `agent-swarm/event/v2` line. Round-trips the existing
`events.jsonl` format without altering the wire bytes; `payload` stays a
`serde_json::Value` (a typed payload union is deferred). Returned by
[`EventRepo::events_since`](#eventrepo) and [`EventRepo::tail`](#eventrepo).

Related: [[swarm-core#eventrepo]], [[swarm-contracts#eventkind]].

### LedgerRepo

```rust
pub trait LedgerRepo: Send + Sync {
    fn record_task(&self, task: LedgerTask) -> Result<(), RepoError>;
    fn tasks(&self) -> Result<Vec<LedgerTask>, RepoError>;
}
```

Append-only task ledger (the metadirector virtual-context "T2"). Writes append a
snapshot and never mutate prior rows; `tasks()` returns current state — the
latest snapshot per `id`, in first-seen order (folded via
[`fold_tasks`](#fold_tasks)).

- **`record_task(task)`** — append one snapshot; a status change is a new row
  with the same `id`.
- **`tasks()`** — current per-id state.
- **When to use:** track sub-tasks of a metadirector run as they move
  `open → claimed → claimed_done → verified_done`.

Related: [[swarm-core#ledgertask]], [[swarm-core#ledgerstatus]], [[swarm-core#fold_tasks]], [[swarm-store#fileledgerrepo--memledgerrepo]].

### LedgerTask

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedgerTask {
    pub schema: String,          // == LEDGER_TASK_SCHEMA
    pub id: String,
    pub intent: String,
    pub status: LedgerStatus,
    pub owner_agent: Option<String>,   // serde default
    pub depends_on: Vec<String>,       // serde default
    pub created_at_ms: u128,
    pub closed_at_ms: Option<u128>,    // serde default
    pub validation_anchor: Option<String>, // serde default
    pub synthesis_depth: u32,          // serde default
}

impl LedgerTask {
    pub fn new(id: impl Into<String>, intent: impl Into<String>, created_at_ms: u128) -> Self;
    pub fn is_verified(&self) -> bool;
    pub fn is_unverified_done(&self) -> bool;
}
```

One task-snapshot row. The optional fields carry `#[serde(default)]` so older
rows deserialize forward-compatibly.

- **`new(id, intent, created_at_ms)`** — a fresh `Open` task with no owner,
  deps, or anchor.
- **`is_verified()`** — `true` only when `status == VerifiedDone` **and** a
  non-empty `validation_anchor` is present (spec: done is only trustworthy with
  an anchor).
- **`is_unverified_done()`** — `true` when status is `ClaimedDone`/`VerifiedDone`
  but the anchor is missing/blank — surface as UNVERIFIED rather than trusted.
- **When to use:** build snapshots to feed [`LedgerRepo::record_task`](#ledgerrepo);
  use the two predicates to gate trust on the folded current state.

Related: [[swarm-core#ledgerrepo]], [[swarm-core#ledgerstatus]], [[swarm-core#ledger_task_schema]].

### LedgerStatus

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LedgerStatus { Open, Claimed, ClaimedDone, VerifiedDone }

impl LedgerStatus {
    pub fn as_str(self) -> &'static str;       // "open" | "claimed" | "claimed_done" | "verified_done"
    pub fn parse(token: &str) -> Option<Self>; // inverse of as_str
    pub fn is_active(self) -> bool;            // everything except VerifiedDone
}
```

Lifecycle status of a ledger task. `as_str`/`parse` are exact inverses over the
snake_case wire tokens; `is_active()` returns `true` for everything that still
occupies the metadirector's working set (i.e. anything not `VerifiedDone`).

Related: [[swarm-core#ledgertask]], [[swarm-core#ledgerrepo]].

### fold_tasks

```rust
pub fn fold_tasks(snapshots: Vec<LedgerTask>) -> Vec<LedgerTask>;
```

Collapse an append-only snapshot log into current state: keep the latest
snapshot per `id`, preserving first-seen order. The pure transform behind
[`LedgerRepo::tasks`](#ledgerrepo).

- **Params:** all snapshots in append order. **Returns:** one row per distinct
  `id` (the most recent), ordered by when each id was first seen.
- **When to use:** whenever you have raw ledger snapshots and need the
  deduplicated current view.

Related: [[swarm-core#ledgertask]], [[swarm-core#ledgerrepo]].

### LEDGER_TASK_SCHEMA

```rust
pub const LEDGER_TASK_SCHEMA: &str = "agent-swarm/ledger-task/v1";
```

The schema string stamped into every [`LedgerTask::schema`](#ledgertask) and
used to validate ledger rows.

Related: [[swarm-core#ledgertask]].

### TelemetryRepo

```rust
pub trait TelemetryRepo: Send + Sync {
    fn record_observation(&self, obs: AgentObservation) -> Result<(), RepoError>;
    fn record_feedback(&self, fb: AgentFeedback) -> Result<(), RepoError>;
    fn record_proposal(&self, prop: AgentProposal) -> Result<(), RepoError>;
    fn record_proposal_vote(&self, vote: AgentProposalVote) -> Result<(), RepoError>;

    fn observations(&self) -> Result<Vec<AgentObservation>, RepoError>;
    fn feedback(&self) -> Result<Vec<AgentFeedback>, RepoError>;
    fn proposals(&self) -> Result<Vec<AgentProposal>, RepoError>;
    fn proposal_votes(&self) -> Result<Vec<AgentProposalVote>, RepoError>;
}
```

Append-only storage for agent telemetry — observations, feedback, proposals, and
votes. Each `record_*` method appends one entry; each reader returns the full
collection in append order (FIFO). The four payload types are the canonical
`swarm-contracts::telemetry` records.

- **When to use:** persist routing telemetry that higher layers
  (`swarm-kernel::telemetry`) aggregate into per-agent stats and recommendations.

Related: [[swarm-contracts#agentobservation--agentfeedback--agentproposal--agentproposalvote]], [[swarm-store#filetelemetryrepo--memtelemetryrepo]].
