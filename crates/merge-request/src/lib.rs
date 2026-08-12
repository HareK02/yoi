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

const SCHEMA_VERSION: i64 = 7;
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
    #[error("Ticket must be inprogress before Merge Request completion (current: {0})")]
    TicketStateConflict(String),
    #[error("only an authenticated user with explicit confirmation may merge")]
    MergeConfirmationRequired,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeRequestRevision {
    pub revision_id: String,
    pub ordinal: u64,
    pub base_commit: String,
    pub head_commit: String,
    pub head_tree: String,
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
pub struct MergeRequest {
    pub merge_request_id: String,
    pub workspace_id: String,
    pub ticket_id: String,
    pub repository_id: String,
    pub state: MergeRequestState,
    pub lifecycle_generation: u64,
    pub current_revision: MergeRequestRevision,
    pub review_status: ReviewStatus,
    pub current_review: Option<MergeRequestReview>,
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
    pub capability_token: String,
    pub decision: ReviewDecision,
    pub body: String,
    pub findings: Vec<ReviewFinding>,
    pub now: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompleteMergeRequest {
    pub operation_id: String,
    pub ticket_id: String,
    pub expected_revision_id: String,
    pub assignment_id: String,
    pub authenticated_runtime_id: String,
    pub authenticated_worker_id: String,
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
    pub ready: bool,
    pub review_status: ReviewStatus,
    pub blockers: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeConfirmation {
    pub ticket_id: String,
    pub expected_revision_id: String,
    pub authenticated_account_id: String,
    pub actor_kind: String,
    pub explicit_confirmation: bool,
    pub now: String,
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
        nonempty("ticket_id", ticket_id)?;
        let conn = self.connect()?;
        verify(&conn)?;
        load_merge_request(&conn, &self.workspace_id, ticket_id)
    }

    pub fn readiness_for_ticket(&self, ticket_id: &str) -> Result<MergeRequestReadiness> {
        let mr = self
            .show_for_ticket(ticket_id)?
            .ok_or_else(|| MergeRequestError::NotFound(ticket_id.to_string()))?;
        let mut blockers = Vec::new();
        if mr.state != MergeRequestState::Open {
            blockers.push(format!("merge request is {}", mr.state.as_str()));
        }
        match mr.review_status {
            ReviewStatus::Pending => blockers.push("current revision has no review result".into()),
            ReviewStatus::ChangesRequested => {
                blockers.push("current revision has request_changes".into())
            }
            ReviewStatus::Approved => {}
        }
        Ok(MergeRequestReadiness {
            ticket_id: ticket_id.to_string(),
            merge_request_id: mr.merge_request_id,
            revision_id: mr.current_revision.revision_id,
            ready: blockers.is_empty(),
            review_status: mr.review_status,
            blockers,
        })
    }

    pub fn open_merge_request(&self, input: OpenMergeRequest) -> Result<MergeRequest> {
        validate_revision(&input.revision)?;
        for (name, value) in [
            ("merge_request_id", input.merge_request_id.as_str()),
            ("ticket_id", input.ticket_id.as_str()),
            ("repository_id", input.repository_id.as_str()),
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
                "INSERT INTO merge_requests (workspace_id, merge_request_id, repository_id, state, lifecycle_generation, current_revision_id, created_at, updated_at) VALUES (?1, ?2, ?3, 'open', 1, ?4, ?5, ?5)",
                params![self.workspace_id, input.merge_request_id, input.repository_id, input.revision.revision_id, input.now],
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
            let existing: Option<(String, String, String, String)> = conn.query_row(
                "SELECT base_commit, head_commit, head_tree, diff_digest FROM merge_request_revisions WHERE workspace_id=?1 AND merge_request_id=?2 AND revision_id=?3",
                params![self.workspace_id, current.merge_request_id, input.revision.revision_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            ).optional().map_err(db)?;
            if let Some(existing) = existing {
                if existing == (input.revision.base_commit.clone(), input.revision.head_commit.clone(), input.revision.head_tree.clone(), input.revision.diff_digest.clone()) {
                    return Ok(current);
                }
                return Err(MergeRequestError::RevisionConflict(input.revision.revision_id.clone()));
            }
            insert_revision(conn, &self.workspace_id, &current.merge_request_id, &input.revision)?;
            conn.execute(
                "UPDATE merge_requests SET current_revision_id=?3, updated_at=?4 WHERE workspace_id=?1 AND merge_request_id=?2 AND current_revision_id=?5",
                params![self.workspace_id, current.merge_request_id, input.revision.revision_id, input.now, input.expected_current_revision_id],
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
            validate_current_assignment(conn, &self.workspace_id, &input.ticket_id, &input.parent_assignment_id, &input.parent_runtime_id, &input.parent_worker_id)?;
            let effective_profile: Option<String> = conn.query_row(
                "SELECT effective_profile FROM merge_request_reviewer_child_sessions WHERE workspace_id=?1 AND child_session_id=?2 AND parent_runtime_id=?3 AND parent_worker_id=?4",
                params![self.workspace_id,input.child_session_id,input.parent_runtime_id,input.parent_worker_id],
                |row| row.get(0),
            ).optional().map_err(db)?;
            if effective_profile.as_deref() != Some(REVIEWER_PROFILE) {
                return Err(MergeRequestError::InvalidReviewer);
            }
            conn.execute(
                "INSERT INTO merge_request_review_attempts (workspace_id, attempt_id, merge_request_id, ticket_id, revision_id, lifecycle_generation, parent_assignment_id, parent_runtime_id, parent_worker_id, child_session_id, child_effective_profile, capability_token_sha256, status, created_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,'open',?13)",
                params![self.workspace_id, input.attempt_id, mr.merge_request_id, input.ticket_id, input.revision_id, mr.lifecycle_generation as i64, input.parent_assignment_id, input.parent_runtime_id, input.parent_worker_id, input.child_session_id, REVIEWER_PROFILE, token_hash(&input.capability_token), input.now],
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
            let attempt: Option<(String,String,String,String,String,String,String,String,i64)> = conn.query_row(
                "SELECT attempt_id, merge_request_id, parent_assignment_id, parent_runtime_id, parent_worker_id, child_session_id, child_effective_profile, status, lifecycle_generation FROM merge_request_review_attempts WHERE workspace_id=?1 AND ticket_id=?2 AND revision_id=?3 AND capability_token_sha256=?4",
                params![self.workspace_id, input.ticket_id, input.revision_id, token],
                |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?,row.get(6)?,row.get(7)?,row.get(8)?)),
            ).optional().map_err(db)?;
            let Some((attempt_id, mr_id, assignment_id, runtime_id, worker_id, child_session_id, effective_profile, status, lifecycle_generation)) = attempt else {
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
            validate_current_assignment(conn, &self.workspace_id, &input.ticket_id, &assignment_id, &runtime_id, &worker_id)?;
            conn.execute(
                "INSERT INTO merge_request_reviews (workspace_id, attempt_id, merge_request_id, revision_id, decision, body, submitted_at) VALUES (?1,?2,?3,?4,?5,?6,?7)",
                params![self.workspace_id, attempt_id, mr_id, input.revision_id, input.decision.as_str(), input.body, input.now],
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

    pub fn complete(&self, input: CompleteMergeRequest) -> Result<CompletionOutcome> {
        for (name, value) in [
            ("operation_id", input.operation_id.as_str()),
            ("ticket_id", input.ticket_id.as_str()),
            ("revision_id", input.expected_revision_id.as_str()),
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
                    "INSERT INTO merge_request_completion_operations (workspace_id, operation_id, ticket_id, revision_id, assignment_id, fingerprint, status, created_at, updated_at) VALUES (?1,?2,?3,?4,?5,?6,'pending',?7,?7)",
                    params![self.workspace_id, input.operation_id, input.ticket_id, input.expected_revision_id, input.assignment_id, fingerprint, input.now],
                ).map_err(db)?;
            }
            let mr = load_merge_request(conn, &self.workspace_id, &input.ticket_id)?
                .ok_or_else(|| MergeRequestError::NotFound(input.ticket_id.clone()))?;
            ensure_open(&mr)?;
            if mr.current_revision.revision_id != input.expected_revision_id {
                return Err(MergeRequestError::StaleRevision { expected: input.expected_revision_id.clone(), current: mr.current_revision.revision_id });
            }
            validate_current_assignment(conn, &self.workspace_id, &input.ticket_id, &input.assignment_id, &input.authenticated_runtime_id, &input.authenticated_worker_id)?;
            if mr.review_status != ReviewStatus::Approved { return Err(MergeRequestError::NotApproved); }
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

    pub fn confirm_merge(&self, input: MergeConfirmation) -> Result<MergeRequest> {
        if !input.explicit_confirmation
            || input.actor_kind != "user"
            || input.authenticated_account_id.trim().is_empty()
        {
            return Err(MergeRequestError::MergeConfirmationRequired);
        }
        self.write(|conn| {
            let mr = load_merge_request(conn, &self.workspace_id, &input.ticket_id)?.ok_or_else(|| MergeRequestError::NotFound(input.ticket_id.clone()))?;
            ensure_open(&mr)?;
            if mr.current_revision.revision_id != input.expected_revision_id { return Err(MergeRequestError::StaleRevision { expected: input.expected_revision_id.clone(), current: mr.current_revision.revision_id }); }
            if mr.review_status != ReviewStatus::Approved { return Err(MergeRequestError::NotApproved); }
            let ticket_state: String = conn.query_row("SELECT workflow_state FROM typed_tickets WHERE workspace_id=?1 AND ticket_id=?2", params![self.workspace_id, input.ticket_id], |row| row.get(0)).map_err(db)?;
            if ticket_state != "done" { return Err(MergeRequestError::TicketStateConflict(ticket_state)); }
            conn.execute("UPDATE merge_requests SET state='merged', merged_by_account_id=?3, merged_at=?4, updated_at=?4 WHERE workspace_id=?1 AND merge_request_id=?2 AND state='open'", params![self.workspace_id, mr.merge_request_id, input.authenticated_account_id, input.now]).map_err(db)?;
            load_merge_request(conn, &self.workspace_id, &input.ticket_id)?.ok_or_else(|| MergeRequestError::NotFound(input.ticket_id.clone()))
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
    conn.pragma_update(None, "foreign_keys", "ON").map_err(db)?;
    conn.execute_batch("CREATE TABLE IF NOT EXISTS merge_request_schema_migrations (version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP);").map_err(db)?;
    let version: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(version),0) FROM merge_request_schema_migrations",
            [],
            |row| row.get(0),
        )
        .map_err(db)?;
    archive_incompatible_legacy_tables(conn, version)?;
    conn.execute_batch(SCHEMA_V1).map_err(db)?;
    if version < 1 {
        conn.execute(
            "INSERT INTO merge_request_schema_migrations(version) VALUES (1)",
            [],
        )
        .map_err(db)?;
    }
    if version < SCHEMA_VERSION {
        // Version 6 is the fresh bounded-context authority marker. Versions 1..=5
        // were emitted by the retired implementation; their relational evidence is
        // preserved and revalidated by the current typed store rather than rewritten.
        if column_exists(conn, "merge_request_schema_migrations", "name")? {
            conn.execute(
                "INSERT OR IGNORE INTO merge_request_schema_migrations(version,name) VALUES (?1,'fresh_bounded_context_authority')",
                params![SCHEMA_VERSION],
            ).map_err(db)?;
        } else {
            conn.execute(
                "INSERT OR IGNORE INTO merge_request_schema_migrations(version) VALUES (?1)",
                params![SCHEMA_VERSION],
            )
            .map_err(db)?;
        }
    }
    verify(conn)
}

pub fn verify(conn: &Connection) -> Result<()> {
    let version: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(version),0) FROM merge_request_schema_migrations",
            [],
            |row| row.get(0),
        )
        .map_err(db)?;
    if !(1..=SCHEMA_VERSION).contains(&version) {
        return Err(MergeRequestError::Database(format!(
            "unsupported merge request schema version {version}, expected at most {SCHEMA_VERSION}"
        )));
    }
    for table in [
        "merge_requests",
        "merge_request_ticket_relations",
        "merge_request_revisions",
        "merge_request_revision_paths",
        "merge_request_reviewer_child_sessions",
        "merge_request_review_attempts",
        "merge_request_reviews",
        "merge_request_review_findings",
        "merge_request_completion_operations",
    ] {
        let present: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1",
                params![table],
                |row| row.get(0),
            )
            .optional()
            .map_err(db)?;
        if present.is_none() {
            return Err(MergeRequestError::Database(format!(
                "missing table {table}"
            )));
        }
    }
    for (table, required) in [
        (
            "merge_requests",
            &[
                "workspace_id",
                "merge_request_id",
                "repository_id",
                "state",
                "lifecycle_generation",
                "current_revision_id",
            ] as &[_],
        ),
        (
            "merge_request_ticket_relations",
            &[
                "workspace_id",
                "merge_request_id",
                "ticket_id",
                "relation_kind",
            ] as &[_],
        ),
        (
            "merge_request_revisions",
            &[
                "workspace_id",
                "merge_request_id",
                "revision_id",
                "ordinal",
                "base_commit",
                "head_commit",
                "head_tree",
                "diff_digest",
                "assignment_id",
            ] as &[_],
        ),
        (
            "merge_request_reviewer_child_sessions",
            &[
                "workspace_id",
                "child_session_id",
                "parent_runtime_id",
                "parent_worker_id",
                "effective_profile",
            ] as &[_],
        ),
        (
            "merge_request_review_attempts",
            &[
                "workspace_id",
                "attempt_id",
                "merge_request_id",
                "ticket_id",
                "revision_id",
                "lifecycle_generation",
                "parent_assignment_id",
                "parent_runtime_id",
                "parent_worker_id",
                "child_session_id",
                "child_effective_profile",
                "capability_token_sha256",
                "status",
            ] as &[_],
        ),
        (
            "merge_request_reviews",
            &[
                "workspace_id",
                "attempt_id",
                "merge_request_id",
                "revision_id",
                "decision",
                "body",
            ] as &[_],
        ),
        (
            "merge_request_completion_operations",
            &[
                "workspace_id",
                "operation_id",
                "ticket_id",
                "revision_id",
                "assignment_id",
                "fingerprint",
                "status",
                "result_ticket_state",
            ] as &[_],
        ),
    ] {
        let mut statement = conn
            .prepare(&format!("PRAGMA table_info({table})"))
            .map_err(db)?;
        let columns = statement
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(db)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(db)?;
        for column in required {
            if !columns.iter().any(|actual| actual == column) {
                return Err(MergeRequestError::Database(format!(
                    "schema drift: table {table} is missing required column {column}"
                )));
            }
        }
    }
    Ok(())
}

const SCHEMA_V1: &str = r#"
CREATE TABLE IF NOT EXISTS merge_requests (
 workspace_id TEXT NOT NULL, merge_request_id TEXT NOT NULL,
 repository_id TEXT NOT NULL, state TEXT NOT NULL CHECK(state IN ('draft','open','closed','merged')),
 lifecycle_generation INTEGER NOT NULL, current_revision_id TEXT NOT NULL,
 created_at TEXT NOT NULL, updated_at TEXT NOT NULL, merged_by_account_id TEXT, merged_at TEXT,
 PRIMARY KEY(workspace_id,merge_request_id),
 FOREIGN KEY(workspace_id,repository_id) REFERENCES repositories(workspace_id,repository_id)
);
CREATE TABLE IF NOT EXISTS merge_request_ticket_relations (
 workspace_id TEXT NOT NULL, merge_request_id TEXT NOT NULL, ticket_id TEXT NOT NULL,
 relation_kind TEXT NOT NULL CHECK(relation_kind='implements'), created_at TEXT NOT NULL,
 PRIMARY KEY(workspace_id,merge_request_id,ticket_id),
 FOREIGN KEY(workspace_id,merge_request_id) REFERENCES merge_requests(workspace_id,merge_request_id) ON DELETE CASCADE,
 FOREIGN KEY(workspace_id,ticket_id) REFERENCES typed_tickets(workspace_id,ticket_id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS merge_request_revisions (
 workspace_id TEXT NOT NULL, merge_request_id TEXT NOT NULL, revision_id TEXT NOT NULL,
 ordinal INTEGER NOT NULL, base_commit TEXT NOT NULL, head_commit TEXT NOT NULL, head_tree TEXT NOT NULL, diff_digest TEXT NOT NULL,
 summary TEXT NOT NULL, assignment_id TEXT NOT NULL, created_at TEXT NOT NULL,
 PRIMARY KEY(workspace_id,merge_request_id,revision_id), UNIQUE(workspace_id,merge_request_id,ordinal),
 FOREIGN KEY(workspace_id,merge_request_id) REFERENCES merge_requests(workspace_id,merge_request_id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS merge_request_revision_paths (
 workspace_id TEXT NOT NULL, merge_request_id TEXT NOT NULL, revision_id TEXT NOT NULL, ordinal INTEGER NOT NULL, path TEXT NOT NULL,
 PRIMARY KEY(workspace_id,merge_request_id,revision_id,ordinal),
 FOREIGN KEY(workspace_id,merge_request_id,revision_id) REFERENCES merge_request_revisions(workspace_id,merge_request_id,revision_id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS merge_request_reviewer_child_sessions (
 workspace_id TEXT NOT NULL, child_session_id TEXT NOT NULL, parent_runtime_id TEXT NOT NULL,
 parent_worker_id TEXT NOT NULL, effective_profile TEXT NOT NULL CHECK(effective_profile='builtin:reviewer'), registered_at TEXT NOT NULL,
 PRIMARY KEY(workspace_id,child_session_id)
);
CREATE TABLE IF NOT EXISTS merge_request_review_attempts (
 workspace_id TEXT NOT NULL, attempt_id TEXT NOT NULL, merge_request_id TEXT NOT NULL, ticket_id TEXT NOT NULL,
 revision_id TEXT NOT NULL, lifecycle_generation INTEGER NOT NULL,
 parent_assignment_id TEXT NOT NULL, parent_runtime_id TEXT NOT NULL, parent_worker_id TEXT NOT NULL,
 child_session_id TEXT NOT NULL, child_effective_profile TEXT NOT NULL CHECK(child_effective_profile='builtin:reviewer'),
 capability_token_sha256 TEXT NOT NULL, status TEXT NOT NULL CHECK(status IN ('open','submitted','revoked')),
 created_at TEXT NOT NULL, consumed_at TEXT,
 PRIMARY KEY(workspace_id,attempt_id), UNIQUE(workspace_id,capability_token_sha256), UNIQUE(workspace_id,child_session_id),
 FOREIGN KEY(workspace_id,merge_request_id,revision_id) REFERENCES merge_request_revisions(workspace_id,merge_request_id,revision_id),
 FOREIGN KEY(workspace_id,ticket_id,parent_assignment_id) REFERENCES ticket_worker_assignments(workspace_id,ticket_id,assignment_id)
);
CREATE TABLE IF NOT EXISTS merge_request_reviews (
 workspace_id TEXT NOT NULL, attempt_id TEXT NOT NULL, merge_request_id TEXT NOT NULL, revision_id TEXT NOT NULL,
 decision TEXT NOT NULL CHECK(decision IN ('approve','request_changes')), body TEXT NOT NULL, submitted_at TEXT NOT NULL,
 PRIMARY KEY(workspace_id,attempt_id),
 FOREIGN KEY(workspace_id,attempt_id) REFERENCES merge_request_review_attempts(workspace_id,attempt_id),
 FOREIGN KEY(workspace_id,merge_request_id,revision_id) REFERENCES merge_request_revisions(workspace_id,merge_request_id,revision_id)
);
CREATE TABLE IF NOT EXISTS merge_request_review_findings (
 workspace_id TEXT NOT NULL, attempt_id TEXT NOT NULL, ordinal INTEGER NOT NULL, severity TEXT NOT NULL,
 code TEXT, path TEXT, line INTEGER, body TEXT NOT NULL, PRIMARY KEY(workspace_id,attempt_id,ordinal),
 FOREIGN KEY(workspace_id,attempt_id) REFERENCES merge_request_reviews(workspace_id,attempt_id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS merge_request_completion_operations (
 workspace_id TEXT NOT NULL, operation_id TEXT NOT NULL, ticket_id TEXT NOT NULL, revision_id TEXT NOT NULL,
 assignment_id TEXT NOT NULL, fingerprint TEXT NOT NULL, status TEXT NOT NULL CHECK(status IN ('pending','completed')),
 result_ticket_state TEXT, created_at TEXT NOT NULL, updated_at TEXT NOT NULL,
 PRIMARY KEY(workspace_id,operation_id),
 FOREIGN KEY(workspace_id,ticket_id) REFERENCES typed_tickets(workspace_id,ticket_id)
);
"#;

fn archive_incompatible_legacy_tables(conn: &Connection, version: i64) -> Result<()> {
    if version == 0 || !table_exists(conn, "merge_requests")? {
        return Ok(());
    }
    let incompatible = column_exists(conn, "merge_requests", "ticket_id")?
        || !table_has_columns(
            conn,
            "merge_requests",
            &[
                "workspace_id",
                "merge_request_id",
                "repository_id",
                "state",
                "lifecycle_generation",
                "current_revision_id",
            ],
        )?
        || !table_has_columns(
            conn,
            "merge_request_ticket_relations",
            &[
                "workspace_id",
                "merge_request_id",
                "ticket_id",
                "relation_kind",
            ],
        )?
        || !table_has_columns(
            conn,
            "merge_request_revisions",
            &[
                "workspace_id",
                "merge_request_id",
                "revision_id",
                "ordinal",
                "base_commit",
                "head_commit",
                "head_tree",
                "diff_digest",
                "assignment_id",
            ],
        )?;
    if !incompatible {
        return Ok(());
    }
    let tables = [
        "merge_request_review_findings",
        "merge_request_reviews",
        "merge_request_review_attempts",
        "merge_request_reviewer_child_sessions",
        "merge_request_completion_operations",
        "merge_request_revision_paths",
        "merge_request_ticket_relations",
        "merge_request_revisions",
        "merge_requests",
    ];
    conn.pragma_update(None, "foreign_keys", "OFF")
        .map_err(db)?;
    for table in tables {
        if !table_exists(conn, table)? {
            continue;
        }
        let archive = format!("legacy_v6_{table}");
        if table_exists(conn, &archive)? {
            conn.pragma_update(None, "foreign_keys", "ON").map_err(db)?;
            return Err(MergeRequestError::Database(format!(
                "legacy archive table {archive} already exists"
            )));
        }
        conn.execute_batch(&format!("ALTER TABLE {table} RENAME TO {archive};"))
            .map_err(db)?;
    }
    conn.pragma_update(None, "foreign_keys", "ON").map_err(db)?;
    Ok(())
}

fn table_has_columns(conn: &Connection, table: &str, required: &[&str]) -> Result<bool> {
    if !table_exists(conn, table)? {
        return Ok(false);
    }
    for column in required {
        if !column_exists(conn, table, column)? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn table_exists(conn: &Connection, table: &str) -> Result<bool> {
    let present: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1",
            params![table],
            |row| row.get(0),
        )
        .optional()
        .map_err(db)?;
    Ok(present.is_some())
}

fn column_exists(conn: &Connection, table: &str, column: &str) -> Result<bool> {
    let mut statement = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(db)?;
    let names = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(db)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(db)?;
    Ok(names.iter().any(|name| name == column))
}

fn load_merge_request(
    conn: &Connection,
    workspace_id: &str,
    ticket_id: &str,
) -> Result<Option<MergeRequest>> {
    let row: Option<(String,String,String,String,i64,String,String,String,Option<String>,Option<String>)> = conn.query_row(
        "SELECT mr.merge_request_id,rel.ticket_id,mr.repository_id,mr.state,mr.lifecycle_generation,mr.current_revision_id,mr.created_at,mr.updated_at,mr.merged_by_account_id,mr.merged_at FROM merge_requests mr JOIN merge_request_ticket_relations rel ON rel.workspace_id=mr.workspace_id AND rel.merge_request_id=mr.merge_request_id WHERE mr.workspace_id=?1 AND rel.ticket_id=?2 AND rel.relation_kind='implements' ORDER BY mr.updated_at DESC,mr.merge_request_id DESC LIMIT 1",
        params![workspace_id,ticket_id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?,r.get(8)?,r.get(9)?)),
    ).optional().map_err(db)?;
    let Some((
        mr_id,
        ticket_id,
        repository_id,
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
    Ok(Some(MergeRequest {
        merge_request_id: mr_id,
        workspace_id: workspace_id.into(),
        ticket_id,
        repository_id,
        state: MergeRequestState::parse(&state),
        lifecycle_generation: generation as u64,
        current_revision: revision,
        review_status,
        current_review,
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
        "SELECT revision_id,ordinal,base_commit,head_commit,head_tree,diff_digest,summary,assignment_id,created_at FROM merge_request_revisions WHERE workspace_id=?1 AND merge_request_id=?2 AND revision_id=?3",
        params![workspace_id,mr_id,revision_id], |r| Ok(MergeRequestRevision { revision_id:r.get(0)?, ordinal:r.get::<_,i64>(1)? as u64, base_commit:r.get(2)?, head_commit:r.get(3)?, head_tree:r.get(4)?, diff_digest:r.get(5)?, changed_paths:Vec::new(), summary:r.get(6)?, assignment_id:r.get(7)?, created_at:r.get(8)? }),
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
    conn.execute("INSERT INTO merge_request_revisions (workspace_id,merge_request_id,revision_id,ordinal,base_commit,head_commit,head_tree,diff_digest,summary,assignment_id,created_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)", params![workspace_id,mr_id,revision.revision_id,revision.ordinal as i64,revision.base_commit,revision.head_commit,revision.head_tree,revision.diff_digest,revision.summary,revision.assignment_id,revision.created_at]).map_err(db)?;
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
    let attempt: Option<String> = conn.query_row("SELECT r.attempt_id FROM merge_request_reviews r JOIN merge_request_review_attempts a ON a.workspace_id=r.workspace_id AND a.attempt_id=r.attempt_id WHERE r.workspace_id=?1 AND r.merge_request_id=?2 AND r.revision_id=?3 AND a.lifecycle_generation=?4 ORDER BY r.submitted_at DESC, r.attempt_id DESC LIMIT 1", params![workspace_id,mr_id,revision_id,generation], |r| r.get(0)).optional().map_err(db)?;
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
    let row: Option<(String,String,String,String,String,String,String,String,String)> = conn.query_row(
        "SELECT r.revision_id,r.decision,r.body,a.parent_assignment_id,a.parent_runtime_id,a.parent_worker_id,a.child_session_id,a.child_effective_profile,r.submitted_at FROM merge_request_reviews r JOIN merge_request_review_attempts a ON a.workspace_id=r.workspace_id AND a.attempt_id=r.attempt_id WHERE r.workspace_id=?1 AND r.attempt_id=?2",
        params![workspace_id,attempt_id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?,r.get(8)?)),
    ).optional().map_err(db)?;
    let Some((
        revision_id,
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
    conn.execute("INSERT INTO typed_ticket_events (workspace_id,ticket_id,event_index,kind,author,at,from_state,to_state,heading,body) VALUES (?1,?2,?3,'state_changed',?4,?5,'inprogress','done','Merge Request completed',?6)",params![workspace_id,input.ticket_id,index,format!("worker:{}:{}",input.authenticated_runtime_id,input.authenticated_worker_id),input.now,format!("Approved immutable revision `{}` completed implementation.",input.expected_revision_id)]).map_err(db)?;
    for (key, value) in [
        ("assignment_id", input.assignment_id.as_str()),
        (
            "merge_request_revision_id",
            input.expected_revision_id.as_str(),
        ),
        ("operation_id", input.operation_id.as_str()),
        ("runtime_id", input.authenticated_runtime_id.as_str()),
        ("worker_id", input.authenticated_worker_id.as_str()),
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
        ("head_tree", revision.head_tree.as_str()),
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
        "{}\0{}\0{}\0{}\0{}",
        input.ticket_id,
        input.expected_revision_id,
        input.assignment_id,
        input.authenticated_runtime_id,
        input.authenticated_worker_id
    ))
}
fn db(error: rusqlite::Error) -> MergeRequestError {
    MergeRequestError::Database(error.to_string())
}
