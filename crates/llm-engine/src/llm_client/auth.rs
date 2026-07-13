//! `Scheme` 実装と通信層が要求する認証要件、および動的認証プロバイダ。
//!
//! `AuthRequirement` は scheme が宣言する「この scheme はどんな認証を
//! 期待するか」のランタイム記述で、設定ファイルや環境変数などから
//! [`super::transport::ResolvedAuth`] を組み立てる責務は呼び出し側にある。
//!
//! リクエスト毎にトークンが変わり得る認証は [`AuthProvider`] trait を
//! 実装し、[`super::transport::ResolvedAuth::Custom`] 経由で transport に渡す。

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
    /// 複合ヘッダ（呼び出し側が [`AuthProvider`] で解決）
    Custom,
}

/// リクエスト毎に認証ヘッダを動的に組み立てるプロバイダ。
///
/// access token が refresh で更新されたり、複数ヘッダを同時に注入する
/// 必要があるケースで使う。実体は呼び出し側に置き、llm-engine は
/// trait を知るだけ。
///
/// 返したヘッダはそのまま `HeaderMap` に挿入される。`Authorization`
/// 含む scheme 既定の認証ヘッダは送出されないので、必要なら
/// 実装側でセットすること。
#[async_trait]
pub trait AuthProvider: Send + Sync + std::fmt::Debug {
    /// 1 リクエスト分の認証ヘッダを返す。refresh が必要なら内部で行う。
    async fn headers(&self) -> Result<Vec<(HeaderName, HeaderValue)>, ClientError>;

    /// Conversation header / request compression が必要な backend profile かどうか。
    ///
    /// transport は呼び出し側の具象型を知らないため、この hook だけで
    /// 追加の wire behavior を切り替える。
    fn is_codex_backend(&self) -> bool {
        false
    }
}
