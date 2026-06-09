//! `scaffold-backend <name>` — emit starter files for a new agent backend.
//!
//! Two paths exist for adding a backend (see `docs/authoring-a-backend.md`):
//!
//! 1. **Descriptor (90% case)** — a config block, no code. The emitted
//!    `<name>.backend.toml` is a commented starter for the `cli` kind.
//! 2. **Trait impl (escape hatch)** — implement `AgentBackend` in Rust when a
//!    descriptor can't express the behavior. The emitted `<name>_backend.rs`
//!    is a commented skeleton with all four trait methods stubbed.
//!
//! [`scaffold_backend`] is a pure function (templates → files) so it is
//! unit-testable without a process or network. The subcommand is a thin
//! wrapper that resolves the output dir and prints the written paths.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Emit a backend scaffold (`<name>_backend.rs` trait skeleton +
/// `<name>.backend.toml` descriptor stub) into `out_dir`.
///
/// Returns the written paths in a stable order: the Rust skeleton first, the
/// descriptor stub second.
///
/// Overwrite behavior: files are written unconditionally. Re-running the
/// command regenerates the scaffold from scratch, clobbering any prior output
/// for the same `<name>`. This is intentional — a scaffold is a starting
/// point, and a re-run is the obvious way to reset it. Move your edits out of
/// the generated file before re-running.
///
/// All emitted identifiers derive from `name` only, so the output carries no
/// project-specific strings.
pub fn scaffold_backend(name: &str, out_dir: &Path) -> io::Result<Vec<PathBuf>> {
    fs::create_dir_all(out_dir)?;

    let rust_path = out_dir.join(format!("{name}_backend.rs"));
    let descriptor_path = out_dir.join(format!("{name}.backend.toml"));

    fs::write(&rust_path, rust_skeleton(name))?;
    fs::write(&descriptor_path, descriptor_stub(name))?;

    Ok(vec![rust_path, descriptor_path])
}

const SCAFFOLD_USAGE: &str = "usage: agent-swarm scaffold-backend <name> [--out DIR]\n\
     \n\
     Emits two starter files for a new agent backend into DIR (default: cwd):\n\
       <name>.backend.toml   declarative descriptor stub (the no-code 90% case)\n\
       <name>_backend.rs     commented AgentBackend trait-impl skeleton\n\
     \n\
     See docs/authoring-a-backend.md for the full guide.";

/// Thin CLI wrapper around [`scaffold_backend`]. Parses `<name> [--out DIR]`,
/// writes the files, and prints the written paths.
pub fn cmd_scaffold_backend(args: &[String]) -> Result<i32, String> {
    let mut name: Option<String> = None;
    let mut out_dir: Option<PathBuf> = None;

    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                println!("{SCAFFOLD_USAGE}");
                return Ok(0);
            }
            "--out" => {
                let value = iter
                    .next()
                    .ok_or_else(|| format!("--out requires a directory\n{SCAFFOLD_USAGE}"))?;
                out_dir = Some(PathBuf::from(value));
            }
            other if other.starts_with('-') => {
                return Err(format!("unknown flag `{other}`\n{SCAFFOLD_USAGE}"));
            }
            other => {
                if name.is_some() {
                    return Err(format!(
                        "unexpected extra argument `{other}`\n{SCAFFOLD_USAGE}"
                    ));
                }
                name = Some(other.to_string());
            }
        }
    }

    let name =
        name.ok_or_else(|| format!("scaffold-backend requires a <name>\n{SCAFFOLD_USAGE}"))?;
    let out_dir = out_dir.unwrap_or_else(|| PathBuf::from("."));

    let written = scaffold_backend(&name, &out_dir)
        .map_err(|err| format!("failed to write scaffold for `{name}`: {err}"))?;

    println!("Scaffolded backend `{name}`:");
    for path in &written {
        println!("  {}", path.display());
    }
    println!("\nNext: see docs/authoring-a-backend.md to fill in the descriptor or trait impl.");
    Ok(0)
}

