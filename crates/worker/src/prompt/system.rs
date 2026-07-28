//! System prompt template machinery for the Worker layer.
//!
//! Manifests describe the system prompt body as a reference to a
//! prompt asset (`worker.instruction`, see [`manifest::EngineManifest`]).
//! [`SystemPromptTemplate`] resolves that reference through a
//! [`PromptLoader`], parses the source as a minijinja template, and
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
#[cfg(test)]
use std::path::Path;
use std::sync::Arc;

use chrono::{DateTime, SecondsFormat, Utc};
use manifest::Scope;
use minijinja::value::Value;
use minijinja::{Environment, ErrorKind, UndefinedBehavior};
use thiserror::Error;

use crate::prompt::catalog::{CatalogError, PromptCatalog};
use crate::prompt::loader::{LoaderError, PromptLoader, PromptRef};

#[derive(Debug, Error)]
pub enum SystemPromptError {
    #[error("failed to resolve instruction reference: {0}")]
    LoaderResolve(#[source] LoaderError),
    #[error("system prompt template parse error: {0}")]
    Parse(String),
    #[error("system prompt template render error: {0}")]
    Render(String),
    #[error("failed to render trailing section template: {0}")]
    Catalog(#[from] CatalogError),
}

/// Parsed instruction template bound to a prompt loader.
///
/// Holds a minijinja Environment pre-populated with the instruction
/// template registered under its fully-qualified name (`$prefix/path`).
/// Includes are resolved via the loader using a path-join callback that
/// tracks the including template's prefix and directory, so
/// `{% include "sibling" %}` fragments work as expected.
#[derive(Clone)]
pub struct SystemPromptTemplate {
    env: Arc<Environment<'static>>,
    instruction_name: String,
}

impl SystemPromptTemplate {
    /// Parse the instruction asset referenced by `instruction_ref`
    /// using the supplied [`PromptLoader`]. The reference is resolved
    /// at parse time so syntax errors surface immediately.
    pub fn parse(instruction_ref: &str, loader: PromptLoader) -> Result<Self, SystemPromptError> {
        let root_ref = loader
            .parse_ref(instruction_ref, None)
            .map_err(SystemPromptError::LoaderResolve)?;
        let source = loader
            .load(&root_ref)
            .map_err(SystemPromptError::LoaderResolve)?;
        let root_name = root_ref.to_qualified_string();

        let mut env = Environment::new();
        env.set_undefined_behavior(UndefinedBehavior::Strict);

        // Path-join callback: compute the target template name when a
        // template includes another by a possibly-unqualified string.
        // The joined name is then looked up via `set_loader` below.
        let loader_for_join = loader.clone();
        env.set_path_join_callback(move |name, parent| {
            let parent_ref = loader_for_join.parse_ref(parent, None).ok();
            match loader_for_join.parse_ref(name, parent_ref.as_ref()) {
                Ok(r) => r.to_qualified_string().into(),
                // Propagate the raw name on error so set_loader surfaces
                // a proper TemplateNotFound/LoaderError to the caller.
                Err(_) => name.to_string().into(),
            }
        });

        let loader_for_src = loader.clone();
        env.set_loader(move |name| {
            let reference = loader_for_src
                .parse_ref(name, None)
                .map_err(|e| minijinja::Error::new(ErrorKind::TemplateNotFound, e.to_string()))?;
            match loader_for_src.load(&reference) {
                Ok(source) => Ok(Some(source)),
                Err(e) => Err(minijinja::Error::new(
                    ErrorKind::TemplateNotFound,
                    e.to_string(),
                )),
            }
        });

        env.add_template_owned(root_name.clone(), source)
            .map_err(|e| SystemPromptError::Parse(e.to_string()))?;

        Ok(Self {
            env: Arc::new(env),
            instruction_name: root_name,
        })
    }

    /// Render the instruction body and append the fixed trailing
    /// section (scope summary + optional AGENTS.md). The trailing
    /// section is assembled in Rust so that authored templates cannot
    /// accidentally omit the scope boundary or the project instructions.
    pub fn render(&self, ctx: &SystemPromptContext<'_>) -> Result<String, SystemPromptError> {
        let tmpl = self
            .env
            .get_template(&self.instruction_name)
            .map_err(|e| SystemPromptError::Render(e.to_string()))?;
        let body = tmpl
            .render(ctx.to_minijinja_value())
            .map_err(|e| SystemPromptError::Render(e.to_string()))?;
        append_trailing_section(
            &body,
            ctx.prompts,
            ctx.scope,
            ctx.agents_md.as_deref(),
            ctx.resident_summary,
            ToolCapabilities::from_tool_names(&ctx.tool_names),
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
    worker_spawn: bool,
    worker_send: bool,
    worker_read_output: bool,
    worker_stop: bool,
    worker_list: bool,
    worker_restore: bool,
    ticket_any: bool,
}

impl ToolCapabilities {
    fn from_tool_names(names: &[String]) -> Self {
        let mut capabilities = Self::default();
        for name in names {
            match name.as_str() {
                "MemoryQuery" => capabilities.memory_query = true,
                "MemoryReadDocument" => capabilities.memory_read_document = true,
                "MemoryUpdateDocument" => capabilities.memory_update_document = true,
                "SpawnWorker" => capabilities.worker_spawn = true,
                "SendToWorker" => capabilities.worker_send = true,
                "ReadWorkerOutput" => capabilities.worker_read_output = true,
                "StopWorker" => capabilities.worker_stop = true,
                "ListWorkers" => capabilities.worker_list = true,
                "RestoreWorker" => capabilities.worker_restore = true,
                name if name.starts_with("Ticket") => capabilities.ticket_any = true,
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

    fn worker_management(self) -> bool {
        self.worker_spawn
            || self.worker_send
            || self.worker_read_output
            || self.worker_stop
            || self.worker_list
            || self.worker_restore
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
        map.insert("worker_management", Value::from(self.worker_management()));
        map.insert("ticket_any", Value::from(self.ticket_any));
        Value::from(map)
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
    prompts: &PromptCatalog,
    scope: &Scope,
    agents_md: Option<&str>,
    resident_summary: Option<&str>,
    tool_capabilities: ToolCapabilities,
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
    if tool_capabilities.worker_management() {
        out.push('\n');
        let section = prompts.worker_orchestration_guidance_section()?;
        out.push_str(section.trim_end_matches(&['\n', ' '][..]));
        out.push('\n');
    }
    // Canonicalise the tail so the emitted prompt has a single form
    // regardless of how individual templates chose to end.
    while out.ends_with('\n') || out.ends_with(' ') {
        out.pop();
    }
    Ok(out)
}

/// Bridge used by [`Worker::ensure_system_prompt_materialized`] so tests
/// can construct a synthetic context without going through a full Worker.
#[doc(hidden)]
pub fn __instruction_ref_for_tests(raw: &str, loader: &PromptLoader) -> Option<PromptRef> {
    loader.parse_ref(raw, None).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use manifest::{Permission, ScopeConfig, ScopeRule};
    use tempfile::TempDir;

    fn fixed_now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 4, 15, 9, 30, 0).unwrap()
    }

    fn build_scope(dir: &Path) -> Scope {
        let cfg = ScopeConfig {
            allow: vec![ScopeRule {
                target: dir.to_path_buf(),
                permission: Permission::Write,
                recursive: true,
            }],
            deny: Vec::new(),
        };
        Scope::from_config(&cfg).unwrap()
    }

    fn ctx<'a>(
        cwd: &'a Path,
        scope: &'a Scope,
        tools: Vec<String>,
        agents_md: Option<String>,
    ) -> SystemPromptContext<'a> {
        SystemPromptContext {
            now: fixed_now(),
            cwd: cwd.display().to_string().into(),
            language: manifest::defaults::WORKER_LANGUAGE,
            scope,
            tool_names: tools,
            agents_md,
            resident_summary: None,
            prompts: test_prompts(),
        }
    }

    fn ctx_with_summary<'a>(
        cwd: &'a Path,
        scope: &'a Scope,
        summary: Option<&'a str>,
    ) -> SystemPromptContext<'a> {
        SystemPromptContext {
            now: fixed_now(),
            cwd: cwd.display().to_string().into(),
            language: manifest::defaults::WORKER_LANGUAGE,
            scope,
            tool_names: Vec::new(),
            agents_md: None,
            resident_summary: summary,
            prompts: test_prompts(),
        }
    }

    fn ctx_with_resident<'a>(cwd: &'a Path, scope: &'a Scope) -> SystemPromptContext<'a> {
        SystemPromptContext {
            now: fixed_now(),
            cwd: cwd.display().to_string().into(),
            language: manifest::defaults::WORKER_LANGUAGE,
            scope,
            tool_names: Vec::new(),
            agents_md: None,
            resident_summary: None,
            prompts: test_prompts(),
        }
    }

    fn memory_tool_names() -> Vec<String> {
        ["MemoryQuery", "MemoryReadDocument", "MemoryUpdateDocument"]
            .into_iter()
            .map(String::from)
            .collect()
    }

    fn worker_management_tool_names() -> Vec<String> {
        [
            "SpawnWorker",
            "SendToWorker",
            "ReadWorkerOutput",
            "StopWorker",
            "ListWorkers",
            "RestoreWorker",
        ]
        .into_iter()
        .map(String::from)
        .collect()
    }

    fn ticket_tool_names() -> Vec<String> {
        ["TicketList", "TicketShow", "TicketComment"]
            .into_iter()
            .map(String::from)
            .collect()
    }

    /// Lazily-initialised builtin catalog shared across system-prompt
    /// tests, so every `ctx()` can hand out a `&'static PromptCatalog`
    /// reference without forcing test bodies to create one per call.
    fn test_prompts() -> &'static PromptCatalog {
        use std::sync::OnceLock;
        static CELL: OnceLock<Arc<PromptCatalog>> = OnceLock::new();
        CELL.get_or_init(|| PromptCatalog::builtins_only().unwrap())
            .as_ref()
    }

    fn user_loader_with(file_name: &str, body: &str) -> (TempDir, PromptLoader) {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join(file_name), body).unwrap();
        let loader = PromptLoader::new(Some(tmp.path().to_path_buf()), None);
        (tmp, loader)
    }

