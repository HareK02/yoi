//! HTTP/WS integration support: real Profile spawn, saved Session restore and
//! execution-owned paired protocol streams. Model I/O is scripted; WS cases also
//! substitute the external Workspace projection source with isolated loopback HTTP
//! through the production RuntimeWorkspaceRequestClient. Controller Submit/refresh,
//! restore, report/join and protocol streams remain production implementations.
//! Uses the ordinary lib suite and shared isolated restore-environment preflight.

use super::*;
use crate::catalog::{ProfileSelector, WorkerDetail};
use crate::config_bundle::{ConfigBundleMetadata, ConfigBundleProvenance, ConfigProfileDescriptor};
use crate::execution::{WorkerExecutionBackend, WorkerExecutionTermination};
use crate::management::RuntimeOptions;
use crate::worker_backend::{ProfileRuntimeWorkerFactory, WorkerRuntimeExecutionBackend};
use agen::llm_client::event::{Event as LlmEvent, ResponseStatus, StatusEvent};
use agen::llm_client::{ClientError, LlmClient, Request as ModelRequest};
use async_trait::async_trait;
use futures::Stream;
use session_store::{Store, WorkerAggregateStore, WorkerMetadataStore, WorkerSessionStore};
use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};

pub(super) const OLD_MARKER: &str = "controller-original-output";
pub(super) const NEW_MARKER: &str = "controller-restored-output";
pub(super) type Backend = WorkerRuntimeExecutionBackend<ProfileRuntimeWorkerFactory>;

