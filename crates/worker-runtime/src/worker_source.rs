use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use worker::{
    WorkspaceClient, WorkspaceClientError, WorkspacePromptCatalogResolution,
    WorkspacePromptProjection, WorkspaceRequest, WorkspaceRequestMethod, WorkspaceResponse,
};

use crate::auth::{
    RUNTIME_REQUEST_SOURCE_PROOF_HEADER, RuntimeAuthError, RuntimeIdentityMaterial,
    RuntimeRequestSourceSigner, RuntimeWorkerMutationSourceSigner, WORKER_REMOVE_PERMISSION,
    WORKSPACE_REQUEST_PERMISSION, WORKSPACE_WORKER_DISCOVERY_PERMISSION, WorkerMutationActorKind,
    WorkerMutationOperation, WorkerMutationSourceClaims, new_token_id,
};
use crate::runtime::RuntimeWorkspaceScope;
use crate::worker_backend::WorkspacePromptProjectionCache;

pub const DEFAULT_WORKER_MUTATION_SOURCE_TTL_SECONDS: u64 = 60;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuntimeOwnedWorkerMutationProof {
    Remote(String),
    InProcess(InProcessWorkerMutationProof),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InProcessWorkerMutationProof {
    claims: WorkerMutationSourceClaims,
}

impl InProcessWorkerMutationProof {
    pub fn claims(&self) -> &WorkerMutationSourceClaims {
        &self.claims
    }

    pub fn into_claims(self) -> WorkerMutationSourceClaims {
        self.claims
    }
}

#[derive(Clone)]
pub struct RuntimeWorkerMutationSourceAuthority {
    mode: RuntimeWorkerMutationSourceMode,
}

#[derive(Clone)]
enum RuntimeWorkerMutationSourceMode {
    Remote {
        signer: RuntimeWorkerMutationSourceSigner,
    },
    Embedded {
        runtime_id: String,
        audience: String,
    },
}

impl RuntimeWorkerMutationSourceAuthority {
    pub fn remote(identity: &RuntimeIdentityMaterial) -> Self {
        Self {
            mode: RuntimeWorkerMutationSourceMode::Remote {
                signer: RuntimeWorkerMutationSourceSigner::from_identity(identity),
            },
        }
    }

    pub fn embedded(runtime_id: impl Into<String>, workspace_id: impl AsRef<str>) -> Self {
        Self {
            mode: RuntimeWorkerMutationSourceMode::Embedded {
                runtime_id: runtime_id.into(),
                audience: format!("embedded:{}", workspace_id.as_ref()),
            },
        }
    }

    pub fn issue_worker_remove(
        &self,
        scope: &RuntimeWorkspaceScope,
        source_worker_id: &str,
        target_runtime_id: &str,
        target_worker_id: &str,
    ) -> Result<RuntimeOwnedWorkerMutationProof, RuntimeAuthError> {
        match &self.mode {
            RuntimeWorkerMutationSourceMode::Remote { signer } => {
                let token = signer.issue_worker_remove(
                    &scope.server_id,
                    &scope.workspace_id,
                    source_worker_id,
                    target_runtime_id,
                    target_worker_id,
                    DEFAULT_WORKER_MUTATION_SOURCE_TTL_SECONDS,
                )?;
                Ok(RuntimeOwnedWorkerMutationProof::Remote(token))
            }
            RuntimeWorkerMutationSourceMode::Embedded {
                runtime_id,
                audience,
            } => {
                let issued_at = unix_now_seconds();
                Ok(RuntimeOwnedWorkerMutationProof::InProcess(
                    InProcessWorkerMutationProof {
                        claims: WorkerMutationSourceClaims {
                            iss: runtime_id.clone(),
                            aud: audience.clone(),
                            workspace_id: scope.workspace_id.clone(),
                            worker_id: source_worker_id.to_string(),
                            actor_kind: WorkerMutationActorKind::Worker,
                            operation: WorkerMutationOperation::WorkerRemove,
                            target_runtime_id: target_runtime_id.to_string(),
                            target_worker_id: target_worker_id.to_string(),
                            permission: WORKER_REMOVE_PERMISSION.to_string(),
                            iat: issued_at,
                            exp: issued_at
                                .saturating_add(DEFAULT_WORKER_MUTATION_SOURCE_TTL_SECONDS),
                            jti: new_token_id()?,
                        },
                    },
                ))
            }
        }
    }
}

pub trait EmbeddedWorkerMutationDispatcher: Send + Sync {
    fn execute_worker_remove(
        &self,
        proof: InProcessWorkerMutationProof,
        target_runtime_id: &str,
        target_worker_id: &str,
        reason: &str,
    ) -> Result<WorkspaceResponse, RuntimeWorkerMutationForwardError>;
}

#[derive(Clone)]
enum RuntimeWorkerMutationTransport {
    Remote {
        base_url: String,
        request_source_signer: RuntimeRequestSourceSigner,
        request_source_audience: String,
    },
    Embedded {
        dispatcher: Arc<dyn EmbeddedWorkerMutationDispatcher>,
    },
}

#[derive(Clone)]
pub struct RuntimeWorkerMutationForwarder {
    authority: RuntimeWorkerMutationSourceAuthority,
    scope: RuntimeWorkspaceScope,
    source_worker_id: String,
    transport: RuntimeWorkerMutationTransport,
}

impl RuntimeWorkerMutationForwarder {
    pub fn remote(
        identity: &RuntimeIdentityMaterial,
        scope: RuntimeWorkspaceScope,
        source_worker_id: impl Into<String>,
        base_url: impl Into<String>,
    ) -> Self {
        Self {
            authority: RuntimeWorkerMutationSourceAuthority::remote(identity),
            scope: scope.clone(),
            source_worker_id: source_worker_id.into(),
            transport: RuntimeWorkerMutationTransport::Remote {
                base_url: base_url.into().trim_end_matches('/').to_string(),
                request_source_signer: RuntimeRequestSourceSigner::from_identity(identity),
                request_source_audience: scope.server_id,
            },
        }
    }

    pub fn embedded(
        runtime_id: impl Into<String>,
        scope: RuntimeWorkspaceScope,
        source_worker_id: impl Into<String>,
        dispatcher: Arc<dyn EmbeddedWorkerMutationDispatcher>,
    ) -> Self {
        let runtime_id = runtime_id.into();
        Self {
            authority: RuntimeWorkerMutationSourceAuthority::embedded(
                &runtime_id,
                &scope.workspace_id,
            ),
            scope,
            source_worker_id: source_worker_id.into(),
            transport: RuntimeWorkerMutationTransport::Embedded { dispatcher },
        }
    }

    pub fn execute_worker_remove(
        &self,
        target_runtime_id: &str,
        target_worker_id: &str,
        reason: &str,
    ) -> Result<WorkspaceResponse, RuntimeWorkerMutationForwardError> {
        let proof = self.authority.issue_worker_remove(
            &self.scope,
            &self.source_worker_id,
            target_runtime_id,
            target_worker_id,
        )?;
        match (&self.transport, proof) {
            (
                RuntimeWorkerMutationTransport::Remote {
                    base_url,
                    request_source_signer,
                    request_source_audience,
                },
                RuntimeOwnedWorkerMutationProof::Remote(token),
            ) => execute_remote_worker_remove_http(RemoteWorkerRemoveHttpRequest {
                base_url: base_url.clone(),
                workspace_id: self.scope.workspace_id.clone(),
                source_worker_id: self.source_worker_id.clone(),
                request_source_signer: request_source_signer.clone(),
                request_source_audience: request_source_audience.clone(),
                token,
                target_runtime_id: target_runtime_id.to_string(),
                target_worker_id: target_worker_id.to_string(),
                reason: reason.to_string(),
            }),
            (
                RuntimeWorkerMutationTransport::Embedded { dispatcher },
                RuntimeOwnedWorkerMutationProof::InProcess(claims),
            ) => dispatcher.execute_worker_remove(
                claims,
                target_runtime_id,
                target_worker_id,
                reason,
            ),
            _ => Err(RuntimeWorkerMutationForwardError::AuthorityTransportMismatch),
        }
    }
}

struct RemoteWorkerRemoveHttpRequest {
    base_url: String,
    workspace_id: String,
    source_worker_id: String,
    request_source_signer: RuntimeRequestSourceSigner,
    request_source_audience: String,
    token: String,
    target_runtime_id: String,
    target_worker_id: String,
    reason: String,
}

fn execute_remote_worker_remove_http(
    request: RemoteWorkerRemoveHttpRequest,
) -> Result<WorkspaceResponse, RuntimeWorkerMutationForwardError> {
    if tokio::runtime::Handle::try_current().is_ok() {
        return std::thread::Builder::new()
            .name("yoi-worker-mutation-http".to_string())
            .spawn(move || execute_remote_worker_remove_http_blocking(request))
            .map_err(|error| {
                RuntimeWorkerMutationForwardError::Transport(format!(
                    "failed to start Worker mutation HTTP thread: {error}"
                ))
            })?
            .join()
            .map_err(|_| {
                RuntimeWorkerMutationForwardError::Transport(
                    "Worker mutation HTTP thread panicked".to_string(),
                )
            })?;
    }

    execute_remote_worker_remove_http_blocking(request)
}

fn execute_remote_worker_remove_http_blocking(
    request: RemoteWorkerRemoveHttpRequest,
) -> Result<WorkspaceResponse, RuntimeWorkerMutationForwardError> {
    let path = format!("/api/w/{}/workers/remove", request.workspace_id);
    let url = format!("{}{}", request.base_url, path);
    let body = serde_json::to_string(&serde_json::json!({
        "target_runtime_id": request.target_runtime_id,
        "target_worker_id": request.target_worker_id,
        "reason": request.reason,
    }))
    .map_err(|error| RuntimeWorkerMutationForwardError::Transport(error.to_string()))?;
    let request_source_proof = request.request_source_signer.issue(
        &request.request_source_audience,
        &request.workspace_id,
        Some(&request.source_worker_id),
        WORKSPACE_REQUEST_PERMISSION,
        "POST",
        &path,
        body.as_bytes(),
        i64::try_from(unix_now_seconds()).unwrap_or(i64::MAX),
        30,
    )?;
    let client = reqwest::blocking::Client::new();
    let response = client
        .post(url)
        .header(RUNTIME_REQUEST_SOURCE_PROOF_HEADER, request_source_proof)
        .header(
            crate::auth::WORKER_MUTATION_SOURCE_PROOF_HEADER,
            request.token,
        )
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(body)
        .send()
        .map_err(|error| RuntimeWorkerMutationForwardError::Transport(error.to_string()))?;
    let status = response.status().as_u16();
    let body = response
        .text()
        .map_err(|error| RuntimeWorkerMutationForwardError::Transport(error.to_string()))?;
    Ok(WorkspaceResponse { status, body })
}

#[derive(Clone)]
pub struct RuntimeOwnedWorkspaceClient {
    workspace_id: String,
    base_url: String,
    runtime_id: String,
    worker_id: String,
    request_timeout: Option<Duration>,
    worker_remove: Option<RuntimeWorkerMutationForwarder>,
    request_source_signer: Option<RuntimeRequestSourceSigner>,
    request_source_audience: Option<String>,
    prompt_projection_cache: Option<Arc<WorkspacePromptProjectionCache>>,
}

impl RuntimeOwnedWorkspaceClient {
    pub fn new(
        workspace_id: impl Into<String>,
        base_url: impl Into<String>,
        runtime_id: impl Into<String>,
        worker_id: impl Into<String>,
    ) -> Self {
        Self {
            workspace_id: workspace_id.into(),
            base_url: base_url.into().trim_end_matches('/').to_string(),
            runtime_id: runtime_id.into(),
            worker_id: worker_id.into(),
            request_timeout: None,
            worker_remove: None,
            request_source_signer: None,
            request_source_audience: None,
            prompt_projection_cache: None,
        }
    }

    pub fn with_worker_remove(mut self, worker_remove: RuntimeWorkerMutationForwarder) -> Self {
        self.worker_remove = Some(worker_remove);
        self
    }

    pub fn with_runtime_request_source(
        mut self,
        identity: &RuntimeIdentityMaterial,
        audience: impl Into<String>,
    ) -> Self {
        self.request_source_signer = Some(RuntimeRequestSourceSigner::from_identity(identity));
        self.request_source_audience = Some(audience.into());
        self
    }

    pub(crate) fn with_prompt_projection_cache(
        mut self,
        cache: Arc<WorkspacePromptProjectionCache>,
    ) -> Self {
        self.prompt_projection_cache = Some(cache);
        self
    }

    #[cfg(test)]
    fn with_request_timeout(mut self, request_timeout: Option<Duration>) -> Self {
        self.request_timeout = request_timeout;
        self
    }

    fn execute_with_permission(
        &self,
        request: WorkspaceRequest,
        permission: &'static str,
    ) -> Result<WorkspaceResponse, WorkspaceClientError> {
        let base_url = self.base_url.clone();
        let workspace_id = self.workspace_id.clone();
        let runtime_id = self.runtime_id.clone();
        let worker_id = self.worker_id.clone();
        let request_source_signer = self.request_source_signer.clone();
        let request_source_audience = self.request_source_audience.clone();
        let request_timeout = self.request_timeout;
        if tokio::runtime::Handle::try_current().is_ok() {
            std::thread::spawn(move || {
                execute_runtime_owned_workspace_http(
                    &base_url,
                    &workspace_id,
                    &runtime_id,
                    &worker_id,
                    request_source_signer.as_ref(),
                    request_source_audience.as_deref(),
                    request_timeout,
                    permission,
                    request,
                )
            })
            .join()
            .map_err(|_| {
                WorkspaceClientError::Request("workspace request thread panicked".to_string())
            })?
        } else {
            execute_runtime_owned_workspace_http(
                &self.base_url,
                &self.workspace_id,
                &self.runtime_id,
                &self.worker_id,
                self.request_source_signer.as_ref(),
                self.request_source_audience.as_deref(),
                self.request_timeout,
                permission,
                request,
            )
        }
    }
}

impl std::fmt::Debug for RuntimeOwnedWorkspaceClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RuntimeOwnedWorkspaceClient")
            .field("workspace_id", &self.workspace_id)
            .field("base_url", &self.base_url)
            .field("source", &"Runtime-owned")
            .field(
                "worker_remove",
                &self.worker_remove.as_ref().map(|_| "enabled"),
            )
            .finish()
    }
}

