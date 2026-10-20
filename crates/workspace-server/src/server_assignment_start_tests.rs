// Include from server.rs's `tests` module as a child module:
// mod assignment_start_tests { include!("server_assignment_start_tests.rs"); }
// These exercise the authenticated router with the existing in-process Runtime
// adapter; they do not start product processes or claim model/E2E coverage.
use super::*;

struct StartSource {
    identity: RuntimeIdentityMaterial,
    worker: RuntimeWorkerRef,
}

async fn start_source(api: &mut WorkspaceApi, runtime: &WorkdirlessFixtureRuntime) -> StartSource {
    let runtime_id = WorkdirlessFixtureRuntime::RUNTIME_ID;
    let identity = RuntimeIdentityMaterial::generate(runtime_id).unwrap();
    configure_runtime_request_auth(api, &identity, runtime_id);
    let memory = api
        .config_store
        .get_workspace_memory_settings(TEST_WORKSPACE_ID)
        .unwrap();
    let reservation = api
        .config_store
        .reserve_worker_create(
            TEST_WORKSPACE_ID,
            runtime_id,
            "start-orchestrator",
            "sha256:start-orchestrator",
            Some(crate::hosts::WORKSPACE_ORCHESTRATOR_SINGLETON_KEY),
            &memory,
        )
        .unwrap();
    let mut summary = WorkdirlessFixtureRuntime::worker_summary(
        WorkerCreateBinding {
            worker_id: reservation.worker_id,
            create_fingerprint: reservation.create_fingerprint,
        },
        &WorkerSpawnRequest {
            requested_worker_name: Some("Start Orchestrator".into()),
            singleton_key: Some(crate::hosts::WORKSPACE_ORCHESTRATOR_SINGLETON_KEY.into()),
            intent: WorkerSpawnIntent::WorkspaceCompanion,
            acceptance: WorkerSpawnAcceptanceRequirement::RunAccepted {
                expected_segments: 0,
            },
            profile: ProfileSelector::Builtin("builtin:orchestrator".into()),
            ticket_assignment: None,
            initial_submit: Vec::new(),
            workdir_attachment_requests: Vec::new(),
            resolved_workdir_attachment_requests: Vec::new(),
            resolved_workdir_attachments: Vec::new(),
            resolved_config_bundle: None,
            resolved_worker_observation_enabled: false,
            resolved_worker_observation_grants: Vec::new(),
            resolved_workspace_api: None,
            resolved_memory_settings: None,
            resolved_subjektiv_attached: false,
            resolved_control_operation: None,
            resolved_execution_grant: None,
        },
    );
    summary.tags = vec!["orchestrator".into(), "singleton".into()];
    let worker = summary.worker.clone();
    runtime.workers.lock().unwrap().push(summary);
    seed_worker_source_member(api, runtime_id, &worker.worker_id);
    api.config_store
        .complete_worker_create_reservation(TEST_WORKSPACE_ID, worker.worker_id.parse().unwrap())
        .unwrap();
    assert_eq!(find_workspace_orchestrator(api).unwrap().worker, worker);
    StartSource { identity, worker }
}

fn add_unfinished_start_dependency(api: &WorkspaceApi, ticket_id: &str) -> String {
    let backend = browser_ticket_backend(api).unwrap();
    let dependency = backend
        .create(ticket::NewTicket::new("Unfinished prerequisite"))
        .unwrap();
    backend
        .add_ticket_relation(
            ticket_id.into(),
            ticket::NewTicketRelation {
                kind: ticket::TicketRelationKind::DependsOn,
                target: dependency.id.clone(),
                note: Some("Partial work is intentional".into()),
                author: Some("owner".into()),
            },
        )
        .unwrap();
    dependency.id
}

