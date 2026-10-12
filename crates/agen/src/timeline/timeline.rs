//! Timeline layer.
//!
//! Receives an LLM event stream and dispatches it to registered handlers.
//! Usually used through the Engine, but can also be used directly.

use std::{collections::HashMap, marker::PhantomData};

use super::event::*;
use crate::handler::*;

// =============================================================================
// Helpers
// =============================================================================

/// Merge multiple UsageEvents received within one request.
/// Overwrite each field when the new value is `Some`.
/// Some providers send input/cache fields only in the first event and update
/// output_tokens in later events, so use the latest non-None value per field
/// rather than taking only the last event.
fn merge_usage(acc: &mut UsageEvent, new: &UsageEvent) {
    if new.input_tokens.is_some() {
        acc.input_tokens = new.input_tokens;
    }
    if new.output_tokens.is_some() {
        acc.output_tokens = new.output_tokens;
    }
    if new.total_tokens.is_some() {
        acc.total_tokens = new.total_tokens;
    }
    if new.cache_read_input_tokens.is_some() {
        acc.cache_read_input_tokens = new.cache_read_input_tokens;
    }
    if new.cache_creation_input_tokens.is_some() {
        acc.cache_creation_input_tokens = new.cache_creation_input_tokens;
    }
}

// =============================================================================
// Type-erased Handler
// =============================================================================

/// Type-erased `Handler` trait.
///
/// Each handler has its own Scope type, so Timeline needs type erasure to store them.
/// Usually wrapped automatically by methods such as `Timeline::on_text_block()`
/// rather than used directly.
pub trait ErasedHandler<K: Kind>: Send + Sync {
    /// Dispatch an event.
    fn dispatch(&mut self, event: &K::Event);
    /// Start a scope.
    fn start_scope(&mut self);
}

/// Wrapper that exposes `Handler<K>` as `ErasedHandler<K>`.
pub struct HandlerWrapper<H, K>
where
    H: Handler<K>,
    K: Kind,
{
    handler: H,
    scope: Option<H::Scope>,
    // fn() -> K is always Send+Sync, so Send does not depend on K's bounds.
    _kind: PhantomData<fn() -> K>,
}

impl<H, K> HandlerWrapper<H, K>
where
    H: Handler<K>,
    K: Kind,
{
    pub fn new(handler: H) -> Self {
        Self {
            handler,
            scope: None,
            _kind: PhantomData,
        }
    }
}

impl<H, K> ErasedHandler<K> for HandlerWrapper<H, K>
where
    H: Handler<K> + Send + Sync,
    K: Kind,
    H::Scope: Send + Sync,
{
    fn dispatch(&mut self, event: &K::Event) {
        if let Some(scope) = &mut self.scope {
            self.handler.on_event(scope, event);
        }
    }

    fn start_scope(&mut self) {
        self.scope = Some(H::Scope::default());
    }
}

// =============================================================================
// Block Handler Registry
// =============================================================================

/// Type-erased trait for block handlers.
trait ErasedBlockHandler: Send + Sync {
    fn dispatch_start(&mut self, start: &BlockStart);
    fn dispatch_delta(&mut self, delta: &BlockDelta);
    fn dispatch_stop(&mut self, stop: &BlockStop);
    fn dispatch_abort(&mut self, abort: &BlockAbort);
    fn start_scope(&mut self);
    fn end_scope(&mut self);
    /// Whether a scope is active.
    fn has_scope(&self) -> bool;
}

/// Wrapper for TextBlockKind.
struct TextBlockHandlerWrapper<H>
where
    H: Handler<TextBlockKind>,
{
    handler: H,
    scope: Option<H::Scope>,
}

impl<H> TextBlockHandlerWrapper<H>
where
    H: Handler<TextBlockKind>,
{
    fn new(handler: H) -> Self {
        Self {
            handler,
            scope: None,
        }
    }
}

