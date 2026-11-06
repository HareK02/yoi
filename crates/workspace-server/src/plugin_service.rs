//! Durable domain delivery state, not a task registry. The Runtime owns the driver
//! task; this owner serializes admission, lease proofs, receipts and cursor commits
//! in the catalog database. No guest or network I/O runs inside these transactions.
use crate::{Result, plugin_host::failure, plugin_plan::StoredPluginPlan, plugin_receipt};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use server_api::*;
use sha2::{Digest, Sha256};

pub const SCHEMA_SQL: &str = include_str!("plugin_service_schema.sql");
/// Parent store migration hook: run once through the catalog migration ledger,
/// after SCHEMA_SQL for both new and existing databases, before enabling writers.
/// Do NOT append this to the already-applied Service schema. The parent owns
/// store.rs registration and the new migration SQL file (outside this scope).
pub const FAILURE_MIGRATION_SQL: &str =
    "ALTER TABLE plugin_service_consumers ADD COLUMN last_outcome_json TEXT;";
const MAX_OUTCOME_BYTES: usize = 16 * 1024;
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
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredOutcome {
    version: u16,
    event_id: String,
    invocation_id: String,
    attempt: u32,
    result: PluginServiceEventResult,
    #[serde(default)]
    failure: Option<PluginServiceFailureAttachment>,
}

// V1 invocation bytes stay opaque when adding lifecycle evidence, including
// unsupported/corrupt cause attachments. Neither slot grants writer authority
// over the other. Older readers see version 2 as Unsupported, not a false result.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredEvidence {
    version: u16,
    invocation: Option<String>,
    lifecycle: Vec<StoredLifecycleObservation>,
}
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredLifecycleObservation {
    lease_id: String,
    observation: PluginServiceLifecycleObservation,
}
#[derive(serde::Deserialize)]
struct EvidenceHeader {
    version: u16,
}

fn evidence_json(conn: &Connection, c: &Consumer) -> Result<Option<String>> {
    let (bytes, json): (Option<i64>, Option<String>) = conn.query_row(
        "SELECT length(CAST(last_outcome_json AS BLOB)),CASE WHEN length(CAST(last_outcome_json AS BLOB))<=?3 THEN last_outcome_json ELSE NULL END FROM plugin_service_consumers WHERE workspace_id=?1 AND consumer_id=?2",
        params![c.workspace, c.consumer_id, MAX_OUTCOME_BYTES], |r| Ok((r.get(0)?, r.get(1)?)))?;
    if bytes.is_some_and(|bytes| bytes > MAX_OUTCOME_BYTES as i64) {
        // Sentinel is deliberately not valid JSON; never allocate oversized data.
        return Ok(Some(String::new()));
    }
    Ok(json)
}
fn decode_evidence(json: Option<String>) -> Result<StoredEvidence> {
    let mut evidence = StoredEvidence {
        version: 2,
        invocation: None,
        lifecycle: vec![],
    };
    if let Some(json) = json {
        match serde_json::from_str::<EvidenceHeader>(&json) {
            Ok(EvidenceHeader { version: 1 }) => evidence.invocation = Some(json),
            Ok(EvidenceHeader { version: 2 }) => {
                evidence = decode(&json)?;
                if evidence.lifecycle.len() > 3
                    || evidence.lifecycle.iter().enumerate().any(|(index, slot)| {
                        let digest = slot.lease_id.strip_prefix("sha256:");
                        !digest.is_some_and(|digest| {
                            digest.len() == 64 && digest.bytes().all(|b| b.is_ascii_hexdigit())
                        }) || slot.observation.invocation().is_some_and(|identity| {
                            !valid(&identity.event_id, 1024)
                                || !valid(&identity.invocation_id, 1024)
                                || identity.attempt == 0
                        }) || evidence.lifecycle[..index].iter().any(|previous| {
                            same_lifecycle_slot(&previous.observation, &slot.observation)
                        })
                    })
                {
                    return Err(failure(PluginHostErrorCode::Unavailable));
                }
            }
            // Do not destroy unrecognized retained evidence on a write.
            _ => return Err(failure(PluginHostErrorCode::Unavailable)),
        }
    }
    Ok(evidence)
}
fn write_evidence(tx: &Transaction<'_>, c: &Consumer, evidence: &StoredEvidence) -> Result<()> {
    let json = if evidence.lifecycle.is_empty() {
        evidence
            .invocation
            .clone()
            .ok_or_else(|| failure(PluginHostErrorCode::InvalidInput))?
    } else {
        encode(evidence)?
    };
    if json.len() > MAX_OUTCOME_BYTES {
        return Err(failure(PluginHostErrorCode::QuotaExceeded));
    }
    tx.execute("UPDATE plugin_service_consumers SET last_outcome_json=?3 WHERE workspace_id=?1 AND consumer_id=?2",
        params![c.workspace, c.consumer_id, json])?;
    Ok(())
}
fn invocation_json(conn: &Connection, c: &Consumer) -> Result<Option<String>> {
    let json = evidence_json(conn, c)?;
    if json.as_ref().is_some_and(|json| {
        matches!(
            serde_json::from_str::<EvidenceHeader>(json),
            Ok(EvidenceHeader { version: 2 })
        )
    }) {
        #[derive(serde::Deserialize)]
        struct InvocationSlot {
            invocation: Option<String>,
        }
        // Decode this slot independently: damaged cleanup evidence must not hide
        // an otherwise readable owner-committed invocation result.
        return Ok(Some(
            match serde_json::from_str::<InvocationSlot>(json.as_deref().unwrap_or("")) {
                Ok(slot) => match slot.invocation {
                    Some(invocation) => invocation,
                    None => return Ok(None),
                },
                Err(_) => String::new(),
            },
        ));
    }
    Ok(json)
}
fn same_lifecycle_slot(
    a: &PluginServiceLifecycleObservation,
    b: &PluginServiceLifecycleObservation,
) -> bool {
    std::mem::discriminant(a) == std::mem::discriminant(b)
}

