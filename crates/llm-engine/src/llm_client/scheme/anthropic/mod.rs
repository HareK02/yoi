//! Anthropic Messages API スキーマ
//!
//! - リクエストJSON生成
//! - SSEイベントパース → Event変換

mod capability;
mod events;
mod request;
mod scheme_impl;

pub use scheme_impl::AnthropicState;

/// Anthropicスキーマ
///
/// Anthropic Messages APIのリクエスト/レスポンス変換を担当
#[derive(Debug, Clone)]
pub struct AnthropicScheme {
    /// APIバージョン
    pub api_version: String,
    /// 細粒度ツールストリーミングを有効にするか
    pub fine_grained_tool_streaming: bool,
}

impl Default for AnthropicScheme {
    fn default() -> Self {
        Self {
            api_version: "2023-06-01".to_string(),
            fine_grained_tool_streaming: true,
        }
    }
}

impl AnthropicScheme {
    /// 新しいスキーマを作成
    pub fn new() -> Self {
        Self::default()
    }

    /// 細粒度ツールストリーミングを有効/無効にする
    pub fn with_fine_grained_tool_streaming(mut self, enabled: bool) -> Self {
        self.fine_grained_tool_streaming = enabled;
        self
    }
}
