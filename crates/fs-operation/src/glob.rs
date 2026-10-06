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
    let logical_walk_root = traversal
        .as_ref()
        .and_then(|t| t.logical_root())
        .unwrap_or(root);
    let mut descriptor_ignores = traversal
        .as_ref()
        .map(|_| crate::descriptor_ignore::DescriptorIgnores::new(root, logical_walk_root, access))
        .transpose()?;
    let mut visited = 0_usize;
    let mut walk = if traversal.is_some() {
        // Configuration is read via authorized provider opens, never by
        // WalkBuilder's host-path ignore discovery.
        crate::walk::Walk::descriptor(logical_walk_root, access)
    } else {
        crate::walk::Walk::Plain(
            WalkBuilder::new(walker_root)
                .hidden(false)
                .follow_links(false)
                .build(),
        )
    };
    while let Some(entry) = walk.next() {
        let entry = entry?;
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
        let path = entry.path;
        if path.strip_prefix(root).unwrap_or(&path).to_str().is_none() {
            continue;
        }
        let resolved = match access.resolve_access_path(&path) {
            Ok(resolved) if access.is_readable_paths(&path, &resolved) => resolved,
            _ => continue,
        };
        if let Some(ignores) = descriptor_ignores.as_mut() {
            let is_dir = entry.kind.is_some_and(|kind| kind.is_dir());
            if path != logical_walk_root && ignores.is_ignored(&path, is_dir) {
                if is_dir {
                    walk.skip_current_dir();
                }
                continue;
            }
            if is_dir {
                ignores.load_directory(&path, access)?;
                continue;
            }
        }
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
        let Some(logical) = logical.to_str() else {
            continue;
        };
        let Ok(result_path) = FsPath::new(logical) else {
            continue;
        };
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
