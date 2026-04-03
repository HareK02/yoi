//! Public event types for Worker layer
//!
//! Event representation exposed to external users.

use serde::{Deserialize, Serialize};

// =============================================================================
// Core Event Types (from llm_client layer)
// =============================================================================

/// Streaming events from LLM
///
/// Responses from each LLM provider are processed uniformly
/// as a stream of `Event`.
///
/// # Event Types
///
/// - **Meta events**: `Ping`, `Usage`, `Status`, `Error`
/// - **Block events**: `BlockStart`, `BlockDelta`, `BlockStop`, `BlockAbort`
///
/// # Block Lifecycle
///
/// Text and tool calls have events in the order of
/// `BlockStart` → `BlockDelta`(multiple) → `BlockStop`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Event {
    /// Heartbeat
    Ping(PingEvent),
    /// Token usage
    Usage(UsageEvent),
    /// Stream status change
    Status(StatusEvent),
    /// Error occurred
    Error(ErrorEvent),

    /// Block start (text, tool use, etc.)
    BlockStart(BlockStart),
    /// Block delta data
    BlockDelta(BlockDelta),
    /// Block normal end
    BlockStop(BlockStop),
    /// Block abort
    BlockAbort(BlockAbort),
}

// =============================================================================
// Meta Events
// =============================================================================

/// Ping event (heartbeat)
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct PingEvent {
    pub timestamp: Option<u64>,
}

/// Usage event
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct UsageEvent {
    /// Input token count
    pub input_tokens: Option<u64>,
    /// Output token count
    pub output_tokens: Option<u64>,
    /// Total token count
    pub total_tokens: Option<u64>,
    /// Cache read token count
    pub cache_read_input_tokens: Option<u64>,
    /// Cache creation token count
    pub cache_creation_input_tokens: Option<u64>,
}

/// Status event
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatusEvent {
    pub status: ResponseStatus,
}

/// Response status
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ResponseStatus {
    /// Stream started
    Started,
    /// Completed normally
    Completed,
    /// Cancelled
    Cancelled,
    /// Error occurred
    Failed,
}

/// Error event
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ErrorEvent {
    pub code: Option<String>,
    pub message: String,
}

// =============================================================================
// Block Types
// =============================================================================

/// Block type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BlockType {
    /// Text generation
    Text,
    /// Thinking (Claude Extended Thinking, etc.)
    Thinking,
    /// Tool call
    ToolUse,
    /// Tool result
    ToolResult,
}

/// Block start event
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BlockStart {
    /// Block index
    pub index: usize,
    /// Block type
    pub block_type: BlockType,
    /// Block-specific metadata
    pub metadata: BlockMetadata,
}

impl BlockStart {
    pub fn block_type(&self) -> BlockType {
        self.block_type
    }
}

/// Block metadata
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum BlockMetadata {
    Text,
    Thinking,
    ToolUse { id: String, name: String },
    ToolResult { tool_use_id: String },
}

/// Block delta event
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BlockDelta {
    /// Block index
    pub index: usize,
    /// Delta content
    pub delta: DeltaContent,
}

/// Delta content
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DeltaContent {
    /// Text delta
    Text(String),
    /// Thinking delta
    Thinking(String),
    /// JSON substring of tool arguments
    InputJson(String),
}

impl DeltaContent {
    /// Get block type of the delta
    pub fn block_type(&self) -> BlockType {
        match self {
            DeltaContent::Text(_) => BlockType::Text,
            DeltaContent::Thinking(_) => BlockType::Thinking,
            DeltaContent::InputJson(_) => BlockType::ToolUse,
        }
    }
}

/// Block stop event
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BlockStop {
    /// Block index
    pub index: usize,
    /// Block type
    pub block_type: BlockType,
    /// Stop reason
    pub stop_reason: Option<StopReason>,
}

impl BlockStop {
    pub fn block_type(&self) -> BlockType {
        self.block_type
    }
}

/// Block abort event
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BlockAbort {
    /// Block index
    pub index: usize,
    /// Block type
    pub block_type: BlockType,
    /// Abort reason
    pub reason: String,
}

