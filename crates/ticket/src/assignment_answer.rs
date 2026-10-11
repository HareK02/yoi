//! Continuing input is authored against an observed delegation, never a new Assign.
use super::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketAssignmentAnswer {
    pub assignment_id: String,
    pub expected_last_event_id: String,
    pub operation_key: String,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TicketAssignmentAnswerReceipt {
    pub assignment_id: String,
    pub input_event_id: String,
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_u64))]
    pub event_index: u64,
}

/// Host owns the IMMEDIATE transaction and authenticates/fences its source before
/// calling, including on replay. Reuse Ticket's existing immutable operation
/// receipts with a tagged input; these are not handling events or another ledger.
pub fn sqlite_answer_ticket_assignment_in_transaction(
    conn: &Connection,
    workspace_id: &str,
    ticket_id: &str,
    answer: &TicketAssignmentAnswer,
    source_context: &serde_json::Value,
    actor: &str,
) -> Result<TicketAssignmentAnswerReceipt> {
    for value in [
        &answer.assignment_id,
        &answer.expected_last_event_id,
        &answer.operation_key,
        actor,
    ] {
        if value.trim().is_empty() || value.len() > 1024 || value.chars().any(char::is_control) {
            return Err(TicketError::Conflict(
                "Answer requires bounded nonempty identities".into(),
            ));
        }
    }
    if answer.body.trim().is_empty() || answer.body.len() > 16384 || answer.body.contains('\0') {
        return Err(TicketError::Conflict(
            "Answer body must be nonempty, at most 16384 bytes and without NUL".into(),
        ));
    }
    let input =
        serde_json::to_string(&serde_json::json!({"kind":"assignment_answer", "answer":answer}))
            .map_err(sqlite_err)?;
    let source = serde_json::to_string(
        &serde_json::json!({"kind":"assignment_answer", "source":source_context, "actor":actor}),
    )
    .map_err(sqlite_err)?;
    let receipt: Option<(String,String,String,String)> = conn.query_row(
        "SELECT ticket_id,input_json,source_context_json,result_json FROM typed_ticket_assignment_tracking_operations WHERE workspace_id=?1 AND operation_key=?2",
        params![workspace_id, answer.operation_key], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional().map_err(sqlite_err)?;
    if let Some((saved_ticket, saved_input, saved_source, saved_result)) = receipt {
        if saved_ticket != ticket_id || saved_input != input || saved_source != source {
            return Err(TicketError::OperationFingerprintMismatch {
                operation_key: answer.operation_key.clone(),
            });
        }
        return serde_json::from_str(&saved_result).map_err(sqlite_err);
    }
    let current =
        sqlite_ticket_assignment_tracking(conn, workspace_id, ticket_id, &answer.assignment_id)?
            .ok_or_else(|| {
                TicketError::Conflict("Answer assignment does not exist in this Ticket".into())
            })?;
    if !current.current
        || current.last_event_id.as_deref() != Some(&answer.expected_last_event_id)
        || !current.tracking.as_ref().is_some_and(|tracking| {
            matches!(
                tracking.status,
                TicketAssignmentTrackingStatus::Waiting
                    | TicketAssignmentTrackingStatus::Inprogress
            )
        })
    {
        return Err(TicketError::Conflict("Answer observed assignment or handling event is stale; read current responsibility before authoring".into()));
    }
    let open: bool = conn.query_row("SELECT workflow_state='open' FROM typed_tickets WHERE workspace_id=?1 AND ticket_id=?2", params![workspace_id,ticket_id], |r| r.get(0)).map_err(sqlite_err)?;
    if !open {
        return Err(TicketError::Conflict(
            "A closed Ticket does not admit continuing work input".into(),
        ));
    }
    let index: i64 = conn.query_row("SELECT COALESCE(MAX(event_index),-1)+1 FROM typed_ticket_events WHERE workspace_id=?1 AND ticket_id=?2",params![workspace_id,ticket_id],|r| r.get(0)).map_err(sqlite_err)?;
    let at = now_utc();
    conn.execute("INSERT INTO typed_ticket_events(workspace_id,ticket_id,event_index,kind,author,at,heading,body) VALUES(?1,?2,?3,'assignment_answer',?4,?5,'Assignment answer',?6)", params![workspace_id,ticket_id,index,actor,at,answer.body]).map_err(sqlite_err)?;
    for (key, value) in [
        ("orchestrator_assignment_id", answer.assignment_id.as_str()),
        (
            "expected_last_event_id",
            answer.expected_last_event_id.as_str(),
        ),
        ("operation_key", answer.operation_key.as_str()),
        ("authenticated_source", source.as_str()),
    ] {
        conn.execute("INSERT INTO typed_ticket_event_attributes(workspace_id,ticket_id,event_index,key,value) VALUES(?1,?2,?3,?4,?5)",params![workspace_id,ticket_id,index,key,value]).map_err(sqlite_err)?;
    }
    conn.execute(
        "UPDATE typed_tickets SET updated_at=?3 WHERE workspace_id=?1 AND ticket_id=?2",
        params![workspace_id, ticket_id, at],
    )
    .map_err(sqlite_err)?;
    let result = TicketAssignmentAnswerReceipt {
        assignment_id: answer.assignment_id.clone(),
        input_event_id: format!("ticket-event:{index}"),
        event_index: index as u64,
    };
    let result_json = serde_json::to_string(&result).map_err(sqlite_err)?;
    conn.execute("INSERT INTO typed_ticket_assignment_tracking_operations(workspace_id,operation_key,ticket_id,input_json,source_context_json,result_json) VALUES(?1,?2,?3,?4,?5,?6)",params![workspace_id,answer.operation_key,ticket_id,input,source,result_json]).map_err(sqlite_err)?;
    Ok(result)
}
