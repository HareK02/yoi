//! Shared Workspace HTTP resource contracts.
//!
//! This crate owns transport DTOs exposed by the Workspace Server and consumed
//! by Rust clients. Runtime-internal projections remain in their owning crates;
//! callers must explicitly construct these Server-authoritative resources.

use std::collections::BTreeMap;

use api_macros::api;
pub use api_macros::axum as server_support;
pub use api_macros::reqwest as client_support;
pub use api_macros::{ApiContract, HttpMethod};
pub mod repository_openapi_typescript;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use webauthn_rs_proto::{
    CreationChallengeResponse, PublicKeyCredential, RegisterPublicKeyCredential,
    RequestChallengeResponse,
};

#[allow(dead_code)]
#[derive(JsonSchema)]
struct ServerJsonSafeU64(#[schemars(range(min = 0, max = 9_007_199_254_740_991_u64))] u64);
type JsonSafeU64Pair = [ServerJsonSafeU64; 2];

pub type ServerApiError = runtime_api::RuntimeApiError;
pub type ServerApiClientError = client_support::ClientError<ServerApiError>;

/// Error body shared by the existing repository routes and their generated adapters.
#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RepositoryApiError {
    pub error: String,
    pub message: String,
    #[serde(default)]
    pub diagnostics: Vec<Diagnostic>,
    #[serde(skip, default = "default_repository_error_status")]
    status: u16,
}

const fn default_repository_error_status() -> u16 {
    400
}

impl RepositoryApiError {
    pub fn new(
        status: u16,
        error: impl Into<String>,
        message: impl Into<String>,
        diagnostics: Vec<Diagnostic>,
    ) -> Self {
        Self {
            error: error.into(),
            message: message.into(),
            diagnostics,
            status,
        }
    }
}

impl api_macros::HttpError for RepositoryApiError {
    fn status_code(&self) -> u16 {
        self.status
    }
}

impl api_macros::HttpRequestError for RepositoryApiError {
    fn from_request_rejection(status: u16, message: String) -> Self {
        let reason = match status {
            413 => "Payload Too Large",
            415 => "Unsupported Media Type",
            422 => "Unprocessable Entity",
            _ => "Bad Request",
        };
        Self::new(status, reason, message, Vec::new())
    }
}

macro_rules! impl_openapi_schema {
    ($($ty:ty),+ $(,)?) => {
        $(impl api_macros::openapi::OpenApiSchema for $ty {})+
    };
}

impl_openapi_schema!(
    AuthBootstrapUserRequest,
    AuthPublicConfig,
    AuthUserResponse,
    DeviceLoginApproveRequest,
    DeviceLoginApproveResponse,
    DeviceLoginPollRequest,
    DeviceLoginPollResponse,
    DeviceLoginStartRequest,
    DeviceLoginStartResponse,
    HealthResponse,
    PasskeyLoginOptionsRequest,
    PasskeyLoginOptionsResponse,
    PasskeyRegistrationOptionsRequest,
    PasskeyRegistrationOptionsResponse,
    WhoamiResponse,
    WorkspaceCreateRequest,
    WorkspaceCreateResponse,
    WorkspaceDeletionOperationResponse,
    WorkspaceDeletionPreflightResponse,
    WorkspaceDeletionRequest,
    WorkspaceListQuery,
    WorkspaceCatalogListResponse,
    CreateWorkspaceRepositoryRequest,
    CreateWorkspaceRepositoryResponse,
    RepositoryApiError,
    RepositoryListResponse,
    RepositoryDetailResponse,
    WorkspaceResponse,
    WorkspaceMetadataSettingsResponse,
    UpdateWorkspaceMetadataRequest,
    WorkspaceMetadataMutationResponse,
    WorkspaceSigningIdentityResponse,
    WorkspaceMemorySettings,
    UpdateWorkspaceMemorySettingsRequest,
    RepositoryAccessProjection,
    RepositorySshCredentialListResponse,
    RepositorySshCredential,
    CreateRepositorySshCredentialRequest,
    GenerateRepositorySshCredentialRequest,
    RepositorySshPublicKey,
    RotateRepositorySshCredentialRequest,
    DeleteRepositorySshCredentialRequest,
    RepositorySshHostTrustListResponse,
    RepositorySshHostTrust,
    PutRepositorySshHostTrustRequest,
    RepositorySshHostTrustMutationResponse,
    DeleteRepositorySshHostTrustRequest,
    WorkspaceConfigTreeResponse,
    WorkspacePromptProjection,
    ConfigCommitRequest,
    ConfigTreeSnapshot,
    ConfigEntry,
    ProfileSettingsResponse,
    FlowSourceListResponse,
    FlowSourceRecord,
    PutFlowRequest,
    FlowSourceResolveRequest,
    ResolvedFlowSource,
    MemoryDocumentResponse,
    MemoryStagingQuery,
    MemoryStagingListResponse,
    MemoryBackendRequest,
    MemoryBackendResponse,
    MemoryConsolidateStagingRequest,
    MemoryConsolidationResponse,
    SkillCatalogResponse,
    SkillDetailResponse,
    SkillActivationResponse,
    BrowserAppendTicketEventRequest,
    BrowserCloseTicketRequest,
    BrowserEditTicketRequest,
    BrowserQueueTicketRequest,
    BrowserTransitionTicketStateRequest,
    CancelTicketImplementationRequest,
    ClearTicketRoleAssignmentQuery,
    CompleteMergeRequestRequest,
    CreateTicketOrchestrationPlanRequest,
    CreateTicketRecordRequest,
    CreateTicketRelationRequest,
    DefaultIntakeReadyBodyRequest,
    EditTicketRecordItemRequest,
    MergeEvent,
    MergeRequestDetailResponse,
    MergeRequestListQuery,
    MergeRequestListResponse,
    MergeRequestReadinessResponse,
    MergeRequestThreadQuery,
    MergeRequestThreadResponse,
    ObjectiveCreateRequest,
    ObjectiveDetail,
    ObjectiveEditRequest,
    ObjectiveLinkTicketRequest,
    ObjectiveListQuery,
    ObjectiveListResponse,
    ObjectiveQueryRequest,
    ObjectiveQueryResponse,
    ObjectiveShowRequest,
    ObjectiveStateRequest,
    OpenMergeRequestRequest,
    PublicMergeRequest,
    RegisterMergeRequestReviewCapabilityRequest,
    RegisterReviewerChildSessionRequest,
    RepairMergeRequestSelectorRequest,
    ReviewEvent,
    ReviewRevokedEvent,
    RevokeMergeRequestReviewRequest,
    SetTicketRoleAssignmentRequest,
    SubmitMergeRequestReviewRequest,
    TextResponse,
    TicketCloseRecordRequest,
    TicketDependencyCheckResponse,
    TicketDetail,
    TicketDoctorResponse,
    TicketIntakeSummaryRequest,
    TicketListHttpQuery,
    TicketListResponse,
    TicketMarkReadyRequest,
    TicketOrchestrationPlanRecord,
    TicketOrchestrationPlanRecordList,
    TicketOrchestrationPlanSearchRequest,
    TicketQueryRequest,
    TicketQueryResponse,
    TicketQueueResponse,
    TicketRecord,
    TicketRecordRef,
    TicketRecordSummaryList,
    TicketRelationRecord,
    TicketRelationRecordList,
    TicketRelationRecordView,
    TicketRelationRemoveRequest,
    TicketRelationSearchRequest,
    TicketRoleAssignmentMutationResponse,
    TicketRoleAssignmentsResponse,
    TicketShowRequest,
    TicketStateChangeRequest,
    TicketSummarySearchQuery,
    TicketThreadEventRequest,
);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceWorkerSessionResponse {
    pub subject: WorkspaceWorkerSubject,
    #[serde(flatten)]
    pub observation: runtime_api::WorkerSessionAvailability,
}

#[api(reqwest, axum, openapi)]
pub trait ServerApi {
    #[get("/health", status = 200, error_status = 400)]
    async fn health(&self) -> Result<HealthResponse, RepositoryApiError>;

    #[get("/api/auth/config", status = 200, error_status = 400)]
    async fn auth_config(&self) -> Result<AuthPublicConfig, RepositoryApiError>;

