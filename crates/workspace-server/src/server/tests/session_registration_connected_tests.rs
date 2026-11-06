//! Connected startup acceptance: real Subject policy -> signed Session POST ->
//! ProfileRuntimeWorkerFactory -> Runtime HTTP -> Backend, including the real
//! authenticated Browser worker-create HTTP route and saved Worker metadata.
//! Only the unused external model is replaced. Read-only reconciliation is not
//! restore, operation replay, or a Browser rendering contract.
use super::*;
use api_macros::HttpError as _;
use protocol::session_failure::{
    SessionPrincipal, SessionRegistrationCause, session_registration_attachment,
};
use session_store::WorkerMetadataStore as _;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use tracing::instrument::WithSubscriber as _;
use tracing_subscriber::{Layer as _, layer::SubscriberExt as _};
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

#[derive(Clone, Default)]
struct StartupEvents(Arc<Mutex<Vec<BTreeMap<String, String>>>>);

#[derive(Default)]
struct StartupEventFields(BTreeMap<String, String>);

impl tracing::field::Visit for StartupEventFields {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.0.insert(field.name().into(), value.into());
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.0.insert(field.name().into(), format!("{value:?}"));
    }
}

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for StartupEvents {
    fn on_event(&self, event: &tracing::Event<'_>, _: tracing_subscriber::layer::Context<'_, S>) {
        if event.metadata().target() == "yoi::worker_startup" {
            let mut fields = StartupEventFields::default();
            event.record(&mut fields);
            self.0.lock().unwrap().push(fields.0);
        }
    }
}

/// A real formatter writer failure, not a replacement result/observer function.
/// The request-scoped subscriber avoids process-global test interference.
#[derive(Clone, Default)]
struct FailedStartupSink(Arc<AtomicUsize>);

