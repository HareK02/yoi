// Include from server.rs's `tests` module as a child module:
// mod assignment_start_tests { include!("server_assignment_start_tests.rs"); }
// These exercise the authenticated router with the existing in-process Runtime
// adapter; they do not start product processes or claim model/E2E coverage.
use super::*;

struct StartSource {
    identity: RuntimeIdentityMaterial,
    worker: RuntimeWorkerRef,
    authorization: server_api::launch_policy::WorkerCreationAuthorization,
    resource_free_authorization: server_api::launch_policy::WorkerCreationAuthorization,
}

async fn start_source(api: &mut WorkspaceApi, _runtime: &WorkdirlessFixtureRuntime) -> StartSource {
    start_body_source(api, WorkdirlessFixtureRuntime::RUNTIME_ID).await
}

async fn start_body_source(api: &mut WorkspaceApi, runtime_id: &str) -> StartSource {
    let identity = RuntimeIdentityMaterial::generate(runtime_id).unwrap();
    configure_runtime_request_auth(api, &identity, runtime_id);
    let subject_id = ensure_test_assignment_subject(api);
    let worker = create_test_assignment_body(api, runtime_id, &subject_id).await;
    for ticket in api.authority.list_tickets(100).unwrap().items {
        grant_test_assignment_ticket(api, &subject_id, &ticket.id);
    }
    let child_permissions = test_operation_assignment(api, &["filesystem.read"]);
    let (_, authorization) = test_worker_policy_authorizations(
        api,
        "assignment-read-only-start",
        "builtin:intake",
        child_permissions.clone(),
    );
    let (_, resource_free_authorization) = test_worker_policy_authorizations(
        api,
        "assignment-resource-free-start",
        "builtin:intake",
        Default::default(),
    );
    // Publish the dynamic usage key before assigning it through the owner domain.
    let root =
        ServerConfig::server_data_root_for_data_dir(&api.config.data_root).join("plugin-artifacts");
    api.config_store
        .sync_plugin_permission_registry(
            &root,
            TEST_WORKSPACE_ID,
            &mut api.permission_registry.write().unwrap(),
        )
        .unwrap();
    extend_test_assignment_operations(
        api,
        &subject_id,
        &[
            "worker.create",
            "worker.send_input",
            "worker.launch.assignment-read-only-start",
            "worker.launch.assignment-resource-free-start",
        ],
    );
    let previous = api
        .config_store
        .current_subject_permission_policy(TEST_WORKSPACE_ID, &subject_id)
        .unwrap()
        .unwrap();
    set_test_subject_policy_as_owner(api, &subject_id, previous.execution, child_permissions);
    StartSource {
        identity,
        worker,
        authorization,
        resource_free_authorization,
    }
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
    mut body: Value,
) -> Request<Body> {
    if let Some(source) =
        source.filter(|_| method == "POST" && path.ends_with("/worker-control/workers"))
    {
        let authorization = if body["workdir_attachments"].as_array().unwrap().is_empty() {
            &source.resource_free_authorization
        } else {
            &source.authorization
        };
        body["authorization"] = serde_json::to_value(authorization).unwrap();
    }
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

fn start_worker_body(attachments: Value) -> Value {
    let authorization = if attachments.as_array().unwrap().is_empty() {
        test_worker_operation_authorization(&[])
    } else {
        test_worker_operation_authorization(&[("filesystem", &["filesystem.read"])])
    };
    serde_json::json!({
        "runtime_id": WorkdirlessFixtureRuntime::RUNTIME_ID,
        "display_name": "Partial investigation",
        "profile": "builtin:intake",
        "initial_submit": [],
        "workdir_attachments": attachments,
        "feature_connections": {},
        "control_operation_id": "partial-control-start",
        "authorization": authorization
    })
}

fn start_assignment_body(
    api: &WorkspaceApi,
    worker: &RuntimeWorkerRef,
    operation_id: &str,
    delegation: Option<&str>,
) -> Value {
    // Claims name current connections from the authority, never spawn inputs or
    // a cached fixture connection. Exact retries below retain the original body.
    let workdir_bindings = api
        .store
        .list_worker_workdir_links(TEST_WORKSPACE_ID, worker)
        .unwrap()
        .into_iter()
        .filter(|link| link.unlinked_at.is_none())
        .map(|link| server_api::TicketWorkerAttachmentBinding {
            alias: link.alias,
            working_directory_id: link.workdir_id,
            connection_id: link.connection_id,
        })
        .collect::<Vec<_>>();
    serde_json::to_value(server_api::SetTicketRoleAssignmentRequest {
        operation_id: operation_id.into(),
        principal: server_api::TicketAssignmentPrincipal::Worker {
            runtime_id: worker.runtime_id.clone(),
            worker_id: worker.worker_id.clone(),
        },
        orchestrator_assignment_id: delegation.map(str::to_owned),
        expected_assignment_id: None,
        workdir_bindings,
    })
    .unwrap()
}

async fn spawn_start_worker(
    api: &WorkspaceApi,
    source: Option<&StartSource>,
    token: &str,
    body: Value,
) -> RuntimeWorkerRef {
    assert_eq!(body["initial_submit"], serde_json::json!([]));
    assert!(body.get("ticket_assignment").is_none());
    assert!(body.get("ticket_role").is_none());
    let path = if source.is_some() {
        format!("/api/w/{TEST_WORKSPACE_ID}/worker-control/workers")
    } else {
        format!("/api/w/{TEST_WORKSPACE_ID}/workers")
    };
    let created = start_http_response(
        api,
        start_http_request(source, token, "POST", &path, body),
        StatusCode::OK,
    )
    .await;
    assert_eq!(created["worker"]["profile"], "builtin:intake");
    RuntimeWorkerRef::new(
        WorkdirlessFixtureRuntime::RUNTIME_ID,
        created["worker_id"].as_str().unwrap(),
    )
}

// Existing owner-created fixture candidates need explicit Worker control too;
// Ticket delegation alone is not authority over an arbitrary existing Worker.
fn grant_start_candidate_control(
    api: &WorkspaceApi,
    source: &StartSource,
    worker: &RuntimeWorkerRef,
) {
    api.store
        .create_worker_control_grant(&WorkerControlGrantRecord {
            workspace_id: TEST_WORKSPACE_ID.into(),
            grant_id: format!("claim-control-{}", worker.worker_id),
            controller: source.worker.clone(),
            subject: worker.clone(),
            relation: "spawned".into(),
            origin: "test".into(),
            permissions: vec!["send_input".into()],
            operation_id: format!("claim-control-{}", worker.worker_id),
            created_at: now_registry_timestamp(),
            revoked_at: None,
        })
        .unwrap();
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
async fn ticket_assignment_rechecks_changed_target_at_admission_and_finalization() {
    for (phase, change) in [
        ("admission", "remove_repository"),
        ("finalization", "access='read_only'"),
        ("finalization", "ref_selector='different-branch'"),
    ] {
        let fixture = guarded_spawn_fixture().await;
        let api = &fixture.api;
        // Selector values are provenance, not attachment eligibility. Target
        // edits during a claim still participate in its current-content fence.
        browser_ticket_backend(api)
            .unwrap()
            .edit_item(
                fixture.ticket_id.clone().into(),
                ticket::TicketItemEdit {
                    targets: Some(ticket::TicketTargetsEdit::Set {
                        targets: vec![
                            test_ticket_target(
                                "test-repository",
                                "HEAD",
                                TicketTargetAccess::ReadWrite,
                            ),
                            test_ticket_target("docs", "HEAD", TicketTargetAccess::ReadOnly),
                        ],
                    }),
                    ..Default::default()
                },
            )
            .unwrap();
        let token = seed_test_workspace_owner_token(api.store.as_ref(), "target-guard-owner").await;
        let worker = spawn_start_worker(
            api,
            None,
            &token,
            start_worker_body(serde_json::json!([
                {"alias": "checkout", "working_directory_id": fixture.workdir_id},
                {"alias": "docs", "working_directory_id": fixture.read_only_workdir_id}
            ])),
        )
        .await;
        // Ordinary spawn does not inspect or claim Ticket work. The claim names
        // its live connections, with no initial work submitted.
        assert!(
            api.store
                .get_active_ticket_worker_assignment(TEST_WORKSPACE_ID, &fixture.ticket_id)
                .unwrap()
                .is_none()
        );
        // Prepare an already-narrowed candidate so claim finalization needs no
        // stop/rebind/restore. Ordinary spawn no longer narrows by Ticket targets.
        let mut links = api
            .store
            .list_worker_workdir_links(TEST_WORKSPACE_ID, &worker)
            .unwrap();
        assert_eq!(links.len(), 2);
        for link in &mut links {
            link.capabilities = workdir::WorkdirSessionCapabilities::READ_ONLY;
        }
        api.store
            .replace_worker_workdir_link_capabilities(TEST_WORKSPACE_ID, &worker, &links)
            .unwrap();
        sync_runtime_worker_workdir_attachments(api, &worker).unwrap();
        let body = start_assignment_body(api, &worker, "target-changed-claim", None);
        let attachments = fixture.runtime.logical_attachments(&worker.worker_id);
        let spawns = fixture.runtime.spawn_requests().len();
        let restores = *fixture.runtime.restore_attempts.lock().unwrap();
        let mut runtime = fixture.runtime.clone();
        runtime.stop_callback = Some(Arc::new(|| {
            panic!("target guard must reject before stopping the Worker")
        }));
        api.runtime.register_or_replace(runtime);
        let conn = rusqlite::Connection::open(&api.config.database_path).unwrap();
        if phase == "admission" {
            conn.execute_batch(&format!(
                "DELETE FROM typed_ticket_targets
                 WHERE ticket_id='{}' AND repository_key='test-repository';",
                fixture.ticket_id,
            ))
            .unwrap();
        } else {
            // Interleave after resource validation and claim admission, before
            // unchanged-binding finalization; no Runtime mutation is needed.
            conn.execute_batch(&format!(
                "CREATE TRIGGER change_target_after_claim AFTER INSERT ON ticket_assignment_operations
                 WHEN NEW.operation_id='target-changed-claim'
                 BEGIN UPDATE typed_ticket_targets SET {change}
                   WHERE ticket_id='{}' AND repository_key='test-repository'; END;",
                fixture.ticket_id,
            )).unwrap();
        }
        let path = format!(
            "/api/w/{TEST_WORKSPACE_ID}/tickets/{}/assignments/worker",
            fixture.ticket_id
        );
        let denied = start_http_response(
            api,
            start_http_request(None, &token, "PUT", &path, body),
            if phase == "admission" {
                StatusCode::BAD_REQUEST
            } else {
                StatusCode::CONFLICT
            },
        )
        .await;
        let message = denied["message"].as_str().unwrap();
        assert!(
            if phase == "admission" {
                message.contains("undeclared Repository")
            } else {
                message.contains("changed")
            },
            "{phase}: {change}: {denied}"
        );
        assert!(
            api.store
                .get_active_ticket_worker_assignment(TEST_WORKSPACE_ID, &fixture.ticket_id)
                .unwrap()
                .is_none()
        );
        let operation = api
            .store
            .get_ticket_assignment_operation(TEST_WORKSPACE_ID, "target-changed-claim")
            .unwrap();
        if phase == "admission" {
            assert!(operation.is_none());
        } else {
            let operation = operation.unwrap();
            assert_eq!(operation.claim_state, "failed");
            assert!(operation.assignment_id.is_none());
        }
        assert_eq!(fixture.runtime.spawn_requests().len(), spawns);
        assert_eq!(*fixture.runtime.restore_attempts.lock().unwrap(), restores);
        assert!(fixture.runtime.input_requests.lock().unwrap().is_empty());
        assert_eq!(
            api.store
                .list_worker_workdir_links(TEST_WORKSPACE_ID, &worker)
                .unwrap(),
            links
        );
        assert_eq!(api.runtime.worker(&worker).unwrap().state, "idle");
        assert_eq!(
            fixture.runtime.logical_attachments(&worker.worker_id),
            attachments
        );
        assert_eq!(
            conn.query_row(
                "SELECT count(*) FROM worker_workdir_attachment_reservations",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
    }
}

#[tokio::test]
async fn unfinished_dependencies_allow_manual_and_orchestrator_resource_free_spawn_then_assign() {
    for orchestrator in [false, true] {
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
        let subject_assignment = api
            .store
            .get_current_ticket_role_assignment(
                TEST_WORKSPACE_ID,
                &ticket.id,
                TicketAssignmentRole::Orchestrator,
            )
            .unwrap();
        if let Some(assignment) = &subject_assignment {
            assert_eq!(
                assignment.principal,
                TicketAssignmentPrincipal::Subject {
                    subject_id: ensure_test_assignment_subject(&api)
                }
            );
        }
        let delegation = observed_start_delegation(&api, &ticket.id, source.as_ref());
        assert!(
            browser_ticket_detail(&api, &ticket.id)
                .unwrap()
                .0
                .action_eligibility
                .can_start_manual_worker
        );
        let worker = spawn_start_worker(
            &api,
            source.as_ref(),
            &token,
            start_worker_body(serde_json::json!([])),
        )
        .await;
        assert!(
            api.store
                .get_active_ticket_worker_assignment(TEST_WORKSPACE_ID, &ticket.id)
                .unwrap()
                .is_none()
        );
        let path = format!(
            "/api/w/{TEST_WORKSPACE_ID}/tickets/{}/assignments/worker",
            ticket.id
        );
        let body = start_assignment_body(
            &api,
            &worker,
            "partial-existing-assign",
            delegation.as_deref(),
        );
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
        let assignment = api
            .store
            .get_active_ticket_worker_assignment(TEST_WORKSPACE_ID, &ticket.id)
            .unwrap()
            .unwrap();
        assert_eq!(assignment.worker, worker);
        assert_eq!(
            api.store
                .get_current_ticket_role_assignment(
                    TEST_WORKSPACE_ID,
                    &ticket.id,
                    TicketAssignmentRole::Orchestrator
                )
                .unwrap(),
            subject_assignment
        );
        assert!(
            api.store
                .list_worker_workdir_links(TEST_WORKSPACE_ID, &worker)
                .unwrap()
                .is_empty()
        );
        assert_start_dependency_diagnostic_only(&api, &ticket.id, &dependency);
        // A competing ordinary spawn is valid; only its Ticket claim conflicts.
        // Keep the created Worker addressable without dispatching any work.
        let mut competing_body = start_worker_body(serde_json::json!([]));
        competing_body["control_operation_id"] = Value::String("competing-control-start".into());
        let competing = spawn_start_worker(&api, source.as_ref(), &token, competing_body).await;
        let spawns = runtime.spawn_requests().len();
        let claim =
            start_assignment_body(&api, &competing, "competing-claim", delegation.as_deref());
        start_http_response(
            &api,
            start_http_request(source.as_ref(), &token, "PUT", &path, claim),
            StatusCode::CONFLICT,
        )
        .await;
        assert_eq!(runtime.spawn_requests().len(), spawns);
        assert_eq!(api.runtime.worker(&competing).unwrap().state, "idle");
        assert!(
            api.store
                .get_ticket_assignment_operation(TEST_WORKSPACE_ID, "competing-claim")
                .unwrap()
                .is_none()
        );
        assert_eq!(
            api.store
                .get_active_ticket_worker_assignment(TEST_WORKSPACE_ID, &ticket.id)
                .unwrap()
                .unwrap()
                .assignment_id,
            assignment.assignment_id
        );
        assert!(runtime.input_requests.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn unfinished_dependencies_allow_read_only_spawn_then_assign_without_write_or_command_grants()
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
                                if existing { "develop" } else { "HEAD" },
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
                spawn_start_worker(
                    api,
                    source.as_ref(),
                    &token,
                    start_worker_body(serde_json::json!([{
                        "alias": "docs", "working_directory_id": fixture.docs_workdir_id
                    }])),
                )
                .await
            };
            // Both an existing fixture Worker and an ordinary newly spawned
            // Worker are claimed through the same existing-Worker PUT contract.
            assert!(
                api.store
                    .get_active_ticket_worker_assignment(TEST_WORKSPACE_ID, &fixture.ticket_id)
                    .unwrap()
                    .is_none()
            );
            let path = format!(
                "/api/w/{TEST_WORKSPACE_ID}/tickets/{}/assignments/worker",
                fixture.ticket_id
            );
            if existing && let Some(source) = &source {
                grant_start_candidate_control(api, source, &worker);
            }
            let body =
                start_assignment_body(api, &worker, "read-only-existing", delegation.as_deref());
            start_http_response(
                api,
                start_http_request(source.as_ref(), &token, "PUT", &path, body),
                StatusCode::OK,
            )
            .await;
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
            // Assignment, not spawn, enforces the Ticket reference ceiling on
            // both the persisted links and the live Runtime attachment set.
            let attachments = fixture.runtime.logical_attachments(&worker.worker_id);
            assert_eq!(attachments.len(), 1);
            assert_eq!(attachments[0].working_directory_id, fixture.docs_workdir_id);
            assert_eq!(
                attachments[0].capabilities,
                workdir::WorkdirSessionCapabilities::READ_ONLY
            );
            assert!(fixture.runtime.input_requests.lock().unwrap().is_empty());
            if !existing {
                let request = fixture.runtime.spawn_requests().last().unwrap().clone();
                assert_eq!(request.resolved_workdir_attachments.len(), 1);
                assert_eq!(
                    request.resolved_workdir_attachments[0].working_directory_id,
                    fixture.docs_workdir_id
                );
                assert!(request.initial_submit.is_empty());
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
    let valid = start_assignment_body(api, &fixture.worker, "guarded-assign", None);
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
async fn orchestrator_assignment_rejects_missing_stale_cleared_or_reissued_delegation() {
    for owner_spawned in [false, true] {
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
                .create(ticket::NewTicket::new("Observed delegation claim"))
                .unwrap();
            assign_test_orchestrator(&api, &ticket.id);
            let observed = observed_start_delegation(&api, &ticket.id, Some(&source)).unwrap();
            let worker = spawn_start_worker(
                &api,
                if owner_spawned { None } else { Some(&source) },
                &token,
                start_worker_body(serde_json::json!([])),
            )
            .await;
            if owner_spawned {
                grant_start_candidate_control(&api, &source, &worker);
            }
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
                        serde_json::json!({"operation_id":"reissue-observed", "principal":test_assignment_principal(&api)})), StatusCode::OK).await;
                    assert_ne!(
                        observed_start_delegation(&api, &ticket.id, Some(&source)).unwrap(),
                        observed
                    );
                }
            }
            let subject_assignment = api
                .store
                .get_current_ticket_role_assignment(
                    TEST_WORKSPACE_ID,
                    &ticket.id,
                    TicketAssignmentRole::Orchestrator,
                )
                .unwrap();
            let submitted = match invalidation {
                "missing" => None,
                "stale" => Some("unrelated-assignment"),
                _ => Some(observed.as_str()),
            };
            let body = start_assignment_body(&api, &worker, "invalid-existing-claim", submitted);
            let path = format!(
                "/api/w/{TEST_WORKSPACE_ID}/tickets/{}/assignments/worker",
                ticket.id
            );
            let spawns = runtime.spawn_requests().len();
            let inputs = runtime.input_requests.lock().unwrap().len();
            let denied = start_http_response(
                &api,
                start_http_request(Some(&source), &token, "PUT", &path, body),
                StatusCode::CONFLICT,
            )
            .await;
            assert!(
                denied["message"].as_str().unwrap().contains("assignment")
                    || denied["message"].as_str().unwrap().contains("delegation"),
                "{invalidation}: {denied}"
            );
            assert_eq!(runtime.spawn_requests().len(), spawns);
            assert_eq!(api.runtime.worker(&worker).unwrap().state, "idle");
            assert_eq!(runtime.input_requests.lock().unwrap().len(), inputs);
            assert!(
                api.store
                    .get_active_ticket_worker_assignment(TEST_WORKSPACE_ID, &ticket.id)
                    .unwrap()
                    .is_none()
            );
            assert!(
                api.store
                    .get_ticket_assignment_operation(TEST_WORKSPACE_ID, "invalid-existing-claim")
                    .unwrap()
                    .is_none()
            );
            assert_eq!(
                api.store
                    .get_current_ticket_role_assignment(
                        TEST_WORKSPACE_ID,
                        &ticket.id,
                        TicketAssignmentRole::Orchestrator
                    )
                    .unwrap(),
                subject_assignment
            );
        }
    }
}

#[tokio::test]
async fn replaced_subject_body_cannot_assign_under_retained_current_delegation() {
    for owner_spawned in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let (mut api, _) = assignment_test_api_with_recording_backend(directory.path()).await;
        register_test_runtime(&api, WorkdirlessFixtureRuntime::RUNTIME_ID).await;
        let runtime = WorkdirlessFixtureRuntime::default();
        api.runtime.register_or_replace(runtime.clone());
        let source = start_body_source(&mut api, EMBEDDED_WORKER_RUNTIME_ID).await;
        bootstrap_test_assignment_session(&api, &source.worker);
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
        let subject_assignment = api
            .store
            .get_current_ticket_role_assignment(
                TEST_WORKSPACE_ID,
                &ticket.id,
                TicketAssignmentRole::Orchestrator,
            )
            .unwrap();
        let worker = spawn_start_worker(
            &api,
            if owner_spawned { None } else { Some(&source) },
            &token,
            start_worker_body(serde_json::json!([])),
        )
        .await;
        if owner_spawned {
            grant_start_candidate_control(&api, &source, &worker);
        }
        // Body replacement preserves Subject assignment, but rotates the real
        // create reservation/singleton generation through normal Subject launch.
        remove_test_assignment_body(&api, &source.worker).await;
        let replacement = create_test_assignment_body(
            &api,
            &source.worker.runtime_id,
            &ensure_test_assignment_subject(&api),
        )
        .await;
        assert_eq!(test_assignment_body(&api).unwrap().worker, replacement);
        assert_eq!(
            observed_start_delegation(&api, &ticket.id, Some(&source)).unwrap(),
            delegation
        );
        let path = format!(
            "/api/w/{TEST_WORKSPACE_ID}/tickets/{}/assignments/worker",
            ticket.id
        );
        let body =
            start_assignment_body(&api, &worker, "obsolete-existing-claim", Some(&delegation));
        let spawns = runtime.spawn_requests().len();
        let denied = start_http_response(
            &api,
            start_http_request(Some(&source), &token, "PUT", &path, body),
            // Authenticated Worker-source middleware fences the replaced body
            // before the assignment handler is admitted.
            StatusCode::UNAUTHORIZED,
        )
        .await;
        assert!(denied["message"].is_string());
        assert_eq!(runtime.spawn_requests().len(), spawns);
        assert_eq!(api.runtime.worker(&worker).unwrap().state, "idle");
        assert!(runtime.input_requests.lock().unwrap().is_empty());
        assert!(
            api.store
                .get_active_ticket_worker_assignment(TEST_WORKSPACE_ID, &ticket.id)
                .unwrap()
                .is_none()
        );
        assert!(
            api.store
                .get_ticket_assignment_operation(TEST_WORKSPACE_ID, "obsolete-existing-claim")
                .unwrap()
                .is_none()
        );
        assert_eq!(
            api.store
                .get_current_ticket_role_assignment(
                    TEST_WORKSPACE_ID,
                    &ticket.id,
                    TicketAssignmentRole::Orchestrator
                )
                .unwrap(),
            subject_assignment
        );
    }
}
