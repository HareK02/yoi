//! Transactional Ticket extension cells, not an authorization or schema authority.
//!
//! The owner must reauthorize the exact object/collection on every call (including
//! receipt replay), validate values against the pinned closed schema, and supply
//! canonical Ticket and catalog identities. Use the Ticket owner's SQLite DB and
//! an IMMEDIATE transaction; commit before reporting success. No guest identity,
//! SQL, migration or cross-object transaction is exposed by this module.
//!
//! Cells and delete tombstones survive Ticket deletion and revocation. A parent
//! existence check under write ownership plus SQL triggers fences deletion; the
//! Workspace `deleting` state is also fenced. Explicit admin purge first requires
//! all artifacts revoked, then compares the ordered exact cell/last-write set.
use crate::{
    Error, Result,
    plugin_catalog::{self, PluginReference, PluginReferenceKind},
    plugin_receipt::{self, InvocationRecord},
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use server_api::{
    PluginDataAddress, PluginDataCell, PluginDataCollection, PluginDataEntry,
    PluginDataExpectation, PluginDataMutation, PluginDataMutationLookup, PluginDataPage,
    PluginDataReceipt, PluginEffectObservation, PluginHostError, PluginHostErrorCode,
};
use sha2::{Digest, Sha256};

pub const SCHEMA_SQL: &str = include_str!("plugin_data/schema.sql");
/// Apply exactly once in a new ordered migration, before `SCHEMA_SQL`.
pub const REFERENCE_MIGRATION_SQL: &str = include_str!("plugin_data/reference_migration.sql");
const CURSOR_LIFETIME_MS: i64 = 10 * 60 * 1000;
const MAX_CURSORS_PER_WORKSPACE: i64 = 4096;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Namespace {
    pub workspace_id: String,
    pub installation_id: String,
    pub ticket_id: String,
    pub collection: PluginDataCollection,
}
fn fail<T>(code: PluginHostErrorCode) -> Result<T> {
    // Key/identity conflicts and expired/cancelled invocations can have an older
    // committed mutation. Match the shared receipt owner's conservative signal.
    let effect = match code {
        PluginHostErrorCode::Conflict
        | PluginHostErrorCode::ReceiptExpired
        | PluginHostErrorCode::Cancelled => PluginEffectObservation::Indeterminate,
        _ => PluginEffectObservation::NotDispatched,
    };
    Err(PluginHostError {
        code,
        effect,
        diagnostic_id: uuid::Uuid::now_v7().to_string(),
    }
    .into())
}
fn valid(value: &str, max: usize) -> bool {
    !value.is_empty() && value.len() <= max && !value.chars().any(char::is_control)
}
fn encode(value: &impl Serialize) -> Result<String> {
    serde_json::to_string(value).map_err(|_| Error::Store("invalid Plugin data encoding".into()))
}
fn decode<T: serde::de::DeserializeOwned>(value: &str) -> Result<T> {
    serde_json::from_str(value).map_err(|_| Error::Store("invalid persisted Plugin data".into()))
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
fn hash(bytes: &[u8]) -> String {
    format!("sha256:{}", hex(&Sha256::digest(bytes)))
}
// Recursion is bounded by serde_json's parser; objects are explicitly sorted even
// if another dependency enables serde_json/preserve_order through feature union.
fn canonical(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let sorted: std::collections::BTreeMap<_, _> = map
                .into_iter()
                .map(|(key, value)| (key, canonical(value)))
                .collect();
            serde_json::Value::Object(sorted.into_iter().collect())
        }
        serde_json::Value::Array(values) => {
            serde_json::Value::Array(values.into_iter().map(canonical).collect())
        }
        other => other,
    }
}
fn canonical_json(value: &str) -> Result<String> {
    if value.len() > server_api::PLUGIN_DATA_VALUE_LIMIT {
        return fail(PluginHostErrorCode::QuotaExceeded);
    }
    let value = serde_json::from_str(value).map_err(|_| {
        Error::from(PluginHostError {
            code: PluginHostErrorCode::InvalidInput,
            effect: PluginEffectObservation::NotDispatched,
            diagnostic_id: uuid::Uuid::now_v7().to_string(),
        })
    })?;
    let encoded = encode(&canonical(value))?;
    if encoded.len() > server_api::PLUGIN_DATA_VALUE_LIMIT {
        return fail(PluginHostErrorCode::QuotaExceeded);
    }
    Ok(encoded)
}
fn digest(value: &impl Serialize) -> Result<String> {
    let value =
        serde_json::to_value(value).map_err(|_| Error::Store("invalid Plugin data".into()))?;
    Ok(hash(encode(&canonical(value))?.as_bytes()))
}
fn validate_namespace(ns: &Namespace) -> Result<()> {
    if !valid(&ns.workspace_id, 512)
        || !valid(&ns.installation_id, 512)
        || !valid(&ns.ticket_id, 512)
        || !valid(&ns.collection.collection, 128)
        || !valid(&ns.collection.schema_id, 128)
        || !valid(&ns.collection.schema_version, 128)
    {
        return fail(PluginHostErrorCode::InvalidInput);
    }
    Ok(())
}
fn parent(conn: &Connection, ns: &Namespace) -> Result<()> {
    validate_namespace(ns)?;
    if !conn.query_row("SELECT EXISTS(SELECT 1 FROM typed_tickets t JOIN workspaces w USING(workspace_id) JOIN plugin_installations i ON i.workspace_id=t.workspace_id WHERE t.workspace_id=?1 AND t.ticket_id=?2 AND w.state='active' AND i.installation_id=?3 AND i.uninstalled=0)",params![ns.workspace_id,ns.ticket_id,ns.installation_id], |row| row.get::<_,bool>(0))? {
        return fail(PluginHostErrorCode::NotFound);
    }
    Ok(())
}
fn invocation(
    conn: &Connection,
    record: &InvocationRecord,
    ns: &Namespace,
    now_ms: i64,
) -> Result<InvocationRecord> {
    parent(conn, ns)?;
    let record = plugin_receipt::verify_record(conn, record)?;
    if record.workspace_id != ns.workspace_id || record.installation_id != ns.installation_id {
        return fail(PluginHostErrorCode::Conflict);
    }
    if now_ms < 0 {
        return fail(PluginHostErrorCode::InvalidInput);
    }
    Ok(record)
}
fn expired(record: &InvocationRecord, now_ms: i64) -> bool {
    record.expired || now_ms >= record.expires_at_ms
}
fn active(record: &InvocationRecord, now_ms: i64) -> Result<()> {
    if expired(record, now_ms) {
        return fail(PluginHostErrorCode::ReceiptExpired);
    }
    if record.cancelled {
        return fail(PluginHostErrorCode::Cancelled);
    }
    Ok(())
}
fn validate_key(key: &str) -> Result<()> {
    if !valid(key, 256) {
        return fail(PluginHostErrorCode::InvalidInput);
    }
    Ok(())
}
fn cell(conn: &Connection, ns: &Namespace, key: &str) -> Result<PluginDataCell> {
    let saved = conn.query_row("SELECT last_write,value_json FROM plugin_data_cells WHERE workspace_id=?1 AND installation_id=?2 AND ticket_id=?3 AND collection=?4 AND schema_id=?5 AND schema_version=?6 AND cell_key=?7",
        params![ns.workspace_id,ns.installation_id,ns.ticket_id,ns.collection.collection,ns.collection.schema_id,ns.collection.schema_version,key],
        |row| Ok((row.get::<_,String>(0)?,row.get::<_,Option<String>>(1)?))).optional()?;
    Ok(match saved {
        None => PluginDataCell::NeverWritten,
        Some((last_write, Some(value_json))) => PluginDataCell::Present {
            value_json,
            last_write,
        },
        Some((last_delete, None)) => PluginDataCell::Deleted { last_delete },
    })
}
/// Current read, never a receipt replay. Parent must check the exact read grant.
pub fn get(conn: &Connection, ns: &Namespace, key: &str) -> Result<PluginDataCell> {
    parent(conn, ns)?;
    validate_key(key)?;
    cell(conn, ns, key)
}

