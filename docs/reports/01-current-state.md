# swarm — Current State

> **Scope.** This is an honest assessment of the `swarm` workspace as it exists today, judged against the three stated goals: (1) a **more deterministic** intelligence-orchestration system, (2) **block-like / lego composability** on a strong foundation, and (3) making **cheap models dramatically smarter by working together**. Every claim is grounded at `file:line`. Where a claim is "absent," it was verified by an empty grep, noted inline.

---

## 1. What swarm is

`swarm` (CLI: `agent-swarm` / `swarm`) is a ~39k-LOC Rust workspace that orchestrates multiple coding agents and collapses their outputs into a single answer. A leaf "agent" is one of three backend kinds, declared as a TOML descriptor in `~/.swarm/config.toml` (`kind = cli | openai-compatible | native`):

- **cli** — a frontier coding-agent **subprocess** (codex, claude). This is the default path; the dispatch passes only `--model`, no sampling controls (`crates/swarm-exec/src/executor.rs` `execute_partner` path; subprocess args confirmed to carry no seed).
- **openai-compatible** — an HTTP chat-completions endpoint.
- **native** — an in-process harness (`swarm-manager`): a `Provider` trait with HTTP backends, a deterministic tool-loop `Agent`, a `ToolRegistry`, JSON presets, and a `SKILL.md` loader.

The user drives it through orchestration **verbs**: `run`, `fanout` (alias `swarm`), `discuss`, `metadirector`, `audit`, `design`, `converge`. Persistence is event-sourced-ish under `$SWARM_HOME`: append-only `events.jsonl`, plus sessions, jobs, telemetry, and a folding task ledger.

### Crate dependency DAG (the implicit foundation)

```
swarm-contracts ──▶ swarm-core ──▶ swarm-store ──▶ swarm-kernel ──▶ swarm-exec ──▶ swarm-mcp
   (1580)            (1076)         (3640)          (7040)          (8189)        (3170)
                                                                       └────────▶ swarm-cli (6949)

swarm-manager (7429) — native single-agent harness, driven by swarm-exec's NativeBackend
swarm-registrar (368) — service registry
```

The DAG is **clean and one-directional**. `swarm-contracts` depends only on `serde` + `serde_json`; `swarm-core` only on `swarm-contracts` + `serde` (Cargo.toml confirmed). The crate doc enforces "no external-system types" in the foundation (`swarm-core/src/lib.rs`). This is a genuinely strong base — but, as Section 3 shows, it is a strong **persistence** foundation, not yet the **orchestration** foundation the lego goal needs.

---

## 2. Architecture at a glance

| Layer | Crate(s) | What it owns | Block-quality |
|---|---|---|---|
| Foundation | `swarm-contracts`, `swarm-core` | Wire-stable event/job/ledger types, typed id newtypes, repo traits, pure deriver fns | 2/5 |
| Persistence | `swarm-store` | Append-only NDJSON logs, atomic record writes, dual File/Mem repo impls | 4/5 |
| Kernel | `swarm-kernel` | Routing (`build_fallback_chain`, `next_action`), backoff, task classifier, config, arg parsing | 4/5 |
| Native harness | `swarm-manager` | `Provider`/`Tool`/`ToolRegistry`/`SkillSet`/`Preset` blocks + deterministic agent loop | 4/5 |
| Orchestration | `swarm-exec` | `run_swarm`/`run_discussion`/`run_converge`, manager synthesis, evidence gate | 2/5 |
| Surface | `swarm-mcp`, `swarm-cli` | Stable CLI token table, MCP JSON-RPC server, schema-tagged read tools | 3/5 |

The pattern is consistent and telling: **the leaf-level seams are clean blocks; the orchestration layer that composes them is monolithic.**

---

## 3. Honest determinism + composability scorecard

### Where swarm already IS deterministic / block-like ✅

| Property | Evidence | Why it counts |
|---|---|---|
| Pure, rng-free routing core | `routing.rs:52` (`build_fallback_chain`), `:107` (`next_action`), `:135` (`backoff_with_jitter`, FNV-1a over `(role, attempt)`, no rng) | Same config + telemetry snapshot ⇒ byte-identical chain and sequencing. This is the reference pattern for the whole engine. |
| Ordered concurrent collection | Workers spawned over an ordered `Vec` and joined **in spawn order**: fanout join loop `orchestration.rs:1810-1827` (converge), analogous in swarm/discuss | `worker_results` and the manager prompt are independent of thread completion timing. |
| Deterministic evidence gate | `assess_worker_output` `synthesis.rs:38-89` — pure, no rng/clock/IO; `verified = flags.is_empty()` (`:80`) is a conjunction of mechanical checks | A reproducible VERIFIED/UNVERIFIED verdict per worker output. |
| Byte-stable wire format | `SessionEventV2` alphabetical field order + lockbox round-trip fixture tests (`swarm-contracts/src/events.rs`); `EventKind::Other(String)` forward-compat arm | Persisted state round-trips exactly; replay/diff is possible at the storage layer. |
| Monotonic total order | `EVENT_SEQ_COUNTER` (`store.rs:36`), separate from `ATOMIC_WRITE_COUNTER` (`:29`) so temp-file nonces can't gap the sequence; single `EVENT_LOG_LOCK` (`:23`) serializes appends | True total order independent of wall-clock `ts_ms`, with crash-safe torn-line tolerance. |
| Dual File/Mem repos | Every persistence concern is a `swarm-core` trait with interchangeable File/Mem impls + shared contract tests | The Mem variants make deterministic test/replay harnesses trivial. |
| Deterministic ranking | `learned_candidates_for_role` `telemetry.rs:524-544` — BTreeMap grouping + full tiebreak (score desc, runs desc, name asc) | Routing output never depends on map iteration order. |
| Frozen surface contracts | CLI subcommand token test + MCP descriptor→`mcp-tools.json` idempotency gate; schema-tagged read tools | An external orchestrator can tail `events.jsonl` and parse a stable schema. |

