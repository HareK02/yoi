//! Pulls only from the Runtime's current registered Server. Credentials, URLs
//! and transport diagnostics stay in the Host, outside plan and model inputs.
//! Registration is reread for every operation, including cached-byte admission;
//! a captured registration is not a perpetual lease after trust revocation.
use super::*;
use crate::workspace_issuer::{WorkspaceIssuerTrustRecord, WorkspaceIssuerTrustState};
use server_api::{PluginArtifactDeliveryMetadata, PluginArtifactDeliveryRequest};
use std::sync::Arc;
use yoi_plugin_runtime::delivery::{ArtifactSource, ArtifactStream, DeliveryError};

pub type CurrentArtifactServer =
    dyn Fn() -> Result<WorkspaceIssuerTrustRecord, DeliveryError> + Send + Sync;

pub struct RemotePluginArtifactSource {
    client: RuntimeWorkspaceRequestClient,
    worker_id: String,
    authority: String,
    current_server: Arc<CurrentArtifactServer>,
}

impl RemotePluginArtifactSource {
    /// `current_server` reads Runtime-owned registration, never a model-provided
    /// bundle or credentials. The request client must already have a Runtime key
    /// and the registered Backend URL as its signing audience.
    pub fn new(
        client: RuntimeWorkspaceRequestClient,
        worker_id: String,
        current_server: Arc<CurrentArtifactServer>,
    ) -> Result<Self, DeliveryError> {
        if worker_id.is_empty() {
            return Err(DeliveryError::InvalidRequest);
        }
        let registration = current_server()?;
        let authority = server_api::plugin_server_authority_identity(&registration.public_bundle());
        let source = Self {
            client,
            worker_id,
            authority,
            current_server,
        };
        source.registration()?;
        Ok(source)
    }

    pub(crate) fn registration(&self) -> Result<(), DeliveryError> {
        let registration = (self.current_server)()?;
        if registration.state != WorkspaceIssuerTrustState::Active
            || registration.workspace_id != self.client.workspace_id
            || registration.backend_url.trim_end_matches('/') != self.client.base_url
            || self.client.audience().map(|a| a.trim_end_matches('/'))
                != Some(self.client.base_url.as_str())
            || server_api::plugin_server_authority_identity(&registration.public_bundle())
                != self.authority
        {
            return Err(DeliveryError::Denied);
        }
        Ok(())
    }

    fn request(
        &self,
        suffix: &str,
        selection: &PluginArtifactDeliveryRequest,
    ) -> Result<RuntimeWorkspaceRequest, DeliveryError> {
        self.registration()?;
        let mut url = reqwest::Url::parse(&self.client.base_url)
            .map_err(|_| DeliveryError::InvalidRequest)?;
        url.path_segments_mut()
            .map_err(|_| DeliveryError::InvalidRequest)?
            .pop_if_empty()
            .extend([
                "api",
                "w",
                self.client.workspace_id(),
                "workers",
                "self",
                "plugin-artifacts",
                suffix,
            ]);
        let path_and_query = url.path().to_owned();
        Ok(RuntimeWorkspaceRequest {
            method: reqwest::Method::POST,
            path_and_query,
            body: serde_json::to_vec(selection).map_err(|_| DeliveryError::InvalidRequest)?,
            headers: HeaderMap::from_iter([(
                reqwest::header::CONTENT_TYPE,
                reqwest::header::HeaderValue::from_static("application/json"),
            )]),
            permission: crate::auth::WORKSPACE_REQUEST_PERMISSION.into(),
            worker_id: Some(self.worker_id.clone()),
            timeout: Some(Duration::from_secs(30)),
            max_response_bytes: manifest::plugin::PluginPackageLimits::default()
                .max_package_size_bytes as usize,
        })
    }

    async fn response(
        &self,
        request: RuntimeWorkspaceRequest,
    ) -> Result<reqwest::Response, DeliveryError> {
        let prepared = self.client.prepare(&request).map_err(|_| unavailable())?;
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|_| unavailable())?;
        let response = client
            .request(request.method, prepared.url)
            .headers(request.headers)
            .header(RUNTIME_ID_HEADER, &self.client.runtime_id)
            .header(WORKER_ID_HEADER, &self.worker_id)
            .header(
                RUNTIME_REQUEST_SOURCE_PROOF_HEADER,
                prepared.proof.ok_or(DeliveryError::Denied)?,
            )
            .body(request.body)
            .send()
            .await
            .map_err(|_| unavailable())?;
        if !response.status().is_success() {
            return Err(if response.status().is_server_error() {
                unavailable()
            } else {
                DeliveryError::Denied
            });
        }
        if response
            .content_length()
            .is_some_and(|size| size > request.max_response_bytes as u64)
        {
            return Err(DeliveryError::TooLarge);
        }
        self.registration()?;
        Ok(response)
    }
}

fn unavailable() -> DeliveryError {
    DeliveryError::Unavailable("registered owner unavailable".into())
}

#[async_trait::async_trait]
impl ArtifactSource for RemotePluginArtifactSource {
    fn server_authority_identity(&self) -> &str {
        &self.authority
    }

    async fn authorize(
        &self,
        selection: &PluginArtifactDeliveryRequest,
    ) -> Result<PluginArtifactDeliveryMetadata, DeliveryError> {
        let request = self.request("authorize", selection)?;
        let response = self.response(request).await?;
        // Static schema metadata is also bounded; never use response.json() on an
        // unbounded peer response or reflect its body into diagnostics.
        let mut bytes = Vec::new();
        let mut chunks = response.bytes_stream();
        while let Some(chunk) = chunks.next().await {
            let chunk = chunk.map_err(|_| unavailable())?;
            if bytes.len().saturating_add(chunk.len()) > 1024 * 1024 {
                return Err(DeliveryError::TooLarge);
            }
            bytes.extend_from_slice(&chunk);
        }
        self.registration()?;
        serde_json::from_slice(&bytes).map_err(|_| DeliveryError::MetadataMismatch)
    }

    async fn fetch(
        &self,
        selection: &PluginArtifactDeliveryRequest,
    ) -> Result<ArtifactStream, DeliveryError> {
        let response = self.response(self.request("pull", selection)?).await?;
        let chunks = response.bytes_stream().map(|chunk| {
            chunk.map_err(|_| std::io::Error::other("artifact transport interrupted"))
        });
        Ok(Box::pin(tokio_util::io::StreamReader::new(Box::pin(
            chunks,
        ))))
    }
}
