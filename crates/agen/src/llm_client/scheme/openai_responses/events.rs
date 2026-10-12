//! OpenAI Responses API SSE event parsing.
//!
//! Converts SSE events in the `response.*` namespace to shared [`Event`]s.
//! [`OpenAIResponsesState`] holds the mapping from Responses' two-dimensional
//! (output_index, content_index) coordinates to this crate's one-dimensional
//! `BlockStart/Delta/Stop::index`.

use std::collections::{BTreeMap, HashMap};

use serde::Deserialize;
use serde_json::{Map, Value};

use crate::llm_client::{
    ClientError,
    event::{
        BlockDelta, BlockMetadata, BlockStart, BlockStop, BlockType, DeltaContent, ErrorEvent,
        Event, ReasoningBlockData, ResponseStatus, StatusEvent, UnhandledSseEvent, UsageEvent,
    },
};

/// Coordinate → flat block index map used during SSE parsing.
#[derive(Debug, Default)]
pub struct OpenAIResponsesState {
    slots: HashMap<SlotKey, SlotInfo>,
    next_index: usize,
    /// Reasoning output_items being accumulated. Allocated at
    /// `output_item.added` (Reasoning), filled by `reasoning_text.delta` /
    /// `reasoning_summary_text.delta`, then attached as reasoning persistence
    /// material to an existing reasoning_text block or metadata-only Thinking
    /// block at `output_item.done` (Reasoning).
    pending_reasoning: HashMap<usize, PendingReasoning>,
    /// Tool input deltas and authoritative final values, keyed by output index.
    pending_tool_inputs: HashMap<usize, PendingToolInput>,
}

/// A tool input stream accumulated until an authoritative done payload arrives.
#[derive(Debug)]
struct PendingToolInput {
    kind: ToolInputKind,
    streamed: String,
    saw_delta: bool,
    final_value: Option<String>,
}

