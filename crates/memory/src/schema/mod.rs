//! Bounded evidence and source references shared by Subject Memory.
//!
//! These snapshots provide traceability, not authority to read Session content
//! or mutate Memory. Hosts validate access separately from stored attribution.

mod common;

pub(crate) use common::JsonSafeU64Pair;
pub use common::{EvidenceKind, EvidenceOrigin, EvidenceOriginKind, SourceEvidenceRef, SourceRef};
