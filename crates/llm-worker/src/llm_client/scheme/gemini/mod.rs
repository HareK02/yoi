//! Google Gemini API スキーマ
//!
//! - リクエストJSON生成
//! - SSEイベントパース → Event変換

mod events;
mod request;

/// Geminiスキーマ
///
/// Google Gemini APIのリクエスト/レスポンス変換を担当
#[derive(Debug, Clone, Default)]
pub struct GeminiScheme {
    /// ストリーミング関数呼び出し引数を有効にするか
    pub stream_function_call_arguments: bool,
}

impl GeminiScheme {
    /// 新しいスキーマを作成
    pub fn new() -> Self {
        Self::default()
    }

    /// ストリーミング関数呼び出し引数を有効/無効にする
    pub fn with_stream_function_call_arguments(mut self, enabled: bool) -> Self {
        self.stream_function_call_arguments = enabled;
        self
    }
}
