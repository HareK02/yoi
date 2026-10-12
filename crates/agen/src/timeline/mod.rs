//! Dispatch normalized LLM events and collect blocks.
//!
//! [`Timeline`] dispatches common events to handlers without parsing provider protocols.
//! Block assembly and display callbacks are separate from turn decisions. The Engine
//! uses collected responses to update history and execute tools. Provisional streaming
//! output is distinct from committed history.

pub mod event;
mod text_block_collector;
mod thinking_block_collector;
mod timeline;
mod tool_call_collector;

// Public API
pub use event::*;
pub use text_block_collector::TextBlockCollector;
pub use thinking_block_collector::ThinkingBlockCollector;
pub use timeline::Timeline;
pub(crate) use tool_call_collector::CollectedToolCall;
pub use tool_call_collector::ToolCallCollector;

// Re-exports from type definitions
pub use crate::handler::{
    // Meta Kinds
    ErrorKind,
    // Core traits
    Handler,
    Kind,
    PingKind,
    StatusKind,
    // Block Events
    TextBlockEvent,
    // Block Kinds
    TextBlockKind,
    TextBlockStart,
    TextBlockStop,
    ThinkingBlockEvent,
    ThinkingBlockKind,
    ThinkingBlockStart,
    ThinkingBlockStop,
    ToolUseBlockEvent,
    ToolUseBlockKind,
    ToolUseBlockStart,
    ToolUseBlockStop,
    UsageKind,
};
