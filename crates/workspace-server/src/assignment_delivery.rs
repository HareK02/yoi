//! Durable operational delivery and startup fencing, independent of Ticket handling.
//!
//! Notification eligibility comes from the shared routing policy and its persisted decisions.
//! This adapter retains the original assignment delivery identity, immutable request and ACK.
//! Retained migration SQL establishes origins only for reconciled assignment-specific legacy
//! Queue fences; migration does not reinterpret historical comments as new requests.
//!
//! Reserve and prepare recheck delegation, Subject body and live binding under
//! IMMEDIATE ownership. ACK instead verifies the target authorized before I/O;
//! a later responsibility change must not erase Runtime acceptance evidence.
//! Persist the entire rendered request before Runtime I/O, then reuse
//! its request ID and content on response loss. Send outside the transaction; acknowledgement
//! proves Runtime acceptance, not model consideration, execution, handling or fulfillment.
//! Async callers offload whole helpers, never split authorization/receipt/CAS/audit across awaits.
//!
//! Orchestrator starts require the observed assignment and the request's authenticated binding,
//! not a newly resolved binding or model role. Saved start fences extend existing allocation
//! receipts through reserve/retry, bind, finalization and recovery. Unknown external outcomes
//! retain resource fences for reconciliation, not a second start. Manual authorized starts
//! omit delegation; unresolved dependencies are diagnostics, not blanket start gates.
//! Clear/reassign does not stop or transfer running work; reopen alone revives no old delegation.
//! Immutable operational receipts are deleted only by the guarded Workspace purge.
use super::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketAssignmentDelivery {
    pub ticket_id: String,
    pub assignment_id: String,
    pub input_event_id: String,
    pub delivery_id: String,
    pub request_id: String,
    pub waiting_reason: Option<String>,
    pub next_action: Option<String>,
}

pub(super) const SCHEMA_SQL: &str = r#"
CREATE TABLE ticket_assignment_delivery_origins (
 workspace_id TEXT NOT NULL, ticket_id TEXT NOT NULL, assignment_id TEXT NOT NULL,
 input_event_id TEXT NOT NULL, after_ticket_event_index INTEGER NOT NULL,
 PRIMARY KEY(workspace_id,assignment_id),
 FOREIGN KEY(workspace_id,ticket_id,assignment_id) REFERENCES ticket_worker_assignments(workspace_id,ticket_id,assignment_id)
);
CREATE TABLE ticket_assignment_deliveries (
 workspace_id TEXT NOT NULL, assignment_id TEXT NOT NULL, input_event_id TEXT NOT NULL,
 delivery_id TEXT NOT NULL, payload_json TEXT NOT NULL CHECK(json_valid(payload_json)),
 reserved_source_json TEXT NOT NULL CHECK(json_valid(reserved_source_json)),
 PRIMARY KEY(workspace_id,assignment_id,input_event_id), UNIQUE(workspace_id,delivery_id),
 FOREIGN KEY(workspace_id,assignment_id) REFERENCES ticket_worker_assignments(workspace_id,assignment_id)
);
CREATE TABLE ticket_assignment_delivery_requests (
 workspace_id TEXT NOT NULL, delivery_id TEXT NOT NULL, content TEXT NOT NULL,
 PRIMARY KEY(workspace_id,delivery_id),
 FOREIGN KEY(workspace_id,delivery_id) REFERENCES ticket_assignment_deliveries(workspace_id,delivery_id)
);
CREATE TRIGGER ticket_assignment_delivery_request_no_update BEFORE UPDATE ON ticket_assignment_delivery_requests
BEGIN SELECT RAISE(ABORT,'Assignment delivery request is immutable'); END;
CREATE TRIGGER ticket_assignment_delivery_request_no_delete BEFORE DELETE ON ticket_assignment_delivery_requests
BEGIN SELECT RAISE(ABORT,'Assignment delivery request is immutable'); END;
CREATE TABLE ticket_assignment_delivery_acknowledgements (
 workspace_id TEXT NOT NULL, delivery_id TEXT NOT NULL,
 source_json TEXT NOT NULL CHECK(json_valid(source_json)), PRIMARY KEY(workspace_id,delivery_id),
 FOREIGN KEY(workspace_id,delivery_id) REFERENCES ticket_assignment_deliveries(workspace_id,delivery_id)
);
CREATE TABLE ticket_assignment_operation_fences (
 workspace_id TEXT NOT NULL, operation_id TEXT NOT NULL, orchestrator_assignment_id TEXT NOT NULL,
 runtime_id TEXT NOT NULL, worker_id TEXT NOT NULL, agent_key TEXT NOT NULL,
 authentication_binding_id TEXT NOT NULL, source_json TEXT NOT NULL CHECK(json_valid(source_json)),
 PRIMARY KEY(workspace_id,operation_id),
 FOREIGN KEY(workspace_id,operation_id) REFERENCES ticket_assignment_operations(workspace_id,operation_id),
 FOREIGN KEY(workspace_id,orchestrator_assignment_id) REFERENCES ticket_worker_assignments(workspace_id,assignment_id)
);
CREATE TRIGGER ticket_assignment_delivery_origin_insert
AFTER INSERT ON typed_ticket_assignment_tracking_history
WHEN json_extract(NEW.source_context_json,'$.kind')='assignment'
BEGIN
 INSERT INTO ticket_assignment_delivery_origins
 VALUES(NEW.workspace_id,NEW.ticket_id,NEW.assignment_id,'assignment-event:' || NEW.event_id,
 COALESCE((SELECT MAX(event_index) FROM typed_ticket_events
 WHERE workspace_id=NEW.workspace_id AND ticket_id=NEW.ticket_id),-1));
