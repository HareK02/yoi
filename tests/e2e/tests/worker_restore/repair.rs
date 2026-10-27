//! Real orphan-pin repair on an installed fixed-asset copy, not a synthetic
//! pending head. Only the postcommit activation path can reconcile these pins.
use super::*;
use session_store::{Store, UploadedFileLimits, WorkerSessionStore};

const PIN_OWNER: &str = "interrupted-pin-before-queue-checkpoint";
const UPLOAD: &[u8] = b"isolated orphan upload; never model input";

#[test]
fn orphan_pin_not_committed_process_death_preserves_unrepaired_source() -> Result<()> {
    repair_crash(RestoreCheckpointStage::PreparedNotCommitted)
}

#[test]
fn orphan_pin_activated_process_death_preserves_real_repair_and_committed_decision() -> Result<()> {
    repair_crash(RestoreCheckpointStage::ActivatedPreterminal)
}

fn repair_crash(stage: RestoreCheckpointStage) -> Result<()> {
    let fixture = Fixture::load()?;
    let result = (|| {
        let mut environment = WorkerE2eEnvironment::new()?;
        let mut peer = Peer::configure(&mut environment, &fixture, stage)?;
        let session_id = fixture::support(fixture.manifest.session_id.parse())?;
        let mut upload = None;
        let mut installed = seed::start_in_with_source_setup(&fixture, environment, |aggregate| {
            let store = fixture::support(WorkerSessionStore::new(aggregate.join("session")))?;
            let reference = fixture::support(store.write_uploaded_file(
                session_id,
                "orphan.txt",
                "text/plain",
                UPLOAD,
                UploadedFileLimits::default(),
            ))?;
            fixture::support(store.pin_uploaded_file(session_id, &reference, PIN_OWNER))?;
            upload = Some(reference);
            Ok(())
        })?;
        let upload = upload.ok_or_else(|| failure("offline upload setup not called"))?;
        let relative = format!("session/artifacts/paste/{}.file.json", upload.artifact_id);
        let pin_path = aggregate_path(&installed.environment, &fixture).join(&relative);
        let pin_before: serde_json::Value = serde_json::from_slice(&fs::read(&pin_path)?)?;
        assert_eq!(pin_before["pending_owner_id"], PIN_OWNER);
        let before = saved_source_hashes(&installed.environment, &fixture)?;
        let initial = canonical(&installed.environment, &fixture)?;
        let token = assert_single_state(&installed.environment, &fixture, "stopped")?
            .restore_observation_token
            .ok_or_else(|| failure("missing original token"))?;
        let request_id = "real-orphan-repair-crash-request";
        let committed = stage == RestoreCheckpointStage::ActivatedPreterminal;
        let call = installed.environment.pending_restore_call(
            &fixture.manifest.worker_id,
            request_id,
            &token,
        )?;
        let (ack, at_death) = std::thread::scope(|scope| -> Result<_> {
            let request = scope.spawn(call);
            let ack = peer.acknowledge(&installed.environment)?;
            installed.environment.crash_runtime()?;
            let saved = canonical(&installed.environment, &fixture)?;
            let op = &saved["execution_state"]["execution"]["intent"];
            let owner = &saved["restore_guard"]["owners"][request_id];
            assert_eq!(saved["restore_guard"]["active_request_id"], request_id);
            assert_eq!(op["operation_id"], serde_json::to_value(ack.operation_id)?);
            assert_eq!(owner["operation_id"], op["operation_id"]);
            assert_eq!(owner["request"]["expected_observation_token"], token);
            assert!(owner["receipt"].is_null());
            assert_eq!(
                op["commit"],
                if committed {
                    "committed"
                } else {
                    "not_committed"
                }
            );
            assert_eq!(
                ack.commit,
                if committed {
                    RestoreCheckpointCommit::Committed
                } else {
                    RestoreCheckpointCommit::NotCommitted
                }
            );
            // The durable Committed decision authorizes activation effects. The
            // precommit repair callback is not used for a materialized head.
            assert_eq!(op["source_repair_possible"], false);
            let at_death = saved_source_hashes(&installed.environment, &fixture)?;
            let pin_after: serde_json::Value = serde_json::from_slice(&fs::read(&pin_path)?)?;
            let mut expected_pin = pin_before.clone();
            if committed {
                expected_pin
                    .as_object_mut()
                    .unwrap()
                    .remove("pending_owner_id");
            }
            assert_eq!(
                pin_after, expected_pin,
                "real repair changes only orphan ownership"
            );
            if committed {
                assert_ne!(
                    at_death[&relative], before[&relative],
                    "actual pin bytes must change"
                );
            } else {
                assert_eq!(
                    at_death, before,
                    "preparation cannot repair pin or metadata"
                );
            }
            // In both windows the original conversation is immutable.
            for (path, hash) in &before {
                if path == "session/session.json" || path.starts_with("session/segments/") {
                    assert_eq!(at_death.get(path), Some(hash), "{path}");
                }
            }
            let store = fixture::support(WorkerSessionStore::open_read_only(
                aggregate_path(&installed.environment, &fixture).join("session"),
            ))?;
            assert_eq!(
                fixture::support(store.read_uploaded_file(session_id, &upload))?,
                UPLOAD
            );
            assert_eq!(
                restore_runs(&installed.environment, &fixture)?,
                vec![format!("restore-{}", ack.operation_id)]
            );
            fs::write(
                installed
                    .environment
                    .artifacts_dir
                    .join("repair-at-death.json"),
                serde_json::to_vec_pretty(&serde_json::json!({
                    "ack":ack, "initial_execution":initial["execution_state"],
                    "canonical_commit":op["commit"], "source_repair_possible":op["source_repair_possible"],
                    "operation_id":op["operation_id"], "owner_operation_id":owner["operation_id"],
                    "receipt_absent":owner["receipt"].is_null(), "original_request_and_token":true,
                    "pin_pending_owner_before":pin_before["pending_owner_id"],
                    "pin_pending_owner_after":pin_after["pending_owner_id"],
                    "hashes_before":before, "hashes_at_death":at_death,
                    "restore_runs":restore_runs(&installed.environment, &fixture)?,
                }))?,
            )?;
            let outcome = request
                .join()
                .map_err(|_| failure("restore caller panicked"))?;
            if let Ok(value) = outcome {
                let receipt: WorkerRestoreResponse = serde_json::from_value(value)?;
                assert_ne!(receipt.result.state, WorkerRestoreState::Accepted);
            }
            installed.environment.restart_runtime_after_crash()?;
            assert_ne!(installed.environment.runtime_pid(), Some(ack.pid));
            Ok((ack, at_death))
        })?;
        drop(peer);
        let recovered = installed.environment.restore_worker(
            &fixture.manifest.worker_id,
            request_id,
            &token,
        )?;
        let receipt: WorkerRestoreResponse = serde_json::from_value(recovered.clone())?;
        fs::write(
            installed
                .environment
                .artifacts_dir
                .join("repair-first-recovery.json"),
            serde_json::to_vec_pretty(&recovered)?,
        )?;
        assert_eq!(
            receipt.result.state,
            if committed {
                WorkerRestoreState::Accepted
            } else {
                WorkerRestoreState::RolledBack
            }
        );
        assert_eq!(
            installed.environment.restore_worker(
                &fixture.manifest.worker_id,
                request_id,
                &token
            )?,
            recovered
        );
        let final_saved = canonical(&installed.environment, &fixture)?;
        let owners = final_saved["restore_guard"]["owners"]
            .as_object()
            .ok_or_else(|| failure("missing owners"))?;
        assert_eq!(owners.len(), 1);
        let owner = &owners[request_id];
        assert_eq!(
            owner["operation_id"],
            serde_json::to_value(ack.operation_id)?
        );
        assert_eq!(owner["request"]["expected_observation_token"], token);
        assert_eq!(
            owner["receipt"]["state"],
            if committed { "accepted" } else { "rolled_back" }
        );
        let after = saved_source_hashes(&installed.environment, &fixture)?;
        assert_eq!(
            after, at_death,
            "restart/replay cannot restore the orphan pin or change saved source"
        );
        let stale = installed.environment.restore_worker(
            &fixture.manifest.worker_id,
            "fresh-stale-repair-intent",
            &token,
        );
        assert!(matches!(stale, Err(error) if error.to_string().contains("HTTP 409")));
        fs::write(
            installed
                .environment
                .artifacts_dir
                .join("repair-recovery.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "ack":ack, "state":receipt.result.state, "diagnostics":receipt.result.diagnostics,
                "operation_id":owner["operation_id"], "owner_count":owners.len(),
                "hashes_after_recovery":after, "restore_runs":restore_runs(&installed.environment, &fixture)?,
                "same_original_request_and_token":true, "fresh_stale_rejected":true,
            }))?,
        )?;
        if committed {
            assert_single_state(&installed.environment, &fixture, "idle")?;
            assert_eq!(
                restore_runs(&installed.environment, &fixture)?,
                vec![format!("restore-{}", ack.operation_id)]
            );
            let mut subscription = WorkerSessionSubscription::connect(
                &installed.environment,
                &fixture.manifest.worker_id,
                &installed.snapshot,
            )?;
            subscription.assert_alive()?;
            stop_and_cleanup(installed, vec![subscription], &fixture)
        } else {
            assert_single_state(&installed.environment, &fixture, "stopped")?;
            assert!(restore_runs(&installed.environment, &fixture)?.is_empty());
            let report = installed.environment.cleanup()?;
            assert!(report.cleanup_success && !report.fixture_root_exists_after);
            Ok(())
        }
    })();
    fixture.source.assert_unchanged()?;
    result
}
