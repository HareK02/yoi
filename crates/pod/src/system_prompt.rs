//! System prompt template machinery for the Pod layer.
//!
//! Manifests describe `system_prompt` as a minijinja template string.
//! The template is parsed eagerly at `Pod::from_manifest` (syntax check
//! only) and held on the Pod until `ensure_system_prompt_materialized`
//! renders it exactly once, just before the first LLM turn. The rendered
//! string is pushed to the worker via `set_system_prompt` and is reused
//! for every subsequent turn, including after compaction.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use chrono::{DateTime, SecondsFormat, Utc};
use manifest::Scope;
use minijinja::value::Value;
use minijinja::{Environment, UndefinedBehavior};
use thiserror::Error;

const TEMPLATE_NAME: &str = "system_prompt";

#[derive(Debug, Error)]
pub enum SystemPromptError {
    #[error("system prompt template parse error: {0}")]
    Parse(String),
    #[error("system prompt template render error: {0}")]
    Render(String),
}

/// Parsed system-prompt template. Holds a minijinja Environment with a
/// single named template; rendering only needs a fresh [`SystemPromptContext`].
#[derive(Clone)]
pub struct SystemPromptTemplate {
    env: Arc<Environment<'static>>,
}

impl SystemPromptTemplate {
    /// Parse a template source. Performs syntax validation only — no
    /// variable resolution is attempted here.
    pub fn parse(source: impl Into<String>) -> Result<Self, SystemPromptError> {
        let mut env = Environment::new();
        env.set_undefined_behavior(UndefinedBehavior::Strict);
        env.add_template_owned(TEMPLATE_NAME, source.into())
            .map_err(|e| SystemPromptError::Parse(e.to_string()))?;
        Ok(Self { env: Arc::new(env) })
    }

    /// Render the template with the supplied context. Missing variables
    /// surface as [`SystemPromptError::Render`].
    pub fn render(&self, ctx: &SystemPromptContext<'_>) -> Result<String, SystemPromptError> {
        let tmpl = self
            .env
            .get_template(TEMPLATE_NAME)
            .map_err(|e| SystemPromptError::Render(e.to_string()))?;
        tmpl.render(ctx.to_minijinja_value())
            .map_err(|e| SystemPromptError::Render(e.to_string()))
    }
}

impl std::fmt::Debug for SystemPromptTemplate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SystemPromptTemplate")
            .finish_non_exhaustive()
    }
}

/// Inputs available to a system-prompt template at materialisation time.
///
/// `files` is reserved for AGENTS.md and other external file ingestion
/// (supplied by a separate ticket). It is always present so template
/// authors can reference `{{ files.agents_md }}` without having to guard
/// for key existence.
pub struct SystemPromptContext<'a> {
    pub now: DateTime<Utc>,
    pub cwd: &'a Path,
    pub scope: &'a Scope,
    pub tool_names: Vec<String>,
    pub files: BTreeMap<String, String>,
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
        root.insert("scope".into(), scope_value(self.scope));
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
            "files".into(),
            Value::from(
                self.files
                    .iter()
                    .map(|(k, v)| (k.clone(), Value::from(v.clone())))
                    .collect::<BTreeMap<String, Value>>(),
            ),
        );
        Value::from(root)
    }
}

fn scope_value(scope: &Scope) -> Value {
    let readable: Vec<Value> = scope
        .readable_paths()
        .map(|p| Value::from(p.display().to_string()))
        .collect();
    let writable: Vec<Value> = scope
        .writable_paths()
        .map(|p| Value::from(p.display().to_string()))
        .collect();
    let mut obj: BTreeMap<String, Value> = BTreeMap::new();
    obj.insert("readable".into(), Value::from(readable));
    obj.insert("writable".into(), Value::from(writable));
    obj.insert("summary".into(), Value::from(scope.summary()));
    Value::from(obj)
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
        Scope::from_config(&cfg, dir).unwrap()
    }

    fn ctx<'a>(cwd: &'a Path, scope: &'a Scope, tools: Vec<String>) -> SystemPromptContext<'a> {
        SystemPromptContext {
            now: fixed_now(),
            cwd,
            scope,
            tool_names: tools,
            files: BTreeMap::new(),
        }
    }

    #[test]
    fn parse_succeeds_for_minimal_template() {
        let t = SystemPromptTemplate::parse("hello").unwrap();
        let dir = TempDir::new().unwrap();
        let scope = build_scope(dir.path());
        let rendered = t.render(&ctx(dir.path(), &scope, vec![])).unwrap();
        assert_eq!(rendered, "hello");
    }

    #[test]
    fn parse_fails_on_syntax_error() {
        let err = SystemPromptTemplate::parse("{{ unclosed").unwrap_err();
        assert!(matches!(err, SystemPromptError::Parse(_)));
    }

    #[test]
    fn render_substitutes_date_cwd_tools() {
        let t = SystemPromptTemplate::parse(
            "date={{ date }} cwd={{ cwd }} tools={{ tools | join(',') }}",
        )
        .unwrap();
        let dir = TempDir::new().unwrap();
        let scope = build_scope(dir.path());
        let rendered = t
            .render(&ctx(
                dir.path(),
                &scope,
                vec!["alpha".into(), "beta".into()],
            ))
            .unwrap();
        assert!(rendered.contains("date=2026-04-15"));
        assert!(rendered.contains(&format!("cwd={}", dir.path().display())));
        assert!(rendered.contains("tools=alpha,beta"));
    }

    #[test]
    fn render_fails_on_undefined_variable() {
        let t = SystemPromptTemplate::parse("{{ ghost }}").unwrap();
        let dir = TempDir::new().unwrap();
        let scope = build_scope(dir.path());
        let err = t.render(&ctx(dir.path(), &scope, vec![])).unwrap_err();
        assert!(matches!(err, SystemPromptError::Render(_)));
    }

    #[test]
    fn escape_double_braces() {
        let t = SystemPromptTemplate::parse("literal {{ '{{' }} here").unwrap();
        let dir = TempDir::new().unwrap();
        let scope = build_scope(dir.path());
        let rendered = t.render(&ctx(dir.path(), &scope, vec![])).unwrap();
        assert_eq!(rendered, "literal {{ here");
    }

    #[test]
    fn scope_summary_renders() {
        let t = SystemPromptTemplate::parse("{{ scope.summary }}").unwrap();
        let dir = TempDir::new().unwrap();
        let scope = build_scope(dir.path());
        let rendered = t.render(&ctx(dir.path(), &scope, vec![])).unwrap();
        assert!(rendered.starts_with("Readable:"));
        assert!(rendered.contains(&dir.path().canonicalize().unwrap().display().to_string()));
    }

    #[test]
    fn files_reserved_namespace_is_empty() {
        let t = SystemPromptTemplate::parse(
            "{% if files.agents_md is defined %}yes{% else %}no{% endif %}",
        )
        .unwrap();
        let dir = TempDir::new().unwrap();
        let scope = build_scope(dir.path());
        let rendered = t.render(&ctx(dir.path(), &scope, vec![])).unwrap();
        assert_eq!(rendered, "no");
    }
}
