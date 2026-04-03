//! APIスキーマ定義
//!
//! 各APIスキーマごとの変換ロジック
//! - リクエスト変換: Request → プロバイダ固有JSON
//! - レスポンス変換: SSEイベント → Event

pub mod anthropic;
pub mod gemini;
pub mod openai;
