//! Plugin package contributions for model-visible Tool schemas.
//!
//! This module registers *enabled* plugin package tool surface definitions as
//! unavailable Tool stubs. It deliberately does not execute plugin code or grant
//! plugin permissions; the runtime/WASM executor belongs to a later boundary.

use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use llm_worker::tool::{
    Tool, ToolDefinition, ToolError, ToolExecutionContext, ToolMeta, ToolOrigin, ToolOutput,
};
use manifest::plugin::{PluginConfig, PluginSurface, ResolvedPluginRecord};
use serde_json::Value;

use super::{
    FeatureDescriptor, FeatureId, FeatureInstallContext, FeatureInstallError, FeatureModule,
    FeatureRuntimeKind, ToolContribution, ToolDeclaration,
};

/// Build Feature modules for enabled plugin packages when the profile exposes
/// the plugin Tool surface feature.
pub fn plugin_tool_features_if_enabled(
    feature_enabled: bool,
    config: &PluginConfig,
) -> Vec<PluginToolFeature> {
    if !feature_enabled {
        return Vec::new();
    }
    plugin_tool_features(config)
}

/// Build Feature modules for enabled plugin packages that declare Tool surfaces.
pub fn plugin_tool_features(config: &PluginConfig) -> Vec<PluginToolFeature> {
    config
        .resolved
        .iter()
        .filter(|record| record.enabled_surfaces.contains(&PluginSurface::Tool))
        .filter(|record| !record.manifest.tools.is_empty())
        .cloned()
        .map(PluginToolFeature::new)
        .collect()
}

#[derive(Clone, Debug)]
pub struct PluginToolFeature {
    record: ResolvedPluginRecord,
    feature_id: FeatureId,
}

impl PluginToolFeature {
    pub fn new(record: ResolvedPluginRecord) -> Self {
        let feature_id = FeatureId::new(format!("plugin:{}:tool", record.identity))
            .expect("source-qualified plugin identity yields non-empty feature id");
        Self { record, feature_id }
    }

    pub fn origin(&self) -> ToolOrigin {
        ToolOrigin {
            kind: "plugin".into(),
            plugin_id: self.record.manifest.id.clone(),
            plugin_ref: self.record.identity.to_string(),
            source: self.record.identity.source.to_string(),
            digest: self.record.digest.clone(),
            package_version: self.record.version.clone(),
            package_api_version: self.record.manifest.schema_version,
            surface: "tool".into(),
        }
    }
}

impl FeatureModule for PluginToolFeature {
    fn descriptor(&self) -> FeatureDescriptor {
        let mut descriptor =
            FeatureDescriptor {
                id: self.feature_id.clone(),
                runtime: FeatureRuntimeKind::ExternalPlugin,
                display_name: self.record.manifest.name.clone(),
                version: self.record.manifest.version.clone(),
                description: self.record.manifest.description.clone().unwrap_or_else(|| {
                    format!("Plugin tool surface from {}", self.record.identity)
                }),
                tools: Vec::new(),
                hooks: Vec::new(),
                background_tasks: Vec::new(),
                provides_services: Vec::new(),
                requires_services: Vec::new(),
                protocol_providers: Vec::new(),
            };
        for tool in &self.record.manifest.tools {
            descriptor = descriptor.with_tool(ToolDeclaration::new(
                tool.name.clone(),
                tool.description.clone(),
            ));
        }
        descriptor
    }

    fn install(&self, context: &mut FeatureInstallContext<'_>) -> Result<(), FeatureInstallError> {
        validate_declared_tool_names(&self.record)?;
        let origin = self.origin();
        for tool in &self.record.manifest.tools {
            validate_tool_name(&tool.name).map_err(|reason| {
                FeatureInstallError::Install(format!(
                    "plugin `{}` tool `{}` has invalid name: {reason}",
                    self.record.identity, tool.name
                ))
            })?;
            validate_input_schema(&tool.input_schema).map_err(|reason| {
                FeatureInstallError::Install(format!(
                    "plugin `{}` tool `{}` has invalid input_schema: {reason}",
                    self.record.identity, tool.name
                ))
            })?;
            context.tools().register(ToolContribution::new(
                tool.name.clone(),
                plugin_runtime_missing_definition(
                    tool.name.clone(),
                    tool.description.clone(),
                    tool.input_schema.clone(),
                    origin.clone(),
                ),
            ))?;
        }
        Ok(())
    }
}