impl PendingToolInput {
    fn new(kind: ToolInputKind) -> Self {
        Self {
            kind,
            streamed: String::new(),
            saw_delta: false,
            final_value: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolInputKind {
    FunctionCall,
    CustomToolCall,
}

impl ToolInputKind {
    fn label(self) -> &'static str {
        match self {
            Self::FunctionCall => "function-call arguments",
            Self::CustomToolCall => "custom-tool input",
        }
    }
}

/// Accumulation buffer for one reasoning output_item.
#[derive(Debug, Default)]
struct PendingReasoning {
    id: Option<String>,
    /// Accumulated `reasoning_text.delta`, concatenating multiple content_parts in order.
    text: String,
    /// Accumulates `reasoning_summary_text.delta` in summary_index order.
    summary: Vec<String>,
    /// Reasoning/thinking blocks whose `response.content_part.done` arrived first.
    ///
    /// encrypted_content and final summaries are not complete until
    /// `response.output_item.done`. Attach persistence metadata to the existing
    /// block's stop rather than creating an extra live-visible synthetic Thinking block.
    deferred_thinking_stops: Vec<SlotInfo>,
}

impl PendingReasoning {
    fn into_reasoning_data(self, encrypted_content: Option<String>) -> ReasoningBlockData {
        ReasoningBlockData {
            id: self.id,
            text: Some(self.text),
            summary: self.summary.into_iter().filter(|s| !s.is_empty()).collect(),
            encrypted_content,
            signature: None,
        }
    }
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

    /// Get an existing slot or implicitly allocate one with `block_type`, also
    /// returning whether it was newly allocated. Defends against deltas arriving
    /// first or missing content_part.added events.
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

    fn defer_reasoning_stop(&mut self, output_index: usize, info: SlotInfo) {
        self.ensure_reasoning(output_index)
            .deferred_thinking_stops
            .push(info);
    }

    fn take_active_reasoning_slots(&mut self, output_index: usize) -> Vec<SlotInfo> {
        let mut keys: Vec<_> = self
            .slots
            .iter()
            .filter_map(|(key, info)| match key {
                SlotKey::ContentPart { output, content }
                    if *output == output_index && info.block_type == BlockType::Thinking =>
                {
                    Some((*content, *key))
                }
                _ => None,
            })
            .collect();
        keys.sort_by_key(|(content, _)| *content);
        keys.into_iter()
            .filter_map(|(_, key)| self.slots.remove(&key))
            .collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum SlotKey {
    /// tool_use (function_call / custom_tool_call) — one block for the entire output_item.
    OutputItem(usize),
    /// Message output_text / reasoning item reasoning_text.
    ContentPart { output: usize, content: usize },
    /// Reasoning item summary_text (summary_index).
    Summary { output: usize, summary: usize },
}

#[derive(Debug, Clone, Copy)]
struct SlotInfo {
    flat_index: usize,
    block_type: BlockType,
}

// ============================================================================
// SSE event JSON structures
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

/// `item` in `response.output_item.added/done`.
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
        /// First populated at `output_item.done`. Carries an opaque blob when
        /// `include=["reasoning.encrypted_content"]` is specified.
        #[serde(default)]
        encrypted_content: Option<String>,
    },
    FunctionCall {
        #[allow(dead_code)]
        #[serde(default)]
        id: Option<String>,
        call_id: String,
        name: String,
        #[serde(default)]
        arguments: Option<String>,
    },
    CustomToolCall {
        #[allow(dead_code)]
        #[serde(default)]
        id: Option<String>,
        call_id: String,
        name: String,
        #[serde(default)]
        input: Option<String>,
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

/// `part` in `response.content_part.added/done`.
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
struct FunctionCallArgumentsDone {
    output_index: usize,
    arguments: String,
}

#[derive(Debug, Deserialize)]
struct CustomToolCallInputDelta {
    output_index: usize,
    delta: String,
}

#[derive(Debug, Deserialize)]
struct CustomToolCallInputDone {
    output_index: usize,
    input: String,
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
    /// Breakdown of `input_tokens`; `cached_tokens` counts prompt cache hits.
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

/// Parse one SSE frame and convert it to zero or more [`Event`]s.
///
/// `event_type` is the SSE `event:` field. Unsupported event types are observable
/// as [`Event::UnhandledSse`]. Non-JSON `data`, missing required fields, and similar
/// failures return [`ClientError::Api`].
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
                    // The Responses API does not account for cache writes separately (included in input_tokens).
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
                OutputItem::FunctionCall { call_id, name, .. } => start_tool_output_item(
                    state,
                    ev.output_index,
                    ToolInputKind::FunctionCall,
                    call_id,
                    name,
                ),
                OutputItem::CustomToolCall { call_id, name, .. } => start_tool_output_item(
                    state,
                    ev.output_index,
                    ToolInputKind::CustomToolCall,
                    call_id,
                    name,
                ),
                OutputItem::Reasoning { id, .. } => {
                    // Allocate the wrapper. Its content_parts / summary_parts continue
                    // to use separate SlotKeys (preserving the streaming display).
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
            // On Reasoning wrapper done, attach accumulated material to the existing
            // reasoning_text block's stop. If content_part.done arrived first, defer
            // its stop and complete it here with encrypted_content / summary.
            // Create synthetic blocks only for metadata-only items without reasoning_text.
            if let OutputItem::Reasoning {
                id,
                encrypted_content,
                ..
            } = &ev.item
            {
                let mut pending = state
                    .pending_reasoning
                    .remove(&ev.output_index)
                    .unwrap_or_default();
                if pending.id.is_none() {
                    pending.id = id.clone();
                }

                let mut stop_blocks = std::mem::take(&mut pending.deferred_thinking_stops);
                stop_blocks.extend(state.take_active_reasoning_slots(ev.output_index));
                let reasoning = pending.into_reasoning_data(encrypted_content.clone());

                if stop_blocks.is_empty() {
                    let info =
                        state.allocate(SlotKey::OutputItem(ev.output_index), BlockType::Thinking);
                    state.slots.remove(&SlotKey::OutputItem(ev.output_index));
                    return Ok(vec![
                        Event::BlockStart(BlockStart {
                            index: info.flat_index,
                            block_type: BlockType::Thinking,
                            metadata: BlockMetadata::Thinking,
                        }),
                        Event::BlockStop(BlockStop {
                            index: info.flat_index,
                            block_type: BlockType::Thinking,
                            stop_reason: None,
                            reasoning: Some(reasoning),
                        }),
                    ]);
                }

                let last = stop_blocks.len() - 1;
                return Ok(stop_blocks
                    .into_iter()
                    .enumerate()
                    .map(|(idx, info)| {
                        Event::BlockStop(BlockStop {
                            index: info.flat_index,
                            block_type: info.block_type,
                            stop_reason: None,
                            reasoning: (idx == last).then(|| reasoning.clone()),
                        })
                    })
                    .collect());
            }
            match ev.item {
                OutputItem::FunctionCall {
                    call_id,
                    name,
                    arguments,
                    ..
                } => finish_tool_output_item(
                    state,
                    ev.output_index,
                    ToolInputKind::FunctionCall,
                    call_id,
                    name,
                    arguments,
                ),
                OutputItem::CustomToolCall {
                    call_id,
                    name,
                    input,
                    ..
                } => finish_tool_output_item(
                    state,
                    ev.output_index,
                    ToolInputKind::CustomToolCall,
                    call_id,
                    name,
                    input,
                ),
                _ => {
                    if let Some(info) = state.slots.remove(&SlotKey::OutputItem(ev.output_index)) {
                        Ok(vec![Event::BlockStop(BlockStop {
                            index: info.flat_index,
                            block_type: info.block_type,
                            stop_reason: None,
                            reasoning: None,
                        })])
                    } else {
                        Ok(Vec::new())
                    }
                }
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
                if matches!(ev.part, ContentPart::ReasoningText { .. })
                    || info.block_type == BlockType::Thinking
                {
                    state.defer_reasoning_stop(ev.output_index, info);
                    Ok(Vec::new())
                } else {
                    Ok(vec![Event::BlockStop(BlockStop {
                        index: info.flat_index,
                        block_type: info.block_type,
                        stop_reason: None,
                        reasoning: None,
                    })])
                }
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
            // Accumulate for round-tripping.
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
            // Accumulate for round-tripping.
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
                    reasoning: None,
                })])
            } else {
                Ok(Vec::new())
            }
        }

