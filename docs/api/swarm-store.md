# swarm-store

File-backed and in-memory implementations of the `swarm-core` repo traits, plus the low-level store primitives (swarm-home path resolution, atomic writes, id minting), monitor/alert storage, and the production OS process-liveness oracle.

## Overview

`swarm-store` is where the engine's persistence lives. `swarm-core` defines *what* a session/job/event/ledger/telemetry store must do (traits + companion types); this crate provides the *how* — two concrete backends for each trait:

- **`File*` repos** — durable, on-disk under the swarm home (`<swarm home>/sessions`, `/jobs`, `/ledger`, `/telemetry`). Sessions are directories of JSON + JSONL; jobs are one pretty-printed JSON file per id; events, layer reports, ledger snapshots and telemetry records are append-only NDJSON.
- **`Mem*` repos** — in-process `Mutex<…>` doubles with byte-for-byte identical observable semantics (same id-sorted listing order, same fold-to-current-state logic), so tests and replay harnesses can stand in for the file repos without touching disk.

It also owns the cross-cutting store primitives every higher crate calls to find and write data: the `swarm_home()` resolver and its derived directories, `write_text_atomic` (sibling-temp-file + rename), monotonic id minting (`new_job_id`/`new_session_id`), and the durable monitor state used by the sidecar resource monitor.

The crate is deliberately dependency-light: it depends only on `swarm-core`, `swarm-contracts`, and serde — no external-system crates. Process-liveness and a handful of pure file helpers that would otherwise pull in heavier crates are inlined as private modules (`process_helpers`, `ledger_helpers`, `telemetry_helpers`).

**Encapsulation invariant:** the three write-synchronization statics (`EVENT_LOG_LOCK`, `ATOMIC_WRITE_COUNTER`, `EVENT_SEQ_COUNTER`) are `pub(crate)` and are *not* re-exported. Only this crate's `event_repo`/`session_repo` acquire them; the boundary is enforced structurally by `ci/law-checks.sh` CHECK 7.

## Dependency position

```
swarm-contracts → swarm-core → [ swarm-store ] → swarm-kernel → swarm-exec → swarm-mcp/swarm-cli
```

- **Depends on:** [[swarm-core]] (repo traits + companion types: `SessionRepo`, `JobRepo`, `EventRepo`, `LedgerRepo`, `TelemetryRepo`, `RepoError`, `Cursor`, `ProcessLiveness`, …), [[swarm-contracts]] (wire types: `JobRecord`, `SessionId`/`JobId`, `EventKind`, `SessionEventV2`, `LayerReportEnvelope`, telemetry records), and `serde`/`serde_json`.
- **Depended on by:** [[swarm-kernel]], [[swarm-exec]], [[swarm-mcp]], [[swarm-cli]] — anything that needs to read or write runtime state.

The `swarm-core` trait/companion types are re-exported from `repos::mod` (and the per-repo modules) for caller convenience, so a consumer can pull both the trait and its implementation from `swarm_store`.

## Concepts

