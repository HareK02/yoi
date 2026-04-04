//! LLM Client Common Types
//!
//! Core conversation types for insomnia's LLM interaction model.
//! The core abstraction is `Item` which represents different types of conversation elements:
//! - Message items (user/assistant messages with content parts)
//! - ToolCall items (tool invocations)
//! - ToolResult items (tool results)
//! - Reasoning items (extended thinking)

use serde::{Deserialize, Serialize};

// ============================================================================
// Item - The core unit of conversation
// ============================================================================

/// Item ID type for tracking items in a conversation
pub type ItemId = String;

/// Call ID type for linking function calls to their outputs
pub type CallId = String;

/// Conversation item - the primary unit of conversation history
///
/// Items represent discrete elements in a conversation. Tool calls and reasoning
/// are first-class items rather than parts of messages.
///
/// # Examples
///
/// ```ignore
/// use llm_worker::Item;
///
/// let user = Item::user_message("Hello!");
/// let assistant = Item::assistant_message("Hi there!");
/// let call = Item::tool_call("call_123", "get_weather", json!({"city": "Tokyo"}));
/// let result = Item::tool_result("call_123", "Sunny, 25°C");
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Item {
    /// User or assistant message with content parts
    Message {
        /// Optional item ID
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<ItemId>,
        /// Message role
        role: Role,
        /// Content parts
        content: Vec<ContentPart>,
        /// Item status
        #[serde(skip_serializing_if = "Option::is_none")]
        status: Option<ItemStatus>,
    },

    /// Tool call from the assistant
    ToolCall {
        /// Optional item ID
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<ItemId>,
        /// Call ID for linking to result
        call_id: CallId,
        /// Tool name
        name: String,
        /// Tool arguments as JSON string
        arguments: String,
        /// Item status
        #[serde(skip_serializing_if = "Option::is_none")]
        status: Option<ItemStatus>,
    },

    /// Tool call result
    ToolResult {
        /// Optional item ID
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<ItemId>,
        /// Call ID linking to the tool call
        call_id: CallId,
        /// Output content
        output: String,
    },

    /// Reasoning/thinking item
    Reasoning {
        /// Optional item ID
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<ItemId>,
        /// Reasoning text
        text: String,
        /// Item status
        #[serde(skip_serializing_if = "Option::is_none")]
        status: Option<ItemStatus>,
    },
}

impl Item {
    // ========================================================================
    // Message constructors
    // ========================================================================

    /// Create a user message item with text content
    pub fn user_message(text: impl Into<String>) -> Self {
        Self::Message {
            id: None,
            role: Role::User,
            content: vec![ContentPart::Text { text: text.into() }],
            status: None,
        }
    }

    /// Create a user message item with multiple content parts
    pub fn user_message_parts(parts: Vec<ContentPart>) -> Self {
        Self::Message {
            id: None,
            role: Role::User,
            content: parts,
            status: None,
        }
    }

    /// Create an assistant message item with text content
    pub fn assistant_message(text: impl Into<String>) -> Self {
        Self::Message {
            id: None,
            role: Role::Assistant,
            content: vec![ContentPart::Text { text: text.into() }],
            status: None,
        }
    }

    /// Create an assistant message item with multiple content parts
    pub fn assistant_message_parts(parts: Vec<ContentPart>) -> Self {
        Self::Message {
            id: None,
            role: Role::Assistant,
            content: parts,
            status: None,
        }
    }

    // ========================================================================
    // Tool call constructors
    // ========================================================================

    /// Create a tool call item
    pub fn tool_call(
        call_id: impl Into<String>,
        name: impl Into<String>,
        arguments: impl Into<String>,
    ) -> Self {
        Self::ToolCall {
            id: None,
            call_id: call_id.into(),
            name: name.into(),
            arguments: arguments.into(),
            status: None,
        }
    }

    /// Create a tool call item from a JSON value
    pub fn tool_call_json(
        call_id: impl Into<String>,
        name: impl Into<String>,
        arguments: serde_json::Value,
    ) -> Self {
        Self::tool_call(call_id, name, arguments.to_string())
    }

