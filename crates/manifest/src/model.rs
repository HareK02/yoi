//! LLM モデル宣言型
//!
//! Pod マニフェストの `[model]` セクションで記述する型。`ref`（プロバイダ
//! とモデルを両方指し示す短縮形）と inline 指定（`scheme` / `model_id`
//! 直書き）の両方を受け入れるため、すべてのフィールドを `Option` として
//! 持つ 1 つの型 [`ModelManifest`] に統合している。実解決（ref をプロバイダ
//! カタログ / モデルカタログから引いて `scheme` や `model_id` を埋める）
//! は `crates/provider` の責務で、本モジュールはデータ表現のみを提供する。
//!
//! 同じ型を partial（カスケード層）と完成形（最終マニフェスト）の両方で
//! 使うことで、merge と最終変換の重複を避ける。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

// `ModelCapability` は `llm-worker` 側に定義される runtime 構造だが、
// マニフェストで任意に override できるよう型だけ再エクスポートする。
pub use llm_worker::llm_client::capability::ModelCapability;

/// Pod マニフェストの `[model]` セクション。
///
/// - ref だけ書く: `[model] ref = "anthropic/claude-sonnet-4-6"`
/// - ref + 一部 override: ref で基底を引き、`auth` 等だけ書き換え
/// - 完全 inline: `ref` を省略して `scheme` / `model_id` / `auth` を直書き
///
/// どの形が有効かの判定は `provider::resolve_model_manifest` が担う。
/// 本クレートは「どこから取るか」を表現するだけで、未設定かどうかを
/// 理由にした hard error は出さない。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ModelManifest {
    /// `<provider_id>/<model_id_in_ref>` 形式のカタログ参照。`/` の
    /// 最初の 1 文字目で split し provider カタログを引く。
    /// OpenRouter の `anthropic/claude-sonnet-4` のように `/` を含む
    /// model_id は `openrouter/anthropic/claude-sonnet-4` と書く
    /// （provider 側で最初の `/` のみ split するため）。
    #[serde(default, rename = "ref", skip_serializing_if = "Option::is_none")]
    pub ref_: Option<String>,
    /// wire format の明示指定。ref 未指定時は必須、ref 指定時は override。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheme: Option<SchemeKind>,
    /// API のベース URL。scheme の既定値を override する。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// プロバイダが受け付けるモデル ID。ref 未指定時は必須。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
    /// 認証方式。ref 未指定時は必須、ref 指定時は override。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<AuthRef>,
    /// モデル能力の明示指定。未指定時はモデルカタログ → provider
    /// `default_capability` → scheme 既定の順で解決される。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability: Option<ModelCapability>,
}

impl ModelManifest {
    /// `upper` を `self` に上書きマージする。マニフェスト cascade 向け
    /// （builtin → user → project → overlay の優先順位で呼ばれる）。
    pub fn merge(self, upper: Self) -> Self {
        Self {
            ref_: upper.ref_.or(self.ref_),
            scheme: upper.scheme.or(self.scheme),
            base_url: upper.base_url.or(self.base_url),
            model_id: upper.model_id.or(self.model_id),
            auth: upper.auth.or(self.auth),
            capability: upper.capability.or(self.capability),
        }
    }
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
