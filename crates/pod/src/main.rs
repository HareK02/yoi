use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Parser;
use pod::{Pod, PodController};
use session_store::FsStore;

#[derive(Parser)]
#[command(name = "pod", about = "Run a Pod process from a manifest file")]
struct Cli {
    /// Path to the manifest TOML file
    #[arg(short, long)]
    manifest: PathBuf,

    /// Directory for session persistence (default: ~/.insomnia/sessions/)
    #[arg(short, long)]
    store: Option<PathBuf>,
}

fn default_store_dir() -> Result<PathBuf, std::io::Error> {
    let home = std::env::var("HOME")
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::NotFound, "HOME is not set"))?;
    Ok(PathBuf::from(home).join(".insomnia").join("sessions"))
}

fn default_runtime_dir() -> Result<PathBuf, std::io::Error> {
    if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
        Ok(PathBuf::from(runtime_dir).join("insomnia"))
    } else if let Ok(home) = std::env::var("HOME") {
        Ok(PathBuf::from(home).join(".insomnia").join("run"))
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "neither XDG_RUNTIME_DIR nor HOME is set",
        ))
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();

    // Read and parse the manifest
    let toml_str = match tokio::fs::read_to_string(&cli.manifest).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: failed to read manifest {:?}: {e}", cli.manifest);
            return ExitCode::FAILURE;
        }
    };
    let manifest = match manifest::PodManifest::from_toml(&toml_str) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("error: invalid manifest: {e}");
            return ExitCode::FAILURE;
        }
    };

    let pod_name = manifest.pod.name.clone();

    // Initialize persistent store
    let store_dir = cli.store.unwrap_or_else(|| {
        default_store_dir().unwrap_or_else(|_| PathBuf::from(".insomnia/sessions"))
    });
    let store = match FsStore::new(&store_dir).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: failed to initialize store at {store_dir:?}: {e}");
            return ExitCode::FAILURE;
        }
    };

    // Build the Pod (pwd/scope derived from manifest + manifest_dir).
    let manifest_dir = std::fs::canonicalize(&cli.manifest)
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf));
    let pod = match Pod::from_manifest(manifest, store, manifest_dir).await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error: failed to create pod: {e}");
            return ExitCode::FAILURE;
        }
    };

    // Spawn the controller (starts socket server)
    let runtime_base = match default_runtime_dir() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };
    let handle = match PodController::spawn(pod, &runtime_base).await {
        Ok(h) => h,
        Err(e) => {
            eprintln!("error: failed to start pod controller: {e}");
            return ExitCode::FAILURE;
        }
    };

    eprintln!(
        "pod: {pod_name} listening on {:?}",
        handle.runtime_dir.socket_path()
    );

    // Wait for shutdown signal
    match tokio::signal::ctrl_c().await {
        Ok(()) => {
            eprintln!("pod: {pod_name} shutting down");
        }
        Err(e) => {
            eprintln!("error: failed to listen for signal: {e}");
        }
    }

    // TODO: handle.shutdown().await — PodController に採用しないスフルシャットダウン機構を追加したら組み込む
    drop(handle);

    ExitCode::SUCCESS
}
