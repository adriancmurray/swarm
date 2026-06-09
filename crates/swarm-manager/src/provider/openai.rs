//! OpenAI-compatible chat provider.
//!
//! Speaks the OpenAI `/chat/completions` wire format, which is also spoken
//! by OpenRouter and DeepSeek (the latter with a couple of extra reasoning
//! fields). Tool names are sanitized to the `[A-Za-z0-9_-]` charset the API
//! accepts and mapped back on the response.

use super::types::{header_pairs, network_error, parse_response_error};
use super::{classify_http_error, LLMResponse, Message, Provider, ProviderError, ToolCall, Usage};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{HashMap, HashSet};

const DEEPSEEK_DEFAULT_REASONING_EFFORT: &str = "high";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpenAIProviderProfile {
    Generic,
    DeepSeek,
}

/// OpenAI-compatible provider (also serves OpenRouter and DeepSeek).
pub struct OpenAIProvider {
    pub api_key: String,
    pub base_url: String,
    pub model: String,
    pub temperature: f32,
    pub max_tokens: Option<usize>,
    pub client: reqwest::Client,
    profile: OpenAIProviderProfile,
}

impl OpenAIProvider {
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
            profile: OpenAIProviderProfile::Generic,
        }
    }

    /// DeepSeek profile: enables the reasoning-content + thinking fields.
    pub fn deepseek(
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
            profile: OpenAIProviderProfile::DeepSeek,
        }
    }

    #[cfg(test)]
    fn build_request<'a>(
        &'a self,
        messages: Vec<Message>,
        tools: Option<Vec<serde_json::Value>>,
    ) -> OpenAIChatRequest<'a> {
        let (tools, tool_name_codec) = encode_tool_names(tools);
        self.build_request_with_codec(messages, tools, &tool_name_codec)
    }

    fn build_request_with_codec<'a>(
        &'a self,
        messages: Vec<Message>,
        tools: Option<Vec<serde_json::Value>>,
        tool_name_codec: &ToolNameCodec,
    ) -> OpenAIChatRequest<'a> {
        let include_reasoning_content = self.profile == OpenAIProviderProfile::DeepSeek;
        let api_messages: Vec<OpenAIMessage> = messages
            .into_iter()
            .map(|m| OpenAIMessage {
                role: m.role,
                content: m.content,
                reasoning_content: if include_reasoning_content {
                    m.reasoning_content
                } else {
                    None
                },
                tool_call_id: m.tool_call_id,
                tool_calls: m.tool_calls.map(|tcs| {
                    tcs.into_iter()
                        .map(|tc| OpenAIToolCall {
                            id: tc.id,
                            r#type: "function".to_string(),
                            function: OpenAIFunctionCall {
                                name: tool_name_codec.encode(&tc.name),
                                arguments: tc.arguments.to_string(),
                            },
                        })
                        .collect()
                }),
            })
            .collect();

        let deepseek_thinking = self.deepseek_thinking();
        OpenAIChatRequest {
            model: &self.model,
            messages: api_messages,
            tools,
            temperature: self.request_temperature(deepseek_thinking.as_ref()),
            max_tokens: self.max_tokens,
            thinking: deepseek_thinking,
            reasoning_effort: self.deepseek_reasoning_effort(),
        }
    }

    fn request_temperature(&self, thinking: Option<&DeepSeekThinking>) -> Option<f32> {
        if self.profile == OpenAIProviderProfile::DeepSeek
            && matches!(
                thinking.map(|t| t.r#type),
                Some(DeepSeekThinkingType::Enabled)
            )
        {
            return None;
        }
        Some(self.temperature)
    }

    fn deepseek_thinking(&self) -> Option<DeepSeekThinking> {
        if self.profile != OpenAIProviderProfile::DeepSeek {
            return None;
        }
        let r#type = if self.model == "deepseek-chat" {
            DeepSeekThinkingType::Disabled
        } else {
            DeepSeekThinkingType::Enabled
        };
        Some(DeepSeekThinking { r#type })
    }

    fn deepseek_reasoning_effort(&self) -> Option<&'static str> {
        if self.profile != OpenAIProviderProfile::DeepSeek || self.model == "deepseek-chat" {
            return None;
        }
        Some(DEEPSEEK_DEFAULT_REASONING_EFFORT)
    }
}

#[derive(Default)]
struct ToolNameCodec {
    original_to_api: HashMap<String, String>,
    api_to_original: HashMap<String, String>,
}

