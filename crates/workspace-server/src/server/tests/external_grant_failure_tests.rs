//! Bearer-authenticated grant routes and generated Rust client against isolated
//! SQLite and the shared signed Runtime HTTP harness. Only Worker execution is
//! deterministic; cleanup admission, Runtime receipts, CAS and storage are real.
//! Explicit legacy rows and SQL triggers exercise compatibility and failed writes.
use super::*;
use api_macros::reqwest::ClientError;
use server_api::{ServerApiClient, WorkdirManagementApiError};
use workdir::management_failure::{
    WorkdirManagementFailure, WorkdirManagementOperation as Op, WorkdirManagementReason as Reason,
    WorkdirManagementVisibility,
};

struct GrantCleanupFixture {
    _source: super::ticket_worker_launch_tests::LaunchFixture,
    api: WorkspaceApi,
    worker: RuntimeWorkerRef,
    workdir_id: String,
}

async fn grant_cleanup_fixture() -> GrantCleanupFixture {
    let (source, binding) =
        super::ticket_worker_launch_tests::LaunchFixture::with_source_workdir().await;
    let runtime_id = source
        .api
        .store
        .get_workdir_registry(TEST_WORKSPACE_ID, binding.working_directory.id.as_str())
        .unwrap()
        .unwrap()
        .source
        .runtime_id()
        .unwrap()
        .to_owned();
    let workdir_id = binding.working_directory.id.clone();
    drop(binding);
    source.publish_profile("builtin:companion");
    let Json(created) = create_workspace_worker(
        State(source.api.clone()),
        HeaderMap::new(),
        Json(CreateWorkspaceWorkerRequest {
            runtime_id,
            display_name: "grant cleanup Worker".into(),
            singleton_key: None,
            profile: Some("builtin:companion".into()),
            ticket_assignment: None,
            initial_submit: Vec::new(),
            workdir_attachments: Vec::new(),
            feature_connections: Default::default(),
            control_operation_id: None,
            execution_grant: None,
        }),
    )
    .await
    .unwrap();
    let worker = RuntimeWorkerRef::new(created.runtime_id, created.worker_id);
    source.seed_remote_session_for_cleanup(&worker);
    GrantCleanupFixture {
        api: source.api.clone(),
        _source: source,
        worker,
        workdir_id,
    }
}

// Old saved rows have no never-dispatched authority. Seed their original shape
// directly, rather than using the new writer to manufacture a legacy operation.
fn seed_legacy_cleanup(api: &WorkspaceApi, grant: &str, state: &str, raw: Option<&str>) {
    api.config_store.with_conn(|conn| {
        assert_eq!(conn.execute(
            "UPDATE external_workdir_grants SET cleanup_state=?3,cleanup_error=?4 WHERE workspace_id=?1 AND grant_id=?2 AND status IN ('revoked','expired')",
            rusqlite::params![TEST_WORKSPACE_ID, grant, state, raw],
        )?, 1);
        Ok(())
    }).unwrap();
}

async fn client(
    api: &WorkspaceApi,
) -> (
    ServerApiClient,
    tokio::task::JoinSet<()>,
    String,
    reqwest::Client,
) {
    let token = seed_test_api_token(api.store.as_ref(), TEST_WORKSPACE_ID);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let router = build_router(api.clone());
    let mut tasks = tokio::task::JoinSet::new();
    tasks.spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::AUTHORIZATION,
        format!("Bearer {token}").parse().unwrap(),
    );
    let http = reqwest::Client::builder()
        .default_headers(headers)
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .unwrap();
    (
        ServerApiClient::with_client(
            format!("http://{address}").as_str().try_into().unwrap(),
            http.clone(),
        ),
        tasks,
        format!("http://{address}"),
        http,
    )
}

fn request() -> ExternalWorkdirGrantCreateRequest {
    ExternalWorkdirGrantCreateRequest {
        provider_instance_id: "grant-failure-provider".into(),
        display_name: "External fixture".into(),
        ttl_seconds: None,
        permissions: server_api::ExternalWorkdirPermissions::READ,
    }
}

fn cause(
    error: ClientError<WorkdirManagementApiError>,
    operation: Op,
    reason: Reason,
    status: u16,
) {
    let ClientError::Public {
        status: actual,
        error,
    } = error
    else {
        panic!("lost generated typed decoder: {error:?}")
    };
    assert_eq!(actual.as_u16(), status);
    assert_eq!(
        error.details(),
        Some(&WorkdirManagementFailure { operation, reason })
    );
    let tool = error_contract::project_failure::<error_contract::Tool, _>(
        &error_contract::FailureFacts::new(*error.details().unwrap()).unwrap(),
        &WorkdirManagementVisibility {
            can_view_cause: true,
        },
    )
    .unwrap();
    assert!(tool.explanation().contains(reason.explanation()));
    assert!(!tool.explanation().contains("/private/"));
}

type ProviderSocket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn register_provider(
    _api: &WorkspaceApi,
    base: &str,
    grant: &ExternalWorkdirGrantResponse,
) -> ProviderSocket {
    use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest};
    // Reuse the existing client account/user, not a second user with that account.
    let token = format!("api-token-{TEST_WORKSPACE_ID}");
    let url = format!(
        "{}/api/w/{TEST_WORKSPACE_ID}/external-workdir-grants/{}/provider",
        base.replacen("http://", "ws://", 1),
        grant.grant_id
    );
    let mut request = url.into_client_request().unwrap();
    request
        .headers_mut()
        .insert("Authorization", format!("Bearer {token}").parse().unwrap());
    let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    let registration =
        ExternalWorkdirProviderFrame::current(ExternalWorkdirProviderMessage::Register {
            registration: workdir::external::ExternalWorkdirProviderRegistration {
                provider_instance_id: ExternalProviderInstanceId::new(
                    grant.provider_instance_id.clone(),
                )
                .unwrap(),
                grant_id: workdir::external::ExternalWorkdirGrantId::new(grant.grant_id.clone())
                    .unwrap(),
                workdir_id: workdir::Workdir::new(&grant.working_directory_id)
                    .id()
                    .clone(),
                generation: grant.generation,
                capabilities: workdir::WorkdirSessionCapabilities::READ_ONLY,
                read_limits: workdir::BoundedReadLimits::EXTERNAL_DEFAULT,
            },
        });
    socket
        .send(Message::Text(
            serde_json::to_string(&registration).unwrap().into(),
        ))
        .await
        .unwrap();
    socket
}

