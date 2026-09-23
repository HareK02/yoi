use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct CommandHandle(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CommandRequest {
    pub command: String,
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_u64))]
    pub timeout_secs: u64,
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_usize))]
    pub output_limit: usize,
    /// Workdir-relative command directory. Providers validate it against the
    /// active session before process start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<fs_operation::FsPath>,
    /// Provider-local directory where complete output is retained when the
    /// inline result exceeds `output_limit`.
    pub spill_dir: Option<PathBuf>,
    /// Optional caller-owned correlation id. Bash supplies its tool-call id so
    /// user-facing command telemetry can update the corresponding Console row
    /// without exposing provider/session handles.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CommandOutputRequest {
    pub handle: CommandHandle,
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_usize))]
    pub cursor: usize,
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_usize))]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CommandOutput {
    pub status: CommandStatus,
    #[schemars(range(min = -2147483648_i32, max = 2147483647_i32))]
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub content: String,
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_usize))]
    pub next_cursor: Option<usize>,
    pub truncated: bool,
    /// Complete output retained by the provider when `truncated` is true.
    pub output_path: Option<PathBuf>,
}
