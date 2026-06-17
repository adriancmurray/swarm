# swarm-registrar

Optional, dependency-light self-registration of a process into a JSON **service
registry** at a caller-chosen path. Off by default.

## Overview

`swarm-registrar` owns one small capability: letting a running process *announce
itself* so peers can discover its endpoint. A registry is a single JSON file
holding an object that maps each service `id` to a [`ServiceRecord`](#servicerecord).
A process registers by handing a record to a [`ServiceRegistrar`](#serviceregistrar);
the registrar persists it.

The design is deliberately minimal and has three load-bearing properties:

- **Additive merge.** Registering inserts or updates only the registering id.
  Existing entries — including foreign records and any unknown fields they carry —
  survive every round-trip untouched. Re-registering the same content twice leaves
  the file byte-for-byte identical (idempotent on disk).
- **Atomic writes.** The registrar serializes the whole map to a temp file in the
  same directory and `rename`s it over the target. A concurrent reader sees either
  the old complete file or the new complete file, never a partial one; a crash
  mid-write leaves the previous registry intact and no stray temp file.
- **Off by default.** Nothing here runs unless a caller constructs a registrar and
  hands it a record. The disabled state is modelled either by [`NoopRegistrar`](#noopregistrar)
  or by leaving an `Option<Box<dyn ServiceRegistrar>>` as `None` — neither touches
  the filesystem. There is no hardcoded home-directory path; the caller always
  supplies the location.

This crate is the storage primitive only. The opt-in *policy* of when swarm
registers itself lives behind the `registry` feature in `swarm-mcp`
(`register_with`), which constructs a `JsonFileRegistrar` and calls it.

## Dependency position

A standalone leaf. It sits off to the side of the main DAG with **no sibling
crates** — it depends only on external crates (`serde`, `serde_json`, `tempfile`)
and has no required dependents. The single optional consumer is
[[swarm-mcp#registry-hook]] (`registry` feature), which is itself off by default.

```
swarm-contracts → swarm-core → swarm-store → swarm-kernel → swarm-exec → swarm-mcp/swarm-cli
swarm-registrar  (optional, standalone; serde + tempfile only)
```

## Concepts

- [`ServiceRecord`](#servicerecord) — one entry in the registry: stable `id`, name, endpoint, tags, and a flattened `extra` map preserved verbatim.
- [`ServiceRecord::new`](#servicerecordnew) — construct a record from the three required fields, no tags or extras.
- [`ServiceRecord::with_tags`](#servicerecordwith_tags) — builder-style override of the discovery tags.
- [`ServiceRegistrar`](#serviceregistrar) — trait sink that records (and optionally removes) a process's presence: `register` / `deregister`.
- [`NoopRegistrar`](#noopregistrar) — the explicit "off" implementation; every call is a no-op that never touches disk.
- [`JsonFileRegistrar`](#jsonfileregistrar) — file-backed registrar performing the additive, atomic JSON-map persistence.
- [`JsonFileRegistrar::new`](#jsonfileregistrarnew) — create a registrar bound to a given file path.
- [`JsonFileRegistrar::path`](#jsonfileregistrarpath) — the registry file path this registrar writes to.

## API surface

### ServiceRecord

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServiceRecord {
    pub id: String,
    pub name: String,
    pub endpoint: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(flatten, default)]
    pub extra: BTreeMap<String, serde_json::Value>,
}
```

One entry in the service registry.

Fields:
- `id` — stable identity, used as the registry map key. Re-registering the same id
  updates the entry in place rather than appending a duplicate.
- `name` — human-readable name.
- `endpoint` — how peers reach this service (a path, URL, or `host:port` — opaque
  to this crate).
- `tags` — free-form discovery tags; defaults to empty on deserialize.
- `extra` — flattened catch-all for any caller-specific fields, captured verbatim.
  This is what lets a record (or a foreign peer's record) be extended without
  changing this type and still round-trip unchanged through the registry file.

When to use: build one of these to describe a service you want discoverable, then
pass it to [`ServiceRegistrar::register`](#serviceregistrar). Because it derives
`Serialize`/`Deserialize`, it is also the value type stored in the JSON map.

Related: [[swarm-registrar#serviceregistrar]], [[swarm-registrar#servicerecordnew]], [[swarm-registrar#servicerecordwith_tags]]

### ServiceRecord::new

```rust
pub fn new(
    id: impl Into<String>,
    name: impl Into<String>,
    endpoint: impl Into<String>,
) -> Self
```

Construct a record from the three required fields, with empty `tags` and `extra`.

Params: `id`, `name`, `endpoint` — anything convertible into `String`.
Returns: a `ServiceRecord` ready to register or to extend with
[`with_tags`](#servicerecordwith_tags).

When to use: the standard constructor for a new service entry. Chain `with_tags`
when you need discovery tags; set `extra` directly for arbitrary fields.

Related: [[swarm-registrar#servicerecord]], [[swarm-registrar#servicerecordwith_tags]]

### ServiceRecord::with_tags

```rust
pub fn with_tags(mut self, tags: impl IntoIterator<Item = impl Into<String>>) -> Self
```

Builder-style override of the discovery tags, consuming and returning `self`.

Params: `tags` — any iterable of items convertible into `String` (e.g.
`["agent", "demo"]`).
Returns: the record with `tags` replaced by the supplied collection.

When to use: immediately after [`new`](#servicerecordnew) to attach discovery
tags in a single expression, e.g.
`ServiceRecord::new("svc", "Svc", "host:9000").with_tags(["agent"])`.

Related: [[swarm-registrar#servicerecordnew]]

### ServiceRegistrar

```rust
pub trait ServiceRegistrar {
    fn register(&self, record: &ServiceRecord) -> io::Result<()>;
    fn deregister(&self, _id: &str) -> io::Result<()> { Ok(()) }
}
```

A sink that records (and optionally removes) a process's presence. The
implementation decides where and how registration is stored.

Methods:
- `register(record)` — insert or update `record` in the registry, keyed by
  `record.id`. Idempotent for unchanged content: registering the same record twice
  leaves the registry identical.
- `deregister(id)` — remove the entry for `id` if present. Removing an absent id is
  a success (no-op). The provided default implementation is a no-op, so registrars
  that do not support removal need not override it.

When to use: program against `&dyn ServiceRegistrar` (or
`Option<Box<dyn ServiceRegistrar>>`) so the registry backend — real, off, or a
test double — is swappable at the call site. A `None` `Option` is the canonical
"disabled" form and never reaches an implementation.

Implementors: [`JsonFileRegistrar`](#jsonfileregistrar) (file-backed),
[`NoopRegistrar`](#noopregistrar) (off).

Related: [[swarm-registrar#servicerecord]], [[swarm-registrar#jsonfileregistrar]], [[swarm-registrar#noopregistrar]], [[swarm-mcp#registry-hook]]

### NoopRegistrar

```rust
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopRegistrar;

impl ServiceRegistrar for NoopRegistrar { /* register/deregister return Ok(()) */ }
```

A registrar that does nothing — the explicit "off" implementation. Both
`register` and `deregister` return `Ok(())` and never touch the filesystem.

When to use: where a concrete `&dyn ServiceRegistrar` is required but registration
should be suppressed, as an alternative to threading an `Option` through the call
site. Being `Default`/`Copy`, it is free to construct (`NoopRegistrar`).

Related: [[swarm-registrar#serviceregistrar]], [[swarm-registrar#jsonfileregistrar]]

### JsonFileRegistrar

```rust
#[derive(Debug, Clone)]
pub struct JsonFileRegistrar { /* private: path: PathBuf */ }

impl ServiceRegistrar for JsonFileRegistrar {
    fn register(&self, record: &ServiceRecord) -> io::Result<()>;
    fn deregister(&self, id: &str) -> io::Result<()>;
}
```

A registrar backed by a JSON file at a configurable path. The file holds a single
JSON object mapping `id -> record`.

Behaviour:
- `register` reads the current map (an absent file is treated as an empty map),
  inserts/updates this record by `id`, then writes the whole map back atomically.
- `deregister` reads the map, removes `id` if present, and rewrites only if
  something was removed (an absent id performs no write).
- The parent directory is created on demand. Foreign entries and unknown fields in
  existing records are preserved across every write (additive merge). The pretty
  JSON output makes unchanged re-registration byte-stable on disk.
- Atomicity: a temp file is written in the same directory and `rename`d over the
  target, so readers never see a partial file and a failure leaves no stray temp
  file. Returns an `io::Error` on serialize/IO failure.

When to use: the production backend whenever real service discovery is desired —
construct it with the path peers will read.

Related: [[swarm-registrar#serviceregistrar]], [[swarm-registrar#servicerecord]], [[swarm-registrar#jsonfileregistrarnew]], [[swarm-registrar#jsonfileregistrarpath]]

### JsonFileRegistrar::new

```rust
pub fn new(path: impl Into<PathBuf>) -> Self
```

Create a registrar that persists to `path`.

Params: `path` — anything convertible into `PathBuf` (the registry file location;
need not exist yet, and intermediate directories are created on first write).
Returns: a `JsonFileRegistrar` bound to that path.

When to use: the single constructor; the caller always supplies the path (there is
no default location).

Related: [[swarm-registrar#jsonfileregistrar]], [[swarm-registrar#jsonfileregistrarpath]]

### JsonFileRegistrar::path

```rust
pub fn path(&self) -> &Path
```

The registry file path this registrar writes to.

Returns: a borrowed `&Path` to the configured location.

When to use: to report or log where registrations land, or to read the registry
file directly for inspection.

Related: [[swarm-registrar#jsonfileregistrarnew]]
