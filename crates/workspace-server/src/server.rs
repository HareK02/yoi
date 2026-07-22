use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use axum::extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade};
use axum::extract::{Path as AxumPath, Query, State};
use axum::http::header::{CONTENT_TYPE, ETAG, IF_NONE_MATCH, LOCATION};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use chrono::{SecondsFormat, Utc};
use futures::{SinkExt, StreamExt};
use memory::backend::{
    MemoryBackendHttpResponse, MemoryBackendOperation, execute_memory_backend_operation,
};
use protocol::stream::{decode_method, encode_event};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use ticket::{
    LocalTicketBackend, TicketBackendHttpResponse, TicketBackendOperation,
    execute_ticket_backend_operation,
};
use tokio::net::TcpListener;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message as TungsteniteMessage;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use worker_runtime::resource::{BackendResourceError, BackendResourceFetchRequest};
use worker_runtime::worker_backend::{ProfileRuntimeWorkerFactory, WorkerRuntimeExecutionBackend};

use crate::companion::{
    CompanionCancelRequest, CompanionConsole, CompanionMessageRequest, CompanionMessageResponse,
    CompanionStatusResponse, CompanionTranscriptProjection,
};
use crate::config::{RemoteRuntimeConfigFile, WorkspaceBackendConfigFile, resolve_remote_runtime};
use crate::hosts::{
    ConfigBundleCheckResult, ConfigBundleSyncResult, DiagnosticSeverity, EmbeddedWorkerRuntime,
    HostSummary, RemoteRuntimeConfig, RemoteWorkerRuntime, RuntimeDiagnostic, RuntimeRegistry,
    RuntimeRegistryError, RuntimeRegistryUnregisterResult, RuntimeSummary, WorkerCapabilitySummary,
    WorkerCompletionsRequest, WorkerCompletionsResult, WorkerImplementationSummary,
    WorkerInputRequest, WorkerInputResult, WorkerLifecycleRequest, WorkerLifecycleResult,
    WorkerOperationState, WorkerSpawnAcceptanceRequirement, WorkerSpawnIntent, WorkerSpawnRequest,
    WorkerSpawnResult, WorkerSpawnWorkingDirectoryRequest, WorkerSummary, WorkerWorkspaceSummary,
};
use crate::identity::WorkspaceIdentity;
use crate::observation::{
    BackendObservationProxy, ObservationProxyError, RuntimeObservationClient,
    RuntimeObservationSource, RuntimeObservationSourceConfig,
};
use crate::profile_settings::{
    CreateWorkspaceProfileSourceRequest, DeleteWorkspaceProfileSourceRequest,
    DeleteWorkspaceProfileTreeFileRequest, ReadWorkspaceProfileTreeFileQuery,
    UpdateWorkspaceMetadataRequest, UpdateWorkspaceProfileRegistryRequest,
    UpdateWorkspaceProfileSourceRequest, WriteWorkspaceProfileTreeFileRequest,
};
use crate::records::{
    LocalProjectRecordReader, ObjectiveDetail, ProjectRecordList, TicketDetail, TicketSummary,
};
use crate::repositories::{
    ConfiguredRepository, RepositoryListProjection, RepositoryLogRead, RepositoryLookupError,
    RepositoryRegistryReader, RepositorySummary,
};
use crate::resource_broker::BackendResourceBroker;
use crate::skills;
use crate::store::{
    ControlPlaneStore, WorkdirRegistryRecord, WorkerRegistryRecord, WorkerWorkdirLinkRecord,
    WorkspaceRecord,
};
use crate::{Error, Result};
use worker_runtime::catalog::{
    ConfigBundleRef, MaterializerKind, ProfileSelector,
    RepositorySelector as RuntimeRepositorySelector, WorkingDirectoryClaim,
    WorkingDirectoryRepository, WorkingDirectoryRequest, WorkingDirectoryStatusKind,
    WorkingDirectorySummary,
};
use worker_runtime::config_bundle::ConfigBundle;
use worker_runtime::http_server::{
    RuntimeHttpConfigBundleAvailabilityResponse, RuntimeHttpConfigBundlesResponse,
    RuntimeHttpSummaryResponse, RuntimeHttpWorkerResponse, RuntimeHttpWorkersResponse,
};
use worker_runtime::interaction::{
    WorkerInput as EmbeddedWorkerInput, WorkerInputKind as EmbeddedWorkerInputKind,
};

const EMBEDDED_WORKER_RUNTIME_ID: &str = "embedded-worker-runtime";

fn embedded_runtime_id() -> String {
    EMBEDDED_WORKER_RUNTIME_ID.to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum AuthConfig {
    /// Local/dev-only mode. If a token is configured by a future entrypoint, it
    /// is a development guard only and not a production SaaS auth model.
    LocalDevToken { token_configured: bool },
}

#[derive(Clone)]
pub struct ServerConfig {
    pub workspace_id: String,
    pub workspace_display_name: String,
    pub workspace_created_at: String,
    pub workspace_root: PathBuf,
    pub frontend_url: String,
    pub embedded_runtime_store_root: PathBuf,
    pub static_assets_dir: Option<PathBuf>,
    pub auth: AuthConfig,
    pub max_records: usize,
    pub repositories: Vec<ConfiguredRepository>,
    pub runtime_event_sources: Vec<RuntimeObservationSourceConfig>,
    pub remote_runtime_sources: Vec<RemoteRuntimeConfig>,
    pub backend_base_url: Option<String>,
}

impl ServerConfig {
    pub fn local_dev(workspace_root: impl Into<PathBuf>, identity: WorkspaceIdentity) -> Self {
        let workspace_root = workspace_root.into();
        let workspace_id = identity.workspace_id;
        let embedded_runtime_store_root = Self::default_embedded_runtime_store_root(&workspace_id);
        Self {
            workspace_id,
            workspace_display_name: identity.display_name,
            workspace_created_at: identity.created_at,
            workspace_root,
            frontend_url: "http://127.0.0.1:5173".to_string(),
            embedded_runtime_store_root,
            static_assets_dir: None,
            auth: AuthConfig::LocalDevToken {
                token_configured: false,
            },
            max_records: 200,
            repositories: Vec::new(),
            runtime_event_sources: Vec::new(),
            remote_runtime_sources: Vec::new(),
            backend_base_url: None,
        }
    }

    pub fn workspace_backend_data_root_for_data_dir(
        data_dir: impl Into<PathBuf>,
        workspace_id: impl AsRef<str>,
    ) -> PathBuf {
        data_dir
            .into()
            .join("workspace-server")
            .join(workspace_id.as_ref())
    }

    pub fn default_workspace_backend_data_root(workspace_id: impl AsRef<str>) -> PathBuf {
        match manifest::paths::data_dir() {
            Some(data_dir) => {
                Self::workspace_backend_data_root_for_data_dir(data_dir, workspace_id.as_ref())
            }
            None => std::env::temp_dir()
                .join("yoi")
                .join("workspace-server")
                .join(workspace_id.as_ref()),
        }
    }

    pub fn embedded_runtime_store_root_for_data_dir(
        data_dir: impl Into<PathBuf>,
        workspace_id: impl AsRef<str>,
    ) -> PathBuf {
        Self::workspace_backend_data_root_for_data_dir(data_dir, workspace_id)
            .join("embedded-runtime")
    }

    pub fn default_embedded_runtime_store_root(workspace_id: impl AsRef<str>) -> PathBuf {
        Self::default_workspace_backend_data_root(workspace_id).join("embedded-runtime")
    }

    pub fn with_embedded_runtime_store_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.embedded_runtime_store_root = root.into();
        self
    }
}

#[derive(Clone)]
pub struct WorkspaceApi {
    config: ServerConfig,
    store: Arc<dyn ControlPlaneStore>,
    records: LocalProjectRecordReader,
    runtime: Arc<RuntimeRegistry>,
    companion: Arc<CompanionConsole>,
    observation_proxy: BackendObservationProxy,
    resource_broker: BackendResourceBroker,
}

impl WorkspaceApi {
    pub async fn new(config: ServerConfig, store: Arc<dyn ControlPlaneStore>) -> Result<Self> {
        let resource_broker = BackendResourceBroker::default();
        let execution_backend = WorkerRuntimeExecutionBackend::new(
            ProfileRuntimeWorkerFactory::new(config.workspace_root.clone())
                .with_resource_client(Arc::new(resource_broker.clone())),
        )
        .map_err(|err| {
            crate::Error::Store(format!(
                "failed to initialize embedded Worker backend: {err}"
            ))
        })?;
        Self::new_with_execution_backend_and_broker(
            config,
            store,
            Arc::new(execution_backend),
            resource_broker,
        )
        .await
    }

    #[cfg(test)]
    async fn new_with_execution_backend(
        config: ServerConfig,
        store: Arc<dyn ControlPlaneStore>,
        execution_backend: Arc<dyn worker_runtime::execution::WorkerExecutionBackend>,
    ) -> Result<Self> {
        Self::new_with_execution_backend_and_broker(
            config,
            store,
            execution_backend,
            BackendResourceBroker::default(),
        )
        .await
    }

    async fn new_with_execution_backend_and_broker(
        config: ServerConfig,
        store: Arc<dyn ControlPlaneStore>,
        execution_backend: Arc<dyn worker_runtime::execution::WorkerExecutionBackend>,
        resource_broker: BackendResourceBroker,
    ) -> Result<Self> {
        store
            .upsert_workspace(&WorkspaceRecord {
                workspace_id: config.workspace_id.clone(),
                display_name: config.workspace_display_name.clone(),
                state: "active".to_string(),
                created_at: config.workspace_created_at.clone(),
                updated_at: config.workspace_created_at.clone(),
            })
            .await?;
        let runtime = RuntimeRegistry::for_workspace(
            EmbeddedWorkerRuntime::new_fs_store_with_execution_backend(
                config.workspace_id.clone(),
                config.embedded_runtime_store_root.clone(),
                execution_backend,
            )
            .map(|runtime| runtime.with_resource_broker(resource_broker.clone()))
            .map(|runtime| {
                runtime.with_backend_base_url(
                    config
                        .backend_base_url
                        .clone()
                        .unwrap_or_else(|| "http://127.0.0.1:8787".to_string()),
                )
            })
            .map_err(|err| {
                crate::Error::Store(format!("invalid embedded Worker backend: {err}"))
            })?,
        );
        for remote_config in config.remote_runtime_sources.iter().cloned() {
            runtime.register(
                RemoteWorkerRuntime::new(
                    remote_config,
                    config.workspace_id.clone(),
                    config
                        .backend_base_url
                        .clone()
                        .unwrap_or_else(|| "http://127.0.0.1:8787".to_string()),
                )
                .map(|host| host.with_resource_broker(resource_broker.clone()))
                .map_err(|err| err.into_error())?,
            );
        }
        let runtime = Arc::new(runtime);
        let companion = Arc::new(CompanionConsole::disabled());
        let observation_proxy = BackendObservationProxy::new(config.runtime_event_sources.clone());
        Ok(Self {
            records: LocalProjectRecordReader::new(config.workspace_root.clone())?,
            config,
            store,
            runtime,
            companion,
            observation_proxy,
            resource_broker,
        })
    }

    pub fn workspace_id(&self) -> &str {
        self.config.workspace_id.as_str()
    }

    fn repository_reader(&self) -> RepositoryRegistryReader {
        RepositoryRegistryReader::new(self.config.repositories.clone())
    }
}

pub fn build_router(api: WorkspaceApi) -> Router {
    Router::new()
        .route("/api/workspace", get(get_workspace))
        .route("/api/w/{workspace_id}/workspace", get(scoped_get_workspace))
        .route(
            "/api/w/{workspace_id}/settings/workspace",
            get(scoped_get_workspace_settings).put(scoped_update_workspace_settings),
        )
        .route(
            "/api/w/{workspace_id}/settings/profiles",
            get(scoped_get_profile_settings).post(scoped_create_profile_source),
        )
        .route(
            "/api/w/{workspace_id}/settings/profiles/registry",
            put(scoped_update_profile_registry),
        )
        .route(
            "/api/w/{workspace_id}/settings/profiles/trees/{source_tree_id}",
            get(scoped_get_profile_source_tree),
        )
        .route(
            "/api/w/{workspace_id}/settings/profiles/trees/{source_tree_id}/file",
            get(scoped_get_profile_tree_file)
                .put(scoped_write_profile_tree_file)
                .delete(scoped_delete_profile_tree_file),
        )
        .route(
            "/api/w/{workspace_id}/settings/profiles/{profile_source_id}",
            get(scoped_get_profile_source)
                .put(scoped_update_profile_source)
                .delete(scoped_delete_profile_source),
        )
        .route("/api/tickets", get(list_tickets))
        .route("/api/w/{workspace_id}/tickets", get(scoped_list_tickets))
        .route(
            "/api/w/{workspace_id}/tickets/backend",
            post(scoped_ticket_backend_operation),
        )
        .route(
            "/api/w/{workspace_id}/memory/backend",
            post(scoped_memory_backend_operation),
        )
        .route("/api/tickets/{id}", get(get_ticket))
        .route("/api/w/{workspace_id}/skills", get(scoped_list_skills))
        .route("/api/w/{workspace_id}/skills/lint", get(scoped_lint_skills))
        .route("/api/w/{workspace_id}/skills/{name}", get(scoped_get_skill))
        .route(
            "/api/w/{workspace_id}/skills/{name}/activate",
            get(scoped_activate_skill),
        )
        .route("/api/w/{workspace_id}/tickets/{id}", get(scoped_get_ticket))
        .route("/api/objectives", get(list_objectives))
        .route(
            "/api/w/{workspace_id}/objectives",
            get(scoped_list_objectives),
        )
        .route("/api/objectives/{id}", get(get_objective))
        .route(
            "/api/w/{workspace_id}/objectives/{id}",
            get(scoped_get_objective),
        )
        .route("/api/repositories", get(list_repositories))
        .route(
            "/api/w/{workspace_id}/repositories",
            get(scoped_list_repositories),
        )
        .route("/api/repositories/{repository_id}", get(repository_detail))
        .route(
            "/api/w/{workspace_id}/repositories/{repository_id}",
            get(scoped_repository_detail),
        )
        .route("/api/repositories/{repository_id}/log", get(repository_log))
        .route(
            "/api/w/{workspace_id}/repositories/{repository_id}/log",
            get(scoped_repository_log),
        )
        .route(
            "/api/repositories/{repository_id}/tickets",
            get(repository_tickets),
        )
        .route(
            "/api/w/{workspace_id}/repositories/{repository_id}/tickets",
            get(scoped_repository_tickets),
        )
        .route("/api/hosts", get(list_hosts))
        .route("/api/w/{workspace_id}/hosts", get(scoped_list_hosts))
        .route(
            "/api/w/{workspace_id}/profile-source-archives/{digest}",
            get(scoped_get_profile_source_archive),
        )
        .route(
            "/api/w/{workspace_id}/runtimes/{runtime_id}/working-directories",
            get(scoped_list_runtime_working_directories).post(scoped_create_runtime_working_directory),
        )
        .route(
            "/api/w/{workspace_id}/runtimes/{runtime_id}/working-directories/{working_directory_id}",
            get(scoped_runtime_working_directory_detail).delete(scoped_cleanup_runtime_working_directory),
        )
        .route(
            "/api/w/{workspace_id}/working-directories",
            get(scoped_list_working_directories).post(scoped_create_working_directory),
        )
        .route(
            "/api/w/{workspace_id}/working-directories/{working_directory_id}",
            get(scoped_working_directory_detail).delete(scoped_cleanup_working_directory),
        )
        .route("/api/runtimes", get(list_runtimes))
        .route("/api/w/{workspace_id}/runtimes", get(scoped_list_runtimes))
        .route(
            "/api/workers",
            get(list_workers).post(create_workspace_worker),
        )
        .route(
            "/api/w/{workspace_id}/workers",
            get(scoped_list_workers).post(scoped_create_workspace_worker),
        )
        .route(
            "/api/workers/launch-options",
            get(get_worker_launch_options),
        )
        .route(
            "/api/w/{workspace_id}/workers/launch-options",
            get(scoped_get_worker_launch_options),
        )
        .route(
            "/api/settings/runtime-connections",
            get(get_runtime_connection_settings),
        )
        .route(
            "/api/w/{workspace_id}/settings/runtime-connections",
            get(scoped_get_runtime_connection_settings),
        )
        .route(
            "/api/settings/runtime-connections/remotes",
            post(add_remote_runtime_connection),
        )
        .route(
            "/api/w/{workspace_id}/settings/runtime-connections/remotes",
            post(scoped_add_remote_runtime_connection),
        )
        .route(
            "/api/settings/runtime-connections/remotes/{runtime_id}",
            delete(delete_remote_runtime_connection),
        )
        .route(
            "/api/w/{workspace_id}/settings/runtime-connections/remotes/{runtime_id}",
            delete(scoped_delete_remote_runtime_connection),
        )
        .route(
            "/api/settings/runtime-connections/remotes/{runtime_id}/test",
            post(test_remote_runtime_connection),
        )
        .route(
            "/api/w/{workspace_id}/settings/runtime-connections/remotes/{runtime_id}/test",
            post(scoped_test_remote_runtime_connection),
        )
        .route(
            "/internal/runtime/resources/fetch",
            post(post_internal_runtime_resource_fetch),
        )
        .route("/api/companion/status", get(get_companion_status))
        .route(
            "/api/w/{workspace_id}/companion/status",
            get(scoped_get_companion_status),
        )
        .route("/api/companion/transcript", get(get_companion_transcript))
        .route(
            "/api/w/{workspace_id}/companion/transcript",
            get(scoped_get_companion_transcript),
        )
        .route("/api/companion/messages", post(post_companion_message))
        .route(
            "/api/w/{workspace_id}/companion/messages",
            post(scoped_post_companion_message),
        )
        .route("/api/companion/cancel", post(post_companion_cancel))
        .route(
            "/api/w/{workspace_id}/companion/cancel",
            post(scoped_post_companion_cancel),
        )
        .route(
            "/api/runtimes/{runtime_id}/workers",
            get(list_runtime_workers).post(create_runtime_worker),
        )
        .route(
            "/api/w/{workspace_id}/runtimes/{runtime_id}/workers",
            get(scoped_list_runtime_workers).post(scoped_create_runtime_worker),
        )
        .route(
            "/api/runtimes/{runtime_id}/config-bundles",
            post(sync_runtime_config_bundle),
        )
        .route(
            "/api/w/{workspace_id}/runtimes/{runtime_id}/config-bundles",
            post(scoped_sync_runtime_config_bundle),
        )
        .route(
            "/api/runtimes/{runtime_id}/config-bundles/{bundle_id}/availability",
            get(check_runtime_config_bundle),
        )
        .route(
            "/api/w/{workspace_id}/runtimes/{runtime_id}/config-bundles/{bundle_id}/availability",
            get(scoped_check_runtime_config_bundle),
        )
        .route(
            "/api/runtimes/{runtime_id}/workers/{worker_id}",
            get(get_runtime_worker),
        )
        .route(
            "/api/w/{workspace_id}/runtimes/{runtime_id}/workers/{worker_id}",
            get(scoped_get_runtime_worker),
        )
        .route(
            "/api/w/{workspace_id}/runtimes/{runtime_id}/workers/{worker_id}/pin",
            put(scoped_pin_runtime_worker).delete(scoped_unpin_runtime_worker),
        )
        .route(
            "/api/w/{workspace_id}/runtimes/{runtime_id}/cleanup-plan",
            get(scoped_runtime_cleanup_plan),
        )
        .route(
            "/api/w/{workspace_id}/runtimes/{runtime_id}/cleanup-executions",
            post(scoped_execute_runtime_cleanup),
        )
        .route(
            "/api/runtimes/{runtime_id}/workers/{worker_id}/input",
            post(send_runtime_worker_input),
        )
        .route(
            "/api/w/{workspace_id}/runtimes/{runtime_id}/workers/{worker_id}/input",
            post(scoped_send_runtime_worker_input),
        )
        .route(
            "/api/runtimes/{runtime_id}/workers/{worker_id}/completions",
            post(runtime_worker_completions),
        )
        .route(
            "/api/w/{workspace_id}/runtimes/{runtime_id}/workers/{worker_id}/completions",
            post(scoped_runtime_worker_completions),
        )
        .route(
            "/api/runtimes/{runtime_id}/workers/{worker_id}/stop",
            post(stop_runtime_worker),
        )
        .route(
            "/api/w/{workspace_id}/runtimes/{runtime_id}/workers/{worker_id}/stop",
            post(scoped_stop_runtime_worker),
        )
        .route(
            "/api/runtimes/{runtime_id}/workers/{worker_id}/cancel",
            post(cancel_runtime_worker),
        )
        .route(
            "/api/w/{workspace_id}/runtimes/{runtime_id}/workers/{worker_id}/cancel",
            post(scoped_cancel_runtime_worker),
        )
        .route(
            "/api/runtimes/{runtime_id}/workers/{worker_id}/protocol/ws",
            get(worker_protocol_ws),
        )
        .route(
            "/api/w/{workspace_id}/runtimes/{runtime_id}/workers/{worker_id}/protocol/ws",
            get(scoped_worker_protocol_ws),
        )
        .route("/api/hosts/{host_id}/workers", get(list_host_workers))
        .route(
            "/api/w/{workspace_id}/hosts/{host_id}/workers",
            get(scoped_list_host_workers),
        )
        .fallback(get(static_or_spa_fallback))
        .with_state(api)
}

pub async fn serve(
    config: ServerConfig,
    store: Arc<dyn ControlPlaneStore>,
    listener: TcpListener,
) -> Result<()> {
    let api = WorkspaceApi::new(config, store).await?;
    axum::serve(listener, build_router(api)).await?;
    Ok(())
}