impl ToolNameCodec {
    fn encode(&self, original: &str) -> String {
        self.original_to_api
            .get(original)
            .cloned()
            .unwrap_or_else(|| sanitize_tool_name(original, &mut HashSet::new()))
    }

    fn decode(&self, api_name: &str) -> String {
        self.api_to_original
            .get(api_name)
            .cloned()
            .unwrap_or_else(|| api_name.to_string())
    }
}

fn encode_tool_names(
    tools: Option<Vec<serde_json::Value>>,
) -> (Option<Vec<serde_json::Value>>, ToolNameCodec) {
    let Some(mut tools) = tools else {
        return (None, ToolNameCodec::default());
    };

    let mut used = HashSet::new();
    let mut codec = ToolNameCodec::default();
    for tool in &mut tools {
        let Some(function) = tool.get_mut("function").and_then(|v| v.as_object_mut()) else {
            continue;
        };
        let Some(original) = function
            .get("name")
            .and_then(|v| v.as_str())
            .map(str::to_string)
        else {
            continue;
        };
        let api_name = sanitize_tool_name(&original, &mut used);
        if api_name != original {
            function.insert(
                "description".to_string(),
                serde_json::Value::String(
                    match function.get("description").and_then(|v| v.as_str()) {
                        Some(description) if !description.is_empty() => {
                            format!("{description}\n\nInternal tool name: {original}")
                        }
                        _ => format!("Internal tool name: {original}"),
                    },
                ),
            );
        }
        function.insert(
            "name".to_string(),
            serde_json::Value::String(api_name.clone()),
        );
        codec
            .original_to_api
            .insert(original.clone(), api_name.clone());
        codec.api_to_original.insert(api_name, original);
    }

    (Some(tools), codec)
}

fn sanitize_tool_name(original: &str, used: &mut HashSet<String>) -> String {
    let mut sanitized: String = original
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if sanitized.is_empty() || !sanitized.chars().next().unwrap().is_ascii_alphanumeric() {
        sanitized = format!("tool_{sanitized}");
    }

    let base = sanitized.clone();
    let mut suffix = 2;
    while !used.insert(sanitized.clone()) {
        sanitized = format!("{base}_{suffix}");
        suffix += 1;
    }
    sanitized
}

#[derive(Serialize)]
struct OpenAIChatRequest<'a> {
    model: &'a str,
    messages: Vec<OpenAIMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<DeepSeekThinking>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_effort: Option<&'a str>,
}

#[derive(Clone, Copy, Serialize)]
struct DeepSeekThinking {
    #[serde(rename = "type")]
    r#type: DeepSeekThinkingType,
}

#[derive(Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum DeepSeekThinkingType {
    Enabled,
    Disabled,
}

#[derive(Serialize)]
struct OpenAIMessage {
    role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<OpenAIToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct OpenAIToolCall {
    id: String,
    #[serde(rename = "type")]
    r#type: String,
    function: OpenAIFunctionCall,
}

#[derive(Serialize, Deserialize)]
struct OpenAIFunctionCall {
    name: String,
    arguments: String,
}

#[derive(Deserialize)]
struct OpenAIResponse {
    choices: Vec<OpenAIChoice>,
    usage: Option<Usage>,
}

#[derive(Deserialize)]
struct OpenAIChoice {
    message: OpenAIMessageResponse,
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct OpenAIMessageResponse {
    content: Option<String>,
    #[serde(default)]
    reasoning_content: Option<String>,
    tool_calls: Option<Vec<OpenAIToolCall>>,
}

#[async_trait]
impl Provider for OpenAIProvider {
    async fn chat(
        &self,
        messages: Vec<Message>,
        tools: Option<Vec<serde_json::Value>>,
    ) -> Result<LLMResponse, ProviderError> {
        if self.api_key.is_empty() {
            return Err(ProviderError::NotConfigured("API key not set".to_string()));
        }

        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));

        let (tools, tool_name_codec) = encode_tool_names(tools);
        let request = self.build_request_with_codec(messages, tools, &tool_name_codec);

        let res = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await
            .map_err(network_error)?;

        if !res.status().is_success() {
            let status = res.status().as_u16();
            let headers = header_pairs(res.headers());
            let text = res.text().await.map_err(network_error)?;
            return Err(classify_http_error(status, &headers, &text, &self.model));
        }

        let response: OpenAIResponse = res.json().await.map_err(parse_response_error)?;

