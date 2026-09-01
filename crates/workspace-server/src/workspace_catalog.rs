use std::sync::Arc;

use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use workspace_api::{RepositoryObservedStatus, RepositorySource};

use crate::repository_source::{parse_repository_source, repository_source_fingerprint};
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceCreateResult {
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

    pub fn is_empty(&self) -> Result<bool> {
        Ok(self.store.list_workspaces()?.is_empty())
    }

    pub fn list(&self, owner_account_id: &str, limit: usize) -> Result<Vec<WorkspaceRecord>> {
        let limit = limit.clamp(1, 200);
        Ok(self
            .store
            .list_workspaces()?
            .into_iter()
            .filter(|workspace| workspace.owner_account_id == owner_account_id)
            .take(limit)
            .collect())
    }

    pub fn create(
        &self,
        request: WorkspaceCreateRequest,
        owner_account_id: String,
    ) -> Result<WorkspaceCreateResult> {
        self.create_internal(request, owner_account_id, None)
    }

    fn create_internal(
        &self,
        request: WorkspaceCreateRequest,
        owner_account_id: String,
        requested_workspace_id: Option<String>,
    ) -> Result<WorkspaceCreateResult> {
        let owner = self.store.get_account(&owner_account_id)?.ok_or_else(|| {
            Error::InvalidInput(
                "Workspace owner must reference an existing user account".to_string(),
            )
        })?;
        if owner.kind != "user" {
            return Err(Error::InvalidInput(
                "Workspace owner must be a user account".to_string(),
            ));
        }
        let operation_key = normalize_required(
            "operation_key",
            request.operation_key,
            MAX_OPERATION_KEY_BYTES,
        )?;
        let display_name =
            normalize_required("display_name", request.display_name, MAX_DISPLAY_NAME_BYTES)?;
        let repository_source = validate_repository_source(&request.repository.uri)?;
        let repository_uri = repository_source.uri.clone();
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
            Some(&owner_account_id),
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
                require_empty_catalog: false,
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
                    source: repository_source.clone(),
                    default_ref: Some(default_ref),
                    source_revision: 1,
                    source_fingerprint: repository_source_fingerprint(&repository_source),
                    observed_status: RepositoryObservedStatus::Unverified,
                    observed_at: None,
                    created_at: now.clone(),
                    updated_at: now,
                },
            })?;
        Ok(WorkspaceCreateResult {
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

fn validate_repository_source(uri: &str) -> Result<RepositorySource> {
    parse_repository_source(uri)
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
    use crate::store::{AccountRecord, SqliteWorkspaceStore};
    use workspace_api::RepositorySourceKind;

    fn git_repository() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        dir
    }

    fn owner_account(store: &SqliteWorkspaceStore) -> String {
        let account_id = Uuid::now_v7().to_string();
        let now = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
        store
            .upsert_account(&AccountRecord {
                account_id: account_id.clone(),
                kind: "user".to_string(),
                handle: format!("owner-{}", &account_id[..8]),
                display_name: "Workspace Owner".to_string(),
                created_at: now.clone(),
                updated_at: now,
            })
            .unwrap();
        account_id
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

        let owner_account_id = owner_account(store.as_ref());
        let created = service
            .create(request.clone(), owner_account_id.clone())
            .unwrap();
        let replayed = service.create(request, owner_account_id).unwrap();

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
        let owner_account_id = owner_account(store.as_ref());
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
        service
            .create(request.clone(), owner_account_id.clone())
            .unwrap();
        request.display_name = "Workspace B".to_string();

        let error = service
            .create(request, owner_account_id)
            .unwrap_err()
            .to_string();
        assert!(error.contains("different input"), "{error}");
    }

    #[test]
    fn repository_intent_accepts_unavailable_local_sources_without_server_io() {
        let remote = validate_repository_source("https://example.test/repo.git").unwrap();
        assert_eq!(remote.kind, RepositorySourceKind::Https);

        let local = validate_repository_source("/runtime-only/missing/repository").unwrap();
        assert_eq!(local.kind, RepositorySourceKind::LocalPath);
        assert_eq!(local.uri, "/runtime-only/missing/repository");

        let file = validate_repository_source("file:///runtime-only/missing/repository").unwrap();
        assert_eq!(file.kind, RepositorySourceKind::File);

        assert!(validate_repository_source("relative/repository").is_err());
    }

    #[test]
    fn create_rejects_non_user_account_owners() {
        let store = Arc::new(SqliteWorkspaceStore::in_memory().unwrap());
        store
            .upsert_account(&AccountRecord {
                account_id: "organization-owner".to_string(),
                kind: "organization".to_string(),
                handle: "organization-owner".to_string(),
                display_name: "Organization Owner".to_string(),
                created_at: "2026-07-03T00:00:00Z".to_string(),
                updated_at: "2026-07-03T00:00:00Z".to_string(),
            })
            .unwrap();
        let service = WorkspaceCatalogService::new(store);
        let repository = git_repository();
        let error = service
            .create(
                WorkspaceCreateRequest {
                    operation_key: "organization-owner-create".to_string(),
                    display_name: "Organization Workspace".to_string(),
                    repository: InitialRepositoryIntent {
                        uri: repository.path().display().to_string(),
                        display_name: None,
                        default_ref: None,
                    },
                },
                "organization-owner".to_string(),
            )
            .unwrap_err()
            .to_string();
        assert!(error.contains("must be a user account"), "{error}");
    }

    #[test]
    fn catalog_list_is_scoped_to_the_required_owner_account() {
        let store = Arc::new(SqliteWorkspaceStore::in_memory().unwrap());
        let owner_a = owner_account(store.as_ref());
        let owner_b = "account-owner-b".to_string();
        store
            .upsert_account(&AccountRecord {
                account_id: owner_b.clone(),
                kind: "user".to_string(),
                handle: "owner-b".to_string(),
                display_name: "Owner B".to_string(),
                created_at: "2026-07-03T00:00:00Z".to_string(),
                updated_at: "2026-07-03T00:00:00Z".to_string(),
            })
            .unwrap();
        let service = WorkspaceCatalogService::new(store);
        let repository_a = git_repository();
        let repository_b = git_repository();
        let created_a = service
            .create(
                WorkspaceCreateRequest {
                    operation_key: "owner-a-create".to_string(),
                    display_name: "Owner A Workspace".to_string(),
                    repository: InitialRepositoryIntent {
                        uri: repository_a.path().display().to_string(),
                        display_name: None,
                        default_ref: None,
                    },
                },
                owner_a.clone(),
            )
            .unwrap();
        service
            .create(
                WorkspaceCreateRequest {
                    operation_key: "owner-b-create".to_string(),
                    display_name: "Owner B Workspace".to_string(),
                    repository: InitialRepositoryIntent {
                        uri: repository_b.path().display().to_string(),
                        display_name: None,
                        default_ref: None,
                    },
                },
                owner_b.clone(),
            )
            .unwrap();

        let owner_a_workspaces = service.list(&owner_a, 100).unwrap();
        assert_eq!(owner_a_workspaces.len(), 1);
        assert_eq!(
            owner_a_workspaces[0].workspace_id,
            created_a.workspace.workspace_id
        );
        assert_eq!(owner_a_workspaces[0].owner_account_id, owner_a);
        assert!(
            service
                .list(&owner_b, 100)
                .unwrap()
                .into_iter()
                .all(|workspace| workspace.owner_account_id == owner_b)
        );
    }

    #[test]
    fn remote_repository_creation_persists_typed_source_without_auth_metadata() {
        let store = Arc::new(SqliteWorkspaceStore::in_memory().unwrap());
        let owner_account_id = owner_account(store.as_ref());
        let service = WorkspaceCatalogService::new(store.clone());
        let result = service
            .create(
                WorkspaceCreateRequest {
                    operation_key: "remote-create".to_string(),
                    display_name: "Remote Workspace".to_string(),
                    repository: InitialRepositoryIntent {
                        uri: "ssh://git@example.test/org/repository.git".to_string(),
                        display_name: Some("Remote Repository".to_string()),
                        default_ref: Some("main".to_string()),
                    },
                },
                owner_account_id,
            )
            .unwrap();

        let persisted = store
            .get_repository(
                &result.workspace.workspace_id,
                &result.repository.repository_id,
            )
            .unwrap()
            .unwrap();
        assert_eq!(persisted.source.kind, RepositorySourceKind::Ssh);
        assert_eq!(persisted.source_revision, 1);
        assert!(persisted.source_fingerprint.starts_with("sha256:"));
        assert_eq!(
            persisted.observed_status,
            RepositoryObservedStatus::Unverified
        );
        let json = serde_json::to_value(&persisted).unwrap();
        assert!(json.get("source").is_some());
        assert!(json.get("auth_ref_kind").is_none());
        assert!(json.get("auth_ref_key").is_none());
    }
}
