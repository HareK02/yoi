use std::ffi::OsString;
use std::fs;
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::{
    BoundedReadLimits, ContentHash, EditRequest, EditResult, EntryKind, FsAccessPolicy, FsError,
    FsPath, ListEntry, ListRequest, ListResult, ReadRequest, ReadResult, StatRequest, StatResult,
    WriteRequest, WriteResult, direct_symlink,
};

const READ_BUFFER_BYTES: usize = 16 * 1024;

/// Execute stat while keeping host paths inside the provider boundary.
pub fn run_stat(
    root: &Path,
    request: StatRequest,
    access: &dyn FsAccessPolicy,
) -> Result<StatResult, FsError> {
    let logical = request.path;
    access
        .check_cancelled()
        .map_err(|error| map_io(&logical, error))?;
    let path = resolve(root, &logical)?;
    let resolved = resolve_access_path(&path).map_err(|error| map_io(&logical, error))?;
    if !access.is_readable_paths(&path, &resolved) {
        return Err(FsError::OutOfScope(PathBuf::from(logical.as_str())));
    }
    let metadata = access
        .read_metadata(&path, &resolved)
        .map_err(|error| map_io(&logical, error))?;
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
    run_read_with_limits(root, request, access, None)
}

/// Execute a read while enforcing provider-owned source and response bounds.
///
/// The source is streamed through a fixed-size buffer. Files larger than the
/// configured source bound fail before their content is read, and growth after
/// the metadata check is fenced while streaming.
pub fn run_read_bounded(
    root: &Path,
    request: ReadRequest,
    access: &dyn FsAccessPolicy,
    limits: BoundedReadLimits,
) -> Result<ReadResult, FsError> {
    run_read_with_limits(root, request, access, Some(limits))
}

fn run_read_with_limits(
    root: &Path,
    request: ReadRequest,
    access: &dyn FsAccessPolicy,
    limits: Option<BoundedReadLimits>,
) -> Result<ReadResult, FsError> {
    let logical = request.path;
    access
        .check_cancelled()
        .map_err(|error| map_io(&logical, error))?;
    let path = resolve(root, &logical)?;
    let target = require_access(&path, &logical, access, false, false)?;
    let file = access
        .open_read_file(&path, &target)
        .map_err(|error| map_io(&logical, error))?;
    let metadata = file.metadata().map_err(|error| map_io(&logical, error))?;
    if metadata.is_dir() {
        return Err(FsError::IsDirectory(PathBuf::from(logical.as_str())));
    }
    if !metadata.is_file() {
        return Err(FsError::InvalidArgument(
            "read source must be a regular file".to_string(),
        ));
    }
    if let Some(limits) = limits
        && metadata.len() > limits.max_source_bytes
    {
        return Err(FsError::InvalidArgument(format!(
            "read source size {} exceeds provider limit {}",
            metadata.len(),
            limits.max_source_bytes
        )));
    }

    let response_limit = limits
        .map(|limits| request.max_bytes.min(limits.max_response_bytes))
        .unwrap_or(request.max_bytes);
    let retained_limit = response_limit.saturating_add(4);
    let selected_end = request.offset.saturating_add(request.limit);
    let mut selected = Vec::with_capacity(retained_limit.min(READ_BUFFER_BYTES));
    let mut selected_bytes_seen = 0usize;
    let mut current_line = 0usize;
    let mut source_bytes_seen = 0u64;
    let mut last_byte = None;
    let mut content_hasher = Sha256::new();
    let mut reader = BufReader::with_capacity(READ_BUFFER_BYTES, file);
    let mut buffer = [0u8; READ_BUFFER_BYTES];
    loop {
        access
            .check_cancelled()
            .map_err(|error| map_io(&logical, error))?;
        let read_limit = limits
            .map(|limits| {
                limits
                    .max_source_bytes
                    .saturating_sub(source_bytes_seen)
                    .saturating_add(1)
                    .min(READ_BUFFER_BYTES as u64) as usize
            })
            .unwrap_or(READ_BUFFER_BYTES);
        let read = reader
            .read(&mut buffer[..read_limit])
            .map_err(|error| map_io(&logical, error))?;
        if read == 0 {
            break;
        }
        source_bytes_seen = source_bytes_seen.saturating_add(read as u64);
        if let Some(limits) = limits
            && source_bytes_seen > limits.max_source_bytes
        {
            return Err(FsError::InvalidArgument(format!(
                "read source exceeds provider limit {}",
                limits.max_source_bytes
            )));
        }
        content_hasher.update(&buffer[..read]);
        for byte in &buffer[..read] {
            if current_line >= request.offset && current_line < selected_end {
                selected_bytes_seen = selected_bytes_seen.saturating_add(1);
                if selected.len() < retained_limit {
                    selected.push(*byte);
                }
            }
            if *byte == b'\n' {
                current_line = current_line.saturating_add(1);
            }
            last_byte = Some(*byte);
        }
    }

    let total_lines =
        current_line.saturating_add(usize::from(last_byte.is_some_and(|byte| byte != b'\n')));
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
    let byte_truncated = selected_bytes_seen > response_limit;
    if byte_truncated {
        let mut byte_end = response_limit.min(selected.len());
        while byte_end > 0 {
            match std::str::from_utf8(&selected[..byte_end]) {
                Ok(_) => break,
                Err(error) if error.error_len().is_none() => byte_end -= 1,
                Err(_) => break,
            }
        }
        selected.truncate(byte_end);
    }
    Ok(ReadResult {
        path: logical,
        bytes: selected,
        start_line: request.offset,
        total_lines,
        content_hash: content_hasher.finalize().into(),
        truncated: end < total_lines || byte_truncated,
    })
}

