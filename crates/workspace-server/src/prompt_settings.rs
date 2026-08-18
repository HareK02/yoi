use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};

use config_source::{ConfigProjectionValidator, ConfigSchemaContribution};
use worker::{EffectivePromptCatalog, WorkspacePromptProjection, prompt_schema_source};

use crate::config_source::{
    WorkspaceConfigSchemaProvider, WorkspaceConfigState, evaluate_workspace_config_state,
};
use crate::{Error, Result};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct PromptProjectionCacheKey {
    workspace_id: String,
    config_revision: u64,
    source_digest: String,
    projection_digest: String,
    schema_fingerprint: String,
    toolchain_fingerprint: String,
}

impl PromptProjectionCacheKey {
    fn new(workspace_id: &str, state: &WorkspaceConfigState) -> Self {
        Self {
            workspace_id: workspace_id.to_string(),
            config_revision: state.snapshot.revision,
            source_digest: state.snapshot.digest.clone(),
            projection_digest: state.projection_digest.clone(),
            schema_fingerprint: state.contract.schema_bundle.fingerprint.clone(),
            toolchain_fingerprint: state.contract.fingerprint.clone(),
        }
    }
}

type PromptProjectionCell = OnceLock<std::result::Result<Arc<WorkspacePromptProjection>, String>>;

#[derive(Debug, Default)]
struct PromptProjectionCacheState {
    entries: BTreeMap<PromptProjectionCacheKey, Arc<PromptProjectionCell>>,
    active: BTreeMap<String, PromptProjectionCacheKey>,
}

/// WorkspaceApi-shared immutable Prompt projections keyed by authoritative Workspace config
/// identity.
///
/// This cache is an evaluation optimization only. Callers must load the active
/// [`WorkspaceConfigState`] from Server DB authority before resolving an entry. Advancing a
/// Workspace replaces only its active cache entry; in-flight users retain their immutable `Arc`.
#[derive(Debug, Clone, Default)]
pub struct WorkspacePromptProjectionCache {
    inner: Arc<Mutex<PromptProjectionCacheState>>,
}

impl WorkspacePromptProjectionCache {
    pub fn resolve(
        &self,
        workspace_id: &str,
        state: &WorkspaceConfigState,
    ) -> Result<Arc<WorkspacePromptProjection>> {
        let key = PromptProjectionCacheKey::new(workspace_id, state);
        let (cell, cached) = {
            let mut cache = self.lock()?;
            if let Some(active) = cache.active.get(workspace_id) {
                if key.config_revision == active.config_revision && key != *active {
                    return Err(Error::RegistryInconsistency(format!(
                        "Workspace Prompt projection identity changed without a config revision transition: workspace={workspace_id} revision={}",
                        key.config_revision
                    )));
                }
                if key.config_revision < active.config_revision {
                    (Arc::new(PromptProjectionCell::new()), false)
                } else {
                    let cell = cache
                        .entries
                        .entry(key.clone())
                        .or_insert_with(|| Arc::new(PromptProjectionCell::new()))
                        .clone();
                    (cell, true)
                }
            } else {
                let cell = cache
                    .entries
                    .entry(key.clone())
                    .or_insert_with(|| Arc::new(PromptProjectionCell::new()))
                    .clone();
                (cell, true)
            }
        };

        let resolved = cell
            .get_or_init(|| {
                project_workspace_prompt_projection(workspace_id, state)
                    .map(Arc::new)
                    .map_err(|error| error.to_string())
            })
            .clone();
        let catalog = match resolved {
            Ok(catalog) => catalog,
            Err(error) => {
                if cached {
                    self.lock()?.entries.remove(&key);
                }
                return Err(Error::Config(error));
            }
        };

        if cached {
            self.record_resolved(workspace_id, &key, &cell)?;
        }
        Ok(catalog)
    }

