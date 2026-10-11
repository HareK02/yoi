//! Durable operational delivery and startup fencing, independent of handling.
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

const INPUTS_SQL: &str = r#"
WITH eligible AS (
 SELECT a.*,o.input_event_id,o.after_ticket_event_index
 FROM ticket_current_worker_assignments c
 JOIN ticket_worker_assignments a ON a.workspace_id=c.workspace_id AND a.assignment_id=c.assignment_id
 JOIN ticket_assignment_delivery_origins o ON o.workspace_id=a.workspace_id AND o.assignment_id=a.assignment_id
 JOIN typed_tickets t ON t.workspace_id=a.workspace_id AND t.ticket_id=a.ticket_id
 WHERE a.workspace_id=?1 AND a.role='orchestrator' AND a.principal_kind='workspace_agent'
 AND a.principal_id='workspace-orchestrator' AND t.workflow_state='open'
 AND json_extract(a.tracking_json,'$.status') IN ('waiting','inprogress')
 AND NOT EXISTS(SELECT 1 FROM ticket_assignment_work_releases r
 WHERE r.workspace_id=a.workspace_id AND r.assignment_id=a.assignment_id)
), inputs AS (
 SELECT ticket_id,assignment_id,input_event_id,tracking_json,-1 AS input_order FROM eligible
 UNION ALL
 SELECT a.ticket_id,a.assignment_id,'ticket-event:' || e.event_index,a.tracking_json,e.event_index
 FROM eligible a JOIN typed_ticket_events e ON e.workspace_id=a.workspace_id AND e.ticket_id=a.ticket_id
 WHERE e.event_index>a.after_ticket_event_index
 -- Continuing request intent is explicit: fenced assignment answers or item content edits.
 -- Ordinary comments and all case/routing/handling history are not new requests.
 AND e.kind IN ('assignment_answer','item_edit')
 AND (e.kind<>'assignment_answer' OR EXISTS(SELECT 1 FROM typed_ticket_event_attributes attr WHERE attr.workspace_id=e.workspace_id AND attr.ticket_id=e.ticket_id AND attr.event_index=e.event_index AND attr.key='orchestrator_assignment_id' AND attr.value=a.assignment_id))
 AND e.kind NOT LIKE 'assignment_tracking%'
 AND e.kind NOT LIKE 'assignment_delivery%'
 AND COALESCE(e.state_field,'') NOT IN ('case','routing','assignment_tracking')
 AND NOT EXISTS(SELECT 1 FROM typed_ticket_event_attributes attr WHERE attr.workspace_id=e.workspace_id AND attr.ticket_id=e.ticket_id AND attr.event_index=e.event_index AND attr.key='orchestrator_assignment_id' AND attr.value<>a.assignment_id)
)
SELECT i.ticket_id,i.assignment_id,i.input_event_id,
 json_extract(i.tracking_json,'$.waiting_reason'),json_extract(i.tracking_json,'$.next_action'),d.payload_json
FROM inputs i
LEFT JOIN ticket_assignment_deliveries d ON d.workspace_id=?1 AND d.assignment_id=i.assignment_id AND d.input_event_id=i.input_event_id
WHERE NOT EXISTS(SELECT 1 FROM ticket_assignment_delivery_acknowledgements ack
 WHERE ack.workspace_id=?1 AND ack.delivery_id=d.delivery_id)
AND (?3 IS NULL OR i.assignment_id=?3) AND (?4 IS NULL OR i.input_event_id=?4)
"#;

fn decode_payload(payload: &str) -> Result<TicketAssignmentDelivery> {
    serde_json::from_str(payload).map_err(|e| Error::Store(e.to_string()))
}

pub(super) fn list_in_tx(
    conn: &Connection,
    workspace_id: &str,
    limit: usize,
) -> Result<Vec<TicketAssignmentDelivery>> {
    inputs_in_tx(conn, workspace_id, limit, None, None)
}
fn inputs_in_tx(
    conn: &Connection,
    workspace_id: &str,
    limit: usize,
    assignment_id: Option<&str>,
    input_event_id: Option<&str>,
) -> Result<Vec<TicketAssignmentDelivery>> {
    let sql = format!("{INPUTS_SQL} ORDER BY i.ticket_id,i.assignment_id,i.input_order LIMIT ?2");
    let mut statement = conn.prepare(&sql)?;
    let rows = statement.query_map(
        params![workspace_id, limit as i64, assignment_id, input_event_id],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
            ))
        },
    )?;
    rows.map(|row| {
        let (ticket_id, assignment_id, input_event_id, waiting_reason, next_action, payload) = row?;
        if let Some(payload) = payload {
            return decode_payload(&payload);
        }
        // Hash the identities, not mutable handling. Scope is Workspace-qualified.
        let identities = serde_json::to_string(&(workspace_id, &assignment_id, &input_event_id))
            .map_err(|e| Error::Store(e.to_string()))?;
        let digest = Sha256::digest(identities.as_bytes());
        let hex = digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let delivery_id = format!("assignment-delivery:{hex}");
        Ok(TicketAssignmentDelivery {
            ticket_id,
            assignment_id,
            input_event_id,
            request_id: delivery_id.clone(),
            delivery_id,
            waiting_reason,
            next_action,
        })
    })
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
        &source.agent_key,
        &source.authentication_binding_id,
    )
}

