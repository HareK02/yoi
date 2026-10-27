use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct CommandHandle(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CommandRequest {
    pub command: String,
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_u64))]
    pub timeout_secs: u64,
    /// Aggregate unique-byte capture allowance, independent of consumer storage.
    /// Providers drain and count excess bytes without delivering their payload.
    #[schemars(range(min = 0, max = 67_108_864_u64))]
    pub transport_byte_budget: u64,
    /// Workdir-relative command directory. Providers validate it against the
    /// active session before process start.
    pub cwd: fs_operation::FsPath,
    /// Optional caller-owned correlation id. Bash supplies its tool-call id so
    /// user-facing command telemetry can update the corresponding Console row
    /// without exposing provider/session handles.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl CommandRequest {
    pub fn validate(&self) -> Result<(), crate::WorkdirError> {
        if self.transport_byte_budget > MAX_COMMAND_TRANSPORT_BYTES {
            return Err(crate::WorkdirError::InvalidArgument(
                "command transport byte budget exceeds provider maximum".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CommandOutputRequest {
    pub handle: CommandHandle,
    #[schemars(range(min = 0, max = 67_108_864_u64))]
    pub cursor: u64,
    #[schemars(range(min = 1, max = 65_536_usize))]
    pub limit: usize,
    pub wait: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CommandStatus {
    Running,
    Completed,
    Failed,
    TimedOut,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CommandStream {
    Stdout,
    Stderr,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
pub struct CommandStreamSlice {
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_u64))]
    pub start_offset: u64,
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_u64))]
    pub end_offset: u64,
    pub content: String,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CommandSnapshot {
    pub command_id: String,
    pub tool_call_id: Option<String>,
    pub status: CommandStatus,
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_u64))]
    pub started_at_ms: u64,
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_u64))]
    pub observed_at_ms: u64,
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_u64))]
    pub last_output_at_ms: Option<u64>,
    pub stdout: CommandStreamSlice,
    pub stderr: CommandStreamSlice,
    #[schemars(range(min = -2147483648_i32, max = 2147483647_i32))]
    pub exit_code: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CommandEvent {
    Started {
        command_id: String,
        tool_call_id: Option<String>,
        observed_at_ms: u64,
    },
    Output {
        command_id: String,
        stream: CommandStream,
        start_offset: u64,
        end_offset: u64,
        content: String,
        observed_at_ms: u64,
    },
    Terminal {
        command_id: String,
        status: CommandStatus,
        exit_code: Option<i32>,
        stdout_end_offset: u64,
        stderr_end_offset: u64,
        observed_at_ms: u64,
    },
}

/// Sequence is the merged transport byte position. Offset addresses raw bytes
/// within the named stream; neither ordering nor offsets depend on UTF-8 decoding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CommandByteChunk {
    pub sequence: u64,
    pub stream: CommandStream,
    pub offset: u64,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CommandOutputTruncation {
    ByteBudget,
}

/// Closed diagnostic vocabulary: never serialize OS messages, paths, or errno text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CommandIoErrorKind {
    NotFound,
    PermissionDenied,
    ConnectionRefused,
    ConnectionReset,
    ConnectionAborted,
    NotConnected,
    BrokenPipe,
    AlreadyExists,
    WouldBlock,
    InvalidInput,
    InvalidData,
    TimedOut,
    WriteZero,
    Interrupted,
    UnexpectedEof,
    OutOfMemory,
    HostUnreachable,
    NetworkUnreachable,
    AddrInUse,
    AddrNotAvailable,
    NetworkDown,
    NotADirectory,
    IsADirectory,
    DirectoryNotEmpty,
    ReadOnlyFilesystem,
    StaleNetworkFileHandle,
    StorageFull,
    NotSeekable,
    QuotaExceeded,
    FileTooLarge,
    ResourceBusy,
    ExecutableFileBusy,
    Deadlock,
    CrossesDevices,
    TooManyLinks,
    InvalidFilename,
    ArgumentListTooLong,
    Unsupported,
    Other,
    Unknown,
}

