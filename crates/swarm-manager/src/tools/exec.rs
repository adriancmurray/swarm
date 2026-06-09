//! Shell execution tool. The command runs unwrapped (no sandbox) inside a
//! workspace root; output is scrubbed of secrets and truncated.

use super::output::{prepare_tool_output, DEFAULT_MAX_OUTPUT_CHARS};
use super::Tool;
use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;
use tokio::process::Command;

/// Shell execution tool.
pub struct ExecTool {
    timeout: Duration,
    workspace_root: PathBuf,
    max_output_chars: usize,
}

impl ExecTool {
    pub fn new(timeout_secs: u64) -> Self {
        let workspace_root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        Self {
            timeout: Duration::from_secs(timeout_secs),
            workspace_root,
            max_output_chars: DEFAULT_MAX_OUTPUT_CHARS,
        }
    }

    pub fn workspace(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            timeout: Duration::from_secs(30),
            workspace_root: workspace_root.into(),
            max_output_chars: DEFAULT_MAX_OUTPUT_CHARS,
        }
    }

    pub fn with_output_limit(mut self, max_output_chars: usize) -> Self {
        self.max_output_chars = max_output_chars;
        self
    }

    async fn run(&self, args: Value) -> Result<String> {
        let command = args["command"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing command parameter"))?;

        let workdir = self.resolve_workdir(args["workdir"].as_str())?;
        let timeout = args["timeout"]
            .as_u64()
            .map(Duration::from_secs)
            .unwrap_or(self.timeout);

        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(command);
        cmd.current_dir(&workdir);

        let output = tokio::time::timeout(timeout, cmd.output())
            .await
            .map_err(|_| {
                anyhow::anyhow!(
                    "Command timed out after {:?}. Retry with a narrower command or a smaller timeout-scoped test target.",
                    timeout
                )
            })?
            .with_context(|| {
                format!(
                    "Could not run command in `{}`. Retry with a workdir inside the execution workspace.",
                    workdir.display()
                )
            })?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        let mut result = String::new();

        if !stdout.is_empty() {
            result.push_str(&stdout);
        }

        if !stderr.is_empty() {
            if !result.is_empty() {
                result.push_str("\n--- stderr ---\n");
            }
            result.push_str(&stderr);
        }

        if !output.status.success() {
            result.push_str(&format!(
                "\n[exit code: {}]",
                output.status.code().unwrap_or(-1)
            ));
        }

        Ok(prepare_tool_output(&result, self.max_output_chars))
    }

    fn resolve_workdir(&self, raw_workdir: Option<&str>) -> Result<PathBuf> {
        let root = self
            .workspace_root
            .canonicalize()
            .unwrap_or_else(|_| normalize_lexical(&self.workspace_root));
        let requested = raw_workdir
            .map(PathBuf::from)
            .unwrap_or_else(|| root.clone());
        let resolved = if requested.is_absolute() {
            normalize_existing_or_lexical(&requested)
        } else {
            normalize_lexical(&root.join(&requested))
        };

        if !is_within_or_same(&resolved, &root) {
            anyhow::bail!(
                "workdir {} is outside execution workspace {}. Retry with a relative subdirectory inside the session workspace.",
                requested.display(),
                root.display()
            );
        }

        Ok(resolved)
    }
}

impl Default for ExecTool {
    fn default() -> Self {
        Self::new(30)
    }
}

#[async_trait]
impl Tool for ExecTool {
    fn name(&self) -> &str {
        "exec"
    }

    fn description(&self) -> &str {
        "Execute a shell command and return output"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "Shell command to execute"
                },
                "workdir": {
                    "type": "string",
                    "description": "Working directory"
                },
                "timeout": {
                    "type": "integer",
                    "description": "Timeout in seconds"
                }
            },
            "required": ["command"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        self.run(args).await
    }
}

fn normalize_existing_or_lexical(path: &Path) -> PathBuf {
    path.canonicalize()
        .unwrap_or_else(|_| normalize_lexical(path))
}

fn normalize_lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn is_within_or_same(path: &Path, root: &Path) -> bool {
    path == root || path.starts_with(root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::tempdir;

    #[tokio::test]
    async fn exec_runs_harmless_command() {
        let root = tempdir().unwrap();
        let tool = ExecTool::workspace(root.path());

        let output = tool
            .execute(json!({"command": "echo hello-world"}))
            .await
            .unwrap();

        assert!(output.contains("hello-world"));
    }

    #[tokio::test]
    async fn exec_reports_nonzero_exit() {
        let root = tempdir().unwrap();
        let tool = ExecTool::workspace(root.path());

        let output = tool.execute(json!({"command": "exit 3"})).await.unwrap();

        assert!(output.contains("[exit code: 3]"));
    }

    #[tokio::test]
    async fn exec_rejects_workdir_escape() {
        let root = tempdir().unwrap();
        let tool = ExecTool::workspace(root.path());

        let err = tool
            .execute(json!({"command": "pwd", "workdir": "../"}))
            .await
            .unwrap_err();

        assert!(err.to_string().contains("outside execution workspace"));
    }

    #[tokio::test]
    async fn exec_scrubs_and_truncates_output() {
        let root = tempdir().unwrap();
        let tool = ExecTool::workspace(root.path()).with_output_limit(300);

        let output = tool
            .execute(json!({
                "command": "printf 'OPENAI_API_KEY=sk-testsecretsecretsecretsecret\\n'; yes x | head -n 400"
            }))
            .await
            .unwrap();

        assert!(!output.contains("sk-testsecret"));
        assert!(output.contains("[REDACTED"));
        assert!(output.contains("[output truncated:"));
    }
}
