//! Server list/detail projection through the production handlers and real SQLite grants/links.
//! The execution owner is the deterministic fixture Runtime; transport coverage lives in
//! ticket_worker_launch_tests and restored_parent_control_tests.
use super::*;
use server_api::WorkerListState;

async fn fixture() -> (ManualCoderAssignmentFixture, RuntimeWorkerRef) {
    let f = manual_worker_assignment_fixture().await;
    let owner = seed_test_orchestrator_as_owner(&f.api, &[("worker", &["worker.list"])]);
    let owner = owner.worker.unwrap();
    let controller = RuntimeWorkerRef::new(owner.runtime_id, owner.worker_id);
    f.api
        .store
        .create_worker_control_grant(&WorkerControlGrantRecord {
            workspace_id: TEST_WORKSPACE_ID.into(),
            grant_id: "list-control".into(),
            controller: controller.clone(),
            subject: f.worker.clone(),
            relation: "spawned".into(),
            origin: "fixture".into(),
            permissions: vec!["send_input".into(), "observe".into(), "restore".into()],
            operation_id: "list-control".into(),
            created_at: now_registry_timestamp(),
            revoked_at: None,
        })
        .unwrap();
    (f, controller)
}

async fn list(
    f: &ManualCoderAssignmentFixture,
    controller: &RuntimeWorkerRef,
) -> server_api::WorkerControlListResponse {
    list_known_workers(
        State(f.api.clone()),
        AxumPath(ScopedWorkspacePath {
            workspace_id: TEST_WORKSPACE_ID.into(),
        }),
        ticket_check_headers(controller),
    )
    .await
    .unwrap()
    .0
}

async fn details(
    f: &ManualCoderAssignmentFixture,
    controller: &RuntimeWorkerRef,
) -> server_api::WorkerControlDetails {
    let worker_key = f
        .api
        .store
        .worker_resource_key(TEST_WORKSPACE_ID, &f.worker)
        .unwrap()
        .unwrap();
    known_worker_details(
        State(f.api.clone()),
        AxumPath(ScopedRuntimeWorkerPath {
            workspace_id: TEST_WORKSPACE_ID.into(),
            worker: RuntimeWorkerRef::new(&f.worker.runtime_id, worker_key),
        }),
        ticket_check_headers(controller),
    )
    .await
    .unwrap()
    .0
}

#[tokio::test]
async fn known_list_projects_live_state_without_launch_or_workdir_details() {
    use protocol::{WorkerLifecycleState as L, WorkerStatus};
    let (f, controller) = fixture().await;
    for (lifecycle, snapshot, availability, expected) in [
        (
            Some(L::Active),
            Some(WorkerStatus::Running.into()),
            true,
            WorkerListState::Running,
        ),
        (
            Some(L::Active),
            Some(WorkerStatus::Paused.into()),
            true,
            WorkerListState::Paused,
        ),
        (
            Some(L::Active),
            Some(WorkerStatus::Idle.into()),
            true,
            WorkerListState::Idle,
        ),
        (Some(L::Active), None, true, WorkerListState::Unavailable),
        (
            Some(L::Active),
            Some(WorkerStatus::Idle.into()),
            false,
            WorkerListState::Unavailable,
        ),
        (Some(L::Creating), None, true, WorkerListState::Creating),
        (Some(L::Restoring), None, true, WorkerListState::Restoring),
        (Some(L::Stopping), None, true, WorkerListState::Stopping),
        (
            Some(L::RecoveryRequired),
            None,
            true,
            WorkerListState::RecoveryRequired,
        ),
        (Some(L::Stopped), None, true, WorkerListState::Stopped),
        (
            Some(L::AwaitingRestore),
            None,
            true,
            WorkerListState::AwaitingRestore,
        ),
        (Some(L::NotStarted), None, true, WorkerListState::NotStarted),
        (
            None,
            Some(WorkerStatus::Idle.into()),
            true,
            WorkerListState::Unavailable,
        ),
    ] {
        {
            let mut workers = f.runtime.workers.lock().unwrap();
            let worker = workers.iter_mut().find(|w| w.worker == f.worker).unwrap();
            worker.state = "idle".into(); // Deliberately stale catalog, never the list source.
            worker.lifecycle = lifecycle;
            worker.observation = snapshot
                .as_ref()
                .filter(|_| availability)
                .map(|snapshot: &protocol::WorkerStateSnapshot| {
                    runtime_api::WorkerObservationState::Current {
                        state_sequence: snapshot.state_sequence,
                    }
                })
                .unwrap_or(runtime_api::WorkerObservationState::Unavailable);
            worker.last_finished_submission_request_id = Some("durable-history-not-live".into());
            worker.worker_state = snapshot;
            worker.availability = if availability {
                protocol::subscription::SubscriptionWorkerAvailability::Observed
            } else {
                protocol::subscription::SubscriptionWorkerAvailability::Unavailable
            };
        }
        let listed = list(&f, &controller).await;
        assert_eq!(listed.items.len(), 1);
        assert_eq!(listed.items[0].state, expected);
        assert_eq!(listed.items[0].subject.worker_id, f.worker.worker_id);
        assert!(listed.items[0].worker.starts_with("W-"));
        let json = serde_json::to_value(&listed.items[0]).unwrap();
        for hidden in [
            "summary",
            "workdir_connections",
            "host_id",
            "origin",
            "implementation",
            "workspace",
            "restore_observation_token",
        ] {
            assert!(json.get(hidden).is_none(), "{hidden}: {json}");
        }
    }
    f.runtime
        .workers
        .lock()
        .unwrap()
        .retain(|w| w.worker != f.worker);
    let error = list_known_workers(
        State(f.api.clone()),
        AxumPath(ScopedWorkspacePath {
            workspace_id: TEST_WORKSPACE_ID.into(),
        }),
        ticket_check_headers(&controller),
    )
    .await
    .unwrap_err();
    assert_eq!(
        error.into_response().status(),
        StatusCode::NOT_FOUND,
        "a missing Runtime detail must fail the list, not produce Idle"
    );
}

