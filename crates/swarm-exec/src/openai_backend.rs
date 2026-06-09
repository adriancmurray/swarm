//! `OpenAiCompatibleBackend` talks HTTP to any OpenAI-compatible
//! `/v1/chat/completions` endpoint. The base URL and API key come from the
//! environment (named by the descriptor); the key is never logged, written to
//! captured output, or placed in any error string.
//!
//! Streaming (`on_chunk = Some`) requests `stream: true` and maps each
//! Server-Sent-Events `choices[0].delta.content` chunk onto the callback as a
//! "stdout" chunk. Non-streaming (`on_chunk = None`) requests `stream: false`
//! and returns `choices[0].message.content` plus token usage.
//!
//! Gated behind the `openai` cargo feature so the default build pulls no
//! HTTP/TLS dependency.

use std::io::{BufRead, BufReader};
use std::time::Duration;

use serde_json::Value;

use swarm_kernel::backend_descriptor::BackendDescriptor;

use crate::executor::AgentBackend;
use swarm_kernel::backend_abi::{
    BackendCaps, BackendError, BackendRequest, BackendSink, RunOutcome, TokenUsage,
};

/// Endpoint used when the descriptor names no base-URL env var, or that var is
/// unset/empty.
const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";

/// A descriptor-driven backend for any OpenAI-compatible chat-completions API.
pub struct OpenAiCompatibleBackend {
    id: String,
    descriptor: BackendDescriptor,
}

impl OpenAiCompatibleBackend {
    pub fn new(id: impl Into<String>, descriptor: BackendDescriptor) -> Self {
        Self {
            id: id.into(),
            descriptor,
        }
    }

    /// Base URL from the descriptor's `base_url_env`, falling back to the
    /// default OpenAI endpoint.
    fn base_url(&self) -> String {
        self.descriptor
            .base_url_env
            .as_deref()
            .and_then(|name| std::env::var(name).ok())
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
    }

    /// API key from the env var named by the descriptor. Read only at call time
    /// and never stored, logged, or returned in an error (the var NAME may be
    /// named for actionability; the VALUE never is).
    fn api_key(&self) -> Result<String, BackendError> {
        let name = self.descriptor.api_key_env.as_deref().ok_or_else(|| {
            BackendError::NotReady(format!(
                "backend `{}` has no `api_key_env` set in its descriptor.",
                self.id
            ))
        })?;
        std::env::var(name)
            .ok()
            .filter(|v| !v.trim().is_empty())
            .ok_or_else(|| {
                BackendError::NotReady(format!(
                    "backend `{}` requires the API key env var `{name}` to be set.",
                    self.id
                ))
            })
    }

    /// Run-time model, then descriptor `default_model`, else a loud error.
    fn model(&self, req: &BackendRequest) -> Result<String, BackendError> {
        if let Some(m) = req.model {
            return Ok(m.to_string());
        }
        if let Some(m) = self
            .descriptor
            .default_model
            .as_deref()
            .filter(|m| !m.trim().is_empty())
        {
            return Ok(m.to_string());
        }
        Err(BackendError::NotReady(format!(
            "backend `{}` has no model: pass --model or set `default_model` in the descriptor.",
            self.id
        )))
    }

    /// Parse a Server-Sent-Events stream, forwarding each delta to `sink`
    /// and accumulating the full text.
    fn read_stream(
        &self,
        response: ureq::Response,
        sink: &mut dyn BackendSink,
    ) -> Result<RunOutcome, BackendError> {
        let reader = BufReader::new(response.into_reader());
        let mut stdout = String::new();
        let mut input_tokens = None;
        let mut output_tokens = None;
        for line in reader.lines() {
            let line = line.map_err(|e| {
                BackendError::Protocol(format!("backend `{}` failed reading stream: {e}", self.id))
            })?;
            let payload = match line.strip_prefix("data:") {
                Some(p) => p.trim(),
                None => continue,
            };
            if payload.is_empty() {
                continue;
            }
            if payload == "[DONE]" {
                break;
            }
            // Skip non-JSON keep-alive/comment frames rather than aborting.
            let Ok(v) = serde_json::from_str::<Value>(payload) else {
                continue;
            };
            if let Some(delta) = v["choices"][0]["delta"]["content"].as_str() {
                stdout.push_str(delta);
                sink.stdout_chunk(delta);
            }
            absorb_usage(&v, &mut input_tokens, &mut output_tokens);
        }
        Ok(http_outcome(stdout, input_tokens, output_tokens))
    }

