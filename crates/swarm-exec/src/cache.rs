//! Content-addressed leaf cache for backend dispatch — the determinism spine
//! for the default (subprocess) backends, which expose no sampling seed.
//!
//! A clean, successful [`RunOutcome`] is keyed by a content hash of the request
//! (prompt + resolved spec + cwd + timeout). A cache hit replays that outcome at
//! zero token cost, so a run becomes reproducible. The 64-bit key is
//! collision-safe: every entry stores the full request fingerprint and a hit is
//! honored only when the fingerprint matches, so a hash collision degrades to a
//! miss (a live run), never to wrong output.
//!
//! **Placement.** This lives in `swarm-exec`, not `swarm-core`, because it stores
//! [`RunOutcome`], which is defined in `swarm-kernel` (above `swarm-core`/
//! `swarm-store` in the dependency DAG). It mirrors the File/Mem repo pattern
//! within this crate instead of being a `swarm-core` repo trait.
//!
//! The cache is wired into worker dispatch in [`crate::orchestration`] behind the
//! default-off `[reliability].cache` config key — worker prompts are
//! deterministic from `(task, role, context)`, so re-running the same swarm hits
//! the cache. (The manager prompt embeds stochastic worker output, so it is not
//! cached: its hit rate would be near zero.)

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use swarm_kernel::agent::{describe_spec, AgentSpec};
use swarm_kernel::backend_abi::RunOutcome;
use swarm_store::store::write_text_atomic;

use crate::executor::{FallbackAttempt, FallbackOutcome};

/// ASCII Unit Separator — joins fingerprint fields; cannot appear in a path,
/// spec id, or the integer timeout, so the fields can't run together.
const FIELD_SEP: char = '\u{1f}';

/// A cached successful run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedRun {
    /// Full request fingerprint. A hit is honored only when this matches, so a
    /// 64-bit key collision degrades to a miss rather than serving wrong output.
    pub fingerprint: String,
    /// `describe_spec` of the backend that actually produced the outcome.
    pub used_spec: String,
    /// The replayed outcome.
    pub outcome: RunOutcome,
}

/// Read-through cache of successful backend runs. `Send + Sync` so a single
/// instance can be shared across worker threads. Both impls are
/// interchangeable; [`MemCacheRepo`] is the deterministic test/replay backend.
pub trait CacheRepo: Send + Sync {
    fn get(&self, key: &str) -> Option<CachedRun>;
    fn put(&self, key: &str, run: &CachedRun);
}

/// Stable content key + fingerprint for a dispatch request.
///
/// Returns `(key, fingerprint)`: `key` is the FNV-1a/64 hex storage handle;
/// `fingerprint` is the full request material, stored with the entry and
/// re-checked on read for collision safety. Only inputs that actually change the
/// backend's output belong here — the prompt, the resolved primary spec, the
/// cwd, and the timeout.
pub fn cache_key(prompt: &str, primary: &AgentSpec, cwd: &Path, timeout_secs: u64) -> (String, String) {
    let fingerprint = format!(
        "{}{FIELD_SEP}{}{FIELD_SEP}{}{FIELD_SEP}{}",
        describe_spec(primary),
        cwd.display(),
        timeout_secs,
        prompt,
    );
    (fnv1a_hex(fingerprint.as_bytes()), fingerprint)
}

/// The cache-correctness rule: a run is cacheable only when it is a clean
/// success — not timed out, not flagged retryable, and either exit 0 (subprocess)
/// or no exit code (a non-process backend, e.g. HTTP). A transient / timed-out /
/// failed outcome is never stored, so it can never be served as a hit.
pub fn is_cacheable(outcome: &RunOutcome) -> bool {
    !outcome.timed_out && !outcome.retryable && matches!(outcome.exit_status, Some(0) | None)
}