pub(super) fn require_current(
    conn: &Connection,
    workspace_id: &str,
    ticket_id: &str,
    assignment_id: &str,
) -> Result<()> {
    let current:bool=conn.query_row(r#"SELECT EXISTS(SELECT 1 FROM ticket_current_worker_assignments c
 JOIN ticket_worker_assignments a ON a.workspace_id=c.workspace_id AND a.assignment_id=c.assignment_id
 JOIN typed_tickets t ON t.workspace_id=a.workspace_id AND t.ticket_id=a.ticket_id
 WHERE c.workspace_id=?1 AND c.ticket_id=?2 AND c.assignment_id=?3 AND c.role='orchestrator'
 AND a.principal_kind='workspace_agent' AND a.principal_id='workspace-orchestrator'
 AND t.workflow_state='open' AND json_extract(a.tracking_json,'$.status') IN ('waiting','inprogress')
 AND EXISTS(SELECT 1 FROM ticket_assignment_delivery_origins o WHERE o.workspace_id=a.workspace_id AND o.assignment_id=a.assignment_id)
 AND NOT EXISTS(SELECT 1 FROM ticket_assignment_work_releases r WHERE r.workspace_id=a.workspace_id AND r.assignment_id=a.assignment_id))"#,params![workspace_id,ticket_id,assignment_id],|r|r.get(0))?;
    if !current {
        return Err(Error::TicketAssignmentConflict(
            "Assignment is no longer a current actionable delegation".into(),
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
        require_current(&tx,workspace_id,&ticket_id,assignment_id)?;
        let saved:Option<String>=tx.query_row("SELECT payload_json FROM ticket_assignment_deliveries WHERE workspace_id=?1 AND assignment_id=?2 AND input_event_id=?3",params![workspace_id,assignment_id,input_event_id],|r|r.get(0)).optional()?;
        if let Some(saved)=saved {let result=decode_payload(&saved)?;tx.commit()?;return Ok(result);}
        // A caller cannot fabricate an input. Restrict the same common query by
        // exact assignment/input identity rather than trusting a scan snapshot.
        let candidates=inputs_in_tx(&tx,workspace_id,1,Some(assignment_id),Some(input_event_id))?;
        let delivery=candidates.into_iter().find(|d|d.assignment_id==assignment_id && d.input_event_id==input_event_id)
            .ok_or_else(||Error::TicketAssignmentConflict("Assignment input is not pending".into()))?;
        tx.execute("INSERT INTO ticket_assignment_deliveries VALUES(?1,?2,?3,?4,?5,?6)",params![workspace_id,assignment_id,input_event_id,delivery.delivery_id,serde_json::to_string(&delivery).map_err(|e|Error::Store(e.to_string()))?,serde_json::to_string(source).map_err(|e|Error::Store(e.to_string()))?])?;
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
        require_current(&tx,workspace_id,&delivery.ticket_id,&delivery.assignment_id)?;
        let saved:Option<String>=tx.query_row("SELECT content FROM ticket_assignment_delivery_requests WHERE workspace_id=?1 AND delivery_id=?2",params![workspace_id,delivery_id],|r|r.get(0)).optional()?;
        let result=if let Some(saved)=saved {saved} else {
            tx.execute("INSERT INTO ticket_assignment_delivery_requests VALUES(?1,?2,?3)",params![workspace_id,delivery_id,content])?;
            content.to_owned()
        };
        tx.commit()?;
        Ok(result)
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
        source_guard(&tx,workspace_id,source)?;
        let payload:String=tx.query_row("SELECT payload_json FROM ticket_assignment_deliveries WHERE workspace_id=?1 AND delivery_id=?2",params![workspace_id,delivery_id],|r|r.get(0))?;
        let delivery=decode_payload(&payload)?;
        require_current(&tx,workspace_id,&delivery.ticket_id,&delivery.assignment_id)?;
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
    if let Some((ticket_id, assignment_id, runtime_id, worker_id, agent_key, binding_id)) = fence {
        assignment_tracking::require_runtime_source(
            conn,
            workspace_id,
            &runtime_id,
            &worker_id,
            &agent_key,
            &binding_id,
        )?;
        require_current(conn, workspace_id, &ticket_id, &assignment_id)?;
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
    require_current(conn, workspace_id, ticket_id, assignment_id)?;
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
                source.agent_key,
                source.authentication_binding_id,
                source_json
            ],
        )?;
    }
    Ok(())
}

// Same input selection as delivery, plus immutable delivered history. Never
// SELECT payload/source/content into the public diagnostic projection.
const DIAGNOSTIC_INPUTS_SQL: &str = r#"
WITH inputs AS (
 SELECT o.input_event_id,-1 AS input_order FROM ticket_assignment_delivery_origins o
 WHERE o.workspace_id=?1 AND o.assignment_id=?3
 UNION
 SELECT d.input_event_id,CASE WHEN d.input_event_id LIKE 'ticket-event:%' THEN CAST(substr(d.input_event_id,14) AS INTEGER) ELSE -1 END
 FROM ticket_assignment_deliveries d WHERE d.workspace_id=?1 AND d.assignment_id=?3
 UNION
 SELECT 'ticket-event:' || e.event_index,e.event_index
 FROM ticket_assignment_delivery_origins o JOIN typed_ticket_events e ON e.workspace_id=o.workspace_id AND e.ticket_id=o.ticket_id
 WHERE o.workspace_id=?1 AND o.ticket_id=?2 AND o.assignment_id=?3 AND e.event_index>o.after_ticket_event_index
 AND e.kind IN ('assignment_answer','item_edit')
 AND (e.kind<>'assignment_answer' OR EXISTS(SELECT 1 FROM typed_ticket_event_attributes attr WHERE attr.workspace_id=e.workspace_id AND attr.ticket_id=e.ticket_id AND attr.event_index=e.event_index AND attr.key='orchestrator_assignment_id' AND attr.value=?3))
 AND e.kind NOT LIKE 'assignment_tracking%' AND e.kind NOT LIKE 'assignment_delivery%'
 AND COALESCE(e.state_field,'') NOT IN ('case','routing','assignment_tracking')
 -- Retain historical input identity after done/release. A later delegation's
 -- origin bounds ordinary events; an explicit answer's attribute is authoritative.
 AND e.event_index<=COALESCE((SELECT MIN(later.after_ticket_event_index) FROM ticket_assignment_delivery_origins later
 JOIN ticket_worker_assignments la ON la.workspace_id=later.workspace_id AND la.assignment_id=later.assignment_id
 JOIN ticket_worker_assignments original ON original.workspace_id=o.workspace_id AND original.assignment_id=o.assignment_id
 WHERE later.workspace_id=o.workspace_id AND later.ticket_id=o.ticket_id AND la.rowid>original.rowid),9223372036854775807)
 AND NOT EXISTS(SELECT 1 FROM typed_ticket_event_attributes attr WHERE attr.workspace_id=e.workspace_id AND attr.ticket_id=e.ticket_id AND attr.event_index=e.event_index AND attr.key='orchestrator_assignment_id' AND attr.value<>?3)
)
"#;

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
        let after = match after_input_event_id {
            None => -2,
            Some(id) => tx.query_row(&format!("{DIAGNOSTIC_INPUTS_SQL} SELECT input_order FROM inputs WHERE input_event_id=?5"),params![workspace_id,ticket_id,assignment_id,actionable,id],|r| r.get::<_,i64>(0)).optional()?.ok_or_else(|| Error::InvalidInput("Input cursor does not belong to this assignment".into()))?,
        };
        let sql = format!("{DIAGNOSTIC_INPUTS_SQL} SELECT i.input_event_id,d.delivery_id,EXISTS(SELECT 1 FROM ticket_assignment_delivery_requests req WHERE req.workspace_id=?1 AND req.delivery_id=d.delivery_id),EXISTS(SELECT 1 FROM ticket_assignment_delivery_acknowledgements ack WHERE ack.workspace_id=?1 AND ack.delivery_id=d.delivery_id) FROM inputs i LEFT JOIN ticket_assignment_deliveries d ON d.workspace_id=?1 AND d.assignment_id=?3 AND d.input_event_id=i.input_event_id WHERE i.input_order>?5 ORDER BY i.input_order LIMIT ?6");
        let mut stmt = tx.prepare(&sql)?;
        let mut inputs = stmt.query_map(params![workspace_id,ticket_id,assignment_id,actionable,after,(limit+1) as i64],|r| {
            let delivery_id: Option<String> = r.get(1)?;
            let prepared: bool = r.get(2)?;
            let ack: bool = r.get(3)?;
            use server_api::TicketAssignmentInputDeliveryState::*;
            let notification = if ack { RuntimeAccepted } else if prepared { RequestPersisted } else if delivery_id.is_some() { Reserved } else { NotReserved };
            Ok(server_api::TicketAssignmentInputDiagnostic { input_event_id:r.get(0)?, delivery_id,notification,acceptance_outcome_unknown:prepared && !ack })
        })?.collect::<std::result::Result<Vec<_>,_>>()?;
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
