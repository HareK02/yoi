use rusqlite::{OptionalExtension, params};
use sha2::{Digest, Sha256};

use crate::store::WorkdirCreateOperationRecord;
use crate::{Error, Result, SqliteWorkspaceStore};

pub fn request_fingerprint(
    repository_id: &str,
    selector: Option<&str>,
    requested_runtime_id: Option<&str>,
) -> String {
    let mut hasher = Sha256::new();
    for value in [Some(repository_id), selector, requested_runtime_id] {
        match value {
            Some(value) => {
                hasher.update([1]);
                hasher.update((value.len() as u64).to_be_bytes());
                hasher.update(value.as_bytes());
            }
            None => hasher.update([0]),
        }
    }
    let digest = hasher.finalize();
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        write!(&mut encoded, "{byte:02x}").expect("writing to String cannot fail");
    }
    format!("sha256:{encoded}")
}

impl SqliteWorkspaceStore {
    pub fn reserve_workdir_create_operation(
        &self,
        record: &WorkdirCreateOperationRecord,
    ) -> Result<WorkdirCreateOperationRecord> {
        self.with_conn_mut(|conn| {
            let tx = conn.transaction()?;
            tx.execute(
                r#"INSERT OR IGNORE INTO workdir_create_operations (
                    workspace_id, operation_id, request_fingerprint, repository_id, selector,
                    requested_runtime_id, resolved_runtime_id, config_revision,
                    config_projection_digest, working_directory_id, state, failure,
                    created_at, updated_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)"#,
                params![
                    record.workspace_id,
                    record.operation_id,
                    record.request_fingerprint,
                    record.repository_id,
                    record.selector,
                    record.requested_runtime_id,
                    record.resolved_runtime_id,
                    record.config_revision as i64,
                    record.config_projection_digest,
                    record.working_directory_id,
                    record.state,
                    record.failure,
                    record.created_at,
                    record.updated_at,
                ],
            )?;
            let persisted =
                read_workdir_create_operation(&tx, &record.workspace_id, &record.operation_id)?
                    .ok_or_else(|| {
                        Error::RegistryInconsistency(format!(
                            "Workdir create operation `{}` was not persisted",
                            record.operation_id
                        ))
                    })?;
            if persisted.request_fingerprint != record.request_fingerprint {
                return Err(Error::InvalidInput(format!(
                    "Workdir create operation `{}` was reused with different input",
                    record.operation_id
                )));
            }
            tx.commit()?;
            Ok(persisted)
        })
    }

    pub fn finish_workdir_create_operation(
        &self,
        workspace_id: &str,
        operation_id: &str,
        request_fingerprint: &str,
        succeeded: bool,
        failure: Option<&str>,
        updated_at: &str,
    ) -> Result<WorkdirCreateOperationRecord> {
        self.with_conn_mut(|conn| {
            let changed = conn.execute(
                r#"UPDATE workdir_create_operations
                   SET state = ?1, failure = ?2, updated_at = ?3
                   WHERE workspace_id = ?4 AND operation_id = ?5
                     AND request_fingerprint = ?6"#,
                params![
                    if succeeded { "succeeded" } else { "failed" },
                    failure,
                    updated_at,
                    workspace_id,
                    operation_id,
                    request_fingerprint,
                ],
            )?;
            if changed != 1 {
                return Err(Error::RegistryInconsistency(format!(
                    "Workdir create operation `{operation_id}` could not be finalized"
                )));
            }
            read_workdir_create_operation(conn, workspace_id, operation_id)?.ok_or_else(|| {
                Error::RegistryInconsistency(format!(
                    "Workdir create operation `{operation_id}` disappeared"
                ))
            })
        })
    }

    pub fn load_workdir_create_operation(
        &self,
        workspace_id: &str,
        operation_id: &str,
    ) -> Result<Option<WorkdirCreateOperationRecord>> {
        self.with_conn(|conn| read_workdir_create_operation(conn, workspace_id, operation_id))
    }
}

