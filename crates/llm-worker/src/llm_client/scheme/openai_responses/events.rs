//! OpenAI Responses API の SSE イベントパース
//!
//! `response.*` 名前空間の SSE を共通の [`Event`](crate::llm_client::event::Event)
//! に変換する。Responses の (output_index, content_index) 2 次元座標と
//! insomnia 側 1 次元 `BlockStart/Delta/Stop::index` のマッピングは
//! [`OpenAIResponsesState`] が保持する。

use std::collections::{BTreeMap, HashMap};

use serde::Deserialize;
use serde_json::{Map, Value};

use crate::llm_client::{
    ClientError,
    event::{
        BlockDelta, BlockMetadata, BlockStart, BlockStop, BlockType, DeltaContent, ErrorEvent,
        Event, ReasoningItemEvent, ResponseStatus, StatusEvent, UnhandledSseEvent, UsageEvent,
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
    #[serde(flatten)]
    extra: BTreeMap<String, Value>,
}

#[derive(Debug, Deserialize)]
struct FailedResponse {
    #[serde(default)]
    error: Option<ErrorDetail>,
    #[serde(flatten)]
    extra: BTreeMap<String, Value>,
}

#[derive(Debug, Deserialize)]
struct ErrorDetail {
    #[serde(rename = "type", default)]
    error_type: Option<String>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    code: Option<String>,
    #[serde(flatten)]
    extra: BTreeMap<String, Value>,
}

#[derive(Debug, Deserialize)]
struct TopLevelErrorEnvelope {
    error: TopLevelError,
    #[serde(flatten)]
    extra: BTreeMap<String, Value>,
}

#[derive(Debug, Deserialize)]
struct TopLevelError {
    #[serde(default)]
    message: Option<String>,
    #[serde(rename = "type", default)]
    error_type: Option<String>,
    #[serde(default)]
    code: Option<String>,
    #[serde(flatten)]
    extra: BTreeMap<String, Value>,
}

// ============================================================================
// parse entry point
// ============================================================================

/// SSE フレーム 1 件をパースし、0 個以上の [`Event`] に変換する。
///
/// `event_type` は SSE の `event:` フィールド。未対応の event type は
/// [`Event::UnhandledSse`] として観測可能にする。`data` が JSON でない /
/// 必要なフィールドが抜けている等は [`ClientError::Api`] で返す。
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
            let (code, message) = response_failure_diagnostic(event_type, ev);
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
            let ev = from_json::<TopLevelErrorEnvelope>(data).unwrap_or_else(|_| {
                TopLevelErrorEnvelope {
                    error: TopLevelError {
                        message: Some(data.to_string()),
                        error_type: None,
                        code: None,
                        extra: BTreeMap::new(),
                    },
                    extra: BTreeMap::new(),
                }
            });
            let (code, message) = top_level_error_diagnostic(ev);
            Ok(vec![Event::Error(ErrorEvent { code, message })])
        }

        // 未対応 / 情報系 event type は生成 semantics からは無視しつつ trace に残す。
        _ => Ok(vec![unhandled_sse_event(event_type, data)]),
    }
}