impl WorkspaceClient for RuntimeOwnedWorkspaceClient {
    fn workspace_id(&self) -> Option<&str> {
        Some(&self.workspace_id)
    }

    fn kind(&self) -> &str {
        "runtime-owned-workspace-client"
    }

    fn is_available(&self) -> bool {
        true
    }

    fn execute(
        &self,
        request: WorkspaceRequest,
    ) -> Result<WorkspaceResponse, WorkspaceClientError> {
        self.execute_with_permission(request, WORKSPACE_REQUEST_PERMISSION)
    }

    fn list_workspace_workers(
        &self,
        request: worker::WorkspaceWorkerDiscoveryRequest,
    ) -> Result<workspace_api::WorkspaceWorkerDiscoveryPage, WorkspaceClientError> {
        let mut path = format!(
            "/api/w/{}/worker-discovery/workers?limit={}",
            self.workspace_id, request.limit
        );
        if let Some(cursor) = request.cursor.as_deref() {
            path.push_str("&cursor=");
            path.push_str(&percent_encode_query(cursor));
        }
        if let Some(query) = request.query.as_deref() {
            path.push_str("&query=");
            path.push_str(&percent_encode_query(query));
        }
        let response = self.execute_with_permission(
            WorkspaceRequest::get(path),
            WORKSPACE_WORKER_DISCOVERY_PERMISSION,
        )?;
        if !(200..300).contains(&response.status) {
            return Err(WorkspaceClientError::Request(format!(
                "Workspace Worker discovery failed with HTTP {}: {}",
                response.status, response.body
            )));
        }
        serde_json::from_str(&response.body).map_err(|error| {
            WorkspaceClientError::Request(format!(
                "invalid Workspace Worker discovery response: {error}"
            ))
        })
    }

