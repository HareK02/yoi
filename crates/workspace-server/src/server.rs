use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};

use axum::extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade};
use axum::extract::{Path as AxumPath, Query, Request, State};
use axum::http::header::{CONTENT_TYPE, ETAG, IF_NONE_MATCH, LOCATION, ORIGIN, SET_COOKIE};
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, patch, post, put};
use axum::{Json, Router};
use chrono::{Duration, SecondsFormat, Utc};
use config_source::ConfigTreeSnapshot;
use flow::{FlowSourceKind, FlowSourceResolveRequest, ResolvedFlowSource};
use futures::{SinkExt, StreamExt};
use memory::backend::{
    MemoryBackendHttpResponse, MemoryBackendOperation, MemoryConsolidateStagingOperation,
    MemoryConsolidationOutput,
};
use protocol::Segment;
use protocol::stream::{decode_method, encode_event};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use ticket::{
    MarkdownText, NewTicketEvent, TicketBackend, TicketBodyReplacement, TicketEventKind,
    TicketIdOrSlug, TicketItemEdit, TicketStateChange, TicketTargetEdit, TicketWorkflowState,
};
use ticket::{
    SqliteTicketBackend, TicketBackendOperation, TicketBackendOperationResult,
    execute_ticket_backend_operation,
};
use tokio::net::TcpListener;
use tokio::sync::Mutex as AsyncMutex;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message as TungsteniteMessage;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tower::ServiceExt;
use url::Url;
use uuid::Uuid;
use webauthn_rs::prelude::{
    CreationChallengeResponse, Passkey, PasskeyAuthentication, PasskeyRegistration,
    PublicKeyCredential, RegisterPublicKeyCredential, RequestChallengeResponse, Webauthn,
    WebauthnBuilder,
};
use workdir::WorkdirSessionHandle;
use workdir::http::{WorkdirSessionOperation, WorkdirSessionOperationResult};
use workdir::workspace::{
    MaterializerKind, WorkingDirectoryCleanupTarget,
    WorkingDirectoryDetailResponse as BrowserWorkingDirectoryDetailResponse,
    WorkingDirectoryDiagnostic, WorkingDirectoryDiagnosticSeverity,
    WorkingDirectoryListResponse as BrowserWorkingDirectoryListResponse, WorkingDirectoryOccupancy,
    WorkingDirectoryStatusKind, WorkingDirectorySummary, WorkspaceWorkdirSessionFence,
    WorkspaceWorkdirSessionOperationRequest,
};
use worker::feature::builtin::{WorkerObservationSubject, WorkerObservationSubjectRef};
use worker_runtime::resource::{BackendResourceError, BackendResourceFetchRequest};
use worker_runtime::worker_backend::{ProfileRuntimeWorkerFactory, WorkerRuntimeExecutionBackend};
use workspace_api::{
    ObjectiveCreateRequest, ObjectiveEditRequest, ObjectiveLinkTicketRequest,
    ObjectiveStateRequest, TICKET_ORCHESTRATION_PLANS_QUERY_PATH, TICKET_RELATIONS_QUERY_PATH,
};

use crate::auth::{
    ActorAuthMethod, AuthPublicConfig, AuthenticatedUser, RequestActor, SessionCookiePolicy,
    auth_error, is_expired, mint_secret, new_id, new_user_code, normalize_handle, parse_cookie,
    resolve_request_actor, rfc3339_after, session_set_cookie, token_hash,
};
use crate::authority::{
    MemoryAuthority, ObjectiveAuthority, ObjectiveCreateInput, ObjectiveEditInput,
    SqliteWorkspaceAuthority, TicketAuthority, TicketMergeRevisionSource, merge_request_summary,
};
use crate::companion::{
    CompanionCancelRequest, CompanionConsole, CompanionMessageRequest, CompanionMessageResponse,
    CompanionStatusResponse, CompanionTranscriptProjection,
};
use crate::config::{BackendRuntimesConfigFile, RemoteRuntimeConfigFile, resolve_remote_runtime};
use crate::config_source::ConfigCommitRequest;
use crate::hosts::{
    ConfigBundleCheckResult, ConfigBundleSyncResult, DiagnosticSeverity, EMBEDDED_RUNTIME_ID,
    EmbeddedWorkerRuntime, HostSummary, RemoteRuntimeConfig, RemoteWorkerRuntime,
    RuntimeDiagnostic, RuntimeRegistry, RuntimeRegistryError, RuntimeRegistryUnregisterResult,
    TicketWorkerRole, WorkerCapabilitySummary, WorkerCompletionsRequest, WorkerCompletionsResult,
    WorkerControlOperation, WorkerCreateBinding, WorkerImplementationSummary, WorkerInputKind,
    WorkerInputRequest, WorkerInputResult, WorkerLifecycleRequest, WorkerLifecycleResult,
    WorkerOperationState, WorkerRestoreResult, WorkerSpawnAcceptanceRequirement, WorkerSpawnIntent,
    WorkerSpawnRequest, WorkerSpawnResult, WorkerSpawnWorkingDirectoryRequest, WorkerSummary,
    WorkerTicketAssignmentRequest, WorkerWorkspaceSummary, worker_spawn_create_fingerprint,
    workspace_worker_summary,
};
use crate::identity::WorkspaceIdentity;
use crate::memory_backend::execute_memory_backend_operation_with_authority;
use crate::memory_staging::{
    MemoryStagingListResponse, list_memory_staging_from_authority,
    memory_staging_backlog_from_authority,
};
use crate::observation::{
    BackendObservationProxy, ObservationProxyError, RuntimeObservationClient,
    RuntimeObservationSource, RuntimeObservationSourceConfig,
};
use crate::profile_settings::UpdateWorkspaceMetadataRequest;
use crate::records::{
    MergeRequestListItem, MergeRequestListResponse, ObjectiveDetail, ObjectiveQueryRequest,
    ObjectiveQueryResponse, ObjectiveShowRequest, ProjectRecordList, TicketDetail,
    TicketQueryRequest, TicketQueryResponse, TicketShowRequest,
};
use crate::repositories::{
    ConfiguredRepository, RepositoryListProjection, RepositoryLogRead, RepositoryLookupError,
    RepositoryRegistryReader, RepositorySummary,
};
use crate::resource_broker::BackendResourceBroker;
use crate::runtime_settings::RuntimeConfigSchemaProvider;
use crate::runtime_subscription::RuntimeSubscriptionBroker;
use crate::skills;
use crate::store::{
    AccountRecord, ApiTokenRecord, AuthChallengeRecord, BrowserSessionRecord, ControlPlaneStore,
    DeviceLoginFlowRecord, FlowSourceRecord, PasskeyCredentialRecord, RepositoryRecord,
    TicketAssignmentPrincipal, TicketAssignmentRole, TicketCoderAssignmentRecord,
    TicketRoleAssignmentRecord, UserRecord, WorkdirCreateOperationRecord, WorkdirRegistryRecord,
    WorkerControlGrantRecord, WorkerRegistryRecord, WorkerWorkdirLinkRecord, WorkspaceRecord,
    WorkspaceResourceKind,
};
use crate::workspace_catalog::{WorkspaceCatalogService, WorkspaceCreateRequest};
use crate::{Error, Result};
use worker_runtime::catalog::{
    ConfigBundleRef, ProfileSelector, RepositorySelector as RuntimeRepositorySelector,
    WorkingDirectoryClaim, WorkingDirectoryRepository, WorkingDirectoryRequest, WorkspaceApiRef,
};
use worker_runtime::config_bundle::ConfigBundle;
use worker_runtime::http_server::{
    RuntimeHttpConfigBundleAvailabilityResponse, RuntimeHttpConfigBundlesResponse,
    RuntimeHttpSummaryResponse, RuntimeHttpWorkerResponse, RuntimeHttpWorkersResponse,
};
use worker_runtime::identity::{RuntimeWorkerRef, WorkerId};

const EMBEDDED_WORKER_RUNTIME_ID: &str = "embedded-worker-runtime";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum AuthConfig {
    /// Browser human auth uses Passkey ceremonies and HttpOnly cookie sessions;
    /// CLI/TUI auth uses API tokens obtained through the device login flow.
    Passkey {
        rp_id: String,
        origin: String,
        public_base_url: String,
        cookie_name: String,
    },
}

#[derive(Clone)]
pub struct ServerConfig {
    pub workspace_id: String,
    pub workspace_display_name: String,
    pub workspace_created_at: String,
    pub workspace_root: PathBuf,
    pub database_path: PathBuf,
    pub frontend_url: String,
    pub embedded_runtime_store_root: PathBuf,
    pub static_assets_dir: Option<PathBuf>,
    pub auth: AuthConfig,
    pub max_records: usize,
    pub repositories: Vec<ConfiguredRepository>,
    pub runtime_event_sources: Vec<RuntimeObservationSourceConfig>,
    pub remote_runtime_sources: Vec<RemoteRuntimeConfig>,
    pub runtime_config_path: Option<PathBuf>,
    pub backend_base_url: Option<String>,
    /// Allows the first ownerless Workspace to be created without a session.
    /// This must only be enabled for a loopback-bound local Server.
    pub allow_local_workspace_bootstrap: bool,
}

impl ServerConfig {
    pub fn local_dev(workspace_root: impl Into<PathBuf>, identity: WorkspaceIdentity) -> Self {
        let workspace_root = workspace_root.into();
        let workspace_id = identity.workspace_id;
        let embedded_runtime_store_root = Self::default_embedded_runtime_store_root(&workspace_id);
        let database_path = Self::default_server_database_path();
        Self {
            workspace_id,
            workspace_display_name: identity.display_name,
            workspace_created_at: identity.created_at,
            workspace_root,
            database_path,
            frontend_url: "http://127.0.0.1:5173".to_string(),
            embedded_runtime_store_root,
            static_assets_dir: None,
            auth: AuthConfig::Passkey {
                rp_id: "localhost".to_string(),
                origin: "http://localhost:8787".to_string(),
                public_base_url: "http://localhost:8787".to_string(),
                cookie_name: "yoi_workspace_session".to_string(),
            },
            max_records: 200,
            repositories: Vec::new(),
            runtime_event_sources: Vec::new(),
            remote_runtime_sources: Vec::new(),
            runtime_config_path: BackendRuntimesConfigFile::default_path(),
            backend_base_url: None,
            allow_local_workspace_bootstrap: false,
        }
    }

    pub fn server_data_root_for_data_dir(data_dir: impl Into<PathBuf>) -> PathBuf {
        data_dir.into().join("server")
    }

    pub fn default_server_data_root() -> PathBuf {
        match manifest::paths::data_dir() {
            Some(data_dir) => Self::server_data_root_for_data_dir(data_dir),
            None => std::env::temp_dir().join("yoi").join("server"),
        }
    }

    pub fn server_database_path_for_data_dir(data_dir: impl Into<PathBuf>) -> PathBuf {
        Self::server_data_root_for_data_dir(data_dir).join("server.db")
    }

    pub fn default_server_database_path() -> PathBuf {
        Self::default_server_data_root().join("server.db")
    }

    pub fn workspace_backend_data_root_for_data_dir(
        data_dir: impl Into<PathBuf>,
        workspace_id: impl AsRef<str>,
    ) -> PathBuf {
        Self::server_data_root_for_data_dir(data_dir)
            .join("workspaces")
            .join(workspace_id.as_ref())
    }

    pub fn default_workspace_backend_data_root(workspace_id: impl AsRef<str>) -> PathBuf {
        match manifest::paths::data_dir() {
            Some(data_dir) => {
                Self::workspace_backend_data_root_for_data_dir(data_dir, workspace_id.as_ref())
            }
            None => std::env::temp_dir()
                .join("yoi")
                .join("server")
                .join("workspaces")
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

    pub fn with_local_workspace_bootstrap(mut self, enabled: bool) -> Self {
        self.allow_local_workspace_bootstrap = enabled;
        self
    }

    pub fn with_embedded_runtime_store_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.embedded_runtime_store_root = root.into();
        self
    }

    fn for_catalog_workspace(
        &self,
        workspace: &WorkspaceRecord,
        repositories: Vec<RepositoryRecord>,
    ) -> Result<Self> {
        if repositories.is_empty() {
            return Err(Error::Config(format!(
                "Workspace {} has no registered repository",
                workspace.workspace_id
            )));
        }
        let workspace_data_root =
            Self::default_workspace_backend_data_root(&workspace.workspace_id);
        let workspace_root = workspace_data_root.clone();
        let repositories = repositories
            .into_iter()
            .map(|repository| ConfiguredRepository {
                id: repository.repository_id,
                provider: repository.provider.unwrap_or(repository.kind),
                path: repository_local_path(&repository.source),
                source: repository.source,
                source_revision: repository.source_revision,
                source_fingerprint: repository.source_fingerprint,
                observed_status: repository.observed_status,
                observed_at: repository.observed_at,
                display_name: Some(repository.name),
                default_selector: repository.default_ref,
            })
            .collect();
        let mut scoped = self.clone();
        scoped.workspace_id.clone_from(&workspace.workspace_id);
        scoped
            .workspace_display_name
            .clone_from(&workspace.display_name);
        scoped
            .workspace_created_at
            .clone_from(&workspace.created_at);
        scoped.workspace_root = workspace_root;
        scoped.embedded_runtime_store_root =
            Self::default_embedded_runtime_store_root(&workspace.workspace_id);
        scoped.repositories = repositories;
        // Runtime trust is server-global. Only explicitly assigned sources enter
        // this Workspace's registry and receive Workspace-scoped capabilities.
        scoped.remote_runtime_sources.retain(|runtime| {
            runtime.workspace_id.as_deref() == Some(workspace.workspace_id.as_str())
        });
        scoped.runtime_event_sources.clear();
        Ok(scoped)
    }
}

fn repository_local_path(source: &workspace_api::RepositorySource) -> Option<PathBuf> {
    match source.kind {
        workspace_api::RepositorySourceKind::LocalPath => Some(PathBuf::from(&source.uri)),
        workspace_api::RepositorySourceKind::File => url::Url::parse(&source.uri)
            .ok()
            .and_then(|uri| uri.to_file_path().ok()),
        workspace_api::RepositorySourceKind::Ssh
        | workspace_api::RepositorySourceKind::Http
        | workspace_api::RepositorySourceKind::Https
        | workspace_api::RepositorySourceKind::Invalid => None,
    }
}

const ORCHESTRATOR_ATTENTION_TICKET_LIMIT: usize = 20;
const ORCHESTRATOR_ATTENTION_PROMPT_NAME: &str = "internal.workspace_orchestrator_queue_attention";
static EMBEDDED_RUNTIME_REQUEST_IDENTITY: std::sync::LazyLock<
    worker_runtime::auth::RuntimeIdentityMaterial,
> = std::sync::LazyLock::new(|| {
    worker_runtime::auth::RuntimeIdentityMaterial::generate(EMBEDDED_RUNTIME_ID)
        .expect("embedded Runtime request identity generation must succeed")
});

#[derive(Clone)]
pub struct WorkspaceApi {
    pub(crate) config: ServerConfig,
    pub(crate) store: Arc<dyn ControlPlaneStore>,
    config_store: Arc<crate::SqliteWorkspaceStore>,
    config_schema_registry: crate::config_source::WorkspaceConfigSchemaRegistry,
    prompt_projection_cache: crate::prompt_settings::WorkspacePromptProjectionCache,
    authority: SqliteWorkspaceAuthority,
    runtime: Arc<RuntimeRegistry>,
    companion: Arc<CompanionConsole>,
    orchestrator_spawn_lock: Arc<std::sync::Mutex<()>>,
    orchestrator_attention_fingerprint: Arc<Mutex<Option<String>>>,
    observation_proxy: BackendObservationProxy,
    runtime_subscription_broker: RuntimeSubscriptionBroker,
    resource_broker: BackendResourceBroker,
    workdir_sessions: Arc<Mutex<HashMap<RuntimeWorkerRef, WorkdirSessionHandle>>>,
    workdir_session_locks: Arc<Mutex<HashMap<RuntimeWorkerRef, Arc<tokio::sync::Mutex<()>>>>>,
    worker_remove_locks: Arc<Mutex<HashMap<RuntimeWorkerRef, Arc<tokio::sync::Mutex<()>>>>>,
    worker_control_locks: Arc<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>>,
}

#[derive(Clone)]
struct ServerAuthApi {
    config: ServerConfig,
    store: Arc<dyn ControlPlaneStore>,
}

impl From<&WorkspaceApi> for ServerAuthApi {
    fn from(api: &WorkspaceApi) -> Self {
        Self {
            config: api.config.clone(),
            store: api.store.clone(),
        }
    }
}

#[derive(Clone)]
struct WorkspaceWorkerRemoveExecutor {
    workspace_id: String,
    store: Arc<dyn ControlPlaneStore>,
    runtime: Weak<RuntimeRegistry>,
    workdir_sessions: Arc<Mutex<HashMap<RuntimeWorkerRef, WorkdirSessionHandle>>>,
    workdir_session_locks: Arc<Mutex<HashMap<RuntimeWorkerRef, Arc<tokio::sync::Mutex<()>>>>>,
    worker_remove_locks: Arc<Mutex<HashMap<RuntimeWorkerRef, Arc<tokio::sync::Mutex<()>>>>>,
    worker_control_locks: Arc<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>>,
}

impl WorkspaceWorkerRemoveExecutor {
    fn new(api: &WorkspaceApi) -> Self {
        Self {
            workspace_id: api.config.workspace_id.clone(),
            store: api.store.clone(),
            runtime: Arc::downgrade(&api.runtime),
            workdir_sessions: api.workdir_sessions.clone(),
            workdir_session_locks: api.workdir_session_locks.clone(),
            worker_remove_locks: api.worker_remove_locks.clone(),
            worker_control_locks: api.worker_control_locks.clone(),
        }
    }

    async fn resume_worker_retention(
        &self,
        runtime: &RuntimeRegistry,
        target: &RuntimeWorkerRef,
        prepared: crate::retention::PreparedWorkerRemoval,
    ) -> std::result::Result<worker::WorkspaceResponse, String> {
        let result =
            match runtime.execute_worker_retention(target, prepared.runtime_request.clone()) {
                Ok(result) => result,
                Err(_) => {
                    return Ok(worker_remove_error_response(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "runtime_retention_failed",
                        "Runtime retention recovery failed; removal can be retried",
                    ));
                }
            };
        match self.store.commit_worker_removal(
            &self.workspace_id,
            &prepared.plan.operation_id,
            &prepared.plan.input_fingerprint,
            &result,
        ) {
            Ok(_) => Ok(worker_remove_success_response(target)),
            Err(error) => Ok(worker_retention_error_response(error)),
        }
    }

    async fn execute_async(
        &self,
        source: crate::worker_source::VerifiedWorkerMutationSource,
        target_runtime_id: &str,
        target_worker_id: &str,
        reason: &str,
    ) -> std::result::Result<worker::WorkspaceResponse, String> {
        let reason = reason.trim();
        if reason.is_empty() || reason.len() > 512 {
            return Ok(worker_remove_error_response(
                StatusCode::BAD_REQUEST,
                "invalid_reason",
                "WorkerRemove reason must be between 1 and 512 bytes",
            ));
        }
        let runtime = self.runtime.upgrade().ok_or_else(|| {
            "Workspace Runtime registry is unavailable during WorkerRemove".to_string()
        })?;
        let target = RuntimeWorkerRef {
            runtime_id: target_runtime_id.to_string(),
            worker_id: target_worker_id.to_string(),
        };
        if source.runtime_id == target_runtime_id && source.worker_id == target_worker_id {
            return Ok(worker_remove_error_response(
                StatusCode::CONFLICT,
                "self_removal_forbidden",
                "The current Orchestrator cannot remove itself",
            ));
        }
        let controller = RuntimeWorkerRef::new(&source.runtime_id, &source.worker_id);
        let grant = self
            .store
            .get_active_worker_control_grant(&self.workspace_id, &controller, &target)
            .map_err(|_| "Worker control grant authority is unavailable".to_string())?
            .filter(|grant| {
                grant
                    .permissions
                    .iter()
                    .any(|permission| permission == "remove")
            });
        let Some(grant) = grant else {
            return Ok(worker_remove_error_response(
                StatusCode::NOT_FOUND,
                "unknown_worker",
                "The target Worker is not known to the current Worker",
            ));
        };
        let control_lock = {
            let mut locks = self
                .worker_control_locks
                .lock()
                .map_err(|_| "Worker control lock registry was poisoned".to_string())?;
            locks
                .entry(grant.grant_id.clone())
                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
                .clone()
        };
        let _control_guard = control_lock.lock().await;
        let still_granted = self
            .store
            .get_active_worker_control_grant(&self.workspace_id, &controller, &target)
            .map_err(|_| "Worker control grant authority is unavailable".to_string())?
            .is_some_and(|current| {
                current.grant_id == grant.grant_id
                    && current
                        .permissions
                        .iter()
                        .any(|permission| permission == "remove")
            });
        if !still_granted {
            return Ok(worker_remove_error_response(
                StatusCode::NOT_FOUND,
                "unknown_worker",
                "The target Worker is not known to the current Worker",
            ));
        }

        let remove_lock = {
            let mut locks = self
                .worker_remove_locks
                .lock()
                .map_err(|_| "WorkerRemove lock registry was poisoned".to_string())?;
            locks
                .entry(target.clone())
                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
                .clone()
        };
        let _remove_guard = remove_lock.lock().await;
        let workdir_session_lock = {
            let mut locks = self
                .workdir_session_locks
                .lock()
                .map_err(|_| "Workdir session lock registry was poisoned".to_string())?;
            locks
                .entry(target.clone())
                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
                .clone()
        };
        let _workdir_session_guard = workdir_session_lock.lock().await;

        let prepared = self
            .store
            .recover_worker_removal_execution(&self.workspace_id, &target)
            .map_err(|_| "Worker removal recovery authority is unavailable".to_string())?;
        if let Some(prepared) = prepared {
            if prepared.plan.state == crate::retention::WorkerRemovalPlanState::Succeeded {
                return Ok(worker_remove_success_response(&target));
            }
            let prepared = if matches!(
                prepared.plan.state,
                crate::retention::WorkerRemovalPlanState::Planned
                    | crate::retention::WorkerRemovalPlanState::Failed
            ) {
                match self.store.prepare_worker_removal_execution(
                    &self.workspace_id,
                    &prepared.plan.plan_id,
                    &prepared.plan.input_fingerprint,
                ) {
                    Ok(prepared) => prepared,
                    Err(error) => return Ok(worker_retention_error_response(error)),
                }
            } else {
                prepared
            };
            let session = {
                self.workdir_sessions
                    .lock()
                    .map_err(|_| "Workdir session registry was poisoned".to_string())?
                    .get(&target)
                    .cloned()
            };
            if let Some(session) = session {
                if session.close().await.is_err() {
                    let _ = self.store.fail_worker_removal(
                        &self.workspace_id,
                        &prepared.plan.operation_id,
                        &prepared.plan.input_fingerprint,
                        "workdir_session_close_failed",
                    );
                    return Ok(worker_remove_error_response(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "attachment_close_failed",
                        "Worker Workdir session could not be closed; removal can be retried",
                    ));
                }
                self.workdir_sessions
                    .lock()
                    .map_err(|_| "Workdir session registry was poisoned".to_string())?
                    .remove(&target);
            }
            if self
                .store
                .detach_worker_workdir(
                    &self.workspace_id,
                    &target,
                    None,
                    &Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
                )
                .is_err()
            {
                let _ = self.store.fail_worker_removal(
                    &self.workspace_id,
                    &prepared.plan.operation_id,
                    &prepared.plan.input_fingerprint,
                    "workdir_attachment_release_failed",
                );
                return Ok(worker_remove_error_response(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "attachment_release_failed",
                    "Worker Workdir attachment could not be released; removal can be retried",
                ));
            }
            return self
                .resume_worker_retention(&runtime, &target, prepared)
                .await;
        }

        let worker = match runtime.worker(&target) {
            Ok(worker) => worker,
            Err(_) => {
                return Ok(worker_remove_error_response(
                    StatusCode::NOT_FOUND,
                    "worker_not_found",
                    "Worker was not found in this Workspace",
                ));
            }
        };
        if worker.singleton_key.is_some() {
            return Ok(worker_remove_error_response(
                StatusCode::CONFLICT,
                "internal_worker_forbidden",
                "Internal service Workers cannot be removed with WorkerRemove",
            ));
        }
        if !worker.state.eq_ignore_ascii_case("stopped") {
            return Ok(worker_remove_error_response(
                StatusCode::CONFLICT,
                "worker_not_stopped",
                "Worker must be stopped before removal",
            ));
        }

        let inventory = match runtime.worker_retention_inventory(&target) {
            Ok(inventory) => inventory,
            Err(_) => {
                return Ok(worker_remove_error_response(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "retention_inventory_unavailable",
                    "Retention inventory could not be loaded; removal can be retried",
                ));
            }
        };
        let request = crate::retention::WorkerRemovalPlanRequest {
            workspace_id: self.workspace_id.clone(),
            worker: target.clone(),
            reason: reason.to_string(),
        };
        let plan = match self.store.plan_worker_removal(&request, &inventory) {
            Ok(plan) => plan,
            Err(error) => return Ok(worker_retention_error_response(error)),
        };
        let prepared = match self.store.prepare_worker_removal_execution(
            &self.workspace_id,
            &plan.plan_id,
            &plan.input_fingerprint,
        ) {
            Ok(prepared) => prepared,
            Err(error) => return Ok(worker_retention_error_response(error)),
        };

        let session = self
            .workdir_sessions
            .lock()
            .map_err(|_| "Workdir session registry was poisoned".to_string())?
            .get(&target)
            .cloned();
        if let Some(session) = session {
            if session.close().await.is_err() {
                let _ = self.store.fail_worker_removal(
                    &self.workspace_id,
                    &plan.operation_id,
                    &plan.input_fingerprint,
                    "workdir_session_close_failed",
                );
                return Ok(worker_remove_error_response(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "attachment_close_failed",
                    "Worker Workdir session could not be closed; removal can be retried",
                ));
            }
            self.workdir_sessions
                .lock()
                .map_err(|_| "Workdir session registry was poisoned".to_string())?
                .remove(&target);
        }

        if let Err(_) = self.store.detach_worker_workdir(
            &self.workspace_id,
            &target,
            None,
            &Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        ) {
            let _ = self.store.fail_worker_removal(
                &self.workspace_id,
                &plan.operation_id,
                &plan.input_fingerprint,
                "workdir_attachment_release_failed",
            );
            return Ok(worker_remove_error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "attachment_release_failed",
                "Worker Workdir attachment could not be released; removal can be retried",
            ));
        }

        let retention_result =
            match runtime.execute_worker_retention(&target, prepared.runtime_request.clone()) {
                Ok(result) => result,
                Err(_) => {
                    let _ = self.store.fail_worker_removal(
                        &self.workspace_id,
                        &plan.operation_id,
                        &plan.input_fingerprint,
                        "runtime_retention_failed",
                    );
                    return Ok(worker_remove_error_response(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "runtime_retention_failed",
                        "Runtime retention execution failed; removal can be retried",
                    ));
                }
            };
        match self.store.commit_worker_removal(
            &self.workspace_id,
            &plan.operation_id,
            &plan.input_fingerprint,
            &retention_result,
        ) {
            Ok(_) => {}
            Err(error) => {
                let _ = self.store.fail_worker_removal(
                    &self.workspace_id,
                    &plan.operation_id,
                    &plan.input_fingerprint,
                    "metadata_commit_failed",
                );
                return Ok(worker_retention_error_response(error));
            }
        };
        Ok(worker_remove_success_response(&target))
    }
}

impl crate::worker_source::VerifiedWorkerRemoveExecutor for WorkspaceWorkerRemoveExecutor {
    fn execute(
        &self,
        source: crate::worker_source::VerifiedWorkerMutationSource,
        target_runtime_id: &str,
        target_worker_id: &str,
        reason: &str,
    ) -> std::result::Result<worker::WorkspaceResponse, String> {
        let executor = self.clone();
        let target_runtime_id = target_runtime_id.to_string();
        let target_worker_id = target_worker_id.to_string();
        let reason = reason.to_string();
        std::thread::spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|error| error.to_string())?
                .block_on(executor.execute_async(
                    source,
                    &target_runtime_id,
                    &target_worker_id,
                    &reason,
                ))
        })
        .join()
        .map_err(|_| "embedded WorkerRemove executor thread panicked".to_string())?
    }
}

#[derive(Clone)]
pub struct WorkspaceServerApi {
    template: Arc<ServerConfig>,
    store: Arc<dyn ControlPlaneStore>,
    catalog: WorkspaceCatalogService,
    routers: Arc<AsyncMutex<HashMap<String, Router>>>,
}

impl WorkspaceServerApi {
    pub fn new(template: ServerConfig, store: Arc<dyn ControlPlaneStore>) -> Self {
        Self {
            template: Arc::new(template),
            catalog: WorkspaceCatalogService::new(store.clone()),
            store,
            routers: Arc::new(AsyncMutex::new(HashMap::new())),
        }
    }

    async fn router_for_workspace(&self, workspace_id: &str) -> Result<Option<Router>> {
        let mut routers = self.routers.lock().await;
        if let Some(router) = routers.get(workspace_id) {
            return Ok(Some(router.clone()));
        }
        let Some(workspace) = self.store.get_workspace(workspace_id).await? else {
            return Ok(None);
        };
        let repositories = self.store.list_repositories(workspace_id)?;
        let config = self
            .template
            .for_catalog_workspace(&workspace, repositories)?;
        let api = WorkspaceApi::new(config, self.store.clone()).await?;
        tokio::spawn(run_orchestrator_turn_end_hook(api.clone()));
        let router = build_inner_router(api);
        routers.insert(workspace_id.to_string(), router.clone());
        Ok(Some(router))
    }

    async fn preload(&self) -> Result<()> {
        for workspace in self.store.list_workspaces()? {
            let _ = self
                .router_for_workspace(&workspace.workspace_id)
                .await?
                .ok_or_else(|| {
                    Error::Config(format!(
                        "Workspace {} disappeared while loading",
                        workspace.workspace_id
                    ))
                })?;
        }
        Ok(())
    }
}

fn server_error_response(error: Error) -> Response {
    ApiError::from(error).into_response()
}

fn forbidden_server_response(message: &str) -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(serde_json::json!({ "error": message })),
    )
        .into_response()
}

#[derive(Debug, Deserialize)]
struct WorkspaceListQuery {
    limit: Option<usize>,
}

async fn list_server_workspaces(
    State(api): State<WorkspaceServerApi>,
    headers: HeaderMap,
    Query(query): Query<WorkspaceListQuery>,
) -> Response {
    let owner = match resolve_server_actor(&api, &headers).await {
        Ok(Some(actor)) => Some(actor.account_id),
        Ok(None) => match api.catalog.list(None, 1) {
            Ok(workspaces) if workspaces.is_empty() => return Json(workspaces).into_response(),
            Ok(_) => return StatusCode::UNAUTHORIZED.into_response(),
            Err(error) => return server_error_response(error),
        },
        Err(error) => return server_error_response(error),
    };
    match api
        .catalog
        .list(owner.as_deref(), query.limit.unwrap_or(100))
    {
        Ok(workspaces) => Json(workspaces).into_response(),
        Err(error) => server_error_response(error),
    }
}

async fn create_server_workspace(
    State(api): State<WorkspaceServerApi>,
    headers: HeaderMap,
    Json(request): Json<WorkspaceCreateRequest>,
) -> Response {
    let (owner_account_id, local_bootstrap) = match resolve_server_actor(&api, &headers).await {
        Ok(Some(actor)) => (Some(actor.account_id), false),
        Ok(None) if api.template.allow_local_workspace_bootstrap => (None, true),
        Ok(None) => {
            return forbidden_server_response("Workspace creation requires an authenticated owner");
        }
        Err(error) => return server_error_response(error),
    };
    let created = match if local_bootstrap {
        api.catalog.create_first_ownerless(request)
    } else {
        api.catalog.create(request, owner_account_id)
    } {
        Ok(created) => created,
        Err(error) => return server_error_response(error),
    };
    if let Err(error) = api
        .router_for_workspace(&created.workspace.workspace_id)
        .await
    {
        return server_error_response(error);
    }
    let status = if created.replayed {
        StatusCode::OK
    } else {
        StatusCode::CREATED
    };
    (status, Json(created)).into_response()
}

async fn resolve_server_actor(
    api: &WorkspaceServerApi,
    headers: &HeaderMap,
) -> std::result::Result<Option<RequestActor>, Error> {
    let cookie_name = auth_public_config(api.template.as_ref()).cookie_name;
    resolve_request_actor(api.store.as_ref(), headers, &cookie_name).await
}

fn signed_request_target(uri: &Uri) -> &str {
    uri.path_and_query()
        .map(|path_and_query| path_and_query.as_str())
        .unwrap_or_else(|| uri.path())
}

async fn authorize_scoped_workspace_request(
    api: &WorkspaceServerApi,
    workspace_id: &str,
    request: &mut Request,
) -> std::result::Result<(), Response> {
    let proof = request
        .headers()
        .get(worker_runtime::auth::RUNTIME_REQUEST_SOURCE_PROOF_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    if let Some(proof) = proof {
        let method = request.method().as_str().to_owned();
        let path = signed_request_target(request.uri()).to_owned();
        let body = std::mem::take(request.body_mut());
        let body = axum::body::to_bytes(body, 16 * 1024 * 1024)
            .await
            .map_err(|_| StatusCode::BAD_REQUEST.into_response())?;
        let digest = worker_runtime::auth::request_body_digest(&body);
        *request.body_mut() = axum::body::Body::from(body);
        let permission = if path.starts_with("/api/runtime/v1/workspaces/")
            || path.contains("/profile-source-archives/")
        {
            worker_runtime::auth::BACKEND_RESOURCE_FETCH_PERMISSION
        } else {
            worker_runtime::auth::WORKSPACE_REQUEST_PERMISSION
        };
        let source = crate::worker_source::verify_runtime_request_source_proof_with_store(
            api.store.as_ref(),
            api.template.as_ref(),
            &proof,
            workspace_id,
            permission,
            &method,
            &path,
            &digest,
        )
        .await
        .map_err(|_| StatusCode::UNAUTHORIZED.into_response())?;
        request.extensions_mut().insert(source);
        return Ok(());
    }

    let actor = resolve_server_actor(api, request.headers())
        .await
        .map_err(server_error_response)?
        .ok_or_else(|| StatusCode::UNAUTHORIZED.into_response())?;

    let cookie_authenticated = matches!(actor.auth_method, ActorAuthMethod::BrowserSession);
    let mutating = !matches!(
        *request.method(),
        Method::GET | Method::HEAD | Method::OPTIONS
    );
    if cookie_authenticated && mutating {
        let AuthConfig::Passkey { origin, .. } = &api.template.auth;
        if origin
            != request
                .headers()
                .get(ORIGIN)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
        {
            return Err(StatusCode::FORBIDDEN.into_response());
        }
    }
    Ok(())
}

async fn authorize_workspace_api_request(
    State(api): State<WorkspaceApi>,
    mut request: Request,
    next: axum::middleware::Next,
) -> Response {
    if !request.uri().path().starts_with("/api/") {
        return next.run(request).await;
    }
    let public_server_api = is_server_global_forward(request.uri().path());
    let workspace_id = api.workspace_id().to_owned();
    let proof = request
        .headers()
        .get(worker_runtime::auth::RUNTIME_REQUEST_SOURCE_PROOF_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    if let Some(proof) = proof {
        let method = request.method().as_str().to_owned();
        let path = signed_request_target(request.uri()).to_owned();
        let body = std::mem::take(request.body_mut());
        let Ok(body) = axum::body::to_bytes(body, 16 * 1024 * 1024).await else {
            return StatusCode::BAD_REQUEST.into_response();
        };
        let digest = worker_runtime::auth::request_body_digest(&body);
        *request.body_mut() = axum::body::Body::from(body);
        let permission = if path.starts_with("/api/runtime/v1/workspaces/")
            || path.contains("/profile-source-archives/")
        {
            worker_runtime::auth::BACKEND_RESOURCE_FETCH_PERMISSION
        } else {
            worker_runtime::auth::WORKSPACE_REQUEST_PERMISSION
        };
        let Ok(source) = crate::worker_source::verify_runtime_request_source_proof(
            &api,
            &proof,
            &workspace_id,
            permission,
            &method,
            &path,
            &digest,
        )
        .await
        else {
            return StatusCode::UNAUTHORIZED.into_response();
        };
        request.extensions_mut().insert(source);
        return next.run(request).await;
    }

    let AuthConfig::Passkey { cookie_name, .. } = &api.config.auth;
    let actor = match crate::auth::resolve_request_actor(
        api.store.as_ref(),
        request.headers(),
        cookie_name,
    )
    .await
    {
        Ok(Some(actor)) => actor,
        Ok(None) if public_server_api => return next.run(request).await,
        Ok(None) | Err(_) => return StatusCode::UNAUTHORIZED.into_response(),
    };
    let cookie_authenticated = matches!(actor.auth_method, ActorAuthMethod::BrowserSession);
    let mutating = !matches!(
        *request.method(),
        Method::GET | Method::HEAD | Method::OPTIONS
    );
    if cookie_authenticated && mutating {
        let AuthConfig::Passkey { origin, .. } = &api.config.auth;
        if origin
            != request
                .headers()
                .get(ORIGIN)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
        {
            return StatusCode::FORBIDDEN.into_response();
        }
    }
    next.run(request).await
}

async fn enforce_server_cookie_mutation_origin(
    State(api): State<WorkspaceServerApi>,
    request: Request,
    next: axum::middleware::Next,
) -> Response {
    if matches!(
        *request.method(),
        Method::GET | Method::HEAD | Method::OPTIONS
    ) {
        return next.run(request).await;
    }
    let actor = match resolve_server_actor(&api, request.headers()).await {
        Ok(actor) => actor,
        Err(error) => return server_error_response(error),
    };
    if actor
        .as_ref()
        .is_some_and(|actor| matches!(actor.auth_method, ActorAuthMethod::BrowserSession))
    {
        let AuthConfig::Passkey { origin, .. } = &api.template.auth;
        let presented_origin = request
            .headers()
            .get(ORIGIN)
            .and_then(|value| value.to_str().ok());
        if presented_origin != Some(origin.as_str()) {
            return forbidden_server_response(
                "cookie-authenticated mutations require the configured Browser origin",
            );
        }
    }
    next.run(request).await
}

async fn dispatch_workspace_request(
    State(api): State<WorkspaceServerApi>,
    mut request: Request,
) -> Response {
    let path = request.uri().path().to_owned();
    let workspace_id = scoped_workspace_id(&path);
    if let Some(workspace_id) = workspace_id
        && (path.starts_with("/api/w/") || path.starts_with("/api/runtime/v1/workspaces/"))
        && let Err(response) =
            authorize_scoped_workspace_request(&api, workspace_id, &mut request).await
    {
        return response;
    }
    let router = if let Some(workspace_id) = workspace_id {
        match api.router_for_workspace(workspace_id).await {
            Ok(Some(router)) => Some(router),
            Ok(None) => None,
            Err(error) => return server_error_response(error),
        }
    } else {
        let workspaces = match api.store.list_workspaces() {
            Ok(workspaces) => workspaces,
            Err(error) => return server_error_response(error),
        };
        if workspaces.is_empty() && is_server_static_forward(&path) {
            return serve_server_static_shell(&api, &path).await;
        }
        if workspaces.len() == 1 || is_server_global_forward(&path) {
            match workspaces.first() {
                Some(workspace) => {
                    if path.starts_with("/api/")
                        && !is_server_global_forward(&path)
                        && let Err(response) = authorize_scoped_workspace_request(
                            &api,
                            &workspace.workspace_id,
                            &mut request,
                        )
                        .await
                    {
                        return response;
                    }
                    match api.router_for_workspace(&workspace.workspace_id).await {
                        Ok(router) => router,
                        Err(error) => return server_error_response(error),
                    }
                }
                None => None,
            }
        } else {
            None
        }
    };
    let Some(router) = router else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match router.oneshot(request).await {
        Ok(response) => response,
        Err(error) => match error {},
    }
}

fn is_server_global_forward(path: &str) -> bool {
    path == "/api/auth"
        || path.starts_with("/api/auth/")
        || path == "/health"
        || path == "/"
        || path.starts_with("/_app/")
        || path.starts_with("/assets/")
}

fn is_server_static_forward(path: &str) -> bool {
    !path.starts_with("/api/") && !path.starts_with("/internal/")
}

async fn serve_server_static_shell(api: &WorkspaceServerApi, path: &str) -> Response {
    let Some(static_root) = api.template.static_assets_dir.as_ref() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    static_file_or_spa_response(static_root, path).await
}

fn scoped_workspace_id(path: &str) -> Option<&str> {
    if let Some(rest) = path.strip_prefix("/api/runtime/v1/workspaces/") {
        return rest
            .split('/')
            .next()
            .filter(|workspace_id| !workspace_id.is_empty());
    }
    let mut segments = path.trim_start_matches('/').split('/');
    match (segments.next(), segments.next(), segments.next()) {
        (Some("api"), Some("w"), Some(workspace_id))
        | (Some("internal"), Some("w"), Some(workspace_id))
            if !workspace_id.is_empty() =>
        {
            Some(workspace_id)
        }
        (Some("w"), Some(workspace_id), _) if !workspace_id.is_empty() => Some(workspace_id),
        _ => None,
    }
}

pub async fn build_workspace_server_router(
    template: ServerConfig,
    store: Arc<dyn ControlPlaneStore>,
) -> Result<Router> {
    let auth = build_server_auth_router(ServerAuthApi {
        config: template.clone(),
        store: store.clone(),
    });
    let api = WorkspaceServerApi::new(template, store);
    api.preload().await?;
    let catalog = Router::new()
        .route(
            "/api/workspaces",
            get(list_server_workspaces).post(create_server_workspace),
        )
        .fallback(dispatch_workspace_request)
        .with_state(api.clone());
    Ok(auth
        .merge(catalog)
        .layer(axum::middleware::from_fn_with_state(
            api,
            enforce_server_cookie_mutation_origin,
        )))
}

impl WorkspaceApi {
    pub fn with_config_schema_provider(
        mut self,
        provider: Arc<dyn crate::config_source::WorkspaceConfigSchemaProvider>,
    ) -> Self {
        self.config_schema_registry = self.config_schema_registry.with_provider(provider);
        self
    }

    pub async fn new(config: ServerConfig, store: Arc<dyn ControlPlaneStore>) -> Result<Self> {
        let resource_broker = BackendResourceBroker::default();
        let embedded_identity = (*EMBEDDED_RUNTIME_REQUEST_IDENTITY).clone();
        store
            .upsert_trusted_runtime_record(&crate::store::TrustedRuntimeRecord {
                runtime_id: EMBEDDED_RUNTIME_ID.to_owned(),
                workspace_id: None,
                display_name: "Embedded Runtime".to_owned(),
                base_url: "in-process://embedded".to_owned(),
                public_key: embedded_identity.public_key.clone(),
                created_at: config.workspace_created_at.clone(),
                updated_at: config.workspace_created_at.clone(),
                revoked_at: None,
            })
            .await?;
        let embedded_audience = format!("embedded:{}", config.workspace_id);
        let worker_remove_dispatcher = Arc::new(
            crate::worker_source::EmbeddedServerWorkerMutationDispatcher::new(
                config.clone(),
                store.clone(),
            ),
        );
        let execution_backend = WorkerRuntimeExecutionBackend::new(
            ProfileRuntimeWorkerFactory::new(config.workspace_root.clone())
                .with_embedded_worker_mutation_dispatcher(
                    EMBEDDED_RUNTIME_ID,
                    worker_remove_dispatcher.clone(),
                )
                .with_runtime_request_identity(embedded_identity, embedded_audience)
                .with_runtime_store_dir(config.embedded_runtime_store_root.clone())
                .with_controller_transport(worker::WorkerControllerTransport::InProcess)
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
            Some(worker_remove_dispatcher),
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
            None,
        )
        .await
    }

    async fn new_with_execution_backend_and_broker(
        mut config: ServerConfig,
        store: Arc<dyn ControlPlaneStore>,
        execution_backend: Arc<dyn worker_runtime::execution::WorkerExecutionBackend>,
        resource_broker: BackendResourceBroker,
        worker_remove_dispatcher: Option<
            Arc<crate::worker_source::EmbeddedServerWorkerMutationDispatcher>,
        >,
    ) -> Result<Self> {
        store
            .upsert_workspace(&WorkspaceRecord {
                workspace_id: config.workspace_id.clone(),
                owner_account_id: None,
                display_name: config.workspace_display_name.clone(),
                state: "active".to_string(),
                created_at: config.workspace_created_at.clone(),
                updated_at: config.workspace_created_at.clone(),
            })
            .await?;
        import_configured_repositories(store.as_ref(), &config)?;
        config.repositories = load_configured_repositories_from_store(store.as_ref(), &config)?;
        let embedded_runtime = EmbeddedWorkerRuntime::new_fs_store_with_execution_backend(
            config.workspace_id.clone(),
            config.embedded_runtime_store_root.clone(),
            execution_backend,
        )
        .map(|runtime| runtime.with_resource_broker(resource_broker.clone()))
        .map_err(|err| crate::Error::Store(format!("invalid embedded Worker backend: {err}")))?;
        let embedded_subscription_runtime = embedded_runtime.subscription_runtime();
        let embedded_runtime_id = EMBEDDED_WORKER_RUNTIME_ID.to_string();
        let runtime = RuntimeRegistry::for_workspace(embedded_runtime);
        let runtime_subscription_broker =
            RuntimeSubscriptionBroker::new(config.workspace_id.clone());
        runtime_subscription_broker
            .register_embedded_runtime(embedded_runtime_id, embedded_subscription_runtime);
        for remote_config in config.remote_runtime_sources.iter().cloned() {
            let remote_runtime = RemoteWorkerRuntime::new(
                remote_config.clone(),
                config.workspace_id.clone(),
                config
                    .backend_base_url
                    .clone()
                    .unwrap_or_else(|| "http://127.0.0.1:8787".to_string()),
            )
            .map(|host| host.with_resource_broker(resource_broker.clone()))
            .map_err(|err| err.into_error())?;
            runtime.register(remote_runtime);
            runtime_subscription_broker.register_remote_runtime(remote_config);
        }
        let runtime = Arc::new(runtime);
        let companion = Arc::new(CompanionConsole::disabled());
        let observation_proxy = BackendObservationProxy::new(config.runtime_event_sources.clone());
        let config_store = Arc::new(crate::SqliteWorkspaceStore::open(
            config.database_path.clone(),
        )?);
        let config_schema_registry = crate::config_source::WorkspaceConfigSchemaRegistry::default()
            .with_provider(Arc::new(
                crate::profile_settings::ProfileConfigSchemaProvider,
            ))
            .with_provider(Arc::new(crate::prompt_settings::PromptConfigSchemaProvider))
            .with_provider(Arc::new(RuntimeConfigSchemaProvider))
            .with_provider(Arc::new(skills::SkillConfigSchemaProvider));
        config_store.ensure_workspace_config_materialized_with_schema(
            &config.workspace_id,
            &Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
            config_schema_registry.compose()?,
        )?;
        let api = Self {
            config_store,
            config_schema_registry,
            prompt_projection_cache:
                crate::prompt_settings::WorkspacePromptProjectionCache::default(),
            authority: SqliteWorkspaceAuthority::new(
                config.database_path.clone(),
                config.workspace_id.clone(),
            )?
            .with_merge_revision_source(Arc::new(MergeRequestRepositorySource {
                workspace_id: config.workspace_id.clone(),
                reader: RepositoryRegistryReader::new(config.repositories.clone()),
            })),
            config,
            store,
            runtime,
            companion,
            orchestrator_spawn_lock: Arc::new(std::sync::Mutex::new(())),
            orchestrator_attention_fingerprint: Arc::new(Mutex::new(None)),
            observation_proxy,
            runtime_subscription_broker,
            resource_broker,
            workdir_sessions: Arc::new(Mutex::new(HashMap::new())),
            workdir_session_locks: Arc::new(Mutex::new(HashMap::new())),
            worker_remove_locks: Arc::new(Mutex::new(HashMap::new())),
            worker_control_locks: Arc::new(Mutex::new(HashMap::new())),
        };
        if let Some(dispatcher) = worker_remove_dispatcher {
            dispatcher
                .install_executor(Arc::new(WorkspaceWorkerRemoveExecutor::new(&api)))
                .map_err(|message| Error::Config(message.to_string()))?;
        }
        Ok(api)
    }

    pub fn workspace_id(&self) -> &str {
        self.config.workspace_id.as_str()
    }

    pub fn runtime_subscription_broker(&self) -> &RuntimeSubscriptionBroker {
        &self.runtime_subscription_broker
    }

    fn workspace_api_ref(&self, _runtime_id: &str) -> WorkspaceApiRef {
        WorkspaceApiRef {
            workspace_id: self.config.workspace_id.clone(),
            base_url: self
                .config
                .backend_base_url
                .clone()
                .unwrap_or_else(|| "http://127.0.0.1:8787".to_string())
                .trim_end_matches('/')
                .to_string(),
        }
    }

    fn spawn_workspace_worker(
        &self,
        runtime_id: &str,
        mut request: WorkerSpawnRequest,
    ) -> ApiResult<WorkerSpawnResult> {
        self.validate_worker_spawn_repository_scope(&request)?;
        let workspace_api = self.workspace_api_ref(runtime_id);
        request.resolved_workspace_api = Some(workspace_api.clone());
        let attachment_reservation =
            request
                .resolved_working_directory
                .as_ref()
                .map(|working_directory| {
                    (
                        working_directory.working_directory_id.clone(),
                        Uuid::new_v4().to_string(),
                    )
                });
        if let Some((workdir_id, reservation_id)) = attachment_reservation.as_ref() {
            self.store.reserve_worker_workdir_attachment(
                &self.config.workspace_id,
                workdir_id,
                reservation_id,
                &now_registry_timestamp(),
            )?;
        }
        let request_fingerprint = worker_spawn_create_fingerprint(&request)
            .map_err(|message| Error::Config(message.to_string()))?;
        let current_memory_settings = self
            .config_store
            .get_workspace_memory_settings(&self.config.workspace_id)?;
        let allocation_key = request
            .resolved_control_operation
            .as_ref()
            .map(|operation| operation.operation_id.clone())
            .or_else(|| {
                request
                    .ticket_assignment
                    .as_ref()
                    .map(|assignment| assignment.operation_id.clone())
            })
            .unwrap_or_else(|| format!("manual:{}", WorkerId::now_v7()));
        let reservation = self
            .config_store
            .reserve_worker_create(
                &self.config.workspace_id,
                runtime_id,
                &allocation_key,
                &request_fingerprint,
                &current_memory_settings,
            )
            .map_err(|error| Error::RuntimeOperationFailed {
                runtime_id: runtime_id.to_string(),
                code: "workspace_worker_allocation_conflict".to_string(),
                message: error.to_string(),
            })?;
        let worker_id = reservation.worker_id;
        request.resolved_memory_settings = Some(reservation.memory_settings);
        let create_binding = WorkerCreateBinding {
            worker_id,
            create_fingerprint: reservation.create_fingerprint,
        };
        let result = match self
            .runtime
            .spawn_worker(runtime_id, create_binding, request)
        {
            Ok(result) => result,
            Err(error) => {
                if let Some((workdir_id, reservation_id)) = attachment_reservation.as_ref() {
                    let _ = self.store.release_worker_workdir_attachment_reservation(
                        &self.config.workspace_id,
                        workdir_id,
                        reservation_id,
                    );
                }
                return Err(error.into_error().into());
            }
        };
        let Some(worker) = result.worker.as_ref() else {
            if let Some((workdir_id, reservation_id)) = attachment_reservation.as_ref() {
                self.store.release_worker_workdir_attachment_reservation(
                    &self.config.workspace_id,
                    workdir_id,
                    reservation_id,
                )?;
            }
            return Ok(result);
        };
        let worker_ref = worker.worker.clone();
        if worker_ref.worker_id != worker_id.to_string() {
            if let Some((workdir_id, reservation_id)) = attachment_reservation.as_ref() {
                let _ = self.store.release_worker_workdir_attachment_reservation(
                    &self.config.workspace_id,
                    workdir_id,
                    reservation_id,
                );
            }
            return Err(Error::RuntimeOperationFailed {
                runtime_id: runtime_id.to_string(),
                code: "workspace_worker_identity_mismatch".to_string(),
                message: format!(
                    "Runtime returned Worker {} for reserved Workspace Worker {}",
                    worker_ref.worker_id, worker_id
                ),
            }
            .into());
        }
        let replacement = match self
            .runtime
            .replace_worker_workspace_api(&worker_ref, workspace_api)
        {
            Ok(replacement) => replacement,
            Err(error) => {
                let _ = self.runtime.delete_worker(&worker_ref);
                if let Some((workdir_id, reservation_id)) = attachment_reservation.as_ref() {
                    let _ = self.store.release_worker_workdir_attachment_reservation(
                        &self.config.workspace_id,
                        workdir_id,
                        reservation_id,
                    );
                }
                return Err(error.into_error().into());
            }
        };
        if replacement.state != WorkerOperationState::Accepted {
            let _ = self.runtime.delete_worker(&worker_ref);
            if let Some((workdir_id, reservation_id)) = attachment_reservation.as_ref() {
                let _ = self.store.release_worker_workdir_attachment_reservation(
                    &self.config.workspace_id,
                    workdir_id,
                    reservation_id,
                );
            }
            return Err(Error::RuntimeOperationFailed {
                runtime_id: runtime_id.to_string(),
                code: "worker_workspace_api_replace_failed".to_string(),
                message: replacement
                    .diagnostics
                    .first()
                    .map(|diagnostic| diagnostic.message.clone())
                    .unwrap_or_else(|| {
                        "Runtime rejected Workspace API replacement after spawn".to_string()
                    }),
            }
            .into());
        }
        if let Some((workdir_id, reservation_id)) = attachment_reservation.as_ref() {
            if let Err(error) = parse_runtime_worker_id_for_registry(&worker.worker.worker_id) {
                let _ = self.runtime.delete_worker(&worker_ref);
                let _ = self.store.release_worker_workdir_attachment_reservation(
                    &self.config.workspace_id,
                    workdir_id,
                    reservation_id,
                );
                return Err(error);
            }
            let compensation_context = WorkerSpawnCompensationContext {
                assignment: None,
                prepared_workdir_id: Some(workdir_id.as_str()),
                cleanup_spawned_workdir: false,
            };
            let registry_result = record_worker_summary(
                self,
                worker,
                worker.label.as_str(),
                worker.profile.clone(),
                WorkerRegistryDisplayNamePolicy::PreserveExisting,
            )
            .map(|_| ());
            if let Err(mut error) = finalize_worker_spawn_stage(
                self,
                worker,
                &compensation_context,
                WorkerSpawnFinalizeStage::WorkerRegistry,
                registry_result,
            ) {
                append_attachment_reservation_release_diagnostic(
                    self,
                    workdir_id,
                    reservation_id,
                    &mut error,
                );
                return Err(error);
            }

            let attachment = WorkerWorkdirLinkRecord {
                workspace_id: self.config.workspace_id.clone(),
                worker: worker_ref.clone(),
                workdir_id: workdir_id.clone(),
                role: "attachment".to_string(),
                linked_at: now_registry_timestamp(),
                unlinked_at: None,
            };
            let attachment_result = self
                .store
                .finalize_reserved_worker_workdir_attachment(&attachment, reservation_id)
                .map_err(ApiError::from);
            if let Err(mut error) = finalize_worker_spawn_stage(
                self,
                worker,
                &compensation_context,
                WorkerSpawnFinalizeStage::WorkdirAttachment,
                attachment_result,
            ) {
                append_attachment_reservation_release_diagnostic(
                    self,
                    workdir_id,
                    reservation_id,
                    &mut error,
                );
                return Err(error);
            }
        }
        self.config_store
            .complete_worker_create_reservation(&self.config.workspace_id, worker_id)
            .map_err(|error| Error::Config(error.to_string()))?;
        Ok(result)
    }

    fn restore_workspace_worker(
        &self,
        worker: &RuntimeWorkerRef,
    ) -> ApiResult<WorkerRestoreResult> {
        let binding = self
            .runtime
            .replace_worker_workspace_api(worker, self.workspace_api_ref(&worker.runtime_id))
            .map_err(|error| error.into_error())?;
        if binding.state != WorkerOperationState::Accepted {
            return Ok(WorkerRestoreResult {
                state: binding.state,
                worker: binding.worker,
                diagnostics: binding.diagnostics,
            });
        }
        Ok(self
            .runtime
            .restore_worker(worker)
            .map_err(|error| error.into_error())?)
    }

    fn repository_reader(&self) -> RepositoryRegistryReader {
        RepositoryRegistryReader::new(self.config.repositories.clone())
    }

    fn require_workspace_repository(&self, repository_id: &str) -> ApiResult<RepositoryRecord> {
        self.store
            .get_repository(&self.config.workspace_id, repository_id)?
            .ok_or_else(|| ApiError::from(Error::UnknownRepository(repository_id.to_string())))
    }

    fn require_configured_workspace_repository(
        &self,
        repository_id: &str,
    ) -> ApiResult<ConfiguredRepository> {
        self.require_workspace_repository(repository_id)?;
        self.config
            .repositories
            .iter()
            .find(|repository| repository.id == repository_id)
            .cloned()
            .ok_or_else(|| ApiError::from(Error::UnknownRepository(repository_id.to_string())))
    }

    fn validate_worker_spawn_repository_scope(
        &self,
        request: &WorkerSpawnRequest,
    ) -> ApiResult<()> {
        let (selected_repository_id, selected_ref_selector) =
            if let Some(working_directory) = request.resolved_working_directory_request.as_ref() {
                let repository_id = working_directory.repository.id.as_str();
                self.require_workspace_repository(repository_id)?;
                (
                    Some(repository_id.to_string()),
                    working_directory
                        .repository
                        .selector
                        .as_deref()
                        .map(str::to_owned),
                )
            } else if let Some(claim) = request.resolved_working_directory.as_ref() {
                let workdir = self
                    .store
                    .get_workdir_registry(&self.config.workspace_id, &claim.working_directory_id)?
                    .ok_or_else(|| {
                        ApiError::from(Error::Config(format!(
                            "unknown working directory `{}` in this Workspace",
                            claim.working_directory_id
                        )))
                    })?;
                self.require_workspace_repository(&workdir.repository_id)?;
                (Some(workdir.repository_id), workdir.creation_selector)
            } else {
                (None, None)
            };

        if let WorkerSpawnIntent::TicketRole { ticket_id, .. } = &request.intent {
            let ticket = self.authority.ticket(ticket_id)?;
            // Workdir-less Ticket Workers cannot execute repository implementation.
            // Preserve that control-plane launch while still validating any persisted
            // target (including its Workspace ownership) when one exists.
            if selected_repository_id.is_none() && ticket.repository_id.is_none() {
                return Ok(());
            }
            let repository_id = ticket.repository_id.as_deref().ok_or_else(|| {
                ApiError::from(Error::Config(
                    "Ticket implementation target must be validated and persisted before spawning a Ticket Worker".to_owned(),
                ))
            })?;
            let ref_selector = ticket.ref_selector.as_deref().ok_or_else(|| {
                ApiError::from(Error::Config(
                    "Ticket implementation target selector must be validated and persisted before spawning a Ticket Worker".to_owned(),
                ))
            })?;
            self.require_workspace_repository(repository_id)?;
            self.repository_reader()
                .observe_merge_target(repository_id, Some(ref_selector))
                .map_err(|error| {
                    ApiError::from(Error::Config(format!(
                        "Ticket implementation target is no longer resolvable: {error:?}"
                    )))
                })?;
            if selected_repository_id.as_deref() != Some(repository_id) {
                return Err(ApiError::from(Error::Config(format!(
                    "Ticket `{ticket_id}` targets repository `{repository_id}`, but the Worker launch resolves `{}`",
                    selected_repository_id.as_deref().unwrap_or("none")
                ))));
            }
            if selected_ref_selector.as_deref() != Some(ref_selector) {
                return Err(ApiError::from(Error::Config(format!(
                    "Ticket `{ticket_id}` targets selector `{ref_selector}`, but the Worker launch resolves `{}`",
                    selected_ref_selector.as_deref().unwrap_or("none")
                ))));
            }
        }
        Ok(())
    }
}

fn import_configured_repositories(
    store: &dyn ControlPlaneStore,
    config: &ServerConfig,
) -> Result<()> {
    if config.repositories.is_empty() {
        return Ok(());
    }
    let now = crate::auth::now_rfc3339();
    for repository in &config.repositories {
        store.upsert_repository(&RepositoryRecord {
            workspace_id: config.workspace_id.clone(),
            repository_id: repository.id.clone(),
            name: repository
                .display_name
                .clone()
                .unwrap_or_else(|| repository.id.clone()),
            kind: repository.provider.clone(),
            provider: Some(repository.provider.clone()),
            source: repository.source.clone(),
            default_ref: repository.default_selector.clone(),
            source_revision: repository.source_revision,
            source_fingerprint: repository.source_fingerprint.clone(),
            observed_status: repository.observed_status,
            observed_at: repository.observed_at.clone(),
            created_at: now.clone(),
            updated_at: now.clone(),
        })?;
    }
    Ok(())
}

fn load_configured_repositories_from_store(
    store: &dyn ControlPlaneStore,
    config: &ServerConfig,
) -> Result<Vec<ConfiguredRepository>> {
    store
        .list_repositories(&config.workspace_id)?
        .into_iter()
        .map(|record| configured_repository_from_record(&config.workspace_root, record))
        .collect()
}

fn configured_repository_from_record(
    _workspace_root: &Path,
    record: RepositoryRecord,
) -> Result<ConfiguredRepository> {
    let provider = record.provider.unwrap_or_else(|| record.kind.clone());
    let path = repository_local_path(&record.source);
    Ok(ConfiguredRepository {
        id: record.repository_id,
        provider,
        path,
        source: record.source,
        source_revision: record.source_revision,
        source_fingerprint: record.source_fingerprint,
        observed_status: record.observed_status,
        observed_at: record.observed_at,
        display_name: Some(record.name),
        default_selector: record.default_ref,
    })
}

fn build_server_auth_router(api: ServerAuthApi) -> Router {
    Router::new()
        .route("/api/auth/config", get(get_auth_config))
        .route("/api/auth/bootstrap-user", post(post_auth_bootstrap_user))
        .route(
            "/api/auth/passkeys/registration/options",
            post(post_passkey_registration_options),
        )
        .route(
            "/api/auth/passkeys/registration/complete",
            post(post_passkey_registration_complete),
        )
        .route(
            "/api/auth/passkeys/login/options",
            post(post_passkey_login_options),
        )
        .route(
            "/api/auth/passkeys/login/complete",
            post(post_passkey_login_complete),
        )
        .route("/api/auth/logout", post(post_auth_logout))
        .route(
            "/api/auth/device-login/start",
            post(post_device_login_start),
        )
        .route(
            "/api/auth/device-login/approve",
            post(post_device_login_approve),
        )
        .route("/api/auth/device-login/poll", post(post_device_login_poll))
        .route("/api/auth/whoami", get(get_auth_whoami))
        .with_state(api)
}

fn build_inner_router(api: WorkspaceApi) -> Router {
    let auth = build_server_auth_router(ServerAuthApi::from(&api));
    let scoped_ticket_relations_query_path =
        format!("/api/w/{{workspace_id}}{TICKET_RELATIONS_QUERY_PATH}");
    let scoped_ticket_orchestration_plans_query_path =
        format!("/api/w/{{workspace_id}}{TICKET_ORCHESTRATION_PLANS_QUERY_PATH}");
    let workspace = Router::new()
        .route("/api/workspace", get(get_workspace))
        .route("/api/w/{workspace_id}/workspace", get(scoped_get_workspace))
        .route(
            "/api/w/{workspace_id}/settings/workspace",
            get(scoped_get_workspace_settings).put(scoped_update_workspace_settings),
        )
        .route(
            "/api/w/{workspace_id}/settings/memory",
            get(scoped_get_workspace_memory_settings)
                .put(scoped_update_workspace_memory_settings),
        )
        .route(
            "/api/w/{workspace_id}/config/source-tree",
            get(scoped_get_workspace_config_tree),
        )
        .route(
            "/api/w/{workspace_id}/config/projections/prompts",
            get(scoped_get_prompt_projection),
        )
        .route(
            "/api/w/{workspace_id}/config/source-tree/commit",
            post(scoped_commit_workspace_config_tree),
        )
        .route(
            "/api/w/{workspace_id}/config/source-tree/revisions/{revision}",
            get(scoped_get_workspace_config_revision),
        )
        .route(
            "/api/w/{workspace_id}/config/source-tree/entries/{*path}",
            get(scoped_get_workspace_config_entry),
        )
        .route(
            "/api/w/{workspace_id}/settings/profiles",
            get(scoped_get_profile_settings),
        )
        .route(
            "/api/w/{workspace_id}/flows",
            get(scoped_list_flows).put(scoped_put_flow),
        )
        .route(
            "/api/w/{workspace_id}/flows/resolve",
            post(scoped_resolve_flow_source),
        )
        .route(
            "/api/w/{workspace_id}/flows/{flow_id}",
            get(scoped_get_flow),
        )
        .route("/api/tickets", get(list_tickets))
        .route(
            "/api/w/{workspace_id}/tickets",
            get(scoped_list_tickets).post(scoped_create_ticket_record),
        )
        .route(
            "/api/w/{workspace_id}/tickets/query",
            post(scoped_query_tickets),
        )
        .route(
            "/api/w/{workspace_id}/memory",
            get(scoped_get_memory_document),
        )
        .route(
            "/api/w/{workspace_id}/memory/staging",
            get(scoped_list_memory_staging),
        )
        .route(
            "/api/w/{workspace_id}/memory/backend",
            post(scoped_memory_backend_operation),
        )
        .route(
            "/api/w/{workspace_id}/memory/consolidation",
            post(scoped_memory_consolidation),
        )
        .route("/api/tickets/{id}", get(get_ticket))
        .route("/api/w/{workspace_id}/skills", get(scoped_list_skills))
        .route("/api/w/{workspace_id}/skills/lint", get(scoped_lint_skills))
        .route("/api/w/{workspace_id}/skills/{name}", get(scoped_get_skill))
        .route(
            "/api/w/{workspace_id}/skills/{name}/activate",
            get(scoped_activate_skill),
        )
        .route(
            "/api/w/{workspace_id}/tickets/default-intake-ready-body",
            post(scoped_default_intake_ready_body),
        )
        .route(
            "/api/w/{workspace_id}/tickets/search",
            get(scoped_list_ticket_summaries),
        )
        .route(
            "/api/w/{workspace_id}/tickets/doctor",
            get(scoped_ticket_doctor),
        )
        .route(
            scoped_ticket_relations_query_path.as_str(),
            post(scoped_query_ticket_relations),
        )
        .route(
            scoped_ticket_orchestration_plans_query_path.as_str(),
            post(scoped_query_ticket_orchestration_plans),
        )
        .route(
            "/api/w/{workspace_id}/tickets/{id}/record",
            get(scoped_get_ticket_record),
        )
        .route(
            "/api/w/{workspace_id}/tickets/{id}/item",
            patch(scoped_edit_ticket_record_item),
        )
        .route(
            "/api/w/{workspace_id}/tickets/{id}/dependency-check",
            get(scoped_ticket_dependency_check),
        )
        .route(
            "/api/w/{workspace_id}/tickets/{id}/thread-events",
            post(scoped_add_ticket_thread_event),
        )
        .route(
            "/api/w/{workspace_id}/tickets/{id}/state-changes",
            post(scoped_add_ticket_state_change),
        )
        .route(
            "/api/w/{workspace_id}/tickets/{id}/intake-summaries",
            post(scoped_add_ticket_intake_summary),
        )
        .route(
            "/api/w/{workspace_id}/tickets/{id}/state-fields/{field}",
            post(scoped_set_ticket_state_field),
        )
        .route(
            "/api/w/{workspace_id}/tickets/{id}/workflow-state",
            post(scoped_set_ticket_workflow_state),
        )
        .route(
            "/api/w/{workspace_id}/tickets/{id}/workflow/mark-ready",
            post(scoped_mark_ticket_ready),
        )
        .route(
            "/api/w/{workspace_id}/tickets/{id}/workflow/queue",
            post(scoped_queue_ticket_record),
        )
        .route(
            "/api/w/{workspace_id}/merge-requests",
            get(scoped_list_merge_requests),
        )
        .route(
            "/api/w/{workspace_id}/merge-requests/{merge_request_id}",
            get(scoped_show_merge_request),
        )
        .route(
            "/api/w/{workspace_id}/tickets/{id}/merge-request",
            post(scoped_open_merge_request),
        )
        .route(
            "/api/w/{workspace_id}/tickets/{id}/merge-request/readiness",
            get(scoped_merge_request_readiness),
        )
        .route(
            "/api/w/{workspace_id}/tickets/{id}/merge-request/thread",
            get(scoped_merge_request_thread),
        )
        .route(
            "/api/w/{workspace_id}/tickets/{id}/merge-request/repair-source",
            post(scoped_repair_merge_request_selector),
        )
        .route(
            "/api/w/{workspace_id}/internal/reviewer-child-sessions",
            post(scoped_register_reviewer_child_session),
        )
        .route(
            "/api/w/{workspace_id}/tickets/{id}/merge-request/review-capabilities",
            post(scoped_register_merge_request_review_capability),
        )
        .route(
            "/api/w/{workspace_id}/tickets/{id}/merge-request/reviews",
            post(scoped_submit_merge_request_review),
        )
        .route(
            "/api/w/{workspace_id}/tickets/{id}/merge-request/reviews/revoke",
            post(scoped_revoke_merge_request_review),
        )
        .route(
            "/api/w/{workspace_id}/tickets/{id}/merge-request/complete",
            post(scoped_complete_merge_request),
        )

        .route(
            "/api/w/{workspace_id}/tickets/{id}/workflow/close",
            post(scoped_close_ticket_record),
        )
        .route(
            "/api/w/{workspace_id}/tickets/{id}/relation-view",
            get(scoped_ticket_relation_view),
        )
        .route(
            "/api/w/{workspace_id}/tickets/{id}/relations",
            post(scoped_record_ticket_relation).delete(scoped_remove_ticket_relation),
        )
        .route(
            "/api/w/{workspace_id}/tickets/{id}/orchestration-plans",
            post(scoped_record_ticket_orchestration_plan),
        )
        .route(
            "/api/w/{workspace_id}/tickets/{id}",
            get(scoped_get_ticket).patch(scoped_edit_ticket_item),
        )
        .route(
            "/api/w/{workspace_id}/tickets/{id}/show",
            post(scoped_show_ticket),
        )
        .route(
            "/api/w/{workspace_id}/tickets/{id}/assignments",
            get(scoped_list_ticket_assignments),
        )
        .route(
            "/api/w/{workspace_id}/tickets/{id}/assignments/{role}",
            put(scoped_set_ticket_assignment).delete(scoped_clear_ticket_assignment),
        )
        .route(
            "/api/w/{workspace_id}/tickets/{id}/implementation-cancellations",
            post(scoped_cancel_ticket_implementation),
        )
        .route(
            "/api/w/{workspace_id}/tickets/{id}/state",
            post(scoped_transition_ticket_state),
        )
        .route(
            "/api/w/{workspace_id}/tickets/{id}/ready",
            post(scoped_mark_ticket_ready_from_browser),
        )
        .route(
            "/api/w/{workspace_id}/tickets/{id}/events",
            post(scoped_append_ticket_event),
        )
        .route(
            "/api/w/{workspace_id}/tickets/{id}/queue",
            post(scoped_queue_ticket),
        )
        .route(
            "/api/w/{workspace_id}/tickets/{id}/close",
            post(scoped_close_ticket),
        )
        .route("/api/objectives", get(list_objectives))
        .route(
            "/api/w/{workspace_id}/objectives",
            get(scoped_list_objectives).post(scoped_create_objective),
        )
        .route(
            "/api/w/{workspace_id}/objectives/query",
            post(scoped_query_objectives),
        )
        .route("/api/objectives/{id}", get(get_objective))
        .route(
            "/api/w/{workspace_id}/objectives/{objective_id}",
            get(scoped_get_objective).patch(scoped_edit_objective),
        )
        .route(
            "/api/w/{workspace_id}/objectives/{objective_id}/show",
            post(scoped_show_objective),
        )
        .route(
            "/api/w/{workspace_id}/objectives/{objective_id}/state",
            post(scoped_set_objective_state),
        )
        .route(
            "/api/w/{workspace_id}/objectives/{objective_id}/ticket-links",
            post(scoped_link_objective_ticket),
        )
        .route(
            "/api/w/{workspace_id}/objectives/{objective_id}/ticket-links/{ticket_id}",
            delete(scoped_unlink_objective_ticket),
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
            "/api/w/{workspace_id}/workers/self/workdir-attachment",
            post(scoped_attach_current_worker_workdir)
                .delete(scoped_detach_current_worker_workdir),
        )
        .route(
            "/api/w/{workspace_id}/workers/self/workdir-session/fence",
            get(scoped_current_worker_workdir_session_fence),
        )
        .route(
            "/api/w/{workspace_id}/workers/self/workdir-session/operations",
            post(scoped_execute_current_worker_workdir_operation),
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
            "/api/w/{workspace_id}/orchestrator",
            get(scoped_workspace_orchestrator_status)
                .post(scoped_start_workspace_orchestrator),
        )
        .route(
            "/api/w/{workspace_id}/worker-control/workers",
            get(list_known_workers).post(spawn_known_worker),
        )
        .route(
            "/api/w/{workspace_id}/worker-control/workers/{runtime_id}/{worker_id}/input",
            post(send_known_worker_input),
        )
        .route(
            "/api/w/{workspace_id}/worker-control/workers/{runtime_id}/{worker_id}/cancel",
            post(cancel_known_worker),
        )
        .route(
            "/api/w/{workspace_id}/worker-control/workers/{runtime_id}/{worker_id}/stop",
            post(stop_known_worker),
        )
        .route(
            "/api/w/{workspace_id}/worker-control/workers/{runtime_id}/{worker_id}/restore",
            post(restore_known_worker),
        )
        .route(
            "/api/w/{workspace_id}/worker-observation/sessions",
            get(scoped_list_worker_observation_sessions),
        )
        .route(
            "/api/w/{workspace_id}/worker-observation/session",
            post(scoped_capture_worker_observation_session),
        )
        .route(
            "/api/w/{workspace_id}/workers",
            get(scoped_list_workers).post(scoped_create_workspace_worker),
        )
        .route(
            "/api/w/{workspace_id}/workers/{worker_ref}",
            get(scoped_get_workspace_worker),
        )
        .route(
            "/api/w/{workspace_id}/protocol/ws",
            get(scoped_workspace_protocol_ws),
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
            "/api/runtime/v1/workspaces/{workspace_id}/resources/fetch",
            post(scoped_post_internal_runtime_resource_fetch),
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
            "/api/w/{workspace_id}/workers/remove",
            post(scoped_worker_remove_source_boundary),
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
            "/api/runtimes/{runtime_id}/workers/{worker_id}/restore",
            post(restore_runtime_worker),
        )
        .route(
            "/api/w/{workspace_id}/runtimes/{runtime_id}/workers/{worker_id}",
            get(scoped_get_runtime_worker),
        )
        .route(
            "/api/w/{workspace_id}/runtimes/{runtime_id}/workers/{worker_id}/restore",
            post(scoped_restore_runtime_worker),
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
        .with_state(api);
    auth.merge(workspace)
        .layer(middleware::from_fn(log_failed_api_response))
}

async fn log_failed_api_response(request: Request, next: Next) -> Response {
    let method = request.method().clone();
    let uri = request.uri().clone();
    let response = next.run(request).await;
    let status = response.status();

    if uri.path().starts_with("/api/") && (status.is_client_error() || status.is_server_error()) {
        let error = response.extensions().get::<ApiErrorLog>();
        eprintln!(
            "{} yoi-server {}",
            Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
            failed_api_log_json(&method, &uri, status, error)
        );
    }

    response
}

fn failed_api_log_json(
    method: &Method,
    uri: &Uri,
    status: StatusCode,
    error: Option<&ApiErrorLog>,
) -> String {
    let event = ApiFailureLogEvent {
        event: "api_error",
        method: method.as_str(),
        path: uri.path(),
        status: status.as_u16(),
        kind: error.map(|error| error.kind.as_str()),
        message: error.map(|error| error.message.as_str()),
        diagnostics: error.map(|error| error.diagnostics.as_slice()),
    };
    serde_json::to_string(&event).unwrap_or_else(|serialization_error| {
        format!(
            "{{\"event\":\"api_error\",\"status\":{},\"log_serialization_error\":{:?}}}",
            status.as_u16(),
            serialization_error.to_string()
        )
    })
}

#[derive(Serialize)]
struct ApiFailureLogEvent<'a> {
    event: &'static str,
    method: &'a str,
    path: &'a str,
    status: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    kind: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    diagnostics: Option<&'a [RuntimeDiagnostic]>,
}

pub async fn serve_workspace_catalog(
    template: ServerConfig,
    store: Arc<dyn ControlPlaneStore>,
    listener: TcpListener,
) -> Result<()> {
    let router = build_workspace_server_router(template, store).await?;
    axum::serve(listener, router).await?;
    Ok(())
}

pub fn build_router(api: WorkspaceApi) -> Router {
    build_inner_router(api.clone()).layer(axum::middleware::from_fn_with_state(
        api,
        authorize_workspace_api_request,
    ))
}

pub async fn serve(
    config: ServerConfig,
    store: Arc<dyn ControlPlaneStore>,
    listener: TcpListener,
) -> Result<()> {
    let api = WorkspaceApi::new(config, store).await?;
    let orchestrator_hook = tokio::spawn(run_orchestrator_turn_end_hook(api.clone()));
    let result = axum::serve(listener, build_router(api)).await;
    orchestrator_hook.abort();
    result?;
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

#[derive(Debug, Default, Deserialize)]
struct RuntimeWorkersQuery {
    status: Option<RuntimeWorkersStatusFilter>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RuntimeWorkersStatusFilter {
    Stopped,
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
    #[serde(flatten)]
    pub worker_ref: RuntimeWorkerRef,
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
    pub default_profile: Option<String>,
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

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrowserWorkingDirectoryCreateRequest {
    #[serde(default)]
    pub runtime_id: Option<String>,
    pub repository_id: String,
    #[serde(default)]
    pub selector: Option<String>,
    #[serde(default)]
    pub operation_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrowserWorkerWorkingDirectorySelection {
    pub working_directory_id: String,
    #[serde(default)]
    pub relative_cwd: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BrowserWorkspaceOrchestratorResponse {
    pub workspace_id: String,
    pub online: bool,
    pub disposition: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worker: Option<WorkerSummary>,
    pub diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateWorkspaceWorkerTicketAssignmentRequest {
    pub ticket_id: String,
    pub operation_id: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateWorkspaceWorkerRequest {
    pub runtime_id: String,
    pub display_name: String,
    #[serde(default)]
    pub profile: Option<String>,
    #[serde(default)]
    pub ticket_assignment: Option<CreateWorkspaceWorkerTicketAssignmentRequest>,
    #[serde(default)]
    pub initial_submit: Vec<Segment>,
    #[serde(default)]
    pub working_directory: Option<BrowserWorkerWorkingDirectorySelection>,
    /// Backend idempotency key used only for authenticated Worker-owned spawn/control.
    #[serde(default)]
    pub control_operation_id: Option<String>,
    /// Trusted resolution populated only by the authenticated worker-control handler.
    #[serde(skip, default)]
    pub resolved_control_operation: Option<WorkerControlOperation>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BrowserCreateWorkerResponse {
    pub workspace_id: String,
    #[serde(flatten)]
    pub worker_ref: RuntimeWorkerRef,
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

#[derive(Debug, Deserialize)]
struct LogQuery {
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct MemoryStagingQuery {
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct ObjectiveListQuery {
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct TicketListQuery {
    limit: Option<usize>,
    cursor: Option<String>,
    /// Comma-separated workflow states. Repeated lane requests normally pass one state group.
    states: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ScopedObjectivePath {
    workspace_id: String,
    objective_id: String,
}

#[derive(Debug, Deserialize)]
struct ScopedObjectiveTicketPath {
    workspace_id: String,
    objective_id: String,
    ticket_id: String,
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
struct ScopedFlowPath {
    workspace_id: String,
    flow_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PutFlowRequest {
    path: String,
    content: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AttachCurrentWorkerWorkdirRequest {
    workdir_id: String,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct CurrentWorkerWorkdirAttachmentResponse {
    workspace_id: String,
    workdir_id: String,
    attached: bool,
}

#[derive(Debug, Clone, Deserialize)]
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
struct ScopedWorkspaceWorkerReferencePath {
    workspace_id: String,
    worker_ref: String,
}

#[derive(Debug, Deserialize)]
struct ScopedRuntimeWorkerPath {
    workspace_id: String,
    #[serde(flatten)]
    worker: RuntimeWorkerRef,
}

fn validate_workspace_scope(api: &WorkspaceApi, workspace_id: &str) -> ApiResult<()> {
    if workspace_id == api.workspace_id() {
        Ok(())
    } else {
        Err(workspace_id_mismatch_error())
    }
}

async fn scoped_list_flows(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedWorkspacePath>,
) -> ApiResult<Json<Vec<FlowSourceRecord>>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    Ok(Json(api.store.list_flow_sources(&path.workspace_id)?))
}

async fn scoped_put_flow(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedWorkspacePath>,
    Json(request): Json<PutFlowRequest>,
) -> ApiResult<Json<FlowSourceRecord>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    let definition = flow::compile_flow_source(&request.content).map_err(|error| {
        Error::InvalidInput(format!(
            "invalid Flow source: {}",
            serde_json::to_string(&error.diagnostics)
                .unwrap_or_else(|_| "diagnostics unavailable".to_string())
        ))
    })?;
    let expected_path = format!("flows/{}.dcdl", definition.name);
    if request.path != expected_path {
        return Err(
            Error::InvalidInput(format!("Flow source path must be `{expected_path}`")).into(),
        );
    }
    let now = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
    Ok(Json(api.store.put_flow_source_for_kind(
        &path.workspace_id,
        FlowSourceKind::Workspace,
        &request.path,
        &request.content,
        &now,
    )?))
}

async fn scoped_resolve_flow_source(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedWorkspacePath>,
    Json(request): Json<FlowSourceResolveRequest>,
) -> ApiResult<Json<ResolvedFlowSource>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    let resolved = match &request.selector {
        flow::FlowSelector::Builtin { slug } => {
            let builtin = flow::builtin_flow_source(slug)
                .ok_or_else(|| Error::InvalidRecordId(request.selector.to_string()))?;
            let definition = builtin.compile().map_err(|error| {
                Error::Store(format!(
                    "compile built-in Flow {slug:?}: {:?}",
                    error.diagnostics
                ))
            })?;
            ResolvedFlowSource {
                selector: request.selector.clone(),
                workspace_id: path.workspace_id,
                flow_id: format!("builtin:{slug}"),
                revision: builtin.revision,
                content_digest: definition.content_digest.clone(),
                definition,
            }
        }
        flow::FlowSelector::Workspace { slug } => {
            let source = api
                .store
                .get_flow_source_by_name(&path.workspace_id, FlowSourceKind::Workspace, slug)?
                .ok_or_else(|| Error::InvalidRecordId(request.selector.to_string()))?;
            let revision = api
                .store
                .get_flow_source_revision(&path.workspace_id, &source.flow_id, source.revision)?
                .ok_or_else(|| {
                    Error::Store(format!(
                        "resolved Flow revision {}@{} is missing",
                        source.flow_id, source.revision
                    ))
                })?;
            ResolvedFlowSource {
                selector: request.selector.clone(),
                workspace_id: path.workspace_id,
                flow_id: source.flow_id,
                revision: source.revision,
                content_digest: revision.content_digest,
                definition: revision.definition,
            }
        }
    };
    Ok(Json(resolved))
}

async fn scoped_get_flow(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedFlowPath>,
) -> ApiResult<Json<FlowSourceRecord>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    let source = api
        .store
        .get_flow_source(&path.workspace_id, &path.flow_id)?
        .ok_or_else(|| Error::InvalidRecordId(path.flow_id))?;
    Ok(Json(source))
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

#[derive(Debug, Deserialize)]
struct WorkspaceConfigRevisionPath {
    workspace_id: String,
    revision: u64,
}

async fn scoped_get_workspace_config_revision(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<WorkspaceConfigRevisionPath>,
) -> ApiResult<Json<ConfigTreeSnapshot>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    let snapshot = api
        .config_store
        .load_workspace_config_revision(&path.workspace_id, path.revision)?
        .ok_or_else(|| ApiError::from(Error::InvalidRecordId(path.revision.to_string())))?;
    Ok(Json(snapshot))
}

#[derive(Debug, Deserialize)]
struct WorkspaceConfigEntryPath {
    workspace_id: String,
    path: String,
}

async fn scoped_get_prompt_projection(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedWorkspacePath>,
) -> ApiResult<Json<worker::WorkspacePromptProjection>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    let state = api
        .config_store
        .load_workspace_config(&path.workspace_id)?
        .ok_or_else(|| {
            ApiError::from(Error::InvalidInput(
                "Workspace config is not initialized".to_string(),
            ))
        })?;
    let projection = api
        .prompt_projection_cache
        .resolve(&path.workspace_id, &state)?;
    Ok(Json(projection.as_ref().clone()))
}

#[derive(Debug, Serialize)]
struct WorkspaceConfigTreeResponse {
    snapshot: ConfigTreeSnapshot,
    contract: config_source::ToolchainContract,
    projection_digest: String,
}

async fn scoped_get_workspace_memory_settings(
    State(api): State<WorkspaceApi>,
    AxumPath(workspace_id): AxumPath<String>,
) -> ApiResult<Json<workspace_api::WorkspaceMemorySettings>> {
    validate_workspace_scope(&api, &workspace_id)?;
    let settings = api
        .config_store
        .get_workspace_memory_settings(&workspace_id)
        .map_err(ApiError::from)?;
    Ok(Json(workspace_api::WorkspaceMemorySettings {
        workspace_id: settings.workspace_id,
        settings_revision: settings.settings_revision,
        language: settings.language,
    }))
}

async fn scoped_update_workspace_memory_settings(
    State(api): State<WorkspaceApi>,
    AxumPath(workspace_id): AxumPath<String>,
    Json(request): Json<workspace_api::UpdateWorkspaceMemorySettingsRequest>,
) -> ApiResult<Json<workspace_api::WorkspaceMemorySettings>> {
    validate_workspace_scope(&api, &workspace_id)?;
    let settings = api
        .config_store
        .update_workspace_memory_settings(
            &workspace_id,
            request.expected_revision,
            &request.language,
        )
        .map_err(ApiError::from)?;
    Ok(Json(workspace_api::WorkspaceMemorySettings {
        workspace_id: settings.workspace_id,
        settings_revision: settings.settings_revision,
        language: settings.language,
    }))
}

async fn scoped_get_workspace_config_tree(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedWorkspacePath>,
) -> ApiResult<Json<WorkspaceConfigTreeResponse>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    let state = api
        .config_store
        .load_workspace_config(&path.workspace_id)?
        .unwrap_or_else(|| crate::config_source::WorkspaceConfigState {
            snapshot: ConfigTreeSnapshot::empty(),
            contract: config_source::ToolchainContract::new(
                config_source::DEFAULT_SCHEMA_VERSION,
                Vec::new(),
                config_source::DEFAULT_IMPORT_POLICY_VERSION,
            ),
            projection_digest: config_source::digest_bytes(b"[]"),
        });
    Ok(Json(WorkspaceConfigTreeResponse {
        snapshot: state.snapshot,
        contract: state.contract,
        projection_digest: state.projection_digest,
    }))
}

async fn scoped_get_workspace_config_entry(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<WorkspaceConfigEntryPath>,
) -> ApiResult<Json<config_source::ConfigEntry>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    let virtual_path = config_source::VirtualPath::parse(&path.path)
        .map_err(|error| ApiError::from(Error::InvalidInput(error.to_string())))?;
    let state = api
        .config_store
        .load_workspace_config(&path.workspace_id)?
        .ok_or_else(|| {
            ApiError::from(Error::InvalidRecordId("virtual config source tree".into()))
        })?;
    let entry = state
        .snapshot
        .get(&virtual_path)
        .cloned()
        .ok_or_else(|| ApiError::from(Error::InvalidRecordId(path.path)))?;
    Ok(Json(entry))
}

async fn scoped_commit_workspace_config_tree(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedWorkspacePath>,
    Json(request): Json<ConfigCommitRequest>,
) -> ApiResult<(StatusCode, Json<WorkspaceConfigTreeResponse>)> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    let candidate = api
        .config_store
        .evaluate_workspace_config_candidate_with_schema(
            &path.workspace_id,
            &request,
            api.config_schema_registry.compose()?,
        )?;
    crate::prompt_settings::validate_evaluated_prompt_catalog(&candidate.evaluation)?;
    let state = api
        .config_store
        .commit_evaluated_workspace_config(&path.workspace_id, &candidate)?;
    if let Ok(projection) = api
        .prompt_projection_cache
        .resolve(&path.workspace_id, &state)
    {
        let _diagnostics = api
            .runtime
            .observe_workspace_prompt_projection((*projection).clone());
    }
    Ok((
        StatusCode::CREATED,
        Json(WorkspaceConfigTreeResponse {
            snapshot: state.snapshot,
            contract: state.contract,
            projection_digest: state.projection_digest,
        }),
    ))
}

async fn scoped_get_profile_settings(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedWorkspacePath>,
) -> ApiResult<Json<crate::profile_settings::ProfileSettingsResponse>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    let state = api
        .config_store
        .load_workspace_config(&path.workspace_id)?
        .ok_or_else(|| {
            ApiError::from(Error::InvalidRecordId("virtual config source tree".into()))
        })?;
    Ok(Json(
        crate::profile_settings::project_profiles_from_workspace_config(
            &path.workspace_id,
            &state,
        )?
        .settings,
    ))
}

async fn scoped_list_tickets(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedWorkspacePath>,
    Query(query): Query<TicketListQuery>,
) -> ApiResult<Json<crate::records::TicketListResponse>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    list_tickets(State(api), Query(query)).await
}

async fn scoped_get_ticket(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedRecordPath>,
) -> ApiResult<Json<TicketDetail>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    get_ticket(State(api), AxumPath(path.id)).await
}

async fn scoped_query_tickets(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedWorkspacePath>,
    Json(query): Json<TicketQueryRequest>,
) -> ApiResult<Json<TicketQueryResponse>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    Ok(Json(api.authority.query_tickets(query)?))
}

async fn scoped_show_ticket(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedRecordPath>,
    Json(query): Json<TicketShowRequest>,
) -> ApiResult<Json<TicketDetail>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    Ok(Json(api.authority.show_ticket(&path.id, query)?))
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct TicketRoleAssignmentsResponse {
    workspace_id: String,
    ticket_id: String,
    assignments: Vec<TicketRoleAssignmentRecord>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct TicketRoleAssignmentMutationResponse {
    workspace_id: String,
    ticket_id: String,
    assignment: Option<TicketRoleAssignmentRecord>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SetTicketRoleAssignmentRequest {
    operation_id: String,
    principal: TicketAssignmentPrincipal,
    expected_assignment_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CancelTicketImplementationRequest {
    operation_id: String,
    assignment_id: String,
    reason: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClearTicketRoleAssignmentQuery {
    operation_id: Option<String>,
    assignment_id: Option<String>,
}

fn parse_ticket_assignment_role(role: &str) -> ApiResult<TicketAssignmentRole> {
    match role {
        "orchestrator" => Ok(TicketAssignmentRole::Orchestrator),
        "coder" => Ok(TicketAssignmentRole::Coder),
        "owner" => Ok(TicketAssignmentRole::Owner),
        "contributor" => Ok(TicketAssignmentRole::Contributor),
        _ => Err(Error::InvalidInput(format!("unknown Ticket assignment role `{role}`")).into()),
    }
}

async fn scoped_list_ticket_assignments(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedRecordPath>,
) -> ApiResult<Json<TicketRoleAssignmentsResponse>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    let ticket = api.authority.ticket(&path.id)?;
    let assignments = api
        .store
        .list_current_ticket_role_assignments(&path.workspace_id, &ticket.id)?;
    Ok(Json(TicketRoleAssignmentsResponse {
        workspace_id: path.workspace_id,
        ticket_id: ticket.id,
        assignments,
    }))
}

async fn scoped_set_ticket_assignment(
    State(api): State<WorkspaceApi>,
    AxumPath((workspace_id, id, role)): AxumPath<(String, String, String)>,
    Json(request): Json<SetTicketRoleAssignmentRequest>,
) -> ApiResult<Json<TicketRoleAssignmentMutationResponse>> {
    validate_workspace_scope(&api, &workspace_id)?;
    let ticket = api.authority.ticket(&id)?;
    let role = parse_ticket_assignment_role(&role)?;
    let operation_id = require_ticket_assignment_value("operation_id", request.operation_id)?;
    let expected_assignment_id = request
        .expected_assignment_id
        .map(|value| require_ticket_assignment_value("expected_assignment_id", value))
        .transpose()?;
    if matches!(request.principal, TicketAssignmentPrincipal::User { .. }) {
        return Err(Error::TicketAssignmentConflict(
            "user-principal Ticket assignment requires an authenticated authoring boundary; weak Workspace Web access is not authority"
                .to_string(),
        )
        .into());
    }
    let assigned_at = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
    let record = TicketRoleAssignmentRecord {
        workspace_id: workspace_id.clone(),
        ticket_id: ticket.id.clone(),
        assignment_id: new_id("tasg"),
        role,
        principal: request.principal,
        assigned_by: "workspace-web".to_string(),
        assigned_at,
    };
    let assignment = match role {
        TicketAssignmentRole::Orchestrator => {
            if !matches!(
                ticket.state.as_str(),
                state if state == TicketWorkflowState::Planning.as_str()
                    || state == TicketWorkflowState::Ready.as_str()
            ) {
                return Err(Error::TicketAssignmentConflict(format!(
                    "Orchestrator assignment requires planning or ready Ticket; current state is {}",
                    ticket.state
                ))
                .into());
            }
            if api
                .store
                .get_current_ticket_role_assignment(
                    &workspace_id,
                    &ticket.id,
                    TicketAssignmentRole::Coder,
                )?
                .is_some()
            {
                return Err(Error::TicketAssignmentConflict(
                    "Orchestrator assignment conflicts with an active Coder assignment".to_string(),
                )
                .into());
            }
            api.store.set_current_ticket_role_assignment(
                &record,
                expected_assignment_id.as_deref(),
                &new_id("tasev"),
                &operation_id,
                expected_assignment_id.is_some(),
            )?
        }
        TicketAssignmentRole::Coder => {
            if expected_assignment_id.is_some() {
                return Err(Error::TicketAssignmentConflict(
                    "manual Coder start does not support reassign; clear through a guarded lifecycle operation first"
                        .to_string(),
                )
                .into());
            }
            if let TicketAssignmentPrincipal::Worker {
                runtime_id,
                worker_id,
            } = &record.principal
            {
                api.runtime
                    .worker(&RuntimeWorkerRef::new(
                        runtime_id.clone(),
                        worker_id.clone(),
                    ))
                    .map_err(|error| error.into_error())?;
            }
            api.store.start_ready_ticket_with_coder_assignment(
                &record,
                &new_id("tasev"),
                &operation_id,
            )?
        }
        TicketAssignmentRole::Owner | TicketAssignmentRole::Contributor => {
            return Err(Error::TicketAssignmentConflict(
                "Owner and Contributor mutation requires an authenticated authoring boundary"
                    .to_string(),
            )
            .into());
        }
    };
    Ok(Json(TicketRoleAssignmentMutationResponse {
        workspace_id,
        ticket_id: ticket.id,
        assignment: Some(assignment),
    }))
}

async fn scoped_clear_ticket_assignment(
    State(api): State<WorkspaceApi>,
    AxumPath((workspace_id, id, role)): AxumPath<(String, String, String)>,
    Query(query): Query<ClearTicketRoleAssignmentQuery>,
) -> ApiResult<Json<TicketRoleAssignmentMutationResponse>> {
    validate_workspace_scope(&api, &workspace_id)?;
    let ticket = api.authority.ticket(&id)?;
    let role = parse_ticket_assignment_role(&role)?;
    let operation_id = query
        .operation_id
        .map(|value| require_ticket_assignment_value("operation_id", value))
        .transpose()?
        .ok_or_else(|| {
            Error::TicketAssignmentConflict("unassign requires operation_id".to_string())
        })?;
    let assignment_id = query
        .assignment_id
        .map(|value| require_ticket_assignment_value("assignment_id", value))
        .transpose()?
        .ok_or_else(|| {
            Error::TicketAssignmentConflict("unassign requires assignment_id".to_string())
        })?;
    if matches!(
        ticket.state.as_str(),
        state if state == TicketWorkflowState::Queued.as_str()
            || state == TicketWorkflowState::InProgress.as_str()
    ) {
        return Err(Error::TicketAssignmentConflict(format!(
            "cannot unassign role `{}` while Ticket is {}; rescope through a guarded lifecycle operation",
            role.as_str(),
            ticket.state
        ))
        .into());
    }
    let cleared = api.store.clear_current_ticket_role_assignment(
        &workspace_id,
        &ticket.id,
        role,
        &assignment_id,
        &new_id("tasev"),
        &operation_id,
        "workspace-web",
        &Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
        Some("role assignment removed from Ticket detail"),
    )?;
    if !cleared {
        return Err(Error::TicketAssignmentConflict(format!(
            "assignment `{assignment_id}` is not current for role `{}`",
            role.as_str()
        ))
        .into());
    }
    Ok(Json(TicketRoleAssignmentMutationResponse {
        workspace_id,
        ticket_id: ticket.id,
        assignment: None,
    }))
}

async fn scoped_cancel_ticket_implementation(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedRecordPath>,
    Json(request): Json<CancelTicketImplementationRequest>,
) -> ApiResult<Json<TicketDetail>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    let ticket = api.authority.ticket(&path.id)?;
    let operation_id = require_ticket_assignment_value("operation_id", request.operation_id)?;
    let assignment_id = require_ticket_assignment_value("assignment_id", request.assignment_id)?;
    let reason = require_ticket_assignment_value("reason", request.reason)?;
    if reason.len() > 512 {
        return Err(Error::InvalidInput(
            "implementation cancellation reason must be at most 512 bytes".to_string(),
        )
        .into());
    }

    if !matches!(
        ticket.state.as_str(),
        state if state == TicketWorkflowState::InProgress.as_str()
            || state == TicketWorkflowState::Ready.as_str()
    ) {
        return Err(Error::TicketAssignmentConflict(format!(
            "implementation cancellation requires an inprogress Ticket; current state is {}",
            ticket.state
        ))
        .into());
    }

    let current = api.store.get_current_ticket_role_assignment(
        &path.workspace_id,
        &ticket.id,
        TicketAssignmentRole::Coder,
    )?;
    if let Some(assignment) = current.filter(|value| value.assignment_id == assignment_id)
        && let TicketAssignmentPrincipal::Worker {
            runtime_id,
            worker_id,
        } = assignment.principal
    {
        if api
            .store
            .get_ticket_assignment_operation(&path.workspace_id, &operation_id)?
            .is_some()
        {
            return Err(Error::TicketAssignmentConflict(format!(
                "operation `{operation_id}` was already used for another Ticket assignment mutation"
            ))
            .into());
        }
        let worker = RuntimeWorkerRef::new(runtime_id, worker_id);
        cancel_ticket_coder_worker(&api, &worker, &reason).await?;
    }

    let cancelled = api.store.cancel_current_ticket_coder_assignment(
        &path.workspace_id,
        &ticket.id,
        &assignment_id,
        &new_id("tasev"),
        &new_id("tev"),
        &operation_id,
        "workspace-web",
        &Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
        &reason,
    )?;
    if !cancelled {
        return Err(Error::TicketAssignmentConflict(format!(
            "assignment `{assignment_id}` is not the current Coder implementation"
        ))
        .into());
    }
    browser_ticket_detail(&api, &ticket.id)
}

async fn cancel_ticket_coder_worker(
    api: &WorkspaceApi,
    worker: &RuntimeWorkerRef,
    reason: &str,
) -> ApiResult<()> {
    let session_lock = current_worker_session_lock(api, worker);
    let _session_guard = session_lock.lock().await;
    match api.runtime.cancel_worker(
        worker,
        WorkerLifecycleRequest {
            reason: Some(format!("Ticket implementation cancelled: {reason}")),
            ticket_assignment: None,
        },
    ) {
        Ok(result) if result.state == WorkerOperationState::Accepted => {}
        Ok(result) => {
            return Err(ApiError::with_diagnostics(
                Error::RuntimeOperationFailed {
                    runtime_id: worker.runtime_id.clone(),
                    code: "workspace_ticket_implementation_cancel_rejected".to_string(),
                    message: "Runtime did not cancel the assigned Coder Worker".to_string(),
                },
                result.diagnostics,
            ));
        }
        Err(RuntimeRegistryError::UnknownWorker { .. }) => {}
        Err(error) => return Err(error.into_error().into()),
    }
    close_current_worker_session_locked(api, worker).await?;
    Ok(())
}

fn validate_ticket_assignment_state(
    api: &WorkspaceApi,
    assignment: &WorkerTicketAssignmentRequest,
) -> Result<()> {
    let ticket = api.authority.ticket(&assignment.ticket_id)?;
    if !matches!(
        ticket.state.as_str(),
        state if state == TicketWorkflowState::Queued.as_str()
            || state == TicketWorkflowState::InProgress.as_str()
    ) {
        return Err(Error::TicketAssignmentConflict(format!(
            "Ticket {} must be queued or inprogress before assigning an implementation Coder; current state is {}",
            ticket.id, ticket.state
        )));
    }
    let Some(orchestrator_assignment) =
        orchestrator_interested(api, &api.config.workspace_id, &ticket.id, &ticket.state)?
    else {
        return Err(Error::TicketAssignmentConflict(format!(
            "Ticket {} cannot be assigned an orchestration Coder without an active Orchestrator role assignment",
            ticket.id
        )));
    };
    let queued = browser_ticket_backend(api)?.show(TicketIdOrSlug::Id(ticket.id.clone()))?;
    let queued_assignment_id = queued
        .events
        .iter()
        .rev()
        .find_map(|event| event.attributes.get("orchestrator_assignment_id"));
    if queued_assignment_id.map(String::as_str)
        != Some(orchestrator_assignment.assignment_id.as_str())
    {
        return Err(Error::TicketAssignmentConflict(format!(
            "Ticket {} Queue fence does not match active Orchestrator assignment {}",
            ticket.id, orchestrator_assignment.assignment_id
        )));
    }
    Ok(())
}

fn validate_ticket_assignment_spawn(
    api: &WorkspaceApi,
    runtime_id: &str,
    request: &WorkerSpawnRequest,
) -> Result<()> {
    let Some(assignment) = request.ticket_assignment.as_ref() else {
        return Ok(());
    };

    match &request.intent {
        WorkerSpawnIntent::TicketRole {
            ticket_id,
            role: TicketWorkerRole::Coder,
        } if ticket_id == &assignment.ticket_id => {}
        WorkerSpawnIntent::TicketRole {
            ticket_id,
            role: TicketWorkerRole::Coder,
        } => {
            return Err(Error::TicketAssignmentConflict(format!(
                "spawn intent Ticket {ticket_id} does not match assignment Ticket {}",
                assignment.ticket_id
            )));
        }
        _ => {
            return Err(Error::TicketAssignmentConflict(
                "ticket_assignment is accepted only for a Ticket-role Coder spawn".to_string(),
            ));
        }
    }
    if !request
        .initial_submit
        .iter()
        .any(|segment| matches!(segment, Segment::Flow { .. }))
    {
        return Err(Error::TicketAssignmentConflict(
            "Ticket-assigned Coder spawn requires one Flow segment in initial_submit".to_string(),
        ));
    }
    validate_ticket_assignment_state(api, assignment)?;

    if let Some(current) = api
        .store
        .get_current_ticket_coder_assignment(&api.config.workspace_id, &assignment.ticket_id)?
    {
        let replay_matches = api
            .store
            .get_ticket_assignment_operation(&api.config.workspace_id, &assignment.operation_id)?
            .is_some_and(|operation| {
                operation.action == "assign"
                    && operation.ticket_id == assignment.ticket_id
                    && operation.runtime_id.as_deref() == Some(runtime_id)
                    && operation.assignment_id.as_deref() == Some(current.assignment_id.as_str())
                    && operation.worker.as_ref() == Some(&current.worker)
            });
        if !replay_matches {
            return Err(Error::TicketAssignmentConflict(format!(
                "Ticket {} is already assigned; use the explicit reassign operation",
                assignment.ticket_id
            )));
        }
    }
    Ok(())
}

fn optional_worker_mutation_source(
    api: &WorkspaceApi,
    workspace_id: &str,
    headers: &HeaderMap,
) -> Result<Option<WorkerMutationSource>> {
    let has_runtime = headers.contains_key("x-yoi-runtime-id");
    let has_worker = headers.contains_key("x-yoi-worker-id");
    if !has_runtime && !has_worker {
        return Ok(None);
    }
    authenticate_worker_mutation_source(api, workspace_id, headers).map(Some)
}

fn reject_orchestrator_generic_flow_spawn(
    api: &WorkspaceApi,
    source: Option<&WorkerMutationSource>,
    initial_submit: &[Segment],
    has_ticket_assignment: bool,
) -> Result<()> {
    let is_current_orchestrator = source.is_some_and(|source| {
        find_workspace_orchestrator(api).is_some_and(|orchestrator| orchestrator.worker == *source)
    });
    reject_orchestrator_generic_flow_spawn_for_source(
        is_current_orchestrator,
        initial_submit,
        has_ticket_assignment,
    )
}

fn reject_orchestrator_generic_flow_spawn_for_source(
    is_current_orchestrator: bool,
    initial_submit: &[Segment],
    has_ticket_assignment: bool,
) -> Result<()> {
    let has_flow = initial_submit
        .iter()
        .any(|segment| matches!(segment, Segment::Flow { .. }));
    if is_current_orchestrator && has_flow && !has_ticket_assignment {
        return Err(Error::TicketAssignmentConflict(
            "Workspace Orchestrator Flow spawn requires typed Ticket assignment; Ticket identity in initial text is not assignment authority"
                .to_string(),
        ));
    }
    Ok(())
}

fn assign_ticket_worker_from_lifecycle(
    api: &WorkspaceApi,
    assignment: &crate::hosts::WorkerTicketAssignmentRequest,
    runtime_id: &str,
    worker_id: &str,
) -> Result<TicketCoderAssignmentRecord> {
    let ticket = api.authority.ticket(&assignment.ticket_id)?;
    let worker = RuntimeWorkerRef::new(runtime_id, worker_id);
    if let Some(operation) = api
        .store
        .get_ticket_assignment_operation(&api.config.workspace_id, &assignment.operation_id)?
        && let Some(assignment_id) = operation.assignment_id.as_ref()
    {
        if operation.action == "assign"
            && operation.ticket_id == assignment.ticket_id
            && operation.worker.as_ref() == Some(&worker)
            && let Some(current) = api.store.get_current_ticket_coder_assignment(
                &api.config.workspace_id,
                &assignment.ticket_id,
            )?
            && current.assignment_id == *assignment_id
            && current.worker == worker
        {
            return Ok(current);
        }
        return Err(Error::TicketAssignmentConflict(format!(
            "assignment operation {} is already bound to another assignment",
            assignment.operation_id
        )));
    }
    let assigned_at = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
    let record = TicketCoderAssignmentRecord {
        workspace_id: api.config.workspace_id.clone(),
        ticket_id: ticket.id,
        assignment_id: new_id("tasg"),
        worker,
        assigned_by: "worker-lifecycle".to_string(),
        assigned_at,
    };
    Ok(api
        .store
        .set_current_ticket_coder_assignment(
            &record,
            None,
            &new_id("tasev"),
            &assignment.operation_id,
            false,
        )?
        .current)
}

fn accept_queued_ticket_after_worker_spawn(
    api: &WorkspaceApi,
    assignment: &crate::hosts::WorkerTicketAssignmentRequest,
) -> Result<()> {
    let ticket = api.authority.ticket(&assignment.ticket_id)?;
    if ticket.state == TicketWorkflowState::InProgress.as_str() {
        return Ok(());
    }
    if ticket.state != TicketWorkflowState::Queued.as_str() {
        return Err(Error::TicketAssignmentConflict(format!(
            "Ticket {} left queued state before Coder spawn acceptance; current state is {}",
            ticket.id, ticket.state
        )));
    }
    let mut change = TicketStateChange::new(
        TicketWorkflowState::Queued.as_str(),
        TicketWorkflowState::InProgress.as_str(),
        "Coder spawn, assignment, and initial input were durably accepted",
        "",
    );
    change.author = Some("workspace-orchestrator".to_string());
    browser_ticket_backend(api)?
        .set_workflow_state(TicketIdOrSlug::Id(ticket.id), change)
        .map_err(Error::from)?;
    Ok(())
}

fn existing_lifecycle_assignment_worker(
    api: &WorkspaceApi,
    assignment: &crate::hosts::WorkerTicketAssignmentRequest,
    runtime_id: &str,
) -> Result<Option<WorkerSummary>> {
    let Some(operation) = api
        .store
        .get_ticket_assignment_operation(&api.config.workspace_id, &assignment.operation_id)?
    else {
        return Ok(None);
    };
    if operation.action != "assign"
        || operation.ticket_id != assignment.ticket_id
        || operation.runtime_id.as_deref() != Some(runtime_id)
    {
        return Err(Error::TicketAssignmentConflict(format!(
            "assignment operation {} was already used with different lifecycle input",
            assignment.operation_id
        )));
    }
    let Some(worker_ref) = operation.worker else {
        return Ok(None);
    };
    let worker = api
        .runtime
        .worker(&worker_ref)
        .map_err(|error| error.into_error())?;
    if operation.assignment_id.is_none() && worker.state == "stopped" {
        return Ok(None);
    }
    Ok(Some(worker))
}

fn require_ticket_assignment_value(field: &str, value: String) -> Result<String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(Error::RuntimeOperationFailed {
            runtime_id: "workspace-server".to_string(),
            code: "invalid_ticket_assignment".to_string(),
            message: format!("{field} must not be empty"),
        });
    }
    Ok(value.to_string())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BrowserEditTicketRequest {
    title: Option<String>,
    body: Option<String>,
    old_string: Option<String>,
    new_string: Option<String>,
    #[serde(default)]
    replace_all: bool,
    target: Option<TicketTargetEdit>,
    author: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BrowserTransitionTicketStateRequest {
    state: TicketWorkflowState,
    reason: Option<String>,
    body: Option<String>,
    author: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum BrowserTicketThreadRole {
    Comment,
    Plan,
    Decision,
    ImplementationReport,
}

impl From<BrowserTicketThreadRole> for TicketEventKind {
    fn from(role: BrowserTicketThreadRole) -> Self {
        match role {
            BrowserTicketThreadRole::Comment => Self::Comment,
            BrowserTicketThreadRole::Plan => Self::Plan,
            BrowserTicketThreadRole::Decision => Self::Decision,
            BrowserTicketThreadRole::ImplementationReport => Self::ImplementationReport,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BrowserAppendTicketEventRequest {
    role: BrowserTicketThreadRole,
    body: String,
    author: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BrowserQueueTicketRequest {}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BrowserCloseTicketRequest {
    resolution: String,
}

#[derive(Clone)]
struct WorkspaceTicketTargetAuthority {
    api: WorkspaceApi,
}

impl ticket::TicketTargetAuthority for WorkspaceTicketTargetAuthority {
    fn resolve_target(
        &self,
        workspace_id: &str,
        repository_id: Option<&str>,
        ref_selector: Option<&str>,
    ) -> ticket::Result<ticket::ResolvedTicketTarget> {
        if workspace_id != self.api.config.workspace_id {
            return Err(ticket::TicketError::UnknownTargetRepository(
                repository_id.unwrap_or_default().to_owned(),
            ));
        }
        let repository_id = repository_id
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or(ticket::TicketError::MissingTargetRepository)?;
        let repository = self
            .api
            .store
            .get_repository(workspace_id, repository_id)
            .map_err(|error| ticket::TicketError::Conflict(error.to_string()))?
            .ok_or_else(|| {
                ticket::TicketError::UnknownTargetRepository(repository_id.to_owned())
            })?;
        let selector = ref_selector
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .or(repository.default_ref.as_deref())
            .ok_or_else(|| ticket::TicketError::MissingTargetSelector(repository_id.to_owned()))?;
        self.api
            .repository_reader()
            .observe_merge_target(repository_id, Some(selector))
            .map_err(|error| ticket::TicketError::InvalidTargetSelector {
                repository_id: repository_id.to_owned(),
                selector: selector.to_owned(),
                reason: format!("{error:?}"),
            })?;
        Ok(ticket::ResolvedTicketTarget {
            repository_id: repository_id.to_owned(),
            ref_selector: selector.to_owned(),
        })
    }
}

fn browser_ticket_backend(api: &WorkspaceApi) -> Result<SqliteTicketBackend> {
    Ok(SqliteTicketBackend::open_verified(
        api.config.database_path.clone(),
        api.config.workspace_id.clone(),
    )?
    .with_target_authority(Arc::new(WorkspaceTicketTargetAuthority {
        api: api.clone(),
    })))
}

fn browser_ticket_detail(api: &WorkspaceApi, ticket_id: &str) -> ApiResult<Json<TicketDetail>> {
    Ok(Json(api.authority.ticket(ticket_id)?))
}

async fn scoped_edit_ticket_item(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedRecordPath>,
    Json(request): Json<BrowserEditTicketRequest>,
) -> ApiResult<Json<TicketDetail>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    if let Some(TicketTargetEdit::Set { repository_id, .. }) = request.target.as_ref() {
        if api
            .store
            .get_repository(&api.config.workspace_id, repository_id)?
            .is_none()
        {
            return Err(settings_bad_request(
                "unknown_ticket_repository",
                "repository_id must identify a repository registered in this Workspace",
            ));
        }
    }
    browser_ticket_backend(&api)?
        .edit_item(
            TicketIdOrSlug::Id(path.id.clone()),
            TicketItemEdit {
                title: request.title,
                body: request.body.map(MarkdownText::new),
                body_replacement: match (request.old_string, request.new_string) {
                    (Some(old_string), Some(new_string)) => Some(TicketBodyReplacement {
                        old_string,
                        new_string,
                        replace_all: request.replace_all,
                    }),
                    (None, None) => None,
                    _ => {
                        return Err(settings_bad_request(
                            "invalid_ticket_edit_replacement",
                            "old_string and new_string must be provided together",
                        ));
                    }
                },
                target: request.target,
                author: request.author,
            },
        )
        .map_err(Error::from)?;
    browser_ticket_detail(&api, &path.id)
}

async fn scoped_transition_ticket_state(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedRecordPath>,
    Json(request): Json<BrowserTransitionTicketStateRequest>,
) -> ApiResult<Json<TicketDetail>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    if request.state == TicketWorkflowState::Done {
        return Err(Error::TicketAssignmentConflict(
            "done is guarded by MergeRequestComplete with an approved immutable revision and operation_id".to_string(),
        ).into());
    }
    let current = api.authority.ticket(&path.id)?;
    if request.state == TicketWorkflowState::InProgress
        && current.state != TicketWorkflowState::InProgress.as_str()
    {
        return Err(Error::TicketAssignmentConflict(
            "generic Ticket state mutation cannot enter inprogress; use Queue acceptance or atomic ready-state Coder assignment"
                .to_string(),
        )
        .into());
    }
    let mut change = TicketStateChange::new(
        current.state,
        request.state.as_str(),
        request
            .reason
            .unwrap_or_else(|| "state changed from Web Ticket API".to_owned()),
        request.body.unwrap_or_default(),
    );
    change.author = request.author;
    browser_ticket_backend(&api)?
        .set_workflow_state(TicketIdOrSlug::Id(path.id.clone()), change)
        .map_err(Error::from)?;
    browser_ticket_detail(&api, &path.id)
}

async fn scoped_append_ticket_event(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedRecordPath>,
    Json(request): Json<BrowserAppendTicketEventRequest>,
) -> ApiResult<Json<TicketDetail>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    let mut event = NewTicketEvent::new(request.role.into(), request.body);
    event.author = request.author;
    browser_ticket_backend(&api)?
        .add_event(TicketIdOrSlug::Id(path.id.clone()), event)
        .map_err(Error::from)?;
    browser_ticket_detail(&api, &path.id)
}

async fn scoped_mark_ticket_ready_from_browser(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedRecordPath>,
    Json(request): Json<TicketMarkReadyRequest>,
) -> ApiResult<Json<TicketDetail>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    browser_ticket_backend(&api)?
        .mark_ready(
            TicketIdOrSlug::Id(path.id.clone()),
            ticket::TicketMarkReady {
                operation_key: request.operation_key,
                reason: request.reason,
                author: Some("web".to_owned()),
                intake_summary: None,
            },
        )
        .map_err(Error::from)?;
    browser_ticket_detail(&api, &path.id)
}

async fn scoped_queue_ticket(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedRecordPath>,
    Json(_request): Json<BrowserQueueTicketRequest>,
) -> ApiResult<Json<TicketDetail>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    let _ = execute_ticket_rest_operation(
        &api,
        &path.workspace_id,
        HeaderMap::new(),
        TicketBackendOperation::QueueReady {
            id: TicketIdOrSlug::Id(path.id.clone()),
            queued_by: "workspace-web".to_string(),
        },
    )
    .await?;
    let Json(ticket) = browser_ticket_detail(&api, &path.id)?;
    Ok(Json(ticket))
}

async fn scoped_close_ticket(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedRecordPath>,
    Json(request): Json<BrowserCloseTicketRequest>,
) -> ApiResult<Json<TicketDetail>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    browser_ticket_backend(&api)?
        .close(
            TicketIdOrSlug::Id(path.id.clone()),
            MarkdownText::new(request.resolution),
        )
        .map_err(Error::from)?;
    browser_ticket_detail(&api, &path.id)
}

fn generic_ticket_state_change(operation: &TicketBackendOperation) -> Option<&TicketStateChange> {
    match operation {
        TicketBackendOperation::SetWorkflowState { change, .. }
        | TicketBackendOperation::SetStateField { change, .. }
        | TicketBackendOperation::AddStateChanged { change, .. } => Some(change),
        _ => None,
    }
}

fn reject_unguarded_ticket_start(operation: &TicketBackendOperation) -> Result<()> {
    if generic_ticket_state_change(operation)
        .is_some_and(|change| change.to == "inprogress" && change.from != "inprogress")
    {
        return Err(Error::TicketAssignmentConflict(
            "inprogress is guarded by Queue acceptance or atomic ready-state Coder assignment"
                .to_string(),
        ));
    }
    Ok(())
}

fn reject_unguarded_ticket_completion(operation: &TicketBackendOperation) -> Result<()> {
    reject_unguarded_ticket_start(operation)?;
    if generic_ticket_state_change(operation).is_some_and(|change| change.to == "done") {
        return Err(Error::TicketAssignmentConflict(
            "done is guarded by MergeRequestComplete with an approved immutable revision and operation_id".to_string(),
        ));
    }
    Ok(())
}

async fn execute_ticket_rest_operation(
    api: &WorkspaceApi,
    workspace_id: &str,
    headers: HeaderMap,
    mut operation: TicketBackendOperation,
) -> ApiResult<TicketBackendOperationResult> {
    validate_workspace_scope(api, workspace_id)?;
    let mut backend = SqliteTicketBackend::open_verified(
        api.config.database_path.clone(),
        api.config.workspace_id.clone(),
    )
    .map_err(Error::from)?
    .with_target_authority(Arc::new(WorkspaceTicketTargetAuthority {
        api: api.clone(),
    }));
    let operation_kind = ticket_mutation_operation_kind(&operation);
    let is_mutation = operation_kind != "read";
    let target = ticket_mutation_target(&operation).cloned();
    // Human clients are authorized by the Workspace route boundary. Runtime-forwarded Worker
    // calls carry source headers; when either header is present, the complete pair is required
    // and authenticated before source attribution is attached.
    let source = optional_worker_mutation_source(api, workspace_id, &headers)?;
    reject_unguarded_ticket_completion(&operation)?;
    validate_ticket_repository_operation(api, &operation)?;
    let before = target.as_ref().and_then(|id| backend.show(id.clone()).ok());
    let previous_state = before
        .as_ref()
        .map(|ticket| ticket.meta.workflow_state.as_str().to_string())
        .unwrap_or_else(|| ticket_operation_initial_state(&operation));
    let mut event_attributes = BTreeMap::new();
    if matches!(operation, TicketBackendOperation::QueueReady { .. }) {
        let ticket = before.as_ref().ok_or_else(|| {
            Error::TicketAssignmentConflict(
                "Queue requires an existing Ticket with an active Orchestrator assignment"
                    .to_string(),
            )
        })?;
        let assignment = active_orchestrator_assignment(api, workspace_id, &ticket.meta.id)?
            .ok_or_else(|| {
                Error::TicketAssignmentConflict(
                    "Queue requires role=orchestrator assignment to workspace-orchestrator"
                        .to_string(),
                )
            })?;
        let operation_id = new_id("tqueue");
        let fingerprint = Sha256::digest(format!(
            "ticket-queue:v1\0{workspace_id}\0{}\0{}\0{}",
            ticket.meta.id,
            ticket.meta.workflow_state.as_str(),
            assignment.assignment_id
        ))
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
        event_attributes.extend([
            (
                "orchestrator_assignment_id".to_string(),
                assignment.assignment_id,
            ),
            (
                "routing_principal".to_string(),
                "workspace-orchestrator".to_string(),
            ),
            ("routing_operation_id".to_string(), operation_id),
            ("routing_request_fingerprint".to_string(), fingerprint),
        ]);
    }
    if let Some(source) = source.as_ref() {
        bind_worker_ticket_operation_source(source, &mut operation);
        let source_context =
            worker_ticket_source_context(api, workspace_id, source, before.as_ref());
        event_attributes.extend(source_context.attributes(operation_kind));
    }
    if !event_attributes.is_empty() {
        backend = backend.with_event_attributes(event_attributes);
    }

    let result = execute_ticket_backend_operation(&backend, operation).map_err(Error::from)?;
    if is_mutation
        && let Some(target) = target
        && let Ok(ticket) = backend.show(target)
    {
        notify_ticket_recipients(
            api,
            workspace_id,
            &ticket.meta.id,
            &previous_state,
            ticket.meta.workflow_state.as_str(),
            source,
        );
    }
    Ok(result)
}

#[cfg(test)]
async fn execute_worker_ticket_test_operation(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedWorkspacePath>,
    headers: HeaderMap,
    Json(operation): Json<TicketBackendOperation>,
) -> ApiResult<Json<TicketBackendOperationResult>> {
    execute_ticket_rest_operation(&api, &path.workspace_id, headers, operation)
        .await
        .map(Json)
}

fn ticket_rest_result<T>(
    result: TicketBackendOperationResult,
    extract: impl FnOnce(TicketBackendOperationResult) -> Option<T>,
) -> ApiResult<Json<T>> {
    extract(result).map(Json).ok_or_else(|| {
        Error::Config("Ticket REST handler received an unexpected backend result".to_string())
            .into()
    })
}

fn ticket_rest_unit(result: TicketBackendOperationResult) -> ApiResult<StatusCode> {
    match result {
        TicketBackendOperationResult::Unit => Ok(StatusCode::NO_CONTENT),
        _ => Err(Error::Config(
            "Ticket REST handler received an unexpected backend result".to_string(),
        )
        .into()),
    }
}

#[derive(Debug, Deserialize)]
struct DefaultIntakeReadyBodyRequest {
    from: String,
}

async fn scoped_default_intake_ready_body(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedWorkspacePath>,
    headers: HeaderMap,
    Json(request): Json<DefaultIntakeReadyBodyRequest>,
) -> ApiResult<Json<String>> {
    let result = execute_ticket_rest_operation(
        &api,
        &path.workspace_id,
        headers,
        TicketBackendOperation::DefaultIntakeReadyStateChangeBody { from: request.from },
    )
    .await?;
    ticket_rest_result(result, |result| match result {
        TicketBackendOperationResult::Text(body) => Some(body),
        _ => None,
    })
}

#[derive(Debug, Default, Deserialize)]
struct TicketSummarySearchQuery {
    state: Option<String>,
}

async fn scoped_list_ticket_summaries(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedWorkspacePath>,
    headers: HeaderMap,
    Query(query): Query<TicketSummarySearchQuery>,
) -> ApiResult<Json<Vec<ticket::TicketSummary>>> {
    let filter = match query.state.as_deref().unwrap_or("active") {
        "active" => ticket::TicketListQuery::active(),
        "all" => ticket::TicketListQuery::all(),
        states => {
            let mut selected = Vec::new();
            for state in states.split(',') {
                selected.push(ticket::TicketListState::parse(state).ok_or_else(|| {
                    Error::Ticket(ticket::TicketError::InvalidPathComponent(state.to_string()))
                })?);
            }
            ticket::TicketListQuery::states(selected)
        }
    };
    let result = execute_ticket_rest_operation(
        &api,
        &path.workspace_id,
        headers,
        TicketBackendOperation::List { filter },
    )
    .await?;
    ticket_rest_result(result, |result| match result {
        TicketBackendOperationResult::Tickets(tickets) => Some(tickets),
        _ => None,
    })
}

async fn scoped_get_ticket_record(
    State(api): State<WorkspaceApi>,
    AxumPath((workspace_id, id)): AxumPath<(String, String)>,
    headers: HeaderMap,
) -> ApiResult<Json<ticket::Ticket>> {
    let result = execute_ticket_rest_operation(
        &api,
        &workspace_id,
        headers,
        TicketBackendOperation::Show {
            id: TicketIdOrSlug::Query(id),
        },
    )
    .await?;
    ticket_rest_result(result, |result| match result {
        TicketBackendOperationResult::Ticket(ticket) => Some(ticket),
        _ => None,
    })
}

async fn scoped_create_ticket_record(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedWorkspacePath>,
    headers: HeaderMap,
    Json(input): Json<ticket::NewTicket>,
) -> ApiResult<Json<ticket::TicketRef>> {
    if input
        .workflow_state
        .is_some_and(|state| state != TicketWorkflowState::Planning)
    {
        return Err(settings_bad_request(
            "ticket_create_state_bypass",
            "Ticket creation must start in planning; use guarded workflow operations for later states",
        ));
    }
    let result = execute_ticket_rest_operation(
        &api,
        &path.workspace_id,
        headers,
        TicketBackendOperation::Create { input },
    )
    .await?;
    ticket_rest_result(result, |result| match result {
        TicketBackendOperationResult::TicketRef(ticket) => Some(ticket),
        _ => None,
    })
}

async fn scoped_edit_ticket_record_item(
    State(api): State<WorkspaceApi>,
    AxumPath((workspace_id, id)): AxumPath<(String, String)>,
    headers: HeaderMap,
    Json(edit): Json<TicketItemEdit>,
) -> ApiResult<Json<ticket::Ticket>> {
    let result = execute_ticket_rest_operation(
        &api,
        &workspace_id,
        headers,
        TicketBackendOperation::EditItem {
            id: TicketIdOrSlug::Query(id),
            edit,
        },
    )
    .await?;
    ticket_rest_result(result, |result| match result {
        TicketBackendOperationResult::Ticket(ticket) => Some(ticket),
        _ => None,
    })
}

async fn scoped_ticket_dependency_check(
    State(api): State<WorkspaceApi>,
    AxumPath((workspace_id, id)): AxumPath<(String, String)>,
    headers: HeaderMap,
) -> ApiResult<Json<ticket::TicketDependencyCheck>> {
    let result = execute_ticket_rest_operation(
        &api,
        &workspace_id,
        headers,
        TicketBackendOperation::DependencyCheck {
            id: TicketIdOrSlug::Query(id),
        },
    )
    .await?;
    ticket_rest_result(result, |result| match result {
        TicketBackendOperationResult::DependencyCheck(check) => Some(check),
        _ => None,
    })
}

async fn scoped_add_ticket_thread_event(
    State(api): State<WorkspaceApi>,
    AxumPath((workspace_id, id)): AxumPath<(String, String)>,
    headers: HeaderMap,
    Json(event): Json<NewTicketEvent>,
) -> ApiResult<StatusCode> {
    let result = execute_ticket_rest_operation(
        &api,
        &workspace_id,
        headers,
        TicketBackendOperation::AddEvent {
            id: TicketIdOrSlug::Query(id),
            event,
        },
    )
    .await?;
    ticket_rest_unit(result)
}

async fn scoped_add_ticket_state_change(
    State(api): State<WorkspaceApi>,
    AxumPath((workspace_id, id)): AxumPath<(String, String)>,
    headers: HeaderMap,
    Json(change): Json<TicketStateChange>,
) -> ApiResult<StatusCode> {
    let result = execute_ticket_rest_operation(
        &api,
        &workspace_id,
        headers,
        TicketBackendOperation::AddStateChanged {
            id: TicketIdOrSlug::Query(id),
            change,
        },
    )
    .await?;
    ticket_rest_unit(result)
}

async fn scoped_add_ticket_intake_summary(
    State(api): State<WorkspaceApi>,
    AxumPath((workspace_id, id)): AxumPath<(String, String)>,
    headers: HeaderMap,
    Json(summary): Json<ticket::TicketIntakeSummary>,
) -> ApiResult<StatusCode> {
    let result = execute_ticket_rest_operation(
        &api,
        &workspace_id,
        headers,
        TicketBackendOperation::AddIntakeSummary {
            id: TicketIdOrSlug::Query(id),
            summary,
        },
    )
    .await?;
    ticket_rest_unit(result)
}

#[derive(Debug, Deserialize)]
struct TicketMarkReadyRequest {
    operation_key: String,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    intake_summary: Option<ticket::TicketIntakeSummary>,
}

async fn scoped_set_ticket_state_field(
    State(api): State<WorkspaceApi>,
    AxumPath((workspace_id, id, field)): AxumPath<(String, String, String)>,
    headers: HeaderMap,
    Json(change): Json<TicketStateChange>,
) -> ApiResult<StatusCode> {
    let result = execute_ticket_rest_operation(
        &api,
        &workspace_id,
        headers,
        TicketBackendOperation::SetStateField {
            id: TicketIdOrSlug::Query(id),
            field,
            change,
        },
    )
    .await?;
    ticket_rest_unit(result)
}

async fn scoped_set_ticket_workflow_state(
    State(api): State<WorkspaceApi>,
    AxumPath((workspace_id, id)): AxumPath<(String, String)>,
    headers: HeaderMap,
    Json(change): Json<TicketStateChange>,
) -> ApiResult<StatusCode> {
    let result = execute_ticket_rest_operation(
        &api,
        &workspace_id,
        headers,
        TicketBackendOperation::SetWorkflowState {
            id: TicketIdOrSlug::Query(id),
            change,
        },
    )
    .await?;
    ticket_rest_unit(result)
}

async fn scoped_mark_ticket_ready(
    State(api): State<WorkspaceApi>,
    AxumPath((workspace_id, id)): AxumPath<(String, String)>,
    headers: HeaderMap,
    Json(request): Json<TicketMarkReadyRequest>,
) -> ApiResult<Json<ticket::Ticket>> {
    let result = execute_ticket_rest_operation(
        &api,
        &workspace_id,
        headers,
        TicketBackendOperation::MarkReady {
            id: TicketIdOrSlug::Query(id),
            request: ticket::TicketMarkReady {
                operation_key: request.operation_key,
                reason: request.reason,
                author: None,
                intake_summary: request.intake_summary,
            },
        },
    )
    .await?;
    ticket_rest_result(result, |result| match result {
        TicketBackendOperationResult::Ticket(ticket) => Some(ticket),
        _ => None,
    })
}

async fn scoped_queue_ticket_record(
    State(api): State<WorkspaceApi>,
    AxumPath((workspace_id, id)): AxumPath<(String, String)>,
    headers: HeaderMap,
) -> ApiResult<StatusCode> {
    let result = execute_ticket_rest_operation(
        &api,
        &workspace_id,
        headers,
        TicketBackendOperation::QueueReady {
            id: TicketIdOrSlug::Query(id),
            queued_by: "workspace-web".to_string(),
        },
    )
    .await?;
    ticket_rest_unit(result)
}

#[derive(Debug, serde::Deserialize)]
struct OpenMergeRequestRequest {
    repository_id: String,
    selector_from: String,
    selector_to: String,
    #[serde(default)]
    summary: String,
}

#[derive(Debug, serde::Deserialize)]
struct RepairMergeRequestSelectorRequest {
    selector_from: String,
    reason: String,
    explicit_confirmation: bool,
}

#[derive(Debug, serde::Deserialize)]
struct RevokeMergeRequestReviewRequest {
    review_event_id: String,
    reason: String,
    explicit_confirmation: bool,
}

#[derive(Debug, serde::Deserialize)]
struct MergeRequestThreadQuery {
    after: Option<u64>,
    limit: Option<usize>,
}

#[derive(Debug, serde::Deserialize)]
struct RegisterReviewerChildSessionRequest {
    child_session_id: String,
}

#[derive(Debug, serde::Deserialize)]
struct RegisterMergeRequestReviewCapabilityRequest {
    child_session_id: String,
    capability_token: String,
}

#[derive(Debug, serde::Deserialize)]
struct SubmitMergeRequestReviewRequest {
    capability_token: String,
    decision: merge_request::ReviewDecision,
    #[serde(default)]
    body: String,
    #[serde(default)]
    findings: Vec<merge_request::ReviewFinding>,
}

#[derive(Debug, serde::Deserialize)]
struct CompleteMergeRequestRequest {
    operation_id: String,
    approval_event_id: String,
    target_ref_before: String,
    target_ref_after: String,
    strategy: merge_request::MergeStrategy,
    resolution: merge_request::ConflictResolution,
}

fn parse_workspace_id(value: &str) -> ApiResult<String> {
    if value.trim().is_empty() {
        return Err(Error::InvalidInput("workspace_id must not be empty".to_string()).into());
    }
    Ok(value.to_string())
}

fn require_workspace_access(workspace_id: &str, api: &WorkspaceApi) -> ApiResult<()> {
    if workspace_id != api.workspace_id() {
        return Err(Error::WorkspaceIdMismatch.into());
    }
    Ok(())
}

#[derive(Clone)]
struct MergeRequestAssignmentSource {
    store: Arc<dyn ControlPlaneStore>,
}

impl merge_request::AssignmentSource for MergeRequestAssignmentSource {
    fn current_assignment(
        &self,
        workspace_id: &str,
        ticket_id: &str,
    ) -> std::result::Result<Option<merge_request::CurrentAssignment>, String> {
        self.store
            .get_current_ticket_coder_assignment(workspace_id, ticket_id)
            .map(|value| {
                value.map(|assignment| merge_request::CurrentAssignment {
                    assignment_id: assignment.assignment_id,
                    ticket_id: ticket_id.to_string(),
                    runtime_id: assignment.worker.runtime_id,
                    worker_id: assignment.worker.worker_id,
                })
            })
            .map_err(|error| error.to_string())
    }
}

#[derive(Clone)]
struct MergeRequestRepositorySource {
    workspace_id: String,
    reader: RepositoryRegistryReader,
}

impl TicketMergeRevisionSource for MergeRequestRepositorySource {
    fn resolve_subject_ref(&self, repository_id: &str, selector: &str) -> Option<String> {
        self.reader
            .observe_merge_target(repository_id, Some(selector))
            .ok()
            .map(|target| target.commit)
    }
}

impl merge_request::RepositorySource for MergeRequestRepositorySource {
    fn repository_belongs_to_workspace(
        &self,
        workspace_id: &str,
        repository_id: &str,
    ) -> std::result::Result<bool, String> {
        if workspace_id != self.workspace_id {
            return Ok(false);
        }
        Ok(self.reader.summary(repository_id).is_ok())
    }
}

fn merge_request_store(
    api: &WorkspaceApi,
    workspace_id: &str,
) -> ApiResult<merge_request::MergeRequestStore> {
    require_workspace_access(workspace_id, api)?;
    merge_request::MergeRequestStore::open(
        api.config.database_path.clone(),
        Arc::new(MergeRequestAssignmentSource {
            store: api.store.clone(),
        }),
        Arc::new(MergeRequestRepositorySource {
            workspace_id: workspace_id.to_string(),
            reader: api.repository_reader(),
        }),
    )
    .map_err(Error::from)
    .map_err(Into::into)
}

fn repository_merge_evidence_error(error: RepositoryLookupError) -> ApiError {
    Error::InvalidInput(format!(
        "repository merge evidence validation failed: {error:?}"
    ))
    .into()
}

fn recorded_merge_completion<'a>(
    thread: &'a [merge_request::MergeRequestThreadEvent],
    operation_id: &str,
) -> Option<&'a merge_request::MergeEvent> {
    thread.iter().find_map(|event| match event {
        merge_request::MergeRequestThreadEvent::Merge(event)
            if event.operation_id == operation_id =>
        {
            Some(event)
        }
        _ => None,
    })
}

fn require_completed_target_observation(
    observed: &str,
    target_ref_before: &str,
    target_ref_after: &str,
) -> ApiResult<()> {
    if observed == target_ref_after {
        return Ok(());
    }
    if observed == target_ref_before {
        return Err(Error::InvalidInput(
            "target selector is still at target_ref_before; push the verified result from the Orchestrator Workdir before MergeRequestComplete".into(),
        )
        .into());
    }
    Err(Error::InvalidInput("target selector moved outside completion evidence".into()).into())
}

fn resolve_workspace_ticket_reference(
    api: &WorkspaceApi,
    workspace_id: &str,
    reference: &str,
) -> ApiResult<String> {
    api.store
        .resolve_resource_reference(workspace_id, WorkspaceResourceKind::Ticket, reference)?
        .ok_or_else(|| Error::Ticket(ticket::TicketError::NotFound(reference.to_string())).into())
}

#[derive(Debug, serde::Deserialize)]
struct MergeRequestListHttpQuery {
    state: Option<String>,
    repository_id: Option<String>,
    ticket_ref: Option<String>,
    selector_from: Option<String>,
    selector_to: Option<String>,
    cursor: Option<String>,
    limit: Option<usize>,
}

#[derive(Debug, serde::Serialize)]
struct MergeRequestRefResponse {
    status: String,
    #[serde(rename = "ref")]
    revision_ref: Option<String>,
    observed_at: String,
}

#[derive(Debug, serde::Serialize)]
struct MergeRequestLinkedTicketResponse {
    ticket_id: String,
    key: Option<String>,
}

#[derive(Debug, serde::Serialize)]
struct MergeRequestDetailResponse {
    #[serde(flatten)]
    merge_request: merge_request::MergeRequest,
    source: MergeRequestRefResponse,
    target: MergeRequestRefResponse,
    linked_tickets: Vec<MergeRequestLinkedTicketResponse>,
}

async fn scoped_list_merge_requests(
    State(api): State<WorkspaceApi>,
    AxumPath(workspace_id): AxumPath<String>,
    Query(query): Query<MergeRequestListHttpQuery>,
) -> ApiResult<Json<MergeRequestListResponse>> {
    let workspace_id = parse_workspace_id(&workspace_id)?;
    require_workspace_access(&workspace_id, &api)?;
    let ticket_id = query
        .ticket_ref
        .as_deref()
        .map(|reference| resolve_workspace_ticket_reference(&api, &workspace_id, reference))
        .transpose()?;
    let state = query
        .state
        .as_deref()
        .map(|state| match state {
            "open" => Ok(merge_request::MergeRequestState::Open),
            "merged" => Ok(merge_request::MergeRequestState::Merged),
            "closed" => Ok(merge_request::MergeRequestState::Closed),
            _ => Err(settings_bad_request(
                "invalid_merge_request_state",
                "state must be one of open, merged, or closed",
            )),
        })
        .transpose()?;
    let store = merge_request_store(&api, &workspace_id)?;
    let page = store.list(
        &workspace_id,
        &merge_request::MergeRequestListQuery {
            state,
            repository_id: query.repository_id,
            ticket_id,
            selector_from: query.selector_from,
            selector_to: query.selector_to,
            cursor: query.cursor,
            limit: query.limit.unwrap_or(50),
        },
    )?;
    let reader = api.repository_reader();
    let items = page
        .items
        .into_iter()
        .map(|merge_request| {
            let current_subject_ref = merge_request.selector_from.as_deref().and_then(|selector| {
                reader
                    .observe_merge_target(&merge_request.repository_id, Some(selector))
                    .ok()
                    .map(|observation| observation.commit)
            });
            let ticket_ids = merge_request.ticket_ids.clone();
            let thread_event_count = merge_request.thread.len();
            MergeRequestListItem {
                summary: merge_request_summary(merge_request, current_subject_ref),
                ticket_ids,
                thread_event_count,
            }
        })
        .collect();
    Ok(Json(MergeRequestListResponse {
        items,
        next_cursor: page.next_cursor,
    }))
}

async fn scoped_show_merge_request(
    State(api): State<WorkspaceApi>,
    AxumPath((workspace_id, merge_request_id)): AxumPath<(String, String)>,
    Query(query): Query<MergeRequestThreadQuery>,
) -> ApiResult<Json<MergeRequestDetailResponse>> {
    let workspace_id = parse_workspace_id(&workspace_id)?;
    require_workspace_access(&workspace_id, &api)?;
    let store = merge_request_store(&api, &workspace_id)?;
    let mut mr = store.get_by_id(&workspace_id, &merge_request_id)?;
    mr.thread = store.thread_page_by_id(
        &workspace_id,
        &merge_request_id,
        query.after,
        query.limit.unwrap_or(100),
    )?;
    let reader = api.repository_reader();
    let observed_at = Utc::now().to_rfc3339();
    let source = match mr.selector_from.as_deref() {
        Some(selector) => match reader.observe_merge_target(&mr.repository_id, Some(selector)) {
            Ok(value) => MergeRequestRefResponse {
                status: "known".into(),
                revision_ref: Some(value.commit),
                observed_at: observed_at.clone(),
            },
            Err(_) => MergeRequestRefResponse {
                status: "unknown".into(),
                revision_ref: None,
                observed_at: observed_at.clone(),
            },
        },
        None => MergeRequestRefResponse {
            status: "requires_repair".into(),
            revision_ref: None,
            observed_at: observed_at.clone(),
        },
    };
    let target = match reader.observe_merge_target(&mr.repository_id, Some(&mr.selector_to)) {
        Ok(value) => MergeRequestRefResponse {
            status: "known".into(),
            revision_ref: Some(value.commit),
            observed_at,
        },
        Err(_) => MergeRequestRefResponse {
            status: "unknown".into(),
            revision_ref: None,
            observed_at,
        },
    };
    let linked_tickets = mr
        .ticket_ids
        .iter()
        .map(|ticket_id| {
            Ok(MergeRequestLinkedTicketResponse {
                ticket_id: ticket_id.clone(),
                key: api.store.resource_key(
                    &workspace_id,
                    WorkspaceResourceKind::Ticket,
                    ticket_id,
                )?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Json(MergeRequestDetailResponse {
        merge_request: mr,
        source,
        target,
        linked_tickets,
    }))
}

async fn scoped_merge_request_readiness(
    State(api): State<WorkspaceApi>,
    AxumPath((workspace_id, ticket_id)): AxumPath<(String, String)>,
) -> ApiResult<Json<merge_request::ReadinessReport>> {
    let workspace_id = parse_workspace_id(&workspace_id)?;
    let ticket_id = resolve_workspace_ticket_reference(&api, &workspace_id, &ticket_id)?;
    let store = merge_request_store(&api, &workspace_id)?;
    let mr = store.get(&workspace_id, &ticket_id)?;
    let current_subject_ref = mr.selector_from.as_deref().and_then(|selector| {
        api.repository_reader()
            .observe_merge_target(&mr.repository_id, Some(selector))
            .ok()
            .map(|v| v.commit)
    });
    Ok(Json(store.readiness(merge_request::ReadinessCheck {
        ticket_id,
        current_subject_ref,
        auth: merge_request::MergeRequestAuth {
            workspace_id,
            repository_id: mr.repository_id,
            runtime_id: String::new(),
            worker_id: String::new(),
            assignment_id: String::new(),
        },
    })?))
}

async fn scoped_open_merge_request(
    State(api): State<WorkspaceApi>,
    headers: HeaderMap,
    AxumPath((workspace_id, ticket_id)): AxumPath<(String, String)>,
    Json(input): Json<OpenMergeRequestRequest>,
) -> ApiResult<Json<merge_request::MergeRequest>> {
    let workspace_id = parse_workspace_id(&workspace_id)?;
    let ticket_id = resolve_workspace_ticket_reference(&api, &workspace_id, &ticket_id)?;
    require_workspace_access(&workspace_id, &api)?;
    let source = authenticate_worker_mutation_source(&api, &workspace_id, &headers)?;
    let assignment = api
        .store
        .get_current_ticket_coder_assignment(&workspace_id, &ticket_id)?
        .ok_or_else(|| {
            Error::TicketAssignmentConflict("Ticket has no current assigned Coder".into())
        })?;
    if assignment.worker.runtime_id != source.runtime_id
        || assignment.worker.worker_id != source.worker_id
    {
        return Err(Error::TicketAssignmentConflict(
            "authenticated Worker is not current assignee".into(),
        )
        .into());
    }
    let ticket = browser_ticket_backend(&api)?
        .show(TicketIdOrSlug::Id(ticket_id.clone().into()))
        .map_err(Error::from)?;
    if ticket.meta.repository_id.as_deref() != Some(input.repository_id.as_str())
        || ticket.meta.ref_selector.as_deref() != Some(input.selector_to.as_str())
    {
        return Err(Error::InvalidInput(
            "selectors must match the authoritative Ticket repository target".into(),
        )
        .into());
    }
    let reader = api.repository_reader();
    reader
        .observe_merge_target(&input.repository_id, Some(&input.selector_from))
        .map_err(repository_merge_evidence_error)?;
    reader
        .observe_merge_target(&input.repository_id, Some(&input.selector_to))
        .map_err(repository_merge_evidence_error)?;
    Ok(Json(
        merge_request_store(&api, &workspace_id)?.open_merge_request(
            merge_request::OpenMergeRequest {
                merge_request_id: Uuid::now_v7().to_string(),
                ticket_id,
                repository_id: input.repository_id.clone(),
                selector_from: input.selector_from,
                selector_to: input.selector_to,
                summary: input.summary,
                auth: merge_request::MergeRequestAuth {
                    workspace_id,
                    repository_id: input.repository_id,
                    runtime_id: source.runtime_id,
                    worker_id: source.worker_id,
                    assignment_id: assignment.assignment_id,
                },
                now: Utc::now(),
            },
        )?,
    ))
}

async fn scoped_merge_request_thread(
    State(api): State<WorkspaceApi>,
    AxumPath((workspace_id, ticket_id)): AxumPath<(String, String)>,
    Query(query): Query<MergeRequestThreadQuery>,
) -> ApiResult<Json<Vec<merge_request::MergeRequestThreadEvent>>> {
    let workspace_id = parse_workspace_id(&workspace_id)?;
    let ticket_id = resolve_workspace_ticket_reference(&api, &workspace_id, &ticket_id)?;
    Ok(Json(
        merge_request_store(&api, &workspace_id)?.thread_page(
            &workspace_id,
            &ticket_id,
            query.after,
            query.limit.unwrap_or(100),
        )?,
    ))
}

async fn scoped_repair_merge_request_selector(
    State(api): State<WorkspaceApi>,
    headers: HeaderMap,
    AxumPath((workspace_id, ticket_id)): AxumPath<(String, String)>,
    Json(input): Json<RepairMergeRequestSelectorRequest>,
) -> ApiResult<Json<merge_request::MergeRequest>> {
    let workspace_id = parse_workspace_id(&workspace_id)?;
    let ticket_id = resolve_workspace_ticket_reference(&api, &workspace_id, &ticket_id)?;
    require_workspace_access(&workspace_id, &api)?;
    reject_non_browser_reopen_auth(&headers)?;
    let _actor = require_actor(&ServerAuthApi::from(&api), &headers).await?;
    if !input.explicit_confirmation {
        return Err(Error::BrowserReopenConfirmationRequired.into());
    }
    let store = merge_request_store(&api, &workspace_id)?;
    let mr = store.get(&workspace_id, &ticket_id)?;
    let resolved_subject_ref = api
        .repository_reader()
        .observe_merge_target(&mr.repository_id, Some(&input.selector_from))
        .map_err(repository_merge_evidence_error)?
        .commit;
    Ok(Json(store.repair_selector_from(
        merge_request::RepairSelectorFrom {
            workspace_id,
            ticket_id,
            selector_from: input.selector_from,
            resolved_subject_ref,
            repaired_by: merge_request::WorkerIdentity {
                runtime_id: "browser".into(),
                worker_id: "authenticated-user".into(),
            },
            reason: input.reason,
            now: Utc::now(),
        },
    )?))
}

async fn scoped_register_reviewer_child_session(
    State(api): State<WorkspaceApi>,
    headers: HeaderMap,
    AxumPath(workspace_id): AxumPath<String>,
    Json(input): Json<RegisterReviewerChildSessionRequest>,
) -> ApiResult<StatusCode> {
    let workspace_id = parse_workspace_id(&workspace_id)?;
    require_workspace_access(&workspace_id, &api)?;
    let source = authenticate_worker_mutation_source(&api, &workspace_id, &headers)?;
    merge_request_store(&api, &workspace_id)?.register_reviewer_child_session(
        merge_request::RegisterReviewerChildSession {
            workspace_id,
            parent_runtime_id: source.runtime_id,
            parent_worker_id: source.worker_id,
            child_session_id: input.child_session_id,
            reviewer_profile: "builtin:reviewer".into(),
            now: Utc::now(),
        },
    )?;
    Ok(StatusCode::NO_CONTENT)
}

async fn scoped_register_merge_request_review_capability(
    State(api): State<WorkspaceApi>,
    headers: HeaderMap,
    AxumPath((workspace_id, ticket_id)): AxumPath<(String, String)>,
    Json(input): Json<RegisterMergeRequestReviewCapabilityRequest>,
) -> ApiResult<StatusCode> {
    let workspace_id = parse_workspace_id(&workspace_id)?;
    let ticket_id = resolve_workspace_ticket_reference(&api, &workspace_id, &ticket_id)?;
    require_workspace_access(&workspace_id, &api)?;
    let source = authenticate_worker_mutation_source(&api, &workspace_id, &headers)?;
    let assignment = api
        .store
        .get_current_ticket_coder_assignment(&workspace_id, &ticket_id)?
        .ok_or_else(|| {
            Error::TicketAssignmentConflict("Ticket has no current assigned Coder".into())
        })?;
    if assignment.worker.runtime_id != source.runtime_id
        || assignment.worker.worker_id != source.worker_id
    {
        return Err(Error::TicketAssignmentConflict(
            "authenticated Worker is not current assignee".into(),
        )
        .into());
    }
    let store = merge_request_store(&api, &workspace_id)?;
    let mr = store.get(&workspace_id, &ticket_id)?;
    let selector = mr
        .selector_from
        .as_deref()
        .ok_or_else(|| Error::InvalidInput("selector_from requires repair".into()))?;
    let subject_ref = api
        .repository_reader()
        .observe_merge_target(&mr.repository_id, Some(selector))
        .map_err(repository_merge_evidence_error)?
        .commit;
    store.request_review(merge_request::RequestMergeRequestReview {
        ticket_id,
        subject_ref,
        child_session_id: input.child_session_id,
        capability_token: input.capability_token,
        auth: merge_request::MergeRequestAuth {
            workspace_id,
            repository_id: mr.repository_id,
            runtime_id: source.runtime_id,
            worker_id: source.worker_id,
            assignment_id: assignment.assignment_id,
        },
        now: Utc::now(),
    })?;
    Ok(StatusCode::NO_CONTENT)
}

async fn scoped_submit_merge_request_review(
    State(api): State<WorkspaceApi>,
    AxumPath((workspace_id, ticket_id)): AxumPath<(String, String)>,
    Json(input): Json<SubmitMergeRequestReviewRequest>,
) -> ApiResult<Json<merge_request::ReviewEvent>> {
    let workspace_id = parse_workspace_id(&workspace_id)?;
    let ticket_id = resolve_workspace_ticket_reference(&api, &workspace_id, &ticket_id)?;
    let store = merge_request_store(&api, &workspace_id)?;
    let mr = store.get(&workspace_id, &ticket_id)?;
    let selector = mr
        .selector_from
        .as_deref()
        .ok_or_else(|| Error::InvalidInput("selector_from requires repair".into()))?;
    let current_subject_ref = api
        .repository_reader()
        .observe_merge_target(&mr.repository_id, Some(selector))
        .map_err(repository_merge_evidence_error)?
        .commit;
    Ok(Json(store.submit_review(
        merge_request::SubmitMergeRequestReview {
            ticket_id,
            current_subject_ref,
            capability_token: input.capability_token,
            decision: input.decision,
            body: input.body,
            findings: input.findings,
            now: Utc::now(),
        },
    )?))
}

async fn scoped_revoke_merge_request_review(
    State(api): State<WorkspaceApi>,
    headers: HeaderMap,
    AxumPath((workspace_id, ticket_id)): AxumPath<(String, String)>,
    Json(input): Json<RevokeMergeRequestReviewRequest>,
) -> ApiResult<Json<merge_request::ReviewRevokedEvent>> {
    let workspace_id = parse_workspace_id(&workspace_id)?;
    let ticket_id = resolve_workspace_ticket_reference(&api, &workspace_id, &ticket_id)?;
    require_workspace_access(&workspace_id, &api)?;
    if !input.explicit_confirmation {
        return Err(Error::BrowserReopenConfirmationRequired.into());
    }
    let source = authenticate_worker_mutation_source(&api, &workspace_id, &headers)?;
    let assignment = api
        .store
        .get_current_ticket_coder_assignment(&workspace_id, &ticket_id)?
        .ok_or_else(|| {
            Error::TicketAssignmentConflict("Ticket has no current assigned Coder".into())
        })?;
    if assignment.worker.runtime_id != source.runtime_id
        || assignment.worker.worker_id != source.worker_id
    {
        return Err(Error::TicketAssignmentConflict(
            "authenticated Worker is not current assignee".into(),
        )
        .into());
    }
    let store = merge_request_store(&api, &workspace_id)?;
    let mr = store.get(&workspace_id, &ticket_id)?;
    Ok(Json(store.revoke_review(
        merge_request::RevokeMergeRequestReview {
            ticket_id,
            review_event_id: input.review_event_id,
            reason: input.reason,
            auth: merge_request::MergeRequestAuth {
                workspace_id,
                repository_id: mr.repository_id,
                runtime_id: source.runtime_id,
                worker_id: source.worker_id,
                assignment_id: assignment.assignment_id,
            },
            now: Utc::now(),
        },
    )?))
}

async fn scoped_complete_merge_request(
    State(api): State<WorkspaceApi>,
    headers: HeaderMap,
    AxumPath((workspace_id, ticket_id)): AxumPath<(String, String)>,
    Json(input): Json<CompleteMergeRequestRequest>,
) -> ApiResult<Json<merge_request::MergeEvent>> {
    let workspace_id = parse_workspace_id(&workspace_id)?;
    let ticket_id = resolve_workspace_ticket_reference(&api, &workspace_id, &ticket_id)?;
    require_workspace_access(&workspace_id, &api)?;
    let source = authenticate_worker_mutation_source(&api, &workspace_id, &headers)?;
    require_online_workspace_orchestrator_source(&api, &source)?;
    let store = merge_request_store(&api, &workspace_id)?;
    let mr = store.get(&workspace_id, &ticket_id)?;
    let repositories = api.repository_reader();
    if let Some(existing) = recorded_merge_completion(&mr.thread, &input.operation_id) {
        let replay = merge_request::CompleteMergeRequest {
            ticket_id,
            operation_id: input.operation_id,
            approval_event_id: input.approval_event_id,
            current_subject_ref: existing.approved_source_ref.clone(),
            target_ref_before: input.target_ref_before,
            target_ref_after: input.target_ref_after,
            strategy: input.strategy,
            resolution: input.resolution,
            auth: merge_request::MergeRequestAuth {
                workspace_id,
                repository_id: mr.repository_id.clone(),
                runtime_id: source.runtime_id,
                worker_id: source.worker_id,
                assignment_id: String::new(),
            },
            now: Utc::now(),
        };
        return store.complete(replay).map(Json).map_err(Into::into);
    }
    let assignment = api
        .store
        .get_current_ticket_coder_assignment(&workspace_id, &ticket_id)?
        .ok_or_else(|| {
            Error::TicketAssignmentConflict("Ticket has no current assigned Coder".into())
        })?;
    let selector = mr
        .selector_from
        .as_deref()
        .ok_or_else(|| Error::InvalidInput("selector_from requires repair".into()))?;
    let current_source_ref = repositories
        .observe_merge_target(&mr.repository_id, Some(selector))
        .map_err(repository_merge_evidence_error)?
        .commit;
    let observed = repositories
        .observe_merge_target(&mr.repository_id, Some(&mr.selector_to))
        .map_err(repository_merge_evidence_error)?;
    require_completed_target_observation(
        &observed.commit,
        &input.target_ref_before,
        &input.target_ref_after,
    )?;
    let completion = merge_request::CompleteMergeRequest {
        ticket_id,
        operation_id: input.operation_id,
        approval_event_id: input.approval_event_id,
        current_subject_ref: current_source_ref,
        target_ref_before: input.target_ref_before.clone(),
        target_ref_after: input.target_ref_after.clone(),
        strategy: input.strategy,
        resolution: input.resolution,
        auth: merge_request::MergeRequestAuth {
            workspace_id,
            repository_id: mr.repository_id.clone(),
            runtime_id: source.runtime_id,
            worker_id: source.worker_id,
            assignment_id: assignment.assignment_id,
        },
        now: Utc::now(),
    };
    store.validate_completion(&completion)?;
    store.complete(completion).map(Json).map_err(Into::into)
}

fn reject_non_browser_reopen_auth(headers: &HeaderMap) -> Result<()> {
    if headers.contains_key("authorization") {
        return Err(Error::BrowserReopenConfirmationRequired);
    }
    Ok(())
}

async fn scoped_close_ticket_record(
    State(api): State<WorkspaceApi>,
    AxumPath((workspace_id, id)): AxumPath<(String, String)>,
    headers: HeaderMap,
    Json(resolution): Json<MarkdownText>,
) -> ApiResult<StatusCode> {
    let result = execute_ticket_rest_operation(
        &api,
        &workspace_id,
        headers,
        TicketBackendOperation::Close {
            id: TicketIdOrSlug::Query(id),
            resolution,
        },
    )
    .await?;
    ticket_rest_unit(result)
}

async fn scoped_record_ticket_relation(
    State(api): State<WorkspaceApi>,
    AxumPath((workspace_id, id)): AxumPath<(String, String)>,
    headers: HeaderMap,
    Json(relation): Json<ticket::NewTicketRelation>,
) -> ApiResult<Json<ticket::TicketRelation>> {
    let result = execute_ticket_rest_operation(
        &api,
        &workspace_id,
        headers,
        TicketBackendOperation::AddTicketRelation {
            id: TicketIdOrSlug::Query(id),
            relation,
        },
    )
    .await?;
    ticket_rest_result(result, |result| match result {
        TicketBackendOperationResult::Relation(relation) => Some(relation),
        _ => None,
    })
}

#[derive(Debug, Deserialize)]
struct TicketRelationRemoveRequest {
    kind: ticket::TicketRelationKind,
    target: String,
}

async fn scoped_remove_ticket_relation(
    State(api): State<WorkspaceApi>,
    AxumPath((workspace_id, id)): AxumPath<(String, String)>,
    headers: HeaderMap,
    Json(relation): Json<TicketRelationRemoveRequest>,
) -> ApiResult<Json<ticket::TicketRelation>> {
    let result = execute_ticket_rest_operation(
        &api,
        &workspace_id,
        headers,
        TicketBackendOperation::RemoveTicketRelation {
            id: TicketIdOrSlug::Query(id),
            kind: relation.kind,
            target: TicketIdOrSlug::Id(relation.target),
        },
    )
    .await?;
    ticket_rest_result(result, |result| match result {
        TicketBackendOperationResult::Relation(relation) => Some(relation),
        _ => None,
    })
}

#[derive(Debug, Deserialize)]
struct TicketRelationSearchRequest {
    ticket: Option<TicketIdOrSlug>,
    kind: Option<ticket::TicketRelationKind>,
}

async fn scoped_query_ticket_relations(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedWorkspacePath>,
    headers: HeaderMap,
    Json(query): Json<TicketRelationSearchRequest>,
) -> ApiResult<Json<Vec<ticket::TicketRelation>>> {
    let result = execute_ticket_rest_operation(
        &api,
        &path.workspace_id,
        headers,
        TicketBackendOperation::QueryTicketRelations {
            ticket: query.ticket,
            kind: query.kind,
        },
    )
    .await?;
    ticket_rest_result(result, |result| match result {
        TicketBackendOperationResult::Relations(relations) => Some(relations),
        _ => None,
    })
}

async fn scoped_ticket_relation_view(
    State(api): State<WorkspaceApi>,
    AxumPath((workspace_id, id)): AxumPath<(String, String)>,
    headers: HeaderMap,
) -> ApiResult<Json<ticket::TicketRelationView>> {
    let result = execute_ticket_rest_operation(
        &api,
        &workspace_id,
        headers,
        TicketBackendOperation::RelationView {
            id: TicketIdOrSlug::Query(id),
        },
    )
    .await?;
    ticket_rest_result(result, |result| match result {
        TicketBackendOperationResult::RelationView(view) => Some(view),
        _ => None,
    })
}

async fn scoped_record_ticket_orchestration_plan(
    State(api): State<WorkspaceApi>,
    AxumPath((workspace_id, id)): AxumPath<(String, String)>,
    headers: HeaderMap,
    Json(record): Json<ticket::NewOrchestrationPlanRecord>,
) -> ApiResult<Json<ticket::OrchestrationPlanRecord>> {
    let result = execute_ticket_rest_operation(
        &api,
        &workspace_id,
        headers,
        TicketBackendOperation::AddOrchestrationPlanRecord {
            id: TicketIdOrSlug::Query(id),
            record,
        },
    )
    .await?;
    ticket_rest_result(result, |result| match result {
        TicketBackendOperationResult::OrchestrationPlanRecord(record) => Some(record),
        _ => None,
    })
}

#[derive(Debug, Deserialize)]
struct TicketOrchestrationPlanSearchRequest {
    ticket: Option<TicketIdOrSlug>,
    kind: Option<ticket::OrchestrationPlanKind>,
}

async fn scoped_query_ticket_orchestration_plans(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedWorkspacePath>,
    headers: HeaderMap,
    Json(query): Json<TicketOrchestrationPlanSearchRequest>,
) -> ApiResult<Json<Vec<ticket::OrchestrationPlanRecord>>> {
    let result = execute_ticket_rest_operation(
        &api,
        &path.workspace_id,
        headers,
        TicketBackendOperation::QueryOrchestrationPlanRecords {
            ticket: query.ticket,
            kind: query.kind,
        },
    )
    .await?;
    ticket_rest_result(result, |result| match result {
        TicketBackendOperationResult::OrchestrationPlanRecords(records) => Some(records),
        _ => None,
    })
}

async fn scoped_ticket_doctor(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedWorkspacePath>,
    headers: HeaderMap,
) -> ApiResult<Json<ticket::TicketDoctorReport>> {
    let result = execute_ticket_rest_operation(
        &api,
        &path.workspace_id,
        headers,
        TicketBackendOperation::Doctor,
    )
    .await?;
    ticket_rest_result(result, |result| match result {
        TicketBackendOperationResult::DoctorReport(report) => Some(report),
        _ => None,
    })
}

type WorkerMutationSource = RuntimeWorkerRef;

fn ticket_mutation_target(operation: &TicketBackendOperation) -> Option<&TicketIdOrSlug> {
    match operation {
        TicketBackendOperation::EditItem { id, .. }
        | TicketBackendOperation::AddEvent { id, .. }
        | TicketBackendOperation::AddStateChanged { id, .. }
        | TicketBackendOperation::AddIntakeSummary { id, .. }
        | TicketBackendOperation::SetStateField { id, .. }
        | TicketBackendOperation::SetWorkflowState { id, .. }
        | TicketBackendOperation::MarkReady { id, .. }
        | TicketBackendOperation::QueueReady { id, .. }
        | TicketBackendOperation::Close { id, .. }
        | TicketBackendOperation::AddTicketRelation { id, .. }
        | TicketBackendOperation::RemoveTicketRelation { id, .. }
        | TicketBackendOperation::AddOrchestrationPlanRecord { id, .. } => Some(id),
        _ => None,
    }
}

fn validate_ticket_repository_operation(
    api: &WorkspaceApi,
    operation: &TicketBackendOperation,
) -> ApiResult<()> {
    let repository_id = match operation {
        TicketBackendOperation::Create { input } => input.repository_id.as_deref(),
        TicketBackendOperation::EditItem { edit, .. } => match edit.target.as_ref() {
            Some(TicketTargetEdit::Set { repository_id, .. }) => Some(repository_id.as_str()),
            _ => None,
        },
        _ => None,
    };
    if let Some(repository_id) = repository_id {
        api.require_workspace_repository(repository_id)?;
    }
    Ok(())
}

fn bind_worker_ticket_operation_source(
    source: &WorkerMutationSource,
    operation: &mut TicketBackendOperation,
) {
    let author = format!("worker:{}/{}", source.runtime_id, source.worker_id);
    match operation {
        TicketBackendOperation::Create { input } => input.author = Some(author),
        TicketBackendOperation::EditItem { edit, .. } => edit.author = Some(author),
        TicketBackendOperation::AddEvent { event, .. } => event.author = Some(author),
        TicketBackendOperation::AddStateChanged { change, .. }
        | TicketBackendOperation::SetStateField { change, .. }
        | TicketBackendOperation::SetWorkflowState { change, .. } => change.author = Some(author),
        TicketBackendOperation::AddIntakeSummary { summary, .. } => summary.author = Some(author),
        TicketBackendOperation::MarkReady { request, .. } => {
            request.author = Some(author.clone());
            if let Some(summary) = request.intake_summary.as_mut() {
                summary.author = Some(author);
            }
        }
        TicketBackendOperation::QueueReady { queued_by, .. } => *queued_by = author,
        TicketBackendOperation::AddTicketRelation { relation, .. } => {
            relation.author = Some(author)
        }
        TicketBackendOperation::AddOrchestrationPlanRecord { record, .. } => {
            record.author = Some(author)
        }
        _ => {}
    }
}

fn ticket_mutation_operation_kind(operation: &TicketBackendOperation) -> &'static str {
    match operation {
        TicketBackendOperation::Create { .. } => "create",
        TicketBackendOperation::EditItem { .. } => "edit_item",
        TicketBackendOperation::AddEvent { .. } => "add_event",
        TicketBackendOperation::AddStateChanged { .. } => "add_state_changed",
        TicketBackendOperation::AddIntakeSummary { .. } => "add_intake_summary",
        TicketBackendOperation::SetStateField { .. } => "set_state_field",
        TicketBackendOperation::SetWorkflowState { .. } => "set_workflow_state",
        TicketBackendOperation::MarkReady { .. } => "mark_ready",
        TicketBackendOperation::QueueReady { .. } => "queue_ready",
        TicketBackendOperation::Close { .. } => "close",
        TicketBackendOperation::AddTicketRelation { .. } => "add_relation",
        TicketBackendOperation::RemoveTicketRelation { .. } => "remove_relation",
        TicketBackendOperation::AddOrchestrationPlanRecord { .. } => "add_plan_record",
        _ => "read",
    }
}

fn ticket_operation_initial_state(operation: &TicketBackendOperation) -> String {
    match operation {
        TicketBackendOperation::Create { input } => input
            .workflow_state
            .as_ref()
            .map(|state| state.as_str().to_string())
            .unwrap_or_else(|| TicketWorkflowState::Planning.as_str().to_string()),
        _ => TicketWorkflowState::Planning.as_str().to_string(),
    }
}

#[derive(Debug, Clone)]
struct WorkerTicketSourceContext {
    worker: RuntimeWorkerRef,
    actor_role: String,
    assignment_id: Option<String>,
}

impl WorkerTicketSourceContext {
    fn attributes(&self, operation_kind: &str) -> BTreeMap<String, String> {
        let mut attributes = BTreeMap::from([
            (
                "source_runtime_id".to_string(),
                self.worker.runtime_id.clone(),
            ),
            (
                "source_worker_id".to_string(),
                self.worker.worker_id.clone(),
            ),
            ("source_actor_role".to_string(), self.actor_role.clone()),
            (
                "source_operation_kind".to_string(),
                operation_kind.to_string(),
            ),
        ]);
        if let Some(assignment_id) = &self.assignment_id {
            attributes.insert("source_assignment_id".to_string(), assignment_id.clone());
        }
        attributes
    }
}

fn worker_source_actor_role(is_current_assignment: bool, is_orchestrator: bool) -> &'static str {
    if is_current_assignment {
        "coder"
    } else if is_orchestrator {
        "orchestrator"
    } else {
        "worker"
    }
}

fn active_orchestrator_assignment(
    api: &WorkspaceApi,
    workspace_id: &str,
    ticket_id: &str,
) -> Result<Option<TicketRoleAssignmentRecord>> {
    let assignment = api.store.get_current_ticket_role_assignment(
        workspace_id,
        ticket_id,
        TicketAssignmentRole::Orchestrator,
    )?;
    match assignment {
        Some(assignment)
            if matches!(
                assignment.principal,
                TicketAssignmentPrincipal::WorkspaceAgent { ref agent_key }
                    if agent_key == "workspace-orchestrator"
            ) =>
        {
            Ok(Some(assignment))
        }
        Some(_) => Err(Error::TicketAssignmentConflict(
            "Orchestrator role must reference the registered workspace-orchestrator principal"
                .to_string(),
        )),
        None => Ok(None),
    }
}

fn orchestrator_interested(
    api: &WorkspaceApi,
    workspace_id: &str,
    ticket_id: &str,
    state: &str,
) -> Result<Option<TicketRoleAssignmentRecord>> {
    if !matches!(state, "queued" | "inprogress") {
        return Ok(None);
    }
    active_orchestrator_assignment(api, workspace_id, ticket_id)
}

fn worker_ticket_source_context(
    api: &WorkspaceApi,
    workspace_id: &str,
    source: &WorkerMutationSource,
    ticket: Option<&ticket::Ticket>,
) -> WorkerTicketSourceContext {
    let assignment = ticket.and_then(|ticket| {
        api.store
            .get_current_ticket_coder_assignment(workspace_id, &ticket.meta.id)
            .ok()
            .flatten()
    });
    let orchestrator = find_workspace_orchestrator(api);
    let is_current_assignment = assignment
        .as_ref()
        .is_some_and(|assignment| &assignment.worker == source);
    let is_orchestrator = active_orchestrator_assignment(
        api,
        workspace_id,
        ticket
            .map(|ticket| ticket.meta.id.as_str())
            .unwrap_or_default(),
    )
    .ok()
    .flatten()
    .is_some()
        && orchestrator
            .as_ref()
            .is_some_and(|worker| worker.worker == *source);
    let actor_role = worker_source_actor_role(is_current_assignment, is_orchestrator);
    WorkerTicketSourceContext {
        worker: source.clone(),
        actor_role: actor_role.to_string(),
        assignment_id: assignment.and_then(|assignment| {
            (assignment.worker == *source).then_some(assignment.assignment_id)
        }),
    }
}

fn ticket_notification_content(ticket_id: &str, current_state: &str) -> String {
    format!(
        "Ticket notification: ticket_id={ticket_id} current_state={current_state}. Reread the Ticket before acting."
    )
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct TicketNotificationDeliveryWarning {
    level: &'static str,
    event: &'static str,
    workspace_id: String,
    ticket_id: String,
    current_state: String,
    recipient_runtime_id: String,
    recipient_worker_id: String,
    error_category: &'static str,
}

impl TicketNotificationDeliveryWarning {
    fn new(
        workspace_id: &str,
        ticket_id: &str,
        current_state: &str,
        recipient: &RuntimeWorkerRef,
        error_category: &'static str,
    ) -> Self {
        Self {
            level: "warning",
            event: "ticket_notification_delivery_failed",
            workspace_id: workspace_id.to_string(),
            ticket_id: ticket_id.to_string(),
            current_state: current_state.to_string(),
            recipient_runtime_id: recipient.runtime_id.clone(),
            recipient_worker_id: recipient.worker_id.clone(),
            error_category,
        }
    }
}

#[cfg(test)]
static TICKET_NOTIFICATION_DELIVERY_WARNING_CAPTURE: Mutex<Vec<TicketNotificationDeliveryWarning>> =
    Mutex::new(Vec::new());

fn ticket_notification_delivery_error_category(
    result: &std::result::Result<WorkerInputResult, RuntimeRegistryError>,
) -> Option<&'static str> {
    match result {
        Ok(result) => match result.state {
            WorkerOperationState::Accepted => None,
            WorkerOperationState::Rejected => Some("runtime_rejected"),
            WorkerOperationState::Unsupported => Some("runtime_unsupported"),
        },
        Err(RuntimeRegistryError::InvalidIdentifier { .. }) => Some("invalid_identifier"),
        Err(RuntimeRegistryError::UnknownRuntime(_)) => Some("unknown_runtime"),
        Err(RuntimeRegistryError::UnknownHost(_)) => Some("unknown_host"),
        Err(RuntimeRegistryError::UnknownWorker { .. }) => Some("unknown_worker"),
        Err(RuntimeRegistryError::RuntimeOperationFailed { .. }) => {
            Some("runtime_operation_failed")
        }
    }
}

fn emit_ticket_notification_delivery_warning(warning: TicketNotificationDeliveryWarning) {
    let serialized = serde_json::to_string(&warning)
        .expect("Ticket notification delivery warnings serialize from bounded string fields");
    eprintln!(
        "{} yoi-server {serialized}",
        Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
    );
    #[cfg(test)]
    TICKET_NOTIFICATION_DELIVERY_WARNING_CAPTURE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .push(warning);
}

fn notify_ticket_recipients(
    api: &WorkspaceApi,
    workspace_id: &str,
    ticket_id: &str,
    _previous_state: &str,
    current_state: &str,
    source: Option<RuntimeWorkerRef>,
) {
    let mut recipients = Vec::new();
    if let Some(assignment) = api
        .store
        .get_current_ticket_coder_assignment(workspace_id, ticket_id)
        .ok()
        .flatten()
    {
        recipients.push(assignment.worker.clone());
    }
    if orchestrator_interested(api, workspace_id, ticket_id, current_state)
        .ok()
        .flatten()
        .is_some()
        && let Some(orchestrator) = find_workspace_orchestrator(api)
    {
        recipients.push(orchestrator.worker.clone());
    }
    recipients.sort();
    recipients.dedup();

    let content = ticket_notification_content(ticket_id, current_state);
    for recipient in recipients {
        if source.as_ref().is_some_and(|source| source == &recipient) {
            continue;
        }
        let result = api.runtime.send_input(
            &recipient,
            WorkerInputRequest {
                kind: WorkerInputKind::Notify,
                content: content.clone(),
                segments: None,
            },
        );
        if let Some(error_category) = ticket_notification_delivery_error_category(&result) {
            emit_ticket_notification_delivery_warning(TicketNotificationDeliveryWarning::new(
                workspace_id,
                ticket_id,
                current_state,
                &recipient,
                error_category,
            ));
        }
    }
}

fn authenticate_worker_mutation_source(
    api: &WorkspaceApi,
    workspace_id: &str,
    headers: &HeaderMap,
) -> Result<WorkerMutationSource> {
    let runtime_id = headers
        .get("x-yoi-runtime-id")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| Error::WorkerSourceIdentity("missing Runtime id".to_string()))?;
    let worker_id = headers
        .get("x-yoi-worker-id")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            Error::WorkerSourceIdentity("missing Runtime-bound Worker id".to_string())
        })?;
    let worker = RuntimeWorkerRef::new(runtime_id, worker_id);
    let summary = api.runtime.worker(&worker).map_err(|_| {
        Error::WorkerSourceIdentity("Runtime-bound Worker identity does not exist".to_string())
    })?;
    if summary.workspace.workspace_id.as_deref() != Some(workspace_id) {
        return Err(Error::WorkerSourceIdentity(format!(
            "Runtime-bound Worker is not scoped to Workspace {workspace_id}"
        )));
    }
    Ok(worker)
}

fn current_worker_identity(
    api: &WorkspaceApi,
    workspace_id: &str,
    headers: &HeaderMap,
) -> Result<RuntimeWorkerRef> {
    authenticate_worker_mutation_source(api, workspace_id, headers)
}

fn current_worker_active_attachment(
    api: &WorkspaceApi,
    worker: &RuntimeWorkerRef,
) -> ApiResult<WorkerWorkdirLinkRecord> {
    if let Some(link) = api
        .store
        .list_worker_workdir_links(&api.config.workspace_id, worker)?
        .into_iter()
        .next()
    {
        return Ok(link);
    }

    if api
        .store
        .worker_workdir_link_history_exists(&api.config.workspace_id, worker)?
    {
        return Err(Error::WorkdirAttachmentConflict(format!(
            "Worker {}:{} has no active Workdir attachment",
            worker.runtime_id, worker.worker_id
        ))
        .into());
    }

    // A Runtime can start the Worker immediately after reserving its local binding, before the
    // outer spawn handler has projected that binding into the Backend registry. Import the same
    // binding transactionally on the first identity-bound operation so initial input cannot race
    // attachment authority.
    let observed_worker = api
        .runtime
        .worker(worker)
        .map_err(|error| error.into_error())?;
    if observed_worker.working_directory.is_some() {
        sync_worker_observation(api, &observed_worker)?;
        if let Some(link) = api
            .store
            .list_worker_workdir_links(&api.config.workspace_id, worker)?
            .into_iter()
            .next()
        {
            return Ok(link);
        }
    }
    Err(Error::WorkdirAttachmentConflict(format!(
        "Worker {}:{} has no active Workdir attachment",
        worker.runtime_id, worker.worker_id
    ))
    .into())
}

fn current_worker_session_lock(
    api: &WorkspaceApi,
    worker: &RuntimeWorkerRef,
) -> Arc<tokio::sync::Mutex<()>> {
    api.workdir_session_locks
        .lock()
        .expect("Workdir session lock registry poisoned")
        .entry(worker.clone())
        .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
        .clone()
}

fn runtime_local_owner_worker_id<'a>(
    caller: &'a RuntimeWorkerRef,
    target_runtime_id: &str,
) -> Option<&'a str> {
    (caller.runtime_id == target_runtime_id).then_some(caller.worker_id.as_str())
}

async fn open_current_worker_workdir_session_locked(
    api: &WorkspaceApi,
    worker: &RuntimeWorkerRef,
    link: &WorkerWorkdirLinkRecord,
) -> Result<WorkdirSessionHandle> {
    if let Some(session) = api
        .workdir_sessions
        .lock()
        .expect("Workdir session registry lock poisoned")
        .get(worker)
        .cloned()
    {
        return Ok(session);
    }
    let workdir = api
        .store
        .list_workdir_registry(&api.config.workspace_id, 10_000)?
        .into_iter()
        .find(|workdir| workdir.workdir_id == link.workdir_id)
        .ok_or_else(|| Error::RuntimeOperationFailed {
            runtime_id: worker.runtime_id.clone(),
            code: "working_directory_not_found".to_string(),
            message: format!(
                "attached Workdir {} is not registered in this Workspace",
                link.workdir_id
            ),
        })?;
    let owner_worker_id = runtime_local_owner_worker_id(worker, &workdir.runtime_id);
    let session = api
        .runtime
        .open_workdir_session(&workdir.runtime_id, &workdir.workdir_id, owner_worker_id)
        .await
        .map_err(|error| error.into_error())?;
    let key = worker.clone();
    let (selected, unused) = {
        let mut sessions = api
            .workdir_sessions
            .lock()
            .expect("Workdir session registry lock poisoned");
        match sessions.entry(key) {
            std::collections::hash_map::Entry::Occupied(entry) => {
                (entry.get().clone(), Some(session))
            }
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(session.clone());
                (session, None)
            }
        }
    };
    if let Some(unused) = unused {
        unused
            .close()
            .await
            .map_err(|error| Error::RuntimeOperationFailed {
                runtime_id: worker.runtime_id.clone(),
                code: "duplicate_workdir_session_close_failed".to_string(),
                message: error.to_string(),
            })?;
    }
    Ok(selected)
}

async fn close_current_worker_session_locked(
    api: &WorkspaceApi,
    worker: &RuntimeWorkerRef,
) -> Result<()> {
    let key = worker.clone();
    let session = api
        .workdir_sessions
        .lock()
        .expect("Workdir session registry lock poisoned")
        .get(&key)
        .cloned();
    if let Some(session) = session {
        session
            .close()
            .await
            .map_err(|error| Error::RuntimeOperationFailed {
                runtime_id: worker.runtime_id.clone(),
                code: "workdir_session_close_failed".to_string(),
                message: error.to_string(),
            })?;
        api.workdir_sessions
            .lock()
            .expect("Workdir session registry lock poisoned")
            .remove(&key);
    }
    Ok(())
}

async fn scoped_attach_current_worker_workdir(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedWorkspacePath>,
    headers: HeaderMap,
    Json(request): Json<AttachCurrentWorkerWorkdirRequest>,
) -> ApiResult<Json<CurrentWorkerWorkdirAttachmentResponse>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    let worker = current_worker_identity(&api, &path.workspace_id, &headers)?;
    let workdir_id = request.workdir_id.trim();
    if workdir_id.is_empty() || workdir_id.chars().any(char::is_control) {
        return Err(Error::InvalidRecordId(request.workdir_id).into());
    }
    if !api
        .store
        .list_workdir_registry(&api.config.workspace_id, 10_000)?
        .iter()
        .any(|workdir| workdir.workdir_id == workdir_id)
    {
        return Err(Error::RuntimeOperationFailed {
            runtime_id: worker.runtime_id.clone(),
            code: "working_directory_not_found".to_string(),
            message: format!("unknown Workdir `{workdir_id}`"),
        }
        .into());
    }
    let session_lock = current_worker_session_lock(&api, &worker);
    let _session_guard = session_lock.lock().await;
    let link = api.store.attach_worker_workdir(&WorkerWorkdirLinkRecord {
        workspace_id: api.config.workspace_id.clone(),
        worker: worker.clone(),
        workdir_id: workdir_id.to_string(),
        role: "attachment".to_string(),
        linked_at: Utc::now().to_rfc3339_opts(SecondsFormat::Nanos, true),
        unlinked_at: None,
    })?;
    if let Err(error) = open_current_worker_workdir_session_locked(&api, &worker, &link).await {
        let _ = api.store.detach_worker_workdir(
            &api.config.workspace_id,
            &worker,
            Some(workdir_id),
            &Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        );
        return Err(error.into());
    }
    Ok(Json(CurrentWorkerWorkdirAttachmentResponse {
        workspace_id: api.config.workspace_id.clone(),
        workdir_id: workdir_id.to_string(),
        attached: true,
    }))
}

async fn scoped_detach_current_worker_workdir(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedWorkspacePath>,
    headers: HeaderMap,
) -> ApiResult<Json<CurrentWorkerWorkdirAttachmentResponse>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    let worker = current_worker_identity(&api, &path.workspace_id, &headers)?;
    let session_lock = current_worker_session_lock(&api, &worker);
    let _session_guard = session_lock.lock().await;
    let link = current_worker_active_attachment(&api, &worker)?;
    close_current_worker_session_locked(&api, &worker).await?;
    api.store.detach_worker_workdir(
        &api.config.workspace_id,
        &worker,
        Some(&link.workdir_id),
        &Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
    )?;
    Ok(Json(CurrentWorkerWorkdirAttachmentResponse {
        workspace_id: api.config.workspace_id.clone(),
        workdir_id: link.workdir_id,
        attached: false,
    }))
}

async fn scoped_current_worker_workdir_session_fence(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedWorkspacePath>,
    headers: HeaderMap,
) -> ApiResult<Json<WorkspaceWorkdirSessionFence>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    let worker = current_worker_identity(&api, &path.workspace_id, &headers)?;
    let session_lock = current_worker_session_lock(&api, &worker);
    let _session_guard = session_lock.lock().await;
    let link = current_worker_active_attachment(&api, &worker)?;
    Ok(Json(WorkspaceWorkdirSessionFence {
        value: current_worker_workdir_session_fence(&link),
    }))
}

fn current_worker_workdir_session_fence(link: &WorkerWorkdirLinkRecord) -> String {
    format!("v1:{}\0{}", link.workdir_id, link.linked_at)
}

fn validate_current_worker_workdir_session_fence(
    link: &WorkerWorkdirLinkRecord,
    expected: Option<&str>,
) -> Result<()> {
    if expected.is_some_and(|expected| expected != current_worker_workdir_session_fence(link)) {
        Err(Error::WorkdirAttachmentConflict(
            "delegated Workdir session attachment changed".to_string(),
        ))
    } else {
        Ok(())
    }
}

async fn scoped_execute_current_worker_workdir_operation(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedWorkspacePath>,
    headers: HeaderMap,
    Json(request): Json<WorkspaceWorkdirSessionOperationRequest>,
) -> ApiResult<Json<WorkdirSessionOperationResult>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    let worker = current_worker_identity(&api, &path.workspace_id, &headers)?;
    let session_lock = current_worker_session_lock(&api, &worker);
    let _session_guard = session_lock.lock().await;
    let link = current_worker_active_attachment(&api, &worker)?;
    validate_current_worker_workdir_session_fence(
        &link,
        request.expected_session_fence.as_deref(),
    )?;
    let source = open_current_worker_workdir_session_locked(&api, &worker, &link).await?;
    let applied = workdir::apply_delegation_chain(source, request.delegations)
        .await
        .map_err(|error| Error::RuntimeOperationFailed {
            runtime_id: worker.runtime_id.clone(),
            code: "workdir_session_delegation_failed".to_string(),
            message: error.to_string(),
        })?;
    let result = execute_workdir_session_operation(&applied.scoped_session, request.operation)
        .await
        .map_err(|error| Error::RuntimeOperationFailed {
            runtime_id: worker.runtime_id.clone(),
            code: "workdir_session_operation_failed".to_string(),
            message: error.to_string(),
        })?;
    Ok(Json(result))
}

async fn execute_workdir_session_operation(
    session: &WorkdirSessionHandle,
    operation: WorkdirSessionOperation,
) -> std::result::Result<WorkdirSessionOperationResult, workdir::WorkdirError> {
    match operation {
        WorkdirSessionOperation::Stat(request) => session
            .stat(request)
            .await
            .map(WorkdirSessionOperationResult::Stat),
        WorkdirSessionOperation::Read(request) => session
            .read(request)
            .await
            .map(WorkdirSessionOperationResult::Read),
        WorkdirSessionOperation::Write(request) => session
            .write(request)
            .await
            .map(WorkdirSessionOperationResult::Write),
        WorkdirSessionOperation::Edit(request) => session
            .edit(request)
            .await
            .map(WorkdirSessionOperationResult::Edit),
        WorkdirSessionOperation::List(request) => session
            .list(request)
            .await
            .map(WorkdirSessionOperationResult::List),
        WorkdirSessionOperation::Glob(request) => session
            .glob(request)
            .await
            .map(WorkdirSessionOperationResult::Glob),
        WorkdirSessionOperation::Grep(request) => session
            .grep(request)
            .await
            .map(WorkdirSessionOperationResult::Grep),
        WorkdirSessionOperation::CommandStart(request) => session
            .start_command(request)
            .await
            .map(WorkdirSessionOperationResult::CommandStart),
        WorkdirSessionOperation::CommandStatus(handle) => session
            .command_status(handle)
            .await
            .map(WorkdirSessionOperationResult::CommandStatus),
        WorkdirSessionOperation::CommandOutput(request) => session
            .command_output(request)
            .await
            .map(WorkdirSessionOperationResult::CommandOutput),
        WorkdirSessionOperation::CommandCancel(handle) => session
            .cancel_command(handle)
            .await
            .map(|()| WorkdirSessionOperationResult::CommandCancel),
    }
}

async fn run_orchestrator_turn_end_hook(api: WorkspaceApi) {
    let Ok(mut subscription) = api.runtime_subscription_broker.subscribe(
        EMBEDDED_WORKER_RUNTIME_ID,
        protocol::subscription::EventSubscriptionSelector::RuntimeWorkers,
    ) else {
        return;
    };
    let mut worker_states = HashMap::new();
    while let Some(update) = subscription.recv().await {
        match update {
            crate::runtime_subscription::BrokerSubscriptionEvent::Snapshot { snapshot, .. } => {
                if let protocol::subscription::SubscriptionSnapshot::Workers { workers } = snapshot
                {
                    worker_states.clear();
                    for worker in workers {
                        let worker_id = worker.worker_id.to_string();
                        maybe_dispatch_orchestrator_turn_end(&api, &worker_id, None, worker.state);
                        worker_states.insert(worker_id, worker.state);
                    }
                }
            }
            crate::runtime_subscription::BrokerSubscriptionEvent::Event { payload, .. } => {
                match payload {
                    protocol::subscription::SubscriptionEventPayload::WorkerUpserted { worker } => {
                        let worker_id = worker.worker_id.to_string();
                        let previous = worker_states.insert(worker_id.clone(), worker.state);
                        maybe_dispatch_orchestrator_turn_end(
                            &api,
                            &worker_id,
                            previous,
                            worker.state,
                        );
                    }
                    protocol::subscription::SubscriptionEventPayload::WorkerRemoved {
                        worker_id,
                        ..
                    } => {
                        worker_states.remove(worker_id.as_str());
                    }
                    _ => {}
                }
            }
            crate::runtime_subscription::BrokerSubscriptionEvent::Disconnected { .. } => {
                worker_states.clear();
            }
            crate::runtime_subscription::BrokerSubscriptionEvent::Rejected { .. }
            | crate::runtime_subscription::BrokerSubscriptionEvent::Closed { .. } => return,
        }
    }
}

fn maybe_dispatch_orchestrator_turn_end(
    api: &WorkspaceApi,
    worker_id: &str,
    previous: Option<protocol::subscription::SubscriptionWorkerState>,
    current: protocol::subscription::SubscriptionWorkerState,
) {
    use protocol::subscription::SubscriptionWorkerState;

    if current != SubscriptionWorkerState::Idle
        || !matches!(
            previous,
            None | Some(SubscriptionWorkerState::Running)
                | Some(SubscriptionWorkerState::Stopped)
                | Some(SubscriptionWorkerState::Paused)
        )
    {
        return;
    }
    let Some(orchestrator) = find_workspace_orchestrator(api) else {
        return;
    };
    if orchestrator.worker.runtime_id == EMBEDDED_WORKER_RUNTIME_ID
        && orchestrator.worker.worker_id == worker_id
    {
        dispatch_orchestrator_queue_attention(api);
    }
}

fn dispatch_orchestrator_queue_attention(api: &WorkspaceApi) {
    let Some(orchestrator) = find_workspace_orchestrator(api) else {
        return;
    };
    let Ok(backend) = browser_ticket_backend(api) else {
        return;
    };
    let Ok(mut queued) = backend.list(ticket::TicketListQuery::states([
        ticket::TicketListState::Queued,
    ])) else {
        return;
    };
    queued.retain(|ticket| {
        orchestrator_interested(api, &api.config.workspace_id, &ticket.id, "queued")
            .ok()
            .flatten()
            .is_some()
    });
    let Ok(mut inprogress) = backend.list(ticket::TicketListQuery::states([
        ticket::TicketListState::InProgress,
    ])) else {
        return;
    };
    inprogress.retain(|ticket| {
        orchestrator_interested(api, &api.config.workspace_id, &ticket.id, "inprogress")
            .ok()
            .flatten()
            .is_some()
    });
    queued.extend(inprogress);
    queued.sort_by(|left, right| left.id.cmp(&right.id));
    if queued.is_empty() {
        *api.orchestrator_attention_fingerprint
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        return;
    }
    let fingerprint = queued
        .iter()
        .map(|ticket| ticket.id.as_str())
        .collect::<Vec<_>>()
        .join("|");
    if api
        .orchestrator_attention_fingerprint
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .as_deref()
        == Some(fingerprint.as_str())
    {
        return;
    }

    let shown = queued
        .iter()
        .take(ORCHESTRATOR_ATTENTION_TICKET_LIMIT)
        .map(|ticket| {
            format!(
                "- {} — {}",
                bounded_orchestrator_attention_text(&ticket.id, 80),
                bounded_orchestrator_attention_text(&ticket.title, 240)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let omitted = queued
        .len()
        .saturating_sub(ORCHESTRATOR_ATTENTION_TICKET_LIMIT);
    let omitted_line = if omitted == 0 {
        String::new()
    } else {
        format!("Additional queued Tickets omitted from this notice: {omitted}\n")
    };
    let Ok(Some(config_state)) = api
        .config_store
        .load_workspace_config(&api.config.workspace_id)
    else {
        return;
    };
    let Ok(projection) = api
        .prompt_projection_cache
        .resolve(&api.config.workspace_id, &config_state)
    else {
        return;
    };
    let Ok(catalog) = worker::PromptCatalog::from_projection(projection.catalog().clone()) else {
        return;
    };
    let content = match catalog.render_serializable(
        ORCHESTRATOR_ATTENTION_PROMPT_NAME,
        &BTreeMap::from([
            ("omitted_line", omitted_line.as_str()),
            ("workspace_id", api.config.workspace_id.as_str()),
            ("ticket_lines", shown.as_str()),
        ]),
    ) {
        Ok(content) => content,
        Err(_) => return,
    };
    let accepted = api
        .runtime
        .send_input(
            &orchestrator.worker,
            WorkerInputRequest {
                kind: WorkerInputKind::Notify,
                content,
                segments: None,
            },
        )
        .is_ok_and(|result| result.state == WorkerOperationState::Accepted);
    if accepted {
        *api.orchestrator_attention_fingerprint
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(fingerprint);
    }
}

fn bounded_orchestrator_attention_text(input: &str, max_chars: usize) -> String {
    let mut output = String::new();
    for (index, character) in input.chars().enumerate() {
        if index == max_chars {
            output.push('…');
            break;
        }
        output.push(if character.is_control() {
            ' '
        } else {
            character
        });
    }
    output
}

fn require_online_workspace_orchestrator_source(
    api: &WorkspaceApi,
    source: &WorkerMutationSource,
) -> Result<()> {
    let orchestrator = find_online_workspace_orchestrator(api).ok_or_else(|| {
        Error::TicketAssignmentConflict(
            "Workspace has no current online Workspace Orchestrator".into(),
        )
    })?;
    if orchestrator.worker != *source {
        return Err(Error::TicketAssignmentConflict(
            "Merge Request completion requires the current online Workspace Orchestrator".into(),
        ));
    }
    Ok(())
}

fn find_online_workspace_orchestrator(api: &WorkspaceApi) -> Option<WorkerSummary> {
    api.runtime
        .list_workers(1000)
        .items
        .into_iter()
        .find(|worker| {
            worker.singleton_key.as_deref()
                == Some(crate::hosts::WORKSPACE_ORCHESTRATOR_SINGLETON_KEY)
                && worker.workspace.workspace_id.as_deref()
                    == Some(api.config.workspace_id.as_str())
                && matches!(worker.state.as_str(), "idle" | "running" | "paused")
        })
}

fn find_workspace_orchestrator(api: &WorkspaceApi) -> Option<WorkerSummary> {
    let is_orchestrator = |worker: &WorkerSummary| {
        worker.singleton_key.as_deref() == Some(crate::hosts::WORKSPACE_ORCHESTRATOR_SINGLETON_KEY)
    };
    if let Some(worker) = find_online_workspace_orchestrator(api) {
        return Some(worker);
    }
    for runtime in api.runtime.list_runtimes(1000).items {
        if let Ok(stopped) = api
            .runtime
            .list_stopped_workers_for_runtime(&runtime.runtime_id, 1000)
        {
            if let Some(worker) = stopped.items.into_iter().find(is_orchestrator) {
                return Some(worker);
            }
        }
    }
    None
}

#[derive(Debug, Clone, Serialize)]
struct MemoryDocumentResponse {
    body_md: String,
    created_at: String,
    updated_at: String,
    bytes: usize,
    record_source: String,
}

async fn scoped_get_memory_document(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedWorkspacePath>,
) -> ApiResult<Json<MemoryDocumentResponse>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    let document = api.authority.memory_document()?;
    Ok(Json(MemoryDocumentResponse {
        bytes: document.body_md.len(),
        body_md: document.body_md,
        created_at: document.created_at,
        updated_at: document.updated_at,
        record_source: document.record_source,
    }))
}

async fn scoped_list_memory_staging(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedWorkspacePath>,
    Query(query): Query<MemoryStagingQuery>,
) -> ApiResult<Json<MemoryStagingListResponse>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    Ok(Json(list_memory_staging_from_authority(
        &api.authority,
        query.limit,
    )?))
}

async fn scoped_memory_backend_operation(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedWorkspacePath>,
    Json(operation): Json<MemoryBackendOperation>,
) -> ApiResult<Json<MemoryBackendHttpResponse>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    let response = match execute_memory_backend_operation_with_authority(&api.authority, operation)
    {
        Ok(result) => MemoryBackendHttpResponse::Ok { result },
        Err(error) => MemoryBackendHttpResponse::Error {
            message: sanitize_backend_error(&error.to_string()),
        },
    };
    Ok(Json(response))
}

const MEMORY_CONSOLIDATION_PROFILE: &str = "memory-consolidation";
const MEMORY_CONSOLIDATION_SINGLETON_KEY: &str = "workspace-memory-consolidation";
const MEMORY_CONSOLIDATION_WORKER_SCAN_LIMIT: usize = 100;

async fn scoped_memory_consolidation(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedWorkspacePath>,
    Json(operation): Json<MemoryConsolidateStagingOperation>,
) -> ApiResult<Json<MemoryConsolidationOutput>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    Ok(Json(start_memory_staging_consolidation(api, operation)?))
}

fn start_memory_staging_consolidation(
    api: WorkspaceApi,
    operation: MemoryConsolidateStagingOperation,
) -> ApiResult<MemoryConsolidationOutput> {
    let backlog = memory_staging_backlog_from_authority(&api.authority)?;
    let candidate_count = backlog.candidate_count;
    let total_bytes = backlog.total_bytes;
    if candidate_count == 0 {
        return Ok(MemoryConsolidationOutput {
            status: "skipped_empty".to_string(),
            summary: "No Memory staging candidates are pending.".to_string(),
            candidate_count,
            total_bytes,
        });
    }
    let reached_files = operation
        .threshold_files
        .is_some_and(|threshold| candidate_count >= threshold);
    let reached_bytes = operation
        .threshold_bytes
        .is_some_and(|threshold| total_bytes >= threshold);
    if !operation.force && !reached_files && !reached_bytes {
        return Ok(MemoryConsolidationOutput {
            status: "skipped_below_threshold".to_string(),
            summary: format!(
                "Memory staging backlog has {candidate_count} candidate(s), {total_bytes} byte(s), below configured threshold."
            ),
            candidate_count,
            total_bytes,
        });
    }

    let runtime_id = select_memory_consolidation_runtime(&api)?;
    let input_content = memory_consolidation_input_content(candidate_count, total_bytes);
    if let Some(output) = try_reuse_memory_consolidation_worker(
        &api,
        &runtime_id,
        &input_content,
        candidate_count,
        total_bytes,
    )? {
        return Ok(output);
    }

    let profile_selector = ProfileSelector::Builtin(MEMORY_CONSOLIDATION_PROFILE.to_string());
    let resolved_config_bundle = None;
    let initial_submit = vec![Segment::text(input_content)];
    let result = api.spawn_workspace_worker(
        &runtime_id,
        WorkerSpawnRequest {
            requested_worker_name: Some(MEMORY_CONSOLIDATION_PROFILE.to_string()),
            intent: WorkerSpawnIntent::WorkspaceOrchestrator,
            acceptance: WorkerSpawnAcceptanceRequirement::RunAccepted {
                expected_segments: 1,
            },
            profile: profile_selector,
            ticket_assignment: None,
            initial_submit,
            working_directory_request: None,
            resolved_working_directory_request: None,
            resolved_working_directory: None,
            resolved_config_bundle,
            resolved_worker_observation_enabled: false,
            resolved_worker_observation_grants: Vec::new(),
            resolved_control_operation: None,
            resolved_workspace_api: None,
            resolved_memory_settings: None,
        },
    )?;
    if result.state != WorkerOperationState::Accepted {
        return Ok(MemoryConsolidationOutput {
            status: "skipped_spawn_rejected".to_string(),
            summary: "Runtime rejected Memory consolidater spawn.".to_string(),
            candidate_count,
            total_bytes,
        });
    }
    let Some(worker) = result.worker else {
        return Ok(MemoryConsolidationOutput {
            status: "skipped_spawn_missing_worker".to_string(),
            summary:
                "Runtime accepted Memory consolidater spawn without returning a Worker summary."
                    .to_string(),
            candidate_count,
            total_bytes,
        });
    };
    Ok(MemoryConsolidationOutput {
        status: "started".to_string(),
        summary: format!(
            "Started Memory consolidater '{}' for {candidate_count} staging candidate(s).",
            worker.worker.worker_id
        ),
        candidate_count,
        total_bytes,
    })
}

fn try_reuse_memory_consolidation_worker(
    api: &WorkspaceApi,
    runtime_id: &str,
    input_content: &str,
    candidate_count: usize,
    total_bytes: u64,
) -> ApiResult<Option<MemoryConsolidationOutput>> {
    let workers = api
        .runtime
        .list_workers_for_runtime(runtime_id, MEMORY_CONSOLIDATION_WORKER_SCAN_LIMIT)
        .map_err(|err| err.into_error())?;
    let mut consolidaters = workers
        .items
        .into_iter()
        .filter(is_memory_consolidation_worker)
        .collect::<Vec<_>>();
    if consolidaters.is_empty() {
        return Ok(None);
    }
    consolidaters.sort_by(|a, b| a.worker.worker_id.cmp(&b.worker.worker_id));
    if let Some(worker) = consolidaters.iter().find(|worker| worker.state != "idle") {
        return Ok(Some(MemoryConsolidationOutput {
            status: "skipped_existing_not_idle".to_string(),
            summary: format!(
                "Existing Memory consolidater '{}' is '{}', not confirmed idle.",
                worker.worker.worker_id, worker.state
            ),
            candidate_count,
            total_bytes,
        }));
    }
    let worker = consolidaters
        .first()
        .expect("non-empty consolidater list")
        .clone();
    let input = api
        .runtime
        .send_input(
            &worker.worker,
            WorkerInputRequest {
                kind: WorkerInputKind::User,
                content: input_content.to_string(),
                segments: None,
            },
        )
        .map_err(|err| err.into_error())?;
    if input.state != WorkerOperationState::Accepted {
        return Ok(Some(MemoryConsolidationOutput {
            status: "skipped_existing_input_rejected".to_string(),
            summary: format!(
                "Existing idle Memory consolidater '{}' rejected the new consolidation input.",
                worker.worker.worker_id
            ),
            candidate_count,
            total_bytes,
        }));
    }
    Ok(Some(MemoryConsolidationOutput {
        status: "reused".to_string(),
        summary: format!(
            "Reused Memory consolidater '{}' for {candidate_count} staging candidate(s).",
            worker.worker.worker_id
        ),
        candidate_count,
        total_bytes,
    }))
}

fn is_memory_consolidation_worker(worker: &WorkerSummary) -> bool {
    worker.singleton_key.as_deref() == Some(MEMORY_CONSOLIDATION_SINGLETON_KEY)
        || worker.profile.as_deref() == Some(MEMORY_CONSOLIDATION_PROFILE)
}

fn memory_consolidation_input_content(candidate_count: usize, total_bytes: u64) -> String {
    format!(
        "Process pending Memory staging candidates through MemoryStagingList, MemoryStagingRead, Memory tools, and MemoryStagingClose. Current backlog: {candidate_count} candidate(s), {total_bytes} byte(s)."
    )
}

fn select_memory_consolidation_runtime(api: &WorkspaceApi) -> ApiResult<String> {
    let runtimes = api.runtime.list_runtimes(100);
    if let Some(runtime) = runtimes
        .items
        .iter()
        .find(|runtime| {
            runtime.runtime_id == EMBEDDED_WORKER_RUNTIME_ID
                && runtime.capabilities.can_spawn_worker
        })
        .or_else(|| {
            runtimes
                .items
                .iter()
                .find(|runtime| runtime.capabilities.can_spawn_worker)
        })
    {
        return Ok(runtime.runtime_id.clone());
    }
    Err(Error::RuntimeOperationFailed {
        runtime_id: "memory-consolidation".to_string(),
        code: "memory_consolidation_runtime_unavailable".to_string(),
        message: "No runtime capable of spawning the Memory consolidater is available".to_string(),
    }
    .into())
}

async fn scoped_list_skills(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedWorkspacePath>,
) -> ApiResult<Json<worker::skill::SkillCatalogResponse>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    let state = api
        .config_store
        .load_workspace_config(&path.workspace_id)?
        .ok_or_else(|| {
            Error::RegistryInconsistency(format!(
                "Workspace {} has no active config revision",
                path.workspace_id
            ))
        })?;
    skills::catalog(&state).map(Json).map_err(skill_api_error)
}

async fn scoped_lint_skills(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedWorkspacePath>,
) -> ApiResult<Json<worker::skill::SkillCatalogResponse>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    let state = api
        .config_store
        .load_workspace_config(&path.workspace_id)?
        .ok_or_else(|| {
            Error::RegistryInconsistency(format!(
                "Workspace {} has no active config revision",
                path.workspace_id
            ))
        })?;
    skills::lint(&state).map(Json).map_err(skill_api_error)
}

async fn scoped_get_skill(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedSkillPath>,
) -> ApiResult<Json<worker::skill::SkillDetailResponse>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    let state = api
        .config_store
        .load_workspace_config(&path.workspace_id)?
        .ok_or_else(|| {
            Error::RegistryInconsistency(format!(
                "Workspace {} has no active config revision",
                path.workspace_id
            ))
        })?;
    skills::detail(&state, &path.name)
        .map(Json)
        .map_err(skill_api_error)
}

async fn scoped_activate_skill(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedSkillPath>,
) -> ApiResult<Json<worker::skill::SkillActivationResponse>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    let state = api
        .config_store
        .load_workspace_config(&path.workspace_id)?
        .ok_or_else(|| {
            Error::RegistryInconsistency(format!(
                "Workspace {} has no active config revision",
                path.workspace_id
            ))
        })?;
    skills::activation(&state, &path.name)
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
        skills::SkillError::InvalidSkill(name) => ApiError::from(Error::InvalidInput(format!(
            "Skill `{name}` has blocking diagnostics"
        ))),
        error => ApiError::from(Error::RuntimeOperationFailed {
            runtime_id: "workspace".to_string(),
            code: "skill_projection_failed".to_string(),
            message: error.to_string(),
        }),
    }
}

async fn scoped_list_objectives(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedWorkspacePath>,
    Query(query): Query<ObjectiveListQuery>,
) -> ApiResult<Json<ListResponse<crate::records::ObjectiveSummary>>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    list_objectives(State(api), Query(query)).await
}

async fn scoped_get_objective(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedObjectivePath>,
) -> ApiResult<Json<ObjectiveDetail>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    get_objective(State(api), AxumPath(path.objective_id)).await
}

async fn scoped_query_objectives(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedWorkspacePath>,
    Json(query): Json<ObjectiveQueryRequest>,
) -> ApiResult<Json<ObjectiveQueryResponse>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    Ok(Json(api.authority.query_objectives(query)?))
}

async fn scoped_show_objective(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedObjectivePath>,
    Json(query): Json<ObjectiveShowRequest>,
) -> ApiResult<Json<ObjectiveDetail>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    Ok(Json(
        api.authority.show_objective(&path.objective_id, query)?,
    ))
}

async fn scoped_create_objective(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedWorkspacePath>,
    Json(request): Json<ObjectiveCreateRequest>,
) -> ApiResult<Json<ObjectiveDetail>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    Ok(Json(api.authority.create_objective(
        ObjectiveCreateInput {
            title: request.title,
            body_md: request.body_md,
            state: request.state,
            linked_tickets: request.linked_tickets,
        },
    )?))
}

async fn scoped_edit_objective(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedObjectivePath>,
    Json(request): Json<ObjectiveEditRequest>,
) -> ApiResult<Json<ObjectiveDetail>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    Ok(Json(api.authority.edit_objective(
        &path.objective_id,
        ObjectiveEditInput {
            title: request.title,
            old_string: request.old_string,
            new_string: request.new_string,
            replace_all: request.replace_all,
        },
    )?))
}

async fn scoped_set_objective_state(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedObjectivePath>,
    Json(request): Json<ObjectiveStateRequest>,
) -> ApiResult<Json<ObjectiveDetail>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    Ok(Json(
        api.authority
            .set_objective_state(&path.objective_id, &request.state)?,
    ))
}

async fn scoped_link_objective_ticket(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedObjectivePath>,
    Json(request): Json<ObjectiveLinkTicketRequest>,
) -> ApiResult<Json<ObjectiveDetail>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    Ok(Json(api.authority.link_objective_ticket(
        &path.objective_id,
        &request.ticket_id,
    )?))
}

async fn scoped_unlink_objective_ticket(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedObjectiveTicketPath>,
) -> ApiResult<Json<ObjectiveDetail>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    Ok(Json(api.authority.unlink_objective_ticket(
        &path.objective_id,
        &path.ticket_id,
    )?))
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
) -> ApiResult<Json<workspace_api::ListResponse<workspace_api::RuntimeSummary>>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    list_runtimes(State(api)).await
}

async fn scoped_workspace_protocol_ws(
    State(api): State<WorkspaceApi>,
    AxumPath(workspace_id): AxumPath<String>,
    ws: axum::extract::ws::WebSocketUpgrade,
) -> std::result::Result<Response, Response> {
    validate_workspace_scope(&api, &workspace_id).map_err(|error| error.into_response())?;
    Ok(ws
        .on_upgrade(move |socket| {
            crate::workspace_subscription::serve_workspace_subscription(api, socket)
        })
        .into_response())
}

#[derive(Debug, Serialize)]
struct WorkerRemoveSuccessResponse<'a> {
    removed: bool,
    runtime_id: &'a str,
    worker_id: &'a str,
}

#[derive(Debug, Serialize)]
struct WorkerRemoveErrorResponse<'a> {
    code: &'a str,
    message: &'a str,
}

fn worker_remove_success_response(worker: &RuntimeWorkerRef) -> worker::WorkspaceResponse {
    let body = serde_json::to_string(&WorkerRemoveSuccessResponse {
        removed: true,
        runtime_id: &worker.runtime_id,
        worker_id: &worker.worker_id,
    })
    .unwrap_or_else(|_| r#"{"removed":true}"#.to_string());
    worker::WorkspaceResponse {
        status: StatusCode::OK.as_u16(),
        body,
    }
}

fn worker_remove_error_response(
    status: StatusCode,
    code: &str,
    message: &str,
) -> worker::WorkspaceResponse {
    let body =
        serde_json::to_string(&WorkerRemoveErrorResponse { code, message }).unwrap_or_else(|_| {
            r#"{"code":"worker_remove_failed","message":"Worker removal failed"}"#.to_string()
        });
    worker::WorkspaceResponse {
        status: status.as_u16(),
        body,
    }
}

fn worker_retention_error_response(
    error: crate::retention::WorkerRetentionError,
) -> worker::WorkspaceResponse {
    match error {
        crate::retention::WorkerRetentionError::WorkerNotFound
        | crate::retention::WorkerRetentionError::CrossWorkspace => worker_remove_error_response(
            StatusCode::NOT_FOUND,
            "worker_not_found",
            "Worker was not found in this Workspace",
        ),
        crate::retention::WorkerRetentionError::PolicyRevisionConflict { .. }
        | crate::retention::WorkerRetentionError::StalePlan { .. }
        | crate::retention::WorkerRetentionError::OperationFingerprintConflict { .. } => {
            worker_remove_error_response(
                StatusCode::CONFLICT,
                "worker_removal_conflict",
                "Worker removal state changed; retry the operation",
            )
        }
        crate::retention::WorkerRetentionError::Blocked(_) => worker_remove_error_response(
            StatusCode::CONFLICT,
            "worker_removal_blocked",
            "Worker removal is blocked by current assignment, hold, or retention policy",
        ),
        crate::retention::WorkerRetentionError::Invalid(_) => worker_remove_error_response(
            StatusCode::BAD_REQUEST,
            "invalid_worker_remove",
            "Worker removal request is invalid",
        ),
        crate::retention::WorkerRetentionError::PolicyMissing { .. }
        | crate::retention::WorkerRetentionError::Store(_) => worker_remove_error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "worker_removal_authority_unavailable",
            "Worker removal authority is unavailable; removal can be retried",
        ),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkerRemoveBoundaryRequest {
    target_runtime_id: String,
    target_worker_id: String,
    reason: String,
}

async fn scoped_worker_remove_source_boundary(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedWorkspacePath>,
    headers: HeaderMap,
    Json(request): Json<WorkerRemoveBoundaryRequest>,
) -> Response {
    if let Err(error) = validate_workspace_scope(&api, &path.workspace_id) {
        return error.into_response();
    }
    let proof = match crate::worker_source::presented_worker_remove_source(&headers, None) {
        Ok(proof) => proof,
        Err(error) => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({ "error": error.to_string() })),
            )
                .into_response();
        }
    };
    match crate::worker_source::verify_worker_remove_source(
        &api,
        proof,
        &request.target_runtime_id,
        &request.target_worker_id,
    )
    .await
    {
        Ok(source) => {
            let executor = WorkspaceWorkerRemoveExecutor::new(&api);
            match executor
                .execute_async(
                    source,
                    &request.target_runtime_id,
                    &request.target_worker_id,
                    &request.reason,
                )
                .await
            {
                Ok(response) => (
                    StatusCode::from_u16(response.status)
                        .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                    [(CONTENT_TYPE, "application/json")],
                    response.body,
                )
                    .into_response(),
                Err(_) => {
                    let response = worker_remove_error_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "worker_remove_failed",
                        "Worker removal failed before lifecycle execution",
                    );
                    (
                        StatusCode::from_u16(response.status)
                            .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                        [(CONTENT_TYPE, "application/json")],
                        response.body,
                    )
                        .into_response()
                }
            }
        }
        Err(error) => {
            let status = match error {
                crate::worker_source::WorkerMutationSourceProofError::Replay => {
                    StatusCode::CONFLICT
                }
                crate::worker_source::WorkerMutationSourceProofError::Authority(_) => {
                    StatusCode::INTERNAL_SERVER_ERROR
                }
                _ => StatusCode::FORBIDDEN,
            };
            (
                status,
                Json(serde_json::json!({ "error": error.to_string() })),
            )
                .into_response()
        }
    }
}

async fn scoped_get_workspace_worker(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedWorkspaceWorkerReferencePath>,
) -> ApiResult<Json<workspace_api::WorkerSummary>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    let worker_id = api
        .store
        .resolve_resource_reference(
            &api.config.workspace_id,
            WorkspaceResourceKind::Worker,
            &path.worker_ref,
        )?
        .ok_or_else(|| Error::UnknownWorker {
            worker: RuntimeWorkerRef::new("unknown", &path.worker_ref),
        })?;
    let workers = workers_response(api.clone())?;
    workers
        .items
        .into_iter()
        .find(|worker| worker.worker_id == worker_id)
        .map(Json)
        .ok_or_else(|| {
            Error::UnknownWorker {
                worker: RuntimeWorkerRef::new("unknown", worker_id),
            }
            .into()
        })
}

async fn scoped_list_workers(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedWorkspacePath>,
) -> ApiResult<Json<workspace_api::ListResponse<workspace_api::WorkerSummary>>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    list_workers(State(api)).await
}

async fn scoped_workspace_orchestrator_status(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedWorkspacePath>,
) -> ApiResult<Json<BrowserWorkspaceOrchestratorResponse>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    Ok(Json(workspace_orchestrator_response(&api, "observed")))
}

#[derive(Debug, Serialize, Deserialize)]
struct KnownWorkerRecord {
    subject: RuntimeWorkerRef,
    relation: String,
    origin: String,
    permissions: Vec<String>,
    summary: WorkerSummary,
}

#[derive(Debug, Serialize, Deserialize)]
struct KnownWorkersResponse {
    workspace_id: String,
    items: Vec<KnownWorkerRecord>,
    truncated: bool,
}

async fn list_known_workers(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedWorkspacePath>,
    headers: HeaderMap,
) -> ApiResult<Json<KnownWorkersResponse>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    let source = authenticate_worker_mutation_source(&api, &path.workspace_id, &headers)?;
    let controller = RuntimeWorkerRef::new(&source.runtime_id, &source.worker_id);
    let limit = api.config.max_records.clamp(1, 500);
    let grants =
        api.store
            .list_active_worker_control_grants(&path.workspace_id, &controller, limit + 1)?;
    let truncated = grants.len() > limit;
    let mut items = Vec::with_capacity(grants.len().min(limit));
    for grant in grants.into_iter().take(limit) {
        let summary = api
            .runtime
            .worker(&grant.subject)
            .map_err(|error| error.into_error())?;
        items.push(KnownWorkerRecord {
            subject: grant.subject,
            relation: grant.relation,
            origin: grant.origin,
            permissions: grant.permissions,
            summary,
        });
    }
    Ok(Json(KnownWorkersResponse {
        workspace_id: path.workspace_id,
        items,
        truncated,
    }))
}

fn scoped_worker_control_operation_id(controller: &RuntimeWorkerRef, operation_id: &str) -> String {
    format!(
        "worker-control:{}:{}:{operation_id}",
        controller.runtime_id, controller.worker_id
    )
}

async fn spawn_known_worker(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedWorkspacePath>,
    headers: HeaderMap,
    Json(mut request): Json<CreateWorkspaceWorkerRequest>,
) -> ApiResult<Json<BrowserCreateWorkerResponse>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    let source = authenticate_worker_mutation_source(&api, &path.workspace_id, &headers)?;
    let relation = if request.ticket_assignment.is_some() {
        "assigned"
    } else {
        "spawned"
    };
    let operation_id = request
        .control_operation_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| Error::InvalidInput("control_operation_id is required".to_string()))?
        .to_string();
    let controller = RuntimeWorkerRef::new(&source.runtime_id, &source.worker_id);
    let fingerprint_input = serde_json::to_vec(&serde_json::json!({
        "controller": &controller,
        "request": &request,
    }))
    .map_err(|error| Error::InvalidInput(format!("invalid Worker spawn input: {error}")))?;
    let input_fingerprint = format!(
        "sha256:{}",
        Sha256::digest(&fingerprint_input)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    );
    request.resolved_control_operation = Some(WorkerControlOperation {
        operation_id: scoped_worker_control_operation_id(&controller, &operation_id),
        input_fingerprint,
    });
    let response = create_workspace_worker(State(api.clone()), headers, Json(request)).await?;
    if let Err(error) = api
        .store
        .create_worker_control_grant(&WorkerControlGrantRecord {
            workspace_id: path.workspace_id.clone(),
            grant_id: new_id("wcg"),
            controller,
            subject: response.0.worker_ref.clone(),
            relation: relation.to_string(),
            origin: "worker_spawn".to_string(),
            permissions: vec![
                "send_input".to_string(),
                "notify".to_string(),
                "cancel".to_string(),
                "stop".to_string(),
                "restore".to_string(),
                "remove".to_string(),
                "observe".to_string(),
            ],
            operation_id,
            created_at: Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
            revoked_at: None,
        })
    {
        // Runtime creation is idempotent under the trusted control operation.
        // Preserve the unacknowledged Worker/assignment so a retry converges on
        // the same subject and can finish grant persistence without creating a
        // second Worker or leaving assignment cleanup races.
        return Err(ApiError::from(error));
    }
    Ok(response)
}

fn worker_control_lock(api: &WorkspaceApi, grant_id: &str) -> Arc<tokio::sync::Mutex<()>> {
    let mut locks = api
        .worker_control_locks
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    Arc::clone(
        locks
            .entry(grant_id.to_string())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(()))),
    )
}

fn authorize_known_worker_permission(
    api: &WorkspaceApi,
    workspace_id: &str,
    controller: &RuntimeWorkerRef,
    subject: &RuntimeWorkerRef,
    permission: &str,
) -> Result<WorkerControlGrantRecord> {
    let grant = api
        .store
        .get_active_worker_control_grant(workspace_id, controller, subject)?
        .ok_or_else(|| Error::UnknownWorker {
            worker: subject.clone(),
        })?;
    if !grant
        .permissions
        .iter()
        .any(|candidate| candidate == permission)
    {
        return Err(Error::InvalidInput(format!(
            "worker control permission `{permission}` was not granted"
        )));
    }
    Ok(grant)
}

async fn send_known_worker_input(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedRuntimeWorkerPath>,
    headers: HeaderMap,
    Json(request): Json<WorkerInputRequest>,
) -> ApiResult<Json<WorkerInputResult>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    let source = authenticate_worker_mutation_source(&api, &path.workspace_id, &headers)?;
    let controller = RuntimeWorkerRef::new(&source.runtime_id, &source.worker_id);
    let permission = match request.kind {
        WorkerInputKind::Notify => "notify",
        _ => "send_input",
    };
    let grant = authorize_known_worker_permission(
        &api,
        &path.workspace_id,
        &controller,
        &path.worker,
        permission,
    )?;
    let lock = worker_control_lock(&api, &grant.grant_id);
    let _guard = lock.lock().await;
    authorize_known_worker_permission(
        &api,
        &path.workspace_id,
        &controller,
        &path.worker,
        permission,
    )?;
    scoped_send_runtime_worker_input(State(api), AxumPath(path), Json(request)).await
}

async fn cancel_known_worker(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedRuntimeWorkerPath>,
    headers: HeaderMap,
    Json(request): Json<WorkerLifecycleRequest>,
) -> ApiResult<Json<WorkerLifecycleResult>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    let source = authenticate_worker_mutation_source(&api, &path.workspace_id, &headers)?;
    let controller = RuntimeWorkerRef::new(&source.runtime_id, &source.worker_id);
    let grant = authorize_known_worker_permission(
        &api,
        &path.workspace_id,
        &controller,
        &path.worker,
        "cancel",
    )?;
    let lock = worker_control_lock(&api, &grant.grant_id);
    let _guard = lock.lock().await;
    authorize_known_worker_permission(
        &api,
        &path.workspace_id,
        &controller,
        &path.worker,
        "cancel",
    )?;
    scoped_cancel_runtime_worker(State(api), AxumPath(path), Json(request)).await
}

async fn stop_known_worker(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedRuntimeWorkerPath>,
    headers: HeaderMap,
    Json(request): Json<WorkerLifecycleRequest>,
) -> ApiResult<Json<WorkerLifecycleResult>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    let source = authenticate_worker_mutation_source(&api, &path.workspace_id, &headers)?;
    let subject = path.worker.clone();
    let controller = RuntimeWorkerRef::new(&source.runtime_id, &source.worker_id);
    let grant =
        authorize_known_worker_permission(&api, &path.workspace_id, &controller, &subject, "stop")?;
    let lock = worker_control_lock(&api, &grant.grant_id);
    let _guard = lock.lock().await;
    authorize_known_worker_permission(&api, &path.workspace_id, &controller, &subject, "stop")?;
    scoped_stop_runtime_worker(State(api), AxumPath(path), Json(request)).await
}

async fn restore_known_worker(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedRuntimeWorkerPath>,
    headers: HeaderMap,
) -> ApiResult<Json<workspace_api::WorkerRestoreResponse>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    let source = authenticate_worker_mutation_source(&api, &path.workspace_id, &headers)?;
    let subject = path.worker.clone();
    let controller = RuntimeWorkerRef::new(&source.runtime_id, &source.worker_id);
    let grant = authorize_known_worker_permission(
        &api,
        &path.workspace_id,
        &controller,
        &subject,
        "restore",
    )?;
    let lock = worker_control_lock(&api, &grant.grant_id);
    let _guard = lock.lock().await;
    authorize_known_worker_permission(&api, &path.workspace_id, &controller, &subject, "restore")?;
    scoped_restore_runtime_worker(State(api), AxumPath(path), Query(Default::default())).await
}

async fn scoped_list_worker_observation_sessions(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedWorkspacePath>,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    let source = authenticate_worker_mutation_source(&api, &path.workspace_id, &headers)?;
    let controller = RuntimeWorkerRef::new(&source.runtime_id, &source.worker_id);
    let sessions = api
        .store
        .list_active_worker_control_grants(&path.workspace_id, &controller, 100)?
        .into_iter()
        .filter(|grant| {
            grant
                .permissions
                .iter()
                .any(|permission| permission == "observe")
        })
        .filter_map(|grant| {
            let worker = api.runtime.worker(&grant.subject).ok()?;
            if matches!(
                worker.state.as_str(),
                "stopped" | "failed" | "rejected" | "disconnected"
            ) {
                return None;
            }
            Some(WorkerObservationSubject {
                subject: WorkerObservationSubjectRef::RuntimeWorker {
                    runtime_id: grant.subject.runtime_id,
                    worker_id: grant.subject.worker_id,
                },
                display_name: worker.display_name,
                relation: grant.relation,
                status: worker.state,
            })
        })
        .collect::<Vec<_>>();
    Ok(Json(serde_json::json!({ "sessions": sessions })))
}

async fn scoped_capture_worker_observation_session(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedWorkspacePath>,
    headers: HeaderMap,
    Json(subject): Json<WorkerObservationSubjectRef>,
) -> ApiResult<Json<serde_json::Value>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    let source = authenticate_worker_mutation_source(&api, &path.workspace_id, &headers)?;
    let WorkerObservationSubjectRef::RuntimeWorker {
        runtime_id,
        worker_id,
    } = subject
    else {
        return Err(ApiError::from(Error::UnknownWorker {
            worker: RuntimeWorkerRef::new("subworker", "inaccessible"),
        }));
    };
    let target = RuntimeWorkerRef::new(runtime_id, worker_id);
    let controller = RuntimeWorkerRef::new(&source.runtime_id, &source.worker_id);
    let grant = authorize_known_worker_permission(
        &api,
        &path.workspace_id,
        &controller,
        &target,
        "observe",
    )?;
    let lock = worker_control_lock(&api, &grant.grant_id);
    let _guard = lock.lock().await;
    authorize_known_worker_permission(&api, &path.workspace_id, &controller, &target, "observe")?;
    let target_summary = api
        .runtime
        .worker(&target)
        .map_err(|error| error.into_error())?;
    if matches!(
        target_summary.state.as_str(),
        "stopped" | "failed" | "rejected" | "disconnected"
    ) {
        return Err(ApiError::from(Error::UnknownWorker { worker: target }));
    }

    let mut connection = connect_workspace_worker_protocol(&api, &target).await?;
    let event = tokio::time::timeout(std::time::Duration::from_secs(10), connection.events.recv())
        .await
        .map_err(|_| {
            ApiError::from(Error::RuntimeOperationFailed {
                runtime_id: target.runtime_id.clone(),
                code: "worker_observation_timeout".to_string(),
                message: "timed out waiting for the committed session snapshot".to_string(),
            })
        })?
        .ok_or_else(|| {
            ApiError::from(Error::RuntimeOperationFailed {
                runtime_id: target.runtime_id.clone(),
                code: "worker_observation_closed".to_string(),
                message: "worker protocol closed before the session snapshot".to_string(),
            })
        })?;
    let protocol::Event::Snapshot { entries, .. } = event else {
        return Err(ApiError::from(Error::RuntimeOperationFailed {
            runtime_id: target.runtime_id.clone(),
            code: "worker_observation_missing_snapshot".to_string(),
            message: "worker protocol did not begin with a committed session snapshot".to_string(),
        }));
    };
    Ok(Json(serde_json::json!({
        "segment_id": format!("runtime:{}:worker:{}", target.runtime_id, target.worker_id),
        "entries": entries,
    })))
}

async fn scoped_start_workspace_orchestrator(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedWorkspacePath>,
) -> ApiResult<Json<BrowserWorkspaceOrchestratorResponse>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    let _guard = api.orchestrator_spawn_lock.lock().map_err(|_| {
        ApiError::with_diagnostics(
            Error::RuntimeOperationFailed {
                runtime_id: EMBEDDED_WORKER_RUNTIME_ID.to_string(),
                code: "workspace_orchestrator_spawn_lock_poisoned".to_string(),
                message: "Workspace Orchestrator launch lock is unavailable".to_string(),
            },
            Vec::new(),
        )
    })?;

    if let Some(existing) = find_workspace_orchestrator(&api) {
        if workspace_orchestrator_is_online(&existing) {
            return Ok(Json(workspace_orchestrator_response(&api, "existing")));
        }
        let restored = api
            .runtime
            .restore_worker(&existing.worker)
            .map_err(|error| error.into_error())?;
        if restored.state != WorkerOperationState::Accepted {
            return Err(ApiError::with_diagnostics(
                Error::RuntimeOperationFailed {
                    runtime_id: existing.worker.runtime_id.clone(),
                    code: "workspace_orchestrator_restore_rejected".to_string(),
                    message: "Runtime rejected Workspace Orchestrator restore".to_string(),
                },
                restored.diagnostics,
            ));
        }
        *api.orchestrator_attention_fingerprint
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        dispatch_orchestrator_queue_attention(&api);
        return Ok(Json(workspace_orchestrator_response(&api, "restored")));
    }

    let result = api.spawn_workspace_worker(
        EMBEDDED_WORKER_RUNTIME_ID,
        WorkerSpawnRequest {
            requested_worker_name: Some(
                crate::hosts::WORKSPACE_ORCHESTRATOR_SINGLETON_KEY.to_string(),
            ),
            intent: WorkerSpawnIntent::WorkspaceOrchestrator,
            acceptance: WorkerSpawnAcceptanceRequirement::RunAccepted {
                expected_segments: 0,
            },
            profile: ProfileSelector::Builtin("builtin:orchestrator".to_string()),
            ticket_assignment: None,
            initial_submit: Vec::new(),
            working_directory_request: None,
            resolved_working_directory_request: None,
            resolved_working_directory: None,
            resolved_config_bundle: None,
            resolved_worker_observation_enabled: true,
            resolved_worker_observation_grants: Vec::new(),
            resolved_control_operation: None,
            resolved_workspace_api: None,
            resolved_memory_settings: None,
        },
    )?;
    if result.state != WorkerOperationState::Accepted || result.worker.is_none() {
        return Err(ApiError::with_diagnostics(
            Error::RuntimeOperationFailed {
                runtime_id: EMBEDDED_WORKER_RUNTIME_ID.to_string(),
                code: "workspace_orchestrator_spawn_rejected".to_string(),
                message: "Embedded Runtime rejected Workspace Orchestrator launch".to_string(),
            },
            result.diagnostics,
        ));
    }
    let worker = result.worker.as_ref().expect("accepted Worker was checked");
    record_worker_summary(
        &api,
        worker,
        &worker.display_name,
        Some("builtin:orchestrator".to_string()),
        WorkerRegistryDisplayNamePolicy::UseProvided,
    )?;
    *api.orchestrator_attention_fingerprint
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    dispatch_orchestrator_queue_attention(&api);
    Ok(Json(workspace_orchestrator_response(&api, "created")))
}

fn workspace_orchestrator_response(
    api: &WorkspaceApi,
    disposition: &str,
) -> BrowserWorkspaceOrchestratorResponse {
    let worker = find_workspace_orchestrator(api);
    let online = worker
        .as_ref()
        .is_some_and(workspace_orchestrator_is_online);
    let diagnostics = worker
        .as_ref()
        .map(|worker| worker.diagnostics.clone())
        .unwrap_or_default();
    BrowserWorkspaceOrchestratorResponse {
        workspace_id: api.config.workspace_id.clone(),
        online,
        disposition: disposition.to_string(),
        worker,
        diagnostics,
    }
}

fn workspace_orchestrator_is_online(worker: &WorkerSummary) -> bool {
    matches!(worker.state.as_str(), "idle" | "running" | "paused")
}

async fn scoped_create_workspace_worker(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedWorkspacePath>,
    headers: HeaderMap,
    Json(request): Json<CreateWorkspaceWorkerRequest>,
) -> ApiResult<Json<BrowserCreateWorkerResponse>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    create_workspace_worker(State(api), headers, Json(request)).await
}

async fn scoped_get_worker_launch_options(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedWorkspacePath>,
) -> ApiResult<Json<WorkerLaunchOptionsResponse>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    get_worker_launch_options(State(api)).await
}

fn working_directory_diagnostics(
    diagnostics: Vec<RuntimeDiagnostic>,
) -> Vec<WorkingDirectoryDiagnostic> {
    diagnostics
        .into_iter()
        .map(|diagnostic| WorkingDirectoryDiagnostic {
            code: diagnostic.code,
            severity: match diagnostic.severity {
                DiagnosticSeverity::Info => WorkingDirectoryDiagnosticSeverity::Info,
                DiagnosticSeverity::Warning => WorkingDirectoryDiagnosticSeverity::Warning,
                DiagnosticSeverity::Error => WorkingDirectoryDiagnosticSeverity::Error,
            },
            message: diagnostic.message,
        })
        .collect()
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
        diagnostics: working_directory_diagnostics(diagnostics),
    }))
}

async fn scoped_create_runtime_working_directory(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedRuntimePath>,
    Json(request): Json<BrowserWorkingDirectoryCreateRequest>,
) -> ApiResult<(StatusCode, Json<BrowserWorkingDirectoryDetailResponse>)> {
    create_workspace_working_directory(
        &api,
        &path.workspace_id,
        Some(path.runtime_id.as_str()),
        request,
    )
    .await
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
    Json(request): Json<BrowserWorkingDirectoryCreateRequest>,
) -> ApiResult<(StatusCode, Json<BrowserWorkingDirectoryDetailResponse>)> {
    create_workspace_working_directory(&api, &path.workspace_id, None, request).await
}

async fn scoped_working_directory_detail(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedWorkingDirectoryPath>,
) -> ApiResult<Json<BrowserWorkingDirectoryDetailResponse>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    let runtime_id = registered_workdir_runtime_id(&api, &path.working_directory_id)?;
    working_directory_detail_for_runtime(api, &runtime_id, &path.working_directory_id)
}

async fn scoped_cleanup_working_directory(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedWorkingDirectoryPath>,
) -> ApiResult<Json<BrowserWorkingDirectoryDetailResponse>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    let runtime_id = registered_workdir_runtime_id(&api, &path.working_directory_id)?;
    cleanup_working_directory_for_runtime(api, &runtime_id, &path.working_directory_id)
}

fn registered_workdir_runtime_id(
    api: &WorkspaceApi,
    working_directory_id: &str,
) -> ApiResult<String> {
    api.store
        .get_workdir_registry(&api.config.workspace_id, working_directory_id)?
        .map(|record| record.runtime_id)
        .ok_or_else(|| {
            ApiError::with_diagnostics(
                Error::RuntimeOperationFailed {
                    runtime_id: "workspace-backend".to_string(),
                    code: "working_directory_not_found".to_string(),
                    message: format!("Unknown Workdir `{working_directory_id}`"),
                },
                Vec::new(),
            )
        })
}

fn workdir_runtime_failure_code(error: &RuntimeRegistryError) -> String {
    match error {
        RuntimeRegistryError::UnknownRuntime(_) | RuntimeRegistryError::UnknownHost(_) => {
            "runtime_unavailable".to_string()
        }
        RuntimeRegistryError::RuntimeOperationFailed { code, .. } => code.clone(),
        RuntimeRegistryError::InvalidIdentifier { .. } => "invalid_runtime".to_string(),
        RuntimeRegistryError::UnknownWorker { .. } => "runtime_workdir_lookup_failed".to_string(),
    }
}

fn workdir_rejection_failure_code(diagnostics: &[RuntimeDiagnostic]) -> String {
    diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic.severity == DiagnosticSeverity::Error
                && !diagnostic.code.is_empty()
                && diagnostic.code.len() <= 128
                && diagnostic
                    .code
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        })
        .map(|diagnostic| diagnostic.code.clone())
        .unwrap_or_else(|| "runtime_workdir_create_failed".to_string())
}

fn finish_rejected_workdir_create_operation(
    store: &crate::SqliteWorkspaceStore,
    workspace_id: &str,
    operation_id: &str,
    request_fingerprint: &str,
    diagnostics: &[RuntimeDiagnostic],
    updated_at: &str,
) -> Result<String> {
    let failure_code = workdir_rejection_failure_code(diagnostics);
    store.finish_workdir_create_operation(
        workspace_id,
        operation_id,
        request_fingerprint,
        false,
        Some(&failure_code),
        updated_at,
    )?;
    Ok(failure_code)
}

async fn create_workspace_working_directory(
    api: &WorkspaceApi,
    workspace_id: &str,
    route_runtime_id: Option<&str>,
    request: BrowserWorkingDirectoryCreateRequest,
) -> ApiResult<(StatusCode, Json<BrowserWorkingDirectoryDetailResponse>)> {
    validate_workspace_scope(api, workspace_id)?;
    if let (Some(route_runtime_id), Some(request_runtime_id)) =
        (route_runtime_id, request.runtime_id.as_deref())
        && route_runtime_id != request_runtime_id
    {
        return Err(Error::InvalidInput(
            "runtime_id does not match the runtime-scoped route".to_string(),
        )
        .into());
    }
    let requested_runtime_id = route_runtime_id
        .map(str::to_string)
        .or_else(|| request.runtime_id.clone());
    let mut working_directory_request =
        working_directory_request_for_browser(api, request.clone())?;
    let operation_id = request
        .operation_id
        .clone()
        .unwrap_or_else(|| Uuid::now_v7().to_string());
    if operation_id.trim().is_empty() || operation_id.len() > 256 {
        return Err(Error::InvalidInput(
            "operation_id must be non-empty and at most 256 bytes".to_string(),
        )
        .into());
    }
    let selector = working_directory_request
        .repository
        .selector
        .as_ref()
        .map(|selector| selector.as_ref().to_string());
    let request_fingerprint = crate::workdir_create_operations::request_fingerprint(
        &request.repository_id,
        selector.as_deref(),
        requested_runtime_id.as_deref(),
    );
    let reserved = if let Some(existing) = api
        .config_store
        .load_workdir_create_operation(workspace_id, &operation_id)?
    {
        if existing.request_fingerprint != request_fingerprint {
            return Err(Error::InvalidInput(format!(
                "Workdir create operation `{operation_id}` was reused with different input"
            ))
            .into());
        }
        existing
    } else {
        let config_state = api
            .config_store
            .load_workspace_config(workspace_id)?
            .ok_or_else(|| {
                Error::RegistryInconsistency(format!(
                    "Workspace {workspace_id} has no active configuration"
                ))
            })?;
        let runtime_projection = crate::runtime_settings::project_runtime_from_workspace_config(
            workspace_id,
            &config_state,
        )?;
        let resolved_runtime_id = requested_runtime_id
            .clone()
            .or(runtime_projection.default_runtime_id.clone())
            .ok_or_else(|| {
                ApiError::with_diagnostics(
                    Error::InvalidInput(
                        "runtime_id was omitted and Workspace configuration has no runtime.default_runtime_id"
                            .to_string(),
                    ),
                    vec![RuntimeDiagnostic {
                        code: "default_runtime_not_configured".to_string(),
                        severity: DiagnosticSeverity::Error,
                        message: "Workspace default Runtime is not configured".to_string(),
                    }],
                )
            })?;
        let now = now_registry_timestamp();
        api.config_store
            .reserve_workdir_create_operation(&WorkdirCreateOperationRecord {
                workspace_id: workspace_id.to_string(),
                operation_id: operation_id.clone(),
                request_fingerprint: request_fingerprint.clone(),
                repository_id: request.repository_id.clone(),
                selector,
                requested_runtime_id,
                resolved_runtime_id,
                config_revision: runtime_projection.config_revision,
                config_projection_digest: runtime_projection.projection_digest,
                working_directory_id: next_backend_workdir_id(&request.repository_id),
                state: "pending".to_string(),
                failure: None,
                created_at: now.clone(),
                updated_at: now,
            })?
    };

    if reserved.state == "succeeded" {
        return working_directory_detail_for_runtime(
            api.clone(),
            &reserved.resolved_runtime_id,
            &reserved.working_directory_id,
        )
        .map(|response| (StatusCode::OK, response));
    }

    let runtime = match api
        .runtime
        .list_runtimes(usize::MAX)
        .items
        .into_iter()
        .find(|runtime| runtime.runtime_id == reserved.resolved_runtime_id)
    {
        Some(runtime) => runtime,
        None => {
            api.config_store.finish_workdir_create_operation(
                workspace_id,
                &operation_id,
                &request_fingerprint,
                false,
                Some("runtime_unavailable"),
                &now_registry_timestamp(),
            )?;
            return Err(ApiError::with_diagnostics(
                Error::UnknownRuntime(reserved.resolved_runtime_id),
                vec![RuntimeDiagnostic {
                    code: "runtime_unavailable".to_string(),
                    severity: DiagnosticSeverity::Error,
                    message: "Selected Runtime is not available".to_string(),
                }],
            ));
        }
    };
    if runtime.kind == "remote_http" && runtime.status != "connected" {
        api.config_store.finish_workdir_create_operation(
            workspace_id,
            &operation_id,
            &request_fingerprint,
            false,
            Some("runtime_unavailable"),
            &now_registry_timestamp(),
        )?;
        return Err(ApiError::with_diagnostics(
            Error::RuntimeOperationFailed {
                runtime_id: reserved.resolved_runtime_id,
                code: "runtime_unavailable".to_string(),
                message: "Selected Runtime is not connected".to_string(),
            },
            vec![RuntimeDiagnostic {
                code: "runtime_unavailable".to_string(),
                severity: DiagnosticSeverity::Error,
                message: "Selected Runtime is not available".to_string(),
            }],
        ));
    }

    if !runtime.capabilities.supports_worktrees {
        api.config_store.finish_workdir_create_operation(
            workspace_id,
            &operation_id,
            &request_fingerprint,
            false,
            Some("runtime_workdir_unsupported"),
            &now_registry_timestamp(),
        )?;
        return Err(ApiError::with_diagnostics(
            Error::RuntimeOperationFailed {
                runtime_id: reserved.resolved_runtime_id,
                code: "runtime_workdir_unsupported".to_string(),
                message: "Selected Runtime does not support Workdir creation".to_string(),
            },
            vec![RuntimeDiagnostic {
                code: "runtime_workdir_unsupported".to_string(),
                severity: DiagnosticSeverity::Error,
                message: "Selected Runtime does not support Workdir creation".to_string(),
            }],
        ));
    }

    working_directory_request.backend_workdir_id = Some(reserved.working_directory_id.clone());
    let existing = match api.runtime.working_directory(
        &reserved.resolved_runtime_id,
        &reserved.working_directory_id,
    ) {
        Ok(existing) => existing,
        Err(error) => {
            let failure_code = workdir_runtime_failure_code(&error);
            api.config_store.finish_workdir_create_operation(
                workspace_id,
                &operation_id,
                &request_fingerprint,
                false,
                Some(&failure_code),
                &now_registry_timestamp(),
            )?;
            return Err(ApiError::with_diagnostics(
                error.into_error(),
                vec![RuntimeDiagnostic {
                    code: failure_code,
                    severity: DiagnosticSeverity::Error,
                    message: "Selected Runtime could not inspect the reserved Workdir".to_string(),
                }],
            ));
        }
    };
    let result = if existing.working_directory.is_some() {
        existing
    } else {
        match api
            .runtime
            .create_working_directory(&reserved.resolved_runtime_id, working_directory_request)
        {
            Ok(result) => result,
            Err(error) => {
                let failure_code = workdir_runtime_failure_code(&error);
                let _ = api
                    .store
                    .delete_workdir_registry(workspace_id, &reserved.working_directory_id);
                api.config_store.finish_workdir_create_operation(
                    workspace_id,
                    &operation_id,
                    &request_fingerprint,
                    false,
                    Some(&failure_code),
                    &now_registry_timestamp(),
                )?;
                return Err(ApiError::with_diagnostics(
                    error.into_error(),
                    vec![RuntimeDiagnostic {
                        code: failure_code,
                        severity: DiagnosticSeverity::Error,
                        message: "Selected Runtime rejected Workdir creation".to_string(),
                    }],
                ));
            }
        }
    };
    let Some(working_directory) = result.working_directory else {
        let _ = api
            .store
            .delete_workdir_registry(workspace_id, &reserved.working_directory_id);
        let mut diagnostics = result.diagnostics;
        let failure_code = finish_rejected_workdir_create_operation(
            &api.config_store,
            workspace_id,
            &operation_id,
            &request_fingerprint,
            &diagnostics,
            &now_registry_timestamp(),
        )?;
        if !diagnostics.iter().any(|diagnostic| {
            diagnostic.severity == DiagnosticSeverity::Error && diagnostic.code == failure_code
        }) {
            diagnostics.insert(
                0,
                RuntimeDiagnostic {
                    code: failure_code.clone(),
                    severity: DiagnosticSeverity::Error,
                    message: "Runtime did not create the reserved Workdir".to_string(),
                },
            );
        }
        return Err(ApiError::with_diagnostics(
            Error::RuntimeOperationFailed {
                runtime_id: reserved.resolved_runtime_id,
                code: failure_code,
                message: "Runtime did not create working directory".to_string(),
            },
            diagnostics,
        ));
    };
    let record = workdir_record_from_summary(
        api,
        &reserved.resolved_runtime_id,
        &working_directory.summary,
    );
    api.store.upsert_workdir_registry(&record)?;
    api.config_store.finish_workdir_create_operation(
        workspace_id,
        &operation_id,
        &request_fingerprint,
        true,
        None,
        &now_registry_timestamp(),
    )?;
    let mut summary = working_directory.summary;
    apply_workdir_occupancy_projection(api, &mut summary)?;
    Ok((
        StatusCode::CREATED,
        Json(BrowserWorkingDirectoryDetailResponse {
            workspace_id: workspace_id.to_string(),
            runtime_id: reserved.resolved_runtime_id,
            item: summary,
            diagnostics: working_directory_diagnostics(result.diagnostics),
        }),
    ))
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
        let mut summary = working_directory.summary;
        apply_workdir_occupancy_projection(&api, &mut summary)?;
        return Ok(Json(BrowserWorkingDirectoryDetailResponse {
            workspace_id: api.config.workspace_id.clone(),
            runtime_id: runtime_id.to_string(),
            item: summary,
            diagnostics: working_directory_diagnostics(result.diagnostics),
        }));
    }
    if let Some(record) = api
        .store
        .get_workdir_registry(&api.config.workspace_id, working_directory_id)?
    {
        return Ok(Json(BrowserWorkingDirectoryDetailResponse {
            workspace_id: api.config.workspace_id.clone(),
            runtime_id: runtime_id.to_string(),
            item: projected_workdir_summary_from_record(&api, &record)?,
            diagnostics: working_directory_diagnostics(result.diagnostics),
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
    let mut summary = working_directory.summary;
    apply_workdir_occupancy_projection(&api, &mut summary)?;
    Ok(Json(BrowserWorkingDirectoryDetailResponse {
        workspace_id: api.config.workspace_id.clone(),
        runtime_id: runtime_id.to_string(),
        item: summary,
        diagnostics: working_directory_diagnostics(result.diagnostics),
    }))
}

async fn set_worker_retention(
    api: WorkspaceApi,
    runtime_id: String,
    runtime_worker_id: String,
    pinned: bool,
) -> ApiResult<Json<WorkerRetentionResponse>> {
    parse_runtime_worker_id_for_registry(&runtime_worker_id)?;
    let worker_ref = RuntimeWorkerRef::new(runtime_id.clone(), runtime_worker_id.clone());
    if api
        .store
        .get_worker_registry(&api.config.workspace_id, &worker_ref)?
        .is_none()
    {
        if let Ok(worker) = api.runtime.worker(&worker_ref) {
            let _ = sync_worker_observation(&api, &worker);
        }
    }
    let retention_state = if pinned { "pinned" } else { "normal" };
    let changed = api.store.update_worker_retention(
        &api.config.workspace_id,
        &worker_ref,
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
        worker_ref,
        pinned,
        retention_state: retention_state.to_string(),
    }))
}

fn build_runtime_cleanup_plan(
    api: &WorkspaceApi,
    runtime_id: &str,
) -> ApiResult<RuntimeCleanupPlanResponse> {
    let workers = workers_response(api.clone())?;
    let live_running_worker_ids: HashSet<RuntimeWorkerRef> = workers
        .items
        .iter()
        .filter(|worker| worker.state == "running")
        .map(|worker| RuntimeWorkerRef::new(&worker.runtime_id, &worker.worker_id))
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
        .map(|record| (record.worker.clone(), record.clone()))
        .collect();
    let observed_workdirs: HashMap<_, _> = workdir_summaries
        .into_iter()
        .map(|summary| (summary.working_directory_id.clone(), summary))
        .collect();

    let mut worker_candidates = Vec::new();
    for record in worker_records
        .iter()
        .filter(|record| record.worker.runtime_id == runtime_id)
    {
        let links = api
            .store
            .list_worker_workdir_links(&api.config.workspace_id, &record.worker)?;
        let current_assignment = api.store.get_current_ticket_role_assignment_for_worker(
            &api.config.workspace_id,
            &record.worker,
        )?;
        let is_running = live_running_worker_ids.contains(&record.worker);
        let pinned = record.retention_state == "pinned";
        let blocking_reason = if let Some(assignment) = current_assignment {
            Some(format!(
                "worker has current Ticket assignment `{}` (`{}`)",
                assignment.ticket_id,
                assignment.role.as_str()
            ))
        } else if pinned {
            Some("worker is pinned".to_string())
        } else if is_running {
            Some("worker is running".to_string())
        } else {
            None
        };
        worker_candidates.push(CleanupWorkerCandidate {
            target_id: format!(
                "worker:{}:{}",
                encode_path_segment(record.worker.runtime_id.as_str()),
                encode_path_segment(&record.worker.worker_id)
            ),
            action: CleanupTargetKind::WorkerDelete,
            worker_id: record.worker.worker_id.clone(),
            runtime_worker_id: record.worker.worker_id.clone(),
            runtime_id: record.worker.runtime_id.clone(),
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
            .filter_map(|link| worker_by_id.get(&link.worker))
            .collect::<Vec<_>>();
        let linked_worker_ids = links
            .iter()
            .map(|link| link.worker.worker_id.clone())
            .collect::<Vec<_>>();
        let linked_running_worker_ids = linked_workers
            .iter()
            .filter(|worker| live_running_worker_ids.contains(&worker.worker))
            .map(|worker| worker.worker.worker_id.clone())
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

async fn execute_runtime_cleanup(
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
        let worker = RuntimeWorkerRef::new(
            candidate.runtime_id.clone(),
            candidate.runtime_worker_id.clone(),
        );
        if let Some(assignment) = api
            .store
            .get_current_ticket_role_assignment_for_worker(&api.config.workspace_id, &worker)?
        {
            return Err(cleanup_api_error(
                runtime_id,
                "workspace_cleanup_worker_assigned",
                &format!(
                    "Worker is assigned to Ticket `{}` as `{}` and cannot be deleted",
                    assignment.ticket_id,
                    assignment.role.as_str()
                ),
            ));
        }
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
        parse_runtime_worker_id_for_registry(&candidate.runtime_worker_id)?;
        let session_lock = current_worker_session_lock(api, &worker);
        let _session_guard = session_lock.lock().await;
        close_current_worker_session_locked(api, &worker).await?;
        cleanup_runtime_worker_for_execution(api, runtime_id, candidate)?;
        api.store
            .delete_worker_registry(&api.config.workspace_id, &worker)?;
        drop(_session_guard);
        api.workdir_session_locks
            .lock()
            .expect("Workdir session lock registry poisoned")
            .remove(&worker);
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
    let worker = RuntimeWorkerRef::new(runtime_id, &candidate.runtime_worker_id);
    match api.runtime.stop_worker(
        &worker,
        WorkerLifecycleRequest {
            reason: Some("cleanup worker before deletion".to_string()),
            ticket_assignment: None,
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

    match api.runtime.delete_worker(&worker) {
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
    Query(query): Query<RuntimeWorkersQuery>,
) -> ApiResult<Json<workspace_api::ListResponse<workspace_api::WorkerSummary>>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    list_runtime_workers(State(api), AxumPath(path.runtime_id), Query(query)).await
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
) -> ApiResult<Json<WorkerShowProjection>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    get_runtime_worker(
        State(api),
        AxumPath((path.worker.runtime_id, path.worker.worker_id)),
    )
    .await
}

#[derive(Debug, Default, Deserialize)]
struct RestoreTicketAssignmentQuery {
    ticket_id: Option<String>,
    assignment_operation_id: Option<String>,
}

async fn scoped_restore_runtime_worker(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedRuntimeWorkerPath>,
    Query(query): Query<RestoreTicketAssignmentQuery>,
) -> ApiResult<Json<workspace_api::WorkerRestoreResponse>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    let workspace_id = path.workspace_id.clone();
    let runtime_id = path.worker.runtime_id.clone();
    let worker_id = path.worker.worker_id.clone();
    let assignment_request = match (
        query.ticket_id.clone(),
        query.assignment_operation_id.clone(),
    ) {
        (Some(ticket_id), Some(operation_id)) => {
            Some(crate::hosts::WorkerTicketAssignmentRequest {
                ticket_id,
                operation_id,
            })
        }
        (None, None) => None,
        _ => {
            return Err(Error::TicketAssignmentConflict(
                "restore assignment requires both ticket_id and assignment_operation_id"
                    .to_string(),
            )
            .into());
        }
    };
    if let Some(assignment) = assignment_request.as_ref() {
        let fingerprint = format!(
            "sha256:{}",
            Sha256::digest(format!(
                "restore\0{}\0{}\0{}",
                assignment.ticket_id, runtime_id, worker_id
            ))
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
        );
        api.store.reserve_ticket_assignment_operation(
            &workspace_id,
            &assignment.operation_id,
            &assignment.ticket_id,
            &runtime_id,
            Some(&worker_id),
            &fingerprint,
            &Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
        )?;
        if let Some(worker) = existing_lifecycle_assignment_worker(&api, assignment, &runtime_id)? {
            if worker.worker.worker_id != worker_id {
                return Err(Error::TicketAssignmentConflict(format!(
                    "assignment operation {} belongs to worker {}, not {}",
                    assignment.operation_id, worker.worker.worker_id, worker_id
                ))
                .into());
            }
            assign_ticket_worker_from_lifecycle(&api, assignment, &runtime_id, &worker_id)?;
            accept_queued_ticket_after_worker_spawn(&api, assignment)?;
            let worker = project_workspace_worker(&api, worker)?;
            return Ok(Json(workspace_api::WorkerRestoreResponse {
                workspace_id,
                runtime_id: runtime_id.clone(),
                worker_id: worker_id.clone(),
                result: workspace_api::WorkerRestoreResult {
                    state: workspace_api::WorkerOperationState::Accepted,
                    worker: Some(worker),
                    diagnostics: Vec::new(),
                },
            }));
        }
    }
    let response = restore_runtime_worker(
        State(api.clone()),
        AxumPath((runtime_id.clone(), worker_id.clone())),
    )
    .await?;
    if let Some(assignment) = assignment_request.as_ref() {
        assign_ticket_worker_from_lifecycle(&api, assignment, &runtime_id, &worker_id)?;
        accept_queued_ticket_after_worker_spawn(&api, assignment)?;
    }
    Ok(response)
}

async fn scoped_pin_runtime_worker(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedRuntimeWorkerPath>,
) -> ApiResult<Json<WorkerRetentionResponse>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    set_worker_retention(api, path.worker.runtime_id, path.worker.worker_id, true).await
}

async fn scoped_unpin_runtime_worker(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedRuntimeWorkerPath>,
) -> ApiResult<Json<WorkerRetentionResponse>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    set_worker_retention(api, path.worker.runtime_id, path.worker.worker_id, false).await
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
    let response = execute_runtime_cleanup(&api, path.runtime_id.as_str(), request).await?;
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
        AxumPath((path.worker.runtime_id, path.worker.worker_id)),
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
        AxumPath((path.worker.runtime_id, path.worker.worker_id)),
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
        AxumPath((path.worker.runtime_id, path.worker.worker_id)),
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
        AxumPath((path.worker.runtime_id, path.worker.worker_id)),
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
    worker_protocol_ws(
        State(api),
        AxumPath((path.worker.runtime_id, path.worker.worker_id)),
        ws,
    )
    .await
    .into_response()
}

async fn scoped_list_host_workers(
    State(api): State<WorkspaceApi>,
    AxumPath(path): AxumPath<ScopedHostPath>,
) -> ApiResult<Json<workspace_api::ListResponse<workspace_api::WorkerSummary>>> {
    validate_workspace_scope(&api, &path.workspace_id)?;
    list_host_workers(State(api), AxumPath(path.host_id)).await
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthBootstrapUserRequest {
    handle: String,
    #[serde(default)]
    display_name: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct AuthUserResponse {
    user: AuthenticatedUser,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PasskeyRegistrationOptionsRequest {
    handle: String,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    browser_origin: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct PasskeyRegistrationOptionsResponse {
    challenge_id: String,
    public_key: CreationChallengeResponse,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PasskeyRegistrationCompleteRequest {
    challenge_id: String,
    credential: RegisterPublicKeyCredential,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PasskeyLoginOptionsRequest {
    #[serde(default)]
    handle: Option<String>,
    #[serde(default)]
    browser_origin: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct PasskeyLoginOptionsResponse {
    challenge_id: String,
    public_key: RequestChallengeResponse,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PasskeyLoginCompleteRequest {
    challenge_id: String,
    credential: PublicKeyCredential,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeviceLoginStartRequest {
    #[serde(default)]
    client_name: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct DeviceLoginStartResponse {
    device_code: String,
    user_code: String,
    verification_uri: String,
    verification_uri_complete: String,
    expires_in: u64,
    interval: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeviceLoginApproveRequest {
    user_code: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct DeviceLoginApproveResponse {
    status: String,
    user: AuthenticatedUser,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeviceLoginPollRequest {
    device_code: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct DeviceLoginPollResponse {
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    access_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    token_type: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct WhoamiResponse {
    actor: Option<RequestActor>,
}

#[derive(Debug, Serialize, Deserialize)]
struct LogoutResponse {
    status: String,
}

async fn get_auth_config(State(api): State<ServerAuthApi>) -> ApiResult<Json<AuthPublicConfig>> {
    Ok(Json(auth_public_config(&api.config)))
}

async fn post_auth_bootstrap_user(
    State(api): State<ServerAuthApi>,
    Json(request): Json<AuthBootstrapUserRequest>,
) -> ApiResult<Json<AuthUserResponse>> {
    let user = ensure_user_account(&api, &request.handle, request.display_name.as_deref())?;
    Ok(Json(AuthUserResponse { user }))
}

async fn post_passkey_registration_options(
    State(api): State<ServerAuthApi>,
    headers: HeaderMap,
    Json(request): Json<PasskeyRegistrationOptionsRequest>,
) -> ApiResult<Json<PasskeyRegistrationOptionsResponse>> {
    let user = ensure_user_account(&api, &request.handle, request.display_name.as_deref())?;
    let header_origin = request_origin(&headers);
    let requested_origin = request
        .browser_origin
        .as_deref()
        .or(header_origin.as_deref());
    let auth = auth_config_for_origin(&api.config, requested_origin)?;
    let webauthn = webauthn_for_auth(&auth)?;
    let exclude_credentials = passkeys_for_user(&api, &user.user_id)?
        .into_iter()
        .map(|passkey| passkey.cred_id().clone())
        .collect();
    let user_unique_id = Uuid::now_v7();
    let (public_key, state) = webauthn
        .start_passkey_registration(
            user_unique_id,
            &user.handle,
            &user.display_name,
            Some(exclude_credentials),
        )
        .map_err(|error| auth_error("webauthn_registration_options_failed", &error.to_string()))?;
    let challenge_id = new_id("webauthn-registration");
    api.store.put_auth_challenge(&AuthChallengeRecord {
        challenge_id: challenge_id.clone(),
        ceremony: "passkey_registration".to_string(),
        challenge: challenge_id.clone(),
        user_id: Some(user.user_id),
        rp_id: auth.rp_id,
        origin: auth.origin,
        state_json: Some(
            serde_json::to_string(&state).map_err(|error| {
                auth_error("webauthn_state_serialize_failed", &error.to_string())
            })?,
        ),
        expires_at: rfc3339_after(Duration::minutes(5)),
        created_at: crate::auth::now_rfc3339(),
        consumed_at: None,
    })?;
    Ok(Json(PasskeyRegistrationOptionsResponse {
        challenge_id,
        public_key,
    }))
}

async fn post_passkey_registration_complete(
    State(api): State<ServerAuthApi>,
    Json(request): Json<PasskeyRegistrationCompleteRequest>,
) -> ApiResult<Response> {
    let challenge = api
        .store
        .consume_auth_challenge_by_id(
            &request.challenge_id,
            "passkey_registration",
            &crate::auth::now_rfc3339(),
        )?
        .ok_or_else(|| {
            auth_error(
                "invalid_passkey_challenge",
                "passkey registration challenge is invalid or already consumed",
            )
        })?;
    if is_expired(&challenge.expires_at) {
        return Err(auth_error(
            "expired_passkey_challenge",
            "passkey registration challenge expired",
        )
        .into());
    }
    let user_id = challenge.user_id.clone().ok_or_else(|| {
        auth_error(
            "invalid_passkey_challenge",
            "passkey registration challenge is not bound to a user",
        )
    })?;
    let user = api.store.get_user(&user_id)?.ok_or_else(|| {
        auth_error(
            "unknown_auth_user",
            "passkey registration user does not exist",
        )
    })?;
    let state_json = challenge.state_json.clone().ok_or_else(|| {
        auth_error(
            "missing_webauthn_state",
            "passkey registration state was not persisted",
        )
    })?;
    let state: PasskeyRegistration = serde_json::from_str(&state_json)
        .map_err(|error| auth_error("webauthn_state_deserialize_failed", &error.to_string()))?;
    let webauthn = webauthn_for_challenge(&api.config, &challenge)?;
    let passkey = webauthn
        .finish_passkey_registration(&request.credential, &state)
        .map_err(|error| {
            auth_error(
                "webauthn_registration_verification_failed",
                &error.to_string(),
            )
        })?;
    let credential_id = passkey_credential_id(&passkey)?;
    api.store
        .upsert_passkey_credential(&PasskeyCredentialRecord {
            credential_id,
            user_id: user.user_id.clone(),
            public_key_cose: serde_json::to_string(&passkey).map_err(|error| {
                auth_error("webauthn_passkey_serialize_failed", &error.to_string())
            })?,
            transports_json: None,
            sign_count: 0,
            created_at: crate::auth::now_rfc3339(),
            last_used_at: None,
        })?;
    issue_browser_session_response(&api, user)
}

async fn post_passkey_login_options(
    State(api): State<ServerAuthApi>,
    headers: HeaderMap,
    Json(request): Json<PasskeyLoginOptionsRequest>,
) -> ApiResult<Json<PasskeyLoginOptionsResponse>> {
    let user = match request.handle.as_deref() {
        Some(handle) => api.store.get_user_by_handle(&normalize_handle(handle)?)?,
        None => api.store.any_user()?,
    }
    .ok_or_else(|| auth_error("unknown_auth_user", "no matching user account exists"))?;
    let passkeys = passkeys_for_user(&api, &user.user_id)?;
    if passkeys.is_empty() {
        return Err(auth_error(
            "passkey_not_registered",
            "user has no registered passkey credentials",
        )
        .into());
    }
    let header_origin = request_origin(&headers);
    let requested_origin = request
        .browser_origin
        .as_deref()
        .or(header_origin.as_deref());
    let auth = auth_config_for_origin(&api.config, requested_origin)?;
    let webauthn = webauthn_for_auth(&auth)?;
    let (public_key, state) = webauthn
        .start_passkey_authentication(&passkeys)
        .map_err(|error| auth_error("webauthn_login_options_failed", &error.to_string()))?;
    let challenge_id = new_id("webauthn-login");
    api.store.put_auth_challenge(&AuthChallengeRecord {
        challenge_id: challenge_id.clone(),
        ceremony: "passkey_login".to_string(),
        challenge: challenge_id.clone(),
        user_id: Some(user.user_id),
        rp_id: auth.rp_id,
        origin: auth.origin,
        state_json: Some(
            serde_json::to_string(&state).map_err(|error| {
                auth_error("webauthn_state_serialize_failed", &error.to_string())
            })?,
        ),
        expires_at: rfc3339_after(Duration::minutes(5)),
        created_at: crate::auth::now_rfc3339(),
        consumed_at: None,
    })?;
    Ok(Json(PasskeyLoginOptionsResponse {
        challenge_id,
        public_key,
    }))
}

async fn post_passkey_login_complete(
    State(api): State<ServerAuthApi>,
    Json(request): Json<PasskeyLoginCompleteRequest>,
) -> ApiResult<Response> {
    let challenge = api
        .store
        .consume_auth_challenge_by_id(
            &request.challenge_id,
            "passkey_login",
            &crate::auth::now_rfc3339(),
        )?
        .ok_or_else(|| {
            auth_error(
                "invalid_passkey_challenge",
                "passkey login challenge is invalid or already consumed",
            )
        })?;
    if is_expired(&challenge.expires_at) {
        return Err(auth_error(
            "expired_passkey_challenge",
            "passkey login challenge expired",
        )
        .into());
    }
    let user_id = challenge.user_id.clone().ok_or_else(|| {
        auth_error(
            "invalid_passkey_challenge",
            "passkey login challenge is not bound to a user",
        )
    })?;
    let user = api
        .store
        .get_user(&user_id)?
        .ok_or_else(|| auth_error("unknown_auth_user", "passkey user does not exist"))?;
    let state_json = challenge.state_json.clone().ok_or_else(|| {
        auth_error(
            "missing_webauthn_state",
            "passkey login state was not persisted",
        )
    })?;
    let state: PasskeyAuthentication = serde_json::from_str(&state_json)
        .map_err(|error| auth_error("webauthn_state_deserialize_failed", &error.to_string()))?;
    let webauthn = webauthn_for_challenge(&api.config, &challenge)?;
    let auth_result = webauthn
        .finish_passkey_authentication(&request.credential, &state)
        .map_err(|error| auth_error("webauthn_login_verification_failed", &error.to_string()))?;
    let credential_id = serde_json::to_value(auth_result.cred_id())
        .ok()
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
        .ok_or_else(|| {
            auth_error(
                "invalid_webauthn_credential_id",
                "verified credential id was not serializable",
            )
        })?;
    let stored = api
        .store
        .get_passkey_credential(&credential_id)?
        .ok_or_else(|| {
            auth_error(
                "unknown_passkey_credential",
                "verified passkey credential is not registered",
            )
        })?;
    if stored.user_id != user.user_id {
        return Err(auth_error(
            "passkey_user_mismatch",
            "verified passkey credential does not belong to the challenged user",
        )
        .into());
    }
    api.store
        .upsert_passkey_credential(&PasskeyCredentialRecord {
            credential_id,
            user_id: user.user_id.clone(),
            public_key_cose: stored.public_key_cose,
            transports_json: stored.transports_json,
            sign_count: u64::from(auth_result.counter()),
            created_at: stored.created_at,
            last_used_at: Some(crate::auth::now_rfc3339()),
        })?;
    issue_browser_session_response(&api, user)
}

fn issue_browser_session_response(api: &ServerAuthApi, user: UserRecord) -> ApiResult<Response> {
    let session_token = mint_secret("yoi_sess");
    api.store.create_browser_session(&BrowserSessionRecord {
        session_id: new_id("session"),
        token_hash: token_hash(&session_token),
        user_id: user.user_id.clone(),
        created_at: crate::auth::now_rfc3339(),
        expires_at: rfc3339_after(Duration::days(14)),
        revoked_at: None,
    })?;
    let auth = auth_public_config(&api.config);
    let mut headers = HeaderMap::new();
    headers.insert(
        SET_COOKIE,
        session_set_cookie(
            session_cookie_policy(&auth),
            &session_token,
            14 * 24 * 60 * 60,
        )
        .parse()
        .map_err(|error| {
            auth_error(
                "invalid_session_cookie",
                &format!("failed to build session cookie: {error}"),
            )
        })?,
    );
    Ok((
        headers,
        Json(AuthUserResponse {
            user: user_response(user),
        }),
    )
        .into_response())
}

async fn post_device_login_start(
    State(api): State<ServerAuthApi>,
    Json(request): Json<DeviceLoginStartRequest>,
) -> ApiResult<Json<DeviceLoginStartResponse>> {
    let auth = auth_public_config(&api.config);
    let device_code = mint_secret("yoi_device");
    let user_code = new_user_code();
    let verification_uri = format!(
        "{}/login/device",
        auth.public_base_url.trim_end_matches('/')
    );
    let verification_uri_complete = format!("{verification_uri}?user_code={user_code}");
    api.store.create_device_login_flow(&DeviceLoginFlowRecord {
        device_code: device_code.clone(),
        user_code: user_code.clone(),
        verification_uri: verification_uri.clone(),
        client_name: request.client_name,
        user_id: None,
        api_token_id: None,
        issued_access_token: None,
        created_at: crate::auth::now_rfc3339(),
        expires_at: rfc3339_after(Duration::minutes(10)),
        approved_at: None,
        consumed_at: None,
    })?;
    Ok(Json(DeviceLoginStartResponse {
        device_code,
        user_code,
        verification_uri,
        verification_uri_complete,
        expires_in: 600,
        interval: 5,
    }))
}

async fn post_device_login_approve(
    State(api): State<ServerAuthApi>,
    headers: HeaderMap,
    Json(request): Json<DeviceLoginApproveRequest>,
) -> ApiResult<Json<DeviceLoginApproveResponse>> {
    let actor = require_actor(&api, &headers).await?;
    let flow = api
        .store
        .get_device_login_flow_by_user_code(&request.user_code.trim().to_ascii_uppercase())?
        .ok_or_else(|| {
            auth_error(
                "unknown_device_login_code",
                "device login code does not exist",
            )
        })?;
    if is_expired(&flow.expires_at) {
        return Err(auth_error("expired_device_login", "device login code expired").into());
    }
    if flow.approved_at.is_some() || flow.consumed_at.is_some() {
        return Err(auth_error(
            "device_login_already_used",
            "device login code is already used",
        )
        .into());
    }
    let access_token = mint_secret("yoi_api");
    let token_id = new_id("api-token");
    api.store.create_api_token(&ApiTokenRecord {
        token_id: token_id.clone(),
        token_hash: token_hash(&access_token),
        user_id: actor.user_id.clone(),
        label: flow
            .client_name
            .clone()
            .unwrap_or_else(|| "yoi device login".to_string()),
        created_at: crate::auth::now_rfc3339(),
        expires_at: None,
        revoked_at: None,
        last_used_at: None,
    })?;
    let approved = api.store.approve_device_login_flow(
        &flow.device_code,
        &actor.user_id,
        &token_id,
        &access_token,
        &crate::auth::now_rfc3339(),
    )?;
    if !approved {
        return Err(auth_error(
            "device_login_already_used",
            "device login code is already used",
        )
        .into());
    }
    Ok(Json(DeviceLoginApproveResponse {
        status: "approved".to_string(),
        user: actor.user(),
    }))
}

async fn post_device_login_poll(
    State(api): State<ServerAuthApi>,
    Json(request): Json<DeviceLoginPollRequest>,
) -> ApiResult<Json<DeviceLoginPollResponse>> {
    let Some(flow) = api
        .store
        .get_device_login_flow_by_device_code(&request.device_code)?
    else {
        return Err(auth_error("unknown_device_login", "device login flow does not exist").into());
    };
    if is_expired(&flow.expires_at) {
        return Ok(Json(DeviceLoginPollResponse {
            status: "expired".to_string(),
            access_token: None,
            token_type: None,
        }));
    }
    if flow.approved_at.is_none() {
        return Ok(Json(DeviceLoginPollResponse {
            status: "pending".to_string(),
            access_token: None,
            token_type: None,
        }));
    }
    let Some(consumed) = api
        .store
        .consume_device_login_token(&request.device_code, &crate::auth::now_rfc3339())?
    else {
        return Ok(Json(DeviceLoginPollResponse {
            status: "consumed".to_string(),
            access_token: None,
            token_type: None,
        }));
    };
    Ok(Json(DeviceLoginPollResponse {
        status: "approved".to_string(),
        access_token: consumed.issued_access_token,
        token_type: Some("Bearer".to_string()),
    }))
}

async fn get_auth_whoami(
    State(api): State<ServerAuthApi>,
    headers: HeaderMap,
) -> ApiResult<Json<WhoamiResponse>> {
    Ok(Json(WhoamiResponse {
        actor: resolve_actor(&api, &headers).await?,
    }))
}

async fn post_auth_logout(
    State(api): State<ServerAuthApi>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let auth = auth_public_config(&api.config);
    if let Some(session_token) = parse_cookie(&headers, &auth.cookie_name) {
        let _ = api
            .store
            .revoke_browser_session(&token_hash(&session_token), &crate::auth::now_rfc3339())?;
    }
    let mut response_headers = HeaderMap::new();
    response_headers.insert(
        SET_COOKIE,
        session_set_cookie(session_cookie_policy(&auth), "", 0)
            .parse()
            .map_err(|error| {
                auth_error(
                    "invalid_session_cookie",
                    &format!("failed to build logout cookie: {error}"),
                )
            })?,
    );
    Ok((
        response_headers,
        Json(LogoutResponse {
            status: "logged_out".to_string(),
        }),
    )
        .into_response())
}

fn request_origin(headers: &HeaderMap) -> Option<String> {
    headers
        .get(ORIGIN)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn auth_config_for_origin(
    config: &ServerConfig,
    request_origin: Option<&str>,
) -> ApiResult<AuthPublicConfig> {
    let mut auth = auth_public_config(config);
    let Some(request_origin) = request_origin else {
        return Ok(auth);
    };
    let origin = Url::parse(request_origin)
        .map_err(|error| auth_error("invalid_webauthn_request_origin", &error.to_string()))?;
    let host = origin.host_str().ok_or_else(|| {
        auth_error(
            "invalid_webauthn_request_origin",
            "request Origin header does not contain a host",
        )
    })?;
    if host != auth.rp_id {
        return Err(auth_error(
            "webauthn_origin_rp_id_mismatch",
            &format!(
                "browser origin host {host} does not match configured RP ID {}",
                auth.rp_id
            ),
        )
        .into());
    }
    auth.origin = request_origin.to_string();
    Ok(auth)
}

fn webauthn_for_challenge(
    config: &ServerConfig,
    challenge: &AuthChallengeRecord,
) -> ApiResult<Webauthn> {
    let mut auth = auth_public_config(config);
    auth.rp_id = challenge.rp_id.clone();
    auth.origin = challenge.origin.clone();
    webauthn_for_auth(&auth)
}

fn webauthn_for_auth(auth: &AuthPublicConfig) -> ApiResult<Webauthn> {
    let origin = Url::parse(&auth.origin)
        .map_err(|error| auth_error("invalid_webauthn_origin", &error.to_string()))?;
    WebauthnBuilder::new(&auth.rp_id, &origin)
        .map_err(|error| auth_error("webauthn_builder_failed", &error.to_string()))?
        .rp_name("Yoi Workspace")
        .build()
        .map_err(|error| auth_error("webauthn_builder_failed", &error.to_string()).into())
}

fn passkeys_for_user(api: &ServerAuthApi, user_id: &str) -> ApiResult<Vec<Passkey>> {
    api.store
        .list_passkey_credentials_for_user(user_id)?
        .into_iter()
        .map(|record| {
            serde_json::from_str::<Passkey>(&record.public_key_cose).map_err(|error| {
                auth_error("webauthn_passkey_deserialize_failed", &error.to_string()).into()
            })
        })
        .collect()
}

fn passkey_credential_id(passkey: &Passkey) -> ApiResult<String> {
    serde_json::to_value(passkey.cred_id())
        .ok()
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
        .ok_or_else(|| {
            auth_error(
                "invalid_webauthn_credential_id",
                "verified passkey credential id was not serializable",
            )
            .into()
        })
}

fn session_cookie_policy(auth: &AuthPublicConfig) -> SessionCookiePolicy<'_> {
    let secure = [&auth.origin, &auth.public_base_url]
        .into_iter()
        .filter_map(|url| reqwest::Url::parse(url).ok())
        .any(|url| url.scheme() == "https");
    SessionCookiePolicy {
        cookie_name: &auth.cookie_name,
        path: "/",
        domain: None,
        secure,
    }
}

fn auth_public_config(config: &ServerConfig) -> AuthPublicConfig {
    match &config.auth {
        AuthConfig::Passkey {
            rp_id,
            origin,
            public_base_url,
            cookie_name,
        } => AuthPublicConfig {
            rp_id: rp_id.clone(),
            origin: origin.clone(),
            public_base_url: public_base_url.clone(),
            cookie_name: cookie_name.clone(),
        },
    }
}

fn ensure_user_account(
    api: &ServerAuthApi,
    handle: &str,
    display_name: Option<&str>,
) -> ApiResult<AuthenticatedUser> {
    let handle = normalize_handle(handle)?;
    if let Some(user) = api.store.get_user_by_handle(&handle)? {
        return Ok(user_response(user));
    }
    let now = crate::auth::now_rfc3339();
    let display_name = display_name
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(handle.as_str())
        .to_string();
    let account = AccountRecord {
        account_id: new_id("acct-user"),
        kind: "user".to_string(),
        handle: handle.clone(),
        display_name: display_name.clone(),
        created_at: now.clone(),
        updated_at: now.clone(),
    };
    api.store.upsert_account(&account)?;
    let user = UserRecord {
        user_id: new_id("user"),
        account_id: account.account_id,
        handle,
        display_name,
        created_at: now.clone(),
        updated_at: now,
    };
    api.store.upsert_user(&user)?;
    Ok(user_response(user))
}

fn user_response(user: UserRecord) -> AuthenticatedUser {
    AuthenticatedUser {
        user_id: user.user_id,
        account_id: user.account_id,
        handle: user.handle,
        display_name: user.display_name,
    }
}

async fn resolve_actor(
    api: &ServerAuthApi,
    headers: &HeaderMap,
) -> ApiResult<Option<RequestActor>> {
    let cookie_name = auth_public_config(&api.config).cookie_name;
    Ok(resolve_request_actor(api.store.as_ref(), headers, &cookie_name).await?)
}

async fn require_actor(api: &ServerAuthApi, headers: &HeaderMap) -> ApiResult<RequestActor> {
    resolve_actor(api, headers).await?.ok_or_else(|| {
        auth_error(
            "auth_required",
            "request requires a browser session or Bearer API token",
        )
        .into()
    })
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
    Query(query): Query<TicketListQuery>,
) -> ApiResult<Json<crate::records::TicketListResponse>> {
    let limit = query.limit.unwrap_or(30).clamp(1, 100);
    let states = query
        .states
        .as_deref()
        .map(|states| {
            states
                .split(',')
                .filter(|state| !state.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let crate::records::TicketSummaryPage {
        items,
        page,
        invalid_records,
        record_authority,
    } = api
        .authority
        .list_ticket_page(crate::records::TicketListPageRequest {
            states,
            limit: Some(limit),
            cursor: query.cursor,
        })?;
    Ok(Json(crate::records::TicketListResponse {
        workspace_id: api.config.workspace_id,
        limit,
        items,
        page,
        invalid_records,
        record_authority,
    }))
}

async fn get_ticket(
    State(api): State<WorkspaceApi>,
    AxumPath(id): AxumPath<String>,
) -> ApiResult<Json<TicketDetail>> {
    Ok(Json(api.authority.ticket(&id)?))
}

async fn list_objectives(
    State(api): State<WorkspaceApi>,
    Query(query): Query<ObjectiveListQuery>,
) -> ApiResult<Json<ListResponse<crate::records::ObjectiveSummary>>> {
    let limit = query.limit.unwrap_or(api.config.max_records).min(1000);
    let ProjectRecordList {
        items,
        invalid_records,
        record_authority,
    } = api.authority.list_objectives(limit)?;
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
    Ok(Json(api.authority.objective(&id)?))
}

async fn list_repositories(
    State(api): State<WorkspaceApi>,
) -> ApiResult<Json<RepositoryListResponse>> {
    let RepositoryListProjection { items, diagnostics } = api.repository_reader().list();
    Ok(Json(RepositoryListResponse {
        workspace_id: api.config.workspace_id,
        items,
        source: "workspace-control-plane".to_string(),
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
        source: "workspace-control-plane".to_string(),
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
) -> ApiResult<Json<workspace_api::ListResponse<workspace_api::RuntimeSummary>>> {
    let limit = api.config.max_records.min(200);
    let runtimes = api.runtime.list_runtimes(limit);
    Ok(Json(workspace_api::ListResponse {
        workspace_id: api.config.workspace_id,
        limit,
        items: runtimes.items.into_iter().map(Into::into).collect(),
        source: "worker_runtime_registry".to_string(),
        diagnostics: runtimes.diagnostics.into_iter().map(Into::into).collect(),
    }))
}

async fn list_workers(
    State(api): State<WorkspaceApi>,
) -> ApiResult<Json<workspace_api::ListResponse<workspace_api::WorkerSummary>>> {
    workers_response(api).map(Json)
}

async fn get_runtime_connection_settings(
    State(api): State<WorkspaceApi>,
) -> ApiResult<Json<RuntimeConnectionSettingsResponse>> {
    let runtime_config = load_backend_runtimes_config_for_settings(&api)?;
    Ok(Json(runtime_connection_settings_response(
        &api,
        &runtime_config,
    )))
}

async fn add_remote_runtime_connection(
    State(api): State<WorkspaceApi>,
    Json(request): Json<AddRemoteRuntimeConnectionRequest>,
) -> ApiResult<Json<RuntimeConnectionMutationResponse>> {
    validate_runtime_connection_request(&request)?;
    let mut runtime_config = load_backend_runtimes_config_for_settings(&api)?;
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
    if runtime_config
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
    runtime_config.runtimes.remote.push(remote_config);
    write_backend_runtimes_config_for_settings(&api, &runtime_config)?;
    api.runtime.register_or_replace(active_runtime);
    let mut response = runtime_connection_mutation_response(
        &api,
        &runtime_config,
        vec![settings_diagnostic(
            "runtime_registry_applied",
            DiagnosticSeverity::Info,
            "Remote Runtime config was persisted and applied to the active Runtime registry without restarting the Workspace backend.",
        )],
    );
    response.diagnostics.push(settings_diagnostic(
        "backend_runtimes_config_rewritten",
        DiagnosticSeverity::Info,
        "Backend runtimes config was rewritten from the typed schema; comments and formatting are not preserved in v0.",
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
    let mut runtime_config = load_backend_runtimes_config_for_settings(&api)?;
    let before = runtime_config.runtimes.remote.len();
    runtime_config
        .runtimes
        .remote
        .retain(|remote| remote.id != runtime_id);
    if before == runtime_config.runtimes.remote.len() {
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
    write_backend_runtimes_config_for_settings(&api, &runtime_config)?;
    let mut response = runtime_connection_mutation_response(
        &api,
        &runtime_config,
        vec![settings_diagnostic(
            "runtime_registry_applied",
            DiagnosticSeverity::Info,
            "Remote Runtime config was removed from persisted config and the active Runtime registry without restarting the Workspace backend.",
        )],
    );
    response.diagnostics.push(settings_diagnostic(
        "backend_runtimes_config_rewritten",
        DiagnosticSeverity::Info,
        "Backend runtimes config was rewritten from the typed schema; comments and formatting are not preserved in v0.",
    ));
    Ok(Json(response))
}

async fn test_remote_runtime_connection(
    State(api): State<WorkspaceApi>,
    AxumPath(runtime_id): AxumPath<String>,
) -> ApiResult<Json<RemoteRuntimeTestResponse>> {
    let runtime_config = load_backend_runtimes_config_for_settings(&api)?;
    let remote = runtime_config
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
    Ok(Json(worker_launch_options_response(&api)?))
}

fn working_directory_request_from_repository(
    repository: &ConfiguredRepository,
    selector: Option<&str>,
) -> WorkingDirectoryRequest {
    WorkingDirectoryRequest {
        repository: WorkingDirectoryRepository {
            id: repository.id.clone(),
            provider: repository.provider.clone(),
            source: repository.source.clone(),
            source_revision: repository.source_revision,
            source_fingerprint: repository.source_fingerprint.clone(),
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
    api: &WorkspaceApi,
    request: &WorkerSpawnWorkingDirectoryRequest,
) -> Result<WorkingDirectoryRequest> {
    if api
        .store
        .get_repository(&api.config.workspace_id, &request.repository_id)?
        .is_none()
    {
        return Err(Error::UnknownRepository(request.repository_id.clone()));
    }
    let repository = api
        .config
        .repositories
        .iter()
        .find(|repository| repository.id == request.repository_id)
        .ok_or_else(|| Error::UnknownRepository(request.repository_id.clone()))?;
    Ok(working_directory_request_from_repository(
        repository,
        request.selector.as_deref(),
    ))
}

fn validate_worker_initial_submit(segments: &[Segment]) -> Result<()> {
    let flow_selectors = segments
        .iter()
        .filter_map(|segment| match segment {
            Segment::Flow { selector } => Some(selector),
            _ => None,
        })
        .collect::<Vec<_>>();
    if flow_selectors.len() > 1 {
        return Err(Error::InvalidInput(
            "initial_submit may contain at most one Flow segment".to_string(),
        ));
    }
    if let Some(selector) = flow_selectors.first() {
        selector
            .parse::<flow::FlowSelector>()
            .map_err(|error| Error::InvalidInput(error.to_string()))?;
    }
    if segments
        .iter()
        .any(|segment| matches!(segment, Segment::Unknown))
    {
        return Err(Error::InvalidInput(
            "initial_submit must not contain unknown segment variants".to_string(),
        ));
    }
    Ok(())
}

fn browser_worker_spawn_policy(
    ticket_assignment: Option<CreateWorkspaceWorkerTicketAssignmentRequest>,
    initial_submit: &[Segment],
) -> Result<(
    WorkerSpawnIntent,
    WorkerSpawnAcceptanceRequirement,
    Option<WorkerTicketAssignmentRequest>,
)> {
    let expected_segments = initial_submit.len();
    match ticket_assignment {
        Some(assignment) => {
            if !initial_submit
                .iter()
                .any(|segment| matches!(segment, Segment::Flow { .. }))
            {
                return Err(Error::InvalidInput(
                    "Ticket-assigned Coder spawn requires one Flow segment in initial_submit"
                        .to_string(),
                ));
            }
            let ticket_id = assignment.ticket_id.trim().to_string();
            if ticket_id.is_empty() {
                return Err(Error::InvalidInput(
                    "ticket_id must not be empty".to_string(),
                ));
            }
            let operation_id = assignment.operation_id.trim().to_string();
            if operation_id.is_empty() {
                return Err(Error::InvalidInput(
                    "assignment operation_id must not be empty".to_string(),
                ));
            }
            Ok((
                WorkerSpawnIntent::TicketRole {
                    ticket_id: ticket_id.clone(),
                    role: TicketWorkerRole::Coder,
                },
                WorkerSpawnAcceptanceRequirement::RunAccepted { expected_segments },
                Some(WorkerTicketAssignmentRequest {
                    ticket_id,
                    operation_id,
                }),
            ))
        }
        None => {
            if initial_submit
                .iter()
                .any(|segment| matches!(segment, Segment::Flow { .. }))
            {
                return Err(Error::InvalidInput(
                    "Workspace Worker Flow spawn requires ticket_assignment; use a typed Ticket-assigned Coder spawn"
                        .to_string(),
                ));
            }
            Ok((
                WorkerSpawnIntent::WorkspaceCoding,
                WorkerSpawnAcceptanceRequirement::RunAccepted { expected_segments },
                None,
            ))
        }
    }
}

async fn create_workspace_worker(
    State(api): State<WorkspaceApi>,
    headers: HeaderMap,
    Json(request): Json<CreateWorkspaceWorkerRequest>,
) -> ApiResult<Json<BrowserCreateWorkerResponse>> {
    let CreateWorkspaceWorkerRequest {
        runtime_id,
        display_name,
        profile,
        ticket_assignment,
        initial_submit,
        working_directory,
        control_operation_id: _,
        resolved_control_operation,
    } = request;
    let config_state = api
        .config_store
        .load_workspace_config(&api.config.workspace_id)?
        .ok_or_else(|| Error::InvalidRecordId("virtual config source tree".into()))?;
    let profile_projection = crate::profile_settings::project_profiles_from_workspace_config(
        &api.config.workspace_id,
        &config_state,
    )?;
    let profile = profile
        .as_deref()
        .map(str::trim)
        .filter(|profile| !profile.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| profile_projection.settings.default_profile.clone())
        .ok_or_else(|| {
            settings_bad_request(
                "workspace_default_profile_missing",
                "profile is required because this Workspace has no default profile configured",
            )
        })?;
    let profile_selector =
        crate::profile_settings::selector_for_workspace_candidate(&profile_projection, &profile)
            .ok_or_else(|| {
                settings_bad_request(
                    "unsupported_worker_profile",
                    "profile must be selected from Backend-published worker profile candidates",
                )
            })?;
    let prompt_catalog = api
        .prompt_projection_cache
        .resolve(&api.config.workspace_id, &config_state)?;
    let resolved_config_bundle =
        crate::profile_settings::build_virtual_profile_config_bundle_with_prompt_projection(
            &profile_projection,
            &config_state,
            &api.config.workspace_id,
            &api.config.workspace_created_at,
            &profile,
            prompt_catalog.as_ref(),
        )?;
    let display_name = sanitize_worker_display_name(&display_name).ok_or_else(|| {
        settings_bad_request(
            "invalid_worker_display_name",
            "display_name must contain at least one non-control character",
        )
    })?;
    if display_name == crate::hosts::WORKSPACE_ORCHESTRATOR_SINGLETON_KEY {
        return Err(Error::ReservedWorkerName(display_name).into());
    }
    validate_worker_initial_submit(&initial_submit)?;
    let source = optional_worker_mutation_source(&api, &api.config.workspace_id, &headers)?;
    reject_orchestrator_generic_flow_spawn(
        &api,
        source.as_ref(),
        &initial_submit,
        ticket_assignment.is_some(),
    )?;
    let selected_working_directory_id = working_directory
        .as_ref()
        .map(|selection| selection.working_directory_id.clone());
    let resolved_working_directory = working_directory.map(|selection| WorkingDirectoryClaim {
        working_directory_id: selection.working_directory_id,
        relative_cwd: selection.relative_cwd,
    });
    validate_working_directory_claim_for_browser(resolved_working_directory.as_ref())?;
    if resolved_working_directory.is_none() {
        reject_no_workdir_for_non_embedded_runtime(&runtime_id)?;
    }
    let (intent, acceptance, ticket_assignment) =
        browser_worker_spawn_policy(ticket_assignment, &initial_submit)?;
    let request = WorkerSpawnRequest {
        requested_worker_name: Some(display_name.clone()),
        intent,
        acceptance,
        profile: profile_selector,
        ticket_assignment,
        initial_submit,
        working_directory_request: None,
        resolved_working_directory_request: None,
        resolved_working_directory,
        resolved_config_bundle,
        resolved_worker_observation_enabled: false,
        resolved_worker_observation_grants: Vec::new(),
        resolved_control_operation,
        resolved_workspace_api: None,
        resolved_memory_settings: None,
    };
    validate_ticket_assignment_spawn(&api, &runtime_id, &request)?;
    let assignment = request.ticket_assignment.clone();
    let assignment_fingerprint = crate::hosts::worker_spawn_idempotency(&request)
        .map_err(Error::Config)?
        .map(|(_, fingerprint)| fingerprint);
    if let (Some(assignment), Some(fingerprint)) =
        (assignment.as_ref(), assignment_fingerprint.as_deref())
    {
        api.store.reserve_ticket_assignment_operation(
            &api.config.workspace_id,
            &assignment.operation_id,
            &assignment.ticket_id,
            &runtime_id,
            None,
            fingerprint,
            &Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
        )?;
        let operation = api
            .store
            .get_ticket_assignment_operation(&api.config.workspace_id, &assignment.operation_id)?
            .ok_or_else(|| {
                Error::TicketAssignmentConflict(format!(
                    "assignment operation {} disappeared after reservation",
                    assignment.operation_id
                ))
            })?;
        if let Some(worker) = existing_lifecycle_assignment_worker(&api, assignment, &runtime_id)? {
            return Ok(Json(browser_worker_response_from_summary(
                &api,
                worker,
                display_name,
                selected_working_directory_id.as_deref(),
                Vec::new(),
                Some(assignment),
            )?));
        }
        if operation.assignment_id.is_some() {
            return Err(Error::TicketAssignmentConflict(format!(
                "completed operation {} has no recoverable Worker",
                assignment.operation_id
            ))
            .into());
        }
    }
    let result = match api.spawn_workspace_worker(&runtime_id, request) {
        Ok(result) => result,
        Err(error) => return Err(error.into()),
    };
    Ok(Json(record_browser_worker_spawn(
        &api,
        runtime_id,
        display_name,
        selected_working_directory_id,
        result,
        assignment.as_ref(),
    )?))
}

fn record_browser_worker_spawn(
    api: &WorkspaceApi,
    requested_runtime_id: String,
    display_name: String,
    selected_working_directory_id: Option<String>,
    result: WorkerSpawnResult,
    assignment: Option<&WorkerTicketAssignmentRequest>,
) -> ApiResult<BrowserCreateWorkerResponse> {
    if result.state != WorkerOperationState::Accepted {
        return Err(worker_create_not_accepted_error(
            requested_runtime_id.clone(),
            result.diagnostics,
        ));
    }
    let worker = result.worker.ok_or_else(|| Error::RuntimeOperationFailed {
        runtime_id: requested_runtime_id,
        code: "workspace_worker_create_missing_summary".to_string(),
        message: "Runtime completed worker creation without returning a Worker summary".to_string(),
    })?;
    browser_worker_response_from_summary(
        api,
        worker,
        display_name,
        selected_working_directory_id.as_deref(),
        result.diagnostics,
        assignment,
    )
}

fn browser_worker_response_from_summary(
    api: &WorkspaceApi,
    worker: WorkerSummary,
    display_name: String,
    selected_working_directory_id: Option<&str>,
    diagnostics: Vec<RuntimeDiagnostic>,
    assignment: Option<&WorkerTicketAssignmentRequest>,
) -> ApiResult<BrowserCreateWorkerResponse> {
    let worker_record = match record_worker_summary(
        api,
        &worker,
        display_name.as_str(),
        worker.profile.clone(),
        WorkerRegistryDisplayNamePolicy::UseProvided,
    ) {
        Ok(record) => record,
        Err(error) => return Err(error.into()),
    };
    if let Some(assignment) = assignment {
        if let Err(error) = api.store.bind_ticket_assignment_operation_worker(
            &api.config.workspace_id,
            &assignment.operation_id,
            &worker.worker.worker_id,
        ) {
            let context = WorkerSpawnCompensationContext {
                assignment: Some(assignment),
                prepared_workdir_id: selected_working_directory_id,
                cleanup_spawned_workdir: false,
            };
            return finalize_worker_spawn_stage(
                api,
                &worker,
                &context,
                WorkerSpawnFinalizeStage::TicketAssignmentBind,
                Err(error.into()),
            );
        }
        if let Err(error) = assign_ticket_worker_from_lifecycle(
            api,
            assignment,
            &worker.worker.runtime_id,
            &worker.worker.worker_id,
        ) {
            let context = WorkerSpawnCompensationContext {
                assignment: Some(assignment),
                prepared_workdir_id: selected_working_directory_id,
                cleanup_spawned_workdir: false,
            };
            return finalize_worker_spawn_stage(
                api,
                &worker,
                &context,
                WorkerSpawnFinalizeStage::TicketAssignmentBind,
                Err(error.into()),
            );
        }
    }
    if let Some(working_directory) = worker.working_directory.as_ref() {
        let workdir_record =
            workdir_record_from_summary(api, worker.worker.runtime_id.as_str(), working_directory);
        api.store.upsert_workdir_registry(&workdir_record)?;
        link_worker_to_workdir(
            api,
            &worker_record,
            &working_directory.working_directory_id,
            None,
        )?;
    }
    if let Some(workdir_id) = selected_working_directory_id {
        if api
            .store
            .get_workdir_registry(&api.config.workspace_id, workdir_id)?
            .is_none()
        {
            if let Ok(result) = api
                .runtime
                .working_directory(worker.worker.runtime_id.as_str(), workdir_id)
                .map_err(|err| err.into_error())
            {
                if let Some(status) = result.working_directory {
                    let record = workdir_record_from_summary(
                        api,
                        worker.worker.runtime_id.as_str(),
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
            link_worker_to_workdir(api, &worker_record, workdir_id, None)?;
        }
    }
    if let Some(assignment) = assignment {
        let context = WorkerSpawnCompensationContext {
            assignment: Some(assignment),
            prepared_workdir_id: selected_working_directory_id,
            cleanup_spawned_workdir: false,
        };
        finalize_worker_spawn_stage(
            api,
            &worker,
            &context,
            WorkerSpawnFinalizeStage::TicketStateAccept,
            accept_queued_ticket_after_worker_spawn(api, assignment).map_err(ApiError::from),
        )?;
    }
    let runtime_id = worker.worker.runtime_id.clone();
    let worker_id = worker.worker.worker_id.clone();
    let workspace_id = api.workspace_id().to_string();
    let console_href = format!(
        "/w/{}/runtimes/{}/workers/{}/console",
        encode_path_segment(&workspace_id),
        encode_path_segment(&runtime_id),
        encode_path_segment(&worker_id)
    );
    Ok(BrowserCreateWorkerResponse {
        workspace_id,
        worker_ref: RuntimeWorkerRef::new(&runtime_id, &worker_id),
        console_href,
        worker,
        diagnostics,
    })
}

async fn scoped_post_internal_runtime_resource_fetch(
    State(api): State<WorkspaceApi>,
    AxumPath(workspace_id): AxumPath<String>,
    headers: HeaderMap,
    request: Request,
) -> std::result::Result<
    Json<worker_runtime::resource::BackendResourceFetchResponse>,
    (StatusCode, Json<BackendResourceError>),
> {
    if workspace_id != api.workspace_id() {
        return Err((
            StatusCode::NOT_FOUND,
            Json(BackendResourceError::MissingResource),
        ));
    }
    let proof = headers
        .get(worker_runtime::auth::RUNTIME_REQUEST_SOURCE_PROOF_HEADER)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            (
                StatusCode::UNAUTHORIZED,
                Json(BackendResourceError::Unauthorized {
                    message: "Runtime request proof is required".to_owned(),
                }),
            )
        })?;
    let verified_source = request
        .extensions()
        .get::<crate::worker_source::VerifiedRuntimeRequestSource>()
        .cloned();
    let method = request.method().as_str().to_owned();
    let path = signed_request_target(request.uri()).to_owned();
    let body = axum::body::to_bytes(request.into_body(), 16 * 1024 * 1024)
        .await
        .map_err(|error| {
            (
                StatusCode::BAD_REQUEST,
                Json(BackendResourceError::InvalidResponse {
                    message: error.to_string(),
                }),
            )
        })?;
    let source = if let Some(source) = verified_source {
        source
    } else {
        crate::worker_source::verify_runtime_request_source_proof(
            &api,
            proof,
            &workspace_id,
            worker_runtime::auth::BACKEND_RESOURCE_FETCH_PERMISSION,
            &method,
            &path,
            &worker_runtime::auth::request_body_digest(&body),
        )
        .await
        .map_err(|_| {
            (
                StatusCode::UNAUTHORIZED,
                Json(BackendResourceError::Unauthorized {
                    message: "Runtime request proof is invalid".to_owned(),
                }),
            )
        })?
    };
    if source.worker_id.is_some() {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(BackendResourceError::Unauthorized {
                message: "Worker-scoped proof cannot fetch Runtime resources".to_owned(),
            }),
        ));
    }
    let request: BackendResourceFetchRequest = serde_json::from_slice(&body).map_err(|error| {
        (
            StatusCode::BAD_REQUEST,
            Json(BackendResourceError::InvalidResponse {
                message: error.to_string(),
            }),
        )
    })?;
    if request.runtime_id != source.runtime_id {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(BackendResourceError::Unauthorized {
                message: "Runtime request proof subject does not match the request".to_owned(),
            }),
        ));
    }
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

#[derive(Debug, Serialize)]
struct WorkerShowProjection {
    #[serde(flatten)]
    worker: workspace_api::WorkerSummary,
    updated_at: String,
}

fn resolve_workspace_worker_reference(
    api: &WorkspaceApi,
    runtime_id: &str,
    reference: &str,
) -> ApiResult<RuntimeWorkerRef> {
    let worker_id = api
        .store
        .resolve_resource_reference(
            &api.config.workspace_id,
            WorkspaceResourceKind::Worker,
            reference,
        )?
        .ok_or_else(|| Error::UnknownWorker {
            worker: RuntimeWorkerRef::new(runtime_id, reference),
        })?;
    let worker = RuntimeWorkerRef::new(runtime_id, worker_id);
    let record = api
        .store
        .get_worker_registry(&api.config.workspace_id, &worker)?
        .ok_or_else(|| Error::UnknownWorker {
            worker: RuntimeWorkerRef::new(runtime_id, reference),
        })?;
    Ok(record.worker)
}

async fn get_runtime_worker(
    State(api): State<WorkspaceApi>,
    AxumPath((runtime_id, worker_id)): AxumPath<(String, String)>,
) -> ApiResult<Json<WorkerShowProjection>> {
    let worker_ref = resolve_workspace_worker_reference(&api, &runtime_id, &worker_id)?;
    let worker = api
        .runtime
        .worker(&worker_ref)
        .map_err(|err| err.into_error())?;
    let record = sync_worker_observation(&api, &worker)?;
    let links = api
        .store
        .list_worker_workdir_links(&api.config.workspace_id, &record.worker)?;
    let workdirs = api
        .store
        .list_workdir_registry(&api.config.workspace_id, 500)?;
    let updated_at = record.updated_at.clone();
    let worker = merge_worker_registry_projection(Some(&worker), &record, links, &workdirs);
    let worker = project_workspace_worker(&api, worker)?;
    Ok(Json(WorkerShowProjection { worker, updated_at }))
}

async fn restore_runtime_worker(
    State(api): State<WorkspaceApi>,
    AxumPath((runtime_id, worker_id)): AxumPath<(String, String)>,
) -> ApiResult<Json<workspace_api::WorkerRestoreResponse>> {
    let worker = resolve_workspace_worker_reference(&api, &runtime_id, &worker_id)?;
    let result = api.restore_workspace_worker(&worker)?;
    let projected_worker = if let Some(worker) = result.worker.as_ref() {
        let record = sync_worker_observation(&api, worker)?;
        let links = api
            .store
            .list_worker_workdir_links(&api.config.workspace_id, &record.worker)?;
        let workdirs = api
            .store
            .list_workdir_registry(&api.config.workspace_id, 500)?;
        let summary = merge_worker_registry_projection(Some(worker), &record, links, &workdirs);
        Some(project_workspace_worker(&api, summary)?)
    } else {
        None
    };
    Ok(Json(workspace_api::WorkerRestoreResponse {
        workspace_id: api.workspace_id().to_string(),
        runtime_id: runtime_id.clone(),
        worker_id: worker_id.clone(),
        result: workspace_api::WorkerRestoreResult {
            state: result.state.into(),
            worker: projected_worker,
            diagnostics: result.diagnostics.into_iter().map(Into::into).collect(),
        },
    }))
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RuntimeConfigBundleSyncRequest {
    pub bundle: ConfigBundle,
}

#[derive(Debug, Deserialize)]
struct RuntimeConfigBundleAvailabilityQuery {
    digest: String,
}

fn reject_workdir_for_embedded_runtime(runtime_id: &str, has_workdir: bool) -> ApiResult<()> {
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

fn reject_no_workdir_for_non_embedded_runtime(runtime_id: &str) -> ApiResult<()> {
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
    Query(query): Query<RuntimeWorkersQuery>,
) -> ApiResult<Json<workspace_api::ListResponse<workspace_api::WorkerSummary>>> {
    let limit = api.config.max_records.min(200);
    let (runtime_workers, source) = match query.status {
        Some(RuntimeWorkersStatusFilter::Stopped) => (
            api.runtime
                .list_stopped_workers_for_runtime(&runtime_id, limit)
                .map_err(|error| error.into_error())?,
            "runtime_registry_stopped",
        ),
        None => (
            api.runtime
                .list_workers_for_runtime(&runtime_id, limit)
                .map_err(|error| error.into_error())?,
            "runtime_registry",
        ),
    };
    let items = project_observed_workspace_workers(&api, runtime_workers.items)?;
    Ok(Json(workspace_api::ListResponse {
        workspace_id: api.workspace_id().to_string(),
        limit,
        items,
        source: source.to_string(),
        diagnostics: runtime_workers
            .diagnostics
            .into_iter()
            .map(Into::into)
            .collect(),
    }))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WorkerSpawnFinalizeStage {
    WorkerRegistry,
    TicketAssignmentBind,
    TicketAssignmentCurrent,
    TicketStateAccept,
    WorkdirRegistry,
    WorkdirAttachment,
}

impl WorkerSpawnFinalizeStage {
    fn as_str(self) -> &'static str {
        match self {
            Self::WorkerRegistry => "worker_registry",
            Self::TicketAssignmentBind => "ticket_assignment_bind",
            Self::TicketAssignmentCurrent => "ticket_assignment_current",
            Self::TicketStateAccept => "ticket_state_accept",
            Self::WorkdirRegistry => "workdir_registry",
            Self::WorkdirAttachment => "workdir_attachment",
        }
    }
}

struct WorkerSpawnCompensationContext<'a> {
    assignment: Option<&'a crate::hosts::WorkerTicketAssignmentRequest>,
    prepared_workdir_id: Option<&'a str>,
    cleanup_spawned_workdir: bool,
}

fn finalize_worker_spawn_stage<T>(
    api: &WorkspaceApi,
    worker: &WorkerSummary,
    context: &WorkerSpawnCompensationContext<'_>,
    stage: WorkerSpawnFinalizeStage,
    result: ApiResult<T>,
) -> ApiResult<T> {
    let Err(source) = result else {
        return result;
    };
    let source_message = sanitize_backend_error(&source.error.to_string());
    let operation_id = context
        .assignment
        .map(|assignment| assignment.operation_id.as_str())
        .unwrap_or("none");
    let mut diagnostics = vec![RuntimeDiagnostic {
        code: format!("worker_spawn_finalize_{}_failed", stage.as_str()),
        severity: DiagnosticSeverity::Error,
        message: format!(
            "Worker spawn finalize failed at stage `{}` for Runtime Worker {}:{} (operation `{operation_id}`): {source_message}",
            stage.as_str(),
            worker.worker.runtime_id,
            worker.worker.worker_id
        ),
    }];
    diagnostics.extend(source.diagnostics);
    let compensation_errors = compensate_failed_worker_spawn(api, worker, context);
    let compensation_failed = !compensation_errors.is_empty();
    diagnostics.extend(compensation_errors);
    if !compensation_failed {
        diagnostics.push(RuntimeDiagnostic {
            code: "worker_spawn_compensated".to_string(),
            severity: DiagnosticSeverity::Info,
            message: format!(
                "Removed Runtime Worker {}:{} and rolled back Backend spawn state",
                worker.worker.runtime_id, worker.worker.worker_id
            ),
        });
    }
    let compensation = if compensation_failed {
        "compensation left residual state; inspect diagnostics"
    } else {
        "compensation completed"
    };
    Err(ApiError::with_diagnostics(
        Error::RuntimeOperationFailed {
            runtime_id: worker.worker.runtime_id.clone(),
            code: format!("worker_spawn_finalize_{}_failed", stage.as_str()),
            message: format!(
                "Worker spawn finalize failed at stage `{}` for Runtime Worker {}:{} (operation `{operation_id}`): {source_message}; {compensation}",
                stage.as_str(),
                worker.worker.runtime_id,
                worker.worker.worker_id
            ),
        },
        diagnostics,
    ))
}

fn append_attachment_reservation_release_diagnostic(
    api: &WorkspaceApi,
    workdir_id: &str,
    reservation_id: &str,
    error: &mut ApiError,
) {
    if let Err(release_error) = api.store.release_worker_workdir_attachment_reservation(
        &api.config.workspace_id,
        workdir_id,
        reservation_id,
    ) {
        error.diagnostics.push(spawn_compensation_diagnostic(
            "worker_spawn_compensation_attachment_reservation_release_failed",
            format!(
                "Failed to release Workdir `{workdir_id}` attachment reservation `{reservation_id}`: {}",
                sanitize_backend_error(&release_error.to_string())
            ),
        ));
    }
}

fn compensate_failed_worker_spawn(
    api: &WorkspaceApi,
    worker: &WorkerSummary,
    context: &WorkerSpawnCompensationContext<'_>,
) -> Vec<RuntimeDiagnostic> {
    let mut diagnostics = Vec::new();
    let lifecycle_request = WorkerLifecycleRequest {
        reason: Some("Backend spawn finalize failed; compensating Runtime Worker".to_string()),
        ticket_assignment: None,
    };
    let cancellation = api
        .runtime
        .cancel_worker(&worker.worker, lifecycle_request.clone());
    let cancellation_accepted = cancellation
        .as_ref()
        .is_ok_and(|result| result.state == WorkerOperationState::Accepted);
    let stop = (!cancellation_accepted)
        .then(|| api.runtime.stop_worker(&worker.worker, lifecycle_request));
    let stop_accepted = stop.as_ref().is_some_and(|result| {
        result
            .as_ref()
            .is_ok_and(|result| result.state == WorkerOperationState::Accepted)
    });
    let termination_detail = (!cancellation_accepted && !stop_accepted).then(|| {
        let cancellation = lifecycle_failure_detail("cancel", &cancellation);
        let stop = stop
            .as_ref()
            .map(|result| lifecycle_failure_detail("stop", result))
            .unwrap_or_else(|| "stop was not attempted".to_string());
        format!("{cancellation}; {stop}")
    });

    let runtime_deleted = match api.runtime.delete_worker(&worker.worker) {
        Ok(result) if result.state == WorkerOperationState::Accepted && result.deleted => true,
        Ok(result) => {
            let mut message = format!(
                "Runtime did not delete Worker {}:{}: state={:?}, deleted={}",
                worker.worker.runtime_id, worker.worker.worker_id, result.state, result.deleted
            );
            if let Some(detail) = termination_detail.as_deref() {
                message.push_str(&format!("; cancellation: {detail}"));
            }
            if !result.diagnostics.is_empty() {
                message.push_str(&format!(
                    "; delete diagnostics: {}",
                    runtime_diagnostics_message(&result.diagnostics)
                ));
            }
            diagnostics.push(spawn_compensation_diagnostic(
                "worker_spawn_compensation_runtime_delete_failed",
                message,
            ));
            false
        }
        Err(RuntimeRegistryError::UnknownWorker { .. }) => true,
        Err(error) => {
            let mut message = format!(
                "Failed to delete Runtime Worker {}:{}: {}",
                worker.worker.runtime_id,
                worker.worker.worker_id,
                error.message()
            );
            if let Some(detail) = termination_detail.as_deref() {
                message.push_str(&format!("; cancellation: {detail}"));
            }
            diagnostics.push(spawn_compensation_diagnostic(
                "worker_spawn_compensation_runtime_delete_failed",
                message,
            ));
            false
        }
    };
    if !runtime_deleted {
        return diagnostics;
    }
    diagnostics.extend(finalize_spawn_compensation_after_worker_delete(
        api, worker, context,
    ));
    diagnostics
}

fn finalize_spawn_compensation_after_worker_delete(
    api: &WorkspaceApi,
    worker: &WorkerSummary,
    context: &WorkerSpawnCompensationContext<'_>,
) -> Vec<RuntimeDiagnostic> {
    let mut diagnostics = Vec::new();
    if let Some(assignment) = context.assignment {
        if let Err(error) = api.store.rollback_ticket_assignment_operation(
            &api.config.workspace_id,
            &assignment.operation_id,
        ) {
            diagnostics.push(spawn_compensation_diagnostic(
                "worker_spawn_compensation_assignment_rollback_failed",
                format!(
                    "Failed to roll back Ticket assignment operation `{}` for Worker {}:{}: {}",
                    assignment.operation_id,
                    worker.worker.runtime_id,
                    worker.worker.worker_id,
                    sanitize_backend_error(&error.to_string())
                ),
            ));
        }
    }
    if let Err(error) = api
        .store
        .delete_worker_registry(&api.config.workspace_id, &worker.worker)
    {
        diagnostics.push(spawn_compensation_diagnostic(
            "worker_spawn_compensation_registry_delete_failed",
            format!(
                "Failed to remove Backend Worker registry for {}:{}: {}",
                worker.worker.runtime_id,
                worker.worker.worker_id,
                sanitize_backend_error(&error.to_string())
            ),
        ));
    }

    if context.cleanup_spawned_workdir {
        if let Some(workdir_id) = context.prepared_workdir_id {
            let runtime_cleanup_succeeded = match api
                .runtime
                .cleanup_working_directory(&worker.worker.runtime_id, workdir_id)
            {
                Ok(result) if result.state == WorkerOperationState::Accepted => true,
                Ok(result) => {
                    diagnostics.push(spawn_compensation_diagnostic(
                        "worker_spawn_compensation_workdir_cleanup_failed",
                        format!(
                            "Runtime did not clean up spawn-created Workdir `{workdir_id}` for Worker {}:{}: state={:?}; {}",
                            worker.worker.runtime_id,
                            worker.worker.worker_id,
                            result.state,
                            runtime_diagnostics_message(&result.diagnostics)
                        ),
                    ));
                    false
                }
                Err(error) => {
                    diagnostics.push(spawn_compensation_diagnostic(
                        "worker_spawn_compensation_workdir_cleanup_failed",
                        format!(
                            "Failed to clean up spawn-created Workdir `{workdir_id}` for Worker {}:{}: {}",
                            worker.worker.runtime_id,
                            worker.worker.worker_id,
                            error.message()
                        ),
                    ));
                    false
                }
            };
            if runtime_cleanup_succeeded {
                if let Err(error) = api
                    .store
                    .delete_workdir_registry(&api.config.workspace_id, workdir_id)
                {
                    diagnostics.push(spawn_compensation_diagnostic(
                        "worker_spawn_compensation_workdir_registry_delete_failed",
                        format!(
                            "Failed to remove Backend Workdir registry `{workdir_id}` after Runtime cleanup: {}",
                            sanitize_backend_error(&error.to_string())
                        ),
                    ));
                }
            }
        }
    }
    diagnostics
}

fn lifecycle_failure_detail(
    action: &str,
    result: &std::result::Result<WorkerLifecycleResult, RuntimeRegistryError>,
) -> String {
    match result {
        Ok(result) => format!(
            "Runtime {action} returned state={:?}: {}",
            result.state,
            runtime_diagnostics_message(&result.diagnostics)
        ),
        Err(error) => format!("Runtime {action} failed: {}", error.message()),
    }
}

fn spawn_compensation_diagnostic(code: &str, message: String) -> RuntimeDiagnostic {
    RuntimeDiagnostic {
        code: code.to_string(),
        severity: DiagnosticSeverity::Error,
        message,
    }
}

fn runtime_diagnostics_message(diagnostics: &[RuntimeDiagnostic]) -> String {
    if diagnostics.is_empty() {
        "no Runtime diagnostics".to_string()
    } else {
        diagnostics
            .iter()
            .map(|diagnostic| format!("{}: {}", diagnostic.code, diagnostic.message))
            .collect::<Vec<_>>()
            .join("; ")
    }
}

async fn create_runtime_worker(
    State(api): State<WorkspaceApi>,
    AxumPath(runtime_id): AxumPath<String>,
    Json(mut request): Json<WorkerSpawnRequest>,
) -> ApiResult<Json<WorkerSpawnResult>> {
    validate_worker_initial_submit(&request.initial_submit)?;
    if let Some(assignment) = request.ticket_assignment.as_ref()
        && let Some(worker) = existing_lifecycle_assignment_worker(&api, assignment, &runtime_id)?
    {
        validate_ticket_assignment_spawn(&api, &runtime_id, &request)?;
        assign_ticket_worker_from_lifecycle(
            &api,
            assignment,
            &runtime_id,
            &worker.worker.worker_id,
        )?;
        accept_queued_ticket_after_worker_spawn(&api, assignment)?;
        return Ok(Json(WorkerSpawnResult {
            state: WorkerOperationState::Accepted,
            worker: Some(worker),
            acceptance_evidence: Vec::new(),
            diagnostics: Vec::new(),
        }));
    }
    let lifecycle_assignment = request.ticket_assignment.clone();
    validate_ticket_assignment_spawn(&api, &runtime_id, &request)?;
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
        .map(|working_directory| configured_working_directory_request(&api, working_directory))
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
    let spawn_idempotency =
        crate::hosts::worker_spawn_idempotency(&request).map_err(Error::Config)?;
    if let (Some(assignment), Some((_, fingerprint))) =
        (lifecycle_assignment.as_ref(), spawn_idempotency.as_ref())
    {
        api.store.reserve_ticket_assignment_operation(
            &api.config.workspace_id,
            &assignment.operation_id,
            &assignment.ticket_id,
            &runtime_id,
            None,
            fingerprint,
            &Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
        )?;
    }
    let cleanup_spawned_workdir = request.resolved_working_directory_request.is_some();
    let result = api.spawn_workspace_worker(&runtime_id, request)?;
    if let Some(worker) = result.worker.as_ref() {
        let compensation = WorkerSpawnCompensationContext {
            assignment: lifecycle_assignment.as_ref(),
            prepared_workdir_id: prepared_workdir_id.as_deref(),
            cleanup_spawned_workdir,
        };
        let display_name = requested_worker_name
            .as_deref()
            .filter(|name| !name.trim().is_empty())
            .unwrap_or(worker.label.as_str())
            .to_string();
        let record = finalize_worker_spawn_stage(
            &api,
            worker,
            &compensation,
            WorkerSpawnFinalizeStage::WorkerRegistry,
            record_worker_summary(
                &api,
                worker,
                display_name.as_str(),
                worker.profile.clone(),
                WorkerRegistryDisplayNamePolicy::UseProvided,
            ),
        )?;
        if let Some(assignment) = lifecycle_assignment.as_ref() {
            finalize_worker_spawn_stage(
                &api,
                worker,
                &compensation,
                WorkerSpawnFinalizeStage::TicketAssignmentBind,
                api.store
                    .bind_ticket_assignment_operation_worker(
                        &api.config.workspace_id,
                        &assignment.operation_id,
                        &worker.worker.worker_id,
                    )
                    .map_err(ApiError::from),
            )?;
            finalize_worker_spawn_stage(
                &api,
                worker,
                &compensation,
                WorkerSpawnFinalizeStage::TicketAssignmentCurrent,
                assign_ticket_worker_from_lifecycle(
                    &api,
                    assignment,
                    &runtime_id,
                    &worker.worker.worker_id,
                )
                .map_err(ApiError::from),
            )?;
            finalize_worker_spawn_stage(
                &api,
                worker,
                &compensation,
                WorkerSpawnFinalizeStage::TicketStateAccept,
                accept_queued_ticket_after_worker_spawn(&api, assignment).map_err(ApiError::from),
            )?;
        }
        if worker.working_directory.is_none() {
            if let Some(workdir_id) = prepared_workdir_id.as_deref() {
                let workdir_exists = finalize_worker_spawn_stage(
                    &api,
                    worker,
                    &compensation,
                    WorkerSpawnFinalizeStage::WorkdirRegistry,
                    api.store
                        .get_workdir_registry(&api.config.workspace_id, workdir_id)
                        .map_err(ApiError::from),
                )?
                .is_some();
                if workdir_exists {
                    finalize_worker_spawn_stage(
                        &api,
                        worker,
                        &compensation,
                        WorkerSpawnFinalizeStage::WorkdirAttachment,
                        link_worker_to_workdir(&api, &record, workdir_id, None),
                    )?;
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
    let worker = resolve_workspace_worker_reference(&api, &runtime_id, &worker_id)?;
    let result = api
        .runtime
        .send_input(&worker, request)
        .map_err(|err| err.into_error())?;
    Ok(Json(result))
}

async fn runtime_worker_completions(
    State(api): State<WorkspaceApi>,
    AxumPath((runtime_id, worker_id)): AxumPath<(String, String)>,
    Json(request): Json<WorkerCompletionsRequest>,
) -> ApiResult<Json<WorkerCompletionsResult>> {
    let worker = resolve_workspace_worker_reference(&api, &runtime_id, &worker_id)?;
    let result = api
        .runtime
        .worker_completions(&worker, request)
        .map_err(|err| err.into_error())?;
    Ok(Json(result))
}

async fn stop_runtime_worker(
    State(api): State<WorkspaceApi>,
    AxumPath((runtime_id, worker_id)): AxumPath<(String, String)>,
    Json(request): Json<WorkerLifecycleRequest>,
) -> ApiResult<Json<WorkerLifecycleResult>> {
    let worker = resolve_workspace_worker_reference(&api, &runtime_id, &worker_id)?;
    let result = api
        .runtime
        .stop_worker(&worker, request)
        .map_err(|err| err.into_error())?;
    parse_runtime_worker_id_for_registry(&worker.worker_id)?;
    let session_lock = current_worker_session_lock(&api, &worker);
    let _session_guard = session_lock.lock().await;
    close_current_worker_session_locked(&api, &worker).await?;
    if let Some(record) = api
        .store
        .get_worker_registry(&api.config.workspace_id, &worker)?
    {
        sync_linked_workdir_after_worker_stop(&api, &worker.runtime_id, &record)?;
    }
    Ok(Json(result))
}

async fn cancel_runtime_worker(
    State(api): State<WorkspaceApi>,
    AxumPath((runtime_id, worker_id)): AxumPath<(String, String)>,
    Json(request): Json<WorkerLifecycleRequest>,
) -> ApiResult<Json<WorkerLifecycleResult>> {
    let worker = resolve_workspace_worker_reference(&api, &runtime_id, &worker_id)?;
    let result = api
        .runtime
        .cancel_worker(&worker, request)
        .map_err(|err| err.into_error())?;
    Ok(Json(result))
}

async fn worker_protocol_ws(
    State(api): State<WorkspaceApi>,
    AxumPath((runtime_id, worker_id)): AxumPath<(String, String)>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    let worker = RuntimeWorkerRef::new(&runtime_id, &worker_id);
    let source = match api.observation_proxy.source(&worker) {
        Ok(source) => source,
        Err(ObservationProxyError::WorkerNotFound(_)) => {
            match api.runtime.observation_source(&worker) {
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

pub(crate) struct WorkspaceWorkerProtocolConnection {
    pub(crate) methods: tokio::sync::mpsc::Sender<protocol::Method>,
    pub(crate) events: tokio::sync::mpsc::Receiver<protocol::Event>,
}

pub(crate) async fn connect_workspace_worker_protocol(
    api: &WorkspaceApi,
    worker: &RuntimeWorkerRef,
) -> Result<WorkspaceWorkerProtocolConnection> {
    let source = match api.observation_proxy.source(worker) {
        Ok(source) => source,
        Err(ObservationProxyError::WorkerNotFound(_)) => api
            .runtime
            .observation_source(worker)
            .map_err(|error| error.into_error())?,
        Err(error) => {
            return Err(Error::RuntimeOperationFailed {
                runtime_id: worker.runtime_id.clone(),
                code: error.code().to_string(),
                message: error.message().to_string(),
            });
        }
    };
    match source {
        RuntimeObservationSource::RemoteWs(config) => connect_remote_worker_protocol(config).await,
        RuntimeObservationSource::Embedded(source) => {
            connect_embedded_worker_protocol(source).await
        }
    }
}

async fn connect_remote_worker_protocol(
    config: RuntimeObservationSourceConfig,
) -> Result<WorkspaceWorkerProtocolConnection> {
    let mut request = config
        .endpoint
        .clone()
        .into_client_request()
        .map_err(|error| Error::Config(format!("invalid Runtime protocol endpoint: {error}")))?;
    if let Some(token) = &config.bearer_token {
        request.headers_mut().insert(
            "authorization",
            format!("Bearer {token}").parse().map_err(|error| {
                Error::Config(format!("invalid Runtime authorization: {error}"))
            })?,
        );
    }
    let (socket, _) =
        connect_async(request)
            .await
            .map_err(|error| Error::RuntimeOperationFailed {
                runtime_id: config.worker.runtime_id.clone(),
                code: "worker_protocol_connect_failed".to_string(),
                message: error.to_string(),
            })?;
    let (mut sink, mut stream) = socket.split();
    let (methods, mut method_receiver) = tokio::sync::mpsc::channel(256);
    let (event_sender, events) = tokio::sync::mpsc::channel(512);
    tokio::spawn(async move {
        loop {
            tokio::select! {
                method = method_receiver.recv() => {
                    let Some(method) = method else { break };
                    let Ok(text) = protocol::stream::encode_method(&method) else { break };
                    if sink.send(TungsteniteMessage::Text(text.into())).await.is_err() { break; }
                }
                message = stream.next() => match message {
                    Some(Ok(TungsteniteMessage::Text(text))) => {
                        let Ok(event) = protocol::stream::decode_event(text.as_ref()) else { break; };
                        if event_sender.send(event).await.is_err() { break; }
                    }
                    Some(Ok(TungsteniteMessage::Ping(value))) => {
                        if sink.send(TungsteniteMessage::Pong(value)).await.is_err() { break; }
                    }
                    Some(Ok(TungsteniteMessage::Pong(_))) => {}
                    _ => break,
                }
            }
        }
    });
    Ok(WorkspaceWorkerProtocolConnection { methods, events })
}

async fn connect_embedded_worker_protocol(
    source: crate::observation::EmbeddedRuntimeObservationSource,
) -> Result<WorkspaceWorkerProtocolConnection> {
    let mut upstream =
        RuntimeObservationClient::connect(&RuntimeObservationSource::Embedded(source.clone()))
            .await
            .map_err(|error| Error::RuntimeOperationFailed {
                runtime_id: source.worker.runtime_id.clone(),
                code: error.code().to_string(),
                message: error.message().to_string(),
            })?;
    let (methods, mut method_receiver) = tokio::sync::mpsc::channel(256);
    let (event_sender, events) = tokio::sync::mpsc::channel(512);
    tokio::spawn(async move {
        loop {
            tokio::select! {
                method = method_receiver.recv() => {
                    let Some(method) = method else { break };
                    match source.runtime.send_protocol_method(&source.worker_ref, method) {
                        Ok(direct_events) => {
                            for event in direct_events {
                                if event_sender.send(event).await.is_err() { return; }
                            }
                        }
                        Err(error) => {
                            if event_sender.send(protocol_error_event(error.to_string())).await.is_err() { return; }
                        }
                    }
                }
                event = upstream.next_event() => match event {
                    Ok(event) => {
                        if event_sender.send(event.payload).await.is_err() { break; }
                    }
                    Err(_) => break,
                }
            }
        }
    });
    Ok(WorkspaceWorkerProtocolConnection { methods, events })
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
) -> ApiResult<Json<workspace_api::ListResponse<workspace_api::WorkerSummary>>> {
    let limit = api.config.max_records.min(200);
    let runtime_workers = api
        .runtime
        .list_workers_for_host(&host_id, limit)
        .map_err(|err| err.into_error())?;
    let items = project_observed_workspace_workers(&api, runtime_workers.items)?;
    Ok(Json(workspace_api::ListResponse {
        workspace_id: api.workspace_id().to_string(),
        limit,
        items,
        source: "worker_runtime_registry".to_string(),
        diagnostics: runtime_workers
            .diagnostics
            .into_iter()
            .map(Into::into)
            .collect(),
    }))
}

fn project_workspace_worker(
    api: &WorkspaceApi,
    summary: WorkerSummary,
) -> ApiResult<workspace_api::WorkerSummary> {
    let resource_key = api
        .store
        .resource_key(
            &api.config.workspace_id,
            WorkspaceResourceKind::Worker,
            &summary.worker.worker_id,
        )?
        .ok_or_else(|| {
            Error::Store(format!(
                "Workspace Worker `{}` has no resource key",
                summary.worker.worker_id
            ))
        })?;
    Ok(workspace_worker_summary(summary, resource_key))
}

fn project_observed_workspace_workers(
    api: &WorkspaceApi,
    workers: Vec<WorkerSummary>,
) -> ApiResult<Vec<workspace_api::WorkerSummary>> {
    let workdirs = api
        .store
        .list_workdir_registry(&api.config.workspace_id, 500)?;
    workers
        .into_iter()
        .map(|worker| {
            let record = sync_worker_observation(api, &worker)?;
            let links = api
                .store
                .list_worker_workdir_links(&api.config.workspace_id, &record.worker)?;
            let summary =
                merge_worker_registry_projection(Some(&worker), &record, links, &workdirs);
            project_workspace_worker(api, summary)
        })
        .collect()
}

fn workers_response(
    api: WorkspaceApi,
) -> ApiResult<workspace_api::ListResponse<workspace_api::WorkerSummary>> {
    let limit = api.config.max_records.min(200);
    let runtime_workers = api.runtime.list_workers(limit);
    let mut observed = std::collections::BTreeMap::new();
    for worker in &runtime_workers.items {
        let _ = sync_worker_observation(&api, worker);
        observed.insert(worker.worker.clone(), worker.clone());
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
        if !observed.contains_key(&record.worker) {
            match api.runtime.worker(&record.worker) {
                Ok(worker) => {
                    let _ = sync_worker_observation(&api, &worker);
                    observed.insert(record.worker.clone(), worker);
                }
                Err(RuntimeRegistryError::UnknownWorker { .. }) => {}
                Err(error) => diagnostics.push(RuntimeDiagnostic {
                    code: "worker_detail_probe_failed".to_string(),
                    severity: DiagnosticSeverity::Info,
                    message: format!(
                        "Could not verify Worker {} on Runtime {}: {}",
                        record.worker.worker_id,
                        record.worker.runtime_id,
                        sanitize_backend_error(&error.into_error().to_string())
                    ),
                }),
            }
        }
        let links = api
            .store
            .list_worker_workdir_links(&api.config.workspace_id, &record.worker)?;
        let summary = merge_worker_registry_projection(
            observed.get(&record.worker),
            &record,
            links,
            &workdir_records,
        );
        items.push(project_workspace_worker(&api, summary)?);
    }
    Ok(workspace_api::ListResponse {
        workspace_id: api.config.workspace_id,
        limit,
        items,
        source: "backend_worker_registry".to_string(),
        diagnostics: diagnostics.into_iter().map(Into::into).collect(),
    })
}

fn load_backend_runtimes_config_for_settings(
    api: &WorkspaceApi,
) -> ApiResult<BackendRuntimesConfigFile> {
    api.config
        .runtime_config_path
        .as_ref()
        .map(BackendRuntimesConfigFile::load_from_path)
        .transpose()
        .map_err(|error| {
            Error::Config(format!(
                "failed to read Backend runtimes config for Runtime connections: {}",
                sanitize_backend_error(&error.to_string())
            ))
            .into()
        })
        .map(|config| config.unwrap_or_default())
}

fn write_backend_runtimes_config_for_settings(
    api: &WorkspaceApi,
    runtime_config: &BackendRuntimesConfigFile,
) -> ApiResult<()> {
    let path = api.config.runtime_config_path.as_ref().ok_or_else(|| {
        Error::Config(
            "Backend runtimes config path is unavailable; set YOI_CONFIG_DIR, YOI_HOME, XDG_CONFIG_HOME, or HOME"
                .to_string(),
        )
    })?;
    runtime_config.write_to_path(path).map_err(|error| {
        Error::Config(format!(
            "failed to write Backend runtimes config for Runtime connections: {}",
            sanitize_backend_error(&error.to_string())
        ))
        .into()
    })
}

fn runtime_connection_settings_response(
    api: &WorkspaceApi,
    runtime_config: &BackendRuntimesConfigFile,
) -> RuntimeConnectionSettingsResponse {
    RuntimeConnectionSettingsResponse {
        workspace_id: api.config.workspace_id.clone(),
        embedded: embedded_runtime_connection_summary(api),
        remotes: remote_runtime_connection_summaries(api, runtime_config, false),
        diagnostics: Vec::new(),
    }
}

fn runtime_connection_mutation_response(
    api: &WorkspaceApi,
    runtime_config: &BackendRuntimesConfigFile,
    diagnostics: Vec<RuntimeDiagnostic>,
) -> RuntimeConnectionMutationResponse {
    RuntimeConnectionMutationResponse {
        workspace_id: api.config.workspace_id.clone(),
        restart_required: false,
        remotes: remote_runtime_connection_summaries(api, runtime_config, false),
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
    runtime_config: &BackendRuntimesConfigFile,
    restart_required: bool,
) -> Vec<RemoteRuntimeConnectionSummary> {
    let live_runtimes = api
        .runtime
        .list_runtimes(api.config.max_records.min(200))
        .items;
    runtime_config
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

fn worker_launch_options_response(api: &WorkspaceApi) -> ApiResult<WorkerLaunchOptionsResponse> {
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
    let config_state = api
        .config_store
        .load_workspace_config(&api.config.workspace_id)?
        .ok_or_else(|| {
            ApiError::from(Error::InvalidRecordId("virtual config source tree".into()))
        })?;
    let profile_settings = crate::profile_settings::project_profiles_from_workspace_config(
        &api.config.workspace_id,
        &config_state,
    )?
    .settings;
    let profiles = profile_settings
        .profiles
        .into_iter()
        .filter(|profile| {
            !profile
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.severity == DiagnosticSeverity::Error)
        })
        .map(|profile| WorkerLaunchProfileCandidate {
            id: profile.profile_id,
            label: profile.label,
            description: profile
                .description
                .unwrap_or_else(|| "Workspace profile.".to_string()),
        })
        .collect();
    Ok(WorkerLaunchOptionsResponse {
        workspace_id: api.config.workspace_id.clone(),
        runtimes,
        default_profile: profile_settings.default_profile,
        profiles,
        repositories: working_directory_repository_options(api),
        working_directories: available_working_directory_summaries(api).unwrap_or_default(),
        diagnostics: Vec::new(),
    })
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
    records
        .iter()
        .map(|record| projected_workdir_summary_from_record(api, record))
        .collect::<Result<Vec<_>>>()
        .map_err(ApiError::from)
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
        if summary.occupied_by.is_none() && summary.primary_worker_id.is_none() {
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
        .map(|record| projected_workdir_summary_from_record(api, record))
        .collect::<Result<Vec<_>>>()
        .map_err(ApiError::from)?;
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
    parse_runtime_worker_id_for_registry(worker.worker.worker_id.as_str())?;
    let worker_ref = worker.worker.clone();
    let existing = api
        .store
        .get_worker_registry(&api.config.workspace_id, &worker_ref)?;
    let display_name = match (display_name_policy, existing.as_ref()) {
        (WorkerRegistryDisplayNamePolicy::PreserveExisting, Some(record)) => {
            record.display_name.clone()
        }
        _ => display_name.to_string(),
    };
    let record = WorkerRegistryRecord {
        workspace_id: api.config.workspace_id.clone(),
        worker: worker_ref.clone(),
        display_name,
        profile,
        retention_state: existing
            .as_ref()
            .map(|record| record.retention_state.clone())
            .unwrap_or_else(|| "normal".to_string()),
        transcript_ref: Some(format!(
            "runtime://{}/workers/{}/transcript",
            worker.worker.runtime_id.as_str(),
            worker.worker.worker_id.as_str()
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
        .get_worker_registry(&api.config.workspace_id, &worker_ref)?
        .unwrap_or(record))
}

fn worker_summary_from_registry(record: &WorkerRegistryRecord) -> WorkerSummary {
    WorkerSummary {
        worker: record.worker.clone(),
        host_id: "backend-registry".to_string(),
        display_name: record.display_name.clone(),
        label: record.display_name.clone(),
        singleton_key: None,
        tags: Vec::new(),
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
            workspace_id: Some(record.workspace_id.clone()),
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
            .map(|workdir| {
                let mut workdir_summary = workdir_summary_from_record(workdir);
                workdir_summary.occupied_by = Some(WorkingDirectoryOccupancy {
                    worker: record.worker.clone(),
                    display_name: record.display_name.clone(),
                    linked_at: link.linked_at.clone(),
                });
                workdir_summary
            })
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
            workdir_record_from_summary(api, worker.worker.runtime_id.as_str(), working_directory);
        api.store.upsert_workdir_registry(&workdir_record)?;
        link_worker_to_workdir(api, &record, &working_directory.working_directory_id, None)?;
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
        creation_selector: request
            .repository
            .selector
            .as_ref()
            .map(|selector| selector.as_ref().to_string()),
        creation_ref: None,
        current_selector: None,
        current_ref: None,
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
    let links = api
        .store
        .list_worker_workdir_links(&api.config.workspace_id, &worker_record.worker)?;
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
        creation_selector: summary.creation_selector.clone(),
        creation_ref: summary.creation_ref.clone(),
        current_selector: summary.current_selector.clone(),
        current_ref: summary.current_ref.clone(),
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
    if record.creation_selector.is_none() {
        record.creation_selector = existing.creation_selector.clone();
    }
    if record.creation_ref.is_none() {
        record.creation_ref = existing.creation_ref.clone();
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
        creation_selector: record.creation_selector.clone(),
        creation_ref: record.creation_ref.clone(),
        current_selector: record.current_selector.clone(),
        current_ref: record.current_ref.clone(),
        materializer_kind: MaterializerKind::LocalGitWorktree,
        cleanup_target: Some(WorkingDirectoryCleanupTarget {
            kind: "local_git_worktree".to_string(),
            working_directory_id: record.workdir_id.clone(),
            repository_id: record.repository_id.clone(),
        }),
        status,
        cleanliness: Some(record.cleanliness.clone()),
        primary_worker_id: None,
        occupied_by: None,
    }
}

fn apply_workdir_occupancy_projection(
    api: &WorkspaceApi,
    summary: &mut WorkingDirectorySummary,
) -> Result<()> {
    let links = api
        .store
        .list_workdir_worker_links(&api.config.workspace_id, &summary.working_directory_id)?;
    let Some(link) = links.first() else {
        summary.primary_worker_id = None;
        summary.occupied_by = None;
        return Ok(());
    };

    let worker = api
        .store
        .get_worker_registry(&api.config.workspace_id, &link.worker)?
        .ok_or_else(|| {
            Error::RegistryInconsistency(format!(
                "Workdir {} attachment references missing Worker {}:{}",
                link.workdir_id, link.worker.runtime_id, link.worker.worker_id
            ))
        })?;
    summary.primary_worker_id = None;
    summary.occupied_by = Some(WorkingDirectoryOccupancy {
        worker: link.worker.clone(),
        display_name: worker.display_name,
        linked_at: link.linked_at.clone(),
    });
    Ok(())
}

fn projected_workdir_summary_from_record(
    api: &WorkspaceApi,
    record: &WorkdirRegistryRecord,
) -> Result<WorkingDirectorySummary> {
    let mut summary = workdir_summary_from_record(record);
    apply_workdir_occupancy_projection(api, &mut summary)?;
    Ok(summary)
}

fn link_worker_to_workdir(
    api: &WorkspaceApi,
    worker_record: &WorkerRegistryRecord,
    workdir_id: &str,
    reservation_id: Option<&str>,
) -> ApiResult<()> {
    let timestamp = now_registry_timestamp();
    let record = WorkerWorkdirLinkRecord {
        workspace_id: api.config.workspace_id.clone(),
        worker: worker_record.worker.clone(),
        workdir_id: workdir_id.to_string(),
        role: "attachment".to_string(),
        linked_at: timestamp,
        unlinked_at: None,
    };
    if let Some(reservation_id) = reservation_id {
        api.store
            .finalize_reserved_worker_workdir_attachment(&record, reservation_id)?;
    } else {
        api.store.attach_worker_workdir(&record)?;
    }
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
    let repository = api.require_configured_workspace_repository(&request.repository_id)?;
    let selector = request
        .selector
        .or_else(|| repository.default_selector.clone())
        .filter(|selector| !selector.trim().is_empty());
    Ok(WorkingDirectoryRequest {
        repository: WorkingDirectoryRepository {
            id: repository.id.clone(),
            provider: "git".to_string(),
            source: repository.source.clone(),
            source_revision: repository.source_revision,
            source_fingerprint: repository.source_fingerprint.clone(),
            selector: selector.map(RuntimeRepositorySelector),
        },
        materializer: MaterializerKind::LocalGitWorktree,
        backend_workdir_id: None,
    })
}

fn parse_runtime_worker_id_for_registry(worker_id: &str) -> ApiResult<WorkerId> {
    worker_id.parse::<WorkerId>().map_err(|_| {
        settings_bad_request(
            "workspace_worker_id_invalid",
            "Workspace Worker id must be a UUIDv7",
        )
    })
}

fn sanitize_worker_display_name(value: &str) -> Option<String> {
    let display_name = value.trim();
    if display_name.chars().any(char::is_control) {
        None
    } else if display_name.is_empty() {
        Some("Worker".to_string())
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
        other => {
            let message = format!("repository evidence validation failed: {other:?}");
            ApiError::with_diagnostics(
                Error::RuntimeOperationFailed {
                    runtime_id: "workspace-repository-registry".to_string(),
                    code: "repository_evidence_invalid".to_string(),
                    message: message.clone(),
                },
                vec![RuntimeDiagnostic {
                    code: "repository_evidence_invalid".to_string(),
                    severity: DiagnosticSeverity::Error,
                    message,
                }],
            )
        }
    })
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

    static_file_or_spa_response(static_root, scoped_workspace_static_path(uri.path())).await
}

fn unscoped_workspace_ui_redirect(
    path: &str,
    query: Option<&str>,
    workspace_id: &str,
) -> Option<String> {
    let scoped_tail = if ["/repositories", "/objectives", "/settings", "/runtimes"]
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

async fn static_file_or_spa_response(static_root: &Path, request_path: &str) -> Response {
    match read_static_or_index(static_root, request_path).await {
        Ok(StaticAsset {
            bytes,
            content_type,
        }) => (StatusCode::OK, [(CONTENT_TYPE, content_type)], bytes).into_response(),
        Err(error) => {
            tracing::debug!(%error, path = request_path, "failed to serve static asset");
            StatusCode::NOT_FOUND.into_response()
        }
    }
}

fn scoped_workspace_static_path(path: &str) -> &str {
    let Some(scoped) = path.strip_prefix("/w/") else {
        return path;
    };
    match scoped.find('/') {
        Some(index) => &scoped[index..],
        None => "/",
    }
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

#[derive(Debug)]
struct ApiError {
    error: Error,
    diagnostics: Vec<RuntimeDiagnostic>,
}

#[derive(Debug, Clone)]
struct ApiErrorLog {
    kind: String,
    message: String,
    diagnostics: Vec<RuntimeDiagnostic>,
}

impl From<merge_request::MergeRequestError> for ApiError {
    fn from(error: merge_request::MergeRequestError) -> Self {
        Error::MergeRequest(error).into()
    }
}

impl From<Error> for ApiError {
    fn from(error: Error) -> Self {
        let diagnostics = match &error {
            Error::RuntimeOperationFailed { code, message, .. } => vec![RuntimeDiagnostic {
                code: code.clone(),
                severity: DiagnosticSeverity::Error,
                message: sanitize_backend_error(message),
            }],
            Error::Ticket(ticket_error) => vec![RuntimeDiagnostic {
                code: match ticket_error {
                    ticket::TicketError::NotFound(_) => "ticket_not_found",
                    ticket::TicketError::Ambiguous { .. } => "ticket_ambiguous",
                    ticket::TicketError::Locked { .. } => "ticket_locked",
                    ticket::TicketError::Conflict(_)
                    | ticket::TicketError::StaleWorkflowState { .. }
                    | ticket::TicketError::InvalidWorkflowTransition { .. }
                    | ticket::TicketError::BlockingRelations(_)
                    | ticket::TicketError::OperationFingerprintMismatch { .. } => "ticket_conflict",
                    ticket::TicketError::MissingTargetRepository => {
                        "ticket_target_repository_missing"
                    }
                    ticket::TicketError::UnknownTargetRepository(_) => {
                        "ticket_target_repository_unknown"
                    }
                    ticket::TicketError::MissingTargetSelector(_) => {
                        "ticket_target_selector_missing"
                    }
                    ticket::TicketError::InvalidTargetSelector { .. } => {
                        "ticket_target_selector_invalid"
                    }
                    ticket::TicketError::TargetAuthorityUnavailable => {
                        "ticket_target_authority_unavailable"
                    }
                    ticket::TicketError::InvalidPathComponent(_)
                    | ticket::TicketError::PathEscapesRoot { .. } => "invalid_ticket_request",
                    ticket::TicketError::Io { .. }
                    | ticket::TicketError::Parse { .. }
                    | ticket::TicketError::Sqlite(_) => "ticket_backend_error",
                }
                .to_string(),
                severity: DiagnosticSeverity::Error,
                message: sanitize_backend_error(&ticket_error.to_string()),
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
            Error::BrowserReopenConfirmationRequired => StatusCode::FORBIDDEN,
            Error::TicketAssignmentConflict(_)
            | Error::WorkdirAttachmentConflict(_)
            | Error::WorkspaceConfigConflict(_) => StatusCode::CONFLICT,
            Error::WorkerSourceIdentity(_) | Error::InvalidInput(_) => StatusCode::BAD_REQUEST,
            Error::InvalidRuntimeIdentifier { .. } | Error::ReservedWorkerName(_) => {
                StatusCode::BAD_REQUEST
            }
            Error::Ticket(ticket::TicketError::NotFound(_))
            | Error::MergeRequest(merge_request::MergeRequestError::NotFound) => {
                StatusCode::NOT_FOUND
            }
            Error::MergeRequest(merge_request::MergeRequestError::Validation(_)) => {
                StatusCode::BAD_REQUEST
            }
            Error::MergeRequest(_) => StatusCode::CONFLICT,
            Error::Ticket(
                ticket::TicketError::Ambiguous { .. }
                | ticket::TicketError::Locked { .. }
                | ticket::TicketError::Conflict(_),
            ) => StatusCode::CONFLICT,
            Error::Ticket(
                ticket::TicketError::InvalidPathComponent(_)
                | ticket::TicketError::PathEscapesRoot { .. },
            ) => StatusCode::BAD_REQUEST,
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
                if code == "unknown_profile_source"
                    || code == "unknown_profile_selector"
                    || code == "unknown_objective" =>
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
                    || code == "default_runtime_not_configured"
                    || code == "workspace_worker_workdir_required"
                    || code.ends_with("_already_exists")
                    || code.ends_with("_not_config_managed")
                    || code.ends_with("_unsupported") =>
            {
                StatusCode::BAD_REQUEST
            }
            Error::RuntimeOperationFailed { code, .. }
                if code == "runtime_capacity_unavailable"
                    || code == "runtime_workdir_capacity_unavailable" =>
            {
                StatusCode::SERVICE_UNAVAILABLE
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
        let log = ApiErrorLog {
            kind: self
                .diagnostics
                .first()
                .map(|diagnostic| diagnostic.code.clone())
                .unwrap_or_else(|| {
                    status
                        .canonical_reason()
                        .unwrap_or("api_error")
                        .to_ascii_lowercase()
                        .replace(' ', "_")
                }),
            message: response_message.clone(),
            diagnostics: self.diagnostics.clone(),
        };
        let mut response = (
            status,
            [(CONTENT_TYPE, "application/json")],
            Json(serde_json::json!({
                "error": status.canonical_reason().unwrap_or("error"),
                "message": response_message,
                "diagnostics": self.diagnostics,
            }))
            .to_string(),
        )
            .into_response();
        response.extensions_mut().insert(log);
        response
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::WorkspaceBackendRuntimesConfig;
    use axum::body::{Body, to_bytes};
    use axum::http::Request;
    use futures::{SinkExt, StreamExt};
    use serde_json::{Value, json};
    use std::{fs, sync::Arc};
    use tokio_tungstenite::connect_async;
    use tokio_tungstenite::tungstenite::Message;
    use tower::ServiceExt;
    use worker_runtime::auth::{
        RuntimeIdentityMaterial, RuntimeWorkerMutationSourceSigner, WORKER_REMOVE_PERMISSION,
        decode_worker_mutation_source_claims,
    };
    use worker_runtime::resource::BackendResourceClient;
    use worker_runtime::worker_source::{
        RuntimeOwnedWorkerMutationProof, RuntimeWorkerMutationSourceAuthority,
    };
    use worker_runtime::working_directory::WorkingDirectoryMaterializer;

    use crate::hosts::{
        RemoteRuntimeAuthConfig, RuntimeCapabilitySummary, TicketWorkerRole, WorkerInputKind,
        WorkerOperationState, WorkerSpawnAcceptanceRequirement, WorkerSpawnIntent,
    };
    use crate::store::{
        AccountRecord, ApiTokenRecord, BrowserSessionRecord, MemoryDocumentRecord,
        MemoryStagingRecord, ObjectiveRecord, ObjectiveResourceRecord, ObjectiveTicketLinkRecord,
        SqliteWorkspaceStore, TrustedRuntimeRecord, UserRecord, WorkspaceRecord,
    };

    fn seed_test_api_token(store: &dyn ControlPlaneStore, suffix: &str) -> String {
        let account_id = format!("account-{suffix}");
        let user_id = format!("user-{suffix}");
        let token = format!("api-token-{suffix}");
        store
            .upsert_account(&AccountRecord {
                account_id: account_id.clone(),
                kind: "user".to_owned(),
                handle: format!("user-{suffix}"),
                display_name: "Test User".to_owned(),
                created_at: "2026-01-01T00:00:00Z".to_owned(),
                updated_at: "2026-01-01T00:00:00Z".to_owned(),
            })
            .unwrap();
        store
            .upsert_user(&UserRecord {
                user_id: user_id.clone(),
                account_id,
                handle: format!("user-{suffix}"),
                display_name: "Test User".to_owned(),
                created_at: "2026-01-01T00:00:00Z".to_owned(),
                updated_at: "2026-01-01T00:00:00Z".to_owned(),
            })
            .unwrap();
        store
            .create_api_token(&ApiTokenRecord {
                token_hash: crate::auth::token_hash(&token),
                token_id: format!("token-{suffix}"),
                user_id,
                label: "test".to_owned(),
                created_at: "2026-01-01T00:00:00Z".to_owned(),
                expires_at: None,
                last_used_at: None,
                revoked_at: None,
            })
            .unwrap();
        token
    }

    fn configure_runtime_request_auth(
        api: &mut WorkspaceApi,
        identity: &worker_runtime::auth::RuntimeIdentityMaterial,
        runtime_id: &str,
    ) {
        api.config.remote_runtime_sources.push(RemoteRuntimeConfig {
            runtime_id: runtime_id.to_owned(),
            workspace_id: Some(api.workspace_id().to_owned()),
            display_name: runtime_id.to_owned(),
            base_url: "https://runtime.test".to_owned(),
            bearer_token: None,
            auth: Some(RemoteRuntimeAuthConfig {
                server_id: "server-test".to_owned(),
                server_private_key: "unused".to_owned(),
            }),
            cached_capabilities: RuntimeCapabilitySummary {
                can_list_hosts: true,
                can_list_workers: true,
                can_get_worker: true,
                can_spawn_worker: true,
                can_stop_worker: true,
                has_workspace_fs: false,
                has_shell: false,
                has_git: false,
                supports_worktrees: false,
                supports_backend_internal_tools: false,
                workspace_scope: api.workspace_id().to_owned(),
                max_workers: 1,
                os: "test".to_owned(),
                arch: "test".to_owned(),
            },
            cached_status: "connected".to_owned(),
            timeout: std::time::Duration::from_secs(1),
        });
        SqliteWorkspaceStore::open(&api.config.database_path)
            .unwrap()
            .upsert_trusted_runtime(&TrustedRuntimeRecord {
                runtime_id: runtime_id.to_owned(),
                workspace_id: Some(api.workspace_id().to_owned()),
                display_name: runtime_id.to_owned(),
                base_url: "https://runtime.test".to_owned(),
                public_key: identity.public_key.clone(),
                created_at: "2026-01-01T00:00:00Z".to_owned(),
                updated_at: "2026-01-01T00:00:00Z".to_owned(),
                revoked_at: None,
            })
            .unwrap();
    }

    fn runtime_resource_fetch_request(
        api: &WorkspaceApi,
        identity: &worker_runtime::auth::RuntimeIdentityMaterial,
        body: Vec<u8>,
    ) -> Request<Body> {
        let path = format!(
            "/api/runtime/v1/workspaces/{}/resources/fetch",
            api.workspace_id()
        );
        let proof = worker_runtime::auth::RuntimeRequestSourceSigner::from_identity(identity)
            .issue(
                "server-test",
                api.workspace_id(),
                None,
                worker_runtime::auth::BACKEND_RESOURCE_FETCH_PERMISSION,
                "POST",
                &path,
                &body,
                i64::try_from(worker_runtime::auth::unix_now_seconds()).unwrap_or(i64::MAX),
                30,
            )
            .unwrap();
        Request::builder()
            .method("POST")
            .uri(path)
            .header(CONTENT_TYPE, "application/json")
            .header(
                worker_runtime::auth::RUNTIME_REQUEST_SOURCE_PROOF_HEADER,
                proof,
            )
            .body(Body::from(body))
            .unwrap()
    }

    fn test_create_binding() -> WorkerCreateBinding {
        WorkerCreateBinding {
            worker_id: WorkerId::now_v7(),
            create_fingerprint: "sha256:test-create".to_string(),
        }
    }

    #[test]
    fn reopen_confirmation_rejects_api_token_actor_before_session_resolution() {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", "Bearer api-token".parse().unwrap());
        assert!(matches!(
            reject_non_browser_reopen_auth(&headers),
            Err(Error::BrowserReopenConfirmationRequired)
        ));
        assert!(reject_non_browser_reopen_auth(&HeaderMap::new()).is_ok());
    }

    #[test]
    fn generic_worker_state_change_cannot_enter_inprogress() {
        for from in ["planning", "ready", "queued"] {
            let change = || {
                TicketStateChange::new(
                    from,
                    "inprogress",
                    "worker-tool",
                    "bypass assignment-aware start",
                )
            };
            let operations = [
                TicketBackendOperation::SetWorkflowState {
                    id: TicketIdOrSlug::Query("T1".to_string()),
                    change: change(),
                },
                TicketBackendOperation::SetStateField {
                    id: TicketIdOrSlug::Query("T1".to_string()),
                    field: "state".to_string(),
                    change: change(),
                },
                TicketBackendOperation::AddStateChanged {
                    id: TicketIdOrSlug::Query("T1".to_string()),
                    change: change(),
                },
            ];
            for operation in operations {
                let error = reject_unguarded_ticket_completion(&operation).unwrap_err();
                assert!(
                    error
                        .to_string()
                        .contains("atomic ready-state Coder assignment")
                );
            }
        }
    }

    #[test]
    fn flow_or_generic_worker_state_change_is_not_ticket_completion_authority() {
        let change = || {
            TicketStateChange::new(
                "inprogress",
                "done",
                "flow reached terminal state",
                "terminal flow state",
            )
        };
        for operation in [
            TicketBackendOperation::SetWorkflowState {
                id: TicketIdOrSlug::Query("T1".to_string()),
                change: change(),
            },
            TicketBackendOperation::SetStateField {
                id: TicketIdOrSlug::Query("T1".to_string()),
                field: "state".to_string(),
                change: change(),
            },
            TicketBackendOperation::AddStateChanged {
                id: TicketIdOrSlug::Query("T1".to_string()),
                change: change(),
            },
        ] {
            let error = reject_unguarded_ticket_completion(&operation).unwrap_err();
            assert!(error.to_string().contains("MergeRequestComplete"));
        }
    }

    #[test]
    fn failed_api_log_is_structured_and_omits_query_values() {
        let uri = "/api/w/workspace/tickets?access_token=secret"
            .parse::<Uri>()
            .expect("valid URI");
        let error = ApiErrorLog {
            kind: "ticket_backend_error".to_string(),
            message: "sqlite error: FOREIGN KEY constraint failed".to_string(),
            diagnostics: vec![RuntimeDiagnostic {
                code: "ticket_backend_error".to_string(),
                severity: DiagnosticSeverity::Error,
                message: "FOREIGN KEY constraint failed".to_string(),
            }],
        };

        let line = failed_api_log_json(
            &Method::POST,
            &uri,
            StatusCode::INTERNAL_SERVER_ERROR,
            Some(&error),
        );
        let event: Value = serde_json::from_str(&line).expect("structured JSON log");

        assert_eq!(event["event"], "api_error");
        assert_eq!(event["method"], "POST");
        assert_eq!(event["path"], "/api/w/workspace/tickets");
        assert_eq!(event["status"], 500);
        assert_eq!(event["kind"], "ticket_backend_error");
        assert_eq!(
            event["message"],
            "sqlite error: FOREIGN KEY constraint failed"
        );
        assert_eq!(
            event["diagnostics"][0]["message"],
            "FOREIGN KEY constraint failed"
        );
        assert!(!line.contains("access_token"));
        assert!(!line.contains("secret"));
    }

    const TEST_WORKSPACE_ID: &str = "0192f0e8-4d84-7d6e-a000-000000000001";
    const TEST_REPOSITORY_ID: &str = "main";
    const TEST_CREATED_AT: &str = "2026-06-23T06:43:28Z";

    fn test_worker_workspace_api(_runtime_id: &str) -> WorkspaceApiRef {
        WorkspaceApiRef {
            workspace_id: TEST_WORKSPACE_ID.to_string(),
            base_url: "http://127.0.0.1:8787".to_string(),
        }
    }

    fn test_worker_memory_settings() -> manifest::WorkspaceMemorySettingsSnapshot {
        manifest::WorkspaceMemorySettingsSnapshot {
            workspace_id: TEST_WORKSPACE_ID.to_string(),
            settings_revision: 1,
            language: "English".to_string(),
        }
    }

    #[test]
    fn ticket_api_errors_preserve_http_status() {
        let not_found = ApiError::from(Error::Ticket(ticket::TicketError::NotFound(
            "0000000000000".to_string(),
        )))
        .into_response();
        assert_eq!(not_found.status(), StatusCode::NOT_FOUND);

        let conflict = ApiError::from(Error::Ticket(ticket::TicketError::Conflict(
            "invalid transition".to_string(),
        )))
        .into_response();
        assert_eq!(conflict.status(), StatusCode::CONFLICT);
    }

    #[test]
    fn backend_worker_projection_preserves_missing_rows_links_and_redacts_paths() {
        let worker = WorkerRegistryRecord {
            workspace_id: "workspace-1".to_string(),
            worker: RuntimeWorkerRef::new("embedded", "1"),
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
            creation_selector: Some("develop".to_string()),
            creation_ref: Some("abcdef".to_string()),
            current_selector: None,
            current_ref: Some("fedcba".to_string()),
            materialization_status: "missing".to_string(),
            cleanliness: "clean".to_string(),
            created_at: "1".to_string(),
            updated_at: "3".to_string(),
        };
        let link = WorkerWorkdirLinkRecord {
            workspace_id: "workspace-1".to_string(),
            worker: worker.worker.clone(),
            workdir_id: workdir.workdir_id.clone(),
            role: "attachment".to_string(),
            linked_at: "4".to_string(),
            unlinked_at: None,
        };

        let projected = merge_worker_registry_projection(None, &worker, vec![link], &[workdir]);

        assert_eq!(projected.state, "missing");
        let working_directory = projected.working_directory.as_ref().unwrap();
        assert_eq!(
            working_directory.status,
            WorkingDirectoryStatusKind::NotFound
        );
        assert_eq!(
            working_directory.creation_selector.as_deref(),
            Some("develop")
        );
        assert_eq!(working_directory.creation_ref.as_deref(), Some("abcdef"));
        assert_eq!(working_directory.current_selector, None);
        assert_eq!(working_directory.current_ref.as_deref(), Some("fedcba"));
        let occupied_by = working_directory.occupied_by.as_ref().unwrap();
        assert_eq!(occupied_by.worker, RuntimeWorkerRef::new("embedded", "1"));
        assert!(working_directory.primary_worker_id.is_none());
        let occupancy = serde_json::to_value(occupied_by).unwrap();
        assert_eq!(occupancy["runtime_id"], "embedded");
        assert_eq!(occupancy["worker_id"], "1");
        assert!(occupancy.get("runtime_worker_id").is_none());
        assert_eq!(occupied_by.display_name, "Missing Worker");
        assert_eq!(occupied_by.linked_at, "4");
        let serialized = serde_json::to_string(&projected).unwrap();
        assert!(!serialized.contains("/tmp/"));
        assert!(!serialized.contains("materialized_path"));
    }

    #[tokio::test]
    async fn flow_source_resolution_returns_immutable_workspace_and_builtin_snapshots() {
        let workspace = tempfile::tempdir().unwrap();
        init_clean_git_workspace(workspace.path());
        let api = test_api(workspace.path()).await;
        let source = r#"{
            schema_version = 1;
            name = "browser-flow";
            initial = "work";
            states = {
                work = {
                    instructions = "Implement and validate the requested change.";
                    transitions = {
                        done = { target = "done"; condition = "The work is complete."; };
                    };
                };
                done = { instructions = ""; terminal = true; };
            };
        }"#;
        let stored = api
            .store
            .put_flow_source_for_kind(
                &api.config.workspace_id,
                FlowSourceKind::Workspace,
                "flows/browser-flow.dcdl",
                source,
                "2026-08-06T00:00:00Z",
            )
            .unwrap();

        let Json(resolved) = scoped_resolve_flow_source(
            State(api.clone()),
            AxumPath(ScopedWorkspacePath {
                workspace_id: api.config.workspace_id.clone(),
            }),
            Json(FlowSourceResolveRequest {
                selector: "workspace:browser-flow".parse().unwrap(),
            }),
        )
        .await
        .unwrap();
        assert_eq!(resolved.flow_id, stored.flow_id);
        assert_eq!(resolved.revision, stored.revision);
        assert_eq!(resolved.content_digest, stored.content_digest);
        assert_eq!(resolved.definition.name, "browser-flow");

        let Json(builtin) = scoped_resolve_flow_source(
            State(api.clone()),
            AxumPath(ScopedWorkspacePath {
                workspace_id: api.config.workspace_id.clone(),
            }),
            Json(FlowSourceResolveRequest {
                selector: "builtin:coder-review".parse().unwrap(),
            }),
        )
        .await
        .unwrap();
        assert_eq!(builtin.definition.name, "coder-review");
        assert_eq!(builtin.selector.to_string(), "builtin:coder-review");
        assert_eq!(builtin.flow_id, "builtin:coder-review");
        assert_eq!(builtin.revision, 3);
        assert_eq!(
            api.store
                .list_flow_sources(&api.config.workspace_id)
                .unwrap(),
            vec![stored],
            "built-in resolution must not mutate Workspace source authority",
        );
    }

    #[test]
    fn worker_initial_submit_validate_flow_shape_before_spawn() {
        let valid = vec![
            Segment::Flow {
                selector: "builtin:coder-review".to_string(),
            },
            Segment::text("Implement Ticket 00001"),
        ];
        assert!(validate_worker_initial_submit(&valid).is_ok());
        assert!(validate_worker_initial_submit(&[]).is_ok());

        let duplicate = vec![
            Segment::Flow {
                selector: "builtin:coder-review".to_string(),
            },
            Segment::Flow {
                selector: "workspace:coder-review".to_string(),
            },
        ];
        assert!(matches!(
            validate_worker_initial_submit(&duplicate),
            Err(Error::InvalidInput(message)) if message.contains("at most one")
        ));
        assert!(matches!(
            validate_worker_initial_submit(&[Segment::Flow {
                selector: "coder-review".to_string(),
            }]),
            Err(Error::InvalidInput(_))
        ));
        assert!(matches!(
            validate_worker_initial_submit(&[Segment::Unknown]),
            Err(Error::InvalidInput(message)) if message.contains("unknown")
        ));
        assert!(
            serde_json::from_value::<CreateWorkspaceWorkerRequest>(serde_json::json!({
                "runtime_id": "runtime-1",
                "display_name": "coder",
                "initial_text": "legacy parallel authority",
                "working_directory": { "kind": "without_workspace" }
            }))
            .is_err()
        );
    }

    #[tokio::test]
    async fn repository_bound_ticket_flow_and_workdir_launches_fail_closed_across_workspaces() {
        let dir = tempfile::tempdir().unwrap();
        let api = test_api(dir.path()).await;
        let other_workspace = WorkspaceRecord {
            workspace_id: "other-workspace".to_string(),
            owner_account_id: None,
            display_name: "Other Workspace".to_string(),
            state: "active".to_string(),
            created_at: "1".to_string(),
            updated_at: "1".to_string(),
        };
        api.store.upsert_workspace(&other_workspace).await.unwrap();
        api.store
            .upsert_repository(&RepositoryRecord {
                workspace_id: other_workspace.workspace_id.clone(),
                repository_id: "foreign".to_string(),
                name: "Foreign".to_string(),
                kind: "git".to_string(),
                provider: Some("git".to_string()),
                source: workspace_api::RepositorySource {
                    kind: workspace_api::RepositorySourceKind::LocalPath,
                    uri: dir.path().join("foreign").display().to_string(),
                },
                default_ref: Some("HEAD".to_string()),
                source_revision: 1,
                source_fingerprint: "sha256:test".to_string(),
                observed_status: workspace_api::RepositoryObservedStatus::Unverified,
                observed_at: None,
                created_at: "1".to_string(),
                updated_at: "1".to_string(),
            })
            .unwrap();

        let mut create_input = ticket::NewTicket::new("Foreign repository target");
        create_input.repository_id = Some("foreign".to_string());
        assert!(
            validate_ticket_repository_operation(
                &api,
                &TicketBackendOperation::Create {
                    input: create_input.clone(),
                },
            )
            .is_err()
        );

        assert!(
            browser_ticket_backend(&api)
                .unwrap()
                .create(create_input)
                .is_err()
        );

        let mut foreign_repository = api.config.repositories[0].clone();
        foreign_repository.id = "foreign".to_string();
        let workdir_flow_launch = WorkerSpawnRequest {
            requested_worker_name: Some("cross-workspace-workdir".to_string()),
            intent: WorkerSpawnIntent::WorkspaceCoding,
            acceptance: WorkerSpawnAcceptanceRequirement::RunAccepted {
                expected_segments: 1,
            },
            profile: ProfileSelector::Builtin("builtin:coder".to_string()),
            ticket_assignment: None,
            initial_submit: vec![Segment::Flow {
                selector: "builtin:coder-review".to_string(),
            }],
            working_directory_request: None,
            resolved_working_directory_request: Some(working_directory_request_from_repository(
                &foreign_repository,
                Some("HEAD"),
            )),
            resolved_working_directory: None,
            resolved_config_bundle: None,
            resolved_worker_observation_enabled: false,
            resolved_worker_observation_grants: Vec::new(),
            resolved_control_operation: None,
            resolved_workspace_api: None,
            resolved_memory_settings: None,
        };
        assert!(
            api.validate_worker_spawn_repository_scope(&workdir_flow_launch)
                .is_err()
        );
    }

    #[test]
    fn recorded_completion_replay_is_identified_before_later_target_observation() {
        let event = merge_request::MergeEvent {
            event_id: "merge-event".into(),
            sequence: 1,
            operation_id: "operation".into(),
            approval_event_id: "approval".into(),
            approved_source_ref: "source".into(),
            target_ref_before: "before".into(),
            target_ref_after: "after".into(),
            strategy: merge_request::MergeStrategy::FastForward,
            resolution: merge_request::ConflictResolution::None,
            merged_by: merge_request::WorkerIdentity {
                runtime_id: "runtime".into(),
                worker_id: "orchestrator".into(),
            },
            created_at: Utc::now(),
        };
        let thread = vec![merge_request::MergeRequestThreadEvent::Merge(event.clone())];

        assert_eq!(
            recorded_merge_completion(&thread, "operation"),
            Some(&event)
        );
        assert!(recorded_merge_completion(&thread, "different").is_none());
        assert!(require_completed_target_observation("later", "before", "after").is_err());
    }

    #[test]
    fn merge_request_completion_records_only_an_observed_remote_target_update() {
        require_completed_target_observation("after", "before", "after").unwrap();

        let not_pushed =
            require_completed_target_observation("before", "before", "after").unwrap_err();
        assert!(matches!(
            not_pushed.error,
            Error::InvalidInput(ref message)
                if message.contains("push the verified result from the Orchestrator Workdir")
        ));

        let moved = require_completed_target_observation("other", "before", "after").unwrap_err();
        assert!(matches!(
            moved.error,
            Error::InvalidInput(ref message)
                if message.contains("moved outside completion evidence")
        ));
    }

    #[test]
    fn worker_ticket_assignment_projects_coder_intent_and_run_acceptance() {
        let initial_submit = vec![
            Segment::Flow {
                selector: "builtin:coder-review".to_string(),
            },
            Segment::text("Implement Ticket 00001KZ9E0DBS"),
        ];
        let assignment_request = || CreateWorkspaceWorkerTicketAssignmentRequest {
            ticket_id: "00001KZ9E0DBS".to_string(),
            operation_id: "worker-spawn:00001KZ9E0DBS:call-1".to_string(),
        };
        let (intent, acceptance, assignment) =
            browser_worker_spawn_policy(Some(assignment_request()), &initial_submit).unwrap();

        assert_eq!(
            intent,
            WorkerSpawnIntent::TicketRole {
                ticket_id: "00001KZ9E0DBS".to_string(),
                role: TicketWorkerRole::Coder,
            }
        );
        assert_eq!(
            acceptance,
            WorkerSpawnAcceptanceRequirement::RunAccepted {
                expected_segments: 2,
            }
        );
        assert_eq!(
            assignment,
            Some(WorkerTicketAssignmentRequest {
                ticket_id: "00001KZ9E0DBS".to_string(),
                operation_id: "worker-spawn:00001KZ9E0DBS:call-1".to_string(),
            })
        );
        assert!(
            browser_worker_spawn_policy(
                Some(CreateWorkspaceWorkerTicketAssignmentRequest {
                    ticket_id: " ".to_string(),
                    operation_id: "operation".to_string(),
                }),
                &initial_submit,
            )
            .is_err()
        );
        assert!(
            browser_worker_spawn_policy(
                Some(assignment_request()),
                &[Segment::text("Ticket text without Flow")],
            )
            .is_err()
        );
        assert!(
            browser_worker_spawn_policy(
                None,
                &[Segment::Flow {
                    selector: "builtin:coder-review".to_string(),
                }],
            )
            .is_err()
        );
        assert!(
            reject_orchestrator_generic_flow_spawn_for_source(
                true,
                &[Segment::Flow {
                    selector: "builtin:coder-review".to_string(),
                }],
                false,
            )
            .is_err()
        );
        assert!(
            reject_orchestrator_generic_flow_spawn_for_source(
                false,
                &[Segment::Flow {
                    selector: "builtin:coder-review".to_string(),
                }],
                false,
            )
            .is_ok()
        );
    }

    #[tokio::test]
    async fn ticket_assignment_spawn_requires_queued_or_inprogress_before_runtime_side_effects() {
        let workspace = tempfile::tempdir().unwrap();
        init_clean_git_workspace(workspace.path());
        let api = test_api(workspace.path()).await;
        let backend = browser_ticket_backend(&api).unwrap();
        let ticket = backend
            .create(ticket::NewTicket::new("Planning Ticket"))
            .unwrap();
        let request = WorkerSpawnRequest {
            requested_worker_name: Some("Rejected Coder".to_string()),
            intent: WorkerSpawnIntent::TicketRole {
                ticket_id: ticket.id.clone(),
                role: TicketWorkerRole::Coder,
            },
            acceptance: WorkerSpawnAcceptanceRequirement::RunAccepted {
                expected_segments: 1,
            },
            profile: ProfileSelector::Builtin("builtin:coder".to_string()),
            ticket_assignment: Some(WorkerTicketAssignmentRequest {
                ticket_id: ticket.id,
                operation_id: "planning-spawn".to_string(),
            }),
            initial_submit: vec![Segment::Flow {
                selector: "builtin:coder-review".to_string(),
            }],
            working_directory_request: None,
            resolved_working_directory_request: None,
            resolved_working_directory: None,
            resolved_config_bundle: None,
            resolved_worker_observation_enabled: false,
            resolved_worker_observation_grants: Vec::new(),
            resolved_control_operation: None,
            resolved_workspace_api: None,
            resolved_memory_settings: None,
        };

        assert!(
            validate_ticket_assignment_spawn(&api, EMBEDDED_WORKER_RUNTIME_ID, &request).is_err()
        );
        assert!(
            api.store
                .get_ticket_assignment_operation(&api.config.workspace_id, "planning-spawn")
                .unwrap()
                .is_none()
        );
        assert!(
            api.runtime
                .list_workers_for_runtime(EMBEDDED_WORKER_RUNTIME_ID, 20)
                .unwrap()
                .items
                .is_empty()
        );
    }

    #[tokio::test]
    async fn workspace_worker_endpoint_finalizes_ticket_assignment() {
        let workspace = tempfile::tempdir().unwrap();
        init_clean_git_workspace(workspace.path());
        let api = test_api(workspace.path()).await;
        let backend = browser_ticket_backend(&api).unwrap();
        let mut input = ticket::NewTicket::new("Assigned Ticket");
        input.workflow_state = Some(TicketWorkflowState::Queued);
        let ticket = backend.create(input).unwrap();
        assign_test_orchestrator(&api, &ticket.id);
        let response = create_workspace_worker(
            State(api.clone()),
            HeaderMap::new(),
            Json(CreateWorkspaceWorkerRequest {
                runtime_id: EMBEDDED_WORKER_RUNTIME_ID.to_string(),
                display_name: "Assigned Coder".to_string(),
                profile: Some("builtin:coder".to_string()),
                ticket_assignment: Some(CreateWorkspaceWorkerTicketAssignmentRequest {
                    ticket_id: ticket.id.clone(),
                    operation_id: "workspace-endpoint-assignment".to_string(),
                }),
                initial_submit: vec![Segment::Flow {
                    selector: "builtin:coder-review".to_string(),
                }],
                working_directory: None,
                control_operation_id: None,
                resolved_control_operation: None,
            }),
        )
        .await
        .unwrap()
        .0;

        assert_eq!(
            api.authority.ticket(&ticket.id).unwrap().state,
            TicketWorkflowState::InProgress.as_str()
        );
        let current = api
            .store
            .get_current_ticket_coder_assignment(&api.config.workspace_id, &ticket.id)
            .unwrap()
            .unwrap();
        assert_eq!(current.worker, response.worker_ref);
        let operation = api
            .store
            .get_ticket_assignment_operation(
                &api.config.workspace_id,
                "workspace-endpoint-assignment",
            )
            .unwrap()
            .unwrap();
        assert_eq!(operation.assignment_id, Some(current.assignment_id));
        assert_eq!(operation.worker, Some(response.worker_ref));
    }

    #[tokio::test]
    async fn failed_ticket_assignment_spawn_leaves_ticket_queued() {
        let workspace = tempfile::tempdir().unwrap();
        init_clean_git_workspace(workspace.path());
        let api = test_api(workspace.path()).await;
        let backend = browser_ticket_backend(&api).unwrap();
        let mut input = ticket::NewTicket::new("Queued Ticket");
        input.workflow_state = Some(TicketWorkflowState::Queued);
        let ticket = backend.create(input).unwrap();
        assign_test_orchestrator(&api, &ticket.id);

        let result = create_workspace_worker(
            State(api.clone()),
            HeaderMap::new(),
            Json(CreateWorkspaceWorkerRequest {
                runtime_id: "missing-runtime".to_string(),
                display_name: "Rejected Coder".to_string(),
                profile: Some("builtin:coder".to_string()),
                ticket_assignment: Some(CreateWorkspaceWorkerTicketAssignmentRequest {
                    ticket_id: ticket.id.clone(),
                    operation_id: "failed-queued-assignment".to_string(),
                }),
                initial_submit: vec![Segment::Flow {
                    selector: "builtin:coder-review".to_string(),
                }],
                working_directory: None,
                control_operation_id: None,
                resolved_control_operation: None,
            }),
        )
        .await;

        assert!(result.is_err());
        assert_eq!(
            api.authority.ticket(&ticket.id).unwrap().state,
            TicketWorkflowState::Queued.as_str()
        );
        assert!(
            api.store
                .get_current_ticket_coder_assignment(&api.config.workspace_id, &ticket.id)
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn worker_source_auth_rejects_cross_workspace_mutation() {
        let workspace = tempfile::tempdir().unwrap();
        init_clean_git_workspace(workspace.path());
        let api = test_api(workspace.path()).await;
        let Json(created) = create_workspace_worker(
            State(api.clone()),
            HeaderMap::new(),
            Json(CreateWorkspaceWorkerRequest {
                runtime_id: EMBEDDED_WORKER_RUNTIME_ID.to_string(),
                display_name: "Scoped Worker".to_string(),
                profile: Some("builtin:coder".to_string()),
                ticket_assignment: None,
                initial_submit: Vec::new(),
                working_directory: None,
                control_operation_id: None,
                resolved_control_operation: None,
            }),
        )
        .await
        .unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-yoi-runtime-id",
            axum::http::HeaderValue::from_str(&created.worker_ref.runtime_id).unwrap(),
        );
        headers.insert(
            "x-yoi-worker-id",
            axum::http::HeaderValue::from_str(&created.worker_ref.worker_id).unwrap(),
        );
        let error =
            authenticate_worker_mutation_source(&api, "other-workspace", &headers).unwrap_err();
        assert!(matches!(error, Error::WorkerSourceIdentity(_)));
    }

    #[tokio::test]
    async fn merge_request_completion_authority_requires_current_online_orchestrator() {
        let workspace = tempfile::tempdir().unwrap();
        init_clean_git_workspace(workspace.path());
        let api = test_api(workspace.path()).await;
        let workspace_id = api.config.workspace_id.clone();
        let Json(generic) = create_workspace_worker(
            State(api.clone()),
            HeaderMap::new(),
            Json(CreateWorkspaceWorkerRequest {
                runtime_id: EMBEDDED_WORKER_RUNTIME_ID.to_string(),
                display_name: "Generic Worker".to_string(),
                profile: Some("builtin:coder".to_string()),
                ticket_assignment: None,
                initial_submit: Vec::new(),
                working_directory: None,
                control_operation_id: None,
                resolved_control_operation: None,
            }),
        )
        .await
        .unwrap();

        assert!(matches!(
            require_online_workspace_orchestrator_source(&api, &generic.worker_ref),
            Err(Error::TicketAssignmentConflict(_))
        ));

        let Json(started) = scoped_start_workspace_orchestrator(
            State(api.clone()),
            AxumPath(ScopedWorkspacePath {
                workspace_id: workspace_id.clone(),
            }),
        )
        .await
        .unwrap();
        let orchestrator = started.worker.unwrap().worker;
        require_online_workspace_orchestrator_source(&api, &orchestrator).unwrap();
        assert!(matches!(
            require_online_workspace_orchestrator_source(&api, &generic.worker_ref),
            Err(Error::TicketAssignmentConflict(_))
        ));

        api.runtime
            .stop_worker(
                &orchestrator,
                WorkerLifecycleRequest {
                    reason: Some("completion authority regression test".into()),
                    ticket_assignment: None,
                },
            )
            .unwrap();
        assert!(find_workspace_orchestrator(&api).is_some());
        assert!(find_online_workspace_orchestrator(&api).is_none());
        assert!(matches!(
            require_online_workspace_orchestrator_source(&api, &orchestrator),
            Err(Error::TicketAssignmentConflict(_))
        ));
    }

    #[tokio::test]
    async fn production_profile_backend_rejects_unrecoverable_pending_orchestrator_restore() {
        let workspace = tempfile::tempdir().unwrap();
        init_clean_git_workspace(workspace.path());
        let config = test_server_config(workspace.path());
        let store = SqliteWorkspaceStore::open(config.database_path.clone()).unwrap();
        let api = WorkspaceApi::new(config, Arc::new(store)).await.unwrap();
        let workspace_id = api.config.workspace_id.clone();

        let result = scoped_start_workspace_orchestrator(
            State(api.clone()),
            AxumPath(ScopedWorkspacePath {
                workspace_id: workspace_id.clone(),
            }),
        )
        .await;

        let Json(started) = result.unwrap_or_else(|error| {
            panic!(
                "production Workspace Orchestrator launch failed: error={:?}, diagnostics={:?}",
                error.error, error.diagnostics
            )
        });
        assert_eq!(started.disposition, "created");
        assert!(started.online);
        let worker = started
            .worker
            .expect("production Workspace Orchestrator Worker")
            .worker;

        let stopped = api
            .runtime
            .stop_worker(
                &worker,
                WorkerLifecycleRequest {
                    reason: Some("production Orchestrator restore regression test".to_string()),
                    ticket_assignment: None,
                },
            )
            .unwrap();
        assert_eq!(stopped.state, WorkerOperationState::Accepted);
        let error = scoped_start_workspace_orchestrator(
            State(api.clone()),
            AxumPath(ScopedWorkspacePath {
                workspace_id: workspace_id.clone(),
            }),
        )
        .await
        .expect_err("pending Workspace Orchestrator restore without durable Prompt must fail");
        assert!(
            format!("{error:?}").contains(
                "pending Workspace Worker restore requires operation-owned launch material"
            ),
            "unexpected restore error: {error:?}"
        );
    }

    #[tokio::test]
    async fn rejected_orchestrator_spawn_stays_offline_and_can_be_retried() {
        let workspace = tempfile::tempdir().unwrap();
        init_clean_git_workspace(workspace.path());
        let config = test_server_config(workspace.path());
        let store = SqliteWorkspaceStore::open(config.database_path.clone()).unwrap();
        let api = WorkspaceApi::new_with_execution_backend(
            config,
            Arc::new(store),
            Arc::new(DeterministicExecutionBackend::fail_first_spawn(
                "provider setup rejected safe-root /tmp/private token=private-token session_id=private-session",
            )),
        )
        .await
        .unwrap();
        let workspace_id = api.config.workspace_id.clone();

        let error = scoped_start_workspace_orchestrator(
            State(api.clone()),
            AxumPath(ScopedWorkspacePath {
                workspace_id: workspace_id.clone(),
            }),
        )
        .await
        .unwrap_err();
        assert!(matches!(
            error.error,
            Error::RuntimeOperationFailed {
                ref code,
                ..
            } if code == "workspace_orchestrator_spawn_rejected"
        ));
        assert!(error.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "embedded_worker_execution_rejected"
                && diagnostic
                    .message
                    .contains("provider setup rejected safe-root")
                && !diagnostic.message.contains("/tmp/private")
                && !diagnostic.message.contains("private-token")
                && !diagnostic.message.contains("private-session")
        }));
        assert!(find_workspace_orchestrator(&api).is_none());
        assert!(!workspace_orchestrator_response(&api, "failed").online);

        let Json(retried) = scoped_start_workspace_orchestrator(
            State(api),
            AxumPath(ScopedWorkspacePath { workspace_id }),
        )
        .await
        .unwrap();
        assert_eq!(retried.disposition, "created");
        assert!(retried.online);
    }

    #[tokio::test]
    async fn worker_control_spawn_retry_converges_on_one_worker_and_one_grant() {
        let workspace = tempfile::tempdir().unwrap();
        init_clean_git_workspace(workspace.path());
        let api = test_api(workspace.path()).await;
        let workspace_id = api.config.workspace_id.clone();
        let Json(controller_worker) = create_workspace_worker(
            State(api.clone()),
            HeaderMap::new(),
            Json(CreateWorkspaceWorkerRequest {
                runtime_id: EMBEDDED_WORKER_RUNTIME_ID.to_string(),
                display_name: "Control caller".to_string(),
                profile: None,
                ticket_assignment: None,
                initial_submit: Vec::new(),
                working_directory: None,
                control_operation_id: None,
                resolved_control_operation: None,
            }),
        )
        .await
        .unwrap();
        let controller = controller_worker.worker_ref;
        assert_ne!(
            scoped_worker_control_operation_id(&controller, "same-operation"),
            scoped_worker_control_operation_id(
                &RuntimeWorkerRef::new(&controller.runtime_id, "different-controller"),
                "same-operation",
            ),
            "Runtime idempotency keys are scoped to the authenticated controller"
        );
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-yoi-runtime-id",
            axum::http::HeaderValue::from_str(&controller.runtime_id).unwrap(),
        );
        headers.insert(
            "x-yoi-worker-id",
            axum::http::HeaderValue::from_str(&controller.worker_id).unwrap(),
        );
        let request = || CreateWorkspaceWorkerRequest {
            runtime_id: EMBEDDED_WORKER_RUNTIME_ID.to_string(),
            display_name: "Idempotent controlled child".to_string(),
            profile: None,
            ticket_assignment: None,
            initial_submit: Vec::new(),
            working_directory: None,
            control_operation_id: Some("control-spawn-retry".to_string()),
            resolved_control_operation: None,
        };

        let Json(first) = spawn_known_worker(
            State(api.clone()),
            AxumPath(ScopedWorkspacePath {
                workspace_id: workspace_id.clone(),
            }),
            headers.clone(),
            Json(request()),
        )
        .await
        .unwrap();
        let Json(retried) = spawn_known_worker(
            State(api.clone()),
            AxumPath(ScopedWorkspacePath {
                workspace_id: workspace_id.clone(),
            }),
            headers,
            Json(request()),
        )
        .await
        .unwrap();

        assert_eq!(retried.worker_ref, first.worker_ref);
        let mut conflicting_request = request();
        conflicting_request.display_name = "Different controlled child".to_string();
        let conflict = spawn_known_worker(
            State(api.clone()),
            AxumPath(ScopedWorkspacePath {
                workspace_id: workspace_id.clone(),
            }),
            {
                let mut headers = HeaderMap::new();
                headers.insert(
                    "x-yoi-runtime-id",
                    axum::http::HeaderValue::from_str(&controller.runtime_id).unwrap(),
                );
                headers.insert(
                    "x-yoi-worker-id",
                    axum::http::HeaderValue::from_str(&controller.worker_id).unwrap(),
                );
                headers
            },
            Json(conflicting_request),
        )
        .await
        .unwrap_err();
        assert_eq!(conflict.into_response().status(), StatusCode::BAD_GATEWAY);
        let grants = api
            .store
            .list_active_worker_control_grants(&workspace_id, &controller, 10)
            .unwrap();
        assert_eq!(grants.len(), 1);
        assert_eq!(grants[0].subject, first.worker_ref);
        assert_eq!(grants[0].operation_id, "control-spawn-retry");
    }

    #[tokio::test]
    async fn explicit_orchestrator_launch_marks_only_the_dedicated_worker() {
        let workspace = tempfile::tempdir().unwrap();
        init_clean_git_workspace(workspace.path());
        let api = test_api(workspace.path()).await;
        let workspace_id = api.config.workspace_id.clone();

        let Json(generic) = create_workspace_worker(
            State(api.clone()),
            HeaderMap::new(),
            Json(CreateWorkspaceWorkerRequest {
                runtime_id: EMBEDDED_WORKER_RUNTIME_ID.to_string(),
                display_name: "Generic Orchestrator Profile Worker".to_string(),
                profile: Some("builtin:orchestrator".to_string()),
                ticket_assignment: None,
                initial_submit: Vec::new(),
                working_directory: None,
                control_operation_id: None,
                resolved_control_operation: None,
            }),
        )
        .await
        .unwrap();
        assert_eq!(generic.worker.singleton_key, None);
        assert!(find_workspace_orchestrator(&api).is_none());
        let reserved = create_workspace_worker(
            State(api.clone()),
            HeaderMap::new(),
            Json(CreateWorkspaceWorkerRequest {
                runtime_id: EMBEDDED_WORKER_RUNTIME_ID.to_string(),
                display_name: crate::hosts::WORKSPACE_ORCHESTRATOR_SINGLETON_KEY.to_string(),
                profile: Some("builtin:orchestrator".to_string()),
                ticket_assignment: None,
                initial_submit: Vec::new(),
                working_directory: None,
                control_operation_id: None,
                resolved_control_operation: None,
            }),
        )
        .await
        .unwrap_err();
        assert!(matches!(reserved.error, Error::ReservedWorkerName(_)));

        let Json(started) = scoped_start_workspace_orchestrator(
            State(api.clone()),
            AxumPath(ScopedWorkspacePath {
                workspace_id: workspace_id.clone(),
            }),
        )
        .await
        .unwrap();
        assert_eq!(started.disposition, "created");
        let dedicated = started.worker.expect("dedicated Orchestrator Worker");
        assert_eq!(
            dedicated.singleton_key.as_deref(),
            Some(crate::hosts::WORKSPACE_ORCHESTRATOR_SINGLETON_KEY)
        );
        assert_ne!(dedicated.worker.worker_id, generic.worker_ref.worker_id);

        api.store
            .create_worker_control_grant(&WorkerControlGrantRecord {
                workspace_id: workspace_id.clone(),
                grant_id: "orchestrator-controls-generic".to_string(),
                controller: dedicated.worker.clone(),
                subject: generic.worker_ref.clone(),
                relation: "spawned".to_string(),
                origin: "test".to_string(),
                permissions: vec!["observe".to_string()],
                operation_id: "observe-generic".to_string(),
                created_at: "2026-07-27T00:00:00Z".to_string(),
                revoked_at: None,
            })
            .unwrap();

        let mut observation_headers = HeaderMap::new();
        observation_headers.insert(
            "x-yoi-runtime-id",
            axum::http::HeaderValue::from_str(&dedicated.worker.runtime_id).unwrap(),
        );
        observation_headers.insert(
            "x-yoi-worker-id",
            axum::http::HeaderValue::from_str(&dedicated.worker.worker_id).unwrap(),
        );
        let Json(known) = list_known_workers(
            State(api.clone()),
            AxumPath(ScopedWorkspacePath {
                workspace_id: workspace_id.clone(),
            }),
            observation_headers.clone(),
        )
        .await
        .unwrap();
        assert_eq!(known.items.len(), 1);
        assert_eq!(known.items[0].subject, generic.worker_ref);
        assert_eq!(known.items[0].permissions, ["observe"]);

        let Json(sessions) = scoped_list_worker_observation_sessions(
            State(api.clone()),
            AxumPath(ScopedWorkspacePath {
                workspace_id: workspace_id.clone(),
            }),
            observation_headers.clone(),
        )
        .await
        .unwrap();
        assert!(
            sessions["sessions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|session| {
                    session["subject"]["kind"] == "runtime_worker"
                        && session["subject"]["runtime_id"] == generic.worker_ref.runtime_id
                        && session["subject"]["worker_id"] == generic.worker_ref.worker_id
                })
        );
        let Json(capture) = scoped_capture_worker_observation_session(
            State(api.clone()),
            AxumPath(ScopedWorkspacePath {
                workspace_id: workspace_id.clone(),
            }),
            observation_headers.clone(),
            Json(WorkerObservationSubjectRef::RuntimeWorker {
                runtime_id: generic.worker_ref.runtime_id.clone(),
                worker_id: generic.worker_ref.worker_id.clone(),
            }),
        )
        .await
        .unwrap();
        assert!(capture["entries"].is_array());

        let revoked = api
            .store
            .revoke_worker_control_grant(
                &workspace_id,
                "orchestrator-controls-generic",
                "2026-07-27T00:00:02Z",
            )
            .unwrap();
        assert!(revoked);
        let revoked_capture = scoped_capture_worker_observation_session(
            State(api.clone()),
            AxumPath(ScopedWorkspacePath {
                workspace_id: workspace_id.clone(),
            }),
            observation_headers.clone(),
            Json(WorkerObservationSubjectRef::RuntimeWorker {
                runtime_id: generic.worker_ref.runtime_id.clone(),
                worker_id: generic.worker_ref.worker_id.clone(),
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(
            revoked_capture.into_response().status(),
            StatusCode::NOT_FOUND
        );

        let mut unauthorized_headers = HeaderMap::new();
        unauthorized_headers.insert(
            "x-yoi-runtime-id",
            axum::http::HeaderValue::from_str(&generic.worker_ref.runtime_id).unwrap(),
        );
        unauthorized_headers.insert(
            "x-yoi-worker-id",
            axum::http::HeaderValue::from_str(&generic.worker_ref.worker_id).unwrap(),
        );
        let Json(unauthorized) = scoped_list_worker_observation_sessions(
            State(api.clone()),
            AxumPath(ScopedWorkspacePath {
                workspace_id: workspace_id.clone(),
            }),
            unauthorized_headers,
        )
        .await
        .unwrap();
        assert!(unauthorized["sessions"].as_array().unwrap().is_empty());

        let Json(existing) = scoped_start_workspace_orchestrator(
            State(api.clone()),
            AxumPath(ScopedWorkspacePath {
                workspace_id: workspace_id.clone(),
            }),
        )
        .await
        .unwrap();
        assert_eq!(existing.disposition, "existing");
        assert_eq!(
            existing.worker.unwrap().worker.worker_id,
            dedicated.worker.worker_id
        );

        let Json(status) = scoped_workspace_orchestrator_status(
            State(api),
            AxumPath(ScopedWorkspacePath { workspace_id }),
        )
        .await
        .unwrap();
        assert_eq!(
            status.worker.unwrap().worker.worker_id,
            dedicated.worker.worker_id
        );
    }

    #[tokio::test]
    async fn workspace_workdir_summaries_include_runtime_observed_rows() {
        let dir = tempfile::tempdir().unwrap();
        let api = test_api(dir.path()).await;
        seed_test_repository(&api, "repo");
        api.store
            .upsert_workdir_registry(&WorkdirRegistryRecord {
                workspace_id: TEST_WORKSPACE_ID.to_string(),
                workdir_id: "managed".to_string(),
                runtime_id: EMBEDDED_WORKER_RUNTIME_ID.to_string(),
                repository_id: "repo".to_string(),
                creation_selector: None,
                creation_ref: None,
                current_selector: None,
                current_ref: None,
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
                creation_selector: None,
                creation_ref: None,
                current_selector: None,
                current_ref: None,
                materialization_status: "present".to_string(),
                cleanliness: "unknown".to_string(),
                created_at: "1".to_string(),
                updated_at: "2".to_string(),
            })
            .unwrap();
        api.store
            .upsert_worker_registry(&WorkerRegistryRecord {
                workspace_id: TEST_WORKSPACE_ID.to_string(),
                worker: RuntimeWorkerRef::new(EMBEDDED_WORKER_RUNTIME_ID, "7"),
                display_name: "Worker Seven".to_string(),
                profile: Some("builtin:coder".to_string()),
                retention_state: "normal".to_string(),
                transcript_ref: None,
                session_ref: None,
                summary_ref: None,
                diagnostics_ref: None,
                created_at: "1".to_string(),
                updated_at: "2".to_string(),
            })
            .unwrap();
        api.store
            .attach_worker_workdir(&WorkerWorkdirLinkRecord {
                workspace_id: TEST_WORKSPACE_ID.to_string(),
                worker: RuntimeWorkerRef::new(EMBEDDED_WORKER_RUNTIME_ID, "7"),
                workdir_id: "managed".to_string(),
                role: "attachment".to_string(),
                linked_at: "3".to_string(),
                unlinked_at: None,
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
        let managed = summaries
            .iter()
            .find(|summary| summary.working_directory_id == "managed")
            .unwrap();
        let occupied_by = managed.occupied_by.as_ref().unwrap();
        assert_eq!(
            occupied_by.worker,
            RuntimeWorkerRef::new(EMBEDDED_WORKER_RUNTIME_ID, "7")
        );
        assert_eq!(occupied_by.display_name, "Worker Seven");
        assert_eq!(occupied_by.linked_at, "3");

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
            creation_selector: None,
            creation_ref: None,
            current_selector: None,
            current_ref: None,
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

    struct DeterministicExecutionBackend {
        contexts: std::sync::Mutex<
            std::collections::HashMap<
                worker_runtime::identity::WorkerRef,
                worker_runtime::execution::WorkerExecutionContext,
            >,
        >,
        materializer: worker_runtime::working_directory::LocalGitWorktreeMaterializer,
        spawn_failure: std::sync::Mutex<Option<String>>,
        input_failure: std::sync::Mutex<Option<String>>,
        inputs: std::sync::Mutex<Vec<(worker_runtime::identity::WorkerRef, String)>>,
        protocol_methods:
            std::sync::Mutex<Vec<(worker_runtime::identity::WorkerRef, protocol::Method)>>,
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
                spawn_failure: std::sync::Mutex::new(None),
                input_failure: std::sync::Mutex::new(None),
                inputs: std::sync::Mutex::new(Vec::new()),
                protocol_methods: std::sync::Mutex::new(Vec::new()),
            }
        }
    }

    impl DeterministicExecutionBackend {
        fn reject_inputs(&self, message: impl Into<String>) {
            *self.input_failure.lock().unwrap() = Some(message.into());
        }

        fn take_inputs(&self) -> Vec<(worker_runtime::identity::WorkerRef, String)> {
            std::mem::take(&mut *self.inputs.lock().expect("inputs lock"))
        }

        fn protocol_methods(&self) -> Vec<(worker_runtime::identity::WorkerRef, protocol::Method)> {
            self.protocol_methods.lock().unwrap().clone()
        }

        fn fail_first_spawn(message: impl Into<String>) -> Self {
            let backend = Self::default();
            *backend.spawn_failure.lock().unwrap() = Some(message.into());
            backend
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
            if let Some(message) = self.spawn_failure.lock().unwrap().take() {
                return worker_runtime::execution::WorkerExecutionSpawnResult::Errored(
                    worker_runtime::execution::WorkerExecutionResult::errored(
                        worker_runtime::execution::WorkerExecutionOperation::Spawn,
                        message,
                    ),
                );
            }
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

        fn dispatch_method(
            &self,
            handle: &worker_runtime::execution::WorkerExecutionHandle,
            method: protocol::Method,
        ) -> worker_runtime::execution::WorkerExecutionResult {
            self.protocol_methods
                .lock()
                .unwrap()
                .push((handle.worker_ref().clone(), method));
            worker_runtime::execution::WorkerExecutionResult::accepted(
                worker_runtime::execution::WorkerExecutionOperation::ProtocolMethod,
                worker_runtime::execution::WorkerExecutionRunState::Idle,
            )
        }

        fn stop_worker(
            &self,
            _handle: &worker_runtime::execution::WorkerExecutionHandle,
        ) -> worker_runtime::execution::WorkerExecutionResult {
            worker_runtime::execution::WorkerExecutionResult::accepted(
                worker_runtime::execution::WorkerExecutionOperation::Stop,
                worker_runtime::execution::WorkerExecutionRunState::Stopped,
            )
        }

        fn cancel_worker(
            &self,
            _handle: &worker_runtime::execution::WorkerExecutionHandle,
        ) -> worker_runtime::execution::WorkerExecutionResult {
            worker_runtime::execution::WorkerExecutionResult::accepted(
                worker_runtime::execution::WorkerExecutionOperation::Cancel,
                worker_runtime::execution::WorkerExecutionRunState::Stopped,
            )
        }

        fn dispatch_input(
            &self,
            handle: &worker_runtime::execution::WorkerExecutionHandle,
            input: worker_runtime::interaction::WorkerInput,
        ) -> worker_runtime::execution::WorkerExecutionResult {
            self.inputs
                .lock()
                .expect("inputs lock")
                .push((handle.worker_ref().clone(), input.content.clone()));
            if let Some(message) = self.input_failure.lock().unwrap().clone() {
                return worker_runtime::execution::WorkerExecutionResult::errored(
                    worker_runtime::execution::WorkerExecutionOperation::Input,
                    message,
                );
            }
            let context = self
                .contexts
                .lock()
                .unwrap()
                .get(handle.worker_ref())
                .cloned()
                .expect("execution context");
            let submission_id = input.submission_id.clone();
            let content = input.content.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(25));
                let _ = context.publish_protocol_event(protocol::Event::TextDone {
                    text: format!("server companion echoed: {content}"),
                });
            });
            if let Some(submission_id) = submission_id {
                worker_runtime::execution::WorkerExecutionResult::accepted_input_committed(
                    worker_runtime::execution::WorkerExecutionOperation::Input,
                    worker_runtime::execution::WorkerExecutionRunState::Idle,
                    submission_id,
                )
            } else {
                worker_runtime::execution::WorkerExecutionResult::accepted(
                    worker_runtime::execution::WorkerExecutionOperation::Input,
                    worker_runtime::execution::WorkerExecutionRunState::Idle,
                )
            }
        }
    }

    fn test_identity() -> WorkspaceIdentity {
        WorkspaceIdentity {
            workspace_id: TEST_WORKSPACE_ID.to_string(),
            display_name: "Test Workspace".to_string(),
            created_at: TEST_CREATED_AT.to_string(),
        }
    }

    #[test]
    fn catalog_workspace_with_remote_source_does_not_require_server_local_repository() {
        let base = test_server_config(tempfile::tempdir().unwrap().path());
        let workspace = WorkspaceRecord {
            workspace_id: "remote-workspace".to_string(),
            display_name: "Remote Workspace".to_string(),
            state: "active".to_string(),
            owner_account_id: None,
            created_at: "1".to_string(),
            updated_at: "1".to_string(),
        };
        let source = workspace_api::RepositorySource {
            kind: workspace_api::RepositorySourceKind::Https,
            uri: "https://example.test/org/repository.git".to_string(),
        };
        let repositories = vec![RepositoryRecord {
            workspace_id: "remote-workspace".to_string(),
            repository_id: "main".to_string(),
            name: "Main".to_string(),
            kind: "git".to_string(),
            provider: Some("git".to_string()),
            source_fingerprint: crate::repository_source::repository_source_fingerprint(&source),
            source,
            default_ref: Some("main".to_string()),
            source_revision: 1,
            observed_status: workspace_api::RepositoryObservedStatus::Unverified,
            observed_at: None,
            created_at: "1".to_string(),
            updated_at: "1".to_string(),
        }];

        let scoped = base
            .for_catalog_workspace(&workspace, repositories)
            .unwrap();
        assert!(scoped.repositories[0].path.is_none());
        assert_eq!(
            scoped.repositories[0].source.kind,
            workspace_api::RepositorySourceKind::Https
        );
        assert_eq!(
            scoped.workspace_root,
            ServerConfig::default_workspace_backend_data_root("remote-workspace")
        );
    }

    fn test_server_config(workspace_root: impl Into<PathBuf>) -> ServerConfig {
        let workspace_root = workspace_root.into();
        let store_root = workspace_root.join(".test-embedded-runtime-store");
        let mut config = ServerConfig::local_dev(workspace_root.clone(), test_identity())
            .with_embedded_runtime_store_root(store_root);
        config.database_path = workspace_root.join(".test-yoi-server.db");
        config.runtime_config_path = Some(workspace_root.join(".test-config/runtimes.toml"));
        let source = workspace_api::RepositorySource {
            kind: workspace_api::RepositorySourceKind::LocalPath,
            uri: workspace_root.display().to_string(),
        };
        config.repositories = vec![ConfiguredRepository {
            id: TEST_REPOSITORY_ID.to_string(),
            provider: "git".to_string(),
            source_fingerprint: crate::repository_source::repository_source_fingerprint(&source),
            source,
            source_revision: 1,
            observed_status: workspace_api::RepositoryObservedStatus::Unverified,
            observed_at: None,
            path: Some(workspace_root),
            display_name: Some("Test Repository".to_string()),
            default_selector: Some("HEAD".to_string()),
        }];
        config
    }

    fn test_control_store(config: &ServerConfig) -> SqliteWorkspaceStore {
        if let Some(parent) = config.database_path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        SqliteWorkspaceStore::open(&config.database_path).unwrap()
    }

    #[tokio::test]
    async fn server_router_serves_workspace_chooser_before_first_workspace_exists() {
        let dir = tempfile::tempdir().unwrap();
        let static_dir = dir.path().join("static");
        std::fs::create_dir_all(static_dir.join("_app/immutable/entry")).unwrap();
        std::fs::write(
            static_dir.join("index.html"),
            "<main>Workspace chooser</main>",
        )
        .unwrap();
        std::fs::write(
            static_dir.join("_app/immutable/entry/start.js"),
            "console.log('workspace app');",
        )
        .unwrap();
        let mut template = test_server_config(dir.path());
        template.static_assets_dir = Some(static_dir);
        let store = Arc::new(SqliteWorkspaceStore::open(&template.database_path).unwrap());
        let app = build_workspace_server_router(template, store)
            .await
            .unwrap();
        let empty_catalog = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/workspaces")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(empty_catalog.status(), StatusCode::OK);
        assert_eq!(
            to_bytes(empty_catalog.into_body(), usize::MAX)
                .await
                .unwrap(),
            "[]"
        );

        for (uri, expected) in [
            ("/", "<main>Workspace chooser</main>"),
            ("/account", "<main>Workspace chooser</main>"),
            ("/login/device", "<main>Workspace chooser</main>"),
            (
                "/_app/immutable/entry/start.js",
                "console.log('workspace app');",
            ),
        ] {
            let response = app
                .clone()
                .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(
                String::from_utf8(
                    to_bytes(response.into_body(), usize::MAX)
                        .await
                        .unwrap()
                        .to_vec(),
                )
                .unwrap(),
                expected
            );
        }

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/workspaces")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(serde_json::from_slice::<Value>(&body).unwrap(), json!([]));

        let auth = app
            .oneshot(
                Request::builder()
                    .uri("/api/auth/config")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(auth.status(), StatusCode::OK);
        let body = to_bytes(auth.into_body(), usize::MAX).await.unwrap();
        let auth: Value = serde_json::from_slice(&body).unwrap();
        assert!(auth["rp_id"].is_string());
        assert!(auth["cookie_name"].is_string());
    }

    #[tokio::test]
    async fn workspace_server_router_requires_identity_for_scoped_rest() {
        let temp = tempfile::tempdir().unwrap();
        let config = test_server_config(temp.path());
        let AuthConfig::Passkey {
            origin: expected_origin,
            ..
        } = &config.auth;
        let expected_origin = expected_origin.clone();
        let store = Arc::new(SqliteWorkspaceStore::open(&config.database_path).unwrap());
        let catalog = WorkspaceCatalogService::new(store.clone());
        let repository = temp.path().join("repository");
        std::fs::create_dir_all(&repository).unwrap();
        assert!(
            std::process::Command::new("git")
                .args(["init", "-q"])
                .current_dir(&repository)
                .status()
                .unwrap()
                .success()
        );
        let workspace = catalog
            .create(
                WorkspaceCreateRequest {
                    operation_key: "create-auth".to_owned(),
                    display_name: "Auth Workspace".to_owned(),
                    repository: crate::workspace_catalog::InitialRepositoryIntent {
                        uri: repository.display().to_string(),
                        display_name: None,
                        default_ref: None,
                    },
                },
                None,
            )
            .unwrap();
        store
            .upsert_account(&AccountRecord {
                account_id: "account-auth".to_owned(),
                kind: "user".to_owned(),
                handle: "auth-user".to_owned(),
                display_name: "Auth User".to_owned(),
                created_at: "2026-01-01T00:00:00Z".to_owned(),
                updated_at: "2026-01-01T00:00:00Z".to_owned(),
            })
            .unwrap();
        store
            .upsert_user(&UserRecord {
                user_id: "user-auth".to_owned(),
                account_id: "account-auth".to_owned(),
                handle: "auth-user".to_owned(),
                display_name: "Auth User".to_owned(),
                created_at: "2026-01-01T00:00:00Z".to_owned(),
                updated_at: "2026-01-01T00:00:00Z".to_owned(),
            })
            .unwrap();
        store
            .create_api_token(&ApiTokenRecord {
                token_hash: crate::auth::token_hash("api-token-auth"),
                token_id: "token-auth".to_owned(),
                user_id: "user-auth".to_owned(),
                label: "test".to_owned(),
                created_at: "2026-01-01T00:00:00Z".to_owned(),
                expires_at: None,
                last_used_at: None,
                revoked_at: None,
            })
            .unwrap();
        store
            .create_browser_session(&BrowserSessionRecord {
                token_hash: crate::auth::token_hash("browser-session-auth"),
                session_id: "session-auth".to_owned(),
                user_id: "user-auth".to_owned(),
                created_at: "2026-01-01T00:00:00Z".to_owned(),
                expires_at: "2099-01-01T00:00:00Z".to_owned(),
                revoked_at: None,
            })
            .unwrap();
        let app = build_workspace_server_router(config, store).await.unwrap();
        let uri = format!("/api/w/{}/workspace", workspace.workspace.workspace_id);

        let anonymous = app
            .clone()
            .oneshot(Request::builder().uri(&uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);

        for legacy_path in ["/api/workspace", "/api/runtimes", "/api/tickets"] {
            let anonymous_legacy = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(legacy_path)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                anonymous_legacy.status(),
                StatusCode::UNAUTHORIZED,
                "{legacy_path} must not bypass Workspace auth"
            );
        }
        let authenticated_legacy = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/workspace")
                    .header(axum::http::header::AUTHORIZATION, "Bearer api-token-auth")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(authenticated_legacy.status(), StatusCode::OK);

        let anonymous_catalog = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/workspaces")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(anonymous_catalog.status(), StatusCode::UNAUTHORIZED);
        let authenticated_catalog = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/workspaces")
                    .header(axum::http::header::AUTHORIZATION, "Bearer api-token-auth")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(authenticated_catalog.status(), StatusCode::OK);

        for path in ["/api/workspaces", "/api/auth/device-login/approve"] {
            let cross_site = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri(path)
                        .header(
                            axum::http::header::COOKIE,
                            "yoi_workspace_session=browser-session-auth",
                        )
                        .header(CONTENT_TYPE, "application/json")
                        .body(Body::from("{}"))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                cross_site.status(),
                StatusCode::FORBIDDEN,
                "{path} must reject a cookie-authenticated cross-site mutation"
            );
            let same_origin = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri(path)
                        .header(
                            axum::http::header::COOKIE,
                            "yoi_workspace_session=browser-session-auth",
                        )
                        .header(ORIGIN, expected_origin.as_str())
                        .header(CONTENT_TYPE, "application/json")
                        .body(Body::from("{}"))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_ne!(
                same_origin.status(),
                StatusCode::FORBIDDEN,
                "{path} must accept the configured Browser origin"
            );
        }

        let ws_uri = format!("/api/w/{}/protocol/ws", workspace.workspace.workspace_id);
        let anonymous_ws = app
            .clone()
            .oneshot(Request::builder().uri(&ws_uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(anonymous_ws.status(), StatusCode::UNAUTHORIZED);
        let authenticated_ws = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(ws_uri)
                    .header(axum::http::header::AUTHORIZATION, "Bearer api-token-auth")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_ne!(authenticated_ws.status(), StatusCode::UNAUTHORIZED);

        let authenticated = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(&uri)
                    .header(axum::http::header::AUTHORIZATION, "Bearer api-token-auth")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(authenticated.status(), StatusCode::OK);

        let settings_uri = format!(
            "/api/w/{}/settings/workspace",
            workspace.workspace.workspace_id
        );
        let csrf_rejected = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri(&settings_uri)
                    .header(
                        axum::http::header::COOKIE,
                        "yoi_workspace_session=browser-session-auth",
                    )
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"display_name":"Renamed"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(csrf_rejected.status(), StatusCode::FORBIDDEN);

        let mixed_auth_csrf_rejected = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri(&settings_uri)
                    .header(
                        axum::http::header::COOKIE,
                        "yoi_workspace_session=browser-session-auth",
                    )
                    .header(axum::http::header::AUTHORIZATION, "Bearer invalid-token")
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"display_name":"Renamed"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(mixed_auth_csrf_rejected.status(), StatusCode::FORBIDDEN);

        let csrf_accepted = app
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri(settings_uri)
                    .header(
                        axum::http::header::COOKIE,
                        "yoi_workspace_session=browser-session-auth",
                    )
                    .header(ORIGIN, expected_origin)
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"display_name":"Renamed"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_ne!(csrf_accepted.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn direct_workspace_router_enforces_origin_on_browser_auth_mutations() {
        let workspace = tempfile::tempdir().unwrap();
        let api = test_api(workspace.path()).await;
        seed_test_api_token(api.store.as_ref(), "direct-cookie");
        api.store
            .create_browser_session(&BrowserSessionRecord {
                token_hash: crate::auth::token_hash("direct-browser-session"),
                session_id: "direct-session".to_owned(),
                user_id: "user-direct-cookie".to_owned(),
                created_at: "2026-01-01T00:00:00Z".to_owned(),
                expires_at: "2099-01-01T00:00:00Z".to_owned(),
                revoked_at: None,
            })
            .unwrap();
        let AuthConfig::Passkey {
            origin,
            cookie_name,
            ..
        } = &api.config.auth;
        let origin = origin.clone();
        let cookie = format!("{cookie_name}=direct-browser-session");
        let app = build_router(api.clone());

        let cross_site = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/auth/device-login/approve")
                    .header(axum::http::header::COOKIE, &cookie)
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(cross_site.status(), StatusCode::FORBIDDEN);

        let same_origin = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/auth/device-login/approve")
                    .header(axum::http::header::COOKIE, cookie)
                    .header(ORIGIN, origin)
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_ne!(same_origin.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn browser_session_set_and_clear_cookies_follow_public_https_scheme() {
        for (scheme, secure) in [("http", false), ("https", true)] {
            let workspace = tempfile::tempdir().unwrap();
            let mut api = test_api(workspace.path()).await;
            let AuthConfig::Passkey {
                origin,
                public_base_url,
                ..
            } = &mut api.config.auth;
            *origin = format!("{scheme}://workspace.test");
            *public_base_url = format!("{scheme}://workspace.test");
            seed_test_api_token(api.store.as_ref(), &format!("cookie-{scheme}"));
            let user = api
                .store
                .get_user(&format!("user-cookie-{scheme}"))
                .unwrap()
                .unwrap();
            let auth_api = ServerAuthApi::from(&api);

            let login = issue_browser_session_response(&auth_api, user).unwrap();
            let login_cookie = login.headers().get(SET_COOKIE).unwrap().to_str().unwrap();
            assert_eq!(login_cookie.contains("; Secure"), secure, "{scheme}");
            assert!(login_cookie.contains("; HttpOnly; SameSite=Lax"));

            let logout = post_auth_logout(State(auth_api), HeaderMap::new())
                .await
                .unwrap();
            let logout_cookie = logout.headers().get(SET_COOKIE).unwrap().to_str().unwrap();
            assert_eq!(logout_cookie.contains("; Secure"), secure, "{scheme}");
            assert!(logout_cookie.contains("Max-Age=0"));
            assert!(logout_cookie.contains("; HttpOnly; SameSite=Lax"));
            for cookie in [login_cookie, logout_cookie] {
                assert!(cookie.contains("; Path=/"));
                assert!(!cookie.contains("; Domain="));
            }
        }
    }

    #[tokio::test]
    async fn server_router_dispatches_two_workspace_contexts_without_state_leakage() {
        let dir = tempfile::tempdir().unwrap();
        let repository_a = dir.path().join("repository-a");
        let repository_b = dir.path().join("repository-b");
        std::fs::create_dir_all(repository_a.join(".git")).unwrap();
        std::fs::create_dir_all(repository_b.join(".git")).unwrap();
        let static_dir = dir.path().join("static");
        std::fs::create_dir_all(static_dir.join("_app/immutable/entry")).unwrap();
        std::fs::write(
            static_dir.join("index.html"),
            "<main>Workspace chooser</main>",
        )
        .unwrap();
        std::fs::write(
            static_dir.join("_app/immutable/entry/start.js"),
            "console.log('workspace app');",
        )
        .unwrap();
        let mut template = test_server_config(dir.path());
        template.static_assets_dir = Some(static_dir);
        let store = Arc::new(SqliteWorkspaceStore::open(&template.database_path).unwrap());
        let catalog = WorkspaceCatalogService::new(store.clone());
        let workspace_a = catalog
            .create(
                WorkspaceCreateRequest {
                    operation_key: "create-a".to_string(),
                    display_name: "Workspace A".to_string(),
                    repository: crate::workspace_catalog::InitialRepositoryIntent {
                        uri: repository_a.display().to_string(),
                        display_name: None,
                        default_ref: None,
                    },
                },
                None,
            )
            .unwrap();
        let workspace_b = catalog
            .create(
                WorkspaceCreateRequest {
                    operation_key: "create-b".to_string(),
                    display_name: "Workspace B".to_string(),
                    repository: crate::workspace_catalog::InitialRepositoryIntent {
                        uri: repository_b.display().to_string(),
                        display_name: None,
                        default_ref: None,
                    },
                },
                None,
            )
            .unwrap();
        let token = seed_test_api_token(store.as_ref(), "two-workspaces");
        let app = build_workspace_server_router(template, store)
            .await
            .unwrap();

        let uri_a = format!("/api/w/{}/workspace", workspace_a.workspace.workspace_id);
        let uri_b = format!("/api/w/{}/workspace", workspace_b.workspace.workspace_id);
        let (a, b) = tokio::join!(
            get_json_authenticated(app.clone(), &uri_a, &token),
            get_json_authenticated(app.clone(), &uri_b, &token)
        );
        assert_eq!(a["workspace_id"], workspace_a.workspace.workspace_id);
        assert_eq!(a["display_name"], "Workspace A");
        assert_eq!(b["workspace_id"], workspace_b.workspace.workspace_id);
        assert_eq!(b["display_name"], "Workspace B");

        let chooser = app
            .clone()
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(chooser.status(), StatusCode::OK);
        assert_eq!(
            String::from_utf8(
                to_bytes(chooser.into_body(), usize::MAX)
                    .await
                    .unwrap()
                    .to_vec(),
            )
            .unwrap(),
            "<main>Workspace chooser</main>"
        );

        for asset_uri in [
            "/_app/immutable/entry/start.js".to_string(),
            format!(
                "/w/{}/_app/immutable/entry/start.js",
                workspace_a.workspace.workspace_id
            ),
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(asset_uri)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(
                String::from_utf8(
                    to_bytes(response.into_body(), usize::MAX)
                        .await
                        .unwrap()
                        .to_vec(),
                )
                .unwrap(),
                "console.log('workspace app');"
            );
        }

        let handle = missing_resource_handle();
        let resource_response = app
            .clone()
            .oneshot(
                Request::post(format!(
                    "/api/runtime/v1/workspaces/{}/resources/fetch",
                    workspace_b.workspace.workspace_id
                ))
                .header(axum::http::header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&BackendResourceFetchRequest {
                        audit_correlation_id: handle.audit_correlation_id.clone(),
                        runtime_id: "runtime-test".to_string(),
                        worker_id: None,
                        handle,
                    })
                    .unwrap(),
                ))
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resource_response.status(), StatusCode::UNAUTHORIZED);

        let missing = app
            .oneshot(
                Request::builder()
                    .uri("/api/w/00000000-0000-0000-0000-000000000001/workspace")
                    .header(axum::http::header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn local_bootstrap_create_activates_workspace_without_server_restart() {
        let dir = tempfile::tempdir().unwrap();
        let repository = dir.path().join("repository");
        std::fs::create_dir_all(repository.join(".git")).unwrap();
        let template = test_server_config(dir.path()).with_local_workspace_bootstrap(true);
        let store = Arc::new(SqliteWorkspaceStore::open(&template.database_path).unwrap());
        let token = seed_test_api_token(store.as_ref(), "bootstrap");
        let app = build_workspace_server_router(template, store)
            .await
            .unwrap();
        let payload = json!({
            "operation_key": "bootstrap-1",
            "display_name": "Created Workspace",
            "repository": {
                "uri": repository,
                "display_name": "Repository",
                "default_ref": "HEAD"
            }
        });

        let created = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/workspaces")
                    .header(axum::http::header::CONTENT_TYPE, "application/json")
                    .body(Body::from(payload.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(created.status(), StatusCode::CREATED);
        let body = to_bytes(created.into_body(), usize::MAX).await.unwrap();
        let body: Value = serde_json::from_slice(&body).unwrap();
        let workspace_id = body["workspace"]["workspace_id"].as_str().unwrap();

        let workspace = get_json_authenticated(
            app.clone(),
            &format!("/api/w/{workspace_id}/workspace"),
            &token,
        )
        .await;
        assert_eq!(workspace["display_name"], "Created Workspace");

        let replayed = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/workspaces")
                    .header(axum::http::header::CONTENT_TYPE, "application/json")
                    .body(Body::from(payload.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(replayed.status(), StatusCode::OK);

        let second_payload = json!({
            "operation_key": "bootstrap-2",
            "display_name": "Second Ownerless Workspace",
            "repository": {
                "uri": repository,
                "display_name": "Repository",
                "default_ref": "HEAD"
            }
        });
        let second = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/workspaces")
                    .header(axum::http::header::CONTENT_TYPE, "application/json")
                    .body(Body::from(second_payload.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(second.status(), StatusCode::CONFLICT);
    }

    #[test]
    fn scoped_workspace_path_requires_an_explicit_workspace_segment() {
        assert_eq!(
            scoped_workspace_id("/api/w/workspace-a/tickets"),
            Some("workspace-a")
        );
        assert_eq!(
            scoped_workspace_id("/w/workspace-b/workers"),
            Some("workspace-b")
        );
        assert_eq!(
            scoped_workspace_id("/internal/w/workspace-c/runtime/resources/fetch"),
            Some("workspace-c")
        );
        assert_eq!(scoped_workspace_id("/api/workspaces"), None);
        assert_eq!(scoped_workspace_id("/api/workspace"), None);
    }

    fn memory_staging_record_json(id: &str, claim: &str) -> String {
        json!({
            "schema_version": 1,
            "id": id,
            "extract_run_id": "extract-run-1",
            "source": {
                "segment_id": "segment-1",
                "range": [0, 10],
            },
            "kind": "working_assumption",
            "claim": claim,
            "why_useful": "useful for future work",
            "staleness": null,
            "evidence": [],
            "source_refs": [],
        })
        .to_string()
    }

    #[tokio::test]
    async fn memory_consolidation_backlog_ignores_legacy_filesystem_staging() {
        let dir = tempfile::tempdir().unwrap();
        let config = test_server_config(dir.path());
        let store = SqliteWorkspaceStore::open(&config.database_path).unwrap();
        let api = WorkspaceApi::new_with_execution_backend(
            config,
            Arc::new(store),
            Arc::new(DeterministicExecutionBackend::default()),
        )
        .await
        .unwrap();
        let legacy_staging_dir = dir.path().join(".yoi/memory/_staging");
        fs::create_dir_all(&legacy_staging_dir).unwrap();
        fs::write(
            legacy_staging_dir.join("00000000001J4.json"),
            memory_staging_record_json("00000000001J4", "legacy filesystem candidate"),
        )
        .unwrap();

        let output = match start_memory_staging_consolidation(
            api,
            MemoryConsolidateStagingOperation {
                force: true,
                threshold_files: None,
                threshold_bytes: None,
            },
        ) {
            Ok(output) => output,
            Err(_) => panic!("unexpected ApiError from memory consolidation trigger"),
        };

        assert_eq!(output.status, "skipped_empty");
        assert_eq!(output.candidate_count, 0);
        assert_eq!(output.total_bytes, 0);
    }

    #[tokio::test]
    async fn memory_consolidation_reuses_existing_idle_worker() {
        let dir = tempfile::tempdir().unwrap();
        let config = test_server_config(dir.path());
        let store = SqliteWorkspaceStore::open(&config.database_path).unwrap();
        let api = WorkspaceApi::new_with_execution_backend(
            config,
            Arc::new(store),
            Arc::new(DeterministicExecutionBackend::default()),
        )
        .await
        .unwrap();
        api.authority
            .upsert_memory_staging_record(
                "00000000000000000000000001",
                &memory_staging_record_json("00000000000000000000000001", "first candidate"),
                None,
            )
            .unwrap();

        let resolved_config_bundle = None;
        let existing = api
            .runtime
            .spawn_worker(
                EMBEDDED_WORKER_RUNTIME_ID,
                test_create_binding(),
                WorkerSpawnRequest {
                    requested_worker_name: Some(MEMORY_CONSOLIDATION_PROFILE.to_string()),
                    intent: WorkerSpawnIntent::WorkspaceOrchestrator,
                    acceptance: WorkerSpawnAcceptanceRequirement::RunAccepted {
                        expected_segments: 0,
                    },
                    profile: ProfileSelector::Builtin(MEMORY_CONSOLIDATION_PROFILE.to_string()),
                    ticket_assignment: None,
                    initial_submit: Vec::new(),
                    working_directory_request: None,
                    resolved_working_directory_request: None,
                    resolved_working_directory: None,
                    resolved_config_bundle,
                    resolved_worker_observation_enabled: false,
                    resolved_worker_observation_grants: Vec::new(),
                    resolved_workspace_api: Some(test_worker_workspace_api(
                        EMBEDDED_WORKER_RUNTIME_ID,
                    )),
                    resolved_memory_settings: Some(test_worker_memory_settings()),
                    resolved_control_operation: None,
                },
            )
            .unwrap();
        assert_eq!(existing.state, WorkerOperationState::Accepted);
        let worker_id = existing.worker.unwrap().worker.worker_id;
        let worker = api
            .runtime
            .worker(&RuntimeWorkerRef::new(
                EMBEDDED_WORKER_RUNTIME_ID,
                &worker_id,
            ))
            .unwrap();
        assert_eq!(worker.state, "idle");
        assert_eq!(worker.display_name, "Memory Consolidation");
        assert_eq!(
            worker.singleton_key.as_deref(),
            Some(MEMORY_CONSOLIDATION_SINGLETON_KEY)
        );
        assert!(worker.tags.iter().any(|tag| tag == "memory"));
        assert!(worker.tags.iter().any(|tag| tag == "consolidation"));

        let second = match start_memory_staging_consolidation(
            api.clone(),
            MemoryConsolidateStagingOperation {
                force: true,
                threshold_files: None,
                threshold_bytes: None,
            },
        ) {
            Ok(output) => output,
            Err(_) => panic!("unexpected ApiError from second memory consolidation trigger"),
        };
        assert_eq!(second.status, "reused");
        assert!(second.summary.contains(&worker_id));
        let workers_after_second = api
            .runtime
            .list_workers_for_runtime(EMBEDDED_WORKER_RUNTIME_ID, 20)
            .unwrap();
        let consolidaters_after_second = workers_after_second
            .items
            .iter()
            .filter(|worker| is_memory_consolidation_worker(worker))
            .collect::<Vec<_>>();
        assert_eq!(consolidaters_after_second.len(), 1);
        assert_eq!(consolidaters_after_second[0].worker.worker_id, worker_id);
    }

    fn init_clean_git_workspace(path: &std::path::Path) {
        for args in [
            vec!["init", "--initial-branch=develop"],
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
    async fn mark_ready_resolves_workspace_target_and_closes_lifecycle_bypasses() {
        let dir = tempfile::tempdir().unwrap();
        init_clean_git_workspace(dir.path());
        let api = test_api(dir.path()).await;
        let backend = browser_ticket_backend(&api).unwrap();

        let mut input = ticket::NewTicket::new("Validated target");
        input.repository_id = Some(TEST_REPOSITORY_ID.to_owned());
        input.ref_selector = Some("develop".to_owned());
        let ticket_ref = backend.create(input).unwrap();
        let request = ticket::TicketMarkReady {
            operation_key: "ready-server-test".to_owned(),
            reason: Some("target accepted".to_owned()),
            author: Some("test".to_owned()),
            intake_summary: None,
        };
        let ready = backend
            .mark_ready(TicketIdOrSlug::Id(ticket_ref.id.clone()), request.clone())
            .unwrap();
        assert_eq!(ready.meta.workflow_state, TicketWorkflowState::Ready);
        assert_eq!(
            ready.meta.repository_id.as_deref(),
            Some(TEST_REPOSITORY_ID)
        );
        assert_eq!(ready.meta.ref_selector.as_deref(), Some("develop"));
        assert_eq!(
            backend
                .mark_ready(TicketIdOrSlug::Id(ticket_ref.id.clone()), request)
                .unwrap()
                .events
                .iter()
                .filter(|event| event.attributes.contains_key("operation_key"))
                .count(),
            1
        );
        assert!(matches!(
            backend.edit_item(
                TicketIdOrSlug::Id(ticket_ref.id.clone()),
                ticket::TicketItemEdit {
                    target: Some(ticket::TicketTargetEdit::Set {
                        repository_id: TEST_REPOSITORY_ID.to_owned(),
                        ref_selector: Some("other".to_owned()),
                    }),
                    ..Default::default()
                },
            ),
            Err(ticket::TicketError::Conflict(_))
        ));

        let mut missing = ticket::NewTicket::new("Missing target");
        missing.repository_id = Some("unknown".to_owned());
        assert!(backend.create(missing).is_err());
        assert!(matches!(
            backend.set_workflow_state(
                TicketIdOrSlug::Id(ticket_ref.id),
                TicketStateChange::new("ready", "queued", "bypass", "must use TicketQueue",),
            ),
            Err(ticket::TicketError::InvalidWorkflowTransition { .. })
        ));
    }

    #[test]
    fn worker_source_actor_roles_use_canonical_vocabulary() {
        assert_eq!(worker_source_actor_role(true, false), "coder");
        assert_eq!(worker_source_actor_role(false, true), "orchestrator");
        assert_eq!(worker_source_actor_role(false, false), "worker");
        assert_eq!(worker_source_actor_role(true, true), "coder");
    }

    #[test]
    fn ticket_notification_projection_exposes_only_ticket_and_current_state() {
        for current_state in ["queued", "inprogress"] {
            let content = ticket_notification_content("00001KZ9SR97B", current_state);
            assert_eq!(
                content,
                format!(
                    "Ticket notification: ticket_id=00001KZ9SR97B current_state={current_state}. Reread the Ticket before acting."
                )
            );
            for forbidden in [
                "workspace_id",
                "event_sequence",
                "event_kind",
                "source_operation_kind",
                "source_runtime_id",
                "source_worker_id",
                "runtime_id",
                "worker_id",
                "operation_id",
                "assignment_id",
            ] {
                assert!(
                    !content.contains(forbidden),
                    "notification leaked forbidden field {forbidden}: {content}"
                );
            }
        }
    }

    #[tokio::test]
    async fn orchestrator_ticket_notifications_project_authoritative_post_mutation_state() {
        let dir = tempfile::tempdir().unwrap();
        init_clean_git_workspace(dir.path());
        let (api, execution) = test_api_with_recording_backend(dir.path()).await;
        let source_worker = api
            .runtime
            .spawn_worker(
                EMBEDDED_WORKER_RUNTIME_ID,
                test_create_binding(),
                WorkerSpawnRequest {
                    requested_worker_name: Some("notification-source".to_string()),
                    intent: WorkerSpawnIntent::TicketRole {
                        ticket_id: "notification-source".to_string(),
                        role: TicketWorkerRole::Coder,
                    },
                    acceptance: WorkerSpawnAcceptanceRequirement::RunAccepted {
                        expected_segments: 0,
                    },
                    profile: ProfileSelector::Builtin("builtin:coder".to_string()),
                    ticket_assignment: None,
                    initial_submit: Vec::new(),
                    working_directory_request: None,
                    resolved_working_directory_request: None,
                    resolved_working_directory: None,
                    resolved_config_bundle: None,
                    resolved_worker_observation_enabled: false,
                    resolved_worker_observation_grants: Vec::new(),
                    resolved_workspace_api: Some(test_worker_workspace_api(
                        EMBEDDED_WORKER_RUNTIME_ID,
                    )),
                    resolved_memory_settings: Some(test_worker_memory_settings()),
                    resolved_control_operation: None,
                },
            )
            .unwrap()
            .worker
            .unwrap();
        let source =
            RuntimeWorkerRef::new(EMBEDDED_WORKER_RUNTIME_ID, source_worker.worker.worker_id);
        api.store
            .upsert_worker_registry(&WorkerRegistryRecord {
                workspace_id: TEST_WORKSPACE_ID.to_string(),
                worker: source.clone(),
                display_name: "Notification Source".to_string(),
                profile: Some("builtin:coder".to_string()),
                retention_state: "normal".to_string(),
                transcript_ref: None,
                session_ref: None,
                summary_ref: None,
                diagnostics_ref: None,
                created_at: TEST_CREATED_AT.to_string(),
                updated_at: TEST_CREATED_AT.to_string(),
            })
            .unwrap();
        let source_headers = || {
            let mut headers = HeaderMap::new();
            headers.insert(
                "x-yoi-runtime-id",
                axum::http::HeaderValue::from_str(&source.runtime_id).unwrap(),
            );
            headers.insert(
                "x-yoi-worker-id",
                axum::http::HeaderValue::from_str(&source.worker_id).unwrap(),
            );
            headers
        };
        let Json(started) = scoped_start_workspace_orchestrator(
            State(api.clone()),
            AxumPath(ScopedWorkspacePath {
                workspace_id: TEST_WORKSPACE_ID.to_string(),
            }),
        )
        .await
        .unwrap();
        let orchestrator = started.worker.unwrap().worker;
        execution.take_inputs();

        let mut input = ticket::NewTicket::new("Bounded notification");
        input.repository_id = Some(TEST_REPOSITORY_ID.to_owned());
        input.ref_selector = Some("develop".to_owned());
        let ticket = browser_ticket_backend(&api).unwrap().create(input).unwrap();
        assign_test_orchestrator(&api, &ticket.id);
        let ticket_id = TicketIdOrSlug::Id(ticket.id.clone());
        let preparatory_operations = [
            TicketBackendOperation::MarkReady {
                id: ticket_id.clone(),
                request: ticket::TicketMarkReady {
                    operation_key: "notification-ready".to_owned(),
                    reason: Some("ready for implementation".to_owned()),
                    author: None,
                    intake_summary: None,
                },
            },
            TicketBackendOperation::QueueReady {
                id: ticket_id.clone(),
                queued_by: "spoofed".to_owned(),
            },
        ];
        for operation in preparatory_operations {
            execute_ticket_rest_operation(&api, TEST_WORKSPACE_ID, source_headers(), operation)
                .await
                .unwrap();
        }
        api.store
            .set_current_ticket_coder_assignment(
                &TicketCoderAssignmentRecord {
                    workspace_id: TEST_WORKSPACE_ID.to_string(),
                    ticket_id: ticket.id.clone(),
                    assignment_id: "notification-source-assignment".to_string(),
                    worker: source.clone(),
                    assigned_by: "workspace-orchestrator".to_string(),
                    assigned_at: TEST_CREATED_AT.to_string(),
                },
                None,
                "notification-source-assignment-event",
                "notification-source-assignment-operation",
                false,
            )
            .unwrap();
        accept_queued_ticket_after_worker_spawn(
            &api,
            &crate::hosts::WorkerTicketAssignmentRequest {
                ticket_id: ticket.id.clone(),
                operation_id: "notification-source-assignment-operation".to_string(),
            },
        )
        .unwrap();

        let operations = [
            TicketBackendOperation::AddEvent {
                id: ticket_id.clone(),
                event: NewTicketEvent::new(TicketEventKind::Comment, "progress comment"),
            },
            TicketBackendOperation::AddEvent {
                id: ticket_id.clone(),
                event: NewTicketEvent::new(
                    TicketEventKind::ImplementationReport,
                    "implementation report",
                ),
            },
            TicketBackendOperation::AddEvent {
                id: ticket_id,
                event: NewTicketEvent::new(TicketEventKind::Decision, "review update"),
            },
        ];
        for operation in operations {
            execute_ticket_rest_operation(&api, TEST_WORKSPACE_ID, source_headers(), operation)
                .await
                .unwrap();
        }

        let inputs = execution.take_inputs();
        let expected_states = ["queued", "inprogress", "inprogress", "inprogress"];
        assert_eq!(inputs.len(), expected_states.len());
        for ((recipient, content), current_state) in inputs.iter().zip(expected_states) {
            assert_eq!(recipient.worker_id.to_string(), orchestrator.worker_id);
            assert_eq!(
                content,
                &ticket_notification_content(&ticket.id, current_state)
            );
        }
    }

    #[tokio::test]
    async fn role_assignment_endpoint_replays_same_operation_result() {
        let dir = tempfile::tempdir().unwrap();
        let api = test_api(dir.path()).await;
        let ticket = browser_ticket_backend(&api)
            .unwrap()
            .create(ticket::NewTicket::new("Idempotent assignment"))
            .unwrap();
        let request = || SetTicketRoleAssignmentRequest {
            operation_id: "same-role-operation".to_string(),
            principal: TicketAssignmentPrincipal::WorkspaceAgent {
                agent_key: "workspace-orchestrator".to_string(),
            },
            expected_assignment_id: None,
        };
        let path = || {
            AxumPath((
                TEST_WORKSPACE_ID.to_string(),
                ticket.id.clone(),
                "orchestrator".to_string(),
            ))
        };

        let Json(first) = scoped_set_ticket_assignment(State(api.clone()), path(), Json(request()))
            .await
            .unwrap();
        let Json(replay) = scoped_set_ticket_assignment(State(api), path(), Json(request()))
            .await
            .unwrap();
        assert_eq!(replay.assignment, first.assignment);
    }

    #[tokio::test]
    async fn ticket_assignment_endpoints_read_and_clear_current_assignment() {
        let dir = tempfile::tempdir().unwrap();
        let api = test_api(dir.path()).await;
        let created = browser_ticket_backend(&api)
            .unwrap()
            .create(ticket::NewTicket::new("Assigned Ticket"))
            .unwrap();
        let ticket_id = created.id;
        api.store
            .upsert_worker_registry(&WorkerRegistryRecord {
                workspace_id: TEST_WORKSPACE_ID.to_string(),
                worker: RuntimeWorkerRef::new("embedded", "42"),
                display_name: "Worker 42".to_string(),
                profile: Some("builtin:coder".to_string()),
                retention_state: "normal".to_string(),
                transcript_ref: None,
                session_ref: None,
                summary_ref: None,
                diagnostics_ref: None,
                created_at: TEST_CREATED_AT.to_string(),
                updated_at: TEST_CREATED_AT.to_string(),
            })
            .unwrap();
        let invalid_coder = scoped_set_ticket_assignment(
            State(api.clone()),
            AxumPath((
                TEST_WORKSPACE_ID.to_string(),
                ticket_id.clone(),
                "coder".to_string(),
            )),
            Json(SetTicketRoleAssignmentRequest {
                operation_id: "invalid-workspace-agent-coder".to_string(),
                principal: TicketAssignmentPrincipal::WorkspaceAgent {
                    agent_key: "workspace-orchestrator".to_string(),
                },
                expected_assignment_id: None,
            }),
        )
        .await
        .unwrap_err()
        .into_response();
        assert_eq!(invalid_coder.status(), StatusCode::CONFLICT);

        let assignment = TicketCoderAssignmentRecord {
            workspace_id: TEST_WORKSPACE_ID.to_string(),
            ticket_id: ticket_id.clone(),
            assignment_id: "assignment-api-1".to_string(),
            worker: RuntimeWorkerRef::new("embedded", "42"),
            assigned_by: "test-user".to_string(),
            assigned_at: TEST_CREATED_AT.to_string(),
        };
        api.store
            .set_current_ticket_coder_assignment(
                &assignment,
                None,
                "event-api-1",
                "operation-api-1",
                false,
            )
            .unwrap();
        assign_test_orchestrator(&api, &ticket_id);
        let path = || ScopedRecordPath {
            workspace_id: TEST_WORKSPACE_ID.to_string(),
            id: ticket_id.clone(),
        };

        let Json(read) = scoped_list_ticket_assignments(State(api.clone()), AxumPath(path()))
            .await
            .unwrap();
        assert_eq!(read.assignments.len(), 2);
        let coder = read
            .assignments
            .iter()
            .find(|assignment| assignment.role == TicketAssignmentRole::Coder)
            .unwrap();
        assert_eq!(coder.assignment_id, assignment.assignment_id);
        let Json(detail) = browser_ticket_detail(&api, &ticket_id).unwrap();
        assert!(
            !detail
                .action_eligibility
                .blockers
                .iter()
                .any(|blocker| blocker.contains("Orchestrator and manual Coder"))
        );
        assert!(!detail.action_eligibility.can_assign_orchestrator);
        assert!(!detail.action_eligibility.can_start_manual_coder);

        let stale = scoped_clear_ticket_assignment(
            State(api.clone()),
            AxumPath((
                TEST_WORKSPACE_ID.to_string(),
                ticket_id.clone(),
                "coder".to_string(),
            )),
            Query(ClearTicketRoleAssignmentQuery {
                operation_id: Some("clear-stale".to_string()),
                assignment_id: Some("stale-assignment".to_string()),
            }),
        )
        .await
        .unwrap_err()
        .into_response();
        assert_eq!(stale.status(), StatusCode::CONFLICT);

        let Json(cleared) = scoped_clear_ticket_assignment(
            State(api.clone()),
            AxumPath((
                TEST_WORKSPACE_ID.to_string(),
                ticket_id.clone(),
                "coder".to_string(),
            )),
            Query(ClearTicketRoleAssignmentQuery {
                operation_id: Some("clear-current".to_string()),
                assignment_id: Some("assignment-api-1".to_string()),
            }),
        )
        .await
        .unwrap();
        assert_eq!(cleared.assignment, None);
        let Json(replayed_clear) = scoped_clear_ticket_assignment(
            State(api),
            AxumPath((
                TEST_WORKSPACE_ID.to_string(),
                ticket_id,
                "coder".to_string(),
            )),
            Query(ClearTicketRoleAssignmentQuery {
                operation_id: Some("clear-current".to_string()),
                assignment_id: Some("assignment-api-1".to_string()),
            }),
        )
        .await
        .unwrap();
        assert_eq!(replayed_clear.assignment, None);
    }

    #[tokio::test]
    async fn implementation_cancellation_cancels_coder_and_returns_ticket_to_ready() {
        let dir = tempfile::tempdir().unwrap();
        let api = test_api(dir.path()).await;
        let worker = api
            .runtime
            .spawn_worker(
                EMBEDDED_WORKER_RUNTIME_ID,
                test_create_binding(),
                WorkerSpawnRequest {
                    requested_worker_name: Some("cancelled-coder".to_string()),
                    intent: WorkerSpawnIntent::TicketRole {
                        ticket_id: "implementation-cancellation".to_string(),
                        role: TicketWorkerRole::Coder,
                    },
                    acceptance: WorkerSpawnAcceptanceRequirement::RunAccepted {
                        expected_segments: 0,
                    },
                    profile: ProfileSelector::Builtin("builtin:coder".to_string()),
                    ticket_assignment: None,
                    initial_submit: Vec::new(),
                    working_directory_request: None,
                    resolved_working_directory_request: None,
                    resolved_working_directory: None,
                    resolved_config_bundle: None,
                    resolved_worker_observation_enabled: false,
                    resolved_worker_observation_grants: Vec::new(),
                    resolved_workspace_api: Some(test_worker_workspace_api(
                        EMBEDDED_WORKER_RUNTIME_ID,
                    )),
                    resolved_memory_settings: Some(test_worker_memory_settings()),
                    resolved_control_operation: None,
                },
            )
            .unwrap()
            .worker
            .unwrap()
            .worker;
        let worker = RuntimeWorkerRef::new(EMBEDDED_WORKER_RUNTIME_ID, worker.worker_id);
        api.store
            .upsert_worker_registry(&WorkerRegistryRecord {
                workspace_id: TEST_WORKSPACE_ID.to_string(),
                worker: worker.clone(),
                display_name: "Cancelled Coder".to_string(),
                profile: Some("builtin:coder".to_string()),
                retention_state: "normal".to_string(),
                transcript_ref: None,
                session_ref: None,
                summary_ref: None,
                diagnostics_ref: None,
                created_at: TEST_CREATED_AT.to_string(),
                updated_at: TEST_CREATED_AT.to_string(),
            })
            .unwrap();
        let backend = browser_ticket_backend(&api).unwrap();
        let mut input = ticket::NewTicket::new("Implementation cancellation");
        input.workflow_state = Some(TicketWorkflowState::InProgress);
        let ticket = backend.create(input).unwrap();
        api.store
            .set_current_ticket_coder_assignment(
                &TicketCoderAssignmentRecord {
                    workspace_id: TEST_WORKSPACE_ID.to_string(),
                    ticket_id: ticket.id.clone(),
                    assignment_id: "cancelled-assignment".to_string(),
                    worker: worker.clone(),
                    assigned_by: "test-user".to_string(),
                    assigned_at: TEST_CREATED_AT.to_string(),
                },
                None,
                "cancelled-assignment-event",
                "cancelled-assignment-operation",
                false,
            )
            .unwrap();
        let path = || {
            AxumPath(ScopedRecordPath {
                workspace_id: TEST_WORKSPACE_ID.to_string(),
                id: ticket.id.clone(),
            })
        };
        let request = || {
            Json(CancelTicketImplementationRequest {
                operation_id: "cancel-implementation-operation".to_string(),
                assignment_id: "cancelled-assignment".to_string(),
                reason: "redo with the corrected design".to_string(),
            })
        };

        let Json(cancelled) =
            scoped_cancel_ticket_implementation(State(api.clone()), path(), request())
                .await
                .unwrap();
        assert_eq!(cancelled.state, TicketWorkflowState::Ready.as_str());
        assert!(cancelled.current_coder.is_none());
        assert!(
            !cancelled
                .assignments
                .iter()
                .any(|assignment| assignment.role == "coder")
        );
        assert_eq!(api.runtime.worker(&worker).unwrap().state, "cancelled");

        let Json(replayed) = scoped_cancel_ticket_implementation(State(api), path(), request())
            .await
            .unwrap();
        assert_eq!(replayed.state, TicketWorkflowState::Ready.as_str());
    }

    #[tokio::test]
    async fn authenticated_worker_ticket_mutation_notifies_current_assignment() {
        let dir = tempfile::tempdir().unwrap();
        let api = test_api(dir.path()).await;
        let spawn = |name: &str| WorkerSpawnRequest {
            requested_worker_name: Some(name.to_string()),
            intent: WorkerSpawnIntent::TicketRole {
                ticket_id: name.to_string(),
                role: TicketWorkerRole::Coder,
            },
            acceptance: WorkerSpawnAcceptanceRequirement::RunAccepted {
                expected_segments: 0,
            },
            profile: ProfileSelector::Builtin("builtin:coder".to_string()),
            ticket_assignment: None,
            initial_submit: Vec::new(),
            working_directory_request: None,
            resolved_working_directory_request: None,
            resolved_working_directory: None,
            resolved_config_bundle: None,
            resolved_worker_observation_enabled: false,
            resolved_worker_observation_grants: Vec::new(),
            resolved_workspace_api: Some(test_worker_workspace_api(EMBEDDED_WORKER_RUNTIME_ID)),
            resolved_memory_settings: Some(test_worker_memory_settings()),
            resolved_control_operation: None,
        };
        let source_worker = api
            .runtime
            .spawn_worker(
                EMBEDDED_WORKER_RUNTIME_ID,
                test_create_binding(),
                spawn("source-worker"),
            )
            .unwrap()
            .worker
            .unwrap();
        api.store
            .upsert_worker_registry(&WorkerRegistryRecord {
                workspace_id: TEST_WORKSPACE_ID.to_string(),
                worker: RuntimeWorkerRef::new(
                    EMBEDDED_WORKER_RUNTIME_ID,
                    source_worker.worker.worker_id.clone(),
                ),
                display_name: "Source Worker".to_string(),
                profile: Some("builtin:coder".to_string()),
                retention_state: "normal".to_string(),
                transcript_ref: None,
                session_ref: None,
                summary_ref: None,
                diagnostics_ref: None,
                created_at: TEST_CREATED_AT.to_string(),
                updated_at: TEST_CREATED_AT.to_string(),
            })
            .unwrap();
        let recipient_worker = api
            .runtime
            .spawn_worker(
                EMBEDDED_WORKER_RUNTIME_ID,
                test_create_binding(),
                spawn("recipient-worker"),
            )
            .unwrap()
            .worker
            .unwrap();
        api.store
            .upsert_worker_registry(&WorkerRegistryRecord {
                workspace_id: TEST_WORKSPACE_ID.to_string(),
                worker: RuntimeWorkerRef::new(
                    EMBEDDED_WORKER_RUNTIME_ID,
                    recipient_worker.worker.worker_id.clone(),
                ),
                display_name: "Recipient Worker".to_string(),
                profile: Some("builtin:coder".to_string()),
                retention_state: "normal".to_string(),
                transcript_ref: None,
                session_ref: None,
                summary_ref: None,
                diagnostics_ref: None,
                created_at: TEST_CREATED_AT.to_string(),
                updated_at: TEST_CREATED_AT.to_string(),
            })
            .unwrap();
        let backend = browser_ticket_backend(&api).unwrap();
        let ticket_ref = backend
            .create(ticket::NewTicket::new("Notify assigned Worker"))
            .unwrap();
        api.store
            .set_current_ticket_coder_assignment(
                &TicketCoderAssignmentRecord {
                    workspace_id: TEST_WORKSPACE_ID.to_string(),
                    ticket_id: ticket_ref.id.clone(),
                    assignment_id: "notify-assignment".to_string(),
                    worker: RuntimeWorkerRef::new(
                        EMBEDDED_WORKER_RUNTIME_ID,
                        recipient_worker.worker.worker_id.clone(),
                    ),
                    assigned_by: "test-user".to_string(),
                    assigned_at: TEST_CREATED_AT.to_string(),
                },
                None,
                "notify-assignment-event",
                "notify-assignment-operation",
                false,
            )
            .unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-yoi-runtime-id",
            axum::http::HeaderValue::from_static(EMBEDDED_WORKER_RUNTIME_ID),
        );
        headers.insert(
            "x-yoi-worker-id",
            axum::http::HeaderValue::from_str(&source_worker.worker.worker_id).unwrap(),
        );
        let response = build_inner_router(api.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/api/w/{TEST_WORKSPACE_ID}/tickets/{}/thread-events",
                        ticket_ref.id
                    ))
                    .header("content-type", "application/json")
                    .header("x-yoi-runtime-id", EMBEDDED_WORKER_RUNTIME_ID)
                    .header("x-yoi-worker-id", &source_worker.worker.worker_id)
                    .body(Body::from(
                        serde_json::to_vec(&NewTicketEvent::new(
                            TicketEventKind::Comment,
                            "implementation update",
                        ))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        let committed = backend.show(ticket_ref.id.clone().into()).unwrap();
        let committed_event = committed.events.last().unwrap();
        assert_eq!(
            committed_event
                .attributes
                .get("source_runtime_id")
                .map(String::as_str),
            Some(EMBEDDED_WORKER_RUNTIME_ID)
        );
        assert_eq!(
            committed_event
                .attributes
                .get("source_worker_id")
                .map(String::as_str),
            Some(source_worker.worker.worker_id.as_str())
        );
        assert_eq!(
            committed_event
                .attributes
                .get("source_operation_kind")
                .map(String::as_str),
            Some("add_event")
        );
        assert!(committed_event.attributes.contains_key("event_id"));
        assert!(committed_event.attributes.contains_key("event_sequence"));
        let Json(stale_report) = execute_worker_ticket_test_operation(
            State(api.clone()),
            AxumPath(ScopedWorkspacePath {
                workspace_id: TEST_WORKSPACE_ID.to_string(),
            }),
            headers.clone(),
            Json(TicketBackendOperation::AddEvent {
                id: ticket_ref.id.clone().into(),
                event: NewTicketEvent::new(
                    TicketEventKind::ImplementationReport,
                    "non-assigned Worker report",
                ),
            }),
        )
        .await
        .unwrap();
        assert!(matches!(stale_report, TicketBackendOperationResult::Unit));
        let non_assigned_report = backend.show(ticket_ref.id.clone().into()).unwrap();
        assert!(
            !non_assigned_report
                .events
                .last()
                .unwrap()
                .attributes
                .contains_key("source_assignment_id")
        );

        api.store
            .set_current_ticket_coder_assignment(
                &TicketCoderAssignmentRecord {
                    workspace_id: TEST_WORKSPACE_ID.to_string(),
                    ticket_id: ticket_ref.id.clone(),
                    assignment_id: "source-assignment".to_string(),
                    worker: RuntimeWorkerRef::new(
                        EMBEDDED_WORKER_RUNTIME_ID,
                        source_worker.worker.worker_id.clone(),
                    ),
                    assigned_by: "test-user".to_string(),
                    assigned_at: TEST_CREATED_AT.to_string(),
                },
                Some("notify-assignment"),
                "source-assignment-event",
                "source-assignment-operation",
                true,
            )
            .unwrap();
        let _ = execute_worker_ticket_test_operation(
            State(api.clone()),
            AxumPath(ScopedWorkspacePath {
                workspace_id: TEST_WORKSPACE_ID.to_string(),
            }),
            headers.clone(),
            Json(TicketBackendOperation::AddEvent {
                id: ticket_ref.id.clone().into(),
                event: NewTicketEvent::new(
                    TicketEventKind::ImplementationReport,
                    "current assignment report",
                ),
            }),
        )
        .await
        .unwrap();
        let reported = backend.show(ticket_ref.id.clone().into()).unwrap();
        let report_event = reported.events.last().unwrap();
        assert_eq!(
            report_event
                .attributes
                .get("source_assignment_id")
                .map(String::as_str),
            Some("source-assignment")
        );
        assert_eq!(
            report_event
                .attributes
                .get("source_actor_role")
                .map(String::as_str),
            Some("coder")
        );

        let human_mutation = execute_worker_ticket_test_operation(
            State(api.clone()),
            AxumPath(ScopedWorkspacePath {
                workspace_id: TEST_WORKSPACE_ID.to_string(),
            }),
            HeaderMap::new(),
            Json(TicketBackendOperation::AddEvent {
                id: ticket_ref.id.clone().into(),
                event: NewTicketEvent::new(TicketEventKind::Comment, "human update"),
            }),
        )
        .await
        .unwrap();
        assert!(matches!(
            human_mutation.0,
            TicketBackendOperationResult::Unit
        ));
        let human_event = backend.show(ticket_ref.id.clone().into()).unwrap();
        let human_event = human_event.events.last().unwrap();
        assert!(!human_event.attributes.contains_key("source_runtime_id"));
        assert!(!human_event.attributes.contains_key("source_worker_id"));

        let mut incomplete_source_headers = HeaderMap::new();
        incomplete_source_headers.insert(
            "x-yoi-runtime-id",
            axum::http::HeaderValue::from_static(EMBEDDED_WORKER_RUNTIME_ID),
        );
        let incomplete_source = execute_worker_ticket_test_operation(
            State(api.clone()),
            AxumPath(ScopedWorkspacePath {
                workspace_id: TEST_WORKSPACE_ID.to_string(),
            }),
            incomplete_source_headers,
            Json(TicketBackendOperation::AddEvent {
                id: ticket_ref.id.clone().into(),
                event: NewTicketEvent::new(TicketEventKind::Comment, "incomplete source"),
            }),
        )
        .await
        .unwrap_err()
        .into_response();
        assert_eq!(incomplete_source.status(), StatusCode::BAD_REQUEST);

        let mut incomplete_source_headers = HeaderMap::new();
        incomplete_source_headers.insert(
            "x-yoi-worker-id",
            axum::http::HeaderValue::from_str(&source_worker.worker.worker_id).unwrap(),
        );
        let incomplete_source = execute_worker_ticket_test_operation(
            State(api.clone()),
            AxumPath(ScopedWorkspacePath {
                workspace_id: TEST_WORKSPACE_ID.to_string(),
            }),
            incomplete_source_headers,
            Json(TicketBackendOperation::AddEvent {
                id: ticket_ref.id.clone().into(),
                event: NewTicketEvent::new(TicketEventKind::Comment, "incomplete source"),
            }),
        )
        .await
        .unwrap_err()
        .into_response();
        assert_eq!(incomplete_source.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn generic_browser_state_mutation_cannot_bypass_assignment_start() {
        let dir = tempfile::tempdir().unwrap();
        let api = test_api(dir.path()).await;
        let backend = browser_ticket_backend(&api).unwrap();
        let ticket = backend
            .create(ticket::NewTicket::new("Generic transition guard"))
            .unwrap();
        let path = ScopedRecordPath {
            workspace_id: TEST_WORKSPACE_ID.to_string(),
            id: ticket.id.clone(),
        };

        let planning = scoped_transition_ticket_state(
            State(api.clone()),
            AxumPath(path.clone()),
            Json(BrowserTransitionTicketStateRequest {
                state: TicketWorkflowState::InProgress,
                reason: None,
                body: None,
                author: None,
            }),
        )
        .await
        .unwrap_err()
        .into_response();
        assert_eq!(planning.status(), StatusCode::CONFLICT);
        assert_eq!(
            backend
                .show(ticket.id.clone().into())
                .unwrap()
                .meta
                .workflow_state,
            TicketWorkflowState::Planning
        );

        let mut ready_input = ticket::NewTicket::new("Ready transition guard");
        ready_input.workflow_state = Some(TicketWorkflowState::Ready);
        let ready = backend.create(ready_input).unwrap();
        let ready_result = scoped_transition_ticket_state(
            State(api.clone()),
            AxumPath(ScopedRecordPath {
                workspace_id: TEST_WORKSPACE_ID.to_string(),
                id: ready.id.clone(),
            }),
            Json(BrowserTransitionTicketStateRequest {
                state: TicketWorkflowState::InProgress,
                reason: None,
                body: None,
                author: None,
            }),
        )
        .await
        .unwrap_err()
        .into_response();
        assert_eq!(ready_result.status(), StatusCode::CONFLICT);
        assert_eq!(
            backend.show(ready.id.into()).unwrap().meta.workflow_state,
            TicketWorkflowState::Ready
        );
    }

    #[tokio::test]
    async fn queue_requires_orchestrator_role_and_records_assignment_fence() {
        let dir = tempfile::tempdir().unwrap();
        init_clean_git_workspace(dir.path());
        let api = test_api(dir.path()).await;
        let backend = browser_ticket_backend(&api).unwrap();
        let mut input = ticket::NewTicket::new("Queue role gate");
        input.workflow_state = Some(TicketWorkflowState::Ready);
        input.repository_id = Some(TEST_REPOSITORY_ID.to_string());
        input.ref_selector = Some("develop".to_string());
        let ticket = backend.create(input).unwrap();
        let path = (TEST_WORKSPACE_ID.to_string(), ticket.id.clone());

        let legacy_missing = scoped_queue_ticket(
            State(api.clone()),
            AxumPath(ScopedRecordPath {
                workspace_id: TEST_WORKSPACE_ID.to_string(),
                id: ticket.id.clone(),
            }),
            Json(BrowserQueueTicketRequest {}),
        )
        .await
        .unwrap_err()
        .into_response();
        assert_eq!(legacy_missing.status(), StatusCode::CONFLICT);

        let missing = scoped_queue_ticket_record(
            State(api.clone()),
            AxumPath(path.clone()),
            HeaderMap::new(),
        )
        .await
        .unwrap_err()
        .into_response();
        assert_eq!(missing.status(), StatusCode::CONFLICT);
        assert_eq!(
            backend
                .show(ticket.id.clone().into())
                .unwrap()
                .meta
                .workflow_state,
            TicketWorkflowState::Ready
        );

        assign_test_orchestrator(&api, &ticket.id);
        scoped_queue_ticket_record(State(api.clone()), AxumPath(path), HeaderMap::new())
            .await
            .unwrap();
        let queued = backend.show(ticket.id.into()).unwrap();
        assert_eq!(queued.meta.workflow_state, TicketWorkflowState::Queued);
        let event = queued.events.last().unwrap();
        let expected_assignment_id = format!("orchestrator-{}", queued.meta.id);
        assert_eq!(
            event
                .attributes
                .get("orchestrator_assignment_id")
                .map(String::as_str),
            Some(expected_assignment_id.as_str())
        );
        assert!(event.attributes.contains_key("routing_operation_id"));
        assert!(event.attributes.contains_key("routing_request_fingerprint"));
    }

    #[tokio::test]
    async fn queued_ticket_mutation_succeeds_without_orchestrator() {
        let dir = tempfile::tempdir().unwrap();
        let api = test_api(dir.path()).await;
        let source = api
            .runtime
            .spawn_worker(
                EMBEDDED_WORKER_RUNTIME_ID,
                test_create_binding(),
                WorkerSpawnRequest {
                    requested_worker_name: Some("orchestrator-source".to_string()),
                    intent: WorkerSpawnIntent::TicketRole {
                        ticket_id: "source-ticket".to_string(),
                        role: TicketWorkerRole::Coder,
                    },
                    acceptance: WorkerSpawnAcceptanceRequirement::RunAccepted {
                        expected_segments: 0,
                    },
                    profile: ProfileSelector::Builtin("builtin:coder".to_string()),
                    ticket_assignment: None,
                    initial_submit: Vec::new(),
                    working_directory_request: None,
                    resolved_working_directory_request: None,
                    resolved_working_directory: None,
                    resolved_config_bundle: None,
                    resolved_worker_observation_enabled: false,
                    resolved_worker_observation_grants: Vec::new(),
                    resolved_workspace_api: Some(test_worker_workspace_api(
                        EMBEDDED_WORKER_RUNTIME_ID,
                    )),
                    resolved_memory_settings: Some(test_worker_memory_settings()),
                    resolved_control_operation: None,
                },
            )
            .unwrap()
            .worker
            .unwrap();
        let backend = browser_ticket_backend(&api).unwrap();
        let mut input = ticket::NewTicket::new("Queued notification");
        input.workflow_state = Some(TicketWorkflowState::Queued);
        let ticket_ref = backend.create(input).unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-yoi-runtime-id",
            axum::http::HeaderValue::from_static(EMBEDDED_WORKER_RUNTIME_ID),
        );
        headers.insert(
            "x-yoi-worker-id",
            axum::http::HeaderValue::from_str(&source.worker.worker_id).unwrap(),
        );
        let _ = execute_worker_ticket_test_operation(
            State(api.clone()),
            AxumPath(ScopedWorkspacePath {
                workspace_id: TEST_WORKSPACE_ID.to_string(),
            }),
            headers,
            Json(TicketBackendOperation::AddEvent {
                id: ticket_ref.id.clone().into(),
                event: NewTicketEvent::new(TicketEventKind::Comment, "queued update"),
            }),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn queued_ticket_mutation_stays_committed_when_notification_recipient_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        init_clean_git_workspace(dir.path());
        let (api, execution) = test_api_with_recording_backend(dir.path()).await;
        let source = api
            .runtime
            .spawn_worker(
                EMBEDDED_WORKER_RUNTIME_ID,
                test_create_binding(),
                WorkerSpawnRequest {
                    requested_worker_name: Some("orchestrator-source".to_string()),
                    intent: WorkerSpawnIntent::TicketRole {
                        ticket_id: "source-ticket".to_string(),
                        role: TicketWorkerRole::Coder,
                    },
                    acceptance: WorkerSpawnAcceptanceRequirement::RunAccepted {
                        expected_segments: 0,
                    },
                    profile: ProfileSelector::Builtin("builtin:coder".to_string()),
                    ticket_assignment: None,
                    initial_submit: Vec::new(),
                    working_directory_request: None,
                    resolved_working_directory_request: None,
                    resolved_working_directory: None,
                    resolved_config_bundle: None,
                    resolved_worker_observation_enabled: false,
                    resolved_worker_observation_grants: Vec::new(),
                    resolved_workspace_api: Some(test_worker_workspace_api(
                        EMBEDDED_WORKER_RUNTIME_ID,
                    )),
                    resolved_memory_settings: Some(test_worker_memory_settings()),
                    resolved_control_operation: None,
                },
            )
            .unwrap()
            .worker
            .unwrap();
        let orchestrator = scoped_start_workspace_orchestrator(
            State(api.clone()),
            AxumPath(ScopedWorkspacePath {
                workspace_id: TEST_WORKSPACE_ID.to_string(),
            }),
        )
        .await
        .unwrap()
        .0
        .worker
        .expect("Workspace Orchestrator should be available")
        .worker;
        let _ = execution.take_inputs();
        let backend = browser_ticket_backend(&api).unwrap();
        let mut input = ticket::NewTicket::new("Queued notification");
        input.workflow_state = Some(TicketWorkflowState::Queued);
        let ticket_ref = backend.create(input).unwrap();
        assign_test_orchestrator(&api, ticket_ref.id.as_str());
        let missing_recipient =
            RuntimeWorkerRef::new(EMBEDDED_WORKER_RUNTIME_ID, "missing-notification-recipient");
        api.store
            .upsert_worker_registry(&WorkerRegistryRecord {
                workspace_id: TEST_WORKSPACE_ID.to_string(),
                worker: missing_recipient.clone(),
                display_name: "Missing notification recipient".to_string(),
                profile: Some("builtin:coder".to_string()),
                retention_state: "normal".to_string(),
                transcript_ref: None,
                session_ref: None,
                summary_ref: None,
                diagnostics_ref: None,
                created_at: TEST_CREATED_AT.to_string(),
                updated_at: TEST_CREATED_AT.to_string(),
            })
            .unwrap();
        api.store
            .set_current_ticket_coder_assignment(
                &TicketCoderAssignmentRecord {
                    workspace_id: TEST_WORKSPACE_ID.to_string(),
                    ticket_id: ticket_ref.id.clone(),
                    assignment_id: "missing-recipient-assignment".to_string(),
                    worker: missing_recipient.clone(),
                    assigned_by: "test-user".to_string(),
                    assigned_at: TEST_CREATED_AT.to_string(),
                },
                None,
                "missing-recipient-assignment-event",
                "missing-recipient-assignment-operation",
                false,
            )
            .unwrap();
        TICKET_NOTIFICATION_DELIVERY_WARNING_CAPTURE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .retain(|warning| warning.ticket_id != ticket_ref.id);
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-yoi-runtime-id",
            axum::http::HeaderValue::from_static(EMBEDDED_WORKER_RUNTIME_ID),
        );
        headers.insert(
            "x-yoi-worker-id",
            axum::http::HeaderValue::from_str(&source.worker.worker_id).unwrap(),
        );
        let _ = execute_worker_ticket_test_operation(
            State(api.clone()),
            AxumPath(ScopedWorkspacePath {
                workspace_id: TEST_WORKSPACE_ID.to_string(),
            }),
            headers,
            Json(TicketBackendOperation::AddEvent {
                id: ticket_ref.id.clone().into(),
                event: NewTicketEvent::new(TicketEventKind::Comment, "queued update"),
            }),
        )
        .await
        .unwrap();

        assert_eq!(
            api.authority.ticket(&ticket_ref.id).unwrap().state,
            TicketWorkflowState::Queued.as_str()
        );
        let warnings = TICKET_NOTIFICATION_DELIVERY_WARNING_CAPTURE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .filter(|warning| warning.ticket_id == ticket_ref.id)
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(warnings.len(), 1);
        let warning = &warnings[0];
        assert_eq!(warning.level, "warning");
        assert_eq!(warning.event, "ticket_notification_delivery_failed");
        assert_eq!(warning.workspace_id, TEST_WORKSPACE_ID);
        assert_eq!(warning.current_state, TicketWorkflowState::Queued.as_str());
        assert_eq!(warning.recipient_runtime_id, missing_recipient.runtime_id);
        assert_eq!(warning.recipient_worker_id, missing_recipient.worker_id);
        assert_eq!(warning.error_category, "unknown_worker");
        let serialized = serde_json::to_string(warning).unwrap();
        assert!(!serialized.contains("queued update"));
        assert!(!serialized.contains("Ticket notification:"));

        let notifications = execution.take_inputs();
        assert_eq!(notifications.len(), 1);
        assert_eq!(
            notifications[0].0.worker_id.to_string(),
            orchestrator.worker_id
        );
        assert_eq!(
            notifications[0].1,
            ticket_notification_content(
                ticket_ref.id.as_str(),
                TicketWorkflowState::Queued.as_str()
            )
        );

        execution.reject_inputs("sensitive fake Runtime transport detail");
        TICKET_NOTIFICATION_DELIVERY_WARNING_CAPTURE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .retain(|warning| warning.ticket_id != ticket_ref.id);
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-yoi-runtime-id",
            axum::http::HeaderValue::from_static(EMBEDDED_WORKER_RUNTIME_ID),
        );
        headers.insert(
            "x-yoi-worker-id",
            axum::http::HeaderValue::from_str(&source.worker.worker_id).unwrap(),
        );
        let _ = execute_worker_ticket_test_operation(
            State(api.clone()),
            AxumPath(ScopedWorkspacePath {
                workspace_id: TEST_WORKSPACE_ID.to_string(),
            }),
            headers,
            Json(TicketBackendOperation::AddEvent {
                id: ticket_ref.id.clone().into(),
                event: NewTicketEvent::new(TicketEventKind::Comment, "all delivery failure update"),
            }),
        )
        .await
        .unwrap();

        assert_eq!(
            api.authority.ticket(&ticket_ref.id).unwrap().state,
            TicketWorkflowState::Queued.as_str()
        );
        let warnings = TICKET_NOTIFICATION_DELIVERY_WARNING_CAPTURE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .filter(|warning| warning.ticket_id == ticket_ref.id)
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(warnings.len(), 2);
        assert!(warnings.iter().any(|warning| {
            warning.recipient_worker_id == missing_recipient.worker_id
                && warning.error_category == "unknown_worker"
        }));
        assert!(warnings.iter().any(|warning| {
            warning.recipient_worker_id == orchestrator.worker_id
                && warning.error_category == "runtime_rejected"
        }));
        let serialized = serde_json::to_string(&warnings).unwrap();
        assert!(!serialized.contains("all delivery failure update"));
        assert!(!serialized.contains("Ticket notification:"));
        assert!(!serialized.contains("sensitive fake Runtime transport detail"));
        let attempts = execution.take_inputs();
        assert_eq!(attempts.len(), 1);
        assert_eq!(attempts[0].0.worker_id.to_string(), orchestrator.worker_id);
    }

    #[tokio::test]
    async fn orchestrator_running_to_idle_recovers_queued_ticket_without_notification_memory() {
        let dir = tempfile::tempdir().unwrap();
        let api = test_api(dir.path()).await;
        let backend = browser_ticket_backend(&api).unwrap();
        let mut input = ticket::NewTicket::new("Recover queued work");
        input.workflow_state = Some(TicketWorkflowState::Queued);
        input.repository_id = Some(TEST_REPOSITORY_ID.to_owned());
        input.ref_selector = Some("HEAD".to_owned());
        let ticket_ref = backend.create(input).unwrap();
        assign_test_orchestrator(&api, &ticket_ref.id);
        *api.orchestrator_attention_fingerprint.lock().unwrap() = Some(ticket_ref.id.clone());

        let Json(started) = scoped_start_workspace_orchestrator(
            State(api.clone()),
            AxumPath(ScopedWorkspacePath {
                workspace_id: TEST_WORKSPACE_ID.to_string(),
            }),
        )
        .await
        .unwrap();
        assert!(started.online);
        assert_eq!(
            api.orchestrator_attention_fingerprint
                .lock()
                .unwrap()
                .as_deref(),
            Some(ticket_ref.id.as_str())
        );
        *api.orchestrator_attention_fingerprint.lock().unwrap() = None;
        let worker_id = started.worker.as_ref().unwrap().worker.worker_id.clone();
        maybe_dispatch_orchestrator_turn_end(
            &api,
            &worker_id,
            Some(protocol::subscription::SubscriptionWorkerState::Idle),
            protocol::subscription::SubscriptionWorkerState::Idle,
        );
        assert!(
            api.orchestrator_attention_fingerprint
                .lock()
                .unwrap()
                .is_none()
        );
        maybe_dispatch_orchestrator_turn_end(
            &api,
            &worker_id,
            Some(protocol::subscription::SubscriptionWorkerState::Running),
            protocol::subscription::SubscriptionWorkerState::Idle,
        );

        assert_eq!(
            api.orchestrator_attention_fingerprint
                .lock()
                .unwrap()
                .as_deref(),
            Some(ticket_ref.id.as_str())
        );
    }

    #[tokio::test]
    async fn worker_spawn_and_restore_assignment_operations_are_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let api = test_api(dir.path()).await;
        let backend = browser_ticket_backend(&api).unwrap();
        let mut first_ticket_input = ticket::NewTicket::new("Spawn assignment");
        first_ticket_input.workflow_state = Some(TicketWorkflowState::Queued);
        let first_ticket = backend.create(first_ticket_input).unwrap();
        assign_test_orchestrator(&api, &first_ticket.id);
        let request = WorkerSpawnRequest {
            requested_worker_name: Some("assigned-spawn".to_string()),
            intent: WorkerSpawnIntent::TicketRole {
                ticket_id: first_ticket.id.clone(),
                role: TicketWorkerRole::Coder,
            },
            acceptance: WorkerSpawnAcceptanceRequirement::RunAccepted {
                expected_segments: 1,
            },
            profile: ProfileSelector::Builtin("builtin:coder".to_string()),
            ticket_assignment: Some(crate::hosts::WorkerTicketAssignmentRequest {
                ticket_id: first_ticket.id.clone(),
                operation_id: "spawn-assignment-operation".to_string(),
            }),
            initial_submit: vec![Segment::Flow {
                selector: "builtin:coder-review".to_string(),
            }],
            working_directory_request: None,
            resolved_working_directory_request: None,
            resolved_working_directory: None,
            resolved_config_bundle: None,
            resolved_worker_observation_enabled: false,
            resolved_worker_observation_grants: Vec::new(),
            resolved_control_operation: None,
            resolved_workspace_api: None,
            resolved_memory_settings: None,
        };
        let Json(first) = scoped_create_runtime_worker(
            State(api.clone()),
            AxumPath(ScopedRuntimePath {
                workspace_id: TEST_WORKSPACE_ID.to_string(),
                runtime_id: EMBEDDED_WORKER_RUNTIME_ID.to_string(),
            }),
            Json(request.clone()),
        )
        .await
        .unwrap();
        let first_worker = first.worker.unwrap();
        let Json(projected) = scoped_list_ticket_assignments(
            State(api.clone()),
            AxumPath(ScopedRecordPath {
                workspace_id: TEST_WORKSPACE_ID.to_string(),
                id: first_ticket.id.clone(),
            }),
        )
        .await
        .unwrap();
        assert_eq!(
            projected.assignments[0]
                .principal
                .worker()
                .map(|worker| worker.worker_id),
            Some(first_worker.worker.worker_id.clone())
        );
        assert_eq!(
            backend
                .show(first_ticket.id.clone().into())
                .unwrap()
                .meta
                .workflow_state,
            TicketWorkflowState::InProgress
        );
        let Json(retried) = scoped_create_runtime_worker(
            State(api.clone()),
            AxumPath(ScopedRuntimePath {
                workspace_id: TEST_WORKSPACE_ID.to_string(),
                runtime_id: EMBEDDED_WORKER_RUNTIME_ID.to_string(),
            }),
            Json(request.clone()),
        )
        .await
        .unwrap();
        assert_eq!(
            retried.worker.unwrap().worker.worker_id,
            first_worker.worker.worker_id
        );
        assert_eq!(
            api.store
                .list_ticket_coder_assignment_events(TEST_WORKSPACE_ID, &first_ticket.id, 10,)
                .unwrap()
                .len(),
            1
        );

        let current = api
            .store
            .get_current_ticket_coder_assignment(TEST_WORKSPACE_ID, &first_ticket.id)
            .unwrap()
            .unwrap();
        api.store
            .clear_current_ticket_worker_assignment(
                TEST_WORKSPACE_ID,
                &first_ticket.id,
                Some(&current.assignment_id),
                "spawn-unassign-operation",
                "spawn-unassign-event",
                "test-user",
                TEST_CREATED_AT,
            )
            .unwrap();
        api.runtime
            .stop_worker(
                &first_worker.worker,
                WorkerLifecycleRequest {
                    reason: Some("restore assignment test".to_string()),
                    ticket_assignment: None,
                },
            )
            .unwrap();
        let mut second_ticket_input = ticket::NewTicket::new("Restore assignment");
        second_ticket_input.workflow_state = Some(TicketWorkflowState::Queued);
        let second_ticket = backend.create(second_ticket_input).unwrap();
        assign_test_orchestrator(&api, &second_ticket.id);
        let _ = scoped_restore_runtime_worker(
            State(api.clone()),
            AxumPath(ScopedRuntimeWorkerPath {
                workspace_id: TEST_WORKSPACE_ID.to_string(),
                worker: RuntimeWorkerRef::new(
                    EMBEDDED_WORKER_RUNTIME_ID,
                    first_worker.worker.worker_id.clone(),
                ),
            }),
            Query(RestoreTicketAssignmentQuery {
                ticket_id: Some(second_ticket.id.clone()),
                assignment_operation_id: Some("restore-assignment-operation".to_string()),
            }),
        )
        .await
        .unwrap();
        let Json(retried_restore) = scoped_restore_runtime_worker(
            State(api.clone()),
            AxumPath(ScopedRuntimeWorkerPath {
                workspace_id: TEST_WORKSPACE_ID.to_string(),
                worker: RuntimeWorkerRef::new(
                    EMBEDDED_WORKER_RUNTIME_ID,
                    first_worker.worker.worker_id.clone(),
                ),
            }),
            Query(RestoreTicketAssignmentQuery {
                ticket_id: Some(second_ticket.id.clone()),
                assignment_operation_id: Some("restore-assignment-operation".to_string()),
            }),
        )
        .await
        .unwrap();
        assert_eq!(retried_restore.worker_id, first_worker.worker.worker_id);
        assert_eq!(
            retried_restore.result.state,
            workspace_api::WorkerOperationState::Accepted
        );
        let restored_assignment = api
            .store
            .get_current_ticket_coder_assignment(TEST_WORKSPACE_ID, &second_ticket.id)
            .unwrap()
            .unwrap();
        assert_eq!(
            restored_assignment.worker.worker_id,
            first_worker.worker.worker_id
        );
        assert_eq!(
            backend
                .show(second_ticket.id.clone().into())
                .unwrap()
                .meta
                .workflow_state,
            TicketWorkflowState::InProgress
        );

        api.store
            .clear_current_ticket_worker_assignment(
                TEST_WORKSPACE_ID,
                &second_ticket.id,
                Some(&restored_assignment.assignment_id),
                "restore-clear-operation",
                "restore-clear-event",
                "test",
                TEST_CREATED_AT,
            )
            .unwrap();
        let mut pending_request = WorkerSpawnRequest {
            intent: WorkerSpawnIntent::TicketRole {
                ticket_id: second_ticket.id.clone(),
                role: TicketWorkerRole::Coder,
            },
            ticket_assignment: Some(crate::hosts::WorkerTicketAssignmentRequest {
                ticket_id: second_ticket.id.clone(),
                operation_id: "pending-spawn-operation".to_string(),
            }),
            resolved_control_operation: None,
            ..request
        };
        pending_request.resolved_workspace_api =
            Some(test_worker_workspace_api(EMBEDDED_WORKER_RUNTIME_ID));
        let (_, pending_fingerprint) = crate::hosts::worker_spawn_idempotency(&pending_request)
            .unwrap()
            .unwrap();
        api.store
            .reserve_ticket_assignment_operation(
                TEST_WORKSPACE_ID,
                "pending-spawn-operation",
                &second_ticket.id,
                EMBEDDED_WORKER_RUNTIME_ID,
                None,
                &pending_fingerprint,
                TEST_CREATED_AT,
            )
            .unwrap();
        let current_memory_settings = api
            .config_store
            .get_workspace_memory_settings(TEST_WORKSPACE_ID)
            .unwrap();
        let reservation = api
            .config_store
            .reserve_worker_create(
                TEST_WORKSPACE_ID,
                EMBEDDED_WORKER_RUNTIME_ID,
                "pending-spawn-operation",
                &pending_fingerprint,
                &current_memory_settings,
            )
            .unwrap();
        pending_request.resolved_memory_settings = Some(reservation.memory_settings.clone());
        let spawned_before_backend_failure = api
            .runtime
            .spawn_worker(
                EMBEDDED_WORKER_RUNTIME_ID,
                WorkerCreateBinding {
                    worker_id: reservation.worker_id,
                    create_fingerprint: reservation.create_fingerprint,
                },
                pending_request.clone(),
            )
            .unwrap()
            .worker
            .unwrap();
        assert!(
            api.store
                .get_ticket_assignment_operation(TEST_WORKSPACE_ID, "pending-spawn-operation")
                .unwrap()
                .is_some_and(|operation| operation.worker.is_none())
        );
        let worker_count_before_retry = api.runtime.list_workers(100).items.len();
        let Json(reconciled) = scoped_create_runtime_worker(
            State(api.clone()),
            AxumPath(ScopedRuntimePath {
                workspace_id: TEST_WORKSPACE_ID.to_string(),
                runtime_id: EMBEDDED_WORKER_RUNTIME_ID.to_string(),
            }),
            Json(pending_request),
        )
        .await
        .unwrap();
        assert_eq!(
            reconciled.worker.unwrap().worker.worker_id,
            spawned_before_backend_failure.worker.worker_id
        );
        assert_eq!(
            api.runtime.list_workers(100).items.len(),
            worker_count_before_retry,
            "retrying a reserved lifecycle operation must not spawn another Worker"
        );
        assert!(
            api.store
                .get_ticket_assignment_operation(TEST_WORKSPACE_ID, "pending-spawn-operation")
                .unwrap()
                .and_then(|operation| operation.assignment_id)
                .is_some()
        );
    }

    #[tokio::test]
    async fn worker_spawn_finalize_failure_reports_stage_and_compensation_outcomes() {
        let dir = tempfile::tempdir().unwrap();
        let api = test_api(dir.path()).await;
        let mut ticket_input = ticket::NewTicket::new("Compensation test Ticket");
        ticket_input.workflow_state = Some(TicketWorkflowState::InProgress);
        let ticket_id = browser_ticket_backend(&api)
            .unwrap()
            .create(ticket_input)
            .unwrap()
            .id;
        assign_test_orchestrator(&api, &ticket_id);
        let assignment = crate::hosts::WorkerTicketAssignmentRequest {
            ticket_id: ticket_id.clone(),
            operation_id: "compensation-test-operation".to_string(),
        };
        let request = WorkerSpawnRequest {
            intent: WorkerSpawnIntent::TicketRole {
                ticket_id: ticket_id.clone(),
                role: TicketWorkerRole::Coder,
            },
            requested_worker_name: Some("Compensation test Worker".to_string()),
            acceptance: WorkerSpawnAcceptanceRequirement::RunAccepted {
                expected_segments: 1,
            },
            profile: ProfileSelector::Builtin("builtin:coder".to_string()),
            ticket_assignment: Some(assignment.clone()),
            initial_submit: vec![Segment::Flow {
                selector: "builtin:coder-review".to_string(),
            }],
            working_directory_request: None,
            resolved_working_directory_request: None,
            resolved_working_directory: None,
            resolved_config_bundle: None,
            resolved_worker_observation_enabled: false,
            resolved_worker_observation_grants: Vec::new(),
            resolved_control_operation: None,
            resolved_workspace_api: None,
            resolved_memory_settings: None,
        };
        let Json(created) = scoped_create_runtime_worker(
            State(api.clone()),
            AxumPath(ScopedRuntimePath {
                workspace_id: TEST_WORKSPACE_ID.to_string(),
                runtime_id: EMBEDDED_WORKER_RUNTIME_ID.to_string(),
            }),
            Json(request),
        )
        .await
        .unwrap();
        let worker = created.worker.unwrap();
        assert!(
            api.store
                .get_worker_registry(TEST_WORKSPACE_ID, &worker.worker)
                .unwrap()
                .is_some()
        );

        let context = WorkerSpawnCompensationContext {
            assignment: Some(&assignment),
            prepared_workdir_id: None,
            cleanup_spawned_workdir: false,
        };
        let error = finalize_worker_spawn_stage::<()>(
            &api,
            &worker,
            &context,
            WorkerSpawnFinalizeStage::WorkdirAttachment,
            Err(Error::RegistryInconsistency("injected finalize failure".to_string()).into()),
        )
        .unwrap_err();
        assert!(matches!(
            error.error,
            Error::RuntimeOperationFailed { ref code, .. }
                if code == "worker_spawn_finalize_workdir_attachment_failed"
        ));
        assert!(error.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "worker_spawn_finalize_workdir_attachment_failed"
                && diagnostic.message.contains(&worker.worker.runtime_id)
                && diagnostic.message.contains(&worker.worker.worker_id)
        }));
        assert!(
            error
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "worker_spawn_compensated")
        );
        assert!(matches!(
            api.runtime.worker(&worker.worker),
            Err(RuntimeRegistryError::UnknownWorker { .. })
        ));
        assert!(
            api.store
                .get_worker_registry(TEST_WORKSPACE_ID, &worker.worker)
                .unwrap()
                .is_none()
        );
        assert!(
            api.store
                .get_ticket_assignment_operation(TEST_WORKSPACE_ID, &assignment.operation_id)
                .unwrap()
                .is_none()
        );
        assert!(
            api.store
                .get_current_ticket_coder_assignment(TEST_WORKSPACE_ID, &ticket_id)
                .unwrap()
                .is_none()
        );

        let mut residual_worker = worker.clone();
        residual_worker.worker.runtime_id = "missing-runtime".to_string();
        let residual_context = WorkerSpawnCompensationContext {
            assignment: None,
            prepared_workdir_id: None,
            cleanup_spawned_workdir: false,
        };
        let residual_error = finalize_worker_spawn_stage::<()>(
            &api,
            &residual_worker,
            &residual_context,
            WorkerSpawnFinalizeStage::WorkerRegistry,
            Err(Error::RegistryInconsistency("injected registry failure".to_string()).into()),
        )
        .unwrap_err();
        assert!(residual_error.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "worker_spawn_compensation_runtime_delete_failed"
                && diagnostic.message.contains("missing-runtime")
        }));
        assert!(
            !residual_error
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "worker_spawn_compensated")
        );
        assert!(residual_error.error.to_string().contains("residual state"));
    }

    #[tokio::test]
    async fn ticket_browser_endpoints_mutate_typed_backend_and_return_thread() {
        let dir = tempfile::tempdir().unwrap();
        init_clean_git_workspace(dir.path());
        fs::create_dir_all(dir.path().join(".yoi")).unwrap();
        fs::write(
            dir.path().join(".yoi/workspace.toml"),
            "this is not valid workspace config",
        )
        .unwrap();
        let api = test_api(dir.path()).await;
        let ticket_ref = browser_ticket_backend(&api)
            .unwrap()
            .create(ticket::NewTicket::new("Browser Ticket API"))
            .unwrap();
        let ticket_resource_key = ticket_ref.resource_key.clone().unwrap();
        let ticket_id = ticket_ref.id;
        assert_eq!(
            resolve_workspace_ticket_reference(&api, TEST_WORKSPACE_ID, &ticket_resource_key)
                .unwrap(),
            ticket_id
        );
        let path = || ScopedRecordPath {
            workspace_id: TEST_WORKSPACE_ID.to_string(),
            id: ticket_id.clone(),
        };
        let related_ticket_id = browser_ticket_backend(&api)
            .unwrap()
            .create(ticket::NewTicket::new("Related Browser Ticket"))
            .unwrap()
            .id;
        browser_ticket_backend(&api)
            .unwrap()
            .add_ticket_relation(
                ticket_id.clone().into(),
                ticket::NewTicketRelation {
                    kind: ticket::TicketRelationKind::Related,
                    target: related_ticket_id.clone(),
                    note: Some("Browser relation".to_string()),
                    author: Some("browser-user".to_string()),
                },
            )
            .unwrap();

        let Json(edited) = scoped_edit_ticket_item(
            State(api.clone()),
            AxumPath(path()),
            Json(BrowserEditTicketRequest {
                title: Some("Browser Ticket API edited".to_string()),
                body: Some("Updated from the Browser API.".to_string()),
                old_string: None,
                new_string: None,
                replace_all: false,
                target: Some(TicketTargetEdit::Set {
                    repository_id: "main".to_string(),
                    ref_selector: Some("develop".to_string()),
                }),
                author: Some("browser-user".to_string()),
            }),
        )
        .await
        .unwrap();
        assert_eq!(edited.title, "Browser Ticket API edited");
        assert_eq!(edited.body, "Updated from the Browser API.");
        assert_eq!(edited.repository_id.as_deref(), Some("main"));
        assert_eq!(edited.ref_selector.as_deref(), Some("develop"));
        assert!(edited.assignments.is_empty());
        assert!(edited.assignment_diagnostics.is_empty());
        assert_eq!(edited.relations.outgoing.len(), 1);
        assert_eq!(edited.relations.outgoing[0].target, related_ticket_id);
        assert_eq!(edited.relations.outgoing[0].kind, "related");

        let Json(commented) = scoped_append_ticket_event(
            State(api.clone()),
            AxumPath(path()),
            Json(BrowserAppendTicketEventRequest {
                role: BrowserTicketThreadRole::Comment,
                body: "API comment".to_string(),
                author: Some("browser-user".to_string()),
            }),
        )
        .await
        .unwrap();
        assert!(commented.events.iter().any(|event| {
            event.kind == "comment" && event.body.as_deref() == Some("API comment")
        }));

        let Json(ready) = scoped_mark_ticket_ready_from_browser(
            State(api.clone()),
            AxumPath(path()),
            Json(TicketMarkReadyRequest {
                operation_key: "browser-ready".to_owned(),
                reason: Some("intake complete".to_owned()),
                intake_summary: None,
            }),
        )
        .await
        .unwrap();
        assert_eq!(ready.state, "ready");
        assign_test_orchestrator(&api, &ticket_id);

        let Json(queued) = scoped_queue_ticket(
            State(api.clone()),
            AxumPath(path()),
            Json(BrowserQueueTicketRequest {}),
        )
        .await
        .unwrap();
        assert_eq!(queued.state, "queued");
        assert_eq!(queued.queued_by.as_deref(), Some("workspace-web"));
        let Json(closed) = scoped_close_ticket(
            State(api),
            AxumPath(path()),
            Json(BrowserCloseTicketRequest {
                resolution: "Closed through the Browser API.".to_string(),
            }),
        )
        .await
        .unwrap();
        assert_eq!(closed.state, "closed");
        assert_eq!(
            closed.resolution.as_deref(),
            Some("Closed through the Browser API.")
        );
    }

    #[tokio::test]
    async fn ticket_rest_operations_use_workspace_sqlite_backend() {
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

        let ticket_ref = browser_ticket_backend(&api)
            .unwrap()
            .create(ticket::NewTicket::new("Endpoint configured root"))
            .unwrap();
        assert!(api.config.database_path.is_file());
        assert!(
            !dir.path()
                .join("server-tickets")
                .join(&ticket_ref.id)
                .join("item.md")
                .exists()
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
    async fn skills_endpoints_use_active_virtual_config_and_ignore_repository_yoi() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join(".yoi/skills/triage-errors");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: triage-errors\ndescription: stale filesystem authority\n---\nfilesystem body",
        )
        .unwrap();
        let api = test_api(dir.path()).await;
        let current = api
            .config_store
            .load_workspace_config(TEST_WORKSPACE_ID)
            .unwrap()
            .unwrap();
        let main_path = config_source::VirtualPath::parse("main.dcdl").unwrap();
        let skill_path =
            config_source::VirtualPath::parse("skills/triage-errors/SKILL.md").unwrap();
        let main = format!(
            r#"{{ skills = {{ triage_errors = import "./skills/triage-errors/SKILL.md" as {}; }}; }}"#,
            skills::SKILL_DOCUMENT_SCHEMA_SOURCE
        );
        let request = crate::config_source::ConfigCommitRequest {
            base_revision: current.snapshot.revision,
            base_digest: current.snapshot.digest.clone(),
            changes: vec![
                config_source::ConfigTreeChange::Update {
                    path: main_path.clone(),
                    expected_digest: current.snapshot.entries[&main_path]
                        .content_digest
                        .clone(),
                    content: main,
                },
                config_source::ConfigTreeChange::Create {
                    path: skill_path,
                    content_type: config_source::ConfigContentType::Text,
                    content: "---\nname: triage-errors\ndescription: Use the active DB-backed virtual config when triaging errors.\n---\n# Triage Errors\n\nInspect logs before changing code."
                        .to_string(),
                },
            ],
            entrypoints: current.contract.entrypoints.clone(),
        };
        let candidate = api
            .config_store
            .evaluate_workspace_config_candidate_with_schema(
                TEST_WORKSPACE_ID,
                &request,
                api.config_schema_registry.compose().unwrap(),
            )
            .unwrap();
        api.config_store
            .commit_evaluated_workspace_config(TEST_WORKSPACE_ID, &candidate)
            .unwrap();

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
        assert_eq!(
            entry.provenance.virtual_path.as_deref(),
            Some("skills/triage-errors/SKILL.md")
        );
        assert!(entry.provenance.revision.is_some());
        assert!(entry.provenance.source_digest.is_some());
        assert_ne!(entry.description, "stale filesystem authority");
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

    async fn test_api_with_recording_backend(
        workspace_root: impl Into<PathBuf>,
    ) -> (WorkspaceApi, Arc<DeterministicExecutionBackend>) {
        let config = test_server_config(workspace_root);
        let store = SqliteWorkspaceStore::open(config.database_path.clone()).unwrap();
        let execution = Arc::new(DeterministicExecutionBackend::default());
        let api =
            WorkspaceApi::new_with_execution_backend(config, Arc::new(store), execution.clone())
                .await
                .unwrap();
        (api, execution)
    }

    async fn test_api(workspace_root: impl Into<PathBuf>) -> WorkspaceApi {
        test_api_with_recording_backend(workspace_root).await.0
    }

    fn set_test_default_runtime(api: &WorkspaceApi, runtime_id: &str) {
        let current = api
            .config_store
            .load_workspace_config(TEST_WORKSPACE_ID)
            .unwrap()
            .unwrap();
        let main_path = config_source::VirtualPath::parse("main.dcdl").unwrap();
        let request = crate::config_source::ConfigCommitRequest {
            base_revision: current.snapshot.revision,
            base_digest: current.snapshot.digest.clone(),
            changes: vec![config_source::ConfigTreeChange::Update {
                path: main_path.clone(),
                expected_digest: current.snapshot.entries[&main_path].content_digest.clone(),
                content: format!(
                    "{{ runtime = {{ default_runtime_id = {runtime_id:?}; }}; }} as WorkspaceConfigSchema"
                ),
            }],
            entrypoints: current.contract.entrypoints.clone(),
        };
        let candidate = api
            .config_store
            .evaluate_workspace_config_candidate_with_schema(
                TEST_WORKSPACE_ID,
                &request,
                api.config_schema_registry.compose().unwrap(),
            )
            .unwrap();
        api.config_store
            .commit_evaluated_workspace_config(TEST_WORKSPACE_ID, &candidate)
            .unwrap();
    }

    fn assign_test_orchestrator(api: &WorkspaceApi, ticket_id: &str) {
        api.store
            .set_current_ticket_role_assignment(
                &TicketRoleAssignmentRecord {
                    workspace_id: TEST_WORKSPACE_ID.to_string(),
                    ticket_id: ticket_id.to_string(),
                    assignment_id: format!("orchestrator-{ticket_id}"),
                    role: TicketAssignmentRole::Orchestrator,
                    principal: TicketAssignmentPrincipal::WorkspaceAgent {
                        agent_key: "workspace-orchestrator".to_string(),
                    },
                    assigned_by: "test-user".to_string(),
                    assigned_at: "2026-09-01T00:00:00Z".to_string(),
                },
                None,
                &format!("orchestrator-event-{ticket_id}"),
                &format!("orchestrator-op-{ticket_id}"),
                false,
            )
            .unwrap();
        if let Ok(ticket) = api.authority.ticket(ticket_id)
            && matches!(ticket.state.as_str(), "queued" | "inprogress")
        {
            let assignment_id = format!("orchestrator-{ticket_id}");
            let backend =
                browser_ticket_backend(api)
                    .unwrap()
                    .with_event_attributes(BTreeMap::from([(
                        "orchestrator_assignment_id".to_string(),
                        assignment_id,
                    )]));
            backend
                .add_event(
                    TicketIdOrSlug::Id(ticket.id),
                    ticket::NewTicketEvent::new(
                        ticket::TicketEventKind::Comment,
                        "test Queue assignment fence",
                    ),
                )
                .unwrap();
        }
    }

    #[tokio::test]
    async fn memory_settings_handlers_reject_foreign_workspace_path_scope() {
        let temp = tempfile::tempdir().unwrap();
        let api = test_api(temp.path()).await;
        assert!(
            scoped_get_workspace_memory_settings(
                State(api.clone()),
                AxumPath("workspace-foreign".to_string()),
            )
            .await
            .is_err()
        );
        assert!(
            scoped_update_workspace_memory_settings(
                State(api),
                AxumPath("workspace-foreign".to_string()),
                Json(workspace_api::UpdateWorkspaceMemorySettingsRequest {
                    expected_revision: 1,
                    language: "English".to_string(),
                }),
            )
            .await
            .is_err()
        );
    }

    #[tokio::test]
    async fn destructive_worker_remove_rejects_browser_and_legacy_source_headers() {
        let headers = HeaderMap::new();
        assert!(matches!(
            crate::worker_source::presented_worker_remove_source(&headers, None),
            Err(crate::worker_source::WorkerMutationSourceProofError::Missing)
        ));

        let mut spoofed = HeaderMap::new();
        spoofed.insert("x-yoi-runtime-id", "runtime-spoofed".parse().unwrap());
        spoofed.insert("x-yoi-worker-id", "worker-spoofed".parse().unwrap());
        assert!(matches!(
            crate::worker_source::presented_worker_remove_source(&spoofed, None),
            Err(crate::worker_source::WorkerMutationSourceProofError::Missing)
        ));

        let temp = tempfile::tempdir().unwrap();
        let app = build_inner_router(test_api(temp.path()).await);
        let body = r#"{"target_runtime_id":"runtime-target","target_worker_id":"target-worker","reason":"retire target Worker"}"#;
        let browser = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/w/{TEST_WORKSPACE_ID}/workers/remove"))
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(browser.status(), StatusCode::UNAUTHORIZED);
        let legacy = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/w/{TEST_WORKSPACE_ID}/workers/remove"))
                    .header(CONTENT_TYPE, "application/json")
                    .header("x-yoi-runtime-id", "runtime-spoofed")
                    .header("x-yoi-worker-id", "worker-spoofed")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(legacy.status(), StatusCode::UNAUTHORIZED);
        let body_spoof = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/w/{TEST_WORKSPACE_ID}/workers/remove"))
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"target_runtime_id":"runtime-target","target_worker_id":"target-worker","expected_worker_revision":"revision-1","reason":"retire target Worker","source_proof":"browser-controlled","actor":"orchestrator","policy":"purge"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(body_spoof.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn embedded_worker_remove_proof_derives_source_and_rejects_replay_and_wrong_target() {
        let temp = tempfile::tempdir().unwrap();
        let api = test_api(temp.path()).await;
        seed_worker_source_member(&api, EMBEDDED_RUNTIME_ID, "7");
        let scope = worker_runtime::RuntimeWorkspaceScope::new(
            api.config.workspace_id.clone(),
            "server-unused-for-embedded",
        );
        let authority = RuntimeWorkerMutationSourceAuthority::embedded(
            EMBEDDED_RUNTIME_ID,
            &api.config.workspace_id,
        );
        let RuntimeOwnedWorkerMutationProof::InProcess(proof) = authority
            .issue_worker_remove(&scope, "7", "runtime-target", "target-worker")
            .unwrap()
        else {
            panic!("embedded Runtime must produce in-process claims");
        };

        let wrong_target = crate::worker_source::verify_worker_remove_source(
            &api,
            crate::worker_source::PresentedWorkerMutationSourceProof::InProcess(proof.clone()),
            "runtime-target",
            "different-worker",
        )
        .await;
        assert!(matches!(
            wrong_target,
            Err(crate::worker_source::WorkerMutationSourceProofError::Invalid)
        ));

        let verified = crate::worker_source::verify_worker_remove_source(
            &api,
            crate::worker_source::PresentedWorkerMutationSourceProof::InProcess(proof.clone()),
            "runtime-target",
            "target-worker",
        )
        .await
        .unwrap();
        assert_eq!(verified.runtime_id, EMBEDDED_RUNTIME_ID);
        assert_eq!(verified.worker_id, "7");
        assert_eq!(verified.permission, WORKER_REMOVE_PERMISSION);

        let replay = crate::worker_source::verify_worker_remove_source(
            &api,
            crate::worker_source::PresentedWorkerMutationSourceProof::InProcess(proof),
            "runtime-target",
            "target-worker",
        )
        .await;
        assert!(matches!(
            replay,
            Err(crate::worker_source::WorkerMutationSourceProofError::Replay)
        ));

        let RuntimeOwnedWorkerMutationProof::InProcess(fresh_proof) = authority
            .issue_worker_remove(&scope, "7", "runtime-target", "target-worker")
            .unwrap()
        else {
            panic!("embedded Runtime must produce in-process claims");
        };
        let dispatcher = crate::worker_source::EmbeddedServerWorkerMutationDispatcher::new(
            api.config.clone(),
            api.store.clone(),
        );
        let error =
            worker_runtime::worker_source::EmbeddedWorkerMutationDispatcher::execute_worker_remove(
                &dispatcher,
                fresh_proof,
                "runtime-target",
                "target-worker",
                "retire target Worker",
            )
            .unwrap_err();
        assert!(error.to_string().contains("executor is unavailable"));
    }

    #[tokio::test]
    async fn worker_remove_rejects_self_and_running_at_caller_boundary() {
        let temp = tempfile::tempdir().unwrap();
        let api = test_api(temp.path()).await;
        let Json(orchestrator) = scoped_start_workspace_orchestrator(
            State(api.clone()),
            AxumPath(ScopedWorkspacePath {
                workspace_id: TEST_WORKSPACE_ID.to_string(),
            }),
        )
        .await
        .unwrap();
        let source = orchestrator.worker.unwrap().worker;
        let verified_source = || crate::worker_source::VerifiedWorkerMutationSource {
            runtime_id: source.runtime_id.clone(),
            worker_id: source.worker_id.clone(),
            actor_kind: worker_runtime::auth::WorkerMutationActorKind::Worker,
            permission: worker_runtime::auth::WORKER_REMOVE_PERMISSION.to_string(),
            jti: "caller-guard-proof".to_string(),
        };
        let executor = WorkspaceWorkerRemoveExecutor::new(&api);
        let self_response = executor
            .execute_async(
                verified_source(),
                &source.runtime_id,
                &source.worker_id,
                "must reject self",
            )
            .await
            .unwrap();
        assert_eq!(self_response.status, StatusCode::CONFLICT.as_u16());
        assert!(self_response.body.contains("self_removal_forbidden"));

        let spawned = api
            .spawn_workspace_worker(
                EMBEDDED_WORKER_RUNTIME_ID,
                WorkerSpawnRequest {
                    intent: WorkerSpawnIntent::WorkspaceCompanion,
                    requested_worker_name: Some("guard-target".to_string()),
                    acceptance: WorkerSpawnAcceptanceRequirement::RunAccepted {
                        expected_segments: 0,
                    },
                    profile: worker_runtime::catalog::ProfileSelector::Builtin(
                        "builtin:companion".to_string(),
                    ),
                    ticket_assignment: None,
                    initial_submit: Vec::new(),
                    working_directory_request: None,
                    resolved_working_directory_request: None,
                    resolved_working_directory: None,
                    resolved_config_bundle: Some(runtime_test_bundle()),
                    resolved_worker_observation_enabled: false,
                    resolved_worker_observation_grants: Vec::new(),
                    resolved_workspace_api: None,
                    resolved_memory_settings: None,
                    resolved_control_operation: None,
                },
            )
            .unwrap();
        let target = spawned.worker.unwrap().worker;
        let target_summary = api.runtime.worker(&target).unwrap();
        sync_worker_observation(&api, &target_summary).unwrap();
        seed_worker_control_grant(&api, &source, &target, "caller-guard-target");
        let running_response = executor
            .execute_async(
                verified_source(),
                &target.runtime_id,
                &target.worker_id,
                "must reject a live Worker",
            )
            .await
            .unwrap();
        assert_eq!(running_response.status, StatusCode::CONFLICT.as_u16());
        assert!(running_response.body.contains("worker_not_stopped"));
    }

    #[tokio::test]
    async fn embedded_worker_remove_executes_retention_and_returns_bounded_result() {
        let temp = tempfile::tempdir().unwrap();
        let api = test_api(temp.path()).await;
        let Json(orchestrator) = scoped_start_workspace_orchestrator(
            State(api.clone()),
            AxumPath(ScopedWorkspacePath {
                workspace_id: TEST_WORKSPACE_ID.to_string(),
            }),
        )
        .await
        .unwrap();
        let source = orchestrator.worker.unwrap().worker;

        let spawned = api
            .spawn_workspace_worker(
                EMBEDDED_WORKER_RUNTIME_ID,
                WorkerSpawnRequest {
                    intent: WorkerSpawnIntent::WorkspaceCompanion,
                    requested_worker_name: Some("remove-target".to_string()),
                    acceptance: WorkerSpawnAcceptanceRequirement::RunAccepted {
                        expected_segments: 0,
                    },
                    profile: worker_runtime::catalog::ProfileSelector::Builtin(
                        "builtin:companion".to_string(),
                    ),
                    ticket_assignment: None,
                    initial_submit: Vec::new(),
                    working_directory_request: None,
                    resolved_working_directory_request: None,
                    resolved_working_directory: None,
                    resolved_config_bundle: Some(runtime_test_bundle()),
                    resolved_worker_observation_enabled: false,
                    resolved_worker_observation_grants: Vec::new(),
                    resolved_workspace_api: None,
                    resolved_memory_settings: None,
                    resolved_control_operation: None,
                },
            )
            .unwrap();
        let target = spawned.worker.unwrap().worker;
        let stopped = api
            .runtime
            .stop_worker(
                &target,
                WorkerLifecycleRequest {
                    reason: Some("prepare WorkerRemove regression".to_string()),
                    ticket_assignment: None,
                },
            )
            .unwrap();
        assert_eq!(stopped.state, WorkerOperationState::Accepted);
        let worker_root = temp
            .path()
            .join(".test-embedded-runtime-store/workers")
            .join(&target.worker_id);
        fs::create_dir_all(worker_root.join("session/segments")).unwrap();
        fs::write(
            worker_root.join("session/session.json"),
            serde_json::to_vec_pretty(&json!({
                "schema_version": 1,
                "session_id": "worker-remove-session"
            }))
            .unwrap(),
        )
        .unwrap();
        fs::write(
            worker_root.join("session/segments/segment-a.jsonl"),
            b"retained evidence\n",
        )
        .unwrap();
        let summary = api.runtime.worker(&target).unwrap();
        sync_worker_observation(&api, &summary).unwrap();
        seed_worker_control_grant(&api, &source, &target, "embedded-valid-proof");

        let response = WorkspaceWorkerRemoveExecutor::new(&api)
            .execute_async(
                crate::worker_source::VerifiedWorkerMutationSource {
                    runtime_id: source.runtime_id,
                    worker_id: source.worker_id,
                    actor_kind: worker_runtime::auth::WorkerMutationActorKind::Worker,
                    permission: worker_runtime::auth::WORKER_REMOVE_PERMISSION.to_string(),
                    jti: "embedded-valid-proof".to_string(),
                },
                &target.runtime_id,
                &target.worker_id,
                "retire completed Worker",
            )
            .await
            .unwrap();
        assert_eq!(
            response.status,
            StatusCode::OK.as_u16(),
            "{}",
            response.body
        );
        assert!(response.body.contains("\"removed\":true"));
        assert!(!response.body.contains("disposition"));
        assert!(!response.body.contains("stage"));
        assert!(!response.body.contains("path"));
        assert!(
            api.store
                .get_worker_registry(TEST_WORKSPACE_ID, &target)
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn remote_worker_remove_proof_requires_current_runtime_trust_scope_and_catalog_member() {
        let temp = tempfile::tempdir().unwrap();
        let identity = RuntimeIdentityMaterial::generate("runtime-remote").unwrap();
        let mut config = test_server_config(temp.path());
        config.remote_runtime_sources.push(RemoteRuntimeConfig {
            runtime_id: "runtime-remote".to_string(),
            workspace_id: Some(TEST_WORKSPACE_ID.to_string()),
            display_name: "Remote Runtime".to_string(),
            base_url: "https://runtime.invalid".to_string(),
            bearer_token: None,
            auth: Some(RemoteRuntimeAuthConfig {
                server_id: "server-main".to_string(),
                server_private_key: identity.private_key.clone(),
            }),
            cached_capabilities: RuntimeCapabilitySummary {
                can_list_hosts: true,
                can_list_workers: true,
                can_get_worker: true,
                can_spawn_worker: true,
                can_stop_worker: true,
                has_workspace_fs: false,
                has_shell: false,
                has_git: false,
                supports_worktrees: false,
                supports_backend_internal_tools: false,
                workspace_scope: TEST_WORKSPACE_ID.to_string(),
                max_workers: 1,
                os: "test".to_string(),
                arch: "test".to_string(),
            },
            cached_status: "connected".to_string(),
            timeout: std::time::Duration::from_secs(1),
        });
        let store = SqliteWorkspaceStore::open(config.database_path.clone()).unwrap();
        store
            .upsert_workspace(&WorkspaceRecord {
                workspace_id: TEST_WORKSPACE_ID.to_string(),
                owner_account_id: None,
                display_name: "Test Workspace".to_string(),
                state: "active".to_string(),
                created_at: "2026-08-11T00:00:00Z".to_string(),
                updated_at: "2026-08-11T00:00:00Z".to_string(),
            })
            .await
            .unwrap();
        let trust = crate::store::TrustedRuntimeRecord {
            runtime_id: "runtime-remote".to_string(),
            workspace_id: Some(TEST_WORKSPACE_ID.to_string()),
            display_name: "Remote Runtime".to_string(),
            base_url: "https://runtime.invalid".to_string(),
            public_key: identity.public_key.clone(),
            created_at: "2026-08-11T00:00:00Z".to_string(),
            updated_at: "2026-08-11T00:00:00Z".to_string(),
            revoked_at: None,
        };
        store.upsert_trusted_runtime(&trust).unwrap();
        let api = WorkspaceApi::new_with_execution_backend(
            config,
            Arc::new(store),
            Arc::new(DeterministicExecutionBackend::default()),
        )
        .await
        .unwrap();
        seed_worker_source_member(&api, "runtime-remote", "7");

        let signer = RuntimeWorkerMutationSourceSigner::from_identity(&identity);
        let token = signer
            .issue_worker_remove(
                "server-main",
                &api.config.workspace_id,
                "7",
                "runtime-target",
                "target-worker",
                60,
            )
            .unwrap();
        let verified = crate::worker_source::verify_worker_remove_source(
            &api,
            crate::worker_source::PresentedWorkerMutationSourceProof::Remote(&token),
            "runtime-target",
            "target-worker",
        )
        .await
        .unwrap();
        assert_eq!(verified.runtime_id, "runtime-remote");
        assert_eq!(verified.worker_id, "7");

        let wrong_scope = signer
            .issue_worker_remove(
                "server-wrong",
                &api.config.workspace_id,
                "7",
                "runtime-target",
                "target-worker",
                60,
            )
            .unwrap();
        assert!(matches!(
            crate::worker_source::verify_worker_remove_source(
                &api,
                crate::worker_source::PresentedWorkerMutationSourceProof::Remote(&wrong_scope),
                "runtime-target",
                "target-worker",
            )
            .await,
            Err(crate::worker_source::WorkerMutationSourceProofError::WrongAudience)
        ));

        let wrong_workspace = signer
            .issue_worker_remove(
                "server-main",
                "workspace-other",
                "7",
                "runtime-target",
                "target-worker",
                60,
            )
            .unwrap();
        assert!(matches!(
            crate::worker_source::verify_worker_remove_source(
                &api,
                crate::worker_source::PresentedWorkerMutationSourceProof::Remote(&wrong_workspace),
                "runtime-target",
                "target-worker",
            )
            .await,
            Err(crate::worker_source::WorkerMutationSourceProofError::WrongWorkspace)
        ));

        let missing_worker = signer
            .issue_worker_remove(
                "server-main",
                &api.config.workspace_id,
                "999",
                "runtime-target",
                "target-worker",
                60,
            )
            .unwrap();
        assert!(matches!(
            crate::worker_source::verify_worker_remove_source(
                &api,
                crate::worker_source::PresentedWorkerMutationSourceProof::Remote(&missing_worker),
                "runtime-target",
                "target-worker",
            )
            .await,
            Err(crate::worker_source::WorkerMutationSourceProofError::WorkerCatalogMembership)
        ));

        let mut expired_claims = decode_worker_mutation_source_claims(&token).unwrap();
        expired_claims.iat = 1;
        expired_claims.exp = 2;
        expired_claims.jti = "expired-proof".to_string();
        let expired = signer.sign(&expired_claims).unwrap();
        assert!(matches!(
            crate::worker_source::verify_worker_remove_source(
                &api,
                crate::worker_source::PresentedWorkerMutationSourceProof::Remote(&expired),
                "runtime-target",
                "target-worker",
            )
            .await,
            Err(crate::worker_source::WorkerMutationSourceProofError::Expired)
        ));

        let route_token = signer
            .issue_worker_remove(
                "server-main",
                &api.config.workspace_id,
                "7",
                "runtime-target",
                "target-worker",
                60,
            )
            .unwrap();
        let route_response = build_inner_router(api.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/w/{TEST_WORKSPACE_ID}/workers/remove"))
                    .header(CONTENT_TYPE, "application/json")
                    .header(
                        worker_runtime::auth::WORKER_MUTATION_SOURCE_PROOF_HEADER,
                        route_token,
                    )
                    .body(Body::from(
                        r#"{"target_runtime_id":"runtime-target","target_worker_id":"target-worker","reason":"retire target Worker"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(route_response.status(), StatusCode::NOT_FOUND);
        let route_body = axum::body::to_bytes(route_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let route_body = String::from_utf8(route_body.to_vec()).unwrap();
        assert!(route_body.contains("unknown_worker"));
        assert!(!route_body.contains("source"));
        assert!(!route_body.contains("proof"));

        let mut revoked = trust;
        revoked.revoked_at = Some("2026-08-11T00:01:00Z".to_string());
        let authority = SqliteWorkspaceStore::open(api.config.database_path.clone()).unwrap();
        authority.upsert_trusted_runtime(&revoked).unwrap();
        let revoked_token = signer
            .issue_worker_remove(
                "server-main",
                &api.config.workspace_id,
                "7",
                "runtime-target",
                "target-worker",
                60,
            )
            .unwrap();
        assert!(matches!(
            crate::worker_source::verify_worker_remove_source(
                &api,
                crate::worker_source::PresentedWorkerMutationSourceProof::Remote(&revoked_token),
                "runtime-target",
                "target-worker",
            )
            .await,
            Err(crate::worker_source::WorkerMutationSourceProofError::RevokedRuntimeTrust)
        ));
    }

    fn seed_worker_source_member(api: &WorkspaceApi, runtime_id: &str, worker_id: &str) {
        let now = now_registry_timestamp();
        api.store
            .upsert_worker_registry(&WorkerRegistryRecord {
                workspace_id: api.config.workspace_id.clone(),
                worker: RuntimeWorkerRef::new(runtime_id, worker_id),
                display_name: worker_id.to_string(),
                profile: None,
                retention_state: "normal".to_string(),
                transcript_ref: None,
                session_ref: None,
                summary_ref: None,
                diagnostics_ref: None,
                created_at: now.clone(),
                updated_at: now,
            })
            .unwrap();
    }

    fn seed_worker_control_grant(
        api: &WorkspaceApi,
        controller: &RuntimeWorkerRef,
        subject: &RuntimeWorkerRef,
        operation_id: &str,
    ) {
        api.store
            .create_worker_control_grant(&WorkerControlGrantRecord {
                workspace_id: api.config.workspace_id.clone(),
                grant_id: format!("grant-{operation_id}"),
                controller: controller.clone(),
                subject: subject.clone(),
                relation: "spawned".to_string(),
                origin: "test".to_string(),
                permissions: vec!["remove".to_string()],
                operation_id: operation_id.to_string(),
                created_at: now_registry_timestamp(),
                revoked_at: None,
            })
            .unwrap();
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
                worker: RuntimeWorkerRef::new("runtime-test", runtime_worker_id.to_string()),
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

    fn seed_cleanup_worker_assignment(
        api: &WorkspaceApi,
        runtime_worker_id: &str,
        ticket_id: &str,
    ) {
        let conn = rusqlite::Connection::open(&api.config.database_path).unwrap();
        crate::store::configure_sqlite(&conn).unwrap();
        conn.execute(
            "INSERT INTO typed_tickets (
                 workspace_id, ticket_id, slug, title, status, kind, priority, body,
                 workflow_state, workflow_state_explicit
             ) VALUES (?1, ?2, ?2, ?2, 'open', 'task', 'normal', '', 'inprogress', 1)",
            rusqlite::params![api.config.workspace_id, ticket_id],
        )
        .unwrap();
        api.store
            .set_current_ticket_role_assignment(
                &TicketRoleAssignmentRecord {
                    workspace_id: api.config.workspace_id.clone(),
                    ticket_id: ticket_id.to_string(),
                    assignment_id: format!("assignment-{ticket_id}"),
                    role: TicketAssignmentRole::Coder,
                    principal: TicketAssignmentPrincipal::Worker {
                        runtime_id: "runtime-test".to_string(),
                        worker_id: runtime_worker_id.to_string(),
                    },
                    assigned_by: "test".to_string(),
                    assigned_at: "2026-08-25T00:00:00Z".to_string(),
                },
                None,
                &format!("event-{ticket_id}"),
                &format!("operation-{ticket_id}"),
                false,
            )
            .unwrap();
    }

    fn seed_test_repository(api: &WorkspaceApi, repository_id: &str) {
        if api
            .store
            .get_repository(&api.config.workspace_id, repository_id)
            .unwrap()
            .is_some()
        {
            return;
        }
        api.store
            .upsert_repository(&RepositoryRecord {
                workspace_id: api.config.workspace_id.clone(),
                repository_id: repository_id.to_string(),
                name: repository_id.to_string(),
                kind: "git".to_string(),
                provider: Some("git".to_string()),
                source: workspace_api::RepositorySource {
                    kind: workspace_api::RepositorySourceKind::LocalPath,
                    uri: api.config.workspace_root.display().to_string(),
                },
                default_ref: Some("HEAD".to_string()),
                source_revision: 1,
                source_fingerprint: "sha256:test".to_string(),
                observed_status: workspace_api::RepositoryObservedStatus::Unverified,
                observed_at: None,
                created_at: "1".to_string(),
                updated_at: "1".to_string(),
            })
            .unwrap();
    }

    fn seed_cleanup_workdir(api: &WorkspaceApi, workdir_id: &str, status: &str, cleanliness: &str) {
        seed_test_repository(api, "repo-test");
        let now = now_registry_timestamp();
        api.store
            .upsert_workdir_registry(&WorkdirRegistryRecord {
                workspace_id: api.config.workspace_id.clone(),
                workdir_id: workdir_id.to_string(),
                runtime_id: "runtime-test".to_string(),
                repository_id: "repo-test".to_string(),
                creation_selector: Some("HEAD".to_string()),
                creation_ref: None,
                current_selector: None,
                current_ref: None,
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
            .attach_worker_workdir(&WorkerWorkdirLinkRecord {
                workspace_id: api.config.workspace_id.clone(),
                worker: RuntimeWorkerRef::new("runtime-test", runtime_worker_id.to_string()),
                workdir_id: workdir_id.to_string(),
                role: "attachment".to_string(),
                linked_at: now_registry_timestamp(),
                unlinked_at: None,
            })
            .unwrap();
    }

    #[tokio::test]
    async fn workspace_scoped_workdir_routes_resolve_runtime_owner_from_registry() {
        let workspace = tempfile::tempdir().unwrap();
        init_clean_git_workspace(workspace.path());
        let api = test_api(workspace.path()).await;
        seed_cleanup_workdir(&api, "remote-workdir", "present", "clean");

        assert_eq!(
            registered_workdir_runtime_id(&api, "remote-workdir").unwrap(),
            "runtime-test"
        );
    }

    #[test]
    fn workdir_session_owner_is_only_sent_for_same_runtime_worker() {
        let embedded_worker = RuntimeWorkerRef::new("embedded-worker-runtime", "5");
        assert_eq!(
            runtime_local_owner_worker_id(&embedded_worker, "arcadia"),
            None
        );
        let arcadia_worker = RuntimeWorkerRef::new("arcadia", "30");
        assert_eq!(
            runtime_local_owner_worker_id(&arcadia_worker, "arcadia"),
            Some("30")
        );
    }

    #[tokio::test]
    async fn current_worker_workdir_routes_require_a_live_runtime_worker_identity() {
        let workspace = tempfile::tempdir().unwrap();
        init_clean_git_workspace(workspace.path());
        let api = test_api(workspace.path()).await;
        let response = build_inner_router(api)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/api/w/{TEST_WORKSPACE_ID}/workers/self/workdir-attachment"
                    ))
                    .header("content-type", "application/json")
                    .body(Body::from(json!({"workdir_id": "wd"}).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn delegated_workdir_session_fence_rejects_reattached_link() {
        let first = WorkerWorkdirLinkRecord {
            workspace_id: "workspace-a".to_string(),
            worker: workdir::workspace::RuntimeWorkerRef::new("runtime-a", "worker-a"),
            workdir_id: "workdir-a".to_string(),
            role: "primary".to_string(),
            linked_at: "2026-01-01T00:00:00Z".to_string(),
            unlinked_at: None,
        };
        let expected = current_worker_workdir_session_fence(&first);
        assert!(validate_current_worker_workdir_session_fence(&first, None).is_ok());
        assert!(validate_current_worker_workdir_session_fence(&first, Some(&expected)).is_ok());

        let reattached = WorkerWorkdirLinkRecord {
            linked_at: "2026-01-01T00:00:01Z".to_string(),
            ..first
        };
        assert!(matches!(
            validate_current_worker_workdir_session_fence(&reattached, Some(&expected)),
            Err(Error::WorkdirAttachmentConflict(_))
        ));
    }

    #[tokio::test]
    async fn backend_workdir_session_proxy_executes_typed_operations() {
        use manifest::Scope;
        use workdir::LocalWorkdirSession;

        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("visible.txt"), "attached").unwrap();
        let session: WorkdirSessionHandle = Arc::new(LocalWorkdirSession::new(
            Scope::writable(root.path()).unwrap(),
            root.path().to_path_buf(),
        ));
        let result = execute_workdir_session_operation(
            &session,
            WorkdirSessionOperation::List(workdir::ListRequest {
                path: workdir::WorkdirPath::root(),
                limit: 10,
            }),
        )
        .await
        .unwrap();
        assert!(matches!(
            result,
            WorkdirSessionOperationResult::List(ref result)
                if result.entries.iter().any(|entry| entry.path.as_str().ends_with("visible.txt"))
        ));
    }

    #[tokio::test]
    async fn simple_workdir_cleanup_rejects_dirty_and_blocked_candidates() {
        let workspace = tempfile::tempdir().unwrap();
        init_clean_git_workspace(workspace.path());
        let api = test_api(workspace.path()).await;
        seed_cleanup_workdir(&api, "dirty-workdir", "present", "dirty");
        let dirty =
            cleanup_working_directory_for_runtime(api.clone(), "runtime-test", "dirty-workdir")
                .unwrap_err();
        assert!(matches!(
            dirty.error,
            Error::RuntimeOperationFailed { ref code, .. }
                if code == "workspace_cleanup_dirty_confirmation_required"
        ));

        let pinned = seed_cleanup_worker(&api, 17, "pinned");
        seed_cleanup_workdir(&api, "blocked-workdir", "present", "clean");
        seed_cleanup_link(&api, pinned.as_str(), "blocked-workdir");
        let blocked =
            cleanup_working_directory_for_runtime(api.clone(), "runtime-test", "blocked-workdir")
                .unwrap_err();
        assert!(matches!(
            blocked.error,
            Error::RuntimeOperationFailed { ref code, .. }
                if code == "workspace_cleanup_workdir_blocked"
        ));
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
        assert!(
            execute_runtime_cleanup(&api, "runtime-test", stale)
                .await
                .is_err()
        );
        let pinned = ExecuteRuntimeCleanupRequest {
            expected_plan_revision: plan.revision,
            expected_plan_digest: plan.digest,
            worker_target_ids: vec![target],
            workdir_target_ids: Vec::new(),
            confirm_dirty_discard_target_ids: Vec::new(),
        };
        assert!(
            execute_runtime_cleanup(&api, "runtime-test", pinned)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn cleanup_blocks_assigned_worker_before_runtime_deletion() {
        let workspace = tempfile::tempdir().unwrap();
        init_clean_git_workspace(workspace.path());
        let api = test_api(workspace.path()).await;
        let worker_id = seed_cleanup_worker(&api, 3, "normal");
        seed_cleanup_worker_assignment(&api, &worker_id, "ticket-assigned");

        let plan = build_runtime_cleanup_plan(&api, "runtime-test")
            .unwrap_or_else(|err| panic!("cleanup plan: {}", err.error));
        let candidate = plan
            .workers
            .iter()
            .find(|candidate| candidate.worker_id == worker_id)
            .unwrap();
        assert_eq!(
            candidate.blocking_reason.as_deref(),
            Some("worker has current Ticket assignment `ticket-assigned` (`coder`)")
        );
        let request = ExecuteRuntimeCleanupRequest {
            expected_plan_revision: plan.revision.clone(),
            expected_plan_digest: plan.digest.clone(),
            worker_target_ids: vec![candidate.target_id.clone()],
            workdir_target_ids: Vec::new(),
            confirm_dirty_discard_target_ids: Vec::new(),
        };

        let error = execute_runtime_cleanup(&api, "runtime-test", request)
            .await
            .unwrap_err();
        assert!(
            matches!(
                error.error,
                Error::RuntimeOperationFailed { ref code, .. }
                    if code == "workspace_cleanup_worker_assigned"
            ),
            "unexpected cleanup error: {:?}",
            error.error
        );
        assert!(
            api.store
                .get_worker_registry(
                    &api.config.workspace_id,
                    &RuntimeWorkerRef::new("runtime-test", worker_id),
                )
                .unwrap()
                .is_some()
        );
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
        assert!(
            execute_runtime_cleanup(&api, "runtime-test", missing_confirmation)
                .await
                .is_err()
        );
        let delete_removed = ExecuteRuntimeCleanupRequest {
            expected_plan_revision: plan.revision,
            expected_plan_digest: plan.digest,
            worker_target_ids: Vec::new(),
            workdir_target_ids: vec![removed_target],
            confirm_dirty_discard_target_ids: Vec::new(),
        };
        let response = execute_runtime_cleanup(&api, "runtime-test", delete_removed)
            .await
            .unwrap_or_else(|err| panic!("cleanup execution: {}", err.error));
        assert_eq!(response.results[0].status, "deleted");
        assert!(
            api.store
                .get_workdir_registry(&api.config.workspace_id, "workdir-not-found")
                .unwrap()
                .is_none()
        );
    }

    const TEST_RUNTIME_HTTP_TOKEN: &str = "workspace-server-test-runtime-token";

    async fn serve_runtime_http_with_injected_test_auth(
        runtime: worker_runtime::Runtime,
        listener: tokio::net::TcpListener,
    ) -> std::io::Result<()> {
        const TOKEN: &str = "workspace-server-test-runtime-token";

        async fn inject_runtime_auth(
            State(inner): State<Router>,
            mut request: Request<Body>,
        ) -> Response {
            request.headers_mut().insert(
                axum::http::header::AUTHORIZATION,
                axum::http::HeaderValue::from_static("Bearer workspace-server-test-runtime-token"),
            );
            match inner.oneshot(request).await {
                Ok(response) => response,
                Err(error) => match error {},
            }
        }

        let protected = worker_runtime::http_server::runtime_http_router(runtime, TOKEN.to_owned());
        let proxy = Router::new()
            .fallback(axum::routing::any(inject_runtime_auth))
            .with_state(protected);
        axum::serve(listener, proxy).await
    }

    async fn test_app(workspace_root: impl Into<PathBuf>) -> Router {
        build_inner_router(test_api(workspace_root).await)
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
    async fn runtime_request_proof_rejects_trust_bound_to_another_workspace() {
        let workspace = tempfile::tempdir().unwrap();
        let mut api = test_api(workspace.path()).await;
        let identity =
            worker_runtime::auth::RuntimeIdentityMaterial::generate("runtime-test").unwrap();
        configure_runtime_request_auth(&mut api, &identity, "runtime-test");
        let other_workspace = "019d0000-0000-7000-8000-0000000000bb";
        let path = format!("/api/runtime/v1/workspaces/{other_workspace}/resources/fetch");
        let proof = worker_runtime::auth::RuntimeRequestSourceSigner::from_identity(&identity)
            .issue(
                "server-test",
                other_workspace,
                None,
                worker_runtime::auth::BACKEND_RESOURCE_FETCH_PERMISSION,
                "POST",
                &path,
                b"{}",
                i64::try_from(worker_runtime::auth::unix_now_seconds()).unwrap_or(i64::MAX),
                30,
            )
            .unwrap();
        let result = crate::worker_source::verify_runtime_request_source_proof_with_store(
            api.store.as_ref(),
            &api.config,
            &proof,
            other_workspace,
            worker_runtime::auth::BACKEND_RESOURCE_FETCH_PERMISSION,
            "POST",
            &path,
            &worker_runtime::auth::request_body_digest(b"{}"),
        )
        .await;
        assert!(matches!(
            result,
            Err(crate::worker_source::WorkerMutationSourceProofError::WrongWorkspace)
        ));
    }

    #[tokio::test]
    async fn runtime_request_proof_verifies_path_and_query_for_ticket_search() {
        let workspace = tempfile::tempdir().unwrap();
        let mut api = test_api(workspace.path()).await;
        let identity =
            worker_runtime::auth::RuntimeIdentityMaterial::generate("runtime-test").unwrap();
        configure_runtime_request_auth(&mut api, &identity, "runtime-test");
        seed_worker_source_member(&api, "runtime-test", "worker-test");
        let signer = worker_runtime::auth::RuntimeRequestSourceSigner::from_identity(&identity);
        let signed_target =
            format!("/api/w/{TEST_WORKSPACE_ID}/tickets/search?state=active&limit=20");
        let tampered_target =
            format!("/api/w/{TEST_WORKSPACE_ID}/tickets/search?state=all&limit=20");
        let issue = |target: &str| {
            signer
                .issue(
                    "server-test",
                    TEST_WORKSPACE_ID,
                    Some("worker-test"),
                    worker_runtime::auth::WORKSPACE_REQUEST_PERMISSION,
                    "GET",
                    target,
                    b"",
                    i64::try_from(worker_runtime::auth::unix_now_seconds()).unwrap_or(i64::MAX),
                    30,
                )
                .unwrap()
        };
        let app = build_router(api);

        let tampered = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(&tampered_target)
                    .header(
                        worker_runtime::auth::RUNTIME_REQUEST_SOURCE_PROOF_HEADER,
                        issue(&signed_target),
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(tampered.status(), StatusCode::UNAUTHORIZED);

        let valid = app
            .oneshot(
                Request::builder()
                    .uri(&signed_target)
                    .header(
                        worker_runtime::auth::RUNTIME_REQUEST_SOURCE_PROOF_HEADER,
                        issue(&signed_target),
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(valid.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn internal_resource_fetch_rest_returns_typed_missing_resource() {
        let workspace = tempfile::tempdir().unwrap();
        init_clean_git_workspace(workspace.path());
        let mut api = test_api(workspace.path()).await;
        let identity =
            worker_runtime::auth::RuntimeIdentityMaterial::generate("runtime-test").unwrap();
        configure_runtime_request_auth(&mut api, &identity, "runtime-test");
        let app = build_inner_router(api.clone());
        let handle = missing_resource_handle();
        let body = serde_json::to_vec(&worker_runtime::resource::BackendResourceFetchRequest {
            audit_correlation_id: handle.audit_correlation_id.clone(),
            runtime_id: "runtime-test".to_string(),
            worker_id: None,
            handle,
        })
        .unwrap();
        let response = app
            .oneshot(runtime_resource_fetch_request(&api, &identity, body))
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
    async fn runtime_signed_profile_source_archive_fetch_uses_resource_permission() {
        let workspace = tempfile::tempdir().unwrap();
        init_clean_git_workspace(workspace.path());
        let mut api = test_api(workspace.path()).await;
        let identity =
            worker_runtime::auth::RuntimeIdentityMaterial::generate("runtime-test").unwrap();
        configure_runtime_request_auth(&mut api, &identity, "runtime-test");
        let handle = api.resource_broker.issue_profile_source_archive_handle(
            TEST_WORKSPACE_ID,
            crate::resource_broker::BackendResourceTarget::Runtime("runtime-test"),
            test_profile_archive(),
        );
        let path = format!(
            "/api/w/{TEST_WORKSPACE_ID}/profile-source-archives/{}",
            handle.digest
        );
        let proof = worker_runtime::auth::RuntimeRequestSourceSigner::from_identity(&identity)
            .issue(
                "server-test",
                TEST_WORKSPACE_ID,
                None,
                worker_runtime::auth::BACKEND_RESOURCE_FETCH_PERMISSION,
                "GET",
                &path,
                b"",
                i64::try_from(worker_runtime::auth::unix_now_seconds()).unwrap_or(i64::MAX),
                30,
            )
            .unwrap();
        let response = build_router(api)
            .oneshot(
                Request::builder()
                    .uri(path)
                    .header(
                        worker_runtime::auth::RUNTIME_REQUEST_SOURCE_PROOF_HEADER,
                        proof,
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(ETAG).unwrap().to_str().unwrap(),
            format!("\"profile-source:{}\"", handle.digest)
        );
        assert!(
            !to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn remote_http_resource_fetch_uses_backend_resource_contract() {
        let workspace = tempfile::tempdir().unwrap();
        init_clean_git_workspace(workspace.path());
        let mut api = test_api(workspace.path()).await;
        let identity =
            worker_runtime::auth::RuntimeIdentityMaterial::generate("runtime-test").unwrap();
        configure_runtime_request_auth(&mut api, &identity, "runtime-test");
        let broker = api.resource_broker.clone();
        let archive = test_profile_archive();
        let runtime_id = "runtime-test";
        let handle = broker.issue_profile_source_archive_handle(
            TEST_WORKSPACE_ID,
            crate::resource_broker::BackendResourceTarget::Runtime(runtime_id),
            archive,
        );
        let app = build_inner_router(api);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = worker_runtime::resource::HttpBackendResourceClient::new(
            format!("http://{addr}/api/runtime/v1/workspaces/{TEST_WORKSPACE_ID}/resources/fetch"),
            None,
        )
        .with_runtime_request_source(&identity, "server-test");

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
                selector: worker_runtime::catalog::ProfileSelector::Builtin(
                    "builtin:companion".to_string(),
                ),
                label: Some("server-test".to_string()),
            }],
            declarations: Vec::new(),
            prompt_catalog: None,
            profile_source_archive: None,
            profile_source_archive_handle: None,
        }
        .with_computed_digest()
    }

    fn runtime_create_request() -> worker_runtime::catalog::CreateWorkerRequest {
        let bundle = runtime_test_bundle();
        let mut memory_settings = test_worker_memory_settings();
        memory_settings.workspace_id = "local".to_owned();
        worker_runtime::catalog::CreateWorkerRequest {
            worker_id: WorkerId::now_v7(),
            create_fingerprint: "test-create".to_string(),
            profile: worker_runtime::catalog::ProfileSelector::Builtin(
                "builtin:companion".to_string(),
            ),
            display_name: None,
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
            worker_observation_enabled: false,
            worker_observation_grants: Vec::new(),
            workspace_api: None,
            memory_settings: Some(memory_settings),
        }
    }

    fn runtime_with_worker() -> (worker_runtime::Runtime, worker_runtime::identity::WorkerRef) {
        let runtime = worker_runtime::Runtime::with_execution_backend(
            worker_runtime::RuntimeOptions::default(),
            Arc::new(DeterministicExecutionBackend::default()),
        )
        .unwrap();
        runtime.store_config_bundle(runtime_test_bundle()).unwrap();
        let worker = runtime
            .create_worker_scoped(
                &worker_runtime::RuntimeWorkspaceScope::new("local", "local-token"),
                runtime_create_request(),
            )
            .unwrap();
        (runtime, worker.worker_ref)
    }

    #[test]
    fn provider_rejection_classification_prefers_stable_error_diagnostic() {
        let diagnostics = vec![
            RuntimeDiagnostic {
                code: "runtime_capacity_warning".to_string(),
                severity: DiagnosticSeverity::Warning,
                message: "capacity is low".to_string(),
            },
            RuntimeDiagnostic {
                code: "runtime_capacity_unavailable".to_string(),
                severity: DiagnosticSeverity::Error,
                message: "capacity is exhausted".to_string(),
            },
        ];
        assert_eq!(
            workdir_rejection_failure_code(&diagnostics),
            "runtime_capacity_unavailable"
        );
        assert_eq!(
            workdir_rejection_failure_code(&[]),
            "runtime_workdir_create_failed"
        );
    }

    #[tokio::test]
    async fn provider_rejection_persists_stable_failure_classification() {
        let dir = tempfile::tempdir().unwrap();
        init_clean_git_workspace(dir.path());
        let api = test_api(dir.path()).await;
        let operation_id = "provider-rejection-classification";
        let request_fingerprint = crate::workdir_create_operations::request_fingerprint(
            TEST_REPOSITORY_ID,
            Some("HEAD"),
            Some(EMBEDDED_WORKER_RUNTIME_ID),
        );
        api.config_store
            .reserve_workdir_create_operation(&WorkdirCreateOperationRecord {
                workspace_id: TEST_WORKSPACE_ID.to_string(),
                operation_id: operation_id.to_string(),
                request_fingerprint: request_fingerprint.clone(),
                repository_id: TEST_REPOSITORY_ID.to_string(),
                selector: Some("HEAD".to_string()),
                requested_runtime_id: Some(EMBEDDED_WORKER_RUNTIME_ID.to_string()),
                resolved_runtime_id: EMBEDDED_WORKER_RUNTIME_ID.to_string(),
                config_revision: 1,
                config_projection_digest: "sha256:test".to_string(),
                working_directory_id: "workdir-provider-rejection".to_string(),
                state: "pending".to_string(),
                failure: None,
                created_at: now_registry_timestamp(),
                updated_at: now_registry_timestamp(),
            })
            .unwrap();
        let diagnostics = vec![RuntimeDiagnostic {
            code: "runtime_capacity_unavailable".to_string(),
            severity: DiagnosticSeverity::Error,
            message: "capacity is exhausted".to_string(),
        }];

        assert_eq!(
            finish_rejected_workdir_create_operation(
                &api.config_store,
                TEST_WORKSPACE_ID,
                operation_id,
                &request_fingerprint,
                &diagnostics,
                &now_registry_timestamp(),
            )
            .unwrap(),
            "runtime_capacity_unavailable"
        );
        let operation = api
            .config_store
            .load_workdir_create_operation(TEST_WORKSPACE_ID, operation_id)
            .unwrap()
            .unwrap();
        assert_eq!(operation.state, "failed");
        assert_eq!(
            operation.failure.as_deref(),
            Some("runtime_capacity_unavailable")
        );
    }

    #[test]
    fn runtime_capacity_unavailable_maps_to_service_unavailable() {
        let response = ApiError::from(Error::RuntimeOperationFailed {
            runtime_id: "runtime".to_string(),
            code: "runtime_capacity_unavailable".to_string(),
            message: "Runtime capacity is exhausted".to_string(),
        })
        .into_response();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[test]
    fn default_runtime_not_configured_operation_error_maps_to_bad_request() {
        let response = ApiError::from(Error::RuntimeOperationFailed {
            runtime_id: "workspace-config".to_string(),
            code: "default_runtime_not_configured".to_string(),
            message: "Workspace default Runtime is not configured".to_string(),
        })
        .into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn browser_workspace_workdir_create_requires_configured_default_runtime() {
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
        assert_eq!(response["error"], "Bad Request");
        assert_eq!(
            response["diagnostics"][0]["code"],
            "default_runtime_not_configured"
        );
        let projected = serde_json::to_string(&response).unwrap();
        assert!(!projected.contains(dir.path().to_string_lossy().as_ref()));
    }

    #[tokio::test]
    async fn browser_workspace_workdir_create_does_not_fallback_from_explicit_runtime() {
        let dir = tempfile::tempdir().unwrap();
        init_clean_git_workspace(dir.path());
        let api = test_api(dir.path()).await;
        let token = seed_test_api_token(api.store.as_ref(), "explicit-runtime-no-fallback");
        set_test_default_runtime(&api, EMBEDDED_WORKER_RUNTIME_ID);
        let operation_id = "workdir-create-explicit-runtime";

        let response = request_json_authenticated(
            build_router(api.clone()),
            "POST",
            &format!("/api/w/{TEST_WORKSPACE_ID}/working-directories"),
            Some(serde_json::json!({
                "runtime_id": "missing-runtime",
                "repository_id": TEST_REPOSITORY_ID,
                "selector": "HEAD",
                "operation_id": operation_id,
            })),
            &token,
            StatusCode::NOT_FOUND,
        )
        .await;
        assert_eq!(response["error"], "Not Found");
        assert_eq!(response["diagnostics"][0]["code"], "runtime_unavailable");
        let operation = api
            .config_store
            .load_workdir_create_operation(TEST_WORKSPACE_ID, operation_id)
            .unwrap()
            .unwrap();
        assert_eq!(
            operation.requested_runtime_id.as_deref(),
            Some("missing-runtime")
        );
        assert_eq!(operation.resolved_runtime_id, "missing-runtime");
        assert_eq!(operation.state, "failed");
        assert_eq!(operation.failure.as_deref(), Some("runtime_unavailable"));
        assert!(
            api.store
                .get_workdir_registry(TEST_WORKSPACE_ID, &operation.working_directory_id)
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn browser_workspace_workdir_create_records_failed_default_resolution() {
        let dir = tempfile::tempdir().unwrap();
        init_clean_git_workspace(dir.path());
        let api = test_api(dir.path()).await;
        let token = seed_test_api_token(api.store.as_ref(), "failed-default-resolution");
        set_test_default_runtime(&api, EMBEDDED_WORKER_RUNTIME_ID);
        let app = build_router(api.clone());
        let operation_id = "workdir-create-default-runtime";

        let response = request_json_authenticated(
            app.clone(),
            "POST",
            &format!("/api/w/{TEST_WORKSPACE_ID}/working-directories"),
            Some(serde_json::json!({
                "repository_id": TEST_REPOSITORY_ID,
                "selector": "HEAD",
                "operation_id": operation_id,
            })),
            &token,
            StatusCode::BAD_REQUEST,
        )
        .await;
        assert_eq!(response["error"], "Bad Request");
        assert_eq!(
            response["diagnostics"][0]["code"],
            "runtime_workdir_unsupported"
        );
        set_test_default_runtime(&api, "not-a-registered-runtime");
        request_json_authenticated(
            app,
            "POST",
            &format!("/api/w/{TEST_WORKSPACE_ID}/working-directories"),
            Some(serde_json::json!({
                "repository_id": TEST_REPOSITORY_ID,
                "selector": "HEAD",
                "operation_id": operation_id,
            })),
            &token,
            StatusCode::BAD_REQUEST,
        )
        .await;

        let operation = api
            .config_store
            .load_workdir_create_operation(TEST_WORKSPACE_ID, operation_id)
            .unwrap()
            .unwrap();
        assert_eq!(operation.resolved_runtime_id, EMBEDDED_WORKER_RUNTIME_ID);
        assert_eq!(operation.state, "failed");
        assert_eq!(
            operation.failure.as_deref(),
            Some("runtime_workdir_unsupported")
        );
        assert_eq!(operation.config_revision, 2);
        assert!(!operation.config_projection_digest.is_empty());
        assert!(
            api.store
                .get_workdir_registry(TEST_WORKSPACE_ID, &operation.working_directory_id)
                .unwrap()
                .is_none()
        );
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
                "initial_submit": [],
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

        let persisted = BackendRuntimesConfigFile::load_from_path(
            dir.path().join(".test-config/runtimes.toml"),
        )
        .unwrap();
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
        let persisted = BackendRuntimesConfigFile::load_from_path(
            dir.path().join(".test-config/runtimes.toml"),
        )
        .unwrap();
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
                serve_runtime_http_with_injected_test_auth(runtime, runtime_listener)
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
        let persisted = BackendRuntimesConfigFile::load_from_path(
            dir.path().join(".test-config/runtimes.toml"),
        )
        .unwrap();
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
                serve_runtime_http_with_injected_test_auth(runtime, runtime_listener)
                    .await
                    .unwrap()
            }
        });

        let dir = tempfile::tempdir().unwrap();
        let endpoint = format!("http://{runtime_addr}");
        BackendRuntimesConfigFile {
            runtimes: WorkspaceBackendRuntimesConfig {
                remote: vec![RemoteRuntimeConfigFile {
                    id: "probe-runtime".to_string(),
                    endpoint: endpoint.clone(),
                    display_name: Some("Probe Runtime".to_string()),
                    token_ref: None,
                }],
            },
        }
        .write_to_path(dir.path().join(".test-config/runtimes.toml"))
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
            serve_runtime_http_with_injected_test_auth(runtime, runtime_listener)
                .await
                .unwrap()
        });

        let dir = tempfile::tempdir().unwrap();
        let endpoint = format!("http://{runtime_addr}");
        BackendRuntimesConfigFile {
            runtimes: WorkspaceBackendRuntimesConfig {
                remote: vec![RemoteRuntimeConfigFile {
                    id: "control-only-runtime".to_string(),
                    display_name: Some("Control-only Runtime".to_string()),
                    endpoint,
                    token_ref: None,
                }],
            },
        }
        .write_to_path(dir.path().join(".test-config/runtimes.toml"))
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
    async fn merge_request_reads_use_first_class_workspace_resources() {
        let dir = tempfile::tempdir().unwrap();
        let app = build_inner_router(test_api(dir.path()).await);

        let collection = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/w/{TEST_WORKSPACE_ID}/merge-requests"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(collection.status(), StatusCode::OK);
        let body = to_bytes(collection.into_body(), usize::MAX).await.unwrap();
        let body: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["items"], json!([]));
        assert!(body["next_cursor"].is_null());

        let missing = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/w/{TEST_WORKSPACE_ID}/merge-requests/missing"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);

        let nested = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/api/w/{TEST_WORKSPACE_ID}/tickets/T-1/merge-request"
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(nested.status(), StatusCode::METHOD_NOT_ALLOWED);

        let invalid_filter = app
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/api/w/{TEST_WORKSPACE_ID}/merge-requests?state=unknown"
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(invalid_filter.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn ticket_rest_search_allows_workspace_product_clients_and_rejects_invalid_worker_source()
    {
        let dir = tempfile::tempdir().unwrap();
        let api = test_api(dir.path()).await;
        let app = build_inner_router(api);

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!(
                        "/api/w/{TEST_WORKSPACE_ID}/tickets/search?state=active"
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/w/{TEST_WORKSPACE_ID}/tickets"))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&ticket::NewTicket::new("CLI Ticket")).unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        for path in [
            TICKET_RELATIONS_QUERY_PATH,
            TICKET_ORCHESTRATION_PLANS_QUERY_PATH,
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(format!("/api/w/{TEST_WORKSPACE_ID}{path}"))
                        .header("content-type", "application/json")
                        .body(Body::from(r#"{"ticket":null,"kind":null}"#))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "path: {path}");
        }

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!(
                        "/api/w/{TEST_WORKSPACE_ID}/tickets/search?state=active"
                    ))
                    .header("x-yoi-runtime-id", "embedded-worker-runtime")
                    .header("x-yoi-worker-id", "missing-worker")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/w/{TEST_WORKSPACE_ID}/tickets/backend"))
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    }

    #[tokio::test]
    async fn browser_worker_create_uses_workspace_default_and_preserves_unsupported_diagnostics() {
        let dir = tempfile::tempdir().unwrap();
        let app = test_app(dir.path()).await;
        let created = post_json(
            app.clone(),
            "/api/workers",
            serde_json::json!({
                "runtime_id": "embedded-worker-runtime",
                "display_name": "",
                "initial_submit": []
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
        assert_eq!(worker["label"], "Worker");
        assert_eq!(worker["profile"], "builtin:companion");
        assert!(worker.get("role").is_none());
        assert_eq!(worker["worker_id"], created["worker_id"]);
        let resource_key = worker["resource_key"]
            .as_str()
            .expect("Workspace Worker list must project a resource key");
        assert!(resource_key.starts_with("W-"));

        let runtime_workers =
            get_json(app.clone(), "/api/runtimes/embedded-worker-runtime/workers").await;
        let runtime_workers = serde_json::from_value::<
            workspace_api::ListResponse<workspace_api::WorkerSummary>,
        >(runtime_workers)
        .expect("Runtime-scoped Worker list must use the shared Workspace API contract");
        assert!(
            runtime_workers
                .items
                .iter()
                .any(|worker| worker.resource_key == resource_key)
        );
        let detail_path = format!(
            "/api/runtimes/{}/workers/{}",
            created["runtime_id"].as_str().unwrap(),
            created["worker_id"].as_str().unwrap()
        );
        let detail = get_json(app.clone(), detail_path.as_str()).await;
        assert_eq!(detail["label"], "Worker");
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
                "profile": "builtin:companion",
                "initial_submit": []
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
                "profile": {
                    "kind": "builtin",
                    "value": "builtin:coder"
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
                "profile": {
                    "kind": "builtin",
                    "value": "builtin:coder"
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
                "initial_submit": [],
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
        let static_dir = dir.path().join("static");
        std::fs::create_dir_all(static_dir.join("assets")).unwrap();
        std::fs::write(static_dir.join("index.html"), "<main>Yoi Workspace</main>").unwrap();
        std::fs::write(static_dir.join("assets/app.js"), "console.log('yoi');").unwrap();

        let store = SqliteWorkspaceStore::in_memory().unwrap();
        let mut config = test_server_config(dir.path());
        let sqlite_store = SqliteWorkspaceStore::open(&config.database_path).unwrap();
        sqlite_store
            .upsert_workspace(&WorkspaceRecord {
                workspace_id: TEST_WORKSPACE_ID.to_string(),
                owner_account_id: None,
                display_name: "Test Workspace".to_string(),
                state: "active".to_string(),
                created_at: "2026-01-01T00:00:00Z".to_string(),
                updated_at: "2026-01-01T00:00:00Z".to_string(),
            })
            .await
            .unwrap();
        let ticket_id = write_ticket(
            &config.database_path,
            TEST_WORKSPACE_ID,
            "API Ticket",
            ticket::TicketWorkflowState::Ready,
        );
        let sqlite_store = SqliteWorkspaceStore::open(&config.database_path).unwrap();
        sqlite_store
            .upsert_workspace(&WorkspaceRecord {
                workspace_id: TEST_WORKSPACE_ID.to_string(),
                owner_account_id: None,
                display_name: "Test Workspace".to_string(),
                state: "active".to_string(),
                created_at: "2026-01-01T00:00:00Z".to_string(),
                updated_at: "2026-01-01T00:00:00Z".to_string(),
            })
            .await
            .unwrap();
        sqlite_store
            .upsert_objective(&ObjectiveRecord {
                workspace_id: TEST_WORKSPACE_ID.to_string(),
                objective_id: "00000000001J3".to_string(),
                title: "API Objective".to_string(),
                state: "active".to_string(),
                body_md: "Objective body.\n".to_string(),
                created_at: "2026-01-01T00:00:00Z".to_string(),
                updated_at: "2026-01-02T00:00:00Z".to_string(),
            })
            .unwrap();
        sqlite_store
            .replace_objective_ticket_links(
                TEST_WORKSPACE_ID,
                "00000000001J3",
                &[ObjectiveTicketLinkRecord {
                    workspace_id: TEST_WORKSPACE_ID.to_string(),
                    objective_id: "00000000001J3".to_string(),
                    ticket_id: ticket_id.clone(),
                    kind: "linked".to_string(),
                    created_at: "2026-01-01T00:00:00Z".to_string(),
                }],
            )
            .unwrap();
        sqlite_store
            .upsert_objective_resource(&ObjectiveResourceRecord {
                workspace_id: TEST_WORKSPACE_ID.to_string(),
                objective_id: "00000000001J3".to_string(),
                resource_path: "memory-architecture-overview.md".to_string(),
                body: "# Memory architecture\n\nResource body.\n".to_string(),
                media_type: Some("text/markdown".to_string()),
                created_at: "2026-01-01T00:00:00Z".to_string(),
                updated_at: "2026-01-02T00:00:00Z".to_string(),
            })
            .unwrap();
        sqlite_store
            .upsert_memory_document(&MemoryDocumentRecord {
                workspace_id: TEST_WORKSPACE_ID.to_string(),
                body_md: "# Memory\n\n## Project facts\n\n- Frontend can read this document.\n"
                    .to_string(),
                created_at: "2026-01-01T00:00:00Z".to_string(),
                updated_at: "2026-01-02T00:00:00Z".to_string(),
            })
            .unwrap();
        sqlite_store
            .upsert_memory_staging_record(&MemoryStagingRecord {
                workspace_id: TEST_WORKSPACE_ID.to_string(),
                candidate_id: "00000000001J4".to_string(),
                raw_json: memory_staging_record_json("00000000001J4", "SQLite memory candidate"),
                source_path: None,
                imported_at: "2026-01-01T00:00:00Z".to_string(),
            })
            .unwrap();
        assert_eq!(
            sqlite_store
                .count_memory_staging_records(TEST_WORKSPACE_ID)
                .unwrap(),
            1
        );
        write_objective(
            dir.path(),
            "00000000001J5",
            "Filesystem Only Objective",
            "active",
        );
        config.static_assets_dir = Some(static_dir);
        let api = WorkspaceApi::new_with_execution_backend(
            config,
            Arc::new(store),
            Arc::new(DeterministicExecutionBackend::default()),
        )
        .await
        .unwrap();
        let app = build_inner_router(api);

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
        assert_eq!(tickets["items"][0]["title"], "API Ticket");
        assert_eq!(tickets["items"][0]["state"], "ready");

        let objectives = get_json(app.clone(), "/api/objectives").await;
        assert_eq!(objectives["items"].as_array().unwrap().len(), 1);
        assert_eq!(objectives["items"][0]["id"], "00000000001J3");
        assert_eq!(objectives["items"][0]["summary"], "Objective body.");
        assert_eq!(objectives["record_authority"], "workspace-sqlite");
        let scoped_objectives = get_json(
            app.clone(),
            &format!("/api/w/{TEST_WORKSPACE_ID}/objectives"),
        )
        .await;
        assert_eq!(scoped_objectives["items"][0]["id"], "00000000001J3");
        let limited_objectives = get_json(
            app.clone(),
            &format!("/api/w/{TEST_WORKSPACE_ID}/objectives?limit=0"),
        )
        .await;
        assert_eq!(limited_objectives["items"].as_array().unwrap().len(), 0);
        let scoped_objective = get_json(
            app.clone(),
            &format!("/api/w/{TEST_WORKSPACE_ID}/objectives/00000000001J3"),
        )
        .await;
        assert_eq!(scoped_objective["id"], "00000000001J3");
        assert_eq!(scoped_objective["record_source"], "workspace-sqlite");
        assert_eq!(
            scoped_objective["revision"].as_str().unwrap().is_empty(),
            false
        );
        assert_eq!(
            scoped_objective["resources"][0]["path"],
            "memory-architecture-overview.md"
        );
        let queried_tickets = request_json(
            app.clone(),
            "POST",
            &format!("/api/w/{TEST_WORKSPACE_ID}/tickets/query"),
            Some(json!({
                "limit": 1
            })),
            StatusCode::OK,
        )
        .await;
        assert_eq!(queried_tickets["items"][0]["title"], "API Ticket");
        let queried_ticket_id = queried_tickets["items"][0]["id"]
            .as_str()
            .expect("query Ticket id")
            .to_string();
        assert_eq!(queried_tickets["page"]["limit"], 1);
        let shown_ticket = request_json(
            app.clone(),
            "POST",
            &format!("/api/w/{TEST_WORKSPACE_ID}/tickets/{queried_ticket_id}/show"),
            Some(json!({"event_limit": 10})),
            StatusCode::OK,
        )
        .await;
        assert!(shown_ticket["evidence"]["missing"].is_array());
        assert!(shown_ticket["item_revision"].as_str().is_some());
        let queried_objectives = request_json(
            app.clone(),
            "POST",
            &format!("/api/w/{TEST_WORKSPACE_ID}/objectives/query"),
            Some(json!({
                "query": "Objective body",
                "linked_ticket_id": ticket_id,
                "limit": 1
            })),
            StatusCode::OK,
        )
        .await;
        assert_eq!(queried_objectives["items"][0]["id"], "00000000001J3");
        assert_eq!(queried_objectives["page"]["limit"], 1);
        let shown_objective = request_json(
            app.clone(),
            "POST",
            &format!("/api/w/{TEST_WORKSPACE_ID}/objectives/00000000001J3/show"),
            Some(json!({"event_limit": 10})),
            StatusCode::OK,
        )
        .await;
        assert_eq!(shown_objective["linked_tickets"][0], ticket_id);
        assert!(shown_objective["event_page"]["returned"].is_number());

        let memory_document =
            get_json(app.clone(), &format!("/api/w/{TEST_WORKSPACE_ID}/memory")).await;
        assert_eq!(memory_document["created_at"], "2026-01-01T00:00:00Z");
        assert_eq!(memory_document["updated_at"], "2026-01-02T00:00:00Z");
        assert_eq!(memory_document["bytes"], 63);
        assert_eq!(memory_document["record_source"], "workspace-sqlite");
        assert!(
            memory_document["body_md"]
                .as_str()
                .unwrap()
                .contains("Frontend can read this document.")
        );

        let memory_staging = get_json(
            app.clone(),
            &format!("/api/w/{TEST_WORKSPACE_ID}/memory/staging?limit=10"),
        )
        .await;
        assert_eq!(
            memory_staging["record_authority"],
            "sqlite_workspace_authority.memory_staging"
        );
        assert_eq!(memory_staging["total_valid_count"], 1);
        assert_eq!(memory_staging["invalid_count"], 0);
        assert_eq!(
            memory_staging["items"][0]["record"]["claim"],
            "SQLite memory candidate"
        );

        let repositories = get_json(app.clone(), "/api/repositories").await;
        assert_eq!(repositories["items"][0]["id"], TEST_REPOSITORY_ID);
        assert_eq!(repositories["items"][0]["kind"], "git");
        assert_eq!(
            repositories["items"][0]["record_authority"],
            "workspace-control-plane"
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

        let removed_repository_tickets = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/repositories/main/tickets")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(removed_repository_tickets.status(), StatusCode::NOT_FOUND);

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
        let store = test_control_store(&config);
        let api = WorkspaceApi::new_with_execution_backend(
            config,
            Arc::new(store),
            Arc::new(DeterministicExecutionBackend::default()),
        )
        .await
        .unwrap();
        let app = build_inner_router(api);

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
    async fn embedded_runtime_fs_store_restores_catalog_and_stops_failed_execution() {
        let dir = tempfile::tempdir().unwrap();
        let config = test_server_config(dir.path().join("workspace"));
        let store_root = config.embedded_runtime_store_root.clone();
        let bundle = runtime_test_bundle();
        let store = test_control_store(&config);

        let api = WorkspaceApi::new_with_execution_backend(
            config.clone(),
            Arc::new(store),
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
                test_create_binding(),
                WorkerSpawnRequest {
                    intent: WorkerSpawnIntent::TicketRole {
                        ticket_id: "00001KVZSGT0Q".to_string(),
                        role: TicketWorkerRole::Coder,
                    },
                    requested_worker_name: None,
                    acceptance: WorkerSpawnAcceptanceRequirement::RunAccepted {
                        expected_segments: 0,
                    },
                    profile: ProfileSelector::Builtin("builtin:coder".to_string()),
                    ticket_assignment: None,
                    initial_submit: Vec::new(),
                    working_directory_request: None,
                    resolved_working_directory_request: None,
                    resolved_working_directory: None,
                    resolved_config_bundle: None,
                    resolved_worker_observation_enabled: false,
                    resolved_worker_observation_grants: Vec::new(),
                    resolved_workspace_api: Some(test_worker_workspace_api(
                        "embedded-worker-runtime",
                    )),
                    resolved_memory_settings: Some(test_worker_memory_settings()),
                    resolved_control_operation: None,
                },
            )
            .expect("spawn worker");
        assert_eq!(spawned.state, WorkerOperationState::Accepted);
        let worker_id = spawned.worker.expect("created worker").worker.worker_id;
        let worker_ref = RuntimeWorkerRef::new("embedded-worker-runtime", &worker_id);
        let sent = api
            .runtime
            .send_input(
                &worker_ref,
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
            let detail = api.runtime.worker(&worker_ref).expect("worker detail");
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

        let restored_store = test_control_store(&config);
        let restored = WorkspaceApi::new_with_execution_backend(
            config,
            Arc::new(restored_store),
            Arc::new(DeterministicExecutionBackend::default()),
        )
        .await
        .expect("restored fs-backed api starts");
        let restored_worker = restored
            .runtime
            .worker(&worker_ref)
            .expect("restored worker");
        assert_eq!(restored_worker.state, "stopped");
        assert!(!restored_worker.capabilities.can_stop);

        let bundles = restored
            .runtime
            .list_config_bundles("embedded-worker-runtime")
            .expect("config bundle list");
        assert!(
            bundles.bundles.is_empty(),
            "transport-only config bundles must be re-delivered after Runtime restart"
        );

        let rejected_input = restored
            .runtime
            .send_input(
                &worker_ref,
                WorkerInputRequest {
                    kind: WorkerInputKind::User,
                    content: "should not be routed to corrupted handle".to_string(),
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
                .join("server")
                .join("workspaces")
                .join(TEST_WORKSPACE_ID)
                .join("embedded-runtime")
        );
        assert!(!default_root.starts_with(workspace_root.join(".yoi")));

        let mut config = ServerConfig::local_dev(workspace_root, test_identity())
            .with_embedded_runtime_store_root(default_root.clone());
        config.database_path = ServerConfig::server_database_path_for_data_dir(&data_dir);
        let store = test_control_store(&config);
        let app = build_inner_router(
            WorkspaceApi::new_with_execution_backend(
                config,
                Arc::new(store),
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
        let store = test_control_store(&config);
        let api = WorkspaceApi::new_with_execution_backend(
            config,
            Arc::new(store),
            Arc::new(DeterministicExecutionBackend::default()),
        )
        .await
        .unwrap();
        let app = build_inner_router(api);

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
            source: workspace_api::RepositorySource {
                kind: workspace_api::RepositorySourceKind::LocalPath,
                uri: root.path().display().to_string(),
            },
            source_revision: 1,
            source_fingerprint: "sha256:test".to_string(),
            observed_status: workspace_api::RepositoryObservedStatus::Unverified,
            observed_at: None,
            path: Some(root.path().to_path_buf()),
            display_name: None,
            default_selector: None,
        }];
        let store = test_control_store(&config);
        let api = WorkspaceApi::new_with_execution_backend(
            config,
            Arc::new(store),
            Arc::new(DeterministicExecutionBackend::default()),
        )
        .await
        .unwrap();
        let app = build_inner_router(api);

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
        let config = test_server_config(dir.path());
        let store = test_control_store(&config);
        let api = WorkspaceApi::new_with_execution_backend(
            config,
            Arc::new(store),
            Arc::new(DeterministicExecutionBackend::default()),
        )
        .await
        .unwrap();
        let app = build_inner_router(api);

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
                    "kind": "workspace_coding"
                },
                "requested_worker_name": "api-friendly-name",
                "acceptance": {
                    "kind": "run_accepted",
                    "expected_segments": 0
                },
                "profile": {
                    "kind": "builtin",
                    "value": "builtin:coder"
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

        let stopped_workers = get_json(
            app.clone(),
            "/api/runtimes/embedded-worker-runtime/workers?status=stopped",
        )
        .await;
        assert!(stopped_workers["items"].as_array().unwrap().is_empty());

        let restored = post_json(
            app.clone(),
            &format!("/api/runtimes/embedded-worker-runtime/workers/{worker_id}/restore"),
            json!({}),
        )
        .await;
        assert_eq!(restored["result"]["state"], "accepted");
        assert_eq!(restored["result"]["worker"]["worker_id"], worker_id);

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

        let projected = format!(
            "{}{}{}{}{}{}",
            embedded_summary, spawned, worker, stopped_workers, restored, accepted
        );
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
            worker: RuntimeWorkerRef::new("runtime-a", "worker-a"),
            endpoint,
            bearer_token: Some(TEST_RUNTIME_HTTP_TOKEN.to_owned()),
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
            worker: RuntimeWorkerRef::new("runtime-a", "worker-a"),
            endpoint,
            bearer_token: Some(TEST_RUNTIME_HTTP_TOKEN.to_owned()),
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
                worker_runtime::http_server::serve_runtime_http(
                    runtime,
                    runtime_listener,
                    Some(TEST_RUNTIME_HTTP_TOKEN.to_owned()),
                )
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
        let mut config = test_server_config(dir.path());
        let store = test_control_store(&config);
        let runtime_id = source.worker.runtime_id.clone();
        let worker_id = source.worker.worker_id.clone();
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
        tokio::spawn(async move {
            axum::serve(app_listener, build_inner_router(api))
                .await
                .unwrap()
        });
        (
            format!("ws://{app_addr}/api/runtimes/{runtime_id}/workers/{worker_id}/protocol/ws"),
            dir,
        )
    }

    #[tokio::test]
    async fn workspace_subscription_inner_router_projects_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = build_inner_router(test_api(dir.path()).await);
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        let (mut socket, response) = tokio_tungstenite::connect_async(format!(
            "ws://{address}/api/w/{TEST_WORKSPACE_ID}/protocol/ws"
        ))
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);
        socket.close(None).await.unwrap();
        server.abort();
    }

    #[tokio::test]
    async fn workspace_subscription_returns_workspace_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let (api, execution_backend) = test_api_with_recording_backend(dir.path()).await;

        let spawn_request = WorkerSpawnRequest {
            intent: WorkerSpawnIntent::WorkspaceCompanion,
            requested_worker_name: Some("multiplexed-console".to_string()),
            acceptance: WorkerSpawnAcceptanceRequirement::RunAccepted {
                expected_segments: 0,
            },
            profile: worker_runtime::catalog::ProfileSelector::Builtin(
                "builtin:companion".to_string(),
            ),
            ticket_assignment: None,
            initial_submit: Vec::new(),
            working_directory_request: None,
            resolved_working_directory_request: None,
            resolved_working_directory: None,
            resolved_config_bundle: Some(runtime_test_bundle()),
            resolved_worker_observation_enabled: false,
            resolved_worker_observation_grants: Vec::new(),
            resolved_control_operation: None,
            resolved_workspace_api: None,
            resolved_memory_settings: None,
        };
        let spawned = api
            .spawn_workspace_worker(EMBEDDED_WORKER_RUNTIME_ID, spawn_request)
            .unwrap();
        assert_eq!(spawned.state, WorkerOperationState::Accepted);
        let worker_id = spawned.worker.unwrap().worker.worker_id;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = build_inner_router(api);
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        let (mut socket, _) = connect_async(format!(
            "ws://{address}/api/w/{TEST_WORKSPACE_ID}/protocol/ws"
        ))
        .await
        .unwrap();
        let frame = protocol::subscription::SubscriptionFrame::new(
            protocol::subscription::SubscriptionFramePayload::Request(
                protocol::subscription::SubscriptionRequest::SubscribeEvents {
                    request_id: protocol::subscription::SubscriptionRequestId::new("request-1")
                        .unwrap(),
                    selector: protocol::subscription::EventSubscriptionSelector::WorkspaceWorkers,
                },
            ),
        );
        socket
            .send(Message::Text(serde_json::to_string(&frame).unwrap().into()))
            .await
            .unwrap();
        let Message::Text(text) = socket.next().await.unwrap().unwrap() else {
            panic!("expected subscription response");
        };
        let response: protocol::subscription::SubscriptionFrame =
            serde_json::from_str(text.as_str()).unwrap();
        let workers = match response.payload {
            protocol::subscription::SubscriptionFramePayload::Response(
                protocol::subscription::SubscriptionResponse::Subscribed {
                    selector: protocol::subscription::EventSubscriptionSelector::WorkspaceWorkers,
                    snapshot: protocol::subscription::SubscriptionSnapshot::Workers { workers },
                    ..
                },
            ) => workers,
            other => panic!("expected Workspace Worker snapshot, got {other:?}"),
        };
        assert_eq!(
            workers
                .iter()
                .find(|worker| worker.worker_id.as_str() == worker_id)
                .and_then(|worker| worker.resource_key.as_deref()),
            Some("W-1")
        );

        let subscribe_protocol = protocol::subscription::SubscriptionFrame::new(
            protocol::subscription::SubscriptionFramePayload::Request(
                protocol::subscription::SubscriptionRequest::SubscribeEvents {
                    request_id: protocol::subscription::SubscriptionRequestId::new("request-2")
                        .unwrap(),
                    selector: protocol::subscription::EventSubscriptionSelector::WorkerProtocol {
                        worker_id: protocol::subscription::SubscriptionWorkerId::new(
                            worker_id.clone(),
                        )
                        .unwrap(),
                        runtime_id: Some(EMBEDDED_WORKER_RUNTIME_ID.to_string()),
                    },
                },
            ),
        );
        socket
            .send(Message::Text(
                serde_json::to_string(&subscribe_protocol).unwrap().into(),
            ))
            .await
            .unwrap();
        let protocol_subscription_id = loop {
            let Message::Text(text) = socket.next().await.unwrap().unwrap() else {
                continue;
            };
            let frame: protocol::subscription::SubscriptionFrame =
                serde_json::from_str(text.as_str()).unwrap();
            if let protocol::subscription::SubscriptionFramePayload::Response(
                protocol::subscription::SubscriptionResponse::Subscribed {
                    subscription_id,
                    selector:
                        protocol::subscription::EventSubscriptionSelector::WorkerProtocol { .. },
                    ..
                },
            ) = frame.payload
            {
                break subscription_id;
            }
        };
        let second_subscribe = protocol::subscription::SubscriptionFrame::new(
            protocol::subscription::SubscriptionFramePayload::Request(
                protocol::subscription::SubscriptionRequest::SubscribeEvents {
                    request_id: protocol::subscription::SubscriptionRequestId::new("request-3")
                        .unwrap(),
                    selector: protocol::subscription::EventSubscriptionSelector::WorkerProtocol {
                        worker_id: protocol::subscription::SubscriptionWorkerId::new(
                            worker_id.clone(),
                        )
                        .unwrap(),
                        runtime_id: Some(EMBEDDED_WORKER_RUNTIME_ID.to_string()),
                    },
                },
            ),
        );
        socket
            .send(Message::Text(
                serde_json::to_string(&second_subscribe).unwrap().into(),
            ))
            .await
            .unwrap();
        let second_protocol_subscription_id = loop {
            let Message::Text(text) = socket.next().await.unwrap().unwrap() else {
                continue;
            };
            let frame: protocol::subscription::SubscriptionFrame =
                serde_json::from_str(text.as_str()).unwrap();
            if let protocol::subscription::SubscriptionFramePayload::Response(
                protocol::subscription::SubscriptionResponse::Subscribed {
                    subscription_id,
                    selector:
                        protocol::subscription::EventSubscriptionSelector::WorkerProtocol { .. },
                    ..
                },
            ) = frame.payload
            {
                break subscription_id;
            }
        };
        assert_ne!(protocol_subscription_id, second_protocol_subscription_id);
        let unsubscribe = protocol::subscription::SubscriptionFrame::new(
            protocol::subscription::SubscriptionFramePayload::Request(
                protocol::subscription::SubscriptionRequest::UnsubscribeEvents {
                    request_id: protocol::subscription::SubscriptionRequestId::new("request-4")
                        .unwrap(),
                    subscription_id: protocol_subscription_id,
                },
            ),
        );
        socket
            .send(Message::Text(
                serde_json::to_string(&unsubscribe).unwrap().into(),
            ))
            .await
            .unwrap();

        let method = protocol::subscription::SubscriptionFrame::new(
            protocol::subscription::SubscriptionFramePayload::WorkerProtocol(
                protocol::subscription::SubscriptionWorkerProtocolMethod {
                    subscription_id: second_protocol_subscription_id.clone(),
                    method: protocol::Method::ListCompletions {
                        kind: protocol::CompletionKind::File,
                        prefix: String::new(),
                    },
                },
            ),
        );
        socket
            .send(Message::Text(
                serde_json::to_string(&method).unwrap().into(),
            ))
            .await
            .unwrap();
        loop {
            let Message::Text(text) = socket.next().await.unwrap().unwrap() else {
                continue;
            };
            let frame: protocol::subscription::SubscriptionFrame =
                serde_json::from_str(text.as_str()).unwrap();
            if matches!(
                frame.payload,
                protocol::subscription::SubscriptionFramePayload::Event(
                    protocol::subscription::SubscriptionEvent::Event {
                        subscription_id,
                        payload: protocol::subscription::SubscriptionEventPayload::WorkerProtocol {
                            event: protocol::Event::Completions { .. },
                            ..
                        },
                        ..
                    }
                ) if subscription_id == second_protocol_subscription_id
            ) {
                break;
            }
        }
        let resume = protocol::subscription::SubscriptionFrame::new(
            protocol::subscription::SubscriptionFramePayload::WorkerProtocol(
                protocol::subscription::SubscriptionWorkerProtocolMethod {
                    subscription_id: second_protocol_subscription_id,
                    method: protocol::Method::Resume,
                },
            ),
        );
        socket
            .send(Message::Text(
                serde_json::to_string(&resume).unwrap().into(),
            ))
            .await
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                if execution_backend
                    .protocol_methods()
                    .iter()
                    .any(|(worker_ref, method)| {
                        worker_ref.worker_id.to_string() == worker_id
                            && matches!(method, protocol::Method::Resume)
                    })
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("Resume method should reach the Runtime execution backend");
        let protocol_methods = execution_backend.protocol_methods();
        assert!(protocol_methods.iter().any(|(worker_ref, method)| {
            worker_ref.worker_id.to_string() == worker_id
                && matches!(method, protocol::Method::Resume)
        }));
        server.abort();
    }

    #[tokio::test]
    async fn scoped_flow_source_route_persists_compiled_dcdl() {
        let temp = tempfile::tempdir().unwrap();
        let app = test_app(temp.path()).await;
        let workspace_id = test_identity().workspace_id;
        let source = r#"{
            schema_version = 1;
            name = "route-flow";
            initial = "work";
            states = {
                work = {
                    instructions = "Do the work.";
                    transitions = {
                        done = {
                            target = "done";
                            condition = "The work is complete.";
                        };
                    };
                };
                done = { instructions = ""; terminal = true; };
            };
        }"#;
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri(format!("/api/w/{workspace_id}/flows"))
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "path": "flows/route-flow.dcdl",
                            "content": source,
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!("/api/w/{workspace_id}/flows"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri(format!("/api/w/{workspace_id}/flows"))
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "path": "flows/broken.dcdl",
                            "content": "{ schema_version = 1; }",
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn passkey_registration_rejects_unverified_credential_response() {
        let dir = tempfile::tempdir().unwrap();
        let app = test_app(dir.path()).await;

        let registration_options = post_json(
            app.clone(),
            "/api/auth/passkeys/registration/options",
            json!({
                "handle": "alice",
                "display_name": "Alice"
            }),
        )
        .await;
        assert!(registration_options["public_key"].is_object());
        let challenge_id = registration_options["challenge_id"].as_str().unwrap();

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/auth/passkeys/registration/complete")
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&json!({
                            "challenge_id": challenge_id,
                            "credential": {
                                "id": "credential-1",
                                "rawId": "Y3JlZGVudGlhbC0x",
                                "type": "public-key",
                                "response": {
                                    "clientDataJSON": "e30",
                                    "attestationObject": "e30"
                                }
                            }
                        }))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_ne!(response.status(), StatusCode::OK);

        let login_without_registered_passkey = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/auth/passkeys/login/options")
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&json!({ "handle": "alice" })).unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_ne!(login_without_registered_passkey.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn objective_mutation_endpoints_round_trip_through_workspace_authority() {
        let dir = tempfile::tempdir().unwrap();
        let config = test_server_config(dir.path());
        let store = Arc::new(SqliteWorkspaceStore::open(&config.database_path).unwrap());
        store
            .upsert_workspace(&WorkspaceRecord {
                workspace_id: TEST_WORKSPACE_ID.to_string(),
                owner_account_id: None,
                display_name: "Test Workspace".to_string(),
                state: "active".to_string(),
                created_at: TEST_CREATED_AT.to_string(),
                updated_at: TEST_CREATED_AT.to_string(),
            })
            .await
            .unwrap();
        rusqlite::Connection::open(&config.database_path)
            .unwrap()
            .execute_batch(
                r#"
INSERT INTO typed_tickets (
    workspace_id, ticket_id, slug, title, status, kind, priority, body,
    workflow_state, workflow_state_explicit
) VALUES
    ('0192f0e8-4d84-7d6e-a000-000000000001', '00000000001J2', 'ticket-j2', 'Ticket J2', 'open', 'task', 'normal', '', 'planning', 1),
    ('0192f0e8-4d84-7d6e-a000-000000000001', '00000000001J3', 'ticket-j3', 'Ticket J3', 'open', 'task', 'normal', '', 'planning', 1);
INSERT INTO workspace_resource_keys (
    workspace_id, resource_kind, resource_id, sequence, resource_key, allocated_at
) VALUES
    ('0192f0e8-4d84-7d6e-a000-000000000001', 'ticket', '00000000001J2', 1, 'T-1', '2026-01-01T00:00:00Z'),
    ('0192f0e8-4d84-7d6e-a000-000000000001', 'ticket', '00000000001J3', 2, 'T-2', '2026-01-01T00:00:00Z');
INSERT INTO workspace_resource_key_counters (workspace_id, resource_kind, next_sequence)
VALUES ('0192f0e8-4d84-7d6e-a000-000000000001', 'ticket', 3);
"#,
            )
            .unwrap();
        let api = WorkspaceApi::new_with_execution_backend(
            config,
            store,
            Arc::new(DeterministicExecutionBackend::default()),
        )
        .await
        .unwrap();
        let app = build_inner_router(api);
        let objectives_path = format!("/api/w/{TEST_WORKSPACE_ID}/objectives");

        let created = request_json(
            app.clone(),
            "POST",
            &objectives_path,
            Some(json!({
                "title": "Objective CRUD",
                "body_md": "First body",
                "state": "active",
                "linked_tickets": ["00000000001J2"]
            })),
            StatusCode::OK,
        )
        .await;
        let id = created["id"].as_str().unwrap().to_string();
        assert_eq!(created["title"], "Objective CRUD");
        assert_eq!(created["linked_tickets"], json!(["00000000001J2"]));

        let edited = request_json(
            app.clone(),
            "PATCH",
            &format!("{objectives_path}/{id}"),
            Some(json!({
                "title": "Objective CRUD updated",
                "old_string": "First",
                "new_string": "Updated"
            })),
            StatusCode::OK,
        )
        .await;
        assert_eq!(edited["title"], "Objective CRUD updated");
        assert_eq!(edited["body"], "Updated body");

        let state = request_json(
            app.clone(),
            "POST",
            &format!("{objectives_path}/{id}/state"),
            Some(json!({ "state": "paused" })),
            StatusCode::OK,
        )
        .await;
        assert_eq!(state["state"], "paused");

        let linked = request_json(
            app.clone(),
            "POST",
            &format!("{objectives_path}/{id}/ticket-links"),
            Some(json!({ "ticket_id": "00000000001J3" })),
            StatusCode::OK,
        )
        .await;
        assert_eq!(
            linked["linked_tickets"],
            json!(["00000000001J2", "00000000001J3"])
        );

        let unlinked = request_json(
            app.clone(),
            "DELETE",
            &format!("{objectives_path}/{id}/ticket-links/00000000001J2"),
            None,
            StatusCode::OK,
        )
        .await;
        assert_eq!(unlinked["linked_tickets"], json!(["00000000001J3"]));

        let shown = get_json(app, &format!("{objectives_path}/{id}")).await;
        assert_eq!(shown["title"], "Objective CRUD updated");
        assert_eq!(shown["state"], "paused");
        assert_eq!(shown["linked_tickets"], json!(["00000000001J3"]));
    }

    #[tokio::test]
    async fn profile_settings_are_read_only_virtual_config_projection() {
        let dir = tempfile::tempdir().unwrap();
        let app = test_app(dir.path()).await;
        let path = format!("/api/w/{TEST_WORKSPACE_ID}/settings/profiles");
        let settings = get_json(app.clone(), &path).await;
        assert_eq!(settings["default_profile"], "builtin:companion");
        assert_eq!(settings["config_revision"], 1);
        assert!(settings["tree_digest"].as_str().is_some());
        assert!(settings["projection_digest"].as_str().is_some());
        assert!(
            settings["profiles"]
                .as_array()
                .unwrap()
                .iter()
                .all(|profile| profile["editable"] == false)
        );
        let mutation = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(&path)
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(mutation.status(), StatusCode::METHOD_NOT_ALLOWED);
    }

    #[test]
    fn workspace_workdir_response_serializes_shared_occupied_contract() {
        let response = BrowserWorkingDirectoryListResponse {
            workspace_id: TEST_WORKSPACE_ID.to_string(),
            items: vec![WorkingDirectorySummary {
                working_directory_id: "wd-1".to_string(),
                repository_id: "main".to_string(),
                creation_selector: None,
                creation_ref: None,
                current_selector: Some("work/ticket".to_string()),
                current_ref: Some("abc123".to_string()),
                materializer_kind: MaterializerKind::LocalGitWorktree,
                cleanup_target: None,
                status: WorkingDirectoryStatusKind::Active,
                cleanliness: Some("clean".to_string()),
                primary_worker_id: None,
                occupied_by: Some(WorkingDirectoryOccupancy {
                    worker: RuntimeWorkerRef::new("arcadia", "worker-opaque-64"),
                    display_name: "Coder".to_string(),
                    linked_at: "2026-08-12T00:00:00Z".to_string(),
                }),
            }],
            diagnostics: vec![],
        };

        let value = serde_json::to_value(response).unwrap();
        assert_eq!(
            value["items"][0]["occupied_by"]["worker_id"],
            "worker-opaque-64"
        );
        assert!(
            value["items"][0]["occupied_by"]
                .get("runtime_worker_id")
                .is_none()
        );
    }

    async fn get_json_authenticated(app: Router, uri: &str, token: &str) -> Value {
        let response = app
            .oneshot(
                Request::builder()
                    .uri(uri)
                    .header(axum::http::header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{uri}");
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
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
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(
            status,
            expected_status,
            "{method} {uri}: {}",
            String::from_utf8_lossy(&bytes)
        );
        serde_json::from_slice(&bytes).unwrap_or_else(
            |_| serde_json::json!({ "message": String::from_utf8_lossy(&bytes).to_string() }),
        )
    }

    async fn request_json_authenticated(
        app: Router,
        method: &str,
        uri: &str,
        body: Option<Value>,
        token: &str,
        expected_status: StatusCode,
    ) -> Value {
        let mut builder = Request::builder()
            .method(method)
            .uri(uri)
            .header(axum::http::header::AUTHORIZATION, format!("Bearer {token}"));
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
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(
            status,
            expected_status,
            "{method} {uri}: {}",
            String::from_utf8_lossy(&bytes)
        );
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

    fn write_ticket(
        database_path: &Path,
        workspace_id: &str,
        title: &str,
        state: ticket::TicketWorkflowState,
    ) -> String {
        use ticket::TicketBackend as _;

        let backend = ticket::SqliteTicketBackend::open(database_path, workspace_id).unwrap();
        let mut input = ticket::NewTicket::new(title);
        input.workflow_state = Some(state);
        backend.create(input).unwrap().id
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
        std::fs::write(
            objective_dir.join("memory-architecture-overview.md"),
            "# Memory architecture\n\nResource body.\n",
        )
        .unwrap();
    }
}