        "response.function_call_arguments.delta" => {
            let ev: FunctionCallArgumentsDelta = from_json(data)?;
            record_tool_input_delta(
                state,
                ev.output_index,
                ToolInputKind::FunctionCall,
                ev.delta,
            )
        }

        "response.function_call_arguments.done" => {
            let ev: FunctionCallArgumentsDone = from_json(data)?;
            reconcile_tool_input(
                state,
                ev.output_index,
                ToolInputKind::FunctionCall,
                ev.arguments,
                BlockMetadata::ToolUse {
                    id: String::new(),
                    name: String::new(),
                },
            )
        }

        "response.custom_tool_call_input.delta" => {
            let ev: CustomToolCallInputDelta = from_json(data)?;
            record_tool_input_delta(
                state,
                ev.output_index,
                ToolInputKind::CustomToolCall,
                ev.delta,
            )
        }

        "response.custom_tool_call_input.done" => {
            let ev: CustomToolCallInputDone = from_json(data)?;
            reconcile_tool_input(
                state,
                ev.output_index,
                ToolInputKind::CustomToolCall,
                ev.input,
                BlockMetadata::ToolUse {
                    id: String::new(),
                    name: String::new(),
                },
            )
        }

        "error" => {
            // Responses documents flat error frames (code/message/param),
            // while providers also send nested error envelopes. Normalize both
            // structurally so classification never scans arbitrary raw JSON.
            let ev = from_json::<TopLevelErrorEnvelope>(data)
                .or_else(|_| {
                    from_json::<TopLevelError>(data).map(|error| TopLevelErrorEnvelope {
                        error,
                        extra: BTreeMap::new(),
                    })
                })
                .unwrap_or_else(|_| TopLevelErrorEnvelope {
                    error: TopLevelError {
                        // Raw data is observation, not a provider diagnostic that
                        // can authorize request recovery. Keep it out of the base
                        // message even when it contains image-size wording.
                        message: Some("OpenAI Responses error with unparseable data".to_string()),
                        error_type: None,
                        code: None,
                        extra: BTreeMap::from([(
                            "raw_data".to_string(),
                            Value::String(data.to_string()),
                        )]),
                    },
                    extra: BTreeMap::new(),
                });
            let (code, message) = top_level_error_diagnostic(ev);
            Ok(vec![Event::Error(ErrorEvent { code, message })])
        }