/// The commented `AgentBackend` trait-impl skeleton for `name`.
///
/// Signatures mirror the real trait exactly. Crate paths are shown as a
/// commented placeholder (`your_crate`) because the concrete import path
/// depends on how you wire the backend into your build.
fn rust_skeleton(name: &str) -> String {
    let struct_name = pascal_case(name);
    format!(
        r##"//! `{name}` agent backend — trait-impl escape hatch.
//!
//! Reach for this only when a declarative descriptor can't express what you
//! need (custom handshake, bespoke streaming, non-standard auth). For the
//! common case, prefer the descriptor stub (`{name}.backend.toml`) — no code.
//!
//! Bring the ABI types into scope from the crate that defines them. The trait
//! `AgentBackend` and the request/outcome types are part of the public ABI:
//!
//! ```ignore
//! // use your_crate::{{
//! //     AgentBackend, BackendCaps, BackendError, BackendRequest, BackendSink,
//! //     RunOutcome,
//! // }};
//! ```

/// The `{name}` backend.
///
/// Hold whatever state a single attempt needs here (a located binary path, a
/// cached client, config). Construct it once and register it with the engine.
pub struct {struct_name}Backend;

impl AgentBackend for {struct_name}Backend {{
    /// Stable identifier for this backend. Used in logs, routing, and
    /// telemetry. Keep it short and lowercase (e.g. `"{name}"`).
    fn id(&self) -> &str {{
        "{name}"
    }}

    /// Gate: can this backend run right now? Check that the binary is on
    /// PATH, the API key env var is set, the config file exists — whatever
    /// this backend depends on. Return `Ok(())` when ready, or
    /// `BackendError::NotReady(detail)` with an actionable message otherwise.
    fn ready(&self) -> Result<(), BackendError> {{
        // TODO: probe the binary / key / endpoint and return a typed result.
        todo!("check that `{name}` can run and return Ok(()) or NotReady")
    }}

    /// Run a single attempt.
    ///
    /// Read everything from the borrowed `req` (prompt, optional model, cwd,
    /// timeout, quiet, permission-bypass, env policy, cancel token). Stream
    /// output as it arrives via `sink.stdout_chunk(..)` / `sink.stderr_chunk(..)`
    /// and optionally `sink.final_answer(..)`. Return a `RunOutcome` on
    /// success, or a typed `BackendError` (so the retry machine branches on
    /// cause, never on error text).
    fn run(
        &self,
        req: &BackendRequest,
        sink: &mut dyn BackendSink,
    ) -> Result<RunOutcome, BackendError> {{
        // Example shape — replace with your real execution:
        //   sink.stdout_chunk("...partial output...");
        //   Ok(RunOutcome {{ stdout: collected, ..Default::default() }})
        let _ = (req, sink);
        todo!("run one `{name}` attempt, stream through `sink`, return RunOutcome")
    }}

    /// Declare what this backend can do. `streaming` = it emits chunks as they
    /// arrive; `cancellation` = it honors `req.cancel`. `BackendCaps::default()`
    /// reports streaming without cancellation.
    fn capabilities(&self) -> BackendCaps {{
        BackendCaps::default()
    }}
}}
"##
    )
}

/// The commented descriptor stub for `name`, defaulting to the `cli` kind.
fn descriptor_stub(name: &str) -> String {
    format!(
        r#"# `{name}` backend descriptor — the no-code (90%) way to add an agent.
#
# Drop a block like this into your backend config. The `cli` kind wraps any
# command-line agent as a subprocess; no Rust required. Reach for a trait impl
# (`{name}_backend.rs`) only when a descriptor can't express what you need.
#
# Three kinds are available:
#   kind = "cli"                # wrap a command-line agent (shown below)
#   kind = "openai-compatible"  # talk HTTP to a /v1/chat/completions endpoint
#   kind = "native"             # select a built-in in-process harness by provider

[backend.{name}]
kind = "cli"

# The executable to run. Must be on PATH or an absolute path.
command = "my-cli"

# Arguments passed to `command`. Two tokens are substituted at run time:
#   {{model}}   -> the model id for this run (omit if the agent has no flag)
#   {{prompt}}  -> the prompt text (only needed if prompt = "arg"; with the
#               default prompt = "stdin" the prompt is piped to the child's
#               stdin and you do not template it into args)
args = ["--print", "--model", "{{model}}"]

# How the prompt reaches the child process:
#   "stdin" (default) -> piped to stdin
#   "arg"             -> appended as the final positional arg (use {{prompt}})
prompt = "stdin"

# ── openai-compatible kind ──────────────────────────────────────────────────
# For an HTTP endpoint, swap to this shape instead of command/args. Secrets are
# read from the environment by name — never written here:
#
#   [backend.{name}]
#   kind = "openai-compatible"
#   base_url_env = "SOME_API_BASE_URL"   # env var holding the endpoint base URL
#   api_key_env  = "SOME_API_KEY"        # env var holding the API key (a secret)
#   default_model = "some-model"         # used when a run specifies no model
"#
    )
}