impl<H> ErasedBlockHandler for TextBlockHandlerWrapper<H>
where
    H: Handler<TextBlockKind> + Send + Sync,
    H::Scope: Send + Sync,
{
    fn dispatch_start(&mut self, start: &BlockStart) {
        if let Some(scope) = &mut self.scope {
            self.handler.on_event(
                scope,
                &TextBlockEvent::Start(TextBlockStart { index: start.index }),
            );
        }
    }

    fn dispatch_delta(&mut self, delta: &BlockDelta) {
        if let Some(scope) = &mut self.scope {
            if let DeltaContent::Text(text) = &delta.delta {
                self.handler
                    .on_event(scope, &TextBlockEvent::Delta(text.clone()));
            }
        }
    }

    fn dispatch_stop(&mut self, stop: &BlockStop) {
        if let Some(scope) = &mut self.scope {
            self.handler.on_event(
                scope,
                &TextBlockEvent::Stop(TextBlockStop {
                    index: stop.index,
                    stop_reason: stop.stop_reason.clone(),
                }),
            );
        }
    }

    fn dispatch_abort(&mut self, _abort: &BlockAbort) {
        // TextBlock needs no special abort handling (only scope cleanup).
    }

    fn start_scope(&mut self) {
        self.scope = Some(H::Scope::default());
    }

    fn end_scope(&mut self) {
        self.scope = None;
    }

    fn has_scope(&self) -> bool {
        self.scope.is_some()
    }
}

/// Wrapper for ThinkingBlockKind.
struct ThinkingBlockHandlerWrapper<H>
where
    H: Handler<ThinkingBlockKind>,
{
    handler: H,
    scopes: HashMap<usize, H::Scope>,
}

impl<H> ThinkingBlockHandlerWrapper<H>
where
    H: Handler<ThinkingBlockKind>,
{
    fn new(handler: H) -> Self {
        Self {
            handler,
            scopes: HashMap::new(),
        }
    }
}

impl<H> ErasedBlockHandler for ThinkingBlockHandlerWrapper<H>
where
    H: Handler<ThinkingBlockKind> + Send + Sync,
    H::Scope: Send + Sync,
{
    fn dispatch_start(&mut self, start: &BlockStart) {
        let scope = self.scopes.entry(start.index).or_default();
        self.handler.on_event(
            scope,
            &ThinkingBlockEvent::Start(ThinkingBlockStart { index: start.index }),
        );
    }

    fn dispatch_delta(&mut self, delta: &BlockDelta) {
        if let DeltaContent::Thinking(text) = &delta.delta {
            let scope = self.scopes.entry(delta.index).or_default();
            self.handler
                .on_event(scope, &ThinkingBlockEvent::Delta(text.clone()));
        }
    }

    fn dispatch_stop(&mut self, stop: &BlockStop) {
        if let Some(mut scope) = self.scopes.remove(&stop.index) {
            self.handler.on_event(
                &mut scope,
                &ThinkingBlockEvent::Stop(ThinkingBlockStop {
                    index: stop.index,
                    reasoning: stop.reasoning.clone(),
                }),
            );
        }
    }

    fn dispatch_abort(&mut self, _abort: &BlockAbort) {
        self.scopes.clear();
    }

    fn start_scope(&mut self) {}

    fn end_scope(&mut self) {}

    fn has_scope(&self) -> bool {
        !self.scopes.is_empty()
    }
}

/// Wrapper for ToolUseBlockKind.
struct ToolUseBlockHandlerWrapper<H>
where
    H: Handler<ToolUseBlockKind>,
{
    handler: H,
    scopes: HashMap<usize, H::Scope>,
    tools: HashMap<usize, (String, String)>, // index -> (id, name)
}

impl<H> ToolUseBlockHandlerWrapper<H>
where
    H: Handler<ToolUseBlockKind>,
{
    fn new(handler: H) -> Self {
        Self {
            handler,
            scopes: HashMap::new(),
            tools: HashMap::new(),
        }
    }
}

