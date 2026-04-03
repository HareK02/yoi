//! Open Responses Request Builder
//!
//! Converts internal Request/Item types to Open Responses API format.
//! Since our internal types are already Open Responses native, this is
//! mostly a direct serialization with some field renaming.

use serde::Serialize;
use serde_json::Value;

use crate::llm_client::{types::Item, Request, ToolDefinition};

/// Open Responses API request body
#[derive(Debug, Serialize)]
pub struct OpenResponsesRequest {
    /// Model identifier
    pub model: String,

    /// Input items (conversation history)
    pub input: Vec<OpenResponsesItem>,

    /// System instructions
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,

    /// Tool definitions
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<OpenResponsesTool>,

    /// Enable streaming
    pub stream: bool,

    /// Maximum output tokens
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,

    /// Temperature
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,

    /// Top P (nucleus sampling)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
}

/// Open Responses input item
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OpenResponsesItem {
    /// Message item
    Message {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        role: String,
        content: Vec<OpenResponsesContentPart>,
    },

    /// Function call item
    FunctionCall {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        call_id: String,
        name: String,
        arguments: String,
    },

    /// Function call output item
    FunctionCallOutput {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        call_id: String,
        output: String,
    },

    /// Reasoning item
    Reasoning {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        text: String,
    },
}

/// Open Responses content part
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OpenResponsesContentPart {
    /// Input text (for user messages)
    InputText { text: String },

    /// Output text (for assistant messages)
    OutputText { text: String },

    /// Refusal
    Refusal { refusal: String },
}

/// Open Responses tool definition
#[derive(Debug, Serialize)]
pub struct OpenResponsesTool {
    /// Tool type (always "function")
    pub r#type: String,

    /// Function definition
    pub name: String,

    /// Description
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Parameters schema
    pub parameters: Value,
}

/// Build Open Responses request from internal Request
pub fn build_request(model: &str, request: &Request) -> OpenResponsesRequest {
    let input = request.items.iter().map(convert_item).collect();
    let tools = request.tools.iter().map(convert_tool).collect();

    OpenResponsesRequest {
        model: model.to_string(),
        input,
        instructions: request.system_prompt.clone(),
        tools,
        stream: true,
        max_output_tokens: request.config.max_tokens,
        temperature: request.config.temperature,
        top_p: request.config.top_p,
    }
}

fn convert_item(item: &Item) -> OpenResponsesItem {
    match item {
        Item::Message {
            id,
            role,
            content,
            status: _,
        } => {
            let role_str = match role {
                crate::llm_client::types::Role::User => "user",
                crate::llm_client::types::Role::Assistant => "assistant",
                crate::llm_client::types::Role::System => "system",
            };

            let parts = content
                .iter()
                .map(|p| match p {
                    crate::llm_client::types::ContentPart::InputText { text } => {
                        OpenResponsesContentPart::InputText { text: text.clone() }
                    }
                    crate::llm_client::types::ContentPart::OutputText { text } => {
                        OpenResponsesContentPart::OutputText { text: text.clone() }
                    }
                    crate::llm_client::types::ContentPart::Refusal { refusal } => {
                        OpenResponsesContentPart::Refusal {
                            refusal: refusal.clone(),
                        }
                    }
                })
                .collect();

            OpenResponsesItem::Message {
                id: id.clone(),
                role: role_str.to_string(),
                content: parts,
            }
        }

        Item::FunctionCall {
            id,
            call_id,
            name,
            arguments,
            status: _,
        } => OpenResponsesItem::FunctionCall {
            id: id.clone(),
            call_id: call_id.clone(),
            name: name.clone(),
            arguments: arguments.clone(),
        },

        Item::FunctionCallOutput {
            id,
            call_id,
            output,
        } => OpenResponsesItem::FunctionCallOutput {
            id: id.clone(),
            call_id: call_id.clone(),
            output: output.clone(),
        },

        Item::Reasoning {
            id,
            text,
            status: _,
        } => OpenResponsesItem::Reasoning {
            id: id.clone(),
            text: text.clone(),
        },
    }
}

fn convert_tool(tool: &ToolDefinition) -> OpenResponsesTool {
    OpenResponsesTool {
        r#type: "function".to_string(),
        name: tool.name.clone(),
        description: tool.description.clone(),
        parameters: tool.input_schema.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm_client::types::Item;

    #[test]
    fn test_build_simple_request() {
        let request = Request::new()
            .system("You are a helpful assistant.")
            .user("Hello!");

        let or_req = build_request("gpt-4o", &request);

        assert_eq!(or_req.model, "gpt-4o");
        assert_eq!(
            or_req.instructions,
            Some("You are a helpful assistant.".to_string())
        );
        assert_eq!(or_req.input.len(), 1);
        assert!(or_req.stream);
    }

    #[test]
    fn test_build_request_with_tool() {
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

        let or_req = build_request("gpt-4o", &request);

        assert_eq!(or_req.tools.len(), 1);
        assert_eq!(or_req.tools[0].name, "get_weather");
        assert_eq!(or_req.tools[0].r#type, "function");
    }

    #[test]
    fn test_function_call_and_output() {
        let request = Request::new()
            .user("What's the weather?")
            .item(Item::function_call(
                "call_123",
                "get_weather",
                r#"{"city":"Tokyo"}"#,
            ))
            .item(Item::function_call_output("call_123", "Sunny, 25°C"));

        let or_req = build_request("gpt-4o", &request);

        assert_eq!(or_req.input.len(), 3);

        // Check function call
        if let OpenResponsesItem::FunctionCall { call_id, name, .. } = &or_req.input[1] {
            assert_eq!(call_id, "call_123");
            assert_eq!(name, "get_weather");
        } else {
            panic!("Expected FunctionCall");
        }

        // Check function call output
        if let OpenResponsesItem::FunctionCallOutput { call_id, output, .. } = &or_req.input[2] {
            assert_eq!(call_id, "call_123");
            assert_eq!(output, "Sunny, 25°C");
        } else {
            panic!("Expected FunctionCallOutput");
        }
    }
}
