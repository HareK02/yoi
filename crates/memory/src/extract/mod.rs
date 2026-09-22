//! Memory candidate extraction domain types and Worker tool boundary.
//!
//! Worker-side extraction produces typed candidates that are submitted to the
//! Workspace backend. Staging persistence is owned by that backend; this module
//! does not write repository-local files.

mod input;
mod payload;
mod pointer;
mod tool;

pub use input::build_extract_input;
pub use payload::{
    CandidateKind, ExtractedCandidate, ExtractedPayload, STAGING_SCHEMA_VERSION, StagingEvidence,
    StagingRecord,
};
pub use pointer::{ExtractPointerPayload, fold_pointer};
pub use tool::{ExtractWorkerContext, write_extracted_tool};

/// session-store `LogEntry::Extension` で使う domain 名。
/// pointer の永続化と読み出しはこの定数を使う側が一致している必要がある。
pub const EXTRACT_DOMAIN: &str = "memory.extract";
