//! Durable domain delivery state, not a task registry. The Runtime owns the driver
//! task; this owner serializes admission, lease proofs, receipts and cursor commits
//! in the catalog database. No guest or network I/O runs inside these transactions.
use crate::{Result, plugin_host::failure, plugin_plan::StoredPluginPlan, plugin_receipt};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use server_api::*;
use sha2::{Digest, Sha256};

pub const SCHEMA_SQL: &str = include_str!("plugin_service_schema.sql");
pub const LEASE_MS: i64 = 60_000;
pub const MAX_QUEUE: u32 = 256;
pub const MAX_ATTEMPTS: u32 = 5;
pub const MAX_RETRY_MS: u32 = 300_000;
pub const RETENTION_MS: i64 = 30 * 24 * 60 * 60 * 1000;

#[derive(Clone, Debug)]
pub struct Consumer {
    pub consumer_id: String,
    pub workspace: String,
    pub runtime_id: String,
    pub worker_id: String,
    pub installation: String,
    pub plan_id: String,
    pub service: String,
    pub subscriptions: Vec<PluginTicketEventType>,
    pub cursor: i64,
    pub drain_through: i64,
    pub pause: Option<PluginServicePauseReason>,
    pub current_sequence: Option<i64>,
    pub current_event_id: Option<String>,
    pub invocation_id: Option<String>,
    pub attempt: u32,
    pub next_attempt_ms: i64,
    pub lease_token: Option<String>,
    pub lease_until_ms: i64,
}
fn decode<T: serde::de::DeserializeOwned>(json: &str) -> Result<T> {
    serde_json::from_str(json).map_err(|_| failure(PluginHostErrorCode::Unavailable))
}
fn encode<T: serde::Serialize>(value: &T) -> Result<String> {
    serde_json::to_string(value).map_err(|_| failure(PluginHostErrorCode::InvalidInput))
}
fn valid(s: &str, max: usize) -> bool {
    !s.is_empty() && s.len() <= max && !s.chars().any(char::is_control)
}

