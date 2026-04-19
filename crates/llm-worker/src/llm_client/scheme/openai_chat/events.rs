//! OpenAI SSEイベントパース

use crate::llm_client::{
    ClientError,
    event::{Event, StopReason, UsageEvent},
};
use serde::Deserialize;

use super::OpenAIScheme;

/// OpenAI Streaming Chat Response Chunk
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub(crate) struct ChatCompletionChunk {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub model: String,
    pub choices: Vec<ChunkChoice>,
    pub usage: Option<ChunkUsage>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub(crate) struct ChunkChoice {
    pub index: usize,
    pub delta: ChunkDelta,
    pub finish_reason: Option<String>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub(crate) struct ChunkDelta {
    pub role: Option<String>,
    pub content: Option<String>,
    pub tool_calls: Option<Vec<ChunkToolCall>>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub(crate) struct ChunkToolCall {
    pub index: usize,
    pub id: Option<String>,
    #[serde(rename = "type")]
    pub call_type: Option<String>,
    pub function: Option<ChunkFunction>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub(crate) struct ChunkFunction {
    pub name: Option<String>,
    pub arguments: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ChunkUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
}

impl OpenAIScheme {
    /// SSEデータのパースとEventへの変換
    ///
    /// OpenAI APIはBlockStartイベントを明示的に送信しない。
    /// Timeline層が暗黙的なBlockStartを処理する。
    pub fn parse_event(&self, data: &str) -> Result<Option<Vec<Event>>, ClientError> {
        if data == "[DONE]" {
            return Ok(None);
        }

        let chunk: ChatCompletionChunk =
            serde_json::from_str(data).map_err(|e| ClientError::Api {
                status: None,
                code: Some("parse_error".to_string()),
                message: format!("Failed to parse SSE data: {} -> {}", e, data),
            })?;

        let mut events = Vec::new();

        // Usage handling
        if let Some(usage) = chunk.usage {
            events.push(Event::Usage(UsageEvent {
                input_tokens: Some(usage.prompt_tokens),
                output_tokens: Some(usage.completion_tokens),
                total_tokens: Some(usage.total_tokens),
                cache_read_input_tokens: None,
                cache_creation_input_tokens: None,
            }));
        }

        for choice in chunk.choices {
            // Text Content Delta
            if let Some(content) = choice.delta.content {
                // OpenAI APIはBlockStartを送らないため、デルタのみを発行
                // Timeline層が暗黙的なBlockStartを処理する
                events.push(Event::text_delta(choice.index, content));
            }

            // Tool Call Delta
            if let Some(tool_calls) = choice.delta.tool_calls {
                for tool_call in tool_calls {
                    // Start of tool call (has ID)
                    if let Some(id) = tool_call.id {
                        let name = tool_call
                            .function
                            .as_ref()
                            .and_then(|f| f.name.clone())
                            .unwrap_or_default();
                        events.push(Event::tool_use_start(tool_call.index, id, name));
                    }

                    // Arguments delta
                    if let Some(function) = tool_call.function {
                        if let Some(args) = function.arguments {
                            if !args.is_empty() {
                                events.push(Event::tool_input_delta(tool_call.index, args));
                            }
                        }
                    }
                }
            }

            // Finish Reason
            if let Some(finish_reason) = choice.finish_reason {
                let stop_reason = match finish_reason.as_str() {
                    "stop" => Some(StopReason::EndTurn),
                    "length" => Some(StopReason::MaxTokens),
                    "tool_calls" | "function_call" => Some(StopReason::ToolUse),
                    _ => Some(StopReason::EndTurn),
                };

                let is_tool_finish =
                    finish_reason == "tool_calls" || finish_reason == "function_call";

                if is_tool_finish {
                    // ツール呼び出し終了
                    // Note: OpenAIはどのツールが終了したか明示しないため、
                    // Timeline層で適切に処理する必要がある
                } else {
                    // テキスト終了
                    events.push(Event::text_block_stop(choice.index, stop_reason));
                }
            }
        }

        if events.is_empty() {
            Ok(None)
        } else {
            Ok(Some(events))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm_client::event::DeltaContent;

    #[test]
    fn test_parse_text_delta() {
        let scheme = OpenAIScheme::new();
        let data = r#"{"id":"chatcmpl-123","object":"chat.completion.chunk","created":1694268190,"model":"gpt-4o","choices":[{"index":0,"delta":{"content":"Hello"},"finish_reason":null}]}"#;

        let events = scheme.parse_event(data).unwrap().unwrap();
        // OpenAIはBlockStartを発行しないため、デルタのみ
        assert_eq!(events.len(), 1);

        if let Event::BlockDelta(delta) = &events[0] {
            assert_eq!(delta.index, 0);
            if let DeltaContent::Text(text) = &delta.delta {
                assert_eq!(text, "Hello");
            } else {
                panic!("Expected text delta");
            }
        } else {
            panic!("Expected BlockDelta");
        }
    }

    #[test]
    fn test_parse_tool_call() {
        let scheme = OpenAIScheme::new();
        // Start of tool call
        let data_start = r#"{"id":"chatcmpl-123","object":"chat.completion.chunk","created":1694268190,"model":"gpt-4o","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_abc","type":"function","function":{"name":"get_weather","arguments":""}}]},"finish_reason":null}]}"#;

        let events = scheme.parse_event(data_start).unwrap().unwrap();
        assert_eq!(events.len(), 1);
        if let Event::BlockStart(start) = &events[0] {
            assert_eq!(start.index, 0);
            if let crate::llm_client::event::BlockMetadata::ToolUse { id, name } = &start.metadata {
                assert_eq!(id, "call_abc");
                assert_eq!(name, "get_weather");
            } else {
                panic!("Expected ToolUse metadata");
            }
        }

        // Tool arguments delta
        let data_arg = r#"{"id":"chatcmpl-123","object":"chat.completion.chunk","created":1694268190,"model":"gpt-4o","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{}}"}}]},"finish_reason":null}]}"#;
        let events = scheme.parse_event(data_arg).unwrap().unwrap();
        assert_eq!(events.len(), 1);
        if let Event::BlockDelta(delta) = &events[0] {
            if let DeltaContent::InputJson(json) = &delta.delta {
                assert_eq!(json, "{}}");
            } else {
                panic!("Expected input json delta");
            }
        }
    }
}
