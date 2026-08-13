use chrono::{SecondsFormat, Utc};
use config_source::{
    ConfigContentType, ConfigEntry, ConfigSchemaContribution, ConfigTreeChange, ConfigTreeSnapshot,
    DECODAL_VERSION, DEFAULT_IMPORT_POLICY_VERSION, DEFAULT_SCHEMA_VERSION, EvaluationResult,
    SnapshotEnvironment, ToolchainContract, VirtualPath, WorkspaceConfigSchemaBundle,
};
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};

use crate::{Error, Result, SqliteWorkspaceStore};

pub const MAIN_CONFIG_ENTRYPOINT: &str = "main.dcdl";
pub const DEFAULT_MAIN_CONFIG_SOURCE: &str = "{}\n";

fn main_config_path() -> VirtualPath {
    VirtualPath::parse(MAIN_CONFIG_ENTRYPOINT).expect("main config entrypoint is a valid path")
}

pub trait WorkspaceConfigSchemaProvider: Send + Sync {
    fn contribution(&self) -> Result<ConfigSchemaContribution>;
}

#[derive(Clone, Default)]
pub struct WorkspaceConfigSchemaRegistry {
    providers: Vec<std::sync::Arc<dyn WorkspaceConfigSchemaProvider>>,
}

impl WorkspaceConfigSchemaRegistry {
    pub fn with_provider(
        mut self,
        provider: std::sync::Arc<dyn WorkspaceConfigSchemaProvider>,
    ) -> Self {
        self.providers.push(provider);
        self
    }

    pub fn compose(&self) -> Result<WorkspaceConfigSchemaBundle> {
        WorkspaceConfigSchemaBundle::compose(
            self.providers
                .iter()
                .map(|provider| provider.contribution())
                .collect::<Result<Vec<_>>>()?,
        )
        .map_err(config_error)
    }
}

fn main_config_contract_with_schema(
    schema_bundle: WorkspaceConfigSchemaBundle,
) -> ToolchainContract {
    ToolchainContract::with_schema_bundle(
        DEFAULT_SCHEMA_VERSION,
        vec![main_config_path()],
        DEFAULT_IMPORT_POLICY_VERSION,
        schema_bundle,
    )
}

