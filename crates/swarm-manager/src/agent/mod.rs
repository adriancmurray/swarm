//! Single-agent tool loop.
//!
//! An [`Agent`] drives one conversation: it seeds the transcript with the
//! preset's system prompt, then on each turn asks the [`Provider`] for a chat
//! completion (offering the registered tools). If the model returns tool
//! calls, the agent executes each via the [`ToolRegistry`], appends the
//! results to the transcript, and loops; if the model returns plain text, the
//! agent returns it as the final answer. The loop is capped at
//! `max_tool_iterations`; exceeding the cap is a loud, typed error — never a
//! silent stop.
//!
//! There is no permission broker, sandbox, or skills loader in this core: the
//! built-in tools run unwrapped and the tool set is whatever the caller
//! registered. Token usage is accumulated across every model turn (including
//! the tool-calling turns) into the returned [`AgentTurn`].

use crate::provider::{Message, Provider, ToolCall, Usage};
use crate::tools::ToolRegistry;
use crate::AgentConfig;
use std::sync::Arc;

/// A single executed tool call and the textual result fed back to the model.
#[derive(Debug, Clone)]
pub struct ToolInvocation {
    /// The tool-call id the model assigned.
    pub id: String,
    /// The tool name that was dispatched.
    pub name: String,
    /// The arguments the model supplied.
    pub arguments: serde_json::Value,
    /// The textual result appended to the transcript (on a tool error this is
    /// the `Error: …` string fed back to the model, matching the loop's
    /// feed-back-and-continue behaviour).
    pub result: String,
}

/// The outcome of running the agent to a final assistant message.
#[derive(Debug, Clone)]
pub struct AgentTurn {
    /// The final assistant text (`"(no response)"` if the model returned an
    /// empty completion with no tool calls).
    pub text: String,
    /// Every tool call executed across the turn, in execution order.
    pub tool_calls: Vec<ToolInvocation>,
    /// Token usage summed across every model completion in the turn.
    pub usage: Usage,
}

/// Typed failures from the agent loop.
#[derive(thiserror::Error, Debug)]
pub enum AgentError {
    /// The underlying provider failed a chat completion.
    #[error("provider error: {0}")]
    Provider(#[from] crate::provider::ProviderError),
    /// A tool failed to execute. The loop itself feeds tool errors back to the
    /// model as a result string and keeps going (matching the ported source),
    /// so this variant is reserved for callers that opt into surfacing tool
    /// failures as hard errors.
    #[error("tool error: {0}")]
    Tool(String),
    /// The loop hit `max_tool_iterations` without the model producing a final
    /// (tool-call-free) message. No silent truncation — the caller is told.
    #[error("max tool iterations ({0}) exceeded without a final response")]
    MaxIterationsExceeded(usize),
}

/// A single-agent tool loop bound to a provider, a tool registry, and a
/// runtime configuration.
pub struct Agent {
    provider: Arc<dyn Provider>,
    tools: ToolRegistry,
    config: AgentConfig,
    messages: Vec<Message>,
}

impl Agent {
    /// Build an agent, seeding the transcript with the config's system prompt.
    pub fn new(provider: Arc<dyn Provider>, tools: ToolRegistry, config: AgentConfig) -> Self {
        let messages = vec![Message::system(config.system_prompt.clone())];
        Self {
            provider,
            tools,
            config,
            messages,
        }
    }

    /// The current conversation transcript (system prompt first).
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// Append a user message and run the loop to a final assistant response.
    pub async fn process(&mut self, input: &str) -> Result<AgentTurn, AgentError> {
        self.messages.push(Message::user(input));
        self.run_until_response().await
    }

