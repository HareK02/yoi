//! Bounded byte-preserving transport, independent of the owning store's schema validation.
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::{HarnessError, Result};

#[derive(Debug, Clone, Copy)]
struct Limits {
    file_bytes: u64,
    total_bytes: u64,
    entries: usize,
    depth: usize,
}

const LIMITS: Limits = Limits {
    file_bytes: 8 * 1024 * 1024,
    total_bytes: 32 * 1024 * 1024,
    entries: 256,
    depth: 8,
};

#[derive(Debug, Clone)]
pub struct FixtureSource {
    root: PathBuf,
    hashes: BTreeMap<PathBuf, String>,
    flat_files: Option<BTreeSet<PathBuf>>,
}

fn reject(message: &'static str) -> HarnessError {
    HarnessError::Protocol(message.into())
}

impl FixtureSource {
    /// Capture a bounded tree without generating or repairing store data.
    pub fn capture(root: &Path) -> Result<Self> {
        Self::capture_with(root, None)
    }

    /// Validate the complete flat file set before reading any content. The same
    /// allowlist is retained for copy and later source-integrity checks.
    pub fn capture_flat(root: &Path, names: &[&str]) -> Result<Self> {
        let files: BTreeSet<_> = names.iter().map(PathBuf::from).collect();
        if files.len() != names.len()
            || files.iter().any(|name| {
                name.components().count() != 1
                    || !matches!(
                        name.components().next(),
                        Some(std::path::Component::Normal(_))
                    )
            })
        {
            return Err(reject("invalid fixture file allowlist"));
        }
        Self::capture_with(root, Some(files))
    }

    fn capture_with(root: &Path, flat_files: Option<BTreeSet<PathBuf>>) -> Result<Self> {
        let hashes = inventory(root, flat_files.as_ref(), LIMITS)?;
        if hashes.is_empty() {
            return Err(reject("provided fixture is empty"));
        }
        Ok(Self {
            root: root.to_owned(),
            hashes,
            flat_files,
        })
    }

    /// Read a known asset through a descriptor with a real byte limit, including
    /// when its size grows after metadata inspection. Errors never include paths.
    pub fn read_bounded(path: &Path) -> Result<Vec<u8>> {
        read_regular(path, LIMITS.file_bytes)
    }

    pub fn hashes(&self) -> &BTreeMap<PathBuf, String> {
        &self.hashes
    }

    pub fn assert_unchanged(&self) -> Result<()> {
        if inventory(&self.root, self.flat_files.as_ref(), LIMITS)? != self.hashes {
            return Err(reject("provided fixture changed during execution"));
        }
        Ok(())
    }

    /// The destination must not exist. A copy is never a link to the source.
    pub fn copy_to(&self, destination: &Path) -> Result<()> {
        self.assert_unchanged()?;
        let parent = destination
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        if parent
            .canonicalize()?
            .starts_with(self.root.canonicalize()?)
        {
            return Err(reject("fixture copy destination is inside the original"));
        }
        fs::create_dir(destination)?;
        let mut remaining = LIMITS.total_bytes;
        for (relative, expected) in &self.hashes {
            let bytes = read_regular(&self.root.join(relative), LIMITS.file_bytes.min(remaining))?;
            remaining -= bytes.len() as u64;
            if hash(&bytes) != *expected {
                return Err(reject("fixture changed while copying"));
            }
            let target = destination.join(relative);
            fs::create_dir_all(target.parent().unwrap())?;
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(target)?;
            file.write_all(&bytes)?;
        }
        if inventory(destination, self.flat_files.as_ref(), LIMITS)? != self.hashes {
            return Err(reject("fixture copy failed hash verification"));
        }
        self.assert_unchanged()
    }
}

fn hash(bytes: &[u8]) -> String {
    format!(
        "sha256:{}",
        Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    )
}

fn read_limited(reader: impl Read, limit: u64) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| reject("fixture content read failed"))?;
    if bytes.len() as u64 > limit {
        return Err(reject("fixture byte budget exceeded"));
    }
    Ok(bytes)
}

fn read_regular(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // Do not follow a replaced final symlink or block on a replaced FIFO.
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options
        .open(path)
        .map_err(|_| reject("fixture asset open failed"))?;
    let metadata = file
        .metadata()
        .map_err(|_| reject("fixture asset metadata failed"))?;
    if !metadata.is_file() || metadata.len() > limit {
        return Err(reject("fixture asset is not a bounded regular file"));
    }
    read_limited(file, limit)
}