    /// Parse a non-streaming JSON completion response.
    fn read_full(&self, response: ureq::Response) -> Result<RunOutcome, BackendError> {
        let text = response.into_string().map_err(|e| {
            BackendError::Protocol(format!(
                "backend `{}` failed reading response: {e}",
                self.id
            ))
        })?;
        let v: Value = serde_json::from_str(&text).map_err(|e| {
            BackendError::Protocol(format!("backend `{}` returned invalid JSON: {e}", self.id))
        })?;
        let content = v["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("")
            .to_string();
        let mut input_tokens = None;
        let mut output_tokens = None;
        absorb_usage(&v, &mut input_tokens, &mut output_tokens);
        Ok(http_outcome(content, input_tokens, output_tokens))
    }
}

/// Build a [`RunOutcome`] for a non-process HTTP backend: `exit_status` is
/// `None` (no subprocess), not timed out, and `retryable` is `false` (a parsed
/// 2xx response is a terminal success). Token counts fold into `token_usage`.
fn http_outcome(stdout: String, input: Option<u64>, output: Option<u64>) -> RunOutcome {
    let token_usage = match (input, output) {
        (None, None) => None,
        (input, output) => Some(TokenUsage { input, output }),
    };
    RunOutcome {
        exit_status: None,
        stdout,
        stderr: String::new(),
        timed_out: false,
        retryable: false,
        token_usage,
    }
}

/// Pull `usage.prompt_tokens`/`completion_tokens` from a response object, if
/// present, without overwriting an already-seen value with `None`.
fn absorb_usage(v: &Value, input: &mut Option<u64>, output: &mut Option<u64>) {
    if let Some(usage) = v.get("usage") {
        if let Some(t) = usage.get("prompt_tokens").and_then(Value::as_u64) {
            *input = Some(t);
        }
        if let Some(t) = usage.get("completion_tokens").and_then(Value::as_u64) {
            *output = Some(t);
        }
    }
}

impl AgentBackend for OpenAiCompatibleBackend {
    fn id(&self) -> &str {
        &self.id
    }

    fn ready(&self) -> Result<(), BackendError> {
        self.api_key().map(|_| ())
    }

    fn run(
        &self,
        req: &BackendRequest,
        sink: &mut dyn BackendSink,
    ) -> Result<RunOutcome, BackendError> {
        let key = self.api_key()?;
        let model = self.model(req)?;
        let url = format!("{}/chat/completions", self.base_url().trim_end_matches('/'));
        // A sink that wants streaming requests `stream: true` (SSE); a one-shot
        // sink (e.g. NullSink) requests `stream: false` so we read usage tokens.
        let streaming = sink.wants_streaming();

        let body = serde_json::json!({
            "model": model,
            "messages": [{ "role": "user", "content": req.prompt }],
            "stream": streaming,
        });
        let body_str = serde_json::to_string(&body).map_err(|e| {
            BackendError::Protocol(format!(
                "backend `{}` failed to encode request: {e}",
                self.id
            ))
        })?;

        let result = ureq::post(&url)
            .set("Authorization", &format!("Bearer {key}"))
            .set("Content-Type", "application/json")
            .timeout(req.timeout + Duration::from_secs(30))
            .send_string(&body_str);

        let response = match result {
            Ok(r) => r,
            Err(ureq::Error::Status(code, r)) => {
                // Upstream rejected the request. The response body may carry an
                // error message (it never contains our key); truncate to bound
                // output size. 5xx and 429 are transient (retryable); other 4xx
                // are caller errors that will not improve on retry.
                let detail: String = r
                    .into_string()
                    .unwrap_or_default()
                    .chars()
                    .take(2000)
                    .collect();
                let retryable = code >= 500 || code == 429;
                return Err(BackendError::Upstream {
                    status: Some(code),
                    retryable,
                    detail: format!("backend `{}` upstream HTTP {code}: {detail}", self.id),
                });
            }
            Err(e) => {
                let msg = e.to_string();
                // ureq surfaces read/connect timeouts as transport errors; flag
                // them like the subprocess backends do.
                if msg.to_lowercase().contains("timed out")
                    || msg.to_lowercase().contains("timeout")
                {
                    return Ok(RunOutcome {
                        exit_status: None,
                        stdout: String::new(),
                        stderr: msg,
                        timed_out: true,
                        retryable: true,
                        token_usage: None,
                    });
                }
                return Err(BackendError::Upstream {
                    status: None,
                    retryable: true,
                    detail: format!("backend `{}` transport failure: {msg}", self.id),
                });
            }
        };

        if streaming {
            self.read_stream(response, sink)
        } else {
            self.read_full(response)
        }
    }

