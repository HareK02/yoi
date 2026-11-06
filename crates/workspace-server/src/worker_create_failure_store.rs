//! Backend coordinator returned startup results belong to the existing create
//! reservation. Runtime primary failure remains in Session metadata. Saving or
//! reading this evidence neither transitions the reservation nor grants retry.

use runtime_api::startup::{DecodeStoredWorkerCreateFailure, decode_stored_worker_create_failure};
use rusqlite::{OptionalExtension, params};
use worker_runtime::identity::WorkerId;

use crate::store::{SqliteWorkspaceStore, WORKER_CREATE_FAILURE_MAX_BYTES};
use crate::{Error, Result};

impl SqliteWorkspaceStore {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn save_worker_create_failure(
        &self,
        workspace_id: &str,
        runtime_id: &str,
        worker_id: WorkerId,
        expected_create_fingerprint: &str,
        expected_previous: Option<&str>,
        encoded: &str,
    ) -> Result<bool> {
        // Bound the wire representation before the typed decoder allocates JSON.
        if encoded.len() > WORKER_CREATE_FAILURE_MAX_BYTES
            || expected_previous.is_some_and(|value| value.len() > WORKER_CREATE_FAILURE_MAX_BYTES)
        {
            return Err(Error::InvalidInput(
                "Worker create failure exceeds storage byte bound".into(),
            ));
        }
        if !matches!(
            decode_stored_worker_create_failure(encoded),
            DecodeStoredWorkerCreateFailure::Known(_)
        ) {
            return Err(Error::InvalidInput(
                "Worker create failure is not a known storage representation".into(),
            ));
        }
        self.with_immediate_transaction(|tx| {
            // Identity, immutable create intent and exact prior bytes are checked
            // in the same UPDATE. Removed reservations deliberately remain eligible.
            let changed = tx.execute(
                "UPDATE worker_create_reservations SET startup_failure_json=?6
                 WHERE workspace_id=?1 AND runtime_id=?2 AND worker_id=?3
                   AND create_fingerprint=?4 AND startup_failure_json IS ?5",
                params![
                    workspace_id,
                    runtime_id,
                    worker_id.to_string(),
                    expected_create_fingerprint,
                    expected_previous,
                    encoded
                ],
            )?;
            Ok(changed == 1)
        })
    }

    pub(crate) fn worker_create_failure(
        &self,
        workspace_id: &str,
        runtime_id: &str,
        worker_id: WorkerId,
    ) -> Result<Option<String>> {
        self.with_conn(|conn| {
            // Return raw bounded evidence, including unknown/corrupt versions;
            // classification belongs to the typed reader, not legacy prose.
            let saved = conn
                .query_row(
                    "SELECT CASE WHEN length(CAST(startup_failure_json AS BLOB)) <= ?4
                             THEN startup_failure_json ELSE NULL END,
                        length(CAST(startup_failure_json AS BLOB))
                 FROM worker_create_reservations
                 WHERE workspace_id=?1 AND runtime_id=?2 AND worker_id=?3",
                    params![
                        workspace_id,
                        runtime_id,
                        worker_id.to_string(),
                        WORKER_CREATE_FAILURE_MAX_BYTES as i64
                    ],
                    |row| {
                        Ok((
                            row.get::<_, Option<String>>(0)?,
                            row.get::<_, Option<i64>>(1)?,
                        ))
                    },
                )
                .optional()?;
            match saved {
                Some((_, Some(bytes))) if bytes > WORKER_CREATE_FAILURE_MAX_BYTES as i64 => Err(
                    Error::Store("Stored Worker create failure exceeds byte bound".into()),
                ),
                Some((value, _)) => Ok(value),
                None => Ok(None),
            }
        })
    }
}