    /// Synchronous bridge over [`process`] for callers with no async context.
    ///
    /// Builds a private current-thread `tokio` runtime and `block_on`s the
    /// async loop, so the agent owns its runtime entirely — there must be no
    /// ambient runtime on the calling thread (a nested `block_on` panics).
    /// `enable_all` turns on the IO + time drivers the real HTTP providers
    /// need; a scripted provider ignores them but they are harmless.
    pub fn run_blocking(&mut self, input: &str) -> Result<AgentTurn, AgentError> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| AgentError::Tool(format!("failed to build agent runtime: {e}")))?;
        runtime.block_on(self.process(input))
    }

    /// Drive `chat` → tool dispatch → `chat` until the model returns a final
    /// (tool-call-free) message, accumulating usage along the way. The
    /// iteration cap is `config.max_tool_iterations`; exceeding it is a typed
    /// error, never a silent stop.
    async fn run_until_response(&mut self) -> Result<AgentTurn, AgentError> {
        let mut tool_calls = Vec::new();
        let mut usage = Usage::default();

        let tools = self.tools.to_openai_tools();
        let tools_opt = if tools.is_empty() { None } else { Some(tools) };

        for _ in 0..self.config.max_tool_iterations {
            let response = self
                .provider
                .chat(self.messages.clone(), tools_opt.clone())
                .await?;

            accumulate_usage(&mut usage, &response.usage);

            if !response.tool_calls.is_empty() {
                // Record the assistant turn that emitted the tool calls so the
                // transcript keeps the tool_calls → tool_result invariant.
                self.messages.push(Message {
                    role: "assistant".to_string(),
                    content: response.content.clone(),
                    reasoning_content: response.reasoning_content.clone(),
                    tool_calls: Some(response.tool_calls.clone()),
                    tool_call_id: None,
                });

                for tool_call in &response.tool_calls {
                    let result = self.dispatch_tool(tool_call).await;
                    self.messages
                        .push(Message::tool_result(&tool_call.id, result.clone()));
                    tool_calls.push(ToolInvocation {
                        id: tool_call.id.clone(),
                        name: tool_call.name.clone(),
                        arguments: tool_call.arguments.clone(),
                        result,
                    });
                }

                continue;
            }

            // No tool calls — this is the final answer.
            let text = response
                .content
                .unwrap_or_else(|| "(no response)".to_string());
            self.messages.push(Message::assistant(text.clone()));
            return Ok(AgentTurn {
                text,
                tool_calls,
                usage,
            });
        }

        Err(AgentError::MaxIterationsExceeded(
            self.config.max_tool_iterations,
        ))
    }

    /// Execute a single tool call. A tool failure is folded into an
    /// `Error: …` string and fed back to the model (matching the ported
    /// loop), rather than aborting the turn.
    async fn dispatch_tool(&self, tool_call: &ToolCall) -> String {
        match self
            .tools
            .execute(&tool_call.name, tool_call.arguments.clone())
            .await
        {
            Ok(output) => output,
            Err(e) => format!("Error: {e}"),
        }
    }
}

