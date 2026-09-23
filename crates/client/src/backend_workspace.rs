use crate::{BackendApiClient, BackendApiClientError};
use server_api::{
    InitialRepositoryIntent, RepositorySummary, WorkspaceCreateRequest, WorkspaceCreateResponse,
    WorkspaceListQuery, WorkspaceSummary,
};
use std::fmt;

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
    ServerApi(server_api::client_support::ClientError<server_api::RepositoryApiError>),
    RuntimeManagementApi(
        server_api::client_support::ClientError<server_api::RuntimeManagementApiError>,
    ),
}

impl fmt::Display for BackendWorkspaceClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidTarget(message) => f.write_str(message),
            Self::Api(error) => write!(f, "{error}"),
            Self::Http(error) => write!(f, "{error}"),
            Self::ServerApi(error) => write!(f, "{error}"),
            Self::RuntimeManagementApi(error) => write!(f, "{error}"),
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

impl From<server_api::client_support::ClientError<server_api::RepositoryApiError>>
    for BackendWorkspaceClientError
{
    fn from(
        error: server_api::client_support::ClientError<server_api::RepositoryApiError>,
    ) -> Self {
        Self::ServerApi(error)
    }
}

pub(crate) fn server_client_error(
    backend: &BackendApiClient,
    error: server_api::client_support::ClientError<server_api::RepositoryApiError>,
) -> BackendWorkspaceClientError {
    let status = match &error {
        server_api::client_support::ClientError::Public { status, .. } => Some(*status),
        server_api::client_support::ClientError::Failure(_) => None,
    };
    match status {
        Some(reqwest::StatusCode::UNAUTHORIZED) => {
            BackendWorkspaceClientError::Api(BackendApiClientError::Unauthorized {
                origin: backend.origin().clone(),
            })
        }
        Some(reqwest::StatusCode::FORBIDDEN) => {
            BackendWorkspaceClientError::Api(BackendApiClientError::Forbidden {
                origin: backend.origin().clone(),
            })
        }
        _ => BackendWorkspaceClientError::ServerApi(error),
    }
}

pub(crate) fn runtime_management_client_error(
    backend: &BackendApiClient,
    error: server_api::client_support::ClientError<server_api::RuntimeManagementApiError>,
) -> BackendWorkspaceClientError {
    let status = match &error {
        server_api::client_support::ClientError::Public { status, .. } => Some(*status),
        server_api::client_support::ClientError::Failure(_) => None,
    };
    match status {
        Some(reqwest::StatusCode::UNAUTHORIZED) => {
            BackendWorkspaceClientError::Api(BackendApiClientError::Unauthorized {
                origin: backend.origin().clone(),
            })
        }
        Some(reqwest::StatusCode::FORBIDDEN) => {
            BackendWorkspaceClientError::Api(BackendApiClientError::Forbidden {
                origin: backend.origin().clone(),
            })
        }
        _ => BackendWorkspaceClientError::RuntimeManagementApi(error),
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ServerBearerAuthorizer {
    authorization: String,
}

impl server_api::client_support::RequestAuthorizer for ServerBearerAuthorizer {
    fn authorize(
        &self,
        _request: server_api::client_support::AuthorizerRequest<'_>,
    ) -> Result<reqwest::header::HeaderMap, server_api::client_support::AuthorizationError> {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::AUTHORIZATION,
            self.authorization
                .parse()
                .map_err(|_| server_api::client_support::AuthorizationError::new())?,
        );
        Ok(headers)
    }
}

const SERVER_RESPONSE_LIMIT: usize = 16 * 1024 * 1024;

pub(crate) fn server_api_client(
    backend: &BackendApiClient,
) -> Result<server_api::ServerApiClient<ServerBearerAuthorizer>, BackendWorkspaceClientError> {
    server_api::ServerApiClient::builder(backend.origin().as_str())
        .map_err(|error| BackendWorkspaceClientError::InvalidTarget(error.to_string()))?
        .client(backend.asynchronous_client())
        .authorizer(ServerBearerAuthorizer {
            authorization: backend.authorization_header_value(),
        })
        .response_body_limit(SERVER_RESPONSE_LIMIT)
        .build()
        .map_err(|error| BackendWorkspaceClientError::InvalidTarget(error.to_string()))
}

pub(crate) fn memory_document_blocking(
    backend: &BackendApiClient,
    workspace_id: &str,
) -> Result<server_api::MemoryDocumentResponse, BackendWorkspaceClientError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| BackendWorkspaceClientError::InvalidTarget(error.to_string()))?;
    runtime.block_on(async {
        server_api_client(backend)?
            .memory_document(workspace_id.to_owned())
            .await
            .map_err(|error| server_client_error(backend, error))
    })
}

pub(crate) fn memory_staging_list_blocking(
    backend: &BackendApiClient,
    workspace_id: &str,
    limit: usize,
) -> Result<server_api::MemoryStagingListResponse, BackendWorkspaceClientError> {
    let limit = u32::try_from(limit).map_err(|_| {
        BackendWorkspaceClientError::InvalidTarget("Memory staging limit exceeds u32".to_string())
    })?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| BackendWorkspaceClientError::InvalidTarget(error.to_string()))?;
    runtime.block_on(async {
        server_api_client(backend)?
            .memory_staging_list(
                workspace_id.to_owned(),
                server_api::MemoryStagingQuery { limit: Some(limit) },
            )
            .await
            .map_err(|error| server_client_error(backend, error))
    })
}

