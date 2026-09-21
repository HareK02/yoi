//! Versioned websocket DTOs for a client-hosted External Workdir provider.
//!
//! Authentication belongs to the websocket upgrade and Backend grant
//! authority. These frames intentionally contain only opaque identities,
//! logical Workdir operations, generation fences, and bounded control data.
//! They must never carry a provider host path, credential, or tunnel token.

use std::path::{Component, Path};

use serde::{Deserialize, Serialize};

use crate::http::{WorkdirSessionOperation, WorkdirSessionOperationResult, WorkdirTransportError};
use crate::{BoundedReadLimits, WorkdirId, WorkdirPath, WorkdirSessionCapabilities};

pub const EXTERNAL_WORKDIR_PROVIDER_PROTOCOL_VERSION: u16 = 1;
/// Maximum serialized Backend-to-provider command frame size.
pub const MAX_EXTERNAL_SERVER_FRAME_BYTES: usize = 1024 * 1024;
/// Maximum serialized provider-to-Backend result frame size. Read bytes are
/// represented as a JSON byte array, so this must exceed the semantic payload
/// bound while remaining an explicit transport allocation ceiling.
pub const MAX_EXTERNAL_PROVIDER_FRAME_BYTES: usize = 8 * 1024 * 1024;
const MAX_PROTOCOL_ID_BYTES: usize = 128;
const MAX_EXTERNAL_PATH_BYTES: usize = 4 * 1024;
const MAX_EXTERNAL_PATTERN_BYTES: usize = 4 * 1024;
const MAX_EXTERNAL_RESULT_ITEMS: usize = 10_000;
const MAX_EXTERNAL_GREP_CONTEXT: usize = 100;
const MAX_EXTERNAL_SCOPE_RULES: usize = 64;

/// Fail-closed wire version for the External Workdir provider protocol.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct ExternalWorkdirProtocolVersion(u16);

impl ExternalWorkdirProtocolVersion {
    pub const CURRENT: Self = Self(EXTERNAL_WORKDIR_PROVIDER_PROTOCOL_VERSION);

    pub const fn as_u16(self) -> u16 {
        self.0
    }
}

impl<'de> Deserialize<'de> for ExternalWorkdirProtocolVersion {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let version = u16::deserialize(deserializer)?;
        if version == EXTERNAL_WORKDIR_PROVIDER_PROTOCOL_VERSION {
            Ok(Self::CURRENT)
        } else {
            Err(serde::de::Error::custom(format!(
                "unsupported External Workdir provider protocol version {version}"
            )))
        }
    }
}

macro_rules! protocol_id {
    ($name:ident, $label:literal) => {
        #[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, String> {
                let value = value.into();
                if value.is_empty()
                    || value.len() > MAX_PROTOCOL_ID_BYTES
                    || value.trim() != value
                    || value.chars().any(char::is_control)
                {
                    return Err(concat!($label, " is invalid").to_string());
                }
                Ok(Self(value))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                Self::new(value).map_err(serde::de::Error::custom)
            }
        }
    };
}

protocol_id!(ExternalProviderInstanceId, "provider instance id");
protocol_id!(ExternalWorkdirGrantId, "External Workdir grant id");
protocol_id!(ExternalWorkdirOperationId, "External Workdir operation id");

/// First provider frame on a newly authenticated outbound websocket.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalWorkdirProviderRegistration {
    pub provider_instance_id: ExternalProviderInstanceId,
    pub grant_id: ExternalWorkdirGrantId,
    pub workdir_id: WorkdirId,
    /// Monotonic grant connection generation. Reconnect advances it and fences
    /// every frame from an older connection.
    pub generation: u64,
    pub capabilities: WorkdirSessionCapabilities,
    pub read_limits: BoundedReadLimits,
}

fn is_root_relative(path: &WorkdirPath) -> bool {
    let path_value = path.as_str();
    let path = Path::new(path_value);
    path_value.len() <= MAX_EXTERNAL_PATH_BYTES
        && !path.is_absolute()
        && !path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
}

