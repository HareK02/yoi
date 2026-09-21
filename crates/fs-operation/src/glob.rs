use std::path::{Path, PathBuf};

use globset::Glob;
use ignore::WalkBuilder;

use crate::{FsAccessPolicy, FsError, FsPath, GlobRequest, GlobResult, resolve_access_path};

/// Execute a bounded glob entirely inside the provider process.
pub fn run_glob(
    root: &Path,
    base: &Path,
    request: GlobRequest,
    access: &dyn FsAccessPolicy,
) -> Result<GlobResult, FsError> {
    if !root.is_absolute() {
        return Err(FsError::RelativePath(root.to_path_buf()));
    }
    let base_resolved = resolve_access_path(base).map_err(|error| FsError::Io {
        path: PathBuf::from(request.path.as_str()),
        source: error,
    })?;
    if !access.is_readable_paths(base, &base_resolved) {
        return Err(FsError::OutOfScope(PathBuf::from(request.path.as_str())));
    }
    let matcher = Glob::new(&request.pattern)
        .map_err(|error| FsError::InvalidGlob(error.to_string()))?
        .compile_matcher();
    let mut matches = Vec::new();
    let mut retained_path_bytes = 0_usize;
    let mut provider_truncated = false;
    let mut walker = WalkBuilder::new(base);
    walker.hidden(false).follow_links(false);
    let mut visited = 0_usize;
    for entry in walker.build().flatten() {
        visited = visited.saturating_add(1);
        if visited > crate::MAX_TRAVERSAL_ENTRIES {
            return Err(FsError::InvalidArgument(format!(
                "glob traversal exceeds provider limit {}",
                crate::MAX_TRAVERSAL_ENTRIES
            )));
        }
        let path = entry.path();
        let readable = resolve_access_path(path)
            .is_ok_and(|resolved| access.is_readable_paths(path, &resolved));
        if !path.is_file() || !readable {
            continue;
        }
        let Ok(file) = access.open_read_file(path, path) else {
            continue;
        };
        if !file.metadata().is_ok_and(|metadata| metadata.is_file()) {
            continue;
        }
        let relative = path.strip_prefix(base).unwrap_or(path);
        if !matcher.is_match(relative) {
            continue;
        }
        let logical = path.strip_prefix(root).map_err(|_| {
            FsError::InvalidArgument("provider returned a path outside its root".to_string())
        })?;
        let result_path = FsPath::new(logical.to_string_lossy())?;
        let retained = result_path.as_str().len().saturating_add(16);
        if retained_path_bytes.saturating_add(retained) > crate::MAX_RESULT_PATH_BYTES {
            provider_truncated = true;
            break;
        }
        retained_path_bytes = retained_path_bytes.saturating_add(retained);
        matches.push(result_path);
    }
    matches.sort_by(|left, right| left.as_str().cmp(right.as_str()));
    let truncated = provider_truncated || matches.len() > request.limit;
    matches.truncate(request.limit);
    Ok(GlobResult {
        paths: matches,
        truncated,
    })
}
