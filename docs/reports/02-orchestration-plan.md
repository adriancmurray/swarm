# swarm — Orchestration Engine Plan

> A deterministic, block-composable intelligence-orchestration engine where cheap models amplify each other.

---

## Verdict: EVOLVE IN PLACE — but ship the value *before* the framework

**Decision: EVOLVE.** Not rewrite, not fork-a-new-crate. And within "evolve," a sharper rule: **ship the value first, build the framework only if a second consumer earns it.**

The scoreboard is close on paper — evolve 41/50, ground-up-rewrite 39/50, fork-a-kernel 37/50 — but the three theses *converge on the same graft*: a typed `WorkerOutput` channel plus a content-addressed leaf cache. That agreement is the signal. The disagreement is only about how much *new framework* to pour around those two grafts, and the honest answer is **as little as possible, as late as possible.**

Why evolve, concretely — every hard-to-earn property already exists and passes tests:

- **Byte-stable wire format** with lockbox round-trip fixtures (`crates/swarm-contracts/src/events.rs`), payload stays `serde_json::Value` and round-trips unchanged.
- **A pure, rng-free retry/fallback state machine** — `build_fallback_chain` + `next_action` + FNV-1a `backoff_with_jitter` (`crates/swarm-kernel/src/routing.rs:52,107,135`).
- **Deterministic ordered spawn/join** — handles joined in spawn order across fanout/discuss/converge, so output ordering is independent of thread completion.
- **Exemplary dependency hygiene** — `swarm-contracts` depends only on `serde`+`serde_json`; `swarm-core` only on `swarm-contracts`+`serde` (Cargo.toml confirmed).
- **`RunOutcome` already derives `Serialize`/`Deserialize`** and carries `timed_out` + `retryable` (`crates/swarm-kernel/src/backend_abi.rs:32-49`) — the exact fields a correct cache needs.

A rewrite re-earns all of these at full risk for zero goal-progress. The missing pieces are **purely additive**: a typed worker→manager channel, a leaf cache, two pure decision functions (judge, verify-retry), and — only if a second DAG consumer materializes — a Block/Plan/Scheduler IR.

**Four corrections to the naive plan, each verified at file:line:**

1. **Do not promote `RunOutcome`/`WorkerEvidenceGate` into `swarm-contracts` in v1.** `RunOutcome` lives in `swarm-kernel`; its sibling `BackendError` is **not** serde-derivable (`backend_abi.rs:50`, no derive) and the two are pattern-matched together. `WorkerEvidenceGate` lives in `swarm-exec` (`synthesis.rs:18-19`) and derives only `Debug, Clone, PartialEq, Eq` — **no `Serialize`**. Keep `WorkerOutput` in `swarm-exec` where both members already live.
2. **`metadirector` is not an orchestration verb.** It rewrites argv and calls the single-agent `run_dispatch` path (`service.rs`). The real topology is **three** orchestration functions — `run_swarm`, `run_discussion`, `run_converge` — plus `run_dispatch`; `design`/`audit` are already just `run_discussion` with different arg parsers.
3. **The gate is not "advisory text only."** `gate.verified` is dead in `orchestration.rs` (empty grep confirmed) but **is** read in `synthesis.rs` to flip the worker-stdout byte-cap (450 vs 700). The job of `judge()` is to make it a *selector* — it does not resurrect an inert flag.
4. **The deletion metric is ~2× overstated.** The three manager-dispatch bodies are **~150 lines measured, not 400**, and they are **not** copy-paste-identical — each builds a distinct manager prompt inline (`run_converge` hardcodes "Convergence Director" and branches on the final iteration). The collapse is a behavior-preserving extraction, not a mechanical dedup.

**The determinism spine is the cache, not seeds.** The default agents are `codex`/`claude` **subprocesses** that expose no seed (`executor.rs` passes only `--model`). For them the content-addressed cache is the *only* replay mechanism. Seeds are a footnote that helps the HTTP/native minority. Stated up front, not buried.

