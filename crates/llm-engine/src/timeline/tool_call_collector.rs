//! ToolCallCollector - ツール呼び出し収集用ハンドラ
//!
//! TimelineのToolUseBlockHandler として登録され、
//! ストリーム中のToolUseブロックを収集する。

use crate::{
    handler::{Handler, ToolUseBlockEvent, ToolUseBlockKind},
    llm_client::types::parse_tool_arguments,
    tool::ToolCall,
};
use std::sync::{Arc, Mutex};

/// ToolUseブロックから収集したツール呼び出し情報を保持
///
/// ToolCallCollectorのHandler実装で使用するスコープ型
#[derive(Debug, Default)]
pub struct CollectorState {
    /// 現在のツール呼び出し情報 (ブロック進行中)
    current_id: Option<String>,
    current_name: Option<String>,
    /// 蓄積中のJSON入力
    input_json_buffer: String,
}

/// ToolCallCollector - ToolUseブロックハンドラ
///
/// Timelineに登録してToolUseブロックイベントを受信し、
/// 完了したToolCallを収集する。
#[derive(Clone)]
pub struct ToolCallCollector {
    /// 収集されたToolCall
    collected: Arc<Mutex<Vec<ToolCall>>>,
}

impl ToolCallCollector {
    /// 新しいToolCallCollectorを作成
    pub fn new() -> Self {
        Self {
            collected: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// 収集されたToolCallを取得してクリア
    pub fn take_collected(&self) -> Vec<ToolCall> {
        let mut guard = self.collected.lock().unwrap();
        std::mem::take(&mut *guard)
    }

    /// 収集されたToolCallの参照を取得
    pub fn collected(&self) -> Vec<ToolCall> {
        self.collected.lock().unwrap().clone()
    }

    /// 収集されたToolCallがあるかどうか
    pub fn has_pending_calls(&self) -> bool {
        !self.collected.lock().unwrap().is_empty()
    }

    /// 収集をクリア
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
                scope.input_json_buffer.clear();
            }
            ToolUseBlockEvent::InputJsonDelta(delta) => {
                scope.input_json_buffer.push_str(delta);
            }
            ToolUseBlockEvent::Stop(_stop) => {
                // ブロック完了時にToolCallを確定
                if let (Some(id), Some(name)) = (scope.current_id.take(), scope.current_name.take())
                {
                    let input = parse_tool_arguments(&scope.input_json_buffer);

                    let tool_call = ToolCall { id, name, input };

                    self.collected.lock().unwrap().push(tool_call);
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

        // ToolUseブロックのイベントシーケンスをディスパッチ
        timeline.dispatch(&Event::tool_use_start(0, "tool_123", "get_weather"));
        timeline.dispatch(&Event::tool_input_delta(0, r#"{"city":"#));
        timeline.dispatch(&Event::tool_input_delta(0, r#""Tokyo"}"#));
        timeline.dispatch(&Event::tool_use_stop(0));

        // 収集されたToolCallを確認
        let calls = collector.take_collected();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "tool_123");
        assert_eq!(calls[0].name, "get_weather");
        assert_eq!(calls[0].input["city"], "Tokyo");
    }

    #[test]
    fn test_collect_empty_buffer_returns_object() {
        // 引数なしツール呼び出し: input_json_delta が一度も来ないケース
        let collector = ToolCallCollector::new();
        let mut timeline = Timeline::new();
        timeline.on_tool_use_block(collector.clone());

        timeline.dispatch(&Event::tool_use_start(0, "tool_empty", "ListWorkers"));
        timeline.dispatch(&Event::tool_use_stop(0));

        let calls = collector.take_collected();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "tool_empty");
        assert_eq!(calls[0].name, "ListWorkers");
        assert!(calls[0].input.is_object());
        assert_eq!(
            calls[0].input,
            serde_json::Value::Object(serde_json::Map::new())
        );
    }

    #[test]
    fn test_collect_multiple_tool_calls() {
        let collector = ToolCallCollector::new();
        let mut timeline = Timeline::new();
        timeline.on_tool_use_block(collector.clone());

        // 1つ目のToolCall
        timeline.dispatch(&Event::tool_use_start(0, "call_1", "tool_a"));
        timeline.dispatch(&Event::tool_input_delta(0, r#"{"a":1}"#));
        timeline.dispatch(&Event::tool_use_stop(0));

        // 2つ目のToolCall
        timeline.dispatch(&Event::tool_use_start(1, "call_2", "tool_b"));
        timeline.dispatch(&Event::tool_input_delta(1, r#"{"b":2}"#));
        timeline.dispatch(&Event::tool_use_stop(1));

        let calls = collector.take_collected();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "tool_a");
        assert_eq!(calls[1].name, "tool_b");
    }
}
