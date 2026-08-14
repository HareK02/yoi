use config_source::{ConfigProjectionValidator, ConfigSchemaContribution};
use worker::{EffectivePromptCatalog, prompt_schema_source};

use crate::config_source::{
    WorkspaceConfigSchemaProvider, WorkspaceConfigState, evaluate_workspace_config_state,
};
use crate::{Error, Result};

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
    EffectivePromptCatalog::from_projection(
        prompts,
        state.snapshot.revision,
        state.contract.schema_bundle.fingerprint.clone(),
        state.contract.fingerprint.clone(),
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
        let snapshot = ConfigTreeSnapshot::from_entries(
            7,
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
    fn workspace_override_deep_patches_builtin_and_preserves_other_leaves() {
        let state = state(r#"{ prompts = { common = { language = "OVERRIDE"; }; }; }"#);
        let catalog = project_prompts_from_workspace_config(&state).unwrap();
        assert_eq!(catalog.config_revision, 7);
        assert_eq!(catalog.templates["common.language"], "OVERRIDE");
        assert!(!catalog.templates["common.workspace"].is_empty());
        assert!(catalog.templates["default"].contains("common.workspace"));
    }

    #[test]
    fn preview_commit_validator_rejects_dynamic_missing_and_cyclic_includes() {
        let schema = WorkspaceConfigSchemaBundle::compose([PromptConfigSchemaProvider
            .contribution()
            .unwrap()])
        .unwrap();
        for source in [
            r#"{ prompts = { common = { language = "{% include target %}"; }; }; }"#,
            r#"{ prompts = { common = { language = "{% include \"missing\" %}"; }; }; }"#,
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