### Where it is NOT ❌

| Gap | Evidence | Impact on goals |
|---|---|---|
| No orchestration IR in the foundation | No `Plan`/`Step`/`Block`/`Stage`/`Dag` struct/enum anywhere in contracts+core (empty grep). `SessionSpec.mode` is `String`; `EventContext.phase` is `String`; `PresetId(String)` | **Goal 2:** no typed blocks to snap together. |
| Block-to-block data is concatenated prose | `worker_outputs: String` built via `push_str(&format!(...))` `orchestration.rs:1809-1827` (converge), analogous in swarm/discuss | **Goal 2/3:** ensembles/best-of-N are un-composable at the type level; everything is text-in/text-out. |
| Orchestration output is an exit code | `run_swarm`/`run_discussion`/`run_converge` all return `Result<i32, String>` (`run_converge` `:1663`) | No `SwarmResult`/`Verdict` contract; consumers scrape stdout. |
| Evidence gate is **advisory only** | `gate.verified` is **never read** in `orchestration.rs` (empty grep confirmed). Its only consumers are `synthesis.rs:301` and `:534`, both of which use it to pick a byte cap (450 vs 700) and print a `gate=` line — not to branch control flow | **Goal 3:** "verification" annotates; it does not amplify. A worker that fails the gate is not dropped, re-dispatched, or down-selected. |
| The one acceptance verifier is **dead code** | `verify_metadirector_contract` `synthesis.rs:611` — all callers are tests (`:1014/:1035/:1057/:1068`); zero production callers (grep confirmed) | **Goal 3:** no deterministic answer-contract check runs in production. |
| No content cache / no seeds | No `CacheRepo`/`sha256`/`memoize`/`options.seed` anywhere (empty grep); no sampling `seed` in `swarm-manager` (the two `seed` hits in `agent/mod.rs` are doc comments — "seeds the transcript"); `create_provider` hardcodes `temperature = 0.7_f32` (`provider/mod.rs:192`) and ignores config | **Goal 1:** identical `(prompt, model, backend)` re-invokes the LLM every run; for the default subprocess backends there is **no replay mechanism at all**. |
| Non-deterministic tool order | `ToolRegistry` backed by `HashMap` (`tools/registry.rs:8`); `to_openai_tools()` iterates it; the array goes straight into the request | The serialized tools list (part of the prompt) varies per process, biasing weak-model tool choice. |
| `now_ms` is a free function | `store.rs:167`, called directly at every append; no injectable `Clock` | Byte-identical runs produce byte-different logs; can't hash/diff a run. |

---

## 4. Orchestration patterns that exist today — and how much they amplify

The real topology is **three orchestration functions plus a single-agent path**, not seven independent verbs:

| Verb | Implementation | Amplification delivered |
|---|---|---|
| `run` | single-agent dispatch (`run_dispatch`) | None — one call. |
| `metadirector` | **not** an orchestration verb: rewrites argv to a single gemini persona and calls `run_dispatch` (`service.rs`) | None — single agent. |
| `swarm` / `fanout` | `run_swarm` | Fan-out of N **different** roles, each once → one manager LLM synthesis. |
| `discuss` | `run_discussion` | N rounds of participant turns → manager synthesis; includes an inline epistemic-red-team falsification step (`orchestration.rs:875-922`). |
| `design` / `audit` | `run_discussion` with different arg parsers | Same machinery as discuss. |
| `converge` | `run_converge` `:1663-1901` | Fixed-iteration refinement loop; manager output fed back verbatim as next prompt. |

**How much real amplification?** Less than the verb count suggests:

