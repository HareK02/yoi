use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use worker::{
    WorkspaceClient, WorkspaceClientError, WorkspacePromptCatalogResolution,
    WorkspacePromptProjection, WorkspaceRequest, WorkspaceRequestMethod, WorkspaceResponse,
};

use crate::auth::{
    RuntimeAuthError, RuntimeIdentityMaterial, RuntimeWorkerMutationSourceSigner,
    WORKER_REMOVE_PERMISSION, WorkerMutationActorKind, WorkerMutationOperation,
    WorkerMutationSourceClaims, new_token_id,
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
        expected_worker_revision: &str,
        reason: &str,
    ) -> Result<WorkspaceResponse, RuntimeWorkerMutationForwardError>;
}

#[derive(Clone)]
enum RuntimeWorkerMutationTransport {
    Remote {
        base_url: String,
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
            scope,
            source_worker_id: source_worker_id.into(),
            transport: RuntimeWorkerMutationTransport::Remote {
                base_url: base_url.into().trim_end_matches('/').to_string(),
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
        expected_worker_revision: &str,
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
                RuntimeWorkerMutationTransport::Remote { base_url },
                RuntimeOwnedWorkerMutationProof::Remote(token),
            ) => execute_remote_worker_remove_http(RemoteWorkerRemoveHttpRequest {
                base_url: base_url.clone(),
                workspace_id: self.scope.workspace_id.clone(),
                token,
                target_runtime_id: target_runtime_id.to_string(),
                target_worker_id: target_worker_id.to_string(),
                expected_worker_revision: expected_worker_revision.to_string(),
                reason: reason.to_string(),
            }),
            (
                RuntimeWorkerMutationTransport::Embedded { dispatcher },
                RuntimeOwnedWorkerMutationProof::InProcess(claims),
            ) => dispatcher.execute_worker_remove(
                claims,
                target_runtime_id,
                target_worker_id,
                expected_worker_revision,
                reason,
            ),
            _ => Err(RuntimeWorkerMutationForwardError::AuthorityTransportMismatch),
        }
    }
}

struct RemoteWorkerRemoveHttpRequest {
    base_url: String,
    workspace_id: String,
    token: String,
    target_runtime_id: String,
    target_worker_id: String,
    expected_worker_revision: String,
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
    let url = format!(
        "{}/api/w/{}/workers/remove",
        request.base_url, request.workspace_id
    );
    let body = serde_json::json!({
        "target_runtime_id": request.target_runtime_id,
        "target_worker_id": request.target_worker_id,
        "expected_worker_revision": request.expected_worker_revision,
        "reason": request.reason,
    });
    let client = reqwest::blocking::Client::new();
    let response = client
        .post(url)
        .header(
            crate::auth::WORKER_MUTATION_SOURCE_PROOF_HEADER,
            request.token,
        )
        .json(&body)
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
            prompt_projection_cache: None,
        }
    }

    pub fn with_worker_remove(mut self, worker_remove: RuntimeWorkerMutationForwarder) -> Self {
        self.worker_remove = Some(worker_remove);
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
        let base_url = self.base_url.clone();
        let runtime_id = self.runtime_id.clone();
        let worker_id = self.worker_id.clone();
        let request_timeout = self.request_timeout;
        if tokio::runtime::Handle::try_current().is_ok() {
            std::thread::spawn(move || {
                execute_runtime_owned_workspace_http(
                    &base_url,
                    &runtime_id,
                    &worker_id,
                    request_timeout,
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
                &self.runtime_id,
                &self.worker_id,
                self.request_timeout,
                request,
            )
        }
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
            "/api/w/{}/config-sources/active/prompt-projection",
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
        expected_worker_revision: &str,
        reason: &str,
    ) -> Result<WorkspaceResponse, WorkspaceClientError> {
        self.worker_remove
            .as_ref()
            .ok_or_else(|| {
                WorkspaceClientError::Unavailable(
                    "Runtime-owned WorkerRemove forwarding is unavailable".to_string(),
                )
            })?
            .execute_worker_remove(
                target_runtime_id,
                target_worker_id,
                expected_worker_revision,
                reason,
            )
            .map_err(|error| WorkspaceClientError::Request(error.to_string()))
    }
}

fn execute_runtime_owned_workspace_http(
    base_url: &str,
    runtime_id: &str,
    worker_id: &str,
    request_timeout: Option<Duration>,
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
    let mut request_builder = client
        .request(method, url)
        .header("x-yoi-runtime-id", runtime_id)
        .header("x-yoi-worker-id", worker_id);
    if let Some(body) = request.body {
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
        WorkerMutationSourceExpectation, decode_worker_mutation_source_claims,
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
            let _ = stream.read(&mut request).unwrap();
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
    fn ordinary_workspace_forwarding_stamps_legacy_source_only_inside_runtime() {
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

        let client = RuntimeOwnedWorkspaceClient::new(
            "workspace-a",
            format!("http://{address}"),
            "runtime-a",
            "worker-a",
        );
        let response = client
            .execute(WorkspaceRequest::get("/api/w/workspace-a/tickets/search"))
            .unwrap();
        assert_eq!(response.status, 200);
        server.join().unwrap();
        let request = received.lock().unwrap().to_ascii_lowercase();
        assert!(request.contains("x-yoi-runtime-id: runtime-a"));
        assert!(request.contains("x-yoi-worker-id: worker-a"));
        assert!(!request.contains("authorization:"));
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
            .execute_worker_remove(
                "runtime-target",
                "worker-target",
                "revision-7",
                "retire obsolete Worker",
            )
            .unwrap();
        assert_eq!(response.status, 204);
        server.join().unwrap();

        let request = received.lock().unwrap().clone();
        assert!(request.starts_with("POST /api/w/workspace-a/workers/remove HTTP/1.1"));
        assert!(request.contains("\"target_runtime_id\":\"runtime-target\""));
        assert!(request.contains("\"target_worker_id\":\"worker-target\""));
        assert!(request.contains("\"expected_worker_revision\":\"revision-7\""));
        assert!(request.contains("\"reason\":\"retire obsolete Worker\""));
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
            seen: Mutex<Option<(WorkerMutationSourceClaims, String, String, String, String)>>,
        }
        impl EmbeddedWorkerMutationDispatcher for RecordingDispatcher {
            fn execute_worker_remove(
                &self,
                proof: InProcessWorkerMutationProof,
                target_runtime_id: &str,
                target_worker_id: &str,
                expected_worker_revision: &str,
                reason: &str,
            ) -> Result<WorkspaceResponse, RuntimeWorkerMutationForwardError> {
                *self.seen.lock().unwrap() = Some((
                    proof.into_claims(),
                    target_runtime_id.to_string(),
                    target_worker_id.to_string(),
                    expected_worker_revision.to_string(),
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
            .execute_worker_remove(
                "runtime-target",
                "worker-target",
                "revision-7",
                "retire obsolete Worker",
            )
            .unwrap();
        assert_eq!(response.status, 202);
        let (claims, target_runtime_id, target_worker_id, expected_revision, reason) =
            dispatcher.seen.lock().unwrap().take().unwrap();
        assert_eq!(claims.iss, "runtime-embedded");
        assert_eq!(claims.worker_id, "worker-source");
        assert_eq!(claims.target_runtime_id, "runtime-target");
        assert_eq!(claims.target_worker_id, "worker-target");
        assert_eq!(target_runtime_id, "runtime-target");
        assert_eq!(target_worker_id, "worker-target");
        assert_eq!(expected_revision, "revision-7");
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