#[derive(Debug, Serialize, Deserialize)]
pub struct WorkspaceResponse {
    pub workspace_id: String,
    pub display_name: String,
    pub record_authority: String,
    pub schema_version: i64,
    pub auth: AuthConfig,
    pub extension_points: ExtensionPoints,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ExtensionPoints {
    pub store: String,
    pub event_stream: ExtensionPointState,
    pub host_worker_bridge: ExtensionPointState,
    pub companion_console: ExtensionPointState,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ExtensionPointState {
    pub status: String,
    pub note: String,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ListResponse<T> {
    pub workspace_id: String,
    pub limit: usize,
    pub items: Vec<T>,
    pub invalid_records: Vec<crate::records::InvalidProjectRecord>,
    pub record_authority: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RuntimeListResponse<T> {
    pub workspace_id: String,
    pub limit: usize,
    pub items: Vec<T>,
    pub source: String,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CleanupTargetKind {
    WorkerDelete,
    WorkdirCleanCleanup,
    WorkdirDirtyDiscard,
    WorkdirRecordDelete,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CleanupWorkdirFileStatus {
    Pending,
    Present,
    Active,
    CleanupPending,
    NotFound,
    Corrupted,
    Failed,
    Unknown,
}

impl CleanupWorkdirFileStatus {
    fn from_registry(value: &str) -> Self {
        match value {
            "pending" => Self::Pending,
            "present" => Self::Present,
            "active" => Self::Active,
            "cleanup_pending" => Self::CleanupPending,
            "not_found" => Self::NotFound,
            "corrupted" => Self::Corrupted,
            "failed" => Self::Failed,
            "unknown" => Self::Unknown,
            _ => Self::Unknown,
        }
    }

    fn from_runtime(value: &WorkingDirectoryStatusKind) -> Self {
        match value {
            WorkingDirectoryStatusKind::Active => Self::Active,
            WorkingDirectoryStatusKind::CleanupPending => Self::CleanupPending,
            WorkingDirectoryStatusKind::Corrupted => Self::Corrupted,
            WorkingDirectoryStatusKind::NotFound => Self::NotFound,
            WorkingDirectoryStatusKind::Unknown => Self::Unknown,
        }
    }

    fn is_record_only(self) -> bool {
        matches!(self, Self::NotFound)
    }

    fn is_corrupted(self) -> bool {
        matches!(self, Self::Corrupted)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CleanupWorkdirCleanliness {
    Clean,
    Dirty,
    Unknown,
}

impl CleanupWorkdirCleanliness {
    fn from_registry(value: &str) -> Self {
        match value {
            "clean" => Self::Clean,
            "dirty" => Self::Dirty,
            "unknown" => Self::Unknown,
            _ => Self::Unknown,
        }
    }

    fn from_runtime(value: Option<&str>) -> Self {
        value.map(Self::from_registry).unwrap_or(Self::Unknown)
    }

    fn is_clean(self) -> bool {
        matches!(self, Self::Clean)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CleanupWorkerCandidate {
    pub target_id: String,
    pub action: CleanupTargetKind,
    pub worker_id: String,
    pub runtime_worker_id: String,
    pub runtime_id: String,
    pub reason: String,
    pub blocking_reason: Option<String>,
    pub pinned: bool,
    pub retention_state: String,
    pub linked_workdir_ids: Vec<String>,
    pub running_linked: bool,
    pub estimated_reclaim_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CleanupWorkdirCandidate {
    pub target_id: String,
    pub action: CleanupTargetKind,
    pub workdir_id: String,
    pub runtime_id: String,
    pub repository_id: String,
    pub reason: String,
    pub blocking_reason: Option<String>,
    pub linked_worker_ids: Vec<String>,
    pub linked_running_worker_ids: Vec<String>,
    pub running_linked: bool,
    pub pinned_linked: bool,
    pub file_status: CleanupWorkdirFileStatus,
    pub cleanliness: CleanupWorkdirCleanliness,
    pub estimated_reclaim_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeCleanupPlanResponse {
    pub workspace_id: String,
    pub runtime_id: String,
    pub generated_at: String,
    pub revision: String,
    pub digest: String,
    pub workers: Vec<CleanupWorkerCandidate>,
    pub workdirs: Vec<CleanupWorkdirCandidate>,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecuteRuntimeCleanupRequest {
    pub expected_plan_revision: String,
    pub expected_plan_digest: String,
    #[serde(default)]
    pub worker_target_ids: Vec<String>,
    #[serde(default)]
    pub workdir_target_ids: Vec<String>,
    #[serde(default)]
    pub confirm_dirty_discard_target_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeCleanupExecutionResult {
    pub target_id: String,
    pub action: CleanupTargetKind,
    pub status: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeCleanupExecutionResponse {
    pub workspace_id: String,
    pub runtime_id: String,
    pub executed_at: String,
    pub results: Vec<RuntimeCleanupExecutionResult>,
    pub plan_after: RuntimeCleanupPlanResponse,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerRetentionResponse {
    pub workspace_id: String,
    pub runtime_id: String,
    pub worker_id: String,
    pub pinned: bool,
    pub retention_state: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RuntimeConnectionSettingsResponse {
    pub workspace_id: String,
    pub embedded: RuntimeConnectionSummary,
    pub remotes: Vec<RemoteRuntimeConnectionSummary>,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RuntimeConnectionSummary {
    pub runtime_id: String,
    pub display_name: String,
    pub kind: String,
    pub built_in: bool,
    pub config_managed: bool,
    pub active: bool,
    pub can_spawn_worker: bool,
    pub restart_required: bool,
    pub status: String,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RemoteRuntimeConnectionSummary {
    #[serde(flatten)]
    pub summary: RuntimeConnectionSummary,
    pub endpoint_configured: bool,
    pub token_ref_configured: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RuntimeConnectionMutationResponse {
    pub workspace_id: String,
    pub restart_required: bool,
    pub remotes: Vec<RemoteRuntimeConnectionSummary>,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AddRemoteRuntimeConnectionRequest {
    pub runtime_id: String,
    pub display_name: Option<String>,
    pub endpoint: String,
    pub token_ref: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RemoteRuntimeTestResponse {
    pub workspace_id: String,
    pub runtime_id: String,
    pub checked_at: String,
    pub state: String,
    pub protocol_version: Option<String>,
    pub compatibility_basis: String,
    pub capabilities: Vec<String>,
    pub health_result: String,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct WorkerLaunchOptionsResponse {
    pub workspace_id: String,
    pub runtimes: Vec<WorkerLaunchRuntimeOption>,
    pub profiles: Vec<WorkerLaunchProfileCandidate>,
    pub repositories: Vec<WorkingDirectoryRepositoryOption>,
    pub working_directories: Vec<WorkingDirectorySummary>,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct WorkerLaunchRuntimeOption {
    pub runtime_id: String,
    pub display_name: String,
    pub built_in: bool,
    pub can_spawn_worker: bool,
    pub working_directory_required: bool,
    pub status: String,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerLaunchProfileCandidate {
    pub id: String,
    pub label: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkingDirectoryRepositoryOption {
    pub id: String,
    pub display_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_selector: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrowserWorkingDirectoryCreateRequest {
    #[serde(default = "embedded_runtime_id")]
    pub runtime_id: String,
    pub repository_id: String,
    #[serde(default)]
    pub selector: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BrowserWorkingDirectoryListResponse {
    pub workspace_id: String,
    pub items: Vec<WorkingDirectorySummary>,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BrowserWorkingDirectoryDetailResponse {
    pub workspace_id: String,
    pub item: WorkingDirectorySummary,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrowserWorkerWorkingDirectorySelection {
    pub working_directory_id: String,
    #[serde(default)]
    pub relative_cwd: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrowserCreateWorkerRequest {
    pub runtime_id: String,
    pub display_name: String,
    pub profile: String,
    pub initial_text: String,
    #[serde(default)]
    pub working_directory: Option<BrowserWorkerWorkingDirectorySelection>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BrowserCreateWorkerResponse {
    pub workspace_id: String,
    pub runtime_id: String,
    pub worker_id: String,
    pub console_href: String,
    pub worker: WorkerSummary,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RepositoryListResponse {
    pub workspace_id: String,
    pub items: Vec<RepositorySummary>,
    pub source: String,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RepositoryDetailResponse {
    pub workspace_id: String,
    pub item: RepositorySummary,
    pub source: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RepositoryLogResponse {
    pub workspace_id: String,
    pub repository_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_selector: Option<String>,
    pub limit: usize,
    pub items: Vec<crate::repositories::GitCommitSummary>,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RepositoryTicketsResponse {
    pub workspace_id: String,
    pub repository_id: String,
    pub limit: usize,
    pub columns: Vec<TicketKanbanColumn>,
    pub invalid_records: Vec<crate::records::InvalidProjectRecord>,
    pub record_authority: String,
    pub source: String,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TicketKanbanColumn {
    pub state: String,
    pub items: Vec<TicketSummary>,
}

#[derive(Debug, Deserialize)]
struct LogQuery {
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct TicketKanbanQuery {
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct TranscriptQuery {
    start: Option<usize>,
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct ScopedWorkspacePath {
    workspace_id: String,
}

#[derive(Debug, Deserialize)]
struct ScopedRecordPath {
    workspace_id: String,
    id: String,
}

#[derive(Debug, Deserialize)]
struct ScopedSkillPath {
    workspace_id: String,
    name: String,
}

#[derive(Debug, Deserialize)]
struct ScopedRepositoryPath {
    workspace_id: String,
    repository_id: String,
}

#[derive(Debug, Deserialize)]
struct ScopedProfileArchivePath {
    workspace_id: String,
    digest: String,
}

#[derive(Debug, Deserialize)]
struct ScopedHostPath {
    workspace_id: String,
    host_id: String,
}

#[derive(Debug, Deserialize)]
struct ScopedRuntimePath {
    workspace_id: String,
    runtime_id: String,
}

#[derive(Debug, Deserialize)]
struct ScopedWorkingDirectoryPath {
    workspace_id: String,
    working_directory_id: String,
}

#[derive(Debug, Deserialize)]
struct ScopedRuntimeWorkingDirectoryPath {
    workspace_id: String,
    runtime_id: String,
    working_directory_id: String,
}

#[derive(Debug, Deserialize)]
struct ScopedConfigBundlePath {
    workspace_id: String,
    runtime_id: String,
    bundle_id: String,
}

#[derive(Debug, Deserialize)]
struct ScopedRuntimeWorkerPath {
    workspace_id: String,
    runtime_id: String,
    worker_id: String,
}

fn validate_workspace_scope(api: &WorkspaceApi, workspace_id: &str) -> ApiResult<()> {
    if workspace_id == api.workspace_id() {
        Ok(())
    } else {
        Err(workspace_id_mismatch_error())
    }
}

async fn scoped_get_workspace(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedWorkspacePath>,
) -> ApiResult<Json<WorkspaceResponse>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    get_workspace(State(api)).await
}

async fn scoped_get_workspace_settings(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedWorkspacePath>,
) -> ApiResult<Json<crate::profile_settings::WorkspaceMetadataSettingsResponse>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    Ok(Json(crate::profile_settings::workspace_metadata_settings(
        &api.config.workspace_root,
        &api.config.workspace_id,
        &api.config.workspace_created_at,
        &api.config.workspace_display_name,
    )))
}

async fn scoped_update_workspace_settings(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedWorkspacePath>,
    Json(request): Json<UpdateWorkspaceMetadataRequest>,
) -> ApiResult<Json<crate::profile_settings::WorkspaceMetadataMutationResponse>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    let workspace =
        crate::profile_settings::update_workspace_metadata(&api.config.workspace_root, request)?;
    Ok(Json(
        crate::profile_settings::WorkspaceMetadataMutationResponse {
            workspace,
            diagnostics: vec![RuntimeDiagnostic {
                code: "workspace_metadata_updated".to_string(),
                severity: DiagnosticSeverity::Info,
                message: "Workspace display metadata was updated.".to_string(),
            }],
        },
    ))
}

async fn scoped_get_profile_settings(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedWorkspacePath>,
) -> ApiResult<Json<crate::profile_settings::ProfileSettingsResponse>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    Ok(Json(crate::profile_settings::load_profile_settings(
        &api.config.workspace_id,
        &api.config.workspace_root,
    )))
}

async fn scoped_create_profile_source(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedWorkspacePath>,
    Json(request): Json<CreateWorkspaceProfileSourceRequest>,
) -> ApiResult<Json<crate::profile_settings::ProfileSettingsMutationResponse>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    Ok(Json(crate::profile_settings::create_profile_source(
        &api.config.workspace_id,
        &api.config.workspace_root,
        request,
    )?))
}

async fn scoped_update_profile_registry(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedWorkspacePath>,
    Json(request): Json<UpdateWorkspaceProfileRegistryRequest>,
) -> ApiResult<Json<crate::profile_settings::ProfileSettingsMutationResponse>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    Ok(Json(crate::profile_settings::update_profile_registry(
        &api.config.workspace_id,
        &api.config.workspace_root,
        request,
    )?))
}

async fn scoped_get_profile_source_tree(
    State(api): State<WorkspaceApi>,
    AxumPath((workspace_id, source_tree_id)): AxumPath<(String, String)>,
) -> ApiResult<Json<crate::profile_settings::WorkspaceProfileSourceTreeResponse>> {
    validate_workspace_scope(&api, &workspace_id)?;
    Ok(Json(crate::profile_settings::read_profile_source_tree(
        &workspace_id,
        &api.config.workspace_root,
        &source_tree_id,
    )?))
}

async fn scoped_get_profile_tree_file(
    State(api): State<WorkspaceApi>,
    AxumPath((workspace_id, source_tree_id)): AxumPath<(String, String)>,
    Query(query): Query<ReadWorkspaceProfileTreeFileQuery>,
) -> ApiResult<Json<crate::profile_settings::WorkspaceProfileSourceTreeFileResponse>> {
    validate_workspace_scope(&api, &workspace_id)?;
    Ok(Json(crate::profile_settings::read_profile_tree_file(
        &workspace_id,
        &api.config.workspace_root,
        &source_tree_id,
        query,
    )?))
}

async fn scoped_write_profile_tree_file(
    State(api): State<WorkspaceApi>,
    AxumPath((workspace_id, source_tree_id)): AxumPath<(String, String)>,
    Json(request): Json<WriteWorkspaceProfileTreeFileRequest>,
) -> ApiResult<Json<crate::profile_settings::WorkspaceProfileSourceTreeFileResponse>> {
    validate_workspace_scope(&api, &workspace_id)?;
    Ok(Json(crate::profile_settings::write_profile_tree_file(
        &workspace_id,
        &api.config.workspace_root,
        &source_tree_id,
        request,
    )?))
}

async fn scoped_delete_profile_tree_file(
    State(api): State<WorkspaceApi>,
    AxumPath((workspace_id, source_tree_id)): AxumPath<(String, String)>,
    Json(request): Json<DeleteWorkspaceProfileTreeFileRequest>,
) -> ApiResult<Json<crate::profile_settings::WorkspaceProfileSourceTreeResponse>> {
    validate_workspace_scope(&api, &workspace_id)?;
    Ok(Json(crate::profile_settings::delete_profile_tree_file(
        &workspace_id,
        &api.config.workspace_root,
        &source_tree_id,
        request,
    )?))
}

async fn scoped_get_profile_source(
    State(api): State<WorkspaceApi>,
    AxumPath((workspace_id, profile_source_id)): AxumPath<(String, String)>,
) -> ApiResult<Json<crate::profile_settings::WorkspaceProfileSourceDetailResponse>> {
    validate_workspace_scope(&api, &workspace_id)?;
    Ok(Json(crate::profile_settings::read_profile_source(
        &api.config.workspace_id,
        &api.config.workspace_root,
        &profile_source_id,
    )?))
}

async fn scoped_update_profile_source(
    State(api): State<WorkspaceApi>,
    AxumPath((workspace_id, profile_source_id)): AxumPath<(String, String)>,
    Json(request): Json<UpdateWorkspaceProfileSourceRequest>,
) -> ApiResult<Json<crate::profile_settings::ProfileSettingsMutationResponse>> {
    validate_workspace_scope(&api, &workspace_id)?;
    Ok(Json(crate::profile_settings::update_profile_source(
        &api.config.workspace_id,
        &api.config.workspace_root,
        &profile_source_id,
        request,
    )?))
}

async fn scoped_delete_profile_source(
    State(api): State<WorkspaceApi>,
    AxumPath((workspace_id, profile_source_id)): AxumPath<(String, String)>,
    Json(request): Json<DeleteWorkspaceProfileSourceRequest>,
) -> ApiResult<Json<crate::profile_settings::ProfileSettingsMutationResponse>> {
    validate_workspace_scope(&api, &workspace_id)?;
    Ok(Json(crate::profile_settings::delete_profile_source(
        &api.config.workspace_id,
        &api.config.workspace_root,
        &profile_source_id,
        request,
    )?))
}

async fn scoped_list_tickets(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedWorkspacePath>,
) -> ApiResult<Json<ListResponse<crate::records::TicketSummary>>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    list_tickets(State(api)).await
}

async fn scoped_get_ticket(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedRecordPath>,
) -> ApiResult<Json<TicketDetail>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    get_ticket(State(api), AxumPath(path.id)).await
}

async fn scoped_ticket_backend_operation(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedWorkspacePath>,
    Json(operation): Json<TicketBackendOperation>,
) -> ApiResult<Json<TicketBackendHttpResponse>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    let config = ticket::config::TicketConfig::load_workspace(&api.config.workspace_root)
        .map_err(|error| Error::Config(format!("load Ticket workspace settings: {error}")))?;
    let backend = LocalTicketBackend::new(config.backend_root().to_path_buf())
        .with_record_language(config.ticket_record_language());
    let response = match execute_ticket_backend_operation(&backend, operation) {
        Ok(result) => TicketBackendHttpResponse::Ok { result },
        Err(error) => TicketBackendHttpResponse::Error {
            message: error.to_string(),
        },
    };
    Ok(Json(response))
}

async fn scoped_memory_backend_operation(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedWorkspacePath>,
    Json(operation): Json<MemoryBackendOperation>,
) -> ApiResult<Json<MemoryBackendHttpResponse>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    let memory_config = manifest::MemoryConfig::default();
    let layout = memory::WorkspaceLayout::resolve(&memory_config, &api.config.workspace_root);
    let response = match execute_memory_backend_operation(&layout, operation) {
        Ok(result) => MemoryBackendHttpResponse::Ok { result },
        Err(error) => MemoryBackendHttpResponse::Error {
            message: sanitize_backend_error(&error.to_string()),
        },
    };
    Ok(Json(response))
}

async fn scoped_list_skills(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedWorkspacePath>,
) -> ApiResult<Json<worker::skill::SkillCatalogResponse>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    Ok(Json(skills::catalog(&api.config.workspace_root)))
}

async fn scoped_lint_skills(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedWorkspacePath>,
) -> ApiResult<Json<worker::skill::SkillCatalogResponse>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    Ok(Json(skills::lint(&api.config.workspace_root)))
}

async fn scoped_get_skill(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedSkillPath>,
) -> ApiResult<Json<worker::skill::SkillDetailResponse>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    skills::detail(&api.config.workspace_root, &path.name)
        .map(Json)
        .map_err(skill_api_error)
}

async fn scoped_activate_skill(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedSkillPath>,
) -> ApiResult<Json<worker::skill::SkillActivationResponse>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    skills::activation(&api.config.workspace_root, &path.name)
        .map(Json)
        .map_err(skill_api_error)
}

fn skill_api_error(error: skills::SkillError) -> ApiError {
    match error {
        skills::SkillError::NotFound(name) => ApiError::with_diagnostics(
            Error::RuntimeOperationFailed {
                runtime_id: "workspace".to_string(),
                code: "skill_not_found".to_string(),
                message: format!("unknown Skill `{name}`"),
            },
            vec![RuntimeDiagnostic {
                code: "skill_not_found".to_string(),
                severity: DiagnosticSeverity::Error,
                message: format!("unknown Skill `{name}`"),
            }],
        ),
        skills::SkillError::Io(error) => ApiError::from(Error::Io(error)),
    }
}

async fn scoped_list_objectives(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedWorkspacePath>,
) -> ApiResult<Json<ListResponse<crate::records::ObjectiveSummary>>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    list_objectives(State(api)).await
}

async fn scoped_get_objective(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedRecordPath>,
) -> ApiResult<Json<ObjectiveDetail>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    get_objective(State(api), AxumPath(path.id)).await
}

async fn scoped_list_repositories(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedWorkspacePath>,
) -> ApiResult<Json<RepositoryListResponse>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    list_repositories(State(api)).await
}

async fn scoped_repository_detail(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedRepositoryPath>,
) -> ApiResult<Json<RepositoryDetailResponse>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    repository_detail(State(api), AxumPath(path.repository_id)).await
}

async fn scoped_repository_log(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedRepositoryPath>,
    Query(query): Query<LogQuery>,
) -> ApiResult<Json<RepositoryLogResponse>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    repository_log(State(api), AxumPath(path.repository_id), Query(query)).await
}

async fn scoped_repository_tickets(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedRepositoryPath>,
    Query(query): Query<TicketKanbanQuery>,
) -> ApiResult<Json<RepositoryTicketsResponse>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    repository_tickets(State(api), AxumPath(path.repository_id), Query(query)).await
}

async fn scoped_list_hosts(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedWorkspacePath>,
) -> ApiResult<Json<RuntimeListResponse<HostSummary>>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    list_hosts(State(api)).await
}

async fn scoped_get_profile_source_archive(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedProfileArchivePath>,
    headers: HeaderMap,
) -> ApiResult<(StatusCode, HeaderMap, Vec<u8>)> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    let archive = api
        .resource_broker
        .profile_source_archive(&path.digest)
        .ok_or_else(|| Error::Store("profile source archive not found".to_string()))?;
    let etag = format!("\"profile-source:{}\"", archive.reference.digest);
    if headers
        .get(IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.split(',').any(|candidate| candidate.trim() == etag))
    {
        let mut response_headers = HeaderMap::new();
        response_headers.insert(ETAG, etag.parse().unwrap());
        return Ok((StatusCode::NOT_MODIFIED, response_headers, Vec::new()));
    }
    let mut response_headers = HeaderMap::new();
    response_headers.insert(ETAG, etag.parse().unwrap());
    response_headers.insert(CONTENT_TYPE, "application/x-tar".parse().unwrap());
    Ok((StatusCode::OK, response_headers, archive.content))
}

async fn scoped_list_runtimes(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedWorkspacePath>,
) -> ApiResult<Json<RuntimeListResponse<RuntimeSummary>>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    list_runtimes(State(api)).await
}

async fn scoped_list_workers(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedWorkspacePath>,
) -> ApiResult<Json<RuntimeListResponse<WorkerSummary>>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    list_workers(State(api)).await
}

async fn scoped_create_workspace_worker(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedWorkspacePath>,
    Json(request): Json<BrowserCreateWorkerRequest>,
) -> ApiResult<Json<BrowserCreateWorkerResponse>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    create_workspace_worker(State(api), Json(request)).await
}

async fn scoped_get_worker_launch_options(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedWorkspacePath>,
) -> ApiResult<Json<WorkerLaunchOptionsResponse>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    get_worker_launch_options(State(api)).await
}

async fn scoped_list_runtime_working_directories(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedRuntimePath>,
) -> ApiResult<Json<BrowserWorkingDirectoryListResponse>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    let (items, diagnostics) = runtime_working_directory_summaries(&api, &path.runtime_id)?;
    Ok(Json(BrowserWorkingDirectoryListResponse {
        workspace_id: api.config.workspace_id.clone(),
        items,
        diagnostics,
    }))
}

async fn scoped_create_runtime_working_directory(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedRuntimePath>,
    Json(mut request): Json<BrowserWorkingDirectoryCreateRequest>,
) -> ApiResult<Json<BrowserWorkingDirectoryDetailResponse>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    request.runtime_id = path.runtime_id;
    create_working_directory_for_runtime(api, request)
}

async fn scoped_runtime_working_directory_detail(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedRuntimeWorkingDirectoryPath>,
) -> ApiResult<Json<BrowserWorkingDirectoryDetailResponse>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    working_directory_detail_for_runtime(api, &path.runtime_id, &path.working_directory_id)
}

async fn scoped_cleanup_runtime_working_directory(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedRuntimeWorkingDirectoryPath>,
) -> ApiResult<Json<BrowserWorkingDirectoryDetailResponse>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    cleanup_working_directory_for_runtime(api, &path.runtime_id, &path.working_directory_id)
}

async fn scoped_list_working_directories(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedWorkspacePath>,
) -> ApiResult<Json<BrowserWorkingDirectoryListResponse>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    let items = working_directory_summaries(&api)?;
    Ok(Json(BrowserWorkingDirectoryListResponse {
        workspace_id: api.config.workspace_id.clone(),
        items,
        diagnostics: Vec::new(),
    }))
}

async fn scoped_create_working_directory(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedWorkspacePath>,
    Json(_request): Json<BrowserWorkingDirectoryCreateRequest>,
) -> ApiResult<Json<BrowserWorkingDirectoryDetailResponse>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    Err(ApiError::with_diagnostics(
        Error::RuntimeOperationFailed {
            runtime_id: "workspace-backend".to_string(),
            code: "backend_workdir_create_unsupported".to_string(),
            message: "Working directory creation must be scoped to a concrete Runtime".to_string(),
        },
        vec![RuntimeDiagnostic {
            code: "backend_workdir_create_unsupported".to_string(),
            severity: DiagnosticSeverity::Error,
            message: "Use runtime-scoped Worker creation or a runtime-scoped working-directory API; the backend does not own workdir lifecycle.".to_string(),
        }],
    ))
}

async fn scoped_working_directory_detail(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedWorkingDirectoryPath>,
) -> ApiResult<Json<BrowserWorkingDirectoryDetailResponse>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    working_directory_detail_for_runtime(
        api,
        EMBEDDED_WORKER_RUNTIME_ID,
        &path.working_directory_id,
    )
}

async fn scoped_cleanup_working_directory(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedWorkingDirectoryPath>,
) -> ApiResult<Json<BrowserWorkingDirectoryDetailResponse>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    cleanup_working_directory_for_runtime(
        api,
        EMBEDDED_WORKER_RUNTIME_ID,
        &path.working_directory_id,
    )
}

fn create_working_directory_for_runtime(
    api: WorkspaceApi,
    request: BrowserWorkingDirectoryCreateRequest,
) -> ApiResult<Json<BrowserWorkingDirectoryDetailResponse>> {
    let runtime_id = request.runtime_id.clone();
    let mut working_directory_request = working_directory_request_for_browser(&api, request)?;
    let workdir_id = next_backend_workdir_id(&working_directory_request.repository.id);
    working_directory_request.backend_workdir_id = Some(workdir_id.clone());
    let pending = WorkdirRegistryRecord {
        workspace_id: api.config.workspace_id.clone(),
        workdir_id: workdir_id.clone(),
        runtime_id: runtime_id.clone(),
        repository_id: working_directory_request.repository.id.clone(),
        selector: working_directory_request
            .repository
            .selector
            .as_ref()
            .map(|selector| selector.as_ref().to_string()),
        resolved_commit: None,
        materialization_status: "pending".to_string(),
        cleanliness: "unknown".to_string(),
        created_at: now_registry_timestamp(),
        updated_at: now_registry_timestamp(),
    };
    api.store.upsert_workdir_registry(&pending)?;
    let result = api
        .runtime
        .create_working_directory(&runtime_id, working_directory_request)
        .map_err(|err| err.into_error())?;
    let Some(working_directory) = result.working_directory else {
        let mut failed = pending;
        failed.materialization_status = "failed".to_string();
        failed.updated_at = now_registry_timestamp();
        api.store.upsert_workdir_registry(&failed)?;
        return Err(ApiError::with_diagnostics(
            Error::RuntimeOperationFailed {
                runtime_id,
                code: "workspace_working_directory_create_failed".to_string(),
                message: "Runtime did not create working directory".to_string(),
            },
            result.diagnostics,
        ));
    };
    let record = workdir_record_from_summary(&api, &runtime_id, &working_directory.summary);
    api.store.upsert_workdir_registry(&record)?;
    Ok(Json(BrowserWorkingDirectoryDetailResponse {
        workspace_id: api.config.workspace_id.clone(),
        item: working_directory.summary,
        diagnostics: result.diagnostics,
    }))
}

fn working_directory_detail_for_runtime(
    api: WorkspaceApi,
    runtime_id: &str,
    working_directory_id: &str,
) -> ApiResult<Json<BrowserWorkingDirectoryDetailResponse>> {
    let result = api
        .runtime
        .working_directory(runtime_id, working_directory_id)
        .map_err(|err| err.into_error())?;
    if let Some(working_directory) = result.working_directory {
        let record = workdir_record_from_summary(&api, runtime_id, &working_directory.summary);
        api.store.upsert_workdir_registry(&record)?;
        return Ok(Json(BrowserWorkingDirectoryDetailResponse {
            workspace_id: api.config.workspace_id.clone(),
            item: working_directory.summary,
            diagnostics: result.diagnostics,
        }));
    }
    if let Some(record) = api
        .store
        .get_workdir_registry(&api.config.workspace_id, working_directory_id)?
    {
        return Ok(Json(BrowserWorkingDirectoryDetailResponse {
            workspace_id: api.config.workspace_id.clone(),
            item: workdir_summary_from_record(&record),
            diagnostics: result.diagnostics,
        }));
    }
    Err(ApiError::with_diagnostics(
        Error::RuntimeOperationFailed {
            runtime_id: runtime_id.to_string(),
            code: "workspace_working_directory_lookup_failed".to_string(),
            message: "Runtime did not return working directory".to_string(),
        },
        result.diagnostics,
    ))
}

fn cleanup_working_directory_for_runtime(
    api: WorkspaceApi,
    runtime_id: &str,
    working_directory_id: &str,
) -> ApiResult<Json<BrowserWorkingDirectoryDetailResponse>> {
    if let Some(candidate) = build_runtime_cleanup_plan(&api, runtime_id)?
        .workdirs
        .into_iter()
        .find(|candidate| candidate.workdir_id == working_directory_id)
    {
        if let Some(reason) = candidate.blocking_reason {
            return Err(cleanup_api_error(
                runtime_id,
                "workspace_cleanup_workdir_blocked",
                &reason,
            ));
        }
        if candidate.action == CleanupTargetKind::WorkdirDirtyDiscard {
            return Err(cleanup_api_error(
                runtime_id,
                "workspace_cleanup_dirty_confirmation_required",
                "dirty Workdir discard requires the cleanup execution API with explicit confirmation",
            ));
        }
    }
    let result = api
        .runtime
        .cleanup_working_directory(runtime_id, working_directory_id)
        .map_err(|err| err.into_error())?;
    let Some(working_directory) = result.working_directory else {
        return Err(ApiError::with_diagnostics(
            Error::RuntimeOperationFailed {
                runtime_id: runtime_id.to_string(),
                code: "workspace_working_directory_cleanup_failed".to_string(),
                message: "Runtime did not cleanup working directory".to_string(),
            },
            result.diagnostics,
        ));
    };
    let record = workdir_record_from_summary(&api, runtime_id, &working_directory.summary);
    api.store.upsert_workdir_registry(&record)?;
    Ok(Json(BrowserWorkingDirectoryDetailResponse {
        workspace_id: api.config.workspace_id.clone(),
        item: working_directory.summary,
        diagnostics: result.diagnostics,
    }))
}

async fn set_worker_retention(
    api: WorkspaceApi,
    runtime_id: String,
    runtime_worker_id: String,
    pinned: bool,
) -> ApiResult<Json<WorkerRetentionResponse>> {
    let runtime_worker_registry_id = parse_runtime_worker_id_for_registry(&runtime_worker_id)?;
    if api
        .store
        .get_worker_registry(
            &api.config.workspace_id,
            runtime_id.as_str(),
            runtime_worker_registry_id,
        )?
        .is_none()
    {
        if let Ok(worker) = api
            .runtime
            .worker(runtime_id.as_str(), runtime_worker_id.as_str())
        {
            let _ = sync_worker_observation(&api, &worker);
        }
    }
    let retention_state = if pinned { "pinned" } else { "normal" };
    let changed = api.store.update_worker_retention(
        &api.config.workspace_id,
        runtime_id.as_str(),
        runtime_worker_registry_id,
        retention_state,
        now_registry_timestamp().as_str(),
    )?;
    if !changed {
        return Err(cleanup_api_error(
            runtime_id.as_str(),
            "workspace_worker_retention_unknown_worker",
            "Worker is not known to the Backend registry",
        ));
    }
    Ok(Json(WorkerRetentionResponse {
        workspace_id: api.config.workspace_id,
        runtime_id,
        worker_id: runtime_worker_id,
        pinned,
        retention_state: retention_state.to_string(),
    }))
}

fn build_runtime_cleanup_plan(
    api: &WorkspaceApi,
    runtime_id: &str,
) -> ApiResult<RuntimeCleanupPlanResponse> {
    let workers = workers_response(api.clone())?;
    let live_running_worker_ids: HashSet<(String, u64)> = workers
        .items
        .iter()
        .filter(|worker| worker.state == "running")
        .filter_map(|worker| {
            parse_runtime_worker_id_for_registry(worker.worker_id.as_str())
                .ok()
                .map(|worker_id| (worker.runtime_id.clone(), worker_id))
        })
        .collect();
    let (workdir_summaries, mut diagnostics) =
        match runtime_working_directory_summaries(api, runtime_id) {
            Ok(result) => result,
            Err(error) => {
                let mut diagnostics = error.diagnostics;
                if diagnostics.is_empty() {
                    diagnostics.push(RuntimeDiagnostic {
                        code: "workspace_cleanup_runtime_observation_unavailable".to_string(),
                        severity: DiagnosticSeverity::Warning,
                        message: sanitize_backend_error(&error.error.to_string()),
                    });
                }
                (Vec::new(), diagnostics)
            }
        };
    let workdir_records = api
        .store
        .list_workdir_registry(&api.config.workspace_id, 500)?;
    let worker_records = api
        .store
        .list_worker_registry(&api.config.workspace_id, 500)?;
    let worker_by_id: HashMap<_, _> = worker_records
        .iter()
        .map(|record| {
            (
                (record.runtime_id.clone(), record.runtime_worker_id.clone()),
                record.clone(),
            )
        })
        .collect();
    let observed_workdirs: HashMap<_, _> = workdir_summaries
        .into_iter()
        .map(|summary| (summary.working_directory_id.clone(), summary))
        .collect();

    let mut worker_candidates = Vec::new();
    for record in worker_records
        .iter()
        .filter(|record| record.runtime_id == runtime_id)
    {
        let links = api.store.list_worker_workdir_links(
            &api.config.workspace_id,
            record.runtime_id.as_str(),
            record.runtime_worker_id,
        )?;
        let is_running = live_running_worker_ids
            .contains(&(record.runtime_id.clone(), record.runtime_worker_id.clone()));
        let pinned = record.retention_state == "pinned";
        let blocking_reason = if pinned {
            Some("worker is pinned".to_string())
        } else if is_running {
            Some("worker is running".to_string())
        } else {
            None
        };
        worker_candidates.push(CleanupWorkerCandidate {
            target_id: format!(
                "worker:{}:{}",
                encode_path_segment(record.runtime_id.as_str()),
                encode_path_segment(&record.runtime_worker_id.to_string())
            ),
            action: CleanupTargetKind::WorkerDelete,
            worker_id: record.runtime_worker_id.to_string(),
            runtime_worker_id: record.runtime_worker_id.to_string(),
            runtime_id: record.runtime_id.clone(),
            reason: if blocking_reason.is_some() {
                "Worker cannot be deleted until blocking conditions are cleared".to_string()
            } else {
                "Stopped or missing Worker can be manually deleted".to_string()
            },
            blocking_reason,
            pinned,
            retention_state: record.retention_state.clone(),
            linked_workdir_ids: links.iter().map(|link| link.workdir_id.clone()).collect(),
            running_linked: is_running,
            estimated_reclaim_bytes: None,
        });
    }

    let mut workdir_candidates = Vec::new();
    for record in workdir_records
        .iter()
        .filter(|record| record.runtime_id == runtime_id)
    {
        let links = api
            .store
            .list_workdir_worker_links(&api.config.workspace_id, record.workdir_id.as_str())?;
        let linked_workers = links
            .iter()
            .filter_map(|link| {
                worker_by_id.get(&(link.runtime_id.clone(), link.runtime_worker_id.clone()))
            })
            .collect::<Vec<_>>();
        let linked_worker_ids = links
            .iter()
            .map(|link| link.runtime_worker_id.to_string())
            .collect::<Vec<_>>();
        let linked_running_worker_ids = linked_workers
            .iter()
            .filter(|worker| {
                live_running_worker_ids
                    .contains(&(worker.runtime_id.clone(), worker.runtime_worker_id.clone()))
            })
            .map(|worker| worker.runtime_worker_id.to_string())
            .collect::<Vec<_>>();
        let pinned_linked = linked_workers
            .iter()
            .any(|worker| worker.retention_state == "pinned");
        let running_linked = !linked_running_worker_ids.is_empty();
        let observed_status = observed_workdirs
            .get(record.workdir_id.as_str())
            .map(|summary| CleanupWorkdirFileStatus::from_runtime(&summary.status));
        let file_status = observed_status.unwrap_or_else(|| {
            CleanupWorkdirFileStatus::from_registry(&record.materialization_status)
        });
        let cleanliness = observed_workdirs
            .get(record.workdir_id.as_str())
            .map(|summary| CleanupWorkdirCleanliness::from_runtime(summary.cleanliness.as_deref()))
            .unwrap_or_else(|| CleanupWorkdirCleanliness::from_registry(&record.cleanliness));
        let action = if file_status.is_record_only() {
            CleanupTargetKind::WorkdirRecordDelete
        } else if file_status.is_corrupted() || cleanliness.is_clean() {
            CleanupTargetKind::WorkdirCleanCleanup
        } else {
            CleanupTargetKind::WorkdirDirtyDiscard
        };
        let blocking_reason = if running_linked {
            Some("workdir is linked to a running Worker".to_string())
        } else if pinned_linked {
            Some("workdir is linked to a pinned Worker/history".to_string())
        } else {
            None
        };
        workdir_candidates.push(CleanupWorkdirCandidate {
            target_id: format!("workdir:{}", record.workdir_id),
            action,
            workdir_id: record.workdir_id.clone(),
            runtime_id: record.runtime_id.clone(),
            repository_id: record.repository_id.clone(),
            reason: if blocking_reason.is_some() {
                "Workdir cleanup is blocked until linked Worker state is safe".to_string()
            } else if file_status.is_record_only() {
                "Not-found Workdir record can be deleted from the Backend registry".to_string()
            } else if file_status.is_corrupted() {
                "Corrupted Workdir can be deleted from Runtime storage and Backend registry"
                    .to_string()
            } else if matches!(cleanliness, CleanupWorkdirCleanliness::Dirty) {
                "Dirty Workdir requires explicit discard confirmation before cleanup".to_string()
            } else if matches!(cleanliness, CleanupWorkdirCleanliness::Unknown) {
                "Workdir clean state is unknown; explicit discard confirmation is required"
                    .to_string()
            } else {
                "Clean Workdir can be manually cleaned up".to_string()
            },
            blocking_reason,
            linked_worker_ids,
            linked_running_worker_ids,
            running_linked,
            pinned_linked,
            file_status,
            cleanliness,
            estimated_reclaim_bytes: None,
        });
    }
    worker_candidates.sort_by(|left, right| left.target_id.cmp(&right.target_id));
    workdir_candidates.sort_by(|left, right| left.target_id.cmp(&right.target_id));
    diagnostics.truncate(16);
    let generated_at = now_registry_timestamp();
    let digest = cleanup_plan_digest(&worker_candidates, &workdir_candidates)?;
    Ok(RuntimeCleanupPlanResponse {
        workspace_id: api.config.workspace_id.clone(),
        runtime_id: runtime_id.to_string(),
        generated_at,
        revision: digest.clone(),
        digest,
        workers: worker_candidates,
        workdirs: workdir_candidates,
        diagnostics,
    })
}

fn cleanup_plan_digest(
    workers: &[CleanupWorkerCandidate],
    workdirs: &[CleanupWorkdirCandidate],
) -> ApiResult<String> {
    let bytes = serde_json::to_vec(&(workers, workdirs)).map_err(|error| {
        cleanup_api_error(
            "backend",
            "workspace_cleanup_plan_digest_failed",
            &format!("failed to serialize cleanup plan: {error}"),
        )
    })?;
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let bytes = hasher.finalize();
    let digest = bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok(format!("sha256:{digest}"))
}

fn execute_runtime_cleanup(
    api: &WorkspaceApi,
    runtime_id: &str,
    request: ExecuteRuntimeCleanupRequest,
) -> ApiResult<RuntimeCleanupExecutionResponse> {
    let plan = build_runtime_cleanup_plan(api, runtime_id)?;
    if request.expected_plan_revision != plan.revision
        || request.expected_plan_digest != plan.digest
    {
        return Err(cleanup_api_error(
            runtime_id,
            "workspace_cleanup_plan_stale",
            "cleanup plan revision/digest is stale; refresh the preview before executing",
        ));
    }
    let worker_targets: HashSet<_> = request.worker_target_ids.iter().cloned().collect();
    let workdir_targets: HashSet<_> = request.workdir_target_ids.iter().cloned().collect();
    let dirty_confirmations: HashSet<_> = request
        .confirm_dirty_discard_target_ids
        .iter()
        .cloned()
        .collect();
    let mut results = Vec::new();

    for candidate in plan
        .workers
        .iter()
        .filter(|candidate| worker_targets.contains(candidate.target_id.as_str()))
    {
        if let Some(reason) = &candidate.blocking_reason {
            return Err(cleanup_api_error(
                runtime_id,
                "workspace_cleanup_worker_blocked",
                reason,
            ));
        }
        if candidate.pinned {
            return Err(cleanup_api_error(
                runtime_id,
                "workspace_cleanup_worker_pinned",
                "pinned Worker/history cannot be deleted",
            ));
        }
        let runtime_worker_id = parse_runtime_worker_id_for_registry(&candidate.runtime_worker_id)?;
        cleanup_runtime_worker_for_execution(api, runtime_id, candidate)?;
        api.store.delete_worker_registry(
            &api.config.workspace_id,
            candidate.runtime_id.as_str(),
            runtime_worker_id,
        )?;
        results.push(RuntimeCleanupExecutionResult {
            target_id: candidate.target_id.clone(),
            action: candidate.action.clone(),
            status: "deleted".to_string(),
            message: "Worker deleted from Runtime and Backend registry".to_string(),
        });
    }

    for candidate in plan
        .workdirs
        .iter()
        .filter(|candidate| workdir_targets.contains(candidate.target_id.as_str()))
    {
        if let Some(reason) = &candidate.blocking_reason {
            return Err(cleanup_api_error(
                runtime_id,
                "workspace_cleanup_workdir_blocked",
                reason,
            ));
        }
        match candidate.action {
            CleanupTargetKind::WorkdirDirtyDiscard => {
                if !dirty_confirmations.contains(candidate.target_id.as_str()) {
                    return Err(cleanup_api_error(
                        runtime_id,
                        "workspace_cleanup_dirty_confirmation_required",
                        "dirty Workdir discard requires explicit confirmation",
                    ));
                }
                cleanup_runtime_workdir_for_execution(api, runtime_id, candidate)?;
                let deleted = api.store.delete_workdir_registry(
                    &api.config.workspace_id,
                    candidate.workdir_id.as_str(),
                )?;
                if !deleted {
                    return Err(cleanup_api_error(
                        runtime_id,
                        "workspace_cleanup_workdir_registry_not_found",
                        "Backend Workdir registry row was not found after Runtime cleanup",
                    ));
                }
                results.push(RuntimeCleanupExecutionResult {
                    target_id: candidate.target_id.clone(),
                    action: candidate.action.clone(),
                    status: "deleted".to_string(),
                    message:
                        "Dirty/unknown Workdir was deleted from Runtime storage and Backend registry after explicit confirmation"
                            .to_string(),
                });
            }
            CleanupTargetKind::WorkdirCleanCleanup => {
                cleanup_runtime_workdir_for_execution(api, runtime_id, candidate)?;
                let deleted = api.store.delete_workdir_registry(
                    &api.config.workspace_id,
                    candidate.workdir_id.as_str(),
                )?;
                if !deleted {
                    return Err(cleanup_api_error(
                        runtime_id,
                        "workspace_cleanup_workdir_registry_not_found",
                        "Backend Workdir registry row was not found after Runtime cleanup",
                    ));
                }
                results.push(RuntimeCleanupExecutionResult {
                    target_id: candidate.target_id.clone(),
                    action: candidate.action.clone(),
                    status: "deleted".to_string(),
                    message: "Workdir deleted from Runtime storage and Backend registry"
                        .to_string(),
                });
            }
            CleanupTargetKind::WorkdirRecordDelete => {
                let deleted = api.store.delete_workdir_registry(
                    &api.config.workspace_id,
                    candidate.workdir_id.as_str(),
                )?;
                if !deleted {
                    return Err(cleanup_api_error(
                        runtime_id,
                        "workspace_cleanup_workdir_registry_not_found",
                        "Backend Workdir registry row was not found",
                    ));
                }
                results.push(RuntimeCleanupExecutionResult {
                    target_id: candidate.target_id.clone(),
                    action: candidate.action.clone(),
                    status: "deleted".to_string(),
                    message: "Not-found Workdir registry row deleted".to_string(),
                });
            }
            CleanupTargetKind::WorkerDelete => {
                return Err(cleanup_api_error(
                    runtime_id,
                    "workspace_cleanup_invalid_target_kind",
                    "worker delete action cannot be executed as a Workdir target",
                ));
            }
        }
    }

    let requested_target_count = worker_targets.len() + workdir_targets.len();
    if requested_target_count > 0 && results.len() != requested_target_count {
        return Err(cleanup_api_error(
            runtime_id,
            "workspace_cleanup_target_not_executed",
            "one or more selected cleanup targets were not present in the current cleanup plan",
        ));
    }

    let plan_after = build_runtime_cleanup_plan(api, runtime_id)?;
    let executed_at = now_registry_timestamp();
    Ok(RuntimeCleanupExecutionResponse {
        workspace_id: api.config.workspace_id.clone(),
        runtime_id: runtime_id.to_string(),
        executed_at,
        results,
        diagnostics: plan_after.diagnostics.clone(),
        plan_after,
    })
}

fn cleanup_runtime_worker_for_execution(
    api: &WorkspaceApi,
    runtime_id: &str,
    candidate: &CleanupWorkerCandidate,
) -> ApiResult<()> {
    match api.runtime.stop_worker(
        runtime_id,
        candidate.runtime_worker_id.as_str(),
        WorkerLifecycleRequest {
            reason: Some("cleanup worker before deletion".to_string()),
        },
    ) {
        Ok(result) if result.state == WorkerOperationState::Accepted => {}
        Ok(result) => {
            return Err(ApiError::with_diagnostics(
                Error::RuntimeOperationFailed {
                    runtime_id: runtime_id.to_string(),
                    code: "workspace_cleanup_worker_runtime_stop_rejected".to_string(),
                    message: "Runtime did not stop selected Worker before deletion".to_string(),
                },
                result.diagnostics,
            ));
        }
        Err(RuntimeRegistryError::UnknownWorker { .. }) => return Ok(()),
        Err(error) => return Err(error.into_error().into()),
    }

    match api
        .runtime
        .delete_worker(runtime_id, candidate.runtime_worker_id.as_str())
    {
        Ok(result) if result.deleted && result.state == WorkerOperationState::Accepted => Ok(()),
        Ok(result) => Err(ApiError::with_diagnostics(
            Error::RuntimeOperationFailed {
                runtime_id: runtime_id.to_string(),
                code: "workspace_cleanup_worker_runtime_delete_rejected".to_string(),
                message: "Runtime did not delete selected Worker after stopping it".to_string(),
            },
            result.diagnostics,
        )),
        Err(RuntimeRegistryError::UnknownWorker { .. }) => Ok(()),
        Err(error) => Err(error.into_error().into()),
    }
}

fn cleanup_runtime_workdir_for_execution(
    api: &WorkspaceApi,
    runtime_id: &str,
    candidate: &CleanupWorkdirCandidate,
) -> ApiResult<()> {
    let result = api
        .runtime
        .cleanup_working_directory(runtime_id, candidate.workdir_id.as_str())
        .map_err(|err| err.into_error())?;
    if result.working_directory.is_none() {
        return Err(ApiError::with_diagnostics(
            Error::RuntimeOperationFailed {
                runtime_id: runtime_id.to_string(),
                code: "workspace_cleanup_workdir_runtime_failed".to_string(),
                message: "Runtime did not cleanup selected Workdir".to_string(),
            },
            result.diagnostics,
        ));
    };
    Ok(())
}

fn cleanup_api_error(runtime_id: &str, code: &str, message: &str) -> ApiError {
    Error::RuntimeOperationFailed {
        runtime_id: runtime_id.to_string(),
        code: code.to_string(),
        message: message.to_string(),
    }
    .into()
}

async fn scoped_get_runtime_connection_settings(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedWorkspacePath>,
) -> ApiResult<Json<RuntimeConnectionSettingsResponse>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    get_runtime_connection_settings(State(api)).await
}

async fn scoped_add_remote_runtime_connection(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedWorkspacePath>,
    Json(request): Json<AddRemoteRuntimeConnectionRequest>,
) -> ApiResult<Json<RuntimeConnectionMutationResponse>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    add_remote_runtime_connection(State(api), Json(request)).await
}

async fn scoped_delete_remote_runtime_connection(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedRuntimePath>,
) -> ApiResult<Json<RuntimeConnectionMutationResponse>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    delete_remote_runtime_connection(State(api), AxumPath(path.runtime_id)).await
}

async fn scoped_test_remote_runtime_connection(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedRuntimePath>,
) -> ApiResult<Json<RemoteRuntimeTestResponse>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    test_remote_runtime_connection(State(api), AxumPath(path.runtime_id)).await
}

async fn scoped_get_companion_status(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedWorkspacePath>,
) -> ApiResult<Json<CompanionStatusResponse>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    get_companion_status(State(api)).await
}

async fn scoped_get_companion_transcript(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedWorkspacePath>,
    Query(query): Query<TranscriptQuery>,
) -> ApiResult<Json<CompanionTranscriptProjection>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    get_companion_transcript(State(api), Query(query)).await
}

