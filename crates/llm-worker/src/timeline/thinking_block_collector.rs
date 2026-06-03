//! `ThinkingBlockCollector` - reasoning material from Thinking block lifecycle.
//!
//! Scheme implementations emit Thinking BlockStart/Delta/Stop events for live
//! streaming. A Thinking block stop with `ReasoningBlockData` is also the single
//! authoritative persistence signal for `Item::Reasoning` round-trip material.

use std::sync::{Arc, Mutex};

use crate::handler::{Handler, ThinkingBlockEvent, ThinkingBlockKind};
use crate::llm_client::event::ReasoningBlockData;

/// Reasoning material collected from completed Thinking blocks.
#[derive(Clone, Default)]
pub struct ThinkingBlockCollector {
    collected: Arc<Mutex<Vec<ReasoningBlockData>>>,
}

impl ThinkingBlockCollector {
    pub fn new() -> Self {
        Self::default()
    }

    /// 収集済み item を取り出してクリア
    pub fn take_collected(&self) -> Vec<ReasoningBlockData> {
        let mut guard = self.collected.lock().unwrap();
        std::mem::take(&mut *guard)
    }

    /// 収集をクリア
    pub fn clear(&self) {
        self.collected.lock().unwrap().clear();
    }
}

impl Handler<ThinkingBlockKind> for ThinkingBlockCollector {
    type Scope = String;

    fn on_event(&mut self, scope: &mut Self::Scope, event: &ThinkingBlockEvent) {
        match event {
            ThinkingBlockEvent::Start(_) => scope.clear(),
            ThinkingBlockEvent::Delta(text) => scope.push_str(text),
            ThinkingBlockEvent::Stop(stop) => {
                if let Some(mut reasoning) = stop.reasoning.clone() {
                    if reasoning.text.is_none() {
                        reasoning.text = Some(scope.clone());
                    }
                    self.collected.lock().unwrap().push(reasoning);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm_client::event::{
        BlockMetadata, BlockStart, BlockStop, BlockType, DeltaContent, Event, ReasoningBlockData,
    };
    use crate::timeline::Timeline;

    #[test]
    fn collects_in_order_from_thinking_block_stops() {
        let collector = ThinkingBlockCollector::new();
        let mut timeline = Timeline::new();
        timeline.on_thinking_block(collector.clone());

        timeline.dispatch(&Event::BlockStart(BlockStart {
            index: 0,
            block_type: BlockType::Thinking,
            metadata: BlockMetadata::Thinking,
        }));
        timeline.dispatch(&Event::BlockDelta(crate::llm_client::event::BlockDelta {
            index: 0,
            delta: DeltaContent::Thinking("first".into()),
        }));
        timeline.dispatch(&Event::BlockStop(BlockStop {
            index: 0,
            block_type: BlockType::Thinking,
            stop_reason: None,
            reasoning: Some(ReasoningBlockData {
                id: Some("r1".into()),
                signature: Some("sig1".into()),
                ..Default::default()
            }),
        }));

        timeline.dispatch(&Event::BlockStart(BlockStart {
            index: 1,
            block_type: BlockType::Thinking,
            metadata: BlockMetadata::Thinking,
        }));
        timeline.dispatch(&Event::BlockStop(BlockStop {
            index: 1,
            block_type: BlockType::Thinking,
            stop_reason: None,
            reasoning: Some(ReasoningBlockData {
                id: Some("r2".into()),
                text: Some("second".into()),
                ..Default::default()
            }),
        }));

        let items = collector.take_collected();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].text.as_deref(), Some("first"));
        assert_eq!(items[0].signature.as_deref(), Some("sig1"));
        assert_eq!(items[1].text.as_deref(), Some("second"));

        // take は drain なので 2 度目は空
        assert!(collector.take_collected().is_empty());
    }

    #[test]
    fn ignores_streaming_only_thinking_blocks() {
        let collector = ThinkingBlockCollector::new();
        let mut timeline = Timeline::new();
        timeline.on_thinking_block(collector.clone());

        timeline.dispatch(&Event::BlockStart(BlockStart {
            index: 0,
            block_type: BlockType::Thinking,
            metadata: BlockMetadata::Thinking,
        }));
        timeline.dispatch(&Event::BlockDelta(crate::llm_client::event::BlockDelta {
            index: 0,
            delta: DeltaContent::Thinking("live only".into()),
        }));
        timeline.dispatch(&Event::BlockStop(BlockStop {
            index: 0,
            block_type: BlockType::Thinking,
            stop_reason: None,
            reasoning: None,
        }));

        assert!(collector.take_collected().is_empty());
    }
}
