//! Optional, generic service-registry self-registration.
//!
//! A process can announce itself into a JSON **service registry** — a map of
//! `id -> record` persisted at a caller-chosen path — so peers can discover its
//! endpoint. The mechanism is deliberately small and dependency-light:
//!
//! - The registry file is a single JSON object mapping each service id to its
//!   [`ServiceRecord`]. Registering is **additive**: existing entries (and any
//!   unknown fields they carry) survive a round-trip; only the registering id is
//!   inserted or updated in place.
//! - Writes are **atomic** (temp file in the same directory + `rename`), so a
//!   concurrent reader never observes a partially written file, and a crash mid
//!   write leaves the previous registry intact.
//! - The whole capability is **off by default**. Nothing here runs unless a
//!   caller constructs a [`ServiceRegistrar`] and hands it a record. The
//!   [`NoopRegistrar`] (or an `Option<Box<dyn ServiceRegistrar>>` left `None`)
//!   models the disabled state and never touches the filesystem.
//!
//! The registry path is always supplied by the caller — there is no hardcoded
//! home-directory location.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// One entry in the service registry.
///
/// `extra` captures any caller-specific fields verbatim so the record can be
/// extended without changing this type; those fields round-trip through the
/// registry file unchanged.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServiceRecord {
    /// Stable identity used as the registry key. Re-registering the same id
    /// updates the entry in place rather than appending a duplicate.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// How peers reach this service (a path, URL, or `host:port` — opaque here).
    pub endpoint: String,
    /// Free-form discovery tags.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Any additional fields, preserved verbatim on round-trip.
    #[serde(flatten, default)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl ServiceRecord {
    /// Construct a record with the required fields and no tags or extras.
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        endpoint: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            endpoint: endpoint.into(),
            tags: Vec::new(),
            extra: BTreeMap::new(),
        }
    }

    /// Builder-style override of the discovery tags.
    pub fn with_tags(mut self, tags: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.tags = tags.into_iter().map(Into::into).collect();
        self
    }
}

/// A sink that records (and optionally removes) a process's presence.
///
/// Implementations decide where and how the registration is stored. The default
/// [`JsonFileRegistrar`] persists to a JSON file; [`NoopRegistrar`] discards the
/// call. A `None` value at the call site (`Option<Box<dyn ServiceRegistrar>>`)
/// is the canonical "disabled" form and never reaches an implementation.
pub trait ServiceRegistrar {
    /// Insert or update `record` in the registry. Idempotent for an unchanged
    /// record: registering the same content twice leaves the registry identical.
    fn register(&self, record: &ServiceRecord) -> io::Result<()>;

    /// Remove the entry for `id` if present. Removing an absent id is a success
    /// (no-op). The default implementation is a no-op for registrars that do not
    /// support removal.
    fn deregister(&self, _id: &str) -> io::Result<()> {
        Ok(())
    }
}

/// A registrar that does nothing — the explicit "off" implementation.
///
/// Useful where a `&dyn ServiceRegistrar` is required but registration should be
/// suppressed, as an alternative to threading `Option` through the call site.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopRegistrar;

impl ServiceRegistrar for NoopRegistrar {
    fn register(&self, _record: &ServiceRecord) -> io::Result<()> {
        Ok(())
    }

    fn deregister(&self, _id: &str) -> io::Result<()> {
        Ok(())
    }
}

/// A registrar backed by a JSON file at a configurable path.
///
/// The file holds a single JSON object mapping `id -> record`. Each
/// [`register`](ServiceRegistrar::register) reads the current map (if the file
/// exists), inserts/updates this record by id, then writes the whole map back
/// atomically (temp file in the same directory + `rename`). The parent directory
/// is created on demand.
#[derive(Debug, Clone)]
pub struct JsonFileRegistrar {
    path: PathBuf,
}

impl JsonFileRegistrar {
    /// Create a registrar that persists to `path`.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// The registry file path this registrar writes to.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Read the current `id -> record` map, or an empty map if the file is absent.
    fn read_map(&self) -> io::Result<BTreeMap<String, ServiceRecord>> {
        match fs::read_to_string(&self.path) {
            Ok(raw) => serde_json::from_str(&raw).map_err(io::Error::from),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(BTreeMap::new()),
            Err(err) => Err(err),
        }
    }

    /// Serialize `map` and write it to `self.path` atomically.
    ///
    /// Writes to a temporary file in the same directory, then `rename`s it over
    /// the target — `rename` within a directory is atomic on the platforms this
    /// targets, so a reader sees either the old file or the complete new one, and
    /// a failure leaves no partial file at the target path.
    fn write_map(&self, map: &BTreeMap<String, ServiceRecord>) -> io::Result<()> {
        let dir = self.path.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(dir)?;

        let mut contents = serde_json::to_string_pretty(map).map_err(io::Error::from)?;
        contents.push('\n');

        let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
        tmp.write_all(contents.as_bytes())?;
        tmp.flush()?;
        tmp.persist(&self.path)
            .map_err(|err| io::Error::other(err.error))?;
        Ok(())
    }
}

impl ServiceRegistrar for JsonFileRegistrar {
    fn register(&self, record: &ServiceRecord) -> io::Result<()> {
        let mut map = self.read_map()?;
        map.insert(record.id.clone(), record.clone());
        self.write_map(&map)
    }

