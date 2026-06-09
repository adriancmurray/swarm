//! Gemini inference provider (Google Generative Language API).
//!
//! The API key travels in the `x-goog-api-key` header rather than the URL
//! query string, so it never lands in request logs or proxy access logs.

use super::types::{header_pairs, network_error, parse_response_error};
use super::{classify_http_error, LLMResponse, Message, Provider, ProviderError, ToolCall, Usage};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Gemini provider for Google's LLMs.
pub struct GeminiProvider {
    pub api_key: String,
    pub base_url: String,
    pub model: String,
    client: reqwest::Client,
}

impl GeminiProvider {
    pub fn new(api_key: String, model: String) -> Self {
        Self {
            api_key,
            base_url: "https://generativelanguage.googleapis.com".to_string(),
            model,
            client: reqwest::Client::new(),
        }
    }

    pub fn default_model(api_key: String) -> Self {
        Self::new(api_key, "gemini-2.5-flash".to_string())
    }
}

#[derive(Serialize)]
struct GeminiRequest {
    contents: Vec<GeminiContent>,
    #[serde(rename = "systemInstruction", skip_serializing_if = "Option::is_none")]
    system_instruction: Option<GeminiContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<GeminiTool>>,
}

#[derive(Serialize)]
struct GeminiContent {
    role: String,
    parts: Vec<GeminiPart>,
}

#[derive(Serialize)]
struct GeminiPart {
    text: String,
}

#[derive(Serialize)]
struct GeminiTool {
    function_declarations: Vec<serde_json::Value>,
}

#[derive(Deserialize)]
struct GeminiResponse {
    candidates: Vec<GeminiCandidate>,
    #[serde(default)]
    usage_metadata: Option<GeminiUsage>,
}

