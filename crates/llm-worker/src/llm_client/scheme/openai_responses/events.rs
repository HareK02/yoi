//! OpenAI Responses API の SSE イベントパース
//!
//! `response.*` 名前空間の SSE を共通の [`Event`](crate::llm_client::event::Event)
//! に変換する。Responses の (output_index, content_index) 2 次元座標と
//! insomnia 側 1 次元 `BlockStart/Delta/Stop::index` のマッピングは
//! [`OpenAIResponsesState`] が保持する。

use std::collections::HashMap;

use serde::Deserialize;

use crate::llm_client::{
    ClientError,
    event::{
        BlockDelta, BlockMetadata, BlockStart, BlockStop, BlockType, DeltaContent, ErrorEvent,
        Event, ReasoningItemEvent, ResponseStatus, StatusEvent, UsageEvent,
    },
};

/// SSE パース中の座標 → flat block index マップ。
#[derive(Debug, Default)]
pub struct OpenAIResponsesState {
    slots: HashMap<SlotKey, SlotInfo>,
    next_index: usize,
    /// 蓄積中の reasoning output_item。`output_item.added`(Reasoning) で
    /// 確保し、`reasoning_text.delta` / `reasoning_summary_text.delta` で
    /// 蓄積、`output_item.done`(Reasoning) で `Event::ReasoningItem` を
    /// 発火してエントリを除去する。
    pending_reasoning: HashMap<usize, PendingReasoning>,
}

/// 1 つの reasoning output_item の蓄積バッファ。
#[derive(Debug, Default)]
struct PendingReasoning {
    id: Option<String>,
    /// `reasoning_text.delta` の累積。複数 content_part あれば順に concat。
    text: String,
    /// `reasoning_summary_text.delta` を summary_index 順に蓄積。
    summary: Vec<String>,
}

impl OpenAIResponsesState {
    fn allocate(&mut self, key: SlotKey, block_type: BlockType) -> SlotInfo {
        let info = SlotInfo {
            flat_index: self.next_index,
            block_type,
        };
        self.next_index += 1;
        self.slots.insert(key, info);
        info
    }

    /// 既存 slot を取得。無ければ `block_type` で暗黙に確保し、
    /// 新規確保したかを併せて返す。delta 先行 / content_part.added が
    /// 抜けたときの防御。
    fn get_or_allocate(&mut self, key: SlotKey, block_type: BlockType) -> (SlotInfo, bool) {
        if let Some(info) = self.slots.get(&key).copied() {
            (info, false)
        } else {
            (self.allocate(key, block_type), true)
        }
    }

    fn ensure_reasoning(&mut self, output_index: usize) -> &mut PendingReasoning {
        self.pending_reasoning.entry(output_index).or_default()
    }