fn main_config_contract() -> ToolchainContract {
    main_config_contract_with_schema(WorkspaceConfigSchemaBundle::empty())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct WorkspaceConfigState {
    pub snapshot: ConfigTreeSnapshot,
    pub contract: ToolchainContract,
    pub projection_digest: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct EvaluatedConfigCandidate {
    #[ts(type = "number")]
    pub base_revision: u64,
    pub base_digest: String,
    pub snapshot: ConfigTreeSnapshot,
    pub contract: ToolchainContract,
    pub evaluation: EvaluationResult,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct ConfigCommitRequest {
    #[ts(type = "number")]
    pub base_revision: u64,
    pub base_digest: String,
    pub changes: Vec<ConfigTreeChange>,
    pub entrypoints: Vec<VirtualPath>,
    pub toolchain_fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct ConfigPreviewRequest {
    pub changes: Vec<ConfigTreeChange>,
    pub entrypoints: Vec<VirtualPath>,
}

impl SqliteWorkspaceStore {
    pub fn ensure_workspace_config_materialized(
        &self,
        workspace_id: &str,
        materialized_at: &str,
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
            let state = match load_state(&tx, workspace_id)? {
                Some(state) => state,
                None => {
                    let state = initial_state()?;
                    insert_materialized_state(&tx, workspace_id, &state, materialized_at)?;
                    state
                }
            };
            tx.commit()?;
            Ok(state)
        })
    }

    pub fn load_workspace_config(
        &self,
        workspace_id: &str,
    ) -> Result<Option<WorkspaceConfigState>> {
        self.with_conn(|conn| load_state(conn, workspace_id))
    }

    pub fn load_workspace_config_revision(
        &self,
        workspace_id: &str,
        revision: u64,
    ) -> Result<Option<ConfigTreeSnapshot>> {
        self.with_conn(|conn| {
            let manifest = conn
                .query_row(
                    "SELECT tree_digest, manifest_json FROM workspace_config_tree_revisions WHERE workspace_id = ?1 AND revision = ?2",
                    params![workspace_id, revision as i64],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .optional()?;
            let Some((stored_digest, manifest_json)) = manifest else {
                return Ok(None);
            };
            let entries: std::collections::BTreeMap<VirtualPath, ConfigEntry> =
                serde_json::from_str(&manifest_json)
                    .map_err(|error| Error::RegistryInconsistency(error.to_string()))?;
            let snapshot = ConfigTreeSnapshot::from_entries(revision, entries.into_values())
                .map_err(config_error)?;
            if snapshot.digest != stored_digest {
                return Err(Error::RegistryInconsistency(format!(
                    "virtual config revision digest mismatch for Workspace {workspace_id} revision {revision}"
                )));
            }
            Ok(Some(snapshot))
        })
    }

    pub fn evaluate_workspace_config_candidate_with_schema(
        &self,
        workspace_id: &str,
        request: &ConfigCommitRequest,
        schema_bundle: WorkspaceConfigSchemaBundle,
    ) -> Result<EvaluatedConfigCandidate> {
        let current = self
            .load_workspace_config(workspace_id)?
            .ok_or_else(config_not_materialized)?;
        validate_entrypoint_request(&request.entrypoints)?;
        if current.snapshot.revision != request.base_revision
            || current.snapshot.digest != request.base_digest
        {
            return Err(config_conflict(format!(
                "base revision/digest mismatch; current revision is {}",
                current.snapshot.revision
            )));
        }
        let expected_contract = main_config_contract_with_schema(schema_bundle.clone());
        if expected_contract.fingerprint != request.toolchain_fingerprint {
            return Err(config_conflict(format!(
                "toolchain fingerprint mismatch; current fingerprint is {}",
                expected_contract.fingerprint
            )));
        }
        evaluate_candidate(current, &request.changes, schema_bundle)
    }

    pub fn evaluate_workspace_config_candidate(
        &self,
        workspace_id: &str,
        request: &ConfigCommitRequest,
    ) -> Result<EvaluatedConfigCandidate> {
        self.evaluate_workspace_config_candidate_with_schema(
            workspace_id,
            request,
            WorkspaceConfigSchemaBundle::empty(),
        )
    }

    pub fn preview_workspace_config_with_schema(
        &self,
        workspace_id: &str,
        request: &ConfigPreviewRequest,
        schema_bundle: WorkspaceConfigSchemaBundle,
    ) -> Result<EvaluatedConfigCandidate> {
        validate_entrypoint_request(&request.entrypoints)?;
        let current = self
            .load_workspace_config(workspace_id)?
            .ok_or_else(config_not_materialized)?;
        evaluate_candidate(current, &request.changes, schema_bundle)
    }

    pub fn preview_workspace_config(
        &self,
        workspace_id: &str,
        request: &ConfigPreviewRequest,
    ) -> Result<EvaluatedConfigCandidate> {
        self.preview_workspace_config_with_schema(
            workspace_id,
            request,
            WorkspaceConfigSchemaBundle::empty(),
        )
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
            let current = load_state(&tx, workspace_id)?.ok_or_else(config_not_materialized)?;
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
                    decodal_version, import_policy_version, schema_bundle_json,
                    toolchain_fingerprint, projection_digest, updated_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                ON CONFLICT(workspace_id) DO UPDATE SET
                    revision = excluded.revision,
                    tree_digest = excluded.tree_digest,
                    schema_version = excluded.schema_version,
                    entrypoints_json = excluded.entrypoints_json,
                    decodal_version = excluded.decodal_version,
                    import_policy_version = excluded.import_policy_version,
                    schema_bundle_json = excluded.schema_bundle_json,
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
                    serde_json::to_string(&candidate.contract.schema_bundle)
                        .map_err(|error| Error::Store(error.to_string()))?,
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
                    schema_bundle_json, projection_digest, manifest_json, created_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)"#,
                params![
                    workspace_id,
                    next_revision as i64,
                    snapshot.digest,
                    candidate.contract.fingerprint,
                    serde_json::to_string(&candidate.contract.schema_bundle)
                        .map_err(|error| Error::Store(error.to_string()))?,
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
    schema_bundle: WorkspaceConfigSchemaBundle,
) -> Result<EvaluatedConfigCandidate> {
    reject_main_entrypoint_mutation(changes)?;
    let snapshot = current.snapshot.apply(changes).map_err(config_error)?;
    ensure_main_entrypoint(&snapshot)?;
    let contract = main_config_contract_with_schema(schema_bundle);
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

pub(crate) fn load_state(
    conn: &rusqlite::Connection,
    workspace_id: &str,
) -> Result<Option<WorkspaceConfigState>> {
    let has_schema_bundle: bool = conn.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM pragma_table_info('workspace_config_trees')
            WHERE name = 'schema_bundle_json'
         )",
        [],
        |row| row.get(0),
    )?;
    let header = if has_schema_bundle {
        conn.query_row(
            r#"SELECT revision, tree_digest, schema_version, entrypoints_json,
                      decodal_version, import_policy_version, schema_bundle_json,
                      toolchain_fingerprint, projection_digest
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
                    Some(row.get::<_, String>(6)?),
                    row.get::<_, String>(7)?,
                    row.get::<_, String>(8)?,
                ))
            },
        )
        .optional()?
    } else {
        conn.query_row(
            r#"SELECT revision, tree_digest, schema_version, entrypoints_json,
                      decodal_version, import_policy_version,
                      toolchain_fingerprint, projection_digest
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
                    None,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                ))
            },
        )
        .optional()?
    };
    let Some((
        revision,
        stored_digest,
        schema_version,
        entrypoints_json,
        decodal_version,
        import_policy_version,
        schema_bundle_json,
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
    let schema_bundle = match schema_bundle_json {
        Some(schema_bundle_json) => serde_json::from_str(&schema_bundle_json)
            .map_err(|error| Error::RegistryInconsistency(error.to_string()))?,
        None => WorkspaceConfigSchemaBundle::empty(),
    };
    let contract = ToolchainContract::with_schema_bundle(
        schema_version,
        entrypoints,
        import_policy_version,
        schema_bundle,
    );
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

pub(crate) fn initial_state() -> Result<WorkspaceConfigState> {
    let path = main_config_path();
    let snapshot = ConfigTreeSnapshot::empty()
        .apply(&[ConfigTreeChange::Create {
            path,
            content_type: ConfigContentType::Decodal,
            content: DEFAULT_MAIN_CONFIG_SOURCE.to_string(),
        }])
        .map_err(config_error)?;
    let contract = main_config_contract();
    let projection_digest = SnapshotEnvironment::new(snapshot.clone())
        .evaluate_contract(&contract)
        .map_err(|diagnostics| {
            Error::InvalidInput(
                serde_json::to_string(&diagnostics)
                    .unwrap_or_else(|_| "virtual config evaluation failed".to_string()),
            )
        })?
        .projection_digest;
    Ok(WorkspaceConfigState {
        snapshot,
        contract,
        projection_digest,
    })
}

pub(crate) fn insert_materialized_state(
    tx: &rusqlite::Connection,
    workspace_id: &str,
    state: &WorkspaceConfigState,
    materialized_at: &str,
) -> Result<()> {
    let entrypoints_json = serde_json::to_string(&state.contract.entrypoints)
        .map_err(|error| Error::Store(error.to_string()))?;
    let manifest_json = serde_json::to_string(&state.snapshot.entries)
        .map_err(|error| Error::Store(error.to_string()))?;
    let has_schema_bundle: bool = tx.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM pragma_table_info('workspace_config_trees')
            WHERE name = 'schema_bundle_json'
         )",
        [],
        |row| row.get(0),
    )?;
    let schema_bundle_json = serde_json::to_string(&state.contract.schema_bundle)
        .map_err(|error| Error::Store(error.to_string()))?;
    if has_schema_bundle {
        tx.execute(
            "INSERT INTO workspace_config_trees (
                workspace_id, revision, tree_digest, schema_version, entrypoints_json,
                decodal_version, import_policy_version, schema_bundle_json,
                toolchain_fingerprint, projection_digest, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT(workspace_id) DO UPDATE SET
                revision = excluded.revision,
                tree_digest = excluded.tree_digest,
                schema_version = excluded.schema_version,
                entrypoints_json = excluded.entrypoints_json,
                decodal_version = excluded.decodal_version,
                import_policy_version = excluded.import_policy_version,
                schema_bundle_json = excluded.schema_bundle_json,
                toolchain_fingerprint = excluded.toolchain_fingerprint,
                projection_digest = excluded.projection_digest,
                updated_at = excluded.updated_at",
            rusqlite::params![
                workspace_id,
                state.snapshot.revision,
                state.snapshot.digest,
                state.contract.schema_version,
                entrypoints_json,
                DECODAL_VERSION,
                state.contract.import_policy_version,
                schema_bundle_json,
                state.contract.fingerprint,
                state.projection_digest,
                materialized_at,
            ],
        )?;
    } else {
        tx.execute(
            "INSERT INTO workspace_config_trees (
                workspace_id, revision, tree_digest, schema_version, entrypoints_json,
                decodal_version, import_policy_version, toolchain_fingerprint,
                projection_digest, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(workspace_id) DO UPDATE SET
                revision = excluded.revision,
                tree_digest = excluded.tree_digest,
                schema_version = excluded.schema_version,
                entrypoints_json = excluded.entrypoints_json,
                decodal_version = excluded.decodal_version,
                import_policy_version = excluded.import_policy_version,
                toolchain_fingerprint = excluded.toolchain_fingerprint,
                projection_digest = excluded.projection_digest,
                updated_at = excluded.updated_at",
            rusqlite::params![
                workspace_id,
                state.snapshot.revision,
                state.snapshot.digest,
                state.contract.schema_version,
                entrypoints_json,
                DECODAL_VERSION,
                state.contract.import_policy_version,
                state.contract.fingerprint,
                state.projection_digest,
                materialized_at,
            ],
        )?;
    }
    for entry in state.snapshot.entries.values() {
        tx.execute(
            "INSERT INTO workspace_config_entries (
                workspace_id, path, content_type, content, content_digest
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![
                workspace_id,
                entry.path.as_str(),
                content_type_label(entry.content_type),
                entry.content,
                entry.content_digest,
            ],
        )?;
    }
    if has_schema_bundle {
        tx.execute(
            "INSERT INTO workspace_config_tree_revisions (
                workspace_id, revision, tree_digest, toolchain_fingerprint,
                schema_bundle_json, projection_digest, manifest_json, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![
                workspace_id,
                state.snapshot.revision,
                state.snapshot.digest,
                state.contract.fingerprint,
                schema_bundle_json,
                state.projection_digest,
                manifest_json,
                materialized_at,
            ],
        )?;
    } else {
        tx.execute(
            "INSERT INTO workspace_config_tree_revisions (
                workspace_id, revision, tree_digest, toolchain_fingerprint,
                projection_digest, manifest_json, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                workspace_id,
                state.snapshot.revision,
                state.snapshot.digest,
                state.contract.fingerprint,
                state.projection_digest,
                manifest_json,
                materialized_at,
            ],
        )?;
    }
    Ok(())
}

