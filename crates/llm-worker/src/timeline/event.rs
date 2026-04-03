//! Timeline層のイベント型
//!
//! Timelineが受け取り、各Handlerへディスパッチするイベント表現。

use serde::{Deserialize, Serialize};

// =============================================================================
// Core Event Types (from llm_client layer)
// =============================================================================

/// LLMからのストリーミングイベント
///
/// 各LLMプロバイダからのレスポンスは、この`Event`のストリームとして
/// 統一的に処理されます。
///
/// # イベントの種類
///
/// - **メタイベント**: `Ping`, `Usage`, `Status`, `Error`
/// - **ブロックイベント**: `BlockStart`, `BlockDelta`, `BlockStop`, `BlockAbort`
///
/// # ブロックのライフサイクル
///
/// テキストやツール呼び出しは、`BlockStart` → `BlockDelta`(複数) → `BlockStop`
/// の順序でイベントが発生します。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Event {
    /// ハートビート
    Ping(PingEvent),
    /// トークン使用量
    Usage(UsageEvent),
    /// ストリームのステータス変化
    Status(StatusEvent),
    /// エラー発生
    Error(ErrorEvent),

    /// ブロック開始（テキスト、ツール使用等）
    BlockStart(BlockStart),
    /// ブロックの差分データ
    BlockDelta(BlockDelta),
    /// ブロック正常終了
    BlockStop(BlockStop),
    /// ブロック中断
    BlockAbort(BlockAbort),
}

// =============================================================================
// Meta Events
// =============================================================================

/// Pingイベント（ハートビート）
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct PingEvent {
    pub timestamp: Option<u64>,
}

/// 使用量イベント
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct UsageEvent {
    /// 入力トークン数
    pub input_tokens: Option<u64>,
    /// 出力トークン数
    pub output_tokens: Option<u64>,
    /// 合計トークン数
    pub total_tokens: Option<u64>,
    /// キャッシュ読み込みトークン数
    pub cache_read_input_tokens: Option<u64>,
    /// キャッシュ作成トークン数
    pub cache_creation_input_tokens: Option<u64>,
}

/// ステータスイベント
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatusEvent {
    pub status: ResponseStatus,
}

/// レスポンスステータス
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ResponseStatus {
    /// ストリーム開始
    Started,
    /// 正常完了
    Completed,
    /// キャンセルされた
    Cancelled,
    /// エラー発生
    Failed,
}

/// エラーイベント
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ErrorEvent {
    pub code: Option<String>,
    pub message: String,
}

// =============================================================================
// Block Types
// =============================================================================

/// ブロックの種別
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BlockType {
    /// テキスト生成
    Text,
    /// 思考 (Claude Extended Thinking等)
    Thinking,
    /// ツール呼び出し
    ToolUse,
    /// ツール結果
    ToolResult,
}

/// ブロック開始イベント
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BlockStart {
    /// ブロックのインデックス
    pub index: usize,
    /// ブロックの種別
    pub block_type: BlockType,
    /// ブロック固有のメタデータ
    pub metadata: BlockMetadata,
}

impl BlockStart {
    pub fn block_type(&self) -> BlockType {
        self.block_type
    }
}

/// ブロックのメタデータ
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum BlockMetadata {
    Text,
    Thinking,
    ToolUse { id: String, name: String },
    ToolResult { tool_use_id: String },
}

/// ブロックデルタイベント
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BlockDelta {
    /// ブロックのインデックス
    pub index: usize,
    /// デルタの内容
    pub delta: DeltaContent,
}

/// デルタの内容
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DeltaContent {
    /// テキストデルタ
    Text(String),
    /// 思考デルタ
    Thinking(String),
    /// ツール引数のJSON部分文字列
    InputJson(String),
}

impl DeltaContent {
    /// デルタのブロック種別を取得
    pub fn block_type(&self) -> BlockType {
        match self {
            DeltaContent::Text(_) => BlockType::Text,
            DeltaContent::Thinking(_) => BlockType::Thinking,
            DeltaContent::InputJson(_) => BlockType::ToolUse,
        }
    }
}

/// ブロック停止イベント
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BlockStop {
    /// ブロックのインデックス
    pub index: usize,
    /// ブロックの種別
    pub block_type: BlockType,
    /// 停止理由
    pub stop_reason: Option<StopReason>,
}

impl BlockStop {
    pub fn block_type(&self) -> BlockType {
        self.block_type
    }
}