END;
CREATE TRIGGER ticket_assignment_delivery_payload_no_update BEFORE UPDATE ON ticket_assignment_deliveries
BEGIN SELECT RAISE(ABORT,'Assignment delivery payload is immutable'); END;
CREATE TRIGGER ticket_assignment_delivery_payload_no_delete BEFORE DELETE ON ticket_assignment_deliveries
BEGIN SELECT RAISE(ABORT,'Assignment delivery payload is immutable'); END;
CREATE TRIGGER ticket_assignment_delivery_ack_no_update BEFORE UPDATE ON ticket_assignment_delivery_acknowledgements
BEGIN SELECT RAISE(ABORT,'Assignment delivery acknowledgement is immutable'); END;
CREATE TRIGGER ticket_assignment_delivery_ack_no_delete BEFORE DELETE ON ticket_assignment_delivery_acknowledgements
BEGIN SELECT RAISE(ABORT,'Assignment delivery acknowledgement is immutable'); END;
CREATE TRIGGER ticket_assignment_operation_fence_no_update BEFORE UPDATE ON ticket_assignment_operation_fences
BEGIN SELECT RAISE(ABORT,'Assignment startup fence is immutable'); END;
CREATE TRIGGER ticket_assignment_operation_fence_no_delete BEFORE DELETE ON ticket_assignment_operation_fences
BEGIN SELECT RAISE(ABORT,'Assignment startup fence is immutable'); END;
CREATE TRIGGER ticket_assignment_delivery_origin_no_update BEFORE UPDATE ON ticket_assignment_delivery_origins
BEGIN SELECT RAISE(ABORT,'Assignment input origin is immutable'); END;
CREATE TRIGGER ticket_assignment_delivery_origin_no_delete BEFORE DELETE ON ticket_assignment_delivery_origins
BEGIN SELECT RAISE(ABORT,'Assignment input origin is immutable'); END;
"#;

