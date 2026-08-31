use crate::{BackendApiClient, BackendApiClientError};
use reqwest::Method;
use serde::{Deserialize, Serialize};
use std::fmt;
use workspace_api::{WorkspaceCreateResponse, WorkspaceRepositoryRecord, WorkspaceSummary};

const DEFAULT_WORKSPACE_LIMIT: usize = 200;

pub type BackendWorkspace = WorkspaceSummary;
pub type CreateBackendWorkspaceResponse = WorkspaceCreateResponse;
pub type CreateBackendWorkspaceRepositoryRecord = WorkspaceRepositoryRecord;

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
    Api(BackendApiClientError),
    Http(reqwest::Error),
}

impl fmt::Display for BackendWorkspaceClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidTarget(message) => f.write_str(message),
            Self::Api(error) => write!(f, "{error}"),
            Self::Http(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for BackendWorkspaceClientError {}

impl From<BackendApiClientError> for BackendWorkspaceClientError {
    fn from(error: BackendApiClientError) -> Self {
        Self::Api(error)
    }
}

impl From<reqwest::Error> for BackendWorkspaceClientError {
    fn from(error: reqwest::Error) -> Self {
        Self::Http(error)
    }
}

pub async fn list_backend_workspaces(
    target: &BackendWorkspaceCatalogTarget,
) -> Result<Vec<BackendWorkspace>, BackendWorkspaceClientError> {
    let client = BackendApiClient::from_stored_token(&target.base_url)?;
    list_backend_workspaces_with_client(&client).await
}

async fn list_backend_workspaces_with_client(
    client: &BackendApiClient,
) -> Result<Vec<BackendWorkspace>, BackendWorkspaceClientError> {
    let response = client
        .request(
            Method::GET,
            &format!("/api/workspaces?limit={DEFAULT_WORKSPACE_LIMIT}"),
        )?
        .send()
        .await?;
    client.check_status(response.status())?;
    Ok(response.json::<Vec<BackendWorkspace>>().await?)
}

pub async fn create_backend_workspace(
    target: &BackendWorkspaceCatalogTarget,
    request: &CreateBackendWorkspaceRequest,
) -> Result<CreateBackendWorkspaceResponse, BackendWorkspaceClientError> {
    let client = BackendApiClient::from_stored_token(&target.base_url)?;
    let response = client
        .request(Method::POST, "/api/workspaces")?
        .json(request)
        .send()
        .await?;
    client.check_status(response.status())?;
    Ok(response.json::<CreateBackendWorkspaceResponse>().await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    #[tokio::test]
    async fn workspace_catalog_request_uses_shared_bearer_client() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = vec![0; 4096];
            let read = stream.read(&mut request).unwrap();
            let request = String::from_utf8_lossy(&request[..read]).to_ascii_lowercase();
            assert!(request.starts_with("get /api/workspaces?limit=200 "));
            assert!(request.contains("authorization: bearer catalog-secret\r\n"));
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n[]",
                )
                .unwrap();
        });
        let client =
            BackendApiClient::from_access_token_for_test(&base_url, "catalog-secret").unwrap();
        assert!(
            list_backend_workspaces_with_client(&client)
                .await
                .unwrap()
                .is_empty()
        );
        handle.join().unwrap();
    }

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
