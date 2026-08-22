//! Timeline層
//!
//! LLMからのイベントストリームを受信し、登録されたHandlerにディスパッチします。
//!
//! # 主要コンポーネント
//!
//! - [`Timeline`] - イベントストリームの管理とディスパッチ
//! - [`Handler`] - イベントを処理するトレイト
//! - [`TextBlockCollector`] - テキストブロックを収集するHandler
//! - [`ThinkingBlockCollector`] - reasoning material を Thinking block から収集するHandler
//! - [`ToolCallCollector`] - ツール呼び出しを収集するHandler

pub mod event;
mod text_block_collector;
mod thinking_block_collector;
mod timeline;
mod tool_call_collector;

// 公開API
pub use event::*;
pub use text_block_collector::TextBlockCollector;
pub use thinking_block_collector::ThinkingBlockCollector;
pub use timeline::Timeline;
pub use tool_call_collector::ToolCallCollector;

// 型定義からのre-export
pub use crate::handler::{
    // Meta Kinds
    ErrorKind,
    // Core traits
    Handler,
    Kind,
    PingKind,
    StatusKind,
    // Block Events
    TextBlockEvent,
    // Block Kinds
    TextBlockKind,
    TextBlockStart,
    TextBlockStop,
    ThinkingBlockEvent,
    ThinkingBlockKind,
    ThinkingBlockStart,
    ThinkingBlockStop,
    ToolUseBlockEvent,
    ToolUseBlockKind,
    ToolUseBlockStart,
    ToolUseBlockStop,
    UsageKind,
};