pub fn load(conn: &Connection, workspace: &str, id: &str) -> Result<Consumer> {
    if !valid(id, 512) {
        return Err(failure(PluginHostErrorCode::InvalidInput));
    }
    let row = conn.query_row("SELECT runtime_id,worker_id,installation_id,plan_id,service_name,subscriptions_json,cursor,pause_reason,current_sequence,current_event_id,invocation_id,attempt,next_attempt_ms,lease_token,lease_until_ms,drain_through FROM plugin_service_consumers WHERE workspace_id=?1 AND consumer_id=?2", params![workspace,id], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?,r.get::<_,String>(5)?,r.get::<_,i64>(6)?,r.get::<_,Option<String>>(7)?,r.get::<_,Option<i64>>(8)?,r.get::<_,Option<String>>(9)?,r.get::<_,Option<String>>(10)?,r.get::<_,u32>(11)?,r.get::<_,i64>(12)?,r.get::<_,Option<String>>(13)?,r.get::<_,i64>(14)?,r.get::<_,i64>(15)?))).optional()?.ok_or_else(||failure(PluginHostErrorCode::NotFound))?;
    Ok(Consumer {
        consumer_id: id.into(),
        workspace: workspace.into(),
        runtime_id: row.0,
        worker_id: row.1,
        installation: row.2,
        plan_id: row.3,
        service: row.4,
        subscriptions: decode(&row.5)?,
        cursor: row.6,
        drain_through: row.15,
        pause: row.7.map(|v| decode(&format!("\"{v}\""))).transpose()?,
        current_sequence: row.8,
        current_event_id: row.9,
        invocation_id: row.10,
        attempt: row.11,
        next_attempt_ms: row.12,
        lease_token: row.13,
        lease_until_ms: row.14,
    })
}
fn floor(conn: &Connection, workspace: &str) -> Result<i64> {
    Ok(conn
        .query_row(
            "SELECT removed_through FROM plugin_service_retention WHERE workspace_id=?1",
            [workspace],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or(0))
}
fn tail(conn: &Connection, workspace: &str) -> Result<i64> {
    Ok(conn.query_row("SELECT COALESCE(MAX(sequence),0) FROM typed_ticket_service_outbox WHERE workspace_id=?1",[workspace],|r|r.get::<_,i64>(0))?.max(floor(conn,workspace)?))
}

pub fn acquire(
    tx: &Transaction<'_>,
    plan: &StoredPluginPlan,
    installation: &str,
    service: &str,
    subscriptions: &[PluginTicketEventType],
    now: i64,
) -> Result<PluginServiceLease> {
    if !valid(service, 512)
        || subscriptions.is_empty()
        || subscriptions.len() > 2
        || (subscriptions.len() == 2 && subscriptions[0] == subscriptions[1])
    {
        return Err(failure(PluginHostErrorCode::InvalidInput));
    }
    let workspace = plan.plan.workspace_id();
    let execution = plan.plan.execution();
    let existing:Option<String>=tx.query_row("SELECT consumer_id FROM plugin_service_consumers WHERE workspace_id=?1 AND runtime_id=?2 AND worker_id=?3 AND installation_id=?4 AND plan_id=?5 AND service_name=?6",params![workspace,execution.runtime_id,execution.worker_id,installation,plan.plan_id,service],|r|r.get(0)).optional()?;
    let id = if let Some(id) = existing {
        let saved = load(tx, workspace, &id)?;
        if saved.subscriptions != subscriptions {
            return Err(failure(PluginHostErrorCode::Conflict));
        }
        if saved.lease_token.is_some() && saved.lease_until_ms > now {
            return Err(failure(PluginHostErrorCode::Conflict));
        }
        id
    } else {
        let count: u32 = tx.query_row(
            "SELECT count(*) FROM plugin_service_consumers WHERE workspace_id=?1",
            [workspace],
            |r| r.get(0),
        )?;
        if count >= 1024 {
            return Err(failure(PluginHostErrorCode::QuotaExceeded));
        }
        let id = uuid::Uuid::now_v7().to_string();
        tx.execute("INSERT INTO plugin_service_consumers(workspace_id,consumer_id,runtime_id,worker_id,installation_id,plan_id,service_name,subscriptions_json,cursor) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",params![workspace,id,execution.runtime_id,execution.worker_id,installation,plan.plan_id,service,encode(&subscriptions)?,tail(tx,workspace)?])?;
        id
    };
    let token = uuid::Uuid::new_v4().to_string();
    tx.execute("UPDATE plugin_service_consumers SET lease_token=?3,lease_until_ms=?4 WHERE workspace_id=?1 AND consumer_id=?2",params![workspace,id,token,now+LEASE_MS])?;
    Ok(PluginServiceLease {
        consumer_id: id,
        lease_token: token,
    })
}

pub fn fence(
    conn: &Connection,
    workspace: &str,
    lease: &PluginServiceLease,
    now: i64,
) -> Result<Consumer> {
    let c = load(conn, workspace, &lease.consumer_id)?;
    if !valid(&lease.lease_token, 512)
        || c.lease_token.as_deref() != Some(lease.lease_token.as_str())
        || c.lease_until_ms <= now
    {
        return Err(failure(PluginHostErrorCode::Conflict));
    }
    Ok(c)
}

/// Called from receipt verification in the same transaction as every effect/data
/// mutation. A stable logical invocation never suffices as an old driver's proof.
pub fn fence_invocation(
    conn: &Connection,
    record: &plugin_receipt::InvocationRecord,
    lease: Option<&PluginServiceLease>,
    now: i64,
) -> Result<()> {
    if let PluginConsumer::Consumer {
        consumer_id,
        event_id,
        export_name,
    } = &record.consumer
    {
        let lease = lease.ok_or_else(|| failure(PluginHostErrorCode::Denied))?;
        let c = fence(conn, &record.workspace_id, lease, now)?;
        if &c.consumer_id != consumer_id
            || c.current_event_id.as_ref() != Some(event_id)
            || &c.service != export_name
            || c.invocation_id.as_deref() != Some(record.invocation_id.as_str())
            || c.pause.is_some()
            || c.plan_id != record.plan_id
            || c.installation != record.installation_id
        {
            return Err(failure(PluginHostErrorCode::Denied));
        }
    } else if lease.is_some() {
        return Err(failure(PluginHostErrorCode::Denied));
    }
    Ok(())
}
fn queued(conn: &Connection, c: &Consumer) -> Result<u32> {
    let changed = c.subscriptions.contains(&PluginTicketEventType::Changed);
    let deleted = c.subscriptions.contains(&PluginTicketEventType::Deleted);
    Ok(conn.query_row("SELECT count(*) FROM (SELECT 1 FROM typed_ticket_service_outbox WHERE workspace_id=?1 AND sequence>?2 AND ((event_type='changed' AND ?3) OR (event_type='deleted' AND ?4)) ORDER BY sequence LIMIT 257)",params![c.workspace,c.cursor,changed,deleted],|r|r.get(0))?)
}
pub fn diagnostics(conn: &Connection, c: &Consumer, now: i64) -> Result<PluginServiceDiagnostics> {
    Ok(PluginServiceDiagnostics {
        consumer_id: c.consumer_id.clone(),
        pause_reason: c.pause,
        queued: queued(conn, c)?,
        attempt: c.attempt,
        current_event_id: match (&c.current_event_id, c.pause) {
            (Some(id), _) => Some(id.clone()),
            (None, Some(PluginServicePauseReason::QueueFull)) => conn.query_row(
                "SELECT event_id FROM typed_ticket_service_outbox WHERE workspace_id=?1 AND sequence>?2 AND ((event_type='changed' AND ?3) OR (event_type='deleted' AND ?4)) ORDER BY sequence LIMIT 1",
                params![c.workspace, c.cursor, c.subscriptions.contains(&PluginTicketEventType::Changed), c.subscriptions.contains(&PluginTicketEventType::Deleted)],
                |row| row.get(0),
            ).optional()?,
            _ => None,
        },
        lease_active: c.lease_token.is_some() && c.lease_until_ms > now,
    })
}
pub fn pause(tx: &Transaction<'_>, c: &Consumer, reason: PluginServicePauseReason) -> Result<()> {
    let reason = encode(&reason)?;
    tx.execute("UPDATE plugin_service_consumers SET pause_reason=?3 WHERE workspace_id=?1 AND consumer_id=?2",params![c.workspace,c.consumer_id,reason.trim_matches('"')])?;
    Ok(())
}
fn unknown_effect(conn: &Connection, c: &Consumer, now: i64) -> Result<bool> {
    let Some(id) = &c.invocation_id else {
        return Ok(false);
    };
    Ok(conn.query_row("SELECT EXISTS(SELECT 1 FROM plugin_effect_receipts WHERE workspace_id=?1 AND invocation_id=?2 AND (state IN ('pending','expired') OR json_extract(error_json,'$.effect')='indeterminate')) OR EXISTS(SELECT 1 FROM plugin_invocations WHERE workspace_id=?1 AND invocation_id=?2 AND (expired=1 OR expires_at_ms<=?3))",params![c.workspace,id,now],|r|r.get(0))?)
}

pub fn poll(
    tx: &Transaction<'_>,
    workspace: &str,
    lease: &PluginServiceLease,
    plan: &StoredPluginPlan,
    now: i64,
) -> Result<PluginServiceResponse> {
    let c = fence(tx, workspace, lease, now)?;
    // Retention is a Host operation in the same owner transaction, not a guest
    // callback. Persisting the floor makes expiry visible to every consumer.
    prune_outbox(tx, workspace, tail(tx, workspace)?, now)?;
    if c.pause.is_some() {
        return Ok(PluginServiceResponse::Diagnostics {
            diagnostics: diagnostics(tx, &c, now)?,
        });
    }
    let reason = if c.cursor < floor(tx, workspace)? {
        Some(PluginServicePauseReason::RetentionGap)
    } else if queued(tx, &c)? >= MAX_QUEUE && c.cursor >= c.drain_through {
        Some(PluginServicePauseReason::QueueFull)
    } else if unknown_effect(tx, &c, now)? {
        Some(PluginServicePauseReason::Indeterminate)
    } else {
        None
    };
    if let Some(reason) = reason {
        pause(tx, &c, reason)?;
        return Ok(PluginServiceResponse::Diagnostics {
            diagnostics: diagnostics(tx, &load(tx, workspace, &c.consumer_id)?, now)?,
        });
    }
    tx.execute("UPDATE plugin_service_consumers SET lease_until_ms=?3 WHERE workspace_id=?1 AND consumer_id=?2",params![workspace,c.consumer_id,now+LEASE_MS])?;
    if c.next_attempt_ms > now {
        return Ok(PluginServiceResponse::Idle);
    }
    if c.current_event_id.is_some() && c.attempt >= MAX_ATTEMPTS {
        pause(tx, &c, PluginServicePauseReason::AttemptsExhausted)?;
        return Ok(PluginServiceResponse::Diagnostics {
            diagnostics: diagnostics(tx, &load(tx, workspace, &c.consumer_id)?, now)?,
        });
    }
    let event=tx.query_row("SELECT sequence,event_id,event_type,occurred_at,ticket_id,domain_event_ref,content_digest,correlation_id,causation_id FROM typed_ticket_service_outbox WHERE workspace_id=?1 AND sequence>?2 AND ((event_type='changed' AND ?3) OR (event_type='deleted' AND ?4)) ORDER BY sequence LIMIT 1",params![workspace,c.cursor,c.subscriptions.contains(&PluginTicketEventType::Changed),c.subscriptions.contains(&PluginTicketEventType::Deleted)],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?,r.get::<_,String>(5)?,r.get::<_,String>(6)?,r.get::<_,String>(7)?,r.get::<_,Option<String>>(8)?))).optional()?;
    let Some((
        sequence,
        event_id,
        kind,
        occurred_at,
        ticket_id,
        domain_event_ref,
        content_digest,
        correlation_id,
        causation_id,
    )) = event
    else {
        return Ok(PluginServiceResponse::Idle);
    };
    if c.current_event_id
        .as_ref()
        .is_some_and(|id| id != &event_id)
    {
        pause(tx, &c, PluginServicePauseReason::RetentionGap)?;
        return Ok(PluginServiceResponse::Diagnostics {
            diagnostics: diagnostics(tx, &load(tx, workspace, &c.consumer_id)?, now)?,
        });
    }
    let record = plugin_receipt::begin(
        tx,
        plan,
        &c.installation,
        &PluginConsumer::Consumer {
            consumer_id: c.consumer_id.clone(),
            event_id: event_id.clone(),
            export_name: c.service.clone(),
        },
        now,
        RETENTION_MS,
    )?;
    let attempt = c.attempt + 1;
    tx.execute("UPDATE plugin_service_consumers SET current_sequence=?3,current_event_id=?4,invocation_id=?5,attempt=?6,next_attempt_ms=?7 WHERE workspace_id=?1 AND consumer_id=?2",params![workspace,c.consumer_id,sequence,event_id,record.invocation_id,attempt,now+30_000])?;
    let envelope = PluginTicketEventEnvelope {
        event_id,
        event_type: decode(&format!("\"{kind}\""))?,
        version: 1,
        occurred_at,
        ticket_id,
        domain_event_ref,
        content_digest,
        correlation_id,
        causation_id,
        attempt,
    };
    if encode(&envelope)?.len() > 64 * 1024 {
        return Err(failure(PluginHostErrorCode::QuotaExceeded));
    }
    Ok(PluginServiceResponse::Delivery {
        envelope,
        invocation_id: record.invocation_id,
        expires_at_ms: record.expires_at_ms,
    })
}

