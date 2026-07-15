//! consolidation: 統合 + 整理。
//!
//! extract が staging に残した活動ログを `memory/*` に
//! 統合し、続けて既存 record を `outdated | superseded | unused | noisy`
//! の観点で整理する disposable Engine を、Worker 側が組み立てるための
//! ヘルパー群を提供する。Worker は次の手順で sub-Engine を構築する:
//!
//! - [`build_consolidate_input`] を sub-Engine の最初の user 入力に
//! - memory 専用 Tool (read / write / edit) と memory 検索ツールを登録
//! - [`StagingLock::acquire`] で並走防止 + consumed ID 確定
//! - sub-Engine run 完了後、[`StagingLock::release_with_cleanup`] で
//!   consumed ID 分の staging のみ削除し、占有ファイルを解放
//!
//! system prompt は Worker の `PromptCatalog`
//! (`WorkerPrompt::MemoryConsolidationSystem`) で管理される。Usage report は
//! 判断材料として渡すだけで、ここでは protection の hard decision はしない
//! （`docs/plan/memory.md` §Consolidation / 整理材料）。

mod input;
mod lock;
mod staging;
mod tidy;

pub use input::{
    build_consolidate_input, render_existing_memory_records, render_staging_records,
    render_tidy_hints,
};
pub use lock::{LockError, LockRecord, StagingLock};
pub use staging::{
    StagingEntriesSnapshot, StagingEntry, list_staging_entries, list_staging_entries_snapshot,
};
pub use tidy::{TidyHints, collect_tidy_hints};
