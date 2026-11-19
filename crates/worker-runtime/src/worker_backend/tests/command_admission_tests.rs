//! Real Runtime backend + Controller admission and MockClient outside the tested
//! boundary. Runtime observation is deliberately stale; no live services are used.

use super::*;
use protocol::{WorkerCommandDisposition as D, WorkerCommandKind as K, WorkerCommandResult as R};

#[test]
#[serial_test::serial(worker_allocation)]
fn host_commands_allocate_after_external_admission_despite_stale_observation() {
    let runtime_base = tempfile::tempdir().unwrap();
    let repo = create_clean_repo();
    let store = tempfile::tempdir().unwrap();
    let factory = MockFactory {
        client: MockClient::new(simple_text_events()),
        runtime_base: runtime_base.path().to_path_buf(),
        cwd: repo.path().to_path_buf(),
        store_dir: store.path().join("sessions"),
        worker_metadata_dir: store.path().join("workers"),
        observed_cwds: Arc::new(Mutex::new(Vec::new())),
        observed_workspace_clients: Arc::new(Mutex::new(Vec::new())),
    };
    let backend = Arc::new(WorkerRuntimeExecutionBackend::new(factory).unwrap());
    let runtime =
        EmbeddedRuntime::with_execution_backend(RuntimeOptions::default(), backend.clone())
            .unwrap();
    runtime.store_config_bundle(test_bundle()).unwrap();
    let detail = runtime
        .create_worker(create_request("host-command-admission"))
        .unwrap();
    let execution = backend
        .workers
        .lock()
        .unwrap()
        .get(&detail.worker_ref)
        .unwrap()
        .clone();
    let external_id = 9_000_000;
    let external = backend.dispatch_method(
        &detail.worker_ref,
        Method::Cancel {
            command: WorkerCommandEnvelope::new(external_id),
        },
    );
    assert!(
        matches!(external.command, Some(R::Acknowledged { acknowledgement })
        if acknowledgement.command_id == external_id && acknowledgement.disposition == D::InvalidState)
    );
    // This is the precise old failure condition: a real admitted external ID
    // ahead of the Runtime bridge's observed floor.
    *execution.worker_state.write().unwrap() = protocol::WorkerStateSnapshot::initial();
    let cancel = backend.cancel_worker(&detail.worker_ref);
    assert!(
        matches!(cancel.command, Some(R::Acknowledged { acknowledgement })
        if acknowledgement.command_id > external_id && acknowledgement.command == K::Cancel
            && acknowledgement.disposition == D::InvalidState)
    );
    let compact = runtime
        .send_input(
            &detail.worker_ref,
            WorkerInput {
                kind: WorkerInputKind::Compact,
                ..WorkerInput::user("")
            },
        )
        .unwrap();
    assert!(
        matches!(compact.command, Some(R::Acknowledged { acknowledgement })
        if acknowledgement.command_id > external_id && acknowledgement.command == K::Compact
            && acknowledgement.disposition == D::Accepted)
    );
    assert!(backend.stop_worker(&detail.worker_ref).is_accepted());
}

#[test]
#[cfg(feature = "http-server")]
#[serial_test::serial(worker_allocation)]
fn external_command_retry_and_collision_return_controller_disposition() {
    let root = tempfile::tempdir().unwrap();
    let (backend, request) = profile_stop_fixture(root.path());
    let method = Method::Cancel {
        command: WorkerCommandEnvelope::new(510),
    };
    let first = backend.dispatch_method(&request.worker_ref, method.clone());
    let retry = backend.dispatch_method(&request.worker_ref, method);
    assert_eq!(first.command, retry.command);
    assert!(
        matches!(retry.command, Some(R::Acknowledged { acknowledgement }) if acknowledgement.disposition == D::InvalidState)
    );
    let conflict = backend.dispatch_method(
        &request.worker_ref,
        Method::Pause {
            command: WorkerCommandEnvelope::new(510),
        },
    );
    assert!(
        matches!(conflict.command, Some(R::Acknowledged { acknowledgement }) if acknowledgement.disposition == D::Conflict)
    );
    let stale = backend.dispatch_method(
        &request.worker_ref,
        Method::Resume {
            command: WorkerCommandEnvelope::new(509),
        },
    );
    assert!(
        matches!(stale.command, Some(R::Acknowledged { acknowledgement }) if acknowledgement.disposition == D::StaleCommandId)
    );
    assert!(backend.stop_worker(&request.worker_ref).is_accepted());
}

