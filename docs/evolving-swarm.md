# Evolving swarm — a playbook for agents

> How to keep turning `swarm` into a deterministic, block-composable intelligence
> engine — the same way it was done on 2026-06-16. This is the process doc. The
> *what* lives in [the plan](reports/02-orchestration-plan.md); this is the *how*.

The north star (judge every change against these):

1. **More deterministic** — control flow, routing, replay, and decision-collapse are reproducible even though leaf LLM calls are stochastic.
2. **Block-like / lego composability** — typed, snap-together primitives with stable contracts at every seam, on a strong foundation.
3. **Cheap models, working together** — ensembles, verification chains, best-of-N with deterministic judges, refinement with deterministic stopping.

The verdict that governs all work: **evolve in place — ship the value before the framework.** Do not rewrite. Do not build the Block/Plan IR until a second consumer earns it.

---

## The loop: how a unit of work is done

This is the rhythm. Each turn through it produces something committable.

```
 understand → design (if non-trivial) → land a SLICE → verify → commit → check in
      ▲                                                                      │
      └──────────────────────── pick the next slice ◄────────────────────────┘
```

### 1. Understand before touching
- Read [reports/01-current-state.md](reports/01-current-state.md) and [the plan](reports/02-orchestration-plan.md) first. They are grounded at `file:line`.
- **Check the working tree.** `git status` / `git diff --stat`. If there is uncommitted WIP in the files you intend to edit, that constrains everything (see Guardrails). Prefer clean files.
- Ground claims in source with Read/Grep. Never act on a `file:line` from a report without confirming it still holds — the tree moves.

### 2. Design only when the slice is non-trivial
For a one-file mechanical fix, skip straight to landing it. For anything with architectural choice (a new type, a new seam, a rewrite-vs-evolve fork), run the **multi-agent design loop** (see "Reusable workflows"). Don't design what you can just verify.

### 3. Land a SLICE — the core discipline
A *slice* is the smallest change that is **complete, correct, and independently shippable.**
- **Complete** — no half-built scaffolding, no "TODO wire this up later."
- **Correct** — the edge-case-right version, not the flimsiest one.
- **Isolated** — prefer clean (non-WIP) files and, where possible, a single crate, so it can't collide with parallel work.
- **Tested** — every non-trivial slice leaves one runnable check behind: a focused unit test that fails if the logic breaks. Match the file's existing test style.

If a "slice" can't be finished + verified before your stopping boundary, it's too big — split it or defer it. **Never leave half-built work across a handoff.**

### 4. Verify — green before you move
- Run the **scoped** suite first: `cargo test -p <crate>`. Then `cargo build --workspace` to confirm nothing downstream broke (default features *and* the feature-gated paths your change touches: `--features runtime,http` / `--features native`).
- The full gate before a check-in: `cargo test --workspace` (today: **555 tests**, keep it at 0 failures).
- Report results faithfully. If something fails, say so with the output.

### 5. Commit with intent
- **Branch first** if on `main` (default). Group commits by concern; write *why*, not just *what*.
- One commit = one coherent slice. If two concerns are tangled in one file (it happens with WIP overlap), say so in the message.
- End commit messages with the `Co-Authored-By` trailer.

### 6. Check in at the boundary
At a time/context boundary, stop at a **green** state and hand off with: what landed (+ test status), what you deliberately deferred and why, the mixed-file caveats, and the precise resume order. A clean checkpoint beats more scope left mid-edit.

---

## Guardrails (learned the hard way)

- **WIP collision is the #1 hazard.** Uncommitted changes in a file mean: (a) parallel agents on that file will collide; (b) `git worktree` isolation branches from HEAD and *excludes* the WIP, so it can't help. Resolution: commit/stash the WIP first, *then* parallelize. Until then, work only in clean files.
- **YAGNI is a hard rule here.** Don't build infrastructure with no consumer (e.g. a `Clock` trait before replay needs it, or the Block IR before a second DAG consumer). The feasibility critic rejects these. Defer with a note rather than ship speculative abstraction.
- **Determinism ≠ removing stochasticity from LLMs.** It means making everything *around* the leaf calls reproducible: ordering (sort every collection/`read_dir` that feeds output), injected clocks/ids, content-addressed caching for replay, and deterministic *collapse* of stochastic outputs (a pure `judge()`). Don't claim a payload-volatility-sensitive thing is "byte-identical" — canonicalize first.
- **Don't promote types upward prematurely.** `RunOutcome` (kernel) pattern-matches with non-serde `BackendError`; `WorkerEvidenceGate` (exec) isn't `Serialize`. Keep `WorkerOutput` in `swarm-exec` in v1.
- **Cache correctness:** only ever cache `!timed_out && !retryable && exit==0` outcomes. A timed-out result served as a hit is a silent corruption.
- **Keep the surface contracts stable.** CLI command tokens and MCP descriptors are guarded by tests because external wrappers depend on them. Change them intentionally.

---

## Reusable workflows (multi-agent)

These were used on 2026-06-16 and are worth re-running. Launch with the `Workflow` tool; each is a single fan-out you stay in the loop on.

### A. Analyze → design → judge → critique (for any big decision)
Fan out grounded readers per subsystem + cross-cutting auditors → 3 independent architects with *distinct theses* (evolve / fork / rewrite) → a judge panel scoring against the 3 goals → synthesis of the winner + grafts → adversarial completeness + YAGNI critics → revision. Produces a decision you can defend. This is how the verdict and plan were reached.

