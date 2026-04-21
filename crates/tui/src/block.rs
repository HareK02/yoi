//! History blocks: the unit of the TUI's stored display model.
//!
//! The TUI holds a flat `Vec<Block>` and re-renders it every frame.
//! Streaming events mutate the most recent matching block instead of
//! queuing a new line, so one logical thing (a tool call, an assistant
//! reply) stays visually coherent as updates arrive.

#![allow(dead_code)] // Phase 5 will consume `output` in detail mode.

use protocol::{Greeting, NotificationLevel, NotificationSource};

pub enum Block {
    Greeting(Greeting),
    TurnHeader {
        turn: usize,
    },
    UserMessage {
        text: String,
    },
    AssistantText {
        text: String,
    },
    ToolCall(ToolCallBlock),
    Notification {
        level: NotificationLevel,
        source: NotificationSource,
        message: String,
    },
    Compact(CompactEvent),
    TurnStats {
        requests: usize,
        input_tokens: u64,
        output_tokens: u64,
    },
}

pub enum CompactEvent {
    Start,
    Done { new_session_id: uuid::Uuid },
    Failed { error: String },
}

pub struct ToolCallBlock {
    pub id: String,
    pub name: String,
    /// Raw JSON fragments accumulated from `ToolCallArgsDelta`.
    pub args_stream: String,
    /// Final arguments text once `ToolCallDone` lands.
    pub arguments: Option<String>,
    pub state: ToolCallState,
    /// For Edit tool calls: snapshot of the file content *before* the
    /// edit was applied to the cache. Captured at result time so the
    /// diff renderer can reproduce the old-content context even after
    /// subsequent mutations have rolled the cache forward.
    pub edit_snapshot: Option<String>,
}

pub enum ToolCallState {
    /// `ToolCallStart` received, nothing else yet.
    Pending,
    /// At least one `ToolCallArgsDelta` has arrived.
    Streaming,
    /// `ToolCallDone` received, waiting on the tool result.
    Executing,
    /// `ToolResult { is_error: false, .. }` received.
    Done { summary: String, output: Option<String> },
    /// `ToolResult { is_error: true, .. }` received.
    Error { summary: String, output: Option<String> },
    /// Turn ended before a matching `ToolResult` arrived.
    Incomplete,
}
