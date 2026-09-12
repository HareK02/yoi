use std::error::Error as _;
use std::io::Read;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures::StreamExt;
use reqwest::header::HeaderMap;
use thiserror::Error;

use crate::auth::{
    RUNTIME_REQUEST_SOURCE_PROOF_HEADER, RuntimeAuthError, RuntimeIdentityMaterial,
    RuntimeRequestSourceSigner,
};

const DEFAULT_REQUEST_PROOF_TTL_SECONDS: u64 = 60;
const RUNTIME_ID_HEADER: &str = "x-yoi-runtime-id";
const WORKER_ID_HEADER: &str = "x-yoi-worker-id";

#[derive(Clone, Debug)]
pub struct RuntimeWorkspaceRequestClient {
    workspace_id: String,
    base_url: String,
    runtime_id: String,
    request_source: Option<(RuntimeRequestSourceSigner, String)>,
}

#[derive(Clone, Debug)]
pub(crate) struct RuntimeWorkspaceRequest {
    pub method: reqwest::Method,
    pub path_and_query: String,
    pub body: Vec<u8>,
    pub headers: HeaderMap,
    pub permission: String,
    pub worker_id: Option<String>,
    pub timeout: Option<Duration>,
    pub max_response_bytes: usize,
}

#[derive(Debug)]
pub(crate) struct RuntimeWorkspaceResponse {
    pub status: reqwest::StatusCode,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

#[derive(Debug, Error)]
pub(crate) enum RuntimeWorkspaceRequestError {
    #[error("invalid Workspace request: {0}")]
    InvalidRequest(String),
    #[error("failed to sign Workspace request: {0}")]
    Sign(#[from] RuntimeAuthError),
    #[error("Workspace request failed: {message}")]
    Transport { message: String, timeout: bool },
    #[error("Workspace response exceeded {max_response_bytes} bytes")]
    ResponseTooLarge { max_response_bytes: usize },
}

impl RuntimeWorkspaceRequestError {
    fn transport(error: reqwest::Error) -> Self {
        let timeout = error.is_timeout();
        Self::Transport {
            message: reqwest_error_chain(&error),
            timeout,
        }
    }

    pub(crate) fn is_timeout(&self) -> bool {
        matches!(self, Self::Transport { timeout: true, .. })
    }
}

impl RuntimeWorkspaceRequestClient {
    pub fn new(
        workspace_id: impl Into<String>,
        base_url: impl Into<String>,
        runtime_id: impl Into<String>,
    ) -> Self {
        Self {
            workspace_id: workspace_id.into(),
            base_url: base_url.into().trim_end_matches('/').to_string(),
            runtime_id: runtime_id.into(),
            request_source: None,
        }
    }

    pub fn with_runtime_request_source(
        mut self,
        identity: &RuntimeIdentityMaterial,
        audience: impl Into<String>,
    ) -> Self {
        self.request_source = Some((
            RuntimeRequestSourceSigner::from_identity(identity),
            audience.into(),
        ));
        self
    }

    pub fn workspace_id(&self) -> &str {
        &self.workspace_id
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn runtime_id(&self) -> &str {
        &self.runtime_id
    }

    pub fn audience(&self) -> Option<&str> {
        self.request_source
            .as_ref()
            .map(|(_, audience)| audience.as_str())
    }

    pub fn matches_workspace(&self, workspace_id: &str, base_url: &str) -> bool {
        self.workspace_id == workspace_id
            && self.base_url.trim_end_matches('/') == base_url.trim_end_matches('/')
    }

    pub(crate) async fn execute(
        &self,
        request: RuntimeWorkspaceRequest,
    ) -> Result<RuntimeWorkspaceResponse, RuntimeWorkspaceRequestError> {
        let prepared = self.prepare(&request)?;
        let mut client_builder = reqwest::Client::builder();
        if let Some(timeout) = request.timeout {
            client_builder = client_builder.timeout(timeout);
        }
        let client = client_builder
            .build()
            .map_err(RuntimeWorkspaceRequestError::transport)?;
        let mut builder = client
            .request(request.method, prepared.url)
            .headers(request.headers)
            .header(RUNTIME_ID_HEADER, &self.runtime_id);
        if let Some(worker_id) = request.worker_id.as_deref() {
            builder = builder.header(WORKER_ID_HEADER, worker_id);
        }
        if let Some(proof) = prepared.proof {
            builder = builder.header(RUNTIME_REQUEST_SOURCE_PROOF_HEADER, proof);
        }
        if !request.body.is_empty() {
            builder = builder.body(request.body);
        }
        let response = builder
            .send()
            .await
            .map_err(RuntimeWorkspaceRequestError::transport)?;
        let status = response.status();
        let headers = response.headers().clone();
        if response
            .content_length()
            .is_some_and(|size| size > request.max_response_bytes as u64)
        {
            return Err(RuntimeWorkspaceRequestError::ResponseTooLarge {
                max_response_bytes: request.max_response_bytes,
            });
        }
        let mut body = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(RuntimeWorkspaceRequestError::transport)?;
            if body.len().saturating_add(chunk.len()) > request.max_response_bytes {
                return Err(RuntimeWorkspaceRequestError::ResponseTooLarge {
                    max_response_bytes: request.max_response_bytes,
                });
            }
            body.extend_from_slice(&chunk);
        }
        Ok(RuntimeWorkspaceResponse {
            status,
            headers,
            body,
        })
    }

    pub(crate) fn execute_blocking(
        &self,
        request: RuntimeWorkspaceRequest,
    ) -> Result<RuntimeWorkspaceResponse, RuntimeWorkspaceRequestError> {
        let client = self.clone();
        std::thread::spawn(move || client.execute_blocking_inner(request))
            .join()
            .map_err(|_| RuntimeWorkspaceRequestError::Transport {
                message: "Workspace request thread panicked".to_string(),
                timeout: false,
            })?
    }

