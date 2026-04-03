//! Open Responses Event Parser
//!
//! Parses SSE events from the Open Responses API into internal Event types.

use serde::Deserialize;

use crate::llm_client::{
    event::{
        BlockMetadata, BlockStart, BlockStop, DeltaContent, ErrorEvent, Event, ResponseStatus,
        StatusEvent, StopReason, UsageEvent,
    },
    ClientError,
};

// =============================================================================
// Open Responses SSE Event Types
// =============================================================================

/// Response created event
#[derive(Debug, Deserialize)]
pub struct ResponseCreatedEvent {
    pub response: ResponseObject,
}

/// Response object
#[derive(Debug, Deserialize)]
pub struct ResponseObject {
    pub id: String,
    pub status: String,
    #[serde(default)]
    pub output: Vec<OutputItem>,
    pub usage: Option<UsageObject>,
}

/// Output item in response
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OutputItem {
    Message {
        id: String,
        role: String,
        #[serde(default)]
        content: Vec<ContentPartObject>,
    },
    FunctionCall {
        id: String,
        call_id: String,
        name: String,
        arguments: String,
    },
    Reasoning {
        id: String,
        #[serde(default)]
        text: String,
    },
}

/// Content part object
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentPartObject {
    OutputText { text: String },
    InputText { text: String },
    Refusal { refusal: String },
}

/// Usage object
#[derive(Debug, Deserialize)]
pub struct UsageObject {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
}

/// Output item added event
#[derive(Debug, Deserialize)]
pub struct OutputItemAddedEvent {
    pub output_index: usize,
    pub item: OutputItem,
}

/// Text delta event
#[derive(Debug, Deserialize)]
pub struct TextDeltaEvent {
    pub output_index: usize,
    pub content_index: usize,
    pub delta: String,
}

/// Text done event
#[derive(Debug, Deserialize)]
pub struct TextDoneEvent {
    pub output_index: usize,
    pub content_index: usize,
    pub text: String,
}

/// Function call arguments delta event
#[derive(Debug, Deserialize)]
pub struct FunctionCallArgumentsDeltaEvent {
    pub output_index: usize,
    pub call_id: String,
    pub delta: String,
}

/// Function call arguments done event
#[derive(Debug, Deserialize)]
pub struct FunctionCallArgumentsDoneEvent {
    pub output_index: usize,
    pub call_id: String,
    pub arguments: String,
}

/// Reasoning delta event
#[derive(Debug, Deserialize)]
pub struct ReasoningDeltaEvent {
    pub output_index: usize,
    pub delta: String,
}

/// Reasoning done event
#[derive(Debug, Deserialize)]
pub struct ReasoningDoneEvent {
    pub output_index: usize,
    pub text: String,
}

/// Content part done event
#[derive(Debug, Deserialize)]
pub struct ContentPartDoneEvent {
    pub output_index: usize,
    pub content_index: usize,
    pub part: ContentPartObject,
}

/// Output item done event
#[derive(Debug, Deserialize)]
pub struct OutputItemDoneEvent {
    pub output_index: usize,
    pub item: OutputItem,
}

/// Response done event
#[derive(Debug, Deserialize)]
pub struct ResponseDoneEvent {
    pub response: ResponseObject,
}

/// Error event from API
#[derive(Debug, Deserialize)]
pub struct ApiErrorEvent {
    pub error: ApiError,
}

/// API error details
#[derive(Debug, Deserialize)]
pub struct ApiError {
    pub code: Option<String>,
    pub message: String,
}

// =============================================================================
// Event Parsing
// =============================================================================

