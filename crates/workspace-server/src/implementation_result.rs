//! Implementation results are authenticated facts, not Orchestrator answers or handling updates.
//! The write lock fences the original implementation assignment and its pinned delegation.
//! Replays return the original result after reassignment, but cannot authorize a new result.
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
            Ok(server_api::TicketImplementationResultReceipt { result_id: result.into(), ticket_id: ticket.into(), request: serde_json::from_str(&request).map_err(|e| Error::Store(e.to_string()))?, created_at })
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
        for (name, value) in [
            ("operation_key", &request.operation_key),
            ("assignment_id", &request.assignment_id),
            (
                "orchestrator_assignment_id",
                &request.orchestrator_assignment_id,
            ),
        ] {
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
            let authenticated: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM workspace_runtime_bindings WHERE workspace_id=?1 AND runtime_id=?2 AND binding_id=?3 AND revoked_at IS NULL)", params![workspace,worker.runtime_id,binding], |r| r.get(0))?;
            if !authenticated {
                return Err(Error::WorkspacePermissionDenied("Implementation result requires its authenticated Runtime binding".into()));
            }
            let saved: Option<(String,String,String,String,String,String)> = tx.query_row("SELECT result_id,ticket_id,request_json,runtime_id,worker_id,created_at FROM ticket_implementation_results WHERE workspace_id=?1 AND operation_key=?2",params![workspace,request.operation_key], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).optional()?;
            if let Some((result_id,saved_ticket,saved_request,runtime,source,created_at)) = saved {
                if saved_ticket != ticket || saved_request != encoded || runtime != worker.runtime_id || source != worker.worker_id {
                    return Err(Error::TicketAssignmentConflict("Implementation result operation key was used for different input or source".into()));
                }
                return Ok(server_api::TicketImplementationResultReceipt { result_id, ticket_id: ticket.into(), request: request.clone(), created_at });
            }
            let owns: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM ticket_current_worker_assignments c JOIN ticket_implementation_delegations d ON d.workspace_id=c.workspace_id AND d.assignment_id=c.assignment_id WHERE c.workspace_id=?1 AND c.ticket_id=?2 AND c.role='worker' AND c.assignment_id=?3 AND c.principal_kind='worker' AND c.runtime_id=?4 AND c.worker_id=?5 AND d.orchestrator_assignment_id=?6 AND NOT EXISTS(SELECT 1 FROM ticket_assignment_work_releases r WHERE r.workspace_id=c.workspace_id AND r.assignment_id=c.assignment_id))",params![workspace,ticket,request.assignment_id,worker.runtime_id,worker.worker_id,request.orchestrator_assignment_id],|r|r.get(0))?;
            if !owns {
                return Err(Error::WorkspacePermissionDenied("Result source is not the current implementation Worker for this delegation; legacy unknown work requires explicit assignment confirmation".into()));
            }
            // A delayed new result is rejected rather than retargeted to a new Orchestrator.
            notification_routing::require_current(&tx,workspace,ticket,&request.orchestrator_assignment_id,"orchestrator")?;
            if let Some(mr) = &request.merge_request {
                // References identify retained evidence, not a model's assertion that a branch
                // is approved/current. Integration must resolve Git and reread the latest MR.
                let valid: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM merge_request_ticket_relations r JOIN merge_request_thread_events e ON e.workspace_id=r.workspace_id AND e.merge_request_id=r.merge_request_id WHERE r.workspace_id=?1 AND r.ticket_id=?2 AND r.merge_request_id=?3 AND (json_extract(e.payload_json,'$.subject_ref')=?4 OR json_extract(e.payload_json,'$.approved_source_ref')=?4))",params![workspace,ticket,mr.merge_request_id,mr.subject_ref],|r|r.get(0))?;
                if !valid { return Err(Error::InvalidInput("MR/source reference does not identify retained evidence for this Ticket".into())); }
            }
            let result_id = uuid::Uuid::now_v7().to_string();
            let created_at = chrono::Utc::now().to_rfc3339();
            tx.execute("INSERT INTO ticket_implementation_results VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",params![workspace,ticket,request.operation_key,result_id,request.assignment_id,request.orchestrator_assignment_id,worker.runtime_id,worker.worker_id,encoded,created_at])?;
            tx.commit()?;
            Ok(server_api::TicketImplementationResultReceipt { result_id, ticket_id: ticket.into(), request: request.clone(), created_at })
        })
    }
}
