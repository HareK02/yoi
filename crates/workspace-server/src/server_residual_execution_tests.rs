use super::*;

fn registry_record(worker: RuntimeWorkerRef) -> WorkerRegistryRecord {
    WorkerRegistryRecord {
        workspace_id: WORKSPACE.into(),
        worker,
        display_name: "current stored label".into(),
        profile: Some("builtin:coder".into()),
        retention_state: "normal".into(),
        transcript_ref: None,
        session_ref: None,
        summary_ref: None,
        diagnostics_ref: None,
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: "2026-01-01T00:00:00Z".into(),
    }
}

fn assigned_ticket(fixture: &Fixture) -> String {
    let record = registry_record(RuntimeWorkerRef::new(
        EMBEDDED_WORKER_RUNTIME_ID,
        Uuid::now_v7().to_string(),
    ));
    fixture.store.upsert_worker_registry(&record).unwrap();
    let mut input = ticket::NewTicket::new("cancel this implementation");
    input.workflow_state = Some(TicketState::Open);
    let ticket = browser_ticket_backend(&fixture.api)
        .unwrap()
        .create(input)
        .unwrap();
    fixture
        .store
        .set_current_ticket_worker_assignment(
            &TicketWorkerAssignmentRecord {
                workspace_id: WORKSPACE.into(),
                ticket_id: ticket.id.clone(),
                assignment_id: "current-coder".into(),
                worker: record.worker,
                assigned_by: "fixture-user".into(),
                assigned_at: "2026-01-01T00:00:00Z".into(),
            },
            None,
            "assign-event",
            "assign-operation",
            false,
        )
        .unwrap();
    ticket.id
}

async fn fixture() -> Fixture {
    let fixture = Fixture::new().await;
    fixture.api.worker_projection.shutdown_async().await;
    observed(fixture.executor.drain()).await.unwrap();
    fixture
}

fn cancel_request() -> serde_json::Value {
    serde_json::json!({
        "operation_id": "cancel-operation", "assignment_id": "current-coder", "reason": "use the corrected design"
    })
}

#[tokio::test(flavor = "current_thread")]
async fn cancelled_ticket_implementation_keeps_assignment_commit_owned() {
    let fixture = fixture().await;
    let ticket = assigned_ticket(&fixture);
    let (mut busy, waiting) = BusyObservation::install_store(fixture.store.clone());
    let mut lock = WriteLock::acquire(fixture.api.config.database_path.clone());
    let leases = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut response = Box::pin(crate::db_executor::with_operation_lifetime(
        Arc::new(CountJobLifetime(leases.clone())),
        fixture.inner_router().oneshot(Fixture::request(
            "POST",
            &format!("/api/w/{WORKSPACE}/tickets/{ticket}/implementation-cancellations"),
            cancel_request(),
        )),
    ));
    assert!(
        futures::poll!(&mut response).is_pending(),
        "cancellation must admit an owned stage, not run SQL on Tokio"
    );
    observed(waiting).await.unwrap(); // Actual cancellation transaction, after Runtime/session cleanup.
    runtime_progress_while_locked(&lock).await;
    drop(response);
    assert_eq!(
        leases.load(Ordering::SeqCst),
        1,
        "actual mixed cancellation must keep its authority after the waiter drops"
    );
    lock.release();
    busy.resume();
    observed(fixture.host_executor.drain()).await.unwrap();
    observed(fixture.executor.drain()).await.unwrap();
    assert_eq!(leases.load(Ordering::SeqCst), 0);
    assert!(
        fixture
            .store
            .get_active_ticket_worker_assignment(WORKSPACE, &ticket)
            .unwrap()
            .is_none()
    );
    assert!(
        fixture
            .store
            .get_ticket_assignment_operation(WORKSPACE, "cancel-operation")
            .unwrap()
            .is_some()
    );
    let detail = browser_ticket_detail(&fixture.api, &ticket).unwrap().0;
    assert!(detail.current_worker.is_none());
    assert_eq!(detail.state.as_str(), TicketState::Open.as_str());
}

async fn saturated_host(fixture: &Fixture) -> Vec<mpsc::Sender<()>> {
    let mut releases = Vec::new();
    let (started, running) = oneshot::channel();
    let (release, permit) = mpsc::channel::<()>();
    releases.push(release);
    fixture
        .host_executor
        .dispatch(move || {
            let _ = started.send(());
            let _ = permit.recv_timeout(WATCHDOG);
        })
        .unwrap();
    observed(running).await.unwrap();
    let (release, permit) = mpsc::channel::<()>();
    releases.push(release);
    fixture
        .host_executor
        .dispatch(move || {
            let _ = permit.recv_timeout(WATCHDOG);
        })
        .unwrap();
    releases
}

#[tokio::test(flavor = "current_thread")]
async fn cancellation_host_overload_leaves_the_coder_assignment_unchanged() {
    let fixture = fixture().await;
    let ticket = assigned_ticket(&fixture);
    let before = fixture
        .store
        .get_active_ticket_worker_assignment(WORKSPACE, &ticket)
        .unwrap()
        .unwrap();
    let releases = saturated_host(&fixture).await;
    let body = response_json(
        observed(fixture.inner_router().oneshot(Fixture::request(
            "POST",
            &format!("/api/w/{WORKSPACE}/tickets/{ticket}/implementation-cancellations"),
            cancel_request(),
        )))
        .await
        .unwrap(),
        StatusCode::SERVICE_UNAVAILABLE,
    )
    .await;
    assert!(body.to_string().contains("db_capacity_exceeded"), "{body}");
    assert_eq!(
        fixture
            .store
            .get_active_ticket_worker_assignment(WORKSPACE, &ticket)
            .unwrap()
            .unwrap(),
        before
    );
    assert!(
        fixture
            .store
            .get_ticket_assignment_operation(WORKSPACE, "cancel-operation")
            .unwrap()
            .is_none()
    );
    drop(releases);
    observed(fixture.host_executor.drain()).await.unwrap();
}

