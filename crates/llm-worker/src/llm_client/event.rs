//! LLMクライアント層のイベント型
//!
//! 各LLMプロバイダからのストリーミングレスポンスを表現するイベント型。

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
///
/// プロバイダから受信した 1 LLM リクエスト分のトークン会計。
/// 各 scheme で正規化され、フィールドの意味は全プロバイダ共通:
///
/// - `input_tokens` は **送信した prompt prefix 全体の占有量**（プロンプト全長）。
///   キャッシュヒット分も含まれる。Anthropic は raw API では非キャッシュ分のみを
///   `input_tokens` として返すため、`AnthropicScheme::convert_usage` で
///   `cache_read + cache_creation` を加算してこの規約に揃えている。
/// - `cache_read_input_tokens` / `cache_creation_input_tokens` は上記の内訳で、
///   料金会計用。占有量からは差し引かない。
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct UsageEvent {
    /// 送信した prompt prefix の総トークン数（占有量、キャッシュ込み）
    pub input_tokens: Option<u64>,
    /// このリクエストで生成された出力トークン数
    pub output_tokens: Option<u64>,
    /// `input_tokens + output_tokens`
    pub total_tokens: Option<u64>,
    /// `input_tokens` のうちキャッシュから読まれた分（割引料金）
    pub cache_read_input_tokens: Option<u64>,
    /// `input_tokens` のうちこのリクエストでキャッシュに書かれた分（割増料金、Anthropic）
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