    #[post(
        "/api/auth/bootstrap-user",
        status = 200,
        error_status = 400,
        additional_error_statuses = [500]
    )]
    async fn auth_bootstrap_user(
        &self,
        #[body] request: AuthBootstrapUserRequest,
    ) -> Result<AuthUserResponse, RepositoryApiError>;

    #[post(
        "/api/auth/passkeys/registration/options",
        status = 200,
        error_status = 400,
        additional_error_statuses = [500, 502]
    )]
    async fn auth_passkey_registration_options(
        &self,
        #[extension] context: ServerRequestContext,
        #[body] request: PasskeyRegistrationOptionsRequest,
    ) -> Result<PasskeyRegistrationOptionsResponse, RepositoryApiError>;

    #[post(
        "/api/auth/passkeys/login/options",
        status = 200,
        error_status = 400,
        additional_error_statuses = [500, 502]
    )]
    async fn auth_passkey_login_options(
        &self,
        #[extension] context: ServerRequestContext,
        #[body] request: PasskeyLoginOptionsRequest,
    ) -> Result<PasskeyLoginOptionsResponse, RepositoryApiError>;

    #[post(
        "/api/auth/device-login/start",
        status = 200,
        error_status = 400,
        additional_error_statuses = [500, 502]
    )]
    async fn auth_device_login_start(
        &self,
        #[body] request: DeviceLoginStartRequest,
    ) -> Result<DeviceLoginStartResponse, RepositoryApiError>;

    #[post(
        "/api/auth/device-login/approve",
        status = 200,
        error_status = 400,
        additional_error_statuses = [401, 403, 500, 502],
        bearer_auth = true,
        browser_auth = true
    )]
    async fn auth_device_login_approve(
        &self,
        #[extension] context: ServerRequestContext,
        #[body] request: DeviceLoginApproveRequest,
    ) -> Result<DeviceLoginApproveResponse, RepositoryApiError>;

    #[post(
        "/api/auth/device-login/poll",
        status = 200,
        error_status = 400,
        additional_error_statuses = [500, 502]
    )]
    async fn auth_device_login_poll(
        &self,
        #[body] request: DeviceLoginPollRequest,
    ) -> Result<DeviceLoginPollResponse, RepositoryApiError>;

    #[get("/api/auth/whoami", status = 200, error_status = 400)]
    async fn auth_whoami(
        &self,
        #[extension] context: ServerRequestContext,
    ) -> Result<WhoamiResponse, RepositoryApiError>;

    #[get(
        "/api/workspaces",
        status = 200,
        error_status = 400,
        additional_error_statuses = [401, 403, 500],
        bearer_auth = true,
        browser_auth = true
    )]
    async fn workspace_catalog_list(
        &self,
        #[extension] context: ServerRequestContext,
        #[query] query: WorkspaceListQuery,
    ) -> Result<WorkspaceCatalogListResponse, RepositoryApiError>;

    #[post(
        "/api/workspaces",
        status = 201,
        alternate_status = 200,
        error_status = 400,
        additional_error_statuses = [401, 403, 409, 500],
        bearer_auth = true,
        browser_auth = true
    )]
    async fn workspace_catalog_create(
        &self,
        #[extension] context: ServerRequestContext,
        #[body] request: WorkspaceCreateRequest,
    ) -> Result<WorkspaceCreateResponse, RepositoryApiError>;

    #[get(
        "/api/workspaces/{workspace_id}/deletion",
        status = 200,
        error_status = 400,
        additional_error_statuses = [401, 403, 404, 500],
        bearer_auth = true,
        browser_auth = true
    )]
    async fn workspace_deletion_preflight(
        &self,
        #[extension] context: ServerRequestContext,
        #[path] workspace_id: String,
    ) -> Result<WorkspaceDeletionPreflightResponse, RepositoryApiError>;

    #[post(
        "/api/workspaces/{workspace_id}/deletion",
        status = 202,
        alternate_status = 200,
        error_status = 400,
        additional_error_statuses = [401, 403, 404, 409, 500],
        bearer_auth = true,
        browser_auth = true
    )]
    async fn workspace_deletion_start(
        &self,
        #[extension] context: ServerRequestContext,
        #[path] workspace_id: String,
        #[body] request: WorkspaceDeletionRequest,
    ) -> Result<WorkspaceDeletionOperationResponse, RepositoryApiError>;

    #[get(
        "/api/workspace-deletions/{operation_id}",
        status = 200,
        error_status = 404,
        additional_error_statuses = [400, 401, 403, 500],
        bearer_auth = true,
        browser_auth = true
    )]
    async fn workspace_deletion_get(
        &self,
        #[extension] context: ServerRequestContext,
        #[path] operation_id: String,
    ) -> Result<WorkspaceDeletionOperationResponse, RepositoryApiError>;

    #[get("/api/workspace", status = 200, error_status = 400)]
    async fn workspace_current(
        &self,
        #[extension] context: ServerRequestContext,
    ) -> Result<WorkspaceResponse, RepositoryApiError>;

    #[get(
        "/api/w/{workspace_id}/workspace",
        status = 200,
        error_status = 400,
        additional_error_statuses = [401, 403, 404, 500],
        bearer_auth = true,
        browser_auth = true
    )]
    async fn workspace_scoped(
        &self,
        #[extension] context: ServerRequestContext,
        #[path] workspace_id: String,
    ) -> Result<WorkspaceResponse, RepositoryApiError>;

    #[get(
        "/api/w/{workspace_id}/settings",
        status = 200,
        error_status = 400,
        additional_error_statuses = [401, 403, 404, 500],
        bearer_auth = true,
        browser_auth = true
    )]
    async fn workspace_metadata_settings(
        &self,
        #[path] workspace_id: String,
    ) -> Result<WorkspaceMetadataSettingsResponse, RepositoryApiError>;

    #[put(
        "/api/w/{workspace_id}/settings",
        status = 200,
        error_status = 400,
        additional_error_statuses = [401, 403, 404, 409, 500],
        bearer_auth = true,
        browser_auth = true
    )]
    async fn workspace_metadata_settings_update(
        &self,
        #[path] workspace_id: String,
        #[body] request: UpdateWorkspaceMetadataRequest,
    ) -> Result<WorkspaceMetadataMutationResponse, RepositoryApiError>;

    #[get(
        "/api/w/{workspace_id}/settings/signing-identity",
        status = 200,
        error_status = 400,
        additional_error_statuses = [401, 403, 404, 500],
        bearer_auth = true,
        browser_auth = true
    )]
    async fn workspace_signing_identity(
        &self,
        #[extension] actor: RequestActor,
        #[path] workspace_id: String,
    ) -> Result<WorkspaceSigningIdentityResponse, RepositoryApiError>;

    #[post(
        "/api/w/{workspace_id}/settings/signing-identity/provision",
        status = 200,
        error_status = 400,
        additional_error_statuses = [401, 403, 404, 409, 500],
        bearer_auth = true,
        browser_auth = true
    )]
    async fn workspace_signing_identity_provision(
        &self,
        #[extension] actor: RequestActor,
        #[path] workspace_id: String,
    ) -> Result<WorkspaceSigningIdentityResponse, RepositoryApiError>;

    #[get(
        "/api/w/{workspace_id}/settings/memory",
        status = 200,
        error_status = 400,
        additional_error_statuses = [401, 403, 404, 500],
        bearer_auth = true,
        browser_auth = true
    )]
    async fn workspace_memory_settings(
        &self,
        #[path] workspace_id: String,
    ) -> Result<WorkspaceMemorySettings, RepositoryApiError>;

    #[put(
        "/api/w/{workspace_id}/settings/memory",
        status = 200,
        error_status = 400,
        additional_error_statuses = [401, 403, 404, 409, 500],
        bearer_auth = true,
        browser_auth = true
    )]
    async fn workspace_memory_settings_update(
        &self,
        #[path] workspace_id: String,
        #[body] request: UpdateWorkspaceMemorySettingsRequest,
    ) -> Result<WorkspaceMemorySettings, RepositoryApiError>;

    #[get(
        "/api/w/{workspace_id}/settings/repository-access",
        status = 200,
        error_status = 400,
        additional_error_statuses = [401, 403, 404, 500],
        bearer_auth = true,
        browser_auth = true
    )]
    async fn repository_access_projection(
        &self,
        #[extension] actor: RequestActor,
        #[path] workspace_id: String,
    ) -> Result<RepositoryAccessProjection, RepositoryApiError>;

    #[get(
        "/api/w/{workspace_id}/settings/repository-access/credentials",
        status = 200,
        error_status = 400,
        additional_error_statuses = [401, 403, 404, 500],
        bearer_auth = true,
        browser_auth = true
    )]
    async fn repository_ssh_credential_list(
        &self,
        #[extension] actor: RequestActor,
        #[path] workspace_id: String,
    ) -> Result<RepositorySshCredentialListResponse, RepositoryApiError>;

    #[post(
        "/api/w/{workspace_id}/settings/repository-access/credentials",
        status = 201,
        error_status = 400,
        additional_error_statuses = [401, 403, 404, 409, 500],
        bearer_auth = true,
        browser_auth = true
    )]
    async fn repository_ssh_credential_create(
        &self,
        #[extension] actor: RequestActor,
        #[path] workspace_id: String,
        #[body] request: CreateRepositorySshCredentialRequest,
    ) -> Result<RepositorySshCredential, RepositoryApiError>;

    #[post(
        "/api/w/{workspace_id}/settings/repository-access/credentials/generate",
        status = 201,
        error_status = 400,
        additional_error_statuses = [401, 403, 404, 409, 500],
        bearer_auth = true,
        browser_auth = true
    )]
    async fn repository_ssh_credential_generate(
        &self,
        #[extension] actor: RequestActor,
        #[path] workspace_id: String,
        #[body] request: GenerateRepositorySshCredentialRequest,
    ) -> Result<RepositorySshCredential, RepositoryApiError>;

    #[get(
        "/api/w/{workspace_id}/settings/repository-access/credentials/{credential_id}",
        status = 200,
        error_status = 404,
        additional_error_statuses = [400, 401, 403, 500],
        bearer_auth = true,
        browser_auth = true
    )]
    async fn repository_ssh_credential_get(
        &self,
        #[extension] actor: RequestActor,
        #[path] workspace_id: String,
        #[path] credential_id: String,
    ) -> Result<RepositorySshCredential, RepositoryApiError>;

    #[delete(
        "/api/w/{workspace_id}/settings/repository-access/credentials/{credential_id}",
        status = 204,
        error_status = 400,
        additional_error_statuses = [401, 403, 404, 409, 500],
        bearer_auth = true,
        browser_auth = true
    )]
    async fn repository_ssh_credential_delete(
        &self,
        #[extension] actor: RequestActor,
        #[path] workspace_id: String,
        #[path] credential_id: String,
        #[body] request: DeleteRepositorySshCredentialRequest,
    ) -> Result<(), RepositoryApiError>;

    #[get(
        "/api/w/{workspace_id}/settings/repository-access/credentials/{credential_id}/public-key",
        status = 200,
        error_status = 404,
        additional_error_statuses = [400, 401, 403, 500],
        bearer_auth = true,
        browser_auth = true
    )]
    async fn repository_ssh_credential_public_key(
        &self,
        #[extension] actor: RequestActor,
        #[path] workspace_id: String,
        #[path] credential_id: String,
    ) -> Result<RepositorySshPublicKey, RepositoryApiError>;

    #[post(
        "/api/w/{workspace_id}/settings/repository-access/credentials/{credential_id}/rotate",
        status = 200,
        error_status = 400,
        additional_error_statuses = [401, 403, 404, 409, 500],
        bearer_auth = true,
        browser_auth = true
    )]
    async fn repository_ssh_credential_rotate(
        &self,
        #[extension] actor: RequestActor,
        #[path] workspace_id: String,
        #[path] credential_id: String,
        #[body] request: RotateRepositorySshCredentialRequest,
    ) -> Result<RepositorySshCredential, RepositoryApiError>;

    #[get(
        "/api/w/{workspace_id}/settings/repository-access/host-trusts",
        status = 200,
        error_status = 400,
        additional_error_statuses = [401, 403, 404, 500],
        bearer_auth = true,
        browser_auth = true
    )]
    async fn repository_ssh_host_trust_list(
        &self,
        #[extension] actor: RequestActor,
        #[path] workspace_id: String,
    ) -> Result<RepositorySshHostTrustListResponse, RepositoryApiError>;

    #[post(
        "/api/w/{workspace_id}/settings/repository-access/host-trusts",
        status = 201,
        alternate_status = 200,
        error_status = 400,
        additional_error_statuses = [401, 403, 404, 409, 500],
        bearer_auth = true,
        browser_auth = true
    )]
    async fn repository_ssh_host_trust_put(
        &self,
        #[extension] actor: RequestActor,
        #[path] workspace_id: String,
        #[body] request: PutRepositorySshHostTrustRequest,
    ) -> Result<RepositorySshHostTrustMutationResponse, RepositoryApiError>;

    #[get(
        "/api/w/{workspace_id}/settings/repository-access/host-trusts/{host_trust_id}",
        status = 200,
        error_status = 404,
        additional_error_statuses = [400, 401, 403, 500],
        bearer_auth = true,
        browser_auth = true
    )]
    async fn repository_ssh_host_trust_get(
        &self,
        #[extension] actor: RequestActor,
        #[path] workspace_id: String,
        #[path] host_trust_id: String,
    ) -> Result<RepositorySshHostTrust, RepositoryApiError>;

    #[delete(
        "/api/w/{workspace_id}/settings/repository-access/host-trusts/{host_trust_id}",
        status = 204,
        error_status = 400,
        additional_error_statuses = [401, 403, 404, 409, 500],
        bearer_auth = true,
        browser_auth = true
    )]
    async fn repository_ssh_host_trust_delete(
        &self,
        #[extension] actor: RequestActor,
        #[path] workspace_id: String,
        #[path] host_trust_id: String,
        #[body] request: DeleteRepositorySshHostTrustRequest,
    ) -> Result<(), RepositoryApiError>;

    #[get(
        "/api/w/{workspace_id}/config/source-tree",
        status = 200,
        error_status = 400,
        additional_error_statuses = [401, 403, 404, 500],
        bearer_auth = true,
        browser_auth = true
    )]
    async fn workspace_config_tree(
        &self,
        #[path] workspace_id: String,
    ) -> Result<WorkspaceConfigTreeResponse, RepositoryApiError>;

    #[get(
        "/api/w/{workspace_id}/config/projections/prompts",
        status = 200,
        error_status = 400,
        additional_error_statuses = [401, 403, 404, 500]
    )]
    async fn workspace_prompt_projection(
        &self,
        #[path] workspace_id: String,
    ) -> Result<WorkspacePromptProjection, RepositoryApiError>;

    #[post(
        "/api/w/{workspace_id}/config/source-tree/commit",
        status = 201,
        error_status = 400,
        additional_error_statuses = [401, 403, 404, 409, 500],
        bearer_auth = true,
        browser_auth = true
    )]
    async fn workspace_config_tree_commit(
        &self,
        #[path] workspace_id: String,
        #[body] request: ConfigCommitRequest,
    ) -> Result<WorkspaceConfigTreeResponse, RepositoryApiError>;

    #[get(
        "/api/w/{workspace_id}/config/source-tree/revisions/{revision}",
        status = 200,
        error_status = 404,
        additional_error_statuses = [400, 401, 403, 500],
        bearer_auth = true,
        browser_auth = true
    )]
    async fn workspace_config_revision(
        &self,
        #[path] workspace_id: String,
        #[path] revision: String,
    ) -> Result<ConfigTreeSnapshot, RepositoryApiError>;

    #[get(
        "/api/w/{workspace_id}/config/source-tree/entries/{path}",
        status = 200,
        error_status = 404,
        additional_error_statuses = [400, 401, 403, 500],
        bearer_auth = true,
        browser_auth = true
    )]
    async fn workspace_config_entry(
        &self,
        #[path] workspace_id: String,
        #[path] path: String,
    ) -> Result<ConfigEntry, RepositoryApiError>;

    #[get(
        "/api/w/{workspace_id}/settings/profiles",
        status = 200,
        error_status = 400,
        additional_error_statuses = [401, 403, 404, 500],
        bearer_auth = true,
        browser_auth = true
    )]
    async fn profile_settings(
        &self,
        #[path] workspace_id: String,
    ) -> Result<ProfileSettingsResponse, RepositoryApiError>;

    #[get(
        "/api/w/{workspace_id}/flows",
        status = 200,
        error_status = 400,
        additional_error_statuses = [401, 403, 404, 500]
    )]
    async fn flow_list(
        &self,
        #[path] workspace_id: String,
    ) -> Result<FlowSourceListResponse, RepositoryApiError>;

    #[put(
        "/api/w/{workspace_id}/flows",
        status = 200,
        error_status = 400,
        additional_error_statuses = [401, 403, 404, 409, 500]
    )]
    async fn flow_put(
        &self,
        #[path] workspace_id: String,
        #[body] request: PutFlowRequest,
    ) -> Result<FlowSourceRecord, RepositoryApiError>;

    #[post(
        "/api/w/{workspace_id}/flows/resolve",
        status = 200,
        error_status = 400,
        additional_error_statuses = [401, 403, 404, 500]
    )]
    async fn flow_resolve(
        &self,
        #[path] workspace_id: String,
        #[body] request: FlowSourceResolveRequest,
    ) -> Result<ResolvedFlowSource, RepositoryApiError>;

    #[get(
        "/api/w/{workspace_id}/flows/{flow_id}",
        status = 200,
        error_status = 404,
        additional_error_statuses = [400, 401, 403, 500]
    )]
    async fn flow_get(
        &self,
        #[path] workspace_id: String,
        #[path] flow_id: String,
    ) -> Result<FlowSourceRecord, RepositoryApiError>;

    #[get(
        "/api/w/{workspace_id}/memory",
        status = 200,
        error_status = 400,
        additional_error_statuses = [401, 403, 404, 500],
        bearer_auth = true,
        browser_auth = true
    )]
    async fn memory_document(
        &self,
        #[path] workspace_id: String,
    ) -> Result<MemoryDocumentResponse, RepositoryApiError>;

    #[get(
        "/api/w/{workspace_id}/memory/staging",
        status = 200,
        error_status = 400,
        additional_error_statuses = [401, 403, 404, 500],
        bearer_auth = true,
        browser_auth = true
    )]
    async fn memory_staging_list(
        &self,
        #[path] workspace_id: String,
        #[query] query: MemoryStagingQuery,
    ) -> Result<MemoryStagingListResponse, RepositoryApiError>;

    #[post(
        "/api/w/{workspace_id}/memory/backend",
        status = 200,
        error_status = 400,
        additional_error_statuses = [401, 403, 404, 500]
    )]
    async fn memory_backend(
        &self,
        #[path] workspace_id: String,
        #[body] request: MemoryBackendRequest,
    ) -> Result<MemoryBackendResponse, RepositoryApiError>;

    #[post(
        "/api/w/{workspace_id}/memory/consolidation",
        status = 200,
        error_status = 400,
        additional_error_statuses = [401, 403, 404, 409, 500, 502, 503]
    )]
    async fn memory_consolidation(
        &self,
        #[path] workspace_id: String,
        #[body] request: MemoryConsolidateStagingRequest,
    ) -> Result<MemoryConsolidationResponse, RepositoryApiError>;

    #[get(
        "/api/w/{workspace_id}/skills",
        status = 200,
        error_status = 400,
        additional_error_statuses = [401, 403, 404, 500],
        bearer_auth = true,
        browser_auth = true
    )]
    async fn skill_list(
        &self,
        #[path] workspace_id: String,
    ) -> Result<SkillCatalogResponse, RepositoryApiError>;

    #[get(
        "/api/w/{workspace_id}/skills/lint",
        status = 200,
        error_status = 400,
        additional_error_statuses = [401, 403, 404, 500],
        bearer_auth = true,
        browser_auth = true
    )]
    async fn skill_lint(
        &self,
        #[path] workspace_id: String,
    ) -> Result<SkillCatalogResponse, RepositoryApiError>;

    #[get(
        "/api/w/{workspace_id}/skills/{name}",
        status = 200,
        error_status = 404,
        additional_error_statuses = [400, 401, 403, 500],
        bearer_auth = true,
        browser_auth = true
    )]
    async fn skill_get(
        &self,
        #[path] workspace_id: String,
        #[path] name: String,
    ) -> Result<SkillDetailResponse, RepositoryApiError>;

    #[get(
        "/api/w/{workspace_id}/skills/{name}/activate",
        status = 200,
        error_status = 404,
        additional_error_statuses = [400, 401, 403, 409, 500]
    )]
    async fn skill_activate(
        &self,
        #[path] workspace_id: String,
        #[path] name: String,
    ) -> Result<SkillActivationResponse, RepositoryApiError>;

    #[get(
        "/api/w/{workspace_id}/runtimes/{runtime_id}/workers/{worker_id}/session",
        status = 200,
        error_status = 400,
        openapi = false
    )]
    async fn worker_session(
        &self,
        #[path] workspace_id: String,
        #[path] runtime_id: String,
        #[path] worker_id: String,
    ) -> Result<WorkspaceWorkerSessionResponse, ServerApiError>;

    #[get(
        "/api/w/{workspace_id}/repositories",
        status = 200,
        error_status = 400,
        additional_error_statuses = [401, 403, 404, 500],
        bearer_auth = true,
        browser_auth = true
    )]
    async fn repository_list(
        &self,
        #[path] workspace_id: String,
    ) -> Result<RepositoryListResponse, RepositoryApiError>;

    #[get(
        "/api/repositories",
        status = 200,
        error_status = 400,
        additional_error_statuses = [401, 403, 404, 500],
        bearer_auth = true,
        browser_auth = true
    )]
    async fn repository_list_alias(&self) -> Result<RepositoryListResponse, RepositoryApiError>;

    #[get(
        "/api/w/{workspace_id}/repositories/{repository_key}",
        status = 200,
        error_status = 404,
        additional_error_statuses = [400, 401, 403, 500],
        bearer_auth = true,
        browser_auth = true
    )]
    async fn repository_detail(
        &self,
        #[path] workspace_id: String,
        #[path] repository_key: String,
    ) -> Result<RepositoryDetailResponse, RepositoryApiError>;

    #[get(
        "/api/repositories/{repository_key}",
        status = 200,
        error_status = 404,
        additional_error_statuses = [400, 401, 403, 500],
        bearer_auth = true,
        browser_auth = true
    )]
    async fn repository_detail_alias(
        &self,
        #[path] repository_key: String,
    ) -> Result<RepositoryDetailResponse, RepositoryApiError>;

    #[post(
        "/api/w/{workspace_id}/repositories",
        status = 201,
        alternate_status = 200,
        error_status = 400,
        additional_error_statuses = [401, 403, 404, 409, 413, 415, 422, 500],
        bearer_auth = true,
        browser_auth = true,
        normalize_body_errors = true
    )]
    async fn repository_create(
        &self,
        #[extension] actor: RequestActor,
        #[path] workspace_id: String,
        #[body] request: CreateWorkspaceRepositoryRequest,
    ) -> Result<CreateWorkspaceRepositoryResponse, RepositoryApiError>;

    #[get("/api/tickets", status = 200, error_status = 400, additional_error_statuses = [500])]
    async fn ticket_list_alias(
        &self,
        #[query] query: TicketListHttpQuery,
    ) -> Result<TicketListResponse, RepositoryApiError>;

    #[get("/api/w/{workspace_id}/tickets", status = 200, error_status = 400, additional_error_statuses = [401, 403, 404, 500], bearer_auth = true, browser_auth = true)]
    async fn ticket_list(
        &self,
        #[path] workspace_id: String,
        #[query] query: TicketListHttpQuery,
    ) -> Result<TicketListResponse, RepositoryApiError>;

    #[post("/api/w/{workspace_id}/tickets", status = 200, error_status = 400, additional_error_statuses = [401, 403, 404, 409, 500], bearer_auth = true, browser_auth = true)]
    async fn ticket_create_record(
        &self,
        #[extension] context: ServerRequestContext,
        #[path] workspace_id: String,
        #[body] request: CreateTicketRecordRequest,
    ) -> Result<TicketRecordRef, RepositoryApiError>;

    #[post("/api/w/{workspace_id}/tickets/query", status = 200, error_status = 400, additional_error_statuses = [401, 403, 404, 500], bearer_auth = true, browser_auth = true)]
    async fn ticket_query(
        &self,
        #[path] workspace_id: String,
        #[body] request: TicketQueryRequest,
    ) -> Result<TicketQueryResponse, RepositoryApiError>;

    #[get("/api/tickets/{id}", status = 200, error_status = 404, additional_error_statuses = [400, 500])]
    async fn ticket_get_alias(
        &self,
        #[path] id: String,
    ) -> Result<TicketDetail, RepositoryApiError>;

    #[post("/api/w/{workspace_id}/tickets/default-intake-ready-body", status = 200, error_status = 400, additional_error_statuses = [401, 403, 404, 500])]
    async fn ticket_default_intake_ready_body(
        &self,
        #[extension] context: ServerRequestContext,
        #[path] workspace_id: String,
        #[body] request: DefaultIntakeReadyBodyRequest,
    ) -> Result<TextResponse, RepositoryApiError>;

    #[get("/api/w/{workspace_id}/tickets/search", status = 200, error_status = 400, additional_error_statuses = [401, 403, 404, 500])]
    async fn ticket_summary_search(
        &self,
        #[extension] context: ServerRequestContext,
        #[path] workspace_id: String,
        #[query] query: TicketSummarySearchQuery,
    ) -> Result<TicketRecordSummaryList, RepositoryApiError>;

    #[get("/api/w/{workspace_id}/tickets/doctor", status = 200, error_status = 400, additional_error_statuses = [401, 403, 404, 500])]
    async fn ticket_doctor(
        &self,
        #[extension] context: ServerRequestContext,
        #[path] workspace_id: String,
    ) -> Result<TicketDoctorResponse, RepositoryApiError>;

    #[post("/api/w/{workspace_id}/tickets/relations/search", status = 200, error_status = 400, additional_error_statuses = [401, 403, 404, 500])]
    async fn ticket_relation_query(
        &self,
        #[extension] context: ServerRequestContext,
        #[path] workspace_id: String,
        #[body] request: TicketRelationSearchRequest,
    ) -> Result<TicketRelationRecordList, RepositoryApiError>;

    #[post("/api/w/{workspace_id}/tickets/orchestration-plans/search", status = 200, error_status = 400, additional_error_statuses = [401, 403, 404, 500])]
    async fn ticket_orchestration_plan_query(
        &self,
        #[extension] context: ServerRequestContext,
        #[path] workspace_id: String,
        #[body] request: TicketOrchestrationPlanSearchRequest,
    ) -> Result<TicketOrchestrationPlanRecordList, RepositoryApiError>;

    #[get("/api/w/{workspace_id}/tickets/{id}/record", status = 200, error_status = 404, additional_error_statuses = [400, 401, 403, 500])]
    async fn ticket_record_get(
        &self,
        #[extension] context: ServerRequestContext,
        #[path] workspace_id: String,
        #[path] id: String,
    ) -> Result<TicketRecord, RepositoryApiError>;

    #[patch("/api/w/{workspace_id}/tickets/{id}/item", status = 200, error_status = 400, additional_error_statuses = [401, 403, 404, 409, 500])]
    async fn ticket_record_item_edit(
        &self,
        #[extension] context: ServerRequestContext,
        #[path] workspace_id: String,
        #[path] id: String,
        #[body] request: EditTicketRecordItemRequest,
    ) -> Result<TicketRecord, RepositoryApiError>;

    #[get("/api/w/{workspace_id}/tickets/{id}/dependency-check", status = 200, error_status = 400, additional_error_statuses = [401, 403, 404, 500])]
    async fn ticket_dependency_check(
        &self,
        #[extension] context: ServerRequestContext,
        #[path] workspace_id: String,
        #[path] id: String,
    ) -> Result<TicketDependencyCheckResponse, RepositoryApiError>;

    #[post("/api/w/{workspace_id}/tickets/{id}/thread-events", status = 204, error_status = 400, additional_error_statuses = [401, 403, 404, 409, 500])]
    async fn ticket_thread_event_add(
        &self,
        #[extension] context: ServerRequestContext,
        #[path] workspace_id: String,
        #[path] id: String,
        #[body] request: TicketThreadEventRequest,
    ) -> Result<(), RepositoryApiError>;

    #[post("/api/w/{workspace_id}/tickets/{id}/state-changes", status = 204, error_status = 400, additional_error_statuses = [401, 403, 404, 409, 500])]
    async fn ticket_state_change_add(
        &self,
        #[extension] context: ServerRequestContext,
        #[path] workspace_id: String,
        #[path] id: String,
        #[body] request: TicketStateChangeRequest,
    ) -> Result<(), RepositoryApiError>;

    #[post("/api/w/{workspace_id}/tickets/{id}/intake-summaries", status = 204, error_status = 400, additional_error_statuses = [401, 403, 404, 409, 500])]
    async fn ticket_intake_summary_add(
        &self,
        #[extension] context: ServerRequestContext,
        #[path] workspace_id: String,
        #[path] id: String,
        #[body] request: TicketIntakeSummaryRequest,
    ) -> Result<(), RepositoryApiError>;

    #[post("/api/w/{workspace_id}/tickets/{id}/state-fields/{field}", status = 204, error_status = 400, additional_error_statuses = [401, 403, 404, 409, 500])]
    async fn ticket_state_field_set(
        &self,
        #[extension] context: ServerRequestContext,
        #[path] workspace_id: String,
        #[path] id: String,
        #[path] field: String,
        #[body] request: TicketStateChangeRequest,
    ) -> Result<(), RepositoryApiError>;

    #[post("/api/w/{workspace_id}/tickets/{id}/workflow-state", status = 204, error_status = 400, additional_error_statuses = [401, 403, 404, 409, 500])]
    async fn ticket_workflow_state_set(
        &self,
        #[extension] context: ServerRequestContext,
        #[path] workspace_id: String,
        #[path] id: String,
        #[body] request: TicketStateChangeRequest,
    ) -> Result<(), RepositoryApiError>;

    #[post("/api/w/{workspace_id}/tickets/{id}/workflow/mark-ready", status = 200, error_status = 400, additional_error_statuses = [401, 403, 404, 409, 500])]
    async fn ticket_mark_ready_record(
        &self,
        #[extension] context: ServerRequestContext,
        #[path] workspace_id: String,
        #[path] id: String,
        #[body] request: TicketMarkReadyRequest,
    ) -> Result<TicketRecord, RepositoryApiError>;

    #[post("/api/w/{workspace_id}/tickets/{id}/workflow/queue", status = 200, error_status = 400, additional_error_statuses = [401, 403, 404, 409, 500])]
    async fn ticket_queue_record(
        &self,
        #[extension] context: ServerRequestContext,
        #[path] workspace_id: String,
        #[path] id: String,
    ) -> Result<TicketQueueResponse, RepositoryApiError>;

    #[get("/api/w/{workspace_id}/merge-requests", status = 200, error_status = 400, additional_error_statuses = [401, 403, 404, 500], bearer_auth = true, browser_auth = true)]
    async fn merge_request_list(
        &self,
        #[path] workspace_id: String,
        #[query] query: MergeRequestListQuery,
    ) -> Result<MergeRequestListResponse, RepositoryApiError>;

    #[get("/api/w/{workspace_id}/merge-requests/{merge_request_id}", status = 200, error_status = 404, additional_error_statuses = [400, 401, 403, 500], bearer_auth = true, browser_auth = true)]
    async fn merge_request_show(
        &self,
        #[path] workspace_id: String,
        #[path] merge_request_id: String,
        #[query] query: MergeRequestThreadQuery,
    ) -> Result<MergeRequestDetailResponse, RepositoryApiError>;

    #[post("/api/w/{workspace_id}/tickets/{id}/merge-request", status = 200, error_status = 400, additional_error_statuses = [401, 403, 404, 409, 500])]
    async fn merge_request_open(
        &self,
        #[extension] context: ServerRequestContext,
        #[path] workspace_id: String,
        #[path] id: String,
        #[body] request: OpenMergeRequestRequest,
    ) -> Result<PublicMergeRequest, RepositoryApiError>;

    #[get("/api/w/{workspace_id}/tickets/{id}/merge-request/readiness", status = 200, error_status = 400, additional_error_statuses = [401, 403, 404, 409, 500])]
    async fn merge_request_readiness(
        &self,
        #[path] workspace_id: String,
        #[path] id: String,
    ) -> Result<MergeRequestReadinessResponse, RepositoryApiError>;

    #[get("/api/w/{workspace_id}/tickets/{id}/merge-request/thread", status = 200, error_status = 400, additional_error_statuses = [401, 403, 404, 500])]
    async fn merge_request_thread(
        &self,
        #[path] workspace_id: String,
        #[path] id: String,
        #[query] query: MergeRequestThreadQuery,
    ) -> Result<MergeRequestThreadResponse, RepositoryApiError>;

    #[post("/api/w/{workspace_id}/tickets/{id}/merge-request/repair-source", status = 200, error_status = 400, additional_error_statuses = [401, 403, 404, 409, 500], bearer_auth = true, browser_auth = true)]
    async fn merge_request_selector_repair(
        &self,
        #[extension] context: ServerRequestContext,
        #[path] workspace_id: String,
        #[path] id: String,
        #[body] request: RepairMergeRequestSelectorRequest,
    ) -> Result<PublicMergeRequest, RepositoryApiError>;

    #[post("/api/w/{workspace_id}/internal/reviewer-child-sessions", status = 204, error_status = 400, additional_error_statuses = [401, 403, 404, 409, 500])]
    async fn merge_request_reviewer_child_register(
        &self,
        #[extension] context: ServerRequestContext,
        #[path] workspace_id: String,
        #[body] request: RegisterReviewerChildSessionRequest,
    ) -> Result<(), RepositoryApiError>;

    #[post("/api/w/{workspace_id}/tickets/{id}/merge-request/review-capabilities", status = 204, error_status = 400, additional_error_statuses = [401, 403, 404, 409, 500])]
    async fn merge_request_review_capability_register(
        &self,
        #[extension] context: ServerRequestContext,
        #[path] workspace_id: String,
        #[path] id: String,
        #[body] request: RegisterMergeRequestReviewCapabilityRequest,
    ) -> Result<(), RepositoryApiError>;

    #[post("/api/w/{workspace_id}/tickets/{id}/merge-request/reviews", status = 200, error_status = 400, additional_error_statuses = [401, 403, 404, 409, 500])]
    async fn merge_request_review_submit(
        &self,
        #[path] workspace_id: String,
        #[path] id: String,
        #[body] request: SubmitMergeRequestReviewRequest,
    ) -> Result<ReviewEvent, RepositoryApiError>;

    #[post("/api/w/{workspace_id}/tickets/{id}/merge-request/reviews/revoke", status = 200, error_status = 400, additional_error_statuses = [401, 403, 404, 409, 500])]
    async fn merge_request_review_revoke(
        &self,
        #[extension] context: ServerRequestContext,
        #[path] workspace_id: String,
        #[path] id: String,
        #[body] request: RevokeMergeRequestReviewRequest,
    ) -> Result<ReviewRevokedEvent, RepositoryApiError>;

    #[post("/api/w/{workspace_id}/tickets/{id}/merge-request/complete", status = 200, error_status = 400, additional_error_statuses = [401, 403, 404, 409, 500])]
    async fn merge_request_complete(
        &self,
        #[extension] context: ServerRequestContext,
        #[path] workspace_id: String,
        #[path] id: String,
        #[body] request: CompleteMergeRequestRequest,
    ) -> Result<MergeEvent, RepositoryApiError>;

    #[post("/api/w/{workspace_id}/tickets/{id}/workflow/close", status = 204, error_status = 400, additional_error_statuses = [401, 403, 404, 409, 500])]
    async fn ticket_close_record(
        &self,
        #[extension] context: ServerRequestContext,
        #[path] workspace_id: String,
        #[path] id: String,
        #[body] request: TicketCloseRecordRequest,
    ) -> Result<(), RepositoryApiError>;

    #[get("/api/w/{workspace_id}/tickets/{id}/relation-view", status = 200, error_status = 400, additional_error_statuses = [401, 403, 404, 500])]
    async fn ticket_relation_view(
        &self,
        #[extension] context: ServerRequestContext,
        #[path] workspace_id: String,
        #[path] id: String,
    ) -> Result<TicketRelationRecordView, RepositoryApiError>;

    #[post("/api/w/{workspace_id}/tickets/{id}/relations", status = 200, error_status = 400, additional_error_statuses = [401, 403, 404, 409, 500])]
    async fn ticket_relation_record(
        &self,
        #[extension] context: ServerRequestContext,
        #[path] workspace_id: String,
        #[path] id: String,
        #[body] request: CreateTicketRelationRequest,
    ) -> Result<TicketRelationRecord, RepositoryApiError>;

    #[delete("/api/w/{workspace_id}/tickets/{id}/relations", status = 200, error_status = 400, additional_error_statuses = [401, 403, 404, 409, 500])]
    async fn ticket_relation_remove(
        &self,
        #[extension] context: ServerRequestContext,
        #[path] workspace_id: String,
        #[path] id: String,
        #[body] request: TicketRelationRemoveRequest,
    ) -> Result<TicketRelationRecord, RepositoryApiError>;

    #[post("/api/w/{workspace_id}/tickets/{id}/orchestration-plans", status = 200, error_status = 400, additional_error_statuses = [401, 403, 404, 409, 500])]
    async fn ticket_orchestration_plan_record(
        &self,
        #[extension] context: ServerRequestContext,
        #[path] workspace_id: String,
        #[path] id: String,
        #[body] request: CreateTicketOrchestrationPlanRequest,
    ) -> Result<TicketOrchestrationPlanRecord, RepositoryApiError>;

    #[get("/api/w/{workspace_id}/tickets/{id}", status = 200, error_status = 404, additional_error_statuses = [400, 401, 403, 500], bearer_auth = true, browser_auth = true)]
    async fn ticket_get(
        &self,
        #[path] workspace_id: String,
        #[path] id: String,
    ) -> Result<TicketDetail, RepositoryApiError>;

    #[patch("/api/w/{workspace_id}/tickets/{id}", status = 200, error_status = 400, additional_error_statuses = [401, 403, 404, 409, 500], bearer_auth = true, browser_auth = true)]
    async fn ticket_edit(
        &self,
        #[path] workspace_id: String,
        #[path] id: String,
        #[body] request: BrowserEditTicketRequest,
    ) -> Result<TicketDetail, RepositoryApiError>;

    #[post("/api/w/{workspace_id}/tickets/{id}/show", status = 200, error_status = 400, additional_error_statuses = [401, 403, 404, 500])]
    async fn ticket_show(
        &self,
        #[path] workspace_id: String,
        #[path] id: String,
        #[body] request: TicketShowRequest,
    ) -> Result<TicketDetail, RepositoryApiError>;

    #[get("/api/w/{workspace_id}/tickets/{id}/assignments", status = 200, error_status = 400, additional_error_statuses = [401, 403, 404, 500], bearer_auth = true, browser_auth = true)]
    async fn ticket_assignment_list(
        &self,
        #[path] workspace_id: String,
        #[path] id: String,
    ) -> Result<TicketRoleAssignmentsResponse, RepositoryApiError>;

    #[put("/api/w/{workspace_id}/tickets/{id}/assignments/{role}", status = 200, error_status = 400, additional_error_statuses = [401, 403, 404, 409, 500], bearer_auth = true, browser_auth = true)]
    async fn ticket_assignment_set(
        &self,
        #[path] workspace_id: String,
        #[path] id: String,
        #[path] role: String,
        #[body] request: SetTicketRoleAssignmentRequest,
    ) -> Result<TicketRoleAssignmentMutationResponse, RepositoryApiError>;

    #[delete("/api/w/{workspace_id}/tickets/{id}/assignments/{role}", status = 200, error_status = 400, additional_error_statuses = [401, 403, 404, 409, 500], bearer_auth = true, browser_auth = true)]
    async fn ticket_assignment_clear(
        &self,
        #[path] workspace_id: String,
        #[path] id: String,
        #[path] role: String,
        #[query] query: ClearTicketRoleAssignmentQuery,
    ) -> Result<TicketRoleAssignmentMutationResponse, RepositoryApiError>;

    #[post("/api/w/{workspace_id}/tickets/{id}/implementation-cancellations", status = 200, error_status = 400, additional_error_statuses = [401, 403, 404, 409, 500], bearer_auth = true, browser_auth = true)]
    async fn ticket_implementation_cancel(
        &self,
        #[path] workspace_id: String,
        #[path] id: String,
        #[body] request: CancelTicketImplementationRequest,
    ) -> Result<TicketDetail, RepositoryApiError>;

    #[post("/api/w/{workspace_id}/tickets/{id}/state", status = 200, error_status = 400, additional_error_statuses = [401, 403, 404, 409, 500], bearer_auth = true, browser_auth = true)]
    async fn ticket_state_transition(
        &self,
        #[path] workspace_id: String,
        #[path] id: String,
        #[body] request: BrowserTransitionTicketStateRequest,
    ) -> Result<TicketDetail, RepositoryApiError>;

    #[post("/api/w/{workspace_id}/tickets/{id}/ready", status = 200, error_status = 400, additional_error_statuses = [401, 403, 404, 409, 500], bearer_auth = true, browser_auth = true)]
    async fn ticket_ready(
        &self,
        #[path] workspace_id: String,
        #[path] id: String,
        #[body] request: TicketMarkReadyRequest,
    ) -> Result<TicketDetail, RepositoryApiError>;

    #[post("/api/w/{workspace_id}/tickets/{id}/events", status = 200, error_status = 400, additional_error_statuses = [401, 403, 404, 409, 500], bearer_auth = true, browser_auth = true)]
    async fn ticket_event_append(
        &self,
        #[path] workspace_id: String,
        #[path] id: String,
        #[body] request: BrowserAppendTicketEventRequest,
    ) -> Result<TicketDetail, RepositoryApiError>;

    #[post("/api/w/{workspace_id}/tickets/{id}/queue", status = 200, error_status = 400, additional_error_statuses = [401, 403, 404, 409, 500], bearer_auth = true, browser_auth = true)]
    async fn ticket_queue(
        &self,
        #[path] workspace_id: String,
        #[path] id: String,
        #[body] request: BrowserQueueTicketRequest,
    ) -> Result<TicketQueueResponse, RepositoryApiError>;

    #[post("/api/w/{workspace_id}/tickets/{id}/close", status = 200, error_status = 400, additional_error_statuses = [401, 403, 404, 409, 500], bearer_auth = true, browser_auth = true)]
    async fn ticket_close(
        &self,
        #[path] workspace_id: String,
        #[path] id: String,
        #[body] request: BrowserCloseTicketRequest,
    ) -> Result<TicketDetail, RepositoryApiError>;

    #[get("/api/objectives", status = 200, error_status = 400, additional_error_statuses = [500])]
    async fn objective_list_alias(
        &self,
        #[query] query: ObjectiveListQuery,
    ) -> Result<ObjectiveListResponse, RepositoryApiError>;

    #[get("/api/w/{workspace_id}/objectives", status = 200, error_status = 400, additional_error_statuses = [401, 403, 404, 500], bearer_auth = true, browser_auth = true)]
    async fn objective_list(
        &self,
        #[path] workspace_id: String,
        #[query] query: ObjectiveListQuery,
    ) -> Result<ObjectiveListResponse, RepositoryApiError>;

    #[post("/api/w/{workspace_id}/objectives", status = 200, error_status = 400, additional_error_statuses = [401, 403, 404, 409, 500], bearer_auth = true, browser_auth = true)]
    async fn objective_create(
        &self,
        #[path] workspace_id: String,
        #[body] request: ObjectiveCreateRequest,
    ) -> Result<ObjectiveDetail, RepositoryApiError>;

    #[post("/api/w/{workspace_id}/objectives/query", status = 200, error_status = 400, additional_error_statuses = [401, 403, 404, 500])]
    async fn objective_query(
        &self,
        #[path] workspace_id: String,
        #[body] request: ObjectiveQueryRequest,
    ) -> Result<ObjectiveQueryResponse, RepositoryApiError>;

    #[get("/api/objectives/{id}", status = 200, error_status = 404, additional_error_statuses = [400, 500])]
    async fn objective_get_alias(
        &self,
        #[path] id: String,
    ) -> Result<ObjectiveDetail, RepositoryApiError>;

    #[get("/api/w/{workspace_id}/objectives/{objective_id}", status = 200, error_status = 404, additional_error_statuses = [400, 401, 403, 500], bearer_auth = true, browser_auth = true)]
    async fn objective_get(
        &self,
        #[path] workspace_id: String,
        #[path] objective_id: String,
    ) -> Result<ObjectiveDetail, RepositoryApiError>;

    #[patch("/api/w/{workspace_id}/objectives/{objective_id}", status = 200, error_status = 400, additional_error_statuses = [401, 403, 404, 409, 500], bearer_auth = true, browser_auth = true)]
    async fn objective_edit(
        &self,
        #[path] workspace_id: String,
        #[path] objective_id: String,
        #[body] request: ObjectiveEditRequest,
    ) -> Result<ObjectiveDetail, RepositoryApiError>;

    #[post("/api/w/{workspace_id}/objectives/{objective_id}/show", status = 200, error_status = 400, additional_error_statuses = [401, 403, 404, 500])]
    async fn objective_show(
        &self,
        #[path] workspace_id: String,
        #[path] objective_id: String,
        #[body] request: ObjectiveShowRequest,
    ) -> Result<ObjectiveDetail, RepositoryApiError>;

    #[post("/api/w/{workspace_id}/objectives/{objective_id}/state", status = 200, error_status = 400, additional_error_statuses = [401, 403, 404, 409, 500], bearer_auth = true, browser_auth = true)]
    async fn objective_state_set(
        &self,
        #[path] workspace_id: String,
        #[path] objective_id: String,
        #[body] request: ObjectiveStateRequest,
    ) -> Result<ObjectiveDetail, RepositoryApiError>;

    #[post("/api/w/{workspace_id}/objectives/{objective_id}/ticket-links", status = 200, error_status = 400, additional_error_statuses = [401, 403, 404, 409, 500], bearer_auth = true, browser_auth = true)]
    async fn objective_ticket_link(
        &self,
        #[path] workspace_id: String,
        #[path] objective_id: String,
        #[body] request: ObjectiveLinkTicketRequest,
    ) -> Result<ObjectiveDetail, RepositoryApiError>;

    #[delete("/api/w/{workspace_id}/objectives/{objective_id}/ticket-links/{ticket_id}", status = 200, error_status = 400, additional_error_statuses = [401, 403, 404, 409, 500], bearer_auth = true, browser_auth = true)]
    async fn objective_ticket_unlink(
        &self,
        #[path] workspace_id: String,
        #[path] objective_id: String,
        #[path] ticket_id: String,
    ) -> Result<ObjectiveDetail, RepositoryApiError>;
}

/// Digest of the fully rendered canonical contract with its digest slot normalized.
pub fn canonical_openapi_source_digest() -> Result<String, api_macros::openapi::OpenApiError> {
    const NORMALIZED_DIGEST: &str =
        "sha256:0000000000000000000000000000000000000000000000000000000000000000";
    let normalized = server_api_openapi(api_macros::openapi::OpenApiInfo {
        title: "Yoi Server API",
        version: env!("CARGO_PKG_VERSION"),
        source_digest: NORMALIZED_DIGEST,
    })?
    .to_json()?;
    let digest = Sha256::digest(normalized.as_bytes());
    let mut encoded = String::with_capacity("sha256:".len() + digest.len() * 2);
    encoded.push_str("sha256:");
    for byte in digest {
        use std::fmt::Write as _;
        write!(&mut encoded, "{byte:02x}").expect("writing to String cannot fail");
    }
    Ok(encoded)
}

/// Build the deployment-independent canonical ServerApi OpenAPI document.
pub fn canonical_openapi_document()
-> Result<api_macros::openapi::OpenApiDocument, api_macros::openapi::OpenApiError> {
    let source_digest = canonical_openapi_source_digest()?;
    server_api_openapi(api_macros::openapi::OpenApiInfo {
        title: "Yoi Server API",
        version: env!("CARGO_PKG_VERSION"),
        source_digest: &source_digest,
    })
}

/// Server-local request context populated by authentication middleware.
///
/// Generated clients and OpenAPI omit extension parameters; operation implementations use this
/// value instead of re-parsing transport headers inside domain handlers.
#[derive(Clone)]
pub struct ServerRequestContext {
    pub actor: Option<RequestActor>,
    pub origin: Option<String>,
    /// Transport headers retained for Server-side authentication adapters.
    ///
    /// Generated clients and OpenAPI do not expose this extension. Values stay byte-exact so the
    /// Workspace Server can reuse the existing Runtime source-proof and browser-session authority
    /// while generated Axum routes replace only the handwritten route registration.
    pub transport_headers: Vec<(String, Vec<u8>)>,
}

impl std::fmt::Debug for ServerRequestContext {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ServerRequestContext")
            .field("actor", &self.actor)
            .field("origin", &self.origin)
            .field("transport_header_count", &self.transport_headers.len())
            .finish()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HealthResponse {
    pub status: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceListQuery {
    pub limit: Option<u32>,
}

/// Public browser-authentication configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct AuthPublicConfig {
    pub rp_id: String,
    pub origin: String,
    pub public_base_url: String,
    pub cookie_name: String,
}

/// Authentication method that established the current request actor.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum ActorAuthMethod {
    BrowserSession,
    ApiToken,
}

/// Public user identity returned by authentication operations.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct AuthenticatedUser {
    pub user_id: String,
    pub account_id: String,
    pub handle: String,
    pub display_name: String,
}

/// Authenticated actor returned by `GET /api/auth/whoami`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct RequestActor {
    pub user_id: String,
    pub account_id: String,
    pub handle: String,
    pub display_name: String,
    pub auth_method: ActorAuthMethod,
}

