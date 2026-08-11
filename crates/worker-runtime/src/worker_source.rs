use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use worker::{
    WorkspaceClient, WorkspaceClientError, WorkspaceRequest, WorkspaceRequestMethod,
    WorkspaceResponse,
};

use crate::auth::{
    RuntimeAuthError, RuntimeIdentityMaterial, RuntimeWorkerMutationSourceSigner,
    WORKER_REMOVE_PERMISSION, WorkerMutationActorKind, WorkerMutationOperation,
    WorkerMutationSourceClaims, new_token_id,
};
use crate::runtime::RuntimeWorkspaceScope;

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
    ) -> Result<WorkspaceResponse, RuntimeWorkerMutationForwardError>;
}

#[derive(Clone)]
enum RuntimeWorkerMutationTransport {
    Remote {
        base_url: String,
        client: reqwest::blocking::Client,
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
                client: reqwest::blocking::Client::new(),
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
    ) -> Result<WorkspaceResponse, RuntimeWorkerMutationForwardError> {
        let proof = self.authority.issue_worker_remove(
            &self.scope,
            &self.source_worker_id,
            target_runtime_id,
            target_worker_id,
        )?;
        match (&self.transport, proof) {
            (
                RuntimeWorkerMutationTransport::Remote { base_url, client },
                RuntimeOwnedWorkerMutationProof::Remote(token),
            ) => {
                let url = format!(
                    "{base_url}/api/w/{}/workers/remove",
                    self.scope.workspace_id
                );
                let body = serde_json::json!({
                    "target_runtime_id": target_runtime_id,
                    "target_worker_id": target_worker_id,
                });
                let response = client
                    .post(url)
                    .header(crate::auth::WORKER_MUTATION_SOURCE_PROOF_HEADER, token)
                    .json(&body)
                    .send()
                    .map_err(|error| {
                        RuntimeWorkerMutationForwardError::Transport(error.to_string())
                    })?;
                let status = response.status().as_u16();
                let body = response.text().map_err(|error| {
                    RuntimeWorkerMutationForwardError::Transport(error.to_string())
                })?;
                Ok(WorkspaceResponse { status, body })
            }
            (
                RuntimeWorkerMutationTransport::Embedded { dispatcher },
                RuntimeOwnedWorkerMutationProof::InProcess(claims),
            ) => dispatcher.execute_worker_remove(claims, target_runtime_id, target_worker_id),
            _ => Err(RuntimeWorkerMutationForwardError::AuthorityTransportMismatch),
        }
    }
}

pub struct RuntimeOwnedWorkspaceClient {
    workspace_id: String,
    base_url: String,
    runtime_id: String,
    worker_id: String,
    worker_remove: Option<RuntimeWorkerMutationForwarder>,
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
            worker_remove: None,
        }
    }

    pub fn with_worker_remove(mut self, worker_remove: RuntimeWorkerMutationForwarder) -> Self {
        self.worker_remove = Some(worker_remove);
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
        if tokio::runtime::Handle::try_current().is_ok() {
            std::thread::spawn(move || {
                execute_runtime_owned_workspace_http(&base_url, &runtime_id, &worker_id, request)
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
                request,
            )
        }
    }

    fn execute_worker_remove(
        &self,
        target_runtime_id: &str,
        target_worker_id: &str,
    ) -> Result<WorkspaceResponse, WorkspaceClientError> {
        self.worker_remove
            .as_ref()
            .ok_or_else(|| {
                WorkspaceClientError::Unavailable(
                    "Runtime-owned WorkerRemove forwarding is unavailable".to_string(),
                )
            })?
            .execute_worker_remove(target_runtime_id, target_worker_id)
            .map_err(|error| WorkspaceClientError::Request(error.to_string()))
    }
}

fn execute_runtime_owned_workspace_http(
    base_url: &str,
    runtime_id: &str,
    worker_id: &str,
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
    let client = reqwest::blocking::Client::new();
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
        .map_err(|error| WorkspaceClientError::Request(error.to_string()))?;
    let status = response.status().as_u16();
    let body = response
        .text()
        .map_err(|error| WorkspaceClientError::Request(error.to_string()))?;
    Ok(WorkspaceResponse { status, body })
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

    #[test]
    fn remote_forwarder_stamps_signed_proof_inside_runtime_before_http_delivery() {
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
            .execute_worker_remove("runtime-target", "worker-target")
            .unwrap();
        assert_eq!(response.status, 204);
        server.join().unwrap();

        let request = received.lock().unwrap().clone();
        assert!(request.starts_with("POST /api/w/workspace-a/workers/remove HTTP/1.1"));
        assert!(request.contains(
            r#"{"target_runtime_id":"runtime-target","target_worker_id":"worker-target"}"#
        ));
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
            seen: Mutex<Option<(WorkerMutationSourceClaims, String, String)>>,
        }
        impl EmbeddedWorkerMutationDispatcher for RecordingDispatcher {
            fn execute_worker_remove(
                &self,
                proof: InProcessWorkerMutationProof,
                target_runtime_id: &str,
                target_worker_id: &str,
            ) -> Result<WorkspaceResponse, RuntimeWorkerMutationForwardError> {
                *self.seen.lock().unwrap() = Some((
                    proof.into_claims(),
                    target_runtime_id.to_string(),
                    target_worker_id.to_string(),
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
            .execute_worker_remove("runtime-target", "worker-target")
            .unwrap();
        assert_eq!(response.status, 202);
        let (claims, target_runtime_id, target_worker_id) =
            dispatcher.seen.lock().unwrap().take().unwrap();
        assert_eq!(claims.iss, "runtime-embedded");
        assert_eq!(claims.worker_id, "worker-source");
        assert_eq!(claims.target_runtime_id, "runtime-target");
        assert_eq!(claims.target_worker_id, "worker-target");
        assert_eq!(target_runtime_id, "runtime-target");
        assert_eq!(target_worker_id, "worker-target");
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
