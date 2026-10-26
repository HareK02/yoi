use super::*;
use crate::restore_checkpoints::{
    RestoreCheckpointAck, RestoreCheckpointCommit, RestoreCheckpointConfig, RestoreCheckpointStage,
};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::time::Duration;

#[test]
fn checkpoint_stages_ack_canonical_decision_and_eof_resumes_same_operation_once() {
    // Component scope: durable Runtime with a substituted execution backend.
    // Real prepared Controllers and process death are owned by the E2E suite.
    for (stage, release) in [
        (RestoreCheckpointStage::PreparedNotCommitted, "release"),
        (RestoreCheckpointStage::CommittedPreterminal, "eof"),
        (RestoreCheckpointStage::CommittedPreterminal, "timeout"),
        (RestoreCheckpointStage::ActivatedPreterminal, "release"),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(TestExecutionBackend::default());
        let runtime = Runtime::with_fs_store_and_execution_backend(
            FsRuntimeStoreOptions::new(dir.path().join("runtime"))
                .with_runtime_id("checkpoint-runtime"),
            backend.clone(),
        )
        .unwrap();
        runtime.bind_runtime_identity("checkpoint-runtime").unwrap();
        runtime.store_config_bundle(test_bundle()).unwrap();
        let worker = runtime
            .create_worker(task_request("checkpoint original operation"))
            .unwrap();
        runtime.stop_worker(&worker.worker_ref, None).unwrap();
        let request = runtime.test_restore_request(&worker.worker_ref);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        runtime
            .install_test_restore_checkpoint(RestoreCheckpointConfig {
                endpoint: listener.local_addr().unwrap(),
                worker_id: worker.worker_id,
                stage,
                nonce: "runtime-stage-test".into(),
            })
            .unwrap();
        let cloned = runtime.clone();
        let worker_ref = worker.worker_ref.clone();
        let sent_request = request.clone();
        let (sent, received) = std::sync::mpsc::channel();
        let task = std::thread::spawn(move || {
            sent.send(cloned.restore_worker_operation(&worker_ref, sent_request))
                .unwrap();
        });
        listener.set_nonblocking(true).unwrap();
        let stream = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let listener = tokio::net::TcpListener::from_std(listener).unwrap();
                let (stream, _) = tokio::time::timeout(Duration::from_secs(5), listener.accept())
                    .await
                    .expect("checkpoint did not connect")
                    .unwrap();
                stream.into_std().unwrap()
            });
        stream.set_nonblocking(false).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let ack: RestoreCheckpointAck = serde_json::from_str(&line).unwrap();
        assert_eq!(ack.stage, stage);
        assert_eq!(ack.nonce, "runtime-stage-test");
        assert_eq!(ack.runtime_id, "checkpoint-runtime");
        assert_eq!(ack.worker_id, worker.worker_id);
        assert_eq!(ack.pid, std::process::id());
        assert!(ack.receipt_absent);
        assert_eq!(
            ack.commit,
            match stage {
                RestoreCheckpointStage::PreparedNotCommitted =>
                    RestoreCheckpointCommit::NotCommitted,
                RestoreCheckpointStage::CommittedPreterminal
                | RestoreCheckpointStage::ActivatedPreterminal => RestoreCheckpointCommit::Committed,
            }
        );
        assert_eq!(
            backend.restore_operation_ids.lock().unwrap().as_slice(),
            &[ack.operation_id]
        );
        if stage == RestoreCheckpointStage::ActivatedPreterminal {
            assert_eq!(backend.activation_operation_ids.lock().unwrap().as_slice(), &[ack.operation_id]);
        } else {
            assert!(backend.activation_operation_ids.lock().unwrap().is_empty());
        }
        assert!(received.try_recv().is_err());
        let canonical = runtime_store(&runtime)
            .read_durable_worker_record(&worker.worker_ref)
            .unwrap();
        let id = canonical.restore_guard.active_request_id.as_ref().unwrap();
        assert_eq!(id, &request.request_id);
        assert!(canonical.restore_guard.owners[id].receipt.is_none());
        // Release, EOF and timeout all resume the original lifecycle owner.
        let start = std::time::Instant::now();
        if release == "release" {
            reader.get_mut().write_all(b"R").unwrap();
        } else if release == "eof" {
            reader
                .get_mut()
                .shutdown(std::net::Shutdown::Write)
                .unwrap();
        }
        let result = received
            .recv_timeout(Duration::from_secs(12))
            .unwrap()
            .unwrap();
        assert!(start.elapsed() < Duration::from_secs(11));
        task.join().unwrap();
        assert_eq!(result.state, WorkerRestoreState::Accepted);
        assert_eq!(
            backend.activation_operation_ids.lock().unwrap().as_slice(),
            &[ack.operation_id]
        );
        assert_eq!(
            runtime
                .restore_worker_operation(&worker.worker_ref, request)
                .unwrap(),
            result
        );
        assert_eq!(*backend.restore_count.lock().unwrap(), 1);
        assert!(runtime.lock().unwrap().restore_checkpoint.is_none());
        runtime.stop_worker(&worker.worker_ref, None).unwrap();
        let next = runtime.test_restore_request(&worker.worker_ref);
        assert_eq!(
            runtime
                .restore_worker_operation(&worker.worker_ref, next)
                .unwrap()
                .state,
            WorkerRestoreState::Accepted
        );
        assert_eq!(*backend.restore_count.lock().unwrap(), 2);
    }
}

