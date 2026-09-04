use std::collections::{BTreeMap, BTreeSet};

use config_source::{
    ConfigSchemaContribution, MarkdownDocumentProjection, VirtualPath, project_markdown_document,
};
use serde::Deserialize;
use worker::skill::SkillActivationResponse;
use workspace_api::{
    SKILL_CATALOG_AUTHORITY, SkillActivationStatus, SkillCatalogEntry, SkillCatalogResponse,
    SkillDetailResponse, SkillDiagnostic, SkillDiagnosticSeverity, SkillProjectionIdentity,
    SkillProjectionStatus, SkillProvenance, SkillResourceRef, SkillSourceKind,
};

use crate::config_source::{
    WorkspaceConfigSchemaProvider, WorkspaceConfigState, evaluate_workspace_config_state,
};

const BUILTIN_SKILL_ID: &str = "agent-skills";
const BUILTIN_SKILL_SOURCE: &str = include_str!("../../../resources/skills/agent-skills/SKILL.md");
const BUILTIN_SKILL_VIRTUAL_PATH: &str = "builtin/skills/agent-skills/SKILL.md";
const SKILL_SCHEMA_PROVIDER_ID: &str = "builtin:skills";
const SKILL_SCHEMA_NAMESPACE: &str = "skills";
const SKILL_SCHEMA_VERSION: &str = "1";

/// Skill documents are values imported from `SKILL.md`. Known Agent Skills
/// frontmatter is typed while extension keys remain concrete values.
pub const SKILL_DOCUMENT_SCHEMA_SOURCE: &str = r#"{
    frontmatter = {
        name = String;
        description = String;
        license = String default "";
        compatibility = String default "";
        metadata = {...String} default {};
        ...Unknown
    };
    content = String;
}"#;

const SKILL_CONFIG_SCHEMA_SOURCE: &str = r#"{
    skills = {...{
        frontmatter = {
            name = String;
            description = String;
            license = String default "";
            compatibility = String default "";
            metadata = {...String} default {};
            ...Unknown
        };
        content = String;
    }} default {};
}"#;

#[derive(Debug, Clone, Copy)]
pub struct SkillConfigSchemaProvider;

