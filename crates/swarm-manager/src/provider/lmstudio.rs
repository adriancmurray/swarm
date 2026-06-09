//! LM Studio inference provider (local, OpenAI-compatible API).
//!
//! Also serves the MLX local server, which exposes the same
//! OpenAI-compatible `/v1/chat/completions` shape.

use super::types::{header_pairs, network_error, parse_response_error};
use super::{classify_http_error, LLMResponse, Message, Provider, ProviderError, ToolCall, Usage};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;

/// LM Studio provider (OpenAI-compatible local API).
pub struct LmStudioProvider {
    pub base_url: String,
    pub model: Option<String>,
    client: reqwest::Client,
}

impl LmStudioProvider {
    pub fn new(base_url: String, model: Option<String>) -> Self {
        Self {
            base_url,
            model,
            client: reqwest::Client::new(),
        }
    }

    pub fn default_local() -> Self {
        Self::new("http://localhost:1234".to_string(), None)
    }
}

#[derive(Serialize)]
struct OpenAIRequest<'a> {
    model: &'a str,
    messages: Vec<OpenAIMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<serde_json::Value>>,
    stream: bool,
}

#[derive(Serialize)]
struct OpenAIMessage {
    role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
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
    tool_calls: Option<Vec<OpenAIToolCall>>,
}

#[async_trait]
impl Provider for LmStudioProvider {
    async fn chat(
        &self,
        messages: Vec<Message>,
        tools: Option<Vec<serde_json::Value>>,
    ) -> Result<LLMResponse, ProviderError> {
        let url = format!(
            "{}/v1/chat/completions",
            self.base_url.trim_end_matches('/')
        );

        let openai_messages: Vec<OpenAIMessage> = messages
            .into_iter()
            .map(|m| OpenAIMessage {
                role: m.role,
                content: m.content,
                tool_call_id: m.tool_call_id,
                tool_calls: m.tool_calls.map(|tcs| {
                    tcs.into_iter()
                        .map(|tc| OpenAIToolCall {
                            id: tc.id,
                            r#type: "function".to_string(),
                            function: OpenAIFunctionCall {
                                name: tc.name,
                                arguments: tc.arguments.to_string(),
                            },
                        })
                        .collect()
                }),
            })
            .collect();

        let model = self.model.as_deref().unwrap_or("local-model");

        let request = OpenAIRequest {
            model,
            messages: openai_messages,
            tools,
            stream: false,
        };

        let res = self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await
            .map_err(network_error)?;

        if !res.status().is_success() {
            let status = res.status().as_u16();
            let headers = header_pairs(res.headers());
            let text = res.text().await.map_err(network_error)?;
            return Err(classify_http_error(status, &headers, &text, model));
        }

        let response: OpenAIResponse = res.json().await.map_err(parse_response_error)?;

        let choice = response
            .choices
            .first()
            .ok_or_else(|| ProviderError::ParseResponse("No choices in response".to_string()))?;

        let tool_calls = if let Some(tcs) = &choice.message.tool_calls {
            tcs.iter()
                .map(|tc| {
                    let args_json: serde_json::Value =
                        serde_json::from_str(&tc.function.arguments).unwrap_or_else(|_| json!({}));
                    ToolCall {
                        id: tc.id.clone(),
                        name: tc.function.name.clone(),
                        arguments: args_json,
                    }
                })
                .collect()
        } else {
            Vec::new()
        };

        Ok(LLMResponse {
            content: choice.message.content.clone(),
            reasoning_content: None,
            tool_calls,
            finish_reason: choice
                .finish_reason
                .clone()
                .unwrap_or_else(|| "stop".to_string()),
            usage: response.usage.unwrap_or_default(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::mock::{request_capture_server, single_response_server};

    fn provider(base_url: String) -> LmStudioProvider {
        LmStudioProvider::new(base_url, Some("local-test".to_string()))
    }

    #[tokio::test]
    async fn request_shape_and_response_parse_with_tool_calls() {
        let body = r#"{
            "choices": [{
                "message": {
                    "content": "calling",
                    "tool_calls": [{
                        "id": "call_9",
                        "type": "function",
                        "function": { "name": "list_dir", "arguments": "{\"path\":\".\"}" }
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": { "prompt_tokens": 4, "completion_tokens": 3, "total_tokens": 7 }
        }"#;
        let (base_url, capture) = request_capture_server(body).await;

        let response = provider(base_url)
            .chat(vec![Message::user("list .")], None)
            .await
            .expect("response parses");

        assert_eq!(response.content.as_deref(), Some("calling"));
        assert_eq!(response.tool_calls.len(), 1);
        assert_eq!(response.tool_calls[0].id, "call_9");
        assert_eq!(response.tool_calls[0].name, "list_dir");
        assert_eq!(response.tool_calls[0].arguments["path"], ".");
        assert_eq!(response.usage.total_tokens, 7);

        let captured = capture.recv().expect("captured request");
        assert!(
            captured.path.ends_with("/v1/chat/completions"),
            "{}",
            captured.path
        );
        let json: serde_json::Value = serde_json::from_str(&captured.body).expect("request json");
        assert_eq!(json["model"], "local-test");
        assert_eq!(json["stream"], false);
        assert_eq!(json["messages"][0]["content"], "list .");
    }

    #[tokio::test]
    async fn defaults_model_when_unset() {
        let body = r#"{
            "choices": [{ "message": { "content": "ok" }, "finish_reason": "stop" }]
        }"#;
        let (base_url, capture) = request_capture_server(body).await;

        LmStudioProvider::new(base_url, None)
            .chat(vec![Message::user("hi")], None)
            .await
            .expect("response parses");

        let captured = capture.recv().expect("captured request");
        let json: serde_json::Value = serde_json::from_str(&captured.body).expect("request json");
        assert_eq!(json["model"], "local-model");
    }

    #[tokio::test]
    async fn classifies_401_as_invalid_api_key() {
        let base_url = single_response_server(
            401,
            &[("content-type", "application/json")],
            r#"{"error":{"message":"unauthorized"}}"#,
        )
        .await;

        let err = provider(base_url)
            .chat(vec![Message::user("hi")], None)
            .await
            .expect_err("expected provider error");
        assert_eq!(err, ProviderError::InvalidApiKey);
    }

    #[tokio::test]
    async fn classifies_429_as_rate_limited() {
        let base_url = single_response_server(
            429,
            &[("retry-after", "4"), ("content-type", "application/json")],
            r#"{"error":{"message":"too many requests"}}"#,
        )
        .await;

        let err = provider(base_url)
            .chat(vec![Message::user("hi")], None)
            .await
            .expect_err("expected provider error");
        assert_eq!(
            err,
            ProviderError::RateLimited {
                retry_after_seconds: Some(4)
            }
        );
    }

    #[tokio::test]
    async fn classifies_5xx_as_upstream() {
        let base_url = single_response_server(
            502,
            &[("content-type", "application/json")],
            r#"{"error":{"message":"local model crashed"}}"#,
        )
        .await;

        let err = provider(base_url)
            .chat(vec![Message::user("hi")], None)
            .await
            .expect_err("expected provider error");
        assert_eq!(
            err,
            ProviderError::Upstream {
                status: 502,
                message: "local model crashed".to_string(),
            }
        );
    }
}