impl From<std::io::ErrorKind> for CommandIoErrorKind {
    fn from(kind: std::io::ErrorKind) -> Self {
        use std::io::ErrorKind as Io;
        match kind {
            Io::NotFound => Self::NotFound,
            Io::PermissionDenied => Self::PermissionDenied,
            Io::ConnectionRefused => Self::ConnectionRefused,
            Io::ConnectionReset => Self::ConnectionReset,
            Io::ConnectionAborted => Self::ConnectionAborted,
            Io::NotConnected => Self::NotConnected,
            Io::BrokenPipe => Self::BrokenPipe,
            Io::AlreadyExists => Self::AlreadyExists,
            Io::WouldBlock => Self::WouldBlock,
            Io::InvalidInput => Self::InvalidInput,
            Io::InvalidData => Self::InvalidData,
            Io::TimedOut => Self::TimedOut,
            Io::WriteZero => Self::WriteZero,
            Io::Interrupted => Self::Interrupted,
            Io::UnexpectedEof => Self::UnexpectedEof,
            Io::OutOfMemory => Self::OutOfMemory,
            Io::HostUnreachable => Self::HostUnreachable,
            Io::NetworkUnreachable => Self::NetworkUnreachable,
            Io::AddrInUse => Self::AddrInUse,
            Io::AddrNotAvailable => Self::AddrNotAvailable,
            Io::NetworkDown => Self::NetworkDown,
            Io::NotADirectory => Self::NotADirectory,
            Io::IsADirectory => Self::IsADirectory,
            Io::DirectoryNotEmpty => Self::DirectoryNotEmpty,
            Io::ReadOnlyFilesystem => Self::ReadOnlyFilesystem,
            Io::StaleNetworkFileHandle => Self::StaleNetworkFileHandle,
            Io::StorageFull => Self::StorageFull,
            Io::NotSeekable => Self::NotSeekable,
            Io::QuotaExceeded => Self::QuotaExceeded,
            Io::FileTooLarge => Self::FileTooLarge,
            Io::ResourceBusy => Self::ResourceBusy,
            Io::ExecutableFileBusy => Self::ExecutableFileBusy,
            Io::Deadlock => Self::Deadlock,
            Io::CrossesDevices => Self::CrossesDevices,
            Io::TooManyLinks => Self::TooManyLinks,
            Io::InvalidFilename => Self::InvalidFilename,
            Io::ArgumentListTooLong => Self::ArgumentListTooLong,
            Io::Unsupported => Self::Unsupported,
            Io::Other => Self::Other,
            _ => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CommandFailureStage {
    Spawn,
    Wait,
    StdoutRead,
    StderrRead,
    StdoutDrain,
    StderrDrain,
    OutputDelivery,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CommandFailure {
    pub stage: CommandFailureStage,
    pub reason: CommandIoErrorKind,
    /// Observed numeric OS code, if supplied by the source. Never inferred from text.
    pub os_code: Option<i32>,
}

impl CommandFailure {
    pub(crate) fn io(stage: CommandFailureStage, error: &std::io::Error) -> Self {
        Self {
            stage,
            reason: error.kind().into(),
            os_code: error.raw_os_error(),
        }
    }
}

/// EOF collection is independent of exit status and the explicit byte budget.
/// Incomplete counts are lower bounds on bytes actually produced by the process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum CommandOutputCollection {
    NotStarted,
    Complete,
    Incomplete { failure: CommandFailure },
}

/// Raw totals include discarded *observed* pipe bytes, not an unknown failed tail.
/// Delivered totals count only prefixes admitted to the transport queue, not page
/// reads or retries. Neither terminal arrival nor exit zero implies EOF collection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CommandTerminal {
    pub status: CommandStatus,
    pub exit_code: Option<i32>,
    pub process_failure: Option<CommandFailure>,
    pub output_collection: CommandOutputCollection,
    pub stdout_bytes: u64,
    pub stderr_bytes: u64,
    pub stdout_delivered_bytes: u64,
    pub stderr_delivered_bytes: u64,
    pub truncation_reason: Option<CommandOutputTruncation>,
}

impl CommandTerminal {
    /// Validate provider-authored metadata before publishing it as collected evidence.
    pub fn metadata_valid(&self) -> bool {
        let truncated = self.stdout_bytes > self.stdout_delivered_bytes
            || self.stderr_bytes > self.stderr_delivered_bytes;
        if self.status == CommandStatus::Running
            || self.stdout_delivered_bytes > self.stdout_bytes
            || self.stderr_delivered_bytes > self.stderr_bytes
            || self.truncation_reason != truncated.then_some(CommandOutputTruncation::ByteBudget)
        {
            return false;
        }
        match self.process_failure {
            Some(CommandFailure {
                stage: CommandFailureStage::Spawn,
                ..
            }) => {
                self.status == CommandStatus::Failed
                    && self.exit_code.is_none()
                    && self.output_collection == CommandOutputCollection::NotStarted
                    && self.stdout_bytes == 0
                    && self.stderr_bytes == 0
            }
            Some(CommandFailure {
                stage: CommandFailureStage::Wait,
                ..
            }) => {
                self.status != CommandStatus::Completed
                    && self.exit_code.is_none()
                    && self.collection_started()
            }
            Some(_) => false,
            None => self.collection_started(),
        }
    }

    fn collection_started(&self) -> bool {
        match self.output_collection {
            CommandOutputCollection::NotStarted => false,
            CommandOutputCollection::Complete => true,
            CommandOutputCollection::Incomplete { failure } => !matches!(
                failure.stage,
                CommandFailureStage::Spawn | CommandFailureStage::Wait
            ),
        }
    }
}

/// Sending next_cursor acknowledges this page. Only the current page may be
/// retried; acknowledged bytes expire. Terminal fences all captured bytes,
/// including on timeout/cancel. Consumers must pull while the process runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CommandOutput {
    pub chunks: Vec<CommandByteChunk>,
    pub next_cursor: u64,
    pub terminal: Option<CommandTerminal>,
}

impl CommandOutput {
    pub(crate) fn transport_bounds_valid(&self) -> bool {
        let mut end = None;
        let mut stream_ends = [None, None];
        for chunk in &self.chunks {
            let stream_index = match chunk.stream {
                CommandStream::Stdout => 0,
                CommandStream::Stderr => 1,
            };
            if chunk.bytes.is_empty()
                || chunk.offset > chunk.sequence
                || stream_ends[stream_index].is_some_and(|end| chunk.offset != end)
                || end.is_some_and(|end| chunk.sequence != end)
                || chunk
                    .sequence
                    .checked_add(chunk.bytes.len() as u64)
                    .is_none_or(|end| end > MAX_COMMAND_TRANSPORT_BYTES)
                || chunk
                    .offset
                    .checked_add(chunk.bytes.len() as u64)
                    .is_none_or(|end| end > MAX_COMMAND_TRANSPORT_BYTES)
            {
                return false;
            }
            end = chunk.sequence.checked_add(chunk.bytes.len() as u64);
            stream_ends[stream_index] = chunk.offset.checked_add(chunk.bytes.len() as u64);
        }
        self.next_cursor <= MAX_COMMAND_TRANSPORT_BYTES
            && end.is_none_or(|end| end == self.next_cursor)
            && self.terminal.as_ref().is_none_or(|terminal| {
                terminal.metadata_valid()
                    && stream_ends[0].is_none_or(|end| end == terminal.stdout_delivered_bytes)
                    && stream_ends[1].is_none_or(|end| end == terminal.stderr_delivered_bytes)
                    && terminal
                        .stdout_delivered_bytes
                        .checked_add(terminal.stderr_delivered_bytes)
                        == Some(self.next_cursor)
            })
    }
}

pub const MAX_COMMAND_TRANSPORT_BYTES: u64 = 64 * 1024 * 1024;

pub const MAX_COMMAND_READ_BYTES: usize = 64 * 1024;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_wire_rejects_legacy_output_host_paths() {
        let command = serde_json::json!({
            "command": "printf bytes", "timeout_secs": 1, "cwd": "", "transport_byte_budget": 0
        });
        assert!(serde_json::from_value::<CommandRequest>(command.clone()).is_ok());
        for field in ["spill_dir", "output_limit"] {
            let mut legacy = command.clone();
            legacy[field] = serde_json::json!("/host/private");
            assert!(serde_json::from_value::<CommandRequest>(legacy).is_err());
        }
        let output = serde_json::json!({"chunks": [], "next_cursor": 0, "terminal": null, "output_path": "/host/private"});
        assert!(serde_json::from_value::<CommandOutput>(output).is_err());
    }

