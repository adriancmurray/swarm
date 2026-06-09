//! `PackageRepo` — read-only trait + `StaticPackageRepo` implementation.
//!
//! Phase 1 of the repository-trait wave. The package manifest, presets, and
//! agent profiles are all hardcoded (compiled-in) data today. `StaticPackageRepo`
//! wraps those existing functions behind the trait so callers can be written
//! against the trait boundary rather than the concrete functions.
//!
//! Write paths (preset installation, version management) are explicitly out of
//! scope for Phase 1. See design spec §3.6 for the Phase 2 design question.
//!
//! **`presets()` decision:** `crate::telemetry::presets_json()` is `pub` and
//! callable without editing `telemetry.rs`. `StaticPackageRepo::presets()`
//! delegates to it, returning the raw `serde_json::Value`. No changes to
//! `telemetry.rs` were made.

use swarm_core::RepoError;
use swarm_kernel::profiles::AgentProfile;

// ── Trait ────────────────────────────────────────────────────────────────────

/// Read-only access to package manifests and preset definitions.
///
/// Both backends — static (compiled-in) and any future file-backed backend —
/// implement this trait. The in-memory test backend is identical to the static
/// backend for Phase 1 because there is no durable package store yet.
///
/// Write path (installation / preset authoring) is deferred to Phase 2.
pub trait PackageRepo: Send + Sync {
    /// Return the package manifest (`swarm.manifest/v1` wire schema) for this service.
    ///
    /// Returns the same JSON shape produced by `swarm_mcp::manifest::manifest_payload()`.
    fn manifest(&self) -> Result<serde_json::Value, RepoError>;

    /// Return the named presets list.
    ///
    /// Returns the same JSON shape produced by `swarm_kernel::telemetry::presets_json()`.
    fn presets(&self) -> Result<serde_json::Value, RepoError>;

    /// Return agent profiles (compiled-in static data in Phase 1).
    fn profiles(&self) -> Result<Vec<AgentProfile>, RepoError>;
}

// ── StaticPackageRepo ────────────────────────────────────────────────────────

/// Wraps compiled-in package data behind the `PackageRepo` trait.
///
/// No files are read; no writes occur. Suitable as both the production
/// implementation and the in-memory test double (they are identical for
/// Phase 1).
pub struct StaticPackageRepo;

impl PackageRepo for StaticPackageRepo {
    fn manifest(&self) -> Result<serde_json::Value, RepoError> {
        Ok(swarm_mcp::manifest::manifest_payload())
    }

    fn presets(&self) -> Result<serde_json::Value, RepoError> {
        Ok(swarm_kernel::telemetry::presets_json())
    }

    fn profiles(&self) -> Result<Vec<AgentProfile>, RepoError> {
        Ok(swarm_kernel::profiles::profiles())
    }
}

// ── Contract tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Gate-3 contract: any `PackageRepo` impl must satisfy these invariants.
    ///
    /// `StaticPackageRepo` is the only Phase-1 backend (file-backed == static),
    /// so we instantiate the contract once. Additional backends instantiate it
    /// the same way.
    fn package_repo_contract<R: PackageRepo>(repo: R) {
        // manifest() — must advertise the supported backend capabilities
        let manifest = repo.manifest().expect("manifest() must not fail");
        let capabilities = manifest
            .get("capabilities")
            .and_then(|v| v.as_array())
            .expect("manifest must contain a 'capabilities' array");
        let backends = capabilities
            .iter()
            .filter_map(|cap| cap.as_str())
            .filter(|cap| cap.starts_with("backend."))
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            backends,
            std::collections::BTreeSet::from(["backend.claude", "backend.codex"])
        );

        // presets() — must return a value with a non-empty presets array
        let presets = repo.presets().expect("presets() must not fail");
        let preset_list = presets
            .get("presets")
            .and_then(|v| v.as_array())
            .expect("presets must contain a 'presets' array");
        assert!(
            !preset_list.is_empty(),
            "presets must be non-empty in StaticPackageRepo"
        );

        // profiles() — must be non-empty
        let profiles = repo.profiles().expect("profiles() must not fail");
        assert!(
            !profiles.is_empty(),
            "profiles() must return at least one AgentProfile"
        );
    }

    #[test]
    fn static_package_repo_satisfies_contract() {
        package_repo_contract(StaticPackageRepo);
    }
}
