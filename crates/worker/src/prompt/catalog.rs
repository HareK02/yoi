//! Typed effective Prompt catalog.
//!
//! Builtins are evaluated from the embedded `resources/prompts/catalog.dcdl`
//! source tree. Markdown imports use the same `{ frontmatter, content }`
//! projection as Workspace config; the catalog DCDL selects `.content`.
//! Workspace configuration materializes a complete closed `prompts` object,
//! which is carried as an immutable [`EffectivePromptCatalog`] projection.

use std::collections::BTreeMap;
use std::sync::Arc;

use config_source::{
    ConfigContentType, ConfigEntry, ConfigTreeSnapshot, SnapshotEnvironment, ToolchainContract,
    VirtualPath, digest_bytes,
};
use include_dir::{Dir, include_dir};
use minijinja::value::Value;
use minijinja::{Environment, UndefinedBehavior};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::prompt::source::PromptCatalogSource;

static BUILTIN_PROMPT_SOURCES: Dir<'static> =
    include_dir!("$CARGO_MANIFEST_DIR/../../resources/prompts");
const BUILTIN_CATALOG_ENTRY: &str = "catalog.dcdl";
const BUILTIN_TOOLCHAIN_FINGERPRINT: &str = "builtin:prompts:decodal-0.4";

/// Immutable Prompt projection delivered by Workspace authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectivePromptCatalog {
    pub templates: BTreeMap<String, String>,
    pub config_revision: u64,
    pub schema_fingerprint: String,
    pub toolchain_fingerprint: String,
    pub catalog_digest: String,
}

impl EffectivePromptCatalog {
    pub fn new(
        templates: BTreeMap<String, String>,
        config_revision: u64,
        schema_fingerprint: impl Into<String>,
        toolchain_fingerprint: impl Into<String>,
    ) -> Result<Self, CatalogError> {
        validate_prompt_templates(&templates)?;
        let catalog_digest = catalog_digest(&templates)?;
        Ok(Self {
            templates,
            config_revision,
            schema_fingerprint: schema_fingerprint.into(),
            toolchain_fingerprint: toolchain_fingerprint.into(),
            catalog_digest,
        })
    }

    pub fn from_projection(
        prompts: &serde_json::Value,
        config_revision: u64,
        schema_fingerprint: impl Into<String>,
        toolchain_fingerprint: impl Into<String>,
    ) -> Result<Self, CatalogError> {
        let mut templates = BTreeMap::new();
        flatten_templates("", prompts, &mut templates)?;
        if let Some(default_prompt) = templates.remove("default_prompt") {
            templates.insert("default".to_string(), default_prompt);
        }
        Self::new(
            templates,
            config_revision,
            schema_fingerprint,
            toolchain_fingerprint,
        )
    }

    pub fn verify_digest(&self) -> Result<(), CatalogError> {
        let actual = catalog_digest(&self.templates)?;
        if actual != self.catalog_digest {
            return Err(CatalogError::DigestMismatch {
                expected: self.catalog_digest.clone(),
                actual,
            });
        }
        validate_prompt_templates(&self.templates)
    }
}

/// Worker-level prompt injection point.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WorkerPrompt {
    CompactSystem,
    MemoryExtractSystem,
    MemoryConsolidationSystem,
    FlowVerifierSystem,
    NotifyWrapper,
    InterruptToolResultSummary,
    InterruptSystemNote,
    WorkingBoundariesSection,
    AgentsMdSection,
    ResidentMemorySummarySection,
    WorkerOrchestrationGuidanceSection,
    TicketEventCompanionNotice,
    SubWorkerSpawnToolDescription,
}

