//! Identity-bound Drive API adapter. The Backend owns grants, hierarchy, CAS and
//! receipts. In particular, a lost mutation response is queried, never resent.
use std::{sync::Arc, time::Duration};

use crate::worker::{
    WorkspaceBinaryRequest, WorkspaceClient, WorkspaceRequest, WorkspaceRequestMethod,
};
use serde::{Serialize, de::DeserializeOwned};
use server_api::*;

pub(super) const DEADLINE: Duration = Duration::from_secs(30);
const RESPONSE_LIMIT: usize = 2 * 1024 * 1024;

#[derive(Clone)]
pub struct DriveBackend {
    client: Arc<dyn WorkspaceClient>,
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum DriveError {
    #[error("Drive access denied")]
    Denied,
    #[error("Drive resource not found")]
    NotFound,
    #[error("Drive revision or name conflict; observe explicitly before another operation")]
    Conflict,
    #[error("Invalid Drive arguments: {0}")]
    Invalid(String),
    #[error("Drive request exceeds a limit")]
    Limit,
    #[error("Drive storage or transport unavailable")]
    Unavailable,
    #[error(
        "Drive mutation outcome unknown; request_id={0}; query request_status; do not blindly repeat"
    )]
    OutcomeUnknown(String),
}

impl DriveBackend {
    pub fn new(client: Arc<dyn WorkspaceClient>) -> Self {
        Self { client }
    }
    pub fn workspace(&self) -> Result<&str, DriveError> {
        self.client
            .workspace_id()
            .filter(|id| !id.is_empty() && self.client.is_available())
            .ok_or(DriveError::Denied)
    }
    fn base(&self) -> Result<String, DriveError> {
        Ok(format!("/api/w/{}/drive", encode(self.workspace()?)))
    }
    pub(super) fn validate_entry(&self, entry: &DriveEntry) -> Result<(), DriveError> {
        self.validate_ref(&entry.entry)?;
        if let Some(parent) = &entry.parent {
            self.validate_ref(parent)?;
        }
        if !decimal(&entry.revision) || entry.size.is_some_and(|n| n > DRIVE_FILE_MAX_BYTES) {
            return Err(DriveError::Unavailable);
        }
        Ok(())
    }
    pub(super) fn validate_ref(&self, entry: &DriveEntryRef) -> Result<(), DriveError> {
        if entry.workspace_id != self.workspace()? {
            return Err(DriveError::Denied);
        }
        if !decimal(&entry.node_id) {
            return Err(DriveError::Invalid(
                "canonical decimal node ID required".into(),
            ));
        }
        Ok(())
    }
    async fn json<T: DeserializeOwned + Send + 'static>(
        &self,
        request: WorkspaceRequest,
        mutation: bool,
        request_id: &str,
    ) -> Result<T, DriveError> {
        let client = self.client.clone();
        let response = tokio::time::timeout(
            DEADLINE,
            tokio::task::spawn_blocking(move || client.execute_with_timeout(request, DEADLINE)),
        )
        .await;
        let unknown = || {
            if mutation {
                DriveError::OutcomeUnknown(request_id.into())
            } else {
                DriveError::Unavailable
            }
        };
        let response = response
            .map_err(|_| unknown())?
            .map_err(|_| unknown())?
            .map_err(|_| unknown())?;
        if response.body.len() > RESPONSE_LIMIT {
            return Err(if mutation {
                unknown()
            } else {
                DriveError::Limit
            });
        }
        if !response.is_success() {
            return Err(classify(
                response.status,
                response.body.as_bytes(),
                mutation,
                request_id,
            ));
        }
        serde_json::from_str(&response.body).map_err(|_| unknown())
    }
    async fn get<T: DeserializeOwned + Send + 'static, Q: Serialize>(
        &self,
        suffix: &str,
        query: &Q,
    ) -> Result<T, DriveError> {
        let query = query_string(query)?;
        self.json(
            WorkspaceRequest::get(format!("{}{suffix}{query}", self.base()?)),
            false,
            "",
        )
        .await
    }
    pub async fn root(&self) -> Result<DriveEntry, DriveError> {
        let entry: DriveEntry = self
            .json(
                WorkspaceRequest::get(format!("{}/root", self.base()?)),
                false,
                "",
            )
            .await?;
        self.validate_entry(&entry)?;
        if entry.kind != DriveEntryKind::Folder || entry.parent.is_some() {
            return Err(DriveError::Unavailable);
        }
        Ok(entry)
    }
    pub async fn metadata(&self, entry: &DriveEntryRef) -> Result<DriveEntry, DriveError> {
        self.validate_ref(entry)?;
        let result: DriveEntry = self
            .get(
                "/metadata",
                &DriveEntryQuery {
                    entry_workspace_id: entry.workspace_id.clone(),
                    id: entry.node_id.clone(),
                },
            )
            .await?;
        self.validate_entry(&result)?;
        if result.entry != *entry {
            return Err(DriveError::Unavailable);
        }
        Ok(result)
    }
    pub async fn list(
        &self,
        parent: &DriveEntryRef,
        limit: Option<u32>,
        after: Option<String>,
    ) -> Result<DriveListResponse, DriveError> {
        self.validate_ref(parent)?;
        let result: DriveListResponse = self
            .get(
                "/list",
                &DriveListQuery {
                    entry_workspace_id: parent.workspace_id.clone(),
                    id: parent.node_id.clone(),
                    limit,
                    after,
                },
            )
            .await?;
        self.validate_page(
            &result,
            limit
                .unwrap_or(DRIVE_PAGE_MAX_LIMIT)
                .min(DRIVE_PAGE_MAX_LIMIT),
        )?;
        if result
            .entries
            .iter()
            .any(|entry| entry.parent.as_ref() != Some(parent))
        {
            return Err(DriveError::Unavailable);
        }
        Ok(result)
    }
    pub async fn search(&self, query: DriveSearchQuery) -> Result<DriveListResponse, DriveError> {
        let result: DriveListResponse = self.get("/search", &query).await?;
        self.validate_page(
            &result,
            query
                .limit
                .unwrap_or(DRIVE_SEARCH_MAX_LIMIT)
                .min(DRIVE_SEARCH_MAX_LIMIT),
        )?;
        Ok(result)
    }
    fn validate_page(&self, page: &DriveListResponse, limit: u32) -> Result<(), DriveError> {
        if page.entries.len() > limit as usize {
            return Err(DriveError::Limit);
        }
        for entry in &page.entries {
            self.validate_entry(entry)?;
        }
        Ok(())
    }
    pub async fn read(
        &self,
        entry: &DriveEntryRef,
        max_bytes: u32,
    ) -> Result<DriveReadTextResponse, DriveError> {
        self.validate_ref(entry)?;
        let result: DriveReadTextResponse = self
            .get(
                "/read-text",
                &DriveReadTextQuery {
                    entry_workspace_id: entry.workspace_id.clone(),
                    id: entry.node_id.clone(),
                    max_bytes,
                },
            )
            .await?;
        self.validate_entry(&result.entry)?;
        if result.entry.entry != *entry || result.text.len() > max_bytes as usize {
            return Err(DriveError::Unavailable);
        }
        Ok(result)
    }
    pub async fn bytes(&self, entry: &DriveEntry, limit: usize) -> Result<Vec<u8>, DriveError> {
        self.validate_entry(entry)?;
        let total = entry
            .size
            .ok_or_else(|| DriveError::Invalid("file required".into()))?;
        if total as usize > limit {
            return Err(DriveError::Limit);
        }
        let mut bytes = Vec::with_capacity(total as usize);
        // Even an empty file gets a revision-fixed read and current authorization.
        loop {
            let offset = bytes.len() as u32;
            let length = total.saturating_sub(offset).clamp(1, DRIVE_CHUNK_MAX_BYTES);
            let query = query_string(&DriveReadChunkQuery {
                entry_workspace_id: entry.entry.workspace_id.clone(),
                id: entry.entry.node_id.clone(),
                expected_revision: entry.revision.clone(),
                offset,
                length,
            })?;
            let request = WorkspaceBinaryRequest {
                method: WorkspaceRequestMethod::Get,
                path: format!("{}/read-chunk{query}", self.base()?),
                body: None,
                max_response_bytes: DRIVE_CHUNK_MAX_BYTES as usize,
            };
            let client = self.client.clone();
            let response = tokio::time::timeout(
                DEADLINE,
                tokio::task::spawn_blocking(move || {
                    client.execute_binary_with_timeout(request, DEADLINE)
                }),
            )
            .await
            .map_err(|_| DriveError::Unavailable)?
            .map_err(|_| DriveError::Unavailable)?
            .map_err(|_| DriveError::Unavailable)?;
            if !(200..300).contains(&response.status) {
                return Err(classify(response.status, &response.body, false, ""));
            }
            let expected = total.saturating_sub(offset).min(length) as usize;
            if response.body.len() != expected {
                return Err(DriveError::Unavailable);
            }
            bytes.extend_from_slice(&response.body);
            if bytes.len() == total as usize {
                return Ok(bytes);
            }
        }
    }
    async fn reconcile(
        &self,
        id: &str,
        result: Result<DriveMutationResponse, DriveError>,
    ) -> Result<DriveMutationResponse, DriveError> {
        // A parseable success can still be an invalid completion. Classify it
        // before reconciliation so it also gets one original-request inquiry.
        let result = result.and_then(|response| {
            self.validate_completion(id, &response)
                .map_err(|_| DriveError::OutcomeUnknown(id.into()))?;
            Ok(response)
        });
        match result {
            Err(DriveError::OutcomeUnknown(_)) => match self.status(id).await {
                Ok(DriveRequestStatusResponse {
                    state: DriveRequestState::Committed,
                    response: Some(response),
                    ..
                }) => Ok(response), // status validates both IDs and entry metadata
                _ => Err(DriveError::OutcomeUnknown(id.into())),
            },
            other => other,
        }
    }
    fn validate_completion(
        &self,
        id: &str,
        response: &DriveMutationResponse,
    ) -> Result<(), DriveError> {
        if response.request_id != id {
            return Err(DriveError::Unavailable);
        }
        if let Some(entry) = &response.entry {
            self.validate_entry(entry)?;
        }
        Ok(())
    }
    pub async fn mutate(
        &self,
        request: DriveMutationRequest,
    ) -> Result<DriveMutationResponse, DriveError> {
        let body = serde_json::to_string(&request)
            .map_err(|_| DriveError::Invalid("invalid mutation".into()))?;
        if body.len() > 512 * 1024 {
            return Err(DriveError::Limit);
        }
        let result = self
            .json(
                WorkspaceRequest::json(
                    WorkspaceRequestMethod::Post,
                    format!("{}/mutate", self.base()?),
                    body,
                ),
                true,
                &request.request_id,
            )
            .await;
        self.reconcile(&request.request_id, result).await
    }
    pub async fn upload(
        &self,
        query: DriveUploadQuery,
        bytes: Vec<u8>,
    ) -> Result<DriveMutationResponse, DriveError> {
        if bytes.len() > DRIVE_FILE_MAX_BYTES as usize {
            return Err(DriveError::Limit);
        }
        let path = format!("{}/upload{}", self.base()?, query_string(&query)?);
        let request = WorkspaceBinaryRequest {
            method: WorkspaceRequestMethod::Put,
            path,
            body: Some(bytes),
            max_response_bytes: RESPONSE_LIMIT,
        };
        let client = self.client.clone();
        let result = tokio::time::timeout(
            DEADLINE,
            tokio::task::spawn_blocking(move || {
                client.execute_binary_with_timeout(request, DEADLINE)
            }),
        )
        .await;
        let result = match result {
            Ok(Ok(Ok(response))) if (200..300).contains(&response.status) => {
                serde_json::from_slice(&response.body)
                    .map_err(|_| DriveError::OutcomeUnknown(query.request_id.clone()))
            }
            Ok(Ok(Ok(response))) => Err(classify(
                response.status,
                &response.body,
                true,
                &query.request_id,
            )),
            _ => Err(DriveError::OutcomeUnknown(query.request_id.clone())),
        };
        self.reconcile(&query.request_id, result).await
    }
    pub async fn status(&self, request_id: &str) -> Result<DriveRequestStatusResponse, DriveError> {
        if request_id.is_empty() || request_id.len() > 256 {
            return Err(DriveError::Invalid("invalid request ID".into()));
        }
        let response: DriveRequestStatusResponse = self
            .json(
                WorkspaceRequest::get(format!("{}/requests/{}", self.base()?, encode(request_id))),
                false,
                "",
            )
            .await?;
        if response.request_id != request_id {
            return Err(DriveError::Unavailable);
        }
        if let Some(result) = &response.response {
            self.validate_completion(request_id, result)?;
        }
        Ok(response)
    }
}

