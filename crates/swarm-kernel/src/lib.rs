//! `swarm-kernel` — stateless leaf modules for the swarm runtime.
//!
//! This crate sits BELOW `swarm-exec` in the DAG:
//! `swarm-contracts ← swarm-core ← swarm-store ← swarm-kernel ← swarm-exec`
//!
//! # Contents (P5-S2.5 extraction)
//!
//! - Agent model + choice — `agent`
//! - Declarative backend descriptors — `backend_descriptor`
//! - CLI argument parsing + prompt dispatch args — `args`
//! - Config loading + typed structs — `config`
//! - Harness-neutral conductor activity records — `conductor`
//! - Backend fallback routing — `routing`
//! - Workspace context gathering — `context`
//! - Text formatting helpers — `format`
//! - Binary resolution + path helpers — `resolver`
//! - Role profiles — `profiles`
//! - Typed ID re-exports — `ids`
//! - Job type discriminants re-exports — `job_types`
//! - Event kind re-exports — `events`
//! - Process / OS helpers — `process`
//! - Telemetry types — `telemetry`
//! - Prompt builders (audit + design) — `prompts`
//!
//! # Gate-2 isolation invariant
//!
//! This crate depends ONLY on:
//! - `swarm-store` (store primitives, job record)
//! - `swarm-core` (repo traits + companion types)
//! - `swarm-contracts` (wire types)
//! - External crates: serde, serde_json, toml, sysinfo, libc (unix)
//!
//! No external-system deps.

pub mod agent;
pub mod args;
pub mod backend_abi;
pub mod backend_descriptor;
pub mod conductor;
pub mod config;
pub mod context;
pub mod events;
pub mod format;
pub mod ids;
pub mod job_types;
pub mod process;
pub mod profiles;
pub mod prompts;
pub mod resolver;
pub mod routing;
pub mod task_classifier;
pub mod telemetry;
