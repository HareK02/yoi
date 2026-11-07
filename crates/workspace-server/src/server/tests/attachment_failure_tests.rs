//! Ordinary Server lib tests: existing isolated SQLite/Worker fixture -> signed
//! self-attachment HTTP -> production detach, compensation and generated decoder.
//! Only Runtime attachment settlement and provider close faults are substituted;
//! authorization, current identity, ledger CAS, restoration and projection are real.
//! The close wrapper forwards a real Local provider (the sibling HTTP close-fault
//! fixture is private), and exercises the production multi-session close aggregate.
use super::*;
use crate::hosts::{RuntimeOperationResponse, RuntimeOperationStatus, WorkspaceWorkerRuntime};
use protocol::control_failure::RuntimeControlCause as C;
use server_api::WorkdirManagementApiError;
use server_api::workdir_management_error::{
    WorkdirAttachmentFailureStage as Stage, WorkdirManagementCause as Cause,
};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tower::ServiceExt;
use workdir::failure::{WorkdirFailureDetails, WorkdirFailureOperation};
use workdir::management_failure::{
    WorkdirManagementFailure, WorkdirManagementOperation as Op, WorkdirManagementReason as Reason,
};

const PRIVATE_RUNTIME: &str = "/private/runtime/attachment token=ATTACHMENT_SECRET";
const PRIVATE_SQL: &str = "/private/sqlite/restore token=RESTORE_SECRET";
const PRIVATE_CLOSE: &str = "/private/provider/close token=CLOSE_SECRET";
const WATCHDOG: std::time::Duration = std::time::Duration::from_secs(20);

type Replacements = Arc<Mutex<Vec<(String, Vec<LogicalWorkdirAttachment>)>>>;

// Reuse the fixture's actual worker identities and ordinary lookup/lifecycle
// methods; do not create a second registry or bypass current-worker authorization.
#[derive(Clone)]
struct AttachmentRuntime {
    source: WorkdirlessFixtureRuntime,
    status: RuntimeOperationStatus,
    cause: C,
    replacements: Replacements,
}

impl WorkspaceWorkerRuntime for AttachmentRuntime {
    fn runtime_id(&self) -> &str {
        self.source.runtime_id()
    }
    fn runtime_summary(&self, limit: usize) -> crate::hosts::InternalRuntimeSummary {
        self.source.runtime_summary(limit)
    }
    fn list_hosts(&self, limit: usize) -> crate::hosts::RuntimeList<InternalHostSummary> {
        self.source.list_hosts(limit)
    }
    fn list_workers(&self, limit: usize) -> crate::hosts::RuntimeList<InternalWorkerSummary> {
        self.source.list_workers(limit)
    }
    fn worker(&self, worker_id: &str) -> crate::hosts::WorkerLookupResult {
        self.source.worker(worker_id)
    }
    fn spawn_worker(
        &self,
        binding: WorkerCreateBinding,
        request: WorkerSpawnRequest,
    ) -> WorkerSpawnResult {
        self.source.spawn_worker(binding, request)
    }
    fn stop_worker(
        &self,
        worker_id: &str,
        request: WorkerLifecycleRequest,
    ) -> WorkerLifecycleResult {
        self.source.stop_worker(worker_id, request)
    }
    fn cancel_worker(
        &self,
        worker_id: &str,
        request: WorkerLifecycleRequest,
    ) -> WorkerLifecycleResult {
        self.source.cancel_worker(worker_id, request)
    }
    fn delete_worker(&self, worker_id: &str) -> crate::hosts::WorkerDeleteResult {
        self.source.delete_worker(worker_id)
    }
    fn send_input(
        &self,
        worker_id: &str,
        request: WorkerInputRequest,
    ) -> crate::hosts::WorkerInputResult {
        self.source.send_input(worker_id, request)
    }
    fn working_directory(&self, id: &str) -> crate::hosts::RuntimeWorkingDirectoryResult {
        self.source.working_directory(id)
    }
    fn open_workdir_session<'a>(
        &'a self,
        id: &'a str,
        owner: Option<&'a str>,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = std::result::Result<WorkdirSessionHandle, workdir::WorkdirError>,
                > + Send
                + 'a,
        >,
    > {
        self.source.open_workdir_session(id, owner)
    }
    fn replace_worker_workdir_attachments(
        &self,
        worker_id: &str,
        attachments: Vec<LogicalWorkdirAttachment>,
    ) -> RuntimeOperationResponse {
        self.replacements
            .lock()
            .unwrap()
            .push((worker_id.into(), attachments));
        RuntimeOperationResponse {
            status: self.status,
            cause: Some(self.cause),
            worker: Some(
                self.source
                    .worker(worker_id)
                    .into_worker(self.source.runtime_id(), worker_id)
                    .expect("fixture worker remains present"),
            ),
            diagnostics: vec![RuntimeDiagnostic {
                session: None,
                workdir: None,
                control: None,
                startup: None,
                code: "private_attachment_diagnostic".into(),
                severity: HostDiagnosticSeverity::Error,
                message: PRIVATE_RUNTIME.into(),
            }],
        }
    }
}