fn read_workdir_create_operation(
    conn: &rusqlite::Connection,
    workspace_id: &str,
    operation_id: &str,
) -> Result<Option<WorkdirCreateOperationRecord>> {
    conn.query_row(
        r#"SELECT workspace_id, operation_id, request_fingerprint, repository_id, selector,
                  requested_runtime_id, resolved_runtime_id, config_revision,
                  config_projection_digest, working_directory_id, state, failure,
                  created_at, updated_at
           FROM workdir_create_operations
           WHERE workspace_id = ?1 AND operation_id = ?2"#,
        params![workspace_id, operation_id],
        |row| {
            Ok(WorkdirCreateOperationRecord {
                workspace_id: row.get(0)?,
                operation_id: row.get(1)?,
                request_fingerprint: row.get(2)?,
                repository_id: row.get(3)?,
                selector: row.get(4)?,
                requested_runtime_id: row.get(5)?,
                resolved_runtime_id: row.get(6)?,
                config_revision: row.get::<_, i64>(7)? as u64,
                config_projection_digest: row.get(8)?,
                working_directory_id: row.get(9)?,
                state: row.get(10)?,
                failure: row.get(11)?,
                created_at: row.get(12)?,
                updated_at: row.get(13)?,
            })
        },
    )
    .optional()
    .map_err(Error::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{ControlPlaneStore, RepositoryRecord, WorkspaceRecord};

    #[test]
    fn retry_keeps_resolved_config_evidence_and_rejects_changed_input() {
        let store = SqliteWorkspaceStore::in_memory().unwrap();
        futures::executor::block_on(store.upsert_workspace(&WorkspaceRecord {
            workspace_id: "workspace".to_string(),
            owner_account_id: None,
            display_name: "Workspace".to_string(),
            state: "active".to_string(),
            created_at: "2026-08-24T00:00:00Z".to_string(),
            updated_at: "2026-08-24T00:00:00Z".to_string(),
        }))
        .unwrap();
        store
            .upsert_repository(&RepositoryRecord {
                workspace_id: "workspace".to_string(),
                repository_id: "main".to_string(),
                name: "main".to_string(),
                kind: "git".to_string(),
                provider: Some("git".to_string()),
                source: workspace_api::RepositorySource {
                    kind: workspace_api::RepositorySourceKind::LocalPath,
                    uri: "/tmp/main".to_string(),
                },
                default_ref: Some("develop".to_string()),
                source_revision: 1,
                source_fingerprint: "sha256:test".to_string(),
                observed_status: workspace_api::RepositoryObservedStatus::Unverified,
                observed_at: None,
                created_at: "2026-08-24T00:00:00Z".to_string(),
                updated_at: "2026-08-24T00:00:00Z".to_string(),
            })
            .unwrap();
        let record = WorkdirCreateOperationRecord {
            workspace_id: "workspace".to_string(),
            operation_id: "call-1".to_string(),
            request_fingerprint: request_fingerprint("main", Some("develop"), None),
            repository_id: "main".to_string(),
            selector: Some("develop".to_string()),
            requested_runtime_id: None,
            resolved_runtime_id: "arcadia".to_string(),
            config_revision: 7,
            config_projection_digest: "sha256:projection".to_string(),
            working_directory_id: "wd-1".to_string(),
            state: "pending".to_string(),
            failure: None,
            created_at: "2026-08-24T00:00:00Z".to_string(),
            updated_at: "2026-08-24T00:00:00Z".to_string(),
        };
        assert_eq!(
            store.reserve_workdir_create_operation(&record).unwrap(),
            record
        );
        let mut changed_resolution = record.clone();
        changed_resolution.resolved_runtime_id = "other".to_string();
        changed_resolution.config_revision = 8;
        assert_eq!(
            store
                .reserve_workdir_create_operation(&changed_resolution)
                .unwrap(),
            record
        );
        assert_eq!(
            store
                .load_workdir_create_operation("workspace", "call-1")
                .unwrap(),
            Some(record.clone())
        );
        let mut changed_input = record.clone();
        changed_input.request_fingerprint = request_fingerprint("main", Some("main"), None);
        assert!(
            store
                .reserve_workdir_create_operation(&changed_input)
                .unwrap_err()
                .to_string()
                .contains("reused with different input")
        );
    }
}
