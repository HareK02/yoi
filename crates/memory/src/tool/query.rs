//! `MemoryQuery` / `KnowledgeQuery` tools.
//!
//! Both perform a case-insensitive substring scan over markdown record
//! files. With a `query` set, returns `{slug, kind, ..., excerpt}` hits
//! with `excerpt_lines` lines of context around each match. With `query`
//! omitted, returns one entry per file (no excerpt) so the agent can
//! enumerate what records exist without knowing what's inside them.
//!
//! - `MemoryQuery` walks `.insomnia/memory/{summary.md,decisions/,
//!   requests/}`. `.insomnia/workflow/` and `.insomnia/memory/_staging/`
//!   are excluded by construction.
//! - `KnowledgeQuery` walks `.insomnia/knowledge/*.md` and supports a
//!   `kind` filter against the Knowledge frontmatter's `kind` field.
//!
//! No derived index — the file tree is the source of truth and is
//! re-scanned per call. 出現順: within a file by line order, across
//! files by sorted filename.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use llm_worker::tool::{Tool, ToolDefinition, ToolError, ToolMeta, ToolOutput};
use serde::{Deserialize, Serialize};

use crate::schema::{KnowledgeFrontmatter, split_frontmatter};
use crate::workspace::WorkspaceLayout;

const DEFAULT_RESULT_LIMIT: usize = 20;
const DEFAULT_EXCERPT_LINES: usize = 3;

const MEMORY_QUERY_DESCRIPTION: &str = "Inspect memory records (summary / decisions / \
requests). With `query` set, returns substring hits as `{slug, kind, excerpt}` entries \
with line context. Omit `query` to list every record (one entry per file, no excerpt) \
when you don't yet know what's in there. Result count is capped (configurable via the \
manifest's `[memory]` section). Use the returned `slug` + `kind` with MemoryRead to fetch \
the full record. Workflow and staging directories are not visible.";

const KNOWLEDGE_QUERY_DESCRIPTION: &str = "Inspect knowledge records. With `query` set, \
returns substring hits with line context; omit `query` to list every record (one entry \
per file, no excerpt). Optional `kind` filters by the Knowledge frontmatter's `kind` \
field; records whose frontmatter fails to parse are skipped when `kind` is given. Result \
count is capped (configurable via the manifest's `[memory]` section). Returns \
`{slug, kind, description, model_invokation, excerpt}` entries. Use the returned `slug` \
with MemoryRead (kind=knowledge) for the full record.";

/// Tunables passed in from the manifest.
#[derive(Debug, Clone, Copy)]
pub struct QueryConfig {
    pub result_limit: usize,
    /// Lines of context before and after each matched line. Ignored
    /// when the request omits `query`.
    pub excerpt_lines: usize,
}

impl Default for QueryConfig {
    fn default() -> Self {
        Self {
            result_limit: DEFAULT_RESULT_LIMIT,
            excerpt_lines: DEFAULT_EXCERPT_LINES,
        }
    }
}

