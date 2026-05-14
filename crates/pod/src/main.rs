use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Parser;
use manifest::{PodManifest, paths};
use pod::{Pod, PodController, PodFactory, PromptLoader};
use session_store::{FsStore, SessionId};

const USER_MANIFEST_ENV: &str = "INSOMNIA_USER_MANIFEST";

#[derive(Debug, Parser)]
#[command(
    name = "pod",
    about = "Spawn a Pod process from manifest layers or a single manifest file"
)]
struct Cli {
    /// Manifest TOML to use directly, without loading user, project, or
    /// overlay layers.
    #[arg(long, value_name = "PATH", conflicts_with_all = ["project", "overlay"])]
    manifest: Option<PathBuf>,

    /// Start the project-manifest walk from this directory. When
    /// omitted, the factory walks up from the current working
    /// directory looking for `.insomnia/manifest.toml`.
    #[arg(long, value_name = "PATH")]
    project: Option<PathBuf>,

    /// Inline TOML string applied as the highest-priority overlay
    /// layer. Example: `--overlay 'pod.name = "dbg"'`.
    #[arg(long, value_name = "TOML")]
    overlay: Option<String>,

    /// Directory for session persistence. Defaults to
    /// `<data_dir>/sessions/` (see `manifest::paths`).
    #[arg(short, long)]
    store: Option<PathBuf>,

    /// Claim a scope allocation pre-registered by a spawning Pod, rather
    /// than installing a new top-level allocation. Used only when this
    /// process is launched by `SpawnPod`; end users should never pass it.
    #[arg(long)]
    adopt: bool,

    /// Socket path of the spawning Pod, for delivering `Method::Notify`
    /// callbacks upward. Required alongside `--adopt`.
    #[arg(long, value_name = "PATH", requires = "adopt")]
    callback: Option<PathBuf>,

    /// Restore a Pod from an existing session. The Pod re-uses the
    /// given session id and appends new turns to the same jsonl;
    /// concurrent writers are prevented by the pod-registry.
    /// Mutually exclusive with `--adopt` (spawned children always start
    /// fresh).
    #[arg(long, value_name = "UUID", conflicts_with = "adopt")]
    session: Option<SessionId>,
}

fn resolve_manifest(cli: &Cli) -> Result<(PodManifest, PromptLoader), String> {
    resolve_manifest_with_user_manifest_env(cli, std::env::var_os(USER_MANIFEST_ENV))
}

fn resolve_manifest_with_user_manifest_env(
    cli: &Cli,
    user_manifest_env: Option<OsString>,
) -> Result<(PodManifest, PromptLoader), String> {
    let user_manifest = user_manifest_path_from_env(user_manifest_env);

    if let Some(path) = &cli.manifest {
        if user_manifest.is_some() {
            return Err(format!(
                "--manifest cannot be used when {USER_MANIFEST_ENV} is set"
            ));
        }
        return load_single_manifest(path);
    }

    let factory = build_factory_with_user_manifest_path(cli, user_manifest)?;
    factory
        .resolve()
        .map_err(|e| format!("failed to resolve manifest cascade: {e}"))
}

fn user_manifest_path_from_env(value: Option<OsString>) -> Option<PathBuf> {
    value.and_then(|value| {
        if value.is_empty() {
            None
        } else {
            Some(PathBuf::from(value))
        }
    })
}

fn load_single_manifest(path: &Path) -> Result<(PodManifest, PromptLoader), String> {
    let toml = std::fs::read_to_string(path)
        .map_err(|e| format!("failed to read manifest {}: {e}", path.display()))?;
    let manifest = PodManifest::from_toml(&toml)
        .map_err(|e| format!("failed to parse manifest {}: {e}", path.display()))?;
    Ok((manifest, PromptLoader::builtins_only()))
}

