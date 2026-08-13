use chrono::{SecondsFormat, Utc};
use config_source::{
    ConfigContentType, ConfigEntry, ConfigTreeChange, ConfigTreeSnapshot, DECODAL_VERSION,
    DEFAULT_IMPORT_POLICY_VERSION, DEFAULT_SCHEMA_VERSION, EvaluationResult, SnapshotEnvironment,
    ToolchainContract, VirtualPath,
};
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};

use crate::{Error, Result, SqliteWorkspaceStore};

pub const DEFAULT_CONFIG_ENTRYPOINT: &str = "workspace.dcdl";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceConfigState {
    pub snapshot: ConfigTreeSnapshot,
    pub contract: ToolchainContract,
    pub projection_digest: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvaluatedConfigCandidate {
    pub base_revision: u64,
    pub base_digest: String,
    pub snapshot: ConfigTreeSnapshot,
    pub contract: ToolchainContract,
    pub evaluation: EvaluationResult,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConfigCommitRequest {
    pub base_revision: u64,
    pub base_digest: String,
    pub changes: Vec<ConfigTreeChange>,
    pub entrypoints: Vec<VirtualPath>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConfigPreviewRequest {
    pub changes: Vec<ConfigTreeChange>,
    pub entrypoints: Vec<VirtualPath>,
}

impl SqliteWorkspaceStore {
    pub fn load_workspace_config(
        &self,
        workspace_id: &str,
    ) -> Result<Option<WorkspaceConfigState>> {
        self.with_conn(|conn| load_state(conn, workspace_id))
    }

    pub fn evaluate_workspace_config_candidate(
        &self,
        workspace_id: &str,
        request: &ConfigCommitRequest,
    ) -> Result<EvaluatedConfigCandidate> {
        let current = self
            .load_workspace_config(workspace_id)?
            .unwrap_or_else(empty_state);
        if current.snapshot.revision != request.base_revision
            || current.snapshot.digest != request.base_digest
        {
            return Err(config_conflict(format!(
                "base revision/digest mismatch; current revision is {}",
                current.snapshot.revision
            )));
        }
        evaluate_candidate(current, &request.changes, request.entrypoints.clone())
    }

    pub fn preview_workspace_config(
        &self,
        workspace_id: &str,
        request: &ConfigPreviewRequest,
    ) -> Result<EvaluatedConfigCandidate> {
        let current = self
            .load_workspace_config(workspace_id)?
            .unwrap_or_else(empty_state);
        evaluate_candidate(current, &request.changes, request.entrypoints.clone())
    }

    pub fn commit_evaluated_workspace_config(
        &self,
        workspace_id: &str,
        candidate: &EvaluatedConfigCandidate,
    ) -> Result<WorkspaceConfigState> {
        self.with_conn_mut(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let workspace_exists: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM workspaces WHERE workspace_id = ?1)",
                [workspace_id],
                |row| row.get(0),
            )?;
            if !workspace_exists {
                return Err(Error::WorkspaceIdMismatch);
            }
            let current = load_state(&tx, workspace_id)?.unwrap_or_else(empty_state);
            if current.snapshot.revision != candidate.base_revision
                || current.snapshot.digest != candidate.base_digest
            {
                return Err(config_conflict(format!(
                    "base revision/digest mismatch; current revision is {}",
                    current.snapshot.revision
                )));
            }
            let next_revision = current.snapshot.revision + 1;
            let mut snapshot = candidate.snapshot.clone();
            snapshot.revision = next_revision;
            let now = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
            tx.execute(
                r#"INSERT INTO workspace_config_trees (
                    workspace_id, revision, tree_digest, schema_version, entrypoints_json,
                    decodal_version, import_policy_version, toolchain_fingerprint,
                    projection_digest, updated_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                ON CONFLICT(workspace_id) DO UPDATE SET
                    revision = excluded.revision,
                    tree_digest = excluded.tree_digest,
                    schema_version = excluded.schema_version,
                    decodal_version = excluded.decodal_version,
                    import_policy_version = excluded.import_policy_version,
                    toolchain_fingerprint = excluded.toolchain_fingerprint,
                    projection_digest = excluded.projection_digest,
                    updated_at = excluded.updated_at"#,
                params![
                    workspace_id,
                    next_revision as i64,
                    snapshot.digest,
                    candidate.contract.schema_version,
                    serde_json::to_string(&candidate.contract.entrypoints)
                        .map_err(|error| Error::Store(error.to_string()))?,
                    candidate.contract.decodal_version,
                    candidate.contract.import_policy_version,
                    candidate.contract.fingerprint,
                    candidate.evaluation.projection_digest,
                    now,
                ],
            )?;
            tx.execute(
                "DELETE FROM workspace_config_entries WHERE workspace_id = ?1",
                [workspace_id],
            )?;
            for entry in snapshot.entries.values() {
                tx.execute(
                    r#"INSERT INTO workspace_config_entries (
                        workspace_id, path, content_type, content, content_digest
                    ) VALUES (?1, ?2, ?3, ?4, ?5)"#,
                    params![
                        workspace_id,
                        entry.path.as_str(),
                        content_type_label(entry.content_type),
                        entry.content,
                        entry.content_digest,
                    ],
                )?;
            }
            let manifest_json = serde_json::to_string(&snapshot.entries)
                .map_err(|error| Error::Store(error.to_string()))?;
            tx.execute(
                r#"INSERT INTO workspace_config_tree_revisions (
                    workspace_id, revision, tree_digest, toolchain_fingerprint,
                    projection_digest, manifest_json, created_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"#,
                params![
                    workspace_id,
                    next_revision as i64,
                    snapshot.digest,
                    candidate.contract.fingerprint,
                    candidate.evaluation.projection_digest,
                    manifest_json,
                    now,
                ],
            )?;
            tx.commit()?;
            Ok(WorkspaceConfigState {
                snapshot,
                contract: candidate.contract.clone(),
                projection_digest: candidate.evaluation.projection_digest.clone(),
            })
        })
    }

    pub fn evaluate_and_commit_workspace_config(
        &self,
        workspace_id: &str,
        request: &ConfigCommitRequest,
    ) -> Result<WorkspaceConfigState> {
        let candidate = self.evaluate_workspace_config_candidate(workspace_id, request)?;
        self.commit_evaluated_workspace_config(workspace_id, &candidate)
    }
}