**Cut from scope:** `QuorumBlock` (an orphan — `decide_quorum` would operate over MCP-populated vote counts with no orchestration producer) and `EscalateBlock` (lifts eval-only `should_escalate`). Both are padding against the three goals.

---

## The Foundation

The strong base is the existing **persistence + pure-primitive layer**. It is lego-grade and must be extended, never rewritten. The plan adds a thin typed layer *on top*, touching the foundation only additively.

| Foundation asset | Where | Role in the plan |
| --- | --- | --- |
| `swarm-contracts` (serde-only) / `swarm-core` (contracts+serde) | Cargo.toml | The clean place for new typed contracts *when one is earned* |
| Dual-impl repo pattern (`EventRepo`, `TelemetryRepo`, `LedgerRepo`, `SessionRepo` → File/Mem) | `swarm-core` + `swarm-store` | `CacheRepo` mirrors this exactly; `MemCacheRepo` becomes the deterministic test backend |
| Pure injected-nondeterminism deciders (`SessionStatusDeriver` takes `now_ms` + oracle) | `swarm-core/src/session_repo.rs:190-226` | Template for a minimal injected `Clock` |
| Pure routing core (`build_fallback_chain`, `next_action`, FNV-1a backoff) | `swarm-kernel/src/routing.rs` | Reference pattern; FNV-1a helper reused verbatim for per-sample seeds |
| Single shared dispatch seam `execute_with_fallback → FallbackOutcome` | `swarm-exec/src/executor.rs:249` | Where the cache wraps and where every verb already calls in |
| `RunOutcome` (derives Serialize, carries `timed_out`/`retryable`) | `swarm-kernel/src/backend_abi.rs:32-49` | The live dispatch return type; cache-correctness rule is expressible against existing fields |
| Pure predicates `assess_worker_output`, `verify_metadirector_contract` | `swarm-exec/src/synthesis.rs:38,611` | The verifier kernels; the latter is currently test-only (callers at 1014/1035/1057/1068) |
| Byte-stable event log: ordered append + monotonic `EVENT_SEQ_COUNTER` + `strip_seq` normalizer | `swarm-store/src/store.rs:36`, `session.rs:662` | The backbone any replay/parity check folds over *after canonicalization* |

**Key foundation decision:** keep `StoredEvent.payload` as `serde_json::Value`. The lockbox fixtures round-trip it unchanged; retyping it to a tagged enum risks those fixtures for marginal benefit. Replay is delivered by the cache, not by a wire-format migration.

---

## Block Taxonomy

Tier 1 ships first and stands alone (free functions + one value type). Tier 2 is the framework — built **only if a second DAG consumer appears.**

