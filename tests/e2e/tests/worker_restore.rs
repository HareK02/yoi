//! Fixed, externally supplied Worker restore. Never generates a fixture or sends model input.
#[path = "worker_restore/fixture.rs"]
mod fixture;
#[path = "worker_restore/seed.rs"]
mod seed;

use std::fs;
use std::time::{Duration, Instant};

use fixture::{Fixture, failure};
use seed::Installed;
use server_api::{
    RuntimeWorkerLifecycleResult, RuntimeWorkerShowResponse, WorkerListResponse,
    WorkerOperationState, WorkerRestoreResponse, WorkerRestoreState, WorkerSummary,
};
use yoi_e2e::{FixtureSource, Result, WorkerE2eEnvironment, WorkerSessionSubscription};

#[test]
fn supplied_stopped_worker_restores_exact_live_session_and_cleans_up() -> Result<()> {
    let fixture = Fixture::load()?;
    let result = (|| {
        let installed = seed::start(&fixture)?;
        let subscription = restore(&installed, &fixture, "fixed-fixture-restore-single")?;
        stop_and_cleanup(installed, subscription, &fixture)
    })();
    fixture.source.assert_unchanged()?;
    result
}

#[test]
fn concurrent_fixed_id_restores_keep_sessions_storage_and_cleanup_isolated() -> Result<()> {
    let fixture = Fixture::load()?;
    let result = (|| {
        let (first, second) = std::thread::scope(|scope| {
            let first = scope.spawn(|| seed::start(&fixture));
            let second = seed::start(&fixture);
            (
                first
                    .join()
                    .map_err(|_| failure("first environment startup panicked"))
                    .and_then(|result| result),
                second,
            )
        });
        let first = first?;
        let second = second?;
        assert_eq!(
            first.environment.workspace_id,
            second.environment.workspace_id
        );
        assert_eq!(first.environment.runtime_id, second.environment.runtime_id);
        assert_ne!(first.environment.root, second.environment.root);
        assert_ne!(
            first.environment.server_data_dir,
            second.environment.server_data_dir
        );
        assert_ne!(
            first.environment.runtime_data_dir,
            second.environment.runtime_data_dir
        );
        assert_ne!(
            first.environment.artifacts_dir,
            second.environment.artifacts_dir
        );
        assert_ne!(
            first.environment.account_handle,
            second.environment.account_handle
        );
        assert_ne!(first.runtime_public_key, second.runtime_public_key);
        assert_ne!(first.workspace_public_key, second.workspace_public_key);
        assert_ne!(
            first.environment.server_port()?,
            second.environment.server_port()?
        );
        assert_ne!(
            first.environment.runtime_port()?,
            second.environment.runtime_port()?
        );
        assert_ne!(
            first.environment.server_pid(),
            second.environment.server_pid()
        );
        assert_ne!(
            first.environment.runtime_pid(),
            second.environment.runtime_pid()
        );
        first
            .environment
            .assert_authentication_isolated_from(&second.environment)?;
        second
            .environment
            .assert_authentication_isolated_from(&first.environment)?;
        let (first_subscription, second_subscription) = std::thread::scope(|scope| {
            // Deliberately reuse request identity in isolated stores, just like fixed IDs.
            let a = scope.spawn(|| restore(&first, &fixture, "fixed-fixture-restore-concurrent"));
            let b = restore(&second, &fixture, "fixed-fixture-restore-concurrent");
            (
                a.join()
                    .map_err(|_| failure("first restore panicked"))
                    .and_then(|result| result),
                b,
            )
        });
        let first_subscription = first_subscription?;
        let mut second_subscription = second_subscription?;
        let second_session_root = second
            .environment
            .runtime_data_dir
            .join("runtime/workers")
            .join(&fixture.manifest.worker_id)
            .join("session");
        let second_session = FixtureSource::capture(&second_session_root)?;
        let first_root = first.environment.root.clone();
        let first_artifacts = first.environment.artifacts_dir.clone();
        stop_and_cleanup(first, first_subscription, &fixture)?;
        assert!(!first_root.exists());
        assert!(first_artifacts.join("cleanup.json").is_file());
        assert!(second.environment.root.is_dir());
        assert!(second.environment.server_data_dir.is_dir());
        assert!(second.environment.runtime_data_dir.is_dir());
        assert!(
            second
                .environment
                .artifacts_dir
                .join("restored-session.json")
                .is_file()
        );
        second.environment.assert_connected()?;
        assert_single_state(&second.environment, &fixture, "idle")?;
        second_subscription.assert_alive()?;
        // A second live connection independently verifies content after peer cleanup.
        let mut reconnected = WorkerSessionSubscription::connect(
            &second.environment,
            &fixture.manifest.worker_id,
            &second.snapshot,
        )?;
        reconnected.assert_alive()?;
        second_session.assert_unchanged()?;
        stop_and_cleanup(second, second_subscription, &fixture)?;
        reconnected.expect_stopped()?;
        Ok(())
    })();
    fixture.source.assert_unchanged()?;
    result
}

