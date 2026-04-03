//! Event Subscription
//!
//! Trait for receiving streaming events from LLM in real-time.
//! Used for stream display to UI and progress display.

use std::sync::{Arc, Mutex};

use crate::{
    handler::{
        ErrorKind, Handler, StatusKind, TextBlockEvent, TextBlockKind, ToolUseBlockEvent,
        ToolUseBlockKind, UsageKind,
    },
    hook::ToolCall,
    timeline::event::{ErrorEvent, StatusEvent, UsageEvent},
};

// =============================================================================
// WorkerSubscriber Trait
// =============================================================================

/// Trait for subscribing to streaming events from LLM
///
/// When registered with Worker, you can receive events from text generation
/// and tool calls in real-time. Ideal for stream display to UI.
///
/// # Available Events
///
/// - **Block events**: Text, tool use (with scope)
/// - **Meta events**: Usage, status, error
/// - **Completion events**: Text complete, tool call complete
/// - **Turn control**: Turn start, turn end
///
/// # Examples
///
/// ```ignore
/// use llm_worker::subscriber::WorkerSubscriber;
/// use llm_worker::timeline::TextBlockEvent;
///
/// struct StreamPrinter;
///
/// impl WorkerSubscriber for StreamPrinter {
///     type TextBlockScope = ();
///     type ToolUseBlockScope = ();
///
///     fn on_text_block(&mut self, _: &mut (), event: &TextBlockEvent) {
///         if let TextBlockEvent::Delta(text) = event {
///             print!("{}", text);  // Real-time output
///         }
///     }
///
///     fn on_text_complete(&mut self, text: &str) {
///         println!("\n--- Complete: {} chars ---", text.len());
///     }
/// }
///
/// // Register with Worker
/// worker.subscribe(StreamPrinter);
/// ```
pub trait WorkerSubscriber: Send {
    // =========================================================================
    // Scope Types (for block events)
    // =========================================================================

    /// Scope type for text block processing
    ///
    /// Generated with Default::default() at block start,
    /// destroyed at block end.
    type TextBlockScope: Default + Send + Sync;

    /// Scope type for tool use block processing
    type ToolUseBlockScope: Default + Send + Sync;

    // =========================================================================
    // Block Events (with scope management)
    // =========================================================================

    /// Text block event
    ///
    /// Has Start/Delta/Stop lifecycle.
    /// Scope is generated at block start and destroyed at end.
    #[allow(unused_variables)]
    fn on_text_block(&mut self, scope: &mut Self::TextBlockScope, event: &TextBlockEvent) {}

    /// Tool use block event
    ///
    /// Has Start/InputJsonDelta/Stop lifecycle.
    #[allow(unused_variables)]
    fn on_tool_use_block(
        &mut self,
        scope: &mut Self::ToolUseBlockScope,
        event: &ToolUseBlockEvent,
    ) {
    }

    // =========================================================================
    // Single Events (no scope needed)
    // =========================================================================

    /// Usage event
    #[allow(unused_variables)]
    fn on_usage(&mut self, event: &UsageEvent) {}

    /// Status event
    #[allow(unused_variables)]
    fn on_status(&mut self, event: &StatusEvent) {}

    /// Error event
    #[allow(unused_variables)]
    fn on_error(&mut self, event: &ErrorEvent) {}

    // =========================================================================
    // Accumulated Events (added in Worker layer)
    // =========================================================================

    /// Text complete event
    ///
    /// When a text block completes, the entire accumulated text is passed.
    /// Convenient for receiving the final result after block processing.
    #[allow(unused_variables)]
    fn on_text_complete(&mut self, text: &str) {}

    /// Tool call complete event
    ///
    /// When a tool use block completes, the complete ToolCall is passed.
    #[allow(unused_variables)]
    fn on_tool_call_complete(&mut self, call: &ToolCall) {}

    // =========================================================================
    // Turn Control
    // =========================================================================

