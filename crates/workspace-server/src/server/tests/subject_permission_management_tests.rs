//! Management HTTP integration uses the canonical Host policy owner and current
//! payload-free permission references. Real router/account authentication, SQLite
//! CAS/receipts and signed Ticket Tool dispatch are retained; only Runtime execution
//! is replaced by the parent suite's isolated workdirless fixture.
use super::*;
use feature_permissions::PermissionAssignment;

struct ManagementFixture {
    api: WorkspaceApi,
    owner: String,
    subject: String,
    _dir: tempfile::TempDir,
}

impl ManagementFixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let api = test_api(dir.path()).await;
        let owner = seed_test_workspace_owner_token(api.store.as_ref(), "permission-manager").await;
        let subject = open_subjektiv_store(&api)
            .unwrap()
            .create_subject(crate::subjektiv::SubjectRole::new("permission-management").unwrap())
            .unwrap()
            .id;
        Self {
            api,
            owner,
            subject,
            _dir: dir,
        }
    }

    fn path(&self) -> String {
        policy_path(&self.subject)
    }

    async fn read(&self) -> Value {
        let (status, body) = response(
            &self.api,
            human_request(&self.owner, "GET", &self.path(), None),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body
    }

    async fn save(&self, update: Value) -> Value {
        let (status, body) = response(
            &self.api,
            human_request(&self.owner, "PUT", &self.path(), Some(update)),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body
    }

    fn assert_current(&self, expected: &Value) {
        let saved = self
            .api
            .config_store
            .current_subject_permission_policy(TEST_WORKSPACE_ID, &self.subject)
            .unwrap();
        assert_eq!(serde_json::to_value(&saved).unwrap(), *expected);
        assert_eq!(
            open_subjektiv_store(&self.api)
                .unwrap()
                .current_subject_policy(&self.subject)
                .unwrap(),
            saved
        );
    }
}

fn policy_path(subject: &str) -> String {
    format!("/api/w/{TEST_WORKSPACE_ID}/subjektiv/subjects/{subject}/permission-policy")
}

fn update(
    screen: &Value,
    operation: &str,
    execution: PermissionAssignment,
    delegation: PermissionAssignment,
) -> Value {
    json!({"operation_id":operation,"expected_change_id":screen["change_id"],
        "action":{"kind":"replace","execution":execution,"delegation":delegation}})
}

#[tokio::test]
async fn management_read_exposes_operation_keys_without_duplicate_worker_target_grants() {
    let f = ManagementFixture::new().await;
    let screen = f.read().await;
    assert_eq!(screen["policy"], Value::Null);
    f.assert_current(&screen["policy"]);
    assert_eq!(screen["can_manage"], true);
    let catalog: feature_permissions::PermissionCatalog =
        serde_json::from_value(screen["catalog"].clone()).unwrap();
    assert_eq!(catalog.workspace.as_str(), TEST_WORKSPACE_ID);
    for operation in [
        "ticket.create",
        "worker.send_input",
        "worker.cancel",
        "worker.stop",
        "worker.restore",
        "worker.remove",
        "worker.observe",
    ] {
        let permission = catalog
            .namespaces
            .iter()
            .filter(|namespace| namespace.enabled)
            .flat_map(|namespace| &namespace.declaration.permissions)
            .find(|permission| permission.key.as_str() == operation)
            .unwrap();
        assert!(!permission.description.is_empty());
        let wire = serde_json::to_value(permission).unwrap();
        assert!(wire.get("resource").is_none());
        assert!(wire.get("resource_kinds").is_none());
    }
}

#[tokio::test]
async fn two_management_screens_conflict_without_overwriting_saved_execution_or_delegation() {
    let f = ManagementFixture::new().await;
    let first = f.read().await;
    let second = f.read().await;
    let saved = f
        .save(update(
            &first,
            "screen-one",
            test_operation_assignment(&f.api, &["ticket.create"]),
            test_operation_assignment(&f.api, &["ticket.query"]),
        ))
        .await;
    let stale = update(
        &second,
        "screen-two",
        Default::default(),
        test_operation_assignment(&f.api, &["ticket.create"]),
    );
    let (status, body) = response(
        &f.api,
        human_request(&f.owner, "PUT", &f.path(), Some(stale)),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(f.read().await["policy"], saved["policy"]);
    f.assert_current(&saved["policy"]);
}

#[tokio::test]
async fn discarded_save_response_recovers_by_exact_replay_and_authoritative_reread() {
    let f = ManagementFixture::new().await;
    let input = update(
        &f.read().await,
        "lost-save-response",
        test_operation_assignment(&f.api, &["ticket.create"]),
        test_operation_assignment(&f.api, &["ticket.query"]),
    );
    // Commit through HTTP, then discard its response; this is not a process-death test.
    drop(
        build_router(f.api.clone())
            .oneshot(human_request(
                &f.owner,
                "PUT",
                &f.path(),
                Some(input.clone()),
            ))
            .await
            .unwrap(),
    );
    let reread = f.read().await;
    assert_eq!(reread["policy"]["execution"], input["action"]["execution"]);
    assert_eq!(
        reread["policy"]["delegation"],
        input["action"]["delegation"]
    );
    let replay = f.save(input.clone()).await;
    assert_eq!(replay["policy"], reread["policy"]);
    assert_eq!(f.read().await["policy"], reread["policy"]);
    let mut changed = input;
    changed["action"]["execution"] = serde_json::to_value(PermissionAssignment::default()).unwrap();
    let (status, body) = response(
        &f.api,
        human_request(&f.owner, "PUT", &f.path(), Some(changed)),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "operation identity cannot be reused: {body}"
    );
    f.assert_current(&reread["policy"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn historical_save_receipt_after_shrink_and_remove_never_restores_current_authority() {
    let f = Fixture::new().await;
    let path = policy_path(&f.subject);
    let (status, screen) = response(&f.api, human_request(&f.owner, "GET", &path, None)).await;
    assert_eq!(status, StatusCode::OK, "{screen}");
    let original_input = update(
        &screen,
        "before-shrink",
        test_operation_assignment(&f.api, &["ticket.create"]),
        test_operation_assignment(&f.api, &["ticket.create"]),
    );
    let (status, original) = response(
        &f.api,
        human_request(&f.owner, "PUT", &path, Some(original_input.clone())),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{original}");
    assert_eq!(
        f.check(&f.source, &create_permission()).await.0,
        StatusCode::OK
    );
    let mut current = original.clone();
    for (operation, action) in [
        (
            "shrink",
            json!({"kind":"replace", "execution":PermissionAssignment::default(), "delegation":test_operation_assignment(&f.api, &["ticket.create"])}),
        ),
        ("remove", json!({"kind":"remove"})),
    ] {
        let (status, saved) = response(&f.api, human_request(&f.owner, "PUT", &path, Some(json!({"operation_id":operation,"expected_change_id":current["change_id"],"action":action})))).await;
        assert_eq!(status, StatusCode::OK, "{saved}");
        current = saved;
        let (status, replay) = response(
            &f.api,
            human_request(&f.owner, "PUT", &path, Some(original_input.clone())),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{replay}");
        assert_eq!(replay, original);
        let (status, read) = response(&f.api, human_request(&f.owner, "GET", &path, None)).await;
        assert_eq!(status, StatusCode::OK, "{read}");
        assert_eq!(read["policy"], current["policy"]);
        assert_eq!(read["change_id"], current["change_id"]);
        assert_ne!(read["change_id"], replay["change_id"]);
        assert_eq!(
            f.check(&f.source, &create_permission()).await.0,
            StatusCode::FORBIDDEN
        );
    }
}

#[tokio::test]
async fn subject_deletion_rejects_stale_screen_save_and_exact_receipt_replay_without_revival() {
    let f = ManagementFixture::new().await;
    let initial_input = update(
        &f.read().await,
        "save-before-delete",
        test_operation_assignment(&f.api, &["ticket.create"]),
        test_operation_assignment(&f.api, &["ticket.query"]),
    );
    let saved = f.save(initial_input.clone()).await;
    let late_input = update(
        &saved,
        "late-screen-save",
        test_operation_assignment(&f.api, &["ticket.query"]),
        Default::default(),
    );
    let (status, body) = response(
        &f.api,
        human_request(
            &f.owner,
            "DELETE",
            &format!(
                "/api/w/{TEST_WORKSPACE_ID}/subjektiv/subjects/{}",
                f.subject
            ),
            Some(json!({"operation_id":"delete-after-save"})),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["completed"], true);
    for input in [late_input, initial_input] {
        let (status, body) = response(
            &f.api,
            human_request(&f.owner, "PUT", &f.path(), Some(input)),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "deleted Subject cannot be revived: {body}"
        );
        assert!(
            open_subjektiv_store(&f.api)
                .unwrap()
                .subject(&f.subject)
                .unwrap()
                .is_none()
        );
    }
    let (status, body) = response(&f.api, human_request(&f.owner, "GET", &f.path(), None)).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
}

#[tokio::test]
async fn read_only_account_cannot_save_or_replay_an_owner_permission_change() {
    let f = ManagementFixture::new().await;
    let account = seed_test_api_token(f.api.store.as_ref(), "permission-reader");
    let initial = f.read().await;
    let input = update(
        &initial,
        "owner-only-save",
        test_operation_assignment(&f.api, &["ticket.create"]),
        Default::default(),
    );
    let (status, read) = response(&f.api, human_request(&account, "GET", &f.path(), None)).await;
    assert_eq!(status, StatusCode::OK, "{read}");
    assert_eq!(read["can_manage"], false);
    assert_eq!(read["policy"], initial["policy"]);
    let (status, body) = response(
        &f.api,
        human_request(&account, "PUT", &f.path(), Some(input.clone())),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    f.assert_current(&initial["policy"]);
    let saved = f.save(input.clone()).await;
    let (status, body) = response(
        &f.api,
        human_request(&account, "PUT", &f.path(), Some(input)),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "owner replay still authenticates the caller: {body}"
    );
    f.assert_current(&saved["policy"]);
}

#[tokio::test]
async fn unknown_keys_and_legacy_resource_payloads_are_rejected_without_policy_write() {
    let f = ManagementFixture::new().await;
    let screen = f.read().await;
    let valid =
        serde_json::to_value(test_operation_assignment(&f.api, &["ticket.create"])).unwrap();
    let mut unknown = valid.clone();
    unknown["grants"][0]["value"]["keys"] = json!(["ticket.future_operation"]);
    let mut resource_payload = valid.clone();
    resource_payload["grants"][0]["value"]["resource"] =
        json!({"kind":"ticket","workspace_id":TEST_WORKSPACE_ID,"ticket_id":"another-ticket"});
    for (name, assignment) in [
        ("unknown", unknown),
        ("resource-payload", resource_payload),
        ("legacy", json!([activation("ticket")])),
    ] {
        for lane in ["execution", "delegation"] {
            let mut input = update(
                &screen,
                &format!("reject-{name}-{lane}"),
                Default::default(),
                Default::default(),
            );
            input["action"][lane] = assignment.clone();
            let (status, body) = response(
                &f.api,
                human_request(&f.owner, "PUT", &f.path(), Some(input)),
            )
            .await;
            assert_eq!(
                status,
                if name == "unknown" {
                    StatusCode::BAD_REQUEST
                } else {
                    // Invalid DTO shape is rejected by the JSON extractor before
                    // the domain can validate recognized permission references.
                    StatusCode::UNPROCESSABLE_ENTITY
                },
                "{name}/{lane}: {body}"
            );
            f.assert_current(&screen["policy"]);
            assert_eq!(f.read().await["change_id"], screen["change_id"]);
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn management_saved_policy_controls_real_ticket_tool_and_raw_http_before_backend_writes() {
    let f = Fixture::new().await;
    let client = f.client(&f.source);
    let (installed, tool) = ticket_tool_host(
        &f._dir.path().join("management-ticket-tool"),
        client.clone(),
    )
    .await;
    assert!(installed);
    let tool = tool.unwrap();
    assert!(
        client.requests.lock().unwrap().is_empty(),
        "local registration is not authorization"
    );
    let backend = browser_ticket_backend(&f.api).unwrap();
    for state in ["unset", "other-operation", "allowed", "revoked"] {
        match state {
            "unset" => {}
            "other-operation" => {
                owner_subject_policy(
                    &f.api,
                    &f.owner,
                    &f.subject,
                    test_operation_assignment(&f.api, &["ticket.query"]),
                )
                .await;
            }
            "allowed" => {
                owner_subject_policy(
                    &f.api,
                    &f.owner,
                    &f.subject,
                    test_operation_assignment(&f.api, &["ticket.create"]),
                )
                .await;
            }
            "revoked" => {
                owner_subject_policy(
                    &f.api,
                    &f.owner,
                    &f.subject,
                    PermissionAssignment {
                        revoked: true,
                        ..Default::default()
                    },
                )
                .await;
            }
            _ => unreachable!(),
        }
        let before = backend.list(Default::default()).unwrap().len();
        let posts = ticket_posts(&client);
        let result = tool
            .execute(
                &json!({"title":format!("Managed policy {state}")}).to_string(),
                ToolExecutionContext::direct(),
            )
            .await;
        if state == "allowed" {
            let output = value(result.unwrap());
            assert_eq!(ticket_posts(&client), posts + 1);
            let records = backend.list(Default::default()).unwrap();
            assert_eq!(records.len(), before + 1, "real SQLite write: {output}");
            assert!(
                records
                    .iter()
                    .any(|record| record.title == "Managed policy allowed")
            );
        } else {
            assert!(result.is_err(), "{state}: Tool must deny");
            assert_eq!(
                ticket_posts(&client),
                posts,
                "{state}: no downstream create dispatch"
            );
            let (status, body) = response(
                &f.api,
                runtime_source_request(
                    &f.identity,
                    Some(&f.source.worker_id),
                    "POST",
                    &format!("/api/w/{TEST_WORKSPACE_ID}/tickets"),
                    serde_json::to_vec(&NewTicket::new(format!("Raw denied {state}"))).unwrap(),
                ),
            )
            .await;
            assert_eq!(
                status,
                StatusCode::FORBIDDEN,
                "{state}: raw Backend also rechecks: {body}"
            );
            assert_eq!(
                backend.list(Default::default()).unwrap().len(),
                before,
                "{state}: denied writes = 0"
            );
        }
    }
}