impl std::io::Write for FailedStartupSink {
    fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(std::io::Error::other(
            "test: startup observation sink unavailable",
        ))
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for FailedStartupSink {
    type Writer = Self;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

type ReleaseFence = (
    crate::store::WorkerSingletonLease,
    subjektiv::journal::JournalWriterBinding,
);

struct RegistrationFixture {
    connected: ConnectedWorkerHarness,
    subject: String,
    callbacks: Arc<Mutex<Vec<(StatusCode, Value)>>>,
    model: UnusedModel,
    release_fence: Arc<Mutex<Option<ReleaseFence>>>,
    startup_events: StartupEvents,
    failed_sink: FailedStartupSink,
}

#[derive(Clone, Copy)]
enum RegistrationFault {
    Allow,
    Deny,
    DenyWithMetadataReadFailure,
    ReleasePending,
    CommittedResponseLost,
}

impl RegistrationFixture {
    async fn new(fault: RegistrationFault) -> Self {
        // Armed only after real Subject creation with its normal default policy.
        // The callback is a deterministic synchronization point, not a fake Host
        // rejection: the original signed request and real handler remain intact.
        let revoke = Arc::new(Mutex::new(None::<(WorkspaceApi, String, PathBuf)>));
        let hook = revoke.clone();
        let callbacks = Arc::new(Mutex::new(Vec::new()));
        let observed = callbacks.clone();
        let model = UnusedModel::default();
        let release_fence = Arc::new(Mutex::new(None));
        let reserved = release_fence.clone();
        let startup_events = StartupEvents::default();
        let failed_sink = FailedStartupSink::default();
        let events = startup_events.clone();
        let sink = failed_sink.clone();
        let connected = ConnectedWorkerHarness::start_with_layer(model.clone(), move |router| {
            router.layer(axum::middleware::from_fn(
                move |request: axum::extract::Request, next: axum::middleware::Next| {
                    let hook = hook.clone();
                    let observed = observed.clone();
                    let reserved = reserved.clone();
                    let events = events.clone();
                    let sink = sink.clone();
                    async move {
                        if request.method() == axum::http::Method::POST
                            && request.uri().path() == format!("/api/w/{TEST_WORKSPACE_ID}/workers")
                        {
                            let subscriber = tracing_subscriber::registry()
                                .with(events)
                                .with(tracing_subscriber::fmt::layer()
                                    .with_writer(sink)
                                    .with_filter(tracing_subscriber::filter::filter_fn(|metadata| {
                                        metadata.target() == "yoi::worker_startup"
                                    })));
                            return next.run(request).with_subscriber(subscriber).await;
                        }
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
                        let metadata_fault_root = pin_root.clone().filter(|_| {
                            matches!(fault, RegistrationFault::DenyWithMetadataReadFailure)
                        });
                        if let Some((api, subject, _)) = armed {
                            let store = open_subjektiv_store(&api).unwrap();
                            let policy = store.subject_permissions(&subject).unwrap().unwrap();
                            assert!(
                                policy
                                    .execution
                                    .iter()
                                    .any(|p| p.operation == "session.record")
                            );
                            if matches!(fault, RegistrationFault::ReleasePending) {
                                let key = crate::subjektiv::subject_worker_singleton_key(&subject).unwrap();
                                let lease = api.store
                                    .current_worker_singleton_owner(TEST_WORKSPACE_ID, &key)
                                    .unwrap().unwrap();
                                let writer = store.claim_journal_writer(
                                    &subject, &lease.worker.runtime_id, &lease.worker.worker_id,
                                ).unwrap();
                                api.config_store.reserve_subject_release(
                                    TEST_WORKSPACE_ID, &lease, &writer.binding_id,
                                    "registration-release-fence",
                                ).unwrap();
                                assert!(store.journal_writer_active(&writer).unwrap());
                                assert_eq!(api.store.current_worker_singleton_owner(TEST_WORKSPACE_ID, &key).unwrap(), Some(lease.clone()));
                                *reserved.lock().unwrap() = Some((lease, writer));
                            } else if !matches!(fault, RegistrationFault::CommittedResponseLost) {
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
                        if let Some(root) = metadata_fault_root {
                            assert_eq!(response.status(), StatusCode::FORBIDDEN);
                            // The real denial has already been decided. Replace only this
                            // fixture's metadata file with a directory before Worker save:
                            // a real read error, not a fake authorization/store response.
                            let workers = std::fs::read_dir(root.join("workers"))
                                .unwrap().map(|entry| entry.unwrap().path()).collect::<Vec<_>>();
                            assert_eq!(workers.len(), 1);
                            let metadata = workers[0].join("metadata.json");
                            assert!(metadata.is_file());
                            std::fs::remove_file(&metadata).unwrap();
                            std::fs::create_dir(&metadata).unwrap();
                        }
                        observed.lock().unwrap().push((response.status(), input));
                        if matches!(fault, RegistrationFault::CommittedResponseLost) {
                            assert_eq!(response.status(), StatusCode::OK);
                            // The genuine signed handler committed before its response is
                            // lost. Do not replace its authorization, attribution, or write.
                            // A broken body fails the actual Runtime HTTP client rather
                            // than fabricating a typed Runtime/domain failure in this layer.
                            let (parts, _) = response.into_parts();
                            let body = Body::from_stream(futures::stream::once(async {
                                Err::<Vec<u8>, _>(std::io::Error::other(
                                    "test: committed registration response lost",
                                ))
                            }));
                            return Response::from_parts(parts, body);
                        }
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
        if !matches!(fault, RegistrationFault::Allow) {
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
            release_fence,
            startup_events,
            failed_sink,
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

    async fn browser_failure(
        &self,
        expected_status: StatusCode,
    ) -> (
        runtime_api::startup::StartupProblemDetails,
        runtime_api::startup::WorkerCreateFailure,
    ) {
        use runtime_api::startup::{
            DecodeStartupProblem, PROBLEM_CONTENT_TYPE, StartupBoundary, decode_startup_problem,
        };
        let (status, headers, bytes) = self
            .connected
            .human_raw(
                reqwest::Method::POST,
                &format!("/api/w/{TEST_WORKSPACE_ID}/workers"),
                serde_json::to_value(self.request()).unwrap(),
            )
            .await;
        assert_eq!(status, expected_status);
        let content_type = headers[axum::http::header::CONTENT_TYPE].to_str().unwrap();
        assert_eq!(content_type, PROBLEM_CONTENT_TYPE);
        let DecodeStartupProblem::Known(failure) = decode_startup_problem(
            &bytes,
            status.as_u16(),
            content_type,
            StartupBoundary::Browser,
        ) else {
            panic!("real Browser HTTP response must decode as a bounded startup Problem");
        };
        let problem: runtime_api::startup::StartupProblemDetails =
            serde_json::from_slice(&bytes).unwrap();
        assert_eq!(problem.status, status.as_u16());
        // Neither raw callback diagnostics nor authority-bearing identifiers may
        // be copied into a public explanation/extension by the adapters.
        let body = std::str::from_utf8(&bytes).unwrap();
        for secret in [
            &self.connected.owner,
            &self.subject,
            &self.connected.backend_url,
        ] {
            assert!(
                !body.contains(secret),
                "Browser Problem disclosed private fixture data"
            );
        }
        assert!(!body.contains("test: committed registration response lost"));
        assert!(!body.contains("worker-runtime-"));
        // Public, bounded handler bytes, after the non-disclosure checks above.
        // These provide the actual shared Browser fixture source under --nocapture;
        // no metadata, callback body, credential, or private upstream text is logged.
        eprintln!(
            "T812_BROWSER_STARTUP_PROBLEM status={status} content_type={content_type} body={body}"
        );
        (problem, failure)
    }

    fn assert_backend_failure(
        &self,
        worker: &str,
        session: &str,
        problem: &runtime_api::startup::StartupProblemDetails,
        browser: &runtime_api::startup::WorkerCreateFailure,
        expected: SessionRegistrationCause,
    ) {
        use protocol::startup_failure::StartupPersistenceOutcome::Persisted;
        use runtime_api::startup::DecodeStoredWorkerCreateFailure;
        let reference = RuntimeWorkerRef::new(CONNECTED_RUNTIME, worker);
        let Some(DecodeStoredWorkerCreateFailure::Known(saved)) = self
            .connected
            .api
            .worker_create_failure(&reference)
            .unwrap()
        else {
            panic!("real Browser failure must be readable from the canonical Backend reservation");
        };
        assert_startup_semantics(saved.startup(), browser.startup(), expected);
        assert_eq!(
            saved.operation(),
            browser.operation(),
            "HTTP cannot rewrite acceptance/execution/finalization/compensation/correlation"
        );
        assert_eq!(saved.backend_result_persistence(), Some(Persisted));
        assert_eq!(browser.backend_result_persistence(), Some(Persisted));
        assert_eq!(
            problem.yoi_startup.backend_result_persistence,
            Some(Persisted)
        );
        assert_eq!(saved.public_facts(), problem.yoi_startup);
        let correlation = saved
            .operation()
            .correlation
            .as_ref()
            .expect("Backend-selected lookup token");
        assert_eq!(
            correlation.as_str(),
            worker_startup::correlation(CONNECTED_RUNTIME, worker)
        );
        assert_eq!(
            problem.instance,
            format!("urn:yoi:worker-startup:{}", correlation.as_str())
        );

        let events = self.startup_events.0.lock().unwrap();
        let result_events = events
            .iter()
            .filter(|event| {
                event
                    .get("event")
                    .is_some_and(|value| value == "worker_create_result")
            })
            .collect::<Vec<_>>();
        assert_eq!(
            result_events.len(),
            1,
            "one HTTP responsibility boundary records the primary, not every adapter"
        );
        let event = result_events[0];
        assert_eq!(event["domain"], problem.yoi_failure.domain);
        assert_eq!(event["reason"], problem.yoi_failure.reason);
        assert_eq!(event["correlation"], correlation.as_str());
        assert_eq!(event["status"], problem.status.to_string());
        assert_eq!(
            event["acceptance"],
            format!("{:?}", saved.operation().acceptance)
        );
        assert_eq!(
            event["execution"],
            format!("{:?}", saved.operation().execution)
        );
        let logged = serde_json::to_string(&*events).unwrap();
        for secret in [
            self.connected.owner.as_str(),
            self.subject.as_str(),
            self.connected.backend_url.as_str(),
            session,
            worker,
            "test: committed registration response lost",
        ] {
            assert!(
                !logged.contains(secret),
                "startup observation leaked private fixture data"
            );
        }
        assert!(!logged.contains(self.connected.runtime_root.to_str().unwrap()));
        drop(events);
        assert!(
            self.failed_sink.0.load(Ordering::SeqCst) > 0,
            "actual tracing formatter sink failed while HTTP/Backend/metadata results were preserved"
        );
        assert_eq!(
            self.connected
                .api
                .worker_create_failure(&reference)
                .unwrap(),
            Some(DecodeStoredWorkerCreateFailure::Known(saved)),
            "lookup is read-only, never a create retry"
        );
    }

    fn attempted_worker(&self) -> String {
        let workers = std::fs::read_dir(self.connected.runtime_root.join("workers"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            workers.len(),
            1,
            "one real factory attempt, never a new create/replay"
        );
        workers[0].clone()
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

// Runtime/Browser normalization reprojects for the receiving boundary, including
// its local explanation. Compare domain enums, not byte identity of envelopes;
// saved metadata below still checks the original Storage attachment independently.
fn session_primary(
    primary: &protocol::startup_failure::StartupFailureAttachment,
) -> Option<SessionRegistrationCause> {
    match primary.decode::<SessionRegistrationCause>() {
        error_contract::DecodeFailure::Known(received) => Some(*received.facts().cause()),
        _ => None,
    }
}

fn assert_session_primary(
    primary: &protocol::startup_failure::StartupFailureAttachment,
    expected: SessionRegistrationCause,
) {
    assert_eq!(
        session_primary(primary),
        Some(expected),
        "Session failure must remain a known typed domain cause"
    );
}

fn assert_startup_semantics(
    host: &runtime_api::WorkerStartupFailure,
    wire: &runtime_api::WorkerStartupFailure,
    expected: SessionRegistrationCause,
) {
    assert_session_primary(&host.primary, expected);
    assert_session_primary(&wire.primary, expected);
    assert_eq!(host.secondary, wire.secondary);
    assert_eq!(host.persistence, wire.persistence);
    assert_eq!(host.resources, wire.resources);
    assert_eq!(host.io_kind, wire.io_kind);
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
async fn browser_http_session_record_denial_retains_subject_cause_and_saved_resources() {
    use protocol::startup_failure::{
        StartupFailurePhase, StartupPersistenceOutcome, StartupSecondaryFailure,
    };
    use runtime_api::startup::{ExecutionOutcome, StartupCause};

    let fixture = RegistrationFixture::new(RegistrationFault::Deny).await;
    // This POST must pass the real Browser authentication/preflight and reach
    // the signed callback. Only then does the fixture remove session.record.
    let (problem, browser) = fixture.browser_failure(StatusCode::CONFLICT).await;
    let session = fixture.assert_callback(StatusCode::FORBIDDEN);
    assert_eq!(fixture.callbacks.lock().unwrap().len(), 1);
    let expected = SessionRegistrationCause::denied(SessionPrincipal::Subject);
    assert_eq!(problem.type_uri, "urn:yoi:problem:conflict");
    assert_eq!(problem.yoi_failure.domain, "session_registration");
    assert_eq!(problem.yoi_failure.reason, "permission_denied");
    assert_eq!(problem.yoi_failure.details, StartupCause::Session(expected));
    assert!(problem.detail.contains("Subject"), "{}", problem.detail);
    assert!(
        problem.detail.contains("session.record"),
        "{}",
        problem.detail
    );
    assert_eq!(browser.operation().execution, ExecutionOutcome::Errored);
    assert_session_primary(&browser.startup().primary, expected);

    let runtime = fixture.connected.runtime_startup_failures.lock().unwrap();
    assert_eq!(runtime.len(), 1, "one actual Runtime HTTP failure");
    assert_session_primary(&runtime[0].primary, expected);
    assert_eq!(
        browser.startup().persistence,
        Some(StartupPersistenceOutcome::Persisted)
    );
    assert_eq!(browser.startup().persistence, runtime[0].persistence);
    assert!(
        runtime[0].resources.is_some(),
        "resource disposition is independent of the denial"
    );
    assert_eq!(browser.startup().resources, runtime[0].resources);
    assert!(
        browser.startup().secondary.iter().any(|secondary| matches!(
            secondary,
            StartupSecondaryFailure::Persistence {
                phase: StartupFailurePhase::RuntimeRecordDelete,
                ..
            }
        )),
        "real retention pin prevents Runtime record deletion without replacing the denial"
    );
    drop(runtime);

    let store = open_subjektiv_store(&fixture.connected.api).unwrap();
    assert!(store.session_attribution(&session).unwrap().is_none());
    assert!(
        store
            .subject_session_attributions(&fixture.subject)
            .unwrap()
            .is_empty()
    );
    let worker = fixture.attempted_worker();
    fixture.assert_backend_failure(&worker, &session, &problem, &browser, expected);
    let saved = read_metadata(&fixture.connected.runtime_root, &worker);
    assert!(contains_saved_value(
        &saved,
        &serde_json::to_value(session_registration_attachment(expected).unwrap()).unwrap()
    ));
    assert_eq!(
        fixture.connected.runtime.list_workers().unwrap().len(),
        1,
        "retained Runtime resource is observed separately from persisted cause and HTTP409"
    );
    assert_eq!(
        read_metadata(&fixture.connected.runtime_root, &worker),
        saved,
        "ordinary read-only reopen, not create/restore/retry"
    );
    assert_eq!(fixture.callbacks.lock().unwrap().len(), 1);
    assert_eq!(fixture.model.0.load(Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn browser_http_committed_registration_response_loss_stays_unknown_without_reexecution() {
    use protocol::startup_failure::StartupPersistenceOutcome;
    use runtime_api::startup::StartupCause;

    let fixture = RegistrationFixture::new(RegistrationFault::CommittedResponseLost).await;
    let (problem, browser) = fixture.browser_failure(StatusCode::BAD_GATEWAY).await;
    // This is the status of the original signed handler, before transport loss.
    let session = fixture.assert_callback(StatusCode::OK);
    assert_eq!(
        fixture.callbacks.lock().unwrap().len(),
        1,
        "response loss must not automatically reexecute Session registration"
    );
    let expected = SessionRegistrationCause::outcome_unknown();
    assert_eq!(problem.yoi_failure.domain, "session_registration");
    assert_eq!(problem.yoi_failure.reason, "outcome_unknown");
    assert_eq!(problem.yoi_failure.details, StartupCause::Session(expected));
    assert!(
        problem.detail.contains("session.record"),
        "{}",
        problem.detail
    );
    assert_session_primary(&browser.startup().primary, expected);
    assert_eq!(
        browser.startup().persistence,
        Some(StartupPersistenceOutcome::Persisted)
    );
    let runtime = fixture.connected.runtime_startup_failures.lock().unwrap();
    assert_eq!(runtime.len(), 1);
    assert_session_primary(&runtime[0].primary, expected);
    assert_eq!(browser.startup().resources, runtime[0].resources);
    assert_eq!(browser.startup().persistence, runtime[0].persistence);
    drop(runtime);

    let store = open_subjektiv_store(&fixture.connected.api).unwrap();
    let committed = store.session_attribution(&session).unwrap().unwrap();
    assert_eq!(committed.subject_id, fixture.subject);
    assert_eq!(committed.runtime_id, CONNECTED_RUNTIME);
    let worker = fixture.attempted_worker();
    fixture.assert_backend_failure(&worker, &session, &problem, &browser, expected);
    assert_eq!(committed.worker_id, worker);
    assert_eq!(
        store
            .subject_session_attributions(&fixture.subject)
            .unwrap()
            .len(),
        1,
        "HTTP failure is not proof of no Backend write"
    );
    let saved = read_metadata(&fixture.connected.runtime_root, &worker);
    assert_eq!(
        saved["subjektiv_session_attribution"]["state"], "outcome_unknown",
        "saved Runtime observation must not claim confirmed registration or rejection"
    );
    assert_eq!(
        saved["subjektiv_session_attribution"]["session_id"],
        session
    );
    assert!(contains_saved_value(
        &saved,
        &serde_json::to_value(session_registration_attachment(expected).unwrap()).unwrap()
    ));
    assert_eq!(
        read_metadata(&fixture.connected.runtime_root, &worker),
        saved
    );
    assert_eq!(
        store.session_attribution(&session).unwrap(),
        Some(committed)
    );
    assert_eq!(fixture.connected.runtime.list_workers().unwrap().len(), 1);
    assert_eq!(fixture.attempted_worker(), worker);
    assert_eq!(
        fixture.callbacks.lock().unwrap().len(),
        1,
        "Problem decoding, attribution lookup and metadata reopen are read-only, never replay authority"
    );
    assert_eq!(fixture.model.0.load(Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn real_session_record_denial_survives_runtime_host_and_metadata_reopen() {
    let fixture = RegistrationFixture::new(RegistrationFault::Deny).await;
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
    assert_session_primary(
        &wire.primary,
        SessionRegistrationCause::denied(SessionPrincipal::Subject),
    );
    assert_eq!(
        wire.persistence,
        Some(protocol::startup_failure::StartupPersistenceOutcome::Persisted)
    );
    let host = error
        .diagnostics
        .iter()
        .filter_map(|d| d.startup.as_ref())
        .find(|startup| {
            session_primary(&startup.primary)
                == Some(SessionRegistrationCause::denied(SessionPrincipal::Subject))
        })
        .expect("RemoteWorkerRuntime must preserve the typed startup primary in Host diagnostics");
    assert_startup_semantics(
        host,
        wire,
        SessionRegistrationCause::denied(SessionPrincipal::Subject),
    );
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
    let expected = serde_json::to_value(&expected_primary).unwrap();
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
async fn real_session_record_denial_and_metadata_save_failure_remain_separate() {
    use protocol::startup_failure::{
        StartupFailurePhase, StartupPersistenceOutcome, StartupSecondaryFailure,
    };
    let fixture = RegistrationFixture::new(RegistrationFault::DenyWithMetadataReadFailure).await;
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
    let failures = fixture.connected.runtime_startup_failures.lock().unwrap();
    assert_eq!(
        failures.len(),
        1,
        "one real startup attempt, not an operation replay"
    );
    let wire = &failures[0];
    assert_session_primary(
        &wire.primary,
        SessionRegistrationCause::denied(SessionPrincipal::Subject),
    );
    assert_eq!(wire.persistence, Some(StartupPersistenceOutcome::NotSaved));
    assert!(wire.secondary.iter().any(|failure| matches!(
        failure,
        StartupSecondaryFailure::Persistence {
            phase: StartupFailurePhase::MetadataSave,
            outcome: StartupPersistenceOutcome::NotSaved
        }
    )));
    let host = error
        .diagnostics
        .iter()
        .filter_map(|d| d.startup.as_ref())
        .find(|startup| {
            session_primary(&startup.primary)
                == Some(SessionRegistrationCause::denied(SessionPrincipal::Subject))
        })
        .unwrap();
    assert_startup_semantics(
        host,
        wire,
        SessionRegistrationCause::denied(SessionPrincipal::Subject),
    );
    assert!(
        open_subjektiv_store(&fixture.connected.api)
            .unwrap()
            .subject_session_attributions(&fixture.subject)
            .unwrap()
            .is_empty()
    );
    let workers = std::fs::read_dir(fixture.connected.runtime_root.join("workers"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    assert_eq!(workers.len(), 1);
    let name = format!(
        "worker-runtime-{}",
        workers[0].file_name().unwrap().to_str().unwrap()
    );
    let reopened = session_store::WorkerAggregateStore::open_read_only(&workers[0], &name).unwrap();
    assert!(
        reopened.read_by_name(&name).is_err(),
        "the real read failure remains, not a false saved cause"
    );
}

#[tokio::test]
async fn pending_release_binding_refusal_retains_original_conflict_source() {
    let f = Fixture::new().await;
    let store = open_subjektiv_store(&f.api).unwrap();
    let lease = f
        .api
        .store
        .require_current_worker_singleton_owner(TEST_WORKSPACE_ID, &f.source)
        .unwrap()
        .unwrap();
    let writer = store
        .claim_journal_writer(&f.subject, &f.source.runtime_id, &f.source.worker_id)
        .unwrap();
    f.api
        .config_store
        .reserve_subject_release(
            TEST_WORKSPACE_ID,
            &lease,
            &writer.binding_id,
            "registration-source-release",
        )
        .unwrap();
    let session = session_store::new_session_id().to_string();
    let attr = crate::subjektiv::SubjectSessionAttribution::new(
        &f.subject,
        &f.source.runtime_id,
        &f.source.worker_id,
        &session,
    )
    .unwrap();
    let error = session_registration::bind_host_session(&f.api, attr).unwrap_err();
    assert_eq!(
        error.session_registration_cause,
        Some(SessionRegistrationCause::binding_mismatch())
    );
    assert!(matches!(
        std::error::Error::source(&error)
            .unwrap()
            .downcast_ref::<Error>(),
        Some(Error::RepositoryConflict(_))
    ));
    assert_eq!(
        error.into_repository_api_error().status_code(),
        StatusCode::CONFLICT.as_u16()
    );
    assert!(store.session_attribution(&session).unwrap().is_none());
    // A conflict without this explicit decision must not acquire a denial.
    let unrelated = session_registration::Judgment::SessionRecord
        .binding_rejection(Error::RepositoryConflict("an unrelated conflict".into()).into());
    assert!(unrelated.session_registration_cause.is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn release_fence_refusal_survives_signed_registration_and_metadata_reopen() {
    let fixture = RegistrationFixture::new(RegistrationFault::ReleasePending).await;
    let api = &fixture.connected.api;
    let store = open_subjektiv_store(api).unwrap();
    let policy = store
        .subject_permissions(&fixture.subject)
        .unwrap()
        .unwrap();
    let error = scoped_create_workspace_worker(
        State(api.clone()),
        AxumPath(ScopedWorkspacePath {
            workspace_id: TEST_WORKSPACE_ID.into(),
        }),
        HeaderMap::new(),
        Json(fixture.request()),
    )
    .await
    .unwrap_err();
    let session = fixture.assert_callback(StatusCode::CONFLICT);
    let (lease, writer) = fixture.release_fence.lock().unwrap().clone().unwrap();
    assert_eq!(
        store.subject_permissions(&fixture.subject).unwrap(),
        Some(policy)
    );
    assert!(store.session_attribution(&session).unwrap().is_none());
    assert!(store.journal_writer_active(&writer).unwrap());
    api.config_store
        .require_subject_release_intent(
            TEST_WORKSPACE_ID,
            &lease,
            &writer.binding_id,
            "registration-release-fence",
        )
        .unwrap();
    assert!(
        api.config_store
            .subject_release_pending(TEST_WORKSPACE_ID, &lease.worker)
            .unwrap()
    );
    assert!(
        !api.config_store
            .subject_release_proven(
                TEST_WORKSPACE_ID,
                &lease,
                &writer.binding_id,
                "registration-release-fence"
            )
            .unwrap()
    );

    let failures = fixture.connected.runtime_startup_failures.lock().unwrap();
    assert_eq!(failures.len(), 1, "one real factory startup, not replay");
    let wire = &failures[0];
    let expected =
        session_registration_attachment(SessionRegistrationCause::binding_mismatch()).unwrap();
    assert_session_primary(&wire.primary, SessionRegistrationCause::binding_mismatch());
    assert_eq!(
        wire.persistence,
        Some(protocol::startup_failure::StartupPersistenceOutcome::Persisted)
    );
    let host = error
        .diagnostics
        .iter()
        .filter_map(|d| d.startup.as_ref())
        .find(|failure| {
            session_primary(&failure.primary) == Some(SessionRegistrationCause::binding_mismatch())
        })
        .expect("Remote Host retains known refusal");
    assert_startup_semantics(host, wire, SessionRegistrationCause::binding_mismatch());
    assert!(
        host.secondary.iter().any(|failure| matches!(
            failure,
            protocol::startup_failure::StartupSecondaryFailure::Persistence {
                phase: protocol::startup_failure::StartupFailurePhase::RuntimeRecordDelete,
                ..
            }
        )),
        "retention cleanup failure is separate from refusal"
    );
    let saved = read_metadata(&fixture.connected.runtime_root, &lease.worker.worker_id);
    assert!(contains_saved_value(
        &saved,
        &serde_json::to_value(&expected).unwrap()
    ));
    assert_eq!(
        read_metadata(&fixture.connected.runtime_root, &lease.worker.worker_id),
        saved
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn permitted_session_registration_reaches_real_factory_and_saved_confirmation() {
    let fixture = RegistrationFixture::new(RegistrationFault::Allow).await;
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