fn validate_read_only_operation(operation: &WorkdirSessionOperation) -> Result<(), String> {
    let path_is_valid = match operation {
        WorkdirSessionOperation::AuthorizeScope(request) => {
            is_root_relative(&request.path)
                && request.rules.len() <= MAX_EXTERNAL_SCOPE_RULES
                && request
                    .rules
                    .iter()
                    .all(|rule| is_root_relative(&rule.target))
        }
        WorkdirSessionOperation::ScopeRulesOverlap(request) => {
            is_root_relative(&request.left.target) && is_root_relative(&request.right.target)
        }
        WorkdirSessionOperation::Stat(request) => is_root_relative(&request.path),
        WorkdirSessionOperation::Read(request) => {
            is_root_relative(&request.path)
                && request.limit <= 1_000_000
                && request.max_bytes <= BoundedReadLimits::EXTERNAL_DEFAULT.max_response_bytes
        }
        WorkdirSessionOperation::List(request) => {
            is_root_relative(&request.path) && request.limit <= MAX_EXTERNAL_RESULT_ITEMS
        }
        WorkdirSessionOperation::Glob(request) => {
            is_root_relative(&request.path)
                && request.pattern.len() <= MAX_EXTERNAL_PATTERN_BYTES
                && request.limit <= MAX_EXTERNAL_RESULT_ITEMS
        }
        WorkdirSessionOperation::Grep(request) => {
            is_root_relative(&request.path)
                && request.pattern.len() <= MAX_EXTERNAL_PATTERN_BYTES
                && request
                    .glob
                    .as_ref()
                    .is_none_or(|glob| glob.len() <= MAX_EXTERNAL_PATTERN_BYTES)
                && request
                    .file_type
                    .as_ref()
                    .is_none_or(|file_type| file_type.len() <= 128)
                && request.before_context <= MAX_EXTERNAL_GREP_CONTEXT
                && request.after_context <= MAX_EXTERNAL_GREP_CONTEXT
                && request.limit <= MAX_EXTERNAL_RESULT_ITEMS
                && request.offset <= 1_000_000
        }
        WorkdirSessionOperation::Write(_)
        | WorkdirSessionOperation::Edit(_)
        | WorkdirSessionOperation::CommandStart(_)
        | WorkdirSessionOperation::CommandStatus(_)
        | WorkdirSessionOperation::CommandOutput(_)
        | WorkdirSessionOperation::CommandCancel(_) => {
            return Err("operation is not available to a read-only External Workdir".to_string());
        }
    };
    if path_is_valid {
        Ok(())
    } else {
        Err("External Workdir operation paths must be root-relative".to_string())
    }
}

/// Validated read-only subset of the shared Workdir operation contract.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct ExternalWorkdirOperation(WorkdirSessionOperation);

impl ExternalWorkdirOperation {
    pub fn into_inner(self) -> WorkdirSessionOperation {
        self.0
    }

    pub fn as_inner(&self) -> &WorkdirSessionOperation {
        &self.0
    }
}

impl TryFrom<WorkdirSessionOperation> for ExternalWorkdirOperation {
    type Error = String;

    fn try_from(operation: WorkdirSessionOperation) -> Result<Self, Self::Error> {
        validate_read_only_operation(&operation)?;
        Ok(Self(operation))
    }
}

impl<'de> Deserialize<'de> for ExternalWorkdirOperation {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let operation = WorkdirSessionOperation::deserialize(deserializer)?;
        Self::try_from(operation).map_err(serde::de::Error::custom)
    }
}

fn result_paths_fit<'a>(paths: impl IntoIterator<Item = &'a WorkdirPath>) -> bool {
    paths
        .into_iter()
        .try_fold(0_usize, |retained, path| {
            retained.checked_add(path.as_str().len().saturating_add(64))
        })
        .is_some_and(|retained| retained <= fs_operation::MAX_RESULT_PATH_BYTES)
}

