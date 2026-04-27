//! Cascade-layer collection helpers.
//!
//! Pod manifests are assembled from up to three on-disk layers (see
//! `pod::PodFactory` for the full cascade story):
//!
//! 1. **User manifest** — see [`crate::paths::user_manifest_path`]
//! 2. **Project manifest** at the closest `.insomnia/manifest.toml`
//!    found by walking up from a starting directory (typically `cwd`)
//! 3. **Programmatic overlay** supplied at the call site
//!
//! This module owns the project-layer discovery and the parser glue.
//! User-layer path resolution lives in [`crate::paths`].
//!
//! Cascade *merging* and final validation stay outside this module —
//! that's the data layer's responsibility (`PodManifestConfig::merge`
//! and `PodManifest::try_from`). This module only handles the I/O and
//! path-discovery glue around them.

use std::path::{Path, PathBuf};

use crate::PodManifestConfig;

/// Errors returned when reading a single manifest layer from disk.
#[derive(Debug, thiserror::Error)]
pub enum LayerLoadError {
    #[error("failed to read manifest {}: {source}", .path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse manifest {}: {source}", .path.display())]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
}

/// Walk up from `start` looking for `.insomnia/manifest.toml`. Returns
/// the closest match, or `None` if none is found before reaching the
/// filesystem root.
pub fn find_project_manifest_from(start: &Path) -> Option<PathBuf> {
    let start = start
        .canonicalize()
        .ok()
        .unwrap_or_else(|| start.to_path_buf());
    let mut cur: Option<&Path> = Some(start.as_path());
    while let Some(dir) = cur {
        let candidate = dir.join(".insomnia").join("manifest.toml");
        if candidate.is_file() {
            return Some(candidate);
        }
        cur = dir.parent();
    }
    None
}

/// Read a manifest file from `path` and parse it as a partial
/// [`PodManifestConfig`]. Path resolution against a base directory and
/// merging with other layers are the caller's responsibility.
pub fn load_layer(path: &Path) -> Result<PodManifestConfig, LayerLoadError> {
    let toml = std::fs::read_to_string(path).map_err(|source| LayerLoadError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    PodManifestConfig::from_toml(&toml).map_err(|source| LayerLoadError::Parse {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn find_project_manifest_walks_up() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let manifest = root.join(".insomnia").join("manifest.toml");
        std::fs::create_dir_all(manifest.parent().unwrap()).unwrap();
        std::fs::write(&manifest, "").unwrap();

        let nested = root.join("a").join("b");
        std::fs::create_dir_all(&nested).unwrap();

        let found = find_project_manifest_from(&nested).unwrap();
        assert_eq!(found, manifest);
    }

    #[test]
    fn find_project_manifest_returns_none_when_absent() {
        let tmp = TempDir::new().unwrap();
        assert!(find_project_manifest_from(tmp.path()).is_none());
    }

    #[test]
    fn load_layer_round_trips_partial_config() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("manifest.toml");
        std::fs::write(
            &path,
            r#"
[pod]
name = "from-disk"
"#,
        )
        .unwrap();
        let cfg = load_layer(&path).unwrap();
        assert_eq!(cfg.pod.name.as_deref(), Some("from-disk"));
    }

    #[test]
    fn load_layer_io_error_carries_path() {
        let bogus = PathBuf::from("/definitely/does/not/exist/manifest.toml");
        let err = load_layer(&bogus).unwrap_err();
        match err {
            LayerLoadError::Io { path, .. } => assert_eq!(path, bogus),
            _ => panic!("expected Io variant"),
        }
    }

}
