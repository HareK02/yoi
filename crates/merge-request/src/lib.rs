#![doc = include_str!("../README.md")]

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use std::{
    path::Path,
    sync::{Arc, Mutex},
};
use thiserror::Error;
use uuid::Uuid;

const SCHEMA_VERSION: i64 = 16;
const MAX_BODY_BYTES: usize = 16 * 1024;
const DOMAIN_TABLES: [&str; 5] = [
    "merge_requests",
    "merge_request_ticket_relations",
    "merge_request_thread_events",
    "merge_request_review_grants",
    "merge_request_reviewer_child_sessions",
];

mod lifecycle;
pub use lifecycle::*;
mod ownership;
pub use ownership::*;
mod publication;
pub use publication::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReviewDecision {
    Approve,
    RequestChanges,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FindingSeverity {
    Blocker,
    Major,
    Minor,
    Note,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ReviewFinding {
    pub severity: FindingSeverity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    pub body: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WorkerIdentity {
    pub runtime_id: String,
    pub worker_id: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct MergeRequestReviewSubject {
    pub merge_request_id: String,
    pub subject_ref: String,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeRequestAuth {
    pub workspace_id: String,
    pub repository_id: String,
    pub runtime_id: String,
    pub worker_id: String,
    /// Ticket assignment for open/legacy Complete; MR Worker operation ID for review actions.
    pub assignment_id: String,
}

impl MergeRequestAuth {
    fn actor(&self) -> WorkerIdentity {
        WorkerIdentity {
            runtime_id: self.runtime_id.clone(),
            worker_id: self.worker_id.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ReviewRequestedEvent {
    pub event_id: String,
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_u64))]
    pub sequence: u64,
    pub subject_ref: String,
    #[serde(default)]
    pub ticket_content_digest: String,
    #[serde(default)]
    pub ticket_merge_request_subjects: Vec<MergeRequestReviewSubject>,
    pub requested_by: WorkerIdentity,
    pub reviewer: WorkerIdentity,
    pub created_at: DateTime<Utc>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ReviewEvent {
    pub event_id: String,
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_u64))]
    pub sequence: u64,
    pub request_event_id: String,
    pub subject_ref: String,
    #[serde(default)]
    pub ticket_content_digest: String,
    #[serde(default)]
    pub ticket_merge_request_subjects: Vec<MergeRequestReviewSubject>,
    pub decision: ReviewDecision,
    pub body: String,
    pub findings: Vec<ReviewFinding>,
    pub reviewer: WorkerIdentity,
    pub created_at: DateTime<Utc>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ReviewRevokedEvent {
    pub event_id: String,
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_u64))]
    pub sequence: u64,
    pub review_event_id: String,
    pub subject_ref: String,
    pub reason: String,
    pub revoked_by: WorkerIdentity,
    pub created_at: DateTime<Utc>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ReviewCancelledEvent {
    pub event_id: String,
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_u64))]
    pub sequence: u64,
    pub request_event_id: String,
    pub subject_ref: String,
    pub reason: String,
    pub created_at: DateTime<Utc>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CommentEvent {
    #[serde(default)]
    pub operation_id: Option<String>,
    #[serde(default)]
    pub owner_operation_id: Option<String>,
    pub event_id: String,
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_u64))]
    pub sequence: u64,
    pub body: String,
    pub author: WorkerIdentity,
    pub created_at: DateTime<Utc>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MergeStrategy {
    FastForward,
    Merge,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConflictResolution {
    None,
    Clean,
    ConflictsResolved,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct MergeEvent {
    pub event_id: String,
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_u64))]
    pub sequence: u64,
    pub operation_id: String,
    pub approval_event_id: String,
    pub approved_source_ref: String,
    pub target_ref_before: String,
    pub target_ref_after: String,
    pub strategy: MergeStrategy,
    pub resolution: ConflictResolution,
    pub merged_by: WorkerIdentity,
    pub created_at: DateTime<Utc>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MergeRequestThreadEvent {
    ReviewRequested(ReviewRequestedEvent),
    Review(ReviewEvent),
    ReviewRevoked(ReviewRevokedEvent),
    ReviewCancelled(ReviewCancelledEvent),
    Comment(CommentEvent),
    Merge(MergeEvent),
    WorkerAssigned(WorkerAssignedEvent),
}
impl MergeRequestThreadEvent {
    pub fn sequence(&self) -> u64 {
        match self {
            Self::ReviewRequested(v) => v.sequence,
            Self::Review(v) => v.sequence,
            Self::ReviewRevoked(v) => v.sequence,
            Self::ReviewCancelled(v) => v.sequence,
            Self::Comment(v) => v.sequence,
            Self::Merge(v) => v.sequence,
            Self::WorkerAssigned(v) => v.sequence,
        }
    }
    fn bound_bodies(&mut self) {
        match self {
            Self::Review(_) => {}
            Self::ReviewRevoked(value) => truncate_body(&mut value.reason),
            Self::ReviewCancelled(value) => truncate_body(&mut value.reason),
            Self::Comment(value) => truncate_body(&mut value.body),
            Self::ReviewRequested(_) | Self::Merge(_) | Self::WorkerAssigned(_) => {}
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MergeRequest {
    pub workspace_id: String,
    pub merge_request_id: String,
    pub repository_id: String,
    pub state: MergeRequestState,
    pub selector_from: Option<String>,
    pub selector_to: String,
    pub ticket_ids: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub thread: Vec<MergeRequestThreadEvent>,
}
/// Bounded failures from pure evaluation of persisted review and merge evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeRequestEvidenceError {
    NotMerged,
    MergeResultMissing,
    ApprovalMissing,
    ApprovalRevoked,
    ApprovalNotApproved,
    ApprovalSourceMismatch,
    ApprovalTimelineMismatch,
    ReviewRequestMissing,
    ReviewRequestMismatch,
    ApprovalNotEffective,
    TicketContentMismatch,
    SourceSnapshotMismatch,
    RequirementApprovalMissing,
}

impl MergeRequestEvidenceError {
    /// Stable machine-readable diagnostic code, separate from the human message.
    pub fn code(self) -> &'static str {
        match self {
            Self::NotMerged => "not_merged",
            Self::MergeResultMissing => "merge_result_missing",
            Self::ApprovalMissing => "approval_missing",
            Self::ApprovalRevoked => "approval_revoked",
            Self::ApprovalNotApproved => "approval_not_approved",
            Self::ApprovalSourceMismatch => "approval_source_mismatch",
            Self::ApprovalTimelineMismatch => "approval_timeline_mismatch",
            Self::ReviewRequestMissing => "review_request_missing",
            Self::ReviewRequestMismatch => "review_request_mismatch",
            Self::ApprovalNotEffective => "approval_not_effective",
            Self::TicketContentMismatch => "ticket_content_mismatch",
            Self::SourceSnapshotMismatch => "source_snapshot_mismatch",
            Self::RequirementApprovalMissing => "requirement_approval_missing",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotMerged => "Merge Request has no merged result",
            Self::MergeResultMissing => "merged Merge Request has no MergeResult",
            Self::ApprovalMissing => "integration approval is missing",
            Self::ApprovalRevoked => "Reviewer approval is revoked",
            Self::ApprovalNotApproved => "Reviewer decision is not an approval",
            Self::ApprovalSourceMismatch => "Reviewer approval does not match the exact source ref",
            Self::ApprovalTimelineMismatch => {
                "integration approval does not predate the merged result"
            }
            Self::ReviewRequestMissing => "linked ReviewRequested evidence is missing",
            Self::ReviewRequestMismatch => {
                "Reviewer approval does not match linked ReviewRequested evidence"
            }
            Self::ApprovalNotEffective => "Reviewer approval is not the effective review",
            Self::TicketContentMismatch => {
                "Reviewer approval does not attest the current Ticket digest"
            }
            Self::SourceSnapshotMismatch => {
                "Reviewer approval does not attest the exact linked source set"
            }
            Self::RequirementApprovalMissing => {
                "no effective Reviewer approval attests the current Ticket digest and exact linked source set"
            }
        }
    }
}

impl std::fmt::Display for MergeRequestEvidenceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl std::error::Error for MergeRequestEvidenceError {}

impl MergeRequest {
    pub fn phase(&self) -> MergeRequestPhase {
        match self.state {
            MergeRequestState::Implementation {} => MergeRequestPhase::Implementation,
            MergeRequestState::Integration { .. } => MergeRequestPhase::Integration,
            MergeRequestState::Completed { .. } => MergeRequestPhase::Completed,
            MergeRequestState::Canceled { .. } => MergeRequestPhase::Canceled,
        }
    }
    pub fn effective_review(&self, subject: &str) -> Option<&ReviewEvent> {
        self.thread.iter().rev().find_map(|e|match e{MergeRequestThreadEvent::Review(v)if v.subject_ref==subject&&!self.thread.iter().any(|x|matches!(x,MergeRequestThreadEvent::ReviewRevoked(r)if r.review_event_id==v.event_id))=>Some(v),_=>None})
    }

