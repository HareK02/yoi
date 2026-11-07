use super::*;
use crate::fs_store::AtomicWriteFault;
use protocol::control_failure::RuntimeControlCause as C;

#[tokio::test]
async fn attachment_replace_http_preserves_write_cause_and_does_not_resend() {
    for (fault, cause, status) in [
        (
            AtomicWriteFault::BeforeRename,
            C::PersistenceFailed,
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
        (
            AtomicWriteFault::AfterRename,
            C::PersistenceOutcomeUnknown,
            StatusCode::SERVICE_UNAVAILABLE,
        ),
    ] {
        let fixture = super::super::controller_fixture::ControllerFixture::persisted();
        let runtime = fixture.runtime();
        let worker_ref = &fixture.worker.worker_ref;
        let store = runtime.test_runtime_store();
        let before = store.read_worker_record(worker_ref).unwrap();
        assert!(before.logical_workdir_attachments.is_empty());
        let attachments = vec![crate::catalog::LogicalWorkdirAttachment {
            alias: workdir::WorkdirAttachmentAlias::new("checkout").unwrap(),
            working_directory_id: "remote-logical-workdir".into(),
            capabilities: workdir::WorkdirSessionCapabilities::READ_ONLY,
        }];
        let request = runtime_api::WorkerWorkdirAttachmentsRequest {
            logical_workdir_attachments: serde_json::from_value(
                serde_json::to_value(&attachments).unwrap(),
            )
            .unwrap(),
        };
        let token = "attachment-token";
        let app = runtime_http_router(runtime.clone(), token.into());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let mut server = tokio::task::JoinSet::new();
        server.spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let client = runtime_api::RuntimeApiClient::builder(&format!("http://{address}"))
            .unwrap()
            .authorizer(TestRuntimeAuthorizer(token.into()))
            .request_timeout(std::time::Duration::from_secs(5))
            .build()
            .unwrap();
        let before_observation = client
            .observe_worker_workdir_attachments(
                fixture.worker.worker_id.to_string(),
                runtime_api::WorkerWorkdirAttachmentsRequest {
                    logical_workdir_attachments: Vec::new(),
                },
            )
            .await
            .unwrap();
        assert_eq!(
            before_observation.result,
            runtime_api::WorkerWorkdirAttachmentsMatch::Matching
        );
        store.fail_next_worker_write(fault);
        // One mutation only. A new replace is not an outcome query.
        let result = client
            .replace_worker_workdir_attachments(
                fixture.worker.worker_id.to_string(),
                request.clone(),
            )
            .await;
        let error = result.unwrap_err();
        match error {
            runtime_api::client_support::ClientError::Public {
                status: received,
                error,
            } => {
                assert_eq!(received, status);
                assert_eq!(error.control_cause(), Some(cause));
                assert_eq!(error.error.message, cause.explanation());
                assert!(
                    !error
                        .error
                        .message
                        .contains(&fixture.root_path().display().to_string())
                );
            }
            other => panic!("expected typed owner problem: {other:?}"),
        }
        let canonical = store.read_worker_record(worker_ref).unwrap();
        if matches!(fault, AtomicWriteFault::AfterRename) {
            // The canonical file changed despite restoration of Runtime memory.
            assert_eq!(canonical.logical_workdir_attachments, attachments);
        } else {
            // A hidden retry would consume the fault and make this assertion fail.
            assert_eq!(
                canonical.logical_workdir_attachments,
                before.logical_workdir_attachments,
            );
        }
        assert_eq!(canonical.status, before.status);
        let canonical_path = fixture
            .root_path()
            .join("runtime/workers")
            .join(fixture.worker.worker_id.to_string())
            .join("worker.json");
        let canonical_bytes = std::fs::read(&canonical_path).unwrap();
        let observed = client
            .observe_worker_workdir_attachments(
                fixture.worker.worker_id.to_string(),
                request.clone(),
            )
            .await
            .unwrap();
        assert_eq!(
            observed.worker_ref.worker_id.to_string(),
            fixture.worker.worker_id.to_string()
        );
        assert_eq!(observed.request, request);
        assert_eq!(
            observed.result,
            if matches!(fault, AtomicWriteFault::AfterRename) {
                runtime_api::WorkerWorkdirAttachmentsMatch::Unconfirmed
            } else {
                runtime_api::WorkerWorkdirAttachmentsMatch::Different
            }
        );
        for sync_fault in [false, true] {
            if sync_fault {
                store.fail_next_worker_reconcile_sync();
            } else {
                store.fail_next_worker_reconcile_read();
            }
            let error = client
                .observe_worker_workdir_attachments(
                    fixture.worker.worker_id.to_string(),
                    request.clone(),
                )
                .await
                .unwrap_err();
            let runtime_api::client_support::ClientError::Public { error, .. } = error else {
                panic!("observation must preserve the typed storage failure")
            };
            assert_eq!(error.control_cause(), Some(C::PersistenceOutcomeUnknown));
            assert!(
                !error
                    .error
                    .message
                    .contains(&fixture.root_path().display().to_string())
            );
        }
        runtime.worker_detail(worker_ref).unwrap();
        store.read_worker_record(worker_ref).unwrap();
        assert_eq!(std::fs::read(canonical_path).unwrap(), canonical_bytes);
    }
}
