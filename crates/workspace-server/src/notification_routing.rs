//! Domain writes capture facts atomically; this module alone interprets notification intent.
//! Runtime acceptance, handling and execution authority remain independent. Explicit user
//! communication, internal child notifications and Web subscriptions are not business routes.
use super::*;
#[path = "job_notification_routing.rs"]
pub(super) mod jobs;
pub use jobs::{JobNotificationFacts, JobNotificationSource};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationPurpose {
    NewDelegation,
    ContinuingRequest,
    Attention,
    Result,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationEventKind {
    Assignment,
    Answer,
    ContentEdit,
    Comment,
    Case,
    Tracking,
    Tags,
    Release,
    AssignmentChanged,
    Result,
    MergeRequest,
    Unknown,
}
impl NotificationEventKind {
    fn from_source(kind: &str) -> Self {
        match kind {
            "assignment" => Self::Assignment,
            "assignment_answer" => Self::Answer,
            "content_edit" | "item_edit" => Self::ContentEdit,
            "comment" | "comment_edit" => Self::Comment,
            "case" | "case_update" => Self::Case,
            "tracking" | "assignment_tracking" => Self::Tracking,
            "tags" | "tags_set" | "tags_changed" => Self::Tags,
            "release" => Self::Release,
            "assignment_changed" => Self::AssignmentChanged,
            "backend_job_result" => Self::Result,
            "implementation_result" => Self::Result,
            "mr_review_requested"
            | "mr_review"
            | "mr_review_revoked"
            | "mr_review_cancelled"
            | "mr_merge"
            | "mr_source_changed" => Self::MergeRequest,
            _ => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationReason {
    Requested,
    Attention,
    Result,
    NoRecipient,
    NoFindings,
    StaleContent,
    SourceUnavailable,
    UnsupportedRecipient,
    HistoricalOnly,
    CaseClosed,
    HandlingDone,
    HandlingUnknown,
    SelfNotification,
    NotRequest,
    AssignmentChanged,
    UnknownEvent,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationFailureCode {
    ConfigurationUnavailable,
    RenderingFailed,
    ContentTooLarge,
    InvalidResult,
    SourceLookupFailed,
    SourceNotFound,
    TargetUnavailable,
    TargetStopped,
    WorkspaceMismatch,
    BindingRevoked,
    AssignmentChanged,
    RuntimeRejected,
    AcceptanceUnknown,
    PreparationFailed,
    AcknowledgementFailed,
    RuntimeUnsupported,
    RuntimeUnavailable,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotificationDeliveryKey {
    AssignmentInput {
        assignment_id: String,
        input_event_id: String,
    },
    WorkerNotification {
        delivery_id: String,
    },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketWorkerNotification {
    pub ticket_id: String,
    pub assignment_id: String,
    pub input_event_id: String,
    pub delivery_id: String,
    pub request_id: String,
    pub worker: RuntimeWorkerRef,
    pub purpose: NotificationPurpose,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketWorkerNotificationTarget {
    pub worker: RuntimeWorkerRef,
    pub authentication_binding_id: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Facts {
    principal_kind: Option<String>,
    principal_id: Option<String>,
    runtime_id: Option<String>,
    worker_id: Option<String>,
    status: Option<String>,
    case_state: String,
    released: bool,
}
#[derive(Debug)]
struct Source {
    sequence: i64,
    workspace: String,
    ticket: String,
    input: String,
    role: String,
    assignment: String,
    kind: String,
    actor: Option<String>,
    event_index: Option<i64>,
    facts: Facts,
}
fn json<T: Serialize>(value: &T) -> Result<String> {
    serde_json::to_string(value).map_err(|e| Error::Store(e.to_string()))
}
fn code<T: Serialize>(value: T) -> Result<String> {
    Ok(json(&value)?.trim_matches('"').to_owned())
}
fn decode<T: serde::de::DeserializeOwned>(value: &str) -> Result<T> {
    serde_json::from_str(value).map_err(|e| Error::Store(e.to_string()))
}
fn purpose(kind: NotificationEventKind, role: &str) -> NotificationPurpose {
    match (kind, role) {
        (NotificationEventKind::Assignment, "orchestrator") => NotificationPurpose::NewDelegation,
        (NotificationEventKind::Answer | NotificationEventKind::ContentEdit, "orchestrator") => {
            NotificationPurpose::ContinuingRequest
        }
        (NotificationEventKind::Result, _) => NotificationPurpose::Result,
        _ => NotificationPurpose::Attention,
    }
}
fn policy(
    kind: NotificationEventKind,
    role: &str,
    assignment: &str,
    facts: &Facts,
    self_notification: bool,
) -> (&'static str, NotificationReason) {
    use NotificationReason as R;
    if kind == NotificationEventKind::Unknown {
        return ("defer", R::UnknownEvent);
    }
    if assignment.is_empty() {
        return ("suppress", R::NoRecipient);
    }
    let supported = if role == "orchestrator" {
        facts.principal_kind.as_deref() == Some("subject")
            && facts
                .principal_id
                .as_deref()
                .is_some_and(|id| !id.trim().is_empty())
    } else {
        role == "worker" && facts.principal_kind.as_deref() == Some("worker")
    };
    if !supported {
        return ("suppress", R::UnsupportedRecipient);
    }
    if self_notification {
        return ("suppress", R::SelfNotification);
    }
    if facts.released {
        return ("suppress", R::HistoricalOnly);
    }
    if facts.case_state != "open" {
        return ("suppress", R::CaseClosed);
    }
    if role == "orchestrator" {
        match facts.status.as_deref() {
            Some("waiting" | "inprogress") => {}
            Some("done") => return ("suppress", R::HandlingDone),
            _ => return ("suppress", R::HandlingUnknown),
        }
        if !matches!(
            kind,
            NotificationEventKind::Assignment
                | NotificationEventKind::Answer
                | NotificationEventKind::ContentEdit
                | NotificationEventKind::Result
                | NotificationEventKind::MergeRequest
        ) {
            return ("suppress", R::NotRequest);
        }
    }
    if matches!(
        kind,
        NotificationEventKind::Tracking | NotificationEventKind::Release
    ) {
        return ("suppress", R::NotRequest);
    }
    (
        "deliver",
        match purpose(kind, role) {
            NotificationPurpose::Result => R::Result,
            NotificationPurpose::Attention => R::Attention,
            _ => R::Requested,
        },
    )
}

const FACTS_SQL: &str = "json_object('principal_kind',a.principal_kind,'principal_id',a.principal_id,'runtime_id',COALESCE(a.runtime_id,(SELECT runtime_id FROM worker_singleton_owners WHERE workspace_id=a.workspace_id AND singleton_key=a.principal_id AND a.principal_kind='workspace_agent')),'worker_id',COALESCE(a.worker_id,(SELECT worker_id FROM worker_singleton_owners WHERE workspace_id=a.workspace_id AND singleton_key=a.principal_id AND a.principal_kind='workspace_agent')),'status',json_extract(a.tracking_json,'$.status'),'case_state',t.workflow_state,'released',json(CASE WHEN EXISTS(SELECT 1 FROM ticket_assignment_work_releases r WHERE r.workspace_id=a.workspace_id AND r.assignment_id=a.assignment_id) THEN 'true' ELSE 'false' END))";

// FACTS_SQL above is retained verbatim for applied schema95/97/98 migrations.
// Current capture resolves the existing Subject singleton owner, never a second registry.
fn subject_facts_sql() -> String {
    let prefix = crate::subjektiv::SUBJECT_WORKER_SINGLETON_PREFIX.replace('\'', "''");
    let owner = |column: &str| {
        format!(
            "CASE WHEN a.principal_kind='subject' THEN (SELECT {column} FROM worker_singleton_owners WHERE workspace_id=a.workspace_id AND singleton_key='{prefix}' || a.principal_id) ELSE a.{column} END"
        )
    };
    format!(
        "json_object('principal_kind',a.principal_kind,'principal_id',a.principal_id,'runtime_id',{},'worker_id',{},'status',json_extract(a.tracking_json,'$.status'),'case_state',t.workflow_state,'released',json(CASE WHEN EXISTS(SELECT 1 FROM ticket_assignment_work_releases r WHERE r.workspace_id=a.workspace_id AND r.assignment_id=a.assignment_id) THEN 'true' ELSE 'false' END))",
        owner("runtime_id"),
        owner("worker_id")
    )
}

/// Additive upgrade of future capture only. Old recipient snapshots and receipts remain
/// evidence; an unresolved legacy principal is not mapped to an arbitrary Subject.
/// The caller owns the Workspace schema transaction and runs this after assignment migration.
pub(super) fn install_subject_recipients(conn: &Connection) -> Result<()> {
    let facts = subject_facts_sql();
    for trigger in [
        "notification_ticket_event",
        "notification_assignment_history",
        "notification_assignment_change",
        "notification_assignment_release",
        "notification_content_input",
        "notification_unassigned_content",
        "notification_mr_event",
        "notification_mr_source",
        "notification_implementation_result",
    ] {
        let sql: String = conn.query_row(
            "SELECT sql FROM sqlite_schema WHERE type='trigger' AND name=?1",
            [trigger],
            |row| row.get(0),
        )?;
        if !sql.contains(FACTS_SQL) {
            return Err(Error::Store(format!(
                "Notification capture {trigger} does not match the prior schema"
            )));
        }
        conn.execute_batch(&format!(
            "DROP TRIGGER {trigger}; {}",
            sql.replace(FACTS_SQL, &facts)
        ))?;
    }
    Ok(())
}

fn capture(input: &str, kind: &str, actor: &str, event_index: &str) -> String {
    format!("INSERT OR IGNORE INTO notification_sources(workspace_id,ticket_id,input_event_id,role,assignment_id,kind,actor,event_index,facts_json)
 SELECT NEW.workspace_id,NEW.ticket_id,{input},roles.role,COALESCE(a.assignment_id,''),{kind},{actor},{event_index},{FACTS_SQL}
 FROM (SELECT 'orchestrator' role UNION ALL SELECT 'worker') roles
 JOIN typed_tickets t ON t.workspace_id=NEW.workspace_id AND t.ticket_id=NEW.ticket_id
 LEFT JOIN ticket_current_worker_assignments c ON c.workspace_id=t.workspace_id AND c.ticket_id=t.ticket_id AND c.role=roles.role
 LEFT JOIN ticket_worker_assignments a ON a.workspace_id=c.workspace_id AND a.assignment_id=c.assignment_id;")
}

pub(super) fn install(conn: &Connection) -> Result<()> {
    conn.execute_batch(include_str!("notification_routing.sql"))?;
    // Import only already recognized request identities and receipts. Never scan old comments.
    conn.execute_batch(&format!("INSERT OR IGNORE INTO notification_sources(workspace_id,ticket_id,input_event_id,role,assignment_id,kind,event_index,facts_json)
 SELECT a.workspace_id,a.ticket_id,o.input_event_id,a.role,a.assignment_id,'assignment',NULL,{FACTS_SQL}
 FROM ticket_assignment_delivery_origins o JOIN ticket_worker_assignments a USING(workspace_id,assignment_id) JOIN typed_tickets t ON t.workspace_id=a.workspace_id AND t.ticket_id=a.ticket_id;
 INSERT OR IGNORE INTO notification_sources(workspace_id,ticket_id,input_event_id,role,assignment_id,kind,event_index,facts_json)
 SELECT a.workspace_id,a.ticket_id,'ticket-edit:' || printf('%020d',e.input_sequence),a.role,a.assignment_id,'content_edit',NULL,{FACTS_SQL}
 FROM ticket_assignment_content_inputs e JOIN ticket_worker_assignments a USING(workspace_id,assignment_id) JOIN typed_tickets t ON t.workspace_id=a.workspace_id AND t.ticket_id=a.ticket_id;
 INSERT OR IGNORE INTO notification_sources(workspace_id,ticket_id,input_event_id,role,assignment_id,kind,event_index,facts_json)
 SELECT a.workspace_id,a.ticket_id,'ticket-event:' || e.event_index,a.role,a.assignment_id,e.kind,e.event_index,{FACTS_SQL}
 FROM ticket_assignment_delivery_origins o JOIN ticket_worker_assignments a USING(workspace_id,assignment_id) JOIN typed_tickets t ON t.workspace_id=a.workspace_id AND t.ticket_id=a.ticket_id
 JOIN typed_ticket_events e ON e.workspace_id=a.workspace_id AND e.ticket_id=a.ticket_id AND e.event_index>o.after_ticket_event_index
 WHERE e.kind IN ('assignment_answer','item_edit') AND
 (EXISTS(SELECT 1 FROM typed_ticket_event_attributes x WHERE x.workspace_id=e.workspace_id AND x.ticket_id=e.ticket_id AND x.event_index=e.event_index AND x.key='orchestrator_assignment_id' AND x.value=a.assignment_id)
 OR (e.kind='item_edit' AND NOT EXISTS(SELECT 1 FROM ticket_assignment_delivery_origins later JOIN ticket_worker_assignments la ON la.workspace_id=later.workspace_id AND la.assignment_id=later.assignment_id WHERE later.workspace_id=a.workspace_id AND later.ticket_id=a.ticket_id AND la.rowid>a.rowid AND e.event_index>later.after_ticket_event_index)));
 INSERT OR IGNORE INTO notification_sources(workspace_id,ticket_id,input_event_id,role,assignment_id,kind,event_index,facts_json)
 SELECT a.workspace_id,a.ticket_id,d.input_event_id,a.role,a.assignment_id,'content_edit',NULL,{FACTS_SQL}
 FROM ticket_assignment_deliveries d JOIN ticket_worker_assignments a USING(workspace_id,assignment_id) JOIN typed_tickets t ON t.workspace_id=a.workspace_id AND t.ticket_id=a.ticket_id;"))?;
    let event = capture(
        "'ticket-event:' || NEW.event_index",
        "CASE WHEN NEW.state_field='case' THEN 'case' WHEN NEW.state_field='tags' THEN 'tags' ELSE NEW.kind END",
        "NEW.author",
        "NEW.event_index",
    );
    let history = capture(
        "'assignment-event:' || NEW.event_id",
        "CASE WHEN json_extract(NEW.source_context_json,'$.kind')='assignment' THEN 'assignment' ELSE 'tracking' END",
        "json_extract(NEW.source_context_json,'$.actor')",
        "NULL",
    );
    let change = capture(
        "'assignment-change:' || NEW.event_id",
        "'assignment_changed'",
        "NEW.actor",
        "NULL",
    );
    let release = capture(
        "'assignment-release:' || NEW.assignment_id",
        "'release'",
        "NULL",
        "NULL",
    );
    conn.execute_batch(&format!("CREATE TRIGGER notification_ticket_event AFTER INSERT ON typed_ticket_events WHEN NEW.event_index>=0 BEGIN {event} END;
 CREATE TRIGGER notification_assignment_history AFTER INSERT ON typed_ticket_assignment_tracking_history BEGIN {history} END;
 CREATE TRIGGER notification_assignment_change AFTER INSERT ON ticket_worker_assignment_events BEGIN {change}
 INSERT OR IGNORE INTO notification_sources(workspace_id,ticket_id,input_event_id,role,assignment_id,kind,actor,event_index,facts_json)
 SELECT a.workspace_id,a.ticket_id,'assignment-change:' || NEW.event_id,a.role,a.assignment_id,'release',NEW.actor,NULL,json_set({FACTS_SQL},'$.released',json('true'))
 FROM ticket_worker_assignments a JOIN typed_tickets t ON t.workspace_id=a.workspace_id AND t.ticket_id=a.ticket_id
 WHERE a.workspace_id=NEW.workspace_id AND a.assignment_id=NEW.previous_assignment_id AND NEW.previous_assignment_id IS NOT NEW.assignment_id;
 END;"))?;
    // Release rows have no Ticket column; use the assignment's immutable parent.
    let release = release.replace("NEW.ticket_id", "(SELECT ticket_id FROM ticket_worker_assignments WHERE workspace_id=NEW.workspace_id AND assignment_id=NEW.assignment_id)");
    conn.execute_batch(&format!("CREATE TRIGGER notification_assignment_release AFTER INSERT ON ticket_assignment_work_releases BEGIN {release} END;
 CREATE TRIGGER notification_content_input AFTER INSERT ON ticket_assignment_content_inputs BEGIN
 INSERT INTO notification_sources(workspace_id,ticket_id,input_event_id,role,assignment_id,kind,actor,event_index,facts_json)
 SELECT NEW.workspace_id,NEW.ticket_id,'ticket-edit:' || printf('%020d',NEW.input_sequence),a.role,a.assignment_id,NEW.kind,NEW.actor,NULL,json_set({FACTS_SQL},'$.content_change_id',NEW.source_change_id)
 FROM ticket_worker_assignments a JOIN typed_tickets t ON t.workspace_id=a.workspace_id AND t.ticket_id=a.ticket_id
 WHERE a.workspace_id=NEW.workspace_id AND a.assignment_id=NEW.assignment_id;
 END;"))?;
    let missing = capture(
        "'ticket-content:' || NEW.change_id",
        "NEW.kind",
        "NEW.actor",
        "NULL",
    )
    .trim_end_matches(';')
    .to_owned()
        + " WHERE a.assignment_id IS NULL;";
    conn.execute_batch(&format!("CREATE TRIGGER notification_unassigned_content AFTER INSERT ON typed_ticket_content_changes BEGIN {missing} END;"))?;
    for table in [
        "notification_sources",
        "notification_assignment_targets",
        "notification_requests",
        "notification_acknowledgements",
        "notification_failures",
    ] {
        for action in ["update", "delete"] {
            conn.execute_batch(&format!("CREATE TRIGGER {table}_no_{action} BEFORE {action} ON {table} BEGIN SELECT RAISE(ABORT,'Notification evidence is immutable'); END;"))?;
        }
    }
    conn.execute_batch("CREATE TRIGGER notification_decisions_no_delete BEFORE DELETE ON notification_decisions BEGIN SELECT RAISE(ABORT,'Notification decision is retained'); END;
 CREATE TRIGGER notification_decisions_identity_no_update BEFORE UPDATE OF source_sequence,workspace_id,delivery_id,disposition,reason,purpose ON notification_decisions BEGIN SELECT RAISE(ABORT,'Notification decision is immutable'); END;")?;
    Ok(())
}

/// Capture business facts in the same SQLite transaction as their owners. Historical
/// MR events are bounded by their original assignment interval, never today's delegation.
/// Comments are deliberately excluded: neither legacy handoff prose nor review comments
/// prove a typed result. MR input is an instruction to reread evidence, not approval.
pub(super) fn install_business_events(conn: &Connection) -> Result<()> {
    conn.execute_batch(include_str!("implementation_result.sql"))?;
    // Only identities retained by the typed event are actors. Cancellation and
    // provider observations have no actor; never substitute the requester/reviewer.
    let actor_path = "CASE NEW.kind WHEN 'review_requested' THEN '$.requested_by' WHEN 'review' THEN '$.reviewer' WHEN 'review_revoked' THEN '$.revoked_by' WHEN 'merge' THEN '$.merged_by' END";
    let mr_actor = format!(
        "'worker:' || json_extract(NEW.payload_json,({actor_path}) || '.runtime_id') || '/' || json_extract(NEW.payload_json,({actor_path}) || '.worker_id')"
    );
    let mr = format!("INSERT OR IGNORE INTO notification_sources(workspace_id,ticket_id,input_event_id,role,assignment_id,kind,actor,event_index,facts_json)
 SELECT NEW.workspace_id,r.ticket_id,'mr-event:' || NEW.merge_request_id || ':' || NEW.event_id,roles.role,COALESCE(a.assignment_id,''),'mr_' || NEW.kind,{mr_actor},NULL,
 json_set({FACTS_SQL},'$.merge_request_id',NEW.merge_request_id,'$.event_id',NEW.event_id,'$.subject_ref',COALESCE(json_extract(NEW.payload_json,'$.subject_ref'),json_extract(NEW.payload_json,'$.approved_source_ref')),'$.request_event_id',json_extract(NEW.payload_json,'$.request_event_id'))
 FROM merge_request_ticket_relations r JOIN typed_tickets t ON t.workspace_id=r.workspace_id AND t.ticket_id=r.ticket_id
 CROSS JOIN (SELECT 'orchestrator' role UNION ALL SELECT 'worker') roles
 LEFT JOIN ticket_current_worker_assignments c ON c.workspace_id=t.workspace_id AND c.ticket_id=t.ticket_id AND c.role=roles.role
 LEFT JOIN ticket_worker_assignments a ON a.workspace_id=c.workspace_id AND a.assignment_id=c.assignment_id
 WHERE r.workspace_id=NEW.workspace_id AND r.merge_request_id=NEW.merge_request_id;");
    conn.execute_batch(&format!("CREATE TRIGGER notification_mr_event AFTER INSERT ON merge_request_thread_events WHEN NEW.kind!='comment' BEGIN {mr} END;"))?;
    let source = mr.replace(&mr_actor, "NULL")
        .replace("'mr-event:' || NEW.merge_request_id || ':' || NEW.event_id", "'mr-source:' || NEW.merge_request_id || ':' || NEW.sequence")
        .replace("'mr_' || NEW.kind", "'mr_source_changed'")
        .replace("NEW.event_id", "NEW.sequence")
        .replace("COALESCE(json_extract(NEW.payload_json,'$.subject_ref'),json_extract(NEW.payload_json,'$.approved_source_ref'))", "NEW.subject_ref")
        .replace("json_extract(NEW.payload_json,'$.request_event_id')", "NULL");
    conn.execute_batch(&format!("CREATE TRIGGER notification_mr_source AFTER INSERT ON merge_request_source_observations WHEN NEW.previous_subject_ref IS NOT NULL BEGIN {source} END;"))?;
    // Recover retained review/merge evidence once. A source identity is the immutable
    // MR event, so re-review is distinct and retry cannot duplicate an input.
    // Equal/ambiguous assignment timestamps cannot prove event-time ownership;
    // retain a no-recipient source instead of guessing by row order.
    let historical = mr.replace("NEW.", "e.").replace(
        "FROM merge_request_ticket_relations r", "FROM merge_request_thread_events e JOIN merge_request_ticket_relations r ON r.workspace_id=e.workspace_id AND r.merge_request_id=e.merge_request_id"
    ).replace(
        "LEFT JOIN ticket_current_worker_assignments c ON c.workspace_id=t.workspace_id AND c.ticket_id=t.ticket_id AND c.role=roles.role\n LEFT JOIN ticket_worker_assignments a ON a.workspace_id=c.workspace_id AND a.assignment_id=c.assignment_id",
        "LEFT JOIN ticket_worker_assignments a ON a.workspace_id=t.workspace_id AND a.ticket_id=t.ticket_id AND a.role=roles.role AND a.assignment_id=(SELECT CASE WHEN COUNT(*)=1 THEN MIN(h.assignment_id) END FROM ticket_worker_assignments h WHERE h.workspace_id=t.workspace_id AND h.ticket_id=t.ticket_id AND h.role=roles.role AND h.assigned_at=(SELECT MAX(prior.assigned_at) FROM ticket_worker_assignments prior WHERE prior.workspace_id=t.workspace_id AND prior.ticket_id=t.ticket_id AND prior.role=roles.role AND prior.assigned_at<e.created_at))"
    ).replace("WHERE r.workspace_id=e.workspace_id AND r.merge_request_id=e.merge_request_id;", "WHERE e.kind!='comment';");
    conn.execute_batch(&historical)?;
    conn.execute_batch(&format!("CREATE TRIGGER notification_implementation_result AFTER INSERT ON ticket_implementation_results BEGIN
 INSERT INTO notification_sources(workspace_id,ticket_id,input_event_id,role,assignment_id,kind,actor,event_index,facts_json)
 SELECT NEW.workspace_id,NEW.ticket_id,'implementation-result:' || NEW.result_id,'orchestrator',a.assignment_id,'implementation_result','worker:' || NEW.runtime_id || '/' || NEW.worker_id,NULL,
 json_set({FACTS_SQL},'$.result_id',NEW.result_id,'$.implementation_assignment_id',NEW.assignment_id)
 FROM ticket_worker_assignments a JOIN typed_tickets t ON t.workspace_id=a.workspace_id AND t.ticket_id=a.ticket_id
 WHERE a.workspace_id=NEW.workspace_id AND a.assignment_id=NEW.orchestrator_assignment_id;
 END;"))?;
    Ok(())
}

/// New reports notify the delegation present when the report is saved. Assignment
/// identity only fences this saved notification against later retargeting; it is
/// neither a caller input nor permission to publish a report.
pub(super) fn upgrade_result_reports(conn: &Connection) -> Result<()> {
    conn.execute_batch(include_str!("implementation_result_reports.sql"))?;
    conn.execute_batch(&format!("CREATE TRIGGER notification_implementation_result AFTER INSERT ON ticket_implementation_results BEGIN
 INSERT INTO notification_sources(workspace_id,ticket_id,input_event_id,role,assignment_id,kind,actor,event_index,facts_json)
 SELECT NEW.workspace_id,NEW.ticket_id,'implementation-result:' || NEW.result_id,'orchestrator',COALESCE(a.assignment_id,''),'implementation_result','worker:' || NEW.runtime_id || '/' || NEW.worker_id,NULL,
 json_set({FACTS_SQL},'$.result_id',NEW.result_id,'$.merge_request_id',json_extract(NEW.request_json,'$.merge_request.merge_request_id'))
 FROM typed_tickets t
 LEFT JOIN ticket_current_worker_assignments c ON c.workspace_id=t.workspace_id AND c.ticket_id=t.ticket_id AND c.role='orchestrator'
 LEFT JOIN ticket_worker_assignments a ON a.workspace_id=c.workspace_id AND a.assignment_id=c.assignment_id
 WHERE t.workspace_id=NEW.workspace_id AND t.ticket_id=NEW.ticket_id;
 END;"))?;
    Ok(())
}

impl SqliteWorkspaceStore {
    /// Record a provider-resolved source observation, not approval. Git has no local
    /// transactional event log: transitions are captured when the existing MR paths
    /// observe them. Identical observations (including concurrent scans) are deduplicated.
    pub(crate) fn observe_merge_request_source(
        &self,
        workspace: &str,
        repository: &str,
        selector: &str,
        subject: &str,
    ) -> Result<()> {
        self.with_conn_mut(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let ids = tx.prepare("SELECT merge_request_id FROM merge_requests WHERE workspace_id=?1 AND repository_id=?2 AND selector_from=?3 AND state='open'")?.query_map(params![workspace,repository,selector],|r|r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
            for id in ids {
                let previous: Option<String> = tx.query_row("SELECT subject_ref FROM merge_request_source_observations WHERE workspace_id=?1 AND merge_request_id=?2 ORDER BY sequence DESC LIMIT 1", params![workspace,id], |r|r.get(0)).optional()?;
                // New MRs may already have a review before the first observation.
                let previous = match previous {
                    Some(previous) => Some(previous),
                    None => tx.query_row("SELECT json_extract(payload_json,'$.subject_ref') FROM merge_request_thread_events WHERE workspace_id=?1 AND merge_request_id=?2 AND json_extract(payload_json,'$.subject_ref') IS NOT NULL ORDER BY sequence DESC LIMIT 1",params![workspace,id],|r|r.get(0)).optional()?,
                };
                if previous.as_deref() != Some(subject) {
                    tx.execute("INSERT INTO merge_request_source_observations(workspace_id,merge_request_id,subject_ref,previous_subject_ref,observed_at) VALUES(?1,?2,?3,?4,?5)",params![workspace,id,subject,previous,chrono::Utc::now().to_rfc3339()])?;
                }
            }
            tx.commit()?;
            Ok(())
        })
    }
}

pub(super) fn verify(conn: &Connection) -> Result<()> {
    let mut names = vec![
        "notification_sources".to_owned(),
        "notification_assignment_targets".into(),
        "notification_decisions".into(),
        "notification_requests".into(),
        "notification_acknowledgements".into(),
        "notification_failures".into(),
    ];
    for table in [
        "notification_sources",
        "notification_assignment_targets",
        "notification_requests",
        "notification_acknowledgements",
        "notification_failures",
    ] {
        for action in ["update", "delete"] {
            names.push(format!("{table}_no_{action}"));
        }
    }
    for name in [
        "notification_decisions_no_delete",
        "notification_decisions_identity_no_update",
        "notification_ticket_event",
        "notification_assignment_history",
        "notification_assignment_change",
        "notification_assignment_release",
        "notification_content_input",
        "notification_unassigned_content",
    ] {
        names.push(name.into());
    }
    if current_schema_version(conn)? == 97 {
        names.push("implementation_delegation_capture".into());
    }
    if current_schema_version(conn)? >= 97 {
        names.extend(
            [
                "ticket_implementation_delegations",
                "ticket_implementation_results",
                "implementation_results_no_update",
                "implementation_results_no_delete",
                "implementation_delegations_no_update",
                "implementation_delegations_no_delete",
                "notification_mr_event",
                "notification_implementation_result",
                "notification_mr_source",
                "merge_request_source_observations",
                "mr_source_observations_no_update",
                "mr_source_observations_no_delete",
            ]
            .into_iter()
            .map(str::to_owned),
        );
    }
    for name in names {
        let present: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name=?1)",
            [&name],
            |r| r.get(0),
        )?;
        if !present {
            return Err(Error::Store(format!(
                "Notification routing constraint {name} is missing"
            )));
        }
    }
    Ok(())
}

fn read_source(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<(
    i64,
    String,
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    Option<i64>,
    String,
)> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get::<_, Option<String>>(2)?.unwrap_or_default(),
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
        row.get(9)?,
    ))
}
fn source_from_row(row: &rusqlite::Row<'_>) -> Result<Source> {
    let (sequence, workspace, ticket, input, role, assignment, kind, actor, event_index, facts) =
        read_source(row)?;
    Ok(Source {
        sequence,
        workspace,
        ticket,
        input,
        role,
        assignment,
        kind,
        actor,
        event_index,
        facts: decode(&facts)?,
    })
}
fn stable_id(source: &Source) -> Result<String> {
    let identities = if source.role == "orchestrator" && !source.assignment.is_empty() {
        json(&(&source.workspace, &source.assignment, &source.input))?
    } else {
        json(&(
            &source.workspace,
            &source.ticket,
            &source.role,
            &source.assignment,
            &source.input,
        ))?
    };
    let digest = Sha256::digest(identities.as_bytes());
    let hex = digest
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    Ok(format!(
        "{}:{hex}",
        if source.role == "orchestrator" {
            "assignment-delivery"
        } else {
            "worker-notification"
        }
    ))
}
fn self_notification(conn: &Connection, source: &Source) -> Result<bool> {
    // Actor strings are authenticated audit attribution, not authorization grants.
    if let (Some(runtime), Some(worker), Some(actor)) = (
        &source.facts.runtime_id,
        &source.facts.worker_id,
        &source.actor,
    ) {
        if actor == &format!("worker:{runtime}/{worker}")
            || actor == &format!("authenticated-worker:{runtime}/{worker}")
        {
            return Ok(true);
        }
    }
    if let Some(index) = source.event_index {
        let pair: (Option<String>,Option<String>)=conn.query_row("SELECT MAX(CASE WHEN key='source_runtime_id' THEN value END),MAX(CASE WHEN key='source_worker_id' THEN value END) FROM typed_ticket_event_attributes WHERE workspace_id=?1 AND ticket_id=?2 AND event_index=?3",params![source.workspace,source.ticket,index],|r|Ok((r.get(0)?,r.get(1)?)))?;
        if pair.0.is_some() && pair.0 == source.facts.runtime_id && pair.1 == source.facts.worker_id
        {
            return Ok(true);
        }
    }
    Ok(false)
}
fn decision_for_source(
    conn: &Connection,
    source: &Source,
) -> Result<(&'static str, NotificationReason, NotificationPurpose)> {
    let workspace = &source.workspace;
    let kind = NotificationEventKind::from_source(&source.kind);
    let (mut disposition, mut reason) = policy(
        kind,
        &source.role,
        &source.assignment,
        &source.facts,
        self_notification(conn, source)?,
    );
    if kind == NotificationEventKind::Answer && source.role == "orchestrator" {
        let fence:Option<String>=conn.query_row("SELECT value FROM typed_ticket_event_attributes WHERE workspace_id=?1 AND ticket_id=?2 AND event_index=?3 AND key='orchestrator_assignment_id'",params![workspace,source.ticket,source.event_index],|r|r.get(0)).optional()?;
        if fence.as_deref() != Some(&source.assignment) {
            disposition = "suppress";
            reason = NotificationReason::AssignmentChanged;
        }
    }
    Ok((disposition, reason, purpose(kind, &source.role)))
}

pub(super) fn route_pending(conn: &Connection, workspace: &str) -> Result<()> {
    let mut stmt = conn.prepare("SELECT s.* FROM notification_sources s LEFT JOIN notification_decisions d ON d.source_sequence=s.sequence WHERE s.workspace_id=?1 AND s.role!='backend_job' AND d.source_sequence IS NULL ORDER BY s.sequence LIMIT 500")?;
    let mut rows = stmt.query([workspace])?;
    while let Some(row) = rows.next()? {
        let source = source_from_row(row)?;
        let (disposition, reason, purpose) = decision_for_source(conn, &source)?;
        let saved_id: Option<String> = if source.role == "orchestrator" {
            conn.query_row("SELECT delivery_id FROM ticket_assignment_deliveries WHERE workspace_id=?1 AND assignment_id=?2 AND input_event_id=?3", params![workspace,source.assignment,source.input], |r|r.get(0)).optional()?
        } else {
            None
        };
        let id = match saved_id {
            Some(id) => id,
            None => stable_id(&source)?,
        };
        conn.execute("INSERT OR IGNORE INTO notification_decisions(source_sequence,workspace_id,delivery_id,disposition,reason,purpose) VALUES(?1,?2,?3,?4,?5,?6)",params![source.sequence,workspace,id,disposition,code(reason)?,code(purpose)?])?;
    }
    Ok(())
}

// Preview only this assignment's sources in sequence order. Persisted decisions and
// receipts win; unrouted sources use exactly the delivery policy, without saving it.
// Streaming avoids a global routing batch cap and bounds retained rows to limit + 1.
pub(super) fn diagnostic_inputs(
    conn: &Connection,
    workspace: &str,
    ticket: &str,
    assignment: &str,
    after: Option<&str>,
    limit: usize,
) -> Result<Vec<server_api::TicketAssignmentInputDiagnostic>> {
    let mut stmt = conn.prepare("SELECT s.*,d.disposition,r.delivery_id,EXISTS(SELECT 1 FROM ticket_assignment_delivery_requests p WHERE p.workspace_id=s.workspace_id AND p.delivery_id=r.delivery_id),EXISTS(SELECT 1 FROM ticket_assignment_delivery_acknowledgements a WHERE a.workspace_id=s.workspace_id AND a.delivery_id=r.delivery_id) FROM notification_sources s LEFT JOIN notification_decisions d ON d.source_sequence=s.sequence LEFT JOIN ticket_assignment_deliveries r ON r.workspace_id=s.workspace_id AND r.assignment_id=s.assignment_id AND r.input_event_id=s.input_event_id WHERE s.workspace_id=?1 AND s.ticket_id=?2 AND s.assignment_id=?3 AND s.role='orchestrator' ORDER BY s.sequence")?;
    let mut rows = stmt.query(params![workspace, ticket, assignment])?;
    let mut cursor_seen = after.is_none();
    let mut inputs = Vec::new();
    while let Some(row) = rows.next()? {
        let source = source_from_row(row)?;
        let disposition: Option<String> = row.get(10)?;
        let delivery_id: Option<String> = row.get(11)?;
        let eligible = match disposition {
            Some(disposition) => disposition == "deliver",
            None => decision_for_source(conn, &source)?.0 == "deliver",
        };
        if !eligible && delivery_id.is_none() {
            continue;
        }
        if !cursor_seen {
            cursor_seen = after == Some(source.input.as_str());
            continue;
        }
        let prepared: bool = row.get(12)?;
        let ack: bool = row.get(13)?;
        use server_api::TicketAssignmentInputDeliveryState::*;
        let notification = if ack {
            RuntimeAccepted
        } else if prepared {
            RequestPersisted
        } else if delivery_id.is_some() {
            Reserved
        } else {
            NotReserved
        };
        inputs.push(server_api::TicketAssignmentInputDiagnostic {
            input_event_id: source.input,
            delivery_id,
            notification,
            acceptance_outcome_unknown: prepared && !ack,
        });
        if inputs.len() > limit {
            break;
        }
    }
    if !cursor_seen {
        return Err(Error::InvalidInput(
            "Input cursor does not belong to this assignment".into(),
        ));
    }
    Ok(inputs)
}

pub(super) fn require_current(
    conn: &Connection,
    workspace: &str,
    ticket: &str,
    assignment: &str,
    role: &str,
) -> Result<()> {
    let facts_sql = subject_facts_sql();
    let facts:Option<String>=conn.query_row(&format!("SELECT {facts_sql} FROM ticket_current_worker_assignments c JOIN ticket_worker_assignments a ON a.workspace_id=c.workspace_id AND a.assignment_id=c.assignment_id JOIN typed_tickets t ON t.workspace_id=a.workspace_id AND t.ticket_id=a.ticket_id WHERE c.workspace_id=?1 AND c.ticket_id=?2 AND c.assignment_id=?3 AND c.role=?4"),params![workspace,ticket,assignment,role],|r|r.get(0)).optional()?;
    if let Some(facts) = facts {
        let facts: Facts = decode(&facts)?;
        if policy(
            NotificationEventKind::ContentEdit,
            role,
            assignment,
            &facts,
            false,
        )
        .0 == "deliver"
        {
            return Ok(());
        }
    }
    Err(Error::TicketAssignmentConflict(
        "Notification assignment is no longer current actionable responsibility".into(),
    ))
}
pub(super) fn pending_sources(
    conn: &Connection,
    workspace: &str,
    role: &str,
    limit: usize,
) -> Result<Vec<(String, String, String, String)>> {
    route_pending(conn, workspace)?;
    let mut stmt=conn.prepare("SELECT s.ticket_id,s.assignment_id,s.input_event_id,d.delivery_id FROM notification_sources s JOIN notification_decisions d ON d.source_sequence=s.sequence WHERE s.workspace_id=?1 AND s.role=?2 AND d.disposition='deliver' AND NOT EXISTS(SELECT 1 FROM ticket_assignment_delivery_acknowledgements a WHERE a.workspace_id=?1 AND a.delivery_id=d.delivery_id) AND NOT EXISTS(SELECT 1 FROM notification_acknowledgements a WHERE a.workspace_id=?1 AND a.delivery_id=d.delivery_id) ORDER BY d.last_offered,s.sequence LIMIT ?3")?;
    let candidates = stmt
        .query_map(params![workspace, role, limit.clamp(1, 100) as i64], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let tick: i64 = conn.query_row(
        "SELECT COALESCE(MAX(last_offered),0)+1 FROM notification_decisions WHERE workspace_id=?1",
        [workspace],
        |r| r.get(0),
    )?;
    let mut result = Vec::new();
    for row in candidates {
        conn.execute("UPDATE notification_decisions SET last_offered=?3 WHERE workspace_id=?1 AND delivery_id=?2",params![workspace,row.3,tick])?;
        match require_current(conn, workspace, &row.0, &row.1, role) {
            Ok(()) => result.push(row),
            Err(Error::TicketAssignmentConflict(_)) => {
                save_failure(
                    conn,
                    workspace,
                    &row.3,
                    NotificationFailureCode::AssignmentChanged,
                )?;
            }
            Err(error) => return Err(error),
        }
    }
    Ok(result)
}
fn save_failure(
    conn: &Connection,
    workspace: &str,
    id: &str,
    failure: NotificationFailureCode,
) -> Result<()> {
    let value = code(failure)?;
    conn.execute("INSERT INTO notification_failures(workspace_id,delivery_id,code) SELECT ?1,?2,?3 WHERE NOT EXISTS(SELECT 1 FROM notification_failures WHERE sequence=(SELECT MAX(sequence) FROM notification_failures WHERE workspace_id=?1 AND delivery_id=?2) AND code=?3)",params![workspace,id,value])?;
    Ok(())
}
pub(super) fn failure(
    store: &SqliteWorkspaceStore,
    workspace: &str,
    key: &NotificationDeliveryKey,
    failure: NotificationFailureCode,
) -> Result<()> {
    store.with_conn_mut(|conn| {let tx=conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        route_pending(&tx,workspace)?;
        let id:String=match key {
            NotificationDeliveryKey::WorkerNotification{delivery_id}=>tx.query_row("SELECT delivery_id FROM notification_decisions WHERE workspace_id=?1 AND delivery_id=?2",params![workspace,delivery_id],|r|r.get(0))?,
            NotificationDeliveryKey::AssignmentInput{assignment_id,input_event_id}=>tx.query_row("SELECT d.delivery_id FROM notification_decisions d JOIN notification_sources s ON s.sequence=d.source_sequence WHERE s.workspace_id=?1 AND s.assignment_id=?2 AND s.input_event_id=?3 AND s.role='orchestrator'",params![workspace,assignment_id,input_event_id],|r|r.get(0))?,
        };
        save_failure(&tx,workspace,&id,failure)?;tx.commit()?;Ok(())
    })
}
pub(super) fn list_workers(
    store: &SqliteWorkspaceStore,
    workspace: &str,
    limit: usize,
) -> Result<Vec<TicketWorkerNotification>> {
    if limit == 0 || limit > 100 {
        return Err(Error::InvalidInput(
            "Notification limit must be 1..100".into(),
        ));
    }
    store.with_conn_mut(|conn| {let tx=conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut result=Vec::new();
        for (ticket_id,assignment_id,input_event_id,delivery_id) in pending_sources(&tx,workspace,"worker",limit)? {
            let (runtime,worker):(String,String)=tx.query_row("SELECT runtime_id,worker_id FROM ticket_worker_assignments WHERE workspace_id=?1 AND assignment_id=?2",params![workspace,assignment_id],|r|Ok((r.get(0)?,r.get(1)?)))?;
            result.push(TicketWorkerNotification{ticket_id,assignment_id,input_event_id,request_id:delivery_id.clone(),delivery_id,worker:RuntimeWorkerRef::new(runtime,worker),purpose:NotificationPurpose::Attention});
        }
        tx.commit()?;Ok(result)
    })
}
fn worker_guard(
    conn: &Connection,
    workspace: &str,
    id: &str,
    target: &TicketWorkerNotificationTarget,
) -> Result<()> {
    let (ticket,assignment,runtime,worker):(String,String,String,String)=conn.query_row("SELECT s.ticket_id,s.assignment_id,json_extract(s.facts_json,'$.runtime_id'),json_extract(s.facts_json,'$.worker_id') FROM notification_sources s JOIN notification_decisions d ON d.source_sequence=s.sequence WHERE d.workspace_id=?1 AND d.delivery_id=?2 AND s.role='worker' AND d.disposition='deliver'",params![workspace,id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?;
    require_current(conn, workspace, &ticket, &assignment, "worker")?;
    if target.worker != RuntimeWorkerRef::new(runtime, worker) {
        return Err(Error::WorkspacePermissionDenied(
            "Notification target differs from captured assignment".into(),
        ));
    }
    let valid:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM worker_registry w JOIN workspace_runtime_bindings b ON b.workspace_id=w.workspace_id AND b.runtime_id=w.runtime_id WHERE w.workspace_id=?1 AND w.runtime_id=?2 AND w.worker_id=?3 AND b.binding_id=?4 AND b.revoked_at IS NULL AND NOT EXISTS(SELECT 1 FROM worker_removal_operations r WHERE r.workspace_id=w.workspace_id AND r.runtime_id=w.runtime_id AND r.worker_id=w.worker_id AND r.state IN ('executing','failed','succeeded')))",params![workspace,target.worker.runtime_id,target.worker.worker_id,target.authentication_binding_id],|r|r.get(0))?;
    if !valid {
        return Err(Error::WorkspacePermissionDenied(
            "Notification target binding is unavailable or revoked".into(),
        ));
    }
    Ok(())
}
fn worker_acknowledged(conn: &Connection, workspace: &str, id: &str) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM notification_acknowledgements WHERE workspace_id=?1 AND delivery_id=?2)",
        params![workspace, id], |row| row.get(0),
    )?)
}

pub(super) fn prepare_worker(
    store: &SqliteWorkspaceStore,
    workspace: &str,
    id: &str,
    content: &str,
    target: &TicketWorkerNotificationTarget,
) -> Result<String> {
    if content.is_empty() || content.len() > 262144 || content.contains('\0') {
        return Err(Error::InvalidInput(
            "Notification content must be nonempty, NUL-free and at most 262144 bytes".into(),
        ));
    }
    store.with_conn_mut(|conn| {let tx=conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        worker_guard(&tx,workspace,id,target)?;
        if worker_acknowledged(&tx,workspace,id)? { return Err(Error::TicketAssignmentConflict("Worker notification already has Runtime acceptance; new send admission is closed".into())); }
        let saved:Option<(String,String)>=tx.query_row("SELECT content,target_json FROM notification_requests WHERE workspace_id=?1 AND delivery_id=?2",params![workspace,id],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
        let result=if let Some((saved,binding))=saved {
            if binding!=json(target)? { return Err(Error::TicketAssignmentConflict("Prepared notification target binding changed".into())); }
            saved
        } else {tx.execute("INSERT INTO notification_requests VALUES(?1,?2,?3,?4)",params![workspace,id,content,json(target)?])?;content.to_owned()};
        tx.commit()?;Ok(result)
    })
}
/// Authorize a new Runtime send, not a historical receipt read. The Host must
/// additionally check live recipient policy before supply and immediately before
/// I/O. An ACK closes admission without erasing the pinned request or receipt.
pub(super) fn admit_worker_send(
    store: &SqliteWorkspaceStore,
    workspace: &str,
    id: &str,
    target: &TicketWorkerNotificationTarget,
) -> Result<Option<String>> {
    store.with_conn_mut(|conn| {
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        worker_guard(&tx, workspace, id, target)?;
        let (content, saved_target): (String, String) = tx.query_row(
            "SELECT content,target_json FROM notification_requests WHERE workspace_id=?1 AND delivery_id=?2",
            params![workspace, id], |row| Ok((row.get(0)?, row.get(1)?)),
        ).optional()?.ok_or_else(|| Error::TicketAssignmentConflict("Worker notification send requires a persisted request".into()))?;
        if saved_target != json(target)? {
            return Err(Error::TicketAssignmentConflict("Worker notification send target differs from prepared request".into()));
        }
        let content = if worker_acknowledged(&tx, workspace, id)? { None } else { Some(content) };
        tx.commit()?;
        Ok(content)
    })
}

pub(super) fn ack_worker(
    store: &SqliteWorkspaceStore,
    workspace: &str,
    id: &str,
    target: &TicketWorkerNotificationTarget,
) -> Result<()> {
    store.with_conn_mut(|conn| {let tx=conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        // Acceptance is evidence about the prepared target, even if responsibility changed during I/O.
        let saved:String=tx.query_row("SELECT target_json FROM notification_requests WHERE workspace_id=?1 AND delivery_id=?2",params![workspace,id],|r|r.get(0))?;
        if saved!=json(target)? {return Err(Error::TicketAssignmentConflict("Acknowledgement target differs from prepared notification".into()));}
        tx.execute("INSERT OR IGNORE INTO notification_acknowledgements VALUES(?1,?2,?3)",params![workspace,id,saved])?;
        tx.commit()?;Ok(())
    })
}

#[cfg(test)]
#[path = "notification_registration_tests.rs"]
mod registration_tests;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn notification_routing_policy_distinguishes_requests_attention_results_and_suppression() {
        let mut facts = Facts {
            principal_kind: Some("subject".into()),
            principal_id: Some("subject-a".into()),
            runtime_id: None,
            worker_id: None,
            status: Some("waiting".into()),
            case_state: "open".into(),
            released: false,
        };
        use NotificationEventKind as K;
        use NotificationPurpose as P;
        use NotificationReason as R;
        for (kind, disposition, reason, purpose_value) in [
            (K::Assignment, "deliver", R::Requested, P::NewDelegation),
            (K::Answer, "deliver", R::Requested, P::ContinuingRequest),
            (
                K::ContentEdit,
                "deliver",
                R::Requested,
                P::ContinuingRequest,
            ),
            (K::Comment, "suppress", R::NotRequest, P::Attention),
            (K::Case, "suppress", R::NotRequest, P::Attention),
            (K::Tracking, "suppress", R::NotRequest, P::Attention),
            (K::Tags, "suppress", R::NotRequest, P::Attention),
            (K::Release, "suppress", R::NotRequest, P::Attention),
            (
                K::AssignmentChanged,
                "suppress",
                R::NotRequest,
                P::Attention,
            ),
            (K::Result, "deliver", R::Result, P::Result),
            (K::Unknown, "defer", R::UnknownEvent, P::Attention),
        ] {
            assert_eq!(
                policy(kind, "orchestrator", "a", &facts, false),
                (disposition, reason)
            );
            assert_eq!(purpose(kind, "orchestrator"), purpose_value);
        }
        facts.status = Some("done".into());
        assert_eq!(
            policy(K::Result, "orchestrator", "a", &facts, false),
            ("suppress", R::HandlingDone)
        );
        assert_eq!(
            purpose(K::Result, "orchestrator"),
            P::Result,
            "terminal result never changes into a new delegation"
        );
        facts.principal_kind = Some("worker".into());
        facts.principal_id = None;
        for kind in [
            K::Assignment,
            K::Answer,
            K::ContentEdit,
            K::Comment,
            K::Case,
            K::Tags,
            K::AssignmentChanged,
        ] {
            assert_eq!(
                policy(kind, "worker", "a", &facts, false),
                ("deliver", R::Attention)
            );
            assert_eq!(purpose(kind, "worker"), P::Attention);
        }
        assert_eq!(
            policy(K::Comment, "worker", "a", &facts, true),
            ("suppress", R::SelfNotification)
        );
        assert_eq!(
            policy(K::Comment, "worker", "", &facts, false),
            ("suppress", R::NoRecipient)
        );
        facts.case_state = "closed".into();
        assert_eq!(
            policy(K::Comment, "worker", "a", &facts, false),
            ("suppress", R::CaseClosed)
        );
        facts.released = true;
        facts.case_state = "open".into();
        assert_eq!(
            policy(K::Comment, "worker", "a", &facts, false),
            ("suppress", R::HistoricalOnly)
        );
    }
}
