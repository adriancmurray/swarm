//! Writes the frozen `mcp-tools.json` artifact from the descriptor table.
//!
//! # Default behaviour
//!
//! Writes to `<CARGO_MANIFEST_DIR>/mcp-tools.json` (i.e.
//! `crates/swarm-mcp/mcp-tools.json`). Commit the generated file.
//! The idempotency test (`mcp_tools_json_matches_descriptor_table`) asserts that
//! the checked-in file matches the live descriptor table.
//!
//! # Override
//!
//! Pass `--output <path>` to write to a different location. CI uses this flag to
//! write to a temp file and diff against the checked-in artifact without ever
//! mutating the working tree:
//!
//! ```sh
//! cargo run -p swarm-mcp --bin gen_mcp_tools -- --output /tmp/mcp-tools-check.json
//! diff crates/swarm-mcp/mcp-tools.json /tmp/mcp-tools-check.json
//! ```
//!
//! # Byte-identity guarantee
//!
//! Both this binary and the idempotency test call `swarm_mcp::mcp_schema::mcp_tools_pretty_json()`,
//! which is the single serialization path. Output format: pretty-printed JSON array,
//! 2-space indent (serde_json default), trailing newline appended.
//!
//! P5-S6 cleanup: relocated from `tools/agent-swarm/rust/` into `swarm-mcp` —
//! the crate that owns the schema — so the bin and the frozen artifact live beside
//! the descriptor table they derive from. The binary now calls `swarm_mcp`
//! in-crate rather than reaching across the workspace boundary.

fn main() {
    let mut args = std::env::args().skip(1);
    let output_path: std::path::PathBuf = match (args.next().as_deref(), args.next()) {
        (None, None) => {
            // Default: write next to Cargo.toml
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("mcp-tools.json")
        }
        (Some("--output"), Some(path)) => std::path::PathBuf::from(path),
        _ => {
            eprintln!("usage: gen_mcp_tools [--output <path>]");
            std::process::exit(1);
        }
    };

    let content = swarm_mcp::mcp_schema::mcp_tools_pretty_json();

    if let Some(parent) = output_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).unwrap_or_else(|e| {
                eprintln!(
                    "error: could not create directory {}: {e}",
                    parent.display()
                );
                std::process::exit(1);
            });
        }
    }

    std::fs::write(&output_path, content.as_bytes()).unwrap_or_else(|e| {
        eprintln!("error: could not write {}: {e}", output_path.display());
        std::process::exit(1);
    });

    eprintln!("gen_mcp_tools: wrote {}", output_path.display());
}
