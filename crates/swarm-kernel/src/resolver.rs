use std::env;
use std::path::{Path, PathBuf};

use crate::agent::AgentChoice;

pub fn agent_invocation_available(agent: AgentChoice) -> Result<(), String> {
    match agent {
        AgentChoice::Claude => locate_claude().map(|_| ()).ok_or_else(|| {
            "Error: could not locate the `claude` binary in PATH or default locations.".to_string()
        }),
        AgentChoice::Codex => locate_codex().map(|_| ()).ok_or_else(|| {
            "Error: could not locate the `codex` binary in PATH or default locations.".to_string()
        }),
        AgentChoice::Auto => Ok(()),
    }
}

pub fn agent_available(agent: AgentChoice) -> bool {
    match agent {
        AgentChoice::Codex | AgentChoice::Claude => agent_invocation_available(agent).is_ok(),
        AgentChoice::Auto => locate_claude().is_some() || locate_codex().is_some(),
    }
}

pub fn resolve_agent(choice: AgentChoice) -> Result<AgentChoice, String> {
    match choice {
        AgentChoice::Codex | AgentChoice::Claude => Ok(choice),
        AgentChoice::Auto => {
            if locate_claude().is_some() {
                Ok(AgentChoice::Claude)
            } else if locate_codex().is_some() {
                Ok(AgentChoice::Codex)
            } else {
                Err(
                    "Error: no partner agent detected. Install Claude Code or Codex CLI, or pass --agent explicitly after installation."
                        .to_string(),
                )
            }
        }
    }
}

pub fn locate_codex() -> Option<PathBuf> {
    find_on_path("codex").or_else(|| {
        let mut candidates = Vec::new();
        if let Some(home) = home_dir() {
            candidates.push(home.join(".local/bin/codex"));
            candidates.push(home.join(".npm-global/bin/codex"));
            candidates.push(home.join(".yarn/bin/codex"));
        }
        candidates.push(PathBuf::from("/opt/homebrew/bin/codex"));
        candidates.push(PathBuf::from("/usr/local/bin/codex"));
        candidates.into_iter().find(|path| is_executable_file(path))
    })
}

pub fn locate_claude() -> Option<PathBuf> {
    find_on_path("claude").or_else(|| {
        let mut candidates = Vec::new();
        if let Some(home) = home_dir() {
            candidates.push(home.join(".local/bin/claude"));
            candidates.push(home.join(".npm-global/bin/claude"));
            candidates.push(home.join(".yarn/bin/claude"));
        }
        candidates.push(PathBuf::from("/opt/homebrew/bin/claude"));
        candidates.push(PathBuf::from("/usr/local/bin/claude"));
        candidates.into_iter().find(|path| is_executable_file(path))
    })
}

fn find_on_path(bin: &str) -> Option<PathBuf> {
    let path_var = env::var_os("PATH")?;
    env::split_paths(&path_var)
        .map(|dir| dir.join(bin))
        .find(|path| is_executable_file(path))
}

fn is_executable_file(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    is_executable(path)
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    path.metadata()
        .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(_path: &Path) -> bool {
    true
}

pub fn home_dir() -> Option<PathBuf> {
    env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

pub fn running_inside_codex() -> bool {
    env::var_os("CODEX_SHELL").is_some()
        || env::var_os("CODEX_THREAD_ID").is_some()
        || env::var("__CFBundleIdentifier")
            .map(|value| value == "com.openai.codex")
            .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_invocation_not_blocked_inside_codex_session() {
        // Regression guard: with the recursion guard removed, dispatching Codex must
        // succeed (or fail only because the binary is absent) even when the process
        // appears to be running *inside* a Codex session.
        //
        // NOTE: This test mutates the shared process env (`CODEX_SHELL`).  Rust test
        // threads share the env, so we capture the prior value and restore it before
        // returning.  The mutation window is intentionally small.  If `serial_test` is
        // ever added to this crate's dev-dependencies, annotate this test with
        // `#[serial]` to eliminate the theoretical TOCTOU window entirely.

        // --- step 1: put the process into the "inside Codex" state ---
        let prior = env::var_os("CODEX_SHELL");
        // SAFETY: edition 2021 — set_var is safe
        env::set_var("CODEX_SHELL", "1");

        // --- step 2: confirm the precondition (proves we're testing the right branch) ---
        assert!(
            running_inside_codex(),
            "precondition failed: running_inside_codex() must be true when CODEX_SHELL is set"
        );

        // --- step 3: invoke availability check ---
        let result = agent_invocation_available(AgentChoice::Codex);

        // --- step 4: restore env before any assertion can panic ---
        match prior {
            Some(val) => env::set_var("CODEX_SHELL", val),
            None => env::remove_var("CODEX_SHELL"),
        }

        // --- step 5: assert the result is NOT a recursion-style refusal ---
        // An Err is acceptable when Codex is simply absent on CI; only refuse
        // the old guard messages.
        if let Err(ref msg) = result {
            let lower = msg.to_ascii_lowercase();
            assert!(
                !lower.contains("recursive"),
                "recursion guard must be gone; got: {msg}"
            );
            assert!(
                !lower.contains("inside a codex session"),
                "recursion guard must be gone; got: {msg}"
            );
            assert!(
                !lower.contains("refusing to dispatch"),
                "recursion guard must be gone; got: {msg}"
            );
        }
    }
}
