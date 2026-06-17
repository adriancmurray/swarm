# AGENTS.md

`swarm` is a command-line engine that orchestrates other agents: it fans a task
out to parallel workers, runs structured multi-role discussions, and lets a
manager plan or synthesize from supplied context. Backends (CLI agents, HTTP
APIs, or an in-process harness) are wired in TOML config, not source code.

## Build & test

```sh
cargo build --release && cargo test
```

## Key verbs

- `run` — single routed task (a bare prompt also defaults to `run`).
- `fanout` (alias `swarm`) — parallel workers, then synthesis.
- `discuss` — structured multi-participant debate over `--rounds`.
- `metadirector` — manager plans/synthesizes from supplied context.
- `audit` / `design` — read-only review discussions with a `--focus` lens.
- `status` / `result` / `sessions` / `events` / `transcript` — inspect work.
- `provider` / `doctor` / `skills` / `scaffold-backend` — config & health.

Run `swarm doctor` to verify config, backends, routing, and credentials in one
pass. Run `swarm skills list` to see the `SKILL.md` skills a native backend can
load into a worker.

## Learned routing (Phase-2)

Every worker and manager run records an outcome observation. By default
(`[reliability].learned_routing = true`, gated by `learned_min_observations`,
default 3), swarm reads those observations back at dispatch time and, **when a
role has neither an explicit `[routes.<role>]` route nor a global
`[reliability].fallback_chain`**, fills the fallback gap with agents ranked by
observed success. Explicit config always wins — learning only fills the silent
gap, it never overrides the caller's primary or a configured chain. Cold
telemetry (no data) falls back to the static default, so a fresh install is
byte-identical to the old behavior.

Pass `--no-learned` to any fanout/discuss/audit/design/converge run to opt out
for that run, or set `learned_routing = false` in config to disable globally.
Each application emits a `learned_routing_applied` event in `events.jsonl`; the
`swarm doctor` `learned routing` section reports whether the store is readable
and how many observations it holds.

## Driving swarm from a host agent

If you are a host agent (Codex, Claude Code, Gemini CLI, etc.) deciding how to
orchestrate swarm, read [`skills/using-swarm/SKILL.md`](skills/using-swarm/SKILL.md)
— it is the operational playbook for the CLI verbs and result inspection.

To add a backend, see [`docs/authoring-a-backend.md`](docs/authoring-a-backend.md).