fn response_failure_diagnostic(event_type: &str, ev: ResponseFailed) -> (Option<String>, String) {
    let mut diagnostic = Map::new();
    diagnostic.insert("event".to_string(), Value::String(event_type.to_string()));

    let mut code = None;
    let base_message = if let Some(err) = ev.response.error {
        code = err.code.clone().or(err.error_type.clone());
        if let Some(error_type) = err.error_type {
            diagnostic.insert("error_type".to_string(), Value::String(error_type));
        }
        if let Some(error_code) = err.code {
            diagnostic.insert("error_code".to_string(), Value::String(error_code));
        }
        if !err.extra.is_empty() {
            diagnostic.insert(
                "error_extra".to_string(),
                diagnostic_object(err.extra, DIAGNOSTIC_VALUE_LIMIT),
            );
        }
        err.message
            .filter(|message| !message.trim().is_empty())
            .unwrap_or_else(|| format!("OpenAI Responses {event_type}"))
    } else {
        format!("OpenAI Responses {event_type}")
    };

    let response_extra = ev.response.extra;
    if let Some(reason) = response_extra
        .get("incomplete_details")
        .and_then(|value| value.get("reason"))
        .and_then(Value::as_str)
    {
        diagnostic.insert(
            "incomplete_reason".to_string(),
            Value::String(reason.to_string()),
        );
        if code.is_none() {
            code = Some(reason.to_string());
        }
    }
    if !response_extra.is_empty() {
        diagnostic.insert(
            "response_extra".to_string(),
            diagnostic_object(response_extra, DIAGNOSTIC_VALUE_LIMIT),
        );
    }
    if !ev.extra.is_empty() {
        diagnostic.insert(
            "event_extra".to_string(),
            diagnostic_object(ev.extra, DIAGNOSTIC_VALUE_LIMIT),
        );
    }

    (code, append_diagnostic(base_message, diagnostic))
}

fn top_level_error_diagnostic(ev: TopLevelErrorEnvelope) -> (Option<String>, String) {
    let code = ev.error.code.clone().or(ev.error.error_type.clone());
    let mut diagnostic = Map::new();
    diagnostic.insert("event".to_string(), Value::String("error".to_string()));
    if let Some(error_type) = ev.error.error_type {
        diagnostic.insert("error_type".to_string(), Value::String(error_type));
    }
    if let Some(error_code) = ev.error.code {
        diagnostic.insert("error_code".to_string(), Value::String(error_code));
    }
    if !ev.error.extra.is_empty() {
        diagnostic.insert(
            "error_extra".to_string(),
            diagnostic_object(ev.error.extra, DIAGNOSTIC_VALUE_LIMIT),
        );
    }
    if !ev.extra.is_empty() {
        diagnostic.insert(
            "event_extra".to_string(),
            diagnostic_object(ev.extra, DIAGNOSTIC_VALUE_LIMIT),
        );
    }

    let message = ev
        .error
        .message
        .filter(|message| !message.trim().is_empty())
        .unwrap_or_else(|| "OpenAI Responses error".to_string());
    (code, append_diagnostic(message, diagnostic))
}

const DIAGNOSTIC_VALUE_LIMIT: usize = 512;
const UNHANDLED_SSE_DATA_PREVIEW_LIMIT: usize = 512;

fn capped_unhandled_sse_data_preview(data: &str) -> String {
    if data.len() <= UNHANDLED_SSE_DATA_PREVIEW_LIMIT {
        return data.to_string();
    }

    let mut end = 0;
    for (idx, ch) in data.char_indices() {
        let next = idx + ch.len_utf8();
        if next > UNHANDLED_SSE_DATA_PREVIEW_LIMIT {
            break;
        }
        end = next;
    }
    data[..end].to_string()
}

fn unhandled_sse_event(event_type: &str, data: &str) -> Event {
    Event::UnhandledSse(UnhandledSseEvent {
        provider: "openai_responses".to_string(),
        event_type: event_type.to_string(),
        data_preview: capped_unhandled_sse_data_preview(data),
        data_len: data.len(),
    })
}

fn diagnostic_object(extra: BTreeMap<String, Value>, value_limit: usize) -> Value {
    Value::Object(
        extra
            .into_iter()
            .map(|(key, value)| (key, cap_json_value(value, value_limit)))
            .collect(),
    )
}

fn cap_json_value(value: Value, limit: usize) -> Value {
    let rendered = value.to_string();
    if rendered.len() <= limit {
        value
    } else {
        let capped: String = rendered.chars().take(limit).collect();
        Value::String(format!("{capped}…"))
    }
}

