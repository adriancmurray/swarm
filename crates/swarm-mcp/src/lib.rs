//! `swarm-mcp` — MCP server layer for the swarm runtime.
//!
//! Populated in P5-S4. Contains the hand-written MCP stdio loop plus all
//! modules that serve or support the MCP surface:
//!
//! - `mcp_helpers`  — argument parsing / JSON-RPC envelope builders
//! - `mcp_schema`   — descriptor table (36 tools), pretty-JSON serializer
//! - `mcp_dispatch` — stdio loop + method dispatch + thin CLI adapters
//! - `manifest`     — `swarm.manifest/v1` payload builder
//! - `overview`     — `agent-swarm/overview/v1` assembler
//! - `report`       — session/job/process JSON assemblers
//!
//! The engine does not self-register into any service registry by default;
//! opt-in registration lives behind the `registry` feature (`registry_hook`).

pub mod manifest;
pub mod mcp_dispatch;
pub mod mcp_helpers;
pub mod mcp_schema;
pub mod overview;
/// Optional, generic service-registry hook (off by default; see `registry` feature).
#[cfg(feature = "registry")]
pub mod registry_hook;
pub mod report;

#[cfg(feature = "rmcp")]
pub mod rmcp_server;
