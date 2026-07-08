use async_trait::async_trait;
use chrono::{Duration, Utc};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use uuid::Uuid;
use worker_runtime::identity::{RuntimeId, WorkerId};
use worker_runtime::profile_archive::ProfileSourceArchive;
use worker_runtime::resource::{
    BackendResourceClient, BackendResourceError, BackendResourceFetchRequest,
    BackendResourceFetchResponse, BackendResourceHandle, BackendResourceKind,
    BackendResourceOperation, DEFAULT_PROFILE_SOURCE_ARCHIVE_MAX_BYTES,
    PROFILE_SOURCE_ARCHIVE_CONTENT_TYPE, ResourceRedactionPolicy,
};

#[derive(Clone, Default)]
pub struct BackendResourceBroker {
    resources: Arc<Mutex<HashMap<String, StoredResource>>>,
}

#[derive(Clone)]
struct StoredResource {
    runtime_id: Option<String>,
    worker_id: Option<String>,
    handle: BackendResourceHandle,
    archive: ProfileSourceArchive,
}

impl BackendResourceBroker {
    pub fn issue_profile_source_archive_handle(
        &self,
        workspace_id: impl Into<String>,
        runtime_id: Option<&RuntimeId>,
        worker_id: Option<&WorkerId>,
        archive: ProfileSourceArchive,
    ) -> BackendResourceHandle {
        let workspace_id = workspace_id.into();
        let nonce = Uuid::now_v7().to_string();
        let audit_correlation_id = format!("resource-fetch-{nonce}");
        let expires_at = Utc::now() + Duration::minutes(15);
        let handle = BackendResourceHandle {
            kind: BackendResourceKind::ProfileSourceArchive,
            workspace_id: workspace_id.clone(),
            scope_id: Some("workspace-profile-source".to_string()),
            runtime_id: runtime_id.map(|id| id.as_str().to_string()),
            worker_id: worker_id.map(|id| id.as_str().to_string()),
            resource_id: archive.reference.id.clone(),
            digest: archive.reference.digest.clone(),
            operation: BackendResourceOperation::FetchArchive,
            expires_at_unix_seconds: expires_at.timestamp(),
            nonce: nonce.clone(),
            revision: archive.reference.digest.clone(),
            generation: None,
            max_bytes: DEFAULT_PROFILE_SOURCE_ARCHIVE_MAX_BYTES,
            content_type: PROFILE_SOURCE_ARCHIVE_CONTENT_TYPE.to_string(),
            redaction: ResourceRedactionPolicy::RuntimeInternalOnly,
            audit_correlation_id,
            profile_source_graph: Some(archive.reference.source_graph.clone()),
        };
        let stored = StoredResource {
            runtime_id: runtime_id.map(|id| id.as_str().to_string()),
            worker_id: worker_id.map(|id| id.as_str().to_string()),
            handle: handle.clone(),
            archive,
        };
        if let Ok(mut resources) = self.resources.lock() {
            resources.insert(nonce, stored);
        }
        handle
    }

    pub fn fetch_profile_source_archive(
        &self,
        request: BackendResourceFetchRequest,
    ) -> Result<BackendResourceFetchResponse, BackendResourceError> {
        verify_handle_shape(&request.handle)?;
        let stored = self
            .resources
            .lock()
            .map_err(|_| BackendResourceError::Transport {
                message: "resource broker lock poisoned".to_string(),
            })?
            .get(&request.handle.nonce)
            .cloned()
            .ok_or(BackendResourceError::MissingResource)?;
        verify_handle_shape(&stored.handle)?;
        if stored.handle.expires_at_unix_seconds < Utc::now().timestamp() {
            return Err(BackendResourceError::Expired);
        }
        let actual_bytes = stored.archive.content.len() as u64;
        if actual_bytes > stored.handle.max_bytes {
            return Err(BackendResourceError::Oversized {
                max_bytes: stored.handle.max_bytes,
                actual_bytes,
            });
        }
        if request.handle != stored.handle {
            return Err(BackendResourceError::Unauthorized {
                message: "resource handle does not match broker-issued handle".to_string(),
            });
        }
        if let Some(expected_runtime_id) = stored.runtime_id.as_deref() {
            if expected_runtime_id != request.runtime_id {
                return Err(BackendResourceError::Unauthorized {
                    message: "runtime id does not match resource handle".to_string(),
                });
            }
        }
        if let Some(expected_worker_id) = stored.worker_id.as_deref() {
            if Some(expected_worker_id) != request.worker_id.as_deref() {
                return Err(BackendResourceError::Unauthorized {
                    message: "worker id does not match resource handle".to_string(),
                });
            }
        }
        Ok(BackendResourceFetchResponse {
            kind: BackendResourceKind::ProfileSourceArchive,
            resource_id: stored.archive.reference.id,
            digest: stored.archive.reference.digest,
            content_type: PROFILE_SOURCE_ARCHIVE_CONTENT_TYPE.to_string(),
            bytes: stored.archive.content,
            audit_correlation_id: request.audit_correlation_id,
        })
    }
}

