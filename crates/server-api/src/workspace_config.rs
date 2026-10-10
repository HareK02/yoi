//! Grant-bound logical Workspace configuration transport. No host filesystem contract.
use super::{ConfigCommitRequest, ConfigContentType};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceConfigAccess {
    ReadOnly,
    ReadWrite,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceConfigNodeKind {
    Directory,
    File,
    Missing,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceConfigFailureClassification {
    NotCommitted,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkspaceConfigAttachRequest {
    #[serde(default)]
    pub alias: Option<String>,
    #[serde(default)]
    pub access: Option<WorkspaceConfigAccess>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkspaceConfigAttachment {
    pub workspace_id: String,
    pub connection_id: String,
    pub alias: String,
    pub working_directory_id: String,
    pub access: WorkspaceConfigAccess,
    pub name: String,
    pub purpose: String,
    pub content_path: String,
    pub already_attached: bool,
}
/// Named transport wrapper required by the generated API; JSON remains attachment-or-null.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(transparent)]
pub struct WorkspaceConfigCurrentResponse(pub Option<WorkspaceConfigAttachment>);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkspaceConfigObserveRequest {
    pub connection_id: String,
    pub paths: Vec<String>,
    pub depth: u32,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkspaceConfigNode {
    pub path: String,
    pub kind: WorkspaceConfigNodeKind,
    pub validator: String,
    pub digest: Option<String>,
    pub content_type: Option<ConfigContentType>,
    pub operations: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkspaceConfigObserveResponse {
    pub connection_id: String,
    pub validator: String,
    pub digest: String,
    /// Canonical commit entrypoints from this exact metadata snapshot.
    pub entrypoints: Vec<String>,
    pub nodes: Vec<WorkspaceConfigNode>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkspaceConfigReadRequest {
    pub connection_id: String,
    pub path: String,
    pub validator: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkspaceConfigReadResponse {
    pub path: String,
    pub content: String,
    pub content_type: ConfigContentType,
    pub digest: String,
    pub validator: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkspaceConfigCommitRequest {
    pub connection_id: String,
    pub validator: String,
    pub request: ConfigCommitRequest,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkspaceConfigCommitResponse {
    pub validator: String,
    pub digest: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkspaceConfigApiError {
    pub status: u16,
    pub code: String,
    pub message: String,
    pub classification: WorkspaceConfigFailureClassification,
}
impl api_macros::HttpError for WorkspaceConfigApiError {
    fn status_code(&self) -> u16 {
        self.status
    }
}
impl api_macros::HttpRequestError for WorkspaceConfigApiError {
    fn from_request_rejection(status: u16, _message: String) -> Self {
        Self {
            status,
            code: "invalid_request".into(),
            message: "Invalid configuration request".into(),
            classification: WorkspaceConfigFailureClassification::NotCommitted,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkspaceConfigGrantCreateRequest {
    pub runtime_id: String,
    pub worker_id: String,
    pub access: WorkspaceConfigAccess,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkspaceConfigGrantResponse {
    pub grant_id: String,
    pub workspace_id: String,
    pub runtime_id: String,
    pub worker_id: String,
    pub working_directory_id: String,
    pub access: WorkspaceConfigAccess,
    pub revoked: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ApiContract, HttpMethod, ServerApiMetadata};

    #[test]
    fn workspace_config_observation_is_metadata_only_and_has_same_snapshot_cas_inputs() {
        let value = serde_json::json!({"connection_id":"c", "validator":"v", "digest":"d", "entrypoints":["main.dcdl"], "nodes":[{"path":"", "kind":"directory", "validator":"v", "digest":null, "content_type":null, "operations":["apply_changes"]}]});
        let response: WorkspaceConfigObserveResponse =
            serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(response).unwrap(), value);
        for field in ["validator", "digest", "entrypoints"] {
            let mut missing = value.clone();
            missing.as_object_mut().unwrap().remove(field);
            assert!(serde_json::from_value::<WorkspaceConfigObserveResponse>(missing).is_err());
        }
        let mut with_body = value;
        with_body["nodes"][0]["content"] = serde_json::json!("must never appear here");
        assert!(serde_json::from_value::<WorkspaceConfigObserveResponse>(with_body).is_err());
        assert!(
            serde_json::from_value::<WorkspaceConfigGrantCreateRequest>(
                serde_json::json!({"runtime_id":"r","worker_id":"w","access":"read_write_command"})
            )
            .is_err()
        );
        assert!(
            serde_json::from_value::<WorkspaceConfigAttachRequest>(
                serde_json::json!({"grant_id":"invented"})
            )
            .is_err()
        );
        assert_eq!(
            serde_json::to_value(WorkspaceConfigCurrentResponse(None)).unwrap(),
            serde_json::Value::Null
        );
    }

    #[test]
    fn workspace_config_commit_response_is_content_based() {
        let value = serde_json::json!({"validator":"v", "digest":"d"});
        let response: WorkspaceConfigCommitResponse =
            serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(response).unwrap(), value);
        for field in ["validator", "digest"] {
            let mut missing = value.clone();
            missing.as_object_mut().unwrap().remove(field);
            assert!(serde_json::from_value::<WorkspaceConfigCommitResponse>(missing).is_err());
        }
    }

    #[test]
    fn workspace_config_failure_classification_roundtrips_and_rejections_are_pre_effects() {
        for classification in [
            WorkspaceConfigFailureClassification::NotCommitted,
            WorkspaceConfigFailureClassification::Unknown,
        ] {
            let error = WorkspaceConfigApiError {
                status: 500,
                code: "unavailable".into(),
                message: "safe".into(),
                classification,
            };
            assert_eq!(
                serde_json::from_value::<WorkspaceConfigApiError>(
                    serde_json::to_value(&error).unwrap()
                )
                .unwrap(),
                error
            );
            assert_eq!(api_macros::HttpError::status_code(&error), 500);
        }
        let error =
            <WorkspaceConfigApiError as api_macros::HttpRequestError>::from_request_rejection(
                413,
                "/private/secret".into(),
            );
        assert_eq!(
            error.classification,
            WorkspaceConfigFailureClassification::NotCommitted
        );
        assert!(!error.message.contains("private"));
    }

    #[test]
    fn workspace_config_generated_routes_are_exact_and_grants_are_separate() {
        for (name, method, suffix) in [
            ("get", HttpMethod::Get, ""),
            ("attach", HttpMethod::Post, ""),
            ("observe", HttpMethod::Post, "/observe"),
            ("read", HttpMethod::Post, "/read"),
            ("commit", HttpMethod::Post, "/commit"),
        ] {
            let name = format!("current_worker_workspace_config_{name}");
            let operation = ServerApiMetadata::OPERATIONS
                .iter()
                .find(|operation| operation.operation_id == name)
                .unwrap();
            assert_eq!(operation.method, method);
            assert_eq!(
                operation.path,
                format!("/api/w/{{workspace_id}}/workers/self/workspace-config{suffix}")
            );
        }
        for (name, method, path) in [
            (
                "workspace_config_grant_create",
                HttpMethod::Post,
                "/api/w/{workspace_id}/workspace-config-grants",
            ),
            (
                "workspace_config_grant_revoke",
                HttpMethod::Delete,
                "/api/w/{workspace_id}/workspace-config-grants/{grant_id}",
            ),
        ] {
            let operation = ServerApiMetadata::OPERATIONS
                .iter()
                .find(|operation| operation.operation_id == name)
                .unwrap();
            assert_eq!(operation.method, method);
            assert_eq!(operation.path, path);
        }
    }
}