/// Run `exec` through the cache: on a fingerprint-matched hit, return the stored
/// outcome at zero token cost; otherwise run `exec` and store the result when
/// [`is_cacheable`]. With `cache: None` this is a transparent pass-through
/// (byte-identical to not having a cache at all).
pub fn with_cache(
    cache: Option<&dyn CacheRepo>,
    key: &str,
    fingerprint: &str,
    primary: &AgentSpec,
    exec: impl FnOnce() -> FallbackOutcome,
) -> FallbackOutcome {
    if let Some(c) = cache {
        if let Some(hit) = c.get(key) {
            if hit.fingerprint == fingerprint {
                // A clean first-try success served from cache: one succeeded
                // attempt, so no retry/fallback reliability events fire.
                // ponytail: `used` reports the primary, not the cached fallback
                // spec — a hit performs no dispatch, so the distinction is
                // cosmetic for event emission. Upgrade: persist/parse the spec.
                return FallbackOutcome {
                    used: primary.clone(),
                    result: Ok(hit.outcome),
                    attempts: vec![FallbackAttempt {
                        spec: primary.clone(),
                        retries: 0,
                        succeeded: true,
                        reason: None,
                    }],
                };
            }
            // fingerprint mismatch => 64-bit key collision; fall through to a live run.
        }
    }
    let outcome = exec();
    if let Some(c) = cache {
        if let Ok(o) = &outcome.result {
            if is_cacheable(o) {
                c.put(
                    key,
                    &CachedRun {
                        fingerprint: fingerprint.to_string(),
                        used_spec: describe_spec(&outcome.used),
                        outcome: o.clone(),
                    },
                );
            }
        }
    }
    outcome
}

