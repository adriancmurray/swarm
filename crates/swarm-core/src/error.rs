//! `RepoError` — thin error enum returned by all repo trait methods.
//!
//! Moved from `agent-swarm::repos` in P5-S1 (behavior-preserving copy; the
//! original is replaced by a `pub use swarm_core::RepoError` shim).

use std::fmt;
use std::path::PathBuf;

// ── RepoError ────────────────────────────────────────────────────────────────

/// Thin error enum returned by all repo trait methods.
///
/// `Io` and `Serialize` wrap their inner error as the `source()` chain.
/// Leaf variants (`NotFound`, `InvalidId`, `TooLarge`) have no source.
/// `Clone` is intentionally not derived — `io::Error` is not `Clone`.
#[derive(Debug)]
pub enum RepoError {
    Io(std::io::Error),
    Serialize(serde_json::Error),
    NotFound(String),
    InvalidId(String),
    TooLarge { path: PathBuf, bytes: usize },
}

impl fmt::Display for RepoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "io error: {e}"),
            Self::Serialize(e) => write!(f, "serialize error: {e}"),
            Self::NotFound(id) => write!(f, "not found: {id}"),
            Self::InvalidId(id) => write!(f, "invalid id: {id}"),
            Self::TooLarge { path, bytes } => {
                write!(f, "too large: {} ({bytes} bytes)", path.display())
            }
        }
    }
}

impl std::error::Error for RepoError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Serialize(e) => Some(e),
            Self::NotFound(_) | Self::InvalidId(_) | Self::TooLarge { .. } => None,
        }
    }
}

impl From<std::io::Error> for RepoError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<serde_json::Error> for RepoError {
    fn from(e: serde_json::Error) -> Self {
        Self::Serialize(e)
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_error_from_io() {
        let io = std::io::Error::new(std::io::ErrorKind::NotFound, "missing file");
        let e: RepoError = io.into();
        assert!(matches!(e, RepoError::Io(_)));
    }

    #[test]
    fn repo_error_from_serde() {
        let bad_json = serde_json::from_str::<serde_json::Value>("{bad").unwrap_err();
        let e: RepoError = bad_json.into();
        assert!(matches!(e, RepoError::Serialize(_)));
    }

    #[test]
    fn repo_error_source_io_has_source() {
        let io = std::io::Error::other("boom");
        let e = RepoError::Io(io);
        assert!(std::error::Error::source(&e).is_some());
    }

    #[test]
    fn repo_error_source_serialize_has_source() {
        let bad_json = serde_json::from_str::<serde_json::Value>("{bad").unwrap_err();
        let e = RepoError::Serialize(bad_json);
        assert!(std::error::Error::source(&e).is_some());
    }

    #[test]
    fn repo_error_source_leaf_variants_no_source() {
        assert!(std::error::Error::source(&RepoError::NotFound("x".into())).is_none());
        assert!(std::error::Error::source(&RepoError::InvalidId("x".into())).is_none());
        assert!(std::error::Error::source(&RepoError::TooLarge {
            path: PathBuf::from("/tmp/f"),
            bytes: 1,
        })
        .is_none());
    }

    #[test]
    fn repo_error_display_not_found() {
        let e = RepoError::NotFound("abc-123".into());
        assert_eq!(e.to_string(), "not found: abc-123");
    }

    #[test]
    fn repo_error_display_invalid_id() {
        let e = RepoError::InvalidId("bad-id".into());
        assert_eq!(e.to_string(), "invalid id: bad-id");
    }

    #[test]
    fn repo_error_display_too_large() {
        let e = RepoError::TooLarge {
            path: PathBuf::from("/tmp/events.jsonl"),
            bytes: 1_000_000,
        };
        let s = e.to_string();
        assert!(s.contains("too large"));
        assert!(s.contains("events.jsonl"));
        assert!(s.contains("1000000"));
    }

    #[test]
    fn repo_error_display_io() {
        let io = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied");
        let e = RepoError::Io(io);
        assert!(e.to_string().starts_with("io error:"));
    }
}