    fn current_prompt_projection(
        &self,
        minimum_revision: Option<u64>,
    ) -> Result<Option<WorkspacePromptCatalogResolution>, WorkspaceClientError> {
        let Some(cache) = self.prompt_projection_cache.as_ref() else {
            return Ok(None);
        };
        if let Some(resolution) = cache
            .active(&self.workspace_id)
            .map_err(WorkspaceClientError::Request)?
            .filter(|resolution| {
                minimum_revision
                    .map(|minimum| resolution.projection.config_revision >= minimum)
                    .unwrap_or(true)
            })
        {
            return Ok(Some((*resolution).clone()));
        }
        let fetch_gate = cache
            .fetch_gate(&self.workspace_id)
            .map_err(WorkspaceClientError::Request)?;
        let _fetch_guard = fetch_gate.lock().map_err(|_| {
            WorkspaceClientError::Request(
                "Workspace Prompt projection fetch gate was poisoned".to_string(),
            )
        })?;
        if let Some(resolution) = cache
            .active(&self.workspace_id)
            .map_err(WorkspaceClientError::Request)?
            .filter(|resolution| {
                minimum_revision
                    .map(|minimum| resolution.projection.config_revision >= minimum)
                    .unwrap_or(true)
            })
        {
            return Ok(Some((*resolution).clone()));
        }
        let response = self.execute(WorkspaceRequest::get(format!(
            "/api/w/{}/config/projections/prompts",
            self.workspace_id
        )))?;
        if !(200..300).contains(&response.status) {
            return Err(WorkspaceClientError::Request(format!(
                "active Workspace Prompt projection request failed with HTTP {}: {}",
                response.status, response.body
            )));
        }
        let projection: WorkspacePromptProjection =
            serde_json::from_str(&response.body).map_err(|error| {
                WorkspaceClientError::Request(format!(
                    "invalid active Workspace Prompt projection response: {error}"
                ))
            })?;
        if projection.workspace_id != self.workspace_id {
            return Err(WorkspaceClientError::Request(format!(
                "active Workspace Prompt projection scope mismatch: expected {}, got {}",
                self.workspace_id, projection.workspace_id
            )));
        }
        let resolution = cache
            .observe(projection)
            .map_err(WorkspaceClientError::Request)?;
        if let Some(minimum_revision) = minimum_revision
            && resolution.projection.config_revision < minimum_revision
        {
            return Err(WorkspaceClientError::Request(format!(
                "active Workspace Prompt projection is stale: required revision {minimum_revision}, got {}",
                resolution.projection.config_revision
            )));
        }
        Ok(Some((*resolution).clone()))
    }

