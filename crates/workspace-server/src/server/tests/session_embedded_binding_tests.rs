//! Original signed source -> production Session handler -> real Embedded Runtime
//! public reader -> persisted Controller history -> coherent final return fence.
//! Only the external model stream is scripted; its Run completes before the read.
//! The timing hook holds AFTER domain/Runtime response construction, not before
//! supply. List is the real Host attribution/observation DTO path, not Runtime I/O.
//! Signature/body/JTI verification is real; its exact result is injected into the
//! handler context. This does not claim Session-history router/middleware coverage.
//! Normal registered lib tests, isolated SQLite/loopback resources, bounded event
//! waits and RAII release/abort on failure; no subprocess or sleep-based readiness.
use super::*;
use server_api::{
    SubjektivSessionBackendOperation as Op, SubjektivSessionBackendRequest as SessionRequest,
    SubjektivSessionBackendResponse as SessionResponse,
    SubjektivSessionBackendResult as SessionResult,
};
use std::{future::Future, sync::mpsc, time::Duration};
use worker_runtime::auth::{RuntimeIdentityMaterial, RuntimeRequestSourceSigner};

const WAIT: Duration = Duration::from_secs(15);
const SECRET: &str = "LOCAL_SIGNED_SESSION_NEEDLE original persisted user input";

#[derive(Clone, Copy, Debug)]
enum Change {
    Unchanged,
    Rotate,
    ReactivateSameKey,
    AbaSameTrust,
}

#[track_caller]
fn bounded<T>(future: impl Future<Output = T>) -> impl Future<Output = T> {
    let caller = std::panic::Location::caller();
    async move {
        eprintln!("signed Embedded Session waiting at {caller}");
        let value = tokio::time::timeout(WAIT, future)
            .await
            .unwrap_or_else(|_| {
                panic!("signed Embedded Session event/request deadline at {caller}")
            });
        eprintln!("signed Embedded Session completed at {caller}");
        value
    }
}

struct HeldReturn {
    release: Option<mpsc::Sender<()>>,
    task: tokio::task::JoinHandle<ApiResult<Json<SessionResponse>>>,
}
impl HeldReturn {
    fn release(&mut self) {
        self.release.take().unwrap().send(()).unwrap();
    }
}
impl Drop for HeldReturn {
    fn drop(&mut self) {
        // Also release the synchronous Host callback on timeout/assertion unwind.
        if let Some(release) = self.release.take() {
            let _ = release.send(());
        }
        self.task.abort();
    }
}

#[derive(Clone, Default)]
struct CompletedSessionModel {
    calls: Arc<std::sync::atomic::AtomicUsize>,
    jobs: ConnectedJournalModel,
}

#[async_trait::async_trait]
impl LlmClient for CompletedSessionModel {
    fn clone_boxed(&self) -> Box<dyn LlmClient> {
        Box::new(self.clone())
    }

    async fn stream(
        &self,
        request: Request,
    ) -> std::result::Result<agen::llm_client::client::ResponseStream, ClientError> {
        if request.tools.iter().any(|tool| {
            matches!(
                tool.name.as_str(),
                "ReadJournalState" | "SubmitBackendJobResult" | "SubmitMemorySurface"
            )
        }) {
            return self.jobs.stream(request).await;
        }
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(Box::pin(futures::stream::iter([
            Ok(Event::text_block_start(0)),
            Ok(Event::text_delta(0, "completed Local Session fixture")),
            Ok(Event::text_block_stop(0, Some(StopReason::EndTurn))),
        ])))
    }
}

struct SignedEmbeddedSession {
    // Abort the server/fixture Controllers before releasing API/temporary roots.
    _server: ConnectedRestoreServer,
    api: WorkspaceApi,
    identity: RuntimeIdentityMaterial,
    subject: String,
    worker: RuntimeWorkerRef,
    session: String,
    segment: session_store::SegmentId,
    session_id: session_store::SessionId,
    _dir: tempfile::TempDir,
}