pub fn complete(
    tx: &Transaction<'_>,
    workspace: &str,
    lease: &PluginServiceLease,
    event: &str,
    result: &PluginServiceEventResult,
    now: i64,
) -> Result<()> {
    let c = fence(tx, workspace, lease, now)?;
    if c.current_event_id.as_deref() != Some(event) {
        let ack:Option<String>=tx.query_row("SELECT last_ack_event_id FROM plugin_service_consumers WHERE workspace_id=?1 AND consumer_id=?2",params![workspace,c.consumer_id],|r|r.get(0))?;
        if ack.as_deref() == Some(event) && matches!(result, PluginServiceEventResult::Ack) {
            return Ok(());
        }
        return Err(failure(PluginHostErrorCode::Conflict));
    }
    if c.pause.is_some() {
        return Err(failure(PluginHostErrorCode::Conflict));
    }
    if unknown_effect(tx, &c, now)? || matches!(result, PluginServiceEventResult::Indeterminate) {
        return pause(tx, &c, PluginServicePauseReason::Indeterminate);
    }
    match result {
        PluginServiceEventResult::Ack => {
            tx.execute("UPDATE plugin_service_consumers SET cursor=current_sequence,last_ack_event_id=current_event_id,current_sequence=NULL,current_event_id=NULL,invocation_id=NULL,attempt=0,next_attempt_ms=0 WHERE workspace_id=?1 AND consumer_id=?2",params![workspace,c.consumer_id])?;
        }
        PluginServiceEventResult::RetryAfter { delay_ms } => {
            if *delay_ms > MAX_RETRY_MS {
                return Err(failure(PluginHostErrorCode::InvalidInput));
            }
            if c.attempt >= MAX_ATTEMPTS {
                pause(tx, &c, PluginServicePauseReason::AttemptsExhausted)?;
            } else {
                tx.execute("UPDATE plugin_service_consumers SET next_attempt_ms=?3 WHERE workspace_id=?1 AND consumer_id=?2",params![workspace,c.consumer_id,now+i64::from(*delay_ms)])?;
            }
        }
        PluginServiceEventResult::Reject => pause(tx, &c, PluginServicePauseReason::Rejected)?,
        PluginServiceEventResult::Indeterminate => unreachable!(),
    }
    Ok(())
}
/// Cleanup never authorizes work, so an expired matching token may be released.
/// The API authenticates the consumer's immutable Worker identity. Exact released
/// proofs reconcile lost responses without touching any successor lease.
pub fn release(
    tx: &Transaction<'_>,
    workspace: &str,
    lease: &PluginServiceLease,
    now: i64,
) -> Result<()> {
    if !valid(&lease.consumer_id, 512) || !valid(&lease.lease_token, 512) {
        return Err(failure(PluginHostErrorCode::InvalidInput));
    }
    let released: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM plugin_service_released_proofs WHERE workspace_id=?1 AND consumer_id=?2 AND lease_token=?3)",
        params![workspace, lease.consumer_id, lease.lease_token],
        |r| r.get(0),
    )?;
    if released {
        return Ok(());
    }
    // NULL alone is not proof of this release. Only the exact current token may
    // be cleared, even if it has expired; an unrecorded stale token must fail.
    let cleared = tx.execute(
        "UPDATE plugin_service_consumers SET lease_token=NULL,lease_until_ms=0 WHERE workspace_id=?1 AND consumer_id=?2 AND lease_token=?3",
        params![workspace, lease.consumer_id, lease.lease_token],
    )?;
    if cleared != 1 {
        return Err(failure(PluginHostErrorCode::Conflict));
    }
    tx.execute(
        "INSERT INTO plugin_service_released_proofs(workspace_id,consumer_id,lease_token,released_at_ms) VALUES(?1,?2,?3,?4)",
        params![workspace, lease.consumer_id, lease.lease_token, now],
    )?;
    Ok(())
}

