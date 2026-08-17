//! System prompt template machinery for the Worker layer.
//!
//! Manifests describe the system prompt body as a reference to a
//! prompt asset (`worker.instruction`, see [`manifest::EngineManifest`]).
//! [`SystemPromptTemplate`] resolves that reference through a
//! [`PromptCatalogSource`], parses the source as a minijinja template, and
//! eagerly syntax-checks it at Worker construction. The final system
//! prompt is materialised exactly once just before the first LLM turn:
//! the rendered body is appended with a fixed trailing section carrying
//! the Worker's `Scope` summary, (if present) the project's `AGENTS.md`
//! contents, resident memory sections, and conditional Worker-orchestration
//! guidance, then the whole string is handed to the Engine via
//! `set_system_prompt`. Subsequent turns and compactions reuse that
//! materialised string verbatim.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::sync::Arc;

use chrono::{DateTime, SecondsFormat, Utc};
use manifest::Scope;
use minijinja::value::Value;
use thiserror::Error;

use crate::feature::{FeatureInstructionDeclaration, dedupe_instruction_contributions};
use crate::prompt::catalog::{CatalogError, PromptCatalog};
use crate::prompt::source::PromptCatalogSource;

#[derive(Debug, Error)]
pub enum SystemPromptError {
    #[error("system prompt template parse error: {0}")]
    Parse(String),
    #[error("system prompt template render error: {0}")]
    Render(String),
    #[error("failed to render trailing section template: {0}")]
    Catalog(#[from] CatalogError),
}

/// Parsed instruction template bound to one immutable effective Prompt catalog.
#[derive(Clone)]
pub struct SystemPromptTemplate {
    catalog: Arc<PromptCatalog>,
    instruction_name: String,
}

impl SystemPromptTemplate {
    /// Resolve an exact catalog-root dotted Prompt name and eagerly verify it.
    pub fn parse(
        instruction_ref: &str,
        loader: PromptCatalogSource,
    ) -> Result<Self, SystemPromptError> {
        let instruction_name = exact_prompt_name(instruction_ref).ok_or_else(|| {
            SystemPromptError::Parse(format!(
                "instruction must be an exact catalog-root dotted Prompt name: {instruction_ref}"
            ))
        })?;
        let catalog = if let Some(projection) = loader.effective_catalog() {
            Arc::new(PromptCatalog::from_projection(projection.clone())?)
        } else {
            PromptCatalog::builtins_only()?
        };
        if !catalog.contains(&instruction_name) {
            return Err(SystemPromptError::Parse(format!(
                "Prompt '{instruction_name}' is not present in the effective catalog"
            )));
        }
        Ok(Self {
            catalog,
            instruction_name,
        })
    }

    /// Render the instruction body and append the fixed trailing
    /// section (scope summary + optional AGENTS.md). The trailing
    /// section is assembled in Rust so that authored templates cannot
    /// accidentally omit the scope boundary or the project instructions.
    pub fn render(&self, ctx: &SystemPromptContext<'_>) -> Result<String, SystemPromptError> {
        let body = self
            .catalog
            .render_name(&self.instruction_name, ctx.to_minijinja_value())
            .map_err(|error| SystemPromptError::Render(error.to_string()))?;
        append_trailing_section(
            &body,
            ctx,
            &self.catalog,
            ctx.scope,
            ctx.agents_md.as_deref(),
            ctx.resident_summary,
        )
    }
}

impl std::fmt::Debug for SystemPromptTemplate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SystemPromptTemplate")
            .field("instruction", &self.instruction_name)
            .finish_non_exhaustive()
    }
}

/// Inputs available to an instruction template at materialisation time.
///
/// Scope summary and AGENTS.md are deliberately **not** exposed to the
/// template — they live in the Rust-owned trailing section so user
/// templates cannot drop them on the floor.
pub struct SystemPromptContext<'a> {
    pub now: DateTime<Utc>,
    pub cwd: Cow<'a, str>,
    /// Language policy exposed to instruction templates as `{{ language }}`.
    pub language: &'a str,
    pub scope: &'a Scope,
    pub tool_names: Vec<String>,
    pub feature_instructions: &'a [FeatureInstructionDeclaration],
    /// Project-level instructions read from the nearest `AGENTS.md`.
    /// Not visible from the template; consumed by the trailing-section
    /// formatter in [`SystemPromptTemplate::render`].
    pub agents_md: Option<String>,
    /// The body of the workspace Memory document. `None` disables the
    /// resident summary section; empty strings are ignored by the trailing-section
    /// formatter.
    pub resident_summary: Option<&'a str>,
    /// Catalog used to render the fixed trailing section headers.
    /// Passed by reference so callers do not give up ownership across
    /// the short-lived render borrow.
    pub prompts: &'a PromptCatalog,
}