- **No best-of-N anywhere.** Fan-out runs N *different* roles; nothing samples the *same* task K times and selects a winner. No `vote`/`quorum`/`consensus`/`best_of`/`judge` selector in `orchestration.rs` (empty grep). Cheap-model ensembling currently bottoms out on a single stochastic manager call.
- **The deterministic judge that exists is wired to routing, not answers.** `learned_candidates_for_role` (`telemetry.rs:524`) is a working ranked selector — but it picks *which backend runs*, never *which answer wins*.
- **Verification chains are one-off, not composable.** The red team is hardcoded inline only in discuss, with a fixed 120s timeout and an exit-code-as-signal protocol (`orchestration.rs:875-922`); fanout and converge get nothing. The pure structural verifier (`verify_metadirector_contract`) is dead code.
- **`converge` does not detect convergence.** It loops `for iteration in 1..=args.iterations` (`:1689`) with no fixpoint/diff/quality stop, always burning the full count, feeding raw manager stdout forward (`current_prompt = out.stdout.trim()` `:1883`). It is also **observability-blind**: the body (`:1663-1901`) uses only `append_layer_report` + `println!` — no `append_event`, no `JobRecord`, no `FanoutStarted` events (the `FanoutStarted`/`JobRecord` machinery at `:291`/`:57` belongs to the swarm/discuss paths), and it `return Err(...)` on mid-loop manager failure (`:1895`) instead of degrading like the other verbs. It is invisible to `monitor`/`sessions`/MCP.

**Net:** the deterministic *primitives* needed for amplification already exist as clean pure functions — the gap is purely that nothing **composes** them. Adding best-of-N or quorum today means copy-pasting a fourth ~200-line procedure, not snapping a block onto a seam.

---

## 5. Top gaps relative to the three goals

| # | Goal | Single biggest gap | Where |
|---|---|---|---|
| 1 | More deterministic | **No leaf cache.** For the default codex/claude **subprocess** backends (no seed exposed), the content-addressed cache is the *only* possible replay mechanism — and it does not exist. Seeds would help only the HTTP/native minority. | No `CacheRepo`/`sha256` (empty grep); subprocess dispatch passes only `--model`; `create_provider` hardcodes temp 0.7 (`provider/mod.rs:192`) |
| 2 | Lego composability | **No typed worker→manager channel and no Plan IR.** Block-to-block data is prose concatenation; verbs return exit codes; there is no `WorkerOutput`/`Verdict`/`Plan` type. | concat at `orchestration.rs:1816`; `run_*` return `Result<i32, String>`; no IR in contracts (empty grep) |
| 3 | Cheap models smarter together | **Verification is advisory, not control flow, and there is no best-of-N.** `gate.verified` never branches dispatch; the one answer-verifier is dead; no same-task sampling or deterministic selector exists. | `gate.verified` unread in orchestration (empty grep); `verify_metadirector_contract` test-only callers; no `judge`/`vote`/`best_of` |

**The encouraging shape of the problem:** every hard-to-earn property is already present and tested — pure routing, ordered concurrent collection, a pure evidence gate, byte-stable wire format with lockbox fixtures, dual File/Mem repos. The missing pieces are **additive**: a typed `WorkerOutput` channel to replace the prose concat, a content-addressed cache bolted onto `execute_with_fallback` (`executor.rs:249`, the single shared dispatch seam — `RunOutcome` already carries `timed_out` and `retryable`, `backend_abi.rs:42-44`, so a correct "only cache successful, non-timed-out outcomes" rule is expressible against existing fields), and two pure free functions (a judge over `&[WorkerOutput]`, and a retry-once wrapper that finally gives `verify_metadirector_contract` a production caller). None of these require rewriting the foundation.

---

Key files referenced (all absolute):
- `/Users/adrian/swarm/crates/swarm-exec/src/orchestration.rs` (prose concat `:1809-1827`; converge `:1663-1901`; red team `:875-922`)
- `/Users/adrian/swarm/crates/swarm-exec/src/synthesis.rs` (`assess_worker_output:38`; `verify_metadirector_contract:611`; gate consumers `:301,:534`)
- `/Users/adrian/swarm/crates/swarm-exec/src/executor.rs` (`execute_with_fallback:249`; `FallbackOutcome:234`; `run_fallback_loop:305`)
- `/Users/adrian/swarm/crates/swarm-kernel/src/routing.rs` (`build_fallback_chain:52`; `next_action:107`; `backoff_with_jitter:135`)
- `/Users/adrian/swarm/crates/swarm-kernel/src/backend_abi.rs` (`RunOutcome:33`; `BackendError:51`; `BackendRequest:152`; not-yet-wired note `:8-10`)
- `/Users/adrian/swarm/crates/swarm-kernel/src/telemetry.rs` (comparators `:500-502`, `:536-538`; ranking `:524`)
- `/Users/adrian/swarm/crates/swarm-store/src/store.rs` (`EVENT_SEQ_COUNTER:36`; `now_ms:167`; `new_session_id:154`)
- `/Users/adrian/swarm/crates/swarm-manager/src/provider/mod.rs` (`temperature = 0.7:192`); `/Users/adrian/swarm/crates/swarm-manager/src/tools/registry.rs` (`HashMap:8`)
- `/Users/adrian/swarm/crates/swarm-contracts/src/events.rs` (`SessionEventV2`, lockbox fixtures)

---

*Generated 2026-06-16 by a multi-agent analysis workflow (22 agents): 9 grounded subsystem readers + a determinism auditor + an amplification auditor → 3 independent architects (evolve / fork / rewrite) → a judge panel → synthesis → adversarial critique → revision. All claims are grounded at `file:line` against the working tree.*