impl From<&manifest::MemoryConfig> for QueryConfig {
    fn from(cfg: &manifest::MemoryConfig) -> Self {
        let mut out = Self::default();
        if let Some(n) = cfg.query_result_limit {
            out.result_limit = n;
        }
        if let Some(n) = cfg.query_excerpt_lines {
            out.excerpt_lines = n;
        }
        out
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct MemoryQueryParams {
    /// Optional substring filter. Case-insensitive. Omit to list every
    /// record under the query scope.
    #[serde(default)]
    query: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct KnowledgeQueryParams {
    /// Optional substring filter. Case-insensitive. Omit to list every
    /// knowledge record under the query scope.
    #[serde(default)]
    query: Option<String>,
    /// Optional filter on the Knowledge frontmatter's `kind` field.
    #[serde(default)]
    kind: Option<String>,
}

#[derive(Debug, Serialize)]
struct MemoryRecord {
    slug: String,
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    excerpt: Option<String>,
}

#[derive(Debug, Serialize)]
struct KnowledgeRecord {
    slug: String,
    kind: Option<String>,
    description: Option<String>,
    model_invokation: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    excerpt: Option<String>,
}

struct MemoryQueryTool {
    layout: WorkspaceLayout,
    config: QueryConfig,
}

struct KnowledgeQueryTool {
    layout: WorkspaceLayout,
    config: QueryConfig,
}

#[async_trait]
impl Tool for MemoryQueryTool {
    async fn execute(&self, input_json: &str) -> Result<ToolOutput, ToolError> {
        let params: MemoryQueryParams = serde_json::from_str(input_json)
            .map_err(|e| ToolError::InvalidArgument(format!("invalid MemoryQuery input: {e}")))?;
        let needle = match params.query.as_deref() {
            Some(q) => Some(validate_query(q)?),
            None => None,
        };

        let mut records: Vec<MemoryRecord> = Vec::new();
        let limit = self.config.result_limit;
        let ctx = self.config.excerpt_lines;

        // summary
        if records.len() < limit {
            let summary_path = self.layout.summary_path();
            if summary_path.is_file() {
                collect_memory_records(
                    &summary_path,
                    "summary",
                    "summary",
                    needle.as_deref(),
                    ctx,
                    limit - records.len(),
                    &mut records,
                );
            }
        }

        // decisions
        if records.len() < limit {
            for (path, slug) in list_md_files(&self.layout.decisions_dir()) {
                if records.len() >= limit {
                    break;
                }
                collect_memory_records(
                    &path,
                    &slug,
                    "decision",
                    needle.as_deref(),
                    ctx,
                    limit - records.len(),
                    &mut records,
                );
            }
        }

        // requests
        if records.len() < limit {
            for (path, slug) in list_md_files(&self.layout.requests_dir()) {
                if records.len() >= limit {
                    break;
                }
                collect_memory_records(
                    &path,
                    &slug,
                    "request",
                    needle.as_deref(),
                    ctx,
                    limit - records.len(),
                    &mut records,
                );
            }
        }

        let body = serde_json::to_string_pretty(&records)
            .map_err(|e| ToolError::ExecutionFailed(format!("serialize records: {e}")))?;
        let summary = match params.query.as_deref() {
            Some(q) => format!("{} hit(s) for {q:?}", records.len()),
            None => format!("{} record(s)", records.len()),
        };
        Ok(ToolOutput {
            summary,
            content: Some(body),
        })
    }
}

#[async_trait]
impl Tool for KnowledgeQueryTool {
    async fn execute(&self, input_json: &str) -> Result<ToolOutput, ToolError> {
        let params: KnowledgeQueryParams = serde_json::from_str(input_json).map_err(|e| {
            ToolError::InvalidArgument(format!("invalid KnowledgeQuery input: {e}"))
        })?;
        let needle = match params.query.as_deref() {
            Some(q) => Some(validate_query(q)?),
            None => None,
        };
        let kind_filter = params.kind.as_deref();

        let mut records: Vec<KnowledgeRecord> = Vec::new();
        let limit = self.config.result_limit;
        let ctx = self.config.excerpt_lines;

        for (path, slug) in list_md_files(&self.layout.knowledge_dir()) {
            if records.len() >= limit {
                break;
            }
            let raw = match std::fs::read_to_string(&path) {
                Ok(s) => s,
                Err(_) => continue,
            };
            let fm = parse_knowledge_frontmatter(&raw);

            // kind filter applies to the frontmatter's kind field.
            if let Some(filter) = kind_filter {
                let matches = fm
                    .as_ref()
                    .map(|f| f.kind.as_str() == filter)
                    .unwrap_or(false);
                if !matches {
                    continue;
                }
            }

            let kind = fm.as_ref().map(|f| f.kind.clone());
            let description = fm.as_ref().map(|f| f.description.clone());
            let model_invokation = fm.as_ref().map(|f| f.model_invokation);

            match needle.as_deref() {
                Some(n) => {
                    scan_text(&raw, n, ctx, limit - records.len(), |excerpt| {
                        records.push(KnowledgeRecord {
                            slug: slug.clone(),
                            kind: kind.clone(),
                            description: description.clone(),
                            model_invokation,
                            excerpt: Some(excerpt),
                        });
                    });
                }
                None => {
                    records.push(KnowledgeRecord {
                        slug: slug.clone(),
                        kind,
                        description,
                        model_invokation,
                        excerpt: None,
                    });
                }
            }
        }

        let body = serde_json::to_string_pretty(&records)
            .map_err(|e| ToolError::ExecutionFailed(format!("serialize records: {e}")))?;
        let summary = match params.query.as_deref() {
            Some(q) => format!("{} hit(s) for {q:?}", records.len()),
            None => format!("{} record(s)", records.len()),
        };
        Ok(ToolOutput {
            summary,
            content: Some(body),
        })
    }
}

fn collect_memory_records(
    path: &Path,
    slug: &str,
    kind: &'static str,
    needle_lower: Option<&str>,
    ctx: usize,
    remaining: usize,
    out: &mut Vec<MemoryRecord>,
) {
    if remaining == 0 {
        return;
    }
    match needle_lower {
        Some(n) => {
            scan_file(path, n, ctx, remaining, |excerpt| {
                out.push(MemoryRecord {
                    slug: slug.to_string(),
                    kind,
                    excerpt: Some(excerpt),
                });
            });
        }
        None => {
            out.push(MemoryRecord {
                slug: slug.to_string(),
                kind,
                excerpt: None,
            });
        }
    }
}

fn validate_query(query: &str) -> Result<String, ToolError> {
    if query.trim().is_empty() {
        return Err(ToolError::InvalidArgument(
            "query must not be empty when provided; omit it to list all records".into(),
        ));
    }
    Ok(query.to_lowercase())
}

/// Sorted list of `(path, slug)` for `*.md` files directly under `dir`.
/// Returns empty if the directory doesn't exist.
fn list_md_files(dir: &Path) -> Vec<(PathBuf, String)> {
    let mut out: Vec<(PathBuf, String)> = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(it) => it,
        Err(_) => return out,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };
        let slug = match name.strip_suffix(".md") {
            Some(s) => s.to_string(),
            None => continue,
        };
        out.push((path, slug));
    }
    out.sort_by(|a, b| a.1.cmp(&b.1));
    out
}

fn scan_file(
    path: &Path,
    needle_lower: &str,
    ctx: usize,
    remaining: usize,
    mut on_match: impl FnMut(String),
) {
    if remaining == 0 {
        return;
    }
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(_) => return,
    };
    scan_text(&text, needle_lower, ctx, remaining, |e| on_match(e));
}