    /// On turn start
    ///
    /// `turn` is a 0-based turn number.
    #[allow(unused_variables)]
    fn on_turn_start(&mut self, turn: usize) {}

    /// On turn end
    #[allow(unused_variables)]
    fn on_turn_end(&mut self, turn: usize) {}
}

// =============================================================================
// SubscriberAdapter - Bridge WorkerSubscriber to Timeline handlers
// =============================================================================

// =============================================================================
// TextBlock Handler Adapter
// =============================================================================

/// Subscriber adapter for TextBlockKind
pub(crate) struct TextBlockSubscriberAdapter<S: WorkerSubscriber> {
    subscriber: Arc<Mutex<S>>,
}

impl<S: WorkerSubscriber> TextBlockSubscriberAdapter<S> {
    pub fn new(subscriber: Arc<Mutex<S>>) -> Self {
        Self { subscriber }
    }
}

impl<S: WorkerSubscriber> Clone for TextBlockSubscriberAdapter<S> {
    fn clone(&self) -> Self {
        Self {
            subscriber: self.subscriber.clone(),
        }
    }
}

/// Wrapper for TextBlock scope
pub struct TextBlockScopeWrapper<S: WorkerSubscriber> {
    inner: S::TextBlockScope,
    buffer: String, // Buffer for on_text_complete
}

impl<S: WorkerSubscriber> Default for TextBlockScopeWrapper<S> {
    fn default() -> Self {
        Self {
            inner: S::TextBlockScope::default(),
            buffer: String::new(),
        }
    }
}

impl<S: WorkerSubscriber + 'static> Handler<TextBlockKind> for TextBlockSubscriberAdapter<S> {
    type Scope = TextBlockScopeWrapper<S>;

    fn on_event(&mut self, scope: &mut Self::Scope, event: &TextBlockEvent) {
        // Accumulate deltas into buffer
        if let TextBlockEvent::Delta(text) = event {
            scope.buffer.push_str(text);
        }

        // Call Subscriber's TextBlock event handler
        if let Ok(mut subscriber) = self.subscriber.lock() {
            subscriber.on_text_block(&mut scope.inner, event);

            // Also call on_text_complete on Stop
            if matches!(event, TextBlockEvent::Stop(_)) {
                subscriber.on_text_complete(&scope.buffer);
            }
        }
    }
}

// =============================================================================
// ToolUseBlock Handler Adapter
// =============================================================================

/// Subscriber adapter for ToolUseBlockKind
pub(crate) struct ToolUseBlockSubscriberAdapter<S: WorkerSubscriber> {
    subscriber: Arc<Mutex<S>>,
}

impl<S: WorkerSubscriber> ToolUseBlockSubscriberAdapter<S> {
    pub fn new(subscriber: Arc<Mutex<S>>) -> Self {
        Self { subscriber }
    }
}

impl<S: WorkerSubscriber> Clone for ToolUseBlockSubscriberAdapter<S> {
    fn clone(&self) -> Self {
        Self {
            subscriber: self.subscriber.clone(),
        }
    }
}

/// Wrapper for ToolUseBlock scope
pub struct ToolUseBlockScopeWrapper<S: WorkerSubscriber> {
    inner: S::ToolUseBlockScope,
    id: String,
    name: String,
    input_json: String, // JSON accumulation
}

impl<S: WorkerSubscriber> Default for ToolUseBlockScopeWrapper<S> {
    fn default() -> Self {
        Self {
            inner: S::ToolUseBlockScope::default(),
            id: String::new(),
            name: String::new(),
            input_json: String::new(),
        }
    }
}

