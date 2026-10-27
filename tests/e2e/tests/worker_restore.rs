//! Fixed, externally supplied Worker restore. Never generates a fixture or sends model input.
#[cfg(feature = "restore-checkpoints")]
#[path = "worker_restore/checkpoint.rs"]
mod checkpoint;
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
        stop_and_cleanup(installed, vec![subscription], &fixture)
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
        stop_and_cleanup(first, vec![first_subscription], &fixture)?;
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
        stop_and_cleanup(second, vec![second_subscription, reconnected], &fixture)?;
        Ok(())
    })();
    fixture.source.assert_unchanged()?;
    result
}

#[test]
fn lost_http_response_recovers_exact_request_and_rejects_fresh_stale_intent() -> Result<()> {
    response_loss_recovery(false)
}

#[test]
fn committed_response_loss_survives_owned_runtime_process_kill_and_restart() -> Result<()> {
    response_loss_recovery(true)
}

fn response_loss_recovery(crash_runtime: bool) -> Result<()> {
    let fixture = Fixture::load()?;
    let result = (|| {
        let mut installed = seed::start(&fixture)?;
        let stopped = assert_single_state(&installed.environment, &fixture, "stopped")?;
        let token = stopped
            .restore_observation_token
            .as_deref()
            .filter(|token| !token.is_empty())
            .ok_or_else(|| failure("missing stopped observation token"))?;
        let request_id = "fixed-fixture-response-loss";
        phase(
            &installed.environment,
            "drop-http-response-after-complete-upstream",
        )?;
        let observed = installed.environment.restore_worker_losing_response(
            &fixture.manifest.worker_id,
            request_id,
            token,
        )?;
        let receipt: WorkerRestoreResponse = serde_json::from_value(observed.clone())?;
        if receipt.result.state != WorkerRestoreState::Accepted {
            return Err(failure(
                "upstream did not reach Accepted before response loss",
            ));
        }
        fs::write(
            installed
                .environment
                .artifacts_dir
                .join("response-loss.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "caller_received_response": false,
                "relay_upstream_state": receipt.result.state,
                "request_id": request_id,
                "crash_after_committed_terminal_receipt": crash_runtime,
            }))?,
        )?;
        let owner_before = restore_owner_evidence(&installed.environment, &fixture, request_id)?;
        if crash_runtime {
            phase(
                &installed.environment,
                "kill-owned-runtime-after-committed-receipt",
            )?;
            installed.environment.crash_and_restart_runtime()?;
        }
        phase(
            &installed.environment,
            "same-request-original-token-recovery",
        )?;
        let recovered =
            installed
                .environment
                .restore_worker(&fixture.manifest.worker_id, request_id, token)?;
        if crash_runtime {
            // The public response projects the current observation fence. Runtime
            // startup invalidates prior active observations; this is not a change
            // to the original request token or its saved outcome.
            let current = installed
                .environment
                .worker_detail(&fixture.manifest.worker_id)?;
            assert_eq!(
                recovered.pointer("/result/worker/restore_observation_token"),
                current.get("restore_observation_token")
            );
            let mut expected = observed.clone();
            expected["result"]["worker"]["restore_observation_token"] =
                recovered["result"]["worker"]["restore_observation_token"].clone();
            assert_eq!(
                recovered, expected,
                "restart may change only the public observation fence, not the saved outcome"
            );
        } else {
            assert_eq!(
                recovered, observed,
                "exact request must recover the saved receipt"
            );
        }
        assert_single_state(&installed.environment, &fixture, "idle")?;
        let mut subscription = WorkerSessionSubscription::connect(
            &installed.environment,
            &fixture.manifest.worker_id,
            &installed.snapshot,
        )?;
        subscription.assert_alive()?;
        let stale = installed.environment.restore_worker(
            &fixture.manifest.worker_id,
            "fixed-fixture-fresh-stale-intent",
            token,
        );
        match stale {
            Err(error) if error.to_string().contains("HTTP 409") => {}
            _ => {
                return Err(failure(
                    "fresh intent with old observation was not HTTP 409",
                ));
            }
        }
        let owner_after = restore_owner_evidence(&installed.environment, &fixture, request_id)?;
        assert_eq!(
            owner_after, owner_before,
            "saved owner/operation/receipt must not change"
        );
        fs::write(
            installed
                .environment
                .artifacts_dir
                .join("restore-owner-evidence.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "before": owner_before, "after": owner_after,
                "request_token_replaced": false,
            }))?,
        )?;
        installed.environment.assert_connected()?;
        subscription.assert_alive()?;
        stop_and_cleanup(installed, vec![subscription], &fixture)
    })();
    fixture.source.assert_unchanged()?;
    result
}

// Read-only diagnostic of the product-written aggregate; never seeds or edits it.
// Persist only a digest, operation ID and owner count, not prompts or credentials.
fn restore_owner_evidence(
    environment: &WorkerE2eEnvironment,
    fixture: &Fixture,
    request_id: &str,
) -> Result<serde_json::Value> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let path = environment
        .runtime_data_dir
        .join("runtime/workers")
        .join(&fixture.manifest.worker_id)
        .join("worker.json");
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(8 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 8 * 1024 * 1024 {
        return Err(failure("Worker aggregate byte budget exceeded"));
    }
    let aggregate: serde_json::Value = serde_json::from_slice(&bytes)?;
    let owners = aggregate
        .pointer("/restore_guard/owners")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| failure("canonical Worker omitted restore owners"))?;
    let owner = owners
        .get(request_id)
        .ok_or_else(|| failure("canonical owner missing"))?;
    if owner
        .pointer("/receipt/state")
        .and_then(serde_json::Value::as_str)
        != Some("accepted")
    {
        return Err(failure("canonical owner has no Accepted receipt"));
    }
    let operation = owner
        .get("operation_id")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| failure("canonical owner omitted operation ID"))?;
    let digest: String = Sha256::digest(serde_json::to_vec(owner)?)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    Ok(
        serde_json::json!({"operation_id": operation, "owner_sha256": digest, "owner_count": owners.len()}),
    )
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
    subscriptions: Vec<WorkerSessionSubscription>,
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
    installed.environment.assert_connected()?;
    for subscription in subscriptions {
        subscription.expect_stopped()?;
        // A transport close caused by Server/Runtime shutdown is not stop evidence.
        installed.environment.assert_connected()?;
    }
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