fn scan_text(
    text: &str,
    needle_lower: &str,
    ctx: usize,
    remaining: usize,
    mut on_match: impl FnMut(String),
) {
    if remaining == 0 {
        return;
    }
    let lines: Vec<&str> = text.lines().collect();
    let mut produced = 0;
    for (i, line) in lines.iter().enumerate() {
        if produced >= remaining {
            break;
        }
        if line.to_lowercase().contains(needle_lower) {
            let start = i.saturating_sub(ctx);
            let end = i.saturating_add(ctx + 1).min(lines.len());
            let excerpt = lines[start..end].join("\n");
            on_match(excerpt);
            produced += 1;
        }
    }
}

/// Best-effort frontmatter parse. Returns `None` if missing/malformed
/// — query still finds matches in the body even when the header is
/// broken.
fn parse_knowledge_frontmatter(raw: &str) -> Option<KnowledgeFrontmatter> {
    let (yaml, _body) = split_frontmatter(raw).ok()?;
    serde_yaml::from_str::<KnowledgeFrontmatter>(yaml).ok()
}

pub fn memory_query_tool(layout: WorkspaceLayout, config: QueryConfig) -> ToolDefinition {
    Arc::new(move || {
        let schema = schemars::schema_for!(MemoryQueryParams);
        let schema_value = serde_json::to_value(schema).unwrap_or(serde_json::json!({}));
        let meta = ToolMeta::new("MemoryQuery")
            .description(MEMORY_QUERY_DESCRIPTION)
            .input_schema(schema_value);
        let tool: Arc<dyn Tool> = Arc::new(MemoryQueryTool {
            layout: layout.clone(),
            config,
        });
        (meta, tool)
    })
}