// Persist only a bounded selector, never authorization or a snapshot of cells.
// The 256-bit CSPRNG token cannot be forged by editing namespace/offset/deadline.
fn issue_cursor(
    conn: &Connection,
    ns: &Namespace,
    scope: &str,
    after: &str,
    deadline: i64,
) -> Result<String> {
    use ring::rand::{SecureRandom, SystemRandom};
    if let Some(token) = conn.query_row("SELECT cursor_token FROM plugin_data_cursors WHERE workspace_id=?1 AND namespace_digest=?2 AND after_key=?3 AND expires_at_ms=?4",params![ns.workspace_id,scope,after,deadline],|row|row.get(0)).optional()? {
        return Ok(token);
    }
    let mut bytes = [0u8; 32];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| Error::Store("Plugin data cursor entropy unavailable".into()))?;
    let token = format!("pdc-{}", hex(&bytes));
    // The bounded INSERT predicate is evaluated under SQLite write ownership,
    // even when tests call list on a Connection rather than the owner's tx.
    let changed = conn.execute("INSERT INTO plugin_data_cursors(workspace_id,installation_id,cursor_token,namespace_digest,after_key,expires_at_ms) SELECT ?1,?2,?3,?4,?5,?6 WHERE (SELECT count(*) FROM plugin_data_cursors WHERE workspace_id=?1)<?7 ON CONFLICT(workspace_id,namespace_digest,after_key,expires_at_ms) DO NOTHING",params![ns.workspace_id,ns.installation_id,token,scope,after,deadline,MAX_CURSORS_PER_WORKSPACE])?;
    if changed == 1 {
        return Ok(token);
    }
    match conn.query_row("SELECT cursor_token FROM plugin_data_cursors WHERE workspace_id=?1 AND namespace_digest=?2 AND after_key=?3 AND expires_at_ms=?4",params![ns.workspace_id,scope,after,deadline],|row|row.get(0)).optional()? {
        Some(token)=>Ok(token),None=>fail(PluginHostErrorCode::QuotaExceeded),
    }
}
/// Stable bytewise key order including tombstones, not a cross-page snapshot.
/// Opaque persisted cursors bind namespace/offset/expiry, not access authority.
/// Current authorization is required; the owner commits cursor issuance in its
/// IMMEDIATE transaction before returning a page. Expired selectors are pruned.
pub fn list(
    conn: &Connection,
    ns: &Namespace,
    cursor: Option<&str>,
    limit: u32,
    now_ms: i64,
) -> Result<PluginDataPage> {
    parent(conn, ns)?;
    if !(1..=server_api::PLUGIN_DATA_LIST_LIMIT).contains(&limit) || now_ms < 0 {
        return fail(PluginHostErrorCode::InvalidInput);
    }
    let scope = digest(ns)?;
    conn.execute(
        "DELETE FROM plugin_data_cursors WHERE workspace_id=?1 AND expires_at_ms<=?2",
        params![ns.workspace_id, now_ms],
    )?;
    let (after, deadline) = match cursor {
        Some(raw) => {
            if raw.len() != 68
                || !raw.starts_with("pdc-")
                || !raw[4..]
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            {
                return fail(PluginHostErrorCode::InvalidInput);
            }
            let saved = conn.query_row("SELECT after_key,expires_at_ms FROM plugin_data_cursors WHERE workspace_id=?1 AND installation_id=?2 AND namespace_digest=?3 AND cursor_token=?4 AND expires_at_ms>?5",params![ns.workspace_id,ns.installation_id,scope,raw,now_ms],|row|Ok((row.get::<_,String>(0)?,row.get::<_,i64>(1)?))).optional()?;
            match saved {
                Some(value) => value,
                None => return fail(PluginHostErrorCode::InvalidInput),
            }
        }
        None => (
            String::new(),
            now_ms
                .checked_add(CURSOR_LIFETIME_MS)
                .ok_or_else(|| Error::InvalidInput("Plugin cursor time".into()))?,
        ),
    };
    let mut stmt = conn.prepare("SELECT cell_key,last_write,value_json FROM plugin_data_cells WHERE workspace_id=?1 AND installation_id=?2 AND ticket_id=?3 AND collection=?4 AND schema_id=?5 AND schema_version=?6 AND cell_key>?7 COLLATE BINARY ORDER BY cell_key COLLATE BINARY LIMIT ?8")?;
    let mut entries = stmt
        .query_map(
            params![
                ns.workspace_id,
                ns.installation_id,
                ns.ticket_id,
                ns.collection.collection,
                ns.collection.schema_id,
                ns.collection.schema_version,
                after,
                i64::from(limit) + 1
            ],
            |row| {
                let key = row.get(0)?;
                let last = row.get::<_, String>(1)?;
                let value = row.get::<_, Option<String>>(2)?;
                Ok(PluginDataEntry {
                    key,
                    cell: match value {
                        Some(value_json) => PluginDataCell::Present {
                            value_json,
                            last_write: last,
                        },
                        None => PluginDataCell::Deleted { last_delete: last },
                    },
                })
            },
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let next_cursor = if entries.len() > limit as usize {
        entries.pop();
        Some(issue_cursor(
            conn,
            ns,
            &scope,
            &entries.last().expect("nonzero limit").key,
            deadline,
        )?)
    } else {
        None
    };
    Ok(PluginDataPage {
        entries,
        next_cursor,
    })
}

struct Saved {
    namespace: String,
    fingerprint: String,
    receipt: PluginDataReceipt,
}
fn saved(
    conn: &Connection,
    record: &InvocationRecord,
    operation_key: &str,
) -> Result<Option<Saved>> {
    Ok(conn.query_row("SELECT namespace_json,fingerprint,mutation_id,result_digest FROM plugin_data_receipts WHERE workspace_id=?1 AND invocation_id=?2 AND operation_key=?3",
        params![record.workspace_id,record.invocation_id,operation_key], |row| Ok(Saved{namespace:row.get(0)?,fingerprint:row.get(1)?,receipt:PluginDataReceipt{mutation_id:row.get(2)?,result_digest:row.get(3)?}})).optional()?)
}
fn http_key(conn: &Connection, record: &InvocationRecord, key: &str) -> Result<bool> {
    Ok(conn.query_row("SELECT EXISTS(SELECT 1 FROM plugin_effect_receipts WHERE workspace_id=?1 AND invocation_id=?2 AND effect_key=?3)",params![record.workspace_id,record.invocation_id,key],|r|r.get(0))?)
}
fn address_json(ns: &Namespace, key: &str) -> Result<String> {
    encode(&(ns, key))
}
/// Lookup is scoped to one exact address; transport/DB errors are not NotRecorded.
/// Expiry takes precedence over unused keys, as for the existing HTTPS receipts.
pub fn lookup_mutation(
    conn: &Connection,
    record: &InvocationRecord,
    ns: &Namespace,
    key: &str,
    operation_key: &str,
    now_ms: i64,
) -> Result<PluginDataMutationLookup> {
    validate_key(key)?;
    validate_key(operation_key)?;
    let record = invocation(conn, record, ns, now_ms)?;
    if expired(&record, now_ms) {
        return Ok(PluginDataMutationLookup::Expired);
    }
    if record.cancelled {
        return fail(PluginHostErrorCode::Cancelled);
    }
    if let Some(saved) = saved(conn, &record, operation_key)? {
        if saved.namespace != address_json(ns, key)? {
            return fail(PluginHostErrorCode::Conflict);
        }
        return Ok(PluginDataMutationLookup::Committed(saved.receipt));
    }
    if http_key(conn, &record, operation_key)? {
        return fail(PluginHostErrorCode::Conflict);
    }
    Ok(PluginDataMutationLookup::NotRecorded)
}
fn atomic<T>(tx: &Transaction<'_>, work: impl FnOnce() -> Result<T>) -> Result<T> {
    tx.execute_batch("SAVEPOINT plugin_data_atomic")?;
    match work() {
        Ok(value) => {
            tx.execute_batch("RELEASE plugin_data_atomic")?;
            Ok(value)
        }
        Err(error) => {
            tx.execute_batch("ROLLBACK TO plugin_data_atomic; RELEASE plugin_data_atomic")?;
            Err(error)
        }
    }
}
/// Put/delete and receipt are atomic even if the caller catches an error and
/// commits unrelated work. `reference` is Host-admitted, never guest input.
/// Replay is checked before catalog/CAS and returns the original receipt only.
pub fn mutate(
    tx: &Transaction<'_>,
    record: &InvocationRecord,
    ns: &Namespace,
    key: &str,
    operation_key: &str,
    mutation: &PluginDataMutation,
    reference: &PluginReference,
    now_ms: i64,
) -> Result<PluginDataReceipt> {
    validate_key(key)?;
    validate_key(operation_key)?;
    let record = invocation(tx, record, ns, now_ms)?;
    active(&record, now_ms)?;
    let mutation = match mutation {
        PluginDataMutation::Put {
            expected,
            value_json,
        } => PluginDataMutation::Put {
            expected: expected.clone(),
            value_json: canonical_json(value_json)?,
        },
        PluginDataMutation::Delete {
            expected_last_write,
        } => PluginDataMutation::Delete {
            expected_last_write: expected_last_write.clone(),
        },
    };
    let address = address_json(ns, key)?;
    let fingerprint = digest(&("ticket-data-mutation", ns, key, &mutation))?;
    if let Some(saved) = saved(tx, &record, operation_key)? {
        if saved.namespace != address || saved.fingerprint != fingerprint {
            return fail(PluginHostErrorCode::Conflict);
        }
        return Ok(saved.receipt);
    }
    if http_key(tx, &record, operation_key)? {
        return fail(PluginHostErrorCode::Conflict);
    }
    let expected = match &mutation {
        PluginDataMutation::Put { expected, .. } => expected.clone(),
        PluginDataMutation::Delete {
            expected_last_write,
        } => PluginDataExpectation::LastWrite(expected_last_write.clone()),
    };
    if let PluginDataExpectation::LastWrite(last) = &expected {
        if !valid(last, 512) {
            return fail(PluginHostErrorCode::InvalidInput);
        }
    }
    atomic(tx, || {
        let observed = cell(tx, ns, key)?;
        let matches = match (&expected, &observed) {
            (PluginDataExpectation::NeverWritten, PluginDataCell::NeverWritten) => true,
            (
                PluginDataExpectation::LastWrite(want),
                PluginDataCell::Present { last_write, .. },
            ) => want == last_write,
            (PluginDataExpectation::LastWrite(want), PluginDataCell::Deleted { last_delete }) => {
                want == last_delete
            }
            _ => false,
        };
        if !matches {
            return fail(PluginHostErrorCode::Conflict);
        }
        if reference.installation_id != ns.installation_id {
            return fail(PluginHostErrorCode::Conflict);
        }
        let owner = digest(ns)?;
        let reference = PluginReference {
            kind: PluginReferenceKind::Data,
            owner: owner.clone(),
            ..reference.clone()
        };
        plugin_catalog::plugin_reference_put(tx, &ns.workspace_id, &reference)?;
        tx.execute("INSERT INTO plugin_data_holders(workspace_id,installation_id,owner,artifact_digest) VALUES(?1,?2,?3,?4) ON CONFLICT DO NOTHING",params![ns.workspace_id,ns.installation_id,owner,reference.artifact_digest])?;
        let mutation_id = uuid::Uuid::now_v7().to_string();
        let value = match &mutation {
            PluginDataMutation::Put { value_json, .. } => Some(value_json.as_str()),
            PluginDataMutation::Delete { .. } => None,
        };
        tx.execute("INSERT INTO plugin_data_cells(workspace_id,installation_id,ticket_id,collection,schema_id,schema_version,cell_key,last_write,value_json) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9) ON CONFLICT(workspace_id,installation_id,ticket_id,collection,schema_id,schema_version,cell_key) DO UPDATE SET last_write=excluded.last_write,value_json=excluded.value_json",
            params![ns.workspace_id,ns.installation_id,ns.ticket_id,ns.collection.collection,ns.collection.schema_id,ns.collection.schema_version,key,mutation_id,value])?;
        let result = match value {
            Some(value_json) => PluginDataCell::Present {
                value_json: value_json.into(),
                last_write: mutation_id.clone(),
            },
            None => PluginDataCell::Deleted {
                last_delete: mutation_id.clone(),
            },
        };
        let receipt = PluginDataReceipt {
            mutation_id,
            result_digest: digest(&result)?,
        };
        tx.execute("INSERT INTO plugin_data_receipts(workspace_id,invocation_id,operation_key,namespace_json,fingerprint,mutation_id,result_digest,result_json) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            params![ns.workspace_id,record.invocation_id,operation_key,address,fingerprint,receipt.mutation_id,receipt.result_digest,encode(&result)?])?;
        Ok(receipt)
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PurgeSnapshot {
    pub installation_last_operation: String,
    pub cells_digest: String,
    pub cell_count: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PurgeReceipt {
    pub purge_id: String,
    pub observed: PurgeSnapshot,
}
fn purge_fenced(conn: &Connection, workspace: &str, installation: &str) -> Result<String> {
    if !valid(workspace, 512) || !valid(installation, 512) {
        return fail(PluginHostErrorCode::InvalidInput);
    }
    let operation = conn.query_row("SELECT i.last_operation FROM plugin_installations i JOIN workspaces w USING(workspace_id) WHERE i.workspace_id=?1 AND i.installation_id=?2 AND i.uninstalled=0 AND w.state='active' AND NOT EXISTS(SELECT 1 FROM plugin_artifacts a WHERE a.workspace_id=i.workspace_id AND a.installation_id=i.installation_id AND a.revoked=0)",params![workspace,installation],|row|row.get(0)).optional()?;
    match operation {
        Some(value) => Ok(value),
        None => fail(PluginHostErrorCode::Conflict),
    }
}
/// Observe only after execution is fenced by catalog revocation. Hash a streamed,
/// ordered set of exact namespace/key/last-write/deleted tuples, not a generation.
/// Caller keeps the same IMMEDIATE transaction through this observation.
pub fn purge_snapshot(
    tx: &Transaction<'_>,
    workspace: &str,
    installation: &str,
) -> Result<PurgeSnapshot> {
    let installation_last_operation = purge_fenced(tx, workspace, installation)?;
    let mut stmt = tx.prepare("SELECT ticket_id,collection,schema_id,schema_version,cell_key,last_write,value_json IS NULL FROM plugin_data_cells WHERE workspace_id=?1 AND installation_id=?2 ORDER BY ticket_id COLLATE BINARY,collection COLLATE BINARY,schema_id COLLATE BINARY,schema_version COLLATE BINARY,cell_key COLLATE BINARY")?;
    let mut rows = stmt.query(params![workspace, installation])?;
    let mut hash = Sha256::new();
    hash.update(b"ticket-data-purge-exact-set-v1\0");
    hash.update(encode(&(workspace, installation))?.as_bytes());
    let mut count = 0u64;
    while let Some(row) = rows.next()? {
        let tuple = (
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, String>(5)?,
            row.get::<_, bool>(6)?,
        );
        let bytes = encode(&tuple)?;
        hash.update((bytes.len() as u64).to_be_bytes());
        hash.update(bytes.as_bytes());
        count += 1;
    }
    Ok(PurgeSnapshot {
        installation_last_operation,
        cells_digest: format!("sha256:{}", hex(&hash.finalize())),
        cell_count: count,
    })
}
/// Current Workspace-admin auth is external and required even for replay. Purge
/// receipts survive data removal and uninstall. No implicit uninstall or revoke.
pub fn purge(
    tx: &Transaction<'_>,
    workspace: &str,
    installation: &str,
    actor: &str,
    operation_key: &str,
    expected: &PurgeSnapshot,
) -> Result<PurgeReceipt> {
    if !valid(workspace, 512) || !valid(installation, 512) || !valid(actor, 512) {
        return fail(PluginHostErrorCode::InvalidInput);
    }
    validate_key(operation_key)?;
    let fingerprint = digest(&("ticket-data-purge", installation, expected))?;
    if let Some((saved_fingerprint,result)) = tx.query_row("SELECT fingerprint,result_json FROM plugin_data_purges WHERE workspace_id=?1 AND actor=?2 AND operation_key=?3",params![workspace,actor,operation_key],|row|Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?))).optional()? {
        if saved_fingerprint!=fingerprint { return fail(PluginHostErrorCode::Conflict); }
        return decode(&result);
    }
    atomic(tx, || {
        let observed = purge_snapshot(tx, workspace, installation)?;
        if observed != *expected {
            return fail(PluginHostErrorCode::Conflict);
        }
        let holders = tx.prepare("SELECT owner,artifact_digest FROM plugin_data_holders WHERE workspace_id=?1 AND installation_id=?2")?
            .query_map(params![workspace,installation],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
        let receipt = PurgeReceipt {
            purge_id: uuid::Uuid::now_v7().to_string(),
            observed,
        };
        tx.execute("INSERT INTO plugin_data_purges(workspace_id,actor,operation_key,installation_id,fingerprint,result_json) VALUES(?1,?2,?3,?4,?5,?6)",params![workspace,actor,operation_key,installation,fingerprint,encode(&receipt)?])?;
        tx.execute(
            "DELETE FROM plugin_data_cells WHERE workspace_id=?1 AND installation_id=?2",
            params![workspace, installation],
        )?;
        tx.execute(
            "DELETE FROM plugin_data_cursors WHERE workspace_id=?1 AND installation_id=?2",
            params![workspace, installation],
        )?;
        for (owner, artifact) in holders {
            plugin_catalog::plugin_reference_remove(
                tx,
                workspace,
                PluginReferenceKind::Data,
                &owner,
                installation,
                &artifact,
            )?;
        }
        tx.execute(
            "DELETE FROM plugin_data_holders WHERE workspace_id=?1 AND installation_id=?2",
            params![workspace, installation],
        )?;
        Ok(receipt)
    })
}

/// Convenience constructor for the owner after resolving a wire address.
pub fn namespace(
    record: &InvocationRecord,
    ticket_id: &str,
    address: &PluginDataAddress,
) -> Namespace {
    Namespace {
        workspace_id: record.workspace_id.clone(),
        installation_id: record.installation_id.clone(),
        ticket_id: ticket_id.into(),
        collection: address.collection.clone(),
    }
}

#[cfg(test)]
mod tests;