impl<H> ErasedBlockHandler for ToolUseBlockHandlerWrapper<H>
where
    H: Handler<ToolUseBlockKind> + Send + Sync,
    H::Scope: Send + Sync,
{
    fn dispatch_start(&mut self, start: &BlockStart) {
        if let BlockMetadata::ToolUse { id, name } = &start.metadata {
            let scope = self.scopes.entry(start.index).or_default();
            self.tools.insert(start.index, (id.clone(), name.clone()));
            self.handler.on_event(
                scope,
                &ToolUseBlockEvent::Start(ToolUseBlockStart {
                    index: start.index,
                    id: id.clone(),
                    name: name.clone(),
                }),
            );
        }
    }

    fn dispatch_delta(&mut self, delta: &BlockDelta) {
        if let DeltaContent::InputJson(json) = &delta.delta {
            let scope = self.scopes.entry(delta.index).or_default();
            self.handler
                .on_event(scope, &ToolUseBlockEvent::InputJsonDelta(json.clone()));
        }
    }

    fn dispatch_stop(&mut self, stop: &BlockStop) {
        if let (Some(mut scope), Some((id, name))) = (
            self.scopes.remove(&stop.index),
            self.tools.remove(&stop.index),
        ) {
            self.handler.on_event(
                &mut scope,
                &ToolUseBlockEvent::Stop(ToolUseBlockStop {
                    index: stop.index,
                    id,
                    name,
                }),
            );
        }
    }

    fn dispatch_abort(&mut self, abort: &BlockAbort) {
        self.scopes.remove(&abort.index);
        self.tools.remove(&abort.index);
    }

    fn start_scope(&mut self) {}

    fn end_scope(&mut self) {}

    fn has_scope(&self) -> bool {
        !self.scopes.is_empty()
    }
}

// =============================================================================
// Timeline
// =============================================================================

/// Manages an event stream and dispatches events to handlers.
///
/// Receives events from the LLM and routes them to registered handlers.
/// Block events are handled with scope management.
///
/// # Examples
///
/// ```ignore
/// use agen::{Timeline, Handler, TextBlockKind, TextBlockEvent};
///
/// struct MyHandler;
/// impl Handler<TextBlockKind> for MyHandler {
///     type Scope = String;
///     fn on_event(&mut self, buffer: &mut String, event: &TextBlockEvent) {
///         if let TextBlockEvent::Delta(text) = event {
///             buffer.push_str(text);
///         }
///     }
/// }
///
/// let mut timeline = Timeline::new();
/// timeline.on_text_block(MyHandler);
/// ```
///
/// # Supported event kinds
///
/// - **Meta**: Usage, Ping, Status, Error
/// - **Block**: TextBlock, ThinkingBlock, ToolUseBlock
pub struct Timeline {
    // Meta handlers
    usage_handlers: Vec<Box<dyn ErasedHandler<UsageKind>>>,
    ping_handlers: Vec<Box<dyn ErasedHandler<PingKind>>>,
    status_handlers: Vec<Box<dyn ErasedHandler<StatusKind>>>,
    error_handlers: Vec<Box<dyn ErasedHandler<ErrorKind>>>,
    // Block handlers (grouped by BlockType)
    text_block_handlers: Vec<Box<dyn ErasedBlockHandler>>,
    thinking_block_handlers: Vec<Box<dyn ErasedBlockHandler>>,
    tool_use_block_handlers: Vec<Box<dyn ErasedBlockHandler>>,

    // Currently active blocks. Providers can interleave multiple scopes by index,
    // so track all scopes rather than a single current block.
    active_blocks: HashMap<(BlockType, usize), ()>,
    current_block: Option<BlockType>,

    // Aggregation buffer for Usage events received within one request.
    // Many providers emit multiple Usage events: Anthropic in message_start and
    // message_delta, Gemini in each chunk. Merge them here to dispatch only once
    // at the request boundary via flush_usage().
    pending_usage: Option<UsageEvent>,
}

impl Default for Timeline {
    fn default() -> Self {
        Self::new()
    }
}

impl Timeline {
    pub fn new() -> Self {
        Self {
            usage_handlers: Vec::new(),
            ping_handlers: Vec::new(),
            status_handlers: Vec::new(),
            error_handlers: Vec::new(),
            text_block_handlers: Vec::new(),
            thinking_block_handlers: Vec::new(),
            tool_use_block_handlers: Vec::new(),
            active_blocks: HashMap::new(),
            current_block: None,
            pending_usage: None,
        }
    }

    // =========================================================================
    // Handler Registration
    // =========================================================================

    /// Register a Handler for UsageKind.
    pub fn on_usage<H>(&mut self, handler: H) -> &mut Self
    where
        H: Handler<UsageKind> + Send + Sync + 'static,
        H::Scope: Send + Sync,
    {
        // Meta handlers start with a scope by default.
        let mut wrapper = HandlerWrapper::new(handler);
        wrapper.start_scope();
        self.usage_handlers.push(Box::new(wrapper));
        self
    }