fn plugin_runtime_missing_definition(
    name: String,
    description: String,
    input_schema: Value,
    origin: ToolOrigin,
) -> ToolDefinition {
    Arc::new(move || {
        (
            ToolMeta::new(name.clone())
                .description(description.clone())
                .input_schema(input_schema.clone())
                .origin(origin.clone()),
            Arc::new(PluginRuntimeMissingTool {
                name: name.clone(),
                origin: origin.clone(),
            }) as Arc<dyn Tool>,
        )
    })
}

struct PluginRuntimeMissingTool {
    name: String,
    origin: ToolOrigin,
}

#[async_trait]
impl Tool for PluginRuntimeMissingTool {
    async fn execute(
        &self,
        _input_json: &str,
        _ctx: ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        Err(ToolError::ExecutionFailed(format!(
            "plugin tool runtime missing/unavailable for `{}` from `{}` (digest {}, package {} api {})",
            self.name,
            self.origin.plugin_ref,
            self.origin.digest,
            self.origin.package_version,
            self.origin.package_api_version
        )))
    }
}

fn validate_declared_tool_names(record: &ResolvedPluginRecord) -> Result<(), FeatureInstallError> {
    let mut seen = HashSet::new();
    for tool in &record.manifest.tools {
        if !seen.insert(tool.name.as_str()) {
            return Err(FeatureInstallError::DuplicateToolName {
                tool: tool.name.clone(),
                first_feature: format!("{} (same plugin package)", record.identity),
                duplicate_feature: record.identity.to_string(),
            });
        }
    }
    Ok(())
}

fn validate_tool_name(name: &str) -> Result<(), &'static str> {
    if name.is_empty() {
        return Err("name must not be empty");
    }
    if name.len() > 128 {
        return Err("name is longer than 128 bytes");
    }
    if name.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err("name must not contain whitespace or control characters");
    }
    Ok(())
}

fn validate_input_schema(schema: &Value) -> Result<(), String> {
    let Value::Object(root) = schema else {
        return Err("root schema must be a JSON object".into());
    };
    match root.get("type") {
        Some(Value::String(value)) if value == "object" => {}
        Some(_) => return Err("root schema type must be `object`".into()),
        None => return Err("root schema must declare type = `object`".into()),
    }
    if let Some(properties) = root.get("properties") {
        if !properties.is_object() {
            return Err("properties must be a JSON object".into());
        }
    }
    if let Some(required) = root.get("required") {
        let Some(required) = required.as_array() else {
            return Err("required must be an array".into());
        };
        if !required.iter().all(Value::is_string) {
            return Err("required entries must be strings".into());
        }
    }
    if let Some(additional) = root.get("additionalProperties") {
        if !(additional.is_boolean() || additional.is_object()) {
            return Err("additionalProperties must be boolean or object".into());
        }
    }
    reject_unsupported_keywords(schema)
}

