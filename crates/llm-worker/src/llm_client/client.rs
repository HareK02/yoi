//! LLMクライアント共通trait定義

use std::pin::Pin;

use crate::llm_client::{ClientError, Request, RequestConfig, event::Event};
use async_trait::async_trait;
use futures::Stream;

/// 設定に関する警告
///
/// プロバイダがサポートしていない設定を使用した場合に返される。
#[derive(Debug, Clone)]
pub struct ConfigWarning {
    /// 設定オプション名
    pub option_name: &'static str,
    /// 警告メッセージ
    pub message: String,
}

impl ConfigWarning {
    /// 新しい警告を作成
    pub fn unsupported(option_name: &'static str, provider_name: &str) -> Self {
        Self {
            option_name,
            message: format!(
                "'{}' is not supported by {} and will be ignored",
                option_name, provider_name
            ),
        }
    }
}

impl std::fmt::Display for ConfigWarning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.option_name, self.message)
    }
}

/// LLMクライアントのtrait
///
/// 各プロバイダはこのtraitを実装し、統一されたインターフェースを提供する。
#[async_trait]
pub trait LlmClient: Send + Sync {
    /// ストリーミングリクエストを送信し、Eventストリームを返す
    ///
    /// # Arguments
    /// * `request` - リクエスト情報
    ///
    /// # Returns
    /// * `Ok(Stream)` - イベントストリーム
    /// * `Err(ClientError)` - エラー
    async fn stream(
        &self,
        request: Request,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<Event, ClientError>> + Send>>, ClientError>;

    /// 設定をバリデーションし、未サポートの設定があれば警告を返す
    ///
    /// # Arguments
    /// * `config` - バリデーション対象の設定
    ///
    /// # Returns
    /// サポートされていない設定に対する警告のリスト
    fn validate_config(&self, config: &RequestConfig) -> Vec<ConfigWarning> {
        // デフォルト実装: 全ての設定をサポート
        let _ = config;
        Vec::new()
    }
}

/// `Box<dyn LlmClient>` に対する `LlmClient` の実装
///
/// これにより、動的ディスパッチを使用するクライアントも `Worker` で利用可能になる。
#[async_trait]
impl LlmClient for Box<dyn LlmClient> {
    async fn stream(
        &self,
        request: Request,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<Event, ClientError>> + Send>>, ClientError> {
        (**self).stream(request).await
    }

    fn validate_config(&self, config: &RequestConfig) -> Vec<ConfigWarning> {
        (**self).validate_config(config)
    }
}