impl SignedEmbeddedSession {
    async fn new() -> Self {
        use session_store::WorkerMetadataStore as _;

        let dir = connected_restore_tempdir();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut config = test_server_config(dir.path());
        config.repositories.clear();
        config.backend_base_url = Some(format!("http://{}", listener.local_addr().unwrap()));
        let control = Arc::new(test_control_store(&config));
        seed_test_registered_workspace(control.as_ref(), &config)
            .await
            .unwrap();
        let broker = BackendResourceBroker::default();
        let dispatcher = Arc::new(
            crate::worker_source::EmbeddedServerWorkerMutationDispatcher::new(
                config.clone(),
                control.clone(),
            ),
        );
        let probe = Arc::new(RestoreAuthProbe::default());
        let model = CompletedSessionModel::default();
        let model_factory = model.clone();
        let audience = embedded_runtime_request_audience(&config).unwrap();
        let factory = worker_runtime::worker_backend::ProfileRuntimeWorkerFactory::new(
            config.workspace_execution_root.clone(),
        )
        .with_embedded_worker_mutation_dispatcher(EMBEDDED_WORKER_RUNTIME_ID, dispatcher.clone())
        .with_workspace_request_client(
            worker_runtime::workspace_request::RuntimeWorkspaceRequestClient::new(
                TEST_WORKSPACE_ID,
                audience.clone(),
                EMBEDDED_WORKER_RUNTIME_ID,
            )
            .with_runtime_request_source(&EMBEDDED_RUNTIME_REQUEST_IDENTITY, audience),
        )
        .with_runtime_store_dir(config.embedded_runtime_store_root.clone())
        .with_resource_client(Arc::new(broker.clone()))
        .with_spawn_model_client_factory(move || Box::new(model_factory.clone()));
        // This wrapper delegates retained snapshot/history/attachment reads to
        // ProfileRuntimeWorkerFactory. It never implements SessionPublic replies.
        let execution = worker_runtime::worker_backend::WorkerRuntimeExecutionBackend::new(
            RestoreAuthFactory {
                inner: factory,
                probe: probe.clone(),
            },
        )
        .unwrap();
        let api = WorkspaceApi::new_with_execution_backend_and_broker(
            config,
            control,
            Arc::new(execution),
            broker,
            HostExecutors::default(),
            Some(dispatcher),
        )
        .await
        .unwrap();
        seed_test_delivery_binding(&api);
        let identity = (*EMBEDDED_RUNTIME_REQUEST_IDENTITY).clone();
        // Workspace setup may already have inserted a synthetic trust key. Install
        // the real factory signer ONCE, before both Host startup and B1 verification.
        let mut binding = api
            .config_store
            .get_workspace_runtime_binding(TEST_WORKSPACE_ID, EMBEDDED_WORKER_RUNTIME_ID)
            .unwrap()
            .unwrap();
        binding.public_key = identity.public_key.clone();
        binding.public_key_fingerprint.clear();
        binding.authentication_mode =
            crate::store::WorkspaceRuntimeAuthenticationMode::WorkspaceIdentity;
        api.config_store
            .upsert_workspace_runtime_binding(binding, true)
            .unwrap();

        let store = open_subjektiv_store(&api).unwrap();
        let subject = store
            .create_subject(crate::subjektiv::SubjectRole::new("signed-embedded-session").unwrap())
            .unwrap();
        let mut execution = connected_subject_permissions(&subject.id);
        execution.extend(
            ["session.list", "session.search", "session.read"].map(|operation| {
                server_api::authorization::ScopedPermission {
                    operation: operation.into(),
                    resource: server_api::authorization::PermissionResource::Subject {
                        workspace_id: TEST_WORKSPACE_ID.into(),
                        subject_id: subject.id.clone(),
                    },
                }
            }),
        );
        store
            .set_subject_permissions(
                &subject.id,
                &server_api::authorization::SubjectPermissionsUpdate {
                    operation_id: "signed-embedded-session-grants".into(),
                    expected_change_id: store
                        .current_subject_policy(&subject.id)
                        .unwrap()
                        .map(|policy| policy.change_id),
                    execution,
                    delegation: vec![],
                },
            )
            .unwrap();
        let app = build_router(api.clone());
        let server = ConnectedRestoreServer {
            server: tokio::spawn(async move {
                axum::serve(listener, app).await.unwrap();
            }),
            probe: probe.clone(),
        };
        // Real connected creation performs signed Session registration. No fake
        // Runtime endpoint, synthetic attribution, or seeded protected DTO is used.
        let Json(created) = scoped_create_workspace_worker(
            State(api.clone()),
            AxumPath(ScopedWorkspacePath {
                workspace_id: TEST_WORKSPACE_ID.into(),
            }),
            HeaderMap::new(),
            Json(CreateWorkspaceWorkerRequest {
                runtime_id: EMBEDDED_WORKER_RUNTIME_ID.into(),
                display_name: "signed-embedded-session".into(),
                singleton_key: None,
                profile: Some("builtin:companion".into()),
                ticket_assignment: None,
                initial_submit: vec![],
                workdir_attachments: vec![],
                feature_connections: server_api::WorkspaceWorkerFeatureConnectionsRequest {
                    subjektiv: Some(server_api::SubjektivWorkerConnectionRequest {
                        subject_id: subject.id.clone(),
                    }),
                },
                control_operation_id: None,
                execution_grant: None,
            }),
        )
        .await
        .unwrap();
        let worker = RuntimeWorkerRef::new(created.runtime_id, created.worker_id);
        assert_eq!(worker.runtime_id, EMBEDDED_WORKER_RUNTIME_ID);
        let handle = probe.handles.lock().unwrap()[0].clone();
        let mut events = handle.subscribe();
        handle
            .send(protocol::Method::submit_text("signed-session-seed", SECRET))
            .await
            .unwrap();
        restore_auth_event(&mut events, |event| {
            matches!(event, protocol::Event::RunEnd { .. })
        })
        .await;
        assert_eq!(model.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        // Public Session projection exposes committed Runs, not a pending Invoke
        // that merely persisted its user input. RunEnd follows that durable boundary.
        let aggregate = api
            .config
            .embedded_runtime_store_root
            .join("workers")
            .join(&worker.worker_id);
        let name = format!("worker-runtime-{}", worker.worker_id);
        let metadata =
            session_store::WorkerAggregateStore::open_read_only(&aggregate, &name).unwrap();
        let saved = metadata.read_by_name(&name).unwrap().unwrap();
        let active = saved.active.as_ref().unwrap();
        let session_id = active.session_id;
        let segment = active.segment_id.unwrap();
        let session = session_id.to_string();
        let attribution = store
            .session_attribution(&session)
            .unwrap()
            .expect("real signed Host registration");
        assert_eq!(attribution.subject_id, subject.id);
        assert_eq!(attribution.worker_id, worker.worker_id);
        if api
            .store
            .get_active_worker_control_grant(TEST_WORKSPACE_ID, &worker, &worker)
            .unwrap()
            .is_none()
        {
            api.store
                .create_worker_control_grant(&WorkerControlGrantRecord {
                    workspace_id: TEST_WORKSPACE_ID.into(),
                    grant_id: "signed-session-observe".into(),
                    controller: worker.clone(),
                    subject: worker.clone(),
                    relation: "subject_session".into(),
                    origin: "test".into(),
                    permissions: vec!["observe".into()],
                    operation_id: "signed-session-observe".into(),
                    created_at: now_registry_timestamp(),
                    revoked_at: None,
                })
                .unwrap();
        }
        let fixture = Self {
            _server: server,
            api,
            identity,
            subject: subject.id,
            worker,
            session,
            segment,
            session_id,
            _dir: dir,
        };
        assert!(
            fixture.history_snapshot().to_string().contains(SECRET),
            "real persisted user history"
        );
        fixture
    }

    fn binding(&self) -> crate::store::WorkspaceRuntimeBinding {
        self.api
            .config_store
            .get_workspace_runtime_binding(TEST_WORKSPACE_ID, &self.worker.runtime_id)
            .unwrap()
            .unwrap()
    }

    fn history_snapshot(&self) -> Value {
        let root = self
            .api
            .config
            .embedded_runtime_store_root
            .join("workers")
            .join(&self.worker.worker_id)
            .join("session");
        let store = session_store::WorkerSessionStore::open_read_only(root).unwrap();
        serde_json::to_value(
            store
                .read_all_read_only(self.session_id, self.segment)
                .unwrap(),
        )
        .unwrap()
    }

    fn domain_snapshot(&self) -> Value {
        let store = open_subjektiv_store(&self.api).unwrap();
        json!({
            "history": self.history_snapshot(),
            "policy": store.current_subject_policy(&self.subject).unwrap(),
            "attribution": store.subject_session_attributions(&self.subject).unwrap(),
            "worker": self.api.store.get_worker_registry(TEST_WORKSPACE_ID, &self.worker).unwrap(),
            "owner": self.api.store.require_current_worker_singleton_owner(TEST_WORKSPACE_ID, &self.worker).unwrap(),
            "observe": self.api.store.get_active_worker_control_grant(TEST_WORKSPACE_ID, &self.worker, &self.worker).unwrap(),
            "archive": self.api.config_store.worker_session_archive(TEST_WORKSPACE_ID, &self.worker, &self.session).unwrap(),
        })
    }

    async fn verified_context(&self, bytes: &[u8]) -> server_api::ServerRequestContext {
        let path = format!("/api/w/{TEST_WORKSPACE_ID}/subjektiv/session-history");
        let proof = RuntimeRequestSourceSigner::from_identity(&self.identity)
            .issue(
                self.api.config.backend_base_url.as_deref().unwrap(),
                TEST_WORKSPACE_ID,
                Some(&self.worker.worker_id),
                worker_runtime::auth::WORKSPACE_REQUEST_PERMISSION,
                "POST",
                &path,
                bytes,
                i64::try_from(worker_runtime::auth::unix_now_seconds()).unwrap(),
                30,
            )
            .unwrap();
        let verified = bounded(crate::worker_source::verify_runtime_request_source_proof(
            &self.api,
            &proof,
            TEST_WORKSPACE_ID,
            worker_runtime::auth::WORKSPACE_REQUEST_PERMISSION,
            "POST",
            &path,
            &worker_runtime::auth::request_body_digest(bytes),
        ))
        .await
        .unwrap();
        assert_eq!(verified.runtime_id, self.worker.runtime_id);
        assert_eq!(
            verified.worker_id.as_deref(),
            Some(self.worker.worker_id.as_str())
        );
        server_api::ServerRequestContext {
            actor: None,
            worker_source: None,
            runtime_source: Some(server_api::ServerRuntimeSource {
                runtime_id: verified.runtime_id,
                worker_id: verified.worker_id,
                // Retain the ORIGINAL verifier result, never a binding lookup.
                authentication_binding_id: verified.authentication_binding_id,
            }),
            origin: None,
            transport_headers: vec![],
        }
    }

    async fn current(&self, bytes: &[u8]) -> SessionResponse {
        let context = self.verified_context(bytes).await;
        bounded(subjektiv_session_backend_before_return(
            self.api.clone(),
            ScopedWorkspacePath {
                workspace_id: TEST_WORKSPACE_ID.into(),
            },
            context,
            serde_json::from_slice::<SessionRequest>(bytes).unwrap(),
            || {},
        ))
        .await
        .unwrap()
        .0
    }

    fn replace_trust(&mut self, change: Change) {
        if matches!(change, Change::Unchanged) {
            return;
        }
        let original = self.binding();
        let mut replacement = original.clone();
        match change {
            Change::Unchanged => unreachable!(),
            Change::Rotate => {
                self.identity = RuntimeIdentityMaterial::generate(&self.worker.runtime_id).unwrap();
                replacement.public_key = self.identity.public_key.clone();
                replacement.public_key_fingerprint.clear();
            }
            Change::ReactivateSameKey => {
                assert!(
                    self.api
                        .config_store
                        .revoke_workspace_runtime_binding(
                            TEST_WORKSPACE_ID,
                            &self.worker.runtime_id,
                            &now_registry_timestamp(),
                        )
                        .unwrap()
                );
                assert!(self.binding().revoked_at.is_some());
            }
            Change::AbaSameTrust => {
                let temporary = RuntimeIdentityMaterial::generate(&self.worker.runtime_id).unwrap();
                replacement.public_key = temporary.public_key;
                replacement.public_key_fingerprint.clear();
                self.api
                    .config_store
                    .upsert_workspace_runtime_binding(replacement, true)
                    .unwrap();
                assert_ne!(self.binding().binding_id, original.binding_id);
                assert_ne!(self.binding().public_key, original.public_key);
                replacement = original.clone();
            }
        }
        self.api
            .config_store
            .upsert_workspace_runtime_binding(replacement, true)
            .unwrap();
        let current = self.binding();
        assert_ne!(current.binding_id, original.binding_id);
        assert!(current.revoked_at.is_none());
        assert_eq!(current.public_key, self.identity.public_key);
        if !matches!(change, Change::Rotate) {
            assert_eq!(current.public_key, original.public_key);
            assert_eq!(
                current.public_key_fingerprint,
                original.public_key_fingerprint
            );
        }
        assert_eq!(current.base_url, original.base_url);
        assert_eq!(current.display_name, original.display_name);
        assert_eq!(current.authentication_mode, original.authentication_mode);
    }
}

fn assert_positive(kind: &str, session: &str, response: SessionResponse) {
    let SessionResponse::Ok { result } = response else {
        panic!("{kind}: {response:?}")
    };
    match (kind, result) {
        ("read", SessionResult::Read(page)) => {
            assert_eq!(page.session_id, session);
            assert!(page.content.contains(SECRET), "{}", page.content);
            assert!(!page.truncated);
        }
        ("search", SessionResult::Search(page)) => {
            assert!(page.issues.is_empty(), "{:?}", page.issues);
            assert_eq!(page.items.len(), 1);
            assert_eq!(page.items[0].session_id, session);
            assert!(page.items[0].snippet.contains(SECRET));
            assert!(!page.items[0].snippet_truncated);
        }
        ("list", SessionResult::List(page)) => {
            assert_eq!(page.items.len(), 1);
            assert_eq!(page.items[0].session_id, session);
            assert_eq!(
                page.items[0].storage,
                server_api::SubjektivSessionStorage::Retained
            );
            assert_eq!(
                page.items[0].availability,
                server_api::SubjektivSessionAvailability::Unchecked
            );
        }
        (_, result) => panic!("unexpected {kind} result: {result:?}"),
    }
}

async fn session_case(change: Change) {
    let mut f = bounded(SignedEmbeddedSession::new()).await;
    let before = f.domain_snapshot();
    let search = serde_json::to_vec(&SessionRequest {
        operation: Op::Search(
            serde_json::from_value(json!({"session_id":f.session,"query":SECRET})).unwrap(),
        ),
    })
    .unwrap();
    // Discover the actual public selector from real persisted Controller history.
    // A diagnostic, empty fake page, or unsupported SessionPublic cannot pass.
    let discovered = f.current(&search).await;
    assert_positive("search", &f.session, discovered.clone());
    let SessionResponse::Ok {
        result: SessionResult::Search(page),
    } = discovered
    else {
        unreachable!()
    };
    let selected = &page.items[0];
    let read = serde_json::to_vec(&SessionRequest {
        operation: Op::Read(
            serde_json::from_value(json!({
                "session_id": f.session, "segment_id":selected.segment_id,
                "entry_ref":selected.entry_ref,"mode":"compact",
            }))
            .unwrap(),
        ),
    })
    .unwrap();
    let list = serde_json::to_vec(&SessionRequest {
        operation: Op::List(serde_json::from_value(json!({"session_id":f.session})).unwrap()),
    })
    .unwrap();
    for (kind, bytes) in [("read", read), ("search", search), ("list", list)] {
        assert_positive(kind, &f.session, f.current(&bytes).await);
        let context = f.verified_context(&bytes).await;
        let original = context
            .runtime_source
            .as_ref()
            .unwrap()
            .authentication_binding_id
            .clone();
        assert_eq!(original, f.binding().binding_id);
        let (entered, waiting) = tokio::sync::oneshot::channel();
        let (release, resume) = mpsc::channel();
        let api = f.api.clone();
        let request: SessionRequest = serde_json::from_slice(&bytes).unwrap();
        let mut held = HeldReturn {
            release: Some(release),
            task: tokio::spawn(async move {
                subjektiv_session_backend_before_return(
                    api,
                    ScopedWorkspacePath {
                        workspace_id: TEST_WORKSPACE_ID.into(),
                    },
                    context,
                    request,
                    move || {
                        let _ = entered.send(());
                        resume
                            .recv_timeout(WAIT)
                            .expect("release post-read Embedded Session return");
                    },
                )
                .await
            }),
        };
        bounded(waiting).await.unwrap();
        assert!(
            !held.task.is_finished(),
            "{kind} must be held after domain read"
        );
        assert_eq!(f.binding().binding_id, original);
        f.replace_trust(change);
        assert_eq!(
            f.domain_snapshot(),
            before,
            "rotation changes only Runtime trust"
        );
        held.release();
        let result = bounded(&mut held.task).await.unwrap();
        if matches!(change, Change::Unchanged) {
            assert_positive(kind, &f.session, result.unwrap().0);
        } else {
            let error = result.unwrap_err();
            assert!(
                matches!(&error.error, Error::WorkspacePermissionDenied(_)),
                "{kind}/{change:?}: {error:?}"
            );
            assert!(
                error
                    .error
                    .to_string()
                    .contains("authentication binding changed"),
                "{error:?}"
            );
            let response = error.into_response();
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
            let body = bounded(to_bytes(response.into_body(), 64 * 1024))
                .await
                .unwrap();
            let text = std::str::from_utf8(&body).unwrap();
            assert!(
                !text.contains(SECRET),
                "stale request leaked content/snippet"
            );
            assert!(
                !text.contains(&f.session),
                "stale request leaked Session list identity"
            );
            assert!(
                serde_json::from_slice::<SessionResponse>(&body).is_err(),
                "stale request returned a protected DTO: {text}"
            );
        }
        assert_eq!(
            f.domain_snapshot(),
            before,
            "denial/return must not rewrite persisted Session or grants"
        );
        // SAME serialized bytes, Worker, Session and public selector. Only proof
        // JTI/signature is new, genuinely issued under current B2 after a change.
        let current_binding = f.binding().binding_id;
        assert_positive(kind, &f.session, f.current(&bytes).await);
        assert_eq!(f.binding().binding_id, current_binding);
        assert_eq!(f.domain_snapshot(), before);
    }
    bounded(f.api.stop_backend_job_recovery()).await;
}

macro_rules! signed_session_case {
    ($name:ident, $change:ident) => {
        #[tokio::test(flavor = "multi_thread", worker_threads = 6)]
        async fn $name() {
            tokio::time::timeout(Duration::from_secs(90), session_case(Change::$change))
                .await
                .expect("bounded Embedded Session case");
        }
    };
}
signed_session_case!(
    signed_embedded_current_returns_read_search_and_list,
    Unchanged
);
signed_session_case!(
    signed_embedded_rotation_withholds_read_search_and_list,
    Rotate
);
signed_session_case!(
    signed_embedded_same_key_reactivation_withholds_read_search_and_list,
    ReactivateSameKey
);
signed_session_case!(
    signed_embedded_aba_withholds_read_search_and_list,
    AbaSameTrust
);
