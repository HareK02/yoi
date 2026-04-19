//! Builder that assembles a [`PodManifest`] from cascade layers.
//!
//! Layers are merged in order of increasing priority:
//! 1. **Builtin defaults** — in-code defaults, currently empty. Upper
//!    layers provide everything; `TryFrom<PodManifestConfig>` fills in
//!    per-field defaults (`ToolOutputLimits`, `CompactionConfig`, ...).
//! 2. **User manifest** — `$XDG_CONFIG_HOME/insomnia/manifest.toml`
//!    (falling back to `~/.config/insomnia/manifest.toml`).
//! 3. **Project manifest** — closest `.insomnia/manifest.toml` found by
//!    walking up from `cwd`.
//! 4. **Programmatic overlay** — inline TOML string or typed
//!    [`PodManifestConfig`] supplied by the caller (CLI flags, GUI,
//!    spawning Pod, etc.). Highest priority.
//!
//! Path resolution happens **before** merge. Each layer is resolved
//! against its own base directory so that a relative `target = "."`
//! in the project manifest means the project root regardless of how
//! the user or overlay layers lay out their own paths:
//!
//! - user manifest: base = the directory holding the manifest file
//! - project manifest: base = the **project root** (the parent of
//!   `.insomnia/`, not `.insomnia/` itself) so that natural project
//!   manifests with `target = "."` cover the whole workspace
//! - overlay: base = the process's `current_dir()` at the time the
//!   overlay is installed, since an inline TOML string has no file
//!   location of its own

use std::path::{Path, PathBuf};

use manifest::{PodManifest, PodManifestConfig, ResolveError};

use crate::prompt_loader::PromptLoader;

