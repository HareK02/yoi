//! Gemini Request Builder
//!
//! Converts Open Responses native Item model to Google Gemini API format.

use serde::Serialize;
use serde_json::Value;

use crate::llm_client::{
    Request,
    capability::{ModelCapability, ReasoningControl, ReasoningSupport},
    types::{Item, Role, ToolDefinition, parse_tool_arguments},
};

use super::GeminiScheme;

/// Gemini API request body
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GeminiRequest {
    /// Contents (conversation history)
    pub contents: Vec<GeminiContent>,
    /// System instruction
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_instruction: Option<GeminiContent>,
    /// Tool definitions
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<GeminiTool>,
    /// Tool config
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_config: Option<GeminiToolConfig>,
    /// Generation config
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generation_config: Option<GeminiGenerationConfig>,
}

/// Gemini content
#[derive(Debug, Serialize)]
pub(crate) struct GeminiContent {
    /// Role
    pub role: String,
    /// Parts
    pub parts: Vec<GeminiPart>,
}

/// Gemini part
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub(crate) enum GeminiPart {
    /// Text part
    Text { text: String },
    /// Function call part
    FunctionCall {
        #[serde(rename = "functionCall")]
        function_call: GeminiFunctionCall,
    },
    /// Function response part
    FunctionResponse {
        #[serde(rename = "functionResponse")]
        function_response: GeminiFunctionResponse,
    },
}

/// Gemini function call
#[derive(Debug, Serialize)]
pub(crate) struct GeminiFunctionCall {
    pub name: String,
    pub args: Value,
}

/// Gemini function response
#[derive(Debug, Serialize)]
pub(crate) struct GeminiFunctionResponse {
    pub name: String,
    pub response: GeminiFunctionResponseContent,
}

/// Gemini function response content
#[derive(Debug, Serialize)]
pub(crate) struct GeminiFunctionResponseContent {
    pub name: String,
    pub content: Value,
}

/// Gemini tool definition
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GeminiTool {
    /// Function declarations
    pub function_declarations: Vec<GeminiFunctionDeclaration>,
}

/// Gemini function declaration
#[derive(Debug, Serialize)]
pub(crate) struct GeminiFunctionDeclaration {
    /// Function name
    pub name: String,
    /// Description
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Parameter schema
    pub parameters: Value,
}

/// Gemini tool config
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GeminiToolConfig {
    /// Function calling config
    pub function_calling_config: GeminiFunctionCallingConfig,
}

/// Gemini function calling config
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GeminiFunctionCallingConfig {
    /// Mode: AUTO, ANY, NONE
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// Enable streaming function call arguments
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_function_call_arguments: Option<bool>,
}

/// Gemini generation config
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GeminiGenerationConfig {
    /// Max output tokens
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    /// Temperature
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    /// Top P
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    /// Top K
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,
    /// Stop sequences
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub stop_sequences: Vec<String>,
    /// Thinking / reasoning 設定（Gemini 2.5 以降）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_config: Option<GeminiThinkingConfig>,
}

/// Gemini thinking config (gemini-2.5 以降)
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GeminiThinkingConfig {
    /// Token budget for thinking. `-1` means dynamic.
    pub thinking_budget: i32,
}