/// Convert a backend name to a PascalCase Rust type prefix. Splits on common
/// separators (`-`, `_`, space) and capitalizes each segment; non-alphanumeric
/// characters within a segment are dropped so the result is a valid identifier
/// fragment.
fn pascal_case(name: &str) -> String {
    let mut out = String::new();
    for segment in name.split(['-', '_', ' ']) {
        let mut chars = segment.chars().filter(|c| c.is_alphanumeric());
        if let Some(first) = chars.next() {
            out.extend(first.to_uppercase());
            for c in chars {
                out.push(c);
            }
        }
    }
    if out.is_empty() {
        out.push_str("Custom");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The banned project-specific strings the scaffold output must never emit.
    /// Mirrors the zero-mentions gate. Each needle is assembled with `concat!`
    /// so the joined token never appears verbatim in this source file — this
    /// file is itself subject to the gate and must stay born-clean.
    const BANNED: &[&str] = &[
        concat!("nano", "bot"),
        concat!("ab", "zu"),
        concat!("my", "cel"),
        concat!("mne", "me"),
        concat!("de", "ch-agent"),
        concat!("de", "ch"),
        concat!("feder", "ation"),
        concat!("a", "gy"),
    ];

    fn assert_clean(text: &str) {
        let lower = text.to_lowercase();
        for needle in BANNED {
            assert!(
                !lower.contains(needle),
                "generated output must not contain banned string `{needle}`:\n{text}"
            );
        }
    }

    #[test]
    fn writes_both_files() {
        let dir = tempfile::tempdir().unwrap();
        let written = scaffold_backend("example", dir.path()).unwrap();
        assert_eq!(written.len(), 2);
        assert!(written[0].ends_with("example_backend.rs"));
        assert!(written[1].ends_with("example.backend.toml"));
        assert!(written[0].exists());
        assert!(written[1].exists());
    }

    #[test]
    fn rust_skeleton_has_all_four_trait_methods_and_impl() {
        let rs = rust_skeleton("example");
        assert!(rs.contains("impl AgentBackend for"));
        assert!(rs.contains("fn id("));
        assert!(rs.contains("fn ready("));
        assert!(rs.contains("fn run("));
        assert!(rs.contains("fn capabilities("));
        // Signature shape matches the real trait.
        assert!(rs.contains("-> Result<RunOutcome, BackendError>"));
        assert!(rs.contains("sink: &mut dyn BackendSink"));
        assert!(rs.contains("todo!"));
    }

    #[test]
    fn rust_skeleton_struct_name_derives_from_name() {
        let rs = rust_skeleton("my-cool-agent");
        assert!(rs.contains("pub struct MyCoolAgentBackend;"));
        assert!(rs.contains("impl AgentBackend for MyCoolAgentBackend"));
    }

    #[test]
    fn descriptor_stub_has_kind_command_and_tokens() {
        let toml = descriptor_stub("example");
        assert!(toml.contains("[backend.example]"));
        assert!(toml.contains("kind = \"cli\""));
        assert!(toml.contains("command = "));
        assert!(toml.contains("{model}"));
        assert!(toml.contains("{prompt}"));
        // The two other kinds are documented.
        assert!(toml.contains("openai-compatible"));
        assert!(toml.contains("native"));
    }

    #[test]
    fn emitted_output_is_born_clean() {
        assert_clean(&rust_skeleton("example"));
        assert_clean(&descriptor_stub("example"));
    }

    #[test]
    fn files_on_disk_are_born_clean() {
        let dir = tempfile::tempdir().unwrap();
        let written = scaffold_backend("example", dir.path()).unwrap();
        for path in written {
            let text = fs::read_to_string(&path).unwrap();
            assert_clean(&text);
        }
    }

    #[test]
    fn rerun_overwrites_existing_files() {
        let dir = tempfile::tempdir().unwrap();
        scaffold_backend("example", dir.path()).unwrap();
        let rust_path = dir.path().join("example_backend.rs");
        fs::write(&rust_path, "STALE").unwrap();
        // Re-running regenerates from scratch, clobbering the stale content.
        scaffold_backend("example", dir.path()).unwrap();
        let regenerated = fs::read_to_string(&rust_path).unwrap();
        assert!(regenerated.contains("impl AgentBackend for"));
        assert!(!regenerated.contains("STALE"));
    }

    #[test]
    fn creates_missing_output_dir() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("a").join("b");
        let written = scaffold_backend("example", &nested).unwrap();
        assert!(written[0].exists());
        assert!(written[1].exists());
    }

    #[test]
    fn pascal_case_handles_separators_and_fallback() {
        assert_eq!(pascal_case("example"), "Example");
        assert_eq!(pascal_case("my-cli"), "MyCli");
        assert_eq!(pascal_case("foo_bar baz"), "FooBarBaz");
        assert_eq!(pascal_case("---"), "Custom");
    }
}