fn start_http_request(
    source: Option<&StartSource>,
    token: &str,
    method: &str,
    path: &str,
    body: Value,
) -> Request<Body> {
    let body = serde_json::to_vec(&body).unwrap();
    match source {
        Some(source) => runtime_source_request(
            &source.identity,
            Some(&source.worker.worker_id),
            method,
            path,
            body,
        ),
        None => Request::builder()
            .method(method)
            .uri(path)
            .header(CONTENT_TYPE, "application/json")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::from(body))
            .unwrap(),
    }
}

async fn start_http_response(
    api: &WorkspaceApi,
    request: Request<Body>,
    expected: StatusCode,
) -> Value {
    let response = build_router(api.clone()).oneshot(request).await.unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(status, expected, "{}", String::from_utf8_lossy(&body));
    serde_json::from_slice(&body)
        .unwrap_or_else(|_| serde_json::json!({"message": String::from_utf8_lossy(&body)}))
}

fn observed_start_delegation(
    api: &WorkspaceApi,
    ticket_id: &str,
    source: Option<&StartSource>,
) -> Option<String> {
    source.map(|_| {
        api.store
            .get_current_ticket_role_assignment(
                TEST_WORKSPACE_ID,
                ticket_id,
                TicketAssignmentRole::Orchestrator,
            )
            .unwrap()
            .unwrap()
            .assignment_id
    })
}

fn start_worker_body(
    ticket_id: Option<&str>,
    attachments: Value,
    delegation: Option<&str>,
) -> Value {
    let mut body = serde_json::json!({
        "runtime_id": WorkdirlessFixtureRuntime::RUNTIME_ID,
        "display_name": "Partial investigation",
        "profile": "builtin:intake",
        "initial_submit": [],
        "workdir_attachments": attachments,
        "feature_connections": {},
        "control_operation_id": "partial-control-start"
    });
    if let Some(ticket_id) = ticket_id {
        body["ticket_assignment"] = serde_json::json!({
            "ticket_id": ticket_id,
            "operation_id": "partial-start"
        });
        if let Some(delegation) = delegation {
            body["ticket_assignment"]["orchestrator_assignment_id"] =
                Value::String(delegation.into());
        }
    }
    body
}

fn assert_start_dependency_diagnostic_only(api: &WorkspaceApi, ticket: &str, dependency: &str) {
    let Json(detail) = browser_ticket_detail(api, ticket).unwrap();
    assert_eq!(detail.relations.blockers.len(), 1);
    assert_eq!(detail.relations.blockers[0].blocking_ticket, dependency);
    assert!(!detail.action_eligibility.can_start_manual_worker);
    assert!(detail.current_worker.is_some());
    let Json(prerequisite) = browser_ticket_detail(api, dependency).unwrap();
    assert_eq!(prerequisite.state.as_str(), "open");
    assert!(prerequisite.assignments.is_empty());
    assert!(prerequisite.assignment_tracking.is_empty());
}

