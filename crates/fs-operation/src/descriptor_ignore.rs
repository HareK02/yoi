//! Ignore rules loaded only through the provider's authorized descriptor opens.
//! Never use GitignoreBuilder::add/build_global: those reopen host pathnames.
use crate::{FsAccessPolicy, FsError};
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};

pub(crate) const MAX_IGNORE_SOURCE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_IGNORE_FILE_BYTES: u64 = 1024 * 1024;
const MAX_IGNORE_LINE_BYTES: u64 = 16 * 1024;

pub(crate) struct SourceBoundedReader<'a, R> {
    pub inner: R,
    pub remaining: &'a mut u64,
    pub access: &'a dyn FsAccessPolicy,
}
impl<R: Read> Read for SourceBoundedReader<'_, R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.access.check_cancelled()?;
        if buffer.is_empty() {
            return Ok(0);
        }
        if *self.remaining == 0 {
            let mut probe = [0_u8; 1];
            return match self.inner.read(&mut probe)? {
                0 => Ok(0),
                _ => Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "search source exceeded provider byte limit",
                )),
            };
        }
        let limit = usize::try_from(*self.remaining)
            .unwrap_or(usize::MAX)
            .min(buffer.len());
        let read = self.inner.read(&mut buffer[..limit])?;
        *self.remaining = (*self.remaining).saturating_sub(read as u64);
        Ok(read)
    }
}

/// Shared Glob/Grep grammar and provider authorization. Glob, like WalkBuilder,
/// ignores unavailable configuration files; Grep retains its existing explicit
/// error on a rejected ignore-file open. Cancellation/bounds always fail closed.
pub(crate) fn add_checked_ignore_file(
    builder: &mut GitignoreBuilder,
    path: &Path,
    remaining: &mut u64,
    access: &dyn FsAccessPolicy,
    skip_unavailable: bool,
) -> Result<bool, FsError> {
    access.check_cancelled().map_err(|e| FsError::io(path, e))?;
    let resolved = match access.resolve_access_path(path) {
        Ok(path) => path,
        Err(error) if skip_unavailable && error.kind() != std::io::ErrorKind::Interrupted => {
            return Ok(false);
        }
        Err(error) => return Err(FsError::io(path, error)),
    };
    if !access.is_readable_paths(path, &resolved) {
        return Ok(false);
    }
    let file = match access.open_read_file(path, &resolved) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) if skip_unavailable && error.kind() != std::io::ErrorKind::Interrupted => {
            return Ok(false);
        }
        Err(error) => return Err(FsError::io(path, error)),
    };
    let metadata = file.metadata().map_err(|e| FsError::io(path, e))?;
    if !metadata.is_file() {
        return Ok(false);
    }
    if metadata.len() > MAX_IGNORE_FILE_BYTES {
        return Err(FsError::InvalidArgument(
            "ignore file exceeds provider limit 1 MiB".into(),
        ));
    }
    let bounded = SourceBoundedReader {
        inner: file,
        remaining,
        access,
    };
    // Take also fences files that grow after metadata. Each line is bounded
    // before allocation, and every underlying read checks cancellation.
    let mut reader = BufReader::new(bounded.take(MAX_IGNORE_FILE_BYTES + 1));
    let mut line = Vec::new();
    let mut bytes = 0_u64;
    let mut added = false;
    loop {
        line.clear();
        // BufRead::read_line/read_until retry Interrupted internally. Explicit
        // fill_buf prevents a cooperative cancellation from becoming a retry
        // loop, and caps allocation before copying even a growing long line.
        loop {
            access.check_cancelled().map_err(|e| FsError::io(path, e))?;
            let buffer = reader.fill_buf().map_err(|e| FsError::io(path, e))?;
            if buffer.is_empty() {
                break;
            }
            let count = buffer
                .iter()
                .position(|byte| *byte == b'\n')
                .map_or(buffer.len(), |index| index + 1);
            if line.len().saturating_add(count) as u64 > MAX_IGNORE_LINE_BYTES {
                return Err(FsError::InvalidArgument(
                    "ignore line exceeds provider limit 16 KiB".into(),
                ));
            }
            let finished = buffer[count - 1] == b'\n';
            line.extend_from_slice(&buffer[..count]);
            reader.consume(count);
            if finished {
                break;
            }
        }
        if line.is_empty() {
            break;
        }
        bytes = bytes.saturating_add(line.len() as u64);
        if bytes > MAX_IGNORE_FILE_BYTES {
            return Err(FsError::InvalidArgument(
                "ignore file exceeds provider limit 1 MiB".into(),
            ));
        }
        let line = std::str::from_utf8(&line)
            .map_err(|_| FsError::InvalidArgument("ignore file must be UTF-8".into()))?
            .trim_end_matches(['\r', '\n']);
        // One invalid glob must not discard other valid rules.
        if builder.add_line(Some(path.to_path_buf()), line).is_ok() {
            added = true;
        }
    }
    Ok(added)
}