| Block | Typed contract | Purpose |
| --- | --- | --- |
| **`WorkerOutput`** *(keystone, Tier 1)* | value type `{ role: String, spec: String, outcome: RunOutcome, gate: WorkerEvidenceGate }` — lives in `swarm-exec` | Replaces the `push_str(&format!(…))` prose concat feeding the manager (`orchestration.rs:501,1029,1620` family). Makes the gate verdict structurally available to any future collapse. Survives independent of everything else. |
| **`Verdict`** *(Tier 1)* | value type `{ accepted: bool, winner: Option<usize>, missing: Vec<String>, score: i64 }` | Typed decision-collapse result. `score` is a **quantized i64**, never raw f64. Today collapse returns a process exit code; this is the typed output of `judge()`. |
| **`judge(workers: &[WorkerOutput]) -> Verdict`** *(Tier 1, FREE FN)* | pure `&[WorkerOutput] -> Verdict` | Deterministic collapse, no LLM. Drop `gate.verified == false`, rank by citation_count → quantized score → stable index tiebreak. Turns the gate from a byte-cap signal into a worker **selector**. Best-of-N winner selection lives here. A stateless reducer needs no trait. |
| **`verify_retry`** *(Tier 1, FREE FN)* | pure `&str -> Result<(), Vec<String>>` (wraps `verify_metadirector_contract`) + caller-side retry-once | Wires the dead `verify_metadirector_contract` as a live acceptance predicate; on `Err(missing)` re-prompt **once** with the missing-sections list (deterministic retry-once, mirroring `next_action`). |
| **sample-then-judge** *(Tier 1, a LOOP + judge)* | run same spec `n` times → `Vec<WorkerOutput>` → `judge()` | Best-of-N. Per-sample seed = `fnv1a(run_seed, role, i)`. On subprocess backends samples are stochastic but the **judge is deterministic**, so selection is reproducible given identical inputs (fully reproducible on cache hit). A for-loop in `run_swarm` + `judge()`, not a new type. |
| **`LeafBlock`** *(Tier 2)* | `BlockInput -> BlockOutput{ results: [one WorkerOutput] }` | One role through `execute_with_fallback` (reused verbatim). The **only** impure block; consults the cache before dispatch. The only operation that justifies a trait. |
| **`FanoutBlock(Vec<LeafBlock>)`** *(Tier 2)* | `BlockInput -> BlockOutput{ results: Vec<WorkerOutput> }` | The already-deterministic ordered spawn/join, extracted once; collects by stable role index. |
| **`SynthesizeBlock`** *(Tier 2, the hard deletion target)* | `BlockInput{ prior: Vec<WorkerOutput> } -> BlockOutput{ results: [manager WorkerOutput] }` | Collapses the three manager bodies (`orchestration.rs:521-579`, `1102-1150`, `1830-1872`, **~150 lines measured**) into one, factoring out three distinct prompt-builders. The make-or-break that makes the IR real. |
| **`IterateBlock(inner, stop)`** *(Tier 2, fixes converge)* | `BlockInput -> BlockOutput` (loops `inner` until `stop`) | Replaces the fixed `for iteration in 1..=args.iterations` (`orchestration.rs` line ~1689) with a deterministic Jaccard stop. Must **also** add converge's missing event/JobRecord plumbing (its own line item). |
| **`Plan` + `Scheduler`** *(Tier 2, conditional)* | `Plan = Vec<Stage>`; `Scheduler::run(&Plan, BlockInput, &mut Ctx) -> BlockOutput` | DAG-as-data. `fanout = [Fanout, Synthesize]`; `discuss/design/audit = [Round*N, Synthesize]`; `converge = [Iterate(...)]`. **Build only if a second consumer demands it.** |
| ~~`QuorumBlock` / `EscalateBlock`~~ *(CUT)* | n/a | Orphan / eval-only padding against the three goals. Removed. |

---

## Determinism Pillars

