use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CommandHandle(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandRequest {
    pub command: String,
    pub timeout_secs: u64,
    pub output_limit: usize,
    /// Optional caller-owned correlation id. Bash supplies its tool-call id so
    /// user-facing command telemetry can update the corresponding Console row
    /// without exposing provider/session handles.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandOutputRequest {
    pub handle: CommandHandle,
    pub cursor: usize,
    pub limit: usize,
    pub wait: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandStatus {
    Running,
    Completed,
    Failed,
    TimedOut,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandStream {
    Stdout,
    Stderr,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CommandStreamSlice {
    pub start_offset: u64,
    pub end_offset: u64,
    pub content: String,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandSnapshot {
    pub command_id: String,
    pub tool_call_id: Option<String>,
    pub status: CommandStatus,
    pub stdout: CommandStreamSlice,
    pub stderr: CommandStreamSlice,
    pub exit_code: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CommandEvent {
    Started {
        command_id: String,
        tool_call_id: Option<String>,
    },
    Output {
        command_id: String,
        stream: CommandStream,
        start_offset: u64,
        end_offset: u64,
        content: String,
    },
    Terminal {
        command_id: String,
        status: CommandStatus,
        exit_code: Option<i32>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandOutput {
    pub status: CommandStatus,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub content: String,
    pub next_cursor: Option<usize>,
    pub truncated: bool,
}
