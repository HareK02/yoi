//! Opt-in real-process checkpoint cases; never substitute the featureless lane.
#[path = "repair.rs"]
mod repair;
use super::*;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use worker_runtime::restore_checkpoints::{
    MAX_RESTORE_CHECKPOINT_ACK_BYTES, RestoreCheckpointAck, RestoreCheckpointCommit,
    RestoreCheckpointConfig, RestoreCheckpointStage,
};

struct Peer {
    listener: TcpListener,
    config: RestoreCheckpointConfig,
    connection: Option<TcpStream>,
}

impl Peer {
    fn configure(
        environment: &mut WorkerE2eEnvironment,
        fixture: &Fixture,
        stage: RestoreCheckpointStage,
    ) -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let config = RestoreCheckpointConfig {
            endpoint: listener.local_addr()?,
            worker_id: fixture::support(fixture.manifest.worker_id.parse())?,
            stage,
            nonce: format!(
                "owned-checkpoint-{}-{}",
                std::process::id(),
                listener.local_addr()?.port()
            ),
        };
        let path = environment.root.join("restore-checkpoint.json");
        fs::write(&path, serde_json::to_vec(&config)?)?;
        let provider = yoi_e2e::worker_binary_provider_info()?;
        let binary = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/e2e-checkpoint-products")
            .join(provider.profile)
            .join("yoi-runtime");
        environment.use_checkpoint_runtime(binary, &path)?;
        Ok(Self {
            listener,
            config,
            connection: None,
        })
    }

    fn acknowledge(&mut self, environment: &WorkerE2eEnvironment) -> Result<RestoreCheckpointAck> {
        let deadline = Instant::now() + Duration::from_secs(15);
        let (mut stream, address) = loop {
            match self.listener.accept() {
                Ok(value) => break value,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        return Err(failure("checkpoint ack deadline exceeded"));
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => return Err(error.into()),
            }
        };
        if !address.ip().is_loopback() {
            return Err(failure("non-loopback checkpoint peer"));
        }
        let mut bytes = Vec::new();
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(failure("checkpoint ack read deadline exceeded"));
            }
            stream.set_read_timeout(Some(remaining))?;
            let mut byte = [0];
            stream.read_exact(&mut byte)?;
            if byte == [b'\n'] {
                break;
            }
            bytes.push(byte[0]);
            if bytes.len() > MAX_RESTORE_CHECKPOINT_ACK_BYTES {
                return Err(failure("checkpoint ack oversized"));
            }
        }
        let ack: RestoreCheckpointAck = serde_json::from_slice(&bytes)?;
        if ack.nonce != self.config.nonce
            || ack.stage != self.config.stage
            || ack.worker_id != self.config.worker_id
            || ack.runtime_id != environment.runtime_id
            || Some(ack.pid) != environment.runtime_pid()
            || !ack.receipt_absent
        {
            return Err(failure(
                "checkpoint ack does not match owned process and stage",
            ));
        }
        fs::write(
            environment.artifacts_dir.join("checkpoint-ack.json"),
            serde_json::to_vec_pretty(&ack)?,
        )?;
        self.connection = Some(stream);
        Ok(ack)
    }

    fn release(&mut self) -> Result<()> {
        let mut stream = self
            .connection
            .take()
            .ok_or_else(|| failure("checkpoint not acknowledged"))?;
        stream.set_write_timeout(Some(Duration::from_secs(1)))?;
        stream.write_all(b"R")?;
        Ok(())
    }
}

fn aggregate_path(environment: &WorkerE2eEnvironment, fixture: &Fixture) -> PathBuf {
    environment
        .runtime_data_dir
        .join("runtime/workers")
        .join(&fixture.manifest.worker_id)
}