    /// Create a tool result item
    pub fn tool_result(call_id: impl Into<String>, output: impl Into<String>) -> Self {
        Self::ToolResult {
            id: None,
            call_id: call_id.into(),
            output: output.into(),
        }
    }

    // ========================================================================
    // Reasoning constructors
    // ========================================================================

    /// Create a reasoning item
    pub fn reasoning(text: impl Into<String>) -> Self {
        Self::Reasoning {
            id: None,
            text: text.into(),
            status: None,
        }
    }

    // ========================================================================
    // Builder methods
    // ========================================================================

    /// Set the item ID
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        match &mut self {
            Self::Message { id: item_id, .. } => *item_id = Some(id.into()),
            Self::ToolCall { id: item_id, .. } => *item_id = Some(id.into()),
            Self::ToolResult { id: item_id, .. } => *item_id = Some(id.into()),
            Self::Reasoning { id: item_id, .. } => *item_id = Some(id.into()),
        }
        self
    }

    /// Set the item status
    pub fn with_status(mut self, new_status: ItemStatus) -> Self {
        match &mut self {
            Self::Message { status, .. } => *status = Some(new_status),
            Self::ToolCall { status, .. } => *status = Some(new_status),
            Self::ToolResult { .. } => {} // Result items don't have status
            Self::Reasoning { status, .. } => *status = Some(new_status),
        }
        self
    }

    // ========================================================================
    // Accessors
    // ========================================================================

    /// Get the item ID if set
    pub fn id(&self) -> Option<&str> {
        match self {
            Self::Message { id, .. } => id.as_deref(),
            Self::ToolCall { id, .. } => id.as_deref(),
            Self::ToolResult { id, .. } => id.as_deref(),
            Self::Reasoning { id, .. } => id.as_deref(),
        }
    }

    /// Get the item type as a string
    pub fn item_type(&self) -> &'static str {
        match self {
            Self::Message { .. } => "message",
            Self::ToolCall { .. } => "tool_call",
            Self::ToolResult { .. } => "tool_result",
            Self::Reasoning { .. } => "reasoning",
        }
    }

    /// Check if this is a user message
    pub fn is_user_message(&self) -> bool {
        matches!(
            self,
            Self::Message {
                role: Role::User,
                ..
            }
        )
    }

    /// Check if this is an assistant message
    pub fn is_assistant_message(&self) -> bool {
        matches!(
            self,
            Self::Message {
                role: Role::Assistant,
                ..
            }
        )
    }

    /// Check if this is a tool call
    pub fn is_tool_call(&self) -> bool {
        matches!(self, Self::ToolCall { .. })
    }

    /// Check if this is a tool result
    pub fn is_tool_result(&self) -> bool {
        matches!(self, Self::ToolResult { .. })
    }

    /// Check if this is a reasoning item
    pub fn is_reasoning(&self) -> bool {
        matches!(self, Self::Reasoning { .. })
    }

    /// Get text content if this is a simple text message
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Message { content, .. } if content.len() == 1 => match &content[0] {
                ContentPart::Text { text } => Some(text),
                _ => None,
            },
            _ => None,
        }
    }
}

// ============================================================================
// Content Parts - Components within message items
// ============================================================================

/// Content part within a message item
///
/// Text content is role-agnostic; the containing Item's Role determines
/// whether it's user input or assistant output.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentPart {
    /// Text content
    Text {
        /// The text content
        text: String,
    },

    /// Refusal content (for assistant messages)
    Refusal {
        /// The refusal message
        refusal: String,
    },
}

impl ContentPart {
    /// Create a text part
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into() }
    }

    /// Create a refusal part
    pub fn refusal(refusal: impl Into<String>) -> Self {
        Self::Refusal {
            refusal: refusal.into(),
        }
    }

    /// Get the text content regardless of type
    pub fn as_text(&self) -> &str {
        match self {
            Self::Text { text } => text,
            Self::Refusal { refusal } => refusal,
        }
    }
}

// ============================================================================
// Role and Status
// ============================================================================

/// Message role
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// User
    User,
    /// Assistant
    Assistant,
    /// System (for system prompts, not typically used in items)
    System,
}

