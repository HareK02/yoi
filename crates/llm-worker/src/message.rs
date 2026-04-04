//! Message and Item Types
//!
//! Core types for representing conversation items.
//!
//! The primary type is [`Item`], which represents different kinds of conversation
//! elements: messages, tool calls, tool results, and reasoning.

// Re-export all types from llm_client::types
pub use crate::llm_client::types::{ContentPart, Item, Role};

/// Convenience alias
///
/// Messages are just one type of Item.
pub type Message = Item;
