//! `Scheme` 実装と通信層が要求する認証要件。
//!
//! マニフェスト側の型（`ModelConfig` / `SchemeKind` / `AuthRef`）は
//! `crates/manifest` に置き、llm-worker はそれを知らずに済む。
//! `AuthRequirement` は scheme が宣言する「この scheme はどんな認証を
//! 期待するか」のランタイム記述で、manifest 側の `AuthRef` との
//! 照合（`AuthRef → ResolvedAuth` 変換の適否）は `crates/provider`
//! で行う。

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
