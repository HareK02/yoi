use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use worker::skill::{
    SkillActivationResponse, SkillCatalogEntry, SkillCatalogResponse, SkillDetailResponse,
    SkillDiagnostic, SkillDiagnosticSeverity, SkillProvenance, SkillResourceRef, SkillSourceKind,
};

const BUILTIN_AGENT_SKILLS: &str = include_str!("../../../resources/skills/agent-skills/SKILL.md");

#[derive(Debug, thiserror::Error)]
pub enum SkillError {
    #[error("unknown Skill `{0}`")]
    NotFound(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone)]
struct SkillSource {
    source_kind: SkillSourceKind,
    parent_name: String,
    content: String,
    resource_root: Option<PathBuf>,
}

#[derive(Debug, Clone)]
struct ParsedSkill {
    name: String,
    description: String,
    content: String,
    allowed_tools: Vec<String>,
    diagnostics: Vec<SkillDiagnostic>,
    provenance: SkillProvenance,
    resource_root: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct SkillFrontmatter {
    name: Option<String>,
    description: Option<String>,
    license: Option<String>,
    compatibility: Option<String>,
    metadata: Option<BTreeMap<String, serde_yaml::Value>>,
    #[serde(default)]
    allowed_tools: Option<serde_yaml::Value>,
}

pub fn catalog(workspace_root: &Path) -> SkillCatalogResponse {
    let mut diagnostics = Vec::new();
    let mut active = BTreeMap::<String, ParsedSkill>::new();
    let mut builtin_by_name = BTreeMap::<String, ParsedSkill>::new();

    for source in builtin_skill_sources() {
        match parse_skill_source(source) {
            Ok(skill) => {
                builtin_by_name.insert(skill.name.clone(), skill.clone());
                active.insert(skill.name.clone(), skill);
            }
            Err(errs) => diagnostics.extend(errs),
        }
    }

    let workspace_sources = match workspace_skill_sources(workspace_root) {
        Ok(sources) => sources,
        Err(error) => {
            diagnostics.push(SkillDiagnostic::error(
                "workspace_skill_read_failed",
                format!("failed to read workspace Skills: {error}"),
                Some("workspace:.yoi/skills".to_string()),
            ));
            Vec::new()
        }
    };

    for source in workspace_sources {
        match parse_skill_source(source) {
            Ok(skill) => {
                let overrides = builtin_by_name
                    .get(&skill.name)
                    .map(|builtin| vec![builtin.provenance.clone()])
                    .unwrap_or_default();
                if let Some(overridden) = overrides.first() {
                    diagnostics.push(SkillDiagnostic::warning(
                        "workspace_skill_overrides_builtin",
                        format!(
                            "workspace Skill `{}` overrides builtin Skill `{}`",
                            skill.name, overridden.id
                        ),
                        Some(skill.provenance.id.clone()),
                    ));
                }
                let mut skill = skill;
                if !overrides.is_empty() {
                    skill.diagnostics.push(SkillDiagnostic::warning(
                        "workspace_skill_overrides_builtin",
                        "workspace Skill has priority over the builtin Skill with the same name",
                        Some(skill.provenance.id.clone()),
                    ));
                }
                active.insert(skill.name.clone(), skill);
            }
            Err(errs) => diagnostics.extend(errs),
        }
    }

    let mut entries = active
        .into_values()
        .map(|skill| {
            let overrides = if matches!(&skill.provenance.kind, SkillSourceKind::Workspace) {
                builtin_by_name
                    .get(&skill.name)
                    .map(|builtin| vec![builtin.provenance.clone()])
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            SkillCatalogEntry {
                name: skill.name,
                description: skill.description,
                provenance: skill.provenance,
                overrides,
                diagnostics: skill.diagnostics,
            }
        })
        .collect::<Vec<_>>();
    entries.sort_by(|a, b| a.name.cmp(&b.name));

    SkillCatalogResponse {
        authority: "workspace-backend-skills-v0".to_string(),
        entries,
        diagnostics,
    }
}

pub fn lint(workspace_root: &Path) -> SkillCatalogResponse {
    catalog(workspace_root)
}

pub fn detail(workspace_root: &Path, name: &str) -> Result<SkillDetailResponse, SkillError> {
    let skill = active_skill(workspace_root, name)?;
    let overrides = if matches!(&skill.provenance.kind, SkillSourceKind::Workspace) {
        builtin_skill_sources()
            .into_iter()
            .filter_map(|source| parse_skill_source(source).ok())
            .find(|builtin| builtin.name == skill.name)
            .map(|builtin| vec![builtin.provenance])
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let resources = resource_refs(&skill);
    Ok(SkillDetailResponse {
        name: skill.name,
        description: skill.description,
        provenance: skill.provenance,
        overrides,
        diagnostics: skill.diagnostics,
        body: skill.content,
        allowed_tools: skill.allowed_tools,
        allowed_tools_status:
            "experimental_ignored_by_workspace_backend; does not grant or deny tool authority"
                .to_string(),
        resources,
    })
}

pub fn activation(
    workspace_root: &Path,
    name: &str,
) -> Result<SkillActivationResponse, SkillError> {
    let detail = detail(workspace_root, name)?;
    Ok(SkillActivationResponse {
        name: detail.name,
        provenance: detail.provenance,
        diagnostics: detail.diagnostics,
        body: detail.body,
    })
}

fn active_skill(workspace_root: &Path, name: &str) -> Result<ParsedSkill, SkillError> {
    let mut parsed = BTreeMap::<String, ParsedSkill>::new();
    for source in builtin_skill_sources() {
        if let Ok(skill) = parse_skill_source(source) {
            parsed.insert(skill.name.clone(), skill);
        }
    }
    for source in workspace_skill_sources(workspace_root)? {
        if let Ok(skill) = parse_skill_source(source) {
            parsed.insert(skill.name.clone(), skill);
        }
    }
    parsed
        .remove(name)
        .ok_or_else(|| SkillError::NotFound(name.to_string()))
}

fn builtin_skill_sources() -> Vec<SkillSource> {
    vec![SkillSource {
        source_kind: SkillSourceKind::Builtin,
        parent_name: "agent-skills".to_string(),
        content: BUILTIN_AGENT_SKILLS.to_string(),
        resource_root: None,
    }]
}

fn workspace_skill_sources(workspace_root: &Path) -> Result<Vec<SkillSource>, std::io::Error> {
    let skills_dir = workspace_root.join(".yoi").join("skills");
    if !skills_dir.exists() {
        return Ok(Vec::new());
    }
    let mut sources = Vec::new();
    for entry in fs::read_dir(&skills_dir)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if !file_type.is_dir() {
            continue;
        }
        let parent_name = entry.file_name().to_string_lossy().to_string();
        let skill_path = entry.path().join("SKILL.md");
        if !skill_path.exists() {
            sources.push(SkillSource {
                source_kind: SkillSourceKind::Workspace,
                parent_name,
                content: String::new(),
                resource_root: Some(entry.path()),
            });
            continue;
        }
        match fs::read_to_string(&skill_path) {
            Ok(content) => sources.push(SkillSource {
                source_kind: SkillSourceKind::Workspace,
                parent_name,
                content,
                resource_root: Some(entry.path()),
            }),
            Err(error) => sources.push(SkillSource {
                source_kind: SkillSourceKind::Workspace,
                parent_name,
                content: format!("__read_error__:{error}"),
                resource_root: None,
            }),
        }
    }
    Ok(sources)
}

fn parse_skill_source(source: SkillSource) -> Result<ParsedSkill, Vec<SkillDiagnostic>> {
    let provenance = provenance(source.source_kind.clone(), &source.parent_name);
    let mut diagnostics = Vec::new();

    if !valid_skill_name(&source.parent_name) {
        diagnostics.push(SkillDiagnostic::error(
            "invalid_skill_directory_name",
            "Skill directory name must be 1-64 chars of lowercase letters, numbers, or single hyphens with no leading/trailing hyphen",
            Some(provenance.id.clone()),
        ));
    }
    if source.content.is_empty() {
        diagnostics.push(SkillDiagnostic::error(
            "missing_skill_markdown",
            "Skill directory must contain SKILL.md",
            Some(provenance.id.clone()),
        ));
        return Err(diagnostics);
    }
    if source.content.starts_with("__read_error__:") {
        diagnostics.push(SkillDiagnostic::error(
            "skill_markdown_read_failed",
            source
                .content
                .trim_start_matches("__read_error__:")
                .to_string(),
            Some(provenance.id.clone()),
        ));
        return Err(diagnostics);
    }

    let Some((frontmatter, _markdown)) = split_frontmatter(&source.content) else {
        diagnostics.push(SkillDiagnostic::error(
            "missing_frontmatter",
            "SKILL.md must start with YAML frontmatter delimited by ---",
            Some(provenance.id.clone()),
        ));
        return Err(diagnostics);
    };
    let frontmatter_value = match serde_yaml::from_str::<serde_yaml::Value>(frontmatter) {
        Ok(value) => value,
        Err(error) => {
            diagnostics.push(SkillDiagnostic::error(
                "invalid_frontmatter_yaml",
                format!("SKILL.md frontmatter is invalid YAML: {error}"),
                Some(provenance.id.clone()),
            ));
            return Err(diagnostics);
        }
    };
    diagnose_unsupported_frontmatter_keys(&frontmatter_value, &provenance, &mut diagnostics);
    let frontmatter = match serde_yaml::from_value::<SkillFrontmatter>(frontmatter_value) {
        Ok(frontmatter) => frontmatter,
        Err(error) => {
            diagnostics.push(SkillDiagnostic::error(
                "invalid_frontmatter_yaml",
                format!("SKILL.md frontmatter is invalid YAML: {error}"),
                Some(provenance.id.clone()),
            ));
            return Err(diagnostics);
        }
    };

    let Some(name) = frontmatter.name else {
        diagnostics.push(SkillDiagnostic::error(
            "missing_name",
            "Skill frontmatter requires `name`",
            Some(provenance.id.clone()),
        ));
        return Err(diagnostics);
    };
    if name != source.parent_name {
        diagnostics.push(SkillDiagnostic::error(
            "name_parent_mismatch",
            "Skill frontmatter `name` must match its parent directory name",
            Some(provenance.id.clone()),
        ));
    }
    if !valid_skill_name(&name) {
        diagnostics.push(SkillDiagnostic::error(
            "invalid_name",
            "Skill name must be 1-64 chars of lowercase letters, numbers, or single hyphens with no leading/trailing hyphen",
            Some(provenance.id.clone()),
        ));
    }

    let Some(description) = frontmatter.description else {
        diagnostics.push(SkillDiagnostic::error(
            "missing_description",
            "Skill frontmatter requires `description`",
            Some(provenance.id.clone()),
        ));
        return Err(diagnostics);
    };
    let description = description.trim().to_string();
    if description.is_empty() || description.chars().count() > 1024 {
        diagnostics.push(SkillDiagnostic::error(
            "invalid_description",
            "Skill description must be 1-1024 characters",
            Some(provenance.id.clone()),
        ));
    } else if description.chars().count() < 16 {
        diagnostics.push(SkillDiagnostic::warning(
            "description_too_generic",
            "Skill description should state concrete when/what guidance",
            Some(provenance.id.clone()),
        ));
    }

    if let Some(license) = frontmatter.license.as_deref() {
        validate_optional_string("license", license, &provenance, &mut diagnostics);
    }
    if let Some(compatibility) = frontmatter.compatibility.as_deref() {
        validate_optional_string(
            "compatibility",
            compatibility,
            &provenance,
            &mut diagnostics,
        );
    }
    if let Some(metadata) = frontmatter.metadata {
        for (key, value) in metadata {
            if !matches!(value, serde_yaml::Value::String(_)) {
                diagnostics.push(SkillDiagnostic::error(
                    "invalid_metadata_value",
                    format!("metadata `{key}` must be a string value"),
                    Some(provenance.id.clone()),
                ));
            }
        }
    }

    let mut allowed_tools = Vec::new();
    if let Some(value) = frontmatter.allowed_tools {
        allowed_tools = parse_allowed_tools(value, &provenance, &mut diagnostics);
        diagnostics.push(SkillDiagnostic::warning(
            "allowed_tools_ignored",
            "allowed-tools is experimental metadata only; Workspace Skill activation does not grant or deny tools",
            Some(provenance.id.clone()),
        ));
    }

    if diagnostics
        .iter()
        .any(|d| d.severity == SkillDiagnosticSeverity::Error)
    {
        return Err(diagnostics);
    }

    Ok(ParsedSkill {
        name,
        description,
        content: source.content,
        allowed_tools,
        diagnostics,
        provenance,
        resource_root: source.resource_root,
    })
}

fn provenance(kind: SkillSourceKind, parent_name: &str) -> SkillProvenance {
    let prefix = match kind {
        SkillSourceKind::Builtin => "builtin",
        SkillSourceKind::Workspace => "workspace",
    };
    SkillProvenance {
        kind,
        id: format!("{prefix}:{parent_name}"),
    }
}

fn split_frontmatter(content: &str) -> Option<(&str, &str)> {
    let rest = content.strip_prefix("---\n")?;
    let (frontmatter, body) = rest.split_once("\n---")?;
    let body = body.strip_prefix('\n').unwrap_or(body);
    Some((frontmatter, body))
}

fn valid_skill_name(name: &str) -> bool {
    let len = name.chars().count();
    if !(1..=64).contains(&len)
        || name.starts_with('-')
        || name.ends_with('-')
        || name.contains("--")
    {
        return false;
    }
    name.chars()
        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
}

fn validate_optional_string(
    field: &str,
    value: &str,
    provenance: &SkillProvenance,
    diagnostics: &mut Vec<SkillDiagnostic>,
) {
    if value.trim().is_empty() {
        diagnostics.push(SkillDiagnostic::error(
            format!("invalid_{field}"),
            format!("optional `{field}` must be a non-empty string when present"),
            Some(provenance.id.clone()),
        ));
    }
}

fn diagnose_unsupported_frontmatter_keys(
    value: &serde_yaml::Value,
    provenance: &SkillProvenance,
    diagnostics: &mut Vec<SkillDiagnostic>,
) {
    let Some(mapping) = value.as_mapping() else {
        diagnostics.push(SkillDiagnostic::error(
            "invalid_frontmatter_shape",
            "SKILL.md frontmatter must be a YAML mapping",
            Some(provenance.id.clone()),
        ));
        return;
    };

    for key in mapping.keys() {
        let Some(key) = key.as_str() else {
            diagnostics.push(SkillDiagnostic::error(
                "invalid_frontmatter_key",
                "Skill frontmatter keys must be strings",
                Some(provenance.id.clone()),
            ));
            continue;
        };
        if !is_supported_frontmatter_key(key) {
            let (code, message) = if is_workflow_projection_key(key) {
                (
                    "unsupported_workflow_frontmatter_field",
                    format!(
                        "Skill frontmatter field `{key}` is a removed Workflow projection/invocation field and is not accepted as Skill semantics"
                    ),
                )
            } else {
                (
                    "unsupported_frontmatter_field",
                    format!(
                        "Skill frontmatter field `{key}` is not supported; supported fields are name, description, license, compatibility, metadata, and allowed-tools"
                    ),
                )
            };
            diagnostics.push(SkillDiagnostic::error(
                code,
                message,
                Some(provenance.id.clone()),
            ));
        }
    }
}

fn is_supported_frontmatter_key(key: &str) -> bool {
    matches!(
        key,
        "name" | "description" | "license" | "compatibility" | "metadata" | "allowed-tools"
    )
}

fn is_workflow_projection_key(key: &str) -> bool {
    matches!(
        key,
        "model_invokation"
            | "model_invocation"
            | "user_invocable"
            | "workflow"
            | "workflow_record"
            | "workflow_invoke"
            | "invocation"
            | "invocations"
            | "graph"
            | "nodes"
            | "edges"
            | "triggers"
    )
}

fn parse_allowed_tools(
    value: serde_yaml::Value,
    provenance: &SkillProvenance,
    diagnostics: &mut Vec<SkillDiagnostic>,
) -> Vec<String> {
    match value {
        serde_yaml::Value::String(text) => text
            .split(',')
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(ToOwned::to_owned)
            .collect(),
        serde_yaml::Value::Sequence(items) => items
            .into_iter()
            .filter_map(|item| match item {
                serde_yaml::Value::String(text) if !text.trim().is_empty() => Some(text),
                _ => {
                    diagnostics.push(SkillDiagnostic::warning(
                        "invalid_allowed_tools_entry_ignored",
                        "allowed-tools entries must be strings; invalid entries are ignored",
                        Some(provenance.id.clone()),
                    ));
                    None
                }
            })
            .collect(),
        _ => {
            diagnostics.push(SkillDiagnostic::warning(
                "invalid_allowed_tools_ignored",
                "allowed-tools must be a string or string list; value ignored",
                Some(provenance.id.clone()),
            ));
            Vec::new()
        }
    }
}

fn resource_refs(skill: &ParsedSkill) -> Vec<SkillResourceRef> {
    let Some(root) = &skill.resource_root else {
        return Vec::new();
    };
    let mut refs = Vec::new();
    for (dir, kind, supported, diagnostic) in [
        (
            "references",
            "reference",
            false,
            Some(
                "Skill references are listed only; resource read endpoints are not implemented yet",
            ),
        ),
        (
            "assets",
            "asset",
            false,
            Some("Skill assets are listed only; resource read endpoints are not implemented yet"),
        ),
        (
            "scripts",
            "script",
            false,
            Some(
                "Skill scripts are discovered but not executable; use normal typed tools and permissions",
            ),
        ),
    ] {
        let path = root.join(dir);
        if !path.is_dir() {
            continue;
        }
        if let Ok(entries) = fs::read_dir(&path) {
            for entry in entries.flatten() {
                if let Ok(file_type) = entry.file_type() {
                    if !(file_type.is_file() || file_type.is_dir()) {
                        continue;
                    }
                }
                let name = format!("{dir}/{}", entry.file_name().to_string_lossy());
                refs.push(SkillResourceRef {
                    kind: kind.to_string(),
                    name,
                    supported,
                    diagnostic: diagnostic.map(ToOwned::to_owned),
                });
            }
        }
    }
    refs.sort_by(|a, b| a.name.cmp(&b.name));
    refs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_skill(root: &Path, name: &str, content: &str) {
        let dir = root.join(".yoi").join("skills").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), content).unwrap();
    }

