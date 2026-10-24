//! Byte-preserving fixture transport, independent of the owning store's schema validation.
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::{HarnessError, Result};

#[derive(Debug, Clone)]
pub struct FixtureSource {
    root: PathBuf,
    hashes: BTreeMap<PathBuf, String>,
}

impl FixtureSource {
    /// Record every supplied file, including the manifest. No fixture is generated
    /// or repaired here; the typed store importer must additionally validate it.
    pub fn capture(root: &Path) -> Result<Self> {
        let hashes = inventory(root)?;
        if hashes.is_empty() {
            return Err(HarnessError::Protocol("provided fixture is empty".into()));
        }
        Ok(Self {
            root: root.to_owned(),
            hashes,
        })
    }

    pub fn hashes(&self) -> &BTreeMap<PathBuf, String> {
        &self.hashes
    }

    pub fn assert_unchanged(&self) -> Result<()> {
        if inventory(&self.root)? != self.hashes {
            return Err(HarnessError::Protocol(
                "provided fixture changed during execution".into(),
            ));
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
            return Err(HarnessError::Protocol(
                "fixture copy destination is inside the original".into(),
            ));
        }
        fs::create_dir(destination)?;
        for (relative, expected) in &self.hashes {
            let bytes = fs::read(self.root.join(relative))?;
            if hash(&bytes) != *expected {
                return Err(HarnessError::Protocol(
                    "fixture changed while copying".into(),
                ));
            }
            let target = destination.join(relative);
            fs::create_dir_all(target.parent().unwrap())?;
            use std::io::Write;
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(target)?;
            file.write_all(&bytes)?;
        }
        if inventory(destination)? != self.hashes {
            return Err(HarnessError::Protocol(
                "fixture copy failed hash verification".into(),
            ));
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

fn inventory(root: &Path) -> Result<BTreeMap<PathBuf, String>> {
    fn visit(root: &Path, path: &Path, files: &mut BTreeMap<PathBuf, String>) -> Result<()> {
        let metadata = fs::symlink_metadata(path)?;
        if metadata.is_dir() {
            for entry in fs::read_dir(path)? {
                visit(root, &entry?.path(), files)?;
            }
        } else if metadata.is_file() {
            files.insert(
                path.strip_prefix(root).unwrap().to_owned(),
                hash(&fs::read(path)?),
            );
        } else {
            return Err(HarnessError::Protocol(
                "fixture contains a symlink or special file".into(),
            ));
        }
        Ok(())
    }
    if !fs::symlink_metadata(root)?.is_dir() {
        return Err(HarnessError::Protocol(
            "fixture source must be a real directory".into(),
        ));
    }
    let mut files = BTreeMap::new();
    visit(root, root, &mut files)?;
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    // These are transport-only byte samples, never saved Worker substitutes.
    #[test]
    fn copies_are_independent_and_source_mutation_is_detected() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let source = temp.path().join("source");
        fs::create_dir(&source)?;
        fs::write(source.join("bytes"), b"original")?;
        let fixture = FixtureSource::capture(&source)?;
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

    #[cfg(unix)]
    #[test]
    fn rejects_symbolic_links() -> Result<()> {
        let temp = tempfile::tempdir()?;
        fs::write(temp.path().join("bytes"), b"original")?;
        std::os::unix::fs::symlink("bytes", temp.path().join("link"))?;
        assert!(FixtureSource::capture(temp.path()).is_err());
        Ok(())
    }
}