fn evaluate_candidate(
    current: WorkspaceConfigState,
    changes: &[ConfigTreeChange],
    entrypoints: Vec<VirtualPath>,
) -> Result<EvaluatedConfigCandidate> {
    let snapshot = current.snapshot.apply(changes).map_err(config_error)?;
    let contract = ToolchainContract::new(
        DEFAULT_SCHEMA_VERSION,
        entrypoints,
        DEFAULT_IMPORT_POLICY_VERSION,
    );
    let evaluation = SnapshotEnvironment::new(snapshot.clone())
        .evaluate_contract(&contract)
        .map_err(|diagnostics| {
            Error::InvalidInput(
                serde_json::to_string(&diagnostics)
                    .unwrap_or_else(|_| "virtual config evaluation failed".to_string()),
            )
        })?;
    Ok(EvaluatedConfigCandidate {
        base_revision: current.snapshot.revision,
        base_digest: current.snapshot.digest,
        snapshot,
        contract,
        evaluation,
    })
}

fn load_state(
    conn: &rusqlite::Connection,
    workspace_id: &str,
) -> Result<Option<WorkspaceConfigState>> {
    let header = conn
        .query_row(
            r#"SELECT revision, tree_digest, schema_version, entrypoints_json,
                      decodal_version, import_policy_version, toolchain_fingerprint, projection_digest
               FROM workspace_config_trees WHERE workspace_id = ?1"#,
            [workspace_id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, u32>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, u32>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                ))
            },
        )
        .optional()?;
    let Some((
        revision,
        stored_digest,
        schema_version,
        entrypoints_json,
        decodal_version,
        import_policy_version,
        fingerprint,
        projection_digest,
    )) = header
    else {
        return Ok(None);
    };
    let mut statement = conn.prepare(
        r#"SELECT path, content_type, content, content_digest
           FROM workspace_config_entries WHERE workspace_id = ?1 ORDER BY path"#,
    )?;
    let entries = statement
        .query_map([workspace_id], |row| {
            let path = row.get::<_, String>(0)?;
            let content_type = row.get::<_, String>(1)?;
            let content = row.get::<_, String>(2)?;
            let stored_entry_digest = row.get::<_, String>(3)?;
            Ok((path, content_type, content, stored_entry_digest))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?
        .into_iter()
        .map(|(path, content_type, content, stored_entry_digest)| {
            let path = VirtualPath::parse(path).map_err(config_error)?;
            let entry = ConfigEntry::new(path, parse_content_type(&content_type)?, content)
                .map_err(config_error)?;
            if entry.content_digest != stored_entry_digest {
                return Err(Error::RegistryInconsistency(format!(
                    "virtual config entry digest mismatch for {}",
                    entry.path
                )));
            }
            Ok(entry)
        })
        .collect::<Result<Vec<_>>>()?;
    let snapshot =
        ConfigTreeSnapshot::from_entries(revision as u64, entries).map_err(config_error)?;
    if snapshot.digest != stored_digest {
        return Err(Error::RegistryInconsistency(format!(
            "virtual config tree digest mismatch for Workspace {workspace_id}"
        )));
    }
    let entrypoints: Vec<VirtualPath> = serde_json::from_str(&entrypoints_json)
        .map_err(|error| Error::RegistryInconsistency(error.to_string()))?;
    let contract = ToolchainContract::new(schema_version, entrypoints, import_policy_version);
    if decodal_version != DECODAL_VERSION || contract.fingerprint != fingerprint {
        return Err(Error::RegistryInconsistency(format!(
            "virtual config toolchain metadata mismatch for Workspace {workspace_id}"
        )));
    }
    Ok(Some(WorkspaceConfigState {
        snapshot,
        contract,
        projection_digest,
    }))
}

fn empty_state() -> WorkspaceConfigState {
    WorkspaceConfigState {
        snapshot: ConfigTreeSnapshot::empty(),
        contract: ToolchainContract::new(
            DEFAULT_SCHEMA_VERSION,
            Vec::new(),
            DEFAULT_IMPORT_POLICY_VERSION,
        ),
        projection_digest: config_source::digest_bytes(b"[]"),
    }
}

fn content_type_label(value: ConfigContentType) -> &'static str {
    match value {
        ConfigContentType::Decodal => "decodal",
        ConfigContentType::Text => "text",
    }
}