fn validate_read_only_result(result: &WorkdirSessionOperationResult) -> Result<(), String> {
    let result_is_valid = match result {
        WorkdirSessionOperationResult::AuthorizeScope
        | WorkdirSessionOperationResult::ScopeRulesOverlap { .. } => true,
        WorkdirSessionOperationResult::Stat(result) => is_root_relative(&result.path),
        WorkdirSessionOperationResult::Read(result) => {
            is_root_relative(&result.path)
                && result.bytes.len() <= BoundedReadLimits::EXTERNAL_DEFAULT.max_response_bytes
        }
        WorkdirSessionOperationResult::List(result) => {
            result.entries.len() <= MAX_EXTERNAL_RESULT_ITEMS
                && result
                    .entries
                    .iter()
                    .all(|entry| is_root_relative(&entry.path))
                && result_paths_fit(result.entries.iter().map(|entry| &entry.path))
        }
        WorkdirSessionOperationResult::Glob(result) => {
            result.paths.len() <= MAX_EXTERNAL_RESULT_ITEMS
                && result.paths.iter().all(is_root_relative)
                && result_paths_fit(&result.paths)
        }
        WorkdirSessionOperationResult::Grep(result) => {
            result.output.len() <= BoundedReadLimits::EXTERNAL_DEFAULT.max_response_bytes
                && result.match_count <= MAX_EXTERNAL_RESULT_ITEMS
                && result.matched_files <= MAX_EXTERNAL_RESULT_ITEMS
        }
        WorkdirSessionOperationResult::Write(_)
        | WorkdirSessionOperationResult::Edit(_)
        | WorkdirSessionOperationResult::CommandStart(_)
        | WorkdirSessionOperationResult::CommandStatus(_)
        | WorkdirSessionOperationResult::CommandOutput(_)
        | WorkdirSessionOperationResult::CommandCancel => {
            return Err(
                "operation result is not available to a read-only External Workdir".to_string(),
            );
        }
    };
    if result_is_valid {
        Ok(())
    } else {
        Err("External Workdir result exceeds path or response bounds".to_string())
    }
}

/// Validated read-only subset of the shared Workdir operation result contract.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct ExternalWorkdirOperationResult(WorkdirSessionOperationResult);

impl ExternalWorkdirOperationResult {
    pub fn into_inner(self) -> WorkdirSessionOperationResult {
        self.0
    }

    pub fn as_inner(&self) -> &WorkdirSessionOperationResult {
        &self.0
    }
}

impl TryFrom<WorkdirSessionOperationResult> for ExternalWorkdirOperationResult {
    type Error = String;

    fn try_from(result: WorkdirSessionOperationResult) -> Result<Self, Self::Error> {
        validate_read_only_result(&result)?;
        Ok(Self(result))
    }
}

impl<'de> Deserialize<'de> for ExternalWorkdirOperationResult {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let result = WorkdirSessionOperationResult::deserialize(deserializer)?;
        Self::try_from(result).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExternalWorkdirOperationOutcome {
    Completed {
        result: ExternalWorkdirOperationResult,
    },
    Failed {
        error: WorkdirTransportError,
    },
    Cancelled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExternalWorkdirRevokeReason {
    UserRequested,
    Expired,
    AuthorityWithdrawn,
}

/// Frames sent by the authenticated Backend to the outbound provider.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExternalWorkdirServerMessage {
    Registered {
        generation: u64,
        heartbeat_interval_ms: u64,
        max_in_flight_operations: usize,
    },
    Operation {
        generation: u64,
        operation_id: ExternalWorkdirOperationId,
        operation: ExternalWorkdirOperation,
    },
    Cancel {
        generation: u64,
        operation_id: ExternalWorkdirOperationId,
    },
    Revoke {
        generation: u64,
        reason: ExternalWorkdirRevokeReason,
    },
    Heartbeat {
        generation: u64,
        sequence: u64,
    },
}

/// Frames sent by the client-hosted provider over its outbound connection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExternalWorkdirProviderMessage {
    Register {
        registration: ExternalWorkdirProviderRegistration,
    },
    OperationResult {
        generation: u64,
        operation_id: ExternalWorkdirOperationId,
        outcome: ExternalWorkdirOperationOutcome,
    },
    Heartbeat {
        generation: u64,
        sequence: u64,
    },
    RevokeAcknowledged {
        generation: u64,
    },
}