#[tokio::test]
async fn unfinished_dependencies_allow_manual_and_orchestrator_resource_free_start_and_assign() {
    for orchestrator in [false, true] {
        for existing in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let mut api = test_api(directory.path()).await;
            register_test_runtime(&api, WorkdirlessFixtureRuntime::RUNTIME_ID).await;
            let runtime = WorkdirlessFixtureRuntime::default();
            api.runtime.register_or_replace(runtime.clone());
            let source = if orchestrator {
                Some(start_source(&mut api, &runtime).await)
            } else {
                None
            };
            let token =
                seed_test_workspace_owner_token(api.store.as_ref(), "partial-start-owner").await;
            let ticket = browser_ticket_backend(&api)
                .unwrap()
                .create(ticket::NewTicket::new(
                    "Partial resource-free investigation",
                ))
                .unwrap();
            let dependency = add_unfinished_start_dependency(&api, &ticket.id);
            if orchestrator {
                assign_test_orchestrator(&api, &ticket.id);
            }
            let delegation = observed_start_delegation(&api, &ticket.id, source.as_ref());
            assert!(
                browser_ticket_detail(&api, &ticket.id)
                    .unwrap()
                    .0
                    .action_eligibility
                    .can_start_manual_worker
            );
            let create_path = if orchestrator {
                format!("/api/w/{TEST_WORKSPACE_ID}/worker-control/workers")
            } else {
                format!("/api/w/{TEST_WORKSPACE_ID}/workers")
            };
            let created = start_http_response(
                &api,
                start_http_request(
                    source.as_ref(),
                    &token,
                    "POST",
                    &create_path,
                    start_worker_body(
                        (!existing).then_some(ticket.id.as_str()),
                        serde_json::json!([]),
                        delegation.as_deref(),
                    ),
                ),
                StatusCode::OK,
            )
            .await;
            let worker_id = created["worker_id"].as_str().unwrap();
            assert_eq!(created["worker"]["profile"], "builtin:intake");
            if existing {
                let path = format!(
                    "/api/w/{TEST_WORKSPACE_ID}/tickets/{}/assignments/worker",
                    ticket.id
                );
                let mut body = serde_json::json!({
                    "operation_id": "partial-existing-assign",
                    "principal": {
                        "kind": "worker",
                        "runtime_id": WorkdirlessFixtureRuntime::RUNTIME_ID,
                        "worker_id": worker_id
                    },
                    "workdir_bindings": []
                });
                if let Some(delegation) = &delegation {
                    body["orchestrator_assignment_id"] = Value::String(delegation.clone());
                }
                let assigned = start_http_response(
                    &api,
                    start_http_request(source.as_ref(), &token, "PUT", &path, body.clone()),
                    StatusCode::OK,
                )
                .await;
                let replay = start_http_response(
                    &api,
                    start_http_request(source.as_ref(), &token, "PUT", &path, body),
                    StatusCode::OK,
                )
                .await;
                assert_eq!(assigned, replay);
            }
            let worker = RuntimeWorkerRef::new(WorkdirlessFixtureRuntime::RUNTIME_ID, worker_id);
            assert!(
                api.store
                    .list_worker_workdir_links(TEST_WORKSPACE_ID, &worker)
                    .unwrap()
                    .is_empty()
            );
            assert_start_dependency_diagnostic_only(&api, &ticket.id, &dependency);
            // Partial work never permits a second unfinished Worker.
            let mut competing = start_worker_body(
                Some(&ticket.id),
                serde_json::json!([]),
                delegation.as_deref(),
            );
            competing["ticket_assignment"]["operation_id"] =
                Value::String("competing-start".into());
            competing["control_operation_id"] = Value::String("competing-control-start".into());
            let spawn_count = runtime.spawn_requests().len();
            start_http_response(
                &api,
                start_http_request(source.as_ref(), &token, "POST", &create_path, competing),
                StatusCode::CONFLICT,
            )
            .await;
            assert_eq!(runtime.spawn_requests().len(), spawn_count);
            if !orchestrator {
                assert!(
                    api.store
                        .get_current_ticket_role_assignment(
                            TEST_WORKSPACE_ID,
                            &ticket.id,
                            TicketAssignmentRole::Orchestrator
                        )
                        .unwrap()
                        .is_none()
                );
            }
        }
    }
}

