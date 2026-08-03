use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::{
    ContentHash, EditRequest, EditResult, EntryKind, FsAccessPolicy, FsError, FsPath, ListEntry,
    ListRequest, ListResult, ReadRequest, ReadResult, StatRequest, StatResult, WriteRequest,
    WriteResult, direct_symlink,
};

/// Execute stat while keeping host paths inside the provider boundary.
pub fn run_stat(
    root: &Path,
    request: StatRequest,
    access: &dyn FsAccessPolicy,
) -> Result<StatResult, FsError> {
    let logical = request.path;
    let path = resolve(root, &logical)?;
    if !access.is_readable(&path) {
        return Err(FsError::OutOfScope(PathBuf::from(logical.as_str())));
    }
    let metadata = fs::symlink_metadata(&path).map_err(|error| map_io(&logical, error))?;
    let kind = if metadata.file_type().is_symlink() {
        EntryKind::Symlink
    } else if metadata.is_file() {
        EntryKind::File
    } else if metadata.is_dir() {
        EntryKind::Directory
    } else {
        EntryKind::Other
    };
    Ok(StatResult {
        path: logical,
        kind,
        size: metadata.len(),
    })
}

pub fn run_read(
    root: &Path,
    request: ReadRequest,
    access: &dyn FsAccessPolicy,
) -> Result<ReadResult, FsError> {
    let logical = request.path;
    let path = resolve(root, &logical)?;
    let path = require_access(&path, &logical, access, false)?;
    let metadata = fs::metadata(&path).map_err(|error| map_io(&logical, error))?;
    if metadata.is_dir() {
        return Err(FsError::IsDirectory(PathBuf::from(logical.as_str())));
    }
    let bytes = fs::read(&path).map_err(|error| map_io(&logical, error))?;
    let content_hash = hash_bytes(&bytes);
    let lines = bytes
        .split_inclusive(|byte| *byte == b'\n')
        .collect::<Vec<_>>();
    let total_lines = lines.len();
    if request.offset > total_lines && request.offset != 0 {
        return Err(FsError::InvalidArgument(format!(
            "offset {} exceeds file length {total_lines}",
            request.offset
        )));
    }
    let end = request
        .offset
        .saturating_add(request.limit)
        .min(total_lines);
    let mut selected = lines[request.offset.min(total_lines)..end]
        .iter()
        .flat_map(|line| line.iter().copied())
        .collect::<Vec<_>>();
    let byte_truncated = selected.len() > request.max_bytes;
    if byte_truncated {
        let mut byte_end = request.max_bytes;
        if let Ok(text) = std::str::from_utf8(&selected) {
            while byte_end > 0 && !text.is_char_boundary(byte_end) {
                byte_end -= 1;
            }
        }
        selected.truncate(byte_end);
    }
    Ok(ReadResult {
        path: logical,
        bytes: selected,
        start_line: request.offset,
        total_lines,
        content_hash,
        truncated: end < total_lines || byte_truncated,
    })
}

pub fn run_write(
    root: &Path,
    request: WriteRequest,
    access: &dyn FsAccessPolicy,
) -> Result<WriteResult, FsError> {
    let logical = request.path;
    let path = resolve(root, &logical)?;
    let created = !path.exists();
    if path.exists() {
        let target = require_access(&path, &logical, access, true)?;
        let metadata = fs::metadata(&target).map_err(|error| map_io(&logical, error))?;
        if metadata.is_dir() {
            return Err(FsError::IsDirectory(PathBuf::from(logical.as_str())));
        }
        let actual = hash_bytes(&fs::read(&target).map_err(|error| map_io(&logical, error))?);
        if request.expected_hash != Some(actual) {
            return Err(FsError::Conflict(logical.as_str().to_string()));
        }
        atomic_write(&target, &request.content, &logical)?;
    } else {
        if request.expected_hash.is_some() {
            return Err(FsError::Conflict(logical.as_str().to_string()));
        }
        let parent = path.parent().ok_or_else(|| {
            FsError::InvalidArgument(format!("{} has no parent", logical.as_str()))
        })?;
        let parent_logical = logical_parent(&logical);
        require_access(parent, &parent_logical, access, true)?;
        atomic_write(&path, &request.content, &logical)?;
    }
    Ok(WriteResult {
        bytes_written: request.content.len(),
        created,
    })
}

pub fn run_edit(
    root: &Path,
    request: EditRequest,
    access: &dyn FsAccessPolicy,
) -> Result<EditResult, FsError> {
    let logical = request.path;
    let path = resolve(root, &logical)?;
    let target = require_access(&path, &logical, access, true)?;
    let bytes = fs::read(&target).map_err(|error| map_io(&logical, error))?;
    let actual_hash = hash_bytes(&bytes);
    if actual_hash != request.expected_hash {
        return Err(FsError::Conflict(logical.as_str().to_string()));
    }
    let content = String::from_utf8(bytes).map_err(|_| {
        FsError::InvalidArgument(format!("{} is not valid UTF-8", logical.as_str()))
    })?;
    let occurrences = content.matches(&request.old_string).count();
    if occurrences == 0 {
        return Err(FsError::InvalidArgument(
            "old_string was not found".to_string(),
        ));
    }
    if !request.replace_all && occurrences != 1 {
        return Err(FsError::InvalidArgument(format!(
            "old_string matched {occurrences} times; set replace_all=true or provide a unique string"
        )));
    }
    let edited = if request.replace_all {
        content.replace(&request.old_string, &request.new_string)
    } else {
        content.replacen(&request.old_string, &request.new_string, 1)
    };
    atomic_write(&target, edited.as_bytes(), &logical)?;
    Ok(EditResult {
        replacements: if request.replace_all { occurrences } else { 1 },
        bytes_written: edited.len(),
        content_hash: hash_bytes(edited.as_bytes()),
    })
}