fn reject_unsupported_keywords(schema: &Value) -> Result<(), String> {
    match schema {
        Value::Object(map) => {
            for (key, value) in map {
                if matches!(
                    key.as_str(),
                    "$ref"
                        | "$dynamicRef"
                        | "oneOf"
                        | "anyOf"
                        | "allOf"
                        | "not"
                        | "patternProperties"
                        | "dependentSchemas"
                        | "dependencies"
                ) {
                    return Err(format!("unsupported schema keyword `{key}`"));
                }
                reject_unsupported_keywords(value)?;
            }
            Ok(())
        }
        Value::Array(values) => {
            for value in values {
                reject_unsupported_keywords(value)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use manifest::plugin::{PluginPackageManifest, SourceQualifiedPluginId};
    use serde_json::json;

    fn tool(name: &str) -> manifest::plugin::PluginToolManifest {
        manifest::plugin::PluginToolManifest {
            name: name.into(),
            description: format!("{name} tool"),
            input_schema: json!({"type":"object","properties":{},"additionalProperties":false}),
        }
    }

    fn record(tools: Vec<manifest::plugin::PluginToolManifest>) -> ResolvedPluginRecord {
        record_with_identity("project:example", tools)
    }

    fn record_with_identity(
        identity: &str,
        tools: Vec<manifest::plugin::PluginToolManifest>,
    ) -> ResolvedPluginRecord {
        let parsed_identity = SourceQualifiedPluginId::parse(identity).unwrap();
        ResolvedPluginRecord {
            identity: parsed_identity.clone(),
            source: parsed_identity.source,
            package_path: std::path::PathBuf::from("/tmp/example.zip"),
            package_label: "example.zip".into(),
            digest: "sha256:abc".into(),
            version: "0.1.0".into(),
            manifest: PluginPackageManifest {
                schema_version: 1,
                id: "example".into(),
                name: "Example".into(),
                version: "0.1.0".into(),
                description: None,
                surfaces: vec![PluginSurface::Tool],
                runtime: None,
                hooks: Vec::new(),
                tools,
            },
            enabled_surfaces: vec![PluginSurface::Tool],
            grants: manifest::plugin::PluginGrantConfig::default(),
            config: None,
        }
    }

    fn skipped_count(report: &super::super::FeatureRegistryInstallReport) -> usize {
        report
            .reports
            .iter()
            .map(|feature_report| feature_report.skipped.len())
            .sum()
    }

    fn has_diagnostic(report: &super::super::FeatureRegistryInstallReport, needle: &str) -> bool {
        report.reports.iter().any(|feature_report| {
            feature_report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains(needle))
        })
    }

    #[test]
    fn rejects_invalid_root_schema() {
        let schema = json!({"type":"string"});
        assert!(
            validate_input_schema(&schema)
                .unwrap_err()
                .contains("type must be `object`")
        );
    }

    #[test]
    fn rejects_unsupported_schema_keyword() {
        let schema = json!({"type":"object","oneOf":[]});
        assert!(
            validate_input_schema(&schema)
                .unwrap_err()
                .contains("unsupported schema keyword")
        );
    }

    #[test]
    fn accepts_object_tool_schema() {
        validate_input_schema(&json!({
            "type":"object",
            "properties":{"query":{"type":"string"}},
            "required":["query"],
            "additionalProperties":false
        }))
        .unwrap();
    }

    #[test]
    fn origin_retains_plugin_metadata() {
        let feature = PluginToolFeature::new(record(Vec::new()));
        let origin = feature.origin();
        assert_eq!(origin.kind, "plugin");
        assert_eq!(origin.plugin_id, "example");
        assert_eq!(origin.plugin_ref, "project:example");
        assert_eq!(origin.source, "project");
        assert_eq!(origin.digest, "sha256:abc");
        assert_eq!(origin.package_version, "0.1.0");
        assert_eq!(origin.package_api_version, 1);
        assert_eq!(origin.surface, "tool");
    }

    #[test]
    fn enabled_plugin_tool_registers_model_visible_schema_and_origin() {
        let mut pending = Vec::new();
        let mut hooks = crate::hook::HookRegistryBuilder::new();
        let report = super::super::FeatureRegistryBuilder::default()
            .with_module(PluginToolFeature::new(record(vec![tool("PluginSearch")])))
            .install_into_pending(&mut pending, &mut hooks);

        assert!(
            report
                .reports
                .iter()
                .all(|feature_report| feature_report.diagnostics.is_empty()),
            "{:#?}",
            report.reports
        );
        assert_eq!(report.installed_tool_names(), vec!["PluginSearch"]);
        assert_eq!(pending.len(), 1);
        let (meta, _) = pending[0]();
        assert_eq!(meta.name, "PluginSearch");
        assert_eq!(meta.input_schema["type"], "object");
        let origin = meta.origin.expect("plugin origin metadata");
        assert_eq!(origin.plugin_ref, "project:example");
        assert_eq!(origin.digest, "sha256:abc");
        assert_eq!(origin.source, "project");
        assert_eq!(origin.surface, "tool");
    }

    #[test]
    fn package_without_enabled_tool_surface_registers_no_schema() {
        let mut config = PluginConfig::default();
        let mut disabled = record(vec![tool("PluginSearch")]);
        disabled.enabled_surfaces.clear();
        config.resolved.push(disabled);

        assert!(plugin_tool_features(&config).is_empty());
    }

    #[test]
    fn disabled_profile_feature_registers_no_schema() {
        let mut config = PluginConfig::default();
        config.resolved.push(record(vec![tool("PluginSearch")]));

        assert!(plugin_tool_features_if_enabled(false, &config).is_empty());
        assert_eq!(plugin_tool_features_if_enabled(true, &config).len(), 1);
    }

    #[test]
    fn duplicate_plugin_tool_names_are_rejected_with_diagnostic() {
        let mut pending = Vec::new();
        let mut hooks = crate::hook::HookRegistryBuilder::new();
        let report = super::super::FeatureRegistryBuilder::default()
            .with_module(PluginToolFeature::new(record(vec![tool("PluginSearch")])))
            .with_module(PluginToolFeature::new(record_with_identity(
                "project:other",
                vec![tool("PluginSearch")],
            )))
            .install_into_pending(&mut pending, &mut hooks);

        assert_eq!(pending.len(), 1);
        assert_eq!(skipped_count(&report), 1);
        assert!(has_diagnostic(&report, "duplicate tool contribution"));
    }

    #[test]
    fn builtin_tool_name_collision_is_rejected_with_diagnostic() {
        let mut pending = Vec::new();
        let mut hooks = crate::hook::HookRegistryBuilder::new();
        let mut registered = std::collections::HashMap::new();
        registered.insert("Read".to_string(), FeatureId::builtin("preexisting-tool"));

        let report = super::super::FeatureRegistryBuilder::default()
            .with_module(PluginToolFeature::new(record(vec![tool("Read")])))
            .install_into_pending_with_registered(&mut pending, &mut hooks, registered);

        assert!(pending.is_empty());
        assert_eq!(skipped_count(&report), 1);
        assert!(has_diagnostic(&report, "duplicate tool contribution"));
    }

    #[test]
    fn invalid_input_schema_is_rejected_with_diagnostic() {
        let mut invalid = tool("BadSchema");
        invalid.input_schema = json!({"type":"object","$ref":"#/defs/input"});
        let mut pending = Vec::new();
        let mut hooks = crate::hook::HookRegistryBuilder::new();

        let report = super::super::FeatureRegistryBuilder::default()
            .with_module(PluginToolFeature::new(record(vec![invalid])))
            .install_into_pending(&mut pending, &mut hooks);

        assert!(pending.is_empty());
        assert!(has_diagnostic(&report, "invalid input_schema"));
    }

    #[tokio::test]
    async fn registered_tool_executes_as_runtime_missing_error() {
        let mut pending = Vec::new();
        let mut hooks = crate::hook::HookRegistryBuilder::new();
        let report = super::super::FeatureRegistryBuilder::default()
            .with_module(PluginToolFeature::new(record(vec![tool("PluginSearch")])))
            .install_into_pending(&mut pending, &mut hooks);
        assert!(
            report
                .reports
                .iter()
                .all(|feature_report| feature_report.diagnostics.is_empty()),
            "{:#?}",
            report.reports
        );

        let (_, tool) = pending[0]();
        let error = tool
            .execute("{}", ToolExecutionContext::default())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("runtime missing/unavailable"));
        assert!(error.to_string().contains("project:example"));
    }
}
