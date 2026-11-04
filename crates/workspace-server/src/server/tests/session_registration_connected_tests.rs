//! Focused T840 connection: real Subject policy -> signed Session POST -> real
//! ProfileRuntimeWorkerFactory startup -> RuntimeHTTP/Host diagnostic -> saved
//! Worker metadata. Only the unused external model is replaced. No restore,
//! operation replay, singleton-release, or Browser rendering contract is asserted.
use super::*;
use protocol::session_failure::{
    SessionPrincipal, SessionRegistrationCause, session_registration_attachment,
};
use session_store::WorkerMetadataStore as _;
use std::sync::atomic::{AtomicUsize, Ordering};
use worker_runtime::worker_backend::RuntimeWorkerFactory as _;

#[derive(Clone, Default)]
struct UnusedModel(Arc<AtomicUsize>);

#[async_trait::async_trait]
impl agen::llm_client::LlmClient for UnusedModel {
    fn clone_boxed(&self) -> Box<dyn agen::llm_client::LlmClient> {
        Box::new(self.clone())
    }

    async fn stream(
        &self,
        _: agen::llm_client::Request,
    ) -> std::result::Result<agen::llm_client::client::ResponseStream, agen::llm_client::ClientError>
    {
        self.0.fetch_add(1, Ordering::SeqCst);
        panic!("Session registration does not require a model turn")
    }
}

struct RegistrationFixture {
    connected: ConnectedWorkerHarness,
    subject: String,
    callbacks: Arc<Mutex<Vec<(StatusCode, Value)>>>,
    model: UnusedModel,
}

impl RegistrationFixture {
    async fn new(revoke_on_registration: bool) -> Self {
        // Armed only after real Subject creation with its normal default policy.
        // The callback is a deterministic synchronization point, not a fake Host
        // rejection: the original signed request and real handler remain intact.
        let revoke = Arc::new(Mutex::new(None::<(WorkspaceApi, String, PathBuf)>));
        let hook = revoke.clone();
        let callbacks = Arc::new(Mutex::new(Vec::new()));
        let observed = callbacks.clone();
        let model = UnusedModel::default();
        let connected = ConnectedWorkerHarness::start_with_layer(model.clone(), move |router| {
            router.layer(axum::middleware::from_fn(
                move |request: axum::extract::Request, next: axum::middleware::Next| {
                    let hook = hook.clone();
                    let observed = observed.clone();
                    async move {
                        if request.method() != axum::http::Method::POST
                            || request.uri().path()
                                != format!("/api/w/{TEST_WORKSPACE_ID}/subjektiv/sessions")
                        {
                            return next.run(request).await;
                        }
                        assert!(request.headers().contains_key(
                            worker_runtime::auth::RUNTIME_REQUEST_SOURCE_PROOF_HEADER
                        ));
                        let armed = hook.lock().unwrap().take();
                        let pin_root = armed.as_ref().map(|(_, _, root)| root.clone());
                        if let Some((api, subject, _)) = armed {
                            let store = open_subjektiv_store(&api).unwrap();
                            let policy = store.subject_permissions(&subject).unwrap().unwrap();
                            assert!(
                                policy
                                    .execution
                                    .iter()
                                    .any(|p| p.operation == "session.record")
                            );
                            store
                                .set_subject_permissions(
                                    &subject,
                                    &SubjectPermissionsUpdate {
                                        operation_id: "session-registration-revoke-at-callback"
                                            .into(),
                                        expected_change_id: Some(policy.change_id),
                                        execution: policy
                                            .execution
                                            .into_iter()
                                            .filter(|p| p.operation != "session.record")
                                            .collect(),
                                        delegation: policy.delegation,
                                    },
                                )
                                .unwrap();
                        }
                        let (parts, body) = request.into_parts();
                        let bytes = to_bytes(body, 2 * 1024 * 1024).await.unwrap();
                        let input: Value = serde_json::from_slice(&bytes).unwrap();
                        if let Some(root) = pin_root {
                            let request: server_api::SubjektivRecordSessionRequest = serde_json::from_slice(&bytes).unwrap();
                            let session = request.session_id.parse().unwrap();
                            let retention = worker_runtime::retention::runtime_session_retention(&root);
                            retention.pin(session, session_store::retention::SessionRetentionPin(uuid::Uuid::now_v7()), || {
                                let entries = std::fs::read_dir(root.join("workers"))?;
                                for entry in entries {
                                    let entry = entry?;
                                    let name = format!("worker-runtime-{}", entry.file_name().to_string_lossy());
                                    let store = session_store::WorkerAggregateStore::open_read_only(&entry.path(), &name)
                                        .map_err(|error| session_store::StoreError::Io(std::io::Error::other(error)))?;
                                    if store.read_by_name(&name).unwrap().and_then(|m| m.active).is_some_and(|a| a.session_id == session) { return Ok(true); }
                                }
                                Ok(false)
                            }).unwrap();
                        }
                        let response = next
                            .run(Request::from_parts(parts, Body::from(bytes)))
                            .await;
                        observed.lock().unwrap().push((response.status(), input));
                        response
                    }
                },
            ))
        })
        .await;
        let subject = open_subjektiv_store(&connected.api)
            .unwrap()
            .create_subject(
                crate::subjektiv::SubjectRole::new("session-registration-connected").unwrap(),
            )
            .unwrap()
            .id;
        let initial = open_subjektiv_store(&connected.api)
            .unwrap()
            .subject_permissions(&subject)
            .unwrap()
            .unwrap();
        assert!(
            initial
                .execution
                .iter()
                .any(|p| p.operation == "session.record")
        );
        if revoke_on_registration {
            *revoke.lock().unwrap() = Some((
                connected.api.clone(),
                subject.clone(),
                connected.runtime_root.clone(),
            ));
        }
        Self {
            connected,
            subject,
            callbacks,
            model,
        }
    }

