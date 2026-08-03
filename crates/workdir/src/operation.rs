use std::fmt;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::WorkdirError;

/// Logical path relative to the bound Workdir root.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct WorkdirPath(String);

impl<'de> Deserialize<'de> for WorkdirPath {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(&value).map_err(serde::de::Error::custom)
    }
}

impl WorkdirPath {
    pub fn root() -> Self {
        Self(String::new())
    }

    pub fn new(value: impl AsRef<str>) -> Result<Self, WorkdirError> {
        let value = value.as_ref();
        if value.is_empty() || value == "." {
            return Ok(Self::root());
        }
        let path = Path::new(value);
        if path.is_absolute() || value.contains('\\') {
            return Err(WorkdirError::InvalidPath(value.to_owned()));
        }

        let mut normalized = PathBuf::new();
        for component in path.components() {
            match component {
                Component::Normal(part) => normalized.push(part),
                Component::CurDir => {}
                Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                    return Err(WorkdirError::InvalidPath(value.to_owned()));
                }
            }
        }
        let value = normalized.to_string_lossy().replace('\\', "/");
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_root(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Display for WorkdirPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0.is_empty() {
            f.write_str(".")
        } else {
            f.write_str(&self.0)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatRequest {
    pub path: WorkdirPath,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatResult {
    pub path: WorkdirPath,
    pub kind: EntryKind,
    pub size: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    File,
    Directory,
    Symlink,
    Other,
}

pub type ContentHash = [u8; 32];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadRequest {
    pub path: WorkdirPath,
    pub offset: usize,
    pub limit: usize,
    pub max_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadResult {
    pub path: WorkdirPath,
    pub bytes: Vec<u8>,
    pub start_line: usize,
    pub total_lines: usize,
    pub content_hash: ContentHash,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WriteRequest {
    pub path: WorkdirPath,
    pub content: Vec<u8>,
    pub expected_hash: Option<ContentHash>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WriteResult {
    pub bytes_written: usize,
    pub created: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditRequest {
    pub path: WorkdirPath,
    pub old_string: String,
    pub new_string: String,
    pub replace_all: bool,
    pub expected_hash: ContentHash,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditResult {
    pub replacements: usize,
    pub bytes_written: usize,
    pub content_hash: ContentHash,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListRequest {
    pub path: WorkdirPath,
    pub limit: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListEntry {
    pub path: WorkdirPath,
    pub kind: EntryKind,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListResult {
    pub entries: Vec<ListEntry>,
    pub total_entries: usize,
    pub total_bytes: u64,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GlobRequest {
    pub pattern: String,
    pub path: WorkdirPath,
    pub limit: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GlobResult {
    pub paths: Vec<WorkdirPath>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GrepOutputMode {
    Content,
    FilesWithMatches,
    Count,
}

impl Default for GrepOutputMode {
    fn default() -> Self {
        Self::FilesWithMatches
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrepRequest {
    pub pattern: String,
    pub path: WorkdirPath,
    pub glob: Option<String>,
    pub file_type: Option<String>,
    pub case_insensitive: bool,
    pub before_context: usize,
    pub after_context: usize,
    pub multiline: bool,
    pub output_mode: GrepOutputMode,
    pub limit: usize,
    pub offset: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrepResult {
    /// Provider-rendered bounded grep report. Keeping rendering here avoids
    /// transferring candidate files across a remote provider boundary.
    pub output: String,
    pub match_count: usize,
    pub matched_files: usize,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CommandHandle(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandRequest {
    pub command: String,
    pub timeout_secs: u64,
    pub output_limit: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandOutputRequest {
    pub handle: CommandHandle,
    pub cursor: usize,
    pub limit: usize,
    pub wait: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandStatus {
    Running,
    Completed,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandOutput {
    pub status: CommandStatus,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub content: String,
    pub next_cursor: Option<usize>,
    pub truncated: bool,
}

#[cfg(test)]
mod tests {
    use super::WorkdirPath;

    #[test]
    fn logical_paths_normalize_only_safe_root_relative_components() {
        assert_eq!(
            WorkdirPath::new("./docs//item.md").unwrap().as_str(),
            "docs/item.md"
        );
        assert!(WorkdirPath::new("../secret").is_err());
        assert!(WorkdirPath::new("docs/../secret").is_err());
        assert!(WorkdirPath::new("/absolute").is_err());
        assert!(WorkdirPath::new(r"..\secret").is_err());
    }

    #[test]
    fn deserialization_cannot_bypass_logical_path_validation() {
        let error = serde_json::from_str::<WorkdirPath>(r#""../secret""#).unwrap_err();
        assert!(error.to_string().contains("invalid Workdir path"));

        let path = serde_json::from_str::<WorkdirPath>(r#""docs/item.md""#).unwrap();
        assert_eq!(path.as_str(), "docs/item.md");
    }
}