fn config_not_materialized() -> Error {
    Error::RegistryInconsistency("workspace config tree is not materialized".to_string())
}

fn validate_entrypoint_request(entrypoints: &[VirtualPath]) -> Result<()> {
    if entrypoints == [main_config_path()] {
        Ok(())
    } else {
        Err(Error::InvalidInput(format!(
            "workspace config entrypoints must be exactly [{MAIN_CONFIG_ENTRYPOINT}]"
        )))
    }
}

fn reject_main_entrypoint_mutation(changes: &[ConfigTreeChange]) -> Result<()> {
    let main = main_config_path();
    for change in changes {
        match change {
            ConfigTreeChange::Delete { path, .. } if path == &main => {
                return Err(Error::InvalidInput(format!(
                    "{MAIN_CONFIG_ENTRYPOINT} is the required Workspace entrypoint and cannot be deleted"
                )));
            }
            ConfigTreeChange::Rename { from, to, .. } if from == &main || to == &main => {
                return Err(Error::InvalidInput(format!(
                    "{MAIN_CONFIG_ENTRYPOINT} is the required Workspace entrypoint and cannot be renamed"
                )));
            }
            ConfigTreeChange::Create { path, .. } if path == &main => {
                return Err(Error::WorkspaceConfigConflict(format!(
                    "{MAIN_CONFIG_ENTRYPOINT} is already materialized"
                )));
            }
            _ => {}
        }
    }
    Ok(())
}

