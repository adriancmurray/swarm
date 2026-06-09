//! Provider value types and HTTP error classification.
//!
//! Dependency-light: `classify_http_error` operates on a plain status
//! code plus header/body strings so this module pulls in no HTTP client.

use serde::{Deserialize, Serialize};

/// Typed provider failures.
#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
pub enum ProviderError {
    #[error("provider not configured: {0}")]
    NotConfigured(String),
    #[error("provider not implemented: {0}")]
    NotImplemented(String),
    #[error("invalid api key")]
    InvalidApiKey,
    #[error("model '{model}' not found on provider")]
    ModelNotFound { model: String },
    #[error("rate limited (retry after {retry_after_seconds:?}s)")]
    RateLimited { retry_after_seconds: Option<u64> },
    #[error("network: {0}")]
    Network(String),
    #[error("upstream {status}: {message}")]
    Upstream { status: u16, message: String },
    #[error("response parse: {0}")]
    ParseResponse(String),
}

/// Classify an HTTP error response into a typed `ProviderError`.
///
/// Pure status/header/body classification — no HTTP-client dependency.
/// `headers` is a slice of `(name, value)` pairs; header-name matching is
/// case-insensitive. `is_server_error` mirrors the 5xx range.
pub fn classify_http_error(
    status: u16,
    headers: &[(String, String)],
    body: &str,
    model: &str,
) -> ProviderError {
    let message = extract_error_message(body);

    match status {
        401 | 403 => ProviderError::InvalidApiKey,
        404 if is_model_not_found(body, &message) => ProviderError::ModelNotFound {
            model: model.to_string(),
        },
        429 => ProviderError::RateLimited {
            retry_after_seconds: retry_after_seconds(headers),
        },
        code => ProviderError::Upstream {
            status: code,
            message,
        },
    }
}

/// Map a transport-layer HTTP-client error into a `ProviderError`.
///
/// Only compiled with the `http` feature (the sole caller is the concrete
/// provider implementations). The error text never contains credentials.
#[cfg(feature = "http")]
pub(crate) fn network_error(err: reqwest::Error) -> ProviderError {
    ProviderError::Network(err.to_string())
}

/// Map a response-decoding error into a `ProviderError::ParseResponse`.
#[cfg(feature = "http")]
pub(crate) fn parse_response_error(err: impl std::fmt::Display) -> ProviderError {
    ProviderError::ParseResponse(err.to_string())
}

/// Fold reqwest's `HeaderMap` into the `(name, value)` pairs that
/// `classify_http_error` expects. Non-UTF-8 header values are dropped.
#[cfg(feature = "http")]
pub(crate) fn header_pairs(headers: &reqwest::header::HeaderMap) -> Vec<(String, String)> {
    headers
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|v| (name.as_str().to_string(), v.to_string()))
        })
        .collect()
}

fn retry_after_seconds(headers: &[(String, String)]) -> Option<u64> {
    headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("retry-after"))
        .and_then(|(_, value)| value.trim().parse::<u64>().ok())
}

fn extract_error_message(body: &str) -> String {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(body) else {
        return fallback_error_message(body);
    };

    value
        .get("error")
        .and_then(|error| error.get("message"))
        .or_else(|| value.get("message"))
        .and_then(serde_json::Value::as_str)
        .map(ToString::to_string)
        .unwrap_or_else(|| fallback_error_message(body))
}

fn fallback_error_message(body: &str) -> String {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        "empty error response".to_string()
    } else {
        trimmed.to_string()
    }
}

fn is_model_not_found(body: &str, message: &str) -> bool {
    let body = body.to_ascii_lowercase();
    let message = message.to_ascii_lowercase();
    let combined = format!("{body}\n{message}");
    combined.contains("model")
        && (combined.contains("not_found")
            || combined.contains("not found")
            || combined.contains("does not exist")
            || combined.contains("not exist"))
}