    #[test]
    fn workspace_skill_is_cataloged_without_body_and_detail_contains_body() {
        let tmp = tempfile::tempdir().unwrap();
        write_skill(
            tmp.path(),
            "debug-rust",
            "---\nname: debug-rust\ndescription: Use when debugging Rust failures and deciding what tests to run.\nallowed-tools:\n  - Bash\nmetadata:\n  owner: dev\n---\n\n# Debug Rust\n\nRun focused checks.",
        );

        let catalog = catalog(tmp.path());
        let entry = catalog
            .entries
            .iter()
            .find(|entry| entry.name == "debug-rust")
            .expect("workspace skill listed");
        assert_eq!(
            entry.description,
            "Use when debugging Rust failures and deciding what tests to run."
        );
        assert_eq!(entry.provenance.id, "workspace:debug-rust");
        let catalog_json = serde_json::to_string(&catalog).unwrap();
        assert!(!catalog_json.contains("# Debug Rust"));

        let detail = detail(tmp.path(), "debug-rust").unwrap();
        assert!(detail.body.contains("# Debug Rust"));
        assert_eq!(detail.allowed_tools, vec!["Bash"]);
        assert!(
            detail
                .allowed_tools_status
                .contains("does not grant or deny")
        );
        assert!(
            detail
                .diagnostics
                .iter()
                .any(|d| d.code == "allowed_tools_ignored")
        );
    }