impl<S: WorkerSubscriber + 'static> Handler<ToolUseBlockKind> for ToolUseBlockSubscriberAdapter<S> {
    type Scope = ToolUseBlockScopeWrapper<S>;

    fn on_event(&mut self, scope: &mut Self::Scope, event: &ToolUseBlockEvent) {
        // Save metadata on Start
        if let ToolUseBlockEvent::Start(start) = event {
            scope.id = start.id.clone();
            scope.name = start.name.clone();
        }

        // Accumulate InputJsonDelta into buffer
        if let ToolUseBlockEvent::InputJsonDelta(json) = event {
            scope.input_json.push_str(json);
        }

        // Call Subscriber's ToolUseBlock event handler
        if let Ok(mut subscriber) = self.subscriber.lock() {
            subscriber.on_tool_use_block(&mut scope.inner, event);

            // Also call on_tool_call_complete on Stop
            if matches!(event, ToolUseBlockEvent::Stop(_)) {
                let input: serde_json::Value =
                    serde_json::from_str(&scope.input_json).unwrap_or_default();
                let tool_call = ToolCall {
                    id: scope.id.clone(),
                    name: scope.name.clone(),
                    input,
                };
                subscriber.on_tool_call_complete(&tool_call);
            }
        }
    }
}

// =============================================================================
// Meta Event Handler Adapters
// =============================================================================

/// Subscriber adapter for UsageKind
pub(crate) struct UsageSubscriberAdapter<S: WorkerSubscriber> {
    subscriber: Arc<Mutex<S>>,
}

impl<S: WorkerSubscriber> UsageSubscriberAdapter<S> {
    pub fn new(subscriber: Arc<Mutex<S>>) -> Self {
        Self { subscriber }
    }
}

impl<S: WorkerSubscriber> Clone for UsageSubscriberAdapter<S> {
    fn clone(&self) -> Self {
        Self {
            subscriber: self.subscriber.clone(),
        }
    }
}

impl<S: WorkerSubscriber + 'static> Handler<UsageKind> for UsageSubscriberAdapter<S> {
    type Scope = ();

    fn on_event(&mut self, _scope: &mut Self::Scope, event: &UsageEvent) {
        if let Ok(mut subscriber) = self.subscriber.lock() {
            subscriber.on_usage(event);
        }
    }
}

/// Subscriber adapter for StatusKind
pub(crate) struct StatusSubscriberAdapter<S: WorkerSubscriber> {
    subscriber: Arc<Mutex<S>>,
}

impl<S: WorkerSubscriber> StatusSubscriberAdapter<S> {
    pub fn new(subscriber: Arc<Mutex<S>>) -> Self {
        Self { subscriber }
    }
}

impl<S: WorkerSubscriber> Clone for StatusSubscriberAdapter<S> {
    fn clone(&self) -> Self {
        Self {
            subscriber: self.subscriber.clone(),
        }
    }
}

impl<S: WorkerSubscriber + 'static> Handler<StatusKind> for StatusSubscriberAdapter<S> {
    type Scope = ();

    fn on_event(&mut self, _scope: &mut Self::Scope, event: &StatusEvent) {
        if let Ok(mut subscriber) = self.subscriber.lock() {
            subscriber.on_status(event);
        }
    }
}

/// Subscriber adapter for ErrorKind
pub(crate) struct ErrorSubscriberAdapter<S: WorkerSubscriber> {
    subscriber: Arc<Mutex<S>>,
}

impl<S: WorkerSubscriber> ErrorSubscriberAdapter<S> {
    pub fn new(subscriber: Arc<Mutex<S>>) -> Self {
        Self { subscriber }
    }
}

impl<S: WorkerSubscriber> Clone for ErrorSubscriberAdapter<S> {
    fn clone(&self) -> Self {
        Self {
            subscriber: self.subscriber.clone(),
        }
    }
}

impl<S: WorkerSubscriber + 'static> Handler<ErrorKind> for ErrorSubscriberAdapter<S> {
    type Scope = ();

    fn on_event(&mut self, _scope: &mut Self::Scope, event: &ErrorEvent) {
        if let Ok(mut subscriber) = self.subscriber.lock() {
            subscriber.on_error(event);
        }
    }
}
