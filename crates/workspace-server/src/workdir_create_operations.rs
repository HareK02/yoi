use rusqlite::{OptionalExtension, TransactionBehavior, params};
use sha2::{Digest, Sha256};

use crate::store::WorkdirCreateOperationRecord;
use crate::{Error, Result, SqliteWorkspaceStore};

pub fn selector_for_retry(
    explicit_selector: Option<&str>,
    persisted_selector: Option<&str>,
    current_default_selector: Option<&str>,
) -> Option<String> {
    explicit_selector
        .or(persisted_selector)
        .or(current_default_selector)
        .map(str::to_string)
}

pub fn request_fingerprint(
    repository_id: &str,
    selector: Option<&str>,
    requested_runtime_id: Option<&str>,
    repository_source_fingerprint: &str,
    repository_source_revision: u64,
) -> String {
    let mut hasher = Sha256::new();
    for value in [
        Some(repository_id),
        selector,
        requested_runtime_id,
        Some(repository_source_fingerprint),
    ] {
        match value {
            Some(value) => {
                hasher.update([1]);
                hasher.update((value.len() as u64).to_be_bytes());
                hasher.update(value.as_bytes());
            }
            None => hasher.update([0]),
        }
    }
    hasher.update(repository_source_revision.to_be_bytes());
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
                    config_projection_digest, source_kind, source_uri, source_revision,
                    source_fingerprint, working_directory_id, state, failure,
                    created_at, updated_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)"#,
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
                    record.source_kind,
                    record.source_uri,
                    record.source_revision.map(|revision| revision as i64),
                    record.source_fingerprint,
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

    pub fn begin_failed_workdir_create_retry(
        &self,
        workspace_id: &str,
        operation_id: &str,
        request_fingerprint: &str,
        updated_at: &str,
    ) -> Result<WorkdirCreateOperationRecord> {
        self.with_conn_mut(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let operation = read_workdir_create_operation(&tx, workspace_id, operation_id)?
                .ok_or_else(|| {
                    Error::RegistryInconsistency(format!(
                        "Workdir create operation `{operation_id}` disappeared before retry"
                    ))
                })?;
            if operation.request_fingerprint != request_fingerprint {
                return Err(Error::InvalidInput(format!(
                    "Workdir create operation `{operation_id}` was reused with different input"
                )));
            }
            if operation.state != "failed" {
                return Err(Error::WorkdirAttachmentConflict(format!(
                    "Workdir create operation `{operation_id}` is not a failed retry"
                )));
            }
            let removal_pending: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM workdir_removal_operations WHERE workspace_id=?1 AND workdir_id=?2 AND state='pending')",
                params![workspace_id, operation.working_directory_id],
                |row| row.get(0),
            )?;
            if removal_pending {
                return Err(Error::WorkdirAttachmentConflict(format!(
                    "Workdir {} has a pending durable removal operation",
                    operation.working_directory_id
                )));
            }
            let changed = tx.execute(
                r#"UPDATE workdir_create_operations
                   SET state='pending', failure=NULL, updated_at=?1
                   WHERE workspace_id=?2 AND operation_id=?3
                     AND request_fingerprint=?4 AND state='failed'"#,
                params![updated_at, workspace_id, operation_id, request_fingerprint],
            )?;
            if changed != 1 {
                return Err(Error::WorkdirAttachmentConflict(format!(
                    "Workdir create operation `{operation_id}` retry was claimed concurrently"
                )));
            }
            let updated = read_workdir_create_operation(&tx, workspace_id, operation_id)?
                .ok_or_else(|| {
                    Error::RegistryInconsistency(format!(
                        "Workdir create operation `{operation_id}` disappeared after retry claim"
                    ))
                })?;
            tx.commit()?;
            Ok(updated)
        })
    }

    pub fn bind_workdir_create_repository_access(
        &self,
        workspace_id: &str,
        operation_id: &str,
        request_fingerprint: &str,
        credential_id: &str,
        credential_revision: u64,
        host_trust_id: &str,
        host_trust_revision: u64,
        repository_access_mode: &str,
        cache_generation: u64,
        now: &str,
    ) -> Result<WorkdirCreateOperationRecord> {
        self.with_conn_mut(|conn| {
            let operation = read_workdir_create_operation(conn, workspace_id, operation_id)?
                .ok_or_else(|| {
                    Error::RegistryInconsistency(format!(
                        "Workdir create operation `{operation_id}` disappeared before Repository access binding"
                    ))
                })?;
            if operation.request_fingerprint != request_fingerprint {
                return Err(Error::InvalidInput(format!(
                    "Workdir create operation `{operation_id}` was reused with different input"
                )));
            }
            if let Some(existing) = operation.credential_id.as_deref() {
                if existing != credential_id
                    || operation.credential_revision != Some(credential_revision)
                    || operation.host_trust_id.as_deref() != Some(host_trust_id)
                    || operation.host_trust_revision != Some(host_trust_revision)
                    || operation.repository_access_mode.as_deref()
                        != Some(repository_access_mode)
                    || operation.cache_generation != cache_generation
                {
                    return Err(Error::InvalidInput(format!(
                        "Workdir create operation `{operation_id}` Repository access evidence changed"
                    )));
                }
                return Ok(operation);
            }
            conn.execute(
                r#"UPDATE workdir_create_operations
                   SET credential_id = ?4, credential_revision = ?5,
                       host_trust_id = ?6, host_trust_revision = ?7,
                       repository_access_mode = ?8, cache_generation = ?9,
                       updated_at = ?10
                   WHERE workspace_id = ?1 AND operation_id = ?2
                     AND request_fingerprint = ?3 AND credential_id IS NULL"#,
                params![
                    workspace_id,
                    operation_id,
                    request_fingerprint,
                    credential_id,
                    i64::try_from(credential_revision).map_err(|_| Error::InvalidInput(
                        "credential revision is out of range".to_string()
                    ))?,
                    host_trust_id,
                    i64::try_from(host_trust_revision).map_err(|_| Error::InvalidInput(
                        "host-trust revision is out of range".to_string()
                    ))?,
                    repository_access_mode,
                    i64::try_from(cache_generation).map_err(|_| Error::InvalidInput(
                        "cache generation is out of range".to_string()
                    ))?,
                    now,
                ],
            )?;
            read_workdir_create_operation(conn, workspace_id, operation_id)?.ok_or_else(|| {
                Error::RegistryInconsistency(format!(
                    "Workdir create operation `{operation_id}` disappeared after Repository access binding"
                ))
            })
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
                  config_projection_digest, source_kind, source_uri, source_revision,
                  source_fingerprint, credential_id, credential_revision,
                  host_trust_id, host_trust_revision, repository_access_mode,
                  cache_generation, working_directory_id, state, failure,
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
                source_kind: row.get(9)?,
                source_uri: row.get(10)?,
                source_revision: row.get::<_, Option<i64>>(11)?.map(|value| value as u64),
                source_fingerprint: row.get(12)?,
                credential_id: row.get(13)?,
                credential_revision: row.get::<_, Option<i64>>(14)?.map(|value| value as u64),
                host_trust_id: row.get(15)?,
                host_trust_revision: row.get::<_, Option<i64>>(16)?.map(|value| value as u64),
                repository_access_mode: row.get(17)?,
                cache_generation: row.get::<_, i64>(18)? as u64,
                working_directory_id: row.get(19)?,
                state: row.get(20)?,
                failure: row.get(21)?,
                created_at: row.get(22)?,
                updated_at: row.get(23)?,
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
    fn retry_selector_keeps_persisted_default_but_honors_explicit_input() {
        assert_eq!(
            selector_for_retry(None, Some("develop"), Some("main")),
            Some("develop".to_string())
        );
        assert_eq!(
            selector_for_retry(Some("release"), Some("develop"), Some("main")),
            Some("release".to_string())
        );
        assert_eq!(
            selector_for_retry(None, None, Some("main")),
            Some("main".to_string())
        );
    }

    #[test]
    fn retry_keeps_resolved_config_evidence_and_rejects_changed_input() {
        let store = SqliteWorkspaceStore::in_memory().unwrap();
        futures::executor::block_on(store.upsert_workspace(&WorkspaceRecord {
            workspace_id: "workspace".to_string(),
            owner_account_id: "owner-account".to_string(),
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
                repository_key: "main".to_string(),
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
            request_fingerprint: request_fingerprint(
                "main",
                Some("develop"),
                None,
                "sha256:test",
                1,
            ),
            repository_id: "main".to_string(),
            selector: Some("develop".to_string()),
            requested_runtime_id: None,
            resolved_runtime_id: "arcadia".to_string(),
            config_revision: 7,
            config_projection_digest: "sha256:projection".to_string(),
            source_kind: Some("local_path".to_string()),
            source_uri: Some("/tmp/repo".to_string()),
            source_revision: Some(1),
            source_fingerprint: Some("sha256:source".to_string()),
            credential_id: None,
            credential_revision: None,
            host_trust_id: None,
            host_trust_revision: None,
            repository_access_mode: None,
            cache_generation: 0,
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
        let bound = store
            .bind_workdir_create_repository_access(
                "workspace",
                "call-1",
                &record.request_fingerprint,
                "credential-1",
                3,
                "trust-1",
                5,
                "read_only",
                2,
                "2026-08-24T00:00:01Z",
            )
            .unwrap();
        assert_eq!(bound.credential_id.as_deref(), Some("credential-1"));
        assert_eq!(bound.credential_revision, Some(3));
        assert_eq!(bound.host_trust_revision, Some(5));
        assert_eq!(bound.cache_generation, 2);
        assert!(
            store
                .bind_workdir_create_repository_access(
                    "workspace",
                    "call-1",
                    &record.request_fingerprint,
                    "credential-1",
                    4,
                    "trust-1",
                    5,
                    "read_only",
                    2,
                    "2026-08-24T00:00:02Z",
                )
                .is_err()
        );
        let mut changed_resolution = record.clone();
        changed_resolution.resolved_runtime_id = "other".to_string();
        changed_resolution.config_revision = 8;
        changed_resolution.source_uri = Some("ssh://git@other.test/repo.git".to_string());
        changed_resolution.source_revision = Some(9);
        let replayed = store
            .reserve_workdir_create_operation(&changed_resolution)
            .unwrap();
        assert_eq!(replayed, bound);
        assert_eq!(replayed.source_uri.as_deref(), Some("/tmp/repo"));
        let failed = store
            .finish_workdir_create_operation(
                "workspace",
                "call-1",
                &record.request_fingerprint,
                false,
                Some("provider failed"),
                "2026-08-24T00:00:03Z",
            )
            .unwrap();
        assert_eq!(failed.state, "failed");
        let retry = store
            .begin_failed_workdir_create_retry(
                "workspace",
                "call-1",
                &record.request_fingerprint,
                "2026-08-24T00:00:04Z",
            )
            .unwrap();
        assert_eq!(retry.state, "pending");
        assert_eq!(retry.failure, None);
        assert_eq!(
            store
                .load_workdir_create_operation("workspace", "call-1")
                .unwrap(),
            Some(retry.clone())
        );
        let mut changed_input = record.clone();
        changed_input.request_fingerprint =
            request_fingerprint("main", Some("main"), None, "sha256:test", 1);
        assert!(
            store
                .reserve_workdir_create_operation(&changed_input)
                .unwrap_err()
                .to_string()
                .contains("reused with different input")
        );
    }
}