/// Caller supplies authenticated Workspace management authority. Audit and cursor
/// change commit atomically; unknown external effects cannot be resumed blindly.
pub fn recover(
    tx: &Transaction<'_>,
    workspace: &str,
    id: &str,
    expected: PluginServicePauseReason,
    action: &PluginServiceRecoveryAction,
    key: &str,
    actor: &str,
    reason: &str,
    now: i64,
) -> Result<()> {
    if !valid(key, 128) || !valid(actor, 512) || !valid(reason, 1024) {
        return Err(failure(PluginHostErrorCode::InvalidInput));
    }
    let hex: String = Sha256::digest(encode(&(id, expected, action, actor, reason))?)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let fp = format!("sha256:{hex}");
    let saved:Option<String>=tx.query_row("SELECT fingerprint FROM plugin_service_recovery_audit WHERE workspace_id=?1 AND operation_key=?2",params![workspace,key],|r|r.get(0)).optional()?;
    if let Some(saved) = saved {
        return if saved == fp {
            Ok(())
        } else {
            Err(failure(PluginHostErrorCode::Conflict))
        };
    }
    let c = load(tx, workspace, id)?;
    if c.pause != Some(expected) || (c.lease_token.is_some() && c.lease_until_ms > now) {
        return Err(failure(PluginHostErrorCode::Conflict));
    }
    let cursor = match action {
        PluginServiceRecoveryAction::Resume => {
            if matches!(
                expected,
                PluginServicePauseReason::Indeterminate | PluginServicePauseReason::RetentionGap
            ) || unknown_effect(tx, &c, now)?
                || c.cursor < floor(tx, workspace)?
                || (expected != PluginServicePauseReason::QueueFull && queued(tx, &c)? >= MAX_QUEUE)
            {
                return Err(failure(PluginHostErrorCode::Conflict));
            }
            c.cursor
        }
        PluginServiceRecoveryAction::Skip { event_id } => {
            if c.current_event_id.as_ref() == Some(event_id) {
                c.current_sequence
                    .ok_or_else(|| failure(PluginHostErrorCode::Conflict))?
            } else if c.current_event_id.is_none()
                && expected == PluginServicePauseReason::QueueFull
            {
                // Queue admission may stop before an invocation exists. Only the
                // exact observed queue head can be abandoned, never an arbitrary
                // later event or a guessed missing retention identity.
                let head: Option<(i64, String)> = tx.query_row(
                    "SELECT sequence,event_id FROM typed_ticket_service_outbox WHERE workspace_id=?1 AND sequence>?2 AND ((event_type='changed' AND ?3) OR (event_type='deleted' AND ?4)) ORDER BY sequence LIMIT 1",
                    params![workspace, c.cursor, c.subscriptions.contains(&PluginTicketEventType::Changed), c.subscriptions.contains(&PluginTicketEventType::Deleted)],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                ).optional()?;
                let Some((sequence, id)) = head else {
                    return Err(failure(PluginHostErrorCode::Conflict));
                };
                if &id != event_id {
                    return Err(failure(PluginHostErrorCode::Conflict));
                }
                sequence
            } else {
                return Err(failure(PluginHostErrorCode::Conflict));
            }
        }
        PluginServiceRecoveryAction::AcceptGap => {
            if expected != PluginServicePauseReason::RetentionGap {
                return Err(failure(PluginHostErrorCode::Conflict));
            }
            floor(tx, workspace)?
        }
    };
    tx.execute("INSERT INTO plugin_service_recovery_audit(workspace_id,operation_key,consumer_id,fingerprint,actor,reason,action_json,at_ms) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",params![workspace,key,id,fp,actor,reason,encode(action)?,now])?;
    // Resume preserves the blocked event identity/receipts; skip/gap explicitly
    // abandons it. Attempts reset only under this audited management decision.
    if matches!(action, PluginServiceRecoveryAction::Resume) {
        let drain = if expected == PluginServicePauseReason::QueueFull {
            tail(tx, workspace)?
        } else {
            c.drain_through
        };
        tx.execute("UPDATE plugin_service_consumers SET pause_reason=NULL,attempt=0,next_attempt_ms=0,drain_through=?3 WHERE workspace_id=?1 AND consumer_id=?2",params![workspace,id,drain])?;
    } else {
        tx.execute("UPDATE plugin_service_consumers SET cursor=?3,pause_reason=NULL,current_sequence=NULL,current_event_id=NULL,invocation_id=NULL,attempt=0,next_attempt_ms=0 WHERE workspace_id=?1 AND consumer_id=?2",params![workspace,id,cursor])?;
    }
    Ok(())
}