    fn capabilities(&self) -> BackendCaps {
        BackendCaps::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::path::Path;
    use std::sync::{Arc, Mutex};
    use std::thread::JoinHandle;
    use swarm_kernel::backend_abi::{CancelToken, ClosureSink, EnvPolicy, NullSink};
    use swarm_kernel::backend_descriptor::{BackendDescriptor, BackendKind};

    use crate::executor::AgentBackend;

    /// A `BackendRequest` with a fixed test model and temp cwd.
    fn req<'a>(prompt: &'a str, cwd: &'a Path) -> BackendRequest<'a> {
        BackendRequest {
            prompt,
            model: Some("test-model"),
            cwd,
            timeout: Duration::from_secs(30),
            quiet: true,
            allow_bypass_permissions: false,
            env_policy: EnvPolicy::Inherit,
            cancel: CancelToken::new(),
        }
    }

    /// A one-shot mock HTTP/1.1 server. Accepts a single connection, captures the
    /// full request (headers + body), and replies with `content_type` + `body`.
    /// Returns the bound `http://127.0.0.1:<port>` URL and a handle yielding the
    /// captured request text. Network-free: binds 127.0.0.1:0.
    fn mock_server(content_type: &'static str, body: &'static str) -> (String, JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            // Read until end-of-headers so the Authorization header is captured
            // regardless of TCP packet boundaries.
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request = String::new();
            let mut content_length = 0usize;
            loop {
                let mut line = String::new();
                let n = reader.read_line(&mut line).unwrap();
                if let Some(len) = line
                    .to_ascii_lowercase()
                    .strip_prefix("content-length:")
                    .and_then(|v| v.trim().parse::<usize>().ok())
                {
                    content_length = len;
                }
                request.push_str(&line);
                if n == 0 || line == "\r\n" {
                    break;
                }
            }
            // Drain the request body before replying — otherwise closing the
            // connection mid-write races the client's body write (EINVAL).
            if content_length > 0 {
                let mut body = vec![0u8; content_length];
                reader.read_exact(&mut body).unwrap();
                request.push_str(&String::from_utf8_lossy(&body));
            }
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).unwrap();
            stream.flush().unwrap();
            request
        });
        (format!("http://127.0.0.1:{port}/v1"), handle)
    }

    #[test]
    fn streams_sse_deltas_to_on_chunk() {
        let sse = "data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}\n\n\
                   data: {\"choices\":[{\"delta\":{\"content\":\", world\"}}]}\n\n\
                   data: [DONE]\n\n";
        let (base, server) = mock_server("text/event-stream", sse);

        std::env::set_var("OAI_A2_STREAM_BASE", &base);
        std::env::set_var("OAI_A2_STREAM_KEY", "sk-stream-key");
        let desc = BackendDescriptor {
            kind: BackendKind::OpenAiCompatible,
            base_url_env: Some("OAI_A2_STREAM_BASE".into()),
            api_key_env: Some("OAI_A2_STREAM_KEY".into()),
            ..Default::default()
        };
        let backend = OpenAiCompatibleBackend::new("api", desc);

        let chunks = Arc::new(Mutex::new(Vec::<String>::new()));
        let chunks2 = Arc::clone(&chunks);
        let mut cb = move |stream: &str, text: &str| {
            assert_eq!(stream, "stdout");
            chunks2.lock().unwrap().push(text.to_string());
        };
        let cwd = std::env::temp_dir();
        let out = backend
            .run(&req("hi", &cwd), &mut ClosureSink::new(&mut cb))
            .unwrap();
        server.join().unwrap();

        assert_eq!(out.stdout, "Hello, world");
        assert_eq!(
            *chunks.lock().unwrap(),
            vec!["Hello".to_string(), ", world".to_string()]
        );
        assert!(!out.timed_out);
    }

    #[test]
    fn non_streaming_returns_message_content_and_usage() {
        let json = "{\"choices\":[{\"message\":{\"content\":\"Full answer\"}}],\
                    \"usage\":{\"prompt_tokens\":11,\"completion_tokens\":7}}";
        let (base, server) = mock_server("application/json", json);

        std::env::set_var("OAI_A2_JSON_BASE", &base);
        std::env::set_var("OAI_A2_JSON_KEY", "sk-json-key");
        let desc = BackendDescriptor {
            kind: BackendKind::OpenAiCompatible,
            base_url_env: Some("OAI_A2_JSON_BASE".into()),
            api_key_env: Some("OAI_A2_JSON_KEY".into()),
            ..Default::default()
        };
        let backend = OpenAiCompatibleBackend::new("api", desc);

        let cwd = std::env::temp_dir();
        let out = backend.run(&req("hi", &cwd), &mut NullSink).unwrap();
        server.join().unwrap();

        assert_eq!(out.stdout, "Full answer");
        let usage = out.token_usage.expect("usage present");
        assert_eq!(usage.input, Some(11));
        assert_eq!(usage.output, Some(7));
    }

    #[test]
    fn missing_api_key_errors_with_var_name() {
        // The key env var is never set, so it is naturally absent.
        let desc = BackendDescriptor {
            kind: BackendKind::OpenAiCompatible,
            api_key_env: Some("OAI_A2_NEVER_SET_KEY".into()),
            ..Default::default()
        };
        let backend = OpenAiCompatibleBackend::new("api", desc);
        let cwd = std::env::temp_dir();
        let err = backend
            .run(&req("hi", &cwd), &mut NullSink)
            .unwrap_err()
            .to_string();
        // Names the missing var (actionable) — the var NAME is not a secret.
        assert!(
            err.contains("OAI_A2_NEVER_SET_KEY"),
            "error should name the missing env var: {err}"
        );
    }

    #[test]
    fn sends_bearer_key_upstream_but_never_in_output() {
        const SECRET: &str = "sk-SECRET-do-not-leak-9876";
        let sse = "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\ndata: [DONE]\n\n";
        let (base, server) = mock_server("text/event-stream", sse);

        std::env::set_var("OAI_A2_SECRET_BASE", &base);
        std::env::set_var("OAI_A2_SECRET_KEY", SECRET);
        let desc = BackendDescriptor {
            kind: BackendKind::OpenAiCompatible,
            base_url_env: Some("OAI_A2_SECRET_BASE".into()),
            api_key_env: Some("OAI_A2_SECRET_KEY".into()),
            ..Default::default()
        };
        let backend = OpenAiCompatibleBackend::new("api", desc);

        // Streaming path (SSE body) — the callback's content is irrelevant here.
        let mut cb = |_stream: &str, _text: &str| {};
        let cwd = std::env::temp_dir();
        let out = backend
            .run(&req("hi", &cwd), &mut ClosureSink::new(&mut cb))
            .unwrap();
        let request = server.join().unwrap();

        // The key IS sent upstream as a bearer token...
        assert!(
            request.contains(&format!("Authorization: Bearer {SECRET}")),
            "request should carry the bearer key"
        );
        // ...but NEVER appears in captured output (§11 secret handling).
        assert!(!out.stdout.contains(SECRET));
        assert!(!out.stderr.contains(SECRET));
    }
}
