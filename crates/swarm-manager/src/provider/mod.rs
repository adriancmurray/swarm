//! Provider abstraction: the async `Provider` trait, the `ProviderType`
//! enum, and a `create_provider` factory.
//!
//! Concrete HTTP-backed implementations land in a later task (behind an
//! `http` feature). For now the factory returns a loud `NotImplemented`
//! error for every type so callers fail clearly rather than silently.

pub mod crypto;
pub mod registry;
pub mod types;

#[cfg(feature = "http")]
pub mod anthropic;
#[cfg(feature = "http")]
pub mod gemini;
#[cfg(feature = "http")]
pub mod lmstudio;
#[cfg(feature = "http")]
pub mod ollama;
#[cfg(feature = "http")]
pub mod openai;

#[cfg(all(test, feature = "http"))]
pub(crate) mod mock;

pub use registry::{KeyStatus, ProviderConfig, ProviderRegistry};
pub use types::*;

#[cfg(feature = "http")]
pub use anthropic::AnthropicProvider;
#[cfg(feature = "http")]
pub use gemini::GeminiProvider;
#[cfg(feature = "http")]
pub use lmstudio::LmStudioProvider;
#[cfg(feature = "http")]
pub use ollama::OllamaProvider;
#[cfg(feature = "http")]
pub use openai::OpenAIProvider;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Provider type enum for configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ProviderType {
    /// No provider configured.
    None,
    /// OpenAI API.
    OpenAI,
    /// Local Ollama server (default).
    #[default]
    Ollama,
    /// Google Gemini API.
    Gemini,
    /// LM Studio local server.
    LMStudio,
    /// Apple MLX local server (OpenAI-compatible via mlx-lm).
    MLX,
    /// Anthropic API.
    Anthropic,
    /// OpenRouter proxy.
    OpenRouter,
    /// DeepSeek API (OpenAI-compatible).
    DeepSeek,
}

impl ProviderType {
    pub fn as_str(&self) -> &'static str {
        match self {
            ProviderType::None => "none",
            ProviderType::OpenAI => "openai",
            ProviderType::Ollama => "ollama",
            ProviderType::Gemini => "gemini",
            ProviderType::LMStudio => "lmstudio",
            ProviderType::MLX => "mlx",
            ProviderType::Anthropic => "anthropic",
            ProviderType::OpenRouter => "openrouter",
            ProviderType::DeepSeek => "deepseek",
        }
    }

    // Infallible parse that defaults unknown strings to `None`, so it
    // intentionally does not match the fallible `std::str::FromStr` shape.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "openai" => ProviderType::OpenAI,
            "ollama" => ProviderType::Ollama,
            "gemini" => ProviderType::Gemini,
            "lmstudio" | "lm_studio" => ProviderType::LMStudio,
            "mlx" | "mlx-lm" | "mlx_lm" => ProviderType::MLX,
            "anthropic" => ProviderType::Anthropic,
            "openrouter" => ProviderType::OpenRouter,
            "deepseek" => ProviderType::DeepSeek,
            _ => ProviderType::None,
        }
    }

    /// Check if this provider requires an API key.
    pub fn requires_api_key(&self) -> bool {
        matches!(
            self,
            ProviderType::OpenAI
                | ProviderType::Gemini
                | ProviderType::Anthropic
                | ProviderType::OpenRouter
                | ProviderType::DeepSeek
        )
    }

    /// Get the default endpoint for this provider.
    pub fn default_endpoint(&self) -> Option<&'static str> {
        match self {
            ProviderType::OpenAI => Some("https://api.openai.com/v1"),
            ProviderType::Ollama => Some("http://localhost:11434"),
            ProviderType::LMStudio => Some("http://localhost:1234"),
            ProviderType::MLX => Some("http://localhost:8080"),
            ProviderType::Gemini => None,
            ProviderType::Anthropic => Some("https://api.anthropic.com/v1"),
            ProviderType::OpenRouter => Some("https://openrouter.ai/api/v1"),
            ProviderType::DeepSeek => Some("https://api.deepseek.com"),
            ProviderType::None => None,
        }
    }

    pub fn suggested_models(&self) -> &'static [&'static str] {
        match self {
            ProviderType::OpenAI => &["gpt-4o", "gpt-4o-mini"],
            ProviderType::DeepSeek => &["deepseek-v4-flash", "deepseek-v4-pro"],
            ProviderType::Gemini => &[
                "gemini-3.5-flash",
                "gemini-3.1-pro-preview",
                "gemini-3.1-flash-lite",
                "gemini-3-flash-preview",
                "gemini-2.5-pro",
                "gemini-2.5-flash",
                "gemini-2.5-flash-lite",
            ],
            ProviderType::Anthropic => {
                &["claude-sonnet-4-6", "claude-opus-4-7", "claude-haiku-4-5"]
            }
            ProviderType::MLX => &["mlx-community/gemma-4-e2b-it-OptiQ-4bit"],
            ProviderType::OpenRouter
            | ProviderType::Ollama
            | ProviderType::LMStudio
            | ProviderType::None => &[],
        }
    }

    /// Legacy model aliases that should remain loadable but not be used
    /// as first-choice suggestions for new presets.
    pub fn legacy_model_aliases(&self) -> &'static [&'static str] {
        match self {
            ProviderType::DeepSeek => &["deepseek-chat", "deepseek-reasoner"],
            _ => &[],
        }
    }
}

/// An LLM provider capable of a single chat completion turn.
#[async_trait]
pub trait Provider: Send + Sync {
    async fn chat(
        &self,
        messages: Vec<Message>,
        tools: Option<Vec<serde_json::Value>>,
    ) -> Result<LLMResponse, ProviderError>;
}

