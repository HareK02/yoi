//! TextBlockCollector - テキストブロック収集用ハンドラ
//!
//! TimelineのTextBlockHandler として登録され、
//! ストリーム中のテキストブロックを収集する。

use crate::handler::{Handler, TextBlockEvent, TextBlockKind};
use std::sync::{Arc, Mutex};

/// TextBlockから収集したテキスト情報を保持
#[derive(Debug, Default)]
pub struct TextCollectorState {
    /// 蓄積中のテキスト
    buffer: String,
}

/// TextBlockCollector - テキストブロックハンドラ
///
/// Timelineに登録してTextBlockイベントを受信し、
/// 完了したテキストブロックを収集する。
#[derive(Clone)]
pub struct TextBlockCollector {
    /// 収集されたテキストブロック
    collected: Arc<Mutex<Vec<String>>>,
}

impl TextBlockCollector {
    /// 新しいTextBlockCollectorを作成
    pub fn new() -> Self {
        Self {
            collected: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// 収集されたテキストを取得してクリア
    pub fn take_collected(&self) -> Vec<String> {
        let mut guard = self.collected.lock().unwrap();
        std::mem::take(&mut *guard)
    }

    /// 収集されたテキストの参照を取得
    pub fn collected(&self) -> Vec<String> {
        self.collected.lock().unwrap().clone()
    }

    /// 収集されたテキストがあるかどうか
    pub fn has_content(&self) -> bool {
        !self.collected.lock().unwrap().is_empty()
    }

    /// 収集をクリア
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
                // ブロック完了時にテキストを確定
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

    /// TextBlockCollectorが単一のテキストブロックを正しく収集することを確認
    #[test]
    fn test_collect_single_text_block() {
        let collector = TextBlockCollector::new();
        let mut timeline = Timeline::new();
        timeline.on_text_block(collector.clone());

        // テキストブロックのイベントシーケンスをディスパッチ
        timeline.dispatch(&Event::text_block_start(0));
        timeline.dispatch(&Event::text_delta(0, "Hello, "));
        timeline.dispatch(&Event::text_delta(0, "World!"));
        timeline.dispatch(&Event::text_block_stop(0, None));

        // 収集されたテキストを確認
        let texts = collector.take_collected();
        assert_eq!(texts.len(), 1);
        assert_eq!(texts[0], "Hello, World!");
    }

    /// TextBlockCollectorが複数のテキストブロックを正しく収集することを確認
    #[test]
    fn test_collect_multiple_text_blocks() {
        let collector = TextBlockCollector::new();
        let mut timeline = Timeline::new();
        timeline.on_text_block(collector.clone());

        // 1つ目のテキストブロック
        timeline.dispatch(&Event::text_block_start(0));
        timeline.dispatch(&Event::text_delta(0, "First"));
        timeline.dispatch(&Event::text_block_stop(0, None));

        // 2つ目のテキストブロック
        timeline.dispatch(&Event::text_block_start(1));
        timeline.dispatch(&Event::text_delta(1, "Second"));
        timeline.dispatch(&Event::text_block_stop(1, None));

        let texts = collector.take_collected();
        assert_eq!(texts.len(), 2);
        assert_eq!(texts[0], "First");
        assert_eq!(texts[1], "Second");
    }
}
