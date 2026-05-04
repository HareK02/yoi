//! Anthropic Request Builder
//!
//! Converts Open Responses native Item model to Anthropic Messages API format.

use std::collections::BTreeSet;

use serde::Serialize;

use crate::llm_client::{
    Request,
    capability::{CacheStrategy, ModelCapability, ReasoningControl, ReasoningSupport},
    types::{ContentPart, Item, Role, ToolDefinition, parse_tool_arguments},
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<AnthropicThinking>,
}

/// Anthropic extended thinking 指示。
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum AnthropicThinking {
    Enabled { budget_tokens: i32 },
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

/// `cache_control` marker attached to a content part, signalling that the
/// prefix up to and including this part should be cached.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum CacheControl {
    Ephemeral,
}

/// Anthropic content part
#[derive(Debug, Serialize)]
#[serde(tag = "type")]
pub(crate) enum AnthropicContentPart {
    #[serde(rename = "text")]
    Text {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
    #[serde(rename = "thinking")]
    Thinking {
        thinking: String,
        signature: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
    #[serde(rename = "redacted_thinking")]
    RedactedThinking {
        /// 暗号化済み reasoning blob。`Item::Reasoning::encrypted_content`
        /// から渡る。
        data: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
}

impl AnthropicContentPart {
    fn text(text: String) -> Self {
        Self::Text {
            text,
            cache_control: None,
        }
    }

    fn thinking(thinking: String, signature: String) -> Self {
        Self::Thinking {
            thinking,
            signature,
            cache_control: None,
        }
    }

    fn redacted_thinking(data: String) -> Self {
        Self::RedactedThinking {
            data,
            cache_control: None,
        }
    }

    fn tool_use(id: String, name: String, input: serde_json::Value) -> Self {
        Self::ToolUse {
            id,
            name,
            input,
            cache_control: None,
        }
    }

    fn tool_result(tool_use_id: String, content: String) -> Self {
        Self::ToolResult {
            tool_use_id,
            content,
            cache_control: None,
        }
    }

    fn set_cache_control(&mut self, cc: CacheControl) {
        match self {
            Self::Text { cache_control, .. }
            | Self::Thinking { cache_control, .. }
            | Self::RedactedThinking { cache_control, .. }
            | Self::ToolUse { cache_control, .. }
            | Self::ToolResult { cache_control, .. } => {
                *cache_control = Some(cc);
            }
        }
    }
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
    /// Build Anthropic request from Request.
    ///
    /// `capability.prompt_caching` が [`CacheStrategy::Auto`] のときは
    /// `cache_control` マーカーを一切挿入しない（Ollama の `/v1/messages`
    /// 流用時など、サーバ側が `cache_control` を受け付けないケース）。
    pub(crate) fn build_request(
        &self,
        model: &str,
        request: &Request,
        capability: &ModelCapability,
    ) -> AnthropicRequest {
        let breakpoints = if matches!(capability.prompt_caching, CacheStrategy::Explicit { .. }) {
            compute_breakpoints(&request.items, request.cache_anchor)
        } else {
            BTreeSet::new()
        };
        let messages = self.convert_items_to_messages(&request.items, &breakpoints);
        let tools = request.tools.iter().map(|t| self.convert_tool(t)).collect();

        // Reasoning の投影: capability が BudgetTokens / Both をサポート
        // していて、request 側で budget_tokens が指定されているときだけ
        // thinking フィールドを付ける。
        let supports_budget_tokens = matches!(
            capability.reasoning,
            Some(ReasoningSupport::BudgetTokens | ReasoningSupport::Both),
        );
        let thinking = request
            .config
            .reasoning
            .as_ref()
            .filter(|_| supports_budget_tokens)
            .and_then(|rc| match rc {
                ReasoningControl::BudgetTokens(budget_tokens) => Some(AnthropicThinking::Enabled {
                    budget_tokens: *budget_tokens,
                }),
                ReasoningControl::Effort(_) => None,
            });

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
            thinking,
        }
    }

    /// Convert Open Responses Items to Anthropic Messages and attach
    /// `cache_control` markers at each breakpoint item's last content
    /// part.
    ///
    /// Anthropic uses a message-based model where:
    /// - User messages have role "user"
    /// - Assistant messages have role "assistant"
    /// - Tool calls are content parts within assistant messages
    /// - Tool results are content parts within user messages
    ///
    /// Each non-`Message` item produces exactly one content part, so
    /// "last part for the item" is always well-defined. For breakpoint
    /// `Message` items the output is forced into the array form so a
    /// marker has a part to attach to.
    fn convert_items_to_messages(
        &self,
        items: &[Item],
        breakpoints: &BTreeSet<usize>,
    ) -> Vec<AnthropicMessage> {
        let mut messages = Vec::new();
        // Pending parts carry their origin item index so we can record
        // (msg_idx, part_idx) when we flush them into a message.
        let mut pending_assistant: Vec<(usize, AnthropicContentPart)> = Vec::new();
        let mut pending_user: Vec<(usize, AnthropicContentPart)> = Vec::new();
        let mut locations: Vec<Option<(usize, usize)>> = vec![None; items.len()];

        for (i, item) in items.iter().enumerate() {
            match item {
                Item::Message { role, content, .. } => {
                    flush_pending(
                        &mut messages,
                        &mut pending_assistant,
                        "assistant",
                        &mut locations,
                    );
                    flush_pending(&mut messages, &mut pending_user, "user", &mut locations);

                    let anthropic_role = match role {
                        Role::User | Role::System => "user",
                        Role::Assistant => "assistant",
                    };

                    let parts: Vec<AnthropicContentPart> = content
                        .iter()
                        .map(|p| match p {
                            ContentPart::Text { text } => AnthropicContentPart::text(text.clone()),
                            ContentPart::Refusal { refusal } => {
                                AnthropicContentPart::text(refusal.clone())
                            }
                        })
                        .collect();

                    let force_parts = breakpoints.contains(&i);
                    let msg_idx = messages.len();

                    // Preserve the single-text shorthand unless a
                    // breakpoint needs a concrete part to live on.
                    if parts.len() == 1 && !force_parts {
                        if let AnthropicContentPart::Text { text, .. } = &parts[0] {
                            messages.push(AnthropicMessage {
                                role: anthropic_role.to_string(),
                                content: AnthropicContent::Text(text.clone()),
                            });
                            continue;
                        }
                    }

                    let last_part_idx = parts.len().saturating_sub(1);
                    messages.push(AnthropicMessage {
                        role: anthropic_role.to_string(),
                        content: AnthropicContent::Parts(parts),
                    });
                    locations[i] = Some((msg_idx, last_part_idx));
                }

                Item::ToolCall {
                    call_id,
                    name,
                    arguments,
                    ..
                } => {
                    flush_pending(&mut messages, &mut pending_user, "user", &mut locations);
                    // Parse arguments JSON string to Value (defensive:
                    // normalize non-object / legacy "null" payloads to
                    // `{}` so the Anthropic API accepts it).
                    let input = parse_tool_arguments(arguments);
                    pending_assistant.push((
                        i,
                        AnthropicContentPart::tool_use(call_id.clone(), name.clone(), input),
                    ));
                }

                Item::ToolResult {
                    call_id,
                    summary,
                    content,
                    ..
                } => {
                    flush_pending(
                        &mut messages,
                        &mut pending_assistant,
                        "assistant",
                        &mut locations,
                    );
                    let text = match content {
                        Some(c) => format!("{summary}\n{c}"),
                        None => summary.clone(),
                    };
                    pending_user
                        .push((i, AnthropicContentPart::tool_result(call_id.clone(), text)));
                }

                Item::Reasoning {
                    text,
                    encrypted_content,
                    signature,
                    ..
                } => {
                    flush_pending(&mut messages, &mut pending_user, "user", &mut locations);
                    // Anthropic はアシスタントターン中の `thinking` /
                    // `redacted_thinking` ブロックを必ず assistant role の
                    // content_part として送り返す必要がある。
                    //
                    // - signature あり: `thinking` content_part を投影
                    // - signature 無し + encrypted_content あり:
                    //   `redacted_thinking` content_part を投影
                    // - どちらも無い: 他 scheme（OpenAI 等）から流入した
                    //   素の reasoning text。Anthropic に投げる意味も
                    //   round-trip の根拠も無いので drop。
                    if let Some(sig) = signature.clone() {
                        pending_assistant.push((
                            i,
                            AnthropicContentPart::thinking(text.clone(), sig),
                        ));
                    } else if let Some(data) = encrypted_content.clone() {
                        pending_assistant
                            .push((i, AnthropicContentPart::redacted_thinking(data)));
                    }
                    // どちらも None なら何も pend せず、本 item は無視。
                }
            }
        }

        flush_pending(
            &mut messages,
            &mut pending_assistant,
            "assistant",
            &mut locations,
        );
        flush_pending(&mut messages, &mut pending_user, "user", &mut locations);

        // Apply cache_control markers at each breakpoint item's last part.
        for &bp in breakpoints {
            let Some((msg_idx, part_idx)) = locations.get(bp).copied().flatten() else {
                continue;
            };
            if let AnthropicContent::Parts(parts) = &mut messages[msg_idx].content {
                if let Some(part) = parts.get_mut(part_idx) {
                    part.set_cache_control(CacheControl::Ephemeral);
                }
            }
        }

        messages
    }

    fn convert_tool(&self, tool: &ToolDefinition) -> AnthropicTool {
        AnthropicTool {
            name: tool.name.clone(),
            description: tool.description.clone(),
            input_schema: tool.input_schema.clone(),
        }
    }
}

/// Flush a pending parts buffer into a new message, recording the
/// emitted `(msg_idx, part_idx)` for each originating item.
fn flush_pending(
    messages: &mut Vec<AnthropicMessage>,
    pending: &mut Vec<(usize, AnthropicContentPart)>,
    role: &str,
    locations: &mut [Option<(usize, usize)>],
) {
    if pending.is_empty() {
        return;
    }
    let msg_idx = messages.len();
    let taken: Vec<(usize, AnthropicContentPart)> = std::mem::take(pending);
    let mut parts = Vec::with_capacity(taken.len());
    for (part_idx, (origin, part)) in taken.into_iter().enumerate() {
        locations[origin] = Some((msg_idx, part_idx));
        parts.push(part);
    }
    messages.push(AnthropicMessage {
        role: role.to_string(),
        content: AnthropicContent::Parts(parts),
    });
}

/// Compute the set of item indices that should receive a cache_control
/// breakpoint:
///
/// 1. `cache_anchor` (stable prefix boundary — typically a post-compact
///    summary at index 0).
/// 2. The item immediately preceding the most recent `Role::User`
///    message (i.e. the end of the previous turn). This is the rewind
///    boundary: if a user re-issues the turn from scratch, the cache up
///    to here remains valid.
/// 3. The final item (head of the outgoing request, for the next
///    request in the same turn to read from).
///
/// Overlapping positions are collapsed to a single breakpoint.
fn compute_breakpoints(items: &[Item], cache_anchor: Option<usize>) -> BTreeSet<usize> {
    let mut bps = BTreeSet::new();
    if items.is_empty() {
        return bps;
    }
    if let Some(anchor) = cache_anchor {
        if anchor < items.len() {
            bps.insert(anchor);
        }
    }
    let last_user = items.iter().rposition(|it| {
        matches!(
            it,
            Item::Message {
                role: Role::User,
                ..
            }
        )
    });
    if let Some(i) = last_user {
        if i > 0 {
            bps.insert(i - 1);
        }
    }
    bps.insert(items.len() - 1);
    bps
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm_client::capability::{
        CacheStrategy, ReasoningEffort, StructuredOutput, ToolCallingSupport,
    };

    /// cache_control が有効になる既定の capability。
    fn cap_explicit() -> ModelCapability {
        ModelCapability {
            tool_calling: ToolCallingSupport::Parallel,
            structured_output: StructuredOutput::JsonSchema,
            reasoning: None,
            vision: false,
            prompt_caching: CacheStrategy::Explicit { max_breakpoints: 4 },
        }
    }

    /// cache_control を送らない capability（Ollama 等）。
    fn cap_auto() -> ModelCapability {
        ModelCapability {
            prompt_caching: CacheStrategy::Auto,
            ..cap_explicit()
        }
    }

    fn cap_budget_reasoning() -> ModelCapability {
        ModelCapability {
            reasoning: Some(ReasoningSupport::BudgetTokens),
            ..cap_explicit()
        }
    }

    #[test]
    fn test_build_simple_request() {
        let scheme = AnthropicScheme::new();
        let request = Request::new()
            .system("You are a helpful assistant.")
            .user("Hello!");

        let anthropic_req =
            scheme.build_request("claude-sonnet-4-20250514", &request, &cap_explicit());

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

        let anthropic_req =
            scheme.build_request("claude-sonnet-4-20250514", &request, &cap_explicit());

        assert_eq!(anthropic_req.tools.len(), 1);
        assert_eq!(anthropic_req.tools[0].name, "get_weather");
    }

    #[test]
    fn thinking_budget_projected_when_supported() {
        let scheme = AnthropicScheme::new();
        let mut request = Request::new().user("think");
        request.config.reasoning = Some(ReasoningControl::BudgetTokens(4096));

        let req = scheme.build_request(
            "claude-sonnet-4-20250514",
            &request,
            &cap_budget_reasoning(),
        );
        let json = serde_json::to_value(&req).unwrap();

        assert_eq!(json["thinking"]["type"], "enabled");
        assert_eq!(json["thinking"]["budget_tokens"], 4096);
    }

    #[test]
    fn effort_reasoning_not_projected_to_anthropic() {
        let scheme = AnthropicScheme::new();
        let mut request = Request::new().user("think");
        request.config.reasoning = Some(ReasoningControl::Effort(ReasoningEffort::High));

        let req = scheme.build_request(
            "claude-sonnet-4-20250514",
            &request,
            &cap_budget_reasoning(),
        );

        assert!(req.thinking.is_none());
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

        let anthropic_req =
            scheme.build_request("claude-sonnet-4-20250514", &request, &cap_explicit());

        assert_eq!(anthropic_req.messages.len(), 3);
        assert_eq!(anthropic_req.messages[0].role, "user");
        assert_eq!(anthropic_req.messages[1].role, "assistant");
        assert_eq!(anthropic_req.messages[2].role, "user");
    }

    /// Pull out the `cache_control` field from a part regardless of variant.
    fn part_cache_control(part: &AnthropicContentPart) -> Option<CacheControl> {
        match part {
            AnthropicContentPart::Text { cache_control, .. }
            | AnthropicContentPart::Thinking { cache_control, .. }
            | AnthropicContentPart::RedactedThinking { cache_control, .. }
            | AnthropicContentPart::ToolUse { cache_control, .. }
            | AnthropicContentPart::ToolResult { cache_control, .. } => *cache_control,
        }
    }

    /// All `(msg_idx, part_idx, cache_control)` triples whose `cache_control`
    /// is set, in iteration order over the output.
    fn breakpoint_positions(req: &AnthropicRequest) -> Vec<(usize, usize, CacheControl)> {
        let mut out = Vec::new();
        for (mi, msg) in req.messages.iter().enumerate() {
            if let AnthropicContent::Parts(parts) = &msg.content {
                for (pi, part) in parts.iter().enumerate() {
                    if let Some(cc) = part_cache_control(part) {
                        out.push((mi, pi, cc));
                    }
                }
            }
        }
        out
    }

    /// Convenience: a turn that ends with one assistant text, one tool
    /// call/result pair, and a final assistant text. Produced at
    /// `history[head..]` indices shown alongside, so tests can reason
    /// about breakpoint positions.
    fn completed_turn() -> Vec<Item> {
        vec![
            Item::user_message("hello"),           // 0
            Item::assistant_message("hi"),         // 1
            Item::tool_call("c1", "tool_a", "{}"), // 2
            Item::tool_result("c1", "ok"),         // 3
            Item::assistant_message("done"),       // 4
        ]
    }

    #[test]
    fn three_breakpoints_when_compact_plus_prior_turn() {
        let scheme = AnthropicScheme::new();
        let mut items = vec![Item::system_message("[Compacted context summary]\n\nprior")];
        items.extend(completed_turn()); // now indices 1..=5
        items.push(Item::user_message("next turn"));
        // anchor=0, last user idx=6 → turn_end=5, head=6.
        let mut request = Request::new().items(items);
        request.cache_anchor = Some(0);

        let req = scheme.build_request("claude-sonnet-4-20250514", &request, &cap_explicit());
        let bps = breakpoint_positions(&req);
        assert_eq!(bps.len(), 3, "expected 3 breakpoints, got {:?}", bps);
        for (_, _, cc) in bps {
            assert_eq!(cc, CacheControl::Ephemeral);
        }
    }

    #[test]
    fn two_breakpoints_without_compaction() {
        let scheme = AnthropicScheme::new();
        let mut items = completed_turn();
        items.push(Item::user_message("next turn")); // index 5 = latest user
        // cache_anchor=None, turn_end=4, head=5.
        let request = Request::new().items(items);

        let req = scheme.build_request("claude-sonnet-4-20250514", &request, &cap_explicit());
        let bps = breakpoint_positions(&req);
        assert_eq!(bps.len(), 2, "expected 2 breakpoints, got {:?}", bps);
    }

    #[test]
    fn single_breakpoint_when_only_first_user_message() {
        let scheme = AnthropicScheme::new();
        let request = Request::new().user("first ever turn");
        // latest user at 0 → no turn_end; head=0; no anchor. Collapse → 1.
        let req = scheme.build_request("claude-sonnet-4-20250514", &request, &cap_explicit());
        let bps = breakpoint_positions(&req);
        assert_eq!(bps.len(), 1, "expected 1 breakpoint, got {:?}", bps);
    }

    #[test]
    fn overlap_collapses_anchor_and_turn_end() {
        // items = [compact_summary(0, Role::System), user(1)]
        // anchor=0, latest user=1 → turn_end=0, head=1. Anchor∩turn_end at 0.
        let scheme = AnthropicScheme::new();
        let mut request = Request::new().items(vec![
            Item::system_message("[Compacted context summary]\n\nprior"),
            Item::user_message("fresh user"),
        ]);
        request.cache_anchor = Some(0);

        let req = scheme.build_request("claude-sonnet-4-20250514", &request, &cap_explicit());
        let bps = breakpoint_positions(&req);
        assert_eq!(bps.len(), 2, "expected collapse to 2, got {:?}", bps);
    }

    #[test]
    fn breakpoint_on_tool_result_head() {
        // Mid-turn second call: items end with a tool_result. Head must
        // land on the ToolResult part.
        let scheme = AnthropicScheme::new();
        let request = Request::new()
            .user("run it")
            .item(Item::tool_call("c1", "t", "{}"))
            .item(Item::tool_result("c1", "result"));
        let req = scheme.build_request("claude-sonnet-4-20250514", &request, &cap_explicit());
        let bps = breakpoint_positions(&req);
        assert_eq!(bps.len(), 1);
        let (mi, pi, _) = bps[0];
        let part = match &req.messages[mi].content {
            AnthropicContent::Parts(parts) => &parts[pi],
            _ => panic!("expected Parts for breakpoint-bearing message"),
        };
        assert!(
            matches!(part, AnthropicContentPart::ToolResult { .. }),
            "expected ToolResult, got {:?}",
            part,
        );
    }

    #[test]
    fn single_text_message_uses_text_shorthand_without_breakpoint() {
        // Non-breakpoint single-text messages stay on the text shorthand
        // so we don't bloat requests with wrapper arrays. Here the Head
        // lands on items[1], leaving items[0] without a marker.
        let scheme = AnthropicScheme::new();
        let request = Request::new().user("hello").assistant("hi there");
        let req = scheme.build_request("claude-sonnet-4-20250514", &request, &cap_explicit());
        assert!(
            matches!(req.messages[0].content, AnthropicContent::Text(_)),
            "non-breakpoint single-text message should use text shorthand",
        );
    }

    #[test]
    fn single_text_message_is_forced_to_parts_when_breakpoint() {
        // A breakpoint on a single-text message must be emitted in the
        // array form so cache_control has a part to attach to.
        let scheme = AnthropicScheme::new();
        let mut request = Request::new().user("hello");
        request.cache_anchor = Some(0);
        let req = scheme.build_request("claude-sonnet-4-20250514", &request, &cap_explicit());
        match &req.messages[0].content {
            AnthropicContent::Parts(parts) => {
                assert_eq!(parts.len(), 1);
                assert_eq!(part_cache_control(&parts[0]), Some(CacheControl::Ephemeral));
            }
            AnthropicContent::Text(_) => panic!("breakpoint item should use Parts form"),
        }
    }

    #[test]
    fn serialized_json_shape_matches_anthropic_spec() {
        // Single-sentence smoke test against the exact JSON key shape
        // Anthropic expects for cache_control.
        let scheme = AnthropicScheme::new();
        let mut request = Request::new().user("hello");
        request.cache_anchor = Some(0);
        let req = scheme.build_request("claude-sonnet-4-20250514", &request, &cap_explicit());
        let json = serde_json::to_value(&req).unwrap();
        let part = &json["messages"][0]["content"][0];
        assert_eq!(part["type"], "text");
        assert_eq!(part["text"], "hello");
        assert_eq!(part["cache_control"]["type"], "ephemeral");
    }

    #[test]
    fn cache_anchor_out_of_range_is_ignored() {
        // Defensive: if a caller passes a stale anchor beyond items.len(),
        // we drop it silently rather than panicking.
        let scheme = AnthropicScheme::new();
        let mut request = Request::new().user("one");
        request.cache_anchor = Some(99);
        let req = scheme.build_request("claude-sonnet-4-20250514", &request, &cap_explicit());
        // Only the Head breakpoint survives.
        let bps = breakpoint_positions(&req);
        assert_eq!(bps.len(), 1);
    }

    #[test]
    fn empty_items_produce_no_breakpoints() {
        let scheme = AnthropicScheme::new();
        let req =
            scheme.build_request("claude-sonnet-4-20250514", &Request::new(), &cap_explicit());
        assert!(req.messages.is_empty());
        assert!(breakpoint_positions(&req).is_empty());
    }

    #[test]
    fn cache_auto_does_not_add_cache_control() {
        // Ollama のように `CacheStrategy::Auto` のときは cache_control
        // マーカーを一切付けない。breakpoint 計算も走らないこと。
        let scheme = AnthropicScheme::new();
        let mut request = Request::new().user("hello");
        request.cache_anchor = Some(0);
        let req = scheme.build_request("claude-sonnet-4-20250514", &request, &cap_auto());
        assert!(breakpoint_positions(&req).is_empty());
    }

    fn collect_assistant_thinking_parts(req: &AnthropicRequest) -> Vec<&AnthropicContentPart> {
        let mut out = Vec::new();
        for msg in &req.messages {
            if msg.role != "assistant" {
                continue;
            }
            if let AnthropicContent::Parts(parts) = &msg.content {
                for part in parts {
                    if matches!(
                        part,
                        AnthropicContentPart::Thinking { .. }
                            | AnthropicContentPart::RedactedThinking { .. }
                    ) {
                        out.push(part);
                    }
                }
            }
        }
        out
    }

    #[test]
    fn reasoning_with_signature_projects_thinking_part() {
        // Item::Reasoning に signature があれば assistant role の
        // `thinking` content_part として送る。
        let scheme = AnthropicScheme::new();
        let request = Request::new()
            .user("hi")
            .item(Item::reasoning("step-by-step").with_signature("SIG-A"))
            .item(Item::assistant_message("done"));
        let req = scheme.build_request("claude-sonnet-4-20250514", &request, &cap_explicit());
        let thinking_parts = collect_assistant_thinking_parts(&req);
        assert_eq!(thinking_parts.len(), 1);
        match thinking_parts[0] {
            AnthropicContentPart::Thinking {
                thinking, signature, ..
            } => {
                assert_eq!(thinking, "step-by-step");
                assert_eq!(signature, "SIG-A");
            }
            other => panic!("expected Thinking part, got {other:?}"),
        }
    }

    #[test]
    fn reasoning_with_only_encrypted_content_projects_redacted_thinking() {
        let scheme = AnthropicScheme::new();
        let request = Request::new()
            .user("hi")
            .item(Item::reasoning("").with_encrypted_content("opaque"))
            .item(Item::assistant_message("done"));
        let req = scheme.build_request("claude-sonnet-4-20250514", &request, &cap_explicit());
        let parts = collect_assistant_thinking_parts(&req);
        assert_eq!(parts.len(), 1);
        match parts[0] {
            AnthropicContentPart::RedactedThinking { data, .. } => {
                assert_eq!(data, "opaque");
            }
            other => panic!("expected RedactedThinking, got {other:?}"),
        }
    }

    #[test]
    fn reasoning_without_signature_or_encrypted_is_dropped() {
        // 他 scheme から流入した素の reasoning は Anthropic に投げない。
        let scheme = AnthropicScheme::new();
        let request = Request::new()
            .user("hi")
            .item(Item::reasoning("plain text"))
            .item(Item::assistant_message("done"));
        let req = scheme.build_request("claude-sonnet-4-20250514", &request, &cap_explicit());
        // thinking part は 1 つも乗らない
        assert!(collect_assistant_thinking_parts(&req).is_empty());
    }

    #[test]
    fn tool_definitions_carry_no_cache_control() {
        // Tool JSON schema must serialise unchanged — no sneak-in of
        // cache_control at the tools-array level.
        let scheme = AnthropicScheme::new();
        let request = Request::new()
            .user("hello")
            .tool(ToolDefinition::new("noop").input_schema(serde_json::json!({
                "type": "object",
                "properties": {}
            })));
        let req = scheme.build_request("claude-sonnet-4-20250514", &request, &cap_explicit());
        let json = serde_json::to_value(&req).unwrap();
        let tool = &json["tools"][0];
        assert!(tool.get("cache_control").is_none());
    }
}
