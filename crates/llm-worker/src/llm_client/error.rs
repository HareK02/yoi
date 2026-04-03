//! LLMクライアントエラー型

use std::fmt;

/// LLMクライアントのエラー
#[derive(Debug)]
pub enum ClientError {
    /// HTTPリクエストエラー
    Http(reqwest::Error),
    /// JSONパースエラー
    Json(serde_json::Error),
    /// SSEパースエラー
    Sse(String),
    /// APIエラー (プロバイダからのエラーレスポンス)
    Api {
        status: Option<u16>,
        code: Option<String>,
        message: String,
    },
    /// 設定エラー
    Config(String),
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ClientError::Http(e) => write!(f, "HTTP error: {}", e),
            ClientError::Json(e) => write!(f, "JSON parse error: {}", e),
            ClientError::Sse(msg) => write!(f, "SSE parse error: {}", msg),
            ClientError::Api {
                status,
                code,
                message,
            } => {
                write!(f, "API error")?;
                if let Some(s) = status {
                    write!(f, " (status: {})", s)?;
                }
                if let Some(c) = code {
                    write!(f, " [{}]", c)?;
                }
                write!(f, ": {}", message)
            }
            ClientError::Config(msg) => write!(f, "Config error: {}", msg),
        }
    }
}

impl std::error::Error for ClientError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ClientError::Http(e) => Some(e),
            ClientError::Json(e) => Some(e),
            _ => None,
        }
    }
}

impl From<reqwest::Error> for ClientError {
    fn from(err: reqwest::Error) -> Self {
        ClientError::Http(err)
    }
}

impl From<serde_json::Error> for ClientError {
    fn from(err: serde_json::Error) -> Self {
        ClientError::Json(err)
    }
}