/// A chat message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl Message {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".to_string(),
            content: Some(content.into()),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: "assistant".to_string(),
            content: Some(content.into()),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
        }
    }

    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system".to_string(),
            content: Some(content.into()),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
        }
    }

    pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: "tool".to_string(),
            content: Some(content.into()),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: Some(tool_call_id.into()),
        }
    }
}

/// A tool call (simplified flat format).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

impl ToolCall {
    /// Get arguments as an owned `Value`.
    pub fn get_arguments(&self) -> serde_json::Value {
        self.arguments.clone()
    }
}

/// Response from the LLM.
#[derive(Debug, Clone)]
pub struct LLMResponse {
    pub content: Option<String>,
    pub reasoning_content: Option<String>,
    pub tool_calls: Vec<ToolCall>,
    pub finish_reason: String,
    pub usage: Usage,
}

/// Token usage information.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub prompt_tokens: u32,
    #[serde(default)]
    pub completion_tokens: u32,
    #[serde(default)]
    pub total_tokens: u32,
}

/// Tool definition for the LLM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    #[serde(rename = "type")]
    pub tool_type: String,
    pub function: FunctionDefinition,
}

/// Function definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

impl ToolDefinition {
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: serde_json::Value,
    ) -> Self {
        Self {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: name.into(),
                description: description.into(),
                parameters,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hdrs(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(n, v)| ((*n).to_string(), (*v).to_string()))
            .collect()
    }

    #[test]
    fn classify_401_403_is_invalid_key() {
        assert_eq!(
            classify_http_error(401, &[], "", "m"),
            ProviderError::InvalidApiKey
        );
        assert_eq!(
            classify_http_error(403, &[], "", "m"),
            ProviderError::InvalidApiKey
        );
    }

    #[test]
    fn classify_404_model_not_found() {
        let body = r#"{"error":{"message":"The model gpt-x does not exist"}}"#;
        assert_eq!(
            classify_http_error(404, &[], body, "gpt-x"),
            ProviderError::ModelNotFound {
                model: "gpt-x".to_string()
            }
        );
    }

    #[test]
    fn classify_404_without_model_phrase_is_upstream() {
        let err = classify_http_error(404, &[], "not here", "gpt-x");
        assert!(matches!(err, ProviderError::Upstream { status: 404, .. }));
    }

    #[test]
    fn classify_429_reads_retry_after_case_insensitive() {
        let err = classify_http_error(429, &hdrs(&[("Retry-After", "30")]), "", "m");
        assert_eq!(
            err,
            ProviderError::RateLimited {
                retry_after_seconds: Some(30)
            }
        );
    }

    #[test]
    fn classify_429_without_retry_after() {
        let err = classify_http_error(429, &[], "", "m");
        assert_eq!(
            err,
            ProviderError::RateLimited {
                retry_after_seconds: None
            }
        );
    }

    #[test]
    fn classify_500_is_upstream_server_error() {
        let body = r#"{"message":"boom"}"#;
        assert_eq!(
            classify_http_error(503, &[], body, "m"),
            ProviderError::Upstream {
                status: 503,
                message: "boom".to_string()
            }
        );
    }

    #[test]
    fn classify_400_is_upstream_with_extracted_message() {
        let body = r#"{"error":{"message":"bad request param"}}"#;
        assert_eq!(
            classify_http_error(400, &[], body, "m"),
            ProviderError::Upstream {
                status: 400,
                message: "bad request param".to_string()
            }
        );
    }

    #[test]
    fn classify_empty_body_gives_placeholder_message() {
        let err = classify_http_error(418, &[], "   ", "m");
        assert_eq!(
            err,
            ProviderError::Upstream {
                status: 418,
                message: "empty error response".to_string()
            }
        );
    }

    #[test]
    fn message_constructors_set_roles() {
        assert_eq!(Message::user("hi").role, "user");
        assert_eq!(Message::assistant("ok").role, "assistant");
        assert_eq!(Message::system("be nice").role, "system");
        let t = Message::tool_result("call-1", "result");
        assert_eq!(t.role, "tool");
        assert_eq!(t.tool_call_id.as_deref(), Some("call-1"));
    }
}
