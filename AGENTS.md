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

## Driving swarm from a host agent

If you are a host agent (Codex, Claude Code, Gemini CLI, etc.) deciding how to
orchestrate swarm, read [`skills/using-swarm/SKILL.md`](skills/using-swarm/SKILL.md)
— it is the operational playbook for the CLI verbs and result inspection.

To add a backend, see [`docs/authoring-a-backend.md`](docs/authoring-a-backend.md).
