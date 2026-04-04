//! Anthropic Request Builder
//!
//! Converts Open Responses native Item model to Anthropic Messages API format.

use serde::Serialize;

use crate::llm_client::{
    Request,
    types::{ContentPart, Item, Role, ToolDefinition},
};

use super::AnthropicScheme;

/// Anthropic API request body
#[derive(Debug, Serialize)]
pub(crate) struct AnthropicRequest {
    pub model: String,
    pub max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    pub messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<AnthropicTool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub stop_sequences: Vec<String>,
    pub stream: bool,
}

/// Anthropic message
#[derive(Debug, Serialize)]
pub(crate) struct AnthropicMessage {
    pub role: String,
    pub content: AnthropicContent,
}

/// Anthropic content
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub(crate) enum AnthropicContent {
    Text(String),
    Parts(Vec<AnthropicContentPart>),
}

/// Anthropic content part
#[derive(Debug, Serialize)]
#[serde(tag = "type")]
pub(crate) enum AnthropicContentPart {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: String,
    },
}

/// Anthropic tool definition
#[derive(Debug, Serialize)]
pub(crate) struct AnthropicTool {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub input_schema: serde_json::Value,
}

impl AnthropicScheme {
    /// Build Anthropic request from Request
    pub(crate) fn build_request(&self, model: &str, request: &Request) -> AnthropicRequest {
        let messages = self.convert_items_to_messages(&request.items);
        let tools = request.tools.iter().map(|t| self.convert_tool(t)).collect();

        AnthropicRequest {
            model: model.to_string(),
            max_tokens: request.config.max_tokens.unwrap_or(4096),
            system: request.system_prompt.clone(),
            messages,
            tools,
            temperature: request.config.temperature,
            top_p: request.config.top_p,
            top_k: request.config.top_k,
            stop_sequences: request.config.stop_sequences.clone(),
            stream: true,
        }
    }

    /// Convert Open Responses Items to Anthropic Messages
    ///
    /// Anthropic uses a message-based model where:
    /// - User messages have role "user"
    /// - Assistant messages have role "assistant"
    /// - Tool calls are content parts within assistant messages
    /// - Tool results are content parts within user messages
    fn convert_items_to_messages(&self, items: &[Item]) -> Vec<AnthropicMessage> {
        let mut messages = Vec::new();
        let mut pending_assistant_parts: Vec<AnthropicContentPart> = Vec::new();
        let mut pending_user_parts: Vec<AnthropicContentPart> = Vec::new();

        for item in items {
            match item {
                Item::Message { role, content, .. } => {
                    // Flush pending parts before a new message
                    self.flush_pending_parts(
                        &mut messages,
                        &mut pending_assistant_parts,
                        &mut pending_user_parts,
                    );

                    let anthropic_role = match role {
                        Role::User => "user",
                        Role::Assistant => "assistant",
                        Role::System => continue, // Skip system role items
                    };

                    let parts: Vec<AnthropicContentPart> = content
                        .iter()
                        .map(|p| match p {
                            ContentPart::Text { text } => {
                                AnthropicContentPart::Text { text: text.clone() }
                            }
                            ContentPart::Refusal { refusal } => AnthropicContentPart::Text {
                                text: refusal.clone(),
                            },
                        })
                        .collect();

                    if parts.len() == 1 {
                        if let AnthropicContentPart::Text { text } = &parts[0] {
                            messages.push(AnthropicMessage {
                                role: anthropic_role.to_string(),
                                content: AnthropicContent::Text(text.clone()),
                            });
                        } else {
                            messages.push(AnthropicMessage {
                                role: anthropic_role.to_string(),
                                content: AnthropicContent::Parts(parts),
                            });
                        }
                    } else {
                        messages.push(AnthropicMessage {
                            role: anthropic_role.to_string(),
                            content: AnthropicContent::Parts(parts),
                        });
                    }
                }

                Item::ToolCall {
                    call_id,
                    name,
                    arguments,
                    ..
                } => {
                    // Flush pending user parts first
                    if !pending_user_parts.is_empty() {
                        messages.push(AnthropicMessage {
                            role: "user".to_string(),
                            content: AnthropicContent::Parts(std::mem::take(
                                &mut pending_user_parts,
                            )),
                        });
                    }

                    // Parse arguments JSON string to Value
                    let input = serde_json::from_str(arguments)
                        .unwrap_or_else(|_| serde_json::Value::Object(serde_json::Map::new()));

                    pending_assistant_parts.push(AnthropicContentPart::ToolUse {
                        id: call_id.clone(),
                        name: name.clone(),
                        input,
                    });
                }

                Item::ToolResult {
                    call_id, output, ..
                } => {
                    // Flush pending assistant parts first
                    if !pending_assistant_parts.is_empty() {
                        messages.push(AnthropicMessage {
                            role: "assistant".to_string(),
                            content: AnthropicContent::Parts(std::mem::take(
                                &mut pending_assistant_parts,
                            )),
                        });
                    }

                    pending_user_parts.push(AnthropicContentPart::ToolResult {
                        tool_use_id: call_id.clone(),
                        content: output.clone(),
                    });
                }

                Item::Reasoning { text, .. } => {
                    // Flush pending user parts first
                    if !pending_user_parts.is_empty() {
                        messages.push(AnthropicMessage {
                            role: "user".to_string(),
                            content: AnthropicContent::Parts(std::mem::take(
                                &mut pending_user_parts,
                            )),
                        });
                    }

                    // Reasoning is treated as assistant text in Anthropic
                    // (actual thinking blocks are handled differently in streaming)
                    pending_assistant_parts.push(AnthropicContentPart::Text { text: text.clone() });
                }
            }
        }

        // Flush remaining pending parts
        self.flush_pending_parts(
            &mut messages,
            &mut pending_assistant_parts,
            &mut pending_user_parts,
        );

        messages
    }

