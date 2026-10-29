//! Durable Host-issued logical invocations and all-method effect receipts.
//!
//! These owner functions are storage, NOT authentication or admission authority.
//! The caller verifies the authenticated source, current plan/installation,
//! delegation, grants, binding and result-read authority on *every* call, including
//! replay. Consumer identities come from durable Host Tool-call/event records,
//! never the guest. A saved invocation or receipt confers no execution authority.
//!
//! Use an IMMEDIATE owner transaction. Commit `begin` and `effect_admit` before
//! dispatching any network request; a returned `Dispatch` from an uncommitted or
//! failed commit is NOT permission to send. No transaction is held across I/O.
//! Every inserted receipt starts with a durable indeterminate error. Losing the
//! dispatch opportunity (even before sending) sacrifices availability, never
//! safety: no existing receipt is dispatchable, for ANY HTTP method.
//!
//! The parent owns migration registration and Workspace deletion. This module
//! neither registers a schema at runtime nor creates pins/grants/catalog state.
use crate::{
    Error, Result,
    plugin_plan::{HostExecutionIdentity, StoredPluginPlan},
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use server_api::{
    PluginConsumer, PluginEffectObservation, PluginHostError, PluginHostErrorCode,
    PluginHttpResponse,
};

pub const SCHEMA_SQL: &str = include_str!("plugin_receipt/schema.sql");
/// Buffered body ceiling; encoded allowed headers have their separate finite bound.
/// Both are protected Workspace data rather than diagnostic text.
pub const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
pub const MAX_RESPONSE_HEADER_BYTES: usize = 64 * 1024;
pub const MAX_RESPONSE_HEADERS: usize = 128;
const MAX_ERROR_BYTES: usize = 4096;

type HttpResult = std::result::Result<PluginHttpResponse, PluginHostError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvocationRecord {
    pub invocation_id: String,
    pub workspace_id: String,
    pub plan_id: String,
    pub installation_id: String,
    pub execution: HostExecutionIdentity,
    pub consumer: PluginConsumer,
    pub expires_at_ms: i64,
    pub cancelled: bool,
    pub expired: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EffectAdmission {
    /// Only a first INSERT, to be committed durably before external dispatch.
    Dispatch,
    /// Includes a saved indeterminate error for pending/unknown outcomes.
    Replay(HttpResult),
}

fn host_error(code: PluginHostErrorCode, effect: PluginEffectObservation) -> PluginHostError {
    PluginHostError {
        code,
        effect,
        diagnostic_id: uuid::Uuid::now_v7().to_string(),
    }
}
fn fail<T>(code: PluginHostErrorCode, effect: PluginEffectObservation) -> Result<T> {
    Err(host_error(code, effect).into())
}
fn invalid<T>() -> Result<T> {
    fail(
        PluginHostErrorCode::InvalidInput,
        PluginEffectObservation::NotDispatched,
    )
}
fn conflict<T>() -> Result<T> {
    // A conflicting key or logical identity might already have external effects.
    fail(
        PluginHostErrorCode::Conflict,
        PluginEffectObservation::Indeterminate,
    )
}
fn expired<T>() -> Result<T> {
    fail(
        PluginHostErrorCode::ReceiptExpired,
        PluginEffectObservation::Indeterminate,
    )
}
fn valid_identity(value: &str, max: usize) -> bool {
    !value.is_empty() && value.len() <= max && !value.chars().any(char::is_control)
}
fn valid_fingerprint(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}
fn encode(value: &impl serde::Serialize) -> Result<String> {
    serde_json::to_string(value).map_err(|_| Error::Store("invalid Plugin receipt data".into()))
}
fn decode<T: serde::de::DeserializeOwned>(value: &str) -> Result<T> {
    serde_json::from_str(value)
        .map_err(|_| Error::Store("invalid persisted Plugin receipt data".into()))
}

// The exported name is immutable input, NOT part of logical identity. Renaming
// an export/tool on the same durable call/event conflicts, rather than opening a
// fresh effect-key namespace. JSON tuples avoid separator/escaping collisions.
fn consumer_key(consumer: &PluginConsumer) -> Result<String> {
    match consumer {
        PluginConsumer::ToolCall {
            tool_call_id,
            tool_name,
        } => {
            if !valid_identity(tool_call_id, 512) || !valid_identity(tool_name, 512) {
                return invalid();
            }
            encode(&["tool_call", tool_call_id])
        }
        PluginConsumer::Consumer {
            consumer_id,
            event_id,
            export_name,
        } => {
            if !valid_identity(consumer_id, 512)
                || !valid_identity(event_id, 512)
                || !valid_identity(export_name, 512)
            {
                return invalid();
            }
            encode(&["consumer", consumer_id, event_id])
        }
    }
}

/// Establish/recover a Host UUID for a durable consumer under the stored plan.
/// The first receipt-retention deadline is fixed; retries do not extend it.
/// Replay returns the identity even after cancellation/expiry, not new execution
/// authority. `effect_admit` refuses expired invocations, including unused keys.
/// A new Tool-call/event identity must represent an explicit new request.
pub fn begin(
    tx: &Transaction<'_>,
    stored: &StoredPluginPlan,
    installation_id: &str,
    consumer: &PluginConsumer,
    now_ms: i64,
    retention_ms: i64,
) -> Result<InvocationRecord> {
    let Some(expires_at_ms) = now_ms.checked_add(retention_ms) else {
        return invalid();
    };
    if now_ms < 0 || retention_ms <= 0 || !valid_identity(installation_id, 512) {
        return invalid();
    }
    // Snapshot integrity only. Live security authority belongs to the caller.
    stored.plan.validate_canonical()?;
    if !valid_identity(&stored.plan_id, 512)
        || stored.fingerprint != stored.plan.fingerprint()?
        || !stored
            .plan
            .plugins()
            .iter()
            .any(|pin| pin.installation_id() == installation_id)
    {
        return invalid();
    }
    let workspace = stored.plan.workspace_id();
    let execution = stored.plan.execution();
    let key = consumer_key(consumer)?;
    let execution_json = encode(execution)?;
    let consumer_json = encode(consumer)?;
    let saved = tx.query_row(
        "SELECT invocation_id,plan_fingerprint FROM plugin_invocations WHERE workspace_id=?1 AND runtime_id=?2 AND worker_id=?3 AND installation_id=?4 AND consumer_key=?5",
        params![workspace, execution.runtime_id, execution.worker_id, installation_id, key],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
    ).optional()?;
    if let Some((id, fingerprint)) = saved {
        let record = lookup(tx, workspace, &id)?;
        if record.plan_id != stored.plan_id
            || fingerprint != stored.fingerprint
            || record.execution != *execution
            || record.consumer != *consumer
        {
            return conflict();
        }
        return Ok(record);
    }
    let invocation_id = uuid::Uuid::now_v7().to_string();
    tx.execute(
        "INSERT INTO plugin_invocations(workspace_id,invocation_id,plan_id,plan_fingerprint,installation_id,runtime_id,worker_id,execution_json,consumer_key,consumer_json,created_at_ms,expires_at_ms) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
        params![workspace, invocation_id, stored.plan_id, stored.fingerprint, installation_id, execution.runtime_id, execution.worker_id, execution_json, key, consumer_json, now_ms, expires_at_ms],
    )?;
    lookup(tx, workspace, &invocation_id)
}

/// Persisted identity lookup only; the caller checks current read authority.
pub fn lookup(conn: &Connection, workspace: &str, invocation_id: &str) -> Result<InvocationRecord> {
    if !valid_identity(workspace, 512) || !valid_identity(invocation_id, 512) {
        return invalid();
    }
    let row = conn.query_row(
        "SELECT plan_id,installation_id,execution_json,consumer_json,expires_at_ms,cancelled,expired FROM plugin_invocations WHERE workspace_id=?1 AND invocation_id=?2",
        params![workspace, invocation_id],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, String>(3)?, row.get::<_, i64>(4)?, row.get::<_, bool>(5)?, row.get::<_, bool>(6)?)),
    ).optional()?;
    let Some((plan_id, installation_id, execution, consumer, expires_at_ms, cancelled, expired)) =
        row
    else {
        return fail(
            PluginHostErrorCode::NotFound,
            PluginEffectObservation::NotDispatched,
        );
    };
    Ok(InvocationRecord {
        invocation_id: invocation_id.into(),
        workspace_id: workspace.into(),
        plan_id,
        installation_id,
        execution: decode(&execution)?,
        consumer: decode(&consumer)?,
        expires_at_ms,
        cancelled,
        expired,
    })
}