    /// Register a Handler for PingKind.
    pub fn on_ping<H>(&mut self, handler: H) -> &mut Self
    where
        H: Handler<PingKind> + Send + Sync + 'static,
        H::Scope: Send + Sync,
    {
        let mut wrapper = HandlerWrapper::new(handler);
        wrapper.start_scope();
        self.ping_handlers.push(Box::new(wrapper));
        self
    }

    /// Register a Handler for StatusKind.
    pub fn on_status<H>(&mut self, handler: H) -> &mut Self
    where
        H: Handler<StatusKind> + Send + Sync + 'static,
        H::Scope: Send + Sync,
    {
        let mut wrapper = HandlerWrapper::new(handler);
        wrapper.start_scope();
        self.status_handlers.push(Box::new(wrapper));
        self
    }

    /// Register a Handler for ErrorKind.
    pub fn on_error<H>(&mut self, handler: H) -> &mut Self
    where
        H: Handler<ErrorKind> + Send + Sync + 'static,
        H::Scope: Send + Sync,
    {
        let mut wrapper = HandlerWrapper::new(handler);
        wrapper.start_scope();
        self.error_handlers.push(Box::new(wrapper));
        self
    }

    /// Register a Handler for TextBlockKind.
    pub fn on_text_block<H>(&mut self, handler: H) -> &mut Self
    where
        H: Handler<TextBlockKind> + Send + Sync + 'static,
        H::Scope: Send + Sync,
    {
        self.text_block_handlers
            .push(Box::new(TextBlockHandlerWrapper::new(handler)));
        self
    }

    /// Register a Handler for ThinkingBlockKind.
    pub fn on_thinking_block<H>(&mut self, handler: H) -> &mut Self
    where
        H: Handler<ThinkingBlockKind> + Send + Sync + 'static,
        H::Scope: Send + Sync,
    {
        self.thinking_block_handlers
            .push(Box::new(ThinkingBlockHandlerWrapper::new(handler)));
        self
    }

    /// Register a Handler for ToolUseBlockKind.
    pub fn on_tool_use_block<H>(&mut self, handler: H) -> &mut Self
    where
        H: Handler<ToolUseBlockKind> + Send + Sync + 'static,
        H::Scope: Send + Sync,
    {
        self.tool_use_block_handlers
            .push(Box::new(ToolUseBlockHandlerWrapper::new(handler)));
        self
    }

    // =========================================================================
    // Event Dispatch
    // =========================================================================

    /// Main dispatch entry point.
    pub fn dispatch(&mut self, event: &Event) {
        match event {
            // Buffer usage until flush; dispatch other handled meta events immediately.
            Event::Usage(u) => self.dispatch_usage(u),
            Event::Ping(p) => self.dispatch_ping(p),
            Event::Status(s) => self.dispatch_status(s),
            Event::Error(e) => self.dispatch_error(e),
            // Observability-only event: stream trace records it before timeline dispatch.
            Event::UnhandledSse(_) => {}

            // Block events: dispatch with scope management.
            Event::BlockStart(s) => self.handle_block_start(s),
            Event::BlockDelta(d) => self.handle_block_delta(d),
            Event::BlockStop(s) => self.handle_block_stop(s),
            Event::BlockAbort(a) => self.handle_block_abort(a),
        }
    }

    /// Merge Usage events into pending_usage instead of dispatching immediately.
    /// Even if a request emits multiple Usage events, handlers receive the final
    /// values only once, when flush_usage() dispatches them.
    fn dispatch_usage(&mut self, event: &UsageEvent) {
        match &mut self.pending_usage {
            Some(acc) => merge_usage(acc, event),
            None => self.pending_usage = Some(event.clone()),
        }
    }

    /// Dispatch pending_usage to usage_handlers and clear the buffer.
    /// Intended to be called once at the end of a request's stream.
    /// Does nothing if pending_usage is empty.
    pub fn flush_usage(&mut self) {
        if let Some(event) = self.pending_usage.take() {
            for handler in &mut self.usage_handlers {
                handler.dispatch(&event);
            }
        }
    }

    fn dispatch_ping(&mut self, event: &PingEvent) {
        for handler in &mut self.ping_handlers {
            handler.dispatch(event);
        }
    }

