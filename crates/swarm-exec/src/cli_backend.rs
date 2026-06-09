//! `CliBackend` runs a `BackendDescriptor { kind: cli }` as a subprocess,
//! reusing the same capture/stream/timeout path as the built-in CLI backends.

use std::io::Write;
use std::process::{Command, Stdio};

use swarm_kernel::backend_descriptor::{BackendDescriptor, PromptDelivery};

use crate::executor::{configure_agent_command, wait_for_child, AgentBackend, CaptureFile};
use swarm_kernel::backend_abi::{
    BackendCaps, BackendError, BackendRequest, BackendSink, RunOutcome,
};

/// A descriptor-driven CLI backend, resolvable by string id from the registry.
pub struct CliBackend {
    id: String,
    descriptor: BackendDescriptor,
}

impl CliBackend {
    pub fn new(id: impl Into<String>, descriptor: BackendDescriptor) -> Self {
        Self {
            id: id.into(),
            descriptor,
        }
    }

    // Single-pass substitution: scans `raw` once and never re-scans substituted content,
    // so a prompt containing literal "{model}" is never corrupted by the model substitution.
    fn render(&self, raw: &str, prompt: &str, model: &str, cwd: &str) -> String {
        let mut out = String::with_capacity(raw.len());
        let mut rest = raw;
        while let Some(idx) = rest.find('{') {
            out.push_str(&rest[..idx]);
            let tail = &rest[idx..];
            if let Some(stripped) = tail.strip_prefix("{prompt}") {
                out.push_str(prompt);
                rest = stripped;
            } else if let Some(stripped) = tail.strip_prefix("{model}") {
                out.push_str(model);
                rest = stripped;
            } else if let Some(stripped) = tail.strip_prefix("{cwd}") {
                out.push_str(cwd);
                rest = stripped;
            } else {
                out.push('{');
                rest = &tail['{'.len_utf8()..];
            }
        }
        out.push_str(rest);
        out
    }
}

impl AgentBackend for CliBackend {
    fn id(&self) -> &str {
        &self.id
    }

    fn ready(&self) -> Result<(), BackendError> {
        if self.descriptor.command.as_deref().is_some() {
            Ok(())
        } else {
            Err(BackendError::NotReady(format!(
                "backend `{}` has no `command` set in its descriptor.",
                self.id
            )))
        }
    }

    fn run(
        &self,
        req: &BackendRequest,
        sink: &mut dyn BackendSink,
    ) -> Result<RunOutcome, BackendError> {
        let command_name = self.descriptor.command.as_deref().ok_or_else(|| {
            BackendError::NotReady(format!(
                "backend `{}` has no `command` set in its descriptor.",
                self.id
            ))
        })?;

        // When no model is set, `{model}` substitutes to an empty string.
        // Descriptor authors should only use `{model}` for backends that accept a model arg.
        let model = req.model.unwrap_or("");
        let mut stdout_capture = CaptureFile::new("stdout").map_err(BackendError::Spawn)?;
        let mut stderr_capture = CaptureFile::new("stderr").map_err(BackendError::Spawn)?;

        let cwd = req.cwd.display().to_string();
        let mut command = Command::new(command_name);
        for raw in &self.descriptor.args {
            command.arg(self.render(raw, req.prompt, model, &cwd));
        }
        if matches!(self.descriptor.prompt, PromptDelivery::Arg)
            && !self.descriptor.args.iter().any(|a| a.contains("{prompt}"))
        {
            command.arg(req.prompt);
        }

        command
            .current_dir(req.cwd)
            .stdin(Stdio::piped())
            .stdout(stdout_capture.stdio().map_err(BackendError::Spawn)?)
            .stderr(stderr_capture.stdio().map_err(BackendError::Spawn)?);
        configure_agent_command(&mut command);

        let mut child = command.spawn().map_err(|err| {
            BackendError::Spawn(format!(
                "executing backend `{}` ({command_name}): {err}",
                self.id
            ))
        })?;

        if matches!(self.descriptor.prompt, PromptDelivery::Stdin) {
            if let Some(mut stdin) = child.stdin.take() {
                stdin.write_all(req.prompt.as_bytes()).map_err(|err| {
                    BackendError::Spawn(format!("writing prompt to `{}` stdin: {err}", self.id))
                })?;
            }
        }
        drop(child.stdin.take());

        wait_for_child(
            &mut child,
            req.timeout.as_secs() + 30,
            &mut stdout_capture,
            &mut stderr_capture,
            sink,
        )
        .map_err(BackendError::Spawn)
    }

    fn capabilities(&self) -> BackendCaps {
        BackendCaps::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::Duration;
    use swarm_kernel::backend_abi::{CancelToken, EnvPolicy, NullSink};
    use swarm_kernel::backend_descriptor::{BackendDescriptor, BackendKind, PromptDelivery};

    /// A `BackendRequest` over a temp cwd; `prompt` borrows the caller's str.
    fn req<'a>(prompt: &'a str, cwd: &'a PathBuf) -> BackendRequest<'a> {
        BackendRequest {
            prompt,
            model: None,
            cwd,
            timeout: Duration::from_secs(30),
            quiet: true,
            allow_bypass_permissions: false,
            env_policy: EnvPolicy::Inherit,
            cancel: CancelToken::new(),
        }
    }

    #[test]
    fn cli_backend_runs_command_and_captures_stdout() {
        let desc = BackendDescriptor {
            kind: BackendKind::Cli,
            command: Some("printf".to_string()),
            args: vec!["hello %s".to_string(), "{prompt}".to_string()],
            prompt: PromptDelivery::Arg,
            ..Default::default()
        };
        let backend = CliBackend::new("echo-test", desc);
        let cwd = std::env::temp_dir();
        let out = backend.run(&req("world", &cwd), &mut NullSink).unwrap();
        assert!(out.stdout.contains("hello world"), "got: {:?}", out.stdout);
    }

    #[test]
    fn cli_backend_missing_command_errors() {
        let desc = BackendDescriptor {
            kind: BackendKind::Cli,
            command: None,
            args: vec![],
            prompt: PromptDelivery::Stdin,
            ..Default::default()
        };
        let backend = CliBackend::new("broken", desc);
        let cwd = std::env::temp_dir();
        let err = backend
            .run(&req("x", &cwd), &mut NullSink)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("broken"),
            "error should name the backend id: {err}"
        );
    }

    #[test]
    fn render_does_not_resubstitute_tokens_from_prompt_text() {
        let backend = CliBackend::new(
            "t",
            BackendDescriptor {
                kind: BackendKind::Cli,
                command: Some("x".into()),
                args: vec![],
                prompt: PromptDelivery::Stdin,
                ..Default::default()
            },
        );
        // The prompt text itself contains a literal "{model}" which must survive verbatim.
        let out = backend.render(
            "p={prompt} m={model}",
            "explain {model} scaling",
            "gpt-5",
            "/work",
        );
        assert_eq!(out, "p=explain {model} scaling m=gpt-5");
    }

    #[test]
    fn render_substitutes_cwd_token() {
        let backend = CliBackend::new(
            "t",
            BackendDescriptor {
                kind: BackendKind::Cli,
                command: Some("x".into()),
                ..Default::default()
            },
        );
        let out = backend.render("--add-dir={cwd}", "p", "m", "/work/dir");
        assert_eq!(out, "--add-dir=/work/dir");
    }
}