async fn scoped_post_companion_message(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedWorkspacePath>,
    Json(request): Json<CompanionMessageRequest>,
) -> ApiResult<Json<CompanionMessageResponse>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    post_companion_message(State(api), Json(request)).await
}

async fn scoped_post_companion_cancel(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedWorkspacePath>,
    Json(request): Json<CompanionCancelRequest>,
) -> ApiResult<Json<CompanionMessageResponse>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    post_companion_cancel(State(api), Json(request)).await
}

async fn scoped_list_runtime_workers(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedRuntimePath>,
) -> ApiResult<Json<RuntimeListResponse<WorkerSummary>>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    list_runtime_workers(State(api), AxumPath(path.runtime_id)).await
}

async fn scoped_create_runtime_worker(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedRuntimePath>,
    Json(request): Json<WorkerSpawnRequest>,
) -> ApiResult<Json<WorkerSpawnResult>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    create_runtime_worker(State(api), AxumPath(path.runtime_id), Json(request)).await
}

async fn scoped_sync_runtime_config_bundle(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedRuntimePath>,
    Json(request): Json<RuntimeConfigBundleSyncRequest>,
) -> ApiResult<Json<ConfigBundleSyncResult>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    sync_runtime_config_bundle(State(api), AxumPath(path.runtime_id), Json(request)).await
}

async fn scoped_check_runtime_config_bundle(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedConfigBundlePath>,
    Query(query): Query<RuntimeConfigBundleAvailabilityQuery>,
) -> ApiResult<Json<ConfigBundleCheckResult>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    check_runtime_config_bundle(
        State(api),
        AxumPath((path.runtime_id, path.bundle_id)),
        Query(query),
    )
    .await
}

async fn scoped_get_runtime_worker(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedRuntimeWorkerPath>,
) -> ApiResult<Json<WorkerSummary>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    get_runtime_worker(State(api), AxumPath((path.runtime_id, path.worker_id))).await
}

async fn scoped_pin_runtime_worker(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedRuntimeWorkerPath>,
) -> ApiResult<Json<WorkerRetentionResponse>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    set_worker_retention(api, path.runtime_id, path.worker_id, true).await
}

async fn scoped_unpin_runtime_worker(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedRuntimeWorkerPath>,
) -> ApiResult<Json<WorkerRetentionResponse>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    set_worker_retention(api, path.runtime_id, path.worker_id, false).await
}

async fn scoped_runtime_cleanup_plan(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedRuntimePath>,
) -> ApiResult<Json<RuntimeCleanupPlanResponse>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    let plan = build_runtime_cleanup_plan(&api, path.runtime_id.as_str())?;
    Ok(Json(plan))
}

async fn scoped_execute_runtime_cleanup(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedRuntimePath>,
    Json(request): Json<ExecuteRuntimeCleanupRequest>,
) -> ApiResult<Json<RuntimeCleanupExecutionResponse>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    let response = execute_runtime_cleanup(&api, path.runtime_id.as_str(), request)?;
    Ok(Json(response))
}

async fn scoped_send_runtime_worker_input(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedRuntimeWorkerPath>,
    Json(request): Json<WorkerInputRequest>,
) -> ApiResult<Json<WorkerInputResult>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    send_runtime_worker_input(
        State(api),
        AxumPath((path.runtime_id, path.worker_id)),
        Json(request),
    )
    .await
}

async fn scoped_runtime_worker_completions(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedRuntimeWorkerPath>,
    Json(request): Json<WorkerCompletionsRequest>,
) -> ApiResult<Json<WorkerCompletionsResult>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    runtime_worker_completions(
        State(api),
        AxumPath((path.runtime_id, path.worker_id)),
        Json(request),
    )
    .await
}

async fn scoped_stop_runtime_worker(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedRuntimeWorkerPath>,
    Json(request): Json<WorkerLifecycleRequest>,
) -> ApiResult<Json<WorkerLifecycleResult>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    stop_runtime_worker(
        State(api),
        AxumPath((path.runtime_id, path.worker_id)),
        Json(request),
    )
    .await
}

async fn scoped_cancel_runtime_worker(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedRuntimeWorkerPath>,
    Json(request): Json<WorkerLifecycleRequest>,
) -> ApiResult<Json<WorkerLifecycleResult>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    cancel_runtime_worker(
        State(api),
        AxumPath((path.runtime_id, path.worker_id)),
        Json(request),
    )
    .await
}

async fn scoped_worker_protocol_ws(
    ws: WebSocketUpgrade,
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedRuntimeWorkerPath>,
) -> Response {
    if let Err(err) = validate_workspace_scope(&api, &path.workspace_id) {
        return err.into_response();
    }
    worker_protocol_ws(State(api), AxumPath((path.runtime_id, path.worker_id)), ws)
        .await
        .into_response()
}

async fn scoped_list_host_workers(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedHostPath>,
) -> ApiResult<Json<RuntimeListResponse<WorkerSummary>>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    list_host_workers(State(api), AxumPath(path.host_id)).await
}

async fn get_workspace(State(api): State<WorkspaceApi>) -> ApiResult<Json<WorkspaceResponse>> {
    let schema_version = api.store.schema_version().await?;
    let stored = api.store.get_workspace(api.workspace_id()).await?;
    let display_name = stored
        .as_ref()
        .map(|record| record.display_name.clone())
        .unwrap_or_else(|| api.config.workspace_display_name.clone());
    let companion_status = api.companion.status();
    let companion_console = companion_console_extension_point(&companion_status);
    Ok(Json(WorkspaceResponse {
        workspace_id: api.config.workspace_id.clone(),
        display_name,
        record_authority: "local_yoi_project_records".to_string(),
        schema_version,
        auth: api.config.auth.clone(),
        extension_points: ExtensionPoints {
            store: "sqlite".to_string(),
            event_stream: ExtensionPointState {
                status: "backend_proxy".to_string(),
                note: "Worker observation streams are exposed only through the Workspace server proxy keyed by runtime_id + worker_id; browser clients never receive raw Runtime endpoints or socket paths.".to_string(),
                diagnostics: Vec::new(),
            },
            host_worker_bridge: ExtensionPointState {
                status: "runtime_registry".to_string(),
                note: "Hosts and Workers are projected from the Workspace RuntimeRegistry; raw Runtime endpoints, sockets, and local metadata paths are not exposed.".to_string(),
                diagnostics: Vec::new(),
            },
            companion_console,
        },
    }))
}

fn companion_console_extension_point(status: &CompanionStatusResponse) -> ExtensionPointState {
    let completion = status.transport.completion.clone();
    let note = match completion.as_str() {
        "connected" => "Workspace Companion is input-capable and browser input is dispatched through the normal Worker runtime path.".to_string(),
        "not_input_capable" => {
            let diagnostic_codes = status
                .diagnostics
                .iter()
                .map(|diagnostic| diagnostic.code.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            if diagnostic_codes.is_empty() {
                "Workspace Companion is not input-capable; check provider, config, profile, secret, and authority diagnostics.".to_string()
            } else {
                format!(
                    "Workspace Companion is not input-capable; check typed diagnostics: {diagnostic_codes}."
                )
            }
        }
        "disabled" => "Workspace Companion auto-start has been removed; create an explicit Worker instead.".to_string(),
        other => format!(
            "Workspace Companion transport reports {other}; browser input follows the Companion Worker runtime capability state."
        ),
    };
    ExtensionPointState {
        status: completion,
        note,
        diagnostics: status.diagnostics.clone(),
    }
}

async fn list_tickets(
    State(api): State<WorkspaceApi>,
) -> ApiResult<Json<ListResponse<crate::records::TicketSummary>>> {
    let limit = api.config.max_records.min(200);
    let ProjectRecordList {
        items,
        invalid_records,
        record_authority,
    } = api.records.list_tickets(limit)?;
    Ok(Json(ListResponse {
        workspace_id: api.config.workspace_id,
        limit,
        items,
        invalid_records,
        record_authority,
    }))
}

async fn get_ticket(
    State(api): State<WorkspaceApi>,
    AxumPath(id): AxumPath<String>,
) -> ApiResult<Json<TicketDetail>> {
    Ok(Json(api.records.ticket(&id)?))
}

async fn list_objectives(
    State(api): State<WorkspaceApi>,
) -> ApiResult<Json<ListResponse<crate::records::ObjectiveSummary>>> {
    let limit = api.config.max_records.min(200);
    let ProjectRecordList {
        items,
        invalid_records,
        record_authority,
    } = api.records.list_objectives(limit)?;
    Ok(Json(ListResponse {
        workspace_id: api.config.workspace_id,
        limit,
        items,
        invalid_records,
        record_authority,
    }))
}

async fn get_objective(
    State(api): State<WorkspaceApi>,
    AxumPath(id): AxumPath<String>,
) -> ApiResult<Json<ObjectiveDetail>> {
    Ok(Json(api.records.objective(&id)?))
}

async fn list_repositories(
    State(api): State<WorkspaceApi>,
) -> ApiResult<Json<RepositoryListResponse>> {
    let RepositoryListProjection { items, diagnostics } = api.repository_reader().list();
    Ok(Json(RepositoryListResponse {
        workspace_id: api.config.workspace_id,
        items,
        source: "workspace_backend_config".to_string(),
        diagnostics: repository_diagnostics(diagnostics),
    }))
}

async fn repository_detail(
    State(api): State<WorkspaceApi>,
    AxumPath(repository_id): AxumPath<String>,
) -> ApiResult<Json<RepositoryDetailResponse>> {
    let item = repository_lookup(api.repository_reader().summary(&repository_id))?;
    Ok(Json(RepositoryDetailResponse {
        workspace_id: api.config.workspace_id.clone(),
        item,
        source: "workspace_backend_config".to_string(),
    }))
}

async fn repository_log(
    State(api): State<WorkspaceApi>,
    AxumPath(repository_id): AxumPath<String>,
    Query(query): Query<LogQuery>,
) -> ApiResult<Json<RepositoryLogResponse>> {
    let RepositoryLogRead {
        repository_id,
        default_selector,
        limit,
        commits,
        diagnostics,
    } = repository_lookup(
        api.repository_reader()
            .recent_log(&repository_id, query.limit),
    )?;
    Ok(Json(RepositoryLogResponse {
        workspace_id: api.config.workspace_id,
        repository_id,
        default_selector,
        limit,
        items: commits,
        diagnostics: repository_diagnostics(diagnostics),
    }))
}

async fn repository_tickets(
    State(api): State<WorkspaceApi>,
    AxumPath(repository_id): AxumPath<String>,
    Query(query): Query<TicketKanbanQuery>,
) -> ApiResult<Json<RepositoryTicketsResponse>> {
    repository_lookup(api.repository_reader().summary(&repository_id))?;
    let canonical_repository_id = repository_id;
    let limit = query.limit.unwrap_or(api.config.max_records).min(200);
    let ProjectRecordList {
        items,
        invalid_records,
        record_authority,
    } = api.records.list_tickets(limit)?;
    Ok(Json(RepositoryTicketsResponse {
        workspace_id: api.config.workspace_id,
        repository_id: canonical_repository_id,
        limit,
        columns: ticket_kanban_columns(items),
        invalid_records,
        record_authority,
        source: "workspace_local_ticket_fallback".to_string(),
        diagnostics: vec![RuntimeDiagnostic {
            code: "repository_ticket_target_metadata_absent".to_string(),
            severity: DiagnosticSeverity::Info,
            message: "Ticket target Repository metadata is not available yet; Kanban groups all workspace-local Tickets by state as a read-only fallback.".to_string(),
        }],
    }))
}

async fn list_hosts(
    State(api): State<WorkspaceApi>,
) -> ApiResult<Json<RuntimeListResponse<HostSummary>>> {
    let limit = api.config.max_records.min(200);
    let runtime_hosts = api.runtime.list_hosts(limit);
    Ok(Json(RuntimeListResponse {
        workspace_id: api.config.workspace_id,
        limit,
        items: runtime_hosts.items,
        source: "worker_runtime_registry".to_string(),
        diagnostics: runtime_hosts.diagnostics,
    }))
}

async fn list_runtimes(
    State(api): State<WorkspaceApi>,
) -> ApiResult<Json<RuntimeListResponse<RuntimeSummary>>> {
    let limit = api.config.max_records.min(200);
    let runtimes = api.runtime.list_runtimes(limit);
    Ok(Json(RuntimeListResponse {
        workspace_id: api.config.workspace_id,
        limit,
        items: runtimes.items,
        source: "worker_runtime_registry".to_string(),
        diagnostics: runtimes.diagnostics,
    }))
}

async fn list_workers(
    State(api): State<WorkspaceApi>,
) -> ApiResult<Json<RuntimeListResponse<WorkerSummary>>> {
    workers_response(api).map(Json)
}

async fn get_runtime_connection_settings(
    State(api): State<WorkspaceApi>,
) -> ApiResult<Json<RuntimeConnectionSettingsResponse>> {
    let local_config = load_workspace_backend_config_for_settings(&api)?;
    Ok(Json(runtime_connection_settings_response(
        &api,
        &local_config,
    )))
}

async fn add_remote_runtime_connection(
    State(api): State<WorkspaceApi>,
    Json(request): Json<AddRemoteRuntimeConnectionRequest>,
) -> ApiResult<Json<RuntimeConnectionMutationResponse>> {
    validate_runtime_connection_request(&request)?;
    let mut local_config = load_workspace_backend_config_for_settings(&api)?;
    let id = request.runtime_id.trim().to_string();
    if id == EMBEDDED_WORKER_RUNTIME_ID {
        return Err(settings_bad_request(
            "embedded_runtime_not_config_managed",
            "the embedded Runtime is built in and cannot be managed from local remote Runtime config",
        ));
    }
    if request
        .token_ref
        .as_ref()
        .is_some_and(|value| !value.trim().is_empty())
    {
        return Err(settings_bad_request(
            "remote_runtime_token_ref_unsupported",
            "remote Runtime token_ref persistence is not supported by this v0 browser settings surface",
        ));
    }
    if local_config
        .runtimes
        .remote
        .iter()
        .any(|remote| remote.id == id)
    {
        return Err(settings_bad_request(
            "remote_runtime_already_exists",
            "a remote Runtime connection with that id is already configured",
        ));
    }
    let remote_config = RemoteRuntimeConfigFile {
        id,
        endpoint: request.endpoint.trim().to_string(),
        display_name: request
            .display_name
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned),
        token_ref: None,
    };
    let active_config = remote_runtime_config_from_file(&remote_config).map_err(|diagnostic| {
        ApiError::with_diagnostics(
            Error::RuntimeOperationFailed {
                runtime_id: remote_config.id.clone(),
                code: diagnostic.code.clone(),
                message: diagnostic.message.clone(),
            },
            vec![diagnostic],
        )
    })?;
    let active_runtime = RemoteWorkerRuntime::new(
        active_config,
        api.config.workspace_id.clone(),
        api.config
            .backend_base_url
            .clone()
            .unwrap_or_else(|| "http://127.0.0.1:8787".to_string()),
    )
    .map(|host| host.with_resource_broker(api.resource_broker.clone()))
    .map_err(|err| err.into_error())?;
    local_config.runtimes.remote.push(remote_config);
    write_workspace_backend_config_for_settings(&api, &local_config)?;
    api.runtime.register_or_replace(active_runtime);
    let mut response = runtime_connection_mutation_response(
        &api,
        &local_config,
        vec![settings_diagnostic(
            "runtime_registry_applied",
            DiagnosticSeverity::Info,
            "Remote Runtime config was persisted and applied to the active Runtime registry without restarting the Workspace backend.",
        )],
    );
    response.diagnostics.push(settings_diagnostic(
        "workspace_backend_config_rewritten",
        DiagnosticSeverity::Info,
        "Local Runtime connection config was rewritten from the typed schema; comments and formatting are not preserved in v0.",
    ));
    Ok(Json(response))
}

async fn delete_remote_runtime_connection(
    State(api): State<WorkspaceApi>,
    AxumPath(runtime_id): AxumPath<String>,
) -> ApiResult<Json<RuntimeConnectionMutationResponse>> {
    if runtime_id == EMBEDDED_WORKER_RUNTIME_ID {
        return Err(settings_bad_request(
            "embedded_runtime_not_config_managed",
            "the embedded Runtime is built in and cannot be deleted from remote Runtime config",
        ));
    }
    let mut local_config = load_workspace_backend_config_for_settings(&api)?;
    let before = local_config.runtimes.remote.len();
    local_config
        .runtimes
        .remote
        .retain(|remote| remote.id != runtime_id);
    if before == local_config.runtimes.remote.len() {
        return Err(Error::UnknownRuntime(runtime_id).into());
    }
    match api
        .runtime
        .unregister_if_idle(&runtime_id, api.config.max_records.min(200))
        .map_err(|err| err.into_error())?
    {
        RuntimeRegistryUnregisterResult::Removed | RuntimeRegistryUnregisterResult::NotFound => {}
        RuntimeRegistryUnregisterResult::BlockedByWorkers {
            worker_count,
            diagnostics,
        } => {
            let mut diagnostics = diagnostics;
            diagnostics.push(settings_diagnostic(
                "remote_runtime_delete_blocked",
                DiagnosticSeverity::Error,
                format!(
                    "Remote Runtime '{runtime_id}' has {worker_count} active worker(s); stop or move them before deleting the connection."
                ),
            ));
            return Err(ApiError::with_diagnostics(
                Error::RuntimeOperationFailed {
                    runtime_id,
                    code: "remote_runtime_delete_blocked".to_string(),
                    message: "Remote Runtime connection has active workers".to_string(),
                },
                diagnostics,
            ));
        }
    }
    write_workspace_backend_config_for_settings(&api, &local_config)?;
    let mut response = runtime_connection_mutation_response(
        &api,
        &local_config,
        vec![settings_diagnostic(
            "runtime_registry_applied",
            DiagnosticSeverity::Info,
            "Remote Runtime config was removed from persisted config and the active Runtime registry without restarting the Workspace backend.",
        )],
    );
    response.diagnostics.push(settings_diagnostic(
        "workspace_backend_config_rewritten",
        DiagnosticSeverity::Info,
        "Local Runtime connection config was rewritten from the typed schema; comments and formatting are not preserved in v0.",
    ));
    Ok(Json(response))
}

async fn test_remote_runtime_connection(
    State(api): State<WorkspaceApi>,
    AxumPath(runtime_id): AxumPath<String>,
) -> ApiResult<Json<RemoteRuntimeTestResponse>> {
    let local_config = load_workspace_backend_config_for_settings(&api)?;
    let remote = local_config
        .runtimes
        .remote
        .iter()
        .find(|remote| remote.id == runtime_id)
        .ok_or_else(|| Error::UnknownRuntime(runtime_id.clone()))?;
    Ok(Json(test_remote_runtime_config(&api, remote).await))
}

async fn get_worker_launch_options(
    State(api): State<WorkspaceApi>,
) -> ApiResult<Json<WorkerLaunchOptionsResponse>> {
    Ok(Json(worker_launch_options_response(&api)))
}

fn working_directory_request_from_repository(
    repository: &ConfiguredRepository,
    selector: Option<&str>,
) -> WorkingDirectoryRequest {
    WorkingDirectoryRequest {
        repository: WorkingDirectoryRepository {
            id: repository.id.clone(),
            provider: repository.provider.clone(),
            uri: repository.uri.clone(),
            local_path: Some(repository.path.clone()),
            selector: selector
                .map(|selector| RuntimeRepositorySelector::from(selector.to_string()))
                .or_else(|| {
                    repository
                        .default_selector
                        .clone()
                        .map(RuntimeRepositorySelector)
                })
                .or_else(|| Some(RuntimeRepositorySelector::from("HEAD"))),
        },
        materializer: MaterializerKind::LocalGitWorktree,
        backend_workdir_id: None,
    }
}

fn configured_working_directory_request(
    config: &ServerConfig,
    request: &WorkerSpawnWorkingDirectoryRequest,
) -> Result<WorkingDirectoryRequest> {
    let repository = config
        .repositories
        .iter()
        .find(|repository| repository.id == request.repository_id)
        .ok_or_else(|| {
            Error::Config(format!(
                "unknown repository id `{}` for Worker working directory",
                request.repository_id
            ))
        })?;
    Ok(working_directory_request_from_repository(
        repository,
        request.selector.as_deref(),
    ))
}

async fn create_workspace_worker(
    State(api): State<WorkspaceApi>,
    Json(request): Json<BrowserCreateWorkerRequest>,
) -> ApiResult<Json<BrowserCreateWorkerResponse>> {
    let profile_selector =
        profile_selector_for_candidate_with_root(&api.config.workspace_root, &request.profile)
            .ok_or_else(|| {
                settings_bad_request(
                    "unsupported_worker_profile",
                    "profile must be selected from Backend-published worker profile candidates",
                )
            })?;
    let resolved_config_bundle = if request.profile.starts_with("project:") {
        crate::profile_settings::build_workspace_profile_config_bundle(
            &api.config.workspace_root,
            &api.config.workspace_id,
            &api.config.workspace_created_at,
            &request.profile,
        )?
    } else {
        None
    };
    let display_name = sanitize_worker_display_name(&request.display_name).ok_or_else(|| {
        settings_bad_request(
            "invalid_worker_display_name",
            "display_name must contain at least one non-control character",
        )
    })?;
    let initial_text = request.initial_text.trim().to_string();
    let initial_input = if initial_text.is_empty() {
        None
    } else {
        Some(EmbeddedWorkerInput {
            kind: EmbeddedWorkerInputKind::User,
            content: initial_text,
            segments: None,
        })
    };
    let selected_working_directory_id = request
        .working_directory
        .as_ref()
        .map(|selection| selection.working_directory_id.clone());
    let resolved_working_directory =
        request
            .working_directory
            .map(|selection| WorkingDirectoryClaim {
                working_directory_id: selection.working_directory_id,
                relative_cwd: selection.relative_cwd,
            });
    validate_working_directory_claim_for_browser(resolved_working_directory.as_ref())?;
    if resolved_working_directory.is_none() {
        reject_no_workdir_for_non_embedded_runtime(&request.runtime_id)?;
    }
    let result = api
        .runtime
        .spawn_worker(
            &request.runtime_id,
            WorkerSpawnRequest {
                requested_worker_name: Some(display_name.clone()),
                intent: WorkerSpawnIntent::WorkspaceCoding,
                acceptance: WorkerSpawnAcceptanceRequirement::RunAccepted {
                    expected_segments: if initial_input.is_some() { 1 } else { 0 },
                },
                profile: Some(profile_selector),
                initial_input,
                working_directory_request: None,
                resolved_working_directory_request: None,
                resolved_working_directory,
                resolved_config_bundle,
            },
        )
        .map_err(|err| err.into_error())?;
    if result.state != WorkerOperationState::Accepted {
        return Err(worker_create_not_accepted_error(
            request.runtime_id.clone(),
            result.diagnostics,
        ));
    }
    let worker = result.worker.ok_or_else(|| Error::RuntimeOperationFailed {
        runtime_id: request.runtime_id.clone(),
        code: "workspace_worker_create_missing_summary".to_string(),
        message: "Runtime completed worker creation without returning a Worker summary".to_string(),
    })?;
    let worker_record = record_worker_summary(
        &api,
        &worker,
        display_name.as_str(),
        worker.profile.clone(),
        WorkerRegistryDisplayNamePolicy::UseProvided,
    )?;
    if let Some(working_directory) = worker.working_directory.as_ref() {
        let workdir_record =
            workdir_record_from_summary(&api, worker.runtime_id.as_str(), working_directory);
        api.store.upsert_workdir_registry(&workdir_record)?;
        link_worker_to_workdir(
            &api,
            &worker_record,
            &working_directory.working_directory_id,
        )?;
    }
    if let Some(workdir_id) = selected_working_directory_id.as_deref() {
        if api
            .store
            .get_workdir_registry(&api.config.workspace_id, workdir_id)?
            .is_none()
        {
            if let Ok(result) = api
                .runtime
                .working_directory(worker.runtime_id.as_str(), workdir_id)
                .map_err(|err| err.into_error())
            {
                if let Some(status) = result.working_directory {
                    let record = workdir_record_from_summary(
                        &api,
                        worker.runtime_id.as_str(),
                        &status.summary,
                    );
                    api.store.upsert_workdir_registry(&record)?;
                }
            }
        }
        if api
            .store
            .get_workdir_registry(&api.config.workspace_id, workdir_id)?
            .is_some()
        {
            link_worker_to_workdir(&api, &worker_record, workdir_id)?;
        }
    }
    let runtime_id = worker.runtime_id.clone();
    let worker_id = worker.worker_id.clone();
    let workspace_id = api.workspace_id().to_string();
    let console_href = format!(
        "/w/{}/runtimes/{}/workers/{}/console",
        encode_path_segment(&workspace_id),
        encode_path_segment(&runtime_id),
        encode_path_segment(&worker_id)
    );
    Ok(Json(BrowserCreateWorkerResponse {
        workspace_id,
        runtime_id,
        worker_id,
        console_href,
        worker,
        diagnostics: result.diagnostics,
    }))
}

