use std::path::PathBuf;

use ticket::{
    SqliteTicketBackend, TicketBackend, TicketIdOrSlug, TicketListQuery,
    TicketWorkspaceActionPriority, project_ticket_workspace_item,
};

use crate::records::{
    ObjectiveDetail, ObjectiveResourceSummary, ObjectiveSummary, ProjectRecordList, TicketDetail,
    TicketSummary, summarize_body, truncate_body, validate_project_id,
};
use crate::store::{ControlPlaneStore, SqliteWorkspaceStore};
use crate::{Error, Result};

const DETAIL_BODY_LIMIT: usize = 64 * 1024;

/// Workspace-scoped runtime authority for project resources.
///
/// Normal Backend API handlers should depend on this authority abstraction, not
/// on legacy filesystem layouts. Filesystem readers belong in explicit import
/// / migration helpers such as `objective_import`.
pub trait WorkspaceAuthority: ObjectiveAuthority + TicketAuthority + MemoryAuthorityMarker {}

impl<T> WorkspaceAuthority for T where
    T: ObjectiveAuthority + TicketAuthority + MemoryAuthorityMarker
{
}

pub trait TicketAuthority {
    fn list_tickets(&self, limit: usize) -> Result<ProjectRecordList<TicketSummary>>;
    fn ticket(&self, id: &str) -> Result<TicketDetail>;
}

pub trait ObjectiveAuthority {
    fn list_objectives(&self, limit: usize) -> Result<ProjectRecordList<ObjectiveSummary>>;
    fn objective(&self, id: &str) -> Result<ObjectiveDetail>;
}

/// Marker for the pending Memory authority split.
///
/// Memory still routes through the legacy `memory` crate backend in this codebase.
/// This marker makes the missing SQL-backed Memory authority explicit so callers
/// do not mistake the current Backend API boundary for a completed storage
/// authority migration.
pub trait MemoryAuthorityMarker {
    fn memory_authority_status(&self) -> MemoryAuthorityStatus;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryAuthorityStatus {
    LegacyFilesystemBackendPendingSqliteAuthority,
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
            ticket_backend: SqliteTicketBackend::new(database_path, workspace_id),
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
        Ok(TicketDetail {
            id: ticket.meta.id,
            title: ticket.meta.title,
            state: ticket.meta.workflow_state.as_str().to_string(),
            priority: ticket.meta.priority,
            created_at: ticket.meta.created_at,
            updated_at: ticket.meta.updated_at,
            queued_by: ticket.meta.queued_by,
            queued_at: ticket.meta.queued_at,
            risk_flags: ticket.meta.risk_flags,
            body,
            body_truncated,
            event_count: ticket.events.len(),
            artifact_count: ticket.artifacts.len(),
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
                record_source: "workspace-sqlite".to_string(),
            });
        }
        Ok(ProjectRecordList {
            items,
            invalid_records: Vec::new(),
            record_authority: "workspace-sqlite".to_string(),
        })
    }

    fn objective(&self, id: &str) -> Result<ObjectiveDetail> {
        validate_project_id(id)?;
        let record = self
            .store
            .get_objective(&self.workspace_id, id)?
            .ok_or_else(|| Error::Store(format!("unknown objective `{id}`")))?;
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
            record_source: "workspace-sqlite".to_string(),
        })
    }
}

impl MemoryAuthorityMarker for SqliteWorkspaceAuthority {
    fn memory_authority_status(&self) -> MemoryAuthorityStatus {
        MemoryAuthorityStatus::LegacyFilesystemBackendPendingSqliteAuthority
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
    use crate::store::WorkspaceRecord;

    #[tokio::test]
    async fn sqlite_workspace_authority_reads_sqlite_records_without_filesystem_authority() {
        let dir = tempfile::tempdir().unwrap();
        write_ticket(dir.path(), "00000000001J2", "Read bridge", "ready");
        let db_path = dir.path().join("workspace.db");
        SqliteTicketBackend::new(&db_path, "workspace-test")
            .import_from_local_backend(&ticket::LocalTicketBackend::new(
                dir.path().join(".yoi/tickets"),
            ))
            .unwrap();
        write_objective(dir.path(), "00000000001J3", "Control plane", "active");
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
        crate::objective_import::import_legacy_objectives_and_memory_staging(
            dir.path(),
            "workspace-test",
            &store,
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
        assert_eq!(
            authority.memory_authority_status(),
            MemoryAuthorityStatus::LegacyFilesystemBackendPendingSqliteAuthority
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