    /// Returns the stored immutable integration result without resolving selectors again.
    /// Pins the approved source and approval event; branch movement/deletion or later requirement reviews never replace them.
    /// Post-integration evidence evaluation uses the pinned source, independent of current branch/provider availability.
    /// Do not infer approval from `merged` state alone; verify evidence with [`Self::integration_approval`].
    pub fn merged_result(&self) -> Result<&MergeEvent, MergeRequestEvidenceError> {
        let merge_event_id = match &self.state {
            MergeRequestState::Completed {
                completion: MergeRequestCompletion::Published { merge_event_id, .. },
            } => merge_event_id,
            MergeRequestState::Completed {
                completion: MergeRequestCompletion::Legacy { merge_event_id },
            } => merge_event_id
                .as_ref()
                .ok_or(MergeRequestEvidenceError::MergeResultMissing)?,
            _ => return Err(MergeRequestEvidenceError::NotMerged),
        };
        self.thread
            .iter()
            .rev()
            .find_map(|event| match event {
                MergeRequestThreadEvent::Merge(event) if &event.event_id == merge_event_id => {
                    Some(event)
                }
                _ => None,
            })
            .ok_or(MergeRequestEvidenceError::MergeResultMissing)
    }

    /// Verifies source equality, ordering, non-revocation, and the matching review request for the approval event named by the integration result.
    /// Later requirement approval does not replace integration-time approval. Validation requires the full thread;
    /// a merge event in a page alone cannot prove approval exists.
    pub fn integration_approval(&self) -> Result<&ReviewEvent, MergeRequestEvidenceError> {
        let merge = self.merged_result()?;
        let review = self
            .thread
            .iter()
            .find_map(|event| match event {
                MergeRequestThreadEvent::Review(review)
                    if review.event_id == merge.approval_event_id =>
                {
                    Some(review)
                }
                _ => None,
            })
            .ok_or(MergeRequestEvidenceError::ApprovalMissing)?;
        if merge.approved_source_ref.trim().is_empty()
            || review.subject_ref != merge.approved_source_ref
        {
            return Err(MergeRequestEvidenceError::ApprovalSourceMismatch);
        }
        if review.sequence >= merge.sequence {
            return Err(MergeRequestEvidenceError::ApprovalTimelineMismatch);
        }
        self.approval_evidence(review)?;
        Ok(review)
    }

    fn approval_evidence(&self, review: &ReviewEvent) -> Result<(), MergeRequestEvidenceError> {
        if self.thread.iter().any(|event| {
            matches!(event, MergeRequestThreadEvent::ReviewRevoked(revoked)
                if revoked.review_event_id == review.event_id)
        }) {
            return Err(MergeRequestEvidenceError::ApprovalRevoked);
        }
        if review.decision != ReviewDecision::Approve {
            return Err(MergeRequestEvidenceError::ApprovalNotApproved);
        }
        let requested = self
            .thread
            .iter()
            .find_map(|event| match event {
                MergeRequestThreadEvent::ReviewRequested(requested)
                    if requested.event_id == review.request_event_id =>
                {
                    Some(requested)
                }
                _ => None,
            })
            .ok_or(MergeRequestEvidenceError::ReviewRequestMissing)?;
        if requested.subject_ref != review.subject_ref
            || requested.ticket_content_digest != review.ticket_content_digest
            || requested.ticket_merge_request_subjects != review.ticket_merge_request_subjects
            || requested.reviewer != review.reviewer
            || requested.sequence >= review.sequence
        {
            return Err(MergeRequestEvidenceError::ReviewRequestMismatch);
        }
        Ok(())
    }
}

/// Finds a valid review approving the current Ticket digest and the exact set of all related MR/sources.
/// The source of the MR holding the review and its corresponding ReviewRequested must also match.
/// Comments, recent timestamps, or one representative MR cannot substitute for approval of all requirements.
///
/// The caller supplies all MRs and observed sources, using stored sources for integrated MRs.
/// Does not resolve selectors, require merged state, or authorize/change Ticket completion.
/// A post-integration review may approve a new requirement set while integration results remain immutable.
/// With an event ID, validates only that approval; otherwise searches for a valid approval.
/// Set order does not matter, but duplicate IDs are rejected.
pub fn requirement_approval<'a>(
    requests: &'a [MergeRequest],
    content_digest: &str,
    subjects: &[MergeRequestReviewSubject],
    event_id: Option<&str>,
) -> Result<&'a ReviewEvent, MergeRequestEvidenceError> {
    if requests.len() != subjects.len()
        || !same_source_snapshot(subjects, subjects)
        || requests.iter().any(|request| {
            requests
                .iter()
                .filter(|other| other.merge_request_id == request.merge_request_id)
                .count()
                != 1
                || !subjects
                    .iter()
                    .any(|subject| subject.merge_request_id == request.merge_request_id)
        })
    {
        return Err(MergeRequestEvidenceError::SourceSnapshotMismatch);
    }
    for request in requests {
        for event in request.thread.iter().rev() {
            let MergeRequestThreadEvent::Review(review) = event else {
                continue;
            };
            if event_id.is_some_and(|id| id != review.event_id) {
                continue;
            }
            let evidence = (|| {
                request.approval_evidence(review)?;
                if !request
                    .effective_review(&review.subject_ref)
                    .is_some_and(|effective| effective.event_id == review.event_id)
                {
                    return Err(MergeRequestEvidenceError::ApprovalNotEffective);
                }
                if review.ticket_content_digest != content_digest
                    || content_digest.trim().is_empty()
                {
                    return Err(MergeRequestEvidenceError::TicketContentMismatch);
                }
                if !same_source_snapshot(&review.ticket_merge_request_subjects, subjects) {
                    return Err(MergeRequestEvidenceError::SourceSnapshotMismatch);
                }
                if !subjects.iter().any(|subject| {
                    subject.merge_request_id == request.merge_request_id
                        && subject.subject_ref == review.subject_ref
                }) {
                    return Err(MergeRequestEvidenceError::ApprovalSourceMismatch);
                }
                Ok(review)
            })();
            if evidence.is_ok() || event_id.is_some() {
                return evidence;
            }
        }
    }
    Err(MergeRequestEvidenceError::RequirementApprovalMissing)
}

fn same_source_snapshot(
    left: &[MergeRequestReviewSubject],
    right: &[MergeRequestReviewSubject],
) -> bool {
    !left.is_empty()
        && left.len() == right.len()
        && left.iter().all(|subject| {
            !subject.merge_request_id.trim().is_empty()
                && !subject.subject_ref.trim().is_empty()
                && left
                    .iter()
                    .filter(|other| other.merge_request_id == subject.merge_request_id)
                    .count()
                    == 1
                && right.iter().filter(|other| *other == subject).count() == 1
        })
}

#[derive(Clone, Debug, Default)]
pub struct MergeRequestListQuery {
    pub state: Option<MergeRequestStateFilter>,
    pub repository_id: Option<String>,
    pub ticket_id: Option<String>,
    pub selector_from: Option<String>,
    pub selector_to: Option<String>,
    pub cursor: Option<String>,
    pub limit: usize,
}

#[derive(Clone, Debug)]
pub struct MergeRequestListPage {
    pub items: Vec<MergeRequest>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone)]
