//! llm-engine - LLM Engine Library
//!
//! Provides components for managing interactions with LLMs.
//!
//! # Main Components
//!
//! - [`Engine`] - Central component for managing LLM interactions
//! - [`tool::Tool`] - Tools that can be invoked by the LLM
//! - [`interceptor::Interceptor`] - Control-flow delegation for the execution loop
//! - Closure-based event callbacks via `Engine::on_text_block()`, `on_tool_use_block()`, etc.
//!
//! # Quick Start
//!
//! ```ignore
//! use llm_engine::{Engine, Item};
//!
//! // Create a Engine
//! let mut engine = Engine::new(client)
//!     .system_prompt("You are a helpful assistant.");
//!
//! // Register tools (optional)
//! // engine.register_tool(my_tool_definition)?;
//!
//! // Run the interaction
//! let history = engine.run("Hello!").await?;
//! ```
//!
//! # Cache Protection
//!
//! `run()` automatically locks the cache. To edit state between turns,
//! call `unlock_cache()` first; the next `run()` re-locks automatically.
//!
//! ```ignore
//! engine.run("user input").await?;
//! engine.unlock_cache();
//! engine.set_system_prompt("new prompt");
//! engine.run("next input").await?;
//! ```

mod engine;
mod handler;
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

pub use callback::{TextBlockScope, ThinkingBlockScope, ToolUseBlockScope};
pub use engine::{
    Engine, EngineConfig, EngineError, EngineResult, EngineRunOutput, LlmRetryNotice,
    ToolRegistryError,
};
pub use handler::ToolUseBlockStart;
pub use interceptor::Interceptor;
pub use message::{ContentPart, Item, Message, Role};
pub use tool::{ToolCall, ToolExecutionContext, ToolOutputLimits, ToolResult};
pub use usage_record::UsageRecord;