impl WorkerPrompt {
    pub fn key(self) -> &'static str {
        match self {
            Self::CompactSystem => "internal.compact_system",
            Self::MemoryExtractSystem => "internal.memory_extract_system",
            Self::MemoryConsolidationSystem => "internal.memory_consolidation_system",
            Self::FlowVerifierSystem => "internal.flow_verifier_system",
            Self::NotifyWrapper => "internal.notify_wrapper",
            Self::InterruptToolResultSummary => "internal.interrupt_tool_result_summary",
            Self::InterruptSystemNote => "internal.interrupt_system_note",
            Self::WorkingBoundariesSection => "internal.working_boundaries_section",
            Self::AgentsMdSection => "internal.agents_md_section",
            Self::ResidentMemorySummarySection => "internal.resident_memory_summary_section",
            Self::WorkerOrchestrationGuidanceSection => {
                "internal.worker_orchestration_guidance_section"
            }
            Self::TicketEventCompanionNotice => "worker.ticket_event_companion_notice",
            Self::SubWorkerSpawnToolDescription => "internal.sub_worker_spawn_tool_description",
        }
    }

    pub const ALL: &'static [WorkerPrompt] = &[
        WorkerPrompt::CompactSystem,
        WorkerPrompt::MemoryExtractSystem,
        WorkerPrompt::MemoryConsolidationSystem,
        WorkerPrompt::FlowVerifierSystem,
        WorkerPrompt::NotifyWrapper,
        WorkerPrompt::InterruptToolResultSummary,
        WorkerPrompt::InterruptSystemNote,
        WorkerPrompt::WorkingBoundariesSection,
        WorkerPrompt::AgentsMdSection,
        WorkerPrompt::ResidentMemorySummarySection,
        WorkerPrompt::WorkerOrchestrationGuidanceSection,
        WorkerPrompt::TicketEventCompanionNotice,
        WorkerPrompt::SubWorkerSpawnToolDescription,
    ];
}

#[derive(Debug, Error)]
pub enum CatalogError {
    #[error("failed to build builtin Prompt source tree: {0}")]
    BuiltinTree(String),
    #[error("failed to evaluate builtin Prompt source tree: {0}")]
    BuiltinEvaluation(String),
    #[error("effective Prompt projection at '{path}' must be an object or string")]
    InvalidProjection { path: String },
    #[error("invalid effective Prompt template catalog: {0}")]
    InvalidTemplateCatalog(String),
    #[error("failed to compile prompt template '{key}': {source}")]
    TemplateCompile {
        key: String,
        #[source]
        source: minijinja::Error,
    },
    #[error("failed to render prompt '{key}': {source}")]
    Render {
        key: String,
        #[source]
        source: minijinja::Error,
    },
    #[error("prompt key '{key}' is not registered in the catalog")]
    UnknownKey { key: String },
    #[error("failed to serialize effective Prompt catalog: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("effective Prompt catalog digest mismatch: expected {expected}, got {actual}")]
    DigestMismatch { expected: String, actual: String },
}

pub struct PromptCatalog {
    env: Environment<'static>,
    projection: EffectivePromptCatalog,
}

impl std::fmt::Debug for PromptCatalog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PromptCatalog")
            .field("config_revision", &self.projection.config_revision)
            .field("catalog_digest", &self.projection.catalog_digest)
            .finish_non_exhaustive()
    }
}

impl PromptCatalog {
    pub fn builtins_only() -> Result<Arc<Self>, CatalogError> {
        let templates = builtin_prompt_templates()?;
        let projection = EffectivePromptCatalog::new(
            templates,
            0,
            BUILTIN_TOOLCHAIN_FINGERPRINT,
            BUILTIN_TOOLCHAIN_FINGERPRINT,
        )?;
        Self::from_projection(projection).map(Arc::new)
    }

    pub fn load(loader: &PromptCatalogSource) -> Result<Arc<Self>, CatalogError> {
        if let Some(projection) = loader.effective_catalog() {
            return Self::from_projection(projection.clone()).map(Arc::new);
        }
        Self::builtins_only()
    }

    pub fn from_projection(projection: EffectivePromptCatalog) -> Result<Self, CatalogError> {
        projection.verify_digest()?;
        let mut env = Environment::new();
        env.set_undefined_behavior(UndefinedBehavior::Strict);
        for (key, source) in &projection.templates {
            env.add_template_owned(key.clone(), source.clone())
                .map_err(|source| CatalogError::TemplateCompile {
                    key: key.clone(),
                    source,
                })?;
        }
        Ok(Self { env, projection })
    }

    pub fn projection(&self) -> &EffectivePromptCatalog {
        &self.projection
    }

    pub(crate) fn source(&self) -> PromptCatalogSource {
        PromptCatalogSource::builtins_only().with_effective_catalog(self.projection.clone())
    }

    pub fn contains(&self, key: &str) -> bool {
        self.projection.templates.contains_key(key)
    }