| Pillar | Mechanism |
| --- | --- |
| **Content cache IS the spine** | New `CacheRepo` trait in `swarm-core` + File/Mem impls mirroring `TelemetryRepo`. Key = `sha256(prompt + persona/system bytes + model_id + resolved fallback-chain identity + decode_params + cwd fingerprint + timeout)`. Wrap `execute_with_fallback` (`executor.rs:249`); a hit returns the stored `RunOutcome` at zero token cost. **The only replay path for the default subprocess backends.** Ship default-off behind `--cache`. |
| **Cache-correctness rule** | Write an outcome **only** when `!timed_out && !retryable && (exit_status == Some(0) || non-process backend)`. `RunOutcome` carries `timed_out` and `retryable` (`backend_abi.rs:43,45`), so timed-out / transient / nonzero outcomes are **never** served as hits. |
| **Seeds are a footnote, not the spine** | Add `seed: Option<u64>` + `temperature: Option<f32>` to `BackendRequest`; plumb only to providers that honor them (OpenAI `seed`, Ollama `options.seed`, native). Fix the `temperature = 0.7` hardcode at `provider/mod.rs:192`. **Coverage limit, stated up front:** cli-subprocess + Anthropic cloud expose no seed → they rely on the cache, full stop. |
| **Deterministic collapse before the stochastic manager** | `judge(&[WorkerOutput]) -> Verdict` drops `gate.verified == false`, ranks by citation_count → quantized i64 score → stable index. Only survivors reach `SynthesizeBlock`/the manager. Converts `gate.verified` from a byte-cap-only signal (`synthesis.rs:324`) into a real selector. |
| **Quantize the two comparators that exist** | `telemetry.rs:502` **and** `telemetry.rs:538` both `partial_cmp` on f64 `score`. Make both total: quantize `score` to scaled i64 + stable index tiebreak at each site. A two-site fix, not a system-wide mandate. |
| **Replay via cache + a Value-reading reducer** | Keep `StoredEvent.payload` as `serde_json::Value` (lockbox fixtures stay safe). `swarm replay <session>` re-runs the same argv against a warm cache → zero LLM calls. A pure `fold_run(events) -> RunState` reducer (modeled on `fold_tasks`) reads fields off the Value by `kind`, folding only the functional spine in role-sorted order. |
| **Canonicalizer is a Phase-0 deliverable** | Worker events fire from concurrent threads with a process-global seq, so raw `events.jsonl` interleaving is nondeterministic (the repo already strips seq in its own tests, `session.rs:662`). The parity gate operates on a **canonical projection**: drop `seq`, `ts_ms`, `pid`, and `TurnHeartbeat`; group/sort the per-thread slice by role index; keep the functional spine. Every "byte-identical" gate downstream depends on this. |
| **Order-stable collections that feed the key** | Back `ToolRegistry` with `BTreeMap` (today `HashMap`, `swarm-manager/src/tools/registry.rs:8` — note this is the *native* path). Sort context-gather entries before the `max_files` truncation. **Verify-before-claim:** only inputs that actually reach the cached backend belong in the key. |
| **Minimal injected nondeterminism** | Phase 0 adds a `Clock` trait + `ctx.clock.now_ms()` shim wrapping the existing `now_ms`, following `SessionStatusDeriver`. **Do not** build a 5-field god-context up front; add fields only when a block needs them. Ids become injectable (optional caller-supplied id on `SessionSpec`). |
| **Ephemeral vs replayable event split** | Tag `TurnHeartbeat` (6s wall-clock interval — a slower machine emits *more*) and the stagger sleep (`(index % 6) * 90ms`) as ephemeral; the canonicalizer drops them. Remove the stagger sleep or gate it default-off. |

---

## Amplification Blocks — how cheap models get smarter

| Name | How | Why cheap models get smarter |
| --- | --- | --- |
| **Gate-as-filter before synthesis** *(ship FIRST)* | A pre-collapse step in `judge()` drops `gate.verified == false` workers (or down-weights, reusing the 450-vs-700 byte-cap intent at `synthesis.rs:324`, now as control flow). ~tens of lines, no cache, no seeds. | A weak manager spends its limited context budget only on cited, non-blocked evidence → a better synthesis from pre-filtered inputs. De-risks the whole amplification thesis at Phase 1.5 before investing in the content store. |
| **Best-of-N (sample-loop + judge)** | Run the same worker spec `n` times with per-sample FNV-1a seeds, gate each with `assess_worker_output`, collapse with pure `judge()`. The cache dedupes identical sub-prompts. | The single biggest missing piece for goal #3 — today fanout runs N *different* roles once each; no same-task sampling exists anywhere. N cheap samples + a free deterministic judge beat one expensive call; the winner is reproducible given identical inputs. |
| **Verification chain (verify → retry-once)** | After a step, run `verify_metadirector_contract` (currently dead, `synthesis.rs:611`) as a pure acceptance predicate; on `Err(missing)` re-prompt **once** with the missing sections. For the lifted red team: replace the exit-code-as-signal with a **tested, fail-closed `FALSIFIED: yes/no` stdout parser** (no parseable verdict ⇒ not-falsified/unverified). | A cheap generator + a deterministic structural verifier catches missing-section / uncited output the model would emit unchecked, and finally gives the repo's one acceptance verifier a production caller. Reproducible because the verifier is pure text-structure. |
| **Iterative refinement with deterministic stopping (converge fix)** | Replace converge's fixed `for iteration in 1..=args.iterations` with a single deterministic stop: normalized-token Jaccard stability between successive baselines ≥ threshold. Ship Jaccard alone; add the verify gate only if early-stop proves too eager. | Cheap models refine until output **stabilizes** rather than burning a fixed budget. **Honest caveat:** the early-stop *mechanism* is deterministic given identical inputs; on subprocess/cloud leaves the inputs (hence the stop point) still vary run-to-run unless cached/seeded. |
| **Hardening: de-game the gate** *(before trusting best-of-N scoring)* | `count_citations` includes the literal `/Users/` (`synthesis.rs:96`) and `missing_proof_of_work` keys off the literal `exit_code: 0` (`synthesis.rs:127`) — both emittable by the model. Validate citations against the real context index (reuse `context_gather`); derive proof-of-work from recorded command exit codes. | Best-of-N on a gameable judge *amplifies the gaming*. Orthogonal to any IR; lands as a hardening pass that **must precede** trusting best-of-N scoring on real tasks. |

