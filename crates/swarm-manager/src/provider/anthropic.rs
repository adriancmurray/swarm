//! Anthropic Messages API provider.
//!
//! Translates the local OpenAI-shaped `Vec<Message>` into Anthropic's
//! Messages API request body (top-level `system` string + `messages` array
//! of user/assistant only), POSTs to `<base>/messages`, and parses the
//! response's content-block array back into the local `LLMResponse` shape.
//!
//! Reference: https://docs.anthropic.com/en/api/messages

use super::types::{header_pairs, network_error, parse_response_error};
use super::{classify_http_error, LLMResponse, Message, Provider, ProviderError, ToolCall, Usage};
use async_trait::async_trait;
use serde_json::{json, Value};

const ANTHROPIC_VERSION: &str = "2023-06-01";
const DEFAULT_MAX_TOKENS: usize = 4096;

/// Anthropic Messages API provider.
pub struct AnthropicProvider {
    pub api_key: String,
    pub base_url: String,
    pub model: String,
    pub temperature: f32,
    pub max_tokens: Option<usize>,
    pub client: reqwest::Client,
}

impl AnthropicProvider {
    pub fn new(
        api_key: String,
        base_url: String,
        model: String,
        temperature: f32,
        max_tokens: Option<usize>,
    ) -> Self {
        Self {
            api_key,
            base_url,
            model,
            temperature,
            max_tokens,
            client: reqwest::Client::new(),
        }
    }

    /// Build the Anthropic Messages API request body from local types.
    ///
    /// Pulls any `role: "system"` message into a top-level `system` string
    /// (joined with `\n\n` if multiple). Remaining `user`/`assistant`
    /// messages flow into `messages[]`. OpenAI-shaped tools, if supplied,
    /// are translated to Anthropic's `{name, description, input_schema}`.
    fn build_request_body(&self, messages: Vec<Message>, tools: Option<Vec<Value>>) -> Value {
        let mut system_parts: Vec<String> = Vec::new();
        let mut api_messages: Vec<Value> = Vec::with_capacity(messages.len());

        for m in messages {
            if m.role == "system" {
                if let Some(text) = m.content {
                    system_parts.push(text);
                }
                continue;
            }
            let role = if m.role == "assistant" {
                "assistant"
            } else {
                "user"
            };
            api_messages.push(json!({
                "role": role,
                "content": m.content.unwrap_or_default(),
            }));
        }

        let mut body = json!({
            "model": self.model,
            "max_tokens": self.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
            "temperature": self.temperature,
            "messages": api_messages,
        });

        if !system_parts.is_empty() {
            body["system"] = Value::String(system_parts.join("\n\n"));
        }

        if let Some(ts) = tools {
            body["tools"] = Value::Array(ts.into_iter().map(translate_tool).collect());
        }

        body
    }

    /// Parse Anthropic's Messages API response JSON into the local shape.
    ///
    /// `content` is an array of typed blocks; `text` blocks join into
    /// `LLMResponse.content`, `tool_use` blocks become `ToolCall` entries.
    /// `usage` carries `input_tokens`/`output_tokens`; `total_tokens` is
    /// computed locally since Anthropic doesn't emit it.
    fn parse_response(body: Value) -> Result<LLMResponse, ProviderError> {
        let content_blocks = body
            .get("content")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                ProviderError::ParseResponse("response missing `content` array".to_string())
            })?;

        let mut text_parts: Vec<String> = Vec::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();

        for block in content_blocks {
            match block.get("type").and_then(Value::as_str) {
                Some("text") => {
                    if let Some(t) = block.get("text").and_then(Value::as_str) {
                        text_parts.push(t.to_string());
                    }
                }
                Some("tool_use") => {
                    let id = block
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    let name = block
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    let arguments = block.get("input").cloned().unwrap_or(Value::Null);
                    tool_calls.push(ToolCall {
                        id,
                        name,
                        arguments,
                    });
                }
                _ => {}
            }
        }

        let finish_reason = body
            .get("stop_reason")
            .and_then(Value::as_str)
            .unwrap_or("end_turn")
            .to_string();

        let usage = body
            .get("usage")
            .map(|u| {
                let input = u.get("input_tokens").and_then(Value::as_u64).unwrap_or(0) as u32;
                let output = u.get("output_tokens").and_then(Value::as_u64).unwrap_or(0) as u32;
                Usage {
                    prompt_tokens: input,
                    completion_tokens: output,
                    total_tokens: input + output,
                }
            })
            .unwrap_or_default();

        let content = if text_parts.is_empty() {
            None
        } else {
            Some(text_parts.join(""))
        };

        Ok(LLMResponse {
            content,
            reasoning_content: None,
            tool_calls,
            finish_reason,
            usage,
        })
    }
}

