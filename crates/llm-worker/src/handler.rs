//! Handler/Kind Types
//!
//! Traits for processing events in the Timeline layer.
//! By implementing custom handlers and registering them with Timeline,
//! you can receive stream events.

use crate::timeline::event::*;

// =============================================================================
// Kind Trait
// =============================================================================

/// Marker trait defining event types
///
/// Each Kind specifies its corresponding event type.
/// Handlers are implemented for this Kind, and multiple Handlers
/// with different Scope types can be registered for the same Kind.
pub trait Kind {
    /// Event type corresponding to this Kind
    type Event;
}

// =============================================================================
// Handler Trait
// =============================================================================

/// Handler trait for processing events
///
/// Defines event processing for a specific `Kind`.
/// `Scope` is state held during the block's lifecycle.
///
/// # Examples
///
/// ```ignore
/// use llm_worker::timeline::{Handler, TextBlockEvent, TextBlockKind};
///
/// struct TextCollector {
///     texts: Vec<String>,
/// }
///
/// impl Handler<TextBlockKind> for TextCollector {
///     type Scope = String;  // Buffer per block
///
///     fn on_event(&mut self, buffer: &mut String, event: &TextBlockEvent) {
///         match event {
///             TextBlockEvent::Delta(text) => buffer.push_str(text),
///             TextBlockEvent::Stop(_) => {
///                 self.texts.push(std::mem::take(buffer));
///             }
///             _ => {}
///         }
///     }
/// }
/// ```
pub trait Handler<K: Kind> {
    /// Handler-specific scope type
    ///
    /// Generated with `Default::default()` at block start,
    /// and destroyed at block end.
    type Scope: Default;

    /// Process the event
    fn on_event(&mut self, scope: &mut Self::Scope, event: &K::Event);
}

// =============================================================================
// Meta Kind Definitions
// =============================================================================

/// Usage Kind - for usage events
pub struct UsageKind;
impl Kind for UsageKind {
    type Event = UsageEvent;
}

/// Ping Kind - for ping events
pub struct PingKind;
impl Kind for PingKind {
    type Event = PingEvent;
}

/// Status Kind - for status events
pub struct StatusKind;
impl Kind for StatusKind {
    type Event = StatusEvent;
}

/// Error Kind - for error events
pub struct ErrorKind;
impl Kind for ErrorKind {
    type Event = ErrorEvent;
}

/// Reasoning item Kind - 完成済み reasoning item の永続化用
///
/// 1 reasoning item につき 1 度だけ発火する。Worker は
/// `ReasoningItemCollector` 経由で受け取り、ターン終了時に
/// `Item::Reasoning` として history に append する。
pub struct ReasoningItemKind;
impl Kind for ReasoningItemKind {
    type Event = ReasoningItemEvent;
}

// =============================================================================
// Block Kind Definitions
// =============================================================================

/// TextBlock Kind - for text blocks
pub struct TextBlockKind;
impl Kind for TextBlockKind {
    type Event = TextBlockEvent;
}

/// Text block events
#[derive(Debug, Clone, PartialEq)]
pub enum TextBlockEvent {
    Start(TextBlockStart),
    Delta(String),
    Stop(TextBlockStop),
}

#[derive(Debug, Clone, PartialEq)]
pub struct TextBlockStart {
    pub index: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TextBlockStop {
    pub index: usize,
    pub stop_reason: Option<StopReason>,
}

/// ThinkingBlock Kind - for thinking blocks
pub struct ThinkingBlockKind;
impl Kind for ThinkingBlockKind {
    type Event = ThinkingBlockEvent;
}

/// Thinking block events
#[derive(Debug, Clone, PartialEq)]
pub enum ThinkingBlockEvent {
    Start(ThinkingBlockStart),
    Delta(String),
    Stop(ThinkingBlockStop),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ThinkingBlockStart {
    pub index: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ThinkingBlockStop {
    pub index: usize,
}

/// ToolUseBlock Kind - for tool use blocks
pub struct ToolUseBlockKind;
impl Kind for ToolUseBlockKind {
    type Event = ToolUseBlockEvent;
}

/// Tool use block events
#[derive(Debug, Clone, PartialEq)]
pub enum ToolUseBlockEvent {
    Start(ToolUseBlockStart),
    /// JSON substring of tool arguments
    InputJsonDelta(String),
    Stop(ToolUseBlockStop),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolUseBlockStart {
    pub index: usize,
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolUseBlockStop {
    pub index: usize,
    pub id: String,
    pub name: String,
}
