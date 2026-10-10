use rusqlite::{OptionalExtension, TransactionBehavior, params};
use sha2::{Digest, Sha256};

use crate::store::{
    WorkdirCreateCredentialCandidate, WorkdirCreateCredentialCandidateRole,
    WorkdirCreateOperationRecord,
};
use crate::{Error, Result, SqliteWorkspaceStore};

const MAX_WORKDIR_CREATE_CREDENTIAL_CANDIDATES: usize = 2;
const MAX_CREDENTIAL_ID_BYTES: usize = 128;

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
    display_name: Option<&str>,
    repository_source_fingerprint: &str,
) -> String {
    encode_request_digest(request_hasher(
        repository_id,
        selector,
        requested_runtime_id,
        display_name,
        repository_source_fingerprint,
    ))
}

fn request_hasher(
    repository_id: &str,
    selector: Option<&str>,
    requested_runtime_id: Option<&str>,
    display_name: Option<&str>,
    repository_source_fingerprint: &str,
) -> Sha256 {
    let mut hasher = Sha256::new();
    for value in [
        Some(repository_id),
        selector,
        requested_runtime_id,
        display_name,
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
    hasher
}

fn encode_request_digest(hasher: Sha256) -> String {
    let digest = hasher.finalize();
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        write!(&mut encoded, "{byte:02x}").expect("writing to String cannot fail");
    }
    format!("sha256:{encoded}")
}

