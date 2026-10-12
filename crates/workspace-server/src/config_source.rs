use chrono::{SecondsFormat, Utc};
use config_source::{
    ConfigContentType, ConfigDiagnostic, ConfigEntry, ConfigSchemaContribution, ConfigTreeChange,
    ConfigTreeSnapshot, DECODAL_VERSION, DEFAULT_IMPORT_POLICY_VERSION, DEFAULT_SCHEMA_VERSION,
    EvaluationResult, SnapshotEnvironment, ToolchainContract, VirtualPath,
    WorkspaceConfigSchemaBundle,
};
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};

use crate::{Error, Result, SqliteWorkspaceStore};

pub const MAIN_CONFIG_ENTRYPOINT: &str = "main.dcdl";
pub const DEFAULT_MAIN_CONFIG_SOURCE: &str = "{} as WorkspaceConfigSchema\n";
const WORKSPACE_CONFIG_SCHEMA_ASSERTION: &str = "WorkspaceConfigSchema";
const MAX_TOOLCHAIN_UPGRADE_DIAGNOSTICS: usize = 20;

fn toolchain_upgrade_diagnostics(mut diagnostics: Vec<ConfigDiagnostic>) -> Error {
    let omitted = diagnostics
        .len()
        .saturating_sub(MAX_TOOLCHAIN_UPGRADE_DIAGNOSTICS);
    diagnostics.truncate(MAX_TOOLCHAIN_UPGRADE_DIAGNOSTICS);
    let rendered = serde_json::to_string(&diagnostics)
        .unwrap_or_else(|_| "[diagnostics could not be serialized]".to_string());
    let suffix = if omitted == 0 {
        String::new()
    } else {
        format!("; {omitted} additional diagnostic(s) omitted")
    };
    Error::InvalidInput(format!(
        "workspace configuration is invalid under Decodal {DECODAL_VERSION}: {rendered}{suffix}"
    ))
}

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

