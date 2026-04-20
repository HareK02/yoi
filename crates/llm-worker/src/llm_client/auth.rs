//! `Scheme` 実装と通信層が要求する認証要件、および動的認証プロバイダ。
//!
//! マニフェスト側の型（`ModelConfig` / `SchemeKind` / `AuthRef`）は
//! `crates/manifest` に置き、llm-worker はそれを知らずに済む。
//! `AuthRequirement` は scheme が宣言する「この scheme はどんな認証を
//! 期待するか」のランタイム記述で、manifest 側の `AuthRef` との
//! 照合（`AuthRef → ResolvedAuth` 変換の適否）は `crates/provider`
//! で行う。
//!
//! Codex OAuth のようにリクエスト毎にトークンが変わり得る認証は
//! [`AuthProvider`] trait を `crates/provider` 側で実装し、
//! [`super::transport::ResolvedAuth::Custom`] 経由で transport に渡す。

use async_trait::async_trait;
use reqwest::header::{HeaderName, HeaderValue};

use super::error::ClientError;

/// `Scheme::required_auth()` が返す認証要件。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthRequirement {
    /// 認証を行わない（Ollama など）
    None,
    /// `Authorization: Bearer <token>` ヘッダ（token は API key 相当）
    Bearer,
    /// `x-api-key: <token>` ヘッダ（Anthropic 形式）
    XApiKey,
    /// クエリパラメータ `?<name>=<token>`（Gemini 形式）
    QueryParam { name: &'static str },
    /// 複合ヘッダ（Codex OAuth 等、`crates/provider` 側で解決）
    Custom,
}

/// リクエスト毎に認証ヘッダを動的に組み立てるプロバイダ。
///
/// Codex OAuth のように access_token が refresh で更新されたり、
/// `ChatGPT-Account-Id` / `X-OpenAI-Fedramp` のような複数ヘッダを
/// 同時に注入する必要があるケースで使う。実体は `crates/provider`
/// 側に置き、llm-worker は trait を知るだけ。
///
/// 返したヘッダはそのまま `HeaderMap` に挿入される。`Authorization`
/// 含む scheme 既定の認証ヘッダは送出されないので、必要なら
/// 実装側でセットすること。
#[async_trait]
pub trait AuthProvider: Send + Sync + std::fmt::Debug {
    /// 1 リクエスト分の認証ヘッダを返す。refresh が必要なら内部で行う。
    async fn headers(&self) -> Result<Vec<(HeaderName, HeaderValue)>, ClientError>;
}
