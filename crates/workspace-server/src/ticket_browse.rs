//! SQL-only browser projection. No Ticket detail, dependency graph, MR/provider
//! observations, or per-ticket reads. Full QueryTicket/show retain their own semantics.
use super::{Error, RECORD_SOURCE_WORKSPACE_SQLITE, Result, SqliteWorkspaceAuthority};
use rusqlite::{Connection, params_from_iter, types::Value};
use server_api::{
    BrowserTicketState, QueryPage, TicketBrowseQueryItem, TicketBrowseQueryRequest,
    TicketBrowseQueryResponse,
};
use sha2::{Digest, Sha256};
use std::collections::HashMap;

#[cfg(test)]
#[path = "ticket_browse_tests.rs"]
mod tests;

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct BrowseCursor {
    scope: String,
    updated_at: Option<String>,
    id: String,
}

impl SqliteWorkspaceAuthority {
    /// A filtered page of display fields in one SQLite read snapshot. All matching
    /// predicates execute before LIMIT; unrelated live evidence is never observed.
    pub fn query_ticket_browse(
        &self,
        mut query: TicketBrowseQueryRequest,
    ) -> Result<TicketBrowseQueryResponse> {
        if query
            .sort
            .as_deref()
            .is_some_and(|sort| sort != "updated_desc")
        {
            return Err(Error::InvalidInput(
                "Ticket browse supports only updated_desc".into(),
            ));
        }
        query.sort = Some("updated_desc".into());
        query.states.sort_by_key(|state| state.as_str());
        query.states.dedup();
        query.tag_ids.sort();
        query.tag_ids.dedup();
        if query.tag_ids.len() > 100 || query.tag_ids.iter().any(|id| id.trim().is_empty()) {
            return Err(Error::InvalidInput(
                "Ticket browse requires at most 100 nonblank tag IDs".into(),
            ));
        }
        query.query = query
            .query
            .filter(|text| !text.is_empty())
            .map(|text| text.to_lowercase());
        let cursor = query.cursor.take();
        let limit = query.limit.take().unwrap_or(50).clamp(1, 100);
        let digest = Sha256::digest(
            serde_json::to_vec(&(&self.workspace_id, &query))
                .map_err(|e| Error::InvalidInput(e.to_string()))?,
        );
        let scope = format!(
            "ticket-browse-v1:{}",
            digest
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        );
        let after = cursor
            .map(|cursor| -> Result<BrowseCursor> {
                let cursor: BrowseCursor = serde_json::from_str(&cursor)
                    .map_err(|_| Error::InvalidInput("Invalid Ticket browse cursor".into()))?;
                if cursor.scope != scope || cursor.id.is_empty() {
                    return Err(Error::InvalidInput(
                        "Ticket browse cursor does not match Workspace, filters or sort".into(),
                    ));
                }
                Ok(cursor)
            })
            .transpose()?;
        let (sql, values) = page_sql(&self.workspace_id, &query, after.as_ref(), limit);
        self.store.with_conn_mut(|conn| {
            if query.query.is_some() {
                // SQLite lower() only folds ASCII. Use the same Unicode lowercasing
                // as the bound needle, without post-filtering a limited page.
                conn.create_scalar_function(
                    "ticket_browse_contains",
                    2,
                    rusqlite::functions::FunctionFlags::SQLITE_UTF8
                        | rusqlite::functions::FunctionFlags::SQLITE_DETERMINISTIC
                        | rusqlite::functions::FunctionFlags::SQLITE_INNOCUOUS,
                    |context| {
                        let raw_text = context.get_raw(0);
                        if raw_text == rusqlite::types::ValueRef::Null {
                            return Ok(false);
                        }
                        let text = raw_text
                            .as_str()
                            .map_err(|error| rusqlite::Error::UserFunctionError(Box::new(error)))?;
                        let needle = context
                            .get_raw(1)
                            .as_str()
                            .map_err(|error| rusqlite::Error::UserFunctionError(Box::new(error)))?;
                        Ok(text.to_lowercase().contains(needle))
                    },
                )?;
            }
            let tx = conn.transaction()?;
            let mut items = {
                let mut statement = tx.prepare(&sql)?;
                let rows = statement.query_map(params_from_iter(values.iter()), |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, Option<String>>(4)?,
                    ))
                })?;
                rows.map(|row| {
                    let (id, key, title, state, updated_at) = row?;
                    let resource_key =
                        key.ok_or_else(|| Error::Store(format!("missing resource key for {id}")))?;
                    let state = match state.as_str() {
                        "open" => BrowserTicketState::Open,
                        "closed" => BrowserTicketState::Closed,
                        _ => {
                            return Err(Error::Store(format!("invalid Ticket case state {state}")));
                        }
                    };
                    Ok(TicketBrowseQueryItem {
                        id,
                        resource_key,
                        title,
                        state,
                        updated_at,
                        tags: vec![],
                        current_closure: None,
                        assignment_tracking: vec![],
                    })
                })
                .collect::<Result<Vec<_>>>()?
            };
            let has_more = items.len() > limit;
            items.truncate(limit);
            enrich_page(&tx, &self.workspace_id, &mut items)?;
            let next_cursor = if has_more {
                items
                    .last()
                    .map(|item| {
                        serde_json::to_string(&BrowseCursor {
                            scope,
                            updated_at: item.updated_at.clone(),
                            id: item.id.clone(),
                        })
                        .map_err(|e| Error::Store(e.to_string()))
                    })
                    .transpose()?
            } else {
                None
            };
            tx.commit()?;
            Ok(TicketBrowseQueryResponse {
                page: QueryPage {
                    limit,
                    returned: items.len(),
                    has_more,
                    next_cursor,
                    sort: "updated_desc".into(),
                    source_limit: None,
                    source_truncated: false,
                },
                items,
                record_authority: RECORD_SOURCE_WORKSPACE_SQLITE.into(),
            })
        })
    }
}

