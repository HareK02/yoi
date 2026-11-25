//! Operation HTTP -> Runtime -> canonical FsStore integration. Most cases use the
//! existing isolated Controller fixture with scripted model I/O; the signed-scope
//! case substitutes execution only. Ordinary fs-store lib tests.
use super::*;
use crate::fs_store::AtomicWriteFault;
use runtime_api::{
    WorkerWorkdirAttachmentsOperationObservation as Observation,
    WorkerWorkdirAttachmentsOperationOutcome as Outcome,
    WorkerWorkdirAttachmentsOperationReceipt as Receipt,
    WorkerWorkdirAttachmentsOperationRequest as Operation,
};

fn request(id: &str, generation: &str) -> Operation {
    Operation {
        request_id: id.into(),
        expected_generation: generation.into(),
        logical_workdir_attachments: vec![runtime_api::LogicalWorkdirAttachment {
            alias: workdir::WorkdirAttachmentAlias::new("checkout").unwrap(),
            working_directory_id: "remote-operation-workdir".into(),
            capabilities: workdir::WorkdirSessionCapabilities::READ_ONLY,
        }],
    }
}

async fn post<T: Serialize>(
    app: &Router,
    worker: &WorkerRef,
    suffix: &str,
    request: &T,
) -> Response {
    app.clone()
        .oneshot(
            axum::http::Request::builder()
                .method(Method::POST)
                .uri(format!(
                    "/v1/workers/{}/workdir-attachments{suffix}",
                    worker.worker_id
                ))
                .header(header::AUTHORIZATION, "Bearer attachment-operation-token")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(request).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn json<T: serde::de::DeserializeOwned>(response: Response) -> T {
    assert_eq!(response.status(), StatusCode::OK);
    serde_json::from_slice(
        &to_bytes(response.into_body(), 8 * 1024 * 1024)
            .await
            .unwrap(),
    )
    .unwrap()
}

async fn generation(app: &Router, worker: &WorkerRef) -> String {
    let observation: runtime_api::WorkerWorkdirAttachmentsObservation = json(
        post(
            app,
            worker,
            "/observe",
            &runtime_api::WorkerWorkdirAttachmentsRequest {
                logical_workdir_attachments: Vec::new(),
            },
        )
        .await,
    )
    .await;
    observation.current_generation.unwrap()
}

fn canonical_path(
    fixture: &super::super::controller_fixture::ControllerFixture,
) -> std::path::PathBuf {
    fixture
        .root_path()
        .join("runtime/workers")
        .join(fixture.worker.worker_id.to_string())
        .join("worker.json")
}

#[tokio::test]
async fn attachment_operation_http_replays_original_receipt_after_supersession_and_conflicts_on_reuse()
 {
    let fixture = super::super::controller_fixture::ControllerFixture::persisted();
    let runtime = fixture.runtime();
    let worker = &fixture.worker.worker_ref;
    let app = runtime_http_router(runtime.clone(), "attachment-operation-token".into());
    let initial = generation(&app, worker).await;
    let first_request = request("attempt-one", &initial);
    let absent: Observation =
        json(post(&app, worker, "/operations/observe", &first_request).await).await;
    assert_eq!(absent.result, Outcome::Absent);
    let first: Receipt = json(post(&app, worker, "/operations", &first_request).await).await;
    assert_ne!(first.resulting_generation, initial);
    let path = canonical_path(&fixture);
    let first_bytes = std::fs::read(&path).unwrap();
    let replay: Receipt = json(post(&app, worker, "/operations", &first_request).await).await;
    assert_eq!(replay, first);
    assert_eq!(std::fs::read(&path).unwrap(), first_bytes);
    let stale_attempt = request("different-attempt-same-set", &initial);
    assert_eq!(
        post(&app, worker, "/operations", &stale_attempt)
            .await
            .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(std::fs::read(&path).unwrap(), first_bytes);
    let second_request = request("different-attempt-same-set", &first.resulting_generation);
    let unfulfilled: Observation =
        json(post(&app, worker, "/operations/observe", &second_request).await).await;
    assert_eq!(unfulfilled.result, Outcome::Absent);
    let matching_set: runtime_api::WorkerWorkdirAttachmentsObservation = json(
        post(
            &app,
            worker,
            "/observe",
            &runtime_api::WorkerWorkdirAttachmentsRequest {
                logical_workdir_attachments: second_request.logical_workdir_attachments.clone(),
            },
        )
        .await,
    )
    .await;
    assert_eq!(
        matching_set.result,
        runtime_api::WorkerWorkdirAttachmentsMatch::Matching
    );
    let second: Receipt = json(post(&app, worker, "/operations", &second_request).await).await;
    assert_ne!(first.resulting_generation, second.resulting_generation);
    let third_request = Operation {
        request_id: "superseding-removal".into(),
        expected_generation: second.resulting_generation.clone(),
        logical_workdir_attachments: Vec::new(),
    };
    let third: Receipt = json(post(&app, worker, "/operations", &third_request).await).await;
    let set: runtime_api::WorkerWorkdirAttachmentsObservation = json(
        post(
            &app,
            worker,
            "/observe",
            &runtime_api::WorkerWorkdirAttachmentsRequest {
                logical_workdir_attachments: first_request.logical_workdir_attachments.clone(),
            },
        )
        .await,
    )
    .await;
    assert_eq!(
        set.result,
        runtime_api::WorkerWorkdirAttachmentsMatch::Different
    );
    let settled = std::fs::read(&path).unwrap();
    let historical: Observation =
        json(post(&app, worker, "/operations/observe", &first_request).await).await;
    assert_eq!(
        historical.result,
        Outcome::Receipt {
            receipt: first.clone()
        }
    );
    assert_eq!(
        historical.current_generation,
        Some(third.resulting_generation.clone())
    );
    let replay: Receipt = json(post(&app, worker, "/operations", &first_request).await).await;
    assert_eq!(replay, first);
    for component in 0..4 {
        let mut changed = first_request.clone();
        // Each full input component remains bound, even after later operations.
        match component {
            0 => changed.expected_generation = second.resulting_generation.clone(),
            1 => {
                changed.logical_workdir_attachments[0].working_directory_id =
                    "changed-workdir".into()
            }
            2 => {
                changed.logical_workdir_attachments[0].alias =
                    workdir::WorkdirAttachmentAlias::new("other").unwrap()
            }
            _ => {
                changed.logical_workdir_attachments[0].capabilities =
                    workdir::WorkdirSessionCapabilities::ALL
            }
        }
        let conflict: Observation =
            json(post(&app, worker, "/operations/observe", &changed).await).await;
        assert_eq!(conflict.result, Outcome::Conflict);
        assert_eq!(
            post(&app, worker, "/operations", &changed).await.status(),
            StatusCode::CONFLICT
        );
    }
    assert_eq!(std::fs::read(&path).unwrap(), settled);
    fixture.assert_model_calls(0);
}

#[tokio::test]
async fn attachment_operation_http_unknown_write_is_queryable_without_adoption_and_fences_later_writes()
 {
    for fault in [
        AtomicWriteFault::BeforeRename,
        AtomicWriteFault::AfterRename,
    ] {
        let fixture = super::super::controller_fixture::ControllerFixture::persisted();
        let runtime = fixture.runtime();
        let worker = &fixture.worker.worker_ref;
        let store = runtime.test_runtime_store();
        let app = runtime_http_router(runtime.clone(), "attachment-operation-token".into());
        let initial = generation(&app, worker).await;
        let operation = request("uncertain-attempt", &initial);
        let path = canonical_path(&fixture);
        let before = std::fs::read(&path).unwrap();
        store.fail_next_worker_write(fault);
        let failed = post(&app, worker, "/operations", &operation).await;
        assert_eq!(
            failed.status(),
            if matches!(fault, AtomicWriteFault::AfterRename) {
                StatusCode::SERVICE_UNAVAILABLE
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            }
        );
        let canonical = std::fs::read(&path).unwrap();
        if matches!(fault, AtomicWriteFault::BeforeRename) {
            assert_eq!(canonical, before);
        } else {
            assert_ne!(canonical, before);
        }
        for sync in [false, true] {
            if sync {
                store.fail_next_worker_reconcile_sync();
            } else {
                store.fail_next_worker_reconcile_read();
            }
            let failed = post(&app, worker, "/operations/observe", &operation).await;
            assert_eq!(failed.status(), StatusCode::SERVICE_UNAVAILABLE);
            let body = to_bytes(failed.into_body(), 16384).await.unwrap();
            assert!(
                !String::from_utf8_lossy(&body)
                    .contains(&fixture.root_path().display().to_string())
            );
            assert_eq!(std::fs::read(&path).unwrap(), canonical);
        }
        let observed: Observation =
            json(post(&app, worker, "/operations/observe", &operation).await).await;
        if matches!(fault, AtomicWriteFault::AfterRename) {
            assert!(
                matches!(&observed.result, Outcome::Receipt { receipt } if receipt.request == operation)
            );
            assert_eq!(observed.current_generation, None);
            // A successful exact query must NOT adopt canonical memory. Even set
            // observation remains unconfirmed until mutation explicitly refreshes.
            let set: runtime_api::WorkerWorkdirAttachmentsObservation = json(
                post(
                    &app,
                    worker,
                    "/observe",
                    &runtime_api::WorkerWorkdirAttachmentsRequest {
                        logical_workdir_attachments: operation.logical_workdir_attachments.clone(),
                    },
                )
                .await,
            )
            .await;
            assert_eq!(
                set.result,
                runtime_api::WorkerWorkdirAttachmentsMatch::Unconfirmed
            );
            assert_eq!(set.current_generation, None);
            // Another write cannot blindly overwrite the canonical unseen receipt.
            store.fail_next_worker_reconcile_read();
            assert_eq!(
                post(
                    &app,
                    worker,
                    "/operations",
                    &request("later-attempt", &initial)
                )
                .await
                .status(),
                StatusCode::SERVICE_UNAVAILABLE
            );
            assert_eq!(std::fs::read(&path).unwrap(), canonical);
            let replay: Receipt = json(post(&app, worker, "/operations", &operation).await).await;
            assert_eq!(
                observed.result,
                Outcome::Receipt {
                    receipt: replay.clone()
                }
            );
            assert_eq!(std::fs::read(&path).unwrap(), canonical);
            assert_eq!(generation(&app, worker).await, replay.resulting_generation);
        } else {
            assert_eq!(observed.result, Outcome::Absent);
            assert_eq!(observed.current_generation, Some(initial));
            let receipt: Receipt = json(post(&app, worker, "/operations", &operation).await).await;
            assert_eq!(receipt.request, operation);
        }
        fixture.assert_model_calls(0);
    }
}

#[tokio::test]
async fn attachment_operation_http_requires_auth_and_rejects_unknown_fields_without_writing() {
    let fixture = super::super::controller_fixture::ControllerFixture::persisted();
    let runtime = fixture.runtime();
    let worker = &fixture.worker.worker_ref;
    let app = runtime_http_router(runtime.clone(), "attachment-operation-token".into());
    let operation = request("authenticated-attempt", &generation(&app, worker).await);
    let path = canonical_path(&fixture);
    let before = std::fs::read(&path).unwrap();
    for suffix in ["/operations", "/operations/observe", "/observe"] {
        let response = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method(Method::POST)
                    .uri(format!(
                        "/v1/workers/{}/workdir-attachments{suffix}",
                        worker.worker_id
                    ))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_vec(&operation).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
    for suffix in ["/operations", "/operations/observe"] {
        let mut invalid = serde_json::to_value(&operation).unwrap();
        invalid["owner"] = "untrusted-owner".into();
        assert_eq!(
            post(&app, worker, suffix, &invalid).await.status(),
            StatusCode::BAD_REQUEST
        );
        let mut invalid = serde_json::to_value(&operation).unwrap();
        invalid["logical_workdir_attachments"][0]["host_path"] = "/private/runtime-host".into();
        assert_eq!(
            post(&app, worker, suffix, &invalid).await.status(),
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[tokio::test]
async fn attachment_operation_http_signed_scope_binds_worker_workspace_operation_and_body() {
    // Existing stateless execution substitute; authentication, scope checks and
    // canonical persistence remain production implementations.
    let temp = tempfile::tempdir().unwrap();
    let runtime = Runtime::with_fs_store_and_execution_backend(
        crate::fs_store::FsRuntimeStoreOptions::new(temp.path().join("runtime"))
            .with_runtime_id("runtime-test"),
        Arc::new(AcceptingBackend),
    )
    .unwrap();
    let identity = RuntimeIdentityMaterial::generate("attachment-workspace-key").unwrap();
    let trust = workspace_trust(&identity);
    let owner = RuntimeWorkspaceScope::new("workspace-a", trust.backend_url.clone());
    let mut create = task_request("signed attachment operation");
    create.workspace_api = Some(WorkspaceApiRef {
        workspace_id: "workspace-a".into(),
        base_url: "https://backend.test".into(),
    });
    runtime
        .store_config_bundle(test_bundle(create.profile.clone()))
        .unwrap();
    let worker = runtime.create_worker_scoped(&owner, create).unwrap();
    let current = runtime
        .observe_worker_workdir_attachments_state_scoped(&owner, &worker.worker_ref, Vec::new())
        .unwrap()
        .current_generation
        .unwrap();
    let operation = request("signed-attempt", &current);
    let mut other_workspace = trust.clone();
    other_workspace.workspace_id = "workspace-b".into();
    let app = runtime_http_router_with_workspace_auth(
        runtime.clone(),
        None,
        workspace_test_auth(vec![trust.clone(), other_workspace.clone()]),
    );
    let mut receipt = None;
    for suffix in ["/operations", "/operations/observe"] {
        let path = format!(
            "/v1/workers/{}/workdir-attachments{suffix}",
            worker.worker_id
        );
        let body = serde_json::to_vec(&operation).unwrap();
        let claims = signed_claims(&trust, "POST", &path, &body);
        let response =
            signed_request(app.clone(), &identity, &claims, body.clone(), "workspace-a").await;
        if suffix == "/operations" {
            receipt = Some(json::<Receipt>(response).await);
        } else {
            let observed: Observation = json(response).await;
            assert_eq!(
                observed.result,
                Outcome::Receipt {
                    receipt: receipt.clone().unwrap()
                }
            );
        }
        let claims = signed_claims(&other_workspace, "POST", &path, &body);
        assert_eq!(
            signed_request(app.clone(), &identity, &claims, body.clone(), "workspace-b")
                .await
                .status(),
            StatusCode::NOT_FOUND
        );
        for mismatch in 0..3 {
            let mut claims = signed_claims(&trust, "POST", &path, &body);
            match mismatch {
                0 => claims.worker_id = Some(WorkerId::now_v7().to_string()),
                1 => claims.operation = "workers:read".into(),
                _ => claims.body_digest = workspace_request_body_digest(b"{}"),
            }
            assert_eq!(
                signed_request(app.clone(), &identity, &claims, body.clone(), "workspace-a")
                    .await
                    .status(),
                StatusCode::UNAUTHORIZED
            );
        }
    }
}

#[tokio::test]
async fn attachment_operation_aggregate_backpressure_does_not_evict_receipt_or_change_generation() {
    let fixture = super::super::controller_fixture::ControllerFixture::persisted();
    let runtime = fixture.runtime();
    let worker = &fixture.worker.worker_ref;
    let app = runtime_http_router(runtime.clone(), "attachment-operation-token".into());
    let operation = request("retained-attempt", &generation(&app, worker).await);
    let receipt: Receipt = json(post(&app, worker, "/operations", &operation).await).await;
    let path = canonical_path(&fixture);
    let before = std::fs::read(&path).unwrap();
    // Direct owner admission tests the canonical aggregate bound, not the HTTP
    // body limit. A large new owner cannot erase a previous request conflict.
    let too_large = request(&"x".repeat(8 * 1024 * 1024), &receipt.resulting_generation);
    assert!(
        matches!(runtime.apply_worker_workdir_attachments_operation(worker, too_large), Err(RuntimeError::InvalidRequest(message)) if message.contains("canonical reader limit"))
    );
    let observed: Observation =
        json(post(&app, worker, "/operations/observe", &operation).await).await;
    assert_eq!(
        observed.result,
        Outcome::Receipt {
            receipt: receipt.clone()
        }
    );
    assert_eq!(
        observed.current_generation,
        Some(receipt.resulting_generation)
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[tokio::test]
async fn attachment_operation_http_receipt_survives_catalog_reopen_without_duplicate_effect() {
    let mut fixture = super::super::controller_fixture::ControllerFixture::persisted();
    let runtime = fixture.runtime().clone();
    let worker = fixture.worker.worker_ref.clone();
    let app = runtime_http_router(runtime.clone(), "attachment-operation-token".into());
    let operation = request("restart-attempt", &generation(&app, &worker).await);
    fixture.stop();
    runtime
        .test_runtime_store()
        .fail_next_worker_write(AtomicWriteFault::AfterRename);
    assert_eq!(
        post(&app, &worker, "/operations", &operation)
            .await
            .status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    let path = canonical_path(&fixture);
    let canonical = std::fs::read(&path).unwrap();
    drop(app);
    drop(runtime);
    fixture.close_catalog();
    let reopened = Runtime::with_fs_store_and_execution_backend(
        crate::fs_store::FsRuntimeStoreOptions::new(fixture.root_path().join("runtime"))
            .with_runtime_id("http-controller-fixture"),
        fixture.empty_backend(),
    )
    .unwrap();
    let app = runtime_http_router(reopened.clone(), "attachment-operation-token".into());
    let observed: Observation =
        json(post(&app, &worker, "/operations/observe", &operation).await).await;
    let Outcome::Receipt { receipt } = observed.result else {
        panic!("restart lost canonical receipt")
    };
    assert_eq!(
        observed.current_generation,
        Some(receipt.resulting_generation.clone())
    );
    let replay: Receipt = json(post(&app, &worker, "/operations", &operation).await).await;
    assert_eq!(replay, receipt);
    assert_eq!(std::fs::read(&path).unwrap(), canonical);
    fixture.assert_model_calls(0);
}
