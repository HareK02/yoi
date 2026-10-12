//! Workspace adapter: Ticket owns assignment identities, current pointers, handling,
//! audit and receipts, including tables with historical Worker-assignment names.
//! Assign/reissue creates a new identity; self-report changes that row using observed
//! event CAS and a complete payload. Exact retry returns a receipt, not a fresh observation.
//!
//! Backend constructs actor/source authority, never from model roles or readable audit
//! text. The whole IMMEDIATE mutation fences the singleton and the binding actually
//! used for authentication, including receipt replay; Worker replacement invalidates
//! the old sender. Account authoring keeps authenticated Account/User and Workspace
//! permissions without requiring a Runtime. Async callers offload the entire helper.
//!
//! Case judgments, handling, delivery/start evidence, Worker lifecycle and grants are
//! separate. Close/reopen does not change handling; the current Orchestrator can report
//! results after close without an active-work grant. Done releases no work or resources.
//! A continuing answer fences current assignment/event and saves input/receipt together;
//! it neither changes handling nor creates an assignment. Done needs explicit reissue.
//!
//! Reads and cursors stay bounded and Workspace/Ticket/assignment scoped. Diagnostics
//! expose neither rendered requests nor binding credentials and attest no model processing.
//! Legacy unknown handling remains unknown: a retained Assign event can be the first
//! report's CAS baseline, not invented waiting or a handling-history cursor.
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

pub(super) fn assignments_page(
    store: &SqliteWorkspaceStore,
    workspace_id: &str,
    ticket_id: &str,
    after_assignment_id: Option<&str>,
    limit: usize,
) -> Result<ticket::TicketAssignmentsPage> {
    store.with_conn(|conn| {
        let tx = conn.unchecked_transaction()?;
        ticket::sqlite_ticket_assignments_page(
            &tx,
            workspace_id,
            ticket_id,
            after_assignment_id,
            limit,
        )
        .map_err(|e| Error::InvalidInput(e.to_string()))
    })
}

pub(super) fn answer(
    store: &SqliteWorkspaceStore,
    workspace_id: &str,
    ticket_id: &str,
    input: &ticket::TicketAssignmentAnswer,
    source: &TicketOrchestratorAssignmentSourceContext,
    actor: &str,
) -> Result<ticket::TicketAssignmentAnswerReceipt> {
    store.with_conn_mut(|conn| {
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        require_assignment_source(&tx,workspace_id,source)?;
        // Replay retains its saved result after assignment replacement; new input
        // must still be admitted by the exact current operational delegation.
        let replay: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM typed_ticket_assignment_tracking_operations WHERE workspace_id=?1 AND operation_key=?2)",params![workspace_id,input.operation_key],|r| r.get(0))?;
        if !replay { assignment_delivery::require_current(&tx,workspace_id,ticket_id,&input.assignment_id)?; }
        let source = serde_json::to_value(source).map_err(|e| Error::Store(e.to_string()))?;
        let result = ticket::sqlite_answer_ticket_assignment_in_transaction(&tx,workspace_id,ticket_id,input,&source,actor).map_err(|e| Error::TicketAssignmentConflict(e.to_string()))?;
        tx.commit()?;
        Ok(result)
    })
}
