//! Grant-owned connection evidence, never a second grant state or retry writer.
//! The fresh owner join excludes superseded generations. A failed activation may
//! describe a proposed reconnect, but remains bound to the observed grant generation.
use protocol::background_failure::StoredFailure;
use rusqlite::{Connection, params};
use server_api::ExternalWorkdirConnectionClosedReason;

use crate::{Result, store::ExternalWorkdirGrantRecord};

pub(crate) const SCHEMA: &str = include_str!("external_workdir_connection_observations.sql");
pub(crate) const MIGRATION_NAME: &str = "Independent External Workdir connection failure observations";

pub struct ConnectionObservation {
    pub grant_generation: u64,
    pub connection_generation: u64,
    pub closed_reason: ExternalWorkdirConnectionClosedReason,
    pub failures: Vec<StoredFailure>,
}

pub(crate) fn record(
    conn: &Connection,
    grant: &ExternalWorkdirGrantRecord,
    connection_generation: u64,
    reason: ExternalWorkdirConnectionClosedReason,
    failures: Option<&str>,
    now: &str,
) -> Result<bool> {
    let reason = serde_json::to_value(reason).expect("closed reason serialization");
    // No authority mutation: this conditional insert only attributes evidence to
    // the current provider and accepted generation or its rejected reconnect.
    Ok(conn.execute(
        "INSERT INTO external_workdir_connection_observations
         (workspace_id,grant_id,provider_instance_id,grant_generation,connection_generation,closed_reason,failures_json,observed_at)
         SELECT workspace_id,grant_id,provider_instance_id,generation,?4,?5,?6,?7
         FROM external_workdir_grants
         WHERE workspace_id=?1 AND grant_id=?2 AND provider_instance_id=?3
           AND (generation=?4 OR (status='offline' AND generation+1=?4 AND ?5='activation_failed'))",
        params![grant.workspace_id, grant.grant_id, grant.provider_instance_id,
            connection_generation as i64, reason.as_str().expect("closed reason string"), failures, now],
    )? > 0)
}

pub(crate) fn read(
    conn: &Connection,
    workspace: &str,
    grant: &str,
) -> Result<Vec<ConnectionObservation>> {
    let mut statement = conn.prepare(
        "SELECT o.grant_generation,o.connection_generation,o.closed_reason,o.failures_json
         FROM external_workdir_connection_observations o
         JOIN external_workdir_grants g ON g.workspace_id=o.workspace_id AND g.grant_id=o.grant_id
           AND g.provider_instance_id=o.provider_instance_id AND g.generation=o.grant_generation
         WHERE g.workspace_id=?1 AND g.grant_id=?2 ORDER BY o.observation_id",
    )?;
    let rows = statement.query_map(params![workspace, grant], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?, row.get::<_, String>(2)?, row.get::<_, Option<String>>(3)?))
    })?;
    let mut observations = Vec::new();
    for row in rows {
        let (grant_generation, connection_generation, reason, raw) = row?;
        // Optional historical evidence is not an alternate source of causes.
        let Ok(closed_reason) = serde_json::from_value(serde_json::Value::String(reason)) else {
            continue;
        };
        let failures = raw.and_then(|raw| serde_json::from_str::<Vec<String>>(&raw).ok())
            .unwrap_or_default().into_iter().map(StoredFailure::from_storage).collect();
        observations.push(ConnectionObservation {
            grant_generation: grant_generation as u64,
            connection_generation: connection_generation as u64,
            closed_reason,
            failures,
        });
    }
    Ok(observations)
}
