//! Optional service-registry hook for the MCP layer.
//!
//! This module exists only under the `registry` feature. It is the seam through
//! which a consumer may opt this process into a generic JSON service registry,
//! without the default build linking the registry crate or touching the
//! filesystem.
//!
//! The disabled state is modelled as `None`: [`register_with`] given `None`
//! returns `Ok(())` and never reaches a registrar. A consumer that wants
//! registration passes `Some(&registrar)` (for example a
//! `swarm_registrar::JsonFileRegistrar` aimed at a path of its choosing).
//!
//! The default MCP dispatch loop does not call this hook; wiring it in is a
//! consumer-side decision left to the caller.

pub use swarm_registrar::{JsonFileRegistrar, NoopRegistrar, ServiceRecord, ServiceRegistrar};

/// Register `record` through `registrar` when one is supplied.
///
/// `None` is the off state and a guaranteed no-op — it never touches the
/// filesystem. `Some(reg)` delegates to the registrar.
pub fn register_with(
    registrar: Option<&dyn ServiceRegistrar>,
    record: &ServiceRecord,
) -> std::io::Result<()> {
    match registrar {
        Some(reg) => reg.register(record),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn none_is_a_noop_and_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("services.json");
        let record = ServiceRecord::new("svc", "Service", "127.0.0.1:9000");

        register_with(None, &record).expect("None must succeed");

        assert!(
            !path.exists(),
            "a None registrar must never create a registry file"
        );
        // Nothing at all should have been written into the temp dir.
        let count = std::fs::read_dir(dir.path()).unwrap().count();
        assert_eq!(count, 0, "None registrar must not write anything");
    }

    #[test]
    fn some_registrar_writes_through() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("services.json");
        let registrar = JsonFileRegistrar::new(&path);
        let record = ServiceRecord::new("svc", "Service", "127.0.0.1:9000");

        register_with(Some(&registrar), &record).expect("Some must succeed");

        assert!(path.exists(), "Some registrar must write the registry file");
    }
}
