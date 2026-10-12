//! ToolCallCollector - a handler for collecting tool calls.
//!
//! Registered as a ToolUseBlockHandler on Timeline to collect streaming ToolUse blocks.

use crate::{
    handler::{Handler, ToolUseBlockEvent, ToolUseBlockKind},
    tool::ToolCall,
};
use std::sync::{Arc, Mutex};

/// Holds tool call information collected from ToolUse blocks.
///
/// Scope type for ToolCallCollector's Handler implementation.
#[derive(Debug, Default)]
pub struct CollectorState {
    /// Current tool call information (while the block is in progress).
    current_id: Option<String>,
    current_name: Option<String>,
    /// Zero-based order among tool calls in this assistant response.
    call_index: Option<usize>,
    /// JSON input being accumulated.
    input_json_buffer: String,
}

/// One provider-completed call together with its stable model order.
#[derive(Debug, Clone)]
pub(crate) struct CollectedToolCall {
    pub call_index: usize,
    pub call: ToolCall,
}

/// ToolCallCollector - a ToolUse block handler.
///
/// Register with Timeline to receive ToolUse block events and collect completed ToolCalls.
#[derive(Clone)]
pub struct ToolCallCollector {
    /// Collected ToolCalls.
    collected: Arc<Mutex<Vec<CollectedToolCall>>>,
    next_call_index: Arc<Mutex<usize>>,
}

impl ToolCallCollector {
    /// Create a new ToolCallCollector.
    pub fn new() -> Self {
        Self {
            collected: Arc::new(Mutex::new(Vec::new())),
            next_call_index: Arc::new(Mutex::new(0)),
        }
    }

    /// Take the collected ToolCalls and clear the collection.
    pub fn take_collected(&self) -> Vec<ToolCall> {
        self.take_collected_with_index()
            .into_iter()
            .map(|collected| collected.call)
            .collect()
    }

    /// Drain completed calls while preserving their model-returned order.
    pub(crate) fn take_collected_with_index(&self) -> Vec<CollectedToolCall> {
        let mut guard = self.collected.lock().unwrap();
        std::mem::take(&mut *guard)
    }

    /// Access the collected ToolCalls.
    pub fn collected(&self) -> Vec<ToolCall> {
        self.collected
            .lock()
            .unwrap()
            .iter()
            .map(|collected| collected.call.clone())
            .collect()
    }

    /// Start one response-local tool-call ordering scope.
    pub(crate) fn begin_response(&self) {
        self.collected.lock().unwrap().clear();
        *self.next_call_index.lock().unwrap() = 0;
    }

    /// Whether any ToolCalls have been collected.
    pub fn has_pending_calls(&self) -> bool {
        !self.collected.lock().unwrap().is_empty()
    }

    /// Clear the collection.
    pub fn clear(&self) {
        self.collected.lock().unwrap().clear();
    }
}

impl Default for ToolCallCollector {
    fn default() -> Self {
        Self::new()
    }
}

impl Handler<ToolUseBlockKind> for ToolCallCollector {
    type Scope = CollectorState;

