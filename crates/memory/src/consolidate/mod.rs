//! Phase 2: 統合 + 整理。
//!
//! Phase 1 が staging に残した活動ログを `memory/*` / `knowledge/*` に
//! 統合し、続けて既存 record を `outdated | superseded | unused | noisy`
//! の観点で整理する disposable Worker を、Pod 側が組み立てるための
//! ヘルパー群を提供する。Pod は次の手順で sub-Worker を構築する:
//!
//! - [`build_consolidate_input`] を sub-Worker の最初の user 入力に
//! - memory 専用 Tool (read / write / edit) と Knowledge / memory 検索ツールを登録
//! - [`StagingLock::acquire`] で並走防止 + consumed ID 確定
//! - sub-Worker run 完了後、[`StagingLock::release_with_cleanup`] で
//!   consumed ID 分の staging のみ削除し、占有ファイルを解放
//!
//! system prompt は Pod の `PromptCatalog`
//! (`PodPrompt::MemoryConsolidationSystem`) で管理される。Knowledge 化候補
//! レポートと使用頻度メトリクスは別チケットで供給される想定。本モジュール
//! 時点では空入力として扱い、prompt 側の説明だけ残しておく
//! （`docs/plan/memory.md` §Phase 2 / 整理材料）。

mod input;
mod lock;
mod staging;
mod tidy;

pub use input::{
    KnowledgeCandidateReport, build_consolidate_input, render_existing_memory_records,
    render_staging_records, render_tidy_hints,
};
pub use lock::{LockError, LockRecord, StagingLock};
pub use staging::{StagingEntry, list_staging_entries};
pub use tidy::{TidyHints, collect_tidy_hints};