async fn post_internal_runtime_resource_fetch(
    State(api): State<WorkspaceApi>,
    Json(request): Json<BackendResourceFetchRequest>,
) -> std::result::Result<
    Json<worker_runtime::resource::BackendResourceFetchResponse>,
    (StatusCode, Json<BackendResourceError>),
> {
    api.resource_broker
        .fetch_profile_source_archive(request)
        .map(Json)
        .map_err(|error| (backend_resource_error_status(&error), Json(error)))
}

fn backend_resource_error_status(error: &BackendResourceError) -> StatusCode {
    match error {
        BackendResourceError::Expired => StatusCode::GONE,
        BackendResourceError::Unauthorized { .. } => StatusCode::UNAUTHORIZED,
        BackendResourceError::MissingResource => StatusCode::NOT_FOUND,
        BackendResourceError::UnsupportedKind
        | BackendResourceError::DigestMismatch { .. }
        | BackendResourceError::Oversized { .. }
        | BackendResourceError::ContentTypeMismatch { .. }
        | BackendResourceError::InvalidResponse { .. } => StatusCode::BAD_REQUEST,
        BackendResourceError::Transport { .. } => StatusCode::BAD_GATEWAY,
    }
}

async fn get_companion_status(
    State(api): State<WorkspaceApi>,
) -> ApiResult<Json<CompanionStatusResponse>> {
    Ok(Json(api.companion.status()))
}

async fn get_companion_transcript(
    State(api): State<WorkspaceApi>,
    Query(query): Query<TranscriptQuery>,
) -> ApiResult<Json<CompanionTranscriptProjection>> {
    let limit = query.limit.unwrap_or(api.config.max_records).min(200);
    let start = query.start.unwrap_or(0);
    Ok(Json(api.companion.transcript(start, limit)))
}

async fn post_companion_message(
    State(api): State<WorkspaceApi>,
    Json(request): Json<CompanionMessageRequest>,
) -> ApiResult<Json<CompanionMessageResponse>> {
    Ok(Json(api.companion.send_message(request)))
}

async fn post_companion_cancel(
    State(api): State<WorkspaceApi>,
    Json(request): Json<CompanionCancelRequest>,
) -> ApiResult<Json<CompanionMessageResponse>> {
    Ok(Json(api.companion.cancel(request)))
}

async fn get_runtime_worker(
    State(api): State<WorkspaceApi>,
    AxumPath((runtime_id, worker_id)): AxumPath<(String, String)>,
) -> ApiResult<Json<WorkerSummary>> {
    let worker = api
        .runtime
        .worker(&runtime_id, &worker_id)
        .map_err(|err| err.into_error())?;
    let record = sync_worker_observation(&api, &worker)?;
    let links = api.store.list_worker_workdir_links(
        &api.config.workspace_id,
        record.runtime_id.as_str(),
        record.runtime_worker_id,
    )?;
    let workdirs = api
        .store
        .list_workdir_registry(&api.config.workspace_id, 500)?;
    Ok(Json(merge_worker_registry_projection(
        Some(&worker),
        &record,
        links,
        &workdirs,
    )))
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RuntimeConfigBundleSyncRequest {
    pub bundle: ConfigBundle,
}

#[derive(Debug, Deserialize)]
struct RuntimeConfigBundleAvailabilityQuery {
    digest: String,
}

fn reject_workdir_for_embedded_runtime(
    runtime_id: &str,
    has_workdir: bool,
) -> std::result::Result<(), ApiError> {
    if runtime_id != EMBEDDED_WORKER_RUNTIME_ID || !has_workdir {
        return Ok(());
    }
    Err(ApiError::with_diagnostics(
        Error::RuntimeOperationFailed {
            runtime_id: runtime_id.to_string(),
            code: "embedded_worker_workdir_unsupported".to_string(),
            message: "The embedded Runtime does not accept working directories".to_string(),
        },
        vec![RuntimeDiagnostic {
            code: "embedded_worker_workdir_unsupported".to_string(),
            severity: DiagnosticSeverity::Error,
            message: "Choose a non-embedded Runtime for workspace-file Workers; embedded Workers are no-workdir Workspace-API workers.".to_string(),
        }],
    ))
}

fn reject_no_workdir_for_non_embedded_runtime(
    runtime_id: &str,
) -> std::result::Result<(), ApiError> {
    if runtime_id == EMBEDDED_WORKER_RUNTIME_ID {
        return Ok(());
    }
    Err(ApiError::with_diagnostics(
        Error::RuntimeOperationFailed {
            runtime_id: runtime_id.to_string(),
            code: "workspace_worker_workdir_required".to_string(),
            message: "Only the embedded Runtime can launch a Worker without a working directory"
                .to_string(),
        },
        vec![RuntimeDiagnostic {
            code: "workspace_worker_workdir_required".to_string(),
            severity: DiagnosticSeverity::Error,
            message: "Select a working directory for this Runtime, or choose the embedded Runtime for a conversation-only Worker."
                .to_string(),
        }],
    ))
}

async fn list_runtime_workers(
    State(api): State<WorkspaceApi>,
    AxumPath(runtime_id): AxumPath<String>,
) -> ApiResult<Json<RuntimeListResponse<WorkerSummary>>> {
    let limit = api.config.max_records.min(200);
    let worker_list = api
        .runtime
        .list_workers_for_runtime(&runtime_id, limit)
        .map_err(|err| err.into_error())?;
    Ok(Json(RuntimeListResponse {
        workspace_id: api.workspace_id().to_string(),
        limit,
        items: worker_list.items,
        source: "runtime_registry".to_string(),
        diagnostics: worker_list.diagnostics,
    }))
}

async fn create_runtime_worker(
    State(api): State<WorkspaceApi>,
    AxumPath(runtime_id): AxumPath<String>,
    Json(mut request): Json<WorkerSpawnRequest>,
) -> ApiResult<Json<WorkerSpawnResult>> {
    reject_workdir_for_embedded_runtime(
        &runtime_id,
        request.working_directory_request.is_some() || request.resolved_working_directory.is_some(),
    )?;
    if request.working_directory_request.is_none() && request.resolved_working_directory.is_none() {
        reject_no_workdir_for_non_embedded_runtime(&runtime_id)?;
    }
    request.resolved_working_directory_request = request
        .working_directory_request
        .as_ref()
        .map(|working_directory| {
            configured_working_directory_request(&api.config, working_directory)
        })
        .transpose()?;
    let prepared_workdir_id = if let Some(working_directory_request) =
        request.resolved_working_directory_request.as_mut()
    {
        Some(upsert_pending_backend_workdir(
            &api,
            &runtime_id,
            working_directory_request,
        )?)
    } else {
        request
            .resolved_working_directory
            .as_ref()
            .map(|claim| claim.working_directory_id.clone())
    };
    let requested_worker_name = request.requested_worker_name.clone();
    let result = api
        .runtime
        .spawn_worker(&runtime_id, request)
        .map_err(|err| err.into_error())?;
    if let Some(worker) = result.worker.as_ref() {
        let display_name = requested_worker_name
            .as_deref()
            .filter(|name| !name.trim().is_empty())
            .unwrap_or(worker.label.as_str())
            .to_string();
        let record = record_worker_summary(
            &api,
            worker,
            display_name.as_str(),
            worker.profile.clone(),
            WorkerRegistryDisplayNamePolicy::UseProvided,
        )?;
        if worker.working_directory.is_none() {
            if let Some(workdir_id) = prepared_workdir_id.as_deref() {
                if api
                    .store
                    .get_workdir_registry(&api.config.workspace_id, workdir_id)?
                    .is_some()
                {
                    link_worker_to_workdir(&api, &record, workdir_id)?;
                }
            }
        }
    } else if let Some(workdir_id) = prepared_workdir_id.as_deref() {
        if let Some(mut record) = api
            .store
            .get_workdir_registry(&api.config.workspace_id, workdir_id)?
        {
            record.materialization_status = "failed".to_string();
            record.updated_at = now_registry_timestamp();
            api.store.upsert_workdir_registry(&record)?;
        }
    }
    Ok(Json(result))
}

async fn sync_runtime_config_bundle(
    State(api): State<WorkspaceApi>,
    AxumPath(runtime_id): AxumPath<String>,
    Json(request): Json<RuntimeConfigBundleSyncRequest>,
) -> ApiResult<Json<ConfigBundleSyncResult>> {
    let result = api
        .runtime
        .sync_config_bundle(&runtime_id, request.bundle)
        .map_err(|err| err.into_error())?;
    Ok(Json(result))
}

async fn check_runtime_config_bundle(
    State(api): State<WorkspaceApi>,
    AxumPath((runtime_id, bundle_id)): AxumPath<(String, String)>,
    Query(query): Query<RuntimeConfigBundleAvailabilityQuery>,
) -> ApiResult<Json<ConfigBundleCheckResult>> {
    let result = api
        .runtime
        .check_config_bundle(
            &runtime_id,
            ConfigBundleRef {
                id: bundle_id,
                digest: query.digest,
            },
        )
        .map_err(|err| err.into_error())?;
    Ok(Json(result))
}

async fn send_runtime_worker_input(
    State(api): State<WorkspaceApi>,
    AxumPath((runtime_id, worker_id)): AxumPath<(String, String)>,
    Json(request): Json<WorkerInputRequest>,
) -> ApiResult<Json<WorkerInputResult>> {
    let result = api
        .runtime
        .send_input(&runtime_id, &worker_id, request)
        .map_err(|err| err.into_error())?;
    Ok(Json(result))
}

async fn runtime_worker_completions(
    State(api): State<WorkspaceApi>,
    AxumPath((runtime_id, worker_id)): AxumPath<(String, String)>,
    Json(request): Json<WorkerCompletionsRequest>,
) -> ApiResult<Json<WorkerCompletionsResult>> {
    let result = api
        .runtime
        .worker_completions(&runtime_id, &worker_id, request)
        .map_err(|err| err.into_error())?;
    Ok(Json(result))
}

async fn stop_runtime_worker(
    State(api): State<WorkspaceApi>,
    AxumPath((runtime_id, worker_id)): AxumPath<(String, String)>,
    Json(request): Json<WorkerLifecycleRequest>,
) -> ApiResult<Json<WorkerLifecycleResult>> {
    let result = api
        .runtime
        .stop_worker(&runtime_id, &worker_id, request)
        .map_err(|err| err.into_error())?;
    let runtime_worker_id = parse_runtime_worker_id_for_registry(&worker_id)?;
    if let Some(record) =
        api.store
            .get_worker_registry(&api.config.workspace_id, &runtime_id, runtime_worker_id)?
    {
        sync_linked_workdir_after_worker_stop(&api, &runtime_id, &record)?;
    }
    Ok(Json(result))
}

async fn cancel_runtime_worker(
    State(api): State<WorkspaceApi>,
    AxumPath((runtime_id, worker_id)): AxumPath<(String, String)>,
    Json(request): Json<WorkerLifecycleRequest>,
) -> ApiResult<Json<WorkerLifecycleResult>> {
    let result = api
        .runtime
        .cancel_worker(&runtime_id, &worker_id, request)
        .map_err(|err| err.into_error())?;
    Ok(Json(result))
}

async fn worker_protocol_ws(
    State(api): State<WorkspaceApi>,
    AxumPath((runtime_id, worker_id)): AxumPath<(String, String)>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    let source = match api.observation_proxy.source(&runtime_id, &worker_id) {
        Ok(source) => source,
        Err(ObservationProxyError::WorkerNotFound(_)) => {
            match api.runtime.observation_source(&runtime_id, &worker_id) {
                Ok(source) => source,
                Err(error) => return ApiError::from(error.into_error()).into_response(),
            }
        }
        Err(error) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": error.code(),
                    "message": error.message(),
                })),
            )
                .into_response();
        }
    };
    ws.on_upgrade(move |socket| worker_protocol_ws_session(source, socket))
}

async fn worker_protocol_ws_session(source: RuntimeObservationSource, socket: WebSocket) {
    match source {
        RuntimeObservationSource::RemoteWs(config) => {
            remote_worker_protocol_ws_session(config, socket).await;
        }
        RuntimeObservationSource::Embedded(source) => {
            embedded_worker_protocol_ws_session(source, socket).await;
        }
    }
}

async fn remote_worker_protocol_ws_session(
    config: RuntimeObservationSourceConfig,
    socket: WebSocket,
) {
    let mut request = match config.endpoint.clone().into_client_request() {
        Ok(request) => request,
        Err(error) => {
            let mut socket = socket;
            let event = protocol_error_event(format!(
                "failed to build runtime protocol WebSocket request: {error}"
            ));
            let _ = send_protocol_event(&mut socket, &event).await;
            return;
        }
    };
    if let Some(token) = &config.bearer_token {
        match format!("Bearer {token}").parse() {
            Ok(value) => {
                request.headers_mut().insert("authorization", value);
            }
            Err(error) => {
                let mut socket = socket;
                let event = protocol_error_event(format!(
                    "failed to build runtime authorization header: {error}"
                ));
                let _ = send_protocol_event(&mut socket, &event).await;
                return;
            }
        }
    }

    let (upstream, _) = match connect_async(request).await {
        Ok(connection) => connection,
        Err(error) => {
            let mut socket = socket;
            let event = protocol_error_event(format!(
                "failed to connect runtime protocol WebSocket: {error}"
            ));
            let _ = send_protocol_event(&mut socket, &event).await;
            return;
        }
    };

    let (mut client_sink, mut client_stream) = socket.split();
    let (mut upstream_sink, mut upstream_stream) = upstream.split();

    loop {
        tokio::select! {
            inbound = client_stream.next() => {
                match inbound {
                    Some(Ok(WsMessage::Text(text))) => {
                        if upstream_sink.send(TungsteniteMessage::Text(text.to_string().into())).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(WsMessage::Binary(binary))) => {
                        if upstream_sink.send(TungsteniteMessage::Binary(binary.to_vec().into())).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(WsMessage::Close(_))) | None => {
                        let _ = upstream_sink.send(TungsteniteMessage::Close(None)).await;
                        break;
                    }
                    Some(Ok(WsMessage::Ping(payload))) => {
                        if upstream_sink.send(TungsteniteMessage::Ping(payload.to_vec().into())).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(WsMessage::Pong(payload))) => {
                        if upstream_sink.send(TungsteniteMessage::Pong(payload.to_vec().into())).await.is_err() {
                            break;
                        }
                    }
                    Some(Err(_)) => break,
                }
            }
            outbound = upstream_stream.next() => {
                match outbound {
                    Some(Ok(TungsteniteMessage::Text(text))) => {
                        if client_sink.send(WsMessage::Text(text.to_string().into())).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(TungsteniteMessage::Binary(binary))) => {
                        if client_sink.send(WsMessage::Binary(binary.to_vec().into())).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(TungsteniteMessage::Close(_))) | None => {
                        let _ = client_sink.send(WsMessage::Close(None)).await;
                        break;
                    }
                    Some(Ok(TungsteniteMessage::Ping(payload))) => {
                        if client_sink.send(WsMessage::Ping(payload.to_vec().into())).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(TungsteniteMessage::Pong(payload))) => {
                        if client_sink.send(WsMessage::Pong(payload.to_vec().into())).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(TungsteniteMessage::Frame(_))) => {}
                    Some(Err(_)) => break,
                }
            }
        }
    }
}

async fn embedded_worker_protocol_ws_session(
    source: crate::observation::EmbeddedRuntimeObservationSource,
    mut socket: WebSocket,
) {
    let mut upstream = match RuntimeObservationClient::connect(&RuntimeObservationSource::Embedded(
        source.clone(),
    ))
    .await
    {
        Ok(client) => client,
        Err(error) => {
            let event = protocol_error_event(error.message());
            let _ = send_protocol_event(&mut socket, &event).await;
            return;
        }
    };

    loop {
        tokio::select! {
            inbound = socket.next() => {
                match inbound {
                    Some(Ok(WsMessage::Text(text))) => match decode_method(&text) {
                        Ok(method) => match source.runtime.send_protocol_method(&source.worker_ref, method) {
                            Ok(events) => {
                                for event in events {
                                    if !send_protocol_event(&mut socket, &event).await {
                                        return;
                                    }
                                }
                            }
                            Err(error) => {
                                let event = protocol_error_event(error.to_string());
                                if !send_protocol_event(&mut socket, &event).await {
                                    return;
                                }
                            }
                        },
                        Err(error) => {
                            let event =
                                protocol_error_event(format!("malformed protocol method frame: {error}"));
                            if !send_protocol_event(&mut socket, &event).await {
                                return;
                            }
                        }
                    },
                    Some(Ok(WsMessage::Close(_))) | None => return,
                    Some(Ok(WsMessage::Ping(payload))) => {
                        if socket.send(WsMessage::Pong(payload)).await.is_err() {
                            return;
                        }
                    }
                    Some(Ok(WsMessage::Pong(_))) | Some(Ok(WsMessage::Binary(_))) => {}
                    Some(Err(error)) => {
                        let event = protocol_error_event(format!("protocol WebSocket error: {error}"));
                        let _ = send_protocol_event(&mut socket, &event).await;
                        return;
                    }
                }
            }
            upstream_event = upstream.next_event() => {
                match upstream_event {
                    Ok(event) => {
                        if !send_protocol_event(&mut socket, &event.payload).await {
                            return;
                        }
                    }
                    Err(error) => {
                        let event = protocol_error_event(error.message());
                        let _ = send_protocol_event(&mut socket, &event).await;
                        return;
                    }
                }
            }
        }
    }
}
async fn send_protocol_event(socket: &mut WebSocket, event: &protocol::Event) -> bool {
    match encode_event(event) {
        Ok(text) => socket.send(WsMessage::Text(text.into())).await.is_ok(),
        Err(error) => {
            let fallback = protocol_error_event(format!(
                "failed to serialize protocol response event: {error}"
            ));
            let Ok(text) = encode_event(&fallback) else {
                return false;
            };
            socket.send(WsMessage::Text(text.into())).await.is_ok()
        }
    }
}

fn protocol_error_event(message: impl Into<String>) -> protocol::Event {
    protocol::Event::Error {
        code: protocol::ErrorCode::Internal,
        message: message.into(),
    }
}

async fn list_host_workers(
    State(api): State<WorkspaceApi>,
    AxumPath(host_id): AxumPath<String>,
) -> ApiResult<Json<RuntimeListResponse<WorkerSummary>>> {
    let limit = api.config.max_records.min(200);
    let runtime_workers = api
        .runtime
        .list_workers_for_host(&host_id, limit)
        .map_err(|err| err.into_error())?;
    Ok(Json(RuntimeListResponse {
        workspace_id: api.config.workspace_id,
        limit,
        items: runtime_workers.items,
        source: "worker_runtime_registry".to_string(),
        diagnostics: runtime_workers.diagnostics,
    }))
}

fn workers_response(api: WorkspaceApi) -> ApiResult<RuntimeListResponse<WorkerSummary>> {
    let limit = api.config.max_records.min(200);
    let runtime_workers = api.runtime.list_workers(limit);
    let mut observed = std::collections::BTreeMap::new();
    for worker in &runtime_workers.items {
        let _ = sync_worker_observation(&api, worker);
        if let Ok(runtime_worker_id) =
            parse_runtime_worker_id_for_registry(worker.worker_id.as_str())
        {
            observed.insert(
                (worker.runtime_id.clone(), runtime_worker_id),
                worker.clone(),
            );
        }
    }
    let mut diagnostics = runtime_workers.diagnostics;
    let worker_records = api
        .store
        .list_worker_registry(&api.config.workspace_id, limit)?;
    let workdir_records = api
        .store
        .list_workdir_registry(&api.config.workspace_id, 500)?;
    let mut items = Vec::new();
    for record in worker_records {
        if !observed.contains_key(&(record.runtime_id.clone(), record.runtime_worker_id)) {
            match api.runtime.worker(
                record.runtime_id.as_str(),
                &record.runtime_worker_id.to_string(),
            ) {
                Ok(worker) => {
                    let _ = sync_worker_observation(&api, &worker);
                    observed.insert(
                        (worker.runtime_id.clone(), record.runtime_worker_id),
                        worker,
                    );
                }
                Err(RuntimeRegistryError::UnknownWorker { .. }) => {}
                Err(error) => diagnostics.push(RuntimeDiagnostic {
                    code: "worker_detail_probe_failed".to_string(),
                    severity: DiagnosticSeverity::Info,
                    message: format!(
                        "Could not verify Worker {} on Runtime {}: {}",
                        record.runtime_worker_id,
                        record.runtime_id,
                        sanitize_backend_error(&error.into_error().to_string())
                    ),
                }),
            }
        }
        let links = api.store.list_worker_workdir_links(
            &api.config.workspace_id,
            record.runtime_id.as_str(),
            record.runtime_worker_id,
        )?;
        items.push(merge_worker_registry_projection(
            observed.get(&(record.runtime_id.clone(), record.runtime_worker_id)),
            &record,
            links,
            &workdir_records,
        ));
    }
    Ok(RuntimeListResponse {
        workspace_id: api.config.workspace_id,
        limit,
        items,
        source: "backend_worker_registry".to_string(),
        diagnostics,
    })
}

fn load_workspace_backend_config_for_settings(
    api: &WorkspaceApi,
) -> ApiResult<WorkspaceBackendConfigFile> {
    WorkspaceBackendConfigFile::load_for_workspace(&api.config.workspace_root).map_err(|error| {
        Error::Config(format!(
            "failed to read workspace backend local config for Runtime connections: {}",
            sanitize_backend_error(&error.to_string())
        ))
        .into()
    })
}

fn write_workspace_backend_config_for_settings(
    api: &WorkspaceApi,
    local_config: &WorkspaceBackendConfigFile,
) -> ApiResult<()> {
    local_config
        .write_for_workspace(&api.config.workspace_root)
        .map_err(|error| {
            Error::Config(format!(
                "failed to write workspace backend local config for Runtime connections: {}",
                sanitize_backend_error(&error.to_string())
            ))
            .into()
        })
}

fn runtime_connection_settings_response(
    api: &WorkspaceApi,
    local_config: &WorkspaceBackendConfigFile,
) -> RuntimeConnectionSettingsResponse {
    RuntimeConnectionSettingsResponse {
        workspace_id: api.config.workspace_id.clone(),
        embedded: embedded_runtime_connection_summary(api),
        remotes: remote_runtime_connection_summaries(api, local_config, false),
        diagnostics: Vec::new(),
    }
}

fn runtime_connection_mutation_response(
    api: &WorkspaceApi,
    local_config: &WorkspaceBackendConfigFile,
    diagnostics: Vec<RuntimeDiagnostic>,
) -> RuntimeConnectionMutationResponse {
    RuntimeConnectionMutationResponse {
        workspace_id: api.config.workspace_id.clone(),
        restart_required: false,
        remotes: remote_runtime_connection_summaries(api, local_config, false),
        diagnostics,
    }
}

fn embedded_runtime_connection_summary(api: &WorkspaceApi) -> RuntimeConnectionSummary {
    let active = api
        .runtime
        .list_runtimes(api.config.max_records.min(200))
        .items
        .into_iter()
        .find(|runtime| runtime.runtime_id == EMBEDDED_WORKER_RUNTIME_ID);
    match active {
        Some(runtime) => RuntimeConnectionSummary {
            runtime_id: runtime.runtime_id,
            display_name: runtime.label,
            kind: runtime.kind,
            built_in: true,
            config_managed: false,
            active: runtime.status == "active",
            can_spawn_worker: runtime.capabilities.can_spawn_worker,
            restart_required: false,
            status: runtime.status,
            diagnostics: runtime.diagnostics,
        },
        None => RuntimeConnectionSummary {
            runtime_id: EMBEDDED_WORKER_RUNTIME_ID.to_string(),
            display_name: "Embedded Runtime".to_string(),
            kind: "embedded_worker_runtime".to_string(),
            built_in: true,
            config_managed: false,
            active: false,
            can_spawn_worker: false,
            restart_required: false,
            status: "unavailable".to_string(),
            diagnostics: vec![settings_diagnostic(
                "embedded_runtime_unavailable",
                DiagnosticSeverity::Warning,
                "The built-in embedded Runtime is not active in the current Runtime registry projection.",
            )],
        },
    }
}

fn remote_runtime_connection_summaries(
    api: &WorkspaceApi,
    local_config: &WorkspaceBackendConfigFile,
    restart_required: bool,
) -> Vec<RemoteRuntimeConnectionSummary> {
    let live_runtimes = api
        .runtime
        .list_runtimes(api.config.max_records.min(200))
        .items;
    local_config
        .runtimes
        .remote
        .iter()
        .map(|remote| {
            let live = live_runtimes
                .iter()
                .find(|runtime| runtime.runtime_id == remote.id);
            let (display_name, kind, active, can_spawn_worker, status, diagnostics) = match live {
                Some(runtime) => (
                    runtime.label.clone(),
                    runtime.kind.clone(),
                    runtime.status == "active",
                    runtime.capabilities.can_spawn_worker,
                    runtime.status.clone(),
                    runtime.diagnostics.clone(),
                ),
                None => (
                    remote
                        .display_name
                        .clone()
                        .unwrap_or_else(|| remote.id.clone()),
                    "remote_http".to_string(),
                    false,
                    false,
                    "configured_restart_required".to_string(),
                    if restart_required {
                        vec![settings_diagnostic(
                            "runtime_registry_restart_required",
                            DiagnosticSeverity::Warning,
                            "This remote Runtime config is persisted but not active until the Workspace backend restarts.",
                        )]
                    } else {
                        Vec::new()
                    },
                ),
            };
            RemoteRuntimeConnectionSummary {
                summary: RuntimeConnectionSummary {
                    runtime_id: remote.id.clone(),
                    display_name,
                    kind,
                    built_in: false,
                    config_managed: true,
                    active,
                    can_spawn_worker,
                    restart_required,
                    status,
                    diagnostics,
                },
                endpoint_configured: !remote.endpoint.trim().is_empty(),
                token_ref_configured: remote
                    .token_ref
                    .as_deref()
                    .is_some_and(|value| !value.trim().is_empty()),
            }
        })
        .collect()
}

fn validate_runtime_connection_request(
    request: &AddRemoteRuntimeConnectionRequest,
) -> ApiResult<()> {
    validate_public_runtime_id(request.runtime_id.trim())?;
    let endpoint = request.endpoint.trim();
    if endpoint.is_empty() || !(endpoint.starts_with("http://") || endpoint.starts_with("https://"))
    {
        return Err(settings_bad_request(
            "invalid_remote_runtime_endpoint",
            "endpoint must be an absolute http or https URL",
        ));
    }
    if request
        .display_name
        .as_deref()
        .is_some_and(|value| value.chars().any(char::is_control))
    {
        return Err(settings_bad_request(
            "invalid_remote_runtime_display_name",
            "display_name cannot contain control characters",
        ));
    }
    Ok(())
}

fn validate_public_runtime_id(runtime_id: &str) -> ApiResult<()> {
    if runtime_id.is_empty() {
        return Err(settings_bad_request(
            "invalid_runtime_id",
            "runtime_id must not be empty",
        ));
    }
    if runtime_id.len() > 96
        || !runtime_id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
    {
        return Err(settings_bad_request(
            "invalid_runtime_id",
            "runtime_id may contain only ASCII letters, digits, '-', '_' and '.' and must be at most 96 characters",
        ));
    }
    Ok(())
}

fn remote_runtime_config_from_file(
    remote: &RemoteRuntimeConfigFile,
) -> std::result::Result<RemoteRuntimeConfig, RuntimeDiagnostic> {
    resolve_remote_runtime(remote).map_err(|err| {
        settings_diagnostic(
            "remote_runtime_apply_failed",
            DiagnosticSeverity::Error,
            err.to_string(),
        )
    })
}