/// ブロック中断イベント
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BlockAbort {
    /// ブロックのインデックス
    pub index: usize,
    /// ブロックの種別
    pub block_type: BlockType,
    /// 中断理由
    pub reason: String,
}

impl BlockAbort {
    pub fn block_type(&self) -> BlockType {
        self.block_type
    }
}

/// 停止理由
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum StopReason {
    /// 自然終了
    EndTurn,
    /// 最大トークン数到達
    MaxTokens,
    /// ストップシーケンス到達
    StopSequence,
    /// ツール使用
    ToolUse,
}

// =============================================================================
// Builder / Factory helpers
// =============================================================================

impl Event {
    /// テキストブロック開始イベントを作成
    pub fn text_block_start(index: usize) -> Self {
        Event::BlockStart(BlockStart {
            index,
            block_type: BlockType::Text,
            metadata: BlockMetadata::Text,
        })
    }

    /// テキストデルタイベントを作成
    pub fn text_delta(index: usize, text: impl Into<String>) -> Self {
        Event::BlockDelta(BlockDelta {
            index,
            delta: DeltaContent::Text(text.into()),
        })
    }

    /// テキストブロック停止イベントを作成
    pub fn text_block_stop(index: usize, stop_reason: Option<StopReason>) -> Self {
        Event::BlockStop(BlockStop {
            index,
            block_type: BlockType::Text,
            stop_reason,
        })
    }

    /// ツール使用ブロック開始イベントを作成
    pub fn tool_use_start(index: usize, id: impl Into<String>, name: impl Into<String>) -> Self {
        Event::BlockStart(BlockStart {
            index,
            block_type: BlockType::ToolUse,
            metadata: BlockMetadata::ToolUse {
                id: id.into(),
                name: name.into(),
            },
        })
    }

    /// ツール引数デルタイベントを作成
    pub fn tool_input_delta(index: usize, json: impl Into<String>) -> Self {
        Event::BlockDelta(BlockDelta {
            index,
            delta: DeltaContent::InputJson(json.into()),
        })
    }

    /// ツール使用ブロック停止イベントを作成
    pub fn tool_use_stop(index: usize) -> Self {
        Event::BlockStop(BlockStop {
            index,
            block_type: BlockType::ToolUse,
            stop_reason: Some(StopReason::ToolUse),
        })
    }

    /// 使用量イベントを作成
    pub fn usage(input_tokens: u64, output_tokens: u64) -> Self {
        Event::Usage(UsageEvent {
            input_tokens: Some(input_tokens),
            output_tokens: Some(output_tokens),
            total_tokens: Some(input_tokens + output_tokens),
            cache_read_input_tokens: None,
            cache_creation_input_tokens: None,
        })
    }

    /// Pingイベントを作成
    pub fn ping() -> Self {
        Event::Ping(PingEvent { timestamp: None })
    }
}

// =============================================================================
// Conversions: llm_client::event -> timeline::event
// =============================================================================

impl From<crate::llm_client::event::ResponseStatus> for ResponseStatus {
    fn from(value: crate::llm_client::event::ResponseStatus) -> Self {
        match value {
            crate::llm_client::event::ResponseStatus::Started => ResponseStatus::Started,
            crate::llm_client::event::ResponseStatus::Completed => ResponseStatus::Completed,
            crate::llm_client::event::ResponseStatus::Cancelled => ResponseStatus::Cancelled,
            crate::llm_client::event::ResponseStatus::Failed => ResponseStatus::Failed,
        }
    }
}

impl From<crate::llm_client::event::BlockType> for BlockType {
    fn from(value: crate::llm_client::event::BlockType) -> Self {
        match value {
            crate::llm_client::event::BlockType::Text => BlockType::Text,
            crate::llm_client::event::BlockType::Thinking => BlockType::Thinking,
            crate::llm_client::event::BlockType::ToolUse => BlockType::ToolUse,
            crate::llm_client::event::BlockType::ToolResult => BlockType::ToolResult,
        }
    }
}

impl From<crate::llm_client::event::BlockMetadata> for BlockMetadata {
    fn from(value: crate::llm_client::event::BlockMetadata) -> Self {
        match value {
            crate::llm_client::event::BlockMetadata::Text => BlockMetadata::Text,
            crate::llm_client::event::BlockMetadata::Thinking => BlockMetadata::Thinking,
            crate::llm_client::event::BlockMetadata::ToolUse { id, name } => {
                BlockMetadata::ToolUse { id, name }
            }
            crate::llm_client::event::BlockMetadata::ToolResult { tool_use_id } => {
                BlockMetadata::ToolResult { tool_use_id }
            }
        }
    }
}