pub struct OpenMergeRequest {
    pub merge_request_id: String,
    pub ticket_id: String,
    pub repository_id: String,
    pub selector_from: String,
    pub selector_to: String,
    pub summary: String,
    pub auth: MergeRequestAuth,
    pub now: DateTime<Utc>,
}
#[derive(Debug, Clone)]
pub struct RequestMergeRequestReview {
    pub merge_request_id: String,
    pub ticket_id: String,
    pub ticket_content_digest: String,
    pub ticket_merge_request_subjects: Vec<MergeRequestReviewSubject>,
    pub subject_ref: String,
    pub child_session_id: String,
    pub capability_token: String,
    pub auth: MergeRequestAuth,
    pub now: DateTime<Utc>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestedMergeRequestReview {
    pub request_event: ReviewRequestedEvent,
}
#[derive(Debug, Clone)]
pub struct RegisterReviewerChildSession {
    pub workspace_id: String,
    pub parent_runtime_id: String,
    pub parent_worker_id: String,
    pub child_session_id: String,
    pub reviewer_profile: String,
    pub now: DateTime<Utc>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewSubmissionAuthorization {
    pub workspace_id: String,
    pub merge_request_id: String,
    pub ticket_id: String,
    pub subject_ref: String,
}

#[derive(Debug, Clone)]
pub struct SubmitMergeRequestReview {
    pub merge_request_id: String,
    pub ticket_id: String,
    pub current_subject_ref: String,
    pub capability_token: String,
    pub decision: ReviewDecision,
    pub body: String,
    pub findings: Vec<ReviewFinding>,
    pub now: DateTime<Utc>,
}
#[derive(Debug, Clone)]
pub struct RevokeMergeRequestReview {
    pub merge_request_id: String,
    pub ticket_id: String,
    pub review_event_id: String,
    pub reason: String,
    pub auth: MergeRequestAuth,
    pub now: DateTime<Utc>,
}
#[derive(Debug, Clone)]
pub struct RepairSelectorFrom {
    pub workspace_id: String,
    pub merge_request_id: String,
    pub ticket_id: String,
    pub selector_from: String,
    pub resolved_subject_ref: String,
    pub repaired_by: WorkerIdentity,
    pub reason: String,
    pub now: DateTime<Utc>,
}
#[derive(Debug, Clone)]
pub struct ReadinessCheck {
    pub merge_request_id: String,
    pub ticket_id: String,
    pub current_subject_ref: Option<String>,
    pub auth: MergeRequestAuth,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ReadinessReport {
    pub ready: bool,
    pub blockers: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review: Option<ReviewEvent>,
}
#[derive(Debug, Clone)]
pub struct CompleteMergeRequest {
    pub merge_request_id: String,
    pub ticket_id: String,
    pub operation_id: String,
    pub approval_event_id: String,
    pub current_subject_ref: String,
    pub target_ref_before: String,
    pub target_ref_after: String,
    pub strategy: MergeStrategy,
    pub resolution: ConflictResolution,
    pub auth: MergeRequestAuth,
    pub now: DateTime<Utc>,
}
/// Historical MR-owned Ticket completion payload, retained for reading existing events.
/// Current close/reopen/correction judgments belong to the Ticket domain and require no
/// universal MR evidence gate. This archived payload is neither a current case operation nor
/// authority to infer closure, self-reported handling, work release, or Worker termination.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TicketCompletionEvent {
    pub operation_id: String,
    pub ticket_id: String,
    /// Frozen historical marker, read only to preserve archived completion payloads.
    /// It is never checked for readiness, content attestation, or live completion.
    #[serde(rename = "item_revision")]
    pub legacy_item_marker: String,
    pub merge_request_ids: Vec<String>,
    pub requirement_approval_event_id: String,
    pub completed_by: WorkerIdentity,
    pub created_at: DateTime<Utc>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurrentAssignment {
    pub assignment_id: String,
    pub ticket_id: String,
    pub runtime_id: String,
    pub worker_id: String,
}
pub trait AssignmentSource: Send + Sync {
    fn current_assignment(
        &self,
        workspace_id: &str,
        ticket_id: &str,
    ) -> Result<Option<CurrentAssignment>, String>;
}
pub trait RepositorySource: Send + Sync {
    fn repository_belongs_to_workspace(
        &self,
        workspace_id: &str,
        repository_id: &str,
    ) -> Result<bool, String>;
}
#[derive(Debug, Error)]
pub enum MergeRequestError {
    #[error("Merge Request not found")]
    NotFound,
    #[error("Merge Request conflict: {0}")]
    Conflict(String),
    #[error("Merge Request unauthorized: {0}")]
    Unauthorized(String),
    #[error("Merge Request is not ready: {0}")]
    NotReady(String),
    #[error("Merge Request validation failed: {0}")]
    Validation(String),
    #[error("Merge Request operation failed: {0}")]
    Operation(String),
    #[error("Merge Request storage is corrupt: {0}")]
    Corrupt(String),
    #[error("Merge Request storage error: {0}")]
    Storage(#[from] rusqlite::Error),
}

/// SQLite storage for Workspace/Repository-scoped MR records and thread events.
/// External authority owns Git operations and source observation; this store checks supplied results,
/// current assignments, and review evidence. Ticket completion decisions are not this store's responsibility.
pub struct MergeRequestStore {
    conn: Arc<Mutex<Connection>>,
    assignments: Arc<dyn AssignmentSource>,
    repositories: Arc<dyn RepositorySource>,
}
impl MergeRequestStore {
    pub fn open(
        path: impl AsRef<Path>,
        assignments: Arc<dyn AssignmentSource>,
        repositories: Arc<dyn RepositorySource>,
    ) -> Result<Self, MergeRequestError> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        migrate(&conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            assignments,
            repositories,
        })
    }
    pub fn open_merge_request(
        &self,
        i: OpenMergeRequest,
    ) -> Result<MergeRequest, MergeRequestError> {
        self.open_merge_request_with_evidence(i, |_| Ok(()))
    }