    fn extend_reasoning_summary(&mut self, output_index: usize, summary_index: usize, text: &str) {
        let entry = self.ensure_reasoning(output_index);
        if entry.summary.len() <= summary_index {
            entry.summary.resize(summary_index + 1, String::new());
        }
        entry.summary[summary_index].push_str(text);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum SlotKey {
    /// tool_use (function_call / custom_tool_call) — output_item 全体で 1 block
    OutputItem(usize),
    /// message の output_text / reasoning item の reasoning_text
    ContentPart { output: usize, content: usize },
    /// reasoning item の summary_text (summary_index)
    Summary { output: usize, summary: usize },
}

#[derive(Debug, Clone, Copy)]
struct SlotInfo {
    flat_index: usize,
    block_type: BlockType,
}

// ============================================================================
// SSE イベントの JSON 構造
// ============================================================================

#[derive(Debug, Deserialize)]
struct OutputItemAdded {
    output_index: usize,
    item: OutputItem,
}

#[derive(Debug, Deserialize)]
struct OutputItemDone {
    output_index: usize,
    #[allow(dead_code)]
    item: OutputItem,
}

/// `response.output_item.added/done` の `item`。
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum OutputItem {
    Message {
        #[allow(dead_code)]
        id: Option<String>,
    },
    Reasoning {
        #[serde(default)]
        id: Option<String>,
        /// `output_item.done` で初めて埋まる。`include=["reasoning.encrypted_content"]`
        /// 指定時に opaque blob が乗る。
        #[serde(default)]
        encrypted_content: Option<String>,
    },
    FunctionCall {
        #[allow(dead_code)]
        #[serde(default)]
        id: Option<String>,
        call_id: String,
        name: String,
    },
    CustomToolCall {
        #[allow(dead_code)]
        #[serde(default)]
        id: Option<String>,
        call_id: String,
        name: String,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
struct ContentPartAdded {
    output_index: usize,
    content_index: usize,
    part: ContentPart,
}

#[derive(Debug, Deserialize)]
struct ContentPartDone {
    output_index: usize,
    content_index: usize,
    #[allow(dead_code)]
    part: ContentPart,
}

/// `response.content_part.added/done` の `part`。
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ContentPart {
    OutputText {
        #[allow(dead_code)]
        #[serde(default)]
        text: String,
    },
    ReasoningText {
        #[allow(dead_code)]
        #[serde(default)]
        text: String,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
struct OutputTextDelta {
    output_index: usize,
    content_index: usize,
    delta: String,
}

#[derive(Debug, Deserialize)]
struct ReasoningTextDelta {
    output_index: usize,
    content_index: usize,
    delta: String,
}

#[derive(Debug, Deserialize)]
struct ReasoningSummaryPartAdded {
    output_index: usize,
    summary_index: usize,
    #[allow(dead_code)]
    #[serde(default)]
    part: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct ReasoningSummaryTextDelta {
    output_index: usize,
    summary_index: usize,
    delta: String,
}

#[derive(Debug, Deserialize)]
struct ReasoningSummaryPartDone {
    output_index: usize,
    summary_index: usize,
}

#[derive(Debug, Deserialize)]
struct FunctionCallArgumentsDelta {
    output_index: usize,
    delta: String,
}

#[derive(Debug, Deserialize)]
struct CustomToolCallInputDelta {
    output_index: usize,
    delta: String,
}

#[derive(Debug, Deserialize)]
struct ResponseCompleted {
    response: CompletedResponse,
}

#[derive(Debug, Deserialize)]
struct CompletedResponse {
    #[serde(default)]
    usage: Option<ResponsesUsage>,
}

#[derive(Debug, Deserialize)]
struct ResponsesUsage {
    #[serde(default)]
    input_tokens: Option<u64>,
    #[serde(default)]
    output_tokens: Option<u64>,
    #[serde(default)]
    total_tokens: Option<u64>,
    /// `input_tokens` の内訳。`cached_tokens` がプロンプトキャッシュヒット分。
    #[serde(default)]
    input_tokens_details: Option<InputTokensDetails>,
}

#[derive(Debug, Deserialize)]
struct InputTokensDetails {
    #[serde(default)]
    cached_tokens: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct ResponseFailed {
    response: FailedResponse,
}

#[derive(Debug, Deserialize)]
struct FailedResponse {
    #[serde(default)]
    error: Option<ErrorDetail>,
}

#[derive(Debug, Deserialize)]
struct ErrorDetail {
    #[serde(rename = "type", default)]
    error_type: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TopLevelError {
    #[serde(default)]
    message: Option<String>,
    #[serde(rename = "type", default)]
    error_type: Option<String>,
    #[serde(default)]
    code: Option<String>,
}

// ============================================================================
// parse entry point
// ============================================================================

/// SSE フレーム 1 件をパースし、0 個以上の [`Event`] に変換する。
///
/// `event_type` は SSE の `event:` フィールド。未対応の event は
/// 静かに無視する。`data` が JSON でない / 必要なフィールドが抜けて
/// いる等は [`ClientError::Api`] で返す。
pub(crate) fn parse_sse(
    event_type: &str,
    data: &str,
    state: &mut OpenAIResponsesState,
) -> Result<Vec<Event>, ClientError> {
    match event_type {
        "response.created" => Ok(vec![Event::Status(StatusEvent {
            status: ResponseStatus::Started,
        })]),

        "response.completed" => {
            let ev: ResponseCompleted = from_json(data)?;
            let mut out = Vec::new();
            if let Some(usage) = ev.response.usage {
                out.push(Event::Usage(UsageEvent {
                    input_tokens: usage.input_tokens,
                    output_tokens: usage.output_tokens,
                    total_tokens: usage.total_tokens.or_else(|| {
                        Some(usage.input_tokens.unwrap_or(0) + usage.output_tokens.unwrap_or(0))
                    }),
                    cache_read_input_tokens: usage
                        .input_tokens_details
                        .and_then(|d| d.cached_tokens),
                    // Responses API は cache 書き込みを別計上しない（input_tokens に含まれる）
                    cache_creation_input_tokens: None,
                }));
            }
            out.push(Event::Status(StatusEvent {
                status: ResponseStatus::Completed,
            }));
            Ok(out)
        }

        "response.failed" | "response.incomplete" => {
            let ev: ResponseFailed = from_json(data)?;
            let (code, message) = match ev.response.error {
                Some(err) => (err.error_type, err.message.unwrap_or_default()),
                None => (None, format!("response {event_type}")),
            };
            Ok(vec![
                Event::Error(ErrorEvent { code, message }),
                Event::Status(StatusEvent {
                    status: ResponseStatus::Failed,
                }),
            ])
        }

        "response.output_item.added" => {
            let ev: OutputItemAdded = from_json(data)?;
            match ev.item {
                OutputItem::FunctionCall { call_id, name, .. }
                | OutputItem::CustomToolCall { call_id, name, .. } => {
                    let info =
                        state.allocate(SlotKey::OutputItem(ev.output_index), BlockType::ToolUse);
                    Ok(vec![Event::BlockStart(BlockStart {
                        index: info.flat_index,
                        block_type: BlockType::ToolUse,
                        metadata: BlockMetadata::ToolUse { id: call_id, name },
                    })])
                }
                OutputItem::Reasoning { id, .. } => {
                    // wrapper を確保。中身の content_part / summary_part は
                    // 別 SlotKey で扱われ続ける（Streaming 表示は維持）。
                    let entry = state.ensure_reasoning(ev.output_index);
                    if id.is_some() {
                        entry.id = id;
                    }
                    Ok(Vec::new())
                }
                _ => Ok(Vec::new()),
            }
        }

        "response.output_item.done" => {
            let ev: OutputItemDone = from_json(data)?;
            // Reasoning wrapper の done で蓄積分を ReasoningItem として発火。
            // これは `slots` の OutputItem slot とは独立している
            // (FunctionCall は slots、Reasoning は pending_reasoning)。
            if let OutputItem::Reasoning {
                id,
                encrypted_content,
                ..
            } = ev.item
            {
                let mut pending = state
                    .pending_reasoning
                    .remove(&ev.output_index)
                    .unwrap_or_default();
                if pending.id.is_none() {
                    pending.id = id;
                }
                return Ok(vec![Event::ReasoningItem(ReasoningItemEvent {
                    id: pending.id,
                    text: pending.text,
                    summary: pending
                        .summary
                        .into_iter()
                        .filter(|s| !s.is_empty())
                        .collect(),
                    encrypted_content,
                    signature: None,
                })]);
            }
            if let Some(info) = state.slots.remove(&SlotKey::OutputItem(ev.output_index)) {
                Ok(vec![Event::BlockStop(BlockStop {
                    index: info.flat_index,
                    block_type: info.block_type,
                    stop_reason: None,
                })])
            } else {
                Ok(Vec::new())
            }
        }

        "response.content_part.added" => {
            let ev: ContentPartAdded = from_json(data)?;
            let (block_type, metadata) = match ev.part {
                ContentPart::OutputText { .. } => (BlockType::Text, BlockMetadata::Text),
                ContentPart::ReasoningText { .. } => (BlockType::Thinking, BlockMetadata::Thinking),
                ContentPart::Other => return Ok(Vec::new()),
            };
            let info = state.allocate(
                SlotKey::ContentPart {
                    output: ev.output_index,
                    content: ev.content_index,
                },
                block_type,
            );
            Ok(vec![Event::BlockStart(BlockStart {
                index: info.flat_index,
                block_type,
                metadata,
            })])
        }

        "response.content_part.done" => {
            let ev: ContentPartDone = from_json(data)?;
            if let Some(info) = state.slots.remove(&SlotKey::ContentPart {
                output: ev.output_index,
                content: ev.content_index,
            }) {
                Ok(vec![Event::BlockStop(BlockStop {
                    index: info.flat_index,
                    block_type: info.block_type,
                    stop_reason: None,
                })])
            } else {
                Ok(Vec::new())
            }
        }

        "response.output_text.delta" => {
            let ev: OutputTextDelta = from_json(data)?;
            Ok(ensure_and_delta(
                state,
                SlotKey::ContentPart {
                    output: ev.output_index,
                    content: ev.content_index,
                },
                BlockType::Text,
                BlockMetadata::Text,
                DeltaContent::Text(ev.delta),
            ))
        }

        "response.reasoning_text.delta" => {
            let ev: ReasoningTextDelta = from_json(data)?;
            // round-trip 用に蓄積
            state
                .ensure_reasoning(ev.output_index)
                .text
                .push_str(&ev.delta);
            Ok(ensure_and_delta(
                state,
                SlotKey::ContentPart {
                    output: ev.output_index,
                    content: ev.content_index,
                },
                BlockType::Thinking,
                BlockMetadata::Thinking,
                DeltaContent::Thinking(ev.delta),
            ))
        }

        "response.reasoning_summary_part.added" => {
            let ev: ReasoningSummaryPartAdded = from_json(data)?;
            let info = state.allocate(
                SlotKey::Summary {
                    output: ev.output_index,
                    summary: ev.summary_index,
                },
                BlockType::Thinking,
            );
            Ok(vec![Event::BlockStart(BlockStart {
                index: info.flat_index,
                block_type: BlockType::Thinking,
                metadata: BlockMetadata::Thinking,
            })])
        }

        "response.reasoning_summary_text.delta" => {
            let ev: ReasoningSummaryTextDelta = from_json(data)?;
            // round-trip 用に蓄積
            state.extend_reasoning_summary(ev.output_index, ev.summary_index, &ev.delta);
            Ok(ensure_and_delta(
                state,
                SlotKey::Summary {
                    output: ev.output_index,
                    summary: ev.summary_index,
                },
                BlockType::Thinking,
                BlockMetadata::Thinking,
                DeltaContent::Thinking(ev.delta),
            ))
        }

        "response.reasoning_summary_part.done" => {
            let ev: ReasoningSummaryPartDone = from_json(data)?;
            if let Some(info) = state.slots.remove(&SlotKey::Summary {
                output: ev.output_index,
                summary: ev.summary_index,
            }) {
                Ok(vec![Event::BlockStop(BlockStop {
                    index: info.flat_index,
                    block_type: info.block_type,
                    stop_reason: None,
                })])
            } else {
                Ok(Vec::new())
            }
        }

        "response.function_call_arguments.delta" => {
            let ev: FunctionCallArgumentsDelta = from_json(data)?;
            Ok(ensure_and_delta(
                state,
                SlotKey::OutputItem(ev.output_index),
                BlockType::ToolUse,
                BlockMetadata::ToolUse {
                    id: String::new(),
                    name: String::new(),
                },
                DeltaContent::InputJson(ev.delta),
            ))
        }

        "response.custom_tool_call_input.delta" => {
            let ev: CustomToolCallInputDelta = from_json(data)?;
            Ok(ensure_and_delta(
                state,
                SlotKey::OutputItem(ev.output_index),
                BlockType::ToolUse,
                BlockMetadata::ToolUse {
                    id: String::new(),
                    name: String::new(),
                },
                DeltaContent::InputJson(ev.delta),
            ))
        }

        "error" => {
            let ev: TopLevelError = from_json(data).unwrap_or(TopLevelError {
                message: Some(data.to_string()),
                error_type: None,
                code: None,
            });
            Ok(vec![Event::Error(ErrorEvent {
                code: ev.error_type.or(ev.code),
                message: ev.message.unwrap_or_default(),
            })])
        }

        // 未対応 / 情報系イベントは無視
        _ => Ok(Vec::new()),
    }
}

/// 対応する BlockStart がまだ発行されていなければ発行しつつ、delta を流す。
/// content_part.added を取りこぼしても delta 単独で復旧できるようにする。
fn ensure_and_delta(
    state: &mut OpenAIResponsesState,
    key: SlotKey,
    block_type: BlockType,
    metadata: BlockMetadata,
    delta: DeltaContent,
) -> Vec<Event> {
    let (info, just_created) = state.get_or_allocate(key, block_type);
    let mut out = Vec::with_capacity(2);
    if just_created {
        out.push(Event::BlockStart(BlockStart {
            index: info.flat_index,
            block_type,
            metadata,
        }));
    }
    out.push(Event::BlockDelta(BlockDelta {
        index: info.flat_index,
        delta,
    }));
    out
}

fn from_json<T: for<'de> Deserialize<'de>>(data: &str) -> Result<T, ClientError> {
    serde_json::from_str(data).map_err(|e| ClientError::Api {
        status: None,
        code: Some("parse_error".to_string()),
        message: format!("Failed to parse SSE data: {e}"),
        retry_after: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(event_type: &str, data: &str) -> (Vec<Event>, OpenAIResponsesState) {
        let mut state = OpenAIResponsesState::default();
        let events = parse_sse(event_type, data, &mut state).unwrap();
        (events, state)
    }

    fn with(state: &mut OpenAIResponsesState, event_type: &str, data: &str) -> Vec<Event> {
        parse_sse(event_type, data, state).unwrap()
    }

    #[test]
    fn created_emits_status_started() {
        let (events, _) = run("response.created", r#"{"response":{}}"#);
        assert!(matches!(
            events[0],
            Event::Status(StatusEvent {
                status: ResponseStatus::Started
            })
        ));
    }

    #[test]
    fn completed_emits_usage_and_status() {
        let data =
            r#"{"response":{"usage":{"input_tokens":10,"output_tokens":20,"total_tokens":30}}}"#;
        let (events, _) = run("response.completed", data);
        assert!(matches!(events[0], Event::Usage(_)));
        assert!(matches!(
            events[1],
            Event::Status(StatusEvent {
                status: ResponseStatus::Completed
            })
        ));
        if let Event::Usage(u) = &events[0] {
            assert_eq!(u.input_tokens, Some(10));
            assert_eq!(u.output_tokens, Some(20));
            assert_eq!(u.total_tokens, Some(30));
            assert_eq!(u.cache_read_input_tokens, None);
            assert_eq!(u.cache_creation_input_tokens, None);
        }
    }

    #[test]
    fn completed_extracts_cached_tokens_from_input_tokens_details() {
        let data = r#"{"response":{"usage":{
            "input_tokens":12345,
            "input_tokens_details":{"cached_tokens":11000},
            "output_tokens":50,
            "total_tokens":12395
        }}}"#;
        let (events, _) = run("response.completed", data);
        let Event::Usage(u) = &events[0] else {
            panic!("expected usage")
        };
        assert_eq!(u.input_tokens, Some(12345));
        assert_eq!(u.output_tokens, Some(50));
        assert_eq!(u.total_tokens, Some(12395));
        assert_eq!(u.cache_read_input_tokens, Some(11000));
        // OpenAI Responses は cache 書き込みを別計上しない
        assert_eq!(u.cache_creation_input_tokens, None);
    }

    #[test]
    fn text_stream_start_delta_stop() {
        let mut state = OpenAIResponsesState::default();
        // output_item.added (message) → 無視
        with(
            &mut state,
            "response.output_item.added",
            r#"{"output_index":0,"item":{"type":"message","id":"m1"}}"#,
        );
        // content_part.added (output_text) → BlockStart(Text)
        let ev = with(
            &mut state,
            "response.content_part.added",
            r#"{"output_index":0,"content_index":0,"item_id":"m1","part":{"type":"output_text","text":""}}"#,
        );
        assert_eq!(ev.len(), 1);
        assert!(matches!(ev[0], Event::BlockStart(_)));
        // delta
        let ev = with(
            &mut state,
            "response.output_text.delta",
            r#"{"output_index":0,"content_index":0,"item_id":"m1","delta":"hi"}"#,
        );
        assert_eq!(ev.len(), 1);
        if let Event::BlockDelta(d) = &ev[0] {
            assert!(matches!(&d.delta, DeltaContent::Text(t) if t == "hi"));
        } else {
            panic!("expected delta");
        }
        // content_part.done → BlockStop
        let ev = with(
            &mut state,
            "response.content_part.done",
            r#"{"output_index":0,"content_index":0,"item_id":"m1","part":{"type":"output_text","text":"hi"}}"#,
        );
        assert_eq!(ev.len(), 1);
        if let Event::BlockStop(s) = &ev[0] {
            assert_eq!(s.block_type, BlockType::Text);
        } else {
            panic!("expected stop");
        }
    }

    #[test]
    fn function_call_start_delta_stop() {
        let mut state = OpenAIResponsesState::default();
        // output_item.added (function_call) → BlockStart(ToolUse, id, name)
        let ev = with(
            &mut state,
            "response.output_item.added",
            r#"{"output_index":1,"item":{"type":"function_call","id":"fc1","call_id":"call_abc","name":"get_weather"}}"#,
        );
        assert_eq!(ev.len(), 1);
        if let Event::BlockStart(s) = &ev[0] {
            assert_eq!(s.block_type, BlockType::ToolUse);
            if let BlockMetadata::ToolUse { id, name } = &s.metadata {
                assert_eq!(id, "call_abc");
                assert_eq!(name, "get_weather");
            } else {
                panic!("expected ToolUse metadata");
            }
        } else {
            panic!("expected BlockStart");
        }
        // arguments delta
        let ev = with(
            &mut state,
            "response.function_call_arguments.delta",
            r#"{"output_index":1,"item_id":"fc1","delta":"{\"x\":"}"#,
        );
        assert_eq!(ev.len(), 1);
        if let Event::BlockDelta(d) = &ev[0] {
            assert!(matches!(&d.delta, DeltaContent::InputJson(j) if j == "{\"x\":"));
        }
        // output_item.done → BlockStop
        let ev = with(
            &mut state,
            "response.output_item.done",
            r#"{"output_index":1,"item":{"type":"function_call","call_id":"call_abc","name":"get_weather","arguments":"{\"x\":1}"}}"#,
        );
        assert_eq!(ev.len(), 1);
        assert!(matches!(ev[0], Event::BlockStop(_)));
    }

    #[test]
    fn custom_tool_call_input_delta_parsed() {
        let mut state = OpenAIResponsesState::default();
        with(
            &mut state,
            "response.output_item.added",
            r#"{"output_index":0,"item":{"type":"custom_tool_call","id":"ct1","call_id":"call_xyz","name":"custom"}}"#,
        );
        let ev = with(
            &mut state,
            "response.custom_tool_call_input.delta",
            r#"{"output_index":0,"item_id":"ct1","delta":"raw"}"#,
        );
        assert_eq!(ev.len(), 1);
        if let Event::BlockDelta(d) = &ev[0] {
            assert!(matches!(&d.delta, DeltaContent::InputJson(j) if j == "raw"));
        } else {
            panic!("expected delta");
        }
    }

    #[test]
    fn reasoning_text_delta_emits_thinking() {
        let mut state = OpenAIResponsesState::default();
        with(
            &mut state,
            "response.content_part.added",
            r#"{"output_index":0,"content_index":0,"item_id":"r1","part":{"type":"reasoning_text","text":""}}"#,
        );
        let ev = with(
            &mut state,
            "response.reasoning_text.delta",
            r#"{"output_index":0,"content_index":0,"item_id":"r1","delta":"think"}"#,
        );
        if let Event::BlockDelta(d) = &ev[0] {
            assert!(matches!(&d.delta, DeltaContent::Thinking(t) if t == "think"));
        } else {
            panic!("expected thinking delta");
        }
    }

    #[test]
    fn reasoning_summary_start_delta_stop() {
        let mut state = OpenAIResponsesState::default();
        let ev = with(
            &mut state,
            "response.reasoning_summary_part.added",
            r#"{"output_index":0,"summary_index":0,"item_id":"r1","part":{"type":"summary_text","text":""}}"#,
        );
        assert!(matches!(ev[0], Event::BlockStart(_)));
        let ev = with(
            &mut state,
            "response.reasoning_summary_text.delta",
            r#"{"output_index":0,"summary_index":0,"item_id":"r1","delta":"sum"}"#,
        );
        if let Event::BlockDelta(d) = &ev[0] {
            assert!(matches!(&d.delta, DeltaContent::Thinking(t) if t == "sum"));
        }
        let ev = with(
            &mut state,
            "response.reasoning_summary_part.done",
            r#"{"output_index":0,"summary_index":0,"item_id":"r1"}"#,
        );
        assert!(matches!(ev[0], Event::BlockStop(_)));
    }

    #[test]
    fn delta_without_prior_start_recovers() {
        // 防御: content_part.added が落ちても delta 単独で BlockStart+Delta を発行
        let mut state = OpenAIResponsesState::default();
        let ev = with(
            &mut state,
            "response.output_text.delta",
            r#"{"output_index":0,"content_index":0,"item_id":"m1","delta":"hi"}"#,
        );
        assert_eq!(ev.len(), 2);
        assert!(matches!(ev[0], Event::BlockStart(_)));
        assert!(matches!(ev[1], Event::BlockDelta(_)));
    }

    #[test]
    fn parallel_output_items_get_distinct_indices() {
        // 2 つの function_call が並列で output_item.added される場合、
        // flat index が別々になる（Parallel tool calling の基本）。
        let mut state = OpenAIResponsesState::default();
        let ev1 = with(
            &mut state,
            "response.output_item.added",
            r#"{"output_index":0,"item":{"type":"function_call","id":"a","call_id":"c1","name":"t1"}}"#,
        );
        let ev2 = with(
            &mut state,
            "response.output_item.added",
            r#"{"output_index":1,"item":{"type":"function_call","id":"b","call_id":"c2","name":"t2"}}"#,
        );
        let i1 = if let Event::BlockStart(s) = &ev1[0] {
            s.index
        } else {
            panic!()
        };
        let i2 = if let Event::BlockStart(s) = &ev2[0] {
            s.index
        } else {
            panic!()
        };
        assert_ne!(i1, i2);
    }

    #[test]
    fn failed_response_emits_error_and_status() {
        let data = r#"{"response":{"error":{"type":"invalid_request_error","message":"bad"}}}"#;
        let (events, _) = run("response.failed", data);
        assert_eq!(events.len(), 2);
        assert!(matches!(events[0], Event::Error(_)));
        assert!(matches!(
            events[1],
            Event::Status(StatusEvent {
                status: ResponseStatus::Failed
            })
        ));
    }

    #[test]
    fn reasoning_output_item_emits_reasoning_item_with_text_summary_encrypted() {
        // 完成済み reasoning wrapper が text + summary[] + encrypted_content を持って
        // ReasoningItem として届くこと。
        let mut state = OpenAIResponsesState::default();

        // wrapper added (id だけ持つ)
        with(
            &mut state,
            "response.output_item.added",
            r#"{"output_index":0,"item":{"type":"reasoning","id":"r1"}}"#,
        );
        // 内側の reasoning_text 用 content_part
        with(
            &mut state,
            "response.content_part.added",
            r#"{"output_index":0,"content_index":0,"item_id":"r1","part":{"type":"reasoning_text","text":""}}"#,
        );
        with(
            &mut state,
            "response.reasoning_text.delta",
            r#"{"output_index":0,"content_index":0,"item_id":"r1","delta":"hello "}"#,
        );
        with(
            &mut state,
            "response.reasoning_text.delta",
            r#"{"output_index":0,"content_index":0,"item_id":"r1","delta":"world"}"#,
        );
        with(
            &mut state,
            "response.content_part.done",
            r#"{"output_index":0,"content_index":0,"item_id":"r1","part":{"type":"reasoning_text","text":"hello world"}}"#,
        );
        // summary 1 件
        with(
            &mut state,
            "response.reasoning_summary_part.added",
            r#"{"output_index":0,"summary_index":0,"item_id":"r1","part":{"type":"summary_text","text":""}}"#,
        );
        with(
            &mut state,
            "response.reasoning_summary_text.delta",
            r#"{"output_index":0,"summary_index":0,"item_id":"r1","delta":"sum-A"}"#,
        );
        with(
            &mut state,
            "response.reasoning_summary_part.done",
            r#"{"output_index":0,"summary_index":0,"item_id":"r1"}"#,
        );

        // wrapper done (encrypted_content が乗る)
        let evs = with(
            &mut state,
            "response.output_item.done",
            r#"{"output_index":0,"item":{"type":"reasoning","id":"r1","encrypted_content":"ENC-XYZ"}}"#,
        );
        assert_eq!(evs.len(), 1);
        let Event::ReasoningItem(reasoning) = &evs[0] else {
            panic!("expected ReasoningItem, got {:?}", evs[0]);
        };
        assert_eq!(reasoning.id.as_deref(), Some("r1"));
        assert_eq!(reasoning.text, "hello world");
        assert_eq!(reasoning.summary, vec!["sum-A".to_string()]);
        assert_eq!(reasoning.encrypted_content.as_deref(), Some("ENC-XYZ"));
        assert!(reasoning.signature.is_none());
        // pending_reasoning は drain されていること
        assert!(state.pending_reasoning.is_empty());
    }

    #[test]
    fn reasoning_wrapper_without_inner_content_emits_empty_text() {
        // encrypted_content だけ届く（reasoning_text 無し）ケースでも
        // ReasoningItem は発火する。
        let mut state = OpenAIResponsesState::default();
        with(
            &mut state,
            "response.output_item.added",
            r#"{"output_index":2,"item":{"type":"reasoning","id":"r9"}}"#,
        );
        let evs = with(
            &mut state,
            "response.output_item.done",
            r#"{"output_index":2,"item":{"type":"reasoning","id":"r9","encrypted_content":"BLOB"}}"#,
        );
        let Event::ReasoningItem(r) = &evs[0] else {
            panic!()
        };
        assert!(r.text.is_empty());
        assert!(r.summary.is_empty());
        assert_eq!(r.encrypted_content.as_deref(), Some("BLOB"));
    }

    #[test]
    fn unknown_event_is_ignored() {
        let (events, _) = run("response.in_progress", "{}");
        assert!(events.is_empty());
    }
}
