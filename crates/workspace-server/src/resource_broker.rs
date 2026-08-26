use async_trait::async_trait;
use chrono::{Duration, Utc};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use uuid::Uuid;
use worker_runtime::identity::RuntimeWorkerRef;
use worker_runtime::profile_archive::ProfileSourceArchive;
use worker_runtime::resource::{
    BackendResourceClient, BackendResourceError, BackendResourceFetchRequest,
    BackendResourceFetchResponse, BackendResourceHandle, BackendResourceKind,
    BackendResourceOperation, DEFAULT_PROFILE_SOURCE_ARCHIVE_MAX_BYTES,
    DEFAULT_REPOSITORY_SSH_ACCESS_MAX_BYTES, PROFILE_SOURCE_ARCHIVE_CONTENT_TYPE,
    REPOSITORY_SSH_ACCESS_CONTENT_TYPE, RepositorySshAccessSecret, ResourceRedactionPolicy,
};

#[derive(Clone, Default)]
pub struct BackendResourceBroker {
    resources: Arc<Mutex<HashMap<String, StoredResource>>>,
}

#[derive(Clone, Copy, Debug)]
pub enum BackendResourceTarget<'a> {
    Workspace,
    Runtime(&'a str),
    Worker(&'a RuntimeWorkerRef),
}

#[derive(Clone)]
struct StoredResource {
    runtime_id: Option<String>,
    worker: Option<RuntimeWorkerRef>,
    handle: BackendResourceHandle,
    bytes: Vec<u8>,
    archive: Option<ProfileSourceArchive>,
    one_shot: bool,
}

impl StoredResource {
    fn byte_len(&self) -> usize {
        self.archive
            .as_ref()
            .map(|archive| archive.content.len())
            .unwrap_or_else(|| self.bytes.len())
    }

    fn take_bytes(&mut self) -> Vec<u8> {
        self.archive
            .as_mut()
            .map(|archive| std::mem::take(&mut archive.content))
            .unwrap_or_else(|| std::mem::take(&mut self.bytes))
    }
}

impl Drop for StoredResource {
    fn drop(&mut self) {
        self.bytes.fill(0);
        if let Some(archive) = self.archive.as_mut() {
            archive.content.fill(0);
        }
    }
}

impl BackendResourceBroker {
    pub fn issue_profile_source_archive_handle(
        &self,
        workspace_id: impl Into<String>,
        target: BackendResourceTarget<'_>,
        archive: ProfileSourceArchive,
    ) -> BackendResourceHandle {
        let workspace_id = workspace_id.into();
        let (runtime_id, worker) = match target {
            BackendResourceTarget::Workspace => (None, None),
            BackendResourceTarget::Runtime(runtime_id) => (Some(runtime_id.to_string()), None),
            BackendResourceTarget::Worker(worker) => {
                (Some(worker.runtime_id.clone()), Some(worker.clone()))
            }
        };
        let nonce = Uuid::now_v7().to_string();
        let audit_correlation_id = format!("resource-fetch-{nonce}");
        let expires_at = Utc::now() + Duration::minutes(15);
        let handle = BackendResourceHandle {
            kind: BackendResourceKind::ProfileSourceArchive,
            workspace_id: workspace_id.clone(),
            scope_id: Some("workspace-profile-source".to_string()),
            runtime_id: runtime_id.clone(),
            worker_id: worker.as_ref().map(|worker| worker.worker_id.clone()),
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
            runtime_id,
            worker,
            handle: handle.clone(),
            bytes: Vec::new(),
            archive: Some(archive),
            one_shot: false,
        };
        if let Ok(mut resources) = self.resources.lock() {
            resources.insert(nonce, stored);
        }
        handle
    }

    pub fn issue_repository_ssh_access_handle(
        &self,
        workspace_id: impl Into<String>,
        runtime_id: &str,
        resource_id: impl Into<String>,
        revision: impl Into<String>,
        expires_at_unix_seconds: i64,
        secret: RepositorySshAccessSecret,
    ) -> Result<BackendResourceHandle, BackendResourceError> {
        let bytes =
            serde_json::to_vec(&secret).map_err(|error| BackendResourceError::InvalidResponse {
                message: error.to_string(),
            })?;
        if bytes.len() as u64 > DEFAULT_REPOSITORY_SSH_ACCESS_MAX_BYTES {
            return Err(BackendResourceError::Oversized {
                max_bytes: DEFAULT_REPOSITORY_SSH_ACCESS_MAX_BYTES,
                actual_bytes: bytes.len() as u64,
            });
        }
        let workspace_id = workspace_id.into();
        let resource_id = resource_id.into();
        let revision = revision.into();
        let nonce = Uuid::now_v7().to_string();
        let handle = BackendResourceHandle {
            kind: BackendResourceKind::RepositorySshAccess,
            workspace_id,
            scope_id: Some("repository-ssh-access".to_string()),
            runtime_id: Some(runtime_id.to_string()),
            worker_id: None,
            resource_id,
            digest: format!("opaque:{nonce}"),
            operation: BackendResourceOperation::FetchOnce,
            expires_at_unix_seconds,
            nonce: nonce.clone(),
            revision,
            generation: None,
            max_bytes: DEFAULT_REPOSITORY_SSH_ACCESS_MAX_BYTES,
            content_type: REPOSITORY_SSH_ACCESS_CONTENT_TYPE.to_string(),
            redaction: ResourceRedactionPolicy::RuntimeInternalOnly,
            audit_correlation_id: format!("repository-ssh-access-{nonce}"),
            profile_source_graph: None,
        };
        let stored = StoredResource {
            runtime_id: Some(runtime_id.to_string()),
            worker: None,
            handle: handle.clone(),
            bytes,
            archive: None,
            one_shot: true,
        };
        let resource_key = nonce.clone();
        self.resources
            .lock()
            .map_err(|_| BackendResourceError::Transport {
                message: "resource broker lock poisoned".to_string(),
            })?
            .insert(resource_key.clone(), stored);
        if expires_at_unix_seconds != i64::MAX {
            let resources = self.resources.clone();
            std::thread::spawn(move || {
                let now = Utc::now().timestamp();
                if expires_at_unix_seconds > now {
                    std::thread::sleep(std::time::Duration::from_secs(
                        (expires_at_unix_seconds - now) as u64,
                    ));
                }
                if let Ok(mut resources) = resources.lock()
                    && resources
                        .get(&resource_key)
                        .is_some_and(|stored| stored.handle.nonce == resource_key)
                {
                    resources.remove(&resource_key);
                }
            });
        }
        Ok(handle)
    }

    pub fn profile_source_archive(
        &self,
        digest: &str,
    ) -> Option<worker_runtime::profile_archive::ProfileSourceArchive> {
        self.resources
            .lock()
            .ok()?
            .values()
            .find(|resource| resource.handle.digest == digest)
            .and_then(|resource| resource.archive.clone())
    }

    pub fn fetch_resource(
        &self,
        request: BackendResourceFetchRequest,
    ) -> Result<BackendResourceFetchResponse, BackendResourceError> {
        verify_handle_shape(&request.handle)?;
        let mut resources = self
            .resources
            .lock()
            .map_err(|_| BackendResourceError::Transport {
                message: "resource broker lock poisoned".to_string(),
            })?;
        let mut stored = resources
            .get(&request.handle.nonce)
            .cloned()
            .ok_or(BackendResourceError::MissingResource)?;
        verify_handle_shape(&stored.handle)?;
        if stored.handle.expires_at_unix_seconds < Utc::now().timestamp() {
            return Err(BackendResourceError::Expired);
        }
        let actual_bytes = stored.byte_len() as u64;
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
        if let Some(expected_worker) = stored.worker.as_ref() {
            if expected_worker.runtime_id != request.runtime_id
                || Some(expected_worker.worker_id.as_str()) != request.worker_id.as_deref()
            {
                return Err(BackendResourceError::Unauthorized {
                    message: "worker id does not match resource handle".to_string(),
                });
            }
        }
        if stored.one_shot {
            resources.remove(&request.handle.nonce);
        }
        Ok(BackendResourceFetchResponse {
            kind: stored.handle.kind.clone(),
            resource_id: stored.handle.resource_id.clone(),
            digest: stored.handle.digest.clone(),
            content_type: stored.handle.content_type.clone(),
            bytes: stored.take_bytes(),
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
        self.fetch_resource(request)
    }
}

fn verify_handle_shape(handle: &BackendResourceHandle) -> Result<(), BackendResourceError> {
    match handle.kind {
        BackendResourceKind::ProfileSourceArchive => {
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
        }
        BackendResourceKind::RepositorySshAccess => {
            if handle.operation != BackendResourceOperation::FetchOnce {
                return Err(BackendResourceError::Unauthorized {
                    message: "resource handle operation is not fetch_once".to_string(),
                });
            }
            if handle.content_type != REPOSITORY_SSH_ACCESS_CONTENT_TYPE {
                return Err(BackendResourceError::ContentTypeMismatch {
                    expected: REPOSITORY_SSH_ACCESS_CONTENT_TYPE.to_string(),
                    actual: handle.content_type.clone(),
                });
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
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
        runtime_id: &str,
        worker_id: Option<&str>,
    ) -> BackendResourceFetchRequest {
        BackendResourceFetchRequest {
            audit_correlation_id: handle.audit_correlation_id.clone(),
            handle,
            runtime_id: runtime_id.to_string(),
            worker_id: worker_id.map(str::to_string),
        }
    }

    #[test]
    fn broker_issues_and_verifies_profile_source_archive_handles() {
        let broker = BackendResourceBroker::default();
        let runtime_id = "runtime-test";
        let handle = broker.issue_profile_source_archive_handle(
            "workspace-test",
            BackendResourceTarget::Runtime(runtime_id),
            archive(),
        );
        let response = broker
            .fetch_resource(BackendResourceFetchRequest {
                handle: handle.clone(),
                runtime_id: runtime_id.to_string(),
                worker_id: None,
                audit_correlation_id: handle.audit_correlation_id.clone(),
            })
            .expect("fetch succeeds");
        assert_eq!(response.digest, handle.digest);
        assert_eq!(response.content_type, PROFILE_SOURCE_ARCHIVE_CONTENT_TYPE);
    }

    #[test]
    fn repository_ssh_access_resource_is_runtime_bound_and_one_shot() {
        let broker = BackendResourceBroker::default();
        let handle = broker
            .issue_repository_ssh_access_handle(
                "workspace-test",
                "runtime-test",
                "repository-access-test",
                "1",
                i64::MAX,
                RepositorySshAccessSecret {
                    private_key: "private-key-bytes".to_string(),
                    known_hosts_entry: "known-hosts-entry".to_string(),
                },
            )
            .unwrap();
        let unauthorized = broker
            .fetch_resource(request(handle.clone(), "runtime-other", None))
            .unwrap_err();
        assert!(matches!(
            unauthorized,
            BackendResourceError::Unauthorized { .. }
        ));

        let response = broker
            .fetch_resource(request(handle.clone(), "runtime-test", None))
            .unwrap();
        assert_eq!(response.kind, BackendResourceKind::RepositorySshAccess);
        assert_eq!(response.content_type, REPOSITORY_SSH_ACCESS_CONTENT_TYPE);
        let debug = format!("{response:?}");
        assert!(!debug.contains("private-key-bytes"));
        assert!(debug.contains("REDACTED"));
        let secret: RepositorySshAccessSecret = serde_json::from_slice(&response.bytes).unwrap();
        assert_eq!(secret.private_key, "private-key-bytes");
        assert!(matches!(
            broker.fetch_resource(request(handle, "runtime-test", None)),
            Err(BackendResourceError::MissingResource)
        ));
    }

    #[test]
    fn broker_rejects_runtime_mismatch() {
        let broker = BackendResourceBroker::default();
        let runtime_a = "runtime-a";
        let handle = broker.issue_profile_source_archive_handle(
            "workspace-test",
            BackendResourceTarget::Runtime(runtime_a),
            archive(),
        );
        let err = broker
            .fetch_resource(request(handle, "runtime-b", None))
            .unwrap_err();
        assert!(matches!(err, BackendResourceError::Unauthorized { .. }));
    }

    #[test]
    fn broker_rejects_worker_mismatch() {
        let broker = BackendResourceBroker::default();
        let runtime_id = "runtime-test";
        let worker_a = RuntimeWorkerRef::new(runtime_id, "1");
        let worker_b = RuntimeWorkerRef::new(runtime_id, "2");
        let handle = broker.issue_profile_source_archive_handle(
            "workspace-test",
            BackendResourceTarget::Worker(&worker_a),
            archive(),
        );
        let err = broker
            .fetch_resource(request(handle, runtime_id, Some(&worker_b.worker_id)))
            .unwrap_err();
        assert!(matches!(err, BackendResourceError::Unauthorized { .. }));
    }

    #[test]
    fn broker_rejects_expiry_extension_from_request_handle() {
        let broker = BackendResourceBroker::default();
        let runtime_id = "runtime-test";
        let handle = broker.issue_profile_source_archive_handle(
            "workspace-test",
            BackendResourceTarget::Runtime(runtime_id),
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
            .fetch_resource(request(extended, &runtime_id, None))
            .unwrap_err();
        assert!(matches!(err, BackendResourceError::Expired));
    }

    #[test]
    fn broker_rejects_policy_tampered_request_handle() {
        let broker = BackendResourceBroker::default();
        let runtime_id = "runtime-test";
        let mut handle = broker.issue_profile_source_archive_handle(
            "workspace-test",
            BackendResourceTarget::Runtime(runtime_id),
            archive(),
        );
        handle.scope_id = Some("tampered-scope".to_string());
        let err = broker
            .fetch_resource(request(handle, &runtime_id, None))
            .unwrap_err();
        assert!(matches!(err, BackendResourceError::Unauthorized { .. }));
    }

    #[test]
    fn broker_uses_stored_max_bytes_when_request_handle_is_tampered() {
        let broker = BackendResourceBroker::default();
        let runtime_id = "runtime-test";
        let archive = archive_with_len((DEFAULT_PROFILE_SOURCE_ARCHIVE_MAX_BYTES + 1) as usize);
        let mut handle = broker.issue_profile_source_archive_handle(
            "workspace-test",
            BackendResourceTarget::Runtime(runtime_id),
            archive,
        );
        handle.max_bytes = DEFAULT_PROFILE_SOURCE_ARCHIVE_MAX_BYTES + 1024;
        let err = broker
            .fetch_resource(request(handle, &runtime_id, None))
            .unwrap_err();
        assert!(matches!(err, BackendResourceError::Oversized { .. }));
    }
}