/// Additive Workspace schema only. Native pre-upgrade assignments use their
/// committed initial audit time to exclude earlier Ticket inputs; ambiguous
/// legacy relationships are NOT treated as fresh delegations. Migration inputs
/// retain the exact Queue fence already reconciled by Ticket schema 11.
pub(super) fn install(conn: &Connection) -> Result<()> {
    conn.execute_batch(SCHEMA_SQL)?;
    conn.execute_batch(r#"
INSERT INTO ticket_assignment_delivery_origins
SELECT h.workspace_id,h.ticket_id,h.assignment_id,'assignment-event:' || h.event_id,
 COALESCE((SELECT MAX(e.event_index) FROM typed_ticket_events e
 WHERE e.workspace_id=h.workspace_id AND e.ticket_id=h.ticket_id AND e.at<h.at),-1)
FROM typed_ticket_assignment_tracking_history h
WHERE json_extract(h.source_context_json,'$.kind')='assignment';
INSERT INTO ticket_assignment_delivery_origins
SELECT h.workspace_id,h.ticket_id,h.assignment_id,'assignment-event:' || h.event_id,
 (SELECT MAX(e.event_index) FROM typed_ticket_events e
 JOIN typed_ticket_event_attributes f ON f.workspace_id=e.workspace_id AND f.ticket_id=e.ticket_id AND f.event_index=e.event_index
 WHERE e.workspace_id=h.workspace_id AND e.ticket_id=h.ticket_id
 AND f.key='orchestrator_assignment_id' AND f.value=h.assignment_id
 AND (e.to_state='queued' OR (e.state_field='routing' AND e.reason='queued')))
FROM typed_ticket_assignment_tracking_history h
WHERE json_extract(h.source_context_json,'$.kind')='migration'
 AND EXISTS(SELECT 1 FROM typed_ticket_events e JOIN typed_ticket_event_attributes f
 ON f.workspace_id=e.workspace_id AND f.ticket_id=e.ticket_id AND f.event_index=e.event_index
 WHERE e.workspace_id=h.workspace_id AND e.ticket_id=h.ticket_id
 AND f.key='orchestrator_assignment_id' AND f.value=h.assignment_id
 AND (e.to_state='queued' OR (e.state_field='routing' AND e.reason='queued')));
"#)?;
    Ok(())
}

/// Additive schema94 composition; keep the applied schema93 install unchanged.
pub(super) fn install_post_inputs(conn: &Connection) -> Result<()> {
    conn.execute_batch(include_str!("assignment_content_inputs.sql"))?;
    Ok(())
}

fn decode_payload(payload: &str) -> Result<TicketAssignmentDelivery> {
    serde_json::from_str(payload).map_err(|e| Error::Store(e.to_string()))
}

pub(super) fn list_in_tx(
    conn: &Connection,
    workspace_id: &str,
    limit: usize,
    subject_id: Option<&str>,
) -> Result<Vec<TicketAssignmentDelivery>> {
    if !(1..=100).contains(&limit) {
        return Err(Error::InvalidInput("Delivery limit must be 1..100".into()));
    }
    notification_routing::pending_sources_for_subject(
        conn,
        workspace_id,
        "orchestrator",
        limit,
        subject_id,
    )?
    .into_iter()
    .map(|(ticket, assignment, input, id)| {
        delivery(conn, workspace_id, ticket, assignment, input, id)
    })
    .collect()
}
fn delivery(
    conn: &Connection,
    workspace: &str,
    ticket_id: String,
    assignment_id: String,
    input_event_id: String,
    delivery_id: String,
) -> Result<TicketAssignmentDelivery> {
    let saved: Option<String> = conn.query_row("SELECT payload_json FROM ticket_assignment_deliveries WHERE workspace_id=?1 AND assignment_id=?2 AND input_event_id=?3",params![workspace,assignment_id,input_event_id],|r|r.get(0)).optional()?;
    if let Some(saved) = saved {
        return decode_payload(&saved);
    }
    let (waiting_reason,next_action)=conn.query_row("SELECT json_extract(tracking_json,'$.waiting_reason'),json_extract(tracking_json,'$.next_action') FROM ticket_worker_assignments WHERE workspace_id=?1 AND assignment_id=?2",params![workspace,assignment_id],|r|Ok((r.get(0)?,r.get(1)?)))?;
    Ok(TicketAssignmentDelivery {
        ticket_id,
        assignment_id,
        input_event_id,
        request_id: delivery_id.clone(),
        delivery_id,
        waiting_reason,
        next_action,
    })
}
fn inputs_in_tx(
    conn: &Connection,
    workspace: &str,
    _limit: usize,
    assignment: Option<&str>,
    input: Option<&str>,
) -> Result<Vec<TicketAssignmentDelivery>> {
    notification_routing::route_pending(conn, workspace)?;
    let row: Option<(String,String,String,String)>=conn.query_row("SELECT s.ticket_id,s.assignment_id,s.input_event_id,d.delivery_id FROM notification_sources s JOIN notification_decisions d ON d.source_sequence=s.sequence WHERE s.workspace_id=?1 AND s.assignment_id=?2 AND s.input_event_id=?3 AND s.role='orchestrator' AND d.disposition='deliver' AND NOT EXISTS(SELECT 1 FROM ticket_assignment_delivery_acknowledgements a WHERE a.workspace_id=?1 AND a.delivery_id=d.delivery_id)",params![workspace,assignment,input],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
    row.into_iter()
        .map(|(ticket, a, i, id)| delivery(conn, workspace, ticket, a, i, id))
        .collect()
}

fn source_guard(
    conn: &Connection,
    workspace_id: &str,
    source: &TicketAssignmentTrackingSourceContext,
) -> Result<()> {
    assignment_tracking::require_runtime_source(
        conn,
        workspace_id,
        &source.runtime_id,
        &source.worker_id,
        &source.subject_id,
        &source.authentication_binding_id,
    )
}

pub(super) fn require_current(
    conn: &Connection,
    workspace_id: &str,
    ticket_id: &str,
    assignment_id: &str,
) -> Result<()> {
    notification_routing::require_current(
        conn,
        workspace_id,
        ticket_id,
        assignment_id,
        "orchestrator",
    )?;
    let known: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM ticket_assignment_delivery_origins WHERE workspace_id=?1 AND assignment_id=?2)", params![workspace_id, assignment_id], |r|r.get(0))?;
    if !known {
        return Err(Error::TicketAssignmentConflict(
            "Assignment has no committed input origin".into(),
        ));
    }
    Ok(())
}

// Runtime source authentication alone is insufficient: two live Subject bodies can
// coexist in one Workspace. Fence the recipient's Subject before any receipt replay
// or target authorization, in the same write transaction as the delivery/start.
fn require_assignment_target(
    conn: &Connection,
    workspace_id: &str,
    ticket_id: &str,
    assignment_id: &str,
    subject_id: &str,
) -> Result<()> {
    require_current(conn, workspace_id, ticket_id, assignment_id)?;
    let matches: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM ticket_worker_assignments WHERE workspace_id=?1 AND ticket_id=?2 AND assignment_id=?3 AND role='orchestrator' AND principal_kind='subject' AND principal_id=?4)",
        params![workspace_id, ticket_id, assignment_id, subject_id],
        |row| row.get(0),
    )?;
    if !matches {
        return Err(Error::WorkspacePermissionDenied(
            "Delivery/start target Subject differs from the current assignment".into(),
        ));
    }
    Ok(())
}