/// Versioned server-to-provider websocket envelope.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalWorkdirServerFrame {
    pub version: ExternalWorkdirProtocolVersion,
    pub message: ExternalWorkdirServerMessage,
}

impl ExternalWorkdirServerFrame {
    pub fn current(message: ExternalWorkdirServerMessage) -> Self {
        Self {
            version: ExternalWorkdirProtocolVersion::CURRENT,
            message,
        }
    }
}

/// Versioned provider-to-server websocket envelope.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalWorkdirProviderFrame {
    pub version: ExternalWorkdirProtocolVersion,
    pub message: ExternalWorkdirProviderMessage,
}

impl ExternalWorkdirProviderFrame {
    pub fn current(message: ExternalWorkdirProviderMessage) -> Self {
        Self {
            version: ExternalWorkdirProtocolVersion::CURRENT,
            message,
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;
    use crate::{CommandRequest, GrepResult, ListRequest, ReadRequest, Workdir, WorkdirPath};

    fn assert_no_provider_authority(value: &Value) {
        match value {
            Value::Object(object) => {
                for (key, value) in object {
                    assert!(
                        ![
                            "host_path",
                            "root",
                            "credential",
                            "credentials",
                            "token",
                            "access_token",
                            "bearer_token",
                            "endpoint",
                            "url",
                        ]
                        .contains(&key.as_str()),
                        "provider authority field leaked into protocol: {key}"
                    );
                    assert_no_provider_authority(value);
                }
            }
            Value::Array(values) => values.iter().for_each(assert_no_provider_authority),
            Value::String(value) => {
                assert!(!value.contains("/home/operator/private"));
                assert!(!value.contains("provider-secret"));
            }
            _ => {}
        }
    }

    #[test]
    fn protocol_is_versioned_correlated_and_contains_no_provider_authority() {
        let workdir = Workdir::new("working-directory-1");
        let registration =
            ExternalWorkdirProviderFrame::current(ExternalWorkdirProviderMessage::Register {
                registration: ExternalWorkdirProviderRegistration {
                    provider_instance_id: ExternalProviderInstanceId::new("provider-1").unwrap(),
                    grant_id: ExternalWorkdirGrantId::new("grant-1").unwrap(),
                    workdir_id: workdir.id().clone(),
                    generation: 7,
                    capabilities: WorkdirSessionCapabilities::READ_ONLY,
                    read_limits: BoundedReadLimits::new(4096, 1024).unwrap(),
                },
            });
        let operation =
            ExternalWorkdirServerFrame::current(ExternalWorkdirServerMessage::Operation {
                generation: 7,
                operation_id: ExternalWorkdirOperationId::new("operation-1").unwrap(),
                operation: WorkdirSessionOperation::Read(ReadRequest {
                    path: WorkdirPath::new("sessions/events.jsonl").unwrap(),
                    offset: 0,
                    limit: 20,
                    max_bytes: 1024,
                })
                .try_into()
                .unwrap(),
            });

        for value in [
            serde_json::to_value(registration).unwrap(),
            serde_json::to_value(operation).unwrap(),
            serde_json::to_value(ExternalWorkdirServerFrame::current(
                ExternalWorkdirServerMessage::Cancel {
                    generation: 7,
                    operation_id: ExternalWorkdirOperationId::new("operation-1").unwrap(),
                },
            ))
            .unwrap(),
            serde_json::to_value(ExternalWorkdirServerFrame::current(
                ExternalWorkdirServerMessage::Revoke {
                    generation: 7,
                    reason: ExternalWorkdirRevokeReason::Expired,
                },
            ))
            .unwrap(),
            serde_json::to_value(ExternalWorkdirProviderFrame::current(
                ExternalWorkdirProviderMessage::Heartbeat {
                    generation: 7,
                    sequence: 3,
                },
            ))
            .unwrap(),
        ] {
            assert_eq!(
                value.get("version").and_then(Value::as_u64),
                Some(EXTERNAL_WORKDIR_PROVIDER_PROTOCOL_VERSION.into())
            );
            assert_no_provider_authority(&value);
        }
    }

    #[test]
    fn unsupported_protocol_versions_and_invalid_read_limits_fail_closed() {
        let future = r#"{
            "version": 2,
            "message": {"kind": "heartbeat", "generation": 1, "sequence": 1}
        }"#;
        assert!(serde_json::from_str::<ExternalWorkdirServerFrame>(future).is_err());

        let invalid_limits = r#"{
            "max_source_bytes": 1024,
            "max_response_bytes": 2048
        }"#;
        assert!(serde_json::from_str::<BoundedReadLimits>(invalid_limits).is_err());
    }