#[tokio::test]
async fn unfinished_dependencies_allow_read_only_start_and_assign_without_write_or_command_grants()
{
    for orchestrator in [false, true] {
        for existing in [false, true] {
            let mut fixture = manual_worker_assignment_fixture().await;
            let source = if orchestrator {
                Some(start_source(&mut fixture.api, &fixture.runtime).await)
            } else {
                None
            };
            let api = &fixture.api;
            // Spawn finalization observes Runtime Workdir identity; do not use
            // the adapter's unrelated default Repository id for this checkout.
            fixture.runtime.set_workdir_repository(
                &fixture.docs_workdir_id,
                &test_repository_id_by_key(api, "docs"),
            );
            let backend = browser_ticket_backend(api).unwrap();
            // Keep only the reference target. Partial investigation needs no writable Repository.
            backend
                .edit_item(
                    fixture.ticket_id.clone().into(),
                    ticket::TicketItemEdit {
                        targets: Some(ticket::TicketTargetsEdit::Set {
                            targets: vec![test_ticket_target(
                                "docs",
                                "develop",
                                TicketTargetAccess::ReadOnly,
                            )],
                        }),
                        ..Default::default()
                    },
                )
                .unwrap();
            let dependency = add_unfinished_start_dependency(api, &fixture.ticket_id);
            if orchestrator {
                assign_test_orchestrator(api, &fixture.ticket_id);
            }
            let delegation = observed_start_delegation(api, &fixture.ticket_id, source.as_ref());
            let token =
                seed_test_workspace_owner_token(api.store.as_ref(), "read-only-start-owner").await;
            let worker = if existing {
                // Drop the unrelated checkout so the entire attachment set is reference-only.
                let main = api
                    .store
                    .list_worker_workdir_links(TEST_WORKSPACE_ID, &fixture.worker)
                    .unwrap()
                    .into_iter()
                    .find(|link| link.workdir_id == fixture.main_workdir_id)
                    .unwrap();
                api.store
                    .detach_worker_workdir_connection(
                        TEST_WORKSPACE_ID,
                        &fixture.worker,
                        &main.alias,
                        &main.connection_id,
                        &now_registry_timestamp(),
                    )
                    .unwrap();
                sync_runtime_worker_workdir_attachments(api, &fixture.worker).unwrap();
                let mut body = serde_json::to_value(manual_worker_assignment_request(
                    &fixture,
                    "read-only-existing",
                ))
                .unwrap();
                if let Some(delegation) = &delegation {
                    body["orchestrator_assignment_id"] = Value::String(delegation.clone());
                }
                body["workdir_bindings"] = serde_json::json!([{
                    "alias": "docs", "working_directory_id": fixture.docs_workdir_id,
                    "connection_id": api.store.list_worker_workdir_links(TEST_WORKSPACE_ID, &fixture.worker).unwrap().into_iter().find(|link| link.workdir_id == fixture.docs_workdir_id && link.unlinked_at.is_none()).unwrap().connection_id
                }]);
                let path = format!(
                    "/api/w/{TEST_WORKSPACE_ID}/tickets/{}/assignments/worker",
                    fixture.ticket_id
                );
                start_http_response(
                    api,
                    start_http_request(source.as_ref(), &token, "PUT", &path, body),
                    StatusCode::OK,
                )
                .await;
                fixture.worker.clone()
            } else {
                // This matrix tests read-only start, not sharing an occupied
                // Workdir. Release the fixture candidate's reference attachment.
                let docs = api
                    .store
                    .list_worker_workdir_links(TEST_WORKSPACE_ID, &fixture.worker)
                    .unwrap()
                    .into_iter()
                    .find(|link| {
                        link.workdir_id == fixture.docs_workdir_id && link.unlinked_at.is_none()
                    })
                    .unwrap();
                api.store
                    .detach_worker_workdir_connection(
                        TEST_WORKSPACE_ID,
                        &fixture.worker,
                        &docs.alias,
                        &docs.connection_id,
                        &now_registry_timestamp(),
                    )
                    .unwrap();
                sync_runtime_worker_workdir_attachments(api, &fixture.worker).unwrap();
                let path = if orchestrator {
                    format!("/api/w/{TEST_WORKSPACE_ID}/worker-control/workers")
                } else {
                    format!("/api/w/{TEST_WORKSPACE_ID}/workers")
                };
                let body = start_worker_body(
                    Some(&fixture.ticket_id),
                    serde_json::json!([{
                        "alias": "docs", "working_directory_id": fixture.docs_workdir_id
                    }]),
                    delegation.as_deref(),
                );
                let created = start_http_response(
                    api,
                    start_http_request(source.as_ref(), &token, "POST", &path, body),
                    StatusCode::OK,
                )
                .await;
                assert_eq!(created["worker"]["profile"], "builtin:intake");
                RuntimeWorkerRef::new(
                    WorkdirlessFixtureRuntime::RUNTIME_ID,
                    created["worker_id"].as_str().unwrap(),
                )
            };
            let links = api
                .store
                .list_worker_workdir_links(TEST_WORKSPACE_ID, &worker)
                .unwrap()
                .into_iter()
                .filter(|link| link.unlinked_at.is_none())
                .collect::<Vec<_>>();
            assert_eq!(links.len(), 1);
            assert_eq!(links[0].workdir_id, fixture.docs_workdir_id);
            assert_eq!(
                links[0].capabilities,
                workdir::WorkdirSessionCapabilities::READ_ONLY
            );
            if existing {
                let attachments = fixture.runtime.logical_attachments(&worker.worker_id);
                assert_eq!(attachments.len(), 1);
                assert_eq!(
                    attachments[0].capabilities,
                    workdir::WorkdirSessionCapabilities::READ_ONLY
                );
            } else {
                // This adapter records spawn inputs separately from the
                // replacement-attachment calls used by existing-Worker assign.
                let request = fixture.runtime.spawn_requests().last().unwrap().clone();
                assert_eq!(request.resolved_workdir_attachments.len(), 1);
                assert_eq!(
                    request.resolved_workdir_attachments[0].working_directory_id,
                    fixture.docs_workdir_id
                );
                assert_eq!(
                    request.resolved_workdir_attachments[0].capabilities,
                    workdir::WorkdirSessionCapabilities::READ_ONLY
                );
            }
            assert_start_dependency_diagnostic_only(api, &fixture.ticket_id, &dependency);
        }
    }
}