// Call only after source_guard and require_assignment_target in the reservation/preparation
// transaction. A replacement target gets its own evidence for the same delivery,
// not a new request or authority to revive a completed/released assignment.
fn save_authorized_target(
    conn: &Connection,
    workspace: &str,
    delivery: &str,
    source: &TicketAssignmentTrackingSourceContext,
) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO notification_assignment_targets VALUES(?1,?2,?3,?4,?5,?6)",
        params![
            workspace,
            delivery,
            source.runtime_id,
            source.worker_id,
            source.subject_id,
            source.authentication_binding_id
        ],
    )?;
    Ok(())
}

fn is_acknowledged(conn: &Connection, workspace: &str, delivery: &str) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM ticket_assignment_delivery_acknowledgements WHERE workspace_id=?1 AND delivery_id=?2)",
        params![workspace, delivery], |row| row.get(0),
    )?)
}

fn require_unacknowledged(conn: &Connection, workspace: &str, delivery: &str) -> Result<()> {
    if is_acknowledged(conn, workspace, delivery)? {
        return Err(Error::TicketAssignmentConflict(
            "Assignment delivery already has Runtime acceptance; new send admission is closed"
                .into(),
        ));
    }
    Ok(())
}

pub(super) fn reserve(
    store: &SqliteWorkspaceStore,
    workspace_id: &str,
    assignment_id: &str,
    input_event_id: &str,
    source: &TicketAssignmentTrackingSourceContext,
) -> Result<TicketAssignmentDelivery> {
    store.with_conn_mut(|conn| {
        let tx=conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        source_guard(&tx,workspace_id,source)?;
        let ticket_id:String=tx.query_row("SELECT ticket_id FROM ticket_worker_assignments WHERE workspace_id=?1 AND assignment_id=?2",params![workspace_id,assignment_id],|r|r.get(0))?;
        require_assignment_target(&tx,workspace_id,&ticket_id,assignment_id,&source.subject_id)?;
        let saved:Option<String>=tx.query_row("SELECT payload_json FROM ticket_assignment_deliveries WHERE workspace_id=?1 AND assignment_id=?2 AND input_event_id=?3",params![workspace_id,assignment_id,input_event_id],|r|r.get(0)).optional()?;
        if let Some(saved)=saved {let result=decode_payload(&saved)?;require_unacknowledged(&tx,workspace_id,&result.delivery_id)?;save_authorized_target(&tx,workspace_id,&result.delivery_id,source)?;tx.commit()?;return Ok(result);}
        // A caller cannot fabricate an input. Restrict the same common query by
        // exact assignment/input identity rather than trusting a scan snapshot.
        let candidates=inputs_in_tx(&tx,workspace_id,1,Some(assignment_id),Some(input_event_id))?;
        let delivery=candidates.into_iter().find(|d|d.assignment_id==assignment_id && d.input_event_id==input_event_id)
            .ok_or_else(||Error::TicketAssignmentConflict("Assignment input is not pending".into()))?;
        tx.execute("INSERT INTO ticket_assignment_deliveries VALUES(?1,?2,?3,?4,?5,?6)",params![workspace_id,assignment_id,input_event_id,delivery.delivery_id,serde_json::to_string(&delivery).map_err(|e|Error::Store(e.to_string()))?,serde_json::to_string(source).map_err(|e|Error::Store(e.to_string()))?])?;
        save_authorized_target(&tx,workspace_id,&delivery.delivery_id,source)?;
        tx.commit()?;
        Ok(delivery)
    })
}

