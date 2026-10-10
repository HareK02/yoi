//! Identity-fenced access adapter for the existing read/edit/write engine.
//!
//! Descriptor-rooted resolution is Linux-only and symlinks/mount crossings fail
//! closed. The final compare and atomic rename are NOT a kernel CAS against an
//! arbitrary external editor: external editors must cooperate with provider
//! serialization. A hostile rename in that last syscall gap is not excluded.
use crate::{AtomicWriteMode, FsAccessPolicy};
use std::fs::{File, Metadata};
use std::io::{Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// Identity and state, deliberately not just a content digest. atime is omitted
/// so reading does not invalidate an observation. No host paths are encoded.
pub fn identity_validator(metadata: &Metadata) -> std::io::Result<Vec<u8>> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let mut bytes = b"yoi-checkout-v1".to_vec();
        for value in [
            metadata.dev(),
            metadata.ino(),
            metadata.len(),
            metadata.mode() as u64,
            metadata.mtime() as u64,
            metadata.mtime_nsec() as u64,
            metadata.ctime() as u64,
            metadata.ctime_nsec() as u64,
        ] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        Ok(bytes)
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "identity validators unavailable",
        ))
    }
}

/// One pinned target and its save parent. Retained through the shared operation.
pub struct CheckedTarget<'a> {
    policy: &'a dyn FsAccessPolicy,
    root: File,
    relative: PathBuf,
    logical: PathBuf,
    file: File,
    parent: Option<File>,
    validator: Vec<u8>,
    effects_possible: AtomicBool,
}
impl<'a> CheckedTarget<'a> {
    pub fn pin(
        root: &File,
        relative: &Path,
        logical: &Path,
        policy: &'a dyn FsAccessPolicy,
        write: bool,
    ) -> std::io::Result<Self> {
        policy.check_cancelled()?;
        if !(if write {
            policy.is_writable_paths(logical, logical)
        } else {
            policy.is_readable_paths(logical, logical)
        }) {
            return Err(crate::FsDenialReason::CheckoutTargetDenied.into_io_error());
        }
        let parent = if relative.as_os_str().is_empty() {
            None
        } else {
            Some(crate::open_beneath_no_symlinks_at(
                root,
                relative.parent().unwrap_or(Path::new("")),
            )?)
        };
        let file = if let Some(parent) = &parent {
            crate::open_beneath_no_symlinks_at(parent, Path::new(relative.file_name().unwrap()))?
        } else {
            crate::open_beneath_no_symlinks_at(root, Path::new(""))?
        };
        let metadata = file.metadata()?;
        if !metadata.is_file() && !metadata.is_dir() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "checkout requires regular file or directory",
            ));
        }
        let result = Self {
            policy,
            root: root.try_clone()?,
            relative: relative.to_path_buf(),
            logical: logical.to_path_buf(),
            file,
            parent,
            validator: identity_validator(&metadata)?,
            effects_possible: AtomicBool::new(false),
        };
        result.confirm()?;
        Ok(result)
    }
    pub fn validator(&self) -> &[u8] {
        &self.validator
    }
    pub fn metadata(&self) -> std::io::Result<Metadata> {
        self.file.metadata()
    }
    pub fn effects_possible(&self) -> bool {
        self.effects_possible.load(Ordering::Acquire)
    }
    pub fn confirm(&self) -> std::io::Result<()> {
        self.policy.check_cancelled()?;
        if let Some(parent) = &self.parent {
            let named_parent = crate::open_beneath_no_symlinks_at(
                &self.root,
                self.relative.parent().unwrap_or(Path::new("")),
            )?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                let pinned_metadata = parent.metadata()?;
                let named_metadata = named_parent.metadata()?;
                if (pinned_metadata.dev(), pinned_metadata.ino())
                    != (named_metadata.dev(), named_metadata.ino())
                {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::WouldBlock,
                        "checkout parent identity changed",
                    ));
                }
            }
        }
        let named = crate::open_beneath_no_symlinks_at(&self.root, &self.relative)?;
        if identity_validator(&named.metadata()?)? != self.validator
            || identity_validator(&self.file.metadata()?)? != self.validator
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                "checkout observation changed",
            ));
        }
        Ok(())
    }
    /// Refresh a directory observation after Create, retaining the original
    /// pinned target/root/parent handles. A replacement named directory cannot
    /// supply the validator for effects performed in the original directory.
    pub fn refresh_directory_observation(&mut self) -> std::io::Result<()> {
        let metadata = self.file.metadata()?;
        if !metadata.is_dir() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Create target must be directory",
            ));
        }
        self.validator = identity_validator(&metadata)?;
        self.confirm()
    }

    /// Adapter for no-clobber creation beneath a pinned directory. Both target
    /// and destination authorization remain in the provider's normal policy.
    pub fn create_access<'b>(&'b self, suffix: PathBuf, logical: PathBuf) -> CheckedCreate<'b, 'a> {
        CheckedCreate {
            target: self,
            suffix,
            logical,
        }
    }
}
impl FsAccessPolicy for CheckedTarget<'_> {
    fn is_readable(&self, p: &Path) -> bool {
        self.policy.is_readable(p)
    }
    fn is_writable(&self, p: &Path) -> bool {
        self.policy.is_writable(p)
    }
    fn is_readable_paths(&self, p: &Path, r: &Path) -> bool {
        p == self.logical && r == p && self.policy.is_readable_paths(p, r)
    }
    fn is_writable_paths(&self, p: &Path, r: &Path) -> bool {
        p == self.logical && r == p && self.policy.is_writable_paths(p, r)
    }
    fn check_cancelled(&self) -> std::io::Result<()> {
        self.policy.check_cancelled()
    }
    fn resolve_access_path(&self, p: &Path) -> std::io::Result<PathBuf> {
        Ok(p.to_path_buf())
    }
    fn read_metadata(&self, _: &Path, _: &Path) -> std::io::Result<Metadata> {
        self.file.metadata()
    }
    fn open_read_file(&self, _: &Path, _: &Path) -> std::io::Result<File> {
        self.confirm()?;
        let mut file = self.file.try_clone()?;
        file.seek(SeekFrom::Start(0))?;
        Ok(file)
    }
    fn max_write_bytes(&self) -> Option<usize> {
        Some(
            self.policy
                .max_write_bytes()
                .unwrap_or(8 * 1024 * 1024)
                .min(8 * 1024 * 1024),
        )
    }
    fn max_edit_replacements(&self) -> Option<usize> {
        Some(
            self.policy
                .max_edit_replacements()
                .unwrap_or(10_000)
                .min(10_000),
        )
    }
    fn atomic_write_file(
        &self,
        _: &Path,
        _: &Path,
        content: &[u8],
        mode: AtomicWriteMode,
    ) -> std::io::Result<()> {
        self.confirm()?;
        let parent = self.parent.as_ref().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "cannot save root")
        })?;
        crate::atomic_write_beneath_no_symlinks_at(
            parent,
            Path::new(self.relative.file_name().unwrap()),
            content,
            mode,
            || {
                self.confirm()?;
                // From here rename/encoding/post-observation failures have an unknown outcome.
                self.effects_possible.store(true, Ordering::Release);
                Ok(())
            },
        )
    }
}

