//! Read-before-edit policy tracker for the builtin file-manipulation tools.
//!
//! A `ReadTracker` records a SHA-256 hash of each file's contents at the
//! moment it was observed via the `Read` tool, and lets `Write` / `Edit`
//! later verify that the file has not been externally modified since then.
//!
//! # Lifetime
//!
//! A `ReadTracker` is **session-scoped**: the Pod layer creates a fresh
//! instance at the start of each agent session and discards it when the
//! session ends. The `ScopedFs` write boundary, by contrast, is
//! pod-lifetime (derived from the manifest). The two are orthogonal and
//! the Pod wires them together when registering builtin tools.
//!
//! ```no_run
//! # use manifest::Scope;
//! # use tools::{ScopedFs, ReadTracker, builtin_tools};
//! let scope = Scope::new("/workspace").unwrap();
//! let fs = ScopedFs::new(scope);        // pod lifetime
//! let tracker = ReadTracker::new();     // session lifetime
//! let defs = builtin_tools(fs, tracker);
//! ```

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use sha2::{Digest, Sha256};

use crate::error::ToolsError;

/// Fixed-size content hash recorded per file.
type ContentHash = [u8; 32];

fn hash_bytes(bytes: &[u8]) -> ContentHash {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher.finalize().into()
}

/// Canonical-path keyed record of which files have been observed and at
/// what content hash.
///
/// Cheap to clone: internally an `Arc<Mutex<HashMap<...>>>`, so sharing a
/// `ReadTracker` across every builtin tool in a session is effectively
/// free and keeps their views consistent.
#[derive(Debug, Clone, Default)]
pub struct ReadTracker {
    inner: Arc<Mutex<HashMap<PathBuf, ContentHash>>>,
}

impl ReadTracker {
    /// Create an empty tracker. Typically called once per session.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record that `path` has been observed with the given content bytes.
    ///
    /// Called by the `Read` tool after a successful read, and by the
    /// `Write` / `Edit` tools after a successful modification (so that
    /// subsequent edits see a clean history).
    pub fn record(&self, path: &Path, bytes: &[u8]) {
        let key = canonicalize_or_owned(path);
        let hash = hash_bytes(bytes);
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key, hash);
    }

    /// Verify that `path` was previously recorded and its current bytes
    /// match the recorded hash.
    ///
    /// - If the path has no history entry, returns [`ToolsError::NotRead`].
    /// - If the current content hashes differ from the recorded value,
    ///   returns [`ToolsError::ExternallyModified`].
    pub fn verify(&self, path: &Path, current_bytes: &[u8]) -> Result<(), ToolsError> {
        let key = canonicalize_or_owned(path);
        let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let recorded = guard
            .get(&key)
            .ok_or_else(|| ToolsError::NotRead(path.to_path_buf()))?;
        let current = hash_bytes(current_bytes);
        if *recorded != current {
            return Err(ToolsError::ExternallyModified(path.to_path_buf()));
        }
        Ok(())
    }

    /// Returns true if `path` has a history entry. Test-only.
    #[cfg(test)]
    pub(crate) fn has(&self, path: &Path) -> bool {
        let key = canonicalize_or_owned(path);
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(&key)
    }

    /// Number of distinct files in the history. Test-only.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len()
    }
}

fn canonicalize_or_owned(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn record_then_verify_clean_ok() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("a.txt");
        fs::write(&file, b"hello").unwrap();

        let tracker = ReadTracker::new();
        tracker.record(&file, b"hello");
        assert!(tracker.has(&file));
        assert_eq!(tracker.len(), 1);
        tracker.verify(&file, b"hello").unwrap();
    }

    #[test]
    fn verify_without_record_returns_not_read() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("a.txt");
        fs::write(&file, b"x").unwrap();

        let tracker = ReadTracker::new();
        let err = tracker.verify(&file, b"x").unwrap_err();
        assert!(matches!(err, ToolsError::NotRead(_)));
    }

    #[test]
    fn verify_mismatch_returns_externally_modified() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("a.txt");
        fs::write(&file, b"original").unwrap();

        let tracker = ReadTracker::new();
        tracker.record(&file, b"original");
        let err = tracker.verify(&file, b"tampered").unwrap_err();
        assert!(matches!(err, ToolsError::ExternallyModified(_)));
    }

    #[test]
    fn record_overwrites_previous_hash() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("a.txt");
        fs::write(&file, b"v1").unwrap();

        let tracker = ReadTracker::new();
        tracker.record(&file, b"v1");
        tracker.record(&file, b"v2");
        tracker.verify(&file, b"v2").unwrap();
        assert!(tracker.verify(&file, b"v1").is_err());
    }

    #[test]
    fn canonical_keys_collapse_symlink_variants() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let dir = TempDir::new().unwrap();
            let real = dir.path().join("real.txt");
            fs::write(&real, b"data").unwrap();
            let link = dir.path().join("link.txt");
            symlink(&real, &link).unwrap();

            let tracker = ReadTracker::new();
            tracker.record(&real, b"data");
            // Looking up via the symlink should hit the same entry.
            tracker.verify(&link, b"data").unwrap();
            // Exactly one entry.
            assert_eq!(tracker.len(), 1);
        }
    }

    #[test]
    fn clone_shares_state() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("a.txt");
        fs::write(&file, b"x").unwrap();

        let t1 = ReadTracker::new();
        let t2 = t1.clone();
        t1.record(&file, b"x");
        t2.verify(&file, b"x").unwrap();
    }

    #[test]
    fn empty_bytes_hash_stable() {
        let tracker = ReadTracker::new();
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("empty.txt");
        fs::write(&file, b"").unwrap();

        tracker.record(&file, b"");
        tracker.verify(&file, b"").unwrap();
        assert!(tracker.verify(&file, b"x").is_err());
    }
}