pub fn run_write(
    root: &Path,
    request: WriteRequest,
    access: &dyn FsAccessPolicy,
) -> Result<WriteResult, FsError> {
    let logical = request.path;
    access
        .check_cancelled()
        .map_err(|error| map_io(&logical, error))?;
    let path = resolve(root, &logical)?;
    let created = !path.exists();
    if path.exists() {
        let target = require_access(&path, &logical, access, true, false)?;
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
        let target = require_access(&path, &logical, access, true, true)?;
        atomic_write(&target, &request.content, &logical)?;
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
    access
        .check_cancelled()
        .map_err(|error| map_io(&logical, error))?;
    let path = resolve(root, &logical)?;
    let target = require_access(&path, &logical, access, true, false)?;
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
    access
        .check_cancelled()
        .map_err(|error| map_io(&logical, error))?;
    let path = resolve(root, &logical)?;
    let logical_base = path.clone();
    let path = require_access(&path, &logical, access, false, true)?;
    let metadata = access
        .open_read_file(&logical_base, &path)
        .and_then(|directory| directory.metadata())
        .map_err(|error| map_io(&logical, error))?;
    if !metadata.is_dir() {
        return Err(FsError::NotDirectory(PathBuf::from(logical.as_str())));
    }
    let mut entries = Vec::new();
    let mut retained_path_bytes = 0_usize;
    let mut provider_truncated = false;
    let read_dir = access
        .open_read_dir(&logical_base, &path)
        .map_err(|error| map_io(&logical, error))?;
    for (entry_index, entry) in read_dir.enumerate() {
        access
            .check_cancelled()
            .map_err(|error| map_io(&logical, error))?;
        if entry_index >= crate::MAX_TRAVERSAL_ENTRIES {
            return Err(FsError::InvalidArgument(format!(
                "directory traversal exceeds provider limit {}",
                crate::MAX_TRAVERSAL_ENTRIES
            )));
        }
        let entry = entry.map_err(|error| map_io(&logical, error))?;
        let logical_absolute = logical_base.join(entry.file_name());
        let resolved = match resolve_access_path(&logical_absolute) {
            Ok(resolved) => resolved,
            Err(_) => continue,
        };
        if !access.is_readable_paths(&logical_absolute, &resolved) {
            continue;
        }
        let metadata = access
            .read_metadata(&logical_absolute, &resolved)
            .map_err(|error| map_io(&logical, error))?;
        let kind = if metadata.file_type().is_symlink() {
            EntryKind::Symlink
        } else if metadata.is_file() {
            EntryKind::File
        } else if metadata.is_dir() {
            EntryKind::Directory
        } else {
            EntryKind::Other
        };
        let relative = logical_absolute.strip_prefix(root).map_err(|_| {
            FsError::InvalidArgument("provider returned a path outside its root".to_string())
        })?;
        let result_path = FsPath::new(relative.to_string_lossy())?;
        let retained = result_path.as_str().len().saturating_add(64);
        if retained_path_bytes.saturating_add(retained) > crate::MAX_RESULT_PATH_BYTES {
            provider_truncated = true;
            break;
        }
        retained_path_bytes = retained_path_bytes.saturating_add(retained);
        entries.push(ListEntry {
            path: result_path,
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
    let truncated = provider_truncated || entries.len() > request.limit;
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
    allow_symlink_directory: bool,
) -> Result<PathBuf, FsError> {
    let symlink = direct_symlink(path);
    if let Some(info) = symlink.as_ref()
        && !info.target_exists
    {
        return Err(FsError::BrokenSymlink {
            path: PathBuf::from(logical.as_str()),
            link: PathBuf::from(logical.as_str()),
            target: PathBuf::from("<provider-internal target>"),
        });
    }
    let resolved = resolve_access_path(path).map_err(|error| map_io(logical, error))?;
    if let Some(info) = symlink {
        let allowed = if write {
            access.is_writable_paths(path, &resolved)
        } else {
            access.is_readable_paths(path, &resolved)
        };
        if !allowed {
            return Err(FsError::SymlinkOutOfScope {
                path: PathBuf::from(logical.as_str()),
                target: PathBuf::from("<provider-internal target>"),
                required_permission: if write { "write" } else { "read" },
            });
        }
        if !allow_symlink_directory && info.resolved_path.is_dir() {
            return Err(FsError::SymlinkTargetIsDirectory {
                path: PathBuf::from(logical.as_str()),
                target: PathBuf::from("<provider-internal target>"),
            });
        }
        return Ok(resolved);
    }
    let allowed = if write {
        access.is_writable_paths(path, &resolved)
    } else {
        access.is_readable_paths(path, &resolved)
    };
    if allowed {
        Ok(resolved)
    } else if write {
        Err(FsError::ReadOnly(PathBuf::from(logical.as_str())))
    } else {
        Err(FsError::OutOfScope(PathBuf::from(logical.as_str())))
    }
}

/// Resolve every existing component of an absolute provider path while
/// retaining a missing final tail for create operations. Dangling symlinks are
/// rejected because no resolved authority identity can be established.
pub fn resolve_access_path(path: &Path) -> std::io::Result<PathBuf> {
    let mut cursor = path;
    let mut missing = Vec::<OsString>::new();
    loop {
        match fs::canonicalize(cursor) {
            Ok(mut resolved) => {
                for component in missing.iter().rev() {
                    resolved.push(component);
                }
                return Ok(resolved);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if fs::symlink_metadata(cursor)
                    .is_ok_and(|metadata| metadata.file_type().is_symlink())
                {
                    return Err(error);
                }
                let name = cursor.file_name().ok_or(error)?;
                missing.push(name.to_os_string());
                cursor = cursor.parent().ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        "path has no existing ancestor",
                    )
                })?;
            }
            Err(error) => return Err(error),
        }
    }
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

#[cfg(test)]
mod bounded_read_tests {
    use std::path::{Path, PathBuf};

    use super::*;

    struct RootAccess(PathBuf);

    impl FsAccessPolicy for RootAccess {
        fn is_readable(&self, path: &Path) -> bool {
            path.starts_with(&self.0)
        }

        fn is_writable(&self, _path: &Path) -> bool {
            false
        }
    }

    #[test]
    fn bounded_read_rejects_an_oversized_source_before_loading_content() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let source = root.join("large.log");
        let file = fs::File::create(&source).unwrap();
        file.set_len(4097).unwrap();

        let error = run_read_bounded(
            &root,
            ReadRequest {
                path: FsPath::new("large.log").unwrap(),
                offset: 0,
                limit: 10,
                max_bytes: usize::MAX,
            },
            &RootAccess(root.clone()),
            BoundedReadLimits::new(4096, 1024).unwrap(),
        )
        .unwrap_err();

        assert!(matches!(error, FsError::InvalidArgument(_)));
        assert!(error.to_string().contains("exceeds provider limit 4096"));
    }

    #[test]
    fn bounded_read_caps_retained_response_independently_of_request() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        fs::write(root.join("events.log"), "alpha\nbeta\ngamma\n").unwrap();

        let result = run_read_bounded(
            &root,
            ReadRequest {
                path: FsPath::new("events.log").unwrap(),
                offset: 0,
                limit: 10,
                max_bytes: usize::MAX,
            },
            &RootAccess(root.clone()),
            BoundedReadLimits::new(1024, 5).unwrap(),
        )
        .unwrap();

        assert_eq!(result.bytes, b"alpha");
        assert_eq!(result.total_lines, 3);
        assert!(result.truncated);
    }
}
