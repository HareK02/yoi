use std::fmt;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::FsError;

/// Absolute provider-side bounds for one filesystem read.
///
/// `max_source_bytes` limits how much of the source the provider will inspect
/// while computing line and content metadata. `max_response_bytes` limits the
/// bytes retained for the response independently of the caller's requested
/// `ReadRequest::max_bytes`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct BoundedReadLimits {
    pub max_source_bytes: u64,
    pub max_response_bytes: usize,
}

impl BoundedReadLimits {
    /// Conservative defaults for a client-hosted, read-only provider.
    pub const EXTERNAL_DEFAULT: Self = Self {
        max_source_bytes: 64 * 1024 * 1024,
        max_response_bytes: 1024 * 1024,
    };

    pub fn new(max_source_bytes: u64, max_response_bytes: usize) -> Result<Self, FsError> {
        let limits = Self {
            max_source_bytes,
            max_response_bytes,
        };
        limits.validate()?;
        Ok(limits)
    }

    pub fn validate(self) -> Result<(), FsError> {
        if self.max_source_bytes == 0 {
            return Err(FsError::InvalidArgument(
                "max_source_bytes must be greater than zero".to_string(),
            ));
        }
        if self.max_response_bytes == 0 {
            return Err(FsError::InvalidArgument(
                "max_response_bytes must be greater than zero".to_string(),
            ));
        }
        if self.max_response_bytes as u128 > self.max_source_bytes as u128 {
            return Err(FsError::InvalidArgument(
                "max_response_bytes must not exceed max_source_bytes".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BoundedReadLimitsWire {
    max_source_bytes: u64,
    max_response_bytes: usize,
}

impl<'de> Deserialize<'de> for BoundedReadLimits {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = BoundedReadLimitsWire::deserialize(deserializer)?;
        Self::new(wire.max_source_bytes, wire.max_response_bytes).map_err(serde::de::Error::custom)
    }
}

/// Scope-checked filesystem path. Relative paths resolve below the bound
/// Workdir root; absolute paths require an explicit matching scope rule.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct FsPath(String);

impl<'de> Deserialize<'de> for FsPath {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new_scoped(&value).map_err(serde::de::Error::custom)
    }
}

impl FsPath {
    /// Construct a path for a scope-checked operation that may target an
    /// explicitly granted absolute path outside the provider root.
    pub fn new_scoped(value: impl Into<String>) -> Result<Self, FsError> {
        let value = value.into();
        if !Path::new(&value).is_absolute() {
            return Self::new(value);
        }
        if value.contains('\\') {
            return Err(FsError::InvalidPath(value));
        }
        if Path::new(&value)
            .components()
            .any(|component| component == Component::ParentDir)
        {
            return Err(FsError::InvalidPath(value));
        }
        Ok(Self(value))
    }

    pub fn root() -> Self {
        Self(String::new())
    }

    pub fn new(value: impl AsRef<str>) -> Result<Self, FsError> {
        let value = value.as_ref();
        if value.is_empty() || value == "." {
            return Ok(Self::root());
        }
        let path = Path::new(value);
        if path.is_absolute() || value.contains('\\') {
            return Err(FsError::InvalidPath(value.to_owned()));
        }

        let mut normalized = PathBuf::new();
        for component in path.components() {
            match component {
                Component::Normal(part) => normalized.push(part),
                Component::CurDir => {}
                Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                    return Err(FsError::InvalidPath(value.to_owned()));
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

impl fmt::Display for FsPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0.is_empty() {
            f.write_str(".")
        } else {
            f.write_str(&self.0)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StatRequest {
    pub path: FsPath,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StatResult {
    pub path: FsPath,
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
#[serde(deny_unknown_fields)]
pub struct ReadRequest {
    pub path: FsPath,
    pub offset: usize,
    pub limit: usize,
    pub max_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadResult {
    pub path: FsPath,
    pub bytes: Vec<u8>,
    pub start_line: usize,
    pub total_lines: usize,
    pub content_hash: ContentHash,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WriteRequest {
    pub path: FsPath,
    pub content: Vec<u8>,
    pub expected_hash: Option<ContentHash>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WriteResult {
    pub bytes_written: usize,
    pub created: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EditRequest {
    pub path: FsPath,
    pub old_string: String,
    pub new_string: String,
    pub replace_all: bool,
    pub expected_hash: ContentHash,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EditResult {
    pub replacements: usize,
    pub bytes_written: usize,
    pub content_hash: ContentHash,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListRequest {
    pub path: FsPath,
    pub limit: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListEntry {
    pub path: FsPath,
    pub kind: EntryKind,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListResult {
    pub entries: Vec<ListEntry>,
    pub total_entries: usize,
    pub total_bytes: u64,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GlobRequest {
    pub pattern: String,
    pub path: FsPath,
    pub limit: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GlobResult {
    pub paths: Vec<FsPath>,
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
#[serde(deny_unknown_fields)]
pub struct GrepRequest {
    pub pattern: String,
    pub path: FsPath,
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
#[serde(deny_unknown_fields)]
pub struct GrepResult {
    /// Provider-rendered bounded grep report. Keeping rendering here avoids
    /// transferring candidate files across a remote provider boundary.
    pub output: String,
    pub match_count: usize,
    pub matched_files: usize,
    pub truncated: bool,
}
