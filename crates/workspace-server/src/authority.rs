use std::{path::PathBuf, sync::Arc};

use chrono::{DateTime, Utc};
use merge_request::{
    MergeRequest, MergeRequestError, MergeRequestState, MergeRequestStore, MergeRequestThreadEvent,
    ReviewDecision,
};
use project_record::{allocate_record_id, unix_epoch_millis_now};
use rusqlite::{params_from_iter, types::Value as SqlValue};

use ticket::{
    SqliteTicketBackend, SqliteTicketListCursor, SqliteTicketListItem, SqliteTicketListPageQuery,
    TicketBackend, TicketEvent, TicketIdOrSlug, TicketWorkflowState, TicketWorkspaceActionPriority,
    project_ticket_workspace_item,
};

use crate::records::{
    ObjectiveDetail, ObjectiveEventDetail, ObjectiveLinkSummary, ObjectiveLinkedTicketSummary,
    ObjectiveQueryItem, ObjectiveQueryRequest, ObjectiveQueryResponse, ObjectiveResourceSummary,
    ObjectiveShowRequest, ObjectiveSummary, ProjectRecordList, QueryPage, TicketActionEligibility,
    TicketAssignmentPrincipalSummary, TicketAssignmentSummary, TicketDetail, TicketEventDetail,
    TicketEvidenceEvent, TicketEvidenceSummary, TicketListPageRequest, TicketMergeRequestSummary,
    TicketQueryItem, TicketQueryRequest, TicketQueryResponse, TicketRelationView,
    TicketRoleAssignmentSummary, TicketShowRequest, TicketSummary, TicketSummaryPage,
    summarize_body, truncate_body, validate_project_id,
};
use crate::store::{
    ControlPlaneStore, MemoryDocumentRecord, MemoryStagingRecord, MemoryStagingResolutionRecord,
    ObjectiveEventRecord, ObjectiveRecord, ObjectiveTicketLinkRecord, SqliteWorkspaceStore,
    TicketAssignmentPrincipal, TicketAssignmentRole, WorkspaceResourceKind,
};
use crate::{Error, Result};

const DETAIL_BODY_LIMIT: usize = 64 * 1024;
const TICKET_EVENT_LIMIT: usize = 100;
const TICKET_EVENT_BODY_LIMIT: usize = 16 * 1024;
const DEFAULT_MEMORY_DOCUMENT_BODY: &str = "# Memory\n\n";
const RECORD_SOURCE_WORKSPACE_SQLITE: &str = "workspace-sqlite";

/// Workspace-scoped runtime authority for project resources.
///
/// Normal Backend API handlers should depend on this authority abstraction, not
/// on legacy filesystem layouts. Filesystem readers belong in explicitly
/// temporary migration/repair tools and tests, not normal runtime authority paths.
pub trait WorkspaceAuthority: ObjectiveAuthority + TicketAuthority + MemoryAuthority {}

impl<T> WorkspaceAuthority for T where T: ObjectiveAuthority + TicketAuthority + MemoryAuthority {}

pub trait TicketAuthority {
    fn list_tickets(&self, limit: usize) -> Result<ProjectRecordList<TicketSummary>>;
    fn list_ticket_page(&self, request: TicketListPageRequest) -> Result<TicketSummaryPage>;
    fn query_tickets(&self, query: TicketQueryRequest) -> Result<TicketQueryResponse>;
    fn ticket(&self, id: &str) -> Result<TicketDetail>;
    fn show_ticket(&self, id: &str, query: TicketShowRequest) -> Result<TicketDetail>;
}