    #[test]
    fn command_budget_wire_requires_field_and_checks_provider_hard_bound() {
        let wire = serde_json::json!({"command": "true", "timeout_secs": 1, "cwd": ""});
        assert!(serde_json::from_value::<CommandRequest>(wire.clone()).is_err());
        for budget in [
            0,
            MAX_COMMAND_TRANSPORT_BYTES,
            MAX_COMMAND_TRANSPORT_BYTES + 1,
        ] {
            let mut value = wire.clone();
            value["transport_byte_budget"] = serde_json::json!(budget);
            let request: CommandRequest = serde_json::from_value(value).unwrap();
            assert_eq!(
                request.validate().is_ok(),
                budget <= MAX_COMMAND_TRANSPORT_BYTES
            );
        }
        let mut negative = wire;
        negative["transport_byte_budget"] = serde_json::json!(-1);
        assert!(serde_json::from_value::<CommandRequest>(negative).is_err());
        assert_eq!(
            serde_json::to_value(CommandOutputTruncation::ByteBudget).unwrap(),
            "byte_budget"
        );
        let schema = serde_json::to_value(schemars::schema_for!(CommandRequest)).unwrap();
        assert_eq!(
            schema["properties"]["transport_byte_budget"]["maximum"],
            MAX_COMMAND_TRANSPORT_BYTES
        );
        assert!(
            schema["required"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("transport_byte_budget"))
        );
    }

