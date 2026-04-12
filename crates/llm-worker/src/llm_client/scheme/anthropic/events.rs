//! Anthropic SSEイベントパース
//!
//! Anthropic Messages APIのSSEイベントをパースし、統一Event型に変換

use crate::llm_client::{
    ClientError,
    event::{
        BlockDelta, BlockMetadata, BlockStart, BlockStop, BlockType, DeltaContent, ErrorEvent,
        Event, PingEvent, ResponseStatus, StatusEvent, UsageEvent,
    },
};
use serde::Deserialize;

use super::AnthropicScheme;

/// Anthropic SSEイベントタイプ
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AnthropicEventType {
    MessageStart,
    ContentBlockStart,
    ContentBlockDelta,
    ContentBlockStop,
    MessageDelta,
    MessageStop,
    Ping,
    Error,
}

impl AnthropicEventType {
    /// イベントタイプ文字列からパース
    pub(crate) fn parse(s: &str) -> Option<Self> {
        match s {
            "message_start" => Some(Self::MessageStart),
            "content_block_start" => Some(Self::ContentBlockStart),
            "content_block_delta" => Some(Self::ContentBlockDelta),
            "content_block_stop" => Some(Self::ContentBlockStop),
            "message_delta" => Some(Self::MessageDelta),
            "message_stop" => Some(Self::MessageStop),
            "ping" => Some(Self::Ping),
            "error" => Some(Self::Error),
            _ => None,
        }
    }
}

// ============================================================================
// SSEイベントのJSON構造
// ============================================================================

/// message_start イベント
#[derive(Debug, Deserialize)]
pub(crate) struct MessageStartEvent {
    pub message: MessageStartMessage,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub(crate) struct MessageStartMessage {
    pub id: String,
    pub model: String,
    pub usage: Option<UsageData>,
}

/// content_block_start イベント
#[derive(Debug, Deserialize)]
pub(crate) struct ContentBlockStartEvent {
    pub index: usize,
    pub content_block: ContentBlock,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub(crate) enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "thinking")]
    Thinking { thinking: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
}