        let choice = response
            .choices
            .first()
            .ok_or_else(|| ProviderError::ParseResponse("No choices in response".to_string()))?;

        let tool_calls = if let Some(tcs) = &choice.message.tool_calls {
            let mut converted = Vec::new();
            for tc in tcs {
                let args_json: serde_json::Value =
                    serde_json::from_str(&tc.function.arguments).unwrap_or_else(|_| json!({}));
                converted.push(ToolCall {
                    id: tc.id.clone(),
                    name: tool_name_codec.decode(&tc.function.name),
                    arguments: args_json,
                });
            }
            converted
        } else {
            Vec::new()
        };

        Ok(LLMResponse {
            content: choice.message.content.clone(),
            reasoning_content: choice.message.reasoning_content.clone(),
            tool_calls,
            finish_reason: choice
                .finish_reason
                .clone()
                .unwrap_or_else(|| "unknown".to_string()),
            usage: response.usage.unwrap_or_default(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::mock::{request_capture_server, single_response_server};

    fn provider(base_url: String) -> OpenAIProvider {
        OpenAIProvider::new(
            "test-key".to_string(),
            base_url,
            "gpt-test".to_string(),
            0.7,
            Some(128),
        )
    }

    fn deepseek_provider(base_url: String, model: &str) -> OpenAIProvider {
        OpenAIProvider::deepseek(
            "test-key".to_string(),
            base_url,
            model.to_string(),
            0.4,
            Some(256),
        )
    }

    #[test]
    fn generic_request_does_not_emit_deepseek_fields() {
        let provider = provider("http://example.test".to_string());
        let request = provider.build_request(
            vec![Message {
                role: "assistant".to_string(),
                content: Some("hello".to_string()),
                reasoning_content: Some("private chain".to_string()),
                tool_calls: None,
                tool_call_id: None,
            }],
            None,
        );

        let body = serde_json::to_value(request).expect("request serializes");
        assert!((body["temperature"].as_f64().unwrap() - 0.7).abs() < 0.00001);
        assert!(body.get("thinking").is_none());
        assert!(body.get("reasoning_effort").is_none());
        assert!(body["messages"][0].get("reasoning_content").is_none());
    }

    #[test]
    fn deepseek_legacy_chat_alias_uses_non_thinking_mode() {
        let provider = deepseek_provider("http://example.test".to_string(), "deepseek-chat");
        let request = provider.build_request(vec![Message::user("hi")], None);

        let body = serde_json::to_value(request).expect("request serializes");
        assert_eq!(body["thinking"]["type"], "disabled");
        assert!((body["temperature"].as_f64().unwrap() - 0.4).abs() < 0.00001);
        assert!(body.get("reasoning_effort").is_none());
    }

    #[tokio::test]
    async fn request_shape_has_auth_header_model_and_messages() {
        let response_body = r#"{
            "choices": [{
                "message": { "content": "ok", "tool_calls": null },
                "finish_reason": "stop"
            }],
            "usage": { "prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2 }
        }"#;
        let (base_url, capture) = request_capture_server(response_body).await;

        let response = provider(base_url)
            .chat(vec![Message::user("hi there")], None)
            .await
            .expect("response parses");
        assert_eq!(response.content.as_deref(), Some("ok"));

        let captured = capture.recv().expect("captured request");
        assert_eq!(captured.method, "POST");
        assert!(
            captured.path.ends_with("/chat/completions"),
            "{}",
            captured.path
        );
        assert_eq!(
            captured.header("authorization").as_deref(),
            Some("Bearer test-key")
        );
        let json: serde_json::Value =
            serde_json::from_str(&captured.body).expect("request body json");
        assert_eq!(json["model"], "gpt-test");
        assert_eq!(json["messages"][0]["role"], "user");
        assert_eq!(json["messages"][0]["content"], "hi there");
    }

    #[tokio::test]
    async fn deepseek_request_and_response_preserve_reasoning_tool_calls() {
        let response_body = r#"{
            "choices": [{
                "message": {
                    "content": "I'll inspect that file.",
                    "reasoning_content": "I need to read the requested path before answering.",
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": {
                            "name": "read_file",
                            "arguments": "{\"path\":\"Cargo.toml\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": { "prompt_tokens": 11, "completion_tokens": 7, "total_tokens": 18 }
        }"#;
        let (base_url, capture) = request_capture_server(response_body).await;

        let mut assistant_with_reasoning = Message::assistant("calling tool");
        assistant_with_reasoning.reasoning_content = Some("prior reasoning".to_string());
        assistant_with_reasoning.tool_calls = Some(vec![ToolCall {
            id: "call_prev".to_string(),
            name: "read_file".to_string(),
            arguments: json!({"path": "README.md"}),
        }]);

        let response = deepseek_provider(base_url, "deepseek-v4-flash")
            .chat(vec![Message::user("hi"), assistant_with_reasoning], None)
            .await
            .expect("deepseek response parses");

        assert_eq!(
            response.reasoning_content.as_deref(),
            Some("I need to read the requested path before answering.")
        );
        assert_eq!(response.tool_calls.len(), 1);
        assert_eq!(response.tool_calls[0].name, "read_file");
        assert_eq!(response.tool_calls[0].arguments["path"], "Cargo.toml");
        assert_eq!(response.usage.total_tokens, 18);

        let captured = capture.recv().expect("captured request");
        let json: serde_json::Value =
            serde_json::from_str(&captured.body).expect("request body json");
        assert_eq!(json["model"], "deepseek-v4-flash");
        assert_eq!(json["thinking"]["type"], "enabled");
        assert_eq!(json["reasoning_effort"], DEEPSEEK_DEFAULT_REASONING_EFFORT);
        assert!(json.get("temperature").is_none());
        assert_eq!(json["messages"][1]["reasoning_content"], "prior reasoning");
    }

    #[tokio::test]
    async fn sanitizes_dotted_tool_names_and_maps_calls_back() {
        let response_body = r#"{
            "choices": [{
                "message": {
                    "content": null,
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": {
                            "name": "memory_search",
                            "arguments": "{\"query\":\"task-12\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": { "prompt_tokens": 12, "completion_tokens": 8, "total_tokens": 20 }
        }"#;
        let (base_url, capture) = request_capture_server(response_body).await;

        let tools = vec![json!({
            "type": "function",
            "function": {
                "name": "memory.search",
                "description": "Search stored memory",
                "parameters": {
                    "type": "object",
                    "properties": { "query": {"type": "string"} }
                }
            }
        })];
        let response = provider(base_url)
            .chat(vec![Message::user("recall task-12")], Some(tools))
            .await
            .expect("response parses");

        assert_eq!(response.tool_calls.len(), 1);
        assert_eq!(response.tool_calls[0].name, "memory.search");
        assert_eq!(response.tool_calls[0].arguments["query"], "task-12");

        let captured = capture.recv().expect("captured request");
        let json: serde_json::Value =
            serde_json::from_str(&captured.body).expect("request body json");
        assert_eq!(json["tools"][0]["function"]["name"], "memory_search");
        assert!(json["tools"][0]["function"]["description"]
            .as_str()
            .unwrap_or_default()
            .contains("Internal tool name: memory.search"));
    }

    #[tokio::test]
    async fn classifies_401_as_invalid_api_key() {
        let base_url = single_response_server(
            401,
            &[("content-type", "application/json")],
            r#"{"error":{"message":"Incorrect API key provided","type":"invalid_request_error"}}"#,
        )
        .await;

        let err = provider(base_url)
            .chat(vec![Message::user("hi")], None)
            .await
            .expect_err("expected provider error");
        assert_eq!(err, ProviderError::InvalidApiKey);
    }

    #[tokio::test]
    async fn classifies_429_as_rate_limited_with_retry_after() {
        let base_url = single_response_server(
            429,
            &[("retry-after", "30"), ("content-type", "application/json")],
            r#"{"error":{"message":"Rate limit reached"}}"#,
        )
        .await;

        let err = provider(base_url)
            .chat(vec![Message::user("hi")], None)
            .await
            .expect_err("expected provider error");
        assert_eq!(
            err,
            ProviderError::RateLimited {
                retry_after_seconds: Some(30)
            }
        );
    }

    #[tokio::test]
    async fn classifies_5xx_as_upstream() {
        let base_url = single_response_server(
            503,
            &[("content-type", "application/json")],
            r#"{"error":{"message":"The engine is currently overloaded"}}"#,
        )
        .await;

        let err = provider(base_url)
            .chat(vec![Message::user("hi")], None)
            .await
            .expect_err("expected provider error");
        assert_eq!(
            err,
            ProviderError::Upstream {
                status: 503,
                message: "The engine is currently overloaded".to_string(),
            }
        );
    }
}
