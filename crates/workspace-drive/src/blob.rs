//! Synchronous, immutable local blobs for the SQLite drive authority.
//!
//! Call these methods on blocking threads, while holding the authority's SQLite
//! IMMEDIATE transaction. That transaction, not this adapter, serializes reads,
//! uploads and GC across processes. The root and its ancestors must be owned by
//! trusted workspace code: path checks reject existing symlinks, but the
//! path-based LocalFileSystem API cannot prevent hostile concurrent replacement.

use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io;
use std::ops::Range;
use std::path::{Component, Path, PathBuf};

use bytes::Bytes;
use futures::{TryStreamExt, executor::block_on};
use object_store::local::LocalFileSystem;
use object_store::path::Path as ObjectPath;
use object_store::{ObjectStore, ObjectStoreExt, PutMode, PutOptions};
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum BlobError {
    #[error("blob key must be a canonical lowercase hyphenated UUID: {0:?}")]
    InvalidKey(String),
    #[error("blob root contains a parent traversal or is empty: {0}")]
    InvalidRoot(PathBuf),
    #[error("blob path is a symlink or has an unexpected file type: {0}")]
    UnsafePath(PathBuf),
    #[error("blob range {start}..{end} is invalid for size {size}")]
    InvalidRange { start: usize, end: usize, size: u64 },
    #[error("blob filesystem operation failed for {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error(transparent)]
    Store(#[from] object_store::Error),
}

#[derive(Debug)]
pub struct BlobStore {
    root: PathBuf,
    store: LocalFileSystem,
    // Per-instance, test-only failures: no process-global state or production
    // configuration can turn successful I/O into a simulated failure.
    #[cfg(test)]
    fault: Option<TestFault>,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TestFault {
    PartialStagingWrite,
    UploadNoSpace,
    UploadPermissionDenied,
    FileSync,
    DirectorySync,
}

impl BlobStore {
    /// Configure an instance-local simulated failure for crate authority tests.
    /// This constructor is absent from production builds.
    #[cfg(test)]
    pub(super) fn with_fault(mut self, fault: TestFault) -> Self {
        self.fault = Some(fault);
        self
    }

    /// Create and durably link a flat blob directory, rejecting symlinks in
    /// every existing component (including ancestors of an absolute root).
    pub fn open(root: &Path) -> Result<Self, BlobError> {
        if root.as_os_str().is_empty()
            || root.components().any(|c| matches!(c, Component::ParentDir))
        {
            return Err(BlobError::InvalidRoot(root.to_owned()));
        }
        let root = if root.is_absolute() {
            root.to_owned()
        } else {
            io_at(root, std::env::current_dir())?.join(root)
        };
        check_directories(&root, true)?;
        let store = LocalFileSystem::new_with_prefix(&root)?;
        Ok(Self {
            root,
            store,
            #[cfg(test)]
            fault: None,
        })
    }

    /// Atomically create a new blob; an existing key is never overwritten.
    /// Success means the inode and its directory entry have been synced.
    /// A sync error can leave an unreferenced blob for authority GC to collect.
    pub fn put(&self, key: &str, data: &[u8]) -> Result<(), BlobError> {
        let location = key_path(key)?;
        self.check_root()?;
        self.check_blob(key)?;
        #[cfg(test)]
        self.fail_at(
            TestFault::UploadPermissionDenied,
            &self.root.join(format!("{key}#1")),
        )?;
        #[cfg(test)]
        if let Some(fault @ (TestFault::PartialStagingWrite | TestFault::UploadNoSpace)) =
            self.fault
        {
            // Simulate the ObjectStore boundary failing after a real partial
            // staging write, before publishing a final pathname. Leave the
            // artifact as a crashed upload or failed best-effort cleanup would.
            use std::io::Write;
            let path = self.root.join(format!("{key}#1"));
            let mut file = io_at(
                &path,
                fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&path),
            )?;
            io_at(&path, file.write_all(&data[..data.len() / 2]))?;
            self.fail_at(fault, &path)?;
        }
        block_on(self.store.put_opts(
            &location,
            Bytes::copy_from_slice(data).into(),
            PutOptions {
                mode: PutMode::Create,
                ..Default::default()
            },
        ))?;

        // object_store 0.13.2 has no with_fsync option. Its put_opts writes a
        // create_new staging file, publishes with hard_link for Create, and
        // best-effort unlinks staging, without syncing either inode or parent.
        // Sync the published inode BEFORE its directory and BEFORE returning
        // to the SQLite transaction. No cooperating reader sees it before then.
        let path = self.root.join(key);
        let file = io_at(&path, File::open(&path))?;
        #[cfg(test)]
        self.fail_at(TestFault::FileSync, &path)?;
        io_at(&path, file.sync_all())?;
        self.sync_root()
    }

    /// Return the actual blob length, not the length of a requested range.
    /// The authority must compare this with its expected total length before
    /// read/search, even for an empty range or zero-length metadata. Hold the
    /// same SQLite IMMEDIATE transaction through size validation and reading.
    pub fn size(&self, key: &str) -> Result<u64, BlobError> {
        let location = key_path(key)?;
        self.check_root()?;
        self.check_blob(key)?;
        Ok(block_on(self.store.head(&location))?.size)
    }

    /// Read an exact half-open range. Empty ranges at EOF are valid; reversed
    /// ranges and ranges extending past EOF are rejected, not truncated.
    pub fn read(&self, key: &str, range: Range<usize>) -> Result<Vec<u8>, BlobError> {
        let location = key_path(key)?;
        self.check_root()?;
        self.check_blob(key)?;
        let size = block_on(self.store.head(&location))?.size;
        if range.start > range.end || range.end as u64 > size {
            return Err(BlobError::InvalidRange {
                start: range.start,
                end: range.end,
                size,
            });
        }
        if range.is_empty() {
            return Ok(Vec::new());
        }
        Ok(block_on(
            self.store
                .get_range(&location, range.start as u64..range.end as u64),
        )?
        .to_vec())
    }

    /// Durably remove a blob. Missing blobs are also synced before success,
    /// allowing GC to safely retry after an earlier directory-sync failure.
    pub fn delete(&self, key: &str) -> Result<(), BlobError> {
        let location = key_path(key)?;
        self.check_root()?;
        self.check_blob(key)?;
        match block_on(self.store.delete(&location)) {
            Ok(()) | Err(object_store::Error::NotFound { .. }) => self.sync_root(),
            Err(error) => Err(error.into()),
        }
    }

    /// Reclaim at most `limit` crashed-upload staging files. The caller MUST
    /// hold the SQLite IMMEDIATE lock: otherwise an in-flight put's staging
    /// file could be removed. Foreign names and final blobs are never deleted.
    /// A scan can visit the entire flat directory; memory usage is constant.
    /// Success includes a directory sync even when there was nothing to remove,
    /// so a retry after a previous sync failure still establishes durability.
    pub fn cleanup_staging(&self, limit: usize) -> Result<usize, BlobError> {
        self.check_root()?;
        let mut removed = 0;
        if limit > 0 {
            for entry in io_at(&self.root, fs::read_dir(&self.root))? {
                let entry = io_at(&self.root, entry)?;
                let path = entry.path();
                let metadata = io_at(&path, fs::symlink_metadata(&path))?;
                if metadata.is_symlink() {
                    return Err(BlobError::UnsafePath(path));
                }
                let name = entry.file_name();
                if !name.to_str().is_some_and(is_staging_name) {
                    continue;
                }
                if !metadata.is_file() {
                    return Err(BlobError::UnsafePath(path));
                }
                // ObjectStore refuses paths with '#digits'; reclaim its
                // recognized staging artifacts directly, without following links.
                io_at(&path, fs::remove_file(&path))?;
                removed += 1;
                if removed == limit {
                    break;
                }
            }
        }
        self.sync_root()?;
        Ok(removed)
    }

    /// List canonical blob keys in ascending lexical order, strictly after the
    /// cursor. Staging files and foreign regular files are never exposed.
    /// The full local directory must still be traversed: ObjectStore does not
    /// promise ordering. Retained candidates use O(limit) memory (plus the
    /// LocalFileSystem stream's fixed-size, 1024-entry Tokio buffer).
    pub fn list(&self, after: Option<&str>, limit: usize) -> Result<Vec<String>, BlobError> {
        if let Some(after) = after {
            key_path(after)?;
        }
        self.check_root()?;
        if limit == 0 {
            return Ok(Vec::new());
        }
        // LocalFileSystem recursively lists and follows even out-of-root
        // symlinks. Validate this flat directory before giving it to the store.
        for entry in io_at(&self.root, fs::read_dir(&self.root))? {
            let entry = io_at(&self.root, entry)?;
            let path = entry.path();
            let metadata = io_at(&path, fs::symlink_metadata(&path))?;
            if !metadata.is_file() {
                return Err(BlobError::UnsafePath(path));
            }
        }
        block_on(async {
            let mut objects = self.store.list(None);
            let mut keys = BTreeSet::new();
            while let Some(object) = objects.try_next().await? {
                let key = object.location.to_string();
                if key_path(&key).is_err() || after.is_some_and(|after| key.as_str() <= after) {
                    continue;
                }
                if keys.len() == limit {
                    if keys.last().is_some_and(|last| &key >= last) {
                        continue;
                    }
                    keys.pop_last();
                }
                keys.insert(key);
            }
            Ok(keys.into_iter().collect())
        })
    }

    fn sync_root(&self) -> Result<(), BlobError> {
        #[cfg(test)]
        self.fail_at(TestFault::DirectorySync, &self.root)?;
        sync_directory(&self.root)
    }

    #[cfg(test)]
    fn fail_at(&self, point: TestFault, path: &Path) -> Result<(), BlobError> {
        if self.fault == Some(point) {
            let kind = match point {
                TestFault::UploadNoSpace => io::ErrorKind::StorageFull,
                TestFault::UploadPermissionDenied => io::ErrorKind::PermissionDenied,
                _ => io::ErrorKind::Other,
            };
            return io_at(
                path,
                Err(io::Error::new(kind, format!("injected {point:?} failure"))),
            );
        }
        Ok(())
    }

    fn check_root(&self) -> Result<(), BlobError> {
        check_directories(&self.root, false)
    }

    fn check_blob(&self, key: &str) -> Result<(), BlobError> {
        let path = self.root.join(key);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_file() => Ok(()),
            Ok(_) => Err(BlobError::UnsafePath(path)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(BlobError::Io { path, source }),
        }
    }
}

fn key_path(key: &str) -> Result<ObjectPath, BlobError> {
    let uuid = Uuid::parse_str(key).map_err(|_| BlobError::InvalidKey(key.to_owned()))?;
    if uuid.hyphenated().to_string() != key {
        return Err(BlobError::InvalidKey(key.to_owned()));
    }
    Ok(ObjectPath::from(key))
}

fn is_staging_name(name: &str) -> bool {
    let Some((key, suffix)) = name.split_once('#') else {
        return false;
    };
    // object_store 0.13.2 local.rs::new_staged_upload starts an inferred i32
    // at 1 and uses counter.to_string(); staged_upload_path appends '#suffix'.
    // Restrict to names this adapter's puts can generate, not the broader
    // '#digits' pattern that LocalFileSystem merely hides during listing.
    key_path(key).is_ok()
        && suffix
            .parse::<i32>()
            .is_ok_and(|counter| counter > 0 && counter.to_string() == suffix)
}

fn io_at<T>(path: &Path, result: io::Result<T>) -> Result<T, BlobError> {
    result.map_err(|source| BlobError::Io {
        path: path.to_owned(),
        source,
    })
}

fn check_directories(root: &Path, create: bool) -> Result<(), BlobError> {
    let mut path = PathBuf::new();
    for component in root.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => path.push(component),
            Component::CurDir => continue,
            Component::ParentDir => return Err(BlobError::InvalidRoot(root.to_owned())),
            Component::Normal(name) => path.push(name),
        }
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => return Err(BlobError::UnsafePath(path)),
            Err(error) if create && error.kind() == io::ErrorKind::NotFound => {
                match fs::create_dir(&path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                    Err(source) => return Err(BlobError::Io { path, source }),
                }
                // A concurrent opener may have won create_dir. Never accept
                // its entry without checking the type without following links.
                let metadata = io_at(&path, fs::symlink_metadata(&path))?;
                if !metadata.is_dir() {
                    return Err(BlobError::UnsafePath(path));
                }
            }
            Err(source) => return Err(BlobError::Io { path, source }),
        }
        if create {
            // Sync existing directories too: a previous failed open may have
            // created one but failed before persisting its parent link.
            sync_directory(&path)?;
            if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                sync_directory(parent)?;
            }
        }
    }
    Ok(())
}

