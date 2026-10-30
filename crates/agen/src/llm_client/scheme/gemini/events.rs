//! Gemini SSE event parsing.
//!
//! Parse Google Gemini API SSE events and convert them to the unified Event type.

use crate::llm_client::{
    ClientError,
    event::{BlockMetadata, BlockStart, BlockStop, BlockType, Event, StopReason, UsageEvent},
};
use serde::Deserialize;

use super::GeminiScheme;

// ============================================================================
// SSE event JSON structures
// ============================================================================

/// Gemini GenerateContentResponse (streaming chunk).
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GenerateContentResponse {
    /// Candidates.
    pub candidates: Option<Vec<Candidate>>,
    /// Usage metadata.
    pub usage_metadata: Option<UsageMetadata>,
    /// Prompt feedback.
    pub prompt_feedback: Option<PromptFeedback>,
    /// Model version.
    pub model_version: Option<String>,
}

/// Candidate.
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Candidate {
    /// Content.
    pub content: Option<CandidateContent>,
    /// Finish reason.
    pub finish_reason: Option<String>,
    /// Index.
    pub index: Option<usize>,
    /// Safety rating.
    pub safety_ratings: Option<Vec<SafetyRating>>,
}

/// Candidate content.
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub(crate) struct CandidateContent {
    /// Parts.
    pub parts: Option<Vec<CandidatePart>>,
    /// Role.
    pub role: Option<String>,
}

/// Candidate part.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CandidatePart {
    /// Text.
    pub text: Option<String>,
    /// Function call.
    pub function_call: Option<FunctionCall>,
}

/// Function call.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FunctionCall {
    /// Function name.
    pub name: String,
    /// Arguments.
    pub args: Option<serde_json::Value>,
    /// Partial-argument protocol is not a complete args object.
    pub partial_args: Option<serde_json::Value>,
    pub will_continue: Option<bool>,
}

/// Usage metadata.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UsageMetadata {
    /// Prompt token count.
    pub prompt_token_count: Option<u64>,
    /// Candidate token count.
    pub candidates_token_count: Option<u64>,
    /// Total token count.
    pub total_token_count: Option<u64>,
}

/// Prompt feedback.
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PromptFeedback {
    /// Block reason.
    pub block_reason: Option<String>,
    /// Safety rating.
    pub safety_ratings: Option<Vec<SafetyRating>>,
}

/// Safety rating.
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub(crate) struct SafetyRating {
    /// Category.
    pub category: Option<String>,
    /// Probability.
    pub probability: Option<String>,
}

// ============================================================================
// Event conversion
// ============================================================================

/// Response-local Gemini block allocation. Parts in separate SSE frames may
/// restart their array index, and function names are not call identities.
#[derive(Debug, Default)]
pub struct GeminiStreamState {
    next_index: usize,
    last_text_index: Option<usize>,
}