pub fn list_backend_workspaces_blocking(
    target: &BackendWorkspaceCatalogTarget,
) -> Result<Vec<BackendWorkspace>, BackendWorkspaceClientError> {
    let client = BackendApiClient::from_stored_token(&target.base_url)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| BackendWorkspaceClientError::InvalidTarget(error.to_string()))?;
    runtime.block_on(list_backend_workspaces_with_client(&client))
}

pub fn list_backend_workspace_repositories_blocking(
    target: &BackendWorkspaceCatalogTarget,
    workspace_id: &str,
) -> Result<Vec<RepositorySummary>, BackendWorkspaceClientError> {
    let client = BackendApiClient::from_stored_token(&target.base_url)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| BackendWorkspaceClientError::InvalidTarget(error.to_string()))?;
    runtime.block_on(list_backend_workspace_repositories_with_client(
        &client,
        workspace_id,
    ))
}

async fn list_backend_workspace_repositories_with_client(
    backend: &BackendApiClient,
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
    let response = server_api_client(backend)?
        .repository_list(workspace_id.to_owned())
        .await
        .map_err(|error| server_client_error(backend, error))?;
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
    let response = server_api_client(client)?
        .workspace_catalog_list(WorkspaceListQuery {
            limit: Some(DEFAULT_WORKSPACE_LIMIT as u32),
        })
        .await
        .map_err(|error| server_client_error(client, error))?;
    Ok(response.0)
}

pub async fn create_backend_workspace(
    target: &BackendWorkspaceCatalogTarget,
    request: &CreateBackendWorkspaceRequest,
) -> Result<CreateBackendWorkspaceResponse, BackendWorkspaceClientError> {
    let client = BackendApiClient::from_stored_token(&target.base_url)?;
    server_api_client(&client)?
        .workspace_catalog_create(request.clone())
        .await
        .map_err(|error| server_client_error(&client, error))
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

    #[tokio::test]
    async fn repository_catalog_uses_generated_path_and_bearer_authorizer() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = vec![0; 4096];
            let read = stream.read(&mut request).unwrap();
            let request = String::from_utf8_lossy(&request[..read]).to_ascii_lowercase();
            assert!(request.starts_with("get /api/w/workspace-test/repositories "));
            assert!(request.contains("authorization: bearer repository-secret\r\n"));
            let body = r#"{"workspace_id":"workspace-test","items":[],"source":"workspace","diagnostics":[]}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
        });
        let client =
            BackendApiClient::from_access_token_for_test(&base_url, "repository-secret").unwrap();

        assert!(
            list_backend_workspace_repositories_with_client(&client, "workspace-test")
                .await
                .unwrap()
                .is_empty()
        );
        handle.join().unwrap();
    }

    #[tokio::test]
    async fn repository_catalog_enforces_generated_response_limit() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = vec![0; 4096];
            let _ = stream.read(&mut request).unwrap();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                SERVER_RESPONSE_LIMIT + 1,
            )
            .unwrap();
            stream.flush().unwrap();
            let chunk = [b' '; 8192];
            let mut remaining = SERVER_RESPONSE_LIMIT + 1;
            while remaining > 0 {
                let length = remaining.min(chunk.len());
                if stream.write_all(&chunk[..length]).is_err() {
                    break;
                }
                remaining -= length;
            }
        });
        let client =
            BackendApiClient::from_access_token_for_test(&base_url, "repository-secret").unwrap();

        let error = list_backend_workspace_repositories_with_client(&client, "workspace-test")
            .await
            .unwrap_err();
        assert!(
            matches!(
                &error,
                BackendWorkspaceClientError::ServerApi(
                    server_api::client_support::ClientError::Failure(
                        server_api::client_support::ClientFailure::ResponseTooLarge {
                            limit: SERVER_RESPONSE_LIMIT
                        }
                    )
                )
            ),
            "unexpected error: {error:?}"
        );
        handle.join().unwrap();
    }

    #[test]
    fn server_client_preserves_authentication_error_taxonomy() {
        let client =
            BackendApiClient::from_access_token_for_test("https://backend.example.test", "token")
                .unwrap();
        let unauthorized = server_client_error(
            &client,
            server_api::client_support::ClientError::public(
                reqwest::StatusCode::UNAUTHORIZED,
                server_api::RepositoryApiError::new(
                    401,
                    "Unauthorized",
                    "authentication required",
                    Vec::new(),
                ),
            ),
        );
        assert!(matches!(
            unauthorized,
            BackendWorkspaceClientError::Api(BackendApiClientError::Unauthorized { .. })
        ));
        let forbidden = server_client_error(
            &client,
            server_api::client_support::ClientError::public(
                reqwest::StatusCode::FORBIDDEN,
                server_api::RepositoryApiError::new(
                    403,
                    "Forbidden",
                    "repository access denied",
                    Vec::new(),
                ),
            ),
        );
        assert!(matches!(
            forbidden,
            BackendWorkspaceClientError::Api(BackendApiClientError::Forbidden { .. })
        ));
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
