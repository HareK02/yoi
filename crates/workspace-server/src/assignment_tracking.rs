//! Workspace adapter: Ticket owns the assignment/tracking schema and mutations.
//! Authenticated source is constructed by Backend after singleton resolution;
//! grants, work releases and startup recovery never derive from tracking status.
use super::*;

/// Backend-authenticated attribution. Readable assigned_by/actor is audit
/// payload, not this authority context. Never decoded from model/API input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TicketOrchestratorAssignmentSourceContext {
    Runtime {
        runtime_id: String,
        worker_id: String,
        agent_key: String,
        authentication_binding_id: String,
    },
    Account {
        account_id: String,
        user_id: String,
    },
}

pub(super) fn require_runtime_source(
    conn: &Connection,
    workspace_id: &str,
    runtime_id: &str,
    worker_id: &str,
    agent_key: &str,
    binding_id: &str,
) -> Result<()> {
    let current_source:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM worker_singleton_owners singleton JOIN workspace_runtime_bindings binding ON binding.workspace_id=singleton.workspace_id AND binding.runtime_id=singleton.runtime_id WHERE singleton.workspace_id=?1 AND singleton.singleton_key=?2 AND singleton.runtime_id=?3 AND singleton.worker_id=?4 AND binding.binding_id=?5 AND binding.revoked_at IS NULL)",params![workspace_id,crate::hosts::WORKSPACE_ORCHESTRATOR_SINGLETON_KEY,runtime_id,worker_id,binding_id],|row|row.get(0))?;
    if agent_key != "workspace-orchestrator" || binding_id.trim().is_empty() || !current_source {
        return Err(Error::WorkspacePermissionDenied("Assignment source is not the current registered Orchestrator with its authenticated Runtime binding".into()));
    }
    Ok(())
}

pub(super) fn require_assignment_source(
    conn: &Connection,
    workspace_id: &str,
    source: &TicketOrchestratorAssignmentSourceContext,
) -> Result<()> {
    match source {
        TicketOrchestratorAssignmentSourceContext::Runtime {
            runtime_id,
            worker_id,
            agent_key,
            authentication_binding_id,
        } => require_runtime_source(
            conn,
            workspace_id,
            runtime_id,
            worker_id,
            agent_key,
            authentication_binding_id,
        ),
        TicketOrchestratorAssignmentSourceContext::Account {
            account_id,
            user_id,
        } => {
            // Backend already authenticates Account/Workspace permission. Do not
            // infer a model role or require a Runtime for account authoring.
            let exists: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM accounts WHERE account_id=?1)",
                [account_id],
                |row| row.get(0),
            )?;
            if !exists || user_id.trim().is_empty() {
                return Err(Error::WorkspacePermissionDenied(
                    "Assignment account source is not a valid Backend actor".into(),
                ));
            }
            Ok(())
        }
    }
}

pub(super) fn update(
    store: &SqliteWorkspaceStore,
    workspace_id: &str,
    ticket_id: &str,
    update: &TicketAssignmentTrackingUpdate,
    source: &TicketAssignmentTrackingSourceContext,
) -> Result<TicketAssignmentTrackingProjection> {
    store.with_conn_mut(|conn| {
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        // Every call, including a receipt replay, fences the authenticated
        // Runtime binding and singleton snapshot under the write lock. Assignment
        // replacement/removal may be replayed, Worker replacement may not.
        require_runtime_source(
            &tx,
            workspace_id,
            &source.runtime_id,
            &source.worker_id,
            &source.agent_key,
            &source.authentication_binding_id,
        )?;
        let result = ticket::sqlite_update_ticket_assignment_tracking_in_transaction(
            &tx,
            workspace_id,
            ticket_id,
            update,
            source,
        )
        .map_err(|error| Error::TicketAssignmentConflict(error.to_string()))?;
        tx.commit()?;
        Ok(result)
    })
}

pub(super) fn get(
    store: &SqliteWorkspaceStore,
    workspace_id: &str,
    ticket_id: &str,
    assignment_id: &str,
) -> Result<Option<TicketAssignmentTrackingProjection>> {
    store.with_conn(|conn| {
        ticket::sqlite_ticket_assignment_tracking(conn, workspace_id, ticket_id, assignment_id)
            .map_err(|error| Error::Store(error.to_string()))
    })
}

pub(super) fn current(
    store: &SqliteWorkspaceStore,
    workspace_id: &str,
    ticket_id: &str,
) -> Result<Vec<TicketAssignmentTrackingProjection>> {
    store.with_conn(|conn| {
        ticket::sqlite_current_ticket_assignment_tracking(conn, workspace_id, ticket_id)
            .map_err(|error| Error::Store(error.to_string()))
    })
}

pub(super) fn history(
    store: &SqliteWorkspaceStore,
    workspace_id: &str,
    ticket_id: &str,
    assignment_id: &str,
) -> Result<Vec<TicketAssignmentTrackingHistoryEntry>> {
    store.with_conn(|conn| {
        ticket::sqlite_ticket_assignment_tracking_history(
            conn,
            workspace_id,
            ticket_id,
            assignment_id,
        )
        .map_err(|error| Error::Store(error.to_string()))
    })
}

pub(super) fn history_page(
    store: &SqliteWorkspaceStore,
    workspace_id: &str,
    ticket_id: &str,
    assignment_id: &str,
    after_event_id: Option<&str>,
    limit: usize,
) -> Result<ticket::TicketAssignmentTrackingHistoryPage> {
    store.with_conn(|conn| {
        ticket::sqlite_ticket_assignment_tracking_history_page(
            conn,
            workspace_id,
            ticket_id,
            assignment_id,
            after_event_id,
            limit,
        )
        .map_err(|error| match error {
            ticket::TicketError::Conflict(message) => Error::InvalidInput(message),
            other => Error::Store(other.to_string()),
        })
    })
}