/// Errors raised while building a [`PodManifest`] from cascade layers.
#[derive(Debug, thiserror::Error)]
pub enum FactoryError {
    #[error("failed to read manifest {}: {source}", .path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse manifest {}: {source}", .path.display())]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("failed to parse overlay TOML: {0}")]
    OverlayParse(#[source] toml::de::Error),
    #[error("failed to resolve manifest config: {0}")]
    Resolve(#[source] ResolveError),
    #[error("cannot locate home directory for user manifest lookup")]
    HomeDirUnavailable,
}

/// Builder that accumulates cascade layers and resolves them to a
/// validated [`PodManifest`].
///
/// Call order does not matter — layers are always merged in the fixed
/// priority order listed at the module level. Calling the same
/// `with_*` method twice overwrites the previous value for that slot.
#[derive(Debug, Default)]
pub struct PodFactory {
    /// User layer paired with the directory the manifest lives in
    /// (base for resolving its relative paths).
    user: Option<(PodManifestConfig, PathBuf)>,
    /// Project layer paired with the directory the manifest lives in.
    project: Option<(PodManifestConfig, PathBuf)>,
    /// Programmatic overlays are resolved against the process's
    /// `current_dir()` at the time each call arrives, then merged into
    /// this slot. Storing a pre-resolved (absolute-paths) config means
    /// later overlay calls from a different cwd still work correctly.
    overlay: Option<PodManifestConfig>,
    /// Directory holding the user prompts library — co-located with
    /// the user manifest when loaded. `<user_manifest_dir>/prompts/`.
    user_prompts_dir: Option<PathBuf>,
    /// `<project_root>/.insomnia/prompts/` — co-located with the
    /// project manifest when loaded.
    project_prompts_dir: Option<PathBuf>,
}

impl PodFactory {
    pub fn new() -> Self {
        Self::default()
    }

    /// Attempt to load the user manifest from the XDG config directory.
    ///
    /// Looks at `$XDG_CONFIG_HOME/insomnia/manifest.toml` first, then
    /// falls back to `$HOME/.config/insomnia/manifest.toml`. If the
    /// resolved file does not exist the call is a no-op — user
    /// manifests are optional.
    pub fn with_user_manifest_auto(mut self) -> Result<Self, FactoryError> {
        let path = user_manifest_path()?;
        if path.exists() {
            let base = manifest_base(&path)?;
            self.user = Some((read_config_file(&path)?, base.clone()));
            self.user_prompts_dir = Some(base.join("prompts"));
        }
        Ok(self)
    }

    /// Load the user manifest from an explicit path. The file must
    /// exist; missing files are an error (unlike the `_auto` variant).
    pub fn with_user_manifest(mut self, path: impl AsRef<Path>) -> Result<Self, FactoryError> {
        let path = path.as_ref();
        let base = manifest_base(path)?;
        self.user = Some((read_config_file(path)?, base.clone()));
        self.user_prompts_dir = Some(base.join("prompts"));
        Ok(self)
    }

    /// Walk up from `cwd` looking for a `.insomnia/manifest.toml` and
    /// load it as the project layer. If no project root is found the
    /// call is a no-op.
    pub fn with_project_manifest_auto(mut self) -> Result<Self, FactoryError> {
        let cwd = std::env::current_dir().map_err(|source| FactoryError::Io {
            path: PathBuf::from("."),
            source,
        })?;
        if let Some(path) = find_project_manifest(&cwd) {
            self.install_project_manifest(&path)?;
        }
        Ok(self)
    }

    /// Walk up from `start` looking for a `.insomnia/manifest.toml`.
    /// Explicit variant of [`with_project_manifest_auto`] for tests.
    pub fn with_project_manifest_from(
        mut self,
        start: impl AsRef<Path>,
    ) -> Result<Self, FactoryError> {
        if let Some(path) = find_project_manifest(start.as_ref()) {
            self.install_project_manifest(&path)?;
        }
        Ok(self)
    }

    /// Shared setup for `with_project_manifest_auto` / `_from`: record
    /// the manifest's project root as the base for relative-path
    /// resolution (the parent of `.insomnia/`, not `.insomnia/` itself)
    /// so `target = "."` in a project manifest means the project root.
    /// `prompts/` still lives inside `.insomnia/`.
    fn install_project_manifest(&mut self, path: &Path) -> Result<(), FactoryError> {
        let insomnia_dir = manifest_base(path)?;
        let project_root = insomnia_dir
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| insomnia_dir.clone());
        self.project = Some((read_config_file(path)?, project_root));
        self.project_prompts_dir = Some(insomnia_dir.join("prompts"));
        Ok(())
    }

    /// Install a programmatic overlay parsed from a TOML string. Any
    /// relative paths in the overlay are resolved against the process's
    /// current working directory at the time of this call — an inline
    /// TOML string has no file location of its own.
    pub fn with_overlay_toml(mut self, toml: &str) -> Result<Self, FactoryError> {
        let config = PodManifestConfig::from_toml(toml).map_err(FactoryError::OverlayParse)?;
        self.overlay = Some(resolve_and_merge_overlay(self.overlay, config)?);
        Ok(self)
    }

    /// Install a programmatic overlay from an already-parsed config.
    /// Behaves like [`Self::with_overlay_toml`] regarding relative paths.
    pub fn with_overlay_config(mut self, config: PodManifestConfig) -> Result<Self, FactoryError> {
        self.overlay = Some(resolve_and_merge_overlay(self.overlay, config)?);
        Ok(self)
    }

    /// Build a [`PromptLoader`] that reflects the user / project
    /// prompt directories registered with this factory (a sibling of
    /// each manifest file: `prompts/`). Missing directories are
    /// silently skipped.
    fn build_prompt_loader(&self) -> PromptLoader {
        let user = self
            .user_prompts_dir
            .as_ref()
            .filter(|p| p.is_dir())
            .cloned();
        let project = self
            .project_prompts_dir
            .as_ref()
            .filter(|p| p.is_dir())
            .cloned();
        PromptLoader::new(user, project)
    }

    /// Merge all installed layers, convert the result to a validated
    /// [`PodManifest`], and return it together with a [`PromptLoader`]
    /// that reflects the user / project prompt directories. The loader
    /// feeds `{% include "name" %}` references in the Pod's system
    /// prompt template.
    ///
    /// Each layer is resolved to absolute paths against its own base
    /// (see module docs) **before** merge, so scope rules and
    /// `api_key_file` paths from different layers do not accidentally
    /// inherit another layer's base.
    ///
    /// The base layer is [`PodManifestConfig::builtin_defaults`] so
    /// every per-field default flows through a single source of truth
    /// (see [`manifest::defaults`]).
    pub fn resolve(self) -> Result<(PodManifest, PromptLoader), FactoryError> {
        let loader = self.build_prompt_loader();
        let merged = PodManifestConfig::builtin_defaults();
        let merged = match self.user {
            Some((user, base)) => merged.merge(user.resolve_paths(&base)),
            None => merged,
        };
        let merged = match self.project {
            Some((project, base)) => merged.merge(project.resolve_paths(&base)),
            None => merged,
        };
        let merged = match self.overlay {
            Some(overlay) => merged.merge(overlay),
            None => merged,
        };
        let manifest = PodManifest::try_from(merged).map_err(FactoryError::Resolve)?;
        Ok((manifest, loader))
    }
}

fn manifest_base(path: &Path) -> Result<PathBuf, FactoryError> {
    let parent = path.parent().ok_or_else(|| FactoryError::Io {
        path: path.to_path_buf(),
        source: std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "manifest path has no parent directory",
        ),
    })?;
    // Absolutise against cwd so later path joins produce absolute
    // results regardless of whether the caller passed a relative
    // manifest path.
    if parent.is_absolute() {
        Ok(parent.to_path_buf())
    } else {
        let cwd = std::env::current_dir().map_err(|source| FactoryError::Io {
            path: PathBuf::from("."),
            source,
        })?;
        Ok(cwd.join(parent))
    }
}