        // Ignore unsupported / informational event types for generation semantics, but retain them in traces.
        _ => Ok(vec![unhandled_sse_event(event_type, data)]),
    }
}

fn start_tool_output_item(
    state: &mut OpenAIResponsesState,
    output_index: usize,
    kind: ToolInputKind,
    call_id: String,
    name: String,
) -> Result<Vec<Event>, ClientError> {
    ensure_tool_input_kind(state, output_index, kind)?;
    let (info, just_created) =
        state.get_or_allocate(SlotKey::OutputItem(output_index), BlockType::ToolUse);
    if just_created {
        Ok(vec![Event::BlockStart(BlockStart {
            index: info.flat_index,
            block_type: BlockType::ToolUse,
            metadata: BlockMetadata::ToolUse { id: call_id, name },
        })])
    } else {
        Ok(Vec::new())
    }
}

fn record_tool_input_delta(
    state: &mut OpenAIResponsesState,
    output_index: usize,
    kind: ToolInputKind,
    delta: String,
) -> Result<Vec<Event>, ClientError> {
    let pending = ensure_tool_input_kind(state, output_index, kind)?;
    if pending.final_value.is_some() {
        return Err(tool_input_error(
            "tool_input_delta_after_done",
            format!(
                "OpenAI Responses {} delta arrived after its final value at output index {output_index}",
                kind.label()
            ),
        ));
    }
    pending.saw_delta = true;
    pending.streamed.push_str(&delta);

    Ok(ensure_and_delta(
        state,
        SlotKey::OutputItem(output_index),
        BlockType::ToolUse,
        BlockMetadata::ToolUse {
            id: String::new(),
            name: String::new(),
        },
        DeltaContent::InputJson(delta),
    ))
}

fn reconcile_tool_input(
    state: &mut OpenAIResponsesState,
    output_index: usize,
    kind: ToolInputKind,
    final_value: String,
    metadata: BlockMetadata,
) -> Result<Vec<Event>, ClientError> {
    let pending = ensure_tool_input_kind(state, output_index, kind)?;
    if pending
        .final_value
        .as_ref()
        .is_some_and(|existing| existing != &final_value)
    {
        return Err(tool_input_error(
            "tool_input_final_mismatch",
            format!(
                "OpenAI Responses authoritative {} values disagree at output index {output_index}",
                kind.label()
            ),
        ));
    }
    if pending.saw_delta && pending.streamed != final_value {
        return Err(tool_input_error(
            "tool_input_delta_mismatch",
            format!(
                "OpenAI Responses streamed {} does not match its authoritative final value at output index {output_index}",
                kind.label()
            ),
        ));
    }

    let should_emit = !pending.saw_delta;
    if should_emit {
        pending.saw_delta = true;
        pending.streamed.clone_from(&final_value);
    }
    pending.final_value = Some(final_value.clone());

    if should_emit {
        Ok(ensure_and_delta(
            state,
            SlotKey::OutputItem(output_index),
            BlockType::ToolUse,
            metadata,
            DeltaContent::InputJson(final_value),
        ))
    } else {
        Ok(Vec::new())
    }
}