fn seed_unrepaired_mr(fixture: &Fixture) {
    let ticket = browser_ticket_backend(&fixture.api)
        .unwrap()
        .create(ticket::NewTicket::new("repair source selector"))
        .unwrap();
    fixture
        .store
        .upsert_repository(&crate::store::RepositoryRecord {
            workspace_id: WORKSPACE.into(),
            repository_id: "fixture-repo".into(),
            repository_key: "fixture-repo".into(),
            kind: "git".into(),
            provider: Some("git".into()),
            source: server_api::RepositorySource {
                kind: server_api::RepositorySourceKind::LocalPath,
                uri: fixture
                    ._dir
                    .path()
                    .join("unused-repository")
                    .to_string_lossy()
                    .into_owned(),
            },
            default_ref: Some("develop".into()),
            source_fingerprint: "sha256:fixture".into(),
            observed_status: server_api::RepositoryObservedStatus::Unverified,
            observed_at: None,
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
        })
        .unwrap();
    // Persist the external unrepaired-selector shape. No active assignment:
    // the real handler must reach its assignment read, then reject without provider dispatch.
    let conn = Connection::open(&fixture.api.config.database_path).unwrap();
    conn.execute("INSERT INTO merge_requests VALUES (?1,'repair-fixture','fixture-repo','open',NULL,'develop',?2,?2)", rusqlite::params![WORKSPACE, "2026-01-01T00:00:00Z"]).unwrap();
    conn.execute("INSERT INTO merge_request_ticket_relations VALUES (?1,'repair-fixture',?2,'implements',?3)", rusqlite::params![WORKSPACE, ticket.id, "2026-01-01T00:00:00Z"]).unwrap();
}

fn repair_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        "cookie",
        "db_execution_session=fixture-browser-token"
            .parse()
            .unwrap(),
    );
    headers
}

fn repair_input() -> Json<server_api::RepairMergeRequestSelectorRequest> {
    Json(server_api::RepairMergeRequestSelectorRequest {
        selector_from: "work/fixed-selector".into(),
        reason: "repair the imported selector".into(),
        explicit_confirmation: true,
    })
}

