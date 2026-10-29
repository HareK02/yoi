//! Management HTTP integration: real router/account authentication, catalog
//! validation, Subject SQLite policy/receipts and deletion; no running service or
//! model. The connected Ticket case also uses the signed Worker Host, real Tool
//! and Ticket backend via the parent suite's isolated Runtime fixture.
use super::*;

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
        format!(
            "/api/w/{TEST_WORKSPACE_ID}/subjektiv/subjects/{}/permissions",
            self.subject
        )
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
            human_request(&self.owner, "POST", &self.path(), Some(update)),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body
    }

    fn assert_current(&self, expected: &Value) {
        let saved = open_subjektiv_store(&self.api)
            .unwrap()
            .subject_permissions(&self.subject)
            .unwrap();
        assert_eq!(serde_json::to_value(saved).unwrap(), *expected);
    }
}

fn update(screen: &Value, operation: &str, execution: Value, delegation: Value) -> Value {
    json!({"operation_id":operation,"expected_change_id":screen["permissions"]["change_id"],
        "execution":execution,"delegation":delegation})
}

#[tokio::test]
async fn management_read_exposes_operation_use_without_duplicate_worker_target_grants() {
    let f = ManagementFixture::new().await;
    let screen = f.read().await;
    assert!(screen["permissions"].is_null());
    assert_eq!(screen["can_manage"], true);
    let operations = screen["operations"].as_array().expect("management catalog");
    let create = operations
        .iter()
        .find(|entry| entry["operation"] == "ticket.create")
        .unwrap();
    assert_eq!(create["availability"]["kind"], "available");
    assert!(
        create["resource_kinds"]
            .as_array()
            .unwrap()
            .contains(&json!("workspace"))
    );
    assert!(!create["label"].as_str().unwrap().is_empty());
    assert!(!create["description"].as_str().unwrap().is_empty());
    let messaging = operations
        .iter()
        .find(|entry| entry["operation"] == "worker.messaging")
        .unwrap();
    assert_eq!(messaging["availability"]["kind"], "available");
    for name in [
        "worker.messaging",
        "worker.send_input",
        "worker.cancel",
        "worker.stop",
        "worker.restore",
        "worker.remove",
        "worker.observe",
    ] {
        let operation = operations
            .iter()
            .find(|entry| entry["operation"] == name)
            .unwrap();
        assert_eq!(
            operation["availability"]["kind"], "available",
            "implemented Worker contracts: {operation}"
        );
        assert_eq!(
            operation["resource_kinds"],
            json!(["worker_operation"]),
            "operation use must not offer duplicate target control grants"
        );
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
            json!([create_permission()]),
            json!([query_permission()]),
        ))
        .await;
    let stale = update(
        &second,
        "screen-two",
        json!([]),
        json!([create_permission()]),
    );
    let (status, body) = response(
        &f.api,
        human_request(&f.owner, "POST", &f.path(), Some(stale)),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(
        body["message"].is_string(),
        "conflict must be visible: {body}"
    );
    assert_eq!(f.read().await["permissions"], saved["permissions"]);
    f.assert_current(&saved["permissions"]);
}