impl From<crate::llm_client::event::DeltaContent> for DeltaContent {
    fn from(value: crate::llm_client::event::DeltaContent) -> Self {
        match value {
            crate::llm_client::event::DeltaContent::Text(text) => DeltaContent::Text(text),
            crate::llm_client::event::DeltaContent::Thinking(text) => DeltaContent::Thinking(text),
            crate::llm_client::event::DeltaContent::InputJson(json) => {
                DeltaContent::InputJson(json)
            }
        }
    }
}

impl From<crate::llm_client::event::StopReason> for StopReason {
    fn from(value: crate::llm_client::event::StopReason) -> Self {
        match value {
            crate::llm_client::event::StopReason::EndTurn => StopReason::EndTurn,
            crate::llm_client::event::StopReason::MaxTokens => StopReason::MaxTokens,
            crate::llm_client::event::StopReason::StopSequence => StopReason::StopSequence,
            crate::llm_client::event::StopReason::ToolUse => StopReason::ToolUse,
        }
    }
}

impl From<crate::llm_client::event::PingEvent> for PingEvent {
    fn from(value: crate::llm_client::event::PingEvent) -> Self {
        PingEvent {
            timestamp: value.timestamp,
        }
    }
}

impl From<crate::llm_client::event::UsageEvent> for UsageEvent {
    fn from(value: crate::llm_client::event::UsageEvent) -> Self {
        UsageEvent {
            input_tokens: value.input_tokens,
            output_tokens: value.output_tokens,
            total_tokens: value.total_tokens,
            cache_read_input_tokens: value.cache_read_input_tokens,
            cache_creation_input_tokens: value.cache_creation_input_tokens,
        }
    }
}

impl From<crate::llm_client::event::StatusEvent> for StatusEvent {
    fn from(value: crate::llm_client::event::StatusEvent) -> Self {
        StatusEvent {
            status: value.status.into(),
        }
    }
}

impl From<crate::llm_client::event::ErrorEvent> for ErrorEvent {
    fn from(value: crate::llm_client::event::ErrorEvent) -> Self {
        ErrorEvent {
            code: value.code,
            message: value.message,
        }
    }
}

impl From<crate::llm_client::event::BlockStart> for BlockStart {
    fn from(value: crate::llm_client::event::BlockStart) -> Self {
        BlockStart {
            index: value.index,
            block_type: value.block_type.into(),
            metadata: value.metadata.into(),
        }
    }
}

impl From<crate::llm_client::event::BlockDelta> for BlockDelta {
    fn from(value: crate::llm_client::event::BlockDelta) -> Self {
        BlockDelta {
            index: value.index,
            delta: value.delta.into(),
        }
    }
}

impl From<crate::llm_client::event::BlockStop> for BlockStop {
    fn from(value: crate::llm_client::event::BlockStop) -> Self {
        BlockStop {
            index: value.index,
            block_type: value.block_type.into(),
            stop_reason: value.stop_reason.map(Into::into),
        }
    }
}

impl From<crate::llm_client::event::BlockAbort> for BlockAbort {
    fn from(value: crate::llm_client::event::BlockAbort) -> Self {
        BlockAbort {
            index: value.index,
            block_type: value.block_type.into(),
            reason: value.reason,
        }
    }
}

impl From<crate::llm_client::event::Event> for Event {
    fn from(value: crate::llm_client::event::Event) -> Self {
        match value {
            crate::llm_client::event::Event::Ping(p) => Event::Ping(p.into()),
            crate::llm_client::event::Event::Usage(u) => Event::Usage(u.into()),
            crate::llm_client::event::Event::Status(s) => Event::Status(s.into()),
            crate::llm_client::event::Event::Error(e) => Event::Error(e.into()),
            crate::llm_client::event::Event::BlockStart(s) => Event::BlockStart(s.into()),
            crate::llm_client::event::Event::BlockDelta(d) => Event::BlockDelta(d.into()),
            crate::llm_client::event::Event::BlockStop(s) => Event::BlockStop(s.into()),
            crate::llm_client::event::Event::BlockAbort(a) => Event::BlockAbort(a.into()),
        }
    }
}
