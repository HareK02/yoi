//! Boundary for Memory extraction candidates and Worker-facing operations.
//!
//! Extraction creates typed candidates and passes them to Host storage. Saving candidates and
//! committing the extraction pointer are separate responsibilities; generating candidates alone does not update committed Memory.
//! This module does not store data in Repository files.

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

/// Domain name used in session-store `LogEntry::Extension`.
/// Callers persisting and reading pointers must agree on this constant.
pub const EXTRACT_DOMAIN: &str = "memory.extract";