// Never trust a passed record's mutable flags/deadline or accept a fabricated
// plan/execution/consumer. Records may be stale only in monotonic terminal flags.
fn verify_record(conn: &Connection, supplied: &InvocationRecord) -> Result<InvocationRecord> {
    let saved = lookup(conn, &supplied.workspace_id, &supplied.invocation_id)?;
    if supplied.plan_id != saved.plan_id
        || supplied.installation_id != saved.installation_id
        || supplied.execution != saved.execution
        || supplied.consumer != saved.consumer
        || supplied.expires_at_ms != saved.expires_at_ms
    {
        return conflict();
    }
    Ok(saved)
}
fn is_expired(conn: &Connection, record: &InvocationRecord) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT expired FROM plugin_invocations WHERE workspace_id=?1 AND invocation_id=?2",
        params![record.workspace_id, record.invocation_id],
        |row| row.get(0),
    )?)
}
fn validate_key(effect_key: &str, fingerprint: &str) -> Result<()> {
    if !valid_identity(effect_key, 128) || !valid_fingerprint(fingerprint) {
        return invalid();
    }
    Ok(())
}

struct SavedEffect {
    fingerprint: String,
    state: String,
    status: Option<u16>,
    headers: Option<String>,
    body: Option<Vec<u8>>,
    error: Option<String>,
}
fn saved_effect(
    conn: &Connection,
    record: &InvocationRecord,
    key: &str,
) -> Result<Option<SavedEffect>> {
    Ok(conn.query_row(
        "SELECT fingerprint,state,response_status,response_headers,response_body,error_json FROM plugin_effect_receipts WHERE workspace_id=?1 AND invocation_id=?2 AND effect_key=?3",
        params![record.workspace_id, record.invocation_id, key],
        |row| Ok(SavedEffect { fingerprint: row.get(0)?, state: row.get(1)?, status: row.get(2)?, headers: row.get(3)?, body: row.get(4)?, error: row.get(5)? }),
    ).optional()?)
}
impl SavedEffect {
    fn result(self) -> Result<HttpResult> {
        if self.state == "expired" {
            return expired();
        }
        if let Some(json) = self.error {
            return Ok(Err(decode(&json)?));
        }
        match (self.status, self.headers, self.body) {
            (Some(status), Some(headers), Some(body)) if self.state == "completed" => {
                Ok(Ok(PluginHttpResponse {
                    status,
                    headers: decode(&headers)?,
                    body,
                }))
            }
            _ => Err(Error::Store(
                "invalid persisted Plugin receipt result".into(),
            )),
        }
    }
}

