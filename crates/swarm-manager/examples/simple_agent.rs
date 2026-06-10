//! Minimal end-to-end agent: one provider, the built-in tools, one prompt.
//!
//! This example needs the `http` feature (it builds a real HTTP-backed
//! provider) and an API key in the environment. It is gated via
//! `required-features = ["http"]` in `Cargo.toml`, so the default
//! `cargo build` skips it.
//!
//! Run it with:
//!
//! ```bash
//! OPENAI_API_KEY=sk-... \
//!   cargo run --example simple_agent -p swarm-manager --features http
//! ```
//!
//! `main` is a plain `fn` (NOT `#[tokio::main]`): the agent's `run_blocking`
//! owns a private current-thread runtime and would panic if called inside an
//! ambient async runtime.

use std::sync::Arc;

use swarm_manager::tools::{
    EditFileTool, ExecTool, ListDirTool, ReadFileTool, WebFetchTool, WriteFileTool,
};
use swarm_manager::{
    create_provider, Agent, AgentConfig, ProviderConfig, ProviderType, ToolRegistry,
};

fn main() -> anyhow::Result<()> {
    // 1. Read the API key from the environment. Never hard-code secrets.
    let api_key = std::env::var("OPENAI_API_KEY").map_err(|_| {
        anyhow::anyhow!("set OPENAI_API_KEY in the environment before running this example")
    })?;

    // 2. Describe one provider instance and build the HTTP-backed provider.
    //    `create_provider` reads the model from `models.first()`, so set it
    //    explicitly rather than handing the provider an empty model string.
    let mut config = ProviderConfig::new(
        "OpenAI".to_string(),
        ProviderType::OpenAI,
        None, // use the provider's default endpoint
        Some(api_key),
    );
    config.models = vec!["gpt-5.5".to_string()];
    let provider = create_provider(&config)?;

    // 3. Register the built-in tools. They all run unwrapped in v1 (no
    //    sandbox, no permission broker yet). `ExecTool::new` takes a per-call
    //    timeout in seconds; the file tools default sensibly.
    let mut tools = ToolRegistry::new();
    tools.register(Arc::new(ExecTool::new(30)));
    tools.register(Arc::new(ReadFileTool::default()));
    tools.register(Arc::new(WriteFileTool::default()));
    tools.register(Arc::new(EditFileTool::default()));
    tools.register(Arc::new(ListDirTool::default()));
    tools.register(Arc::new(WebFetchTool::default()));

    // 4. Build the agent. The loop reads only `system_prompt` and
    //    `max_tool_iterations` off the config — the provider, model, endpoint,
    //    and key are already baked into the provider built above.
    let agent_config = AgentConfig {
        system_prompt: "You are a concise assistant. Answer in one sentence.".to_string(),
        ..AgentConfig::default()
    };
    let mut agent = Agent::new(provider, tools, agent_config);

    // 5. Run one prompt to a final answer. `run_blocking` drives the async
    //    tool loop to completion on the calling thread.
    let turn = agent.run_blocking("In one sentence, what is a tool-using agent?")?;

    println!("assistant: {}", turn.text);
    println!(
        "tools used: {} | tokens: {}",
        turn.tool_calls.len(),
        turn.usage.total_tokens
    );
    Ok(())
}