/// Build a `Provider` instance from a configuration.
///
/// With the `http` feature on, constructs the concrete HTTP-backed provider
/// for `config.provider_type`. Without it, every type returns a loud
/// `NotImplemented` — never a silent no-op.
///
/// Model selection uses `config.models.first()` (empty → provider default);
/// temperature defaults to `0.7` and `max_tokens` to `None`, since
/// `ProviderConfig` carries no sampling knobs. Real sampling plumbing rides
/// on the agent/preset layer in a later task.
#[cfg(feature = "http")]
pub fn create_provider(config: &ProviderConfig) -> Result<Arc<dyn Provider>, ProviderError> {
    let model = config.models.first().cloned().unwrap_or_default();
    let api_key = config.api_key.clone();
    let base = config.effective_endpoint();
    let temperature = 0.7_f32;
    let max_tokens = None;

    let require_key = || -> Result<String, ProviderError> {
        api_key.clone().ok_or_else(|| {
            ProviderError::NotConfigured(format!(
                "API key required for provider '{}'",
                config.provider_type.as_str()
            ))
        })
    };

    let provider: Arc<dyn Provider> =
        match config.provider_type {
            ProviderType::None => {
                return Err(ProviderError::NotConfigured(
                    "provider type is none".to_string(),
                ))
            }
            ProviderType::OpenAI | ProviderType::OpenRouter => Arc::new(
                openai::OpenAIProvider::new(require_key()?, base, model, temperature, max_tokens),
            ),
            ProviderType::DeepSeek => {
                let model = if model.trim().is_empty() {
                    config.provider_type.suggested_models()[0].to_string()
                } else {
                    model
                };
                Arc::new(openai::OpenAIProvider::deepseek(
                    require_key()?,
                    base,
                    model,
                    temperature,
                    max_tokens,
                ))
            }
            ProviderType::Anthropic => Arc::new(anthropic::AnthropicProvider::new(
                require_key()?,
                base,
                model,
                temperature,
                max_tokens,
            )),
            ProviderType::Gemini => {
                let mut provider = gemini::GeminiProvider::new(require_key()?, model);
                if config.endpoint.is_some() {
                    provider.base_url = base;
                }
                Arc::new(provider)
            }
            ProviderType::Ollama => Arc::new(ollama::OllamaProvider::new(base, model)),
            ProviderType::LMStudio | ProviderType::MLX => Arc::new(
                lmstudio::LmStudioProvider::new(base, (!model.is_empty()).then_some(model)),
            ),
        };
    Ok(provider)
}

/// Stub factory for the feature-off build: every provider type fails loudly
/// with `NotImplemented` (no silent no-op).
#[cfg(not(feature = "http"))]
pub fn create_provider(config: &ProviderConfig) -> Result<Arc<dyn Provider>, ProviderError> {
    Err(ProviderError::NotImplemented(format!(
        "provider implementation for '{}' requires the `http` feature",
        config.provider_type.as_str()
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_type_str_roundtrip() {
        for ty in [
            ProviderType::OpenAI,
            ProviderType::Ollama,
            ProviderType::Gemini,
            ProviderType::LMStudio,
            ProviderType::Anthropic,
            ProviderType::OpenRouter,
            ProviderType::DeepSeek,
        ] {
            assert_eq!(ProviderType::from_str(ty.as_str()), ty);
        }
        assert_eq!(ProviderType::from_str("unknown"), ProviderType::None);
    }

    #[test]
    fn requires_api_key_matches_cloud_providers() {
        assert!(ProviderType::OpenAI.requires_api_key());
        assert!(ProviderType::Anthropic.requires_api_key());
        assert!(!ProviderType::Ollama.requires_api_key());
        assert!(!ProviderType::LMStudio.requires_api_key());
    }

    #[cfg(not(feature = "http"))]
    #[test]
    fn create_provider_requires_http_feature_when_off() {
        let config = ProviderConfig::new(
            "X".to_string(),
            ProviderType::OpenAI,
            None,
            Some("sk-x".to_string()),
        );
        match create_provider(&config) {
            Err(ProviderError::NotImplemented(_)) => {}
            other => panic!(
                "expected NotImplemented, got {:?}",
                other.map(|_| "provider")
            ),
        }
    }

    #[cfg(feature = "http")]
    #[test]
    fn create_provider_builds_concrete_impl_under_http() {
        let config = ProviderConfig::new(
            "X".to_string(),
            ProviderType::OpenAI,
            None,
            Some("sk-x".to_string()),
        );
        assert!(create_provider(&config).is_ok());

        // A cloud type with no key fails loudly, never silently.
        let no_key = ProviderConfig::new("Y".to_string(), ProviderType::Anthropic, None, None);
        match create_provider(&no_key) {
            Err(ProviderError::NotConfigured(_)) => {}
            other => panic!(
                "expected NotConfigured, got {:?}",
                other.map(|_| "provider")
            ),
        }

        // Local providers need no key.
        let local = ProviderConfig::new("Z".to_string(), ProviderType::Ollama, None, None);
        assert!(create_provider(&local).is_ok());
    }

    #[test]
    fn deepseek_defaults_track_official_api_shape() {
        assert_eq!(
            ProviderType::DeepSeek.default_endpoint(),
            Some("https://api.deepseek.com")
        );
        assert_eq!(
            ProviderType::DeepSeek.suggested_models(),
            &["deepseek-v4-flash", "deepseek-v4-pro"]
        );
        assert_eq!(
            ProviderType::DeepSeek.legacy_model_aliases(),
            &["deepseek-chat", "deepseek-reasoner"]
        );
    }
}