    #[test]
    fn instruction_default_resolves_to_yoi_default() {
        let loader = PromptLoader::builtins_only();
        let tmpl = SystemPromptTemplate::parse("$yoi/default", loader).unwrap();
        let dir = TempDir::new().unwrap();
        let scope = build_scope(dir.path());
        let rendered = tmpl
            .render(&ctx(dir.path(), &scope, memory_tool_names(), None))
            .unwrap();
        // Builtin default body must expose the tool and language policies.
        assert!(rendered.contains("### Memory"));
        assert!(rendered.contains("small targeted `MemoryQuery`"));
        assert!(rendered.contains("Strong lookup triggers include"));
        assert!(rendered.contains("MemoryReadDocument"));
        assert!(rendered.contains("Do not query memory every turn"));
        assert!(rendered.contains("MemoryUpdateDocument"));
        assert!(rendered.contains("## Language"));
        assert!(rendered.contains("`language`: `match the user's language"));
        // Trailing section must be present.
        assert!(rendered.contains("## Working boundaries"));
        assert!(rendered.contains("Readable:"));
    }

    #[test]
    fn instruction_default_omits_memory_guidance_without_memory_tools() {
        let loader = PromptLoader::builtins_only();
        let tmpl = SystemPromptTemplate::parse("$yoi/default", loader).unwrap();
        let dir = TempDir::new().unwrap();
        let scope = build_scope(dir.path());
        let rendered = tmpl
            .render(&ctx(
                dir.path(),
                &scope,
                vec!["Read".into(), "Edit".into()],
                None,
            ))
            .unwrap();

        assert!(!rendered.contains("### Memory"));
        assert!(!rendered.contains("MemoryQuery"));
        assert!(!rendered.contains("MemoryRead"));
        assert!(!rendered.contains("MemoryWrite"));
        assert!(!rendered.contains("MemoryEdit"));
        assert!(!rendered.contains("MemoryDelete"));
        assert!(rendered.contains("## Language"));
        assert!(rendered.contains("## Working boundaries"));
    }