async fn registered(socket: &mut ProviderSocket) {
    let message = tokio::time::timeout(std::time::Duration::from_secs(5), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap()
        .into_text()
        .unwrap();
    assert!(matches!(
        serde_json::from_str::<ExternalWorkdirServerFrame>(&message)
            .unwrap()
            .message,
        ExternalWorkdirServerMessage::Registered { .. }
    ));
}

async fn observed_connection(
    client: &ServerApiClient,
    grant: &str,
    count: usize,
) -> ExternalWorkdirGrantResponse {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let response = client
                .external_workdir_grant_get(TEST_WORKSPACE_ID.into(), grant.into())
                .await
                .unwrap();
            if response.connection_observations.len() >= count {
                break response;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("connection evidence was not published through owner GET")
}

fn connection_reasons(response: &ExternalWorkdirGrantResponse) -> Vec<Reason> {
    response
        .connection_observations
        .iter()
        .flat_map(|observation| &observation.failures)
        .filter_map(|failure| {
            failure
                .project_for::<error_contract::Tool, WorkdirManagementFailure>(
                    &WorkdirManagementVisibility {
                        can_view_cause: true,
                    },
                )
                .ok()
        })
        .map(|failure| {
            let details = failure.details();
            if matches!(
                details.reason,
                Reason::GrantAttachmentReleaseFailed
                    | Reason::GrantCleanupPersistenceFailed
                    | Reason::GrantCleanupOutcomeUnknown
            ) {
                assert_eq!(details.operation, Op::GrantCleanup);
            } else if details.reason == Reason::GrantAuditFailed {
                assert!(matches!(
                    details.operation,
                    Op::GrantConnect | Op::GrantCleanup
                ));
            } else {
                assert_eq!(details.operation, Op::GrantConnect);
            }
            failure.details().reason
        })
        .collect()
}

#[tokio::test]
async fn external_grant_failure_routes_keep_operations_and_reject_without_effects() {
    let dir = tempfile::tempdir().unwrap();
    init_clean_git_workspace(dir.path());
    let api = test_api(dir.path()).await;
    let (client, _tasks, ..) = client(&api).await;
    let mut invalid = request();
    invalid.display_name.clear();
    cause(
        client
            .external_workdir_grant_create(TEST_WORKSPACE_ID.into(), invalid)
            .await
            .unwrap_err(),
        Op::GrantCreate,
        Reason::GrantDisplayNameInvalid,
        400,
    );
    cause(
        client
            .external_workdir_grant_get(TEST_WORKSPACE_ID.into(), "missing".into())
            .await
            .unwrap_err(),
        Op::GrantInspect,
        Reason::GrantNotFound,
        400,
    );
    cause(
        client
            .external_workdir_grant_revoke(TEST_WORKSPACE_ID.into(), "missing".into())
            .await
            .unwrap_err(),
        Op::GrantRevoke,
        Reason::GrantNotFound,
        400,
    );
    assert!(
        api.store
            .list_external_workdir_grants(TEST_WORKSPACE_ID)
            .unwrap()
            .is_empty()
    );

    let grant = client
        .external_workdir_grant_create(TEST_WORKSPACE_ID.into(), request())
        .await
        .unwrap();
    api.config_store.with_conn(|conn| {
        conn.execute("UPDATE external_workdir_grants SET created_by = 'another-private-account' WHERE grant_id = ?1", [&grant.grant_id])?;
        Ok(())
    }).unwrap();
    cause(
        client
            .external_workdir_grant_get(TEST_WORKSPACE_ID.into(), grant.grant_id.clone())
            .await
            .unwrap_err(),
        Op::GrantInspect,
        Reason::GrantCreatorRequired,
        403,
    );
    cause(
        client
            .external_workdir_grant_revoke(TEST_WORKSPACE_ID.into(), grant.grant_id.clone())
            .await
            .unwrap_err(),
        Op::GrantRevoke,
        Reason::GrantCreatorRequired,
        403,
    );
    assert_eq!(
        api.store
            .get_external_workdir_grant(TEST_WORKSPACE_ID, &grant.grant_id)
            .unwrap()
            .unwrap()
            .status,
        "pending"
    );
}

#[tokio::test]
async fn external_grant_failure_audit_does_not_block_fence_release_or_saved_causes() {
    let fixture = grant_cleanup_fixture().await;
    let api = &fixture.api;
    let (client, _tasks, ..) = client(api).await;
    let grant = client
        .external_workdir_grant_create(TEST_WORKSPACE_ID.into(), request())
        .await
        .unwrap();
    api.store
        .attach_worker_workdir(&WorkerWorkdirLinkRecord {
            connection_id: String::new(),
            workspace_id: TEST_WORKSPACE_ID.into(),
            worker: fixture.worker.clone(),
            workdir_id: grant.working_directory_id.clone(),
            alias: "external-failure".into(),
            capabilities: workdir::WorkdirSessionCapabilities::READ_ONLY,
            linked_at: Utc::now().to_rfc3339(),
            unlinked_at: None,
        })
        .unwrap();
    inject_external_workdir_cleanup_failure(&grant.working_directory_id);
    api.config_store.with_conn(|conn| {
        conn.execute_batch("CREATE TRIGGER fail_grant_audit BEFORE INSERT ON audit_events
            WHEN NEW.action IN ('external_workdir_grant_revoked','external_workdir_cleanup_attention_required')
            BEGIN SELECT RAISE(FAIL, '/private/provider/audit unavailable'); END;")?;
        Ok(())
    }).unwrap();
    let response = client
        .external_workdir_grant_revoke(TEST_WORKSPACE_ID.into(), grant.grant_id.clone())
        .await
        .unwrap();
    assert_eq!(response.status, "revoked");
    assert_eq!(response.cleanup_state, "pending");
    assert!(
        api.store
            .list_workdir_worker_links(TEST_WORKSPACE_ID, &grant.working_directory_id)
            .unwrap()
            .iter()
            .all(|link| link.unlinked_at.is_some()),
        "fence audit failure must not skip actual detach"
    );
    let reasons = response
        .cleanup_failures
        .iter()
        .map(|failure| {
            failure
                .project_for::<error_contract::Tool, WorkdirManagementFailure>(
                    &WorkdirManagementVisibility {
                        can_view_cause: true,
                    },
                )
                .unwrap()
                .details()
                .reason
        })
        .collect::<Vec<_>>();
    assert!(reasons.contains(&Reason::GrantAttachmentReleaseFailed));
    assert!(reasons.contains(&Reason::GrantAuditFailed));
    let (state, stored) = api
        .store
        .external_workdir_cleanup_observation(TEST_WORKSPACE_ID, &grant.grant_id)
        .unwrap();
    assert_eq!(state, "pending");
    let saved = stored
        .iter()
        .filter_map(external_grant_failure::publish)
        .collect::<Vec<_>>();
    assert_eq!(saved, response.cleanup_failures);
    assert!(
        !serde_json::to_string(&response)
            .unwrap()
            .contains("/private/")
    );
    assert_eq!(
        api.store
            .get_external_workdir_grant(TEST_WORKSPACE_ID, &grant.grant_id)
            .unwrap()
            .unwrap()
            .status,
        "revoked"
    );
}

#[tokio::test]
async fn external_grant_failure_observation_write_does_not_turn_committed_revoke_into_unknown() {
    let dir = tempfile::tempdir().unwrap();
    init_clean_git_workspace(dir.path());
    let api = test_api(dir.path()).await;
    let (client, _tasks, ..) = client(&api).await;
    let grant = client
        .external_workdir_grant_create(TEST_WORKSPACE_ID.into(), request())
        .await
        .unwrap();
    api.config_store.with_conn(|conn| {
        conn.execute_batch("CREATE TRIGGER fail_cleanup_observation BEFORE UPDATE OF cleanup_state ON external_workdir_grants
            WHEN OLD.status IN ('revoked','expired')
            BEGIN SELECT RAISE(FAIL, '/private/database/observation unavailable'); END;")?;
        Ok(())
    }).unwrap();
    let response = client
        .external_workdir_grant_revoke(TEST_WORKSPACE_ID.into(), grant.grant_id.clone())
        .await
        .unwrap();
    assert_eq!(response.status, "revoked");
    assert_eq!(response.cleanup_state, "pending");
    assert!(!response.cleanup_failures.is_empty());
    for failure in &response.cleanup_failures {
        let projected = failure
            .project_for::<error_contract::Tool, WorkdirManagementFailure>(
                &WorkdirManagementVisibility {
                    can_view_cause: true,
                },
            )
            .unwrap();
        assert_eq!(
            projected.details().reason,
            Reason::GrantCleanupPersistenceFailed
        );
    }
    let inspected = client
        .external_workdir_grant_get(TEST_WORKSPACE_ID.into(), grant.grant_id.clone())
        .await
        .unwrap();
    assert_eq!(inspected.status, "revoked");
    assert!(
        inspected.cleanup_failures.is_empty(),
        "failed evidence persistence must not be invented by GET"
    );
    assert!(
        !serde_json::to_string(&inspected)
            .unwrap()
            .contains("/private/")
    );
    assert_eq!(
        api.store
            .list_external_workdir_grants(TEST_WORKSPACE_ID)
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn external_grant_failure_websocket_admission_uses_grant_connection_cause() {
    let dir = tempfile::tempdir().unwrap();
    init_clean_git_workspace(dir.path());
    let api = test_api(dir.path()).await;
    let (_, _tasks, base, http) = client(&api).await;
    let response = http
        .get(format!(
            "{base}/api/w/{TEST_WORKSPACE_ID}/external-workdir-grants/missing/provider"
        ))
        .header("Connection", "upgrade")
        .header("Upgrade", "websocket")
        .header("Sec-WebSocket-Version", "13")
        .header("Sec-WebSocket-Key", "dGhlIHNhbXBsZSBub25jZQ==")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 400);
    let media = response.headers()[reqwest::header::CONTENT_TYPE]
        .to_str()
        .unwrap()
        .to_string();
    assert_eq!(media, error_contract::problem::PROBLEM_CONTENT_TYPE);
    let body = response.bytes().await.unwrap();
    let decoded = WorkdirManagementApiError::decode_received(400, &media, &body).unwrap();
    assert_eq!(
        decoded.details(),
        Some(&WorkdirManagementFailure {
            operation: Op::GrantConnect,
            reason: Reason::GrantNotFound,
        })
    );
    assert!(
        api.store
            .list_external_workdir_grants(TEST_WORKSPACE_ID)
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn external_grant_failure_saved_legacy_and_corrupt_evidence_never_manufactures_cause() {
    let dir = tempfile::tempdir().unwrap();
    init_clean_git_workspace(dir.path());
    let api = test_api(dir.path()).await;
    let (client, _tasks, ..) = client(&api).await;
    let grant = client
        .external_workdir_grant_create(TEST_WORKSPACE_ID.into(), request())
        .await
        .unwrap();
    assert!(
        api.store
            .update_external_workdir_grant_state(
                TEST_WORKSPACE_ID,
                &grant.grant_id,
                &grant.provider_instance_id,
                grant.generation,
                "revoked",
                &Utc::now().to_rfc3339()
            )
            .unwrap()
    );
    for raw in [
        "attachment_release_failed",
        "[\"/private/provider/raw cause\"]",
    ] {
        seed_legacy_cleanup(&api, &grant.grant_id, "retry_required", Some(raw));
        let record = api
            .store
            .get_external_workdir_grant(TEST_WORKSPACE_ID, &grant.grant_id)
            .unwrap()
            .unwrap();
        let response = external_workdir_grant_response(&api, &record).unwrap();
        assert_eq!(response.status, "revoked");
        assert_eq!(response.cleanup_state, "retry_required");
        assert!(response.cleanup_failures.is_empty());
        assert!(
            !serde_json::to_string(&response)
                .unwrap()
                .contains("/private/")
        );
    }
}

#[tokio::test]
async fn external_grant_completed_cleanup_inspection_keeps_audit_without_repeating_release() {
    let fixture = grant_cleanup_fixture().await;
    let api = &fixture.api;
    let (client, _tasks, ..) = client(api).await;
    let grant = client
        .external_workdir_grant_create(TEST_WORKSPACE_ID.into(), request())
        .await
        .unwrap();
    api.store
        .attach_worker_workdir(&WorkerWorkdirLinkRecord {
            connection_id: String::new(),
            workspace_id: TEST_WORKSPACE_ID.into(),
            worker: fixture.worker.clone(),
            workdir_id: grant.working_directory_id.clone(),
            alias: "completed-external".into(),
            capabilities: workdir::WorkdirSessionCapabilities::READ_ONLY,
            linked_at: Utc::now().to_rfc3339(),
            unlinked_at: None,
        })
        .unwrap();
    let now = Utc::now().to_rfc3339();
    api.config_store
        .with_conn(|conn| {
            conn.execute_batch(
                "CREATE TRIGGER fail_completed_audit BEFORE INSERT ON audit_events
            WHEN NEW.action='external_workdir_cleanup_completed'
            BEGIN SELECT RAISE(FAIL, '/private/audit after confirmed release'); END;",
            )?;
            Ok(())
        })
        .unwrap();
    let first = client
        .external_workdir_grant_revoke(TEST_WORKSPACE_ID.into(), grant.grant_id.clone())
        .await
        .unwrap();
    assert_eq!(first.cleanup_state, "completed");
    let owner = api
        .store
        .external_workdir_cleanup_operation(TEST_WORKSPACE_ID, &grant.grant_id)
        .unwrap()
        .unwrap();
    assert!(owner.operation().settled);
    assert!(owner.operation().runtime_dispatched);
    assert_eq!(
        owner.operation().runtime_status,
        Some(crate::hosts::RuntimeOperationStatus::Accepted)
    );
    let saved = api
        .store
        .external_workdir_cleanup_observation(TEST_WORKSPACE_ID, &grant.grant_id)
        .unwrap();
    let published = saved
        .1
        .iter()
        .filter_map(external_grant_failure::publish)
        .collect::<Vec<_>>();
    assert_eq!(published.len(), 1);
    let reconciliations =
        external_workdir_cleanup_reconciliation_count(&grant.working_directory_id);
    for repeat_revoke in [false, true, false] {
        let response = if repeat_revoke {
            client
                .external_workdir_grant_revoke(TEST_WORKSPACE_ID.into(), grant.grant_id.clone())
                .await
                .unwrap()
        } else {
            client
                .external_workdir_grant_get(TEST_WORKSPACE_ID.into(), grant.grant_id.clone())
                .await
                .unwrap()
        };
        assert_eq!(response.status, "revoked");
        assert_eq!(response.cleanup_state, "completed");
        assert_eq!(response.cleanup_failures, published);
        assert!(
            !serde_json::to_string(&response)
                .unwrap()
                .contains("/private/")
        );
        assert_eq!(
            api.store
                .external_workdir_cleanup_observation(TEST_WORKSPACE_ID, &grant.grant_id)
                .unwrap(),
            saved,
        );
        assert_eq!(
            external_workdir_cleanup_reconciliation_count(&grant.working_directory_id),
            reconciliations,
            "a completed release is not permission to issue another Runtime replacement",
        );
    }
    assert!(
        api.store
            .reconcile_external_workdir_grants_after_restart(TEST_WORKSPACE_ID, &now)
            .unwrap()
            .is_empty(),
        "restart must not schedule completed release again",
    );
}

#[tokio::test]
async fn external_grant_inspections_are_read_only_while_cleanup_continuations_serialize() {
    let fixture = grant_cleanup_fixture().await;
    let api = &fixture.api;
    let (client, _server, ..) = client(api).await;
    let client = Arc::new(client);
    let grant = client
        .external_workdir_grant_create(TEST_WORKSPACE_ID.into(), request())
        .await
        .unwrap();
    api.store
        .attach_worker_workdir(&WorkerWorkdirLinkRecord {
            connection_id: String::new(),
            workspace_id: TEST_WORKSPACE_ID.into(),
            worker: fixture.worker.clone(),
            workdir_id: grant.working_directory_id.clone(),
            alias: "queued-external".into(),
            capabilities: workdir::WorkdirSessionCapabilities::READ_ONLY,
            linked_at: Utc::now().to_rfc3339(),
            unlinked_at: None,
        })
        .unwrap();
    let now = Utc::now().to_rfc3339();
    assert!(
        api.store
            .update_external_workdir_grant_state(
                TEST_WORKSPACE_ID,
                &grant.grant_id,
                &grant.provider_instance_id,
                grant.generation,
                "revoked",
                &now,
            )
            .unwrap()
    );
    api.config_store
        .with_conn(|conn| {
            conn.execute_batch(
                "CREATE TRIGGER fail_queued_cleanup_audit BEFORE INSERT ON audit_events
             WHEN NEW.action='external_workdir_cleanup_completed'
             BEGIN SELECT RAISE(FAIL, '/private/queued/audit secret'); END;",
            )?;
            Ok(())
        })
        .unwrap();

    let lock = current_worker_session_lock(api, &fixture.worker);
    let held = lock.clone().lock_owned().await;
    let holders = Arc::strong_count(&lock);
    let reconciliations =
        external_workdir_cleanup_reconciliation_count(&grant.working_directory_id);
    let saved_before = api
        .store
        .external_workdir_cleanup_observation(TEST_WORKSPACE_ID, &grant.grant_id)
        .unwrap();
    let mut inspections = tokio::task::JoinSet::new();
    for _ in 0..2 {
        let client = client.clone();
        let grant_id = grant.grant_id.clone();
        inspections.spawn(async move {
            client
                .external_workdir_grant_get(TEST_WORKSPACE_ID.into(), grant_id)
                .await
                .unwrap()
        });
    }
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while let Some(response) = inspections.join_next().await {
            assert_eq!(response.unwrap().cleanup_state, "pending");
        }
    })
    .await
    .expect("inspection must not wait on or drive cleanup");
    assert_eq!(
        api.store
            .external_workdir_cleanup_observation(TEST_WORKSPACE_ID, &grant.grant_id)
            .unwrap(),
        saved_before
    );
    assert_eq!(Arc::strong_count(&lock), holders);
    assert_eq!(
        external_workdir_cleanup_reconciliation_count(&grant.working_directory_id),
        reconciliations
    );
    let mut requests = tokio::task::JoinSet::new();
    for _ in 0..2 {
        let api = api.clone();
        let workdir = grant.working_directory_id.clone();
        requests.spawn(async move { cleanup_external_workdir_attachments(&api, &workdir).await });
    }
    // Explicit cleanup continuations must reach the ordinary Worker lock.
    // No sleep or presumed DB scheduling establishes this race.
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while Arc::strong_count(&lock) < holders + 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("both continuations must queue at the Worker lock");
    drop(held);
    let results = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let mut results = Vec::new();
        while let Some(result) = requests.join_next().await {
            results.push(result.unwrap());
        }
        results
    })
    .await
    .unwrap();
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(results.iter().filter(|result| result.is_err()).count(), 1);
    let mut responses = Vec::new();
    for _ in 0..2 {
        responses.push(
            client
                .external_workdir_grant_get(TEST_WORKSPACE_ID.into(), grant.grant_id.clone())
                .await
                .unwrap(),
        );
    }
    assert_eq!(responses.len(), 2);
    for response in &responses {
        assert_eq!(response.status, "revoked");
        assert_eq!(response.cleanup_state, "completed");
        assert_eq!(response.cleanup_failures.len(), 1);
        let audit = response.cleanup_failures[0]
            .project_for::<error_contract::Tool, WorkdirManagementFailure>(
                &WorkdirManagementVisibility {
                    can_view_cause: true,
                },
            )
            .unwrap();
        assert_eq!(audit.details().reason, Reason::GrantAuditFailed);
        assert!(
            !serde_json::to_string(response)
                .unwrap()
                .contains("/private/")
        );
    }
    assert_eq!(responses[0].cleanup_failures, responses[1].cleanup_failures);
    assert_eq!(
        external_workdir_cleanup_reconciliation_count(&grant.working_directory_id),
        reconciliations + 1,
        "queued continuation must reread owner completion before Runtime dispatch",
    );
    let saved = api
        .store
        .external_workdir_cleanup_observation(TEST_WORKSPACE_ID, &grant.grant_id)
        .unwrap();
    assert_eq!(saved.0, "completed");
    assert_eq!(
        saved
            .1
            .iter()
            .filter_map(external_grant_failure::publish)
            .collect::<Vec<_>>(),
        responses[0].cleanup_failures
    );
    assert!(
        api.store
            .reconcile_external_workdir_grants_after_restart(TEST_WORKSPACE_ID, &now)
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn external_grant_legacy_uncertainty_retains_release_and_audit_after_database_reopen() {
    let dir = tempfile::tempdir().unwrap();
    init_clean_git_workspace(dir.path());
    let api = test_api(dir.path()).await;
    let (client, _server, ..) = client(&api).await;
    let grant = client
        .external_workdir_grant_create(TEST_WORKSPACE_ID.into(), request())
        .await
        .unwrap();
    let now = Utc::now().to_rfc3339();
    assert!(
        api.store
            .update_external_workdir_grant_state(
                TEST_WORKSPACE_ID,
                &grant.grant_id,
                &grant.provider_instance_id,
                grant.generation,
                "revoked",
                &now,
            )
            .unwrap()
    );
    let previous = external_grant_failure::combine(
        external_grant_failure::attributed(
            Error::Store("/private/previous/release".into()),
            Reason::GrantAttachmentReleaseFailed,
        ),
        external_grant_failure::attributed(
            Error::Store("/private/previous/audit".into()),
            Reason::GrantAuditFailed,
        ),
    );
    let raw = external_grant_failure::encode(&external_grant_failure::stored_error(
        &previous,
        Reason::GrantAttachmentReleaseFailed,
    ))
    .unwrap();
    seed_legacy_cleanup(&api, &grant.grant_id, "retry_required", raw.as_deref());
    // No current Worker is not a receipt for an old dispatch. Neither GET nor
    // an explicit continuation may retroactively adopt this legacy uncertainty.
    cleanup_external_workdir_attachments(&api, &grant.working_directory_id)
        .await
        .unwrap();
    let response = client
        .external_workdir_grant_get(TEST_WORKSPACE_ID.into(), grant.grant_id.clone())
        .await
        .unwrap();
    assert_eq!(response.cleanup_state, "retry_required");
    assert_eq!(response.cleanup_failures.len(), 2);
    assert_eq!(
        response.cleanup_failures[1]
            .project_for::<error_contract::Tool, WorkdirManagementFailure>(
                &WorkdirManagementVisibility {
                    can_view_cause: true
                },
            )
            .unwrap()
            .details()
            .reason,
        Reason::GrantAuditFailed
    );
    let db_path: String = api
        .config_store
        .with_conn(|conn| {
            Ok(conn.query_row(
                "SELECT file FROM pragma_database_list WHERE name='main'",
                [],
                |row| row.get(0),
            )?)
        })
        .unwrap();
    assert!(
        !db_path.is_empty(),
        "this contract requires file-backed SQLite"
    );
    let reopened = crate::SqliteWorkspaceStore::open(db_path).unwrap();
    let (state, failures) = reopened
        .external_workdir_cleanup_observation(TEST_WORKSPACE_ID, &grant.grant_id)
        .unwrap();
    assert_eq!(state, "retry_required");
    assert_eq!(
        failures
            .iter()
            .filter_map(external_grant_failure::publish)
            .collect::<Vec<_>>(),
        response.cleanup_failures
    );
    let scheduled = reopened
        .reconcile_external_workdir_grants_after_restart(TEST_WORKSPACE_ID, &now)
        .unwrap();
    assert_eq!(scheduled, vec![grant.working_directory_id.clone()]);
    for workdir in scheduled {
        cleanup_external_workdir_attachments(&api, &workdir)
            .await
            .unwrap();
    }
    assert_eq!(
        reopened
            .external_workdir_cleanup_observation(TEST_WORKSPACE_ID, &grant.grant_id)
            .unwrap()
            .0,
        "retry_required"
    );
    assert_eq!(
        external_workdir_cleanup_reconciliation_count(&grant.working_directory_id),
        0
    );
    let reread = client
        .external_workdir_grant_get(TEST_WORKSPACE_ID.into(), grant.grant_id.clone())
        .await
        .unwrap();
    assert_eq!(reread.cleanup_failures, response.cleanup_failures);
    assert!(
        !serde_json::to_string(&reread)
            .unwrap()
            .contains("/private/")
    );
}

#[tokio::test]
async fn external_grant_owned_unknown_survives_reopen_without_get_or_restart_redispatch() {
    use crate::grant_cleanup_operation::{CleanupAdmission, CleanupRequest, CleanupRuntimeTarget};
    use crate::hosts::{RuntimeOperationResponse, RuntimeOperationStatus as Status};
    use protocol::control_failure::RuntimeControlCause;
    let fixture = grant_cleanup_fixture().await;
    let api = &fixture.api;
    let (client, _server, ..) = client(api).await;
    let grant = client
        .external_workdir_grant_create(TEST_WORKSPACE_ID.into(), request())
        .await
        .unwrap();
    api.store
        .attach_worker_workdir(&WorkerWorkdirLinkRecord {
            connection_id: String::new(),
            workspace_id: TEST_WORKSPACE_ID.into(),
            worker: fixture.worker.clone(),
            workdir_id: grant.working_directory_id.clone(),
            alias: "owned-unknown".into(),
            capabilities: workdir::WorkdirSessionCapabilities::READ_ONLY,
            linked_at: Utc::now().to_rfc3339(),
            unlinked_at: None,
        })
        .unwrap();
    let link = api
        .store
        .latest_workdir_worker_link(TEST_WORKSPACE_ID, &grant.working_directory_id)
        .unwrap()
        .unwrap();
    let now = Utc::now().to_rfc3339();
    assert!(
        api.store
            .update_external_workdir_grant_state(
                TEST_WORKSPACE_ID,
                &grant.grant_id,
                &grant.provider_instance_id,
                grant.generation,
                "revoked",
                &now,
            )
            .unwrap()
    );
    let request = CleanupRequest {
        operation_id: Uuid::now_v7().to_string(),
        workspace_id: TEST_WORKSPACE_ID.into(),
        grant_id: grant.grant_id.clone(),
        provider_instance_id: grant.provider_instance_id.clone(),
        grant_generation: grant.generation,
        working_directory_id: grant.working_directory_id.clone(),
        target: Some(CleanupRuntimeTarget {
            worker: fixture.worker.clone(),
            connection_id: link.connection_id.clone(),
            alias: link.alias.clone(),
            expected_generation: "fixture-incarnation-generation".into(),
            logical_workdir_attachments: api
                .store
                .list_worker_workdir_links(TEST_WORKSPACE_ID, &fixture.worker)
                .unwrap()
                .into_iter()
                .filter(|link| link.workdir_id != grant.working_directory_id)
                .map(|link| LogicalWorkdirAttachment {
                    alias: workdir::WorkdirAttachmentAlias::new(link.alias).unwrap(),
                    working_directory_id: link.workdir_id,
                    capabilities: link.capabilities,
                })
                .collect(),
        }),
    };
    // The real fence writer persisted never-dispatched intent in its transaction.
    // Runtime's response boundary below is substituted: this test owns Store CAS,
    // while the sibling caller tests use the actual authenticated Runtime HTTP.
    assert!(
        api.store
            .external_workdir_cleanup_has_intent(TEST_WORKSPACE_ID, &grant.grant_id)
            .unwrap()
    );
    let CleanupAdmission::Admitted(admitted) = api
        .store
        .begin_external_workdir_cleanup_operation(&request, &[], &now)
        .unwrap()
    else {
        panic!("fresh fence must admit exactly one owner")
    };
    assert!(matches!(
        api.store
            .begin_external_workdir_cleanup_operation(&request, &[], &now)
            .unwrap(),
        CleanupAdmission::Existing(_)
    ));
    let mut changed = request.clone();
    changed.target.as_mut().unwrap().expected_generation = "another-generation".into();
    assert!(matches!(
        api.store
            .begin_external_workdir_cleanup_operation(&changed, &[], &now),
        Err(Error::WorkdirAttachmentConflict(_))
    ));
    api.config_store.with_conn(|conn| {
        conn.execute_batch("CREATE TRIGGER fail_owned_release_checkpoint BEFORE UPDATE OF cleanup_error ON external_workdir_grants
            BEGIN SELECT RAISE(ABORT, '/private/owned/checkpoint'); END;")?;
        Ok(())
    }).unwrap();
    assert!(matches!(
        api.store
            .confirm_external_workdir_cleanup_release(&admitted, &now),
        Err(Error::Sqlite(_))
    ));
    assert!(
        api.store
            .latest_workdir_worker_link(TEST_WORKSPACE_ID, &grant.working_directory_id)
            .unwrap()
            .unwrap()
            .unlinked_at
            .is_none()
    );
    assert_eq!(
        api.store
            .external_workdir_cleanup_operation(TEST_WORKSPACE_ID, &grant.grant_id)
            .unwrap()
            .unwrap()
            .operation(),
        admitted.operation()
    );
    api.config_store
        .with_conn(|conn| {
            conn.execute_batch("DROP TRIGGER fail_owned_release_checkpoint")?;
            Ok(())
        })
        .unwrap();
    let released = api
        .store
        .confirm_external_workdir_cleanup_release(&admitted, &now)
        .unwrap();
    let dispatched = api
        .store
        .mark_external_workdir_cleanup_runtime_dispatched(&released, &now)
        .unwrap();
    let primary = ApiError::from(Error::WorkdirAttachmentSyncFailed {
        runtime_id: fixture.worker.runtime_id.clone(),
        observation: Box::new(RuntimeOperationResponse {
            status: Status::Unknown,
            cause: Some(RuntimeControlCause::PersistenceFailed),
            worker: None,
            diagnostics: Vec::new(),
        }),
    });
    let failure = external_grant_failure::combine(
        primary,
        external_grant_failure::attributed(
            Error::Store("/private/owned/audit secret".into()),
            Reason::GrantAuditFailed,
        ),
    );
    let failures =
        external_grant_failure::stored_error(&failure, Reason::GrantAttachmentReleaseFailed);
    let observed = api
        .store
        .record_external_workdir_cleanup_runtime_observation(
            &dispatched,
            Status::Unknown,
            &failures,
            &now,
        )
        .unwrap();
    assert_eq!(observed.operation().runtime_status, Some(Status::Unknown));
    assert!(matches!(
        api.store
            .record_external_workdir_cleanup_runtime_observation(
                &observed,
                Status::Rejected,
                &[],
                &now,
            ),
        Err(Error::WorkdirAttachmentConflict(_))
    ));
    assert!(matches!(
        api.store
            .record_external_workdir_cleanup_runtime_observation(
                &dispatched,
                Status::Rejected,
                &[],
                &now,
            ),
        Err(Error::WorkdirAttachmentConflict(_))
    ));
    assert!(matches!(
        api.store.update_external_workdir_cleanup_state(
            TEST_WORKSPACE_ID,
            &grant.grant_id,
            "pending",
            None,
            &now,
        ),
        Err(Error::WorkdirAttachmentConflict(_))
    ));
    let before = external_workdir_cleanup_reconciliation_count(&grant.working_directory_id);
    let mut current = observed.clone();
    for repeat in [false, true, false] {
        let response = if repeat {
            client
                .external_workdir_grant_revoke(TEST_WORKSPACE_ID.into(), grant.grant_id.clone())
                .await
                .unwrap()
        } else {
            client
                .external_workdir_grant_get(TEST_WORKSPACE_ID.into(), grant.grant_id.clone())
                .await
                .unwrap()
        };
        assert_eq!(response.cleanup_state, "pending");
        assert!(response.cleanup_failures.len() >= 2);
        assert!(
            !serde_json::to_string(&response)
                .unwrap()
                .contains("/private/")
        );
        let reread = api
            .store
            .external_workdir_cleanup_operation(TEST_WORKSPACE_ID, &grant.grant_id)
            .unwrap()
            .unwrap();
        if !repeat {
            assert_eq!(reread.operation(), current.operation(), "GET is read-only");
        }
        assert_eq!(reread.operation().request, observed.operation().request);
        assert_eq!(reread.operation().runtime_status, Some(Status::Unknown));
        assert!(!reread.operation().settled);
        for failure in observed.operation().failures() {
            assert!(reread.operation().failures().contains(&failure));
        }
        current = reread;
    }
    let observed = current;
    let db_path: String = api
        .config_store
        .with_conn(|conn| {
            Ok(conn.query_row(
                "SELECT file FROM pragma_database_list WHERE name='main'",
                [],
                |row| row.get(0),
            )?)
        })
        .unwrap();
    let reopened = crate::SqliteWorkspaceStore::open(db_path).unwrap();
    assert_eq!(
        reopened
            .external_workdir_cleanup_operation(TEST_WORKSPACE_ID, &grant.grant_id)
            .unwrap()
            .unwrap()
            .operation(),
        observed.operation()
    );
    let pending = reopened
        .reconcile_external_workdir_grants_after_restart(TEST_WORKSPACE_ID, &now)
        .unwrap();
    assert_eq!(pending, vec![grant.working_directory_id.clone()]);
    for workdir in pending {
        assert!(
            cleanup_external_workdir_attachments(api, &workdir)
                .await
                .is_err()
        );
    }
    assert_eq!(
        external_workdir_cleanup_reconciliation_count(&grant.working_directory_id),
        before
    );
    let recovered = api
        .store
        .external_workdir_cleanup_operation(TEST_WORKSPACE_ID, &grant.grant_id)
        .unwrap()
        .unwrap();
    assert_eq!(recovered.operation().request, observed.operation().request);
    assert_eq!(recovered.operation().runtime_status, Some(Status::Unknown));
    assert!(!recovered.operation().settled);
    for failure in observed.operation().failures() {
        assert!(recovered.operation().failures().contains(&failure));
    }
    let observed = recovered;
    let frozen = request.runtime_request().unwrap();
    let worker_ref = runtime_api::WorkerRef {
        worker_id: fixture.worker.worker_id.parse().unwrap(),
    };
    let receipt = runtime_api::WorkerWorkdirAttachmentsOperationReceipt {
        worker_ref: worker_ref.clone(),
        request: frozen.clone(),
        resulting_generation: "original-result-generation".into(),
    };
    let proof = runtime_api::WorkerWorkdirAttachmentsOperationObservation {
        worker_ref,
        request: frozen,
        result: runtime_api::WorkerWorkdirAttachmentsOperationOutcome::Receipt { receipt },
        current_generation: Some("original-result-generation".into()),
    };
    for mismatch in [
        "superseded_same_set",
        "unconfirmed_generation",
        "different_operation",
        "different_worker",
        "absent",
        "conflict",
        "unconfirmed",
    ] {
        let mut invalid = proof.clone();
        match mismatch {
            "superseded_same_set" => {
                invalid.current_generation = Some("same-set-later-attempt".into())
            }
            "unconfirmed_generation" => invalid.current_generation = None,
            "different_operation" => {
                let runtime_api::WorkerWorkdirAttachmentsOperationOutcome::Receipt { receipt } =
                    &mut invalid.result
                else {
                    unreachable!()
                };
                receipt.request.request_id = "another-operation".into();
            }
            "different_worker" => {
                invalid.worker_ref.worker_id =
                    "00000000-0000-7000-8000-000000000002".parse().unwrap()
            }
            "absent" => {
                invalid.result = runtime_api::WorkerWorkdirAttachmentsOperationOutcome::Absent
            }
            "conflict" => {
                invalid.result = runtime_api::WorkerWorkdirAttachmentsOperationOutcome::Conflict
            }
            _ => {
                invalid.result = runtime_api::WorkerWorkdirAttachmentsOperationOutcome::Unconfirmed
            }
        }
        assert!(
            matches!(
                api.store.settle_external_workdir_cleanup_operation(
                    &observed,
                    Some(&invalid),
                    &now
                ),
                Err(Error::WorkdirAttachmentConflict(_))
            ),
            "{mismatch}"
        );
        assert!(
            !api.store
                .external_workdir_cleanup_operation(TEST_WORKSPACE_ID, &grant.grant_id)
                .unwrap()
                .unwrap()
                .operation()
                .settled
        );
    }
    // Even a valid original Runtime receipt cannot settle a Backend connection
    // that has since been replaced. Roll this competing connection back solely
    // as fixture setup; this does not authorize production ABA recovery.
    api.config_store.with_conn(|conn| {
        conn.execute("UPDATE worker_workdir_links SET connection_id='competing-connection' WHERE workspace_id=?1 AND workdir_id=?2",
            rusqlite::params![TEST_WORKSPACE_ID, grant.working_directory_id])?;
        Ok(())
    }).unwrap();
    assert!(matches!(
        api.store
            .settle_external_workdir_cleanup_operation(&observed, Some(&proof), &now),
        Err(Error::WorkdirAttachmentConflict(_))
    ));
    api.config_store.with_conn(|conn| {
        conn.execute("UPDATE worker_workdir_links SET connection_id=?3 WHERE workspace_id=?1 AND workdir_id=?2",
            rusqlite::params![TEST_WORKSPACE_ID, grant.working_directory_id, link.connection_id])?;
        Ok(())
    }).unwrap();
    assert!(matches!(
        api.store
            .settle_external_workdir_cleanup_operation(&dispatched, Some(&proof), &now),
        Err(Error::WorkdirAttachmentConflict(_))
    ));
    let settled = api
        .store
        .settle_external_workdir_cleanup_operation(&observed, Some(&proof), &now)
        .unwrap();
    assert!(settled.operation().settled);
    assert_eq!(settled.operation().runtime_status, Some(Status::Unknown));
    let response = client
        .external_workdir_grant_get(TEST_WORKSPACE_ID.into(), grant.grant_id.clone())
        .await
        .unwrap();
    assert_eq!(response.cleanup_state, "completed");
    assert_eq!(
        response.cleanup_failures.len(),
        observed.operation().failures().len()
    );
    assert!(
        !format!("{settled:?} {:?} {:?}", request, request.target).contains(&link.connection_id)
    );
}

#[test]
fn external_grant_failure_unknown_and_auxiliary_audit_survive_storage_codec_independently() {
    let error = external_grant_failure::combine(
        Error::DbOperationOutcomeUnknown.into(),
        external_grant_failure::attributed(
            Error::Store("/private/audit database".into()),
            Reason::GrantAuditFailed,
        ),
    );
    let stored = external_grant_failure::stored_error(&error, Reason::GrantAttachmentReleaseFailed);
    let raw = external_grant_failure::encode(&stored).unwrap().unwrap();
    let reloaded: Vec<String> = serde_json::from_str(&raw).unwrap();
    let projected = reloaded
        .into_iter()
        .map(|raw| {
            protocol::background_failure::StoredFailure::from_storage(raw)
                .project_for::<error_contract::Tool, WorkdirManagementFailure>(
                    &WorkdirManagementVisibility {
                        can_view_cause: true,
                    },
                )
                .unwrap()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        projected[0].details().reason,
        Reason::GrantCleanupOutcomeUnknown
    );
    assert_eq!(projected[1].details().reason, Reason::GrantAuditFailed);
    assert!(!raw.contains("/private/"));
    let Error::ExternalWorkdirCleanupFailed {
        primary,
        supplementary,
    } = error.error
    else {
        panic!("local sources lost")
    };
    assert!(matches!(*primary, Error::DbOperationOutcomeUnknown));
    assert!(matches!(
        supplementary.as_slice(),
        [Error::ExternalWorkdirObservationFailed {
            reason: Reason::GrantAuditFailed,
            ..
        }]
    ));
}

#[tokio::test]
async fn external_grant_failure_registry_write_rolls_back_authority_and_activation_together() {
    let dir = tempfile::tempdir().unwrap();
    init_clean_git_workspace(dir.path());
    let api = test_api(dir.path()).await;
    let (client, _tasks, ..) = client(&api).await;
    let grant = client
        .external_workdir_grant_create(TEST_WORKSPACE_ID.into(), request())
        .await
        .unwrap();
    api.config_store
        .with_conn(|conn| {
            conn.execute_batch(
                "CREATE TRIGGER fail_grant_registry BEFORE UPDATE ON workdir_registry
            BEGIN SELECT RAISE(FAIL, '/private/registry unavailable'); END;",
            )?;
            Ok(())
        })
        .unwrap();
    let now = Utc::now().to_rfc3339_opts(SecondsFormat::Nanos, true);
    assert!(
        api.store
            .activate_external_workdir_grant(
                TEST_WORKSPACE_ID,
                &grant.grant_id,
                &grant.provider_instance_id,
                grant.generation,
                &now
            )
            .is_err()
    );
    assert!(
        api.store
            .update_external_workdir_grant_state(
                TEST_WORKSPACE_ID,
                &grant.grant_id,
                &grant.provider_instance_id,
                grant.generation,
                "revoked",
                &now
            )
            .is_err()
    );
    let stored = api
        .store
        .get_external_workdir_grant(TEST_WORKSPACE_ID, &grant.grant_id)
        .unwrap()
        .unwrap();
    assert_eq!(stored.status, "pending");
    assert_eq!(stored.generation, grant.generation);
    assert_eq!(
        api.store
            .get_workdir_registry(TEST_WORKSPACE_ID, &grant.working_directory_id)
            .unwrap()
            .unwrap()
            .materialization_status,
        "pending"
    );
}

#[tokio::test]
async fn external_grant_failure_activation_and_audit_compensation_are_independent() {
    use server_api::ExternalWorkdirConnectionClosedReason as Closed;
    for audit_failure in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        init_clean_git_workspace(dir.path());
        let api = test_api(dir.path()).await;
        let (client, _tasks, base, _) = client(&api).await;
        let grant = client
            .external_workdir_grant_create(TEST_WORKSPACE_ID.into(), request())
            .await
            .unwrap();
        api.config_store
            .with_conn(|conn| {
                conn.execute_batch(if audit_failure {
                    "CREATE TRIGGER fail_connect_audit BEFORE INSERT ON audit_events
                 WHEN NEW.action='external_workdir_provider_connected'
                 BEGIN SELECT RAISE(FAIL, '/private/provider/audit'); END;
                 CREATE TRIGGER fail_offline BEFORE UPDATE ON external_workdir_grants
                 WHEN NEW.status='offline'
                 BEGIN SELECT RAISE(FAIL, '/private/database/compensation'); END;"
                } else {
                    "CREATE TRIGGER fail_activation BEFORE UPDATE ON external_workdir_grants
                 WHEN NEW.status='online'
                 BEGIN SELECT RAISE(FAIL, '/private/database/activation'); END;"
                })?;
                Ok(())
            })
            .unwrap();
        let mut socket = register_provider(&api, &base, &grant).await;
        let response = observed_connection(&client, &grant.grant_id, 1).await;
        assert_eq!(
            response.status,
            if audit_failure { "online" } else { "pending" }
        );
        assert_eq!(response.cleanup_state, "not_required");
        assert!(response.cleanup_failures.is_empty());
        assert_eq!(
            response.connection_observations[0].closed_reason,
            Some(Closed::ActivationFailed)
        );
        assert_eq!(
            connection_reasons(&response),
            if audit_failure {
                vec![Reason::GrantAuditFailed, Reason::GrantConnectionStateFailed]
            } else {
                vec![Reason::GrantActivationFailed]
            }
        );
        assert!(
            !serde_json::to_string(&response)
                .unwrap()
                .contains("/private/")
        );
        assert!(
            api.external_workdir_providers
                .lock()
                .unwrap()
                .connection(&grant.grant_id)
                .is_none()
        );
        let _ = socket.close(None).await;
    }
}

#[tokio::test]
async fn external_grant_failure_terminal_primary_survives_state_or_audit_failure() {
    use server_api::ExternalWorkdirConnectionClosedReason as Closed;
    use tokio_tungstenite::tungstenite::Message;
    for audit_failure in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        init_clean_git_workspace(dir.path());
        let api = test_api(dir.path()).await;
        let (client, _tasks, base, _) = client(&api).await;
        let grant = client
            .external_workdir_grant_create(TEST_WORKSPACE_ID.into(), request())
            .await
            .unwrap();
        let mut socket = register_provider(&api, &base, &grant).await;
        registered(&mut socket).await;
        api.config_store
            .with_conn(|conn| {
                conn.execute_batch(if audit_failure {
                    "CREATE TRIGGER fail_disconnect_audit BEFORE INSERT ON audit_events
                 WHEN NEW.action='external_workdir_provider_disconnected'
                 BEGIN SELECT RAISE(FAIL, '/private/provider/audit'); END;"
                } else {
                    "CREATE TRIGGER fail_disconnect_state BEFORE UPDATE ON external_workdir_grants
                 WHEN NEW.status='offline'
                 BEGIN SELECT RAISE(FAIL, '/private/database/state'); END;"
                })?;
                Ok(())
            })
            .unwrap();
        socket
            .send(Message::Text("/private/provider/not-json".into()))
            .await
            .unwrap();
        let response = observed_connection(&client, &grant.grant_id, 1).await;
        assert_eq!(
            response.status,
            if audit_failure { "offline" } else { "online" }
        );
        assert_eq!(response.cleanup_state, "not_required");
        assert!(response.cleanup_failures.is_empty());
        assert_eq!(
            response.connection_observations[0].closed_reason,
            Some(Closed::InvalidFrame)
        );
        assert_eq!(
            connection_reasons(&response),
            vec![
                Reason::GrantConnectionProtocolFailed,
                if audit_failure {
                    Reason::GrantAuditFailed
                } else {
                    Reason::GrantConnectionStateFailed
                }
            ]
        );
        let reloaded = client
            .external_workdir_grant_get(TEST_WORKSPACE_ID.into(), grant.grant_id.clone())
            .await
            .unwrap();
        assert_eq!(
            response.connection_observations,
            reloaded.connection_observations
        );
        assert!(
            !serde_json::to_string(&reloaded)
                .unwrap()
                .contains("/private/")
        );
        assert!(
            api.external_workdir_providers
                .lock()
                .unwrap()
                .connection(&grant.grant_id)
                .is_none()
        );
    }
}

#[tokio::test]
async fn external_grant_failure_registered_send_retains_transport_state_and_save_sources() {
    use server_api::ExternalWorkdirConnectionClosedReason as Closed;
    let dir = tempfile::tempdir().unwrap();
    init_clean_git_workspace(dir.path());
    let api = test_api(dir.path()).await;
    let (client, _tasks, ..) = client(&api).await;
    let grant = client
        .external_workdir_grant_create(TEST_WORKSPACE_ID.into(), request())
        .await
        .unwrap();
    let record = api
        .store
        .get_external_workdir_grant(TEST_WORKSPACE_ID, &grant.grant_id)
        .unwrap()
        .unwrap();
    api.store
        .activate_external_workdir_grant(
            TEST_WORKSPACE_ID,
            &grant.grant_id,
            &grant.provider_instance_id,
            grant.generation,
            &Utc::now().to_rfc3339(),
        )
        .unwrap();
    api.config_store
        .with_conn(|conn| {
            conn.execute_batch(
                "CREATE TRIGGER fail_registered_offline BEFORE UPDATE ON external_workdir_grants
            WHEN NEW.status='offline' BEGIN SELECT RAISE(FAIL, '/private/database/offline'); END;",
            )?;
            Ok(())
        })
        .unwrap();
    for fail_save in [false, true] {
        if fail_save {
            api.config_store.with_conn(|conn| {
                conn.execute_batch("CREATE TRIGGER fail_registered_evidence BEFORE INSERT ON external_workdir_connection_observations
                    BEGIN SELECT RAISE(FAIL, '/private/database/evidence'); END;")?;
                Ok(())
            }).unwrap();
        }
        let (sender, _commands) = tokio::sync::mpsc::channel(1);
        let connection = Arc::new(ExternalProviderConnection {
            grant_id: grant.grant_id.clone(),
            workdir_id: grant.working_directory_id.clone(),
            provider_instance_id: grant.provider_instance_id.clone(),
            generation: grant.generation,
            expires_at: None,
            capabilities: workdir::WorkdirSessionCapabilities::READ_ONLY,
            admission: Arc::new(tokio::sync::Semaphore::new(16)),
            shutdown_confirmed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            sender,
        });
        assert!(
            api.external_workdir_providers
                .lock()
                .unwrap()
                .connected(connection.clone())
                .is_none()
        );
        let timeout = tokio::time::timeout(std::time::Duration::ZERO, std::future::pending::<()>())
            .await
            .unwrap_err();
        let original = Arc::new(Error::ExternalWorkdirTransportTimeout(timeout));
        let stage_api = api.clone();
        let stage_grant = record.clone();
        let stage_source = original.clone();
        let retained = api
            .store
            .db_executor()
            .run(move || -> Result<_> {
                Ok(external_grant_failure::registration_failed(
                    &stage_api,
                    &stage_grant,
                    &connection,
                    stage_source,
                ))
            })
            .await
            .unwrap()
            .unwrap();
        assert!(
            api.external_workdir_providers
                .lock()
                .unwrap()
                .connection(&grant.grant_id)
                .is_none()
        );
        let failures = external_grant_failure::connection_stored(&retained);
        let reasons = failures
            .iter()
            .map(|failure| {
                protocol::background_failure::StoredFailure::from_storage(
                    failure.as_storage_str().into(),
                )
                .project_for::<error_contract::Tool, WorkdirManagementFailure>(
                    &WorkdirManagementVisibility {
                        can_view_cause: true,
                    },
                )
                .unwrap()
                .details()
                .reason
            })
            .collect::<Vec<_>>();
        let mut expected = vec![
            Reason::GrantConnectionTransportFailed,
            Reason::GrantConnectionStateFailed,
        ];
        if fail_save {
            expected.push(Reason::GrantConnectionPersistenceFailed);
        }
        assert_eq!(reasons, expected);
        // The real timeout source remains shared, rather than reconstructed from
        // the safe explanation or converted into a database/string error.
        assert!(Arc::strong_count(&original) >= 2);
        assert!(matches!(
            original.as_ref(),
            Error::ExternalWorkdirTransportTimeout(_)
        ));
        let response = client
            .external_workdir_grant_get(TEST_WORKSPACE_ID.into(), grant.grant_id.clone())
            .await
            .unwrap();
        assert_eq!(response.status, "online");
        assert_eq!(response.cleanup_state, "not_required");
        assert!(response.cleanup_failures.is_empty());
        assert_eq!(response.connection_observations.len(), 1);
        assert_eq!(
            response.connection_observations[0].closed_reason,
            Some(Closed::RegistrationSendFailed)
        );
        assert_eq!(
            connection_reasons(&response),
            vec![
                Reason::GrantConnectionTransportFailed,
                Reason::GrantConnectionStateFailed
            ]
        );
        assert!(
            !serde_json::to_string(&response)
                .unwrap()
                .contains("/private/")
        );
    }
}

#[tokio::test]
async fn external_grant_failure_connection_unknown_and_save_failure_keep_local_sources_without_retry()
 {
    use server_api::ExternalWorkdirConnectionClosedReason as Closed;
    let dir = tempfile::tempdir().unwrap();
    init_clean_git_workspace(dir.path());
    let api = test_api(dir.path()).await;
    let (client, _tasks, ..) = client(&api).await;
    let grant = client
        .external_workdir_grant_create(TEST_WORKSPACE_ID.into(), request())
        .await
        .unwrap();
    let record = api
        .store
        .get_external_workdir_grant(TEST_WORKSPACE_ID, &grant.grant_id)
        .unwrap()
        .unwrap();
    let primary = || {
        external_grant_failure::combine(
            external_grant_failure::connection_error(
                Error::DbOperationOutcomeUnknown,
                Reason::GrantActivationFailed,
            ),
            external_grant_failure::connection_error(
                Error::Store("/private/audit".into()),
                Reason::GrantAuditFailed,
            ),
        )
    };
    let retained = external_grant_failure::save_connection(
        &api,
        &record,
        grant.generation,
        Closed::ActivationFailed,
        Some(primary()),
    );
    assert!(retained.is_some());
    let response = client
        .external_workdir_grant_get(TEST_WORKSPACE_ID.into(), grant.grant_id.clone())
        .await
        .unwrap();
    assert_eq!(response.status, "pending");
    assert_eq!(response.cleanup_state, "not_required");
    assert_eq!(
        connection_reasons(&response),
        vec![Reason::GrantStateOutcomeUnknown, Reason::GrantAuditFailed]
    );
    api.config_store.with_conn(|conn| {
        conn.execute_batch("CREATE TRIGGER fail_connection_save BEFORE INSERT ON external_workdir_connection_observations
            BEGIN SELECT RAISE(FAIL, '/private/database/evidence'); END;")?;
        Ok(())
    }).unwrap();
    let retained = external_grant_failure::save_connection(
        &api,
        &record,
        grant.generation,
        Closed::ActivationFailed,
        Some(primary()),
    )
    .unwrap();
    assert!(
        external_grant_failure::outcome_unknown(&retained),
        "supplementary save failure must not authorize background retry of the unknown primary"
    );
    let reasons = external_grant_failure::connection_stored(&retained)
        .iter()
        .map(|failure| {
            protocol::background_failure::StoredFailure::from_storage(
                failure.as_storage_str().into(),
            )
            .project_for::<error_contract::Tool, WorkdirManagementFailure>(
                &WorkdirManagementVisibility {
                    can_view_cause: true,
                },
            )
            .unwrap()
            .details()
            .reason
        })
        .collect::<Vec<_>>();
    assert_eq!(
        reasons,
        vec![
            Reason::GrantStateOutcomeUnknown,
            Reason::GrantAuditFailed,
            Reason::GrantConnectionPersistenceFailed
        ]
    );
    let Error::ExternalWorkdirCleanupFailed {
        primary,
        supplementary,
    } = retained.error
    else {
        panic!("source tree lost")
    };
    assert!(matches!(
        *primary,
        Error::ExternalWorkdirCleanupFailed { .. }
    ));
    assert!(
        matches!(supplementary.as_slice(), [Error::ExternalWorkdirObservationFailed {
        reason: Reason::GrantConnectionPersistenceFailed, source, ..
    }] if matches!(source.as_ref(), Error::Sqlite(_)))
    );
    let reread = client
        .external_workdir_grant_get(TEST_WORKSPACE_ID.into(), grant.grant_id.clone())
        .await
        .unwrap();
    assert_eq!(
        reread.connection_observations,
        response.connection_observations
    );
    assert_eq!(reread.status, "pending");
    assert_eq!(reread.cleanup_state, "not_required");
    assert!(
        !serde_json::to_string(&reread)
            .unwrap()
            .contains("/private/")
    );
}

#[tokio::test]
async fn external_grant_failure_connection_legacy_mix_and_superseded_generation_do_not_invent_causes()
 {
    use server_api::ExternalWorkdirConnectionClosedReason as Closed;
    let dir = tempfile::tempdir().unwrap();
    init_clean_git_workspace(dir.path());
    let api = test_api(dir.path()).await;
    let (client, _tasks, ..) = client(&api).await;
    let grant = client
        .external_workdir_grant_create(TEST_WORKSPACE_ID.into(), request())
        .await
        .unwrap();
    let record = api
        .store
        .get_external_workdir_grant(TEST_WORKSPACE_ID, &grant.grant_id)
        .unwrap()
        .unwrap();
    let known =
        external_grant_failure::connection_stored(&external_grant_failure::connection_error(
            Error::Store("/private/provider/raw".into()),
            Reason::GrantAuditFailed,
        ));
    let mixed = serde_json::to_string(&vec![
        "/private/legacy/error",
        known[0].as_storage_str(),
        "{broken}",
    ])
    .unwrap();
    api.store
        .record_external_workdir_connection_observation(
            &record,
            grant.generation,
            Closed::PeerClosed,
            Some(&mixed),
            &Utc::now().to_rfc3339(),
        )
        .unwrap();
    let response = client
        .external_workdir_grant_get(TEST_WORKSPACE_ID.into(), grant.grant_id.clone())
        .await
        .unwrap();
    assert_eq!(
        connection_reasons(&response),
        vec![Reason::GrantAuditFailed]
    );
    assert!(
        !serde_json::to_string(&response)
            .unwrap()
            .contains("/private/")
    );
    api.config_store.with_conn(|conn| {
        assert!(conn.execute("UPDATE external_workdir_connection_observations SET closed_reason='send_failed'", []).is_err());
        assert!(conn.execute("INSERT INTO external_workdir_connection_observations
            (workspace_id,grant_id,provider_instance_id,grant_generation,connection_generation,closed_reason,observed_at)
            VALUES(?1,?2,?3,1,1,'invented_close','now')",
            rusqlite::params![TEST_WORKSPACE_ID, grant.grant_id, grant.provider_instance_id]).is_err());
        Ok(())
    }).unwrap();
    assert!(
        api.store
            .record_external_workdir_connection_observation(
                &record,
                grant.generation,
                Closed::PeerClosed,
                Some(&"é".repeat(131073)),
                &Utc::now().to_rfc3339()
            )
            .is_err(),
        "observation byte bound must not become a character bound"
    );
    for raw in ["/private/old-untyped", "[\"{broken}\"]"] {
        api.store
            .record_external_workdir_connection_observation(
                &record,
                grant.generation,
                Closed::PeerClosed,
                Some(raw),
                &Utc::now().to_rfc3339(),
            )
            .unwrap();
    }
    let response = client
        .external_workdir_grant_get(TEST_WORKSPACE_ID.into(), grant.grant_id.clone())
        .await
        .unwrap();
    assert!(
        response
            .connection_observations
            .iter()
            .skip(1)
            .all(|observation| observation.failures.is_empty())
    );
    assert_eq!(
        connection_reasons(&response),
        vec![Reason::GrantAuditFailed]
    );
    api.store
        .update_external_workdir_grant_state(
            TEST_WORKSPACE_ID,
            &grant.grant_id,
            &grant.provider_instance_id,
            grant.generation,
            "offline",
            &Utc::now().to_rfc3339(),
        )
        .unwrap();
    assert!(
        api.store
            .record_external_workdir_connection_observation(
                &record,
                grant.generation + 1,
                Closed::ActivationFailed,
                Some(&mixed),
                &Utc::now().to_rfc3339()
            )
            .unwrap()
    );
    let rejected_reconnect = client
        .external_workdir_grant_get(TEST_WORKSPACE_ID.into(), grant.grant_id.clone())
        .await
        .unwrap();
    let attempt = rejected_reconnect.connection_observations.last().unwrap();
    assert_eq!(attempt.grant_generation, grant.generation);
    assert_eq!(attempt.connection_generation, grant.generation + 1);
    api.store
        .activate_external_workdir_grant(
            TEST_WORKSPACE_ID,
            &grant.grant_id,
            &grant.provider_instance_id,
            grant.generation + 1,
            &Utc::now().to_rfc3339(),
        )
        .unwrap();
    let fresh = client
        .external_workdir_grant_get(TEST_WORKSPACE_ID.into(), grant.grant_id.clone())
        .await
        .unwrap();
    assert_eq!(fresh.generation, grant.generation + 1);
    assert!(fresh.connection_observations.is_empty());
    assert!(
        !api.store
            .record_external_workdir_connection_observation(
                &record,
                grant.generation,
                Closed::PeerClosed,
                Some(&mixed),
                &Utc::now().to_rfc3339()
            )
            .unwrap(),
        "stale continuation must not enter the new generation"
    );
    let mut older_dto = serde_json::to_value(&fresh).unwrap();
    older_dto
        .as_object_mut()
        .unwrap()
        .remove("connection_observations");
    let older_dto: ExternalWorkdirGrantResponse = serde_json::from_value(older_dto).unwrap();
    assert!(older_dto.connection_observations.is_empty());
}

#[tokio::test]
async fn external_grant_failure_connection_query_failure_is_not_a_closed_connection_or_cleanup_result()
 {
    let dir = tempfile::tempdir().unwrap();
    init_clean_git_workspace(dir.path());
    let api = test_api(dir.path()).await;
    let (client, _tasks, ..) = client(&api).await;
    let grant = client
        .external_workdir_grant_create(TEST_WORKSPACE_ID.into(), request())
        .await
        .unwrap();
    api.config_store
        .with_conn(|conn| {
            conn.execute_batch("DROP TABLE external_workdir_connection_observations")?;
            Ok(())
        })
        .unwrap();
    let response = client
        .external_workdir_grant_get(TEST_WORKSPACE_ID.into(), grant.grant_id.clone())
        .await
        .unwrap();
    assert_eq!(response.status, "pending");
    assert_eq!(response.cleanup_state, "not_required");
    assert!(response.cleanup_failures.is_empty());
    assert_eq!(response.connection_observations[0].closed_reason, None);
    assert_eq!(
        connection_reasons(&response),
        vec![Reason::GrantConnectionQueryFailed]
    );
}

#[test]
fn external_grant_failure_connection_telemetry_projects_each_source_without_raw_text() {
    #[derive(Clone, Default)]
    struct Logs(Arc<Mutex<Vec<u8>>>);
    impl std::io::Write for Logs {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let logs = Logs::default();
    let writer = logs.clone();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    let error = external_grant_failure::combine(
        external_grant_failure::connection_error(
            Error::DbOperationOutcomeUnknown,
            Reason::GrantActivationFailed,
        ),
        external_grant_failure::connection_error(
            Error::Store("/private/provider token=SECRET".into()),
            Reason::GrantAuditFailed,
        ),
    );
    tracing::subscriber::with_default(subscriber, || external_grant_failure::telemetry(&error));
    let text = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
    assert!(text.contains(Reason::GrantStateOutcomeUnknown.as_str()));
    assert!(text.contains(Reason::GrantAuditFailed.as_str()));
    assert!(!text.contains("/private/"));
    assert!(!text.contains("SECRET"));
}

#[tokio::test]
async fn external_grant_failure_terminal_cleanup_preserves_fence_release_and_audit_sources() {
    use server_api::ExternalWorkdirConnectionClosedReason as Closed;
    use tokio_tungstenite::tungstenite::Message;
    let fixture = grant_cleanup_fixture().await;
    let api = &fixture.api;
    let (client, _tasks, base, _) = client(api).await;
    let grant = client
        .external_workdir_grant_create(TEST_WORKSPACE_ID.into(), request())
        .await
        .unwrap();
    api.store
        .attach_worker_workdir(&WorkerWorkdirLinkRecord {
            connection_id: String::new(),
            workspace_id: TEST_WORKSPACE_ID.into(),
            worker: fixture.worker.clone(),
            workdir_id: grant.working_directory_id.clone(),
            alias: "terminal-failure".into(),
            capabilities: workdir::WorkdirSessionCapabilities::READ_ONLY,
            linked_at: Utc::now().to_rfc3339(),
            unlinked_at: None,
        })
        .unwrap();
    let mut socket = register_provider(api, &base, &grant).await;
    registered(&mut socket).await;
    inject_external_workdir_cleanup_failure(&grant.working_directory_id);
    api.config_store
        .with_conn(|conn| {
            conn.execute_batch(
                "CREATE TRIGGER fail_terminal_audit BEFORE INSERT ON audit_events
            WHEN NEW.action='external_workdir_provider_disconnected'
            BEGIN SELECT RAISE(FAIL, '/private/terminal/audit'); END;",
            )?;
            Ok(())
        })
        .unwrap();
    let connection = api
        .external_workdir_providers
        .lock()
        .unwrap()
        .connection(&grant.grant_id)
        .unwrap();
    connection
        .sender
        .send(ExternalProviderCommand::Revoke {
            reason: workdir::external::ExternalWorkdirRevokeReason::UserRequested,
        })
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let message = socket.next().await.unwrap().unwrap().into_text().unwrap();
            let frame: ExternalWorkdirServerFrame = serde_json::from_str(&message).unwrap();
            if matches!(frame.message, ExternalWorkdirServerMessage::Revoke { .. }) {
                break;
            }
        }
    })
    .await
    .unwrap();
    socket
        .send(Message::Text(
            serde_json::to_string(&ExternalWorkdirProviderFrame::current(
                ExternalWorkdirProviderMessage::RevokeAcknowledged {
                    generation: grant.generation,
                },
            ))
            .unwrap()
            .into(),
        ))
        .await
        .unwrap();
    // Wait for the connection continuation itself; owner GET has independent
    // terminal-cleanup retry behavior and must not consume the injected failure.
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if api
                .store
                .external_workdir_connection_observations(TEST_WORKSPACE_ID, &grant.grant_id)
                .unwrap()
                .len()
                >= 2
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let response = client
        .external_workdir_grant_get(TEST_WORKSPACE_ID.into(), grant.grant_id.clone())
        .await
        .unwrap();
    assert_eq!(response.status, "revoked");
    assert!(
        response
            .connection_observations
            .iter()
            .all(|observation| observation.closed_reason == Some(Closed::RevokeAcknowledged))
    );
    let reasons = connection_reasons(&response);
    assert!(reasons.contains(&Reason::GrantAuditFailed));
    assert!(reasons.contains(&Reason::GrantAttachmentReleaseFailed));
    assert!(
        api.store
            .list_workdir_worker_links(TEST_WORKSPACE_ID, &grant.working_directory_id)
            .unwrap()
            .iter()
            .all(|link| link.unlinked_at.is_some())
    );
    assert!(
        !serde_json::to_string(&response)
            .unwrap()
            .contains("/private/")
    );
    let reread = client
        .external_workdir_grant_get(TEST_WORKSPACE_ID.into(), grant.grant_id.clone())
        .await
        .unwrap();
    assert_eq!(
        reread.connection_observations,
        response.connection_observations
    );
}

#[test]
fn external_grant_runtime_cleanup_retains_owner_cause_and_uses_outcome_not_prose() {
    use crate::hosts::{RuntimeOperationResponse, RuntimeOperationStatus as Status};
    use protocol::control_failure::RuntimeControlCause as C;
    for (status, unknown) in [(Status::Unknown, true), (Status::Rejected, false)] {
        let error = ApiError::from(Error::WorkdirAttachmentSyncFailed {
            runtime_id: "/private/runtime".into(),
            observation: Box::new(RuntimeOperationResponse {
                status,
                cause: Some(C::PersistenceFailed),
                worker: None,
                diagnostics: vec![RuntimeDiagnostic {
                    workdir: None,
                    startup: None,
                    code: "safe_to_retry".into(),
                    control: None,
                    severity: HostDiagnosticSeverity::Error,
                    message: "/private/provider secret says no effects".into(),
                }],
            }),
        });
        let error = external_grant_failure::combine(
            external_grant_failure::attributed(error.error, Reason::GrantAttachmentReleaseFailed),
            external_grant_failure::attributed(
                Error::DbExecutorUnavailable,
                Reason::GrantAuditFailed,
            ),
        );
        assert_eq!(external_grant_failure::outcome_unknown(&error), unknown);
        let stored =
            external_grant_failure::stored_error(&error, Reason::GrantAttachmentReleaseFailed);
        assert_eq!(stored.len(), 2);
        let loaded = protocol::background_failure::StoredFailure::from_storage(
            stored[0].as_storage_str().into(),
        );
        let public = external_grant_failure::publish(&loaded).unwrap();
        let tool = public.project_for::<error_contract::Tool, C>(&()).unwrap();
        assert_eq!(tool.details(), &C::PersistenceFailed);
        let bytes = serde_json::to_string(&external_grant_failure::public_error(
            &error,
            Reason::GrantAttachmentReleaseFailed,
        ))
        .unwrap();
        assert!(!bytes.contains("/private/"));
        assert!(!bytes.contains("safe_to_retry"));
    }
}

#[tokio::test]
async fn external_grant_cleanup_query_failure_keeps_the_preceding_audit_without_release() {
    let dir = tempfile::tempdir().unwrap();
    init_clean_git_workspace(dir.path());
    let api = test_api(dir.path()).await;
    let audit = external_grant_failure::attributed(
        Error::RegistryInconsistency("/private/audit secret".into()),
        Reason::GrantAuditFailed,
    );
    let error =
        cleanup_external_workdir_attachments_with_lock(&api, "missing-workdir", None, Some(audit))
            .await
            .unwrap_err();
    let failures =
        external_grant_failure::public_error(&error, Reason::GrantAttachmentReleaseFailed);
    let reasons: Vec<_> = failures
        .iter()
        .map(|failure| {
            failure
                .project_for::<error_contract::Tool, WorkdirManagementFailure>(
                    &WorkdirManagementVisibility {
                        can_view_cause: true,
                    },
                )
                .unwrap()
                .details()
                .reason
        })
        .collect();
    assert_eq!(
        reasons,
        vec![Reason::GrantCleanupQueryFailed, Reason::GrantAuditFailed]
    );
    assert_eq!(
        external_workdir_cleanup_reconciliation_count("missing-workdir"),
        0
    );
    assert!(
        !serde_json::to_string(&failures)
            .unwrap()
            .contains("/private/")
    );
}

#[tokio::test]
async fn external_grant_terminal_websocket_refusal_keeps_cleanup_secondary() {
    use server_api::workdir_management_error::WorkdirAttachmentFailureStage as Stage;
    let fixture = grant_cleanup_fixture().await;
    let api = &fixture.api;
    let (client, _tasks, base, http) = client(api).await;
    for status in ["revoked", "expired"] {
        let grant = client
            .external_workdir_grant_create(TEST_WORKSPACE_ID.into(), request())
            .await
            .unwrap();
        api.store
            .attach_worker_workdir(&WorkerWorkdirLinkRecord {
                connection_id: String::new(),
                workspace_id: TEST_WORKSPACE_ID.into(),
                worker: fixture.worker.clone(),
                workdir_id: grant.working_directory_id.clone(),
                alias: format!("terminal-{status}"),
                capabilities: workdir::WorkdirSessionCapabilities::READ_ONLY,
                linked_at: Utc::now().to_rfc3339(),
                unlinked_at: None,
            })
            .unwrap();
        assert!(
            api.store
                .update_external_workdir_grant_state(
                    TEST_WORKSPACE_ID,
                    &grant.grant_id,
                    &grant.provider_instance_id,
                    grant.generation,
                    status,
                    &Utc::now().to_rfc3339()
                )
                .unwrap()
        );
        inject_external_workdir_cleanup_failure(&grant.working_directory_id);
        let response = http
            .get(format!(
                "{base}/api/w/{TEST_WORKSPACE_ID}/external-workdir-grants/{}/provider",
                grant.grant_id
            ))
            .header("Connection", "Upgrade")
            .header("Upgrade", "websocket")
            .header("Sec-WebSocket-Version", "13")
            .header("Sec-WebSocket-Key", "dGhlIHNhbXBsZSBub25jZQ==")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), 400);
        let media = response.headers()[reqwest::header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .to_string();
        let body = response.bytes().await.unwrap();
        let error = WorkdirManagementApiError::decode_received(400, &media, &body).unwrap();
        assert_eq!(
            error.details().unwrap().reason,
            if status == "expired" {
                Reason::GrantExpired
            } else {
                Reason::GrantRevoked
            }
        );
        assert_eq!(error.details().unwrap().operation, Op::GrantConnect);
        assert_eq!(error.related_failures().len(), 1);
        assert_eq!(error.related_failures()[0].stage, Stage::GrantCleanup);
        let cleanup = error.related_failures()[0]
            .failure
            .as_ref()
            .unwrap()
            .project_for::<error_contract::Tool, WorkdirManagementFailure>(
                &WorkdirManagementVisibility {
                    can_view_cause: true,
                },
            )
            .unwrap();
        assert_eq!(cleanup.details().operation, Op::GrantCleanup);
        assert_eq!(
            cleanup.details().reason,
            Reason::GrantAttachmentReleaseFailed
        );
        assert!(!std::str::from_utf8(&body).unwrap().contains("/private/"));
        assert_eq!(
            api.store
                .get_external_workdir_grant(TEST_WORKSPACE_ID, &grant.grant_id)
                .unwrap()
                .unwrap()
                .status,
            status
        );
    }
}

#[tokio::test]
async fn external_grant_lost_runtime_response_settles_only_original_receipt_without_resend() {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use tower::ServiceExt;
    #[derive(Clone)]
    struct Transport {
        runtime: Router,
        sends: Arc<AtomicUsize>,
        hide_query: Arc<AtomicBool>,
    }
    async fn forward(State(state): State<Transport>, request: Request<Body>) -> Response {
        let mutation = request
            .uri()
            .path()
            .ends_with("/workdir-attachments/operations");
        let query = request
            .uri()
            .path()
            .ends_with("/workdir-attachments/operations/observe");
        if query && state.hide_query.load(Ordering::SeqCst) {
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
        let response = state.runtime.oneshot(request).await.unwrap();
        if mutation {
            state.sends.fetch_add(1, Ordering::SeqCst);
            assert_eq!(
                response.status(),
                StatusCode::OK,
                "the authenticated owner actually committed"
            );
            // Only the response is lost. The owner handler, generation and receipt
            // are production code, not a matching-set or Accepted stand-in.
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
        response
    }
    let fixture = grant_cleanup_fixture().await;
    let api = &fixture.api;
    let runtime_id = fixture.worker.runtime_id.clone();
    let binding = api
        .store
        .get_workspace_runtime_binding(TEST_WORKSPACE_ID, &runtime_id)
        .await
        .unwrap()
        .unwrap();
    let sends = Arc::new(AtomicUsize::new(0));
    let hide_query = Arc::new(AtomicBool::new(true));
    let transport = Transport {
        runtime: fixture._source.runtime_router(),
        sends: sends.clone(),
        hide_query: hide_query.clone(),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let router = Router::new()
        .fallback(axum::routing::any(forward))
        .with_state(transport);
    let mut proxy = tokio::task::JoinSet::new();
    proxy.spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let backend_url = api.config.backend_base_url.clone().unwrap();
    api.runtime.register_or_replace(
        RemoteWorkerRuntime::new(
            RemoteRuntimeConfig {
                runtime_id,
                workspace_id: Some(TEST_WORKSPACE_ID.into()),
                display_name: binding.display_name.clone(),
                base_url: endpoint,
                bearer_token: None,
                workspace_authorization: Some(crate::hosts::WorkspaceRuntimeAuthorization::new(
                    api.store.clone(),
                    api.signing_identities.clone(),
                    backend_url.clone(),
                    binding,
                )),
                strict_public_egress: false,
                cached_worker_creation_available: true,
                cached_os: "linux".into(),
                cached_arch: "x86_64".into(),
                cached_status: "active".into(),
                timeout: std::time::Duration::from_secs(10),
            },
            TEST_WORKSPACE_ID.into(),
            backend_url,
        )
        .unwrap(),
    );
    let (client, _server, ..) = client(api).await;
    let grant = client
        .external_workdir_grant_create(TEST_WORKSPACE_ID.into(), request())
        .await
        .unwrap();
    api.store
        .attach_worker_workdir(&WorkerWorkdirLinkRecord {
            connection_id: String::new(),
            workspace_id: TEST_WORKSPACE_ID.into(),
            worker: fixture.worker.clone(),
            workdir_id: grant.working_directory_id.clone(),
            alias: "lost-response".into(),
            capabilities: workdir::WorkdirSessionCapabilities::READ_ONLY,
            linked_at: Utc::now().to_rfc3339(),
            unlinked_at: None,
        })
        .unwrap();
    let response = client
        .external_workdir_grant_revoke(TEST_WORKSPACE_ID.into(), grant.grant_id.clone())
        .await
        .unwrap();
    assert_eq!(response.status, "revoked");
    assert_eq!(response.cleanup_state, "pending");
    let unknown = api
        .store
        .external_workdir_cleanup_operation(TEST_WORKSPACE_ID, &grant.grant_id)
        .unwrap()
        .unwrap();
    assert!(unknown.operation().runtime_dispatched);
    assert!(!unknown.operation().settled);
    assert_eq!(
        unknown.operation().runtime_status,
        Some(crate::hosts::RuntimeOperationStatus::Unknown)
    );
    assert!(!unknown.operation().failures().is_empty());
    for _ in 0..2 {
        assert_eq!(
            client
                .external_workdir_grant_get(TEST_WORKSPACE_ID.into(), grant.grant_id.clone())
                .await
                .unwrap()
                .cleanup_state,
            "pending"
        );
    }
    assert_eq!(sends.load(Ordering::SeqCst), 1);
    let frozen = unknown.operation().request.clone();
    hide_query.store(false, Ordering::SeqCst);
    cleanup_external_workdir_attachments(api, &grant.working_directory_id)
        .await
        .unwrap();
    let settled = api
        .store
        .external_workdir_cleanup_operation(TEST_WORKSPACE_ID, &grant.grant_id)
        .unwrap()
        .unwrap();
    assert!(settled.operation().settled);
    assert_eq!(settled.operation().request, frozen);
    assert_eq!(
        settled.operation().runtime_status,
        Some(crate::hosts::RuntimeOperationStatus::Unknown)
    );
    assert_eq!(
        settled.operation().failures(),
        unknown.operation().failures()
    );
    assert_eq!(
        sends.load(Ordering::SeqCst),
        1,
        "same-operation query is not mutation replay"
    );
    let published = client
        .external_workdir_grant_get(TEST_WORKSPACE_ID.into(), grant.grant_id.clone())
        .await
        .unwrap();
    assert_eq!(published.cleanup_state, "completed");
    assert!(!published.cleanup_failures.is_empty());
    let public = serde_json::to_string(&published).unwrap();
    assert!(!public.contains(&frozen.operation_id));
    assert!(!public.contains(&frozen.target.unwrap().expected_generation));
}

#[tokio::test]
async fn external_grant_unknown_child_does_not_block_workspace_reopen_or_authorize_redispatch() {
    use crate::grant_cleanup_operation::{CleanupAdmission, CleanupRequest, CleanupRuntimeTarget};
    let dir = tempfile::tempdir().unwrap();
    init_clean_git_workspace(dir.path());
    let (config, grant_id, workdir_id, frozen, saved_failures) = {
        let api = test_api(dir.path()).await;
        let (client, server, ..) = client(&api).await;
        let grant = client
            .external_workdir_grant_create(TEST_WORKSPACE_ID.into(), request())
            .await
            .unwrap();
        let worker =
            RuntimeWorkerRef::new("unavailable-cleanup-runtime", Uuid::now_v7().to_string());
        api.config_store.with_conn(|conn| {
            conn.execute("INSERT INTO worker_registry(workspace_id,runtime_id,worker_id,display_name,retention_state,created_at,updated_at) VALUES(?1,?2,?3,'Unavailable cleanup Worker','normal','1','1')", rusqlite::params![TEST_WORKSPACE_ID, worker.runtime_id, worker.worker_id])?;
            Ok(())
        }).unwrap();
        api.store
            .attach_worker_workdir(&WorkerWorkdirLinkRecord {
                connection_id: String::new(),
                workspace_id: TEST_WORKSPACE_ID.into(),
                worker: worker.clone(),
                workdir_id: grant.working_directory_id.clone(),
                alias: "unavailable".into(),
                capabilities: workdir::WorkdirSessionCapabilities::READ_ONLY,
                linked_at: Utc::now().to_rfc3339(),
                unlinked_at: None,
            })
            .unwrap();
        let link = api
            .store
            .latest_workdir_worker_link(TEST_WORKSPACE_ID, &grant.working_directory_id)
            .unwrap()
            .unwrap();
        let now = Utc::now().to_rfc3339();
        assert!(
            api.store
                .update_external_workdir_grant_state(
                    TEST_WORKSPACE_ID,
                    &grant.grant_id,
                    &grant.provider_instance_id,
                    grant.generation,
                    "revoked",
                    &now
                )
                .unwrap()
        );
        let frozen = CleanupRequest {
            operation_id: Uuid::now_v7().to_string(),
            workspace_id: TEST_WORKSPACE_ID.into(),
            grant_id: grant.grant_id.clone(),
            provider_instance_id: grant.provider_instance_id.clone(),
            grant_generation: grant.generation,
            working_directory_id: grant.working_directory_id.clone(),
            target: Some(CleanupRuntimeTarget {
                worker,
                connection_id: link.connection_id,
                alias: link.alias,
                expected_generation: "unavailable-original-generation".into(),
                logical_workdir_attachments: Vec::new(),
            }),
        };
        let CleanupAdmission::Admitted(admitted) = api
            .store
            .begin_external_workdir_cleanup_operation(&frozen, &[], &now)
            .unwrap()
        else {
            panic!("fresh intent")
        };
        let released = api
            .store
            .confirm_external_workdir_cleanup_release(&admitted, &now)
            .unwrap();
        let dispatched = api
            .store
            .mark_external_workdir_cleanup_runtime_dispatched(&released, &now)
            .unwrap();
        let unknown = api
            .store
            .record_external_workdir_cleanup_runtime_observation(
                &dispatched,
                crate::hosts::RuntimeOperationStatus::Unknown,
                &[],
                &now,
            )
            .unwrap();
        let saved_failures = unknown.operation().failures();
        let config = api.config.clone();
        drop(client);
        drop(server);
        (
            config,
            grant.grant_id,
            grant.working_directory_id,
            frozen,
            saved_failures,
        )
    };
    // Reopen Workspace persistence with a separate embedded execution store;
    // the unresolved child belongs to the intentionally unavailable Runtime.
    let config =
        config.with_embedded_runtime_store_root(dir.path().join("reopened-embedded-runtime"));
    let store = Arc::new(crate::SqliteWorkspaceStore::open(&config.database_path).unwrap());
    let reopened = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        WorkspaceApi::new_for_test(config, store),
    )
    .await
    .unwrap()
    .expect("an unresolved child is not a Workspace startup requirement");
    let owner = reopened
        .store
        .external_workdir_cleanup_operation(TEST_WORKSPACE_ID, &grant_id)
        .unwrap()
        .unwrap();
    assert_eq!(owner.operation().request, frozen);
    assert_eq!(
        owner.operation().runtime_status,
        Some(crate::hosts::RuntimeOperationStatus::Unknown)
    );
    assert!(owner.operation().runtime_dispatched);
    assert!(!owner.operation().settled);
    for failure in saved_failures {
        assert!(owner.operation().failures().contains(&failure));
    }
    assert_eq!(
        external_workdir_cleanup_reconciliation_count(&workdir_id),
        0
    );
    assert_eq!(
        reopened
            .store
            .external_workdir_cleanup_observation(TEST_WORKSPACE_ID, &grant_id)
            .unwrap()
            .0,
        "pending"
    );
}

#[tokio::test]
async fn external_grant_reused_alias_does_not_close_replacement_session_or_dispatch() {
    use crate::grant_cleanup_operation::{CleanupAdmission, CleanupRequest, CleanupRuntimeTarget};
    let fixture = grant_cleanup_fixture().await;
    let api = &fixture.api;
    let (client, _server, ..) = client(api).await;
    let grant = client
        .external_workdir_grant_create(TEST_WORKSPACE_ID.into(), request())
        .await
        .unwrap();
    let original = api
        .store
        .attach_worker_workdir(&WorkerWorkdirLinkRecord {
            connection_id: String::new(),
            workspace_id: TEST_WORKSPACE_ID.into(),
            worker: fixture.worker.clone(),
            workdir_id: grant.working_directory_id.clone(),
            alias: "reused-alias".into(),
            capabilities: workdir::WorkdirSessionCapabilities::READ_ONLY,
            linked_at: Utc::now().to_rfc3339(),
            unlinked_at: None,
        })
        .unwrap();
    let now = Utc::now().to_rfc3339();
    assert!(
        api.store
            .update_external_workdir_grant_state(
                TEST_WORKSPACE_ID,
                &grant.grant_id,
                &grant.provider_instance_id,
                grant.generation,
                "revoked",
                &now
            )
            .unwrap()
    );
    let request = CleanupRequest {
        operation_id: Uuid::now_v7().to_string(),
        workspace_id: TEST_WORKSPACE_ID.into(),
        grant_id: grant.grant_id.clone(),
        provider_instance_id: grant.provider_instance_id.clone(),
        grant_generation: grant.generation,
        working_directory_id: grant.working_directory_id.clone(),
        target: Some(CleanupRuntimeTarget {
            worker: fixture.worker.clone(),
            connection_id: original.connection_id.clone(),
            alias: original.alias.clone(),
            expected_generation: "not-dispatched-fixture-generation".into(),
            logical_workdir_attachments: Vec::new(),
        }),
    };
    let CleanupAdmission::Admitted(admitted) = api
        .store
        .begin_external_workdir_cleanup_operation(&request, &[], &now)
        .unwrap()
    else {
        panic!("fresh intent")
    };
    let detached = api
        .store
        .detach_worker_workdir_connection(
            TEST_WORKSPACE_ID,
            &fixture.worker,
            &original.alias,
            &original.connection_id,
            &now,
        )
        .unwrap();
    assert_eq!(detached.connection_id, original.connection_id);
    assert!(detached.unlinked_at.is_some());
    let replacement = api
        .store
        .attach_worker_workdir(&WorkerWorkdirLinkRecord {
            workdir_id: fixture.workdir_id.clone(),
            connection_id: String::new(),
            linked_at: Utc::now().to_rfc3339(),
            ..original.clone()
        })
        .unwrap();
    assert_ne!(replacement.connection_id, original.connection_id);
    let session = api
        .runtime
        .open_workdir_session(&fixture.worker.runtime_id, &fixture.workdir_id, None)
        .await
        .unwrap();
    api.workdir_sessions
        .lock()
        .unwrap()
        .attachments
        .insert(fixture.worker.clone(), session.clone());
    cleanup_external_workdir_attachments(api, &grant.working_directory_id)
        .await
        .expect_err("historical connection is not release authority");
    assert!(Arc::ptr_eq(
        api.workdir_sessions
            .lock()
            .unwrap()
            .attachments
            .get(&fixture.worker)
            .unwrap(),
        &session
    ));
    session
        .stat(workdir::StatRequest {
            path: workdir::WorkdirPath::new("README.md").unwrap(),
        })
        .await
        .expect("replacement Runtime session remains usable");
    assert_eq!(
        api.store
            .list_worker_workdir_links(TEST_WORKSPACE_ID, &fixture.worker)
            .unwrap(),
        vec![replacement]
    );
    let owner = api
        .store
        .external_workdir_cleanup_operation(TEST_WORKSPACE_ID, &grant.grant_id)
        .unwrap()
        .unwrap();
    assert_eq!(owner.operation().request, admitted.operation().request);
    assert!(!owner.operation().release_confirmed);
    assert!(!owner.operation().runtime_dispatched);
    assert_eq!(owner.operation().runtime_status, None);
    assert_eq!(
        external_workdir_cleanup_reconciliation_count(&grant.working_directory_id),
        0
    );
}