impl<'a> SystemPromptContext<'a> {
    fn to_minijinja_value(&self) -> Value {
        let mut root: BTreeMap<String, Value> = BTreeMap::new();
        root.insert(
            "date".into(),
            Value::from(self.now.format("%Y-%m-%d").to_string()),
        );
        root.insert(
            "time".into(),
            Value::from(self.now.format("%H:%M:%S").to_string()),
        );
        root.insert(
            "datetime".into(),
            Value::from(self.now.to_rfc3339_opts(SecondsFormat::Secs, true)),
        );
        root.insert("cwd".into(), Value::from(self.cwd.as_ref()));
        root.insert("language".into(), Value::from(self.language));
        root.insert(
            "tools".into(),
            Value::from(
                self.tool_names
                    .iter()
                    .cloned()
                    .map(Value::from)
                    .collect::<Vec<_>>(),
            ),
        );
        root.insert(
            "tool_capabilities".into(),
            ToolCapabilities::from_tool_names(&self.tool_names).to_minijinja_value(),
        );
        Value::from(root)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct ToolCapabilities {
    memory_query: bool,
    memory_read_document: bool,
    memory_update_document: bool,
    sub_worker_spawn: bool,
    sub_worker_send: bool,
    sub_worker_stop: bool,
    sub_worker_list: bool,
    sub_worker_restore: bool,
}

impl ToolCapabilities {
    fn from_tool_names(names: &[String]) -> Self {
        let mut capabilities = Self::default();
        for name in names {
            match name.as_str() {
                "MemoryQuery" => capabilities.memory_query = true,
                "MemoryReadDocument" => capabilities.memory_read_document = true,
                "MemoryUpdateDocument" => capabilities.memory_update_document = true,
                "SubWorkerSpawn" => capabilities.sub_worker_spawn = true,
                "SubWorkerSend" => capabilities.sub_worker_send = true,
                "SubWorkerStop" => capabilities.sub_worker_stop = true,
                "SubWorkerList" => capabilities.sub_worker_list = true,
                _ => {}
            }
        }
        capabilities
    }

    fn memory_records(self) -> bool {
        self.memory_query || self.memory_read_document || self.memory_update_document
    }

    fn memory_any(self) -> bool {
        self.memory_records()
    }

    fn memory_mutation(self) -> bool {
        self.memory_update_document
    }

    fn sub_worker_management(self) -> bool {
        self.sub_worker_spawn
            || self.sub_worker_send
            || self.sub_worker_stop
            || self.sub_worker_list
            || self.sub_worker_restore
    }

    fn to_minijinja_value(self) -> Value {
        let mut map: BTreeMap<&'static str, Value> = BTreeMap::new();
        map.insert("memory_any", Value::from(self.memory_any()));
        map.insert("memory_records", Value::from(self.memory_records()));
        map.insert("memory_query", Value::from(self.memory_query));
        map.insert(
            "memory_read_document",
            Value::from(self.memory_read_document),
        );
        map.insert(
            "memory_update_document",
            Value::from(self.memory_update_document),
        );
        map.insert("memory_mutation", Value::from(self.memory_mutation()));
        map.insert(
            "sub_worker_management",
            Value::from(self.sub_worker_management()),
        );
        Value::from(map)
    }
}

fn exact_prompt_name(reference: &str) -> Option<String> {
    let candidate = reference.to_string();
    if candidate.is_empty()
        || candidate.split('.').any(|segment| {
            segment.is_empty()
                || !segment.chars().all(|character| {
                    character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_'
                })
        })
    {
        None
    } else {
        Some(candidate)
    }
}

/// Build the final system prompt by appending the fixed trailing
/// section to `body`. The Rust side owns the layout (blank-line
/// separators, trailing-whitespace trim); each section's header + body
/// comes from the prompt catalog (`WorkerPrompt::WorkingBoundariesSection`
/// / `WorkerPrompt::AgentsMdSection`) so that wording can be overridden
/// per-pack without touching this function.
fn append_trailing_section(
    body: &str,
    ctx: &SystemPromptContext<'_>,
    prompts: &PromptCatalog,
    scope: &Scope,
    agents_md: Option<&str>,
    resident_summary: Option<&str>,
) -> Result<String, SystemPromptError> {
    let mut out = String::with_capacity(body.len() + 256);
    out.push_str(body);
    if !body.ends_with('\n') {
        out.push('\n');
    }
    out.push('\n');

    let boundaries = prompts.working_boundaries_section(&scope.summary())?;
    out.push_str(boundaries.trim_end_matches(&['\n', ' '][..]));
    out.push('\n');
    if let Some(agents) = agents_md {
        out.push('\n');
        let section = prompts.agents_md_section(agents)?;
        out.push_str(section.trim_end_matches(&['\n', ' '][..]));
        out.push('\n');
    }
    if let Some(summary) = resident_summary {
        let summary = summary.trim_matches(&['\n', '\r'][..]);
        if !summary.trim().is_empty() {
            out.push('\n');
            let section = prompts.resident_memory_summary_section(summary)?;
            out.push_str(section.trim_end_matches(&['\n', ' '][..]));
            out.push('\n');
        }
    }
    for instruction in dedupe_instruction_contributions(ctx.feature_instructions.iter().cloned()) {
        out.push('\n');
        let prompt_ref = exact_prompt_name(&instruction.prompt_ref).ok_or_else(|| {
            SystemPromptError::Render(format!(
                "feature instruction must be an exact catalog-root dotted Prompt name: {}",
                instruction.prompt_ref
            ))
        })?;
        let section = prompts
            .render_name(&prompt_ref, ctx.to_minijinja_value())
            .map_err(|error| SystemPromptError::Render(error.to_string()))?;
        let section = section.trim_end_matches(&['\n', ' '][..]);
        if !section.trim().is_empty() {
            out.push_str(section);
            out.push('\n');
        }
    }
    // Canonicalise the tail so the emitted prompt has a single form
    // regardless of how individual templates chose to end.
    while out.ends_with('\n') || out.ends_with(' ') {
        out.pop();
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_legacy_prefix_relative_and_missing_names() {
        for reference in ["legacy/custom", "custom.md", "../custom", "missing"] {
            assert!(
                SystemPromptTemplate::parse(reference, PromptCatalogSource::builtins_only())
                    .is_err()
            );
        }
    }
}
