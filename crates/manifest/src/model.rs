//! LLM モデル宣言型
//!
//! Pod マニフェストの `[model]` セクションで記述する型。`scheme` と
//! `auth` を直交軸として表現し、1 つの汎用アダプタ（`crates/provider`）
//! で任意の wire / 認証組合せを受け止める。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

// `ModelCapability` は `llm-worker` 側に定義される runtime 構造だが、
// マニフェストで任意に override できるよう型だけ再エクスポートする。
pub use llm_worker::llm_client::capability::ModelCapability;

/// Pod が使う LLM モデルの宣言。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelConfig {
    /// wire format
    pub scheme: SchemeKind,
    /// API のベース URL。未指定なら scheme の既定値にフォールバック
    #[serde(default)]
    pub base_url: Option<String>,
    /// プロバイダが受け付けるモデル ID
    pub model_id: String,
    /// 認証方式
    #[serde(default)]
    pub auth: AuthRef,
    /// モデル能力の明示指定。`None` のときは `crates/provider` が
    /// scheme 静的テーブル → scheme 既定値の順でフォールバックする。
    /// OpenAI 互換ルーター（OpenRouter / xAI / Groq 等）で scheme テーブル
    /// に載っていないモデル ID を使うときに指定する。
    #[serde(default)]
    pub capability: Option<ModelCapability>,
}

/// サポートする wire scheme の種類。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SchemeKind {
    /// Anthropic Messages API (`/v1/messages`)。Ollama `/v1/messages` もこれで扱う
    Anthropic,
    /// OpenAI Chat Completions (`/v1/chat/completions`)。OpenAI 互換ルーター共通枠
    OpenaiChat,
    /// OpenAI Responses API (`/v1/responses`)。別チケットで scheme 新設予定
    OpenaiResponses,
    /// Google Gemini (`/v1beta/models/...:streamGenerateContent`)
    Gemini,
}

/// 認証の参照。
///
/// 実際のトークン値の解決（env / file 読取、OAuth refresh 等）は
/// `crates/provider` で行う。ここはあくまで「どこから取るか」の宣言。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AuthRef {
    /// 認証不要（ローカル Ollama 等）
    #[default]
    None,
    /// API key。env / file のいずれか（両方指定された場合は env が優先）
    ApiKey {
        /// 環境変数名。未指定のときは scheme ごとの既定（`INSOMNIA_API_KEY_*`）
        #[serde(default)]
        env: Option<String>,
        /// key を書き込んだファイル（絶対パス）
        #[serde(default)]
        file: Option<PathBuf>,
    },
    /// ChatGPT OAuth（`~/.codex/auth.json`）。実装は `llm-auth-codex-oauth` チケット
    #[serde(rename = "codex_oauth")]
    CodexOAuth,
}

impl SchemeKind {
    /// 既定の環境変数名（`INSOMNIA_API_KEY_*`）。
    ///
    /// `AuthRef::ApiKey { env: None, .. }` の env 未指定時に使う。
    pub fn default_env_var(self) -> &'static str {
        match self {
            Self::Anthropic => "INSOMNIA_API_KEY_ANTHROPIC",
            Self::OpenaiChat | Self::OpenaiResponses => "INSOMNIA_API_KEY_OPENAI",
            Self::Gemini => "INSOMNIA_API_KEY_GEMINI",
        }
    }
}
