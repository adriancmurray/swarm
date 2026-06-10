//! Single-agent manager core for the swarm orchestrator.
//!
//! This crate is standalone and mesh-free: it provides the provider
//! abstraction (`Provider` trait + `ProviderType`), a persistent provider
//! registry, an at-rest credential vault with an environment-variable
//! fallback, agent presets, and the tool registry plus built-in tools. The
//! agent loop and concrete provider implementations land in later tasks.

#[cfg(feature = "runtime")]
pub mod agent;
pub mod preset;
pub mod provider;
pub mod skills;
pub mod tools;

#[cfg(feature = "runtime")]
pub use agent::{Agent, AgentError, AgentTurn, ToolInvocation};

pub use preset::{
    AgentConfig, AgentOwner, ConfigUpdate, ConsumerPolicy, Preset, PresetRequest, PresetStore,
};
pub use provider::{
    create_provider, KeyStatus, LLMResponse, Message, Provider, ProviderConfig, ProviderError,
    ProviderRegistry, ProviderType, ToolCall, Usage,
};
pub use skills::{
    load_skills, parse_skill, Skill, SkillError, SkillLoadIssue, SkillSelectionIssue, SkillSet,
};
pub use tools::{Tool, ToolRegistry};