    fn execute_worker_remove(
        &self,
        target_runtime_id: &str,
        target_worker_id: &str,
        reason: &str,
    ) -> Result<WorkspaceResponse, WorkspaceClientError> {
        self.worker_remove
            .as_ref()
            .ok_or_else(|| {
                WorkspaceClientError::Unavailable(
                    "Runtime-owned WorkerRemove forwarding is unavailable".to_string(),
                )
            })?
            .execute_worker_remove(target_runtime_id, target_worker_id, reason)
            .map_err(|error| WorkspaceClientError::Request(error.to_string()))
    }
}

fn percent_encode_query(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            use std::fmt::Write as _;
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

fn execute_runtime_owned_workspace_http(
    base_url: &str,
    workspace_id: &str,
    runtime_id: &str,
    worker_id: &str,
    request_source_signer: Option<&RuntimeRequestSourceSigner>,
    request_source_audience: Option<&str>,
    request_timeout: Option<Duration>,
    permission: &'static str,
    request: WorkspaceRequest,
) -> Result<WorkspaceResponse, WorkspaceClientError> {
    if !request.path.starts_with('/') || request.path.starts_with("//") {
        return Err(WorkspaceClientError::InvalidPath(request.path));
    }
    let url = format!("{base_url}{}", request.path);
    let method = match request.method {
        WorkspaceRequestMethod::Get => reqwest::Method::GET,
        WorkspaceRequestMethod::Post => reqwest::Method::POST,
        WorkspaceRequestMethod::Put => reqwest::Method::PUT,
        WorkspaceRequestMethod::Patch => reqwest::Method::PATCH,
        WorkspaceRequestMethod::Delete => reqwest::Method::DELETE,
    };
    let client = reqwest::blocking::Client::builder()
        .timeout(request_timeout)
        .build()
        .map_err(|error| {
            WorkspaceClientError::Unavailable(format!(
                "failed to build Workspace API HTTP client: {}",
                reqwest_error_chain(&error)
            ))
        })?;
    let request_label = format!("{method} {}", request.path);
    let body = request.body.unwrap_or_default();
    let mut request_builder = client
        .request(method.clone(), url)
        .header("x-yoi-runtime-id", runtime_id)
        .header("x-yoi-worker-id", worker_id);
    if let Some(signer) = request_source_signer {
        let audience = request_source_audience.ok_or_else(|| {
            WorkspaceClientError::Request(
                "runtime request proof audience is unavailable".to_owned(),
            )
        })?;
        let proof = signer
            .issue(
                audience,
                workspace_id,
                Some(worker_id),
                permission,
                method.as_str(),
                &request.path,
                body.as_bytes(),
                i64::try_from(unix_now_seconds()).unwrap_or(i64::MAX),
                30,
            )
            .map_err(|error| WorkspaceClientError::Request(error.to_string()))?;
        request_builder = request_builder.header(RUNTIME_REQUEST_SOURCE_PROOF_HEADER, proof);
    }
    if !body.is_empty() {
        request_builder = request_builder
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body);
    }
    let response = request_builder
        .send()
        .map_err(|error| workspace_http_error(&request_label, "waiting for response", error))?;
    let status = response.status().as_u16();
    let body = response
        .text()
        .map_err(|error| workspace_http_error(&request_label, "reading response body", error))?;
    Ok(WorkspaceResponse { status, body })
}

