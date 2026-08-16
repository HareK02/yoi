use std::path::PathBuf;

use chrono::Utc;
use merge_request::{MergeRequest, ReviewStatus, SqliteMergeRequestStore};
use project_record::{allocate_record_id, unix_epoch_millis_now};

use ticket::{
    SqliteTicketBackend, TicketBackend, TicketEvent, TicketIdOrSlug, TicketWorkspaceActionPriority,
    project_ticket_workspace_item,
};

use crate::records::{
    ObjectiveDetail, ObjectiveEventDetail, ObjectiveLinkSummary, ObjectiveLinkedTicketSummary,
    ObjectiveQueryItem, ObjectiveQueryRequest, ObjectiveQueryResponse, ObjectiveResourceSummary,
    ObjectiveShowRequest, ObjectiveSummary, ProjectRecordList, QueryPage, TicketAssignmentSummary,
    TicketDetail, TicketEventDetail, TicketEvidenceEvent, TicketEvidenceSummary,
    TicketMergeRequestSummary, TicketQueryItem, TicketQueryRequest, TicketQueryResponse,
    TicketShowRequest, TicketSummary, summarize_body, truncate_body, validate_project_id,
};
use crate::store::{
    ControlPlaneStore, MemoryDocumentRecord, MemoryStagingRecord, MemoryStagingResolutionRecord,
    ObjectiveEventRecord, ObjectiveRecord, ObjectiveTicketLinkRecord, SqliteWorkspaceStore,
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
pub struct SqliteWorkspaceAuthority {
    workspace_id: String,
    store: SqliteWorkspaceStore,
    ticket_backend: SqliteTicketBackend,
    merge_request_store: SqliteMergeRequestStore,
}

impl SqliteWorkspaceAuthority {
    pub fn new(database_path: impl Into<PathBuf>, workspace_id: impl Into<String>) -> Result<Self> {
        let database_path = database_path.into();
        let workspace_id = workspace_id.into();
        Ok(Self {
            workspace_id: workspace_id.clone(),
            store: SqliteWorkspaceStore::open(&database_path)?,
            ticket_backend: SqliteTicketBackend::open_verified(
                database_path.clone(),
                workspace_id.clone(),
            )?,
            merge_request_store: SqliteMergeRequestStore::open(database_path, workspace_id)
                .map_err(|error| Error::Store(error.to_string()))?,
        })
    }

    fn objective_record(&self, id: &str) -> Result<ObjectiveRecord> {
        self.store
            .get_objective(&self.workspace_id, id)?
            .ok_or_else(|| unknown_objective_error(id))
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

    fn read_ticket_detail(&self, id: &str, request: TicketShowRequest) -> Result<TicketDetail> {
        validate_project_id(id)?;
        let ticket = self
            .ticket_backend
            .show(TicketIdOrSlug::Id(id.to_string()))?;
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
            .map(|objective| ObjectiveLinkSummary {
                id: objective.objective_id,
                title: objective.title,
                state: objective.state,
            })
            .collect::<Vec<_>>();
        let implementation_reports = ticket
            .events
            .iter()
            .enumerate()
            .filter(|(_, event)| event.kind.as_str() == "implementation_report")
            .map(|(sequence, event)| ticket_evidence_event(sequence, event))
            .collect::<Vec<_>>();
        let current_assignment = self
            .store
            .get_current_ticket_worker_assignment(&self.workspace_id, id)?
            .map(|assignment| TicketAssignmentSummary {
                assignment_id: assignment.assignment_id,
                runtime_id: assignment.worker.runtime_id,
                worker_id: assignment.worker.worker_id,
            });
        let merge_request = self
            .merge_request_store
            .show_for_ticket(id)
            .map_err(|error| Error::Store(error.to_string()))?
            .map(merge_request_summary);
        let evidence = ticket_evidence_summary(&ticket.events, merge_request.as_ref());
        let item_revision = ticket
            .events
            .iter()
            .rev()
            .find(|event| matches!(event.kind.as_str(), "create" | "item_edit"))
            .and_then(|event| event.attributes.get("event_id").cloned())
            .or_else(|| ticket.meta.updated_at.clone())
            .unwrap_or_else(|| format!("{}:0", ticket.meta.id));
        Ok(TicketDetail {
            id: ticket.meta.id,
            title: ticket.meta.title,
            state: ticket.meta.workflow_state.as_str().to_string(),
            readiness: ticket.meta.readiness,
            priority: ticket.meta.priority,
            created_at: ticket.meta.created_at,
            updated_at: ticket.meta.updated_at,
            item_revision,
            queued_by: ticket.meta.queued_by,
            queued_at: ticket.meta.queued_at,
            assignee: ticket.meta.assignee,
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
            relations: ticket.relations.into(),
            linked_objectives,
            implementation_reports,
            current_assignment,
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
                TicketSummary {
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
                }
            })
            .collect();
        Ok(ProjectRecordList {
            items,
            invalid_records: Vec::new(),
            record_authority: "workspace-sqlite".to_string(),
        })
    }

    fn query_tickets(&self, query: TicketQueryRequest) -> Result<TicketQueryResponse> {
        validate_ticket_query(&query)?;
        let limit = query.limit.unwrap_or(50).clamp(1, 100);
        let sort = normalize_ticket_sort(query.sort.as_deref(), query.query.is_some())?;
        let cursor = query
            .cursor
            .as_deref()
            .map(parse_query_cursor)
            .transpose()?;
        let mut summaries = self.list_tickets(1_001)?.items;
        let source_truncated = summaries.len() > 1_000;
        summaries.truncate(1_000);
        let mut items = Vec::new();
        for summary in summaries {
            let detail = self.read_ticket_detail(
                &summary.id,
                TicketShowRequest {
                    event_limit: Some(TICKET_EVENT_LIMIT),
                    event_cursor: None,
                },
            )?;
            if ticket_matches_query(&summary, &detail, &query) {
                items.push(ticket_query_item(summary, &detail, &query));
            }
        }
        sort_ticket_query_items(&mut items, sort);
        if let Some(cursor) = cursor {
            items.retain(|item| ticket_item_after_cursor(item, sort, &cursor));
        }
        let has_more = items.len() > limit;
        items.truncate(limit);
        let next_cursor = has_more
            .then(|| items.last().map(|item| make_ticket_cursor(item, sort)))
            .flatten();
        Ok(TicketQueryResponse {
            page: QueryPage {
                limit,
                returned: items.len(),
                has_more,
                next_cursor,
                sort: sort.to_string(),
                source_limit: Some(1_000),
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
        let cursor = query
            .cursor
            .as_deref()
            .map(parse_query_cursor)
            .transpose()?;
        let mut objectives = self.list_objectives(1_001)?.items;
        let source_truncated = objectives.len() > 1_000;
        objectives.truncate(1_000);
        let mut items = Vec::new();
        for objective in objectives {
            let body_md = self.objective_record(&objective.id)?.body_md;
            if !objective_matches_query(&objective, &body_md, &query) {
                continue;
            }
            let linked_tickets = self
                .store
                .list_objective_ticket_links(&self.workspace_id, &objective.id)?
                .into_iter()
                .map(|link| link.ticket_id)
                .collect::<Vec<_>>();
            if query
                .linked_ticket_id
                .as_ref()
                .is_some_and(|id| !linked_tickets.iter().any(|ticket_id| ticket_id == id))
            {
                continue;
            }
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
            .then(|| items.last().map(|item| make_objective_cursor(item, sort)))
            .flatten();
        Ok(ObjectiveQueryResponse {
            page: QueryPage {
                limit,
                returned: items.len(),
                has_more,
                next_cursor,
                sort: sort.to_string(),
                source_limit: Some(1_000),
                source_truncated,
            },
            items,
            record_authority: RECORD_SOURCE_WORKSPACE_SQLITE.to_string(),
        })
    }

    fn objective(&self, id: &str) -> Result<ObjectiveDetail> {
        validate_project_id(id)?;
        let record = self.objective_record(id)?;
        self.objective_detail_from_record(record)
    }

    fn show_objective(&self, id: &str, query: ObjectiveShowRequest) -> Result<ObjectiveDetail> {
        let mut detail = self.objective(id)?;
        let all_events = self.store.list_objective_events(&self.workspace_id, id)?;
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

    fn edit_objective(&self, id: &str, input: ObjectiveEditInput) -> Result<ObjectiveDetail> {
        validate_project_id(id)?;
        let mut record = self.objective_record(id)?;
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
        self.store.upsert_objective(&record)?;
        self.insert_objective_event(id, "edit", None)?;
        self.objective(id)
    }

    fn set_objective_state(&self, id: &str, state: &str) -> Result<ObjectiveDetail> {
        validate_project_id(id)?;
        validate_objective_state(state)?;
        let mut record = self.objective_record(id)?;
        record.state = state.trim().to_string();
        record.updated_at = now_rfc3339();
        self.store.upsert_objective(&record)?;
        self.insert_objective_event(id, "state", Some(&record.state))?;
        self.objective(id)
    }

    fn link_objective_ticket(&self, id: &str, ticket_id: &str) -> Result<ObjectiveDetail> {
        validate_project_id(id)?;
        validate_project_id(ticket_id)?;
        let _record = self.objective_record(id)?;
        let now = now_rfc3339();
        let mut links = self
            .store
            .list_objective_ticket_links(&self.workspace_id, id)?;
        if !links.iter().any(|link| link.ticket_id == ticket_id) {
            links.push(ObjectiveTicketLinkRecord {
                workspace_id: self.workspace_id.clone(),
                objective_id: id.to_string(),
                ticket_id: ticket_id.to_string(),
                kind: "linked".to_string(),
                created_at: now,
            });
            self.store
                .replace_objective_ticket_links(&self.workspace_id, id, &links)?;
            self.insert_objective_event(id, "link_ticket", Some(ticket_id))?;
        }
        self.objective(id)
    }

    fn unlink_objective_ticket(&self, id: &str, ticket_id: &str) -> Result<ObjectiveDetail> {
        validate_project_id(id)?;
        validate_project_id(ticket_id)?;
        let _record = self.objective_record(id)?;
        let mut links = self
            .store
            .list_objective_ticket_links(&self.workspace_id, id)?;
        let original_len = links.len();
        links.retain(|link| link.ticket_id != ticket_id);
        if links.len() != original_len {
            self.store
                .replace_objective_ticket_links(&self.workspace_id, id, &links)?;
            self.insert_objective_event(id, "unlink_ticket", Some(ticket_id))?;
        }
        self.objective(id)
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

fn merge_request_summary(request: MergeRequest) -> TicketMergeRequestSummary {
    let review_status = match request.review_status {
        ReviewStatus::Pending => "pending",
        ReviewStatus::Approved => "approved",
        ReviewStatus::ChangesRequested => "changes_requested",
    };
    TicketMergeRequestSummary {
        merge_request_id: request.merge_request_id,
        state: serde_json::to_value(request.state)
            .ok()
            .and_then(|value| value.as_str().map(str::to_string))
            .unwrap_or_else(|| "open".to_string()),
        review_status: review_status.to_string(),
        revision_id: request.current_revision.revision_id,
        base_commit: request.current_revision.base_commit,
        head_commit: request.current_revision.head_commit,
        changed_paths: request.current_revision.changed_paths,
        updated_at: request.updated_at,
        review_submitted_at: request
            .current_review
            .as_ref()
            .map(|review| review.submitted_at.clone()),
        review_excerpt: request
            .current_review
            .as_ref()
            .map(|review| truncate_body(&review.body, 512).0),
    }
}

fn ticket_evidence_summary(
    events: &[TicketEvent],
    merge_request: Option<&TicketMergeRequestSummary>,
) -> TicketEvidenceSummary {
    let latest_report = events
        .iter()
        .enumerate()
        .rev()
        .find(|(_, event)| event.kind.as_str() == "implementation_report")
        .map(|(sequence, _)| sequence);
    let latest_rescope = events
        .iter()
        .enumerate()
        .rev()
        .find(|(_, event)| event.kind.as_str() == "item_edit")
        .map(|(sequence, _)| sequence);
    let report_after_rescope =
        latest_report.is_some_and(|report| latest_rescope.is_none_or(|rescope| report > rescope));
    let has_commit = merge_request.is_some_and(|request| !request.head_commit.is_empty())
        || events.iter().any(|event| {
            event.attributes.contains_key("commit") || event.attributes.contains_key("head_commit")
        });
    let review_status = merge_request.map(|request| request.review_status.clone());
    let approved = review_status.as_deref() == Some("approved");
    let unresolved_request_changes = review_status.as_deref() == Some("changes_requested");
    let mut missing = Vec::new();
    if latest_report.is_none() {
        missing.push("implementation_report".to_string());
    } else if !report_after_rescope {
        missing.push("implementation_report_after_rescope".to_string());
    }
    if !has_commit {
        missing.push("commit".to_string());
    }
    if merge_request.is_none() {
        missing.push("merge_request".to_string());
    }
    if !approved {
        missing.push("approved_review".to_string());
    }
    TicketEvidenceSummary {
        has_implementation_report: latest_report.is_some(),
        implementation_report_after_rescope: report_after_rescope,
        has_merge_request: merge_request.is_some(),
        has_commit,
        review_status,
        approved,
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
            "implementation_report"
                | "implementation_report_after_rescope"
                | "merge_request"
                | "commit"
                | "approved_review"
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
                | "implementation_report_not_closed"
                | "report_after_rescope"
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

fn ticket_matches_query(
    summary: &TicketSummary,
    detail: &TicketDetail,
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
        && !detail
            .events
            .iter()
            .any(|event| query.event_kinds.iter().any(|kind| kind == &event.kind))
    {
        return false;
    }
    if let Some(review_status) = &query.review_status {
        let matches = match review_status.as_str() {
            "none" => detail.evidence.review_status.is_none(),
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
        .all(|evidence| match evidence.as_str() {
            "implementation_report" => detail.evidence.has_implementation_report,
            "implementation_report_after_rescope" => {
                detail.evidence.implementation_report_after_rescope
            }
            "merge_request" => detail.evidence.has_merge_request,
            "commit" => detail.evidence.has_commit,
            "approved_review" => detail.evidence.approved,
            _ => false,
        })
    {
        return false;
    }
    if !query
        .attention
        .iter()
        .all(|attention| match attention.as_str() {
            "done_not_closed" => summary.state == "done",
            "implementation_report_not_closed" => {
                detail.evidence.has_implementation_report && summary.state != "closed"
            }
            "report_after_rescope" => detail.evidence.implementation_report_after_rescope,
            "unresolved_review" => detail.evidence.unresolved_request_changes,
            "missing_commit" => !detail.evidence.has_commit,
            "blocked" => !detail.relations.blockers.is_empty(),
            "unblocked" => detail.relations.blockers.is_empty(),
            "ready" => summary.state == "ready" && detail.relations.blockers.is_empty(),
            "awaiting_review" => detail.evidence.review_status.as_deref() == Some("pending"),
            "unresolved_changes" => detail.evidence.unresolved_request_changes,
            "stale_after_rescope" => {
                detail.evidence.has_implementation_report
                    && !detail.evidence.implementation_report_after_rescope
            }
            "missing_evidence" => !detail.evidence.complete_for_integration,
            _ => false,
        })
    {
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
    if let Some(ticket_id) = &query.related_ticket_id {
        let relation_json = serde_json::to_string(&detail.relations).unwrap_or_default();
        if !relation_json.contains(ticket_id) {
            return false;
        }
    }
    if let Some(kind) = &query.relation_kind {
        let relation_json = serde_json::to_string(&detail.relations).unwrap_or_default();
        if !relation_json.contains(kind) {
            return false;
        }
    }
    query.query.as_ref().is_none_or(|text| {
        let needle = text.to_lowercase();
        summary.title.to_lowercase().contains(&needle)
            || detail.body.to_lowercase().contains(&needle)
            || detail.events.iter().any(|event| {
                event
                    .body
                    .as_deref()
                    .is_some_and(|body| body.to_lowercase().contains(&needle))
            })
    })
}

fn ticket_query_item(
    summary: TicketSummary,
    detail: &TicketDetail,
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
        if detail.body.to_lowercase().contains(&needle) {
            matched_fields.push("body".to_string());
            snippet.get_or_insert_with(|| matching_snippet(&detail.body, text));
        }
        if let Some(event) = detail.events.iter().find(|event| {
            event
                .body
                .as_deref()
                .is_some_and(|body| body.to_lowercase().contains(&needle))
        }) {
            matched_fields.push("event".to_string());
            matching_event = Some(TicketEvidenceEvent {
                event_ref: event.event_ref.clone(),
                sequence: event.sequence,
                kind: event.kind.clone(),
                at: event.at.clone(),
                author: event.author.clone(),
                excerpt: event.body.clone().unwrap_or_default(),
            });
            snippet.get_or_insert_with(|| event.body.clone().unwrap_or_default());
        }
    }
    TicketQueryItem {
        id: summary.id,
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

fn make_ticket_cursor(item: &TicketQueryItem, sort: TicketQuerySort) -> String {
    make_query_cursor(&ticket_sort_key(item, sort), &item.id)
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

fn objective_matches_query(
    objective: &ObjectiveSummary,
    body_md: &str,
    query: &ObjectiveQueryRequest,
) -> bool {
    if !query.states.is_empty() && !query.states.iter().any(|state| state == &objective.state) {
        return false;
    }
    if query
        .updated_after
        .as_ref()
        .is_some_and(|after| objective.updated_at.as_deref().unwrap_or("") <= after.as_str())
        || query
            .updated_before
            .as_ref()
            .is_some_and(|before| objective.updated_at.as_deref().unwrap_or("") >= before.as_str())
    {
        return false;
    }
    query.query.as_ref().is_none_or(|text| {
        let needle = text.to_lowercase();
        objective.title.to_lowercase().contains(&needle) || body_md.to_lowercase().contains(&needle)
    })
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

fn make_objective_cursor(item: &ObjectiveQueryItem, sort: ObjectiveQuerySort) -> String {
    make_query_cursor(&objective_sort_key(item, sort), &item.id)
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

fn workspace_action_priority_name(priority: TicketWorkspaceActionPriority) -> &'static str {
    match priority {
        TicketWorkspaceActionPriority::ReadyForQueue => "ready_for_queue",
        TicketWorkspaceActionPriority::ActiveWork => "active_work",
        TicketWorkspaceActionPriority::Background => "background",
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::*;
    use crate::store::{ObjectiveRecord, ObjectiveTicketLinkRecord, WorkspaceRecord};

    #[test]
    fn ticket_evidence_summary_requires_report_after_latest_rescope_and_approved_revision() {
        let event = |kind: &str| TicketEvent {
            kind: ticket::TicketEventKind::Other(kind.to_string()),
            author: Some("coder".to_string()),
            at: Some("2026-01-01T00:00:00Z".to_string()),
            status: None,
            from: None,
            to: None,
            reason: None,
            state_field: None,
            heading: None,
            body: ticket::MarkdownText::new(kind),
            attributes: Default::default(),
            references: Vec::new(),
        };
        let request = TicketMergeRequestSummary {
            merge_request_id: "mr-1".to_string(),
            state: "open".to_string(),
            review_status: "approved".to_string(),
            revision_id: "revision-1".to_string(),
            base_commit: "base".to_string(),
            head_commit: "head".to_string(),
            changed_paths: vec!["src/lib.rs".to_string()],
            updated_at: "2026-01-01T00:00:00Z".to_string(),
            review_submitted_at: Some("2026-01-01T00:00:00Z".to_string()),
            review_excerpt: Some("approved".to_string()),
        };
        let stale = ticket_evidence_summary(
            &[event("implementation_report"), event("item_edit")],
            Some(&request),
        );
        assert!(!stale.implementation_report_after_rescope);
        assert!(!stale.complete_for_integration);
        assert!(
            stale
                .missing
                .contains(&"implementation_report_after_rescope".to_string())
        );
        let current = ticket_evidence_summary(
            &[
                event("implementation_report"),
                event("item_edit"),
                event("implementation_report"),
            ],
            Some(&request),
        );
        assert!(current.implementation_report_after_rescope);
        assert!(current.has_commit);
        assert!(current.approved);
        assert!(current.complete_for_integration);
    }

    #[tokio::test]
    async fn sqlite_workspace_authority_reads_sqlite_records_without_filesystem_authority() {
        let dir = tempfile::tempdir().unwrap();
        write_ticket(dir.path(), "00000000001J2", "Read bridge", "ready");
        write_ticket(dir.path(), "00000000001J5", "Second ticket", "planning");
        let db_path = dir.path().join("workspace.db");
        SqliteTicketBackend::open(&db_path, "workspace-test")
            .unwrap()
            .import_from_local_backend(&ticket::LocalTicketBackend::new(
                dir.path().join(".yoi/tickets"),
            ))
            .unwrap();
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
        let tickets = authority.list_tickets(20).unwrap();
        assert_eq!(tickets.record_authority, "workspace-sqlite");
        assert_eq!(tickets.items[0].record_source, "sqlite_yoi_ticket");
        assert_eq!(tickets.items[0].id, "00000000001J2");
        assert_eq!(tickets.items[0].state, "ready");
        assert_eq!(
            tickets.items[0].workspace_action_priority,
            "ready_for_queue"
        );

        let ticket = authority.ticket("00000000001J2").unwrap();
        assert!(ticket.body.contains("Ticket body"));
        assert!(!ticket.item_revision.is_empty());
        assert_eq!(ticket.linked_objectives[0].id, "00000000001J3");
        assert_eq!(ticket.event_page.returned, ticket.events.len());
        let ticket_query = authority
            .query_tickets(TicketQueryRequest {
                query: Some("Ticket body".to_string()),
                states: vec!["ready".to_string()],
                linked_objective_id: Some("00000000001J3".to_string()),
                attention: vec!["unblocked".to_string(), "missing_commit".to_string()],
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
        assert!(!second_page.page.has_more);

        let objectives = authority.list_objectives(20).unwrap();
        assert_eq!(objectives.record_authority, "workspace-sqlite");
        assert_eq!(objectives.items.len(), 1);
        assert_eq!(objectives.items[0].id, "00000000001J3");
        assert_eq!(objectives.items[0].linked_tickets, vec!["00000000001J2"]);

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
"#,
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