impl BlockAbort {
    pub fn block_type(&self) -> BlockType {
        self.block_type
    }
}

/// Stop reason
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum StopReason {
    /// Natural end
    EndTurn,
    /// Max tokens reached
    MaxTokens,
    /// Stop sequence reached
    StopSequence,
    /// Tool use
    ToolUse,
}

// =============================================================================
// Builder / Factory helpers
// =============================================================================

impl Event {
    /// Create text block start event
    pub fn text_block_start(index: usize) -> Self {
        Event::BlockStart(BlockStart {
            index,
            block_type: BlockType::Text,
            metadata: BlockMetadata::Text,
        })
    }

    /// Create text delta event
    pub fn text_delta(index: usize, text: impl Into<String>) -> Self {
        Event::BlockDelta(BlockDelta {
            index,
            delta: DeltaContent::Text(text.into()),
        })
    }

    /// Create text block stop event
    pub fn text_block_stop(index: usize, stop_reason: Option<StopReason>) -> Self {
        Event::BlockStop(BlockStop {
            index,
            block_type: BlockType::Text,
            stop_reason,
        })
    }

    /// Create tool use block start event
    pub fn tool_use_start(index: usize, id: impl Into<String>, name: impl Into<String>) -> Self {
        Event::BlockStart(BlockStart {
            index,
            block_type: BlockType::ToolUse,
            metadata: BlockMetadata::ToolUse {
                id: id.into(),
                name: name.into(),
            },
        })
    }

    /// Create tool input delta event
    pub fn tool_input_delta(index: usize, json: impl Into<String>) -> Self {
        Event::BlockDelta(BlockDelta {
            index,
            delta: DeltaContent::InputJson(json.into()),
        })
    }

    /// Create tool use block stop event
    pub fn tool_use_stop(index: usize) -> Self {
        Event::BlockStop(BlockStop {
            index,
            block_type: BlockType::ToolUse,
            stop_reason: Some(StopReason::ToolUse),
        })
    }

    /// Create usage event
    pub fn usage(input_tokens: u64, output_tokens: u64) -> Self {
        Event::Usage(UsageEvent {
            input_tokens: Some(input_tokens),
            output_tokens: Some(output_tokens),
            total_tokens: Some(input_tokens + output_tokens),
            cache_read_input_tokens: None,
            cache_creation_input_tokens: None,
        })
    }

    /// Create ping event
    pub fn ping() -> Self {
        Event::Ping(PingEvent { timestamp: None })
    }
}

// =============================================================================
// Conversions: timeline::event -> worker::event
// =============================================================================

impl From<crate::timeline::event::ResponseStatus> for ResponseStatus {
    fn from(value: crate::timeline::event::ResponseStatus) -> Self {
        match value {
            crate::timeline::event::ResponseStatus::Started => ResponseStatus::Started,
            crate::timeline::event::ResponseStatus::Completed => ResponseStatus::Completed,
            crate::timeline::event::ResponseStatus::Cancelled => ResponseStatus::Cancelled,
            crate::timeline::event::ResponseStatus::Failed => ResponseStatus::Failed,
        }
    }
}

impl From<crate::timeline::event::BlockType> for BlockType {
    fn from(value: crate::timeline::event::BlockType) -> Self {
        match value {
            crate::timeline::event::BlockType::Text => BlockType::Text,
            crate::timeline::event::BlockType::Thinking => BlockType::Thinking,
            crate::timeline::event::BlockType::ToolUse => BlockType::ToolUse,
            crate::timeline::event::BlockType::ToolResult => BlockType::ToolResult,
        }
    }
}

impl From<crate::timeline::event::BlockMetadata> for BlockMetadata {
    fn from(value: crate::timeline::event::BlockMetadata) -> Self {
        match value {
            crate::timeline::event::BlockMetadata::Text => BlockMetadata::Text,
            crate::timeline::event::BlockMetadata::Thinking => BlockMetadata::Thinking,
            crate::timeline::event::BlockMetadata::ToolUse { id, name } => {
                BlockMetadata::ToolUse { id, name }
            }
            crate::timeline::event::BlockMetadata::ToolResult { tool_use_id } => {
                BlockMetadata::ToolResult { tool_use_id }
            }
        }
    }
}