    fn flush_pending_parts(
        &self,
        messages: &mut Vec<AnthropicMessage>,
        pending_assistant_parts: &mut Vec<AnthropicContentPart>,
        pending_user_parts: &mut Vec<AnthropicContentPart>,
    ) {
        if !pending_assistant_parts.is_empty() {
            messages.push(AnthropicMessage {
                role: "assistant".to_string(),
                content: AnthropicContent::Parts(std::mem::take(pending_assistant_parts)),
            });
        }
        if !pending_user_parts.is_empty() {
            messages.push(AnthropicMessage {
                role: "user".to_string(),
                content: AnthropicContent::Parts(std::mem::take(pending_user_parts)),
            });
        }
    }

    fn convert_tool(&self, tool: &ToolDefinition) -> AnthropicTool {
        AnthropicTool {
            name: tool.name.clone(),
            description: tool.description.clone(),
            input_schema: tool.input_schema.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_simple_request() {
        let scheme = AnthropicScheme::new();
        let request = Request::new()
            .system("You are a helpful assistant.")
            .user("Hello!");

        let anthropic_req = scheme.build_request("claude-sonnet-4-20250514", &request);

        assert_eq!(anthropic_req.model, "claude-sonnet-4-20250514");
        assert_eq!(
            anthropic_req.system,
            Some("You are a helpful assistant.".to_string())
        );
        assert_eq!(anthropic_req.messages.len(), 1);
        assert!(anthropic_req.stream);
    }

    #[test]
    fn test_build_request_with_tool() {
        let scheme = AnthropicScheme::new();
        let request = Request::new().user("What's the weather?").tool(
            ToolDefinition::new("get_weather")
                .description("Get current weather")
                .input_schema(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "location": { "type": "string" }
                    },
                    "required": ["location"]
                })),
        );

        let anthropic_req = scheme.build_request("claude-sonnet-4-20250514", &request);

        assert_eq!(anthropic_req.tools.len(), 1);
        assert_eq!(anthropic_req.tools[0].name, "get_weather");
    }

    #[test]
    fn test_tool_call_and_result() {
        let scheme = AnthropicScheme::new();
        let request = Request::new()
            .user("What's the weather?")
            .item(Item::tool_call(
                "call_123",
                "get_weather",
                r#"{"city":"Tokyo"}"#,
            ))
            .item(Item::tool_result("call_123", "Sunny, 25°C"));

        let anthropic_req = scheme.build_request("claude-sonnet-4-20250514", &request);

        assert_eq!(anthropic_req.messages.len(), 3);
        assert_eq!(anthropic_req.messages[0].role, "user");
        assert_eq!(anthropic_req.messages[1].role, "assistant");
        assert_eq!(anthropic_req.messages[2].role, "user");
    }
}
