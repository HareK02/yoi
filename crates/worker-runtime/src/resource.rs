use crate::identity::{RuntimeId, WorkerId};
use crate::profile_archive::{ProfileSourceArchive, ProfileSourceArchiveRef, sha256_hex};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;

pub const PROFILE_SOURCE_ARCHIVE_CONTENT_TYPE: &str =
    "application/vnd.yoi.profile-source-archive+tar";
pub const DEFAULT_PROFILE_SOURCE_ARCHIVE_MAX_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendResourceKind {
    ProfileSourceArchive,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendResourceOperation {
    FetchArchive,
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendResourceFetchResponse {
    pub kind: BackendResourceKind,
    pub resource_id: String,
    pub digest: String,
    pub content_type: String,
    pub bytes: Vec<u8>,
    pub audit_correlation_id: String,
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
    client: reqwest::Client,
}

#[cfg(feature = "http-server")]
impl HttpBackendResourceClient {
    pub fn new(endpoint: impl Into<String>, bearer_token: Option<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            bearer_token,
            client: reqwest::Client::new(),
        }
    }
}

#[cfg(feature = "http-server")]
#[async_trait]
impl BackendResourceClient for HttpBackendResourceClient {
    async fn fetch_resource(
        &self,
        request: BackendResourceFetchRequest,
    ) -> Result<BackendResourceFetchResponse, BackendResourceError> {
        let builder = self.client.post(&self.endpoint).json(&request);
        let builder = if let Some(token) = self.bearer_token.as_deref() {
            builder.bearer_auth(token)
        } else {
            builder
        };
        let response = builder
            .send()
            .await
            .map_err(|err| BackendResourceError::Transport {
                message: err.to_string(),
            })?;
        if response.status().is_success() {
            response
                .json::<BackendResourceFetchResponse>()
                .await
                .map_err(|err| BackendResourceError::InvalidResponse {
                    message: err.to_string(),
                })
        } else {
            let status = response.status();
            match response.json::<BackendResourceError>().await {
                Ok(error) => Err(error),
                Err(err) => Err(BackendResourceError::Transport {
                    message: format!("backend resource fetch failed with HTTP {status}: {err}"),
                }),
            }
        }
    }
}

#[derive(Default, Debug)]
pub struct ProfileSourceArchiveCache {
    archives: Mutex<HashMap<String, ProfileSourceArchive>>,
}

impl ProfileSourceArchiveCache {
    pub fn get(&self, digest: &str) -> Option<ProfileSourceArchive> {
        self.archives.lock().ok()?.get(digest).cloned()
    }

    pub fn insert(&self, archive: ProfileSourceArchive) {
        if let Ok(mut archives) = self.archives.lock() {
            archives.insert(archive.reference.digest.clone(), archive);
        }
    }
}

pub fn build_profile_source_archive_fetch_request(
    handle: BackendResourceHandle,
    runtime_id: &RuntimeId,
    worker_id: Option<&WorkerId>,
) -> BackendResourceFetchRequest {
    let audit_correlation_id = handle.audit_correlation_id.clone();
    BackendResourceFetchRequest {
        handle,
        runtime_id: runtime_id.as_str().to_string(),
        worker_id: worker_id.map(|id| id.to_string()),
        audit_correlation_id,
    }
}

pub fn profile_source_archive_from_response(
    handle: &BackendResourceHandle,
    response: BackendResourceFetchResponse,
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
            actual: response.content_type,
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
                response.digest
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
        content: response.bytes,
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