#[tokio::test]
async fn discarded_save_response_recovers_by_exact_replay_and_authoritative_reread() {
    let f = ManagementFixture::new().await;
    let input = update(
        &f.read().await,
        "lost-save-response",
        json!([create_permission()]),
        json!([query_permission()]),
    );
    // Commit through the real HTTP route, then discard the entire response. This
    // models response loss, not process death or rollback of the committed save.
    drop(
        build_router(f.api.clone())
            .oneshot(human_request(
                &f.owner,
                "POST",
                &f.path(),
                Some(input.clone()),
            ))
            .await
            .unwrap(),
    );
    let reread = f.read().await;
    assert_eq!(reread["permissions"]["execution"], input["execution"]);
    assert_eq!(reread["permissions"]["delegation"], input["delegation"]);
    let replay = f.save(input.clone()).await;
    assert_eq!(replay["permissions"], reread["permissions"]);
    assert_eq!(replay["interruptions"], json!([]));
    assert_eq!(f.read().await["permissions"], reread["permissions"]);
    f.assert_current(&reread["permissions"]);
    let mut changed = input;
    changed["execution"] = json!([]);
    let (status, body) = response(
        &f.api,
        human_request(&f.owner, "POST", &f.path(), Some(changed)),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "operation identity cannot be reused: {body}"
    );
    f.assert_current(&reread["permissions"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn historical_save_receipt_after_shrink_and_revoke_never_restores_current_authority() {
    let f = Fixture::new().await;
    let (status, screen) = response(
        &f.api,
        human_request(&f.owner, "GET", &f.policy_path(), None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let original_input = update(
        &screen,
        "before-shrink",
        json!([create_permission()]),
        json!([create_permission()]),
    );
    let (status, original) = response(
        &f.api,
        human_request(
            &f.owner,
            "POST",
            &f.policy_path(),
            Some(original_input.clone()),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{original}");
    assert_eq!(
        f.check(&f.source, &create_permission()).await.0,
        StatusCode::OK
    );
    for delegation in [vec![create_permission()], vec![]] {
        let current = f.assign(vec![], delegation).await;
        let (status, replay) = response(
            &f.api,
            human_request(
                &f.owner,
                "POST",
                &f.policy_path(),
                Some(original_input.clone()),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "historical receipt: {replay}");
        assert_eq!(replay["permissions"], original["permissions"]);
        assert_eq!(replay["interruptions"], json!([]));
        let (status, read) = response(
            &f.api,
            human_request(&f.owner, "GET", &f.policy_path(), None),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(read["permissions"], serde_json::to_value(&current).unwrap());
        assert_ne!(read["permissions"], replay["permissions"]);
        assert_eq!(
            open_subjektiv_store(&f.api)
                .unwrap()
                .subject_permissions(&f.subject)
                .unwrap(),
            Some(current)
        );
        assert_eq!(
            f.check(&f.source, &create_permission()).await.0,
            StatusCode::FORBIDDEN
        );
    }
}

#[tokio::test]
async fn subject_deletion_rejects_stale_screen_save_and_exact_receipt_replay_without_revival() {
    let f = ManagementFixture::new().await;
    let screen = f.read().await;
    let initial_input = update(
        &screen,
        "save-before-delete",
        json!([create_permission()]),
        json!([query_permission()]),
    );
    let saved = f.save(initial_input.clone()).await;
    let stale_screen = json!({"permissions":saved["permissions"]});
    let late_input = update(
        &stale_screen,
        "late-screen-save",
        json!([query_permission()]),
        json!([]),
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
            human_request(&f.owner, "POST", &f.path(), Some(input)),
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
    assert!(
        open_subjektiv_store(&f.api)
            .unwrap()
            .subject_permissions(&f.subject)
            .is_err()
    );
}

#[tokio::test]
async fn read_only_account_cannot_save_or_replay_an_owner_permission_change() {
    let f = ManagementFixture::new().await;
    let account = seed_test_api_token(f.api.store.as_ref(), "permission-reader");
    let initial = f.read().await;
    let input = update(
        &initial,
        "owner-only-save",
        json!([create_permission()]),
        json!([]),
    );
    let (status, read) = response(&f.api, human_request(&account, "GET", &f.path(), None)).await;
    assert_eq!(status, StatusCode::OK, "{read}");
    assert_eq!(read["can_manage"], false);
    assert_eq!(read["permissions"], initial["permissions"]);
    let (status, body) = response(
        &f.api,
        human_request(&account, "POST", &f.path(), Some(input.clone())),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    f.assert_current(&Value::Null);
    let saved = f.save(input.clone()).await;
    let (status, body) = response(
        &f.api,
        human_request(&account, "POST", &f.path(), Some(input)),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "exact owner receipt replay must still authorize the caller: {body}"
    );
    f.assert_current(&saved["permissions"]);
}

#[tokio::test]
async fn new_unknown_legacy_unavailable_and_operation_resource_mismatch_are_rejected_without_policy_write()
 {
    let f = ManagementFixture::new().await;
    let screen = f.read().await;
    let mut unknown = create_permission();
    unknown.operation = "ticket.future_operation".into();
    let mut mismatch = create_permission();
    mismatch.resource = PermissionResource::Ticket {
        workspace_id: TEST_WORKSPACE_ID.into(),
        ticket_id: "not-a-collection".into(),
    };
    let mut unavailable = query_permission();
    unavailable.operation = "worker.messaging".into();
    for (name, permission) in [
        ("unknown", unknown),
        ("legacy", activation("ticket")),
        ("resource-mismatch", mismatch),
        ("unavailable", unavailable),
    ] {
        for lane in ["execution", "delegation"] {
            let mut input = update(
                &screen,
                &format!("reject-{name}-{lane}"),
                json!([]),
                json!([]),
            );
            input[lane] = json!([permission]);
            let (status, body) = response(
                &f.api,
                human_request(&f.owner, "POST", &f.path(), Some(input)),
            )
            .await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{name}/{lane}: {body}");
            assert!(body["message"].is_string(), "{body}");
            f.assert_current(&Value::Null);
            assert!(f.read().await["permissions"].is_null());
        }
    }
}

#[tokio::test]
async fn unsupported_saved_entries_can_only_be_retained_in_the_same_lane_or_removed() {
    let f = ManagementFixture::new().await;
    let execution = vec![activation("ticket")];
    let mut unknown = query_permission();
    unknown.operation = "legacy.custom_operation".into();
    let delegation = vec![unknown];
    open_subjektiv_store(&f.api)
        .unwrap()
        .set_subject_permissions(
            &f.subject,
            &SubjectPermissionsUpdate {
                operation_id: "seed-pre-catalog-policy".into(),
                expected_change_id: None,
                execution: execution.clone(),
                delegation: delegation.clone(),
            },
        )
        .unwrap();
    let screen = f.read().await;
    let saved = f
        .save(update(
            &screen,
            "retain-existing",
            json!([execution[0], create_permission()]),
            json!(delegation),
        ))
        .await;
    assert_eq!(
        saved["permissions"]["execution"],
        json!([execution[0], create_permission()])
    );
    assert_eq!(saved["permissions"]["delegation"], json!(delegation));
    let screen = f.read().await;
    for (name, execution, delegation) in [
        (
            "copy-legacy-lane",
            json!([activation("ticket"), create_permission()]),
            json!([activation("ticket")]),
        ),
        (
            "copy-unknown-lane",
            json!([delegation[0], create_permission()]),
            json!(delegation),
        ),
    ] {
        let (status, body) = response(
            &f.api,
            human_request(
                &f.owner,
                "POST",
                &f.path(),
                Some(update(&screen, name, execution, delegation)),
            ),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "retention cannot expand into another lane: {body}"
        );
        f.assert_current(&saved["permissions"]);
    }
    let removed = f
        .save(update(
            &screen,
            "remove-unsupported",
            json!([create_permission()]),
            json!([]),
        ))
        .await;
    assert_eq!(
        removed["permissions"]["execution"],
        json!([create_permission()])
    );
    assert_eq!(removed["permissions"]["delegation"], json!([]));
    f.assert_current(&removed["permissions"]);
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
    for state in ["unset", "out-of-scope", "allowed", "revoked"] {
        match state {
            "unset" => {}
            "out-of-scope" => {
                // Intentionally migrated, unsupported kind: management may retain
                // this exact saved entry but it must never authorize collection creation.
                let mut wrong_kind = create_permission();
                wrong_kind.resource = PermissionResource::Ticket {
                    workspace_id: TEST_WORKSPACE_ID.into(),
                    ticket_id: "legacy-ticket-scope".into(),
                };
                f.seed_saved_policy(vec![wrong_kind.clone()], vec![]);
                f.assign(vec![wrong_kind], vec![]).await;
            }
            "allowed" => {
                f.assign(vec![create_permission()], vec![]).await;
            }
            "revoked" => {
                f.assign(vec![], vec![]).await;
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
                "{state}: direct Backend path also rechecks: {body}"
            );
            assert_eq!(
                backend.list(Default::default()).unwrap().len(),
                before,
                "{state}: denied writes = 0"
            );
        }
    }
}
