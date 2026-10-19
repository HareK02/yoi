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

/// Raw totals include discarded pipe bytes. Delivered totals count only prefixes
/// admitted to the transport queue, not page reads or retries by a consumer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CommandTerminal {
    pub status: CommandStatus,
    pub exit_code: Option<i32>,
    pub stdout_bytes: u64,
    pub stderr_bytes: u64,
    pub stdout_delivered_bytes: u64,
    pub stderr_delivered_bytes: u64,
    pub truncation_reason: Option<CommandOutputTruncation>,
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
                let truncated = terminal.stdout_bytes > terminal.stdout_delivered_bytes
                    || terminal.stderr_bytes > terminal.stderr_delivered_bytes;
                stream_ends[0].is_none_or(|end| end == terminal.stdout_delivered_bytes)
                    && stream_ends[1].is_none_or(|end| end == terminal.stderr_delivered_bytes)
                    && terminal.status != CommandStatus::Running
                    && terminal.stdout_delivered_bytes <= terminal.stdout_bytes
                    && terminal.stderr_delivered_bytes <= terminal.stderr_bytes
                    && terminal
                        .stdout_delivered_bytes
                        .checked_add(terminal.stderr_delivered_bytes)
                        == Some(self.next_cursor)
                    && terminal.truncation_reason
                        == truncated.then_some(CommandOutputTruncation::ByteBudget)
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
}
