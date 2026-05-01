//! OpenAI Responses API リクエスト body 生成
//!
//! Chat Completions の `messages` と違い、Responses は `input[]` の
//! item 配列で reasoning / function_call / function_call_output が
//! first-class。`Item` を素に近い形で `input[]` に投影できる。

use serde::{Serialize, Serializer};
use serde_json::Value;

use crate::llm_client::{
    Request,
    capability::{ModelCapability, ReasoningControl, ReasoningSupport},
    types::{ContentPart, Item, Role, ToolDefinition, parse_tool_arguments},
};

use super::OpenAIResponsesScheme;

/// `/v1/responses` のリクエスト body。
#[derive(Debug, Serialize)]
pub(crate) struct ResponsesRequest {
    pub model: String,
    /// システムプロンプト相当。`input[]` とは別フィールド。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    pub input: Vec<InputItem>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ResponseTool>,
    /// 常時 `"auto"` を送る。scheme 固定値。
    pub tool_choice: &'static str,
    /// 常時 `true` を送る。scheme 固定値。
    pub parallel_tool_calls: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<ReasoningConfig>,
    /// ZDR / stateless 運用では `false`。
    pub store: bool,
    /// 常時 `true`。
    pub stream: bool,
    /// `["reasoning.encrypted_content"]` 等。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub include: Vec<&'static str>,
    /// 公式 OpenAI Responses API では受理されるが、ChatGPT backend
    /// (codex-oauth) は 400 で弾く。scheme の `send_max_output_tokens`
    /// が `false` のときは `None` のまま送る (skip_serializing_if で除外)。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    /// 公式 OpenAI Responses API では受理されるが、ChatGPT backend
    /// (codex-oauth) は `temperature` / `top_p` を 400 で弾く。scheme の
    /// `send_sampling_params` が `false` のときは `None` のまま送る。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    /// 会話単位の安定キー。ChatGPT backend (codex-oauth) は明示キーが
    /// 無いとプロンプトキャッシュがほぼ効かない。pod 側は `SessionId`
    /// を渡す。`Request::cache_key` が `None` のときはキー自体を送らない。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_cache_key: Option<String>,
}

/// reasoning 制御。
#[derive(Debug, Serialize)]
pub(crate) struct ReasoningConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// summary の出力制御。`"auto"` 固定で summary_text を受け取る。
    pub summary: &'static str,
}

/// `input[]` の 1 要素。
///
/// Responses API の item 型を素に近い形で投影する。未対応 type は
/// 無視（reasoning 送信時に `content: []` の場合は `None` として弾く）。
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum InputItem {
    /// 会話メッセージ。user / assistant / developer のいずれか。
    /// `Role::System` items は `developer` として投影する（ChatGPT
    /// backend が `role: "system"` を拒否するため。Codex CLI も
    /// system 相当の挿入には DeveloperInstructions = `role: "developer"`
    /// を使う）。
    Message {
        role: &'static str,
        content: Vec<InputContent>,
    },
    /// 過去の function tool 呼び出し（assistant 側）。
    FunctionCall {
        call_id: String,
        name: String,
        /// JSON 文字列（object でなくても正規化済み）。
        arguments: String,
    },
    /// function tool の結果（user 側）。
    FunctionCallOutput {
        call_id: String,
        /// Responses は文字列 or 構造化 output を許すが、ここでは
        /// `summary` + `content` を改行連結した文字列で送る。
        output: String,
    },
    /// reasoning item。`encrypted_content` があれば必ず添える。
    Reasoning {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        summary: Vec<ReasoningSummaryPart>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        content: Vec<ReasoningContentPart>,
        #[serde(skip_serializing_if = "Option::is_none")]
        encrypted_content: Option<String>,
    },
}