    fn request(&self) -> CreateWorkspaceWorkerRequest {
        CreateWorkspaceWorkerRequest {
            runtime_id: CONNECTED_RUNTIME.into(),
            display_name: "Connected Session registration".into(),
            singleton_key: None,
            profile: Some("builtin:companion".into()),
            ticket_assignment: None,
            initial_submit: vec![],
            workdir_attachments: vec![],
            feature_connections: server_api::WorkspaceWorkerFeatureConnectionsRequest {
                subjektiv: Some(server_api::SubjektivWorkerConnectionRequest {
                    subject_id: self.subject.clone(),
                }),
            },
            control_operation_id: None,
            execution_grant: None,
        }
    }

    fn assert_callback(&self, expected: StatusCode) -> String {
        let callbacks = self.callbacks.lock().unwrap();
        assert!(!callbacks.is_empty(), "real signed startup callback");
        assert!(callbacks.iter().all(|(status, body)| *status == expected
            && body["session_id"] == callbacks[0].1["session_id"]));
        let (status, input) = &callbacks[0];
        assert_eq!(*status, expected);
        assert_eq!(input["create_if_missing"], true);
        assert_eq!(self.model.0.load(Ordering::SeqCst), 0);
        input["session_id"].as_str().unwrap().to_owned()
    }
}

// This deliberately inspects only persisted serde shape, not rendered error text
// or an error-contract dependency. The expected value is a typed live attachment.
fn contains_saved_value(saved: &Value, expected: &Value) -> bool {
    saved == expected
        || match saved {
            Value::Object(fields) => fields
                .values()
                .any(|value| contains_saved_value(value, expected)),
            Value::Array(values) => values
                .iter()
                .any(|value| contains_saved_value(value, expected)),
            _ => false,
        }
}

fn read_metadata(root: &std::path::Path, worker: &str) -> Value {
    // Reopen the canonical metadata store, not the separate Runtime catalog.
    let name = format!("worker-runtime-{worker}");
    let store = session_store::WorkerAggregateStore::open_read_only(
        &root.join("workers").join(worker),
        &name,
    )
    .unwrap();
    let metadata = store.read_by_name(&name).unwrap().unwrap();
    assert!(matches!(
        metadata.decode_startup_failure::<SessionRegistrationCause>(),
        session_store::worker_metadata::StartupFailureRead::Present(_)
    ));
    serde_json::to_value(metadata).unwrap()
}

