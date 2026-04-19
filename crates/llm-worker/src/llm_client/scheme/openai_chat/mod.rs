//! OpenAI Chat Completions API スキーマ
//!
//! - リクエストJSON生成
//! - SSEイベントパース → Event変換

pub(crate) mod capability;
mod events;
mod request;
mod scheme_impl;

/// OpenAIスキーマ
///
/// OpenAI Chat Completions API (および互換API) のリクエスト/レスポンス変換を担当
#[derive(Debug, Clone, Default)]
pub struct OpenAIScheme {
    /// モデル名 (リクエスト時に指定されるが、デフォルト値として保持も可能)
    pub model: Option<String>,
    /// レガシーなmax_tokensを使用するか (Ollama互換用)
    pub use_legacy_max_tokens: bool,
}

impl OpenAIScheme {
    /// 新しいスキーマを作成
    pub fn new() -> Self {
        Self::default()
    }

    /// レガシーなmax_tokensを使用するか設定
    pub fn with_legacy_max_tokens(mut self, use_legacy: bool) -> Self {
        self.use_legacy_max_tokens = use_legacy;
        self
    }
}