/// Item status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ItemStatus {
    /// Item is being generated
    InProgress,
    /// Item completed successfully
    Completed,
    /// Item was truncated (e.g., max tokens)
    Incomplete,
}

// ============================================================================
// Request Types
// ============================================================================

/// LLM Request
#[derive(Debug, Clone, Default)]
pub struct Request {
    /// System prompt (instructions)
    pub system_prompt: Option<String>,
    /// Input items (conversation history)
    pub items: Vec<Item>,
    /// Tool definitions
    pub tools: Vec<ToolDefinition>,
    /// Request configuration
    pub config: RequestConfig,
}

impl Request {
    /// Create a new empty request
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the system prompt
    pub fn system(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(prompt.into());
        self
    }

    /// Add a user message
    pub fn user(mut self, content: impl Into<String>) -> Self {
        self.items.push(Item::user_message(content));
        self
    }

    /// Add an assistant message
    pub fn assistant(mut self, content: impl Into<String>) -> Self {
        self.items.push(Item::assistant_message(content));
        self
    }

    /// Add an item
    pub fn item(mut self, item: Item) -> Self {
        self.items.push(item);
        self
    }

    /// Add multiple items
    pub fn items(mut self, items: impl IntoIterator<Item = Item>) -> Self {
        self.items.extend(items);
        self
    }

    /// Add a tool definition
    pub fn tool(mut self, tool: ToolDefinition) -> Self {
        self.tools.push(tool);
        self
    }

    /// Set the request config
    pub fn config(mut self, config: RequestConfig) -> Self {
        self.config = config;
        self
    }

    /// Set max tokens
    pub fn max_tokens(mut self, max_tokens: u32) -> Self {
        self.config.max_tokens = Some(max_tokens);
        self
    }

    /// Set temperature
    pub fn temperature(mut self, temperature: f32) -> Self {
        self.config.temperature = Some(temperature);
        self
    }

    /// Set top_p
    pub fn top_p(mut self, top_p: f32) -> Self {
        self.config.top_p = Some(top_p);
        self
    }

    /// Set top_k
    pub fn top_k(mut self, top_k: u32) -> Self {
        self.config.top_k = Some(top_k);
        self
    }

    /// Add a stop sequence
    pub fn stop_sequence(mut self, sequence: impl Into<String>) -> Self {
        self.config.stop_sequences.push(sequence.into());
        self
    }
}

// ============================================================================
// Tool Definition
// ============================================================================

/// Tool (function) definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// Tool name
    pub name: String,
    /// Tool description
    pub description: Option<String>,
    /// Input schema (JSON Schema)
    pub input_schema: serde_json::Value,
}

impl ToolDefinition {
    /// Create a new tool definition
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: None,
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
        }
    }

    /// Set the description
    pub fn description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }

    /// Set the input schema
    pub fn input_schema(mut self, schema: serde_json::Value) -> Self {
        self.input_schema = schema;
        self
    }
}

// ============================================================================
// Request Config
// ============================================================================

/// Request configuration
#[derive(Debug, Clone, Default)]
pub struct RequestConfig {
    /// Maximum tokens to generate
    pub max_tokens: Option<u32>,
    /// Temperature (randomness)
    pub temperature: Option<f32>,
    /// Top P (nucleus sampling)
    pub top_p: Option<f32>,
    /// Top K
    pub top_k: Option<u32>,
    /// Stop sequences
    pub stop_sequences: Vec<String>,
}

impl RequestConfig {
    /// Create a new default config
    pub fn new() -> Self {
        Self::default()
    }

    /// Set max tokens
    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = Some(max_tokens);
        self
    }

    /// Set temperature
    pub fn with_temperature(mut self, temperature: f32) -> Self {
        self.temperature = Some(temperature);
        self
    }

    /// Set top_p
    pub fn with_top_p(mut self, top_p: f32) -> Self {
        self.top_p = Some(top_p);
        self
    }

    /// Set top_k
    pub fn with_top_k(mut self, top_k: u32) -> Self {
        self.top_k = Some(top_k);
        self
    }

    /// Add a stop sequence
    pub fn with_stop_sequence(mut self, sequence: impl Into<String>) -> Self {
        self.stop_sequences.push(sequence.into());
        self
    }
}