    /// Retain caller-owned evidence atomically with creation. The callback runs
    /// after the MR and Ticket relation exist, under this store's transaction;
    /// it must use that transaction rather than reacquire the store connection.
    pub fn open_merge_request_with_evidence(
        &self,
        i: OpenMergeRequest,
        retain_evidence: impl FnOnce(&Transaction<'_>) -> Result<(), MergeRequestError>,
    ) -> Result<MergeRequest, MergeRequestError> {
        bounded_body("summary", &i.summary)?;
        for (n, v) in [
            ("merge_request_id", i.merge_request_id.as_str()),
            ("ticket_id", &i.ticket_id),
            ("repository_id", &i.repository_id),
            ("selector_from", &i.selector_from),
            ("selector_to", &i.selector_to),
        ] {
            nonempty(n, v)?
        }
        if i.selector_from == i.selector_to {
            return Err(MergeRequestError::Validation(
                "selectors must differ".into(),
            ));
        }
        let merge_request_id = i.merge_request_id.clone();
        self.assigned(&i.auth, &i.ticket_id, &i.repository_id)?;
        let mut c = self.lock()?;
        // Acquire the writer before reading the open-MR conflict snapshot. A
        // deferred read-to-write upgrade can fail immediately with SQLITE_BUSY
        // while normal Subject history/Job writers use the same database.
        let t = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let conflict:bool=t.query_row("SELECT EXISTS(SELECT 1 FROM merge_request_ticket_relations rel JOIN merge_requests mr ON mr.workspace_id=rel.workspace_id AND mr.merge_request_id=rel.merge_request_id WHERE rel.workspace_id=?1 AND rel.ticket_id=?2 AND mr.repository_id=?3 AND json_extract(mr.state,'$.kind') IN ('implementation','integration'))",params![i.auth.workspace_id,i.ticket_id,i.repository_id],|r|r.get(0))?;
        if conflict {
            return Err(MergeRequestError::Conflict(
                "Ticket already has an open Merge Request for this repository; advance that Merge Request's existing selector_from with a normal non-force push instead of opening a replacement".into(),
            ));
        }
        let now = i.now.to_rfc3339();
        t.execute(
            "INSERT INTO merge_requests(workspace_id,merge_request_id,repository_id,state,selector_from,selector_to,created_at,updated_at) VALUES(?1,?2,?3,json_object('kind','implementation'),?4,?5,?6,?6)",
            params![
                i.auth.workspace_id,
                i.merge_request_id,
                i.repository_id,
                i.selector_from,
                i.selector_to,
                now
            ],
        )?;
        t.execute(
            "INSERT INTO merge_request_ticket_relations VALUES(?1,?2,?3,'implements',?4)",
            params![i.auth.workspace_id, i.merge_request_id, i.ticket_id, now],
        )?;
        let owner = WorkerAssignedEvent {
            event_id: Uuid::now_v7().to_string(),
            sequence: next_seq(&t, &i.auth.workspace_id, &i.merge_request_id)?,
            operation_id: Uuid::now_v7().to_string(),
            previous_operation_id: None,
            worker: Some(i.auth.actor()),
            assigned_by: i.auth.actor(),
            created_at: i.now,
        };
        insert_event(
            &t,
            &i.auth.workspace_id,
            &i.merge_request_id,
            "worker_assigned",
            &owner,
            i.now,
            Some(&owner.operation_id),
        )?;
        if !i.summary.trim().is_empty() {
            let e = CommentEvent {
                operation_id: None,
                owner_operation_id: None,
                event_id: Uuid::now_v7().to_string(),
                sequence: next_seq(&t, &i.auth.workspace_id, &i.merge_request_id)?,
                body: i.summary,
                author: WorkerIdentity {
                    runtime_id: i.auth.runtime_id,
                    worker_id: i.auth.worker_id,
                },
                created_at: i.now,
            };
            insert_event(
                &t,
                &i.auth.workspace_id,
                &i.merge_request_id,
                "comment",
                &e,
                i.now,
                None,
            )?
        }
        retain_evidence(&t)?;
        t.commit()?;
        drop(c);
        self.get_by_id(&i.auth.workspace_id, &merge_request_id)
    }
    pub fn register_reviewer_child_session(
        &self,
        i: RegisterReviewerChildSession,
    ) -> Result<(), MergeRequestError> {
        if i.reviewer_profile != "builtin:reviewer" {
            return Err(MergeRequestError::Unauthorized(
                "reviewer profile mismatch".into(),
            ));
        }
        self.lock()?.execute(
            "INSERT INTO merge_request_reviewer_child_sessions VALUES(?1,?2,?3,?4,?5,?6,'active')",
            params![
                i.workspace_id,
                i.child_session_id,
                i.parent_runtime_id,
                i.parent_worker_id,
                i.reviewer_profile,
                i.now.to_rfc3339()
            ],
        )?;
        Ok(())
    }
    pub fn request_review(
        &self,
        mut i: RequestMergeRequestReview,
    ) -> Result<RequestedMergeRequestReview, MergeRequestError> {
        nonempty("subject_ref", &i.subject_ref)?;
        if i.ticket_merge_request_subjects.is_empty() {
            return Err(MergeRequestError::Validation(
                "review must bind the linked Merge Request subject set".into(),
            ));
        }
        for subject in &i.ticket_merge_request_subjects {
            nonempty("merge_request_id", &subject.merge_request_id)?;
            nonempty("subject_ref", &subject.subject_ref)?;
        }
        i.ticket_merge_request_subjects
            .sort_by(|left, right| left.merge_request_id.cmp(&right.merge_request_id));
        if i.ticket_merge_request_subjects
            .windows(2)
            .any(|subjects| subjects[0].merge_request_id == subjects[1].merge_request_id)
        {
            return Err(MergeRequestError::Validation(
                "review Merge Request subject set must not contain duplicates".into(),
            ));
        }
        if !i.ticket_merge_request_subjects.iter().any(|subject| {
            subject.merge_request_id == i.merge_request_id && subject.subject_ref == i.subject_ref
        }) {
            return Err(MergeRequestError::Validation(
                "review subject set does not contain the addressed Merge Request subject".into(),
            ));
        }
        let mr = self.get_for_operation(&i.auth.workspace_id, &i.merge_request_id, &i.ticket_id)?;
        self.current_worker_auth(&i.auth, &mr)?;
        if mr.state.is_canceled() || (mr.state.is_open() && mr.selector_from.is_none()) {
            return Err(MergeRequestError::Conflict(
                "Merge Request is not reviewable".into(),
            ));
        }
        let mut c = self.lock()?;
        let t = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let current_digest = current_ticket_content_digest(&t, &i.auth.workspace_id, &i.ticket_id)?
            .ok_or(MergeRequestError::NotFound)?;
        if current_digest != i.ticket_content_digest {
            return Err(MergeRequestError::Conflict(
                "Ticket item digest changed before review request".into(),
            ));
        }
        let current_mr = load_mr(&t, &i.auth.workspace_id, &i.merge_request_id)?
            .filter(|request| request.ticket_ids.iter().any(|id| id == &i.ticket_id))
            .ok_or(MergeRequestError::NotFound)?;
        self.current_worker_auth(&i.auth, &current_mr)?;
        if current_mr.state.is_canceled()
            || (current_mr.state.is_open() && current_mr.selector_from.is_none())
        {
            return Err(MergeRequestError::Conflict(
                "Merge Request changed before review request".into(),
            ));
        }
        let linked_ids = linked_merge_request_ids(&t, &i.auth.workspace_id, &i.ticket_id)?;
        let subject_ids = i
            .ticket_merge_request_subjects
            .iter()
            .map(|subject| subject.merge_request_id.clone())
            .collect::<Vec<_>>();
        if linked_ids != subject_ids {
            return Err(MergeRequestError::Conflict(
                "linked Merge Request set changed before review request".into(),
            ));
        }
        let child:Option<(String,String,String)>=t.query_row("SELECT parent_runtime_id,child_session_id,reviewer_profile FROM merge_request_reviewer_child_sessions WHERE workspace_id=?1 AND child_session_id=?2 AND parent_runtime_id=?3 AND parent_worker_id=?4 AND status='active'",params![i.auth.workspace_id,i.child_session_id,i.auth.runtime_id,i.auth.worker_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
        let Some((rr, rw, p)) = child else {
            return Err(MergeRequestError::Unauthorized(
                "reviewer child attestation missing".into(),
            ));
        };
        if p != "builtin:reviewer" {
            return Err(MergeRequestError::Unauthorized(
                "reviewer profile mismatch".into(),
            ));
        }
        let e = ReviewRequestedEvent {
            event_id: Uuid::now_v7().to_string(),
            sequence: next_seq(&t, &mr.workspace_id, &mr.merge_request_id)?,
            subject_ref: i.subject_ref,
            ticket_content_digest: i.ticket_content_digest,
            ticket_merge_request_subjects: i.ticket_merge_request_subjects,
            requested_by: WorkerIdentity {
                runtime_id: i.auth.runtime_id,
                worker_id: i.auth.worker_id,
            },
            reviewer: WorkerIdentity {
                runtime_id: rr.clone(),
                worker_id: rw.clone(),
            },
            created_at: i.now,
        };
        insert_event(
            &t,
            &mr.workspace_id,
            &mr.merge_request_id,
            "review_requested",
            &e,
            i.now,
            None,
        )?;
        t.execute("INSERT INTO merge_request_review_grants VALUES(?1,?2,?3,?4,?5,?6,?7,?8,NULL,NULL,'issued')",params![mr.workspace_id,mr.merge_request_id,e.event_id,e.subject_ref,rr,rw,i.capability_token,i.now.to_rfc3339()])?;
        t.execute("UPDATE merge_request_reviewer_child_sessions SET status='consumed' WHERE workspace_id=?1 AND child_session_id=?2",params![mr.workspace_id,i.child_session_id])?;
        t.commit()?;
        Ok(RequestedMergeRequestReview { request_event: e })
    }
    pub fn authorize_review_submission(
        &self,
        merge_request_id: &str,
        capability_token: &str,
    ) -> Result<ReviewSubmissionAuthorization, MergeRequestError> {
        let connection = self.lock()?;
        connection
            .query_row(
                "SELECT g.workspace_id,g.merge_request_id,rel.ticket_id,g.subject_ref
                   FROM merge_request_review_grants g
                   JOIN merge_request_ticket_relations rel
                     ON rel.workspace_id=g.workspace_id AND rel.merge_request_id=g.merge_request_id
                   JOIN merge_requests mr
                     ON mr.workspace_id=g.workspace_id AND mr.merge_request_id=g.merge_request_id
                  WHERE g.capability_token=?1 AND g.merge_request_id=?2
                    AND g.status='issued' AND json_extract(mr.state,'$.kind') IN ('implementation','integration','completed')",
                params![capability_token, merge_request_id],
                |row| {
                    Ok(ReviewSubmissionAuthorization {
                        workspace_id: row.get(0)?,
                        merge_request_id: row.get(1)?,
                        ticket_id: row.get(2)?,
                        subject_ref: row.get(3)?,
                    })
                },
            )
            .optional()?
            .ok_or_else(|| MergeRequestError::Unauthorized("review grant invalid".into()))
    }

    pub fn submit_review(
        &self,
        i: SubmitMergeRequestReview,
    ) -> Result<ReviewEvent, MergeRequestError> {
        let mut c = self.lock()?;
        let t = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let g: Option<(String, String, String, String, String, String)> = t
            .query_row(
                "SELECT g.workspace_id,g.merge_request_id,g.request_event_id,g.subject_ref,
                        g.reviewer_runtime_id,g.reviewer_worker_id
                   FROM merge_request_review_grants g
                   JOIN merge_request_ticket_relations rel
                     ON rel.workspace_id=g.workspace_id AND rel.merge_request_id=g.merge_request_id
                   JOIN merge_requests mr
                     ON mr.workspace_id=g.workspace_id AND mr.merge_request_id=g.merge_request_id
                  WHERE g.capability_token=?1 AND rel.ticket_id=?2
                    AND g.merge_request_id=?3
                    AND g.status='issued' AND json_extract(mr.state,'$.kind') IN ('implementation','integration','completed')",
                params![i.capability_token, i.ticket_id, i.merge_request_id],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                    ))
                },
            )
            .optional()?;
        let Some((ws, mr, req, subject, rr, rw)) = g else {
            return Err(MergeRequestError::Unauthorized(
                "review grant invalid".into(),
            ));
        };
        let (ticket_content_digest, ticket_merge_request_subjects) = load_mr(&t, &ws, &mr)?
            .and_then(|request| {
                request.thread.into_iter().find_map(|event| match event {
                    MergeRequestThreadEvent::ReviewRequested(event) if event.event_id == req => {
                        Some((
                            event.ticket_content_digest,
                            event.ticket_merge_request_subjects,
                        ))
                    }
                    _ => None,
                })
            })
            .ok_or_else(|| MergeRequestError::Corrupt("review request event missing".into()))?;
        if subject != i.current_subject_ref {
            let reason = format!(
                "selector_from moved from requested subject {subject} to current subject {}; fresh review of the exact current source ref is required",
                i.current_subject_ref
            );
            let e = ReviewCancelledEvent {
                event_id: Uuid::now_v7().to_string(),
                sequence: next_seq(&t, &ws, &mr)?,
                request_event_id: req,
                subject_ref: subject,
                reason,
                created_at: i.now,
            };
            insert_event(&t, &ws, &mr, "review_cancelled", &e, i.now, None)?;
            t.execute("UPDATE merge_request_review_grants SET status='revoked',revoked_at=?2 WHERE capability_token=?1",params![i.capability_token,i.now.to_rfc3339()])?;
            t.commit()?;
            return Err(MergeRequestError::Conflict(
                "selector_from moved; review cancelled".into(),
            ));
        }
        let e = ReviewEvent {
            event_id: Uuid::now_v7().to_string(),
            sequence: next_seq(&t, &ws, &mr)?,
            request_event_id: req,
            subject_ref: subject,
            ticket_content_digest,
            ticket_merge_request_subjects,
            decision: i.decision,
            body: i.body,
            findings: i.findings,
            reviewer: WorkerIdentity {
                runtime_id: rr,
                worker_id: rw,
            },
            created_at: i.now,
        };
        insert_event(&t, &ws, &mr, "review", &e, i.now, None)?;
        t.execute("UPDATE merge_request_review_grants SET status='consumed',consumed_at=?2 WHERE capability_token=?1",params![i.capability_token,i.now.to_rfc3339()])?;
        // Enter once with a valid approval; later revocation retains this history.
        if e.decision == ReviewDecision::Approve
            && current_ticket_content_digest(&t, &ws, &i.ticket_id)?.as_deref()
                == Some(e.ticket_content_digest.as_str())
            && !e.ticket_content_digest.is_empty()
            && linked_merge_request_ids(&t, &ws, &i.ticket_id)?
                == e.ticket_merge_request_subjects
                    .iter()
                    .map(|s| s.merge_request_id.clone())
                    .collect::<Vec<_>>()
        {
            t.execute("UPDATE merge_requests SET state=json_object('kind','integration','started_by',?3),updated_at=?4 WHERE workspace_id=?1 AND merge_request_id=?2 AND json_extract(state,'$.kind')='implementation'",
                params![ws, mr, e.event_id, i.now.to_rfc3339()])?;
        }
        t.commit()?;
        Ok(e)
    }
    pub fn revoke_review(
        &self,
        i: RevokeMergeRequestReview,
    ) -> Result<ReviewRevokedEvent, MergeRequestError> {
        bounded_body("revocation reason", &i.reason)?;
        let mut c = self.lock()?;
        let t = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let mr = load_mr(&t, &i.auth.workspace_id, &i.merge_request_id)?
            .filter(|mr| mr.ticket_ids.iter().any(|id| id == &i.ticket_id))
            .ok_or(MergeRequestError::NotFound)?;
        self.current_worker_auth(&i.auth, &mr)?;
        let r = mr
            .thread
            .iter()
            .find_map(|x| match x {
                MergeRequestThreadEvent::Review(v) if v.event_id == i.review_event_id => Some(v),
                _ => None,
            })
            .ok_or(MergeRequestError::NotFound)?;
        if mr.thread.iter().any(|x|matches!(x,MergeRequestThreadEvent::ReviewRevoked(v)if v.review_event_id==r.event_id)){return Err(MergeRequestError::Conflict("already revoked".into()))}
        let e = ReviewRevokedEvent {
            event_id: Uuid::now_v7().to_string(),
            sequence: next_seq(&t, &mr.workspace_id, &mr.merge_request_id)?,
            review_event_id: r.event_id.clone(),
            subject_ref: r.subject_ref.clone(),
            reason: i.reason,
            revoked_by: WorkerIdentity {
                runtime_id: i.auth.runtime_id,
                worker_id: i.auth.worker_id,
            },
            created_at: i.now,
        };
        insert_event(
            &t,
            &mr.workspace_id,
            &mr.merge_request_id,
            "review_revoked",
            &e,
            i.now,
            None,
        )?;
        t.commit()?;
        Ok(e)
    }
    pub fn readiness(&self, i: ReadinessCheck) -> Result<ReadinessReport, MergeRequestError> {
        let mr = self.get_for_operation(&i.auth.workspace_id, &i.merge_request_id, &i.ticket_id)?;
        let review = i
            .current_subject_ref
            .as_deref()
            .and_then(|s| mr.effective_review(s))
            .cloned();
        let mut b = vec![];
        if !mr.state.is_open() {
            b.push("Merge Request is not open".into())
        }
        if mr.phase() == MergeRequestPhase::Implementation {
            b.push("integration entry is unconfirmed; a fresh valid review is required".into());
        }
        if mr.selector_from.is_none() {
            b.push("selector_from requires repair".into())
        }
        if let Some(review) = &review {
            let connection = self.lock()?;
            let current_digest =
                current_ticket_content_digest(&connection, &i.auth.workspace_id, &i.ticket_id)?
                    .ok_or(MergeRequestError::NotFound)?;
            if review.ticket_content_digest != current_digest {
                b.push(
                    "approval predates the current Ticket digest; fresh review is required".into(),
                );
            }
        }
        match (&i.current_subject_ref, &review) {
            (None, _) => b.push("selector_from could not be resolved".into()),
            (Some(subject_ref), None) => {
                let previous_review_subject = mr.thread.iter().rev().find_map(|event| match event {
                    MergeRequestThreadEvent::ReviewRequested(value) => {
                        Some(value.subject_ref.as_str())
                    }
                    MergeRequestThreadEvent::Review(value) => Some(value.subject_ref.as_str()),
                    _ => None,
                });
                match previous_review_subject.filter(|previous| *previous != subject_ref) {
                    Some(previous) => b.push(format!(
                        "selector_from moved from reviewed/requested subject {previous} to current subject {subject_ref}; request a fresh review for this exact source ref (selector_to movement alone does not invalidate source approval)"
                    )),
                    None => b.push(format!(
                        "current source ref {subject_ref} has no valid review; request a fresh review for this exact source ref"
                    )),
                }
            }
            (Some(subject_ref), Some(r)) if r.decision == ReviewDecision::RequestChanges => {
                b.push(format!(
                    "current source ref {subject_ref} requests changes; advance the existing selector_from with a normal non-force push, then request a fresh review for the exact new source ref"
                ))
            }
            _ => {}
        }
        Ok(ReadinessReport {
            ready: b.is_empty(),
            blockers: b,
            subject_ref: i.current_subject_ref,
            review,
        })
    }
    pub fn validate_completion(&self, i: &CompleteMergeRequest) -> Result<(), MergeRequestError> {
        let mr = self.get_for_operation(&i.auth.workspace_id, &i.merge_request_id, &i.ticket_id)?;
        self.repo(&i.auth, &mr.repository_id)?;
        if let Some(existing) = mr.thread.iter().find_map(|event| match event {
            MergeRequestThreadEvent::Merge(value) if value.operation_id == i.operation_id => {
                Some(value)
            }
            _ => None,
        }) {
            if existing.approval_event_id == i.approval_event_id
                && existing.approved_source_ref == i.current_subject_ref
                && existing.target_ref_before == i.target_ref_before
                && existing.target_ref_after == i.target_ref_after
                && existing.strategy == i.strategy
                && existing.resolution == i.resolution
                && existing.merged_by == i.auth.actor()
            {
                return Ok(());
            }
            return Err(MergeRequestError::Conflict(
                "operation fingerprint mismatch".into(),
            ));
        }
        self.current_assignment_auth(&i.auth, &i.ticket_id, &mr.repository_id)?;
        if !mr.state.is_open() {
            return Err(MergeRequestError::Conflict(
                "Merge Request is not open".into(),
            ));
        }
        let MergeRequestState::Integration { .. } = &mr.state else {
            return Err(MergeRequestError::NotReady(
                "Merge Request has no confirmed integration entry".into(),
            ));
        };
        let review = mr
            .effective_review(&i.current_subject_ref)
            .filter(|review| review.event_id == i.approval_event_id)
            .ok_or_else(|| {
                MergeRequestError::NotReady(
                    "approval is not the current effective review for the source ref".into(),
                )
            })?;
        if review.decision != ReviewDecision::Approve {
            return Err(MergeRequestError::NotReady(
                "current effective review does not approve the source ref".into(),
            ));
        }
        if i.target_ref_before == i.target_ref_after {
            return Err(MergeRequestError::Validation(
                "target ref did not change".into(),
            ));
        }
        let connection = self.lock()?;
        let current_digest =
            current_ticket_content_digest(&connection, &mr.workspace_id, &i.ticket_id)?
                .ok_or(MergeRequestError::NotFound)?;
        if review.ticket_content_digest != current_digest {
            return Err(MergeRequestError::NotReady(
                "approval predates the current Ticket digest; fresh review is required".into(),
            ));
        }
        // Case openness is not an integration phase. Exact source approval,
        // current Ticket content and current assignment fencing above remain
        // the independent MR integration guards.
        Ok(())
    }

