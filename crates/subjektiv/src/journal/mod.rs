//! Durable activity records owned by Subjektiv, independent of Session retention.
//!
//! Hosts attest committed public source identity and canonical Session order, not models. Byte
//! ranges are half-open in the Host's canonical UTF-8 public-entry serialization (including
//! framing, so even an empty message has a nonzero length). Compaction copies must retain the
//! original pointer/order/digest; derived summaries are not new sources. Rewind replacement is
//! an identity conflict, never an implicit reset. This module does not fetch or retain history.
//!
//! Each record operation and its exact retry receipt commit together. A separate checkpoint
//! advances only a contiguous, selected prefix backed by saved record/explicit no-change
//! receipts. It cannot use a position update as evidence. Stopping between these boundaries
//! leaves saved records and unreflected input; read receipts before retrying, even across reopen
//! or replacement Jobs. No whole-Job result submission is required. The Host checks live Job
//! execution leases before issuing contexts, and durably closes a generation on cancellation.
//!
//! Selected ranges describe concrete public bytes supplied to generation, not a preview's
//! enclosing history range or a permission to claim unobserved bytes. The Host verifies canonical
//! serialization, SHA-256 identity and UTF-8 slice boundaries before admission/selection. This
//! store validates identities, byte bounds and coverage but cannot attest text it does not fetch.
//! Availability is the latest Host attestation; readers must not equate it with a live fetch.
//! Public text and metadata reads are paged. Full generation/resume snapshots are Host-only
//! construction material bounded by per-field storage caps (three text fields plus current position),
//! not model-facing reads. A restored snapshot never restores an external Job lease.
//!
//! A T-734 caller saves `Create`/`Rewrite`/`Fix` under stable request IDs, carries each returned
//! `state_token` into the next operation, optionally saves `UpdatePosition`, then submits
//! `Checkpoint { record_operation_ids }` for just the reflected prefix. After a lost response it
//! queries the original receipt, status and coverage pages before submitting any new create.
//! On restart it reads the active generation and current resume state, verifies the external Job
//! binding, and continues missing operations; cancellation closes the generation before replacement.
//! A replacement may checkpoint old saved record receipts only for identical selected source bytes.
//! Tokens incorporate prior saved content conditions as well as current content, preventing A→B→A
//! edits from resurrecting an old operation premise after intervening saved effects.

mod store;
#[cfg(test)]
mod tests;

use crate::{SubjectSessionAttribution, SubjektivStore, api::JobAttemptBinding};
use feature_storage::FeatureStorageError;
use rusqlite::Transaction;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub type JournalId = String;
pub type JournalEntryId = String;
pub type JournalResult<T> = Result<T, JournalError>;
pub const MAX_JOURNAL_TEXT_BYTES: usize = 1024 * 1024;
pub const MAX_JOURNAL_RANGES: usize = 64;
pub const MAX_JOURNAL_PAGE: usize = 32;
pub const MAX_JOURNAL_READ_BYTES: usize = 16 * 1024;
/// Whole pretty-serialized public page budget, including escaped text and metadata.
pub const MAX_JOURNAL_RESPONSE_BYTES: usize = 64 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum JournalError {
    #[error("journal permission denied: {0}")]
    PermissionDenied(String),
    #[error("journal conflict: {0}")]
    Conflict(String),
    #[error("invalid journal input: {0}")]
    InvalidInput(String),
    #[error("journal record not found: {0}")]
    NotFound(String),
    #[error(transparent)]
    Storage(#[from] FeatureStorageError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Subject(#[from] crate::SubjektivError),
}
impl From<rusqlite::Error> for JournalError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Storage(e.into())
    }
}

/// Host-issued in-process authority, never a model request DTO. Reader contexts need no live Session.
#[derive(Debug, Clone)]
pub struct JournalHostContext {
    scope_id: String,
    subject_id: String,
    attribution: Option<SubjectSessionAttribution>,
}
impl JournalHostContext {
    pub fn validated_reader(store: &SubjektivStore, subject_id: &str) -> JournalResult<Self> {
        store
            .subject(subject_id)?
            .ok_or_else(|| JournalError::NotFound(subject_id.into()))?;
        Ok(Self {
            scope_id: store.scope_id().into(),
            subject_id: subject_id.into(),
            attribution: None,
        })
    }
    /// External authentication, public-history validation and execution authority are Host duties.
    pub fn validated_session(
        store: &SubjektivStore,
        attribution: SubjectSessionAttribution,
    ) -> JournalResult<Self> {
        if store.session_attribution(&attribution.session_id)?.as_ref() != Some(&attribution) {
            return Err(JournalError::PermissionDenied(
                "immutable Session attribution mismatch".into(),
            ));
        }
        let mut context = Self::validated_reader(store, &attribution.subject_id)?;
        context.attribution = Some(attribution);
        Ok(context)
    }
}