/// メッセージ content_part。role で input/output を使い分ける。
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum InputContent {
    /// user / developer 側のテキスト
    InputText { text: String },
    /// assistant 側のテキスト
    OutputText { text: String },
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum ReasoningSummaryPart {
    SummaryText { text: String },
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum ReasoningContentPart {
    ReasoningText { text: String },
}

/// Responses 用 tool 定義。Chat と違い function キーでネストせず
/// トップレベルに `name` / `parameters` が載る。
#[derive(Debug, Serialize)]
pub(crate) struct ResponseTool {
    #[serde(rename = "type")]
    pub r#type: &'static str,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// OpenAI Responses API は `type:"object"` のパラメータスキーマに
    /// `properties` が存在することを要求する。schemars は引数なし struct
    /// から `properties` を含まない最小スキーマを出すので、serialize
    /// 時に空オブジェクトを補う。
    #[serde(serialize_with = "serialize_parameters")]
    pub parameters: Value,
    /// Structured output モード制御。デフォルト false。
    pub strict: bool,
}

fn serialize_parameters<S: Serializer>(value: &Value, s: S) -> Result<S::Ok, S::Error> {
    if let Some(obj) = value.as_object()
        && obj.get("type").and_then(Value::as_str) == Some("object")
        && !obj.contains_key("properties")
    {
        let mut patched = obj.clone();
        patched.insert("properties".to_string(), Value::Object(Default::default()));
        return Value::Object(patched).serialize(s);
    }
    value.serialize(s)
}

impl OpenAIResponsesScheme {
    /// `Request` から wire 形式の body を組み立てる。
    pub(crate) fn build_request(
        &self,
        model: &str,
        request: &Request,
        capability: &ModelCapability,
    ) -> ResponsesRequest {
        let input = convert_items_to_input(&request.items);
        let tools = request.tools.iter().map(convert_tool).collect();

        // Reasoning 投影: capability が Effort / Both をサポートし、かつ
        // request 側で effort が指定されているときだけ reasoning を付ける。
        let supports_effort = matches!(
            capability.reasoning,
            Some(ReasoningSupport::Effort | ReasoningSupport::Both),
        );
        let reasoning = request
            .config
            .reasoning
            .as_ref()
            .filter(|_| supports_effort)
            .map(|effort| ReasoningConfig {
                effort: match effort {
                    ReasoningControl::Effort(effort) => Some(effort.as_str().to_string()),
                    ReasoningControl::BudgetTokens(_) => None,
                },
                summary: "auto",
            })
            .filter(|reasoning| reasoning.effort.is_some());

        let include: Vec<&'static str> = if self.include_encrypted_content {
            vec!["reasoning.encrypted_content"]
        } else {
            Vec::new()
        };

        ResponsesRequest {
            model: model.to_string(),
            instructions: request.system_prompt.clone(),
            input,
            tools,
            tool_choice: "auto",
            parallel_tool_calls: true,
            reasoning,
            store: self.store,
            stream: true,
            include,
            max_output_tokens: if self.send_max_output_tokens {
                request.config.max_tokens
            } else {
                None
            },
            temperature: if self.send_sampling_params {
                request.config.temperature
            } else {
                None
            },
            top_p: if self.send_sampling_params {
                request.config.top_p
            } else {
                None
            },
            prompt_cache_key: request.cache_key.clone(),
        }
    }
}

/// `Item` 列を `input[]` に変換する。
fn convert_items_to_input(items: &[Item]) -> Vec<InputItem> {
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        match item {
            Item::Message { role, content, .. } => {
                let (role_str, text_variant): (&'static str, fn(String) -> InputContent) =
                    match role {
                        Role::User => ("user", |t| InputContent::InputText { text: t }),
                        Role::Assistant => ("assistant", |t| InputContent::OutputText { text: t }),
                        Role::System => ("developer", |t| InputContent::InputText { text: t }),
                    };
                let parts: Vec<InputContent> = content
                    .iter()
                    .map(|p| match p {
                        ContentPart::Text { text } => text_variant(text.clone()),
                        ContentPart::Refusal { refusal } => text_variant(refusal.clone()),
                    })
                    .collect();
                out.push(InputItem::Message {
                    role: role_str,
                    content: parts,
                });
            }
            Item::ToolCall {
                call_id,
                name,
                arguments,
                ..
            } => {
                // 非 object / 旧形式の "null" を "{}" に正規化。
                let normalized = parse_tool_arguments(arguments).to_string();
                out.push(InputItem::FunctionCall {
                    call_id: call_id.clone(),
                    name: name.clone(),
                    arguments: normalized,
                });
            }
            Item::ToolResult {
                call_id,
                summary,
                content,
                ..
            } => {
                let text = match content {
                    Some(c) => format!("{summary}\n{c}"),
                    None => summary.clone(),
                };
                out.push(InputItem::FunctionCallOutput {
                    call_id: call_id.clone(),
                    output: text,
                });
            }
            Item::Reasoning {
                id,
                text,
                summary,
                encrypted_content,
                ..
            } => {
                let summary_parts = summary
                    .iter()
                    .filter(|s| !s.is_empty())
                    .map(|s| ReasoningSummaryPart::SummaryText { text: s.clone() })
                    .collect();
                let content_parts = if text.is_empty() {
                    Vec::new()
                } else {
                    vec![ReasoningContentPart::ReasoningText { text: text.clone() }]
                };
                out.push(InputItem::Reasoning {
                    id: id.clone(),
                    summary: summary_parts,
                    content: content_parts,
                    encrypted_content: encrypted_content.clone(),
                });
            }
        }
    }
    out
}

fn convert_tool(tool: &ToolDefinition) -> ResponseTool {
    ResponseTool {
        r#type: "function",
        name: tool.name.clone(),
        description: tool.description.clone(),
        parameters: tool.input_schema.clone(),
        strict: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm_client::capability::{
        CacheStrategy, ModelCapability, ReasoningControl, ReasoningEffort, ReasoningSupport,
        StructuredOutput, ToolCallingSupport,
    };

    fn cap_with_reasoning() -> ModelCapability {
        ModelCapability {
            tool_calling: ToolCallingSupport::Parallel,
            structured_output: StructuredOutput::JsonSchema,
            reasoning: Some(ReasoningSupport::Effort),
            vision: true,
            prompt_caching: CacheStrategy::Auto,
        }
    }

    fn cap_no_reasoning() -> ModelCapability {
        ModelCapability {
            reasoning: None,
            ..cap_with_reasoning()
        }
    }

    #[test]
    fn scheme_defaults_to_stateless_zdr() {
        let s = OpenAIResponsesScheme::new();
        assert!(!s.store);
        assert!(s.include_encrypted_content);
    }

    #[test]
    fn includes_encrypted_content_when_enabled() {
        let scheme = OpenAIResponsesScheme::new();
        let req = Request::new().user("hi");
        let body = scheme.build_request("gpt-5", &req, &cap_with_reasoning());
        assert_eq!(body.include, vec!["reasoning.encrypted_content"]);
        assert!(!body.store);
        assert!(body.stream);
    }

    #[test]
    fn instructions_from_system_prompt() {
        let scheme = OpenAIResponsesScheme::new();
        let req = Request::new().system("be terse").user("hi");
        let body = scheme.build_request("gpt-5", &req, &cap_with_reasoning());
        assert_eq!(body.instructions.as_deref(), Some("be terse"));
        assert_eq!(body.input.len(), 1);
    }

    #[test]
    fn tool_choice_and_parallel_are_fixed() {
        let scheme = OpenAIResponsesScheme::new();
        let req = Request::new().user("hi");
        let body = scheme.build_request("gpt-5", &req, &cap_with_reasoning());
        assert_eq!(body.tool_choice, "auto");
        assert!(body.parallel_tool_calls);
    }

    #[test]
    fn user_message_uses_input_text() {
        let scheme = OpenAIResponsesScheme::new();
        let req = Request::new().user("hi");
        let body = scheme.build_request("gpt-5", &req, &cap_with_reasoning());
        match &body.input[0] {
            InputItem::Message { role, content } => {
                assert_eq!(*role, "user");
                assert_eq!(content.len(), 1);
                assert!(matches!(&content[0], InputContent::InputText { text } if text == "hi"));
            }
            _ => panic!("expected message"),
        }
    }

    #[test]
    fn system_role_item_is_projected_as_developer() {
        // ChatGPT backend (codex-oauth) は input[] の `role: "system"` を
        // "System messages are not allowed" で 400 拒否する。in-conversation
        // な system note (notify / fs_view auto-read / compaction summary) は
        // `role: "developer"` として投影し、両 backend で受理されるようにする。
        let scheme = OpenAIResponsesScheme::new();
        let req = Request::new()
            .user("hi")
            .item(Item::system_message("[notify] hello"));
        let body = scheme.build_request("gpt-5", &req, &cap_with_reasoning());
        match &body.input[1] {
            InputItem::Message { role, content } => {
                assert_eq!(*role, "developer");
                assert!(
                    matches!(&content[0], InputContent::InputText { text } if text == "[notify] hello"),
                );
            }
            _ => panic!("expected message"),
        }
    }

    #[test]
    fn assistant_message_uses_output_text() {
        let scheme = OpenAIResponsesScheme::new();
        let req = Request::new().user("hi").assistant("hello");
        let body = scheme.build_request("gpt-5", &req, &cap_with_reasoning());
        match &body.input[1] {
            InputItem::Message { role, content } => {
                assert_eq!(*role, "assistant");
                assert!(
                    matches!(&content[0], InputContent::OutputText { text } if text == "hello")
                );
            }
            _ => panic!("expected message"),
        }
    }

    #[test]
    fn tool_call_and_result_become_function_items() {
        let scheme = OpenAIResponsesScheme::new();
        let req = Request::new()
            .user("run")
            .item(Item::tool_call("c1", "t", r#"{"a":1}"#))
            .item(Item::tool_result("c1", "ok"));
        let body = scheme.build_request("gpt-5", &req, &cap_with_reasoning());
        assert!(matches!(body.input[1], InputItem::FunctionCall { .. }));
        assert!(matches!(
            body.input[2],
            InputItem::FunctionCallOutput { .. }
        ));
    }

    #[test]
    fn reasoning_item_round_trips_encrypted_content() {
        let scheme = OpenAIResponsesScheme::new();
        let item = Item::reasoning("inner")
            .with_reasoning_summary(vec!["s1".into()])
            .with_encrypted_content("ENC");
        let req = Request::new().user("hi").item(item);
        let body = scheme.build_request("gpt-5", &req, &cap_with_reasoning());
        match &body.input[1] {
            InputItem::Reasoning {
                summary,
                content,
                encrypted_content,
                ..
            } => {
                assert_eq!(summary.len(), 1);
                assert_eq!(content.len(), 1);
                assert_eq!(encrypted_content.as_deref(), Some("ENC"));
            }
            _ => panic!("expected reasoning"),
        }
    }

    #[test]
    fn reasoning_effort_projected_when_supported() {
        let scheme = OpenAIResponsesScheme::new();
        let mut req = Request::new().user("hi");
        req.config.reasoning = Some(ReasoningControl::Effort(ReasoningEffort::High));
        let body = scheme.build_request("gpt-5", &req, &cap_with_reasoning());
        let reasoning = body.reasoning.expect("reasoning should be set");
        assert_eq!(reasoning.effort.as_deref(), Some("high"));
        assert_eq!(reasoning.summary, "auto");
    }

    #[test]
    fn reasoning_omitted_when_unsupported() {
        let scheme = OpenAIResponsesScheme::new();
        let mut req = Request::new().user("hi");
        req.config.reasoning = Some(ReasoningControl::Effort(ReasoningEffort::High));
        let body = scheme.build_request("gpt-4o", &req, &cap_no_reasoning());
        assert!(body.reasoning.is_none());
    }

    #[test]
    fn max_output_tokens_passed_through_by_default() {
        let scheme = OpenAIResponsesScheme::new();
        let req = Request::new().user("hi").max_tokens(100);
        let body = scheme.build_request("gpt-5", &req, &cap_with_reasoning());
        assert_eq!(body.max_output_tokens, Some(100));
    }

    #[test]
    fn max_output_tokens_dropped_when_send_disabled() {
        let scheme = OpenAIResponsesScheme::new().with_send_max_output_tokens(false);
        let req = Request::new().user("hi").max_tokens(100);
        let body = scheme.build_request("gpt-5", &req, &cap_with_reasoning());
        assert_eq!(body.max_output_tokens, None);
        let json = serde_json::to_value(&body).unwrap();
        assert!(
            json.get("max_output_tokens").is_none(),
            "max_output_tokens key must not appear in serialised body, got: {json}"
        );
    }

    #[test]
    fn sampling_params_passed_through_by_default() {
        let scheme = OpenAIResponsesScheme::new();
        let req = Request::new().user("hi").temperature(0.4).top_p(0.9);
        let body = scheme.build_request("gpt-5", &req, &cap_with_reasoning());
        assert_eq!(body.temperature, Some(0.4));
        assert_eq!(body.top_p, Some(0.9));
    }

    #[test]
    fn sampling_params_dropped_when_send_disabled() {
        let scheme = OpenAIResponsesScheme::new().with_send_sampling_params(false);
        let req = Request::new().user("hi").temperature(0.4).top_p(0.9);
        let body = scheme.build_request("gpt-5", &req, &cap_with_reasoning());
        assert_eq!(body.temperature, None);
        assert_eq!(body.top_p, None);
        let json = serde_json::to_value(&body).unwrap();
        assert!(
            json.get("temperature").is_none() && json.get("top_p").is_none(),
            "temperature/top_p keys must not appear in serialised body, got: {json}"
        );
    }

    #[test]
    fn prompt_cache_key_passed_through_when_set() {
        let scheme = OpenAIResponsesScheme::new();
        let req = Request::new().user("hi").cache_key("session-abc");
        let body = scheme.build_request("gpt-5", &req, &cap_with_reasoning());
        assert_eq!(body.prompt_cache_key.as_deref(), Some("session-abc"));
        let json = serde_json::to_value(&body).unwrap();
        assert_eq!(json["prompt_cache_key"], "session-abc");
    }

    #[test]
    fn prompt_cache_key_omitted_when_none() {
        let scheme = OpenAIResponsesScheme::new();
        let req = Request::new().user("hi");
        let body = scheme.build_request("gpt-5", &req, &cap_with_reasoning());
        assert!(body.prompt_cache_key.is_none());
        let json = serde_json::to_value(&body).unwrap();
        assert!(
            json.get("prompt_cache_key").is_none(),
            "prompt_cache_key key must not appear in serialised body, got: {json}"
        );
    }

    #[test]
    fn tool_schema_without_properties_is_normalized() {
        // schemars は引数なし struct から `type:"object"` だけのスキーマを
        // 吐く。OpenAI Responses は `properties` 欠落を 400 で拒否するので
        // 送る直前に空オブジェクトを補うのを確認。
        let scheme = OpenAIResponsesScheme::new();
        let raw_schema = serde_json::json!({ "type": "object" });
        let req = Request::new().tool(
            ToolDefinition::new("empty")
                .description("no args")
                .input_schema(raw_schema),
        );
        let body = scheme.build_request("gpt-5", &req, &cap_with_reasoning());
        let json = serde_json::to_value(&body).unwrap();
        assert_eq!(json["tools"][0]["parameters"]["type"], "object");
        assert!(
            json["tools"][0]["parameters"]["properties"].is_object(),
            "properties must be present as an object, got: {}",
            json["tools"][0]["parameters"]
        );
    }

    #[test]
    fn tool_schema_with_properties_is_untouched() {
        let scheme = OpenAIResponsesScheme::new();
        let raw_schema = serde_json::json!({
            "type": "object",
            "properties": { "path": { "type": "string" } },
            "required": ["path"]
        });
        let req = Request::new().tool(
            ToolDefinition::new("t")
                .description("d")
                .input_schema(raw_schema.clone()),
        );
        let body = scheme.build_request("gpt-5", &req, &cap_with_reasoning());
        let json = serde_json::to_value(&body).unwrap();
        assert_eq!(json["tools"][0]["parameters"], raw_schema);
    }

    #[test]
    fn serialized_body_has_expected_shape() {
        // wire 形式が崩れていないかのスモークテスト
        let scheme = OpenAIResponsesScheme::new();
        let req = Request::new()
            .system("sys")
            .user("hi")
            .tool(ToolDefinition::new("t").description("d"));
        let body = scheme.build_request("gpt-5", &req, &cap_with_reasoning());
        let json = serde_json::to_value(&body).unwrap();
        assert_eq!(json["model"], "gpt-5");
        assert_eq!(json["instructions"], "sys");
        assert_eq!(json["tool_choice"], "auto");
        assert_eq!(json["parallel_tool_calls"], true);
        assert_eq!(json["store"], false);
        assert_eq!(json["stream"], true);
        assert_eq!(json["include"][0], "reasoning.encrypted_content");
        assert_eq!(json["tools"][0]["type"], "function");
        assert_eq!(json["tools"][0]["name"], "t");
    }
}