    fn on_event(&mut self, scope: &mut Self::Scope, event: &ToolUseBlockEvent) {
        match event {
            ToolUseBlockEvent::Start(start) => {
                scope.current_id = Some(start.id.clone());
                scope.current_name = Some(start.name.clone());
                if scope.call_index.is_none() {
                    let mut next = self.next_call_index.lock().unwrap();
                    scope.call_index = Some(*next);
                    *next += 1;
                }
                scope.input_json_buffer.clear();
            }
            ToolUseBlockEvent::InputJsonDelta(delta) => {
                scope.input_json_buffer.push_str(delta);
            }
            ToolUseBlockEvent::Stop(_stop) => {
                // Finalize the ToolCall when the block completes.
                if let (Some(id), Some(name)) = (scope.current_id.take(), scope.current_name.take())
                {
                    // A Stop event proves the provider finished the block, but
                    // malformed/non-object arguments are still not executable.
                    // Preserve an explicit null sentinel so the engine can commit
                    // a terminal validation error without guessing `{}`.
                    let input = if scope.input_json_buffer.trim().is_empty() {
                        serde_json::Value::Object(serde_json::Map::new())
                    } else {
                        serde_json::from_str::<serde_json::Value>(&scope.input_json_buffer)
                            .ok()
                            .filter(serde_json::Value::is_object)
                            .unwrap_or(serde_json::Value::Null)
                    };

                    let tool_call = ToolCall { id, name, input };
                    let call_index = scope
                        .call_index
                        .take()
                        .expect("tool-use stop follows a started collector scope");

                    self.collected.lock().unwrap().push(CollectedToolCall {
                        call_index,
                        call: tool_call,
                    });
                }
                scope.input_json_buffer.clear();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timeline::Timeline;
    use crate::timeline::event::Event;

    #[test]
    fn test_collect_single_tool_call() {
        let collector = ToolCallCollector::new();
        let mut timeline = Timeline::new();
        timeline.on_tool_use_block(collector.clone());

        // Dispatch a sequence of ToolUse block events.
        timeline.dispatch(&Event::tool_use_start(0, "tool_123", "get_weather"));
        timeline.dispatch(&Event::tool_input_delta(0, r#"{"city":"#));
        timeline.dispatch(&Event::tool_input_delta(0, r#""Tokyo"}"#));
        timeline.dispatch(&Event::tool_use_stop(0));

        // Check the collected ToolCalls.
        let calls = collector.take_collected();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "tool_123");
        assert_eq!(calls[0].name, "get_weather");
        assert_eq!(calls[0].input["city"], "Tokyo");
    }

    #[test]
    fn test_collect_empty_buffer_returns_object() {
        // A tool call without arguments: no input_json_delta arrives.
        let collector = ToolCallCollector::new();
        let mut timeline = Timeline::new();
        timeline.on_tool_use_block(collector.clone());

        timeline.dispatch(&Event::tool_use_start(0, "tool_empty", "ListItems"));
        timeline.dispatch(&Event::tool_use_stop(0));

        let calls = collector.take_collected();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "tool_empty");
        assert_eq!(calls[0].name, "ListItems");
        assert!(calls[0].input.is_object());
        assert_eq!(
            calls[0].input,
            serde_json::Value::Object(serde_json::Map::new())
        );
    }

    #[test]
    fn test_collect_interleaved_tool_call_indexes_and_ignore_duplicate_stop() {
        let collector = ToolCallCollector::new();
        let mut timeline = Timeline::new();
        timeline.on_tool_use_block(collector.clone());

        timeline.dispatch(&Event::tool_use_start(0, "call_0", "tool_0"));
        timeline.dispatch(&Event::tool_input_delta(0, r#"{"a":"#));
        timeline.dispatch(&Event::tool_use_start(1, "call_1", "tool_1"));
        timeline.dispatch(&Event::tool_input_delta(1, r#"{"b":2}"#));
        timeline.dispatch(&Event::tool_input_delta(0, r#"1}"#));
        timeline.dispatch(&Event::tool_use_stop(1));
        timeline.dispatch(&Event::tool_use_stop(1));
        timeline.dispatch(&Event::tool_use_stop(0));

        let calls = collector.take_collected();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].id, "call_1");
        assert_eq!(calls[0].input, serde_json::json!({"b": 2}));
        assert_eq!(calls[1].id, "call_0");
        assert_eq!(calls[1].input, serde_json::json!({"a": 1}));
    }

    #[test]
    fn malformed_completed_arguments_are_not_normalized_to_executable_object() {
        let collector = ToolCallCollector::new();
        let mut timeline = Timeline::new();
        timeline.on_tool_use_block(collector.clone());
        timeline.dispatch(&Event::tool_use_start(0, "bad", "mutate"));
        timeline.dispatch(&Event::tool_input_delta(0, r#"{"unterminated":"#));
        timeline.dispatch(&Event::tool_use_stop(0));

        let calls = collector.take_collected();
        assert_eq!(calls.len(), 1);
        assert!(calls[0].input.is_null());
    }

    #[test]
    fn test_collect_multiple_tool_calls() {
        let collector = ToolCallCollector::new();
        let mut timeline = Timeline::new();
        timeline.on_tool_use_block(collector.clone());

        // First ToolCall
        timeline.dispatch(&Event::tool_use_start(0, "call_1", "tool_a"));
        timeline.dispatch(&Event::tool_input_delta(0, r#"{"a":1}"#));
        timeline.dispatch(&Event::tool_use_stop(0));

        // Second ToolCall
        timeline.dispatch(&Event::tool_use_start(1, "call_2", "tool_b"));
        timeline.dispatch(&Event::tool_input_delta(1, r#"{"b":2}"#));
        timeline.dispatch(&Event::tool_use_stop(1));

        let calls = collector.take_collected();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "tool_a");
        assert_eq!(calls[1].name, "tool_b");
    }
}
