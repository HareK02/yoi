//! Ticket-owned delegation identity and self-reported handling, not Runtime execution,
//! grants, delivery acknowledgement, or MR evidence. Each explicit Assign/reissue has a
//! new immutable identity, even for the same principal; Worker replacement is not Assign.
//! Assign commits the current pointer, initial waiting, audit, and receipt together.
//!
//! Handling has no mandatory progression: waiting-to-done is valid without MR approval.
//! Done does not close the case, release work, terminate a Worker, or clean up resources.
//! Close/reopen does not change handling; current responsibility can report after close
//! without restoring execution grants. Retained unknown handling is not inferred waiting/done.
//!
//! The host authenticates the current Subject body Worker and its actual Runtime
//! binding, including retries, inside the same write transaction as receipt comparison,
//! current-assignment/event CAS, audit, and result. Never split that transaction around
//! awaits. A replayed receipt is not a fresh event observation: reread before a new update,
//! and reconcile stale or unknown outcomes instead of silently issuing a new operation key.
use super::*;

/// Used by the Workspace host to compose its stricter principal constraints.
/// Historical physical names remain to preserve immutable IDs and receipts.
pub const TICKET_ASSIGNMENT_SCHEMA_SQL: &str = include_str!("assignment_schema.sql");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum TicketAssignmentTrackingStatus {
    Waiting,
    Inprogress,
    Done,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketAssignmentTracking {
    pub status: TicketAssignmentTrackingStatus,
    pub waiting_reason: Option<String>,
    pub progress: Option<String>,
    pub next_action: Option<String>,
    pub result: Option<String>,
}

/// Replaces the complete handling payload against this delegation's observed event,
/// not status alone. Exact retry compares full input and authenticated sender/binding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketAssignmentTrackingUpdate {
    pub assignment_id: String,
    pub expected_last_event_id: String,
    pub operation_key: String,
    pub tracking: TicketAssignmentTracking,
}

/// Backend-owned authenticated context, never decoded from model input. Subject
/// principal remains stable across Worker exchange; exact replay binds to the
/// authenticated concrete sender and its binding, not merely a claimed role.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TicketAssignmentTrackingSourceContext {
    pub actor: String,
    pub runtime_id: String,
    pub worker_id: String,
    pub subject_id: String,
    pub authentication_binding_id: String,
}

/// Enrichment is separate from the established assignment record. `None`
/// tracking means a retained historical relationship with insufficient delegation
/// evidence, not waiting/done. Removal/replacement does not alter last tracking.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TicketAssignmentTrackingProjection {
    pub workspace_id: String,
    pub ticket_id: String,
    pub assignment_id: String,
    pub role: String,
    pub principal: TicketAssignmentPrincipalSnapshot,
    pub assigned_by: String,
    pub assigned_at: String,
    pub release: Option<TicketAssignmentRelease>,
    pub current: bool,
    pub tracking: Option<TicketAssignmentTracking>,
    /// Tracking event, or the existing immutable Assign audit observation for a
    /// retained current relationship with no tracking yet. This baseline does
    /// not imply a handling status, delegation intent, or tracking timestamp.
    pub last_event_id: Option<String>,
    pub updated_at: Option<String>,
    pub migration_diagnostic: Option<String>,
}

/// Immutable principal as stored at Assign, not live Worker availability.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TicketAssignmentPrincipalSnapshot {
    Subject {
        subject_id: String,
    },
    User {
        account_id: String,
    },
    Worker {
        runtime_id: String,
        worker_id: String,
    },
    WorkspaceAgent {
        agent_key: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TicketAssignmentReleaseAction {
    Unassigned,
    Replaced,
}

/// Release is an independent immutable assignment audit event. Last tracking
/// remains unchanged, including unknown legacy tracking; it is never made done.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TicketAssignmentRelease {
    pub action: TicketAssignmentReleaseAction,
    pub event_id: String,
    pub operation_key: Option<String>,
    pub actor: String,
    pub at: String,
    pub reason: Option<String>,
    pub replacement_assignment_id: Option<String>,
    pub last_tracking: Option<TicketAssignmentTracking>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TicketAssignmentTrackingHistoryEntry {
    pub event_id: String,
    pub operation_key: String,
    pub tracking: TicketAssignmentTracking,
    pub source_context: serde_json::Value,
    pub at: String,
}

/// Bounded append-order page. Cursor is an immutable event identity, not a
/// lexical event-ID comparison or a transferable offset into another assignment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TicketAssignmentTrackingHistoryPage {
    pub items: Vec<TicketAssignmentTrackingHistoryEntry>,
    pub next_event_id: Option<String>,
}

fn valid_identity(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 1024 && !value.chars().any(char::is_control)
}