/// A restricted Journal generation, not Memory consolidation authority.
#[derive(Debug, Clone)]
pub struct JournalJobContext {
    host: JournalHostContext,
    generation_id: String,
    binding: JobAttemptBinding,
}
impl JournalJobContext {
    pub fn validated(
        store: &SubjektivStore,
        host: &JournalHostContext,
        generation_id: &str,
        binding: JobAttemptBinding,
    ) -> JournalResult<Self> {
        let context = Self {
            host: host.clone(),
            generation_id: generation_id.into(),
            binding,
        };
        store.validate_journal_job(&context)?;
        Ok(context)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JournalSourcePointer {
    pub segment_id: Uuid,
    /// Serialized public SessionEntryRef, including its `E` prefix.
    pub entry_ref: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JournalSource {
    pub session_id: String,
    pub pointer: JournalSourcePointer,
    /// Host-attested canonical public entry order across Segments, starting at zero.
    pub order: u64,
    pub content_sha256: String,
    pub total_bytes: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JournalByteRange {
    pub start: u64,
    pub end: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JournalSourceRange {
    pub source: JournalSource,
    pub bytes: JournalByteRange,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JournalEntryState {
    Open,
    Fixed,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Journal {
    pub id: JournalId,
    pub subject_id: String,
    pub source_session_id: String,
    pub title: String,
    pub current_position: String,
    /// Processing cursor is independent of every fixed event endpoint.
    pub process_order: u64,
    pub process_byte: u64,
    /// Content condition advanced by committed operations, never by acceptance notifications.
    pub state_token: String,
    pub created_at: String,
    pub updated_at: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalMetadata {
    pub id: JournalId,
    pub subject_id: String,
    pub source_session_id: String,
    pub title: String,
    pub current_position_bytes: usize,
    pub process_order: u64,
    pub process_byte: u64,
    pub state_token: String,
    pub created_at: String,
    pub updated_at: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalEntry {
    pub id: JournalEntryId,
    pub journal_id: JournalId,
    pub position: u64,
    pub state: JournalEntryState,
    pub title: String,
    pub body: String,
    pub ongoing: String,
    pub source_end: Option<JournalSourcePointer>,
    pub created_at: String,
    pub updated_at: String,
    pub fixed_at: Option<String>,
    pub correction_refs: Vec<JournalEntryId>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalGeneration {
    pub id: String,
    pub journal_id: JournalId,
    pub job: JobAttemptBinding,
    pub selected_ranges: Vec<JournalSourceRange>,
    pub initial_journal: Journal,
    pub initial_open: Option<JournalEntry>,
    pub previous_fixed_end: Option<JournalSourcePointer>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum JournalAction {
    Create {
        title: String,
        body: String,
        ongoing: String,
        state: JournalEntryState,
        source_end: Option<JournalSourcePointer>,
        correction_refs: Vec<JournalEntryId>,
    },
    Rewrite {
        entry_id: JournalEntryId,
        title: String,
        body: String,
        ongoing: String,
    },
    Fix {
        entry_id: JournalEntryId,
        source_end: JournalSourcePointer,
    },
    UpdatePosition {
        current_position: String,
    },
    /// Explicitly records the caller's checked conclusion that no entry change is needed.
    ConfirmUnchanged,
    /// Links durable record/no-change receipts to actual reflected input; IDs are Journal scoped.
    Checkpoint {
        record_operation_ids: Vec<String>,
    },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JournalOperation {
    pub request_id: String,
    pub expected_state: String,
    pub action: JournalAction,
    pub covered_ranges: Vec<JournalSourceRange>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalReceipt {
    pub request_id: String,
    pub generation_id: String,
    pub entry_id: Option<JournalEntryId>,
    pub state_token: String,
    pub process_order: u64,
    pub process_byte: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalSavedOperation {
    pub operation: JournalOperation,
    pub receipt: JournalReceipt,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JournalOperationKind {
    Create,
    Rewrite,
    Fix,
    UpdatePosition,
    ConfirmUnchanged,
    Checkpoint,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalOperationStatus {
    pub receipt: JournalReceipt,
    pub kind: JournalOperationKind,
    pub covered_range_count: usize,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalSourceStatus {
    pub source: JournalSource,
    pub accepted: Vec<JournalByteRange>,
    pub unavailable: Vec<JournalByteRange>,
    pub reflected_bytes: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalPage<T> {
    pub items: Vec<T>,
    pub next: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalEntrySummary {
    pub id: JournalEntryId,
    pub journal_id: JournalId,
    pub position: u64,
    pub state: JournalEntryState,
    pub title: String,
    pub source_end: Option<JournalSourcePointer>,
    /// Exclusive start; None means Session start. The end is inclusive.
    pub previous_fixed_end: Option<JournalSourcePointer>,
    pub source_available: bool,
    pub correction_refs: Vec<JournalEntryId>,
    pub content_token: String,
    pub created_at: String,
    pub updated_at: String,
    pub fixed_at: Option<String>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JournalTextField {
    Body,
    Ongoing,
    CurrentPosition,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalTextPage {
    pub text: String,
    pub next_byte: Option<usize>,
    pub content_token: String,
}

pub(super) fn migrate(tx: &Transaction<'_>) -> feature_storage::Result<()> {
    tx.execute_batch(include_str!("schema.sql"))?;
    Ok(())
}