fn install_runtime(
    fixture: &ManualCoderAssignmentFixture,
    status: RuntimeOperationStatus,
    cause: C,
) -> Replacements {
    let replacements = Arc::new(Mutex::new(Vec::new()));
    fixture.api.runtime.register_or_replace(AttachmentRuntime {
        source: fixture.runtime.clone(),
        status,
        cause,
        replacements: replacements.clone(),
    });
    replacements
}

fn checkout_link(fixture: &ManualCoderAssignmentFixture) -> WorkerWorkdirLinkRecord {
    fixture
        .api
        .store
        .list_worker_workdir_links(TEST_WORKSPACE_ID, &fixture.worker)
        .unwrap()
        .into_iter()
        .find(|link| link.alias == "checkout")
        .unwrap()
}

fn assert_detached(fixture: &ManualCoderAssignmentFixture, original: &WorkerWorkdirLinkRecord) {
    assert!(
        fixture
            .api
            .store
            .list_worker_workdir_links(TEST_WORKSPACE_ID, &fixture.worker)
            .unwrap()
            .iter()
            .all(|link| link.alias != original.alias)
    );
    // The ordinary Store list deliberately returns active links only. Inspect
    // the actual historical SQLite row to distinguish detached from deleted.
    let (connection_id, unlinked_at): (String, Option<String>) = fixture
        .api
        .config_store
        .with_conn(|conn| {
            Ok(conn.query_row(
                "SELECT connection_id, unlinked_at FROM worker_workdir_links
             WHERE workspace_id=?1 AND runtime_id=?2 AND worker_id=?3 AND connection_id=?4",
                rusqlite::params![
                    TEST_WORKSPACE_ID,
                    fixture.worker.runtime_id,
                    fixture.worker.worker_id,
                    original.connection_id
                ],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?)
        })
        .unwrap();
    assert_eq!(connection_id, original.connection_id);
    assert!(
        unlinked_at.is_some(),
        "the same lifetime must remain durably detached"
    );
}

fn reject_restore(api: &WorkspaceApi) {
    api.config_store.with_conn(|conn| {
        conn.execute_batch(
            "CREATE TRIGGER attachment_restore_failure BEFORE UPDATE OF unlinked_at ON worker_workdir_links
             WHEN OLD.unlinked_at IS NOT NULL AND NEW.unlinked_at IS NULL
             BEGIN SELECT RAISE(ABORT, '/private/sqlite/restore token=RESTORE_SECRET'); END;",
        )?;
        Ok(())
    }).unwrap();
}

fn detach_request(
    identity: &worker_runtime::auth::RuntimeIdentityMaterial,
    worker: &RuntimeWorkerRef,
    link: &WorkerWorkdirLinkRecord,
) -> Request<Body> {
    runtime_source_request(
        identity,
        Some(&worker.worker_id),
        "DELETE",
        &format!(
            "/api/w/{TEST_WORKSPACE_ID}/workers/self/workdir-attachments/{}?expected_connection_id={}",
            link.alias, link.connection_id,
        ),
        Vec::new(),
    )
}

async fn decode_response(response: Response) -> (WorkdirManagementApiError, Value) {
    let status = response.status();
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    let media = response.headers()[CONTENT_TYPE]
        .to_str()
        .unwrap()
        .to_owned();
    assert_eq!(media, error_contract::problem::PROBLEM_CONTENT_TYPE);
    let bytes = to_bytes(response.into_body(), error_contract::MAX_FAILURE_BYTES)
        .await
        .unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    for private in [
        PRIVATE_RUNTIME,
        PRIVATE_SQL,
        PRIVATE_CLOSE,
        "/private/",
        "ATTACHMENT_SECRET",
        "RESTORE_SECRET",
        "CLOSE_SECRET",
        "private_attachment_diagnostic",
    ] {
        assert!(
            !text.contains(private),
            "private cause escaped into HTTP: {text}"
        );
    }
    let decoded = WorkdirManagementApiError::decode_received(status.as_u16(), &media, &bytes)
        .expect("generated client error decoder must accept the production HTTP failure");
    (decoded, serde_json::from_slice(&bytes).unwrap())
}

fn related_management(
    error: &WorkdirManagementApiError,
    stage: Stage,
    reason: Reason,
    operation: Op,
) {
    assert_eq!(error.related_failures().len(), 1);
    let related = &error.related_failures()[0];
    assert_eq!(related.stage, stage);
    let attachment = related
        .failure
        .as_ref()
        .expect("independent compensation cause retained");
    match attachment.decode::<WorkdirManagementFailure>() {
        error_contract::DecodeFailure::Known(received) => assert_eq!(
            received.facts().cause(),
            &WorkdirManagementFailure { operation, reason },
        ),
        _ => panic!("compensation failure is not decodable as a management cause"),
    }
}

#[tokio::test]
async fn signed_self_detach_unknown_preserves_runtime_cause_and_does_not_restore_or_retry() {
    tokio::time::timeout(WATCHDOG, async {
        let mut fixture = manual_worker_assignment_fixture().await;
        let identity =
            worker_runtime::auth::RuntimeIdentityMaterial::generate(&fixture.worker.runtime_id)
                .unwrap();
        configure_runtime_request_auth(&mut fixture.api, &identity, &fixture.worker.runtime_id);
        let original = checkout_link(&fixture);
        let replacements = install_runtime(
            &fixture,
            RuntimeOperationStatus::Unknown,
            C::PersistenceOutcomeUnknown,
        );
        // An accidental compensation would fail and appear as an extra cause.
        reject_restore(&fixture.api);

        let mut forged = detach_request(&identity, &fixture.worker, &original);
        forged
            .headers_mut()
            .insert("x-yoi-worker-id", "foreign-worker".parse().unwrap());
        assert_eq!(
            build_router(fixture.api.clone())
                .oneshot(forged)
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(checkout_link(&fixture), original);
        assert!(replacements.lock().unwrap().is_empty());

        let response = build_router(fixture.api.clone())
            .oneshot(detach_request(&identity, &fixture.worker, &original))
            .await
            .unwrap();
        let (error, body) = decode_response(response).await;
        assert_eq!(
            error.cause(),
            Some(&Cause::Runtime(C::PersistenceOutcomeUnknown))
        );
        assert_eq!(body["detail"], C::PersistenceOutcomeUnknown.explanation());
        assert!(
            error.related_failures().is_empty(),
            "Unknown must not attempt restore"
        );
        assert_detached(&fixture, &original);
        let calls = replacements.lock().unwrap();
        assert_eq!(
            calls.len(),
            1,
            "no resend or inverse Runtime update after Unknown"
        );
        assert_eq!(calls[0].0, fixture.worker.worker_id);
        assert!(
            calls[0]
                .1
                .iter()
                .all(|attachment| attachment.alias.as_str() != "checkout")
        );
        assert!(
            calls[0]
                .1
                .iter()
                .any(|attachment| attachment.alias.as_str() == "docs")
        );
    })
    .await
    .expect("signed Unknown detach exceeded watchdog");
}

#[tokio::test]
async fn signed_self_detach_runtime_rejection_keeps_failed_sqlite_restore_independent() {
    tokio::time::timeout(WATCHDOG, async {
        let mut fixture = manual_worker_assignment_fixture().await;
        let identity =
            worker_runtime::auth::RuntimeIdentityMaterial::generate(&fixture.worker.runtime_id)
                .unwrap();
        configure_runtime_request_auth(&mut fixture.api, &identity, &fixture.worker.runtime_id);
        let original = checkout_link(&fixture);
        let replacements = install_runtime(
            &fixture,
            RuntimeOperationStatus::Rejected,
            C::PersistenceFailed,
        );
        reject_restore(&fixture.api);
        let response = build_router(fixture.api.clone())
            .oneshot(detach_request(&identity, &fixture.worker, &original))
            .await
            .unwrap();
        let (error, body) = decode_response(response).await;
        assert_eq!(error.cause(), Some(&Cause::Runtime(C::PersistenceFailed)));
        assert_eq!(body["detail"], C::PersistenceFailed.explanation());
        related_management(
            &error,
            Stage::RestoreBackendLink,
            Reason::AttachmentPersistenceFailed,
            Op::Detach,
        );
        assert_eq!(error.related_failures()[0].http_status, 500);
        assert_detached(&fixture, &original);
        assert_eq!(replacements.lock().unwrap().len(), 1);
    })
    .await
    .expect("signed rejected detach exceeded watchdog");
}

#[tokio::test]
async fn detach_compensation_preserves_recreated_same_alias_connection_and_original_session_cause()
{
    tokio::time::timeout(WATCHDOG, async {
        let fixture = manual_worker_assignment_fixture().await;
        let original = checkout_link(&fixture);
        let replacements = install_runtime(
            &fixture,
            RuntimeOperationStatus::Rejected,
            C::PersistenceFailed,
        );
        // Obtain the primary through the actual registered Runtime session-open
        // adapter, not by fabricating a projected Session cause for the assertion.
        let open_error = fixture
            .api
            .runtime
            .open_workdir_session(
                &fixture.worker.runtime_id,
                &original.workdir_id,
                Some(&fixture.worker.worker_id),
            )
            .await
            .err()
            .expect("fixture Runtime has no provider session factory")
            .into_error();
        let Error::WorkdirSessionOpenFailed { transport, .. } = &open_error else {
            panic!("session-open adapter lost its typed provider cause: {open_error:?}");
        };
        let expected = WorkdirFailureDetails {
            operation: WorkdirFailureOperation::SessionOpen,
            classification: transport.code,
            denial_reason: transport.denial_reason,
            failure_reason: transport.failure_reason,
        };
        let guard = Arc::new(
            current_worker_session_lock(&fixture.api, &fixture.worker)
                .lock_owned()
                .await,
        );
        fixture
            .api
            .store
            .detach_worker_workdir_connection(
                TEST_WORKSPACE_ID,
                &fixture.worker,
                &original.alias,
                &original.connection_id,
                TEST_CREATED_AT,
            )
            .unwrap();
        let replacement = fixture.api.store.attach_worker_workdir(&original).unwrap();
        assert_ne!(original.connection_id, replacement.connection_id);
        let error = attachment_failure::compensate_attachment(
            &fixture.api,
            &fixture.worker,
            &original,
            &guard,
            attachment_failure::Compensation::DetachNewLink,
            true,
            true,
            open_error.into(),
        )
        .await;
        assert!(matches!(
            &error.error,
            Error::WorkdirSessionOpenFailed { .. }
        ));
        assert_eq!(error.attachment_failures.len(), 1);
        assert!(matches!(
            &error.attachment_failures[0].1.error,
            Error::WorkdirAttachmentConflict(_)
        ));
        assert_eq!(
            checkout_link(&fixture),
            replacement,
            "CAS must not detach a recreated identical alias/workdir"
        );
        assert!(
            replacements.lock().unwrap().is_empty(),
            "failed connection CAS cannot authorize Runtime sync"
        );
        assert!(
            !fixture
                .api
                .workdir_sessions
                .lock()
                .unwrap()
                .attachments
                .contains_key(&fixture.worker)
        );
        let response = api_macros::axum::http_error_response(
            StatusCode::BAD_GATEWAY,
            error.into_workdir_attachment_api_error(Op::Attach),
        );
        let (decoded, _) = decode_response(response).await;
        assert_eq!(decoded.cause(), Some(&Cause::Session(expected)));
        related_management(
            &decoded,
            Stage::DetachBackendLink,
            Reason::AttachmentConflict,
            Op::Attach,
        );
        assert_eq!(decoded.related_failures()[0].http_status, 409);
    })
    .await
    .expect("connection CAS compensation exceeded watchdog");
}

#[derive(Debug)]
struct CloseFaultSession {
    source: WorkdirSessionHandle,
    missing: bool,
    reject: Arc<AtomicBool>,
    attempts: Arc<AtomicUsize>,
}

// The provider is real for filesystem operations; only close's response is a
// deterministic transport fault. Restoration and close aggregation stay production.
macro_rules! forward_close_fault_provider {
    ($($name:ident($request:ident: $input:ty) -> $output:ty;)*) => {
        #[async_trait::async_trait]
        impl WorkdirSession for CloseFaultSession {
            fn workdir(&self) -> &workdir::Workdir { self.source.workdir() }
            fn capabilities(&self) -> workdir::WorkdirSessionCapabilities { self.source.capabilities() }
            fn environment(&self) -> workdir::WorkdirSessionEnvironment { self.source.environment() }
            fn subscribe_command_events(&self) -> Option<tokio::sync::broadcast::Receiver<workdir::CommandEvent>> {
                self.source.subscribe_command_events()
            }
            fn command_snapshot(&self) -> Vec<workdir::CommandSnapshot> { self.source.command_snapshot() }
            $(async fn $name(&self, $request: $input) -> std::result::Result<$output, workdir::WorkdirError> {
                self.source.$name($request).await
            })*
            async fn close(&self) -> std::result::Result<(), workdir::WorkdirError> {
                self.attempts.fetch_add(1, Ordering::SeqCst);
                if self.reject.load(Ordering::SeqCst) {
                    Err(if self.missing {
                        workdir::WorkdirError::NotFound(PRIVATE_CLOSE.into())
                    } else {
                        workdir::WorkdirError::Unavailable(PRIVATE_CLOSE.into())
                    })
                } else {
                    self.source.close().await
                }
            }
        }
    };
}
forward_close_fault_provider! {
    authorize_scope_path(request: workdir::WorkdirScopeAuthorizationRequest) -> ();
    scope_rules_overlap(request: workdir::WorkdirScopeOverlapRequest) -> bool;
    checkout_search(request: workdir::CheckoutSearchRequest) -> workdir::CheckoutSearchResult;
    checkout_observe(path: workdir::WorkdirPath) -> workdir::CheckoutObservation;
    checkout_execute(request: workdir::CheckoutRequest) -> workdir::CheckoutResult;
    stat(request: workdir::StatRequest) -> workdir::StatResult;
    read(request: workdir::ReadRequest) -> workdir::ReadResult;
    read_bytes(request: workdir::ReadBytesRequest) -> workdir::ReadBytesResult;
    write(request: workdir::WriteRequest) -> workdir::WriteResult;
    edit(request: workdir::EditRequest) -> workdir::EditResult;
    list(request: workdir::ListRequest) -> workdir::ListResult;
    glob(request: workdir::GlobRequest) -> workdir::GlobResult;
    grep(request: workdir::GrepRequest) -> workdir::GrepResult;
    start_command(request: workdir::CommandRequest) -> workdir::CommandHandle;
    command_status(handle: workdir::CommandHandle) -> workdir::CommandStatus;
    command_output(request: workdir::CommandOutputRequest) -> workdir::CommandOutput;
    cancel_command(handle: workdir::CommandHandle) -> ();
}

#[tokio::test]
async fn signed_self_detach_retains_every_provider_close_cause_and_same_unconfirmed_sessions() {
    tokio::time::timeout(WATCHDOG, async {
        let mut fixture = manual_worker_assignment_fixture().await;
        let identity =
            worker_runtime::auth::RuntimeIdentityMaterial::generate(&fixture.worker.runtime_id)
                .unwrap();
        configure_runtime_request_auth(&mut fixture.api, &identity, &fixture.worker.runtime_id);
        let original = checkout_link(&fixture);
        let replacements = install_runtime(
            &fixture,
            RuntimeOperationStatus::Rejected,
            C::PersistenceFailed,
        );
        let reject = Arc::new(AtomicBool::new(true));
        let mut sessions = Vec::new();
        for missing in [false, true] {
            let attempts = Arc::new(AtomicUsize::new(0));
            let session: WorkdirSessionHandle = Arc::new(CloseFaultSession {
                source: Arc::new(workdir::LocalWorkdirSession::new(
                    manifest::Scope::writable(fixture._workspace.path()).unwrap(),
                    fixture._workspace.path().to_path_buf(),
                )),
                missing,
                reject: reject.clone(),
                attempts: attempts.clone(),
            });
            // Prove forwarding reaches an actual provider before injecting close.
            session
                .stat(workdir::StatRequest {
                    path: workdir::WorkdirPath::root(),
                })
                .await
                .unwrap();
            let handle = fixture
                .api
                .workdir_sessions
                .lock()
                .unwrap()
                .register_command(
                    fixture.worker.clone(),
                    original.alias.clone(),
                    session.clone(),
                    CommandHandle(format!("close-fault-{missing}")),
                );
            sessions.push((handle, session, attempts));
        }
        let response = build_router(fixture.api.clone())
            .oneshot(detach_request(&identity, &fixture.worker, &original))
            .await
            .unwrap();
        let (error, _) = decode_response(response).await;
        let Some(Cause::Session(primary)) = error.cause() else {
            panic!("primary close cause lost: {error:?}")
        };
        assert_eq!(primary.operation, WorkdirFailureOperation::SessionClose);
        assert_eq!(
            error.related_failures().len(),
            1,
            "all provider errors, not just Error::source(), must survive"
        );
        let related = &error.related_failures()[0];
        assert_eq!(related.stage, Stage::SessionClose);
        assert_eq!(related.http_status, 502);
        let secondary = match related
            .failure
            .as_ref()
            .unwrap()
            .decode::<WorkdirFailureDetails>()
        {
            error_contract::DecodeFailure::Known(received) => *received.facts().cause(),
            _ => panic!("second provider close cause is not decodable as a Session cause"),
        };
        assert_eq!(secondary.operation, WorkdirFailureOperation::SessionClose);
        let classifications = [primary.classification, secondary.classification];
        assert!(classifications.contains(&workdir::http::WorkdirTransportErrorCode::NotFound));
        assert!(classifications.contains(&workdir::http::WorkdirTransportErrorCode::Unavailable));
        assert_eq!(
            checkout_link(&fixture),
            original,
            "close failure precedes the detach mutation"
        );
        assert!(replacements.lock().unwrap().is_empty());
        {
            let registry = fixture.api.workdir_sessions.lock().unwrap();
            for (handle, session, attempts) in &sessions {
                assert_eq!(
                    attempts.load(Ordering::SeqCst),
                    1,
                    "unconfirmed close must not silently resend"
                );
                let retained = registry
                    .command(&fixture.worker, &original.alias, handle)
                    .unwrap();
                assert!(
                    Arc::ptr_eq(&retained.source, session),
                    "restore the SAME unconfirmed provider session"
                );
            }
        }
        reject.store(false, Ordering::SeqCst);
        let _guard = current_worker_session_lock(&fixture.api, &fixture.worker)
            .lock_owned()
            .await;
        close_current_worker_command_sessions_for_alias_locked(
            &fixture.api,
            &fixture.worker,
            &original.alias,
        )
        .await
        .unwrap();
        let registry = fixture.api.workdir_sessions.lock().unwrap();
        for (handle, _, attempts) in &sessions {
            assert_eq!(attempts.load(Ordering::SeqCst), 2);
            assert!(
                registry
                    .command(&fixture.worker, &original.alias, handle)
                    .is_none()
            );
        }
    })
    .await
    .expect("multi-provider close failure exceeded watchdog");
}