fn build_factory_with_user_manifest_path(
    cli: &Cli,
    user_manifest: Option<PathBuf>,
) -> Result<PodFactory, String> {
    let mut factory = PodFactory::new();

    factory = match user_manifest {
        Some(path) => factory
            .with_user_manifest(path)
            .map_err(|e| format!("failed to load user manifest: {e}"))?,
        None => factory
            .with_user_manifest_auto()
            .map_err(|e| format!("failed to auto-load user manifest: {e}"))?,
    };

    factory = match &cli.project {
        Some(path) => factory
            .with_project_manifest_from(path)
            .map_err(|e| format!("failed to load project manifest: {e}"))?,
        None => factory
            .with_project_manifest_auto()
            .map_err(|e| format!("failed to auto-load project manifest: {e}"))?,
    };

    if let Some(overlay) = cli.overlay.as_deref() {
        factory = factory
            .with_overlay_toml(overlay)
            .map_err(|e| format!("failed to parse overlay TOML: {e}"))?;
    }

    Ok(factory)
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();

    let (manifest, loader) = match resolve_manifest(&cli) {
        Ok(pair) => pair,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };

    // Initialize persistent store. `paths::sessions_dir()` only
    // returns None when none of INSOMNIA_HOME / INSOMNIA_DATA_DIR /
    // HOME is set — surface that as a hard error to match the
    // runtime-dir resolution below, rather than silently writing to a
    // relative path under cwd.
    let store_dir = match cli.store.clone() {
        Some(p) => p,
        None => match paths::sessions_dir() {
            Some(d) => d,
            None => {
                eprintln!(
                    "error: could not resolve sessions directory \
                     (set --store, INSOMNIA_HOME, INSOMNIA_DATA_DIR, or HOME)"
                );
                return ExitCode::FAILURE;
            }
        },
    };
    let store = match FsStore::new(&store_dir) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: failed to initialize store at {store_dir:?}: {e}");
            return ExitCode::FAILURE;
        }
    };

    let pod = if cli.adopt {
        let callback = match cli.callback.clone() {
            Some(p) => p,
            None => {
                eprintln!("error: --adopt requires --callback");
                return ExitCode::FAILURE;
            }
        };
        match Pod::from_manifest_spawned(manifest, store, loader, callback).await {
            Ok(p) => p,
            Err(e) => {
                eprintln!("error: failed to create spawned pod: {e}");
                return ExitCode::FAILURE;
            }
        }
    } else if let Some(source_session_id) = cli.session {
        match Pod::restore_from_manifest(source_session_id, manifest, store, loader).await {
            Ok(p) => p,
            Err(e) => {
                eprintln!("error: failed to restore pod: {e}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        match Pod::from_manifest(manifest, store, loader).await {
            Ok(p) => p,
            Err(e) => {
                eprintln!("error: failed to create pod: {e}");
                return ExitCode::FAILURE;
            }
        }
    };
    let pod_name = pod.manifest().pod.name.clone();

    // Spawn the controller (starts socket server)
    let runtime_base = match paths::runtime_dir() {
        Some(d) => d,
        None => {
            eprintln!(
                "error: could not resolve runtime directory \
                 (set INSOMNIA_HOME, INSOMNIA_RUNTIME_DIR, XDG_RUNTIME_DIR, or HOME)"
            );
            return ExitCode::FAILURE;
        }
    };
    let (handle, shutdown_rx) = match PodController::spawn(pod, &runtime_base).await {
        Ok(pair) => pair,
        Err(e) => {
            eprintln!("error: failed to start pod controller: {e}");
            return ExitCode::FAILURE;
        }
    };

    let socket_path = handle.runtime_dir.socket_path();
    // Machine-readable ready line for parents that spawned this Pod
    // (e.g. the TUI's interactive `spawn` flow). Tab-separated so a
    // pod name with spaces still parses cleanly. Emit before the
    // human line so a stderr-watching parent sees it first.
    eprintln!("INSOMNIA-READY\t{pod_name}\t{}", socket_path.display());
    eprintln!("pod: {pod_name} listening on {:?}", socket_path);

    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            eprintln!("pod: {pod_name} shutting down (signal)");
        }
        _ = shutdown_rx => {
            eprintln!("pod: {pod_name} shutting down (client request)");
        }
    }

    drop(handle);
    ExitCode::SUCCESS
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

    fn manifest_toml(name: &str, scope: &Path) -> String {
        format!(
            r#"
[pod]
name = "{name}"

[model]
scheme = "anthropic"
model_id = "test-model"

[worker]

[[scope.allow]]
target = "{scope}"
permission = "write"
"#,
            scope = scope.display()
        )
    }

    #[test]
    fn user_manifest_flag_is_not_accepted() {
        let err = Cli::try_parse_from(["pod", "--user-manifest", "manifest.toml"]).unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::UnknownArgument);
    }

    #[test]
    fn manifest_conflicts_with_project_and_overlay() {
        let project_err =
            Cli::try_parse_from(["pod", "--manifest", "manifest.toml", "--project", "."])
                .unwrap_err();
        assert_eq!(project_err.kind(), clap::error::ErrorKind::ArgumentConflict);

        let overlay_err = Cli::try_parse_from([
            "pod",
            "--manifest",
            "manifest.toml",
            "--overlay",
            "pod.name = 'x'",
        ])
        .unwrap_err();
        assert_eq!(overlay_err.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn manifest_conflicts_with_user_manifest_env_when_env_is_non_empty() {
        let tmp = TempDir::new().unwrap();
        let manifest = tmp.path().join("manifest.toml");
        write(&manifest, &manifest_toml("single", tmp.path()));
        let cli = Cli::try_parse_from(["pod", "--manifest", manifest.to_str().unwrap()]).unwrap();

        let err = resolve_manifest_with_user_manifest_env(&cli, Some(OsString::from("user.toml")))
            .unwrap_err();

        assert!(err.contains("--manifest cannot be used"));
        assert!(err.contains(USER_MANIFEST_ENV));
    }

    #[test]
    fn manifest_allows_empty_user_manifest_env() {
        let tmp = TempDir::new().unwrap();
        let manifest = tmp.path().join("manifest.toml");
        write(&manifest, &manifest_toml("single", tmp.path()));
        let cli = Cli::try_parse_from(["pod", "--manifest", manifest.to_str().unwrap()]).unwrap();

        let (manifest, loader) =
            resolve_manifest_with_user_manifest_env(&cli, Some(OsString::new())).unwrap();

        assert_eq!(manifest.pod.name, "single");
        assert!(loader.user_dir().is_none());
        assert!(loader.workspace_dir().is_none());
    }

    #[test]
    fn user_manifest_env_overrides_auto_user_manifest_path() {
        let tmp = TempDir::new().unwrap();
        let user_manifest = tmp.path().join("custom-user.toml");
        write(&user_manifest, &manifest_toml("from-env", tmp.path()));
        let no_project_root = tmp.path().join("no-project");
        std::fs::create_dir_all(&no_project_root).unwrap();
        let cli =
            Cli::try_parse_from(["pod", "--project", no_project_root.to_str().unwrap()]).unwrap();

        let (manifest, _loader) = resolve_manifest_with_user_manifest_env(
            &cli,
            Some(user_manifest.as_os_str().to_os_string()),
        )
        .unwrap();

        assert_eq!(manifest.pod.name, "from-env");
    }

    #[test]
    fn manifest_mode_loads_single_file_with_minimal_prompt_loader() {
        let tmp = TempDir::new().unwrap();
        let single_manifest = tmp.path().join("single.toml");
        write(&single_manifest, &manifest_toml("single-file", tmp.path()));
        std::fs::create_dir_all(tmp.path().join("prompts")).unwrap();
        std::fs::create_dir_all(tmp.path().join(".insomnia").join("prompts")).unwrap();
        let cli =
            Cli::try_parse_from(["pod", "--manifest", single_manifest.to_str().unwrap()]).unwrap();

        let (manifest, loader) = resolve_manifest_with_user_manifest_env(&cli, None).unwrap();

        assert_eq!(manifest.pod.name, "single-file");
        assert!(loader.user_dir().is_none());
        assert!(loader.workspace_dir().is_none());
        assert!(loader.user_pack_file().is_none());
        assert!(loader.workspace_pack_file().is_none());
    }
}