fn validate_update(
    update: &TicketAssignmentTrackingUpdate,
    source: &TicketAssignmentTrackingSourceContext,
) -> Result<()> {
    if [
        &update.assignment_id,
        &update.expected_last_event_id,
        &update.operation_key,
        &source.actor,
        &source.runtime_id,
        &source.worker_id,
        &source.authentication_binding_id,
    ]
    .into_iter()
    .any(|value| !valid_identity(value))
        || !valid_identity(&source.subject_id)
        || source.subject_id == "workspace-orchestrator"
    {
        return Err(TicketError::Conflict(
            "tracking requires bounded identities and authenticated Subject source".into(),
        ));
    }
    for text in [
        &update.tracking.waiting_reason,
        &update.tracking.progress,
        &update.tracking.next_action,
        &update.tracking.result,
    ]
    .into_iter()
    .flatten()
    {
        if text.len() > 16 * 1024 || text.contains('\0') {
            return Err(TicketError::Conflict(
                "tracking fields must be at most 16384 bytes without NUL".into(),
            ));
        }
    }
    Ok(())
}

/// Reads responsibility independently of case state and operational work release.
pub fn sqlite_ticket_assignment_tracking(
    conn: &Connection,
    workspace_id: &str,
    ticket_id: &str,
    assignment_id: &str,
) -> Result<Option<TicketAssignmentTrackingProjection>> {
    let row = conn.query_row("SELECT a.tracking_json, a.last_tracking_event_id, a.tracking_updated_at, a.tracking_migration_diagnostic,
      EXISTS(SELECT 1 FROM ticket_current_worker_assignments c WHERE c.workspace_id=a.workspace_id AND c.assignment_id=a.assignment_id),
      a.role, a.principal_kind, a.principal_id, a.runtime_id, a.worker_id, a.assigned_by, a.assigned_at
      FROM ticket_worker_assignments a WHERE a.workspace_id=?1 AND a.ticket_id=?2 AND a.assignment_id=?3 AND a.role='orchestrator'",
      params![workspace_id,ticket_id,assignment_id], |r| Ok((r.get::<_,Option<String>>(0)?,r.get::<_,Option<String>>(1)?,r.get::<_,Option<String>>(2)?,r.get::<_,Option<String>>(3)?,r.get::<_,bool>(4)?, r.get::<_,String>(5)?, r.get::<_,String>(6)?,r.get::<_,Option<String>>(7)?,r.get::<_,Option<String>>(8)?,r.get::<_,Option<String>>(9)?,r.get::<_,String>(10)?,r.get::<_,String>(11)?)))
      .optional().map_err(sqlite_err)?;
    let Some((
        json,
        last_event_id,
        updated_at,
        migration_diagnostic,
        current,
        role,
        kind,
        principal_id,
        runtime_id,
        worker_id,
        assigned_by,
        assigned_at,
    )) = row
    else {
        return Ok(None);
    };
    let tracking: Option<TicketAssignmentTracking> = json
        .map(|value| serde_json::from_str(&value).map_err(sqlite_err))
        .transpose()?;
    // Legacy migration may retain current responsibility without evidence of
    // handling status. Its real Assign audit is still an immutable observation
    // for a later explicitly authorized operation, even after close. This does
    // not authorize a legacy agent relationship to report as a Subject.
    // Never manufacture a tracking event/status or repair a missing audit. Once
    // tracking exists, only its event identity is a valid observation.
    let last_event_id = if current && tracking.is_none() && last_event_id.is_none() {
        conn.query_row(
            "SELECT e.event_id FROM ticket_worker_assignment_events e
             WHERE e.workspace_id=?1 AND e.ticket_id=?2 AND e.assignment_id=?3
               AND e.role='orchestrator' AND e.action IN ('assigned','reassigned')
               AND NOT EXISTS (SELECT 1 FROM typed_ticket_assignment_tracking_history h
                 WHERE h.workspace_id=?1 AND h.ticket_id=?2 AND h.assignment_id=?3)
             ORDER BY e.rowid LIMIT 1",
            params![workspace_id, ticket_id, assignment_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(sqlite_err)?
    } else {
        last_event_id
    };
    let missing =
        || TicketError::Conflict("stored assignment principal identity is incomplete".into());
    let principal = match kind.as_str() {
        "subject" => TicketAssignmentPrincipalSnapshot::Subject {
            subject_id: principal_id.ok_or_else(missing)?,
        },
        "workspace_agent" => TicketAssignmentPrincipalSnapshot::WorkspaceAgent {
            agent_key: principal_id.ok_or_else(missing)?,
        },
        "user" => TicketAssignmentPrincipalSnapshot::User {
            account_id: principal_id.ok_or_else(missing)?,
        },
        "worker" => TicketAssignmentPrincipalSnapshot::Worker {
            runtime_id: runtime_id.ok_or_else(missing)?,
            worker_id: worker_id.ok_or_else(missing)?,
        },
        _ => return Err(missing()),
    };
    let release = conn.query_row("SELECT action,event_id,operation_id,actor,created_at,reason,assignment_id FROM ticket_worker_assignment_events
        WHERE workspace_id=?1 AND ticket_id=?2 AND ((action='unassigned' AND (assignment_id=?3 OR previous_assignment_id=?3)) OR (action='reassigned' AND previous_assignment_id=?3)) ORDER BY rowid DESC LIMIT 1",
        params![workspace_id,ticket_id,assignment_id], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,Option<String>>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?,r.get::<_,Option<String>>(5)?,r.get::<_,Option<String>>(6)?))).optional().map_err(sqlite_err)?
        .map(|(action,event_id,operation_key,actor,at,reason,replacement)| TicketAssignmentRelease {
            action: if action=="unassigned" {TicketAssignmentReleaseAction::Unassigned} else {TicketAssignmentReleaseAction::Replaced},
            event_id,operation_key,actor,at,reason,replacement_assignment_id: if action=="reassigned" {replacement} else {None}, last_tracking: tracking.clone(),
        });
    Ok(Some(TicketAssignmentTrackingProjection {
        workspace_id: workspace_id.into(),
        ticket_id: ticket_id.into(),
        assignment_id: assignment_id.into(),
        role,
        principal,
        assigned_by,
        assigned_at,
        release,
        current,
        tracking,
        last_event_id,
        updated_at,
        migration_diagnostic,
    }))
}

