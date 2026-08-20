use serde::{Deserialize, Serialize};
use std::fmt;

const DEFAULT_WORKSPACE_LIMIT: usize = 200;

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct BackendWorkspace {
    pub workspace_id: String,
    pub owner_account_id: Option<String>,
    pub display_name: String,
    pub state: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CreateBackendWorkspaceRequest {
    pub operation_key: String,
    pub display_name: String,
    pub repository: CreateBackendWorkspaceRepository,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CreateBackendWorkspaceRepository {
    pub uri: String,
    pub display_name: Option<String>,
    pub default_ref: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct CreateBackendWorkspaceResponse {
    pub workspace: BackendWorkspace,
    pub repository: CreateBackendWorkspaceRepositoryRecord,
    pub config_revision: u64,
    pub request_fingerprint: String,
    pub replayed: bool,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct CreateBackendWorkspaceRepositoryRecord {
    pub workspace_id: String,
    pub repository_id: String,
    pub name: String,
    pub kind: String,
    pub uri: String,
    pub default_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendWorkspaceCatalogTarget {
    pub base_url: String,
}

impl BackendWorkspaceCatalogTarget {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
        }
    }
}

#[derive(Debug)]
pub enum BackendWorkspaceClientError {
    InvalidTarget(String),
    RequestFailed { status: u16, message: String },
    Http(reqwest::Error),
}

impl fmt::Display for BackendWorkspaceClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidTarget(message) => f.write_str(message),
            Self::RequestFailed { status, message } => {
                write!(f, "Backend request failed with HTTP {status}: {message}")
            }
            Self::Http(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for BackendWorkspaceClientError {}

impl From<reqwest::Error> for BackendWorkspaceClientError {
    fn from(error: reqwest::Error) -> Self {
        Self::Http(error)
    }
}

pub async fn list_backend_workspaces(
    target: &BackendWorkspaceCatalogTarget,
) -> Result<Vec<BackendWorkspace>, BackendWorkspaceClientError> {
    validate_target(target)?;
    let url = format!(
        "{}/api/workspaces?limit={DEFAULT_WORKSPACE_LIMIT}",
        target.base_url.trim_end_matches('/')
    );
    let response = reqwest::Client::new().get(url).send().await?;
    let response = require_success(response).await?;
    Ok(response.json::<Vec<BackendWorkspace>>().await?)
}

pub async fn create_backend_workspace(
    target: &BackendWorkspaceCatalogTarget,
    request: &CreateBackendWorkspaceRequest,
) -> Result<CreateBackendWorkspaceResponse, BackendWorkspaceClientError> {
    validate_target(target)?;
    let url = format!("{}/api/workspaces", target.base_url.trim_end_matches('/'));
    let response = reqwest::Client::new()
        .post(url)
        .json(request)
        .send()
        .await?;
    let response = require_success(response).await?;
    Ok(response.json::<CreateBackendWorkspaceResponse>().await?)
}

async fn require_success(
    response: reqwest::Response,
) -> Result<reqwest::Response, BackendWorkspaceClientError> {
    if response.status().is_success() {
        return Ok(response);
    }
    let status = response.status().as_u16();
    let message = response.text().await.unwrap_or_default();
    Err(BackendWorkspaceClientError::RequestFailed { status, message })
}

fn validate_target(
    target: &BackendWorkspaceCatalogTarget,
) -> Result<(), BackendWorkspaceClientError> {
    if !(target.base_url.starts_with("http://") || target.base_url.starts_with("https://")) {
        return Err(BackendWorkspaceClientError::InvalidTarget(
            "Backend API base URL must start with http:// or https://".to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_request_keeps_operation_key_for_exact_retry() {
        let request = CreateBackendWorkspaceRequest {
            operation_key: "workspace-create-1".to_string(),
            display_name: "Alpha".to_string(),
            repository: CreateBackendWorkspaceRepository {
                uri: "/srv/repos/alpha".to_string(),
                display_name: Some("Main".to_string()),
                default_ref: Some("develop".to_string()),
            },
        };

        let retry = request.clone();
        assert_eq!(retry.operation_key, "workspace-create-1");
        assert_eq!(retry, request);
    }
}
