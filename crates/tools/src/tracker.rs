//! Worker-lifetime tracker for file operations performed by the builtin
//! file-manipulation tools.
//!
//! A `Tracker` serves two orthogonal purposes:
//!
//! 1. **Read-before-edit policy.** It records a SHA-256 hash of each
//!    file's contents at the moment it was observed via `Read` (or
//!    mutated via `Write` / `Edit`), and lets `Write` / `Edit` later
//!    verify that the file has not been externally modified since then.
//!
//! 2. **Recency of touched files.** It keeps an LRU-ordered list of
//!    files that have been touched by any of the tools, so the Worker
//!    layer can ask "which files did the agent recently look at?" —
//!    used e.g. as a default reference set passed to context compaction.
//!
//! Despite its historic name, the Tracker already watches all three of
//! Read / Write / Edit; the rename away from `ReadTracker` reflects this.
//!
//! # Lifetime
//!
//! A `Tracker` is **Worker-process scoped**: the Worker layer creates a fresh
//! instance at the start of each Worker run (including resume) and discards
//! it when the process exits — it is not persisted, so a resumed
//! conversation starts with an empty read/edit history. The local WorkdirSession
//! scope boundary is likewise Worker-process scoped (derived from the
//! manifest). The two are orthogonal and the Worker wires them together
//! when registering builtin tools.
//!
//! ```no_run
//! # use std::path::PathBuf;
//! # use std::sync::Arc;
//! # use manifest::Scope;
//! # use tools::{Tracker, core_builtin_tools};
//! # use workdir::{LocalWorkdirSession, WorkdirSessionHandle};
//! let scope = Scope::writable("/workspace").unwrap();
//! let session: WorkdirSessionHandle = Arc::new(LocalWorkdirSession::new(
//!     scope,
//!     PathBuf::from("/workspace"),
//! ));
//! let tracker = Tracker::new(); // session lifetime
//! let bash_outputs = PathBuf::from("/run/yoi/bash-output");
//! let defs = core_builtin_tools(session, tracker, bash_outputs);
//! ```

use std::collections::{HashMap, VecDeque};
use std::path::{Component, Path, PathBuf};

use llm_engine::tool::ToolExecutionContext;
use std::sync::{Arc, Mutex};

use sha2::{Digest, Sha256};

use crate::error::ToolsError;

/// Fixed-size content hash recorded per file.
type ContentHash = [u8; 32];

/// How many distinct paths the recency list keeps before evicting the
/// least-recently-touched entry.
const RECENCY_CAPACITY: usize = 20;

fn hash_bytes(bytes: &[u8]) -> ContentHash {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher.finalize().into()
}

#[derive(Debug, Clone, Default)]
struct FileMutationCoordinator {
    locks: Arc<tokio::sync::Mutex<HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>>,
}

pub(crate) struct FileMutationPermit {
    _guard: tokio::sync::OwnedMutexGuard<()>,
}

impl FileMutationCoordinator {
    async fn acquire(&self, path: &Path) -> FileMutationPermit {
        let key = file_mutation_key(path);
        let lock = {
            let mut locks = self.locks.lock().await;
            locks
                .entry(key)
                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
                .clone()
        };
        FileMutationPermit {
            _guard: lock.lock_owned().await,
        }
    }
}

fn file_mutation_key(path: &Path) -> PathBuf {
    if let Ok(canonical) = path.canonicalize() {
        return canonical;
    }
    if let (Some(parent), Some(file_name)) = (path.parent(), path.file_name())
        && let Ok(canonical_parent) = parent.canonicalize()
    {
        return canonical_parent.join(file_name);
    }
    normalize_path_lexically(path)
}

fn normalize_path_lexically(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    normalized.push(component.as_os_str());
                }
            }
            Component::Normal(part) => normalized.push(part),
            Component::RootDir | Component::Prefix(_) => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ChangeStat {
    pub added: u64,
    pub deleted: u64,
}

#[derive(Debug, Default)]
struct Inner {
    /// Hash of each file's last observed contents, keyed by canonical path.
    hashes: HashMap<PathBuf, ContentHash>,
    /// Line count paired with observations that included the file content.
    line_counts: HashMap<PathBuf, usize>,
    /// LRU list of touched files. Front = most recently touched.
    recency: VecDeque<PathBuf>,
    /// Successful Write/Edit mutations attributed to this session's tools.
    change_stat: ChangeStat,
}