    /// Records externally performed integration results with the approved source.
    /// Rechecks assignments, Ticket content/state, and valid approval before commit.
    /// Replay with the same operation ID and all inputs returns the same result; reuse with different inputs is rejected.
    pub fn complete(&self, i: CompleteMergeRequest) -> Result<MergeEvent, MergeRequestError> {
        self.complete_with_admission(i, |_| Ok(()))
    }

    /// Host authority is checked before receipt validation and again under the
    /// IMMEDIATE commit transaction, including exact replay. Admission must use
    /// the supplied connection and must not reenter this store or call providers.
    pub fn complete_with_admission(
        &self,
        i: CompleteMergeRequest,
        admit: impl Fn(&Connection) -> Result<(), MergeRequestError>,
    ) -> Result<MergeEvent, MergeRequestError> {
        {
            let connection = self.lock()?;
            admit(&connection)?;
        }
        self.validate_completion(&i)?;
        let mut connection = self.lock()?;
        let transaction =
            connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        admit(&transaction)?;
        let mr = load_mr(&transaction, &i.auth.workspace_id, &i.merge_request_id)?
            .filter(|mr| mr.ticket_ids.iter().any(|id| id == &i.ticket_id))
            .ok_or(MergeRequestError::NotFound)?;

        if let Some(existing) = mr.thread.iter().find_map(|event| match event {
            MergeRequestThreadEvent::Merge(value) if value.operation_id == i.operation_id => {
                Some(value)
            }
            _ => None,
        }) {
            if existing.approval_event_id == i.approval_event_id
                && existing.approved_source_ref == i.current_subject_ref
                && existing.target_ref_before == i.target_ref_before
                && existing.target_ref_after == i.target_ref_after
                && existing.strategy == i.strategy
                && existing.resolution == i.resolution
                && existing.merged_by == i.auth.actor()
            {
                return Ok(existing.clone());
            }
            return Err(MergeRequestError::Conflict(
                "operation fingerprint mismatch".into(),
            ));
        }
        if !mr.state.is_open() {
            return Err(MergeRequestError::Conflict(
                "Merge Request is not open".into(),
            ));
        }
        let assignment_is_current: bool = transaction.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM ticket_current_worker_assignments
                 WHERE workspace_id=?1 AND ticket_id=?2 AND assignment_id=?3
             )",
            params![i.auth.workspace_id, i.ticket_id, i.auth.assignment_id],
            |row| row.get(0),
        )?;
        if !assignment_is_current {
            return Err(MergeRequestError::Unauthorized(
                "completion assignment changed before commit".into(),
            ));
        }
        let MergeRequestState::Integration { started_by } = &mr.state else {
            return Err(MergeRequestError::NotReady(
                "Merge Request has no confirmed integration entry".into(),
            ));
        };
        let review = mr
            .effective_review(&i.current_subject_ref)
            .filter(|review| review.event_id == i.approval_event_id)
            .ok_or_else(|| {
                MergeRequestError::NotReady("approval changed before completion commit".into())
            })?;
        if review.decision != ReviewDecision::Approve {
            return Err(MergeRequestError::NotReady(
                "current effective review does not approve the source ref".into(),
            ));
        }
        let current_digest =
            current_ticket_content_digest(&transaction, &mr.workspace_id, &i.ticket_id)?
                .ok_or(MergeRequestError::NotFound)?;
        if review.ticket_content_digest != current_digest {
            return Err(MergeRequestError::NotReady(
                "approval predates the current Ticket digest; fresh review is required".into(),
            ));
        }
        // Case openness is not an integration phase. Exact source approval,
        // current Ticket content and active assignment fencing above remain
        // the independent MR integration guards.
        let issued_grants = {
            let mut statement = transaction.prepare(
                "SELECT request_event_id,subject_ref,capability_token
                   FROM merge_request_review_grants
                  WHERE workspace_id=?1 AND merge_request_id=?2 AND status='issued'
                  ORDER BY issued_at,request_event_id",
            )?;
            statement
                .query_map(params![mr.workspace_id, mr.merge_request_id], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?
        };
        for (request_event_id, subject_ref, capability_token) in issued_grants {
            let cancelled = ReviewCancelledEvent {
                event_id: Uuid::now_v7().to_string(),
                sequence: next_seq(&transaction, &mr.workspace_id, &mr.merge_request_id)?,
                request_event_id,
                subject_ref,
                reason: "Merge Request completed before review submission".into(),
                created_at: i.now,
            };
            insert_event(
                &transaction,
                &mr.workspace_id,
                &mr.merge_request_id,
                "review_cancelled",
                &cancelled,
                i.now,
                None,
            )?;
            transaction.execute(
                "UPDATE merge_request_review_grants
                    SET status='revoked',revoked_at=?2
                  WHERE capability_token=?1 AND status='issued'",
                params![capability_token, i.now.to_rfc3339()],
            )?;
        }
        let event = MergeEvent {
            event_id: Uuid::now_v7().to_string(),
            sequence: next_seq(&transaction, &mr.workspace_id, &mr.merge_request_id)?,
            operation_id: i.operation_id,
            approval_event_id: i.approval_event_id,
            approved_source_ref: review.subject_ref.clone(),
            target_ref_before: i.target_ref_before,
            target_ref_after: i.target_ref_after,
            strategy: i.strategy,
            resolution: i.resolution,
            merged_by: WorkerIdentity {
                runtime_id: i.auth.runtime_id,
                worker_id: i.auth.worker_id,
            },
            created_at: i.now,
        };
        insert_event(
            &transaction,
            &mr.workspace_id,
            &mr.merge_request_id,
            "merge",
            &event,
            i.now,
            Some(&event.operation_id),
        )?;
        transaction.execute(
            "UPDATE merge_requests SET state=?4,updated_at=?3
              WHERE workspace_id=?1 AND merge_request_id=?2 AND json_extract(state,'$.kind')='integration'",
            params![mr.workspace_id, mr.merge_request_id, i.now.to_rfc3339(),
                serde_json::to_string(&MergeRequestState::Completed {
                    completion: MergeRequestCompletion::Published {
                        started_by: started_by.clone(), merge_event_id: event.event_id.clone(),
                    },
                }).map_err(|e| MergeRequestError::Operation(e.to_string()))?],
        )?;
        ticket_event(&transaction, &mr, &event, &i.auth.assignment_id)?;
        transaction.commit()?;
        Ok(event)
    }