pub fn knowledge_query_tool(layout: WorkspaceLayout, config: QueryConfig) -> ToolDefinition {
    Arc::new(move || {
        let schema = schemars::schema_for!(KnowledgeQueryParams);
        let schema_value = serde_json::to_value(schema).unwrap_or(serde_json::json!({}));
        let meta = ToolMeta::new("KnowledgeQuery")
            .description(KNOWLEDGE_QUERY_DESCRIPTION)
            .input_schema(schema_value);
        let tool: Arc<dyn Tool> = Arc::new(KnowledgeQueryTool {
            layout: layout.clone(),
            config,
        });
        (meta, tool)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use tempfile::TempDir;

    fn now() -> String {
        Utc::now().to_rfc3339()
    }

    fn setup() -> (TempDir, WorkspaceLayout) {
        let dir = TempDir::new().unwrap();
        let layout = WorkspaceLayout::new(dir.path().to_path_buf());
        std::fs::create_dir_all(dir.path().join(".insomnia/memory/decisions")).unwrap();
        std::fs::create_dir_all(dir.path().join(".insomnia/memory/requests")).unwrap();
        std::fs::create_dir_all(dir.path().join(".insomnia/memory/_staging")).unwrap();
        std::fs::create_dir_all(dir.path().join(".insomnia/workflow")).unwrap();
        std::fs::create_dir_all(dir.path().join(".insomnia/knowledge")).unwrap();
        (dir, layout)
    }

    fn write_decision(dir: &Path, slug: &str, body: &str) {
        let path = dir
            .join(".insomnia/memory/decisions")
            .join(format!("{slug}.md"));
        let content = format!(
            "---\ncreated_at: {n}\nupdated_at: {n}\nsources: []\nstatus: open\n---\n{body}",
            n = now()
        );
        std::fs::write(path, content).unwrap();
    }

    fn write_knowledge(dir: &Path, slug: &str, kind: &str, description: &str, body: &str) {
        let path = dir.join(".insomnia/knowledge").join(format!("{slug}.md"));
        let content = format!(
            "---\ncreated_at: {n}\nupdated_at: {n}\nkind: {kind}\ndescription: \"{description}\"\nmodel_invokation: false\nuser_invocable: true\nlast_sources: []\n---\n{body}",
            n = now()
        );
        std::fs::write(path, content).unwrap();
    }

    fn parse_records<T: for<'de> serde::Deserialize<'de>>(out: &ToolOutput) -> Vec<T> {
        serde_json::from_str(out.content.as_ref().unwrap()).unwrap()
    }

    #[derive(Deserialize)]
    struct OwnedMemoryRecord {
        slug: String,
        kind: String,
        #[serde(default)]
        excerpt: Option<String>,
    }

    #[derive(Deserialize)]
    struct OwnedKnowledgeRecord {
        slug: String,
        kind: Option<String>,
        description: Option<String>,
        model_invokation: Option<bool>,
        #[serde(default)]
        excerpt: Option<String>,
    }

    #[tokio::test]
    async fn memory_query_finds_decision_body() {
        let (dir, layout) = setup();
        write_decision(dir.path(), "alpha", "we chose Ollama because it works\n");
        write_decision(dir.path(), "beta", "no match here\n");
        let (_, tool) = memory_query_tool(layout, QueryConfig::default())();
        let inp = serde_json::json!({ "query": "ollama" });
        let out = tool.execute(&inp.to_string()).await.unwrap();
        let records: Vec<OwnedMemoryRecord> = parse_records(&out);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].slug, "alpha");
        assert_eq!(records[0].kind, "decision");
        assert!(
            records[0]
                .excerpt
                .as_deref()
                .unwrap()
                .to_lowercase()
                .contains("ollama")
        );
    }

    #[tokio::test]
    async fn memory_query_without_query_lists_all_records() {
        let (dir, layout) = setup();
        write_decision(dir.path(), "alpha", "body\n");
        write_decision(dir.path(), "beta", "body\n");
        let summary_path = dir.path().join(".insomnia/memory/summary.md");
        std::fs::write(
            &summary_path,
            format!("---\nupdated_at: {n}\n---\nhello\n", n = now()),
        )
        .unwrap();

        let (_, tool) = memory_query_tool(layout, QueryConfig::default())();
        let out = tool.execute("{}").await.unwrap();
        let records: Vec<OwnedMemoryRecord> = parse_records(&out);
        let mut slugs: Vec<&str> = records.iter().map(|r| r.slug.as_str()).collect();
        slugs.sort();
        assert_eq!(slugs, vec!["alpha", "beta", "summary"]);
        // No excerpts when listing.
        assert!(records.iter().all(|r| r.excerpt.is_none()));
    }

    #[tokio::test]
    async fn memory_query_finds_summary() {
        let (dir, layout) = setup();
        let summary_path = dir.path().join(".insomnia/memory/summary.md");
        std::fs::write(
            &summary_path,
            format!("---\nupdated_at: {n}\n---\nthe needle is here\n", n = now()),
        )
        .unwrap();
        let (_, tool) = memory_query_tool(layout, QueryConfig::default())();
        let inp = serde_json::json!({ "query": "needle" });
        let out = tool.execute(&inp.to_string()).await.unwrap();
        let records: Vec<OwnedMemoryRecord> = parse_records(&out);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].slug, "summary");
        assert_eq!(records[0].kind, "summary");
    }

    #[tokio::test]
    async fn memory_query_excludes_workflow_and_staging() {
        let (dir, layout) = setup();
        let wf = dir.path().join(".insomnia/workflow/wf.md");
        std::fs::write(&wf, "needle in workflow\n").unwrap();
        let stg = dir.path().join(".insomnia/memory/_staging/abc.json");
        std::fs::write(&stg, "needle in staging\n").unwrap();

        let (_, tool) = memory_query_tool(layout, QueryConfig::default())();
        let inp = serde_json::json!({ "query": "needle" });
        let out = tool.execute(&inp.to_string()).await.unwrap();
        let records: Vec<OwnedMemoryRecord> = parse_records(&out);
        assert!(records.is_empty(), "got records: {:?}", out.content);
    }

    #[tokio::test]
    async fn memory_query_respects_result_limit() {
        let (dir, layout) = setup();
        for i in 0..10 {
            write_decision(dir.path(), &format!("rec-{i}"), "needle line\n");
        }
        let cfg = QueryConfig {
            result_limit: 3,
            excerpt_lines: 1,
        };
        let (_, tool) = memory_query_tool(layout, cfg)();
        let inp = serde_json::json!({ "query": "needle" });
        let out = tool.execute(&inp.to_string()).await.unwrap();
        let records: Vec<OwnedMemoryRecord> = parse_records(&out);
        assert_eq!(records.len(), 3);
    }

    #[tokio::test]
    async fn memory_query_excerpt_includes_context_lines() {
        let (dir, layout) = setup();
        write_decision(
            dir.path(),
            "ctx",
            "line a\nline b\nNEEDLE here\nline d\nline e\n",
        );
        let cfg = QueryConfig {
            result_limit: 5,
            excerpt_lines: 1,
        };
        let (_, tool) = memory_query_tool(layout, cfg)();
        let inp = serde_json::json!({ "query": "needle" });
        let out = tool.execute(&inp.to_string()).await.unwrap();
        let records: Vec<OwnedMemoryRecord> = parse_records(&out);
        assert_eq!(records.len(), 1);
        let e = records[0].excerpt.as_deref().unwrap();
        assert!(e.contains("line b"));
        assert!(e.contains("NEEDLE here"));
        assert!(e.contains("line d"));
        assert!(!e.contains("line a"));
        assert!(!e.contains("line e"));
    }

    #[tokio::test]
    async fn memory_query_blank_query_rejected() {
        let (_dir, layout) = setup();
        let (_, tool) = memory_query_tool(layout, QueryConfig::default())();
        let inp = serde_json::json!({ "query": "   " });
        let err = tool.execute(&inp.to_string()).await.unwrap_err();
        assert!(matches!(err, ToolError::InvalidArgument(_)));
    }

    #[tokio::test]
    async fn knowledge_query_returns_frontmatter_fields() {
        let (dir, layout) = setup();
        write_knowledge(
            dir.path(),
            "policy",
            "policy",
            "the policy doc",
            "Ollama first\n",
        );
        let (_, tool) = knowledge_query_tool(layout, QueryConfig::default())();
        let inp = serde_json::json!({ "query": "ollama" });
        let out = tool.execute(&inp.to_string()).await.unwrap();
        let records: Vec<OwnedKnowledgeRecord> = parse_records(&out);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].slug, "policy");
        assert_eq!(records[0].kind.as_deref(), Some("policy"));
        assert_eq!(records[0].description.as_deref(), Some("the policy doc"));
        assert_eq!(records[0].model_invokation, Some(false));
        assert!(
            records[0]
                .excerpt
                .as_deref()
                .unwrap()
                .to_lowercase()
                .contains("ollama")
        );
    }

    #[tokio::test]
    async fn knowledge_query_without_query_lists_all_records() {
        let (dir, layout) = setup();
        write_knowledge(dir.path(), "p1", "policy", "d1", "body\n");
        write_knowledge(dir.path(), "h1", "howto", "d2", "body\n");

        let (_, tool) = knowledge_query_tool(layout, QueryConfig::default())();
        let out = tool.execute("{}").await.unwrap();
        let records: Vec<OwnedKnowledgeRecord> = parse_records(&out);
        let mut slugs: Vec<&str> = records.iter().map(|r| r.slug.as_str()).collect();
        slugs.sort();
        assert_eq!(slugs, vec!["h1", "p1"]);
        assert!(records.iter().all(|r| r.excerpt.is_none()));
    }

    #[tokio::test]
    async fn knowledge_query_kind_filter() {
        let (dir, layout) = setup();
        write_knowledge(dir.path(), "p1", "policy", "d1", "needle\n");
        write_knowledge(dir.path(), "h1", "howto", "d2", "needle\n");

        let (_, tool) = knowledge_query_tool(layout, QueryConfig::default())();
        let inp = serde_json::json!({ "query": "needle", "kind": "howto" });
        let out = tool.execute(&inp.to_string()).await.unwrap();
        let records: Vec<OwnedKnowledgeRecord> = parse_records(&out);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].slug, "h1");
    }

    #[tokio::test]
    async fn knowledge_query_kind_filter_works_without_query() {
        let (dir, layout) = setup();
        write_knowledge(dir.path(), "p1", "policy", "d1", "body\n");
        write_knowledge(dir.path(), "h1", "howto", "d2", "body\n");

        let (_, tool) = knowledge_query_tool(layout, QueryConfig::default())();
        let inp = serde_json::json!({ "kind": "howto" });
        let out = tool.execute(&inp.to_string()).await.unwrap();
        let records: Vec<OwnedKnowledgeRecord> = parse_records(&out);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].slug, "h1");
        assert!(records[0].excerpt.is_none());
    }

    #[tokio::test]
    async fn knowledge_query_searches_frontmatter_too() {
        let (dir, layout) = setup();
        write_knowledge(dir.path(), "p", "policy", "mentions xyzzy here", "body\n");

        let (_, tool) = knowledge_query_tool(layout, QueryConfig::default())();
        let inp = serde_json::json!({ "query": "xyzzy" });
        let out = tool.execute(&inp.to_string()).await.unwrap();
        let records: Vec<OwnedKnowledgeRecord> = parse_records(&out);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].slug, "p");
    }

    #[tokio::test]
    async fn knowledge_query_no_matches_returns_empty() {
        let (dir, layout) = setup();
        write_knowledge(dir.path(), "p", "policy", "d", "no match\n");
        let (_, tool) = knowledge_query_tool(layout, QueryConfig::default())();
        let inp = serde_json::json!({ "query": "absent" });
        let out = tool.execute(&inp.to_string()).await.unwrap();
        let records: Vec<OwnedKnowledgeRecord> = parse_records(&out);
        assert!(records.is_empty());
    }
}