impl GeminiScheme {
    /// Convert SSE data to Event.
    ///
    /// # Arguments
    /// * `data` - SSE event data as a JSON string.
    ///
    /// # Returns
    /// * `Ok(Some(Vec<Event>))` - Conversion succeeded.
    /// * `Ok(None)` - Event ignored.
    /// * `Err(ClientError)` - Parse error.
    pub(crate) fn parse_event(
        &self,
        data: &str,
        state: &mut GeminiStreamState,
    ) -> Result<Option<Vec<Event>>, ClientError> {
        // Skip empty or invalid data.
        if data.is_empty() || data == "[DONE]" {
            return Ok(None);
        }

        let response: GenerateContentResponse =
            serde_json::from_str(data).map_err(|e| ClientError::Api {
                status: None,
                code: Some("parse_error".to_string()),
                message: format!("Failed to parse Gemini SSE data: {} -> {}", e, data),
                retry_after: None,
            })?;

        let mut events = Vec::new();

        // Usage metadata.
        if let Some(usage) = response.usage_metadata {
            events.push(self.convert_usage(&usage));
        }

        // Process candidates.
        if let Some(candidates) = response.candidates {
            for candidate in candidates {
                if let Some(content) = candidate.content {
                    if let Some(parts) = content.parts {
                        for part in &parts {
                            let index = state.next_index;
                            state.next_index += 1;
                            // Text delta.
                            if let Some(text) = &part.text {
                                if !text.is_empty() {
                                    // Gemini sends no explicit BlockStart, so emit TextDelta
                                    // directly (Timeline handles the implicit start).
                                    state.last_text_index = Some(index);
                                    events.push(Event::text_delta(index, text.clone()));
                                }
                            }

                            // Function call.
                            if let Some(function_call) = &part.function_call {
                                if function_call.partial_args.is_some()
                                    || function_call.will_continue.is_some()
                                {
                                    return Err(ClientError::Api {
                                        status: None,
                                        code: Some("unsupported_partial_function_call".into()),
                                        message: "Gemini partial function arguments require an unsupported streaming protocol".into(),
                                        retry_after: None,
                                    });
                                }
                                // These synthetic IDs correlate calls within a response;
                                // durable identity still comes from Host history entries.
                                let function_id = format!("call_{index}");

                                events.push(Event::BlockStart(BlockStart {
                                    index,
                                    block_type: BlockType::ToolUse,
                                    metadata: BlockMetadata::ToolUse {
                                        id: function_id,
                                        name: function_call.name.clone(),
                                    },
                                }));

                                // Send arguments as a delta if present.
                                if let Some(args) = &function_call.args {
                                    let args_str = serde_json::to_string(args).unwrap_or_default();
                                    if !args_str.is_empty() && args_str != "null" {
                                        events.push(Event::tool_input_delta(index, args_str));
                                    }
                                }
                                // Each args object is complete in this wire part. Close
                                // its collector before the next function call starts;
                                // dispatch still waits for the response-complete gate.
                                events.push(Event::tool_use_stop(index));
                            }
                        }
                    }
                }

                // Finish reason.
                if let Some(finish_reason) = candidate.finish_reason {
                    let stop_reason = match finish_reason.as_str() {
                        "STOP" => Some(StopReason::EndTurn),
                        "MAX_TOKENS" => Some(StopReason::MaxTokens),
                        "SAFETY" | "RECITATION" | "OTHER" => Some(StopReason::EndTurn),
                        _ => None,
                    };

                    if let Some(index) = state.last_text_index.take() {
                        events.push(Event::BlockStop(BlockStop {
                            index,
                            block_type: BlockType::Text,
                            stop_reason,
                            reasoning: None,
                        }));
                    }
                }
            }
        }

        if events.is_empty() {
            Ok(None)
        } else {
            Ok(Some(events))
        }
    }

