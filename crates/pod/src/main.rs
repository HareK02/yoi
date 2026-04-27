use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use manifest::paths;
use pod::{Pod, PodController, PodFactory};
use session_store::FsStore;

#[derive(Parser)]
#[command(
    name = "pod",
    about = "Spawn a Pod process from cascaded manifest layers"
)]
struct Cli {
    /// User manifest TOML. Defaults to `<config_dir>/manifest.toml`
    /// (see `manifest::paths`).
    #[arg(long, value_name = "PATH")]
    user_manifest: Option<PathBuf>,

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
}

async fn build_factory(cli: &Cli) -> Result<PodFactory, String> {
    let mut factory = PodFactory::new();

    factory = match &cli.user_manifest {
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

    let factory = match build_factory(&cli).await {
        Ok(f) => f,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };

    let (manifest, loader) = match factory.resolve() {
        Ok(pair) => pair,
        Err(e) => {
            eprintln!("error: failed to resolve manifest cascade: {e}");
            return ExitCode::FAILURE;
        }
    };

    // Initialize persistent store
    let store_dir = cli.store.clone().unwrap_or_else(|| {
        paths::sessions_dir().unwrap_or_else(|| PathBuf::from(".insomnia/sessions"))
    });
    let store = match FsStore::new(&store_dir).await {
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
    eprintln!(
        "INSOMNIA-READY\t{pod_name}\t{}",
        socket_path.display()
    );
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