    fn record_resolved(
        &self,
        workspace_id: &str,
        key: &PromptProjectionCacheKey,
        cell: &Arc<PromptProjectionCell>,
    ) -> Result<()> {
        let mut cache = self.lock()?;
        let active = cache.active.get(workspace_id).cloned();
        match active {
            Some(active) if active.config_revision > key.config_revision => {
                cache.entries.remove(key);
            }
            Some(active) if active.config_revision == key.config_revision => {
                if active != *key {
                    cache.entries.remove(key);
                    return Err(Error::RegistryInconsistency(format!(
                        "Workspace Prompt projection identity changed without a config revision transition: workspace={workspace_id} revision={}",
                        key.config_revision
                    )));
                }
                cache.entries.entry(key.clone()).or_insert(cell.clone());
            }
            _ => {
                cache.entries.insert(key.clone(), cell.clone());
                cache.active.insert(workspace_id.to_string(), key.clone());
                cache.entries.retain(|existing, _| {
                    existing.workspace_id != workspace_id
                        || existing == key
                        || existing.config_revision > key.config_revision
                });
            }
        }
        Ok(())
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, PromptProjectionCacheState>> {
        self.inner.lock().map_err(|_| {
            Error::RegistryInconsistency("Prompt projection cache lock was poisoned".to_string())
        })
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.lock().expect("cache lock").entries.len()
    }
}

#[derive(Debug, Default)]
pub struct PromptConfigSchemaProvider;

impl WorkspaceConfigSchemaProvider for PromptConfigSchemaProvider {
    fn contribution(&self) -> Result<ConfigSchemaContribution> {
        ConfigSchemaContribution::new(
            "builtin:prompts",
            "prompts",
            "1",
            prompt_schema_source().map_err(|error| Error::Config(error.to_string()))?,
        )
        .map(|contribution| {
            contribution.with_projection_validator(
                ConfigProjectionValidator::StaticTemplateCatalog {
                    namespace: "prompts".to_string(),
                    key_aliases: std::collections::BTreeMap::from([(
                        "default_prompt".to_string(),
                        "default".to_string(),
                    )]),
                },
            )
        })
        .map_err(|error| Error::Config(error.to_string()))
    }
}

pub fn validate_evaluated_prompt_catalog(
    evaluation: &config_source::EvaluationResult,
) -> Result<()> {
    let projection = evaluation.projections.first().ok_or_else(|| {
        Error::InvalidInput("Workspace config produced no active projection".to_string())
    })?;
    let prompts = projection.data_json.get("prompts").ok_or_else(|| {
        Error::InvalidInput("Workspace config projection has no prompts namespace".to_string())
    })?;
    EffectivePromptCatalog::from_projection(prompts, 0, "preview", "preview")
        .map(|_| ())
        .map_err(|error| Error::InvalidInput(format!("invalid Prompt catalog: {error}")))
}

pub fn project_prompts_from_workspace_config(
    state: &WorkspaceConfigState,
) -> Result<EffectivePromptCatalog> {
    let evaluation = evaluate_workspace_config_state(state, state.contract.schema_bundle.clone())?;
    if evaluation.projection_digest != state.projection_digest {
        return Err(Error::RegistryInconsistency(
            "Prompt projection digest does not match the active Workspace config revision"
                .to_string(),
        ));
    }
    let projection = evaluation.projections.first().ok_or_else(|| {
        Error::RegistryInconsistency("Workspace config has no active projection".to_string())
    })?;
    let prompts = projection.data_json.get("prompts").ok_or_else(|| {
        Error::RegistryInconsistency(
            "active Workspace config projection has no prompts namespace".to_string(),
        )
    })?;
    let mut catalog = EffectivePromptCatalog::from_projection(
        prompts,
        state.snapshot.revision,
        state.contract.schema_bundle.fingerprint.clone(),
        state.contract.fingerprint.clone(),
    )
    .map_err(|error| Error::RegistryInconsistency(error.to_string()))?;
    catalog.source_digest = state.snapshot.digest.clone();
    Ok(catalog)
}

pub fn project_workspace_prompt_projection(
    workspace_id: &str,
    state: &WorkspaceConfigState,
) -> Result<WorkspacePromptProjection> {
    let catalog = project_prompts_from_workspace_config(state)?;
    WorkspacePromptProjection::new(
        workspace_id,
        state.snapshot.digest.clone(),
        catalog.catalog_digest.clone(),
        catalog,
    )
    .map_err(|error| Error::RegistryInconsistency(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use config_source::{
        ConfigContentType, ConfigEntry, ConfigTreeSnapshot, SnapshotEnvironment, ToolchainContract,
        VirtualPath, WorkspaceConfigSchemaBundle,
    };

    fn state(source: &str) -> WorkspaceConfigState {
        state_at(7, source)
    }

    fn state_at(revision: u64, source: &str) -> WorkspaceConfigState {
        let schema = WorkspaceConfigSchemaBundle::compose([PromptConfigSchemaProvider
            .contribution()
            .unwrap()])
        .unwrap();
        let snapshot = ConfigTreeSnapshot::from_entries(
            revision,
            [ConfigEntry::new(
                VirtualPath::parse("main.dcdl").unwrap(),
                ConfigContentType::Decodal,
                source,
            )
            .unwrap()],
        )
        .unwrap();
        let contract = ToolchainContract::with_schema_bundle(
            config_source::DEFAULT_SCHEMA_VERSION,
            vec![VirtualPath::parse("main.dcdl").unwrap()],
            config_source::DEFAULT_IMPORT_POLICY_VERSION,
            schema,
        );
        let projection_digest = SnapshotEnvironment::new(snapshot.clone())
            .evaluate_contract(&contract)
            .unwrap()
            .projection_digest;
        WorkspaceConfigState {
            snapshot,
            contract,
            projection_digest,
        }
    }

    #[test]
    fn prompt_projection_cache_shares_immutable_entry_and_replaces_workspace_revision() {
        let cache = WorkspacePromptProjectionCache::default();
        let initial = state("{}");
        let first = cache.resolve("workspace-a", &initial).unwrap();
        let retry = cache.resolve("workspace-a", &initial).unwrap();
        assert!(Arc::ptr_eq(&first, &retry));
        assert_eq!(cache.len(), 1);

        let updated = state_at(
            8,
            r#"{ prompts = { common = { language = "UPDATED"; }; }; }"#,
        );
        let replacement = cache.resolve("workspace-a", &updated).unwrap();
        assert!(!Arc::ptr_eq(&first, &replacement));
        assert_eq!(
            replacement.catalog().templates["common.language"],
            "UPDATED"
        );
        assert_eq!(cache.len(), 1);
        assert_ne!(first.catalog().templates["common.language"], "UPDATED");

        let other_workspace = cache.resolve("workspace-b", &updated).unwrap();
        assert!(!Arc::ptr_eq(&replacement, &other_workspace));
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn prompt_projection_cache_does_not_let_stale_revision_evict_active_entry() {
        let cache = WorkspacePromptProjectionCache::default();
        let current = state_at(
            8,
            r#"{ prompts = { common = { language = "CURRENT"; }; }; }"#,
        );
        let stale = state_at(7, "{}");

        let current_catalog = cache.resolve("workspace-a", &current).unwrap();
        let stale_catalog = cache.resolve("workspace-a", &stale).unwrap();
        let current_retry = cache.resolve("workspace-a", &current).unwrap();

        assert_eq!(stale_catalog.catalog().config_revision, 7);
        assert!(Arc::ptr_eq(&current_catalog, &current_retry));
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn prompt_projection_cache_rejects_same_revision_reinterpretation() {
        let cache = WorkspacePromptProjectionCache::default();
        let first = state_at(7, "{}");
        let changed = state_at(
            7,
            r#"{ prompts = { common = { language = "CHANGED"; }; }; }"#,
        );

        cache.resolve("workspace-a", &first).unwrap();
        let error = cache.resolve("workspace-a", &changed).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("without a config revision transition")
        );
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn prompt_projection_cache_post_init_rejects_concurrent_same_revision_identity() {
        let cache = WorkspacePromptProjectionCache::default();
        let first = PromptProjectionCacheKey::new("workspace-a", &state_at(7, "{}"));
        let conflicting = PromptProjectionCacheKey::new(
            "workspace-a",
            &state_at(
                7,
                r#"{ prompts = { common = { language = "CONFLICT"; }; }; }"#,
            ),
        );
        let first_cell = Arc::new(PromptProjectionCell::new());
        let conflicting_cell = Arc::new(PromptProjectionCell::new());
        {
            let mut state = cache.lock().unwrap();
            state.entries.insert(first.clone(), first_cell.clone());
            state
                .entries
                .insert(conflicting.clone(), conflicting_cell.clone());
        }

        cache
            .record_resolved("workspace-a", &first, &first_cell)
            .unwrap();
        let error = cache
            .record_resolved("workspace-a", &conflicting, &conflicting_cell)
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("without a config revision transition")
        );
        let state = cache.lock().unwrap();
        assert_eq!(state.active["workspace-a"], first);
        assert!(!state.entries.contains_key(&conflicting));
    }

    #[test]
    fn prompt_projection_cache_keeps_newer_inflight_entry_when_older_finishes_first() {
        let cache = WorkspacePromptProjectionCache::default();
        let older = PromptProjectionCacheKey::new("workspace-a", &state_at(7, "{}"));
        let newer = PromptProjectionCacheKey::new(
            "workspace-a",
            &state_at(8, r#"{ prompts = { common = { language = "NEW"; }; }; }"#),
        );
        let older_cell = Arc::new(PromptProjectionCell::new());
        let newer_cell = Arc::new(PromptProjectionCell::new());
        {
            let mut state = cache.lock().unwrap();
            state.entries.insert(older.clone(), older_cell.clone());
            state.entries.insert(newer.clone(), newer_cell.clone());
        }

        cache
            .record_resolved("workspace-a", &older, &older_cell)
            .unwrap();
        assert!(cache.lock().unwrap().entries.contains_key(&newer));

        cache
            .record_resolved("workspace-a", &newer, &newer_cell)
            .unwrap();
        let state = cache.lock().unwrap();
        assert_eq!(state.active["workspace-a"], newer);
        assert_eq!(state.entries.len(), 1);
        assert!(state.entries.contains_key(&newer));
    }

    #[test]
    fn prompt_projection_cache_single_flights_concurrent_resolve() {
        let cache = WorkspacePromptProjectionCache::default();
        let state = Arc::new(state("{}"));
        let mut threads = Vec::new();
        for _ in 0..8 {
            let cache = cache.clone();
            let state = state.clone();
            threads.push(std::thread::spawn(move || {
                cache.resolve("workspace-a", &state).unwrap()
            }));
        }

        let catalogs = threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect::<Vec<_>>();
        let first = &catalogs[0];
        assert!(catalogs.iter().all(|catalog| Arc::ptr_eq(first, catalog)));
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn workspace_override_deep_patches_builtin_and_preserves_other_leaves() {
        let baseline = project_prompts_from_workspace_config(&state("{}")).unwrap();
        let state = state(r#"{ prompts = { common = { language = "OVERRIDE"; }; }; }"#);
        let catalog = project_prompts_from_workspace_config(&state).unwrap();
        assert_eq!(catalog.config_revision, 7);
        assert_eq!(catalog.templates["common.language"], "OVERRIDE");
        for (key, value) in baseline.templates {
            if key != "common.language" {
                assert_eq!(catalog.templates.get(&key), Some(&value));
            }
        }
    }

    #[test]
    fn preview_commit_validator_rejects_dynamic_missing_and_cyclic_includes() {
        let schema = WorkspaceConfigSchemaBundle::compose([PromptConfigSchemaProvider
            .contribution()
            .unwrap()])
        .unwrap();
        for source in [
            r#"{ prompts = { common = { language = "{%- include target -%}"; }; }; }"#,
            r#"{ prompts = { common = { language = "{%- include \"missing\" -%}"; }; }; }"#,
            r#"{ prompts = { common = { language = "{% include \"common.workspace\" %}"; workspace = "{% include \"common.language\" %}"; }; }; }"#,
        ] {
            let snapshot = ConfigTreeSnapshot::from_entries(
                0,
                [ConfigEntry::new(
                    VirtualPath::parse("main.dcdl").unwrap(),
                    ConfigContentType::Decodal,
                    source,
                )
                .unwrap()],
            )
            .unwrap();
            let contract = ToolchainContract::with_schema_bundle(
                config_source::DEFAULT_SCHEMA_VERSION,
                vec![VirtualPath::parse("main.dcdl").unwrap()],
                config_source::DEFAULT_IMPORT_POLICY_VERSION,
                schema.clone(),
            );
            assert!(
                SnapshotEnvironment::new(snapshot)
                    .evaluate_contract(&contract)
                    .is_err()
            );
        }
    }

    #[test]
    fn closed_prompt_schema_rejects_unknown_and_non_string_leaves() {
        let schema = WorkspaceConfigSchemaBundle::compose([PromptConfigSchemaProvider
            .contribution()
            .unwrap()])
        .unwrap();
        for source in [
            "{ prompts = { common = { unknown = \"bad\"; }; }; }",
            "{ prompts = { common = { language = 42; }; }; }",
        ] {
            let snapshot = ConfigTreeSnapshot::from_entries(
                0,
                [ConfigEntry::new(
                    VirtualPath::parse("main.dcdl").unwrap(),
                    ConfigContentType::Decodal,
                    source,
                )
                .unwrap()],
            )
            .unwrap();
            let contract = ToolchainContract::with_schema_bundle(
                config_source::DEFAULT_SCHEMA_VERSION,
                vec![VirtualPath::parse("main.dcdl").unwrap()],
                config_source::DEFAULT_IMPORT_POLICY_VERSION,
                schema.clone(),
            );
            assert!(
                SnapshotEnvironment::new(snapshot)
                    .evaluate_contract(&contract)
                    .is_err()
            );
        }
    }
}