/// Translate one OpenAI-shape tool entry into Anthropic's tool shape.
///
/// OpenAI: `{ type: "function", function: { name, description, parameters } }`
/// Anthropic: `{ name, description, input_schema }`. Already-Anthropic-shaped
/// tools (with `input_schema`) pass through untouched.
fn translate_tool(t: Value) -> Value {
    if t.get("input_schema").is_some() {
        return t;
    }
    let inner = t.get("function").cloned().unwrap_or(t);
    let name = inner.get("name").cloned().unwrap_or(Value::Null);
    let description = inner.get("description").cloned().unwrap_or(Value::Null);
    let input_schema = inner
        .get("parameters")
        .cloned()
        .or_else(|| inner.get("input_schema").cloned())
        .unwrap_or_else(|| json!({"type": "object"}));
    json!({
        "name": name,
        "description": description,
        "input_schema": input_schema,
    })
}

#[async_trait]
impl Provider for AnthropicProvider {
    async fn chat(
        &self,
        messages: Vec<Message>,
        tools: Option<Vec<Value>>,
    ) -> Result<LLMResponse, ProviderError> {
        if self.api_key.is_empty() {
            return Err(ProviderError::NotConfigured("API key not set".to_string()));
        }

        let url = format!("{}/messages", self.base_url.trim_end_matches('/'));
        let body = self.build_request_body(messages, tools);

        let res = self
            .client
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(network_error)?;

        if !res.status().is_success() {
            let status = res.status().as_u16();
            let headers = header_pairs(res.headers());
            let text = res.text().await.map_err(network_error)?;
            return Err(classify_http_error(status, &headers, &text, &self.model));
        }

        let response_json: Value = res.json().await.map_err(parse_response_error)?;
        Self::parse_response(response_json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::mock::single_response_server;

    fn provider() -> AnthropicProvider {
        AnthropicProvider::new(
            "test-key".to_string(),
            "https://api.anthropic.com/v1".to_string(),
            "claude-sonnet-4-6".to_string(),
            0.7,
            Some(1024),
        )
    }

    #[test]
    fn build_request_body_extracts_system_and_maps_roles() {
        let p = provider();
        let messages = vec![
            Message::system("You are a concise assistant."),
            Message::user("What is 2+2?"),
            Message::assistant("4."),
            Message::user("Thanks."),
        ];

        let body = p.build_request_body(messages, None);

        assert_eq!(body["model"], json!("claude-sonnet-4-6"));
        let temp = body["temperature"].as_f64().expect("temperature is number");
        assert!((temp - 0.7).abs() < 1e-5, "temperature ~= 0.7, got {temp}");
        assert_eq!(body["max_tokens"], json!(1024));
        assert_eq!(body["system"], json!("You are a concise assistant."));

        let msgs = body["messages"].as_array().expect("messages is array");
        assert_eq!(msgs.len(), 3, "system extracted, three remain");
        assert_eq!(msgs[0]["role"], json!("user"));
        assert_eq!(msgs[0]["content"], json!("What is 2+2?"));
        assert_eq!(msgs[1]["role"], json!("assistant"));
        assert_eq!(msgs[2]["role"], json!("user"));

        assert!(
            body.get("tools").is_none_or(|v| v.is_null()),
            "tools absent when none requested"
        );
    }

    #[test]
    fn build_request_body_translates_openai_tools_to_input_schema() {
        let p = provider();
        let openai_tool = json!({
            "type": "function",
            "function": {
                "name": "get_weather",
                "description": "Look up weather for a city.",
                "parameters": {
                    "type": "object",
                    "properties": { "city": { "type": "string" } },
                    "required": ["city"],
                },
            },
        });
        let body = p.build_request_body(vec![Message::user("weather?")], Some(vec![openai_tool]));

        let tools = body["tools"].as_array().expect("tools is array");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["name"], json!("get_weather"));
        assert_eq!(
            tools[0]["description"],
            json!("Look up weather for a city.")
        );
        assert_eq!(tools[0]["input_schema"]["type"], json!("object"));
        assert_eq!(tools[0]["input_schema"]["required"], json!(["city"]));
        assert!(tools[0].get("type").is_none());
        assert!(tools[0].get("function").is_none());
        assert!(tools[0].get("parameters").is_none());
    }

    #[test]
    fn build_request_body_defaults_max_tokens_when_none() {
        let p = AnthropicProvider::new(
            "test-key".to_string(),
            "https://api.anthropic.com/v1".to_string(),
            "claude-haiku-4-5".to_string(),
            0.5,
            None,
        );
        let body = p.build_request_body(vec![Message::user("hi")], None);
        assert_eq!(body["max_tokens"], json!(DEFAULT_MAX_TOKENS));
    }

    #[test]
    fn parse_response_text_only() {
        let raw = json!({
            "id": "msg_01abc",
            "type": "message",
            "role": "assistant",
            "model": "claude-sonnet-4-6",
            "content": [ { "type": "text", "text": "Hello! How can I help today?" } ],
            "stop_reason": "end_turn",
            "usage": { "input_tokens": 12, "output_tokens": 9 }
        });

        let response = AnthropicProvider::parse_response(raw).expect("parse ok");
        assert_eq!(
            response.content.as_deref(),
            Some("Hello! How can I help today?")
        );
        assert!(response.tool_calls.is_empty());
        assert_eq!(response.finish_reason, "end_turn");
        assert_eq!(response.usage.prompt_tokens, 12);
        assert_eq!(response.usage.completion_tokens, 9);
        assert_eq!(response.usage.total_tokens, 21, "total = input + output");
    }

    #[test]
    fn parse_response_with_tool_use_yields_tool_calls() {
        let raw = json!({
            "id": "msg_02xyz",
            "type": "message",
            "role": "assistant",
            "model": "claude-sonnet-4-6",
            "content": [
                { "type": "text", "text": "Let me look up the weather." },
                {
                    "type": "tool_use",
                    "id": "toolu_01abc",
                    "name": "get_weather",
                    "input": { "city": "San Francisco", "unit": "f" }
                }
            ],
            "stop_reason": "tool_use",
            "usage": { "input_tokens": 25, "output_tokens": 40 }
        });

        let response = AnthropicProvider::parse_response(raw).expect("parse ok");
        assert_eq!(
            response.content.as_deref(),
            Some("Let me look up the weather.")
        );
        assert_eq!(response.tool_calls.len(), 1);
        assert_eq!(response.tool_calls[0].id, "toolu_01abc");
        assert_eq!(response.tool_calls[0].name, "get_weather");
        assert_eq!(
            response.tool_calls[0].arguments,
            json!({ "city": "San Francisco", "unit": "f" })
        );
        assert_eq!(response.finish_reason, "tool_use");
    }

    fn provider_with_base_url(base_url: String) -> AnthropicProvider {
        AnthropicProvider::new(
            "test-key".to_string(),
            base_url,
            "claude-test".to_string(),
            0.7,
            Some(1024),
        )
    }

    #[tokio::test]
    async fn parses_success_response_over_http() {
        let body = r#"{
            "content": [ { "type": "text", "text": "hi" } ],
            "stop_reason": "end_turn",
            "usage": { "input_tokens": 3, "output_tokens": 2 }
        }"#;
        let base_url =
            single_response_server(200, &[("content-type", "application/json")], body).await;

        let response = provider_with_base_url(base_url)
            .chat(vec![Message::user("hi")], None)
            .await
            .expect("response parses");
        assert_eq!(response.content.as_deref(), Some("hi"));
        assert_eq!(response.usage.total_tokens, 5);
    }

