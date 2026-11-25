//! Candidate and progress payloads shared by Subject Memory extraction.
//!
//! Saving candidates and committing the extraction pointer are separate Host
//! responsibilities; generating candidates alone does not update committed Memory.
//! This module does not store data or register an extraction Worker.

mod payload;
mod pointer;

pub use payload::{
    CandidateKind, ExtractedCandidate, STAGING_SCHEMA_VERSION, StagingEvidence, StagingRecord,
};
pub use pointer::ExtractPointerPayload;