fn ensure_main_entrypoint(snapshot: &ConfigTreeSnapshot) -> Result<()> {
    if snapshot.entries.contains_key(&main_config_path()) {
        Ok(())
    } else {
        Err(Error::RegistryInconsistency(format!(
            "workspace config tree is missing required entrypoint {MAIN_CONFIG_ENTRYPOINT}"
        )))
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

    async fn open_store() -> SqliteWorkspaceStore {
        let store = SqliteWorkspaceStore::in_memory().unwrap();
        store.upsert_workspace(&workspace()).await.unwrap();
        store
    }

    fn path(value: &str) -> VirtualPath {
        VirtualPath::parse(value).unwrap()
    }

    fn commit_request(
        current: &WorkspaceConfigState,
        changes: Vec<ConfigTreeChange>,
    ) -> ConfigCommitRequest {
        ConfigCommitRequest {
            base_revision: current.snapshot.revision,
            base_digest: current.snapshot.digest.clone(),
            changes,
            entrypoints: vec![path(MAIN_CONFIG_ENTRYPOINT)],
            toolchain_fingerprint: current.contract.fingerprint.clone(),
        }
    }

    fn update_main(current: &WorkspaceConfigState, content: &str) -> ConfigTreeChange {
        let main = current.snapshot.get(&path(MAIN_CONFIG_ENTRYPOINT)).unwrap();
        ConfigTreeChange::Update {
            path: path(MAIN_CONFIG_ENTRYPOINT),
            expected_digest: main.content_digest.clone(),
            content: content.to_string(),
        }
    }

    #[tokio::test]
    async fn schema_registry_applies_normal_decodal_composition() {
        struct WebSchema;

        impl WorkspaceConfigSchemaProvider for WebSchema {
            fn contribution(&self) -> Result<ConfigSchemaContribution> {
                ConfigSchemaContribution::new(
                    "builtin:web",
                    "web",
                    "1",
                    "{ web = { enabled = Bool default false; }; }",
                )
                .map_err(config_error)
            }
        }

        let store = open_store().await;
        let current = store.load_workspace_config("w-config").unwrap().unwrap();
        let main = current.snapshot.get(&path(MAIN_CONFIG_ENTRYPOINT)).unwrap();
        let registry =
            WorkspaceConfigSchemaRegistry::default().with_provider(std::sync::Arc::new(WebSchema));
        let schema = registry.compose().unwrap();
        let expected_contract = main_config_contract_with_schema(schema.clone());
        let candidate = store
            .evaluate_workspace_config_candidate_with_schema(
                "w-config",
                &ConfigCommitRequest {
                    base_revision: current.snapshot.revision,
                    base_digest: current.snapshot.digest.clone(),
                    changes: vec![ConfigTreeChange::Update {
                        path: path(MAIN_CONFIG_ENTRYPOINT),
                        expected_digest: main.content_digest.clone(),
                        content: "{ web = {}; }".to_string(),
                    }],
                    entrypoints: vec![path(MAIN_CONFIG_ENTRYPOINT)],
                    toolchain_fingerprint: expected_contract.fingerprint.clone(),
                },
                schema,
            )
            .unwrap();
        assert_eq!(
            candidate.evaluation.projections[0].data_json["web"]["enabled"],
            false
        );
        assert_eq!(
            candidate.contract.fingerprint,
            expected_contract.fingerprint
        );
        store
            .commit_evaluated_workspace_config("w-config", &candidate)
            .unwrap();
        assert_eq!(
            store
                .load_workspace_config("w-config")
                .unwrap()
                .unwrap()
                .contract
                .schema_bundle,
            expected_contract.schema_bundle
        );
    }

    #[tokio::test]
    async fn commit_rejects_stale_schema_bundle_fingerprint() {
        let store = open_store().await;
        let current = store.load_workspace_config("w-config").unwrap().unwrap();
        let schema = WorkspaceConfigSchemaBundle::compose([ConfigSchemaContribution::new(
            "builtin:web",
            "web",
            "1",
            "{ web = {}; }",
        )
        .unwrap()])
        .unwrap();
        let error = store
            .evaluate_workspace_config_candidate_with_schema(
                "w-config",
                &ConfigCommitRequest {
                    base_revision: current.snapshot.revision,
                    base_digest: current.snapshot.digest,
                    changes: Vec::new(),
                    entrypoints: vec![path(MAIN_CONFIG_ENTRYPOINT)],
                    toolchain_fingerprint: current.contract.fingerprint,
                },
                schema,
            )
            .unwrap_err();
        assert!(error.to_string().contains("toolchain fingerprint mismatch"));
    }

    #[tokio::test]
    async fn workspace_materializes_main_entrypoint() {
        let store = open_store().await;
        let current = store.load_workspace_config("w-config").unwrap().unwrap();
        assert_eq!(current.snapshot.revision, 0);
        assert_eq!(
            current.contract.entrypoints,
            vec![path(MAIN_CONFIG_ENTRYPOINT)]
        );
        assert_eq!(
            current
                .snapshot
                .get(&path(MAIN_CONFIG_ENTRYPOINT))
                .unwrap()
                .content,
            DEFAULT_MAIN_CONFIG_SOURCE
        );
    }

    #[tokio::test]
    async fn required_main_entrypoint_cannot_be_deleted_or_renamed() {
        let store = open_store().await;
        let current = store.load_workspace_config("w-config").unwrap().unwrap();
        let main = current.snapshot.get(&path(MAIN_CONFIG_ENTRYPOINT)).unwrap();
        for change in [
            ConfigTreeChange::Delete {
                path: path(MAIN_CONFIG_ENTRYPOINT),
                expected_digest: main.content_digest.clone(),
            },
            ConfigTreeChange::Rename {
                from: path(MAIN_CONFIG_ENTRYPOINT),
                to: path("other.dcdl"),
                expected_digest: main.content_digest.clone(),
            },
        ] {
            let error = store
                .preview_workspace_config(
                    "w-config",
                    &ConfigPreviewRequest {
                        changes: vec![change],
                        entrypoints: vec![path(MAIN_CONFIG_ENTRYPOINT)],
                    },
                )
                .unwrap_err();
            assert!(error.to_string().contains("cannot be"));
        }
    }

    #[tokio::test]
    async fn browser_cannot_replace_server_owned_entrypoint_contract() {
        let store = open_store().await;
        let error = store
            .preview_workspace_config(
                "w-config",
                &ConfigPreviewRequest {
                    changes: Vec::new(),
                    entrypoints: vec![path("other.dcdl")],
                },
            )
            .unwrap_err();
        assert!(error.to_string().contains("must be exactly [main.dcdl]"));
    }

    #[tokio::test]
    async fn invalid_candidate_is_never_persisted() {
        let store = SqliteWorkspaceStore::in_memory().unwrap();
        store.upsert_workspace(&workspace()).await.unwrap();
        let current = store.load_workspace_config("w-config").unwrap().unwrap();
        let main = current.snapshot.get(&path(MAIN_CONFIG_ENTRYPOINT)).unwrap();
        let error = store
            .evaluate_and_commit_workspace_config(
                "w-config",
                &ConfigCommitRequest {
                    base_revision: current.snapshot.revision,
                    base_digest: current.snapshot.digest.clone(),
                    changes: vec![ConfigTreeChange::Update {
                        path: path(MAIN_CONFIG_ENTRYPOINT),
                        expected_digest: main.content_digest.clone(),
                        content: "{ broken = ; }".into(),
                    }],
                    entrypoints: vec![path(MAIN_CONFIG_ENTRYPOINT)],
                    toolchain_fingerprint: ToolchainContract::new(
                        DEFAULT_SCHEMA_VERSION,
                        vec![path(MAIN_CONFIG_ENTRYPOINT)],
                        DEFAULT_IMPORT_POLICY_VERSION,
                    )
                    .fingerprint,
                },
            )
            .unwrap_err();
        assert!(matches!(error, Error::InvalidInput(_)));
        assert!(store.load_workspace_config("w-config").unwrap().is_some());
    }

    #[tokio::test]
    async fn valid_candidate_commits_snapshot_revision_and_provenance_atomically() {
        let store = SqliteWorkspaceStore::in_memory().unwrap();
        store.upsert_workspace(&workspace()).await.unwrap();
        let current = store.load_workspace_config("w-config").unwrap().unwrap();
        let committed = store
            .evaluate_and_commit_workspace_config(
                "w-config",
                &commit_request(&current, vec![update_main(&current, "{ answer = 42; }")]),
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
        let current = store.load_workspace_config("w-config").unwrap().unwrap();
        let request = commit_request(&current, vec![update_main(&current, "{ answer = 42; }")]);
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

    #[tokio::test]
    async fn committed_revision_remains_retrievable_after_later_commit() {
        let store = SqliteWorkspaceStore::in_memory().unwrap();
        store.upsert_workspace(&workspace()).await.unwrap();
        let current = store.load_workspace_config("w-config").unwrap().unwrap();
        let first = store
            .evaluate_and_commit_workspace_config(
                "w-config",
                &commit_request(&current, vec![update_main(&current, "{ answer = 1; }")]),
            )
            .unwrap();
        let entry = first.snapshot.get(&path(MAIN_CONFIG_ENTRYPOINT)).unwrap();
        store
            .evaluate_and_commit_workspace_config(
                "w-config",
                &ConfigCommitRequest {
                    base_revision: first.snapshot.revision,
                    base_digest: first.snapshot.digest.clone(),
                    changes: vec![ConfigTreeChange::Update {
                        path: path(MAIN_CONFIG_ENTRYPOINT),
                        expected_digest: entry.content_digest.clone(),
                        content: "{ answer = 2; }".into(),
                    }],
                    entrypoints: first.contract.entrypoints.clone(),
                    toolchain_fingerprint: first.contract.fingerprint.clone(),
                },
            )
            .unwrap();
        let revision = store
            .load_workspace_config_revision("w-config", 1)
            .unwrap()
            .unwrap();
        assert_eq!(revision, first.snapshot);
    }

    #[tokio::test]
    async fn commit_rejects_mismatched_toolchain_fingerprint() {
        let store = SqliteWorkspaceStore::in_memory().unwrap();
        store.upsert_workspace(&workspace()).await.unwrap();
        let current = store.load_workspace_config("w-config").unwrap().unwrap();
        let error = store
            .evaluate_and_commit_workspace_config(
                "w-config",
                &ConfigCommitRequest {
                    base_revision: current.snapshot.revision,
                    base_digest: current.snapshot.digest.clone(),
                    changes: vec![update_main(&current, "{ answer = 42; }")],
                    entrypoints: vec![path(MAIN_CONFIG_ENTRYPOINT)],
                    toolchain_fingerprint: "sha256:stale-toolchain".into(),
                },
            )
            .unwrap_err();
        assert!(matches!(error, Error::WorkspaceConfigConflict(_)));
        assert!(store.load_workspace_config("w-config").unwrap().is_some());
    }

    #[tokio::test]
    async fn migration_materializes_main_for_existing_workspace_without_config() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::store::configure_sqlite(&conn).unwrap();
        crate::store::apply_migrations_through(&conn, 30).unwrap();
        conn.execute(
            "INSERT INTO workspaces (
                workspace_id, display_name, state, created_at, updated_at
             ) VALUES ('legacy', 'Legacy', 'active', '2026-08-06T00:00:00Z', '2026-08-06T00:00:00Z')",
            [],
        )
        .unwrap();
        crate::store::persist_workspace_config_schema_bundles(&conn).unwrap();
        crate::store::materialize_main_config_entrypoint(&conn).unwrap();
        let state = load_state(&conn, "legacy").unwrap().unwrap();
        assert!(
            state
                .snapshot
                .entries
                .contains_key(&path(MAIN_CONFIG_ENTRYPOINT))
        );
        assert_eq!(
            state.contract.entrypoints,
            vec![path(MAIN_CONFIG_ENTRYPOINT)]
        );
    }

    #[test]
    fn exports_typescript_transport_contract() {
        use ts_rs::TS;
        let output = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../web/workspace/src/lib/workspace/config-source/generated/types");
        let config = ts_rs::Config::default().with_out_dir(&output);
        WorkspaceConfigState::export_all(&config).unwrap();
        EvaluatedConfigCandidate::export_all(&config).unwrap();
        ConfigCommitRequest::export_all(&config).unwrap();
        ConfigPreviewRequest::export_all(&config).unwrap();
    }

    #[test]
    fn migration_creates_config_authority_tables() {
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
