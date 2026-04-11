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
//! To maximize KV cache hit rate, transition to the locked state
//! with [`Worker::lock()`] before execution.
//!
//! ```ignore
//! let mut locked = worker.lock();
//! locked.run("user input").await?;
//! ```

mod handler;
mod message;
mod worker;

pub(crate) mod callback;
pub mod event;
pub mod llm_client;
pub mod interceptor;
pub mod state;
pub mod timeline;
pub mod tool;
pub mod tool_server;

pub use callback::{TextBlockScope, ToolUseBlockScope};
pub use handler::ToolUseBlockStart;
pub use message::{ContentPart, Item, Message, Role};
pub use interceptor::Interceptor;
pub use tool::{ToolCall, ToolResult};
pub use worker::{ToolRegistryError, Worker, WorkerConfig, WorkerError, WorkerResult};
