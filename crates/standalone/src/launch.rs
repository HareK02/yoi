use std::path::{Path, PathBuf};

use manifest::{
    ProfileExecutionTarget, ProfileResolveOptions, ProfileResolver, ProfileSelector,
    ResolvedProfile,
};
use thiserror::Error;
use worker::PromptCatalogSource;

/// Process launch input resolved before any Worker/session side effect occurs.
#[derive(Debug, Clone)]
pub struct StandaloneLaunchConfig {
    pub cwd: PathBuf,
    pub state_dir: PathBuf,
    pub profile: ProfileSelector,
    pub worker_name: String,
}

pub struct ResolvedStandaloneLaunch {
    pub cwd: PathBuf,
    pub state_dir: PathBuf,
    pub profile: ResolvedProfile,
    pub prompt_catalog: PromptCatalogSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum StandaloneLaunchError {
    #[error("the standalone working directory is unavailable")]
    WorkingDirectoryUnavailable,
    #[error("path-based profiles are not standalone launch authority")]
    PathProfileUnsupported,
    #[error("the standalone profile could not be resolved")]
    ProfileResolutionFailed,
}

impl StandaloneLaunchConfig {
    pub fn new(
        cwd: impl Into<PathBuf>,
        state_dir: impl Into<PathBuf>,
        profile: ProfileSelector,
        worker_name: impl Into<String>,
    ) -> Self {
        Self {
            cwd: cwd.into(),
            state_dir: state_dir.into(),
            profile,
            worker_name: worker_name.into(),
        }
    }

    /// Resolve only built-in/XDG profile authority and bind standalone scope
    /// to the canonical process cwd. Repository-local profile discovery is
    /// deliberately not part of this path.
    pub fn resolve(self) -> Result<ResolvedStandaloneLaunch, StandaloneLaunchError> {
        if matches!(self.profile, ProfileSelector::Path { .. }) {
            return Err(StandaloneLaunchError::PathProfileUnsupported);
        }
        let cwd = canonical_directory(&self.cwd)?;
        let profile = ProfileResolver::new()
            .with_workspace_base(&cwd)
            .resolve_for_target(
                &self.profile,
                ProfileResolveOptions {
                    worker_name: Some(self.worker_name),
                },
                ProfileExecutionTarget::Standalone,
            )
            .map_err(|_| StandaloneLaunchError::ProfileResolutionFailed)?;

        Ok(ResolvedStandaloneLaunch {
            cwd,
            state_dir: self.state_dir,
            profile,
            prompt_catalog: PromptCatalogSource::builtins_only(),
        })
    }
}

fn canonical_directory(path: &Path) -> Result<PathBuf, StandaloneLaunchError> {
    let path = std::fs::canonicalize(path)
        .map_err(|_| StandaloneLaunchError::WorkingDirectoryUnavailable)?;
    if !path.is_dir() {
        return Err(StandaloneLaunchError::WorkingDirectoryUnavailable);
    }
    Ok(path)
}
