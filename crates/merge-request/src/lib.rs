use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use std::{
    path::Path,
    sync::{Arc, Mutex},
};
use thiserror::Error;
use uuid::Uuid;

const SCHEMA_VERSION: i64 = 12;
const MAX_BODY_BYTES: usize = 16 * 1024;
const DOMAIN_TABLES: [&str; 5] = [
    "merge_requests",
    "merge_request_ticket_relations",
    "merge_request_thread_events",
    "merge_request_review_grants",
    "merge_request_reviewer_child_sessions",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
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

    fn parse(v: &str) -> Result<Self, MergeRequestError> {
        match v {
            "open" => Ok(Self::Open),
            "merged" => Ok(Self::Merged),
            "closed" => Ok(Self::Closed),
            _ => Err(MergeRequestError::Corrupt(format!("unknown state `{v}`"))),
        }
    }
}
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
    pub ticket_item_revision: String,
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
    pub ticket_item_revision: String,
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
        }
    }
    fn bound_bodies(&mut self) {
        match self {
            Self::Review(value) => {
                truncate_body(&mut value.body);
                for finding in &mut value.findings {
                    truncate_body(&mut finding.body);
                }
            }
            Self::ReviewRevoked(value) => truncate_body(&mut value.reason),
            Self::ReviewCancelled(value) => truncate_body(&mut value.reason),
            Self::Comment(value) => truncate_body(&mut value.body),
            Self::ReviewRequested(_) | Self::Merge(_) => {}
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
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
impl MergeRequest {
    pub fn effective_review(&self, subject: &str) -> Option<&ReviewEvent> {
        self.thread.iter().rev().find_map(|e|match e{MergeRequestThreadEvent::Review(v)if v.subject_ref==subject&&!self.thread.iter().any(|x|matches!(x,MergeRequestThreadEvent::ReviewRevoked(r)if r.review_event_id==v.event_id))=>Some(v),_=>None})
    }
}

#[derive(Clone, Debug, Default)]
pub struct MergeRequestListQuery {
    pub state: Option<MergeRequestState>,
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
    pub ticket_item_revision: String,
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
#[derive(Debug, Clone)]
pub struct CompleteTicket {
    pub ticket_id: String,
    pub operation_id: String,
    pub item_revision: String,
    pub merge_request_ids: Vec<String>,
    pub requirement_approval_event_id: String,
    pub auth: MergeRequestAuth,
    pub now: DateTime<Utc>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TicketCompletionEvent {
    pub operation_id: String,
    pub ticket_id: String,
    pub item_revision: String,
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
        let t = c.transaction()?;
        let conflict:bool=t.query_row("SELECT EXISTS(SELECT 1 FROM merge_request_ticket_relations rel JOIN merge_requests mr ON mr.workspace_id=rel.workspace_id AND mr.merge_request_id=rel.merge_request_id WHERE rel.workspace_id=?1 AND rel.ticket_id=?2 AND mr.repository_id=?3 AND mr.state='open')",params![i.auth.workspace_id,i.ticket_id,i.repository_id],|r|r.get(0))?;
        if conflict {
            return Err(MergeRequestError::Conflict(
                "Ticket already has an open Merge Request for this repository; advance that Merge Request's existing selector_from with a normal non-force push instead of opening a replacement".into(),
            ));
        }
        let now = i.now.to_rfc3339();
        t.execute(
            "INSERT INTO merge_requests VALUES(?1,?2,?3,'open',?4,?5,?6,?6)",
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
        if !i.summary.trim().is_empty() {
            let e = CommentEvent {
                event_id: Uuid::now_v7().to_string(),
                sequence: 1,
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
        self.assigned(&i.auth, &i.ticket_id, &mr.repository_id)?;
        if mr.state == MergeRequestState::Closed
            || (mr.state == MergeRequestState::Open && mr.selector_from.is_none())
        {
            return Err(MergeRequestError::Conflict(
                "Merge Request is not reviewable".into(),
            ));
        }
        let mut c = self.lock()?;
        let t = c.transaction()?;
        let current_revision = current_ticket_revision(&t, &i.auth.workspace_id, &i.ticket_id)?
            .ok_or(MergeRequestError::NotFound)?;
        if current_revision != i.ticket_item_revision {
            return Err(MergeRequestError::Conflict(
                "Ticket item revision changed before review request".into(),
            ));
        }
        let current_mr = load_mr(&t, &i.auth.workspace_id, &i.merge_request_id)?
            .filter(|request| request.ticket_ids.iter().any(|id| id == &i.ticket_id))
            .ok_or(MergeRequestError::NotFound)?;
        if current_mr.state == MergeRequestState::Closed
            || (current_mr.state == MergeRequestState::Open && current_mr.selector_from.is_none())
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
            ticket_item_revision: i.ticket_item_revision,
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
                    AND g.status='issued' AND mr.state IN('open','merged')",
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
        bounded_body("review body", &i.body)?;
        for finding in &i.findings {
            bounded_body("review finding", &finding.body)?;
        }
        let mut c = self.lock()?;
        let t = c.transaction()?;
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
                    AND g.status='issued' AND mr.state IN('open','merged')",
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
        let (ticket_item_revision, ticket_merge_request_subjects) = load_mr(&t, &ws, &mr)?
            .and_then(|request| {
                request.thread.into_iter().find_map(|event| match event {
                    MergeRequestThreadEvent::ReviewRequested(event) if event.event_id == req => {
                        Some((
                            event.ticket_item_revision,
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
            ticket_item_revision,
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
        t.commit()?;
        Ok(e)
    }
    pub fn revoke_review(
        &self,
        i: RevokeMergeRequestReview,
    ) -> Result<ReviewRevokedEvent, MergeRequestError> {
        bounded_body("revocation reason", &i.reason)?;
        let mr = self.get_for_operation(&i.auth.workspace_id, &i.merge_request_id, &i.ticket_id)?;
        self.assigned(&i.auth, &i.ticket_id, &mr.repository_id)?;
        let r = mr
            .thread
            .iter()
            .find_map(|x| match x {
                MergeRequestThreadEvent::Review(v) if v.event_id == i.review_event_id => Some(v),
                _ => None,
            })
            .ok_or(MergeRequestError::NotFound)?;
        if mr.thread.iter().any(|x|matches!(x,MergeRequestThreadEvent::ReviewRevoked(v)if v.review_event_id==r.event_id)){return Err(MergeRequestError::Conflict("already revoked".into()))}
        let mut c = self.lock()?;
        let t = c.transaction()?;
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
        if mr.state != MergeRequestState::Open {
            b.push("Merge Request is not open".into())
        }
        if mr.selector_from.is_none() {
            b.push("selector_from requires repair".into())
        }
        if let Some(review) = &review {
            let connection = self.lock()?;
            let current_revision =
                current_ticket_revision(&connection, &i.auth.workspace_id, &i.ticket_id)?
                    .ok_or(MergeRequestError::NotFound)?;
            if review.ticket_item_revision != current_revision {
                b.push(
                    "approval predates the current Ticket revision; fresh review is required"
                        .into(),
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
        self.completion_auth(&i.auth, &i.ticket_id, &mr.repository_id)?;
        if mr.state != MergeRequestState::Open {
            return Err(MergeRequestError::Conflict(
                "Merge Request is not open".into(),
            ));
        }
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
        let current_revision =
            current_ticket_revision(&connection, &mr.workspace_id, &i.ticket_id)?
                .ok_or(MergeRequestError::NotFound)?;
        if review.ticket_item_revision != current_revision {
            return Err(MergeRequestError::NotReady(
                "approval predates the current Ticket revision; fresh review is required".into(),
            ));
        }
        let state: Option<String> = connection
            .query_row(
                "SELECT workflow_state FROM typed_tickets WHERE workspace_id=?1 AND ticket_id=?2",
                params![mr.workspace_id, i.ticket_id],
                |row| row.get(0),
            )
            .optional()?;
        if state.as_deref() != Some("inprogress") {
            return Err(MergeRequestError::Conflict(
                "Ticket must be inprogress".into(),
            ));
        }
        Ok(())
    }

    pub fn complete(&self, i: CompleteMergeRequest) -> Result<MergeEvent, MergeRequestError> {
        self.validate_completion(&i)?;
        let mut connection = self.lock()?;
        let transaction = connection.transaction()?;
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
        if mr.state != MergeRequestState::Open {
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
        let current_revision =
            current_ticket_revision(&transaction, &mr.workspace_id, &i.ticket_id)?
                .ok_or(MergeRequestError::NotFound)?;
        if review.ticket_item_revision != current_revision {
            return Err(MergeRequestError::NotReady(
                "approval predates the current Ticket revision; fresh review is required".into(),
            ));
        }
        let state: Option<String> = transaction
            .query_row(
                "SELECT workflow_state FROM typed_tickets WHERE workspace_id=?1 AND ticket_id=?2",
                params![mr.workspace_id, i.ticket_id],
                |row| row.get(0),
            )
            .optional()?;
        if state.as_deref() != Some("inprogress") {
            return Err(MergeRequestError::Conflict(
                "Ticket must be inprogress".into(),
            ));
        }
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
            "UPDATE merge_requests SET state='merged',updated_at=?3
              WHERE workspace_id=?1 AND merge_request_id=?2 AND state='open'",
            params![mr.workspace_id, mr.merge_request_id, i.now.to_rfc3339()],
        )?;
        ticket_event(&transaction, &mr, &event, &i.auth.assignment_id)?;
        transaction.commit()?;
        Ok(event)
    }

    pub fn complete_ticket(
        &self,
        mut input: CompleteTicket,
    ) -> Result<TicketCompletionEvent, MergeRequestError> {
        nonempty("operation_id", &input.operation_id)?;
        nonempty("item_revision", &input.item_revision)?;
        nonempty(
            "requirement_approval_event_id",
            &input.requirement_approval_event_id,
        )?;
        if input.merge_request_ids.is_empty() {
            return Err(MergeRequestError::NotReady(
                "Ticket completion requires at least one linked Merge Request result".into(),
            ));
        }
        input.merge_request_ids.sort();
        if input
            .merge_request_ids
            .windows(2)
            .any(|ids| ids[0] == ids[1])
        {
            return Err(MergeRequestError::Validation(
                "merge_request_ids must not contain duplicates".into(),
            ));
        }
        let mut connection = self.lock()?;
        let transaction = connection.transaction()?;
        let existing: Option<String> = transaction
            .query_row(
                "SELECT payload.value
                   FROM typed_ticket_event_attributes operation
                   JOIN typed_ticket_event_attributes payload
                     ON payload.workspace_id=operation.workspace_id
                    AND payload.ticket_id=operation.ticket_id
                    AND payload.event_index=operation.event_index
                    AND payload.key='ticket_completion'
                  WHERE operation.workspace_id=?1 AND operation.ticket_id=?2
                    AND operation.key='operation_id' AND operation.value=?3",
                params![input.auth.workspace_id, input.ticket_id, input.operation_id],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(payload) = existing {
            let event: TicketCompletionEvent = json(&payload)?;
            if event.item_revision == input.item_revision
                && event.merge_request_ids == input.merge_request_ids
                && event.requirement_approval_event_id == input.requirement_approval_event_id
                && event.completed_by == input.auth.actor()
            {
                return Ok(event);
            }
            return Err(MergeRequestError::Conflict(
                "Ticket completion operation fingerprint mismatch".into(),
            ));
        }
        let current_revision =
            current_ticket_revision(&transaction, &input.auth.workspace_id, &input.ticket_id)?
                .ok_or(MergeRequestError::NotFound)?;
        if current_revision != input.item_revision {
            return Err(MergeRequestError::Conflict(format!(
                "Ticket item revision changed from `{}` to `{current_revision}`; repeat requirement review before completion",
                input.item_revision
            )));
        }
        let state: String = transaction.query_row(
            "SELECT workflow_state FROM typed_tickets WHERE workspace_id=?1 AND ticket_id=?2",
            params![input.auth.workspace_id, input.ticket_id],
            |row| row.get(0),
        )?;
        if state != "inprogress" {
            return Err(MergeRequestError::Conflict(
                "Ticket must be inprogress".into(),
            ));
        }
        let assignment_is_current: bool = transaction.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM ticket_current_worker_assignments
                 WHERE workspace_id=?1 AND ticket_id=?2 AND assignment_id=?3
             )",
            params![
                input.auth.workspace_id,
                input.ticket_id,
                input.auth.assignment_id
            ],
            |row| row.get(0),
        )?;
        if !assignment_is_current {
            return Err(MergeRequestError::Unauthorized(
                "Ticket completion assignment changed before commit".into(),
            ));
        }
        let actual_ids =
            linked_merge_request_ids(&transaction, &input.auth.workspace_id, &input.ticket_id)?;
        if actual_ids != input.merge_request_ids {
            return Err(MergeRequestError::Conflict(
                "linked Merge Request set changed; refresh Ticket completion evidence".into(),
            ));
        }
        let mut requests = Vec::with_capacity(actual_ids.len());
        let mut merged_subjects = Vec::with_capacity(actual_ids.len());
        for merge_request_id in &actual_ids {
            let request = load_mr(&transaction, &input.auth.workspace_id, merge_request_id)?
                .ok_or(MergeRequestError::NotFound)?;
            if request.state != MergeRequestState::Merged {
                return Err(MergeRequestError::NotReady(format!(
                    "Merge Request `{merge_request_id}` has no merged result"
                )));
            }
            let merge = request
                .thread
                .iter()
                .rev()
                .find_map(|event| match event {
                    MergeRequestThreadEvent::Merge(event) => Some(event),
                    _ => None,
                })
                .ok_or_else(|| {
                    MergeRequestError::Corrupt(format!(
                        "merged Merge Request `{merge_request_id}` has no MergeResult"
                    ))
                })?;
            let review = request
                .thread
                .iter()
                .find_map(|event| match event {
                    MergeRequestThreadEvent::Review(review)
                        if review.event_id == merge.approval_event_id
                            && review.subject_ref == merge.approved_source_ref =>
                    {
                        Some(review)
                    }
                    _ => None,
                })
                .filter(|review| {
                    !request.thread.iter().any(|event| {
                        matches!(
                            event,
                            MergeRequestThreadEvent::ReviewRevoked(revoked)
                                if revoked.review_event_id == review.event_id
                        )
                    })
                })
                .ok_or_else(|| {
                    MergeRequestError::NotReady(format!(
                        "Merge Request `{merge_request_id}` integration approval is missing or revoked"
                    ))
                })?;
            if review.decision != ReviewDecision::Approve {
                return Err(MergeRequestError::NotReady(format!(
                    "Merge Request `{merge_request_id}` lacks an approved integration result"
                )));
            }
            merged_subjects.push(MergeRequestReviewSubject {
                merge_request_id: merge_request_id.clone(),
                subject_ref: merge.approved_source_ref.clone(),
            });
            requests.push(request);
        }
        let has_requirement_attestation = requests.iter().any(|request| {
            request.thread.iter().rev().any(|event| {
                let MergeRequestThreadEvent::Review(review) = event else {
                    return false;
                };
                review.event_id == input.requirement_approval_event_id
                    && review.decision == ReviewDecision::Approve
                    && review.ticket_item_revision == input.item_revision
                    && review.ticket_merge_request_subjects == merged_subjects
                    && request
                        .effective_review(&review.subject_ref)
                        .is_some_and(|effective| effective.event_id == review.event_id)
            })
        });
        if !has_requirement_attestation {
            return Err(MergeRequestError::NotReady(
                "no effective Reviewer approval attests the current Ticket revision and exact merged source set"
                    .into(),
            ));
        }
        let updated = transaction.execute(
            "UPDATE typed_tickets
                SET workflow_state='done',workflow_state_explicit=1,updated_at=?3
              WHERE workspace_id=?1 AND ticket_id=?2 AND workflow_state='inprogress'",
            params![
                input.auth.workspace_id,
                input.ticket_id,
                input.now.to_rfc3339()
            ],
        )?;
        if updated != 1 {
            return Err(MergeRequestError::Conflict(
                "Ticket state changed during completion".into(),
            ));
        }
        let released = transaction.execute(
            "DELETE FROM ticket_current_worker_assignments
              WHERE workspace_id=?1 AND ticket_id=?2 AND assignment_id=?3",
            params![
                input.auth.workspace_id,
                input.ticket_id,
                input.auth.assignment_id
            ],
        )?;
        if released != 1 {
            return Err(MergeRequestError::Unauthorized(
                "Ticket completion assignment changed while releasing it".into(),
            ));
        }
        let event = TicketCompletionEvent {
            operation_id: input.operation_id,
            ticket_id: input.ticket_id.clone(),
            item_revision: input.item_revision,
            merge_request_ids: input.merge_request_ids,
            requirement_approval_event_id: input.requirement_approval_event_id,
            completed_by: input.auth.actor(),
            created_at: input.now,
        };
        ticket_completion_event(&transaction, &input.auth.workspace_id, &event)?;
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
        let id:Option<String>=c.query_row("SELECT rel.merge_request_id FROM merge_request_ticket_relations rel JOIN merge_requests mr ON mr.workspace_id=rel.workspace_id AND mr.merge_request_id=rel.merge_request_id WHERE rel.workspace_id=?1 AND rel.ticket_id=?2 ORDER BY CASE mr.state WHEN 'open' THEN 0 ELSE 1 END,mr.created_at DESC LIMIT 1",params![ws,ticket],|r|r.get(0)).optional()?;
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
        let state = query.state.map(MergeRequestState::as_str);
        let mut statement = c.prepare(
            "SELECT mr.merge_request_id
             FROM merge_requests mr
             WHERE mr.workspace_id=?1
               AND (?2 IS NULL OR mr.state=?2)
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
    fn completion_auth(
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

fn current_ticket_revision(
    connection: &Connection,
    workspace_id: &str,
    ticket_id: &str,
) -> Result<Option<String>, MergeRequestError> {
    let event_revision = connection
        .query_row(
            "SELECT attribute.value
               FROM typed_ticket_events event
               JOIN typed_ticket_event_attributes attribute
                 ON attribute.workspace_id=event.workspace_id
                AND attribute.ticket_id=event.ticket_id
                AND attribute.event_index=event.event_index
                AND attribute.key='event_id'
              WHERE event.workspace_id=?1 AND event.ticket_id=?2
                AND event.kind IN ('create','item_edit')
              ORDER BY event.event_index DESC
              LIMIT 1",
            params![workspace_id, ticket_id],
            |row| row.get(0),
        )
        .optional()?;
    if event_revision.is_some() {
        return Ok(event_revision);
    }
    connection
        .query_row(
            "SELECT updated_at FROM typed_tickets WHERE workspace_id=?1 AND ticket_id=?2",
            params![workspace_id, ticket_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(Into::into)
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
        state: MergeRequestState::parse(&state)?,
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
    t.execute("INSERT INTO typed_ticket_events(workspace_id,ticket_id,event_index,kind,author,at,from_state,to_state,heading,body)VALUES(?1,?2,?3,'comment',?4,?5,NULL,NULL,'Merge Request integrated',?6)",params![mr.workspace_id,ticket,n,format!("worker:{}:{}",e.merged_by.runtime_id,e.merged_by.worker_id),e.created_at.to_rfc3339(),format!("Merge Request `{}` integrated approved source ref `{}`; Ticket remains in progress until guarded Ticket completion.",mr.merge_request_id,e.approved_source_ref)])?;
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

fn ticket_completion_event(
    transaction: &Transaction<'_>,
    workspace_id: &str,
    event: &TicketCompletionEvent,
) -> Result<(), MergeRequestError> {
    let event_index: i64 = transaction.query_row(
        "SELECT COALESCE(MAX(event_index),-1)+1 FROM typed_ticket_events WHERE workspace_id=?1 AND ticket_id=?2",
        params![workspace_id, event.ticket_id],
        |row| row.get(0),
    )?;
    transaction.execute(
        "INSERT INTO typed_ticket_events(workspace_id,ticket_id,event_index,kind,author,at,from_state,to_state,heading,body)
         VALUES(?1,?2,?3,'state_changed',?4,?5,'inprogress','done','Ticket requirements completed',?6)",
        params![
            workspace_id,
            event.ticket_id,
            event_index,
            format!(
                "worker:{}:{}",
                event.completed_by.runtime_id, event.completed_by.worker_id
            ),
            event.created_at.to_rfc3339(),
            format!(
                "Current Ticket revision `{}` was approved by Review `{}` and completed with Merge Requests: {}.",
                event.item_revision,
                event.requirement_approval_event_id,
                event.merge_request_ids.join(", ")
            )
        ],
    )?;
    for (key, value) in [
        ("operation_id", event.operation_id.clone()),
        ("item_revision", event.item_revision.clone()),
        (
            "requirement_approval_event_id",
            event.requirement_approval_event_id.clone(),
        ),
        (
            "merge_request_ids",
            serde_json::to_string(&event.merge_request_ids)
                .map_err(|error| MergeRequestError::Operation(error.to_string()))?,
        ),
        (
            "ticket_completion",
            serde_json::to_string(event)
                .map_err(|error| MergeRequestError::Operation(error.to_string()))?,
        ),
    ] {
        transaction.execute(
            "INSERT INTO typed_ticket_event_attributes VALUES(?1,?2,?3,?4,?5)",
            params![workspace_id, event.ticket_id, event_index, key, value],
        )?;
    }
    Ok(())
}

pub fn migrate(c: &Connection) -> Result<(), MergeRequestError> {
    match schema_state(c)? {
        SchemaState::Fresh => fresh(c),
        SchemaState::Current(SCHEMA_VERSION) => verify(c),
        SchemaState::Current(v) => Err(MergeRequestError::Operation(format!(
            "unsupported schema {v}"
        ))),
    }
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
    t.execute_batch("CREATE TABLE merge_requests(workspace_id TEXT NOT NULL,merge_request_id TEXT NOT NULL,repository_id TEXT NOT NULL,state TEXT NOT NULL CHECK(state IN('open','merged','closed')),selector_from TEXT,selector_to TEXT NOT NULL,created_at TEXT NOT NULL,updated_at TEXT NOT NULL,PRIMARY KEY(workspace_id,merge_request_id),FOREIGN KEY(workspace_id,repository_id)REFERENCES repositories(workspace_id,repository_id));CREATE TABLE merge_request_ticket_relations(workspace_id TEXT NOT NULL,merge_request_id TEXT NOT NULL,ticket_id TEXT NOT NULL,relation_kind TEXT NOT NULL CHECK(relation_kind='implements'),created_at TEXT NOT NULL,PRIMARY KEY(workspace_id,merge_request_id,ticket_id),FOREIGN KEY(workspace_id,merge_request_id)REFERENCES merge_requests(workspace_id,merge_request_id)ON DELETE CASCADE,FOREIGN KEY(workspace_id,ticket_id)REFERENCES typed_tickets(workspace_id,ticket_id)ON DELETE CASCADE);CREATE TABLE merge_request_thread_events(workspace_id TEXT NOT NULL,merge_request_id TEXT NOT NULL,event_id TEXT NOT NULL,sequence INTEGER NOT NULL,kind TEXT NOT NULL CHECK(kind IN('review_requested','review','review_revoked','review_cancelled','comment','merge')),payload_json TEXT NOT NULL,operation_id TEXT,created_at TEXT NOT NULL,PRIMARY KEY(workspace_id,merge_request_id,event_id),UNIQUE(workspace_id,merge_request_id,sequence),FOREIGN KEY(workspace_id,merge_request_id)REFERENCES merge_requests(workspace_id,merge_request_id)ON DELETE CASCADE);CREATE UNIQUE INDEX merge_request_merge_operations ON merge_request_thread_events(workspace_id,operation_id)WHERE operation_id IS NOT NULL;CREATE TABLE merge_request_review_grants(workspace_id TEXT NOT NULL,merge_request_id TEXT NOT NULL,request_event_id TEXT NOT NULL,subject_ref TEXT NOT NULL,reviewer_runtime_id TEXT NOT NULL,reviewer_worker_id TEXT NOT NULL,capability_token TEXT PRIMARY KEY,issued_at TEXT NOT NULL,consumed_at TEXT,revoked_at TEXT,status TEXT NOT NULL CHECK(status IN('issued','consumed','revoked')),FOREIGN KEY(workspace_id,merge_request_id,request_event_id)REFERENCES merge_request_thread_events(workspace_id,merge_request_id,event_id)ON DELETE CASCADE);CREATE TABLE merge_request_reviewer_child_sessions(workspace_id TEXT NOT NULL,child_session_id TEXT NOT NULL,parent_runtime_id TEXT NOT NULL,parent_worker_id TEXT NOT NULL,reviewer_profile TEXT NOT NULL,registered_at TEXT NOT NULL,status TEXT NOT NULL CHECK(status IN('active','consumed')),PRIMARY KEY(workspace_id,child_session_id));")?;
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