/// Canonical-path keyed tracker of file observations and their recency.
///
/// Cheap to clone: internally an `Arc<Mutex<Inner>>`, so sharing a
/// `Tracker` across every builtin tool in a session is effectively free
/// and keeps their views consistent.
#[derive(Debug, Clone, Default)]
pub struct Tracker {
    inner: Arc<Mutex<Inner>>,
    mutations: FileMutationCoordinator,
}

impl Tracker {
    /// Create an empty tracker. Typically called once per session.
    pub fn new() -> Self {
        Self::default()
    }

    /// Acquire the per-target-file mutation guard shared by `Write` and `Edit`.
    ///
    /// The guard is keyed by canonical target path where possible so equivalent
    /// paths serialize through the same lock. Engine still executes tool calls in
    /// parallel; this only gates the critical filesystem mutation section for
    /// builtin file mutation tools.
    pub(crate) async fn acquire_mutation(
        &self,
        path: &Path,
        ctx: &ToolExecutionContext,
    ) -> FileMutationPermit {
        tracing::debug!(
            batch_id = %ctx.batch_id,
            call_index = ctx.call_index,
            "acquire file mutation guard"
        );
        self.mutations.acquire(path).await
    }

    /// Record that `path` has been observed with the given content bytes.
    ///
    /// Called by the `Read` tool after a successful read, and by the
    /// `Write` / `Edit` tools after a successful modification (so that
    /// subsequent edits see a clean history).
    ///
    /// Also bumps `path` to the front of the recency list. If the list
    /// grows past [`RECENCY_CAPACITY`], the oldest entry is evicted.
    pub fn record(&self, path: &Path, bytes: &[u8]) {
        let key = canonicalize_or_owned(path);
        let hash = hash_bytes(bytes);
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.hashes.insert(key.clone(), hash);

        // LRU bump: remove an existing entry for this path then push to
        // the front. We intentionally compare by the canonical key so
        // symlink/real-path pairs collapse into a single slot.
        inner.recency.retain(|p| p != &key);
        inner.recency.push_front(key);
        if inner.recency.len() > RECENCY_CAPACITY {
            inner.recency.pop_back();
        }
    }

    pub fn record_workdir_content(&self, path: &workdir::WorkdirPath, content: &[u8]) {
        let key = PathBuf::from(path.as_str());
        let hash = hash_bytes(content);
        let line_count = String::from_utf8_lossy(content).lines().count();
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.line_counts.insert(key.clone(), line_count);
        inner.hashes.insert(key.clone(), hash);
        inner.recency.retain(|candidate| candidate != &key);
        inner.recency.push_front(key);
        if inner.recency.len() > RECENCY_CAPACITY {
            inner.recency.pop_back();
        }
    }

    pub fn observed_workdir_line_count(&self, path: &workdir::WorkdirPath) -> Option<usize> {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .line_counts
            .get(Path::new(path.as_str()))
            .copied()
    }

    pub fn record_workdir_hash(&self, path: &workdir::WorkdirPath, hash: workdir::ContentHash) {
        let key = PathBuf::from(path.as_str());
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.hashes.insert(key.clone(), hash);
        inner.recency.retain(|candidate| candidate != &key);
        inner.recency.push_front(key);
        if inner.recency.len() > RECENCY_CAPACITY {
            inner.recency.pop_back();
        }
    }

