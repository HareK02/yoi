//! LLMクライアント層
//!
//! 各LLMプロバイダと通信し、統一された[`Event`]
//! ストリームを出力します。
//!
//! # サポートするプロバイダ
//!
//! - Anthropic (Claude)
//! - OpenAI (GPT-4, etc.)
//! - Google (Gemini)
//! - Ollama (ローカルLLM)
//!
//! # アーキテクチャ
//!
//! - [`LlmClient`] - プロバイダ共通のtrait
//! - `providers`: プロバイダ固有のクライアント実装
//! - `scheme`: APIスキーマ（リクエスト/レスポンス変換）

pub mod auth;
pub mod capability;
pub mod client;
pub mod error;
pub mod event;
pub mod types;

pub mod scheme;
pub mod transport;

pub use auth::*;
pub use capability::*;
pub use client::*;
pub use error::*;
pub use event::*;
pub use types::*;