/// Observation-only owner transaction. The authenticated boundary checks the
/// immutable Worker; this proof cannot mutate delivery state or a successor.
/// Released proofs admit delayed response-loss evidence, but not after rotation.
pub fn observe_lifecycle(
    tx: &Transaction<'_>,
    workspace: &str,
    lease: &PluginServiceLease,
    observation: &PluginServiceLifecycleObservation,
) -> Result<()> {
    let c = load(tx, workspace, &lease.consumer_id)?;
    if !valid(&lease.lease_token, 512) {
        return Err(failure(PluginHostErrorCode::InvalidInput));
    }
    match c.lease_token.as_deref() {
        Some(token) if token == lease.lease_token => {}
        None => {
            // Release receipts append once in the owner transaction. Their
            // insertion order (not wall time/token ordering) identifies the last
            // released lifetime. Older proofs still reconcile Release, but may
            // not displace observations after a successor has also released.
            let released: Option<String> = tx.query_row(
                "SELECT lease_token FROM plugin_service_released_proofs WHERE workspace_id=?1 AND consumer_id=?2 ORDER BY rowid DESC LIMIT 1",
                params![workspace, lease.consumer_id], |r| r.get(0)).optional()?;
            if released.as_deref() != Some(lease.lease_token.as_str()) {
                return Err(failure(PluginHostErrorCode::Conflict));
            }
        }
        _ => return Err(failure(PluginHostErrorCode::Conflict)),
    }
    let mut evidence = decode_evidence(evidence_json(tx, &c)?)?;
    if let Some(identity) = observation.invocation() {
        let current_matches = c.current_event_id.as_ref() == Some(&identity.event_id)
            && c.invocation_id.as_ref() == Some(&identity.invocation_id)
            && c.attempt == identity.attempt;
        let retained_matches = evidence
            .invocation
            .as_ref()
            .and_then(|json| serde_json::from_str::<StoredOutcome>(json).ok())
            .is_some_and(|outcome| {
                outcome.version == 1
                    && outcome.event_id == identity.event_id
                    && outcome.invocation_id == identity.invocation_id
                    && outcome.attempt == identity.attempt
            });
        if !valid(&identity.event_id, 1024)
            || !valid(&identity.invocation_id, 1024)
            || identity.attempt == 0
            || (!current_matches && !retained_matches)
        {
            return Err(failure(PluginHostErrorCode::Conflict));
        }
    }
    let hex: String = Sha256::digest(lease.lease_token.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let lease_id = format!("sha256:{hex}");
    let saved = StoredLifecycleObservation {
        lease_id,
        observation: observation.clone(),
    };
    if let Some(slot) = evidence
        .lifecycle
        .iter_mut()
        .find(|slot| same_lifecycle_slot(&slot.observation, observation))
    {
        *slot = saved;
    } else {
        evidence.lifecycle.push(saved);
    }
    write_evidence(tx, &c, &evidence)
}

fn lifecycle_diagnostic(
    conn: &Connection,
    c: &Consumer,
) -> Result<Option<PluginServiceLifecycleDiagnostic>> {
    let Some(json) = evidence_json(conn, c)? else {
        return Ok(None);
    };
    let evidence = match serde_json::from_str::<EvidenceHeader>(&json) {
        Ok(EvidenceHeader { version: 1 }) => return Ok(None),
        Ok(EvidenceHeader { version: 2 }) => decode_evidence(Some(json)),
        Ok(EvidenceHeader { version: 0 }) | Err(_) => {
            return Ok(Some(PluginServiceLifecycleDiagnostic::Corrupt));
        }
        Ok(_) => return Ok(Some(PluginServiceLifecycleDiagnostic::Unsupported)),
    };
    let Ok(evidence) = evidence else {
        return Ok(Some(PluginServiceLifecycleDiagnostic::Corrupt));
    };
    let access = PluginServiceFailureAccess::authorized_service_observer();
    let mut observations = Vec::new();
    for slot in evidence.lifecycle {
        let cause = slot.observation.cause();
        let failure = PluginServiceFailureAttachment::from_cause(cause)
            .map(|attachment| attachment.diagnostic(&access))
            .unwrap_or(PluginServiceFailureDiagnostic::Corrupt);
        observations.push(PluginServiceLifecycleObservationDiagnostic {
            lease_id: slot.lease_id,
            invocation: slot.observation.invocation().cloned(),
            failure,
        });
    }
    Ok(Some(PluginServiceLifecycleDiagnostic::Known {
        observations,
    }))
}

fn save_outcome(
    tx: &Transaction<'_>,
    c: &Consumer,
    result: &PluginServiceEventResult,
    cause: Option<&PluginServiceFailureAttachment>,
) -> Result<()> {
    let attachment = cause
        .map(|cause| {
            // Complete may attach an execution cause, not a save/cleanup observation.
            match cause.decode() {
                error_contract::DecodeFailure::Known(received)
                    if matches!(
                        received.facts().cause(),
                        PluginServiceFailure::Invocation { .. }
                            | PluginServiceFailure::EffectIndeterminate
                    ) =>
                {
                    cause
                        .for_storage()
                        .map_err(|_| failure(PluginHostErrorCode::InvalidInput))
                }
                _ => Err(failure(PluginHostErrorCode::InvalidInput)),
            }
        })
        .transpose()?;
    let outcome = StoredOutcome {
        version: 1,
        event_id: c
            .current_event_id
            .clone()
            .ok_or_else(|| failure(PluginHostErrorCode::Conflict))?,
        invocation_id: c
            .invocation_id
            .clone()
            .ok_or_else(|| failure(PluginHostErrorCode::Conflict))?,
        attempt: c.attempt,
        result: result.clone(),
        failure: attachment,
    };
    let mut evidence = decode_evidence(evidence_json(tx, c)?)?;
    evidence.invocation = Some(encode(&outcome)?);
    write_evidence(tx, c, &evidence)
}

/// Owner discovery of an unknown effect must not erase an already committed
/// execution cause for this exact invocation/attempt. Unsupported evidence is
/// never guessed or relabeled as a known execution cause.
fn retained_invocation_cause(
    conn: &Connection,
    c: &Consumer,
) -> Result<Option<PluginServiceFailureAttachment>> {
    let json = invocation_json(conn, c)?;
    let Some(outcome) = json.and_then(|json| serde_json::from_str::<StoredOutcome>(&json).ok())
    else {
        return Ok(None);
    };
    if outcome.version != 1
        || Some(&outcome.event_id) != c.current_event_id.as_ref()
        || Some(&outcome.invocation_id) != c.invocation_id.as_ref()
        || outcome.attempt != c.attempt
    {
        return Ok(None);
    }
    Ok(outcome.failure.and_then(|cause| match cause.decode() {
        error_contract::DecodeFailure::Known(received)
            if matches!(
                received.facts().cause(),
                PluginServiceFailure::Invocation { .. }
            ) =>
        {
            cause.for_storage().ok()
        }
        _ => None,
    }))
}

/// The calling boundary must have current Service/Workspace observation authority.
/// Decode never turns stored presentation or an audience label into that authority.
fn outcome_diagnostic(
    conn: &Connection,
    c: &Consumer,
) -> Result<Option<PluginServiceOutcomeDiagnostic>> {
    // Bound both the DB read and the nested invocation before decoding.
    let Some(json) = invocation_json(conn, c)? else {
        return Ok(None);
    };
    // Unknown versions are observations, not fabricated execution causes.
    #[derive(serde::Deserialize)]
    struct Header {
        version: u16,
    }
    let result = match serde_json::from_str::<Header>(&json) {
        Ok(Header { version: 1 }) => match serde_json::from_str::<StoredOutcome>(&json) {
            Ok(value)
                if valid(&value.event_id, 1024)
                    && valid(&value.invocation_id, 1024)
                    && value.attempt > 0 =>
            {
                let access = PluginServiceFailureAccess::authorized_service_observer();
                PluginServiceOutcomeDiagnostic::Known {
                    event_id: value.event_id,
                    invocation_id: value.invocation_id,
                    attempt: value.attempt,
                    result: value.result,
                    failure: value.failure.map(|cause| cause.diagnostic(&access)),
                }
            }
            _ => PluginServiceOutcomeDiagnostic::Corrupt,
        },
        Ok(Header { version: 0 }) | Err(_) => PluginServiceOutcomeDiagnostic::Corrupt,
        Ok(_) => PluginServiceOutcomeDiagnostic::Unsupported,
    };
    Ok(Some(result))
}

pub fn diagnostics(conn: &Connection, c: &Consumer, now: i64) -> Result<PluginServiceDiagnostics> {
    // Authentication selects the immutable consumer; reread its mutable owner
    // outcome rather than publishing an earlier checked_consumer snapshot.
    let current = load(conn, &c.workspace, &c.consumer_id)?;
    let c = &current;
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
        last_outcome: outcome_diagnostic(conn, c)?,
        last_lifecycle: lifecycle_diagnostic(conn, c)?,
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
        if reason == PluginServicePauseReason::Indeterminate {
            let cause = retained_invocation_cause(tx, &c)?.unwrap_or(
                PluginServiceFailureAttachment::from_cause(
                    PluginServiceFailure::EffectIndeterminate,
                )
                .map_err(|_| failure(PluginHostErrorCode::Unavailable))?,
            );
            save_outcome(
                tx,
                &c,
                &PluginServiceEventResult::Indeterminate,
                Some(&cause),
            )?;
        }
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
    complete_with_failure(tx, workspace, lease, event, result, None, now)
}

pub fn complete_with_failure(
    tx: &Transaction<'_>,
    workspace: &str,
    lease: &PluginServiceLease,
    event: &str,
    result: &PluginServiceEventResult,
    cause: Option<&PluginServiceFailureAttachment>,
    now: i64,
) -> Result<()> {
    // Keep outcome+cause atomic even if a surrounding owner catches this error.
    tx.execute_batch("SAVEPOINT plugin_service_complete")?;
    match complete_inner(tx, workspace, lease, event, result, cause, now) {
        Ok(()) => {
            tx.execute_batch("RELEASE plugin_service_complete")?;
            Ok(())
        }
        Err(error) => {
            tx.execute_batch(
                "ROLLBACK TO plugin_service_complete; RELEASE plugin_service_complete",
            )?;
            Err(error)
        }
    }
}

fn complete_inner(
    tx: &Transaction<'_>,
    workspace: &str,
    lease: &PluginServiceLease,
    event: &str,
    result: &PluginServiceEventResult,
    cause: Option<&PluginServiceFailureAttachment>,
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
        let unknown =
            PluginServiceFailureAttachment::from_cause(PluginServiceFailure::EffectIndeterminate)
                .map_err(|_| failure(PluginHostErrorCode::Unavailable))?;
        // An observed invocation failure may coexist with unknown effects. The
        // existing disposition remains authoritative for the latter.
        let retained = retained_invocation_cause(tx, &c)?;
        save_outcome(
            tx,
            &c,
            &PluginServiceEventResult::Indeterminate,
            cause.or(retained.as_ref()).or(Some(&unknown)),
        )?;
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
    save_outcome(tx, &c, result, cause)?;
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
mod failure_storage_tests {
    use super::*;
    use serde_json::json;

    // Real file-backed SQLite and production schemas. Plan content is not read by
    // this storage-only fixture; API authentication/admission remains in server tests.
    struct Fixture {
        _dir: tempfile::TempDir,
        path: std::path::PathBuf,
    }
    impl Fixture {
        fn new(migrate: bool) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("service-failure.db");
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch("PRAGMA foreign_keys=ON; CREATE TABLE workspaces(workspace_id TEXT PRIMARY KEY); INSERT INTO workspaces VALUES('w');").unwrap();
            conn.execute_batch(include_str!("plugin_catalog_schema.sql"))
                .unwrap();
            conn.execute_batch(crate::plugin_plan::SCHEMA_SQL).unwrap();
            conn.execute_batch(plugin_receipt::SCHEMA_SQL).unwrap();
            conn.execute_batch(ticket::service_outbox::OUTBOX_SCHEMA_SQL)
                .unwrap();
            conn.execute_batch(SCHEMA_SQL).unwrap();
            conn.execute(
                "INSERT INTO plugin_installations VALUES('w','i','alias','package','install',0)",
                [],
            )
            .unwrap();
            let fingerprint = format!("sha256:{}", "a".repeat(64));
            conn.execute("INSERT INTO plugin_execution_plans VALUES('w','plan','owner','plan',?1,'{}','active')", [&fingerprint]).unwrap();
            conn.execute("INSERT INTO plugin_invocations(workspace_id,invocation_id,plan_id,plan_fingerprint,installation_id,runtime_id,worker_id,execution_json,consumer_key,consumer_json,created_at_ms,expires_at_ms) VALUES('w','invocation','plan',?1,'i','runtime','worker','{}','[\"consumer\"]','{}',0,60000)", [&fingerprint]).unwrap();
            // Simulates a row admitted by the existing owner before reader rollout.
            conn.execute("INSERT INTO plugin_service_consumers(workspace_id,consumer_id,runtime_id,worker_id,installation_id,plan_id,service_name,subscriptions_json,cursor,lease_token,lease_until_ms,current_sequence,current_event_id,invocation_id,attempt) VALUES('w','consumer','runtime','worker','i','plan','watch','[\"changed\"]',0,'lease',60000,1,'event','invocation',1)", []).unwrap();
            if migrate {
                conn.execute_batch(FAILURE_MIGRATION_SQL).unwrap();
            }
            Self { _dir: dir, path }
        }
        fn conn(&self) -> Connection {
            let conn = Connection::open(&self.path).unwrap();
            conn.execute_batch("PRAGMA foreign_keys=ON").unwrap();
            conn
        }
        fn lease(&self) -> PluginServiceLease {
            PluginServiceLease {
                consumer_id: "consumer".into(),
                lease_token: "lease".into(),
            }
        }
        fn complete(&self, result: PluginServiceEventResult, cause: Option<PluginServiceFailure>) {
            let attachment = cause
                .map(PluginServiceFailureAttachment::from_cause)
                .transpose()
                .unwrap();
            let mut conn = self.conn();
            let tx = conn
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .unwrap();
            complete_with_failure(
                &tx,
                "w",
                &self.lease(),
                "event",
                &result,
                attachment.as_ref(),
                100,
            )
            .unwrap();
            tx.commit().unwrap();
        }
        fn diagnostics(&self) -> PluginServiceDiagnostics {
            let conn = self.conn(); // every observation really reopens SQLite
            diagnostics(&conn, &load(&conn, "w", "consumer").unwrap(), 101).unwrap()
        }
        fn saved(&self) -> String {
            self.conn()
                .query_row(
                    "SELECT last_outcome_json FROM plugin_service_consumers",
                    [],
                    |r| r.get(0),
                )
                .unwrap()
        }
        fn replace(&self, value: &str) {
            self.conn()
                .execute(
                    "UPDATE plugin_service_consumers SET last_outcome_json=?1",
                    [value],
                )
                .unwrap();
        }
    }
    fn invocation(reason: PluginServiceExecutionFailure) -> PluginServiceFailure {
        PluginServiceFailure::Invocation { reason }
    }
    fn assert_known(
        diagnostic: PluginServiceDiagnostics,
        result: PluginServiceEventResult,
        cause: Option<PluginServiceFailure>,
    ) {
        assert_eq!(
            diagnostic.last_outcome,
            Some(PluginServiceOutcomeDiagnostic::Known {
                event_id: "event".into(),
                invocation_id: "invocation".into(),
                attempt: 1,
                result,
                failure: cause.map(|cause| PluginServiceFailureDiagnostic::Known {
                    cause,
                    explanation: cause.explanation().into()
                }),
            })
        );
    }

    #[test]
    fn additive_migration_preserves_legacy_row_then_retry_cause_survives_reopen() {
        let f = Fixture::new(false);
        f.conn().execute_batch(FAILURE_MIGRATION_SQL).unwrap();
        assert!(f.diagnostics().last_outcome.is_none());
        let cause = invocation(PluginServiceExecutionFailure::Trap);
        let result = PluginServiceEventResult::RetryAfter { delay_ms: 1_000 };
        f.complete(result.clone(), Some(cause));
        let diagnostic = f.diagnostics();
        assert_eq!(diagnostic.pause_reason, None);
        assert_eq!(diagnostic.attempt, 1);
        assert_eq!(diagnostic.current_event_id.as_deref(), Some("event"));
        assert_known(diagnostic, result, Some(cause));
        let c = load(&f.conn(), "w", "consumer").unwrap();
        assert_eq!(c.cursor, 0);
        assert_eq!(c.invocation_id.as_deref(), Some("invocation"));
        assert_eq!(c.next_attempt_ms, 1_100);
    }

    #[test]
    fn accepted_rejection_and_owner_unknown_effect_keep_existing_dispositions() {
        let f = Fixture::new(true);
        let cause = invocation(PluginServiceExecutionFailure::GuestRejected);
        f.complete(PluginServiceEventResult::Reject, Some(cause));
        assert_eq!(
            f.diagnostics().pause_reason,
            Some(PluginServicePauseReason::Rejected)
        );
        assert_known(
            f.diagnostics(),
            PluginServiceEventResult::Reject,
            Some(cause),
        );

        let f = Fixture::new(true);
        // An already admitted all-method effect has no observed response. Ack
        // cannot advance the cursor, regardless of the attachment or guest prose.
        f.conn().execute("INSERT INTO plugin_effect_receipts(workspace_id,invocation_id,effect_key,fingerprint,admitted_at_ms,state,error_json) VALUES('w','invocation','effect',?1,1,'pending',?2)",
            params![format!("sha256:{}", "a".repeat(64)), json!({"code":"indeterminate","effect":"indeterminate"}).to_string()]).unwrap();
        f.complete(PluginServiceEventResult::Ack, None);
        assert_eq!(
            f.diagnostics().pause_reason,
            Some(PluginServicePauseReason::Indeterminate)
        );
        assert_known(
            f.diagnostics(),
            PluginServiceEventResult::Indeterminate,
            Some(PluginServiceFailure::EffectIndeterminate),
        );
        assert_eq!(load(&f.conn(), "w", "consumer").unwrap().cursor, 0);
        let receipts: u32 = f
            .conn()
            .query_row("SELECT count(*) FROM plugin_effect_receipts", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(receipts, 1);
    }

    #[test]
    fn mixed_absent_unknown_corrupt_and_hostile_stored_presentation_are_read_safely() {
        let f = Fixture::new(true);
        assert!(f.diagnostics().last_outcome.is_none()); // historical row
        f.complete(PluginServiceEventResult::RetryAfter { delay_ms: 1 }, None);
        assert_known(
            f.diagnostics(),
            PluginServiceEventResult::RetryAfter { delay_ms: 1 },
            None,
        );
        let cause = invocation(PluginServiceExecutionFailure::Deadline);
        f.complete(
            PluginServiceEventResult::RetryAfter { delay_ms: 1 },
            Some(cause),
        );
        let original: serde_json::Value = serde_json::from_str(&f.saved()).unwrap();
        let mut envelope: serde_json::Value =
            serde_json::from_str(original["failure"].as_str().unwrap()).unwrap();
        envelope["explanation"] = json!("credential-secret /private/path raw body");
        envelope["audience"] = json!("browser");
        envelope["context"]["diagnostics"] =
            json!([{ "code":"secret", "explanation":"credential-secret" }]);
        let mut outcome = original.clone();
        outcome["failure"] = json!(envelope.to_string());
        f.replace(&outcome.to_string());
        assert_known(
            f.diagnostics(),
            PluginServiceEventResult::RetryAfter { delay_ms: 1 },
            Some(cause),
        );
        assert!(
            !serde_json::to_string(&f.diagnostics())
                .unwrap()
                .contains("credential-secret")
        );

        envelope["version"] = json!(99);
        outcome["failure"] = json!(envelope.to_string());
        f.replace(&outcome.to_string());
        assert!(matches!(
            f.diagnostics().last_outcome,
            Some(PluginServiceOutcomeDiagnostic::Known {
                failure: Some(PluginServiceFailureDiagnostic::Unsupported),
                ..
            })
        ));
        envelope["version"] = json!(1);
        envelope["reason"] = json!("future_reason");
        outcome["failure"] = json!(envelope.to_string());
        f.replace(&outcome.to_string());
        assert!(matches!(
            f.diagnostics().last_outcome,
            Some(PluginServiceOutcomeDiagnostic::Known {
                failure: Some(PluginServiceFailureDiagnostic::Unsupported),
                ..
            })
        ));
        for corrupt in ["{broken".to_string(), "x".repeat(4097)] {
            outcome = original.clone();
            outcome["failure"] = json!(corrupt);
            f.replace(&outcome.to_string());
            assert!(matches!(
                f.diagnostics().last_outcome,
                Some(PluginServiceOutcomeDiagnostic::Known {
                    failure: Some(PluginServiceFailureDiagnostic::Corrupt),
                    ..
                })
            ));
        }
        outcome = original;
        outcome["version"] = json!(99);
        f.replace(&outcome.to_string());
        assert_eq!(
            f.diagnostics().last_outcome,
            Some(PluginServiceOutcomeDiagnostic::Unsupported)
        );
        for corrupt in ["{broken".to_string(), "x".repeat(MAX_OUTCOME_BYTES + 1)] {
            f.replace(&corrupt);
            assert_eq!(
                f.diagnostics().last_outcome,
                Some(PluginServiceOutcomeDiagnostic::Corrupt)
            );
        }
        assert_eq!(f.diagnostics().pause_reason, None);
        assert_eq!(load(&f.conn(), "w", "consumer").unwrap().cursor, 0);
    }

    #[test]
    fn writer_normalizes_untrusted_presentation_before_owner_commit() {
        let f = Fixture::new(true);
        let cause = invocation(PluginServiceExecutionFailure::Guest);
        let attachment = PluginServiceFailureAttachment::from_cause(cause).unwrap();
        let string: String =
            serde_json::from_value(serde_json::to_value(attachment).unwrap()).unwrap();
        let mut envelope: serde_json::Value = serde_json::from_str(&string).unwrap();
        envelope["explanation"] = json!("credential-secret raw source /private/path");
        envelope["context"]["correlation"] = json!("credential-secret");
        envelope["audience"] = json!("storage");
        let attachment: PluginServiceFailureAttachment =
            serde_json::from_value(json!(envelope.to_string())).unwrap();
        let mut conn = f.conn();
        let tx = conn.transaction().unwrap();
        complete_with_failure(
            &tx,
            "w",
            &f.lease(),
            "event",
            &PluginServiceEventResult::RetryAfter { delay_ms: 1 },
            Some(&attachment),
            100,
        )
        .unwrap();
        tx.commit().unwrap();
        assert!(!f.saved().contains("credential-secret"));
        assert_known(
            f.diagnostics(),
            PluginServiceEventResult::RetryAfter { delay_ms: 1 },
            Some(cause),
        );
    }

    #[test]
    fn unknown_effect_preserves_a_committed_execution_cause_for_the_same_attempt_only() {
        let f = Fixture::new(true);
        let cause = invocation(PluginServiceExecutionFailure::Deadline);
        f.complete(
            PluginServiceEventResult::RetryAfter { delay_ms: 1 },
            Some(cause),
        );
        let c = load(&f.conn(), "w", "consumer").unwrap();
        assert!(retained_invocation_cause(&f.conn(), &c).unwrap().is_some());
        f.complete(PluginServiceEventResult::Indeterminate, None);
        assert_known(
            f.diagnostics(),
            PluginServiceEventResult::Indeterminate,
            Some(cause),
        );
        let mut next = c;
        next.attempt += 1;
        assert!(
            retained_invocation_cause(&f.conn(), &next)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn failed_cause_write_rolls_back_disposition_even_when_outer_owner_commits() {
        let f = Fixture::new(true);
        f.complete(
            PluginServiceEventResult::RetryAfter { delay_ms: 1 },
            Some(invocation(PluginServiceExecutionFailure::Deadline)),
        );
        let saved = f.saved();
        f.conn().execute_batch("CREATE TRIGGER fail_service_cause BEFORE UPDATE OF last_outcome_json ON plugin_service_consumers BEGIN SELECT RAISE(ABORT,'injected save failure'); END;").unwrap();
        let mut conn = f.conn();
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .unwrap();
        assert!(
            complete_with_failure(
                &tx,
                "w",
                &f.lease(),
                "event",
                &PluginServiceEventResult::Ack,
                None,
                100
            )
            .is_err()
        );
        tx.commit().unwrap();
        drop(conn);
        assert_eq!(f.saved(), saved);
        let c = load(&f.conn(), "w", "consumer").unwrap();
        assert_eq!(c.cursor, 0);
        assert_eq!(c.current_event_id.as_deref(), Some("event"));
        assert_eq!(c.invocation_id.as_deref(), Some("invocation"));
        assert_eq!(c.attempt, 1);
        assert_eq!(c.pause, None);
    }

    #[test]
    fn lifecycle_slots_preserve_invocation_bytes_and_survive_new_outcomes() {
        let f = Fixture::new(true);
        f.complete(
            PluginServiceEventResult::RetryAfter { delay_ms: 1 },
            Some(invocation(PluginServiceExecutionFailure::Deadline)),
        );
        let mut original: serde_json::Value = serde_json::from_str(&f.saved()).unwrap();
        // Future/corrupt attachments stay byte-for-byte intact when lifecycle
        // evidence is added; they are not promoted to fresh writer capabilities.
        for attachment in ["{broken".to_string(), {
            let mut envelope: serde_json::Value =
                serde_json::from_str(original["failure"].as_str().unwrap()).unwrap();
            envelope["version"] = json!(99);
            envelope.to_string()
        }] {
            original["failure"] = json!(attachment);
            let bytes = original.to_string();
            f.replace(&bytes);
            let mut conn = f.conn();
            let tx = conn.transaction().unwrap();
            observe_lifecycle(
                &tx,
                "w",
                &f.lease(),
                &PluginServiceLifecycleObservation::Stop {
                    reason: PluginServiceExecutionFailure::Guest,
                },
            )
            .unwrap();
            tx.commit().unwrap();
            assert_eq!(
                serde_json::from_str::<StoredEvidence>(&f.saved())
                    .unwrap()
                    .invocation
                    .as_deref(),
                Some(bytes.as_str())
            );
            assert!(matches!(
                f.diagnostics().last_outcome,
                Some(PluginServiceOutcomeDiagnostic::Known {
                    result: PluginServiceEventResult::RetryAfter { delay_ms: 1 },
                    ..
                })
            ));
            assert!(
                matches!(f.diagnostics().last_lifecycle, Some(PluginServiceLifecycleDiagnostic::Known { observations }) if observations.len() == 1)
            );
        }
        // A later invocation result updates only its own slot.
        f.complete(PluginServiceEventResult::Ack, None);
        assert_known(f.diagnostics(), PluginServiceEventResult::Ack, None);
        assert!(
            matches!(f.diagnostics().last_lifecycle, Some(PluginServiceLifecycleDiagnostic::Known { observations }) if observations.len() == 1)
        );
        let original = f.saved();
        let mut damaged: serde_json::Value = serde_json::from_str(&original).unwrap();
        damaged["lifecycle"][0]["lease_id"] = json!("credential-secret");
        f.replace(&damaged.to_string());
        assert_known(f.diagnostics(), PluginServiceEventResult::Ack, None);
        assert_eq!(
            f.diagnostics().last_lifecycle,
            Some(PluginServiceLifecycleDiagnostic::Corrupt)
        );
        assert!(
            !serde_json::to_string(&f.diagnostics())
                .unwrap()
                .contains("credential-secret")
        );
    }

    #[test]
    fn lifecycle_observation_fences_identity_released_proof_and_successor_without_mutating_delivery()
     {
        let f = Fixture::new(true);
        let observation = PluginServiceLifecycleObservation::CompletionUnconfirmed {
            invocation: PluginServiceInvocationIdentity {
                event_id: "event".into(),
                invocation_id: "invocation".into(),
                attempt: 1,
            },
        };
        let mut conn = f.conn();
        let tx = conn.transaction().unwrap();
        observe_lifecycle(&tx, "w", &f.lease(), &observation).unwrap();
        let before = evidence_json(&tx, &load(&tx, "w", "consumer").unwrap()).unwrap();
        for wrong in [
            PluginServiceInvocationIdentity {
                event_id: "other".into(),
                invocation_id: "invocation".into(),
                attempt: 1,
            },
            PluginServiceInvocationIdentity {
                event_id: "event".into(),
                invocation_id: "other".into(),
                attempt: 1,
            },
            PluginServiceInvocationIdentity {
                event_id: "event".into(),
                invocation_id: "invocation".into(),
                attempt: 2,
            },
        ] {
            assert!(
                observe_lifecycle(
                    &tx,
                    "w",
                    &f.lease(),
                    &PluginServiceLifecycleObservation::CompletionUnconfirmed { invocation: wrong }
                )
                .is_err()
            );
        }
        assert_eq!(
            evidence_json(&tx, &load(&tx, "w", "consumer").unwrap()).unwrap(),
            before
        );
        release(&tx, "w", &f.lease(), 100).unwrap();
        observe_lifecycle(
            &tx,
            "w",
            &f.lease(),
            &PluginServiceLifecycleObservation::ReleaseUnconfirmed,
        )
        .unwrap();
        let fake = PluginServiceLease {
            consumer_id: "consumer".into(),
            lease_token: "not-issued".into(),
        };
        assert!(observe_lifecycle(&tx, "w", &fake, &observation).is_err());
        // Rotate only the test lease boundary; immutable consumer/event/receipt
        // identity is unchanged. A delayed old observation cannot mutate it.
        tx.execute(
            "UPDATE plugin_service_consumers SET lease_token='successor',lease_until_ms=60000",
            [],
        )
        .unwrap();
        assert!(observe_lifecycle(&tx, "w", &f.lease(), &observation).is_err());
        let successor = PluginServiceLease {
            consumer_id: "consumer".into(),
            lease_token: "successor".into(),
        };
        release(&tx, "w", &successor, 100).unwrap(); // same timestamp, later receipt
        assert!(observe_lifecycle(&tx, "w", &f.lease(), &observation).is_err());
        observe_lifecycle(
            &tx,
            "w",
            &successor,
            &PluginServiceLifecycleObservation::ReleaseUnconfirmed,
        )
        .unwrap();
        tx.commit().unwrap();
        drop(conn);
        let c = load(&f.conn(), "w", "consumer").unwrap();
        assert_eq!(c.cursor, 0);
        assert_eq!(c.current_event_id.as_deref(), Some("event"));
        assert_eq!(c.invocation_id.as_deref(), Some("invocation"));
        assert_eq!(c.attempt, 1);
        assert_eq!(c.next_attempt_ms, 0);
        assert_eq!(c.pause, None);
        assert_eq!(
            f.diagnostics().last_outcome,
            None,
            "recording uncertainty is not an invocation commit"
        );
        assert!(
            matches!(f.diagnostics().last_lifecycle, Some(PluginServiceLifecycleDiagnostic::Known { observations }) if observations.len() == 2)
        );
    }

    #[test]
    fn lifecycle_write_failure_and_unrecognized_root_do_not_destroy_retained_evidence() {
        let f = Fixture::new(true);
        f.complete(PluginServiceEventResult::RetryAfter { delay_ms: 1 }, None);
        let saved = f.saved();
        f.conn().execute_batch("CREATE TRIGGER fail_lifecycle BEFORE UPDATE OF last_outcome_json ON plugin_service_consumers BEGIN SELECT RAISE(ABORT,'injected observation failure'); END;").unwrap();
        let mut conn = f.conn();
        let tx = conn.transaction().unwrap();
        assert!(
            observe_lifecycle(
                &tx,
                "w",
                &f.lease(),
                &PluginServiceLifecycleObservation::ReleaseUnconfirmed
            )
            .is_err()
        );
        tx.commit().unwrap();
        assert_eq!(f.saved(), saved);
        f.conn()
            .execute_batch("DROP TRIGGER fail_lifecycle")
            .unwrap();
        for retained in ["{broken", r#"{"version":99,"future":"preserve"}"#] {
            f.replace(retained);
            let tx = conn.transaction().unwrap();
            assert!(
                observe_lifecycle(
                    &tx,
                    "w",
                    &f.lease(),
                    &PluginServiceLifecycleObservation::ReleaseUnconfirmed
                )
                .is_err()
            );
            tx.commit().unwrap();
            assert_eq!(f.saved(), retained);
        }
        assert_eq!(load(&f.conn(), "w", "consumer").unwrap().cursor, 0);
    }

    #[test]
    fn cleanup_or_completion_observation_cannot_be_saved_as_invocation_failure() {
        let f = Fixture::new(true);
        for cause in [
            PluginServiceFailure::CompletionUnconfirmed,
            PluginServiceFailure::ReleaseUnconfirmed,
            PluginServiceFailure::Stop {
                reason: PluginServiceExecutionFailure::Guest,
            },
        ] {
            let attachment = PluginServiceFailureAttachment::from_cause(cause).unwrap();
            let mut conn = f.conn();
            let tx = conn.transaction().unwrap();
            assert!(
                complete_with_failure(
                    &tx,
                    "w",
                    &f.lease(),
                    "event",
                    &PluginServiceEventResult::Ack,
                    Some(&attachment),
                    100
                )
                .is_err()
            );
            tx.commit().unwrap();
            assert!(f.diagnostics().last_outcome.is_none());
            assert_eq!(load(&f.conn(), "w", "consumer").unwrap().cursor, 0);
        }
    }
}

#[cfg(test)]
#[path = "plugin_service_tests.rs"]
mod tests;
