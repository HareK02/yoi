//! Durability establishment is local to a manager, not inferred from existence.
//! Retained descriptors pin the established ancestor chain and scoped lock inode;
//! each acquisition rechecks the configured/canonical paths before and after the
//! OS lock. A replacement is rejected for the lifetime of that proof. New
//! managers (including new processes) repeat all persistence steps. Failed
//! establishment publishes no proof. Configured Host symlinks remain supported,
//! but their mutable links require full establishment on every acquisition.
//!
//! This is only durability evidence. The caller must still verify the current
//! owner catalog, DB identity, fences and authorization under the returned lease.
//! As with the owner, advisory checks require a Host-protected filesystem; they
//! do not sandbox native writers racing after the final path check.
use super::*;

/// Instance-local durability evidence, shared only by a manager and its handles.
/// It never substitutes for the owner catalog, DB identity, fences or authority.
#[derive(Default)]
pub(crate) struct EstablishedLocks {
    scopes: Mutex<HashMap<String, Arc<Proof>>>,
}

struct Directory {
    path: PathBuf,
    file: File,
    symlink: bool,
}

struct Proof {
    root: PathBuf,
    canonical_root: PathBuf,
    directories: Vec<Directory>,
    lock_path: PathBuf,
    lock: File,
    reusable: bool,
}

fn conflict() -> FeatureStorageError {
    FeatureStorageError::Identity("canonical lifecycle lock path was replaced or redirected".into())
}

fn same_file(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    left.dev() == right.dev() && left.ino() == right.ino()
}

fn regular_lock(path: &Path) -> Result<fs::Metadata> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() {
        return Err(conflict());
    }
    Ok(metadata)
}

impl Proof {
    fn capture(root: &Path, scope: &str) -> Result<Self> {
        let root = if root.is_absolute() {
            root.to_path_buf()
        } else {
            std::env::current_dir()?.join(root)
        };
        let directory = root.join(".lifecycle");
        // A configured Host root may contain symlinks. The lock namespace itself
        // must not: following a replacement there could split the advisory lock.
        if !fs::symlink_metadata(&directory)?.is_dir() {
            return Err(conflict());
        }
        let canonical_root = fs::canonicalize(&root)?;
        let resolved = fs::canonicalize(&directory)?;
        if resolved != canonical_root.join(".lifecycle") {
            return Err(conflict());
        }
        let mut directories = Vec::new();
        let mut seen = HashSet::new();
        for path in directory.ancestors().chain(resolved.ancestors()) {
            if seen.insert(path.to_path_buf()) {
                let symlink = fs::symlink_metadata(path)?.is_symlink();
                let file = File::open(path)?;
                if !file.metadata()?.is_dir() {
                    return Err(conflict());
                }
                directories.push(Directory {
                    path: path.into(),
                    file,
                    symlink,
                });
            }
        }
        let lock_path = directory.join(format!("{scope}.lock"));
        match fs::symlink_metadata(&lock_path) {
            Ok(_) => {
                regular_lock(&lock_path)?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)?;
        let proof = Self {
            root,
            canonical_root,
            reusable: directories.iter().all(|entry| !entry.symlink),
            directories,
            lock_path,
            lock,
        };
        proof.check()?;
        Ok(proof)
    }

    fn check(&self) -> Result<()> {
        if fs::canonicalize(&self.root)? != self.canonical_root {
            return Err(conflict());
        }
        for directory in &self.directories {
            let link = fs::symlink_metadata(&directory.path)?;
            if link.is_symlink() != directory.symlink
                || !same_file(&fs::metadata(&directory.path)?, &directory.file.metadata()?)
            {
                return Err(conflict());
            }
        }
        if !same_file(&regular_lock(&self.lock_path)?, &self.lock.metadata()?)
            || fs::canonicalize(&self.lock_path)?
                != self
                    .canonical_root
                    .join(".lifecycle")
                    .join(self.lock_path.file_name().ok_or_else(conflict)?)
        {
            return Err(conflict());
        }
        Ok(())
    }

    fn open(&self) -> Result<File> {
        self.check()?;
        // A fresh open description is required for each flock lease. Cloning
        // the pinned File would share one OS lock across independent operations.
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&self.lock_path)?;
        if !same_file(&lock.metadata()?, &self.lock.metadata()?) {
            return Err(conflict());
        }
        Ok(lock)
    }
}

impl Lifecycle {
    pub(super) fn lock_with_sync(
        &self,
        exclusive: bool,
        sync: &mut impl FnMut(&Path) -> std::io::Result<()>,
    ) -> Result<File> {
        #[cfg(feature = "test-support")]
        self.observation.acquisition();
        let proof = {
            let mut scopes = self.established.scopes.lock().map_err(|_| {
                FeatureStorageError::Operation("lifecycle establishment mutex poisoned".into())
            })?;
            let previous = scopes.get(&self.scope);
            if let Some(proof) = previous {
                // Replacements are rejected, not silently accepted as a new
                // lock domain while an old manager/handle might still hold it.
                proof.check()?;
            }
            if let Some(proof) = previous.filter(|proof| proof.reusable) {
                proof.clone()
            } else {
                #[cfg(feature = "test-support")]
                let started = self.observation.sync_start();
                let result = (|| {
                    let directory = self.root.join(".lifecycle");
                    fs::create_dir_all(&directory)?;
                    let proof = Proof::capture(&self.root, &self.scope)?;
                    // Re-sync even existing ancestors: their presence is not
                    // evidence that an earlier attempt persisted their links.
                    durable_create_directory(&directory, sync)?;
                    proof.lock.sync_all()?;
                    sync(&directory)?;
                    proof.check()?;
                    Ok::<_, FeatureStorageError>(Arc::new(proof))
                })();
                #[cfg(feature = "test-support")]
                self.observation.sync_finished(started, result.is_err());
                let proof = result?;
                // Publish only after every durability step and path recheck.
                // Pinned descriptors prevent inode reuse from reviving proof.
                scopes.insert(self.scope.clone(), proof.clone());
                proof
            }
        };
        let lock = proof.open()?;
        #[cfg(feature = "test-support")]
        let started = self.observation.lock_start();
        let result = if exclusive {
            lock.lock()
        } else {
            lock.lock_shared()
        };
        #[cfg(feature = "test-support")]
        self.observation.lock_finished(started, result.is_err());
        result?;
        // Do not hold the instance mutex while waiting for another process.
        // Check again after the OS wait, before the caller can touch its DB.
        proof.check()?;
        if !same_file(&lock.metadata()?, &proof.lock.metadata()?) {
            return Err(conflict());
        }
        Ok(lock)
    }
}

#[cfg(test)]
#[path = "lifecycle_lock_tests.rs"]
mod tests;
