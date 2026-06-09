//! Declarative backend descriptors. The common way to add an agent is config,
//! not code: a `cli` descriptor wraps any command-line agent, an
//! `openai-compatible` descriptor points at any `/v1/chat/completions` endpoint,
//! and a `native` descriptor selects the built-in agent harness by provider id.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BackendKind {
    /// Wrap a command-line agent as a subprocess.
    #[default]
    Cli,
    /// Talk HTTP to any OpenAI-compatible `/v1/chat/completions` endpoint.
    /// Explicit rename: kebab-case would split `OpenAi` into `open-ai`.
    #[serde(rename = "openai-compatible")]
    OpenAiCompatible,
    /// Run the built-in agent harness in-process, selected by `provider`.
    Native,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PromptDelivery {
    /// Prompt is written to the child's stdin (default).
    #[default]
    Stdin,
    /// Prompt is appended as the final positional argument.
    Arg,
}

/// A declarative agent backend. Fields are per-kind; unused fields stay `None`
/// or empty and never error during parsing, so any kind round-trips from a
/// sparse config block.
///
/// - `kind = cli`: `command` is required at execution time; `args`/`prompt`
///   shape the subprocess. `{model}` and `{prompt}` tokens in `args` are
///   substituted at run time.
/// - `kind = openai-compatible`: `base_url_env`/`api_key_env` name the env vars
///   holding the endpoint base URL and API key; `default_model` is used when no
///   model is supplied at run time.
/// - `kind = native`: `provider` selects the in-process harness provider.
///
/// `Default` yields an empty `cli` descriptor — handy for constructing a
/// descriptor of any kind by overriding only the relevant fields.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendDescriptor {
    pub kind: BackendKind,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub prompt: PromptDelivery,
    /// Env var holding the OpenAI-compatible base URL (e.g. `OPENAI_BASE_URL`).
    /// When unset or the var is absent, a sensible default endpoint is used.
    #[serde(default)]
    pub base_url_env: Option<String>,
    /// Env var holding the OpenAI-compatible API key (e.g. `OPENAI_API_KEY`).
    /// Secrets are read from the environment only — never from this struct.
    #[serde(default)]
    pub api_key_env: Option<String>,
    /// Model used when the run does not specify one (openai-compatible kind).
    #[serde(default)]
    pub default_model: Option<String>,
    /// Provider id selecting the in-process harness (native kind).
    #[serde(default)]
    pub provider: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cli_descriptor_from_json() {
        let json = r#"{
            "kind": "cli",
            "command": "some-agent",
            "args": ["--print", "{model}"],
            "prompt": "stdin",
            "stream": "stdout-lines"
        }"#;
        let d: BackendDescriptor = serde_json::from_str(json).unwrap();
        assert_eq!(d.command.as_deref(), Some("some-agent"));
        assert_eq!(d.args, vec!["--print".to_string(), "{model}".to_string()]);
        assert!(matches!(d.kind, BackendKind::Cli));
        assert!(matches!(d.prompt, PromptDelivery::Stdin));
    }

    #[test]
    fn prompt_defaults_to_stdin_when_absent() {
        let d: BackendDescriptor =
            serde_json::from_str(r#"{ "kind": "cli", "command": "x" }"#).unwrap();
        assert!(matches!(d.prompt, PromptDelivery::Stdin));
        assert!(d.args.is_empty());
    }

    #[test]
    fn parses_openai_compatible_descriptor() {
        let json = r#"{
            "kind": "openai-compatible",
            "base_url_env": "OPENAI_BASE_URL",
            "api_key_env": "OPENAI_API_KEY",
            "default_model": "gpt-4o-mini"
        }"#;
        let d: BackendDescriptor = serde_json::from_str(json).unwrap();
        assert!(matches!(d.kind, BackendKind::OpenAiCompatible));
        assert_eq!(d.base_url_env.as_deref(), Some("OPENAI_BASE_URL"));
        assert_eq!(d.api_key_env.as_deref(), Some("OPENAI_API_KEY"));
        assert_eq!(d.default_model.as_deref(), Some("gpt-4o-mini"));
        // A cli-only field stays absent without erroring.
        assert!(d.command.is_none());
    }

    #[test]
    fn openai_fields_default_to_none_when_absent() {
        let d: BackendDescriptor =
            serde_json::from_str(r#"{ "kind": "openai-compatible" }"#).unwrap();
        assert!(d.base_url_env.is_none());
        assert!(d.api_key_env.is_none());
        assert!(d.default_model.is_none());
    }

    #[test]
    fn parses_native_descriptor() {
        let d: BackendDescriptor =
            serde_json::from_str(r#"{ "kind": "native", "provider": "api" }"#).unwrap();
        assert!(matches!(d.kind, BackendKind::Native));
        assert_eq!(d.provider.as_deref(), Some("api"));
    }
}
