//! OpenAI Request Builder
//!
//! Converts Open Responses native Item model to OpenAI Chat Completions API format.

use serde::Serialize;
use serde_json::Value;

use crate::llm_client::{
    Request,
    capability::{ModelCapability, ReasoningControl, ReasoningSupport},
    types::{Item, Role, ToolDefinition, parse_tool_arguments},
};

use super::OpenAIScheme;

/// OpenAI API request body
#[derive(Debug, Serialize)]
pub(crate) struct OpenAIRequest {
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_completion_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>, // Legacy field for compatibility (e.g. Ollama)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub stop: Vec<String>,
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_options: Option<StreamOptions>,
    pub messages: Vec<OpenAIMessage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<OpenAITool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<String>,
    /// Reasoning effort（o1 / o3 / o4 / gpt-5 系で有効）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct StreamOptions {
    pub include_usage: bool,
}

/// OpenAI message
#[derive(Debug, Serialize)]
pub(crate) struct OpenAIMessage {
    pub role: String,
    pub content: Option<OpenAIContent>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<OpenAIToolCall>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// OpenAI content
#[allow(dead_code)]
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub(crate) enum OpenAIContent {
    Text(String),
    Parts(Vec<OpenAIContentPart>),
}

/// OpenAI content part
#[allow(dead_code)]
#[derive(Debug, Serialize)]
#[serde(tag = "type")]
pub(crate) enum OpenAIContentPart {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image_url")]
    ImageUrl { image_url: ImageUrl },
}

#[derive(Debug, Serialize)]
pub(crate) struct ImageUrl {
    pub url: String,
}

/// OpenAI tool definition
#[derive(Debug, Serialize)]
pub(crate) struct OpenAITool {
    pub r#type: String,
    pub function: OpenAIToolFunction,
}

#[derive(Debug, Serialize)]
pub(crate) struct OpenAIToolFunction {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub parameters: Value,
}

/// OpenAI tool call in message
#[derive(Debug, Serialize)]
pub(crate) struct OpenAIToolCall {
    pub id: String,
    pub r#type: String,
    pub function: OpenAIToolCallFunction,
}

#[derive(Debug, Serialize)]
pub(crate) struct OpenAIToolCallFunction {
    pub name: String,
    pub arguments: String,
}

impl OpenAIScheme {
    /// Build OpenAI request from Request
    pub(crate) fn build_request(
        &self,
        model: &str,
        request: &Request,
        capability: &ModelCapability,
    ) -> OpenAIRequest {
        let mut messages = Vec::new();

        // Add system message if present
        if let Some(system) = &request.system_prompt {
            messages.push(OpenAIMessage {
                role: "system".to_string(),
                content: Some(OpenAIContent::Text(system.clone())),
                tool_calls: vec![],
                tool_call_id: None,
                name: None,
            });
        }

        // Convert items to messages
        messages.extend(self.convert_items_to_messages(&request.items));

        let tools = request.tools.iter().map(|t| self.convert_tool(t)).collect();

        let (max_tokens, max_completion_tokens) = if self.use_legacy_max_tokens {
            (request.config.max_tokens, None)
        } else {
            (None, request.config.max_tokens)
        };

        // Reasoning の投影: capability が Effort / Both をサポートし、
        // request 側で effort が指定されているときだけ reasoning_effort を付ける。
        let supports_effort = matches!(
            capability.reasoning,
            Some(ReasoningSupport::Effort | ReasoningSupport::Both),
        );
        let reasoning_effort = request
            .config
            .reasoning
            .as_ref()
            .filter(|_| supports_effort)
            .and_then(|rc| match rc {
                ReasoningControl::Effort(effort) => Some(effort.as_str().to_string()),
                ReasoningControl::BudgetTokens(_) => None,
            });

        OpenAIRequest {
            model: model.to_string(),
            max_completion_tokens,
            max_tokens,
            temperature: request.config.temperature,
            top_p: request.config.top_p,
            stop: request.config.stop_sequences.clone(),
            stream: true,
            stream_options: Some(StreamOptions {
                include_usage: true,
            }),
            messages,
            tools,
            tool_choice: None,
            reasoning_effort,
        }
    }