fn restore(
    installed: &Installed,
    fixture: &Fixture,
    request_id: &str,
) -> Result<WorkerSessionSubscription> {
    phase(&installed.environment, "observe-stopped")?;
    let stopped = assert_single_state(&installed.environment, fixture, "stopped")?;
    let token = stopped
        .restore_observation_token
        .as_deref()
        .filter(|token| !token.is_empty())
        .ok_or_else(|| failure("stopped Worker did not expose a restore observation token"))?;
    phase(&installed.environment, "restore")?;
    let result: WorkerRestoreResponse = serde_json::from_value(
        installed
            .environment
            .restore_worker(&fixture.manifest.worker_id, request_id, token)?,
    )?;
    if result.workspace_id != fixture.manifest.workspace_id
        || result.runtime_id != fixture.manifest.runtime_id
        || result.worker_id != fixture.manifest.worker_id
        || result.result.state != WorkerRestoreState::Accepted
    {
        return Err(failure(format!(
            "restore did not commit accepted: {:?}",
            result.result.state
        )));
    }
    assert_single_state(&installed.environment, fixture, "idle")?;
    let detail: RuntimeWorkerShowResponse = serde_json::from_value(
        installed
            .environment
            .worker_detail(&fixture.manifest.worker_id)?,
    )?;
    if detail.worker.worker_id != fixture.manifest.worker_id
        || detail.worker.runtime_id != fixture.manifest.runtime_id
        || detail.worker.state != "idle"
        || detail
            .worker
            .worker_state
            .as_ref()
            .is_none_or(|state| state.state != protocol::WorkerState::Idle)
    {
        return Err(failure(
            "restored Worker detail is not the single live idle execution",
        ));
    }
    phase(&installed.environment, "subscribe-live-session")?;
    let mut subscription = WorkerSessionSubscription::connect(
        &installed.environment,
        &fixture.manifest.worker_id,
        &installed.snapshot,
    )?;
    subscription.assert_alive()?;
    phase(&installed.environment, "live-session-verified")?;
    Ok(subscription)
}

fn assert_single_state(
    environment: &WorkerE2eEnvironment,
    fixture: &Fixture,
    state: &str,
) -> Result<WorkerSummary> {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let list: WorkerListResponse = serde_json::from_value(environment.list_workers()?)?;
        if list.workspace_id != fixture.manifest.workspace_id || list.items.len() > 1 {
            return Err(failure(
                "Workspace Worker list has wrong identity or duplicate Workers",
            ));
        }
        if let Some(worker) = list.items.into_iter().next() {
            if worker.worker_id != fixture.manifest.worker_id
                || worker.runtime_id != fixture.manifest.runtime_id
            {
                return Err(failure("Workspace listed an unexpected Worker identity"));
            }
            if worker.state == state {
                return Ok(worker);
            }
        }
        if Instant::now() >= deadline {
            return Err(failure(format!(
                "Worker list did not become exactly one {state} Worker"
            )));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn stop_and_cleanup(
    installed: Installed,
    subscription: WorkerSessionSubscription,
    fixture: &Fixture,
) -> Result<()> {
    phase(&installed.environment, "stop")?;
    let result: RuntimeWorkerLifecycleResult = serde_json::from_value(
        installed
            .environment
            .stop_worker(&fixture.manifest.worker_id)?,
    )?;
    if result.state != WorkerOperationState::Accepted {
        return Err(failure("Worker stop was not accepted"));
    }
    subscription.expect_stopped()?;
    assert_single_state(&installed.environment, fixture, "stopped")?;
    phase(&installed.environment, "shutdown-cleanup")?;
    let report = installed.environment.cleanup()?;
    if !report.cleanup_success || report.fixture_root_exists_after {
        return Err(failure("environment cleanup failed"));
    }
    if !report.report_path.is_file() {
        return Err(failure("cleanup evidence is missing"));
    }
    Ok(())
}

fn phase(environment: &WorkerE2eEnvironment, phase: &str) -> Result<()> {
    fs::write(
        environment.artifacts_dir.join("restore-phase.json"),
        serde_json::to_vec_pretty(&serde_json::json!({"phase":phase}))?,
    )?;
    Ok(())
}