    pub fn repair_selector_from(
        &self,
        i: RepairSelectorFrom,
    ) -> Result<MergeRequest, MergeRequestError> {
        nonempty("selector_from", &i.selector_from)?;
        nonempty("resolved_subject_ref", &i.resolved_subject_ref)?;
        let mr = self.get_for_operation(&i.workspace_id, &i.merge_request_id, &i.ticket_id)?;
        if mr.selector_from.is_some() {
            return Err(MergeRequestError::Conflict(
                "selector_from is immutable after it is set".into(),
            ));
        }
        let approved = mr
            .effective_review(&i.resolved_subject_ref)
            .is_some_and(|review| review.decision == ReviewDecision::Approve);
        if !approved {
            return Err(MergeRequestError::NotReady(
                "selector repair must resolve to an approved thread subject".into(),
            ));
        }
        let mut c = self.lock()?;
        let t = c.transaction()?;
        let changed=t.execute("UPDATE merge_requests SET selector_from=?3,updated_at=?4 WHERE workspace_id=?1 AND merge_request_id=?2 AND selector_from IS NULL",params![mr.workspace_id,mr.merge_request_id,i.selector_from,i.now.to_rfc3339()])?;
        if changed != 1 {
            return Err(MergeRequestError::Conflict(
                "selector_from repair raced with another update".into(),
            ));
        }
        let e = CommentEvent {
            operation_id: None,
            owner_operation_id: None,
            event_id: Uuid::now_v7().to_string(),
            sequence: next_seq(&t, &mr.workspace_id, &mr.merge_request_id)?,
            body: format!("selector_from repaired: {}", i.reason),
            author: i.repaired_by,
            created_at: i.now,
        };
        insert_event(
            &t,
            &mr.workspace_id,
            &mr.merge_request_id,
            "comment",
            &e,
            i.now,
            None,
        )?;
        t.commit()?;
        drop(c);
        self.get_by_id(&i.workspace_id, &i.merge_request_id)
    }
    pub fn get(&self, ws: &str, ticket: &str) -> Result<MergeRequest, MergeRequestError> {
        let c = self.lock()?;
        let id:Option<String>=c.query_row("SELECT rel.merge_request_id FROM merge_request_ticket_relations rel JOIN merge_requests mr ON mr.workspace_id=rel.workspace_id AND mr.merge_request_id=rel.merge_request_id WHERE rel.workspace_id=?1 AND rel.ticket_id=?2 ORDER BY CASE WHEN json_extract(mr.state,'$.kind') IN ('implementation','integration') THEN 0 ELSE 1 END,mr.created_at DESC LIMIT 1",params![ws,ticket],|r|r.get(0)).optional()?;
        match id {
            Some(id) => load_mr(&c, ws, &id)?.ok_or(MergeRequestError::NotFound),
            None => Err(MergeRequestError::NotFound),
        }
    }
    fn get_for_operation(
        &self,
        workspace_id: &str,
        merge_request_id: &str,
        ticket_id: &str,
    ) -> Result<MergeRequest, MergeRequestError> {
        let mr = self.get_by_id(workspace_id, merge_request_id)?;
        if !mr.ticket_ids.iter().any(|id| id == ticket_id) {
            return Err(MergeRequestError::Unauthorized(
                "Merge Request is not linked to the addressed Ticket".into(),
            ));
        }
        Ok(mr)
    }

    pub fn get_by_id(
        &self,
        workspace_id: &str,
        merge_request_id: &str,
    ) -> Result<MergeRequest, MergeRequestError> {
        let c = self.lock()?;
        load_mr(&c, workspace_id, merge_request_id)?.ok_or(MergeRequestError::NotFound)
    }
    pub fn list_for_ticket(
        &self,
        workspace_id: &str,
        ticket_id: &str,
    ) -> Result<Vec<MergeRequest>, MergeRequestError> {
        Ok(self
            .list(
                workspace_id,
                &MergeRequestListQuery {
                    ticket_id: Some(ticket_id.to_string()),
                    limit: 100,
                    ..Default::default()
                },
            )?
            .items)
    }