fn sync_directory(path: &Path) -> Result<(), BlobError> {
    // Do not silently weaken durability on platforms/filesystems that cannot
    // open and sync directories: open/put/delete must propagate that failure.
    let directory = io_at(path, File::open(path))?;
    io_at(path, directory.sync_all())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn key() -> String {
        Uuid::now_v7().to_string()
    }

    #[test]
    fn put_existing_key_preserves_original_bytes_after_reopen() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("nested/blobs");
        let blob = key();
        let store = BlobStore::open(&root).unwrap();
        store.put(&blob, b"original").unwrap();
        assert!(matches!(
            store.put(&blob, b"replacement"),
            Err(BlobError::Store(object_store::Error::AlreadyExists { .. }))
        ));
        let reopened = BlobStore::open(&root).unwrap();
        assert_eq!(reopened.read(&blob, 0..8).unwrap(), b"original");
    }

    #[test]
    fn read_returns_exact_ranges_and_rejects_out_of_bounds() {
        let dir = tempdir().unwrap();
        let store = BlobStore::open(dir.path()).unwrap();
        let blob = key();
        store.put(&blob, b"abcdef").unwrap();
        assert_eq!(store.read(&blob, 2..5).unwrap(), b"cde");
        assert_eq!(store.read(&blob, 6..6).unwrap(), b"");
        for (start, end) in [(4, 2), (0, 7), (7, 7)] {
            assert!(matches!(
                store.read(&blob, start..end),
                Err(BlobError::InvalidRange { .. })
            ));
        }
        let empty = key();
        store.put(&empty, b"").unwrap();
        assert_eq!(store.read(&empty, 0..0).unwrap(), b"");
        assert!(matches!(
            store.read(&key(), 0..0),
            Err(BlobError::Store(object_store::Error::NotFound { .. }))
        ));
    }

    #[test]
    fn noncanonical_keys_are_rejected_by_every_boundary() {
        let dir = tempdir().unwrap();
        let store = BlobStore::open(dir.path()).unwrap();
        let canonical = "019abcdef012-7000-8000-000000000001";
        for invalid in [
            "",
            "../escape",
            "/absolute",
            "a/b",
            "not-a-uuid",
            canonical,
            "019abcdef01270008000000000000001",
            "019abcde-f012-7000-8000-000000000001/child",
            "019ABCDE-F012-7000-8000-000000000001",
            "urn:uuid:019abcde-f012-7000-8000-000000000001",
        ] {
            assert!(
                matches!(store.put(invalid, b"data"), Err(BlobError::InvalidKey(_))),
                "{invalid}"
            );
            assert!(
                matches!(store.read(invalid, 0..1), Err(BlobError::InvalidKey(_))),
                "{invalid}"
            );
            assert!(
                matches!(store.size(invalid), Err(BlobError::InvalidKey(_))),
                "{invalid}"
            );
            assert!(
                matches!(store.delete(invalid), Err(BlobError::InvalidKey(_))),
                "{invalid}"
            );
            assert!(
                matches!(store.list(Some(invalid), 0), Err(BlobError::InvalidKey(_))),
                "{invalid}"
            );
        }
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn list_is_sorted_exclusive_and_hides_staging_and_foreign_files() {
        let dir = tempdir().unwrap();
        let store = BlobStore::open(dir.path()).unwrap();
        let keys = [
            "019abcde-f012-7000-8000-000000000001",
            "019abcde-f012-7000-8000-000000000002",
            "019abcde-f012-7000-8000-000000000003",
        ];
        for blob in keys.iter().rev() {
            store.put(blob, b"data").unwrap();
        }
        fs::write(dir.path().join(format!("{}#1", keys[0])), b"partial").unwrap();
        fs::write(dir.path().join("foreign"), b"data").unwrap();
        assert_eq!(store.list(None, 2).unwrap(), keys[..2]);
        assert_eq!(store.list(Some(keys[0]), 1).unwrap(), keys[1..2]);
        assert_eq!(store.list(Some(keys[1]), 10).unwrap(), keys[2..]);
        assert!(store.list(Some(keys[2]), 10).unwrap().is_empty());
        assert!(store.list(None, 0).unwrap().is_empty());
    }

    #[test]
    fn delete_is_idempotent_and_absent_after_reopen() {
        let dir = tempdir().unwrap();
        let blob = key();
        let store = BlobStore::open(dir.path()).unwrap();
        store.put(&blob, b"data").unwrap();
        store.delete(&blob).unwrap();
        store.delete(&blob).unwrap();
        let reopened = BlobStore::open(dir.path()).unwrap();
        assert!(reopened.list(None, 10).unwrap().is_empty());
        assert!(matches!(
            reopened.read(&blob, 0..1),
            Err(BlobError::Store(object_store::Error::NotFound { .. }))
        ));
    }

    #[test]
    fn root_rejects_files_empty_paths_and_parent_traversal() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("file");
        fs::write(&file, b"data").unwrap();
        assert!(matches!(
            BlobStore::open(&file),
            Err(BlobError::UnsafePath(_))
        ));
        assert!(matches!(
            BlobStore::open(&file.join("blobs")),
            Err(BlobError::UnsafePath(_))
        ));
        assert!(matches!(
            BlobStore::open(Path::new("")),
            Err(BlobError::InvalidRoot(_))
        ));
        assert!(matches!(
            BlobStore::open(&dir.path().join("../blobs")),
            Err(BlobError::InvalidRoot(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn root_rejects_symlink_components_including_dangling_links() {
        use std::os::unix::fs::symlink;
        let dir = tempdir().unwrap();
        let target = dir.path().join("target");
        fs::create_dir(&target).unwrap();
        let link = dir.path().join("link");
        symlink(&target, &link).unwrap();
        assert!(matches!(
            BlobStore::open(&link),
            Err(BlobError::UnsafePath(_))
        ));
        assert!(matches!(
            BlobStore::open(&link.join("blobs")),
            Err(BlobError::UnsafePath(_))
        ));
        let dangling = dir.path().join("dangling");
        symlink(dir.path().join("missing"), &dangling).unwrap();
        assert!(matches!(
            BlobStore::open(&dangling),
            Err(BlobError::UnsafePath(_))
        ));
        assert!(!target.join("blobs").exists());
    }

    #[cfg(unix)]
    #[test]
    fn blob_symlink_is_rejected_without_reading_or_mutating_target() {
        use std::os::unix::fs::symlink;
        let dir = tempdir().unwrap();
        let root = dir.path().join("blobs");
        let store = BlobStore::open(&root).unwrap();
        let target = dir.path().join("target");
        fs::write(&target, b"secret").unwrap();
        let blob = key();
        symlink(&target, root.join(&blob)).unwrap();
        assert!(matches!(
            store.read(&blob, 0..6),
            Err(BlobError::UnsafePath(_))
        ));
        assert!(matches!(
            store.put(&blob, b"changed"),
            Err(BlobError::UnsafePath(_))
        ));
        assert!(matches!(store.delete(&blob), Err(BlobError::UnsafePath(_))));
        assert!(matches!(
            store.list(None, 10),
            Err(BlobError::UnsafePath(_))
        ));
        assert!(matches!(store.size(&blob), Err(BlobError::UnsafePath(_))));
        assert!(matches!(
            store.cleanup_staging(10),
            Err(BlobError::UnsafePath(_))
        ));
        assert_eq!(fs::read(&target).unwrap(), b"secret");
    }

    #[cfg(unix)]
    #[test]
    fn replaced_root_symlink_is_rejected_by_existing_store() {
        use std::os::unix::fs::symlink;
        let dir = tempdir().unwrap();
        let root = dir.path().join("blobs");
        let store = BlobStore::open(&root).unwrap();
        let moved = dir.path().join("moved");
        fs::rename(&root, &moved).unwrap();
        symlink(&moved, &root).unwrap();
        let blob = key();
        assert!(matches!(
            store.put(&blob, b"data"),
            Err(BlobError::UnsafePath(_))
        ));
        assert!(matches!(
            store.read(&blob, 0..0),
            Err(BlobError::UnsafePath(_))
        ));
        assert!(matches!(store.delete(&blob), Err(BlobError::UnsafePath(_))));
        assert!(matches!(
            store.list(None, 10),
            Err(BlobError::UnsafePath(_))
        ));
        assert!(matches!(store.size(&blob), Err(BlobError::UnsafePath(_))));
        assert!(matches!(
            store.cleanup_staging(10),
            Err(BlobError::UnsafePath(_))
        ));
        assert_eq!(fs::read_dir(&moved).unwrap().count(), 0);
    }

    #[test]
    fn list_rejects_subdirectories_instead_of_traversing_them() {
        let dir = tempdir().unwrap();
        let store = BlobStore::open(dir.path()).unwrap();
        fs::create_dir(dir.path().join("nested")).unwrap();
        assert!(matches!(
            store.list(None, 10),
            Err(BlobError::UnsafePath(_))
        ));
    }

    #[test]
    fn independent_store_creates_have_exactly_one_winner() {
        use std::sync::Barrier;
        let dir = tempdir().unwrap();
        let first = BlobStore::open(dir.path()).unwrap();
        let second = BlobStore::open(dir.path()).unwrap();
        let blob = key();
        let barrier = Barrier::new(2);
        let results = std::thread::scope(|scope| {
            let one = scope.spawn(|| {
                barrier.wait();
                first.put(&blob, b"first")
            });
            let two = scope.spawn(|| {
                barrier.wait();
                second.put(&blob, b"second")
            });
            [one.join().unwrap(), two.join().unwrap()]
        });
        assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
        let loser = results.iter().find(|r| r.is_err()).unwrap();
        assert!(matches!(
            loser,
            Err(BlobError::Store(object_store::Error::AlreadyExists { .. }))
        ));
        let expected: &[u8] = if results[0].is_ok() {
            b"first"
        } else {
            b"second"
        };
        assert_eq!(first.read(&blob, 0..expected.len()).unwrap(), expected);
    }

    #[test]
    fn small_list_pages_choose_earliest_keys_across_full_directory() {
        let dir = tempdir().unwrap();
        let store = BlobStore::open(dir.path()).unwrap();
        let keys: Vec<_> = (1..=37).map(|id| Uuid::from_u128(id).to_string()).collect();
        for blob in keys.iter().rev() {
            fs::write(dir.path().join(blob), b"data").unwrap();
        }
        assert_eq!(store.list(None, 3).unwrap(), keys[..3]);
        assert_eq!(store.list(Some(&keys[17]), 3).unwrap(), keys[18..21]);
        assert_eq!(store.list(Some(&keys[35]), 3).unwrap(), keys[36..]);
    }

    #[test]
    fn staging_cleanup_is_bounded_and_preserves_final_blobs_and_foreign_names() {
        let dir = tempdir().unwrap();
        let store = BlobStore::open(dir.path()).unwrap();
        let referenced = "019abcde-f012-7000-8000-000000000001".to_owned();
        store.put(&referenced, b"referenced data").unwrap();
        let linked_stage = format!("{referenced}#1");
        fs::hard_link(dir.path().join(&referenced), dir.path().join(&linked_stage)).unwrap();
        let stages = [
            linked_stage,
            format!("{}#2", key()),
            format!("{}#2147483647", key()),
        ];
        for stage in &stages[1..] {
            fs::write(dir.path().join(stage), b"partial").unwrap();
        }
        let foreign = [
            "foreign#1".to_owned(),
            format!("{}#1", referenced.to_uppercase()),
            format!("{referenced}#0"),
            format!("{referenced}#01"),
            format!("{referenced}#+1"),
            format!("{referenced}#-1"),
            format!("{referenced}#2147483648"),
            format!("{referenced}#"),
            format!("{referenced}#1.extra"),
            format!("{referenced}#1#2"),
        ];
        for name in &foreign {
            fs::write(dir.path().join(name), b"foreign data").unwrap();
        }
        assert_eq!(store.cleanup_staging(0).unwrap(), 0);
        assert!(stages.iter().all(|name| dir.path().join(name).exists()));
        for remaining in (0..3).rev() {
            assert_eq!(store.cleanup_staging(1).unwrap(), 1);
            assert_eq!(
                stages
                    .iter()
                    .filter(|name| dir.path().join(name).exists())
                    .count(),
                remaining
            );
        }
        assert_eq!(store.cleanup_staging(10).unwrap(), 0);
        assert_eq!(store.read(&referenced, 0..15).unwrap(), b"referenced data");
        assert_eq!(store.list(None, 10).unwrap(), vec![referenced.clone()]);
        for name in &foreign {
            assert_eq!(
                fs::read(dir.path().join(name)).unwrap(),
                b"foreign data",
                "{name}"
            );
        }
        let reopened = BlobStore::open(dir.path()).unwrap();
        assert_eq!(reopened.size(&referenced).unwrap(), 15);
    }

    #[cfg(unix)]
    #[test]
    fn staging_cleanup_rejects_symlinks_without_unlinking_or_following_them() {
        use std::os::unix::fs::symlink;
        let dir = tempdir().unwrap();
        let target = dir.path().join("target");
        fs::write(&target, b"outside").unwrap();
        for name in [format!("{}#1", key()), "foreign-link".to_owned()] {
            let root = tempdir().unwrap();
            let store = BlobStore::open(root.path()).unwrap();
            let path = root.path().join(name);
            symlink(&target, &path).unwrap();
            assert!(matches!(
                store.cleanup_staging(10),
                Err(BlobError::UnsafePath(_))
            ));
            assert!(fs::symlink_metadata(&path).unwrap().is_symlink());
        }
        let root = tempdir().unwrap();
        let store = BlobStore::open(root.path()).unwrap();
        symlink(
            dir.path().join("missing"),
            root.path().join(format!("{}#1", key())),
        )
        .unwrap();
        assert!(matches!(
            store.cleanup_staging(10),
            Err(BlobError::UnsafePath(_))
        ));
        assert_eq!(fs::read(target).unwrap(), b"outside");
    }

    #[test]
    fn staging_cleanup_rejects_recognized_directories_and_preserves_foreign_directories() {
        let dir = tempdir().unwrap();
        let store = BlobStore::open(dir.path()).unwrap();
        let foreign = dir.path().join("foreign-directory");
        fs::create_dir(&foreign).unwrap();
        assert_eq!(store.cleanup_staging(10).unwrap(), 0);
        assert!(foreign.is_dir());
        let recognized = dir.path().join(format!("{}#1", key()));
        fs::create_dir(&recognized).unwrap();
        assert!(matches!(
            store.cleanup_staging(10),
            Err(BlobError::UnsafePath(_))
        ));
        assert!(recognized.is_dir());
    }

    #[test]
    fn size_reports_total_length_even_for_empty_or_short_range_reads() {
        let dir = tempdir().unwrap();
        let store = BlobStore::open(dir.path()).unwrap();
        let blob = key();
        store.put(&blob, b"data").unwrap();
        assert_eq!(store.read(&blob, 0..0).unwrap(), b"");
        assert_eq!(store.read(&blob, 0..2).unwrap(), b"da");
        assert_eq!(store.size(&blob).unwrap(), 4);
        // External corruption: range output alone cannot reveal truncation if
        // the selected range is empty. The authority can compare total sizes.
        fs::write(dir.path().join(&blob), b"").unwrap();
        assert_eq!(store.read(&blob, 0..0).unwrap(), b"");
        assert_eq!(store.size(&blob).unwrap(), 0);
        assert!(matches!(
            store.size(&key()),
            Err(BlobError::Store(object_store::Error::NotFound { .. }))
        ));
    }

    #[test]
    fn partial_staging_write_failure_never_reports_success_or_publishes_blob() {
        let dir = tempdir().unwrap();
        let mut store = BlobStore::open(dir.path()).unwrap();
        let blob = key();
        store.fault = Some(TestFault::PartialStagingWrite);
        assert!(matches!(
            store.put(&blob, b"abcdef"),
            Err(BlobError::Io { .. })
        ));
        assert_eq!(
            fs::read(dir.path().join(format!("{blob}#1"))).unwrap(),
            b"abc"
        );
        assert!(!dir.path().join(&blob).exists());
        let reopened = BlobStore::open(dir.path()).unwrap();
        assert!(reopened.list(None, 10).unwrap().is_empty());
        assert!(matches!(
            reopened.size(&blob),
            Err(BlobError::Store(object_store::Error::NotFound { .. }))
        ));
        assert_eq!(reopened.cleanup_staging(1).unwrap(), 1);
        reopened.put(&blob, b"abcdef").unwrap();
        assert_eq!(reopened.read(&blob, 0..6).unwrap(), b"abcdef");
    }

    #[test]
    fn simulated_upload_failures_preserve_error_kind_and_do_not_publish_blobs() {
        for (fault, expected_kind, staging_count) in [
            (TestFault::UploadNoSpace, io::ErrorKind::StorageFull, 1),
            (
                TestFault::UploadPermissionDenied,
                io::ErrorKind::PermissionDenied,
                0,
            ),
        ] {
            let dir = tempdir().unwrap();
            let store = BlobStore::open(dir.path()).unwrap().with_fault(fault);
            let blob = key();
            match store.put(&blob, b"abcdef").unwrap_err() {
                BlobError::Io { source, .. } => assert_eq!(source.kind(), expected_kind),
                error => panic!("unexpected {fault:?} error: {error:?}"),
            }
            assert!(!dir.path().join(&blob).exists());
            assert!(store.list(None, 10).unwrap().is_empty());
            if staging_count > 0 {
                assert_eq!(
                    fs::read(dir.path().join(format!("{blob}#1"))).unwrap(),
                    b"abc"
                );
            }
            assert_eq!(store.cleanup_staging(10).unwrap(), staging_count);
        }
    }

    #[test]
    fn put_sync_failures_return_errors_even_when_final_file_exists() {
        for fault in [TestFault::FileSync, TestFault::DirectorySync] {
            let dir = tempdir().unwrap();
            let mut store = BlobStore::open(dir.path()).unwrap();
            let blob = key();
            store.fault = Some(fault);
            assert!(
                matches!(store.put(&blob, b"data"), Err(BlobError::Io { .. })),
                "{fault:?}"
            );
            // Publication is not a successful durable upload. It may leave an
            // unreferenced final file, which the authority's GC can delete.
            assert_eq!(fs::read(dir.path().join(&blob)).unwrap(), b"data");
            let reopened = BlobStore::open(dir.path()).unwrap();
            reopened.delete(&blob).unwrap();
            assert!(!dir.path().join(&blob).exists());
        }
    }

    #[test]
    fn delete_sync_failure_returns_error_and_missing_file_retry_syncs() {
        let dir = tempdir().unwrap();
        let mut store = BlobStore::open(dir.path()).unwrap();
        let blob = key();
        store.put(&blob, b"data").unwrap();
        store.fault = Some(TestFault::DirectorySync);
        assert!(matches!(store.delete(&blob), Err(BlobError::Io { .. })));
        assert!(!dir.path().join(&blob).exists());
        assert!(matches!(store.delete(&blob), Err(BlobError::Io { .. })));
        store.fault = None;
        store.delete(&blob).unwrap();
    }

    #[test]
    fn staging_cleanup_sync_failure_returns_error_and_empty_retry_syncs() {
        let dir = tempdir().unwrap();
        let mut store = BlobStore::open(dir.path()).unwrap();
        let stage = dir.path().join(format!("{}#1", key()));
        fs::write(&stage, b"partial").unwrap();
        store.fault = Some(TestFault::DirectorySync);
        assert!(matches!(
            store.cleanup_staging(1),
            Err(BlobError::Io { .. })
        ));
        assert!(!stage.exists());
        assert!(matches!(
            store.cleanup_staging(1),
            Err(BlobError::Io { .. })
        ));
        store.fault = None;
        assert_eq!(store.cleanup_staging(1).unwrap(), 0);
    }

    #[test]
    fn synchronous_methods_work_on_tokio_blocking_threads() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .build()
            .unwrap();
        runtime.block_on(async {
            tokio::task::spawn_blocking(|| {
                let dir = tempdir().unwrap();
                let store = BlobStore::open(dir.path()).unwrap();
                let blob = key();
                store.put(&blob, b"data").unwrap();
                assert_eq!(store.read(&blob, 0..4).unwrap(), b"data");
                assert_eq!(store.list(None, 1).unwrap(), vec![blob.clone()]);
                store.delete(&blob).unwrap();
            })
            .await
            .unwrap();
        });
    }
}