fn append_diagnostic(message: String, diagnostic: Map<String, Value>) -> String {
    if diagnostic.len() <= 1 {
        return message;
    }
    format!("{} | diagnostic={}", message, Value::Object(diagnostic))
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
    fn incomplete_response_preserves_incomplete_reason_without_error() {
        let data = r#"{
            "response": {
                "status": "incomplete",
                "incomplete_details": {"reason": "max_output_tokens"}
            }
        }"#;
        let (events, _) = run("response.incomplete", data);
        let Event::Error(err) = &events[0] else {
            panic!("expected error event")
        };
        assert_eq!(err.code.as_deref(), Some("max_output_tokens"));
        assert!(err.message.contains("OpenAI Responses response.incomplete"));
        assert!(err.message.contains("incomplete_reason"));
        assert!(err.message.contains("max_output_tokens"));
        assert!(!err.message.ends_with("response response.incomplete"));
    }

    #[test]
    fn incomplete_response_preserves_unknown_response_fields() {
        let data = r#"{
            "response": {
                "status": "incomplete",
                "incomplete_details": {"reason": "content_filter"},
                "mystery_field": {"nested": true}
            },
            "sequence_number": 42
        }"#;
        let (events, _) = run("response.incomplete", data);
        let Event::Error(err) = &events[0] else {
            panic!("expected error event")
        };
        assert!(err.message.contains("mystery_field"));
        assert!(err.message.contains("sequence_number"));
        assert!(err.message.contains("content_filter"));
    }

    #[test]
    fn failed_response_preserves_error_and_response_extra_fields() {
        let data = r#"{
            "response": {
                "error": {
                    "type": "server_error",
                    "code": "upstream_overloaded",
                    "message": "try later",
                    "param": "input"
                },
                "retry_hint": "short"
            }
        }"#;
        let (events, _) = run("response.failed", data);
        let Event::Error(err) = &events[0] else {
            panic!("expected error event")
        };
        assert_eq!(err.code.as_deref(), Some("upstream_overloaded"));
        assert!(err.message.contains("try later"));
        assert!(err.message.contains("param"));
        assert!(err.message.contains("retry_hint"));
    }

    #[test]
    fn top_level_error_preserves_unknown_fields() {
        let data = r#"{
            "error": {
                "type": "rate_limit_error",
                "code": "rate_limit_exceeded",
                "message": "slow down",
                "retry_after_ms": 1000
            },
            "request_id": "req_123"
        }"#;
        let (events, _) = run("error", data);
        let Event::Error(err) = &events[0] else {
            panic!("expected error event")
        };
        assert_eq!(err.code.as_deref(), Some("rate_limit_exceeded"));
        assert!(err.message.contains("slow down"));
        assert!(err.message.contains("retry_after_ms"));
        assert!(err.message.contains("request_id"));
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
    fn unknown_event_emits_trace_visible_unhandled_sse() {
        let data = r#"{"sequence_number":7,"note":"debug me"}"#;
        let (events, _) = run("response.mystery", data);
        assert_eq!(events.len(), 1);
        let Event::UnhandledSse(unhandled) = &events[0] else {
            panic!("expected UnhandledSse, got {:?}", events[0]);
        };
        assert_eq!(unhandled.provider, "openai_responses");
        assert_eq!(unhandled.event_type, "response.mystery");
        assert_eq!(unhandled.data_preview, data);
        assert_eq!(unhandled.data_len, data.len());
    }

    #[test]
    fn unknown_event_data_preview_is_bounded_and_data_len_is_original_bytes() {
        let data = format!("{}終端", "x".repeat(UNHANDLED_SSE_DATA_PREVIEW_LIMIT + 32));
        let (events, _) = run("response.mystery.large", &data);
        assert_eq!(events.len(), 1);
        let Event::UnhandledSse(unhandled) = &events[0] else {
            panic!("expected UnhandledSse, got {:?}", events[0]);
        };
        assert_eq!(unhandled.data_len, data.len());
        assert!(unhandled.data_preview.len() <= UNHANDLED_SSE_DATA_PREVIEW_LIMIT);
        assert_eq!(
            unhandled.data_preview,
            "x".repeat(UNHANDLED_SSE_DATA_PREVIEW_LIMIT)
        );
        assert!(unhandled.data_preview.len() < unhandled.data_len);
    }
}
