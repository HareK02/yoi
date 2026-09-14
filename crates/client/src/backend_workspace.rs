use crate::{BackendApiClient, BackendApiClientError};
use reqwest::Method;
use std::fmt;
use workspace_api::{
    InitialRepositoryIntent, RepositoryListResponse, RepositorySummary,
    WorkspaceCatalogListResponse, WorkspaceCreateRequest, WorkspaceCreateResponse,
    WorkspaceSummary,
};

const DEFAULT_WORKSPACE_LIMIT: usize = 200;

pub type BackendWorkspace = WorkspaceSummary;
pub type CreateBackendWorkspaceResponse = WorkspaceCreateResponse;
pub type CreateBackendWorkspaceRequest = WorkspaceCreateRequest;
pub type CreateBackendWorkspaceRepository = InitialRepositoryIntent;

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

pub fn list_backend_workspaces_blocking(
    target: &BackendWorkspaceCatalogTarget,
) -> Result<Vec<BackendWorkspace>, BackendWorkspaceClientError> {
    let client = BackendApiClient::from_stored_token(&target.base_url)?;
    let response = client
        .blocking_request(
            Method::GET,
            &format!("/api/workspaces?limit={DEFAULT_WORKSPACE_LIMIT}"),
        )?
        .send()?;
    client.check_status(response.status())?;
    Ok(response.json::<WorkspaceCatalogListResponse>()?.0)
}

pub fn list_backend_workspace_repositories_blocking(
    target: &BackendWorkspaceCatalogTarget,
    workspace_id: &str,
) -> Result<Vec<RepositorySummary>, BackendWorkspaceClientError> {
    if workspace_id.is_empty()
        || workspace_id.len() > 200
        || !workspace_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err(BackendWorkspaceClientError::InvalidTarget(
            "Workspace id returned by Backend is invalid".to_string(),
        ));
    }
    let client = BackendApiClient::from_stored_token(&target.base_url)?;
    let response = client
        .blocking_request(Method::GET, &format!("/api/w/{workspace_id}/repositories"))?
        .send()?;
    client.check_status(response.status())?;
    let response = response.json::<RepositoryListResponse>()?;
    if response.workspace_id != workspace_id {
        return Err(BackendWorkspaceClientError::InvalidTarget(
            "Repository catalog response does not match the requested Workspace".to_string(),
        ));
    }
    Ok(response.items)
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
    Ok(response.json::<WorkspaceCatalogListResponse>().await?.0)
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
                repository_key: "main".to_string(),
                uri: "/srv/repos/alpha".to_string(),
                default_ref: Some("develop".to_string()),
            },
        };

        let retry = request.clone();
        assert_eq!(retry.operation_key, "workspace-create-1");
        assert_eq!(retry, request);
    }
}
