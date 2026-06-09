//! Rich backend ABI types — the process-agnostic execution contract.
//!
//! These types are additive scaffolding for the public `AgentBackend` ABI
//! (spec §4.3). They carry full execution context up front (`BackendRequest`),
//! branch failures on cause rather than string-matching (`BackendError`), and
//! stream output through a sink (`BackendSink`) instead of a bare closure.
//!
//! Nothing here wires into the existing trait yet — that lands in the next
//! step. This module is purely new types + adapters so existing call sites can
//! migrate cleanly.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// LLM token accounting, when the backend reports it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TokenUsage {
    /// Input (prompt) token count, if reported.
    pub input: Option<u64>,
    /// Output (completion) token count, if reported.
    pub output: Option<u64>,
}

/// The process-agnostic successor to the legacy output struct.
///
/// `exit_status` is `Option<i32>` (not `std::process::ExitStatus`) so HTTP
/// backends that never spawn a process can set `None`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RunOutcome {
    /// Process exit code, if the backend ran a subprocess. `None` for
    /// non-process backends (e.g. HTTP).
    pub exit_status: Option<i32>,
    /// Captured stdout.
    pub stdout: String,
    /// Captured stderr.
    pub stderr: String,
    /// Whether the run hit its timeout.
    pub timed_out: bool,
    /// Whether this outcome is worth retrying (transient failure).
    pub retryable: bool,
    /// Token accounting, when available.
    pub token_usage: Option<TokenUsage>,
}

/// Typed backend failure — callers branch on cause, never on error text.
#[derive(Debug, Clone, PartialEq)]
pub enum BackendError {
    /// Backend cannot run yet (binary missing, key absent, …). Carries detail.
    NotReady(String),
    /// The run exceeded its timeout.
    Timeout,
    /// The run was cancelled via a `CancelToken`.
    Cancelled,
    /// Spawning the backend process failed. Carries detail.
    Spawn(String),
    /// Protocol-level failure parsing/handshaking with the backend. Carries detail.
    Protocol(String),
    /// Upstream service error (e.g. HTTP non-2xx).
    Upstream {
        /// HTTP-style status code, if any.
        status: Option<u16>,
        /// Whether retrying may succeed.
        retryable: bool,
        /// Human-readable detail.
        detail: String,
    },
}

impl BackendError {
    /// Whether retrying this error may succeed. `Timeout` and
    /// `Upstream { retryable: true, .. }` are retryable; everything else is not.
    pub fn is_retryable(&self) -> bool {
        match self {
            BackendError::Timeout => true,
            BackendError::Upstream { retryable, .. } => *retryable,
            BackendError::NotReady(_)
            | BackendError::Cancelled
            | BackendError::Spawn(_)
            | BackendError::Protocol(_) => false,
        }
    }
}

impl std::fmt::Display for BackendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BackendError::NotReady(detail) => write!(f, "backend not ready: {detail}"),
            BackendError::Timeout => write!(f, "backend timed out"),
            BackendError::Cancelled => write!(f, "backend run cancelled"),
            BackendError::Spawn(detail) => write!(f, "failed to spawn backend: {detail}"),
            BackendError::Protocol(detail) => write!(f, "backend protocol error: {detail}"),
            BackendError::Upstream {
                status,
                retryable,
                detail,
            } => {
                write!(f, "upstream error")?;
                if let Some(code) = status {
                    write!(f, " (status {code})")?;
                }
                write!(f, ": {detail}")?;
                if *retryable {
                    write!(f, " [retryable]")?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for BackendError {}

/// Cooperative cancellation flag, cheap to clone and share across threads.
///
/// Cloning shares the same underlying flag, so cancelling any clone is observed
/// by all of them.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    /// A fresh, un-cancelled token.
    pub fn new() -> Self {
        Self::default()
    }

    /// Request cancellation. Idempotent.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    /// Whether cancellation has been requested.
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// How a backend treats the ambient process environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EnvPolicy {
    /// Inherit the parent process environment (today's behavior).
    #[default]
    Inherit,
    /// Deny inheritance — run with a clean environment.
    Deny,
}

/// Full execution context handed to a backend up front (borrowed).
pub struct BackendRequest<'a> {
    /// The prompt to send.
    pub prompt: &'a str,
    /// Optional model override.
    pub model: Option<&'a str>,
    /// Working directory for the run.
    pub cwd: &'a Path,
    /// Wall-clock timeout.
    pub timeout: Duration,
    /// Suppress backend chatter where supported.
    pub quiet: bool,
    /// Permit the backend to bypass its interactive permission prompts (e.g.
    /// claude's `--permission-mode bypassPermissions`). A per-request policy bit
    /// alongside `quiet`; backends that have no such mode ignore it.
    pub allow_bypass_permissions: bool,
    /// Environment inheritance policy.
    pub env_policy: EnvPolicy,
    /// Cooperative cancellation handle.
    pub cancel: CancelToken,
}

