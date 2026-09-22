use std::fs;

use yoi_e2e::{Result, WorkerE2eEnvironment, WorkerEnvironmentCleanupReport};

#[test]
fn starts_server_and_runtime_from_empty_storage() -> Result<()> {
    let mut environment = WorkerE2eEnvironment::new()?;
    assert!(!environment.server_data_dir.exists());
    assert!(!environment.runtime_data_dir.exists());

    environment.start()?;

    assert!(environment.server_data_dir.join("server.db").is_file());
    assert!(
        environment
            .runtime_data_dir
            .join("runtime/runtime.json")
            .is_file()
    );
    environment.assert_connected_and_empty()?;
    let artifacts_dir = environment.artifacts_dir.clone();
    let report = environment.cleanup()?;
    assert!(report.cleanup_success);
    let persisted: WorkerEnvironmentCleanupReport =
        serde_json::from_slice(&fs::read(artifacts_dir.join("cleanup.json"))?)?;
    assert!(persisted.cleanup_success);
    assert!(!persisted.fixture_root_exists_after);
    Ok(())
}

#[test]
fn concurrent_environments_are_isolated_and_cleanup_is_local() -> Result<()> {
    let mut first = WorkerE2eEnvironment::new()?;
    let mut second = WorkerE2eEnvironment::new()?;
    assert_ne!(first.root, second.root);
    assert_ne!(first.server_data_dir, second.server_data_dir);
    assert_ne!(first.runtime_data_dir, second.runtime_data_dir);
    assert_ne!(first.runtime_id, second.runtime_id);
    assert_ne!(first.account_handle, second.account_handle);
    assert_ne!(first.artifacts_dir, second.artifacts_dir);

    first.start()?;
    second.start()?;

    assert_ne!(first.server_port()?, second.server_port()?);
    assert_ne!(first.runtime_port()?, second.runtime_port()?);
    assert_ne!(first.server_pid(), second.server_pid());
    assert_ne!(first.runtime_pid(), second.runtime_pid());
    assert_ne!(first.workspace_id, second.workspace_id);
    first.assert_connected_and_empty()?;
    second.assert_connected_and_empty()?;

    let second_root = second.root.clone();
    let second_server_data = second.server_data_dir.clone();
    let second_runtime_data = second.runtime_data_dir.clone();
    let second_artifacts = second.artifacts_dir.clone();
    let first_artifacts = first.artifacts_dir.clone();
    let first_report = first.cleanup()?;
    assert!(first_report.cleanup_success);
    assert!(first_artifacts.is_dir());

    assert!(second_root.is_dir());
    assert!(second_server_data.is_dir());
    assert!(second_runtime_data.is_dir());
    assert!(second_artifacts.is_dir());
    second.assert_connected_and_empty()?;

    let second_report = second.cleanup()?;
    assert!(second_report.cleanup_success);
    assert!(second_artifacts.join("cleanup.json").is_file());
    Ok(())
}