#[derive(Deserialize)]
struct GeminiCandidate {
    content: GeminiContentResponse,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct GeminiContentResponse {
    parts: Vec<GeminiPartResponse>,
}

#[derive(Deserialize)]
struct GeminiPartResponse {
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    function_call: Option<GeminiFunctionCall>,
}

#[derive(Deserialize)]
struct GeminiFunctionCall {
    name: String,
    args: serde_json::Value,
}

#[derive(Deserialize)]
struct GeminiUsage {
    #[serde(default)]
    prompt_token_count: u32,
    #[serde(default)]
    candidates_token_count: u32,
    #[serde(default)]
    total_token_count: u32,
}

#[async_trait]
impl Provider for GeminiProvider {
    async fn chat(
        &self,
        messages: Vec<Message>,
        tools: Option<Vec<serde_json::Value>>,
    ) -> Result<LLMResponse, ProviderError> {
        if self.api_key.is_empty() {
            return Err(ProviderError::NotConfigured("API key not set".to_string()));
        }

        let url = format!(
            "{}/v1beta/models/{}:generateContent",
            self.base_url.trim_end_matches('/'),
            self.model,
        );

        // System message becomes a dedicated systemInstruction.
        let system_message = messages.iter().find(|m| m.role == "system");
        let system_instruction = system_message.map(|m| GeminiContent {
            role: "system".to_string(),
            parts: vec![GeminiPart {
                text: m.content.clone().unwrap_or_default(),
            }],
        });

        // Gemini uses 'user' and 'model' roles.
        let contents: Vec<GeminiContent> = messages
            .into_iter()
            .filter(|m| m.role != "system")
            .map(|m| GeminiContent {
                role: if m.role == "assistant" {
                    "model".to_string()
                } else {
                    "user".to_string()
                },
                parts: vec![GeminiPart {
                    text: m.content.unwrap_or_default(),
                }],
            })
            .collect();

        let gemini_tools = tools.map(|ts| {
            vec![GeminiTool {
                function_declarations: ts
                    .into_iter()
                    .map(|t| t.get("function").cloned().unwrap_or(t))
                    .collect(),
            }]
        });

        let request = GeminiRequest {
            contents,
            system_instruction,
            tools: gemini_tools,
        };

        let res = self
            .client
            .post(&url)
            .header("x-goog-api-key", &self.api_key)
            .header("content-type", "application/json")
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

        let response: GeminiResponse = res.json().await.map_err(parse_response_error)?;

        let candidate = response
            .candidates
            .first()
            .ok_or_else(|| ProviderError::ParseResponse("No candidates in response".to_string()))?;

        let mut content_parts = Vec::new();
        let mut tool_calls = Vec::new();
        for (i, part) in candidate.content.parts.iter().enumerate() {
            if let Some(text) = &part.text {
                content_parts.push(text.clone());
            }
            if let Some(fc) = &part.function_call {
                tool_calls.push(ToolCall {
                    id: format!("call_{i}"),
                    name: fc.name.clone(),
                    arguments: fc.args.clone(),
                });
            }
        }

        let usage = response.usage_metadata.unwrap_or(GeminiUsage {
            prompt_token_count: 0,
            candidates_token_count: 0,
            total_token_count: 0,
        });

        Ok(LLMResponse {
            content: if content_parts.is_empty() {
                None
            } else {
                Some(content_parts.join(""))
            },
            reasoning_content: None,
            tool_calls,
            finish_reason: candidate
                .finish_reason
                .clone()
                .unwrap_or_else(|| "stop".to_string()),
            usage: Usage {
                prompt_tokens: usage.prompt_token_count,
                completion_tokens: usage.candidates_token_count,
                total_tokens: usage.total_token_count,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::mock::{request_capture_server, single_response_server};

    fn provider(base_url: String) -> GeminiProvider {
        let mut provider = GeminiProvider::new("test-key".to_string(), "gemini-test".to_string());
        provider.base_url = base_url;
        provider
    }

    #[tokio::test]
    async fn parses_text_and_function_call_response() {
        let body = r#"{
            "candidates": [{
                "content": { "parts": [
                    { "text": "Looking that up." },
                    { "function_call": { "name": "get_weather", "args": { "city": "SF" } } }
                ] },
                "finish_reason": "STOP"
            }],
            "usage_metadata": {
                "prompt_token_count": 5,
                "candidates_token_count": 6,
                "total_token_count": 11
            }
        }"#;
        let (base_url, capture) = request_capture_server(body).await;

        let response = provider(base_url)
            .chat(
                vec![Message::system("be brief"), Message::user("weather in SF?")],
                None,
            )
            .await
            .expect("response parses");

        assert_eq!(response.content.as_deref(), Some("Looking that up."));
        assert_eq!(response.tool_calls.len(), 1);
        assert_eq!(response.tool_calls[0].name, "get_weather");
        assert_eq!(response.tool_calls[0].arguments["city"], "SF");
        assert_eq!(response.usage.total_tokens, 11);

        // API key rides in the header, not the URL; system message lifted out.
        let captured = capture.recv().expect("captured request");
        assert_eq!(
            captured.header("x-goog-api-key").as_deref(),
            Some("test-key")
        );
        assert!(
            !captured.path.contains("key="),
            "key must not be in URL: {}",
            captured.path
        );
        let json: serde_json::Value = serde_json::from_str(&captured.body).expect("request json");
        assert_eq!(json["systemInstruction"]["parts"][0]["text"], "be brief");
        assert_eq!(json["contents"][0]["role"], "user");
    }

    #[tokio::test]
    async fn classifies_401_as_invalid_api_key() {
        let base_url = single_response_server(
            401,
            &[("content-type", "application/json")],
            r#"{"error":{"code":401,"message":"API key not valid","status":"UNAUTHENTICATED"}}"#,
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
            &[("retry-after", "12"), ("content-type", "application/json")],
            r#"{"error":{"code":429,"message":"Quota exceeded","status":"RESOURCE_EXHAUSTED"}}"#,
        )
        .await;

        let err = provider(base_url)
            .chat(vec![Message::user("hi")], None)
            .await
            .expect_err("expected provider error");
        assert_eq!(
            err,
            ProviderError::RateLimited {
                retry_after_seconds: Some(12)
            }
        );
    }

    #[tokio::test]
    async fn classifies_5xx_as_upstream() {
        let base_url = single_response_server(
            500,
            &[("content-type", "application/json")],
            r#"{"error":{"code":500,"message":"Internal error","status":"INTERNAL"}}"#,
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
                message: "Internal error".to_string(),
            }
        );
    }
}