/// Parse SSE event into internal Event(s)
///
/// Returns `Ok(None)` for events that should be ignored (e.g., heartbeats)
/// Returns `Ok(Some(vec))` for events that produce one or more internal Events
pub fn parse_event(event_type: &str, data: &str) -> Result<Option<Vec<Event>>, ClientError> {
    // Skip empty data
    if data.is_empty() || data == "[DONE]" {
        return Ok(None);
    }

    let events = match event_type {
        // Response lifecycle
        "response.created" => {
            let _event: ResponseCreatedEvent = parse_json(data)?;
            Some(vec![Event::Status(StatusEvent {
                status: ResponseStatus::Started,
            })])
        }

        "response.in_progress" => {
            // Just a status update, no action needed
            None
        }

        "response.completed" | "response.done" => {
            let event: ResponseDoneEvent = parse_json(data)?;
            let mut events = Vec::new();

            // Emit usage if present
            if let Some(usage) = event.response.usage {
                events.push(Event::Usage(UsageEvent {
                    input_tokens: usage.input_tokens,
                    output_tokens: usage.output_tokens,
                    total_tokens: usage.total_tokens,
                    cache_read_input_tokens: None,
                    cache_creation_input_tokens: None,
                }));
            }

            events.push(Event::Status(StatusEvent {
                status: ResponseStatus::Completed,
            }));
            Some(events)
        }

        "response.failed" => {
            // Try to parse error
            if let Ok(error_event) = parse_json::<ApiErrorEvent>(data) {
                Some(vec![
                    Event::Error(ErrorEvent {
                        code: error_event.error.code,
                        message: error_event.error.message,
                    }),
                    Event::Status(StatusEvent {
                        status: ResponseStatus::Failed,
                    }),
                ])
            } else {
                Some(vec![Event::Status(StatusEvent {
                    status: ResponseStatus::Failed,
                })])
            }
        }

        // Output item events
        "response.output_item.added" => {
            let event: OutputItemAddedEvent = parse_json(data)?;
            Some(vec![convert_item_added(&event)])
        }

        "response.output_item.done" => {
            let event: OutputItemDoneEvent = parse_json(data)?;
            Some(vec![convert_item_done(&event)])
        }

        // Text content events
        "response.output_text.delta" => {
            let event: TextDeltaEvent = parse_json(data)?;
            Some(vec![Event::text_delta(event.output_index, &event.delta)])
        }

        "response.output_text.done" => {
            // Text done - we'll handle stop in output_item.done
            let _event: TextDoneEvent = parse_json(data)?;
            None
        }

        // Content part events
        "response.content_part.added" => {
            // Content part added - we handle this via output_item.added
            None
        }

        "response.content_part.done" => {
            // Content part done - we handle stop in output_item.done
            None
        }

        // Function call events
        "response.function_call_arguments.delta" => {
            let event: FunctionCallArgumentsDeltaEvent = parse_json(data)?;
            Some(vec![Event::BlockDelta(crate::llm_client::event::BlockDelta {
                index: event.output_index,
                delta: DeltaContent::InputJson(event.delta),
            })])
        }

        "response.function_call_arguments.done" => {
            // Arguments done - we handle stop in output_item.done
            let _event: FunctionCallArgumentsDoneEvent = parse_json(data)?;
            None
        }

        // Reasoning events
        "response.reasoning.delta" | "response.reasoning_summary_text.delta" => {
            let event: ReasoningDeltaEvent = parse_json(data)?;
            Some(vec![Event::BlockDelta(crate::llm_client::event::BlockDelta {
                index: event.output_index,
                delta: DeltaContent::Thinking(event.delta),
            })])
        }

        "response.reasoning.done" | "response.reasoning_summary_text.done" => {
            // Reasoning done - we handle stop in output_item.done
            let _event: ReasoningDoneEvent = parse_json(data)?;
            None
        }

        // Error event
        "error" => {
            let event: ApiErrorEvent = parse_json(data)?;
            Some(vec![Event::Error(ErrorEvent {
                code: event.error.code,
                message: event.error.message,
            })])
        }

        // Unknown event type - ignore
        _ => {
            tracing::debug!(event_type = event_type, "Unknown Open Responses event type");
            None
        }
    };

    Ok(events)
}

fn parse_json<T: serde::de::DeserializeOwned>(data: &str) -> Result<T, ClientError> {
    serde_json::from_str(data).map_err(|e| ClientError::Parse(e.to_string()))
}

fn convert_item_added(event: &OutputItemAddedEvent) -> Event {
    match &event.item {
        OutputItem::Message { id, role: _, content: _ } => Event::BlockStart(BlockStart {
            index: event.output_index,
            block_type: crate::llm_client::event::BlockType::Text,
            metadata: BlockMetadata::Text,
        }),

        OutputItem::FunctionCall {
            id,
            call_id,
            name,
            arguments: _,
        } => Event::BlockStart(BlockStart {
            index: event.output_index,
            block_type: crate::llm_client::event::BlockType::ToolUse,
            metadata: BlockMetadata::ToolUse {
                id: call_id.clone(),
                name: name.clone(),
            },
        }),

        OutputItem::Reasoning { id, text: _ } => Event::BlockStart(BlockStart {
            index: event.output_index,
            block_type: crate::llm_client::event::BlockType::Thinking,
            metadata: BlockMetadata::Thinking,
        }),
    }
}

