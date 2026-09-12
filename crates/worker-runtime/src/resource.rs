use crate::auth::BACKEND_RESOURCE_FETCH_PERMISSION;
use crate::identity::WorkerId;
use crate::profile_archive::{ProfileSourceArchive, ProfileSourceArchiveRef, sha256_hex};
use crate::workspace_request::{RuntimeWorkspaceRequest, RuntimeWorkspaceRequestClient};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

pub const PROFILE_SOURCE_ARCHIVE_CONTENT_TYPE: &str =
    "application/vnd.yoi.profile-source-archive+tar";
pub const REPOSITORY_SSH_ACCESS_CONTENT_TYPE: &str =
    "application/vnd.yoi.repository-ssh-access+json";
pub const DEFAULT_PROFILE_SOURCE_ARCHIVE_MAX_BYTES: u64 = 2 * 1024 * 1024;
pub const DEFAULT_REPOSITORY_SSH_ACCESS_MAX_BYTES: u64 = 64 * 1024;
pub const DEFAULT_BACKEND_RESOURCE_FETCH_TIMEOUT: std::time::Duration =
    std::time::Duration::from_secs(15);

#[derive(Clone, Serialize, Deserialize)]
pub struct RepositorySshAccessSecretCandidate {
    pub credential_id: String,
    pub credential_revision: u64,
    pub private_key: String,
}

impl Drop for RepositorySshAccessSecretCandidate {
    fn drop(&mut self) {
        zeroize::Zeroize::zeroize(&mut self.private_key);
    }
}