#[test]
#[cfg(feature = "ws-server")]
#[serial_test::serial(worker_allocation)]
fn accepted_command_replay_preserves_idle_and_new_run_through_bridge_and_runtime() {
    let runtime_base = tempfile::tempdir().unwrap();
    let repo = create_clean_repo();
    let store = tempfile::tempdir().unwrap();
    let factory = MockFactory {
        client: MockClient::sequential(vec![
            MockResponse::Hang(simple_text_events().into_iter().take(2).collect()),
            MockResponse::Hang(simple_text_events().into_iter().take(2).collect()),
        ]),
        runtime_base: runtime_base.path().to_path_buf(),
        cwd: repo.path().to_path_buf(),
        store_dir: store.path().join("sessions"),
        worker_metadata_dir: store.path().join("workers"),
        observed_cwds: Arc::new(Mutex::new(Vec::new())),
        observed_workspace_clients: Arc::new(Mutex::new(Vec::new())),
    };
    let backend = Arc::new(WorkerRuntimeExecutionBackend::new(factory).unwrap());
    let runtime =
        EmbeddedRuntime::with_execution_backend(RuntimeOptions::default(), backend.clone())
            .unwrap();
    runtime.store_config_bundle(test_bundle()).unwrap();
    let mut request = create_request("replayed-observation");
    request.worker_observation_enabled = true;
    let detail = runtime.create_worker(request).unwrap();
    let compact = runtime
        .send_input(
            &detail.worker_ref,
            WorkerInput {
                kind: WorkerInputKind::Compact,
                ..WorkerInput::user("")
            },
        )
        .unwrap();
    let Some(R::Acknowledged {
        acknowledgement: compact,
    }) = compact.command
    else {
        panic!("missing Compact admission")
    };
    wait_for_adapter_state(&backend, &detail.worker_ref, WorkerStatus::Idle);
    let assert_replay = |command: &protocol::WorkerCommandAcknowledgement,
                         repeats: usize,
                         expected: WorkerStatus| {
        let method = match command.command {
            K::Compact => Method::Compact {
                command: WorkerCommandEnvelope::new(command.command_id),
            },
            K::Cancel => Method::Cancel {
                command: WorkerCommandEnvelope::new(command.command_id),
            },
            _ => unreachable!(),
        };
        let result = backend.dispatch_method(&detail.worker_ref, method);
        assert_eq!(
            result.command,
            Some(R::Acknowledged {
                acknowledgement: command.clone()
            })
        );
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let events = runtime
                .read_worker_observation_events(&detail.worker_ref, WorkerObservationCursor::zero())
                .unwrap();
            let count = events.iter().filter(|event| matches!(&event.payload,
                Event::CommandAcknowledged { acknowledgement } if acknowledgement.command_id == command.command_id)).count();
            if count >= repeats {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "bridge did not retain command evidence"
            );
            std::thread::yield_now();
        }
        wait_for_adapter_state(&backend, &detail.worker_ref, expected);
        assert_eq!(
            runtime
                .worker_detail(&detail.worker_ref)
                .unwrap()
                .worker_state
                .unwrap()
                .catalog_status(),
            expected
        );
    };
    assert_replay(&compact, 2, WorkerStatus::Idle);
    runtime
        .send_input(&detail.worker_ref, WorkerInput::user("first run"))
        .unwrap();
    wait_for_adapter_state(&backend, &detail.worker_ref, WorkerStatus::Running);
    assert_replay(&compact, 3, WorkerStatus::Running);
    let cancel = runtime.cancel_worker(&detail.worker_ref, None).unwrap();
    let Some(R::Acknowledged {
        acknowledgement: cancel,
    }) = cancel.command
    else {
        panic!("missing Cancel admission")
    };
    wait_for_adapter_state(&backend, &detail.worker_ref, WorkerStatus::Idle);
    assert_replay(&cancel, 2, WorkerStatus::Idle);
    runtime
        .send_input(&detail.worker_ref, WorkerInput::user("newer run"))
        .unwrap();
    wait_for_adapter_state(&backend, &detail.worker_ref, WorkerStatus::Running);
    assert_replay(&cancel, 3, WorkerStatus::Running);
    assert!(backend.stop_worker(&detail.worker_ref).is_accepted());
}