impl WorkspaceConfigSchemaProvider for SkillConfigSchemaProvider {
    fn contribution(&self) -> crate::Result<ConfigSchemaContribution> {
        ConfigSchemaContribution::new(
            SKILL_SCHEMA_PROVIDER_ID,
            SKILL_SCHEMA_NAMESPACE,
            SKILL_SCHEMA_VERSION,
            SKILL_CONFIG_SCHEMA_SOURCE,
        )
        .map_err(|error| crate::Error::Config(error.to_string()))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SkillError {
    #[error("unknown Skill `{0}`")]
    NotFound(String),
    #[error("Skill `{0}` has blocking diagnostics")]
    InvalidSkill(String),
    #[error("failed to evaluate the active Workspace config revision: {0}")]
    Evaluation(String),
    #[error("the active Workspace config projection is missing its root value")]
    MissingProjection,
    #[error("the active Workspace config Skill projection is invalid: {0}")]
    InvalidProjection(String),
}

#[derive(Debug, Clone)]
struct ParsedSkill {
    name: String,
    description: String,
    allowed_tools: Vec<String>,
    body: String,
    provenance: SkillProvenance,
    overrides: Vec<SkillProvenance>,
    resources: Vec<SkillResourceRef>,
    diagnostics: Vec<SkillDiagnostic>,
}

#[derive(Debug, Deserialize)]
struct WorkspaceSkillProjection {
    #[serde(default)]
    skills: BTreeMap<String, MarkdownDocumentProjection>,
}

pub fn catalog(state: &WorkspaceConfigState) -> Result<SkillCatalogResponse, SkillError> {
    let entries = merged_skills(state)?
        .into_values()
        .map(|skill| skill.catalog_entry())
        .collect();
    let response = SkillCatalogResponse {
        authority: SKILL_CATALOG_AUTHORITY.to_string(),
        projection: projection_identity(state),
        entries,
        diagnostics: Vec::new(),
    };
    response
        .validate()
        .map_err(|error| SkillError::InvalidProjection(error.to_string()))?;
    Ok(response)
}

pub fn lint(state: &WorkspaceConfigState) -> Result<SkillCatalogResponse, SkillError> {
    catalog(state)
}

fn projection_identity(state: &WorkspaceConfigState) -> SkillProjectionIdentity {
    SkillProjectionIdentity {
        config_revision: state.snapshot.revision,
        tree_digest: state.snapshot.digest.clone(),
    }
}

pub fn detail(state: &WorkspaceConfigState, name: &str) -> Result<SkillDetailResponse, SkillError> {
    let skill = merged_skills(state)?
        .remove(name)
        .ok_or_else(|| SkillError::NotFound(name.to_string()))?;
    let activation_status = skill.activation_status();
    let projection_status = skill.projection_status();
    let response = SkillDetailResponse {
        authority: SKILL_CATALOG_AUTHORITY.to_string(),
        projection: projection_identity(state),
        name: skill.name,
        description: skill.description,
        provenance: skill.provenance,
        overrides: skill.overrides,
        diagnostics: skill.diagnostics,
        activation_status,
        projection_status,
        body: skill.body,
        allowed_tools: skill.allowed_tools,
        allowed_tools_status: "experimental_hint_only".to_string(),
        resources: skill.resources,
    };
    response
        .validate()
        .map_err(|error| SkillError::InvalidProjection(error.to_string()))?;
    Ok(response)
}

pub fn activation(
    state: &WorkspaceConfigState,
    name: &str,
) -> Result<SkillActivationResponse, SkillError> {
    let skill = merged_skills(state)?
        .remove(name)
        .ok_or_else(|| SkillError::NotFound(name.to_string()))?;
    if skill.has_errors() {
        return Err(SkillError::InvalidSkill(name.to_string()));
    }
    Ok(SkillActivationResponse {
        name: skill.name,
        provenance: skill.provenance,
        diagnostics: skill.diagnostics,
        body: skill.body,
    })
}

fn merged_skills(
    state: &WorkspaceConfigState,
) -> Result<BTreeMap<String, ParsedSkill>, SkillError> {
    let mut merged = BTreeMap::new();
    let builtin_projection = project_markdown_document(BUILTIN_SKILL_SOURCE)
        .expect("embedded built-in Skill Markdown is valid");
    let builtin = parse_skill(
        BUILTIN_SKILL_ID,
        builtin_projection,
        SkillProvenance {
            kind: SkillSourceKind::Builtin,
            id: format!("builtin:{BUILTIN_SKILL_ID}"),
            virtual_path: Some(BUILTIN_SKILL_VIRTUAL_PATH.to_string()),
            revision: None,
            source_digest: Some(config_source::digest_bytes(BUILTIN_SKILL_SOURCE.as_bytes())),
            tree_digest: None,
        },
        Vec::new(),
        None,
    );
    merged.insert(builtin.name.clone(), builtin);

    let evaluation = evaluate_workspace_config_state(state, state.contract.schema_bundle.clone())
        .map_err(|error| SkillError::Evaluation(error.to_string()))?;
    let projection = evaluation
        .projections
        .first()
        .ok_or(SkillError::MissingProjection)?;
    let workspace =
        serde_json::from_value::<WorkspaceSkillProjection>(projection.data_json.clone())
            .map_err(|error| SkillError::InvalidProjection(error.to_string()))?;

    for (config_key, document) in workspace.skills {
        let name = string_field(&document.frontmatter, "name")
            .unwrap_or(&config_key)
            .to_string();
        let canonical_path = format!("skills/{name}/SKILL.md");
        let mut source_diagnostic = None;
        let source_entry = VirtualPath::parse(&canonical_path)
            .ok()
            .and_then(|path| state.snapshot.entries.get(&path));
        if let Some(entry) = source_entry {
            match project_markdown_document(&entry.content) {
                Ok(expected) if normalize_document(expected.clone()) == document => {}
                Ok(_) => {
                    source_diagnostic = Some(SkillDiagnostic::error(
                        "skill_source_mismatch",
                        format!(
                            "Skill `{name}` must be the imported value of `{canonical_path}` in the active config revision"
                        ),
                        Some(format!("workspace:{name}")),
                    ));
                }
                Err(message) => {
                    source_diagnostic = Some(SkillDiagnostic::error(
                        "invalid_skill_markdown",
                        format!("{canonical_path}: {message}"),
                        Some(format!("workspace:{name}")),
                    ));
                }
            }
        } else {
            source_diagnostic = Some(SkillDiagnostic::error(
                "missing_skill_source",
                format!("Skill `{name}` requires `{canonical_path}` in the active config revision"),
                Some(format!("workspace:{name}")),
            ));
        }
        let source_digest = source_entry
            .map(|entry| entry.content_digest.clone())
            .unwrap_or_else(|| config_source::digest_bytes(canonical_path.as_bytes()));
        let resources = workspace_resources(state, &name);
        let mut skill = parse_skill(
            &name,
            document,
            SkillProvenance {
                kind: SkillSourceKind::Workspace,
                id: format!("workspace:{name}"),
                virtual_path: Some(canonical_path),
                revision: Some(state.snapshot.revision),
                source_digest: Some(source_digest),
                tree_digest: Some(state.snapshot.digest.clone()),
            },
            resources,
            source_diagnostic,
        );
        if let Some(overridden) = merged.insert(name, skill.clone()) {
            skill.overrides.push(overridden.provenance);
            merged.insert(skill.name.clone(), skill);
        }
    }
    Ok(merged)
}

fn normalize_document(mut document: MarkdownDocumentProjection) -> MarkdownDocumentProjection {
    document
        .frontmatter
        .entry("license")
        .or_insert_with(|| serde_json::Value::String(String::new()));
    document
        .frontmatter
        .entry("compatibility")
        .or_insert_with(|| serde_json::Value::String(String::new()));
    document
        .frontmatter
        .entry("metadata")
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
    document
}

fn parse_skill(
    fallback_name: &str,
    document: MarkdownDocumentProjection,
    provenance: SkillProvenance,
    resources: Vec<SkillResourceRef>,
    source_diagnostic: Option<SkillDiagnostic>,
) -> ParsedSkill {
    let mut diagnostics = Vec::new();
    if let Some(diagnostic) = source_diagnostic {
        diagnostics.push(diagnostic);
    }
    let frontmatter = document.frontmatter;
    let name = string_field(&frontmatter, "name").unwrap_or(fallback_name);
    if !valid_skill_name(name) {
        diagnostics.push(SkillDiagnostic::error(
            "invalid_skill_name",
            "frontmatter `name` must be a lowercase kebab-case Skill id",
            Some(provenance.id.clone()),
        ));
    }
    if name != fallback_name {
        diagnostics.push(SkillDiagnostic::error(
            "skill_name_mismatch",
            format!("frontmatter name `{name}` must match Skill id `{fallback_name}`"),
            Some(provenance.id.clone()),
        ));
    }
    let description = string_field(&frontmatter, "description")
        .unwrap_or_default()
        .trim()
        .to_string();
    if description.is_empty() {
        diagnostics.push(SkillDiagnostic::error(
            "missing_description",
            "frontmatter `description` must be a non-empty string",
            Some(provenance.id.clone()),
        ));
    }
    for field in [
        "profile",
        "system_prompt",
        "prompt",
        "plugins",
        "plugin",
        "model_invokation",
        "model_invocation",
        "user_invocable",
        "graph",
        "invocation",
    ] {
        if frontmatter.contains_key(field) {
            diagnostics.push(SkillDiagnostic::error(
                "workflow_authority",
                format!("frontmatter `{field}` is workflow/profile authority and is not allowed"),
                Some(provenance.id.clone()),
            ));
        }
    }
    let allowed_tools = frontmatter
        .get("allowed-tools")
        .map(parse_allowed_tools)
        .unwrap_or_default();
    if !allowed_tools.is_empty() {
        diagnostics.push(SkillDiagnostic::warning(
            "allowed_tools_hint",
            "frontmatter `allowed-tools` is an instruction hint only and does not grant tools",
            Some(provenance.id.clone()),
        ));
    }
    ParsedSkill {
        name: fallback_name.to_string(),
        description,
        allowed_tools,
        body: document.content,
        provenance,
        overrides: Vec::new(),
        resources,
        diagnostics,
    }
}

fn valid_skill_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && !name.ends_with('-')
        && !name.contains("--")
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn string_field<'a>(
    frontmatter: &'a serde_json::Map<String, serde_json::Value>,
    field: &str,
) -> Option<&'a str> {
    frontmatter.get(field).and_then(serde_json::Value::as_str)
}