#[tokio::test]
async fn details_observed_empty_bindings_feed_workdirless_assignment() {
    let (f, controller) = fixture().await;
    for id in [&f.main_workdir_id, &f.docs_workdir_id] {
        f.api
            .store
            .detach_worker_workdir(TEST_WORKSPACE_ID, &f.worker, Some(id), TEST_CREATED_AT)
            .unwrap();
    }
    sync_runtime_worker_workdir_attachments(&f.api, &f.worker).unwrap();
    browser_ticket_backend(&f.api)
        .unwrap()
        .edit_item(
            f.ticket_id.clone().into(),
            TicketItemEdit {
                targets: Some(TicketTargetsEdit::Clear),
                ..Default::default()
            },
        )
        .unwrap();
    let listed = list(&f, &controller).await;
    let observed = details(&f, &controller).await;
    assert_eq!(observed.worker, listed.items[0].worker);
    let connections = observed.workdir_connections.unwrap();
    assert!(
        connections.is_empty(),
        "an authorized empty observation is not null"
    );
    let mut request = manual_worker_assignment_request(&f, "assign-empty-from-details");
    request.workdir_bindings = connections
        .into_iter()
        .map(|item| server_api::TicketWorkerAttachmentBinding {
            alias: item.alias,
            working_directory_id: item.working_directory_id,
            connection_id: item.connection_id,
        })
        .collect();
    let Json(accepted) = scoped_set_ticket_assignment(
        State(f.api.clone()),
        AxumPath((
            TEST_WORKSPACE_ID.into(),
            f.ticket_id.clone(),
            "worker".into(),
        )),
        Json(request),
    )
    .await
    .unwrap();
    let saved = f
        .api
        .store
        .get_current_ticket_worker_assignment(TEST_WORKSPACE_ID, &f.ticket_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        accepted.assignment.unwrap().assignment_id,
        saved.assignment_id
    );
    assert_eq!(saved.worker, f.worker);
    assert!(
        f.runtime
            .logical_attachments(&f.worker.worker_id)
            .is_empty()
    );
    assert_eq!(*f.runtime.restore_attempts.lock().unwrap(), 0);
}