#[tokio::test]
async fn session_registration_real_judgments_separate_operator_subject_and_attribution() {
    // This normal boundary fixture uses the real router, Runtime signature,
    // authority store and attribution store; its Runtime does not run a model.
    let f = Fixture::new().await;
    let path = format!("/api/w/{TEST_WORKSPACE_ID}/subjektiv/sessions");
    let missing_session = session_store::new_session_id().to_string();
    let request = json!({"session_id": missing_session, "create_if_missing": false});
    let (status, body) = response(
        &f.api,
        human_request(&f.owner, "POST", &path, Some(request.clone())),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_registration_cause(
        body,
        SessionRegistrationCause::denied(SessionPrincipal::Operator),
    );

    let signed = |request: &Value| {
        runtime_source_request(
            &f.identity,
            Some(&f.source.worker_id),
            "POST",
            &path,
            serde_json::to_vec(request).unwrap(),
        )
    };
    let (status, body) = response(&f.api, signed(&request)).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_registration_cause(body, SessionRegistrationCause::attribution_missing());

    let store = open_subjektiv_store(&f.api).unwrap();
    let foreign_session = session_store::new_session_id().to_string();
    store
        .record_session_attribution(
            crate::subjektiv::SubjectSessionAttribution::new(
                &f.subject,
                &f.source.runtime_id,
                "foreign-worker",
                &foreign_session,
            )
            .unwrap(),
        )
        .unwrap();
    let (status, body) = response(
        &f.api,
        signed(&json!({"session_id":foreign_session, "create_if_missing":false})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_registration_cause(body, SessionRegistrationCause::binding_mismatch());

    // The fixture removes its bootstrap policy only after the real setup POST.
    let (status, body) = response(
        &f.api,
        signed(&json!({"session_id":missing_session, "create_if_missing":true})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_registration_cause(
        body,
        SessionRegistrationCause::denied(SessionPrincipal::Subject),
    );
    assert!(
        store
            .session_attribution(&missing_session)
            .unwrap()
            .is_none()
    );
}

fn assert_registration_cause(body: Value, cause: SessionRegistrationCause) {
    let error: server_api::RepositoryApiError = serde_json::from_value(body).unwrap();
    assert_eq!(
        error.failure,
        Some(session_registration_attachment(cause).unwrap())
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn real_session_record_denial_survives_runtime_host_and_metadata_reopen() {
    let fixture = RegistrationFixture::new(true).await;
    // The real create preflight sees normal defaults. Revocation happens only
    // after the actual factory has issued its signed Session-registration POST.
    let error = scoped_create_workspace_worker(
        State(fixture.connected.api.clone()),
        AxumPath(ScopedWorkspacePath {
            workspace_id: TEST_WORKSPACE_ID.into(),
        }),
        HeaderMap::new(),
        Json(fixture.request()),
    )
    .await
    .unwrap_err();
    fixture.assert_callback(StatusCode::FORBIDDEN);
    let store = open_subjektiv_store(&fixture.connected.api).unwrap();
    assert!(
        store
            .subject_session_attributions(&fixture.subject)
            .unwrap()
            .is_empty()
    );
    assert!(
        !store
            .subject_permissions(&fixture.subject)
            .unwrap()
            .unwrap()
            .execution
            .iter()
            .any(|p| p.operation == "session.record")
    );

    let failures = fixture.connected.runtime_startup_failures.lock().unwrap();
    assert_eq!(
        failures.len(),
        1,
        "actual RuntimeHTTP startup error envelope"
    );
    let wire = &failures[0];
    let expected_primary = session_registration_attachment(SessionRegistrationCause::denied(
        SessionPrincipal::Subject,
    ))
    .unwrap();
    assert_eq!(wire.primary, expected_primary);
    assert_eq!(
        wire.persistence,
        Some(protocol::startup_failure::StartupPersistenceOutcome::Persisted)
    );
    let host = error
        .diagnostics
        .iter()
        .filter_map(|d| d.startup.as_ref())
        .find(|startup| startup.primary == wire.primary)
        .expect("RemoteWorkerRuntime must preserve the typed startup primary in Host diagnostics");
    assert_eq!(host.primary, wire.primary);
    assert!(
        host.secondary.iter().any(|failure| matches!(
            failure,
            protocol::startup_failure::StartupSecondaryFailure::Persistence {
                phase: protocol::startup_failure::StartupFailurePhase::RuntimeRecordDelete,
                ..
            }
        )),
        "a real Session retention pin rejects outer Runtime deletion without hiding the primary"
    );
    let expected = serde_json::to_value(&wire.primary).unwrap();
    assert!(
        expected.is_string(),
        "the shared attachment contains a codec envelope"
    );

    // A fresh fixture has exactly one attempted Worker. Discover its canonical
    // saved aggregate, without making any assumptions about retained ownership
    // or public catalog visibility after a failed create.
    let workers = std::fs::read_dir(fixture.connected.runtime_root.join("workers"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        workers.len(),
        1,
        "factory attempt must have real saved metadata"
    );
    let saved = read_metadata(&fixture.connected.runtime_root, &workers[0]);
    assert!(
        contains_saved_value(&saved, &expected),
        "saved metadata must retain the original typed Session registration denial"
    );
    let reopened = read_metadata(&fixture.connected.runtime_root, &workers[0]);
    assert_eq!(
        reopened, saved,
        "read-only reopen, not a startup operation retry"
    );
    assert!(contains_saved_value(&reopened, &expected));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn permitted_session_registration_reaches_real_factory_and_saved_confirmation() {
    let fixture = RegistrationFixture::new(false).await;
    let initial = open_subjektiv_store(&fixture.connected.api)
        .unwrap()
        .subject_permissions(&fixture.subject)
        .unwrap()
        .unwrap();
    let Json(created) = scoped_create_workspace_worker(
        State(fixture.connected.api.clone()),
        AxumPath(ScopedWorkspacePath {
            workspace_id: TEST_WORKSPACE_ID.into(),
        }),
        HeaderMap::new(),
        Json(fixture.request()),
    )
    .await
    .unwrap();
    let session_id = fixture.assert_callback(StatusCode::OK);
    let store = open_subjektiv_store(&fixture.connected.api).unwrap();
    assert_eq!(
        store
            .subject_permissions(&fixture.subject)
            .unwrap()
            .unwrap(),
        initial
    );
    let attributions = store
        .subject_session_attributions(&fixture.subject)
        .unwrap();
    assert_eq!(attributions.len(), 1);
    assert_eq!(attributions[0].session_id, session_id);
    assert_eq!(attributions[0].worker_id, created.worker_id);
    assert_eq!(attributions[0].runtime_id, CONNECTED_RUNTIME);
    assert!(
        fixture
            .connected
            .runtime_startup_failures
            .lock()
            .unwrap()
            .is_empty()
    );

    let name = format!("worker-runtime-{}", created.worker_id);
    let metadata = session_store::WorkerAggregateStore::open_read_only(
        &fixture
            .connected
            .runtime_root
            .join("workers")
            .join(&created.worker_id),
        &name,
    )
    .unwrap();
    let saved = metadata.read_by_name(&name).unwrap().unwrap();
    assert!(matches!(
        saved.subjektiv_session_attribution,
        Some(session_store::SubjektivSessionAttributionState::Confirmed { .. })
    ));
    let source = RuntimeWorkerRef::new(created.runtime_id, created.worker_id);
    fixture
        .connected
        .factory
        .retained_session_snapshot(&worker_runtime::identity::WorkerRef::try_from(&source).unwrap())
        .expect("real factory retained the permitted Worker Session");
}