fn parse_allowed_tools(value: &serde_json::Value) -> Vec<String> {
    match value {
        serde_json::Value::String(value) => value
            .split(|character: char| character == ',' || character.is_whitespace())
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .collect(),
        serde_json::Value::Array(values) => values
            .iter()
            .filter_map(serde_json::Value::as_str)
            .map(ToOwned::to_owned)
            .collect(),
        _ => Vec::new(),
    }
}

fn workspace_resources(state: &WorkspaceConfigState, name: &str) -> Vec<SkillResourceRef> {
    let mut resources = Vec::new();
    for (kind, directory) in [
        ("reference", "references"),
        ("asset", "assets"),
        ("script", "scripts"),
    ] {
        let prefix = format!("skills/{name}/{directory}/");
        for child in resource_children(state, &prefix) {
            resources.push(SkillResourceRef {
                kind: kind.to_string(),
                name: child,
                supported: kind == "reference",
                diagnostic: (kind != "reference").then(|| {
                    format!("{kind} resources are catalogued but are not loaded automatically")
                }),
            });
        }
    }
    resources
}

fn resource_children(state: &WorkspaceConfigState, prefix: &str) -> Vec<String> {
    let mut children = BTreeSet::new();
    for path in state.snapshot.entries.keys() {
        let Some(relative) = path.as_str().strip_prefix(prefix) else {
            continue;
        };
        if relative.is_empty() {
            continue;
        }
        let child = relative.split('/').next().unwrap_or(relative);
        children.insert(format!("{prefix}{child}"));
    }
    children.into_iter().collect()
}

