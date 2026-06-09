//! `swarm-contracts` — canonical contract types for the swarm runtime.
//!
//! This crate is the IDL source of truth for wire-stable types shared across
//! the swarm crates. It has no heavyweight dependencies (serde only) and must
//! compile standalone.
//!
//! # Modules
//!
//! - [`ids`] — `SessionId`, `JobId`, `ProposalId`, `PresetId` newtypes
//! - [`events`] — `EventKind` (31 variants + bare-string wire), `SessionEventV2` envelope
//! - [`jobs`] — `JobStatus`, `JobAgent`, `JobMode` enums + `JobRecord` struct
//! - [`mcp`] — `McpToolDescriptor` shared MCP tool-descriptor type
//! - [`telemetry`] — `AgentObservation`, `AgentFeedback`, `AgentProposal`, `AgentProposalVote`
//! - [`package`] — `LayerReport` envelope

pub mod events;
pub mod ids;
pub mod jobs;
pub mod mcp;
pub mod package;
pub mod telemetry;