    #[tokio::test]
    async fn classifies_401_as_invalid_api_key() {
        let base_url = single_response_server(
            401,
            &[("content-type", "application/json")],
            r#"{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}"#,
        )
        .await;

        let err = provider_with_base_url(base_url)
            .chat(vec![Message::user("hi")], None)
            .await
            .expect_err("expected provider error");
        assert_eq!(err, ProviderError::InvalidApiKey);
    }

    #[tokio::test]
    async fn classifies_429_as_rate_limited() {
        let base_url = single_response_server(
            429,
            &[("retry-after", "22"), ("content-type", "application/json")],
            r#"{"type":"error","error":{"type":"rate_limit_error","message":"rate limit exceeded"}}"#,
        )
        .await;

        let err = provider_with_base_url(base_url)
            .chat(vec![Message::user("hi")], None)
            .await
            .expect_err("expected provider error");
        assert_eq!(
            err,
            ProviderError::RateLimited {
                retry_after_seconds: Some(22)
            }
        );
    }

    #[tokio::test]
    async fn classifies_5xx_as_upstream() {
        let base_url = single_response_server(
            529,
            &[("content-type", "application/json")],
            r#"{"type":"error","error":{"type":"overloaded_error","message":"temporarily overloaded"}}"#,
        )
        .await;

        let err = provider_with_base_url(base_url)
            .chat(vec![Message::user("hi")], None)
            .await
            .expect_err("expected provider error");
        assert_eq!(
            err,
            ProviderError::Upstream {
                status: 529,
                message: "temporarily overloaded".to_string(),
            }
        );
    }
}