async fn test_remote_runtime_config(
    api: &WorkspaceApi,
    remote: &RemoteRuntimeConfigFile,
) -> RemoteRuntimeTestResponse {
    let checked_at = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
    if remote
        .token_ref
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
    {
        return RemoteRuntimeTestResponse {
            workspace_id: api.config.workspace_id.clone(),
            runtime_id: remote.id.clone(),
            checked_at,
            state: "rejected".to_string(),
            protocol_version: None,
            compatibility_basis: "not_checked_token_ref_unsupported".to_string(),
            capabilities: Vec::new(),
            health_result: "not_checked".to_string(),
            diagnostics: vec![settings_diagnostic(
                "remote_runtime_token_ref_unsupported",
                DiagnosticSeverity::Error,
                "Remote Runtime test cannot use token_ref in v0; no token or secret value was exposed to the Browser.",
            )],
        };
    }

    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
    {
        Ok(client) => client,
        Err(_) => {
            return remote_runtime_test_failed(
                api,
                remote,
                checked_at,
                "remote_runtime_test_client_unavailable",
                "Remote Runtime test client could not be initialized.",
            );
        }
    };

    let mut observation = RuntimeCompatibilityObservation::default();
    let summary_url = match remote_probe_url(remote, "/v1/runtime") {
        Ok(url) => url,
        Err(diagnostic) => {
            return remote_runtime_test_failed(
                api,
                remote,
                checked_at,
                diagnostic.code,
                diagnostic.message,
            );
        }
    };

    let summary_payload =
        match probe_remote_json(&client, summary_url, "runtime.summary", "Runtime summary").await {
            Ok(payload) => payload,
            Err(diagnostic) => {
                return remote_runtime_test_failed(
                    api,
                    remote,
                    checked_at,
                    diagnostic.code,
                    diagnostic.message,
                );
            }
        };
    let protocol_version = summary_payload
        .get("protocol_version")
        .and_then(|value| value.as_str())
        .map(ToOwned::to_owned);
    let summary = match serde_json::from_value::<RuntimeHttpSummaryResponse>(summary_payload) {
        Ok(summary) => summary,
        Err(_) => {
            return remote_runtime_test_failed(
                api,
                remote,
                checked_at,
                "remote_runtime_malformed_summary",
                "Remote Runtime summary responded, but the payload was not recognized.",
            );
        }
    };
    observation.available(
        "runtime.summary",
        "Connected: /v1/runtime responded with a recognized worker-runtime summary.",
    );

    let workers_url = match remote_probe_url(remote, "/v1/workers") {
        Ok(url) => url,
        Err(diagnostic) => {
            observation.incompatible("workers.list", diagnostic);
            String::new()
        }
    };
    let workers = if workers_url.is_empty() {
        None
    } else {
        match probe_remote_json(&client, workers_url, "workers.list", "Worker list").await {
            Ok(payload) => match serde_json::from_value::<RuntimeHttpWorkersResponse>(payload) {
                Ok(workers) => {
                    observation.available(
                        "workers.list",
                        "Verified: /v1/workers responded with a recognized worker list.",
                    );
                    Some(workers)
                }
                Err(_) => {
                    observation.incompatible(
                        "workers.list",
                        settings_diagnostic(
                            "remote_runtime_workers_malformed",
                            DiagnosticSeverity::Error,
                            "Remote Runtime worker list responded, but the payload was not recognized.",
                        ),
                    );
                    None
                }
            },
            Err(diagnostic) => {
                observation.incompatible("workers.list", diagnostic);
                None
            }
        }
    };

    if let Some(worker) = workers.as_ref().and_then(|workers| workers.workers.first()) {
        let path = format!(
            "/v1/workers/{}",
            encode_path_segment(&worker.worker_id.to_string())
        );
        match remote_probe_url(remote, &path) {
            Ok(url) => match probe_remote_json(&client, url, "workers.detail", "Worker detail").await {
                Ok(payload) => match serde_json::from_value::<RuntimeHttpWorkerResponse>(payload) {
                    Ok(_) => observation.available(
                        "workers.detail",
                        "Verified: worker detail responded for an existing worker reported by the remote Runtime.",
                    ),
                    Err(_) => observation.incompatible(
                        "workers.detail",
                        settings_diagnostic(
                            "remote_runtime_worker_detail_malformed",
                            DiagnosticSeverity::Error,
                            "Remote Runtime worker detail responded, but the payload was not recognized.",
                        ),
                    ),
                },
                Err(diagnostic) => observation.incompatible("workers.detail", diagnostic),
            },
            Err(diagnostic) => observation.incompatible("workers.detail", diagnostic),
        }
    } else {
        observation.unknown(
            "workers.detail",
            "No connection problem found. Worker detail was not checked because the remote Runtime reported no workers during the lightweight probe.",
        );
    }

    observation.available(
        "workers.events_ws.construct",
        "Verified: worker event websocket URL can be constructed from the configured HTTP(S) Runtime endpoint. The lightweight test does not open a websocket stream.",
    );

    let bundles_url = match remote_probe_url(remote, "/v1/config-bundles") {
        Ok(url) => url,
        Err(diagnostic) => {
            observation.incompatible("config_bundles.list", diagnostic);
            String::new()
        }
    };
    let bundles = if bundles_url.is_empty() {
        None
    } else {
        match probe_remote_json(
            &client,
            bundles_url,
            "config_bundles.list",
            "Config-bundle list",
        )
        .await
        {
            Ok(payload) => {
                match serde_json::from_value::<RuntimeHttpConfigBundlesResponse>(payload) {
                    Ok(bundles) => {
                        observation.available(
                            "config_bundles.list",
                            "Verified: /v1/config-bundles responded with a recognized config-bundle list.",
                        );
                        Some(bundles)
                    }
                    Err(_) => {
                        observation.incompatible(
                        "config_bundles.list",
                        settings_diagnostic(
                            "remote_runtime_config_bundles_malformed",
                            DiagnosticSeverity::Error,
                            "Remote Runtime config-bundle list responded, but the payload was not recognized.",
                        ),
                    );
                        None
                    }
                }
            }
            Err(diagnostic) => {
                observation.incompatible("config_bundles.list", diagnostic);
                None
            }
        }
    };

    if let Some(bundle) = bundles.as_ref().and_then(|bundles| bundles.bundles.first()) {
        let path = format!(
            "/v1/config-bundles/{}/availability?digest={}",
            encode_path_segment(&bundle.id),
            encode_path_segment(&bundle.digest)
        );
        match remote_probe_url(remote, &path) {
            Ok(url) => match probe_remote_json(
                &client,
                url,
                "config_bundles.availability",
                "Config-bundle availability",
            )
            .await
            {
                Ok(payload) => {
                    match serde_json::from_value::<RuntimeHttpConfigBundleAvailabilityResponse>(payload)
                    {
                        Ok(_) => observation.available(
                            "config_bundles.availability",
                            "Verified: config-bundle availability was confirmed for an advertised bundle.",
                        ),
                        Err(_) => observation.incompatible(
                            "config_bundles.availability",
                            settings_diagnostic(
                                "remote_runtime_config_bundle_availability_malformed",
                                DiagnosticSeverity::Error,
                                "Remote Runtime config-bundle availability responded, but the payload was not recognized.",
                            ),
                        ),
                    }
                }
                Err(diagnostic) => {
                    observation.incompatible("config_bundles.availability", diagnostic)
                }
            },
            Err(diagnostic) => observation.incompatible("config_bundles.availability", diagnostic),
        }
    } else {
        observation.unknown(
            "config_bundles.availability",
            "No connection problem found. Config-bundle availability was not checked because the remote Runtime advertised no bundles during the lightweight probe.",
        );
    }

    if summary.runtime.worker_creation_available {
        observation.available(
            "workers.spawn",
            "Verified: /v1/runtime reports worker creation is enabled by a Runtime execution backend. The lightweight test does not create a worker.",
        );
    } else {
        observation.incompatible(
            "workers.spawn",
            settings_diagnostic(
                "remote_runtime_worker_creation_unavailable",
                DiagnosticSeverity::Error,
                "Connected to the Runtime, but worker creation is unavailable because this Runtime process has no execution backend attached.",
            ),
        );
    }
    observation.unknown(
        "workers.input_dispatch",
        "No connection problem found. Worker input dispatch was not checked because this lightweight test does not send model-visible input as a side effect.",
    );
    observation.unknown(
        "config_bundles.sync",
        "No connection problem found. Config-bundle sync was not checked because this lightweight test does not upload bundles as a side effect.",
    );

    RemoteRuntimeTestResponse {
        workspace_id: api.config.workspace_id.clone(),
        runtime_id: remote.id.clone(),
        checked_at,
        state: observation.state().to_string(),
        protocol_version,
        compatibility_basis: "Connected to /v1/runtime and verified non-side-effecting worker-runtime HTTP endpoints. No incompatible operation was found; warning items below are unproven optional or side-effecting checks, not connection failures.".to_string(),
        capabilities: observation.capabilities,
        health_result: format!(
            "connected=true; runtime_status={:?}; available={}; incompatible={}; warnings={}",
            summary.runtime.status,
            observation.available_count,
            observation.incompatible_count,
            observation.unknown_count
        ),
        diagnostics: observation.diagnostics,
    }
}

fn remote_runtime_test_failed(
    api: &WorkspaceApi,
    remote: &RemoteRuntimeConfigFile,
    checked_at: String,
    code: impl Into<String>,
    message: impl Into<String>,
) -> RemoteRuntimeTestResponse {
    RemoteRuntimeTestResponse {
        workspace_id: api.config.workspace_id.clone(),
        runtime_id: remote.id.clone(),
        checked_at,
        state: "failed".to_string(),
        protocol_version: None,
        compatibility_basis: "worker-runtime lightweight HTTP compatibility probes".to_string(),
        capabilities: Vec::new(),
        health_result: "failed".to_string(),
        diagnostics: vec![settings_diagnostic(
            code,
            DiagnosticSeverity::Error,
            message,
        )],
    }
}

#[derive(Default)]
struct RuntimeCompatibilityObservation {
    capabilities: Vec<String>,
    diagnostics: Vec<RuntimeDiagnostic>,
    available_count: usize,
    incompatible_count: usize,
    unknown_count: usize,
}

impl RuntimeCompatibilityObservation {
    fn available(&mut self, operation: &str, message: impl Into<String>) {
        self.available_count += 1;
        self.capabilities.push(format!("{operation}:available"));
        self.diagnostics.push(settings_diagnostic(
            format!("{operation}.available"),
            DiagnosticSeverity::Info,
            message,
        ));
    }

    fn unknown(&mut self, operation: &str, message: impl Into<String>) {
        self.unknown_count += 1;
        self.capabilities.push(format!("{operation}:unknown"));
        self.diagnostics.push(settings_diagnostic(
            format!("{operation}.unknown"),
            DiagnosticSeverity::Warning,
            message,
        ));
    }

    fn incompatible(&mut self, operation: &str, diagnostic: RuntimeDiagnostic) {
        self.incompatible_count += 1;
        self.capabilities.push(format!("{operation}:incompatible"));
        self.diagnostics.push(diagnostic);
    }

    fn state(&self) -> &'static str {
        if self.incompatible_count > 0 {
            "incompatible"
        } else {
            "compatible"
        }
    }
}

fn remote_probe_url(
    remote: &RemoteRuntimeConfigFile,
    path: &str,
) -> std::result::Result<String, RuntimeDiagnostic> {
    let endpoint = remote.endpoint.trim();
    if !(endpoint.starts_with("http://") || endpoint.starts_with("https://")) {
        return Err(settings_diagnostic(
            "remote_runtime_endpoint_invalid",
            DiagnosticSeverity::Error,
            "Configured remote Runtime endpoint is not an absolute HTTP(S) URL.",
        ));
    }
    Ok(format!("{}{}", endpoint.trim_end_matches('/'), path))
}

async fn probe_remote_json(
    client: &reqwest::Client,
    url: String,
    operation: &'static str,
    label: &'static str,
) -> std::result::Result<serde_json::Value, RuntimeDiagnostic> {
    let response = client.get(url).send().await.map_err(|error| {
        let (code, message) = if error.is_timeout() {
            (
                format!("{operation}.timeout"),
                format!("Remote Runtime probe for {label} timed out."),
            )
        } else if error.is_connect() {
            (
                format!("{operation}.connect_failed"),
                format!("Remote Runtime probe for {label} could not connect."),
            )
        } else {
            (
                format!("{operation}.request_failed"),
                format!("Remote Runtime probe for {label} failed before a response was received."),
            )
        };
        settings_diagnostic(code, DiagnosticSeverity::Error, message)
    })?;

    if !response.status().is_success() {
        return Err(settings_diagnostic(
            format!("{operation}.http_status"),
            DiagnosticSeverity::Error,
            format!(
                "Remote Runtime probe for {label} returned HTTP status {}.",
                response.status().as_u16()
            ),
        ));
    }

    response.json::<serde_json::Value>().await.map_err(|_| {
        settings_diagnostic(
            format!("{operation}.malformed_json"),
            DiagnosticSeverity::Error,
            format!("Remote Runtime probe for {label} returned an unrecognized JSON payload."),
        )
    })
}

fn worker_launch_options_response(api: &WorkspaceApi) -> WorkerLaunchOptionsResponse {
    let runtimes = api
        .runtime
        .list_runtimes(api.config.max_records.min(200))
        .items
        .into_iter()
        .map(|runtime| {
            let built_in = runtime.runtime_id == EMBEDDED_WORKER_RUNTIME_ID;
            WorkerLaunchRuntimeOption {
                runtime_id: runtime.runtime_id,
                display_name: runtime.label,
                built_in,
                can_spawn_worker: runtime.capabilities.can_spawn_worker,
                working_directory_required: !built_in,
                status: runtime.status,
                diagnostics: runtime.diagnostics,
            }
        })
        .collect();
    WorkerLaunchOptionsResponse {
        workspace_id: api.config.workspace_id.clone(),
        runtimes,
        profiles: worker_profile_candidates_for_root(&api.config.workspace_root),
        repositories: working_directory_repository_options(api),
        working_directories: available_working_directory_summaries(api).unwrap_or_default(),
        diagnostics: Vec::new(),
    }
}

fn working_directory_repository_options(
    api: &WorkspaceApi,
) -> Vec<WorkingDirectoryRepositoryOption> {
    api.config
        .repositories
        .iter()
        .map(|repository| WorkingDirectoryRepositoryOption {
            id: repository.id.clone(),
            display_name: repository
                .display_name
                .clone()
                .unwrap_or_else(|| repository.id.clone()),
            default_selector: repository.default_selector.clone(),
        })
        .collect()
}

fn working_directory_summaries(api: &WorkspaceApi) -> ApiResult<Vec<WorkingDirectorySummary>> {
    let _ = sync_all_runtime_workdir_observations(api);
    let records = api
        .store
        .list_workdir_registry(&api.config.workspace_id, 200)?;
    Ok(records
        .iter()
        .map(workdir_summary_from_record)
        .collect::<Vec<_>>())
}

fn available_working_directory_summaries(
    api: &WorkspaceApi,
) -> ApiResult<Vec<WorkingDirectorySummary>> {
    let limit = api.config.max_records.min(200);
    for worker in api.runtime.list_workers(limit).items {
        let _ = sync_worker_observation(api, &worker);
    }
    let records = working_directory_summaries(api)?;
    let mut available = Vec::new();
    for summary in records {
        if summary.status != WorkingDirectoryStatusKind::Active
            || summary.cleanliness.as_deref() != Some("clean")
        {
            continue;
        }
        let links = api.store.list_workdir_worker_links(
            &api.config.workspace_id,
            summary.working_directory_id.as_str(),
        )?;
        if links.is_empty() && summary.primary_worker_id.is_none() {
            available.push(summary);
        }
    }
    Ok(available)
}

fn runtime_working_directory_summaries(
    api: &WorkspaceApi,
    runtime_id: &str,
) -> ApiResult<(Vec<WorkingDirectorySummary>, Vec<RuntimeDiagnostic>)> {
    let diagnostics = sync_runtime_workdir_observations(api, runtime_id)?;
    let records = api
        .store
        .list_workdir_registry(&api.config.workspace_id, 200)?;
    let items = records
        .iter()
        .filter(|record| record.runtime_id == runtime_id)
        .map(workdir_summary_from_record)
        .collect::<Vec<_>>();
    Ok((items, diagnostics))
}

static BACKEND_WORKDIR_ID_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn now_registry_timestamp() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().to_string())
        .unwrap_or_else(|_| "0".to_string())
}

fn next_backend_workdir_id(_repository_id: &str) -> String {
    let timestamp_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0);
    let sequence = BACKEND_WORKDIR_ID_SEQUENCE.fetch_add(1, Ordering::Relaxed) & 0x00ff_ffff;
    format!("{timestamp_ms:013x}{sequence:06x}")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WorkerRegistryDisplayNamePolicy {
    PreserveExisting,
    UseProvided,
}

fn record_worker_summary(
    api: &WorkspaceApi,
    worker: &WorkerSummary,
    display_name: &str,
    profile: Option<String>,
    display_name_policy: WorkerRegistryDisplayNamePolicy,
) -> ApiResult<WorkerRegistryRecord> {
    let timestamp = now_registry_timestamp();
    let runtime_worker_id = parse_runtime_worker_id_for_registry(worker.worker_id.as_str())?;
    let existing = api.store.get_worker_registry(
        &api.config.workspace_id,
        worker.runtime_id.as_str(),
        runtime_worker_id,
    )?;
    let display_name = match (display_name_policy, existing.as_ref()) {
        (WorkerRegistryDisplayNamePolicy::PreserveExisting, Some(record)) => {
            record.display_name.clone()
        }
        _ => display_name.to_string(),
    };
    let record = WorkerRegistryRecord {
        workspace_id: api.config.workspace_id.clone(),
        runtime_id: worker.runtime_id.as_str().to_string(),
        runtime_worker_id,
        display_name,
        profile,
        retention_state: existing
            .as_ref()
            .map(|record| record.retention_state.clone())
            .unwrap_or_else(|| "normal".to_string()),
        transcript_ref: Some(format!(
            "runtime://{}/workers/{}/transcript",
            worker.runtime_id.as_str(),
            worker.worker_id.as_str()
        )),
        session_ref: None,
        summary_ref: None,
        diagnostics_ref: None,
        created_at: timestamp.clone(),
        updated_at: timestamp,
    };
    api.store.upsert_worker_registry(&record)?;
    Ok(api
        .store
        .get_worker_registry(
            &api.config.workspace_id,
            worker.runtime_id.as_str(),
            runtime_worker_id,
        )?
        .unwrap_or(record))
}

fn worker_summary_from_registry(record: &WorkerRegistryRecord) -> WorkerSummary {
    WorkerSummary {
        worker_id: record.runtime_worker_id.to_string(),
        runtime_id: record.runtime_id.clone(),
        host_id: "backend-registry".to_string(),
        role: None,
        label: record.display_name.clone(),
        state: "missing".to_string(),
        last_seen_at: Some(record.updated_at.clone()),
        pinned: record.retention_state == "pinned",
        retention_state: record.retention_state.clone(),
        capabilities: WorkerCapabilitySummary {
            can_stop: false,
            can_spawn_followup: false,
        },
        workspace: WorkerWorkspaceSummary {
            visibility: "backend_registry".to_string(),
            identity: record.workspace_id.clone(),
        },
        profile: record.profile.clone(),
        implementation: WorkerImplementationSummary {
            kind: "backend_worker_registry".to_string(),
            display_hint: "Missing Worker".to_string(),
        },
        working_directory: None,
        diagnostics: vec![RuntimeDiagnostic {
            code: "backend_worker_missing".to_string(),
            severity: DiagnosticSeverity::Info,
            message:
                "Worker is preserved in the Backend registry but the Runtime did not find it by id"
                    .to_string(),
        }],
    }
}

fn merge_worker_registry_projection(
    live: Option<&WorkerSummary>,
    record: &WorkerRegistryRecord,
    links: Vec<WorkerWorkdirLinkRecord>,
    workdirs: &[WorkdirRegistryRecord],
) -> WorkerSummary {
    let mut summary = live
        .cloned()
        .unwrap_or_else(|| worker_summary_from_registry(record));
    summary.label = record.display_name.clone();
    summary.profile = record.profile.clone();
    summary.pinned = record.retention_state == "pinned";
    summary.retention_state = record.retention_state.clone();
    summary.working_directory = links.iter().find_map(|link| {
        workdirs
            .iter()
            .find(|workdir| workdir.workdir_id == link.workdir_id)
            .map(|workdir| workdir_summary_from_record(workdir))
    });
    summary
}

fn sync_worker_observation(
    api: &WorkspaceApi,
    worker: &WorkerSummary,
) -> ApiResult<WorkerRegistryRecord> {
    let record = record_worker_summary(
        api,
        worker,
        worker.label.as_str(),
        worker.profile.clone(),
        WorkerRegistryDisplayNamePolicy::PreserveExisting,
    )?;
    if let Some(working_directory) = worker.working_directory.as_ref() {
        let workdir_record =
            workdir_record_from_summary(api, worker.runtime_id.as_str(), working_directory);
        api.store.upsert_workdir_registry(&workdir_record)?;
        link_worker_to_workdir(api, &record, &working_directory.working_directory_id)?;
    }
    Ok(record)
}

fn upsert_pending_backend_workdir(
    api: &WorkspaceApi,
    runtime_id: &str,
    request: &mut WorkingDirectoryRequest,
) -> ApiResult<String> {
    let workdir_id = request
        .backend_workdir_id
        .clone()
        .unwrap_or_else(|| next_backend_workdir_id(&request.repository.id));
    request.backend_workdir_id = Some(workdir_id.clone());
    let timestamp = now_registry_timestamp();
    api.store.upsert_workdir_registry(&WorkdirRegistryRecord {
        workspace_id: api.config.workspace_id.clone(),
        workdir_id: workdir_id.clone(),
        runtime_id: runtime_id.to_string(),
        repository_id: request.repository.id.clone(),
        selector: request
            .repository
            .selector
            .as_ref()
            .map(|selector| selector.as_ref().to_string()),
        resolved_commit: None,
        materialization_status: "pending".to_string(),
        cleanliness: "unknown".to_string(),
        created_at: timestamp.clone(),
        updated_at: timestamp,
    })?;
    Ok(workdir_id)
}

fn sync_runtime_workdir_observations(
    api: &WorkspaceApi,
    runtime_id: &str,
) -> ApiResult<Vec<RuntimeDiagnostic>> {
    let response = api
        .runtime
        .list_working_directories(runtime_id)
        .map_err(|err| err.into_error())?;
    let mut observed = std::collections::BTreeSet::new();
    for status in &response.items {
        observed.insert(status.summary.working_directory_id.clone());
        if status.summary.status == WorkingDirectoryStatusKind::NotFound {
            api.store.delete_workdir_registry(
                &api.config.workspace_id,
                &status.summary.working_directory_id,
            )?;
            continue;
        }
        let existing = api.store.get_workdir_registry(
            &api.config.workspace_id,
            &status.summary.working_directory_id,
        )?;
        let mut record = workdir_record_from_summary(api, runtime_id, &status.summary);
        preserve_workdir_identity_for_corrupted_summary(&mut record, existing.as_ref());
        api.store.upsert_workdir_registry(&record)?;
    }
    for mut record in api
        .store
        .list_workdir_registry(&api.config.workspace_id, 500)?
        .into_iter()
        .filter(|record| record.runtime_id == runtime_id && !observed.contains(&record.workdir_id))
    {
        match api
            .runtime
            .working_directory(runtime_id, record.workdir_id.as_str())
        {
            Ok(result) => {
                if let Some(status) = result.working_directory {
                    if status.summary.status == WorkingDirectoryStatusKind::NotFound {
                        api.store.delete_workdir_registry(
                            &api.config.workspace_id,
                            record.workdir_id.as_str(),
                        )?;
                    } else {
                        let mut updated =
                            workdir_record_from_summary(api, runtime_id, &status.summary);
                        preserve_workdir_identity_for_corrupted_summary(
                            &mut updated,
                            Some(&record),
                        );
                        api.store.upsert_workdir_registry(&updated)?;
                    }
                } else {
                    record.materialization_status =
                        workdir_status_from_runtime_miss(result.diagnostics.as_slice()).to_string();
                    record.cleanliness = "unknown".to_string();
                    record.updated_at = now_registry_timestamp();
                    api.store.upsert_workdir_registry(&record)?;
                }
            }
            Err(_) => {
                record.materialization_status = "unknown".to_string();
                record.cleanliness = "unknown".to_string();
                record.updated_at = now_registry_timestamp();
                api.store.upsert_workdir_registry(&record)?;
            }
        }
    }
    Ok(response.diagnostics)
}

fn workdir_status_from_runtime_miss(diagnostics: &[RuntimeDiagnostic]) -> &'static str {
    if diagnostics
        .iter()
        .any(|diagnostic| diagnostic.code == "working_directory_not_found")
    {
        "not_found"
    } else {
        "unknown"
    }
}

fn sync_all_runtime_workdir_observations(api: &WorkspaceApi) -> Vec<RuntimeDiagnostic> {
    let mut diagnostics = Vec::new();
    let runtimes = api.runtime.list_runtimes(api.config.max_records.min(200));
    for runtime in runtimes.items {
        if runtime.capabilities.supports_worktrees {
            match sync_runtime_workdir_observations(api, runtime.runtime_id.as_str()) {
                Ok(mut runtime_diagnostics) => diagnostics.append(&mut runtime_diagnostics),
                Err(err) => diagnostics.extend(err.diagnostics),
            }
        }
    }
    diagnostics
}

fn sync_linked_workdir_after_worker_stop(
    api: &WorkspaceApi,
    runtime_id: &str,
    worker_record: &WorkerRegistryRecord,
) -> ApiResult<()> {
    let links = api.store.list_worker_workdir_links(
        &api.config.workspace_id,
        worker_record.runtime_id.as_str(),
        worker_record.runtime_worker_id,
    )?;
    for link in links {
        let result = api
            .runtime
            .working_directory(runtime_id, link.workdir_id.as_str())
            .map_err(|err| err.into_error())?;
        if let Some(status) = result.working_directory {
            let record = workdir_record_from_summary(api, runtime_id, &status.summary);
            api.store.upsert_workdir_registry(&record)?;
        } else if let Some(mut record) = api
            .store
            .get_workdir_registry(&api.config.workspace_id, link.workdir_id.as_str())?
        {
            record.materialization_status =
                workdir_status_from_runtime_miss(result.diagnostics.as_slice()).to_string();
            record.cleanliness = "unknown".to_string();
            record.updated_at = now_registry_timestamp();
            api.store.upsert_workdir_registry(&record)?;
        }
    }
    Ok(())
}

fn workdir_record_from_summary(
    api: &WorkspaceApi,
    runtime_id: &str,
    summary: &WorkingDirectorySummary,
) -> WorkdirRegistryRecord {
    let timestamp = now_registry_timestamp();
    WorkdirRegistryRecord {
        workspace_id: api.config.workspace_id.clone(),
        workdir_id: summary.working_directory_id.clone(),
        runtime_id: runtime_id.to_string(),
        repository_id: summary.repository_id.clone(),
        selector: summary.requested_selector.clone(),
        resolved_commit: summary.resolved_commit.clone(),
        materialization_status: match summary.status {
            WorkingDirectoryStatusKind::Active => "present",
            WorkingDirectoryStatusKind::CleanupPending => "pending",
            WorkingDirectoryStatusKind::Corrupted => "corrupted",
            WorkingDirectoryStatusKind::NotFound => "not_found",
            WorkingDirectoryStatusKind::Unknown => "unknown",
        }
        .to_string(),
        cleanliness: summary
            .cleanliness
            .clone()
            .unwrap_or_else(|| "unknown".to_string()),
        created_at: timestamp.clone(),
        updated_at: timestamp,
    }
}

fn preserve_workdir_identity_for_corrupted_summary(
    record: &mut WorkdirRegistryRecord,
    existing: Option<&WorkdirRegistryRecord>,
) {
    if record.materialization_status != "corrupted" {
        return;
    }
    let Some(existing) = existing else {
        return;
    };
    if record.repository_id == "unknown" {
        record.repository_id = existing.repository_id.clone();
    }
    if record.selector.is_none() {
        record.selector = existing.selector.clone();
    }
    if record.resolved_commit.is_none() {
        record.resolved_commit = existing.resolved_commit.clone();
    }
}

fn workdir_summary_from_record(record: &WorkdirRegistryRecord) -> WorkingDirectorySummary {
    let status = match record.materialization_status.as_str() {
        "present" => WorkingDirectoryStatusKind::Active,
        "pending" => WorkingDirectoryStatusKind::CleanupPending,
        "corrupted" => WorkingDirectoryStatusKind::Corrupted,
        "not_found" | "missing" => WorkingDirectoryStatusKind::NotFound,
        "unknown" => WorkingDirectoryStatusKind::Unknown,
        _ => WorkingDirectoryStatusKind::Unknown,
    };
    WorkingDirectorySummary {
        working_directory_id: record.workdir_id.clone(),
        repository_id: record.repository_id.clone(),
        requested_selector: record.selector.clone(),
        materializer_kind: MaterializerKind::LocalGitWorktree,
        resolved_commit: record.resolved_commit.clone(),
        resolved_tree: None,
        cleanup_target: Some(worker_runtime::catalog::WorkingDirectoryCleanupTarget {
            kind: "local_git_worktree".to_string(),
            working_directory_id: record.workdir_id.clone(),
            repository_id: record.repository_id.clone(),
        }),
        status,
        cleanliness: Some(record.cleanliness.clone()),
        primary_worker_id: None,
    }
}

fn link_worker_to_workdir(
    api: &WorkspaceApi,
    worker_record: &WorkerRegistryRecord,
    workdir_id: &str,
) -> ApiResult<()> {
    let timestamp = now_registry_timestamp();
    api.store
        .upsert_worker_workdir_link(&WorkerWorkdirLinkRecord {
            workspace_id: api.config.workspace_id.clone(),
            runtime_id: worker_record.runtime_id.clone(),
            runtime_worker_id: worker_record.runtime_worker_id.clone(),
            workdir_id: workdir_id.to_string(),
            role: "primary_cwd".to_string(),
            linked_at: timestamp,
            unlinked_at: None,
        })?;
    Ok(())
}

fn validate_working_directory_claim_for_browser(
    claim: Option<&WorkingDirectoryClaim>,
) -> ApiResult<()> {
    let Some(claim) = claim else {
        return Ok(());
    };
    if let Some(relative_cwd) = claim.relative_cwd.as_deref() {
        let path = Path::new(relative_cwd);
        if path.is_absolute()
            || path
                .components()
                .any(|component| !matches!(component, Component::CurDir | Component::Normal(_)))
        {
            return Err(ApiError::with_diagnostics(
                Error::RuntimeOperationFailed {
                    runtime_id: EMBEDDED_WORKER_RUNTIME_ID.to_string(),
                    code: "working_directory_relative_cwd_invalid".to_string(),
                    message: "working directory relative_cwd must stay inside the Runtime-owned working directory".to_string(),
                },
                vec![RuntimeDiagnostic {
                    code: "working_directory_relative_cwd_invalid".to_string(),
                    severity: DiagnosticSeverity::Error,
                    message: "relative_cwd must be a relative path without parent traversal".to_string(),
                }],
            ));
        }
    }
    Ok(())
}

fn working_directory_request_for_browser(
    api: &WorkspaceApi,
    request: BrowserWorkingDirectoryCreateRequest,
) -> ApiResult<WorkingDirectoryRequest> {
    let repository = api
        .config
        .repositories
        .iter()
        .find(|repository| repository.id == request.repository_id)
        .ok_or_else(|| Error::UnknownRepository(request.repository_id.clone()))?;
    let selector = request
        .selector
        .or_else(|| repository.default_selector.clone())
        .filter(|selector| !selector.trim().is_empty());
    Ok(WorkingDirectoryRequest {
        repository: WorkingDirectoryRepository {
            id: repository.id.clone(),
            provider: "git".to_string(),
            uri: repository.path.to_string_lossy().to_string(),
            local_path: Some(repository.path.clone()),
            selector: selector.map(RuntimeRepositorySelector),
        },
        materializer: MaterializerKind::LocalGitWorktree,
        backend_workdir_id: None,
    })
}

#[cfg(test)]
fn worker_profile_candidates() -> Vec<WorkerLaunchProfileCandidate> {
    worker_profile_candidates_for_root(Path::new("."))
        .into_iter()
        .filter(|candidate| candidate.id == "builtin:coder" || candidate.id == "runtime_default")
        .collect()
}

fn worker_profile_candidates_for_root(workspace_root: &Path) -> Vec<WorkerLaunchProfileCandidate> {
    let mut candidates = vec![
        WorkerLaunchProfileCandidate {
            id: "builtin:coder".to_string(),
            label: "Coding Worker".to_string(),
            description: "Built-in coding role profile for implementation work.".to_string(),
        },
        WorkerLaunchProfileCandidate {
            id: "runtime_default".to_string(),
            label: "Runtime default".to_string(),
            description: "Use the selected Runtime's default profile.".to_string(),
        },
    ];
    candidates.extend(
        crate::profile_settings::project_profile_candidates(workspace_root)
            .into_iter()
            .filter(|profile| profile.diagnostics.is_empty())
            .map(|profile| WorkerLaunchProfileCandidate {
                id: profile.profile_id,
                label: profile.label,
                description: profile
                    .description
                    .unwrap_or_else(|| "Workspace Decodal profile source.".to_string()),
            }),
    );
    candidates
}

fn profile_selector_for_candidate(profile: &str) -> Option<ProfileSelector> {
    crate::profile_settings::selector_for_builtin_candidate(profile)
        .filter(|_| matches!(profile, "builtin:coder" | "runtime_default"))
}

fn profile_selector_for_candidate_with_root(
    workspace_root: &Path,
    profile: &str,
) -> Option<ProfileSelector> {
    if profile_selector_for_candidate(profile).is_some() {
        profile_selector_for_candidate(profile)
    } else if crate::profile_settings::is_profile_candidate(workspace_root, profile) {
        crate::profile_settings::selector_for_builtin_candidate(profile)
    } else {
        None
    }
}

