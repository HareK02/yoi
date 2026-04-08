use std::path::{Path, PathBuf};

/// Directory scope constraining a Pod's write access.
///
/// Read access is unrestricted — only write operations are checked against the scope.
#[derive(Debug, Clone)]
pub struct Scope {
    root: PathBuf,
}

impl Scope {
    /// Create a new scope rooted at the given directory.
    ///
    /// The path is canonicalized to resolve symlinks and relative components.
    pub fn new(root: impl Into<PathBuf>) -> std::io::Result<Self> {
        let root = root.into().canonicalize()?;
        Ok(Self { root })
    }

    /// The root directory of this scope.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Check whether `path` falls within this scope.
    ///
    /// The path is canonicalized before comparison.
    pub fn contains(&self, path: &Path) -> bool {
        match path.canonicalize() {
            Ok(canonical) => canonical.starts_with(&self.root),
            Err(_) => {
                // Path doesn't exist yet — check the parent directory instead.
                // This handles write_file to a new file inside the scope.
                match path.parent().and_then(|p| p.canonicalize().ok()) {
                    Some(parent) => parent.starts_with(&self.root),
                    None => false,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn contains_file_inside_scope() {
        let dir = TempDir::new().unwrap();
        let scope = Scope::new(dir.path()).unwrap();

        let file = dir.path().join("test.txt");
        fs::write(&file, "hello").unwrap();

        assert!(scope.contains(&file));
    }

    #[test]
    fn rejects_file_outside_scope() {
        let dir = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let scope = Scope::new(dir.path()).unwrap();

        let file = outside.path().join("test.txt");
        fs::write(&file, "hello").unwrap();

        assert!(!scope.contains(&file));
    }

    #[test]
    fn contains_new_file_in_existing_parent() {
        let dir = TempDir::new().unwrap();
        let scope = Scope::new(dir.path()).unwrap();

        // File doesn't exist yet, but parent dir is inside scope
        let new_file = dir.path().join("new.txt");
        assert!(scope.contains(&new_file));
    }

    #[test]
    fn contains_nested_directory() {
        let dir = TempDir::new().unwrap();
        let nested = dir.path().join("a/b/c");
        fs::create_dir_all(&nested).unwrap();
        let scope = Scope::new(dir.path()).unwrap();

        let file = nested.join("test.txt");
        assert!(scope.contains(&file));
    }

    #[test]
    fn rejects_traversal_attack() {
        let dir = TempDir::new().unwrap();
        let scope = Scope::new(dir.path()).unwrap();

        let traversal = dir.path().join("../../../etc/passwd");
        assert!(!scope.contains(&traversal));
    }
}