pub fn sqlite_current_ticket_assignment_tracking(
    conn: &Connection,
    workspace_id: &str,
    ticket_id: &str,
) -> Result<Vec<TicketAssignmentTrackingProjection>> {
    let ids = conn.prepare("SELECT assignment_id FROM ticket_current_worker_assignments WHERE workspace_id=?1 AND ticket_id=?2 AND role='orchestrator' ORDER BY assignment_id")
      .map_err(sqlite_err)?.query_map(params![workspace_id,ticket_id],|r|r.get::<_,String>(0)).map_err(sqlite_err)?
      .collect::<std::result::Result<Vec<_>,_>>().map_err(sqlite_err)?;
    ids.into_iter()
        .map(|id| {
            sqlite_ticket_assignment_tracking(conn, workspace_id, ticket_id, &id)?
                .ok_or_else(|| TicketError::Conflict("current assignment row is missing".into()))
        })
        .collect()
}

pub fn sqlite_ticket_assignment_tracking_history(
    conn: &Connection,
    workspace_id: &str,
    ticket_id: &str,
    assignment_id: &str,
) -> Result<Vec<TicketAssignmentTrackingHistoryEntry>> {
    let mut stmt=conn.prepare("SELECT h.event_id,h.operation_key,h.tracking_json,h.source_context_json,h.at FROM typed_ticket_assignment_tracking_history h
      WHERE h.workspace_id=?1 AND h.ticket_id=?2 AND h.assignment_id=?3 ORDER BY h.rowid").map_err(sqlite_err)?;
    let rows = stmt
        .query_map(params![workspace_id, ticket_id, assignment_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
            ))
        })
        .map_err(sqlite_err)?;
    rows.map(|r| {
        let (event_id, operation_key, json, source, at) = r.map_err(sqlite_err)?;
        Ok(TicketAssignmentTrackingHistoryEntry {
            event_id,
            operation_key,
            tracking: serde_json::from_str(&json).map_err(sqlite_err)?,
            source_context: serde_json::from_str(&source).map_err(sqlite_err)?,
            at,
        })
    })
    .collect()
}