fn accumulate_usage(total: &mut Usage, delta: &Usage) {
    total.prompt_tokens = total.prompt_tokens.saturating_add(delta.prompt_tokens);
    total.completion_tokens = total
        .completion_tokens
        .saturating_add(delta.completion_tokens);
    total.total_tokens = total.total_tokens.saturating_add(delta.total_tokens);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{LLMResponse, ProviderError, ToolCall, Usage};
    use crate::tools::Tool;
    use async_trait::async_trait;
    use serde_json::{json, Value};
    use std::sync::Mutex;

    /// A provider that pops pre-built responses from a queue. No network.
    /// Recording each call's message slice lets tests assert transcript shape.
    struct ScriptedProvider {
        responses: Mutex<std::collections::VecDeque<LLMResponse>>,
        calls: Mutex<Vec<Vec<Message>>>,
    }

    impl ScriptedProvider {
        fn new(responses: Vec<LLMResponse>) -> Self {
            Self {
                responses: Mutex::new(responses.into_iter().collect()),
                calls: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl Provider for ScriptedProvider {
        async fn chat(
            &self,
            messages: Vec<Message>,
            _tools: Option<Vec<Value>>,
        ) -> Result<LLMResponse, ProviderError> {
            self.calls.lock().unwrap().push(messages);
            self.responses
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| ProviderError::ParseResponse("no scripted response".to_string()))
        }
    }

    /// A provider that always returns the same response (used to exhaust the
    /// iteration cap without draining a finite queue).
    struct LoopingProvider {
        response: LLMResponse,
    }

    #[async_trait]
    impl Provider for LoopingProvider {
        async fn chat(
            &self,
            _messages: Vec<Message>,
            _tools: Option<Vec<Value>>,
        ) -> Result<LLMResponse, ProviderError> {
            Ok(self.response.clone())
        }
    }

    /// Echoes its `message` argument. Trivial in-memory tool.
    struct EchoTool;

    #[async_trait]
    impl Tool for EchoTool {
        fn name(&self) -> &str {
            "echo"
        }
        fn description(&self) -> &str {
            "Echo the message argument back"
        }
        fn parameters(&self) -> Value {
            json!({
                "type": "object",
                "properties": { "message": { "type": "string" } },
                "required": ["message"]
            })
        }
        async fn execute(&self, args: Value) -> anyhow::Result<String> {
            let message = args["message"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("missing message"))?;
            Ok(message.to_string())
        }
    }

    /// Always fails — exercises the tool-error feed-back path.
    struct BoomTool;

    #[async_trait]
    impl Tool for BoomTool {
        fn name(&self) -> &str {
            "boom"
        }
        fn description(&self) -> &str {
            "Always errors"
        }
        fn parameters(&self) -> Value {
            json!({ "type": "object", "properties": {} })
        }
        async fn execute(&self, _args: Value) -> anyhow::Result<String> {
            Err(anyhow::anyhow!("kaboom"))
        }
    }

    fn usage(prompt: u32, completion: u32) -> Usage {
        Usage {
            prompt_tokens: prompt,
            completion_tokens: completion,
            total_tokens: prompt + completion,
        }
    }

    fn final_response(content: &str, usage: Usage) -> LLMResponse {
        LLMResponse {
            content: Some(content.to_string()),
            reasoning_content: None,
            tool_calls: Vec::new(),
            finish_reason: "stop".to_string(),
            usage,
        }
    }

    fn tool_call_response(call: ToolCall, usage: Usage) -> LLMResponse {
        LLMResponse {
            content: None,
            reasoning_content: None,
            tool_calls: vec![call],
            finish_reason: "tool_calls".to_string(),
            usage,
        }
    }

    fn echo_call(id: &str, message: &str) -> ToolCall {
        ToolCall {
            id: id.to_string(),
            name: "echo".to_string(),
            arguments: json!({ "message": message }),
        }
    }

    fn config_with(max_iterations: usize) -> AgentConfig {
        AgentConfig {
            system_prompt: "You are a test agent.".to_string(),
            max_tool_iterations: max_iterations,
            ..AgentConfig::default()
        }
    }

    fn registry_with(tool: Arc<dyn Tool>) -> ToolRegistry {
        let mut r = ToolRegistry::new();
        r.register(tool);
        r
    }

    #[tokio::test]
    async fn no_tool_call_returns_final_text_immediately() {
        let provider = Arc::new(ScriptedProvider::new(vec![final_response(
            "hello there",
            usage(3, 5),
        )]));
        let mut agent = Agent::new(provider.clone(), ToolRegistry::new(), config_with(5));

        let turn = agent.process("hi").await.unwrap();

        assert_eq!(turn.text, "hello there");
        assert!(turn.tool_calls.is_empty());
        assert_eq!(turn.usage.total_tokens, 8);

        // Provider was called exactly once.
        assert_eq!(provider.calls.lock().unwrap().len(), 1);

        // Transcript: system, user, assistant(final).
        let msgs = agent.messages();
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[0].role, "system");
        assert_eq!(msgs[0].content.as_deref(), Some("You are a test agent."));
        assert_eq!(msgs[1].role, "user");
        assert_eq!(msgs[1].content.as_deref(), Some("hi"));
        assert_eq!(msgs[2].role, "assistant");
        assert_eq!(msgs[2].content.as_deref(), Some("hello there"));
    }

    #[tokio::test]
    async fn single_tool_call_loops_then_returns_final() {
        let provider = Arc::new(ScriptedProvider::new(vec![
            tool_call_response(echo_call("call-1", "ping"), usage(10, 2)),
            final_response("done: ping", usage(4, 6)),
        ]));
        let mut agent = Agent::new(
            provider.clone(),
            registry_with(Arc::new(EchoTool)),
            config_with(5),
        );

        let turn = agent.process("use the tool").await.unwrap();

        assert_eq!(turn.text, "done: ping");

        // One tool invocation recorded with the echoed result.
        assert_eq!(turn.tool_calls.len(), 1);
        assert_eq!(turn.tool_calls[0].id, "call-1");
        assert_eq!(turn.tool_calls[0].name, "echo");
        assert_eq!(turn.tool_calls[0].result, "ping");

        // Usage aggregates across BOTH chat calls.
        assert_eq!(turn.usage.prompt_tokens, 14);
        assert_eq!(turn.usage.completion_tokens, 8);
        assert_eq!(turn.usage.total_tokens, 22);

        // Two model turns.
        assert_eq!(provider.calls.lock().unwrap().len(), 2);

        // Transcript: system, user, assistant(tool_calls), tool, assistant(final).
        let msgs = agent.messages();
        assert_eq!(msgs.len(), 5);
        assert_eq!(msgs[0].role, "system");
        assert_eq!(msgs[1].role, "user");
        assert_eq!(msgs[2].role, "assistant");
        assert!(msgs[2].tool_calls.is_some());
        assert_eq!(msgs[3].role, "tool");
        assert_eq!(msgs[3].tool_call_id.as_deref(), Some("call-1"));
        assert_eq!(msgs[3].content.as_deref(), Some("ping"));
        assert_eq!(msgs[4].role, "assistant");
        assert_eq!(msgs[4].content.as_deref(), Some("done: ping"));

        // The second chat call saw the tool result fed back.
        let calls = provider.calls.lock().unwrap();
        let second = &calls[1];
        assert!(second.iter().any(|m| m.role == "tool"
            && m.content.as_deref() == Some("ping")
            && m.tool_call_id.as_deref() == Some("call-1")));
    }

    #[tokio::test]
    async fn multi_turn_two_tool_calls_then_final() {
        let provider = Arc::new(ScriptedProvider::new(vec![
            tool_call_response(echo_call("c1", "alpha"), usage(1, 1)),
            tool_call_response(echo_call("c2", "beta"), usage(2, 2)),
            final_response("alpha+beta", usage(3, 3)),
        ]));
        let mut agent = Agent::new(
            provider.clone(),
            registry_with(Arc::new(EchoTool)),
            config_with(5),
        );

        let turn = agent.process("two tools please").await.unwrap();

        assert_eq!(turn.text, "alpha+beta");
        assert_eq!(turn.tool_calls.len(), 2);
        assert_eq!(turn.tool_calls[0].result, "alpha");
        assert_eq!(turn.tool_calls[1].result, "beta");

        // Usage summed across all three completions: 6 prompt, 6 completion, 12 total.
        assert_eq!(turn.usage.prompt_tokens, 6);
        assert_eq!(turn.usage.completion_tokens, 6);
        assert_eq!(turn.usage.total_tokens, 12);

        assert_eq!(provider.calls.lock().unwrap().len(), 3);

        // Transcript ordering:
        // system, user, assistant(tc), tool, assistant(tc), tool, assistant(final).
        let roles: Vec<&str> = agent.messages().iter().map(|m| m.role.as_str()).collect();
        assert_eq!(
            roles,
            vec![
                "system",
                "user",
                "assistant",
                "tool",
                "assistant",
                "tool",
                "assistant"
            ]
        );
        assert!(agent.messages()[2].tool_calls.is_some());
        assert!(agent.messages()[4].tool_calls.is_some());
        assert_eq!(agent.messages()[6].content.as_deref(), Some("alpha+beta"));
    }

    #[tokio::test]
    async fn max_iterations_exceeded_is_typed_error_not_silent_stop() {
        // The model keeps asking for a tool, never finalizing. The looping
        // provider returns a tool-call EVERY time, so the queue never drains
        // before the cap — isolating the max-iterations path.
        let provider = Arc::new(LoopingProvider {
            response: tool_call_response(echo_call("loop", "again"), usage(1, 1)),
        });
        let mut agent = Agent::new(provider, registry_with(Arc::new(EchoTool)), config_with(2));

        let err = agent.process("never stop").await.unwrap_err();
        match err {
            AgentError::MaxIterationsExceeded(2) => {}
            other => panic!("expected MaxIterationsExceeded(2), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn tool_error_is_fed_back_and_loop_continues() {
        // First turn calls the failing tool; the error string is fed back as a
        // tool result and the loop continues to the scripted final answer.
        let provider = Arc::new(ScriptedProvider::new(vec![
            tool_call_response(
                ToolCall {
                    id: "boom-1".to_string(),
                    name: "boom".to_string(),
                    arguments: json!({}),
                },
                usage(1, 1),
            ),
            final_response("recovered", usage(1, 1)),
        ]));
        let mut agent = Agent::new(
            provider.clone(),
            registry_with(Arc::new(BoomTool)),
            config_with(5),
        );

        let turn = agent.process("trigger the failing tool").await.unwrap();

        assert_eq!(turn.text, "recovered");
        assert_eq!(turn.tool_calls.len(), 1);
        assert!(
            turn.tool_calls[0].result.contains("Error:")
                && turn.tool_calls[0].result.contains("kaboom"),
            "tool error should be folded into the result string, got: {}",
            turn.tool_calls[0].result
        );

        // The error string was fed back to the model as a tool message.
        let calls = provider.calls.lock().unwrap();
        let second = &calls[1];
        assert!(second.iter().any(|m| m.role == "tool"
            && m.tool_call_id.as_deref() == Some("boom-1")
            && m.content.as_deref().is_some_and(|c| c.contains("kaboom"))));
    }

    #[tokio::test]
    async fn empty_completion_without_tool_calls_yields_placeholder() {
        let provider = Arc::new(ScriptedProvider::new(vec![LLMResponse {
            content: None,
            reasoning_content: None,
            tool_calls: Vec::new(),
            finish_reason: "stop".to_string(),
            usage: Usage::default(),
        }]));
        let mut agent = Agent::new(provider, ToolRegistry::new(), config_with(5));

        let turn = agent.process("hi").await.unwrap();
        assert_eq!(turn.text, "(no response)");
        assert!(turn.tool_calls.is_empty());
    }

    #[test]
    fn run_blocking_drives_loop_on_a_thread_with_no_ambient_runtime() {
        // A plain `#[test]` (no `#[tokio::test]`) so there is no ambient
        // runtime: `run_blocking` must build and own its own. Scripted
        // provider → no network.
        let provider = Arc::new(ScriptedProvider::new(vec![
            tool_call_response(echo_call("b1", "sync"), usage(2, 1)),
            final_response("done: sync", usage(3, 4)),
        ]));
        let mut agent = Agent::new(
            provider.clone(),
            registry_with(Arc::new(EchoTool)),
            config_with(5),
        );

        let turn = agent.run_blocking("use the tool synchronously").unwrap();

        assert_eq!(turn.text, "done: sync");
        assert_eq!(turn.tool_calls.len(), 1);
        assert_eq!(turn.tool_calls[0].result, "sync");
        assert_eq!(turn.usage.total_tokens, 10);
        assert_eq!(provider.calls.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn unknown_tool_surfaces_as_error_result_and_continues() {
        // Model calls a tool that isn't registered; the registry's
        // "Tool not found" error is fed back, and the loop finishes.
        let provider = Arc::new(ScriptedProvider::new(vec![
            tool_call_response(echo_call("missing-1", "x"), usage(0, 0)),
            final_response("ok", usage(0, 0)),
        ]));
        let mut agent = Agent::new(provider, ToolRegistry::new(), config_with(5));

        let turn = agent.process("call a missing tool").await.unwrap();
        assert_eq!(turn.text, "ok");
        assert_eq!(turn.tool_calls.len(), 1);
        assert!(turn.tool_calls[0].result.contains("Tool not found"));
    }
}