impl ParsedSkill {
    fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == SkillDiagnosticSeverity::Error)
    }

    fn activation_status(&self) -> SkillActivationStatus {
        if self.has_errors() {
            SkillActivationStatus::Inactive
        } else {
            SkillActivationStatus::Active
        }
    }

    fn projection_status(&self) -> SkillProjectionStatus {
        if self.has_errors() {
            SkillProjectionStatus::Invalid
        } else {
            SkillProjectionStatus::Valid
        }
    }

    fn catalog_entry(&self) -> SkillCatalogEntry {
        SkillCatalogEntry {
            name: self.name.clone(),
            description: self.description.clone(),
            activation_status: self.activation_status(),
            projection_status: self.projection_status(),
            provenance: self.provenance.clone(),
            overrides: self.overrides.clone(),
            diagnostics: self.diagnostics.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use config_source::{
        ConfigContentType, ConfigEntry, ConfigTreeSnapshot, ToolchainContract,
        WorkspaceConfigSchemaBundle,
    };

    use super::*;

    fn state(main: &str, markdown: &str, name: &str) -> WorkspaceConfigState {
        let skill_path = format!("skills/{name}/SKILL.md");
        let reference_path = format!("skills/{name}/references/checklist.md");
        let tree = ConfigTreeSnapshot::from_entries(
            9,
            [
                ConfigEntry::new(
                    VirtualPath::parse("main.dcdl").unwrap(),
                    ConfigContentType::Decodal,
                    main,
                )
                .unwrap(),
                ConfigEntry::new(
                    VirtualPath::parse(&skill_path).unwrap(),
                    ConfigContentType::Text,
                    markdown,
                )
                .unwrap(),
                ConfigEntry::new(
                    VirtualPath::parse(&reference_path).unwrap(),
                    ConfigContentType::Text,
                    "checklist",
                )
                .unwrap(),
            ],
        )
        .unwrap();
        let bundle = WorkspaceConfigSchemaBundle::compose([SkillConfigSchemaProvider
            .contribution()
            .unwrap()])
        .unwrap();
        WorkspaceConfigState {
            snapshot: tree,
            contract: ToolchainContract::with_schema_bundle(
                1,
                vec![VirtualPath::parse("main.dcdl").unwrap()],
                1,
                bundle,
            ),
            projection_digest: "projection".to_string(),
        }
    }

    fn main_source(name: &str) -> String {
        let config_key = name.replace('-', "_");
        format!(
            r#"{{ skills = {{ {config_key} = import "./skills/{name}/SKILL.md" as {}; }}; }}"#,
            SKILL_DOCUMENT_SCHEMA_SOURCE
        )
    }

    #[test]
    fn workspace_skill_projection_keeps_extensions_and_uses_virtual_resources() {
        let markdown = concat!(
            "---\n",
            "name: debug-rust\n",
            "description: Debug Rust failures\n",
            "custom-authority: no\n",
            "allowed-tools: Read Grep\n",
            "metadata:\n  owner: platform\n",
            "---\n",
            "# Debug Rust\n",
        );
        let state = state(&main_source("debug-rust"), markdown, "debug-rust");
        let evaluation =
            evaluate_workspace_config_state(&state, state.contract.schema_bundle.clone()).unwrap();
        assert_eq!(
            evaluation.projections[0].data_json["skills"]["debug_rust"]["frontmatter"]["custom-authority"],
            "no"
        );
        let catalog = catalog(&state).unwrap();
        let item = catalog
            .entries
            .iter()
            .find(|item| item.name == "debug-rust")
            .unwrap();
        assert_eq!(item.provenance.kind, SkillSourceKind::Workspace);
        assert_eq!(item.provenance.revision, Some(9));
        assert_eq!(catalog.projection.config_revision, 9);
        assert_eq!(catalog.projection.tree_digest, state.snapshot.digest);
        assert_eq!(item.activation_status, SkillActivationStatus::Active);
        assert_eq!(item.projection_status, SkillProjectionStatus::Valid);
        assert!(
            item.diagnostics
                .iter()
                .all(|diagnostic| diagnostic.severity != SkillDiagnosticSeverity::Error)
        );
        let detail = detail(&state, "debug-rust").unwrap();
        assert_eq!(detail.authority, SKILL_CATALOG_AUTHORITY);
        assert_eq!(detail.projection.config_revision, 9);
        assert_eq!(detail.activation_status, SkillActivationStatus::Active);
        assert_eq!(detail.projection_status, SkillProjectionStatus::Valid);
        assert_eq!(detail.body, "# Debug Rust\n");
        assert_eq!(detail.allowed_tools, vec!["Read", "Grep"]);
        assert_eq!(
            detail.resources[0].name,
            "skills/debug-rust/references/checklist.md"
        );
        assert_eq!(
            activation(&state, "debug-rust").unwrap().body,
            "# Debug Rust\n"
        );
    }

    #[test]
    fn workspace_override_replaces_builtin_deterministically() {
        let markdown = concat!(
            "---\nname: agent-skills\n",
            "description: Workspace override\n",
            "---\n# Workspace agent skills\n",
        );
        let state = state(&main_source("agent-skills"), markdown, "agent-skills");
        let catalog = catalog(&state).unwrap();
        assert_eq!(
            catalog
                .entries
                .iter()
                .filter(|item| item.name == "agent-skills")
                .count(),
            1
        );
        let item = catalog
            .entries
            .iter()
            .find(|item| item.name == "agent-skills")
            .unwrap();
        assert_eq!(item.description, "Workspace override");
        assert_eq!(item.provenance.kind, SkillSourceKind::Workspace);
        assert_eq!(item.overrides[0].kind, SkillSourceKind::Builtin);
    }

    #[test]
    fn inline_skill_value_cannot_claim_canonical_source_identity() {
        let main = r#"{
            skills = {
                debug_rust = {
                    frontmatter = {
                        name = "debug-rust";
                        description = "Inline authority";
                    };
                    content = "inline";
                };
            };
        }"#;
        let markdown = "---\nname: debug-rust\ndescription: File authority\n---\nfile\n";
        let state = state(main, markdown, "debug-rust");
        let item = catalog(&state)
            .unwrap()
            .entries
            .into_iter()
            .find(|item| item.name == "debug-rust")
            .unwrap();
        assert_eq!(item.activation_status, SkillActivationStatus::Inactive);
        assert_eq!(item.projection_status, SkillProjectionStatus::Invalid);
        assert!(
            item.diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "skill_source_mismatch")
        );
        assert!(matches!(
            activation(&state, "debug-rust"),
            Err(SkillError::InvalidSkill(_))
        ));
    }

    #[test]
    fn schema_accepts_unknown_extensions_but_keeps_skill_documents_typed() {
        let contribution = SkillConfigSchemaProvider.contribution().unwrap();
        assert_eq!(contribution.namespace, "skills");
        assert!(contribution.source.contains("...Unknown"));
        assert!(contribution.source.contains("metadata = {...String}"));
    }
}
