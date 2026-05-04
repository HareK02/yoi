//! `ReasoningItemCollector` - 完成済み reasoning item を収集する Handler
//!
//! Timeline の `ReasoningItemKind` Handler として登録し、scheme 側が
//! `Event::ReasoningItem` を発火するたびに 1 件ずつバッファに溜める。
//! Worker はターン終了時に `take_collected()` でドレインして
//! `Item::Reasoning` として `worker.history` に append する。

use std::sync::{Arc, Mutex};

use crate::handler::{Handler, ReasoningItemKind};
use crate::llm_client::event::ReasoningItemEvent;

/// 収集された reasoning item の連列。
#[derive(Clone, Default)]
pub struct ReasoningItemCollector {
    collected: Arc<Mutex<Vec<ReasoningItemEvent>>>,
}

impl ReasoningItemCollector {
    pub fn new() -> Self {
        Self::default()
    }

    /// 収集済み item を取り出してクリア
    pub fn take_collected(&self) -> Vec<ReasoningItemEvent> {
        let mut guard = self.collected.lock().unwrap();
        std::mem::take(&mut *guard)
    }

    /// 収集をクリア
    pub fn clear(&self) {
        self.collected.lock().unwrap().clear();
    }
}

impl Handler<ReasoningItemKind> for ReasoningItemCollector {
    type Scope = ();

    fn on_event(&mut self, _scope: &mut Self::Scope, event: &ReasoningItemEvent) {
        self.collected.lock().unwrap().push(event.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm_client::event::Event;
    use crate::timeline::Timeline;

    #[test]
    fn collects_in_order() {
        let collector = ReasoningItemCollector::new();
        let mut timeline = Timeline::new();
        timeline.on_reasoning_item(collector.clone());

        timeline.dispatch(&Event::ReasoningItem(ReasoningItemEvent {
            id: Some("r1".into()),
            text: "first".into(),
            signature: Some("sig1".into()),
            ..Default::default()
        }));
        timeline.dispatch(&Event::ReasoningItem(ReasoningItemEvent {
            id: Some("r2".into()),
            text: "second".into(),
            ..Default::default()
        }));

        let items = collector.take_collected();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].text, "first");
        assert_eq!(items[0].signature.as_deref(), Some("sig1"));
        assert_eq!(items[1].text, "second");

        // take は drain なので 2 度目は空
        assert!(collector.take_collected().is_empty());
    }
}