    fn deregister(&self, id: &str) -> io::Result<()> {
        let mut map = self.read_map()?;
        if map.remove(id).is_some() {
            self.write_map(&map)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn registry_path(dir: &TempDir) -> PathBuf {
        // Nested path proves the parent directory is created on demand.
        dir.path().join("registry").join("services.json")
    }

    fn read_map(path: &Path) -> BTreeMap<String, ServiceRecord> {
        let raw = fs::read_to_string(path).expect("registry file should exist");
        serde_json::from_str(&raw).expect("registry should parse")
    }

    #[test]
    fn register_creates_file_with_record() {
        let dir = tempfile::tempdir().unwrap();
        let path = registry_path(&dir);
        assert!(!path.exists());

        let registrar = JsonFileRegistrar::new(&path);
        let record =
            ServiceRecord::new("svc-a", "Service A", "127.0.0.1:9000").with_tags(["agent", "demo"]);
        registrar
            .register(&record)
            .expect("register should succeed");

        assert!(path.exists(), "registry file must be created");
        let map = read_map(&path);
        assert_eq!(map.len(), 1);
        assert_eq!(map.get("svc-a"), Some(&record));
    }

    #[test]
    fn second_register_merges_additively() {
        let dir = tempfile::tempdir().unwrap();
        let path = registry_path(&dir);
        let registrar = JsonFileRegistrar::new(&path);

        let a = ServiceRecord::new("svc-a", "Service A", "127.0.0.1:9000");
        let b = ServiceRecord::new("svc-b", "Service B", "127.0.0.1:9100");
        registrar.register(&a).unwrap();
        registrar.register(&b).unwrap();

        let map = read_map(&path);
        assert_eq!(map.len(), 2, "both records must be present");
        assert_eq!(map.get("svc-a"), Some(&a));
        assert_eq!(map.get("svc-b"), Some(&b));
    }

    #[test]
    fn same_id_updates_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let path = registry_path(&dir);
        let registrar = JsonFileRegistrar::new(&path);

        registrar
            .register(&ServiceRecord::new("svc-a", "Service A", "old:1"))
            .unwrap();
        let updated = ServiceRecord::new("svc-a", "Service A v2", "new:2").with_tags(["fresh"]);
        registrar.register(&updated).unwrap();

        let map = read_map(&path);
        assert_eq!(map.len(), 1, "same id must not duplicate");
        assert_eq!(map.get("svc-a"), Some(&updated));
    }

    #[test]
    fn unchanged_record_is_idempotent_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = registry_path(&dir);
        let registrar = JsonFileRegistrar::new(&path);
        let record =
            ServiceRecord::new("svc-a", "Service A", "127.0.0.1:9000").with_tags(["agent"]);

        registrar.register(&record).unwrap();
        let first = fs::read(&path).unwrap();
        registrar.register(&record).unwrap();
        let second = fs::read(&path).unwrap();

        assert_eq!(
            first, second,
            "re-registering identical content must be byte-stable"
        );
    }

    #[test]
    fn foreign_entries_and_unknown_fields_survive_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = registry_path(&dir);
        fs::create_dir_all(path.parent().unwrap()).unwrap();

        // Seed the registry with a foreign record carrying an unknown field.
        let seed = serde_json::json!({
            "peer": {
                "id": "peer",
                "name": "Peer Service",
                "endpoint": "/usr/local/bin/peer",
                "tags": ["memory"],
                "custom_extension": "keep-me"
            }
        });
        fs::write(&path, serde_json::to_string_pretty(&seed).unwrap() + "\n").unwrap();

        let registrar = JsonFileRegistrar::new(&path);
        registrar
            .register(&ServiceRecord::new("svc-a", "Service A", "127.0.0.1:9000"))
            .unwrap();

        let map = read_map(&path);
        assert_eq!(map.len(), 2, "foreign and new records both present");

        let peer = map.get("peer").expect("foreign record must survive");
        assert_eq!(
            peer.extra.get("custom_extension"),
            Some(&serde_json::Value::String("keep-me".into())),
            "unknown field must round-trip"
        );
    }

    #[test]
    fn atomic_write_leaves_no_partial_file() {
        // Happy path: after a successful register, the only file in the registry
        // directory is the target itself — no leftover temp file.
        let dir = tempfile::tempdir().unwrap();
        let path = registry_path(&dir);
        let registrar = JsonFileRegistrar::new(&path);
        registrar
            .register(&ServiceRecord::new("svc-a", "Service A", "127.0.0.1:9000"))
            .unwrap();

        let entries: Vec<_> = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(
            entries.len(),
            1,
            "exactly one file (no stray temp): {entries:?}"
        );
        assert_eq!(entries[0], "services.json");
    }

    #[test]
    fn deregister_removes_entry() {
        let dir = tempfile::tempdir().unwrap();
        let path = registry_path(&dir);
        let registrar = JsonFileRegistrar::new(&path);

        registrar
            .register(&ServiceRecord::new("svc-a", "Service A", "a:1"))
            .unwrap();
        registrar
            .register(&ServiceRecord::new("svc-b", "Service B", "b:1"))
            .unwrap();
        registrar.deregister("svc-a").unwrap();

        let map = read_map(&path);
        assert_eq!(map.len(), 1);
        assert!(!map.contains_key("svc-a"));
        assert!(map.contains_key("svc-b"));

        // Removing an absent id is a success and a no-op.
        registrar.deregister("nope").unwrap();
        assert_eq!(read_map(&path).len(), 1);
    }

    #[test]
    fn record_serde_roundtrips() {
        let record = ServiceRecord::new("svc-a", "Service A", "127.0.0.1:9000")
            .with_tags(["agent", "swarm"]);
        let json = serde_json::to_string(&record).unwrap();
        let back: ServiceRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(record, back);
    }

    #[test]
    fn noop_registrar_never_writes() {
        let dir = tempfile::tempdir().unwrap();
        let path = registry_path(&dir);
        let registrar = NoopRegistrar;
        registrar
            .register(&ServiceRecord::new("svc-a", "Service A", "127.0.0.1:9000"))
            .unwrap();
        registrar.deregister("svc-a").unwrap();
        assert!(!path.exists(), "noop registrar must never create a file");
    }
}