/// The parent supplies a canonical request fingerprint covering all HTTP methods,
/// binding/slot/policy, plan, path, ordered query/headers and body digest, excluding
/// secret values and remaining timeout. Never accepts/stores raw request data.
/// Commit the insertion before I/O. Repeated pending/unknown results NEVER dispatch.
pub fn effect_admit(
    tx: &Transaction<'_>,
    record: &InvocationRecord,
    effect_key: &str,
    fingerprint: &str,
    now_ms: i64,
) -> Result<EffectAdmission> {
    validate_key(effect_key, fingerprint)?;
    if now_ms < 0 {
        return invalid();
    }
    let record = verify_record(tx, record)?;
    // Retention expiry takes precedence: it must not look like an unused key.
    if now_ms >= record.expires_at_ms || is_expired(tx, &record)? {
        return expired();
    }
    if record.cancelled {
        return fail(
            PluginHostErrorCode::Cancelled,
            PluginEffectObservation::Indeterminate,
        );
    }
    if let Some(saved) = saved_effect(tx, &record, effect_key)? {
        if saved.fingerprint != fingerprint {
            return conflict();
        }
        return Ok(EffectAdmission::Replay(saved.result()?));
    }
    let unknown = host_error(
        PluginHostErrorCode::Indeterminate,
        PluginEffectObservation::Indeterminate,
    );
    tx.execute(
        "INSERT INTO plugin_effect_receipts(workspace_id,invocation_id,effect_key,fingerprint,admitted_at_ms,state,error_json) VALUES(?1,?2,?3,?4,?5,'pending',?6)",
        params![record.workspace_id, record.invocation_id, effect_key, fingerprint, now_ms, encode(&unknown)?],
    )?;
    Ok(EffectAdmission::Dispatch)
}