    /// Record a successful, session-attributable source mutation.
    ///
    /// Callers supply line counts derived from the exact replacement accepted
    /// by a Write/Edit tool. Bash and external process mutations are excluded
    /// because this tracker cannot attribute them to one tool operation
    /// authoritatively.
    pub fn record_change(&self, added: usize, deleted: usize) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.change_stat.added = inner.change_stat.added.saturating_add(added as u64);
        inner.change_stat.deleted = inner.change_stat.deleted.saturating_add(deleted as u64);
    }

    pub fn record_workdir_edit(
        &self,
        path: &workdir::WorkdirPath,
        hash: workdir::ContentHash,
        replacements: usize,
        added_lines_per_replacement: usize,
        deleted_lines_per_replacement: usize,
    ) {
        let added = added_lines_per_replacement.saturating_mul(replacements);
        let deleted = deleted_lines_per_replacement.saturating_mul(replacements);
        let key = PathBuf::from(path.as_str());
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.change_stat.added = inner.change_stat.added.saturating_add(added as u64);
        inner.change_stat.deleted = inner.change_stat.deleted.saturating_add(deleted as u64);
        if let Some(line_count) = inner.line_counts.get_mut(&key) {
            *line_count = line_count.saturating_sub(deleted).saturating_add(added);
        }
        inner.hashes.insert(key.clone(), hash);
        inner.recency.retain(|candidate| candidate != &key);
        inner.recency.push_front(key);
        if inner.recency.len() > RECENCY_CAPACITY {
            inner.recency.pop_back();
        }
    }

    pub fn change_stat(&self) -> ChangeStat {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .change_stat
    }

    pub fn expected_workdir_hash(
        &self,
        path: &workdir::WorkdirPath,
    ) -> Result<workdir::ContentHash, ToolsError> {
        let key = PathBuf::from(path.as_str());
        self.inner
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .hashes
            .get(&key)
            .copied()
            .ok_or_else(|| ToolsError::NotRead(key))
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
            .hashes
            .get(&key)
            .ok_or_else(|| ToolsError::NotRead(path.to_path_buf()))?;
        let current = hash_bytes(current_bytes);
        if *recorded != current {
            return Err(ToolsError::ExternallyModified(path.to_path_buf()));
        }
        Ok(())
    }

    /// Return up to `n` most recently touched file paths, most-recent first.
    ///
    /// Intended for callers like the Worker's context-compaction path, which
    /// wants to know which files the agent has been working with so it
    /// can pass them as default references to the compaction worker.
    pub fn recent_files(&self, n: usize) -> Vec<PathBuf> {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.recency.iter().take(n).cloned().collect()
    }

    /// Returns true if `path` has a history entry. Test-only.
    #[cfg(test)]
    pub(crate) fn has(&self, path: &Path) -> bool {
        let key = canonicalize_or_owned(path);
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .hashes
            .contains_key(&key)
    }

    /// Number of distinct files in the history. Test-only.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .hashes
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

        let tracker = Tracker::new();
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

        let tracker = Tracker::new();
        let err = tracker.verify(&file, b"x").unwrap_err();
        assert!(matches!(err, ToolsError::NotRead(_)));
    }

    #[test]
    fn verify_mismatch_returns_externally_modified() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("a.txt");
        fs::write(&file, b"original").unwrap();

        let tracker = Tracker::new();
        tracker.record(&file, b"original");
        let err = tracker.verify(&file, b"tampered").unwrap_err();
        assert!(matches!(err, ToolsError::ExternallyModified(_)));
    }

    #[test]
    fn record_overwrites_previous_hash() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("a.txt");
        fs::write(&file, b"v1").unwrap();

        let tracker = Tracker::new();
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

            let tracker = Tracker::new();
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

        let t1 = Tracker::new();
        let t2 = t1.clone();
        t1.record(&file, b"x");
        t2.verify(&file, b"x").unwrap();
    }

    #[test]
    fn empty_bytes_hash_stable() {
        let tracker = Tracker::new();
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("empty.txt");
        fs::write(&file, b"").unwrap();

        tracker.record(&file, b"");
        tracker.verify(&file, b"").unwrap();
        assert!(tracker.verify(&file, b"x").is_err());
    }

    // --- recency ---

    #[test]
    fn recent_files_returns_in_lru_order() {
        let dir = TempDir::new().unwrap();
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        let c = dir.path().join("c.txt");
        fs::write(&a, b"").unwrap();
        fs::write(&b, b"").unwrap();
        fs::write(&c, b"").unwrap();

        let tracker = Tracker::new();
        tracker.record(&a, b"");
        tracker.record(&b, b"");
        tracker.record(&c, b"");

        let recent = tracker.recent_files(10);
        // Most recent first.
        assert_eq!(recent.len(), 3);
        assert!(recent[0].ends_with("c.txt"));
        assert!(recent[1].ends_with("b.txt"));
        assert!(recent[2].ends_with("a.txt"));
    }

    #[test]
    fn recent_files_respects_n_limit() {
        let dir = TempDir::new().unwrap();
        let tracker = Tracker::new();
        for i in 0..5 {
            let p = dir.path().join(format!("f{i}.txt"));
            fs::write(&p, b"").unwrap();
            tracker.record(&p, b"");
        }
        assert_eq!(tracker.recent_files(3).len(), 3);
        assert_eq!(tracker.recent_files(0).len(), 0);
        assert_eq!(tracker.recent_files(100).len(), 5);
    }

    #[test]
    fn re_recording_moves_entry_to_front() {
        let dir = TempDir::new().unwrap();
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        let c = dir.path().join("c.txt");
        fs::write(&a, b"").unwrap();
        fs::write(&b, b"").unwrap();
        fs::write(&c, b"").unwrap();

        let tracker = Tracker::new();
        tracker.record(&a, b"");
        tracker.record(&b, b"");
        tracker.record(&c, b"");
        // Touching `a` again promotes it to the front.
        tracker.record(&a, b"");

        let recent = tracker.recent_files(10);
        assert_eq!(recent.len(), 3);
        assert!(recent[0].ends_with("a.txt"));
        assert!(recent[1].ends_with("c.txt"));
        assert!(recent[2].ends_with("b.txt"));
    }

    #[test]
    fn recency_capacity_evicts_oldest() {
        let dir = TempDir::new().unwrap();
        let tracker = Tracker::new();
        // Record one more than the capacity.
        for i in 0..(RECENCY_CAPACITY + 5) {
            let p = dir.path().join(format!("f{i:02}.txt"));
            fs::write(&p, b"").unwrap();
            tracker.record(&p, b"");
        }

        let recent = tracker.recent_files(RECENCY_CAPACITY + 100);
        assert_eq!(recent.len(), RECENCY_CAPACITY);
        // Newest-first: f24 down to f05. f00..f04 must be evicted.
        assert!(recent[0].ends_with(&format!("f{:02}.txt", RECENCY_CAPACITY + 4)));
        let last = recent.last().unwrap();
        assert!(last.ends_with("f05.txt"), "oldest surviving: {last:?}");
        // The evicted oldest ones must not appear.
        for i in 0..5 {
            let name = format!("f{i:02}.txt");
            assert!(recent.iter().all(|p| !p.ends_with(&name)));
        }
    }

    #[test]
    fn change_stat_saturates_and_accumulates_tracked_mutations() {
        let tracker = Tracker::new();
        tracker.record_change(7, 3);
        tracker.record_change(5, 2);

        assert_eq!(
            tracker.change_stat(),
            ChangeStat {
                added: 12,
                deleted: 5,
            }
        );
    }

    #[tokio::test]
    async fn mutation_guard_blocks_equivalent_paths_until_drop() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("target.txt");
        fs::write(&file, "x").unwrap();
        let equivalent = dir.path().join("sub").join("..").join("target.txt");
        fs::create_dir(dir.path().join("sub")).unwrap();
        let tracker = Tracker::new();
        let first = tracker
            .acquire_mutation(&file, &ToolExecutionContext::new("a", "batch", 0))
            .await;

        let second_ctx = ToolExecutionContext::new("b", "batch", 1);
        let second = tracker.acquire_mutation(&equivalent, &second_ctx);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(10), second)
                .await
                .is_err()
        );

        drop(first);
        tracker
            .acquire_mutation(&equivalent, &ToolExecutionContext::new("b", "batch", 1))
            .await;
    }

    #[tokio::test]
    async fn mutation_guard_does_not_block_different_files() {
        let dir = tempfile::tempdir().unwrap();
        let first_file = dir.path().join("a.txt");
        let second_file = dir.path().join("b.txt");
        fs::write(&first_file, "a").unwrap();
        fs::write(&second_file, "b").unwrap();
        let tracker = Tracker::new();
        let _first = tracker
            .acquire_mutation(&first_file, &ToolExecutionContext::new("a", "batch", 0))
            .await;

        tokio::time::timeout(
            std::time::Duration::from_millis(100),
            tracker.acquire_mutation(&second_file, &ToolExecutionContext::new("b", "batch", 1)),
        )
        .await
        .expect("different files should not share a mutation guard");
    }
}