#[tokio::test]
async fn unfinished_dependencies_do_not_bypass_http_auth_closed_or_resource_binding_guards() {
    let fixture = manual_worker_assignment_fixture().await;
    let api = &fixture.api;
    add_unfinished_start_dependency(api, &fixture.ticket_id);
    let token = seed_test_workspace_owner_token(api.store.as_ref(), "guard-start-owner").await;
    let path = format!(
        "/api/w/{TEST_WORKSPACE_ID}/tickets/{}/assignments/worker",
        fixture.ticket_id
    );
    let valid =
        serde_json::to_value(manual_worker_assignment_request(&fixture, "guarded-assign")).unwrap();
    let denied = start_http_response(
        api,
        start_http_request(None, "invalid-token", "PUT", &path, valid.clone()),
        StatusCode::UNAUTHORIZED,
    )
    .await;
    assert!(denied["message"].is_string());
    let mut stale = valid.clone();
    stale["workdir_bindings"][0]["connection_id"] = Value::String("stale-connection".into());
    let denied = start_http_response(
        api,
        start_http_request(None, &token, "PUT", &path, stale),
        StatusCode::CONFLICT,
    )
    .await;
    assert!(denied["message"].as_str().unwrap().contains("connection"));
    assert!(
        api.store
            .get_active_ticket_worker_assignment(TEST_WORKSPACE_ID, &fixture.ticket_id)
            .unwrap()
            .is_none()
    );
    // The existing case/work-release trigger is separate from relation diagnostics.
    rusqlite::Connection::open(&api.config.database_path).unwrap().execute(
        "UPDATE typed_tickets SET workflow_state='closed' WHERE workspace_id=?1 AND ticket_id=?2",
        rusqlite::params![TEST_WORKSPACE_ID, fixture.ticket_id],
    ).unwrap();
    let denied = start_http_response(
        api,
        start_http_request(None, &token, "PUT", &path, valid),
        StatusCode::CONFLICT,
    )
    .await;
    assert!(
        denied["message"].as_str().unwrap().contains("open")
            || denied["message"].as_str().unwrap().contains("closed")
    );
    assert!(
        api.store
            .get_active_ticket_worker_assignment(TEST_WORKSPACE_ID, &fixture.ticket_id)
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn orchestrator_start_and_assign_reject_missing_stale_cleared_or_reissued_delegation() {
    for existing in [false, true] {
        for invalidation in ["missing", "stale", "clear", "reissue"] {
            let directory = tempfile::tempdir().unwrap();
            let mut api = test_api(directory.path()).await;
            register_test_runtime(&api, WorkdirlessFixtureRuntime::RUNTIME_ID).await;
            let runtime = WorkdirlessFixtureRuntime::default();
            api.runtime.register_or_replace(runtime.clone());
            let source = start_source(&mut api, &runtime).await;
            let token =
                seed_test_workspace_owner_token(api.store.as_ref(), "delegation-owner").await;
            let ticket = browser_ticket_backend(&api)
                .unwrap()
                .create(ticket::NewTicket::new("Observed delegation start"))
                .unwrap();
            assign_test_orchestrator(&api, &ticket.id);
            let observed = observed_start_delegation(&api, &ticket.id, Some(&source)).unwrap();
            let worker = if existing {
                Some(
                    start_http_response(
                        &api,
                        start_http_request(
                            None,
                            &token,
                            "POST",
                            &format!("/api/w/{TEST_WORKSPACE_ID}/workers"),
                            start_worker_body(None, serde_json::json!([]), None),
                        ),
                        StatusCode::OK,
                    )
                    .await,
                )
            } else {
                None
            };
            if matches!(invalidation, "clear" | "reissue") {
                let clear = format!(
                    "/api/w/{TEST_WORKSPACE_ID}/tickets/{}/assignments/orchestrator?assignment_id={observed}&operation_id=clear-observed",
                    ticket.id
                );
                start_http_response(
                    &api,
                    start_http_request(None, &token, "DELETE", &clear, Value::Null),
                    StatusCode::OK,
                )
                .await;
                if invalidation == "reissue" {
                    let path = format!(
                        "/api/w/{TEST_WORKSPACE_ID}/tickets/{}/assignments/orchestrator",
                        ticket.id
                    );
                    start_http_response(&api, start_http_request(None, &token, "PUT", &path,
                        serde_json::json!({"operation_id":"reissue-observed", "principal":{"kind":"workspace_agent", "agent_key":"workspace-orchestrator"}})), StatusCode::OK).await;
                    assert_ne!(
                        observed_start_delegation(&api, &ticket.id, Some(&source)).unwrap(),
                        observed
                    );
                }
            }
            let submitted = match invalidation {
                "missing" => None,
                "stale" => Some("unrelated-assignment"),
                _ => Some(observed.as_str()),
            };
            let (method, path, body) = if let Some(worker) = worker {
                let mut body = serde_json::json!({
                    "operation_id":"invalid-existing-start",
                    "principal":{"kind":"worker", "runtime_id":WorkdirlessFixtureRuntime::RUNTIME_ID, "worker_id":worker["worker_id"]},
                    "workdir_bindings":[]
                });
                if let Some(submitted) = submitted {
                    body["orchestrator_assignment_id"] = Value::String(submitted.into());
                }
                (
                    "PUT",
                    format!(
                        "/api/w/{TEST_WORKSPACE_ID}/tickets/{}/assignments/worker",
                        ticket.id
                    ),
                    body,
                )
            } else {
                (
                    "POST",
                    format!("/api/w/{TEST_WORKSPACE_ID}/worker-control/workers"),
                    start_worker_body(Some(&ticket.id), serde_json::json!([]), submitted),
                )
            };
            let spawns = runtime.spawn_requests().len();
            let denied = start_http_response(
                &api,
                start_http_request(Some(&source), &token, method, &path, body),
                StatusCode::CONFLICT,
            )
            .await;
            assert!(
                denied["message"].as_str().unwrap().contains("assignment")
                    || denied["message"].as_str().unwrap().contains("delegation"),
                "{invalidation}: {denied}"
            );
            assert_eq!(runtime.spawn_requests().len(), spawns);
            assert!(
                api.store
                    .get_active_ticket_worker_assignment(TEST_WORKSPACE_ID, &ticket.id)
                    .unwrap()
                    .is_none()
            );
            let operation = if existing {
                "invalid-existing-start"
            } else {
                "partial-start"
            };
            assert!(
                api.store
                    .get_ticket_assignment_operation(TEST_WORKSPACE_ID, operation)
                    .unwrap()
                    .is_none()
            );
        }
    }
}

#[tokio::test]
async fn replaced_orchestrator_cannot_start_or_assign_under_retained_current_delegation() {
    for existing in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let mut api = test_api(directory.path()).await;
        register_test_runtime(&api, WorkdirlessFixtureRuntime::RUNTIME_ID).await;
        let runtime = WorkdirlessFixtureRuntime::default();
        api.runtime.register_or_replace(runtime.clone());
        let source = start_source(&mut api, &runtime).await;
        let token =
            seed_test_workspace_owner_token(api.store.as_ref(), "obsolete-worker-owner").await;
        let ticket = browser_ticket_backend(&api)
            .unwrap()
            .create(ticket::NewTicket::new(
                "Retained delegation after Worker replacement",
            ))
            .unwrap();
        assign_test_orchestrator(&api, &ticket.id);
        let delegation = observed_start_delegation(&api, &ticket.id, Some(&source)).unwrap();
        let worker = if existing {
            Some(
                start_http_response(
                    &api,
                    start_http_request(
                        None,
                        &token,
                        "POST",
                        &format!("/api/w/{TEST_WORKSPACE_ID}/workers"),
                        start_worker_body(None, serde_json::json!([]), None),
                    ),
                    StatusCode::OK,
                )
                .await,
            )
        } else {
            None
        };
        // Backend ownership changes independently of the retained assignment row.
        let replacement =
            RuntimeWorkerRef::new(&source.worker.runtime_id, WorkerId::now_v7().to_string());
        seed_worker_source_member(&api, &replacement.runtime_id, &replacement.worker_id);
        let mut summary = runtime.workers.lock().unwrap()[0].clone();
        summary.worker = replacement.clone();
        runtime.workers.lock().unwrap().push(summary);
        rusqlite::Connection::open(&api.config.database_path).unwrap().execute(
            "UPDATE worker_singleton_owners SET worker_id=?1,generation=generation+1 WHERE workspace_id=?2 AND singleton_key=?3",
            rusqlite::params![replacement.worker_id, TEST_WORKSPACE_ID, crate::hosts::WORKSPACE_ORCHESTRATOR_SINGLETON_KEY],
        ).unwrap();
        assert_eq!(
            find_workspace_orchestrator(&api).unwrap().worker,
            replacement
        );
        assert_eq!(
            observed_start_delegation(&api, &ticket.id, Some(&source)).unwrap(),
            delegation
        );
        let (method, path, body) = if let Some(worker) = worker {
            (
                "PUT",
                format!(
                    "/api/w/{TEST_WORKSPACE_ID}/tickets/{}/assignments/worker",
                    ticket.id
                ),
                serde_json::json!({
                    "operation_id":"obsolete-existing-start", "orchestrator_assignment_id":delegation,
                    "principal":{"kind":"worker", "runtime_id":WorkdirlessFixtureRuntime::RUNTIME_ID, "worker_id":worker["worker_id"]},
                    "workdir_bindings":[]
                }),
            )
        } else {
            (
                "POST",
                format!("/api/w/{TEST_WORKSPACE_ID}/worker-control/workers"),
                start_worker_body(Some(&ticket.id), serde_json::json!([]), Some(&delegation)),
            )
        };
        let spawns = runtime.spawn_requests().len();
        let denied = start_http_response(
            &api,
            start_http_request(Some(&source), &token, method, &path, body),
            // The authenticated Worker-source middleware fences the replaced
            // singleton before the start/assign handler is admitted.
            StatusCode::UNAUTHORIZED,
        )
        .await;
        assert!(denied["message"].is_string());
        assert_eq!(runtime.spawn_requests().len(), spawns);
        assert!(
            api.store
                .get_active_ticket_worker_assignment(TEST_WORKSPACE_ID, &ticket.id)
                .unwrap()
                .is_none()
        );
    }
}