fn parse_runtime_worker_id_for_registry(worker_id: &str) -> ApiResult<u64> {
    worker_id.parse::<u64>().map_err(|_| {
        settings_bad_request(
            "workspace_worker_id_invalid",
            "Runtime Worker id must be an unsigned integer",
        )
    })
}

fn sanitize_worker_display_name(value: &str) -> Option<String> {
    let display_name = value.trim();
    if display_name.chars().any(char::is_control) {
        None
    } else if display_name.is_empty() {
        Some("Coding Worker".to_string())
    } else {
        Some(display_name.chars().take(80).collect())
    }
}

fn encode_path_segment(value: &str) -> String {
    value
        .bytes()
        .flat_map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                vec![byte as char]
            }
            _ => format!("%{byte:02X}").chars().collect(),
        })
        .collect()
}

fn worker_create_not_accepted_error(
    runtime_id: String,
    mut diagnostics: Vec<RuntimeDiagnostic>,
) -> ApiError {
    diagnostics.push(settings_diagnostic(
        "workspace_worker_create_not_accepted",
        DiagnosticSeverity::Error,
        "Runtime did not accept worker creation; see diagnostics for sanitized Runtime compatibility details.",
    ));
    ApiError::with_diagnostics(
        Error::RuntimeOperationFailed {
            runtime_id,
            code: "workspace_worker_create_failed".to_string(),
            message: "Runtime did not accept worker creation".to_string(),
        },
        diagnostics,
    )
}

fn settings_bad_request(code: &'static str, message: &'static str) -> ApiError {
    Error::RuntimeOperationFailed {
        runtime_id: "workspace-backend".to_string(),
        code: code.to_string(),
        message: message.to_string(),
    }
    .into()
}

fn settings_diagnostic(
    code: impl Into<String>,
    severity: DiagnosticSeverity,
    message: impl Into<String>,
) -> RuntimeDiagnostic {
    RuntimeDiagnostic {
        code: code.into(),
        severity,
        message: message.into(),
    }
}

fn sanitize_backend_error(message: &str) -> String {
    message.to_string()
}

fn repository_diagnostics(
    diagnostics: Vec<crate::repositories::RepositoryDiagnostic>,
) -> Vec<RuntimeDiagnostic> {
    diagnostics
        .into_iter()
        .map(|diagnostic| RuntimeDiagnostic {
            code: diagnostic.code,
            severity: match diagnostic.severity.as_str() {
                "error" => DiagnosticSeverity::Error,
                "warning" => DiagnosticSeverity::Warning,
                _ => DiagnosticSeverity::Info,
            },
            message: diagnostic.message,
        })
        .collect()
}

fn repository_lookup<T>(result: std::result::Result<T, RepositoryLookupError>) -> ApiResult<T> {
    result.map_err(|error| match error {
        RepositoryLookupError::UnknownRepository { id } => {
            let message = format!("repository `{id}` is not configured for this workspace");
            ApiError::with_diagnostics(
                Error::RuntimeOperationFailed {
                    runtime_id: "workspace-repository-registry".to_string(),
                    code: "repository_not_configured".to_string(),
                    message: message.clone(),
                },
                vec![RuntimeDiagnostic {
                    code: "repository_not_configured".to_string(),
                    severity: DiagnosticSeverity::Error,
                    message,
                }],
            )
        }
        RepositoryLookupError::UnsupportedProvider { id, provider } => {
            let message = format!(
                "repository `{id}` uses unsupported provider `{provider}` for this operation"
            );
            ApiError::with_diagnostics(
                Error::RuntimeOperationFailed {
                    runtime_id: "workspace-repository-registry".to_string(),
                    code: "repository_provider_unsupported".to_string(),
                    message: message.clone(),
                },
                vec![RuntimeDiagnostic {
                    code: "repository_provider_unsupported".to_string(),
                    severity: DiagnosticSeverity::Error,
                    message,
                }],
            )
        }
    })
}

fn ticket_kanban_columns(items: Vec<TicketSummary>) -> Vec<TicketKanbanColumn> {
    let mut columns = vec![
        TicketKanbanColumn {
            state: "planning".to_string(),
            items: Vec::new(),
        },
        TicketKanbanColumn {
            state: "ready".to_string(),
            items: Vec::new(),
        },
        TicketKanbanColumn {
            state: "queued".to_string(),
            items: Vec::new(),
        },
        TicketKanbanColumn {
            state: "inprogress".to_string(),
            items: Vec::new(),
        },
        TicketKanbanColumn {
            state: "done".to_string(),
            items: Vec::new(),
        },
        TicketKanbanColumn {
            state: "closed".to_string(),
            items: Vec::new(),
        },
        TicketKanbanColumn {
            state: "other".to_string(),
            items: Vec::new(),
        },
    ];
    for item in items {
        let index = match item.state.as_str() {
            "planning" => 0,
            "ready" => 1,
            "queued" => 2,
            "inprogress" => 3,
            "done" => 4,
            "closed" => 5,
            _ => 6,
        };
        columns[index].items.push(item);
    }
    columns
}

async fn static_or_spa_fallback(State(api): State<WorkspaceApi>, uri: Uri) -> Response {
    if uri.path().starts_with("/api/") || uri.path() == "/api" {
        return (
            StatusCode::NOT_FOUND,
            [(CONTENT_TYPE, "application/json")],
            Json(serde_json::json!({
                "error": "not_found",
                "message": "unknown api route"
            }))
            .to_string(),
        )
            .into_response();
    }

    if let Some(workspace_id) = workspace_id_from_ui_path(uri.path()) {
        if workspace_id != api.workspace_id() {
            return workspace_id_mismatch_error().into_response();
        }
    }

    if let Some(location) =
        unscoped_workspace_ui_redirect(uri.path(), uri.query(), api.workspace_id())
    {
        return (StatusCode::TEMPORARY_REDIRECT, [(LOCATION, location)]).into_response();
    }

    let Some(static_root) = api.config.static_assets_dir.as_ref() else {
        return StatusCode::NOT_FOUND.into_response();
    };

    match read_static_or_index(static_root, uri.path()).await {
        Ok(StaticAsset {
            bytes,
            content_type,
        }) => (StatusCode::OK, [(CONTENT_TYPE, content_type)], bytes).into_response(),
        Err(error) => {
            tracing::debug!(%error, path = %uri.path(), "failed to serve static asset");
            StatusCode::NOT_FOUND.into_response()
        }
    }
}

fn unscoped_workspace_ui_redirect(
    path: &str,
    query: Option<&str>,
    workspace_id: &str,
) -> Option<String> {
    let scoped_tail = if path == "/" {
        ""
    } else if ["/repositories", "/objectives", "/settings", "/runtimes"]
        .iter()
        .any(|prefix| path == *prefix || path.starts_with(&format!("{prefix}/")))
    {
        path
    } else {
        return None;
    };

    let mut location = format!("/w/{}{}", encode_path_segment(workspace_id), scoped_tail);
    if let Some(query) = query.filter(|query| !query.is_empty()) {
        location.push('?');
        location.push_str(query);
    }
    Some(location)
}

fn workspace_id_from_ui_path(path: &str) -> Option<&str> {
    let tail = path.strip_prefix("/w/")?;
    let workspace_id = tail.split('/').next().unwrap_or_default();
    if workspace_id.is_empty() {
        None
    } else {
        Some(workspace_id)
    }
}

struct StaticAsset {
    bytes: Vec<u8>,
    content_type: &'static str,
}

async fn read_static_or_index(root: &Path, request_path: &str) -> Result<StaticAsset> {
    let candidate = safe_static_candidate(root, request_path)?;
    let file = if tokio::fs::metadata(&candidate)
        .await
        .map(|m| m.is_file())
        .unwrap_or(false)
    {
        candidate
    } else {
        root.join("index.html")
    };
    let content_type = content_type_for(&file);
    let bytes = tokio::fs::read(file).await?;
    Ok(StaticAsset {
        bytes,
        content_type,
    })
}

fn safe_static_candidate(root: &Path, request_path: &str) -> Result<PathBuf> {
    let mut path = root.to_path_buf();
    let clean = request_path.trim_start_matches('/');
    if clean.is_empty() {
        path.push("index.html");
        return Ok(path);
    }
    for component in Path::new(clean).components() {
        match component {
            Component::Normal(part) => path.push(part),
            Component::CurDir => {}
            _ => return Err(Error::Store("static path escape rejected".to_string())),
        }
    }
    Ok(path)
}

fn content_type_for(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default()
    {
        "css" => "text/css; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "html" | "" => "text/html; charset=utf-8",
        _ => "application/octet-stream",
    }
}

type ApiResult<T> = std::result::Result<T, ApiError>;

fn workspace_id_mismatch_error() -> ApiError {
    let message = "workspace id does not match this Workspace backend".to_string();
    ApiError::with_diagnostics(
        Error::WorkspaceIdMismatch,
        vec![RuntimeDiagnostic {
            code: "workspace_id_mismatch".to_string(),
            severity: DiagnosticSeverity::Error,
            message,
        }],
    )
}

struct ApiError {
    error: Error,
    diagnostics: Vec<RuntimeDiagnostic>,
}

impl From<Error> for ApiError {
    fn from(error: Error) -> Self {
        let diagnostics = match &error {
            Error::RuntimeOperationFailed { code, message, .. } => vec![RuntimeDiagnostic {
                code: code.clone(),
                severity: DiagnosticSeverity::Error,
                message: sanitize_backend_error(message),
            }],
            _ => Vec::new(),
        };
        Self { error, diagnostics }
    }
}

impl ApiError {
    fn with_diagnostics(error: Error, diagnostics: Vec<RuntimeDiagnostic>) -> Self {
        Self { error, diagnostics }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match &self.error {
            Error::InvalidRuntimeIdentifier { .. } => StatusCode::BAD_REQUEST,
            Error::InvalidRecordId(_)
            | Error::MissingFrontmatter(_)
            | Error::UnknownHost(_)
            | Error::UnknownRuntime(_)
            | Error::UnknownWorker { .. }
            | Error::UnknownRepository(_)
            | Error::WorkspaceIdMismatch => StatusCode::NOT_FOUND,
            Error::RuntimeOperationFailed { code, .. } if code == "skill_not_found" => {
                StatusCode::NOT_FOUND
            }
            Error::RuntimeCapabilityUnsupported { .. } => StatusCode::NOT_IMPLEMENTED,
            Error::RuntimeOperationFailed { code, .. } if code == "repository_not_configured" => {
                StatusCode::NOT_FOUND
            }
            Error::RuntimeOperationFailed { code, .. }
                if code == "repository_provider_unsupported" =>
            {
                StatusCode::BAD_REQUEST
            }
            Error::RuntimeOperationFailed { code, .. } if code == "remote_runtime_auth_failed" => {
                StatusCode::UNAUTHORIZED
            }
            Error::RuntimeOperationFailed { code, .. } if code == "remote_runtime_timeout" => {
                StatusCode::GATEWAY_TIMEOUT
            }
            Error::RuntimeOperationFailed { code, .. } if code == "remote_runtime_unsupported" => {
                StatusCode::NOT_IMPLEMENTED
            }
            Error::RuntimeOperationFailed { code, .. }
                if code == "profile_registry_revision_conflict"
                    || code == "profile_source_revision_conflict"
                    || code == "workspace_metadata_revision_conflict"
                    || code == "workspace_cleanup_plan_stale"
                    || code == "workspace_cleanup_worker_blocked"
                    || code == "workspace_cleanup_workdir_blocked"
                    || code == "workspace_cleanup_worker_pinned" =>
            {
                StatusCode::CONFLICT
            }
            Error::RuntimeOperationFailed { code, .. }
                if code == "unknown_profile_source" || code == "unknown_profile_selector" =>
            {
                StatusCode::NOT_FOUND
            }
            Error::RuntimeOperationFailed { code, .. }
                if code == "workspace_display_name_invalid" || code.starts_with("profile_") =>
            {
                StatusCode::BAD_REQUEST
            }
            Error::RuntimeOperationFailed { code, .. } if code.ends_with("_blocked") => {
                StatusCode::CONFLICT
            }
            Error::RuntimeOperationFailed { code, .. }
                if code.starts_with("workspace_settings_")
                    || code.starts_with("invalid_")
                    || code.starts_with("unsupported_worker_profile")
                    || code.starts_with("working_directory_")
                    || code.starts_with("workspace_cleanup_")
                    || code == "workspace_worker_workdir_required"
                    || code.ends_with("_already_exists")
                    || code.ends_with("_not_config_managed")
                    || code.ends_with("_unsupported") =>
            {
                StatusCode::BAD_REQUEST
            }
            Error::RuntimeOperationFailed { .. } => StatusCode::BAD_GATEWAY,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        let response_message = match &self.error {
            Error::RuntimeOperationFailed { code, message, .. } => {
                format!("{code}: {}", sanitize_backend_error(message))
            }
            _ => sanitize_backend_error(&self.error.to_string()),
        };
        (
            status,
            [(CONTENT_TYPE, "application/json")],
            Json(serde_json::json!({
                "error": status.canonical_reason().unwrap_or("error"),
                "message": response_message,
                "diagnostics": self.diagnostics,
            }))
            .to_string(),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{Body, to_bytes};
    use axum::http::Request;
    use futures::{SinkExt, StreamExt};
    use serde_json::{Value, json};
    use std::{fs, sync::Arc};
    use tokio_tungstenite::connect_async;
    use tokio_tungstenite::tungstenite::Message;
    use tower::ServiceExt;
    use worker_runtime::resource::BackendResourceClient;
    use worker_runtime::working_directory::WorkingDirectoryMaterializer;

    use crate::hosts::{
        TicketWorkerRole, WorkerInputKind, WorkerOperationState, WorkerSpawnAcceptanceRequirement,
        WorkerSpawnIntent,
    };
    use crate::store::SqliteWorkspaceStore;

    const TEST_WORKSPACE_ID: &str = "0192f0e8-4d84-7d6e-a000-000000000001";
    const TEST_REPOSITORY_ID: &str = "main";
    const TEST_CREATED_AT: &str = "2026-06-23T06:43:28Z";

    #[test]
    fn backend_worker_projection_preserves_missing_rows_links_and_redacts_paths() {
        let worker = WorkerRegistryRecord {
            workspace_id: "workspace-1".to_string(),
            runtime_id: "embedded".to_string(),
            runtime_worker_id: 1,
            display_name: "Missing Worker".to_string(),
            profile: Some("builtin:coder".to_string()),
            retention_state: "pinned".to_string(),
            transcript_ref: Some("runtime://embedded/workers/worker-1/transcript".to_string()),
            session_ref: None,
            summary_ref: None,
            diagnostics_ref: None,
            created_at: "1".to_string(),
            updated_at: "2".to_string(),
        };
        let workdir = WorkdirRegistryRecord {
            workspace_id: "workspace-1".to_string(),
            workdir_id: "0000019a00000000000".to_string(),
            runtime_id: "embedded".to_string(),
            repository_id: "repo".to_string(),
            selector: Some("develop".to_string()),
            resolved_commit: Some("abcdef".to_string()),
            materialization_status: "missing".to_string(),
            cleanliness: "clean".to_string(),
            created_at: "1".to_string(),
            updated_at: "3".to_string(),
        };
        let link = WorkerWorkdirLinkRecord {
            workspace_id: "workspace-1".to_string(),
            runtime_id: worker.runtime_id.clone(),
            runtime_worker_id: worker.runtime_worker_id.clone(),
            workdir_id: workdir.workdir_id.clone(),
            role: "primary_cwd".to_string(),
            linked_at: "4".to_string(),
            unlinked_at: None,
        };

        let projected = merge_worker_registry_projection(None, &worker, vec![link], &[workdir]);

        assert_eq!(projected.state, "missing");
        assert_eq!(
            projected.working_directory.as_ref().unwrap().status,
            WorkingDirectoryStatusKind::NotFound
        );
        let serialized = serde_json::to_string(&projected).unwrap();
        assert!(!serialized.contains("/tmp/"));
        assert!(!serialized.contains("materialized_path"));
    }

    #[tokio::test]
    async fn workspace_workdir_summaries_include_runtime_observed_rows() {
        let dir = tempfile::tempdir().unwrap();
        let api = test_api(dir.path()).await;
        api.store
            .upsert_workdir_registry(&WorkdirRegistryRecord {
                workspace_id: TEST_WORKSPACE_ID.to_string(),
                workdir_id: "managed".to_string(),
                runtime_id: EMBEDDED_WORKER_RUNTIME_ID.to_string(),
                repository_id: "repo".to_string(),
                selector: None,
                resolved_commit: None,
                materialization_status: "present".to_string(),
                cleanliness: "clean".to_string(),
                created_at: "1".to_string(),
                updated_at: "1".to_string(),
            })
            .unwrap();
        api.store
            .upsert_workdir_registry(&WorkdirRegistryRecord {
                workspace_id: TEST_WORKSPACE_ID.to_string(),
                workdir_id: "runtime-direct".to_string(),
                runtime_id: EMBEDDED_WORKER_RUNTIME_ID.to_string(),
                repository_id: "repo".to_string(),
                selector: None,
                resolved_commit: None,
                materialization_status: "present".to_string(),
                cleanliness: "unknown".to_string(),
                created_at: "1".to_string(),
                updated_at: "2".to_string(),
            })
            .unwrap();

        let summaries = working_directory_summaries(&api)
            .unwrap_or_else(|err| panic!("working_directory_summaries failed: {}", err.error));
        let ids = summaries
            .iter()
            .map(|summary| summary.working_directory_id.as_str())
            .collect::<Vec<_>>();
        assert!(ids.contains(&"managed"));
        assert!(ids.contains(&"runtime-direct"));

        let (runtime_projection, _) =
            runtime_working_directory_summaries(&api, EMBEDDED_WORKER_RUNTIME_ID).unwrap_or_else(
                |err| panic!("runtime_working_directory_summaries failed: {}", err.error),
            );
        assert!(
            runtime_projection
                .iter()
                .any(|summary| summary.working_directory_id == "runtime-direct")
        );
    }
    #[test]
    fn unmanaged_runtime_workdir_projection_is_typed_and_diagnostic_safe() {
        let workdir = WorkdirRegistryRecord {
            workspace_id: "workspace-1".to_string(),
            workdir_id: "runtime-direct".to_string(),
            runtime_id: "embedded".to_string(),
            repository_id: "repo".to_string(),
            selector: None,
            resolved_commit: None,
            materialization_status: "present".to_string(),
            cleanliness: "unknown".to_string(),
            created_at: "1".to_string(),
            updated_at: "2".to_string(),
        };

        let projected = workdir_summary_from_record(&workdir);

        assert_eq!(projected.status, WorkingDirectoryStatusKind::Active);
        let serialized = serde_json::to_string(&projected).unwrap();
        assert!(!serialized.contains("/tmp/"));
        assert!(!serialized.contains("materialized_path"));
    }

    #[test]
    fn worker_profile_candidates_are_backend_published_and_mapped() {
        let candidates = worker_profile_candidates();
        assert!(
            candidates
                .iter()
                .any(|candidate| candidate.id == "builtin:coder")
        );
        assert!(matches!(
            profile_selector_for_candidate("builtin:coder"),
            Some(ProfileSelector::Builtin(value)) if value == "builtin:coder"
        ));
        assert!(profile_selector_for_candidate("free-text-profile").is_none());
    }

    #[test]
    fn runtime_connection_request_validation_bounds_browser_input() {
        let ok = AddRemoteRuntimeConnectionRequest {
            runtime_id: "team-runtime_1".to_string(),
            display_name: Some("Team Runtime".to_string()),
            endpoint: "https://runtime.example".to_string(),
            token_ref: None,
        };
        assert!(validate_runtime_connection_request(&ok).is_ok());

        let bad_endpoint = AddRemoteRuntimeConnectionRequest {
            endpoint: "/tmp/socket".to_string(),
            ..ok
        };
        assert!(validate_runtime_connection_request(&bad_endpoint).is_err());
    }

    #[test]
    fn backend_errors_preserve_operation_details() {
        let sanitized = sanitize_backend_error(
            "failed to open /home/example/.yoi/workspace-backend.local.toml",
        );
        assert_eq!(
            sanitized,
            "failed to open /home/example/.yoi/workspace-backend.local.toml"
        );
    }

    #[test]
    fn workdir_runtime_miss_uses_exact_typed_code() {
        assert_eq!(
            workdir_status_from_runtime_miss(&[RuntimeDiagnostic {
                code: "working_directory_not_found".to_string(),
                severity: DiagnosticSeverity::Warning,
                message: "missing".to_string(),
            }]),
            "not_found"
        );
        assert_eq!(
            workdir_status_from_runtime_miss(&[RuntimeDiagnostic {
                code: "some_other_not_found".to_string(),
                severity: DiagnosticSeverity::Warning,
                message: "not a typed workdir miss".to_string(),
            }]),
            "unknown"
        );
    }

    #[tokio::test]
    async fn profile_settings_api_returns_typed_diagnostics_for_duplicate_selector() {
        let dir = tempfile::tempdir().unwrap();
        let response = profile_settings_request(
            dir.path(),
            "POST",
            "/settings/profiles",
            json!({
                "name": "coder",
                "content": valid_profile_source("coder"),
                "registry_revision": "missing"
            }),
        )
        .await;
        assert_eq!(response.0, StatusCode::BAD_REQUEST);
        assert_diagnostic(&response.1, "profile_selector_duplicate");
    }

    #[tokio::test]
    async fn profile_settings_api_returns_typed_diagnostics_for_invalid_decodal() {
        let dir = tempfile::tempdir().unwrap();
        let response = profile_settings_request(
            dir.path(),
            "POST",
            "/settings/profiles",
            json!({
                "name": "bad",
                "content": "not decodal",
                "registry_revision": "missing"
            }),
        )
        .await;
        assert_eq!(response.0, StatusCode::BAD_REQUEST);
        assert!(
            diagnostic_codes(&response.1)
                .iter()
                .any(|code| code.starts_with("profile_source_"))
        );
    }

    #[tokio::test]
    async fn profile_settings_api_returns_typed_diagnostic_for_invalid_registry_schema() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".yoi")).unwrap();
        fs::write(dir.path().join(".yoi/profiles.toml"), "[profile\n").unwrap();
        let response = profile_settings_request(
            dir.path(),
            "POST",
            "/settings/profiles",
            json!({
                "name": "schema",
                "content": valid_profile_source("schema"),
                "registry_revision": test_file_revision(&dir.path().join(".yoi/profiles.toml"))
            }),
        )
        .await;
        assert_eq!(response.0, StatusCode::BAD_REQUEST);
        assert_diagnostic(&response.1, "profile_registry_schema_invalid");
    }

    #[tokio::test]
    async fn profile_settings_api_returns_conflict_diagnostic_for_stale_revision() {
        let dir = tempfile::tempdir().unwrap();
        let response = profile_settings_request(
            dir.path(),
            "POST",
            "/settings/profiles",
            json!({
                "name": "alpha",
                "content": valid_profile_source("alpha"),
                "registry_revision": "stale"
            }),
        )
        .await;
        assert_eq!(response.0, StatusCode::CONFLICT);
        assert_diagnostic(&response.1, "profile_registry_revision_conflict");
    }