/// Host retention only: never remove an event before the advertised replay window.
/// Missing rows become an explicit consumer gap, not an automatic tail reset.
pub fn prune_outbox(
    tx: &Transaction<'_>,
    workspace: &str,
    through: i64,
    now: i64,
) -> Result<usize> {
    let cutoff = chrono::DateTime::from_timestamp_millis(now - RETENTION_MS)
        .ok_or_else(|| failure(PluginHostErrorCode::InvalidInput))?
        .to_rfc3339();
    let removed:Option<i64>=tx.query_row("SELECT MAX(sequence) FROM typed_ticket_service_outbox WHERE workspace_id=?1 AND sequence<=?2 AND occurred_at<?3",params![workspace,through,cutoff],|r|r.get(0))?;
    let Some(removed) = removed else { return Ok(0) };
    // Never create a hole past a newer retained event in the same Workspace.
    let blocked:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM typed_ticket_service_outbox WHERE workspace_id=?1 AND sequence<=?2 AND occurred_at>=?3)",params![workspace,removed,cutoff],|r|r.get(0))?;
    if blocked {
        return Err(failure(PluginHostErrorCode::Conflict));
    }
    let count = tx.execute(
        "DELETE FROM typed_ticket_service_outbox WHERE workspace_id=?1 AND sequence<=?2",
        params![workspace, removed],
    )?;
    tx.execute("INSERT INTO plugin_service_retention(workspace_id,removed_through) VALUES(?1,?2) ON CONFLICT(workspace_id) DO UPDATE SET removed_through=MAX(removed_through,excluded.removed_through)",params![workspace,removed])?;
    Ok(count)
}

#[cfg(test)]
#[path = "plugin_service_tests.rs"]
mod tests;
