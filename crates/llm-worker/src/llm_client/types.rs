//! LLM Client Common Types
//!
//! Core conversation types for insomnia's LLM interaction model.
//! The core abstraction is `Item` which represents different types of conversation elements:
//! - Message items (user/assistant messages with content parts)
//! - ToolCall items (tool invocations)
//! - ToolResult items (tool results)
//! - Reasoning items (extended thinking)

use serde::{Deserialize, Serialize};

fn is_false(value: &bool) -> bool {
    !*value
}

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
        /// Short summary (always kept in history, survives pruning)
        summary: String,
        /// Detailed output (removed by pruning when old enough)
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content: Option<String>,
        /// Whether the tool result represents an execution error.
        #[serde(default, skip_serializing_if = "is_false")]
        is_error: bool,
    },

    /// Reasoning/thinking item
    Reasoning {
        /// Optional item ID
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<ItemId>,
        /// Reasoning text（reasoning body, `reasoning_text.delta` の累積）
        text: String,
        /// Reasoning summary（OpenAI Responses の `summary_text[]` を格納。
        /// 他 scheme は空）
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        summary: Vec<String>,
        /// サーバから返された暗号化済み reasoning blob。ZDR / `store=false`
        /// 運用で stateless に再送するときそのまま添える必要がある。
        /// Anthropic の `redacted_thinking.data` もここに格納する。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        encrypted_content: Option<String>,
        /// Anthropic extended thinking の `signature`。新世代 Claude
        /// (Opus 4.5+/Sonnet 4.6+) では同一論理ターン内の `thinking`
        /// ブロックを送り返す際に必須。改ざん検知に使われる。他 scheme
        /// では `None`。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
        /// Item status
        #[serde(skip_serializing_if = "Option::is_none")]
        status: Option<ItemStatus>,
    },
}

impl Item {
    // ========================================================================
    // Message constructors
    // ========================================================================

    /// Create a system message item with text content.
    ///
    /// System items in history are sent as `role: "system"` on OpenAI,
    /// and as `role: "user"` on Anthropic/Gemini (which lack a system
    /// role in conversation items).
    pub fn system_message(text: impl Into<String>) -> Self {
        Self::Message {
            id: None,
            role: Role::System,
            content: vec![ContentPart::Text { text: text.into() }],
            status: None,
        }
    }

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

    /// Create a tool result item with summary only (no content).
    pub fn tool_result(call_id: impl Into<String>, summary: impl Into<String>) -> Self {
        Self::tool_result_item(call_id, summary, None, false)
    }

    /// Create an error tool result item with summary only (no content).
    pub fn tool_result_error(call_id: impl Into<String>, summary: impl Into<String>) -> Self {
        Self::tool_result_item(call_id, summary, None, true)
    }

    /// Create a tool result item with summary, optional content, and error flag.
    pub fn tool_result_item(
        call_id: impl Into<String>,
        summary: impl Into<String>,
        content: Option<String>,
        is_error: bool,
    ) -> Self {
        Self::ToolResult {
            id: None,
            call_id: call_id.into(),
            summary: summary.into(),
            content,
            is_error,
        }
    }

    /// Create a tool result item with summary and content.
    pub fn tool_result_with_content(
        call_id: impl Into<String>,
        summary: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        Self::tool_result_item(call_id, summary, Some(content.into()), false)
    }

    // ========================================================================
    // Reasoning constructors
    // ========================================================================

    /// Create a reasoning item
    pub fn reasoning(text: impl Into<String>) -> Self {
        Self::Reasoning {
            id: None,
            text: text.into(),
            summary: Vec::new(),
            encrypted_content: None,
            signature: None,
            status: None,
        }
    }

    /// Set reasoning summary on a `Reasoning` item. No-op on other variants.
    pub fn with_reasoning_summary(mut self, new_summary: Vec<String>) -> Self {
        if let Self::Reasoning { summary, .. } = &mut self {
            *summary = new_summary;
        }
        self
    }

    /// Set `encrypted_content` on a `Reasoning` item. No-op on other variants.
    pub fn with_encrypted_content(mut self, content: impl Into<String>) -> Self {
        if let Self::Reasoning {
            encrypted_content, ..
        } = &mut self
        {
            *encrypted_content = Some(content.into());
        }
        self
    }