    pub fn render_name(&self, key: &str, ctx: Value) -> Result<String, CatalogError> {
        let template = self
            .env
            .get_template(key)
            .map_err(|_| CatalogError::UnknownKey { key: key.into() })?;
        template.render(ctx).map_err(|source| CatalogError::Render {
            key: key.into(),
            source,
        })
    }

    pub fn render(&self, prompt: WorkerPrompt, ctx: Value) -> Result<String, CatalogError> {
        self.render_name(prompt.key(), ctx)
    }

    pub fn compact_system(&self) -> Result<String, CatalogError> {
        self.render(WorkerPrompt::CompactSystem, Value::UNDEFINED)
    }
    pub fn memory_extract_system(&self, language: &str) -> Result<String, CatalogError> {
        self.render(
            WorkerPrompt::MemoryExtractSystem,
            single("language", language),
        )
    }
    pub fn memory_consolidation_system(&self, language: &str) -> Result<String, CatalogError> {
        self.render(
            WorkerPrompt::MemoryConsolidationSystem,
            single("language", language),
        )
    }
    pub fn flow_verifier_system(&self) -> Result<String, CatalogError> {
        self.render(WorkerPrompt::FlowVerifierSystem, Value::UNDEFINED)
    }
    pub fn notify_wrapper(&self, message: &str) -> Result<String, CatalogError> {
        self.render(WorkerPrompt::NotifyWrapper, single("message", message))
    }
    pub fn interrupt_tool_result_summary(&self) -> Result<String, CatalogError> {
        self.render(WorkerPrompt::InterruptToolResultSummary, Value::UNDEFINED)
    }
    pub fn interrupt_system_note(&self) -> Result<String, CatalogError> {
        self.render(WorkerPrompt::InterruptSystemNote, Value::UNDEFINED)
    }
    pub fn working_boundaries_section(&self, scope_summary: &str) -> Result<String, CatalogError> {
        self.render(
            WorkerPrompt::WorkingBoundariesSection,
            single("scope_summary", scope_summary),
        )
    }
    pub fn agents_md_section(&self, agents_md: &str) -> Result<String, CatalogError> {
        self.render(
            WorkerPrompt::AgentsMdSection,
            single("agents_md", agents_md),
        )
    }
    pub fn resident_memory_summary_section(&self, summary: &str) -> Result<String, CatalogError> {
        self.render(
            WorkerPrompt::ResidentMemorySummarySection,
            single("summary", summary),
        )
    }
    pub fn worker_orchestration_guidance_section(&self) -> Result<String, CatalogError> {
        self.render(
            WorkerPrompt::WorkerOrchestrationGuidanceSection,
            Value::UNDEFINED,
        )
    }
    pub fn sub_worker_spawn_tool_description(
        &self,
        available_profiles: &str,
        default_profile: &str,
        profile_diagnostic: &str,
    ) -> Result<String, CatalogError> {
        let mut context = BTreeMap::new();
        context.insert("available_profiles", Value::from(available_profiles));
        context.insert("default_profile", Value::from(default_profile));
        context.insert("profile_diagnostic", Value::from(profile_diagnostic));
        self.render(
            WorkerPrompt::SubWorkerSpawnToolDescription,
            Value::from(context),
        )
    }
}

/// DCDL schema contribution for the closed `WorkspaceConfig.prompts` namespace.
/// Every leaf defaults to its builtin value, so Workspace config is a right-biased
/// deep patch while evaluation materializes a complete effective catalog.
pub fn prompt_schema_source() -> Result<String, CatalogError> {
    let mut templates = builtin_prompt_templates()?;
    if let Some(default) = templates.remove("default") {
        templates.insert("default_prompt".to_string(), default);
    }
    let tree = unflatten_templates(&templates);
    let mut output = String::from("{ prompts = ");
    write_schema_object(&mut output, &tree)?;
    output.push_str(" default {}; }");
    Ok(output)
}