    fn dispatch_status(&mut self, event: &StatusEvent) {
        for handler in &mut self.status_handlers {
            handler.dispatch(event);
        }
    }

    fn dispatch_error(&mut self, event: &ErrorEvent) {
        for handler in &mut self.error_handlers {
            handler.dispatch(event);
        }
    }

    fn handle_block_start(&mut self, start: &BlockStart) {
        self.active_blocks
            .insert((start.block_type, start.index), ());
        self.current_block = Some(start.block_type);

        let handlers = self.get_block_handlers_mut(start.block_type);
        for handler in handlers {
            handler.start_scope();
            handler.dispatch_start(start);
        }
    }

    fn handle_block_delta(&mut self, delta: &BlockDelta) {
        let block_type = delta.delta.block_type();

        // Providers such as OpenAI may omit BlockStart, so implicitly start a scope
        // for this index when a Delta arrives without one.
        self.active_blocks
            .entry((block_type, delta.index))
            .or_insert(());
        self.current_block = Some(block_type);

        let handlers = self.get_block_handlers_mut(block_type);
        for handler in handlers {
            // Implicitly start a scope if none exists.
            if !handler.has_scope() {
                handler.start_scope();
            }
            handler.dispatch_delta(delta);
        }
    }

    fn handle_block_stop(&mut self, stop: &BlockStop) {
        let handlers = self.get_block_handlers_mut(stop.block_type);
        for handler in handlers {
            handler.dispatch_stop(stop);
            handler.end_scope();
        }
        self.active_blocks.remove(&(stop.block_type, stop.index));
        self.current_block = self
            .active_blocks
            .keys()
            .next()
            .map(|(block_type, _)| *block_type);
    }

    fn handle_block_abort(&mut self, abort: &BlockAbort) {
        let handlers = self.get_block_handlers_mut(abort.block_type);
        for handler in handlers {
            handler.dispatch_abort(abort);
            handler.end_scope();
        }
        self.active_blocks.remove(&(abort.block_type, abort.index));
        self.current_block = self
            .active_blocks
            .keys()
            .next()
            .map(|(block_type, _)| *block_type);
    }

    fn get_block_handlers_mut(
        &mut self,
        block_type: BlockType,
    ) -> &mut Vec<Box<dyn ErasedBlockHandler>> {
        match block_type {
            BlockType::Text => &mut self.text_block_handlers,
            BlockType::Thinking => &mut self.thinking_block_handlers,
            BlockType::ToolUse => &mut self.tool_use_block_handlers,
            BlockType::ToolResult => &mut self.text_block_handlers, // Treat ToolResult as Text.
        }
    }

    /// Get the currently active block type.
    pub fn current_block(&self) -> Option<BlockType> {
        self.current_block
    }

