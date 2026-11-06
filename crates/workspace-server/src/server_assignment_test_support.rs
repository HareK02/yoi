// Test-only Subject lifetime support. Session metadata and Host writer binding
// are real; the deterministic executor substitutes only the Controller protocol.
// Tests requiring real Controller release use the connected Runtime suites.
async fn assignment_test_api_with_recording_backend(
    root: impl Into<PathBuf>,
) -> (WorkspaceApi, Arc<DeterministicExecutionBackend>) {
    let (api, execution) = test_api_with_recording_backend(root).await;
    execution
        .assignment_release_enabled
        .store(true, std::sync::atomic::Ordering::SeqCst);
    (api, execution)
}

// These legacy suites deliberately install exact scoped fixtures by normal CAS.
// A Feature source supplies only its change ID, never a projection of shared sets.
// Existing legacy grants remain in their original lanes and retain their resources.
fn extend_legacy_assignment_fixture_permissions(
    store: &crate::subjektiv::SubjektivStore,
    subject: &str,
    operation_id: String,
    permissions: Vec<server_api::authorization::ScopedPermission>,
) {
    use server_api::authorization::{SubjectPermissionsUpdate, SubjectPolicySource};
    let (expected_change_id, previous) = match store.subject_policy_source(subject).unwrap() {
        Some(SubjectPolicySource::Legacy(policy)) => (Some(policy.change_id.clone()), Some(policy)),
        Some(SubjectPolicySource::Feature(policy)) => (Some(policy.change_id), None),
        None => (None, None),
    };
    let mut execution = previous
        .as_ref()
        .map(|p| p.execution.clone())
        .unwrap_or_default();
    for permission in permissions {
        if !execution.contains(&permission) {
            execution.push(permission);
        }
    }
    if previous.as_ref().is_some_and(|p| p.execution == execution) {
        return;
    }
    let delegation = previous
        .as_ref()
        .map(|p| p.delegation.clone())
        .unwrap_or_default();
    let saved = store
        .set_subject_permissions(
            subject,
            &SubjectPermissionsUpdate {
                operation_id,
                expected_change_id,
                execution: execution.clone(),
                delegation: delegation.clone(),
            },
        )
        .unwrap();
    assert_eq!(saved.execution, execution);
    assert_eq!(saved.delegation, delegation);
    assert_eq!(store.subject_permissions(subject).unwrap().unwrap(), saved);
}

fn bootstrap_test_assignment_session(api: &WorkspaceApi, worker: &RuntimeWorkerRef) {
    use server_api::authorization::{PermissionResource, ScopedPermission};
    use session_store::WorkerMetadataStore;
    assert_eq!(worker.runtime_id, EMBEDDED_WORKER_RUNTIME_ID);
    let lease = api
        .store
        .require_current_worker_singleton_owner(api.workspace_id(), worker)
        .unwrap()
        .unwrap();
    let subject = lease.key.strip_prefix(SUBJEKTIV_SINGLETON_PREFIX).unwrap();
    let store = open_subjektiv_store(api).unwrap();
    extend_legacy_assignment_fixture_permissions(
        &store,
        subject,
        format!("assignment-session-{}", worker.worker_id),
        ["session.record", "journal.recording.request"]
            .into_iter()
            .map(|operation| ScopedPermission {
                operation: operation.into(),
                resource: PermissionResource::Subject {
                    workspace_id: api.workspace_id().into(),
                    subject_id: subject.into(),
                },
            })
            .collect(),
    );
    let name = format!("worker-runtime-{}", worker.worker_id);
    let aggregate = session_store::worker_metadata::WorkerAggregateStore::new(
        api.config
            .embedded_runtime_store_root
            .join("workers")
            .join(&worker.worker_id),
        &name,
    )
    .unwrap();
    let metadata = aggregate.read_read_only().unwrap();
    let session = if let Some(active) = metadata.as_ref().and_then(|m| m.active.as_ref()) {
        active.session_id
    } else {
        let session = session_store::new_session_id();
        let mut metadata =
            metadata.unwrap_or_else(|| session_store::WorkerMetadata::new(&name, None));
        metadata.active = Some(session_store::WorkerActiveSegmentRef::pending_segment(
            session,
        ));
        aggregate
            .write(&metadata.with_workspace_id(api.workspace_id()))
            .unwrap();
        session
    };
    subject_authorization::authorize(
        api,
        worker,
        &server_api::authorization::OperationAuthorizationRequest {
            operation: "session.record".into(),
            resource: PermissionResource::Subject {
                workspace_id: api.workspace_id().into(),
                subject_id: subject.into(),
            },
        },
    )
    .unwrap();
    subjektiv_journal::bind_host_session(
        api,
        crate::subjektiv::SubjectSessionAttribution::new(
            subject,
            &worker.runtime_id,
            &worker.worker_id,
            session.to_string(),
        )
        .unwrap(),
    )
    .unwrap();
}