/// Resolve replay identity without rewriting the original durable request. Schema 86
/// did not persist display_name, so its digest cannot be converted at migration time.
/// The frozen ordinal is hash evidence only, never a live Repository precondition.
pub fn request_fingerprint_for_replay(
    repository_id: &str,
    selector: Option<&str>,
    requested_runtime_id: Option<&str>,
    display_name: Option<&str>,
    repository_source_fingerprint: &str,
    persisted_fingerprint: Option<&str>,
) -> Result<String> {
    let current = request_fingerprint(
        repository_id,
        selector,
        requested_runtime_id,
        display_name,
        repository_source_fingerprint,
    );
    let Some(persisted) = persisted_fingerprint else {
        return Ok(current);
    };
    if persisted == current {
        return Ok(current);
    }
    if let Some(legacy) = persisted.strip_prefix("workdir-create-v86:") {
        if let Some((legacy_counter, digest)) = legacy.split_once(':') {
            if let Ok(legacy_counter) = legacy_counter.parse::<u64>() {
                let mut hasher = request_hasher(
                    repository_id,
                    selector,
                    requested_runtime_id,
                    display_name,
                    repository_source_fingerprint,
                );
                hasher.update(legacy_counter.to_be_bytes());
                if encode_request_digest(hasher) == digest {
                    return Ok(persisted.to_string());
                }
            }
        }
    }
    Err(Error::InvalidInput(
        "Workdir create operation was reused with different input or lacks frozen request identity evidence".into(),
    ))
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
                    requested_runtime_id, resolved_runtime_id,
                    config_projection_digest, source_kind, source_uri,
                    source_fingerprint, working_directory_id, state, failure,
                    created_at, updated_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)"#,
                params![
                    record.workspace_id,
                    record.operation_id,
                    record.request_fingerprint,
                    record.repository_id,
                    record.selector,
                    record.requested_runtime_id,
                    record.resolved_runtime_id,
                    record.config_projection_digest,
                    record.source_kind,
                    record.source_uri,
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
            if operation.state != "failed" || workdir_create_has_archived_ssh(&tx, workspace_id, operation_id)? {
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
        credential_fingerprint: &str,
        host_trust_id: &str,
        host_trust_fingerprint: &str,
        repository_access_mode: &str,
        credential_candidates: &[WorkdirCreateCredentialCandidate],
        now: &str,
    ) -> Result<WorkdirCreateOperationRecord> {
        validate_content_fingerprint(host_trust_fingerprint)?;
        validate_workdir_create_credential_candidates(
            credential_id,
            credential_fingerprint,
            credential_candidates,
        )?;
        self.with_conn_mut(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let operation = read_workdir_create_operation(&tx, workspace_id, operation_id)?
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
            if operation.state != "pending" || workdir_create_has_archived_ssh(&tx, workspace_id, operation_id)? {
                return Err(Error::WorkdirAttachmentConflict("Only a pending, nonarchived Workdir create can bind Repository access".into()));
            }
            if let Some(existing) = operation.credential_id.as_deref() {
                if existing != credential_id
                    || operation.credential_fingerprint.as_deref() != Some(credential_fingerprint)
                    || operation.host_trust_id.as_deref() != Some(host_trust_id)
                    || operation.host_trust_fingerprint.as_deref() != Some(host_trust_fingerprint)
                    || operation.repository_access_mode.as_deref()
                        != Some(repository_access_mode)
                    || operation.credential_candidates != credential_candidates
                {
                    return Err(Error::InvalidInput(format!(
                        "Workdir create operation `{operation_id}` Repository access evidence changed"
                    )));
                }
                return Ok(operation);
            }
            let updated = tx.execute(
                r#"UPDATE workdir_create_operations
                   SET credential_id = ?4, credential_fingerprint = ?5,
                       host_trust_id = ?6, host_trust_fingerprint = ?7,
                       repository_access_mode = ?8, updated_at = ?9
                   WHERE workspace_id = ?1 AND operation_id = ?2
                     AND request_fingerprint = ?3 AND credential_id IS NULL"#,
                params![
                    workspace_id,
                    operation_id,
                    request_fingerprint,
                    credential_id,
                    credential_fingerprint,
                    host_trust_id,
                    host_trust_fingerprint,
                    repository_access_mode,
                    now,
                ],
            )?;
            if updated != 1 {
                return Err(Error::RegistryInconsistency(format!(
                    "Workdir create operation `{operation_id}` changed before Repository access binding"
                )));
            }
            for (ordinal, candidate) in credential_candidates.iter().enumerate() {
                tx.execute(
                    r#"INSERT INTO workdir_create_credential_candidates (
                           workspace_id, operation_id, ordinal, role,
                           credential_id, credential_fingerprint
                       ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)"#,
                    params![
                        workspace_id,
                        operation_id,
                        i64::try_from(ordinal).map_err(|_| Error::InvalidInput(
                            "credential candidate ordinal is out of range".to_string()
                        ))?,
                        candidate.role.as_str(),
                        candidate.credential_id,
                        candidate.credential_fingerprint,
                    ],
                )?;
                let retained = tx.execute(
                    r#"INSERT INTO workdir_create_credential_retentions (
                           workspace_id, operation_id, ordinal,
                           credential_id, credential_fingerprint, credential_operation_id
                       ) SELECT ?1, ?2, ?3, ?4, ?5, k.operation_id
                         FROM repository_ssh_credential_keys k
                         JOIN repository_ssh_credentials c
                           ON c.workspace_id=k.workspace_id AND c.credential_id=k.credential_id
                         WHERE k.workspace_id=?1 AND k.credential_id=?4 AND k.public_key_fingerprint=?5
                           AND c.status='active'
                         ORDER BY (k.operation_id=c.current_operation_id) DESC,
                                  k.created_at DESC, k.operation_id DESC LIMIT 1"#,
                    params![
                        workspace_id,
                        operation_id,
                        i64::try_from(ordinal).map_err(|_| Error::InvalidInput(
                            "credential candidate ordinal is out of range".to_string()
                        ))?,
                        candidate.credential_id,
                        candidate.credential_fingerprint,
                    ],
                )?;

                if retained != 1 { return Err(Error::RegistryInconsistency("Workdir create credential fingerprint has no stored key".into())); }            }
            let bound = read_workdir_create_operation(&tx, workspace_id, operation_id)?
                .ok_or_else(|| {
                    Error::RegistryInconsistency(format!(
                        "Workdir create operation `{operation_id}` disappeared after Repository access binding"
                    ))
                })?;
            tx.commit()?;
            Ok(bound)
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
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let operation = read_workdir_create_operation(&tx, workspace_id, operation_id)?
                .ok_or_else(|| {
                    Error::RegistryInconsistency(
                        "Workdir create disappeared before finalization".into(),
                    )
                })?;
            let target_state = if succeeded { "succeeded" } else { "failed" };
            if operation.request_fingerprint != request_fingerprint {
                return Err(Error::InvalidInput(
                    "Workdir create operation was reused with different input".into(),
                ));
            }
            if operation.state != "pending" {
                if operation.state == target_state && operation.failure.as_deref() == failure {
                    return Ok(operation);
                }
                return Err(Error::WorkdirAttachmentConflict(
                    "A completed Workdir create cannot be finalized into another state".into(),
                ));
            }
            let changed = tx.execute(
                r#"UPDATE workdir_create_operations
                   SET state = ?1, failure = ?2, updated_at = ?3
                   WHERE workspace_id = ?4 AND operation_id = ?5
                     AND request_fingerprint = ?6 AND state='pending'"#,
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
            if succeeded {
                tx.execute(
                    r#"DELETE FROM workdir_create_credential_retentions
                       WHERE workspace_id = ?1 AND operation_id = ?2"#,
                    params![workspace_id, operation_id],
                )?;
            }
            let finished = read_workdir_create_operation(&tx, workspace_id, operation_id)?
                .ok_or_else(|| {
                    Error::RegistryInconsistency(format!(
                        "Workdir create operation `{operation_id}` disappeared"
                    ))
                })?;
            tx.commit()?;
            Ok(finished)
        })
    }

    /// Frozen, nonauthorizing historical data. Never use this payload for a retry or lease.
    pub fn load_workdir_create_legacy_ssh_archive(
        &self,
        workspace_id: &str,
        operation_id: &str,
    ) -> Result<Option<(serde_json::Value, serde_json::Value)>> {
        self.with_conn(|conn| {
            let archive: Option<(String, String)> = conn.query_row(
                "SELECT operation_json,candidates_json FROM workdir_create_legacy_ssh_archives WHERE workspace_id=?1 AND operation_id=?2",
                params![workspace_id, operation_id], |r| Ok((r.get(0)?, r.get(1)?)),
            ).optional()?;
            archive.map(|(operation, candidates)| Ok((
                serde_json::from_str(&operation).map_err(|e| Error::Store(e.to_string()))?,
                serde_json::from_str(&candidates).map_err(|e| Error::Store(e.to_string()))?,
            ))).transpose()
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

fn workdir_create_has_archived_ssh(
    conn: &rusqlite::Connection,
    workspace_id: &str,
    operation_id: &str,
) -> Result<bool> {
    Ok(conn.query_row("SELECT EXISTS(SELECT 1 FROM workdir_create_legacy_ssh_archives WHERE workspace_id=?1 AND operation_id=?2)", params![workspace_id,operation_id], |r| r.get(0))?)
}

fn read_workdir_create_operation(
    conn: &rusqlite::Connection,
    workspace_id: &str,
    operation_id: &str,
) -> Result<Option<WorkdirCreateOperationRecord>> {
    let mut operation = conn
        .query_row(
            r#"SELECT workspace_id, operation_id, request_fingerprint, repository_id, selector,
                  requested_runtime_id, resolved_runtime_id,
                  config_projection_digest, source_kind, source_uri,
                  source_fingerprint, credential_id, credential_fingerprint,
                  host_trust_id, host_trust_fingerprint, repository_access_mode,
                  working_directory_id, state, failure,
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
                    config_projection_digest: row.get(7)?,
                    source_kind: row.get(8)?,
                    source_uri: row.get(9)?,
                    source_fingerprint: row.get(10)?,
                    credential_id: row.get(11)?,
                    credential_fingerprint: row.get(12)?,
                    host_trust_id: row.get(13)?,
                    host_trust_fingerprint: row.get(14)?,
                    repository_access_mode: row.get(15)?,
                    credential_candidates: Vec::new(),
                    working_directory_id: row.get(16)?,
                    state: row.get(17)?,
                    failure: row.get(18)?,
                    created_at: row.get(19)?,
                    updated_at: row.get(20)?,
                })
            },
        )
        .optional()?;

    if let Some(operation) = operation.as_mut() {
        let mut statement = conn.prepare(
            r#"SELECT ordinal, role, credential_id, credential_fingerprint
               FROM workdir_create_credential_candidates
               WHERE workspace_id = ?1 AND operation_id = ?2
               ORDER BY ordinal ASC"#,
        )?;
        let rows = statement.query_map(params![workspace_id, operation_id], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;
        for (expected_ordinal, row) in rows.enumerate() {
            let (ordinal, role, credential_id, credential_fingerprint) = row?;
            if ordinal
                != i64::try_from(expected_ordinal).map_err(|_| {
                    Error::Store(
                        "Workdir create credential candidate ordinal is out of range".to_string(),
                    )
                })?
            {
                return Err(Error::Store(
                    "Workdir create credential candidate ordinals are not contiguous".to_string(),
                ));
            }
            operation
                .credential_candidates
                .push(WorkdirCreateCredentialCandidate {
                    role: WorkdirCreateCredentialCandidateRole::parse(&role)?,
                    credential_id,
                    credential_fingerprint,
                });
        }
        if !operation.credential_candidates.is_empty() {
            validate_workdir_create_credential_candidates(
                operation.credential_id.as_deref().unwrap_or_default(),
                operation
                    .credential_fingerprint
                    .as_deref()
                    .unwrap_or_default(),
                &operation.credential_candidates,
            )
            .map_err(|error| {
                Error::Store(format!(
                    "invalid persisted Workdir create credential snapshot: {error}"
                ))
            })?;
        }
    }
    Ok(operation)
}

fn validate_content_fingerprint(fingerprint: &str) -> Result<()> {
    if fingerprint.trim().is_empty()
        || fingerprint.len() > 256
        || fingerprint.chars().any(char::is_control)
    {
        return Err(Error::InvalidInput(
            "Workdir create content fingerprint is invalid".to_string(),
        ));
    }
    Ok(())
}

pub(crate) fn validate_workdir_create_credential_candidates(
    credential_id: &str,
    credential_fingerprint: &str,
    candidates: &[WorkdirCreateCredentialCandidate],
) -> Result<()> {
    if candidates.is_empty() || candidates.len() > MAX_WORKDIR_CREATE_CREDENTIAL_CANDIDATES {
        return Err(Error::InvalidInput(format!(
            "Workdir create credential candidates must contain 1..={MAX_WORKDIR_CREATE_CREDENTIAL_CANDIDATES} entries"
        )));
    }
    if credential_id.is_empty() || credential_id.len() > MAX_CREDENTIAL_ID_BYTES {
        return Err(Error::InvalidInput(
            "Workdir create credential id is invalid".to_string(),
        ));
    }
    validate_content_fingerprint(credential_fingerprint)?;
    let primary = &candidates[0];
    if primary.role != WorkdirCreateCredentialCandidateRole::Primary
        || primary.credential_id != credential_id
        || primary.credential_fingerprint != credential_fingerprint
    {
        return Err(Error::InvalidInput(
            "Workdir create primary credential evidence does not match the ordered candidate snapshot"
                .to_string(),
        ));
    }
    if candidates.len() == 2
        && candidates[1].role != WorkdirCreateCredentialCandidateRole::WorkspaceDefaultFallback
    {
        return Err(Error::InvalidInput(
            "Workdir create fallback credential role is invalid".to_string(),
        ));
    }
    for candidate in candidates {
        validate_content_fingerprint(&candidate.credential_fingerprint)?;
    }
    if candidates.iter().any(|candidate| {
        candidate.credential_id.is_empty()
            || candidate.credential_id.len() > MAX_CREDENTIAL_ID_BYTES
            || candidate.credential_fingerprint.is_empty()
    }) {
        return Err(Error::InvalidInput(
            "Workdir create credential candidate identity is invalid".to_string(),
        ));
    }
    if candidates.len() == 2 && candidates[0].credential_id == candidates[1].credential_id {
        return Err(Error::InvalidInput(
            "Workdir create credential candidates contain a duplicate credential id".to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{ControlPlaneStore, RepositoryRecord, WorkspaceRecord};

    #[test]
    fn request_identity_depends_on_content_and_distinguishes_optional_values() {
        let original = request_fingerprint("main", None, None, None, "sha256:source-a");
        assert_eq!(
            original,
            request_fingerprint("main", None, None, None, "sha256:source-a")
        );
        assert_ne!(
            original,
            request_fingerprint("main", None, None, None, "sha256:source-b")
        );
        assert_ne!(
            original,
            request_fingerprint("main", Some(""), None, None, "sha256:source-a")
        );
        assert_ne!(
            request_fingerprint("a", Some("bc"), None, None, "source"),
            request_fingerprint("ab", Some("c"), None, None, "source")
        );
    }

    #[test]
    fn frozen_request_replay_checks_every_input_and_keeps_original_identity() {
        // Digest emitted by the schema-86 algorithm, including source ordinal 9.
        let legacy = "workdir-create-v86:9:sha256:db15419814117c33c089a36c5348bda85d01a53fccd8d4d427685daf5983353b";
        assert_eq!(
            request_fingerprint_for_replay(
                "repo",
                Some("main"),
                None,
                Some("Name"),
                "source",
                Some(legacy)
            )
            .unwrap(),
            legacy,
        );
        for (repo, selector, runtime, name, source) in [
            ("other", Some("main"), None, Some("Name"), "source"),
            ("repo", Some("develop"), None, Some("Name"), "source"),
            (
                "repo",
                Some("main"),
                Some("runtime"),
                Some("Name"),
                "source",
            ),
            ("repo", Some("main"), None, None, "source"),
            ("repo", Some("main"), None, Some("Other name"), "source"),
            ("repo", Some("main"), None, Some("Name"), "changed-source"),
        ] {
            assert!(
                request_fingerprint_for_replay(repo, selector, runtime, name, source, Some(legacy))
                    .is_err()
            );
        }
        for invalid in [
            "workdir-create-v86:unknown:sha256:db15419814117c33c089a36c5348bda85d01a53fccd8d4d427685daf5983353b",
            "workdir-create-v86:8:sha256:db15419814117c33c089a36c5348bda85d01a53fccd8d4d427685daf5983353b",
            "workdir-create-v86:9:sha256:bad",
        ] {
            assert!(
                request_fingerprint_for_replay(
                    "repo",
                    Some("main"),
                    None,
                    Some("Name"),
                    "source",
                    Some(invalid)
                )
                .is_err()
            );
        }
        let current = request_fingerprint("repo", Some("main"), None, Some("Name"), "source");
        assert_eq!(
            request_fingerprint_for_replay(
                "repo",
                Some("main"),
                None,
                Some("Name"),
                "source",
                Some(&current)
            )
            .unwrap(),
            current
        );
        assert_eq!(
            request_fingerprint_for_replay(
                "repo",
                Some("main"),
                None,
                Some("Name"),
                "source",
                None
            )
            .unwrap(),
            current
        );
    }

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
    fn retry_keeps_resolved_config_and_credential_candidates_after_fallback_moves() {
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
                source: server_api::RepositorySource {
                    kind: server_api::RepositorySourceKind::LocalPath,
                    uri: "/tmp/main".to_string(),
                },
                default_ref: Some("develop".to_string()),
                source_fingerprint: "sha256:test".to_string(),
                observed_status: server_api::RepositoryObservedStatus::Unverified,
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
                None,
                "sha256:test",
            ),
            repository_id: "main".to_string(),
            selector: Some("develop".to_string()),
            requested_runtime_id: None,
            resolved_runtime_id: "arcadia".to_string(),
            config_projection_digest: "sha256:projection".to_string(),
            source_kind: Some("local_path".to_string()),
            source_uri: Some("/tmp/repo".to_string()),
            source_fingerprint: Some("sha256:source".to_string()),
            credential_id: None,
            credential_fingerprint: None,
            host_trust_id: None,
            host_trust_fingerprint: None,
            repository_access_mode: None,
            credential_candidates: Vec::new(),
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
        store
            .with_conn_mut(|conn| {
                for (credential_id, fingerprint) in [
                    ("credential-1", "key-a"),
                    ("workspace-default-ssh", "key-fallback"),
                ] {
                    conn.execute(
                        r#"INSERT INTO repository_ssh_credentials (
                               workspace_id, credential_id, name,
                               public_key_algorithm, public_key_fingerprint,
                               current_operation_id, status, created_at
                           ) VALUES ('workspace', ?1, ?1, 'ssh-ed25519', ?2,
                                     ?1, 'active', '2026-08-24T00:00:00Z')"#,
                        params![credential_id, fingerprint],
                    )?;
                    conn.execute(
                        r#"INSERT INTO repository_ssh_credential_keys (
                               workspace_id, credential_id, operation_id,
                               public_key_algorithm, public_key_fingerprint, created_at
                           ) VALUES ('workspace', ?1, ?1, 'ssh-ed25519', ?2,
                                     '2026-08-24T00:00:00Z')"#,
                        params![credential_id, fingerprint],
                    )?;
                }
                Ok(())
            })
            .unwrap();
        let candidates = vec![
            WorkdirCreateCredentialCandidate {
                role: WorkdirCreateCredentialCandidateRole::Primary,
                credential_id: "credential-1".to_string(),
                credential_fingerprint: "key-a".to_string(),
            },
            WorkdirCreateCredentialCandidate {
                role: WorkdirCreateCredentialCandidateRole::WorkspaceDefaultFallback,
                credential_id: "workspace-default-ssh".to_string(),
                credential_fingerprint: "key-fallback".to_string(),
            },
        ];
        let bound = store
            .bind_workdir_create_repository_access(
                "workspace",
                "call-1",
                &record.request_fingerprint,
                "credential-1",
                "key-a",
                "trust-1",
                "trust-a",
                "read_only",
                &candidates,
                "2026-08-24T00:00:01Z",
            )
            .unwrap();
        assert_eq!(bound.credential_id.as_deref(), Some("credential-1"));
        assert_eq!(bound.credential_fingerprint.as_deref(), Some("key-a"));
        assert_eq!(bound.host_trust_fingerprint.as_deref(), Some("trust-a"));
        assert_eq!(bound.credential_candidates, candidates);
        let serialized = serde_json::to_string(&bound).unwrap();
        assert!(serialized.contains("workspace_default_fallback"));
        assert!(!serialized.contains("private_key"));
        assert!(!serialized.contains("known_hosts"));
        // A concurrent Workspace-default rotation must not replace the fallback
        // fingerprint already bound to this operation.
        let mut changed_candidates = candidates.clone();
        changed_candidates[1].credential_fingerprint = "key-rotated".to_string();
        assert!(
            store
                .bind_workdir_create_repository_access(
                    "workspace",
                    "call-1",
                    &record.request_fingerprint,
                    "credential-1",
                    "key-a",
                    "trust-1",
                    "trust-a",
                    "read_only",
                    &changed_candidates,
                    "2026-08-24T00:00:02Z",
                )
                .is_err()
        );
        let mut changed_resolution = record.clone();
        changed_resolution.resolved_runtime_id = "other".to_string();
        changed_resolution.config_projection_digest = "sha256:changed".to_string();
        changed_resolution.source_uri = Some("ssh://git@other.test/repo.git".to_string());
        changed_resolution.source_fingerprint = Some("sha256:changed-source".to_string());
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
        assert_eq!(retry.credential_candidates, candidates);
        assert_eq!(
            store
                .load_workdir_create_operation("workspace", "call-1")
                .unwrap(),
            Some(retry.clone())
        );
        let mut changed_input = record.clone();
        changed_input.request_fingerprint =
            request_fingerprint("main", Some("main"), None, None, "sha256:test");
        assert!(
            store
                .reserve_workdir_create_operation(&changed_input)
                .unwrap_err()
                .to_string()
                .contains("reused with different input")
        );
    }
}
