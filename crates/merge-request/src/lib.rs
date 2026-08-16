use std::path::Path;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use thiserror::Error;

const SCHEMA_VERSION: i64 = 12;
const PREVIOUS_SCHEMA_VERSION: i64 = 11;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MergeRequestState {
    Open,
    Merged,
    Closed,
}

impl MergeRequestState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Merged => "merged",
            Self::Closed => "closed",
        }
    }

    fn parse(value: &str) -> Result<Self, MergeRequestError> {
        match value {
            "open" | "draft" => Ok(Self::Open),
            "merged" => Ok(Self::Merged),
            "closed" => Ok(Self::Closed),
            other => Err(MergeRequestError::Corrupt(format!(
                "unknown merge request state `{other}`"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewDecision {
    Approve,
    RequestChanges,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingSeverity {
    Blocker,
    Major,
    Minor,
    Note,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewFinding {
    pub severity: FindingSeverity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerIdentity {
    pub runtime_id: String,
    pub worker_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeRequestAuth {
    pub workspace_id: String,
    pub repository_id: String,
    pub runtime_id: String,
    pub worker_id: String,
    pub assignment_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestForReview {
    pub base_commit: String,
    pub head_commit: String,
    #[serde(default)]
    pub changed_paths: Vec<String>,
    #[serde(default)]
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestForReviewEvent {
    pub event_seq: u64,
    pub base_commit: String,
    pub head_commit: String,
    pub changed_paths: Vec<String>,
    pub summary: String,
    pub assignment_id: String,
    pub requested_by: WorkerIdentity,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewEvent {
    pub event_seq: u64,
    pub request_event_seq: u64,
    pub head_commit: String,
    pub reviewer_worker: WorkerIdentity,
    pub reviewer_profile: String,
    pub decision: ReviewDecision,
    pub body: String,
    pub findings: Vec<ReviewFinding>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MergeStrategy {
    FastForward,
    Merge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictResolution {
    None,
    Clean,
    ConflictsResolved,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeEvent {
    pub event_seq: u64,
    pub operation_id: String,
    pub target_commit: String,
    pub source_commit: String,
    pub result_commit: String,
    pub strategy: MergeStrategy,
    pub resolution: ConflictResolution,
    pub merged_by: WorkerIdentity,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LifecycleEvent {
    pub event_seq: u64,
    pub actor: WorkerIdentity,
    #[serde(default)]
    pub body: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MergeRequestThreadEvent {
    RequestForReview(RequestForReviewEvent),
    Review(ReviewEvent),
    Merge(MergeEvent),
    Reopen(LifecycleEvent),
    Close(LifecycleEvent),
}

impl MergeRequestThreadEvent {
    pub fn event_seq(&self) -> u64 {
        match self {
            Self::RequestForReview(value) => value.event_seq,
            Self::Review(value) => value.event_seq,
            Self::Merge(value) => value.event_seq,
            Self::Reopen(value) | Self::Close(value) => value.event_seq,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeRequest {
    pub workspace_id: String,
    pub merge_request_id: String,
    pub ticket_id: String,
    pub repository_id: String,
    pub state: MergeRequestState,
    pub selector_from: String,
    pub selector_to: String,
    pub opened_by_worker: WorkerIdentity,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub thread: Vec<MergeRequestThreadEvent>,
}

impl MergeRequest {
    pub fn current_request(&self) -> Option<&RequestForReviewEvent> {
        let reopened_at = self.thread.iter().rev().find_map(|event| match event {
            MergeRequestThreadEvent::Reopen(value) => Some(value.event_seq),
            _ => None,
        });
        self.thread.iter().rev().find_map(|event| match event {
            MergeRequestThreadEvent::RequestForReview(value)
                if reopened_at.is_none_or(|seq| value.event_seq > seq) =>
            {
                Some(value)
            }
            _ => None,
        })
    }

    pub fn current_review(&self) -> Option<&ReviewEvent> {
        let request = self.current_request()?;
        self.thread.iter().rev().find_map(|event| match event {
            MergeRequestThreadEvent::Review(value)
                if value.request_event_seq == request.event_seq =>
            {
                Some(value)
            }
            _ => None,
        })
    }
}

#[derive(Debug, Clone)]
pub struct OpenMergeRequest {
    pub merge_request_id: String,
    pub ticket_id: String,
    pub repository_id: String,
    pub selector_from: String,
    pub selector_to: String,
    pub request: RequestForReview,
    pub auth: MergeRequestAuth,
    pub now: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct RequestMergeRequestReview {
    pub ticket_id: String,
    pub expected_head_commit: String,
    pub request: RequestForReview,
    pub auth: MergeRequestAuth,
    pub now: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct RegisterReviewCapability {
    pub ticket_id: String,
    pub expected_head_commit: String,
    pub child_session_id: String,
    pub capability_token: String,
    pub auth: MergeRequestAuth,
    pub now: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisteredReviewCapability {
    pub capability_token: String,
    pub request_event_seq: u64,
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

#[derive(Debug, Clone)]
pub struct SubmitMergeRequestReview {
    pub ticket_id: String,
    pub expected_head_commit: String,
    pub capability_token: String,
    pub decision: ReviewDecision,
    pub body: String,
    pub findings: Vec<ReviewFinding>,
    pub now: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct ReadinessCheck {
    pub ticket_id: String,
    pub expected_head_commit: Option<String>,
    pub auth: MergeRequestAuth,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadinessReport {
    pub ready: bool,
    pub blockers: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<RequestForReviewEvent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review: Option<ReviewEvent>,
}

#[derive(Debug, Clone)]
pub struct CompleteMergeRequest {
    pub ticket_id: String,
    pub expected_head_commit: String,
    pub operation_id: String,
    pub target_commit: String,
    pub source_commit: String,
    pub result_commit: String,
    pub strategy: MergeStrategy,
    pub resolution: ConflictResolution,
    pub auth: MergeRequestAuth,
    pub now: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct ChangeMergeRequestState {
    pub ticket_id: String,
    pub body: String,
    pub auth: MergeRequestAuth,
    pub now: DateTime<Utc>,
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

    fn is_ancestor(
        &self,
        workspace_id: &str,
        repository_id: &str,
        ancestor: &str,
        descendant: &str,
    ) -> Result<bool, String>;
}

#[derive(Debug, Error)]
pub enum MergeRequestError {
    #[error("merge request not found")]
    NotFound,
    #[error("merge request conflict: {0}")]
    Conflict(String),
    #[error("merge request unauthorized: {0}")]
    Unauthorized(String),
    #[error("merge request is not ready: {0}")]
    NotReady(String),
    #[error("merge request validation failed: {0}")]
    Validation(String),
    #[error("merge request operation failed: {0}")]
    Operation(String),
    #[error("merge request storage is corrupt: {0}")]
    Corrupt(String),
    #[error("merge request storage error: {0}")]
    Storage(#[from] rusqlite::Error),
}

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
        Self::from_connection(conn, assignments, repositories)
    }

    pub fn open_in_memory(
        assignments: Arc<dyn AssignmentSource>,
        repositories: Arc<dyn RepositorySource>,
    ) -> Result<Self, MergeRequestError> {
        Self::from_connection(Connection::open_in_memory()?, assignments, repositories)
    }

    fn from_connection(
        mut conn: Connection,
        assignments: Arc<dyn AssignmentSource>,
        repositories: Arc<dyn RepositorySource>,
    ) -> Result<Self, MergeRequestError> {
        conn.pragma_update(None, "foreign_keys", "ON")?;
        migrate(&mut conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            assignments,
            repositories,
        })
    }

    pub fn open_merge_request(
        &self,
        input: OpenMergeRequest,
    ) -> Result<MergeRequest, MergeRequestError> {
        validate_nonempty("merge_request_id", &input.merge_request_id)?;
        validate_nonempty("ticket_id", &input.ticket_id)?;
        validate_nonempty("selector_from", &input.selector_from)?;
        validate_nonempty("selector_to", &input.selector_to)?;
        self.validate_auth(&input.auth, &input.ticket_id, &input.repository_id)?;
        validate_request(&input.request)?;

        let mut conn = self.lock_conn()?;
        let tx = conn.transaction()?;
        if load_merge_request_tx(&tx, &input.auth.workspace_id, &input.ticket_id)?.is_some() {
            return Err(MergeRequestError::Conflict(
                "an open merge request already exists for this ticket".into(),
            ));
        }
        let now = input.now.to_rfc3339();
        tx.execute(
            "INSERT INTO merge_requests (
                workspace_id, merge_request_id, ticket_id, repository_id, state,
                selector_from, selector_to, opened_by_runtime_id, opened_by_worker_id,
                created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, 'open', ?5, ?6, ?7, ?8, ?9, ?9)",
            params![
                input.auth.workspace_id,
                input.merge_request_id,
                input.ticket_id,
                input.repository_id,
                input.selector_from,
                input.selector_to,
                input.auth.runtime_id,
                input.auth.worker_id,
                now,
            ],
        )?;
        append_request_event_tx(
            &tx,
            &input.auth.workspace_id,
            &input.merge_request_id,
            &input.auth,
            &input.request,
            input.now,
        )?;
        tx.commit()?;
        drop(conn);
        self.get(&input.auth.workspace_id, &input.ticket_id)
    }

    pub fn request_review(
        &self,
        input: RequestMergeRequestReview,
    ) -> Result<RequestForReviewEvent, MergeRequestError> {
        validate_request(&input.request)?;
        let current = self.get(&input.auth.workspace_id, &input.ticket_id)?;
        self.validate_auth(&input.auth, &input.ticket_id, &current.repository_id)?;
        ensure_open(&current)?;
        let current_request = current
            .thread
            .iter()
            .rev()
            .find_map(|event| match event {
                MergeRequestThreadEvent::RequestForReview(value) => Some(value),
                _ => None,
            })
            .ok_or_else(|| {
                MergeRequestError::Corrupt("open merge request has no review request".into())
            })?;
        if current_request.head_commit != input.expected_head_commit {
            return Err(MergeRequestError::Conflict(
                "expected head commit is stale".into(),
            ));
        }

        let mut conn = self.lock_conn()?;
        let tx = conn.transaction()?;
        let event = append_request_event_tx(
            &tx,
            &current.workspace_id,
            &current.merge_request_id,
            &input.auth,
            &input.request,
            input.now,
        )?;
        tx.commit()?;
        Ok(event)
    }

    pub fn register_review_capability(
        &self,
        input: RegisterReviewCapability,
    ) -> Result<RegisteredReviewCapability, MergeRequestError> {
        let mr = self.get(&input.auth.workspace_id, &input.ticket_id)?;
        self.validate_auth(&input.auth, &input.ticket_id, &mr.repository_id)?;
        ensure_open(&mr)?;
        let request = mr.current_request().ok_or_else(|| {
            MergeRequestError::Corrupt("open merge request has no review request".into())
        })?;
        if request.head_commit != input.expected_head_commit {
            return Err(MergeRequestError::Conflict(
                "expected head commit is stale".into(),
            ));
        }
        validate_nonempty("capability_token", &input.capability_token)?;
        let mut conn = self.lock_conn()?;
        let tx = conn.transaction()?;
        let child: Option<(String, String, String)> = tx
            .query_row(
                "SELECT parent_runtime_id, child_session_id, reviewer_profile
               FROM merge_request_reviewer_child_sessions
              WHERE workspace_id = ?1 AND child_session_id = ?2
                AND parent_runtime_id = ?3 AND parent_worker_id = ?4
                AND status = 'active'",
                params![
                    input.auth.workspace_id,
                    input.child_session_id,
                    input.auth.runtime_id,
                    input.auth.worker_id
                ],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        let Some((reviewer_runtime_id, reviewer_worker_id, reviewer_profile)) = child else {
            return Err(MergeRequestError::Unauthorized(
                "reviewer child session is missing or no longer active".into(),
            ));
        };
        validate_reviewer_profile(&reviewer_profile)?;
        tx.execute(
            "INSERT INTO merge_request_review_capabilities (
                workspace_id, merge_request_id, request_event_seq, capability_token,
                issued_by_assignment_id, reviewer_runtime_id, reviewer_worker_id,
                reviewer_profile, issued_at, status
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'issued')",
            params![
                mr.workspace_id,
                mr.merge_request_id,
                request.event_seq,
                input.capability_token,
                input.auth.assignment_id,
                reviewer_runtime_id,
                reviewer_worker_id,
                reviewer_profile,
                input.now.to_rfc3339()
            ],
        )?;
        tx.execute(
            "UPDATE merge_request_reviewer_child_sessions SET status = 'consumed'
              WHERE workspace_id = ?1 AND child_session_id = ?2 AND status = 'active'",
            params![input.auth.workspace_id, input.child_session_id],
        )?;
        tx.commit()?;
        Ok(RegisteredReviewCapability {
            capability_token: input.capability_token,
            request_event_seq: request.event_seq,
        })
    }

    pub fn register_reviewer_child_session(
        &self,
        input: RegisterReviewerChildSession,
    ) -> Result<(), MergeRequestError> {
        validate_reviewer_profile(&input.reviewer_profile)?;
        let conn = self.lock_conn()?;
        conn.execute(
            "INSERT INTO merge_request_reviewer_child_sessions (
                workspace_id, child_session_id, parent_runtime_id, parent_worker_id,
                reviewer_profile, registered_at, status
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'active')",
            params![
                input.workspace_id,
                input.child_session_id,
                input.parent_runtime_id,
                input.parent_worker_id,
                input.reviewer_profile,
                input.now.to_rfc3339()
            ],
        )?;
        Ok(())
    }

    pub fn submit_review(
        &self,
        input: SubmitMergeRequestReview,
    ) -> Result<ReviewEvent, MergeRequestError> {
        if input.body.trim().is_empty() {
            return Err(MergeRequestError::Validation(
                "review body must not be empty".into(),
            ));
        }
        let mut conn = self.lock_conn()?;
        let tx = conn.transaction()?;
        let capability: Option<(String, String, i64, String)> = tx
            .query_row(
                "SELECT c.workspace_id, c.merge_request_id, c.request_event_seq, c.status
                   FROM merge_request_review_capabilities c
                   JOIN merge_requests m
                     ON m.workspace_id = c.workspace_id
                    AND m.merge_request_id = c.merge_request_id
                  WHERE c.capability_token = ?1 AND m.ticket_id = ?2",
                params![input.capability_token, input.ticket_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()?;
        let Some((workspace_id, merge_request_id, request_event_seq, status)) = capability else {
            return Err(MergeRequestError::Unauthorized(
                "review capability is invalid".into(),
            ));
        };
        if status != "issued" {
            return Err(MergeRequestError::Conflict(
                "review capability has already been consumed".into(),
            ));
        }
        let reviewer: Option<(String, String, String)> = tx
            .query_row(
                "SELECT reviewer_runtime_id, reviewer_worker_id, reviewer_profile
                   FROM merge_request_review_capabilities
                  WHERE capability_token = ?1 AND status = 'issued'",
                params![input.capability_token],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        let Some((reviewer_runtime_id, reviewer_worker_id, reviewer_profile)) = reviewer else {
            return Err(MergeRequestError::Unauthorized(
                "review source is not the registered reviewer child".into(),
            ));
        };
        validate_reviewer_profile(&reviewer_profile)?;
        let request_payload: String = tx.query_row(
            "SELECT payload_json FROM merge_request_thread_events
              WHERE workspace_id = ?1 AND merge_request_id = ?2
                AND event_seq = ?3 AND kind = 'request_for_review'",
            params![workspace_id, merge_request_id, request_event_seq],
            |row| row.get(0),
        )?;
        let request: RequestForReviewEvent = decode_payload(&request_payload)?;
        if request.head_commit != input.expected_head_commit {
            return Err(MergeRequestError::Conflict(
                "expected head commit is stale".into(),
            ));
        }
        let latest_request_seq: i64 = tx.query_row(
            "SELECT MAX(event_seq) FROM merge_request_thread_events
              WHERE workspace_id = ?1 AND merge_request_id = ?2 AND kind = 'request_for_review'",
            params![workspace_id, merge_request_id],
            |row| row.get(0),
        )?;
        if latest_request_seq != request_event_seq {
            return Err(MergeRequestError::Conflict(
                "review request has been superseded".into(),
            ));
        }
        let event_seq = next_event_seq_tx(&tx, &workspace_id, &merge_request_id)?;
        let event = ReviewEvent {
            event_seq,
            request_event_seq: request_event_seq as u64,
            head_commit: request.head_commit,
            reviewer_worker: WorkerIdentity {
                runtime_id: reviewer_runtime_id,
                worker_id: reviewer_worker_id,
            },
            reviewer_profile,
            decision: input.decision,
            body: input.body,
            findings: input.findings,
            created_at: input.now,
        };
        insert_event_tx(
            &tx,
            &workspace_id,
            &merge_request_id,
            "review",
            &event,
            input.now,
            None,
        )?;
        tx.execute(
            "UPDATE merge_request_review_capabilities SET status = 'consumed', consumed_at = ?2
              WHERE capability_token = ?1 AND status = 'issued'",
            params![input.capability_token, input.now.to_rfc3339()],
        )?;
        tx.commit()?;
        Ok(event)
    }

    pub fn readiness(&self, input: ReadinessCheck) -> Result<ReadinessReport, MergeRequestError> {
        let mr = self.get(&input.auth.workspace_id, &input.ticket_id)?;
        if mr.repository_id != input.auth.repository_id {
            return Err(MergeRequestError::Unauthorized(
                "repository does not match merge request".into(),
            ));
        }
        let request = mr.current_request().cloned();
        let review = mr.current_review().cloned();
        let mut blockers = Vec::new();
        if mr.state != MergeRequestState::Open {
            blockers.push("merge request is not open".into());
        }
        match &request {
            None => blockers.push("merge request has no review request".into()),
            Some(value) => {
                if input
                    .expected_head_commit
                    .as_deref()
                    .is_some_and(|expected| expected != value.head_commit)
                {
                    blockers.push("expected head commit is stale".into());
                }
                match &review {
                    None => blockers.push("current review request has no review result".into()),
                    Some(review) if review.decision != ReviewDecision::Approve => {
                        blockers.push("current review requests changes".into())
                    }
                    Some(review) if review.head_commit != value.head_commit => blockers
                        .push("current review does not match the requested head commit".into()),
                    Some(_) => {}
                }
            }
        }
        Ok(ReadinessReport {
            ready: blockers.is_empty(),
            blockers,
            request,
            review,
        })
    }

    pub fn complete(&self, input: CompleteMergeRequest) -> Result<MergeEvent, MergeRequestError> {
        validate_nonempty("operation_id", &input.operation_id)?;
        let mr = self.get(&input.auth.workspace_id, &input.ticket_id)?;
        self.validate_completion_auth(&input.auth, &input.ticket_id, &mr.repository_id)?;

        if let Some(existing) = mr.thread.iter().find_map(|event| match event {
            MergeRequestThreadEvent::Merge(value) if value.operation_id == input.operation_id => {
                Some(value)
            }
            _ => None,
        }) {
            if merge_matches(existing, &input) {
                return Ok(existing.clone());
            }
            return Err(MergeRequestError::Conflict(
                "operation id was already used with a different completion fingerprint".into(),
            ));
        }

        let readiness = self.readiness(ReadinessCheck {
            ticket_id: input.ticket_id.clone(),
            expected_head_commit: Some(input.expected_head_commit.clone()),
            auth: input.auth.clone(),
        })?;
        if !readiness.ready {
            return Err(MergeRequestError::NotReady(readiness.blockers.join("; ")));
        }
        let request = readiness.request.expect("ready request");
        if input.source_commit != request.head_commit
            || input.expected_head_commit != request.head_commit
        {
            return Err(MergeRequestError::Conflict(
                "completion source does not match current review request".into(),
            ));
        }
        validate_completion_shape(&input, &request, self.repositories.as_ref(), &mr)?;

        let mut conn = self.lock_conn()?;
        let tx = conn.transaction()?;
        let current_ticket_state: Option<String> = tx
            .query_row(
                "SELECT workflow_state FROM typed_tickets WHERE workspace_id = ?1 AND ticket_id = ?2",
                params![mr.workspace_id, input.ticket_id],
                |row| row.get(0),
            )
            .optional()?;
        match current_ticket_state.as_deref() {
            Some("inprogress") => {}
            Some(other) => {
                return Err(MergeRequestError::Conflict(format!(
                    "Ticket state `{other}` cannot be completed"
                )));
            }
            None => return Err(MergeRequestError::NotFound),
        }
        let changed = tx.execute(
            "UPDATE typed_tickets
                SET workflow_state = 'done', workflow_state_explicit = 1, updated_at = ?3
              WHERE workspace_id = ?1 AND ticket_id = ?2 AND workflow_state = 'inprogress'",
            params![mr.workspace_id, input.ticket_id, input.now.to_rfc3339()],
        )?;
        if changed != 1 {
            return Err(MergeRequestError::Conflict(
                "Ticket state changed concurrently".into(),
            ));
        }
        let implementation_assignment_id = input.auth.assignment_id.clone();
        let event_seq = next_event_seq_tx(&tx, &mr.workspace_id, &mr.merge_request_id)?;
        let event = MergeEvent {
            event_seq,
            operation_id: input.operation_id,
            target_commit: input.target_commit,
            source_commit: input.source_commit,
            result_commit: input.result_commit,
            strategy: input.strategy,
            resolution: input.resolution,
            merged_by: WorkerIdentity {
                runtime_id: input.auth.runtime_id,
                worker_id: input.auth.worker_id,
            },
            created_at: input.now,
        };
        insert_event_tx(
            &tx,
            &mr.workspace_id,
            &mr.merge_request_id,
            "merge",
            &event,
            input.now,
            Some(&event.operation_id),
        )?;
        tx.execute(
            "UPDATE merge_requests SET state = 'merged', updated_at = ?3
              WHERE workspace_id = ?1 AND merge_request_id = ?2 AND state = 'open'",
            params![mr.workspace_id, mr.merge_request_id, input.now.to_rfc3339()],
        )?;
        append_ticket_completion_event_tx(&tx, &mr, &event, &implementation_assignment_id)?;
        tx.commit()?;
        Ok(event)
    }

    pub fn close(&self, input: ChangeMergeRequestState) -> Result<MergeRequest, MergeRequestError> {
        self.change_state(input, MergeRequestState::Closed, "close")
    }

    pub fn reopen(
        &self,
        input: ChangeMergeRequestState,
    ) -> Result<MergeRequest, MergeRequestError> {
        self.change_state(input, MergeRequestState::Open, "reopen")
    }

    fn change_state(
        &self,
        input: ChangeMergeRequestState,
        target: MergeRequestState,
        kind: &str,
    ) -> Result<MergeRequest, MergeRequestError> {
        let mr = self.get(&input.auth.workspace_id, &input.ticket_id)?;
        self.validate_lifecycle_auth(&input.auth, &mr.repository_id)?;
        match (mr.state, target) {
            (MergeRequestState::Open, MergeRequestState::Closed)
            | (MergeRequestState::Closed, MergeRequestState::Open)
            | (MergeRequestState::Merged, MergeRequestState::Open) => {}
            _ => {
                return Err(MergeRequestError::Conflict(
                    "illegal merge request state transition".into(),
                ));
            }
        }
        let mut conn = self.lock_conn()?;
        let tx = conn.transaction()?;
        let event_seq = next_event_seq_tx(&tx, &mr.workspace_id, &mr.merge_request_id)?;
        let event = LifecycleEvent {
            event_seq,
            actor: WorkerIdentity {
                runtime_id: input.auth.runtime_id,
                worker_id: input.auth.worker_id,
            },
            body: input.body,
            created_at: input.now,
        };
        insert_event_tx(
            &tx,
            &mr.workspace_id,
            &mr.merge_request_id,
            kind,
            &event,
            input.now,
            None,
        )?;
        tx.execute(
            "UPDATE merge_requests SET state = ?3, updated_at = ?4
              WHERE workspace_id = ?1 AND merge_request_id = ?2",
            params![
                mr.workspace_id,
                mr.merge_request_id,
                target.as_str(),
                input.now.to_rfc3339()
            ],
        )?;
        tx.commit()?;
        drop(conn);
        self.get(&input.auth.workspace_id, &input.ticket_id)
    }

    pub fn get(
        &self,
        workspace_id: &str,
        ticket_id: &str,
    ) -> Result<MergeRequest, MergeRequestError> {
        let conn = self.lock_conn()?;
        load_merge_request(&conn, workspace_id, ticket_id)?.ok_or(MergeRequestError::NotFound)
    }

    pub fn list_for_ticket(
        &self,
        workspace_id: &str,
        ticket_id: &str,
    ) -> Result<Vec<MergeRequest>, MergeRequestError> {
        let conn = self.lock_conn()?;
        let mut stmt = conn.prepare(
            "SELECT merge_request_id FROM merge_requests
              WHERE workspace_id = ?1 AND ticket_id = ?2
              ORDER BY created_at DESC, merge_request_id DESC",
        )?;
        let ids = stmt
            .query_map(params![workspace_id, ticket_id], |row| {
                row.get::<_, String>(0)
            })?
            .collect::<Result<Vec<_>, _>>()?;
        ids.into_iter()
            .map(|id| {
                load_merge_request_by_id(&conn, workspace_id, &id)?
                    .ok_or(MergeRequestError::NotFound)
            })
            .collect()
    }

    fn validate_auth(
        &self,
        auth: &MergeRequestAuth,
        ticket_id: &str,
        repository_id: &str,
    ) -> Result<(), MergeRequestError> {
        if auth.repository_id != repository_id {
            return Err(MergeRequestError::Unauthorized(
                "repository does not match request".into(),
            ));
        }
        if !self
            .repositories
            .repository_belongs_to_workspace(&auth.workspace_id, repository_id)
            .map_err(MergeRequestError::Operation)?
        {
            return Err(MergeRequestError::Unauthorized(
                "repository does not belong to workspace".into(),
            ));
        }
        let assignment = self
            .assignments
            .current_assignment(&auth.workspace_id, ticket_id)
            .map_err(MergeRequestError::Operation)?
            .ok_or_else(|| {
                MergeRequestError::Unauthorized("ticket has no current assignment".into())
            })?;
        if assignment.assignment_id != auth.assignment_id
            || assignment.runtime_id != auth.runtime_id
            || assignment.worker_id != auth.worker_id
            || assignment.ticket_id != ticket_id
        {
            return Err(MergeRequestError::Unauthorized(
                "caller is not the current assigned worker".into(),
            ));
        }
        Ok(())
    }

    fn validate_lifecycle_auth(
        &self,
        auth: &MergeRequestAuth,
        repository_id: &str,
    ) -> Result<(), MergeRequestError> {
        if auth.repository_id != repository_id {
            return Err(MergeRequestError::Unauthorized(
                "repository does not match request".into(),
            ));
        }
        if !self
            .repositories
            .repository_belongs_to_workspace(&auth.workspace_id, repository_id)
            .map_err(MergeRequestError::Operation)?
        {
            return Err(MergeRequestError::Unauthorized(
                "repository does not belong to workspace".into(),
            ));
        }
        Ok(())
    }

    fn validate_completion_auth(
        &self,
        auth: &MergeRequestAuth,
        ticket_id: &str,
        repository_id: &str,
    ) -> Result<(), MergeRequestError> {
        if auth.repository_id != repository_id {
            return Err(MergeRequestError::Unauthorized(
                "repository does not match request".into(),
            ));
        }
        if !self
            .repositories
            .repository_belongs_to_workspace(&auth.workspace_id, repository_id)
            .map_err(MergeRequestError::Operation)?
        {
            return Err(MergeRequestError::Unauthorized(
                "repository does not belong to workspace".into(),
            ));
        }
        let assignment = self
            .assignments
            .current_assignment(&auth.workspace_id, ticket_id)
            .map_err(MergeRequestError::Operation)?
            .ok_or_else(|| {
                MergeRequestError::Unauthorized("ticket has no current assignment".into())
            })?;
        if assignment.assignment_id != auth.assignment_id || assignment.ticket_id != ticket_id {
            return Err(MergeRequestError::Unauthorized(
                "completion does not match the current assignment".into(),
            ));
        }
        Ok(())
    }

    fn lock_conn(&self) -> Result<std::sync::MutexGuard<'_, Connection>, MergeRequestError> {
        self.conn
            .lock()
            .map_err(|_| MergeRequestError::Operation("database lock poisoned".into()))
    }
}

fn validate_request(request: &RequestForReview) -> Result<(), MergeRequestError> {
    validate_nonempty("base_commit", &request.base_commit)?;
    validate_nonempty("head_commit", &request.head_commit)?;
    if request
        .changed_paths
        .iter()
        .any(|path| path.trim().is_empty())
    {
        return Err(MergeRequestError::Validation(
            "changed_paths must not contain empty entries".into(),
        ));
    }
    Ok(())
}

fn validate_nonempty(field: &str, value: &str) -> Result<(), MergeRequestError> {
    if value.trim().is_empty() {
        Err(MergeRequestError::Validation(format!(
            "{field} must not be empty"
        )))
    } else {
        Ok(())
    }
}

fn validate_reviewer_profile(profile: &str) -> Result<(), MergeRequestError> {
    if profile == "builtin:reviewer" {
        Ok(())
    } else {
        Err(MergeRequestError::Unauthorized(
            "review source must use builtin:reviewer".into(),
        ))
    }
}

fn ensure_open(mr: &MergeRequest) -> Result<(), MergeRequestError> {
    if mr.state == MergeRequestState::Open {
        Ok(())
    } else {
        Err(MergeRequestError::Conflict(
            "merge request is not open".into(),
        ))
    }
}

fn merge_matches(existing: &MergeEvent, input: &CompleteMergeRequest) -> bool {
    existing.target_commit == input.target_commit
        && existing.source_commit == input.source_commit
        && existing.result_commit == input.result_commit
        && existing.strategy == input.strategy
        && existing.resolution == input.resolution
}

fn validate_completion_shape(
    input: &CompleteMergeRequest,
    request: &RequestForReviewEvent,
    repositories: &dyn RepositorySource,
    mr: &MergeRequest,
) -> Result<(), MergeRequestError> {
    match input.strategy {
        MergeStrategy::FastForward => {
            if input.resolution != ConflictResolution::None
                || input.target_commit != request.base_commit
                || input.result_commit != input.source_commit
            {
                return Err(MergeRequestError::Validation(
                    "fast-forward completion must use the review base as target, the source as result, and resolution none".into(),
                ));
            }
            if !repositories
                .is_ancestor(
                    &mr.workspace_id,
                    &mr.repository_id,
                    &input.target_commit,
                    &input.source_commit,
                )
                .map_err(MergeRequestError::Operation)?
            {
                return Err(MergeRequestError::Conflict(
                    "source commit is not a descendant of target commit".into(),
                ));
            }
        }
        MergeStrategy::Merge => {
            if input.resolution == ConflictResolution::None
                || input.result_commit == input.target_commit
            {
                return Err(MergeRequestError::Validation(
                    "merge completion requires an explicit resolution and a new result commit"
                        .into(),
                ));
            }
            if !repositories
                .is_ancestor(
                    &mr.workspace_id,
                    &mr.repository_id,
                    &input.target_commit,
                    &input.result_commit,
                )
                .map_err(MergeRequestError::Operation)?
                || !repositories
                    .is_ancestor(
                        &mr.workspace_id,
                        &mr.repository_id,
                        &input.source_commit,
                        &input.result_commit,
                    )
                    .map_err(MergeRequestError::Operation)?
            {
                return Err(MergeRequestError::Conflict(
                    "merge result must descend from both target and source commits".into(),
                ));
            }
        }
    }
    Ok(())
}

fn append_ticket_completion_event_tx(
    tx: &Transaction<'_>,
    mr: &MergeRequest,
    event: &MergeEvent,
    implementation_assignment_id: &str,
) -> Result<(), MergeRequestError> {
    let event_index: i64 = tx.query_row(
        "SELECT COALESCE(MAX(event_index), -1) + 1 FROM typed_ticket_events
          WHERE workspace_id = ?1 AND ticket_id = ?2",
        params![mr.workspace_id, mr.ticket_id],
        |row| row.get(0),
    )?;
    tx.execute(
        "INSERT INTO typed_ticket_events (
            workspace_id, ticket_id, event_index, kind, author, at,
            from_state, to_state, heading, body
         ) VALUES (?1, ?2, ?3, 'state_changed', ?4, ?5,
                   'inprogress', 'done', 'Merge Request completed', ?6)",
        params![
            mr.workspace_id,
            mr.ticket_id,
            event_index,
            format!(
                "worker:{}:{}",
                event.merged_by.runtime_id, event.merged_by.worker_id
            ),
            event.created_at.to_rfc3339(),
            format!(
                "Approved candidate `{}` completed implementation.",
                event.source_commit
            ),
        ],
    )?;
    let event_seq = event.event_seq.to_string();
    for (key, value) in [
        ("implementation_assignment_id", implementation_assignment_id),
        ("merge_request_event_seq", event_seq.as_str()),
        ("merge_request_head_commit", event.source_commit.as_str()),
        ("operation_id", event.operation_id.as_str()),
        ("completion_authority", "workspace_orchestrator"),
        ("runtime_id", event.merged_by.runtime_id.as_str()),
        ("worker_id", event.merged_by.worker_id.as_str()),
    ] {
        tx.execute(
            "INSERT INTO typed_ticket_event_attributes (
                workspace_id, ticket_id, event_index, key, value
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![mr.workspace_id, mr.ticket_id, event_index, key, value],
        )?;
    }
    Ok(())
}

fn append_request_event_tx(
    tx: &Transaction<'_>,
    workspace_id: &str,
    merge_request_id: &str,
    auth: &MergeRequestAuth,
    request: &RequestForReview,
    now: DateTime<Utc>,
) -> Result<RequestForReviewEvent, MergeRequestError> {
    let event_seq = next_event_seq_tx(tx, workspace_id, merge_request_id)?;
    let event = RequestForReviewEvent {
        event_seq,
        base_commit: request.base_commit.clone(),
        head_commit: request.head_commit.clone(),
        changed_paths: request.changed_paths.clone(),
        summary: request.summary.clone(),
        assignment_id: auth.assignment_id.clone(),
        requested_by: WorkerIdentity {
            runtime_id: auth.runtime_id.clone(),
            worker_id: auth.worker_id.clone(),
        },
        created_at: now,
    };
    insert_event_tx(
        tx,
        workspace_id,
        merge_request_id,
        "request_for_review",
        &event,
        now,
        None,
    )?;
    tx.execute(
        "UPDATE merge_requests SET updated_at = ?3
          WHERE workspace_id = ?1 AND merge_request_id = ?2",
        params![workspace_id, merge_request_id, now.to_rfc3339()],
    )?;
    tx.execute(
        "UPDATE merge_request_review_capabilities SET status = 'revoked'
          WHERE workspace_id = ?1 AND merge_request_id = ?2 AND status = 'issued'",
        params![workspace_id, merge_request_id],
    )?;
    Ok(event)
}

fn next_event_seq_tx(
    tx: &Transaction<'_>,
    workspace_id: &str,
    merge_request_id: &str,
) -> Result<u64, MergeRequestError> {
    let current: i64 = tx.query_row(
        "SELECT COALESCE(MAX(event_seq), 0) FROM merge_request_thread_events
          WHERE workspace_id = ?1 AND merge_request_id = ?2",
        params![workspace_id, merge_request_id],
        |row| row.get(0),
    )?;
    Ok((current + 1) as u64)
}

fn insert_event_tx<T: Serialize>(
    tx: &Transaction<'_>,
    workspace_id: &str,
    merge_request_id: &str,
    kind: &str,
    payload: &T,
    now: DateTime<Utc>,
    operation_id: Option<&str>,
) -> Result<(), MergeRequestError> {
    let event_seq = serde_json::to_value(payload)
        .map_err(|error| MergeRequestError::Operation(error.to_string()))?
        .get("event_seq")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| MergeRequestError::Operation("event payload has no event_seq".into()))?;
    let payload_json = serde_json::to_string(payload)
        .map_err(|error| MergeRequestError::Operation(error.to_string()))?;
    tx.execute(
        "INSERT INTO merge_request_thread_events (
            workspace_id, merge_request_id, event_seq, kind, payload_json,
            operation_id, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            workspace_id,
            merge_request_id,
            event_seq as i64,
            kind,
            payload_json,
            operation_id,
            now.to_rfc3339()
        ],
    )?;
    Ok(())
}

fn load_merge_request(
    conn: &Connection,
    workspace_id: &str,
    ticket_id: &str,
) -> Result<Option<MergeRequest>, MergeRequestError> {
    let id: Option<String> = conn
        .query_row(
            "SELECT merge_request_id FROM merge_requests
              WHERE workspace_id = ?1 AND ticket_id = ?2
              ORDER BY CASE state WHEN 'open' THEN 0 ELSE 1 END, created_at DESC
              LIMIT 1",
            params![workspace_id, ticket_id],
            |row| row.get(0),
        )
        .optional()?;
    id.map(|id| load_merge_request_by_id(conn, workspace_id, &id))
        .transpose()
        .map(Option::flatten)
}

fn load_merge_request_tx(
    tx: &Transaction<'_>,
    workspace_id: &str,
    ticket_id: &str,
) -> Result<Option<MergeRequest>, MergeRequestError> {
    let id: Option<String> = tx
        .query_row(
            "SELECT merge_request_id FROM merge_requests
              WHERE workspace_id = ?1 AND ticket_id = ?2 AND state = 'open' LIMIT 1",
            params![workspace_id, ticket_id],
            |row| row.get(0),
        )
        .optional()?;
    id.map(|id| load_merge_request_by_id(tx, workspace_id, &id))
        .transpose()
        .map(Option::flatten)
}

fn load_merge_request_by_id(
    conn: &Connection,
    workspace_id: &str,
    merge_request_id: &str,
) -> Result<Option<MergeRequest>, MergeRequestError> {
    let row: Option<(
        String,
        String,
        String,
        String,
        String,
        String,
        String,
        String,
        String,
        String,
    )> = conn
        .query_row(
            "SELECT ticket_id, repository_id, state, selector_from, selector_to,
                    opened_by_runtime_id, opened_by_worker_id, created_at, updated_at,
                    merge_request_id
               FROM merge_requests
              WHERE workspace_id = ?1 AND merge_request_id = ?2",
            params![workspace_id, merge_request_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                    row.get(9)?,
                ))
            },
        )
        .optional()?;
    let Some((
        ticket_id,
        repository_id,
        state,
        selector_from,
        selector_to,
        opened_runtime,
        opened_worker,
        created_at,
        updated_at,
        id,
    )) = row
    else {
        return Ok(None);
    };
    let mut stmt = conn.prepare(
        "SELECT kind, payload_json FROM merge_request_thread_events
          WHERE workspace_id = ?1 AND merge_request_id = ?2 ORDER BY event_seq",
    )?;
    let rows = stmt
        .query_map(params![workspace_id, merge_request_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut thread = Vec::with_capacity(rows.len());
    for (kind, payload) in rows {
        thread.push(match kind.as_str() {
            "request_for_review" => {
                MergeRequestThreadEvent::RequestForReview(decode_payload(&payload)?)
            }
            "review" => MergeRequestThreadEvent::Review(decode_payload(&payload)?),
            "merge" => MergeRequestThreadEvent::Merge(decode_payload(&payload)?),
            "reopen" => MergeRequestThreadEvent::Reopen(decode_payload(&payload)?),
            "close" => MergeRequestThreadEvent::Close(decode_payload(&payload)?),
            other => {
                return Err(MergeRequestError::Corrupt(format!(
                    "unknown merge request thread event kind `{other}`"
                )));
            }
        });
    }
    Ok(Some(MergeRequest {
        workspace_id: workspace_id.to_string(),
        merge_request_id: id,
        ticket_id,
        repository_id,
        state: MergeRequestState::parse(&state)?,
        selector_from,
        selector_to,
        opened_by_worker: WorkerIdentity {
            runtime_id: opened_runtime,
            worker_id: opened_worker,
        },
        created_at: parse_timestamp(&created_at)?,
        updated_at: parse_timestamp(&updated_at)?,
        thread,
    }))
}

fn decode_payload<T: for<'de> Deserialize<'de>>(value: &str) -> Result<T, MergeRequestError> {
    serde_json::from_str(value).map_err(|error| MergeRequestError::Corrupt(error.to_string()))
}

fn parse_timestamp(value: &str) -> Result<DateTime<Utc>, MergeRequestError> {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|error| MergeRequestError::Corrupt(error.to_string()))
}

pub fn migrate(conn: &Connection) -> Result<(), MergeRequestError> {
    let version = schema_version(conn)?;
    match version {
        None => create_latest_schema(conn),
        Some(SCHEMA_VERSION) => verify_latest_schema(conn),
        Some(PREVIOUS_SCHEMA_VERSION) => migrate_v11_to_v12(conn),
        Some(other) => Err(MergeRequestError::Operation(format!(
            "unsupported merge request schema version {other}; expected {PREVIOUS_SCHEMA_VERSION} or {SCHEMA_VERSION}"
        ))),
    }
}

fn schema_version(conn: &Connection) -> Result<Option<i64>, MergeRequestError> {
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'merge_request_schema')",
        [],
        |row| row.get(0),
    )?;
    if !exists {
        return Ok(None);
    }
    conn.query_row(
        "SELECT version FROM merge_request_schema WHERE singleton = 1",
        [],
        |row| row.get(0),
    )
    .optional()
    .map_err(Into::into)
}

fn create_latest_schema(conn: &Connection) -> Result<(), MergeRequestError> {
    let tx = conn.unchecked_transaction()?;
    create_v12_tables(&tx)?;
    tx.execute(
        "INSERT INTO merge_request_schema (singleton, version) VALUES (1, ?1)",
        params![SCHEMA_VERSION],
    )?;
    foreign_key_check(&tx)?;
    tx.commit()?;
    Ok(())
}

fn migrate_v11_to_v12(conn: &Connection) -> Result<(), MergeRequestError> {
    let tx = conn.unchecked_transaction()?;
    let incompatible: Option<String> = tx
        .query_row(
            "SELECT mr.merge_request_id
               FROM merge_requests mr
               LEFT JOIN merge_request_revisions r
                 ON r.workspace_id = mr.workspace_id
                AND r.revision_id = mr.current_revision_id
              WHERE r.revision_id IS NULL
              LIMIT 1",
            [],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(id) = incompatible {
        return Err(MergeRequestError::Operation(format!(
            "cannot migrate merge request `{id}`: current revision is missing"
        )));
    }

    tx.execute_batch(
        "ALTER TABLE merge_requests RENAME TO merge_requests_v11;
         ALTER TABLE merge_request_revisions RENAME TO merge_request_revisions_v11;
         ALTER TABLE merge_request_review_attempts RENAME TO merge_request_review_attempts_v11;
         ALTER TABLE merge_request_reviews RENAME TO merge_request_reviews_v11;
         ALTER TABLE merge_request_completion_operations RENAME TO merge_request_completion_operations_v11;
         ALTER TABLE merge_request_reviewer_child_sessions RENAME TO merge_request_reviewer_child_sessions_v11;",
    )?;
    create_v12_domain_tables(&tx)?;
    tx.execute(
        "INSERT INTO merge_requests (
            workspace_id, merge_request_id, ticket_id, repository_id, state,
            selector_from, selector_to, opened_by_runtime_id, opened_by_worker_id,
            created_at, updated_at
         )
         SELECT mr.workspace_id, mr.merge_request_id, mr.ticket_id, mr.repository_id,
                CASE mr.state WHEN 'draft' THEN 'open' ELSE mr.state END,
                r.head_commit, mr.target_ref_selector,
                mr.opened_by_worker_runtime_id, mr.opened_by_worker_id,
                mr.created_at, mr.updated_at
           FROM merge_requests_v11 mr
           JOIN merge_request_revisions_v11 r
             ON r.workspace_id = mr.workspace_id AND r.revision_id = mr.current_revision_id",
        [],
    )?;

    migrate_v11_events(&tx)?;
    tx.execute_batch(
        "DROP TABLE merge_request_reviewer_child_sessions_v11;
         DROP TABLE merge_request_completion_operations_v11;
         DROP TABLE merge_request_reviews_v11;
         DROP TABLE merge_request_review_attempts_v11;
         DROP TABLE merge_request_revisions_v11;
         DROP TABLE merge_requests_v11;
         UPDATE merge_request_schema SET version = 12 WHERE singleton = 1;",
    )?;
    foreign_key_check(&tx)?;
    tx.commit()?;
    Ok(())
}

fn migrate_v11_events(tx: &Transaction<'_>) -> Result<(), MergeRequestError> {
    let mut stmt = tx.prepare(
        "SELECT r.workspace_id, r.merge_request_id, r.revision_id, r.base_commit, r.head_commit,
                r.changed_paths_json, r.summary, r.assignment_id,
                r.coder_worker_runtime_id, r.coder_worker_id, r.created_at
           FROM merge_request_revisions_v11 r
           ORDER BY r.workspace_id, r.merge_request_id, r.created_at, r.revision_id",
    )?;
    let revisions = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, String>(8)?,
                row.get::<_, String>(9)?,
                row.get::<_, String>(10)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(stmt);
    for (
        workspace_id,
        mr_id,
        revision_id,
        base,
        head,
        paths_json,
        summary,
        assignment,
        runtime,
        worker,
        created_at,
    ) in revisions
    {
        let event_seq = next_event_seq_tx(tx, &workspace_id, &mr_id)?;
        let event = RequestForReviewEvent {
            event_seq,
            base_commit: base,
            head_commit: head,
            changed_paths: serde_json::from_str(&paths_json).map_err(|error| {
                MergeRequestError::Operation(format!(
                    "cannot migrate revision `{revision_id}` changed paths: {error}"
                ))
            })?,
            summary,
            assignment_id: assignment,
            requested_by: WorkerIdentity {
                runtime_id: runtime,
                worker_id: worker,
            },
            created_at: parse_timestamp(&created_at)?,
        };
        insert_event_tx(
            tx,
            &workspace_id,
            &mr_id,
            "request_for_review",
            &event,
            event.created_at,
            None,
        )?;

        let reviews = {
            let mut reviews_stmt = tx.prepare(
                "SELECT reviewer_worker_runtime_id, reviewer_worker_id, reviewer_profile,
                        decision, body, findings_json, created_at
                   FROM merge_request_reviews_v11
                  WHERE workspace_id = ?1 AND revision_id = ?2
                  ORDER BY created_at, review_id",
            )?;
            reviews_stmt
                .query_map(params![workspace_id, revision_id], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, String>(6)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?
        };
        for (
            review_runtime,
            review_worker,
            profile,
            decision,
            body,
            findings_json,
            review_created,
        ) in reviews
        {
            let review_seq = next_event_seq_tx(tx, &workspace_id, &mr_id)?;
            let review = ReviewEvent {
                event_seq: review_seq,
                request_event_seq: event_seq,
                head_commit: event.head_commit.clone(),
                reviewer_worker: WorkerIdentity {
                    runtime_id: review_runtime,
                    worker_id: review_worker,
                },
                reviewer_profile: profile,
                decision: match decision.as_str() {
                    "approve" => ReviewDecision::Approve,
                    "request_changes" => ReviewDecision::RequestChanges,
                    other => {
                        return Err(MergeRequestError::Operation(format!(
                            "cannot migrate unknown review decision `{other}`"
                        )));
                    }
                },
                body,
                findings: serde_json::from_str(&findings_json).map_err(|error| {
                    MergeRequestError::Operation(format!("cannot migrate review findings: {error}"))
                })?,
                created_at: parse_timestamp(&review_created)?,
            };
            insert_event_tx(
                tx,
                &workspace_id,
                &mr_id,
                "review",
                &review,
                review.created_at,
                None,
            )?;
        }
    }

    let mut completion_stmt = tx.prepare(
        "SELECT workspace_id, merge_request_id, operation_id, target_commit, source_commit,
                result_commit, strategy, resolution, requested_by_runtime_id,
                requested_by_worker_id, completed_at
           FROM merge_request_completion_operations_v11
          WHERE status = 'succeeded'
          ORDER BY completed_at, operation_id",
    )?;
    let completions = completion_stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, String>(8)?,
                row.get::<_, String>(9)?,
                row.get::<_, Option<String>>(10)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(completion_stmt);
    for (
        workspace_id,
        mr_id,
        operation_id,
        target,
        source,
        result,
        strategy,
        resolution,
        runtime,
        worker,
        completed_at,
    ) in completions
    {
        let created_at = parse_timestamp(completed_at.as_deref().ok_or_else(|| {
            MergeRequestError::Operation(format!(
                "cannot migrate completion `{operation_id}` without completed_at"
            ))
        })?)?;
        let event = MergeEvent {
            event_seq: next_event_seq_tx(tx, &workspace_id, &mr_id)?,
            operation_id,
            target_commit: target,
            source_commit: source,
            result_commit: result,
            strategy: match strategy.as_str() {
                "fast_forward" => MergeStrategy::FastForward,
                "merge" => MergeStrategy::Merge,
                other => {
                    return Err(MergeRequestError::Operation(format!(
                        "cannot migrate unknown merge strategy `{other}`"
                    )));
                }
            },
            resolution: match resolution.as_str() {
                "none" => ConflictResolution::None,
                "clean" => ConflictResolution::Clean,
                "conflicts_resolved" => ConflictResolution::ConflictsResolved,
                other => {
                    return Err(MergeRequestError::Operation(format!(
                        "cannot migrate unknown conflict resolution `{other}`"
                    )));
                }
            },
            merged_by: WorkerIdentity {
                runtime_id: runtime,
                worker_id: worker,
            },
            created_at,
        };
        insert_event_tx(
            tx,
            &workspace_id,
            &mr_id,
            "merge",
            &event,
            created_at,
            Some(&event.operation_id),
        )?;
    }
    Ok(())
}

fn create_v12_tables(tx: &Transaction<'_>) -> Result<(), MergeRequestError> {
    tx.execute_batch(
        "CREATE TABLE merge_request_schema (
            singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
            version INTEGER NOT NULL
         );",
    )?;
    create_v12_domain_tables(tx)
}

fn create_v12_domain_tables(tx: &Transaction<'_>) -> Result<(), MergeRequestError> {
    tx.execute_batch(
        "CREATE TABLE merge_requests (
            workspace_id TEXT NOT NULL,
            merge_request_id TEXT NOT NULL,
            ticket_id TEXT NOT NULL,
            repository_id TEXT NOT NULL,
            state TEXT NOT NULL CHECK (state IN ('open', 'merged', 'closed')),
            selector_from TEXT NOT NULL CHECK (length(trim(selector_from)) > 0),
            selector_to TEXT NOT NULL CHECK (length(trim(selector_to)) > 0),
            opened_by_runtime_id TEXT NOT NULL,
            opened_by_worker_id TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            PRIMARY KEY (workspace_id, merge_request_id),
            FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
            FOREIGN KEY (workspace_id, repository_id)
                REFERENCES repositories(workspace_id, repository_id) ON DELETE RESTRICT,
            FOREIGN KEY (workspace_id, ticket_id)
                REFERENCES typed_tickets(workspace_id, ticket_id) ON DELETE CASCADE
         );
         CREATE UNIQUE INDEX merge_requests_one_open_per_ticket
             ON merge_requests(workspace_id, ticket_id) WHERE state = 'open';
         CREATE INDEX merge_requests_ticket_history
             ON merge_requests(workspace_id, ticket_id, created_at DESC);

         CREATE TABLE merge_request_thread_events (
            workspace_id TEXT NOT NULL,
            merge_request_id TEXT NOT NULL,
            event_seq INTEGER NOT NULL CHECK (event_seq > 0),
            kind TEXT NOT NULL CHECK (kind IN ('request_for_review', 'review', 'merge', 'reopen', 'close')),
            payload_json TEXT NOT NULL,
            operation_id TEXT,
            created_at TEXT NOT NULL,
            PRIMARY KEY (workspace_id, merge_request_id, event_seq),
            FOREIGN KEY (workspace_id, merge_request_id)
                REFERENCES merge_requests(workspace_id, merge_request_id) ON DELETE CASCADE
         );
         CREATE UNIQUE INDEX merge_request_merge_operation_ids
             ON merge_request_thread_events(workspace_id, operation_id)
             WHERE operation_id IS NOT NULL;

         CREATE TABLE merge_request_review_capabilities (
            workspace_id TEXT NOT NULL,
            merge_request_id TEXT NOT NULL,
            request_event_seq INTEGER NOT NULL,
            capability_token TEXT PRIMARY KEY,
            issued_by_assignment_id TEXT NOT NULL,
            reviewer_runtime_id TEXT NOT NULL,
            reviewer_worker_id TEXT NOT NULL,
            reviewer_profile TEXT NOT NULL,
            issued_at TEXT NOT NULL,
            consumed_at TEXT,
            status TEXT NOT NULL CHECK (status IN ('issued', 'consumed', 'revoked')),
            FOREIGN KEY (workspace_id, merge_request_id, request_event_seq)
                REFERENCES merge_request_thread_events(workspace_id, merge_request_id, event_seq)
                ON DELETE CASCADE
         );
         CREATE TABLE merge_request_reviewer_child_sessions (
            workspace_id TEXT NOT NULL,
            child_session_id TEXT NOT NULL,
            parent_runtime_id TEXT NOT NULL,
            parent_worker_id TEXT NOT NULL,
            reviewer_profile TEXT NOT NULL,
            registered_at TEXT NOT NULL,
            status TEXT NOT NULL CHECK (status IN ('active', 'consumed')),
            PRIMARY KEY (workspace_id, child_session_id)
         );",
    )?;
    Ok(())
}

fn verify_latest_schema(conn: &Connection) -> Result<(), MergeRequestError> {
    for table in [
        "merge_requests",
        "merge_request_thread_events",
        "merge_request_review_capabilities",
        "merge_request_reviewer_child_sessions",
    ] {
        let exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
            params![table],
            |row| row.get(0),
        )?;
        if !exists {
            return Err(MergeRequestError::Corrupt(format!(
                "schema v{SCHEMA_VERSION} is missing table `{table}`"
            )));
        }
    }
    foreign_key_check(conn)
}

fn foreign_key_check(conn: &Connection) -> Result<(), MergeRequestError> {
    let violation: Option<(String, i64)> = conn
        .query_row("PRAGMA foreign_key_check", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .optional()?;
    if let Some((table, rowid)) = violation {
        return Err(MergeRequestError::Corrupt(format!(
            "foreign key violation in `{table}` row {rowid}"
        )));
    }
    Ok(())
}