    #[test]
    fn invalid_name_and_parent_mismatch_are_lint_errors() {
        let tmp = tempfile::tempdir().unwrap();
        write_skill(
            tmp.path(),
            "Bad--Name",
            "---\nname: other\ndescription: Use when checking invalid examples.\n---\n\n# Invalid",
        );
        let diagnostics = catalog(tmp.path()).diagnostics;
        assert!(
            diagnostics
                .iter()
                .any(|d| d.code == "invalid_skill_directory_name")
        );
        assert!(diagnostics.iter().any(|d| d.code == "name_parent_mismatch"));
    }

    #[test]
    fn workflow_projection_frontmatter_fields_are_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        write_skill(
            tmp.path(),
            "workflow-shaped",
            "---\nname: workflow-shaped\ndescription: Use when proving workflow projection fields are rejected as Skills.\nmodel_invokation: old-typo\nuser_invocable: true\ngraph: {}\ninvocation:\n  run: now\n---\n\n# Workflow Shaped",
        );

        let catalog = catalog(tmp.path());
        assert!(
            catalog
                .entries
                .iter()
                .all(|entry| entry.name != "workflow-shaped")
        );
        let codes = catalog
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code.as_str())
            .collect::<Vec<_>>();
        assert!(codes.contains(&"unsupported_workflow_frontmatter_field"));
        for unsupported in ["model_invokation", "user_invocable", "graph", "invocation"] {
            assert!(
                catalog.diagnostics.iter().any(|diagnostic| {
                    diagnostic.code == "unsupported_workflow_frontmatter_field"
                        && diagnostic.message.contains(unsupported)
                }),
                "missing unsupported-field diagnostic for {unsupported}"
            );
        }
        assert!(matches!(
            detail(tmp.path(), "workflow-shaped"),
            Err(SkillError::NotFound(_))
        ));
    }

    #[test]
    fn unknown_frontmatter_fields_are_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        write_skill(
            tmp.path(),
            "unknown-field",
            "---\nname: unknown-field\ndescription: Use when proving unsupported Skill fields are rejected.\ncustom-authority: no\n---\n\n# Unknown",
        );

        let catalog = catalog(tmp.path());
        assert!(
            catalog
                .entries
                .iter()
                .all(|entry| entry.name != "unknown-field")
        );
        assert!(catalog.diagnostics.iter().any(|diagnostic| diagnostic.code
            == "unsupported_frontmatter_field"
            && diagnostic.message.contains("custom-authority")));
    }

    #[test]
    fn workspace_skill_overrides_builtin() {
        let tmp = tempfile::tempdir().unwrap();
        write_skill(
            tmp.path(),
            "agent-skills",
            "---\nname: agent-skills\ndescription: Use when testing deterministic workspace override of builtin Skills.\n---\n\n# Workspace Override",
        );
        let catalog = catalog(tmp.path());
        let entry = catalog
            .entries
            .iter()
            .find(|entry| entry.name == "agent-skills")
            .unwrap();
        assert_eq!(entry.provenance.id, "workspace:agent-skills");
        assert_eq!(entry.overrides[0].id, "builtin:agent-skills");
        let detail = detail(tmp.path(), "agent-skills").unwrap();
        assert!(detail.body.contains("Workspace Override"));
    }

    #[test]
    fn scripts_are_reported_not_executable_without_raw_paths() {
        let tmp = tempfile::tempdir().unwrap();
        write_skill(
            tmp.path(),
            "scripted-help",
            "---\nname: scripted-help\ndescription: Use when checking Skill resource diagnostics safely.\n---\n\n# Scripted",
        );
        let script_dir = tmp.path().join(".yoi/skills/scripted-help/scripts");
        std::fs::create_dir_all(&script_dir).unwrap();
        std::fs::write(script_dir.join("run.sh"), "echo no").unwrap();
        let detail = detail(tmp.path(), "scripted-help").unwrap();
        let script = detail
            .resources
            .iter()
            .find(|r| r.name == "scripts/run.sh")
            .unwrap();
        assert!(!script.supported);
        assert!(
            !serde_json::to_string(&detail)
                .unwrap()
                .contains(tmp.path().to_str().unwrap())
        );
    }
}
