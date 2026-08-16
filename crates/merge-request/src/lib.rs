//! Workspace-scoped Merge Request authority.
//!
//! Merge Requests deliberately do not reuse Ticket thread review events.  A review is
//! evidence for one immutable revision and can only be committed with a one-shot
//! capability registered from an actual Runtime-owned direct-child reviewer session.

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::Duration;
use thiserror::Error;

const SCHEMA_VERSION: i64 = 10;
const REVIEWER_PROFILE: &str = "builtin:reviewer";
const MAX_SUMMARY_BYTES: usize = 16 * 1024;
const MAX_REVIEW_BODY_BYTES: usize = 64 * 1024;
const MAX_CHANGED_PATHS: usize = 1_000;
const MAX_FINDINGS: usize = 1_000;
const MAX_FIELD_BYTES: usize = 4 * 1024;

pub type Result<T> = std::result::Result<T, MergeRequestError>;

#[derive(Debug, Error)]
pub enum MergeRequestError {
    #[error("merge request database error: {0}")]
    Database(String),
    #[error("{0} must not be empty")]
    Empty(&'static str),
    #[error("{field} exceeds its bounded limit of {max} bytes/items")]
    TooLarge { field: &'static str, max: usize },
    #[error("merge request not found for ticket {0}")]
    NotFound(String),
    #[error("merge request already exists for ticket {0}")]
    AlreadyExists(String),
    #[error("immutable revision {0} already exists with different content")]
    RevisionConflict(String),
    #[error("stale merge request revision: expected {expected}, current {current}")]
    StaleRevision { expected: String, current: String },
    #[error("current Ticket assignment does not match the authenticated Coder")]
    AssignmentMismatch,
    #[error("reviewer must be an actual direct-child with effective profile builtin:reviewer")]
    InvalidReviewer,
    #[error("review attempt is invalid, revoked, already used, or belongs to another revision")]
    InvalidReviewAttempt,
    #[error("review result cannot be supplied by the assigned Coder itself")]
    SelfApproval,
    #[error("merge request current revision is not approved")]
    NotApproved,
    #[error("merge request is {0}, expected open")]
    NotOpen(String),
    #[error("completion operation id was reused with different input")]
    OperationConflict,
    #[error("merge result operation id was reused with different input")]
    MergeResultOperationConflict,
    #[error("merge result {0} was not found for the current Merge Request revision")]
    MergeResultNotFound(String),
    #[error("merge result is not the current final integration candidate")]
    MergeResultNotFinal,
    #[error("current final merge result is missing")]
    FinalMergeResultMissing,
    #[error("current target does not equal the final merge result commit")]
    FinalMergeResultNotApplied,
    #[error("merge result evidence is invalid: {0}")]
    InvalidMergeResult(String),
    #[error("Merge Request target is unknown and must be resolved explicitly")]
    UnknownTarget,
    #[error("Ticket must be inprogress before Merge Request completion (current: {0})")]
    TicketStateConflict(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MergeRequestState {
    Draft,
    Open,
    Closed,
    Merged,
}

impl MergeRequestState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Open => "open",
            Self::Closed => "closed",
            Self::Merged => "merged",
        }
    }

    fn parse(value: &str) -> Self {
        match value {
            "draft" => Self::Draft,
            "closed" => Self::Closed,
            "merged" => Self::Merged,
            _ => Self::Open,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewDecision {
    Approve,
    RequestChanges,
}

impl ReviewDecision {
    fn as_str(self) -> &'static str {
        match self {
            Self::Approve => "approve",
            Self::RequestChanges => "request_changes",
        }
    }

    fn parse(value: &str) -> Self {
        match value {
            "approve" => Self::Approve,
            _ => Self::RequestChanges,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewStatus {
    Pending,
    Approved,
    ChangesRequested,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MergeRequestTargetStatus {
    Known,
    Unknown,
}

impl MergeRequestTargetStatus {
    fn parse(value: &str) -> Self {
        match value {
            "known" => Self::Known,
            _ => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MergeStrategy {
    FastForward,
    Merge,
}

impl MergeStrategy {
    fn as_str(self) -> &'static str {
        match self {
            Self::FastForward => "fast_forward",
            Self::Merge => "merge",
        }
    }

    fn parse(value: &str) -> Self {
        match value {
            "fast_forward" => Self::FastForward,
            _ => Self::Merge,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MergeResolution {
    None,
    Clean,
    ConflictsResolved,
}

impl MergeResolution {
    fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Clean => "clean",
            Self::ConflictsResolved => "conflicts_resolved",
        }
    }

    fn parse(value: &str) -> Self {
        match value {
            "none" => Self::None,
            "conflicts_resolved" => Self::ConflictsResolved,
            _ => Self::Clean,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MergeResultTargetStatus {
    Current,
    Applied,
    Stale,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeRequestRevision {
    pub revision_id: String,
    pub ordinal: u64,
    pub base_commit: String,
    pub head_commit: String,
    pub diff_digest: String,
    pub changed_paths: Vec<String>,
    pub summary: String,
    pub assignment_id: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewFinding {
    pub severity: String,
    pub code: Option<String>,
    pub path: Option<String>,
    pub line: Option<u64>,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeRequestReview {
    pub attempt_id: String,
    pub revision_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub merge_result_id: Option<String>,
    pub decision: ReviewDecision,
    pub body: String,
    pub findings: Vec<ReviewFinding>,
    pub parent_assignment_id: String,
    pub parent_runtime_id: String,
    pub parent_worker_id: String,
    pub reviewer_child_session_id: String,
    pub reviewer_effective_profile: String,
    pub submitted_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeResult {
    pub merge_result_id: String,
    pub revision_id: String,
    pub target_commit: String,
    pub source_commit: String,
    pub result_commit: String,
    pub strategy: MergeStrategy,
    pub resolution: MergeResolution,
    pub created_by_runtime_id: String,
    pub created_by_worker_id: String,
    pub created_at: String,
    pub operation_id: String,
    pub validated_at: String,
    pub target_status: MergeResultTargetStatus,
    pub review_status: ReviewStatus,
    pub current_review: Option<MergeRequestReview>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeRequest {
    pub merge_request_id: String,
    pub workspace_id: String,
    pub ticket_id: String,
    pub repository_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_ref_selector: Option<String>,
    pub target_status: MergeRequestTargetStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_target_commit: Option<String>,
    pub state: MergeRequestState,
    pub lifecycle_generation: u64,
    pub current_revision: MergeRequestRevision,
    pub review_status: ReviewStatus,
    pub current_review: Option<MergeRequestReview>,
    pub merge_results: Vec<MergeResult>,
    /// The one explicitly selected final integration candidate. Historical
    /// candidates remain in `merge_results` but never compete with this pointer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_merge_result: Option<MergeResult>,
    pub created_at: String,
    pub updated_at: String,
    pub merged_by_account_id: Option<String>,
    pub merged_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenMergeRequest {
    pub merge_request_id: String,
    pub ticket_id: String,
    pub repository_id: String,
    pub target_ref_selector: String,
    pub revision: MergeRequestRevision,
    pub authenticated_runtime_id: String,
    pub authenticated_worker_id: String,
    pub now: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddRevision {
    pub ticket_id: String,
    pub expected_current_revision_id: String,
    pub revision: MergeRequestRevision,
    pub authenticated_runtime_id: String,
    pub authenticated_worker_id: String,
    pub now: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisterReviewerChildSession {
    pub parent_runtime_id: String,
    pub parent_worker_id: String,
    pub child_session_id: String,
    pub now: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisterReviewAttempt {
    pub attempt_id: String,
    pub ticket_id: String,
    pub revision_id: String,
    pub merge_result_id: Option<String>,
    pub parent_assignment_id: String,
    pub parent_runtime_id: String,
    pub parent_worker_id: String,
    pub child_session_id: String,
    /// A secret generated by the trusted spawn layer and injected only into the child client.
    pub capability_token: String,
    pub now: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmitReview {
    pub ticket_id: String,
    pub revision_id: String,
    pub merge_result_id: Option<String>,
    pub capability_token: String,
    pub decision: ReviewDecision,
    pub body: String,
    pub findings: Vec<ReviewFinding>,
    pub now: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordMergeResult {
    pub merge_result_id: String,
    pub ticket_id: String,
    pub expected_revision_id: String,
    pub target_commit: String,
    pub source_commit: String,
    pub result_commit: String,
    pub strategy: MergeStrategy,
    pub resolution: MergeResolution,
    pub operation_id: String,
    pub actor_runtime_id: String,
    pub actor_worker_id: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordMergeResultOutcome {
    pub merge_result: MergeResult,
    pub replayed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompleteMergeRequest {
    pub operation_id: String,
    pub ticket_id: String,
    pub expected_revision_id: String,
    pub expected_merge_result_id: String,
    pub observed_target_commit: String,
    pub implementation_assignment_id: String,
    pub completion_actor_runtime_id: String,
    pub completion_actor_worker_id: String,
    pub now: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletionOutcome {
    pub operation_id: String,
    pub ticket_id: String,
    pub revision_id: String,
    pub ticket_state: String,
    pub replayed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeRequestReadiness {
    pub ticket_id: String,
    pub merge_request_id: String,
    pub revision_id: String,
    pub target_ref_selector: Option<String>,
    pub observed_target_commit: Option<String>,
    pub ready: bool,
    pub review_status: ReviewStatus,
    pub merge_result_id: Option<String>,
    pub merge_result_review_status: Option<ReviewStatus>,
    pub blockers: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct SqliteMergeRequestStore {
    db_path: PathBuf,
    workspace_id: String,
}

impl SqliteMergeRequestStore {
    pub fn open(db_path: impl Into<PathBuf>, workspace_id: impl Into<String>) -> Result<Self> {
        let store = Self {
            db_path: db_path.into(),
            workspace_id: workspace_id.into(),
        };
        let conn = store.connect()?;
        migrate(&conn)?;
        Ok(store)
    }

    pub fn open_verified(
        db_path: impl Into<PathBuf>,
        workspace_id: impl Into<String>,
    ) -> Result<Self> {
        let store = Self {
            db_path: db_path.into(),
            workspace_id: workspace_id.into(),
        };
        verify(&store.connect()?)?;
        Ok(store)
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    pub fn workspace_id(&self) -> &str {
        &self.workspace_id
    }

    fn connect(&self) -> Result<Connection> {
        let conn = Connection::open(&self.db_path).map_err(db)?;
        conn.busy_timeout(Duration::from_secs(5)).map_err(db)?;
        conn.pragma_update(None, "foreign_keys", "ON").map_err(db)?;
        Ok(conn)
    }

    fn write<R>(&self, op: impl FnOnce(&Connection) -> Result<R>) -> Result<R> {
        let conn = self.connect()?;
        verify(&conn)?;
        conn.execute_batch("BEGIN IMMEDIATE").map_err(db)?;
        match op(&conn) {
            Ok(value) => {
                conn.execute_batch("COMMIT").map_err(db)?;
                Ok(value)
            }
            Err(error) => {
                let _ = conn.execute_batch("ROLLBACK");
                Err(error)
            }
        }
    }

    pub fn show_for_ticket(&self, ticket_id: &str) -> Result<Option<MergeRequest>> {
        self.show_for_ticket_with_target(ticket_id, None)
    }

    pub fn show_for_ticket_with_target(
        &self,
        ticket_id: &str,
        observed_target_commit: Option<&str>,
    ) -> Result<Option<MergeRequest>> {
        nonempty("ticket_id", ticket_id)?;
        let conn = self.connect()?;
        verify(&conn)?;
        let mut mr = load_merge_request(&conn, &self.workspace_id, ticket_id)?;
        if let Some(mr) = mr.as_mut() {
            apply_target_observation(mr, observed_target_commit);
        }
        Ok(mr)
    }

    pub fn readiness_for_ticket(&self, ticket_id: &str) -> Result<MergeRequestReadiness> {
        self.readiness_for_ticket_with_target(ticket_id, None)
    }

    pub fn readiness_for_ticket_with_target(
        &self,
        ticket_id: &str,
        observed_target_commit: Option<&str>,
    ) -> Result<MergeRequestReadiness> {
        let mr = self
            .show_for_ticket_with_target(ticket_id, observed_target_commit)?
            .ok_or_else(|| MergeRequestError::NotFound(ticket_id.to_string()))?;
        let mut blockers = Vec::new();
        if mr.state != MergeRequestState::Open {
            blockers.push(format!("merge request is {}", mr.state.as_str()));
        }
        match mr.review_status {
            ReviewStatus::Pending => {
                blockers.push("current source revision has no review result".into())
            }
            ReviewStatus::ChangesRequested => {
                blockers.push("current source revision has request_changes".into())
            }
            ReviewStatus::Approved => {}
        }
        if mr.target_status != MergeRequestTargetStatus::Known || observed_target_commit.is_none() {
            blockers.push("merge request target is unknown or could not be resolved".into());
        }
        let final_result = mr.final_merge_result.as_ref();
        match final_result {
            None if observed_target_commit.is_some() => {
                blockers.push("current source revision has no final validated MergeResult".into())
            }
            Some(result) if result.target_status == MergeResultTargetStatus::Stale => {
                blockers.push("target moved after the final MergeResult was recorded".into())
            }
            Some(result) if result.target_status == MergeResultTargetStatus::Unknown => {
                blockers.push("final MergeResult target state could not be resolved".into())
            }
            Some(result)
                if matches!(result.strategy, MergeStrategy::Merge)
                    && result.review_status != ReviewStatus::Approved =>
            {
                blockers
                    .push("non-fast-forward final MergeResult is not independently approved".into())
            }
            _ => {}
        }
        Ok(MergeRequestReadiness {
            ticket_id: ticket_id.to_string(),
            merge_request_id: mr.merge_request_id,
            revision_id: mr.current_revision.revision_id,
            target_ref_selector: mr.target_ref_selector,
            observed_target_commit: mr.observed_target_commit,
            ready: blockers.is_empty(),
            review_status: mr.review_status,
            merge_result_id: final_result.map(|result| result.merge_result_id.clone()),
            merge_result_review_status: final_result.map(|result| result.review_status),
            blockers,
        })
    }

    pub fn open_merge_request(&self, input: OpenMergeRequest) -> Result<MergeRequest> {
        validate_revision(&input.revision)?;
        for (name, value) in [
            ("merge_request_id", input.merge_request_id.as_str()),
            ("ticket_id", input.ticket_id.as_str()),
            ("repository_id", input.repository_id.as_str()),
            ("target_ref_selector", input.target_ref_selector.as_str()),
            ("runtime_id", input.authenticated_runtime_id.as_str()),
            ("worker_id", input.authenticated_worker_id.as_str()),
        ] {
            nonempty(name, value)?;
        }
        self.write(|conn| {
            validate_current_assignment(
                conn,
                &self.workspace_id,
                &input.ticket_id,
                &input.revision.assignment_id,
                &input.authenticated_runtime_id,
                &input.authenticated_worker_id,
            )?;
            conn.execute(
                "INSERT INTO merge_requests (workspace_id, merge_request_id, repository_id, target_ref_selector, target_status, state, lifecycle_generation, current_revision_id, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, 'known', 'open', 1, ?5, ?6, ?6)",
                params![self.workspace_id, input.merge_request_id, input.repository_id, input.target_ref_selector, input.revision.revision_id, input.now],
            ).map_err(db)?;
            conn.execute(
                "INSERT INTO merge_request_ticket_relations (workspace_id,merge_request_id,ticket_id,relation_kind,created_at) VALUES (?1,?2,?3,'implements',?4)",
                params![self.workspace_id,input.merge_request_id,input.ticket_id,input.now],
            ).map_err(db)?;
            insert_revision(conn, &self.workspace_id, &input.merge_request_id, &input.revision)?;
            load_merge_request(conn, &self.workspace_id, &input.ticket_id)?.ok_or_else(|| MergeRequestError::NotFound(input.ticket_id.clone()))
        })
    }

    pub fn add_revision(&self, input: AddRevision) -> Result<MergeRequest> {
        validate_revision(&input.revision)?;
        self.write(|conn| {
            let current = load_merge_request(conn, &self.workspace_id, &input.ticket_id)?
                .ok_or_else(|| MergeRequestError::NotFound(input.ticket_id.clone()))?;
            ensure_open(&current)?;
            if current.current_revision.revision_id != input.expected_current_revision_id {
                return Err(MergeRequestError::StaleRevision {
                    expected: input.expected_current_revision_id.clone(),
                    current: current.current_revision.revision_id,
                });
            }
            validate_current_assignment(
                conn,
                &self.workspace_id,
                &input.ticket_id,
                &input.revision.assignment_id,
                &input.authenticated_runtime_id,
                &input.authenticated_worker_id,
            )?;
            if input.revision.ordinal != current.current_revision.ordinal + 1 {
                return Err(MergeRequestError::RevisionConflict(input.revision.revision_id.clone()));
            }
            let existing: Option<(String, String, String)> = conn.query_row(
                "SELECT base_commit, head_commit, diff_digest FROM merge_request_revisions WHERE workspace_id=?1 AND merge_request_id=?2 AND revision_id=?3",
                params![self.workspace_id, current.merge_request_id, input.revision.revision_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            ).optional().map_err(db)?;
            if let Some(existing) = existing {
                if existing == (input.revision.base_commit.clone(), input.revision.head_commit.clone(), input.revision.diff_digest.clone()) {
                    return Ok(current);
                }
                return Err(MergeRequestError::RevisionConflict(input.revision.revision_id.clone()));
            }
            insert_revision(conn, &self.workspace_id, &current.merge_request_id, &input.revision)?;
            conn.execute(
                "UPDATE merge_requests SET current_revision_id=?3, updated_at=?4 WHERE workspace_id=?1 AND merge_request_id=?2 AND current_revision_id=?5",
                params![self.workspace_id, current.merge_request_id, input.revision.revision_id, input.now, input.expected_current_revision_id],
            ).map_err(db)?;
            conn.execute(
                "DELETE FROM merge_request_final_results WHERE workspace_id=?1 AND merge_request_id=?2",
                params![self.workspace_id,current.merge_request_id],
            ).map_err(db)?;
            load_merge_request(conn, &self.workspace_id, &input.ticket_id)?.ok_or_else(|| MergeRequestError::NotFound(input.ticket_id.clone()))
        })
    }

    pub fn register_reviewer_child_session(
        &self,
        input: RegisterReviewerChildSession,
    ) -> Result<()> {
        nonempty("runtime_id", &input.parent_runtime_id)?;
        nonempty("worker_id", &input.parent_worker_id)?;
        nonempty("child_session_id", &input.child_session_id)?;
        self.write(|conn| {
            conn.execute(
                "INSERT INTO merge_request_reviewer_child_sessions (workspace_id,child_session_id,parent_runtime_id,parent_worker_id,effective_profile,registered_at) VALUES (?1,?2,?3,?4,'builtin:reviewer',?5)",
                params![self.workspace_id,input.child_session_id,input.parent_runtime_id,input.parent_worker_id,input.now],
            ).map_err(|_| MergeRequestError::InvalidReviewer)?;
            Ok(())
        })
    }

    pub fn register_review_attempt(&self, input: RegisterReviewAttempt) -> Result<()> {
        for (name, value) in [
            ("attempt_id", input.attempt_id.as_str()),
            ("capability_token", input.capability_token.as_str()),
            ("child_session_id", input.child_session_id.as_str()),
        ] {
            nonempty(name, value)?;
        }
        if input.child_session_id == input.parent_worker_id {
            return Err(MergeRequestError::SelfApproval);
        }
        self.write(|conn| {
            let mr = load_merge_request(conn, &self.workspace_id, &input.ticket_id)?
                .ok_or_else(|| MergeRequestError::NotFound(input.ticket_id.clone()))?;
            ensure_open(&mr)?;
            if mr.current_revision.revision_id != input.revision_id {
                return Err(MergeRequestError::StaleRevision { expected: input.revision_id.clone(), current: mr.current_revision.revision_id });
            }
            if let Some(merge_result_id) = input.merge_result_id.as_deref() {
                let exists: bool = conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM merge_request_merge_results WHERE workspace_id=?1 AND merge_request_id=?2 AND revision_id=?3 AND merge_result_id=?4)",
                    params![self.workspace_id,mr.merge_request_id,input.revision_id,merge_result_id],
                    |row| row.get(0),
                ).map_err(db)?;
                if !exists {
                    return Err(MergeRequestError::MergeResultNotFound(merge_result_id.into()));
                }
                let is_final: bool = conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM merge_request_final_results WHERE workspace_id=?1 AND merge_request_id=?2 AND revision_id=?3 AND merge_result_id=?4)",
                    params![self.workspace_id,mr.merge_request_id,input.revision_id,merge_result_id],
                    |row| row.get(0),
                ).map_err(db)?;
                if !is_final {
                    return Err(MergeRequestError::MergeResultNotFinal);
                }
                validate_current_assignment_id(conn, &self.workspace_id, &input.ticket_id, &input.parent_assignment_id)?;
            } else {
                validate_current_assignment(conn, &self.workspace_id, &input.ticket_id, &input.parent_assignment_id, &input.parent_runtime_id, &input.parent_worker_id)?;
            }
            let effective_profile: Option<String> = conn.query_row(
                "SELECT effective_profile FROM merge_request_reviewer_child_sessions WHERE workspace_id=?1 AND child_session_id=?2 AND parent_runtime_id=?3 AND parent_worker_id=?4",
                params![self.workspace_id,input.child_session_id,input.parent_runtime_id,input.parent_worker_id],
                |row| row.get(0),
            ).optional().map_err(db)?;
            if effective_profile.as_deref() != Some(REVIEWER_PROFILE) {
                return Err(MergeRequestError::InvalidReviewer);
            }
            conn.execute(
                "INSERT INTO merge_request_review_attempts (workspace_id, attempt_id, merge_request_id, ticket_id, revision_id, merge_result_id, lifecycle_generation, parent_assignment_id, parent_runtime_id, parent_worker_id, child_session_id, child_effective_profile, capability_token_sha256, status, created_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,'open',?14)",
                params![self.workspace_id, input.attempt_id, mr.merge_request_id, input.ticket_id, input.revision_id, input.merge_result_id, mr.lifecycle_generation as i64, input.parent_assignment_id, input.parent_runtime_id, input.parent_worker_id, input.child_session_id, REVIEWER_PROFILE, token_hash(&input.capability_token), input.now],
            ).map_err(|_| MergeRequestError::InvalidReviewAttempt)?;
            Ok(())
        })
    }

    pub fn revoke_review_attempt(
        &self,
        attempt_id: &str,
        child_session_id: &str,
        now: &str,
    ) -> Result<bool> {
        self.write(|conn| {
            let changed = conn.execute(
                "UPDATE merge_request_review_attempts SET status='revoked', consumed_at=?4 WHERE workspace_id=?1 AND attempt_id=?2 AND child_session_id=?3 AND status='open'",
                params![self.workspace_id, attempt_id, child_session_id, now],
            ).map_err(db)?;
            Ok(changed == 1)
        })
    }

    pub fn submit_review(&self, input: SubmitReview) -> Result<MergeRequestReview> {
        nonempty("capability_token", &input.capability_token)?;
        validate_review_input(&input)?;
        self.write(|conn| {
            let token = token_hash(&input.capability_token);
            let attempt: Option<(String,String,Option<String>,String,String,String,String,String,String,i64)> = conn.query_row(
                "SELECT attempt_id, merge_request_id, merge_result_id, parent_assignment_id, parent_runtime_id, parent_worker_id, child_session_id, child_effective_profile, status, lifecycle_generation FROM merge_request_review_attempts WHERE workspace_id=?1 AND ticket_id=?2 AND revision_id=?3 AND ((?4 IS NULL AND merge_result_id IS NULL) OR merge_result_id=?4) AND capability_token_sha256=?5",
                params![self.workspace_id, input.ticket_id, input.revision_id, input.merge_result_id, token],
                |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?,row.get(6)?,row.get(7)?,row.get(8)?,row.get(9)?)),
            ).optional().map_err(db)?;
            let Some((attempt_id, mr_id, merge_result_id, assignment_id, runtime_id, worker_id, child_session_id, effective_profile, status, lifecycle_generation)) = attempt else {
                return Err(MergeRequestError::InvalidReviewAttempt);
            };
            if status != "open" || effective_profile != REVIEWER_PROFILE || child_session_id == worker_id {
                return Err(MergeRequestError::InvalidReviewAttempt);
            }
            let mr = load_merge_request(conn, &self.workspace_id, &input.ticket_id)?
                .ok_or_else(|| MergeRequestError::NotFound(input.ticket_id.clone()))?;
            if lifecycle_generation != mr.lifecycle_generation as i64 {
                return Err(MergeRequestError::InvalidReviewAttempt);
            }
            ensure_open(&mr)?;
            if mr.current_revision.revision_id != input.revision_id {
                return Err(MergeRequestError::StaleRevision { expected: input.revision_id.clone(), current: mr.current_revision.revision_id });
            }
            if let Some(merge_result_id) = merge_result_id.as_deref() {
                let is_final: bool = conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM merge_request_final_results WHERE workspace_id=?1 AND merge_request_id=?2 AND revision_id=?3 AND merge_result_id=?4)",
                    params![self.workspace_id,mr.merge_request_id,input.revision_id,merge_result_id],
                    |row| row.get(0),
                ).map_err(db)?;
                if !is_final {
                    return Err(MergeRequestError::MergeResultNotFinal);
                }
                validate_current_assignment_id(conn, &self.workspace_id, &input.ticket_id, &assignment_id)?;
            } else {
                validate_current_assignment(conn, &self.workspace_id, &input.ticket_id, &assignment_id, &runtime_id, &worker_id)?;
            }
            conn.execute(
                "INSERT INTO merge_request_reviews (workspace_id, attempt_id, merge_request_id, revision_id, merge_result_id, decision, body, submitted_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
                params![self.workspace_id, attempt_id, mr_id, input.revision_id, merge_result_id, input.decision.as_str(), input.body, input.now],
            ).map_err(|_| MergeRequestError::InvalidReviewAttempt)?;
            for (ordinal, finding) in input.findings.iter().enumerate() {
                nonempty("finding.body", &finding.body)?;
                conn.execute(
                    "INSERT INTO merge_request_review_findings (workspace_id, attempt_id, ordinal, severity, code, path, line, body) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
                    params![self.workspace_id, attempt_id, ordinal as i64, finding.severity, finding.code, finding.path, finding.line.map(|v| v as i64), finding.body],
                ).map_err(db)?;
            }
            conn.execute(
                "UPDATE merge_request_review_attempts SET status='submitted', consumed_at=?3 WHERE workspace_id=?1 AND attempt_id=?2 AND status='open'",
                params![self.workspace_id, attempt_id, input.now],
            ).map_err(db)?;
            load_review(conn, &self.workspace_id, &attempt_id)?.ok_or(MergeRequestError::InvalidReviewAttempt)
        })
    }

    pub fn record_merge_result(
        &self,
        input: RecordMergeResult,
    ) -> Result<RecordMergeResultOutcome> {
        for (name, value) in [
            ("merge_result_id", input.merge_result_id.as_str()),
            ("ticket_id", input.ticket_id.as_str()),
            ("expected_revision_id", input.expected_revision_id.as_str()),
            ("target_commit", input.target_commit.as_str()),
            ("source_commit", input.source_commit.as_str()),
            ("result_commit", input.result_commit.as_str()),
            ("operation_id", input.operation_id.as_str()),
            ("actor_runtime_id", input.actor_runtime_id.as_str()),
            ("actor_worker_id", input.actor_worker_id.as_str()),
        ] {
            nonempty(name, value)?;
        }
        if matches!(input.strategy, MergeStrategy::FastForward)
            && (input.result_commit != input.source_commit
                || !matches!(input.resolution, MergeResolution::None))
        {
            return Err(MergeRequestError::InvalidMergeResult(
                "fast-forward result must equal the source commit and use resolution=none".into(),
            ));
        }
        if matches!(input.strategy, MergeStrategy::Merge)
            && matches!(input.resolution, MergeResolution::None)
        {
            return Err(MergeRequestError::InvalidMergeResult(
                "merge strategy requires clean or conflicts_resolved resolution".into(),
            ));
        }
        let fingerprint = merge_result_fingerprint(&input);
        self.write(|conn| {
            if let Some((stored, merge_result_id, generation)) = conn
                .query_row(
                    "SELECT r.operation_fingerprint,r.merge_result_id,mr.lifecycle_generation FROM merge_request_merge_results r JOIN merge_requests mr ON mr.workspace_id=r.workspace_id AND mr.merge_request_id=r.merge_request_id WHERE r.workspace_id=?1 AND r.operation_id=?2",
                    params![self.workspace_id,input.operation_id],
                    |row| Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,i64>(2)?)),
                )
                .optional()
                .map_err(db)?
            {
                if stored != fingerprint {
                    return Err(MergeRequestError::MergeResultOperationConflict);
                }
                let merge_result = load_merge_result(
                    conn,
                    &self.workspace_id,
                    &merge_result_id,
                    generation as u64,
                )?
                    .ok_or_else(|| MergeRequestError::MergeResultNotFound(merge_result_id.clone()))?;
                return Ok(RecordMergeResultOutcome { merge_result, replayed: true });
            }
            let mr = load_merge_request(conn, &self.workspace_id, &input.ticket_id)?
                .ok_or_else(|| MergeRequestError::NotFound(input.ticket_id.clone()))?;
            ensure_open(&mr)?;
            if mr.target_status != MergeRequestTargetStatus::Known {
                return Err(MergeRequestError::UnknownTarget);
            }
            if mr.current_revision.revision_id != input.expected_revision_id {
                return Err(MergeRequestError::StaleRevision {
                    expected: input.expected_revision_id.clone(),
                    current: mr.current_revision.revision_id,
                });
            }
            if mr.current_revision.head_commit != input.source_commit {
                return Err(MergeRequestError::InvalidMergeResult(
                    "source commit does not match the current source revision".into(),
                ));
            }
            conn.execute(
                "INSERT INTO merge_request_merge_results (workspace_id,merge_result_id,merge_request_id,ticket_id,revision_id,target_commit,source_commit,result_commit,strategy,resolution,created_by_runtime_id,created_by_worker_id,created_at,operation_id,operation_fingerprint,validated_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?13)",
                params![self.workspace_id,input.merge_result_id,mr.merge_request_id,input.ticket_id,input.expected_revision_id,input.target_commit,input.source_commit,input.result_commit,input.strategy.as_str(),input.resolution.as_str(),input.actor_runtime_id,input.actor_worker_id,input.created_at,input.operation_id,fingerprint],
            ).map_err(db)?;
            conn.execute(
                "INSERT INTO merge_request_final_results (workspace_id,merge_request_id,revision_id,merge_result_id,selected_at) VALUES (?1,?2,?3,?4,?5) ON CONFLICT(workspace_id,merge_request_id) DO UPDATE SET revision_id=excluded.revision_id,merge_result_id=excluded.merge_result_id,selected_at=excluded.selected_at",
                params![self.workspace_id,mr.merge_request_id,input.expected_revision_id,input.merge_result_id,input.created_at],
            ).map_err(db)?;
            let merge_result = load_merge_result(conn, &self.workspace_id, &input.merge_result_id, mr.lifecycle_generation)?
                .ok_or_else(|| MergeRequestError::MergeResultNotFound(input.merge_result_id.clone()))?;
            Ok(RecordMergeResultOutcome { merge_result, replayed: false })
        })
    }

    pub fn complete(&self, input: CompleteMergeRequest) -> Result<CompletionOutcome> {
        for (name, value) in [
            ("operation_id", input.operation_id.as_str()),
            ("ticket_id", input.ticket_id.as_str()),
            ("revision_id", input.expected_revision_id.as_str()),
            ("merge_result_id", input.expected_merge_result_id.as_str()),
            (
                "observed_target_commit",
                input.observed_target_commit.as_str(),
            ),
            (
                "implementation_assignment_id",
                input.implementation_assignment_id.as_str(),
            ),
            (
                "completion_actor_runtime_id",
                input.completion_actor_runtime_id.as_str(),
            ),
            (
                "completion_actor_worker_id",
                input.completion_actor_worker_id.as_str(),
            ),
        ] {
            nonempty(name, value)?;
        }
        let fingerprint = completion_fingerprint(&input);
        self.write(|conn| {
            if let Some((stored, status, state)) = conn.query_row(
                "SELECT fingerprint, status, result_ticket_state FROM merge_request_completion_operations WHERE workspace_id=?1 AND operation_id=?2",
                params![self.workspace_id, input.operation_id],
                |row| Ok((row.get::<_,String>(0)?, row.get::<_,String>(1)?, row.get::<_,Option<String>>(2)?)),
            ).optional().map_err(db)? {
                if stored != fingerprint { return Err(MergeRequestError::OperationConflict); }
                if status == "completed" {
                    return Ok(CompletionOutcome { operation_id: input.operation_id.clone(), ticket_id: input.ticket_id.clone(), revision_id: input.expected_revision_id.clone(), ticket_state: state.unwrap_or_else(|| "done".into()), replayed: true });
                }
            } else {
                conn.execute(
                    "INSERT INTO merge_request_completion_operations (workspace_id, operation_id, ticket_id, revision_id, merge_result_id, authority_kind, implementation_assignment_id, completion_actor_runtime_id, completion_actor_worker_id, fingerprint, status, created_at, updated_at) VALUES (?1,?2,?3,?4,?5,'workspace_orchestrator',?6,?7,?8,?9,'pending',?10,?10)",
                    params![self.workspace_id, input.operation_id, input.ticket_id, input.expected_revision_id, input.expected_merge_result_id, input.implementation_assignment_id, input.completion_actor_runtime_id, input.completion_actor_worker_id, fingerprint, input.now],
                ).map_err(db)?;
            }
            let mr = load_merge_request(conn, &self.workspace_id, &input.ticket_id)?
                .ok_or_else(|| MergeRequestError::NotFound(input.ticket_id.clone()))?;
            validate_current_implementation_assignment(
                conn,
                &self.workspace_id,
                &input.ticket_id,
                &input.implementation_assignment_id,
            )?;
            ensure_open(&mr)?;
            if mr.current_revision.revision_id != input.expected_revision_id {
                return Err(MergeRequestError::StaleRevision { expected: input.expected_revision_id.clone(), current: mr.current_revision.revision_id });
            }
            if mr.review_status != ReviewStatus::Approved { return Err(MergeRequestError::NotApproved); }
            let final_result = mr.final_merge_result.as_ref().ok_or(MergeRequestError::FinalMergeResultMissing)?;
            if final_result.merge_result_id != input.expected_merge_result_id {
                return Err(MergeRequestError::MergeResultNotFinal);
            }
            if final_result.result_commit != input.observed_target_commit {
                return Err(MergeRequestError::FinalMergeResultNotApplied);
            }
            if matches!(final_result.strategy, MergeStrategy::Merge)
                && final_result.review_status != ReviewStatus::Approved
            {
                return Err(MergeRequestError::NotApproved);
            }
            let current_state: String = conn.query_row(
                "SELECT workflow_state FROM typed_tickets WHERE workspace_id=?1 AND ticket_id=?2",
                params![self.workspace_id, input.ticket_id], |row| row.get(0),
            ).optional().map_err(db)?.ok_or_else(|| MergeRequestError::NotFound(input.ticket_id.clone()))?;
            if current_state != "inprogress" {
                return Err(MergeRequestError::TicketStateConflict(current_state));
            }
            let changed = conn.execute(
                "UPDATE typed_tickets SET workflow_state='done', workflow_state_explicit=1, updated_at=?3 WHERE workspace_id=?1 AND ticket_id=?2 AND workflow_state='inprogress'",
                params![self.workspace_id, input.ticket_id, input.now],
            ).map_err(db)?;
            if changed != 1 { return Err(MergeRequestError::TicketStateConflict("concurrent_change".into())); }
            conn.execute(
                "UPDATE merge_requests SET state='merged',merged_at=?3,updated_at=?3 WHERE workspace_id=?1 AND merge_request_id=?2 AND current_revision_id=?4 AND state='open'",
                params![self.workspace_id,mr.merge_request_id,input.now,input.expected_revision_id],
            ).map_err(db)?;
            append_completion_event(conn, &self.workspace_id, &input)?;
            conn.execute(
                "UPDATE merge_request_completion_operations SET status='completed', result_ticket_state='done', updated_at=?3 WHERE workspace_id=?1 AND operation_id=?2 AND status='pending'",
                params![self.workspace_id, input.operation_id, input.now],
            ).map_err(db)?;
            Ok(CompletionOutcome { operation_id: input.operation_id, ticket_id: input.ticket_id, revision_id: input.expected_revision_id, ticket_state: "done".into(), replayed: false })
        })
    }

    pub fn close(
        &self,
        ticket_id: &str,
        expected_revision_id: &str,
        now: &str,
    ) -> Result<MergeRequest> {
        self.transition_open(ticket_id, expected_revision_id, "closed", now)
    }

    pub fn reopen(
        &self,
        ticket_id: &str,
        expected_revision_id: &str,
        now: &str,
    ) -> Result<MergeRequest> {
        self.write(|conn| {
            let mr = load_merge_request(conn, &self.workspace_id, ticket_id)?.ok_or_else(|| MergeRequestError::NotFound(ticket_id.into()))?;
            if mr.state != MergeRequestState::Closed { return Err(MergeRequestError::NotOpen(mr.state.as_str().into())); }
            if mr.current_revision.revision_id != expected_revision_id { return Err(MergeRequestError::StaleRevision { expected: expected_revision_id.into(), current: mr.current_revision.revision_id }); }
            conn.execute("UPDATE merge_requests SET state='open', lifecycle_generation=lifecycle_generation+1, updated_at=?3 WHERE workspace_id=?1 AND merge_request_id=?2 AND state='closed'", params![self.workspace_id, mr.merge_request_id, now]).map_err(db)?;
            load_merge_request(conn, &self.workspace_id, ticket_id)?.ok_or_else(|| MergeRequestError::NotFound(ticket_id.into()))
        })
    }

    fn transition_open(
        &self,
        ticket_id: &str,
        expected_revision_id: &str,
        state: &str,
        now: &str,
    ) -> Result<MergeRequest> {
        self.write(|conn| {
            let mr = load_merge_request(conn, &self.workspace_id, ticket_id)?.ok_or_else(|| MergeRequestError::NotFound(ticket_id.into()))?;
            ensure_open(&mr)?;
            if mr.current_revision.revision_id != expected_revision_id { return Err(MergeRequestError::StaleRevision { expected: expected_revision_id.into(), current: mr.current_revision.revision_id }); }
            conn.execute("UPDATE merge_requests SET state=?3, updated_at=?4 WHERE workspace_id=?1 AND merge_request_id=?2 AND state='open'", params![self.workspace_id, mr.merge_request_id, state, now]).map_err(db)?;
            load_merge_request(conn, &self.workspace_id, ticket_id)?.ok_or_else(|| MergeRequestError::NotFound(ticket_id.into()))
        })
    }
}

pub fn migrate(conn: &Connection) -> Result<()> {
    migrate_with_failpoint(conn, false)
}

fn migrate_with_failpoint(conn: &Connection, force_failure_after_v9_ddl: bool) -> Result<()> {
    let original_foreign_keys: i64 = conn
        .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
        .map_err(db)?;
    conn.pragma_update(None, "foreign_keys", "OFF")
        .map_err(db)?;

    let transaction_result = (|| {
        conn.execute_batch("BEGIN IMMEDIATE").map_err(db)?;
        let result = migrate_locked(conn, force_failure_after_v9_ddl);
        match result {
            Ok(()) => {
                if let Err(error) = conn.execute_batch("COMMIT").map_err(db) {
                    let _ = conn.execute_batch("ROLLBACK");
                    Err(error)
                } else {
                    Ok(())
                }
            }
            Err(error) => {
                let _ = conn.execute_batch("ROLLBACK");
                Err(error)
            }
        }
    })();

    let restore_result = conn
        .pragma_update(None, "foreign_keys", original_foreign_keys)
        .map_err(db);
    transaction_result?;
    restore_result?;
    verify(conn)
}

fn migrate_locked(conn: &Connection, force_failure_after_v9_ddl: bool) -> Result<()> {
    let marker_exists = table_exists(conn, MIGRATION_TABLE)?;
    if !marker_exists {
        if has_merge_request_domain_tables(conn)? {
            return Err(MergeRequestError::Database(
                "unsupported unversioned legacy merge request schema; automatic migration requires a fresh database or exact version 9"
                    .into(),
            ));
        }
        conn.execute_batch(MIGRATION_TABLE_SQL).map_err(db)?;
        conn.execute_batch(SCHEMA_V9).map_err(db)?;
        migrate_v9_to_v10(conn)?;
        verify_schema_v10(conn)?;
        ensure_foreign_key_integrity(conn)?;
        replace_schema_marker(conn, SCHEMA_VERSION)?;
        return verify(conn);
    }

    let version = schema_version(conn)?;
    match version {
        SCHEMA_VERSION => {
            verify_marker_state(conn, SCHEMA_VERSION)?;
            verify(conn)
        }
        9 => {
            verify_marker_state(conn, 9)?;
            verify_schema_shape(conn, SCHEMA_V9, "v9").map_err(|_| {
                MergeRequestError::Database(
                    "schema drift at merge request version 9; automatic migration requires the exact v9 shape"
                        .into(),
                )
            })?;
            migrate_v9_to_v10(conn)?;
            if force_failure_after_v9_ddl {
                return Err(MergeRequestError::Database(
                    "forced v9 to v10 migration failure after DDL".into(),
                ));
            }
            verify_schema_v10(conn)?;
            ensure_foreign_key_integrity(conn)?;
            replace_schema_marker(conn, SCHEMA_VERSION)?;
            verify(conn)
        }
        0..=8 => Err(MergeRequestError::Database(format!(
            "unsupported legacy merge request schema version {version}; automatic migration only supports exact v9 to v10"
        ))),
        other => Err(MergeRequestError::Database(format!(
            "unsupported merge request schema version {other}; expected version 9 or {SCHEMA_VERSION}"
        ))),
    }
}

fn migrate_v9_to_v10(conn: &Connection) -> Result<()> {
    conn.execute_batch(MIGRATE_V9_TO_V10_SQL).map_err(db)
}

fn verify_schema_v10(conn: &Connection) -> Result<()> {
    let expected = Connection::open_in_memory().map_err(db)?;
    expected.execute_batch(SCHEMA_V9).map_err(db)?;
    expected.execute_batch(MIGRATE_V9_TO_V10_SQL).map_err(db)?;
    let expected_shape = domain_schema_shape(&expected)?;
    let actual_shape = domain_schema_shape(conn)?;
    if actual_shape != expected_shape {
        return Err(MergeRequestError::Database(
            "schema drift: merge request v10 shape mismatch".into(),
        ));
    }
    Ok(())
}

pub fn verify(conn: &Connection) -> Result<()> {
    if !table_exists(conn, MIGRATION_TABLE)? {
        return Err(MergeRequestError::Database(
            "missing merge request schema version marker".into(),
        ));
    }
    let version = schema_version(conn)?;
    if version != SCHEMA_VERSION {
        return Err(MergeRequestError::Database(format!(
            "unsupported merge request schema version {version}; expected {SCHEMA_VERSION}"
        )));
    }
    verify_marker_state(conn, SCHEMA_VERSION)?;
    verify_schema_v10(conn)
}

fn schema_version(conn: &Connection) -> Result<i64> {
    conn.query_row(
        "SELECT COALESCE(MAX(version),0) FROM merge_request_schema_migrations",
        [],
        |row| row.get(0),
    )
    .map_err(db)
}

fn verify_marker_state(conn: &Connection, expected_version: i64) -> Result<()> {
    let expected = Connection::open_in_memory().map_err(db)?;
    expected.execute_batch(MIGRATION_TABLE_SQL).map_err(db)?;
    if table_shape(conn, MIGRATION_TABLE)? != table_shape(&expected, MIGRATION_TABLE)? {
        return Err(MergeRequestError::Database(
            "schema drift: merge request version marker table does not match the latest contract"
                .into(),
        ));
    }
    let state: (i64, i64, i64) = conn
        .query_row(
            "SELECT COUNT(*),COALESCE(MIN(version),0),COALESCE(MAX(version),0) FROM merge_request_schema_migrations",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(db)?;
    if state != (1, expected_version, expected_version) {
        return Err(MergeRequestError::Database(format!(
            "schema drift: merge request version marker must contain only version {expected_version}"
        )));
    }
    Ok(())
}

fn replace_schema_marker(conn: &Connection, version: i64) -> Result<()> {
    conn.execute("DELETE FROM merge_request_schema_migrations", [])
        .map_err(db)?;
    conn.execute(
        "INSERT INTO merge_request_schema_migrations(version) VALUES (?1)",
        params![version],
    )
    .map_err(db)?;
    Ok(())
}

fn ensure_foreign_key_integrity(conn: &Connection) -> Result<()> {
    let mut statement = conn.prepare("PRAGMA foreign_key_check").map_err(db)?;
    let mut rows = statement.query([]).map_err(db)?;
    if let Some(row) = rows.next().map_err(db)? {
        let table: String = row.get(0).map_err(db)?;
        let row_id: Option<i64> = row.get(1).map_err(db)?;
        let parent: String = row.get(2).map_err(db)?;
        return Err(MergeRequestError::Database(format!(
            "foreign key integrity check failed for table {table}, row {row_id:?}, parent {parent}"
        )));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ColumnShape {
    cid: i64,
    name: String,
    data_type: String,
    not_null: i64,
    default_value: Option<String>,
    primary_key: i64,
    hidden: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ForeignKeyShape {
    id: i64,
    sequence: i64,
    parent_table: String,
    from_column: String,
    to_column: Option<String>,
    on_update: String,
    on_delete: String,
    match_kind: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct IndexColumnShape {
    sequence: i64,
    column_id: i64,
    name: Option<String>,
    descending: i64,
    collation: Option<String>,
    key: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct IndexShape {
    unique: i64,
    origin: String,
    partial: i64,
    columns: Vec<IndexColumnShape>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct TableShape {
    name: String,
    columns: Vec<ColumnShape>,
    foreign_keys: Vec<ForeignKeyShape>,
    indexes: Vec<IndexShape>,
    checks: Vec<String>,
}

fn verify_schema_shape(conn: &Connection, expected_sql: &str, label: &str) -> Result<()> {
    let expected = Connection::open_in_memory().map_err(db)?;
    expected.execute_batch(expected_sql).map_err(db)?;
    let expected_shape = domain_schema_shape(&expected)?;
    let actual_shape = domain_schema_shape(conn)?;
    if actual_shape != expected_shape {
        let mismatch = expected_shape
            .iter()
            .zip(actual_shape.iter())
            .find(|(expected, actual)| expected != actual)
            .map(|(expected, actual)| {
                format!(" expected {}, observed {}", expected.name, actual.name)
            })
            .unwrap_or_else(|| {
                format!(
                    " expected {} tables, observed {}",
                    expected_shape.len(),
                    actual_shape.len()
                )
            });
        return Err(MergeRequestError::Database(format!(
            "schema drift: merge request {label} shape mismatch;{mismatch}"
        )));
    }
    Ok(())
}

fn domain_schema_shape(conn: &Connection) -> Result<Vec<TableShape>> {
    let mut statement = conn
        .prepare(
            "SELECT name FROM sqlite_master WHERE type='table' AND name LIKE 'merge_request_%' AND name <> ?1 ORDER BY name",
        )
        .map_err(db)?;
    let names = statement
        .query_map(params![MIGRATION_TABLE], |row| row.get::<_, String>(0))
        .map_err(db)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(db)?;
    names
        .into_iter()
        .map(|name| table_shape(conn, &name))
        .collect()
}

fn table_shape(conn: &Connection, table: &str) -> Result<TableShape> {
    let quoted = table.replace('\'', "''");
    let mut column_statement = conn
        .prepare(&format!("PRAGMA table_xinfo('{quoted}')"))
        .map_err(db)?;
    let columns = column_statement
        .query_map([], |row| {
            Ok(ColumnShape {
                cid: row.get(0)?,
                name: row.get(1)?,
                data_type: row.get(2)?,
                not_null: row.get(3)?,
                default_value: row.get(4)?,
                primary_key: row.get(5)?,
                hidden: row.get(6)?,
            })
        })
        .map_err(db)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(db)?;

    let mut foreign_key_statement = conn
        .prepare(&format!("PRAGMA foreign_key_list('{quoted}')"))
        .map_err(db)?;
    let mut foreign_keys = foreign_key_statement
        .query_map([], |row| {
            Ok(ForeignKeyShape {
                id: row.get(0)?,
                sequence: row.get(1)?,
                parent_table: row.get(2)?,
                from_column: row.get(3)?,
                to_column: row.get(4)?,
                on_update: row.get(5)?,
                on_delete: row.get(6)?,
                match_kind: row.get(7)?,
            })
        })
        .map_err(db)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(db)?;
    foreign_keys.sort();

    let mut index_statement = conn
        .prepare(&format!("PRAGMA index_list('{quoted}')"))
        .map_err(db)?;
    let index_rows = index_statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })
        .map_err(db)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(db)?;
    let mut indexes = Vec::with_capacity(index_rows.len());
    for (index_name, unique, origin, partial) in index_rows {
        let index_quoted = index_name.replace('\'', "''");
        let mut columns_statement = conn
            .prepare(&format!("PRAGMA index_xinfo('{index_quoted}')"))
            .map_err(db)?;
        let columns = columns_statement
            .query_map([], |row| {
                Ok(IndexColumnShape {
                    sequence: row.get(0)?,
                    column_id: row.get(1)?,
                    name: row.get(2)?,
                    descending: row.get(3)?,
                    collation: row.get(4)?,
                    key: row.get(5)?,
                })
            })
            .map_err(db)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(db)?;
        indexes.push(IndexShape {
            unique,
            origin,
            partial,
            columns,
        });
    }
    indexes.sort();
    let create_sql: String = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type='table' AND name=?1",
            params![table],
            |row| row.get(0),
        )
        .map_err(db)?;
    Ok(TableShape {
        name: table.to_string(),
        columns,
        foreign_keys,
        indexes,
        checks: extract_check_constraints(&create_sql),
    })
}

fn extract_check_constraints(sql: &str) -> Vec<String> {
    let bytes = sql.as_bytes();
    let lower = sql.to_ascii_lowercase();
    let lower_bytes = lower.as_bytes();
    let mut checks = Vec::new();
    let mut cursor = 0;
    while cursor + 5 <= bytes.len() {
        let Some(relative) = lower[cursor..].find("check") else {
            break;
        };
        let start = cursor + relative;
        let mut open = start + 5;
        while open < bytes.len() && bytes[open].is_ascii_whitespace() {
            open += 1;
        }
        if open >= bytes.len() || bytes[open] != b'(' {
            cursor = start + 5;
            continue;
        }
        let mut depth = 0_i32;
        let mut quoted = false;
        let mut end = open;
        while end < bytes.len() {
            let byte = bytes[end];
            if byte == b'\'' {
                if quoted && end + 1 < bytes.len() && bytes[end + 1] == b'\'' {
                    end += 2;
                    continue;
                }
                quoted = !quoted;
            } else if !quoted {
                if byte == b'(' {
                    depth += 1;
                } else if byte == b')' {
                    depth -= 1;
                    if depth == 0 {
                        end += 1;
                        break;
                    }
                }
            }
            end += 1;
        }
        if depth == 0 {
            checks.push(
                lower_bytes[open..end]
                    .iter()
                    .filter(|byte| !byte.is_ascii_whitespace())
                    .map(|byte| *byte as char)
                    .collect(),
            );
        }
        cursor = end.max(start + 5);
    }
    checks.sort();
    checks
}

fn has_merge_request_domain_tables(conn: &Connection) -> Result<bool> {
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name LIKE 'merge_request_%' AND name <> ?1",
            params![MIGRATION_TABLE],
            |row| row.get(0),
        )
        .map_err(db)?;
    Ok(count > 0)
}

fn table_exists(conn: &Connection, table: &str) -> Result<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
        params![table],
        |row| row.get::<_, i64>(0),
    )
    .map(|value| value != 0)
    .map_err(db)
}

#[cfg(test)]
fn column_exists(conn: &Connection, table: &str, column: &str) -> Result<bool> {
    let mut statement = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(db)?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(db)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(db)?;
    Ok(columns.iter().any(|candidate| candidate == column))
}

const MIGRATION_TABLE: &str = "merge_request_schema_migrations";
const MIGRATION_TABLE_SQL: &str = "CREATE TABLE merge_request_schema_migrations (version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP);";
const SCHEMA_V9: &str = r#"
CREATE TABLE merge_requests (
 workspace_id TEXT NOT NULL, merge_request_id TEXT NOT NULL,
 repository_id TEXT NOT NULL, state TEXT NOT NULL CHECK(state IN ('draft','open','closed','merged')),
 lifecycle_generation INTEGER NOT NULL, current_revision_id TEXT NOT NULL,
 created_at TEXT NOT NULL, updated_at TEXT NOT NULL, merged_by_account_id TEXT, merged_at TEXT,
 target_ref_selector TEXT,
 target_status TEXT NOT NULL DEFAULT 'unknown' CHECK(target_status IN ('known','unknown')),
 PRIMARY KEY(workspace_id,merge_request_id),
 FOREIGN KEY(workspace_id,repository_id) REFERENCES repositories(workspace_id,repository_id)
);
CREATE TABLE merge_request_ticket_relations (
 workspace_id TEXT NOT NULL, merge_request_id TEXT NOT NULL, ticket_id TEXT NOT NULL,
 relation_kind TEXT NOT NULL CHECK(relation_kind='implements'), created_at TEXT NOT NULL,
 PRIMARY KEY(workspace_id,merge_request_id,ticket_id),
 FOREIGN KEY(workspace_id,merge_request_id) REFERENCES merge_requests(workspace_id,merge_request_id) ON DELETE CASCADE,
 FOREIGN KEY(workspace_id,ticket_id) REFERENCES typed_tickets(workspace_id,ticket_id) ON DELETE CASCADE
);
CREATE TABLE merge_request_revisions (
 workspace_id TEXT NOT NULL, merge_request_id TEXT NOT NULL, revision_id TEXT NOT NULL,
 ordinal INTEGER NOT NULL, base_commit TEXT NOT NULL, head_commit TEXT NOT NULL,
 diff_digest TEXT NOT NULL, summary TEXT NOT NULL, assignment_id TEXT NOT NULL, created_at TEXT NOT NULL,
 PRIMARY KEY(workspace_id,merge_request_id,revision_id), UNIQUE(workspace_id,merge_request_id,ordinal),
 FOREIGN KEY(workspace_id,merge_request_id) REFERENCES merge_requests(workspace_id,merge_request_id) ON DELETE CASCADE
);
CREATE TABLE merge_request_revision_paths (
 workspace_id TEXT NOT NULL, merge_request_id TEXT NOT NULL, revision_id TEXT NOT NULL, ordinal INTEGER NOT NULL, path TEXT NOT NULL,
 PRIMARY KEY(workspace_id,merge_request_id,revision_id,ordinal),
 FOREIGN KEY(workspace_id,merge_request_id,revision_id) REFERENCES merge_request_revisions(workspace_id,merge_request_id,revision_id) ON DELETE CASCADE
);
CREATE TABLE merge_request_reviewer_child_sessions (
 workspace_id TEXT NOT NULL, child_session_id TEXT NOT NULL, parent_runtime_id TEXT NOT NULL,
 parent_worker_id TEXT NOT NULL, effective_profile TEXT NOT NULL CHECK(effective_profile='builtin:reviewer'), registered_at TEXT NOT NULL,
 PRIMARY KEY(workspace_id,child_session_id)
);
CREATE TABLE merge_request_review_attempts (
 workspace_id TEXT NOT NULL, attempt_id TEXT NOT NULL, merge_request_id TEXT NOT NULL, ticket_id TEXT NOT NULL,
 revision_id TEXT NOT NULL, lifecycle_generation INTEGER NOT NULL,
 parent_assignment_id TEXT NOT NULL, parent_runtime_id TEXT NOT NULL, parent_worker_id TEXT NOT NULL,
 child_session_id TEXT NOT NULL, child_effective_profile TEXT NOT NULL CHECK(child_effective_profile='builtin:reviewer'),
 capability_token_sha256 TEXT NOT NULL, status TEXT NOT NULL CHECK(status IN ('open','submitted','revoked')),
 created_at TEXT NOT NULL, consumed_at TEXT, merge_result_id TEXT,
 PRIMARY KEY(workspace_id,attempt_id), UNIQUE(workspace_id,capability_token_sha256), UNIQUE(workspace_id,child_session_id),
 FOREIGN KEY(workspace_id,merge_request_id,revision_id) REFERENCES merge_request_revisions(workspace_id,merge_request_id,revision_id),
 FOREIGN KEY(workspace_id,ticket_id,parent_assignment_id) REFERENCES ticket_worker_assignments(workspace_id,ticket_id,assignment_id),
 FOREIGN KEY(workspace_id,child_session_id) REFERENCES merge_request_reviewer_child_sessions(workspace_id,child_session_id),
 FOREIGN KEY(workspace_id,merge_result_id) REFERENCES merge_request_merge_results(workspace_id,merge_result_id)
);
CREATE TABLE merge_request_reviews (
 workspace_id TEXT NOT NULL, attempt_id TEXT NOT NULL, merge_request_id TEXT NOT NULL, revision_id TEXT NOT NULL,
 decision TEXT NOT NULL CHECK(decision IN ('approve','request_changes')), body TEXT NOT NULL, submitted_at TEXT NOT NULL,
 merge_result_id TEXT,
 PRIMARY KEY(workspace_id,attempt_id),
 FOREIGN KEY(workspace_id,attempt_id) REFERENCES merge_request_review_attempts(workspace_id,attempt_id),
 FOREIGN KEY(workspace_id,merge_request_id,revision_id) REFERENCES merge_request_revisions(workspace_id,merge_request_id,revision_id),
 FOREIGN KEY(workspace_id,merge_result_id) REFERENCES merge_request_merge_results(workspace_id,merge_result_id)
);
CREATE TABLE merge_request_review_findings (
 workspace_id TEXT NOT NULL, attempt_id TEXT NOT NULL, ordinal INTEGER NOT NULL, severity TEXT NOT NULL,
 code TEXT, path TEXT, line INTEGER, body TEXT NOT NULL, PRIMARY KEY(workspace_id,attempt_id,ordinal),
 FOREIGN KEY(workspace_id,attempt_id) REFERENCES merge_request_reviews(workspace_id,attempt_id) ON DELETE CASCADE
);
CREATE TABLE merge_request_merge_results (
 workspace_id TEXT NOT NULL, merge_result_id TEXT NOT NULL, merge_request_id TEXT NOT NULL,
 ticket_id TEXT NOT NULL, revision_id TEXT NOT NULL, target_commit TEXT NOT NULL,
 source_commit TEXT NOT NULL, result_commit TEXT NOT NULL,
 strategy TEXT NOT NULL CHECK(strategy IN ('fast_forward','merge')),
 resolution TEXT NOT NULL CHECK(resolution IN ('none','clean','conflicts_resolved')),
 created_by_runtime_id TEXT NOT NULL, created_by_worker_id TEXT NOT NULL,
 created_at TEXT NOT NULL, operation_id TEXT NOT NULL, operation_fingerprint TEXT NOT NULL,
 validated_at TEXT NOT NULL,
 PRIMARY KEY(workspace_id,merge_result_id),
 UNIQUE(workspace_id,operation_id),
 FOREIGN KEY(workspace_id,merge_request_id,revision_id)
   REFERENCES merge_request_revisions(workspace_id,merge_request_id,revision_id),
 FOREIGN KEY(workspace_id,ticket_id) REFERENCES typed_tickets(workspace_id,ticket_id)
);
CREATE INDEX merge_request_merge_results_current_idx
 ON merge_request_merge_results(workspace_id,merge_request_id,revision_id,target_commit,created_at);
CREATE TABLE merge_request_completion_operations (
 workspace_id TEXT NOT NULL, operation_id TEXT NOT NULL, ticket_id TEXT NOT NULL, revision_id TEXT NOT NULL,
 authority_kind TEXT NOT NULL CHECK(authority_kind IN ('workspace_orchestrator','legacy_assigned_coder')),
 implementation_assignment_id TEXT NOT NULL, completion_actor_runtime_id TEXT, completion_actor_worker_id TEXT,
 fingerprint TEXT NOT NULL, status TEXT NOT NULL CHECK(status IN ('pending','completed')),
 result_ticket_state TEXT, created_at TEXT NOT NULL, updated_at TEXT NOT NULL,
 PRIMARY KEY(workspace_id,operation_id),
 FOREIGN KEY(workspace_id,ticket_id) REFERENCES typed_tickets(workspace_id,ticket_id),
 FOREIGN KEY(workspace_id,ticket_id,implementation_assignment_id)
   REFERENCES ticket_worker_assignments(workspace_id,ticket_id,assignment_id)
);
"#;

const MIGRATE_V9_TO_V10_SQL: &str = r#"
CREATE TABLE merge_request_final_results (
 workspace_id TEXT NOT NULL, merge_request_id TEXT NOT NULL, revision_id TEXT NOT NULL,
 merge_result_id TEXT NOT NULL, selected_at TEXT NOT NULL,
 PRIMARY KEY(workspace_id,merge_request_id),
 FOREIGN KEY(workspace_id,merge_request_id,revision_id)
   REFERENCES merge_request_revisions(workspace_id,merge_request_id,revision_id) ON DELETE CASCADE,
 FOREIGN KEY(workspace_id,merge_result_id)
   REFERENCES merge_request_merge_results(workspace_id,merge_result_id)
);
ALTER TABLE merge_request_completion_operations ADD COLUMN merge_result_id TEXT;
"#;

#[cfg(test)]
mod migration_tests {
    use super::*;

    const SUPPORT_SCHEMA: &str = r#"
CREATE TABLE repositories(
 workspace_id TEXT NOT NULL, repository_id TEXT NOT NULL,
 PRIMARY KEY(workspace_id,repository_id)
);
CREATE TABLE typed_tickets(
 workspace_id TEXT NOT NULL, ticket_id TEXT NOT NULL, workflow_state TEXT NOT NULL,
 workflow_state_explicit INTEGER NOT NULL DEFAULT 1, updated_at TEXT NOT NULL,
 PRIMARY KEY(workspace_id,ticket_id)
);
CREATE TABLE ticket_worker_assignments(
 workspace_id TEXT NOT NULL, ticket_id TEXT NOT NULL, assignment_id TEXT NOT NULL,
 runtime_id TEXT NOT NULL, worker_id TEXT NOT NULL,
 PRIMARY KEY(workspace_id,ticket_id,assignment_id)
);
"#;

    fn fresh_connection() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SUPPORT_SCHEMA).unwrap();
        conn
    }

    fn exact_v9_connection() -> Connection {
        let conn = fresh_connection();
        conn.execute_batch(MIGRATION_TABLE_SQL).unwrap();
        conn.execute(
            "INSERT INTO merge_request_schema_migrations(version) VALUES(9)",
            [],
        )
        .unwrap();
        conn.execute_batch(SCHEMA_V9).unwrap();
        conn.execute_batch(
            "INSERT INTO repositories VALUES('ws','repo');
             INSERT INTO typed_tickets VALUES('ws','T1','inprogress',1,'t0');
             INSERT INTO ticket_worker_assignments VALUES('ws','T1','A1','R1','W1');
             INSERT INTO merge_requests VALUES('ws','MR1','repo','open',3,'V1','t0','t1',NULL,NULL,'refs/heads/develop','known');
             INSERT INTO merge_request_ticket_relations VALUES('ws','MR1','T1','implements','t0');
             INSERT INTO merge_request_revisions VALUES('ws','MR1','V1',1,'base','head','digest','summary','A1','t0');
             INSERT INTO merge_request_merge_results VALUES('ws','R1','MR1','T1','V1','base','head','head','fast_forward','none','R1','W1','t1','OPR1','fp1','t1');
             INSERT INTO merge_request_merge_results VALUES('ws','R2','MR1','T1','V1','base','head','merge','merge','clean','R1','W1','t2','OPR2','fp2','t2');
             INSERT INTO merge_request_completion_operations VALUES('ws','OP1','T1','V1','workspace_orchestrator','A1','OR','OW','fingerprint','pending',NULL,'t0','t1');",
        )
        .unwrap();
        conn
    }

    fn marker_version(conn: &Connection) -> i64 {
        conn.query_row(
            "SELECT MAX(version) FROM merge_request_schema_migrations",
            [],
            |row| row.get(0),
        )
        .unwrap()
    }

    #[test]
    fn fresh_database_materializes_latest_v10_contract() {
        let conn = fresh_connection();
        migrate(&conn).unwrap();
        verify(&conn).unwrap();
        assert_eq!(marker_version(&conn), 10);
        assert!(table_exists(&conn, "merge_request_final_results").unwrap());
        assert!(
            column_exists(
                &conn,
                "merge_request_completion_operations",
                "merge_result_id"
            )
            .unwrap()
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
            0
        );
    }

    #[test]
    fn exact_v9_migrates_without_guessing_a_final_candidate() {
        let conn = exact_v9_connection();
        migrate(&conn).unwrap();
        verify(&conn).unwrap();
        assert_eq!(marker_version(&conn), 10);
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM merge_request_merge_results",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            2
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM merge_request_final_results",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            0,
            "migration must not guess which historical candidate is final"
        );
        assert_eq!(
            conn.query_row("SELECT merge_result_id FROM merge_request_completion_operations WHERE operation_id='OP1'", [], |row| row.get::<_, Option<String>>(0)).unwrap(),
            None
        );
    }

    #[test]
    fn v9_to_v10_failure_rolls_back_schema_and_marker() {
        let conn = exact_v9_connection();
        let error = migrate_with_failpoint(&conn, true).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("forced v9 to v10 migration failure")
        );
        assert_eq!(marker_version(&conn), 9);
        assert!(!table_exists(&conn, "merge_request_final_results").unwrap());
        assert!(
            !column_exists(
                &conn,
                "merge_request_completion_operations",
                "merge_result_id"
            )
            .unwrap()
        );
        verify_schema_shape(&conn, SCHEMA_V9, "v9 after rollback").unwrap();
    }

    #[test]
    fn drifted_v9_fails_closed_without_mutation() {
        let conn = exact_v9_connection();
        conn.execute_batch("ALTER TABLE merge_requests ADD COLUMN drift TEXT;")
            .unwrap();
        let error = migrate(&conn).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("schema drift at merge request version 9")
        );
        assert_eq!(marker_version(&conn), 9);
        assert!(!table_exists(&conn, "merge_request_final_results").unwrap());
    }

    #[test]
    fn versions_older_than_v9_are_rejected() {
        let conn = fresh_connection();
        conn.execute_batch(MIGRATION_TABLE_SQL).unwrap();
        conn.execute(
            "INSERT INTO merge_request_schema_migrations(version) VALUES(8)",
            [],
        )
        .unwrap();
        let error = migrate(&conn).unwrap_err();
        assert!(error.to_string().contains("only supports exact v9 to v10"));
        assert_eq!(marker_version(&conn), 8);
    }
}

fn load_merge_request(
    conn: &Connection,
    workspace_id: &str,
    ticket_id: &str,
) -> Result<Option<MergeRequest>> {
    let row: Option<(String,String,String,Option<String>,String,String,i64,String,String,String,Option<String>,Option<String>)> = conn.query_row(
        "SELECT mr.merge_request_id,rel.ticket_id,mr.repository_id,mr.target_ref_selector,mr.target_status,mr.state,mr.lifecycle_generation,mr.current_revision_id,mr.created_at,mr.updated_at,mr.merged_by_account_id,mr.merged_at FROM merge_requests mr JOIN merge_request_ticket_relations rel ON rel.workspace_id=mr.workspace_id AND rel.merge_request_id=mr.merge_request_id WHERE mr.workspace_id=?1 AND rel.ticket_id=?2 AND rel.relation_kind='implements' ORDER BY mr.updated_at DESC,mr.merge_request_id DESC LIMIT 1",
        params![workspace_id,ticket_id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?,r.get(8)?,r.get(9)?,r.get(10)?,r.get(11)?)),
    ).optional().map_err(db)?;
    let Some((
        mr_id,
        ticket_id,
        repository_id,
        target_ref_selector,
        target_status,
        state,
        generation,
        revision_id,
        created_at,
        updated_at,
        merged_by_account_id,
        merged_at,
    )) = row
    else {
        return Ok(None);
    };
    let revision = load_revision(conn, workspace_id, &mr_id, &revision_id)?;
    let current_review = load_latest_review(conn, workspace_id, &mr_id, &revision_id, generation)?;
    let review_status = match current_review.as_ref().map(|review| review.decision) {
        Some(ReviewDecision::Approve) => ReviewStatus::Approved,
        Some(ReviewDecision::RequestChanges) => ReviewStatus::ChangesRequested,
        None => ReviewStatus::Pending,
    };
    let merge_results =
        load_merge_results(conn, workspace_id, &mr_id, &revision_id, generation as u64)?;
    let final_merge_result_id: Option<String> = conn
        .query_row(
            "SELECT merge_result_id FROM merge_request_final_results WHERE workspace_id=?1 AND merge_request_id=?2 AND revision_id=?3",
            params![workspace_id,mr_id,revision_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(db)?;
    let final_merge_result = final_merge_result_id.and_then(|id| {
        merge_results
            .iter()
            .find(|result| result.merge_result_id == id)
            .cloned()
    });
    Ok(Some(MergeRequest {
        merge_request_id: mr_id,
        workspace_id: workspace_id.into(),
        ticket_id,
        repository_id,
        target_ref_selector,
        target_status: MergeRequestTargetStatus::parse(&target_status),
        observed_target_commit: None,
        state: MergeRequestState::parse(&state),
        lifecycle_generation: generation as u64,
        current_revision: revision,
        review_status,
        current_review,
        merge_results,
        final_merge_result,
        created_at,
        updated_at,
        merged_by_account_id,
        merged_at,
    }))
}

fn load_revision(
    conn: &Connection,
    workspace_id: &str,
    mr_id: &str,
    revision_id: &str,
) -> Result<MergeRequestRevision> {
    let mut revision: MergeRequestRevision = conn.query_row(
        "SELECT revision_id,ordinal,base_commit,head_commit,diff_digest,summary,assignment_id,created_at FROM merge_request_revisions WHERE workspace_id=?1 AND merge_request_id=?2 AND revision_id=?3",
        params![workspace_id,mr_id,revision_id], |r| Ok(MergeRequestRevision { revision_id:r.get(0)?, ordinal:r.get::<_,i64>(1)? as u64, base_commit:r.get(2)?, head_commit:r.get(3)?, diff_digest:r.get(4)?, changed_paths:Vec::new(), summary:r.get(5)?, assignment_id:r.get(6)?, created_at:r.get(7)? }),
    ).map_err(db)?;
    let mut statement = conn.prepare("SELECT path FROM merge_request_revision_paths WHERE workspace_id=?1 AND merge_request_id=?2 AND revision_id=?3 ORDER BY ordinal").map_err(db)?;
    revision.changed_paths = statement
        .query_map(params![workspace_id, mr_id, revision_id], |r| r.get(0))
        .map_err(db)?
        .collect::<std::result::Result<Vec<String>, _>>()
        .map_err(db)?;
    Ok(revision)
}

fn insert_revision(
    conn: &Connection,
    workspace_id: &str,
    mr_id: &str,
    revision: &MergeRequestRevision,
) -> Result<()> {
    conn.execute("INSERT INTO merge_request_revisions (workspace_id,merge_request_id,revision_id,ordinal,base_commit,head_commit,diff_digest,summary,assignment_id,created_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)", params![workspace_id,mr_id,revision.revision_id,revision.ordinal as i64,revision.base_commit,revision.head_commit,revision.diff_digest,revision.summary,revision.assignment_id,revision.created_at]).map_err(db)?;
    for (ordinal, path) in revision.changed_paths.iter().enumerate() {
        conn.execute("INSERT INTO merge_request_revision_paths (workspace_id,merge_request_id,revision_id,ordinal,path) VALUES (?1,?2,?3,?4,?5)", params![workspace_id,mr_id,revision.revision_id,ordinal as i64,path]).map_err(db)?;
    }
    Ok(())
}

fn load_latest_review(
    conn: &Connection,
    workspace_id: &str,
    mr_id: &str,
    revision_id: &str,
    generation: i64,
) -> Result<Option<MergeRequestReview>> {
    let attempt: Option<String> = conn.query_row("SELECT r.attempt_id FROM merge_request_reviews r JOIN merge_request_review_attempts a ON a.workspace_id=r.workspace_id AND a.attempt_id=r.attempt_id WHERE r.workspace_id=?1 AND r.merge_request_id=?2 AND r.revision_id=?3 AND r.merge_result_id IS NULL AND a.lifecycle_generation=?4 ORDER BY r.submitted_at DESC, r.attempt_id DESC LIMIT 1", params![workspace_id,mr_id,revision_id,generation], |r| r.get(0)).optional().map_err(db)?;
    match attempt {
        Some(id) => load_review(conn, workspace_id, &id),
        None => Ok(None),
    }
}

fn load_review(
    conn: &Connection,
    workspace_id: &str,
    attempt_id: &str,
) -> Result<Option<MergeRequestReview>> {
    let row: Option<(String,Option<String>,String,String,String,String,String,String,String,String)> = conn.query_row(
        "SELECT r.revision_id,r.merge_result_id,r.decision,r.body,a.parent_assignment_id,a.parent_runtime_id,a.parent_worker_id,a.child_session_id,a.child_effective_profile,r.submitted_at FROM merge_request_reviews r JOIN merge_request_review_attempts a ON a.workspace_id=r.workspace_id AND a.attempt_id=r.attempt_id WHERE r.workspace_id=?1 AND r.attempt_id=?2",
        params![workspace_id,attempt_id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?,r.get(8)?,r.get(9)?)),
    ).optional().map_err(db)?;
    let Some((
        revision_id,
        merge_result_id,
        decision,
        body,
        assignment,
        runtime,
        worker,
        child,
        profile,
        submitted_at,
    )) = row
    else {
        return Ok(None);
    };
    let mut stmt=conn.prepare("SELECT severity,code,path,line,body FROM merge_request_review_findings WHERE workspace_id=?1 AND attempt_id=?2 ORDER BY ordinal").map_err(db)?;
    let findings = stmt
        .query_map(params![workspace_id, attempt_id], |r| {
            Ok(ReviewFinding {
                severity: r.get(0)?,
                code: r.get(1)?,
                path: r.get(2)?,
                line: r.get::<_, Option<i64>>(3)?.map(|v| v as u64),
                body: r.get(4)?,
            })
        })
        .map_err(db)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(db)?;
    Ok(Some(MergeRequestReview {
        attempt_id: attempt_id.into(),
        revision_id,
        merge_result_id,
        decision: ReviewDecision::parse(&decision),
        body,
        findings,
        parent_assignment_id: assignment,
        parent_runtime_id: runtime,
        parent_worker_id: worker,
        reviewer_child_session_id: child,
        reviewer_effective_profile: profile,
        submitted_at,
    }))
}

fn validate_current_implementation_assignment(
    conn: &Connection,
    workspace_id: &str,
    ticket_id: &str,
    assignment_id: &str,
) -> Result<()> {
    let current: Option<String> = conn
        .query_row(
            "SELECT assignment_id FROM ticket_current_worker_assignments WHERE workspace_id=?1 AND ticket_id=?2",
            params![workspace_id, ticket_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(db)?;
    if current.as_deref() != Some(assignment_id) {
        return Err(MergeRequestError::AssignmentMismatch);
    }
    Ok(())
}

fn validate_current_assignment_id(
    conn: &Connection,
    workspace_id: &str,
    ticket_id: &str,
    assignment_id: &str,
) -> Result<()> {
    validate_current_implementation_assignment(conn, workspace_id, ticket_id, assignment_id)
}

fn validate_current_assignment(
    conn: &Connection,
    workspace_id: &str,
    ticket_id: &str,
    assignment_id: &str,
    runtime_id: &str,
    worker_id: &str,
) -> Result<()> {
    let valid: Option<i64> = conn.query_row("SELECT 1 FROM ticket_current_worker_assignments WHERE workspace_id=?1 AND ticket_id=?2 AND assignment_id=?3 AND runtime_id=?4 AND worker_id=?5", params![workspace_id,ticket_id,assignment_id,runtime_id,worker_id], |r| r.get(0)).optional().map_err(db)?;
    if valid.is_none() {
        return Err(MergeRequestError::AssignmentMismatch);
    }
    Ok(())
}

fn append_completion_event(
    conn: &Connection,
    workspace_id: &str,
    input: &CompleteMergeRequest,
) -> Result<()> {
    let index:i64=conn.query_row("SELECT COALESCE(MAX(event_index),-1)+1 FROM typed_ticket_events WHERE workspace_id=?1 AND ticket_id=?2",params![workspace_id,input.ticket_id],|r|r.get(0)).map_err(db)?;
    conn.execute("INSERT INTO typed_ticket_events (workspace_id,ticket_id,event_index,kind,author,at,from_state,to_state,heading,body) VALUES (?1,?2,?3,'state_changed',?4,?5,'inprogress','done','Merge Request completed',?6)",params![workspace_id,input.ticket_id,index,format!("worker:{}:{}",input.completion_actor_runtime_id,input.completion_actor_worker_id),input.now,format!("Approved immutable revision `{}` completed implementation.",input.expected_revision_id)]).map_err(db)?;
    for (key, value) in [
        (
            "implementation_assignment_id",
            input.implementation_assignment_id.as_str(),
        ),
        (
            "merge_request_revision_id",
            input.expected_revision_id.as_str(),
        ),
        ("operation_id", input.operation_id.as_str()),
        ("completion_authority", "workspace_orchestrator"),
        ("runtime_id", input.completion_actor_runtime_id.as_str()),
        ("worker_id", input.completion_actor_worker_id.as_str()),
    ] {
        conn.execute("INSERT INTO typed_ticket_event_attributes (workspace_id,ticket_id,event_index,key,value) VALUES (?1,?2,?3,?4,?5)",params![workspace_id,input.ticket_id,index,key,value]).map_err(db)?;
    }
    Ok(())
}

fn validate_revision(revision: &MergeRequestRevision) -> Result<()> {
    for (name, value) in [
        ("revision_id", revision.revision_id.as_str()),
        ("base_commit", revision.base_commit.as_str()),
        ("head_commit", revision.head_commit.as_str()),
        ("diff_digest", revision.diff_digest.as_str()),
        ("assignment_id", revision.assignment_id.as_str()),
    ] {
        nonempty(name, value)?;
    }
    if revision.ordinal == 0 {
        return Err(MergeRequestError::Empty("revision.ordinal"));
    }
    if revision.summary.len() > MAX_SUMMARY_BYTES {
        return Err(MergeRequestError::TooLarge {
            field: "revision.summary",
            max: MAX_SUMMARY_BYTES,
        });
    }
    if revision.changed_paths.len() > MAX_CHANGED_PATHS {
        return Err(MergeRequestError::TooLarge {
            field: "revision.changed_paths",
            max: MAX_CHANGED_PATHS,
        });
    }
    for path in &revision.changed_paths {
        nonempty("changed_path", path)?;
        if path.len() > MAX_FIELD_BYTES {
            return Err(MergeRequestError::TooLarge {
                field: "changed_path",
                max: MAX_FIELD_BYTES,
            });
        }
        if Path::new(path).is_absolute() || path.split('/').any(|p| p == "..") {
            return Err(MergeRequestError::Empty("changed_path"));
        }
    }
    Ok(())
}

fn merge_result_fingerprint(input: &RecordMergeResult) -> String {
    token_hash(&format!(
        "{}\0{}\0{}\0{}\0{}\0{}\0{}\0{}\0{}",
        input.ticket_id,
        input.expected_revision_id,
        input.target_commit,
        input.source_commit,
        input.result_commit,
        input.strategy.as_str(),
        input.resolution.as_str(),
        input.actor_runtime_id,
        input.actor_worker_id,
    ))
}

fn apply_target_observation(mr: &mut MergeRequest, observed_target_commit: Option<&str>) {
    mr.observed_target_commit = observed_target_commit.map(str::to_owned);
    for result in &mut mr.merge_results {
        result.target_status = match observed_target_commit {
            Some(commit) if result.target_commit == commit => MergeResultTargetStatus::Current,
            Some(commit) if result.result_commit == commit => MergeResultTargetStatus::Applied,
            Some(_) => MergeResultTargetStatus::Stale,
            None => MergeResultTargetStatus::Unknown,
        };
    }
    let final_id = mr
        .final_merge_result
        .as_ref()
        .map(|result| result.merge_result_id.clone());
    mr.final_merge_result = final_id.and_then(|id| {
        mr.merge_results
            .iter()
            .find(|result| result.merge_result_id == id)
            .cloned()
    });
}

fn load_merge_result(
    conn: &Connection,
    workspace_id: &str,
    merge_result_id: &str,
    generation: u64,
) -> Result<Option<MergeResult>> {
    let row: Option<(String,String,String,String,String,String,String,String,String,String,String)> = conn.query_row(
        "SELECT revision_id,target_commit,source_commit,result_commit,strategy,resolution,created_by_runtime_id,created_by_worker_id,created_at,operation_id,validated_at FROM merge_request_merge_results WHERE workspace_id=?1 AND merge_result_id=?2",
        params![workspace_id,merge_result_id],
        |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?,r.get(8)?,r.get(9)?,r.get(10)?)),
    ).optional().map_err(db)?;
    let Some((
        revision_id,
        target_commit,
        source_commit,
        result_commit,
        strategy,
        resolution,
        created_by_runtime_id,
        created_by_worker_id,
        created_at,
        operation_id,
        validated_at,
    )) = row
    else {
        return Ok(None);
    };
    let current_review =
        load_latest_merge_result_review(conn, workspace_id, merge_result_id, generation)?;
    let review_status =
        current_review
            .as_ref()
            .map_or(ReviewStatus::Pending, |review| match review.decision {
                ReviewDecision::Approve => ReviewStatus::Approved,
                ReviewDecision::RequestChanges => ReviewStatus::ChangesRequested,
            });
    Ok(Some(MergeResult {
        merge_result_id: merge_result_id.into(),
        revision_id,
        target_commit,
        source_commit,
        result_commit,
        strategy: MergeStrategy::parse(&strategy),
        resolution: MergeResolution::parse(&resolution),
        created_by_runtime_id,
        created_by_worker_id,
        created_at,
        operation_id,
        validated_at,
        target_status: MergeResultTargetStatus::Unknown,
        review_status,
        current_review,
    }))
}

fn load_merge_results(
    conn: &Connection,
    workspace_id: &str,
    merge_request_id: &str,
    revision_id: &str,
    generation: u64,
) -> Result<Vec<MergeResult>> {
    let mut statement = conn.prepare(
        "SELECT merge_result_id FROM merge_request_merge_results WHERE workspace_id=?1 AND merge_request_id=?2 AND revision_id=?3 ORDER BY created_at,merge_result_id",
    ).map_err(db)?;
    let ids = statement
        .query_map(
            params![workspace_id, merge_request_id, revision_id],
            |row| row.get::<_, String>(0),
        )
        .map_err(db)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(db)?;
    ids.into_iter()
        .map(|id| {
            load_merge_result(conn, workspace_id, &id, generation)?
                .ok_or(MergeRequestError::MergeResultNotFound(id))
        })
        .collect()
}

fn load_latest_merge_result_review(
    conn: &Connection,
    workspace_id: &str,
    merge_result_id: &str,
    generation: u64,
) -> Result<Option<MergeRequestReview>> {
    let attempt: Option<String> = conn.query_row(
        "SELECT r.attempt_id FROM merge_request_reviews r JOIN merge_request_review_attempts a ON a.workspace_id=r.workspace_id AND a.attempt_id=r.attempt_id WHERE r.workspace_id=?1 AND r.merge_result_id=?2 AND a.lifecycle_generation=?3 ORDER BY r.submitted_at DESC,r.attempt_id DESC LIMIT 1",
        params![workspace_id,merge_result_id,generation as i64], |row| row.get(0),
    ).optional().map_err(db)?;
    attempt
        .map(|attempt| load_review(conn, workspace_id, &attempt))
        .transpose()
        .map(|review| review.flatten())
}

fn validate_review_input(input: &SubmitReview) -> Result<()> {
    if input.body.len() > MAX_REVIEW_BODY_BYTES {
        return Err(MergeRequestError::TooLarge {
            field: "review.body",
            max: MAX_REVIEW_BODY_BYTES,
        });
    }
    if input.findings.len() > MAX_FINDINGS {
        return Err(MergeRequestError::TooLarge {
            field: "review.findings",
            max: MAX_FINDINGS,
        });
    }
    for finding in &input.findings {
        nonempty("finding.severity", &finding.severity)?;
        nonempty("finding.body", &finding.body)?;
        for (field, value) in [
            ("finding.severity", Some(finding.severity.as_str())),
            ("finding.code", finding.code.as_deref()),
            ("finding.path", finding.path.as_deref()),
            ("finding.body", Some(finding.body.as_str())),
        ] {
            if value.is_some_and(|value| value.len() > MAX_FIELD_BYTES) {
                return Err(MergeRequestError::TooLarge {
                    field,
                    max: MAX_FIELD_BYTES,
                });
            }
        }
    }
    Ok(())
}

fn ensure_open(mr: &MergeRequest) -> Result<()> {
    if mr.state != MergeRequestState::Open {
        Err(MergeRequestError::NotOpen(mr.state.as_str().into()))
    } else {
        Ok(())
    }
}
fn nonempty(name: &'static str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        Err(MergeRequestError::Empty(name))
    } else {
        Ok(())
    }
}
fn token_hash(token: &str) -> String {
    Sha256::digest(token.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
fn completion_fingerprint(input: &CompleteMergeRequest) -> String {
    token_hash(&format!(
        "workspace_orchestrator\0{}\0{}\0{}\0{}\0{}\0{}\0{}",
        input.ticket_id,
        input.expected_revision_id,
        input.expected_merge_result_id,
        input.observed_target_commit,
        input.implementation_assignment_id,
        input.completion_actor_runtime_id,
        input.completion_actor_worker_id
    ))
}
fn db(error: rusqlite::Error) -> MergeRequestError {
    MergeRequestError::Database(error.to_string())
}