fn page_sql(
    workspace: &str,
    query: &TicketBrowseQueryRequest,
    after: Option<&BrowseCursor>,
    limit: usize,
) -> (String, Vec<Value>) {
    let mut values = vec![Value::Text(workspace.into())];
    let mut predicates = vec!["t.workspace_id=?1".to_string()];
    let mut bind = |value: String| {
        values.push(Value::Text(value));
        format!("?{}", values.len())
    };
    if !query.states.is_empty() {
        let states = query
            .states
            .iter()
            .map(|state| bind(state.as_str().into()))
            .collect::<Vec<_>>();
        predicates.push(format!("t.workflow_state IN ({})", states.join(",")));
    }
    if let Some(reason) = &query.closure_reason {
        let reason = bind(serde_json::to_string(reason).expect("closure reason serializes"));
        predicates.push(format!("t.workflow_state='closed' AND EXISTS (
            SELECT 1 FROM typed_ticket_case_metadata m JOIN typed_ticket_case_history h
              ON h.workspace_id=m.workspace_id AND h.ticket_id=m.ticket_id AND h.operation_key=m.closure_operation_key
            WHERE m.workspace_id=t.workspace_id AND m.ticket_id=t.ticket_id
              AND json_extract(h.judgment_json,'$.action.reason')=json_extract({reason},'$'))"));
    }
    if query.assigned_principal.is_some() || query.handling_status.is_some() {
        let mut terms = vec![
            "c.workspace_id=t.workspace_id".to_string(),
            "c.ticket_id=t.ticket_id".into(),
        ];
        if let Some(principal) = &query.assigned_principal {
            use ticket::TicketAssignmentPrincipalSnapshot::*;
            terms.push(match principal {
                Subject { subject_id } => format!(
                    "a.principal_kind='subject' AND a.principal_id={}",
                    bind(subject_id.clone())
                ),
                WorkspaceAgent { agent_key } => format!(
                    "a.principal_kind='workspace_agent' AND a.principal_id={}",
                    bind(agent_key.clone())
                ),
                User { account_id } => format!(
                    "a.principal_kind='user' AND a.principal_id={}",
                    bind(account_id.clone())
                ),
                Worker {
                    runtime_id,
                    worker_id,
                } => format!(
                    "a.principal_kind='worker' AND a.runtime_id={} AND a.worker_id={}",
                    bind(runtime_id.clone()),
                    bind(worker_id.clone())
                ),
            });
        }
        if let Some(status) = &query.handling_status {
            let status = match status {
                ticket::TicketAssignmentTrackingStatus::Waiting => "waiting",
                ticket::TicketAssignmentTrackingStatus::Inprogress => "inprogress",
                ticket::TicketAssignmentTrackingStatus::Done => "done",
            };
            terms.push(format!(
                "a.role='orchestrator' AND json_extract(a.tracking_json,'$.status')={}",
                bind(status.into())
            ));
        }
        predicates.push(format!("EXISTS (SELECT 1 FROM ticket_current_worker_assignments c
            JOIN ticket_worker_assignments a ON a.workspace_id=c.workspace_id AND a.ticket_id=c.ticket_id AND a.assignment_id=c.assignment_id AND a.role=c.role
            WHERE {})", terms.join(" AND ")));
    }
    for tag in &query.tag_ids {
        let tag = bind(tag.clone());
        predicates.push(format!("EXISTS (SELECT 1 FROM typed_ticket_tags l
            JOIN workspace_tags tag ON tag.workspace_id=l.workspace_id AND tag.tag_id=l.tag_id AND tag.deleted=0
            WHERE l.workspace_id=t.workspace_id AND l.ticket_id=t.ticket_id AND l.tag_id={tag})"));
    }
    if let Some(text) = &query.query {
        // Literal substring search: %, _ and backslash are not wildcards.
        // Initial comment (-1) is the authoritative body.
        let needle = bind(text.clone());
        predicates.push(format!("(ticket_browse_contains(t.title,{needle}) OR EXISTS (
            SELECT 1 FROM typed_ticket_events e WHERE e.workspace_id=t.workspace_id AND e.ticket_id=t.ticket_id
              AND e.archived=0 AND e.event_index>=-1 AND ticket_browse_contains(e.body,{needle}))
            OR EXISTS (SELECT 1 FROM typed_ticket_case_history h
              WHERE h.workspace_id=t.workspace_id AND h.ticket_id=t.ticket_id
                AND ticket_browse_contains(CASE json_type(h.judgment_json,'$.action.reason')
                  WHEN 'object' THEN 'other ' || json_extract(h.judgment_json,'$.action.reason.other')
                  ELSE json_extract(h.judgment_json,'$.action.reason') END,{needle})))"));
    }
    if let Some(after) = after {
        let id = bind(after.id.clone());
        if let Some(time) = &after.updated_at {
            let time = bind(time.clone());
            predicates.push(format!("(t.updated_at<{time} OR t.updated_at IS NULL OR (t.updated_at={time} AND t.ticket_id>{id}))"));
        } else {
            predicates.push(format!("(t.updated_at IS NULL AND t.ticket_id>{id})"));
        }
    }
    values.push(Value::Integer((limit + 1) as i64));
    let sql = format!("SELECT t.ticket_id,k.resource_key,t.title,t.workflow_state,t.updated_at
        FROM typed_tickets t LEFT JOIN workspace_resource_keys k
          ON k.workspace_id=t.workspace_id AND k.resource_kind='ticket' AND k.resource_id=t.ticket_id
        WHERE {} ORDER BY t.updated_at DESC,t.ticket_id ASC LIMIT ?{}", predicates.join(" AND "), values.len());
    (sql, values)
}

fn enrich_page(
    conn: &Connection,
    workspace: &str,
    items: &mut [TicketBrowseQueryItem],
) -> Result<()> {
    if items.is_empty() {
        return Ok(());
    }
    let positions: HashMap<String, usize> = items
        .iter()
        .enumerate()
        .map(|(i, item)| (item.id.clone(), i))
        .collect();
    let mut values = vec![Value::Text(workspace.into())];
    values.extend(items.iter().map(|item| Value::Text(item.id.clone())));
    let ids = (2..=values.len())
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(",");
    let mut statement = conn.prepare(&format!("SELECT l.ticket_id,tag.tag_id,tag.name,tag.last_operation_key
        FROM typed_ticket_tags l JOIN workspace_tags tag ON tag.workspace_id=l.workspace_id AND tag.tag_id=l.tag_id
        WHERE l.workspace_id=?1 AND l.ticket_id IN ({ids}) AND tag.deleted=0 ORDER BY l.ticket_id,tag.tag_id"))?;
    let mut rows = statement.query(params_from_iter(values.iter()))?;
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        items[positions[&id]].tags.push(ticket::WorkspaceTag {
            workspace_id: workspace.into(),
            id: row.get(1)?,
            name: row.get(2)?,
            last_operation_key: row.get(3)?,
            deleted: false,
        });
    }
    let mut statement = conn.prepare(&format!("SELECT t.ticket_id,h.judgment_json FROM typed_tickets t
        JOIN typed_ticket_case_metadata m ON m.workspace_id=t.workspace_id AND m.ticket_id=t.ticket_id
        JOIN typed_ticket_case_history h ON h.workspace_id=m.workspace_id AND h.ticket_id=m.ticket_id AND h.operation_key=m.closure_operation_key
        WHERE t.workspace_id=?1 AND t.ticket_id IN ({ids}) AND t.workflow_state='closed'"))?;
    let mut rows = statement.query(params_from_iter(values.iter()))?;
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        let entry: ticket::TicketCaseHistoryEntry = decode_json(&row.get::<_, String>(1)?)?;
        let reason = match entry.action {
            ticket::TicketCaseAction::Close { reason }
            | ticket::TicketCaseAction::CorrectClosure { reason } => reason,
            ticket::TicketCaseAction::Reopen => {
                return Err(Error::Store("Closure key names a reopen receipt".into()));
            }
        };
        items[positions[&id]].current_closure = Some(ticket::TicketClosureJudgment {
            operation_key: entry.operation_key,
            reason,
            explanation: entry.explanation,
            references: entry.references,
            author: entry.author,
            at: entry.at,
        });
    }
    // Current Orchestrator projections only, exactly as TicketQueryItem. Legacy
    // unknown handling retains its real Assign observation without inventing status.
    let mut statement = conn.prepare(&format!("SELECT a.ticket_id,json_object(
        'workspace_id',a.workspace_id,'ticket_id',a.ticket_id,'assignment_id',a.assignment_id,'role',a.role,
        'principal',json(CASE a.principal_kind
          WHEN 'subject' THEN json_object('kind','subject','subject_id',a.principal_id)
          WHEN 'workspace_agent' THEN json_object('kind','workspace_agent','agent_key',a.principal_id)
          WHEN 'user' THEN json_object('kind','user','account_id',a.principal_id)
          WHEN 'worker' THEN json_object('kind','worker','runtime_id',a.runtime_id,'worker_id',a.worker_id) END),
        'assigned_by',a.assigned_by,'assigned_at',a.assigned_at,'release',NULL,'current',json('true'),
        'tracking',json(a.tracking_json),'last_event_id',CASE
          WHEN a.tracking_json IS NULL AND a.last_tracking_event_id IS NULL THEN (
            SELECT e.event_id FROM ticket_worker_assignment_events e
            WHERE e.workspace_id=a.workspace_id AND e.ticket_id=a.ticket_id AND e.assignment_id=a.assignment_id
              AND e.role='orchestrator' AND e.action IN ('assigned','reassigned')
              AND NOT EXISTS (SELECT 1 FROM typed_ticket_assignment_tracking_history h
                WHERE h.workspace_id=a.workspace_id AND h.ticket_id=a.ticket_id AND h.assignment_id=a.assignment_id)
            ORDER BY e.rowid LIMIT 1)
          ELSE a.last_tracking_event_id END,
        'updated_at',a.tracking_updated_at,'migration_diagnostic',a.tracking_migration_diagnostic)
        FROM ticket_current_worker_assignments c JOIN ticket_worker_assignments a
          ON a.workspace_id=c.workspace_id AND a.ticket_id=c.ticket_id AND a.assignment_id=c.assignment_id AND a.role=c.role
        WHERE c.workspace_id=?1 AND c.ticket_id IN ({ids}) AND c.role='orchestrator'
        ORDER BY a.ticket_id,a.assignment_id"))?;
    let mut rows = statement.query(params_from_iter(values.iter()))?;
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        items[positions[&id]]
            .assignment_tracking
            .push(decode_json(&row.get::<_, String>(1)?)?);
    }
    Ok(())
}

fn decode_json<T: serde::de::DeserializeOwned>(json: &str) -> Result<T> {
    serde_json::from_str(json).map_err(|e| Error::Store(e.to_string()))
}