    #[test]
    fn ticket_guidance_is_included_for_typed_ticket_tools() {
        let loader = PromptLoader::builtins_only();
        let tmpl = SystemPromptTemplate::parse("$yoi/default", loader).unwrap();
        let dir = TempDir::new().unwrap();
        let scope = build_scope(dir.path());
        let rendered = tmpl
            .render(&ctx(dir.path(), &scope, ticket_tool_names(), None))
            .unwrap();

        assert!(rendered.contains("## Ticket workflow"));
        assert!(rendered.contains("available typed Ticket tools as the authority"));
        assert!(rendered.contains("Do not invoke a Ticket CLI"));
        assert!(rendered.contains("Distinguish implementation completion"));
    }

    #[test]
    fn ticket_guidance_is_omitted_without_ticket_tools() {
        let loader = PromptLoader::builtins_only();
        let tmpl = SystemPromptTemplate::parse("$yoi/default", loader).unwrap();
        let dir = TempDir::new().unwrap();
        let scope = build_scope(dir.path());
        let rendered = tmpl
            .render(&ctx(
                dir.path(),
                &scope,
                vec!["Read".into(), "Edit".into()],
                None,
            ))
            .unwrap();

        assert!(!rendered.contains("## Ticket workflow"));
        assert!(!rendered.contains("Do not invoke a Ticket CLI"));
    }