pub(super) fn decimal(value: &str) -> bool {
    value
        .as_bytes()
        .first()
        .is_some_and(|c| matches!(c, b'1'..=b'9'))
        && value.bytes().all(|c| c.is_ascii_digit())
        && value.parse::<i64>().is_ok_and(|id| id > 0)
}
pub(super) fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_') {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}
fn query_string<T: Serialize>(value: &T) -> Result<String, DriveError> {
    let value =
        serde_json::to_value(value).map_err(|_| DriveError::Invalid("invalid query".into()))?;
    let object = value
        .as_object()
        .ok_or_else(|| DriveError::Invalid("invalid query".into()))?;
    let pairs: Vec<_> = object
        .iter()
        .filter(|(_, v)| !v.is_null())
        .map(|(k, v)| {
            format!(
                "{}={}",
                encode(k),
                encode(
                    v.as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| v.to_string())
                        .as_str()
                )
            )
        })
        .collect();
    Ok(if pairs.is_empty() {
        String::new()
    } else {
        format!("?{}", pairs.join("&"))
    })
}
fn classify(status: u16, body: &[u8], mutation: bool, id: &str) -> DriveError {
    if let Ok(error) = serde_json::from_slice::<DriveApiError>(body) {
        if error.classification == DriveFailureClassification::Unknown {
            return DriveError::OutcomeUnknown(id.into());
        }
        return match error.code {
            DriveApiErrorCode::Denied => DriveError::Denied,
            DriveApiErrorCode::NotFound => DriveError::NotFound,
            DriveApiErrorCode::Conflict => DriveError::Conflict,
            DriveApiErrorCode::Invalid => DriveError::Invalid("Backend rejected arguments".into()),
            DriveApiErrorCode::Limit => DriveError::Limit,
            DriveApiErrorCode::StorageUnavailable => DriveError::Unavailable,
            DriveApiErrorCode::OutcomeUnknown => DriveError::OutcomeUnknown(id.into()),
        };
    }
    if matches!(status, 401 | 403) {
        DriveError::Denied
    } else if mutation {
        DriveError::OutcomeUnknown(id.into())
    } else {
        DriveError::Unavailable
    }
}