fn finish_tool_output_item(
    state: &mut OpenAIResponsesState,
    output_index: usize,
    kind: ToolInputKind,
    call_id: String,
    name: String,
    final_value: Option<String>,
) -> Result<Vec<Event>, ClientError> {
    let metadata = BlockMetadata::ToolUse { id: call_id, name };
    let mut out = if let Some(final_value) = final_value {
        reconcile_tool_input(state, output_index, kind, final_value, metadata.clone())?
    } else {
        let pending = ensure_tool_input_kind(state, output_index, kind)?;
        if pending.final_value.is_none() {
            return Err(tool_input_error(
                "tool_input_final_missing",
                format!(
                    "OpenAI Responses {} ended without an authoritative final value at output index {output_index}",
                    kind.label()
                ),
            ));
        }
        Vec::new()
    };

    let (info, just_created) =
        state.get_or_allocate(SlotKey::OutputItem(output_index), BlockType::ToolUse);
    if just_created {
        out.push(Event::BlockStart(BlockStart {
            index: info.flat_index,
            block_type: BlockType::ToolUse,
            metadata,
        }));
    }
    state.slots.remove(&SlotKey::OutputItem(output_index));
    state.pending_tool_inputs.remove(&output_index);
    out.push(Event::BlockStop(BlockStop {
        index: info.flat_index,
        block_type: info.block_type,
        stop_reason: None,
        reasoning: None,
    }));
    Ok(out)
}

fn ensure_tool_input_kind(
    state: &mut OpenAIResponsesState,
    output_index: usize,
    kind: ToolInputKind,
) -> Result<&mut PendingToolInput, ClientError> {
    let pending = state
        .pending_tool_inputs
        .entry(output_index)
        .or_insert_with(|| PendingToolInput::new(kind));
    if pending.kind != kind {
        return Err(tool_input_error(
            "tool_input_kind_mismatch",
            format!("OpenAI Responses tool input kind changed at output index {output_index}"),
        ));
    }
    Ok(pending)
}

