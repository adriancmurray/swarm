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

## The work queue (status as of 2026-06-16)

**DONE — `evolve/p0-determinism-foundation` (merged to `main`):**
- ✅ Determinism ordering — comparators, `ToolRegistry`→`BTreeMap`, `read_dir` id-sorting + the `take(80)` artifact bug, Mem-repo `list()` parity.
- ✅ **Canonicalizer (P0)** — pure event projection + `canonical_run_hash` (`swarm-contracts/canonical.rs`), data-driven spec ([spec](specs/canonicalizer-spec.md)). 8 tests.
- ✅ **`WorkerOutput` keystone (P1)** — typed worker→manager channel carrying the gate computed once. Behavior-preserving.
- ✅ **`judge()` (P1.5 core)** — pure decision-collapse; `gate.verified` is a real selector, surfaced in the result artifact.
- ✅ **Content cache (P2)** — `CacheRepo` + Mem/File + `with_cache`, wired into worker dispatch behind default-off `[reliability].cache`. Collision-safe. 7 tests.
- ✅ **converge early-stop (P3a)** — Jaccard baseline-stability halt. 3 tests.

**DONE — `evolve/amplification` (open):**
- ✅ **gate-as-filter (P1.5a)** — `verified_for_manager` + default-off `[reliability].gate_filter`; manager reasons over verified workers (falls back to all). 1 test.
- ✅ **best-of-N (P1.5b)** — `swarm --best-of N`: sample the manager spec N×, `judge()`-select; **cache bypassed** for sample diversity. Parse + validation tests.
- ✅ **converge observability (P3b)** — tracking JobRecord + Worker/Manager/Session events; warning-free.
- ✅ **`run-hash` (P5 parity half)** — `swarm run-hash <session>` prints `canonical_run_hash`; the canonicalizer's first CLI consumer + the reproducibility check.

Workspace: **580 tests, 0 failures, 0 warnings.**

**Remaining — with the gotchas found:**
1. **verify-retry (P1.5c)** — wire the dead `verify_metadirector_contract`. **Two gotchas:** (a) it needs a "Source Map" section the fanout manager doesn't produce, so it belongs in the **metadirector/direct-persona path** (`service.rs` → `run_dispatch`), not fanout; (b) `run_partner_foreground` *consumes* the output (prints + saves) without returning it, so a clean retry-once needs that function refactored to return the `RunOutcome`. Not done to avoid destabilizing the shared `run` dispatch for modest value. Add a fail-closed `FALSIFIED:` parser for the discuss red-team while here.
2. **full `swarm replay` (P5 tail)** — `run-hash` is the *verification* half (done). The *re-run* half needs argv/spec capture per session (today only the prompt is stored in the JobRecord) + a re-dispatch path that asserts zero LLM calls against a warm cache and a matching `canonical_run_hash`. Design the argv-capture first.
3. **seeds (P2 tail, footnote)** — `seed`/`temperature` on `BackendRequest`, plumbed to HTTP/native providers only; fix the `temperature = 0.7` hardcode at `provider/mod.rs:192`. Low marginal value: the cache already gives the *default* subprocess agents reproducibility; seeds only help the HTTP/native minority.
4. **`--json` result envelopes** for all verbs (versioned `agent-swarm/*-result/v1`), and expose `converge`/`ledger` over MCP.
5. **Block/Plan/Scheduler IR (P4, CONDITIONAL)** — only if a real second DAG consumer appears. Otherwise skip.

Pick the lowest-numbered unblocked item, run the loop, leave it green.

---

## Definition of done (per slice)

- [ ] **Structure** — clean boundaries; nothing reaches across a layer; no speculative abstraction.
- [ ] **Quality** — edge-case-correct; comments match the file; intentional shortcuts marked `// ponytail:` with the ceiling named.
- [ ] **Tests** — scoped suite green; one runnable check added; `cargo test --workspace` at 0 failures.
- [ ] **Committed** — on a branch, with an intent-bearing message.
- [ ] **Handoff-ready** — deferrals and caveats written down.

Then **stop**. Report what you did. Don't pad with unrequested scope.