/// DB-bounded history pagination. Immutable append-only rows provide a stable
/// cursor position; the scope index also orders its implicit rowids. No caller
/// needs to load, deserialize or truncate the lifetime assignment history.
pub fn sqlite_ticket_assignment_tracking_history_page(
    conn: &Connection,
    workspace_id: &str,
    ticket_id: &str,
    assignment_id: &str,
    after_event_id: Option<&str>,
    limit: usize,
) -> Result<TicketAssignmentTrackingHistoryPage> {
    if !(1..=100).contains(&limit) {
        return Err(TicketError::Conflict(
            "tracking history limit must be 1..100".into(),
        ));
    }
    let exists:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM ticket_worker_assignments WHERE workspace_id=?1 AND ticket_id=?2 AND assignment_id=?3 AND role='orchestrator')",params![workspace_id,ticket_id,assignment_id],|r|r.get(0)).map_err(sqlite_err)?;
    if !exists {
        return Err(TicketError::Conflict(
            "tracking assignment is not in this Workspace/Ticket".into(),
        ));
    }
    let after_rowid:Option<i64>=after_event_id.map(|event| conn.query_row("SELECT rowid FROM typed_ticket_assignment_tracking_history WHERE workspace_id=?1 AND ticket_id=?2 AND assignment_id=?3 AND event_id=?4",params![workspace_id,ticket_id,assignment_id,event],|r|r.get::<_,i64>(0)).optional().map_err(sqlite_err)?.ok_or_else(||TicketError::Conflict("tracking history cursor is not in this exact Workspace/Ticket/assignment".into()))).transpose()?;
    let after_clause = if after_rowid.is_some() {
        "AND h.rowid>?4"
    } else {
        "AND ?4 IS NULL"
    };
    let sql = format!(
        "SELECT h.event_id,h.operation_key,h.tracking_json,h.source_context_json,h.at FROM typed_ticket_assignment_tracking_history h WHERE h.workspace_id=?1 AND h.ticket_id=?2 AND h.assignment_id=?3 {after_clause} ORDER BY h.rowid LIMIT ?5"
    );
    let mut stmt = conn.prepare(&sql).map_err(sqlite_err)?;
    let rows = stmt
        .query_map(
            params![
                workspace_id,
                ticket_id,
                assignment_id,
                after_rowid,
                (limit + 1) as i64
            ],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                ))
            },
        )
        .map_err(sqlite_err)?;
    let mut items = rows
        .map(|r| {
            let (event_id, operation_key, json, source, at) = r.map_err(sqlite_err)?;
            Ok(TicketAssignmentTrackingHistoryEntry {
                event_id,
                operation_key,
                tracking: serde_json::from_str(&json).map_err(sqlite_err)?,
                source_context: serde_json::from_str(&source).map_err(sqlite_err)?,
                at,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let has_more = items.len() > limit;
    items.truncate(limit);
    let next_event_id = if has_more {
        items.last().map(|entry| entry.event_id.clone())
    } else {
        None
    };
    Ok(TicketAssignmentTrackingHistoryPage {
        items,
        next_event_id,
    })
}

/// Host owns BEGIN IMMEDIATE/commit. Receipt lookup precedes current/CAS checks;
/// exact replay returns the immutable operation result even after reassignment.
/// Case closed and work-release records are deliberately absent from this guard.
pub fn sqlite_update_ticket_assignment_tracking_in_transaction(
    conn: &Connection,
    workspace_id: &str,
    ticket_id: &str,
    update: &TicketAssignmentTrackingUpdate,
    source: &TicketAssignmentTrackingSourceContext,
) -> Result<TicketAssignmentTrackingProjection> {
    if conn.is_autocommit() {
        return Err(TicketError::Conflict(
            "tracking update requires a host write transaction".into(),
        ));
    }
    validate_update(update, source)?;
    let input_json = serde_json::to_string(&(ticket_id, update)).map_err(sqlite_err)?;
    let source_json = serde_json::to_string(source).map_err(sqlite_err)?;
    let receipt=conn.query_row("SELECT input_json,source_context_json,result_json FROM typed_ticket_assignment_tracking_operations WHERE workspace_id=?1 AND operation_key=?2",params![workspace_id,update.operation_key],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?))).optional().map_err(sqlite_err)?;
    if let Some((input, context, result)) = receipt {
        if input != input_json || context != source_json {
            return Err(TicketError::Conflict(
                "tracking operation key already used for different input or authenticated source"
                    .into(),
            ));
        }
        return serde_json::from_str(&result).map_err(sqlite_err);
    }
    let current =
        sqlite_ticket_assignment_tracking(conn, workspace_id, ticket_id, &update.assignment_id)?
            .ok_or_else(|| {
                TicketError::Conflict(
                    "tracking assignment not found in this Workspace/Ticket".into(),
                )
            })?;
    if !current.current
        || current.last_event_id.as_deref() != Some(update.expected_last_event_id.as_str())
    {
        return Err(TicketError::Conflict(
            "tracking assignment is removed/replaced or last event changed".into(),
        ));
    }
    let principal_matches:bool=conn.query_row("SELECT principal_kind='subject' AND principal_id=?3 FROM ticket_worker_assignments WHERE workspace_id=?1 AND assignment_id=?2",params![workspace_id,update.assignment_id,source.subject_id],|r|r.get(0)).map_err(sqlite_err)?;
    if !principal_matches {
        return Err(TicketError::Conflict(
            "authenticated Subject does not own this assignment".into(),
        ));
    }
    let event_id=format!("tracking-{}",allocate_record_id(unix_epoch_millis_now().unwrap_or(0), |candidate| conn.query_row("SELECT EXISTS(SELECT 1 FROM typed_ticket_assignment_tracking_history WHERE workspace_id=?1 AND event_id=?2)",params![workspace_id,format!("tracking-{candidate}")],|r|r.get::<_,bool>(0)).unwrap_or(true)).map_err(sqlite_err)?);
    let at = Utc::now().to_rfc3339();
    let tracking_json = serde_json::to_string(&update.tracking).map_err(sqlite_err)?;
    conn.execute("UPDATE ticket_worker_assignments SET tracking_json=?3,last_tracking_event_id=?4,tracking_updated_at=?5 WHERE workspace_id=?1 AND assignment_id=?2",params![workspace_id,update.assignment_id,tracking_json,event_id,at]).map_err(sqlite_err)?;
    conn.execute(
        "INSERT INTO typed_ticket_assignment_tracking_history VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
        params![
            workspace_id,
            ticket_id,
            update.assignment_id,
            event_id,
            update.operation_key,
            tracking_json,
            source_json,
            at
        ],
    )
    .map_err(sqlite_err)?;
    let result =
        sqlite_ticket_assignment_tracking(conn, workspace_id, ticket_id, &update.assignment_id)?
            .ok_or_else(|| TicketError::Conflict("tracking assignment disappeared".into()))?;
    conn.execute(
        "INSERT INTO typed_ticket_assignment_tracking_operations VALUES (?1,?2,?3,?4,?5,?6)",
        params![
            workspace_id,
            update.operation_key,
            ticket_id,
            input_json,
            source_json,
            serde_json::to_string(&result).map_err(sqlite_err)?
        ],
    )
    .map_err(sqlite_err)?;
    Ok(result)
}

pub(crate) fn install_assignment_core(conn: &Connection) -> Result<()> {
    // Standalone Ticket stores do not own Workspace records. The Workspace host
    // composes the unmodified SQL and adds its principal/resource constraints.
    let sql = if conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='workspaces')",
            [],
            |r| r.get::<_, bool>(0),
        )
        .map_err(sqlite_err)?
    {
        TICKET_ASSIGNMENT_SCHEMA_SQL.to_owned()
    } else {
        TICKET_ASSIGNMENT_SCHEMA_SQL.replace("FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE,", "").replace(",\n            FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE\n", "\n")
    };
    conn.execute_batch(&sql).map_err(sqlite_err)
}