fn tool_input_error(code: &str, message: String) -> ClientError {
    ClientError::Api {
        status: None,
        code: Some(code.to_string()),
        message,
        retry_after: None,
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

/// Emit a delta, first emitting its BlockStart if not already sent.
/// Allows recovery from a delta alone when content_part.added was missed.
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

    fn input_json_deltas(events: &[Event]) -> Vec<&str> {
        events
            .iter()
            .filter_map(|event| match event {
                Event::BlockDelta(BlockDelta {
                    delta: DeltaContent::InputJson(delta),
                    ..
                }) => Some(delta.as_str()),
                _ => None,
            })
            .collect()
    }

    fn api_error_code(error: ClientError) -> Option<String> {
        match error {
            ClientError::Api { code, .. } => code,
            other => panic!("expected ClientError::Api, got {other:?}"),
        }
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
        // OpenAI Responses does not account for cache writes separately.
        assert_eq!(u.cache_creation_input_tokens, None);
    }

    #[test]
    fn text_stream_start_delta_stop() {
        let mut state = OpenAIResponsesState::default();
        // output_item.added (message) → ignored.
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
        let ev = with(
            &mut state,
            "response.function_call_arguments.delta",
            r#"{"output_index":1,"item_id":"fc1","delta":"1}"}"#,
        );
        assert_eq!(ev.len(), 1);
        assert!(matches!(
            &ev[0],
            Event::BlockDelta(BlockDelta {
                delta: DeltaContent::InputJson(j),
                ..
            }) if j == "1}"
        ));
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
    fn function_call_done_payload_emits_missing_final_input_before_stop() {
        let mut state = OpenAIResponsesState::default();
        with(
            &mut state,
            "response.output_item.added",
            r#"{"output_index":0,"item":{"type":"function_call","call_id":"call_1","name":"lookup"}}"#,
        );

        let events = with(
            &mut state,
            "response.output_item.done",
            r#"{"output_index":0,"item":{"type":"function_call","call_id":"call_1","name":"lookup","arguments":"{\"city\":\"Tokyo\"}"}}"#,
        );

        assert_eq!(input_json_deltas(&events), vec![r#"{"city":"Tokyo"}"#]);
        assert!(matches!(events.last(), Some(Event::BlockStop(_))));
    }

    #[test]
    fn function_call_arguments_done_accepts_exact_stream_without_duplication() {
        let mut state = OpenAIResponsesState::default();
        with(
            &mut state,
            "response.output_item.added",
            r#"{"output_index":0,"item":{"type":"function_call","call_id":"call_1","name":"lookup"}}"#,
        );
        let delta = with(
            &mut state,
            "response.function_call_arguments.delta",
            r#"{"output_index":0,"delta":"{\"city\":\"Tokyo\"}"}"#,
        );
        assert_eq!(input_json_deltas(&delta), vec![r#"{"city":"Tokyo"}"#]);

        let done = with(
            &mut state,
            "response.function_call_arguments.done",
            r#"{"output_index":0,"arguments":"{\"city\":\"Tokyo\"}"}"#,
        );
        assert!(done.is_empty());
        let output_done = with(
            &mut state,
            "response.output_item.done",
            r#"{"output_index":0,"item":{"type":"function_call","call_id":"call_1","name":"lookup","arguments":"{\"city\":\"Tokyo\"}"}}"#,
        );
        assert!(input_json_deltas(&output_done).is_empty());
        assert!(matches!(output_done.as_slice(), [Event::BlockStop(_)]));
    }

    #[test]
    fn function_call_arguments_done_rejects_stream_mismatch() {
        let mut state = OpenAIResponsesState::default();
        with(
            &mut state,
            "response.output_item.added",
            r#"{"output_index":0,"item":{"type":"function_call","call_id":"call_1","name":"lookup"}}"#,
        );
        with(
            &mut state,
            "response.function_call_arguments.delta",
            r#"{"output_index":0,"delta":"{\"city\":\"Kyoto\"}"}"#,
        );

        let error = parse_sse(
            "response.function_call_arguments.done",
            r#"{"output_index":0,"arguments":"{\"city\":\"Tokyo\"}"}"#,
            &mut state,
        )
        .unwrap_err();
        assert_eq!(
            api_error_code(error).as_deref(),
            Some("tool_input_delta_mismatch")
        );
    }

    #[test]
    fn custom_tool_input_done_event_emits_missing_final_input_once() {
        let mut state = OpenAIResponsesState::default();
        with(
            &mut state,
            "response.output_item.added",
            r#"{"output_index":0,"item":{"type":"custom_tool_call","call_id":"call_1","name":"shell"}}"#,
        );

        let done = with(
            &mut state,
            "response.custom_tool_call_input.done",
            r#"{"output_index":0,"input":"echo ready"}"#,
        );
        assert_eq!(input_json_deltas(&done), vec!["echo ready"]);
        let output_done = with(
            &mut state,
            "response.output_item.done",
            r#"{"output_index":0,"item":{"type":"custom_tool_call","call_id":"call_1","name":"shell","input":"echo ready"}}"#,
        );
        assert!(input_json_deltas(&output_done).is_empty());
        assert!(matches!(output_done.as_slice(), [Event::BlockStop(_)]));
    }

    #[test]
    fn custom_tool_done_payload_accepts_exact_stream_without_duplication() {
        let mut state = OpenAIResponsesState::default();
        with(
            &mut state,
            "response.output_item.added",
            r#"{"output_index":0,"item":{"type":"custom_tool_call","call_id":"call_1","name":"shell"}}"#,
        );
        let delta = with(
            &mut state,
            "response.custom_tool_call_input.delta",
            r#"{"output_index":0,"delta":"echo ready"}"#,
        );
        assert_eq!(input_json_deltas(&delta), vec!["echo ready"]);

        let output_done = with(
            &mut state,
            "response.output_item.done",
            r#"{"output_index":0,"item":{"type":"custom_tool_call","call_id":"call_1","name":"shell","input":"echo ready"}}"#,
        );
        assert!(input_json_deltas(&output_done).is_empty());
        assert!(matches!(output_done.as_slice(), [Event::BlockStop(_)]));
    }

    #[test]
    fn custom_tool_done_payload_rejects_stream_mismatch() {
        let mut state = OpenAIResponsesState::default();
        with(
            &mut state,
            "response.output_item.added",
            r#"{"output_index":0,"item":{"type":"custom_tool_call","call_id":"call_1","name":"shell"}}"#,
        );
        with(
            &mut state,
            "response.custom_tool_call_input.delta",
            r#"{"output_index":0,"delta":"echo stale"}"#,
        );

        let error = parse_sse(
            "response.output_item.done",
            r#"{"output_index":0,"item":{"type":"custom_tool_call","call_id":"call_1","name":"shell","input":"echo ready"}}"#,
            &mut state,
        )
        .unwrap_err();
        assert_eq!(
            api_error_code(error).as_deref(),
            Some("tool_input_delta_mismatch")
        );
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
        // Defensive: emit BlockStart+Delta from a delta alone if content_part.added was lost.
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
        // Two parallel function_call output_item.added events get distinct
        // flat indices (the basis of parallel tool calling).
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
    fn reasoning_output_item_completes_metadata_thinking_block_with_text_summary_encrypted() {
        // A completed reasoning wrapper must deliver text + summary[] +
        // encrypted_content as Thinking BlockStop metadata.
        let mut state = OpenAIResponsesState::default();

        // Wrapper added (with only an id).
        with(
            &mut state,
            "response.output_item.added",
            r#"{"output_index":0,"item":{"type":"reasoning","id":"r1"}}"#,
        );
        // Inner content_part for reasoning_text.
        let start = with(
            &mut state,
            "response.content_part.added",
            r#"{"output_index":0,"content_index":0,"item_id":"r1","part":{"type":"reasoning_text","text":""}}"#,
        );
        let start_index = match start.as_slice() {
            [Event::BlockStart(start)] => start.index,
            other => panic!("expected one BlockStart, got {other:?}"),
        };
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
        let part_done = with(
            &mut state,
            "response.content_part.done",
            r#"{"output_index":0,"content_index":0,"item_id":"r1","part":{"type":"reasoning_text","text":"hello world"}}"#,
        );
        assert!(part_done.is_empty());
        // One summary.
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

        // Wrapper done (includes encrypted_content).
        let evs = with(
            &mut state,
            "response.output_item.done",
            r#"{"output_index":0,"item":{"type":"reasoning","id":"r1","encrypted_content":"ENC-XYZ"}}"#,
        );
        assert_eq!(evs.len(), 1);
        let Event::BlockStop(stop) = &evs[0] else {
            panic!("expected BlockStop, got {:?}", evs[0]);
        };
        assert_eq!(stop.index, start_index);
        let reasoning = stop.reasoning.as_ref().expect("reasoning metadata");
        assert_eq!(reasoning.id.as_deref(), Some("r1"));
        assert_eq!(reasoning.text.as_deref(), Some("hello world"));
        assert_eq!(reasoning.summary, vec!["sum-A".to_string()]);
        assert_eq!(reasoning.encrypted_content.as_deref(), Some("ENC-XYZ"));
        assert!(reasoning.signature.is_none());
        // pending_reasoning must be drained.
        assert!(state.pending_reasoning.is_empty());
    }

    #[test]
    fn reasoning_text_done_then_output_done_emits_single_existing_block_stop() {
        let mut state = OpenAIResponsesState::default();
        with(
            &mut state,
            "response.output_item.added",
            r#"{"output_index":0,"item":{"type":"reasoning","id":"r1"}}"#,
        );

        let mut lifecycle = Vec::new();
        lifecycle.extend(with(
            &mut state,
            "response.content_part.added",
            r#"{"output_index":0,"content_index":0,"item_id":"r1","part":{"type":"reasoning_text","text":""}}"#,
        ));
        lifecycle.extend(with(
            &mut state,
            "response.reasoning_text.delta",
            r#"{"output_index":0,"content_index":0,"item_id":"r1","delta":"think"}"#,
        ));
        lifecycle.extend(with(
            &mut state,
            "response.content_part.done",
            r#"{"output_index":0,"content_index":0,"item_id":"r1","part":{"type":"reasoning_text","text":"think"}}"#,
        ));
        lifecycle.extend(with(
            &mut state,
            "response.output_item.done",
            r#"{"output_index":0,"item":{"type":"reasoning","id":"r1","encrypted_content":"ENC"}}"#,
        ));

        let starts: Vec<_> = lifecycle
            .iter()
            .filter_map(|event| match event {
                Event::BlockStart(start) => Some(start.index),
                _ => None,
            })
            .collect();
        let stops: Vec<_> = lifecycle
            .iter()
            .filter_map(|event| match event {
                Event::BlockStop(stop) => Some(stop),
                _ => None,
            })
            .collect();

        assert_eq!(starts.len(), 1, "no synthetic second Thinking start");
        assert_eq!(stops.len(), 1, "no duplicate empty Thinking stop");
        assert_eq!(stops[0].index, starts[0]);
        let reasoning = stops[0].reasoning.as_ref().expect("reasoning metadata");
        assert_eq!(reasoning.text.as_deref(), Some("think"));
        assert_eq!(reasoning.encrypted_content.as_deref(), Some("ENC"));

        struct StopRecorder(std::sync::Arc<std::sync::Mutex<Vec<(usize, String, bool)>>>);
        impl crate::handler::Handler<crate::handler::ThinkingBlockKind> for StopRecorder {
            type Scope = String;

            fn on_event(
                &mut self,
                scope: &mut Self::Scope,
                event: &crate::handler::ThinkingBlockEvent,
            ) {
                match event {
                    crate::handler::ThinkingBlockEvent::Start(_) => scope.clear(),
                    crate::handler::ThinkingBlockEvent::Delta(delta) => scope.push_str(delta),
                    crate::handler::ThinkingBlockEvent::Stop(stop) => self
                        .0
                        .lock()
                        .unwrap()
                        .push((stop.index, scope.clone(), stop.reasoning.is_some())),
                }
            }
        }

        let stops = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut timeline = crate::timeline::Timeline::new();
        timeline.on_thinking_block(StopRecorder(stops.clone()));
        for event in &lifecycle {
            timeline.dispatch(event);
        }
        let stops = stops.lock().unwrap().clone();
        assert_eq!(stops, vec![(starts[0], "think".to_string(), true)]);
    }

    #[test]
    fn reasoning_wrapper_without_inner_content_emits_empty_text() {
        // Reasoning metadata must arrive even when only encrypted_content
        // is received (no reasoning_text).
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
        assert_eq!(evs.len(), 2);
        assert!(matches!(evs[0], Event::BlockStart(_)));
        let Event::BlockStop(stop) = &evs[1] else {
            panic!("expected BlockStop")
        };
        let r = stop.reasoning.as_ref().expect("reasoning metadata");
        assert_eq!(r.text.as_deref(), Some(""));
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