pub fn run_list(
    root: &Path,
    request: ListRequest,
    access: &dyn FsAccessPolicy,
) -> Result<ListResult, FsError> {
    let logical = request.path;
    let path = resolve(root, &logical)?;
    let path = require_access(&path, &logical, access, false)?;
    let metadata = fs::metadata(&path).map_err(|error| map_io(&logical, error))?;
    if !metadata.is_dir() {
        return Err(FsError::NotDirectory(PathBuf::from(logical.as_str())));
    }
    let mut entries = Vec::new();
    let read_dir = fs::read_dir(&path).map_err(|error| map_io(&logical, error))?;
    for entry in read_dir {
        let entry = entry.map_err(|error| map_io(&logical, error))?;
        let absolute = entry.path();
        if !access.is_readable(&absolute) {
            continue;
        }
        let link_metadata =
            fs::symlink_metadata(&absolute).map_err(|error| map_io(&logical, error))?;
        let is_symlink = link_metadata.file_type().is_symlink();
        let metadata = if is_symlink {
            link_metadata
        } else {
            entry.metadata().map_err(|error| map_io(&logical, error))?
        };
        let kind = if is_symlink {
            EntryKind::Symlink
        } else if metadata.is_file() {
            EntryKind::File
        } else if metadata.is_dir() {
            EntryKind::Directory
        } else {
            EntryKind::Other
        };
        let relative = absolute.strip_prefix(root).map_err(|_| {
            FsError::InvalidArgument("provider returned a path outside its root".to_string())
        })?;
        entries.push(ListEntry {
            path: FsPath::new(relative.to_string_lossy())?,
            kind,
            size: metadata.len(),
        });
    }
    entries.sort_by(|left, right| {
        let left_dir = left.kind == EntryKind::Directory;
        let right_dir = right.kind == EntryKind::Directory;
        right_dir
            .cmp(&left_dir)
            .then_with(|| left.path.as_str().cmp(right.path.as_str()))
    });
    let total_entries = entries.len();
    let total_bytes = entries.iter().map(|entry| entry.size).sum();
    let truncated = entries.len() > request.limit;
    entries.truncate(request.limit);
    Ok(ListResult {
        entries,
        total_entries,
        total_bytes,
        truncated,
    })
}

fn resolve(root: &Path, logical: &FsPath) -> Result<PathBuf, FsError> {
    if !root.is_absolute() {
        return Err(FsError::RelativePath(root.to_path_buf()));
    }
    Ok(if logical.as_str().is_empty() {
        root.to_path_buf()
    } else {
        root.join(logical.as_str())
    })
}

fn require_access(
    path: &Path,
    logical: &FsPath,
    access: &dyn FsAccessPolicy,
    write: bool,
) -> Result<PathBuf, FsError> {
    if let Some(info) = direct_symlink(path) {
        if !info.target_exists {
            return Err(FsError::BrokenSymlink {
                path: PathBuf::from(logical.as_str()),
                link: PathBuf::from(logical.as_str()),
                target: PathBuf::from("<provider-internal target>"),
            });
        }
        let allowed = if write {
            access.is_writable(&info.resolved_path)
        } else {
            access.is_readable(&info.resolved_path)
        };
        if !allowed {
            return Err(FsError::SymlinkOutOfScope {
                path: PathBuf::from(logical.as_str()),
                target: PathBuf::from("<provider-internal target>"),
                required_permission: if write { "write" } else { "read" },
            });
        }
        if write && info.resolved_path.is_dir() {
            return Err(FsError::SymlinkTargetIsDirectory {
                path: PathBuf::from(logical.as_str()),
                target: PathBuf::from("<provider-internal target>"),
            });
        }
        return Ok(info.resolved_path);
    }
    let allowed = if write {
        access.is_writable(path)
    } else {
        access.is_readable(path)
    };
    if allowed {
        Ok(path.to_path_buf())
    } else if write {
        Err(FsError::ReadOnly(PathBuf::from(logical.as_str())))
    } else {
        Err(FsError::OutOfScope(PathBuf::from(logical.as_str())))
    }
}

fn logical_parent(path: &FsPath) -> FsPath {
    let parent = Path::new(path.as_str())
        .parent()
        .unwrap_or_else(|| Path::new(""))
        .to_string_lossy();
    FsPath::new(parent).unwrap_or_else(|_| FsPath::root())
}

fn atomic_write(path: &Path, content: &[u8], logical: &FsPath) -> Result<(), FsError> {
    let parent = path
        .parent()
        .ok_or_else(|| FsError::InvalidArgument(format!("{} has no parent", logical.as_str())))?;
    fs::create_dir_all(parent).map_err(|error| map_io(logical, error))?;
    let mut temporary =
        tempfile::NamedTempFile::new_in(parent).map_err(|error| map_io(logical, error))?;
    temporary
        .write_all(content)
        .map_err(|error| map_io(logical, error))?;
    temporary.flush().map_err(|error| map_io(logical, error))?;
    temporary
        .persist(path)
        .map_err(|error| map_io(logical, error.error))?;
    Ok(())
}

fn hash_bytes(content: &[u8]) -> ContentHash {
    Sha256::digest(content).into()
}

fn map_io(logical: &FsPath, error: std::io::Error) -> FsError {
    match error.kind() {
        std::io::ErrorKind::NotFound => FsError::NotFound(PathBuf::from(logical.as_str())),
        _ => FsError::Io {
            path: PathBuf::from(logical.as_str()),
            source: error,
        },
    }
}