pub trait ObjectiveAuthority {
    fn list_objectives(&self, limit: usize) -> Result<ProjectRecordList<ObjectiveSummary>>;
    fn query_objectives(&self, query: ObjectiveQueryRequest) -> Result<ObjectiveQueryResponse>;
    fn objective(&self, id: &str) -> Result<ObjectiveDetail>;
    fn show_objective(&self, id: &str, query: ObjectiveShowRequest) -> Result<ObjectiveDetail>;
    fn create_objective(&self, input: ObjectiveCreateInput) -> Result<ObjectiveDetail>;
    fn edit_objective(&self, id: &str, input: ObjectiveEditInput) -> Result<ObjectiveDetail>;
    fn set_objective_state(&self, id: &str, state: &str) -> Result<ObjectiveDetail>;
    fn link_objective_ticket(&self, id: &str, ticket_id: &str) -> Result<ObjectiveDetail>;
    fn unlink_objective_ticket(&self, id: &str, ticket_id: &str) -> Result<ObjectiveDetail>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectiveCreateInput {
    pub title: String,
    pub body_md: String,
    pub state: String,
    pub linked_tickets: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ObjectiveEditInput {
    pub title: Option<String>,
    pub old_string: Option<String>,
    pub new_string: Option<String>,
    pub replace_all: bool,
}

pub trait MemoryAuthority {
    fn ensure_memory_document(&self) -> Result<MemoryDocument>;
    fn memory_document(&self) -> Result<MemoryDocument>;
    fn update_memory_document(&self, body_md: &str) -> Result<MemoryDocument>;
    fn list_memory_staging_records(&self, limit: usize) -> Result<Vec<MemoryStagingEntry>>;
    fn memory_staging_record(&self, candidate_id: &str) -> Result<MemoryStagingEntry>;
    fn upsert_memory_staging_record(
        &self,
        candidate_id: &str,
        raw_json: &str,
        source_path: Option<&str>,
    ) -> Result<MemoryStagingEntry>;
    fn close_memory_staging_record(
        &self,
        candidate_id: &str,
        action: &str,
        reason: &str,
        affected_refs_json: &str,
    ) -> Result<MemoryStagingResolution>;
    fn list_memory_staging_resolutions(&self, limit: usize)
    -> Result<Vec<MemoryStagingResolution>>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryDocument {
    pub body_md: String,
    pub created_at: String,
    pub updated_at: String,
    pub record_source: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryStagingEntry {
    pub candidate_id: String,
    pub raw_json: String,
    pub source_path: Option<String>,
    pub imported_at: String,
    pub record_source: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryStagingResolution {
    pub candidate_id: String,
    pub action: String,
    pub reason: String,
    pub affected_refs_json: String,
    pub staging_raw_json: String,
    pub source_path: Option<String>,
    pub imported_at: String,
    pub resolved_at: String,
    pub record_source: String,
}

#[derive(Clone)]
struct AuthorityMergeRequestSource {
    store: SqliteWorkspaceStore,
}

impl merge_request::AssignmentSource for AuthorityMergeRequestSource {
    fn current_assignment(
        &self,
        workspace_id: &str,
        ticket_id: &str,
    ) -> std::result::Result<Option<merge_request::CurrentAssignment>, String> {
        self.store
            .get_current_ticket_coder_assignment(workspace_id, ticket_id)
            .map(|assignment| {
                assignment.map(|assignment| merge_request::CurrentAssignment {
                    assignment_id: assignment.assignment_id,
                    ticket_id: ticket_id.to_string(),
                    runtime_id: assignment.worker.runtime_id,
                    worker_id: assignment.worker.worker_id,
                })
            })
            .map_err(|error| error.to_string())
    }
}

impl merge_request::RepositorySource for AuthorityMergeRequestSource {
    fn repository_belongs_to_workspace(
        &self,
        workspace_id: &str,
        repository_id: &str,
    ) -> std::result::Result<bool, String> {
        self.store
            .get_repository(workspace_id, repository_id)
            .map(|repository| repository.is_some())
            .map_err(|error| error.to_string())
    }
}

pub trait TicketMergeRevisionSource: Send + Sync {
    fn resolve_subject_ref(&self, repository_id: &str, selector: &str) -> Option<String>;
}

struct UnresolvedTicketMergeRevisionSource;

impl TicketMergeRevisionSource for UnresolvedTicketMergeRevisionSource {
    fn resolve_subject_ref(&self, _repository_id: &str, _selector: &str) -> Option<String> {
        None
    }
}

#[derive(Clone)]
pub struct SqliteWorkspaceAuthority {
    workspace_id: String,
    store: SqliteWorkspaceStore,
    ticket_backend: SqliteTicketBackend,
    merge_request_store: Arc<MergeRequestStore>,
    merge_revision_source: Arc<dyn TicketMergeRevisionSource>,
}

impl SqliteWorkspaceAuthority {
    pub fn new(database_path: impl Into<PathBuf>, workspace_id: impl Into<String>) -> Result<Self> {
        let database_path = database_path.into();
        let workspace_id = workspace_id.into();
        let store = SqliteWorkspaceStore::open(&database_path)?;
        let merge_request_source = Arc::new(AuthorityMergeRequestSource {
            store: store.clone(),
        });
        Ok(Self {
            workspace_id: workspace_id.clone(),
            store,
            ticket_backend: SqliteTicketBackend::open_verified(
                database_path.clone(),
                workspace_id,
            )?,
            merge_request_store: Arc::new(
                MergeRequestStore::open(
                    database_path,
                    merge_request_source.clone(),
                    merge_request_source,
                )
                .map_err(|error| Error::Store(error.to_string()))?,
            ),
            merge_revision_source: Arc::new(UnresolvedTicketMergeRevisionSource),
        })
    }

    pub fn with_merge_revision_source(
        mut self,
        merge_revision_source: Arc<dyn TicketMergeRevisionSource>,
    ) -> Self {
        self.merge_revision_source = merge_revision_source;
        self
    }

    fn resource_key(&self, kind: WorkspaceResourceKind, resource_id: &str) -> Result<String> {
        self.store
            .resource_key(&self.workspace_id, kind, resource_id)?
            .ok_or_else(|| Error::Store(format!("missing resource key for {resource_id}")))
    }

    fn objective_record(&self, reference: &str) -> Result<ObjectiveRecord> {
        let id = self
            .store
            .resolve_resource_reference(
                &self.workspace_id,
                WorkspaceResourceKind::Objective,
                reference,
            )?
            .ok_or_else(|| unknown_objective_error(reference))?;
        self.store
            .get_objective(&self.workspace_id, &id)?
            .ok_or_else(|| unknown_objective_error(reference))
    }

    fn objective_detail_from_record(&self, record: ObjectiveRecord) -> Result<ObjectiveDetail> {
        let linked_tickets = self
            .store
            .list_objective_ticket_links(&self.workspace_id, &record.objective_id)?
            .into_iter()
            .map(|link| link.ticket_id)
            .collect::<Vec<_>>();
        let linked_ticket_summaries = self
            .list_tickets(1_000)?
            .items
            .into_iter()
            .filter(|ticket| linked_tickets.iter().any(|id| id == &ticket.id))
            .map(|ticket| ObjectiveLinkedTicketSummary {
                id: ticket.id,
                resource_key: ticket.resource_key,
                title: ticket.title,
                state: ticket.state,
            })
            .collect::<Vec<_>>();
        let resources = self
            .store
            .list_objective_resources(&self.workspace_id, &record.objective_id)?
            .into_iter()
            .map(|resource| ObjectiveResourceSummary {
                path: resource.resource_path,
                media_type: resource.media_type,
                bytes: resource.body.len(),
                updated_at: resource.updated_at,
            })
            .collect();
        let (body, body_truncated) = truncate_body(&record.body_md, DETAIL_BODY_LIMIT);
        let all_events = self
            .store
            .list_objective_events(&self.workspace_id, &record.objective_id)?;
        let event_start = all_events.len().saturating_sub(TICKET_EVENT_LIMIT);
        let events = all_events[event_start..]
            .iter()
            .map(|event| ObjectiveEventDetail {
                event_ref: event.event_id.clone(),
                kind: event.kind.clone(),
                body: event
                    .body_md
                    .as_deref()
                    .map(|body| truncate_body(body, TICKET_EVENT_BODY_LIMIT).0),
                created_at: event.created_at.clone(),
            })
            .collect::<Vec<_>>();
        let revision = format!(
            "{}:{}",
            record.updated_at,
            all_events
                .last()
                .map(|event| event.event_id.as_str())
                .unwrap_or("none")
        );
        Ok(ObjectiveDetail {
            resource_key: self
                .resource_key(WorkspaceResourceKind::Objective, &record.objective_id)?,
            id: record.objective_id,
            title: record.title,
            state: record.state,
            revision,
            created_at: Some(record.created_at),
            updated_at: Some(record.updated_at),
            linked_tickets,
            linked_ticket_summaries,
            resources,
            body,
            body_truncated,
            events,
            event_page: QueryPage {
                limit: TICKET_EVENT_LIMIT,
                returned: all_events.len() - event_start,
                has_more: event_start > 0,
                next_cursor: (event_start > 0).then(|| event_start.to_string()),
                sort: "sequence_desc".to_string(),
                source_limit: None,
                source_truncated: false,
            },
            record_source: RECORD_SOURCE_WORKSPACE_SQLITE.to_string(),
        })
    }

    fn insert_objective_event(
        &self,
        objective_id: &str,
        kind: &str,
        body_md: Option<&str>,
    ) -> Result<()> {
        let event_id = allocate_record_id(
            unix_epoch_millis_now().map_err(|err| {
                invalid_objective_error(format!("failed to read objective event clock: {err}"))
            })?,
            |candidate| {
                self.store
                    .list_objective_events(&self.workspace_id, objective_id)
                    .map(|events| events.iter().any(|event| event.event_id == candidate))
                    .unwrap_or(true)
            },
        )
        .map_err(|err| {
            invalid_objective_error(format!("failed to allocate objective event id: {err}"))
        })?;
        self.store.insert_objective_event(&ObjectiveEventRecord {
            workspace_id: self.workspace_id.clone(),
            objective_id: objective_id.to_string(),
            event_id,
            kind: kind.to_string(),
            body_md: body_md.map(str::to_string),
            created_at: now_rfc3339(),
        })
    }

    fn query_ticket_candidate_ids(
        &self,
        query: &TicketQueryRequest,
        sort: TicketQuerySort,
        after: Option<&(String, String)>,
        limit: usize,
    ) -> Result<Vec<String>> {
        let mut values = vec![SqlValue::Text(self.workspace_id.clone())];
        let mut predicates = vec!["t.workspace_id=?1".to_string()];
        let mut bind = |value: SqlValue| {
            values.push(value);
            format!("?{}", values.len())
        };
        if !query.states.is_empty() {
            let states = query
                .states
                .iter()
                .map(|state| bind(SqlValue::Text(state.clone())))
                .collect::<Vec<_>>();
            predicates.push(format!("t.workflow_state IN ({})", states.join(",")));
        }
        if let Some(text) = query.query.as_deref().filter(|text| !text.is_empty()) {
            let pattern = bind(SqlValue::Text(format!("%{}%", text.to_lowercase())));
            predicates.push(format!(
                "(lower(t.title) LIKE {p} OR lower(t.body) LIKE {p} OR EXISTS (
                    SELECT 1 FROM typed_ticket_events e
                    WHERE e.workspace_id=t.workspace_id AND e.ticket_id=t.ticket_id
                      AND lower(e.body) LIKE {p}))",
                p = pattern
            ));
        }
        if let Some(value) = &query.updated_after {
            let value = bind(SqlValue::Text(value.clone()));
            predicates.push(format!("COALESCE(t.updated_at,'')>{value}"));
        }
        if let Some(value) = &query.updated_before {
            let value = bind(SqlValue::Text(value.clone()));
            predicates.push(format!("COALESCE(t.updated_at,'')<{value}"));
        }
        if let Some(value) = &query.linked_objective_id {
            let value = bind(SqlValue::Text(value.clone()));
            predicates.push(format!("EXISTS (SELECT 1 FROM objective_ticket_links link WHERE link.workspace_id=t.workspace_id AND link.ticket_id=t.ticket_id AND link.objective_id={value})"));
        }
        if query.related_ticket_id.is_some() || query.relation_kind.is_some() {
            let related = query
                .related_ticket_id
                .as_ref()
                .map(|value| bind(SqlValue::Text(value.clone())));
            let kind = query
                .relation_kind
                .as_ref()
                .map(|value| bind(SqlValue::Text(value.clone())));
            let related = related
                .map(|value| format!("AND ((r.ticket_id=t.ticket_id AND r.target={value}) OR (r.target=t.ticket_id AND r.ticket_id={value}))"))
                .unwrap_or_else(|| {
                    "AND (r.ticket_id=t.ticket_id OR r.target=t.ticket_id)".to_string()
                });
            let kind = kind
                .map(|value| format!("AND r.kind={value}"))
                .unwrap_or_default();
            predicates.push(format!("EXISTS (SELECT 1 FROM typed_ticket_relations r WHERE r.workspace_id=t.workspace_id {related} {kind})"));
        }
        let blocker = "EXISTS (SELECT 1 FROM typed_ticket_relations relation
            JOIN typed_tickets blocker ON blocker.workspace_id=relation.workspace_id
             AND blocker.ticket_id=CASE WHEN relation.ticket_id=t.ticket_id THEN relation.target ELSE relation.ticket_id END
            WHERE relation.workspace_id=t.workspace_id
              AND ((relation.ticket_id=t.ticket_id AND relation.kind='depends_on')
                OR (relation.target=t.ticket_id AND relation.kind='blocks'))
              AND blocker.workflow_state NOT IN ('done','closed'))";
        let active_blocker = "EXISTS (SELECT 1 FROM typed_ticket_relations relation
            JOIN typed_tickets blocker ON blocker.workspace_id=relation.workspace_id
             AND blocker.ticket_id=CASE WHEN relation.ticket_id=t.ticket_id THEN relation.target ELSE relation.ticket_id END
            WHERE relation.workspace_id=t.workspace_id
              AND ((relation.ticket_id=t.ticket_id AND relation.kind='depends_on')
                OR (relation.target=t.ticket_id AND relation.kind='blocks'))
              AND blocker.workflow_state NOT IN ('queued','inprogress','done','closed'))";
        let report_index = "(SELECT max(event.event_index) FROM typed_ticket_events event WHERE event.workspace_id=t.workspace_id AND event.ticket_id=t.ticket_id AND event.kind='implementation_report')";
        let edit_index = "(SELECT max(event.event_index) FROM typed_ticket_events event WHERE event.workspace_id=t.workspace_id AND event.ticket_id=t.ticket_id AND event.kind='item_edit')";
        let current_report = format!(
            "({report_index} IS NOT NULL AND ({edit_index} IS NULL OR {report_index}>={edit_index}))"
        );
        let merge_request_id = "(SELECT relation.merge_request_id FROM merge_request_ticket_relations relation
            JOIN merge_requests request ON request.workspace_id=relation.workspace_id AND request.merge_request_id=relation.merge_request_id
            WHERE relation.workspace_id=t.workspace_id AND relation.ticket_id=t.ticket_id
            ORDER BY CASE WHEN request.state='open' THEN 0 ELSE 1 END, request.created_at DESC LIMIT 1)";
        let review_subject = format!(
            "(SELECT json_extract(requested.payload_json,'$.subject_ref')
            FROM merge_request_thread_events requested
            WHERE requested.workspace_id=t.workspace_id
              AND requested.merge_request_id={merge_request_id}
              AND requested.kind='review_requested'
            ORDER BY requested.sequence DESC LIMIT 1)"
        );
        let review_decision = format!(
            "(SELECT json_extract(event.payload_json,'$.decision')
            FROM merge_request_thread_events event
            WHERE event.workspace_id=t.workspace_id
              AND event.merge_request_id={merge_request_id} AND event.kind='review'
              AND json_extract(event.payload_json,'$.subject_ref')={review_subject}
              AND NOT EXISTS (SELECT 1 FROM merge_request_thread_events revoked
                WHERE revoked.workspace_id=event.workspace_id
                  AND revoked.merge_request_id=event.merge_request_id
                  AND revoked.kind='review_revoked'
                  AND json_extract(revoked.payload_json,'$.review_event_id')=event.event_id)
            ORDER BY event.sequence DESC LIMIT 1)"
        );
        let review_status = format!(
            "CASE WHEN {merge_request_id} IS NULL THEN 'none' WHEN {review_decision}='approve' THEN 'approved' WHEN {review_decision}='request_changes' THEN 'request_changes' ELSE 'pending' END"
        );
        let has_commit = format!(
            "({review_subject} IS NOT NULL OR EXISTS (SELECT 1 FROM typed_ticket_event_references reference WHERE reference.workspace_id=t.workspace_id AND reference.ticket_id=t.ticket_id AND reference.kind='commit'))"
        );
        if !query.event_kinds.is_empty() {
            let event_kinds = query
                .event_kinds
                .iter()
                .map(|event_kind| bind(SqlValue::Text(event_kind.clone())))
                .collect::<Vec<_>>();
            predicates.push(format!("EXISTS (SELECT 1 FROM typed_ticket_events event WHERE event.workspace_id=t.workspace_id AND event.ticket_id=t.ticket_id AND event.kind IN ({}))", event_kinds.join(",")));
        }
        for evidence in &query.evidence {
            predicates.push(match evidence.as_str() {
                "implementation_report" => format!("{report_index} IS NOT NULL"),
                "implementation_report_after_rescope" => current_report.clone(),
                "merge_request" => format!("{merge_request_id} IS NOT NULL"),
                "commit" => has_commit.clone(),
                "approved_review" => format!("{review_status}='approved'"),
                other => {
                    return Err(Error::InvalidRecordId(format!(
                        "unsupported evidence filter `{other}`"
                    )));
                }
            });
        }
        if let Some(status) = &query.review_status {
            let status = if matches!(status.as_str(), "unresolved_changes" | "changes_requested") {
                "request_changes"
            } else {
                status.as_str()
            };
            let status = bind(SqlValue::Text(status.to_string()));
            predicates.push(format!("{review_status}={status}"));
        }
        for attention in &query.attention {
            predicates.push(match attention.as_str() {
                "done_not_closed" => "t.workflow_state='done'".to_string(),
                "implementation_report_not_closed" => {
                    format!("{report_index} IS NOT NULL AND t.workflow_state!='closed'")
                }
                "report_after_rescope" => current_report.clone(),
                "unresolved_review" | "unresolved_changes" => {
                    format!("{review_status}='request_changes'")
                }
                "missing_commit" => format!("NOT {has_commit}"),
                "blocked" => blocker.to_string(),
                "unblocked" => format!("NOT {blocker}"),
                "ready" => format!("t.workflow_state='ready' AND NOT {blocker}"),
                "awaiting_review" => format!("{review_status}='pending'"),
                "stale_after_rescope" => {
                    format!("{report_index} IS NOT NULL AND NOT {current_report}")
                }
                "missing_evidence" => format!(
                    "NOT ({current_report} AND {has_commit} AND {review_status}='approved')"
                ),
                other => {
                    return Err(Error::InvalidRecordId(format!(
                        "unsupported attention filter `{other}`"
                    )));
                }
            });
        }
        let rank_expression = match sort {
            TicketQuerySort::Priority => format!(
                "CASE WHEN t.workflow_state='ready' AND NOT {active_blocker} THEN 0 WHEN t.workflow_state IN ('queued','inprogress') THEN 1 ELSE 2 END"
            ),
            TicketQuerySort::Relevance => {
                if let Some(text) = query.query.as_deref().filter(|text| !text.is_empty()) {
                    let pattern = bind(SqlValue::Text(format!("%{}%", text.to_lowercase())));
                    format!(
                        "CASE WHEN lower(t.title) LIKE {pattern} THEN 0 WHEN lower(t.body) LIKE {pattern} THEN 1 WHEN EXISTS (SELECT 1 FROM typed_ticket_events event WHERE event.workspace_id=t.workspace_id AND event.ticket_id=t.ticket_id AND lower(event.body) LIKE {pattern}) THEN 2 ELSE 3 END"
                    )
                } else {
                    "3".to_string()
                }
            }
            _ => "0".to_string(),
        };
        if let Some((key, id)) = after {
            match sort {
                TicketQuerySort::UpdatedDesc => {
                    let key = bind(SqlValue::Text(key.clone()));
                    let id = bind(SqlValue::Text(id.clone()));
                    predicates.push(format!("(COALESCE(t.updated_at,'')<{key} OR (COALESCE(t.updated_at,'')={key} AND t.ticket_id>{id}))"));
                }
                TicketQuerySort::CreatedDesc => {
                    let key = bind(SqlValue::Text(key.clone()));
                    let id = bind(SqlValue::Text(id.clone()));
                    predicates.push(format!("(COALESCE(t.created_at,'')<{key} OR (COALESCE(t.created_at,'')={key} AND t.ticket_id>{id}))"));
                }
                TicketQuerySort::Title => {
                    let key = bind(SqlValue::Text(key.clone()));
                    let id = bind(SqlValue::Text(id.clone()));
                    predicates.push(format!(
                        "(lower(t.title)>{key} OR (lower(t.title)={key} AND t.ticket_id>{id}))"
                    ));
                }
                TicketQuerySort::Priority | TicketQuerySort::Relevance => {
                    let (rank, updated_at) = key.split_once('|').unwrap_or(("9", ""));
                    let rank = bind(SqlValue::Integer(rank.parse::<i64>().unwrap_or(9)));
                    let updated_at = bind(SqlValue::Text(updated_at.to_string()));
                    let id = bind(SqlValue::Text(id.clone()));
                    predicates.push(format!("({rank_expression}>{rank} OR ({rank_expression}={rank} AND (COALESCE(t.updated_at,'')<{updated_at} OR (COALESCE(t.updated_at,'')={updated_at} AND t.ticket_id>{id}))))"));
                }
            }
        }
        let order = match sort {
            TicketQuerySort::Title => "t.title COLLATE NOCASE ASC, t.ticket_id ASC".to_string(),
            TicketQuerySort::CreatedDesc => "t.created_at DESC, t.ticket_id ASC".to_string(),
            TicketQuerySort::UpdatedDesc => "t.updated_at DESC, t.ticket_id ASC".to_string(),
            TicketQuerySort::Priority | TicketQuerySort::Relevance => {
                format!("{rank_expression} ASC, t.updated_at DESC, t.ticket_id ASC")
            }
        };
        let limit = bind(SqlValue::Integer(i64::try_from(limit).unwrap_or(i64::MAX)));
        let sql = format!(
            "SELECT t.ticket_id FROM typed_tickets t WHERE {} ORDER BY {order} LIMIT {limit}",
            predicates.join(" AND ")
        );
        self.store.with_conn(|connection| {
            let mut statement = connection.prepare(&sql)?;
            let rows = statement.query_map(params_from_iter(values.iter()), |row| row.get(0))?;
            Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
        })
    }

    fn query_objective_candidate_ids(
        &self,
        query: &ObjectiveQueryRequest,
        sort: ObjectiveQuerySort,
        after: Option<&(String, String)>,
        limit: usize,
    ) -> Result<Vec<String>> {
        let mut values = vec![SqlValue::Text(self.workspace_id.clone())];
        let mut predicates = vec!["o.workspace_id=?1".to_string()];
        let mut bind = |value: SqlValue| {
            values.push(value);
            format!("?{}", values.len())
        };
        if !query.states.is_empty() {
            let states = query
                .states
                .iter()
                .map(|state| bind(SqlValue::Text(state.clone())))
                .collect::<Vec<_>>();
            predicates.push(format!("o.state IN ({})", states.join(",")));
        }
        if let Some(text) = query.query.as_deref().filter(|text| !text.is_empty()) {
            let pattern = bind(SqlValue::Text(format!("%{}%", text.to_lowercase())));
            predicates.push(format!(
                "(lower(o.title) LIKE {pattern} OR lower(o.body_md) LIKE {pattern})"
            ));
        }
        if let Some(value) = &query.updated_after {
            let value = bind(SqlValue::Text(value.clone()));
            predicates.push(format!("o.updated_at>{value}"));
        }
        if let Some(value) = &query.updated_before {
            let value = bind(SqlValue::Text(value.clone()));
            predicates.push(format!("o.updated_at<{value}"));
        }
        if let Some(value) = &query.linked_ticket_id {
            let value = bind(SqlValue::Text(value.clone()));
            predicates.push(format!("EXISTS (SELECT 1 FROM objective_ticket_links link WHERE link.workspace_id=o.workspace_id AND link.objective_id=o.objective_id AND link.ticket_id={value})"));
        }
        let relevance_rank = if let Some(text) =
            query.query.as_deref().filter(|text| !text.is_empty())
        {
            let pattern = bind(SqlValue::Text(format!("%{}%", text.to_lowercase())));
            format!(
                "CASE WHEN lower(o.title) LIKE {pattern} THEN 0 WHEN lower(o.body_md) LIKE {pattern} THEN 1 ELSE 2 END"
            )
        } else {
            "2".to_string()
        };
        if let Some((key, id)) = after {
            match sort {
                ObjectiveQuerySort::UpdatedDesc => {
                    let key = bind(SqlValue::Text(key.clone()));
                    let id = bind(SqlValue::Text(id.clone()));
                    predicates.push(format!(
                        "(o.updated_at<{key} OR (o.updated_at={key} AND o.objective_id>{id}))"
                    ));
                }
                ObjectiveQuerySort::CreatedDesc => {
                    let key = bind(SqlValue::Text(key.clone()));
                    let id = bind(SqlValue::Text(id.clone()));
                    predicates.push(format!(
                        "(o.created_at<{key} OR (o.created_at={key} AND o.objective_id>{id}))"
                    ));
                }
                ObjectiveQuerySort::Title => {
                    let key = bind(SqlValue::Text(key.clone()));
                    let id = bind(SqlValue::Text(id.clone()));
                    predicates.push(format!(
                        "(lower(o.title)>{key} OR (lower(o.title)={key} AND o.objective_id>{id}))"
                    ));
                }
                ObjectiveQuerySort::Relevance => {
                    let (rank, updated_at) = key.split_once('|').unwrap_or(("9", ""));
                    let rank = bind(SqlValue::Integer(rank.parse::<i64>().unwrap_or(9)));
                    let updated_at = bind(SqlValue::Text(updated_at.to_string()));
                    let id = bind(SqlValue::Text(id.clone()));
                    predicates.push(format!("({relevance_rank}>{rank} OR ({relevance_rank}={rank} AND (o.updated_at<{updated_at} OR (o.updated_at={updated_at} AND o.objective_id>{id}))))"));
                }
            }
        }
        let order = match sort {
            ObjectiveQuerySort::Title => {
                "o.title COLLATE NOCASE ASC, o.objective_id ASC".to_string()
            }
            ObjectiveQuerySort::CreatedDesc => "o.created_at DESC, o.objective_id ASC".to_string(),
            ObjectiveQuerySort::UpdatedDesc => "o.updated_at DESC, o.objective_id ASC".to_string(),
            ObjectiveQuerySort::Relevance => {
                format!("{relevance_rank} ASC, o.updated_at DESC, o.objective_id ASC")
            }
        };
        let limit = bind(SqlValue::Integer(i64::try_from(limit).unwrap_or(i64::MAX)));
        let sql = format!(
            "SELECT o.objective_id FROM objectives o WHERE {} ORDER BY {order} LIMIT {limit}",
            predicates.join(" AND ")
        );
        self.store.with_conn(|connection| {
            let mut statement = connection.prepare(&sql)?;
            let rows = statement.query_map(params_from_iter(values.iter()), |row| row.get(0))?;
            Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
        })
    }

    fn read_ticket_detail(
        &self,
        reference: &str,
        request: TicketShowRequest,
    ) -> Result<TicketDetail> {
        let id = self
            .store
            .resolve_resource_reference(
                &self.workspace_id,
                WorkspaceResourceKind::Ticket,
                reference,
            )?
            .ok_or_else(|| Error::Ticket(ticket::TicketError::NotFound(reference.to_string())))?;
        let ticket = self.ticket_backend.show(TicketIdOrSlug::Id(id))?;
        self.ticket_detail_from_ticket(ticket, request)
    }

    fn ticket_detail_from_ticket(
        &self,
        ticket: ticket::Ticket,
        request: TicketShowRequest,
    ) -> Result<TicketDetail> {
        let id = ticket.meta.id.as_str();
        let dependency_check = self
            .ticket_backend
            .dependency_check(TicketIdOrSlug::Id(id.to_string()))?;
        let (body, body_truncated) =
            truncate_body(ticket.document.body.as_str(), DETAIL_BODY_LIMIT);
        let event_limit = request
            .event_limit
            .unwrap_or(TICKET_EVENT_LIMIT)
            .clamp(1, TICKET_EVENT_LIMIT);
        let event_end = request
            .event_cursor
            .as_deref()
            .map(|cursor| parse_offset_cursor(cursor, "event_cursor"))
            .transpose()?
            .unwrap_or(ticket.events.len())
            .min(ticket.events.len());
        let event_start = event_end.saturating_sub(event_limit);
        let events = ticket.events[event_start..event_end]
            .iter()
            .enumerate()
            .map(|(index, event)| ticket_event_detail(event_start + index, event))
            .collect::<Vec<_>>();
        let linked_objectives = self
            .store
            .list_objectives_for_ticket(&self.workspace_id, id, 1_000)?
            .into_iter()
            .map(|objective| {
                Ok::<_, Error>(ObjectiveLinkSummary {
                    resource_key: self
                        .resource_key(WorkspaceResourceKind::Objective, &objective.objective_id)?,
                    id: objective.objective_id,
                    title: objective.title,
                    state: objective.state,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let implementation_reports = ticket
            .events
            .iter()
            .enumerate()
            .filter(|(_, event)| event.kind.as_str() == "implementation_report")
            .map(|(sequence, event)| ticket_evidence_event(sequence, event))
            .collect::<Vec<_>>();
        let role_assignments = self
            .store
            .list_current_ticket_role_assignments(&self.workspace_id, id)?;
        let assignments = role_assignments
            .iter()
            .cloned()
            .map(|assignment| TicketRoleAssignmentSummary {
                assignment_id: assignment.assignment_id,
                role: assignment.role.as_str().to_string(),
                principal: match assignment.principal {
                    TicketAssignmentPrincipal::User { account_id } => {
                        TicketAssignmentPrincipalSummary::User { account_id }
                    }
                    TicketAssignmentPrincipal::Worker {
                        runtime_id,
                        worker_id,
                    } => TicketAssignmentPrincipalSummary::Worker {
                        runtime_id,
                        worker_id,
                    },
                    TicketAssignmentPrincipal::WorkspaceAgent { agent_key } => {
                        TicketAssignmentPrincipalSummary::WorkspaceAgent { agent_key }
                    }
                },
                assigned_by: assignment.assigned_by,
                assigned_at: assignment.assigned_at,
            })
            .collect::<Vec<_>>();
        let current_coder = role_assignments
            .iter()
            .find(|assignment| assignment.role == TicketAssignmentRole::Coder)
            .and_then(|assignment| {
                assignment
                    .principal
                    .worker()
                    .map(|worker| (assignment, worker))
            })
            .map(|(assignment, worker)| {
                let worker_resource_key = self.store.resource_key(
                    &self.workspace_id,
                    WorkspaceResourceKind::Worker,
                    &worker.worker_id,
                )?;
                Ok::<_, Error>(TicketAssignmentSummary {
                    assignment_id: assignment.assignment_id.clone(),
                    runtime_id: worker.runtime_id,
                    worker_id: worker.worker_id,
                    worker_resource_key,
                })
            })
            .transpose()?;
        let has_orchestrator = role_assignments
            .iter()
            .any(|assignment| assignment.role == TicketAssignmentRole::Orchestrator);
        let has_coder = role_assignments
            .iter()
            .any(|assignment| assignment.role == TicketAssignmentRole::Coder);
        let has_target = ticket.meta.repository_id.is_some() && ticket.meta.ref_selector.is_some();
        let has_blockers = !ticket.relations.blockers.is_empty();
        let mut queue_assignment_blockers = Vec::new();
        for ticket_id in &dependency_check.queue_tickets {
            let assignments = self
                .store
                .list_current_ticket_role_assignments(&self.workspace_id, ticket_id)?;
            if !assignments
                .iter()
                .any(|assignment| assignment.role == TicketAssignmentRole::Orchestrator)
            {
                queue_assignment_blockers.push(format!(
                    "Ticket {ticket_id} requires an active Orchestrator assignment"
                ));
            }
            if assignments
                .iter()
                .any(|assignment| assignment.role == TicketAssignmentRole::Coder)
            {
                queue_assignment_blockers
                    .push(format!("Ticket {ticket_id} has an active Coder assignment"));
            }
        }
        let mut assignment_diagnostics = Vec::new();
        if let Some(legacy_assignee) = ticket
            .meta
            .assignee
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            assignment_diagnostics.push(format!(
                "legacy Ticket assignee `{legacy_assignee}` is not assignment authority"
            ));
        }
        let mut action_blockers = Vec::new();
        if !has_target {
            action_blockers.push("Ticket target is required".to_string());
        }
        if !dependency_check.queue_guard.can_queue_for_orchestrator {
            if let Some(reason) = dependency_check.queue_guard.blocked_reason.clone() {
                action_blockers.push(reason);
            } else if let Some(reason) = dependency_check.queue_guard.reason.clone() {
                action_blockers.push(reason);
            }
        }
        let queue_assignments_valid = queue_assignment_blockers.is_empty();
        action_blockers.extend(queue_assignment_blockers);
        let action_eligibility = TicketActionEligibility {
            can_assign_orchestrator: matches!(
                ticket.meta.workflow_state,
                TicketWorkflowState::Planning | TicketWorkflowState::Ready
            ) && !has_orchestrator
                && !has_coder,
            can_unassign_orchestrator: has_orchestrator
                && matches!(
                    ticket.meta.workflow_state,
                    TicketWorkflowState::Planning | TicketWorkflowState::Ready
                ),
            can_queue: ticket.meta.workflow_state == TicketWorkflowState::Ready
                && has_orchestrator
                && !has_coder
                && has_target
                && dependency_check.queue_guard.can_queue_for_orchestrator
                && queue_assignments_valid,
            can_start_manual_coder: ticket.meta.workflow_state == TicketWorkflowState::Ready
                && !has_orchestrator
                && !has_coder
                && has_target
                && !has_blockers,
            blockers: action_blockers,
        };
        let merge_request = match self.merge_request_store.get(&self.workspace_id, id) {
            Ok(request) => {
                let current_subject_ref = request.selector_from.as_deref().and_then(|selector| {
                    self.merge_revision_source
                        .resolve_subject_ref(&request.repository_id, selector)
                });
                Some(merge_request_summary(request, current_subject_ref))
            }
            Err(MergeRequestError::NotFound) => None,
            Err(error) => return Err(Error::Store(error.to_string())),
        };
        let evidence = ticket_evidence_summary(
            ticket.meta.repository_id.as_deref(),
            &ticket.events,
            merge_request.as_ref(),
        );
        let item_revision = ticket
            .events
            .iter()
            .rev()
            .find(|event| matches!(event.kind.as_str(), "create" | "item_edit"))
            .and_then(|event| event.attributes.get("event_id").cloned())
            .or_else(|| ticket.meta.updated_at.clone())
            .unwrap_or_else(|| format!("{}:0", ticket.meta.id));
        let resource_key = ticket
            .meta
            .resource_key
            .clone()
            .or(self.store.resource_key(
                &self.workspace_id,
                WorkspaceResourceKind::Ticket,
                &ticket.meta.id,
            )?)
            .ok_or_else(|| Error::Store(format!("missing resource key for {}", ticket.meta.id)))?;
        let mut relations: TicketRelationView = ticket.relations.into();
        for relation in &mut relations.outgoing {
            relation.target_resource_key = self.store.resource_key(
                &self.workspace_id,
                WorkspaceResourceKind::Ticket,
                &relation.target,
            )?;
        }
        for relation in &mut relations.incoming {
            relation.source_resource_key = self.store.resource_key(
                &self.workspace_id,
                WorkspaceResourceKind::Ticket,
                &relation.source_ticket,
            )?;
        }
        for blocker in &mut relations.blockers {
            blocker.blocking_resource_key = self.store.resource_key(
                &self.workspace_id,
                WorkspaceResourceKind::Ticket,
                &blocker.blocking_ticket,
            )?;
        }
        Ok(TicketDetail {
            id: ticket.meta.id,
            resource_key,
            title: ticket.meta.title,
            state: ticket.meta.workflow_state.as_str().to_string(),
            readiness: ticket.meta.readiness,
            priority: ticket.meta.priority,
            created_at: ticket.meta.created_at,
            updated_at: ticket.meta.updated_at,
            item_revision,
            queued_by: ticket.meta.queued_by,
            queued_at: ticket.meta.queued_at,
            repository_id: ticket.meta.repository_id,
            ref_selector: ticket.meta.ref_selector,
            risk_flags: ticket.meta.risk_flags,
            body,
            body_truncated,
            event_count: ticket.events.len(),
            events,
            event_page: QueryPage {
                limit: event_limit,
                returned: event_end - event_start,
                has_more: event_start > 0,
                next_cursor: (event_start > 0).then(|| event_start.to_string()),
                sort: "sequence_desc".to_string(),
                source_limit: None,
                source_truncated: false,
            },
            artifact_count: ticket.artifacts.len(),
            artifacts: ticket
                .artifacts
                .into_iter()
                .map(|artifact| artifact.relative_path.display().to_string())
                .collect(),
            relations,
            linked_objectives,
            implementation_reports,
            assignments,
            current_coder,
            assignment_diagnostics,
            action_eligibility,
            merge_request,
            evidence,
            resolution: ticket
                .resolution
                .map(|resolution| resolution.as_str().to_string()),
            record_source: "sqlite_yoi_ticket".to_string(),
        })
    }
}

impl TicketAuthority for SqliteWorkspaceAuthority {
    fn list_tickets(&self, limit: usize) -> Result<ProjectRecordList<TicketSummary>> {
        let projection = self.ticket_backend.list_workspace_projection(limit)?;
        let items = projection
            .items
            .into_iter()
            .map(|item| {
                let projection =
                    project_ticket_workspace_item(&item.summary, &item.relation_blockers, None);
                let resource_key = item.summary.resource_key.clone().ok_or_else(|| {
                    Error::Store(format!("missing resource key for {}", item.summary.id))
                })?;
                Ok::<_, Error>(TicketSummary {
                    resource_key,
                    id: item.summary.id,
                    title: item.summary.title,
                    state: item.summary.workflow_state.as_str().to_string(),
                    priority: item.summary.priority,
                    updated_at: item.summary.updated_at,
                    queued_by: item.summary.queued_by,
                    queued_at: item.summary.queued_at,
                    workspace_action_priority: workspace_action_priority_name(projection.priority)
                        .to_string(),
                    record_source: "sqlite_yoi_ticket".to_string(),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(ProjectRecordList {
            items,
            invalid_records: Vec::new(),
            record_authority: "workspace-sqlite".to_string(),
        })
    }

    fn list_ticket_page(&self, request: TicketListPageRequest) -> Result<TicketSummaryPage> {
        let limit = request.limit.unwrap_or(30).clamp(1, 100);
        let mut states = request.states;
        states.sort();
        states.dedup();
        let parsed_states = states
            .iter()
            .map(|state| {
                TicketWorkflowState::parse(state).ok_or_else(|| {
                    Error::InvalidRecordId(format!("unsupported ticket state `{state}`"))
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let fingerprint = format!(
            "ticket-summary:v2:sort=priority:states={}",
            states.join(",")
        );
        let after = request
            .cursor
            .as_deref()
            .map(|cursor| parse_ticket_summary_cursor(cursor, &fingerprint))
            .transpose()?;
        let page =
            self.ticket_backend
                .list_workspace_projection_page(SqliteTicketListPageQuery {
                    states: parsed_states,
                    limit,
                    after,
                })?;
        let items = page
            .items
            .into_iter()
            .map(ticket_summary_from_sqlite_item)
            .collect::<Result<Vec<_>>>()?;
        let next_cursor = page
            .next
            .map(|position| make_ticket_summary_cursor(&fingerprint, position));
        Ok(TicketSummaryPage {
            page: QueryPage {
                limit,
                returned: items.len(),
                has_more: page.has_more,
                next_cursor,
                sort: "priority".to_string(),
                source_limit: None,
                source_truncated: false,
            },
            items,
            invalid_records: Vec::new(),
            record_authority: RECORD_SOURCE_WORKSPACE_SQLITE.to_string(),
        })
    }

    fn query_tickets(&self, query: TicketQueryRequest) -> Result<TicketQueryResponse> {
        validate_ticket_query(&query)?;
        let limit = query.limit.unwrap_or(50).clamp(1, 100);
        let sort = normalize_ticket_sort(query.sort.as_deref(), query.query.is_some())?;
        let fingerprint = ticket_query_fingerprint(&query, sort);
        let cursor = query
            .cursor
            .as_deref()
            .map(|cursor| parse_bound_query_cursor(cursor, &fingerprint))
            .transpose()?;
        let candidate_limit = limit.saturating_add(1);
        let candidate_ids =
            self.query_ticket_candidate_ids(&query, sort, cursor.as_ref(), candidate_limit)?;
        let source_truncated = candidate_ids.len() == candidate_limit;
        let mut items = Vec::new();
        for ticket_id in candidate_ids {
            let authoritative = self
                .ticket_backend
                .show(TicketIdOrSlug::Id(ticket_id.clone()))?;
            let summary = ticket_summary_from_ticket(&authoritative)?;
            let authoritative_body = authoritative.document.body.clone();
            let authoritative_events = authoritative.events.clone();
            let detail = self.ticket_detail_from_ticket(
                authoritative,
                TicketShowRequest {
                    event_limit: Some(TICKET_EVENT_LIMIT),
                    event_cursor: None,
                },
            )?;
            if ticket_matches_query(
                &summary,
                &detail,
                authoritative_body.as_str(),
                &authoritative_events,
                &query,
            ) {
                items.push(ticket_query_item(
                    summary,
                    &detail,
                    authoritative_body.as_str(),
                    &authoritative_events,
                    &query,
                ));
            }
        }
        sort_ticket_query_items(&mut items, sort);
        if let Some(cursor) = cursor {
            items.retain(|item| ticket_item_after_cursor(item, sort, &cursor));
        }
        let has_more = items.len() > limit;
        items.truncate(limit);
        let next_cursor = has_more
            .then(|| {
                items
                    .last()
                    .map(|item| make_ticket_cursor(item, sort, &fingerprint))
            })
            .flatten();
        Ok(TicketQueryResponse {
            page: QueryPage {
                limit,
                returned: items.len(),
                has_more,
                next_cursor,
                sort: sort.to_string(),
                source_limit: Some(candidate_limit),
                source_truncated,
            },
            items,
            record_authority: RECORD_SOURCE_WORKSPACE_SQLITE.to_string(),
        })
    }

    fn ticket(&self, id: &str) -> Result<TicketDetail> {
        self.read_ticket_detail(id, TicketShowRequest::default())
    }

    fn show_ticket(&self, id: &str, query: TicketShowRequest) -> Result<TicketDetail> {
        self.read_ticket_detail(id, query)
    }
}

impl ObjectiveAuthority for SqliteWorkspaceAuthority {
    fn list_objectives(&self, limit: usize) -> Result<ProjectRecordList<ObjectiveSummary>> {
        let mut items = Vec::new();
        for record in self.store.list_objectives(&self.workspace_id, limit)? {
            let linked_tickets = self
                .store
                .list_objective_ticket_links(&self.workspace_id, &record.objective_id)?
                .into_iter()
                .map(|link| link.ticket_id)
                .collect::<Vec<_>>();
            items.push(ObjectiveSummary {
                resource_key: self
                    .resource_key(WorkspaceResourceKind::Objective, &record.objective_id)?,
                id: record.objective_id,
                title: record.title,
                state: record.state,
                created_at: Some(record.created_at),
                updated_at: Some(record.updated_at),
                summary: summarize_body(&record.body_md),
                linked_tickets,
                record_source: RECORD_SOURCE_WORKSPACE_SQLITE.to_string(),
            });
        }
        Ok(ProjectRecordList {
            items,
            invalid_records: Vec::new(),
            record_authority: RECORD_SOURCE_WORKSPACE_SQLITE.to_string(),
        })
    }

    fn query_objectives(&self, query: ObjectiveQueryRequest) -> Result<ObjectiveQueryResponse> {
        validate_time_bounds(
            query.updated_after.as_deref(),
            query.updated_before.as_deref(),
        )?;
        let limit = query.limit.unwrap_or(50).clamp(1, 100);
        let sort = normalize_objective_sort(query.sort.as_deref(), query.query.is_some())?;
        let fingerprint = objective_query_fingerprint(&query, sort);
        let cursor = query
            .cursor
            .as_deref()
            .map(|cursor| parse_bound_query_cursor(cursor, &fingerprint))
            .transpose()?;
        let candidate_limit = limit.saturating_add(1);
        let objective_ids =
            self.query_objective_candidate_ids(&query, sort, cursor.as_ref(), candidate_limit)?;
        let source_truncated = objective_ids.len() == candidate_limit;
        let mut items = Vec::new();
        for objective_id in objective_ids {
            let record = self.objective_record(&objective_id)?;
            let linked_tickets = self
                .store
                .list_objective_ticket_links(&self.workspace_id, &objective_id)?
                .into_iter()
                .map(|link| link.ticket_id)
                .collect::<Vec<_>>();
            let body_md = record.body_md.clone();
            let objective = ObjectiveSummary {
                resource_key: self
                    .resource_key(WorkspaceResourceKind::Objective, &record.objective_id)?,
                id: record.objective_id,
                title: record.title,
                state: record.state,
                created_at: Some(record.created_at),
                updated_at: Some(record.updated_at),
                summary: summarize_body(&body_md),
                linked_tickets: linked_tickets.clone(),
                record_source: RECORD_SOURCE_WORKSPACE_SQLITE.to_string(),
            };
            items.push(objective_query_item(
                objective,
                linked_tickets,
                query.query.as_deref(),
                &body_md,
            ));
        }
        sort_objective_query_items(&mut items, sort);
        if let Some(cursor) = cursor {
            items.retain(|item| objective_item_after_cursor(item, sort, &cursor));
        }
        let has_more = items.len() > limit;
        items.truncate(limit);
        let next_cursor = has_more
            .then(|| {
                items
                    .last()
                    .map(|item| make_objective_cursor(item, sort, &fingerprint))
            })
            .flatten();
        Ok(ObjectiveQueryResponse {
            page: QueryPage {
                limit,
                returned: items.len(),
                has_more,
                next_cursor,
                sort: sort.to_string(),
                source_limit: Some(candidate_limit),
                source_truncated,
            },
            items,
            record_authority: RECORD_SOURCE_WORKSPACE_SQLITE.to_string(),
        })
    }

    fn objective(&self, reference: &str) -> Result<ObjectiveDetail> {
        let record = self.objective_record(reference)?;
        self.objective_detail_from_record(record)
    }

    fn show_objective(
        &self,
        reference: &str,
        query: ObjectiveShowRequest,
    ) -> Result<ObjectiveDetail> {
        let mut detail = self.objective(reference)?;
        let objective_id = detail.id.clone();
        let all_events = self
            .store
            .list_objective_events(&self.workspace_id, &objective_id)?;
        let event_limit = query
            .event_limit
            .unwrap_or(TICKET_EVENT_LIMIT)
            .clamp(1, TICKET_EVENT_LIMIT);
        let event_end = query
            .event_cursor
            .as_deref()
            .map(|cursor| parse_offset_cursor(cursor, "event_cursor"))
            .transpose()?
            .unwrap_or(all_events.len())
            .min(all_events.len());
        let event_start = event_end.saturating_sub(event_limit);
        detail.events = all_events[event_start..event_end]
            .iter()
            .map(|event| ObjectiveEventDetail {
                event_ref: event.event_id.clone(),
                kind: event.kind.clone(),
                body: event
                    .body_md
                    .as_deref()
                    .map(|body| truncate_body(body, TICKET_EVENT_BODY_LIMIT).0),
                created_at: event.created_at.clone(),
            })
            .collect();
        detail.event_page = QueryPage {
            limit: event_limit,
            returned: event_end - event_start,
            has_more: event_start > 0,
            next_cursor: (event_start > 0).then(|| event_start.to_string()),
            sort: "sequence_desc".to_string(),
            source_limit: None,
            source_truncated: false,
        };
        Ok(detail)
    }

    fn create_objective(&self, input: ObjectiveCreateInput) -> Result<ObjectiveDetail> {
        validate_objective_title(&input.title)?;
        validate_objective_state(&input.state)?;
        for ticket_id in &input.linked_tickets {
            validate_project_id(ticket_id)?;
        }
        let now = now_rfc3339();
        let objective_id = allocate_record_id(
            unix_epoch_millis_now().map_err(|err| {
                invalid_objective_error(format!("failed to read objective clock: {err}"))
            })?,
            |candidate| {
                self.store
                    .get_objective(&self.workspace_id, candidate)
                    .map(|record| record.is_some())
                    .unwrap_or(true)
            },
        )
        .map_err(|err| {
            invalid_objective_error(format!("failed to allocate objective id: {err}"))
        })?;
        let record = ObjectiveRecord {
            workspace_id: self.workspace_id.clone(),
            objective_id: objective_id.clone(),
            title: input.title.trim().to_string(),
            state: input.state.trim().to_string(),
            body_md: input.body_md,
            created_at: now.clone(),
            updated_at: now.clone(),
        };
        self.store.upsert_objective(&record)?;
        let links = input
            .linked_tickets
            .into_iter()
            .map(|ticket_id| ObjectiveTicketLinkRecord {
                workspace_id: self.workspace_id.clone(),
                objective_id: objective_id.clone(),
                ticket_id,
                kind: "linked".to_string(),
                created_at: now.clone(),
            })
            .collect::<Vec<_>>();
        self.store
            .replace_objective_ticket_links(&self.workspace_id, &objective_id, &links)?;
        self.insert_objective_event(&objective_id, "create", Some(&record.body_md))?;
        self.objective(&objective_id)
    }

    fn edit_objective(
        &self,
        reference: &str,
        input: ObjectiveEditInput,
    ) -> Result<ObjectiveDetail> {
        let mut record = self.objective_record(reference)?;
        let mut changed = false;
        if let Some(title) = input.title {
            validate_objective_title(&title)?;
            let title = title.trim().to_string();
            if title != record.title {
                record.title = title;
                changed = true;
            }
        }
        match (input.old_string, input.new_string) {
            (Some(old_string), Some(new_string)) => {
                if old_string.is_empty() {
                    return Err(invalid_objective_error("old_string must not be empty"));
                }
                let matches = record.body_md.matches(&old_string).count();
                if matches == 0 {
                    return Err(invalid_objective_error(
                        "old_string was not found in objective body",
                    ));
                }
                if matches > 1 && !input.replace_all {
                    return Err(invalid_objective_error(format!(
                        "old_string matched {matches} times; set replace_all = true or provide a unique string"
                    )));
                }
                record.body_md = if input.replace_all {
                    record.body_md.replace(&old_string, &new_string)
                } else {
                    record.body_md.replacen(&old_string, &new_string, 1)
                };
                changed = true;
            }
            (None, None) => {}
            _ => {
                return Err(invalid_objective_error(
                    "old_string and new_string must be provided together",
                ));
            }
        }
        if !changed {
            return Err(invalid_objective_error(
                "objective edit must change title or body",
            ));
        }
        record.updated_at = now_rfc3339();
        let objective_id = record.objective_id.clone();
        self.store.upsert_objective(&record)?;
        self.insert_objective_event(&objective_id, "edit", None)?;
        self.objective(&objective_id)
    }

    fn set_objective_state(&self, reference: &str, state: &str) -> Result<ObjectiveDetail> {
        validate_objective_state(state)?;
        let mut record = self.objective_record(reference)?;
        record.state = state.trim().to_string();
        record.updated_at = now_rfc3339();
        let objective_id = record.objective_id.clone();
        self.store.upsert_objective(&record)?;
        self.insert_objective_event(&objective_id, "state", Some(&record.state))?;
        self.objective(&objective_id)
    }

    fn link_objective_ticket(
        &self,
        objective_reference: &str,
        ticket_reference: &str,
    ) -> Result<ObjectiveDetail> {
        let objective_id = self.objective_record(objective_reference)?.objective_id;
        let ticket_id = self
            .store
            .resolve_resource_reference(
                &self.workspace_id,
                WorkspaceResourceKind::Ticket,
                ticket_reference,
            )?
            .ok_or_else(|| {
                Error::Ticket(ticket::TicketError::NotFound(ticket_reference.to_string()))
            })?;
        let now = now_rfc3339();
        let mut links = self
            .store
            .list_objective_ticket_links(&self.workspace_id, &objective_id)?;
        if !links.iter().any(|link| link.ticket_id == ticket_id) {
            links.push(ObjectiveTicketLinkRecord {
                workspace_id: self.workspace_id.clone(),
                objective_id: objective_id.clone(),
                ticket_id: ticket_id.clone(),
                kind: "linked".to_string(),
                created_at: now,
            });
            self.store
                .replace_objective_ticket_links(&self.workspace_id, &objective_id, &links)?;
            self.insert_objective_event(&objective_id, "link_ticket", Some(&ticket_id))?;
        }
        self.objective(&objective_id)
    }

    fn unlink_objective_ticket(
        &self,
        objective_reference: &str,
        ticket_reference: &str,
    ) -> Result<ObjectiveDetail> {
        let objective_id = self.objective_record(objective_reference)?.objective_id;
        let ticket_id = self
            .store
            .resolve_resource_reference(
                &self.workspace_id,
                WorkspaceResourceKind::Ticket,
                ticket_reference,
            )?
            .ok_or_else(|| {
                Error::Ticket(ticket::TicketError::NotFound(ticket_reference.to_string()))
            })?;
        let mut links = self
            .store
            .list_objective_ticket_links(&self.workspace_id, &objective_id)?;
        let original_len = links.len();
        links.retain(|link| link.ticket_id != ticket_id);
        if links.len() != original_len {
            self.store
                .replace_objective_ticket_links(&self.workspace_id, &objective_id, &links)?;
            self.insert_objective_event(&objective_id, "unlink_ticket", Some(&ticket_id))?;
        }
        self.objective(&objective_id)
    }
}

impl MemoryAuthority for SqliteWorkspaceAuthority {
    fn ensure_memory_document(&self) -> Result<MemoryDocument> {
        let now = now_rfc3339();
        self.store
            .ensure_memory_document(&self.workspace_id, DEFAULT_MEMORY_DOCUMENT_BODY, &now)
            .map(memory_document_from_record)
    }

    fn memory_document(&self) -> Result<MemoryDocument> {
        self.store
            .get_memory_document(&self.workspace_id)?
            .map(memory_document_from_record)
            .ok_or_else(|| Error::Store("workspace memory document is not initialized".to_string()))
    }

    fn update_memory_document(&self, body_md: &str) -> Result<MemoryDocument> {
        let existing = self.ensure_memory_document()?;
        let updated_at = now_rfc3339();
        let record = MemoryDocumentRecord {
            workspace_id: self.workspace_id.clone(),
            body_md: body_md.to_string(),
            created_at: existing.created_at,
            updated_at,
        };
        self.store.upsert_memory_document(&record)?;
        Ok(memory_document_from_record(record))
    }

    fn list_memory_staging_records(&self, limit: usize) -> Result<Vec<MemoryStagingEntry>> {
        self.store
            .list_memory_staging_records(&self.workspace_id, limit)
            .map(|records| {
                records
                    .into_iter()
                    .map(memory_staging_from_record)
                    .collect()
            })
    }

    fn memory_staging_record(&self, candidate_id: &str) -> Result<MemoryStagingEntry> {
        validate_memory_candidate_id(candidate_id)?;
        self.store
            .get_memory_staging_record(&self.workspace_id, candidate_id)?
            .map(memory_staging_from_record)
            .ok_or_else(|| {
                Error::Store(format!("unknown memory staging candidate '{candidate_id}'"))
            })
    }

    fn upsert_memory_staging_record(
        &self,
        candidate_id: &str,
        raw_json: &str,
        source_path: Option<&str>,
    ) -> Result<MemoryStagingEntry> {
        validate_memory_candidate_id(candidate_id)?;
        validate_json_object(raw_json, "raw_json")?;
        let imported_at = now_rfc3339();
        let record = MemoryStagingRecord {
            workspace_id: self.workspace_id.clone(),
            candidate_id: candidate_id.to_string(),
            raw_json: raw_json.to_string(),
            source_path: source_path.map(str::to_string),
            imported_at,
        };
        self.store.upsert_memory_staging_record(&record)?;
        Ok(memory_staging_from_record(record))
    }

    fn close_memory_staging_record(
        &self,
        candidate_id: &str,
        action: &str,
        reason: &str,
        affected_refs_json: &str,
    ) -> Result<MemoryStagingResolution> {
        validate_memory_candidate_id(candidate_id)?;
        validate_non_empty(action, "action")?;
        validate_non_empty(reason, "reason")?;
        validate_json_array(affected_refs_json, "affected_refs_json")?;
        let staging = self
            .store
            .get_memory_staging_record(&self.workspace_id, candidate_id)?
            .ok_or_else(|| {
                Error::Store(format!("unknown memory staging candidate '{candidate_id}'"))
            })?;
        let record = MemoryStagingResolutionRecord {
            workspace_id: self.workspace_id.clone(),
            candidate_id: staging.candidate_id,
            action: action.trim().to_string(),
            reason: reason.trim().to_string(),
            affected_refs_json: affected_refs_json.to_string(),
            staging_raw_json: staging.raw_json,
            source_path: staging.source_path,
            imported_at: staging.imported_at,
            resolved_at: now_rfc3339(),
        };
        self.store.insert_memory_staging_resolution(&record)?;
        self.store
            .delete_memory_staging_record(&self.workspace_id, candidate_id)?;
        Ok(memory_resolution_from_record(record))
    }

    fn list_memory_staging_resolutions(
        &self,
        limit: usize,
    ) -> Result<Vec<MemoryStagingResolution>> {
        self.store
            .list_memory_staging_resolutions(&self.workspace_id, limit)
            .map(|records| {
                records
                    .into_iter()
                    .map(memory_resolution_from_record)
                    .collect()
            })
    }
}

#[derive(Clone, Copy)]
enum TicketQuerySort {
    Relevance,
    UpdatedDesc,
    CreatedDesc,
    Priority,
    Title,
}

impl std::fmt::Display for TicketQuerySort {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Relevance => "relevance",
            Self::UpdatedDesc => "updated_desc",
            Self::CreatedDesc => "created_desc",
            Self::Priority => "priority",
            Self::Title => "title",
        })
    }
}

#[derive(Clone, Copy)]
enum ObjectiveQuerySort {
    Relevance,
    UpdatedDesc,
    CreatedDesc,
    Title,
}

impl std::fmt::Display for ObjectiveQuerySort {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Relevance => "relevance",
            Self::UpdatedDesc => "updated_desc",
            Self::CreatedDesc => "created_desc",
            Self::Title => "title",
        })
    }
}

fn ticket_event_ref(sequence: usize, event: &TicketEvent) -> String {
    event
        .attributes
        .get("event_id")
        .cloned()
        .unwrap_or_else(|| format!("sequence:{sequence}"))
}

fn ticket_event_detail(sequence: usize, event: &TicketEvent) -> TicketEventDetail {
    TicketEventDetail {
        sequence,
        event_ref: ticket_event_ref(sequence, event),
        kind: event.kind.as_str().to_owned(),
        author: event.author.clone(),
        at: event.at.clone(),
        status: event.status.clone(),
        from: event.from.clone(),
        to: event.to.clone(),
        reason: event.reason.clone(),
        state_field: event.state_field.clone(),
        heading: event.heading.clone(),
        body: (!event.body.as_str().is_empty())
            .then(|| truncate_body(event.body.as_str(), TICKET_EVENT_BODY_LIMIT).0),
        attributes: event.attributes.clone(),
        references: event
            .references
            .iter()
            .map(|reference| format!("{}:{}", reference.kind.as_str(), reference.target))
            .collect(),
    }
}

fn ticket_evidence_event(sequence: usize, event: &TicketEvent) -> TicketEvidenceEvent {
    TicketEvidenceEvent {
        event_ref: ticket_event_ref(sequence, event),
        sequence,
        kind: event.kind.as_str().to_owned(),
        at: event.at.clone(),
        author: event.author.clone(),
        excerpt: truncate_body(event.body.as_str(), 512).0,
    }
}

pub(crate) fn merge_request_summary(
    request: MergeRequest,
    current_subject_ref: Option<String>,
) -> TicketMergeRequestSummary {
    let latest_review_request = request.thread.iter().rev().find_map(|event| match event {
        MergeRequestThreadEvent::ReviewRequested(review) => Some(review),
        _ => None,
    });
    let current_review = current_subject_ref
        .as_deref()
        .and_then(|subject_ref| request.effective_review(subject_ref));
    let current_review_request = current_review
        .and_then(|review| {
            request.thread.iter().find_map(|event| match event {
                MergeRequestThreadEvent::ReviewRequested(review_request)
                    if review_request.event_id == review.request_event_id =>
                {
                    Some(review_request)
                }
                _ => None,
            })
        })
        .or_else(|| {
            current_subject_ref.as_deref().and_then(|subject_ref| {
                request.thread.iter().rev().find_map(|event| match event {
                    MergeRequestThreadEvent::ReviewRequested(review)
                        if review.subject_ref == subject_ref =>
                    {
                        Some(review)
                    }
                    _ => None,
                })
            })
        });
    let review_status = match current_review.map(|review| &review.decision) {
        Some(ReviewDecision::Approve) => "approved",
        Some(ReviewDecision::RequestChanges) => "changes_requested",
        None if latest_review_request.is_some() => "pending",
        None => "none",
    }
    .to_string();
    let state = match request.state {
        MergeRequestState::Open => "open",
        MergeRequestState::Merged => "merged",
        MergeRequestState::Closed => "closed",
    }
    .to_string();

    TicketMergeRequestSummary {
        merge_request_id: request.merge_request_id.clone(),
        repository_id: request.repository_id.clone(),
        state,
        review_status,
        selector_from: request.selector_from.clone(),
        selector_to: request.selector_to.clone(),
        updated_at: request.updated_at.to_rfc3339(),
        current_subject_ref,
        review_subject_ref: latest_review_request.map(|review| review.subject_ref.clone()),
        review_requested_at: current_review_request.map(|review| review.created_at.to_rfc3339()),
        review_submitted_at: current_review.map(|review| review.created_at.to_rfc3339()),
        review_excerpt: current_review.map(|review| truncate_body(&review.body, 240).0),
    }
}

fn substantive_item_edit(event: &TicketEvent) -> bool {
    if event.kind.as_str() != "item_edit" {
        return false;
    }
    let Some(changes) = event.attributes.get("changes") else {
        // Legacy item-edit events do not identify changed fields. Treat them as
        // substantive so readiness fails closed rather than accepting a stale review.
        return true;
    };
    changes
        .split(',')
        .map(str::trim)
        .any(|field| matches!(field, "title" | "body" | "target"))
}

fn event_timestamp(event: &TicketEvent) -> Option<DateTime<Utc>> {
    event
        .at
        .as_deref()
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.with_timezone(&Utc))
}

fn ticket_evidence_summary(
    ticket_repository_id: Option<&str>,
    events: &[TicketEvent],
    merge_request: Option<&TicketMergeRequestSummary>,
) -> TicketEvidenceSummary {
    let linked_merge_request = merge_request.filter(|request| {
        request.state == "open"
            && ticket_repository_id
                .is_some_and(|repository_id| repository_id == request.repository_id)
    });
    let has_merge_request = linked_merge_request.is_some();
    let has_current_subject_ref = linked_merge_request.is_some_and(|request| {
        request
            .current_subject_ref
            .as_deref()
            .is_some_and(|subject_ref| !subject_ref.is_empty())
    });
    let has_review_request = linked_merge_request.is_some_and(|request| {
        request
            .review_subject_ref
            .as_deref()
            .is_some_and(|subject_ref| !subject_ref.is_empty())
    });
    let has_commit = has_current_subject_ref;
    let review_status = linked_merge_request.map(|request| request.review_status.clone());
    let approved_current_subject = review_status.as_deref() == Some("approved");
    let unresolved_request_changes = review_status.as_deref() == Some("changes_requested");
    let latest_rescope = events
        .iter()
        .filter(|event| substantive_item_edit(event))
        .last();
    let review_after_rescope = approved_current_subject
        && latest_rescope.is_none_or(|rescope| {
            let Some(rescope_at) = event_timestamp(rescope) else {
                return false;
            };
            let Some(requested_at) = linked_merge_request
                .and_then(|request| request.review_requested_at.as_deref())
                .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
                .map(|value| value.with_timezone(&Utc))
            else {
                return false;
            };
            let Some(reviewed_at) = linked_merge_request
                .and_then(|request| request.review_submitted_at.as_deref())
                .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
                .map(|value| value.with_timezone(&Utc))
            else {
                return false;
            };
            requested_at > rescope_at && reviewed_at > rescope_at
        });

    let mut missing = Vec::new();
    match merge_request {
        None => missing.push("merge_request".to_string()),
        Some(request) if request.state != "open" => missing.push("open_merge_request".to_string()),
        Some(request)
            if ticket_repository_id
                .is_none_or(|repository_id| repository_id != request.repository_id) =>
        {
            missing.push("merge_request_repository".to_string())
        }
        Some(_) => {}
    }
    if !has_current_subject_ref {
        missing.push("current_subject_ref".to_string());
    }
    if !has_commit {
        missing.push("commit".to_string());
    }
    if unresolved_request_changes {
        missing.push("unresolved_request_changes".to_string());
    }
    if !approved_current_subject {
        missing.push("approved_current_subject".to_string());
    }
    if approved_current_subject && !review_after_rescope {
        missing.push("review_after_rescope".to_string());
    }

    TicketEvidenceSummary {
        has_merge_request,
        has_current_subject_ref,
        has_review_request,
        has_commit,
        review_status,
        approved_current_subject,
        review_after_rescope,
        unresolved_request_changes,
        complete_for_integration: missing.is_empty(),
        missing,
    }
}

fn validate_ticket_query(query: &TicketQueryRequest) -> Result<()> {
    for state in &query.states {
        if ticket::TicketWorkflowState::parse(state).is_none() {
            return Err(Error::InvalidRecordId(format!(
                "unsupported Ticket workflow state `{state}`"
            )));
        }
    }
    for evidence in &query.evidence {
        if !matches!(
            evidence.as_str(),
            "merge_request" | "commit" | "approved_review"
        ) {
            return Err(Error::InvalidRecordId(format!(
                "unsupported Ticket evidence filter `{evidence}`"
            )));
        }
    }
    for attention in &query.attention {
        if !matches!(
            attention.as_str(),
            "done_not_closed"
                | "unresolved_review"
                | "missing_commit"
                | "blocked"
                | "unblocked"
                | "ready"
                | "awaiting_review"
                | "unresolved_changes"
                | "stale_after_rescope"
                | "missing_evidence"
        ) {
            return Err(Error::InvalidRecordId(format!(
                "unsupported Ticket attention filter `{attention}`"
            )));
        }
    }
    if let Some(status) = query.review_status.as_deref()
        && !matches!(
            status,
            "none"
                | "pending"
                | "approved"
                | "request_changes"
                | "unresolved_changes"
                | "changes_requested"
        )
    {
        return Err(Error::InvalidRecordId(format!(
            "unsupported review status `{status}`"
        )));
    }
    if let Some(kind) = query.relation_kind.as_deref()
        && !matches!(
            kind,
            "depends_on" | "blocks" | "related" | "supersedes" | "duplicate_of"
        )
    {
        return Err(Error::InvalidRecordId(format!(
            "unsupported Ticket relation kind `{kind}`"
        )));
    }
    validate_time_bounds(
        query.updated_after.as_deref(),
        query.updated_before.as_deref(),
    )
}

fn validate_time_bounds(after: Option<&str>, before: Option<&str>) -> Result<()> {
    let parse = |field: &str, value: &str| {
        chrono::DateTime::parse_from_rfc3339(value)
            .map_err(|_| Error::InvalidRecordId(format!("{field} must be an RFC3339 timestamp")))
    };
    if let Some(after) = after {
        parse("updated_after", after)?;
    }
    if let Some(before) = before {
        parse("updated_before", before)?;
    }
    Ok(())
}

fn normalize_ticket_sort(sort: Option<&str>, has_query: bool) -> Result<TicketQuerySort> {
    match sort.unwrap_or(if has_query {
        "relevance"
    } else {
        "updated_desc"
    }) {
        "relevance" => Ok(TicketQuerySort::Relevance),
        "updated_desc" => Ok(TicketQuerySort::UpdatedDesc),
        "created_desc" => Ok(TicketQuerySort::CreatedDesc),
        "priority" => Ok(TicketQuerySort::Priority),
        "title" => Ok(TicketQuerySort::Title),
        other => Err(Error::InvalidRecordId(format!(
            "unsupported Ticket query sort `{other}`"
        ))),
    }
}

fn normalize_objective_sort(sort: Option<&str>, has_query: bool) -> Result<ObjectiveQuerySort> {
    match sort.unwrap_or(if has_query {
        "relevance"
    } else {
        "updated_desc"
    }) {
        "relevance" => Ok(ObjectiveQuerySort::Relevance),
        "updated_desc" => Ok(ObjectiveQuerySort::UpdatedDesc),
        "created_desc" => Ok(ObjectiveQuerySort::CreatedDesc),
        "title" => Ok(ObjectiveQuerySort::Title),
        other => Err(Error::InvalidRecordId(format!(
            "unsupported Objective query sort `{other}`"
        ))),
    }
}

fn parse_offset_cursor(cursor: &str, field: &str) -> Result<usize> {
    cursor
        .parse::<usize>()
        .map_err(|_| Error::InvalidRecordId(format!("invalid {field}")))
}

fn make_bound_query_cursor(fingerprint: &str, key: &str, id: &str) -> String {
    make_query_cursor(&format!("{fingerprint}\n{key}"), id)
}

fn parse_bound_query_cursor(value: &str, fingerprint: &str) -> Result<(String, String)> {
    let (key, id) = parse_query_cursor(value)?;
    let prefix = format!("{fingerprint}\n");
    let key = key.strip_prefix(&prefix).ok_or_else(|| {
        Error::InvalidRecordId("cursor does not match the current filters or sort".to_string())
    })?;
    Ok((key.to_string(), id))
}

fn ticket_query_fingerprint(query: &TicketQueryRequest, sort: TicketQuerySort) -> String {
    let mut query = query.clone();
    query.cursor = None;
    query.limit = None;
    query.sort = Some(sort.to_string());
    serde_json::to_string(&query).expect("ticket query fingerprint must serialize")
}

fn objective_query_fingerprint(query: &ObjectiveQueryRequest, sort: ObjectiveQuerySort) -> String {
    let mut query = query.clone();
    query.cursor = None;
    query.limit = None;
    query.sort = Some(sort.to_string());
    serde_json::to_string(&query).expect("objective query fingerprint must serialize")
}

fn make_query_cursor(key: &str, id: &str) -> String {
    format!("v1:{}:{key}{id}", key.len())
}

fn parse_query_cursor(cursor: &str) -> Result<(String, String)> {
    let rest = cursor
        .strip_prefix("v1:")
        .ok_or_else(|| Error::InvalidRecordId("invalid query cursor".to_string()))?;
    let (length, value) = rest
        .split_once(':')
        .ok_or_else(|| Error::InvalidRecordId("invalid query cursor".to_string()))?;
    let length = length
        .parse::<usize>()
        .map_err(|_| Error::InvalidRecordId("invalid query cursor".to_string()))?;
    if length > value.len() || !value.is_char_boundary(length) {
        return Err(Error::InvalidRecordId("invalid query cursor".to_string()));
    }
    Ok((value[..length].to_string(), value[length..].to_string()))
}

fn ticket_evidence_matches(evidence: &TicketEvidenceSummary, filter: &str) -> bool {
    match filter {
        "merge_request" => evidence.has_merge_request,
        "commit" => evidence.has_commit,
        "approved_review" => evidence.approved_current_subject,
        _ => false,
    }
}

fn ticket_attention_matches(
    state: &str,
    evidence: &TicketEvidenceSummary,
    is_blocked: bool,
    authoritative_events: &[TicketEvent],
    attention: &str,
) -> bool {
    match attention {
        "done_not_closed" => state == "done",
        "unresolved_review" => evidence.unresolved_request_changes,
        "missing_commit" => !evidence.has_commit,
        "blocked" => is_blocked,
        "unblocked" => !is_blocked,
        "ready" => state == "ready" && !is_blocked,
        "awaiting_review" => {
            evidence.has_review_request && evidence.review_status.as_deref() == Some("pending")
        }
        "unresolved_changes" => evidence.unresolved_request_changes,
        "stale_after_rescope" => {
            authoritative_events.iter().any(substantive_item_edit) && !evidence.review_after_rescope
        }
        "missing_evidence" => !evidence.complete_for_integration,
        _ => false,
    }
}

fn ticket_matches_query(
    summary: &TicketSummary,
    detail: &TicketDetail,
    authoritative_body: &str,
    authoritative_events: &[TicketEvent],
    query: &TicketQueryRequest,
) -> bool {
    if !query.states.is_empty() && !query.states.iter().any(|state| state == &summary.state) {
        return false;
    }
    if query
        .updated_after
        .as_ref()
        .is_some_and(|after| summary.updated_at.as_deref().unwrap_or("") <= after.as_str())
        || query
            .updated_before
            .as_ref()
            .is_some_and(|before| summary.updated_at.as_deref().unwrap_or("") >= before.as_str())
    {
        return false;
    }
    if !query.event_kinds.is_empty()
        && !authoritative_events.iter().any(|event| {
            query
                .event_kinds
                .iter()
                .any(|kind| kind == event.kind.as_str())
        })
    {
        return false;
    }
    if let Some(review_status) = &query.review_status {
        let matches = match review_status.as_str() {
            "none" => matches!(
                detail.evidence.review_status.as_deref(),
                None | Some("none")
            ),
            "request_changes" | "unresolved_changes" => {
                detail.evidence.review_status.as_deref() == Some("changes_requested")
            }
            status => detail.evidence.review_status.as_deref() == Some(status),
        };
        if !matches {
            return false;
        }
    }
    if !query
        .evidence
        .iter()
        .all(|filter| ticket_evidence_matches(&detail.evidence, filter))
    {
        return false;
    }
    if !query.attention.iter().all(|attention| {
        ticket_attention_matches(
            &summary.state,
            &detail.evidence,
            !detail.relations.blockers.is_empty(),
            authoritative_events,
            attention,
        )
    }) {
        return false;
    }
    if query.linked_objective_id.as_ref().is_some_and(|id| {
        !detail
            .linked_objectives
            .iter()
            .any(|objective| &objective.id == id)
    }) {
        return false;
    }
    if (query.related_ticket_id.is_some() || query.relation_kind.is_some())
        && !detail.relations.outgoing.iter().any(|relation| {
            query
                .related_ticket_id
                .as_ref()
                .is_none_or(|ticket_id| &relation.target == ticket_id)
                && query
                    .relation_kind
                    .as_ref()
                    .is_none_or(|kind| &relation.kind == kind)
        })
        && !detail.relations.incoming.iter().any(|relation| {
            query
                .related_ticket_id
                .as_ref()
                .is_none_or(|ticket_id| &relation.source_ticket == ticket_id)
                && query
                    .relation_kind
                    .as_ref()
                    .is_none_or(|kind| &relation.forward_kind == kind)
        })
        && !detail.relations.blockers.iter().any(|relation| {
            query
                .related_ticket_id
                .as_ref()
                .is_none_or(|ticket_id| &relation.blocking_ticket == ticket_id)
                && query
                    .relation_kind
                    .as_ref()
                    .is_none_or(|kind| &relation.relation_kind == kind)
        })
        && !detail.relations.notices.iter().any(|relation| {
            query
                .related_ticket_id
                .as_ref()
                .is_none_or(|ticket_id| &relation.related_ticket == ticket_id)
                && query
                    .relation_kind
                    .as_ref()
                    .is_none_or(|kind| &relation.kind == kind)
        })
    {
        return false;
    }
    query.query.as_ref().is_none_or(|text| {
        let needle = text.to_lowercase();
        summary.title.to_lowercase().contains(&needle)
            || authoritative_body.to_lowercase().contains(&needle)
            || authoritative_events
                .iter()
                .any(|event| event.body.as_str().to_lowercase().contains(&needle))
    })
}

fn ticket_query_item(
    summary: TicketSummary,
    detail: &TicketDetail,
    authoritative_body: &str,
    authoritative_events: &[TicketEvent],
    query: &TicketQueryRequest,
) -> TicketQueryItem {
    let mut matched_fields = Vec::new();
    let mut snippet = None;
    let mut matching_event = None;
    if let Some(text) = query.query.as_ref() {
        let needle = text.to_lowercase();
        if summary.title.to_lowercase().contains(&needle) {
            matched_fields.push("title".to_string());
            snippet = Some(summary.title.clone());
        }
        if authoritative_body.to_lowercase().contains(&needle) {
            matched_fields.push("body".to_string());
            snippet.get_or_insert_with(|| matching_snippet(authoritative_body, text));
        }
        if let Some((sequence, event)) = authoritative_events
            .iter()
            .enumerate()
            .find(|(_, event)| event.body.as_str().to_lowercase().contains(&needle))
        {
            matched_fields.push("event".to_string());
            let event_snippet = matching_snippet(event.body.as_str(), text);
            let mut evidence = ticket_evidence_event(sequence, event);
            evidence.excerpt = event_snippet.clone();
            matching_event = Some(evidence);
            snippet = Some(event_snippet);
        }
    }
    TicketQueryItem {
        id: summary.id,
        resource_key: summary.resource_key,
        title: summary.title,
        state: summary.state,
        readiness: detail.readiness.clone(),
        priority: summary.priority,
        created_at: detail.created_at.clone(),
        updated_at: summary.updated_at,
        item_revision: detail.item_revision.clone(),
        workspace_action_priority: summary.workspace_action_priority,
        matched_fields,
        snippet: snippet.map(|value| truncate_body(&value, 512).0),
        matching_event,
        linked_objective_ids: detail
            .linked_objectives
            .iter()
            .map(|objective| objective.id.clone())
            .collect(),
        relation_count: detail.relations.outgoing.len() + detail.relations.incoming.len(),
        blocker_count: detail.relations.blockers.len(),
        unresolved_blocker_count: detail.relations.blockers.len(),
        unresolved_review_count: usize::from(detail.evidence.unresolved_request_changes),
        evidence: detail.evidence.clone(),
        merge_request: detail.merge_request.clone(),
    }
}

fn matching_snippet(body: &str, text: &str) -> String {
    let lower = body.to_lowercase();
    let start = lower
        .find(&text.to_lowercase())
        .unwrap_or(0)
        .saturating_sub(96);
    let start = body.floor_char_boundary(start);
    let end = body.ceil_char_boundary((start + 320).min(body.len()));
    body[start..end].to_string()
}

fn ticket_match_rank(item: &TicketQueryItem) -> usize {
    if item.matched_fields.iter().any(|field| field == "title") {
        0
    } else if item.matched_fields.iter().any(|field| field == "body") {
        1
    } else if item.matched_fields.iter().any(|field| field == "event") {
        2
    } else {
        3
    }
}

fn ticket_sort_key(item: &TicketQueryItem, sort: TicketQuerySort) -> String {
    match sort {
        TicketQuerySort::Relevance => format!(
            "{}|{}",
            ticket_match_rank(item),
            item.updated_at.as_deref().unwrap_or("")
        ),
        TicketQuerySort::UpdatedDesc => item.updated_at.clone().unwrap_or_default(),
        TicketQuerySort::CreatedDesc => item.created_at.clone().unwrap_or_default(),
        TicketQuerySort::Priority => format!(
            "{}|{}",
            match item.workspace_action_priority.as_str() {
                "ready_for_queue" => 0,
                "active_work" => 1,
                _ => 2,
            },
            item.updated_at.as_deref().unwrap_or("")
        ),
        TicketQuerySort::Title => item.title.to_lowercase(),
    }
}

fn sort_ticket_query_items(items: &mut [TicketQueryItem], sort: TicketQuerySort) {
    items.sort_by(|left, right| match sort {
        TicketQuerySort::Relevance => ticket_match_rank(left)
            .cmp(&ticket_match_rank(right))
            .then_with(|| right.updated_at.cmp(&left.updated_at))
            .then_with(|| left.id.cmp(&right.id)),
        TicketQuerySort::UpdatedDesc => right
            .updated_at
            .cmp(&left.updated_at)
            .then_with(|| left.id.cmp(&right.id)),
        TicketQuerySort::CreatedDesc => right
            .created_at
            .cmp(&left.created_at)
            .then_with(|| left.id.cmp(&right.id)),
        TicketQuerySort::Priority => {
            let rank = |item: &TicketQueryItem| match item.workspace_action_priority.as_str() {
                "ready_for_queue" => 0,
                "active_work" => 1,
                _ => 2,
            };
            rank(left)
                .cmp(&rank(right))
                .then_with(|| right.updated_at.cmp(&left.updated_at))
                .then_with(|| left.id.cmp(&right.id))
        }
        TicketQuerySort::Title => left
            .title
            .to_lowercase()
            .cmp(&right.title.to_lowercase())
            .then_with(|| left.id.cmp(&right.id)),
    });
}

fn make_ticket_cursor(item: &TicketQueryItem, sort: TicketQuerySort, fingerprint: &str) -> String {
    make_bound_query_cursor(fingerprint, &ticket_sort_key(item, sort), &item.id)
}

fn ticket_item_after_cursor(
    item: &TicketQueryItem,
    sort: TicketQuerySort,
    cursor: &(String, String),
) -> bool {
    let key = ticket_sort_key(item, sort);
    match sort {
        TicketQuerySort::UpdatedDesc | TicketQuerySort::CreatedDesc => {
            key < cursor.0 || (key == cursor.0 && item.id > cursor.1)
        }
        TicketQuerySort::Title => key > cursor.0 || (key == cursor.0 && item.id > cursor.1),
        TicketQuerySort::Relevance | TicketQuerySort::Priority => {
            let (rank, updated) = key.split_once('|').unwrap_or(("9", ""));
            let (cursor_rank, cursor_updated) = cursor.0.split_once('|').unwrap_or(("9", ""));
            rank > cursor_rank
                || (rank == cursor_rank
                    && (updated < cursor_updated
                        || (updated == cursor_updated && item.id > cursor.1)))
        }
    }
}

fn objective_query_item(
    objective: ObjectiveSummary,
    linked_tickets: Vec<String>,
    text: Option<&str>,
    body_md: &str,
) -> ObjectiveQueryItem {
    let mut matched_fields = Vec::new();
    let mut snippet = None;
    if let Some(text) = text {
        let needle = text.to_lowercase();
        if objective.title.to_lowercase().contains(&needle) {
            matched_fields.push("title".to_string());
            snippet = Some(objective.title.clone());
        }
        if body_md.to_lowercase().contains(&needle) {
            matched_fields.push("body".to_string());
            snippet.get_or_insert_with(|| matching_snippet(body_md, text));
        }
    }
    ObjectiveQueryItem {
        id: objective.id,
        resource_key: objective.resource_key,
        title: objective.title,
        state: objective.state,
        created_at: objective.created_at,
        updated_at: objective.updated_at,
        matched_fields,
        snippet,
        linked_ticket_count: linked_tickets.len(),
        linked_tickets,
    }
}

fn objective_match_rank(item: &ObjectiveQueryItem) -> usize {
    if item.matched_fields.iter().any(|field| field == "title") {
        0
    } else if item.matched_fields.iter().any(|field| field == "body") {
        1
    } else {
        2
    }
}

fn objective_sort_key(item: &ObjectiveQueryItem, sort: ObjectiveQuerySort) -> String {
    match sort {
        ObjectiveQuerySort::Relevance => format!(
            "{}|{}",
            objective_match_rank(item),
            item.updated_at.as_deref().unwrap_or("")
        ),
        ObjectiveQuerySort::UpdatedDesc => item.updated_at.clone().unwrap_or_default(),
        ObjectiveQuerySort::CreatedDesc => item.created_at.clone().unwrap_or_default(),
        ObjectiveQuerySort::Title => item.title.to_lowercase(),
    }
}

fn sort_objective_query_items(items: &mut [ObjectiveQueryItem], sort: ObjectiveQuerySort) {
    items.sort_by(|left, right| match sort {
        ObjectiveQuerySort::Relevance => objective_match_rank(left)
            .cmp(&objective_match_rank(right))
            .then_with(|| right.updated_at.cmp(&left.updated_at))
            .then_with(|| left.id.cmp(&right.id)),
        ObjectiveQuerySort::UpdatedDesc => right
            .updated_at
            .cmp(&left.updated_at)
            .then_with(|| left.id.cmp(&right.id)),
        ObjectiveQuerySort::CreatedDesc => right
            .created_at
            .cmp(&left.created_at)
            .then_with(|| left.id.cmp(&right.id)),
        ObjectiveQuerySort::Title => left
            .title
            .to_lowercase()
            .cmp(&right.title.to_lowercase())
            .then_with(|| left.id.cmp(&right.id)),
    });
}

fn make_objective_cursor(
    item: &ObjectiveQueryItem,
    sort: ObjectiveQuerySort,
    fingerprint: &str,
) -> String {
    make_bound_query_cursor(fingerprint, &objective_sort_key(item, sort), &item.id)
}

fn objective_item_after_cursor(
    item: &ObjectiveQueryItem,
    sort: ObjectiveQuerySort,
    cursor: &(String, String),
) -> bool {
    let key = objective_sort_key(item, sort);
    match sort {
        ObjectiveQuerySort::UpdatedDesc | ObjectiveQuerySort::CreatedDesc => {
            key < cursor.0 || (key == cursor.0 && item.id > cursor.1)
        }
        ObjectiveQuerySort::Relevance => {
            let (rank, updated) = key.split_once('|').unwrap_or(("9", ""));
            let (cursor_rank, cursor_updated) = cursor.0.split_once('|').unwrap_or(("9", ""));
            rank > cursor_rank
                || (rank == cursor_rank
                    && (updated < cursor_updated
                        || (updated == cursor_updated && item.id > cursor.1)))
        }
        ObjectiveQuerySort::Title => key > cursor.0 || (key == cursor.0 && item.id > cursor.1),
    }
}

fn now_rfc3339() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

fn validate_memory_candidate_id(candidate_id: &str) -> Result<()> {
    validate_non_empty(candidate_id, "candidate_id")
}

fn validate_non_empty(value: &str, field: &str) -> Result<()> {
    if value.trim().is_empty() {
        Err(Error::Store(format!("memory {field} must not be empty")))
    } else {
        Ok(())
    }
}

fn validate_objective_title(title: &str) -> Result<()> {
    if title.trim().is_empty() {
        Err(invalid_objective_error("objective title must not be empty"))
    } else {
        Ok(())
    }
}

fn validate_objective_state(state: &str) -> Result<()> {
    if state.trim().is_empty() {
        Err(invalid_objective_error("objective state must not be empty"))
    } else {
        Ok(())
    }
}

fn invalid_objective_error(message: impl Into<String>) -> Error {
    Error::RuntimeOperationFailed {
        runtime_id: "workspace-authority".to_string(),
        code: "invalid_objective_request".to_string(),
        message: message.into(),
    }
}

fn unknown_objective_error(id: &str) -> Error {
    Error::RuntimeOperationFailed {
        runtime_id: "workspace-authority".to_string(),
        code: "unknown_objective".to_string(),
        message: format!("unknown objective `{id}`"),
    }
}

fn validate_json_object(raw_json: &str, field: &str) -> Result<()> {
    let value: serde_json::Value = serde_json::from_str(raw_json)
        .map_err(|err| Error::Store(format!("memory {field} must be valid JSON: {err}")))?;
    if value.is_object() {
        Ok(())
    } else {
        Err(Error::Store(format!(
            "memory {field} must be a JSON object"
        )))
    }
}

fn validate_json_array(raw_json: &str, field: &str) -> Result<()> {
    let value: serde_json::Value = serde_json::from_str(raw_json)
        .map_err(|err| Error::Store(format!("memory {field} must be valid JSON: {err}")))?;
    if value.is_array() {
        Ok(())
    } else {
        Err(Error::Store(format!("memory {field} must be a JSON array")))
    }
}

fn memory_document_from_record(record: MemoryDocumentRecord) -> MemoryDocument {
    MemoryDocument {
        body_md: record.body_md,
        created_at: record.created_at,
        updated_at: record.updated_at,
        record_source: RECORD_SOURCE_WORKSPACE_SQLITE.to_string(),
    }
}

fn memory_staging_from_record(record: MemoryStagingRecord) -> MemoryStagingEntry {
    MemoryStagingEntry {
        candidate_id: record.candidate_id,
        raw_json: record.raw_json,
        source_path: record.source_path,
        imported_at: record.imported_at,
        record_source: RECORD_SOURCE_WORKSPACE_SQLITE.to_string(),
    }
}

fn memory_resolution_from_record(record: MemoryStagingResolutionRecord) -> MemoryStagingResolution {
    MemoryStagingResolution {
        candidate_id: record.candidate_id,
        action: record.action,
        reason: record.reason,
        affected_refs_json: record.affected_refs_json,
        staging_raw_json: record.staging_raw_json,
        source_path: record.source_path,
        imported_at: record.imported_at,
        resolved_at: record.resolved_at,
        record_source: RECORD_SOURCE_WORKSPACE_SQLITE.to_string(),
    }
}

fn ticket_summary_from_ticket(ticket: &ticket::Ticket) -> Result<TicketSummary> {
    let summary = ticket::TicketSummary {
        id: ticket.meta.id.clone(),
        resource_key: ticket.meta.resource_key.clone(),
        slug: ticket.meta.slug.clone(),
        title: ticket.meta.title.clone(),
        status: ticket.meta.status.clone(),
        kind: ticket.meta.kind.clone(),
        priority: ticket.meta.priority.clone(),
        labels: ticket.meta.labels.clone(),
        readiness: ticket.meta.readiness.clone(),
        workflow_state: ticket.meta.workflow_state,
        workflow_state_explicit: ticket.meta.workflow_state_explicit,
        queued_by: ticket.meta.queued_by.clone(),
        queued_at: ticket.meta.queued_at.clone(),
        updated_at: ticket.meta.updated_at.clone(),
    };
    ticket_summary_from_sqlite_item(SqliteTicketListItem {
        summary,
        relation_blockers: ticket.relations.blockers.clone(),
    })
}

fn ticket_summary_from_sqlite_item(item: SqliteTicketListItem) -> Result<TicketSummary> {
    let projection = project_ticket_workspace_item(&item.summary, &item.relation_blockers, None);
    let resource_key = item
        .summary
        .resource_key
        .clone()
        .ok_or_else(|| Error::Store(format!("missing resource key for {}", item.summary.id)))?;
    Ok(TicketSummary {
        resource_key,
        id: item.summary.id,
        title: item.summary.title,
        state: item.summary.workflow_state.as_str().to_string(),
        priority: item.summary.priority,
        updated_at: item.summary.updated_at,
        queued_by: item.summary.queued_by,
        queued_at: item.summary.queued_at,
        workspace_action_priority: workspace_action_priority_name(projection.priority).to_string(),
        record_source: "sqlite_yoi_ticket".to_string(),
    })
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct TicketSummaryCursorEnvelope {
    version: u8,
    fingerprint: String,
    position: SqliteTicketListCursor,
}

fn make_ticket_summary_cursor(fingerprint: &str, position: SqliteTicketListCursor) -> String {
    make_query_cursor(
        &serde_json::to_string(&TicketSummaryCursorEnvelope {
            version: 1,
            fingerprint: fingerprint.to_string(),
            position,
        })
        .expect("ticket summary cursor must serialize"),
        "",
    )
}

fn parse_ticket_summary_cursor(
    value: &str,
    expected_fingerprint: &str,
) -> Result<SqliteTicketListCursor> {
    let (encoded, trailing) = parse_query_cursor(value)?;
    if !trailing.is_empty() {
        return Err(Error::InvalidRecordId("cursor is malformed".to_string()));
    }
    let cursor: TicketSummaryCursorEnvelope = serde_json::from_str(&encoded)
        .map_err(|_| Error::InvalidRecordId("cursor is malformed".to_string()))?;
    if cursor.version != 1 || cursor.fingerprint != expected_fingerprint {
        return Err(Error::InvalidRecordId(
            "cursor does not match the current ticket filters or sort".to_string(),
        ));
    }
    Ok(cursor.position)
}

fn workspace_action_priority_name(priority: TicketWorkspaceActionPriority) -> &'static str {
    match priority {
        TicketWorkspaceActionPriority::ReadyForQueue => "ready_for_queue",
        TicketWorkspaceActionPriority::ActiveWork => "active_work",
        TicketWorkspaceActionPriority::Background => "background",
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::Path;

    use super::*;
    use crate::store::{ObjectiveRecord, ObjectiveTicketLinkRecord, WorkspaceRecord};

    fn actor() -> merge_request::WorkerIdentity {
        merge_request::WorkerIdentity {
            runtime_id: "runtime-1".to_string(),
            worker_id: "worker-1".to_string(),
        }
    }

    fn reviewed_merge_request(decision: ReviewDecision, revoked: bool) -> MergeRequest {
        let requested_at = DateTime::parse_from_rfc3339("2026-01-01T00:03:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let reviewed_at = DateTime::parse_from_rfc3339("2026-01-01T00:04:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let reviewer = actor();
        let mut thread = vec![
            MergeRequestThreadEvent::ReviewRequested(merge_request::ReviewRequestedEvent {
                event_id: "request-1".to_string(),
                sequence: 1,
                subject_ref: "commit-1".to_string(),
                requested_by: reviewer.clone(),
                reviewer: reviewer.clone(),
                created_at: requested_at,
            }),
            MergeRequestThreadEvent::Review(merge_request::ReviewEvent {
                event_id: "review-1".to_string(),
                sequence: 2,
                request_event_id: "request-1".to_string(),
                subject_ref: "commit-1".to_string(),
                decision,
                body: "review body".to_string(),
                findings: Vec::new(),
                reviewer: reviewer.clone(),
                created_at: reviewed_at,
            }),
        ];
        if revoked {
            thread.push(MergeRequestThreadEvent::ReviewRevoked(
                merge_request::ReviewRevokedEvent {
                    event_id: "revoke-1".to_string(),
                    sequence: 3,
                    review_event_id: "review-1".to_string(),
                    subject_ref: "commit-1".to_string(),
                    reason: "stale".to_string(),
                    revoked_by: reviewer,
                    created_at: reviewed_at + chrono::Duration::minutes(1),
                },
            ));
        }
        MergeRequest {
            workspace_id: "workspace-1".to_string(),
            merge_request_id: "mr-1".to_string(),
            repository_id: "main".to_string(),
            state: MergeRequestState::Open,
            selector_from: Some("work/ticket-1".to_string()),
            selector_to: "orchestration".to_string(),
            ticket_ids: vec!["ticket-1".to_string()],
            thread,
            created_at: requested_at,
            updated_at: reviewed_at,
        }
    }

    fn ticket_event(kind: &str, at: &str, changes: Option<&str>) -> TicketEvent {
        let mut attributes = BTreeMap::new();
        if let Some(changes) = changes {
            attributes.insert("changes".to_string(), changes.to_string());
        }
        TicketEvent {
            kind: ticket::TicketEventKind::Other(kind.to_string()),
            author: Some("coder".to_string()),
            at: Some(at.to_string()),
            status: None,
            from: None,
            to: None,
            reason: None,
            state_field: None,
            heading: None,
            body: ticket::MarkdownText::new(kind),
            attributes,
            references: Vec::new(),
        }
    }

    #[test]
    fn merge_request_summary_uses_the_provider_resolved_current_subject() {
        let approved = merge_request_summary(
            reviewed_merge_request(ReviewDecision::Approve, false),
            Some("commit-1".to_string()),
        );
        assert_eq!(approved.review_status, "approved");
        assert_eq!(approved.current_subject_ref.as_deref(), Some("commit-1"));
        assert_eq!(
            approved.review_requested_at.as_deref(),
            Some("2026-01-01T00:03:00+00:00")
        );

        let moved = merge_request_summary(
            reviewed_merge_request(ReviewDecision::Approve, false),
            Some("commit-2".to_string()),
        );
        assert_eq!(moved.review_status, "pending");
        assert_eq!(moved.current_subject_ref.as_deref(), Some("commit-2"));
        assert_eq!(moved.review_subject_ref.as_deref(), Some("commit-1"));
        assert_eq!(moved.review_submitted_at, None);
    }

    #[test]
    fn ticket_readiness_requires_current_unrevoked_approval_without_a_report() {
        let approved = merge_request_summary(
            reviewed_merge_request(ReviewDecision::Approve, false),
            Some("commit-1".to_string()),
        );
        let evidence = ticket_evidence_summary(Some("main"), &[], Some(&approved));
        assert!(evidence.complete_for_integration);
        assert!(evidence.approved_current_subject);
        assert!(evidence.review_after_rescope);
        assert!(evidence.has_commit);

        let with_audit_events = ticket_evidence_summary(
            Some("main"),
            &[
                ticket_event("comment", "2026-01-01T00:05:00Z", None),
                ticket_event("implementation_report", "2026-01-01T00:06:00Z", None),
            ],
            Some(&approved),
        );
        assert!(with_audit_events.complete_for_integration);

        let revoked = merge_request_summary(
            reviewed_merge_request(ReviewDecision::Approve, true),
            Some("commit-1".to_string()),
        );
        let evidence = ticket_evidence_summary(Some("main"), &[], Some(&revoked));
        assert!(!evidence.approved_current_subject);
        assert!(!evidence.complete_for_integration);

        let changes = merge_request_summary(
            reviewed_merge_request(ReviewDecision::RequestChanges, false),
            Some("commit-1".to_string()),
        );
        let evidence = ticket_evidence_summary(Some("main"), &[], Some(&changes));
        assert!(evidence.unresolved_request_changes);
        assert!(!evidence.complete_for_integration);
    }

    #[test]
    fn ticket_readiness_fails_closed_for_missing_or_closed_current_merge_request() {
        let unresolved =
            merge_request_summary(reviewed_merge_request(ReviewDecision::Approve, false), None);
        let evidence = ticket_evidence_summary(Some("main"), &[], Some(&unresolved));
        assert!(!evidence.has_current_subject_ref);
        assert!(!evidence.has_commit);
        assert!(!evidence.complete_for_integration);

        let mut closed_request = reviewed_merge_request(ReviewDecision::Approve, false);
        closed_request.state = MergeRequestState::Closed;
        let closed = merge_request_summary(closed_request, Some("commit-1".to_string()));
        let evidence = ticket_evidence_summary(Some("main"), &[], Some(&closed));
        assert!(!evidence.has_merge_request);
        assert!(!evidence.complete_for_integration);
        assert!(evidence.missing.contains(&"open_merge_request".to_string()));
    }

    #[test]
    fn ticket_readiness_requires_request_and_approval_after_substantive_rescope() {
        let approved = merge_request_summary(
            reviewed_merge_request(ReviewDecision::Approve, false),
            Some("commit-1".to_string()),
        );
        let fresh = ticket_evidence_summary(
            Some("main"),
            &[ticket_event(
                "item_edit",
                "2026-01-01T00:02:00Z",
                Some("body"),
            )],
            Some(&approved),
        );
        assert!(fresh.review_after_rescope);
        assert!(fresh.complete_for_integration);

        let stale = ticket_evidence_summary(
            Some("main"),
            &[ticket_event(
                "item_edit",
                "2026-01-01T00:05:00Z",
                Some("title"),
            )],
            Some(&approved),
        );
        assert!(!stale.review_after_rescope);
        assert!(!stale.complete_for_integration);
        assert!(stale.missing.contains(&"review_after_rescope".to_string()));

        let metadata_only = ticket_evidence_summary(
            Some("main"),
            &[ticket_event(
                "item_edit",
                "2026-01-01T00:05:00Z",
                Some("formatter"),
            )],
            Some(&approved),
        );
        assert!(metadata_only.complete_for_integration);
    }

    #[test]
    fn ticket_query_filters_map_to_current_merge_request_evidence() {
        let approved_summary = merge_request_summary(
            reviewed_merge_request(ReviewDecision::Approve, false),
            Some("commit-1".to_string()),
        );
        let approved = ticket_evidence_summary(Some("main"), &[], Some(&approved_summary));
        assert!(ticket_evidence_matches(&approved, "merge_request"));
        assert!(ticket_evidence_matches(&approved, "commit"));
        assert!(ticket_evidence_matches(&approved, "approved_review"));
        assert!(!ticket_attention_matches(
            "inprogress",
            &approved,
            false,
            &[],
            "missing_evidence",
        ));

        let pending_summary = merge_request_summary(
            reviewed_merge_request(ReviewDecision::Approve, false),
            Some("commit-2".to_string()),
        );
        let pending = ticket_evidence_summary(Some("main"), &[], Some(&pending_summary));
        assert!(ticket_attention_matches(
            "inprogress",
            &pending,
            false,
            &[],
            "awaiting_review",
        ));
        assert!(!ticket_evidence_matches(&pending, "approved_review"));

        let stale_events = vec![ticket_event(
            "item_edit",
            "2026-01-01T00:05:00Z",
            Some("target"),
        )];
        let stale = ticket_evidence_summary(Some("main"), &stale_events, Some(&approved_summary));
        assert!(ticket_attention_matches(
            "inprogress",
            &stale,
            false,
            &stale_events,
            "stale_after_rescope",
        ));
        assert!(ticket_attention_matches(
            "inprogress",
            &stale,
            false,
            &stale_events,
            "missing_evidence",
        ));
    }

    #[tokio::test]
    async fn sqlite_workspace_authority_reads_sqlite_records_without_filesystem_authority() {
        let dir = tempfile::tempdir().unwrap();
        write_ticket(dir.path(), "00000000001J2", "Read bridge", "ready");
        write_ticket(dir.path(), "00000000001J5", "Second ticket", "queued");
        write_ticket(dir.path(), "00000000001J6", "Third ticket", "planning");
        let db_path = dir.path().join("workspace.db");
        let store = SqliteWorkspaceStore::open(&db_path).unwrap();
        store
            .upsert_workspace(&WorkspaceRecord {
                workspace_id: "workspace-test".to_string(),
                owner_account_id: None,
                display_name: "Workspace Test".to_string(),
                state: "active".to_string(),
                created_at: "2026-01-01T00:00:00Z".to_string(),
                updated_at: "2026-01-01T00:00:00Z".to_string(),
            })
            .await
            .unwrap();
        SqliteTicketBackend::open(&db_path, "workspace-test")
            .unwrap()
            .import_from_local_backend(&ticket::LocalTicketBackend::new(
                dir.path().join(".yoi/tickets"),
            ))
            .unwrap();
        rusqlite::Connection::open(&db_path)
            .unwrap()
            .execute_batch(
                r#"
INSERT INTO workspace_resource_keys (
    workspace_id, resource_kind, resource_id, sequence, resource_key, allocated_at
) VALUES
    ('workspace-test', 'ticket', '00000000001J2', 1, 'T-1', '2026-01-01T00:00:00Z'),
    ('workspace-test', 'ticket', '00000000001J5', 2, 'T-2', '2026-01-01T00:00:00Z'),
    ('workspace-test', 'ticket', '00000000001J6', 3, 'T-3', '2026-01-01T00:00:00Z');
INSERT INTO workspace_resource_key_counters (workspace_id, resource_kind, next_sequence)
VALUES ('workspace-test', 'ticket', 4);
"#,
            )
            .unwrap();
        store
            .upsert_objective(&ObjectiveRecord {
                workspace_id: "workspace-test".to_string(),
                objective_id: "00000000001J3".to_string(),
                title: "Control plane".to_string(),
                state: "active".to_string(),
                body_md: format!(
                    "Objective body. {}\n\nDeep objective marker.\n",
                    "x".repeat(300)
                ),
                created_at: "2026-01-01T00:00:00Z".to_string(),
                updated_at: "2026-01-02T00:00:00Z".to_string(),
            })
            .unwrap();
        store
            .replace_objective_ticket_links(
                "workspace-test",
                "00000000001J3",
                &[ObjectiveTicketLinkRecord {
                    workspace_id: "workspace-test".to_string(),
                    objective_id: "00000000001J3".to_string(),
                    ticket_id: "00000000001J2".to_string(),
                    kind: "linked".to_string(),
                    created_at: "2026-01-01T00:00:00Z".to_string(),
                }],
            )
            .unwrap();
        write_objective(
            dir.path(),
            "00000000001J4",
            "Filesystem-only objective",
            "active",
        );

        let authority = SqliteWorkspaceAuthority::new(&db_path, "workspace-test").unwrap();
        authority
            .ticket_backend
            .add_event(
                TicketIdOrSlug::Id("00000000001J2".to_string()),
                ticket::NewTicketEvent::new(
                    ticket::TicketEventKind::Other("historical_signal".to_string()),
                    format!("{} Historical event marker.", "z".repeat(2_000)),
                ),
            )
            .unwrap();
        for index in 0..110 {
            authority
                .ticket_backend
                .add_event(
                    TicketIdOrSlug::Id("00000000001J2".to_string()),
                    ticket::NewTicketEvent::new(
                        ticket::TicketEventKind::Comment,
                        format!("Filler event {index}."),
                    ),
                )
                .unwrap();
        }
        authority
            .ticket_backend
            .add_ticket_relation(
                TicketIdOrSlug::Id("00000000001J2".to_string()),
                ticket::NewTicketRelation {
                    kind: ticket::TicketRelationKind::Related,
                    target: "00000000001J5".to_string(),
                    note: Some(
                        "mentions unrelated id 00000000001J9 and kind duplicate_of".to_string(),
                    ),
                    author: Some("tester".to_string()),
                },
            )
            .unwrap();
        authority
            .ticket_backend
            .add_ticket_relation(
                TicketIdOrSlug::Id("00000000001J2".to_string()),
                ticket::NewTicketRelation {
                    kind: ticket::TicketRelationKind::DependsOn,
                    target: "00000000001J5".to_string(),
                    note: Some("queued dependency with a transitive blocker".to_string()),
                    author: Some("tester".to_string()),
                },
            )
            .unwrap();
        authority
            .ticket_backend
            .add_ticket_relation(
                TicketIdOrSlug::Id("00000000001J5".to_string()),
                ticket::NewTicketRelation {
                    kind: ticket::TicketRelationKind::DependsOn,
                    target: "00000000001J6".to_string(),
                    note: Some("transitive planning dependency".to_string()),
                    author: Some("tester".to_string()),
                },
            )
            .unwrap();
        let tickets = authority.list_tickets(20).unwrap();
        assert_eq!(tickets.record_authority, "workspace-sqlite");
        assert_eq!(tickets.items[0].record_source, "sqlite_yoi_ticket");
        assert_eq!(tickets.items[0].id, "00000000001J2");
        assert_eq!(tickets.items[0].state, "ready");
        assert_eq!(tickets.items[0].workspace_action_priority, "background");
        let ticket_by_key = authority.ticket(&tickets.items[0].resource_key).unwrap();
        assert_eq!(ticket_by_key.id, tickets.items[0].id);

        let ticket = authority.ticket("00000000001J2").unwrap();
        assert!(!ticket.action_eligibility.can_queue);
        assert!(
            ticket
                .action_eligibility
                .blockers
                .iter()
                .any(|reason| reason.contains("00000000001J6"))
        );
        assert!(ticket.body.contains("Ticket body"));
        assert!(ticket.body_truncated);
        assert!(!ticket.body.contains("Deep Ticket marker"));
        assert!(!ticket.item_revision.is_empty());
        assert_eq!(ticket.linked_objectives[0].id, "00000000001J3");
        assert_eq!(ticket.event_page.returned, ticket.events.len());
        let ticket_query = authority
            .query_tickets(TicketQueryRequest {
                query: Some("Deep Ticket marker".to_string()),
                states: vec!["ready".to_string()],
                linked_objective_id: Some("00000000001J3".to_string()),
                attention: vec!["missing_commit".to_string()],
                limit: Some(1),
                ..TicketQueryRequest::default()
            })
            .unwrap();
        assert_eq!(ticket_query.items.len(), 1);
        assert_eq!(ticket_query.items[0].id, "00000000001J2");
        assert!(
            ticket_query.items[0]
                .matched_fields
                .contains(&"body".to_string())
        );
        assert_eq!(ticket_query.page.limit, 1);
        let no_review = authority
            .query_tickets(TicketQueryRequest {
                review_status: Some("none".to_string()),
                sort: Some("updated_desc".to_string()),
                limit: Some(10),
                ..TicketQueryRequest::default()
            })
            .unwrap();
        assert!(
            no_review
                .items
                .iter()
                .any(|item| item.id == "00000000001J2"),
            "review-status storage predicate must query the authoritative MR thread schema"
        );
        authority
            .query_tickets(TicketQueryRequest {
                review_status: Some("changes_requested".to_string()),
                limit: Some(10),
                ..TicketQueryRequest::default()
            })
            .expect("accepted review-status alias must execute");
        let historical_event_query = authority
            .query_tickets(TicketQueryRequest {
                query: Some("Historical event marker".to_string()),
                event_kinds: vec!["historical_signal".to_string()],
                limit: Some(1),
                ..TicketQueryRequest::default()
            })
            .unwrap();
        assert_eq!(historical_event_query.items.len(), 1);
        let matching_event = historical_event_query.items[0]
            .matching_event
            .as_ref()
            .expect("matching historical event");
        assert_eq!(matching_event.kind, "historical_signal");
        assert!(matching_event.excerpt.contains("Historical event marker"));
        assert!(
            historical_event_query.items[0]
                .snippet
                .as_deref()
                .is_some_and(|snippet| snippet.contains("Historical event marker"))
        );
        let note_only_id = authority
            .query_tickets(TicketQueryRequest {
                related_ticket_id: Some("00000000001J9".to_string()),
                ..TicketQueryRequest::default()
            })
            .unwrap();
        assert!(note_only_id.items.is_empty());
        let note_only_kind = authority
            .query_tickets(TicketQueryRequest {
                relation_kind: Some("duplicate_of".to_string()),
                ..TicketQueryRequest::default()
            })
            .unwrap();
        assert!(note_only_kind.items.is_empty());
        let crossed_relation_filters = authority
            .query_tickets(TicketQueryRequest {
                related_ticket_id: Some("00000000001J6".to_string()),
                relation_kind: Some("related".to_string()),
                ..TicketQueryRequest::default()
            })
            .unwrap();
        assert!(crossed_relation_filters.items.is_empty());
        let exact_relation = authority
            .query_tickets(TicketQueryRequest {
                related_ticket_id: Some("00000000001J5".to_string()),
                relation_kind: Some("related".to_string()),
                ..TicketQueryRequest::default()
            })
            .unwrap();
        assert_eq!(exact_relation.items.len(), 1);
        assert_eq!(exact_relation.items[0].id, "00000000001J2");
        let incoming_relation = authority
            .query_tickets(TicketQueryRequest {
                related_ticket_id: Some("00000000001J2".to_string()),
                relation_kind: Some("related".to_string()),
                limit: Some(10),
                ..TicketQueryRequest::default()
            })
            .unwrap();
        assert_eq!(incoming_relation.items.len(), 1);
        assert_eq!(incoming_relation.items[0].id, "00000000001J5");
        let summary_page = authority
            .list_ticket_page(TicketListPageRequest {
                states: vec!["planning".to_string(), "ready".to_string()],
                limit: Some(1),
                cursor: None,
            })
            .unwrap();
        assert_eq!(summary_page.items.len(), 1);
        assert!(summary_page.page.has_more);
        let mismatched_summary_cursor = authority.list_ticket_page(TicketListPageRequest {
            states: vec!["done".to_string()],
            limit: Some(1),
            cursor: summary_page.page.next_cursor,
        });
        assert!(matches!(
            mismatched_summary_cursor,
            Err(Error::InvalidRecordId(_))
        ));

        let first_page = authority
            .query_tickets(TicketQueryRequest {
                sort: Some("title".to_string()),
                limit: Some(1),
                ..TicketQueryRequest::default()
            })
            .unwrap();
        assert!(first_page.page.has_more);
        let second_page = authority
            .query_tickets(TicketQueryRequest {
                sort: Some("title".to_string()),
                limit: Some(1),
                cursor: first_page.page.next_cursor.clone(),
                ..TicketQueryRequest::default()
            })
            .unwrap();
        assert_eq!(second_page.items.len(), 1);
        assert_ne!(first_page.items[0].id, second_page.items[0].id);
        assert!(second_page.page.has_more);
        let mismatched_cursor = authority.query_tickets(TicketQueryRequest {
            sort: Some("updated_desc".to_string()),
            limit: Some(1),
            cursor: first_page.page.next_cursor.clone(),
            ..TicketQueryRequest::default()
        });
        assert!(matches!(mismatched_cursor, Err(Error::InvalidRecordId(_))));
        let third_page = authority
            .query_tickets(TicketQueryRequest {
                sort: Some("title".to_string()),
                limit: Some(1),
                cursor: second_page.page.next_cursor.clone(),
                ..TicketQueryRequest::default()
            })
            .unwrap();
        assert_eq!(third_page.items.len(), 1);
        assert!(!third_page.page.has_more);

        let objectives = authority.list_objectives(20).unwrap();
        assert_eq!(objectives.record_authority, "workspace-sqlite");
        assert_eq!(objectives.items.len(), 1);
        assert_eq!(objectives.items[0].id, "00000000001J3");
        assert_eq!(objectives.items[0].linked_tickets, vec!["00000000001J2"]);
        let objective_by_key = authority
            .objective(&objectives.items[0].resource_key)
            .unwrap();
        assert_eq!(objective_by_key.id, objectives.items[0].id);
        assert_eq!(
            authority
                .show_objective(
                    &objectives.items[0].resource_key,
                    ObjectiveShowRequest::default(),
                )
                .unwrap()
                .id,
            objectives.items[0].id
        );

        let objective = authority.objective("00000000001J3").unwrap();
        assert!(objective.body.contains("Objective body"));
        assert!(!objective.revision.is_empty());
        assert_eq!(objective.linked_ticket_summaries[0].id, "00000000001J2");
        assert_eq!(objective.linked_ticket_summaries[0].state, "ready");
        let objective_query = authority
            .query_objectives(ObjectiveQueryRequest {
                query: Some("Control plane".to_string()),
                linked_ticket_id: Some("00000000001J2".to_string()),
                limit: Some(1),
                ..ObjectiveQueryRequest::default()
            })
            .unwrap();
        assert_eq!(objective_query.items.len(), 1);
        assert_eq!(objective_query.items[0].linked_ticket_count, 1);
        assert_eq!(objective_query.page.limit, 1);
        let body_query = authority
            .query_objectives(ObjectiveQueryRequest {
                query: Some("Deep objective marker".to_string()),
                limit: Some(1),
                ..ObjectiveQueryRequest::default()
            })
            .unwrap();
        assert_eq!(body_query.items.len(), 1);
        assert_eq!(body_query.items[0].matched_fields, vec!["body"]);
        assert!(
            body_query.items[0]
                .snippet
                .as_deref()
                .is_some_and(|snippet| snippet.contains("Deep objective marker"))
        );
        let memory = authority.ensure_memory_document().unwrap();
        assert_eq!(memory.body_md, DEFAULT_MEMORY_DOCUMENT_BODY);
        let updated = authority
            .update_memory_document("# Memory\n\n- Durable fact.\n")
            .unwrap();
        assert!(updated.body_md.contains("Durable fact"));

        let staging = authority
            .upsert_memory_staging_record(
                "candidate-1",
                r#"{"claim":"Candidate"}"#,
                Some("memory/_staging/candidate-1.json"),
            )
            .unwrap();
        assert_eq!(staging.record_source, "workspace-sqlite");
        assert_eq!(authority.list_memory_staging_records(20).unwrap().len(), 1);
        let resolution = authority
            .close_memory_staging_record("candidate-1", "apply", "accepted", r#"["memory"]"#)
            .unwrap();
        assert_eq!(resolution.action, "apply");
        assert_eq!(resolution.reason, "accepted");
        assert!(
            authority
                .list_memory_staging_records(20)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            authority.list_memory_staging_resolutions(20).unwrap().len(),
            1
        );
    }

    #[tokio::test]
    async fn objective_mutations_write_sqlite_records_and_audit_events() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("workspace.db");
        let store = SqliteWorkspaceStore::open(&db_path).unwrap();
        store
            .upsert_workspace(&WorkspaceRecord {
                workspace_id: "workspace-test".to_string(),
                owner_account_id: None,
                display_name: "Workspace Test".to_string(),
                state: "active".to_string(),
                created_at: "2026-01-01T00:00:00Z".to_string(),
                updated_at: "2026-01-01T00:00:00Z".to_string(),
            })
            .await
            .unwrap();
        rusqlite::Connection::open(&db_path)
            .unwrap()
            .execute_batch(
                r#"
INSERT INTO typed_tickets (
    workspace_id, ticket_id, slug, title, status, kind, priority, body,
    workflow_state, workflow_state_explicit
) VALUES
    ('workspace-test', '00000000001J2', 'ticket-j2', 'Ticket J2', 'open', 'task', 'normal', '', 'planning', 1),
    ('workspace-test', '00000000001J3', 'ticket-j3', 'Ticket J3', 'open', 'task', 'normal', '', 'planning', 1);
INSERT INTO workspace_resource_keys (
    workspace_id, resource_kind, resource_id, sequence, resource_key, allocated_at
) VALUES
    ('workspace-test', 'ticket', '00000000001J2', 1, 'T-1', '2026-01-01T00:00:00Z'),
    ('workspace-test', 'ticket', '00000000001J3', 2, 'T-2', '2026-01-01T00:00:00Z');
INSERT INTO workspace_resource_key_counters (workspace_id, resource_kind, next_sequence)
VALUES ('workspace-test', 'ticket', 3);
"#,
            )
            .unwrap();
        let authority = SqliteWorkspaceAuthority::new(&db_path, "workspace-test").unwrap();

        let created = authority
            .create_objective(ObjectiveCreateInput {
                title: "Create Objective".to_string(),
                body_md: "Alpha body".to_string(),
                state: "active".to_string(),
                linked_tickets: vec!["00000000001J2".to_string()],
            })
            .unwrap();
        assert_eq!(created.title, "Create Objective");
        assert_eq!(created.linked_tickets, vec!["00000000001J2"]);

        let edited = authority
            .edit_objective(
                &created.id,
                ObjectiveEditInput {
                    title: Some("Edited Objective".to_string()),
                    old_string: Some("Alpha".to_string()),
                    new_string: Some("Beta".to_string()),
                    replace_all: false,
                },
            )
            .unwrap();
        assert_eq!(edited.title, "Edited Objective");
        assert_eq!(edited.body, "Beta body");

        let state = authority
            .set_objective_state(&created.id, "paused")
            .unwrap();
        assert_eq!(state.state, "paused");
        assert_eq!(
            authority
                .link_objective_ticket(&created.id, "00000000001J3")
                .unwrap()
                .linked_tickets,
            vec!["00000000001J2", "00000000001J3"]
        );
        assert_eq!(
            authority
                .unlink_objective_ticket(&created.id, "00000000001J2")
                .unwrap()
                .linked_tickets,
            vec!["00000000001J3"]
        );

        let events = store
            .list_objective_events("workspace-test", &created.id)
            .unwrap();
        assert_eq!(
            events
                .iter()
                .map(|event| event.kind.as_str())
                .collect::<Vec<_>>(),
            vec!["create", "edit", "state", "link_ticket", "unlink_ticket"]
        );
    }

    #[test]
    fn does_not_read_legacy_ticket_files_without_sqlite_import() {
        let dir = tempfile::tempdir().unwrap();
        write_ticket(dir.path(), "00000000001J5", "Legacy file", "ready");
        let db_path = dir.path().join("workspace.db");

        let authority = SqliteWorkspaceAuthority::new(&db_path, "workspace-test").unwrap();
        let tickets = authority.list_tickets(20).unwrap();
        assert!(tickets.items.is_empty());
    }

    fn write_ticket(root: &Path, id: &str, title: &str, state: &str) {
        write_ticket_at(&root.join(".yoi/tickets"), id, title, state);
    }

    fn write_ticket_at(ticket_root: &Path, id: &str, title: &str, state: &str) {
        let ticket_dir = ticket_root.join(id);
        fs::create_dir_all(&ticket_dir).unwrap();
        fs::write(
            ticket_dir.join("item.md"),
            format!(
                r#"---
title: "{title}"
state: "{state}"
created_at: "2026-01-01T00:00:00Z"
updated_at: "2026-01-02T00:00:00Z"
---

Ticket body.
{padding}
Deep Ticket marker.
"#,
                padding = "x".repeat(70_000),
            ),
        )
        .unwrap();
        fs::write(ticket_dir.join("thread.md"), "").unwrap();
    }

    fn write_objective(root: &Path, id: &str, title: &str, state: &str) {
        let objective_dir = root.join(".yoi/objectives").join(id);
        fs::create_dir_all(&objective_dir).unwrap();
        fs::write(
            objective_dir.join("item.md"),
            format!(
                r#"---
title: "{title}"
state: "{state}"
created_at: "2026-01-01T00:00:00Z"
updated_at: "2026-01-02T00:00:00Z"
linked_tickets: ["00000000001J2"]
---

Objective body.
"#,
            ),
        )
        .unwrap();
    }
}