fn inventory(
    root: &Path,
    flat: Option<&BTreeSet<PathBuf>>,
    limits: Limits,
) -> Result<BTreeMap<PathBuf, String>> {
    struct Budget {
        entries: usize,
        bytes: u64,
    }
    fn visit(
        root: &Path,
        path: &Path,
        depth: usize,
        limits: Limits,
        flat: Option<&BTreeSet<PathBuf>>,
        budget: &mut Budget,
        files: &mut BTreeSet<PathBuf>,
    ) -> Result<()> {
        if depth > limits.depth || budget.entries == 0 {
            return Err(reject("fixture tree budget exceeded"));
        }
        budget.entries -= 1;
        let metadata = fs::symlink_metadata(path).map_err(|_| reject("fixture metadata failed"))?;
        let relative = path.strip_prefix(root).unwrap();
        if depth != 0 && flat.is_some_and(|names| !names.contains(relative)) {
            return Err(reject("unexpected fixture file set"));
        }
        if metadata.is_dir() && (depth == 0 || flat.is_none()) {
            for entry in fs::read_dir(path).map_err(|_| reject("fixture directory read failed"))? {
                let entry = entry.map_err(|_| reject("fixture directory entry failed"))?;
                visit(root, &entry.path(), depth + 1, limits, flat, budget, files)?;
            }
        } else if metadata.is_file() && depth != 0 {
            if metadata.len() > limits.file_bytes || metadata.len() > budget.bytes {
                return Err(reject("fixture byte budget exceeded"));
            }
            budget.bytes -= metadata.len();
            files.insert(relative.to_owned());
        } else {
            return Err(reject("fixture contains a symlink or special file"));
        }
        Ok(())
    }
    // Preflight the entire shape and metadata budget before reading any body.
    let mut files = BTreeSet::new();
    visit(
        root,
        root,
        0,
        limits,
        flat,
        &mut Budget {
            entries: limits.entries,
            bytes: limits.total_bytes,
        },
        &mut files,
    )?;
    if flat.is_some_and(|expected| expected != &files) {
        return Err(reject("unexpected fixture file set"));
    }
    let mut remaining = limits.total_bytes;
    let mut hashes = BTreeMap::new();
    for relative in files {
        let bytes = read_regular(&root.join(&relative), limits.file_bytes.min(remaining))?;
        remaining -= bytes.len() as u64;
        hashes.insert(relative, hash(&bytes));
    }
    Ok(hashes)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Transport-only synthetic bytes, never a substitute saved Worker fixture.
    #[test]
    fn copies_are_independent_and_source_mutation_is_detected() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let source = temp.path().join("source");
        fs::create_dir(&source)?;
        fs::write(source.join("bytes"), b"original")?;
        let fixture = FixtureSource::capture_flat(&source, &["bytes"])?;
        assert!(fixture.copy_to(&source.join("nested-copy")).is_err());
        assert!(!source.join("nested-copy").exists());
        fixture.assert_unchanged()?;
        let first = temp.path().join("first");
        let second = temp.path().join("second");
        fixture.copy_to(&first)?;
        fixture.copy_to(&second)?;
        fs::write(first.join("bytes"), b"changed")?;
        fs::remove_dir_all(first)?;
        assert_eq!(fs::read(second.join("bytes"))?, b"original");
        fixture.assert_unchanged()?;
        fs::write(source.join("bytes"), b"changed")?;
        assert!(fixture.assert_unchanged().is_err());
        Ok(())
    }

    #[test]
    fn rejects_missing_empty_existing_destination_and_extra_file() -> Result<()> {
        let temp = tempfile::tempdir()?;
        assert!(FixtureSource::capture(&temp.path().join("missing")).is_err());
        assert!(FixtureSource::capture(temp.path()).is_err());
        fs::write(temp.path().join("bytes"), b"original")?;
        let source = FixtureSource::capture(temp.path())?;
        assert!(source.copy_to(temp.path()).is_err());
        fs::write(temp.path().join("extra"), b"new")?;
        assert!(source.assert_unchanged().is_err());
        Ok(())
    }

    #[test]
    fn flat_allowlist_rejects_extra_entries_before_reading_content() -> Result<()> {
        let temp = tempfile::tempdir()?;
        fs::write(temp.path().join("asset"), b"ok")?;
        // Sparse oversized input is never read. Even an allowed file that cannot
        // be decoded does not take precedence over an unexpected file set.
        fs::File::create(temp.path().join("extra"))?.set_len(LIMITS.file_bytes + 1)?;
        let error = FixtureSource::capture_flat(temp.path(), &["asset"]).unwrap_err();
        assert!(error.to_string().contains("unexpected fixture file set"));
        fs::remove_file(temp.path().join("extra"))?;
        assert!(FixtureSource::capture_flat(temp.path(), &["missing"]).is_err());
        fs::create_dir(temp.path().join("dir"))?;
        assert!(FixtureSource::capture_flat(temp.path(), &["asset", "dir"]).is_err());
        Ok(())
    }

    #[test]
    fn tree_count_depth_total_and_actual_read_budgets_are_enforced() -> Result<()> {
        let temp = tempfile::tempdir()?;
        fs::write(temp.path().join("a"), b"1234")?;
        fs::write(temp.path().join("b"), b"1234")?;
        for limits in [
            Limits {
                entries: 2,
                ..LIMITS
            },
            Limits {
                total_bytes: 7,
                ..LIMITS
            },
            Limits {
                file_bytes: 3,
                ..LIMITS
            },
        ] {
            assert!(inventory(temp.path(), None, limits).is_err());
        }
        fs::create_dir_all(temp.path().join("nested/deep"))?;
        assert!(inventory(temp.path(), None, Limits { depth: 1, ..LIMITS }).is_err());
        // A reader can return more bytes than earlier metadata reported. Bound
        // actual IO rather than trusting that observation or an EOF at stat size.
        assert!(read_limited(std::io::Cursor::new(b"12345"), 4).is_err());
        assert_eq!(read_limited(std::io::Cursor::new(b"1234"), 4)?, b"1234");
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symbolic_links() -> Result<()> {
        let temp = tempfile::tempdir()?;
        fs::write(temp.path().join("bytes"), b"original")?;
        std::os::unix::fs::symlink("bytes", temp.path().join("link"))?;
        assert!(FixtureSource::capture(temp.path()).is_err());
        assert!(FixtureSource::read_bounded(&temp.path().join("link")).is_err());
        Ok(())
    }
}
