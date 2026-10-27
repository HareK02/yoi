//! Identity-bound Drive API adapter and mutation receipt checks.
//!
//! The Backend owns grants, hierarchy, CAS, and receipts, applying current authorization to every
//! read/chunk/status request. The adapter validates Workspace, node ID, commit ID, and size, and
//! never copies raw diagnostics that may contain provider paths, SQL, or credentials into output.
//!
//! Mutations use Host-generated request IDs and observed node/commit IDs. Conflicts do not trigger
//! retries against substituted current state. For an unknown mutation response, check the same
//! request's receipt once; only a committed receipt with valid IDs and metadata proves DB success.
//! An uncommitted snapshot cannot rule out a later commit by an in-flight request, so all other
//! cases retain an unknown outcome with the original request ID. Never blindly resend with a new ID.
use std::{sync::Arc, time::Duration};

use super::super::backend_diagnostic::{Adapter, Diagnostic, Reason, Stage};

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
    deadline: Duration,
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum DriveError {
    #[error("Drive access denied")]
    Denied,
    #[error("Drive resource not found")]
    NotFound,
    #[error(
        "Drive content, location, or name conflict; observe explicitly before another operation"
    )]
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
        Self {
            client,
            deadline: DEADLINE,
        }
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
        self.validate_entry_at(
            entry,
            &Diagnostic::new(Adapter::Drive, "validate_entry", false, None),
        )
    }
    fn validate_entry_at(
        &self,
        entry: &DriveEntry,
        diagnostic: &Diagnostic,
    ) -> Result<(), DriveError> {
        self.validate_ref(&entry.entry)
            .inspect_err(|_| diagnostic.record(Stage::Completion, Reason::Identity, None))?;
        if let Some(parent) = &entry.parent {
            self.validate_ref(parent)
                .inspect_err(|_| diagnostic.record(Stage::Completion, Reason::Identity, None))?;
        }
        if (entry.last_mutation_id.len() > 128
            || entry.last_mutation_id.chars().any(char::is_control)
            || (entry.parent.is_some() && entry.last_mutation_id.is_empty()))
            || entry.size.is_some_and(|n| n > DRIVE_FILE_MAX_BYTES)
        {
            diagnostic.record(Stage::Completion, Reason::InvalidMetadata, None);
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
        diagnostic: &Diagnostic,
    ) -> Result<T, DriveError> {
        let client = self.client.clone();
        let deadline = self.deadline;
        let response = tokio::time::timeout(
            deadline,
            tokio::task::spawn_blocking(move || client.execute_with_timeout(request, deadline)),
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
            .map_err(|_| {
                diagnostic.record(Stage::Request, Reason::Deadline, None);
                unknown()
            })?
            .map_err(|_| {
                diagnostic.record(Stage::Request, Reason::TaskJoin, None);
                unknown()
            })?
            .map_err(|error| {
                diagnostic.transport(&error);
                unknown()
            })?;
        if response.body.len() > RESPONSE_LIMIT {
            diagnostic.record(Stage::Response, Reason::ResponseSize, None);
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
                diagnostic,
            ));
        }
        serde_json::from_str(&response.body).map_err(|_| {
            diagnostic.record(Stage::Response, Reason::Decode, None);
            unknown()
        })
    }
    async fn get<T: DeserializeOwned + Send + 'static, Q: Serialize>(
        &self,
        suffix: &'static str,
        query: &Q,
        diagnostic: &Diagnostic,
    ) -> Result<T, DriveError> {
        let query = query_string(query)?;
        self.json(
            WorkspaceRequest::get(format!("{}{suffix}{query}", self.base()?)),
            false,
            "",
            diagnostic,
        )
        .await
    }
    pub async fn capabilities(&self) -> Result<DriveCapabilities, DriveError> {
        self.get(
            "/capabilities",
            &serde_json::json!({}),
            &Diagnostic::new(Adapter::Drive, "capabilities", false, None),
        )
        .await
    }
    pub async fn root(&self) -> Result<DriveEntry, DriveError> {
        let diagnostic = Diagnostic::new(Adapter::Drive, "root", false, None);
        let entry: DriveEntry = self
            .json(
                WorkspaceRequest::get(format!("{}/root", self.base()?)),
                false,
                "",
                &diagnostic,
            )
            .await?;
        self.validate_entry_at(&entry, &diagnostic)?;
        if entry.kind != DriveEntryKind::Folder || entry.parent.is_some() {
            diagnostic.record(Stage::Completion, Reason::InvalidMetadata, None);
            return Err(DriveError::Unavailable);
        }
        Ok(entry)
    }
    pub async fn metadata(&self, entry: &DriveEntryRef) -> Result<DriveEntry, DriveError> {
        self.validate_ref(entry)?;
        let diagnostic = Diagnostic::new(Adapter::Drive, "metadata", false, None);
        let result: DriveEntry = self
            .get(
                "/metadata",
                &DriveEntryQuery {
                    entry_workspace_id: entry.workspace_id.clone(),
                    id: entry.node_id.clone(),
                },
                &diagnostic,
            )
            .await?;
        self.validate_entry_at(&result, &diagnostic)?;
        if result.entry != *entry {
            diagnostic.record(Stage::Completion, Reason::Identity, None);
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
        let diagnostic = Diagnostic::new(Adapter::Drive, "list", false, None);
        let result: DriveListResponse = self
            .get(
                "/list",
                &DriveListQuery {
                    entry_workspace_id: parent.workspace_id.clone(),
                    id: parent.node_id.clone(),
                    limit,
                    after,
                },
                &diagnostic,
            )
            .await?;
        self.validate_page(
            &result,
            limit
                .unwrap_or(DRIVE_PAGE_MAX_LIMIT)
                .min(DRIVE_PAGE_MAX_LIMIT),
            &diagnostic,
        )?;
        if result
            .entries
            .iter()
            .any(|entry| entry.parent.as_ref() != Some(parent))
        {
            diagnostic.record(Stage::Completion, Reason::Identity, None);
            return Err(DriveError::Unavailable);
        }
        Ok(result)
    }
    pub async fn search(&self, query: DriveSearchQuery) -> Result<DriveListResponse, DriveError> {
        let diagnostic = Diagnostic::new(Adapter::Drive, "search", false, None);
        let result: DriveListResponse = self.get("/search", &query, &diagnostic).await?;
        self.validate_page(
            &result,
            query
                .limit
                .unwrap_or(DRIVE_SEARCH_MAX_LIMIT)
                .min(DRIVE_SEARCH_MAX_LIMIT),
            &diagnostic,
        )?;
        Ok(result)
    }
    fn validate_page(
        &self,
        page: &DriveListResponse,
        limit: u32,
        diagnostic: &Diagnostic,
    ) -> Result<(), DriveError> {
        if page.entries.len() > limit as usize {
            diagnostic.record(Stage::Completion, Reason::ResponseSize, None);
            return Err(DriveError::Limit);
        }
        for entry in &page.entries {
            self.validate_entry_at(entry, diagnostic)?;
        }
        Ok(())
    }
    pub async fn read(
        &self,
        entry: &DriveEntryRef,
        max_bytes: u32,
    ) -> Result<DriveReadTextResponse, DriveError> {
        self.validate_ref(entry)?;
        let diagnostic = Diagnostic::new(Adapter::Drive, "read_text", false, None);
        let result: DriveReadTextResponse = self
            .get(
                "/read-text",
                &DriveReadTextQuery {
                    entry_workspace_id: entry.workspace_id.clone(),
                    id: entry.node_id.clone(),
                    max_bytes,
                },
                &diagnostic,
            )
            .await?;
        self.validate_entry_at(&result.entry, &diagnostic)?;
        if result.entry.entry != *entry {
            diagnostic.record(Stage::Completion, Reason::Identity, None);
            return Err(DriveError::Unavailable);
        }
        if result.text.len() > max_bytes as usize {
            diagnostic.record(Stage::Completion, Reason::ResponseSize, None);
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
        let diagnostic = Diagnostic::new(Adapter::Drive, "read_chunk", false, None);
        let mut bytes = Vec::with_capacity(total as usize);
        // Even an empty file gets a read bound to the observed committed mutation and current authorization.
        loop {
            let offset = bytes.len() as u32;
            let length = total.saturating_sub(offset).clamp(1, DRIVE_CHUNK_MAX_BYTES);
            let query = query_string(&DriveReadChunkQuery {
                entry_workspace_id: entry.entry.workspace_id.clone(),
                id: entry.entry.node_id.clone(),
                expected_mutation_id: entry.last_mutation_id.clone(),
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
            let deadline = self.deadline;
            let response = tokio::time::timeout(
                deadline,
                tokio::task::spawn_blocking(move || {
                    client.execute_binary_with_timeout(request, deadline)
                }),
            )
            .await
            .map_err(|_| {
                diagnostic.record(Stage::Request, Reason::Deadline, None);
                DriveError::Unavailable
            })?
            .map_err(|_| {
                diagnostic.record(Stage::Request, Reason::TaskJoin, None);
                DriveError::Unavailable
            })?
            .map_err(|error| {
                diagnostic.transport(&error);
                DriveError::Unavailable
            })?;
            if response.body.len() > DRIVE_CHUNK_MAX_BYTES as usize {
                diagnostic.record(Stage::Response, Reason::ResponseSize, None);
                return Err(DriveError::Unavailable);
            }
            if !(200..300).contains(&response.status) {
                return Err(classify(
                    response.status,
                    &response.body,
                    false,
                    "",
                    &diagnostic,
                ));
            }
            let expected = total.saturating_sub(offset).min(length) as usize;
            if response.body.len() != expected {
                diagnostic.record(Stage::Response, Reason::ResponseLength, None);
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
        diagnostic: &Diagnostic,
    ) -> Result<DriveMutationResponse, DriveError> {
        // A parseable success can still be an invalid completion. Classify it
        // before reconciliation so it also gets one original-request inquiry.
        let result = result.and_then(|response| {
            self.validate_completion(id, &response, diagnostic)
                .map_err(|_| DriveError::OutcomeUnknown(id.into()))?;
            Ok(response)
        });
        match result {
            Err(DriveError::OutcomeUnknown(_)) => match self
                .status_with_diagnostic(id, &diagnostic.at_stage(Stage::Reconcile))
                .await
            {
                Ok(DriveRequestStatusResponse {
                    state: DriveRequestState::Committed,
                    response: Some(response),
                    ..
                }) => {
                    diagnostic.record(Stage::Reconcile, Reason::Reconciled, None);
                    Ok(response) // status validates both IDs and entry metadata
                }
                Ok(status) => {
                    diagnostic.record(
                        Stage::Reconcile,
                        match status.state {
                            DriveRequestState::Uncommitted => Reason::NotCommittedSnapshot,
                            DriveRequestState::Committed => Reason::IncompleteReceipt,
                        },
                        None,
                    );
                    Err(DriveError::OutcomeUnknown(id.into()))
                }
                Err(_) => Err(DriveError::OutcomeUnknown(id.into())),
            },
            other => other,
        }
    }
    fn validate_completion(
        &self,
        id: &str,
        response: &DriveMutationResponse,
        diagnostic: &Diagnostic,
    ) -> Result<(), DriveError> {
        if response.request_id != id {
            diagnostic.record(Stage::Completion, Reason::Identity, None);
            return Err(DriveError::Unavailable);
        }
        if let Some(entry) = &response.entry {
            self.validate_entry_at(entry, diagnostic)?;
        }
        Ok(())
    }
    pub async fn mutate(
        &self,
        request: DriveMutationRequest,
    ) -> Result<DriveMutationResponse, DriveError> {
        let diagnostic = Diagnostic::new(Adapter::Drive, "mutate", true, Some(&request.request_id));
        let body = serde_json::to_string(&request)
            .map_err(|_| DriveError::Invalid("invalid mutation".into()))?;
        if body.len() > 512 * 1024 {
            diagnostic.record(Stage::Request, Reason::RequestSize, None);
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
                &diagnostic,
            )
            .await;
        self.reconcile(&request.request_id, result, &diagnostic)
            .await
    }
    pub async fn upload(
        &self,
        query: DriveUploadQuery,
        bytes: Vec<u8>,
    ) -> Result<DriveMutationResponse, DriveError> {
        let diagnostic = Diagnostic::new(Adapter::Drive, "upload", true, Some(&query.request_id));
        if bytes.len() > DRIVE_FILE_MAX_BYTES as usize {
            diagnostic.record(Stage::Request, Reason::RequestSize, None);
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
        let deadline = self.deadline;
        let result = tokio::time::timeout(
            deadline,
            tokio::task::spawn_blocking(move || {
                client.execute_binary_with_timeout(request, deadline)
            }),
        )
        .await;
        let result = match result {
            Ok(Ok(Ok(response))) if response.body.len() > RESPONSE_LIMIT => {
                diagnostic.record(Stage::Response, Reason::ResponseSize, None);
                Err(DriveError::OutcomeUnknown(query.request_id.clone()))
            }
            Ok(Ok(Ok(response))) if (200..300).contains(&response.status) => {
                serde_json::from_slice(&response.body).map_err(|_| {
                    diagnostic.record(Stage::Response, Reason::Decode, None);
                    DriveError::OutcomeUnknown(query.request_id.clone())
                })
            }
            Ok(Ok(Ok(response))) => Err(classify(
                response.status,
                &response.body,
                true,
                &query.request_id,
                &diagnostic,
            )),
            Err(_) => {
                diagnostic.record(Stage::Request, Reason::Deadline, None);
                Err(DriveError::OutcomeUnknown(query.request_id.clone()))
            }
            Ok(Err(_)) => {
                diagnostic.record(Stage::Request, Reason::TaskJoin, None);
                Err(DriveError::OutcomeUnknown(query.request_id.clone()))
            }
            Ok(Ok(Err(error))) => {
                diagnostic.transport(&error);
                Err(DriveError::OutcomeUnknown(query.request_id.clone()))
            }
        };
        self.reconcile(&query.request_id, result, &diagnostic).await
    }
    pub async fn status(&self, request_id: &str) -> Result<DriveRequestStatusResponse, DriveError> {
        let diagnostic = Diagnostic::new(Adapter::Drive, "request_status", false, Some(request_id));
        self.status_with_diagnostic(request_id, &diagnostic).await
    }
    async fn status_with_diagnostic(
        &self,
        request_id: &str,
        diagnostic: &Diagnostic,
    ) -> Result<DriveRequestStatusResponse, DriveError> {
        if request_id.is_empty() || request_id.len() > 256 {
            return Err(DriveError::Invalid("invalid request ID".into()));
        }
        let response: DriveRequestStatusResponse = self
            .json(
                WorkspaceRequest::get(format!("{}/requests/{}", self.base()?, encode(request_id))),
                false,
                "",
                diagnostic,
            )
            .await?;
        if response.request_id != request_id {
            diagnostic.record(Stage::Reconcile, Reason::Identity, None);
            return Err(DriveError::Unavailable);
        }
        if let Some(result) = &response.response {
            self.validate_completion(request_id, result, diagnostic)?;
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
fn classify(
    status: u16,
    body: &[u8],
    mutation: bool,
    id: &str,
    diagnostic: &Diagnostic,
) -> DriveError {
    if let Ok(error) = serde_json::from_slice::<DriveApiError>(body) {
        if error.classification == DriveFailureClassification::Unknown
            || error.code == DriveApiErrorCode::OutcomeUnknown
        {
            diagnostic.record(Stage::Response, Reason::BackendUnknown, Some(status));
            return DriveError::OutcomeUnknown(id.into());
        }
        diagnostic.record(Stage::Response, Reason::BackendRejected, Some(status));
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
    diagnostic.record(Stage::Response, Reason::UnrecognizedResponse, Some(status));
    if matches!(status, 401 | 403) {
        DriveError::Denied
    } else if mutation {
        DriveError::OutcomeUnknown(id.into())
    } else {
        DriveError::Unavailable
    }
}

#[cfg(test)]
#[path = "backend_tests.rs"]
mod tests;
