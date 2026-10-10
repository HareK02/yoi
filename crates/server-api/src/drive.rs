//! Workspace Drive HTTP contracts. Entries are logical, workspace-bound resources,
//! never host paths. Worker identity and grants are resolved by the service from
//! trusted request context, not supplied by ordinary operation DTOs.
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};

pub const DRIVE_TEXT_MAX_BYTES: usize = 64 * 1024;
pub const DRIVE_CHUNK_MAX_BYTES: u32 = 64 * 1024;
pub const DRIVE_PAGE_MAX_LIMIT: u32 = 200;
pub const DRIVE_SEARCH_MAX_LIMIT: u32 = 128;
pub const DRIVE_FILE_MAX_BYTES: u32 = 16 * 1024 * 1024;

fn decimal<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    let value = String::deserialize(deserializer)?;
    if !canonical_positive_decimal(&value) {
        return Err(serde::de::Error::custom(
            "expected a canonical positive decimal string",
        ));
    }
    Ok(value)
}
fn optional_decimal<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    let value = Option::<String>::deserialize(deserializer)?;
    if value
        .as_deref()
        .is_some_and(|value| !canonical_positive_decimal(value))
    {
        return Err(serde::de::Error::custom(
            "expected a canonical positive decimal string",
        ));
    }
    Ok(value)
}
fn canonical_positive_decimal(value: &str) -> bool {
    value
        .as_bytes()
        .first()
        .is_some_and(|first| matches!(first, b'1'..=b'9'))
        && value.bytes().all(|byte| byte.is_ascii_digit())
        && value.parse::<i64>().is_ok_and(|value| value > 0)
}
fn node_mutation_id<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    let value = String::deserialize(deserializer)?;
    if value.len() > 128 || value.chars().any(char::is_control) {
        return Err(serde::de::Error::custom(
            "invalid committed mutation request ID",
        ));
    }
    Ok(value)
}
fn mutation_id<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    let value = node_mutation_id(deserializer)?;
    if value.is_empty() {
        return Err(serde::de::Error::custom(
            "expected a committed mutation request ID",
        ));
    }
    Ok(value)
}
fn optional_mutation_id<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    Option::<String>::deserialize(deserializer)?
        .map(|value| {
            if value.is_empty() || value.len() > 128 || value.chars().any(char::is_control) {
                Err(serde::de::Error::custom(
                    "invalid committed mutation request ID",
                ))
            } else {
                Ok(value)
            }
        })
        .transpose()
}
fn bounded_text<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    let value = String::deserialize(deserializer)?;
    if value.len() > DRIVE_TEXT_MAX_BYTES {
        return Err(serde::de::Error::custom("Drive text exceeds 64 KiB"));
    }
    Ok(value)
}
fn bounded_chunk<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u32, D::Error> {
    let value = u32::deserialize(deserializer)?;
    if value == 0 || value > DRIVE_CHUNK_MAX_BYTES {
        return Err(serde::de::Error::custom(
            "Drive read size must be between 1 and 65536 bytes",
        ));
    }
    Ok(value)
}
fn optional_limit<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<u32>, D::Error> {
    let value = Option::<u32>::deserialize(deserializer)?;
    if value.is_some_and(|value| value == 0 || value > DRIVE_PAGE_MAX_LIMIT) {
        return Err(serde::de::Error::custom(
            "Drive page limit must be between 1 and 200",
        ));
    }
    Ok(value)
}
const fn default_read_size() -> u32 {
    DRIVE_CHUNK_MAX_BYTES
}
fn optional_search_limit<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<u32>, D::Error> {
    let value = Option::<u32>::deserialize(deserializer)?;
    if value.is_some_and(|value| value == 0 || value > DRIVE_SEARCH_MAX_LIMIT) {
        return Err(serde::de::Error::custom(
            "Drive search limit must be between 1 and 128",
        ));
    }
    Ok(value)
}

