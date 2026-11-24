//! CLI integration: the product executable loads host configuration before opening storage.
//! Each command owns a temporary HOME/config/data tree; no running service or real DB is used.
//! The normal Cargo test target covers template creation and fail-closed startup, not serving.

use std::path::Path;
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

async fn run(root: &Path, args: &[&str]) -> (ExitStatus, String) {
    let stdout = std::fs::File::create(root.join("stdout")).unwrap();
    let stderr = std::fs::File::create(root.join("stderr")).unwrap();
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_yoi-server"))
        .args(args)
        .env_clear()
        .env("HOME", root)
        .env("YOI_CONFIG_DIR", root.join("config"))
        .env("YOI_DATA_DIR", root.join("data"))
        .env("YOI_RUNTIME_DIR", root.join("runtime"))
        .env("YOI_CACHE_DIR", root.join("cache"))
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr)
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let status = match tokio::time::timeout(Duration::from_secs(15), child.wait()).await {
        Ok(result) => result.unwrap(),
        Err(_) => {
            child.kill().await.unwrap();
            child.wait().await.unwrap();
            panic!("Server command did not terminate: {args:?}");
        }
    };
    (
        status,
        std::fs::read_to_string(root.join("stderr")).unwrap(),
    )
}

#[tokio::test]
async fn missing_default_or_explicit_config_stops_before_creating_storage() {
    for explicit in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let args = if explicit {
            vec!["serve", "--config", "missing.toml"]
        } else {
            vec!["serve"]
        };
        let (status, error) = run(dir.path(), &args).await;
        assert!(!status.success());
        assert!(
            error.contains("init-config") && error.contains("browser.public_urls"),
            "{error}"
        );
        assert!(
            error.contains(if explicit {
                "missing.toml"
            } else {
                "server.toml"
            }),
            "{error}"
        );
        assert!(!dir.path().join("data").exists());
        assert!(!dir.path().join("config").exists());
    }
}

#[tokio::test]
async fn generated_template_is_inactive_and_reinitialization_never_overwrites() {
    let dir = tempfile::tempdir().unwrap();
    let (status, error) = run(dir.path(), &["init-config"]).await;
    assert!(status.success(), "{error}");
    let path = dir.path().join("config/server.toml");
    let original = std::fs::read_to_string(&path).unwrap();
    assert!(original.contains("# public_urls = [\"https://example.com\""));
    assert!(!original.contains("5173"));
    let (status, error) = run(dir.path(), &["serve"]).await;
    assert!(!status.success());
    assert!(
        error.contains("public_urls") && error.contains(path.to_str().unwrap()),
        "{error}"
    );
    assert!(!dir.path().join("data").exists());
    let (status, error) = run(dir.path(), &["init-config"]).await;
    assert!(!status.success());
    assert!(error.contains("never overwritten"), "{error}");
    assert_eq!(std::fs::read_to_string(path).unwrap(), original);
}