    pub fn list(
        &self,
        workspace_id: &str,
        query: &MergeRequestListQuery,
    ) -> Result<MergeRequestListPage, MergeRequestError> {
        let c = self.lock()?;
        let limit = query.limit.clamp(1, 100);
        let cursor_position = match query.cursor.as_deref() {
            Some(cursor) => Some(
                c.query_row(
                    "SELECT updated_at,merge_request_id FROM merge_requests WHERE workspace_id=?1 AND merge_request_id=?2",
                    params![workspace_id, cursor],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .optional()?
                .ok_or_else(|| MergeRequestError::Validation("invalid merge request cursor".into()))?,
            ),
            None => None,
        };
        let cursor_updated_at = cursor_position
            .as_ref()
            .map(|(updated_at, _)| updated_at.as_str());
        let cursor_id = cursor_position.as_ref().map(|(_, id)| id.as_str());
        let state = query.state.map(MergeRequestStateFilter::as_str);
        let mut statement = c.prepare(
            "SELECT mr.merge_request_id
             FROM merge_requests mr
             WHERE mr.workspace_id=?1
               AND (?2 IS NULL OR CASE json_extract(mr.state,'$.kind') WHEN 'implementation' THEN 'open' WHEN 'integration' THEN 'open' WHEN 'completed' THEN 'merged' WHEN 'canceled' THEN 'closed' END=?2)
               AND (?3 IS NULL OR mr.repository_id=?3)
               AND (?4 IS NULL OR mr.selector_from=?4)
               AND (?5 IS NULL OR mr.selector_to=?5)
               AND (?6 IS NULL OR EXISTS (
                    SELECT 1 FROM merge_request_ticket_relations relation
                    WHERE relation.workspace_id=mr.workspace_id
                      AND relation.merge_request_id=mr.merge_request_id
                      AND relation.ticket_id=?6
               ))
               AND (?7 IS NULL OR mr.updated_at<?7 OR (mr.updated_at=?7 AND mr.merge_request_id>?8))
             ORDER BY mr.updated_at DESC,mr.merge_request_id ASC
             LIMIT ?9",
        )?;
        let rows = statement.query_map(
            params![
                workspace_id,
                state,
                query.repository_id.as_deref(),
                query.selector_from.as_deref(),
                query.selector_to.as_deref(),
                query.ticket_id.as_deref(),
                cursor_updated_at,
                cursor_id,
                (limit + 1) as i64,
            ],
            |row| row.get::<_, String>(0),
        )?;
        let mut ids = rows.collect::<Result<Vec<_>, _>>()?;
        let has_more = ids.len() > limit;
        ids.truncate(limit);
        let next_cursor = has_more.then(|| ids.last().cloned()).flatten();
        let items = ids
            .iter()
            .map(|id| load_mr(&c, workspace_id, id)?.ok_or(MergeRequestError::NotFound))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(MergeRequestListPage { items, next_cursor })
    }
    pub fn thread_page(
        &self,
        ws: &str,
        ticket: &str,
        after: Option<u64>,
        limit: usize,
    ) -> Result<Vec<MergeRequestThreadEvent>, MergeRequestError> {
        let mr = self.get(ws, ticket)?;
        let c = self.lock()?;
        load_thread(&c, ws, &mr.merge_request_id, after, limit.clamp(1, 200))
    }
    pub fn thread_page_by_id(
        &self,
        workspace_id: &str,
        merge_request_id: &str,
        after: Option<u64>,
        limit: usize,
    ) -> Result<Vec<MergeRequestThreadEvent>, MergeRequestError> {
        let c = self.lock()?;
        if load_mr(&c, workspace_id, merge_request_id)?.is_none() {
            return Err(MergeRequestError::NotFound);
        }
        load_thread(
            &c,
            workspace_id,
            merge_request_id,
            after,
            limit.clamp(1, 200),
        )
    }
    fn assigned(&self, a: &MergeRequestAuth, t: &str, r: &str) -> Result<(), MergeRequestError> {
        self.repo(a, r)?;
        let x = self
            .assignments
            .current_assignment(&a.workspace_id, t)
            .map_err(MergeRequestError::Operation)?
            .ok_or_else(|| MergeRequestError::Unauthorized("no assignment".into()))?;
        if x.assignment_id != a.assignment_id
            || x.runtime_id != a.runtime_id
            || x.worker_id != a.worker_id
        {
            return Err(MergeRequestError::Unauthorized(
                "not current assigned Worker".into(),
            ));
        }
        Ok(())
    }
    fn current_assignment_auth(
        &self,
        a: &MergeRequestAuth,
        t: &str,
        r: &str,
    ) -> Result<(), MergeRequestError> {
        self.repo(a, r)?;
        let x = self
            .assignments
            .current_assignment(&a.workspace_id, t)
            .map_err(MergeRequestError::Operation)?
            .ok_or_else(|| MergeRequestError::Unauthorized("no assignment".into()))?;
        if x.assignment_id != a.assignment_id {
            return Err(MergeRequestError::Unauthorized("stale assignment".into()));
        }
        Ok(())
    }
    fn repo(&self, a: &MergeRequestAuth, r: &str) -> Result<(), MergeRequestError> {
        if a.repository_id != r
            || !self
                .repositories
                .repository_belongs_to_workspace(&a.workspace_id, r)
                .map_err(MergeRequestError::Operation)?
        {
            return Err(MergeRequestError::Unauthorized(
                "repository scope mismatch".into(),
            ));
        }
        Ok(())
    }
    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Connection>, MergeRequestError> {
        self.conn
            .lock()
            .map_err(|_| MergeRequestError::Operation("database lock poisoned".into()))
    }
}

fn truncate_body(value: &mut String) {
    if value.len() <= MAX_BODY_BYTES {
        return;
    }
    const MARKER: &str = "\n[truncated]";
    let limit = MAX_BODY_BYTES.saturating_sub(MARKER.len());
    let boundary = value
        .char_indices()
        .map(|(index, _)| index)
        .take_while(|index| *index <= limit)
        .last()
        .unwrap_or(0);
    value.truncate(boundary);
    value.push_str(MARKER);
}

fn bounded_body(name: &str, value: &str) -> Result<(), MergeRequestError> {
    if value.len() > MAX_BODY_BYTES {
        Err(MergeRequestError::Validation(format!(
            "{name} exceeds {MAX_BODY_BYTES} bytes"
        )))
    } else {
        Ok(())
    }
}

fn nonempty(n: &str, v: &str) -> Result<(), MergeRequestError> {
    if v.trim().is_empty() {
        Err(MergeRequestError::Validation(format!(
            "{n} must not be empty"
        )))
    } else {
        Ok(())
    }
}
fn next_seq(t: &Transaction<'_>, w: &str, m: &str) -> Result<u64, MergeRequestError> {
    Ok(t.query_row("SELECT COALESCE(MAX(sequence),0)+1 FROM merge_request_thread_events WHERE workspace_id=?1 AND merge_request_id=?2",params![w,m],|r|r.get::<_,i64>(0))? as u64)
}
fn insert_event<T: Serialize>(
    t: &Transaction<'_>,
    w: &str,
    m: &str,
    k: &str,
    e: &T,
    at: DateTime<Utc>,
    op: Option<&str>,
) -> Result<(), MergeRequestError> {
    let v = serde_json::to_value(e).map_err(|x| MergeRequestError::Operation(x.to_string()))?;
    let id = v["event_id"]
        .as_str()
        .ok_or_else(|| MergeRequestError::Operation("event_id missing".into()))?;
    let seq = v["sequence"]
        .as_u64()
        .ok_or_else(|| MergeRequestError::Operation("sequence missing".into()))?;
    t.execute(
        "INSERT INTO merge_request_thread_events VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
        params![
            w,
            m,
            id,
            seq as i64,
            k,
            serde_json::to_string(e).map_err(|x| MergeRequestError::Operation(x.to_string()))?,
            op,
            at.to_rfc3339()
        ],
    )?;
    Ok(())
}
fn linked_merge_request_ids(
    connection: &Connection,
    workspace_id: &str,
    ticket_id: &str,
) -> Result<Vec<String>, MergeRequestError> {
    let mut statement = connection.prepare(
        "SELECT merge_request_id FROM merge_request_ticket_relations
          WHERE workspace_id=?1 AND ticket_id=?2 AND relation_kind='implements'
          ORDER BY merge_request_id",
    )?;
    statement
        .query_map(params![workspace_id, ticket_id], |row| row.get(0))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(Into::into)
}

fn current_ticket_content_digest(
    connection: &Connection,
    workspace_id: &str,
    ticket_id: &str,
) -> Result<Option<String>, MergeRequestError> {
    ticket::sqlite_ticket_content_digest(connection, workspace_id, ticket_id)
        .map_err(|error| MergeRequestError::Operation(error.to_string()))
}

fn load_mr(c: &Connection, w: &str, m: &str) -> Result<Option<MergeRequest>, MergeRequestError> {
    let row:Option<(String,String,Option<String>,String,String,String)>=c.query_row("SELECT repository_id,state,selector_from,selector_to,created_at,updated_at FROM merge_requests WHERE workspace_id=?1 AND merge_request_id=?2",params![w,m],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).optional()?;
    let Some((repo, state, from, to, created, updated)) = row else {
        return Ok(None);
    };
    let mut s=c.prepare("SELECT ticket_id FROM merge_request_ticket_relations WHERE workspace_id=?1 AND merge_request_id=?2 ORDER BY ticket_id")?;
    let tickets = s
        .query_map(params![w, m], |r| r.get(0))?
        .collect::<Result<Vec<String>, _>>()?;
    Ok(Some(MergeRequest {
        workspace_id: w.into(),
        merge_request_id: m.into(),
        repository_id: repo,
        state: json(&state)?,
        selector_from: from,
        selector_to: to,
        ticket_ids: tickets,
        created_at: time(&created)?,
        updated_at: time(&updated)?,
        thread: load_thread(c, w, m, None, i64::MAX as usize)?,
    }))
}
fn load_thread(
    c: &Connection,
    w: &str,
    m: &str,
    after: Option<u64>,
    limit: usize,
) -> Result<Vec<MergeRequestThreadEvent>, MergeRequestError> {
    let mut s=c.prepare("SELECT kind,payload_json FROM merge_request_thread_events WHERE workspace_id=?1 AND merge_request_id=?2 AND sequence>?3 ORDER BY sequence LIMIT ?4")?;
    let rows = s
        .query_map(
            params![w, m, after.unwrap_or(0) as i64, limit as i64],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        )?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter()
        .map(|(k, j)| {
            let mut event = match k.as_str() {
                "review_requested" => MergeRequestThreadEvent::ReviewRequested(json(&j)?),
                "review" => MergeRequestThreadEvent::Review(json(&j)?),
                "review_revoked" => MergeRequestThreadEvent::ReviewRevoked(json(&j)?),
                "review_cancelled" => MergeRequestThreadEvent::ReviewCancelled(json(&j)?),
                "comment" => MergeRequestThreadEvent::Comment(json(&j)?),
                "merge" => MergeRequestThreadEvent::Merge(json(&j)?),
                "worker_assigned" => MergeRequestThreadEvent::WorkerAssigned(json(&j)?),
                _ => return Err(MergeRequestError::Corrupt(format!("unknown event `{k}`"))),
            };
            event.bound_bodies();
            Ok(event)
        })
        .collect()
}
fn json<T: for<'a> Deserialize<'a>>(v: &str) -> Result<T, MergeRequestError> {
    serde_json::from_str(v).map_err(|e| MergeRequestError::Corrupt(e.to_string()))
}
fn time(v: &str) -> Result<DateTime<Utc>, MergeRequestError> {
    DateTime::parse_from_rfc3339(v)
        .map(|x| x.with_timezone(&Utc))
        .map_err(|e| MergeRequestError::Corrupt(e.to_string()))
}
fn ticket_event(
    t: &Transaction<'_>,
    mr: &MergeRequest,
    e: &MergeEvent,
    a: &str,
) -> Result<(), MergeRequestError> {
    let ticket = &mr.ticket_ids[0];
    let n:i64=t.query_row("SELECT COALESCE(MAX(event_index),-1)+1 FROM typed_ticket_events WHERE workspace_id=?1 AND ticket_id=?2",params![mr.workspace_id,ticket],|r|r.get(0))?;
    t.execute(
        "INSERT INTO typed_ticket_events(
             workspace_id,ticket_id,event_index,kind,author,at,from_state,to_state,heading,body,event_ref
         ) VALUES(?1,?2,?3,'comment',?4,?5,NULL,NULL,'Merge Request integrated',?6,?7)",
        params![
            mr.workspace_id,
            ticket,
            n,
            format!("worker:{}:{}", e.merged_by.runtime_id, e.merged_by.worker_id),
            e.created_at.to_rfc3339(),
            format!(
                "Merge Request `{}` integrated approved source ref `{}`; Ticket remains in progress until guarded Ticket completion.",
                mr.merge_request_id, e.approved_source_ref
            ),
            format!("{ticket}:{n}"),
        ],
    )?;
    for (k, v) in [
        ("implementation_assignment_id", a),
        ("merge_request_id", &mr.merge_request_id),
        ("approval_event_id", &e.approval_event_id),
        ("approved_source_ref", &e.approved_source_ref),
        ("operation_id", &e.operation_id),
    ] {
        t.execute(
            "INSERT INTO typed_ticket_event_attributes VALUES(?1,?2,?3,?4,?5)",
            params![mr.workspace_id, ticket, n, k, v],
        )?;
    }
    Ok(())
}

pub fn migrate(c: &Connection) -> Result<(), MergeRequestError> {
    match schema_state(c)? {
        SchemaState::Fresh => fresh(c),
        SchemaState::Current(SCHEMA_VERSION) => {
            verify(c)?;
            verify_lifecycle_schema(c)?;
            publication::verify(c)
        }
        SchemaState::Current(15) => publication::migrate(c),
        SchemaState::Current(14) => {
            ownership::migrate_v14_worker_ownership(c)?;
            publication::migrate(c)
        }
        SchemaState::Current(13) => {
            migrate_v13_lifecycle(c)?;
            ownership::migrate_v14_worker_ownership(c)?;
            publication::migrate(c)
        }
        SchemaState::Current(12) => {
            migrate_v12_content_attestations(c)?;
            migrate_v13_lifecycle(c)?;
            ownership::migrate_v14_worker_ownership(c)?;
            publication::migrate(c)
        }
        SchemaState::Current(v) => Err(MergeRequestError::Operation(format!(
            "unsupported schema {v}"
        ))),
    }
}

/// Frozen v12 migration: historical counters cannot prove the reviewed content.
/// Preserve request/review identities and integration evidence, but do not
/// manufacture content attestations from today's Ticket. Fresh review is needed.
fn migrate_v12_content_attestations(c: &Connection) -> Result<(), MergeRequestError> {
    let transaction = c.unchecked_transaction()?;
    verify(&transaction)?;
    // Keep the exact historical audit bytes independently of the current DTO
    // projection. An unknown content attestation must not erase its old evidence.
    transaction.execute_batch(
        "CREATE TABLE merge_request_legacy_review_archive (
            workspace_id TEXT NOT NULL,
            merge_request_id TEXT NOT NULL,
            event_id TEXT NOT NULL,
            payload_json TEXT NOT NULL,
            PRIMARY KEY(workspace_id, merge_request_id, event_id)
        );
        INSERT INTO merge_request_legacy_review_archive
            SELECT workspace_id,merge_request_id,event_id,payload_json
            FROM merge_request_thread_events WHERE kind IN ('review_requested','review');
        CREATE TRIGGER merge_request_legacy_review_archive_no_update
            BEFORE UPDATE ON merge_request_legacy_review_archive
            BEGIN SELECT RAISE(ABORT, 'historical review evidence is immutable'); END;
        CREATE TRIGGER merge_request_legacy_review_archive_no_delete
            BEFORE DELETE ON merge_request_legacy_review_archive
            BEGIN SELECT RAISE(ABORT, 'historical review evidence is retained'); END;",
    )?;
    let rows = {
        let mut statement = transaction.prepare(
            "SELECT workspace_id,merge_request_id,event_id,payload_json FROM merge_request_thread_events WHERE kind IN ('review_requested','review')",
        )?;
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?
    };
    for (workspace_id, merge_request_id, event_id, payload) in rows {
        let mut value: serde_json::Value = json(&payload)?;
        let object = value
            .as_object_mut()
            .ok_or_else(|| MergeRequestError::Corrupt("review payload is not an object".into()))?;
        object.remove("ticket_item_revision");
        object.insert(
            "ticket_content_digest".into(),
            serde_json::Value::String(String::new()),
        );
        transaction.execute(
            "UPDATE merge_request_thread_events SET payload_json=?4 WHERE workspace_id=?1 AND merge_request_id=?2 AND event_id=?3",
            params![workspace_id, merge_request_id, event_id, value.to_string()],
        )?;
    }
    transaction.execute(
        "UPDATE merge_request_review_grants SET status='revoked',revoked_at=COALESCE(revoked_at,issued_at) WHERE status='issued'",
        [],
    )?;
    transaction.execute(
        "UPDATE merge_request_schema SET version=?1 WHERE singleton=1",
        params![13],
    )?;
    verify(&transaction)?;
    transaction.commit()?;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SchemaState {
    Fresh,
    Current(i64),
}

fn schema_state(c: &Connection) -> Result<SchemaState, MergeRequestError> {
    let current: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='merge_request_schema')",
        [],
        |r| r.get(0),
    )?;
    if current {
        let (count, singleton, version): (i64, Option<i64>, Option<i64>) = c.query_row(
            "SELECT COUNT(*),MIN(singleton),MAX(version) FROM merge_request_schema",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        if count != 1 || singleton != Some(1) {
            return Err(MergeRequestError::Corrupt(
                "current schema marker must contain exactly singleton 1".into(),
            ));
        }
        let version = version.ok_or_else(|| {
            MergeRequestError::Corrupt("current schema marker version is null".into())
        })?;
        return Ok(SchemaState::Current(version));
    }
    let domain_tables: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name GLOB 'merge_request*')",
        [],
        |r| r.get(0),
    )?;
    if domain_tables {
        return Err(MergeRequestError::Corrupt(
            "merge request tables exist without a schema marker".into(),
        ));
    }
    Ok(SchemaState::Fresh)
}
fn fresh(c: &Connection) -> Result<(), MergeRequestError> {
    let t = c.unchecked_transaction()?;
    tables(&t)?;
    ownership::worker_event_tables(&t)?;
    publication::tables(&t)?;
    lifecycle::add_lifecycle_column(&t, "state")?;
    lifecycle::lifecycle_triggers(&t)?;
    t.execute(
        "INSERT INTO merge_request_schema VALUES(1,?1)",
        params![SCHEMA_VERSION],
    )?;
    fk(&t)?;
    t.commit()?;
    Ok(())
}
fn tables(t: &Transaction<'_>) -> Result<(), MergeRequestError> {
    t.execute_batch("CREATE TABLE merge_request_schema(singleton INTEGER PRIMARY KEY CHECK(singleton=1),version INTEGER NOT NULL);")?;
    t.execute_batch("CREATE TABLE merge_requests(workspace_id TEXT NOT NULL,merge_request_id TEXT NOT NULL,repository_id TEXT NOT NULL,selector_from TEXT,selector_to TEXT NOT NULL,created_at TEXT NOT NULL,updated_at TEXT NOT NULL,PRIMARY KEY(workspace_id,merge_request_id),FOREIGN KEY(workspace_id,repository_id)REFERENCES repositories(workspace_id,repository_id));CREATE TABLE merge_request_ticket_relations(workspace_id TEXT NOT NULL,merge_request_id TEXT NOT NULL,ticket_id TEXT NOT NULL,relation_kind TEXT NOT NULL CHECK(relation_kind='implements'),created_at TEXT NOT NULL,PRIMARY KEY(workspace_id,merge_request_id,ticket_id),FOREIGN KEY(workspace_id,merge_request_id)REFERENCES merge_requests(workspace_id,merge_request_id)ON DELETE CASCADE,FOREIGN KEY(workspace_id,ticket_id)REFERENCES typed_tickets(workspace_id,ticket_id)ON DELETE CASCADE);CREATE TABLE merge_request_reviewer_child_sessions(workspace_id TEXT NOT NULL,child_session_id TEXT NOT NULL,parent_runtime_id TEXT NOT NULL,parent_worker_id TEXT NOT NULL,reviewer_profile TEXT NOT NULL,registered_at TEXT NOT NULL,status TEXT NOT NULL CHECK(status IN('active','consumed')),PRIMARY KEY(workspace_id,child_session_id));")?;
    Ok(())
}
fn verify(c: &Connection) -> Result<(), MergeRequestError> {
    for n in DOMAIN_TABLES {
        let e: bool = c.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table'AND name=?1)",
            params![n],
            |r| r.get(0),
        )?;
        if !e {
            return Err(MergeRequestError::Corrupt(format!("missing `{n}`")));
        }
    }
    fk(c)
}
fn fk(c: &Connection) -> Result<(), MergeRequestError> {
    for table in DOMAIN_TABLES {
        let sql = format!("PRAGMA foreign_key_check('{table}')");
        let violation: Option<(String, Option<i64>, String)> = c
            .query_row(&sql, [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .optional()?;
        if let Some((child, row, parent)) = violation {
            return Err(MergeRequestError::Corrupt(format!(
                "foreign key violation in `{child}` row {row:?}, parent `{parent}`"
            )));
        }
    }
    Ok(())
}

mod comments;