fn canonical(environment: &WorkerE2eEnvironment, fixture: &Fixture) -> Result<serde_json::Value> {
    let mut bytes = Vec::new();
    fs::File::open(aggregate_path(environment, fixture).join("worker.json"))?
        .take(8 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 8 * 1024 * 1024 {
        return Err(failure("canonical aggregate oversized"));
    }
    Ok(serde_json::from_slice(&bytes)?)
}

fn saved_source_hashes(
    environment: &WorkerE2eEnvironment,
    fixture: &Fixture,
) -> Result<BTreeMap<String, String>> {
    fn visit(root: &Path, path: &Path, files: &mut BTreeMap<String, String>) -> Result<()> {
        if files.len() > 128 {
            return Err(failure("saved source file budget exceeded"));
        }
        let metadata = fs::symlink_metadata(path)?;
        if metadata.file_type().is_symlink() {
            return Err(failure("unexpected saved source symlink"));
        }
        if metadata.is_dir() {
            for entry in fs::read_dir(path)? {
                visit(root, &entry?.path(), files)?;
            }
        } else if metadata.is_file() {
            if metadata.len() > 8 * 1024 * 1024 {
                return Err(failure("saved source byte budget exceeded"));
            }
            use sha2::{Digest, Sha256};
            let digest: String = Sha256::digest(fs::read(path)?)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            files.insert(
                path.strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
                digest,
            );
        } else {
            return Err(failure("unexpected saved source entry type"));
        }
        Ok(())
    }
    let root = aggregate_path(environment, fixture);
    let mut files = BTreeMap::new();
    for name in ["metadata.json", "session"] {
        visit(&root, &root.join(name), &mut files)?;
    }
    if files.len() < 2 {
        return Err(failure("saved source inventory is incomplete"));
    }
    Ok(files)
}

fn restore_runs(environment: &WorkerE2eEnvironment, fixture: &Fixture) -> Result<Vec<String>> {
    let runs = aggregate_path(environment, fixture).join("runs");
    if !runs.exists() {
        return Ok(Vec::new());
    }
    let mut names = Vec::new();
    for entry in fs::read_dir(runs)? {
        let entry = entry?;
        if entry.file_name().to_string_lossy().starts_with("restore-") {
            if !entry.file_type()?.is_dir() {
                return Err(failure("restore run is not a directory"));
            }
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    names.sort();
    Ok(names)
}

#[test]
fn acknowledged_not_committed_candidate_process_death_recovers_same_request_as_rollback()
-> Result<()> {
    crash_window(RestoreCheckpointStage::PreparedNotCommitted)
}

#[test]
fn acknowledged_committed_preterminal_process_death_reconstructs_same_decision() -> Result<()> {
    crash_window(RestoreCheckpointStage::CommittedPreterminal)
}

fn crash_window(stage: RestoreCheckpointStage) -> Result<()> {
    let fixture = Fixture::load()?;
    let result = (|| {
        let mut environment = WorkerE2eEnvironment::new()?;
        let mut peer = Peer::configure(&mut environment, &fixture, stage)?;
        let mut installed = seed::start_in(&fixture, environment)?;
        let stopped = assert_single_state(&installed.environment, &fixture, "stopped")?;
        let token = stopped
            .restore_observation_token
            .ok_or_else(|| failure("missing original observation"))?;
        let original_sources = saved_source_hashes(&installed.environment, &fixture)?;
        let request_id = "owned-process-checkpoint-request";
        let call = installed.environment.pending_restore_call(
            &fixture.manifest.worker_id,
            request_id,
            &token,
        )?;
        let ack = std::thread::scope(|scope| -> Result<RestoreCheckpointAck> {
            let request = scope.spawn(call);
            let ack = peer.acknowledge(&installed.environment)?;
            installed.environment.crash_runtime()?;
            // With the actual child reaped there can be no deadline race while
            // inspecting the exact decision and nonterminal owner at process death.
            let saved = canonical(&installed.environment, &fixture)?;
            let op = saved
                .pointer("/execution_state/execution/intent")
                .ok_or_else(|| failure("checkpoint lacks canonical pending operation"))?;
            let owner = &saved["restore_guard"]["owners"][request_id];
            assert_eq!(op["operation_id"], serde_json::to_value(ack.operation_id)?);
            assert_eq!(owner["operation_id"], op["operation_id"]);
            assert!(owner["receipt"].is_null());
            assert_eq!(owner["request"]["expected_observation_token"], token);
            assert_eq!(op["source_repair_possible"], false);
            let committed = stage == RestoreCheckpointStage::CommittedPreterminal;
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
            assert_eq!(
                saved_source_hashes(&installed.environment, &fixture)?,
                original_sources
            );
            assert_eq!(
                restore_runs(&installed.environment, &fixture)?,
                vec![format!("restore-{}", ack.operation_id)]
            );
            fs::write(
                installed
                    .environment
                    .artifacts_dir
                    .join("checkpoint-at-death.json"),
                serde_json::to_vec_pretty(&serde_json::json!({
                    "runtime_reaped_before_inspection": installed.environment.runtime_pid().is_none(),
                    "operation_id": op["operation_id"], "commit": op["commit"],
                    "receipt_absent": owner["receipt"].is_null(),
                    "source_repair_possible": op["source_repair_possible"],
                    "same_original_request_and_token": true,
                    "restore_runs": restore_runs(&installed.environment, &fixture)?,
                    "saved_source_hashes": saved_source_hashes(&installed.environment, &fixture)?,
                }))?,
            )?;
            let outcome = request
                .join()
                .map_err(|_| failure("public restore caller panicked"))?;
            if let Ok(value) = outcome {
                let receipt: WorkerRestoreResponse = serde_json::from_value(value)?;
                assert_ne!(
                    receipt.result.state,
                    WorkerRestoreState::Accepted,
                    "no caller Accepted may precede either checkpoint kill"
                );
            }
            installed.environment.restart_runtime_after_crash()?;
            assert_ne!(installed.environment.runtime_pid(), Some(ack.pid));
            Ok(ack)
        })?;
        drop(peer);
        let recovered = installed.environment.restore_worker(
            &fixture.manifest.worker_id,
            request_id,
            &token,
        )?;
        let receipt: WorkerRestoreResponse = serde_json::from_value(recovered.clone())?;
        let committed = stage == RestoreCheckpointStage::CommittedPreterminal;
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
            .ok_or_else(|| failure("missing canonical owners"))?;
        assert_eq!(owners.len(), 1);
        assert_eq!(
            owners[request_id]["operation_id"],
            serde_json::to_value(ack.operation_id)?
        );
        assert_eq!(
            owners[request_id]["request"]["expected_observation_token"],
            token
        );
        assert_eq!(
            owners[request_id]["receipt"]["state"],
            if committed { "accepted" } else { "rolled_back" }
        );
        let stale = installed.environment.restore_worker(
            &fixture.manifest.worker_id,
            "new-stale-checkpoint-intent",
            &token,
        );
        if !matches!(stale, Err(error) if error.to_string().contains("HTTP 409")) {
            return Err(failure("fresh stale intent was not rejected"));
        }
        fs::write(
            installed
                .environment
                .artifacts_dir
                .join("checkpoint-recovery.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "ack": ack, "recovered_state": receipt.result.state,
                "canonical_operation_id": owners[request_id]["operation_id"],
                "owner_count": owners.len(), "same_original_request_and_token": true,
                "saved_source_hashes_before": original_sources,
                "saved_source_hashes_after": saved_source_hashes(&installed.environment, &fixture)?,
                "restore_runs_after": restore_runs(&installed.environment, &fixture)?,
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
            assert_eq!(
                saved_source_hashes(&installed.environment, &fixture)?,
                original_sources
            );
            let report = installed.environment.cleanup()?;
            assert!(report.cleanup_success && !report.fixture_root_exists_after);
            Ok(())
        }
    })();
    fixture.source.assert_unchanged()?;
    result
}

#[test]
fn acknowledged_checkpoint_peer_disconnect_resumes_original_operation_and_tears_down() -> Result<()>
{
    observer_completion(false)
}

#[test]
fn acknowledged_checkpoint_explicit_release_resumes_original_operation_and_tears_down() -> Result<()>
{
    observer_completion(true)
}

fn observer_completion(release: bool) -> Result<()> {
    let fixture = Fixture::load()?;
    let result = (|| {
        let mut environment = WorkerE2eEnvironment::new()?;
        let mut peer = Peer::configure(
            &mut environment,
            &fixture,
            RestoreCheckpointStage::PreparedNotCommitted,
        )?;
        let installed = seed::start_in(&fixture, environment)?;
        let stopped = assert_single_state(&installed.environment, &fixture, "stopped")?;
        let token = stopped
            .restore_observation_token
            .ok_or_else(|| failure("missing observation"))?;
        let call = installed.environment.pending_restore_call(
            &fixture.manifest.worker_id,
            "peer-disconnect-request",
            &token,
        )?;
        std::thread::scope(|scope| -> Result<()> {
            let request = scope.spawn(call);
            peer.acknowledge(&installed.environment)?;
            // EOF models an abandoned observer, not a successful restore result.
            if release {
                peer.release()?;
            } else {
                drop(peer);
            }
            let value = request
                .join()
                .map_err(|_| failure("public caller panicked"))??;
            let response: WorkerRestoreResponse = serde_json::from_value(value)?;
            assert_eq!(response.result.state, WorkerRestoreState::Accepted);
            Ok(())
        })?;
        let mut subscription = WorkerSessionSubscription::connect(
            &installed.environment,
            &fixture.manifest.worker_id,
            &installed.snapshot,
        )?;
        subscription.assert_alive()?;
        stop_and_cleanup(installed, vec![subscription], &fixture)
    })();
    fixture.source.assert_unchanged()?;
    result
}

#[test]
fn acknowledged_checkpoint_failed_observer_scope_has_bounded_owned_cleanup() -> Result<()> {
    let fixture = Fixture::load()?;
    let environment = WorkerE2eEnvironment::new()?;
    let root = environment.root.clone();
    let artifacts = environment.artifacts_dir.clone();
    let started = Instant::now();
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<()> {
        let mut environment = environment;
        let mut peer = Peer::configure(
            &mut environment,
            &fixture,
            RestoreCheckpointStage::PreparedNotCommitted,
        )?;
        let installed = seed::start_in(&fixture, environment)?;
        let stopped = assert_single_state(&installed.environment, &fixture, "stopped")?;
        let token = stopped
            .restore_observation_token
            .ok_or_else(|| failure("missing observation"))?;
        let call = installed.environment.pending_restore_call(
            &fixture.manifest.worker_id,
            "failed-observer-request",
            &token,
        )?;
        std::thread::scope(|scope| -> Result<()> {
            scope.spawn(call);
            peer.acknowledge(&installed.environment)?;
            // Leave the peer connected while unwinding the scoped HTTP caller.
            // The Runtime's own deadline must unblock it, then RAII must reap
            // both owned processes and remove the environment, not wait forever.
            panic!("intentional checkpoint observer failure");
        })
    }));
    assert!(outcome.is_err(), "the observer failure was not exercised");
    assert!(
        started.elapsed() < Duration::from_secs(45),
        "owned cleanup exceeded scenario budget"
    );
    assert!(!root.exists(), "failed observer left its isolated root");
    let cleanup: serde_json::Value =
        serde_json::from_slice(&fs::read(artifacts.join("cleanup.json"))?)?;
    assert_eq!(cleanup["cleanup_success"], true);
    assert_eq!(cleanup["fixture_root_exists_after"], false);
    fixture.source.assert_unchanged()?;
    Ok(())
}