struct DirectoryRules {
    path: PathBuf,
    ignore: Gitignore,
    git_ignore: Gitignore,
    exclude: Gitignore,
    has_git: bool,
}

/// Request-local ancestor stack, not a file-tree cache. Only the current
/// directory's ancestry is retained; source bytes and depth bound its matchers.
pub(crate) struct DescriptorIgnores {
    directories: Vec<DirectoryRules>,
    remaining: u64,
    require_git: bool,
    skip_unavailable: bool,
}
impl DescriptorIgnores {
    pub fn new(root: &Path, base: &Path, access: &dyn FsAccessPolicy) -> Result<Self, FsError> {
        Self::with_options(root, base, access, true, true)
    }
    pub fn grep(root: &Path, base: &Path, access: &dyn FsAccessPolicy) -> Result<Self, FsError> {
        // Preserve the existing descriptor Grep provider contract: gitignore
        // sources apply without a .git marker and rejected opens fail closed.
        Self::with_options(root, base, access, false, false)
    }
    fn with_options(
        root: &Path,
        base: &Path,
        access: &dyn FsAccessPolicy,
        require_git: bool,
        skip_unavailable: bool,
    ) -> Result<Self, FsError> {
        let relative = base.strip_prefix(root).map_err(|_| {
            FsError::InvalidArgument("glob base is outside its provider root".into())
        })?;
        if relative.components().count() >= 128 {
            return Err(FsError::InvalidArgument(
                "ignore ancestry exceeds provider depth limit 128".into(),
            ));
        }
        let mut this = Self {
            directories: Vec::new(),
            remaining: MAX_IGNORE_SOURCE_BYTES,
            require_git,
            skip_unavailable,
        };
        let mut directory = root.to_path_buf();
        for component in relative.components() {
            this.load_directory(&directory, access)?;
            directory.push(component);
        }
        Ok(this)
    }

    pub fn load_directory(
        &mut self,
        directory: &Path,
        access: &dyn FsAccessPolicy,
    ) -> Result<(), FsError> {
        self.directories
            .retain(|rules| directory.starts_with(&rules.path));
        if self
            .directories
            .last()
            .is_some_and(|rules| rules.path == directory)
        {
            return Ok(());
        }
        if self.directories.len() >= 128 {
            return Err(FsError::InvalidArgument(
                "ignore ancestry exceeds provider depth limit 128".into(),
            ));
        }
        access
            .check_cancelled()
            .map_err(|e| FsError::io(directory, e))?;
        let marker = directory.join(".git");
        let git_kind = access
            .resolve_access_path(&marker)
            .ok()
            .and_then(|resolved| {
                if !access.is_readable_paths(&marker, &resolved) {
                    return None;
                }
                access
                    .read_metadata(&marker, &resolved)
                    .ok()
                    .map(|metadata| metadata.file_type())
            });
        let has_git = git_kind.is_some_and(|kind| kind.is_dir() || kind.is_file());
        let mut load = |name: &str| -> Result<Gitignore, FsError> {
            let mut builder = GitignoreBuilder::new(directory);
            add_checked_ignore_file(
                &mut builder,
                &directory.join(name),
                &mut self.remaining,
                access,
                self.skip_unavailable,
            )?;
            builder.build().map_err(|_| {
                FsError::InvalidArgument("ignore patterns could not be compiled".into())
            })
        };
        let ignore = load(".ignore")?;
        let git_ignore = load(".gitignore")?;
        // A linked-worktree .git file activates repository rules but is not a
        // directory. Never chase its arbitrary host gitdir/commondir pointers.
        let exclude = if git_kind.is_some_and(|kind| kind.is_dir()) {
            load(".git/info/exclude")?
        } else {
            Gitignore::empty()
        };
        self.directories.push(DirectoryRules {
            path: directory.to_path_buf(),
            ignore,
            git_ignore,
            exclude,
            has_git,
        });
        Ok(())
    }

    pub fn is_ignored(&mut self, path: &Path, is_dir: bool) -> bool {
        self.directories
            .retain(|rules| path.starts_with(&rules.path));
        // .ignore has priority over all .gitignore/exclude rules. Within each
        // class the nearest directory's last matching rule wins. Match entries,
        // not any parents: ignored directories are pruned by the walker, while
        // an explicitly selected ignored search root remains traversable.
        for rules in self.directories.iter().rev() {
            let matched = rules.ignore.matched(path, is_dir);
            if !matched.is_none() {
                return matched.is_ignore();
            }
        }
        if self.require_git && !self.directories.iter().any(|rules| rules.has_git) {
            return false;
        }
        let mut exclude = None;
        for rules in self.directories.iter().rev() {
            let matched = rules.git_ignore.matched(path, is_dir);
            if !matched.is_none() {
                return matched.is_ignore();
            }
            if exclude.is_none() {
                let matched = rules.exclude.matched(path, is_dir);
                if !matched.is_none() {
                    exclude = Some(matched.is_ignore());
                }
            }
            if rules.has_git {
                break;
            }
        }
        exclude.unwrap_or(false)
    }
}