#[async_trait]
impl BackendResourceClient for BackendResourceBroker {
    async fn fetch_resource(
        &self,
        request: BackendResourceFetchRequest,
    ) -> Result<BackendResourceFetchResponse, BackendResourceError> {
        self.fetch_profile_source_archive(request)
    }
}

fn verify_handle_shape(handle: &BackendResourceHandle) -> Result<(), BackendResourceError> {
    if handle.kind != BackendResourceKind::ProfileSourceArchive {
        return Err(BackendResourceError::UnsupportedKind);
    }
    if handle.operation != BackendResourceOperation::FetchArchive {
        return Err(BackendResourceError::Unauthorized {
            message: "resource handle operation is not fetch_archive".to_string(),
        });
    }
    if handle.content_type != PROFILE_SOURCE_ARCHIVE_CONTENT_TYPE {
        return Err(BackendResourceError::ContentTypeMismatch {
            expected: PROFILE_SOURCE_ARCHIVE_CONTENT_TYPE.to_string(),
            actual: handle.content_type.clone(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use worker_runtime::identity::{RuntimeId, WorkerId};
    use worker_runtime::profile_archive::{
        ProfileSourceArchive, ProfileSourceArchiveRef, ProfileSourceGraphSummary, sha256_hex,
    };

    fn archive() -> ProfileSourceArchive {
        let content = b"archive-content".to_vec();
        let mut entrypoints = BTreeMap::new();
        entrypoints.insert("default".to_string(), "profiles/default.dcdl".to_string());
        ProfileSourceArchive {
            reference: ProfileSourceArchiveRef {
                id: "profile-source-archive:test".to_string(),
                digest: sha256_hex(&content),
                size_bytes: content.len() as u64,
                source_graph: ProfileSourceGraphSummary {
                    entrypoints,
                    source_count: 1,
                    import_count: 0,
                    total_source_bytes: content.len() as u64,
                },
            },
            content,
        }
    }

    fn archive_with_len(len: usize) -> ProfileSourceArchive {
        let content = vec![b'x'; len];
        let mut entrypoints = BTreeMap::new();
        entrypoints.insert("default".to_string(), "profiles/default.dcdl".to_string());
        ProfileSourceArchive {
            reference: ProfileSourceArchiveRef {
                id: format!("profile-source-archive:test-{len}"),
                digest: sha256_hex(&content),
                size_bytes: content.len() as u64,
                source_graph: ProfileSourceGraphSummary {
                    entrypoints,
                    source_count: 1,
                    import_count: 0,
                    total_source_bytes: content.len() as u64,
                },
            },
            content,
        }
    }

    fn request(
        handle: BackendResourceHandle,
        runtime_id: &RuntimeId,
        worker_id: Option<&WorkerId>,
    ) -> BackendResourceFetchRequest {
        BackendResourceFetchRequest {
            audit_correlation_id: handle.audit_correlation_id.clone(),
            handle,
            runtime_id: runtime_id.as_str().to_string(),
            worker_id: worker_id.map(|id| id.as_str().to_string()),
        }
    }

    #[test]
    fn broker_issues_and_verifies_profile_source_archive_handles() {
        let broker = BackendResourceBroker::default();
        let runtime_id = RuntimeId::new("runtime-test").unwrap();
        let handle = broker.issue_profile_source_archive_handle(
            "workspace-test",
            Some(&runtime_id),
            None,
            archive(),
        );
        let response = broker
            .fetch_profile_source_archive(BackendResourceFetchRequest {
                handle: handle.clone(),
                runtime_id: runtime_id.as_str().to_string(),
                worker_id: None,
                audit_correlation_id: handle.audit_correlation_id.clone(),
            })
            .expect("fetch succeeds");
        assert_eq!(response.digest, handle.digest);
        assert_eq!(response.content_type, PROFILE_SOURCE_ARCHIVE_CONTENT_TYPE);
    }

    #[test]
    fn broker_rejects_runtime_mismatch() {
        let broker = BackendResourceBroker::default();
        let runtime_a = RuntimeId::new("runtime-a").unwrap();
        let handle = broker.issue_profile_source_archive_handle(
            "workspace-test",
            Some(&runtime_a),
            None,
            archive(),
        );
        let err = broker
            .fetch_profile_source_archive(request(
                handle,
                &RuntimeId::new("runtime-b").unwrap(),
                None,
            ))
            .unwrap_err();
        assert!(matches!(err, BackendResourceError::Unauthorized { .. }));
    }

    #[test]
    fn broker_rejects_worker_mismatch() {
        let broker = BackendResourceBroker::default();
        let runtime_id = RuntimeId::new("runtime-test").unwrap();
        let worker_a = WorkerId::new("worker-a").unwrap();
        let worker_b = WorkerId::new("worker-b").unwrap();
        let handle = broker.issue_profile_source_archive_handle(
            "workspace-test",
            Some(&runtime_id),
            Some(&worker_a),
            archive(),
        );
        let err = broker
            .fetch_profile_source_archive(request(handle, &runtime_id, Some(&worker_b)))
            .unwrap_err();
        assert!(matches!(err, BackendResourceError::Unauthorized { .. }));
    }

    #[test]
    fn broker_rejects_expiry_extension_from_request_handle() {
        let broker = BackendResourceBroker::default();
        let runtime_id = RuntimeId::new("runtime-test").unwrap();
        let handle = broker.issue_profile_source_archive_handle(
            "workspace-test",
            Some(&runtime_id),
            None,
            archive(),
        );
        broker
            .resources
            .lock()
            .unwrap()
            .get_mut(&handle.nonce)
            .unwrap()
            .handle
            .expires_at_unix_seconds = 1;
        let mut extended = handle;
        extended.expires_at_unix_seconds = 4_102_444_800;
        let err = broker
            .fetch_profile_source_archive(request(extended, &runtime_id, None))
            .unwrap_err();
        assert!(matches!(err, BackendResourceError::Expired));
    }

    #[test]
    fn broker_rejects_policy_tampered_request_handle() {
        let broker = BackendResourceBroker::default();
        let runtime_id = RuntimeId::new("runtime-test").unwrap();
        let mut handle = broker.issue_profile_source_archive_handle(
            "workspace-test",
            Some(&runtime_id),
            None,
            archive(),
        );
        handle.scope_id = Some("tampered-scope".to_string());
        let err = broker
            .fetch_profile_source_archive(request(handle, &runtime_id, None))
            .unwrap_err();
        assert!(matches!(err, BackendResourceError::Unauthorized { .. }));
    }

    #[test]
    fn broker_uses_stored_max_bytes_when_request_handle_is_tampered() {
        let broker = BackendResourceBroker::default();
        let runtime_id = RuntimeId::new("runtime-test").unwrap();
        let archive = archive_with_len((DEFAULT_PROFILE_SOURCE_ARCHIVE_MAX_BYTES + 1) as usize);
        let mut handle = broker.issue_profile_source_archive_handle(
            "workspace-test",
            Some(&runtime_id),
            None,
            archive,
        );
        handle.max_bytes = DEFAULT_PROFILE_SOURCE_ARCHIVE_MAX_BYTES + 1024;
        let err = broker
            .fetch_profile_source_archive(request(handle, &runtime_id, None))
            .unwrap_err();
        assert!(matches!(err, BackendResourceError::Oversized { .. }));
    }
}
