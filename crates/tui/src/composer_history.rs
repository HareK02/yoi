use std::collections::VecDeque;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use protocol::Segment;
use serde::{Deserialize, Serialize};

pub(crate) const COMPOSER_INPUT_HISTORY_LIMIT: usize = 30;
const COMPOSER_HISTORY_VERSION: u32 = 1;

#[derive(Debug, Clone)]
pub(crate) struct ComposerHistoryStore {
    path: PathBuf,
    workspace: WorkspaceIdentity,
}

#[derive(Debug)]
pub(crate) enum ComposerHistoryLoadError {
    Io(io::Error),
    Json(serde_json::Error),
}

impl std::fmt::Display for ComposerHistoryLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "{error}"),
            Self::Json(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for ComposerHistoryLoadError {}

impl From<io::Error> for ComposerHistoryLoadError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for ComposerHistoryLoadError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ComposerHistoryFile {
    version: u32,
    workspace: WorkspaceIdentity,
    entries: Vec<Vec<Segment>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WorkspaceIdentity {
    root: String,
    label: String,
    key: String,
}

impl ComposerHistoryStore {
    pub(crate) fn default_for_workspace(workspace_root: &Path) -> io::Result<Option<Self>> {
        let Some(data_dir) = manifest::paths::data_dir() else {
            return Ok(None);
        };
        Ok(Some(Self::for_data_dir(data_dir, workspace_root)))
    }

    pub(crate) fn for_data_dir(data_dir: impl AsRef<Path>, workspace_root: &Path) -> Self {
        let workspace = workspace_identity(workspace_root);
        let path = data_dir
            .as_ref()
            .join("client")
            .join("composer-history")
            .join("workspaces")
            .join(format!("{}-{}", workspace.label, workspace.key))
            .join("history.json");
        Self { path, workspace }
    }

    #[cfg(test)]
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn load(&self) -> Result<VecDeque<Vec<Segment>>, ComposerHistoryLoadError> {
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(VecDeque::new()),
            Err(error) => return Err(error.into()),
        };
        let persisted: ComposerHistoryFile = serde_json::from_slice(&bytes)?;
        Ok(normalized_entries(persisted.entries))
    }

    pub(crate) fn save(
        &self,
        entries: &VecDeque<Vec<Segment>>,
    ) -> Result<(), ComposerHistoryLoadError> {
        let persisted = ComposerHistoryFile {
            version: COMPOSER_HISTORY_VERSION,
            workspace: self.workspace.clone(),
            entries: entries.iter().cloned().collect(),
        };
        let bytes = serde_json::to_vec_pretty(&persisted)?;
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = self.path.with_extension("json.tmp");
        fs::write(&tmp, bytes)?;
        fs::rename(tmp, &self.path)?;
        Ok(())
    }
}

pub(crate) fn normalized_entries(entries: Vec<Vec<Segment>>) -> VecDeque<Vec<Segment>> {
    let mut normalized = VecDeque::new();
    for segments in entries {
        if segments_are_blank(&segments) {
            continue;
        }
        if normalized.back() == Some(&segments) {
            continue;
        }
        if normalized.len() == COMPOSER_INPUT_HISTORY_LIMIT {
            normalized.pop_front();
        }
        normalized.push_back(segments);
    }
    normalized
}

pub(crate) fn segments_are_blank(segments: &[Segment]) -> bool {
    segments.iter().all(|s| match s {
        Segment::Text { content } => content.trim().is_empty(),
        _ => false,
    })
}

fn workspace_identity(workspace_root: &Path) -> WorkspaceIdentity {
    let root = normalized_workspace_key(workspace_root);
    let label = workspace_leaf(&root);
    let key = fnv1a64_hex(root.as_bytes());
    WorkspaceIdentity { root, label, key }
}

fn normalized_workspace_key(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn workspace_leaf(workspace_root: &str) -> String {
    let leaf = workspace_root
        .rsplit('/')
        .find(|part| !part.is_empty())
        .unwrap_or("workspace");
    let sanitized = leaf
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string();
    if sanitized.is_empty() {
        "workspace".to_string()
    } else {
        sanitized
    }
}

fn fnv1a64_hex(bytes: &[u8]) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn store_path_is_workspace_scoped_under_client_data_dir() {
        let data_dir = TempDir::new().unwrap();
        let store = ComposerHistoryStore::for_data_dir(data_dir.path(), Path::new("/repo/yoi"));
        let other = ComposerHistoryStore::for_data_dir(data_dir.path(), Path::new("/repo/other"));

        assert!(store.path().starts_with(data_dir.path()));
        assert!(
            store
                .path()
                .to_string_lossy()
                .contains("client/composer-history/workspaces/yoi-")
        );
        assert_ne!(store.path(), other.path());
    }

    #[test]
    fn legacy_top_level_history_is_ignored_without_migration_or_fallback() {
        let data_dir = TempDir::new().unwrap();
        let workspace_root = Path::new("/repo/yoi");
        let workspace = workspace_identity(workspace_root);
        let legacy_path = data_dir
            .path()
            .join("composer-history")
            .join("workspaces")
            .join(format!("{}-{}", workspace.label, workspace.key))
            .join("history.json");
        let legacy_file = ComposerHistoryFile {
            version: COMPOSER_HISTORY_VERSION,
            workspace,
            entries: vec![vec![Segment::text("legacy entry")]],
        };
        let legacy_bytes = serde_json::to_vec_pretty(&legacy_file).unwrap();
        fs::create_dir_all(legacy_path.parent().unwrap()).unwrap();
        fs::write(&legacy_path, &legacy_bytes).unwrap();

        let store = ComposerHistoryStore::for_data_dir(data_dir.path(), workspace_root);
        assert!(store.load().unwrap().is_empty());
        assert!(!store.path().exists());
        assert_eq!(fs::read(&legacy_path).unwrap(), legacy_bytes);

        let current_entries = VecDeque::from([vec![Segment::text("current entry")]]);
        store.save(&current_entries).unwrap();

        assert_eq!(store.load().unwrap(), current_entries);
        assert_eq!(fs::read(&legacy_path).unwrap(), legacy_bytes);
    }

    #[test]
    fn file_records_workspace_identity_metadata_and_typed_segments() {
        let data_dir = TempDir::new().unwrap();
        let store = ComposerHistoryStore::for_data_dir(data_dir.path(), Path::new("/repo/yoi"));
        let entries = VecDeque::from([vec![
            Segment::text("see "),
            Segment::FileRef {
                path: "src/main.rs".into(),
            },
        ]]);

        store.save(&entries).unwrap();
        let raw: serde_json::Value =
            serde_json::from_slice(&fs::read(store.path()).unwrap()).unwrap();
        assert_eq!(raw["workspace"]["root"], "/repo/yoi");
        assert_eq!(raw["workspace"]["label"], "yoi");
        assert!(raw["workspace"]["key"].as_str().unwrap().len() >= 16);
        assert_eq!(raw["entries"][0][1]["kind"], "file_ref");
        assert_eq!(store.load().unwrap(), entries);
    }

    #[test]
    fn normalization_bounds_blanks_and_consecutive_duplicates() {
        let mut entries = Vec::new();
        entries.push(vec![Segment::text("   ")]);
        entries.push(vec![Segment::text("same")]);
        entries.push(vec![Segment::text("same")]);
        for i in 0..COMPOSER_INPUT_HISTORY_LIMIT + 5 {
            entries.push(vec![Segment::text(format!("entry-{i}"))]);
        }

        let normalized = normalized_entries(entries);
        assert_eq!(normalized.len(), COMPOSER_INPUT_HISTORY_LIMIT);
        assert_eq!(normalized.front(), Some(&vec![Segment::text("entry-5")]));
        assert_eq!(normalized.back(), Some(&vec![Segment::text("entry-34")]));
    }
}