pub fn builtin_prompt_templates() -> Result<BTreeMap<String, String>, CatalogError> {
    let mut entries = Vec::new();
    collect_builtin_entries(&BUILTIN_PROMPT_SOURCES, "", &mut entries)?;
    let snapshot = ConfigTreeSnapshot::from_entries(0, entries)
        .map_err(|error| CatalogError::BuiltinTree(error.to_string()))?;
    let entry = VirtualPath::parse(BUILTIN_CATALOG_ENTRY)
        .map_err(|error| CatalogError::BuiltinTree(error.to_string()))?;
    let result = SnapshotEnvironment::new(snapshot)
        .evaluate_contract(&ToolchainContract::new(1, vec![entry], 1))
        .map_err(|diagnostics| {
            CatalogError::BuiltinEvaluation(
                diagnostics
                    .into_iter()
                    .map(|diagnostic| {
                        format!(
                            "{}:{}:{}..{}: {}",
                            diagnostic.path,
                            diagnostic.kind,
                            diagnostic.span.start_byte,
                            diagnostic.span.end_byte,
                            diagnostic.message
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("; "),
            )
        })?;
    let projection = result
        .projections
        .first()
        .ok_or_else(|| CatalogError::BuiltinEvaluation("catalog produced no projection".into()))?;
    let mut templates = BTreeMap::new();
    flatten_templates("", &projection.data_json, &mut templates)?;
    if let Some(default_prompt) = templates.remove("default_prompt") {
        templates.insert("default".to_string(), default_prompt);
    }
    validate_prompt_templates(&templates)?;
    Ok(templates)
}

fn collect_builtin_entries(
    dir: &Dir<'static>,
    prefix: &str,
    entries: &mut Vec<ConfigEntry>,
) -> Result<(), CatalogError> {
    for file in dir.files() {
        let Some(name) = file.path().file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let relative = if prefix.is_empty() {
            name.to_string()
        } else {
            format!("{prefix}/{name}")
        };
        let content_type = if relative.ends_with(".dcdl") {
            ConfigContentType::Decodal
        } else if relative.ends_with(".md") {
            ConfigContentType::Text
        } else {
            continue;
        };
        let content = file.contents_utf8().ok_or_else(|| {
            CatalogError::BuiltinTree(format!("builtin Prompt source is not UTF-8: {relative}"))
        })?;
        let path = VirtualPath::parse(&relative)
            .map_err(|error| CatalogError::BuiltinTree(error.to_string()))?;
        entries.push(
            ConfigEntry::new(path, content_type, content)
                .map_err(|error| CatalogError::BuiltinTree(error.to_string()))?,
        );
    }
    for child in dir.dirs() {
        let Some(name) = child.path().file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let child_prefix = if prefix.is_empty() {
            name.to_string()
        } else {
            format!("{prefix}/{name}")
        };
        collect_builtin_entries(child, &child_prefix, entries)?;
    }
    Ok(())
}

fn single(key: &'static str, value: &str) -> Value {
    Value::from(BTreeMap::from([(key, Value::from(value))]))
}

fn flatten_templates(
    prefix: &str,
    value: &serde_json::Value,
    output: &mut BTreeMap<String, String>,
) -> Result<(), CatalogError> {
    match value {
        serde_json::Value::String(source) if !prefix.is_empty() => {
            output.insert(prefix.to_string(), source.clone());
            Ok(())
        }
        serde_json::Value::Object(fields) => {
            for (name, value) in fields {
                let key = if prefix.is_empty() {
                    name.clone()
                } else {
                    format!("{prefix}.{name}")
                };
                flatten_templates(&key, value, output)?;
            }
            Ok(())
        }
        _ => Err(CatalogError::InvalidProjection {
            path: if prefix.is_empty() {
                "prompts".into()
            } else {
                format!("prompts.{prefix}")
            },
        }),
    }
}

#[derive(Default)]
struct TemplateNode {
    value: Option<String>,
    children: BTreeMap<String, TemplateNode>,
}

fn unflatten_templates(templates: &BTreeMap<String, String>) -> TemplateNode {
    let mut root = TemplateNode::default();
    for (key, value) in templates {
        let mut node = &mut root;
        for segment in key.split('.') {
            node = node.children.entry(segment.to_string()).or_default();
        }
        node.value = Some(value.clone());
    }
    root
}

fn write_schema_object(output: &mut String, node: &TemplateNode) -> Result<(), CatalogError> {
    output.push_str("{");
    for (name, child) in &node.children {
        output.push_str(name);
        output.push_str(" = ");
        if let Some(value) = &child.value {
            output.push_str("String default ");
            output.push_str(&serde_json::to_string(value)?);
        } else {
            write_schema_object(output, child)?;
            output.push_str(" default {}");
        }
        output.push_str("; ");
    }
    output.push('}');
    Ok(())
}

fn catalog_digest(templates: &BTreeMap<String, String>) -> Result<String, CatalogError> {
    Ok(digest_bytes(&serde_json::to_vec(templates)?))
}

fn validate_prompt_templates(templates: &BTreeMap<String, String>) -> Result<(), CatalogError> {
    config_source::validate_static_template_catalog(templates)
        .map_err(CatalogError::InvalidTemplateCatalog)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_dcdl_catalog_covers_worker_prompts() {
        let catalog = PromptCatalog::builtins_only().unwrap();
        for prompt in WorkerPrompt::ALL {
            assert!(catalog.projection.templates.contains_key(prompt.key()));
        }
        assert!(catalog.projection.templates.contains_key("default"));
        assert!(
            catalog
                .projection
                .templates
                .contains_key("common.workspace")
        );
        assert!(catalog.projection.templates.contains_key("role.coder"));
        assert!(
            catalog
                .projection
                .templates
                .contains_key("panel.orchestrator_idle_queue_notice")
        );
    }

    #[test]
    fn builtin_render_resolves_catalog_root_dotted_includes() {
        let catalog = PromptCatalog::builtins_only().unwrap();
        let source = &catalog.projection.templates["default"];
        assert!(source.contains("{% include \"common.workspace\" %}"));
        assert!(source.contains("{% include \"common.tool_usage\" %}"));
    }

    #[test]
    fn schema_is_closed_and_materializes_builtin_defaults() {
        let source = prompt_schema_source().unwrap();
        assert!(source.starts_with("{ prompts = {"));
        assert!(source.contains("compact_system = String default"));
        assert!(source.contains("role = {"));
    }

    #[test]
    fn graph_rejects_dynamic_legacy_missing_and_cycles() {
        let invalid = BTreeMap::from([
            ("a".into(), "{% include target %}".into()),
            ("target".into(), "ok".into()),
        ]);
        assert!(validate_prompt_templates(&invalid).is_err());

        let legacy = BTreeMap::from([("a".into(), "{% include \"legacy/default\" %}".into())]);
        assert!(validate_prompt_templates(&legacy).is_err());

        let missing = BTreeMap::from([("a".into(), "{% include \"missing\" %}".into())]);
        assert!(validate_prompt_templates(&missing).is_err());

        let cycle = BTreeMap::from([
            ("a".into(), "{% include \"b\" %}".into()),
            ("b".into(), "{% include \"a\" %}".into()),
        ]);
        assert!(validate_prompt_templates(&cycle).is_err());
    }

    #[test]
    fn workspace_projection_digest_is_stable_and_verified() {
        let templates = builtin_prompt_templates().unwrap();
        let projection = EffectivePromptCatalog::new(templates, 42, "schema", "toolchain").unwrap();
        projection.verify_digest().unwrap();
        let mut tampered = projection.clone();
        tampered
            .templates
            .insert("default".into(), "tampered".into());
        assert!(matches!(
            tampered.verify_digest(),
            Err(CatalogError::DigestMismatch { .. })
        ));
    }

    #[test]
    fn catalog_source_preserves_workspace_projection_for_subworkers() {
        let mut templates = builtin_prompt_templates().unwrap();
        templates.insert("common.workspace".into(), "CHILD OVERRIDE".into());
        let catalog = PromptCatalog::from_projection(
            EffectivePromptCatalog::new(templates, 9, "schema", "toolchain").unwrap(),
        )
        .unwrap();
        let child = PromptCatalog::load(&catalog.source()).unwrap();
        assert_eq!(child.projection.config_revision, 9);
        assert_eq!(
            child.projection.templates["common.workspace"],
            "CHILD OVERRIDE"
        );
    }

    #[test]
    fn existing_internal_prompt_render_contracts_are_preserved() {
        let catalog = PromptCatalog::builtins_only().unwrap();
        assert!(catalog.compact_system().unwrap().contains("write_summary"));
        assert!(
            catalog
                .memory_extract_system("Japanese")
                .unwrap()
                .contains("`language`: `Japanese`")
        );
        assert!(
            catalog
                .notify_wrapper("changed")
                .unwrap()
                .contains("changed")
        );
        assert!(
            catalog
                .working_boundaries_section("Readable: /a")
                .unwrap()
                .contains("Readable: /a")
        );
        assert!(
            catalog
                .worker_orchestration_guidance_section()
                .unwrap()
                .contains("## SubWorker orchestration")
        );
    }
}