impl RequestActor {
    pub fn user(&self) -> AuthenticatedUser {
        AuthenticatedUser {
            user_id: self.user_id.clone(),
            account_id: self.account_id.clone(),
            handle: self.handle.clone(),
            display_name: self.display_name.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WhoamiResponse {
    pub actor: Option<RequestActor>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct AuthBootstrapUserRequest {
    pub handle: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional = nullable))]
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct AuthUserResponse {
    pub user: AuthenticatedUser,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct PasskeyRegistrationOptionsRequest {
    pub handle: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional = nullable))]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional = nullable))]
    pub browser_origin: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct PasskeyRegistrationOptionsResponse {
    pub challenge_id: String,
    #[cfg_attr(feature = "typescript", ts(type = "unknown"))]
    #[schemars(with = "serde_json::Value")]
    pub public_key: CreationChallengeResponse,
}

#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct PasskeyRegistrationCompleteRequest {
    pub challenge_id: String,
    #[cfg_attr(feature = "typescript", ts(type = "unknown"))]
    pub credential: RegisterPublicKeyCredential,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct PasskeyLoginOptionsRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional = nullable))]
    pub handle: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional = nullable))]
    pub browser_origin: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct PasskeyLoginOptionsResponse {
    pub challenge_id: String,
    #[cfg_attr(feature = "typescript", ts(type = "unknown"))]
    #[schemars(with = "serde_json::Value")]
    pub public_key: RequestChallengeResponse,
}

#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct PasskeyLoginCompleteRequest {
    pub challenge_id: String,
    #[cfg_attr(feature = "typescript", ts(type = "unknown"))]
    pub credential: PublicKeyCredential,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct DeviceLoginStartRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional = nullable))]
    pub client_name: Option<String>,
}

const DEVICE_LOGIN_EXPIRES_IN_MAX_SECONDS: u64 = 24 * 60 * 60;
const DEVICE_LOGIN_POLL_INTERVAL_MAX_SECONDS: u64 = 60;

#[derive(Clone, Serialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct DeviceLoginStartResponse {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: String,
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    #[schemars(range(min = 1, max = 86400))]
    pub expires_in: u64,
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    #[schemars(range(min = 1, max = 60))]
    pub interval: u64,
}

impl<'de> Deserialize<'de> for DeviceLoginStartResponse {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            device_code: String,
            user_code: String,
            verification_uri: String,
            verification_uri_complete: String,
            expires_in: u64,
            interval: u64,
        }

        let wire = Wire::deserialize(deserializer)?;
        if wire.expires_in == 0 || wire.expires_in > DEVICE_LOGIN_EXPIRES_IN_MAX_SECONDS {
            return Err(serde::de::Error::custom(format!(
                "expires_in must be between 1 and {DEVICE_LOGIN_EXPIRES_IN_MAX_SECONDS}"
            )));
        }
        if wire.interval == 0 || wire.interval > DEVICE_LOGIN_POLL_INTERVAL_MAX_SECONDS {
            return Err(serde::de::Error::custom(format!(
                "interval must be between 1 and {DEVICE_LOGIN_POLL_INTERVAL_MAX_SECONDS}"
            )));
        }
        Ok(Self {
            device_code: wire.device_code,
            user_code: wire.user_code,
            verification_uri: wire.verification_uri,
            verification_uri_complete: wire.verification_uri_complete,
            expires_in: wire.expires_in,
            interval: wire.interval,
        })
    }
}

impl std::fmt::Debug for DeviceLoginStartResponse {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DeviceLoginStartResponse")
            .field("device_code", &"[redacted]")
            .field("user_code", &self.user_code)
            .field("verification_uri", &self.verification_uri)
            .field("verification_uri_complete", &self.verification_uri_complete)
            .field("expires_in", &self.expires_in)
            .field("interval", &self.interval)
            .finish()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct DeviceLoginApproveRequest {
    pub user_code: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum DeviceLoginApprovalStatus {
    Approved,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct DeviceLoginApproveResponse {
    pub status: DeviceLoginApprovalStatus,
    pub user: AuthenticatedUser,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct DeviceLoginPollRequest {
    pub device_code: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub enum DeviceAccessTokenType {
    Bearer,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum DeviceLoginPollStatus {
    Pending,
    Approved,
    Expired,
    Denied,
    Consumed,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct DeviceLoginPollResponse {
    pub status: DeviceLoginPollStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional = nullable))]
    pub access_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional = nullable))]
    pub token_type: Option<DeviceAccessTokenType>,
}

impl std::fmt::Debug for DeviceLoginPollResponse {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DeviceLoginPollResponse")
            .field("status", &self.status)
            .field(
                "access_token",
                &self.access_token.as_ref().map(|_| "[redacted]"),
            )
            .field("token_type", &self.token_type)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum LogoutStatus {
    LoggedOut,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct LogoutResponse {
    pub status: LogoutStatus,
}

pub const REPOSITORY_KEY_MIN_LEN: usize = 1;
pub const REPOSITORY_KEY_MAX_LEN: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepositoryKeyError {
    Length,
    Character,
    LeadingHyphen,
    TrailingHyphen,
}

impl std::fmt::Display for RepositoryKeyError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Length => "must contain between 1 and 64 ASCII bytes",
            Self::Character => "must contain only lowercase ASCII letters, digits, and hyphens",
            Self::LeadingHyphen => "must not start with a hyphen",
            Self::TrailingHyphen => "must not end with a hyphen",
        })
    }
}

impl std::error::Error for RepositoryKeyError {}

/// Validate one immutable Workspace-scoped Repository key.
///
/// Keys are deliberately not normalized: callers must submit the exact canonical
/// lowercase ASCII spelling so idempotency and route identity cannot alias.
pub fn validate_repository_key(value: &str) -> Result<(), RepositoryKeyError> {
    let bytes = value.as_bytes();
    if !(REPOSITORY_KEY_MIN_LEN..=REPOSITORY_KEY_MAX_LEN).contains(&bytes.len()) {
        return Err(RepositoryKeyError::Length);
    }
    if bytes[0] == b'-' {
        return Err(RepositoryKeyError::LeadingHyphen);
    }
    if bytes[bytes.len() - 1] == b'-' {
        return Err(RepositoryKeyError::TrailingHyphen);
    }
    if !bytes
        .iter()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
    {
        return Err(RepositoryKeyError::Character);
    }
    Ok(())
}

/// Provider-neutral classification of an authoritative Repository source.
///
/// Local paths remain distinct from network Git transports so callers cannot
/// accidentally treat an unmaterialized remote as a server-local filesystem path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum RepositorySourceKind {
    LocalPath,
    File,
    Ssh,
    Https,
    /// A legacy value that could not be classified during migration. It remains
    /// inspectable but every provider operation must fail closed. Historical
    /// `http` wire values decode into this non-executable classification.
    Invalid,
}

impl RepositorySourceKind {
    pub const fn is_remote(self) -> bool {
        matches!(self, Self::Ssh | Self::Https)
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LocalPath => "local_path",
            Self::File => "file",
            Self::Ssh => "ssh",
            Self::Https => "https",
            Self::Invalid => "invalid",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "local_path" => Self::LocalPath,
            "file" => Self::File,
            "ssh" => Self::Ssh,
            "https" => Self::Https,
            "http" | "invalid" => Self::Invalid,
            _ => return None,
        })
    }
}

impl<'de> Deserialize<'de> for RepositorySourceKind {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).ok_or_else(|| {
            serde::de::Error::unknown_variant(
                &value,
                &["local_path", "file", "ssh", "https", "invalid"],
            )
        })
    }
}

/// Stable Repository source identity stored by Workspace authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct RepositorySource {
    pub kind: RepositorySourceKind,
    /// Canonical source representation. This is an absolute local path for
    /// `local_path`, and a normalized URI/remote specification otherwise.
    pub uri: String,
}

/// Browser/user intent for registering one Repository in a Workspace.
///
/// The Server parses and canonicalizes `source`; callers cannot assert a
/// transport classification or supply credential material through this DTO.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateWorkspaceRepositoryRequest {
    pub repository_key: String,
    pub source: String,
    #[serde(default)]
    pub default_ref: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct CreateWorkspaceRepositoryResponse {
    pub workspace_id: String,
    pub repository_key: String,
    pub replayed: bool,
}

impl api_macros::HttpSuccess for CreateWorkspaceRepositoryResponse {
    fn status_code(&self) -> u16 {
        if self.replayed { 200 } else { 201 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum RepositoryObservedStatus {
    Unverified,
    Ready,
    Invalid,
}

impl RepositoryObservedStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unverified => "unverified",
            Self::Ready => "ready",
            Self::Invalid => "invalid",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "unverified" => Self::Unverified,
            "ready" => Self::Ready,
            "invalid" => Self::Invalid,
            _ => return None,
        })
    }
}

/// Public Workspace catalog item returned by `GET /api/workspaces`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkspaceSummary {
    pub workspace_id: String,
    pub owner_account_id: String,
    pub display_name: String,
    pub state: String,
    pub created_at: String,
    pub updated_at: String,
}

/// Public response returned by `GET /api/workspaces`.
///
/// The transparent newtype keeps the established top-level JSON array while making the
/// complete list response a named cross-crate and generated-TypeScript authority.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct WorkspaceCatalogListResponse(pub Vec<WorkspaceSummary>);

/// Public Repository record embedded in Workspace creation responses.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkspaceRepositoryRecord {
    pub workspace_id: String,
    pub repository_key: String,
    pub kind: String,
    pub provider: Option<String>,
    pub source: RepositorySource,
    pub default_ref: Option<String>,
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_u64))]
    pub source_revision: u64,
    pub source_fingerprint: String,
    pub observed_status: RepositoryObservedStatus,
    pub observed_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Initial Repository registration intent for Workspace creation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InitialRepositoryIntent {
    pub repository_key: String,
    pub uri: String,
    #[serde(default)]
    pub default_ref: Option<String>,
}

/// Request for atomically creating a Workspace and its initial Repository.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceCreateRequest {
    pub operation_key: String,
    pub display_name: String,
    pub repository: InitialRepositoryIntent,
}

/// Response returned after atomically creating a Workspace and its first Repository.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkspaceCreateResponse {
    pub workspace: WorkspaceSummary,
    pub repository: WorkspaceRepositoryRecord,
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_u64))]
    pub config_revision: u64,
    pub request_fingerprint: String,
    pub replayed: bool,
}

impl api_macros::HttpSuccess for WorkspaceCreateResponse {
    fn status_code(&self) -> u16 {
        if self.replayed { 200 } else { 201 }
    }
}

/// Browser authentication configuration exposed by the scoped Workspace summary.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub enum WorkspaceAuthConfig {
    Passkey {
        rp_id: String,
        origin: String,
        public_base_url: String,
        cookie_name: String,
    },
}

/// Backend-authoritative permissions for the current Workspace actor.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkspacePermissionSummary {
    pub manage_repositories: bool,
    pub manage_secrets: bool,
    pub manage_runtimes: bool,
    pub delete_workspace: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkspaceExtensionPointState {
    pub status: String,
    pub note: String,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkspaceExtensionPoints {
    pub store: String,
    pub event_stream: WorkspaceExtensionPointState,
    pub host_worker_bridge: WorkspaceExtensionPointState,
    pub companion_console: WorkspaceExtensionPointState,
}

/// Scoped Workspace metadata and current-actor permission projection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkspaceResponse {
    pub workspace_id: String,
    pub display_name: String,
    pub record_authority: String,
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_i64))]
    pub schema_version: i64,
    pub auth: WorkspaceAuthConfig,
    pub permissions: WorkspacePermissionSummary,
    pub extension_points: WorkspaceExtensionPoints,
}

/// Workspace display metadata exposed from the Server DB settings authority.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkspaceMetadataSettingsResponse {
    pub workspace_id: String,
    pub display_name: String,
    pub created_at: String,
    pub revision: String,
    pub source: String,
    pub diagnostics: Vec<Diagnostic>,
}

/// Compare-and-swap update for Workspace identity display metadata.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct UpdateWorkspaceMetadataRequest {
    pub display_name: String,
    pub revision: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkspaceMetadataMutationResponse {
    pub workspace: WorkspaceMetadataSettingsResponse,
    pub diagnostics: Vec<Diagnostic>,
}

/// Lifecycle state for a Workspace-scoped Ed25519 signing identity.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceSigningIdentityState {
    PendingProvisioning,
    Active,
}

/// Public metadata for a Workspace signing identity. Private material and its
/// storage reference are deliberately not part of this wire authority.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkspaceSigningIdentityPublic {
    pub workspace_id: String,
    pub key_id: String,
    pub algorithm: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional))]
    pub public_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional))]
    pub public_key_fingerprint: Option<String>,
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_u64))]
    pub revision: u64,
    pub state: WorkspaceSigningIdentityState,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional))]
    pub provisioned_at: Option<String>,
}

/// Copyable public trust bundle consumed by future Runtime enrollment work.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkspacePublicIdentityBundle {
    pub workspace_id: String,
    pub backend_url: String,
    pub key_id: String,
    pub algorithm: String,
    pub public_key: String,
    pub public_key_fingerprint: String,
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_u64))]
    pub revision: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkspaceSigningIdentityResponse {
    pub identity: WorkspaceSigningIdentityPublic,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional))]
    pub public_bundle: Option<WorkspacePublicIdentityBundle>,
}

/// Source content kinds accepted by the Workspace configuration tree.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum ConfigContentType {
    Decodal,
    Text,
}

/// One source entry in the Workspace configuration tree.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct ConfigEntry {
    pub path: String,
    pub content_type: ConfigContentType,
    pub content: String,
    pub content_digest: String,
}

/// Immutable Workspace configuration tree snapshot.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct ConfigTreeSnapshot {
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_u64))]
    pub revision: u64,
    pub digest: String,
    pub entries: BTreeMap<String, ConfigEntry>,
}

/// Base-relative change applied by a Workspace configuration commit.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConfigTreeChange {
    Create {
        path: String,
        content_type: ConfigContentType,
        content: String,
    },
    Update {
        path: String,
        expected_digest: String,
        content: String,
    },
    Rename {
        from: String,
        to: String,
        expected_digest: String,
    },
    Delete {
        path: String,
        expected_digest: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConfigProjectionValidator {
    StaticTemplateCatalog {
        namespace: String,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        key_aliases: BTreeMap<String, String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct ConfigSchemaContribution {
    pub provider_id: String,
    pub namespace: String,
    pub version: String,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional))]
    pub projection_validator: Option<ConfigProjectionValidator>,
    pub source_digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkspaceConfigSchemaBundle {
    pub contributions: Vec<ConfigSchemaContribution>,
    pub source: String,
    pub fingerprint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct ToolchainContract {
    pub contract_version: u32,
    pub decodal_version: String,
    pub schema_version: u32,
    pub entrypoints: Vec<String>,
    pub import_policy_version: u32,
    pub schema_bundle: WorkspaceConfigSchemaBundle,
    pub fingerprint: String,
}

/// Workspace configuration tree plus the exact evaluation contract that produced it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkspaceConfigTreeResponse {
    pub snapshot: ConfigTreeSnapshot,
    pub contract: ToolchainContract,
    pub projection_digest: String,
}

/// Compare-and-swap request for applying a complete set of source-tree changes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct ConfigCommitRequest {
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_u64))]
    pub base_revision: u64,
    pub base_digest: String,
    pub changes: Vec<ConfigTreeChange>,
    pub entrypoints: Vec<String>,
}

/// Effective Prompt catalog projected from one immutable Workspace config revision.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkspacePromptProjection {
    pub workspace_id: String,
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_u64))]
    pub config_revision: u64,
    pub source_digest: String,
    pub projection_digest: String,
    pub schema_fingerprint: String,
    pub toolchain_fingerprint: String,
    pub catalog: EffectivePromptCatalog,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EffectivePromptCatalog {
    pub templates: BTreeMap<String, String>,
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_u64))]
    pub config_revision: u64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source_digest: String,
    pub schema_fingerprint: String,
    pub toolchain_fingerprint: String,
    pub catalog_digest: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum FlowSourceKind {
    Builtin,
    Workspace,
}

/// Current source record for one Workspace-authored Flow.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct FlowSourceRecord {
    pub workspace_id: String,
    pub flow_id: String,
    pub source_kind: FlowSourceKind,
    pub name: String,
    pub path: String,
    pub content: String,
    pub content_digest: String,
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_u64))]
    pub revision: u64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(transparent)]
pub struct FlowSourceListResponse(pub Vec<FlowSourceRecord>);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct PutFlowRequest {
    pub path: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct FlowSourceResolveRequest {
    pub selector: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct CompiledFlowTransition {
    pub id: String,
    pub target: String,
    pub condition: String,
    pub synthetic: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct CompiledFlowState {
    pub id: String,
    pub instructions: String,
    pub terminal: bool,
    pub transitions: Vec<CompiledFlowTransition>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct CompiledFlowDefinition {
    pub schema_version: u32,
    pub name: String,
    pub initial: String,
    pub states: BTreeMap<String, CompiledFlowState>,
    pub content_digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct ResolvedFlowSource {
    pub selector: String,
    pub workspace_id: String,
    pub flow_id: String,
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_u64))]
    pub revision: u64,
    pub content_digest: String,
    pub definition: CompiledFlowDefinition,
}

/// Query parameters for the bounded Memory staging list.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MemoryStagingQuery {
    #[schemars(range(min = 0, max = 1000))]
    pub limit: Option<u32>,
}

/// Public wrapper retaining the established tagged Memory backend wire shape.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct MemoryBackendRequest(pub memory::backend::MemoryBackendOperation);

/// Public wrapper retaining the established tagged Memory backend response shape.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct MemoryBackendResponse(pub memory::backend::MemoryBackendHttpResponse);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MemoryConsolidateStagingRequest {
    #[serde(default)]
    pub force: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MemoryConsolidationResponse {
    pub status: String,
    pub summary: String,
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_usize))]
    pub candidate_count: usize,
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_u64))]
    pub total_bytes: u64,
}

pub const WORKSPACE_DELETION_MAX_OPERATION_ID_BYTES: usize = 128;
pub const WORKSPACE_DELETION_MAX_REVISION_BYTES: usize = 128;
pub const WORKSPACE_DELETION_MAX_CONFIRMATION_BYTES: usize = 256;
pub const WORKSPACE_DELETION_MAX_BLOCKERS: usize = 1024;
pub const WORKSPACE_DELETION_MAX_CHILD_OPERATION_IDS: usize = 4096;
pub const WORKSPACE_DELETION_MAX_RESOURCE_VALUE_BYTES: usize = 128;
pub const WORKSPACE_DELETION_MAX_BLOCKER_MESSAGE_BYTES: usize = 512;

fn deserialize_workspace_deletion_operation_id<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    if value.len() > WORKSPACE_DELETION_MAX_OPERATION_ID_BYTES {
        return Err(serde::de::Error::custom(
            "Workspace deletion operation_id is too long",
        ));
    }
    Ok(value)
}

fn deserialize_workspace_deletion_revision<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    if value.len() > WORKSPACE_DELETION_MAX_REVISION_BYTES {
        return Err(serde::de::Error::custom(
            "Workspace deletion revision is too long",
        ));
    }
    Ok(value)
}

fn deserialize_workspace_deletion_confirmation<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    if value.len() > WORKSPACE_DELETION_MAX_CONFIRMATION_BYTES {
        return Err(serde::de::Error::custom(
            "Workspace deletion confirmation is too long",
        ));
    }
    Ok(value)
}

fn deserialize_workspace_deletion_resource_value<'de, D>(
    deserializer: D,
) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<String>::deserialize(deserializer)?;
    if value
        .as_ref()
        .is_some_and(|value| value.len() > WORKSPACE_DELETION_MAX_RESOURCE_VALUE_BYTES)
    {
        return Err(serde::de::Error::custom(
            "Workspace deletion resource value is too long",
        ));
    }
    Ok(value)
}

fn deserialize_workspace_deletion_blocker_message<'de, D>(
    deserializer: D,
) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    if value.len() > WORKSPACE_DELETION_MAX_BLOCKER_MESSAGE_BYTES {
        return Err(serde::de::Error::custom(
            "Workspace deletion blocker message is too long",
        ));
    }
    Ok(value)
}

fn deserialize_workspace_deletion_blockers<'de, D>(
    deserializer: D,
) -> Result<Vec<WorkspaceDeletionBlocker>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Vec::<WorkspaceDeletionBlocker>::deserialize(deserializer)?;
    if value.len() > WORKSPACE_DELETION_MAX_BLOCKERS {
        return Err(serde::de::Error::custom(
            "too many Workspace deletion blockers",
        ));
    }
    Ok(value)
}

fn deserialize_workspace_deletion_child_operation_ids<'de, D>(
    deserializer: D,
) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Vec::<String>::deserialize(deserializer)?;
    if value.len() > WORKSPACE_DELETION_MAX_CHILD_OPERATION_IDS {
        return Err(serde::de::Error::custom(
            "too many Workspace deletion child operations",
        ));
    }
    if value
        .iter()
        .any(|operation_id| operation_id.len() > WORKSPACE_DELETION_MAX_OPERATION_ID_BYTES)
    {
        return Err(serde::de::Error::custom(
            "Workspace deletion child operation_id is too long",
        ));
    }
    Ok(value)
}

/// Lifecycle state for one durable Workspace deletion operation.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceDeletionState {
    Queued,
    Running,
    Blocked,
    Failed,
    Succeeded,
}

/// Stable category explaining why Workspace deletion cannot currently advance.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceDeletionBlockerKind {
    LastAccessibleWorkspace,
    RevisionConflict,
    DirtyWorkdir,
    WorkerRemovalBlocked,
    WorkdirRemovalBlocked,
    RetentionHold,
    CleanupUnavailable,
}

/// One bounded, user-actionable blocker returned by preflight or execution.
#[derive(Debug, Clone, Serialize, JsonSchema, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkspaceDeletionBlocker {
    pub kind: WorkspaceDeletionBlockerKind,
    pub resource_kind: Option<String>,
    pub resource_key: Option<String>,
    pub message: String,
}

/// Workspace-owned resources summarized before destructive confirmation.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkspaceDeletionResourceCounts {
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_u64))]
    pub workers: u64,
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_u64))]
    pub workdirs: u64,
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_u64))]
    pub repositories: u64,
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_u64))]
    pub runtime_bindings: u64,
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_u64))]
    pub secrets: u64,
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_u64))]
    pub artifacts: u64,
}

/// Owner-only impact preview for deleting one Workspace.
#[derive(Debug, Clone, Serialize, JsonSchema, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkspaceDeletionPreflightResponse {
    pub workspace_id: String,
    pub display_name: String,
    /// Opaque persisted Workspace metadata revision used as a CAS fence.
    pub expected_revision: String,
    pub can_delete: bool,
    pub resources: WorkspaceDeletionResourceCounts,
    pub blockers: Vec<WorkspaceDeletionBlocker>,
}

/// Idempotent request to start or resume Workspace deletion.
#[derive(Debug, Clone, Serialize, JsonSchema, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkspaceDeletionRequest {
    pub operation_id: String,
    pub expected_revision: String,
    pub confirmation: String,
}

/// Durable deletion operation projection used by request responses and polling.
#[derive(Debug, Clone, Serialize, JsonSchema, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkspaceDeletionOperationResponse {
    pub operation_id: String,
    pub workspace_id: String,
    pub display_name: String,
    pub state: WorkspaceDeletionState,
    pub resources: WorkspaceDeletionResourceCounts,
    pub child_operation_ids: Vec<String>,
    pub blockers: Vec<WorkspaceDeletionBlocker>,
    pub failure_category: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub completed_at: Option<String>,
}

