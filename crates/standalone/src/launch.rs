use std::path::{Path, PathBuf};

use manifest::{
    BUILTIN_DEFAULT_PROFILE, BUILTIN_STANDALONE_PROFILE, ProfileDiscovery, ProfileExecutionTarget,
    ProfileRegistry, ProfileRegistrySource, ProfileResolveOptions, ProfileResolver,
    ProfileSelector, ResolvedProfile, validate_profile_execution_target,
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
        let registry = ProfileDiscovery::user_settings()
            .discover()
            .map_err(|_| StandaloneLaunchError::ProfileResolutionFailed)?;
        let selector = standalone_profile_selector(&self.profile, &registry)
            .map_err(|_| StandaloneLaunchError::ProfileResolutionFailed)?;
        let profile = ProfileResolver::new()
            .with_workspace_base(&cwd)
            .resolve_from_registry(
                &selector,
                &registry,
                ProfileResolveOptions {
                    worker_name: Some(self.worker_name),
                },
            )
            .map_err(|_| StandaloneLaunchError::ProfileResolutionFailed)?;
        validate_profile_execution_target(&profile.manifest, ProfileExecutionTarget::Standalone)
            .map_err(|_| StandaloneLaunchError::ProfileResolutionFailed)?;

        Ok(ResolvedStandaloneLaunch {
            cwd,
            state_dir: self.state_dir,
            profile,
            prompt_catalog: PromptCatalogSource::builtins_only(),
        })
    }
}

fn standalone_profile_selector(
    requested: &ProfileSelector,
    registry: &ProfileRegistry,
) -> Result<ProfileSelector, manifest::ProfileError> {
    if requested != &ProfileSelector::Default
        || registry.default_entry()?.qualified_name() != BUILTIN_DEFAULT_PROFILE
    {
        return Ok(requested.clone());
    }
    Ok(ProfileSelector::source_named(
        ProfileRegistrySource::Builtin,
        BUILTIN_STANDALONE_PROFILE
            .strip_prefix("builtin:")
            .expect("built-in standalone selector must be source-qualified"),
    ))
}

fn canonical_directory(path: &Path) -> Result<PathBuf, StandaloneLaunchError> {
    let path = std::fs::canonicalize(path)
        .map_err(|_| StandaloneLaunchError::WorkingDirectoryUnavailable)?;
    if !path.is_dir() {
        return Err(StandaloneLaunchError::WorkingDirectoryUnavailable);
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_default_maps_to_standalone_only_for_standalone_launch() {
        let registry = ProfileDiscovery::with_sources(None, None)
            .discover()
            .unwrap();
        assert_eq!(
            standalone_profile_selector(&ProfileSelector::Default, &registry).unwrap(),
            ProfileSelector::source_named(ProfileRegistrySource::Builtin, "standalone")
        );
        let explicit_default =
            ProfileSelector::source_named(ProfileRegistrySource::Builtin, "default");
        assert_eq!(
            standalone_profile_selector(&explicit_default, &registry).unwrap(),
            explicit_default
        );
        assert_eq!(
            registry.default_entry().unwrap().qualified_name(),
            BUILTIN_DEFAULT_PROFILE
        );
    }

    #[test]
    fn operator_default_remains_the_standalone_default() {
        let temp = tempfile::tempdir().unwrap();
        let registry_path = temp.path().join("profiles.toml");
        std::fs::write(
            &registry_path,
            "default = 'operator'\n[profile]\noperator = 'operator.toml'\n",
        )
        .unwrap();
        let registry = ProfileDiscovery::with_sources(Some(registry_path), None)
            .discover()
            .unwrap();
        assert_eq!(
            standalone_profile_selector(&ProfileSelector::Default, &registry).unwrap(),
            ProfileSelector::Default
        );
        assert_eq!(
            registry.default_entry().unwrap().qualified_name(),
            "user:operator"
        );
    }
}