    #[test]
    fn read_only_protocol_rejects_host_paths_and_command_payloads() {
        let absolute = WorkdirSessionOperation::Read(ReadRequest {
            path: WorkdirPath::new_scoped("/home/operator/private/session.log").unwrap(),
            offset: 0,
            limit: 1,
            max_bytes: 1024,
        });
        assert!(ExternalWorkdirOperation::try_from(absolute).is_err());

        let command = WorkdirSessionOperation::CommandStart(CommandRequest {
            command: "cat /home/operator/private/session.log".to_string(),
            timeout_secs: 1,
            output_limit: 1024,
            cwd: None,
            spill_dir: Some("/home/operator/private/output".into()),
            tool_call_id: None,
        });
        assert!(ExternalWorkdirOperation::try_from(command).is_err());
    }

    #[test]
    fn protocol_rejects_unknown_nested_fields_and_unbounded_requests() {
        let unknown = r#"{
            "version": 1,
            "message": {
                "kind": "heartbeat",
                "generation": 1,
                "sequence": 1,
                "credential": "must-not-be-ignored"
            }
        }"#;
        assert!(serde_json::from_str::<ExternalWorkdirServerFrame>(unknown).is_err());

        let mut nested = serde_json::to_value(ExternalWorkdirServerFrame::current(
            ExternalWorkdirServerMessage::Operation {
                generation: 1,
                operation_id: ExternalWorkdirOperationId::new("operation-1").unwrap(),
                operation: WorkdirSessionOperation::Read(ReadRequest {
                    path: WorkdirPath::new("events.jsonl").unwrap(),
                    offset: 0,
                    limit: 1,
                    max_bytes: 1024,
                })
                .try_into()
                .unwrap(),
            },
        ))
        .unwrap();
        nested["message"]["operation"]["request"]["credential"] =
            Value::String("must-not-be-ignored".to_string());
        assert!(serde_json::from_value::<ExternalWorkdirServerFrame>(nested).is_err());

        let unbounded = WorkdirSessionOperation::List(ListRequest {
            path: WorkdirPath::root(),
            limit: MAX_EXTERNAL_RESULT_ITEMS + 1,
        });
        assert!(ExternalWorkdirOperation::try_from(unbounded).is_err());

        let oversized_result = WorkdirSessionOperationResult::Grep(GrepResult {
            output: "x".repeat(BoundedReadLimits::EXTERNAL_DEFAULT.max_response_bytes + 1),
            match_count: 1,
            matched_files: 1,
            truncated: true,
        });
        assert!(ExternalWorkdirOperationResult::try_from(oversized_result).is_err());
    }

    #[test]
    fn protocol_ids_are_bounded_and_control_free() {
        assert!(ExternalWorkdirOperationId::new("").is_err());
        assert!(ExternalWorkdirOperationId::new(" operation-1").is_err());
        assert!(ExternalWorkdirOperationId::new("operation\n1").is_err());
        assert!(ExternalWorkdirOperationId::new("x".repeat(129)).is_err());
        assert!(ExternalWorkdirOperationId::new("operation-1").is_ok());
    }
}
