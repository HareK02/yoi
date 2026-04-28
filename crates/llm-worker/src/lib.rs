//! llm-worker - LLM Worker Library
//!
//! Provides components for managing interactions with LLMs.
//!
//! # Main Components
//!
//! - [`Worker`] - Central component for managing LLM interactions
//! - [`tool::Tool`] - Tools that can be invoked by the LLM
//! - [`interceptor::Interceptor`] - Control-flow delegation for the execution loop
//! - Closure-based event callbacks via `Worker::on_text_block()`, `on_tool_use_block()`, etc.
//!
//! # Quick Start
//!
//! ```ignore
//! use llm_worker::{Worker, Item};
//!
//! // Create a Worker
//! let mut worker = Worker::new(client)
//!     .system_prompt("You are a helpful assistant.");
//!
//! // Register tools (optional)
//! // worker.register_tool(my_tool_definition)?;
//!
//! // Run the interaction
//! let history = worker.run("Hello!").await?;
//! ```
//!
//! # Cache Protection
//!
//! `run()` automatically locks the cache. To edit state between turns,
//! call `unlock_cache()` first; the next `run()` re-locks automatically.
//!
//! ```ignore
//! worker.run("user input").await?;
//! worker.unlock_cache();
//! worker.set_system_prompt("new prompt");
//! worker.run("next input").await?;
//! ```

mod handler;
mod message;
mod worker;

pub(crate) mod callback;
pub mod event;
pub mod interceptor;
pub mod llm_client;
pub mod prune;
pub mod state;
pub mod timeline;
pub mod token_counter;
pub mod tool;
pub mod tool_server;
pub mod usage_record;

pub use callback::{TextBlockScope, ThinkingBlockScope, ToolUseBlockScope};
pub use handler::ToolUseBlockStart;
pub use interceptor::Interceptor;
pub use message::{ContentPart, Item, Message, Role};
pub use tool::{ToolCall, ToolOutputLimits, ToolResult};
pub use usage_record::UsageRecord;
pub use worker::{RunOutput, ToolRegistryError, Worker, WorkerConfig, WorkerError, WorkerResult};
