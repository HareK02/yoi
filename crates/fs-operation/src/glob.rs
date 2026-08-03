use std::path::{Path, PathBuf};

use globset::Glob;
use ignore::WalkBuilder;

use crate::{FsAccessPolicy, FsError, FsPath, GlobRequest, GlobResult, direct_symlink};

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
    if !access.is_readable(base) {
        return Err(FsError::OutOfScope(PathBuf::from(request.path.as_str())));
    }
    if let Some(info) = direct_symlink(base)
        && info.target_exists
        && info.resolved_path.is_dir()
    {
        return Err(FsError::SymlinkDirectoryNotTraversed {
            tool: "Glob",
            path: PathBuf::from(request.path.as_str()),
            target: PathBuf::from("<provider-internal target>"),
        });
    }
    let matcher = Glob::new(&request.pattern)
        .map_err(|error| FsError::InvalidGlob(error.to_string()))?
        .compile_matcher();
    let mut matches = Vec::new();
    for entry in WalkBuilder::new(base).hidden(false).build().flatten() {
        let path = entry.path();
        if !path.is_file() || !access.is_readable(path) {
            continue;
        }
        let relative = path.strip_prefix(base).unwrap_or(path);
        if !matcher.is_match(relative) {
            continue;
        }
        let logical = path.strip_prefix(root).map_err(|_| {
            FsError::InvalidArgument("provider returned a path outside its root".to_string())
        })?;
        matches.push(FsPath::new(logical.to_string_lossy())?);
    }
    matches.sort_by(|left, right| left.as_str().cmp(right.as_str()));
    let truncated = matches.len() > request.limit;
    matches.truncate(request.limit);
    Ok(GlobResult {
        paths: matches,
        truncated,
    })
}
