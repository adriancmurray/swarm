---
name: using-swarm
description: Drive the swarm CLI to orchestrate other agents — reach for it when a task needs multiple perspectives, parallel fan-out across workers, a structured debate between roles, or a manager that plans and synthesizes from supplied context.
---

# Using swarm

`swarm` is a command-line orchestrator. You (the host agent — Claude Code,
Codex, Gemini CLI, or any other) invoke it to coordinate **other** agents:
fan a task out to parallel workers, run a structured multi-role discussion,
or hand a manager a pile of context to plan and synthesize. The engine routes
each role to a configured backend (a CLI agent, an HTTP API, or an in-process
harness) and returns the combined result.

## Cardinal rule

**This skill drives the orchestrator. It is never injected into a worker.**

There are two unrelated skill layers (see the project README, "Two layers of
skills"). One layer is `SKILL.md` files that a *native* backend loads into a
single worker's system prompt. This is the *other* layer: a playbook for the
host agent calling the `swarm` binary. They never touch. Nothing you read here
changes how any worker behaves — it only tells you how to launch the swarm.

## When to reach for swarm

- **Multi-perspective review** — you want a security lens, a simplification
  lens, and a tests lens on the same diff. → `audit`
- **Parallel fan-out** — a task splits cleanly into independent slices and you
  want them worked at once, then merged. → `fanout`
- **Structured debate** — two or more roles should argue a design across
  rounds, not just answer in isolation. → `discuss`
- **Manager synthesis** — you have a large context and want one capable agent
  to plan or reconcile it. → `metadirector`
- **Design review** — a product/UI change needs visual, motion, interaction,
  and accessibility eyes. → `design`

For a single routed task with no coordination, just use `run` (or omit the
verb — a bare prompt is treated as `run`).

## Core verbs

Flags below are verified against the binary. A bare prompt defaults to `run`.
`--timeout` is in seconds (default 300). `--cwd` sets the working directory.

### fanout (alias: swarm) — parallel workers + synthesis

```
swarm fanout [--manager AGENT[:MODEL]] [--worker ROLE=AGENT[:MODEL]]... \
             [--parent ID] [--slice ID] [--cwd CWD] [--timeout SECONDS] PROMPT
```

Send one task to several workers and let the manager merge their output.

```sh
swarm fanout \
  --worker security=claude:sonnet \
  --worker perf=codex \
  "Review src/auth.rs for vulnerabilities and hot paths"
```

### discuss — structured multi-round debate

```
swarm discuss [--participant ROLE=AGENT[:MODEL]]... [--manager AGENT[:MODEL]] \
              [--rounds N] [--parent ID] [--slice ID] [--docs] [--helpers] PROMPT
```

Each participant is a role; they exchange views over `--rounds` rounds and the
manager closes with a synthesis.

```sh
swarm discuss \
  --participant architecture=claude:sonnet \
  --participant review=codex \
  --rounds 2 \
  "Should the job store move from files to SQLite?"
```

### metadirector — manager plans/synthesizes from context

```
swarm metadirector [--model MODEL] [--cwd CWD] [--timeout SECONDS] PROMPT
```

A read-only consult: hand it a large context and ask for a plan or a
reconciliation. Pipe context in on stdin if it is long.

```sh
git diff main | swarm metadirector "Plan the safe rollout order for this change"
```

### design — product / UI review

```
swarm design [--focus all|visual-system|motion|interaction|accessibility|implementation] \
             [--participant ROLE=AGENT[:MODEL]]... [--rounds N] [--helpers] PROMPT
```

A discussion preset centered on design concerns.

```sh
swarm design --focus accessibility "Review the new settings sheet"
```

### audit — read-only codebase review

```
swarm audit [--focus all|simplify|harden|architecture|api-docs|tests] \
            [--participant ROLE=AGENT[:MODEL]]... [--rounds N] \
            [--docs|--no-docs] [--helpers] PROMPT
```

A read-only audit discussion with a chosen lens.

```sh
swarm audit --focus harden "Audit the credential vault in swarm-manager"
```

## Picking a pattern

- One independent task → `run`.
- Independent slices you want worked at once → `fanout`.
- Roles that need to argue/iterate → `discuss`.
- Big context, one synthesizing brain → `metadirector`.
- Read-only review with a named lens → `audit` (code) or `design` (UI).

Not sure which manager/participants to name? Ask the engine:

```sh
swarm recommend "Refactor the routing layer and add tests"
```

## Reading results

`run` / `fanout` runs are tracked as jobs; discussions are tracked as sessions.

```sh
swarm status [JOB_ID]      # recent jobs, or one job
swarm result [JOB_ID]      # print a job result (defaults to latest)
swarm cancel JOB_ID        # terminate a queued/running job
swarm sessions             # recent discussion sessions
swarm events SESSION_ID    # the JSONL event stream for a session
swarm transcript SESSION_ID# the human-readable transcript
```

Use `--background` on a `run` to queue it and return immediately, then poll
with `swarm status` / `swarm result`.

## How backends are configured

The host agent does not pick *models* in source — backends are declared as
descriptors in `~/.swarm/config.toml`. Any name you pass to `--agent`,
`--worker ROLE=NAME`, or `--participant ROLE=NAME` must resolve to a declared
or built-in backend (public `claude` / `codex` resolve out of the box).

- `[backend.<id>]` blocks declare backends. Three kinds: `cli` (wrap any
  command-line agent), `openai-compatible` (any `/v1/chat/completions`
  endpoint), and `native` (the in-process harness). See
  `docs/authoring-a-backend.md`.
- `swarm provider ...` manages stored credentials for `native` backends
  (`provider add`, `provider key set` reads the key from stdin, `provider
  list`, `provider key check`).
- `swarm doctor` health-checks the whole setup — config parse, every backend's
  readiness, routing that points nowhere, and credential status. Run it first
  when a backend name fails to resolve. `swarm doctor --probe` additionally
  sends one tiny real request per CLI agent to confirm it is authenticated.

If you pass a backend name that is not registered, dispatch fails with an error
listing every backend that *is* — run `swarm doctor` to see the same picture.