### Store primitives — `store.rs`
- [`swarm_home`](#swarm_home) — resolve the single data root (`$SWARM_HOME` or `$HOME/.swarm`).
- [`providers_dir`](#providers_dir) — `<swarm home>/providers`.
- [`skills_dir`](#skills_dir) — `<swarm home>/skills`.
- [`swarm_home_err`](#swarm_home_err) — shared "cannot resolve data root" error string.
- [`job_store_dir`](#job_store_dir) — `<swarm home>/jobs`.
- [`session_store_dir`](#session_store_dir) — `<swarm home>/sessions`.
- [`session_dir`](#session_dir) — a single validated session directory.
- [`new_job_id`](#new_job_id) — mint an opaque `JobId`.
- [`new_session_id`](#new_session_id) — mint an opaque, collision-resistant `SessionId`.
- [`now_ms`](#now_ms) — current Unix time in milliseconds.
- [`write_text_atomic`](#write_text_atomic) — crash-safe write via sibling temp file + rename.
- [`read_text_tail`](#read_text_tail) — read the last `max_bytes` of a file, line-aligned.
- [`validate_store_id`](#validate_store_id) — reject ids unsafe to join into paths.
- [`MAX_SESSION_EVENTS_TAIL_BYTES`](#byte-caps) / [`MAX_ARTIFACT_TEXT_BYTES`](#byte-caps) — read/scan size caps.

### Job record helpers — `job.rs`
- [`JobRecord`](#jobrecord-re-export) — re-export of the persisted per-job record.
- [`create_tracking_record_in`](#create_tracking_record_in) — mint a job id, write the prompt file, and persist a `Running` record.
- [`list_job_records_in`](#list_job_records_in) — read all job records in a dir, id-sorted.
- [`read_job_record_in`](#read_job_record_in) — read one job record by id.
- [`write_job_record_in`](#write_job_record_in) — atomically (re)write a job record.

### Monitor store — `monitor_store.rs`
- [`MonitorOptions`](#monitoroptions) — sidecar-monitor threshold config.
- [`monitor_store_dir`](#monitor-paths) / [`monitor_status_path`](#monitor-paths) / [`monitor_alerts_path`](#monitor-paths) — monitor file paths.
- [`write_monitor_status`](#write_monitor_status) — write the monitor heartbeat document.
- [`read_monitor_pid`](#read_monitor_pid) — read the running monitor's pid.
- [`append_monitor_alert`](#append_monitor_alert) — append an alert (rotating the log when oversized).
- [`alerts_json`](#alerts_json) — assemble the alert-list payload (with `running` liveness flag).
- [`file_modified_ms`](#file_modified_ms) — file mtime in ms (used for stale detection).
- [Monitor defaults](#monitor-defaults) — `DEFAULT_MONITOR_INTERVAL_SECS` / `_RSS_MB` / `_SPIKE_FACTOR` / `_STALE_SECS`.

### Repo implementations — `repos/`
- [`FileSessionRepo`](#filesessionrepo) / [`MemSessionRepo`](#memsessionrepo) — `SessionRepo` backends.
- [`FileJobRepo`](#filejobrepo) / [`MemJobRepo`](#memjobrepo) — `JobRepo` backends.
- [`FileEventRepo`](#fileeventrepo) / [`MemEventRepo`](#memeventrepo) — `EventRepo` backends.
- [`FileLedgerRepo`](#fileledgerrepo) / [`MemLedgerRepo`](#memledgerrepo) / [`default_file_ledger_repo`](#default_file_ledger_repo) — `LedgerRepo` backends.
- [`FileTelemetryRepo`](#filetelemetryrepo) / [`MemTelemetryRepo`](#memtelemetryrepo) / [`default_file_telemetry_repo`](#default_file_telemetry_repo) — `TelemetryRepo` backends.
- [`OsProcessLiveness`](#osprocessliveness) — production `ProcessLiveness` oracle.
- [Re-exported trait + companion types](#re-exported-core-types) — `swarm-core` traits/types surfaced through `repos`.

## API surface

### `swarm_home`

```rust
pub fn swarm_home() -> Option<PathBuf>
```

Resolves the single data root for all runtime state. `$SWARM_HOME` (treated as an absolute path) wins when set and non-empty; otherwise `$HOME/.swarm`. Returns `None` only when neither env var is available.

The home contains: `jobs/`, `sessions/`, `monitor/`, `ledger/`, `telemetry/`, `evals/`, `providers/`, `conductor-sessions/`, `conductor-policy.json`, and `bin/`.

**When to use:** as the base for any new store directory. Every other path helper here is derived from it.

Related: [[swarm-store#swarm_home_err]], [[swarm-store#job_store_dir]], [[swarm-store#session_store_dir]], [[swarm-store#providers_dir]], [[swarm-store#skills_dir]]

### `providers_dir`

```rust
pub fn providers_dir() -> Option<PathBuf>
```

Returns `<swarm home>/providers`, or `None` if the home cannot be resolved. Single source of truth shared by the native backend and the `provider` CLI so both operate on the same provider registry.

Related: [[swarm-store#swarm_home]]

### `skills_dir`

```rust
pub fn skills_dir() -> Option<PathBuf>
```

Returns `<swarm home>/skills`, or `None` if the home cannot be resolved. The user-global source of `SKILL.md` skills; a native run also consults a project-local `<cwd>/.swarm/skills` that overrides this one.

Related: [[swarm-store#swarm_home]]

### `swarm_home_err`

```rust
pub fn swarm_home_err() -> String
```

The shared error string for callers that need to fail when `swarm_home()` returns `None`: `"Error: cannot resolve the swarm home (set SWARM_HOME or HOME)"`. Used with `.ok_or_else(swarm_home_err)`.

Related: [[swarm-store#swarm_home]]

### `job_store_dir`

```rust
pub fn job_store_dir() -> Result<PathBuf, String>
```

Returns `<swarm home>/jobs`, erroring with [`swarm_home_err`](#swarm_home_err) when the home is unresolvable. This is the directory the file job repo is normally pointed at.

Related: [[swarm-store#FileJobRepo]], [[swarm-store#create_tracking_record_in]]

### `session_store_dir`

```rust
pub fn session_store_dir() -> Result<PathBuf, String>
```

Returns `<swarm home>/sessions`, erroring with [`swarm_home_err`](#swarm_home_err) when unresolvable. The base dir for the file session/event repos.

Related: [[swarm-store#session_dir]], [[swarm-store#FileSessionRepo]]

### `session_dir`

```rust
pub fn session_dir(id: &str) -> Result<PathBuf, String>
```

Returns the directory for a single session: `<swarm home>/sessions/<id>`. Validates `id` with [`validate_store_id`](#validate_store_id) before joining, so a malicious id cannot escape the store. Errors if validation fails or the home is unresolvable.

Related: [[swarm-store#session_store_dir]], [[swarm-store#validate_store_id]]

### `new_job_id`

```rust
pub fn new_job_id() -> swarm_contracts::ids::JobId
```

Mints an opaque job id of the form `job-{hex_ms}-{pid}`. The format is an implementation detail — treat the value as opaque.

Related: [[swarm-store#create_tracking_record_in]], [[swarm-contracts#JobId]]

### `new_session_id`

```rust
pub fn new_session_id() -> swarm_contracts::ids::SessionId
```

Mints an opaque session id of the form `session-{hex_ms}-{pid}-{counter}`. The trailing counter (from the crate-private `ATOMIC_WRITE_COUNTER`) disambiguates two sessions created in the same millisecond — common in tests. Treat the value as opaque.

Note: `FileSessionRepo::create` and `MemSessionRepo::create` mint ids inline using the same scheme rather than calling this function.

Related: [[swarm-store#new_job_id]], [[swarm-contracts#SessionId]]

### `now_ms`

```rust
pub fn now_ms() -> u128
```

Current wall-clock time as Unix epoch milliseconds. Saturates to `0` on a clock error rather than panicking. The single timestamp source used across records, events, and ids in this crate.

### `write_text_atomic`

```rust
pub fn write_text_atomic(path: &Path, contents: impl AsRef<[u8]>) -> Result<(), String>
```

Writes `contents` to `path` crash-safely: create the parent dir, write to a uniquely-named sibling temp file (`.{name}.{pid}.{ms}.{nonce}.tmp`), then `rename` over the target. On rename failure the temp file is removed. A reader of `path` therefore never observes a partially-written file. The per-write nonce comes from the crate-private `ATOMIC_WRITE_COUNTER`.

**When to use:** any time you persist a whole-file document (session metadata, job records, monitor status). Use append-mode writes instead for NDJSON logs.

Related: [[swarm-store#write_job_record_in]], [[swarm-store#write_monitor_status]]

### `read_text_tail`

```rust
pub fn read_text_tail(path: &Path, max_bytes: usize) -> Result<String, String>
```

Reads at most the last `max_bytes` of a file. If the file is larger than `max_bytes`, the read starts mid-file and the first (likely partial) line is dropped so every returned line is complete. Used to scan recent NDJSON tails (events, alerts) without loading the whole file.

Related: [[swarm-store#byte-caps]], [[swarm-store#alerts_json]]

### `validate_store_id`

```rust
pub fn validate_store_id(id: &str) -> Result<(), String>
```

Validates an id before it is joined into a store path. Accepts only ASCII letters, digits, `-`, and `_`; rejects path separators, dots, and anything else. Returns `Err("Error: invalid id `{id}`")` on rejection. A trust-boundary guard against path traversal in user/agent-supplied ids.

**When to use:** before constructing any filesystem path from a caller-supplied id (the session/job path helpers already call it for you).

Related: [[swarm-store#session_dir]], [[swarm-store#read_job_record_in]]

### byte-caps

```rust
pub const MAX_SESSION_EVENTS_TAIL_BYTES: usize = 512 * 1024; // 512 KiB
pub const MAX_ARTIFACT_TEXT_BYTES: usize = 2 * 1024 * 1024;  // 2 MiB
```

`MAX_SESSION_EVENTS_TAIL_BYTES` caps how much of an `events.jsonl` tail is scanned for recent events / latest kind. `MAX_ARTIFACT_TEXT_BYTES` caps the size of a single artifact/record that will be read (oversized job records and alert tails are skipped or truncated). Together they bound the memory cost of any read path.

Related: [[swarm-store#read_text_tail]], [[swarm-store#read_job_record_in]]

---

### `JobRecord` (re-export)

```rust
pub use swarm_contracts::jobs::JobRecord;
```

Re-export of the canonical persisted per-job record (id, status, agent, model, mode, cwd, prompt preview, timing, pid, exit code, and the four artifact paths). Defined in [[swarm-contracts#JobRecord]]; surfaced here so job-helper callers can name the type without depending on `swarm-contracts` directly.

Related: [[swarm-store#create_tracking_record_in]], [[swarm-contracts#JobRecord]]

### `create_tracking_record_in`

```rust
pub fn create_tracking_record_in(
    job_dir: &Path,
    agent: JobAgent,
    model: Option<String>,
    mode: JobMode,
    cwd: &Path,
    prompt_for_preview: &str,
    prompt_for_file: &str,
    timeout_secs: u64,
    allow_recursive_codex: bool,
) -> Result<JobRecord, String>
```

Creates a new tracked job in `job_dir`: mints a job id, writes `prompt_for_file` to `<id>.prompt.md`, and persists a fresh `JobRecord` with `status = Running`, `created_at_ms == started_at_ms`, `pid = current process`, and the four derived artifact paths (`.prompt.md`, `.stdout.log`, `.stderr.log`, `.result.txt`). `prompt_for_preview` is stored **verbatim** — the caller must pre-truncate it.

**When to use:** the file-backed implementation behind `JobRepo::create`; call it (or the repo method) to start tracking a background job.

Related: [[swarm-store#FileJobRepo]], [[swarm-core#JobSpec]], [[swarm-contracts#JobStatus]]

### `list_job_records_in`

```rust
pub fn list_job_records_in(dir: &Path) -> Result<Vec<JobRecord>, String>
```

Reads every `*.json` job record in `dir`, skipping files that are unparseable or larger than [`MAX_ARTIFACT_TEXT_BYTES`](#byte-caps). A missing directory yields an empty `Vec`, not an error. Results are sorted by id (ascending) for deterministic enumeration regardless of OS directory order — this id order is the reproducible same-millisecond tiebreak when callers re-sort by `created_at_ms` with a stable sort.

Related: [[swarm-store#FileJobRepo]], [[swarm-store#read_job_record_in]]

### `read_job_record_in`

```rust
pub fn read_job_record_in(dir: &Path, id: &str) -> Result<JobRecord, String>
```

Reads a single job record `<dir>/<id>.json`. Validates `id` first, rejects records over [`MAX_ARTIFACT_TEXT_BYTES`](#byte-caps), and parses the JSON. The error message is load-bearing: an ENOENT failure produces a message containing `os error 2` / `No such file`, which `FileJobRepo::get` maps to `RepoError::NotFound` (any other I/O error maps to `RepoError::Io`).

Related: [[swarm-store#FileJobRepo]], [[swarm-store#validate_store_id]], [[swarm-core#RepoError]]

### `write_job_record_in`

```rust
pub fn write_job_record_in(dir: &Path, record: &JobRecord) -> Result<(), String>
```

Atomically writes `record` to `<dir>/<record.id>.json` (pretty-printed, trailing newline) via a `.json.tmp` sibling + rename. Validates the record id and creates the directory if needed.

**When to use:** to persist any job state transition (e.g. flipping `Running` → `Completed`/`Lost`).

Related: [[swarm-store#create_tracking_record_in]], [[swarm-store#FileJobRepo]]

---

### `MonitorOptions`

```rust
pub struct MonitorOptions {
    pub interval_secs: u64,
    pub rss_threshold_bytes: u64,
    pub spike_factor: f64,
    pub stale_secs: u64,
}
```

Threshold configuration for the sidecar resource monitor: poll cadence, the absolute RSS ceiling that triggers an alert, the relative growth factor that flags a memory spike, and the age after which a heartbeat is considered stale. Persisted into the monitor status document by [`write_monitor_status`](#write_monitor_status).

Related: [[swarm-store#monitor-defaults]], [[swarm-store#write_monitor_status]]

### monitor-paths

```rust
pub fn monitor_store_dir() -> Result<PathBuf, String>   // <swarm home>/monitor
pub fn monitor_status_path() -> Result<PathBuf, String>  // <…>/monitor/monitor.json
pub fn monitor_alerts_path() -> Result<PathBuf, String>  // <…>/monitor/alerts.ndjson
```

Resolve the monitor's directory, its heartbeat status file, and its append-only alert log. All three error with [`swarm_home_err`](#swarm_home_err) when the home is unresolvable.

Related: [[swarm-store#write_monitor_status]], [[swarm-store#append_monitor_alert]]

### `write_monitor_status`

```rust
pub fn write_monitor_status(pid: u32, options: &MonitorOptions) -> Result<(), String>
```

Atomically writes the monitor heartbeat document (`schema: "agent-swarm/monitor-status/v1"`) to [`monitor_status_path`](#monitor-paths): the monitor's `pid`, `started_at_ms`, the four threshold fields from `options`, and the alerts path. Establishes the running monitor's identity so [`read_monitor_pid`](#read_monitor_pid) can find it.

Related: [[swarm-store#MonitorOptions]], [[swarm-store#read_monitor_pid]], [[swarm-store#write_text_atomic]]

### `read_monitor_pid`

```rust
pub fn read_monitor_pid() -> Result<Option<u32>, String>
```

Reads the `pid` field from the monitor status file. Returns `Ok(None)` when the file is absent or the `pid` field is missing/out of range; errors only on malformed JSON. Used to tell whether a monitor is registered and (combined with a liveness check) whether it is still running.

Related: [[swarm-store#write_monitor_status]], [[swarm-store#alerts_json]], [[swarm-store#OsProcessLiveness]]

### `append_monitor_alert`

```rust
pub fn append_monitor_alert(alert: &serde_json::Value) -> Result<(), String>
```

Appends one alert as an NDJSON line to [`monitor_alerts_path`](#monitor-paths), creating the directory if needed. Before appending it rotates the log when it exceeds an internal cap (2,000 lines) by renaming the current file to `alerts.ndjson.1` (replacing any prior rotation). Bounds unbounded alert-log growth.

Related: [[swarm-store#alerts_json]], [[swarm-store#monitor-paths]]

### `alerts_json`

```rust
pub fn alerts_json(since_ts_ms: Option<u128>, limit: usize) -> Result<serde_json::Value, String>
```

Builds the alert-list payload (`schema: "agent-swarm/monitor-alerts/v1"`): reads the alert-log tail, parses NDJSON lines (skipping malformed ones), optionally filters to alerts with `ts_ms >= since_ts_ms`, keeps at most `limit` of the most-recent, and returns them in chronological order. Also reports `running` (the registered monitor's pid checked against [`OsProcessLiveness`](#osprocessliveness)) and the log `path`.

**When to use:** to answer "what alerts has the monitor raised and is it alive?" for the CLI/MCP alerts views.

Related: [[swarm-store#append_monitor_alert]], [[swarm-store#read_monitor_pid]], [[swarm-store#read_text_tail]]

### `file_modified_ms`

```rust
pub fn file_modified_ms(path: &Path) -> Option<u128>
```

Returns a file's last-modified time as Unix epoch milliseconds, or `None` if the file is missing or its mtime is unavailable. Used by the monitor to detect stale/idle session activity against `MonitorOptions::stale_secs`.

Related: [[swarm-store#MonitorOptions]]

### monitor-defaults

```rust
pub const DEFAULT_MONITOR_INTERVAL_SECS: u64 = 5;
pub const DEFAULT_MONITOR_RSS_MB: u64 = 4 * 1024;   // 4096 MiB
pub const DEFAULT_MONITOR_SPIKE_FACTOR: f64 = 2.5;
pub const DEFAULT_MONITOR_STALE_SECS: u64 = 300;
```

Default monitor thresholds: poll every 5s, alert at 4 GiB RSS, flag a 2.5× memory spike, treat a heartbeat older than 300s as stale. Seed values for [`MonitorOptions`](#monitoroptions).

Related: [[swarm-store#MonitorOptions]]

---

### `FileSessionRepo`

```rust
pub struct FileSessionRepo { /* base_dir */ }

impl FileSessionRepo {
    pub fn new(base_dir: impl Into<PathBuf>) -> Self;
}

impl SessionRepo for FileSessionRepo {
    type Events = FileEventRepo;
    fn create(&self, spec: SessionSpec) -> Result<SessionHandle<FileEventRepo>, RepoError>;
    fn open(&self, id: &SessionId) -> Result<SessionHandle<FileEventRepo>, RepoError>;
    fn list(&self) -> Result<Vec<SessionIndexRecord>, RepoError>;
    fn write_metadata(&self, id: &SessionId, meta: &SessionMeta) -> Result<(), RepoError>;
    fn summary(&self, id: &SessionId) -> Result<SessionSummary, RepoError>;
    fn artifacts(&self, id: &SessionId) -> Result<Vec<SessionArtifact>, RepoError>;
}
```

The on-disk session store, rooted at `base_dir` (normally [`session_store_dir`](#session_store_dir)). Each session is a directory `<base_dir>/<id>/` holding `session.json` (metadata), `events.jsonl`, and rendered artifacts (`transcript.md`, `summary.md`, `digest.md`, `api-docs.md`, `layer-reports.jsonl`, and a `layer-reports/` dir of per-report `.md` files).

- `create` mints a session id, emits a `Created` event, and returns a `SessionHandle` whose `events` is a [`FileEventRepo`](#fileeventrepo).
- `open` errors `NotFound` if the session directory is absent.
- `list` reads each session's `session.json` and returns id-sorted [`SessionIndexRecord`](#re-exported-core-types)s for deterministic enumeration (it does **not** consult process liveness).
- `summary` assembles the `agent-swarm/session-summary/v1` payload — status (via `SessionStatusDeriver` + [`OsProcessLiveness`](#osprocessliveness)), recent events, and previews of digest/summary/docs.
- `artifacts` lists the present artifact files with label, path, mime, and byte size.

**When to use:** the production session backend behind any `SessionRepo` consumer.

Related: [[swarm-store#MemSessionRepo]], [[swarm-store#FileEventRepo]], [[swarm-core#SessionRepo]], [[swarm-store#OsProcessLiveness]]

### `MemSessionRepo`

```rust
pub struct MemSessionRepo { /* Mutex<HashMap<String, …>> */ }

impl MemSessionRepo {
    pub fn new() -> Self;        // also: Default
}

impl SessionRepo for MemSessionRepo {
    type Events = MemEventRepo;
    // same method set as FileSessionRepo
}
```

In-memory `SessionRepo` double backed by a `Mutex<HashMap<…>>`, with [`MemEventRepo`](#memeventrepo) as its event store. `list` is id-sorted to match `FileSessionRepo` for test/replay parity; `summary` returns the same `agent-swarm/session-summary/v1` schema field (without on-disk artifact bytes), and `artifacts` returns an empty list. `write_metadata` updates the entry's pid and prompt preview.

**When to use:** tests and harnesses that need session semantics without disk I/O.

Related: [[swarm-store#FileSessionRepo]], [[swarm-store#MemEventRepo]], [[swarm-core#SessionRepo]]

### `FileJobRepo`

```rust
pub struct FileJobRepo { /* base_dir */ }

impl FileJobRepo {
    pub fn new(base_dir: impl Into<PathBuf>) -> Self;
}

impl JobRepo for FileJobRepo {
    fn create(&self, spec: JobSpec) -> Result<JobRecord, RepoError>;
    fn get(&self, id: &JobId) -> Result<JobRecord, RepoError>;
    fn list(&self) -> Result<Vec<JobRecord>, RepoError>;
    fn save(&self, record: &JobRecord) -> Result<(), RepoError>;
    fn latest(&self) -> Result<Option<JobRecord>, RepoError>;
    fn reconcile_liveness(&self, liveness: &dyn ProcessLiveness) -> Result<Vec<JobRecord>, RepoError>;
}
```

On-disk `JobRepo`, rooted at `base_dir` (normally [`job_store_dir`](#job_store_dir)). Delegates to the [`job.rs`](#create_tracking_record_in) helpers: `create` calls `create_tracking_record_in`, `get`/`save` map I/O errors via the ENOENT → `NotFound` rule, and `list`/`latest` sort by `(created_at_ms, id)`. `reconcile_liveness` flips any `Running` record whose pid is dead (or `0`) to `Lost` (sets `completed_at_ms`, `exit_code = 1`), writing a breadcrumb to the record's stderr file if none exists, and returns the flipped records.

**When to use:** the production job backend; `reconcile_liveness` is how stale "running" jobs from crashed workers get cleaned up on inspection.

Related: [[swarm-store#MemJobRepo]], [[swarm-store#create_tracking_record_in]], [[swarm-core#JobRepo]], [[swarm-store#OsProcessLiveness]]

### `MemJobRepo`

```rust
pub struct MemJobRepo { /* base_dir + Mutex<HashMap<JobId, JobRecord>> */ }

impl MemJobRepo {
    pub fn new(base_dir: impl Into<PathBuf>) -> Self;
}

impl JobRepo for MemJobRepo { /* same method set as FileJobRepo */ }
```

In-memory `JobRepo` double. Still takes a `base_dir` so the artifact paths it synthesizes into each `JobRecord` match what the file repo would produce (the files themselves are not written). `list` is id-sorted and `reconcile_liveness` applies the same `Running` → `Lost` flip logic as `FileJobRepo`, making it a faithful stand-in for replay/test harnesses.

Related: [[swarm-store#FileJobRepo]], [[swarm-core#JobRepo]]

### `FileEventRepo`

```rust
pub struct FileEventRepo { /* base_dir */ }

impl FileEventRepo {
    pub fn new(base_dir: impl Into<PathBuf>) -> Self;
}

impl EventRepo for FileEventRepo {
    fn append(&self, session: &SessionId, kind: EventKind, payload: serde_json::Value, ctx: EventContext) -> Result<Cursor, RepoError>;
    fn append_layer_report(&self, session: &SessionId, spec: LayerReportSpec) -> Result<Cursor, RepoError>;
    fn events_since(&self, session: &SessionId, after: Cursor, limit: usize) -> Result<(Vec<StoredEvent>, Cursor), RepoError>;
    fn tail(&self, session: &SessionId, limit: usize) -> Result<Vec<StoredEvent>, RepoError>;
    fn latest_kind(&self, session: &SessionId) -> Result<Option<EventKind>, RepoError>;
}
```

JSONL-backed event store, rooted at the session base dir; writes `<base>/<session>/events.jsonl` (and `layer-reports.jsonl` + per-report `.md` files). All appends acquire the crate-private `EVENT_LOG_LOCK` and stamp a monotonic `seq` from `EVENT_SEQ_COUNTER`, wrapping each event in a `agent-swarm/event/v2` envelope ([[swarm-contracts#SessionEventV2]]). Oversized payloads are compacted to fit `MAX_EVENT_LINE_BYTES` (128 KiB). `append_layer_report` additionally renders a `.md` report and emits a `LayerReport` event referencing it.

**Cursor contract:** for this backend the `Cursor` is a **byte offset** of the last confirmed complete `\n`. `events_since(after, limit)` resumes from that offset and returns at most `limit` events plus the new cursor; `tail` reads the most-recent `limit` events from the file tail; `latest_kind` returns the kind of the last event (or `Other(...)` for an unknown wire token).

**When to use:** the production event log behind a session; obtained via `SessionHandle::events` from [`FileSessionRepo`](#filesessionrepo).

Related: [[swarm-store#MemEventRepo]], [[swarm-core#EventRepo]], [[swarm-core#Cursor]], [[swarm-contracts#SessionEventV2]], [[swarm-contracts#EventKind]]

### `MemEventRepo`

```rust
pub struct MemEventRepo { /* Mutex<HashMap<String, Vec<StoredEvent>>> */ }

impl MemEventRepo {
    pub fn new() -> Self;        // also: Default
}

impl EventRepo for MemEventRepo { /* same method set as FileEventRepo */ }
```

In-memory `EventRepo` double: a per-session `Vec<StoredEvent>` guarded by a `Mutex`. Stamps the same monotonic `seq` from `EVENT_SEQ_COUNTER` as the file repo. **Cursor contract:** here the `Cursor` is a **Vec index** of the last returned event rather than a byte offset. Otherwise observable semantics match `FileEventRepo`.

Related: [[swarm-store#FileEventRepo]], [[swarm-core#EventRepo]], [[swarm-core#Cursor]]

### `FileLedgerRepo`

```rust
pub struct FileLedgerRepo { /* dir */ }

impl FileLedgerRepo {
    pub fn new(dir: PathBuf) -> Self;
    pub fn dir(&self) -> &PathBuf;
}

impl LedgerRepo for FileLedgerRepo {
    fn record_task(&self, task: LedgerTask) -> Result<(), RepoError>;
    fn tasks(&self) -> Result<Vec<LedgerTask>, RepoError>;
}
```

Append-only NDJSON task ledger in `<dir>/tasks.jsonl`. `record_task` appends one raw `LedgerTask` snapshot (re-recording an id is allowed — it is a new snapshot, not an overwrite). `tasks` reads all snapshots and collapses them to current state via [[swarm-core#fold_tasks]] (latest snapshot per id, first-seen order preserved), so File and Mem share identical fold semantics.

Related: [[swarm-store#MemLedgerRepo]], [[swarm-store#default_file_ledger_repo]], [[swarm-core#LedgerRepo]]

### `MemLedgerRepo`

```rust
pub struct MemLedgerRepo { /* Mutex<Vec<LedgerTask>> */ }

impl MemLedgerRepo {
    pub fn new() -> Self;        // also: Default
}

impl LedgerRepo for MemLedgerRepo { /* record_task + tasks */ }
```

In-memory `LedgerRepo` double: snapshots accumulate in a `Mutex<Vec<…>>` and `tasks` applies the same `fold_tasks` collapse on read, matching `FileLedgerRepo`.

Related: [[swarm-store#FileLedgerRepo]], [[swarm-core#LedgerRepo]]

### `default_file_ledger_repo`

```rust
pub fn default_file_ledger_repo() -> Option<FileLedgerRepo>
```

Constructs a `FileLedgerRepo` pointed at `<swarm home>/ledger`, or `None` if the home is unresolvable. The conventional way to get the production ledger repo.

Related: [[swarm-store#FileLedgerRepo]], [[swarm-store#swarm_home]]

### `FileTelemetryRepo`

```rust
pub struct FileTelemetryRepo { /* dir */ }

impl FileTelemetryRepo {
    pub fn new(dir: PathBuf) -> Self;
    pub fn dir(&self) -> &PathBuf;
}

impl TelemetryRepo for FileTelemetryRepo {
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

Append-only NDJSON telemetry store under `dir`, one log per record family: `observations.jsonl`, `feedback.jsonl`, `proposals.jsonl`, `proposal-votes.jsonl`. Each `record_*` appends one line; each reader returns all parseable records in append order (malformed lines skipped). This is the raw substrate that [[swarm-kernel]]'s telemetry aggregation folds into per-agent stats and recommendations.

Related: [[swarm-store#MemTelemetryRepo]], [[swarm-store#default_file_telemetry_repo]], [[swarm-core#TelemetryRepo]], [[swarm-contracts#AgentObservation]]

### `MemTelemetryRepo`

```rust
pub struct MemTelemetryRepo { /* four Mutex<Vec<…>> */ }

impl MemTelemetryRepo {
    pub fn new() -> Self;        // also: Default
}

impl TelemetryRepo for MemTelemetryRepo { /* same eight methods as FileTelemetryRepo */ }
```

In-memory `TelemetryRepo` double: four `Mutex<Vec<…>>` (observations, feedback, proposals, votes). Records accumulate in insertion order; readers clone the vecs. Faithful stand-in for the file repo in tests.

Related: [[swarm-store#FileTelemetryRepo]], [[swarm-core#TelemetryRepo]]

### `default_file_telemetry_repo`

```rust
pub fn default_file_telemetry_repo() -> Option<FileTelemetryRepo>
```

Constructs a `FileTelemetryRepo` pointed at `<swarm home>/telemetry`, or `None` if the home is unresolvable. The conventional way to get the production telemetry repo.

Related: [[swarm-store#FileTelemetryRepo]], [[swarm-store#swarm_home]]

### `OsProcessLiveness`

```rust
pub struct OsProcessLiveness;

impl ProcessLiveness for OsProcessLiveness {
    fn is_alive(&self, pid: u32) -> bool;
}
```

The production liveness oracle. `is_alive` shells out to the OS (`/bin/kill -0 <pid>` on unix; `tasklist /FI "PID eq <pid>"` on other platforms) via a private crate helper. This is the real implementation of the `swarm-core` `ProcessLiveness` trait (the `NeverAlive`/`AlwaysAlive` doubles live in `swarm-core`).

**When to use:** wherever a real "is this pid running?" answer is needed — job/session status derivation (`FileJobRepo::reconcile_liveness`, session `summary`) and the monitor's `running` flag.

Related: [[swarm-core#ProcessLiveness]], [[swarm-store#FileJobRepo]], [[swarm-store#alerts_json]]

### re-exported core types

```rust
// from repos::mod (and the per-repo modules)
pub use swarm_core::{
    fold_tasks, AlwaysAlive, Cursor, EventContext, EventRepo, JobRepo, JobSpec, LayerReportSpec,
    LedgerRepo, LedgerStatus, LedgerTask, NeverAlive, ProcessLiveness, RepoError, SessionArtifact,
    SessionHandle, SessionIndexRecord, SessionMeta, SessionRepo, SessionSpec, SessionStatus,
    SessionStatusDeriver, SessionSummary, StoredEvent, TelemetryRepo, LEDGER_TASK_SCHEMA,
};
```

For caller convenience, `swarm-store` re-exports the `swarm-core` repo traits and their companion types (request/handle/record/status types, the `Cursor` token, `RepoError`, the `fold_tasks` helper, the `NeverAlive`/`AlwaysAlive` liveness doubles, and the `LEDGER_TASK_SCHEMA` constant). These are documented in full in [[swarm-core]]; importing them from `swarm_store` lets a consumer name both a trait and its implementation from one crate.

Related: [[swarm-core]]
