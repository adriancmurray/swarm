//! Single-agent subprocess execution and capture.

use std::env;
use std::fs::{remove_file, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use swarm_kernel::agent::{agent_name, describe_spec, AgentChoice, AgentSpec};
use swarm_kernel::args::Args;
use swarm_kernel::backend_abi::{
    BackendCaps, BackendError, BackendRequest, BackendSink, ClosureSink, EnvPolicy, NullSink,
    RunOutcome, TokenUsage,
};
use swarm_kernel::config::ReliabilityConfig;
use swarm_kernel::job_types::JobStatus;
use swarm_kernel::resolver::{locate_claude, locate_codex};
use swarm_kernel::routing::NextAction;
use swarm_kernel::telemetry;
use swarm_store::repos::telemetry_repo::{self, FileTelemetryRepo, TelemetryRepo};
use swarm_store::store::now_ms;

use crate::backend_registry::BackendRegistry;
use crate::preflight::classify_error;

const MAX_CAPTURE_RESULT_BYTES: usize = 2 * 1024 * 1024;
const MAX_CAPTURE_CHUNK_BYTES: usize = 96 * 1024;

/// Returns a `FileTelemetryRepo` pointed at the default telemetry directory
/// (`<swarm home>/telemetry`, see `swarm_store::store::swarm_home`), or `None`
/// if neither `SWARM_HOME` nor `HOME` is set.
///
/// Delegates to `telemetry_repo::default_file_telemetry_repo()` — the single
/// canonical path resolver. Do NOT inline the path here.
fn default_telemetry_repo() -> Option<FileTelemetryRepo> {
    telemetry_repo::default_file_telemetry_repo()
}

// ---------------------------------------------------------------------------
// AgentBackend trait — the process-agnostic execution contract (spec §4.3)
//
// A backend reports a stable `id`, gates on `ready()` (binary located / key
// present), and runs a single attempt via `run(req, sink)`: it builds from the
// borrowed `BackendRequest` (prompt/model/cwd/timeout/quiet/bypass) and streams
// output through `sink.stdout_chunk` / `sink.stderr_chunk`. Failures are typed
// `BackendError` so the retry/fallback machine branches on cause, never on
// error text.
//
// PRESERVE EXACTLY: each backend's subprocess args, env, the claude
// `--output-format json` non-streaming token-capture path, and the streaming
// path are byte-identical to before. The usable-output predicate and the
// retry/fallback machine are UNTOUCHED.
// ---------------------------------------------------------------------------

/// Per-backend single-attempt execution (spec §4.3).
pub trait AgentBackend: Send + Sync {
    /// Stable backend id (e.g. `"gemini"`).
    fn id(&self) -> &str;
    /// Whether the backend can run now: binary located, key present, etc.
    /// Returns [`BackendError::NotReady`] with actionable detail on a miss.
    fn ready(&self) -> Result<(), BackendError>;
    /// Run a single attempt, streaming output through `sink`.
    fn run(
        &self,
        req: &BackendRequest,
        sink: &mut dyn BackendSink,
    ) -> Result<RunOutcome, BackendError>;
    /// What this backend can do.
    fn capabilities(&self) -> BackendCaps;
}

/// Codex backend — dispatches via `codex exec`.
pub struct CodexBackend;

impl AgentBackend for CodexBackend {
    fn id(&self) -> &str {
        "codex"
    }

    fn ready(&self) -> Result<(), BackendError> {
        locate_codex().map(|_| ()).ok_or_else(|| {
            BackendError::NotReady(
                "could not locate the `codex` binary in PATH or default locations. \
                 Install Codex CLI with `npm install -g @openai/codex` or another supported installer."
                    .to_string(),
            )
        })
    }

    fn run(
        &self,
        req: &BackendRequest,
        sink: &mut dyn BackendSink,
    ) -> Result<RunOutcome, BackendError> {
        let codex = locate_codex().ok_or_else(|| {
            BackendError::NotReady("could not locate the `codex` binary".to_string())
        })?;
        run_codex(&codex, req, sink).map_err(BackendError::Spawn)
    }

    fn capabilities(&self) -> BackendCaps {
        BackendCaps::default()
    }
}

/// Claude backend — dispatches via `claude --print`.
pub struct ClaudeBackend;

impl AgentBackend for ClaudeBackend {
    fn id(&self) -> &str {
        "claude"
    }

    fn ready(&self) -> Result<(), BackendError> {
        locate_claude().map(|_| ()).ok_or_else(|| {
            BackendError::NotReady(
                "could not locate the `claude` binary in PATH or default locations. \
                 Install Claude Code and authenticate it before selecting --agent claude."
                    .to_string(),
            )
        })
    }

    fn run(
        &self,
        req: &BackendRequest,
        sink: &mut dyn BackendSink,
    ) -> Result<RunOutcome, BackendError> {
        let claude = locate_claude().ok_or_else(|| {
            BackendError::NotReady("could not locate the `claude` binary".to_string())
        })?;
        run_claude(&claude, req, sink).map_err(BackendError::Spawn)
    }

    fn capabilities(&self) -> BackendCaps {
        BackendCaps::default()
    }
}

/// The dispatch id for one execution: the config-defined backend id when the
/// custom lane is set, else the built-in's canonical name. `"auto"` only
/// reaches the registry if a caller skipped `resolve_agent` — and then fails
/// with the registry's clear unknown-id error instead of a panic.
fn dispatch_id(agent: AgentChoice, args: &Args) -> &str {
    args.agent_custom
        .as_deref()
        .unwrap_or_else(|| agent_name(agent))
}

/// Build a borrowed [`BackendRequest`] from `Args` + `prompt`. The timeout is
/// the raw configured value; backends add their own grace margin internally.
pub(crate) fn request_from_args<'a>(args: &'a Args, prompt: &'a str) -> BackendRequest<'a> {
    BackendRequest {
        prompt,
        model: args
            .model
            .as_deref()
            .filter(|model| !model.trim().is_empty()),
        cwd: &args.cwd,
        timeout: Duration::from_secs(args.timeout_secs),
        quiet: args.quiet,
        allow_bypass_permissions: args.allow_bypass_permissions,
        env_policy: EnvPolicy::Inherit,
        cancel: swarm_kernel::backend_abi::CancelToken::new(),
    }
}

/// Run a resolved backend once, bridging the typed `BackendError` back to the
/// `String` error the fallback machine and call sites still consume.
fn run_backend(
    backend: &dyn AgentBackend,
    req: &BackendRequest,
    sink: &mut dyn BackendSink,
) -> Result<RunOutcome, String> {
    backend.ready().map_err(|e| format!("Error: {e}"))?;
    backend.run(req, sink).map_err(|e| format!("Error: {e}"))
}

pub fn execute_partner(
    registry: &BackendRegistry,
    agent: AgentChoice,
    args: &Args,
    prompt: &str,
) -> Result<RunOutcome, String> {
    execute_partner_with_chunks(registry, agent, args, prompt, None)
}

/// Streaming chunk callback `(stream_name, chunk)` — the closure form fed to
/// [`ClosureSink`].
pub type ChunkCallback<'a> = &'a mut dyn FnMut(&str, &str);

pub fn execute_partner_with_chunks(
    registry: &BackendRegistry,
    agent: AgentChoice,
    args: &Args,
    prompt: &str,
    on_chunk: Option<ChunkCallback<'_>>,
) -> Result<RunOutcome, String> {
    let backend = registry.resolve(dispatch_id(agent, args))?;
    let req = request_from_args(args, prompt);
    match on_chunk {
        Some(cb) => {
            let mut sink = ClosureSink::new(cb);
            run_backend(backend, &req, &mut sink)
        }
        None => {
            let mut sink = NullSink;
            run_backend(backend, &req, &mut sink)
        }
    }
}

/// One attempt within a fallback chain, recorded for visible event emission.
#[derive(Debug)]
pub struct FallbackAttempt {
    pub spec: AgentSpec,
    /// Retries spent on this backend before resolving (0 = first try settled it).
    pub retries: u32,
    pub succeeded: bool,
    /// Brief reason this attempt failed, when it did not produce usable output.
    pub reason: Option<String>,
}

/// The result of walking a backend fallback chain: the backend whose output is
/// returned, that output, and the full attempt trail (for `worker_fallback`
/// events).
#[derive(Debug)]
pub struct FallbackOutcome {
    pub used: AgentSpec,
    pub result: Result<RunOutcome, String>,
    pub attempts: Vec<FallbackAttempt>,
}

impl FallbackOutcome {
    /// True if a backend other than the chain's primary ultimately ran.
    pub fn fell_back(&self) -> bool {
        self.attempts.len() > 1
    }
}

/// Non-chunked fallback execution: runs `prompt` against the chain, retrying
/// and falling back. Thin wrapper over the shared [`run_fallback_loop`].
pub fn execute_with_fallback(
    registry: &BackendRegistry,
    chain: &[AgentSpec],
    base_args: &Args,
    prompt: &str,
    role: &str,
    reliability: &ReliabilityConfig,
) -> FallbackOutcome {
    run_fallback_loop(chain, base_args, role, reliability, |call_args| {
        let agent = resolve_unless_custom(call_args)?;
        execute_partner(registry, agent, call_args, prompt)
    })
}

/// Built-in specs go through `resolve_agent` (Auto → an installed built-in);
/// custom specs skip it entirely — their `agent` field is a placeholder, and
/// resolving it would consult built-in availability that is irrelevant (and
/// possibly absent) when dispatch goes to a config-defined backend.
fn resolve_unless_custom(call_args: &Args) -> Result<AgentChoice, String> {
    if call_args.agent_custom.is_some() {
        Ok(call_args.agent)
    } else {
        swarm_kernel::resolver::resolve_agent(call_args.agent)
    }
}

/// Chunked fallback execution for live-streaming discussion turns. Identical
/// fallback semantics to [`execute_with_fallback`], but each attempt streams via
/// `on_chunk`. Chunks from a failed attempt may reach the UI before a
/// `worker_fallback` event and the next backend's chunks — acceptable, since a
/// failed attempt produces little or no stdout.
pub fn execute_with_fallback_chunks(
    registry: &BackendRegistry,
    chain: &[AgentSpec],
    base_args: &Args,
    prompt: &str,
    role: &str,
    reliability: &ReliabilityConfig,
    on_chunk: &mut dyn FnMut(&str, &str),
) -> FallbackOutcome {
    run_fallback_loop(chain, base_args, role, reliability, |call_args| {
        let agent = resolve_unless_custom(call_args)?;
        execute_partner_with_chunks(registry, agent, call_args, prompt, Some(&mut *on_chunk))
    })
}

/// Shared retry → fallback driver. Walks `chain`, building per-spec `call_args`
/// and running `exec` for each attempt, consulting the pure
/// [`swarm_kernel::routing::next_action`] state machine after each. The only impure
/// parts are `exec` (spawning) and the backoff sleep.
///
/// "Usable output" = the agent ran, did not time out, exited 0, and produced
/// non-empty stdout. A missing binary, a timeout, OR any non-zero exit (even one
/// that printed an error to stdout, e.g. claude's `"selected model may not
/// exist"` or the `"model not supported on this account"` blip that motivated
/// this) all count as failure and advance the chain.
fn run_fallback_loop<F>(
    chain: &[AgentSpec],
    base_args: &Args,
    role: &str,
    reliability: &ReliabilityConfig,
    mut exec: F,
) -> FallbackOutcome
where
    F: FnMut(&Args) -> Result<RunOutcome, String>,
{
    let mut attempts = Vec::new();
    for (position, spec) in chain.iter().enumerate() {
        let mut retries_used = 0;
        loop {
            let mut call_args = base_args.clone();
            call_args.agent = spec.agent;
            call_args.agent_custom = spec.custom.clone();
            call_args.model = spec.model.clone();
            let result = exec(&call_args);
            let succeeded = matches!(&result, Ok(output) if produced_usable_output(output));

            match swarm_kernel::routing::next_action(
                succeeded,
                retries_used,
                reliability.retry_attempts,
                position,
                chain.len(),
                reliability.retry_backoff_ms,
                role,
            ) {
                NextAction::Done => {
                    attempts.push(FallbackAttempt {
                        spec: spec.clone(),
                        retries: retries_used,
                        succeeded,
                        reason: (!succeeded).then(|| failure_reason(&result)),
                    });
                    return FallbackOutcome {
                        used: spec.clone(),
                        result,
                        attempts,
                    };
                }
                NextAction::RetrySame { backoff_ms } => {
                    thread::sleep(Duration::from_millis(backoff_ms));
                    retries_used += 1;
                }
                NextAction::FallbackNext => {
                    attempts.push(FallbackAttempt {
                        spec: spec.clone(),
                        retries: retries_used,
                        succeeded: false,
                        reason: Some(failure_reason(&result)),
                    });
                    break;
                }
            }
        }
    }
    // `build_fallback_chain` always yields >= 1 spec, so the loop returns via
    // `Done` on the final backend; this is an unreachable defensive sentinel.
    FallbackOutcome {
        used: AgentSpec {
            agent: base_args.agent,
            model: base_args.model.clone(),
            custom: base_args.agent_custom.clone(),
        },
        result: Err("Error: empty backend fallback chain".to_string()),
        attempts,
    }
}

fn produced_usable_output(output: &RunOutcome) -> bool {
    // A non-zero exit is a failure regardless of stdout: CLIs print their error
    // text to stdout (e.g. claude's "selected model may not exist") and still
    // exit non-zero, so "ran and printed something" is not success. Usable = a
    // clean exit (0), not timed out, that actually produced content.
    !output.timed_out && output_status_code(output) == 0 && !output.stdout.trim().is_empty()
}

fn failure_reason(result: &Result<RunOutcome, String>) -> String {
    match result {
        Err(err) => brief_reason(err),
        Ok(output) if output.timed_out => "timed out".to_string(),
        Ok(output) => {
            let code = output_status_code(output);
            // Error text may land on stderr (typical) or stdout (claude prints
            // model errors there); only "no output" if both are empty.
            let detail = if !output.stderr.trim().is_empty() {
                brief_reason(output.stderr.trim())
            } else if !output.stdout.trim().is_empty() {
                brief_reason(output.stdout.trim())
            } else {
                "no output".to_string()
            };
            format!("exit {code}: {detail}")
        }
    }
}

fn brief_reason(text: &str) -> String {
    let cleaned = text.trim().replace('\n', " ");
    if cleaned.chars().count() > 160 {
        let truncated: String = cleaned.chars().take(157).collect();
        format!("{truncated}...")
    } else {
        cleaned
    }
}

pub fn print_partner_output(
    agent: AgentChoice,
    args: &Args,
    output: RunOutcome,
) -> Result<i32, String> {
    print!("{stdout}", stdout = output.stdout);
    let code = output.exit_status.unwrap_or(1);
    let suppress_success_stderr =
        agent == AgentChoice::Codex && code == 0 && !output.stdout.trim().is_empty();
    if !output.stderr.is_empty() && !suppress_success_stderr {
        eprint!("{stderr}", stderr = output.stderr);
    }

    if output.timed_out {
        if !output
            .stdout
            .contains("Error: timed out waiting for response")
        {
            eprintln!(
                "Error: `{}` did not return within {}s (configured timeout was {}s). The process was killed.",
                agent.command_name(),
                args.timeout_secs + 30,
                args.timeout_secs
            );
        }
        return Ok(124);
    }

    Ok(code)
}

pub fn output_status_code(output: &RunOutcome) -> i32 {
    if output.timed_out {
        124
    } else {
        output.exit_status.unwrap_or(1)
    }
}

pub fn output_record_status(output: &RunOutcome, code: i32) -> JobStatus {
    if output.timed_out {
        JobStatus::TimedOut
    } else if code == 0 {
        JobStatus::Completed
    } else {
        JobStatus::Failed
    }
}

// Args mirror the flat `AgentObservation` telemetry schema field-for-field;
// a params struct would just restate that schema at every call site.
#[allow(clippy::too_many_arguments)]
pub fn record_agent_observation(
    mode: &str,
    session_id: Option<&str>,
    role: &str,
    spec: &AgentSpec,
    cwd: &Path,
    prompt: &str,
    output: &RunOutcome,
    duration: Duration,
) {
    let code = output_status_code(output);
    if let Some(repo) = default_telemetry_repo() {
        let _ = repo.record_observation(telemetry::AgentObservation {
            schema: "agent-swarm/observation/v1".to_string(),
            ts_ms: now_ms(),
            mode: mode.to_string(),
            session_id: session_id.map(ToString::to_string),
            role: role.to_string(),
            agent: describe_spec(spec),
            cwd: cwd.display().to_string(),
            status: output_record_status(output, code).as_str().to_string(),
            exit_code: code,
            timed_out: output.timed_out,
            duration_ms: duration.as_millis(),
            prompt_bytes: prompt.len(),
            stdout_bytes: output.stdout.len(),
            stderr_bytes: output.stderr.len(),
            input_tokens: output.token_usage.as_ref().and_then(|u| u.input),
            output_tokens: output.token_usage.as_ref().and_then(|u| u.output),
        });
    }
}

// Same rationale as `record_agent_observation`: args mirror the telemetry schema.
#[allow(clippy::too_many_arguments)]
pub fn record_agent_error(
    mode: &str,
    session_id: Option<&str>,
    role: &str,
    spec: &AgentSpec,
    cwd: &Path,
    prompt: &str,
    error: &str,
    duration: Duration,
) {
    if let Some(repo) = default_telemetry_repo() {
        let _ = repo.record_observation(telemetry::AgentObservation {
            schema: "agent-swarm/observation/v1".to_string(),
            ts_ms: now_ms(),
            mode: mode.to_string(),
            session_id: session_id.map(ToString::to_string),
            role: role.to_string(),
            agent: describe_spec(spec),
            cwd: cwd.display().to_string(),
            status: "failed".to_string(),
            exit_code: 1,
            timed_out: classify_error(error) == "timeout",
            duration_ms: duration.as_millis(),
            prompt_bytes: prompt.len(),
            stdout_bytes: 0,
            stderr_bytes: error.len(),
            input_tokens: None,
            output_tokens: None,
        });
    }
}

/// Build a [`RunOutcome`] from a subprocess `ExitStatus`, mapping the status to
/// `exit_status: Option<i32>` (signal-killed processes yield `None`, treated as
/// a non-zero/failed exit downstream) and computing `retryable` from the prior
/// usable-output heuristic: a run is worth retrying when it timed out, did not
/// exit cleanly (0), or produced no stdout content. Tokens fold into
/// `token_usage` only when at least one count is present.
fn outcome_from_status(
    status: Option<ExitStatus>,
    stdout: String,
    stderr: String,
    timed_out: bool,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
) -> RunOutcome {
    let exit_status = status.and_then(|s| s.code());
    let token_usage = match (input_tokens, output_tokens) {
        (None, None) => None,
        (input, output) => Some(TokenUsage { input, output }),
    };
    let usable = !timed_out && exit_status == Some(0) && !stdout.trim().is_empty();
    RunOutcome {
        exit_status,
        stdout,
        stderr,
        timed_out,
        retryable: !usable,
        token_usage,
    }
}

fn run_codex(
    codex: &Path,
    req: &BackendRequest,
    sink: &mut dyn BackendSink,
) -> Result<RunOutcome, String> {
    let mut stdout_capture = CaptureFile::new("stdout")?;
    let mut stderr_capture = CaptureFile::new("stderr")?;
    let mut last_message_capture = CaptureFile::new("codex-last-message")?;

    let sandbox = if req.quiet {
        "read-only"
    } else {
        "workspace-write"
    };

    let mut command = Command::new(codex);
    command
        .arg("exec")
        .arg("--cd")
        .arg(req.cwd)
        .arg("--sandbox")
        .arg(sandbox)
        .arg("--color")
        .arg("never")
        .arg("--output-last-message")
        .arg(&last_message_capture.path)
        .arg("--skip-git-repo-check");

    // Honor an explicit `codex:MODEL` spec (e.g. gpt-5.3). Without this the
    // model parsed onto the AgentSpec is silently dropped and codex falls back
    // to its config default. Mirror run_claude. The model flag must precede the
    // trailing `-` stdin marker or codex treats it as a positional.
    if let Some(model) = req.model {
        command.arg("--model").arg(model);
    }

    command
        .arg("-")
        .current_dir(req.cwd)
        .stdin(Stdio::piped())
        .stdout(stdout_capture.stdio()?)
        .stderr(stderr_capture.stdio()?);

    configure_agent_command(&mut command);
    let mut child = command
        .spawn()
        .map_err(|err| format!("Error executing codex: {err}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(req.prompt.as_bytes())
            .map_err(|err| format!("Error writing prompt to codex stdin: {err}"))?;
    }

    let mut output = wait_for_child(
        &mut child,
        req.timeout.as_secs() + 30,
        &mut stdout_capture,
        &mut stderr_capture,
        sink,
    )?;
    let last_message = last_message_capture.read_to_string()?;
    if !last_message.trim().is_empty() {
        output.stdout = last_message;
        // stdout was replaced after construction; re-derive the informational
        // retryable flag from the final fields so it matches the decision path.
        output.retryable = !produced_usable_output(&output);
    }
    Ok(output)
}

/// Parse the JSON envelope that `claude --print --output-format json` emits.
///
/// Expected shape: `{"type":"result","result":"<text>","usage":{"input_tokens":N,"output_tokens":M,...},...}`
///
/// Returns `(text, input_tokens, output_tokens)`.  On any parse failure the
/// raw string is returned as text and tokens are `None` — this ensures that
/// error stdout (e.g. "selected model may not exist") is preserved for
/// `failure_reason` when JSON parsing fails.
pub fn parse_claude_json_output(raw: &str) -> (String, Option<u64>, Option<u64>) {
    let Ok(val) = serde_json::from_str::<serde_json::Value>(raw.trim()) else {
        return (raw.to_string(), None, None);
    };
    let text = match val.get("result").and_then(|v| v.as_str()) {
        Some(t) => t.to_string(),
        None => return (raw.to_string(), None, None),
    };
    let input_tokens = val
        .get("usage")
        .and_then(|u| u.get("input_tokens"))
        .and_then(|v| v.as_u64());
    let output_tokens = val
        .get("usage")
        .and_then(|u| u.get("output_tokens"))
        .and_then(|v| v.as_u64());
    (text, input_tokens, output_tokens)
}

fn run_claude(
    claude: &Path,
    req: &BackendRequest,
    sink: &mut dyn BackendSink,
) -> Result<RunOutcome, String> {
    let mut stdout_capture = CaptureFile::new("stdout")?;
    let mut stderr_capture = CaptureFile::new("stderr")?;

    // One-shot path (sink does not want streaming, e.g. NullSink): use JSON
    // output format so we can extract token counts. Streaming path: keep text
    // format — chunk consumers receive raw stdout bytes and must not see a JSON
    // envelope.
    let use_json = !sink.wants_streaming();

    let mut command = Command::new(claude);
    command
        .arg("--print")
        .arg("--output-format")
        .arg(if use_json { "json" } else { "text" })
        .arg("--add-dir")
        .arg(req.cwd)
        .current_dir(req.cwd)
        .stdin(Stdio::piped())
        .stdout(stdout_capture.stdio()?)
        .stderr(stderr_capture.stdio()?);

    if let Some(model) = req.model {
        command.arg("--model").arg(model);
    }

    if req.quiet {
        command.arg("--tools").arg("Read,Grep,Glob,LS");
    } else if req.allow_bypass_permissions {
        command.arg("--permission-mode").arg("bypassPermissions");
    }

    configure_agent_command(&mut command);
    let mut child = command
        .spawn()
        .map_err(|err| format!("Error executing claude: {err}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(req.prompt.as_bytes())
            .map_err(|err| format!("Error writing prompt to claude stdin: {err}"))?;
    }

    let mut output = wait_for_child(
        &mut child,
        req.timeout.as_secs() + 30,
        &mut stdout_capture,
        &mut stderr_capture,
        sink,
    )?;

    if use_json {
        let (text, input_tokens, output_tokens) = parse_claude_json_output(&output.stdout);
        output.stdout = text;
        output.token_usage = match (input_tokens, output_tokens) {
            (None, None) => None,
            (input, output) => Some(TokenUsage { input, output }),
        };
        // The JSON envelope was unwrapped into plain text after construction;
        // re-derive the informational retryable flag from the final fields.
        output.retryable = !produced_usable_output(&output);
    }

    Ok(output)
}

pub(crate) fn wait_for_child(
    child: &mut std::process::Child,
    timeout_secs: u64,
    stdout_capture: &mut CaptureFile,
    stderr_capture: &mut CaptureFile,
    sink: &mut dyn BackendSink,
) -> Result<RunOutcome, String> {
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    let mut status = None;
    let mut timed_out = false;
    let mut stdout_offset = 0usize;
    let mut stderr_offset = 0usize;
    let mut last_chunk_check = Instant::now();
    loop {
        match child
            .try_wait()
            .map_err(|err| format!("Error waiting for child process: {err}"))?
        {
            Some(done) => {
                status = Some(done);
                break;
            }
            None if Instant::now() >= deadline => {
                timed_out = true;
                terminate_child(child);
                let grace = Instant::now() + Duration::from_secs(5);
                while Instant::now() < grace {
                    if let Some(done) = child
                        .try_wait()
                        .map_err(|err| format!("Error waiting after terminate: {err}"))?
                    {
                        status = Some(done);
                        break;
                    }
                    thread::sleep(Duration::from_millis(100));
                }
                if status.is_none() {
                    child.kill().map_err(|err| {
                        format!("Error killing child process after timeout: {err}")
                    })?;
                    status = child.wait().ok();
                }
                break;
            }
            None => thread::sleep(Duration::from_millis(100)),
        }
        if last_chunk_check.elapsed() >= Duration::from_millis(500) {
            emit_capture_chunks(&stdout_capture.path, "stdout", &mut stdout_offset, sink);
            emit_capture_chunks(&stderr_capture.path, "stderr", &mut stderr_offset, sink);
            last_chunk_check = Instant::now();
        }
    }
    emit_capture_chunks(&stdout_capture.path, "stdout", &mut stdout_offset, sink);
    emit_capture_chunks(&stderr_capture.path, "stderr", &mut stderr_offset, sink);

    Ok(outcome_from_status(
        status,
        stdout_capture.read_to_string()?,
        stderr_capture.read_to_string()?,
        timed_out,
        None,
        None,
    ))
}

fn emit_capture_chunks(path: &Path, stream: &str, offset: &mut usize, sink: &mut dyn BackendSink) {
    // No streaming wanted (e.g. NullSink) — skip the capture-file read entirely,
    // matching the old `on_chunk.is_none()` early-return.
    if !sink.wants_streaming() {
        return;
    }
    let Ok(mut file) = File::open(path) else {
        return;
    };
    let Ok(metadata) = file.metadata() else {
        return;
    };
    let len = metadata.len() as usize;
    if len <= *offset {
        return;
    }
    let available = len - *offset;
    let mut skipped = 0usize;
    if available > MAX_CAPTURE_CHUNK_BYTES {
        skipped = available - MAX_CAPTURE_CHUNK_BYTES;
        *offset += skipped;
    }
    if file.seek(SeekFrom::Start(*offset as u64)).is_err() {
        return;
    }
    let mut bytes = vec![0; (len - *offset).min(MAX_CAPTURE_CHUNK_BYTES)];
    let Ok(read) = file.read(&mut bytes) else {
        return;
    };
    bytes.truncate(read);
    *offset += read;
    let mut text = String::new();
    if skipped > 0 {
        text.push_str(&format!(
            "[agent-swarm: skipped {skipped} buffered {stream} bytes]\n"
        ));
    }
    text.push_str(&String::from_utf8_lossy(&bytes));
    if text.trim().is_empty() {
        return;
    }
    match stream {
        "stderr" => sink.stderr_chunk(&text),
        _ => sink.stdout_chunk(&text),
    }
}

#[cfg(unix)]
pub(crate) fn configure_agent_command(command: &mut Command) {
    use std::os::unix::process::CommandExt;

    command.env("SWARM_DEPTH", (current_swarm_depth() + 1).to_string());
    command.process_group(0);
}

#[cfg(not(unix))]
pub(crate) fn configure_agent_command(command: &mut Command) {
    command.env("SWARM_DEPTH", (current_swarm_depth() + 1).to_string());
}

pub fn current_swarm_depth() -> u32 {
    env::var("SWARM_DEPTH")
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(0)
}

#[cfg(unix)]
fn terminate_child(child: &mut std::process::Child) {
    let pgid = format!("-{}", child.id());
    let status = Command::new("/bin/kill").arg("-TERM").arg(pgid).status();
    if !matches!(status, Ok(done) if done.success()) {
        let _ = Command::new("/bin/kill")
            .arg("-TERM")
            .arg(child.id().to_string())
            .status();
    }
}

#[cfg(not(unix))]
fn terminate_child(child: &mut std::process::Child) {
    let _ = child.kill();
}

pub(crate) struct CaptureFile {
    pub(crate) path: PathBuf,
    pub(crate) file: File,
}

impl CaptureFile {
    pub(crate) fn new(label: &str) -> Result<Self, String> {
        let dir = env::temp_dir();
        let pid = std::process::id();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_else(|_| Duration::from_secs(0))
            .as_nanos();

        for attempt in 0..100 {
            let path = dir.join(format!("swarm-partner-{pid}-{now}-{label}-{attempt}.log"));
            match OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(file) => return Ok(Self { path, file }),
                Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(err) => {
                    return Err(format!(
                        "Error creating temporary {label} capture file: {err}"
                    ))
                }
            }
        }

        Err(format!(
            "Error creating temporary {label} capture file: exhausted name attempts"
        ))
    }

    pub(crate) fn stdio(&self) -> Result<Stdio, String> {
        self.file
            .try_clone()
            .map(Stdio::from)
            .map_err(|err| format!("Error cloning capture file descriptor: {err}"))
    }

    fn read_to_string(&mut self) -> Result<String, String> {
        let len = self
            .file
            .metadata()
            .map_err(|err| format!("Error reading capture file metadata: {err}"))?
            .len() as usize;
        let start = len.saturating_sub(MAX_CAPTURE_RESULT_BYTES);
        self.file
            .seek(SeekFrom::Start(start as u64))
            .map_err(|err| format!("Error seeking capture file: {err}"))?;
        let mut text = String::new();
        if start > 0 {
            text.push_str(&format!(
                "[agent-swarm: output truncated to last {} bytes; skipped {} bytes]\n",
                MAX_CAPTURE_RESULT_BYTES, start
            ));
        }
        self.file
            .read_to_string(&mut text)
            .map_err(|err| format!("Error reading capture file: {err}"))?;
        Ok(text)
    }
}

impl Drop for CaptureFile {
    fn drop(&mut self) {
        let _ = remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use swarm_kernel::job_types::JobStatus;

    /// Happy path: well-formed claude JSON envelope → text + token counts extracted.
    #[test]
    fn parse_claude_json_output_extracts_tokens() {
        let raw = r#"{"type":"result","subtype":"success","is_error":false,"result":"Hello, world!","usage":{"input_tokens":42,"output_tokens":7,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}}"#;
        let (text, input, output) = parse_claude_json_output(raw);
        assert_eq!(text, "Hello, world!");
        assert_eq!(input, Some(42));
        assert_eq!(output, Some(7));
    }

    /// Error path: non-JSON stdout (e.g. error message) falls back to raw text,
    /// tokens are None.  This preserves `failure_reason` accuracy.
    #[test]
    fn parse_claude_json_output_falls_back_on_error_text() {
        let raw = "selected model may not exist: claude-fake-model";
        let (text, input, output) = parse_claude_json_output(raw);
        assert_eq!(text, raw);
        assert!(input.is_none());
        assert!(output.is_none());
    }

    fn output(status: Option<ExitStatus>, timed_out: bool) -> RunOutcome {
        outcome_from_status(status, String::new(), String::new(), timed_out, None, None)
    }

    #[test]
    fn output_status_code_prefers_timeout() {
        assert_eq!(output_status_code(&output(None, true)), 124);
        assert_eq!(output_status_code(&output(None, false)), 1);
    }

    #[test]
    fn output_record_status_maps_timeout_success_and_failure() {
        assert_eq!(
            output_record_status(&output(None, true), 0),
            JobStatus::TimedOut
        );
        assert_eq!(
            output_record_status(&output(None, false), 0),
            JobStatus::Completed
        );
        assert_eq!(
            output_record_status(&output(None, false), 1),
            JobStatus::Failed
        );
    }

    #[test]
    fn usable_output_requires_clean_exit_and_content() {
        let clean = Command::new("true").status().unwrap();
        let failed = Command::new("false").status().unwrap();
        let with = |status, stdout: &str, timed_out| {
            outcome_from_status(
                Some(status),
                stdout.to_string(),
                String::new(),
                timed_out,
                None,
                None,
            )
        };
        // A clean exit (0) with content is the only "usable" case.
        assert!(produced_usable_output(&with(clean, "real result", false)));
        // Regression (smoke 2026-06-01): a non-zero exit is failure EVEN with
        // stdout — claude prints "selected model may not exist" to stdout + exits
        // 1, which must trigger fallback, not pass as success.
        assert!(!produced_usable_output(&with(
            failed,
            "error: model not found",
            false
        )));
        // A clean exit with empty output is not usable.
        assert!(!produced_usable_output(&with(clean, "   ", false)));
        // A timeout is not usable regardless of partial content.
        assert!(!produced_usable_output(&with(clean, "partial", true)));
    }

    #[test]
    fn emit_capture_chunks_tracks_offsets() {
        let mut capture = CaptureFile::new("chunk-offset").expect("create capture");
        capture.file.write_all(b"first").expect("write first");
        capture.file.flush().expect("flush first");

        let mut chunks = Vec::new();
        let mut offset = 0usize;
        let mut callback = |stream: &str, text: &str| {
            chunks.push((stream.to_string(), text.to_string()));
        };
        {
            let mut sink = ClosureSink::new(&mut callback);
            emit_capture_chunks(&capture.path, "stdout", &mut offset, &mut sink);
            assert_eq!(offset, 5);

            capture.file.write_all(b" second").expect("write second");
            capture.file.flush().expect("flush second");
            emit_capture_chunks(&capture.path, "stdout", &mut offset, &mut sink);
        }

        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0], ("stdout".to_string(), "first".to_string()));
        assert_eq!(chunks[1], ("stdout".to_string(), " second".to_string()));
    }

    #[test]
    fn emit_capture_chunks_reports_skipped_buffer_bytes() {
        let mut capture = CaptureFile::new("chunk-skip").expect("create capture");
        capture
            .file
            .write_all(&vec![b'x'; MAX_CAPTURE_CHUNK_BYTES + 10])
            .expect("write large capture");
        capture.file.flush().expect("flush large capture");

        let mut chunks = Vec::new();
        let mut offset = 0usize;
        let mut callback = |stream: &str, text: &str| {
            chunks.push((stream.to_string(), text.to_string()));
        };
        {
            let mut sink = ClosureSink::new(&mut callback);
            emit_capture_chunks(&capture.path, "stderr", &mut offset, &mut sink);
        }

        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].0 == "stderr");
        assert!(chunks[0]
            .1
            .starts_with("[agent-swarm: skipped 10 buffered stderr bytes]"));
        assert_eq!(offset, MAX_CAPTURE_CHUNK_BYTES + 10);
    }

    #[test]
    fn capture_file_names_are_unique_and_live_until_drop() {
        let captures = (0..10)
            .map(|_| CaptureFile::new("unique").expect("create capture"))
            .collect::<Vec<_>>();
        let paths = captures
            .iter()
            .map(|capture| capture.path.clone())
            .collect::<HashSet<_>>();

        assert_eq!(paths.len(), captures.len());
        for path in paths {
            assert!(path.exists());
        }
    }

    fn custom_args(id: &str) -> Args {
        Args {
            prompt: String::new(),
            cwd: std::env::temp_dir(),
            timeout_secs: 30,
            quiet: true,
            agent: AgentChoice::Auto,
            agent_custom: Some(id.to_string()),
            model: None,
            persona: None,
            background: false,
            allow_bypass_permissions: false,
        }
    }

    /// End-to-end §4.4 proof: a `[backend.<id>]` config descriptor is runnable
    /// through the real dispatch path (config → registry → execute_partner),
    /// with no enum variant or engine code naming it.
    #[test]
    fn config_descriptor_dispatches_through_registry() {
        let config: swarm_kernel::config::SwarmConfig = toml::from_str(
            r#"
            [backend.echo-test]
            kind = "cli"
            command = "printf"
            args = ["dispatched:%s", "{prompt}"]
            prompt = "arg"
            "#,
        )
        .unwrap();
        let registry = BackendRegistry::from_config(&config);
        let args = custom_args("echo-test");
        let output =
            execute_partner(&registry, AgentChoice::Auto, &args, "hello-registry").unwrap();
        assert!(
            output.stdout.contains("dispatched:hello-registry"),
            "got: {:?}",
            output.stdout
        );
    }

    #[test]
    fn unknown_custom_backend_errors_listing_available() {
        let registry = BackendRegistry::with_builtins();
        let args = custom_args("nope");
        let err = execute_partner(&registry, AgentChoice::Auto, &args, "x").unwrap_err();
        assert!(err.contains("nope") && err.contains("available"), "{err}");
    }
}