---

## Phased Roadmap — each phase independently shippable

| Phase | Goal | Ships | Depends on |
| --- | --- | --- | --- |
| **0 — Foundation prep + parity harness** | Build everything later gates depend on, zero behavior change: (a) the **canonicalizer** + a `MemCacheRepo`-backed deterministic test path (reuse the `strip_seq` shape); (b) a `Clock` trait + `now_ms()` shim; (c) quantize **both** comparators (`telemetry.rs:502` and `538`) with stable tiebreaks. *(Config-drift cleanup — fold `default_timeout`/`default_agent` into serde — is good but unrelated; do it in a separate PR.)* | A canonicalizer + `MemCacheRepo`; injected `Clock`; two total comparators. All tests pass. | none |
| **1 — Typed worker channel (keystone)** | Introduce `WorkerOutput { role, spec, outcome, gate }` in `swarm-exec` (do **not** promote to contracts). Make `build_manager_prompt` / `build_discussion_manager_prompt` consume `Vec<WorkerOutput>` instead of accumulating a `String`. Pin the manager prompt bytes with a snapshot test. | The single biggest composability unlock; the gate verdict is structurally available to any future collapse. Touches `synthesis.rs` + three call sites. Stands alone. | Phase 0 |
| **1.5 — Gate-filter + best-of-N + verify-retry (goal #3, NO framework)** | Add `judge(&[WorkerOutput]) -> Verdict` as a free function. Wire gate-as-filter before synthesis. Add a sample-loop in `run_swarm` for best-of-N (per-sample FNV-1a seeds). Wire `verify_metadirector_contract` as a retry-once predicate; build the fail-closed `FALSIFIED:` parser. **Prove the amplification thesis on a real task before building the cache.** | Best-of-N, gate-filter, verification chain — goal #3 met with zero new crates and zero IR. `verify_metadirector_contract` gains a production caller. | Phase 1 |
| **2 — Content cache + seeds (goal #1)** | Add `CacheRepo` trait + File/Mem impls. Wrap `execute_with_fallback` (`executor.rs:249`); enforce the cache-correctness rule. Add `seed`+`temperature` to `BackendRequest`; plumb to HTTP/native only; fix the `0.7` hardcode (`provider/mod.rs:192`). State the coverage limit. | Leaf calls cacheable (zero-token replay) and reproducible on HTTP/native. Default-off behind `--cache`. Bolts onto one function, no IR dependency. | Phase 1.5 (so amplification is proven first); cache itself needs only Phase 0 |
| **3 — Converge fix + observability** | Replace converge's fixed loop with a deterministic Jaccard early-stop. **Separately** add converge's missing `JobRecord`, `SessionSpec`, `append_event` spine, and summary/transcript paths (verified absent in 1663-1901) so it becomes visible to monitor/sessions/MCP. | Converge terminates early on stability and emits `Worker*`/`Manager*` events + a `JobRecord` per iteration (today: none). | Phase 2 (cache makes early-stop deterministic on subprocess backends) |
| **4 — Block + Plan + Scheduler (CONDITIONAL)** | **Only if a real second DAG consumer appears** (external Plan author, config-driven pipeline): add `LeafBlock`/`FanoutBlock`/`SynthesizeBlock`/`IterateBlock` + minimal scheduler behind a feature flag; re-express the three verbs as Plan literals; assert canonical-events parity; **collapse the three manager bodies (~150 lines) into one `SynthesizeBlock`.** Not done until those bodies are gone and parity is green. If no consumer appears, **skip entirely.** | DAG-as-data; ~150 lines of manager-dispatch collapsed. Explicitly optional. | Phase 3 + a demonstrated second consumer |
| **5 — Replay surface + symmetry** | `swarm replay <session>` = re-run argv against a warm cache, asserting zero LLM calls + matching canonical run-hash. Add a pure `fold_run(events) -> RunState` reducer reading off `serde_json::Value` by `kind` (keep payload as Value). Add `agent-swarm/swarm-result/v1` + `discussion-result/v1` JSON envelopes under `--json` (**net-new for all orchestration verbs** — they emit only Markdown today). Expose `converge` + `ledger` over MCP. | Reproducible, diffable, hashable runs; symmetric JSON across CLI/MCP. Goal #1 fully realized via the cache, not a wire migration. | Phase 3 (Phase 4 not required) |

---

## Risks & Mitigations

- **The IR is built before a second consumer exists** — "a trait nobody uses, worse than either pole" (the top risk, most likely *because* the trait is not needed to ship the value). **Mitigation:** the IR is **conditional Phase 4**, gated on a demonstrated second DAG consumer. All three goals ship in Phases 1-3 with free functions and a cache wrapper. If the consumer never appears, the IR is correctly never built.
- **"Byte-identical `events.jsonl`" is unenforceable as worded** — worker events fire from concurrent threads with a process-global seq; heartbeat count varies with wall time. **Mitigation:** the gate operates on a **canonical projection** (drop seq/ts/pid/heartbeats; role-sorted spine), built in Phase 0 reusing the repo's own `strip_seq` (`session.rs:662`). "Byte-identical" always means "byte-identical canonical projection."
- **No deterministic test backend on the exec path** — `MockProvider` lives in `swarm-manager` (the native path), not the exec path, so it cannot test `execute_with_fallback`. **Mitigation:** Phase 0's `MemCacheRepo`-backed path returns stored `RunOutcome`s with no network/subprocess. The cache **is** the deterministic test backend; no new mock provider needed.
- **Caching serves stale/wrong output** if the key omits an input, or caches failures as successes. **Mitigation:** key covers the full canonical request (persona bytes, resolved chain identity, cwd fingerprint, timeout). Write only when `!timed_out && !retryable && exit==0`. Default-off. A unit test writes a `timed_out` outcome and asserts a subsequent lookup **misses**.
- **Seeds don't cover the default backends** — `codex`/`claude` subprocesses and Anthropic cloud expose no seed. **Mitigation:** reframed up front — the cache is the spine for defaults; seeds are a footnote for HTTP/native. Best-of-N still works on stochastic samples because the judge is deterministic.
- **Verdict-from-stdout is optimistic** — models emit verdicts inconsistently. **Mitigation:** a defined, tested `FALSIFIED: yes/no` parser with a **fail-closed default** (no parseable verdict ⇒ not-falsified/unverified) is its own Phase-1.5 deliverable, not an assumed predicate.
- **Gate needles are gameable** (`/Users/` at `synthesis.rs:96`, `exit_code: 0` at `synthesis.rs:127`); best-of-N amplifies gaming. **Mitigation:** validate citations against the real context index; derive proof-of-work from recorded exit codes. Lands as a hardening pass that **precedes** trusting best-of-N scoring.
- **Converge rework is larger than "wrap in IterateBlock"** — it emits no events and has no `JobRecord` today (verified empty grep, 1663-1901). **Mitigation:** converge observability is its own Phase-3 line item, separate from the Jaccard stop.

---

## Success Metrics

- **Goal #3 ships without the IR:** gate-as-filter + best-of-N land in Phase 1.5 and, on a benchmark task, best-of-N beats a single cheap sample on the gate score — with no Block trait, no cache, no seeds in the build path.
- **`gate.verified` is read in control flow as a selector:** grep for `gate.verified` in `judge()`/orchestration selection returns non-empty (today it is read only as a byte-cap signal at `synthesis.rs:324`).
- **Zero-token replay:** `swarm replay <session>` against a populated cache makes zero LLM calls (testable against `MemCacheRepo`), reproduces the same `Verdict`/winner, and a canonical (seq/ts/pid/heartbeat-stripped, role-sorted) run-hash matches across two cached runs.
- **`verify_metadirector_contract` has a production caller** (today all callers are test-only: `synthesis.rs:1014/1035/1057/1068`), and the red-team `FALSIFIED:` parser has a fail-closed unit test.
- **Converge terminates early:** on a task that converges by iteration 2, the Jaccard stop halts at 2 instead of burning `args.iterations` — **and** converge now emits `Worker*`/`Manager*` events plus a `JobRecord` per iteration (today: none).
- **The parity canonicalizer exists and is used:** a Phase-0 harness produces a stable canonical events projection identical across two concurrent runs of the same prompt.
- **Both score comparators are total:** `telemetry.rs:502` **and** `538` use quantized i64 + a stable index tiebreak.
- **Conditional deletion metric (only if Phase 4 ships):** the three manager-dispatch bodies (`orchestration.rs:521-579/1102-1150/1830-1872`, ~150 lines measured) collapse into one `SynthesizeBlock` behind a green canonical-parity gate. *(Corrected from the "~400 lines" framing; `orchestration.rs` will not reach "a few hundred lines" from this collapse alone — the bulk is worker-spawn, context-gather, red-team, heartbeat, and converge plumbing.)*
- **Surface symmetry:** every action verb (`run`/`swarm`/`fanout`/`discuss`/`audit`/`design`/`converge`) emits a versioned `agent-swarm/*-result/v1` JSON envelope under `--json` (net-new for all orchestration verbs), and `converge`+`ledger` are reachable over MCP.
- **Cache correctness:** a timed-out or nonzero-exit `RunOutcome` is never served as a cache hit (unit test writes a `timed_out` outcome and asserts a subsequent lookup misses).

---

### Start Phase 1 tomorrow

1. **Phase 0, item (c) is a 1-hour warm-up:** quantize `score` at `crates/swarm-kernel/src/telemetry.rs:502` and `:538` (scale to i64, add `.then_with(|| a.index.cmp(&b.index))`). Two-site fix, fully testable, zero behavior risk.
2. **Then the keystone:** define `WorkerOutput` in `swarm-exec`, change `build_manager_prompt`/`build_discussion_manager_prompt` (`synthesis.rs`) to take `&[WorkerOutput]`, and replace the worker-accumulation concat at the `orchestration.rs:501` family of call sites. Snapshot-test the manager prompt bytes.
3. **Everything after that is additive and independently shippable** — and the framework gets built only if a second consumer earns it.

---

*Generated 2026-06-16 by a multi-agent analysis workflow (22 agents): 9 grounded subsystem readers + a determinism auditor + an amplification auditor → 3 independent architects (evolve / fork / rewrite) → a judge panel → synthesis → adversarial critique → revision. All claims are grounded at `file:line` against the working tree.*