    /// Convert Open Responses Items to OpenAI Messages
    ///
    /// OpenAI uses a message-based model where:
    /// - User messages have role "user"
    /// - Assistant messages have role "assistant"
    /// - Tool calls are within assistant messages as tool_calls array
    /// - Tool results have role "tool" with tool_call_id
    fn convert_items_to_messages(&self, items: &[Item]) -> Vec<OpenAIMessage> {
        let mut messages = Vec::new();
        let mut pending_tool_calls: Vec<OpenAIToolCall> = Vec::new();
        let mut pending_assistant_text: Option<String> = None;

        for item in items {
            match item {
                Item::Message { role, content, .. } => {
                    // Flush pending tool calls
                    self.flush_pending_assistant(
                        &mut messages,
                        &mut pending_tool_calls,
                        &mut pending_assistant_text,
                    );

                    let openai_role = match role {
                        Role::User => "user",
                        Role::Assistant => "assistant",
                        Role::System => "system",
                    };

                    let text_content: String = content
                        .iter()
                        .map(|p| p.as_text())
                        .collect::<Vec<_>>()
                        .join("");

                    messages.push(OpenAIMessage {
                        role: openai_role.to_string(),
                        content: Some(OpenAIContent::Text(text_content)),
                        tool_calls: vec![],
                        tool_call_id: None,
                        name: None,
                    });
                }

                Item::ToolCall {
                    call_id,
                    name,
                    arguments,
                    ..
                } => {
                    // Normalize non-object / legacy "null" payloads to "{}" so
                    // OpenAI gets a valid JSON object string.
                    let normalized_args = parse_tool_arguments(arguments).to_string();
                    pending_tool_calls.push(OpenAIToolCall {
                        id: call_id.clone(),
                        r#type: "function".to_string(),
                        function: OpenAIToolCallFunction {
                            name: name.clone(),
                            arguments: normalized_args,
                        },
                    });
                }

                Item::ToolResult {
                    call_id,
                    summary,
                    content,
                    ..
                } => {
                    // Flush pending tool calls before tool result
                    self.flush_pending_assistant(
                        &mut messages,
                        &mut pending_tool_calls,
                        &mut pending_assistant_text,
                    );

                    let text = match content {
                        Some(c) => format!("{summary}\n{c}"),
                        None => summary.clone(),
                    };
                    messages.push(OpenAIMessage {
                        role: "tool".to_string(),
                        content: Some(OpenAIContent::Text(text)),
                        tool_calls: vec![],
                        tool_call_id: Some(call_id.clone()),
                        name: None,
                    });
                }

                Item::Reasoning { text, .. } => {
                    // Reasoning is treated as assistant text in OpenAI
                    // (OpenAI doesn't have native reasoning support like Claude)
                    if let Some(ref mut existing) = pending_assistant_text {
                        existing.push_str(text);
                    } else {
                        pending_assistant_text = Some(text.clone());
                    }
                }
            }
        }

        // Flush remaining pending items
        self.flush_pending_assistant(
            &mut messages,
            &mut pending_tool_calls,
            &mut pending_assistant_text,
        );

        messages
    }

    fn flush_pending_assistant(
        &self,
        messages: &mut Vec<OpenAIMessage>,
        pending_tool_calls: &mut Vec<OpenAIToolCall>,
        pending_assistant_text: &mut Option<String>,
    ) {
        if !pending_tool_calls.is_empty() || pending_assistant_text.is_some() {
            messages.push(OpenAIMessage {
                role: "assistant".to_string(),
                content: pending_assistant_text.take().map(OpenAIContent::Text),
                tool_calls: std::mem::take(pending_tool_calls),
                tool_call_id: None,
                name: None,
            });
        }
    }

