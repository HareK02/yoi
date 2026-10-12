//! TextBlockCollector - a handler for collecting text blocks.
//!
//! Registered as a TextBlockHandler on Timeline to collect streaming text blocks.

use crate::handler::{Handler, TextBlockEvent, TextBlockKind};
use std::sync::{Arc, Mutex};

/// Holds text collected from TextBlock events.
#[derive(Debug, Default)]
pub struct TextCollectorState {
    /// Text being accumulated.
    buffer: String,
}

/// TextBlockCollector - a text block handler.
///
/// Register with Timeline to receive TextBlock events and collect completed text blocks.
#[derive(Clone)]
pub struct TextBlockCollector {
    /// Collected text blocks.
    collected: Arc<Mutex<Vec<String>>>,
}

impl TextBlockCollector {
    /// Create a new TextBlockCollector.
    pub fn new() -> Self {
        Self {
            collected: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Take the collected text and clear the collection.
    pub fn take_collected(&self) -> Vec<String> {
        let mut guard = self.collected.lock().unwrap();
        std::mem::take(&mut *guard)
    }

    /// Access the collected text.
    pub fn collected(&self) -> Vec<String> {
        self.collected.lock().unwrap().clone()
    }

    /// Whether any text has been collected.
    pub fn has_content(&self) -> bool {
        !self.collected.lock().unwrap().is_empty()
    }

    /// Clear the collection.
    pub fn clear(&self) {
        self.collected.lock().unwrap().clear();
    }
}

impl Default for TextBlockCollector {
    fn default() -> Self {
        Self::new()
    }
}

impl Handler<TextBlockKind> for TextBlockCollector {
    type Scope = TextCollectorState;

    fn on_event(&mut self, scope: &mut Self::Scope, event: &TextBlockEvent) {
        match event {
            TextBlockEvent::Start(_) => {
                scope.buffer.clear();
            }
            TextBlockEvent::Delta(text) => {
                scope.buffer.push_str(text);
            }
            TextBlockEvent::Stop(_) => {
                // Finalize the text when the block completes.
                if !scope.buffer.is_empty() {
                    let text = std::mem::take(&mut scope.buffer);
                    self.collected.lock().unwrap().push(text);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timeline::Timeline;
    use crate::timeline::event::Event;

    /// Verify that TextBlockCollector correctly collects a single text block.
    #[test]
    fn test_collect_single_text_block() {
        let collector = TextBlockCollector::new();
        let mut timeline = Timeline::new();
        timeline.on_text_block(collector.clone());

        // Dispatch a sequence of text block events.
        timeline.dispatch(&Event::text_block_start(0));
        timeline.dispatch(&Event::text_delta(0, "Hello, "));
        timeline.dispatch(&Event::text_delta(0, "World!"));
        timeline.dispatch(&Event::text_block_stop(0, None));

        // Check the collected text.
        let texts = collector.take_collected();
        assert_eq!(texts.len(), 1);
        assert_eq!(texts[0], "Hello, World!");
    }

    /// Verify that TextBlockCollector correctly collects multiple text blocks.
    #[test]
    fn test_collect_multiple_text_blocks() {
        let collector = TextBlockCollector::new();
        let mut timeline = Timeline::new();
        timeline.on_text_block(collector.clone());

        // First text block
        timeline.dispatch(&Event::text_block_start(0));
        timeline.dispatch(&Event::text_delta(0, "First"));
        timeline.dispatch(&Event::text_block_stop(0, None));

        // Second text block
        timeline.dispatch(&Event::text_block_start(1));
        timeline.dispatch(&Event::text_delta(1, "Second"));
        timeline.dispatch(&Event::text_block_stop(1, None));

        let texts = collector.take_collected();
        assert_eq!(texts.len(), 2);
        assert_eq!(texts[0], "First");
        assert_eq!(texts[1], "Second");
    }
}
