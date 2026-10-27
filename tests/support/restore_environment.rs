//! Fail-closed preflight shared by restore fixtures across Worker, Runtime and Server.
//!
//! The normal runner supplies a separate namespace per process; a manually
//! configured invocation may use the checkout-owned base itself. This module
//! never sets process-wide environment or creates directories/tables.

use std::fs;
use std::path::{Component, Path, PathBuf};

fn validate_restore_test_paths(
    base: &Path,
    resolved: [Option<PathBuf>; 5],
) -> Result<PathBuf, &'static str> {
    let root = resolved[0]
        .as_deref()
        .filter(|config| config.file_name().is_some_and(|name| name == "config"))
        .and_then(Path::parent)
        .ok_or("config")?;
    let namespace = root.strip_prefix(base).map_err(|_| "config")?;
    if namespace
        .components()
        .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err("config");
    }
    for ((label, suffix), actual) in [
        ("config", "config"),
        ("data", "data"),
        ("secrets", "data"),
        ("runtime", "run"),
        ("allocation", "run/workers.json"),
    ]
    .into_iter()
    .zip(resolved.iter())
    {
        if actual.as_deref() != Some(root.join(suffix).as_path()) {
            return Err(label);
        }
    }
    Ok(root.to_path_buf())
}

fn require_owned_directory(checkout: &Path, directory: &Path) {
    let mut checked = checkout.to_path_buf();
    for part in directory.strip_prefix(checkout).unwrap().components() {
        assert!(matches!(part, Component::Normal(_)));
        checked.push(part);
        let metadata = fs::symlink_metadata(&checked)
            .expect("create isolated restore test directories before launching the test process");
        assert!(
            metadata.is_dir() && !metadata.file_type().is_symlink(),
            "restore test directory must not traverse symlinks"
        );
    }
}

pub(crate) fn require_restore_test_environment() {
    let checkout = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let base = checkout.join("target/restore-test-environment");
    // Compare all resolver results before inspecting even an absent allocation
    // table (its lookup can create a lock). Never inspect a live fallback path.
    let root = validate_restore_test_paths(
        &base,
        [
            manifest::paths::config_dir(),
            manifest::paths::data_dir(),
            manifest::paths::secret_data_dir(),
            manifest::paths::runtime_dir(),
            manifest::paths::worker_allocation_path(),
        ],
    )
    .unwrap_or_else(|label| {
        panic!(
            "restore fixture requires isolated {label}: run deno task test, or configure YOI_CONFIG_DIR=<checkout>/target/restore-test-environment/config, YOI_DATA_DIR=<checkout>/target/restore-test-environment/data, YOI_RUNTIME_DIR=<checkout>/target/restore-test-environment/run before launch"
        )
    });
    assert_eq!(fs::canonicalize(checkout).unwrap(), checkout);
    for directory in [
        root.clone(),
        root.join("config"),
        root.join("data"),
        root.join("run"),
    ] {
        require_owned_directory(checkout, &directory);
    }
    for name in ["workers.json", "workers.json.lock"] {
        match fs::symlink_metadata(root.join("run").join(name)) {
            Ok(metadata) => assert!(
                metadata.is_file() && !metadata.file_type().is_symlink(),
                "owned allocation paths must be regular files, not symlinks"
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => panic!("inspect owned restore allocation path: {error}"),
        }
    }
}

#[test]
fn restore_fixture_path_validation_rejects_fallback_before_resource_access() {
    let base = Path::new("/owned/test-environment");
    for root in [base.to_path_buf(), base.join("run-1/worker-shard-2")] {
        let safe = ["config", "data", "data", "run", "run/workers.json"]
            .map(|suffix| Some(root.join(suffix)));
        assert_eq!(
            validate_restore_test_paths(base, safe.clone()),
            Ok(root.clone())
        );
        for index in 0..safe.len() {
            for fallback in [None, Some(PathBuf::from("/not-a-fixture"))] {
                let mut unsafe_paths = safe.clone();
                unsafe_paths[index] = fallback;
                assert!(validate_restore_test_paths(base, unsafe_paths).is_err());
            }
        }
    }
    for root in [
        base.join("../escape"),
        base.join("safe/../../escape"),
        PathBuf::from("/outside"),
    ] {
        let paths = ["config", "data", "data", "run", "run/workers.json"]
            .map(|suffix| Some(root.join(suffix)));
        assert!(validate_restore_test_paths(base, paths).is_err());
    }
}
