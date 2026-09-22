use std::path::{Path, PathBuf};

use globset::Glob;
use ignore::WalkBuilder;

use crate::{FsAccessPolicy, FsError, FsPath, GlobRequest, GlobResult};

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
    access.check_cancelled().map_err(|source| FsError::Io {
        path: PathBuf::from(request.path.as_str()),
        source,
    })?;
    let base_resolved = access
        .resolve_access_path(base)
        .map_err(|error| FsError::Io {
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
    let traversal = access
        .open_traversal_root(base, &base_resolved)
        .map_err(|source| FsError::Io {
            path: PathBuf::from(request.path.as_str()),
            source,
        })?;
    let walker_root = traversal
        .as_ref()
        .map(|traversal| traversal.path())
        .unwrap_or(base);
    let mut walker = WalkBuilder::new(walker_root);
    walker.hidden(false).follow_links(false);
    if traversal.is_some() {
        // Descriptor providers must never let WalkBuilder open pathname-based
        // ignore files, which may be symlinked outside the pinned root. Glob's
        // explicit pattern remains the complete selection contract.
        walker
            .git_ignore(false)
            .git_global(false)
            .git_exclude(false)
            .ignore(false)
            .parents(false);
        let search_relative = base.strip_prefix(root).map_err(|_| {
            FsError::InvalidArgument("glob base is outside its provider root".to_string())
        })?;
        let search_relative = search_relative.to_path_buf();
        let filter_root = walker_root.to_path_buf();
        walker.filter_entry(move |entry| {
            entry
                .path()
                .strip_prefix(&filter_root)
                .is_ok_and(|relative| {
                    relative.starts_with(&search_relative) || search_relative.starts_with(relative)
                })
        });
    }
    let mut visited = 0_usize;
    for entry in walker.build().flatten() {
        access.check_cancelled().map_err(|source| FsError::Io {
            path: PathBuf::from(request.path.as_str()),
            source,
        })?;
        visited = visited.saturating_add(1);
        if visited > crate::MAX_TRAVERSAL_ENTRIES {
            return Err(FsError::InvalidArgument(format!(
                "glob traversal exceeds provider limit {}",
                crate::MAX_TRAVERSAL_ENTRIES
            )));
        }
        let walked_path = entry.path();
        let path = if traversal.is_some() {
            let relative = walked_path.strip_prefix(walker_root).map_err(|_| {
                FsError::InvalidArgument(
                    "descriptor traversal returned a path outside its root".to_string(),
                )
            })?;
            root.join(relative)
        } else {
            walked_path.to_path_buf()
        };
        let resolved = match access.resolve_access_path(&path) {
            Ok(resolved) if access.is_readable_paths(&path, &resolved) => resolved,
            _ => continue,
        };
        let Ok(file) = access.open_read_file(&path, &resolved) else {
            continue;
        };
        if !file.metadata().is_ok_and(|metadata| metadata.is_file()) {
            continue;
        }
        let relative = path.strip_prefix(base).unwrap_or(&path);
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
