# Event Canonicalizer — SPEC

Status: P0 ("parity harness") foundation. See
`docs/reports/02-orchestration-plan.md` → Determinism Pillars ("Canonical
projection", "Canonicalizer is a Phase-0 deliverable", lines 49/85/88/121/138).

Home: `crates/swarm-contracts/src/canonical.rs` (lowest crate that owns
`SessionEventV2`; serde-only deps — correct layer).

## 1. Problem

The engine persists an append-only `events.jsonl` of `SessionEventV2` records.
Two runs of the *same logical work* (same prompt, same routing) produce
**byte-different** logs because each record carries:

- a process-global monotonic `seq` (`swarm-store/src/store.rs` `EVENT_SEQ_COUNTER`),
- a wall-clock `ts_ms` (`now_ms()`),
- a per-run `session_id` / `run_id`,
- heartbeats (`TurnHeartbeat`) whose **count depends on machine speed** (6 s
  wall-clock interval — a slower machine emits more; `orchestration.rs:817-838`),
- thread-interleaved ordering (worker/participant events fire from concurrent
  `thread::spawn`s — `orchestration.rs:369`, `:740`),
- payload fields that vary run-to-run: durations/elapsed, pids, absolute paths,
  byte counts, free text that embeds any of the above.

The **functional content** — which roles emitted which kinds of events, with
what stable payload fields — is what must match across runs. We need a pure
`canonicalize` that folds the raw stream down to exactly that functional spine,
deterministically ordered, so equality / hashing / diffing across runs is
meaningful. Every downstream "byte-identical" gate means "byte-identical
**canonical projection**".

## 2. Wire shape (ground truth)

`SessionEventV2` (`crates/swarm-contracts/src/events.rs:187`), fields
alphabetical for byte-stable serialization:

```
agent_id: String      kind: EventKind        parent_id: Option<String>
payload: Value        phase: String          role: String
run_id: String        schema: String         seq: u64
session_id: String    ts_ms: u128
```

`EventKind` (`events.rs:27`) serializes as a **bare string**; 31 named variants
+ `Other(String)` (unknown kinds pass through verbatim, e.g.
`manager_synthesis`, `learned_routing_applied`).

The existing test-only normalizer (`session.rs:662` `strip_seq`) removes only
`seq` + `ts_ms` from a top-level event object. This spec generalizes that shape:
deeper volatile-field scrub, ephemeral-kind drop, projection, and total order.

## 3. Top-level envelope handling

For each kept event we project to exactly:

```json
{ "kind": <wire string>, "role": <role>, "payload": <scrubbed payload> }
```

**Dropped top-level fields (always):**

| field        | why                                                        |
|--------------|------------------------------------------------------------|
| `seq`        | process-global counter; differs every run                  |
| `ts_ms`      | wall clock                                                 |
| `session_id` | per-run id                                                 |
| `run_id`     | per-run id (== session_id in practice; `session.rs:697`)   |
| `agent_id`   | resolved-backend label; carried *inside* payload as `agent` where functional. The top-level `agent_id` defaults to `"auto"` for `append_event` (`session.rs:219`) and is the concrete agent only for context-aware calls — noisy/inconsistent, so dropped at the envelope level. |
| `parent_id`  | a role/agent pointer that is redundant with payload `parent_role`; defaults to `None`/`"manager"` inconsistently — dropped |
| `phase`      | defaults `"discussion"` for `append_event` but `"turn"` for chunk/heartbeat context calls (`orchestration.rs:835`,`:852`) — inconsistent with the functional `round`, dropped |
| `schema`     | constant `"agent-swarm/event/v2"` — no signal                |

**Kept top-level:** `kind`, `role`. `role` is the stable functional dimension
(manager / a worker role / a helper role); it is set deterministically from
config, not from runtime timing.

## 4. Ephemeral kinds (dropped entirely)

| kind             | reason                                                                 |
|------------------|-----------------------------------------------------------------------|
| `TurnHeartbeat`  | emitted on a 6 s wall-clock loop; **count is machine-speed-dependent** (`orchestration.rs:817-838`). Pure timing noise. |
| `TurnChunk`      | streaming deltas; count + chunk boundaries depend on backend streaming cadence, not logical work (`orchestration.rs:839-854`, `session_repo.rs:595`). The *final* text survives in `TurnCompleted`. |

`TurnChunk` is classified ephemeral because two runs of identical work split the
same final text into a different number of chunks (provider-dependent), so chunk
*count* and chunk *text* are non-functional. Dropping it is conservative for the
parity gate: the functional payload (full turn text) is preserved by
`TurnCompleted`/`AgentMessage`.

`TurnHealthCheck` is **kept** (functional: it fires only on a real failure path,
`orchestration.rs:1041`), but its payload is scrubbed (it embeds `error` text +
`round`).

## 5. Volatile-field scrub rules (recursive over payload `Value`)

`scrub_volatile(&mut Value)` walks objects + arrays. For **object keys**, remove
the key if it matches the volatile set; for surviving **string values**,
normalize path-like strings. Two mechanisms:

### 5a. Volatile KEY removal (exact key-name match, case-sensitive snake_case)

Removed wherever they appear at any depth:

| key                  | classification | source examples |
|----------------------|----------------|-----------------|
| `seq`                | counter        | envelope        |
| `ts_ms`, `ts`        | timestamp      | envelope / nested |
| `*_ms` suffix (`elapsed_ms`, `started_at_ms`, `completed_at_ms`, `created_at_ms`, …) | timestamp / duration | `orchestration.rs:830` heartbeat `elapsed_ms`; metadata `created_at_ms` |
| `started_at`, `completed_at`, `created_at` | timestamp | metadata-shaped payloads |
| `elapsed`, `duration`, `duration_ms`, `took_ms`, `latency_ms` | duration | defensive (no current event uses bare `elapsed`, but the rule is cheap and future-proof) |
| `pid`                | process id     | metadata (`session.rs:133`) — defensive; not in event payloads today |
| `bytes`, `*_bytes` suffix (`text_bytes`) | byte count | `orchestration.rs:1077` digest `bytes`; layer-report `text_bytes` (in envelope, not event payload) |
| `path`, `*_path` suffix (`summary_path`, `transcript_path`, `events_path`, `docs_path`, `digest_path`) | absolute fs path | `SessionCompleted` (`orchestration.rs:621-623`), `DocsCompleted` `path` (`:1254`), `DiscussionDigestUpdated` `path` (`:1076`) |
| `file`               | per-run filename (embeds `ts_ms`: `layer-reports/{ts_ms}-…md`, `event_repo.rs:239`) | `LayerReport` `file` (`event_repo.rs:280`) |
| `cwd`                | absolute working dir | `Created`/`SessionStarted`/`FanoutStarted` (`session.rs:95`, `orchestration.rs:294`,`:674`) |
| `session_id`, `run_id` | per-run id   | defensive nested |

`*_ms`, `*_bytes`, `*_path` are matched by **suffix** so future fields are
covered without edits. `path`/`file`/`cwd`/`pid` are matched **exactly**.

Rationale for dropping `path`/`file`/`cwd` wholesale rather than normalizing:
they are absolute paths rooted in a per-run session dir (`session_store_dir()`
under the user's home) — the *fact* that a summary path exists is functional and
already implied by the `kind`; the path *string* is pure environment. Dropping
is simpler and strictly safe (ponytail: see §8 limitation on path normalization).

### 5b. Path-like STRING normalization (surviving string values)

After key removal, any remaining **string value** (at any depth) is checked: if
it *looks like an absolute path* it is replaced with the sentinel `"<path>"`.
Heuristic (conservative — only unambiguous absolute paths):

- starts with `/` and contains another `/` (Unix absolute: `/Users/...`,
  `/home/...`, `/tmp/...`, `/private/var/...`), OR
- matches `^[A-Za-z]:[\\/]` (Windows drive absolute).

This catches absolute paths embedded in **free text is NOT attempted** — we only
normalize a string value that is *entirely* a path (the whole value matches),
not paths spliced inside prose (`text` fields). That keeps the heuristic from
mangling legitimate functional text that merely mentions a slash. (ponytail
ceiling: §8.)

### 5c. FUNCTIONAL fields (kept, untouched) — by intent

Roles/kinds/ids/flags/counts that are stable for identical logical work:

- `role`, `parent_role`, `from`, `to`, `direction` — the message graph.
- `agent` (`describe_spec` label, e.g. `claude:sonnet`) — resolved backend
  identity; stable given identical routing. `requested`/`used` (fallback).
- `round` — logical round index (deterministic loop counter).
- `exit_code`, `timed_out`, `failed`, `ok`, `succeeded` — outcome flags.
- `status`, `verdict`, `category`, `severity`, `suggested_action`,
  `classified` outputs — diagnosis.
- `stream` (`"stdout"`/`"stderr"`), `mode`, `source`, `purpose`,
  `profile`, `profile_title`, `profile_helpers`, `automation_hooks`,
  `deterministic_checks`, `helper_count`, `min_observations`, `docs`,
  `docs_agent`, `rounds`, `retries`, `reason` — stable config/decision fields.
- `text`, `stderr`, `prompt`, `error`, `summary` — model/agent output text.
  **Kept as-is.** These are the substantive functional content; they are stable
  for *cached/replayed* identical work (the P0 use case is a warm-cache replay,
  `02-orchestration-plan.md:135`). For *live* re-runs LLM nondeterminism makes
  them differ — that is out of scope for the canonicalizer (it strips
  *infrastructure* nondeterminism, not *model* nondeterminism; §8).

> Note on `attempts`/`issues`/`participants`/`workers` arrays: these are arrays
> of objects; `scrub_volatile` recurses into them, so a nested `elapsed_ms` or
> `path` inside an attempt is stripped while `agent`/`role`/`succeeded` survive.

## 6. Per-variant catalogue

All 31 named kinds + observed `Other` kinds. "Payload fields" are the literal
`json!` keys at the cited build site. V = stripped/normalized, F = kept.

| kind | build site | payload fields (classification) | canonical action |
|------|-----------|-------------------------------|------------------|
| `created` | session.rs:66/93/120 | `mode`(F); `cwd`(V) | keep, strip cwd |
| `session_started` | orchestration.rs:670 | `prompt`(F) `rounds`(F) `participants[]`(F:role,agent,profile) `manager`(F) `docs`(F) `docs_agent`(F) `profile_helpers`(F); `cwd`(V) | keep, strip cwd |
| `session_completed` | orchestration.rs:311/617/695/1284 | `failed`(F); `summary_path`(V) `transcript_path`(V) `events_path`(V) | keep, strip *_path |
| `fanout_started` | orchestration.rs:291 | `prompt`(F) `manager`(F) `workers[]`(F:role,agent); `cwd`(V) | keep, strip cwd |
| `preflight_started` | preflight.rs:31 | `manager`(F) `participants[]`(F) | keep as-is |
| `preflight_completed` | preflight.rs:63 | `ok`(F) | keep as-is |
| `preflight_failed` | preflight.rs:74 | `ok`(F) `issues[]`(F) `error`(F) `category`(F) `severity`(F) `suggested_action`(F) | keep as-is |
| `manager_started` | orchestration.rs:515/1084 | `agent`(F) | keep as-is |
| `manager_completed` | orchestration.rs:600/1168 | `agent`(F) `exit_code`(F) `timed_out`(F) `text`(F) `stderr`(F) | keep as-is |
| `manager_failed` | orchestration.rs:564/1132 | `agent`(F) `error`(F) | keep as-is |
| `worker_started` | orchestration.rs:372 | `role`(F) `agent`(F) | keep as-is |
| `worker_completed` | orchestration.rs:422 | `role`(F) `agent`(F) `exit_code`(F) `timed_out`(F) `text`(F) `stderr`(F) | keep as-is |
| `worker_failed` | orchestration.rs:457 | `role`(F) `agent`(F) `error`(F) | keep as-is |
| `worker_fallback` | orchestration.rs:249 | `role`(F) `requested`(F) `used`(F) `attempts[]`(F:agent,retries,succeeded,reason) | keep as-is |
| `backend_retry` | orchestration.rs:268 | `role`(F) `agent`(F) `retries`(F) | keep as-is |
| `helper_started` | orchestration.rs:899/1528 | `round`(F) `role`(F) `agent`(F)? `parent_role`(F) `purpose`(F) | keep as-is |
| `helper_completed` | orchestration.rs:1584 | `round`(F) `role`(F) `agent`(F) `parent_role`(F) `exit_code`(F) `timed_out`(F) `text`(F) `stderr`(F) | keep as-is |
| `helper_failed` | orchestration.rs:1640 | `round`(F) `role`(F) `agent`(F) `parent_role`(F) `error`(F) | keep as-is |
| `docs_started` | orchestration.rs:1192 | `agent`(F) | keep as-is |
| `docs_completed` | orchestration.rs:1249 | `agent`(F) `exit_code`(F) `timed_out`(F) `text`(F) `stderr`(F); `path`(V) | keep, strip path |
| `docs_failed` | orchestration.rs:1273 | `agent`(F) `error`(F) | keep as-is |
| `turn_started` | orchestration.rs:789 | `round`(F) `role`(F) `agent`(F) | keep as-is |
| `turn_completed` | orchestration.rs:937 | `round`(F) `role`(F) `agent`(F) `exit_code`(F) `timed_out`(F) `text`(F) `stderr`(F) | keep as-is |
| `turn_failed` | orchestration.rs:984/1007 | `round`(F) `role`(F) `agent`(F) `error`(F) `category`(F) `severity`(F) `suggested_action`(F) (or `label`/`error`) | keep as-is |
| `turn_heartbeat` | orchestration.rs:825 | `round`,`role`,`agent`,`elapsed_ms` | **DROP (ephemeral)** |
| `turn_chunk` | orchestration.rs:841, session_repo.rs:595 | `round`,`role`,`agent`,`stream`,`text` | **DROP (ephemeral)** |
| `turn_health_check` | orchestration.rs:1042 | `round`(F) `role`(F) `agent`(F) `error`(F) `category`(F) `severity`(F) `suggested_action`(F) | keep as-is |
| `agent_message` | orchestration.rs:756/949/1538/1609 | `round`(F) `from`(F) `to`(F) `direction`(F) `agent`(F) `text`(F) | keep as-is |
| `profile_assigned` | orchestration.rs:743 | `round`(F) `role`(F) `agent`(F) `profile`(F) `profile_title`(F) `automation_hooks`(F) `deterministic_checks`(F) `helper_count`(F) | keep as-is |
| `layer_report` | event_repo.rs:273 | `layer`(F) `role`(F) `agent`(F) `parent_role`(F) `status`(F) `text`(F); `file`(V) `path`(V) | keep, strip file+path |
| `discussion_digest_updated` | orchestration.rs:1073 | `round`(F) `text`(F); `path`(V) `bytes`(V) | keep, strip path+bytes |
| `Other("learned_routing_applied")` | orchestration.rs:217 | `role`(F) `agents`(F) `min_observations`(F) `source`(F) | keep as-is |
| `Other("manager_synthesis")` | (fixtures) | `summary`(F) | keep as-is |

`Other(_)` kinds flow through the generic scrub + projection unchanged — no
special-casing beyond the ephemeral check (which `Other` never matches).

## 7. Ordering (total, deterministic)

Cross-thread interleaving is nondeterministic, so the canonical projection is
**sorted** by a total key. Sort each projected event by the lexicographic tuple:

```
(kind_string, role, canonical_payload_json_string)
```

where `canonical_payload_json_string = serde_json::to_string(&scrubbed_payload)`
(serde sorts object keys via BTreeMap → key order is stable).

**Justification:**

- `kind_string` first groups all events of a kind together — the dominant
  functional axis.
- `role` next groups per-role within a kind — the second functional axis,
  deterministic from config.
- the canonical payload JSON is the final tiebreak, giving a **total** order even
  when two events share `(kind, role)` (e.g. two `agent_message`s of the same
  role in different rounds — `round` lives in the payload, so the payload string
  separates them deterministically).

This is a **total order** (the payload string tiebreak removes all ties: two
events with identical `(kind, role, scrubbed_payload)` are genuinely
indistinguishable post-scrub, so their relative order is irrelevant for equality
or hashing). It is **stable across runs** because every component is derived only
from functional data after volatile scrub.

Trade-off: sorting **discards original emission order**, including causal
ordering (started-before-completed). That is acceptable and intended for the
parity gate — the gate asks "did the same set of functional events occur?", not
"in what wall-clock order?". A future ordering-sensitive reducer
(`fold_run`, `02-orchestration-plan.md:114`) can read the *raw* stream; the
canonicalizer is deliberately order-insensitive. (ponytail: §8.)

## 8. API + contract

```rust
/// One canonical event: kind + role + scrubbed payload. Volatile-free,
/// projection-equal across runs of the same logical work.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CanonicalEvent {
    pub kind: String,
    pub role: String,
    pub payload: serde_json::Value,
}

/// Fold a raw event stream into the deterministic functional spine:
/// drop ephemeral kinds → project to {kind, role, payload} → scrub volatile
/// payload fields → sort into the total canonical order.
///
/// Pure: no clock, IO, or RNG. Deterministic, idempotent, order-independent.
pub fn canonicalize(events: &[SessionEventV2]) -> Vec<CanonicalEvent>;

/// Stable, dependency-free hash (FNV-1a/64, hex) over the serialized canonical
/// projection. Equal iff two runs did the same functional work.
pub fn canonical_run_hash(events: &[SessionEventV2]) -> String;
```

**Contract guarantees:**

1. **Volatile-invariant:** two inputs differing only in `seq`/`ts_ms`/
   `session_id`/`run_id`/`agent_id`/`parent_id`/`phase`/durations/pids/paths/
   byte-counts produce **equal** output + equal hash.
2. **Order-invariant:** any permutation of the input produces equal output.
3. **Ephemeral-drop:** `TurnHeartbeat` + `TurnChunk` never appear in output
   (their count/content is machine/provider-dependent).
4. **Idempotent on the functional axis:** running over the same logical work
   twice yields the same projection + hash.
5. **Sensitivity:** a genuinely different functional event (different `role`,
   `kind`, or a stable payload field like `exit_code`/`text`/`round`) changes the
   projection **and** the hash.

The hash mirrors the FNV-1a style at `swarm-kernel/src/routing.rs:135`
(seed `0xcbf29ce484222325`, prime `0x100000001b3`) → **no new dependency**.

## 9. Honest limitations (ponytail: ceiling + upgrade path)

- **Model nondeterminism is NOT stripped.** Free-text `text`/`stderr`/`summary`/
  `error` are kept verbatim. Identical work via a **warm cache / replay** (the P0
  use case) reproduces identical text → projection matches. *Live* re-runs of a
  nondeterministic LLM will differ in text and thus in hash. *Ceiling:* parity is
  exact only for cached/replayed runs. *Upgrade:* add a `normalize_text` mode
  (e.g. hash long text fields, or strip to a token/line count) behind a flag when
  live-run fuzzy parity is needed.
- **Path normalization is whole-value only.** A path embedded *inside* prose
  (`text: "wrote /Users/x/out.md"`) is not normalized — only a value that is
  *entirely* an absolute path becomes `"<path>"`. *Ceiling:* absolute paths inside
  free text survive. *Upgrade:* a regex sweep over string values if it proves
  necessary — deferred because it risks mangling legitimate functional text.
- **Order is discarded.** The canonical form answers "same set of functional
  events?", not "same sequence?". *Ceiling:* a regression that only reorders
  events (e.g. swaps started/completed causality) is invisible to the hash.
  *Upgrade:* the planned `fold_run` reducer reads the raw stream for
  ordering-sensitive replay assertions.
- **Key-name volatility list is allow/deny by convention.** A new volatile
  payload field with an unrecognized name (no `_ms`/`_bytes`/`_path` suffix, not
  in the exact set) would leak into the projection. *Ceiling:* coverage is as good
  as the catalogue in §6. *Upgrade:* this spec's per-variant table is the
  checklist — when a new payload field is added, classify it here and extend the
  scrub set.