#[tokio::test]
async fn details_bindings_feed_assignment_and_stale_rebind_is_rejected() {
    let (f, controller) = fixture().await;
    let observed = details(&f, &controller).await;
    assert_eq!(observed.worker, list(&f, &controller).await.items[0].worker);
    let connections = observed.workdir_connections.unwrap();
    assert_eq!(connections.len(), 2);
    let mut request = manual_worker_assignment_request(&f, "assign-from-details");
    request.workdir_bindings = connections
        .into_iter()
        .map(|item| server_api::TicketWorkerAttachmentBinding {
            alias: item.alias,
            working_directory_id: item.working_directory_id,
            connection_id: item.connection_id,
        })
        .collect();
    let old = f
        .api
        .store
        .list_worker_workdir_links(TEST_WORKSPACE_ID, &f.worker)
        .unwrap()
        .into_iter()
        .find(|link| link.workdir_id == f.docs_workdir_id && link.unlinked_at.is_none())
        .unwrap();
    f.api
        .store
        .detach_worker_workdir(
            TEST_WORKSPACE_ID,
            &f.worker,
            Some(&f.docs_workdir_id),
            &now_registry_timestamp(),
        )
        .unwrap();
    f.api
        .store
        .attach_worker_workdir(&WorkerWorkdirLinkRecord {
            connection_id: String::new(),
            linked_at: now_registry_timestamp(),
            ..old
        })
        .unwrap();
    let rejected = scoped_set_ticket_assignment(
        State(f.api.clone()),
        AxumPath((
            TEST_WORKSPACE_ID.into(),
            f.ticket_id.clone(),
            "worker".into(),
        )),
        Json(request.clone()),
    )
    .await
    .unwrap_err();
    assert_eq!(rejected.into_response().status(), StatusCode::CONFLICT);
    assert!(
        f.api
            .store
            .get_current_ticket_worker_assignment(TEST_WORKSPACE_ID, &f.ticket_id)
            .unwrap()
            .is_none()
    );
    let refreshed = details(&f, &controller).await.workdir_connections.unwrap();
    request.workdir_bindings = refreshed
        .into_iter()
        .map(|item| server_api::TicketWorkerAttachmentBinding {
            alias: item.alias,
            working_directory_id: item.working_directory_id,
            connection_id: item.connection_id,
        })
        .collect();
    let _ = scoped_set_ticket_assignment(
        State(f.api.clone()),
        AxumPath((
            TEST_WORKSPACE_ID.into(),
            f.ticket_id.clone(),
            "worker".into(),
        )),
        Json(request),
    )
    .await
    .unwrap();
    assert_eq!(
        f.api
            .store
            .get_current_ticket_worker_assignment(TEST_WORKSPACE_ID, &f.ticket_id)
            .unwrap()
            .unwrap()
            .worker,
        f.worker
    );
}

#[tokio::test]
async fn details_disclosure_and_list_membership_follow_current_grant() {
    let (f, controller) = fixture().await;
    let full = details(&f, &controller).await;
    assert!(full.workdir_connections.is_some());
    assert!(full.restore_observation_token.is_some());
    f.api
        .store
        .revoke_worker_control_grant(TEST_WORKSPACE_ID, "list-control", &now_registry_timestamp())
        .unwrap();
    assert!(list(&f, &controller).await.items.is_empty());
    let denied = known_worker_details(
        State(f.api.clone()),
        AxumPath(ScopedRuntimeWorkerPath {
            workspace_id: TEST_WORKSPACE_ID.into(),
            worker: f.worker.clone(),
        }),
        ticket_check_headers(&controller),
    )
    .await
    .unwrap_err();
    assert_eq!(denied.into_response().status(), StatusCode::NOT_FOUND);
    f.api
        .store
        .create_worker_control_grant(&WorkerControlGrantRecord {
            workspace_id: TEST_WORKSPACE_ID.into(),
            grant_id: "notify-only".into(),
            controller: controller.clone(),
            subject: f.worker.clone(),
            relation: "peer".into(),
            origin: "fixture".into(),
            permissions: vec!["notify".into()],
            operation_id: "notify-only".into(),
            created_at: now_registry_timestamp(),
            revoked_at: None,
        })
        .unwrap();
    let limited = details(&f, &controller).await;
    assert!(limited.workdir_connections.is_none());
    assert!(limited.restore_observation_token.is_none());
    assert_eq!(list(&f, &controller).await.items[0].permissions, ["notify"]);
}

#[tokio::test]
async fn known_list_is_grant_bounded_and_same_names_do_not_collapse_identity() {
    let (mut f, controller) = fixture().await;
    let other = RuntimeWorkerRef::new(&f.worker.runtime_id, "same-name-other");
    let mut summary = f
        .runtime
        .workers
        .lock()
        .unwrap()
        .iter()
        .find(|worker| worker.worker == f.worker)
        .unwrap()
        .clone();
    summary.worker = other.clone();
    f.runtime.workers.lock().unwrap().push(summary);
    seed_worker_source_member(&f.api, &other.runtime_id, &other.worker_id);
    assert_eq!(
        list(&f, &controller).await.items.len(),
        1,
        "Workspace membership is not a control grant"
    );
    f.api
        .store
        .create_worker_control_grant(&WorkerControlGrantRecord {
            workspace_id: TEST_WORKSPACE_ID.into(),
            grant_id: "second-control".into(),
            controller: controller.clone(),
            subject: other,
            relation: "peer".into(),
            origin: "fixture".into(),
            permissions: vec!["notify".into()],
            operation_id: "second-control".into(),
            created_at: now_registry_timestamp(),
            revoked_at: None,
        })
        .unwrap();
    let both = list(&f, &controller).await;
    assert_eq!(both.items.len(), 2);
    assert_eq!(both.items[0].display_name, both.items[1].display_name);
    assert_ne!(both.items[0].worker, both.items[1].worker);
    assert_ne!(both.items[0].subject, both.items[1].subject);
    f.api.config.max_records = 1;
    let bounded = list(&f, &controller).await;
    assert_eq!(bounded.limit, 1);
    assert_eq!(bounded.items.len(), 1);
    assert!(bounded.truncated);
}
