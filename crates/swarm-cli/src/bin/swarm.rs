//! The `swarm` binary — a thin shim; all logic lives in the swarm-cli library.
fn main() {
    std::process::exit(swarm_cli::SwarmService::new().run().unwrap_or_else(|e| {
        eprintln!("{e}");
        1
    }));
}