fn workspace_http_error(
    request_label: &str,
    stage: &str,
    error: reqwest::Error,
) -> WorkspaceClientError {
    let details = reqwest_error_chain(&error);
    if error.is_timeout() {
        WorkspaceClientError::Request(format!(
            "Workspace API {request_label} timed out while {stage}: {details}"
        ))
    } else if error.is_connect() {
        WorkspaceClientError::Unavailable(format!(
            "Workspace API {request_label} could not connect while {stage}: {details}"
        ))
    } else {
        WorkspaceClientError::Request(format!(
            "Workspace API {request_label} transport failed while {stage}: {details}"
        ))
    }
}

fn reqwest_error_chain(error: &reqwest::Error) -> String {
    let mut details = error.to_string();
    let mut source = std::error::Error::source(error);
    for _ in 0..4 {
        let Some(current) = source else {
            break;
        };
        let current_text = current.to_string();
        if !current_text.is_empty() && !details.ends_with(&current_text) {
            details.push_str(": ");
            details.push_str(&current_text);
        }
        source = std::error::Error::source(current);
    }
    details
}

#[derive(Debug, thiserror::Error)]
pub enum RuntimeWorkerMutationForwardError {
    #[error(transparent)]
    Auth(#[from] RuntimeAuthError),
    #[error("Worker mutation forwarding transport failed: {0}")]
    Transport(String),
    #[error("Worker mutation source authority does not match its forwarding transport")]
    AuthorityTransportMismatch,
    #[error("embedded Worker mutation dispatcher failed: {0}")]
    Embedded(String),
}

fn unix_now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{
        WorkerMutationSourceExpectation, decode_runtime_request_source_claims,
        decode_worker_mutation_source_claims, request_body_digest,
        verify_worker_mutation_source_proof,
    };

    #[test]
    fn current_prompt_projection_uses_the_shared_runtime_cache_without_http() {
        let cache = Arc::new(WorkspacePromptProjectionCache::default());
        let catalog = worker::EffectivePromptCatalog::new(
            std::collections::BTreeMap::from([(
                "default".to_string(),
                "workspace prompt".to_string(),
            )]),
            3,
            "schema",
            "toolchain",
        )
        .unwrap();
        let projection = WorkspacePromptProjection::new(
            "workspace-a",
            "source-3",
            catalog.catalog_digest.clone(),
            catalog,
        )
        .unwrap();
        cache.observe(projection).unwrap();
        let client = RuntimeOwnedWorkspaceClient::new(
            "workspace-a",
            "http://127.0.0.1:1",
            "runtime-a",
            "worker-a",
        )
        .with_prompt_projection_cache(cache);

        let projection = client.current_prompt_projection(None).unwrap().unwrap();
        let second = client.current_prompt_projection(None).unwrap().unwrap();

        assert_eq!(projection.projection.config_revision, 3);
        assert_eq!(projection.projection.source_digest, "source-3");
        assert!(Arc::ptr_eq(&projection.catalog, &second.catalog));
    }

