//! Immutable post-adoption evidence in the configuration owner's existing store.
//!
//! The receipt remains the authority for adoption. These rows only record external
//! publication observations; absence, storage failure and unconfirmed Runtime results
//! never authorize replay. The receipt FK and writer comparison bind every row to the
//! same Workspace, full request fingerprint, source and editing history.
use error_contract::codec::FailureEnvelope;
use error_contract::{Audience, Budget, WireDetails, encode_projected_failure};
use protocol::background_failure::{BackgroundFailure, StoredFailure};
use rusqlite::params;
use server_api::{
    ConfigurationPublicationOutcome as Outcome, ConfigurationPublicationStage as Stage,
};

use crate::config_adoption::{ConfigurationAdoptionResult, receipt, require_owner};
use crate::configuration_failure::ConfigurationFailureOperation;
use crate::{Error, Result, SqliteWorkspaceStore};

pub(crate) const SCHEMA: &str = include_str!("config_publication.sql");

#[derive(Clone)]
pub(crate) struct PublicationIdentity {
    pub request_fingerprint: String,
    pub receipt: ConfigurationAdoptionResult,
}

#[derive(Debug)]
pub(crate) struct PublicationObservation {
    pub stage: Stage,
    pub runtime_id: Option<String>,
    pub projection_digest: Option<String>,
    pub outcome: Outcome,
    pub failure: Option<StoredFailure>,
}

pub(crate) struct PublicationRecord {
    pub identity: PublicationIdentity,
    pub observations: Vec<PublicationObservation>,
}

fn invalid() -> Error {
    Error::RegistryInconsistency("configuration publication evidence identity mismatch".into())
}
fn stage_key(stage: Stage) -> &'static str {
    match stage {
        Stage::Dispatch => "dispatch",
        Stage::Projection => "projection",
        Stage::Runtime => "runtime",
    }
}
fn outcome_key(outcome: Outcome) -> &'static str {
    match outcome {
        Outcome::Observed => "observed",
        Outcome::Unconfirmed => "unconfirmed",
    }
}

impl SqliteWorkspaceStore {
    /// Trusted publication continuation only. This does not adopt source or send
    /// notifications, and cannot turn a loaded Storage label into writer authority.
    pub(crate) fn record_configuration_publication(
        &self,
        identity: &PublicationIdentity,
        stage: Stage,
        runtime_id: Option<&str>,
        projection_digest: Option<&str>,
        outcome: Outcome,
        failure: Option<&BackgroundFailure>,
    ) -> Result<()> {
        self.with_immediate_transaction(|tx| {
            let result = &identity.receipt;
            let Some((fingerprint, retained)) = receipt(tx, &result.workspace_id, &result.operation_id)? else {
                return Err(invalid());
            };
            if fingerprint != identity.request_fingerprint || retained != *result {
                return Err(invalid());
            }
            tx.execute(
                "INSERT INTO workspace_configuration_publication_observations(workspace_id,operation_id,stage,runtime_id,projection_digest,outcome,failure_cause) VALUES(?1,?2,?3,?4,?5,?6,?7)",
                params![result.workspace_id, result.operation_id, stage_key(stage), runtime_id.unwrap_or_default(), projection_digest, outcome_key(outcome), failure.map(BackgroundFailure::as_storage_str)],
            )?;
            Ok(())
        })
    }

    pub(crate) fn configuration_publication(
        &self,
        workspace: &str,
        account: &str,
        operation: &str,
    ) -> Result<Option<PublicationRecord>> {
        self.with_conn(|conn| {
            // Authorization and receipt/observation reads share one DB snapshot.
            let tx = conn.unchecked_transaction()?;
            require_owner(&tx, workspace, account, ConfigurationFailureOperation::Publication)?;
            let Some((request_fingerprint, result)) = receipt(&tx, workspace, operation)? else {
                return Ok(None);
            };
            if result.workspace_id != workspace || result.operation_id != operation {
                return Err(invalid());
            }
            let mut query = tx.prepare("SELECT stage,runtime_id,projection_digest,outcome,failure_cause FROM workspace_configuration_publication_observations WHERE workspace_id=?1 AND operation_id=?2 ORDER BY CASE stage WHEN 'dispatch' THEN 0 WHEN 'projection' THEN 1 ELSE 2 END,runtime_id")?;
            let rows = query.query_map(params![workspace, operation], |row| {
                Ok((row.get::<_,String>(0)?, row.get::<_,String>(1)?, row.get::<_,Option<String>>(2)?, row.get::<_,String>(3)?, row.get::<_,Option<String>>(4)?))
            })?;
            let observations = rows.map(|row| {
                let (stage, runtime_id, projection_digest, outcome, failure) = row?;
                Ok(PublicationObservation {
                    stage: serde_json::from_value(serde_json::Value::String(stage)).map_err(|_| invalid())?,
                    runtime_id: (!runtime_id.is_empty()).then_some(runtime_id),
                    projection_digest,
                    outcome: serde_json::from_value(serde_json::Value::String(outcome)).map_err(|_| invalid())?,
                    failure: failure.map(StoredFailure::from_storage),
                })
            }).collect::<Result<Vec<_>>>()?;
            Ok(Some(PublicationRecord {
                identity: PublicationIdentity { request_fingerprint, receipt: result },
                observations,
            }))
        })
    }
}

/// The shared encoder is the only producer of wire envelopes. The DTO conversion
/// below does not grant projection authority or forward retained response text.
pub(crate) fn envelope<A: Audience, D: WireDetails>(
    projected: &error_contract::ProjectedFailure<A, D>,
) -> Result<FailureEnvelope<D>> {
    let encoded =
        encode_projected_failure(projected, Budget::new(32768, 32768)).map_err(|_| invalid())?;
    serde_json::from_slice(encoded.as_bytes()).map_err(|_| invalid())
}