fn parse_content_type(value: &str) -> Result<ConfigContentType> {
    match value {
        "decodal" => Ok(ConfigContentType::Decodal),
        "text" => Ok(ConfigContentType::Text),
        _ => Err(Error::RegistryInconsistency(format!(
            "unknown virtual config content type {value:?}"
        ))),
    }
}

fn config_error(error: impl std::fmt::Display) -> Error {
    Error::InvalidInput(error.to_string())
}

fn config_conflict(message: impl Into<String>) -> Error {
    Error::WorkspaceConfigConflict(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ControlPlaneStore, WorkspaceRecord};

    fn workspace() -> WorkspaceRecord {
        WorkspaceRecord {
            workspace_id: "w-config".into(),
            owner_account_id: None,
            display_name: "Config".into(),
            state: "active".into(),
            created_at: "2026-08-13T00:00:00Z".into(),
            updated_at: "2026-08-13T00:00:00Z".into(),
        }
    }

    fn path(value: &str) -> VirtualPath {
        VirtualPath::parse(value).unwrap()
    }

    #[tokio::test]
    async fn invalid_candidate_is_never_persisted() {
        let store = SqliteWorkspaceStore::in_memory().unwrap();
        store.upsert_workspace(&workspace()).await.unwrap();
        let current = ConfigTreeSnapshot::empty();
        let error = store
            .evaluate_and_commit_workspace_config(
                "w-config",
                &ConfigCommitRequest {
                    base_revision: 0,
                    base_digest: current.digest,
                    changes: vec![ConfigTreeChange::Create {
                        path: path(DEFAULT_CONFIG_ENTRYPOINT),
                        content_type: ConfigContentType::Decodal,
                        content: "{ broken = ; }".into(),
                    }],
                    entrypoints: vec![path(DEFAULT_CONFIG_ENTRYPOINT)],
                },
            )
            .unwrap_err();
        assert!(matches!(error, Error::InvalidInput(_)));
        assert!(store.load_workspace_config("w-config").unwrap().is_none());
    }

    #[tokio::test]
    async fn valid_candidate_commits_snapshot_revision_and_provenance_atomically() {
        let store = SqliteWorkspaceStore::in_memory().unwrap();
        store.upsert_workspace(&workspace()).await.unwrap();
        let empty = ConfigTreeSnapshot::empty();
        let committed = store
            .evaluate_and_commit_workspace_config(
                "w-config",
                &ConfigCommitRequest {
                    base_revision: 0,
                    base_digest: empty.digest,
                    changes: vec![ConfigTreeChange::Create {
                        path: path(DEFAULT_CONFIG_ENTRYPOINT),
                        content_type: ConfigContentType::Decodal,
                        content: "{ answer = 42; }".into(),
                    }],
                    entrypoints: vec![path(DEFAULT_CONFIG_ENTRYPOINT)],
                },
            )
            .unwrap();
        assert_eq!(committed.snapshot.revision, 1);
        assert_eq!(committed.contract.decodal_version, DECODAL_VERSION);
        assert!(!committed.projection_digest.is_empty());
        let reread = store.load_workspace_config("w-config").unwrap().unwrap();
        assert_eq!(reread.snapshot, committed.snapshot);
    }

    #[tokio::test]
    async fn stale_cas_cannot_overwrite_newer_tree() {
        let store = SqliteWorkspaceStore::in_memory().unwrap();
        store.upsert_workspace(&workspace()).await.unwrap();
        let empty = ConfigTreeSnapshot::empty();
        let request = ConfigCommitRequest {
            base_revision: 0,
            base_digest: empty.digest,
            changes: vec![ConfigTreeChange::Create {
                path: path(DEFAULT_CONFIG_ENTRYPOINT),
                content_type: ConfigContentType::Decodal,
                content: "{ answer = 42; }".into(),
            }],
            entrypoints: vec![path(DEFAULT_CONFIG_ENTRYPOINT)],
        };
        let candidate = store
            .evaluate_workspace_config_candidate("w-config", &request)
            .unwrap();
        store
            .commit_evaluated_workspace_config("w-config", &candidate)
            .unwrap();
        let error = store
            .commit_evaluated_workspace_config("w-config", &candidate)
            .unwrap_err();
        assert!(matches!(error, Error::WorkspaceConfigConflict(_)));
    }

    #[test]
    fn migration_creates_config_authority_without_changing_applied_migrations() {
        let store = SqliteWorkspaceStore::in_memory().unwrap();
        store
            .with_conn(|conn| {
                for table in [
                    "workspace_config_trees",
                    "workspace_config_entries",
                    "workspace_config_tree_revisions",
                ] {
                    let exists: bool = conn.query_row(
                        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
                        [table],
                        |row| row.get(0),
                    )?;
                    assert!(exists, "missing {table}");
                }
                Ok(())
            })
            .unwrap();
    }
}