    #[test]
    fn ticket_role_instructions_include_common_ticket_guidance() {
        let loader = PromptLoader::builtins_only();
        let dir = TempDir::new().unwrap();
        let scope = build_scope(dir.path());

        for role in ["intake", "orchestrator", "coder", "reviewer"] {
            let tmpl =
                SystemPromptTemplate::parse(&format!("$yoi/role/{role}"), loader.clone()).unwrap();
            let rendered = tmpl
                .render(&ctx(dir.path(), &scope, ticket_tool_names(), None))
                .unwrap();

            assert!(rendered.contains("## Ticket workflow"), "role: {role}");
            assert!(
                rendered.contains("Do not invoke a Ticket CLI"),
                "role: {role}"
            );
        }
    }

    #[test]
    fn memory_guidance_names_only_available_memory_tools() {
        let loader = PromptLoader::builtins_only();
        let tmpl = SystemPromptTemplate::parse("$yoi/default", loader).unwrap();
        let dir = TempDir::new().unwrap();
        let scope = build_scope(dir.path());
        let rendered = tmpl
            .render(&ctx(
                dir.path(),
                &scope,
                vec!["MemoryQuery".into(), "MemoryReadDocument".into()],
                None,
            ))
            .unwrap();

        assert!(rendered.contains("### Memory"));
        assert!(rendered.contains("small targeted `MemoryQuery`"));
        assert!(rendered.contains("MemoryReadDocument"));
        assert!(!rendered.contains("MemoryUpdateDocument"));
        assert!(!rendered.contains("MemoryEdit"));
        assert!(!rendered.contains("MemoryDelete"));
    }

    #[test]
    fn worker_orchestration_guidance_is_included_for_worker_management_tools() {
        let loader = PromptLoader::builtins_only();
        let tmpl = SystemPromptTemplate::parse("$yoi/default", loader).unwrap();
        let dir = TempDir::new().unwrap();
        let scope = build_scope(dir.path());
        let rendered = tmpl
            .render(&ctx(
                dir.path(),
                &scope,
                worker_management_tool_names(),
                None,
            ))
            .unwrap();

        assert!(rendered.contains("## Worker orchestration"));
        assert!(rendered.contains("spawned Worker notifications are background signals"));
        assert!(rendered.contains("does not need to keep a turn open"));
        assert!(rendered.contains("Do not use `sleep` or polling loops"));
        assert!(rendered.contains("worktree state, diff, and test results"));
        assert!(rendered.contains("not scheduler or auto-maintain authorization"));
        assert!(rendered.contains("bypass user/Ticket authorization"));
    }

    #[test]
    fn worker_orchestration_guidance_is_omitted_without_worker_management_tools() {
        let loader = PromptLoader::builtins_only();
        let tmpl = SystemPromptTemplate::parse("$yoi/default", loader).unwrap();
        let dir = TempDir::new().unwrap();
        let scope = build_scope(dir.path());
        let rendered = tmpl
            .render(&ctx(
                dir.path(),
                &scope,
                vec!["Read".into(), "Edit".into(), "MemoryReadDocument".into()],
                None,
            ))
            .unwrap();

        assert!(!rendered.contains("## Worker orchestration"));
        assert!(!rendered.contains("spawned Worker notifications are background signals"));
        assert!(!rendered.contains("does not need to keep a turn open"));
        assert!(!rendered.contains("Do not use `sleep` or polling loops"));
    }

