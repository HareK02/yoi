//! Read-only projection of source facts and saved routing/delivery evidence.
//! No policy reimplementation: absent decisions stay awaiting_routing, never suppression.
use super::*;
use server_api::{
    NotificationDiagnostic, NotificationDiagnosticStage as Stage, NotificationDiagnosticsPage,
    NotificationDiagnosticsQuery,
};

fn source_checks(
    conn: &Connection,
    workspace: &str,
    query: &NotificationDiagnosticsQuery,
) -> Result<Vec<server_api::NotificationSourceCheck>> {
    let mut checks = Vec::new();
    if let Some(id) = &query.merge_request_id {
        let origin: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM merge_request_thread_events WHERE workspace_id=?1 AND merge_request_id=?2)",params![workspace,id],|r|r.get(0))?;
        let captured: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM notification_sources WHERE workspace_id=?1 AND json_extract(facts_json,'$.merge_request_id')=?2)",params![workspace,id],|r|r.get(0))?;
        checks.push(server_api::NotificationSourceCheck {
            resource_kind: "merge_request_events".into(),
            reference: id.clone(),
            origin_present: Some(origin),
            captured,
        });
    }
    if let Some(id) = &query.job_id {
        let origin: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM backend_jobs WHERE workspace_id=?1 AND job_id=?2 AND result_digest IS NOT NULL)",params![workspace,id],|r|r.get(0))?;
        let captured: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM notification_sources WHERE workspace_id=?1 AND COALESCE(json_extract(facts_json,'$.job_id'),json_extract(facts_json,'$.backend_job.job_id'))=?2)",params![workspace,id],|r|r.get(0))?;
        checks.push(server_api::NotificationSourceCheck {
            resource_kind: "job_result".into(),
            reference: id.clone(),
            origin_present: Some(origin),
            captured,
        });
    }
    if let Some(id) = &query.source_event_id {
        let captured: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM notification_sources s LEFT JOIN workspace_resource_keys k ON k.workspace_id=s.workspace_id AND k.resource_kind='ticket' AND k.resource_id=s.ticket_id WHERE s.workspace_id=?1 AND s.input_event_id=?2 AND (?3 IS NULL OR s.ticket_id=?3 OR k.resource_key=?3))",params![workspace,id,query.ticket],|r|r.get(0))?;
        let origin = if captured {
            Some(true)
        } else if let (Some(ticket), Some(index)) = (
            &query.ticket,
            id.strip_prefix("ticket-event:")
                .and_then(|v| v.parse::<i64>().ok()),
        ) {
            Some(conn.query_row("SELECT EXISTS(SELECT 1 FROM typed_ticket_events e LEFT JOIN workspace_resource_keys k ON k.workspace_id=e.workspace_id AND k.resource_kind='ticket' AND k.resource_id=e.ticket_id WHERE e.workspace_id=?1 AND (e.ticket_id=?2 OR k.resource_key=?2) AND e.event_index=?3)",params![workspace,ticket,index],|r|r.get(0))?)
        } else {
            None
        };
        checks.push(server_api::NotificationSourceCheck {
            resource_kind: "source_event".into(),
            reference: id.clone(),
            origin_present: origin,
            captured,
        });
    }
    Ok(checks)
}

fn sequence(value: &str) -> Result<i64> {
    value
        .strip_prefix("notification-source:")
        .and_then(|n| n.parse::<i64>().ok())
        .filter(|n| *n > 0)
        .ok_or_else(|| Error::InvalidInput("Invalid notification source cursor".into()))
}
fn bounded(value: Option<String>) -> Option<String> {
    value.map(|s| s.chars().take(512).collect())
}

pub(super) fn page(
    store: &SqliteWorkspaceStore,
    workspace: &str,
    query: &NotificationDiagnosticsQuery,
) -> Result<NotificationDiagnosticsPage> {
    store.with_conn(|conn| {
        let tx = conn.unchecked_transaction()?;
        page_with_connection(&tx, workspace, query)
    })
}