impl api_macros::HttpSuccess for WorkspaceDeletionOperationResponse {
    fn status_code(&self) -> u16 {
        if self.state == WorkspaceDeletionState::Succeeded {
            200
        } else {
            202
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceDeletionBlockerWire {
    kind: WorkspaceDeletionBlockerKind,
    #[serde(deserialize_with = "deserialize_workspace_deletion_resource_value")]
    resource_kind: Option<String>,
    #[serde(deserialize_with = "deserialize_workspace_deletion_resource_value")]
    resource_key: Option<String>,
    #[serde(deserialize_with = "deserialize_workspace_deletion_blocker_message")]
    message: String,
}

impl<'de> Deserialize<'de> for WorkspaceDeletionBlocker {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = WorkspaceDeletionBlockerWire::deserialize(deserializer)?;
        Ok(Self {
            kind: wire.kind,
            resource_kind: wire.resource_kind,
            resource_key: wire.resource_key,
            message: wire.message,
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceDeletionPreflightResponseWire {
    workspace_id: String,
    display_name: String,
    #[serde(deserialize_with = "deserialize_workspace_deletion_revision")]
    expected_revision: String,
    can_delete: bool,
    resources: WorkspaceDeletionResourceCounts,
    #[serde(deserialize_with = "deserialize_workspace_deletion_blockers")]
    blockers: Vec<WorkspaceDeletionBlocker>,
}

impl<'de> Deserialize<'de> for WorkspaceDeletionPreflightResponse {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = WorkspaceDeletionPreflightResponseWire::deserialize(deserializer)?;
        Ok(Self {
            workspace_id: wire.workspace_id,
            display_name: wire.display_name,
            expected_revision: wire.expected_revision,
            can_delete: wire.can_delete,
            resources: wire.resources,
            blockers: wire.blockers,
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceDeletionRequestWire {
    #[serde(deserialize_with = "deserialize_workspace_deletion_operation_id")]
    operation_id: String,
    #[serde(deserialize_with = "deserialize_workspace_deletion_revision")]
    expected_revision: String,
    #[serde(deserialize_with = "deserialize_workspace_deletion_confirmation")]
    confirmation: String,
}

impl<'de> Deserialize<'de> for WorkspaceDeletionRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = WorkspaceDeletionRequestWire::deserialize(deserializer)?;
        Ok(Self {
            operation_id: wire.operation_id,
            expected_revision: wire.expected_revision,
            confirmation: wire.confirmation,
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceDeletionOperationResponseWire {
    #[serde(deserialize_with = "deserialize_workspace_deletion_operation_id")]
    operation_id: String,
    workspace_id: String,
    display_name: String,
    state: WorkspaceDeletionState,
    resources: WorkspaceDeletionResourceCounts,
    #[serde(deserialize_with = "deserialize_workspace_deletion_child_operation_ids")]
    child_operation_ids: Vec<String>,
    #[serde(deserialize_with = "deserialize_workspace_deletion_blockers")]
    blockers: Vec<WorkspaceDeletionBlocker>,
    failure_category: Option<String>,
    created_at: String,
    updated_at: String,
    completed_at: Option<String>,
}

impl<'de> Deserialize<'de> for WorkspaceDeletionOperationResponse {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = WorkspaceDeletionOperationResponseWire::deserialize(deserializer)?;
        Ok(Self {
            operation_id: wire.operation_id,
            workspace_id: wire.workspace_id,
            display_name: wire.display_name,
            state: wire.state,
            resources: wire.resources,
            child_operation_ids: wire.child_operation_ids,
            blockers: wire.blockers,
            failure_category: wire.failure_category,
            created_at: wire.created_at,
            updated_at: wire.updated_at,
            completed_at: wire.completed_at,
        })
    }
}

/// Read-only Profile catalog projected from one active Workspace config revision.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(optional_fields = nullable))]
#[serde(deny_unknown_fields)]
pub struct ProfileSettingsResponse {
    pub workspace_id: String,
    pub registry_revision: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional, type = "number | null"))]
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_u64))]
    pub config_revision: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tree_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projection_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_profile: Option<String>,
    pub profiles: Vec<WorkspaceProfileSummary>,
    pub sources: Vec<WorkspaceProfileSourceSummary>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(optional_fields = nullable))]
#[serde(deny_unknown_fields)]
pub struct WorkspaceProfileSummary {
    pub profile_id: String,
    pub selector: String,
    pub label: String,
    pub source_kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_source_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub editable: bool,
    pub is_default: bool,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkspaceProfileSourceSummary {
    pub profile_source_id: String,
    pub display_path: String,
    pub kind: String,
    pub content_type: String,
    pub content_digest: String,
    pub provenance: WorkspaceProfileSourceProvenance,
    pub editable: bool,
    pub revision: String,
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_u64))]
    pub size_bytes: u64,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceProfileSourceProvenance {
    ProjectProfileSourceTree,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct RepositoryDiagnostic {
    pub severity: String,
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct GitRemoteSummary {
    pub name: String,
    pub fetch_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct GitRepositorySummary {
    pub status: String,
    pub head: Option<String>,
    pub branch: Option<String>,
    pub dirty: bool,
    pub remotes: Vec<GitRemoteSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct RepositorySummary {
    pub repository_key: String,
    pub kind: String,
    pub provider: String,
    pub source: RepositorySource,
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_u64))]
    pub source_revision: u64,
    pub source_fingerprint: String,
    pub observed_status: RepositoryObservedStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional = nullable))]
    pub observed_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional = nullable))]
    pub default_selector: Option<String>,
    pub record_authority: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional = nullable))]
    pub git: Option<GitRepositorySummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional = nullable))]
    pub diagnostics: Option<Vec<RepositoryDiagnostic>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct GitCommitSummary {
    pub hash: String,
    pub short_hash: String,
    pub summary: String,
    pub author_name: String,
    pub author_email: String,
    pub author_date: String,
    pub parents: Vec<String>,
    pub refs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct RepositoryListResponse {
    pub workspace_id: String,
    pub items: Vec<RepositorySummary>,
    pub source: String,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct RepositoryDetailResponse {
    pub workspace_id: String,
    pub item: RepositorySummary,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct RepositorySshConnectionProbeRequest {
    pub runtime_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct RepositorySshHostKeyCandidate {
    pub algorithm: String,
    pub host_key: String,
    pub fingerprint: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum RepositorySshConnectionTrustState {
    Untrusted,
    Verified,
    Changed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct RepositorySshConnectionProbeResponse {
    pub workspace_id: String,
    pub repository_key: String,
    pub runtime_id: String,
    pub hostname: String,
    pub port: u16,
    pub trust_state: RepositorySshConnectionTrustState,
    pub host_trust_id: String,
    #[cfg_attr(feature = "typescript", ts(type = "number | null"))]
    pub expected_host_trust_revision: Option<u64>,
    pub candidates: Vec<RepositorySshHostKeyCandidate>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct ConfirmRepositorySshHostTrustRequest {
    pub operation_id: String,
    pub runtime_id: String,
    pub host_key: String,
    #[cfg_attr(feature = "typescript", ts(type = "number | null"))]
    pub expected_host_trust_revision: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct RepositoryLogResponse {
    pub workspace_id: String,
    pub repository_key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional = nullable))]
    pub default_selector: Option<String>,
    pub limit: usize,
    pub items: Vec<GitCommitSummary>,
    pub diagnostics: Vec<Diagnostic>,
}

pub const TICKET_RELATIONS_QUERY_PATH: &str = "/tickets/relations/search";
pub const TICKET_ORCHESTRATION_PLANS_QUERY_PATH: &str = "/tickets/orchestration-plans/search";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticSeverity {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct Diagnostic {
    pub code: String,
    pub severity: DiagnosticSeverity,
    pub message: String,
}

/// Public Workdir materializer classification.
///
/// The value identifies stable materialization provenance without exposing a
/// provider path, Runtime handle, or session identity.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum WorkingDirectoryMaterializerKind {
    #[default]
    RuntimeGitClone,
    ClientHostedExternal,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum WorkingDirectoryStatusKind {
    Active,
    CleanupPending,
    Corrupted,
    NotFound,
    Unknown,
}

impl WorkingDirectoryStatusKind {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::CleanupPending => "cleanup_pending",
            Self::Corrupted => "corrupted",
            Self::NotFound => "not_found",
            Self::Unknown => "unknown",
        }
    }
}

impl std::fmt::Display for WorkingDirectoryStatusKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkingDirectoryCleanupTarget {
    pub kind: String,
    pub working_directory_id: String,
    pub repository_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkingDirectoryRemovalRequest {
    pub reason: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum WorkingDirectoryRemovalDisposition {
    Removed,
    Retained,
    AttentionRequired,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(optional_fields = nullable))]
#[serde(deny_unknown_fields)]
pub struct WorkingDirectoryRemovalResponse {
    pub working_directory_id: String,
    pub disposition: WorkingDirectoryRemovalDisposition,
    pub retryable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_category: Option<String>,
}

/// Durable Workspace occupancy projection for one Workdir.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkingDirectoryOccupancy {
    pub runtime_id: String,
    pub worker_id: String,
    pub display_name: String,
    pub linked_at: String,
}

/// Runtime-internal Workdir cleanup authority. This transport intentionally
/// retains the Backend-generated Repository id and is never a Workspace public
/// projection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(optional_fields = nullable))]
#[serde(deny_unknown_fields)]
pub struct RuntimeWorkingDirectoryCleanupTarget {
    pub kind: String,
    pub working_directory_id: String,
    pub repository_id: String,
}

/// Runtime-internal Workdir inventory transport. Workspace REST and model-facing
/// surfaces must project this through [`WorkingDirectorySummary`] so the UUID is
/// replaced with `repository_key`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(optional_fields = nullable))]
#[serde(deny_unknown_fields)]
pub struct RuntimeWorkingDirectorySummary {
    pub working_directory_id: String,
    /// Optional human-facing label, never a routing key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    pub repository_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creation_selector: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creation_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creation_tree: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_selector: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_tree: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(type = "number | null"))]
    pub observed_at_epoch_seconds: Option<u64>,
    pub materializer_kind: WorkingDirectoryMaterializerKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanup_target: Option<RuntimeWorkingDirectoryCleanupTarget>,
    pub status: WorkingDirectoryStatusKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanliness: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub occupied_by: Option<WorkingDirectoryOccupancy>,
}

/// Public Workdir source identity. Provider routing details and host paths are intentionally absent.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkingDirectorySource {
    Repository { repository_key: String },
    ExternalGrant { grant_id: String },
}

/// Public, provider-neutral Workdir inventory projection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(optional_fields = nullable))]
#[serde(deny_unknown_fields)]
pub struct WorkingDirectorySummary {
    pub working_directory_id: String,
    /// Optional human-facing label, never a routing key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    pub source: WorkingDirectorySource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creation_selector: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creation_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creation_tree: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_selector: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_tree: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional, type = "number | null"))]
    pub observed_at_epoch_seconds: Option<u64>,
    pub materializer_kind: WorkingDirectoryMaterializerKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanup_target: Option<WorkingDirectoryCleanupTarget>,
    pub status: WorkingDirectoryStatusKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanliness: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub occupied_by: Option<WorkingDirectoryOccupancy>,
}

impl WorkingDirectorySummary {
    /// Workspace-managed inventory rows carry explicit cleanup authority.
    pub fn is_workspace_managed(&self) -> bool {
        self.cleanup_target.is_some()
    }
}
impl From<workdir::workspace::WorkingDirectoryMaterializerKind>
    for WorkingDirectoryMaterializerKind
{
    fn from(value: workdir::workspace::WorkingDirectoryMaterializerKind) -> Self {
        match value {
            workdir::workspace::WorkingDirectoryMaterializerKind::RuntimeGitClone => {
                Self::RuntimeGitClone
            }
            workdir::workspace::WorkingDirectoryMaterializerKind::ClientHostedExternal => {
                Self::ClientHostedExternal
            }
        }
    }
}

impl From<workdir::workspace::WorkingDirectoryStatusKind> for WorkingDirectoryStatusKind {
    fn from(value: workdir::workspace::WorkingDirectoryStatusKind) -> Self {
        match value {
            workdir::workspace::WorkingDirectoryStatusKind::Active => Self::Active,
            workdir::workspace::WorkingDirectoryStatusKind::CleanupPending => Self::CleanupPending,
            workdir::workspace::WorkingDirectoryStatusKind::Corrupted => Self::Corrupted,
            workdir::workspace::WorkingDirectoryStatusKind::NotFound => Self::NotFound,
            workdir::workspace::WorkingDirectoryStatusKind::Unknown => Self::Unknown,
        }
    }
}

impl From<workdir::workspace::WorkingDirectoryCleanupTarget> for WorkingDirectoryCleanupTarget {
    fn from(value: workdir::workspace::WorkingDirectoryCleanupTarget) -> Self {
        Self {
            kind: value.kind,
            working_directory_id: value.working_directory_id,
            repository_key: value.repository_key,
        }
    }
}

impl From<workdir::workspace::WorkingDirectoryOccupancy> for WorkingDirectoryOccupancy {
    fn from(value: workdir::workspace::WorkingDirectoryOccupancy) -> Self {
        Self {
            runtime_id: value.runtime_id,
            worker_id: value.worker_id,
            display_name: value.display_name,
            linked_at: value.linked_at,
        }
    }
}

impl From<workdir::workspace::RuntimeWorkingDirectoryCleanupTarget>
    for RuntimeWorkingDirectoryCleanupTarget
{
    fn from(value: workdir::workspace::RuntimeWorkingDirectoryCleanupTarget) -> Self {
        Self {
            kind: value.kind,
            working_directory_id: value.working_directory_id,
            repository_id: value.repository_id,
        }
    }
}

impl From<workdir::workspace::RuntimeWorkingDirectorySummary> for RuntimeWorkingDirectorySummary {
    fn from(value: workdir::workspace::RuntimeWorkingDirectorySummary) -> Self {
        Self {
            working_directory_id: value.working_directory_id,
            display_name: value.display_name,
            repository_id: value.repository_id,
            creation_selector: value.creation_selector,
            creation_ref: value.creation_ref,
            creation_tree: value.creation_tree,
            current_selector: value.current_selector,
            current_ref: value.current_ref,
            current_tree: value.current_tree,
            observed_at_epoch_seconds: value.observed_at_epoch_seconds,
            materializer_kind: value.materializer_kind.into(),
            cleanup_target: value.cleanup_target.map(Into::into),
            status: value.status.into(),
            cleanliness: value.cleanliness,
            occupied_by: value.occupied_by.map(Into::into),
        }
    }
}

impl From<workdir::workspace::WorkingDirectorySummary> for WorkingDirectorySummary {
    fn from(value: workdir::workspace::WorkingDirectorySummary) -> Self {
        Self {
            working_directory_id: value.working_directory_id,
            display_name: value.display_name,
            source: match value.source {
                workdir::workspace::WorkingDirectorySource::Repository { repository_key } => {
                    WorkingDirectorySource::Repository { repository_key }
                }
                workdir::workspace::WorkingDirectorySource::ExternalGrant { grant_id } => {
                    WorkingDirectorySource::ExternalGrant { grant_id }
                }
            },
            creation_selector: value.creation_selector,
            creation_ref: value.creation_ref,
            creation_tree: value.creation_tree,
            current_selector: value.current_selector,
            current_ref: value.current_ref,
            current_tree: value.current_tree,
            observed_at_epoch_seconds: value.observed_at_epoch_seconds,
            materializer_kind: value.materializer_kind.into(),
            cleanup_target: value.cleanup_target.map(Into::into),
            status: value.status.into(),
            cleanliness: value.cleanliness,
            occupied_by: value.occupied_by.map(Into::into),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct ExternalWorkdirGrantCreateRequest {
    pub provider_instance_id: String,
    pub display_name: String,
    pub ttl_seconds: u64,
    pub read_only: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct ExternalWorkdirGrantResponse {
    pub grant_id: String,
    pub workspace_id: String,
    pub working_directory_id: String,
    pub provider_instance_id: String,
    pub display_name: String,
    pub permissions: String,
    pub expires_at: String,
    pub generation: u64,
    pub status: String,
}

/// Browser/Rust-client Workdir materialization request.
///
/// `runtime_id = None` requests Workspace default Runtime resolution and
/// `operation_id = Some(_)` fences exact replay. All four fields deliberately
/// preserve the Server's existing optionality.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(optional_fields = nullable))]
#[serde(deny_unknown_fields)]
pub struct WorkingDirectoryCreateRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_id: Option<String>,
    /// Optional human-facing label, independent of every Worker attachment alias.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    pub repository_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selector: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkingDirectoryListResponse {
    pub workspace_id: String,
    pub items: Vec<WorkingDirectorySummary>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkingDirectoryDetailResponse {
    pub workspace_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_id: Option<String>,
    pub item: WorkingDirectorySummary,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkingDirectoryCreateResponse {
    pub workspace_id: String,
    pub runtime_id: String,
    pub item: WorkingDirectorySummary,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct ListResponse<T> {
    pub workspace_id: String,
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_usize))]
    pub limit: usize,
    pub items: Vec<T>,
    pub source: String,
    #[serde(default)]
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct QueryPage {
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_usize))]
    pub limit: usize,
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_usize))]
    pub returned: usize,
    pub has_more: bool,
    pub next_cursor: Option<String>,
    pub sort: String,
    #[cfg_attr(feature = "typescript", ts(type = "number | null"))]
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_usize))]
    pub source_limit: Option<usize>,
    pub source_truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct ObjectiveEventDetail {
    pub event_ref: String,
    pub kind: String,
    pub body: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct ObjectiveLinkedTicketSummary {
    pub id: String,
    pub resource_key: String,
    pub title: String,
    pub state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct ObjectiveResourceSummary {
    pub path: String,
    pub media_type: Option<String>,
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_usize))]
    pub bytes: usize,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct ObjectiveSummary {
    pub id: String,
    pub resource_key: String,
    pub title: String,
    pub state: String,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub summary: String,
    pub linked_tickets: Vec<String>,
    pub record_source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct ObjectiveDetail {
    pub id: String,
    pub resource_key: String,
    pub title: String,
    pub state: String,
    pub revision: String,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub linked_tickets: Vec<String>,
    pub linked_ticket_summaries: Vec<ObjectiveLinkedTicketSummary>,
    pub resources: Vec<ObjectiveResourceSummary>,
    pub body: String,
    pub body_truncated: bool,
    pub events: Vec<ObjectiveEventDetail>,
    pub event_page: QueryPage,
    pub record_source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct ObjectiveCreateRequest {
    pub title: String,
    #[serde(default)]
    pub body_md: String,
    #[serde(default = "default_objective_state")]
    pub state: String,
    #[serde(default)]
    pub linked_tickets: Vec<String>,
}

fn default_objective_state() -> String {
    "active".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct ObjectiveEditRequest {
    pub title: Option<String>,
    pub old_string: Option<String>,
    pub new_string: Option<String>,
    #[serde(default)]
    pub replace_all: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct ObjectiveStateRequest {
    pub state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct ObjectiveLinkTicketRequest {
    pub ticket_id: String,
}

/// One malformed project record reported by a bounded list projection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct InvalidProjectRecord {
    pub label: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct TicketListItemSummary {
    pub id: String,
    pub resource_key: String,
    pub title: String,
    pub state: String,
    pub priority: String,
    pub updated_at: Option<String>,
    pub queued_by: Option<String>,
    pub queued_at: Option<String>,
    pub workspace_action_priority: String,
    pub record_source: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketListHttpQuery {
    pub states: Option<String>,
    #[schemars(range(min = 0, max = 1000))]
    pub limit: Option<usize>,
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct TicketListResponse {
    pub workspace_id: String,
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    #[schemars(range(min = 0, max = 1000))]
    pub limit: usize,
    pub items: Vec<TicketListItemSummary>,
    pub page: QueryPage,
    pub invalid_records: Vec<InvalidProjectRecord>,
    pub record_authority: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct TicketEventDetail {
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_usize))]
    pub sequence: usize,
    pub event_ref: String,
    pub kind: String,
    pub author: Option<String>,
    pub at: Option<String>,
    pub status: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub reason: Option<String>,
    pub state_field: Option<String>,
    pub heading: Option<String>,
    pub body: Option<String>,
    pub attributes: BTreeMap<String, String>,
    pub references: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct TicketDetailRelation {
    pub ticket_id: String,
    pub kind: String,
    pub target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional = nullable))]
    pub target_resource_key: Option<String>,
    pub note: Option<String>,
    pub author: String,
    pub at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct TicketDetailDerivedRelation {
    pub source_ticket: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional = nullable))]
    pub source_resource_key: Option<String>,
    pub inverse_kind: String,
    pub forward_kind: String,
    pub note: Option<String>,
    pub author: String,
    pub at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct TicketDetailRelationBlocker {
    pub blocking_ticket: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional = nullable))]
    pub blocking_resource_key: Option<String>,
    pub reason_kind: String,
    pub relation_kind: String,
    pub note: Option<String>,
    pub blocking_state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct TicketDetailRelationNotice {
    pub related_ticket: String,
    pub kind: String,
    pub message: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct TicketDetailRelationView {
    pub outgoing: Vec<TicketDetailRelation>,
    pub incoming: Vec<TicketDetailDerivedRelation>,
    pub blockers: Vec<TicketDetailRelationBlocker>,
    pub notices: Vec<TicketDetailRelationNotice>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct ObjectiveLinkSummary {
    pub id: String,
    pub resource_key: String,
    pub title: String,
    pub state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct TicketEvidenceEvent {
    pub event_ref: String,
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_usize))]
    pub sequence: usize,
    pub kind: String,
    pub at: Option<String>,
    pub author: Option<String>,
    pub excerpt: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct TicketAssignmentSummary {
    pub assignment_id: String,
    pub runtime_id: String,
    pub worker_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional = nullable))]
    pub worker_resource_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "typescript", ts(tag = "kind", rename_all = "snake_case"))]
pub enum TicketAssignmentPrincipal {
    User {
        account_id: String,
    },
    Worker {
        runtime_id: String,
        worker_id: String,
    },
    WorkspaceAgent {
        agent_key: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum TicketAssignmentRole {
    Orchestrator,
    Coder,
    Owner,
    Contributor,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct TicketRoleAssignmentRecord {
    pub workspace_id: String,
    pub ticket_id: String,
    pub assignment_id: String,
    pub role: TicketAssignmentRole,
    pub principal: TicketAssignmentPrincipal,
    pub assigned_by: String,
    pub assigned_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct TicketRoleAssignmentSummary {
    pub assignment_id: String,
    pub role: String,
    pub principal: TicketAssignmentPrincipal,
    pub assigned_by: String,
    pub assigned_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct TicketActionEligibility {
    pub can_assign_orchestrator: bool,
    pub can_unassign_orchestrator: bool,
    pub can_queue: bool,
    pub can_start_manual_coder: bool,
    pub queue_tickets: Vec<String>,
    pub blockers: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct TicketMergeRequestSummary {
    pub merge_request_id: String,
    pub repository_key: String,
    pub state: String,
    pub review_status: String,
    pub selector_from: Option<String>,
    pub selector_to: String,
    pub updated_at: String,
    pub current_subject_ref: Option<String>,
    pub review_subject_ref: Option<String>,
    pub review_requested_at: Option<String>,
    pub review_submitted_at: Option<String>,
    pub review_excerpt: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct MergeRequestRefDiagnostic {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct MergeRequestListItem {
    pub summary: TicketMergeRequestSummary,
    pub ticket_ids: Vec<String>,
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_usize))]
    pub thread_event_count: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ref_diagnostics: Vec<MergeRequestRefDiagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct MergeRequestListResponse {
    pub items: Vec<MergeRequestListItem>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct TicketEvidenceSummary {
    pub has_merge_request: bool,
    pub has_current_subject_ref: bool,
    pub has_review_request: bool,
    pub has_commit: bool,
    pub review_status: Option<String>,
    pub approved_current_subject: bool,
    pub review_after_rescope: bool,
    pub unresolved_request_changes: bool,
    pub complete_for_integration: bool,
    pub missing: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct TicketQueryRequest {
    pub query: Option<String>,
    #[serde(default)]
    pub states: Vec<String>,
    #[serde(default)]
    pub event_kinds: Vec<String>,
    #[serde(default)]
    pub evidence: Vec<String>,
    pub review_status: Option<String>,
    #[serde(default)]
    pub attention: Vec<String>,
    pub related_ticket_id: Option<String>,
    pub relation_kind: Option<String>,
    pub linked_objective_id: Option<String>,
    pub updated_after: Option<String>,
    pub updated_before: Option<String>,
    pub sort: Option<String>,
    #[schemars(range(min = 0, max = 1000))]
    pub limit: Option<usize>,
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct TicketQueryItem {
    pub id: String,
    pub resource_key: String,
    pub title: String,
    pub state: String,
    pub readiness: Option<String>,
    pub priority: String,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub item_revision: String,
    pub workspace_action_priority: String,
    pub matched_fields: Vec<String>,
    pub snippet: Option<String>,
    pub matching_event: Option<TicketEvidenceEvent>,
    pub linked_objective_ids: Vec<String>,
    pub linked_objective_keys: Vec<String>,
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_usize))]
    pub relation_count: usize,
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_usize))]
    pub blocker_count: usize,
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_usize))]
    pub unresolved_blocker_count: usize,
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_usize))]
    pub unresolved_review_count: usize,
    pub evidence: TicketEvidenceSummary,
    pub merge_request: Option<TicketMergeRequestSummary>,
    pub current_coder: Option<TicketAssignmentSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct TicketQueryResponse {
    pub items: Vec<TicketQueryItem>,
    pub page: QueryPage,
    pub record_authority: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct TicketShowRequest {
    #[schemars(range(min = 0, max = 1000))]
    pub event_limit: Option<usize>,
    pub event_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct TicketDetail {
    pub id: String,
    pub resource_key: String,
    pub title: String,
    pub state: String,
    pub readiness: Option<String>,
    pub priority: String,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub item_revision: String,
    pub queued_by: Option<String>,
    pub queued_at: Option<String>,
    pub repository_key: Option<String>,
    pub ref_selector: Option<String>,
    pub risk_flags: Vec<String>,
    pub body: String,
    pub body_truncated: bool,
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_usize))]
    pub event_count: usize,
    pub events: Vec<TicketEventDetail>,
    pub event_page: QueryPage,
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_usize))]
    pub artifact_count: usize,
    pub artifacts: Vec<String>,
    pub relations: TicketDetailRelationView,
    pub linked_objectives: Vec<ObjectiveLinkSummary>,
    pub implementation_reports: Vec<TicketEvidenceEvent>,
    pub assignments: Vec<TicketRoleAssignmentSummary>,
    pub current_coder: Option<TicketAssignmentSummary>,
    pub assignment_diagnostics: Vec<String>,
    pub action_eligibility: TicketActionEligibility,
    pub merge_request: Option<TicketMergeRequestSummary>,
    pub evidence: TicketEvidenceSummary,
    pub resolution: Option<String>,
    pub record_source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DefaultIntakeReadyBodyRequest {
    pub from: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(transparent)]
pub struct TextResponse(pub String);

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketSummarySearchQuery {
    pub state: Option<String>,
    #[schemars(range(min = 0, max = 1000))]
    pub limit: Option<usize>,
}

macro_rules! transparent_ticket_dto {
    ($name:ident, $inner:ty) => {
        #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
        #[serde(transparent)]
        pub struct $name(pub $inner);
    };
}