/// Save one bounded observed outcome. This records an already-admitted effect;
/// it does not authorize dispatch or publication. An in-flight outcome may be
/// recorded after cancellation. Once pruned, payloads can never be restored.
/// Exact completion replay is harmless; different/completed results conflict.
/// On invalid/oversized completion the original indeterminate receipt survives.
pub fn effect_complete(
    tx: &Transaction<'_>,
    record: &InvocationRecord,
    effect_key: &str,
    fingerprint: &str,
    result: &std::result::Result<PluginHttpResponse, PluginHostError>,
) -> Result<()> {
    validate_key(effect_key, fingerprint)?;
    let record = verify_record(tx, record)?;
    if is_expired(tx, &record)? {
        return expired();
    }
    let Some(saved) = saved_effect(tx, &record, effect_key)? else {
        return fail(
            PluginHostErrorCode::NotFound,
            PluginEffectObservation::NotDispatched,
        );
    };
    if saved.fingerprint != fingerprint {
        return conflict();
    }
    if saved.state == "expired" {
        return expired();
    }
    if saved.state == "completed" {
        return if saved.result()? == *result {
            Ok(())
        } else {
            conflict()
        };
    }
    let (status, headers, body, error) = match result {
        Ok(response) => {
            if !(100..=599).contains(&response.status) {
                return fail(
                    PluginHostErrorCode::InvalidInput,
                    PluginEffectObservation::ResponseObserved,
                );
            }
            if response.headers.len() > MAX_RESPONSE_HEADERS {
                return fail(
                    PluginHostErrorCode::QuotaExceeded,
                    PluginEffectObservation::ResponseObserved,
                );
            }
            let headers = encode(&response.headers)?;
            if headers.len() > MAX_RESPONSE_HEADER_BYTES || response.body.len() > MAX_RESPONSE_BYTES
            {
                return fail(
                    PluginHostErrorCode::QuotaExceeded,
                    PluginEffectObservation::ResponseObserved,
                );
            }
            (
                Some(response.status),
                Some(headers),
                Some(response.body.as_slice()),
                None,
            )
        }
        Err(error) => {
            if !valid_identity(&error.diagnostic_id, 128) {
                return fail(
                    PluginHostErrorCode::InvalidInput,
                    PluginEffectObservation::Indeterminate,
                );
            }
            let json = encode(error)?;
            if json.len() > MAX_ERROR_BYTES {
                return fail(
                    PluginHostErrorCode::InvalidInput,
                    PluginEffectObservation::Indeterminate,
                );
            }
            (None, None, None, Some(json))
        }
    };
    let changed = tx.execute(
        "UPDATE plugin_effect_receipts SET state='completed',response_status=?4,response_headers=?5,response_body=?6,error_json=?7 WHERE workspace_id=?1 AND invocation_id=?2 AND effect_key=?3 AND state='pending' AND fingerprint=?8",
        params![record.workspace_id, record.invocation_id, effect_key, status, headers, body, error, fingerprint],
    )?;
    if changed != 1 {
        return conflict();
    }
    Ok(())
}

/// Irreversible, idempotent cancellation. Does not claim to undo external effects.
pub fn cancel(tx: &Transaction<'_>, workspace: &str, invocation_id: &str) -> Result<()> {
    lookup(tx, workspace, invocation_id)?;
    tx.execute(
        "UPDATE plugin_invocations SET cancelled=1 WHERE workspace_id=?1 AND invocation_id=?2 AND cancelled=0",
        params![workspace, invocation_id],
    )?;
    Ok(())
}

/// Expire matching invocations and erase response/error payloads, never identity,
/// fingerprints or keys. Returns the number of receipts newly tombstoned. Even a
/// subsequent backwards clock cannot resurrect an explicitly expired invocation.
/// Tombstones survive until the parent's Workspace deletion cascade.
pub fn prune(tx: &Transaction<'_>, workspace: &str, now_ms: i64) -> Result<usize> {
    if !valid_identity(workspace, 512) || now_ms < 0 {
        return invalid();
    }
    // A local savepoint keeps expiry atomic if the owner handles an error and
    // commits unrelated work in its surrounding transaction.
    tx.execute_batch("SAVEPOINT plugin_receipt_prune")?;
    let work = (|| {
        tx.execute(
            "UPDATE plugin_invocations SET expired=1 WHERE workspace_id=?1 AND expires_at_ms<=?2 AND expired=0",
            params![workspace, now_ms],
        )?;
        Ok(tx.execute(
            "UPDATE plugin_effect_receipts SET state='expired',response_status=NULL,response_headers=NULL,response_body=NULL,error_json=NULL WHERE workspace_id=?1 AND state<>'expired' AND invocation_id IN (SELECT invocation_id FROM plugin_invocations WHERE workspace_id=?1 AND expired=1)",
            [workspace],
        )?)
    })();
    match work {
        Ok(count) => {
            tx.execute_batch("RELEASE plugin_receipt_prune")?;
            Ok(count)
        }
        Err(error) => {
            tx.execute_batch("ROLLBACK TO plugin_receipt_prune; RELEASE plugin_receipt_prune")?;
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests;