fn convert_item_done(event: &OutputItemDoneEvent) -> Event {
    let stop_reason = match &event.item {
        OutputItem::FunctionCall { .. } => Some(StopReason::ToolUse),
        _ => Some(StopReason::EndTurn),
    };

    Event::BlockStop(BlockStop {
        index: event.output_index,
        stop_reason,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_response_created() {
        let data = r#"{"response":{"id":"resp_123","status":"in_progress","output":[]}}"#;
        let events = parse_event("response.created", data).unwrap().unwrap();
        assert_eq!(events.len(), 1);
        assert!(matches!(
            events[0],
            Event::Status(StatusEvent {
                status: ResponseStatus::Started
            })
        ));
    }

    #[test]
    fn test_parse_text_delta() {
        let data = r#"{"output_index":0,"content_index":0,"delta":"Hello"}"#;
        let events = parse_event("response.output_text.delta", data)
            .unwrap()
            .unwrap();
        assert_eq!(events.len(), 1);
        if let Event::BlockDelta(delta) = &events[0] {
            assert_eq!(delta.index, 0);
            assert!(matches!(&delta.delta, DeltaContent::Text(t) if t == "Hello"));
        } else {
            panic!("Expected BlockDelta");
        }
    }

    #[test]
    fn test_parse_output_item_added_message() {
        let data = r#"{"output_index":0,"item":{"type":"message","id":"msg_123","role":"assistant","content":[]}}"#;
        let events = parse_event("response.output_item.added", data)
            .unwrap()
            .unwrap();
        assert_eq!(events.len(), 1);
        if let Event::BlockStart(start) = &events[0] {
            assert_eq!(start.index, 0);
            assert!(matches!(
                start.block_type,
                crate::llm_client::event::BlockType::Text
            ));
        } else {
            panic!("Expected BlockStart");
        }
    }

    #[test]
    fn test_parse_output_item_added_function_call() {
        let data = r#"{"output_index":1,"item":{"type":"function_call","id":"fc_123","call_id":"call_456","name":"get_weather","arguments":""}}"#;
        let events = parse_event("response.output_item.added", data)
            .unwrap()
            .unwrap();
        assert_eq!(events.len(), 1);
        if let Event::BlockStart(start) = &events[0] {
            assert_eq!(start.index, 1);
            assert!(matches!(
                start.block_type,
                crate::llm_client::event::BlockType::ToolUse
            ));
            if let BlockMetadata::ToolUse { id, name } = &start.metadata {
                assert_eq!(id, "call_456");
                assert_eq!(name, "get_weather");
            } else {
                panic!("Expected ToolUse metadata");
            }
        } else {
            panic!("Expected BlockStart");
        }
    }

    #[test]
    fn test_parse_function_call_arguments_delta() {
        let data = r#"{"output_index":1,"call_id":"call_456","delta":"{\"city\":"}"#;
        let events = parse_event("response.function_call_arguments.delta", data)
            .unwrap()
            .unwrap();
        assert_eq!(events.len(), 1);
        if let Event::BlockDelta(delta) = &events[0] {
            assert_eq!(delta.index, 1);
            assert!(matches!(
                &delta.delta,
                DeltaContent::InputJson(s) if s == "{\"city\":"
            ));
        } else {
            panic!("Expected BlockDelta");
        }
    }

    #[test]
    fn test_parse_response_completed() {
        let data = r#"{"response":{"id":"resp_123","status":"completed","output":[],"usage":{"input_tokens":10,"output_tokens":20,"total_tokens":30}}}"#;
        let events = parse_event("response.completed", data).unwrap().unwrap();
        assert_eq!(events.len(), 2);

        // First event should be usage
        if let Event::Usage(usage) = &events[0] {
            assert_eq!(usage.input_tokens, Some(10));
            assert_eq!(usage.output_tokens, Some(20));
            assert_eq!(usage.total_tokens, Some(30));
        } else {
            panic!("Expected Usage event");
        }

        // Second event should be status
        assert!(matches!(
            events[1],
            Event::Status(StatusEvent {
                status: ResponseStatus::Completed
            })
        ));
    }

    #[test]
    fn test_parse_error() {
        let data = r#"{"error":{"code":"rate_limit","message":"Too many requests"}}"#;
        let events = parse_event("error", data).unwrap().unwrap();
        assert_eq!(events.len(), 1);
        if let Event::Error(err) = &events[0] {
            assert_eq!(err.code, Some("rate_limit".to_string()));
            assert_eq!(err.message, "Too many requests");
        } else {
            panic!("Expected Error event");
        }
    }

    #[test]
    fn test_parse_unknown_event() {
        let data = r#"{}"#;
        let events = parse_event("some.unknown.event", data).unwrap();
        assert!(events.is_none());
    }
}