    /// Set Anthropic `signature` on a `Reasoning` item. No-op on other variants.
    pub fn with_signature(mut self, sig: impl Into<String>) -> Self {
        if let Self::Reasoning { signature, .. } = &mut self {
            *signature = Some(sig.into());
        }
        self
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

/// Parse a ToolCall `arguments` string into a JSON object.
///
/// Tool call arguments must be a JSON object at the provider API level
/// (Anthropic rejects non-object `tool_use.input`). This helper normalizes
/// anything that is not a JSON object — empty string, the literal `"null"`,
/// arrays, scalars, or parse failures — to an empty object `{}`.
pub fn parse_tool_arguments(arguments: &str) -> serde_json::Value {
    match serde_json::from_str::<serde_json::Value>(arguments) {
        Ok(value) if value.is_object() => value,
        _ => serde_json::Value::Object(serde_json::Map::new()),
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
    /// Index into `items` marking the end of a stable, cacheable prefix.
    ///
    /// Higher layers that know about durable prefix boundaries (e.g. a
    /// post-compaction summary) set this so that caching-aware providers
    /// (Anthropic today) can place a long-lived cache breakpoint there.
    /// Providers without prompt caching ignore the field.
    pub cache_anchor: Option<usize>,
    /// 会話単位の安定キー。`prompt_cache_key` として送られる
    /// (OpenAI Responses)。ChatGPT backend (codex-oauth) は明示キーが
    /// 無いと org/project ハッシュ衝突でプロンプトキャッシュが
    /// ほぼヒットしないため、pod 側で `SessionId` を渡す運用を想定。
    /// `cache_anchor` と違い名前空間キーであり、`prefix anchor` とは
    /// 別の概念。`cache_anchor` を読まない provider と同じく、
    /// `prompt_cache_key` を持たない provider は無視する。
    pub cache_key: Option<String>,
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

    /// Set the conversation cache key.
    ///
    /// 詳細は [`Request::cache_key`] のフィールドコメント参照。
    pub fn cache_key(mut self, key: impl Into<String>) -> Self {
        self.cache_key = Some(key.into());
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
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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
    /// Reasoning / extended-thinking 制御（共通型、scheme 側で各社形式に投影）。
    ///
    /// `None` のときは何も送らない。`Some` でも scheme の
    /// `ModelCapability::reasoning` が `None` なら無視される。
    #[serde(default)]
    pub reasoning: Option<crate::llm_client::capability::ReasoningControl>,
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

#[cfg(test)]
mod parse_tool_arguments_tests {
    use super::parse_tool_arguments;
    use serde_json::{Value, json};

    fn empty_object() -> Value {
        Value::Object(serde_json::Map::new())
    }

    #[test]
    fn empty_string_normalizes_to_object() {
        assert_eq!(parse_tool_arguments(""), empty_object());
    }

    #[test]
    fn literal_null_normalizes_to_object() {
        // 既存セッションに残っている "null" が resume 時に復旧できること
        assert_eq!(parse_tool_arguments("null"), empty_object());
    }

    #[test]
    fn array_normalizes_to_object() {
        assert_eq!(parse_tool_arguments("[1, 2, 3]"), empty_object());
    }

    #[test]
    fn scalar_normalizes_to_object() {
        assert_eq!(parse_tool_arguments("42"), empty_object());
        assert_eq!(parse_tool_arguments("\"str\""), empty_object());
        assert_eq!(parse_tool_arguments("true"), empty_object());
    }

    #[test]
    fn invalid_json_normalizes_to_object() {
        assert_eq!(parse_tool_arguments("{not json"), empty_object());
    }

    #[test]
    fn valid_object_passes_through() {
        assert_eq!(
            parse_tool_arguments(r#"{"city":"Tokyo","days":3}"#),
            json!({"city": "Tokyo", "days": 3}),
        );
    }

    #[test]
    fn empty_object_passes_through() {
        assert_eq!(parse_tool_arguments("{}"), empty_object());
    }
}