pub fn evaluate_workspace_config_state(
    state: &WorkspaceConfigState,
    schema_bundle: WorkspaceConfigSchemaBundle,
) -> Result<EvaluationResult> {
    let expected_fingerprint = state.contract.fingerprint.clone();
    let contract = main_config_contract_with_schema(schema_bundle);
    if !state.contract.schema_bundle.contributions.is_empty()
        && contract.fingerprint != expected_fingerprint
    {
        return Err(Error::RegistryInconsistency(
            "active Workspace config schema fingerprint does not match the current provider bundle"
                .to_string(),
        ));
    }
    SnapshotEnvironment::new(state.snapshot.clone())
        .evaluate_contract(&contract)
        .map_err(|diagnostics| {
            Error::InvalidInput(
                serde_json::to_string(&diagnostics)
                    .unwrap_or_else(|_| "virtual config evaluation failed".to_string()),
            )
        })
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
pub struct WorkspaceConfigState {
    pub snapshot: ConfigTreeSnapshot,
    pub contract: ToolchainContract,
    pub projection_digest: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvaluatedConfigCandidate {
    pub base_digest: String,
    pub base_toolchain_fingerprint: String,
    pub base_projection_digest: String,
    pub snapshot: ConfigTreeSnapshot,
    pub contract: ToolchainContract,
    pub evaluation: EvaluationResult,
}

/// Validate feature semantics before committing a schema-valid candidate.
///
/// Keep this shared by the UI and logical config operations. Schema evaluation
/// alone does not resolve Profile sources or reject blocking Skill diagnostics.
/// This function has no publication or repository-secret side effects.
pub(crate) fn validate_workspace_config_candidate_projections(
    workspace_id: &str,
    candidate: &EvaluatedConfigCandidate,
) -> Result<()> {
    let state = WorkspaceConfigState {
        snapshot: candidate.snapshot.clone(),
        contract: candidate.contract.clone(),
        projection_digest: candidate.evaluation.projection_digest.clone(),
    };
    let has_provider = |id: &str| {
        state
            .contract
            .schema_bundle
            .contributions
            .iter()
            .any(|provider| provider.provider_id == id)
    };
    crate::prompt_settings::validate_evaluated_prompt_catalog(&candidate.evaluation)?;
    if has_provider("builtin:profile") {
        crate::profile_settings::project_profiles_from_evaluation(
            workspace_id,
            &state,
            &candidate.evaluation,
        )?;
    }
    if has_provider("builtin:runtime") {
        crate::runtime_settings::project_runtime_from_workspace_config(workspace_id, &state)?;
    }
    if has_provider("builtin:skills") {
        let catalog = crate::skills::catalog(&state)
            .map_err(|_| Error::InvalidInput("invalid Workspace Skill projection".to_string()))?;
        if catalog.entries.iter().any(|entry| {
            entry
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.severity == server_api::SkillDiagnosticSeverity::Error)
        }) {
            return Err(Error::InvalidInput(
                "Workspace Skills have blocking diagnostics".to_string(),
            ));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[serde(deny_unknown_fields)]
pub struct ConfigCommitRequest {
    pub base_digest: String,
    pub changes: Vec<ConfigTreeChange>,
    pub entrypoints: Vec<VirtualPath>,
}

impl SqliteWorkspaceStore {
    pub fn ensure_workspace_config_materialized_with_schema(
        &self,
        workspace_id: &str,
        materialized_at: &str,
        schema_bundle: WorkspaceConfigSchemaBundle,
    ) -> Result<WorkspaceConfigState> {
        let desired_schema = schema_bundle.clone();
        let (state, requires_toolchain_refresh, base_identity) = self.with_conn_mut(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let workspace_exists: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM workspaces WHERE workspace_id = ?1)",
                [workspace_id],
                |row| row.get(0),
            )?;
            if !workspace_exists {
                return Err(Error::WorkspaceIdMismatch);
            }
            let stored_decodal_version = tx
                .query_row(
                    "SELECT decodal_version FROM workspace_config_trees WHERE workspace_id = ?1",
                    [workspace_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            let requires_toolchain_refresh = stored_decodal_version
                .as_deref()
                .is_some_and(|version| version != DECODAL_VERSION);
            let state = match load_state(&tx, workspace_id)? {
                Some(state) => state,
                None => {
                    let state = initial_state_with_schema(schema_bundle.clone())?;
                    insert_materialized_state(&tx, workspace_id, &state, materialized_at)?;
                    state
                }
            };
            let base_identity = load_evaluation_identity(&tx, workspace_id)?;
            tx.commit()?;
            Ok((state, requires_toolchain_refresh, base_identity))
        })?;
        let main_needs_normalization = state
            .snapshot
            .get(&main_config_path())
            .is_some_and(|entry| !main_config_has_schema_assertion(&entry.content));
        if requires_toolchain_refresh
            || state.contract.schema_bundle.fingerprint != desired_schema.fingerprint
            || main_needs_normalization
        {
            let candidate = evaluate_candidate(state, base_identity, &[], desired_schema)?;
            return self.commit_evaluated_workspace_config(workspace_id, &candidate);
        }
        Ok(state)
    }

    pub fn load_workspace_config(
        &self,
        workspace_id: &str,
    ) -> Result<Option<WorkspaceConfigState>> {
        self.with_conn(|conn| load_state(conn, workspace_id))
    }

    pub fn load_workspace_config_history(
        &self,
        workspace_id: &str,
        content_digest: &str,
    ) -> Result<Option<ConfigTreeSnapshot>> {
        self.with_conn(|conn| {
            let manifest = conn
                .query_row(
                    "SELECT content_digest, manifest_json FROM workspace_config_tree_history WHERE workspace_id = ?1 AND content_digest = ?2",
                    params![workspace_id, content_digest],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .optional()?;
            let Some((stored_digest, manifest_json)) = manifest else {
                return Ok(None);
            };
            let entries: std::collections::BTreeMap<VirtualPath, ConfigEntry> =
                serde_json::from_str(&manifest_json)
                    .map_err(|error| Error::RegistryInconsistency(error.to_string()))?;
            if entries.iter().any(|(path, entry)| path != &entry.path) {
                return Err(Error::RegistryInconsistency(
                    "virtual config history manifest path mismatch".into(),
                ));
            }
            let snapshot = ConfigTreeSnapshot::from_entries(entries.into_values())
                .map_err(config_error)?;
            if snapshot.digest != stored_digest {
                return Err(Error::RegistryInconsistency(format!(
                    "virtual config history digest mismatch for Workspace {workspace_id} content {content_digest}"
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
        let (current, base_identity) = self.with_conn(|conn| {
            let tx = conn.unchecked_transaction()?;
            let current = load_state(&tx, workspace_id)?.ok_or_else(config_not_materialized)?;
            let identity = load_evaluation_identity(&tx, workspace_id)?;
            tx.commit()?;
            Ok((current, identity))
        })?;
        validate_entrypoint_request(&request.entrypoints)?;
        if current.snapshot.digest != request.base_digest {
            return Err(config_conflict("base content digest mismatch"));
        }
        evaluate_candidate(current, base_identity, &request.changes, schema_bundle)
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
            if current.snapshot.digest != candidate.base_digest {
                return Err(config_conflict("base content digest mismatch"));
            }
            let (base_toolchain, base_projection) = load_evaluation_identity(&tx, workspace_id)?;
            if base_toolchain != candidate.base_toolchain_fingerprint
                || base_projection != candidate.base_projection_digest
            {
                return Err(config_conflict("base evaluation identity mismatch"));
            }
            let snapshot = candidate.snapshot.clone();
            snapshot.validate().map_err(config_error)?;
            let now = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
            tx.execute(
                "DELETE FROM workspace_config_entries WHERE workspace_id = ?1",
                [workspace_id],
            )?;
            let state = WorkspaceConfigState {
                snapshot: snapshot.clone(),
                contract: candidate.contract.clone(),
                projection_digest: candidate.evaluation.projection_digest.clone(),
            };
            insert_materialized_state(&tx, workspace_id, &state, &now)?;
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

fn main_config_has_schema_assertion(source: &str) -> bool {
    source.starts_with('{')
        && source
            .trim_end()
            .ends_with(&format!("}} as {WORKSPACE_CONFIG_SCHEMA_ASSERTION}"))
}

fn normalize_main_config_schema_assertion(
    snapshot: ConfigTreeSnapshot,
) -> Result<ConfigTreeSnapshot> {
    let main_path = main_config_path();
    let main = snapshot.get(&main_path).ok_or_else(|| {
        Error::InvalidInput(format!(
            "workspace config snapshot must contain {MAIN_CONFIG_ENTRYPOINT}"
        ))
    })?;
    if main_config_has_schema_assertion(&main.content) {
        return Ok(snapshot);
    }

    let source = main.content.trim();
    let normalized = if source.starts_with('{')
        && source.ends_with(&format!("}} as {WORKSPACE_CONFIG_SCHEMA_ASSERTION}"))
    {
        format!("{source}\n")
    } else if source.starts_with('{') && source.ends_with('}') {
        format!("{source} as {WORKSPACE_CONFIG_SCHEMA_ASSERTION}\n")
    } else {
        return Err(Error::InvalidInput(format!(
            "{MAIN_CONFIG_ENTRYPOINT} must be a top-level object so it can be asserted as {WORKSPACE_CONFIG_SCHEMA_ASSERTION}"
        )));
    };
    let mut entries = Vec::with_capacity(snapshot.entries.len());
    for entry in snapshot.entries.into_values() {
        if entry.path == main_path {
            entries.push(
                ConfigEntry::new(entry.path, entry.content_type, normalized.clone())
                    .map_err(config_error)?,
            );
        } else {
            entries.push(entry);
        }
    }
    ConfigTreeSnapshot::from_entries(entries).map_err(config_error)
}

fn format_candidate_sources(
    snapshot: ConfigTreeSnapshot,
    changes: &[ConfigTreeChange],
) -> Result<ConfigTreeSnapshot> {
    let mut paths = std::collections::BTreeSet::from([main_config_path()]);
    for change in changes {
        match change {
            ConfigTreeChange::Create { path, .. } | ConfigTreeChange::Update { path, .. } => {
                paths.insert(path.clone());
            }
            ConfigTreeChange::Rename { to, .. } => {
                paths.insert(to.clone());
            }
            ConfigTreeChange::Delete { .. } => {}
        }
    }

    let environment = SnapshotEnvironment::new(snapshot.clone());
    let mut entries = Vec::with_capacity(snapshot.entries.len());
    for entry in snapshot.entries.into_values() {
        if paths.contains(&entry.path) && entry.content_type == ConfigContentType::Decodal {
            let formatted = environment.format(&entry.content).map_err(config_error)?;
            entries.push(
                ConfigEntry::new(entry.path, entry.content_type, formatted)
                    .map_err(config_error)?,
            );
        } else {
            entries.push(entry);
        }
    }
    ConfigTreeSnapshot::from_entries(entries).map_err(config_error)
}

// Read persisted provenance, not load_state's upgraded in-memory contract.
fn load_evaluation_identity(
    conn: &rusqlite::Connection,
    workspace_id: &str,
) -> Result<(String, String)> {
    conn.query_row(
        "SELECT toolchain_fingerprint, projection_digest FROM workspace_config_trees WHERE workspace_id = ?1",
        [workspace_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .map_err(Error::from)
}

fn evaluate_candidate(
    current: WorkspaceConfigState,
    base_identity: (String, String),
    changes: &[ConfigTreeChange],
    schema_bundle: WorkspaceConfigSchemaBundle,
) -> Result<EvaluatedConfigCandidate> {
    reject_main_entrypoint_mutation(changes)?;
    let snapshot = current.snapshot.apply(changes).map_err(config_error)?;
    ensure_main_entrypoint(&snapshot)?;
    let snapshot = normalize_main_config_schema_assertion(snapshot)?;
    let snapshot = format_candidate_sources(snapshot, changes)?;
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
        base_digest: current.snapshot.digest,
        base_toolchain_fingerprint: base_identity.0,
        base_projection_digest: base_identity.1,
        snapshot,
        contract,
        evaluation,
    })
}

pub(crate) fn load_state(
    conn: &rusqlite::Connection,
    workspace_id: &str,
) -> Result<Option<WorkspaceConfigState>> {
    let header = conn
        .query_row(
            r#"SELECT content_digest, schema_version, entrypoints_json,
                  decodal_version, import_policy_version, schema_bundle_json,
                  toolchain_fingerprint, projection_digest
           FROM workspace_config_trees WHERE workspace_id = ?1"#,
            [workspace_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, u32>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, u32>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                ))
            },
        )
        .optional()?;
    let Some((
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
    let snapshot = ConfigTreeSnapshot::from_entries(entries).map_err(config_error)?;
    if snapshot.digest != stored_digest {
        return Err(Error::RegistryInconsistency(format!(
            "virtual config tree digest mismatch for Workspace {workspace_id}"
        )));
    }
    let entrypoints: Vec<VirtualPath> = serde_json::from_str(&entrypoints_json)
        .map_err(|error| Error::RegistryInconsistency(error.to_string()))?;
    let stored_schema_bundle: WorkspaceConfigSchemaBundle =
        serde_json::from_str(&schema_bundle_json)
            .map_err(|error| Error::RegistryInconsistency(error.to_string()))?;
    let requires_toolchain_refresh = decodal_version != DECODAL_VERSION;
    if requires_toolchain_refresh && !matches!(decodal_version.as_str(), "0.2.0" | "0.3.0") {
        return Err(Error::RegistryInconsistency(format!(
            "unsupported virtual config Decodal version {decodal_version} for Workspace {workspace_id}"
        )));
    }
    let schema_bundle = if requires_toolchain_refresh {
        WorkspaceConfigSchemaBundle::compose(stored_schema_bundle.contributions)
            .map_err(config_error)?
    } else {
        stored_schema_bundle
    };
    let contract = ToolchainContract::with_schema_bundle(
        schema_version,
        entrypoints,
        import_policy_version,
        schema_bundle,
    );
    if !requires_toolchain_refresh && contract.fingerprint != fingerprint {
        return Err(Error::RegistryInconsistency(format!(
            "virtual config toolchain metadata mismatch for Workspace {workspace_id}"
        )));
    }
    let projection_digest = if requires_toolchain_refresh {
        SnapshotEnvironment::new(snapshot.clone())
            .evaluate_contract(&contract)
            .map_err(toolchain_upgrade_diagnostics)?
            .projection_digest
    } else {
        projection_digest
    };
    Ok(Some(WorkspaceConfigState {
        snapshot,
        contract,
        projection_digest,
    }))
}

pub(crate) fn initial_state() -> Result<WorkspaceConfigState> {
    initial_state_with_schema(WorkspaceConfigSchemaBundle::empty())
}

pub(crate) fn initial_state_with_schema(
    schema_bundle: WorkspaceConfigSchemaBundle,
) -> Result<WorkspaceConfigState> {
    let path = main_config_path();
    let snapshot = ConfigTreeSnapshot::empty()
        .apply(&[ConfigTreeChange::Create {
            path,
            content_type: ConfigContentType::Decodal,
            content: DEFAULT_MAIN_CONFIG_SOURCE.to_string(),
        }])
        .map_err(config_error)?;
    let contract = main_config_contract_with_schema(schema_bundle);
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
    let schema_bundle_json = serde_json::to_string(&state.contract.schema_bundle)
        .map_err(|error| Error::Store(error.to_string()))?;
    tx.execute(
        "INSERT INTO workspace_config_trees (
            workspace_id, content_digest, schema_version, entrypoints_json,
            decodal_version, import_policy_version, schema_bundle_json,
            toolchain_fingerprint, projection_digest, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT(workspace_id) DO UPDATE SET
            content_digest = excluded.content_digest,
            schema_version = excluded.schema_version,
            entrypoints_json = excluded.entrypoints_json,
            decodal_version = excluded.decodal_version,
            import_policy_version = excluded.import_policy_version,
            schema_bundle_json = excluded.schema_bundle_json,
            toolchain_fingerprint = excluded.toolchain_fingerprint,
            projection_digest = excluded.projection_digest,
            updated_at = excluded.updated_at",
        params![
            workspace_id,
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
    // Preserve every distinct evaluation's provenance; only an identical
    // source/toolchain/projection tuple is deduplicated.
    tx.execute(
        "INSERT INTO workspace_config_tree_history (
            workspace_id, content_digest, toolchain_fingerprint,
            schema_bundle_json, projection_digest, manifest_json, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(workspace_id, content_digest, toolchain_fingerprint, projection_digest) DO NOTHING",
        params![
            workspace_id,
            state.snapshot.digest,
            state.contract.fingerprint,
            schema_bundle_json,
            state.projection_digest,
            manifest_json,
            materialized_at,
        ],
    )?;
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
            owner_account_id: "owner-account".to_string(),
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
            base_digest: current.snapshot.digest.clone(),
            changes,
            entrypoints: vec![path(MAIN_CONFIG_ENTRYPOINT)],
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

    fn semantic_candidate(
        contribution: ConfigSchemaContribution,
        source: &str,
    ) -> EvaluatedConfigCandidate {
        let schema = WorkspaceConfigSchemaBundle::compose([
            crate::prompt_settings::PromptConfigSchemaProvider
                .contribution()
                .unwrap(),
            contribution,
        ])
        .unwrap();
        let current = initial_state_with_schema(schema.clone()).unwrap();
        let change = update_main(&current, source);
        let identity = (
            current.contract.fingerprint.clone(),
            current.projection_digest.clone(),
        );
        evaluate_candidate(current, identity, &[change], schema).unwrap()
    }

    #[test]
    fn candidate_semantics_reject_unknown_default_profile() {
        let candidate = semantic_candidate(
            crate::profile_settings::ProfileConfigSchemaProvider
                .contribution()
                .unwrap(),
            r#"{ profile = { default_profile = "project:missing"; }; }"#,
        );
        let error =
            validate_workspace_config_candidate_projections("w-config", &candidate).unwrap_err();
        assert!(error.to_string().contains("unknown_default_profile"));
        let valid = semantic_candidate(
            crate::profile_settings::ProfileConfigSchemaProvider
                .contribution()
                .unwrap(),
            "{}",
        );
        validate_workspace_config_candidate_projections("w-config", &valid).unwrap();
    }

    #[test]
    fn candidate_semantics_reject_invalid_runtime_identifier() {
        let candidate = semantic_candidate(
            crate::runtime_settings::RuntimeConfigSchemaProvider
                .contribution()
                .unwrap(),
            r#"{ runtime = { default_runtime_id = "bad\nidentifier"; }; }"#,
        );
        assert!(matches!(
            validate_workspace_config_candidate_projections("w-config", &candidate),
            Err(Error::InvalidRuntimeIdentifier { .. })
        ));
        let valid = semantic_candidate(
            crate::runtime_settings::RuntimeConfigSchemaProvider
                .contribution()
                .unwrap(),
            "{}",
        );
        validate_workspace_config_candidate_projections("w-config", &valid).unwrap();
    }

    #[test]
    fn candidate_semantics_reject_skill_without_canonical_source() {
        let candidate = semantic_candidate(
            crate::skills::SkillConfigSchemaProvider
                .contribution()
                .unwrap(),
            r#"{
                skills = {
                    debug_rust = {
                        frontmatter = { name = "debug-rust"; description = "Debug Rust"; };
                        content = "inline";
                    };
                };
            }"#,
        );
        let error =
            validate_workspace_config_candidate_projections("w-config", &candidate).unwrap_err();
        assert!(error.to_string().contains("blocking diagnostics"));
        // Do not copy Skill source bodies or diagnostic paths into this error.
        assert!(!error.to_string().contains("inline"));
        assert!(!error.to_string().contains("SKILL.md"));
        let valid = semantic_candidate(
            crate::skills::SkillConfigSchemaProvider
                .contribution()
                .unwrap(),
            "{}",
        );
        validate_workspace_config_candidate_projections("w-config", &valid).unwrap();
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
                    base_digest: current.snapshot.digest.clone(),
                    changes: vec![ConfigTreeChange::Update {
                        path: path(MAIN_CONFIG_ENTRYPOINT),
                        expected_digest: main.content_digest.clone(),
                        content: "{ web = {}; }".to_string(),
                    }],
                    entrypoints: vec![path(MAIN_CONFIG_ENTRYPOINT)],
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

    #[test]
    fn active_state_evaluation_rejects_provider_fingerprint_drift() {
        let snapshot = ConfigTreeSnapshot::from_entries([ConfigEntry::new(
            path(MAIN_CONFIG_ENTRYPOINT),
            ConfigContentType::Decodal,
            "{}",
        )
        .unwrap()])
        .unwrap();
        let persisted_bundle =
            WorkspaceConfigSchemaBundle::compose([ConfigSchemaContribution::new(
                "builtin:test",
                "test",
                "1",
                r#"{ test = { value = String default "one"; }; }"#,
            )
            .unwrap()])
            .unwrap();
        let state = WorkspaceConfigState {
            projection_digest: "persisted".to_string(),
            contract: main_config_contract_with_schema(persisted_bundle),
            snapshot,
        };
        let changed_bundle = WorkspaceConfigSchemaBundle::compose([ConfigSchemaContribution::new(
            "builtin:test",
            "test",
            "2",
            r#"{ test = { value = String default "two"; }; }"#,
        )
        .unwrap()])
        .unwrap();
        let error = evaluate_workspace_config_state(&state, changed_bundle).unwrap_err();
        assert!(error.to_string().contains("schema fingerprint"));
    }

    #[tokio::test]
    async fn initial_materialization_persists_composed_schema_contract() {
        let store = open_store().await;
        let schema_bundle = WorkspaceConfigSchemaBundle::compose([ConfigSchemaContribution::new(
            "builtin:test",
            "test",
            "1",
            r#"{ test = { value = String default "initial"; }; }"#,
        )
        .unwrap()])
        .unwrap();
        let expected = initial_state_with_schema(schema_bundle.clone()).unwrap();
        let state = store
            .ensure_workspace_config_materialized_with_schema(
                "w-config",
                "2026-08-13T00:00:00Z",
                schema_bundle,
            )
            .unwrap();
        assert_eq!(state.contract.fingerprint, expected.contract.fingerprint);
        assert_eq!(
            state.contract.schema_bundle,
            expected.contract.schema_bundle
        );
        assert_eq!(state.projection_digest, expected.projection_digest);
        let reloaded = store.load_workspace_config("w-config").unwrap().unwrap();
        assert_eq!(reloaded.contract, state.contract);
        assert_eq!(reloaded.projection_digest, state.projection_digest);
    }

    #[tokio::test]
    async fn toolchain_upgrade_re_evaluates_current_tree_and_preserves_prior_content_history() {
        let store = open_store().await;
        let schema_bundle = WorkspaceConfigSchemaBundle::compose([ConfigSchemaContribution::new(
            "builtin:test",
            "test",
            "1",
            r#"{ test = { value = String default "initial"; }; }"#,
        )
        .unwrap()])
        .unwrap();
        let current = store
            .ensure_workspace_config_materialized_with_schema(
                "w-config",
                "2026-08-13T00:00:00Z",
                schema_bundle.clone(),
            )
            .unwrap();
        store
            .with_conn(|conn| {
                conn.execute(
                    "UPDATE workspace_config_trees
                     SET decodal_version = '0.2.0', toolchain_fingerprint = 'sha256:legacy'
                     WHERE workspace_id = 'w-config'",
                    [],
                )?;
                conn.execute(
                    "UPDATE workspace_config_tree_history
                     SET toolchain_fingerprint = 'sha256:legacy'
                     WHERE workspace_id = 'w-config' AND content_digest = ?1",
                    [current.snapshot.digest.as_str()],
                )?;
                Ok(())
            })
            .unwrap();

        let before_history = history_count(&store);
        let refreshed = store
            .ensure_workspace_config_materialized_with_schema(
                "w-config",
                "2026-08-14T00:00:00Z",
                schema_bundle,
            )
            .unwrap();
        assert_eq!(refreshed.snapshot, current.snapshot);
        assert_eq!(refreshed.snapshot.digest, current.snapshot.digest);
        assert_eq!(refreshed.contract.decodal_version, DECODAL_VERSION);
        assert_ne!(refreshed.contract.fingerprint, "sha256:legacy");
        let prior = store
            .load_workspace_config_history("w-config", &current.snapshot.digest)
            .unwrap()
            .unwrap();
        assert_eq!(prior, current.snapshot);
        let prior_fingerprint = store
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT toolchain_fingerprint
                     FROM workspace_config_tree_history
                     WHERE workspace_id = 'w-config' AND content_digest = ?1
                       AND toolchain_fingerprint = 'sha256:legacy'",
                    [current.snapshot.digest.as_str()],
                    |row| row.get::<_, String>(0),
                )
                .map_err(Error::from)
            })
            .unwrap();
        assert_eq!(prior_fingerprint, "sha256:legacy");
        assert_eq!(history_count(&store), before_history + 1);
    }

    #[tokio::test]
    async fn schema_refresh_preserves_evaluation_provenance_and_rejects_prior_candidate() {
        let store = open_store().await;
        let initial = WorkspaceConfigSchemaBundle::compose([ConfigSchemaContribution::new(
            "builtin:profile-test",
            "profile",
            "1",
            r#"{ profile = { enabled = Bool default true; }; }"#,
        )
        .unwrap()])
        .unwrap();
        let current = store
            .ensure_workspace_config_materialized_with_schema(
                "w-config",
                "2026-08-13T00:00:00Z",
                initial,
            )
            .unwrap();
        let stale_candidate = store
            .evaluate_workspace_config_candidate_with_schema(
                "w-config",
                &commit_request(&current, vec![]),
                current.contract.schema_bundle.clone(),
            )
            .unwrap();
        let before_history = history_count(&store);
        let extended = WorkspaceConfigSchemaBundle::compose([
            ConfigSchemaContribution::new(
                "builtin:profile-test",
                "profile",
                "1",
                r#"{ profile = { enabled = Bool default true; }; }"#,
            )
            .unwrap(),
            ConfigSchemaContribution::new(
                "builtin:skill-test",
                "skills",
                "1",
                r#"{ skills = {...String} default {}; }"#,
            )
            .unwrap(),
        ])
        .unwrap();

        let refreshed = store
            .ensure_workspace_config_materialized_with_schema(
                "w-config",
                "2026-08-14T00:00:00Z",
                extended.clone(),
            )
            .unwrap();
        assert_eq!(refreshed.snapshot, current.snapshot);
        assert_eq!(refreshed.snapshot.digest, current.snapshot.digest);
        assert_eq!(refreshed.contract.schema_bundle, extended);
        assert!(matches!(
            store.commit_evaluated_workspace_config("w-config", &stale_candidate),
            Err(Error::WorkspaceConfigConflict(_))
        ));
        assert_eq!(
            store.load_workspace_config("w-config").unwrap().unwrap(),
            refreshed
        );
        assert_eq!(history_count(&store), before_history + 1);
        for state in [&current, &refreshed] {
            let saved: (String, String) = store.with_conn(|conn| {
                conn.query_row(
                    "SELECT schema_bundle_json, manifest_json FROM workspace_config_tree_history
                     WHERE workspace_id=?1 AND content_digest=?2 AND toolchain_fingerprint=?3 AND projection_digest=?4",
                    params!["w-config", state.snapshot.digest, state.contract.fingerprint, state.projection_digest],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                ).map_err(Error::from)
            }).unwrap();
            assert_eq!(
                saved.0,
                serde_json::to_string(&state.contract.schema_bundle).unwrap()
            );
            assert_eq!(
                saved.1,
                serde_json::to_string(&current.snapshot.entries).unwrap()
            );
        }
        assert_ne!(
            refreshed.projection_digest, current.projection_digest,
            "the newly defaulted namespace changes the evaluated projection"
        );
        assert!(
            store
                .load_workspace_config_history("w-config", &current.snapshot.digest)
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn invalid_toolchain_upgrade_returns_diagnostics_without_mutating_authority() {
        let store = open_store().await;
        let schema_bundle = WorkspaceConfigSchemaBundle::compose([ConfigSchemaContribution::new(
            "builtin:test",
            "test",
            "1",
            r#"{ test = { value = String default "initial"; }; }"#,
        )
        .unwrap()])
        .unwrap();
        let current = store
            .ensure_workspace_config_materialized_with_schema(
                "w-config",
                "2026-08-13T00:00:00Z",
                schema_bundle.clone(),
            )
            .unwrap();
        let legacy_entry = ConfigEntry::new(
            path(MAIN_CONFIG_ENTRYPOINT),
            ConfigContentType::Decodal,
            "{ test = {}; custom = 42; }\n",
        )
        .unwrap();
        let legacy_snapshot = ConfigTreeSnapshot::from_entries([legacy_entry.clone()]).unwrap();
        let manifest_json = serde_json::to_string(&legacy_snapshot.entries).unwrap();
        store
            .with_conn(|conn| {
                conn.execute(
                    "UPDATE workspace_config_entries
                     SET content = ?1, content_digest = ?2
                     WHERE workspace_id = 'w-config' AND path = 'main.dcdl'",
                    rusqlite::params![legacy_entry.content, legacy_entry.content_digest],
                )?;
                conn.execute(
                    "UPDATE workspace_config_trees
                     SET content_digest = ?1, decodal_version = '0.2.0',
                         toolchain_fingerprint = 'sha256:legacy',
                         projection_digest = 'sha256:legacy-projection'
                     WHERE workspace_id = 'w-config'",
                    [legacy_snapshot.digest.as_str()],
                )?;
                conn.execute(
                    "UPDATE workspace_config_tree_history
                     SET content_digest = ?1, toolchain_fingerprint = 'sha256:legacy',
                         projection_digest = 'sha256:legacy-projection', manifest_json = ?2
                     WHERE workspace_id = 'w-config' AND content_digest = ?3
                         AND toolchain_fingerprint = ?4 AND projection_digest = ?5",
                    rusqlite::params![
                        legacy_snapshot.digest,
                        manifest_json,
                        current.snapshot.digest,
                        current.contract.fingerprint,
                        current.projection_digest
                    ],
                )?;
                Ok(())
            })
            .unwrap();

        let error = store
            .ensure_workspace_config_materialized_with_schema(
                "w-config",
                "2026-08-14T00:00:00Z",
                schema_bundle,
            )
            .unwrap_err();
        let message = error.to_string();
        assert!(message.contains("Decodal 0.4.0"));
        assert!(message.contains("main.dcdl"));
        assert!(message.contains("constraintviolation"));
        let persisted = store
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT content_digest, decodal_version, toolchain_fingerprint
                     FROM workspace_config_trees WHERE workspace_id = 'w-config'",
                    [],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    },
                )
                .map_err(Error::from)
            })
            .unwrap();
        assert_eq!(
            persisted,
            (
                legacy_snapshot.digest,
                "0.2.0".into(),
                "sha256:legacy".into()
            )
        );
    }

    #[tokio::test]
    async fn workspace_materializes_main_entrypoint() {
        let store = open_store().await;
        let current = store.load_workspace_config("w-config").unwrap().unwrap();
        assert!(!current.snapshot.digest.is_empty());
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
        assert!(main_config_has_schema_assertion(DEFAULT_MAIN_CONFIG_SOURCE));
    }

    #[tokio::test]
    async fn candidate_normalizes_main_and_formats_changed_decodal_sources() {
        let store = open_store().await;
        let current = store.load_workspace_config("w-config").unwrap().unwrap();
        let candidate = store
            .evaluate_workspace_config_candidate(
                "w-config",
                &commit_request(
                    &current,
                    vec![
                        update_main(&current, "{}"),
                        ConfigTreeChange::Create {
                            path: path("module.dcdl"),
                            content_type: ConfigContentType::Decodal,
                            content: "{}".into(),
                        },
                    ],
                ),
            )
            .unwrap();

        assert_eq!(
            candidate
                .snapshot
                .get(&path(MAIN_CONFIG_ENTRYPOINT))
                .unwrap()
                .content,
            DEFAULT_MAIN_CONFIG_SOURCE
        );
        assert_eq!(
            candidate
                .snapshot
                .get(&path("module.dcdl"))
                .unwrap()
                .content,
            "{}\n"
        );
    }

    #[tokio::test]
    async fn materialization_upgrades_legacy_main_and_preserves_prior_content_history() {
        let store = open_store().await;
        let current = store.load_workspace_config("w-config").unwrap().unwrap();
        let legacy_snapshot = ConfigTreeSnapshot::from_entries([ConfigEntry::new(
            path(MAIN_CONFIG_ENTRYPOINT),
            ConfigContentType::Decodal,
            "{}",
        )
        .unwrap()])
        .unwrap();
        let legacy = WorkspaceConfigState {
            projection_digest: current.projection_digest.clone(),
            snapshot: legacy_snapshot,
            contract: current.contract.clone(),
        };
        store
            .with_conn_mut(|conn| {
                conn.execute(
                    "DELETE FROM workspace_config_entries WHERE workspace_id = ?1",
                    params!["w-config"],
                )?;
                insert_materialized_state(conn, "w-config", &legacy, "2026-08-14T00:00:00Z")
            })
            .unwrap();

        let upgraded = store
            .ensure_workspace_config_materialized_with_schema(
                "w-config",
                "2026-08-14T00:00:01Z",
                current.contract.schema_bundle.clone(),
            )
            .unwrap();
        assert_ne!(upgraded.snapshot.digest, legacy.snapshot.digest);
        assert_eq!(
            upgraded
                .snapshot
                .get(&path(MAIN_CONFIG_ENTRYPOINT))
                .unwrap()
                .content,
            DEFAULT_MAIN_CONFIG_SOURCE
        );
        assert_eq!(
            store
                .load_workspace_config_history("w-config", &legacy.snapshot.digest)
                .unwrap()
                .unwrap()
                .get(&path(MAIN_CONFIG_ENTRYPOINT))
                .unwrap()
                .content,
            "{}"
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
                .evaluate_workspace_config_candidate(
                    "w-config",
                    &commit_request(&current, vec![change]),
                )
                .unwrap_err();
            assert!(error.to_string().contains("cannot be"));
        }
    }

    #[tokio::test]
    async fn browser_cannot_replace_server_owned_entrypoint_contract() {
        let store = open_store().await;
        let current = store.load_workspace_config("w-config").unwrap().unwrap();
        let mut request = commit_request(&current, Vec::new());
        request.entrypoints = vec![path("other.dcdl")];
        let error = store
            .evaluate_workspace_config_candidate("w-config", &request)
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
                    base_digest: current.snapshot.digest.clone(),
                    changes: vec![ConfigTreeChange::Update {
                        path: path(MAIN_CONFIG_ENTRYPOINT),
                        expected_digest: main.content_digest.clone(),
                        content: "{ broken = ; }".into(),
                    }],
                    entrypoints: vec![path(MAIN_CONFIG_ENTRYPOINT)],
                },
            )
            .unwrap_err();
        assert!(matches!(error, Error::InvalidInput(_)));
        assert!(store.load_workspace_config("w-config").unwrap().is_some());
    }

    #[tokio::test]
    async fn valid_candidate_commits_snapshot_content_and_provenance_atomically() {
        let store = SqliteWorkspaceStore::in_memory().unwrap();
        store.upsert_workspace(&workspace()).await.unwrap();
        let current = store.load_workspace_config("w-config").unwrap().unwrap();
        let committed = store
            .evaluate_and_commit_workspace_config(
                "w-config",
                &commit_request(&current, vec![update_main(&current, "{ answer = 42; }")]),
            )
            .unwrap();
        assert_ne!(committed.snapshot.digest, current.snapshot.digest);
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
    async fn committed_content_remains_retrievable_after_later_commit() {
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
                    base_digest: first.snapshot.digest.clone(),
                    changes: vec![ConfigTreeChange::Update {
                        path: path(MAIN_CONFIG_ENTRYPOINT),
                        expected_digest: entry.content_digest.clone(),
                        content: "{ answer = 2; }".into(),
                    }],
                    entrypoints: first.contract.entrypoints.clone(),
                },
            )
            .unwrap();
        let history = store
            .load_workspace_config_history("w-config", &first.snapshot.digest)
            .unwrap()
            .unwrap();
        assert_eq!(history, first.snapshot);
    }

    fn history_count(store: &SqliteWorkspaceStore) -> i64 {
        store
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT COUNT(*) FROM workspace_config_tree_history WHERE workspace_id = ?1",
                    ["w-config"],
                    |row| row.get(0),
                )
                .map_err(Error::from)
            })
            .unwrap()
    }

    #[tokio::test]
    async fn identical_content_commit_reuses_immutable_history() {
        let store = open_store().await;
        let current = store.load_workspace_config("w-config").unwrap().unwrap();
        let before = history_count(&store);
        let original_history: (String, String) = store
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT manifest_json, created_at FROM workspace_config_tree_history
                WHERE workspace_id = ?1 AND content_digest = ?2",
                    params!["w-config", current.snapshot.digest],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(Error::from)
            })
            .unwrap();
        let request = commit_request(
            &current,
            vec![update_main(&current, DEFAULT_MAIN_CONFIG_SOURCE)],
        );
        let first = store
            .evaluate_and_commit_workspace_config("w-config", &request)
            .unwrap();
        let repeated = store
            .evaluate_and_commit_workspace_config("w-config", &request)
            .unwrap();
        assert_eq!(first.snapshot, current.snapshot);
        assert_eq!(repeated.snapshot, current.snapshot);
        assert_eq!(history_count(&store), before);
        let saved_history: (String, String) = store
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT manifest_json, created_at FROM workspace_config_tree_history
                WHERE workspace_id = ?1 AND content_digest = ?2",
                    params!["w-config", current.snapshot.digest],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(Error::from)
            })
            .unwrap();
        assert_eq!(saved_history, original_history);
    }

    #[tokio::test]
    async fn returning_to_prior_content_reuses_digest_history() {
        let store = open_store().await;
        let initial = store.load_workspace_config("w-config").unwrap().unwrap();
        let before = history_count(&store);
        let changed = store
            .evaluate_and_commit_workspace_config(
                "w-config",
                &commit_request(
                    &initial,
                    vec![ConfigTreeChange::Create {
                        path: path("notes.txt"),
                        content_type: ConfigContentType::Text,
                        content: "temporary".into(),
                    }],
                ),
            )
            .unwrap();
        assert_ne!(changed.snapshot.digest, initial.snapshot.digest);
        let entry = changed.snapshot.get(&path("notes.txt")).unwrap();
        let returned = store
            .evaluate_and_commit_workspace_config(
                "w-config",
                &commit_request(
                    &changed,
                    vec![ConfigTreeChange::Delete {
                        path: entry.path.clone(),
                        expected_digest: entry.content_digest.clone(),
                    }],
                ),
            )
            .unwrap();
        assert_eq!(returned.snapshot, initial.snapshot);
        assert_eq!(history_count(&store), before + 1);
        assert_eq!(
            store
                .load_workspace_config_history("w-config", &changed.snapshot.digest)
                .unwrap()
                .unwrap(),
            changed.snapshot
        );
        assert_eq!(
            store
                .load_workspace_config_history("w-config", &initial.snapshot.digest)
                .unwrap()
                .unwrap(),
            initial.snapshot
        );
        assert!(
            store
                .load_workspace_config_history("another-workspace", &initial.snapshot.digest)
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn candidate_from_prior_projection_is_rejected_without_replacing_current_state() {
        let store = open_store().await;
        let current = store.load_workspace_config("w-config").unwrap().unwrap();
        let candidate = store
            .evaluate_workspace_config_candidate("w-config", &commit_request(&current, vec![]))
            .unwrap();
        store.with_conn(|conn| {
            conn.execute("UPDATE workspace_config_trees SET projection_digest='new-projection' WHERE workspace_id='w-config'", [])?;
            Ok(())
        }).unwrap();
        assert!(matches!(
            store.commit_evaluated_workspace_config("w-config", &candidate),
            Err(Error::WorkspaceConfigConflict(_))
        ));
        assert_eq!(
            store
                .load_workspace_config("w-config")
                .unwrap()
                .unwrap()
                .projection_digest,
            "new-projection"
        );
    }

    #[tokio::test]
    async fn incorrect_base_digest_is_rejected_before_evaluation() {
        let store = open_store().await;
        let current = store.load_workspace_config("w-config").unwrap().unwrap();
        let mut request =
            commit_request(&current, vec![update_main(&current, "not valid Decodal")]);
        request.base_digest = "sha256:incorrect".into();
        let error = store
            .evaluate_workspace_config_candidate("w-config", &request)
            .unwrap_err();
        assert!(matches!(error, Error::WorkspaceConfigConflict(_)));
        assert_eq!(
            store.load_workspace_config("w-config").unwrap().unwrap(),
            current
        );
    }

    #[tokio::test]
    async fn history_content_and_manifest_paths_are_verified() {
        let store = open_store().await;
        let current = store.load_workspace_config("w-config").unwrap().unwrap();
        assert!(
            store
                .load_workspace_config_history("w-config", "sha256:missing")
                .unwrap()
                .is_none()
        );
        let original_manifest = serde_json::to_string(&current.snapshot.entries).unwrap();
        // A syntactically valid manifest with different complete content cannot
        // satisfy the requested digest.
        store.with_conn(|conn| {
            conn.execute("UPDATE workspace_config_tree_history SET manifest_json = '{}' WHERE workspace_id = ?1",
                ["w-config"])?;
            Ok(())
        }).unwrap();
        assert!(matches!(
            store.load_workspace_config_history("w-config", &current.snapshot.digest),
            Err(Error::RegistryInconsistency(_))
        ));
        let mut manifest: serde_json::Value = serde_json::from_str(&original_manifest).unwrap();
        let entry = manifest
            .as_object_mut()
            .unwrap()
            .remove(MAIN_CONFIG_ENTRYPOINT)
            .unwrap();
        manifest
            .as_object_mut()
            .unwrap()
            .insert("aliased.dcdl".into(), entry);
        store.with_conn(|conn| {
            conn.execute("UPDATE workspace_config_tree_history SET manifest_json = ?1 WHERE workspace_id = ?2",
                params![manifest.to_string(), "w-config"])?;
            Ok(())
        }).unwrap();
        assert!(matches!(
            store.load_workspace_config_history("w-config", &current.snapshot.digest),
            Err(Error::RegistryInconsistency(_))
        ));
        assert_eq!(
            store.load_workspace_config("w-config").unwrap().unwrap(),
            current
        );
    }

    #[test]
    fn config_commit_request_requires_only_content_identity() {
        let request: ConfigCommitRequest = serde_json::from_value(serde_json::json!({
            "base_digest": "sha256:source", "changes": [], "entrypoints": [MAIN_CONFIG_ENTRYPOINT],
        }))
        .unwrap();
        assert_eq!(request.base_digest, "sha256:source");
        let mut unexpected = serde_json::to_value(&request).unwrap();
        unexpected
            .as_object_mut()
            .unwrap()
            .insert("unexpected_counter".into(), 7.into());
        assert!(serde_json::from_value::<ConfigCommitRequest>(unexpected).is_err());
    }

    #[test]
    fn exports_typescript_transport_contract() {
        use ts_rs::TS;
        let output = tempfile::tempdir().unwrap();
        let config = ts_rs::Config::default().with_out_dir(output.path());
        WorkspaceConfigState::export_all(&config).unwrap();
        ConfigCommitRequest::export_all(&config).unwrap();
    }

    #[test]
    fn migration_creates_config_authority_tables() {
        let store = SqliteWorkspaceStore::in_memory().unwrap();
        store
            .with_conn(|conn| {
                for table in [
                    "workspace_config_trees",
                    "workspace_config_entries",
                    "workspace_config_tree_history",
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