/// content_block_delta イベント
#[derive(Debug, Deserialize)]
pub(crate) struct ContentBlockDeltaEvent {
    pub index: usize,
    pub delta: DeltaBlock,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub(crate) enum DeltaBlock {
    #[serde(rename = "text_delta")]
    TextDelta { text: String },
    #[serde(rename = "thinking_delta")]
    ThinkingDelta { thinking: String },
    #[serde(rename = "input_json_delta")]
    InputJsonDelta { partial_json: String },
    #[serde(rename = "signature_delta")]
    SignatureDelta { signature: String },
}

/// content_block_stop イベント
#[derive(Debug, Deserialize)]
pub(crate) struct ContentBlockStopEvent {
    pub index: usize,
}

/// message_delta イベント
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub(crate) struct MessageDeltaEvent {
    pub delta: MessageDeltaData,
    pub usage: Option<UsageData>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub(crate) struct MessageDeltaData {
    pub stop_reason: Option<String>,
    pub stop_sequence: Option<String>,
}

/// 使用量データ
#[derive(Debug, Deserialize)]
pub(crate) struct UsageData {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_read_input_tokens: Option<u64>,
    pub cache_creation_input_tokens: Option<u64>,
}

/// エラーイベント
#[derive(Debug, Deserialize)]
pub(crate) struct ErrorEventData {
    pub error: ErrorDetail,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ErrorDetail {
    #[serde(rename = "type")]
    pub error_type: String,
    pub message: String,
}

// ============================================================================
// イベント変換
// ============================================================================

impl AnthropicScheme {
    /// SSEイベントをEvent型に変換
    ///
    /// # Arguments
    /// * `event_type` - SSEイベントタイプ
    /// * `data` - イベントデータJSON文字列
    ///
    /// # Returns
    /// * `Ok(Some(Event))` - 変換成功
    /// * `Ok(None)` - イベントを無視（unknown event等）
    /// * `Err(ClientError)` - パースエラー
    pub(crate) fn parse_event(
        &self,
        event_type: &str,
        data: &str,
    ) -> Result<Option<Event>, ClientError> {
        let Some(event_type) = AnthropicEventType::parse(event_type) else {
            // Unknown event type, ignore
            return Ok(None);
        };

        match event_type {
            AnthropicEventType::MessageStart => {
                let event: MessageStartEvent = serde_json::from_str(data)?;
                // message_start時にUsageイベントがあれば出力
                if let Some(usage) = event.message.usage {
                    return Ok(Some(Event::Usage(self.convert_usage(&usage))));
                }
                // Statusイベントとして開始を通知
                Ok(Some(Event::Status(StatusEvent {
                    status: ResponseStatus::Started,
                })))
            }
            AnthropicEventType::ContentBlockStart => {
                let event: ContentBlockStartEvent = serde_json::from_str(data)?;
                Ok(Some(self.convert_block_start(&event)))
            }
            AnthropicEventType::ContentBlockDelta => {
                let event: ContentBlockDeltaEvent = serde_json::from_str(data)?;
                Ok(self.convert_block_delta(&event))
            }
            AnthropicEventType::ContentBlockStop => {
                let event: ContentBlockStopEvent = serde_json::from_str(data)?;
                // Note: BlockStopにはblock_typeが必要だが、AnthropicはStopイベントに含めない
                // Timeline層がBlockStartを追跡して正しいblock_typeを知る
                Ok(Some(Event::BlockStop(BlockStop {
                    index: event.index,
                    block_type: BlockType::Text, // Timeline層で上書きされる
                    stop_reason: None,
                })))
            }
            AnthropicEventType::MessageDelta => {
                let event: MessageDeltaEvent = serde_json::from_str(data)?;
                // Usage情報があれば出力
                if let Some(usage) = event.usage {
                    return Ok(Some(Event::Usage(self.convert_usage(&usage))));
                }
                Ok(None)
            }
            AnthropicEventType::MessageStop => Ok(Some(Event::Status(StatusEvent {
                status: ResponseStatus::Completed,
            }))),
            AnthropicEventType::Ping => Ok(Some(Event::Ping(PingEvent { timestamp: None }))),
            AnthropicEventType::Error => {
                let event: ErrorEventData = serde_json::from_str(data)?;
                Ok(Some(Event::Error(ErrorEvent {
                    code: Some(event.error.error_type),
                    message: event.error.message,
                })))
            }
        }
    }

    fn convert_block_start(&self, event: &ContentBlockStartEvent) -> Event {
        let (block_type, metadata) = match &event.content_block {
            ContentBlock::Text { .. } => (BlockType::Text, BlockMetadata::Text),
            ContentBlock::Thinking { .. } => (BlockType::Thinking, BlockMetadata::Thinking),
            ContentBlock::ToolUse { id, name, .. } => (
                BlockType::ToolUse,
                BlockMetadata::ToolUse {
                    id: id.clone(),
                    name: name.clone(),
                },
            ),
        };

        Event::BlockStart(BlockStart {
            index: event.index,
            block_type,
            metadata,
        })
    }

    fn convert_block_delta(&self, event: &ContentBlockDeltaEvent) -> Option<Event> {
        let delta = match &event.delta {
            DeltaBlock::TextDelta { text } => DeltaContent::Text(text.clone()),
            DeltaBlock::ThinkingDelta { thinking } => DeltaContent::Thinking(thinking.clone()),
            DeltaBlock::InputJsonDelta { partial_json } => {
                DeltaContent::InputJson(partial_json.clone())
            }
            DeltaBlock::SignatureDelta { .. } => {
                // signature_delta は無視
                return None;
            }
        };

        Some(Event::BlockDelta(BlockDelta {
            index: event.index,
            delta,
        }))
    }

    fn convert_usage(&self, usage: &UsageData) -> UsageEvent {
        // Anthropic の `input_tokens` は **キャッシュ外** の入力トークンのみで、
        // プロンプト全長は input_tokens + cache_read + cache_creation。
        // UsageEvent の `input_tokens` には「占有量（プロンプト全長）」を載せる
        // 規約に合わせて、ここでキャッシュ分を足し込む。
        // cache_read_input_tokens / cache_creation_input_tokens は内訳として
        // 別フィールドに残るので、料金計算側で `input - cache_read - cache_creation`
        // により非キャッシュ入力分は逆算可能。
        let raw_input = usage.input_tokens.unwrap_or(0);
        let cache_read = usage.cache_read_input_tokens.unwrap_or(0);
        let cache_creation = usage.cache_creation_input_tokens.unwrap_or(0);
        let input_total = raw_input + cache_read + cache_creation;
        let output = usage.output_tokens.unwrap_or(0);

        UsageEvent {
            input_tokens: usage.input_tokens.map(|_| input_total),
            output_tokens: usage.output_tokens,
            total_tokens: Some(input_total + output),
            cache_read_input_tokens: usage.cache_read_input_tokens,
            cache_creation_input_tokens: usage.cache_creation_input_tokens,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_message_start() {
        let scheme = AnthropicScheme::new();
        let data = r#"{"type":"message_start","message":{"id":"msg_123","type":"message","role":"assistant","content":[],"model":"claude-sonnet-4-20250514","stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":10,"output_tokens":0}}}"#;

        let event = scheme.parse_event("message_start", data).unwrap().unwrap();
        match event {
            Event::Usage(u) => {
                // キャッシュなしなので input_total = raw_input = 10
                assert_eq!(u.input_tokens, Some(10));
            }
            _ => panic!("Expected Usage event"),
        }
    }

    #[test]
    fn test_convert_usage_includes_cache_in_input_total() {
        // Anthropic の input_tokens はキャッシュ外のみで、占有量は
        // input + cache_read + cache_creation。
        // UsageEvent.input_tokens は占有量に正規化される。
        let scheme = AnthropicScheme::new();
        let usage = UsageData {
            input_tokens: Some(100),
            output_tokens: Some(50),
            cache_read_input_tokens: Some(800),
            cache_creation_input_tokens: Some(200),
        };
        let event = scheme.convert_usage(&usage);
        // 100 + 800 + 200 = 1100
        assert_eq!(event.input_tokens, Some(1100));
        assert_eq!(event.cache_read_input_tokens, Some(800));
        assert_eq!(event.cache_creation_input_tokens, Some(200));
        assert_eq!(event.total_tokens, Some(1150));
    }

    #[test]
    fn test_parse_content_block_start_text() {
        let scheme = AnthropicScheme::new();
        let data =
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#;

        let event = scheme
            .parse_event("content_block_start", data)
            .unwrap()
            .unwrap();
        match event {
            Event::BlockStart(s) => {
                assert_eq!(s.index, 0);
                assert_eq!(s.block_type, BlockType::Text);
            }
            _ => panic!("Expected BlockStart event"),
        }
    }

    #[test]
    fn test_parse_content_block_delta_text() {
        let scheme = AnthropicScheme::new();
        let data = r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#;

        let event = scheme
            .parse_event("content_block_delta", data)
            .unwrap()
            .unwrap();
        match event {
            Event::BlockDelta(d) => {
                assert_eq!(d.index, 0);
                match d.delta {
                    DeltaContent::Text(t) => assert_eq!(t, "Hello"),
                    _ => panic!("Expected Text delta"),
                }
            }
            _ => panic!("Expected BlockDelta event"),
        }
    }

    #[test]
    fn test_parse_tool_use_start() {
        let scheme = AnthropicScheme::new();
        let data = r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_123","name":"get_weather","input":{}}}"#;

        let event = scheme
            .parse_event("content_block_start", data)
            .unwrap()
            .unwrap();
        match event {
            Event::BlockStart(s) => {
                assert_eq!(s.block_type, BlockType::ToolUse);
                match s.metadata {
                    BlockMetadata::ToolUse { id, name } => {
                        assert_eq!(id, "toolu_123");
                        assert_eq!(name, "get_weather");
                    }
                    _ => panic!("Expected ToolUse metadata"),
                }
            }
            _ => panic!("Expected BlockStart event"),
        }
    }

    #[test]
    fn test_parse_ping() {
        let scheme = AnthropicScheme::new();
        let data = r#"{"type":"ping"}"#;

        let event = scheme.parse_event("ping", data).unwrap().unwrap();
        match event {
            Event::Ping(_) => {}
            _ => panic!("Expected Ping event"),
        }
    }
}
