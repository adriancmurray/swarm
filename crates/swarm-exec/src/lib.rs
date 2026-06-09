//! `swarm-exec` — swarm engine for the swarm runtime.
//!
//! This crate sits ABOVE `swarm-kernel` in the DAG:
//! `swarm-contracts ← swarm-core ← swarm-store ← swarm-kernel ← **swarm-exec**`
//!
//! # Contents (P5-S3 extraction)
//!
//! - Session types + lifecycle — `session`
//! - Backend availability checks + error classification — `preflight`
//! - Single-agent subprocess execution — `executor`
//! - Prompt / transcript synthesis — `synthesis`
//! - Background job runtime — `background_runtime`
//! - Live observation + watch runtime — `monitor_runtime`
//! - Swarm / discussion orchestration — `orchestration`
//!
//! # Gate-2 isolation invariant
//!
//! This crate depends ONLY on:
//! - `swarm-kernel` (stateless leaf modules)
//! - `swarm-store` (file-backed store machinery)
//! - `swarm-core` (repo traits + companion types)
//! - `swarm-contracts` (wire types)
//! - External crates: serde, serde_json, toml, sysinfo, notify, libc (unix)
//!
//! No external-system deps.

pub mod backend_registry;
pub mod background_runtime;
pub mod cli_backend;
pub mod executor;
pub mod monitor_runtime;
#[cfg(feature = "native")]
pub mod native_backend;
#[cfg(feature = "openai")]
pub mod openai_backend;
pub mod orchestration;
pub mod preflight;
pub mod session;
pub mod synthesis;