impl GeminiScheme {
    /// Build Gemini request from Request
    pub(crate) fn build_request(
        &self,
        request: &Request,
        capability: &ModelCapability,
    ) -> GeminiRequest {
        let contents = self.convert_items_to_contents(&request.items);

        // System prompt
        let system_instruction = request.system_prompt.as_ref().map(|s| GeminiContent {
            role: "user".to_string(),
            parts: vec![GeminiPart::Text { text: s.clone() }],
        });

        // Tools
        let tools = if request.tools.is_empty() {
            vec![]
        } else {
            vec![GeminiTool {
                function_declarations: request.tools.iter().map(|t| self.convert_tool(t)).collect(),
            }]
        };

        // Tool config
        let tool_config = if !request.tools.is_empty() {
            Some(GeminiToolConfig {
                function_calling_config: GeminiFunctionCallingConfig {
                    mode: Some("AUTO".to_string()),
                    stream_function_call_arguments: if self.stream_function_call_arguments {
                        Some(true)
                    } else {
                        None
                    },
                },
            })
        } else {
            None
        };

        // Reasoning の投影: capability が BudgetTokens / Both をサポートし、
        // request 側で budget_tokens が指定されているときだけ thinking_config を付ける。
        let supports_budget = matches!(
            capability.reasoning,
            Some(ReasoningSupport::BudgetTokens | ReasoningSupport::Both),
        );
        let thinking_config = request
            .config
            .reasoning
            .as_ref()
            .filter(|_| supports_budget)
            .and_then(|rc| match rc {
                ReasoningControl::BudgetTokens(budget) => Some(GeminiThinkingConfig {
                    thinking_budget: *budget,
                }),
                ReasoningControl::Effort(_) => None,
            });

        // Generation config
        let generation_config = Some(GeminiGenerationConfig {
            max_output_tokens: request.config.max_tokens,
            temperature: request.config.temperature,
            top_p: request.config.top_p,
            top_k: request.config.top_k,
            stop_sequences: request.config.stop_sequences.clone(),
            thinking_config,
        });

        GeminiRequest {
            contents,
            system_instruction,
            tools,
            tool_config,
            generation_config,
        }
    }

    /// Convert Open Responses Items to Gemini Contents
    ///
    /// Gemini uses:
    /// - role "user" for user messages and function responses
    /// - role "model" for assistant messages and function calls
    fn convert_items_to_contents(&self, items: &[Item]) -> Vec<GeminiContent> {
        let mut contents = Vec::new();
        let mut pending_model_parts: Vec<GeminiPart> = Vec::new();
        let mut pending_user_parts: Vec<GeminiPart> = Vec::new();

        for item in items {
            match item {
                Item::Message { role, content, .. } => {
                    // Flush pending parts
                    self.flush_pending_parts(
                        &mut contents,
                        &mut pending_model_parts,
                        &mut pending_user_parts,
                    );

                    let gemini_role = match role {
                        Role::User | Role::System => "user",
                        Role::Assistant => "model",
                    };

                    let parts: Vec<GeminiPart> = content
                        .iter()
                        .map(|p| GeminiPart::Text {
                            text: p.as_text().to_string(),
                        })
                        .collect();

                    contents.push(GeminiContent {
                        role: gemini_role.to_string(),
                        parts,
                    });
                }

                Item::ToolCall {
                    name, arguments, ..
                } => {
                    // Flush pending user parts first
                    if !pending_user_parts.is_empty() {
                        contents.push(GeminiContent {
                            role: "user".to_string(),
                            parts: std::mem::take(&mut pending_user_parts),
                        });
                    }

                    // Parse arguments (normalize non-object / legacy "null" payloads to {})
                    let args = parse_tool_arguments(arguments);

                    pending_model_parts.push(GeminiPart::FunctionCall {
                        function_call: GeminiFunctionCall {
                            name: name.clone(),
                            args,
                        },
                    });
                }

                Item::ToolResult {
                    call_id,
                    summary,
                    content,
                    ..
                } => {
                    // Flush pending model parts first
                    if !pending_model_parts.is_empty() {
                        contents.push(GeminiContent {
                            role: "model".to_string(),
                            parts: std::mem::take(&mut pending_model_parts),
                        });
                    }

                    let text = match content {
                        Some(c) => format!("{summary}\n{c}"),
                        None => summary.clone(),
                    };
                    pending_user_parts.push(GeminiPart::FunctionResponse {
                        function_response: GeminiFunctionResponse {
                            name: call_id.clone(),
                            response: GeminiFunctionResponseContent {
                                name: call_id.clone(),
                                content: Value::String(text),
                            },
                        },
                    });
                }

                Item::Reasoning { text, .. } => {
                    // Flush pending user parts first
                    if !pending_user_parts.is_empty() {
                        contents.push(GeminiContent {
                            role: "user".to_string(),
                            parts: std::mem::take(&mut pending_user_parts),
                        });
                    }

                    // Reasoning is treated as model text in Gemini
                    pending_model_parts.push(GeminiPart::Text { text: text.clone() });
                }
            }
        }

        // Flush remaining pending parts
        self.flush_pending_parts(
            &mut contents,
            &mut pending_model_parts,
            &mut pending_user_parts,
        );

        contents
    }