#[test]
fn checkpoint_disabled_mismatched_or_unreachable_does_not_change_restore_result() {
    for mode in ["disabled", "mismatch", "unreachable"] {
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(TestExecutionBackend::default());
        let runtime = Runtime::with_fs_store_and_execution_backend(
            FsRuntimeStoreOptions::new(dir.path().join("runtime"))
                .with_runtime_id("checkpoint-runtime"),
            backend.clone(),
        )
        .unwrap();
        runtime.bind_runtime_identity("checkpoint-runtime").unwrap();
        runtime.store_config_bundle(test_bundle()).unwrap();
        let worker = runtime
            .create_worker(task_request("checkpoint absent"))
            .unwrap();
        runtime.stop_worker(&worker.worker_ref, None).unwrap();
        let request = runtime.test_restore_request(&worker.worker_ref);
        if mode != "disabled" {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let endpoint = listener.local_addr().unwrap();
            drop(listener);
            runtime
                .install_test_restore_checkpoint(RestoreCheckpointConfig {
                    endpoint,
                    worker_id: if mode == "mismatch" {
                        WorkerId::now_v7()
                    } else {
                        worker.worker_id
                    },
                    stage: RestoreCheckpointStage::PreparedNotCommitted,
                    nonce: "absent-test".into(),
                })
                .unwrap();
        }
        let result = runtime
            .restore_worker_operation(&worker.worker_ref, request.clone())
            .unwrap();
        assert_eq!(result.state, WorkerRestoreState::Accepted, "{mode}");
        assert_eq!(
            runtime
                .restore_worker_operation(&worker.worker_ref, request)
                .unwrap(),
            result
        );
        assert_eq!(*backend.restore_count.lock().unwrap(), 1);
        assert_eq!(
            runtime.lock().unwrap().restore_checkpoint.is_some(),
            mode == "mismatch"
        );
    }
}

#[test]
fn checkpoint_verification_requires_canonical_worker_operation_commit_and_absent_receipt() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("runtime");
    let backend = Arc::new(TestExecutionBackend::default());
    let runtime = Runtime::with_fs_store_and_execution_backend(
        FsRuntimeStoreOptions::new(&root).with_runtime_id("checkpoint-runtime"),
        backend.clone(),
    )
    .unwrap();
    runtime.bind_runtime_identity("checkpoint-runtime").unwrap();
    runtime.store_config_bundle(test_bundle()).unwrap();
    let worker = runtime
        .create_worker(task_request("checkpoint validation"))
        .unwrap();
    runtime.stop_worker(&worker.worker_ref, None).unwrap();
    let request = runtime.test_restore_request(&worker.worker_ref);
    *backend.restore_result.lock().unwrap() = Some(WorkerExecutionSpawnResult::Errored(
        WorkerExecutionResult::errored(WorkerExecutionOperation::Restore, "not yet connected"),
    ));
    assert_eq!(
        runtime
            .restore_worker_operation(&worker.worker_ref, request.clone())
            .unwrap()
            .state,
        WorkerRestoreState::ReconciliationRequired
    );
    let operation = backend.restore_operation_ids.lock().unwrap()[0];
    let mut config = RestoreCheckpointConfig {
        endpoint: "127.0.0.1:1234".parse().unwrap(),
        worker_id: worker.worker_id,
        stage: RestoreCheckpointStage::PreparedNotCommitted,
        nonce: "verification-test".into(),
    };
    let path = root
        .join("workers")
        .join(worker.worker_id.to_string())
        .join("worker.json");
    let before = std::fs::read(&path).unwrap();
    assert!(
        runtime
            .restore_checkpoint_ack(&config, &worker.worker_ref, operation)
            .is_ok()
    );
    assert!(
        runtime
            .restore_checkpoint_ack(
                &config,
                &worker.worker_ref,
                WorkerLifecycleOperationId::new()
            )
            .is_err()
    );
    config.worker_id = WorkerId::now_v7();
    assert!(
        runtime
            .restore_checkpoint_ack(&config, &worker.worker_ref, operation)
            .is_err()
    );
    config.worker_id = worker.worker_id;
    config.stage = RestoreCheckpointStage::CommittedPreterminal;
    assert!(
        runtime
            .restore_checkpoint_ack(&config, &worker.worker_ref, operation)
            .is_err()
    );
    config.stage = RestoreCheckpointStage::ActivatedPreterminal;
    assert!(runtime.restore_checkpoint_ack(&config, &worker.worker_ref, operation).is_err());
    config.stage = RestoreCheckpointStage::PreparedNotCommitted;
    runtime_store(&runtime).fail_next_worker_reconcile_sync();
    assert!(
        runtime
            .restore_checkpoint_ack(&config, &worker.worker_ref, operation)
            .is_err()
    );
    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "verification must not write authority"
    );
    // In-memory equality must not hide an unreadable canonical aggregate.
    std::fs::write(&path, b"unreadable canonical aggregate").unwrap();
    assert!(
        runtime
            .restore_checkpoint_ack(&config, &worker.worker_ref, operation)
            .is_err()
    );
    assert_eq!(
        std::fs::read(&path).unwrap(),
        b"unreadable canonical aggregate"
    );
    std::fs::write(&path, before).unwrap();
    backend.restore_result.lock().unwrap().take();
    assert_eq!(
        runtime
            .restore_worker_operation(&worker.worker_ref, request)
            .unwrap()
            .state,
        WorkerRestoreState::Accepted
    );
    assert!(
        runtime
            .restore_checkpoint_ack(&config, &worker.worker_ref, operation)
            .is_err(),
        "terminal operation cannot ack a preterminal checkpoint"
    );
}