/// Numeric node IDs alone are not public references: the referenced Workspace
/// must match the route Workspace before the service resolves the node.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct DriveEntryRef {
    pub workspace_id: String,
    #[serde(deserialize_with = "decimal")]
    #[schemars(regex(pattern = "^[1-9][0-9]*$"))]
    pub node_id: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum DriveEntryKind {
    Folder,
    File,
}

/// Metadata only: content is returned through bounded text or binary routes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct DriveEntry {
    pub entry: DriveEntryRef,
    pub parent: Option<DriveEntryRef>,
    pub name: String,
    pub kind: DriveEntryKind,
    #[serde(deserialize_with = "node_mutation_id")]
    #[schemars(length(max = 128))]
    /// Request ID of the last committed mutation, or empty for the immutable root.
    pub last_mutation_id: String,
    /// File length in bytes; absent for folders.
    #[schemars(range(min = 0, max = 16_777_216))]
    pub size: Option<u32>,
    pub content_type: Option<String>,
    pub updated_by: String,
    /// Timestamp projected from storage's updated_at_ms; not a creation time.
    pub updated_at: String,
    /// Stable relative authenticated URL in this same Workspace. No bearer token
    /// or cross-Workspace link is embedded in this metadata.
    pub latest_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct DriveEntryQuery {
    pub entry_workspace_id: String,
    #[serde(deserialize_with = "decimal")]
    #[schemars(regex(pattern = "^[1-9][0-9]*$"))]
    pub id: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct DriveListQuery {
    pub entry_workspace_id: String,
    /// Folder to list.
    #[serde(deserialize_with = "decimal")]
    #[schemars(regex(pattern = "^[1-9][0-9]*$"))]
    pub id: String,
    #[serde(default, deserialize_with = "optional_limit")]
    #[schemars(range(min = 1, max = 200))]
    pub limit: Option<u32>,
    /// Opaque service-issued page cursor, not a bare node ID.
    #[serde(default)]
    pub after: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct DriveSearchQuery {
    pub query: String,
    /// Search file text as well as names only when explicitly requested.
    #[serde(default)]
    pub include_text: bool,
    #[serde(default, deserialize_with = "optional_search_limit")]
    #[schemars(range(min = 1, max = 128))]
    pub limit: Option<u32>,
    #[serde(default)]
    pub after: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct DriveListResponse {
    pub entries: Vec<DriveEntry>,
    pub next_after: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct DriveReadTextQuery {
    pub entry_workspace_id: String,
    #[serde(deserialize_with = "decimal")]
    #[schemars(regex(pattern = "^[1-9][0-9]*$"))]
    pub id: String,
    #[serde(default = "default_read_size", deserialize_with = "bounded_chunk")]
    #[schemars(range(min = 1, max = 65536))]
    pub max_bytes: u32,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct DriveReadTextResponse {
    pub entry: DriveEntry,
    #[serde(deserialize_with = "bounded_text")]
    pub text: String,
    pub truncated: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct DriveReadChunkQuery {
    pub entry_workspace_id: String,
    #[serde(deserialize_with = "decimal")]
    #[schemars(regex(pattern = "^[1-9][0-9]*$"))]
    pub id: String,
    /// Require each chunk to match the same committed mutation observed in metadata.
    #[serde(deserialize_with = "mutation_id")]
    #[schemars(length(max = 128))]
    pub expected_mutation_id: String,
    #[schemars(range(min = 0, max = 16_777_216))]
    pub offset: u32,
    #[serde(default = "default_read_size", deserialize_with = "bounded_chunk")]
    #[schemars(range(min = 1, max = 65536))]
    pub length: u32,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct DriveDownloadQuery {
    pub entry_workspace_id: String,
    #[serde(deserialize_with = "decimal")]
    #[schemars(regex(pattern = "^[1-9][0-9]*$"))]
    pub id: String,
    #[serde(default, deserialize_with = "optional_mutation_id")]
    pub expected_mutation_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct DriveMutationRequest {
    pub request_id: String,
    pub mutation: DriveMutation,
}
/// JSON can mutate bounded UTF-8 TEXT only. Arbitrary file bytes use drive_upload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum DriveMutation {
    CreateFolder {
        parent: DriveEntryRef,
        name: String,
    },
    CreateText {
        parent: DriveEntryRef,
        name: String,
        #[serde(deserialize_with = "bounded_text")]
        text: String,
        content_type: String,
    },
    UpdateText {
        id: DriveEntryRef,
        #[serde(deserialize_with = "mutation_id")]
        #[schemars(length(max = 128))]
        expected_mutation_id: String,
        #[serde(deserialize_with = "bounded_text")]
        text: String,
        content_type: String,
    },
    Relocate {
        id: DriveEntryRef,
        #[serde(deserialize_with = "mutation_id")]
        #[schemars(length(max = 128))]
        expected_mutation_id: String,
        parent: DriveEntryRef,
        name: String,
    },
    Delete {
        id: DriveEntryRef,
        #[serde(deserialize_with = "mutation_id")]
        #[schemars(length(max = 128))]
        expected_mutation_id: String,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct DriveMutationResponse {
    pub request_id: String,
    /// A deleted entry has no live metadata.
    pub entry: Option<DriveEntry>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum DriveUploadOperation {
    Create,
    Update,
}
/// Flat query metadata accompanies the binary body. The service rejects invalid
/// combinations: create requires parent_id/name and forbids id/expected_mutation_id;
/// update requires id/expected_mutation_id and forbids parent_id/name. Both targets
/// are bound by entry_workspace_id; declared size and SHA256 must match the body.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct DriveUploadQuery {
    pub operation: DriveUploadOperation,
    pub request_id: String,
    pub entry_workspace_id: String,
    #[serde(default, deserialize_with = "optional_decimal")]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default, deserialize_with = "optional_decimal")]
    pub id: Option<String>,
    #[serde(default, deserialize_with = "optional_mutation_id")]
    pub expected_mutation_id: Option<String>,
    pub content_type: String,
    /// Values above the file limit still decode as u32 so the service returns
    /// typed Limit rather than a generic request-decoding error.
    #[schemars(range(min = 0, max = 16_777_216))]
    pub size: u32,
    pub sha256: String,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum DriveRequestState {
    /// No commit record exists at the observation snapshot. An in-flight request
    /// can still commit afterward; this is not proof of a final failed outcome.
    Uncommitted,
    Committed,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct DriveRequestStatusResponse {
    pub request_id: String,
    pub state: DriveRequestState,
    pub response: Option<DriveMutationResponse>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum DriveAccess {
    ReadOnly,
    ReadWrite,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct DriveGrantCreateRequest {
    pub runtime_id: String,
    pub worker_id: String,
    pub access: DriveAccess,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct DriveGrantResponse {
    #[serde(deserialize_with = "decimal")]
    #[schemars(regex(pattern = "^[1-9][0-9]*$"))]
    pub grant_id: String,
    pub workspace_id: String,
    pub runtime_id: String,
    pub worker_id: String,
    pub access: DriveAccess,
    pub revoked: bool,
    pub created_by: String,
    pub created_at: String,
    pub revoked_by: Option<String>,
    pub revoked_at: Option<String>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct DriveGrantListQuery {
    #[serde(default, deserialize_with = "optional_limit")]
    #[schemars(range(min = 1, max = 200))]
    pub limit: Option<u32>,
    /// Canonical grant ID cursor within this Workspace.
    #[serde(default, deserialize_with = "optional_decimal")]
    pub after: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct DriveGrantListResponse {
    pub grants: Vec<DriveGrantResponse>,
    pub next_after: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum DriveApiErrorCode {
    Denied,
    NotFound,
    Conflict,
    Invalid,
    Limit,
    StorageUnavailable,
    OutcomeUnknown,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum DriveFailureClassification {
    NotCommitted,
    Unknown,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct DriveApiError {
    pub code: DriveApiErrorCode,
    pub classification: DriveFailureClassification,
    pub message: String,
}
impl DriveApiError {
    /// Never include provider errors, paths, SQL, or caller-supplied messages.
    pub fn new(code: DriveApiErrorCode) -> Self {
        use DriveApiErrorCode::*;
        Self {
            code,
            classification: if code == OutcomeUnknown {
                DriveFailureClassification::Unknown
            } else {
                DriveFailureClassification::NotCommitted
            },
            message: match code {
                Denied => "Drive access denied",
                NotFound => "Drive resource not found",
                Conflict => "Drive content, location, or name conflict",
                Invalid => "Invalid Drive request",
                Limit => "Drive request exceeds a limit",
                StorageUnavailable => "Drive storage unavailable",
                OutcomeUnknown => "Drive mutation outcome unknown",
            }
            .into(),
        }
    }
}
impl api_macros::HttpError for DriveApiError {
    fn status_code(&self) -> u16 {
        use DriveApiErrorCode::*;
        match self.code {
            Denied => 403,
            NotFound => 404,
            Conflict => 409,
            Invalid => 400,
            Limit => 413,
            StorageUnavailable | OutcomeUnknown => 503,
        }
    }
}
impl api_macros::HttpRequestError for DriveApiError {
    fn from_request_rejection(status: u16, _message: String) -> Self {
        Self::new(if status == 413 {
            DriveApiErrorCode::Limit
        } else {
            DriveApiErrorCode::Invalid
        })
    }
}

impl api_macros::openapi::OpenApiSchema for DriveApiError {}
impl api_macros::openapi::OpenApiSchema for DriveEntry {}
impl api_macros::openapi::OpenApiSchema for DriveEntryQuery {}
impl api_macros::openapi::OpenApiSchema for DriveListQuery {}
impl api_macros::openapi::OpenApiSchema for DriveSearchQuery {}
impl api_macros::openapi::OpenApiSchema for DriveListResponse {}
impl api_macros::openapi::OpenApiSchema for DriveReadTextQuery {}
impl api_macros::openapi::OpenApiSchema for DriveReadTextResponse {}
impl api_macros::openapi::OpenApiSchema for DriveReadChunkQuery {}
impl api_macros::openapi::OpenApiSchema for DriveDownloadQuery {}
impl api_macros::openapi::OpenApiSchema for DriveMutationRequest {}
impl api_macros::openapi::OpenApiSchema for DriveMutationResponse {}
impl api_macros::openapi::OpenApiSchema for DriveUploadQuery {}
impl api_macros::openapi::OpenApiSchema for DriveRequestStatusResponse {}
impl api_macros::openapi::OpenApiSchema for DriveGrantCreateRequest {}
impl api_macros::openapi::OpenApiSchema for DriveGrantResponse {}
impl api_macros::openapi::OpenApiSchema for DriveGrantListQuery {}
impl api_macros::openapi::OpenApiSchema for DriveGrantListResponse {}

/// Browser wire declarations are derived from the transport DTOs, not copied
/// from storage models or maintained independently in the Web client.
#[cfg(feature = "typescript")]
pub fn drive_api_typescript() -> String {
    use ts_rs::TS;
    let config = ts_rs::Config::default();
    let declarations = [
        DriveEntryRef::decl(&config),
        DriveEntryKind::decl(&config),
        DriveEntry::decl(&config),
        DriveEntryQuery::decl(&config),
        DriveListQuery::decl(&config),
        DriveSearchQuery::decl(&config),
        DriveListResponse::decl(&config),
        DriveReadTextQuery::decl(&config),
        DriveReadTextResponse::decl(&config),
        DriveReadChunkQuery::decl(&config),
        DriveDownloadQuery::decl(&config),
        DriveMutation::decl(&config),
        DriveMutationRequest::decl(&config),
        DriveMutationResponse::decl(&config),
        DriveUploadOperation::decl(&config),
        DriveUploadQuery::decl(&config),
        DriveRequestState::decl(&config),
        DriveRequestStatusResponse::decl(&config),
        DriveAccess::decl(&config),
        DriveGrantCreateRequest::decl(&config),
        DriveGrantResponse::decl(&config),
        DriveGrantListQuery::decl(&config),
        DriveGrantListResponse::decl(&config),
        DriveApiErrorCode::decl(&config),
        DriveFailureClassification::decl(&config),
        DriveApiError::decl(&config),
    ];
    format!(
        "// Generated from server-api. Do not edit by hand.\n// Regenerate: cargo run -q -p server-api --features typescript --example generate_drive_api_types > web/workspace/src/lib/generated/drive-api.ts\n\n{}\n",
        declarations
            .into_iter()
            .map(|declaration| format!("export {declaration}"))
            .collect::<Vec<_>>()
            .join("\n\n")
    )
}

#[cfg(all(test, feature = "typescript"))]
mod typescript_tests {
    #[test]
    fn generated_drive_api_contract_is_current() {
        let expected = super::drive_api_typescript();
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../web/workspace/src/lib/generated/drive-api.ts");
        let actual = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        assert_eq!(
            normalize(&actual),
            normalize(&expected),
            "regenerate Drive API TypeScript types with `cargo run -q -p server-api --features typescript --example generate_drive_api_types > web/workspace/src/lib/generated/drive-api.ts` and format the generated file"
        );
    }
    fn normalize(value: &str) -> String {
        value
            .chars()
            .filter_map(|character| match character {
                character if character.is_whitespace() => None,
                ',' => Some(';'),
                character => Some(character),
            })
            .collect::<String>()
            .replace("=|", "=")
            .replace(";}", "}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ApiContract, HttpMethod, ServerApiMetadata, canonical_openapi_document};
    use serde_json::json;

    #[test]
    fn drive_entry_references_reject_unbound_or_noncanonical_numeric_ids() {
        let valid = json!({"workspace_id":"workspace-a", "node_id":"9007199254740993"});
        let entry: DriveEntryRef = serde_json::from_value(valid.clone()).unwrap();
        assert_eq!(serde_json::to_value(entry).unwrap(), valid);
        for invalid in [
            json!("7"),
            json!(7),
            json!({"node_id":"7"}),
            json!({"workspace_id":"a", "node_id":7}),
        ] {
            assert!(serde_json::from_value::<DriveEntryRef>(invalid).is_err());
        }
        for id in [
            "",
            "0",
            "01",
            "-1",
            "+1",
            "1.0",
            "1e3",
            " 1",
            "١",
            "9223372036854775808",
            "18446744073709551616",
        ] {
            assert!(
                serde_json::from_value::<DriveEntryRef>(json!({"workspace_id":"a", "node_id":id}))
                    .is_err(),
                "accepted {id:?}"
            );
            assert!(
                serde_json::from_value::<DriveEntryQuery>(
                    json!({"entry_workspace_id":"a", "id":id})
                )
                .is_err()
            );
        }
        assert!(
            serde_json::from_value::<DriveEntryRef>(
                json!({"workspace_id":"a","node_id":"9223372036854775807"})
            )
            .is_ok()
        );
        assert!(serde_json::from_value::<DriveEntryQuery>(json!({"id":"7"})).is_err());
        assert!(serde_json::from_value::<DriveListQuery>(json!({"id":"7"})).is_err());
        assert!(serde_json::from_value::<DriveReadTextQuery>(json!({"id":"7"})).is_err());
        assert!(serde_json::from_value::<DriveDownloadQuery>(json!({"id":"7"})).is_err());
        assert!(
            serde_json::from_value::<DriveEntryQuery>(
                json!({"entry_workspace_id":"a", "id":"7", "worker_id":"spoofed"})
            )
            .is_err()
        );
    }

    #[test]
    fn drive_metadata_exposes_only_stored_update_provenance_and_same_workspace_url() {
        let value = json!({
            "entry":{"workspace_id":"a","node_id":"7"},
            "parent":{"workspace_id":"a","node_id":"1"},
            "name":"file.txt","kind":"file","last_mutation_id":"2","size":5,
            "content_type":"text/plain","updated_by":"user-a","updated_at":"2026-10-08T00:00:00Z",
            "latest_url":"/api/w/a/drive/download?entry_workspace_id=a&id=7"
        });
        let entry: DriveEntry = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(entry).unwrap(), value);
        for field in ["sha256", "created_at"] {
            let mut fabricated = value.clone();
            fabricated[field] = json!("not stored");
            assert!(serde_json::from_value::<DriveEntry>(fabricated).is_err());
        }
    }

    #[test]
    fn drive_search_text_is_opt_in_and_work_budget_is_bounded() {
        let query: DriveSearchQuery = serde_json::from_value(json!({"query":"name"})).unwrap();
        assert!(!query.include_text);
        let query: DriveSearchQuery =
            serde_json::from_value(json!({"query":"text","include_text":true,"limit":128}))
                .unwrap();
        assert!(query.include_text);
        for limit in [0, 129, u32::MAX] {
            assert!(
                serde_json::from_value::<DriveSearchQuery>(json!({"query":"name","limit":limit}))
                    .is_err()
            );
        }
    }

    #[test]
    fn drive_request_status_is_a_commit_snapshot_not_a_fictitious_lifecycle() {
        for state in ["uncommitted", "committed"] {
            let value = json!({"request_id":"r","state":state,"response":null});
            let response: DriveRequestStatusResponse =
                serde_json::from_value(value.clone()).unwrap();
            assert_eq!(serde_json::to_value(response).unwrap(), value);
        }
        for state in ["pending", "unknown", "not_committed"] {
            assert!(
                serde_json::from_value::<DriveRequestStatusResponse>(
                    json!({"request_id":"r","state":state,"response":null})
                )
                .is_err()
            );
        }
    }

    #[test]
    fn drive_json_mutations_accept_only_bounded_text_and_workspace_bound_cas() {
        let valid = json!({"request_id":"request-a", "mutation":{
            "operation":"update_text", "id":{"workspace_id":"a","node_id":"7"},
            "expected_mutation_id":"write-資料/one", "text":"hello", "content_type":"text/plain"
        }});
        let request: DriveMutationRequest = serde_json::from_value(valid.clone()).unwrap();
        assert_eq!(serde_json::to_value(request).unwrap(), valid);
        let mut invalid = valid.clone();
        invalid["mutation"]["bytes"] = json!([0, 255]);
        assert!(serde_json::from_value::<DriveMutationRequest>(invalid).is_err());
        let mut invalid = valid.clone();
        invalid["mutation"]["id"] = json!("7");
        assert!(serde_json::from_value::<DriveMutationRequest>(invalid).is_err());
        for bad in [
            "".to_string(),
            "\ncontrol".to_string(),
            "x".repeat(129),
            "é".repeat(65),
        ] {
            let mut invalid = valid.clone();
            invalid["mutation"]["expected_mutation_id"] = json!(bad);
            assert!(serde_json::from_value::<DriveMutationRequest>(invalid).is_err());
        }
        // UTF-8 byte count, not character count, determines the text bound.
        let mut boundary = valid;
        boundary["mutation"]["text"] = json!("é".repeat(DRIVE_TEXT_MAX_BYTES / 2));
        assert!(serde_json::from_value::<DriveMutationRequest>(boundary.clone()).is_ok());
        boundary["mutation"]["text"] = json!("é".repeat(DRIVE_TEXT_MAX_BYTES / 2 + 1));
        assert!(serde_json::from_value::<DriveMutationRequest>(boundary).is_err());
    }

    #[test]
    fn drive_reads_reject_zero_or_over_64kib_requests() {
        for size in [0, 65537, u32::MAX] {
            assert!(
                serde_json::from_value::<DriveReadTextQuery>(
                    json!({"entry_workspace_id":"a","id":"7","max_bytes":size})
                )
                .is_err()
            );
            assert!(serde_json::from_value::<DriveReadChunkQuery>(json!({"entry_workspace_id":"a","id":"7","expected_mutation_id":"1","offset":0,"length":size})).is_err());
        }
        let default: DriveReadTextQuery =
            serde_json::from_value(json!({"entry_workspace_id":"a","id":"7"})).unwrap();
        assert_eq!(default.max_bytes, 65536);
        let chunk: DriveReadChunkQuery = serde_json::from_value(json!({"entry_workspace_id":"a","id":"7","expected_mutation_id":"1","offset":0,"length":65536})).unwrap();
        assert_eq!(chunk.length, 65536);
    }

    #[test]
    fn drive_upload_requires_binding_declared_integrity_and_request_identity() {
        let valid = json!({"operation":"update","request_id":"request-a","entry_workspace_id":"a","id":"7","expected_mutation_id":"2","content_type":"application/octet-stream","size":3,"sha256":"a".repeat(64)});
        assert!(serde_json::from_value::<DriveUploadQuery>(valid.clone()).is_ok());
        for field in ["entry_workspace_id", "size", "sha256", "request_id"] {
            let mut missing = valid.clone();
            missing.as_object_mut().unwrap().remove(field);
            assert!(
                serde_json::from_value::<DriveUploadQuery>(missing).is_err(),
                "accepted missing {field}"
            );
        }
        for size in [DRIVE_FILE_MAX_BYTES, DRIVE_FILE_MAX_BYTES + 1, u32::MAX] {
            let mut oversized = valid.clone();
            oversized["size"] = json!(size);
            let query: DriveUploadQuery = serde_json::from_value(oversized).unwrap();
            assert_eq!(
                query.size, size,
                "service must receive oversized declared sizes for typed Limit classification"
            );
        }
        assert!(serde_json::from_value::<DriveReadChunkQuery>(json!({"entry_workspace_id":"a","id":"7","expected_mutation_id":"1","offset":DRIVE_FILE_MAX_BYTES,"length":1})).is_ok());
        let mut invalid = valid;
        invalid["bytes"] = json!([0, 255]);
        assert!(serde_json::from_value::<DriveUploadQuery>(invalid).is_err());
    }

    #[test]
    fn drive_grants_reject_command_access_and_unbounded_pages() {
        let grant = json!({"runtime_id":"r","worker_id":"w","access":"read_write"});
        assert_eq!(
            serde_json::to_value(
                serde_json::from_value::<DriveGrantCreateRequest>(grant.clone()).unwrap()
            )
            .unwrap(),
            grant
        );
        assert!(
            serde_json::from_value::<DriveGrantCreateRequest>(
                json!({"runtime_id":"r","worker_id":"w","access":"read_write_command"})
            )
            .is_err()
        );
        assert!(serde_json::from_value::<DriveGrantCreateRequest>(json!({"runtime_id":"r","worker_id":"w","access":"read_only","workspace_id":"spoofed"})).is_err());
        for limit in [0, 201, u32::MAX] {
            assert!(serde_json::from_value::<DriveGrantListQuery>(json!({"limit":limit})).is_err());
        }
        assert!(
            serde_json::from_value::<DriveGrantListQuery>(json!({"limit":200,"after":"7"})).is_ok()
        );
        assert!(serde_json::from_value::<DriveGrantListQuery>(json!({"after":"01"})).is_err());
    }

    #[test]
    fn drive_errors_have_fixed_safe_messages_and_typed_outcome_classification() {
        use DriveApiErrorCode::*;
        for (code, wire, status, message) in [
            (Denied, "denied", 403, "Drive access denied"),
            (NotFound, "not_found", 404, "Drive resource not found"),
            (
                Conflict,
                "conflict",
                409,
                "Drive content, location, or name conflict",
            ),
            (Invalid, "invalid", 400, "Invalid Drive request"),
            (Limit, "limit", 413, "Drive request exceeds a limit"),
            (
                StorageUnavailable,
                "storage_unavailable",
                503,
                "Drive storage unavailable",
            ),
            (
                OutcomeUnknown,
                "outcome_unknown",
                503,
                "Drive mutation outcome unknown",
            ),
        ] {
            let error = DriveApiError::new(code);
            assert_eq!(api_macros::HttpError::status_code(&error), status);
            assert_eq!(
                serde_json::to_value(&error).unwrap(),
                json!({"code":wire,"classification":if code == OutcomeUnknown {"unknown"} else {"not_committed"},"message":message})
            );
        }
        for status in [400, 413, 415, 422] {
            let error = <DriveApiError as api_macros::HttpRequestError>::from_request_rejection(
                status,
                "/private/path: SQL failed".into(),
            );
            assert_eq!(
                error,
                DriveApiError::new(if status == 413 { Limit } else { Invalid })
            );
        }
        assert!(serde_json::from_value::<DriveApiError>(json!({"code":"invalid","classification":"not_committed","message":"Invalid Drive request","provider_error":"secret"})).is_err());
    }

    #[test]
    fn drive_routes_are_workspace_scoped_authenticated_and_binary_is_not_json() {
        let document = canonical_openapi_document().unwrap();
        let document: serde_json::Value =
            serde_json::from_str(&document.to_json().unwrap()).unwrap();
        let schemas = &document["components"]["schemas"];
        assert_eq!(
            schemas["DriveEntry"]["properties"]["size"]["maximum"],
            DRIVE_FILE_MAX_BYTES
        );
        assert_eq!(
            schemas["DriveReadChunkQuery"]["properties"]["offset"]["maximum"],
            DRIVE_FILE_MAX_BYTES
        );
        assert_eq!(
            schemas["DriveUploadQuery"]["properties"]["size"]["maximum"],
            DRIVE_FILE_MAX_BYTES
        );
        assert_eq!(
            schemas["DriveSearchQuery"]["properties"]["limit"]["maximum"],
            DRIVE_SEARCH_MAX_LIMIT
        );
        for (name, method, suffix, wire_method) in [
            ("drive_root", HttpMethod::Get, "/root", "get"),
            ("drive_metadata", HttpMethod::Get, "/metadata", "get"),
            ("drive_list", HttpMethod::Get, "/list", "get"),
            ("drive_search", HttpMethod::Get, "/search", "get"),
            ("drive_read_text", HttpMethod::Get, "/read-text", "get"),
            ("drive_read_chunk", HttpMethod::Get, "/read-chunk", "get"),
            ("drive_download", HttpMethod::Get, "/download", "get"),
            ("drive_mutate", HttpMethod::Post, "/mutate", "post"),
            ("drive_upload", HttpMethod::Put, "/upload", "put"),
            (
                "drive_request_status",
                HttpMethod::Get,
                "/requests/{request_id}",
                "get",
            ),
            ("drive_grant_create", HttpMethod::Post, "/grants", "post"),
            (
                "drive_grant_revoke",
                HttpMethod::Delete,
                "/grants/{grant_id}",
                "delete",
            ),
            ("drive_grant_list", HttpMethod::Get, "/grants", "get"),
        ] {
            let operation = ServerApiMetadata::OPERATIONS
                .iter()
                .find(|operation| operation.operation_id == name)
                .unwrap();
            let path = format!("/api/w/{{workspace_id}}/drive{suffix}");
            assert_eq!(operation.method, method);
            assert_eq!(operation.path, path);
            let documented = &document["paths"][&path][wire_method];
            assert_eq!(documented["operationId"], name);
            assert_eq!(
                documented["security"],
                json!([{"bearerAuth":[]},{"browserSession":[]}])
            );
            // Server extensions must not become caller-controlled parameters.
            assert!(
                documented["parameters"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|parameter| parameter["name"] != "context")
            );
            if matches!(name, "drive_read_chunk" | "drive_download") {
                let content = &documented["responses"]["200"]["content"];
                assert!(content["application/octet-stream"].is_object());
                assert!(content["application/json"].is_null());
            }
            if name == "drive_download" {
                let headers = documented["responses"]["200"]["headers"]
                    .as_object()
                    .unwrap();
                assert_eq!(headers.len(), 5);
                for header in [
                    "content-disposition",
                    "cache-control",
                    "etag",
                    "x-content-type-options",
                    "content-security-policy",
                ] {
                    assert_eq!(
                        headers[header]["schema"]["$ref"],
                        "#/components/schemas/string"
                    );
                }
                assert!(!headers.contains_key("content-type"));
                assert!(!headers.contains_key("content-length"));
            }
            if name == "drive_upload" {
                let content = &documented["requestBody"]["content"];
                assert!(content["application/octet-stream"].is_object());
                assert!(content["application/json"].is_null());
            }
        }
    }
}
