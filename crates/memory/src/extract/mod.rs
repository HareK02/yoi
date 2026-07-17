//! extract: memory candidate extraction.
//!
//! 通常 Worker の post-run hook で発火する disposable Engine と、その
//! 出力を `<workspace>/.yoi/memory/_staging/<id>.json` に書き出す
//! ヘルパーを提供する。Worker 側はこのモジュールから:
//!
//! - [`build_extract_input`] を sub-Engine の最初の user 入力に
//! - [`write_extracted_tool`] を唯一のツールとして
//! - [`write_staging`] で受け取った JSON を staging に書き出し
//!
//! の順で組み立てる。system prompt は Worker の `PromptCatalog`
//! (`WorkerPrompt::MemoryExtractSystem`) で管理される。pointer 永続化
//! （session-store の `LogEntry::Extension`、domain `"memory.extract"`）は
//! Worker 側が責務を持つ。
//!
//! 出力 JSON の wrap は [`write_staging`] が `source: { segment_id, range }`
//! を機械付与する形で担当し、LLM には source を推論させない。

mod input;
mod payload;
mod pointer;
mod staging;
mod tool;

pub use input::build_extract_input;
pub use payload::{
    CandidateKind, ExtractedCandidate, ExtractedPayload, STAGING_SCHEMA_VERSION, StagingEvidence,
    StagingRecord,
};
pub use pointer::{ExtractPointerPayload, fold_pointer};
pub use staging::{StagingWriteResult, write_staging};
pub use tool::{ExtractWorkerContext, write_extracted_tool};

/// session-store `LogEntry::Extension` で使う domain 名。
/// pointer の永続化と読み出しはこの定数を使う側が一致している必要がある。
pub const EXTRACT_DOMAIN: &str = "memory.extract";
