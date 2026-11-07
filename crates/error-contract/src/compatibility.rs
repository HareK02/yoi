//! Reader-first compatibility for optional structured failure attachments.
//!
//! Existing domain envelopes retain their operation outcome and legacy diagnostics. Ship a
//! reader accepting an absent attachment before enabling a writer of a versioned failure.
//! Absence is historical text without typed facts, not a fabricated `unknown` domain cause.
//! Present-but-corrupt and present-but-unsupported are distinct and must not fall back to
//! parsing legacy prose. Readers reproject validated attachments using current visibility.
//!
//! This module does not migrate DBs or rewrite old archives. The owning store must choose
//! its additive field/migration and rollout unit with its consumer. A rollback disables new
//! writes first and retains the reader while any new-format results remain. Unknown payload
//! retention is local to the original reader; it is never permission to forward to another
//! audience. No production writer is enabled merely by depending on this crate.
use crate::*;

pub enum StoredFailureRead<D> {
    LegacyWithoutFacts,
    Structured(DecodeFailure<D>),
}

pub fn read_failure_attachment<D: WireDetails>(
    attachment: Option<&[u8]>,
    budget: Budget,
) -> StoredFailureRead<D> {
    match attachment {
        None => StoredFailureRead::LegacyWithoutFacts,
        Some(bytes) => StoredFailureRead::Structured(decode_failure(bytes, budget)),
    }
}
