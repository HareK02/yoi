#![doc = include_str!("../README.md")]

mod engine;
mod handler;
mod history;
mod message;

pub(crate) mod callback;
pub mod event;
pub mod interceptor;
pub mod llm_client;
pub mod providers;
pub mod prune;
pub mod state;
pub mod timeline;
pub mod token_counter;
pub mod tool;
pub mod tool_server;
pub mod usage_record;

pub use agen_macros::{description, tool, tool_registry};
pub use callback::{TextBlockScope, ThinkingBlockScope, ToolUseBlockScope};
pub use engine::{
    Engine, EngineConfig, EngineError, EngineResult, EngineRunExit, EngineRunOutput,
    LlmRetryNotice, RunInterruptionReason, ToolRegistryError,
};
pub use handler::ToolUseBlockStart;
pub use history::{History, HistoryEntry};
pub use interceptor::{
    Interceptor, InterceptorError, InterceptorFailure, InterceptorPoint, InterceptorResult,
};
pub use message::{ContentPart, Item, Message, Role};
pub use tool::{
    ToolCall, ToolExecutionContext, ToolExecutionHandle, ToolExecutionPolicy,
    ToolExecutionTerminal, ToolExecutionTerminalFuture, ToolOutputLimits, ToolResult,
    ToolResultDisposition,
};
pub use usage_record::UsageRecord;

/// Implementation dependencies used by code generated from `agen` macros.
///
/// This module is not a stable user-facing API. It is public only because macro expansion
/// happens in the downstream crate.
#[doc(hidden)]
pub mod __private {
    pub use async_trait;
    pub use schemars;
    pub use serde;
    pub use serde_json;
}
