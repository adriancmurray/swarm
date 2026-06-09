//! Tool abstraction for the single-agent manager: the [`Tool`] trait, a
//! [`ToolRegistry`], and the built-in tools the agent loop dispatches.
//!
//! The trait and registry are data-only and compile with no cargo feature
//! (only `async_trait`). The built-in tool implementations need an async
//! runtime and live behind the `runtime` feature (exec / file / output) or the
//! `http` feature (web).

pub mod registry;

#[cfg(feature = "runtime")]
pub mod exec;
#[cfg(feature = "runtime")]
pub mod file;
#[cfg(feature = "runtime")]
pub mod output;
#[cfg(feature = "http")]
pub mod web;

pub use registry::ToolRegistry;

#[cfg(feature = "runtime")]
pub use exec::ExecTool;
#[cfg(feature = "runtime")]
pub use file::{EditFileTool, ListDirTool, ReadFileTool, WriteFileTool};
#[cfg(feature = "http")]
pub use web::{WebFetchTool, WebSearchTool};

use async_trait::async_trait;
use serde_json::Value;

/// Trait that all tools must implement.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Tool name (used in function calling).
    fn name(&self) -> &str;

    /// Human-readable description.
    fn description(&self) -> &str;

    /// JSON Schema for parameters.
    fn parameters(&self) -> Value;

    /// Execute the tool with the given arguments.
    async fn execute(&self, args: Value) -> anyhow::Result<String>;

    /// Execute after the permission broker has admitted the call.
    ///
    /// The v1 broker is permissive, so this defaults to plain [`Tool::execute`].
    /// The override point is kept so a later arc can swap in a real broker
    /// without changing every tool.
    async fn execute_approved(&self, args: Value) -> anyhow::Result<String> {
        self.execute(args).await
    }

    /// Convert to OpenAI tool format.
    fn to_openai_tool(&self) -> Value {
        serde_json::json!({
            "type": "function",
            "function": {
                "name": self.name(),
                "description": self.description(),
                "parameters": self.parameters()
            }
        })
    }
}
