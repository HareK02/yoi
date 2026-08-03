use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CommandHandle(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandRequest {
    pub command: String,
    pub timeout_secs: u64,
    pub output_limit: usize,
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
    Cancelled,
    Failed,
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