// The delivery factory persists real canonical Session bytes, but its scripted model
// has no Journal tools. Substitute only the sequential model/tool calls here: normal
// Host admission selects the saved source, dispatches the immutable BackendJob input
// and attests actual Runtime acceptance. No fixture source or delivery proof is seeded.
async fn reflect_test_assignment_session(
    api: &WorkspaceApi,
    worker: &RuntimeWorkerRef,
    expected_public_output: &str,
) {
    use server_api::authorization::{PermissionResource, ScopedPermission};
    use subjektiv::journal::transport::{JournalJobRequest, JournalJobResponse};
    use subjektiv::journal::{JournalAction, JournalEntryState, JournalOperation};

    let lease = api
        .store
        .require_current_worker_singleton_owner(api.workspace_id(), worker)
        .unwrap()
        .unwrap();
    let subject = lease.key.strip_prefix(SUBJEKTIV_SINGLETON_PREFIX).unwrap();
    let store = open_subjektiv_store(api).unwrap();
    extend_legacy_assignment_fixture_permissions(
        &store,
        subject,
        format!("assignment-journal-{}", worker.worker_id),
        [
            "journal.job.input.read",
            "journal.job.input.supply",
            "journal.state.read",
            "journal.entry.create",
            "journal.position.update",
            "journal.checkpoint",
            "journal.finish",
        ]
        .into_iter()
        .map(|operation| ScopedPermission {
            operation: operation.into(),
            resource: PermissionResource::Subject {
                workspace_id: api.workspace_id().into(),
                subject_id: subject.into(),
            },
        })
        .collect(),
    );
    let attrs = store
        .subject_session_attributions(subject)
        .unwrap()
        .into_iter()
        .filter(|a| a.runtime_id == worker.runtime_id && a.worker_id == worker.worker_id)
        .collect::<Vec<_>>();
    assert_eq!(attrs.len(), 1, "fixture expects one actual saved Session");
    let attr = &attrs[0];
    subjektiv_journal::reconcile_created_worker(api, worker).unwrap();
    let (job, attempt) = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let jobs = api
                .store
                .list_backend_journal_jobs(api.workspace_id(), &attr.session_id, None, 100)
                .unwrap();
            assert!(
                jobs.len() <= 1,
                "unexpected duplicate Journal intent: {jobs:?}"
            );
            if let Some(job) = jobs.into_iter().next() {
                let attempt = api
                    .store
                    .get_backend_job_attempt(
                        api.workspace_id(),
                        &job.request.job_id,
                        &crate::backend_job::attempt_id(&job.request.job_id, job.current_attempt),
                    )
                    .unwrap()
                    .unwrap();
                assert!(
                    !attempt.state.terminal(),
                    "Journal dispatch failed before reflection: {attempt:?}"
                );
                if attempt.state == BackendJobAttemptState::Dispatched {
                    break (job, attempt);
                }
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("actual Journal BackendJob was not dispatched");
    let journal_worker = attempt.worker.as_ref().unwrap();
    assert_ne!(journal_worker, worker);
    let input = subjektiv::journal::recording::JournalInput::decode(&job.request.input).unwrap();
    let canonical = subjektiv_journal::capture_input(api, attr, 0, 0, 48 * 1024).unwrap();
    assert_eq!(input.capture.entries, canonical.entries);
    assert!(!input.selected_ranges.is_empty());
    let captured_text = input
        .capture
        .entries
        .iter()
        .map(|entry| match &entry.body {
            session_store::capture::CaptureBody::CanonicalPublic { text, .. } => text.as_str(),
            _ => panic!("only actual canonical public material is eligible"),
        })
        .collect::<Vec<_>>()
        .join("\n");
    // System notifications are not canonical public recording material. Reflect
    // the actual assistant output, not a reconstructed copy of its notification.
    assert!(captured_text.contains(expected_public_output));
    let JournalJobResponse::State(state) =
        subjektiv_journal::execute(api, journal_worker, JournalJobRequest::ReadState).unwrap()
    else {
        panic!("bound Journal state required")
    };
    assert_eq!(state.selected_ranges, input.selected_ranges);
    assert_eq!(state.initial_journal.state_token, input.initial_state_token);
    assert!(state.current_open.is_none());
    let record_id = format!("record:{}", state.generation_id);
    let mut token = state.current_journal.state_token;
    for (request_id, action, covered_ranges) in [
        (
            record_id.clone(),
            JournalAction::Create {
                title: "Assignment delivery before body replacement".into(),
                body: "Received the Ticket delegation. The scripted body returned without starting a Coder; no implementation result is asserted.".into(),
                ongoing: "Keep the Subject assignment; handle future pending Ticket input in the replacement body.".into(),
                state: JournalEntryState::Open,
                source_end: None,
                correction_refs: vec![],
            },
            input.selected_ranges.clone(),
        ),
        (
            format!("position:{}", state.generation_id),
            JournalAction::UpdatePosition {
                current_position: "Delegation received; handling remains waiting. Release this body before replacement; do not replay acknowledged input.".into(),
            },
            vec![],
        ),
        (
            format!("checkpoint:{}", state.generation_id),
            JournalAction::Checkpoint { record_operation_ids: vec![record_id] },
            input.selected_ranges,
        ),
    ] {
        let JournalJobResponse::Saved(receipt) = subjektiv_journal::execute(
            api,
            journal_worker,
            JournalJobRequest::Execute {
                operation: JournalOperation { request_id, expected_state: token, action, covered_ranges },
            },
        ).unwrap() else {
            panic!("saved sequential Journal receipt required")
        };
        token = receipt.state_token;
    }
    // The ordinary recovery owner may settle a complete saved checkpoint before
    // the model's final result arrives. Exercise that owner instead of racing a
    // second live-attempt Finish request against its canonical completion.
    let completed = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let current_job = api
                .store
                .get_backend_job(api.workspace_id(), &job.request.job_id)
                .unwrap()
                .unwrap();
            let current_attempt = api
                .store
                .get_backend_job_attempt(
                    api.workspace_id(),
                    &job.request.job_id,
                    &attempt.attempt_id,
                )
                .unwrap()
                .unwrap();
            if current_attempt.state == BackendJobAttemptState::Completed {
                break current_attempt;
            }
            assert_eq!(current_attempt.state, BackendJobAttemptState::Dispatched);
            match subjektiv_journal::reconcile_attempt(api, &current_job, &current_attempt) {
                Ok(()) | Err(Error::RepositoryConflict(_)) => {}
                Err(error) => panic!("Journal checkpoint recovery failed: {error}"),
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("saved Journal checkpoint did not settle its actual attempt");
    let value = completed
        .result
        .as_ref()
        .expect("actual saved Journal result");
    subjektiv_journal::validate_result(api, &job.request, &completed, value).unwrap();
    let completion: subjektiv::journal::transport::JournalJobCompletion =
        serde_json::from_value(value.clone()).unwrap();
    assert_eq!(completion.state_token, token);
    wait_for_backend_job_worker_cleanup(
        api,
        &job.request.job_id,
        &attempt.attempt_id,
        BackendJobWorkerCleanupState::Completed,
    )
    .await;
}

fn release_test_assignment_body(api: &WorkspaceApi, worker: &RuntimeWorkerRef) {
    let response = subjektiv_journal::release_subject(
        api,
        serde_json::from_value(json!({
            "worker":worker, "operation_id":format!("assignment-release-{}", worker.worker_id),
        }))
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        response.state,
        runtime_api::SubjectReleaseState::Disconnected
    );
    let stopped = api
        .runtime
        .stop_worker(worker, WorkerLifecycleRequest::default())
        .unwrap();
    assert_eq!(
        stopped.state,
        InternalWorkerOperationState::Accepted,
        "{:?}",
        stopped.diagnostics
    );
}

async fn remove_test_assignment_body(api: &WorkspaceApi, worker: &RuntimeWorkerRef) {
    release_test_assignment_body(api, worker);
    let removed = WorkerRemovalService::new(api)
        .execute_target_removal(
            api.runtime.as_ref(),
            worker,
            "replace assignment test body",
            None,
        )
        .await
        .unwrap();
    assert_eq!(removed.status, StatusCode::OK.as_u16(), "{}", removed.body);
}

#[tokio::test]
async fn assignment_disconnect_after_restore_keeps_shutdown_and_old_protocol_fenced() {
    use worker_runtime::execution::WorkerExecutionBackend;

    let dir = tempfile::tempdir().unwrap();
    let (api, execution) = assignment_test_api_with_recording_backend(dir.path()).await;
    let started = start_attention_test_orchestrator(&api).await;
    let worker = RuntimeWorkerRef::new(
        EMBEDDED_WORKER_RUNTIME_ID,
        started.worker.unwrap().worker_id,
    );
    let worker_ref = worker_runtime::identity::WorkerRef::new(
        worker_runtime::identity::WorkerId::parse(&worker.worker_id).unwrap(),
    );
    let old = execution
        .clone()
        .attach_worker_protocol(&worker_ref)
        .unwrap();
    let stopped = api
        .runtime
        .stop_worker(&worker, WorkerLifecycleRequest::default())
        .unwrap();
    assert_eq!(stopped.state, InternalWorkerOperationState::Accepted);
    let first = api.runtime.backend_job_termination(&worker).unwrap().unwrap();
    assert!(old.validate().is_err());

    execution.accept_restores();
    let restored = start_attention_test_orchestrator(&api).await;
    assert_eq!(restored.disposition, "restored");
    assert_eq!(restored.worker.unwrap().worker_id, worker.worker_id);
    let current = execution
        .clone()
        .attach_worker_protocol(&worker_ref)
        .unwrap();
    current.validate().unwrap();
    assert!(old.validate().is_err());
    let submit = || protocol::Method::Submit {
        submission_request_id: "disconnected-body-input".into(),
        input: vec![],
    };
    assert!(old.dispatch(submit()).is_err());

    let disconnected = subjektiv_journal::release_subject(
        &api,
        serde_json::from_value(json!({
            "worker": worker, "operation_id": "disconnect-restored-body",
        }))
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        disconnected.state,
        runtime_api::SubjectReleaseState::Disconnected
    );
    current.validate().unwrap();
    assert!(current.dispatch(submit()).is_err());
    let stopped = api
        .runtime
        .stop_worker(&worker, WorkerLifecycleRequest::default())
        .unwrap();
    assert_eq!(
        stopped.state,
        InternalWorkerOperationState::Accepted,
        "{:?}",
        stopped.diagnostics
    );
    let second = api.runtime.backend_job_termination(&worker).unwrap().unwrap();
    assert_ne!(first.execution_id, second.execution_id);
    assert!(second.confirms_scope_shutdown(&worker_ref, second.stop_operation_id));
    assert!(current.validate().is_err());
    assert!(current.dispatch(submit()).is_err());
    assert!(old.dispatch(submit()).is_err());
}

impl DeterministicExecutionBackend {
    fn release_assignment_controller(
        &self,
        request: &runtime_api::SubjectReleaseRequest,
    ) -> std::result::Result<runtime_api::SubjectReleaseResponse, String> {
        use runtime_api::{SubjectReleaseAction as A, SubjectReleaseState as S};
        if !self
            .assignment_release_enabled
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            return Err(
                "deterministic backend has no actual Subject Controller release support".into(),
            );
        }
        let worker = worker_runtime::identity::WorkerRef::new(request.worker_id);
        let execution = self
            .contexts
            .lock()
            .unwrap()
            .get(&worker)
            .cloned()
            .ok_or("Subject execution is absent; no disconnect proof")?;
        let mut saved = execution.subject_release.lock().unwrap();
        if let Some((prior, state)) = saved.as_ref() {
            if *state != S::Cancelled
                && (prior.binding_id != request.binding_id
                    || prior.operation_id != request.operation_id
                    || prior.workspace_id != request.workspace_id)
            {
                return Err("another Subject release owns this execution".into());
            }
        }
        let state = match request.action {
            A::Reserve => {
                if saved.as_ref().is_some_and(|(p, s)| {
                    p.operation_id == request.operation_id && *s == S::Cancelled
                }) {
                    return Err("cancelled Subject release cannot rearm".into());
                }
                if execution.state.lock().unwrap().state != protocol::WorkerState::Idle
                    || execution.method_rx.lock().unwrap().is_none()
                {
                    return Err("mock execution cannot reserve its live Idle receiver".into());
                }
                S::Reserved
            }
            A::Cancel => S::Cancelled,
            A::Disconnect => {
                if !saved.as_ref().is_some_and(|(p, s)| {
                    p.operation_id == request.operation_id
                        && p.binding_id == request.binding_id
                        && matches!(s, S::Reserved | S::Disconnected)
                }) {
                    return Err("Subject disconnect has no exact receiver reservation".into());
                }
                // Disconnect fences new mutations in dispatch; it is not Stop.
                // Keep the receiver alive for canonical Shutdown and scope drain.
                S::Disconnected
            }
        };
        *saved = Some((request.clone(), state));
        Ok(runtime_api::SubjectReleaseResponse {
            binding_id: request.binding_id.clone(),
            operation_id: request.operation_id.clone(),
            state,
        })
    }
}