    #[test]
    fn instruction_prefix_addressing_user() {
        let (_tmp, loader) = user_loader_with("greet.md", "HELLO from {{ cwd }}");
        let tmpl = SystemPromptTemplate::parse("$user/greet", loader).unwrap();
        let dir = TempDir::new().unwrap();
        let scope = build_scope(dir.path());
        let rendered = tmpl.render(&ctx(dir.path(), &scope, vec![], None)).unwrap();
        assert!(rendered.starts_with("HELLO from"));
        assert!(rendered.contains("## Working boundaries"));
    }

    #[test]
    fn instruction_prefix_addressing_workspace() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("ws.md"), "WS {{ date }}").unwrap();
        let loader = PromptLoader::new(None, Some(tmp.path().to_path_buf()));
        let tmpl = SystemPromptTemplate::parse("$workspace/ws", loader).unwrap();
        let dir = TempDir::new().unwrap();
        let scope = build_scope(dir.path());
        let rendered = tmpl.render(&ctx(dir.path(), &scope, vec![], None)).unwrap();
        assert!(rendered.starts_with("WS 2026-04-15"));
    }

    #[test]
    fn include_unqualified_resolves_relative_to_current_prefix() {
        let tmp = TempDir::new().unwrap();
        // parent.md and sibling.md both under the user root.
        std::fs::write(
            tmp.path().join("parent.md"),
            "PARENT\n{% include \"sibling\" %}",
        )
        .unwrap();
        std::fs::write(tmp.path().join("sibling.md"), "SIBLING-BODY").unwrap();
        let loader = PromptLoader::new(Some(tmp.path().to_path_buf()), None);
        let tmpl = SystemPromptTemplate::parse("$user/parent", loader).unwrap();
        let dir = TempDir::new().unwrap();
        let scope = build_scope(dir.path());
        let rendered = tmpl.render(&ctx(dir.path(), &scope, vec![], None)).unwrap();
        assert!(rendered.contains("PARENT"));
        assert!(rendered.contains("SIBLING-BODY"));
    }

    #[test]
    fn include_unqualified_from_subdirectory_resolves_in_same_dir() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir(tmp.path().join("common")).unwrap();
        std::fs::write(
            tmp.path().join("common/header.md"),
            "HEADER\n{% include \"nested\" %}",
        )
        .unwrap();
        std::fs::write(tmp.path().join("common/nested.md"), "NESTED-OK").unwrap();
        let loader = PromptLoader::new(Some(tmp.path().to_path_buf()), None);
        let tmpl = SystemPromptTemplate::parse("$user/common/header", loader).unwrap();
        let dir = TempDir::new().unwrap();
        let scope = build_scope(dir.path());
        let rendered = tmpl.render(&ctx(dir.path(), &scope, vec![], None)).unwrap();
        assert!(rendered.contains("HEADER"));
        assert!(rendered.contains("NESTED-OK"));
    }

    #[test]
    fn include_explicit_prefix_overrides_relative() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("root.md"),
            "U-ROOT\n{% include \"$yoi/common/tool-usage\" %}",
        )
        .unwrap();
        let loader = PromptLoader::new(Some(tmp.path().to_path_buf()), None);
        let tmpl = SystemPromptTemplate::parse("$user/root", loader).unwrap();
        let dir = TempDir::new().unwrap();
        let scope = build_scope(dir.path());
        let rendered = tmpl
            .render(&ctx(
                dir.path(),
                &scope,
                vec!["Read".into(), "Edit".into()],
                None,
            ))
            .unwrap();
        assert!(rendered.contains("U-ROOT"));
        // Pulled in from the builtin tool-usage asset.
        assert!(rendered.contains("Read"));
    }

    #[test]
    fn prefix_with_missing_file_is_hard_error() {
        let loader = PromptLoader::builtins_only();
        let err = SystemPromptTemplate::parse("$yoi/definitely-missing", loader).unwrap_err();
        assert!(matches!(err, SystemPromptError::LoaderResolve(_)));
    }

    #[test]
    fn parse_fails_on_syntax_error() {
        let (_tmp, loader) = user_loader_with("broken.md", "{{ unclosed");
        let err = SystemPromptTemplate::parse("$user/broken", loader).unwrap_err();
        assert!(matches!(err, SystemPromptError::Parse(_)));
    }

    #[test]
    fn render_fails_on_undefined_variable() {
        let (_tmp, loader) = user_loader_with("ghost.md", "{{ ghost }}");
        let tmpl = SystemPromptTemplate::parse("$user/ghost", loader).unwrap();
        let dir = TempDir::new().unwrap();
        let scope = build_scope(dir.path());
        let err = tmpl
            .render(&ctx(dir.path(), &scope, vec![], None))
            .unwrap_err();
        assert!(matches!(err, SystemPromptError::Render(_)));
    }

    #[test]
    fn render_substitutes_date_cwd_tools() {
        let (_tmp, loader) = user_loader_with(
            "vars.md",
            "date={{ date }} cwd={{ cwd }} tools={{ tools | join(',') }}",
        );
        let tmpl = SystemPromptTemplate::parse("$user/vars", loader).unwrap();
        let dir = TempDir::new().unwrap();
        let scope = build_scope(dir.path());
        let rendered = tmpl
            .render(&ctx(
                dir.path(),
                &scope,
                vec!["alpha".into(), "beta".into()],
                None,
            ))
            .unwrap();
        assert!(rendered.contains("date=2026-04-15"));
        assert!(rendered.contains(&format!("cwd={}", dir.path().display())));
        assert!(rendered.contains("tools=alpha,beta"));
    }

    #[test]
    fn trailing_section_always_contains_scope_summary() {
        let (_tmp, loader) = user_loader_with("body.md", "BODY");
        let tmpl = SystemPromptTemplate::parse("$user/body", loader).unwrap();
        let dir = TempDir::new().unwrap();
        let scope = build_scope(dir.path());
        let rendered = tmpl.render(&ctx(dir.path(), &scope, vec![], None)).unwrap();
        assert!(rendered.contains("## Working boundaries"));
        assert!(rendered.contains("Readable:"));
        assert!(rendered.contains("Writable:"));
    }

    #[test]
    fn trailing_section_contains_agents_md_when_present() {
        let (_tmp, loader) = user_loader_with("body.md", "BODY");
        let tmpl = SystemPromptTemplate::parse("$user/body", loader).unwrap();
        let dir = TempDir::new().unwrap();
        let scope = build_scope(dir.path());
        let rendered = tmpl
            .render(&ctx(
                dir.path(),
                &scope,
                vec![],
                Some("PROJECT DOCS".into()),
            ))
            .unwrap();
        assert!(rendered.contains("## Project instructions (AGENTS.md)"));
        assert!(rendered.contains("PROJECT DOCS"));
    }

    #[test]
    fn trailing_section_omits_agents_md_when_absent() {
        let (_tmp, loader) = user_loader_with("body.md", "BODY");
        let tmpl = SystemPromptTemplate::parse("$user/body", loader).unwrap();
        let dir = TempDir::new().unwrap();
        let scope = build_scope(dir.path());
        let rendered = tmpl.render(&ctx(dir.path(), &scope, vec![], None)).unwrap();
        assert!(!rendered.contains("AGENTS.md"));
        assert!(!rendered.contains("Project instructions"));
    }

    #[test]
    fn trailing_section_renders_resident_summary_body() {
        let (_tmp, loader) = user_loader_with("body.md", "BODY");
        let tmpl = SystemPromptTemplate::parse("$user/body", loader).unwrap();
        let dir = TempDir::new().unwrap();
        let scope = build_scope(dir.path());
        let rendered = tmpl
            .render(&ctx_with_summary(
                dir.path(),
                &scope,
                Some("Persistent summary body"),
            ))
            .unwrap();
        assert!(rendered.contains("## Resident memory summary"));
        assert!(rendered.contains("Persistent summary body"));
    }

    #[test]
    fn trailing_section_omits_resident_summary_when_none_or_empty() {
        let (_tmp, loader) = user_loader_with("body.md", "BODY");
        let tmpl = SystemPromptTemplate::parse("$user/body", loader).unwrap();
        let dir = TempDir::new().unwrap();
        let scope = build_scope(dir.path());
        let rendered = tmpl
            .render(&ctx_with_summary(dir.path(), &scope, None))
            .unwrap();
        assert!(!rendered.contains("Resident memory summary"));

        let rendered = tmpl
            .render(&ctx_with_summary(dir.path(), &scope, Some("  \n")))
            .unwrap();
        assert!(!rendered.contains("Resident memory summary"));
    }
}
