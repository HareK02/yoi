use std::path::PathBuf;

use chrono::Utc;
use project_record::{allocate_record_id, unix_epoch_millis_now};

use ticket::{
    SqliteTicketBackend, TicketBackend, TicketIdOrSlug, TicketListQuery,
    TicketWorkspaceActionPriority, project_ticket_workspace_item,
};

use crate::records::{
    ObjectiveDetail, ObjectiveResourceSummary, ObjectiveSummary, ProjectRecordList, TicketDetail,
    TicketEventDetail, TicketSummary, summarize_body, truncate_body, validate_project_id,
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
    fn ticket(&self, id: &str) -> Result<TicketDetail>;
}

pub trait ObjectiveAuthority {
    fn list_objectives(&self, limit: usize) -> Result<ProjectRecordList<ObjectiveSummary>>;
    fn objective(&self, id: &str) -> Result<ObjectiveDetail>;
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
}

impl SqliteWorkspaceAuthority {
    pub fn new(database_path: impl Into<PathBuf>, workspace_id: impl Into<String>) -> Result<Self> {
        let database_path = database_path.into();
        let workspace_id = workspace_id.into();
        Ok(Self {
            workspace_id: workspace_id.clone(),
            store: SqliteWorkspaceStore::open(&database_path)?,
            ticket_backend: SqliteTicketBackend::open_verified(database_path, workspace_id)?,
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
        Ok(ObjectiveDetail {
            id: record.objective_id,
            title: record.title,
            state: record.state,
            created_at: Some(record.created_at),
            updated_at: Some(record.updated_at),
            linked_tickets,
            resources,
            body,
            body_truncated,
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
}

impl TicketAuthority for SqliteWorkspaceAuthority {
    fn list_tickets(&self, limit: usize) -> Result<ProjectRecordList<TicketSummary>> {
        let mut items = Vec::new();
        for item in self.ticket_backend.list(TicketListQuery::all())? {
            let ticket = self
                .ticket_backend
                .show(TicketIdOrSlug::Id(item.id.clone()))?;
            let projection = project_ticket_workspace_item(&item, &ticket.relations.blockers, None);
            items.push(TicketSummary {
                id: item.id,
                title: item.title,
                state: item.workflow_state.as_str().to_string(),
                priority: item.priority,
                updated_at: item.updated_at,
                queued_by: item.queued_by,
                queued_at: item.queued_at,
                workspace_action_priority: workspace_action_priority_name(projection.priority)
                    .to_string(),
                record_source: "sqlite_yoi_ticket".to_string(),
            });
        }
        items.sort_by(|a, b| {
            b.updated_at
                .cmp(&a.updated_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        items.truncate(limit);
        Ok(ProjectRecordList {
            items,
            invalid_records: Vec::new(),
            record_authority: "workspace-sqlite".to_string(),
        })
    }

    fn ticket(&self, id: &str) -> Result<TicketDetail> {
        validate_project_id(id)?;
        let ticket = self
            .ticket_backend
            .show(TicketIdOrSlug::Id(id.to_string()))?;
        let (body, body_truncated) =
            truncate_body(ticket.document.body.as_str(), DETAIL_BODY_LIMIT);
        let event_start = ticket.events.len().saturating_sub(TICKET_EVENT_LIMIT);
        let events = ticket.events[event_start..]
            .iter()
            .enumerate()
            .map(|(index, event)| TicketEventDetail {
                sequence: event_start + index,
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
            })
            .collect();
        Ok(TicketDetail {
            id: ticket.meta.id,
            title: ticket.meta.title,
            state: ticket.meta.workflow_state.as_str().to_string(),
            priority: ticket.meta.priority,
            created_at: ticket.meta.created_at,
            updated_at: ticket.meta.updated_at,
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
            artifact_count: ticket.artifacts.len(),
            artifacts: ticket
                .artifacts
                .into_iter()
                .map(|artifact| artifact.relative_path.display().to_string())
                .collect(),
            relations: ticket.relations.into(),
            resolution: ticket
                .resolution
                .map(|resolution| resolution.as_str().to_string()),
            record_source: "sqlite_yoi_ticket".to_string(),
        })
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

    fn objective(&self, id: &str) -> Result<ObjectiveDetail> {
        validate_project_id(id)?;
        let record = self.objective_record(id)?;
        self.objective_detail_from_record(record)
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

    #[tokio::test]
    async fn sqlite_workspace_authority_reads_sqlite_records_without_filesystem_authority() {
        let dir = tempfile::tempdir().unwrap();
        write_ticket(dir.path(), "00000000001J2", "Read bridge", "ready");
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
                body_md: "Objective body.\n".to_string(),
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

        let objectives = authority.list_objectives(20).unwrap();
        assert_eq!(objectives.record_authority, "workspace-sqlite");
        assert_eq!(objectives.items.len(), 1);
        assert_eq!(objectives.items[0].id, "00000000001J3");
        assert_eq!(objectives.items[0].linked_tickets, vec!["00000000001J2"]);

        let objective = authority.objective("00000000001J3").unwrap();
        assert!(objective.body.contains("Objective body"));
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