    #[tokio::test]
    async fn profile_settings_api_returns_typed_diagnostic_for_too_large_source() {
        let dir = tempfile::tempdir().unwrap();
        let response = profile_settings_request(
            dir.path(),
            "POST",
            "/settings/profiles",
            json!({
                "name": "large",
                "content": "x".repeat((256 * 1024) + 1),
                "registry_revision": "missing"
            }),
        )
        .await;
        assert_eq!(response.0, StatusCode::BAD_REQUEST);
        assert_diagnostic(&response.1, "profile_source_too_large");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn profile_settings_api_redacts_symlink_escape_response() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".yoi/profiles")).unwrap();
        fs::write(dir.path().join(".yoi/profiles.toml"), "").unwrap();
        let outside = dir.path().join("outside.dcdl");
        fs::write(&outside, valid_profile_source("escape")).unwrap();
        std::os::unix::fs::symlink(&outside, dir.path().join(".yoi/profiles/escape.dcdl")).unwrap();
        let response = profile_settings_request(
            dir.path(),
            "PUT",
            "/settings/profiles/registry",
            json!({
                "registry_revision": test_file_revision(&dir.path().join(".yoi/profiles.toml")),
                "default_profile": null,
                "profiles": [{ "name": "escape", "profile_source_id": "project:escape" }]
            }),
        )
        .await;
        assert_eq!(response.0, StatusCode::BAD_REQUEST);
        assert_diagnostic(&response.1, "profile_source_symlink_escape");
        let rendered = response.1.to_string();
        assert!(!rendered.contains(dir.path().to_string_lossy().as_ref()));
    }

    #[tokio::test]
    async fn worker_launch_rejects_invalid_project_profile_candidate() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".yoi/profiles")).unwrap();
        fs::write(
            dir.path().join(".yoi/profiles.toml"),
            "[profile.bad]\npath = \"profiles/bad.dcdl\"\n",
        )
        .unwrap();
        fs::write(dir.path().join(".yoi/profiles/bad.dcdl"), "not decodal").unwrap();
        assert!(
            worker_profile_candidates_for_root(dir.path())
                .iter()
                .all(|candidate| candidate.id != "project:bad")
        );
        assert!(profile_selector_for_candidate_with_root(dir.path(), "project:bad").is_none());
    }

    fn valid_profile_source(slug: &str) -> String {
        format!(
            r#"{{
                slug = "{slug}";
                description = "Test";
                scope = "workspace_read";
            }}"#
        )
    }

    fn test_file_revision(path: &Path) -> String {
        let Ok(metadata) = fs::metadata(path) else {
            return "missing".to_string();
        };
        let modified = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        format!("rev:{modified}:{}", metadata.len())
    }

    async fn profile_settings_request(
        workspace_root: &Path,
        method: &str,
        path: &str,
        body: Value,
    ) -> (StatusCode, Value) {
        let app = test_app(workspace_root.to_path_buf()).await;
        let request = Request::builder()
            .method(method)
            .uri(format!("/api/w/{TEST_WORKSPACE_ID}{path}"))
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        let json = serde_json::from_slice::<Value>(&bytes).unwrap();
        (status, json)
    }

    fn diagnostic_codes(response: &Value) -> Vec<String> {
        response["diagnostics"]
            .as_array()
            .expect("diagnostics array")
            .iter()
            .map(|diagnostic| diagnostic["code"].as_str().unwrap().to_string())
            .collect()
    }

    fn assert_diagnostic(response: &Value, code: &str) {
        let codes = diagnostic_codes(response);
        assert!(
            !codes.is_empty(),
            "diagnostics must not be empty: {response}"
        );
        assert!(
            codes.iter().any(|actual| actual == code),
            "missing {code}: {codes:?}"
        );
    }

    struct DeterministicExecutionBackend {
        contexts: std::sync::Mutex<
            std::collections::HashMap<
                worker_runtime::identity::WorkerRef,
                worker_runtime::execution::WorkerExecutionContext,
            >,
        >,
        materializer: worker_runtime::working_directory::LocalGitWorktreeMaterializer,
    }

    impl Default for DeterministicExecutionBackend {
        fn default() -> Self {
            let unique = format!(
                "yoi-deterministic-wd-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            );
            Self {
                contexts: std::sync::Mutex::new(std::collections::HashMap::new()),
                materializer: worker_runtime::working_directory::LocalGitWorktreeMaterializer::new(
                    std::env::temp_dir().join(unique),
                ),
            }
        }
    }

    impl worker_runtime::execution::WorkerExecutionBackend for DeterministicExecutionBackend {
        fn backend_id(&self) -> &str {
            "deterministic-workspace-server-test"
        }

        fn create_working_directory(
            &self,
            request: &worker_runtime::catalog::WorkingDirectoryRequest,
        ) -> std::result::Result<
            worker_runtime::catalog::WorkingDirectoryStatus,
            worker_runtime::working_directory::WorkingDirectoryDiagnostic,
        > {
            Ok(self.materializer.create(request)?.status())
        }

        fn list_working_directories(&self) -> Vec<worker_runtime::catalog::WorkingDirectoryStatus> {
            self.materializer
                .list_working_directories()
                .unwrap_or_default()
        }

        fn working_directory(
            &self,
            working_directory_id: &str,
        ) -> std::result::Result<
            worker_runtime::catalog::WorkingDirectoryStatus,
            worker_runtime::working_directory::WorkingDirectoryDiagnostic,
        > {
            self.materializer
                .working_directory_status(working_directory_id)
        }

        fn cleanup_working_directory(
            &self,
            working_directory_id: &str,
        ) -> std::result::Result<
            worker_runtime::catalog::WorkingDirectoryStatus,
            worker_runtime::working_directory::WorkingDirectoryDiagnostic,
        > {
            self.materializer
                .cleanup_working_directory(working_directory_id)
        }

        fn spawn_worker(
            &self,
            request: worker_runtime::execution::WorkerExecutionSpawnRequest,
        ) -> worker_runtime::execution::WorkerExecutionSpawnResult {
            let working_directory = match request.request.working_directory.as_ref() {
                Some(claim) => match self.materializer.bind_working_directory(
                    &claim.working_directory_id,
                    claim.relative_cwd.as_deref(),
                ) {
                    Ok(binding) => Some(binding.status()),
                    Err(diagnostic) => {
                        return worker_runtime::execution::WorkerExecutionSpawnResult::Rejected(
                            worker_runtime::execution::WorkerExecutionResult::rejected(
                                worker_runtime::execution::WorkerExecutionOperation::Spawn,
                                diagnostic.to_string(),
                            ),
                        );
                    }
                },
                None => None,
            };
            self.contexts
                .lock()
                .unwrap()
                .insert(request.worker_ref.clone(), request.context);
            worker_runtime::execution::WorkerExecutionSpawnResult::Connected {
                handle: worker_runtime::execution::WorkerExecutionHandle::new(
                    request.worker_ref,
                    self.backend_id(),
                ),
                run_state: worker_runtime::execution::WorkerExecutionRunState::Idle,
                working_directory,
            }
        }

        fn dispatch_input(
            &self,
            handle: &worker_runtime::execution::WorkerExecutionHandle,
            input: worker_runtime::interaction::WorkerInput,
        ) -> worker_runtime::execution::WorkerExecutionResult {
            let context = self
                .contexts
                .lock()
                .unwrap()
                .get(handle.worker_ref())
                .cloned()
                .expect("execution context");
            let content = input.content.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(25));
                let _ = context.publish_protocol_event(protocol::Event::TextDone {
                    text: format!("server companion echoed: {content}"),
                });
            });
            worker_runtime::execution::WorkerExecutionResult::accepted(
                worker_runtime::execution::WorkerExecutionOperation::Input,
                worker_runtime::execution::WorkerExecutionRunState::Idle,
            )
        }
    }

    fn test_identity() -> WorkspaceIdentity {
        WorkspaceIdentity {
            workspace_id: TEST_WORKSPACE_ID.to_string(),
            display_name: "Test Workspace".to_string(),
            created_at: TEST_CREATED_AT.to_string(),
        }
    }

    fn test_server_config(workspace_root: impl Into<PathBuf>) -> ServerConfig {
        let workspace_root = workspace_root.into();
        let store_root = workspace_root.join(".test-embedded-runtime-store");
        let mut config = ServerConfig::local_dev(workspace_root.clone(), test_identity())
            .with_embedded_runtime_store_root(store_root);
        config.repositories = vec![ConfiguredRepository {
            id: TEST_REPOSITORY_ID.to_string(),
            provider: "git".to_string(),
            uri: ".".to_string(),
            path: workspace_root,
            display_name: Some("Test Repository".to_string()),
            default_selector: Some("HEAD".to_string()),
        }];
        config
    }

    fn init_clean_git_workspace(path: &std::path::Path) {
        for args in [
            vec!["init"],
            vec!["config", "user.email", "test@example.invalid"],
            vec!["config", "user.name", "Yoi Test"],
        ] {
            let status = std::process::Command::new("git")
                .arg("-C")
                .arg(path)
                .args(args)
                .status()
                .unwrap();
            assert!(status.success());
        }
        std::fs::write(path.join("README.md"), "clean\n").unwrap();
        std::fs::write(
            path.join(".gitignore"),
            ".yoi/\n.test-embedded-runtime-store/\n",
        )
        .unwrap();
        for args in [
            vec!["add", "README.md", ".gitignore"],
            vec!["commit", "-m", "init"],
        ] {
            let status = std::process::Command::new("git")
                .arg("-C")
                .arg(path)
                .args(args)
                .status()
                .unwrap();
            assert!(status.success());
        }
    }

    #[tokio::test]
    async fn ticket_backend_endpoint_uses_workspace_settings_backend_root() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".yoi")).unwrap();
        fs::write(
            dir.path().join(".yoi/workspace.toml"),
            format!(
                "workspace_id = \"{TEST_WORKSPACE_ID}\"\ncreated_at = \"{TEST_CREATED_AT}\"\ndisplay_name = \"Endpoint Test\"\n\n[ticket]\nlanguage = \"Japanese\"\n\n[ticket.backend]\nprovider = \"builtin:yoi_local\"\nroot = \"server-tickets\"\n"
            ),
        )
        .unwrap();
        let api = test_api(dir.path()).await;

        let Json(response) = scoped_ticket_backend_operation(
            State(api),
            AxumPath(ScopedWorkspacePath {
                workspace_id: TEST_WORKSPACE_ID.to_string(),
            }),
            Json(TicketBackendOperation::Create {
                input: ticket::NewTicket::new("Endpoint configured root"),
            }),
        )
        .await
        .unwrap_or_else(|error| panic!("ticket backend operation failed: {}", error.error));

        let ticket_ref = match response {
            TicketBackendHttpResponse::Ok {
                result: ticket::TicketBackendOperationResult::TicketRef(ticket_ref),
            } => ticket_ref,
            other => panic!("unexpected ticket backend response: {other:?}"),
        };
        assert!(
            dir.path()
                .join("server-tickets")
                .join(&ticket_ref.id)
                .join("item.md")
                .is_file()
        );
        assert!(
            !dir.path()
                .join(".yoi/tickets")
                .join(&ticket_ref.id)
                .join("item.md")
                .exists()
        );
    }

    #[tokio::test]
    async fn skills_endpoints_use_workspace_backend_catalog_and_progressive_detail() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join(".yoi/skills/triage-errors");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: triage-errors\ndescription: Use when triaging backend errors and choosing safe diagnostics.\n---\n\n# Triage Errors\n\nInspect logs before changing code.",
        )
        .unwrap();
        let api = test_api(dir.path()).await;

        let Json(catalog) = scoped_list_skills(
            State(api.clone()),
            AxumPath(ScopedWorkspacePath {
                workspace_id: TEST_WORKSPACE_ID.to_string(),
            }),
        )
        .await
        .unwrap_or_else(|error| panic!("skill catalog failed: {}", error.error));
        let entry = catalog
            .entries
            .iter()
            .find(|entry| entry.name == "triage-errors")
            .expect("workspace Skill catalog entry");
        assert_eq!(entry.provenance.id, "workspace:triage-errors");
        assert!(
            !serde_json::to_string(&catalog)
                .unwrap()
                .contains("# Triage Errors")
        );

        let Json(detail) = scoped_get_skill(
            State(api),
            AxumPath(ScopedSkillPath {
                workspace_id: TEST_WORKSPACE_ID.to_string(),
                name: "triage-errors".to_string(),
            }),
        )
        .await
        .unwrap_or_else(|error| panic!("skill detail failed: {}", error.error));
        assert!(detail.body.contains("# Triage Errors"));
        assert_eq!(detail.provenance.id, "workspace:triage-errors");
    }

    async fn test_api(workspace_root: impl Into<PathBuf>) -> WorkspaceApi {
        let store = SqliteWorkspaceStore::in_memory().unwrap();
        WorkspaceApi::new_with_execution_backend(
            test_server_config(workspace_root),
            Arc::new(store),
            Arc::new(DeterministicExecutionBackend::default()),
        )
        .await
        .unwrap()
    }

    fn seed_cleanup_worker(
        api: &WorkspaceApi,
        runtime_worker_id: u64,
        retention_state: &str,
    ) -> String {
        let now = now_registry_timestamp();
        api.store
            .upsert_worker_registry(&WorkerRegistryRecord {
                workspace_id: api.config.workspace_id.clone(),
                runtime_id: "runtime-test".to_string(),
                runtime_worker_id,
                display_name: runtime_worker_id.to_string(),
                profile: None,
                retention_state: retention_state.to_string(),
                transcript_ref: None,
                session_ref: None,
                summary_ref: None,
                diagnostics_ref: None,
                created_at: now.clone(),
                updated_at: now,
            })
            .unwrap();
        runtime_worker_id.to_string()
    }

    fn seed_cleanup_workdir(api: &WorkspaceApi, workdir_id: &str, status: &str, cleanliness: &str) {
        let now = now_registry_timestamp();
        api.store
            .upsert_workdir_registry(&WorkdirRegistryRecord {
                workspace_id: api.config.workspace_id.clone(),
                workdir_id: workdir_id.to_string(),
                runtime_id: "runtime-test".to_string(),
                repository_id: "repo-test".to_string(),
                selector: Some("HEAD".to_string()),
                resolved_commit: None,
                materialization_status: status.to_string(),
                cleanliness: cleanliness.to_string(),
                created_at: now.clone(),
                updated_at: now,
            })
            .unwrap();
    }

    fn seed_cleanup_link(api: &WorkspaceApi, runtime_worker_id: &str, workdir_id: &str) {
        let runtime_worker_id = runtime_worker_id.parse::<u64>().unwrap();
        api.store
            .upsert_worker_workdir_link(&WorkerWorkdirLinkRecord {
                workspace_id: api.config.workspace_id.clone(),
                runtime_id: "runtime-test".to_string(),
                runtime_worker_id,
                workdir_id: workdir_id.to_string(),
                role: "primary".to_string(),
                linked_at: now_registry_timestamp(),
                unlinked_at: None,
            })
            .unwrap();
    }

    #[tokio::test]
    async fn synthetic_verified_clean_workdir_can_still_use_clean_cleanup_path() {
        let workspace = tempfile::tempdir().unwrap();
        init_clean_git_workspace(workspace.path());
        let api = test_api(workspace.path()).await;
        seed_cleanup_workdir(&api, "verified-clean", "present", "clean");

        let plan = build_runtime_cleanup_plan(&api, "runtime-test")
            .unwrap_or_else(|err| panic!("cleanup plan: {}", err.error));
        let candidate = plan
            .workdirs
            .iter()
            .find(|candidate| candidate.workdir_id == "verified-clean")
            .expect("cleanup candidate");
        assert_eq!(candidate.cleanliness, CleanupWorkdirCleanliness::Clean);
        assert_eq!(candidate.action, CleanupTargetKind::WorkdirCleanCleanup);
    }

    #[tokio::test]
    async fn cleanup_plan_reports_pinned_running_dirty_not_found_and_redacts_paths() {
        let workspace = tempfile::tempdir().unwrap();
        init_clean_git_workspace(workspace.path());
        let api = test_api(workspace.path()).await;
        let pinned = seed_cleanup_worker(&api, 1, "pinned");
        let unobserved = seed_cleanup_worker(&api, 2, "normal");
        seed_cleanup_workdir(&api, "workdir-dirty", "present", "dirty");
        seed_cleanup_workdir(&api, "workdir-not-found", "not_found", "clean");
        seed_cleanup_link(&api, pinned.as_str(), "workdir-dirty");
        seed_cleanup_link(&api, unobserved.as_str(), "workdir-not-found");

        let plan = build_runtime_cleanup_plan(&api, "runtime-test")
            .unwrap_or_else(|err| panic!("cleanup plan: {}", err.error));
        let pinned_worker = plan
            .workers
            .iter()
            .find(|candidate| candidate.worker_id == pinned)
            .unwrap();
        assert!(pinned_worker.pinned);
        assert_eq!(
            pinned_worker.blocking_reason.as_deref(),
            Some("worker is pinned")
        );
        let running_linked_workdir = plan
            .workdirs
            .iter()
            .find(|candidate| candidate.workdir_id == "workdir-not-found")
            .unwrap();
        assert_eq!(
            running_linked_workdir.file_status,
            CleanupWorkdirFileStatus::NotFound
        );
        assert_eq!(
            running_linked_workdir.action,
            CleanupTargetKind::WorkdirRecordDelete
        );
        assert!(!running_linked_workdir.running_linked);
        assert_eq!(running_linked_workdir.blocking_reason.as_deref(), None);
        let dirty_workdir = plan
            .workdirs
            .iter()
            .find(|candidate| candidate.workdir_id == "workdir-dirty")
            .unwrap();
        assert_eq!(dirty_workdir.action, CleanupTargetKind::WorkdirDirtyDiscard);
        assert!(dirty_workdir.pinned_linked);
        let serialized = serde_json::to_string(&plan).unwrap();
        assert!(!serialized.contains("/tmp/secret-runtime-path"));
    }

    #[tokio::test]
    async fn cleanup_execution_rejects_stale_plan_and_pinned_worker_delete() {
        let workspace = tempfile::tempdir().unwrap();
        init_clean_git_workspace(workspace.path());
        let api = test_api(workspace.path()).await;
        let worker = seed_cleanup_worker(&api, 1, "pinned");
        let plan = build_runtime_cleanup_plan(&api, "runtime-test")
            .unwrap_or_else(|err| panic!("cleanup plan: {}", err.error));
        let target = plan
            .workers
            .iter()
            .find(|candidate| candidate.worker_id == worker)
            .unwrap()
            .target_id
            .clone();
        let stale = ExecuteRuntimeCleanupRequest {
            expected_plan_revision: "stale".to_string(),
            expected_plan_digest: plan.digest.clone(),
            worker_target_ids: vec![target.clone()],
            workdir_target_ids: Vec::new(),
            confirm_dirty_discard_target_ids: Vec::new(),
        };
        assert!(execute_runtime_cleanup(&api, "runtime-test", stale).is_err());
        let pinned = ExecuteRuntimeCleanupRequest {
            expected_plan_revision: plan.revision,
            expected_plan_digest: plan.digest,
            worker_target_ids: vec![target],
            workdir_target_ids: Vec::new(),
            confirm_dirty_discard_target_ids: Vec::new(),
        };
        assert!(execute_runtime_cleanup(&api, "runtime-test", pinned).is_err());
    }

    #[tokio::test]
    async fn cleanup_execution_requires_dirty_confirmation_and_deletes_removed_record() {
        let workspace = tempfile::tempdir().unwrap();
        init_clean_git_workspace(workspace.path());
        let api = test_api(workspace.path()).await;
        seed_cleanup_workdir(&api, "workdir-dirty", "present", "dirty");
        seed_cleanup_workdir(&api, "workdir-not-found", "not_found", "clean");
        let plan = build_runtime_cleanup_plan(&api, "runtime-test")
            .unwrap_or_else(|err| panic!("cleanup plan: {}", err.error));
        let dirty_target = plan
            .workdirs
            .iter()
            .find(|candidate| candidate.workdir_id == "workdir-dirty")
            .unwrap()
            .target_id
            .clone();
        let removed_target = plan
            .workdirs
            .iter()
            .find(|candidate| candidate.workdir_id == "workdir-not-found")
            .unwrap()
            .target_id
            .clone();
        let missing_confirmation = ExecuteRuntimeCleanupRequest {
            expected_plan_revision: plan.revision.clone(),
            expected_plan_digest: plan.digest.clone(),
            worker_target_ids: Vec::new(),
            workdir_target_ids: vec![dirty_target],
            confirm_dirty_discard_target_ids: Vec::new(),
        };
        assert!(execute_runtime_cleanup(&api, "runtime-test", missing_confirmation).is_err());
        let delete_removed = ExecuteRuntimeCleanupRequest {
            expected_plan_revision: plan.revision,
            expected_plan_digest: plan.digest,
            worker_target_ids: Vec::new(),
            workdir_target_ids: vec![removed_target],
            confirm_dirty_discard_target_ids: Vec::new(),
        };
        let response = execute_runtime_cleanup(&api, "runtime-test", delete_removed)
            .unwrap_or_else(|err| panic!("cleanup execution: {}", err.error));
        assert_eq!(response.results[0].status, "deleted");
        assert!(
            api.store
                .get_workdir_registry(&api.config.workspace_id, "workdir-not-found")
                .unwrap()
                .is_none()
        );
    }

    async fn test_app(workspace_root: impl Into<PathBuf>) -> Router {
        build_router(test_api(workspace_root).await)
    }

    fn test_profile_archive() -> worker_runtime::profile_archive::ProfileSourceArchive {
        use worker_runtime::profile_archive::{ProfileSourceArchive, ProfileSourceArchiveInput};
        ProfileSourceArchive::build(ProfileSourceArchiveInput {
            id: "profile-source-archive:server-test".to_string(),
            entrypoints: std::collections::BTreeMap::from([(
                "default".to_string(),
                "profiles/default.dcdl".to_string(),
            )]),
            imports: std::collections::BTreeMap::new(),
            sources: std::collections::BTreeMap::from([(
                "profiles/default.dcdl".to_string(),
                r#"{
                    slug = "default";
                    description = "Default";
                    scope = "workspace_read";
                }"#
                .to_string(),
            )]),
        })
        .unwrap()
    }

    fn missing_resource_handle() -> worker_runtime::resource::BackendResourceHandle {
        worker_runtime::resource::BackendResourceHandle {
            kind: worker_runtime::resource::BackendResourceKind::ProfileSourceArchive,
            workspace_id: "workspace-test".to_string(),
            scope_id: Some("workspace-profile-source".to_string()),
            runtime_id: Some("runtime-test".to_string()),
            worker_id: None,
            resource_id: "profile-source-archive:missing".to_string(),
            digest: "sha256:0000000000000000000000000000000000000000000000000000000000000000"
                .to_string(),
            operation: worker_runtime::resource::BackendResourceOperation::FetchArchive,
            expires_at_unix_seconds: 4_102_444_800,
            nonce: "missing-nonce".to_string(),
            revision: "missing-revision".to_string(),
            generation: None,
            max_bytes: worker_runtime::resource::DEFAULT_PROFILE_SOURCE_ARCHIVE_MAX_BYTES,
            content_type: worker_runtime::resource::PROFILE_SOURCE_ARCHIVE_CONTENT_TYPE.to_string(),
            redaction: worker_runtime::resource::ResourceRedactionPolicy::RuntimeInternalOnly,
            audit_correlation_id: "audit-missing".to_string(),
            profile_source_graph: Some(
                worker_runtime::profile_archive::ProfileSourceGraphSummary {
                    entrypoints: std::collections::BTreeMap::from([(
                        "default".to_string(),
                        "profiles/default.dcdl".to_string(),
                    )]),
                    source_count: 1,
                    import_count: 0,
                    total_source_bytes: 0,
                },
            ),
        }
    }

    #[tokio::test]
    async fn internal_resource_fetch_rest_returns_typed_missing_resource() {
        let workspace = tempfile::tempdir().unwrap();
        init_clean_git_workspace(workspace.path());
        let app = test_app(workspace.path()).await;
        let handle = missing_resource_handle();
        let response = app
            .oneshot(
                Request::post("/internal/runtime/resources/fetch")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(
                            &worker_runtime::resource::BackendResourceFetchRequest {
                                audit_correlation_id: handle.audit_correlation_id.clone(),
                                runtime_id: "runtime-test".to_string(),
                                worker_id: None,
                                handle,
                            },
                        )
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let error: worker_runtime::resource::BackendResourceError =
            serde_json::from_slice(&bytes).unwrap();
        assert!(matches!(
            error,
            worker_runtime::resource::BackendResourceError::MissingResource
        ));
    }

    #[tokio::test]
    async fn remote_http_resource_fetch_uses_backend_resource_contract() {
        let workspace = tempfile::tempdir().unwrap();
        init_clean_git_workspace(workspace.path());
        let api = test_api(workspace.path()).await;
        let broker = api.resource_broker.clone();
        let archive = test_profile_archive();
        let runtime_id = "runtime-test";
        let handle = broker.issue_profile_source_archive_handle(
            "workspace-test",
            Some(runtime_id),
            None,
            archive,
        );
        let app = build_router(api);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = worker_runtime::resource::HttpBackendResourceClient::new(
            format!("http://{addr}/internal/runtime/resources/fetch"),
            None,
        );

        let response = client
            .fetch_resource(worker_runtime::resource::BackendResourceFetchRequest {
                audit_correlation_id: handle.audit_correlation_id.clone(),
                runtime_id: runtime_id.to_string(),
                worker_id: None,
                handle: handle.clone(),
            })
            .await
            .expect("remote HTTP resource fetch succeeds");
        assert_eq!(response.digest, handle.digest);

        let mut tampered = handle;
        tampered.scope_id = Some("tampered".to_string());
        let error = client
            .fetch_resource(worker_runtime::resource::BackendResourceFetchRequest {
                audit_correlation_id: tampered.audit_correlation_id.clone(),
                runtime_id: runtime_id.to_string(),
                worker_id: None,
                handle: tampered,
            })
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            worker_runtime::resource::BackendResourceError::Unauthorized { .. }
        ));
        server.abort();
    }

    fn runtime_test_bundle() -> worker_runtime::config_bundle::ConfigBundle {
        worker_runtime::config_bundle::ConfigBundle {
            metadata: worker_runtime::config_bundle::ConfigBundleMetadata {
                id: "server-test-bundle".to_string(),
                digest: String::new(),
                revision: "test".to_string(),
                workspace_id: "test".to_string(),
                created_at: "test".to_string(),
                provenance: worker_runtime::config_bundle::ConfigBundleProvenance {
                    source: "test".to_string(),
                    detail: None,
                },
            },
            profiles: vec![worker_runtime::config_bundle::ConfigProfileDescriptor {
                selector: worker_runtime::catalog::ProfileSelector::RuntimeDefault,
                label: Some("server-test".to_string()),
            }],
            declarations: Vec::new(),
            profile_source_archive: None,
            profile_source_archive_handle: None,
        }
        .with_computed_digest()
    }

    fn runtime_create_request() -> worker_runtime::catalog::CreateWorkerRequest {
        let bundle = runtime_test_bundle();
        worker_runtime::catalog::CreateWorkerRequest {
            profile: worker_runtime::catalog::ProfileSelector::RuntimeDefault,
            profile_source: worker_runtime::catalog::ProfileSourceArchiveSource::Http {
                location: worker_runtime::catalog::ProfileSourceArchiveHttpRef {
                    url: "http://127.0.0.1/profile-source.tar".to_string(),
                    etag: None,
                    archive: worker_runtime::profile_archive::ProfileSourceArchiveRef {
                        id: "test-profile-source".to_string(),
                        digest: "test-digest".to_string(),
                        size_bytes: 0,
                        source_graph: worker_runtime::profile_archive::ProfileSourceGraphSummary {
                            source_count: 0,
                            total_source_bytes: 0,
                            entrypoints: std::collections::BTreeMap::new(),
                            import_count: 0,
                        },
                    },
                },
            },
            config_bundle: Some(worker_runtime::catalog::ConfigBundleRef {
                id: bundle.metadata.id,
                digest: bundle.metadata.digest,
            }),
            initial_input: None,
            working_directory_request: None,
            working_directory: None,
            workspace_api: None,
        }
    }

    fn runtime_with_worker() -> (worker_runtime::Runtime, worker_runtime::identity::WorkerRef) {
        let runtime = worker_runtime::Runtime::with_execution_backend(
            worker_runtime::RuntimeOptions::default(),
            Arc::new(DeterministicExecutionBackend::default()),
        )
        .unwrap();
        runtime.store_config_bundle(runtime_test_bundle()).unwrap();
        let worker = runtime.create_worker(runtime_create_request()).unwrap();
        (runtime, worker.worker_ref)
    }

    #[tokio::test]
    async fn browser_working_directory_create_is_rejected_as_backend_lifecycle() {
        let dir = tempfile::tempdir().unwrap();
        init_clean_git_workspace(dir.path());
        let app = test_app(dir.path()).await;
        let workspace_path = format!("/api/w/{TEST_WORKSPACE_ID}/working-directories");

        let response = request_json(
            app,
            "POST",
            &workspace_path,
            Some(serde_json::json!({
                "repository_id": TEST_REPOSITORY_ID,
                "selector": "HEAD",
            })),
            StatusCode::BAD_REQUEST,
        )
        .await;
        assert!(
            response["diagnostics"]
                .as_array()
                .unwrap()
                .iter()
                .any(|diagnostic| diagnostic["code"] == "backend_workdir_create_unsupported"),
            "expected backend lifecycle diagnostic, got {response}"
        );
        let projected = serde_json::to_string(&response).unwrap();
        assert!(!projected.contains(dir.path().to_string_lossy().as_ref()));
    }

    #[tokio::test]
    async fn scoped_worker_create_invalid_relative_cwd_returns_typed_diagnostic() {
        let dir = tempfile::tempdir().unwrap();
        init_clean_git_workspace(dir.path());
        let app = test_app(dir.path()).await;
        let working_directory_id = "test-workdir";

        let response = request_json(
            app,
            "POST",
            &format!("/api/w/{TEST_WORKSPACE_ID}/workers"),
            Some(serde_json::json!({
                "runtime_id": "remote-runtime",
                "display_name": "Coding Worker",
                "profile": "builtin:coder",
                "initial_text": "",
                "working_directory": {
                    "working_directory_id": working_directory_id,
                    "relative_cwd": "../escape"
                }
            })),
            StatusCode::BAD_REQUEST,
        )
        .await;
        assert!(
            response["diagnostics"]
                .as_array()
                .unwrap()
                .iter()
                .any(|diagnostic| diagnostic["code"] == "working_directory_relative_cwd_invalid"),
            "expected typed relative_cwd diagnostic, got {response}"
        );
        assert!(
            response["message"]
                .as_str()
                .unwrap_or_default()
                .contains("working_directory_relative_cwd_invalid")
        );
        let projected = serde_json::to_string(&response).unwrap();
        assert!(!projected.contains(dir.path().to_string_lossy().as_ref()));
    }

    #[tokio::test]
    async fn runtime_connection_settings_add_delete_apply_live_registry() {
        let dir = tempfile::tempdir().unwrap();
        let app = test_app(dir.path()).await;

        let settings = get_json(app.clone(), "/api/settings/runtime-connections").await;
        assert_eq!(settings["embedded"]["built_in"], true);
        assert_eq!(settings["embedded"]["config_managed"], false);

        let added = post_json(
            app.clone(),
            "/api/settings/runtime-connections/remotes",
            serde_json::json!({
                "runtime_id": "team-runtime",
                "display_name": "Team Runtime",
                "endpoint": "https://runtime.example.invalid"
            }),
        )
        .await;
        assert_eq!(added["restart_required"], false);
        assert_eq!(added["remotes"][0]["runtime_id"], "team-runtime");
        assert_eq!(added["remotes"][0]["endpoint_configured"], true);
        assert!(
            added["diagnostics"]
                .as_array()
                .unwrap()
                .iter()
                .any(|diagnostic| diagnostic["code"] == "runtime_registry_applied")
        );
        let projected = serde_json::to_string(&added).unwrap();
        assert!(!projected.contains("runtime.example.invalid"));

        let persisted = WorkspaceBackendConfigFile::load_for_workspace(dir.path()).unwrap();
        assert_eq!(persisted.runtimes.remote.len(), 1);
        assert_eq!(persisted.runtimes.remote[0].id, "team-runtime");
        assert_eq!(
            persisted.runtimes.remote[0].endpoint,
            "https://runtime.example.invalid"
        );

        let launch_options = get_json(app.clone(), "/api/workers/launch-options").await;
        let runtimes = launch_options["runtimes"].as_array().unwrap();
        let embedded_runtime = runtimes
            .iter()
            .find(|runtime| runtime["runtime_id"] == EMBEDDED_WORKER_RUNTIME_ID)
            .expect("embedded runtime launch option");
        assert_eq!(embedded_runtime["working_directory_required"], false);
        let team_runtime = runtimes
            .iter()
            .find(|runtime| runtime["runtime_id"] == "team-runtime")
            .expect("team runtime launch option");
        assert_eq!(team_runtime["working_directory_required"], true);

        let deleted = request_json(
            app.clone(),
            "DELETE",
            "/api/settings/runtime-connections/remotes/team-runtime",
            None,
            StatusCode::OK,
        )
        .await;
        assert_eq!(deleted["restart_required"], false);
        assert_eq!(deleted["remotes"].as_array().unwrap().len(), 0);
        let launch_options = get_json(app.clone(), "/api/workers/launch-options").await;
        assert!(
            !launch_options["runtimes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|runtime| runtime["runtime_id"] == "team-runtime")
        );
        let persisted = WorkspaceBackendConfigFile::load_for_workspace(dir.path()).unwrap();
        assert!(persisted.runtimes.remote.is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn runtime_connection_delete_rejects_active_remote_workers() {
        let (runtime, _worker_ref) = runtime_with_worker();
        let runtime_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let runtime_addr = runtime_listener.local_addr().unwrap();
        tokio::spawn({
            let runtime = runtime.clone();
            async move {
                worker_runtime::http_server::serve_runtime_http(runtime, runtime_listener, None)
                    .await
                    .unwrap()
            }
        });

        let dir = tempfile::tempdir().unwrap();
        let app = test_app(dir.path()).await;
        let added = post_json(
            app.clone(),
            "/api/settings/runtime-connections/remotes",
            serde_json::json!({
                "runtime_id": "busy-runtime",
                "display_name": "Busy Runtime",
                "endpoint": format!("http://{runtime_addr}")
            }),
        )
        .await;
        assert_eq!(added["restart_required"], false);
        let workers = get_json(app.clone(), "/api/workers").await;
        assert!(
            workers["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|worker| worker["runtime_id"] == "busy-runtime"),
            "expected remote runtime to report at least one worker, got {workers}"
        );

        let response = request_json(
            app,
            "DELETE",
            "/api/settings/runtime-connections/remotes/busy-runtime",
            None,
            StatusCode::CONFLICT,
        )
        .await;
        assert!(
            response["message"]
                .as_str()
                .unwrap()
                .contains("remote_runtime_delete_blocked")
        );
        assert!(
            response["diagnostics"]
                .as_array()
                .unwrap()
                .iter()
                .any(|diagnostic| { diagnostic["code"] == "remote_runtime_delete_blocked" })
        );
        let persisted = WorkspaceBackendConfigFile::load_for_workspace(dir.path()).unwrap();
        assert_eq!(persisted.runtimes.remote.len(), 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn runtime_connection_test_reports_compatible_with_unknown_warnings_without_endpoint_leak()
     {
        let (runtime, _worker_ref) = runtime_with_worker();
        let runtime_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let runtime_addr = runtime_listener.local_addr().unwrap();
        tokio::spawn({
            let runtime = runtime.clone();
            async move {
                worker_runtime::http_server::serve_runtime_http(runtime, runtime_listener, None)
                    .await
                    .unwrap()
            }
        });

        let dir = tempfile::tempdir().unwrap();
        let endpoint = format!("http://{runtime_addr}");
        WorkspaceBackendConfigFile {
            runtimes: crate::config::WorkspaceBackendRuntimesConfig {
                remote: vec![RemoteRuntimeConfigFile {
                    id: "probe-runtime".to_string(),
                    endpoint: endpoint.clone(),
                    display_name: Some("Probe Runtime".to_string()),
                    token_ref: None,
                }],
            },
            ..WorkspaceBackendConfigFile::default()
        }
        .write_for_workspace(dir.path())
        .unwrap();
        let app = test_app(dir.path()).await;

        let response = post_json(
            app,
            "/api/settings/runtime-connections/remotes/probe-runtime/test",
            serde_json::json!({}),
        )
        .await;
        assert_eq!(response["state"], "compatible");
        let capabilities = response["capabilities"].as_array().unwrap();
        assert!(
            capabilities
                .iter()
                .any(|value| value == "runtime.summary:available")
        );
        assert!(
            capabilities
                .iter()
                .any(|value| value == "workers.list:available")
        );
        assert!(
            capabilities
                .iter()
                .any(|value| value == "workers.spawn:available")
        );
        assert!(
            response["diagnostics"]
                .as_array()
                .unwrap()
                .iter()
                .any(|diagnostic| { diagnostic["code"] == "workers.spawn.available" })
        );
        let projected = serde_json::to_string(&response).unwrap();
        assert!(!projected.contains(&endpoint));
        assert!(!projected.contains(&runtime_addr.to_string()));
        assert_eq!(response["protocol_version"], serde_json::Value::Null);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn runtime_connection_test_marks_missing_execution_backend_incompatible() {
        let runtime =
            worker_runtime::Runtime::with_options(worker_runtime::RuntimeOptions::default());
        let runtime_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let runtime_addr = runtime_listener.local_addr().unwrap();
        tokio::spawn(async move {
            worker_runtime::http_server::serve_runtime_http(runtime, runtime_listener, None)
                .await
                .unwrap()
        });

        let dir = tempfile::tempdir().unwrap();
        let endpoint = format!("http://{runtime_addr}");
        WorkspaceBackendConfigFile {
            runtimes: crate::config::WorkspaceBackendRuntimesConfig {
                remote: vec![RemoteRuntimeConfigFile {
                    id: "control-only-runtime".to_string(),
                    display_name: Some("Control-only Runtime".to_string()),
                    endpoint,
                    token_ref: None,
                }],
            },
            ..WorkspaceBackendConfigFile::default()
        }
        .write_for_workspace(dir.path())
        .unwrap();
        let app = test_app(dir.path()).await;

        let response = post_json(
            app,
            "/api/settings/runtime-connections/remotes/control-only-runtime/test",
            serde_json::json!({}),
        )
        .await;
        assert_eq!(response["state"], "incompatible");
        assert!(
            response["capabilities"]
                .as_array()
                .unwrap()
                .iter()
                .any(|value| { value == "workers.spawn:incompatible" })
        );
        assert!(
            response["diagnostics"]
                .as_array()
                .unwrap()
                .iter()
                .any(|diagnostic| {
                    diagnostic["code"] == "remote_runtime_worker_creation_unavailable"
                })
        );
    }

    #[tokio::test]
    async fn browser_worker_create_succeeds_and_preserves_unsupported_diagnostics() {
        let dir = tempfile::tempdir().unwrap();
        let app = test_app(dir.path()).await;
        let created = post_json(
            app.clone(),
            "/api/workers",
            serde_json::json!({
                "runtime_id": "embedded-worker-runtime",
                "display_name": "",
                "profile": "runtime_default",
                "initial_text": ""
            }),
        )
        .await;
        assert_eq!(created["runtime_id"], "embedded-worker-runtime");
        let workers = get_json(app.clone(), "/api/workers").await;
        let worker = workers["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|worker| worker["worker_id"] == created["worker_id"])
            .expect("created Worker should be listed");
        assert_eq!(worker["label"], "Coding Worker");
        assert_eq!(worker["worker_id"], created["worker_id"]);
        let detail_path = format!(
            "/api/runtimes/{}/workers/{}",
            created["runtime_id"].as_str().unwrap(),
            created["worker_id"].as_str().unwrap()
        );
        let detail = get_json(app.clone(), detail_path.as_str()).await;
        assert_eq!(detail["label"], "Coding Worker");
        assert_eq!(detail["worker_id"], created["worker_id"]);
        assert!(
            created["console_href"]
                .as_str()
                .unwrap()
                .contains("/console")
        );

        let response = worker_create_not_accepted_error(
            "unsupported-runtime".to_string(),
            vec![settings_diagnostic(
                "remote_runtime_unsupported",
                DiagnosticSeverity::Warning,
                "Remote Runtime provisioning is unsupported by this v0 worker launch path.",
            )],
        )
        .into_response();
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let response: Value = serde_json::from_slice(&bytes).unwrap();
        let diagnostics = response["diagnostics"].as_array().unwrap();
        assert!(diagnostics.len() >= 2);
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| { diagnostic["code"] == "workspace_worker_create_not_accepted" })
        );
        let projected = serde_json::to_string(&response).unwrap();
        assert!(!projected.contains("http://"));
    }

    #[tokio::test]
    async fn browser_worker_create_rejects_non_embedded_no_workdir() {
        let dir = tempfile::tempdir().unwrap();
        let app = test_app(dir.path()).await;
        let response = request_json(
            app,
            "POST",
            "/api/workers",
            Some(serde_json::json!({
                "runtime_id": "remote-runtime",
                "display_name": "Remote Worker",
                "profile": "runtime_default",
                "initial_text": ""
            })),
            StatusCode::BAD_REQUEST,
        )
        .await;
        assert!(
            response["message"]
                .as_str()
                .unwrap_or_default()
                .contains("workspace_worker_workdir_required")
        );
        assert!(
            response["diagnostics"]
                .as_array()
                .unwrap()
                .iter()
                .any(|diagnostic| { diagnostic["code"] == "workspace_worker_workdir_required" })
        );
    }

    #[tokio::test]
    async fn runtime_worker_spawn_rejects_raw_working_directory_fields() {
        let dir = tempfile::tempdir().unwrap();
        let app = test_app(dir.path()).await;
        let response = request_json(
            app,
            "POST",
            "/api/runtimes/embedded-worker-runtime/workers",
            Some(serde_json::json!({
                "intent": {
                    "kind": "ticket_role",
                    "ticket_id": "00001KVZSGT0Q",
                    "role": "coder"
                },
                "acceptance": {
                    "kind": "run_accepted",
                    "expected_segments": 0
                },
                "working_directory_request": {
                    "repository_id": TEST_REPOSITORY_ID,
                    "local_path": dir.path().display().to_string()
                }
            })),
            StatusCode::UNPROCESSABLE_ENTITY,
        )
        .await;
        assert!(
            response["message"]
                .as_str()
                .unwrap_or_default()
                .contains("unknown field"),
            "raw working directory field should be rejected: {response}"
        );
    }

    #[tokio::test]
    async fn runtime_worker_spawn_rejects_embedded_workdir_request() {
        let dir = tempfile::tempdir().unwrap();
        let app = test_app(dir.path()).await;
        let response = request_json(
            app,
            "POST",
            "/api/runtimes/embedded-worker-runtime/workers",
            Some(serde_json::json!({
                "intent": {
                    "kind": "ticket_role",
                    "ticket_id": "00001KVZSGT0Q",
                    "role": "coder"
                },
                "acceptance": {
                    "kind": "run_accepted",
                    "expected_segments": 0
                },
                "working_directory_request": {
                    "repository_id": TEST_REPOSITORY_ID,
                    "selector": "HEAD"
                }
            })),
            StatusCode::BAD_REQUEST,
        )
        .await;
        assert!(
            response["diagnostics"]
                .as_array()
                .unwrap()
                .iter()
                .any(|diagnostic| diagnostic["code"] == "embedded_worker_workdir_unsupported"),
            "expected embedded workdir diagnostic, got {response}"
        );
        let projected = serde_json::to_string(&response).unwrap();
        assert!(!projected.contains(dir.path().to_string_lossy().as_ref()));
    }

    #[tokio::test]
    async fn browser_worker_create_rejects_extra_request_fields() {
        let dir = tempfile::tempdir().unwrap();
        let app = test_app(dir.path()).await;
        let response = request_json(
            app,
            "POST",
            "/api/workers",
            Some(serde_json::json!({
                "runtime_id": "embedded-worker-runtime",
                "display_name": "Coding Worker",
                "profile": "builtin:coder",
                "initial_text": "",
                "kind": "internal"
            })),
            StatusCode::UNPROCESSABLE_ENTITY,
        )
        .await;
        assert!(
            response["message"]
                .as_str()
                .unwrap_or_default()
                .contains("unknown field")
        );
    }

    #[tokio::test]
    async fn serves_bounded_read_apis_and_static_spa_separately() {
        let dir = tempfile::tempdir().unwrap();
        write_ticket(dir.path(), "00000000001J2", "API Ticket", "ready");
        write_objective(dir.path(), "00000000001J3", "API Objective", "active");
        let static_dir = dir.path().join("static");
        std::fs::create_dir_all(static_dir.join("assets")).unwrap();
        std::fs::write(static_dir.join("index.html"), "<main>Yoi Workspace</main>").unwrap();
        std::fs::write(static_dir.join("assets/app.js"), "console.log('yoi');").unwrap();

        let store = SqliteWorkspaceStore::in_memory().unwrap();
        let mut config = test_server_config(dir.path());
        config.static_assets_dir = Some(static_dir);
        let api = WorkspaceApi::new_with_execution_backend(
            config,
            Arc::new(store),
            Arc::new(DeterministicExecutionBackend::default()),
        )
        .await
        .unwrap();
        let app = build_router(api);

        let workspace = get_json(app.clone(), "/api/workspace").await;
        assert_eq!(workspace["workspace_id"], TEST_WORKSPACE_ID);
        assert_eq!(workspace["display_name"], "Test Workspace");
        assert_eq!(workspace["record_authority"], "local_yoi_project_records");
        assert_eq!(
            workspace["extension_points"]["host_worker_bridge"]["status"],
            "runtime_registry"
        );
        let workspace_companion = &workspace["extension_points"]["companion_console"];
        assert_ne!(workspace_companion["status"], "not_connected");
        assert!(
            !workspace_companion["note"]
                .as_str()
                .unwrap()
                .contains("browser input remains disabled"),
            "stale Companion Console note returned: {workspace_companion}"
        );
        if workspace_companion["status"] == "not_input_capable" {
            assert!(
                !workspace_companion["diagnostics"]
                    .as_array()
                    .unwrap()
                    .is_empty(),
                "not_input_capable workspace companion_console lacks typed diagnostics: {workspace_companion}"
            );
        }

        let scoped_workspace = get_json(
            app.clone(),
            &format!("/api/w/{TEST_WORKSPACE_ID}/workspace"),
        )
        .await;
        assert_eq!(scoped_workspace["workspace_id"], TEST_WORKSPACE_ID);

        let mismatched_workspace = request_json(
            app.clone(),
            "GET",
            "/api/w/not-this-workspace/workspace",
            None,
            StatusCode::NOT_FOUND,
        )
        .await;
        assert_eq!(
            mismatched_workspace["diagnostics"][0]["code"],
            "workspace_id_mismatch"
        );
        assert!(
            !mismatched_workspace
                .to_string()
                .contains(dir.path().to_string_lossy().as_ref())
        );

        let unscoped_objectives = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/objectives?focus=active")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unscoped_objectives.status(), StatusCode::TEMPORARY_REDIRECT);
        let expected_location = format!("/w/{TEST_WORKSPACE_ID}/objectives?focus=active");
        assert_eq!(
            unscoped_objectives
                .headers()
                .get(LOCATION)
                .and_then(|value| value.to_str().ok()),
            Some(expected_location.as_str())
        );

        let tickets = get_json(app.clone(), "/api/tickets").await;
        assert_eq!(tickets["items"][0]["id"], "00000000001J2");
        assert_eq!(tickets["items"][0]["state"], "ready");

        let objectives = get_json(app.clone(), "/api/objectives").await;
        assert_eq!(objectives["items"][0]["id"], "00000000001J3");
        assert_eq!(objectives["items"][0]["summary"], "Objective body.");
        let scoped_objectives = get_json(
            app.clone(),
            &format!("/api/w/{TEST_WORKSPACE_ID}/objectives"),
        )
        .await;
        assert_eq!(scoped_objectives["items"][0]["id"], "00000000001J3");
        let scoped_objective = get_json(
            app.clone(),
            &format!("/api/w/{TEST_WORKSPACE_ID}/objectives/00000000001J3"),
        )
        .await;
        assert_eq!(scoped_objective["id"], "00000000001J3");

        let repositories = get_json(app.clone(), "/api/repositories").await;
        assert_eq!(repositories["items"][0]["id"], TEST_REPOSITORY_ID);
        assert_eq!(repositories["items"][0]["kind"], "git");
        assert_eq!(
            repositories["items"][0]["record_authority"],
            "workspace-backend-config"
        );
        assert!(
            repositories
                .to_string()
                .contains("repository_git_unavailable")
        );

        let repository_detail = get_json(app.clone(), "/api/repositories/main").await;
        assert_eq!(repository_detail["item"]["id"], TEST_REPOSITORY_ID);
        let scoped_repository_detail = get_json(
            app.clone(),
            &format!("/api/w/{TEST_WORKSPACE_ID}/repositories/main"),
        )
        .await;
        assert_eq!(scoped_repository_detail["item"]["id"], TEST_REPOSITORY_ID);

        let repository_log = get_json(app.clone(), "/api/repositories/main/log?limit=3").await;
        assert_eq!(repository_log["repository_id"], TEST_REPOSITORY_ID);
        assert_eq!(repository_log["default_selector"], "HEAD");
        assert_eq!(repository_log["limit"], 3);

        let repository_tickets = get_json(app.clone(), "/api/repositories/main/tickets").await;
        assert_eq!(repository_tickets["repository_id"], TEST_REPOSITORY_ID);
        let ready_column = repository_tickets["columns"]
            .as_array()
            .unwrap()
            .iter()
            .find(|column| column["state"] == "ready")
            .unwrap();
        assert_eq!(ready_column["items"][0]["id"], "00000000001J2");
        assert_eq!(
            repository_tickets["diagnostics"][0]["code"],
            "repository_ticket_target_metadata_absent"
        );

        let unknown_repository_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/repositories/nope")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unknown_repository_response.status(), StatusCode::NOT_FOUND);

        let hosts = get_json(app.clone(), "/api/hosts").await;
        assert_eq!(hosts["source"], "worker_runtime_registry");
        assert_eq!(hosts["items"][0]["runtime_id"], "embedded-worker-runtime");
        let host_id = hosts["items"][0]["host_id"].as_str().unwrap().to_string();
        assert_eq!(hosts["items"][0]["kind"], "embedded-worker-runtime-host");
        assert_eq!(
            hosts["items"][0]["capabilities"]["workspace_scope"],
            "backend_internal"
        );
        assert!(!hosts.to_string().contains("metadata.json"));

        let runtimes = get_json(app.clone(), "/api/runtimes").await;
        assert_eq!(runtimes["source"], "worker_runtime_registry");
        assert_eq!(
            runtimes["items"][0]["runtime_id"],
            "embedded-worker-runtime"
        );
        assert_eq!(
            runtimes["items"][0]["source"]["kind"],
            "embedded_worker_runtime"
        );
        assert_eq!(
            runtimes["items"][0]["source"]["identity_authority"],
            "runtime_registry_projection"
        );
        assert!(!runtimes.to_string().contains("/workspace/demo"));
        assert_eq!(runtimes["items"][0]["host_ids"][0], host_id);

        let workers = get_json(app.clone(), "/api/workers").await;
        let worker_items = workers["items"].as_array().unwrap();
        assert!(
            worker_items
                .iter()
                .all(|worker| worker["role"] != "builtin:companion"),
            "companion auto-start should not create runtime workers: {workers}"
        );

        let companion_status = get_json(app.clone(), "/api/companion/status").await;
        assert_eq!(companion_status["state"], "disabled");
        assert!(companion_status["worker"].is_null());
        assert_eq!(companion_status["transport"]["kind"], "none");
        assert_eq!(companion_status["transport"]["completion"], "disabled");
        assert!(!companion_status.to_string().contains("/workspace/demo"));

        let companion_message = post_json(
            app.clone(),
            "/api/companion/messages",
            json!({ "content": "hello companion" }),
        )
        .await;
        assert_eq!(companion_message["state"], "rejected");
        assert_eq!(
            companion_message["diagnostics"][0]["code"],
            "companion_disabled"
        );
        assert!(companion_message["user_item"].is_null());
        assert!(companion_message["assistant_item"].is_null());
        assert!(!companion_message.to_string().contains("/workspace/demo"));

        let companion_transcript = get_json(app.clone(), "/api/companion/transcript").await;
        assert_eq!(companion_transcript["total_items"], 0);

        let host_workers = get_json(app.clone(), &format!("/api/hosts/{host_id}/workers")).await;
        assert!(
            host_workers["items"]
                .as_array()
                .unwrap()
                .iter()
                .all(|worker| worker["role"] != "builtin:companion")
        );

        let runs_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/runs")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(runs_response.status(), StatusCode::NOT_FOUND);

        let runners_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/runners")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(runners_response.status(), StatusCode::NOT_FOUND);

        let static_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/assets/app.js")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(static_response.status(), StatusCode::OK);
        assert_eq!(
            static_response.headers().get(CONTENT_TYPE).unwrap(),
            "text/javascript; charset=utf-8"
        );

        let spa_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/tickets/00000000001J2")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(spa_response.status(), StatusCode::OK);
        let bytes = to_bytes(spa_response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert!(
            String::from_utf8(bytes.to_vec())
                .unwrap()
                .contains("Yoi Workspace")
        );

        let api_miss = app
            .oneshot(
                Request::builder()
                    .uri("/api/nope")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(api_miss.status(), StatusCode::NOT_FOUND);
        let bytes = to_bytes(api_miss.into_body(), usize::MAX).await.unwrap();
        assert!(
            !String::from_utf8(bytes.to_vec())
                .unwrap()
                .contains("Yoi Workspace")
        );
    }

    #[tokio::test]
    async fn companion_routes_report_disabled_without_spawning_worker() {
        let temp = tempfile::tempdir().unwrap();
        let config = test_server_config(temp.path().join("workspace"));
        let api = WorkspaceApi::new_with_execution_backend(
            config,
            Arc::new(SqliteWorkspaceStore::in_memory().unwrap()),
            Arc::new(DeterministicExecutionBackend::default()),
        )
        .await
        .unwrap();
        let app = build_router(api);

        let workspace = get_json(app.clone(), "/api/workspace").await;
        let workspace_companion = &workspace["extension_points"]["companion_console"];
        assert_eq!(workspace_companion["status"], "disabled");
        assert_eq!(
            workspace_companion["diagnostics"][0]["code"],
            "companion_disabled"
        );
        assert!(
            workspace_companion["note"]
                .as_str()
                .unwrap()
                .contains("auto-start has been removed")
        );

        let status = get_json(app.clone(), "/api/companion/status").await;
        assert_eq!(status["state"], "disabled");
        assert_eq!(status["transport"]["completion"], "disabled");
        assert!(status["worker"].is_null());

        let response = post_json(
            app.clone(),
            "/api/companion/messages",
            serde_json::json!({ "content": "from legacy route" }),
        )
        .await;
        assert_eq!(response["state"], "rejected");
        assert_eq!(response["diagnostics"][0]["code"], "companion_disabled");
        assert!(response["user_item"].is_null());
        assert!(response["assistant_item"].is_null());

        let transcript = get_json(app.clone(), "/api/companion/transcript").await;
        assert_eq!(transcript["state"], "disabled");
        assert_eq!(transcript["total_items"], 0);

        let workers = get_json(app, "/api/workers").await;
        assert!(
            workers["items"]
                .as_array()
                .unwrap()
                .iter()
                .all(|worker| worker["role"] != "builtin:companion"),
            "disabled companion route should not spawn workers: {workers}"
        );
    }

    #[tokio::test]
    async fn embedded_runtime_fs_store_restores_catalog_config_bundle_and_stale_execution() {
        let dir = tempfile::tempdir().unwrap();
        let config = test_server_config(dir.path().join("workspace"));
        let store_root = config.embedded_runtime_store_root.clone();
        let bundle = runtime_test_bundle();
        let bundle_id = bundle.metadata.id.clone();

        let api = WorkspaceApi::new_with_execution_backend(
            config.clone(),
            Arc::new(SqliteWorkspaceStore::in_memory().unwrap()),
            Arc::new(DeterministicExecutionBackend::default()),
        )
        .await
        .expect("fs-backed api starts");
        let synced = api
            .runtime
            .sync_config_bundle("embedded-worker-runtime", bundle)
            .expect("sync config bundle");
        assert_eq!(synced.state, WorkerOperationState::Accepted);
        assert!(store_root.exists(), "fs-store root should be created");

        let spawned = api
            .runtime
            .spawn_worker(
                "embedded-worker-runtime",
                WorkerSpawnRequest {
                    intent: WorkerSpawnIntent::TicketRole {
                        ticket_id: "00001KVZSGT0Q".to_string(),
                        role: TicketWorkerRole::Coder,
                    },
                    requested_worker_name: None,
                    acceptance: WorkerSpawnAcceptanceRequirement::RunAccepted {
                        expected_segments: 0,
                    },
                    profile: None,
                    initial_input: None,
                    working_directory_request: None,
                    resolved_working_directory_request: None,
                    resolved_working_directory: None,
                    resolved_config_bundle: None,
                },
            )
            .expect("spawn worker");
        assert_eq!(spawned.state, WorkerOperationState::Accepted);
        let worker_id = spawned.worker.expect("created worker").worker_id;
        let sent = api
            .runtime
            .send_input(
                "embedded-worker-runtime",
                &worker_id,
                WorkerInputRequest {
                    kind: WorkerInputKind::User,
                    content: "persist me".to_string(),
                    segments: None,
                },
            )
            .expect("send input");
        assert_eq!(sent.state, WorkerOperationState::Accepted);

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            let detail = api
                .runtime
                .worker("embedded-worker-runtime", &worker_id)
                .expect("worker detail");
            if detail.state == "idle" {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for deterministic worker completion"
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        drop(api);

        let restored = WorkspaceApi::new_with_execution_backend(
            config,
            Arc::new(SqliteWorkspaceStore::in_memory().unwrap()),
            Arc::new(DeterministicExecutionBackend::default()),
        )
        .await
        .expect("restored fs-backed api starts");
        let restored_worker = restored
            .runtime
            .worker("embedded-worker-runtime", &worker_id)
            .expect("restored worker");
        assert_eq!(restored_worker.state, "stale");
        assert!(!restored_worker.capabilities.can_stop);
        assert!(
            restored_worker
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "embedded_worker_execution_stale")
        );

        let bundles = restored
            .runtime
            .list_config_bundles("embedded-worker-runtime")
            .expect("config bundle list");
        assert!(
            bundles
                .bundles
                .iter()
                .any(|summary| summary.id == bundle_id)
        );

        let rejected_input = restored
            .runtime
            .send_input(
                "embedded-worker-runtime",
                &worker_id,
                WorkerInputRequest {
                    kind: WorkerInputKind::User,
                    content: "should not be routed to stale handle".to_string(),
                    segments: None,
                },
            )
            .expect("stale worker input is projected as an operation result");
        assert_eq!(rejected_input.state, WorkerOperationState::Rejected);
    }

    #[tokio::test]
    async fn embedded_runtime_store_root_is_isolated_and_not_exposed_by_browser_api() {
        let dir = tempfile::tempdir().unwrap();
        let data_dir = dir.path().join("user-data");
        let workspace_root = dir.path().join("workspace");
        let default_root =
            ServerConfig::embedded_runtime_store_root_for_data_dir(&data_dir, TEST_WORKSPACE_ID);
        assert_eq!(
            default_root,
            data_dir
                .join("workspace-server")
                .join(TEST_WORKSPACE_ID)
                .join("embedded-runtime")
        );
        assert!(!default_root.starts_with(workspace_root.join(".yoi")));

        let config = ServerConfig::local_dev(workspace_root, test_identity())
            .with_embedded_runtime_store_root(default_root.clone());
        let app = build_router(
            WorkspaceApi::new_with_execution_backend(
                config,
                Arc::new(SqliteWorkspaceStore::in_memory().unwrap()),
                Arc::new(DeterministicExecutionBackend::default()),
            )
            .await
            .unwrap(),
        );
        let raw_store_root = default_root.to_string_lossy().to_string();
        for uri in [
            "/api/workspace",
            "/api/hosts",
            "/api/runtimes",
            "/api/workers",
        ] {
            let body = get_json(app.clone(), uri).await;
            let serialized = serde_json::to_string(&body).unwrap();
            assert!(
                !serialized.contains(&raw_store_root),
                "{uri} leaked embedded runtime store root: {serialized}"
            );
        }
    }

    #[tokio::test]
    async fn empty_repository_config_returns_empty_list_with_warning() {
        let root = tempfile::tempdir().unwrap();
        let mut config = test_server_config(root.path());
        config.repositories.clear();
        let api = WorkspaceApi::new_with_execution_backend(
            config,
            Arc::new(SqliteWorkspaceStore::in_memory().unwrap()),
            Arc::new(DeterministicExecutionBackend::default()),
        )
        .await
        .unwrap();
        let app = build_router(api);

        let repositories = get_json(app, "/api/repositories").await;

        assert!(repositories["items"].as_array().unwrap().is_empty());
        assert_eq!(
            repositories["diagnostics"][0]["code"],
            "repository_config_empty"
        );
    }

    #[tokio::test]
    async fn repository_log_rejects_unknown_or_unsupported_configured_repository() {
        let root = tempfile::tempdir().unwrap();
        let mut config = test_server_config(root.path());
        config.repositories = vec![ConfiguredRepository {
            id: "files".to_string(),
            provider: "local_fs".to_string(),
            uri: ".".to_string(),
            path: root.path().to_path_buf(),
            display_name: None,
            default_selector: None,
        }];
        let api = WorkspaceApi::new_with_execution_backend(
            config,
            Arc::new(SqliteWorkspaceStore::in_memory().unwrap()),
            Arc::new(DeterministicExecutionBackend::default()),
        )
        .await
        .unwrap();
        let app = build_router(api);

        let unknown = request_json(
            app.clone(),
            "GET",
            "/api/repositories/main/log",
            None,
            StatusCode::NOT_FOUND,
        )
        .await;
        assert_eq!(
            unknown["diagnostics"][0]["code"],
            "repository_not_configured"
        );

        let unsupported = request_json(
            app,
            "GET",
            "/api/repositories/files/log",
            None,
            StatusCode::BAD_REQUEST,
        )
        .await;
        assert_eq!(
            unsupported["diagnostics"][0]["code"],
            "repository_provider_unsupported"
        );
    }

    #[tokio::test]
    async fn embedded_runtime_api_routes_by_runtime_and_worker_ids_without_leaking_internals() {
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteWorkspaceStore::in_memory().unwrap();
        let config = test_server_config(dir.path());
        let api = WorkspaceApi::new_with_execution_backend(
            config,
            Arc::new(store),
            Arc::new(DeterministicExecutionBackend::default()),
        )
        .await
        .unwrap();
        let app = build_router(api);

        let runtimes = get_json(app.clone(), "/api/runtimes").await;
        let embedded_summary = runtimes["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|runtime| runtime["runtime_id"] == "embedded-worker-runtime")
            .expect("embedded runtime summary");
        assert_eq!(
            embedded_summary["source"]["kind"],
            "embedded_worker_runtime"
        );
        assert_eq!(embedded_summary["source"]["status"], "active");
        assert_eq!(
            embedded_summary["capabilities"]["workspace_scope"],
            "backend_internal"
        );
        assert_eq!(embedded_summary["capabilities"]["has_workspace_fs"], false);

        let spawned = post_json(
            app.clone(),
            "/api/runtimes/embedded-worker-runtime/workers",
            json!({
                "intent": {
                    "kind": "ticket_role",
                    "ticket_id": "00001KVZSGT0Q",
                    "role": "coder"
                },
                "requested_worker_name": "api-friendly-name",
                "acceptance": {
                    "kind": "run_accepted",
                    "expected_segments": 0
                }
            }),
        )
        .await;
        assert_eq!(spawned["state"], "accepted");
        let diagnostics = spawned["diagnostics"].as_array().unwrap();
        assert!(diagnostics.iter().all(|diagnostic| {
            !diagnostic["message"]
                .as_str()
                .unwrap_or_default()
                .contains("/workspace/demo")
        }));
        let worker_id = spawned["worker"]["worker_id"].as_str().unwrap().to_string();
        assert_eq!(spawned["worker"]["runtime_id"], "embedded-worker-runtime");
        assert_eq!(
            spawned["worker"]["workspace"]["visibility"],
            "backend_internal"
        );
        assert_eq!(
            spawned["worker"]["implementation"]["kind"],
            "embedded_worker_runtime"
        );

        let worker = get_json(
            app.clone(),
            &format!("/api/runtimes/embedded-worker-runtime/workers/{worker_id}"),
        )
        .await;
        assert_eq!(worker["worker_id"], worker_id);
        assert_eq!(worker["runtime_id"], "embedded-worker-runtime");

        let accepted = post_json(
            app.clone(),
            &format!("/api/runtimes/embedded-worker-runtime/workers/{worker_id}/input"),
            json!({
                "kind": "user",
                "content": "hello from browser-facing api"
            }),
        )
        .await;
        assert_eq!(accepted["state"], "accepted");
        assert_eq!(accepted["runtime_id"], "embedded-worker-runtime");
        assert_eq!(accepted["worker_id"], worker_id);
        assert!(accepted["diagnostics"].as_array().unwrap().is_empty());

        let transcript_route = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!(
                        "/api/runtimes/embedded-worker-runtime/workers/{worker_id}/transcript?start=0&limit=10"
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(transcript_route.status(), StatusCode::NOT_FOUND);

        let wrong_runtime = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/api/runtimes/unknown-runtime/workers/{worker_id}/input"
                    ))
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&json!({
                            "kind": "user",
                            "content": "wrong runtime"
                        }))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(wrong_runtime.status(), StatusCode::NOT_FOUND);

        let projected = format!("{}{}{}{}", embedded_summary, spawned, worker, accepted);
        for forbidden in [
            dir.path().to_string_lossy().as_ref(),
            "metadata.json",
            "socket",
            "session",
            "token",
            "credential",
            "provider",
        ] {
            assert!(
                !projected.contains(forbidden),
                "embedded api projection leaked forbidden term: {forbidden}: {projected}"
            );
        }
    }

    #[tokio::test]
    async fn proxies_worker_protocol_ws_as_raw_events() {
        let (runtime, worker_ref, endpoint) = spawn_runtime_worker().await;
        let source = RuntimeObservationSourceConfig {
            runtime_id: "runtime-a".into(),
            worker_id: "worker-a".into(),
            endpoint,
            bearer_token: None,
        };
        let (url, _dir) = spawn_workspace_proxy(source).await;
        let (mut stream, _) = connect_async(&url).await.unwrap();
        assert!(matches!(
            next_client_frame(&mut stream).await,
            protocol::Event::Snapshot { .. }
        ));

        stream
            .send(Message::Text(
                serde_json::to_string(&protocol::Method::ListCompletions {
                    kind: protocol::CompletionKind::File,
                    prefix: String::new(),
                })
                .unwrap()
                .into(),
            ))
            .await
            .unwrap();
        assert!(matches!(
            next_client_frame(&mut stream).await,
            protocol::Event::Completions { .. }
        ));

        runtime
            .observe_worker_event(
                &worker_ref,
                protocol::Event::TextDelta {
                    text: "live".into(),
                },
            )
            .unwrap();
        assert!(matches!(
            next_client_frame(&mut stream).await,
            protocol::Event::TextDelta { .. }
        ));
    }

    #[tokio::test]
    async fn proxy_maps_runtime_worker_not_found_http_404_to_protocol_error_event() {
        let (_runtime, _worker_ref, endpoint) = spawn_runtime_worker().await;
        let endpoint = endpoint.replace("/protocol/ws", "/missing-worker/protocol/ws");
        let source = RuntimeObservationSourceConfig {
            runtime_id: "runtime-a".into(),
            worker_id: "worker-a".into(),
            endpoint,
            bearer_token: None,
        };
        let (url, _dir) = spawn_workspace_proxy(source).await;
        let (mut stream, _) = connect_async(&url).await.unwrap();
        assert!(matches!(
            next_client_frame(&mut stream).await,
            protocol::Event::Error { .. }
        ));
    }

    async fn next_client_frame(
        stream: &mut tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    ) -> protocol::Event {
        let message = stream.next().await.unwrap().unwrap();
        let Message::Text(text) = message else {
            panic!("expected text frame");
        };
        serde_json::from_str(&text).unwrap()
    }

    async fn spawn_runtime_worker() -> (
        worker_runtime::Runtime,
        worker_runtime::identity::WorkerRef,
        String,
    ) {
        let (runtime, worker_ref) = runtime_with_worker();
        let runtime_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let runtime_addr = runtime_listener.local_addr().unwrap();
        tokio::spawn({
            let runtime = runtime.clone();
            async move {
                worker_runtime::http_server::serve_runtime_http(runtime, runtime_listener, None)
                    .await
                    .unwrap()
            }
        });
        let endpoint = format!(
            "ws://{runtime_addr}/v1/workers/{}/protocol/ws",
            worker_ref.worker_id
        );
        (runtime, worker_ref, endpoint)
    }

    async fn spawn_workspace_proxy(
        source: RuntimeObservationSourceConfig,
    ) -> (String, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteWorkspaceStore::in_memory().unwrap();
        let mut config = test_server_config(dir.path());
        let runtime_id = source.runtime_id.clone();
        let worker_id = source.worker_id.clone();
        config.runtime_event_sources.push(source);
        let api = WorkspaceApi::new_with_execution_backend(
            config,
            Arc::new(store),
            Arc::new(DeterministicExecutionBackend::default()),
        )
        .await
        .unwrap();
        let app_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let app_addr = app_listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(app_listener, build_router(api)).await.unwrap() });
        (
            format!("ws://{app_addr}/api/runtimes/{runtime_id}/workers/{worker_id}/protocol/ws"),
            dir,
        )
    }
    async fn get_json(app: Router, uri: &str) -> Value {
        let response = app
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{uri}");
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    async fn request_json(
        app: Router,
        method: &str,
        uri: &str,
        body: Option<Value>,
        expected_status: StatusCode,
    ) -> Value {
        let mut builder = Request::builder().method(method).uri(uri);
        let request_body = if let Some(body) = body {
            builder = builder.header(CONTENT_TYPE, "application/json");
            Body::from(serde_json::to_vec(&body).unwrap())
        } else {
            Body::empty()
        };
        let response = app
            .oneshot(builder.body(request_body).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), expected_status, "{method} {uri}");
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap_or_else(
            |_| serde_json::json!({ "message": String::from_utf8_lossy(&bytes).to_string() }),
        )
    }

    async fn post_json(app: Router, uri: &str, body: Value) -> Value {
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(uri)
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{uri}");
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    fn write_ticket(root: &Path, id: &str, title: &str, state: &str) {
        let ticket_dir = root.join(".yoi/tickets").join(id);
        std::fs::create_dir_all(&ticket_dir).unwrap();
        std::fs::write(
            ticket_dir.join("item.md"),
            format!(
                r#"---
title: "{title}"
state: "{state}"
created_at: "2026-01-01T00:00:00Z"
updated_at: "2026-01-02T00:00:00Z"
---

Ticket body.
"#,
            ),
        )
        .unwrap();
        std::fs::write(ticket_dir.join("thread.md"), "").unwrap();
    }

    fn write_objective(root: &Path, id: &str, title: &str, state: &str) {
        let objective_dir = root.join(".yoi/objectives").join(id);
        std::fs::create_dir_all(&objective_dir).unwrap();
        std::fs::write(
            objective_dir.join("item.md"),
            format!(
                r#"---
title: "{title}"
state: "{state}"
created_at: "2026-01-01T00:00:00Z"
updated_at: "2026-01-02T00:00:00Z"
linked_tickets: ["00000000001J2"]
---

Objective body.
"#,
            ),
        )
        .unwrap();
    }
}