pub struct CheckedCreate<'b, 'a> {
    target: &'b CheckedTarget<'a>,
    suffix: PathBuf,
    logical: PathBuf,
}
impl FsAccessPolicy for CheckedCreate<'_, '_> {
    fn is_readable(&self, p: &Path) -> bool {
        self.target.policy.is_readable(p)
    }
    fn is_writable(&self, p: &Path) -> bool {
        p == self.logical && self.target.policy.is_writable_paths(p, p)
    }
    fn resolve_access_path(&self, p: &Path) -> std::io::Result<PathBuf> {
        Ok(p.to_path_buf())
    }
    fn check_cancelled(&self) -> std::io::Result<()> {
        self.target.policy.check_cancelled()
    }
    fn max_write_bytes(&self) -> Option<usize> {
        self.target.max_write_bytes()
    }
    fn read_metadata(&self, _: &Path, _: &Path) -> std::io::Result<Metadata> {
        match crate::open_beneath_no_symlinks_at(&self.target.file, &self.suffix) {
            Ok(_) => Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "Create destination exists",
            )),
            Err(e) => Err(e),
        }
    }
    fn atomic_write_file(
        &self,
        _: &Path,
        _: &Path,
        content: &[u8],
        mode: AtomicWriteMode,
    ) -> std::io::Result<()> {
        self.target.confirm()?;
        if mode != AtomicWriteMode::CreateNew || !self.target.file.metadata()?.is_dir() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Create requires directory",
            ));
        }
        // Creating parents must not turn a narrow destination grant into
        // permission to populate ungranted intermediate directories.
        let mut parent = self.target.logical.clone();
        let mut components = self.suffix.components().peekable();
        while let Some(component) = components.next() {
            parent.push(component.as_os_str());
            if components.peek().is_some()
                && !self.target.policy.is_writable_paths(&parent, &parent)
            {
                return Err(crate::FsDenialReason::CheckoutCreateParentDenied.into_io_error());
            }
        }
        // Nested mkdir may have effects before failure; never auto-retry.
        self.target.effects_possible.store(true, Ordering::Release);
        crate::atomic_write_beneath_no_symlinks_at(
            &self.target.file,
            &self.suffix,
            content,
            mode,
            || self.target.policy.check_cancelled(),
        )
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use crate::{FsError, FsPath, ReadRequest, WriteRequest};
    use sha2::{Digest, Sha256};
    use std::sync::atomic::AtomicUsize;
    struct Access {
        path: PathBuf,
        counter: AtomicUsize,
        trigger: usize,
        replacement: bool,
    }
    impl FsAccessPolicy for Access {
        fn is_readable(&self, _: &Path) -> bool {
            true
        }
        fn is_writable(&self, _: &Path) -> bool {
            true
        }
        fn check_cancelled(&self) -> std::io::Result<()> {
            if self.counter.fetch_add(1, Ordering::SeqCst) + 1 == self.trigger {
                if self.replacement {
                    std::fs::rename(&self.path, self.path.with_extension("old"))?;
                    std::fs::write(&self.path, "replacement")?;
                } else {
                    std::fs::write(&self.path, "updated")?;
                }
            }
            Ok(())
        }
    }
    #[test]
    fn checked_save_boundary_rejects_name_substitution_without_mutating_replacement() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("a");
        std::fs::write(&path, "original").unwrap();
        let policy = Access {
            path: path.clone(),
            counter: AtomicUsize::new(0),
            trigger: 4,
            replacement: true,
        };
        let handle = crate::open_root_no_symlinks(root.path()).unwrap();
        let checked = CheckedTarget::pin(&handle, Path::new("a"), &path, &policy, true).unwrap();
        policy.counter.store(0, Ordering::SeqCst);
        let error = crate::run_write(
            root.path(),
            WriteRequest {
                path: FsPath::new("a").unwrap(),
                content: b"worker".to_vec(),
                expected_hash: Some(Sha256::digest(b"original").into()),
            },
            &checked,
        )
        .unwrap_err();
        assert!(
            matches!(error, FsError::Io { source, .. } if source.kind() == std::io::ErrorKind::WouldBlock)
        );
        assert!(!checked.effects_possible());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "replacement");
        assert_eq!(
            std::fs::read_to_string(path.with_extension("old")).unwrap(),
            "original"
        );
    }
    #[test]
    fn checked_read_midread_name_replacement_never_pairs_old_bytes_with_new_validator() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("a");
        std::fs::write(&path, "original").unwrap();
        let policy = Access {
            path: path.clone(),
            counter: AtomicUsize::new(0),
            trigger: 3,
            replacement: true,
        };
        let handle = crate::open_root_no_symlinks(root.path()).unwrap();
        let checked = CheckedTarget::pin(&handle, Path::new("a"), &path, &policy, false).unwrap();
        let validator = checked.validator().to_vec();
        policy.counter.store(0, Ordering::SeqCst);
        let result = crate::run_read(
            root.path(),
            ReadRequest {
                path: FsPath::new("a").unwrap(),
                offset: 0,
                limit: 20,
                max_bytes: 1024,
            },
            &checked,
        );
        match result {
            Err(FsError::Conflict(_)) => {}
            Ok(read) => {
                // Same pinned inode supplies bytes and hash. Execute's final
                // named-target check must reject, not refresh to replacement.
                assert_eq!(read.bytes, b"original");
                let expected_hash: crate::ContentHash = Sha256::digest(b"original").into();
                assert_eq!(read.content_hash, expected_hash);
                assert_eq!(
                    checked.confirm().unwrap_err().kind(),
                    std::io::ErrorKind::WouldBlock
                );
            }
            other => panic!("unexpected Read result: {other:?}"),
        }
        assert_eq!(checked.validator(), validator);
        assert_ne!(
            identity_validator(&std::fs::metadata(&path).unwrap()).unwrap(),
            validator
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "replacement");
    }

    #[test]
    fn create_post_parent_observation_retains_bound_directory_and_rejects_substitution() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("d");
        std::fs::create_dir(&directory).unwrap();
        let policy = Access {
            path: directory.clone(),
            counter: AtomicUsize::new(0),
            trigger: usize::MAX,
            replacement: false,
        };
        let handle = crate::open_root_no_symlinks(root.path()).unwrap();
        let mut checked =
            CheckedTarget::pin(&handle, Path::new("d"), &directory, &policy, true).unwrap();
        let initial = checked.validator().to_vec();
        let create = checked.create_access(PathBuf::from("new"), directory.join("new"));
        crate::run_write(
            root.path(),
            WriteRequest {
                path: FsPath::new("d/new").unwrap(),
                content: b"new".to_vec(),
                expected_hash: None,
            },
            &create,
        )
        .unwrap();
        checked.refresh_directory_observation().unwrap();
        assert_ne!(initial, checked.validator());
        assert_eq!(
            checked.validator(),
            identity_validator(&std::fs::metadata(&directory).unwrap()).unwrap()
        );
        std::fs::rename(&directory, root.path().join("old")).unwrap();
        std::fs::create_dir(&directory).unwrap();
        assert_eq!(
            checked.refresh_directory_observation().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        assert!(checked.effects_possible());
    }

    #[test]
    fn checked_parent_swap_is_rejected_even_with_a_hardlinked_same_inode_target() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("d");
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("a");
        std::fs::write(&path, "original").unwrap();
        let policy = Access {
            path: path.clone(),
            counter: AtomicUsize::new(0),
            trigger: usize::MAX,
            replacement: false,
        };
        let handle = crate::open_root_no_symlinks(root.path()).unwrap();
        let checked = CheckedTarget::pin(&handle, Path::new("d/a"), &path, &policy, true).unwrap();
        std::fs::rename(&directory, root.path().join("old")).unwrap();
        std::fs::create_dir(&directory).unwrap();
        std::fs::hard_link(root.path().join("old/a"), &path).unwrap();
        assert_eq!(
            checked.confirm().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        assert!(!checked.effects_possible());
    }

    #[test]
    fn shared_text_read_rejects_mid_read_metadata_change() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("a");
        std::fs::write(&path, "original").unwrap();
        let policy = Access {
            path,
            counter: AtomicUsize::new(0),
            trigger: 2,
            replacement: false,
        };
        let error = crate::run_read(
            root.path(),
            ReadRequest {
                path: FsPath::new("a").unwrap(),
                offset: 0,
                limit: 20,
                max_bytes: 1024,
            },
            &policy,
        )
        .unwrap_err();
        assert!(matches!(error, FsError::Conflict(_)));
    }
}