/// Persist the exact rendered Runtime request before I/O. A losing renderer or
/// retry receives the original bytes, not a new payload under the same ID.
pub(super) fn prepare_content(
    store: &SqliteWorkspaceStore,
    workspace_id: &str,
    delivery_id: &str,
    content: &str,
    source: &TicketAssignmentTrackingSourceContext,
) -> Result<String> {
    if content.is_empty() || content.len() > 262144 || content.contains('\0') {
        return Err(Error::InvalidInput(
            "Assignment delivery content must be nonempty, NUL-free and at most 262144 bytes"
                .into(),
        ));
    }
    store.with_conn_mut(|conn| {
        let tx=conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        source_guard(&tx,workspace_id,source)?;
        let payload:String=tx.query_row("SELECT payload_json FROM ticket_assignment_deliveries WHERE workspace_id=?1 AND delivery_id=?2",params![workspace_id,delivery_id],|r|r.get(0))?;
        let delivery=decode_payload(&payload)?;
        require_assignment_target(&tx,workspace_id,&delivery.ticket_id,&delivery.assignment_id,&source.subject_id)?;
        require_unacknowledged(&tx,workspace_id,delivery_id)?;
        save_authorized_target(&tx,workspace_id,delivery_id,source)?;
        let saved:Option<String>=tx.query_row("SELECT content FROM ticket_assignment_delivery_requests WHERE workspace_id=?1 AND delivery_id=?2",params![workspace_id,delivery_id],|r|r.get(0)).optional()?;
        let result=if let Some(saved)=saved {saved} else {
            tx.execute("INSERT INTO ticket_assignment_delivery_requests VALUES(?1,?2,?3)",params![workspace_id,delivery_id,content])?;
            content.to_owned()
        };
        tx.commit()?;
        Ok(result)
    })
}

/// Final send admission is distinct from reading a receipt or replaying an ACK.
/// Recheck the current Subject/assignment and prepared target, then return only
/// the pinned bytes. A known ACK closes admission; an unknown outcome keeps the
/// same request eligible. The caller still authorizes live policy immediately
/// before Runtime I/O; the transaction does not serialize external acceptance.
pub(super) fn admit_send(
    store: &SqliteWorkspaceStore,
    workspace_id: &str,
    delivery_id: &str,
    source: &TicketAssignmentTrackingSourceContext,
) -> Result<Option<String>> {
    store.with_conn_mut(|conn| {
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        source_guard(&tx, workspace_id, source)?;
        let payload: String = tx.query_row(
            "SELECT payload_json FROM ticket_assignment_deliveries WHERE workspace_id=?1 AND delivery_id=?2",
            params![workspace_id, delivery_id], |row| row.get(0),
        )?;
        let delivery = decode_payload(&payload)?;
        require_assignment_target(&tx, workspace_id, &delivery.ticket_id, &delivery.assignment_id, &source.subject_id)?;
        let authorized: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM notification_assignment_targets WHERE workspace_id=?1 AND delivery_id=?2 AND runtime_id=?3 AND worker_id=?4 AND agent_key=?5 AND authentication_binding_id=?6)",
            params![workspace_id, delivery_id, source.runtime_id, source.worker_id, source.subject_id, source.authentication_binding_id], |row| row.get(0),
        )?;
        if !authorized {
            return Err(Error::WorkspacePermissionDenied("Assignment send target was not prepared or reserved for this delivery".into()));
        }
        let content = if is_acknowledged(&tx, workspace_id, delivery_id)? {
            None
        } else {
            Some(tx.query_row(
                "SELECT content FROM ticket_assignment_delivery_requests WHERE workspace_id=?1 AND delivery_id=?2",
                params![workspace_id, delivery_id], |row| row.get(0),
            ).optional()?.ok_or_else(|| Error::TicketAssignmentConflict("Assignment send requires a persisted request".into()))?)
        };
        tx.commit()?;
        Ok(content)
    })
}