/// FNV-1a/64 as zero-padded hex. Dependency-free, mirroring the hash style in
/// `swarm-kernel/src/routing.rs`.
fn fnv1a_hex(bytes: &[u8]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        hash = (hash ^ u64::from(b)).wrapping_mul(0x100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// In-memory cache — the deterministic test/replay backend.
#[derive(Default)]
pub struct MemCacheRepo {
    entries: Mutex<std::collections::HashMap<String, CachedRun>>,
}

impl MemCacheRepo {
    pub fn new() -> Self {
        Self::default()
    }
}

impl CacheRepo for MemCacheRepo {
    fn get(&self, key: &str) -> Option<CachedRun> {
        self.entries.lock().unwrap().get(key).cloned()
    }
    fn put(&self, key: &str, run: &CachedRun) {
        self.entries
            .lock()
            .unwrap()
            .insert(key.to_string(), run.clone());
    }
}

/// File-backed cache: one JSON file per key under `dir`. Best-effort — a cache
/// read/write failure never fails the run (it degrades to a miss / no-store).
pub struct FileCacheRepo {
    dir: PathBuf,
}

impl FileCacheRepo {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    fn path(&self, key: &str) -> PathBuf {
        self.dir.join(format!("{key}.json"))
    }
}

impl CacheRepo for FileCacheRepo {
    fn get(&self, key: &str) -> Option<CachedRun> {
        let text = std::fs::read_to_string(self.path(key)).ok()?;
        serde_json::from_str(&text).ok()
    }
    fn put(&self, key: &str, run: &CachedRun) {
        if std::fs::create_dir_all(&self.dir).is_err() {
            return;
        }
        if let Ok(text) = serde_json::to_string(run) {
            // Atomic write so a concurrent reader never sees a torn entry.
            let _ = write_text_atomic(&self.path(key), &text);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use swarm_kernel::agent::{AgentChoice, AgentSpec};

    fn spec() -> AgentSpec {
        AgentSpec::builtin(AgentChoice::Codex, None)
    }

    fn ok_outcome(stdout: &str) -> RunOutcome {
        RunOutcome {
            exit_status: Some(0),
            stdout: stdout.to_string(),
            stderr: String::new(),
            timed_out: false,
            retryable: false,
            token_usage: None,
        }
    }

    fn fb_ok(stdout: &str) -> FallbackOutcome {
        FallbackOutcome {
            used: spec(),
            result: Ok(ok_outcome(stdout)),
            attempts: Vec::new(),
        }
    }

    #[test]
    fn is_cacheable_only_for_clean_success() {
        assert!(is_cacheable(&ok_outcome("x")));
        let mut o = ok_outcome("x");
        o.timed_out = true;
        assert!(!is_cacheable(&o), "timed out must not cache");
        let mut o = ok_outcome("x");
        o.retryable = true;
        assert!(!is_cacheable(&o), "retryable must not cache");
        let mut o = ok_outcome("x");
        o.exit_status = Some(1);
        assert!(!is_cacheable(&o), "nonzero exit must not cache");
        let mut o = ok_outcome("x");
        o.exit_status = None;
        assert!(is_cacheable(&o), "non-process success caches");
    }

    #[test]
    fn cache_key_is_stable_and_request_sensitive() {
        let (k1, f1) = cache_key("prompt", &spec(), Path::new("/tmp"), 60);
        let (k2, _) = cache_key("prompt", &spec(), Path::new("/tmp"), 60);
        assert_eq!(k1, k2, "same request => same key");
        let (k3, _) = cache_key("other prompt", &spec(), Path::new("/tmp"), 60);
        assert_ne!(k1, k3, "different prompt => different key");
        let (k4, _) = cache_key("prompt", &spec(), Path::new("/tmp"), 90);
        assert_ne!(k1, k4, "different timeout => different key");
        assert!(f1.contains("prompt"));
    }

    #[test]
    fn with_cache_none_runs_every_time() {
        let calls = Cell::new(0);
        let (k, f) = cache_key("p", &spec(), Path::new("/tmp"), 60);
        let _ = with_cache(None, &k, &f, &spec(), || {
            calls.set(calls.get() + 1);
            fb_ok("out")
        });
        let _ = with_cache(None, &k, &f, &spec(), || {
            calls.set(calls.get() + 1);
            fb_ok("out")
        });
        assert_eq!(calls.get(), 2, "no cache => always run");
    }

    #[test]
    fn with_cache_hit_skips_the_second_run() {
        let cache = MemCacheRepo::new();
        let (k, f) = cache_key("p", &spec(), Path::new("/tmp"), 60);
        let calls = Cell::new(0);
        let first = with_cache(Some(&cache), &k, &f, &spec(), || {
            calls.set(calls.get() + 1);
            fb_ok("cached-out")
        });
        assert!(first.result.is_ok());
        let second = with_cache(Some(&cache), &k, &f, &spec(), || {
            calls.set(calls.get() + 1);
            fb_ok("SHOULD-NOT-RUN")
        });
        assert_eq!(calls.get(), 1, "second call must hit the cache");
        assert_eq!(second.result.unwrap().stdout, "cached-out");
    }

    #[test]
    fn with_cache_does_not_store_timed_out() {
        let cache = MemCacheRepo::new();
        let (k, f) = cache_key("p", &spec(), Path::new("/tmp"), 60);
        let _ = with_cache(Some(&cache), &k, &f, &spec(), || {
            let mut o = ok_outcome("partial");
            o.timed_out = true;
            FallbackOutcome {
                used: spec(),
                result: Ok(o),
                attempts: Vec::new(),
            }
        });
        assert!(
            cache.get(&k).is_none(),
            "a timed-out outcome must never be cached"
        );
    }

    #[test]
    fn with_cache_collision_degrades_to_miss() {
        let cache = MemCacheRepo::new();
        // Pre-seed key "K" with fingerprint "A".
        cache.put(
            "K",
            &CachedRun {
                fingerprint: "A".to_string(),
                used_spec: "codex".to_string(),
                outcome: ok_outcome("A-out"),
            },
        );
        // Request the same key with a different fingerprint (simulated collision).
        let calls = Cell::new(0);
        let out = with_cache(Some(&cache), "K", "B", &spec(), || {
            calls.set(calls.get() + 1);
            fb_ok("B-out")
        });
        assert_eq!(calls.get(), 1, "fingerprint mismatch must run live");
        assert_eq!(out.result.unwrap().stdout, "B-out");
    }

    #[test]
    fn file_cache_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        let cache = FileCacheRepo::new(dir.path());
        let run = CachedRun {
            fingerprint: "fp".to_string(),
            used_spec: "codex".to_string(),
            outcome: ok_outcome("disk-out"),
        };
        cache.put("key1", &run);
        let got = cache.get("key1").expect("entry should be present");
        assert_eq!(got.outcome.stdout, "disk-out");
        assert_eq!(got.fingerprint, "fp");
        assert!(cache.get("missing").is_none());
    }
}