fn resolve_and_merge_overlay(
    existing: Option<PodManifestConfig>,
    incoming: PodManifestConfig,
) -> Result<PodManifestConfig, FactoryError> {
    let cwd = std::env::current_dir().map_err(|source| FactoryError::Io {
        path: PathBuf::from("."),
        source,
    })?;
    let resolved = incoming.resolve_paths(&cwd);
    Ok(match existing {
        Some(prev) => prev.merge(resolved),
        None => resolved,
    })
}

fn user_manifest_path() -> Result<PathBuf, FactoryError> {
    if let Ok(dir) = std::env::var("XDG_CONFIG_HOME") {
        if !dir.is_empty() {
            return Ok(PathBuf::from(dir).join("insomnia").join("manifest.toml"));
        }
    }
    let home = std::env::var("HOME").map_err(|_| FactoryError::HomeDirUnavailable)?;
    Ok(PathBuf::from(home)
        .join(".config")
        .join("insomnia")
        .join("manifest.toml"))
}

fn find_project_manifest(start: &Path) -> Option<PathBuf> {
    let start = start.canonicalize().ok().unwrap_or_else(|| start.to_path_buf());
    let mut cur: Option<&Path> = Some(start.as_path());
    while let Some(dir) = cur {
        let candidate = dir.join(".insomnia").join("manifest.toml");
        if candidate.is_file() {
            return Some(candidate);
        }
        cur = dir.parent();
    }
    None
}

