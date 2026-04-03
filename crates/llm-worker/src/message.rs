//! Message and Item Types
//!
//! This module provides the core types for representing conversation items
//! in the Open Responses format.
//!
//! The primary type is [`Item`], which represents different kinds of conversation
//! elements: messages, function calls, function call outputs, and reasoning.

// Re-export all types from llm_client::types
pub use crate::llm_client::types::{ContentPart, Item, Role};

/// Convenience alias for backward compatibility
///
/// In the Open Responses model, messages are just one type of Item.
/// This alias allows code that expects a "Message" type to continue working.
pub type Message = Item;
