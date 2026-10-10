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
    source_digest: String,
    projection_digest: String,
    schema_fingerprint: String,
    toolchain_fingerprint: String,
}

impl PromptProjectionCacheKey {
    fn new(workspace_id: &str, state: &WorkspaceConfigState) -> Self {
        Self {
            workspace_id: workspace_id.to_string(),
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
}

/// WorkspaceApi-shared immutable Prompt projections keyed by authoritative Workspace config
/// identity.
///
/// This cache is an evaluation optimization only. Callers must load the active
/// [`WorkspaceConfigState`] from Server DB authority before resolving an entry.
/// Content and evaluation contracts are distinct keys; in-flight users retain their
/// immutable `Arc`. No ordering or freshness is inferred from a digest.
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
        let cell = {
            let mut cache = self.lock()?;
            // Bound retained cells without treating any content identity as newer.
            // In-flight evaluation cells are never evicted, preserving single flight.
            if !cache.entries.contains_key(&key) && cache.entries.len() >= 64 {
                let evict = cache
                    .entries
                    .iter()
                    .find(|(_, cell)| Arc::strong_count(cell) == 1)
                    .map(|(key, _)| key.clone());
                if let Some(evict) = evict {
                    cache.entries.remove(&evict);
                }
            }
            cache
                .entries
                .entry(key.clone())
                .or_insert_with(|| Arc::new(PromptProjectionCell::new()))
                .clone()
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
                self.lock()?.entries.remove(&key);
                return Err(Error::Config(error));
            }
        };

        // Concurrent distinct evaluations may temporarily exceed the retention
        // limit. Trim completed cells after resolution without evicting in-flight
        // work or assigning a temporal order to content identities.
        let mut cache = self.lock()?;
        while cache.entries.len() > 64 {
            let evict = cache
                .entries
                .iter()
                .find(|(_, cell)| Arc::strong_count(cell) == 1)
                .map(|(key, _)| key.clone());
            let Some(evict) = evict else {
                break;
            };
            cache.entries.remove(&evict);
        }
        Ok(catalog)
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
    EffectivePromptCatalog::from_projection(prompts, "preview", "preview")
        .map(|_| ())
        .map_err(|error| Error::InvalidInput(format!("invalid Prompt catalog: {error}")))
}

pub fn project_prompts_from_workspace_config(
    state: &WorkspaceConfigState,
) -> Result<EffectivePromptCatalog> {
    let evaluation = evaluate_workspace_config_state(state, state.contract.schema_bundle.clone())?;
    if evaluation.projection_digest != state.projection_digest {
        return Err(Error::RegistryInconsistency(
            "Prompt projection digest does not match the active Workspace config content"
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
        let schema = WorkspaceConfigSchemaBundle::compose([PromptConfigSchemaProvider
            .contribution()
            .unwrap()])
        .unwrap();
        let snapshot = ConfigTreeSnapshot::from_entries([ConfigEntry::new(
            VirtualPath::parse("main.dcdl").unwrap(),
            ConfigContentType::Decodal,
            source,
        )
        .unwrap()])
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
    fn prompt_projection_cache_reuses_content_and_separates_workspaces() {
        let cache = WorkspacePromptProjectionCache::default();
        let initial = state("{}");
        let first = cache.resolve("workspace-a", &initial).unwrap();
        let retry = cache.resolve("workspace-a", &state("{}")).unwrap();
        assert!(Arc::ptr_eq(&first, &retry));
        let updated = state(r#"{ prompts = { common = { language = "UPDATED"; }; }; }"#);
        let replacement = cache.resolve("workspace-a", &updated).unwrap();
        assert!(!Arc::ptr_eq(&first, &replacement));
        assert_eq!(
            replacement.catalog().templates["common.language"],
            "UPDATED"
        );
        assert!(Arc::ptr_eq(
            &first,
            &cache.resolve("workspace-a", &initial).unwrap()
        ));
        assert_ne!(first.catalog().templates["common.language"], "UPDATED");
        let other = cache.resolve("workspace-b", &updated).unwrap();
        assert!(!Arc::ptr_eq(&replacement, &other));
        assert_eq!(cache.len(), 3);
    }

    #[test]
    fn prompt_projection_cache_separates_contracts_for_same_content() {
        let cache = WorkspacePromptProjectionCache::default();
        let initial = state("{}");
        let mut changed = initial.clone();
        let bundle = WorkspaceConfigSchemaBundle::compose([
            PromptConfigSchemaProvider.contribution().unwrap(),
            ConfigSchemaContribution::new(
                "builtin:extra",
                "extra",
                "1",
                "{ extra = Bool default false; }",
            )
            .unwrap(),
        ])
        .unwrap();
        changed.contract = ToolchainContract::with_schema_bundle(
            config_source::DEFAULT_SCHEMA_VERSION,
            changed.contract.entrypoints.clone(),
            config_source::DEFAULT_IMPORT_POLICY_VERSION,
            bundle,
        );
        changed.projection_digest = SnapshotEnvironment::new(changed.snapshot.clone())
            .evaluate_contract(&changed.contract)
            .unwrap()
            .projection_digest;
        assert_eq!(initial.snapshot.digest, changed.snapshot.digest);
        let first = cache.resolve("workspace-a", &initial).unwrap();
        let second = cache.resolve("workspace-a", &changed).unwrap();
        assert!(!Arc::ptr_eq(&first, &second));
        assert!(Arc::ptr_eq(
            &first,
            &cache.resolve("workspace-a", &initial).unwrap()
        ));
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn prompt_projection_cache_bounds_retained_content_cells() {
        let cache = WorkspacePromptProjectionCache::default();
        for index in 0..80 {
            let source = format!("{{ prompts = {{ common = {{ language = \"{index}\"; }}; }}; }}");
            cache.resolve("workspace-a", &state(&source)).unwrap();
        }
        assert_eq!(cache.len(), 64);
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
        assert_eq!(catalog.source_digest, state.snapshot.digest);
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
            let snapshot = ConfigTreeSnapshot::from_entries([ConfigEntry::new(
                VirtualPath::parse("main.dcdl").unwrap(),
                ConfigContentType::Decodal,
                source,
            )
            .unwrap()])
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
            let snapshot = ConfigTreeSnapshot::from_entries([ConfigEntry::new(
                VirtualPath::parse("main.dcdl").unwrap(),
                ConfigContentType::Decodal,
                source,
            )
            .unwrap()])
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
