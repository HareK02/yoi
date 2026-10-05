//! Bounded descriptor-provider walk shared by Glob and Grep. Unlike a
//! WalkBuilder predicate, this iterator counts even rejected entries and checks
//! cancellation while scanning them. Denied directories are never enumerated.
use crate::{FsAccessPolicy, FsError, FsTraversalFilter};
use std::path::{Path, PathBuf};

pub(crate) struct Entry {
    pub path: PathBuf,
    pub kind: Option<std::fs::FileType>,
}
pub(crate) enum Walk<'a> {
    Plain(ignore::Walk),
    Descriptor(DescriptorWalk<'a>),
}
impl<'a> Walk<'a> {
    pub fn descriptor(root: &Path, access: &'a dyn FsAccessPolicy) -> Self {
        Self::Descriptor(DescriptorWalk {
            access,
            filter: access.traversal_filter(),
            pending: Some((root.to_path_buf(), 0)),
            root: Some(root.to_path_buf()),
            stack: Vec::new(),
            visited: 0,
            failed: false,
        })
    }
}
impl Iterator for Walk<'_> {
    type Item = Result<Entry, FsError>;
    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Plain(walk) => loop {
                match walk.next()? {
                    Ok(entry) => {
                        return Some(Ok(Entry {
                            path: entry.path().to_path_buf(),
                            kind: entry.file_type(),
                        }));
                    }
                    Err(_) => continue,
                }
            },
            Self::Descriptor(walk) => walk.next(),
        }
    }
}
pub(crate) struct DescriptorWalk<'a> {
    access: &'a dyn FsAccessPolicy,
    filter: Option<FsTraversalFilter>,
    root: Option<PathBuf>,
    pending: Option<(PathBuf, usize)>,
    stack: Vec<(PathBuf, usize, std::fs::ReadDir)>,
    visited: usize,
    failed: bool,
}
impl DescriptorWalk<'_> {
    fn next_entry(&mut self) -> Result<Option<Entry>, FsError> {
        if let Some(root) = self.root.take() {
            self.access
                .check_cancelled()
                .map_err(|e| FsError::io(&root, e))?;
            let resolved = self
                .access
                .resolve_access_path(&root)
                .map_err(|e| FsError::io(&root, e))?;
            let metadata = self
                .access
                .read_metadata(&root, &resolved)
                .map_err(|e| FsError::io(&root, e))?;
            return Ok(Some(Entry {
                path: root,
                kind: Some(metadata.file_type()),
            }));
        }
        loop {
            if let Some((path, depth)) = self.pending.take() {
                self.access
                    .check_cancelled()
                    .map_err(|e| FsError::io(&path, e))?;
                let selected = self
                    .filter
                    .as_ref()
                    .is_none_or(|filter| filter(&path, true));
                let resolved = self
                    .access
                    .resolve_access_path(&path)
                    .map_err(|e| FsError::io(&path, e))?;
                if selected && self.access.can_enumerate_directory(&path, &resolved) {
                    if depth >= 128 {
                        return Err(FsError::InvalidArgument(
                            "descriptor traversal depth exceeds provider limit 128".into(),
                        ));
                    }
                    let entries = self
                        .access
                        .open_read_dir(&path, &resolved)
                        .map_err(|e| FsError::io(&path, e))?;
                    self.stack.push((path, depth, entries));
                }
            }
            let Some((parent, depth, entries)) = self.stack.last_mut() else {
                return Ok(None);
            };
            self.access
                .check_cancelled()
                .map_err(|e| FsError::io(&*parent, e))?;
            let Some(entry) = entries.next() else {
                self.stack.pop();
                continue;
            };
            self.visited = self.visited.saturating_add(1);
            if self.visited > crate::MAX_TRAVERSAL_ENTRIES {
                return Err(FsError::InvalidArgument(
                    "descriptor traversal exceeds provider entry limit".into(),
                ));
            }
            let entry = entry.map_err(|e| FsError::io(&*parent, e))?;
            if entry.file_name().to_str().is_none() {
                continue;
            }
            let path = parent.join(entry.file_name());
            let kind = entry.file_type().map_err(|e| FsError::io(&path, e))?;
            let selected = self
                .filter
                .as_ref()
                .is_none_or(|filter| filter(&path, kind.is_dir()));
            if !selected {
                continue;
            }
            let resolved = match self.access.resolve_access_path(&path) {
                Ok(path) => path,
                Err(_) => continue,
            };
            if !self.access.is_readable_paths(&path, &resolved) {
                continue;
            }
            if kind.is_dir() {
                self.pending = Some((path.clone(), depth.saturating_add(1)));
            }
            return Ok(Some(Entry {
                path,
                kind: Some(kind),
            }));
        }
    }
}
impl Iterator for DescriptorWalk<'_> {
    type Item = Result<Entry, FsError>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.failed {
            return None;
        }
        match self.next_entry() {
            Ok(entry) => entry.map(Ok),
            Err(error) => {
                self.failed = true;
                Some(Err(error))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    struct RejectFiles {
        calls: AtomicUsize,
        cancel_at: usize,
    }
    impl FsAccessPolicy for RejectFiles {
        fn is_readable(&self, _: &Path) -> bool {
            true
        }
        fn is_writable(&self, _: &Path) -> bool {
            false
        }
        fn traversal_filter(&self) -> Option<FsTraversalFilter> {
            Some(Arc::new(|_, directory| directory))
        }
        fn check_cancelled(&self) -> std::io::Result<()> {
            if self.calls.fetch_add(1, Ordering::Relaxed) >= self.cancel_at {
                Err(std::io::Error::from(std::io::ErrorKind::Interrupted))
            } else {
                Ok(())
            }
        }
    }
    #[test]
    fn descriptor_walk_counts_rejected_entries_toward_the_entry_budget() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("rejected"), "must not be opened").unwrap();
        let policy = RejectFiles {
            calls: AtomicUsize::new(0),
            cancel_at: usize::MAX,
        };
        let Walk::Descriptor(mut walk) = Walk::descriptor(root.path(), &policy) else {
            panic!()
        };
        assert!(walk.next().unwrap().is_ok());
        walk.visited = crate::MAX_TRAVERSAL_ENTRIES;
        assert!(matches!(
            walk.next().unwrap(),
            Err(FsError::InvalidArgument(_))
        ));
    }
    #[test]
    fn descriptor_walk_checks_cancellation_while_scanning_only_rejected_entries() {
        let root = tempfile::tempdir().unwrap();
        for index in 0..20 {
            std::fs::write(root.path().join(index.to_string()), "must not be opened").unwrap();
        }
        let policy = RejectFiles {
            calls: AtomicUsize::new(0),
            cancel_at: 4,
        };
        let mut walk = Walk::descriptor(root.path(), &policy);
        assert!(walk.next().unwrap().is_ok());
        assert!(
            matches!(walk.next().unwrap(), Err(FsError::Io { source, .. }) if source.kind() == std::io::ErrorKind::Interrupted)
        );
    }
}
