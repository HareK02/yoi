//! Closure-based event callback API
//!
//! Provides a closure-based alternative to implementing `Handler<K>` directly.
//! Register callbacks on `Worker` via `on_text_block()`, `on_tool_use_block()`,
//! `on_usage()`, etc.

use std::marker::PhantomData;

use crate::handler::{
    Handler, Kind, TextBlockEvent, TextBlockKind, ToolUseBlockEvent, ToolUseBlockKind,
    ToolUseBlockStart,
};
use crate::tool::ToolCall;

// =============================================================================
// TextBlock Closure Handler
// =============================================================================

/// Callback scope for a text block.
///
/// Passed to the setup closure registered with `Worker::on_text_block()`.
/// Register per-block callbacks via `on_delta()` and `on_stop()`.
///
/// # Examples
///
/// ```ignore
/// worker.on_text_block(|block| {
///     block.on_delta(|text| print!("{}", text));
///     block.on_stop(|full_text| println!("\n--- {} chars ---", full_text.len()));
/// });
/// ```
pub struct TextBlockScope {
    pub(crate) on_delta: Option<Box<dyn FnMut(&str) + Send + Sync>>,
    pub(crate) on_stop: Option<Box<dyn FnMut(&str) + Send + Sync>>,
}

impl TextBlockScope {
    fn new() -> Self {
        Self {
            on_delta: None,
            on_stop: None,
        }
    }

    /// Register a callback for each text delta (streaming fragment).
    pub fn on_delta(&mut self, f: impl FnMut(&str) + Send + Sync + 'static) {
        self.on_delta = Some(Box::new(f));
    }

    /// Register a callback invoked when the block completes.
    ///
    /// Receives the full accumulated text of the block.
    pub fn on_stop(&mut self, f: impl FnMut(&str) + Send + Sync + 'static) {
        self.on_stop = Some(Box::new(f));
    }
}

/// Per-block state created by Timeline's scope lifecycle.
#[derive(Default)]
pub(crate) struct TextBlockClosureState {
    on_delta: Option<Box<dyn FnMut(&str) + Send + Sync>>,
    on_stop: Option<Box<dyn FnMut(&str) + Send + Sync>>,
    buffer: String,
}

/// Closure-based `Handler<TextBlockKind>` adapter.
pub(crate) struct ClosureTextBlockHandler {
    pub(crate) setup: Box<dyn FnMut(&mut TextBlockScope) + Send + Sync>,
}

impl Handler<TextBlockKind> for ClosureTextBlockHandler {
    type Scope = TextBlockClosureState;

    fn on_event(&mut self, scope: &mut Self::Scope, event: &TextBlockEvent) {
        match event {
            TextBlockEvent::Start(_) => {
                scope.buffer.clear();
                let mut builder = TextBlockScope::new();
                (self.setup)(&mut builder);
                scope.on_delta = builder.on_delta;
                scope.on_stop = builder.on_stop;
            }
            TextBlockEvent::Delta(text) => {
                scope.buffer.push_str(text);
                if let Some(f) = &mut scope.on_delta {
                    f(text);
                }
            }
            TextBlockEvent::Stop(_) => {
                if let Some(f) = &mut scope.on_stop {
                    f(&scope.buffer);
                }
            }
        }
    }
}

// =============================================================================
// ToolUseBlock Closure Handler
// =============================================================================

/// Callback scope for a tool use block.
///
/// Passed to the setup closure registered with `Worker::on_tool_use_block()`.
/// The setup closure also receives `&ToolUseBlockStart` with `id` and `name`.
///
/// # Examples
///
/// ```ignore
/// worker.on_tool_use_block(|start, block| {
///     println!("Tool: {} ({})", start.name, start.id);
///     block.on_delta(|json| { /* streaming JSON fragment */ });
///     block.on_stop(|call| println!("Done: {}", call.name));
/// });
/// ```
pub struct ToolUseBlockScope {
    pub(crate) on_delta: Option<Box<dyn FnMut(&str) + Send + Sync>>,
    pub(crate) on_stop: Option<Box<dyn FnMut(&ToolCall) + Send + Sync>>,
}

impl ToolUseBlockScope {
    fn new() -> Self {
        Self {
            on_delta: None,
            on_stop: None,
        }
    }

    /// Register a callback for each JSON input delta (streaming fragment).
    pub fn on_delta(&mut self, f: impl FnMut(&str) + Send + Sync + 'static) {
        self.on_delta = Some(Box::new(f));
    }

    /// Register a callback invoked when the block completes.
    ///
    /// Receives the fully assembled `ToolCall` with parsed JSON input.
    pub fn on_stop(&mut self, f: impl FnMut(&ToolCall) + Send + Sync + 'static) {
        self.on_stop = Some(Box::new(f));
    }
}

/// Per-block state for tool use closure handler.
#[derive(Default)]
pub(crate) struct ToolUseBlockClosureState {
    on_delta: Option<Box<dyn FnMut(&str) + Send + Sync>>,
    on_stop: Option<Box<dyn FnMut(&ToolCall) + Send + Sync>>,
    id: String,
    name: String,
    input_json: String,
}

/// Closure-based `Handler<ToolUseBlockKind>` adapter.
pub(crate) struct ClosureToolUseBlockHandler {
    pub(crate) setup: Box<dyn FnMut(&ToolUseBlockStart, &mut ToolUseBlockScope) + Send + Sync>,
}

impl Handler<ToolUseBlockKind> for ClosureToolUseBlockHandler {
    type Scope = ToolUseBlockClosureState;

    fn on_event(&mut self, scope: &mut Self::Scope, event: &ToolUseBlockEvent) {
        match event {
            ToolUseBlockEvent::Start(start) => {
                scope.id = start.id.clone();
                scope.name = start.name.clone();
                scope.input_json.clear();
                let mut builder = ToolUseBlockScope::new();
                (self.setup)(start, &mut builder);
                scope.on_delta = builder.on_delta;
                scope.on_stop = builder.on_stop;
            }
            ToolUseBlockEvent::InputJsonDelta(json) => {
                scope.input_json.push_str(json);
                if let Some(f) = &mut scope.on_delta {
                    f(json);
                }
            }
            ToolUseBlockEvent::Stop(_) => {
                let input: serde_json::Value =
                    serde_json::from_str(&scope.input_json).unwrap_or_default();
                let tool_call = ToolCall {
                    id: std::mem::take(&mut scope.id),
                    name: std::mem::take(&mut scope.name),
                    input,
                };
                if let Some(f) = &mut scope.on_stop {
                    f(&tool_call);
                }
            }
        }
    }
}

// =============================================================================
// Generic Meta Event Closure Handler
// =============================================================================

/// Closure-based `Handler<K>` adapter for meta events (Usage, Status, Error).
pub(crate) struct ClosureMetaHandler<F, K>
where
    K: Kind,
{
    pub(crate) callback: F,
    pub(crate) _kind: PhantomData<K>,
}

impl<F, K> Handler<K> for ClosureMetaHandler<F, K>
where
    F: FnMut(&K::Event) + Send + Sync,
    K: Kind,
{
    type Scope = ();

    fn on_event(&mut self, _scope: &mut (), event: &K::Event) {
        (self.callback)(event);
    }
}