transparent_ticket_dto!(TicketRecord, ticket::Ticket);
transparent_ticket_dto!(TicketRecordRef, ticket::TicketRef);
transparent_ticket_dto!(TicketRecordSummaryList, Vec<ticket::TicketSummary>);
transparent_ticket_dto!(TicketDependencyCheckResponse, ticket::TicketDependencyCheck);
transparent_ticket_dto!(TicketQueueResponse, ticket::TicketQueueOutcome);
transparent_ticket_dto!(TicketDoctorResponse, ticket::TicketDoctorReport);
transparent_ticket_dto!(TicketRelationRecord, ticket::TicketRelation);
transparent_ticket_dto!(TicketRelationRecordList, Vec<ticket::TicketRelation>);
transparent_ticket_dto!(TicketRelationRecordView, ticket::TicketRelationView);
transparent_ticket_dto!(
    TicketOrchestrationPlanRecord,
    ticket::OrchestrationPlanRecord
);
transparent_ticket_dto!(
    TicketOrchestrationPlanRecordList,
    Vec<ticket::OrchestrationPlanRecord>
);
transparent_ticket_dto!(CreateTicketRecordRequest, ticket::NewTicket);
transparent_ticket_dto!(EditTicketRecordItemRequest, ticket::TicketItemEdit);
transparent_ticket_dto!(TicketThreadEventRequest, ticket::NewTicketEvent);
transparent_ticket_dto!(TicketStateChangeRequest, ticket::TicketStateChange);
transparent_ticket_dto!(TicketIntakeSummaryRequest, ticket::TicketIntakeSummary);
transparent_ticket_dto!(TicketCloseRecordRequest, ticket::MarkdownText);
transparent_ticket_dto!(CreateTicketRelationRequest, ticket::NewTicketRelation);
transparent_ticket_dto!(
    CreateTicketOrchestrationPlanRequest,
    ticket::NewOrchestrationPlanRecord
);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketMarkReadyRequest {
    pub operation_key: String,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub intake_summary: Option<ticket::TicketIntakeSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketRelationRemoveRequest {
    pub kind: ticket::TicketRelationKind,
    pub target: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketRelationSearchRequest {
    pub ticket: Option<ticket::TicketIdOrSlug>,
    pub kind: Option<ticket::TicketRelationKind>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketOrchestrationPlanSearchRequest {
    pub ticket: Option<ticket::TicketIdOrSlug>,
    pub kind: Option<ticket::OrchestrationPlanKind>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct SetTicketRoleAssignmentRequest {
    pub operation_id: String,
    pub principal: TicketAssignmentPrincipal,
    pub expected_assignment_id: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct ClearTicketRoleAssignmentQuery {
    pub operation_id: Option<String>,
    pub assignment_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct CancelTicketImplementationRequest {
    pub operation_id: String,
    pub assignment_id: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct TicketRoleAssignmentsResponse {
    pub workspace_id: String,
    pub ticket_id: String,
    pub assignments: Vec<TicketRoleAssignmentRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct TicketRoleAssignmentMutationResponse {
    pub workspace_id: String,
    pub ticket_id: String,
    pub assignment: Option<TicketRoleAssignmentRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "typescript", ts(tag = "action", rename_all = "snake_case"))]
pub enum BrowserTicketTargetEdit {
    Set {
        repository_key: String,
        ref_selector: Option<String>,
    },
    Clear,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum BrowserTicketWorkflowState {
    Planning,
    Ready,
    Queued,
    Inprogress,
    Done,
    Closed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct BrowserEditTicketRequest {
    pub title: Option<String>,
    pub body: Option<String>,
    pub old_string: Option<String>,
    pub new_string: Option<String>,
    #[serde(default)]
    pub replace_all: bool,
    pub target: Option<BrowserTicketTargetEdit>,
    pub author: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct BrowserTransitionTicketStateRequest {
    pub state: BrowserTicketWorkflowState,
    pub reason: Option<String>,
    pub body: Option<String>,
    pub author: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum BrowserTicketThreadRole {
    Comment,
    Plan,
    Decision,
    ImplementationReport,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct BrowserAppendTicketEventRequest {
    pub role: BrowserTicketThreadRole,
    pub body: String,
    pub author: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct BrowserQueueTicketRequest {}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct BrowserCloseTicketRequest {
    pub resolution: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ObjectiveListQuery {
    #[schemars(range(min = 0, max = 1000))]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct ObjectiveListResponse {
    pub workspace_id: String,
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    #[schemars(range(min = 0, max = 1000))]
    pub limit: usize,
    pub items: Vec<ObjectiveSummary>,
    pub invalid_records: Vec<InvalidProjectRecord>,
    pub record_authority: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct ObjectiveQueryRequest {
    pub query: Option<String>,
    #[serde(default)]
    pub states: Vec<String>,
    pub linked_ticket_id: Option<String>,
    pub updated_after: Option<String>,
    pub updated_before: Option<String>,
    pub sort: Option<String>,
    #[schemars(range(min = 0, max = 1000))]
    pub limit: Option<usize>,
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct ObjectiveQueryItem {
    pub id: String,
    pub resource_key: String,
    pub title: String,
    pub state: String,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub matched_fields: Vec<String>,
    pub snippet: Option<String>,
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_usize))]
    pub linked_ticket_count: usize,
    pub linked_tickets: Vec<String>,
    pub linked_ticket_keys: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct ObjectiveQueryResponse {
    pub items: Vec<ObjectiveQueryItem>,
    pub page: QueryPage,
    pub record_authority: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct ObjectiveShowRequest {
    #[schemars(range(min = 0, max = 1000))]
    pub event_limit: Option<usize>,
    pub event_cursor: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum MergeRequestState {
    Open,
    Merged,
    Closed,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum ReviewDecision {
    Approve,
    RequestChanges,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum FindingSeverity {
    Blocker,
    Major,
    Minor,
    Note,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct ReviewFinding {
    pub severity: FindingSeverity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional = nullable))]
    pub code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional = nullable))]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional = nullable))]
    pub line: Option<u32>,
    pub body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct MergeRequestWorkerIdentity {
    pub runtime_id: String,
    pub worker_id: String,
}

macro_rules! merge_request_event {
    ($name:ident { $($field:ident : $ty:ty),* $(,)? }) => {
        #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
        #[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
        #[serde(deny_unknown_fields)]
        pub struct $name {
            pub event_id: String,
            #[cfg_attr(feature = "typescript", ts(type = "number"))]
            #[schemars(range(min = 0, max = 9_007_199_254_740_991_u64))]
            pub sequence: u64,
            $(pub $field: $ty,)*
            pub created_at: String,
        }
    };
}

merge_request_event!(ReviewRequestedEvent {
    subject_ref: String,
    requested_by: MergeRequestWorkerIdentity,
    reviewer: MergeRequestWorkerIdentity,
});
merge_request_event!(ReviewEvent {
    request_event_id: String,
    subject_ref: String,
    decision: ReviewDecision,
    body: String,
    findings: Vec<ReviewFinding>,
    reviewer: MergeRequestWorkerIdentity,
});
merge_request_event!(ReviewRevokedEvent {
    review_event_id: String,
    subject_ref: String,
    reason: String,
    revoked_by: MergeRequestWorkerIdentity,
});
merge_request_event!(ReviewCancelledEvent {
    request_event_id: String,
    subject_ref: String,
    reason: String,
});
merge_request_event!(MergeRequestCommentEvent {
    body: String,
    author: MergeRequestWorkerIdentity,
});

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum MergeStrategy {
    FastForward,
    Merge,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum ConflictResolution {
    None,
    Clean,
    ConflictsResolved,
}

merge_request_event!(MergeEvent {
    operation_id: String,
    approval_event_id: String,
    approved_source_ref: String,
    target_ref_before: String,
    target_ref_after: String,
    strategy: MergeStrategy,
    resolution: ConflictResolution,
    merged_by: MergeRequestWorkerIdentity,
});

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MergeRequestThreadEvent {
    ReviewRequested(ReviewRequestedEvent),
    Review(ReviewEvent),
    ReviewRevoked(ReviewRevokedEvent),
    ReviewCancelled(ReviewCancelledEvent),
    Comment(MergeRequestCommentEvent),
    Merge(MergeEvent),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct PublicMergeRequest {
    pub workspace_id: String,
    pub merge_request_id: String,
    pub repository_key: String,
    pub state: MergeRequestState,
    pub selector_from: Option<String>,
    pub selector_to: String,
    pub ticket_ids: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
    pub thread: Vec<MergeRequestThreadEvent>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MergeRequestListQuery {
    pub state: Option<String>,
    pub repository_key: Option<String>,
    pub ticket_ref: Option<String>,
    pub selector_from: Option<String>,
    pub selector_to: Option<String>,
    pub cursor: Option<String>,
    #[schemars(range(min = 0, max = 1000))]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MergeRequestThreadQuery {
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_u64))]
    pub after: Option<u64>,
    #[schemars(range(min = 0, max = 1000))]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct MergeRequestRefResponse {
    pub status: String,
    #[serde(rename = "ref")]
    pub revision_ref: Option<String>,
    pub observed_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional = nullable))]
    pub diagnostic: Option<MergeRequestRefDiagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct MergeRequestLinkedTicketResponse {
    pub ticket_id: String,
    pub key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct MergeRequestDetailResponse {
    #[serde(flatten)]
    pub merge_request: PublicMergeRequest,
    pub source: MergeRequestRefResponse,
    pub target: MergeRequestRefResponse,
    pub linked_tickets: Vec<MergeRequestLinkedTicketResponse>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct MergeRequestReadinessResponse {
    pub ready: bool,
    pub blockers: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional = nullable))]
    pub subject_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional = nullable))]
    pub review: Option<ReviewEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(transparent)]
pub struct MergeRequestThreadResponse(pub Vec<MergeRequestThreadEvent>);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OpenMergeRequestRequest {
    pub repository_key: String,
    pub selector_from: String,
    pub selector_to: String,
    #[serde(default)]
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RepairMergeRequestSelectorRequest {
    pub selector_from: String,
    pub reason: String,
    pub explicit_confirmation: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RevokeMergeRequestReviewRequest {
    pub review_event_id: String,
    pub reason: String,
    pub explicit_confirmation: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RegisterReviewerChildSessionRequest {
    pub child_session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RegisterMergeRequestReviewCapabilityRequest {
    pub child_session_id: String,
    pub capability_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SubmitMergeRequestReviewRequest {
    pub capability_token: String,
    pub decision: ReviewDecision,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub findings: Vec<ReviewFinding>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CompleteMergeRequestRequest {
    pub operation_id: String,
    pub approval_event_id: String,
    pub target_ref_before: String,
    pub target_ref_after: String,
    pub strategy: MergeStrategy,
    pub resolution: ConflictResolution,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum RuntimeSourceKind {
    EmbeddedWorkerRuntime,
    RemoteHttp,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum RuntimeSourceStatus {
    Active,
    Reserved,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum RuntimeIdentityAuthority {
    RuntimeRegistryProjection,
    ServerRuntimeConfiguration,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct RuntimeSourceSummary {
    pub kind: RuntimeSourceKind,
    pub status: RuntimeSourceStatus,
    pub identity_authority: RuntimeIdentityAuthority,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct RuntimeSummary {
    pub runtime_id: String,
    pub label: String,
    pub kind: String,
    pub status: String,
    pub source: RuntimeSourceSummary,
    #[serde(default)]
    pub host_ids: Vec<String>,
    pub worker_creation_available: bool,
    pub os: String,
    pub arch: String,
    #[serde(default)]
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceRuntimeBindingState {
    Configured,
    Verified,
    Revoked,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum RuntimeConnectionDisplayState {
    Configured,
    Verified,
    Unavailable,
    Revoked,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum RuntimeVerificationOutcome {
    Verified,
    ChallengeIssued,
    VerificationFailed,
    ConnectivityFailed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct RuntimeVerificationEvidenceSummary {
    pub verified_at: Option<String>,
    pub last_checked_at: String,
    pub last_outcome: RuntimeVerificationOutcome,
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    pub binding_revision: u64,
    pub workspace_key_id: String,
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    pub workspace_identity_revision: u64,
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    pub workspace_trust_generation: u64,
    pub runtime_public_key_fingerprint: String,
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    pub runtime_identity_revision: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkspaceRuntimeBindingSummary {
    pub state: WorkspaceRuntimeBindingState,
    pub connection_state: RuntimeConnectionDisplayState,
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    pub revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_key_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(type = "number | null"))]
    pub workspace_key_generation: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification: Option<RuntimeVerificationEvidenceSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct RuntimeManagementSummary {
    pub built_in: bool,
    pub config_managed: bool,
    pub removable: bool,
    pub endpoint_configured: bool,
    pub token_ref_configured: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding: Option<WorkspaceRuntimeBindingSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct WorkspaceRuntimeResource {
    #[serde(flatten)]
    pub runtime: RuntimeSummary,
    pub management: RuntimeManagementSummary,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum RuntimeTrustKeyStatus {
    Unconfigured,
    Active,
    Revoked,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct RuntimeTrustKeyState {
    pub status: RuntimeTrustKeyStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(type = "number | null"))]
    pub revision: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_at: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum RuntimeTrustAuditAction {
    Created,
    Replaced,
    Reactivated,
    Revoked,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct RuntimeTrustAuditEntry {
    pub action: RuntimeTrustAuditAction,
    pub actor_account_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_fingerprint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_fingerprint: Option<String>,
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    pub revision: u64,
    pub at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkspaceRuntimeDetail {
    pub workspace_id: String,
    pub runtime: WorkspaceRuntimeResource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    pub trust_key: RuntimeTrustKeyState,
    #[serde(default)]
    pub recent_audit: Vec<RuntimeTrustAuditEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct RuntimeTrustKeyRevealResponse {
    pub public_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct RevokeRuntimeTrustKeyRequest {
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    pub expected_revision: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct RemoveRuntimeRequest {
    pub operation_id: String,
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    pub expected_binding_revision: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum RuntimeRemovalOperationState {
    Pending,
    CleanupPending,
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct RuntimeRemovalOperationResponse {
    pub operation_id: String,
    pub workspace_id: String,
    pub runtime_id: String,
    pub state: RuntimeRemovalOperationState,
    pub binding_removed: bool,
    pub runtime_registration_removed: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_category: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum RuntimeTrustConflictKind {
    StaleRevision,
    FingerprintInUse,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct RuntimeTrustConflictResponse {
    pub error: RuntimeTrustConflictKind,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    pub current_revision: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_fingerprint: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct RuntimePublicIdentityBundle {
    pub identity_id: String,
    pub public_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct CreateRemoteRuntimeRequest {
    pub public_bundle: RuntimePublicIdentityBundle,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    pub endpoint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(type = "number | null"))]
    pub expected_revision: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct UpdateRemoteRuntimeRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    pub endpoint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum RuntimeConnectionTestStatus {
    Compatible,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum RuntimeConnectionTestFailureKind {
    Authentication,
    Authorization,
    NetworkUnreachable,
    Timeout,
    TlsOrTransport,
    MalformedResponse,
    ProtocolVersionMismatch,
    RuntimeIdentityMismatch,
    Configuration,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct RuntimeConnectionTestResponse {
    pub workspace_id: String,
    pub runtime_id: String,
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    pub binding_revision: u64,
    pub connection_state: RuntimeConnectionDisplayState,
    pub verification: Option<RuntimeVerificationEvidenceSummary>,
    pub checked_at: String,
    pub status: RuntimeConnectionTestStatus,
    pub failure_kind: Option<RuntimeConnectionTestFailureKind>,
    pub expected_protocol_version: u32,
    pub actual_protocol_version: Option<u32>,
    #[serde(default)]
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct WorkerWorkspaceSummary {
    pub visibility: String,
    pub identity: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct WorkerImplementationSummary {
    pub kind: String,
    pub display_hint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct WorkerCapabilitySummary {
    pub can_stop: bool,
    pub can_spawn_followup: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkspaceWorkerSubject {
    RuntimeWorker {
        runtime_id: String,
        worker_id: String,
    },
}

/// Bounded, model-safe projection used by privileged Workspace Worker discovery.
/// Runtime placement appears only in the typed subject required by Worker
/// control operations; provider and launch internals are intentionally omitted.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct WorkspaceWorkerDiscoveryItem {
    pub subject: WorkspaceWorkerSubject,
    pub resource_key: String,
    pub display_name: String,
    pub profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

/// Public lifecycle projection for the Workspace Companion endpoint.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum CompanionLifecycleState {
    Idle,
    Running,
    Stopped,
}

/// Public outcome of a Companion message submission.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum CompanionMessageDisposition {
    Accepted,
    Rejected,
}

/// Public, bounded transport metadata for Companion status.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct CompanionTransportSummary {
    pub mode: String,
    pub available: bool,
}

/// Public Workspace Companion status response.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct CompanionStatusResponse {
    pub state: CompanionLifecycleState,
    pub worker: Option<WorkspaceWorkerDiscoveryItem>,
    pub transport: CompanionTransportSummary,
    #[serde(default)]
    pub diagnostics: Vec<Diagnostic>,
}

/// Public Workspace Companion message request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct CompanionMessageRequest {
    pub content: String,
}

/// Public Workspace Companion cancellation request.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct CompanionCancelRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Public Workspace Companion message response.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct CompanionMessageResponse {
    pub state: CompanionMessageDisposition,
    pub message: String,
}

/// User-visible role accepted in the public Companion transcript.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum CompanionTranscriptRole {
    User,
    Assistant,
}

/// One allowlisted, user-visible Companion transcript item.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct CompanionTranscriptItem {
    pub sequence: usize,
    pub role: CompanionTranscriptRole,
    pub content: String,
    pub created_at: String,
}

/// Bounded public Companion transcript projection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct CompanionTranscriptProjection {
    pub state: CompanionLifecycleState,
    pub start: usize,
    pub limit: usize,
    pub total: usize,
    pub next: Option<usize>,
    pub items: Vec<CompanionTranscriptItem>,
}

#[cfg(feature = "typescript")]
pub fn companion_api_typescript() -> String {
    use ts_rs::TS;

    let config = ts_rs::Config::default();
    let declarations = [
        DiagnosticSeverity::decl(&config),
        Diagnostic::decl(&config),
        WorkspaceWorkerSubject::decl(&config),
        WorkspaceWorkerDiscoveryItem::decl(&config),
        CompanionLifecycleState::decl(&config),
        CompanionMessageDisposition::decl(&config),
        CompanionTransportSummary::decl(&config),
        CompanionStatusResponse::decl(&config),
        CompanionMessageRequest::decl(&config),
        CompanionCancelRequest::decl(&config),
        CompanionMessageResponse::decl(&config),
        CompanionTranscriptRole::decl(&config),
        CompanionTranscriptItem::decl(&config),
        CompanionTranscriptProjection::decl(&config),
    ];

    format!(
        "// Generated by `cargo run -p server-api --features typescript --example generate_companion_api_types`.\n// Do not edit manually.\n\n{}\n",
        declarations
            .into_iter()
            .map(|declaration| format!("export {declaration}"))
            .collect::<Vec<_>>()
            .join("\n\n")
    )
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceWorkerDiscoveryPage {
    pub workers: Vec<WorkspaceWorkerDiscoveryItem>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// One Workspace Worker Workdir attachment. `alias` is the stable Worker-local
/// routing key; the nested Workdir id and display name are inventory metadata.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkerWorkdirAttachmentSummary {
    pub alias: String,
    pub working_directory: WorkingDirectorySummary,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct RuntimeWorkerWorkdirAttachmentSummary {
    pub alias: String,
    pub working_directory: RuntimeWorkingDirectorySummary,
}

/// Workspace-authoritative Worker projection.
///
/// `resource_key` is required here even though Runtime-internal Worker summaries
/// do not carry one. The Workspace Server must resolve it from Workspace
/// authority before constructing this response.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct WorkerSummary {
    pub runtime_id: String,
    pub worker_id: String,
    pub resource_key: String,
    pub host_id: String,
    #[serde(default)]
    pub display_name: String,
    pub label: String,
    pub profile: Option<String>,
    pub singleton_key: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub workspace: WorkerWorkspaceSummary,
    /// Runtime catalog lifecycle compatibility state. Live foreground state, when
    /// available, is carried separately in `worker_state`.
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional))]
    pub worker_state: Option<protocol::WorkerStateSnapshot>,
    pub last_seen_at: Option<String>,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub retention_state: String,
    pub implementation: WorkerImplementationSummary,
    pub capabilities: WorkerCapabilitySummary,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workdir_attachments: Vec<WorkerWorkdirAttachmentSummary>,
    #[serde(default)]
    pub diagnostics: Vec<Diagnostic>,
}

/// Runtime-owned Worker summary embedded in Worker launch responses.
///
/// This preserves the existing launch wire shape. Workspace-owned Worker list
/// and detail responses use [`WorkerSummary`] instead.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkerLaunchWorkerSummary {
    pub runtime_id: String,
    pub worker_id: String,
    pub host_id: String,
    pub display_name: String,
    pub label: String,
    pub profile: Option<String>,
    pub singleton_key: Option<String>,
    pub tags: Vec<String>,
    pub workspace: WorkerWorkspaceSummary,
    pub state: String,
    pub last_seen_at: Option<String>,
    pub pinned: bool,
    pub retention_state: String,
    pub implementation: WorkerImplementationSummary,
    pub capabilities: WorkerCapabilitySummary,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workdir_attachments: Vec<RuntimeWorkerWorkdirAttachmentSummary>,
    #[serde(default)]
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkerLaunchOptionsResponse {
    pub workspace_id: String,
    pub runtimes: Vec<WorkerLaunchRuntimeOption>,
    pub default_profile: Option<String>,
    pub profiles: Vec<WorkerLaunchProfileCandidate>,
    pub repositories: Vec<WorkingDirectoryRepositoryOption>,
    pub working_directories: Vec<WorkingDirectorySummary>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkerLaunchRuntimeOption {
    pub runtime_id: String,
    pub display_name: String,
    pub built_in: bool,
    pub worker_creation_available: bool,
    pub working_directory_required: bool,
    pub status: String,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkerLaunchProfileCandidate {
    pub id: String,
    pub label: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkingDirectoryRepositoryOption {
    pub repository_key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional = nullable))]
    pub default_selector: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct BrowserWorkerWorkingDirectorySelection {
    /// Stable Worker-local routing alias.
    pub alias: String,
    pub working_directory_id: String,
    #[serde(default)]
    pub relative_cwd: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct CreateWorkspaceWorkerTicketAssignmentRequest {
    pub ticket_id: String,
    pub operation_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct CreateWorkspaceWorkerRequest {
    pub runtime_id: String,
    pub display_name: String,
    #[serde(default)]
    pub profile: Option<String>,
    #[serde(default)]
    pub ticket_assignment: Option<CreateWorkspaceWorkerTicketAssignmentRequest>,
    #[serde(default)]
    pub initial_submit: Vec<protocol::Segment>,
    #[serde(default)]
    pub workdir_attachments: Vec<BrowserWorkerWorkingDirectorySelection>,
    /// Backend idempotency key used only for authenticated Worker-owned spawn/control.
    #[serde(default)]
    pub control_operation_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct BrowserCreateWorkerResponse {
    pub workspace_id: String,
    pub runtime_id: String,
    pub worker_id: String,
    pub console_href: String,
    pub worker: WorkerLaunchWorkerSummary,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct BrowserWorkspaceOrchestratorResponse {
    pub workspace_id: String,
    pub online: bool,
    pub disposition: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional = nullable))]
    pub worker: Option<WorkerLaunchWorkerSummary>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkerOperationState {
    Accepted,
    Unsupported,
    Rejected,
}

/// Cross-layer outcome of an explicit Worker restore operation.
///
/// `Rejected` is reserved for read-only preflight failures. Once live restore
/// work starts, failure is either a confirmed `RolledBack` operation or a
/// `ReconciliationRequired` result whose commit state must be reread/retried.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum WorkerRestoreState {
    Accepted,
    Rejected,
    RolledBack,
    ReconciliationRequired,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkerRestoreResult {
    pub state: WorkerRestoreState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional = nullable))]
    pub worker: Option<WorkerSummary>,
    #[serde(default)]
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkerRestoreResponse {
    pub workspace_id: String,
    pub runtime_id: String,
    pub worker_id: String,
    pub result: WorkerRestoreResult,
}

pub const MEMORY_API_MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
pub const MEMORY_API_MAX_DOCUMENT_BYTES: usize = 4 * 1024 * 1024;
pub const MEMORY_API_MAX_COLLECTION_ITEMS: usize = 500;
pub const MEMORY_API_MAX_STRING_BYTES: usize = 1024 * 1024;
pub const MEMORY_API_MAX_IDENTIFIER_BYTES: usize = 512;

/// Public Workspace Memory document projection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct MemoryDocumentResponse {
    pub body_md: String,
    pub created_at: String,
    pub updated_at: String,
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_usize))]
    pub bytes: usize,
    pub record_source: String,
}

/// Candidate kinds exposed by the Memory staging resource.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum MemoryCandidateKind {
    Preference,
    WorkingAssumption,
    Constraint,
    Decision,
    OpenQuestion,
    Lesson,
}

/// Typed, bounded provenance classification for public Memory evidence anchors.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum MemoryEvidenceOriginKind {
    HumanInput,
    WorkerInput,
    FlowInstruction,
    BackendInstruction,
    ModelOutput,
    ToolOutput,
    DerivedSummary,
    LegacyUnknown,
}

/// Bounded origin metadata copied from one typed Memory evidence anchor.
///
/// This is provenance only. It carries no message body, prompt, reasoning,
/// secret, tool output, or authorization authority.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(optional_fields = nullable))]
#[serde(deny_unknown_fields)]
pub struct MemoryEvidenceOrigin {
    pub kind: MemoryEvidenceOriginKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flow_selector: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flow_definition_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional, type = "number | null"))]
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_u64))]
    pub flow_definition_revision: Option<u64>,
}

/// Record-level source range for one Memory staging candidate.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct MemorySourceRef {
    pub segment_id: String,
    #[cfg_attr(feature = "typescript", ts(type = "[number, number]"))]
    #[schemars(with = "JsonSafeU64Pair")]
    pub range: [u64; 2],
}

/// Bounded evidence snippet included in one Memory staging record.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct MemoryStagingEvidence {
    pub id: String,
    pub kind: String,
    #[cfg_attr(feature = "typescript", ts(type = "[number, number] | null"))]
    #[schemars(with = "Option<JsonSafeU64Pair>")]
    pub entry_range: Option<[u64; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(
        feature = "typescript",
        ts(optional, type = "MemoryEvidenceOrigin | null")
    )]
    pub origin: Option<MemoryEvidenceOrigin>,
    pub excerpt: Option<String>,
    pub summary: Option<String>,
}

/// Bounded source anchor included in one Memory staging record.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct MemorySourceEvidenceRef {
    pub session_id: Option<String>,
    pub segment_id: Option<String>,
    #[cfg_attr(feature = "typescript", ts(type = "[number, number] | null"))]
    #[schemars(with = "Option<JsonSafeU64Pair>")]
    pub entry_range: Option<[u64; 2]>,
    pub evidence_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(
        feature = "typescript",
        ts(optional, type = "MemoryEvidenceOrigin | null")
    )]
    pub origin: Option<MemoryEvidenceOrigin>,
    pub evidence_kind: Option<String>,
    pub label: Option<String>,
    pub summary: Option<String>,
}

/// Public projection of one valid Memory staging record.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct MemoryStagingRecord {
    pub schema_version: u32,
    pub id: String,
    pub extract_run_id: String,
    pub source: MemorySourceRef,
    pub kind: MemoryCandidateKind,
    pub claim: String,
    pub why_useful: String,
    pub staleness: Option<String>,
    pub evidence: Vec<MemoryStagingEvidence>,
    pub source_refs: Vec<MemorySourceEvidenceRef>,
}

/// Public list entry for one valid Memory staging record.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct MemoryStagingEntry {
    pub id: String,
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_u64))]
    pub byte_len: u64,
    pub record: MemoryStagingRecord,
}

/// Public response returned by the Workspace Memory staging list resource.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct MemoryStagingListResponse {
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_usize))]
    pub limit: usize,
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_usize))]
    pub returned_count: usize,
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_usize))]
    pub total_valid_count: usize,
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_usize))]
    pub invalid_count: usize,
    pub truncated: bool,
    pub order: String,
    pub record_authority: String,
    pub items: Vec<MemoryStagingEntry>,
    pub diagnostics: Vec<Diagnostic>,
}

#[cfg(feature = "typescript")]
pub fn memory_api_typescript() -> String {
    use ts_rs::TS;

    let config = ts_rs::Config::default();
    let declarations = [
        DiagnosticSeverity::decl(&config),
        Diagnostic::decl(&config),
        MemoryDocumentResponse::decl(&config),
        MemoryCandidateKind::decl(&config),
        MemoryEvidenceOriginKind::decl(&config),
        MemoryEvidenceOrigin::decl(&config),
        MemorySourceRef::decl(&config),
        MemoryStagingEvidence::decl(&config),
        MemorySourceEvidenceRef::decl(&config),
        MemoryStagingRecord::decl(&config),
        MemoryStagingEntry::decl(&config),
        MemoryStagingListResponse::decl(&config),
    ];

    let limits = format!(
        "export const MEMORY_API_LIMITS = {{\n  maxResponseBytes: {MEMORY_API_MAX_RESPONSE_BYTES},\n  maxDocumentBytes: {MEMORY_API_MAX_DOCUMENT_BYTES},\n  maxCollectionItems: {MEMORY_API_MAX_COLLECTION_ITEMS},\n  maxStringBytes: {MEMORY_API_MAX_STRING_BYTES},\n  maxIdentifierBytes: {MEMORY_API_MAX_IDENTIFIER_BYTES},\n}} as const;"
    );
    format!(
        "// Generated from server-api. Do not edit by hand.\n// Regenerate: cargo run -q -p server-api --features typescript --example generate_memory_api_types > web/workspace/src/lib/generated/memory-api.ts\n\n{limits}\n\n{}\n",
        declarations
            .into_iter()
            .map(|declaration| format!("export {declaration}"))
            .collect::<Vec<_>>()
            .join("\n\n")
    )
}

/// Workspace-owned Memory settings returned by the shared Server API.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceMemorySettings {
    pub workspace_id: String,
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_u64))]
    pub settings_revision: u64,
    pub language: String,
}

/// Compare-and-swap update for Workspace-owned Memory settings.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateWorkspaceMemorySettingsRequest {
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_u64))]
    pub expected_revision: u64,
    pub language: String,
}

/// Public metadata for one Workspace-scoped Repository SSH credential.
///
/// Secret references and secret material are deliberately not part of this DTO.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct RepositorySshCredential {
    pub credential_id: String,
    pub workspace_id: String,
    pub name: String,
    pub public_key_algorithm: String,
    pub public_key_fingerprint: String,
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_u64))]
    pub current_revision: u64,
    pub status: String,
    pub created_at: String,
    pub rotated_at: Option<String>,
    #[serde(default)]
    pub referenced_repositories: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(transparent)]
pub struct RepositorySshCredentialListResponse(pub Vec<RepositorySshCredential>);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct CreateRepositorySshCredentialRequest {
    pub operation_id: String,
    pub credential_id: String,
    pub name: String,
    pub private_key: String,
    #[serde(default)]
    pub passphrase: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct GenerateRepositorySshCredentialRequest {
    pub operation_id: String,
    pub credential_id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct RepositorySshPublicKey {
    pub credential_id: String,
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_u64))]
    pub current_revision: u64,
    pub public_key_algorithm: String,
    pub public_key_fingerprint: String,
    pub public_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct RotateRepositorySshCredentialRequest {
    pub operation_id: String,
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_u64))]
    pub expected_revision: u64,
    pub private_key: String,
    #[serde(default)]
    pub passphrase: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct DeleteRepositorySshCredentialRequest {
    pub operation_id: String,
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_u64))]
    pub expected_revision: u64,
}

/// Public metadata for an explicitly pinned SSH host key.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct RepositorySshHostTrust {
    pub host_trust_id: String,
    pub workspace_id: String,
    pub hostname: String,
    pub port: u16,
    pub key_algorithm: String,
    pub host_key: String,
    pub fingerprint: String,
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_u64))]
    pub current_revision: u64,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default)]
    pub referenced_repositories: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(transparent)]
pub struct RepositorySshHostTrustListResponse(pub Vec<RepositorySshHostTrust>);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct PutRepositorySshHostTrustRequest {
    pub operation_id: String,
    pub host_trust_id: String,
    pub hostname: String,
    pub port: u16,
    pub host_key: String,
    #[serde(default)]
    #[cfg_attr(feature = "typescript", ts(type = "number | null"))]
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_u64))]
    pub expected_revision: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RepositorySshHostTrustMutationResponse {
    #[serde(flatten)]
    pub host_trust: RepositorySshHostTrust,
    #[serde(skip)]
    #[schemars(skip)]
    pub created: bool,
}

impl api_macros::HttpSuccess for RepositorySshHostTrustMutationResponse {
    fn status_code(&self) -> u16 {
        if self.created { 201 } else { 200 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct DeleteRepositorySshHostTrustRequest {
    pub operation_id: String,
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_u64))]
    pub expected_revision: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(rename_all = "snake_case"))]
#[serde(rename_all = "snake_case")]
pub enum RepositoryAccessMode {
    ReadOnly,
    ReadWrite,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct RepositorySshAccessBinding {
    pub repository_key: String,
    pub credential_id: String,
    pub host_trust_id: String,
    pub access: RepositoryAccessMode,
}

/// Secret-free active Repository access projection consumed by later Runtime work.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct RepositoryAccessProjection {
    pub workspace_id: String,
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_u64))]
    pub config_revision: u64,
    pub projection_digest: String,
    pub bindings: Vec<RepositorySshAccessBinding>,
}

pub const SKILL_CATALOG_AUTHORITY: &str = "workspace-config-skills-v1";
pub const SKILL_API_MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
pub const SKILL_API_MAX_CATALOG_ENTRIES: usize = 500;
pub const SKILL_API_MAX_OVERRIDES: usize = 64;
pub const SKILL_API_MAX_DIAGNOSTICS: usize = 100;
pub const SKILL_API_MAX_RESOURCES: usize = 500;
pub const SKILL_API_MAX_ALLOWED_TOOLS: usize = 100;
pub const SKILL_API_MAX_NAME_BYTES: usize = 128;
pub const SKILL_API_MAX_LABEL_BYTES: usize = 4_096;
pub const SKILL_API_MAX_BODY_BYTES: usize = 1_048_576;
pub const SKILL_API_MAX_PATH_BYTES: usize = 1_024;
pub const SKILL_API_MAX_DIGEST_BYTES: usize = 128;
pub const SKILL_API_MAX_RESPONSE_BYTES: usize = 2_097_152;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(rename_all = "snake_case"))]
#[serde(rename_all = "snake_case")]
pub enum SkillDiagnosticSeverity {
    Error,
    Warning,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct SkillDiagnostic {
    pub severity: SkillDiagnosticSeverity,
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional))]
    pub source: Option<String>,
}

impl SkillDiagnostic {
    pub fn error(
        code: impl Into<String>,
        message: impl Into<String>,
        source: Option<String>,
    ) -> Self {
        Self {
            severity: SkillDiagnosticSeverity::Error,
            code: code.into(),
            message: message.into(),
            source,
        }
    }

    pub fn warning(
        code: impl Into<String>,
        message: impl Into<String>,
        source: Option<String>,
    ) -> Self {
        Self {
            severity: SkillDiagnosticSeverity::Warning,
            code: code.into(),
            message: message.into(),
            source,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(rename_all = "snake_case"))]
#[serde(rename_all = "snake_case")]
pub enum SkillSourceKind {
    Builtin,
    Workspace,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct SkillProvenance {
    pub kind: SkillSourceKind,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional))]
    pub virtual_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional, type = "number"))]
    pub revision: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional))]
    pub source_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional))]
    pub tree_digest: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(rename_all = "snake_case"))]
#[serde(rename_all = "snake_case")]
pub enum SkillActivationStatus {
    Active,
    Inactive,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(rename_all = "snake_case"))]