### B. API documentation sweep
One agent per crate produces its AgentDocs package+API doc + a structured concept list (grounded in the real public surface), then a synthesis agent builds the root manifest + keyword index. Output: [docs/api/](api/).

### C. Adversarial verification of findings
For any claimed bug/finding, spawn N independent skeptics prompted to *refute* it; keep it only if a majority can't. Prevents plausible-but-wrong changes.

**When to reach for a workflow:** comprehensiveness (cover many files in parallel), confidence (independent perspectives + adversarial checks), or scale (more than one context can hold). For a single known fix, just do it.

---

## The work queue (status as of 2026-06-16, end of session)

**DONE this session (committed on `evolve/p0-determinism-foundation`, pushed):**
- ✅ Determinism ordering — score comparators (`telemetry.rs` `score_key`), `ToolRegistry` HashMap→BTreeMap, `read_dir` id-sorting + the `take(80)` artifact bug (store + mcp), Mem repos `list()` id-sorted.
- ✅ **Canonicalizer (P0)** — pure event projection + `canonical_run_hash` in `swarm-contracts/canonical.rs`, from a data-driven spec ([docs/specs/canonicalizer-spec.md](specs/canonicalizer-spec.md), all 31 `EventKind` variants). 8 tests.
- ✅ **`WorkerOutput` keystone (P1)** — the anonymous `(WorkerSpec, i32, RunOutcome)` tuple is now a named `WorkerOutput { worker, exit_code, outcome, gate }` carrying the gate computed once. Behavior-preserving + a keystone test.
- ✅ **`judge()` selector (P1.5 core)** — `Verdict` + pure `judge(&[WorkerOutput]) -> Verdict` (drops `gate.verified == false`, ranks by quantized score, stable tie-break). Wired into the result artifact's "Deterministic Decision" section. `gate.verified` is now a real selector.
- ✅ **Content cache (P2)** — `CacheRepo` + Mem/File + `cache_key`/`is_cacheable`/`with_cache` in `swarm-exec/cache.rs`, wired into worker dispatch behind default-off `[reliability].cache`. Collision-safe FNV-1a key; clean-success-only storage. 7 tests. (Lives in `swarm-exec`, not `swarm-core` — `RunOutcome` is kernel-level, above core.)
- ✅ **converge early-stop (P3a)** — `jaccard_similarity` + `CONVERGE_STABILITY_THRESHOLD`; converge halts once the baseline stabilizes instead of burning a fixed budget. 3 tests.

Workspace: **578 tests, 0 failures.**

**Next, in dependency order — with the gotchas found this session:**
1. **best-of-N (P1.5b)** — the headline amplification: run one spec ×N, collapse with the now-ready `judge()`. **Design gotcha:** seedless subprocess agents (the default) get sample diversity only from stochasticity, so best-of-N must **bypass the content cache** for its samples (an identical prompt would cache-hit and return N copies of one draw). Cleanest shape: a new `best-of` verb (touches `args.rs` parser, `cli.rs` command table at :76 + help list at :155, the `service.rs`/`orchestration.rs:1430` dispatch, and the command-token stability test) OR a `--best-of N` flag on `swarm` that swaps the worker list for N copies of a sampling spec and selects via `judge()` instead of manager synthesis. Prefer the flag (lower surface).
2. **verify-retry (P1.5c)** — wire the dead `verify_metadirector_contract`. **Gotcha:** it requires a "Source Map" section that `build_manager_prompt` (fanout) does NOT ask for — so it belongs in the **metadirector/direct-persona path** (`service.rs` → `run_dispatch`), not fanout, or it will spuriously retry every fanout run. Add a fail-closed `FALSIFIED:` parser for the discuss red-team while here.
3. **gate-as-filter into the manager prompt (P1.5a)** — use `judge()` to drop/down-weight `gate.verified == false` workers before synthesis. Must fall back to all workers when none verify (else the manager gets nothing). Behavior change → config-gate it.
4. **converge observability (P3b)** — converge emits no `JobRecord`/events and is invisible to monitor/sessions/MCP (verified). Add the spine; route its raw string concat (`orchestration.rs` ~1846) through `WorkerOutput`.
5. **seeds (P2 tail)** — `seed`/`temperature` on `BackendRequest`, plumbed to HTTP/native only; fix the `temperature = 0.7` hardcode at `provider/mod.rs:192`. Subprocess agents can't use seeds (cache is their determinism).
6. **Replay surface (P5)** — `swarm replay` (re-run argv against a warm cache, assert zero LLM calls + matching `canonical_run_hash` — both primitives now exist); `--json` result envelopes for all verbs.
7. **Block/Plan/Scheduler IR (P4, CONDITIONAL)** — only if a real second DAG consumer appears. Otherwise skip.

Pick the lowest-numbered unblocked item, run the loop, leave it green.

---

## Definition of done (per slice)

- [ ] **Structure** — clean boundaries; nothing reaches across a layer; no speculative abstraction.
- [ ] **Quality** — edge-case-correct; comments match the file; intentional shortcuts marked `// ponytail:` with the ceiling named.
- [ ] **Tests** — scoped suite green; one runnable check added; `cargo test --workspace` at 0 failures.
- [ ] **Committed** — on a branch, with an intent-bearing message.
- [ ] **Handoff-ready** — deferrals and caveats written down.

Then **stop**. Report what you did. Don't pad with unrequested scope.