/// What a backend can do — reported by `AgentBackend::capabilities`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackendCaps {
    /// The backend can stream output chunks as they arrive.
    pub streaming: bool,
    /// The backend honors a `CancelToken` to abort mid-run.
    pub cancellation: bool,
}

impl Default for BackendCaps {
    /// Today's backends stream but do not yet wire cancellation.
    fn default() -> Self {
        Self {
            streaming: true,
            cancellation: false,
        }
    }
}

/// Streaming output sink. Backends push chunks here instead of returning a
/// monolithic blob, and may emit a structured final answer.
pub trait BackendSink {
    /// A chunk of stdout text.
    fn stdout_chunk(&mut self, text: &str);
    /// A chunk of stderr text.
    fn stderr_chunk(&mut self, text: &str);
    /// The final structured answer, if the backend produces one. Default no-op.
    fn final_answer(&mut self, _text: &str) {}
    /// Whether the caller wants live streaming. Backends that can run either a
    /// streaming or a one-shot path (e.g. Claude's JSON token-capture path, or
    /// an HTTP backend's `stream:false` mode) consult this to choose. A sink
    /// that discards everything (`NullSink`) reports `false` so those backends
    /// take their richer one-shot path; chunk-forwarding sinks report `true`.
    fn wants_streaming(&self) -> bool {
        true
    }
}

/// A sink that discards everything.
pub struct NullSink;

impl BackendSink for NullSink {
    fn stdout_chunk(&mut self, _text: &str) {}
    fn stderr_chunk(&mut self, _text: &str) {}
    /// A discarding sink wants no streaming — backends take their one-shot path.
    fn wants_streaming(&self) -> bool {
        false
    }
}

/// Bridges the new `BackendSink` to an existing `FnMut(&str, &str)` consumer
/// (the legacy `on_chunk("stdout"|"stderr", text)` shape) so call sites migrate
/// cleanly. `stdout_chunk` calls the closure with `("stdout", text)` and
/// `stderr_chunk` with `("stderr", text)`.
pub struct ClosureSink<'a> {
    inner: &'a mut dyn FnMut(&str, &str),
}

impl<'a> ClosureSink<'a> {
    /// Wrap a `FnMut(&str, &str)` consumer.
    pub fn new(inner: &'a mut dyn FnMut(&str, &str)) -> Self {
        Self { inner }
    }
}