fn read_config_file(path: &Path) -> Result<PodManifestConfig, FactoryError> {
    let toml = std::fs::read_to_string(path).map_err(|source| FactoryError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    PodManifestConfig::from_toml(&toml).map_err(|source| FactoryError::Parse {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, contents).unwrap();
    }

    #[test]
    fn resolve_overlay_only() {
        let tmp = TempDir::new().unwrap();
        let pwd = tmp.path().canonicalize().unwrap();
        let overlay = format!(
            r#"
[pod]
name = "solo"

[model]
scheme = "anthropic"
model_id = "claude-sonnet-4-20250514"

[[scope.allow]]
target = "{pwd}"
permission = "write"
"#,
            pwd = pwd.display()
        );
        let manifest = PodFactory::new()
            .with_overlay_toml(&overlay)
            .unwrap()
            .resolve()
            .unwrap();
        let manifest = manifest.0;
        assert_eq!(manifest.pod.name, "solo");
    }

    #[test]
    fn overlay_stacking_merges_in_place() {
        let tmp = TempDir::new().unwrap();
        let pwd = tmp.path().canonicalize().unwrap();
        let user_cfg = PodManifestConfig::from_toml(&format!(
            r#"
[model]
scheme = "anthropic"
model_id = "user-model"

[[scope.allow]]
target = "{pwd}"
permission = "read"
"#,
            pwd = pwd.display()
        ))
        .unwrap();
        let project_cfg = PodManifestConfig::from_toml(&format!(
            r#"
[model]
model_id = "project-model"

[[scope.allow]]
target = "{pwd}"
permission = "write"
"#,
            pwd = pwd.display()
        ))
        .unwrap();
        let overlay_cfg = PodManifestConfig::from_toml(
            r#"
[pod]
name = "overlay-name"
"#,
        )
        .unwrap();

        let (manifest, _loader) = PodFactory::new()
            .with_overlay_config(user_cfg)
            .unwrap()
            .with_overlay_config(project_cfg)
            .unwrap()
            .with_overlay_config(overlay_cfg)
            .unwrap()
            .resolve()
            .unwrap();

        // Note: stacking via with_overlay_config merges into one
        // overlay layer so later calls win. This also exercises the
        // scope union across layers (two allow rules).
        assert_eq!(manifest.pod.name, "overlay-name");
        assert_eq!(manifest.model.model_id, "project-model");
        assert_eq!(manifest.scope.allow.len(), 2);
    }

    #[test]
    fn cascade_priority_layer_ordering() {
        let tmp = TempDir::new().unwrap();
        let pwd = tmp.path().canonicalize().unwrap();

        // Simulate distinct user / project / overlay layers by using
        // the dedicated slots on the factory.
        let user = tmp.path().join("user.toml");
        write(
            &user,
            &format!(
                r#"
[pod]
name = "from-user"

[model]
scheme = "anthropic"
model_id = "user-model"

[[scope.allow]]
target = "{pwd}"
permission = "write"
"#,
                pwd = pwd.display()
            ),
        );

        let project_root = tmp.path().join("proj");
        let project_manifest = project_root.join(".insomnia").join("manifest.toml");
        write(
            &project_manifest,
            r#"
[model]
model_id = "project-model"
"#,
        );

        let (manifest, _loader) = PodFactory::new()
            .with_user_manifest(&user)
            .unwrap()
            .with_project_manifest_from(&project_root)
            .unwrap()
            .resolve()
            .unwrap();

        // project layer overrides user layer on model.model_id
        assert_eq!(manifest.model.model_id, "project-model");
        // user layer provides the rest
        assert_eq!(manifest.pod.name, "from-user");
    }

    #[test]
    fn project_manifest_walks_up_from_nested_dir() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let project_manifest = root.join(".insomnia").join("manifest.toml");
        write(
            &project_manifest,
            &format!(
                r#"
[pod]
name = "walked-up"

[model]
scheme = "anthropic"
model_id = "claude-sonnet-4-20250514"

[[scope.allow]]
target = "{root}"
permission = "write"
"#,
                root = root.display()
            ),
        );

        let nested = root.join("a").join("b").join("c");
        std::fs::create_dir_all(&nested).unwrap();

        let manifest = PodFactory::new()
            .with_project_manifest_from(&nested)
            .unwrap()
            .resolve()
            .unwrap();
        let manifest = manifest.0;
        assert_eq!(manifest.pod.name, "walked-up");
    }

    #[test]
    fn missing_project_root_is_ok() {
        let tmp = TempDir::new().unwrap();
        let pwd = tmp.path().canonicalize().unwrap();
        let overlay = format!(
            r#"
[pod]
name = "standalone"

[model]
scheme = "anthropic"
model_id = "m"

[[scope.allow]]
target = "{pwd}"
permission = "write"
"#,
            pwd = pwd.display()
        );

        // The temp dir has no .insomnia/ — walking up should skip the
        // project layer silently.
        let manifest = PodFactory::new()
            .with_project_manifest_from(&pwd)
            .unwrap()
            .with_overlay_toml(&overlay)
            .unwrap()
            .resolve()
            .unwrap();
        let manifest = manifest.0;
        assert_eq!(manifest.pod.name, "standalone");
    }

    #[test]
    fn user_manifest_relative_paths_resolve_against_its_directory() {
        // user manifest at <tmp>/cfg/manifest.toml with a relative
        // scope target `./workspace` must resolve to <tmp>/cfg/workspace.
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let cfg_dir = root.join("cfg");
        std::fs::create_dir_all(&cfg_dir).unwrap();
        let workspace = cfg_dir.join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();

        let user = cfg_dir.join("manifest.toml");
        write(
            &user,
            r#"
[pod]
name = "rel-user"

[model]
scheme = "anthropic"
model_id = "m"

[[scope.allow]]
target = "./workspace"
permission = "write"
"#,
        );

        let (manifest, _loader) = PodFactory::new()
            .with_user_manifest(&user)
            .unwrap()
            .resolve()
            .unwrap();
        assert_eq!(manifest.scope.allow[0].target, workspace);
    }

    #[test]
    fn project_manifest_relative_paths_resolve_against_project_root() {
        // `.insomnia/manifest.toml` is the marker for the project, but
        // the intuitive base for its relative paths is the project
        // root (the parent of `.insomnia/`) — `target = "."` in a
        // project manifest should cover the whole workspace, not the
        // `.insomnia/` subdir.
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let insomnia_dir = root.join(".insomnia");
        std::fs::create_dir_all(&insomnia_dir).unwrap();
        let project_manifest = insomnia_dir.join("manifest.toml");
        write(
            &project_manifest,
            r#"
[pod]
name = "rel-project"

[model]
scheme = "anthropic"
model_id = "m"

[[scope.allow]]
target = "."
permission = "read"

[[scope.allow]]
target = "src"
permission = "write"
"#,
        );

        let (manifest, _loader) = PodFactory::new()
            .with_project_manifest_from(&root)
            .unwrap()
            .resolve()
            .unwrap();
        assert_eq!(manifest.scope.allow[0].target, root);
        assert_eq!(manifest.scope.allow[1].target, root.join("src"));
    }

    #[test]
    fn resolve_produces_loader_with_workspace_prompts_dir() {
        use crate::system_prompt::{SystemPromptContext, SystemPromptTemplate};
        use manifest::{Permission, Scope, ScopeConfig, ScopeRule};

        let tmp = TempDir::new().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        // .insomnia/manifest.toml and .insomnia/prompts/local.md
        let manifest_path = root.join(".insomnia").join("manifest.toml");
        write(
            &manifest_path,
            &format!(
                r#"
[pod]
name = "factory-pod"

[model]
scheme = "anthropic"
model_id = "m"

[[scope.allow]]
target = "{root}"
permission = "write"
"#,
                root = root.display()
            ),
        );
        let workspace_prompts_dir = root.join(".insomnia").join("prompts");
        std::fs::create_dir_all(&workspace_prompts_dir).unwrap();
        std::fs::write(
            workspace_prompts_dir.join("local.md"),
            "WORKSPACE-BODY from {{ cwd }}",
        )
        .unwrap();

        let (_manifest, loader) = PodFactory::new()
            .with_project_manifest_from(&root)
            .unwrap()
            .resolve()
            .unwrap();

        // The workspace prompt must be reachable via $workspace/local.
        let tmpl = SystemPromptTemplate::parse("$workspace/local", loader).unwrap();
        let scope_cfg = ScopeConfig {
            allow: vec![ScopeRule {
                target: root.clone(),
                permission: Permission::Write,
                recursive: true,
            }],
            deny: Vec::new(),
        };
        let scope = Scope::from_config(&scope_cfg).unwrap();
        let ctx = SystemPromptContext {
            now: chrono::Utc::now(),
            cwd: &root,
            scope: &scope,
            tool_names: Vec::new(),
            agents_md: None,
        };
        let rendered = tmpl.render(&ctx).unwrap();
        assert!(
            rendered.starts_with("WORKSPACE-BODY"),
            "expected workspace body, got: {rendered}"
        );
    }

    #[test]
    fn resolve_fails_on_missing_required_field() {
        let tmp = TempDir::new().unwrap();
        let pwd = tmp.path().canonicalize().unwrap();
        // pod.name missing — resolver must reject.
        let overlay = format!(
            r#"
[model]
scheme = "anthropic"
model_id = "m"

[[scope.allow]]
target = "{pwd}"
permission = "write"
"#,
            pwd = pwd.display()
        );
        let err = PodFactory::new()
            .with_overlay_toml(&overlay)
            .unwrap()
            .resolve()
            .unwrap_err();
        assert!(matches!(err, FactoryError::Resolve(_)));
    }
}