pub(super) fn page_with_connection(
    conn: &Connection,
    workspace: &str,
    query: &NotificationDiagnosticsQuery,
) -> Result<NotificationDiagnosticsPage> {
    let limit = query.limit.unwrap_or(50);
    if !(1..=100).contains(&limit) {
        return Err(Error::InvalidInput(
            "Notification limit must be between 1 and 100".into(),
        ));
    }
    for value in [
        &query.ticket,
        &query.merge_request_id,
        &query.job_id,
        &query.source_event_id,
        &query.assignment_id,
        &query.delivery_id,
        &query.source_id,
        &query.cursor,
    ]
    .into_iter()
    .flatten()
    {
        if value.is_empty() || value.len() > 512 || value.chars().any(char::is_control) {
            return Err(Error::InvalidInput("Invalid notification filter".into()));
        }
    }
    let after = query
        .cursor
        .as_deref()
        .map(sequence)
        .transpose()?
        .unwrap_or(0);
    let source = query.source_id.as_deref().map(sequence).transpose()?;
    let filter = "s.workspace_id=?1
        AND (?2 IS NULL OR s.ticket_id=?2 OR tk.resource_key=?2)
        AND (?3 IS NULL OR json_extract(s.facts_json,'$.merge_request_id')=?3)
        AND (?4 IS NULL OR COALESCE(json_extract(s.facts_json,'$.job_id'),json_extract(s.facts_json,'$.backend_job.job_id'))=?4)
        AND (?5 IS NULL OR s.input_event_id=?5)
        AND (?6 IS NULL OR s.assignment_id=?6)
        AND (?7 IS NULL OR d.delivery_id=?7)
        AND (?8 IS NULL OR s.sequence=?8)";
    let joins = "FROM notification_sources s
        LEFT JOIN notification_decisions d ON d.source_sequence=s.sequence AND d.workspace_id=s.workspace_id
        LEFT JOIN workspace_resource_keys tk ON tk.workspace_id=s.workspace_id AND tk.resource_kind='ticket' AND tk.resource_id=s.ticket_id";
    let bind = params![
        workspace,
        query.ticket,
        query.merge_request_id,
        query.job_id,
        query.source_event_id,
        query.assignment_id,
        query.delivery_id,
        source,
        after,
        limit as i64 + 1
    ];
    // Validate the cursor against the same Workspace and filters, not a global row id.
    if after > 0 {
        let exists: bool = conn.query_row(
            &format!("SELECT EXISTS(SELECT 1 {joins} WHERE {filter} AND s.sequence=?9)"),
            &bind[..9],
            |row| row.get(0),
        )?;
        if !exists {
            return Err(Error::InvalidInput(
                "Notification cursor does not belong to this query".into(),
            ));
        }
    }
    let sql = format!("SELECT s.sequence,s.input_event_id,s.kind,COALESCE(tk.resource_key,s.ticket_id),
        json_extract(s.facts_json,'$.merge_request_id'),COALESCE(json_extract(s.facts_json,'$.job_id'),json_extract(s.facts_json,'$.backend_job.job_id')),s.role,NULLIF(s.assignment_id,''),
        d.disposition,d.reason,d.purpose,d.delivery_id,
        ar.delivery_id IS NOT NULL,
        (r.delivery_id IS NOT NULL OR ap.delivery_id IS NOT NULL),
        (a.delivery_id IS NOT NULL OR aa.delivery_id IS NOT NULL),
        COALESCE(json_extract(a.target_json,'$.worker.runtime_id'),json_extract(aa.source_json,'$.runtime_id'),json_extract(r.target_json,'$.worker.runtime_id'),json_extract(ar.reserved_source_json,'$.runtime_id')),
        COALESCE(json_extract(a.target_json,'$.worker.worker_id'),json_extract(aa.source_json,'$.worker_id'),json_extract(r.target_json,'$.worker.worker_id'),json_extract(ar.reserved_source_json,'$.worker_id')),
        f.code,f.recorded_at,
        CASE WHEN s.role IN ('orchestrator','worker') AND s.assignment_id!='' THEN EXISTS(SELECT 1 FROM ticket_current_worker_assignments c WHERE c.workspace_id=s.workspace_id AND c.ticket_id=s.ticket_id AND c.role=s.role AND c.assignment_id=s.assignment_id) END
        {joins}
        LEFT JOIN notification_requests r ON r.workspace_id=s.workspace_id AND r.delivery_id=d.delivery_id
        LEFT JOIN notification_acknowledgements a ON a.workspace_id=s.workspace_id AND a.delivery_id=d.delivery_id
        LEFT JOIN ticket_assignment_deliveries ar ON ar.workspace_id=s.workspace_id AND ar.delivery_id=d.delivery_id
        LEFT JOIN ticket_assignment_delivery_requests ap ON ap.workspace_id=s.workspace_id AND ap.delivery_id=ar.delivery_id
        LEFT JOIN ticket_assignment_delivery_acknowledgements aa ON aa.workspace_id=s.workspace_id AND aa.delivery_id=ar.delivery_id
        LEFT JOIN notification_failures f ON f.sequence=(SELECT MAX(nf.sequence) FROM notification_failures nf WHERE nf.workspace_id=s.workspace_id AND nf.delivery_id=d.delivery_id)
        WHERE {filter} AND s.sequence>?9 ORDER BY s.sequence LIMIT ?10");
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(bind)?;
    let mut items = Vec::new();
    while let Some(row) = rows.next()? {
        let disposition: Option<String> = row.get(8)?;
        let reserved: bool = row.get(12)?;
        let prepared: bool = row.get(13)?;
        let accepted: bool = row.get(14)?;
        let failure: Option<String> = row.get(17)?;
        let recipient_current: Option<bool> = row.get(19)?;
        let stage = if accepted {
            Stage::RuntimeAccepted
        } else if recipient_current == Some(false) {
            Stage::Historical
        } else if disposition.is_none() {
            Stage::AwaitingRouting
        } else if disposition.as_deref() == Some("suppress") {
            Stage::Suppressed
        } else if disposition.as_deref() == Some("defer") {
            Stage::Deferred
        } else if failure.as_deref() == Some("acceptance_unknown") {
            Stage::AcceptanceUnknown
        } else if failure.as_deref() == Some("runtime_rejected") {
            Stage::RuntimeRejected
        } else if failure.is_some() {
            Stage::DeliveryFailed
        } else if prepared {
            Stage::RequestPersisted
        } else if reserved {
            Stage::Reserved
        } else {
            Stage::Pending
        };
        let retry_candidate = !accepted
            && recipient_current != Some(false)
            && failure.as_deref() != Some("assignment_changed")
            && disposition.as_deref() == Some("deliver");
        items.push(NotificationDiagnostic {
            source_id: format!("notification-source:{}", row.get::<_, i64>(0)?),
            source_event_id: bounded(row.get(1)?).unwrap_or_default(),
            event_kind: bounded(row.get(2)?).unwrap_or_default(),
            ticket: bounded(row.get(3)?),
            merge_request_id: bounded(row.get(4)?),
            job_id: bounded(row.get(5)?),
            recipient_role: bounded(row.get(6)?).unwrap_or_default(),
            assignment_id: bounded(row.get(7)?),
            recipient_current,
            disposition: bounded(disposition),
            reason: bounded(row.get(9)?),
            purpose: bounded(row.get(10)?),
            delivery_id: bounded(row.get(11)?),
            stage,
            target_runtime_id: bounded(row.get(15)?),
            target_worker_id: bounded(row.get(16)?),
            last_failure_code: bounded(failure),
            last_failure_at: bounded(row.get(18)?),
            retry_candidate,
            model_processing_confirmed: false,
        });
    }
    let has_more = items.len() > limit;
    items.truncate(limit);
    let next_cursor = has_more.then(|| items.last().expect("positive limit").source_id.clone());
    let source_checks = source_checks(conn, workspace, query)?;
    Ok(NotificationDiagnosticsPage {
        items,
        next_cursor,
        source_checks,
    })
}
