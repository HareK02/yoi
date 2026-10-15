//! Implementation results are authenticated facts, not Orchestrator answers or handling updates.
//! Reports belong to their authenticated Workspace Worker and Ticket, not an assignment.
//! Replays preserve the original result and notification recipient even after reassignment.

fn decode_result_request(raw: &str) -> Result<server_api::TicketImplementationResultRequest> {
    // Retained pre-v98 requests keep their original bytes for audit. Assignment
    // fields are historical metadata, not inputs or authorization for a new report.
    let mut value: serde_json::Value =
        serde_json::from_str(raw).map_err(|e| Error::Store(e.to_string()))?;
    if let Some(object) = value.as_object_mut() {
        object.remove("assignment_id");
        object.remove("orchestrator_assignment_id");
    }
    serde_json::from_value(value).map_err(|e| Error::Store(e.to_string()))
}
use super::*;

impl SqliteWorkspaceStore {
    pub(crate) fn implementation_result(
        &self,
        workspace: &str,
        ticket: &str,
        result: &str,
    ) -> Result<server_api::TicketImplementationResultReceipt> {
        self.with_conn(|conn| {
            let saved: Option<(String,String)> = conn.query_row("SELECT request_json,created_at FROM ticket_implementation_results WHERE workspace_id=?1 AND ticket_id=?2 AND result_id=?3",params![workspace,ticket,result],|r| Ok((r.get(0)?,r.get(1)?))).optional()?;
            let (request,created_at) = saved.ok_or_else(|| Error::InvalidInput("Implementation result not found in this Ticket".into()))?;
            Ok(server_api::TicketImplementationResultReceipt { result_id: result.into(), ticket_id: ticket.into(), request: decode_result_request(&request)?, created_at })
        })
    }
    pub(crate) fn record_implementation_result(
        &self,
        workspace: &str,
        ticket: &str,
        request: &server_api::TicketImplementationResultRequest,
        worker: &RuntimeWorkerRef,
        binding: &str,
    ) -> Result<server_api::TicketImplementationResultReceipt> {
        for (name, value) in [("operation_key", &request.operation_key)] {
            if value.trim().is_empty() || value.len() > 1024 || value.chars().any(char::is_control)
            {
                return Err(Error::InvalidInput(format!(
                    "{name} must be nonempty, control-free and at most 1024 bytes"
                )));
            }
        }
        if request.body.trim().is_empty()
            || request.body.len() > 16384
            || request.body.contains('\0')
        {
            return Err(Error::InvalidInput(
                "Result body must be nonblank, NUL-free and at most 16384 bytes".into(),
            ));
        }
        let encoded = serde_json::to_string(request).map_err(|e| Error::Store(e.to_string()))?;
        self.with_conn_mut(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let authenticated: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM workspace_runtime_bindings b JOIN worker_registry w ON w.workspace_id=b.workspace_id AND w.runtime_id=b.runtime_id WHERE b.workspace_id=?1 AND b.runtime_id=?2 AND b.binding_id=?3 AND b.revoked_at IS NULL AND w.worker_id=?4)", params![workspace,worker.runtime_id,binding,worker.worker_id], |r| r.get(0))?;
            if !authenticated {
                return Err(Error::WorkspacePermissionDenied("Implementation result requires its authenticated Runtime binding".into()));
            }
            let saved: Option<(String,String,String,String,String,String)> = tx.query_row("SELECT result_id,ticket_id,request_json,runtime_id,worker_id,created_at FROM ticket_implementation_results WHERE workspace_id=?1 AND operation_key=?2",params![workspace,request.operation_key], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).optional()?;
            if let Some((result_id,saved_ticket,saved_request,runtime,source,created_at)) = saved {
                if saved_ticket != ticket || decode_result_request(&saved_request)? != *request || runtime != worker.runtime_id || source != worker.worker_id {
                    return Err(Error::TicketAssignmentConflict("Implementation result operation key was used for different input or source".into()));
                }
                return Ok(server_api::TicketImplementationResultReceipt { result_id, ticket_id: ticket.into(), request: request.clone(), created_at });
            }
            let exists: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM typed_tickets WHERE workspace_id=?1 AND ticket_id=?2)",params![workspace,ticket],|r|r.get(0))?;
            if !exists { return Err(Error::InvalidInput("Result Ticket not found in this Workspace".into())); }
            if let Some(mr) = &request.merge_request {
                // References identify retained evidence, not a model's assertion that a branch
                // is approved/current. Integration must resolve Git and reread the latest MR.
                let valid: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM merge_request_ticket_relations r WHERE r.workspace_id=?1 AND r.ticket_id=?2 AND r.merge_request_id=?3 AND (EXISTS(SELECT 1 FROM merge_request_thread_events e WHERE e.workspace_id=r.workspace_id AND e.merge_request_id=r.merge_request_id AND (json_extract(e.payload_json,'$.subject_ref')=?4 OR json_extract(e.payload_json,'$.approved_source_ref')=?4)) OR EXISTS(SELECT 1 FROM merge_request_source_observations o WHERE o.workspace_id=r.workspace_id AND o.merge_request_id=r.merge_request_id AND o.subject_ref=?4)))",params![workspace,ticket,mr.merge_request_id,mr.subject_ref],|r|r.get(0))?;
                if !valid { return Err(Error::InvalidInput("MR/source reference does not identify retained evidence for this Ticket".into())); }
            }
            let result_id = uuid::Uuid::now_v7().to_string();
            let created_at = chrono::Utc::now().to_rfc3339();
            tx.execute("INSERT INTO ticket_implementation_results(workspace_id,ticket_id,operation_key,result_id,runtime_id,worker_id,request_json,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",params![workspace,ticket,request.operation_key,result_id,worker.runtime_id,worker.worker_id,encoded,created_at])?;
            tx.commit()?;
            Ok(server_api::TicketImplementationResultReceipt { result_id, ticket_id: ticket.into(), request: request.clone(), created_at })
        })
    }
}