    fn convert_tool(&self, tool: &ToolDefinition) -> OpenAITool {
        OpenAITool {
            r#type: "function".to_string(),
            function: OpenAIToolFunction {
                name: tool.name.clone(),
                description: tool.description.clone(),
                parameters: tool.input_schema.clone(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm_client::capability::{
        CacheStrategy, ReasoningEffort, StructuredOutput, ToolCallingSupport,
    };

    fn cap() -> ModelCapability {
        ModelCapability {
            tool_calling: ToolCallingSupport::Parallel,
            structured_output: StructuredOutput::JsonSchema,
            reasoning: None,
            vision: false,
            prompt_caching: CacheStrategy::Auto,
        }
    }

    #[test]
    fn test_build_simple_request() {
        let scheme = OpenAIScheme::new();
        let request = Request::new().system("System prompt").user("Hello");

        let body = scheme.build_request("gpt-4o", &request, &cap());

        assert_eq!(body.model, "gpt-4o");
        assert_eq!(body.messages.len(), 2);
        assert_eq!(body.messages[0].role, "system");
        assert_eq!(body.messages[1].role, "user");

        if let Some(OpenAIContent::Text(text)) = &body.messages[0].content {
            assert_eq!(text, "System prompt");
        } else {
            panic!("Expected text content");
        }
    }

    #[test]
    fn test_build_request_with_tool() {
        let scheme = OpenAIScheme::new();
        let request = Request::new()
            .user("Check weather")
            .tool(ToolDefinition::new("weather").description("Get weather"));

        let body = scheme.build_request("gpt-4o", &request, &cap());
        assert_eq!(body.tools.len(), 1);
        assert_eq!(body.tools[0].function.name, "weather");
    }

    #[test]
    fn test_build_request_legacy_max_tokens() {
        let scheme = OpenAIScheme::new().with_legacy_max_tokens(true);
        let request = Request::new().user("Hello").max_tokens(100);

        let body = scheme.build_request("llama3", &request, &cap());

        assert_eq!(body.max_tokens, Some(100));
        assert!(body.max_completion_tokens.is_none());
    }

    #[test]
    fn test_build_request_modern_max_tokens() {
        let scheme = OpenAIScheme::new();
        let request = Request::new().user("Hello").max_tokens(100);

        let body = scheme.build_request("gpt-4o", &request, &cap());

        assert_eq!(body.max_completion_tokens, Some(100));
        assert!(body.max_tokens.is_none());
    }

    #[test]
    fn reasoning_effort_projected_when_supported() {
        let scheme = OpenAIScheme::new();
        let mut request = Request::new().user("Hello");
        request.config.reasoning = Some(ReasoningControl::Effort(ReasoningEffort::Other(
            "provider-native".into(),
        )));
        let capability = ModelCapability {
            reasoning: Some(ReasoningSupport::Effort),
            ..cap()
        };

        let body = scheme.build_request("gpt-5", &request, &capability);

        assert_eq!(body.reasoning_effort.as_deref(), Some("provider-native"));
    }

    #[test]
    fn budget_reasoning_not_projected_to_openai_chat() {
        let scheme = OpenAIScheme::new();
        let mut request = Request::new().user("Hello");
        request.config.reasoning = Some(ReasoningControl::BudgetTokens(4096));
        let capability = ModelCapability {
            reasoning: Some(ReasoningSupport::Both),
            ..cap()
        };

        let body = scheme.build_request("gpt-5", &request, &capability);

        assert!(body.reasoning_effort.is_none());
    }

    #[test]
    fn test_tool_call_and_result() {
        let scheme = OpenAIScheme::new();
        let request = Request::new()
            .user("Check weather")
            .item(Item::tool_call(
                "call_123",
                "get_weather",
                r#"{"city":"Tokyo"}"#,
            ))
            .item(Item::tool_result("call_123", "Sunny, 25°C"));

        let body = scheme.build_request("gpt-4o", &request, &cap());

        assert_eq!(body.messages.len(), 3);
        assert_eq!(body.messages[0].role, "user");
        assert_eq!(body.messages[1].role, "assistant");
        assert_eq!(body.messages[1].tool_calls.len(), 1);
        assert_eq!(body.messages[2].role, "tool");
    }
}