#[tokio::test(flavor = "current_thread")]
async fn authenticated_selector_repair_waits_off_tokio_and_keeps_cancelled_host_ownership() {
    let fixture = fixture().await;
    seed_unrepaired_mr(&fixture);
    let leases = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut repair = Box::pin(crate::db_executor::with_operation_lifetime(
        Arc::new(CountJobLifetime(leases.clone())),
        scoped_repair_merge_request_selector(
            State(fixture.api.clone()),
            repair_headers(),
            AxumPath((WORKSPACE.into(), "repair-fixture".into())),
            repair_input(),
        ),
    ));
    assert!(futures::poll!(&mut repair).is_pending()); // Browser authentication DB stage only.
    observed(fixture.executor.drain()).await.unwrap();
    let (mut busy, waiting) = BusyObservation::install_store(fixture.store.clone());
    let mut lock = WriteLock::acquire(fixture.api.config.database_path.clone());
    let writer = fixture.store.clone();
    fixture
        .executor
        .dispatch(move || {
            writer
                .mark_api_token_used("missing", "2026-01-01T00:00:00Z")
                .unwrap();
        })
        .unwrap();
    observed(waiting).await.unwrap();
    assert!(
        futures::poll!(&mut repair).is_pending(),
        "post-auth selector repair must not wait on the connection mutex on Tokio"
    );
    runtime_progress_while_locked(&lock).await;
    drop(repair);
    assert_eq!(
        leases.load(Ordering::SeqCst),
        1,
        "actual repair stage must retain authority after cancellation"
    );
    lock.release();
    busy.resume();
    observed(fixture.host_executor.drain()).await.unwrap();
    observed(fixture.executor.drain()).await.unwrap();
    assert_eq!(leases.load(Ordering::SeqCst), 0);
    let mr = merge_request_store(&fixture.api, WORKSPACE)
        .unwrap()
        .get_by_id(WORKSPACE, "repair-fixture")
        .unwrap();
    assert!(mr.selector_from.is_none());
    assert!(
        merge_request_store(&fixture.api, WORKSPACE)
            .unwrap()
            .thread_page_by_id(WORKSPACE, "repair-fixture", None, 10)
            .unwrap()
            .is_empty()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn selector_repair_host_overload_preserves_confirmation_and_no_side_effects() {
    let fixture = fixture().await;
    seed_unrepaired_mr(&fixture);
    let releases = saturated_host(&fixture).await;
    let error = scoped_repair_merge_request_selector(
        State(fixture.api.clone()),
        repair_headers(),
        AxumPath((WORKSPACE.into(), "repair-fixture".into())),
        repair_input(),
    )
    .await
    .unwrap_err();
    let body = response_json(error.into_response(), StatusCode::SERVICE_UNAVAILABLE).await;
    assert!(body.to_string().contains("db_capacity_exceeded"), "{body}");
    let mut input = repair_input();
    input.0.explicit_confirmation = false;
    let error = scoped_repair_merge_request_selector(
        State(fixture.api.clone()),
        repair_headers(),
        AxumPath((WORKSPACE.into(), "repair-fixture".into())),
        input,
    )
    .await
    .unwrap_err();
    assert!(
        !matches!(error.error, Error::DbCapacityExceeded),
        "browser confirmation denial must precede Host admission"
    );
    let mr = merge_request_store(&fixture.api, WORKSPACE)
        .unwrap()
        .get_by_id(WORKSPACE, "repair-fixture")
        .unwrap();
    assert!(mr.selector_from.is_none());
    drop(releases);
    observed(fixture.host_executor.drain()).await.unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn post_restore_projection_waits_off_tokio_and_preserves_historical_results() {
    let fixture = fixture().await;
    let record = registry_record(RuntimeWorkerRef::new(
        EMBEDDED_WORKER_RUNTIME_ID,
        Uuid::now_v7().to_string(),
    ));
    fixture.store.upsert_worker_registry(&record).unwrap();
    let mut historical = worker_summary_from_registry(&record);
    historical.label = "historical label".into();
    historical.state = "paused".into();
    let result = || crate::hosts::InternalWorkerRestoreResult {
        state: server_api::WorkerRestoreState::Rejected,
        worker: Some(historical.clone()),
        diagnostics: Vec::new(),
    };
    let (mut busy, waiting) = BusyObservation::install_store(fixture.store.clone());
    let mut lock = WriteLock::acquire(fixture.api.config.database_path.clone());
    let writer = fixture.store.clone();
    fixture
        .executor
        .dispatch(move || {
            writer
                .mark_api_token_used("missing", "2026-01-01T00:00:00Z")
                .unwrap();
        })
        .unwrap();
    observed(waiting).await.unwrap();
    let leases = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut projection = Box::pin(crate::db_executor::with_operation_lifetime(
        Arc::new(CountJobLifetime(leases.clone())),
        project_worker_restore_response_owned(
            fixture.api.clone(),
            record.worker.runtime_id.clone(),
            record.worker.worker_id.clone(),
            result(),
        ),
    ));
    assert!(
        futures::poll!(&mut projection).is_pending(),
        "stored projection must await a whole DB stage after Host restore returns"
    );
    runtime_progress_while_locked(&lock).await;
    assert_eq!(
        observed(fixture.host_executor.run(|| Ok::<_, Error>("control")))
            .await
            .unwrap(),
        "control"
    );
    let error = project_worker_restore_response_owned(
        fixture.api.clone(),
        record.worker.runtime_id.clone(),
        record.worker.worker_id.clone(),
        result(),
    )
    .await
    .unwrap_err();
    response_json(error.into_response(), StatusCode::SERVICE_UNAVAILABLE).await;
    let Json(no_worker) = project_worker_restore_response_owned(
        fixture.api.clone(),
        record.worker.runtime_id.clone(),
        record.worker.worker_id.clone(),
        crate::hosts::InternalWorkerRestoreResult {
            state: server_api::WorkerRestoreState::Rejected,
            worker: None,
            diagnostics: Vec::new(),
        },
    )
    .await
    .unwrap();
    assert_eq!(
        no_worker.result.state,
        server_api::WorkerRestoreState::Rejected
    );
    drop(projection);
    assert_eq!(leases.load(Ordering::SeqCst), 1);
    lock.release();
    busy.resume();
    observed(fixture.executor.drain()).await.unwrap();
    assert_eq!(leases.load(Ordering::SeqCst), 0);
    let Json(response) = project_worker_restore_response_owned(
        fixture.api.clone(),
        record.worker.runtime_id.clone(),
        record.worker.worker_id.clone(),
        result(),
    )
    .await
    .unwrap();
    assert_eq!(
        response.result.state,
        server_api::WorkerRestoreState::Rejected
    );
    let worker = response.result.worker.unwrap();
    assert_eq!(worker.label, record.display_name);
    assert_eq!(
        worker.state, "paused",
        "a historical receipt is not a new Runtime observation"
    );
    assert_eq!(
        fixture
            .store
            .get_worker_registry(WORKSPACE, &record.worker)
            .unwrap()
            .unwrap(),
        record,
        "response projection must not write a new observation"
    );
    observed(fixture.executor.drain()).await.unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn ticket_source_ref_reads_and_ready_publication_leave_sql_and_tokio_free() {
    for operation in ["detail", "query", "ready"] {
        let repository_dir = tempfile::tempdir().unwrap();
        let source = server_api::RepositorySource {
            kind: server_api::RepositorySourceKind::LocalPath,
            uri: repository_dir.path().to_string_lossy().into_owned(),
        };
        let fixture = Fixture::with_repositories(vec![ConfiguredRepository {
            id: "fixture-repo".into(),
            repository_key: "fixture-repo".into(),
            provider: "git".into(),
            source_fingerprint: crate::repository_source::repository_source_fingerprint(&source),
            source,
            observed_status: server_api::RepositoryObservedStatus::Unverified,
            observed_at: None,
            path: Some(repository_dir.path().into()),
            default_selector: Some("develop".into()),
        }])
        .await;
        fixture.api.worker_projection.shutdown_async().await;
        observed(fixture.executor.drain()).await.unwrap();
        let mut reply = HeldRuntimeReply::for_path("/v1/repository-refs/observe");
        // Narrow Runtime HTTP/provider placement fixture, not a signed authority
        // or complete Git/Runtime E2E test. Existing signed routes test that fence.
        install_remote_fixture(
            &fixture,
            "ref-runtime",
            &format!("http://{}", reply.address),
            false,
        );
        let worker = RuntimeWorkerRef::new("ref-runtime", Uuid::now_v7().to_string());
        fixture
            .store
            .upsert_worker_registry(&registry_record(worker.clone()))
            .unwrap();
        let mut input = ticket::NewTicket::new("source ref projection");
        input.workflow_state = Some(TicketState::Open);
        let ticket = browser_ticket_backend(&fixture.api)
            .unwrap()
            .create(input)
            .unwrap();
        fixture
            .store
            .set_current_ticket_worker_assignment(
                &TicketWorkerAssignmentRecord {
                    workspace_id: WORKSPACE.into(),
                    ticket_id: ticket.id.clone(),
                    assignment_id: "ref-assignment".into(),
                    worker: worker.clone(),
                    assigned_by: "fixture-user".into(),
                    assigned_at: "2026-01-01T00:00:00Z".into(),
                },
                None,
                "ref-event",
                "ref-operation",
                false,
            )
            .unwrap();
        merge_request_store(&fixture.api, WORKSPACE)
            .unwrap()
            .open_merge_request(merge_request::OpenMergeRequest {
                merge_request_id: "ref-mr".into(),
                ticket_id: ticket.id.clone(),
                repository_id: "fixture-repo".into(),
                selector_from: "work/source".into(),
                selector_to: "develop".into(),
                summary: String::new(),
                auth: merge_request::MergeRequestAuth {
                    workspace_id: WORKSPACE.into(),
                    repository_id: "fixture-repo".into(),
                    runtime_id: worker.runtime_id,
                    worker_id: worker.worker_id,
                    assignment_id: "ref-assignment".into(),
                },
                now: Utc::now(),
            })
            .unwrap();
        let leases = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let api = fixture.api.clone();
        let ticket_id = ticket.id.clone();
        let mut response = Box::pin(crate::db_executor::with_operation_lifetime(
            Arc::new(CountJobLifetime(leases.clone())),
            async move {
                match operation {
                    "detail" => scoped_get_ticket(
                        State(api),
                        AxumPath(ScopedRecordPath {
                            workspace_id: WORKSPACE.into(),
                            id: ticket_id,
                        }),
                    )
                    .await
                    .map(|_| ()),
                    "query" => scoped_query_tickets(
                        State(api),
                        AxumPath(ScopedWorkspacePath {
                            workspace_id: WORKSPACE.into(),
                        }),
                        Json(TicketQueryRequest::default()),
                    )
                    .await
                    .map(|_| ()),
                    _ => scoped_mark_ticket_ready_from_browser(
                        State(api),
                        HeaderMap::new(),
                        AxumPath(ScopedRecordPath {
                            workspace_id: WORKSPACE.into(),
                            id: ticket_id,
                        }),
                        Json(InternalTicketMarkReadyRequest {
                            operation_key: "ready-operation".into(),
                            reason: None,
                            intake_summary: None,
                        }),
                    )
                    .await
                    .map(|_| ()),
                }
            },
        ));
        assert!(futures::poll!(&mut response).is_pending());
        // Ready reaches the source POST only after mutation and best-effort
        // notification. Bound this multi-stage setup with the fixture watchdog,
        // not the shorter budget used below to prove timer/independent control
        // progress while the actual provider request is held.
        let request = tokio::select! {
            request = tokio::time::timeout(WATCHDOG, reply.entered.take().unwrap()) => request.expect("source observation setup did not reach the provider").unwrap(),
            result = &mut response => panic!("{operation} returned before the held source observation: {result:?}"),
        };
        assert!(
            request.contains("/v1/repository-refs/observe"),
            "{operation}: {request}"
        );
        let store = fixture.store.clone();
        observed(
            fixture
                .executor
                .run(move || store.get_workspace_memory_settings(WORKSPACE)),
        )
        .await
        .unwrap();
        assert_eq!(
            observed(tokio::spawn(async { "control" })).await.unwrap(),
            "control"
        );
        observed(tokio::time::sleep(StdDuration::from_millis(1))).await;
        assert!(
            !reply.upstream_released.load(Ordering::SeqCst),
            "watchdog release is not provider progress"
        );
        if operation == "ready" {
            let saved = browser_ticket_backend(&fixture.api)
                .unwrap()
                .show(TicketIdOrSlug::Id(ticket.id))
                .unwrap();
            assert_eq!(
                saved.meta.readiness.as_deref(),
                Some("ready"),
                "Ready mutation must be durable before the held detail source observation"
            );
            assert_eq!(
                saved
                    .events
                    .iter()
                    .filter(
                        |event| event.attributes.get("operation_key").map(String::as_str)
                            == Some("ready-operation")
                    )
                    .count(),
                1
            );
            assert_eq!(
                saved.meta.workflow_state,
                TicketState::Open,
                "Ready must commit before detail publication"
            );
        }
        drop(response);
        assert_eq!(
            leases.load(Ordering::SeqCst),
            1,
            "{operation}: cancelled provider stage lost actual ownership"
        );
        fixture.host_executor.dispatch(|| {}).unwrap(); // Remaining finite queued slot.
        let error = scoped_get_ticket(
            State(fixture.api.clone()),
            AxumPath(ScopedRecordPath {
                workspace_id: WORKSPACE.into(),
                id: "unused".into(),
            }),
        )
        .await
        .unwrap_err();
        response_json(error.into_response(), StatusCode::SERVICE_UNAVAILABLE).await;
        reply.finish();
        observed(fixture.host_executor.drain()).await.unwrap();
        observed(fixture.executor.drain()).await.unwrap();
        assert_eq!(leases.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn worker_control_source_lookup_waits_off_tokio_and_retains_finite_host_ownership() {
    let fixture = fixture().await;
    let mut reply = HeldRuntimeReply::new();
    install_remote_fixture(
        &fixture,
        "source-runtime",
        &format!("http://{}", reply.address),
        false,
    );
    let source = RuntimeWorkerRef::new("source-runtime", Uuid::now_v7().to_string());
    let mut headers = HeaderMap::new();
    headers.insert("x-yoi-runtime-id", source.runtime_id.parse().unwrap());
    headers.insert("x-yoi-worker-id", source.worker_id.parse().unwrap());
    let leases = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let request = || {
        Json(server_api::RuntimeWorkerLifecycleRequest {
            reason: None,
            ticket_assignment: None,
        })
    };
    let path = || {
        AxumPath(ScopedRuntimeWorkerPath {
            workspace_id: WORKSPACE.into(),
            worker: source.clone(),
        })
    };
    let mut control = Box::pin(crate::db_executor::with_operation_lifetime(
        Arc::new(CountJobLifetime(leases.clone())),
        cancel_known_worker(
            State(fixture.api.clone()),
            path(),
            headers.clone(),
            request(),
        ),
    ));
    assert!(
        futures::poll!(&mut control).is_pending(),
        "control source lookup must not synchronously join Runtime HTTP on Tokio"
    );
    let request_line = observed(reply.entered.take().unwrap()).await.unwrap();
    assert!(request_line.contains("/v1/workers/"), "{request_line}");
    let store = fixture.store.clone();
    observed(
        fixture
            .executor
            .run(move || store.get_workspace_memory_settings(WORKSPACE)),
    )
    .await
    .unwrap();
    assert_eq!(
        observed(tokio::spawn(async { "control" })).await.unwrap(),
        "control"
    );
    observed(tokio::time::sleep(StdDuration::from_millis(1))).await;
    assert!(!reply.upstream_released.load(Ordering::SeqCst));
    drop(control);
    assert_eq!(leases.load(Ordering::SeqCst), 1);
    fixture.host_executor.dispatch(|| {}).unwrap();
    let error = cancel_known_worker(State(fixture.api.clone()), path(), headers, request())
        .await
        .unwrap_err();
    let body = response_json(error.into_response(), StatusCode::SERVICE_UNAVAILABLE).await;
    assert!(
        body.to_string().contains("db_capacity_exceeded"),
        "Host overload is availability, not identity denial: {body}"
    );
    reply.finish();
    observed(fixture.host_executor.drain()).await.unwrap();
    observed(fixture.executor.drain()).await.unwrap();
    assert_eq!(leases.load(Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn ticket_tracking_authority_and_case_audit_lookup_leave_db_and_tokio_free() {
    for operation in ["tracking", "case"] {
        let fixture = fixture().await;
        let ticket = browser_ticket_backend(&fixture.api)
            .unwrap()
            .create(ticket::NewTicket::new("decision authority"))
            .unwrap();
        let mut reply = HeldRuntimeReply::new();
        install_remote_fixture(
            &fixture,
            "orchestrator-runtime",
            &format!("http://{}", reply.address),
            false,
        );
        let memory = fixture
            .store
            .get_workspace_memory_settings(WORKSPACE)
            .unwrap();
        let reservation = fixture
            .store
            .reserve_worker_create(
                WORKSPACE,
                "orchestrator-runtime",
                "orch-create",
                "orch-create-fingerprint",
                Some(crate::hosts::WORKSPACE_ORCHESTRATOR_SINGLETON_KEY),
                &memory,
            )
            .unwrap();
        let source =
            RuntimeWorkerRef::new("orchestrator-runtime", reservation.worker_id.to_string());
        fixture
            .store
            .upsert_worker_registry(&registry_record(source.clone()))
            .unwrap();
        fixture
            .store
            .complete_worker_create_reservation(WORKSPACE, reservation.worker_id)
            .unwrap();
        let context = server_api::ServerRequestContext {
            actor: None,
            worker_source: None,
            runtime_source: Some(server_api::ServerRuntimeSource {
                authentication_binding_id: "verified-fixture-binding".into(),
                runtime_id: source.runtime_id.clone(),
                worker_id: Some(source.worker_id.clone()),
            }),
            origin: None,
            transport_headers: Vec::new(),
        };
        let backend = browser_ticket_backend(&fixture.api).unwrap();
        let before = backend.show(TicketIdOrSlug::Id(ticket.id.clone())).unwrap();
        let leases = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let api = fixture.api.clone();
        let ticket_id = ticket.id.clone();
        let mut decision = Box::pin(crate::db_executor::with_operation_lifetime(
            Arc::new(CountJobLifetime(leases.clone())),
            async {
                if operation == "tracking" {
                    scoped_ticket_assignment_tracking_update_with_context(
                        State(api),
                        HeaderMap::new(),
                        &context,
                        AxumPath((WORKSPACE.into(), ticket_id)),
                        Json(ticket::TicketAssignmentTrackingUpdate {
                            assignment_id: "no-delegation".into(),
                            expected_last_event_id: "baseline".into(),
                            operation_key: "track-operation".into(),
                            tracking: ticket::TicketAssignmentTracking {
                                status: ticket::TicketAssignmentTrackingStatus::Inprogress,
                                waiting_reason: None,
                                progress: None,
                                next_action: None,
                                result: None,
                            },
                        }),
                    )
                    .await
                    .map(|_| ())
                } else {
                    apply_ticket_case_update(
                        State(api),
                        Some(source),
                        None,
                        AxumPath((WORKSPACE.into(), ticket_id)),
                        Json(ticket::TicketCaseUpdate {
                            expected_content_digest: ticket::ticket_content_digest(&before),
                            expected_state: before.meta.workflow_state,
                            expected_closure_operation_key: None,
                            operation_key: "case-operation".into(),
                            action: ticket::TicketCaseAction::Close {
                                reason: ticket::TicketClosureReason::Completed,
                            },
                            explanation: "check current authority".into(),
                            references: Vec::new(),
                            author: None,
                        }),
                    )
                    .await
                    .map(|_| ())
                }
            },
        ));
        assert!(futures::poll!(&mut decision).is_pending());
        let request = observed(reply.entered.take().unwrap()).await.unwrap();
        assert!(request.contains("/v1/workers/"), "{operation}: {request}");
        let store = fixture.store.clone();
        observed(
            fixture
                .executor
                .run(move || store.get_workspace_memory_settings(WORKSPACE)),
        )
        .await
        .unwrap();
        assert_eq!(
            observed(tokio::spawn(async { "control" })).await.unwrap(),
            "control"
        );
        observed(tokio::time::sleep(StdDuration::from_millis(1))).await;
        assert!(!reply.upstream_released.load(Ordering::SeqCst));
        drop(decision);
        assert_eq!(leases.load(Ordering::SeqCst), 1);
        reply.finish();
        observed(fixture.host_executor.drain()).await.unwrap();
        observed(fixture.executor.drain()).await.unwrap();
        assert_eq!(leases.load(Ordering::SeqCst), 0);
        let after = backend.show(TicketIdOrSlug::Id(ticket.id)).unwrap();
        if operation == "tracking" {
            assert_eq!(
                after, before,
                "tracking still requires live assignment authority"
            );
        } else {
            // Profile-authorized case changes do not depend on a live
            // Orchestrator lookup. Its failure only affects audit role context.
            assert_eq!(after.meta.workflow_state, ticket::TicketState::Closed);
            assert_eq!(
                after.events.last().unwrap().attributes["source_actor_role"],
                "worker"
            );
            assert_eq!(
                after.current_closure.unwrap().operation_key,
                "case-operation"
            );
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn cancelled_feature_shutdown_keeps_sql_close_owned_without_blocking_control() {
    let fixture = fixture().await;
    let (database, _) = fixture.feature_database();
    let mutation = database.clone();
    let (entered, waiting) = oneshot::channel();
    let (release, permit) = mpsc::channel();
    let released = Arc::new(AtomicBool::new(false));
    let flag = released.clone();
    let thread = std::thread::spawn(move || {
        mutation
            .with_connection(|conn| {
                conn.execute_batch("BEGIN IMMEDIATE; CREATE TABLE close_owner_probe(id INTEGER)")?;
                let _ = entered.send(());
                let _ = permit.recv_timeout(WATCHDOG);
                flag.store(true, Ordering::SeqCst);
                conn.execute_batch("ROLLBACK")?;
                Ok(())
            })
            .unwrap();
    });
    let mut lock = WriteLock {
        release: Some(release),
        thread: Some(thread),
        released,
    };
    observed(waiting).await.unwrap(); // actual SQL transaction owns the Feature connection
    // Force close to be queued before the sole DB worker can start it. Pending
    // admission alone must not be mistaken for an available second queue slot.
    let (started, running) = oneshot::channel();
    let (start_close, held) = mpsc::channel();
    fixture
        .executor
        .dispatch(move || {
            let _ = started.send(());
            let _ = held.recv_timeout(WATCHDOG);
        })
        .unwrap();
    observed(running).await.unwrap();
    let leases = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut close = Box::pin(crate::db_executor::with_operation_lifetime(
        Arc::new(CountJobLifetime(leases.clone())),
        shutdown_workspace_feature_storage(&fixture.api),
    ));
    assert!(
        futures::poll!(&mut close).is_pending(),
        "Feature connection close must not wait on Tokio"
    );
    assert!(matches!(
        fixture.executor.dispatch(|| {}),
        Err(Error::DbCapacityExceeded)
    ));
    start_close.send(()).unwrap();
    let fenced = observed(async {
        loop {
            match fixture
                .api
                .feature_storage
                .open(&fixture.api.subjektiv_registration)
            {
                Err(crate::FeatureStorageError::ShuttingDown) => break,
                Ok(_) => tokio::task::yield_now().await,
                Err(error) => panic!("unexpected Feature admission failure: {error}"),
            }
        }
    });
    tokio::select! {
        _ = fenced => {},
        result = &mut close => panic!("close completed while its connection is held: {result:?}"),
    }
    // Observing the real shutdown fence proves close was dequeued and started;
    // only now can the one waiting slot be filled deterministically.
    runtime_progress_while_locked(&lock).await;
    assert_eq!(
        observed(fixture.host_executor.run(|| Ok::<_, Error>("control")))
            .await
            .unwrap(),
        "control"
    );
    fixture.executor.dispatch(|| {}).unwrap();
    assert!(matches!(
        shutdown_workspace_feature_storage(&fixture.api)
            .await
            .unwrap_err(),
        Error::DbCapacityExceeded
    ));
    drop(close);
    assert_eq!(leases.load(Ordering::SeqCst), 1);
    lock.release();
    observed(fixture.executor.drain()).await.unwrap();
    assert_eq!(leases.load(Ordering::SeqCst), 0);
    assert!(
        database.with_connection(|_| Ok(())).is_err(),
        "accepted shutdown must fence later use"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn session_workdir_and_upload_callers_keep_provider_waits_off_sql_and_tokio() {
    use server_api::ServerApi;
    for operation in ["session", "history", "attachment", "workdirs", "upload"] {
        let fixture = fixture().await;
        let mut reply = HeldRuntimeReply::new();
        install_remote_fixture(
            &fixture,
            "caller-runtime",
            &format!("http://{}", reply.address),
            false,
        );
        let worker_id = Uuid::now_v7().to_string();
        fixture
            .store
            .upsert_worker_registry(&registry_record(RuntimeWorkerRef::new(
                "caller-runtime",
                &worker_id,
            )))
            .unwrap();
        let actor = RequestActor {
            user_id: "fixture-user".into(),
            account_id: format!("account-{WORKSPACE}"),
            handle: "fixture-user".into(),
            display_name: "Fixture user".into(),
            auth_method: ActorAuthMethod::BrowserSession,
        };
        fixture.api.attachment_upload_grants.lock().unwrap().insert(
            "caller-upload".into(),
            AttachmentUploadGrant {
                workspace_id: WORKSPACE.into(),
                runtime_id: "caller-runtime".into(),
                worker_id: worker_id.clone(),
                account_id: actor.account_id.clone(),
                file_name: "notes.txt".into(),
                media_type: "text/plain".into(),
                expires_at_ms: u64::MAX,
                state: AttachmentUploadGrantState::Pending,
            },
        );
        let leases = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let api = fixture.api.clone();
        let task = tokio::spawn(crate::db_executor::with_operation_lifetime(
            Arc::new(CountJobLifetime(leases.clone())),
            async move {
                let service = ServerApiContractService::Workspace(api.clone());
                match operation {
                    "session" => {
                        let _ = service
                            .worker_session(WORKSPACE.into(), "caller-runtime".into(), worker_id)
                            .await;
                    }
                    "history" => {
                        let _ = service
                            .worker_session_history(
                                WORKSPACE.into(),
                                "caller-runtime".into(),
                                worker_id,
                                server_api::WorkspaceWorkerSessionHistoryQuery {
                                    cursor: None,
                                    limit: Some(1),
                                },
                            )
                            .await;
                    }
                    "attachment" => {
                        let _ = scoped_worker_session_attachment(
                            State(api),
                            AxumPath((
                                WORKSPACE.into(),
                                "caller-runtime".into(),
                                worker_id,
                                "session".into(),
                                "attachment".into(),
                            )),
                            Extension(actor),
                        )
                        .await;
                    }
                    "workdirs" => {
                        let _ = scoped_list_working_directories(
                            State(api),
                            AxumPath(ScopedWorkspacePath {
                                workspace_id: WORKSPACE.into(),
                            }),
                        )
                        .await;
                    }
                    "upload" => {
                        let _ = upload_runtime_worker_file_contract(
                            api,
                            actor,
                            ScopedAttachmentUploadPath {
                                workspace_id: WORKSPACE.into(),
                                runtime_id: "caller-runtime".into(),
                                worker_id,
                                upload_id: "caller-upload".into(),
                            },
                            b"notes",
                        )
                        .await;
                    }
                    _ => unreachable!(),
                }
            },
        ));
        let request = observed(reply.entered.take().unwrap()).await.unwrap();
        assert!(request.contains("/v1/"), "{operation}: {request}");
        assert!(
            !reply.upstream_released.load(Ordering::SeqCst),
            "{operation}: watchdog release is not executor progress"
        );
        let store = fixture.store.clone();
        observed(
            fixture
                .executor
                .run(move || store.get_workspace_memory_settings(WORKSPACE)),
        )
        .await
        .unwrap();
        assert_eq!(
            observed(tokio::spawn(async { "control" })).await.unwrap(),
            "control"
        );
        observed(tokio::time::sleep(StdDuration::from_millis(1))).await;
        let mut queued = Box::pin(fixture.host_executor.run(|| Ok::<_, Error>(())));
        assert!(futures::poll!(&mut queued).is_pending());
        let error = scoped_list_working_directories(
            State(fixture.api.clone()),
            AxumPath(ScopedWorkspacePath {
                workspace_id: WORKSPACE.into(),
            }),
        )
        .await
        .unwrap_err();
        response_json(error.into_response(), StatusCode::SERVICE_UNAVAILABLE).await;
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert_eq!(
            leases.load(Ordering::SeqCst),
            1,
            "{operation}: lost provider owner"
        );
        reply.finish();
        observed(queued).await.unwrap();
        observed(fixture.host_executor.drain()).await.unwrap();
        observed(fixture.executor.drain()).await.unwrap();
        assert_eq!(
            leases.load(Ordering::SeqCst),
            0,
            "{operation}: retained owner after completion"
        );
        if operation == "upload" {
            assert!(
                matches!(
                    fixture
                        .api
                        .attachment_upload_grants
                        .lock()
                        .unwrap()
                        .get("caller-upload")
                        .unwrap()
                        .state,
                    AttachmentUploadGrantState::Pending
                ),
                "failed Runtime lookup must finish the accepted grant claim even with a cancelled waiter"
            );
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn repository_create_keeps_cancelled_sql_commit_owned_and_replays_one_intent() {
    let fixture = fixture().await;
    let (mut busy, waiting) = BusyObservation::install_store(fixture.api.config_store.clone());
    let mut lock = WriteLock::acquire(fixture.api.config.database_path.clone());
    let leases = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let request = || {
        Fixture::request(
            "POST",
            &format!("/api/w/{WORKSPACE}/repositories"),
            serde_json::json!({ "repository_key": "owned-repository", "source": "https://example.invalid/repo.git", "default_ref": "main" }),
        )
    };
    let mut response = Box::pin(crate::db_executor::with_operation_lifetime(
        Arc::new(CountJobLifetime(leases.clone())),
        fixture.inner_router().oneshot(request()),
    ));
    tokio::select! {
        result = &mut response => panic!("repository must wait for its write transaction: {:?}", result.unwrap().status()),
        reached = observed(waiting) => reached.unwrap(),
    }
    runtime_progress_while_locked(&lock).await;
    assert_eq!(
        observed(fixture.host_executor.run(|| Ok::<_, Error>("control")))
            .await
            .unwrap(),
        "control"
    );
    drop(response);
    assert_eq!(leases.load(Ordering::SeqCst), 1);
    lock.release();
    busy.resume();
    observed(fixture.executor.drain()).await.unwrap();
    assert_eq!(leases.load(Ordering::SeqCst), 0);
    let rows = fixture.store.list_repositories(WORKSPACE).unwrap();
    assert_eq!(
        rows.iter()
            .filter(|record| record.repository_key == "owned-repository")
            .count(),
        1
    );
    let replay = observed(fixture.inner_router().oneshot(request()))
        .await
        .unwrap();
    let body = response_json(replay, StatusCode::OK).await;
    assert_eq!(body["replayed"], true);
    assert_eq!(
        fixture.store.list_repositories(WORKSPACE).unwrap().len(),
        rows.len()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn drive_and_config_preflight_keep_live_authority_owned_and_report_overload_as_availability()
{
    async fn preflight(
        api: &WorkspaceApi,
        context: &server_api::ServerRequestContext,
        operation: &str,
    ) -> u16 {
        match operation {
            "config" => {
                workspace_config::identity(api, context, WORKSPACE)
                    .await
                    .err()
                    .unwrap()
                    .status
            }
            "drive" => api_macros::HttpError::status_code(
                &drive_grants::authorized_drive(api, context, WORKSPACE, false)
                    .await
                    .err()
                    .unwrap(),
            ),
            "grant" => api_macros::HttpError::status_code(
                &drive_grants::create_grant(
                    api,
                    context,
                    WORKSPACE,
                    server_api::DriveGrantCreateRequest {
                        runtime_id: "preflight-runtime".into(),
                        worker_id: "target-worker".into(),
                        access: server_api::DriveAccess::ReadOnly,
                    },
                )
                .await
                .err()
                .unwrap(),
            ),
            _ => unreachable!(),
        }
    }
    for operation in ["config", "drive", "grant"] {
        let fixture = fixture().await;
        let mut reply = HeldRuntimeReply::new();
        install_remote_fixture(
            &fixture,
            "preflight-runtime",
            &format!("http://{}", reply.address),
            false,
        );
        let context = server_api::ServerRequestContext {
            actor: if operation == "grant" {
                Some(RequestActor {
                    user_id: "fixture-user".into(),
                    account_id: format!("account-{WORKSPACE}"),
                    handle: "fixture-user".into(),
                    display_name: "Fixture user".into(),
                    auth_method: ActorAuthMethod::BrowserSession,
                })
            } else {
                None
            },
            worker_source: None,
            runtime_source: if operation == "grant" {
                None
            } else {
                Some(server_api::ServerRuntimeSource {
                    authentication_binding_id: "fixture-binding".into(),
                    runtime_id: "preflight-runtime".into(),
                    worker_id: Some("source-worker".into()),
                })
            },
            origin: None,
            transport_headers: Vec::new(),
        };
        let leases = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let api = fixture.api.clone();
        let request_context = context.clone();
        let task = tokio::spawn(crate::db_executor::with_operation_lifetime(
            Arc::new(CountJobLifetime(leases.clone())),
            async move { preflight(&api, &request_context, operation).await },
        ));
        let request = observed(reply.entered.take().unwrap()).await.unwrap();
        assert!(request.contains("/v1/workers/"), "{operation}: {request}");
        let store = fixture.store.clone();
        observed(
            fixture
                .executor
                .run(move || store.get_workspace_memory_settings(WORKSPACE)),
        )
        .await
        .unwrap();
        observed(tokio::time::sleep(StdDuration::from_millis(1))).await;
        assert_eq!(
            observed(tokio::spawn(async { "control" })).await.unwrap(),
            "control"
        );
        assert!(!reply.upstream_released.load(Ordering::SeqCst));
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert_eq!(leases.load(Ordering::SeqCst), 1);
        fixture.host_executor.dispatch(|| {}).unwrap();
        assert_eq!(
            observed(preflight(&fixture.api, &context, operation)).await,
            503,
            "{operation}: overload must not be authority denial"
        );
        reply.finish();
        observed(fixture.host_executor.drain()).await.unwrap();
        observed(fixture.executor.drain()).await.unwrap();
        assert_eq!(leases.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn ui_config_commit_publishes_after_commit_without_holding_the_sql_lane() {
    let fixture = fixture().await;
    let current = fixture
        .api
        .config_store
        .load_workspace_config(WORKSPACE)
        .unwrap()
        .unwrap();
    let mut reply = HeldRuntimeReply::new();
    install_remote_fixture(
        &fixture,
        "prompt-runtime",
        &format!("http://{}", reply.address),
        false,
    );
    let leases = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut commit = Box::pin(crate::db_executor::with_operation_lifetime(
        Arc::new(CountJobLifetime(leases.clone())),
        scoped_commit_workspace_config_tree(
            State(fixture.api.clone()),
            AxumPath(ScopedWorkspacePath {
                workspace_id: WORKSPACE.into(),
            }),
            Json(server_api::ConfigCommitRequest {
                base_digest: current.snapshot.digest.clone(),
                entrypoints: current
                    .contract
                    .entrypoints
                    .iter()
                    .map(ToString::to_string)
                    .collect(),
                changes: vec![server_api::ConfigTreeChange::Create {
                    path: "notes/owned-publication.txt".into(),
                    content_type: server_api::ConfigContentType::Text,
                    content: "committed before prompt publication".into(),
                }],
            }),
        ),
    ));
    assert!(futures::poll!(&mut commit).is_pending());
    let request = observed(reply.entered.take().unwrap()).await.unwrap();
    assert!(
        request.starts_with("POST "),
        "prompt publication: {request}"
    );
    let config_store = fixture.api.config_store.clone();
    let state = observed(
        fixture
            .executor
            .run(move || config_store.load_workspace_config(WORKSPACE)),
    )
    .await
    .unwrap()
    .unwrap();
    assert_ne!(
        state.snapshot.digest, current.snapshot.digest,
        "publication must observe the committed tree"
    );
    assert!(
        state
            .snapshot
            .entries
            .keys()
            .any(|path| path.to_string() == "notes/owned-publication.txt")
    );
    observed(tokio::time::sleep(StdDuration::from_millis(1))).await;
    assert_eq!(
        observed(tokio::spawn(async { "control" })).await.unwrap(),
        "control"
    );
    assert!(!reply.upstream_released.load(Ordering::SeqCst));
    drop(commit);
    assert_eq!(leases.load(Ordering::SeqCst), 1);
    reply.finish();
    observed(fixture.host_executor.drain()).await.unwrap();
    observed(fixture.executor.drain()).await.unwrap();
    assert_eq!(leases.load(Ordering::SeqCst), 0);
    assert_eq!(
        fixture
            .api
            .config_store
            .load_workspace_config(WORKSPACE)
            .unwrap()
            .unwrap()
            .snapshot
            .digest,
        state.snapshot.digest,
        "failed best-effort publication must not repeat or roll back the commit"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn repository_provider_reads_do_not_consume_the_sql_execution_queue() {
    let fixture = fixture().await;
    let (entered, waiting) = oneshot::channel();
    let (release, permit) = mpsc::channel();
    fixture
        .executor
        .dispatch(move || {
            let _ = entered.send(());
            let _ = permit.recv_timeout(WATCHDOG);
        })
        .unwrap();
    observed(waiting).await.unwrap();
    fixture.executor.dispatch(|| {}).unwrap();
    let probe = fixture.executor.run(|| Ok::<_, Error>(())).await;
    assert!(matches!(probe, Err(Error::DbCapacityExceeded)));
    // Empty catalog / absent key are deliberate boundary inputs: provider
    // reads keep their ordinary response even when SQL job admission is full.
    let list = observed(list_repositories(State(fixture.api.clone())))
        .await
        .unwrap();
    assert!(list.0.items.is_empty());
    let detail = observed(repository_detail(
        State(fixture.api.clone()),
        AxumPath("missing".into()),
    ))
    .await
    .unwrap_err();
    response_json(detail.into_response(), StatusCode::NOT_FOUND).await;
    let log = observed(repository_log(
        State(fixture.api.clone()),
        AxumPath("missing".into()),
        Query(LogQuery { limit: Some(1) }),
    ))
    .await
    .unwrap_err();
    response_json(log.into_response(), StatusCode::NOT_FOUND).await;
    let _ = release.send(());
    observed(fixture.executor.drain()).await.unwrap();
    observed(fixture.host_executor.drain()).await.unwrap();
}