#[serde(rename_all = "snake_case")]
pub enum SkillProjectionStatus {
    Valid,
    Invalid,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct SkillProjectionIdentity {
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_u64))]
    pub config_revision: u64,
    pub tree_digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct SkillResourceRef {
    pub kind: String,
    pub name: String,
    pub supported: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional))]
    pub diagnostic: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct SkillCatalogEntry {
    pub name: String,
    pub description: String,
    pub activation_status: SkillActivationStatus,
    pub projection_status: SkillProjectionStatus,
    pub provenance: SkillProvenance,
    pub overrides: Vec<SkillProvenance>,
    pub diagnostics: Vec<SkillDiagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct SkillCatalogResponse {
    pub authority: String,
    pub projection: SkillProjectionIdentity,
    pub entries: Vec<SkillCatalogEntry>,
    pub diagnostics: Vec<SkillDiagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct SkillDetailResponse {
    pub authority: String,
    pub projection: SkillProjectionIdentity,
    pub name: String,
    pub description: String,
    pub provenance: SkillProvenance,
    pub overrides: Vec<SkillProvenance>,
    pub diagnostics: Vec<SkillDiagnostic>,
    pub activation_status: SkillActivationStatus,
    pub projection_status: SkillProjectionStatus,
    pub body: String,
    pub allowed_tools: Vec<String>,
    pub allowed_tools_status: String,
    pub resources: Vec<SkillResourceRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct SkillActivationResponse {
    pub name: String,
    pub provenance: SkillProvenance,
    #[serde(default)]
    pub diagnostics: Vec<SkillDiagnostic>,
    pub body: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillApiValidationError {
    CollectionTooLarge,
    StringTooLarge,
    InvalidProjectionIdentity,
    InvalidProvenance,
    InvalidVirtualPath,
    StaleProjection,
}

impl std::fmt::Display for SkillApiValidationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::CollectionTooLarge => "Skill API collection exceeds its limit",
            Self::StringTooLarge => "Skill API string exceeds its limit",
            Self::InvalidProjectionIdentity => "Skill API projection identity is invalid",
            Self::InvalidProvenance => "Skill API provenance is invalid",
            Self::InvalidVirtualPath => "Skill API virtual path is invalid",
            Self::StaleProjection => "Workspace Skill projection is stale",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for SkillApiValidationError {}

impl SkillProjectionIdentity {
    fn validate(&self) -> Result<(), SkillApiValidationError> {
        validate_safe_integer(self.config_revision)?;
        validate_nonempty_string(&self.tree_digest, SKILL_API_MAX_DIGEST_BYTES)
            .map_err(|_| SkillApiValidationError::InvalidProjectionIdentity)
    }
}

impl SkillProvenance {
    fn validate(
        &self,
        projection: &SkillProjectionIdentity,
    ) -> Result<(), SkillApiValidationError> {
        validate_nonempty_string(&self.id, SKILL_API_MAX_LABEL_BYTES)?;
        validate_optional_string(self.virtual_path.as_deref(), SKILL_API_MAX_PATH_BYTES)?;
        validate_optional_string(self.source_digest.as_deref(), SKILL_API_MAX_DIGEST_BYTES)?;
        validate_optional_string(self.tree_digest.as_deref(), SKILL_API_MAX_DIGEST_BYTES)?;
        if let Some(revision) = self.revision {
            validate_safe_integer(revision)?;
        }

        let expected_prefix = match self.kind {
            SkillSourceKind::Builtin => "builtin:",
            SkillSourceKind::Workspace => "workspace:",
        };
        if !self.id.starts_with(expected_prefix)
            || self
                .virtual_path
                .as_deref()
                .is_none_or(|path| !is_virtual_path(path))
            || self.source_digest.is_none()
        {
            return Err(SkillApiValidationError::InvalidProvenance);
        }

        match self.kind {
            SkillSourceKind::Builtin => {
                if self.revision.is_some() || self.tree_digest.is_some() {
                    return Err(SkillApiValidationError::InvalidProvenance);
                }
            }
            SkillSourceKind::Workspace => {
                let Some(revision) = self.revision else {
                    return Err(SkillApiValidationError::InvalidProvenance);
                };
                let Some(tree_digest) = self.tree_digest.as_deref() else {
                    return Err(SkillApiValidationError::InvalidProvenance);
                };
                if revision != projection.config_revision || tree_digest != projection.tree_digest {
                    return Err(SkillApiValidationError::StaleProjection);
                }
            }
        }
        Ok(())
    }
}

impl SkillCatalogEntry {
    fn validate(
        &self,
        projection: &SkillProjectionIdentity,
    ) -> Result<(), SkillApiValidationError> {
        validate_nonempty_string(&self.name, SKILL_API_MAX_NAME_BYTES)?;
        validate_string(&self.description, SKILL_API_MAX_LABEL_BYTES)?;
        validate_collection(&self.overrides, SKILL_API_MAX_OVERRIDES)?;
        validate_diagnostics(&self.diagnostics)?;
        self.provenance.validate(projection)?;
        for provenance in &self.overrides {
            provenance.validate(projection)?;
        }
        Ok(())
    }
}

impl SkillCatalogResponse {
    pub fn validate(&self) -> Result<(), SkillApiValidationError> {
        validate_nonempty_string(&self.authority, SKILL_API_MAX_LABEL_BYTES)?;
        if self.authority != SKILL_CATALOG_AUTHORITY {
            return Err(SkillApiValidationError::InvalidProjectionIdentity);
        }
        self.projection.validate()?;
        validate_collection(&self.entries, SKILL_API_MAX_CATALOG_ENTRIES)?;
        validate_diagnostics(&self.diagnostics)?;
        for entry in &self.entries {
            entry.validate(&self.projection)?;
        }
        Ok(())
    }
}

impl SkillDetailResponse {
    pub fn validate(&self) -> Result<(), SkillApiValidationError> {
        validate_nonempty_string(&self.authority, SKILL_API_MAX_LABEL_BYTES)?;
        if self.authority != SKILL_CATALOG_AUTHORITY {
            return Err(SkillApiValidationError::InvalidProjectionIdentity);
        }
        self.projection.validate()?;
        validate_nonempty_string(&self.name, SKILL_API_MAX_NAME_BYTES)?;
        validate_string(&self.description, SKILL_API_MAX_LABEL_BYTES)?;
        validate_string(&self.body, SKILL_API_MAX_BODY_BYTES)?;
        validate_strings(
            &self.allowed_tools,
            SKILL_API_MAX_ALLOWED_TOOLS,
            SKILL_API_MAX_LABEL_BYTES,
        )?;
        validate_nonempty_string(&self.allowed_tools_status, SKILL_API_MAX_LABEL_BYTES)?;
        validate_resources(&self.resources)?;
        validate_collection(&self.overrides, SKILL_API_MAX_OVERRIDES)?;
        validate_diagnostics(&self.diagnostics)?;
        self.provenance.validate(&self.projection)?;
        for provenance in &self.overrides {
            provenance.validate(&self.projection)?;
        }
        Ok(())
    }
}

fn validate_safe_integer(value: u64) -> Result<(), SkillApiValidationError> {
    if value <= SKILL_API_MAX_SAFE_INTEGER {
        Ok(())
    } else {
        Err(SkillApiValidationError::InvalidProjectionIdentity)
    }
}

fn validate_collection<T>(values: &[T], limit: usize) -> Result<(), SkillApiValidationError> {
    if values.len() <= limit {
        Ok(())
    } else {
        Err(SkillApiValidationError::CollectionTooLarge)
    }
}

fn validate_string(value: &str, limit: usize) -> Result<(), SkillApiValidationError> {
    if value.len() <= limit {
        Ok(())
    } else {
        Err(SkillApiValidationError::StringTooLarge)
    }
}

fn validate_nonempty_string(value: &str, limit: usize) -> Result<(), SkillApiValidationError> {
    validate_string(value, limit)?;
    if value.is_empty() {
        Err(SkillApiValidationError::StringTooLarge)
    } else {
        Ok(())
    }
}

fn validate_optional_string(
    value: Option<&str>,
    limit: usize,
) -> Result<(), SkillApiValidationError> {
    if let Some(value) = value {
        validate_nonempty_string(value, limit)?;
    }
    Ok(())
}

fn validate_strings(
    values: &[String],
    collection_limit: usize,
    string_limit: usize,
) -> Result<(), SkillApiValidationError> {
    validate_collection(values, collection_limit)?;
    for value in values {
        validate_nonempty_string(value, string_limit)?;
    }
    Ok(())
}

fn validate_resources(resources: &[SkillResourceRef]) -> Result<(), SkillApiValidationError> {
    validate_collection(resources, SKILL_API_MAX_RESOURCES)?;
    for resource in resources {
        validate_nonempty_string(&resource.kind, SKILL_API_MAX_LABEL_BYTES)?;
        validate_nonempty_string(&resource.name, SKILL_API_MAX_PATH_BYTES)?;
        if !is_virtual_path(&resource.name) {
            return Err(SkillApiValidationError::InvalidVirtualPath);
        }
        validate_optional_string(resource.diagnostic.as_deref(), SKILL_API_MAX_LABEL_BYTES)?;
    }
    Ok(())
}

fn is_virtual_path(value: &str) -> bool {
    !value.starts_with('/')
        && !value.contains('\\')
        && value
            .split('/')
            .all(|component| !component.is_empty() && component != "." && component != "..")
}

fn validate_diagnostics(diagnostics: &[SkillDiagnostic]) -> Result<(), SkillApiValidationError> {
    validate_collection(diagnostics, SKILL_API_MAX_DIAGNOSTICS)?;
    for diagnostic in diagnostics {
        validate_nonempty_string(&diagnostic.code, SKILL_API_MAX_LABEL_BYTES)?;
        validate_nonempty_string(&diagnostic.message, SKILL_API_MAX_LABEL_BYTES)?;
        validate_optional_string(diagnostic.source.as_deref(), SKILL_API_MAX_PATH_BYTES)?;
    }
    Ok(())
}

#[cfg(feature = "typescript")]
pub fn normalized_typescript(generated: String) -> String {
    let mut normalized = generated
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n");
    normalized.push('\n');
    normalized
}

#[cfg(feature = "typescript")]
pub fn legacy_catalog_typescript() -> String {
    use ts_rs::TS;

    let config = ts_rs::Config::default();
    let declarations = [
        WorkspaceSummary::decl(&config),
        WorkspaceCatalogListResponse::decl(&config),
        WorkspaceRepositoryRecord::decl(&config),
        WorkspaceCreateResponse::decl(&config),
        WorkspaceAuthConfig::decl(&config),
        WorkspacePermissionSummary::decl(&config),
        WorkspaceDeletionState::decl(&config),
        WorkspaceDeletionBlockerKind::decl(&config),
        WorkspaceDeletionBlocker::decl(&config),
        WorkspaceDeletionResourceCounts::decl(&config),
        WorkspaceDeletionPreflightResponse::decl(&config),
        WorkspaceDeletionRequest::decl(&config),
        WorkspaceDeletionOperationResponse::decl(&config),
        DiagnosticSeverity::decl(&config),
        Diagnostic::decl(&config),
        WorkspaceExtensionPointState::decl(&config),
        WorkspaceExtensionPoints::decl(&config),
        WorkspaceResponse::decl(&config),
        WorkspaceMetadataSettingsResponse::decl(&config),
        UpdateWorkspaceMetadataRequest::decl(&config),
        WorkspaceMetadataMutationResponse::decl(&config),
        WorkspaceSigningIdentityState::decl(&config),
        WorkspaceSigningIdentityPublic::decl(&config),
        WorkspacePublicIdentityBundle::decl(&config),
        WorkspaceSigningIdentityResponse::decl(&config),
        ConfigContentType::decl(&config),
        ConfigEntry::decl(&config),
        ConfigTreeSnapshot::decl(&config),
        ConfigTreeChange::decl(&config),
        ConfigProjectionValidator::decl(&config),
        ConfigSchemaContribution::decl(&config),
        WorkspaceConfigSchemaBundle::decl(&config),
        ToolchainContract::decl(&config),
        WorkspaceConfigTreeResponse::decl(&config),
        ConfigCommitRequest::decl(&config),
        ProfileSettingsResponse::decl(&config),
        WorkspaceProfileSummary::decl(&config),
        WorkspaceProfileSourceSummary::decl(&config),
        WorkspaceProfileSourceProvenance::decl(&config),
        GitCommitSummary::decl(&config),
        RepositorySshConnectionProbeRequest::decl(&config),
        RepositorySshHostKeyCandidate::decl(&config),
        RepositorySshConnectionTrustState::decl(&config),
        RepositorySshConnectionProbeResponse::decl(&config),
        ConfirmRepositorySshHostTrustRequest::decl(&config),
        RepositoryLogResponse::decl(&config),
        RuntimeSourceKind::decl(&config),
        RuntimeSourceStatus::decl(&config),
        RuntimeIdentityAuthority::decl(&config),
        RuntimeSourceSummary::decl(&config),
        RuntimeSummary::decl(&config),
        WorkspaceRuntimeBindingState::decl(&config),
        RuntimeConnectionDisplayState::decl(&config),
        RuntimeVerificationOutcome::decl(&config),
        RuntimeVerificationEvidenceSummary::decl(&config),
        WorkspaceRuntimeBindingSummary::decl(&config),
        RuntimeManagementSummary::decl(&config),
        WorkspaceRuntimeResource::decl(&config),
        RuntimeTrustKeyStatus::decl(&config),
        RuntimeTrustKeyState::decl(&config),
        RuntimeTrustAuditAction::decl(&config),
        RuntimeTrustAuditEntry::decl(&config),
        WorkspaceRuntimeDetail::decl(&config),
        RuntimeTrustKeyRevealResponse::decl(&config),
        RevokeRuntimeTrustKeyRequest::decl(&config),
        RemoveRuntimeRequest::decl(&config),
        RuntimeRemovalOperationState::decl(&config),
        RuntimeRemovalOperationResponse::decl(&config),
        RuntimeTrustConflictKind::decl(&config),
        RuntimeTrustConflictResponse::decl(&config),
        RuntimePublicIdentityBundle::decl(&config),
        CreateRemoteRuntimeRequest::decl(&config),
        UpdateRemoteRuntimeRequest::decl(&config),
        RuntimeConnectionTestStatus::decl(&config),
        RuntimeConnectionTestFailureKind::decl(&config),
        RuntimeConnectionTestResponse::decl(&config),
    ]
    .map(|declaration| format!("export {declaration}"));

    format!(
        "// This file is generated by `cargo run -p server-api --features typescript --example generate_legacy_typescript | deno fmt -`.\n// Do not edit this file directly.\n\nimport type {{ RepositoryObservedStatus, RepositorySource }} from \"./repository-api\";\n\n{}\n",
        declarations.join("\n\n")
    )
}

#[cfg(feature = "typescript")]
pub fn repository_access_api_typescript() -> String {
    use ts_rs::TS;

    let config = ts_rs::Config::default();
    let declarations = [
        RepositorySshCredential::decl(&config),
        CreateRepositorySshCredentialRequest::decl(&config),
        GenerateRepositorySshCredentialRequest::decl(&config),
        RepositorySshPublicKey::decl(&config),
        RotateRepositorySshCredentialRequest::decl(&config),
        DeleteRepositorySshCredentialRequest::decl(&config),
        RepositorySshHostTrust::decl(&config),
        PutRepositorySshHostTrustRequest::decl(&config),
        DeleteRepositorySshHostTrustRequest::decl(&config),
        RepositoryAccessMode::decl(&config),
        RepositorySshAccessBinding::decl(&config),
        RepositoryAccessProjection::decl(&config),
    ];
    format!(
        "// Generated from server-api. Do not edit by hand.\n// Regenerate: cargo run -q -p server-api --features typescript --example generate_repository_access_types > web/workspace/src/lib/generated/repository-access-api.ts\n\n{}\n",
        declarations
            .into_iter()
            .map(|declaration| format!("export {declaration}"))
            .collect::<Vec<_>>()
            .join("\n\n")
    )
}

#[cfg(feature = "typescript")]
pub fn skill_api_typescript() -> String {
    use ts_rs::TS;

    let config = ts_rs::Config::default();
    let declarations = [
        SkillDiagnosticSeverity::decl(&config),
        SkillDiagnostic::decl(&config),
        SkillSourceKind::decl(&config),
        SkillProvenance::decl(&config),
        SkillActivationStatus::decl(&config),
        SkillProjectionStatus::decl(&config),
        SkillProjectionIdentity::decl(&config),
        SkillResourceRef::decl(&config),
        SkillCatalogEntry::decl(&config),
        SkillCatalogResponse::decl(&config),
        SkillDetailResponse::decl(&config),
        SkillActivationResponse::decl(&config),
    ];
    let limits = format!(
        "export const SKILL_API_AUTHORITY = \"{SKILL_CATALOG_AUTHORITY}\" as const;\n\nexport const SKILL_API_LIMITS = {{\n  maxSafeInteger: {SKILL_API_MAX_SAFE_INTEGER},\n  maxCatalogEntries: {SKILL_API_MAX_CATALOG_ENTRIES},\n  maxOverrides: {SKILL_API_MAX_OVERRIDES},\n  maxDiagnostics: {SKILL_API_MAX_DIAGNOSTICS},\n  maxResources: {SKILL_API_MAX_RESOURCES},\n  maxAllowedTools: {SKILL_API_MAX_ALLOWED_TOOLS},\n  maxNameBytes: {SKILL_API_MAX_NAME_BYTES},\n  maxLabelBytes: {SKILL_API_MAX_LABEL_BYTES},\n  maxBodyBytes: {SKILL_API_MAX_BODY_BYTES},\n  maxPathBytes: {SKILL_API_MAX_PATH_BYTES},\n  maxDigestBytes: {SKILL_API_MAX_DIGEST_BYTES},\n  maxResponseBytes: {SKILL_API_MAX_RESPONSE_BYTES},\n}} as const;"
    );
    format!(
        "// Generated from server-api. Do not edit by hand.\n// Regenerate: cargo run -q -p server-api --features typescript --example generate_skill_api_types > web/workspace/src/lib/generated/skill-api.ts\n\n{limits}\n\n{}\n",
        declarations
            .into_iter()
            .map(|declaration| format!("export {declaration}"))
            .collect::<Vec<_>>()
            .join("\n\n")
    )
}

#[cfg(feature = "typescript")]
pub fn auth_api_typescript() -> String {
    use ts_rs::TS;

    let config = ts_rs::Config::default();
    let declarations = [
        AuthPublicConfig::decl(&config),
        ActorAuthMethod::decl(&config),
        AuthenticatedUser::decl(&config),
        RequestActor::decl(&config),
        WhoamiResponse::decl(&config),
        AuthBootstrapUserRequest::decl(&config),
        AuthUserResponse::decl(&config),
        PasskeyRegistrationOptionsRequest::decl(&config),
        PasskeyRegistrationOptionsResponse::decl(&config),
        PasskeyRegistrationCompleteRequest::decl(&config),
        PasskeyLoginOptionsRequest::decl(&config),
        PasskeyLoginOptionsResponse::decl(&config),
        PasskeyLoginCompleteRequest::decl(&config),
        DeviceLoginStartRequest::decl(&config),
        DeviceLoginStartResponse::decl(&config),
        DeviceLoginApproveRequest::decl(&config),
        DeviceLoginApprovalStatus::decl(&config),
        DeviceLoginApproveResponse::decl(&config),
        DeviceLoginPollRequest::decl(&config),
        DeviceAccessTokenType::decl(&config),
        DeviceLoginPollStatus::decl(&config),
        DeviceLoginPollResponse::decl(&config),
        LogoutStatus::decl(&config),
        LogoutResponse::decl(&config),
    ];
    format!(
        "// Generated from server-api. Do not edit by hand.\n// Regenerate: cargo run -q -p server-api --features typescript --example generate_auth_api_types > web/workspace/src/lib/generated/auth-api.ts\n\n{}\n",
        declarations
            .into_iter()
            .map(|declaration| format!("export {declaration}"))
            .collect::<Vec<_>>()
            .join("\n\n")
    )
}

#[cfg(feature = "typescript")]
pub fn workdir_api_typescript() -> String {
    use ts_rs::TS;

    let config = ts_rs::Config::default();
    let declarations = [
        DiagnosticSeverity::decl(&config),
        Diagnostic::decl(&config),
        WorkingDirectoryMaterializerKind::decl(&config),
        WorkingDirectoryStatusKind::decl(&config),
        WorkingDirectoryCleanupTarget::decl(&config),
        WorkingDirectoryOccupancy::decl(&config),
        WorkingDirectorySource::decl(&config),
        WorkingDirectorySummary::decl(&config),
        WorkingDirectoryCreateRequest::decl(&config),
        WorkingDirectoryListResponse::decl(&config),
        WorkingDirectoryDetailResponse::decl(&config),
        WorkingDirectoryCreateResponse::decl(&config),
    ];
    format!(
        "// Generated from server-api. Do not edit by hand.\n// Regenerate: cargo run -q -p server-api --features typescript --example generate_workdir_api_types > web/workspace/src/lib/generated/workdir-api.ts\n\n{}\n",
        declarations
            .into_iter()
            .map(|declaration| format!("export {declaration}"))
            .collect::<Vec<_>>()
            .join("\n\n")
    )
}

#[cfg(feature = "typescript")]
pub fn worker_launch_api_typescript() -> String {
    use ts_rs::TS;

    let config = ts_rs::Config::default();
    let declarations = [
        DiagnosticSeverity::decl(&config),
        Diagnostic::decl(&config),
        WorkingDirectoryMaterializerKind::decl(&config),
        WorkingDirectoryStatusKind::decl(&config),
        WorkingDirectoryCleanupTarget::decl(&config),
        RuntimeWorkingDirectoryCleanupTarget::decl(&config),
        RuntimeWorkingDirectorySummary::decl(&config),
        WorkingDirectoryOccupancy::decl(&config),
        WorkingDirectorySource::decl(&config),
        WorkingDirectorySummary::decl(&config),
        WorkerWorkspaceSummary::decl(&config),
        WorkerImplementationSummary::decl(&config),
        WorkerCapabilitySummary::decl(&config),
        RuntimeWorkerWorkdirAttachmentSummary::decl(&config),
        WorkerLaunchWorkerSummary::decl(&config),
        WorkerLaunchRuntimeOption::decl(&config),
        WorkerLaunchProfileCandidate::decl(&config),
        WorkingDirectoryRepositoryOption::decl(&config),
        WorkerLaunchOptionsResponse::decl(&config),
        BrowserWorkerWorkingDirectorySelection::decl(&config),
        CreateWorkspaceWorkerTicketAssignmentRequest::decl(&config),
        CreateWorkspaceWorkerRequest::decl(&config),
        BrowserCreateWorkerResponse::decl(&config),
        BrowserWorkspaceOrchestratorResponse::decl(&config),
    ];
    format!(
        "// Generated from server-api. Do not edit by hand.\n// Regenerate: cargo run -q -p server-api --features typescript --example generate_worker_launch_api_types > web/workspace/src/lib/generated/worker-launch-api.ts\n\nimport type {{ Segment }} from \"./protocol\";\n\n{}\n",
        declarations
            .into_iter()
            .map(|declaration| format!("export {declaration}"))
            .collect::<Vec<_>>()
            .join("\n\n")
    )
}

#[cfg(all(test, feature = "typescript"))]
mod worker_launch_typescript_tests {
    #[test]
    fn generated_worker_launch_api_contract_is_current() {
        let expected = super::worker_launch_api_typescript();
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../web/workspace/src/lib/generated/worker-launch-api.ts");
        let actual = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        assert_eq!(
            normalize(&actual),
            normalize(&expected),
            "regenerate Worker launch API TypeScript types with `cargo run -q -p server-api --features typescript --example generate_worker_launch_api_types > web/workspace/src/lib/generated/worker-launch-api.ts` and format the generated file",
        );
    }

    fn normalize(value: &str) -> String {
        value
            .chars()
            .filter_map(|character| match character {
                character if character.is_whitespace() => None,
                ',' => Some(';'),
                character => Some(character),
            })
            .collect::<String>()
            .replace("=|", "=")
    }
}

#[cfg(all(test, feature = "typescript"))]
mod memory_typescript_tests {
    #[test]
    fn generated_memory_api_contract_is_current() {
        let expected = super::memory_api_typescript();
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../web/workspace/src/lib/generated/memory-api.ts");
        let actual = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        assert_eq!(
            normalize(&actual),
            normalize(&expected),
            "regenerate Memory API TypeScript types with `cargo run -q -p server-api --features typescript --example generate_memory_api_types > web/workspace/src/lib/generated/memory-api.ts` and format the generated file",
        );
    }

    fn normalize(value: &str) -> String {
        value
            .chars()
            .filter_map(|character| match character {
                character if character.is_whitespace() => None,
                ',' => Some(';'),
                character => Some(character),
            })
            .collect::<String>()
            .replace("=|", "=")
            .replace(";}", "}")
    }
}

#[cfg(all(test, feature = "typescript"))]
mod skill_typescript_tests {
    #[test]
    fn generated_skill_api_contract_is_current() {
        let expected = super::skill_api_typescript();
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../web/workspace/src/lib/generated/skill-api.ts");
        let actual = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        assert_eq!(
            normalize(&actual),
            normalize(&expected),
            "regenerate Skill API TypeScript types with `cargo run -q -p server-api --features typescript --example generate_skill_api_types > web/workspace/src/lib/generated/skill-api.ts` and format the generated file",
        );
    }

    fn normalize(value: &str) -> String {
        value
            .chars()
            .filter_map(|character| match character {
                character if character.is_whitespace() => None,
                ',' => Some(';'),
                character => Some(character),
            })
            .collect::<String>()
            .replace("=|", "=")
            .replace(";}", "}")
    }
}

#[cfg(all(test, feature = "typescript"))]
mod workdir_typescript_tests {
    #[test]
    fn generated_workdir_api_contract_is_current() {
        let expected = super::workdir_api_typescript();
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../web/workspace/src/lib/generated/workdir-api.ts");
        let actual = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        assert_eq!(
            normalize(&actual),
            normalize(&expected),
            "regenerate Workdir API TypeScript types with `cargo run -q -p server-api --features typescript --example generate_workdir_api_types > web/workspace/src/lib/generated/workdir-api.ts` and format the generated file",
        );
    }

    fn normalize(value: &str) -> String {
        value
            .chars()
            .filter_map(|character| match character {
                character if character.is_whitespace() => None,
                ',' => Some(';'),
                character => Some(character),
            })
            .collect::<String>()
            .replace("=|", "=")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn worker_session_contract_and_flattened_availability_are_stable() {
        let operations = ServerApiMetadata::OPERATIONS;
        let operation = operations
            .iter()
            .find(|operation| operation.operation_id == "worker_session")
            .expect("worker-session operation must remain in ServerApi metadata");
        assert_eq!(operation.method, HttpMethod::Get);
        assert_eq!(
            operation.path,
            "/api/w/{workspace_id}/runtimes/{runtime_id}/workers/{worker_id}/session"
        );

        let response = WorkspaceWorkerSessionResponse {
            subject: WorkspaceWorkerSubject::RuntimeWorker {
                runtime_id: "runtime-a".to_string(),
                worker_id: "worker-a".to_string(),
            },
            observation: runtime_api::WorkerSessionAvailability::LiveProtocol,
        };
        let value = serde_json::to_value(response).unwrap();
        assert_eq!(value["availability"], "live_protocol");
        assert_eq!(value["subject"]["kind"], "runtime_worker");
    }

    #[test]
    fn auth_and_workspace_catalog_operations_are_in_server_api_metadata() {
        let operations = ServerApiMetadata::OPERATIONS;
        for (operation_id, method, path) in [
            ("health", HttpMethod::Get, "/health"),
            ("auth_config", HttpMethod::Get, "/api/auth/config"),
            (
                "auth_passkey_registration_options",
                HttpMethod::Post,
                "/api/auth/passkeys/registration/options",
            ),
            (
                "auth_passkey_login_options",
                HttpMethod::Post,
                "/api/auth/passkeys/login/options",
            ),
            (
                "auth_device_login_start",
                HttpMethod::Post,
                "/api/auth/device-login/start",
            ),
            (
                "auth_device_login_approve",
                HttpMethod::Post,
                "/api/auth/device-login/approve",
            ),
            (
                "auth_device_login_poll",
                HttpMethod::Post,
                "/api/auth/device-login/poll",
            ),
            ("auth_whoami", HttpMethod::Get, "/api/auth/whoami"),
            ("workspace_catalog_list", HttpMethod::Get, "/api/workspaces"),
            (
                "workspace_catalog_create",
                HttpMethod::Post,
                "/api/workspaces",
            ),
            (
                "workspace_deletion_preflight",
                HttpMethod::Get,
                "/api/workspaces/{workspace_id}/deletion",
            ),
            (
                "workspace_deletion_start",
                HttpMethod::Post,
                "/api/workspaces/{workspace_id}/deletion",
            ),
            (
                "workspace_deletion_get",
                HttpMethod::Get,
                "/api/workspace-deletions/{operation_id}",
            ),
        ] {
            let operation = operations
                .iter()
                .find(|operation| operation.operation_id == operation_id)
                .unwrap_or_else(|| panic!("missing ServerApi operation {operation_id}"));
            assert_eq!(operation.method, method, "{operation_id}");
            assert_eq!(operation.path, path, "{operation_id}");
        }
    }

    #[test]
    fn ticket_objective_and_merge_request_operations_are_in_server_api_metadata() {
        let operations = ServerApiMetadata::OPERATIONS;
        assert_eq!(
            operations
                .iter()
                .filter(|operation| operation.operation_id.starts_with("ticket_"))
                .count(),
            37
        );
        assert_eq!(
            operations
                .iter()
                .filter(|operation| operation.operation_id.starts_with("objective_"))
                .count(),
            11
        );
        assert_eq!(
            operations
                .iter()
                .filter(|operation| operation.operation_id.starts_with("merge_request_"))
                .count(),
            11
        );
        for (operation_id, method, path) in [
            (
                "ticket_relation_query",
                HttpMethod::Post,
                "/api/w/{workspace_id}/tickets/relations/search",
            ),
            (
                "ticket_orchestration_plan_query",
                HttpMethod::Post,
                "/api/w/{workspace_id}/tickets/orchestration-plans/search",
            ),
            (
                "merge_request_complete",
                HttpMethod::Post,
                "/api/w/{workspace_id}/tickets/{id}/merge-request/complete",
            ),
            (
                "objective_ticket_unlink",
                HttpMethod::Delete,
                "/api/w/{workspace_id}/objectives/{objective_id}/ticket-links/{ticket_id}",
            ),
        ] {
            let operation = operations
                .iter()
                .find(|operation| operation.operation_id == operation_id)
                .unwrap_or_else(|| panic!("missing ServerApi operation {operation_id}"));
            assert_eq!(operation.method, method, "{operation_id}");
            assert_eq!(operation.path, path, "{operation_id}");
        }
    }

    #[test]
    fn workspace_configuration_domain_operations_are_in_server_api_metadata() {
        let operations = ServerApiMetadata::OPERATIONS;
        for (operation_id, method, path) in [
            ("workspace_current", HttpMethod::Get, "/api/workspace"),
            (
                "workspace_scoped",
                HttpMethod::Get,
                "/api/w/{workspace_id}/workspace",
            ),
            (
                "workspace_metadata_settings",
                HttpMethod::Get,
                "/api/w/{workspace_id}/settings",
            ),
            (
                "workspace_metadata_settings_update",
                HttpMethod::Put,
                "/api/w/{workspace_id}/settings",
            ),
            (
                "workspace_signing_identity",
                HttpMethod::Get,
                "/api/w/{workspace_id}/settings/signing-identity",
            ),
            (
                "workspace_signing_identity_provision",
                HttpMethod::Post,
                "/api/w/{workspace_id}/settings/signing-identity/provision",
            ),
            (
                "workspace_memory_settings",
                HttpMethod::Get,
                "/api/w/{workspace_id}/settings/memory",
            ),
            (
                "workspace_memory_settings_update",
                HttpMethod::Put,
                "/api/w/{workspace_id}/settings/memory",
            ),
            (
                "repository_access_projection",
                HttpMethod::Get,
                "/api/w/{workspace_id}/settings/repository-access",
            ),
            (
                "repository_ssh_credential_list",
                HttpMethod::Get,
                "/api/w/{workspace_id}/settings/repository-access/credentials",
            ),
            (
                "repository_ssh_credential_create",
                HttpMethod::Post,
                "/api/w/{workspace_id}/settings/repository-access/credentials",
            ),
            (
                "repository_ssh_credential_generate",
                HttpMethod::Post,
                "/api/w/{workspace_id}/settings/repository-access/credentials/generate",
            ),
            (
                "repository_ssh_credential_get",
                HttpMethod::Get,
                "/api/w/{workspace_id}/settings/repository-access/credentials/{credential_id}",
            ),
            (
                "repository_ssh_credential_delete",
                HttpMethod::Delete,
                "/api/w/{workspace_id}/settings/repository-access/credentials/{credential_id}",
            ),
            (
                "repository_ssh_credential_public_key",
                HttpMethod::Get,
                "/api/w/{workspace_id}/settings/repository-access/credentials/{credential_id}/public-key",
            ),
            (
                "repository_ssh_credential_rotate",
                HttpMethod::Post,
                "/api/w/{workspace_id}/settings/repository-access/credentials/{credential_id}/rotate",
            ),
            (
                "repository_ssh_host_trust_list",
                HttpMethod::Get,
                "/api/w/{workspace_id}/settings/repository-access/host-trusts",
            ),
            (
                "repository_ssh_host_trust_put",
                HttpMethod::Post,
                "/api/w/{workspace_id}/settings/repository-access/host-trusts",
            ),
            (
                "repository_ssh_host_trust_get",
                HttpMethod::Get,
                "/api/w/{workspace_id}/settings/repository-access/host-trusts/{host_trust_id}",
            ),
            (
                "repository_ssh_host_trust_delete",
                HttpMethod::Delete,
                "/api/w/{workspace_id}/settings/repository-access/host-trusts/{host_trust_id}",
            ),
            (
                "workspace_config_tree",
                HttpMethod::Get,
                "/api/w/{workspace_id}/config/source-tree",
            ),
            (
                "workspace_prompt_projection",
                HttpMethod::Get,
                "/api/w/{workspace_id}/config/projections/prompts",
            ),
            (
                "workspace_config_tree_commit",
                HttpMethod::Post,
                "/api/w/{workspace_id}/config/source-tree/commit",
            ),
            (
                "workspace_config_revision",
                HttpMethod::Get,
                "/api/w/{workspace_id}/config/source-tree/revisions/{revision}",
            ),
            (
                "workspace_config_entry",
                HttpMethod::Get,
                "/api/w/{workspace_id}/config/source-tree/entries/{path}",
            ),
            (
                "profile_settings",
                HttpMethod::Get,
                "/api/w/{workspace_id}/settings/profiles",
            ),
            ("flow_list", HttpMethod::Get, "/api/w/{workspace_id}/flows"),
            ("flow_put", HttpMethod::Put, "/api/w/{workspace_id}/flows"),
            (
                "flow_resolve",
                HttpMethod::Post,
                "/api/w/{workspace_id}/flows/resolve",
            ),
            (
                "flow_get",
                HttpMethod::Get,
                "/api/w/{workspace_id}/flows/{flow_id}",
            ),
            (
                "memory_document",
                HttpMethod::Get,
                "/api/w/{workspace_id}/memory",
            ),
            (
                "memory_staging_list",
                HttpMethod::Get,
                "/api/w/{workspace_id}/memory/staging",
            ),
            (
                "memory_backend",
                HttpMethod::Post,
                "/api/w/{workspace_id}/memory/backend",
            ),
            (
                "memory_consolidation",
                HttpMethod::Post,
                "/api/w/{workspace_id}/memory/consolidation",
            ),
            (
                "skill_list",
                HttpMethod::Get,
                "/api/w/{workspace_id}/skills",
            ),
            (
                "skill_lint",
                HttpMethod::Get,
                "/api/w/{workspace_id}/skills/lint",
            ),
            (
                "skill_get",
                HttpMethod::Get,
                "/api/w/{workspace_id}/skills/{name}",
            ),
            (
                "skill_activate",
                HttpMethod::Get,
                "/api/w/{workspace_id}/skills/{name}/activate",
            ),
        ] {
            let operation = operations
                .iter()
                .find(|operation| operation.operation_id == operation_id)
                .unwrap_or_else(|| panic!("missing ServerApi operation {operation_id}"));
            assert_eq!(operation.method, method, "{operation_id}");
            assert_eq!(operation.path, path, "{operation_id}");
        }
    }

    #[test]
    fn worker_restore_state_round_trips_and_rejects_unknown_variants() {
        for (state, wire) in [
            (WorkerRestoreState::Accepted, "accepted"),
            (WorkerRestoreState::Rejected, "rejected"),
            (WorkerRestoreState::RolledBack, "rolled_back"),
            (
                WorkerRestoreState::ReconciliationRequired,
                "reconciliation_required",
            ),
        ] {
            assert_eq!(
                serde_json::to_value(state).unwrap(),
                serde_json::json!(wire)
            );
            assert_eq!(
                serde_json::from_value::<WorkerRestoreState>(serde_json::json!(wire)).unwrap(),
                state
            );
        }
        assert!(
            serde_json::from_value::<WorkerRestoreState>(serde_json::json!("unknown")).is_err()
        );
    }

    #[test]
    fn worker_restore_result_rejects_unknown_fields() {
        let result = serde_json::from_value::<WorkerRestoreResult>(serde_json::json!({
            "state": "rejected",
            "worker": null,
            "diagnostics": [],
            "unexpected": true
        }));
        assert!(result.is_err());
    }

    #[test]
    fn historical_http_repository_source_kind_decodes_as_invalid_evidence() {
        let source: RepositorySource = serde_json::from_value(serde_json::json!({
            "kind": "http",
            "uri": "http://git.example.test/team/project.git",
            "revision": 1,
        }))
        .unwrap();

        assert_eq!(source.kind, RepositorySourceKind::Invalid);
        assert_eq!(
            serde_json::to_value(source).unwrap()["kind"],
            serde_json::json!("invalid")
        );
        assert_eq!(
            RepositorySourceKind::parse("http"),
            Some(RepositorySourceKind::Invalid)
        );
        assert!(!RepositorySourceKind::Invalid.is_remote());
    }

    fn skill_projection() -> SkillProjectionIdentity {
        SkillProjectionIdentity {
            config_revision: 42,
            tree_digest: "tree-digest".to_string(),
        }
    }

    fn builtin_skill_provenance() -> SkillProvenance {
        SkillProvenance {
            kind: SkillSourceKind::Builtin,
            id: "builtin:errors".to_string(),
            virtual_path: Some("skills/errors/SKILL.md".to_string()),
            revision: None,
            source_digest: Some("builtin-source-digest".to_string()),
            tree_digest: None,
        }
    }

    fn workspace_skill_provenance() -> SkillProvenance {
        SkillProvenance {
            kind: SkillSourceKind::Workspace,
            id: "workspace:skills/release/SKILL.md".to_string(),
            virtual_path: Some("skills/release/SKILL.md".to_string()),
            revision: Some(42),
            source_digest: Some("workspace-source-digest".to_string()),
            tree_digest: Some("tree-digest".to_string()),
        }
    }

    #[test]
    fn skill_catalog_round_trips_builtin_workspace_and_invalid_projection_entries() {
        let response = SkillCatalogResponse {
            authority: "workspace-config-skills-v1".to_string(),
            projection: skill_projection(),
            entries: vec![
                SkillCatalogEntry {
                    name: "errors".to_string(),
                    description: "Builtin guidance".to_string(),
                    activation_status: SkillActivationStatus::Active,
                    projection_status: SkillProjectionStatus::Valid,
                    provenance: builtin_skill_provenance(),
                    overrides: vec![],
                    diagnostics: vec![],
                },
                SkillCatalogEntry {
                    name: "release".to_string(),
                    description: "Workspace guidance".to_string(),
                    activation_status: SkillActivationStatus::Inactive,
                    projection_status: SkillProjectionStatus::Invalid,
                    provenance: workspace_skill_provenance(),
                    overrides: vec![builtin_skill_provenance()],
                    diagnostics: vec![SkillDiagnostic {
                        severity: SkillDiagnosticSeverity::Error,
                        code: "invalid_projection".to_string(),
                        message: "invalid projected Skill".to_string(),
                        source: Some("skills/release/SKILL.md".to_string()),
                    }],
                },
            ],
            diagnostics: vec![],
        };

        response.validate().expect("fixture should be valid");
        let json = serde_json::to_string(&response).expect("serialize Skill catalog");
        let decoded: SkillCatalogResponse =
            serde_json::from_str(&json).expect("deserialize Skill catalog");
        assert_eq!(decoded, response);
        assert!(!json.contains("\"revision\":null"));
        assert!(!json.contains("\"tree_digest\":null"));
    }

    #[test]
    fn skill_detail_round_trips_shared_response() {
        let response = SkillDetailResponse {
            authority: "workspace-config-skills-v1".to_string(),
            projection: skill_projection(),
            name: "release".to_string(),
            description: "Workspace guidance".to_string(),
            body: "# Release\n".to_string(),
            allowed_tools: vec!["Bash".to_string()],
            allowed_tools_status: "experimental_hint_only".to_string(),
            resources: vec![],
            activation_status: SkillActivationStatus::Active,
            projection_status: SkillProjectionStatus::Valid,
            provenance: workspace_skill_provenance(),
            overrides: vec![],
            diagnostics: vec![],
        };

        response.validate().expect("fixture should be valid");
        let decoded: SkillDetailResponse = serde_json::from_value(
            serde_json::to_value(&response).expect("serialize Skill detail"),
        )
        .expect("deserialize Skill detail");
        assert_eq!(decoded, response);
    }

    #[test]
    fn skill_projection_validation_detects_stale_workspace_revision() {
        let mut provenance = workspace_skill_provenance();
        provenance.revision = Some(41);
        let response = SkillCatalogResponse {
            authority: "workspace-config-skills-v1".to_string(),
            projection: skill_projection(),
            entries: vec![SkillCatalogEntry {
                name: "release".to_string(),
                description: String::new(),
                activation_status: SkillActivationStatus::Active,
                projection_status: SkillProjectionStatus::Valid,
                provenance,
                overrides: vec![],
                diagnostics: vec![],
            }],
            diagnostics: vec![],
        };

        assert_eq!(
            response.validate(),
            Err(SkillApiValidationError::StaleProjection)
        );
    }

    #[test]
    fn skill_dto_rejects_unknown_fields_and_unknown_provenance_kind() {
        let unknown_field = serde_json::json!({
            "authority": "workspace-config-skills-v1",
            "projection": {"config_revision": 42, "tree_digest": "tree-digest"},
            "entries": [],
            "diagnostics": [],
            "body": "must not be accepted"
        });
        assert!(serde_json::from_value::<SkillCatalogResponse>(unknown_field).is_err());

        let mut provenance =
            serde_json::to_value(workspace_skill_provenance()).expect("serialize provenance");
        provenance["kind"] = serde_json::Value::String("newer_source_kind".to_string());
        assert!(serde_json::from_value::<SkillProvenance>(provenance).is_err());
    }

    #[test]
    fn memory_evidence_origins_round_trip_as_typed_provenance() {
        let kinds = [
            MemoryEvidenceOriginKind::HumanInput,
            MemoryEvidenceOriginKind::WorkerInput,
            MemoryEvidenceOriginKind::FlowInstruction,
            MemoryEvidenceOriginKind::BackendInstruction,
            MemoryEvidenceOriginKind::ModelOutput,
            MemoryEvidenceOriginKind::ToolOutput,
            MemoryEvidenceOriginKind::DerivedSummary,
            MemoryEvidenceOriginKind::LegacyUnknown,
        ];
        for kind in kinds {
            let origin = MemoryEvidenceOrigin {
                kind,
                account_id: Some("account-1".to_string()),
                workspace_id: Some("workspace-1".to_string()),
                runtime_id: Some("runtime-1".to_string()),
                worker_id: Some("worker-1".to_string()),
                flow_selector: Some("builtin:coder-review".to_string()),
                flow_definition_id: Some("flow-1".to_string()),
                flow_definition_revision: Some(7),
            };
            let encoded = serde_json::to_value(&origin).unwrap();
            let decoded: MemoryEvidenceOrigin = serde_json::from_value(encoded).unwrap();
            assert_eq!(decoded, origin);
        }
    }

    #[test]
    fn memory_evidence_origin_rejects_unknown_kind_and_fields() {
        assert!(
            serde_json::from_value::<MemoryEvidenceOrigin>(
                serde_json::json!({"kind": "future_origin"})
            )
            .is_err()
        );
        assert!(
            serde_json::from_value::<MemoryEvidenceOrigin>(serde_json::json!({
                "kind": "human_input",
                "future_field": "not current schema"
            }))
            .is_err()
        );
    }

    fn worker_launch_summary() -> WorkerLaunchWorkerSummary {
        WorkerLaunchWorkerSummary {
            runtime_id: "runtime-a".to_string(),
            worker_id: "worker-a".to_string(),
            host_id: "host-a".to_string(),
            display_name: "Worker A".to_string(),
            label: "worker-a".to_string(),
            profile: None,
            singleton_key: None,
            tags: Vec::new(),
            workspace: WorkerWorkspaceSummary {
                visibility: "workspace".to_string(),
                identity: "workspace-a".to_string(),
                workspace_id: Some("workspace-a".to_string()),
            },
            state: "idle".to_string(),
            last_seen_at: None,
            pinned: false,
            retention_state: "active".to_string(),
            implementation: WorkerImplementationSummary {
                kind: "runtime".to_string(),
                display_hint: "Runtime Worker".to_string(),
            },
            capabilities: WorkerCapabilitySummary {
                can_stop: true,
                can_spawn_followup: false,
            },
            workdir_attachments: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    #[test]
    fn remote_runtime_metadata_update_cannot_carry_public_key_authority() {
        let request = UpdateRemoteRuntimeRequest {
            display_name: Some("Runtime A".to_string()),
            endpoint: "https://runtime.example.test".to_string(),
        };
        assert_eq!(
            serde_json::to_value(&request).unwrap(),
            serde_json::json!({
                "display_name": "Runtime A",
                "endpoint": "https://runtime.example.test",
            })
        );
        assert!(
            serde_json::from_value::<UpdateRemoteRuntimeRequest>(serde_json::json!({
                "display_name": "Runtime A",
                "endpoint": "https://runtime.example.test",
                "public_bundle": {
                    "identity_id": "runtime-a",
                    "public_key": "yoi-ed25519-pub:v1:not-accepted",
                },
            }))
            .is_err(),
            "metadata updates must reject public key fields"
        );
    }

    #[test]
    fn worker_launch_optional_omission_and_request_shape_are_stable() {
        assert_eq!(
            serde_json::to_value(WorkingDirectoryRepositoryOption {
                repository_key: "main".to_string(),
                default_selector: None,
            })
            .unwrap(),
            serde_json::json!({ "repository_key": "main" })
        );

        let orchestrator = serde_json::to_value(BrowserWorkspaceOrchestratorResponse {
            workspace_id: "workspace-a".to_string(),
            online: false,
            disposition: "unavailable".to_string(),
            worker: None,
            diagnostics: Vec::new(),
        })
        .unwrap();
        assert_eq!(
            orchestrator,
            serde_json::json!({
                "workspace_id": "workspace-a",
                "online": false,
                "disposition": "unavailable",
                "diagnostics": [],
            })
        );

        let worker = serde_json::to_value(worker_launch_summary()).unwrap();
        assert!(
            !worker
                .as_object()
                .unwrap()
                .contains_key("working_directory")
        );
        assert_eq!(worker["profile"], serde_json::Value::Null);
        assert_eq!(worker["singleton_key"], serde_json::Value::Null);
        assert_eq!(worker["last_seen_at"], serde_json::Value::Null);

        let request = serde_json::to_value(CreateWorkspaceWorkerRequest {
            runtime_id: "runtime-a".to_string(),
            display_name: "Worker A".to_string(),
            profile: None,
            ticket_assignment: None,
            initial_submit: Vec::new(),
            workdir_attachments: Vec::new(),
            control_operation_id: None,
        })
        .unwrap();
        assert_eq!(
            request,
            serde_json::json!({
                "runtime_id": "runtime-a",
                "display_name": "Worker A",
                "profile": null,
                "ticket_assignment": null,
                "initial_submit": [],
                "workdir_attachments": [],
                "control_operation_id": null,
            })
        );
    }

    #[test]
    fn worker_launch_request_rejects_unknown_fields() {
        let error = serde_json::from_value::<CreateWorkspaceWorkerRequest>(serde_json::json!({
            "runtime_id": "runtime-a",
            "display_name": "Worker A",
            "unexpected": true,
        }))
        .unwrap_err();
        assert!(error.to_string().contains("unknown field"));
    }

    #[test]
    fn repository_key_validation_is_canonical_and_bounded() {
        let max = "a".repeat(64);
        for valid in ["a", "main", "repo-42", max.as_str()] {
            assert_eq!(validate_repository_key(valid), Ok(()), "{valid}");
        }
        let too_long = "a".repeat(65);
        for invalid in [
            "",
            "-main",
            "main-",
            "Main",
            "main_repo",
            "main.repo",
            "日本語",
            too_long.as_str(),
        ] {
            assert!(validate_repository_key(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn workspace_deletion_wire_contract_is_closed_and_typed() {
        let preflight = WorkspaceDeletionPreflightResponse {
            workspace_id: "workspace-test".to_string(),
            display_name: "Test".to_string(),
            expected_revision: "revision-7".to_string(),
            can_delete: true,
            resources: WorkspaceDeletionResourceCounts {
                workers: 2,
                workdirs: 1,
                repositories: 1,
                runtime_bindings: 1,
                secrets: 0,
                artifacts: 3,
            },
            blockers: Vec::new(),
        };
        let value = serde_json::to_value(&preflight).unwrap();
        assert_eq!(
            serde_json::from_value::<WorkspaceDeletionPreflightResponse>(value.clone()).unwrap(),
            preflight
        );
        let mut stale = value.as_object().unwrap().clone();
        stale.insert("revision".to_string(), serde_json::json!(7));
        assert!(
            serde_json::from_value::<WorkspaceDeletionPreflightResponse>(stale.into()).is_err()
        );

        assert!(
            serde_json::from_value::<WorkspaceDeletionRequest>(serde_json::json!({
                "operation_id": "delete-test",
                "expected_revision": "revision-7",
                "confirmation": "Test",
                "workspace_id": "caller-controlled"
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<WorkspaceDeletionRequest>(serde_json::json!({
                "operation_id": "x".repeat(WORKSPACE_DELETION_MAX_OPERATION_ID_BYTES + 1),
                "expected_revision": "revision-7",
                "confirmation": "Test"
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<WorkspaceDeletionOperationResponse>(serde_json::json!({
                "operation_id": "delete-test",
                "workspace_id": "workspace-test",
                "display_name": "Test",
                "state": "blocked",
                "resources": {
                    "workers": 0,
                    "workdirs": 0,
                    "repositories": 0,
                    "runtime_bindings": 0,
                    "secrets": 0,
                    "artifacts": 0
                },
                "child_operation_ids": [],
                "blockers": (0..=WORKSPACE_DELETION_MAX_BLOCKERS).map(|_| serde_json::json!({
                    "kind": "cleanup_unavailable",
                    "resource_kind": null,
                    "resource_key": null,
                    "message": "blocked"
                })).collect::<Vec<_>>(),
                "failure_category": null,
                "created_at": "1",
                "updated_at": "1",
                "completed_at": null
            }))
            .is_err()
        );
    }

    #[test]
    fn workspace_create_request_has_one_closed_shared_wire_shape() {
        let request = WorkspaceCreateRequest {
            operation_key: "workspace-create-1".to_string(),
            display_name: "Workspace".to_string(),
            repository: InitialRepositoryIntent {
                repository_key: "main".to_string(),
                uri: "/srv/repositories/main".to_string(),
                default_ref: Some("develop".to_string()),
            },
        };
        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["operation_key"], "workspace-create-1");
        assert_eq!(json["repository"]["uri"], "/srv/repositories/main");
        assert!(json.get("operation_id").is_none());
        assert!(json["repository"].get("source").is_none());
        assert!(
            serde_json::from_value::<WorkspaceCreateRequest>(serde_json::json!({
                "operation_id": "workspace-create-1",
                "display_name": "Workspace",
                "repository": {
                    "repository_key": "main",
                    "source": "/srv/repositories/main"
                }
            }))
            .is_err()
        );
    }

    #[test]
    fn workspace_and_repository_response_shapes_round_trip() {
        let workspace = serde_json::json!({
            "workspace_id": "workspace-test",
            "display_name": "Test",
            "record_authority": "workspace-control-plane",
            "schema_version": 46,
            "auth": {"Passkey": {
                "rp_id": "example.test",
                "origin": "https://example.test",
                "public_base_url": "https://example.test",
                "cookie_name": "yoi_session"
            }},
            "permissions": {
                "manage_repositories": true,
                "manage_secrets": true,
                "manage_runtimes": true,
                "delete_workspace": true
            },
            "extension_points": {
                "store": "sqlite",
                "event_stream": {"status": "available", "note": "ready", "diagnostics": []},
                "host_worker_bridge": {"status": "available", "note": "ready", "diagnostics": []},
                "companion_console": {"status": "available", "note": "ready", "diagnostics": []}
            }
        });
        let parsed: WorkspaceResponse = serde_json::from_value(workspace.clone()).unwrap();
        assert_eq!(serde_json::to_value(parsed).unwrap(), workspace);

        let catalog = serde_json::json!([{
            "workspace_id": "workspace-test",
            "owner_account_id": "user-test",
            "display_name": "Test",
            "state": "active",
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z"
        }]);
        let parsed: WorkspaceCatalogListResponse = serde_json::from_value(catalog.clone()).unwrap();
        assert_eq!(serde_json::to_value(parsed).unwrap(), catalog);

        let repositories = serde_json::json!({
            "workspace_id": "workspace-test",
            "items": [{
                "repository_key": "main",
                "kind": "git",
                "provider": "git",
                "source": {"kind": "local_path", "uri": "/srv/project"},
                "source_revision": 1,
                "source_fingerprint": "sha256:test",
                "observed_status": "ready",
                "record_authority": "workspace-control-plane"
            }],
            "source": "workspace-control-plane",
            "diagnostics": []
        });
        let parsed: RepositoryListResponse = serde_json::from_value(repositories.clone()).unwrap();
        assert_eq!(serde_json::to_value(parsed).unwrap(), repositories);
    }

    #[test]
    fn repository_response_rejects_stale_field_aliases() {
        let stale = serde_json::json!({
            "workspace_id": "workspace-test",
            "items": [{
                "repository_key": "main",
                "display_name": "main",
                "kind": "git",
                "provider": "git",
                "source": {"kind": "local_path", "uri": "/srv/project"},
                "source_revision": 1,
                "source_fingerprint": "sha256:test",
                "observed_status": "ready",
                "record_authority": "workspace-control-plane"
            }],
            "source": "workspace-control-plane",
            "diagnostics": []
        });

        assert!(serde_json::from_value::<RepositoryListResponse>(stale).is_err());
    }

    #[test]
    fn runtime_detail_and_trust_mutations_are_closed_and_typed() {
        let detail = serde_json::json!({
            "workspace_id": "workspace-test",
            "runtime": {
                "runtime_id": "runtime-test",
                "label": "Runtime Test",
                "kind": "remote_http",
                "status": "active",
                "source": {
                    "kind": "remote_http",
                    "status": "active",
                    "identity_authority": "runtime_registry_projection",
                    "note": "active"
                },
                "host_ids": [],
                "worker_creation_available": true,
                "os": "linux",
                "arch": "x86_64",
                "diagnostics": [],
                "management": {
                    "built_in": false,
                    "config_managed": true,
                    "removable": true,
                    "endpoint_configured": true,
                    "token_ref_configured": false
                }
            },
            "endpoint": "https://runtime.example",
            "trust_key": {
                "status": "active",
                "fingerprint": "SHA256:test",
                "revision": 2,
                "created_at": "2026-09-01T12:00:00Z",
                "updated_at": "2026-09-01T13:00:00Z"
            },
            "recent_audit": [{
                "action": "replaced",
                "actor_account_id": "account-owner",
                "old_fingerprint": "SHA256:old",
                "new_fingerprint": "SHA256:test",
                "revision": 2,
                "at": "2026-09-01T13:00:00Z"
            }]
        });
        let parsed: WorkspaceRuntimeDetail = serde_json::from_value(detail.clone()).unwrap();
        assert_eq!(serde_json::to_value(parsed).unwrap(), detail);

        let mut unknown = detail;
        unknown["trust_key"]["private_key"] = serde_json::json!("forbidden");
        assert!(serde_json::from_value::<WorkspaceRuntimeDetail>(unknown).is_err());
        assert!(
            serde_json::from_value::<RuntimeTrustKeyRevealResponse>(serde_json::json!({
                "public_key": "yoi-ed25519-pub:v1:key",
                "private_key": "forbidden"
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<RevokeRuntimeTrustKeyRequest>(serde_json::json!({
                "expected_revision": 1,
                "delete_runtime": true
            }))
            .is_err()
        );
    }

    #[test]
    fn runtime_connection_test_response_is_closed_and_typed() {
        let compatible = serde_json::json!({
            "workspace_id": "workspace-test",
            "runtime_id": "runtime-test",
            "binding_revision": 3,
            "connection_state": "verified",
            "verification": null,
            "checked_at": "2026-09-01T12:00:00Z",
            "status": "compatible",
            "failure_kind": null,
            "expected_protocol_version": 1,
            "actual_protocol_version": 1,
            "diagnostics": []
        });
        let parsed: RuntimeConnectionTestResponse =
            serde_json::from_value(compatible.clone()).unwrap();
        assert_eq!(serde_json::to_value(parsed).unwrap(), compatible);

        let mut unknown = compatible;
        unknown["capabilities"] = serde_json::json!(["shell"]);
        assert!(serde_json::from_value::<RuntimeConnectionTestResponse>(unknown).is_err());
    }

    #[cfg(feature = "typescript")]
    #[test]
    fn generated_catalog_typescript_keeps_public_wrappers_and_nullability() {
        let output = legacy_catalog_typescript();
        assert!(
            output.contains("export type WorkspaceCatalogListResponse = Array<WorkspaceSummary>")
        );
        assert!(output.contains("export type WorkspaceResponse ="));
        assert!(output.contains("permissions: WorkspacePermissionSummary"));
        assert!(output.contains(
            "import type { RepositoryObservedStatus, RepositorySource } from \"./repository-api\";"
        ));
        assert!(!output.contains("export type RepositoryListResponse ="));
        assert!(!output.contains("export type RepositoryDetailResponse ="));
        assert!(!output.contains("export type RepositorySummary ="));
        assert!(!output.contains("export type RepositorySource ="));
        assert!(!output.contains("export type RepositoryObservedStatus ="));
        assert!(output.contains("export type WorkspaceMetadataSettingsResponse ="));
        assert!(output.contains("export type WorkspaceMetadataMutationResponse ="));
        assert!(output.contains("export type WorkspaceConfigTreeResponse ="));
        assert!(output.contains("export type ConfigCommitRequest ="));
        assert!(output.contains("export type ProfileSettingsResponse ="));
        assert!(output.contains("config_revision?: number | null"));
        assert!(output.contains("provenance: WorkspaceProfileSourceProvenance"));
        assert!(output.contains(
            "export type WorkspaceProfileSourceProvenance = \"project_profile_source_tree\""
        ));
        assert!(output.contains("export type RuntimeConnectionTestResponse ="));
        assert!(output.contains("status: RuntimeConnectionTestStatus"));
        assert!(output.contains("failure_kind: RuntimeConnectionTestFailureKind | null"));
        assert!(!output.contains("repository_key: string, display_name"));
    }

    #[cfg(feature = "typescript")]
    #[test]
    fn generated_legacy_server_api_contract_is_current() {
        fn normalize(value: &str) -> String {
            value
                .chars()
                .filter_map(|character| match character {
                    character if character.is_whitespace() => None,
                    ',' => Some(';'),
                    character => Some(character),
                })
                .collect::<String>()
                .replace("=|", "=")
                .replace(";}", "}")
        }

        let expected = legacy_catalog_typescript();
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../web/workspace/src/lib/generated/legacy-server-api.ts");
        let actual = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        assert_eq!(
            normalize(&actual),
            normalize(&expected),
            "regenerate legacy Server API TypeScript types and format the generated file",
        );
    }

    #[test]
    fn worker_resource_key_is_required() {
        let payload = serde_json::json!({
            "runtime_id": "arcadia",
            "worker_id": "worker-1",
            "host_id": "host",
            "display_name": "Coder",
            "label": "Coder",
            "workspace": {
                "visibility": "workspace",
                "identity": "workspace-test"
            },
            "state": "idle",
            "implementation": {"kind": "worker", "display_hint": "Coder"},
            "capabilities": {"can_stop": true, "can_spawn_followup": false}
        });

        assert!(serde_json::from_value::<WorkerSummary>(payload).is_err());
    }

    fn round_trip<T>(value: T)
    where
        T: std::fmt::Debug + PartialEq + Serialize + for<'de> Deserialize<'de>,
    {
        let encoded = serde_json::to_vec(&value).expect("fixture should serialize");
        let decoded: T = serde_json::from_slice(&encoded).expect("fixture should deserialize");
        assert_eq!(decoded, value);
    }

    #[test]
    fn workspace_metadata_and_profile_projection_fixtures_round_trip() {
        let diagnostic = Diagnostic {
            code: "profile_projection_warning".to_string(),
            severity: DiagnosticSeverity::Warning,
            message: "projected from the active config revision".to_string(),
        };
        let metadata = WorkspaceMetadataSettingsResponse {
            workspace_id: "workspace-test".to_string(),
            display_name: "Test".to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            revision: "sha256:metadata".to_string(),
            source: "workspace-config".to_string(),
            diagnostics: vec![diagnostic.clone()],
        };
        round_trip(metadata.clone());
        round_trip(UpdateWorkspaceMetadataRequest {
            display_name: "Renamed".to_string(),
            revision: metadata.revision.clone(),
        });
        round_trip(WorkspaceMetadataMutationResponse {
            workspace: metadata,
            diagnostics: vec![],
        });

        round_trip(ProfileSettingsResponse {
            workspace_id: "workspace-test".to_string(),
            registry_revision: "config-source:7:sha256:tree:sha256:projection".to_string(),
            config_revision: Some(7),
            tree_digest: Some("sha256:tree".to_string()),
            projection_digest: Some("sha256:projection".to_string()),
            default_profile: Some("workspace:coder".to_string()),
            profiles: vec![WorkspaceProfileSummary {
                profile_id: "workspace:coder".to_string(),
                selector: "workspace:coder".to_string(),
                label: "Coder".to_string(),
                source_kind: "project".to_string(),
                profile_source_id: Some("profile-source-1".to_string()),
                description: None,
                editable: true,
                is_default: true,
                diagnostics: vec![diagnostic.clone()],
            }],
            sources: vec![WorkspaceProfileSourceSummary {
                profile_source_id: "profile-source-1".to_string(),
                display_path: "profiles/coder.dcdl".to_string(),
                kind: "profile".to_string(),
                content_type: "text/x-decodal".to_string(),
                content_digest: "sha256:source".to_string(),
                provenance: WorkspaceProfileSourceProvenance::ProjectProfileSourceTree,
                editable: false,
                revision: "config-source:7".to_string(),
                size_bytes: 128,
                diagnostics: vec![],
            }],
            diagnostics: vec![diagnostic],
        });

        let absent_optional_fields = serde_json::json!({
            "workspace_id": "workspace-test",
            "registry_revision": "builtin",
            "profiles": [],
            "sources": [],
            "diagnostics": []
        });
        let decoded: ProfileSettingsResponse =
            serde_json::from_value(absent_optional_fields.clone()).unwrap();
        assert_eq!(decoded.config_revision, None);
        assert_eq!(decoded.tree_digest, None);
        assert_eq!(decoded.projection_digest, None);
        assert_eq!(decoded.default_profile, None);
        assert_eq!(
            serde_json::to_value(decoded).unwrap(),
            absent_optional_fields
        );
    }

    #[test]
    fn workspace_signing_identity_wire_contract_omits_private_and_pending_fields() {
        let response = WorkspaceSigningIdentityResponse {
            identity: WorkspaceSigningIdentityPublic {
                workspace_id: "workspace-test".to_string(),
                key_id: "WK-test".to_string(),
                algorithm: "ed25519".to_string(),
                public_key: None,
                public_key_fingerprint: None,
                revision: 1,
                state: WorkspaceSigningIdentityState::PendingProvisioning,
                created_at: "2026-01-01T00:00:00Z".to_string(),
                provisioned_at: None,
            },
            public_bundle: None,
        };
        let encoded = serde_json::to_value(&response).unwrap();
        assert_eq!(
            encoded,
            serde_json::json!({
                "identity": {
                    "workspace_id": "workspace-test",
                    "key_id": "WK-test",
                    "algorithm": "ed25519",
                    "revision": 1,
                    "state": "pending_provisioning",
                    "created_at": "2026-01-01T00:00:00Z"
                }
            })
        );
        assert!(
            serde_json::from_value::<WorkspaceSigningIdentityResponse>(serde_json::json!({
                "identity": encoded["identity"].clone(),
                "private_material_ref": "must-not-cross-the-wire"
            }))
            .is_err()
        );
    }

    fn companion_worker() -> WorkspaceWorkerDiscoveryItem {
        WorkspaceWorkerDiscoveryItem {
            subject: WorkspaceWorkerSubject::RuntimeWorker {
                runtime_id: "arcadia".to_string(),
                worker_id: "worker-7".to_string(),
            },
            resource_key: "W-7".to_string(),
            display_name: "Companion".to_string(),
            profile: Some("builtin:companion".to_string()),
            status: Some("idle".to_string()),
        }
    }

    #[test]
    fn companion_status_fixtures_round_trip() {
        for state in [
            CompanionLifecycleState::Idle,
            CompanionLifecycleState::Running,
            CompanionLifecycleState::Stopped,
        ] {
            round_trip(CompanionStatusResponse {
                state,
                worker: Some(companion_worker()),
                transport: CompanionTransportSummary {
                    mode: "worker_runtime".to_string(),
                    available: state != CompanionLifecycleState::Stopped,
                },
                diagnostics: Vec::new(),
            });
        }
    }

    #[test]
    fn companion_message_fixtures_round_trip() {
        for state in [
            CompanionMessageDisposition::Accepted,
            CompanionMessageDisposition::Rejected,
        ] {
            round_trip(CompanionMessageResponse {
                state,
                message: if state == CompanionMessageDisposition::Accepted {
                    "accepted"
                } else {
                    "rejected"
                }
                .to_string(),
            });
        }
    }

    #[test]
    fn auth_server_fixtures_round_trip_through_shared_dtos() {
        let whoami = serde_json::json!({
            "actor": {
                "user_id": "user-1",
                "account_id": "account-1",
                "handle": "hare",
                "display_name": "Hare",
                "auth_method": "browser_session"
            }
        });
        let decoded = serde_json::from_value::<WhoamiResponse>(whoami.clone())
            .expect("server whoami fixture should match shared DTO");
        assert_eq!(serde_json::to_value(decoded).unwrap(), whoami);

        let auth_config = serde_json::json!({
            "rp_id": "yoi.example",
            "origin": "https://yoi.example",
            "public_base_url": "https://yoi.example",
            "cookie_name": "yoi_workspace_session"
        });
        let decoded = serde_json::from_value::<AuthPublicConfig>(auth_config.clone())
            .expect("server auth-config fixture should match shared DTO");
        assert_eq!(serde_json::to_value(decoded).unwrap(), auth_config);

        let auth_user = serde_json::json!({
            "user": {
                "user_id": "user-1",
                "account_id": "account-1",
                "handle": "hare",
                "display_name": "Hare"
            }
        });
        let decoded = serde_json::from_value::<AuthUserResponse>(auth_user.clone())
            .expect("server auth-user fixture should match shared DTO");
        assert_eq!(serde_json::to_value(decoded).unwrap(), auth_user);

        let registration_options = serde_json::json!({
            "challenge_id": "challenge-1",
            "public_key": {
                "publicKey": {
                    "challenge": "AQID",
                    "rp": {"id": "localhost", "name": "Yoi"},
                    "user": {"id": "BAUG", "name": "hare", "displayName": "Hare"},
                    "pubKeyCredParams": [{"type": "public-key", "alg": -7}]
                }
            }
        });
        let decoded = serde_json::from_value::<PasskeyRegistrationOptionsResponse>(
            registration_options.clone(),
        )
        .expect("server registration options fixture should match shared DTO");
        assert_eq!(serde_json::to_value(decoded).unwrap(), registration_options);

        let registration_complete = serde_json::json!({
            "challenge_id": "challenge-1",
            "credential": {
                "id": "AQID",
                "rawId": "AQID",
                "response": {
                    "attestationObject": "AQID",
                    "clientDataJSON": "AQID",
                    "transports": ["internal"]
                },
                "type": "public-key",
                "clientExtensionResults": {},
                "authenticatorAttachment": "platform"
            }
        });
        let decoded =
            serde_json::from_value::<PasskeyRegistrationCompleteRequest>(registration_complete)
                .expect("server registration-complete fixture should match shared DTO");
        let encoded = serde_json::to_value(decoded).unwrap();
        serde_json::from_value::<PasskeyRegistrationCompleteRequest>(encoded)
            .expect("registration-complete DTO should round-trip");

        let login_options = serde_json::json!({
            "challenge_id": "challenge-2",
            "public_key": {
                "publicKey": {
                    "challenge": "AQID",
                    "rpId": "localhost",
                    "allowCredentials": [],
                    "userVerification": "preferred"
                }
            }
        });
        let decoded = serde_json::from_value::<PasskeyLoginOptionsResponse>(login_options.clone())
            .expect("server login-options fixture should match shared DTO");
        assert_eq!(serde_json::to_value(decoded).unwrap(), login_options);

        let login_complete = serde_json::json!({
            "challenge_id": "challenge-2",
            "credential": {
                "id": "AQID",
                "rawId": "AQID",
                "response": {
                    "authenticatorData": "AQID",
                    "clientDataJSON": "AQID",
                    "signature": "AQID",
                    "userHandle": null
                },
                "type": "public-key",
                "clientExtensionResults": {},
                "authenticatorAttachment": "platform"
            }
        });
        let decoded = serde_json::from_value::<PasskeyLoginCompleteRequest>(login_complete)
            .expect("server login-complete fixture should match shared DTO");
        let encoded = serde_json::to_value(decoded).unwrap();
        serde_json::from_value::<PasskeyLoginCompleteRequest>(encoded)
            .expect("login-complete DTO should round-trip");

        let device_start = serde_json::json!({
            "device_code": "device-secret",
            "user_code": "ABCD-EFGH",
            "verification_uri": "https://yoi.example/login/device",
            "verification_uri_complete": "https://yoi.example/login/device?user_code=ABCD-EFGH",
            "expires_in": 600,
            "interval": 2
        });
        let decoded = serde_json::from_value::<DeviceLoginStartResponse>(device_start.clone())
            .expect("server device-login fixture should match shared DTO");
        assert_eq!(serde_json::to_value(decoded).unwrap(), device_start);

        let approved_user = AuthenticatedUser {
            user_id: "user-1".to_string(),
            account_id: "account-1".to_string(),
            handle: "hare".to_string(),
            display_name: "Hare".to_string(),
        };
        round_trip(DeviceLoginApproveResponse {
            status: DeviceLoginApprovalStatus::Approved,
            user: approved_user,
        });
        for status in [
            DeviceLoginPollStatus::Pending,
            DeviceLoginPollStatus::Expired,
            DeviceLoginPollStatus::Denied,
            DeviceLoginPollStatus::Consumed,
        ] {
            round_trip(DeviceLoginPollResponse {
                status,
                access_token: None,
                token_type: None,
            });
        }
        round_trip(DeviceLoginPollResponse {
            status: DeviceLoginPollStatus::Approved,
            access_token: Some("access-secret".to_string()),
            token_type: Some(DeviceAccessTokenType::Bearer),
        });
        round_trip(LogoutResponse {
            status: LogoutStatus::LoggedOut,
        });
    }

    #[test]
    fn auth_dtos_reject_unknown_status_and_fields() {
        let unknown_status = serde_json::json!({"status": "future_status"});
        assert!(
            serde_json::from_value::<DeviceLoginPollResponse>(unknown_status).is_err(),
            "unknown device-login statuses must fail closed"
        );

        let unsafe_expiry = serde_json::json!({
            "device_code": "device-secret",
            "user_code": "ABCD-EFGH",
            "verification_uri": "https://yoi.example/login/device",
            "verification_uri_complete": "https://yoi.example/login/device?user_code=ABCD-EFGH",
            "expires_in": 0,
            "interval": 2
        });
        assert!(serde_json::from_value::<DeviceLoginStartResponse>(unsafe_expiry).is_err());

        let malformed_credential = serde_json::json!({
            "challenge_id": "challenge-1",
            "credential": {
                "id": "AQID",
                "rawId": "AQID",
                "type": "public-key",
                "response": {"clientDataJSON": 42}
            }
        });
        assert!(
            serde_json::from_value::<PasskeyLoginCompleteRequest>(malformed_credential).is_err(),
            "malformed passkey credential payloads must fail closed"
        );

        let unexpected_field = serde_json::json!({
            "actor": null,
            "access_token": "must-not-be-accepted"
        });
        assert!(serde_json::from_value::<WhoamiResponse>(unexpected_field).is_err());
    }

    #[test]
    fn device_login_debug_output_redacts_secret_material() {
        let start = DeviceLoginStartResponse {
            device_code: "device-secret".to_string(),
            user_code: "ABCD-EFGH".to_string(),
            verification_uri: "https://yoi.example/login/device".to_string(),
            verification_uri_complete: "https://yoi.example/login/device?user_code=ABCD-EFGH"
                .to_string(),
            expires_in: 600,
            interval: 2,
        };
        let start_debug = format!("{start:?}");
        assert!(!start_debug.contains("device-secret"));
        assert!(start_debug.contains("[redacted]"));

        let poll = DeviceLoginPollResponse {
            status: DeviceLoginPollStatus::Approved,
            access_token: Some("access-secret".to_string()),
            token_type: Some(DeviceAccessTokenType::Bearer),
        };
        let poll_debug = format!("{poll:?}");
        assert!(!poll_debug.contains("access-secret"));
        assert!(poll_debug.contains("[redacted]"));
    }

    #[cfg(feature = "typescript")]
    #[test]
    fn generated_auth_api_contract_is_current() {
        let expected = auth_api_typescript();
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../web/workspace/src/lib/generated/auth-api.ts");
        let actual = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        assert_eq!(
            normalize_typescript(&actual),
            normalize_typescript(&expected),
            "regenerate auth API TypeScript types with `cargo run -q -p server-api --features typescript --example generate_auth_api_types > web/workspace/src/lib/generated/auth-api.ts` and format the generated file",
        );
    }

    #[test]
    fn companion_transcript_fixture_round_trips() {
        round_trip(CompanionTranscriptProjection {
            state: CompanionLifecycleState::Idle,
            start: 0,
            limit: 2,
            total: 2,
            next: None,
            items: vec![
                CompanionTranscriptItem {
                    sequence: 1,
                    role: CompanionTranscriptRole::User,
                    content: "hello".to_string(),
                    created_at: "2026-08-31T00:00:00Z".to_string(),
                },
                CompanionTranscriptItem {
                    sequence: 2,
                    role: CompanionTranscriptRole::Assistant,
                    content: "hi".to_string(),
                    created_at: "2026-08-31T00:00:01Z".to_string(),
                },
            ],
        });
    }

    #[test]
    fn companion_transcript_rejects_system_and_private_fields() {
        let public_item = CompanionTranscriptItem {
            sequence: 1,
            role: CompanionTranscriptRole::Assistant,
            content: "visible".to_string(),
            created_at: "2026-08-31T00:00:00Z".to_string(),
        };
        let public_fields = serde_json::to_value(public_item)
            .expect("public transcript item should serialize")
            .as_object()
            .expect("public transcript item should be an object")
            .keys()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            public_fields,
            ["content", "created_at", "role", "sequence"]
                .into_iter()
                .map(str::to_string)
                .collect()
        );

        let system_item = serde_json::json!({
            "sequence": 1,
            "role": "system",
            "content": "raw system prompt",
            "created_at": "2026-08-31T00:00:00Z"
        });
        assert!(serde_json::from_value::<CompanionTranscriptItem>(system_item).is_err());

        let private_item = serde_json::json!({
            "sequence": 1,
            "role": "assistant",
            "content": "visible",
            "created_at": "2026-08-31T00:00:00Z",
            "reasoning": "hidden",
            "credential": "secret",
            "provider_session_id": "session-private"
        });
        assert!(serde_json::from_value::<CompanionTranscriptItem>(private_item).is_err());
    }

    #[cfg(feature = "typescript")]
    #[test]
    fn generated_companion_api_contract_is_current() {
        let expected = companion_api_typescript();
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../web/workspace/src/lib/generated/companion-api.ts");
        let actual = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        assert_eq!(
            normalize_typescript(&actual),
            normalize_typescript(&expected),
            "regenerate Companion API TypeScript types with `cargo run -q -p server-api --features typescript --example generate_companion_api_types > web/workspace/src/lib/generated/companion-api.ts` and format the generated file",
        );
    }

    #[cfg(feature = "typescript")]
    fn normalize_typescript(value: &str) -> String {
        value
            .chars()
            .filter_map(|character| match character {
                character if character.is_whitespace() => None,
                ',' => Some(';'),
                character => Some(character),
            })
            .collect::<String>()
            .replace(";}", "}")
            .replace("=|", "=")
    }

    #[test]
    fn workdir_create_request_preserves_optional_operation_fields() {
        let payload = serde_json::json!({"repository_key": "main"});
        let request = serde_json::from_value::<WorkingDirectoryCreateRequest>(payload)
            .expect("optional create fields may be absent");

        assert_eq!(request.runtime_id, None);
        assert_eq!(request.selector, None);
        assert_eq!(request.operation_id, None);

        let serialized = serde_json::to_value(request).expect("serialize create request");
        assert_eq!(serialized, serde_json::json!({"repository_key": "main"}));
    }

    #[test]
    fn workdir_create_request_rejects_stale_or_incomplete_json() {
        let stale = serde_json::json!({
            "repository_key": "main",
            "selector": "develop",
            "path": "/tmp/workdir"
        });
        assert!(serde_json::from_value::<WorkingDirectoryCreateRequest>(stale).is_err());

        let incomplete = serde_json::json!({
            "runtime_id": "arcadia",
            "operation_id": "operation-1"
        });
        assert!(serde_json::from_value::<WorkingDirectoryCreateRequest>(incomplete).is_err());
    }

    #[test]
    fn workdir_summary_omits_absent_optional_fields_on_the_wire() {
        let value = serde_json::to_value(WorkingDirectorySummary {
            working_directory_id: "workdir-1".into(),
            display_name: None,
            source: WorkingDirectorySource::Repository {
                repository_key: "main".into(),
            },
            creation_selector: None,
            creation_ref: None,
            creation_tree: None,
            current_selector: None,
            current_ref: None,
            current_tree: None,
            observed_at_epoch_seconds: None,
            materializer_kind: WorkingDirectoryMaterializerKind::RuntimeGitClone,
            cleanup_target: None,
            status: WorkingDirectoryStatusKind::Active,
            cleanliness: None,
            occupied_by: None,
        })
        .expect("serialize Workdir summary");
        let object = value.as_object().expect("Workdir summary object");

        for key in [
            "creation_selector",
            "creation_ref",
            "creation_tree",
            "current_selector",
            "current_ref",
            "current_tree",
            "observed_at_epoch_seconds",
            "cleanup_target",
            "cleanliness",
            "occupied_by",
        ] {
            assert!(
                !object.contains_key(key),
                "absent field {key} must be omitted"
            );
        }
    }

    #[test]
    fn workdir_response_rejects_stale_occupancy_shape() {
        let stale = serde_json::json!({
            "workspace_id": "workspace-test",
            "items": [{
                "working_directory_id": "workdir-1",
                "repository_key": "main",
                "materializer_kind": "runtime_git_clone",
                "status": "active",
                "occupied_by": {
                    "runtime_worker_id": "worker-1",
                    "display_name": "Coder",
                    "linked_at": "2026-01-01T00:00:00Z"
                }
            }],
            "diagnostics": []
        });

        assert!(serde_json::from_value::<WorkingDirectoryListResponse>(stale).is_err());
    }
}

#[cfg(test)]
mod openapi_artifact_tests {
    use super::*;

    #[test]
    fn canonical_openapi_artifact_is_current() {
        let generated = canonical_openapi_document()
            .expect("canonical OpenAPI contract must be valid")
            .to_json()
            .expect("canonical OpenAPI contract must serialize");
        let checked_in = include_str!("../../../openapi/server-api.json");
        assert_eq!(
            generated, checked_in,
            "regenerate with `cargo run -p server-api --example export_openapi -- openapi/server-api.json`",
        );
    }

    #[test]
    fn auth_and_workspace_catalog_contract_is_strict_and_excludes_cookie_routes() {
        let document = canonical_openapi_document().expect("canonical OpenAPI contract must build");
        let value: serde_json::Value =
            serde_json::from_str(&document.to_json().expect("document must serialize"))
                .expect("document must be JSON");

        for (path, method) in [
            ("/health", "get"),
            ("/api/auth/config", "get"),
            ("/api/auth/bootstrap-user", "post"),
            ("/api/auth/passkeys/registration/options", "post"),
            ("/api/auth/passkeys/login/options", "post"),
            ("/api/auth/device-login/start", "post"),
            ("/api/auth/device-login/approve", "post"),
            ("/api/auth/device-login/poll", "post"),
            ("/api/auth/whoami", "get"),
            ("/api/workspaces", "get"),
            ("/api/workspaces", "post"),
            ("/api/workspaces/{workspace_id}/deletion", "get"),
            ("/api/workspaces/{workspace_id}/deletion", "post"),
            ("/api/workspace-deletions/{operation_id}", "get"),
        ] {
            assert!(
                value["paths"][path][method].is_object(),
                "missing {method} {path}"
            );
        }
        let workspace_catalog = &value["paths"]["/api/workspaces"];
        for method in ["get", "post"] {
            assert_eq!(
                workspace_catalog[method]["security"][0]["bearerAuth"],
                serde_json::json!([])
            );
            assert_eq!(
                workspace_catalog[method]["security"][1]["browserSession"],
                serde_json::json!([])
            );
        }
        for status in ["200", "201"] {
            assert!(workspace_catalog["post"]["responses"][status].is_object());
        }
        let deletion_start = &value["paths"]["/api/workspaces/{workspace_id}/deletion"]["post"];
        for status in ["200", "202", "409"] {
            assert!(deletion_start["responses"][status].is_object());
        }
        for path in [
            "/api/auth/passkeys/registration/complete",
            "/api/auth/passkeys/login/complete",
            "/api/auth/logout",
        ] {
            assert!(
                value["paths"].get(path).is_none(),
                "cookie response route {path} must remain outside ServerApi"
            );
        }
        for schema in [
            "RepositoryApiError",
            "WorkspaceCreateRequest",
            "WorkspaceDeletionRequest",
            "DeviceLoginStartResponse",
        ] {
            assert_eq!(
                value["components"]["schemas"][schema]["additionalProperties"], false,
                "{schema} must reject unknown fields"
            );
        }
    }

    #[test]
    fn repository_operations_are_in_the_canonical_contract() {
        let document = canonical_openapi_document().expect("canonical OpenAPI contract must build");
        let value: serde_json::Value =
            serde_json::from_str(&document.to_json().expect("document must serialize"))
                .expect("document must be JSON");

        assert_eq!(value["openapi"], "3.1.0");
        assert!(
            value["paths"]
                ["/api/w/{workspace_id}/runtimes/{runtime_id}/workers/{worker_id}/session"]
                .is_null()
        );
        assert!(
            value["info"]["x-yoi-source-digest"]
                .as_str()
                .is_some_and(|digest| digest.starts_with("sha256:"))
        );
        assert_eq!(
            value["paths"]["/api/w/{workspace_id}/repositories"]["get"]["responses"]["200"]["content"]
                ["application/json"]["schema"]["$ref"],
            "#/components/schemas/RepositoryListResponse"
        );
        assert!(value["paths"]["/api/repositories"]["get"].is_object());
        assert!(value["paths"]["/api/repositories/{repository_key}"]["get"].is_object());
        for (path, method) in [
            ("/api/w/{workspace_id}/repositories", "get"),
            ("/api/w/{workspace_id}/repositories", "post"),
            ("/api/w/{workspace_id}/repositories/{repository_key}", "get"),
            ("/api/repositories", "get"),
            ("/api/repositories/{repository_key}", "get"),
        ] {
            let operation = &value["paths"][path][method];
            assert_eq!(
                operation["security"][0]["bearerAuth"],
                serde_json::json!([])
            );
            assert_eq!(
                operation["security"][1]["browserSession"],
                serde_json::json!([])
            );
            for status in ["401", "403", "500"] {
                assert!(
                    operation["responses"][status].is_object(),
                    "missing {method} {path} response {status}"
                );
            }
        }
        let repository_collection = &value["paths"]["/api/w/{workspace_id}/repositories"];
        let repository_create = &repository_collection["post"];
        assert_eq!(
            repository_create["security"][0]["bearerAuth"],
            serde_json::json!([])
        );
        assert_eq!(
            repository_create["security"][1]["browserSession"],
            serde_json::json!([])
        );
        for status in [
            "200", "201", "400", "401", "403", "404", "409", "413", "415", "422", "500",
        ] {
            assert!(
                repository_create["responses"][status].is_object(),
                "missing repository-create response {status}"
            );
        }
        assert_eq!(
            repository_create["responses"]["422"]["content"]["application/json"]["schema"]["$ref"],
            "#/components/schemas/RepositoryApiError"
        );
        assert_eq!(
            value["components"]["securitySchemes"]["bearerAuth"]["scheme"],
            "bearer"
        );
        assert_eq!(
            value["components"]["securitySchemes"]["browserSession"]["in"],
            "cookie"
        );
        assert!(value["paths"]["/api/w/{workspace_id}/repositories/{repository_key}"]
            ["get"]["responses"]["404"]
            .is_object());
    }
}

#[cfg(all(test, feature = "typescript"))]
mod typescript_tests {
    #[test]
    fn generated_repository_access_contract_is_current() {
        let expected = super::repository_access_api_typescript();
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../web/workspace/src/lib/generated/repository-access-api.ts");
        let actual = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        assert_eq!(
            normalize(&actual),
            normalize(&expected),
            "regenerate Repository Access TypeScript types with `cargo run -q -p server-api --features typescript --example generate_repository_access_types > web/workspace/src/lib/generated/repository-access-api.ts` and format the generated file",
        );
    }

    #[test]
    fn generated_repository_access_responses_remain_secret_free() {
        use ts_rs::TS;

        let config = ts_rs::Config::default();
        for declaration in [
            super::RepositorySshCredential::decl(&config),
            super::RepositorySshHostTrust::decl(&config),
            super::RepositoryAccessProjection::decl(&config),
        ] {
            for forbidden in ["private_key", "passphrase", "secret_ref"] {
                assert!(
                    !declaration.contains(forbidden),
                    "Repository Access response declaration must not expose `{forbidden}`"
                );
            }
        }
    }

    fn normalize(value: &str) -> String {
        value
            .chars()
            .filter_map(|character| match character {
                character if character.is_whitespace() => None,
                ',' => Some(';'),
                character => Some(character),
            })
            .collect()
    }
}