#[derive(Clone, Default)]
struct ScriptedModel {
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl LlmClient for ScriptedModel {
    fn clone_boxed(&self) -> Box<dyn LlmClient> {
        Box::new(self.clone())
    }

    async fn stream(
        &self,
        _request: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<LlmEvent, ClientError>> + Send>>, ClientError>
    {
        let marker = match self.calls.fetch_add(1, Ordering::SeqCst) {
            0 => OLD_MARKER,
            1 => NEW_MARKER,
            call => panic!("unexpected model call {call}; fixture permits exactly two Submits"),
        };
        Ok(Box::pin(futures::stream::iter(
            vec![
                LlmEvent::text_block_start(0),
                LlmEvent::text_delta(0, marker),
                LlmEvent::text_block_stop(0, None),
                LlmEvent::Status(StatusEvent {
                    status: ResponseStatus::Completed,
                }),
            ]
            .into_iter()
            .map(Ok),
        )))
    }
}

// A narrow external source, not a refresh/cache/authorization bypass. Keep the
// original Worker WorkspaceApiRef (including its stored URL) untouched; only the
// explicitly injected request client's transport endpoint is substituted. This
// source does not claim to test Workspace Server authentication.
#[cfg(feature = "ws-server")]
struct ProjectionSource {
    client: crate::workspace_request::RuntimeWorkspaceRequestClient,
    server: tokio::task::JoinHandle<()>,
    requests: Arc<AtomicUsize>,
}

#[cfg(feature = "ws-server")]
impl ProjectionSource {
    fn new(root: &std::path::Path) -> Self {
        const ROUTE: &str = "/api/w/local/config/projections/prompts";
        let catalog = worker::PromptCatalog::builtins_only()
            .unwrap()
            .projection()
            .clone();
        let source_digest = if catalog.source_digest.is_empty() {
            catalog.catalog_digest.clone()
        } else {
            catalog.source_digest.clone()
        };
        let projection = worker::WorkspacePromptProjection::new(
            "local",
            source_digest,
            catalog.catalog_digest.clone(),
            catalog,
        )
        .unwrap();
        let path = root.join("workspace-prompt-projection.json");
        std::fs::write(&path, serde_json::to_vec(&projection).unwrap()).unwrap();
        let body = std::fs::read_to_string(path).unwrap();
        let saved: worker::WorkspacePromptProjection = serde_json::from_str(&body).unwrap();
        saved.validate().unwrap();
        assert_eq!(saved, projection);
        let _: server_api::WorkspacePromptProjection = serde_json::from_str(&body).unwrap();

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let listener = tokio::net::TcpListener::from_std(listener).unwrap();
        let requests = Arc::new(AtomicUsize::new(0));
        let received = requests.clone();
        let app = axum::Router::new().fallback(move |request: axum::extract::Request| {
            let body = body.clone();
            let received = received.clone();
            async move {
                if request.method() != axum::http::Method::GET
                    || request.uri().to_string() != ROUTE
                    || request
                        .headers()
                        .get("x-yoi-runtime-id")
                        .and_then(|v| v.to_str().ok())
                        != Some("http-controller-fixture")
                {
                    return Err(axum::http::StatusCode::FORBIDDEN);
                }
                if axum::body::to_bytes(request.into_body(), 0).await.is_err() {
                    return Err(axum::http::StatusCode::BAD_REQUEST);
                }
                received.fetch_add(1, Ordering::SeqCst);
                Ok(([("content-type", "application/json")], body))
            }
        });
        Self {
            client: crate::workspace_request::RuntimeWorkspaceRequestClient::new(
                "local",
                base_url,
                "http-controller-fixture",
            ),
            server: tokio::spawn(async move { axum::serve(listener, app).await.unwrap() }),
            requests,
        }
    }
}

#[cfg(feature = "ws-server")]
impl Drop for ProjectionSource {
    fn drop(&mut self) {
        self.server.abort();
    }
}

// Own the catalog, adapter and loopback tasks until real Stop has joined the
// Controller and its relays. Unwind cleanup never creates termination evidence.
pub(super) struct ControllerFixture {
    runtime: Option<Runtime>,
    backend: Option<Arc<Backend>>,
    pub worker: WorkerDetail,
    create_request: CreateWorkerRequest,
    workspace_scope: RuntimeWorkspaceScope,
    server: Option<tokio::task::JoinHandle<()>>,
    #[cfg(feature = "ws-server")]
    projection_source: Option<ProjectionSource>,
    model: ScriptedModel,
    root: tempfile::TempDir,
}

impl ControllerFixture {
    fn root() -> tempfile::TempDir {
        crate::restore_test_environment::require_restore_test_environment();
        let target = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("target");
        tempfile::Builder::new()
            .prefix("hc")
            .rand_bytes(3)
            .tempdir_in(target)
            .unwrap()
    }

    fn make_backend(
        root: &std::path::Path,
        model: ScriptedModel,
        workspace_client: Option<crate::workspace_request::RuntimeWorkspaceRequestClient>,
    ) -> Arc<Backend> {
        let mut factory = ProfileRuntimeWorkerFactory::new(root)
            .with_runtime_store_dir(root.join("runtime"))
            .with_runtime_id("http-controller-fixture")
            .with_spawn_model_client_factory(move || Box::new(model.clone()));
        if let Some(client) = workspace_client {
            factory = factory.with_workspace_request_client(client);
        }
        Arc::new(WorkerRuntimeExecutionBackend::new(factory).unwrap())
    }

    fn bundle() -> ConfigBundle {
        let archive = crate::profile_archive::ProfileSourceArchive::build(
            crate::profile_archive::ProfileSourceArchiveInput {
                id: "http-controller-profile".into(),
                entrypoints: BTreeMap::from([(
                    "builtin:companion".into(),
                    "profiles/controller.dcdl".into(),
                )]),
                imports: BTreeMap::new(),
                sources: BTreeMap::from([(
                    "profiles/controller.dcdl".into(),
                    r#"{
                    slug = "companion";
                    description = "HTTP Controller fixture";
                    scope = "workspace_read";
                    model = {
                        scheme = "anthropic";
                        model_id = "test-model";
                        auth = { kind = "none"; };
                    };
                    engine = { max_tokens = 100; };
                }"#
                    .into(),
                )]),
            },
        )
        .unwrap();
        ConfigBundle {
            metadata: ConfigBundleMetadata {
                id: "http-controller-bundle".into(),
                digest: String::new(),
                workspace_id: "local".into(),
                created_at: "test".into(),
                provenance: ConfigBundleProvenance {
                    source: "test".into(),
                    detail: None,
                },
            },
            profiles: vec![ConfigProfileDescriptor {
                selector: ProfileSelector::Builtin("builtin:companion".into()),
                label: None,
            }],
            declarations: Vec::new(),
            prompt_catalog: None,
            profile_source_archive: Some(archive),
            profile_source_archive_handle: None,
        }
        .with_computed_digest()
    }

    fn create(
        runtime: Runtime,
        backend: Arc<Backend>,
        root: tempfile::TempDir,
        model: ScriptedModel,
    ) -> Self {
        let bundle = Self::bundle();
        let request = CreateWorkerRequest {
            plugin_execution: None,
            worker_id: WorkerId::now_v7(),
            create_fingerprint: "http-controller-create".into(),
            profile: ProfileSelector::Builtin("builtin:companion".into()),
            display_name: None,
            profile_source: crate::catalog::ProfileSourceArchiveSource::Embedded {
                archive: bundle.profile_source_archive.clone().unwrap(),
            },
            config_bundle: Some(ConfigBundleRef {
                id: bundle.metadata.id.clone(),
                digest: bundle.metadata.digest.clone(),
            }),
            initial_input: None,
            workdir_attachment_requests: Vec::new(),
            workdir_attachments: Vec::new(),
            worker_observation_enabled: false,
            worker_observation_grants: Vec::new(),
            workspace_api: Some(crate::catalog::WorkspaceApiRef {
                workspace_id: "local".into(),
                base_url: "http://127.0.0.1:1".into(),
            }),
            memory_settings: Some(manifest::WorkspaceMemorySettingsSnapshot {
                workspace_id: "local".into(),
                language: "English".into(),
            }),
            subjektiv_attached: false,
            backend_job: None,
        };
        runtime.store_config_bundle(bundle).unwrap();
        let workspace_scope = RuntimeWorkspaceScope::new("local", "local-token");
        let worker = runtime
            .create_worker_scoped(&workspace_scope, request.clone())
            .unwrap();
        Self {
            runtime: Some(runtime),
            backend: Some(backend),
            worker,
            create_request: request,
            workspace_scope,
            server: None,
            #[cfg(feature = "ws-server")]
            projection_source: None,
            model,
            root,
        }
    }

    #[cfg(feature = "ws-server")]
    pub fn memory() -> Self {
        let root = Self::root();
        let model = ScriptedModel::default();
        let projection_source = ProjectionSource::new(root.path());
        let backend = Self::make_backend(
            root.path(),
            model.clone(),
            Some(projection_source.client.clone()),
        );
        let runtime =
            Runtime::with_execution_backend(RuntimeOptions::default(), backend.clone()).unwrap();
        let mut fixture = Self::create(runtime, backend, root, model);
        fixture.projection_source = Some(projection_source);
        fixture
    }

    #[cfg(feature = "fs-store")]
    pub fn persisted() -> Self {
        let root = Self::root();
        let model = ScriptedModel::default();
        let backend = Self::make_backend(root.path(), model.clone(), None);
        let runtime = Runtime::with_fs_store_and_execution_backend(
            crate::fs_store::FsRuntimeStoreOptions::new(root.path().join("runtime"))
                .with_runtime_id("http-controller-fixture"),
            backend.clone(),
        )
        .unwrap();
        Self::create(runtime, backend, root, model)
    }

    pub fn runtime(&self) -> &Runtime {
        self.runtime.as_ref().unwrap()
    }
    pub fn backend(&self) -> &Arc<Backend> {
        self.backend.as_ref().unwrap()
    }
    pub fn root_path(&self) -> &std::path::Path {
        self.root.path()
    }
    pub fn create_request(&self) -> &CreateWorkerRequest {
        &self.create_request
    }

    pub fn restore_request(&self, runtime: &Runtime) -> runtime_api::WorkerRestoreRequest {
        let detail = runtime.worker_detail(&self.worker.worker_ref).unwrap();
        let api = self.create_request.workspace_api.as_ref().unwrap();
        runtime_api::WorkerRestoreRequest {
            expected_observation_token: detail.restore_observation_token.unwrap(),
            request_id: uuid::Uuid::now_v7().to_string(),
            preparation: Some(runtime_api::WorkerRestorePreparation {
                workspace_api: Some(runtime_api::WorkspaceApiRef {
                    workspace_id: api.workspace_id.clone(),
                    base_url: api.base_url.clone(),
                }),
                ..Default::default()
            }),
        }
    }

    #[cfg(feature = "ws-server")]
    pub fn restore(&self) -> WorkerDetail {
        self.runtime()
            .restore_worker_scoped(
                &self.workspace_scope,
                &self.worker.worker_ref,
                self.restore_request(self.runtime()),
            )
            .unwrap()
    }

    // Read the original aggregate through the production decoder and identity
    // validator before discarding its catalog; never repair saved wire data.
    #[cfg(feature = "fs-store")]
    pub fn assert_persisted_request(&self, runtime: &Runtime) {
        let record = runtime
            .test_runtime_store()
            .read_worker_record(&self.worker.worker_ref)
            .unwrap();
        assert_eq!(
            record.workspace_id.as_deref(),
            Some(self.workspace_scope.workspace_id.as_str())
        );
        let crate::fs_store::PersistedWorkerExecutionState::Available(execution) =
            record.execution_state
        else {
            panic!("seed catalog must retain its original available execution request");
        };
        assert_eq!(execution.request, self.create_request);
    }

    // Only use after a real raw Stop or a settled Runtime Stop, never to imply
    // that dropping the old adapter proves shutdown to its replacement.
    #[cfg(feature = "fs-store")]
    pub fn close_catalog(&mut self) {
        self.runtime.take();
        self.backend.take();
    }

    #[cfg(feature = "fs-store")]
    pub fn empty_backend(&self) -> Arc<Backend> {
        Self::make_backend(self.root.path(), self.model.clone(), None)
    }

    pub fn saved_session(&self) -> (session_store::SessionId, session_store::SegmentId) {
        let aggregate = self
            .root
            .path()
            .join("runtime/workers")
            .join(self.worker.worker_id.to_string());
        let name =
            ProfileRuntimeWorkerFactory::runtime_worker_name_for_ref(&self.worker.worker_ref);
        let metadata = WorkerAggregateStore::open_read_only(&aggregate, name.clone())
            .unwrap()
            .read_by_name(&name)
            .unwrap()
            .unwrap();
        let active = metadata
            .active
            .expect("real spawn must save Worker metadata");
        let segment = active
            .segment_id
            .expect("real Submit must materialize the saved Session");
        let entries = WorkerSessionStore::open_read_only(aggregate.join("session"))
            .unwrap()
            .read_all(active.session_id, segment)
            .unwrap();
        assert!(
            !entries.is_empty(),
            "saved Session must contain actual Controller log entries"
        );
        (active.session_id, segment)
    }

    pub fn stop(&self) -> WorkerExecutionTermination {
        self.runtime()
            .stop_worker(&self.worker.worker_ref, None)
            .unwrap();
        let receipt = self
            .runtime()
            .execution_termination(&self.worker.worker_ref)
            .unwrap()
            .unwrap();
        assert_real_termination(&self.worker.worker_ref, &receipt);
        assert_eq!(
            self.backend()
                .termination_receipt(&self.worker.worker_ref)
                .unwrap(),
            Some(receipt.clone())
        );
        receipt
    }

    pub fn assert_model_calls(&self, expected: usize) {
        assert_eq!(self.model.calls.load(Ordering::SeqCst), expected);
    }

    #[cfg(feature = "ws-server")]
    pub async fn finish(&mut self) -> WorkerExecutionTermination {
        let receipt = self.stop();
        if let Some(server) = self.server.take() {
            server.abort();
            let _ = server.await;
        }
        if let Some(mut source) = self.projection_source.take() {
            assert!(
                source.requests.load(Ordering::SeqCst) > 0,
                "real Submit must fetch the saved projection through the production client"
            );
            source.server.abort();
            let _ = (&mut source.server).await;
        }
        receipt
    }

    #[cfg(feature = "ws-server")]
    pub async fn serve(&mut self) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let runtime = self.runtime().clone();
        self.server = Some(tokio::spawn(async move {
            serve_runtime_http(runtime, listener, Some("local-token".into()))
                .await
                .unwrap();
        }));
        format!(
            "ws://{address}/v1/workers/{}/protocol/ws",
            self.worker.worker_id
        )
    }
}

pub(super) fn assert_real_termination(worker: &WorkerRef, receipt: &WorkerExecutionTermination) {
    assert_eq!(
        receipt.scope,
        crate::execution::WorkerExecutionTerminationScope::Controller
    );
    assert!(receipt.confirms_scope_shutdown(worker, receipt.stop_operation_id));
    assert!(
        receipt.shutdown_command_id.is_some(),
        "real Controller must acknowledge Shutdown"
    );
    assert!(receipt.feature_failures.is_empty(), "{receipt:?}");
}

impl Drop for ControllerFixture {
    fn drop(&mut self) {
        if let Some(runtime) = &self.runtime {
            let _ = runtime.stop_worker(&self.worker.worker_ref, None);
        }
        if let Some(server) = self.server.take() {
            server.abort();
        }
        self.runtime.take();
        self.backend.take();
    }
}