    #[test]
    fn failure_wire_is_closed_and_does_not_infer_os_codes_from_custom_messages() {
        let error = std::io::Error::new(
            std::io::ErrorKind::Other,
            "/host/private TOKEN=secret EACCES command",
        );
        let failure = CommandFailure::io(CommandFailureStage::Wait, &error);
        assert_eq!(
            serde_json::to_value(failure).unwrap(),
            serde_json::json!({"stage":"wait","reason":"other","os_code":null})
        );
        let os_error = std::io::Error::from_raw_os_error(libc::EIO);
        let actual = CommandFailure::io(CommandFailureStage::StdoutDrain, &os_error);
        assert_eq!(actual.os_code, Some(libc::EIO));
        for invalid in [
            serde_json::json!({"stage":"wait","reason":"/host/private TOKEN=secret"}),
            serde_json::json!({"stage":"/host/private","reason":"other"}),
            serde_json::json!({"stage":"wait","reason":"other","message":"secret"}),
        ] {
            assert!(serde_json::from_value::<CommandFailure>(invalid).is_err());
        }
    }

    #[test]
    fn provider_terminal_rejects_inconsistent_failure_domains_and_unstarted_byte_claims() {
        let wire = serde_json::json!({"status":"completed","exit_code":0,"process_failure":null,"output_collection":{"state":"incomplete","failure":{"stage":"stdout_drain","reason":"other","os_code":null}},"stdout_bytes":3,"stderr_bytes":0,"stdout_delivered_bytes":3,"stderr_delivered_bytes":0,"truncation_reason":null});
        let terminal: CommandTerminal = serde_json::from_value(wire.clone()).unwrap();
        assert!(terminal.metadata_valid());
        for (field, value) in [
            (
                "output_collection",
                serde_json::json!({"state":"not_started"}),
            ),
            (
                "output_collection",
                serde_json::json!({"state":"incomplete","failure":{"stage":"wait","reason":"other"}}),
            ),
            (
                "process_failure",
                serde_json::json!({"stage":"stdout_read","reason":"other"}),
            ),
            (
                "process_failure",
                serde_json::json!({"stage":"spawn","reason":"not_found"}),
            ),
            ("truncation_reason", serde_json::json!("byte_budget")),
        ] {
            let mut invalid = wire.clone();
            invalid[field] = value;
            assert!(
                !serde_json::from_value::<CommandTerminal>(invalid)
                    .unwrap()
                    .metadata_valid()
            );
        }
        let mut unclassified = wire.clone();
        unclassified
            .as_object_mut()
            .unwrap()
            .remove("output_collection");
        assert!(serde_json::from_value::<CommandTerminal>(unclassified).is_err());
        let mut raw = wire;
        raw["message"] = serde_json::json!("/private TOKEN=secret");
        assert!(serde_json::from_value::<CommandTerminal>(raw).is_err());
    }
}