pub(super) fn ack(
    store: &SqliteWorkspaceStore,
    workspace_id: &str,
    delivery_id: &str,
    source: &TicketAssignmentTrackingSourceContext,
) -> Result<()> {
    store.with_conn_mut(|conn| {
        let tx=conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        // This is an acceptance receipt, not another send authorization. Match all
        // authenticated target identity fields against evidence saved while current.
        // Actor is audit attribution; it is not target authority. Existing ACK bytes
        // still have to match on replay and are never replaced.
        let authorized:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM notification_assignment_targets WHERE workspace_id=?1 AND delivery_id=?2 AND runtime_id=?3 AND worker_id=?4 AND agent_key=?5 AND authentication_binding_id=?6)",params![workspace_id,delivery_id,source.runtime_id,source.worker_id,source.subject_id,source.authentication_binding_id],|r|r.get(0))?;
        if !authorized {return Err(Error::WorkspacePermissionDenied("Acknowledgement target was not authorized for this delivery".into()));}
        let source_json=serde_json::to_string(source).map_err(|e|Error::Store(e.to_string()))?;
        let saved:Option<String>=tx.query_row("SELECT source_json FROM ticket_assignment_delivery_acknowledgements WHERE workspace_id=?1 AND delivery_id=?2",params![workspace_id,delivery_id],|r|r.get(0)).optional()?;
        if let Some(saved)=saved {
            if saved!=source_json {return Err(Error::TicketAssignmentConflict("Delivery acknowledgement source differs from saved receipt".into()));}
        } else {tx.execute("INSERT INTO ticket_assignment_delivery_acknowledgements VALUES(?1,?2,?3)",params![workspace_id,delivery_id,source_json])?;}
        tx.commit()?;
        Ok(())
    })
}

/// Must run before receipt replay as well as every bind/finalize write. Absence
/// is the existing manual reservation contract, not inferred delegation.
pub(super) fn operation_guard(
    conn: &Connection,
    workspace_id: &str,
    operation_id: &str,
) -> Result<()> {
    let fence=conn.query_row("SELECT o.ticket_id,f.orchestrator_assignment_id,f.runtime_id,f.worker_id,f.agent_key,f.authentication_binding_id FROM ticket_assignment_operation_fences f JOIN ticket_assignment_operations o ON o.workspace_id=f.workspace_id AND o.operation_id=f.operation_id WHERE f.workspace_id=?1 AND f.operation_id=?2",params![workspace_id,operation_id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?,r.get::<_,String>(5)?))).optional()?;
    if let Some((ticket_id, assignment_id, runtime_id, worker_id, subject_id, binding_id)) = fence {
        assignment_tracking::require_runtime_source(
            conn,
            workspace_id,
            &runtime_id,
            &worker_id,
            &subject_id,
            &binding_id,
        )?;
        require_assignment_target(conn, workspace_id, &ticket_id, &assignment_id, &subject_id)?;
    }
    Ok(())
}

pub(super) fn save_operation_fence(
    conn: &Connection,
    workspace_id: &str,
    operation_id: &str,
    ticket_id: &str,
    assignment_id: &str,
    source: &TicketAssignmentTrackingSourceContext,
    existing: bool,
) -> Result<()> {
    source_guard(conn, workspace_id, source)?;
    require_assignment_target(
        conn,
        workspace_id,
        ticket_id,
        assignment_id,
        &source.subject_id,
    )?;
    let source_json = serde_json::to_string(source).map_err(|e| Error::Store(e.to_string()))?;
    let saved:Option<(String,String)>=conn.query_row("SELECT orchestrator_assignment_id,source_json FROM ticket_assignment_operation_fences WHERE workspace_id=?1 AND operation_id=?2",params![workspace_id,operation_id],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    if let Some(saved) = saved {
        if saved != (assignment_id.to_owned(), source_json) {
            return Err(Error::TicketAssignmentConflict(
                "Startup operation assignment/source differs from saved fence".into(),
            ));
        }
    } else {
        if existing {
            return Err(Error::TicketAssignmentConflict(
                "Cannot add a delegation fence to an existing manual operation".into(),
            ));
        }
        conn.execute(
            "INSERT INTO ticket_assignment_operation_fences VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            params![
                workspace_id,
                operation_id,
                assignment_id,
                source.runtime_id,
                source.worker_id,
                source.subject_id,
                source.authentication_binding_id,
                source_json
            ],
        )?;
    }
    Ok(())
}