pub(crate) fn migrate_assignment_tracking(conn: &Connection) -> Result<()> {
    install_assignment_core(conn)?;
    conn.execute_batch(include_str!("assignment_tracking_schema.sql"))
        .map_err(sqlite_err)?;
    reconcile_legacy_tracking(conn)
}

fn reconcile_legacy_tracking(conn: &Connection) -> Result<()> {
    let rows=conn.prepare("SELECT a.workspace_id,a.ticket_id,a.assignment_id,s.snapshot_json,
        EXISTS(SELECT 1 FROM ticket_current_worker_assignments c WHERE c.workspace_id=a.workspace_id AND c.assignment_id=a.assignment_id)
        FROM ticket_worker_assignments a LEFT JOIN typed_ticket_legacy_workflow_snapshot s ON s.workspace_id=a.workspace_id AND s.ticket_id=a.ticket_id WHERE a.role='orchestrator'").map_err(sqlite_err)?
        .query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,Option<String>>(3)?,r.get::<_,bool>(4)?))).map_err(sqlite_err)?
        .collect::<std::result::Result<Vec<_>,_>>().map_err(sqlite_err)?;
    for (workspace, ticket, assignment, snapshot, current) in rows {
        let original = snapshot
            .as_deref()
            .map(serde_json::from_str::<TicketLegacyWorkflowSnapshot>)
            .transpose()
            .map_err(sqlite_err)?;
        // Read the latest routing intention, not just the latest matching
        // fence: a newer mismatched Queue must not revive an older delegation.
        let queue=conn.query_row("SELECT e.event_index,e.at,(e.state_field='routing' AND e.reason='queued'),f.value FROM typed_ticket_events e LEFT JOIN typed_ticket_event_attributes f ON f.workspace_id=e.workspace_id AND f.ticket_id=e.ticket_id AND f.event_index=e.event_index AND f.key='orchestrator_assignment_id'
          WHERE e.workspace_id=?1 AND e.ticket_id=?2 AND (e.to_state='queued' OR (e.state_field='routing' AND e.reason='queued')) ORDER BY e.event_index DESC LIMIT 1",params![workspace,ticket],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,Option<String>>(1)?,r.get::<_,Option<bool>>(2)?.unwrap_or(false),r.get::<_,Option<String>>(3)?))).optional().map_err(sqlite_err)?;
        let original_active = original
            .as_ref()
            .is_some_and(|s| matches!(s.workflow_state.as_str(), "queued" | "inprogress"));
        if let Some((index, at, typed_queue, _)) = queue.filter(|(_, _, typed_queue, fence)| {
            current
                && (original_active || *typed_queue)
                && fence.as_deref() == Some(assignment.as_str())
        }) {
            // A case judgment is earlier only when its immutable operation key
            // identifies a case event before Queue. Missing correspondence is
            // ambiguous; neither timestamp order nor Worker state repairs it.
            let later_case:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM typed_ticket_case_history h WHERE h.workspace_id=?1 AND h.ticket_id=?2 AND h.request_fingerprint!='migration-v10'
              AND NOT EXISTS(SELECT 1 FROM typed_ticket_event_attributes operation JOIN typed_ticket_events e ON e.workspace_id=operation.workspace_id AND e.ticket_id=operation.ticket_id AND e.event_index=operation.event_index
                WHERE operation.workspace_id=h.workspace_id AND operation.ticket_id=h.ticket_id AND operation.key='operation_key' AND operation.value=h.operation_key AND e.state_field='case' AND e.event_index<?3))",params![workspace,ticket,index],|r|r.get(0)).map_err(sqlite_err)?;
            let superseded:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM typed_ticket_events WHERE workspace_id=?1 AND ticket_id=?2 AND event_index>?3 AND (state_field='case' OR to_state IN ('planning','ready','done','closed') OR (to_state='open' AND COALESCE(state_field,'')!='routing')))",params![workspace,ticket,index],|r|r.get(0)).map_err(sqlite_err)?;
            if !later_case && !superseded {
                let status = if !typed_queue
                    && original
                        .as_ref()
                        .is_some_and(|s| s.workflow_state == "inprogress")
                {
                    TicketAssignmentTrackingStatus::Inprogress
                } else {
                    TicketAssignmentTrackingStatus::Waiting
                };
                let tracking = TicketAssignmentTracking {
                    status,
                    waiting_reason: Some(
                        "Migrated fenced legacy delegation; Orchestrator confirmation pending"
                            .into(),
                    ),
                    progress: None,
                    next_action: None,
                    result: None,
                };
                let event_id = format!("migration-tracking:{assignment}");
                let at = at.unwrap_or_else(|| Utc::now().to_rfc3339());
                let json = serde_json::to_string(&tracking).map_err(sqlite_err)?;
                conn.execute("UPDATE ticket_worker_assignments SET tracking_json=?3,last_tracking_event_id=?4,tracking_updated_at=?5,tracking_migration_diagnostic=?6 WHERE workspace_id=?1 AND assignment_id=?2",params![workspace,assignment,json,event_id,at,format!("Retained original workflow snapshot and latest Queue event {index} (typed routing={typed_queue}) fenced to this current assignment")]).map_err(sqlite_err)?;
                conn.execute("INSERT INTO typed_ticket_assignment_tracking_history VALUES (?1,?2,?3,?4,?4,?5,'{\"kind\":\"migration\"}',?6)",params![workspace,ticket,assignment,event_id,json,at]).map_err(sqlite_err)?;
                continue;
            }
        }
        conn.execute("UPDATE ticket_worker_assignments SET tracking_migration_diagnostic='Historical assignment retained without tracking: absent/ambiguous or superseded original workflow and exact Queue fence; no delegation or done inferred' WHERE workspace_id=?1 AND assignment_id=?2",params![workspace,assignment]).map_err(sqlite_err)?;
    }
    Ok(())
}

