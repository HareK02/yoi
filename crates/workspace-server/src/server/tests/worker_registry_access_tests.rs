//! Worker reads and input admission use the real Backend handlers, SQLite catalog,
//! and Ticket store; only Runtime execution is replaced by the shared fixture.
//! Run with `cargo test -p yoi-workspace-server --lib worker_registry_access_tests`.

use super::*;

async fn assigned_fixture() -> ManualCoderAssignmentFixture {
    let fixture = manual_worker_assignment_fixture().await;
    let _ = scoped_set_ticket_assignment(
        State(fixture.api.clone()),
        AxumPath((
            TEST_WORKSPACE_ID.into(),
            fixture.ticket_id.clone(),
            "worker".into(),
        )),
        Json(manual_worker_assignment_request(&fixture, "bind-workdirs")),
    )
    .await
    .unwrap();
    fixture
}

async fn show_worker(
    api: &WorkspaceApi,
    worker: &RuntimeWorkerRef,
) -> ApiResult<Json<server_api::WorkerSummary>> {
    let resource_key = api
        .store
        .worker_resource_key(TEST_WORKSPACE_ID, worker)?
        .unwrap();
    scoped_get_workspace_worker(
        State(api.clone()),
        AxumPath(ScopedWorkspaceWorkerReferencePath {
            workspace_id: TEST_WORKSPACE_ID.into(),
            worker_ref: resource_key,
        }),
    )
    .await
}

#[tokio::test]
async fn target_edits_preserve_worker_reads_and_input_but_deny_mismatched_workdirs() {
    for clear_targets in [false, true] {
        let fixture = assigned_fixture().await;
        let before_links = fixture
            .api
            .store
            .list_worker_workdir_links(TEST_WORKSPACE_ID, &fixture.worker)
            .unwrap();
        browser_ticket_backend(&fixture.api)
            .unwrap()
            .edit_item(
                fixture.ticket_id.clone().into(),
                TicketItemEdit {
                    targets: Some(if clear_targets {
                        TicketTargetsEdit::Clear
                    } else {
                        TicketTargetsEdit::Set {
                            // Remove the checkout Repository, not merely its
                            // selector: creation selectors are provenance and
                            // must not become access constraints.
                            targets: vec![test_ticket_target(
                                "docs",
                                "develop",
                                TicketTargetAccess::ReadOnly,
                            )],
                        }
                    }),
                    ..Default::default()
                },
            )
            .unwrap();

        let Json(list) = list_workers(State(fixture.api.clone())).await.unwrap();
        let listed = list
            .items
            .iter()
            .find(|worker| worker.worker_id == fixture.worker.worker_id)
            .unwrap();
        let Json(shown) = show_worker(&fixture.api, &fixture.worker).await.unwrap();
        assert_eq!(listed.workdir_attachments, shown.workdir_attachments);
        assert_eq!(shown.workdir_attachments.len(), 2);
        for attachment in &shown.workdir_attachments {
            let denied = clear_targets || attachment.alias == "checkout";
            assert_eq!(attachment.effective_permissions.read, !denied);
            assert!(!attachment.effective_permissions.write);
            assert!(!attachment.effective_permissions.command);
            if denied {
                assert!(shown.diagnostics.iter().any(|diagnostic| {
                    diagnostic.code == "worker_workdir_ticket_target_mismatch"
                        && diagnostic
                            .message
                            .contains(&attachment.working_directory.working_directory_id)
                }));
                let workdir = fixture
                    .api
                    .store
                    .get_workdir_registry(
                        TEST_WORKSPACE_ID,
                        &attachment.working_directory.working_directory_id,
                    )
                    .unwrap()
                    .unwrap();
                assert_eq!(workdir.creation_selector.as_deref(), Some("develop"));
                let link = before_links
                    .iter()
                    .find(|link| link.workdir_id == workdir.workdir_id)
                    .unwrap();
                assert!(
                    effective_worker_workdir_capabilities(
                        &fixture.api,
                        &fixture.worker,
                        &workdir,
                        link.capabilities,
                    )
                    .is_err()
                );
            }
        }
        assert!(sync_runtime_worker_workdir_attachments(&fixture.api, &fixture.worker).is_err());
        let after_links = fixture
            .api
            .store
            .list_worker_workdir_links(TEST_WORKSPACE_ID, &fixture.worker)
            .unwrap();
        assert_eq!(
            before_links, after_links,
            "projection must not rewrite attachment grants"
        );
        assert!(
            fixture
                .api
                .store
                .get_active_ticket_worker_assignment(TEST_WORKSPACE_ID, &fixture.ticket_id)
                .unwrap()
                .is_some()
        );

        let result =
            WorkspaceWorker::resolve_async(&fixture.api, &shown.runtime_id, &shown.worker_id)
                .await
                .unwrap()
                .input(
                    &WorkerOperationContext::Backend,
                    WorkerInputRequest {
                        source: None,
                        kind: WorkerInputKind::User,
                        content: "Explain the Workdir mismatch".into(),
                        submission_request_id: Some(uuid::Uuid::now_v7().to_string()),
                        segments: None,
                    },
                )
                .await
                .unwrap();
        assert_eq!(result.state, InternalWorkerOperationState::Accepted);
        assert_eq!(
            fixture
                .runtime
                .input_requests
                .lock()
                .unwrap()
                .last()
                .unwrap()
                .content,
            "Explain the Workdir mismatch"
        );
    }
}

#[tokio::test]
async fn worker_detail_is_independent_of_the_list_limit() {
    let workspace = tempfile::tempdir().unwrap();
    let mut api = test_api(workspace.path()).await;
    api.config.max_records = 1;
    let workers = [
        RuntimeWorkerRef::new("runtime-a", "first"),
        RuntimeWorkerRef::new("runtime-a", "second"),
    ];
    for worker in &workers {
        seed_worker_source_member(&api, &worker.runtime_id, &worker.worker_id);
    }
    let Json(list) = list_workers(State(api.clone())).await.unwrap();
    assert_eq!(list.items.len(), 1);
    let excluded = workers
        .iter()
        .find(|worker| worker.worker_id != list.items[0].worker_id)
        .unwrap();
    let Json(detail) = show_worker(&api, excluded).await.unwrap();
    assert_eq!(detail.worker_id, excluded.worker_id);
}

#[tokio::test]
async fn unrelated_projection_errors_do_not_block_worker_detail_or_become_grants() {
    let fixture = assigned_fixture().await;
    fixture
        .api
        .config_store
        .with_conn(|conn| {
            conn.execute_batch("DROP TABLE typed_ticket_targets")?;
            Ok(())
        })
        .unwrap();
    let unrelated = RuntimeWorkerRef::new("other-runtime", "unrelated-worker");
    seed_worker_source_member(&fixture.api, &unrelated.runtime_id, &unrelated.worker_id);
    let Json(detail) = show_worker(&fixture.api, &unrelated).await.unwrap();
    assert_eq!(detail.worker_id, unrelated.worker_id);
    let error = show_worker(&fixture.api, &fixture.worker)
        .await
        .unwrap_err();
    assert_eq!(
        api_error_status(&error.error),
        StatusCode::INTERNAL_SERVER_ERROR
    );
}