pub(super) fn diagnostics_page(
    store: &SqliteWorkspaceStore,
    workspace_id: &str,
    ticket_id: &str,
    assignment_id: &str,
    after_input_event_id: Option<&str>,
    after_start_operation_id: Option<&str>,
    limit: usize,
) -> Result<server_api::TicketAssignmentDiagnosticsPage> {
    if !(1..=100).contains(&limit) {
        return Err(Error::InvalidInput(
            "Diagnostic page limit must be 1..100".into(),
        ));
    }
    store.with_conn(|conn| {
        // One read snapshot for current assignment, evidence and cursor validation.
        let tx = conn.unchecked_transaction()?;
        let assignment = ticket::sqlite_ticket_assignment_tracking(&tx,workspace_id,ticket_id,assignment_id)
            .map_err(|e| Error::Store(e.to_string()))?.ok_or_else(|| Error::InvalidInput("Assignment does not belong to this Ticket".into()))?;
        let known: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM ticket_assignment_delivery_origins WHERE workspace_id=?1 AND ticket_id=?2 AND assignment_id=?3)",params![workspace_id,ticket_id,assignment_id],|r| r.get(0))?;
        let actionable = match require_current(&tx,workspace_id,ticket_id,assignment_id) {
            Ok(()) => true,
            Err(Error::TicketAssignmentConflict(_)) => false,
            Err(error) => return Err(error),
        };
        let mut inputs = notification_routing::diagnostic_inputs(
            &tx, workspace_id, ticket_id, assignment_id, after_input_event_id, limit,
        )?;
        let more = inputs.len()>limit;
        inputs.truncate(limit);
        let next_input_event_id = if more { inputs.last().map(|i| i.input_event_id.clone()) } else { None };
        let after_start = match after_start_operation_id {
            None => 0,
            Some(id) => tx.query_row("SELECT f.rowid FROM ticket_assignment_operation_fences f JOIN ticket_assignment_operations o ON o.workspace_id=f.workspace_id AND o.operation_id=f.operation_id WHERE f.workspace_id=?1 AND f.orchestrator_assignment_id=?3 AND o.ticket_id=?2 AND f.operation_id=?4",params![workspace_id,ticket_id,assignment_id,id],|r| r.get::<_,i64>(0)).optional()?.ok_or_else(|| Error::InvalidInput("Start cursor does not belong to this assignment".into()))?,
        };
        let mut stmt = tx.prepare("SELECT f.operation_id,o.claim_state,o.assignment_id,o.runtime_id,o.worker_id FROM ticket_assignment_operation_fences f JOIN ticket_assignment_operations o ON o.workspace_id=f.workspace_id AND o.operation_id=f.operation_id WHERE f.workspace_id=?1 AND o.ticket_id=?2 AND f.orchestrator_assignment_id=?3 AND f.rowid>?4 ORDER BY f.rowid LIMIT ?5")?;
        let mut starts = stmt.query_map(params![workspace_id,ticket_id,assignment_id,after_start,(limit+1) as i64],|r| Ok(server_api::TicketAssignmentStartDiagnostic { operation_id:r.get(0)?,claim_state:r.get(1)?,assignment_id:r.get(2)?,runtime_id:r.get(3)?,worker_id:r.get(4)?,actual_processing_unknown:true }))?.collect::<std::result::Result<Vec<_>,_>>()?;
        let more = starts.len()>limit;
        starts.truncate(limit);
        let next_start_operation_id = if more { starts.last().map(|i| i.operation_id.clone()) } else { None };
        Ok(server_api::TicketAssignmentDiagnosticsPage { assignment_id:assignment_id.into(),current:assignment.current,input_origin_known:known,actionable,inputs,next_input_event_id,starts,next_start_operation_id })
    })
}
