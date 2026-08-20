use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::store::{
    ControlPlaneStore, RepositoryRecord, WorkspaceBootstrapRecord, WorkspaceRecord,
};
use crate::{Error, Result};

const DEFAULT_REPOSITORY_ID: &str = "main";
const MAX_DISPLAY_NAME_BYTES: usize = 200;
const MAX_OPERATION_KEY_BYTES: usize = 200;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InitialRepositoryIntent {
    pub uri: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub default_ref: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceCreateRequest {
    pub operation_key: String,
    pub display_name: String,
    pub repository: InitialRepositoryIntent,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceCreateResponse {
    pub workspace: WorkspaceRecord,
    pub repository: RepositoryRecord,
    pub config_revision: u64,
    pub request_fingerprint: String,
    pub replayed: bool,
}

#[derive(Clone)]
pub struct WorkspaceCatalogService {
    store: Arc<dyn ControlPlaneStore>,
}

impl WorkspaceCatalogService {
    pub fn new(store: Arc<dyn ControlPlaneStore>) -> Self {
        Self { store }
    }

    pub fn list(
        &self,
        owner_account_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<WorkspaceRecord>> {
        let limit = limit.clamp(1, 200);
        Ok(self
            .store
            .list_workspaces()?
            .into_iter()
            .filter(|workspace| {
                workspace.owner_account_id.is_none()
                    || owner_account_id
                        .is_some_and(|owner| workspace.owner_account_id.as_deref() == Some(owner))
            })
            .take(limit)
            .collect())
    }

    pub fn create(
        &self,
        request: WorkspaceCreateRequest,
        owner_account_id: Option<String>,
    ) -> Result<WorkspaceCreateResponse> {
        self.create_with_workspace_id(request, owner_account_id, None)
    }

    pub fn create_with_workspace_id(
        &self,
        request: WorkspaceCreateRequest,
        owner_account_id: Option<String>,
        requested_workspace_id: Option<String>,
    ) -> Result<WorkspaceCreateResponse> {
        let operation_key = normalize_required(
            "operation_key",
            request.operation_key,
            MAX_OPERATION_KEY_BYTES,
        )?;
        let display_name =
            normalize_required("display_name", request.display_name, MAX_DISPLAY_NAME_BYTES)?;
        let repository_path = validate_repository_uri(&request.repository.uri)?;
        let repository_uri = repository_path.to_string_lossy().into_owned();
        let repository_name = request
            .repository
            .display_name
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("Main repository")
            .to_string();
        let default_ref = request
            .repository
            .default_ref
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("HEAD")
            .to_string();
        let requested_workspace_id = requested_workspace_id
            .map(|value| {
                Uuid::parse_str(value.trim())
                    .map(|id| id.to_string())
                    .map_err(|_| Error::InvalidInput("workspace_id must be a UUID".to_string()))
            })
            .transpose()?;
        let workspace_id = requested_workspace_id
            .clone()
            .unwrap_or_else(|| Uuid::now_v7().to_string());
        let fingerprint = workspace_create_fingerprint(
            requested_workspace_id.as_deref(),
            &display_name,
            owner_account_id.as_deref(),
            &repository_uri,
            &repository_name,
            &default_ref,
        );
        let now = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
        let result = self
            .store
            .create_workspace_bootstrap(&WorkspaceBootstrapRecord {
                operation_key,
                request_fingerprint: fingerprint.clone(),
                workspace: WorkspaceRecord {
                    workspace_id: workspace_id.clone(),
                    owner_account_id,
                    display_name,
                    state: "active".to_string(),
                    created_at: now.clone(),
                    updated_at: now.clone(),
                },
                repository: RepositoryRecord {
                    workspace_id,
                    repository_id: DEFAULT_REPOSITORY_ID.to_string(),
                    name: repository_name,
                    kind: "git".to_string(),
                    provider: Some("git".to_string()),
                    uri: repository_uri,
                    default_ref: Some(default_ref),
                    auth_ref_kind: None,
                    auth_ref_key: None,
                    created_at: now.clone(),
                    updated_at: now,
                },
            })?;
        Ok(WorkspaceCreateResponse {
            workspace: result.workspace,
            repository: result.repository,
            config_revision: result.config_revision,
            request_fingerprint: fingerprint,
            replayed: result.replayed,
        })
    }
}

fn normalize_required(field: &str, value: String, max_bytes: usize) -> Result<String> {
    let value = value.trim();
    if value.is_empty() || value.len() > max_bytes {
        return Err(Error::InvalidInput(format!(
            "{field} must be between 1 and {max_bytes} bytes"
        )));
    }
    Ok(value.to_string())
}

fn validate_repository_uri(uri: &str) -> Result<PathBuf> {
    let uri = uri.trim();
    if uri.is_empty() || uri.contains("://") {
        return Err(Error::InvalidInput(
            "initial repository uri must be an absolute server-local path".to_string(),
        ));
    }
    let path = Path::new(uri);
    if !path.is_absolute() {
        return Err(Error::InvalidInput(
            "initial repository uri must be an absolute server-local path".to_string(),
        ));
    }
    let path = path.canonicalize().map_err(|error| {
        Error::InvalidInput(format!("initial repository path is unavailable: {error}"))
    })?;
    if !path.is_dir() {
        return Err(Error::InvalidInput(
            "initial repository path must be a directory".to_string(),
        ));
    }
    let normal_git = path.join(".git").exists();
    let bare_git = path.join("HEAD").is_file() && path.join("objects").is_dir();
    if !normal_git && !bare_git {
        return Err(Error::InvalidInput(
            "initial repository path is not a Git repository".to_string(),
        ));
    }
    Ok(path)
}

fn workspace_create_fingerprint(
    requested_workspace_id: Option<&str>,
    display_name: &str,
    owner_account_id: Option<&str>,
    repository_uri: &str,
    repository_name: &str,
    default_ref: &str,
) -> String {
    let payload = serde_json::json!({
        "requested_workspace_id": requested_workspace_id,
        "display_name": display_name,
        "owner_account_id": owner_account_id,
        "repository": {
            "repository_id": DEFAULT_REPOSITORY_ID,
            "uri": repository_uri,
            "display_name": repository_name,
            "default_ref": default_ref,
            "kind": "git",
        }
    });
    let mut hasher = Sha256::new();
    hasher.update(serde_json::to_vec(&payload).expect("workspace fingerprint serializes"));
    let digest = hasher.finalize();
    let encoded = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("sha256:{encoded}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::SqliteWorkspaceStore;

    fn git_repository() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        dir
    }

    #[tokio::test]
    async fn create_is_atomic_and_exact_retries_converge() {
        let store = Arc::new(SqliteWorkspaceStore::in_memory().unwrap());
        let service = WorkspaceCatalogService::new(store.clone());
        let repository = git_repository();
        let request = WorkspaceCreateRequest {
            operation_key: "request-1".to_string(),
            display_name: "Workspace A".to_string(),
            repository: InitialRepositoryIntent {
                uri: repository.path().display().to_string(),
                display_name: None,
                default_ref: None,
            },
        };

        let created = service.create(request.clone(), None).unwrap();
        let replayed = service.create(request, None).unwrap();

        assert!(!created.replayed);
        assert!(replayed.replayed);
        assert_eq!(
            created.workspace.workspace_id,
            replayed.workspace.workspace_id
        );
        assert_eq!(store.list_workspaces().unwrap().len(), 1);
        assert_eq!(
            store
                .list_repositories(&created.workspace.workspace_id)
                .unwrap()
                .len(),
            1
        );
        assert!(
            store
                .load_workspace_config(&created.workspace.workspace_id)
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn idempotency_key_reuse_with_different_payload_is_rejected() {
        let store = Arc::new(SqliteWorkspaceStore::in_memory().unwrap());
        let service = WorkspaceCatalogService::new(store);
        let repository = git_repository();
        let mut request = WorkspaceCreateRequest {
            operation_key: "request-1".to_string(),
            display_name: "Workspace A".to_string(),
            repository: InitialRepositoryIntent {
                uri: repository.path().display().to_string(),
                display_name: None,
                default_ref: None,
            },
        };
        service.create(request.clone(), None).unwrap();
        request.display_name = "Workspace B".to_string();

        let error = service.create(request, None).unwrap_err().to_string();
        assert!(error.contains("different input"), "{error}");
    }

    #[test]
    fn repository_intent_rejects_remote_and_non_git_paths() {
        let remote = validate_repository_uri("https://example.test/repo.git").unwrap_err();
        assert!(remote.to_string().contains("server-local path"));

        let dir = tempfile::tempdir().unwrap();
        let non_git = validate_repository_uri(&dir.path().display().to_string()).unwrap_err();
        assert!(non_git.to_string().contains("not a Git repository"));
    }
}