pub(crate) fn verify_assignment_tracking_schema(
    conn: &Connection,
    subject_contract: bool,
) -> Result<()> {
    // Object names alone would accept same-named no-op triggers after drift.
    let canonical = Connection::open_in_memory().map_err(sqlite_err)?;
    canonical
        .execute_batch(TICKET_ASSIGNMENT_SCHEMA_SQL)
        .map_err(sqlite_err)?;
    canonical
        .execute_batch(include_str!("assignment_tracking_schema.sql"))
        .map_err(sqlite_err)?;
    let normalize = |sql: String| {
        // SQL keyword/format differences are harmless; literal case and spaces
        // are not (for example changing the principal or role in a guard).
        let mut result = String::new();
        let mut quoted = false;
        let mut chars = sql.chars().peekable();
        while let Some(ch) = chars.next() {
            if ch == '\'' {
                result.push(ch);
                if quoted && chars.peek() == Some(&'\'') {
                    result.push(chars.next().unwrap());
                } else {
                    quoted = !quoted;
                }
            } else if quoted {
                result.push(ch);
            } else if !ch.is_whitespace() {
                result.push(ch.to_ascii_lowercase());
            }
        }
        result.replace("ifnotexists", "")
    };
    // Generation is selected by the owning migration ledger, never by a trigger name.
    if subject_contract {
        canonical
            .execute_batch(crate::subject_assignment::SUBJECT_GUARDS)
            .map_err(sqlite_err)?;
        crate::subject_assignment::verify(conn)?;
    }
    for name in [
        "typed_ticket_assignment_tracking_history",
        "typed_ticket_assignment_tracking_operations",
        "typed_ticket_assignment_tracking_initialize",
        "typed_ticket_assignment_tracking_assigned",
        "typed_ticket_assignment_tracking_history_no_update",
        "typed_ticket_assignment_tracking_history_no_delete",
        "typed_ticket_assignment_tracking_operations_no_update",
        "typed_ticket_assignment_tracking_operations_no_delete",
        "ticket_current_singleton_role_idx",
        "typed_ticket_assignment_identity_no_update",
        "typed_ticket_assignment_identity_no_delete",
        "typed_ticket_assignment_events_no_update",
        "typed_ticket_assignment_events_no_delete",
        "typed_ticket_assignment_receipt_no_update",
        "typed_ticket_assignment_receipt_no_delete",
        "typed_ticket_assignment_current_principal_insert",
        "typed_ticket_assignment_current_principal_update",
        "typed_ticket_assignment_orchestrator_principal_insert",
        "typed_ticket_assignment_released_tracking_no_update",
        "typed_ticket_assignment_parent_insert",
        "typed_ticket_assignment_role_principal_insert",
        "typed_ticket_assignment_audit_parent_insert",
        "typed_ticket_assignment_operation_parent_insert",
    ] {
        let expected: String = canonical
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE name=?1",
                [name],
                |row| row.get(0),
            )
            .map_err(sqlite_err)?;
        let actual: Option<String> = conn
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE name=?1",
                [name],
                |row| row.get(0),
            )
            .optional()
            .map_err(sqlite_err)?;
        if actual.map(&normalize).as_deref() != Some(normalize(expected).as_str()) {
            return Err(TicketError::Sqlite(format!(
                "Ticket assignment tracking constraint {name} differs from canonical schema"
            )));
        }
    }
    for (table, columns) in [
        (
            "ticket_worker_assignments",
            vec![
                "workspace_id",
                "ticket_id",
                "assignment_id",
                "role",
                "principal_kind",
                "principal_id",
                "runtime_id",
                "worker_id",
                "assigned_by",
                "assigned_at",
                "tracking_json",
                "last_tracking_event_id",
                "tracking_updated_at",
                "tracking_migration_diagnostic",
            ],
        ),
        (
            "ticket_current_worker_assignments",
            vec![
                "workspace_id",
                "ticket_id",
                "role",
                "assignment_id",
                "principal_kind",
                "principal_id",
                "runtime_id",
                "worker_id",
                "updated_at",
            ],
        ),
        (
            "ticket_worker_assignment_events",
            vec![
                "workspace_id",
                "ticket_id",
                "role",
                "event_id",
                "action",
                "assignment_id",
                "previous_assignment_id",
                "actor",
                "created_at",
                "operation_id",
                "reason",
            ],
        ),
        (
            "ticket_assignment_operations",
            vec![
                "workspace_id",
                "operation_id",
                "action",
                "ticket_id",
                "role",
                "principal_kind",
                "principal_id",
                "runtime_id",
                "worker_id",
                "assignment_id",
                "expected_assignment_id",
                "created_at",
                "request_fingerprint",
                "claim_state",
                "failure_reason",
                "binding_recovery_json",
            ],
        ),
        (
            "typed_ticket_assignment_tracking_history",
            vec![
                "workspace_id",
                "ticket_id",
                "assignment_id",
                "event_id",
                "operation_key",
                "tracking_json",
                "source_context_json",
                "at",
            ],
        ),
        (
            "typed_ticket_assignment_tracking_operations",
            vec![
                "workspace_id",
                "operation_key",
                "ticket_id",
                "input_json",
                "source_context_json",
                "result_json",
            ],
        ),
    ] {
        let actual = conn
            .prepare(&format!("PRAGMA table_info({table})"))
            .map_err(sqlite_err)?
            .query_map([], |r| r.get::<_, String>(1))
            .map_err(sqlite_err)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(sqlite_err)?;
        if actual != columns {
            return Err(TicketError::Sqlite(format!(
                "Ticket-owned assignment schema {table} has unexpected columns"
            )));
        }
        // Verify type, nullability, defaults and key positions, not just names.
        let shape = |db: &Connection| -> Result<Vec<(String, String, i64, Option<String>, i64)>> {
            db.prepare(&format!("PRAGMA table_info({table})"))
                .map_err(sqlite_err)?
                .query_map([], |r| {
                    Ok((r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?))
                })
                .map_err(sqlite_err)?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(sqlite_err)
        };
        if shape(conn)? != shape(&canonical)? {
            return Err(TicketError::Sqlite(format!(
                "Ticket-owned assignment schema {table} type/default/key constraints differ"
            )));
        }
        let foreign_keys = |db: &Connection| -> Result<BTreeSet<(String, String, String, String)>> {
            db.prepare(&format!("PRAGMA foreign_key_list({table})"))
                .map_err(sqlite_err)?
                .query_map([], |r| {
                    Ok((
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, String>(6)?,
                    ))
                })
                .map_err(sqlite_err)?
                .filter_map(|r| match r {
                    Ok(key) if key.0 == "workspaces" => None,
                    other => Some(other),
                })
                .collect::<std::result::Result<BTreeSet<_>, _>>()
                .map_err(sqlite_err)
        };
        if !foreign_keys(&canonical)?.is_subset(&foreign_keys(conn)?) {
            return Err(TicketError::Sqlite(format!(
                "Ticket-owned assignment schema {table} lacks canonical foreign keys"
            )));
        }
        let expected_sql: String = canonical
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE name=?1",
                [table],
                |r| r.get(0),
            )
            .map_err(sqlite_err)?;
        let actual_sql: String = conn
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE name=?1",
                [table],
                |r| r.get(0),
            )
            .map_err(sqlite_err)?;
        let expected_sql = if subject_contract {
            expected_sql.replace(
                "CHECK(principal_kind IN ('user', 'worker', 'workspace_agent'))",
                "CHECK(principal_kind IN ('user', 'worker', 'workspace_agent', 'subject'))",
            )
        } else {
            expected_sql
        };
        let expected_sql = normalize(expected_sql);
        let actual_sql = normalize(actual_sql);
        let mut remainder = expected_sql.as_str();
        while let Some(start) = remainder.find("check(") {
            let check = &remainder[start..];
            let mut depth = 0i64;
            let mut end = None;
            for (index, ch) in check.char_indices().skip(5) {
                if ch == '(' {
                    depth += 1;
                } else if ch == ')' {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(index + 1);
                        break;
                    }
                }
            }
            let end = end.ok_or_else(|| {
                TicketError::Sqlite("canonical assignment CHECK is malformed".into())
            })?;
            if !actual_sql.contains(&check[..end]) {
                return Err(TicketError::Sqlite(format!(
                    "Ticket-owned assignment schema {table} CHECK constraint differs"
                )));
            }
            remainder = &check[end..];
        }
        let index_keys = |db: &Connection| -> Result<BTreeSet<(bool, bool, Vec<String>)>> {
            let indices = db
                .prepare(&format!("PRAGMA index_list({table})"))
                .map_err(sqlite_err)?
                .query_map([], |r| {
                    Ok((
                        r.get::<_, String>(1)?,
                        r.get::<_, bool>(2)?,
                        r.get::<_, bool>(4)?,
                    ))
                })
                .map_err(sqlite_err)?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(sqlite_err)?;
            indices
                .into_iter()
                .map(|(name, unique, partial)| {
                    let columns = db
                        .prepare(&format!("PRAGMA index_info({name})"))
                        .map_err(sqlite_err)?
                        .query_map([], |r| r.get::<_, String>(2))
                        .map_err(sqlite_err)?
                        .collect::<std::result::Result<Vec<_>, _>>()
                        .map_err(sqlite_err)?;
                    Ok((unique, partial, columns))
                })
                .collect()
        };
        if !index_keys(&canonical)?.is_subset(&index_keys(conn)?) {
            return Err(TicketError::Sqlite(format!(
                "Ticket-owned assignment schema {table} lacks canonical key/index constraints"
            )));
        }
        let violations: i64 = conn
            .query_row(
                &format!("SELECT COUNT(*) FROM pragma_foreign_key_check('{table}')"),
                [],
                |r| r.get(0),
            )
            .map_err(sqlite_err)?;
        if violations != 0 {
            return Err(TicketError::Sqlite(format!(
                "Ticket-owned assignment schema {table} foreign key violations"
            )));
        }
    }
    for name in [
        "typed_ticket_assignment_tracking_initialize",
        "typed_ticket_assignment_tracking_assigned",
        "typed_ticket_assignment_tracking_history_no_update",
        "typed_ticket_assignment_tracking_history_no_delete",
        "typed_ticket_assignment_tracking_operations_no_update",
        "typed_ticket_assignment_tracking_operations_no_delete",
    ] {
        let exists: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='trigger' AND name=?1)",
                [name],
                |r| r.get(0),
            )
            .map_err(sqlite_err)?;
        if !exists {
            return Err(TicketError::Sqlite(format!(
                "Ticket assignment tracking constraint {name} missing"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "assignment_tracking_tests.rs"]
mod tests;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TicketAssignmentsPage {
    pub items: Vec<TicketAssignmentTrackingProjection>,
    pub next_assignment_id: Option<String>,
}

/// Immutable append-order identities. The cursor must be an identity in this
/// Ticket/Workspace; timestamps and lexical IDs are not insertion order.
pub fn sqlite_ticket_assignments_page(
    conn: &Connection,
    workspace_id: &str,
    ticket_id: &str,
    after_assignment_id: Option<&str>,
    limit: usize,
) -> Result<TicketAssignmentsPage> {
    if !(1..=100).contains(&limit) {
        return Err(TicketError::Conflict(
            "Assignment page limit must be 1..100".into(),
        ));
    }
    let after: i64 = match after_assignment_id {
        None => 0,
        Some(id) => conn.query_row("SELECT rowid FROM ticket_worker_assignments WHERE workspace_id=?1 AND ticket_id=?2 AND role='orchestrator' AND assignment_id=?3",params![workspace_id,ticket_id,id],|r| r.get(0)).optional().map_err(sqlite_err)?
            .ok_or_else(|| TicketError::Conflict("Assignment cursor does not belong to this Ticket".into()))?,
    };
    let mut stmt = conn.prepare("SELECT assignment_id FROM ticket_worker_assignments WHERE workspace_id=?1 AND ticket_id=?2 AND role='orchestrator' AND rowid>?3 ORDER BY rowid LIMIT ?4").map_err(sqlite_err)?;
    let mut ids = stmt
        .query_map(
            params![workspace_id, ticket_id, after, (limit + 1) as i64],
            |r| r.get::<_, String>(0),
        )
        .map_err(sqlite_err)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(sqlite_err)?;
    let has_more = ids.len() > limit;
    ids.truncate(limit);
    let next_assignment_id = if has_more { ids.last().cloned() } else { None };
    let items = ids
        .iter()
        .map(|id| {
            sqlite_ticket_assignment_tracking(conn, workspace_id, ticket_id, id)?
                .ok_or_else(|| TicketError::Conflict("Assignment disappeared while paging".into()))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(TicketAssignmentsPage {
        items,
        next_assignment_id,
    })
}