impl From<crate::timeline::event::DeltaContent> for DeltaContent {
    fn from(value: crate::timeline::event::DeltaContent) -> Self {
        match value {
            crate::timeline::event::DeltaContent::Text(text) => DeltaContent::Text(text),
            crate::timeline::event::DeltaContent::Thinking(text) => DeltaContent::Thinking(text),
            crate::timeline::event::DeltaContent::InputJson(json) => DeltaContent::InputJson(json),
        }
    }
}

impl From<crate::timeline::event::StopReason> for StopReason {
    fn from(value: crate::timeline::event::StopReason) -> Self {
        match value {
            crate::timeline::event::StopReason::EndTurn => StopReason::EndTurn,
            crate::timeline::event::StopReason::MaxTokens => StopReason::MaxTokens,
            crate::timeline::event::StopReason::StopSequence => StopReason::StopSequence,
            crate::timeline::event::StopReason::ToolUse => StopReason::ToolUse,
        }
    }
}

impl From<crate::timeline::event::PingEvent> for PingEvent {
    fn from(value: crate::timeline::event::PingEvent) -> Self {
        PingEvent {
            timestamp: value.timestamp,
        }
    }
}

impl From<crate::timeline::event::UsageEvent> for UsageEvent {
    fn from(value: crate::timeline::event::UsageEvent) -> Self {
        UsageEvent {
            input_tokens: value.input_tokens,
            output_tokens: value.output_tokens,
            total_tokens: value.total_tokens,
            cache_read_input_tokens: value.cache_read_input_tokens,
            cache_creation_input_tokens: value.cache_creation_input_tokens,
        }
    }
}

impl From<crate::timeline::event::StatusEvent> for StatusEvent {
    fn from(value: crate::timeline::event::StatusEvent) -> Self {
        StatusEvent {
            status: value.status.into(),
        }
    }
}

impl From<crate::timeline::event::ErrorEvent> for ErrorEvent {
    fn from(value: crate::timeline::event::ErrorEvent) -> Self {
        ErrorEvent {
            code: value.code,
            message: value.message,
        }
    }
}

impl From<crate::timeline::event::BlockStart> for BlockStart {
    fn from(value: crate::timeline::event::BlockStart) -> Self {
        BlockStart {
            index: value.index,
            block_type: value.block_type.into(),
            metadata: value.metadata.into(),
        }
    }
}

impl From<crate::timeline::event::BlockDelta> for BlockDelta {
    fn from(value: crate::timeline::event::BlockDelta) -> Self {
        BlockDelta {
            index: value.index,
            delta: value.delta.into(),
        }
    }
}

impl From<crate::timeline::event::BlockStop> for BlockStop {
    fn from(value: crate::timeline::event::BlockStop) -> Self {
        BlockStop {
            index: value.index,
            block_type: value.block_type.into(),
            stop_reason: value.stop_reason.map(Into::into),
        }
    }
}

impl From<crate::timeline::event::BlockAbort> for BlockAbort {
    fn from(value: crate::timeline::event::BlockAbort) -> Self {
        BlockAbort {
            index: value.index,
            block_type: value.block_type.into(),
            reason: value.reason,
        }
    }
}

impl From<crate::timeline::event::Event> for Event {
    fn from(value: crate::timeline::event::Event) -> Self {
        match value {
            crate::timeline::event::Event::Ping(p) => Event::Ping(p.into()),
            crate::timeline::event::Event::Usage(u) => Event::Usage(u.into()),
            crate::timeline::event::Event::Status(s) => Event::Status(s.into()),
            crate::timeline::event::Event::Error(e) => Event::Error(e.into()),
            crate::timeline::event::Event::BlockStart(s) => Event::BlockStart(s.into()),
            crate::timeline::event::Event::BlockDelta(d) => Event::BlockDelta(d.into()),
            crate::timeline::event::Event::BlockStop(s) => Event::BlockStop(s.into()),
            crate::timeline::event::Event::BlockAbort(a) => Event::BlockAbort(a.into()),
        }
    }
}
