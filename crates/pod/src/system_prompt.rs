//! System prompt template machinery for the Pod layer.
//!
//! Manifests describe the system prompt body as a reference to a
//! prompt asset (`worker.instruction`, see [`manifest::WorkerManifest`]).
//! [`SystemPromptTemplate`] resolves that reference through a
//! [`PromptLoader`], parses the source as a minijinja template, and
//! eagerly syntax-checks it at Pod construction. The final system
//! prompt is materialised exactly once just before the first LLM turn:
//! the rendered body is appended with a fixed trailing section carrying
//! the Pod's `Scope` summary and (if present) the project's `AGENTS.md`
//! contents, and the whole string is handed to the Worker via
//! `set_system_prompt`. Subsequent turns and compactions reuse that
//! materialised string verbatim.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use chrono::{DateTime, SecondsFormat, Utc};
use manifest::Scope;
use minijinja::value::Value;
use minijinja::{Environment, ErrorKind, UndefinedBehavior};
use thiserror::Error;

use crate::prompt_loader::{LoaderError, PromptLoader, PromptRef};
use crate::prompts::{CatalogError, PromptCatalog};

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
    pub fn parse(
        instruction_ref: &str,
        loader: PromptLoader,
    ) -> Result<Self, SystemPromptError> {
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
            let parent_ref = loader_for_join
                .parse_ref(parent, None)
                .ok();
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
                Err(e) => Err(minijinja::Error::new(ErrorKind::TemplateNotFound, e.to_string())),
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
        append_trailing_section(&body, ctx.prompts, ctx.scope, ctx.agents_md.as_deref())
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
    pub cwd: &'a Path,
    pub scope: &'a Scope,
    pub tool_names: Vec<String>,
    /// Project-level instructions read from the nearest `AGENTS.md`.
    /// Not visible from the template; consumed by the trailing-section
    /// formatter in [`SystemPromptTemplate::render`].
    pub agents_md: Option<String>,
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
        root.insert("cwd".into(), Value::from(self.cwd.display().to_string()));
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
        Value::from(root)
    }
}

/// Build the final system prompt by appending the fixed trailing
/// section to `body`. The Rust side owns the layout (blank-line
/// separators, trailing-whitespace trim); each section's header + body
/// comes from the prompt catalog (`PodPrompt::WorkingBoundariesSection`
/// / `PodPrompt::AgentsMdSection`) so that wording can be overridden
/// per-pack without touching this function.
pub fn append_trailing_section(
    body: &str,
    prompts: &PromptCatalog,
    scope: &Scope,
    agents_md: Option<&str>,
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
    // Canonicalise the tail so the emitted prompt has a single form
    // regardless of how individual templates chose to end.
    while out.ends_with('\n') || out.ends_with(' ') {
        out.pop();
    }
    Ok(out)
}

/// Bridge used by [`Pod::ensure_system_prompt_materialized`] so tests
/// can construct a synthetic context without going through a full Pod.
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
            cwd,
            scope,
            tool_names: tools,
            agents_md,
            prompts: test_prompts(),
        }
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
    fn instruction_default_resolves_to_insomnia_default() {
        let loader = PromptLoader::builtins_only();
        let tmpl = SystemPromptTemplate::parse("$insomnia/default", loader).unwrap();
        let dir = TempDir::new().unwrap();
        let scope = build_scope(dir.path());
        let rendered = tmpl
            .render(&ctx(dir.path(), &scope, vec!["Read".into()], None))
            .unwrap();
        // Trailing section must be present.
        assert!(rendered.contains("## Working boundaries"));
        assert!(rendered.contains("Readable:"));
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
            "U-ROOT\n{% include \"$insomnia/common/tool-usage\" %}",
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
        let err = SystemPromptTemplate::parse("$insomnia/definitely-missing", loader).unwrap_err();
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
        let err = tmpl.render(&ctx(dir.path(), &scope, vec![], None)).unwrap_err();
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
}