    fn execute_blocking_inner(
        &self,
        request: RuntimeWorkspaceRequest,
    ) -> Result<RuntimeWorkspaceResponse, RuntimeWorkspaceRequestError> {
        let prepared = self.prepare(&request)?;
        let mut client_builder = reqwest::blocking::Client::builder();
        if let Some(timeout) = request.timeout {
            client_builder = client_builder.timeout(timeout);
        }
        let client = client_builder
            .build()
            .map_err(RuntimeWorkspaceRequestError::transport)?;
        let mut builder = client
            .request(request.method, prepared.url)
            .headers(request.headers)
            .header(RUNTIME_ID_HEADER, &self.runtime_id);
        if let Some(worker_id) = request.worker_id.as_deref() {
            builder = builder.header(WORKER_ID_HEADER, worker_id);
        }
        if let Some(proof) = prepared.proof {
            builder = builder.header(RUNTIME_REQUEST_SOURCE_PROOF_HEADER, proof);
        }
        if !request.body.is_empty() {
            builder = builder.body(request.body);
        }
        let response = builder
            .send()
            .map_err(RuntimeWorkspaceRequestError::transport)?;
        let status = response.status();
        let headers = response.headers().clone();
        if response
            .content_length()
            .is_some_and(|size| size > request.max_response_bytes as u64)
        {
            return Err(RuntimeWorkspaceRequestError::ResponseTooLarge {
                max_response_bytes: request.max_response_bytes,
            });
        }
        let limit = u64::try_from(request.max_response_bytes)
            .unwrap_or(u64::MAX)
            .saturating_add(1);
        let mut body = Vec::new();
        response
            .take(limit)
            .read_to_end(&mut body)
            .map_err(|error| RuntimeWorkspaceRequestError::Transport {
                message: error.to_string(),
                timeout: false,
            })?;
        if body.len() > request.max_response_bytes {
            return Err(RuntimeWorkspaceRequestError::ResponseTooLarge {
                max_response_bytes: request.max_response_bytes,
            });
        }
        Ok(RuntimeWorkspaceResponse {
            status,
            headers,
            body,
        })
    }

    fn prepare(
        &self,
        request: &RuntimeWorkspaceRequest,
    ) -> Result<PreparedRuntimeWorkspaceRequest, RuntimeWorkspaceRequestError> {
        if !request.path_and_query.starts_with('/') || request.path_and_query.starts_with("//") {
            return Err(RuntimeWorkspaceRequestError::InvalidRequest(
                "path must start with '/'".to_string(),
            ));
        }
        let url = reqwest::Url::parse(&format!("{}{}", self.base_url, request.path_and_query))
            .map_err(|error| RuntimeWorkspaceRequestError::InvalidRequest(error.to_string()))?;
        let mut request_target = url.path().to_string();
        if let Some(query) = url.query() {
            request_target.push('?');
            request_target.push_str(query);
        }
        let proof = self
            .request_source
            .as_ref()
            .map(|(signer, audience)| {
                signer.issue(
                    audience,
                    &self.workspace_id,
                    request.worker_id.as_deref(),
                    &request.permission,
                    request.method.as_str(),
                    &request_target,
                    &request.body,
                    unix_now_seconds(),
                    DEFAULT_REQUEST_PROOF_TTL_SECONDS,
                )
            })
            .transpose()?;
        Ok(PreparedRuntimeWorkspaceRequest { url, proof })
    }
}

struct PreparedRuntimeWorkspaceRequest {
    url: reqwest::Url,
    proof: Option<String>,
}

fn reqwest_error_chain(error: &reqwest::Error) -> String {
    let mut message = error.to_string();
    let mut source = error.source();
    while let Some(error) = source {
        message.push_str(": ");
        message.push_str(&error.to_string());
        source = error.source();
    }
    message
}

fn unix_now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::decode_runtime_request_source_claims;

    #[test]
    fn route_issues_workspace_scoped_request_proof() {
        let identity = RuntimeIdentityMaterial::generate("runtime-a").unwrap();
        let client = RuntimeWorkspaceRequestClient::new(
            "workspace-a",
            "https://workspace.example.test/",
            "runtime-a",
        )
        .with_runtime_request_source(&identity, "workspace-server-a");
        let request = RuntimeWorkspaceRequest {
            method: reqwest::Method::GET,
            path_and_query: "/api/w/workspace-a/runtime-config?profile=coder".to_string(),
            body: Vec::new(),
            headers: HeaderMap::new(),
            permission: "backend.resource.fetch".to_string(),
            worker_id: None,
            timeout: Some(Duration::from_secs(5)),
            max_response_bytes: 1024,
        };

        let prepared = client.prepare(&request).unwrap();
        let claims = decode_runtime_request_source_claims(&prepared.proof.unwrap()).unwrap();
        assert_eq!(claims.aud, "workspace-server-a");
        assert_eq!(claims.workspace_id, "workspace-a");
        assert_eq!(claims.worker_id, None);
        assert_eq!(claims.method, "GET");
        assert_eq!(
            claims.path,
            "/api/w/workspace-a/runtime-config?profile=coder"
        );
    }

    #[test]
    fn route_matches_only_its_workspace_and_backend() {
        let client = RuntimeWorkspaceRequestClient::new(
            "workspace-a",
            "https://workspace.example.test/",
            "runtime-a",
        );

        assert!(client.matches_workspace("workspace-a", "https://workspace.example.test"));
        assert!(!client.matches_workspace("workspace-b", "https://workspace.example.test"));
        assert!(!client.matches_workspace("workspace-a", "https://other.example.test"));
    }
}