impl std::fmt::Debug for RepositorySshAccessSecretCandidate {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RepositorySshAccessSecretCandidate")
            .field("credential_id", &self.credential_id)
            .field("credential_revision", &self.credential_revision)
            .field("private_key", &"[REDACTED]")
            .finish()
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct RepositorySshAccessSecret {
    pub credential_candidates: Vec<RepositorySshAccessSecretCandidate>,
    pub known_hosts_entry: String,
}

impl Drop for RepositorySshAccessSecret {
    fn drop(&mut self) {
        zeroize::Zeroize::zeroize(&mut self.known_hosts_entry);
    }
}

impl std::fmt::Debug for RepositorySshAccessSecret {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RepositorySshAccessSecret")
            .field("credential_candidates", &self.credential_candidates)
            .field("known_hosts_entry", &"[REDACTED]")
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendResourceKind {
    ProfileSourceArchive,
    RepositorySshAccess,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendResourceOperation {
    FetchArchive,
    FetchOnce,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendResourceHandle {
    pub kind: BackendResourceKind,
    pub workspace_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_id: Option<String>,
    pub resource_id: String,
    pub digest: String,
    pub operation: BackendResourceOperation,
    pub expires_at_unix_seconds: i64,
    pub nonce: String,
    pub revision: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<u64>,
    pub max_bytes: u64,
    pub content_type: String,
    pub redaction: ResourceRedactionPolicy,
    pub audit_correlation_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_source_graph: Option<crate::profile_archive::ProfileSourceGraphSummary>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceRedactionPolicy {
    RuntimeInternalOnly,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendResourceFetchRequest {
    pub handle: BackendResourceHandle,
    pub runtime_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_id: Option<String>,
    pub audit_correlation_id: String,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendResourceFetchResponse {
    pub kind: BackendResourceKind,
    pub resource_id: String,
    pub digest: String,
    pub content_type: String,
    pub bytes: Vec<u8>,
    pub audit_correlation_id: String,
}

impl std::fmt::Debug for BackendResourceFetchResponse {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BackendResourceFetchResponse")
            .field("kind", &self.kind)
            .field("resource_id", &self.resource_id)
            .field("digest", &self.digest)
            .field("content_type", &self.content_type)
            .field(
                "bytes",
                &format_args!("[REDACTED; {} bytes]", self.bytes.len()),
            )
            .field("audit_correlation_id", &self.audit_correlation_id)
            .finish()
    }
}

impl Drop for BackendResourceFetchResponse {
    fn drop(&mut self) {
        self.bytes.fill(0);
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum BackendResourceError {
    #[error("backend resource handle is expired")]
    Expired,
    #[error("backend resource handle is unauthorized: {message}")]
    Unauthorized { message: String },
    #[error("backend resource kind is unsupported")]
    UnsupportedKind,
    #[error("backend resource is missing")]
    MissingResource,
    #[error("backend resource digest mismatch: expected {expected}, got {actual}")]
    DigestMismatch { expected: String, actual: String },
    #[error("backend resource response is oversized: limit {max_bytes}, actual {actual_bytes}")]
    Oversized { max_bytes: u64, actual_bytes: u64 },
    #[error("backend resource content type mismatch: expected {expected}, got {actual}")]
    ContentTypeMismatch { expected: String, actual: String },
    #[error("backend resource fetch timed out")]
    Timeout,
    #[error("backend resource transport failed: {message}")]
    Transport { message: String },
    #[error("backend resource response is invalid: {message}")]
    InvalidResponse { message: String },
}

#[async_trait]
pub trait BackendResourceClient: Send + Sync + 'static {
    async fn fetch_resource(
        &self,
        request: BackendResourceFetchRequest,
    ) -> Result<BackendResourceFetchResponse, BackendResourceError>;
}

#[cfg(feature = "http-server")]
#[derive(Clone, Debug)]
pub struct HttpBackendResourceClient {
    endpoint: String,
    bearer_token: Option<String>,
    workspace_request_client: Option<RuntimeWorkspaceRequestClient>,
    request_timeout: std::time::Duration,
}

#[cfg(feature = "http-server")]
impl HttpBackendResourceClient {
    pub fn new(endpoint: impl Into<String>, bearer_token: Option<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            bearer_token,
            workspace_request_client: None,
            request_timeout: DEFAULT_BACKEND_RESOURCE_FETCH_TIMEOUT,
        }
    }

    pub fn with_request_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.request_timeout = timeout;
        self
    }

    pub fn with_workspace_request_client(mut self, client: RuntimeWorkspaceRequestClient) -> Self {
        self.workspace_request_client = Some(client);
        self
    }
}

#[cfg(feature = "http-server")]
#[async_trait]
impl BackendResourceClient for HttpBackendResourceClient {
    async fn fetch_resource(
        &self,
        request: BackendResourceFetchRequest,
    ) -> Result<BackendResourceFetchResponse, BackendResourceError> {
        let body = serde_json::to_vec(&request).map_err(|error| {
            BackendResourceError::InvalidResponse {
                message: error.to_string(),
            }
        })?;
        let endpoint = reqwest::Url::parse(&self.endpoint).map_err(|error| {
            BackendResourceError::Transport {
                message: error.to_string(),
            }
        })?;
        let client = self.workspace_request_client.as_ref().ok_or_else(|| {
            BackendResourceError::Unauthorized {
                message: "Workspace request client is unavailable".to_string(),
            }
        })?;
        if client.workspace_id() != request.handle.workspace_id {
            return Err(BackendResourceError::Unauthorized {
                message: "Workspace request client does not match the resource workspace"
                    .to_string(),
            });
        }
        let base_url = client.base_url().trim_end_matches('/');
        let endpoint_text = endpoint.as_str();
        let endpoint_suffix = endpoint_text.strip_prefix(base_url).ok_or_else(|| {
            BackendResourceError::Unauthorized {
                message: "Workspace resource endpoint does not match its request client"
                    .to_string(),
            }
        })?;
        if !endpoint_suffix.starts_with('/') {
            return Err(BackendResourceError::Unauthorized {
                message: "Workspace resource endpoint does not match its request client"
                    .to_string(),
            });
        }
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::CONTENT_TYPE,
            reqwest::header::HeaderValue::from_static("application/json"),
        );
        if let Some(token) = self.bearer_token.as_deref() {
            let value = reqwest::header::HeaderValue::from_str(&format!("Bearer {token}"))
                .map_err(|error| BackendResourceError::Transport {
                    message: error.to_string(),
                })?;
            headers.insert(reqwest::header::AUTHORIZATION, value);
        }
        let response = client
            .execute(RuntimeWorkspaceRequest {
                method: reqwest::Method::POST,
                path_and_query: endpoint_suffix.to_string(),
                body,
                headers,
                permission: BACKEND_RESOURCE_FETCH_PERMISSION.to_string(),
                worker_id: None,
                timeout: Some(self.request_timeout),
                max_response_bytes: 8 * 1024 * 1024,
            })
            .await
            .map_err(|error| {
                if error.is_timeout() {
                    BackendResourceError::Timeout
                } else {
                    BackendResourceError::Transport {
                        message: error.to_string(),
                    }
                }
            })?;
        if response.status.is_success() {
            serde_json::from_slice::<BackendResourceFetchResponse>(&response.body).map_err(|err| {
                BackendResourceError::InvalidResponse {
                    message: err.to_string(),
                }
            })
        } else {
            let status = response.status;
            match serde_json::from_slice::<BackendResourceError>(&response.body) {
                Ok(error) => Err(error),
                Err(err) => Err(BackendResourceError::Transport {
                    message: format!("backend resource fetch failed with HTTP {status}: {err}"),
                }),
            }
        }
    }
}

pub fn build_profile_source_archive_fetch_request(
    handle: BackendResourceHandle,
    runtime_id: &str,
    worker_id: Option<&WorkerId>,
) -> BackendResourceFetchRequest {
    let audit_correlation_id = handle.audit_correlation_id.clone();
    BackendResourceFetchRequest {
        handle,
        runtime_id: runtime_id.to_string(),
        worker_id: worker_id.map(|id| id.to_string()),
        audit_correlation_id,
    }
}

pub fn profile_source_archive_from_response(
    handle: &BackendResourceHandle,
    mut response: BackendResourceFetchResponse,
) -> Result<ProfileSourceArchive, BackendResourceError> {
    if handle.kind != BackendResourceKind::ProfileSourceArchive
        || response.kind != BackendResourceKind::ProfileSourceArchive
    {
        return Err(BackendResourceError::UnsupportedKind);
    }
    if handle.operation != BackendResourceOperation::FetchArchive {
        return Err(BackendResourceError::Unauthorized {
            message: "resource handle operation is not fetch_archive".to_string(),
        });
    }
    if response.content_type != handle.content_type {
        return Err(BackendResourceError::ContentTypeMismatch {
            expected: handle.content_type.clone(),
            actual: response.content_type.clone(),
        });
    }
    let actual_bytes = response.bytes.len() as u64;
    if actual_bytes > handle.max_bytes {
        return Err(BackendResourceError::Oversized {
            max_bytes: handle.max_bytes,
            actual_bytes,
        });
    }
    let actual_digest = sha256_hex(&response.bytes);
    if actual_digest != handle.digest || response.digest != handle.digest {
        return Err(BackendResourceError::DigestMismatch {
            expected: handle.digest.clone(),
            actual: if response.digest != handle.digest {
                response.digest.clone()
            } else {
                actual_digest
            },
        });
    }
    Ok(ProfileSourceArchive {
        reference: ProfileSourceArchiveRef {
            id: handle.resource_id.clone(),
            digest: handle.digest.clone(),
            size_bytes: actual_bytes,
            source_graph: handle.profile_source_graph.clone().ok_or_else(|| {
                BackendResourceError::InvalidResponse {
                    message: "profile source archive handle omitted source graph summary"
                        .to_string(),
                }
            })?,
        },
        content: std::mem::take(&mut response.bytes),
    })
}

pub fn validate_resource_handle_text(label: &str, value: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err(format!("{label} must not be empty"));
    }
    if value.len() > 256 || value.contains('\0') || value.contains('\n') || value.contains('\r') {
        return Err(format!("{label} contains unsupported boundary text"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::RuntimeIdentityMaterial;
    use crate::profile_archive::ProfileSourceGraphSummary;
    use std::collections::BTreeMap;

    fn graph() -> ProfileSourceGraphSummary {
        ProfileSourceGraphSummary {
            source_count: 1,
            total_source_bytes: 4,
            entrypoints: BTreeMap::from([(
                "default".to_string(),
                "profiles/default.dcdl".to_string(),
            )]),
            import_count: 0,
        }
    }

    fn handle_for(bytes: &[u8]) -> BackendResourceHandle {
        BackendResourceHandle {
            kind: BackendResourceKind::ProfileSourceArchive,
            workspace_id: "workspace-test".to_string(),
            scope_id: Some("workspace-profile-source".to_string()),
            runtime_id: Some("runtime-test".to_string()),
            worker_id: Some("worker-test".to_string()),
            resource_id: "profile-source-archive:test".to_string(),
            digest: sha256_hex(bytes),
            operation: BackendResourceOperation::FetchArchive,
            expires_at_unix_seconds: 4_102_444_800,
            nonce: "nonce-test".to_string(),
            revision: sha256_hex(bytes),
            generation: Some(1),
            max_bytes: 1024,
            content_type: PROFILE_SOURCE_ARCHIVE_CONTENT_TYPE.to_string(),
            redaction: ResourceRedactionPolicy::RuntimeInternalOnly,
            audit_correlation_id: "audit-test".to_string(),
            profile_source_graph: Some(graph()),
        }
    }

    #[cfg(feature = "http-server")]
    #[tokio::test]
    async fn http_backend_resource_fetch_has_a_bounded_timeout() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            futures::future::pending::<()>().await;
            drop(stream);
        });
        let base_url = format!("http://{address}");
        let identity = RuntimeIdentityMaterial::generate("runtime-test").unwrap();
        let handle = handle_for(b"archive-bytes");
        let client = HttpBackendResourceClient::new(format!("{base_url}/fetch"), None)
            .with_request_timeout(std::time::Duration::from_millis(25))
            .with_workspace_request_client(
                RuntimeWorkspaceRequestClient::new(
                    "workspace-test",
                    base_url.clone(),
                    "runtime-test",
                )
                .with_runtime_request_source(&identity, base_url),
            );

        let error = client
            .fetch_resource(BackendResourceFetchRequest {
                audit_correlation_id: handle.audit_correlation_id.clone(),
                handle,
                runtime_id: "runtime-test".to_string(),
                worker_id: None,
            })
            .await
            .unwrap_err();

        server.abort();
        assert_eq!(error, BackendResourceError::Timeout);
    }

    #[test]
    fn repository_ssh_access_secret_debug_redacts_all_secret_values() {
        let secret = RepositorySshAccessSecret {
            credential_candidates: vec![RepositorySshAccessSecretCandidate {
                credential_id: "credential-1".to_string(),
                credential_revision: 2,
                private_key: "PRIVATE KEY secret bytes".to_string(),
            }],
            known_hosts_entry: "host key secret bytes".to_string(),
        };

        let debug = format!("{secret:?}");
        assert!(debug.contains("credential-1"));
        assert!(!debug.contains("secret bytes"));
        assert_eq!(debug.matches("[REDACTED]").count(), 2);
    }

    #[test]
    fn response_verification_detects_digest_mismatch() {
        let bytes = b"archive-bytes";
        let handle = handle_for(bytes);
        let error = profile_source_archive_from_response(
            &handle,
            BackendResourceFetchResponse {
                kind: BackendResourceKind::ProfileSourceArchive,
                resource_id: handle.resource_id.clone(),
                digest: handle.digest.clone(),
                content_type: PROFILE_SOURCE_ARCHIVE_CONTENT_TYPE.to_string(),
                bytes: b"tampered-bytes".to_vec(),
                audit_correlation_id: handle.audit_correlation_id.clone(),
            },
        )
        .unwrap_err();
        assert!(matches!(error, BackendResourceError::DigestMismatch { .. }));
    }

    #[test]
    fn response_verification_rejects_oversized_bytes() {
        let bytes = b"archive-bytes";
        let mut handle = handle_for(bytes);
        handle.max_bytes = 2;
        let error = profile_source_archive_from_response(
            &handle,
            BackendResourceFetchResponse {
                kind: BackendResourceKind::ProfileSourceArchive,
                resource_id: handle.resource_id.clone(),
                digest: handle.digest.clone(),
                content_type: PROFILE_SOURCE_ARCHIVE_CONTENT_TYPE.to_string(),
                bytes: bytes.to_vec(),
                audit_correlation_id: handle.audit_correlation_id.clone(),
            },
        )
        .unwrap_err();
        assert!(matches!(error, BackendResourceError::Oversized { .. }));
    }
}
