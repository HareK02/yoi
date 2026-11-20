//! Signed authorization and creation routers use real Subject policy history,
//! singleton leases and persisted core grants. Only Runtime execution is replaced
//! by the parent suite's WorkdirlessFixtureRuntime. Run the normal Server library
//! tests with the `core_grant_continuity` filter; no external service is required.
use super::*;
use server_api::subject_policy::{SubjectPermissionPolicyAction, SubjectPermissionPolicyMutation};

async fn policy_action(f: &Fixture, action: SubjectPermissionPolicyAction) {
    let path = format!(
        "/api/w/{TEST_WORKSPACE_ID}/subjektiv/subjects/{}/permission-policy",
        f.subject
    );
    let (status, before) = response(&f.api, human_request(&f.owner, "GET", &path, None)).await;
    assert_eq!(status, StatusCode::OK, "{before}");
    let request = SubjectPermissionPolicyMutation {
        operation_id: Uuid::now_v7().to_string(),
        expected_change_id: serde_json::from_value(before["change_id"].clone()).unwrap(),
        action,
    };
    let (status, body) = response(
        &f.api,
        human_request(
            &f.owner,
            "PUT",
            &path,
            Some(serde_json::to_value(request).unwrap()),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

async fn replace_policy(f: &Fixture, execution: &[&str], delegation: &[&str]) {
    policy_action(
        f,
        SubjectPermissionPolicyAction::Replace {
            execution: key_assignment(execution),
            delegation: key_assignment(delegation),
        },
    )
    .await;
}

async fn assert_check(
    f: &Fixture,
    worker: &RuntimeWorkerRef,
    operation: &str,
    expected: StatusCode,
) {
    let (status, body) = f.check(worker, &workspace_operation(operation)).await;
    assert_eq!(status, expected, "{operation}: {body}");
}

async fn successor_body(f: &Fixture) -> RuntimeWorkerRef {
    // This Runtime fixture has no live Controller. Release its real empty writer
    // through the domain before replacing the body; releasing a lease is not
    // equivalent to releasing Journal ownership.
    let store = open_subjektiv_store(&f.api).unwrap();
    let attribution = store
        .subject_session_attributions(&f.subject)
        .unwrap()
        .into_iter()
        .find(|attr| attr.runtime_id == f.source.runtime_id && attr.worker_id == f.source.worker_id)
        .unwrap();
    let host =
        subjektiv::journal::JournalHostContext::validated_session(&store, attribution).unwrap();
    store
        .release_journal_writer(&subjektiv::journal::JournalWriterBinding {
            subject_id: f.subject.clone(),
            runtime_id: f.source.runtime_id.clone(),
            worker_id: f.source.worker_id.clone(),
            binding_id: host.writer_binding_id().unwrap().into(),
        })
        .unwrap();
    f.release_source_lease();
    let context = server_request_context(
        Some(test_owner_actor()),
        None,
        None,
        None,
        &HeaderMap::new(),
    );
    let Json(created) = create_workspace_worker_authenticated(
        f.api.clone(),
        HeaderMap::new(),
        CreateWorkspaceWorkerRequest {
            runtime_id: f.source.runtime_id.clone(),
            display_name: "Successor Subject body".into(),
            singleton_key: None,
            profile: Some("project:authority-source".into()),
            initial_submit: vec![],
            workdir_attachments: vec![],
            feature_connections: server_api::WorkspaceWorkerFeatureConnectionsRequest {
                subjektiv: Some(server_api::SubjektivWorkerConnectionRequest {
                    subject_id: f.subject.clone(),
                }),
            },
            control_operation_id: Some(Uuid::now_v7().to_string()),
            authorization: server_api::launch_policy::WorkerCreationAuthorization::Assignment {
                assignment: None,
            },
        },
        None,
        Some(&context),
    )
    .await
    .unwrap();
    let successor = RuntimeWorkerRef::new(created.runtime_id, created.worker_id);
    assert_ne!(successor, f.source);
    assert!(
        f.api
            .store
            .require_current_worker_singleton_owner(TEST_WORKSPACE_ID, &successor)
            .unwrap()
            .is_some()
    );
    successor
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn body_succession_preserves_delegated_workers_but_denies_old_body_authority() {
    let f = Fixture::new().await;
    replace_policy(
        &f,
        &["worker.create", "ticket.query"],
        &["worker.create", "ticket.query"],
    )
    .await;
    let child = f
        .child(
            "before-succession",
            vec![workspace_operation("worker.create"), query_permission()],
        )
        .await;
    let saved = f.core_grant(&child);
    let successor = successor_body(&f).await;
    assert_check(&f, &child, "ticket.query", StatusCode::OK).await;
    assert_check(&f, &successor, "ticket.query", StatusCode::OK).await;
    assert_check(&f, &f.source, "ticket.query", StatusCode::UNAUTHORIZED).await;
    // Also exercise the core boundary directly, without letting signed-source
    // authentication alone stand in for the old-body ownership check.
    assert!(
        subject_authorization::authorize(
            &f.api,
            &f.source,
            &server_api::authorization::OperationAuthorizationRequest {
                operation: "worker.create".into(),
                resource: PermissionResource::Workspace {
                    workspace_id: TEST_WORKSPACE_ID.into()
                },
            }
        )
        .is_err()
    );
    assert_eq!(
        f.spawn(
            &f.source,
            f.child_request("old-body-cannot-issue", vec![query_permission()])
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    let (status, body) = f
        .spawn(
            &child,
            f.child_request("redelegate-after-succession", vec![query_permission()]),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let grandchild = RuntimeWorkerRef::new(
        body["runtime_id"].as_str().unwrap(),
        body["worker_id"].as_str().unwrap(),
    );
    assert_check(&f, &grandchild, "ticket.query", StatusCode::OK).await;
    assert_eq!(f.core_grant(&child), saved);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mixed_reference_order_in_policy_receipts_preserves_continuity() {
    use feature_permissions::{PermissionReference, PermissionSetId};
    let f = Fixture::new().await;
    replace_policy(&f, &["worker.create"], &["ticket.query"]).await;
    let child = f
        .child("before-mixed-references", vec![query_permission()])
        .await;
    let mut delegation = key_assignment(&["ticket.query"]);
    delegation.grants.push(PermissionReference::Set(
        PermissionSetId::parse("ticket").unwrap(),
    ));
    for _ in 0..2 {
        policy_action(
            &f,
            SubjectPermissionPolicyAction::Replace {
                execution: key_assignment(&["worker.create"]),
                delegation: delegation.clone(),
            },
        )
        .await;
    }
    assert_check(&f, &child, "ticket.query", StatusCode::OK).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unrelated_policy_changes_preserve_only_continuously_delegated_operations() {
    let f = Fixture::new().await;
    replace_policy(
        &f,
        &["worker.create", "ticket.query"],
        &["ticket.query", "ticket.create"],
    )
    .await;
    let child = f
        .child(
            "selective-continuity",
            vec![query_permission(), create_permission()],
        )
        .await;
    let saved = f.core_grant(&child);
    // Change the direct execution lane and narrow an unrelated delegated key.
    replace_policy(&f, &["worker.create", "ticket.read"], &["ticket.query"]).await;
    assert_check(&f, &child, "ticket.query", StatusCode::OK).await;
    assert_check(&f, &child, "ticket.create", StatusCode::FORBIDDEN).await;
    replace_policy(&f, &["worker.create"], &["ticket.query", "ticket.create"]).await;
    assert_check(&f, &child, "ticket.query", StatusCode::OK).await;
    assert_check(&f, &child, "ticket.create", StatusCode::FORBIDDEN).await;
    let fresh = f
        .child("new-grant-after-regrant", vec![create_permission()])
        .await;
    assert_check(&f, &fresh, "ticket.create", StatusCode::OK).await;
    assert_eq!(f.core_grant(&child), saved);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn revocation_then_regrant_without_intermediate_use_never_revives_old_core_grants() {
    use feature_permissions::PermissionAssignment;
    let exclusion = PermissionAssignment {
        exclusions: key_assignment(&["ticket.query"]).grants,
        ..key_assignment(&["ticket.query"])
    };
    let revocations = [
        SubjectPermissionPolicyAction::Replace {
            execution: key_assignment(&["worker.create"]),
            delegation: key_assignment(&[]),
        },
        SubjectPermissionPolicyAction::Replace {
            execution: key_assignment(&["worker.create"]),
            delegation: PermissionAssignment {
                revoked: true,
                ..key_assignment(&["ticket.query"])
            },
        },
        SubjectPermissionPolicyAction::Replace {
            execution: key_assignment(&["worker.create"]),
            delegation: exclusion,
        },
        SubjectPermissionPolicyAction::Replace {
            execution: key_assignment(&[]),
            delegation: key_assignment(&["ticket.query"]),
        },
        SubjectPermissionPolicyAction::Remove,
    ];
    for revocation in revocations {
        let f = Fixture::new().await;
        replace_policy(&f, &["worker.create"], &["ticket.query"]).await;
        let child = f.child("before-revocation", vec![query_permission()]).await;
        assert_check(&f, &child, "ticket.query", StatusCode::OK).await;
        let saved = f.core_grant(&child);
        policy_action(&f, revocation).await;
        // No authorization call in the revoked interval: continuity must come
        // from immutable mutation evidence, not an in-memory denial cache.
        replace_policy(&f, &["worker.create"], &["ticket.query"]).await;
        assert_check(&f, &child, "ticket.query", StatusCode::FORBIDDEN).await;
        let fresh = f.child("after-revocation", vec![query_permission()]).await;
        assert_check(&f, &fresh, "ticket.query", StatusCode::OK).await;
        assert_eq!(f.core_grant(&child), saved);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn policy_and_catalog_growth_never_expand_parent_or_child_issuance_ceilings() {
    let f = Fixture::new().await;
    declared_ticket_children(&f, false);
    replace_policy(&f, &["worker.create"], &["worker.create", "ticket.hoge"]).await;
    let child = f
        .child(
            "finite-root",
            vec![
                workspace_operation("worker.create"),
                workspace_operation("ticket.hoge"),
            ],
        )
        .await;
    let saved = f.core_grant(&child);
    declared_ticket_children(&f, true);
    replace_policy(
        &f,
        &["worker.create", "ticket.query"],
        &["worker.create", "ticket.hoge", "ticket.create"],
    )
    .await;
    assert_check(&f, &child, "ticket.hoge", StatusCode::OK).await;
    for key in ["ticket.hoge.fuga", "ticket.create"] {
        assert_check(&f, &child, key, StatusCode::FORBIDDEN).await;
        let (status, body) = f
            .spawn(
                &child,
                f.child_request(&format!("excess-{key}"), vec![workspace_operation(key)]),
            )
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    }
    let fresh = f
        .child(
            "finite-after-growth",
            vec![workspace_operation("ticket.hoge")],
        )
        .await;
    assert_check(&f, &fresh, "ticket.hoge.fuga", StatusCode::OK).await;
    assert_eq!(f.core_grant(&child), saved);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn launch_children_survive_subject_changes_but_not_launch_authorization_replacement() {
    use server_api::launch_policy::{WorkerLaunchPolicyAction, WorkerLaunchPolicyMutation};
    let f = Fixture::new().await;
    let policy = seed_owner_launch_policy(
        &f.api,
        &f.owner,
        "continuity",
        &["builtin:coder"],
        key_assignment(&["ticket.query"]),
    )
    .await;
    replace_policy(&f, &["worker.create", "worker.launch.continuity"], &[]).await;
    let child = f.policy_child("launch-before-succession", &policy).await;
    let saved = f.core_grant(&child);
    successor_body(&f).await;
    replace_policy(
        &f,
        &["worker.create", "worker.launch.continuity", "ticket.create"],
        &[],
    )
    .await;
    assert_check(&f, &child, "ticket.query", StatusCode::OK).await;
    let headers = human_request(&f.owner, "GET", "/", None).headers().clone();
    let owner = resolve_actor(&ServerAuthApi::from(&f.api), &headers)
        .await
        .unwrap()
        .unwrap();
    let replacement = f
        .api
        .config_store
        .mutate_worker_launch_policy(
            TEST_WORKSPACE_ID,
            &owner.account_id,
            &WorkerLaunchPolicyMutation {
                operation_key: "replace-launch-continuity".into(),
                policy_id: policy.policy_id.clone(),
                expected: Some(policy.observation()),
                action: WorkerLaunchPolicyAction::ReplaceAuthorization {
                    allowed_profiles: policy.allowed_profiles.clone(),
                    child_permissions: policy.child_permissions.clone(),
                },
            },
        )
        .unwrap()
        .policy;
    assert_ne!(replacement.authorization_key, policy.authorization_key);
    assert!(subject_authorization::revalidate_core_grant(&f.api, &saved).is_err());
    assert!(!f.check(&child, &query_permission()).await.0.is_success());
    assert_eq!(f.core_grant(&child), saved);
}
