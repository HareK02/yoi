//! Gemini SSEイベントパース
//!
//! Google Gemini APIのSSEイベントをパースし、統一Event型に変換

use crate::llm_client::{
    ClientError,
    event::{BlockMetadata, BlockStart, BlockStop, BlockType, Event, StopReason, UsageEvent},
};
use serde::Deserialize;

use super::GeminiScheme;

// ============================================================================
// SSEイベントのJSON構造
// ============================================================================

/// Gemini GenerateContentResponse (ストリーミングチャンク)
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GenerateContentResponse {
    /// 候補
    pub candidates: Option<Vec<Candidate>>,
    /// 使用量メタデータ
    pub usage_metadata: Option<UsageMetadata>,
    /// プロンプトフィードバック
    pub prompt_feedback: Option<PromptFeedback>,
    /// モデルバージョン
    pub model_version: Option<String>,
}

/// 候補
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Candidate {
    /// コンテンツ
    pub content: Option<CandidateContent>,
    /// 完了理由
    pub finish_reason: Option<String>,
    /// インデックス
    pub index: Option<usize>,
    /// 安全性評価
    pub safety_ratings: Option<Vec<SafetyRating>>,
}

/// 候補コンテンツ
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub(crate) struct CandidateContent {
    /// パーツ
    pub parts: Option<Vec<CandidatePart>>,
    /// ロール
    pub role: Option<String>,
}

/// 候補パーツ
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CandidatePart {
    /// テキスト
    pub text: Option<String>,
    /// 関数呼び出し
    pub function_call: Option<FunctionCall>,
}

/// 関数呼び出し
#[derive(Debug, Deserialize)]
pub(crate) struct FunctionCall {
    /// 関数名
    pub name: String,
    /// 引数
    pub args: Option<serde_json::Value>,
}

/// 使用量メタデータ
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UsageMetadata {
    /// プロンプトトークン数
    pub prompt_token_count: Option<u64>,
    /// 候補トークン数
    pub candidates_token_count: Option<u64>,
    /// 合計トークン数
    pub total_token_count: Option<u64>,
}

/// プロンプトフィードバック
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PromptFeedback {
    /// ブロック理由
    pub block_reason: Option<String>,
    /// 安全性評価
    pub safety_ratings: Option<Vec<SafetyRating>>,
}

/// 安全性評価
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub(crate) struct SafetyRating {
    /// カテゴリ
    pub category: Option<String>,
    /// 確率
    pub probability: Option<String>,
}

// ============================================================================
// イベント変換
// ============================================================================

impl GeminiScheme {
    /// SSEデータをEvent型に変換
    ///
    /// # Arguments
    /// * `data` - SSEイベントデータJSON文字列
    ///
    /// # Returns
    /// * `Ok(Some(Vec<Event>))` - 変換成功
    /// * `Ok(None)` - イベントを無視
    /// * `Err(ClientError)` - パースエラー
    pub(crate) fn parse_event(&self, data: &str) -> Result<Option<Vec<Event>>, ClientError> {
        // データが空または無効な場合はスキップ
        if data.is_empty() || data == "[DONE]" {
            return Ok(None);
        }

        let response: GenerateContentResponse =
            serde_json::from_str(data).map_err(|e| ClientError::Api {
                status: None,
                code: Some("parse_error".to_string()),
                message: format!("Failed to parse Gemini SSE data: {} -> {}", e, data),
            })?;

        let mut events = Vec::new();

        // 使用量メタデータ
        if let Some(usage) = response.usage_metadata {
            events.push(self.convert_usage(&usage));
        }

        // 候補を処理
        if let Some(candidates) = response.candidates {
            for candidate in candidates {
                let candidate_index = candidate.index.unwrap_or(0);

                if let Some(content) = candidate.content {
                    if let Some(parts) = content.parts {
                        for (part_index, part) in parts.iter().enumerate() {
                            // テキストデルタ
                            if let Some(text) = &part.text {
                                if !text.is_empty() {
                                    // Geminiは明示的なBlockStartを送らないため、
                                    // TextDeltaを直接送る（Timelineが暗黙的に開始を処理）
                                    events.push(Event::text_delta(part_index, text.clone()));
                                }
                            }

                            // 関数呼び出し
                            if let Some(function_call) = &part.function_call {
                                // 関数呼び出しの開始
                                // Geminiでは関数呼び出しは一度に送られることが多い
                                // ストリーミング引数が有効な場合は部分的に送られる可能性がある

                                // 関数呼び出しIDはGeminiにはないので、名前をIDとして使用
                                let function_id = format!("call_{}", function_call.name);

                                events.push(Event::BlockStart(BlockStart {
                                    index: candidate_index * 10 + part_index, // 複合インデックス
                                    block_type: BlockType::ToolUse,
                                    metadata: BlockMetadata::ToolUse {
                                        id: function_id,
                                        name: function_call.name.clone(),
                                    },
                                }));

                                // 引数がある場合はデルタとして送る
                                if let Some(args) = &function_call.args {
                                    let args_str = serde_json::to_string(args).unwrap_or_default();
                                    if !args_str.is_empty() && args_str != "null" {
                                        events.push(Event::tool_input_delta(
                                            candidate_index * 10 + part_index,
                                            args_str,
                                        ));
                                    }
                                }
                            }
                        }
                    }
                }

                // 完了理由
                if let Some(finish_reason) = candidate.finish_reason {
                    let stop_reason = match finish_reason.as_str() {
                        "STOP" => Some(StopReason::EndTurn),
                        "MAX_TOKENS" => Some(StopReason::MaxTokens),
                        "SAFETY" | "RECITATION" | "OTHER" => Some(StopReason::EndTurn),
                        _ => None,
                    };

                    // テキストブロックの停止
                    events.push(Event::BlockStop(BlockStop {
                        index: candidate_index,
                        block_type: BlockType::Text,
                        stop_reason,
                    }));
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

        let events = scheme.parse_event(data).unwrap().unwrap();
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

        let events = scheme.parse_event(data).unwrap().unwrap();

        // Usageイベントが含まれるはず
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

        let events = scheme.parse_event(data).unwrap().unwrap();

        // BlockStartイベントがあるはず
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

        // 引数デルタもあるはず
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

        let events = scheme.parse_event(data).unwrap().unwrap();

        // BlockStopイベントがあるはず
        let stop_event = events.iter().find(|e| matches!(e, Event::BlockStop(_)));
        assert!(stop_event.is_some());

        if let Event::BlockStop(stop) = stop_event.unwrap() {
            assert_eq!(stop.stop_reason, Some(StopReason::EndTurn));
        }
    }

    #[test]
    fn test_parse_empty_data() {
        let scheme = GeminiScheme::new();
        assert!(scheme.parse_event("").unwrap().is_none());
        assert!(scheme.parse_event("[DONE]").unwrap().is_none());
    }
}