    fn convert_usage(&self, usage: &UsageMetadata) -> Event {
        Event::Usage(UsageEvent {
            input_tokens: usage.prompt_token_count,
            output_tokens: usage.candidates_token_count,
            total_tokens: usage.total_token_count,
            cache_read_input_tokens: None,
            cache_creation_input_tokens: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm_client::event::DeltaContent;

    #[test]
    fn test_parse_text_response() {
        let scheme = GeminiScheme::new();
        let data =
            r#"{"candidates":[{"content":{"parts":[{"text":"Hello"}],"role":"model"},"index":0}]}"#;

        let events = scheme
            .parse_event(data, &mut GeminiStreamState::default())
            .unwrap()
            .unwrap();
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
    fn test_parse_usage_metadata() {
        let scheme = GeminiScheme::new();
        let data = r#"{"candidates":[{"content":{"parts":[{"text":"Hi"}],"role":"model"},"index":0}],"usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":5,"totalTokenCount":15}}"#;

        let events = scheme
            .parse_event(data, &mut GeminiStreamState::default())
            .unwrap()
            .unwrap();

        // Should contain a Usage event.
        let usage_event = events.iter().find(|e| matches!(e, Event::Usage(_)));
        assert!(usage_event.is_some());

        if let Event::Usage(usage) = usage_event.unwrap() {
            assert_eq!(usage.input_tokens, Some(10));
            assert_eq!(usage.output_tokens, Some(5));
            assert_eq!(usage.total_tokens, Some(15));
        }
    }

    #[test]
    fn test_parse_function_call() {
        let scheme = GeminiScheme::new();
        let data = r#"{"candidates":[{"content":{"parts":[{"functionCall":{"name":"get_weather","args":{"location":"Tokyo"}}}],"role":"model"},"index":0}]}"#;

        let events = scheme
            .parse_event(data, &mut GeminiStreamState::default())
            .unwrap()
            .unwrap();

        // Should contain a BlockStart event.
        let start_event = events.iter().find(|e| matches!(e, Event::BlockStart(_)));
        assert!(start_event.is_some());

        if let Event::BlockStart(start) = start_event.unwrap() {
            assert_eq!(start.block_type, BlockType::ToolUse);
            if let BlockMetadata::ToolUse { id: _, name } = &start.metadata {
                assert_eq!(name, "get_weather");
            } else {
                panic!("Expected ToolUse metadata");
            }
        }

        // Should also contain an argument delta.
        let delta_event = events.iter().find(|e| {
            if let Event::BlockDelta(d) = e {
                matches!(d.delta, DeltaContent::InputJson(_))
            } else {
                false
            }
        });
        assert!(delta_event.is_some());
    }

    #[test]
    fn test_parse_finish_reason() {
        let scheme = GeminiScheme::new();
        let data = r#"{"candidates":[{"content":{"parts":[{"text":"Done"}],"role":"model"},"finishReason":"STOP","index":0}]}"#;

        let events = scheme
            .parse_event(data, &mut GeminiStreamState::default())
            .unwrap()
            .unwrap();

        // Should contain a BlockStop event.
        let stop_event = events.iter().find(|e| matches!(e, Event::BlockStop(_)));
        assert!(stop_event.is_some());

        if let Event::BlockStop(stop) = stop_event.unwrap() {
            assert_eq!(stop.stop_reason, Some(StopReason::EndTurn));
        }
    }

    #[test]
    fn same_name_calls_have_distinct_ids_and_block_indexes_across_parts_and_frames() {
        use crate::llm_client::scheme::Scheme;
        let scheme = GeminiScheme::new();
        for frames in [
            vec![
                r#"{"candidates":[{"content":{"parts":[{"functionCall":{"name":"echo","args":{"text":"first"}}},{"functionCall":{"name":"echo","args":{"text":"second"}}}]}}]}"#,
            ],
            vec![
                r#"{"candidates":[{"content":{"parts":[{"functionCall":{"name":"echo","args":{"text":"first"}}}]}}]}"#,
                r#"{"candidates":[{"content":{"parts":[{"functionCall":{"name":"echo","args":{"text":"second"}}}]}}]}"#,
            ],
        ] {
            let mut state = Default::default();
            let mut events = Vec::new();
            for frame in frames {
                events.extend(scheme.parse_sse("", frame, &mut state).unwrap());
            }
            let calls: Vec<_> = events
                .iter()
                .filter_map(|event| match event {
                    Event::BlockStart(BlockStart {
                        index,
                        metadata: BlockMetadata::ToolUse { id, name },
                        ..
                    }) => Some((*index, id.as_str(), name.as_str())),
                    _ => None,
                })
                .collect();
            assert_eq!(calls, [(0, "call_0", "echo"), (1, "call_1", "echo")]);
            let inputs: Vec<_> = events
                .iter()
                .filter_map(|event| match event {
                    Event::BlockDelta(delta) => match &delta.delta {
                        DeltaContent::InputJson(json) => Some((delta.index, json.as_str())),
                        _ => None,
                    },
                    _ => None,
                })
                .collect();
            assert_eq!(
                inputs,
                [(0, r#"{"text":"first"}"#), (1, r#"{"text":"second"}"#)]
            );
        }
    }

    #[test]
    fn partial_argument_protocol_is_not_misread_as_an_empty_complete_call() {
        use crate::llm_client::scheme::Scheme;
        let scheme = GeminiScheme::new();
        for call in [
            r#"{"name":"echo","partialArgs":[{"jsonPath":"$.text","stringValue":"part"}]}"#,
            r#"{"name":"echo","willContinue":true,"args":{"text":"part"}}"#,
            r#"{"name":"echo","willContinue":false}"#,
        ] {
            let data = format!(
                r#"{{"candidates":[{{"content":{{"parts":[{{"functionCall":{call}}}]}}}}]}}"#
            );
            assert!(
                matches!(scheme.parse_sse("", &data, &mut Default::default()), Err(ClientError::Api { code: Some(code), .. }) if code == "unsupported_partial_function_call")
            );
        }
    }

    #[test]
    fn test_parse_empty_data() {
        let scheme = GeminiScheme::new();
        assert!(
            scheme
                .parse_event("", &mut GeminiStreamState::default())
                .unwrap()
                .is_none()
        );
        assert!(
            scheme
                .parse_event("[DONE]", &mut GeminiStreamState::default())
                .unwrap()
                .is_none()
        );
    }
}