    fn flush_pending_parts(
        &self,
        contents: &mut Vec<GeminiContent>,
        pending_model_parts: &mut Vec<GeminiPart>,
        pending_user_parts: &mut Vec<GeminiPart>,
    ) {
        if !pending_model_parts.is_empty() {
            contents.push(GeminiContent {
                role: "model".to_string(),
                parts: std::mem::take(pending_model_parts),
            });
        }
        if !pending_user_parts.is_empty() {
            contents.push(GeminiContent {
                role: "user".to_string(),
                parts: std::mem::take(pending_user_parts),
            });
        }
    }

    fn convert_tool(&self, tool: &ToolDefinition) -> GeminiFunctionDeclaration {
        GeminiFunctionDeclaration {
            name: tool.name.clone(),
            description: tool.description.clone(),
            parameters: tool.input_schema.clone(),
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
            vision: true,
            prompt_caching: CacheStrategy::Auto,
        }
    }

    fn cap_budget_reasoning() -> ModelCapability {
        ModelCapability {
            reasoning: Some(ReasoningSupport::BudgetTokens),
            ..cap()
        }
    }

    #[test]
    fn test_build_simple_request() {
        let scheme = GeminiScheme::new();
        let request = Request::new()
            .system("You are a helpful assistant.")
            .user("Hello!");

        let gemini_req = scheme.build_request(&request, &cap());

        assert!(gemini_req.system_instruction.is_some());
        assert_eq!(gemini_req.contents.len(), 1);
        assert_eq!(gemini_req.contents[0].role, "user");
    }

    #[test]
    fn test_build_request_with_tool() {
        let scheme = GeminiScheme::new();
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

        let gemini_req = scheme.build_request(&request, &cap());

        assert_eq!(gemini_req.tools.len(), 1);
        assert_eq!(gemini_req.tools[0].function_declarations.len(), 1);
        assert_eq!(
            gemini_req.tools[0].function_declarations[0].name,
            "get_weather"
        );
        assert!(gemini_req.tool_config.is_some());
    }

    #[test]
    fn test_assistant_role_is_model() {
        let scheme = GeminiScheme::new();
        let request = Request::new().user("Hello").assistant("Hi there!");

        let gemini_req = scheme.build_request(&request, &cap());

        assert_eq!(gemini_req.contents.len(), 2);
        assert_eq!(gemini_req.contents[0].role, "user");
        assert_eq!(gemini_req.contents[1].role, "model");
    }

    #[test]
    fn test_tool_call_and_result() {
        let scheme = GeminiScheme::new();
        let request = Request::new()
            .user("What's the weather?")
            .item(Item::tool_call(
                "call_123",
                "get_weather",
                r#"{"city":"Tokyo"}"#,
            ))
            .item(Item::tool_result("call_123", "Sunny, 25°C"));

        let gemini_req = scheme.build_request(&request, &cap());

        assert_eq!(gemini_req.contents.len(), 3);
        assert_eq!(gemini_req.contents[0].role, "user");
        assert_eq!(gemini_req.contents[1].role, "model");
        assert_eq!(gemini_req.contents[2].role, "user");
    }

    #[test]
    fn thinking_budget_projected_when_supported() {
        let scheme = GeminiScheme::new();
        let mut request = Request::new().user("think");
        request.config.reasoning = Some(ReasoningControl::BudgetTokens(-1));

        let gemini_req = scheme.build_request(&request, &cap_budget_reasoning());
        let config = gemini_req.generation_config.expect("generation config");
        let thinking = config.thinking_config.expect("thinking config");

        assert_eq!(thinking.thinking_budget, -1);
    }

    #[test]
    fn effort_reasoning_not_projected_to_gemini() {
        let scheme = GeminiScheme::new();
        let mut request = Request::new().user("think");
        request.config.reasoning = Some(ReasoningControl::Effort(ReasoningEffort::Medium));

        let gemini_req = scheme.build_request(&request, &cap_budget_reasoning());
        let config = gemini_req.generation_config.expect("generation config");

        assert!(config.thinking_config.is_none());
    }
}