    #[test]
    fn prompt_projection_minimum_revision_rejects_stale_server_response() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let catalog = worker::EffectivePromptCatalog::new(
            std::collections::BTreeMap::from([("default".to_string(), "stale prompt".to_string())]),
            3,
            "schema",
            "toolchain",
        )
        .unwrap();
        let projection = WorkspacePromptProjection::new(
            "workspace-a",
            "source-3",
            catalog.catalog_digest.clone(),
            catalog,
        )
        .unwrap();
        let body = serde_json::to_string(&projection).unwrap();
        let cache = Arc::new(WorkspacePromptProjectionCache::default());
        cache.observe(projection).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 4096];
            let _ = stream.read(&mut request).unwrap();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
        });
        let client =
            RuntimeOwnedWorkspaceClient::new("workspace-a", base_url, "runtime-a", "worker-a")
                .with_prompt_projection_cache(cache);

        let error = client.current_prompt_projection(Some(4)).unwrap_err();
        server.join().unwrap();

        assert!(error.to_string().contains("required revision 4, got 3"));
    }

    #[test]
    fn concurrent_prompt_projection_miss_fetches_once_and_shares_catalog() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::sync::Barrier;

        let catalog = worker::EffectivePromptCatalog::new(
            std::collections::BTreeMap::from([(
                "default".to_string(),
                "shared prompt".to_string(),
            )]),
            5,
            "schema",
            "toolchain",
        )
        .unwrap();
        let projection = WorkspacePromptProjection::new(
            "workspace-a",
            "source-5",
            catalog.catalog_digest.clone(),
            catalog,
        )
        .unwrap();
        let body = serde_json::to_string(&projection).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 4096];
            let read = stream.read(&mut request).unwrap();
            let request = std::str::from_utf8(&request[..read]).unwrap();
            assert!(
                request.contains("GET /api/w/workspace-a/config/projections/prompts "),
                "unexpected Prompt projection request: {request}"
            );
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
        });
        let cache = Arc::new(WorkspacePromptProjectionCache::default());
        let client =
            RuntimeOwnedWorkspaceClient::new("workspace-a", base_url, "runtime-a", "worker-a")
                .with_prompt_projection_cache(cache);
        let barrier = Arc::new(Barrier::new(8));
        let threads = (0..8)
            .map(|_| {
                let client = client.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    client.current_prompt_projection(None).unwrap().unwrap()
                })
            })
            .collect::<Vec<_>>();
        let resolutions = threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect::<Vec<_>>();
        server.join().unwrap();

        let first = &resolutions[0].catalog;
        assert!(
            resolutions
                .iter()
                .all(|resolution| Arc::ptr_eq(first, &resolution.catalog))
        );
    }

    #[test]
    fn current_prompt_projection_rejects_cross_workspace_response() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let catalog = worker::EffectivePromptCatalog::new(
            std::collections::BTreeMap::from([(
                "default".to_string(),
                "foreign prompt".to_string(),
            )]),
            4,
            "schema",
            "toolchain",
        )
        .unwrap();
        let projection = WorkspacePromptProjection::new(
            "workspace-b",
            "source-4",
            catalog.catalog_digest.clone(),
            catalog,
        )
        .unwrap();
        let body = serde_json::to_string(&projection).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 4096];
            let _ = stream.read(&mut request).unwrap();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
        });
        let client =
            RuntimeOwnedWorkspaceClient::new("workspace-a", base_url, "runtime-a", "worker-a")
                .with_prompt_projection_cache(Arc::new(WorkspacePromptProjectionCache::default()));

        let error = client.current_prompt_projection(None).unwrap_err();
        server.join().unwrap();

        assert!(error.to_string().contains("scope mismatch"));
    }

    #[test]
    fn ordinary_workspace_forwarding_stamps_runtime_identity_and_signs_path_and_query() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::sync::Mutex;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let received = Arc::new(Mutex::new(String::new()));
        let received_for_server = received.clone();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut bytes = [0_u8; 4096];
            let count = stream.read(&mut bytes).unwrap();
            *received_for_server.lock().unwrap() =
                String::from_utf8_lossy(&bytes[..count]).into_owned();
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}")
                .unwrap();
        });

        let identity = RuntimeIdentityMaterial::generate("runtime-a").unwrap();
        let client = RuntimeOwnedWorkspaceClient::new(
            "workspace-a",
            format!("http://{address}"),
            "runtime-a",
            "worker-a",
        )
        .with_runtime_request_source(&identity, "server-a");
        let response = client
            .execute(WorkspaceRequest::get(
                "/api/w/workspace-a/tickets/search?state=planning&limit=20",
            ))
            .unwrap();
        assert_eq!(response.status, 200);
        server.join().unwrap();
        let request = received.lock().unwrap().clone();
        let lowercase_request = request.to_ascii_lowercase();
        assert!(lowercase_request.contains("x-yoi-runtime-id: runtime-a"));
        assert!(lowercase_request.contains("x-yoi-worker-id: worker-a"));
        assert!(lowercase_request.contains("x-yoi-runtime-request-proof: yoi-runtime-request-v1."));
        assert!(!lowercase_request.contains("authorization:"));
        let token = request
            .lines()
            .find_map(|line| {
                line.split_once(':').and_then(|(name, value)| {
                    name.eq_ignore_ascii_case(RUNTIME_REQUEST_SOURCE_PROOF_HEADER)
                        .then(|| value.trim())
                })
            })
            .expect("runtime proof header");
        let claims = decode_runtime_request_source_claims(token).unwrap();
        assert_eq!(
            claims.path,
            "/api/w/workspace-a/tickets/search?state=planning&limit=20"
        );
    }

    #[test]
    fn workspace_worker_discovery_signs_dedicated_permission_and_encoded_query() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::sync::Mutex;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let received = Arc::new(Mutex::new(String::new()));
        let received_for_server = received.clone();
        let body = serde_json::json!({
            "workers": [{
                "subject": {
                    "kind": "runtime_worker",
                    "runtime_id": "runtime-b",
                    "worker_id": "worker-b"
                },
                "resource_key": "W-2",
                "display_name": "coder two",
                "profile": "builtin:coder",
                "status": "idle"
            }],
            "next_cursor": "v1:1"
        })
        .to_string();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut bytes = [0_u8; 4096];
            let count = stream.read(&mut bytes).unwrap();
            *received_for_server.lock().unwrap() =
                String::from_utf8_lossy(&bytes[..count]).into_owned();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
        });

        let identity = RuntimeIdentityMaterial::generate("runtime-a").unwrap();
        let client = RuntimeOwnedWorkspaceClient::new(
            "workspace-a",
            format!("http://{address}"),
            "runtime-a",
            "worker-a",
        )
        .with_runtime_request_source(&identity, "server-a");
        let page = client
            .list_workspace_workers(worker::WorkspaceWorkerDiscoveryRequest {
                cursor: Some("v1:0".to_string()),
                limit: 1,
                query: Some("coder two".to_string()),
            })
            .unwrap();
        assert_eq!(page.workers[0].resource_key, "W-2");
        server.join().unwrap();

        let request = received.lock().unwrap().clone();
        assert!(request.contains(
            "GET /api/w/workspace-a/worker-discovery/workers?limit=1&cursor=v1%3A0&query=coder%20two "
        ));
        let token = request
            .lines()
            .find_map(|line| {
                line.split_once(':').and_then(|(name, value)| {
                    name.eq_ignore_ascii_case(RUNTIME_REQUEST_SOURCE_PROOF_HEADER)
                        .then(|| value.trim())
                })
            })
            .unwrap();
        let claims = decode_runtime_request_source_claims(token).unwrap();
        assert_eq!(claims.permission, WORKSPACE_WORKER_DISCOVERY_PERMISSION);
        assert_eq!(
            claims.path,
            "/api/w/workspace-a/worker-discovery/workers?limit=1&cursor=v1%3A0&query=coder%20two"
        );
    }

    #[test]
    fn remote_authority_stamps_and_signs_worker_remove_without_caller_claim_choices() {
        let identity = RuntimeIdentityMaterial::generate("runtime-a").unwrap();
        let authority = RuntimeWorkerMutationSourceAuthority::remote(&identity);
        let scope = RuntimeWorkspaceScope {
            workspace_id: "workspace-a".to_string(),
            server_id: "server-a".to_string(),
        };

        let RuntimeOwnedWorkerMutationProof::Remote(token) = authority
            .issue_worker_remove(&scope, "worker-source", "runtime-b", "worker-target")
            .unwrap()
        else {
            panic!("remote authority must produce a signed proof");
        };
        let claims = decode_worker_mutation_source_claims(&token).unwrap();
        let expected = WorkerMutationSourceExpectation {
            runtime_id: "runtime-a",
            audience: "server-a",
            workspace_id: "workspace-a",
            worker_id: Some("worker-source"),
            actor_kind: WorkerMutationActorKind::Worker,
            operation: WorkerMutationOperation::WorkerRemove,
            target_runtime_id: "runtime-b",
            target_worker_id: "worker-target",
            permission: WORKER_REMOVE_PERMISSION,
        };
        assert_eq!(
            verify_worker_mutation_source_proof(
                &identity.public_key,
                &token,
                &expected,
                claims.iat
            )
            .unwrap(),
            claims
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn remote_forwarder_is_safe_in_async_runtime_and_stamps_signed_proof() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::sync::Mutex;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let received = Arc::new(Mutex::new(String::new()));
        let received_for_server = received.clone();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut bytes = [0_u8; 8192];
            let count = stream.read(&mut bytes).unwrap();
            *received_for_server.lock().unwrap() =
                String::from_utf8_lossy(&bytes[..count]).into_owned();
            stream
                .write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n")
                .unwrap();
        });

        let identity = RuntimeIdentityMaterial::generate("runtime-a").unwrap();
        let scope = RuntimeWorkspaceScope::new("workspace-a", "server-a");
        let forwarder = RuntimeWorkerMutationForwarder::remote(
            &identity,
            scope,
            "worker-source",
            format!("http://{address}"),
        );
        let response = forwarder
            .execute_worker_remove("runtime-target", "worker-target", "retire obsolete Worker")
            .unwrap();
        assert_eq!(response.status, 204);
        server.join().unwrap();

        let request = received.lock().unwrap().clone();
        assert!(request.starts_with("POST /api/w/workspace-a/workers/remove HTTP/1.1"));
        assert!(request.contains("\"target_runtime_id\":\"runtime-target\""));
        assert!(request.contains("\"target_worker_id\":\"worker-target\""));
        assert!(!request.contains("expected_worker_revision"));
        assert!(request.contains("\"reason\":\"retire obsolete Worker\""));
        let request_source_token = request
            .lines()
            .find_map(|line| {
                line.split_once(':').and_then(|(name, value)| {
                    name.eq_ignore_ascii_case(RUNTIME_REQUEST_SOURCE_PROOF_HEADER)
                        .then(|| value.trim())
                })
            })
            .expect("runtime request source proof header");
        let request_source_claims =
            decode_runtime_request_source_claims(request_source_token).unwrap();
        assert_eq!(request_source_claims.iss, "runtime-a");
        assert_eq!(request_source_claims.aud, "server-a");
        assert_eq!(request_source_claims.workspace_id, "workspace-a");
        assert_eq!(
            request_source_claims.worker_id.as_deref(),
            Some("worker-source")
        );
        assert_eq!(
            request_source_claims.permission,
            WORKSPACE_REQUEST_PERMISSION
        );
        assert_eq!(request_source_claims.method, "POST");
        assert_eq!(
            request_source_claims.path,
            "/api/w/workspace-a/workers/remove"
        );
        let request_body = request
            .split_once("\r\n\r\n")
            .map(|(_, body)| body)
            .expect("WorkerRemove request body");
        assert_eq!(
            request_source_claims.body_digest,
            request_body_digest(request_body.as_bytes())
        );
        let token = request
            .lines()
            .find_map(|line| {
                line.split_once(':').and_then(|(name, value)| {
                    name.eq_ignore_ascii_case(crate::auth::WORKER_MUTATION_SOURCE_PROOF_HEADER)
                        .then(|| value.trim())
                })
            })
            .expect("proof header");
        let claims = decode_worker_mutation_source_claims(token).unwrap();
        let expected = WorkerMutationSourceExpectation {
            runtime_id: "runtime-a",
            audience: "server-a",
            workspace_id: "workspace-a",
            worker_id: Some("worker-source"),
            actor_kind: WorkerMutationActorKind::Worker,
            operation: WorkerMutationOperation::WorkerRemove,
            target_runtime_id: "runtime-target",
            target_worker_id: "worker-target",
            permission: WORKER_REMOVE_PERMISSION,
        };
        verify_worker_mutation_source_proof(&identity.public_key, token, &expected, claims.iat)
            .unwrap();
    }

    #[test]
    fn embedded_forwarder_delivers_in_process_proof_with_the_request() {
        use std::sync::Mutex;

        #[derive(Default)]
        struct RecordingDispatcher {
            seen: Mutex<Option<(WorkerMutationSourceClaims, String, String, String)>>,
        }
        impl EmbeddedWorkerMutationDispatcher for RecordingDispatcher {
            fn execute_worker_remove(
                &self,
                proof: InProcessWorkerMutationProof,
                target_runtime_id: &str,
                target_worker_id: &str,
                reason: &str,
            ) -> Result<WorkspaceResponse, RuntimeWorkerMutationForwardError> {
                *self.seen.lock().unwrap() = Some((
                    proof.into_claims(),
                    target_runtime_id.to_string(),
                    target_worker_id.to_string(),
                    reason.to_string(),
                ));
                Ok(WorkspaceResponse {
                    status: 202,
                    body: "accepted".to_string(),
                })
            }
        }

        let dispatcher = Arc::new(RecordingDispatcher::default());
        let scope = RuntimeWorkspaceScope::new("workspace-a", "server-a");
        let forwarder = RuntimeWorkerMutationForwarder::embedded(
            "runtime-embedded",
            scope,
            "worker-source",
            dispatcher.clone(),
        );
        let response = forwarder
            .execute_worker_remove("runtime-target", "worker-target", "retire obsolete Worker")
            .unwrap();
        assert_eq!(response.status, 202);
        let (claims, target_runtime_id, target_worker_id, reason) =
            dispatcher.seen.lock().unwrap().take().unwrap();
        assert_eq!(claims.iss, "runtime-embedded");
        assert_eq!(claims.worker_id, "worker-source");
        assert_eq!(claims.target_runtime_id, "runtime-target");
        assert_eq!(claims.target_worker_id, "worker-target");
        assert_eq!(target_runtime_id, "runtime-target");
        assert_eq!(target_worker_id, "worker-target");
        assert_eq!(reason, "retire obsolete Worker");
    }

    #[test]
    fn runtime_owned_workspace_client_has_no_fixed_request_timeout() {
        let (base_url, server) = delayed_workspace_response(Duration::from_millis(75));
        let client =
            RuntimeOwnedWorkspaceClient::new("workspace-a", base_url, "runtime-a", "worker-a");
        assert_eq!(client.request_timeout, None);

        let response = client
            .execute(WorkspaceRequest::json(
                WorkspaceRequestMethod::Post,
                "/api/test",
                "{}",
            ))
            .unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(response.body, r#"{"ok":true}"#);
        server.join().unwrap();
    }

    #[test]
    fn runtime_owned_workspace_client_reports_request_timeouts() {
        let (base_url, server) = delayed_workspace_response(Duration::from_millis(75));
        let client =
            RuntimeOwnedWorkspaceClient::new("workspace-a", base_url, "runtime-a", "worker-a")
                .with_request_timeout(Some(Duration::from_millis(20)));

        let error = client
            .execute(WorkspaceRequest::json(
                WorkspaceRequestMethod::Post,
                "/api/test",
                "{}",
            ))
            .unwrap_err();
        assert!(matches!(error, WorkspaceClientError::Request(_)));
        let message = error.to_string();
        assert!(message.contains("POST /api/test"), "{message}");
        assert!(message.contains("timed out"), "{message}");
        server.join().unwrap();
    }

    fn delayed_workspace_response(delay: Duration) -> (String, std::thread::JoinHandle<()>) {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut bytes = [0_u8; 8192];
            let _ = stream.read(&mut bytes);
            std::thread::sleep(delay);
            let _ = stream.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 11\r\nConnection: close\r\n\r\n{\"ok\":true}",
            );
        });
        (format!("http://{address}"), server)
    }

    #[test]
    fn embedded_authority_uses_the_same_claim_contract_without_a_credential() {
        let authority =
            RuntimeWorkerMutationSourceAuthority::embedded("runtime-embedded", "workspace-a");
        let scope = RuntimeWorkspaceScope {
            workspace_id: "workspace-a".to_string(),
            server_id: "server-unused-for-embedded".to_string(),
        };

        let RuntimeOwnedWorkerMutationProof::InProcess(proof) = authority
            .issue_worker_remove(&scope, "worker-source", "runtime-b", "worker-target")
            .unwrap()
        else {
            panic!("embedded authority must produce an in-process proof");
        };
        let claims = proof.claims();
        assert_eq!(claims.iss, "runtime-embedded");
        assert_eq!(claims.aud, "embedded:workspace-a");
        assert_eq!(claims.workspace_id, "workspace-a");
        assert_eq!(claims.worker_id, "worker-source");
        assert_eq!(claims.operation, WorkerMutationOperation::WorkerRemove);
        assert_eq!(claims.target_runtime_id, "runtime-b");
        assert_eq!(claims.target_worker_id, "worker-target");
        assert_eq!(claims.permission, WORKER_REMOVE_PERMISSION);
        assert!(!claims.jti.is_empty());
    }
}
