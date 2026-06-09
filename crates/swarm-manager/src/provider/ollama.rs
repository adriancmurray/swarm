//! Ollama inference provider (local).
//!
//! Uses Ollama's native `/api/chat` endpoint. No API key — Ollama is a local
//! server.

use super::types::{header_pairs, network_error, parse_response_error};
use super::{classify_http_error, LLMResponse, Message, Provider, ProviderError, ToolCall, Usage};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Ollama provider for local LLM inference.
pub struct OllamaProvider {
    pub base_url: String,
    pub model: String,
    client: reqwest::Client,
}

impl OllamaProvider {
    pub fn new(base_url: String, model: String) -> Self {
        Self {
            base_url,
            model,
            client: reqwest::Client::new(),
        }
    }

    pub fn default_local() -> Self {
        Self::new("http://localhost:11434".to_string(), "llama3.2".to_string())
    }
}

#[derive(Serialize)]
struct OllamaRequest {
    model: String,
    messages: Vec<OllamaMessage>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<serde_json::Value>>,
}

#[derive(Serialize)]
struct OllamaMessage {
    role: String,
    content: String,
}

#[derive(Deserialize)]
struct OllamaResponse {
    message: OllamaMessageResponse,
    #[serde(default)]
    prompt_eval_count: Option<u32>,
    #[serde(default)]
    eval_count: Option<u32>,
}

#[derive(Deserialize)]
struct OllamaMessageResponse {
    content: String,
    #[serde(default)]
    tool_calls: Option<Vec<OllamaToolCall>>,
}

#[derive(Deserialize)]
struct OllamaToolCall {
    function: OllamaFunction,
}

#[derive(Deserialize)]
struct OllamaFunction {
    name: String,
    arguments: serde_json::Value,
}

#[async_trait]
impl Provider for OllamaProvider {
    async fn chat(
        &self,
        messages: Vec<Message>,
        tools: Option<Vec<serde_json::Value>>,
    ) -> Result<LLMResponse, ProviderError> {
        let url = format!("{}/api/chat", self.base_url);

        let ollama_messages: Vec<OllamaMessage> = messages
            .into_iter()
            .map(|m| OllamaMessage {
                role: m.role,
                content: m.content.unwrap_or_default(),
            })
            .collect();

        let request = OllamaRequest {
            model: self.model.clone(),
            messages: ollama_messages,
            stream: false,
            tools,
        };

        let res = self
            .client
            .post(&url)
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

        let response: OllamaResponse = res.json().await.map_err(parse_response_error)?;

        let tool_calls = response
            .message
            .tool_calls
            .map(|tcs| {
                tcs.into_iter()
                    .enumerate()
                    .map(|(i, tc)| ToolCall {
                        id: format!("call_{i}"),
                        name: tc.function.name,
                        arguments: tc.function.arguments,
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(LLMResponse {
            content: Some(response.message.content),
            reasoning_content: None,
            tool_calls,
            finish_reason: "stop".to_string(),
            usage: Usage {
                prompt_tokens: response.prompt_eval_count.unwrap_or(0),
                completion_tokens: response.eval_count.unwrap_or(0),
                total_tokens: response.prompt_eval_count.unwrap_or(0)
                    + response.eval_count.unwrap_or(0),
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::mock::{request_capture_server, single_response_server};

    fn provider(base_url: String) -> OllamaProvider {
        OllamaProvider::new(base_url, "llama-test".to_string())
    }

    #[tokio::test]
    async fn request_shape_and_response_parse_with_tool_calls() {
        let body = r#"{
            "message": {
                "content": "Reading now.",
                "tool_calls": [
                    { "function": { "name": "read_file", "arguments": { "path": "a.txt" } } }
                ]
            },
            "prompt_eval_count": 8,
            "eval_count": 4
        }"#;
        let (base_url, capture) = request_capture_server(body).await;

        let response = provider(base_url)
            .chat(vec![Message::user("read a.txt")], None)
            .await
            .expect("response parses");

        assert_eq!(response.content.as_deref(), Some("Reading now."));
        assert_eq!(response.tool_calls.len(), 1);
        assert_eq!(response.tool_calls[0].name, "read_file");
        assert_eq!(response.tool_calls[0].arguments["path"], "a.txt");
        assert_eq!(response.usage.total_tokens, 12);

        let captured = capture.recv().expect("captured request");
        assert_eq!(captured.method, "POST");
        assert!(captured.path.ends_with("/api/chat"), "{}", captured.path);
        let json: serde_json::Value = serde_json::from_str(&captured.body).expect("request json");
        assert_eq!(json["model"], "llama-test");
        assert_eq!(json["stream"], false);
        assert_eq!(json["messages"][0]["content"], "read a.txt");
    }

    #[tokio::test]
    async fn classifies_401_as_invalid_api_key() {
        let base_url = single_response_server(
            401,
            &[("content-type", "application/json")],
            r#"{"error":"unauthorized"}"#,
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
            &[("retry-after", "9"), ("content-type", "application/json")],
            r#"{"error":"slow down"}"#,
        )
        .await;

        let err = provider(base_url)
            .chat(vec![Message::user("hi")], None)
            .await
            .expect_err("expected provider error");
        assert_eq!(
            err,
            ProviderError::RateLimited {
                retry_after_seconds: Some(9)
            }
        );
    }

    #[tokio::test]
    async fn classifies_5xx_as_upstream() {
        let base_url = single_response_server(
            500,
            &[("content-type", "application/json")],
            r#"{"error":"backend failed"}"#,
        )
        .await;

        let err = provider(base_url)
            .chat(vec![Message::user("hi")], None)
            .await
            .expect_err("expected provider error");
        assert_eq!(
            err,
            ProviderError::Upstream {
                status: 500,
                message: r#"{"error":"backend failed"}"#.to_string(),
            }
        );
    }
}
