//! APIスキーマ定義
//!
//! 各APIスキーマごとの変換ロジック
//! - リクエスト変換: Request → プロバイダ固有JSON
//! - レスポンス変換: SSEイベント → Event
//!
//! [`Scheme`] trait により `HttpTransport<S>` から scheme 固有の差分
//! （パス、ヘッダ、認証要件、body 生成、SSE パース）をすべて委譲する。

pub mod anthropic;
pub mod gemini;
pub mod openai_chat;
pub mod openai_responses;

use serde_json::Value;

use super::auth::AuthRequirement;
use super::capability::ModelCapability;
use super::client::ConfigWarning;
use super::error::ClientError;
use super::event::Event;
use super::types::{Request, RequestConfig};

/// wire scheme の抽象。各プロバイダの API 仕様ごとに 1 つ実装する。
///
/// `HttpTransport<S: Scheme>` が URL 組立・認証ヘッダ挿入・SSE パース
/// のループを担い、`Scheme` 実装は各仕様固有の差分のみ提供する。
///
/// # 状態
///
/// SSE パースでフレーム間に状態を保つ必要がある scheme（Anthropic の
/// `BlockStop` に `block_type` が載らない仕様の補完など）は
/// [`Scheme::State`] に中間状態を表す型を置く。
/// 状態を持たない scheme は `type State = ()` とする。
pub trait Scheme: Clone + Send + Sync + 'static {
    /// SSE パースのフレーム間で共有する状態。`HttpTransport` が
    /// ストリーム開始時に `Default::default()` を一度だけ作り、
    /// フレームごとに `&mut` で渡す。
    type State: Default + Send + 'static;

    /// scheme のベース URL（`ModelConfig::base_url` 未指定時のデフォルト）
    fn default_base_url(&self) -> &'static str;

    /// リクエスト先の相対パス。Gemini のようにモデル名をパスに埋め込む
    /// プロバイダもあるため、モデル ID を受け取る。
    fn path(&self, model_id: &str) -> String;

    /// この scheme が要求する認証形式。`build_client` 時に
    /// `manifest::AuthRef` と照合する。
    fn required_auth(&self) -> AuthRequirement;

    /// `Content-Type` 以外の追加ヘッダ。`anthropic-version` / `anthropic-beta` 等。
    fn additional_headers(&self) -> Vec<(&'static str, String)> {
        Vec::new()
    }

    /// リクエスト body を生成する。`capability` は `CacheStrategy` や
    /// `ReasoningSupport` を参照して scheme 側の挙動を分岐させるため
    /// に渡される。
    fn build_request_body(
        &self,
        model_id: &str,
        request: &Request,
        capability: &ModelCapability,
    ) -> Value;

    /// SSE イベント 1 件を 0 個以上の [`Event`] に変換する。
    ///
    /// `event_type` は SSE フレームの `event:` フィールド、`data` は
    /// `data:` フィールド。`[DONE]` 等の終端マーカーは実装側で判定する。
    /// `state` はストリーム単位で共有される可変状態。
    fn parse_sse(
        &self,
        event_type: &str,
        data: &str,
        state: &mut Self::State,
    ) -> Result<Vec<Event>, ClientError>;

    /// scheme 既定の capability。モデル ID に関係なく、この wire で
    /// 安全に送れる最小共通項を返す。既知モデル ID の能力テーブルは
    /// `provider::capability::lookup` 側(高レベル構築層)の責務で、
    /// scheme はここには関与しない。
    fn default_capability(&self) -> ModelCapability;

    /// scheme 側でサポートしていない `RequestConfig` フィールドを
    /// 警告として返す（例: OpenAI Chat は `top_k` 非対応）。
    /// デフォルトは空 Vec。
    fn validate_config(&self, config: &RequestConfig) -> Vec<ConfigWarning> {
        let _ = config;
        Vec::new()
    }
}
