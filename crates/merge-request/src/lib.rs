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
const PREVIOUS_SCHEMA_VERSION: i64 = 11;
const MAX_BODY_BYTES: usize = 16 * 1024;
const DOMAIN_TABLES: [&str; 5] = [
    "merge_requests",
    "merge_request_ticket_relations",
    "merge_request_thread_events",
    "merge_request_review_grants",
    "merge_request_reviewer_child_sessions",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MergeRequestState {
    Open,
    Merged,
    Closed,
}
impl MergeRequestState {
    fn parse(v: &str) -> Result<Self, MergeRequestError> {
        match v {
            "draft" | "open" => Ok(Self::Open),
            "merged" => Ok(Self::Merged),
            "closed" => Ok(Self::Closed),
            _ => Err(MergeRequestError::Corrupt(format!("unknown state `{v}`"))),
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
    pub code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    pub body: String,
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

impl MergeRequestAuth {
    fn actor(&self) -> WorkerIdentity {
        WorkerIdentity {
            runtime_id: self.runtime_id.clone(),
            worker_id: self.worker_id.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewRequestedEvent {
    pub event_id: String,
    pub sequence: u64,
    pub subject_ref: String,
    pub requested_by: WorkerIdentity,
    pub reviewer: WorkerIdentity,
    pub created_at: DateTime<Utc>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewEvent {
    pub event_id: String,
    pub sequence: u64,
    pub request_event_id: String,
    pub subject_ref: String,
    pub decision: ReviewDecision,
    pub body: String,
    pub findings: Vec<ReviewFinding>,
    pub reviewer: WorkerIdentity,
    pub created_at: DateTime<Utc>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewRevokedEvent {
    pub event_id: String,
    pub sequence: u64,
    pub review_event_id: String,
    pub subject_ref: String,
    pub reason: String,
    pub revoked_by: WorkerIdentity,
    pub created_at: DateTime<Utc>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewCancelledEvent {
    pub event_id: String,
    pub sequence: u64,
    pub request_event_id: String,
    pub subject_ref: String,
    pub reason: String,
    pub created_at: DateTime<Utc>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommentEvent {
    pub event_id: String,
    pub sequence: u64,
    pub body: String,
    pub author: WorkerIdentity,
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
    pub event_id: String,
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    pub ticket_id: String,
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
#[derive(Debug, Clone)]
pub struct SubmitMergeRequestReview {
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
    pub ticket_id: String,
    pub review_event_id: String,
    pub reason: String,
    pub auth: MergeRequestAuth,
    pub now: DateTime<Utc>,
}
#[derive(Debug, Clone)]
pub struct RepairSelectorFrom {
    pub workspace_id: String,
    pub ticket_id: String,
    pub selector_from: String,
    pub resolved_subject_ref: String,
    pub repaired_by: WorkerIdentity,
    pub reason: String,
    pub now: DateTime<Utc>,
}
#[derive(Debug, Clone)]
pub struct ReadinessCheck {
    pub ticket_id: String,
    pub current_subject_ref: Option<String>,
    pub auth: MergeRequestAuth,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
        self.assigned(&i.auth, &i.ticket_id, &i.repository_id)?;
        let mut c = self.lock()?;
        let t = c.transaction()?;
        let conflict:bool=t.query_row("SELECT EXISTS(SELECT 1 FROM merge_request_ticket_relations rel JOIN merge_requests mr ON mr.workspace_id=rel.workspace_id AND mr.merge_request_id=rel.merge_request_id WHERE rel.workspace_id=?1 AND rel.ticket_id=?2 AND mr.state='open')",params![i.auth.workspace_id,i.ticket_id],|r|r.get(0))?;
        if conflict {
            return Err(MergeRequestError::Conflict(
                "Ticket already has an open Merge Request".into(),
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
        self.get(&i.auth.workspace_id, &i.ticket_id)
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
        i: RequestMergeRequestReview,
    ) -> Result<RequestedMergeRequestReview, MergeRequestError> {
        nonempty("subject_ref", &i.subject_ref)?;
        let mr = self.get(&i.auth.workspace_id, &i.ticket_id)?;
        self.assigned(&i.auth, &i.ticket_id, &mr.repository_id)?;
        if mr.state != MergeRequestState::Open || mr.selector_from.is_none() {
            return Err(MergeRequestError::Conflict(
                "Merge Request is not reviewable".into(),
            ));
        }
        let mut c = self.lock()?;
        let t = c.transaction()?;
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
                    AND g.status='issued' AND mr.state='open'",
                params![i.capability_token, i.ticket_id],
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
        if subject != i.current_subject_ref {
            let e = ReviewCancelledEvent {
                event_id: Uuid::now_v7().to_string(),
                sequence: next_seq(&t, &ws, &mr)?,
                request_event_id: req,
                subject_ref: subject,
                reason: "selector_from moved before submission".into(),
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
        let mr = self.get(&i.auth.workspace_id, &i.ticket_id)?;
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
        let mr = self.get(&i.auth.workspace_id, &i.ticket_id)?;
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
        match (&i.current_subject_ref, &review) {
            (None, _) => b.push("selector_from could not be resolved".into()),
            (Some(_), None) => b.push("current source ref has no valid review".into()),
            (_, Some(r)) if r.decision == ReviewDecision::RequestChanges => {
                b.push("current source ref requests changes".into())
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
        let mr = self.get(&i.auth.workspace_id, &i.ticket_id)?;
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
        let state: Option<String> = self
            .lock()?
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
        let mr = load_mr_for_ticket(&transaction, &i.auth.workspace_id, &i.ticket_id)?
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
        transaction.execute(
            "UPDATE typed_tickets
                SET workflow_state='done',workflow_state_explicit=1,updated_at=?3
              WHERE workspace_id=?1 AND ticket_id=?2 AND workflow_state='inprogress'",
            params![mr.workspace_id, i.ticket_id, i.now.to_rfc3339()],
        )?;
        let released_assignment = transaction.execute(
            "DELETE FROM ticket_current_worker_assignments
              WHERE workspace_id=?1 AND ticket_id=?2 AND assignment_id=?3",
            params![mr.workspace_id, i.ticket_id, i.auth.assignment_id],
        )?;
        if released_assignment != 1 {
            return Err(MergeRequestError::Unauthorized(
                "completion assignment changed while closing Ticket".into(),
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

    pub fn repair_selector_from(
        &self,
        i: RepairSelectorFrom,
    ) -> Result<MergeRequest, MergeRequestError> {
        nonempty("selector_from", &i.selector_from)?;
        nonempty("resolved_subject_ref", &i.resolved_subject_ref)?;
        let mr = self.get(&i.workspace_id, &i.ticket_id)?;
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
        self.get(&i.workspace_id, &i.ticket_id)
    }
    pub fn get(&self, ws: &str, ticket: &str) -> Result<MergeRequest, MergeRequestError> {
        let c = self.lock()?;
        let id:Option<String>=c.query_row("SELECT rel.merge_request_id FROM merge_request_ticket_relations rel JOIN merge_requests mr ON mr.workspace_id=rel.workspace_id AND mr.merge_request_id=rel.merge_request_id WHERE rel.workspace_id=?1 AND rel.ticket_id=?2 ORDER BY CASE mr.state WHEN 'open' THEN 0 ELSE 1 END,mr.created_at DESC LIMIT 1",params![ws,ticket],|r|r.get(0)).optional()?;
        match id {
            Some(id) => load_mr(&c, ws, &id)?.ok_or(MergeRequestError::NotFound),
            None => Err(MergeRequestError::NotFound),
        }
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
fn load_mr_for_ticket(
    connection: &Connection,
    workspace_id: &str,
    ticket_id: &str,
) -> Result<Option<MergeRequest>, MergeRequestError> {
    let merge_request_id: Option<String> = connection
        .query_row(
            "SELECT rel.merge_request_id
               FROM merge_request_ticket_relations rel
               JOIN merge_requests mr
                 ON mr.workspace_id=rel.workspace_id
                AND mr.merge_request_id=rel.merge_request_id
              WHERE rel.workspace_id=?1 AND rel.ticket_id=?2
              ORDER BY CASE mr.state WHEN 'open' THEN 0 ELSE 1 END,mr.created_at DESC
              LIMIT 1",
            params![workspace_id, ticket_id],
            |row| row.get(0),
        )
        .optional()?;
    match merge_request_id {
        Some(id) => load_mr(connection, workspace_id, &id),
        None => Ok(None),
    }
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
    t.execute("INSERT INTO typed_ticket_events(workspace_id,ticket_id,event_index,kind,author,at,from_state,to_state,heading,body)VALUES(?1,?2,?3,'state_changed',?4,?5,'inprogress','done','Merge Request completed',?6)",params![mr.workspace_id,ticket,n,format!("worker:{}:{}",e.merged_by.runtime_id,e.merged_by.worker_id),e.created_at.to_rfc3339(),format!("Approved source ref `{}` completed implementation.",e.approved_source_ref)])?;
    for (k, v) in [
        ("implementation_assignment_id", a),
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
        SchemaState::Current(SCHEMA_VERSION) => verify(c),
        SchemaState::Current(PREVIOUS_SCHEMA_VERSION) => from_v11(c, PreviousSchemaMarker::Current),
        SchemaState::Legacy(PREVIOUS_SCHEMA_VERSION) => from_v11(c, PreviousSchemaMarker::Legacy),
        SchemaState::Current(v) => Err(MergeRequestError::Operation(format!(
            "unsupported schema {v}"
        ))),
        SchemaState::Legacy(v) => Err(MergeRequestError::Operation(format!(
            "unsupported legacy schema {v}"
        ))),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SchemaState {
    Fresh,
    Current(i64),
    Legacy(i64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PreviousSchemaMarker {
    Current,
    Legacy,
}

fn schema_state(c: &Connection) -> Result<SchemaState, MergeRequestError> {
    let (current, legacy): (bool, bool) = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='merge_request_schema'),EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='merge_request_schema_migrations')",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if current && legacy {
        return Err(MergeRequestError::Corrupt(
            "both current and legacy schema markers exist".into(),
        ));
    }
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
    if legacy {
        let (count, version): (i64, Option<i64>) = c.query_row(
            "SELECT COUNT(*),MAX(version) FROM merge_request_schema_migrations",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if count != 1 {
            return Err(MergeRequestError::Corrupt(
                "legacy schema marker must contain exactly one version".into(),
            ));
        }
        let version = version.ok_or_else(|| {
            MergeRequestError::Corrupt("legacy schema marker version is null".into())
        })?;
        return Ok(SchemaState::Legacy(version));
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
    tables(&t, true)?;
    t.execute("INSERT INTO merge_request_schema VALUES(1,12)", [])?;
    fk(&t)?;
    t.commit()?;
    Ok(())
}
fn tables(t: &Transaction<'_>, marker: bool) -> Result<(), MergeRequestError> {
    if marker {
        t.execute_batch("CREATE TABLE merge_request_schema(singleton INTEGER PRIMARY KEY CHECK(singleton=1),version INTEGER NOT NULL);")?
    }
    t.execute_batch("CREATE TABLE merge_requests(workspace_id TEXT NOT NULL,merge_request_id TEXT NOT NULL,repository_id TEXT NOT NULL,state TEXT NOT NULL CHECK(state IN('open','merged','closed')),selector_from TEXT,selector_to TEXT NOT NULL,created_at TEXT NOT NULL,updated_at TEXT NOT NULL,PRIMARY KEY(workspace_id,merge_request_id),FOREIGN KEY(workspace_id,repository_id)REFERENCES repositories(workspace_id,repository_id));CREATE TABLE merge_request_ticket_relations(workspace_id TEXT NOT NULL,merge_request_id TEXT NOT NULL,ticket_id TEXT NOT NULL,relation_kind TEXT NOT NULL CHECK(relation_kind='implements'),created_at TEXT NOT NULL,PRIMARY KEY(workspace_id,merge_request_id,ticket_id),FOREIGN KEY(workspace_id,merge_request_id)REFERENCES merge_requests(workspace_id,merge_request_id)ON DELETE CASCADE,FOREIGN KEY(workspace_id,ticket_id)REFERENCES typed_tickets(workspace_id,ticket_id)ON DELETE CASCADE);CREATE TABLE merge_request_thread_events(workspace_id TEXT NOT NULL,merge_request_id TEXT NOT NULL,event_id TEXT NOT NULL,sequence INTEGER NOT NULL,kind TEXT NOT NULL CHECK(kind IN('review_requested','review','review_revoked','review_cancelled','comment','merge')),payload_json TEXT NOT NULL,operation_id TEXT,created_at TEXT NOT NULL,PRIMARY KEY(workspace_id,merge_request_id,event_id),UNIQUE(workspace_id,merge_request_id,sequence),FOREIGN KEY(workspace_id,merge_request_id)REFERENCES merge_requests(workspace_id,merge_request_id)ON DELETE CASCADE);CREATE UNIQUE INDEX merge_request_merge_operations ON merge_request_thread_events(workspace_id,operation_id)WHERE operation_id IS NOT NULL;CREATE TABLE merge_request_review_grants(workspace_id TEXT NOT NULL,merge_request_id TEXT NOT NULL,request_event_id TEXT NOT NULL,subject_ref TEXT NOT NULL,reviewer_runtime_id TEXT NOT NULL,reviewer_worker_id TEXT NOT NULL,capability_token TEXT PRIMARY KEY,issued_at TEXT NOT NULL,consumed_at TEXT,revoked_at TEXT,status TEXT NOT NULL CHECK(status IN('issued','consumed','revoked')),FOREIGN KEY(workspace_id,merge_request_id,request_event_id)REFERENCES merge_request_thread_events(workspace_id,merge_request_id,event_id)ON DELETE CASCADE);CREATE TABLE merge_request_reviewer_child_sessions(workspace_id TEXT NOT NULL,child_session_id TEXT NOT NULL,parent_runtime_id TEXT NOT NULL,parent_worker_id TEXT NOT NULL,reviewer_profile TEXT NOT NULL,registered_at TEXT NOT NULL,status TEXT NOT NULL CHECK(status IN('active','consumed')),PRIMARY KEY(workspace_id,child_session_id));")?;
    Ok(())
}
fn from_v11(
    c: &Connection,
    previous_marker: PreviousSchemaMarker,
) -> Result<(), MergeRequestError> {
    let t = c.unchecked_transaction()?;
    if previous_marker == PreviousSchemaMarker::Legacy {
        t.execute_batch("CREATE TABLE merge_request_schema(singleton INTEGER PRIMARY KEY CHECK(singleton=1),version INTEGER NOT NULL);")?;
        t.execute(
            "INSERT INTO merge_request_schema VALUES(1,?1)",
            params![PREVIOUS_SCHEMA_VERSION],
        )?;
    }
    t.execute_batch("ALTER TABLE merge_requests RENAME TO merge_requests_v11;ALTER TABLE merge_request_ticket_relations RENAME TO merge_request_ticket_relations_v11;ALTER TABLE merge_request_revisions RENAME TO merge_request_revisions_v11;ALTER TABLE merge_request_revision_paths RENAME TO merge_request_revision_paths_v11;ALTER TABLE merge_request_reviewer_child_sessions RENAME TO merge_request_reviewer_child_sessions_v11;ALTER TABLE merge_request_review_attempts RENAME TO merge_request_review_attempts_v11;ALTER TABLE merge_request_reviews RENAME TO merge_request_reviews_v11;ALTER TABLE merge_request_review_findings RENAME TO merge_request_review_findings_v11;ALTER TABLE merge_request_completion_operations RENAME TO merge_request_completion_operations_v11;")?;
    tables(&t, false)?;
    t.execute("INSERT INTO merge_requests SELECT workspace_id,merge_request_id,repository_id,CASE state WHEN 'draft'THEN'open'ELSE state END,NULL,target_ref_selector,created_at,updated_at FROM merge_requests_v11",[])?;
    t.execute("INSERT INTO merge_request_ticket_relations SELECT * FROM merge_request_ticket_relations_v11",[])?;
    migrate_events(&t)?;
    if previous_marker == PreviousSchemaMarker::Legacy {
        t.execute("DROP TABLE merge_request_schema_migrations", [])?;
    }
    t.execute_batch("DROP TABLE merge_request_review_findings_v11;DROP TABLE merge_request_reviews_v11;DROP TABLE merge_request_review_attempts_v11;DROP TABLE merge_request_reviewer_child_sessions_v11;DROP TABLE merge_request_revision_paths_v11;DROP TABLE merge_request_revisions_v11;DROP TABLE merge_request_completion_operations_v11;DROP TABLE merge_request_ticket_relations_v11;DROP TABLE merge_requests_v11;UPDATE merge_request_schema SET version=12 WHERE singleton=1;")?;
    fk(&t)?;
    t.commit()?;
    Ok(())
}
fn migrate_events(t: &Transaction<'_>) -> Result<(), MergeRequestError> {
    let attempts = {
        let mut s=t.prepare("SELECT a.workspace_id,a.attempt_id,a.merge_request_id,a.parent_runtime_id,a.parent_worker_id,a.child_session_id,a.status,a.created_at,a.consumed_at,r.head_commit FROM merge_request_review_attempts_v11 a JOIN merge_request_revisions_v11 r ON r.workspace_id=a.workspace_id AND r.merge_request_id=a.merge_request_id AND r.revision_id=a.revision_id ORDER BY a.created_at")?;
        s.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, String>(6)?,
                r.get::<_, String>(7)?,
                r.get::<_, Option<String>>(8)?,
                r.get::<_, String>(9)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?
    };
    for (ws, a, mr, pr, pw, child, status, created, consumed, subject) in attempts {
        let req = ReviewRequestedEvent {
            event_id: format!("migrated-request-{a}"),
            sequence: next_seq(t, &ws, &mr)?,
            subject_ref: subject.clone(),
            requested_by: WorkerIdentity {
                runtime_id: pr.clone(),
                worker_id: pw,
            },
            reviewer: WorkerIdentity {
                runtime_id: pr,
                worker_id: child,
            },
            created_at: time(&created)?,
        };
        insert_event(t, &ws, &mr, "review_requested", &req, req.created_at, None)?;
        if status == "submitted" {
            let(row_dec,row_body,row_at):(String,String,String)=t.query_row("SELECT decision,body,submitted_at FROM merge_request_reviews_v11 WHERE workspace_id=?1 AND attempt_id=?2",params![ws,a],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
            let findings = {
                let mut s=t.prepare("SELECT severity,code,path,line,body FROM merge_request_review_findings_v11 WHERE workspace_id=?1 AND attempt_id=?2 ORDER BY ordinal")?;
                s.query_map(params![ws, a], |r| {
                    Ok(ReviewFinding {
                        severity: match r.get::<_, String>(0)?.as_str() {
                            "blocker" => FindingSeverity::Blocker,
                            "major" => FindingSeverity::Major,
                            "minor" => FindingSeverity::Minor,
                            _ => FindingSeverity::Note,
                        },
                        code: r.get(1)?,
                        path: r.get(2)?,
                        line: r.get(3)?,
                        body: r.get(4)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?
            };
            let rev = ReviewEvent {
                event_id: format!("migrated-review-{a}"),
                sequence: next_seq(t, &ws, &mr)?,
                request_event_id: req.event_id,
                subject_ref: subject,
                decision: if row_dec == "approve" {
                    ReviewDecision::Approve
                } else {
                    ReviewDecision::RequestChanges
                },
                body: row_body,
                findings,
                reviewer: req.reviewer,
                created_at: time(&row_at)?,
            };
            insert_event(t, &ws, &mr, "review", &rev, rev.created_at, None)?
        } else {
            let at = consumed.as_deref().unwrap_or(&created);
            let e = ReviewCancelledEvent {
                event_id: format!("migrated-cancel-{a}"),
                sequence: next_seq(t, &ws, &mr)?,
                request_event_id: req.event_id,
                subject_ref: subject,
                reason: format!(
                    "legacy `{status}` review request cancelled because its capability cannot be migrated"
                ),
                created_at: time(at)?,
            };
            insert_event(t, &ws, &mr, "review_cancelled", &e, e.created_at, None)?
        }
    }
    let completed = {
        let mut q=t.prepare("SELECT c.workspace_id,c.operation_id,c.ticket_id,c.target_commit,c.source_commit,c.result_commit,c.strategy,c.resolution,c.completion_actor_runtime_id,c.completion_actor_worker_id,c.updated_at,rel.merge_request_id FROM merge_request_completion_operations_v11 c JOIN merge_request_ticket_relations_v11 rel ON rel.workspace_id=c.workspace_id AND rel.ticket_id=c.ticket_id WHERE c.status='completed' ORDER BY c.updated_at")?;
        q.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, Option<String>>(5)?,
                r.get::<_, Option<String>>(6)?,
                r.get::<_, Option<String>>(7)?,
                r.get::<_, Option<String>>(8)?,
                r.get::<_, Option<String>>(9)?,
                r.get::<_, String>(10)?,
                r.get::<_, String>(11)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?
    };
    for (
        ws,
        op,
        _ticket,
        target,
        source,
        result,
        strategy,
        resolution,
        runtime,
        worker,
        updated,
        mr,
    ) in completed
    {
        let subject = source.ok_or_else(|| {
            MergeRequestError::Operation(format!("completed operation {op} lacks source evidence"))
        })?;
        let approval:Option<String>=t.query_row("SELECT event_id FROM merge_request_thread_events WHERE workspace_id=?1 AND merge_request_id=?2 AND kind='review' AND json_extract(payload_json,'$.subject_ref')=?3 AND json_extract(payload_json,'$.decision')='approve' ORDER BY sequence DESC LIMIT 1",params![ws,mr,subject],|r|r.get(0)).optional()?;
        let approval = approval.ok_or_else(|| {
            MergeRequestError::Operation(format!(
                "completed operation {op} lacks approval evidence"
            ))
        })?;
        let e = MergeEvent {
            event_id: format!("migrated-merge-{op}"),
            sequence: next_seq(t, &ws, &mr)?,
            operation_id: op,
            approval_event_id: approval,
            approved_source_ref: subject,
            target_ref_before: target.ok_or_else(|| {
                MergeRequestError::Operation("completed operation lacks target evidence".into())
            })?,
            target_ref_after: result.ok_or_else(|| {
                MergeRequestError::Operation("completed operation lacks result evidence".into())
            })?,
            strategy: if strategy.as_deref() == Some("merge") {
                MergeStrategy::Merge
            } else {
                MergeStrategy::FastForward
            },
            resolution: match resolution.as_deref() {
                Some("clean") => ConflictResolution::Clean,
                Some("conflicts_resolved") => ConflictResolution::ConflictsResolved,
                _ => ConflictResolution::None,
            },
            merged_by: WorkerIdentity {
                runtime_id: runtime.unwrap_or_else(|| "legacy".into()),
                worker_id: worker.unwrap_or_else(|| "legacy".into()),
            },
            created_at: time(&updated)?,
        };
        insert_event(
            t,
            &ws,
            &mr,
            "merge",
            &e,
            e.created_at,
            Some(&e.operation_id),
        )?;
    }
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
