//! Memory staging queue helpers.
//!
//! Staging candidates are consumed by the backend-managed memory-consolidation
//! Worker through MemoryStaging tools. This module only exposes bounded staging
//! listing/read support; consolidation decisions are recorded by the backend close
//! operation before a staging file is deleted.

mod staging;

pub use staging::{
    StagingEntriesSnapshot, StagingEntry, list_staging_entries, list_staging_entries_snapshot,
};