    /// Abort all currently active blocks.
    ///
    /// Call on cancellation or error to emit BlockAbort events for all in-progress
    /// blocks by index and clean up their scopes.
    pub fn abort_current_block(&mut self) {
        let active = self.active_blocks.keys().copied().collect::<Vec<_>>();
        for (block_type, index) in active {
            let abort = crate::timeline::event::BlockAbort {
                index,
                block_type,
                reason: "Cancelled".to_string(),
            };
            self.handle_block_abort(&abort);
        }
        self.active_blocks.clear();
        self.current_block = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn test_timeline_creation() {
        let timeline = Timeline::new();
        assert!(timeline.current_block().is_none());
    }

    #[test]
    fn unhandled_sse_is_ignored_by_timeline_handlers() {
        struct TestTextHandler {
            calls: Arc<Mutex<Vec<TextBlockEvent>>>,
        }

        impl Handler<TextBlockKind> for TestTextHandler {
            type Scope = ();
            fn on_event(&mut self, _scope: &mut (), event: &TextBlockEvent) {
                self.calls.lock().unwrap().push(event.clone());
            }
        }

        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut timeline = Timeline::new();
        timeline.on_text_block(TestTextHandler {
            calls: calls.clone(),
        });

        timeline.dispatch(&Event::UnhandledSse(UnhandledSseEvent {
            provider: "openai_responses".to_string(),
            event_type: "response.mystery".to_string(),
            data_preview: "{}".to_string(),
            data_len: 2,
        }));

        assert!(timeline.current_block().is_none());
        assert!(calls.lock().unwrap().is_empty());
    }

    #[test]
    fn abort_clears_every_indexed_scope_before_indices_are_reused() {
        #[derive(Default)]
        struct Scope {
            started: bool,
        }
        struct IndexedToolHandler {
            reused_cleanly: Arc<Mutex<Vec<bool>>>,
        }
        impl Handler<ToolUseBlockKind> for IndexedToolHandler {
            type Scope = Scope;

            fn on_event(&mut self, scope: &mut Scope, event: &ToolUseBlockEvent) {
                if matches!(event, ToolUseBlockEvent::Start(_)) {
                    self.reused_cleanly.lock().unwrap().push(!scope.started);
                    scope.started = true;
                }
            }
        }

        let reused_cleanly = Arc::new(Mutex::new(Vec::new()));
        let mut timeline = Timeline::new();
        timeline.on_tool_use_block(IndexedToolHandler {
            reused_cleanly: reused_cleanly.clone(),
        });

        timeline.dispatch(&Event::tool_use_start(0, "first", "tool"));
        timeline.dispatch(&Event::tool_use_start(1, "second", "tool"));
        // Stopping one interleaved block must not hide its still-active sibling.
        timeline.dispatch(&Event::tool_use_stop(0));
        assert_eq!(timeline.current_block(), Some(BlockType::ToolUse));
        timeline.abort_current_block();
        assert!(timeline.current_block().is_none());

        // A later provider response may reuse the same content-block index.
        timeline.dispatch(&Event::tool_use_start(1, "replacement", "tool"));
        timeline.dispatch(&Event::tool_use_stop(1));

        assert_eq!(
            reused_cleanly.lock().unwrap().as_slice(),
            [true, true, true]
        );
    }

    #[test]
    fn test_meta_event_dispatch() {
        // Simple test struct
        struct TestUsageHandler {
            calls: Arc<Mutex<Vec<UsageEvent>>>,
        }

        impl Handler<UsageKind> for TestUsageHandler {
            type Scope = ();
            fn on_event(&mut self, _scope: &mut (), event: &UsageEvent) {
                self.calls.lock().unwrap().push(event.clone());
            }
        }

        let calls = Arc::new(Mutex::new(Vec::new()));
        let handler = TestUsageHandler {
            calls: calls.clone(),
        };

        let mut timeline = Timeline::new();
        timeline.on_usage(handler);

        timeline.dispatch(&Event::usage(100, 50));
        // Only buffered in pending_usage; not yet dispatched.
        assert_eq!(calls.lock().unwrap().len(), 0);

        // Flush dispatches once.
        timeline.flush_usage();
        let recorded = calls.lock().unwrap();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].input_tokens, Some(100));
    }

    #[test]
    fn test_usage_aggregation_and_flush() {
        struct TestUsageHandler {
            calls: Arc<Mutex<Vec<UsageEvent>>>,
        }
        impl Handler<UsageKind> for TestUsageHandler {
            type Scope = ();
            fn on_event(&mut self, _scope: &mut (), event: &UsageEvent) {
                self.calls.lock().unwrap().push(event.clone());
            }
        }

        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut timeline = Timeline::new();
        timeline.on_usage(TestUsageHandler {
            calls: calls.clone(),
        });

        // Anthropic-style: input + provisional output in message_start.
        timeline.dispatch(&Event::Usage(UsageEvent {
            input_tokens: Some(409),
            output_tokens: Some(1),
            total_tokens: Some(410),
            cache_read_input_tokens: Some(0),
            cache_creation_input_tokens: Some(0),
        }));
        // Final output in message_delta.
        timeline.dispatch(&Event::Usage(UsageEvent {
            input_tokens: Some(409),
            output_tokens: Some(71),
            total_tokens: Some(480),
            cache_read_input_tokens: Some(0),
            cache_creation_input_tokens: Some(0),
        }));

        // No dispatch before flushing.
        assert_eq!(calls.lock().unwrap().len(), 0);

        timeline.flush_usage();
        let recorded = calls.lock().unwrap();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].input_tokens, Some(409));
        assert_eq!(recorded[0].output_tokens, Some(71));

        // Flushing again after a flush does nothing.
        drop(recorded);
        timeline.flush_usage();
        assert_eq!(calls.lock().unwrap().len(), 1);
    }
}
