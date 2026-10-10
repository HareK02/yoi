use config_source::ConfigSchemaContribution;
use serde::Deserialize;

use crate::config_source::{
    WorkspaceConfigSchemaProvider, WorkspaceConfigState, evaluate_workspace_config_state,
};
use crate::{Error, Result};

const RUNTIME_SCHEMA_SOURCE: &str = r#"{
    runtime = {
        default_runtime_id = String default "";
    };
}"#;

#[derive(Debug, Default)]
pub struct RuntimeConfigSchemaProvider;

impl WorkspaceConfigSchemaProvider for RuntimeConfigSchemaProvider {
    fn contribution(&self) -> Result<ConfigSchemaContribution> {
        ConfigSchemaContribution::new("builtin:runtime", "runtime", "1", RUNTIME_SCHEMA_SOURCE)
            .map_err(|error| Error::Config(error.to_string()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeConfigProjection {
    pub projection_digest: String,
    pub default_runtime_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct VirtualRuntimeConfig {
    runtime: VirtualRuntimeSection,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct VirtualRuntimeSection {
    default_runtime_id: String,
}

pub fn project_runtime_from_workspace_config(
    workspace_id: &str,
    state: &WorkspaceConfigState,
) -> Result<RuntimeConfigProjection> {
    let has_runtime_schema = state
        .contract
        .schema_bundle
        .contributions
        .iter()
        .any(|entry| entry.provider_id == "builtin:runtime");
    if !has_runtime_schema {
        return Ok(RuntimeConfigProjection {
            projection_digest: state.projection_digest.clone(),
            default_runtime_id: None,
        });
    }

    let evaluation = evaluate_workspace_config_state(state, state.contract.schema_bundle.clone())?;
    if evaluation.projection_digest != state.projection_digest {
        return Err(Error::RegistryInconsistency(format!(
            "Runtime projection digest mismatch for Workspace {workspace_id}"
        )));
    }
    let projected = evaluation.projections.first().ok_or_else(|| {
        Error::RegistryInconsistency("Workspace config has no active projection".to_string())
    })?;
    let config: VirtualRuntimeConfig = serde_json::from_value(projected.data_json.clone())
        .map_err(|error| Error::RegistryInconsistency(error.to_string()))?;
    let default_runtime_id = normalize_runtime_id(&config.runtime.default_runtime_id)?;
    Ok(RuntimeConfigProjection {
        projection_digest: evaluation.projection_digest,
        default_runtime_id,
    })
}

fn normalize_runtime_id(value: &str) -> Result<Option<String>> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    if value.chars().any(char::is_control) {
        return Err(Error::InvalidRuntimeIdentifier {
            kind: "runtime_id".to_string(),
            value: "[redacted invalid value]".to_string(),
        });
    }
    Ok(Some(value.to_string()))
}

#[cfg(test)]
mod tests {
    use config_source::{ConfigContentType, ConfigEntry, ConfigTreeSnapshot, VirtualPath};

    use super::*;

    fn state(source: &str) -> WorkspaceConfigState {
        let bundle =
            config_source::WorkspaceConfigSchemaBundle::compose([RuntimeConfigSchemaProvider
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
        let contract = config_source::ToolchainContract::with_schema_bundle(
            config_source::DEFAULT_SCHEMA_VERSION,
            vec![VirtualPath::parse("main.dcdl").unwrap()],
            config_source::DEFAULT_IMPORT_POLICY_VERSION,
            bundle,
        );
        let projection_digest = config_source::SnapshotEnvironment::new(snapshot.clone())
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
    fn runtime_projection_reads_default_and_preserves_content_evidence() {
        let projection = project_runtime_from_workspace_config(
            "workspace",
            &state(
                r#"{ runtime = { default_runtime_id = "arcadia"; }; } as WorkspaceConfigSchema"#,
            ),
        )
        .unwrap();
        assert_eq!(projection.default_runtime_id.as_deref(), Some("arcadia"));
        assert!(!projection.projection_digest.is_empty());
    }

    #[test]
    fn runtime_projection_digest_tracks_content_not_observation_order() {
        let first =
            state(r#"{ runtime = { default_runtime_id = "arcadia"; }; } as WorkspaceConfigSchema"#);
        let same =
            state(r#"{ runtime = { default_runtime_id = "arcadia"; }; } as WorkspaceConfigSchema"#);
        let changed =
            state(r#"{ runtime = { default_runtime_id = "other"; }; } as WorkspaceConfigSchema"#);
        let first_projection = project_runtime_from_workspace_config("workspace", &first).unwrap();
        assert_eq!(
            first_projection,
            project_runtime_from_workspace_config("workspace", &same).unwrap()
        );
        let changed_projection =
            project_runtime_from_workspace_config("workspace", &changed).unwrap();
        assert_ne!(
            first_projection.projection_digest,
            changed_projection.projection_digest
        );
        assert_eq!(
            changed_projection.default_runtime_id.as_deref(),
            Some("other")
        );
        assert_eq!(
            first_projection,
            project_runtime_from_workspace_config("workspace", &first).unwrap()
        );
    }

    #[test]
    fn runtime_projection_rejects_mismatched_content_evidence() {
        let mut active = state("{} as WorkspaceConfigSchema");
        active.projection_digest = "sha256:tampered".to_string();
        assert!(matches!(
            project_runtime_from_workspace_config("workspace", &active),
            Err(Error::RegistryInconsistency(_))
        ));
    }

    #[test]
    fn runtime_projection_treats_missing_default_as_unconfigured() {
        let projection = project_runtime_from_workspace_config(
            "workspace",
            &state("{} as WorkspaceConfigSchema"),
        )
        .unwrap();
        assert_eq!(projection.default_runtime_id, None);
    }

    #[test]
    fn runtime_projection_treats_pre_runtime_schema_bundle_as_unconfigured() {
        let bundle =
            config_source::WorkspaceConfigSchemaBundle::compose([ConfigSchemaContribution::new(
                "builtin:legacy",
                "legacy",
                "1",
                "{ legacy = { enabled = Bool default false; }; }",
            )
            .unwrap()])
            .unwrap();
        let snapshot = ConfigTreeSnapshot::from_entries([ConfigEntry::new(
            VirtualPath::parse("main.dcdl").unwrap(),
            ConfigContentType::Decodal,
            "{ legacy = { enabled = true; }; } as WorkspaceConfigSchema",
        )
        .unwrap()])
        .unwrap();
        let contract = config_source::ToolchainContract::with_schema_bundle(
            config_source::DEFAULT_SCHEMA_VERSION,
            vec![VirtualPath::parse("main.dcdl").unwrap()],
            config_source::DEFAULT_IMPORT_POLICY_VERSION,
            bundle,
        );
        let projection_digest = config_source::SnapshotEnvironment::new(snapshot.clone())
            .evaluate_contract(&contract)
            .unwrap()
            .projection_digest;
        let state = WorkspaceConfigState {
            snapshot,
            contract,
            projection_digest: projection_digest.clone(),
        };

        let projection = project_runtime_from_workspace_config("workspace", &state).unwrap();
        assert_eq!(projection.default_runtime_id, None);
        assert_eq!(projection.projection_digest, projection_digest);
    }

    #[test]
    fn runtime_schema_rejects_non_string_default() {
        let bundle =
            config_source::WorkspaceConfigSchemaBundle::compose([RuntimeConfigSchemaProvider
                .contribution()
                .unwrap()])
            .unwrap();
        let snapshot = ConfigTreeSnapshot::from_entries([ConfigEntry::new(
            VirtualPath::parse("main.dcdl").unwrap(),
            ConfigContentType::Decodal,
            "{ runtime = { default_runtime_id = 42; }; } as WorkspaceConfigSchema",
        )
        .unwrap()])
        .unwrap();
        let contract = config_source::ToolchainContract::with_schema_bundle(
            config_source::DEFAULT_SCHEMA_VERSION,
            vec![VirtualPath::parse("main.dcdl").unwrap()],
            config_source::DEFAULT_IMPORT_POLICY_VERSION,
            bundle,
        );
        assert!(
            config_source::SnapshotEnvironment::new(snapshot)
                .evaluate_contract(&contract)
                .is_err()
        );
    }
}