impl BackendSink for ClosureSink<'_> {
    fn stdout_chunk(&mut self, text: &str) {
        (self.inner)("stdout", text);
    }

    fn stderr_chunk(&mut self, text: &str) {
        (self.inner)("stderr", text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_outcome_default_is_empty_and_not_retryable() {
        let out = RunOutcome::default();
        assert_eq!(out.exit_status, None);
        assert_eq!(out.stdout, "");
        assert_eq!(out.stderr, "");
        assert!(!out.timed_out);
        assert!(!out.retryable);
        assert_eq!(out.token_usage, None);
    }

    #[test]
    fn token_usage_default_is_none() {
        let tu = TokenUsage::default();
        assert_eq!(
            tu,
            TokenUsage {
                input: None,
                output: None
            }
        );
    }

    #[test]
    fn backend_error_is_retryable_per_variant() {
        assert!(!BackendError::NotReady("x".into()).is_retryable());
        assert!(BackendError::Timeout.is_retryable());
        assert!(!BackendError::Cancelled.is_retryable());
        assert!(!BackendError::Spawn("x".into()).is_retryable());
        assert!(!BackendError::Protocol("x".into()).is_retryable());
        assert!(BackendError::Upstream {
            status: Some(503),
            retryable: true,
            detail: "overloaded".into(),
        }
        .is_retryable());
        assert!(!BackendError::Upstream {
            status: Some(400),
            retryable: false,
            detail: "bad request".into(),
        }
        .is_retryable());
    }

    #[test]
    fn backend_error_display_each_variant() {
        assert_eq!(
            BackendError::NotReady("no binary".into()).to_string(),
            "backend not ready: no binary"
        );
        assert_eq!(BackendError::Timeout.to_string(), "backend timed out");
        assert_eq!(BackendError::Cancelled.to_string(), "backend run cancelled");
        assert_eq!(
            BackendError::Spawn("EPERM".into()).to_string(),
            "failed to spawn backend: EPERM"
        );
        assert_eq!(
            BackendError::Protocol("bad json".into()).to_string(),
            "backend protocol error: bad json"
        );
        assert_eq!(
            BackendError::Upstream {
                status: Some(503),
                retryable: true,
                detail: "overloaded".into(),
            }
            .to_string(),
            "upstream error (status 503): overloaded [retryable]"
        );
        assert_eq!(
            BackendError::Upstream {
                status: None,
                retryable: false,
                detail: "unknown".into(),
            }
            .to_string(),
            "upstream error: unknown"
        );
    }

    #[test]
    fn backend_error_is_std_error() {
        // Confirm the trait object compiles — `BackendError: std::error::Error`.
        let err: Box<dyn std::error::Error> = Box::new(BackendError::Timeout);
        assert_eq!(err.to_string(), "backend timed out");
    }

    #[test]
    fn cancel_token_starts_uncancelled_then_cancels() {
        let token = CancelToken::new();
        assert!(!token.is_cancelled());
        token.cancel();
        assert!(token.is_cancelled());
    }

    #[test]
    fn cancel_token_clone_shares_flag() {
        let token = CancelToken::new();
        let clone = token.clone();
        token.cancel();
        assert!(clone.is_cancelled());
    }

    #[test]
    fn env_policy_default_is_inherit() {
        assert_eq!(EnvPolicy::default(), EnvPolicy::Inherit);
    }

    #[test]
    fn null_sink_is_noop() {
        let mut sink = NullSink;
        // These must not panic and have no observable effect.
        sink.stdout_chunk("ignored");
        sink.stderr_chunk("ignored");
        sink.final_answer("ignored");
    }

    #[test]
    fn backend_caps_default_streams_without_cancellation() {
        let caps = BackendCaps::default();
        assert!(caps.streaming);
        assert!(!caps.cancellation);
    }

    #[test]
    fn null_sink_does_not_want_streaming() {
        let sink = NullSink;
        assert!(!sink.wants_streaming());
    }

    #[test]
    fn closure_sink_wants_streaming() {
        let mut noop = |_stream: &str, _text: &str| {};
        let sink = ClosureSink::new(&mut noop);
        assert!(sink.wants_streaming());
    }

    #[test]
    fn closure_sink_forwards_with_stream_labels() {
        let mut events: Vec<(String, String)> = Vec::new();
        {
            let mut consumer =
                |stream: &str, text: &str| events.push((stream.to_string(), text.to_string()));
            let mut sink = ClosureSink::new(&mut consumer);
            sink.stdout_chunk("hello");
            sink.stderr_chunk("oops");
            sink.final_answer("done"); // default no-op, must not forward
        }
        assert_eq!(
            events,
            vec![
                ("stdout".to_string(), "hello".to_string()),
                ("stderr".to_string(), "oops".to_string()),
            ]
        );
    }
}
