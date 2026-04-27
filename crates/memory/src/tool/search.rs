//! `MemorySearch` / `KnowledgeSearch` tools.
//!
//! Both perform a case-insensitive substring scan over markdown record
//! files, returning a list of `{slug, kind, ..., excerpt}` entries.
//! Excerpts are `excerpt_lines` lines before and after the matched
//! line (so 2N+1 lines per excerpt when not clipped).
//!
//! - `MemorySearch` walks `memory/summary.md`, `memory/decisions/`,
//!   `memory/requests/`. `memory/workflow/` and `memory/_staging/`
//!   are excluded by construction.
//! - `KnowledgeSearch` walks `knowledge/*.md` and supports a `kind`
//!   filter against the Knowledge frontmatter's `kind` field.
//!
//! No derived index — the file tree is the source of truth and is
//! re-scanned per call. grep 出現順: within a file by line order,
//! across files by sorted filename.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use llm_worker::tool::{Tool, ToolDefinition, ToolError, ToolMeta, ToolOutput};
use serde::{Deserialize, Serialize};

use crate::schema::{KnowledgeFrontmatter, split_frontmatter};
use crate::workspace::WorkspaceLayout;

const DEFAULT_HIT_LIMIT: usize = 20;
const DEFAULT_EXCERPT_LINES: usize = 3;

const MEMORY_SEARCH_DESCRIPTION: &str = "Search memory records (summary / decisions / \
requests) for a substring. Returns up to `hit_limit` matches as `{slug, kind, excerpt}` \
entries with line context. Use the returned `slug` + `kind` with MemoryRead to fetch \
the full record. Workflow and staging directories are not searched.";

const KNOWLEDGE_SEARCH_DESCRIPTION: &str = "Search knowledge records for a substring. \
Optional `kind` filters by the Knowledge frontmatter's `kind` field. Returns up to \
`hit_limit` matches as `{slug, kind, description, model_invokation, excerpt}` entries \
with line context. Use the returned `slug` with MemoryRead (kind=knowledge) for the \
full record.";

/// Tunables passed in from the manifest.
#[derive(Debug, Clone, Copy)]
pub struct SearchConfig {
    pub hit_limit: usize,
    /// Lines of context before and after each matched line.
    pub excerpt_lines: usize,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            hit_limit: DEFAULT_HIT_LIMIT,
            excerpt_lines: DEFAULT_EXCERPT_LINES,
        }
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct MemorySearchParams {
    /// Substring to search for. Case-insensitive.
    query: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct KnowledgeSearchParams {
    /// Substring to search for. Case-insensitive.
    query: String,
    /// Optional filter on the Knowledge frontmatter's `kind` field.
    #[serde(default)]
    kind: Option<String>,
}

#[derive(Debug, Serialize)]
struct MemoryHit {
    slug: String,
    kind: &'static str,
    excerpt: String,
}

#[derive(Debug, Serialize)]
struct KnowledgeHit {
    slug: String,
    kind: Option<String>,
    description: Option<String>,
    model_invokation: Option<bool>,
    excerpt: String,
}

struct MemorySearchTool {
    layout: WorkspaceLayout,
    config: SearchConfig,
}

struct KnowledgeSearchTool {
    layout: WorkspaceLayout,
    config: SearchConfig,
}

#[async_trait]
impl Tool for MemorySearchTool {
    async fn execute(&self, input_json: &str) -> Result<ToolOutput, ToolError> {
        let params: MemorySearchParams = serde_json::from_str(input_json).map_err(|e| {
            ToolError::InvalidArgument(format!("invalid MemorySearch input: {e}"))
        })?;
        let needle = validate_query(&params.query)?;

        let mut hits: Vec<MemoryHit> = Vec::new();
        let limit = self.config.hit_limit;
        let ctx = self.config.excerpt_lines;

        // summary
        let summary_path = self.layout.summary_path();
        if summary_path.is_file() {
            scan_file(&summary_path, &needle, ctx, limit - hits.len(), |excerpt| {
                hits.push(MemoryHit {
                    slug: "summary".to_string(),
                    kind: "summary",
                    excerpt,
                });
            });
        }

        // decisions
        if hits.len() < limit {
            for (path, slug) in list_md_files(&self.layout.decisions_dir()) {
                if hits.len() >= limit {
                    break;
                }
                scan_file(&path, &needle, ctx, limit - hits.len(), |excerpt| {
                    hits.push(MemoryHit {
                        slug: slug.clone(),
                        kind: "decision",
                        excerpt,
                    });
                });
            }
        }

        // requests
        if hits.len() < limit {
            for (path, slug) in list_md_files(&self.layout.requests_dir()) {
                if hits.len() >= limit {
                    break;
                }
                scan_file(&path, &needle, ctx, limit - hits.len(), |excerpt| {
                    hits.push(MemoryHit {
                        slug: slug.clone(),
                        kind: "request",
                        excerpt,
                    });
                });
            }
        }

        let body = serde_json::to_string_pretty(&hits)
            .map_err(|e| ToolError::ExecutionFailed(format!("serialize hits: {e}")))?;
        Ok(ToolOutput {
            summary: format!("{} hit(s) for {:?}", hits.len(), params.query),
            content: Some(body),
        })
    }
}

#[async_trait]
impl Tool for KnowledgeSearchTool {
    async fn execute(&self, input_json: &str) -> Result<ToolOutput, ToolError> {
        let params: KnowledgeSearchParams = serde_json::from_str(input_json).map_err(|e| {
            ToolError::InvalidArgument(format!("invalid KnowledgeSearch input: {e}"))
        })?;
        let needle = validate_query(&params.query)?;
        let kind_filter = params.kind.as_deref();

        let mut hits: Vec<KnowledgeHit> = Vec::new();
        let limit = self.config.hit_limit;
        let ctx = self.config.excerpt_lines;

        for (path, slug) in list_md_files(&self.layout.knowledge_dir()) {
            if hits.len() >= limit {
                break;
            }
            // Try to parse frontmatter for description/model_invokation/kind.
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

            scan_text(&raw, &needle, ctx, limit - hits.len(), |excerpt| {
                hits.push(KnowledgeHit {
                    slug: slug.clone(),
                    kind: kind.clone(),
                    description: description.clone(),
                    model_invokation,
                    excerpt,
                });
            });
        }

        let body = serde_json::to_string_pretty(&hits)
            .map_err(|e| ToolError::ExecutionFailed(format!("serialize hits: {e}")))?;
        Ok(ToolOutput {
            summary: format!("{} hit(s) for {:?}", hits.len(), params.query),
            content: Some(body),
        })
    }
}

fn validate_query(query: &str) -> Result<String, ToolError> {
    if query.trim().is_empty() {
        return Err(ToolError::InvalidArgument(
            "query must not be empty".into(),
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
/// — search still finds matches in the body even when the header is
/// broken.
fn parse_knowledge_frontmatter(raw: &str) -> Option<KnowledgeFrontmatter> {
    let (yaml, _body) = split_frontmatter(raw).ok()?;
    serde_yaml::from_str::<KnowledgeFrontmatter>(yaml).ok()
}

pub fn memory_search_tool(layout: WorkspaceLayout, config: SearchConfig) -> ToolDefinition {
    Arc::new(move || {
        let schema = schemars::schema_for!(MemorySearchParams);
        let schema_value = serde_json::to_value(schema).unwrap_or(serde_json::json!({}));
        let meta = ToolMeta::new("MemorySearch")
            .description(MEMORY_SEARCH_DESCRIPTION)
            .input_schema(schema_value);
        let tool: Arc<dyn Tool> = Arc::new(MemorySearchTool {
            layout: layout.clone(),
            config,
        });
        (meta, tool)
    })
}

pub fn knowledge_search_tool(layout: WorkspaceLayout, config: SearchConfig) -> ToolDefinition {
    Arc::new(move || {
        let schema = schemars::schema_for!(KnowledgeSearchParams);
        let schema_value = serde_json::to_value(schema).unwrap_or(serde_json::json!({}));
        let meta = ToolMeta::new("KnowledgeSearch")
            .description(KNOWLEDGE_SEARCH_DESCRIPTION)
            .input_schema(schema_value);
        let tool: Arc<dyn Tool> = Arc::new(KnowledgeSearchTool {
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
        std::fs::create_dir_all(dir.path().join("memory/decisions")).unwrap();
        std::fs::create_dir_all(dir.path().join("memory/requests")).unwrap();
        std::fs::create_dir_all(dir.path().join("memory/workflow")).unwrap();
        std::fs::create_dir_all(dir.path().join("memory/_staging")).unwrap();
        std::fs::create_dir_all(dir.path().join("knowledge")).unwrap();
        (dir, layout)
    }

    fn write_decision(dir: &Path, slug: &str, body: &str) {
        let path = dir.join("memory/decisions").join(format!("{slug}.md"));
        let content = format!(
            "---\ncreated_at: {n}\nupdated_at: {n}\nsources: []\nstatus: open\n---\n{body}",
            n = now()
        );
        std::fs::write(path, content).unwrap();
    }

    fn write_knowledge(dir: &Path, slug: &str, kind: &str, description: &str, body: &str) {
        let path = dir.join("knowledge").join(format!("{slug}.md"));
        let content = format!(
            "---\ncreated_at: {n}\nupdated_at: {n}\nkind: {kind}\ndescription: \"{description}\"\nmodel_invokation: false\nuser_invocable: true\nlast_sources: []\n---\n{body}",
            n = now()
        );
        std::fs::write(path, content).unwrap();
    }

    fn parse_hits<T: for<'de> serde::Deserialize<'de>>(out: &ToolOutput) -> Vec<T> {
        serde_json::from_str(out.content.as_ref().unwrap()).unwrap()
    }

    #[derive(Deserialize)]
    struct OwnedMemoryHit {
        slug: String,
        kind: String,
        excerpt: String,
    }

    #[derive(Deserialize)]
    struct OwnedKnowledgeHit {
        slug: String,
        kind: Option<String>,
        description: Option<String>,
        model_invokation: Option<bool>,
        excerpt: String,
    }

    #[tokio::test]
    async fn memory_search_finds_decision_body() {
        let (dir, layout) = setup();
        write_decision(dir.path(), "alpha", "we chose Ollama because it works\n");
        write_decision(dir.path(), "beta", "no match here\n");
        let (_, tool) = memory_search_tool(layout, SearchConfig::default())();
        let inp = serde_json::json!({ "query": "ollama" });
        let out = tool.execute(&inp.to_string()).await.unwrap();
        let hits: Vec<OwnedMemoryHit> = parse_hits(&out);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].slug, "alpha");
        assert_eq!(hits[0].kind, "decision");
        assert!(hits[0].excerpt.to_lowercase().contains("ollama"));
    }

    #[tokio::test]
    async fn memory_search_finds_summary() {
        let (dir, layout) = setup();
        let summary_path = dir.path().join("memory/summary.md");
        std::fs::write(
            &summary_path,
            format!("---\nupdated_at: {n}\n---\nthe needle is here\n", n = now()),
        )
        .unwrap();
        let (_, tool) = memory_search_tool(layout, SearchConfig::default())();
        let inp = serde_json::json!({ "query": "needle" });
        let out = tool.execute(&inp.to_string()).await.unwrap();
        let hits: Vec<OwnedMemoryHit> = parse_hits(&out);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].slug, "summary");
        assert_eq!(hits[0].kind, "summary");
    }

    #[tokio::test]
    async fn memory_search_excludes_workflow_and_staging() {
        let (dir, layout) = setup();
        // Workflow and staging files contain the needle but must be ignored.
        let wf = dir.path().join("memory/workflow/wf.md");
        std::fs::write(&wf, "needle in workflow\n").unwrap();
        let stg = dir.path().join("memory/_staging/abc.json");
        std::fs::write(&stg, "needle in staging\n").unwrap();

        let (_, tool) = memory_search_tool(layout, SearchConfig::default())();
        let inp = serde_json::json!({ "query": "needle" });
        let out = tool.execute(&inp.to_string()).await.unwrap();
        let hits: Vec<OwnedMemoryHit> = parse_hits(&out);
        assert!(hits.is_empty(), "got hits: {:?}", out.content);
    }

    #[tokio::test]
    async fn memory_search_respects_hit_limit() {
        let (dir, layout) = setup();
        for i in 0..10 {
            write_decision(dir.path(), &format!("rec-{i}"), "needle line\n");
        }
        let cfg = SearchConfig {
            hit_limit: 3,
            excerpt_lines: 1,
        };
        let (_, tool) = memory_search_tool(layout, cfg)();
        let inp = serde_json::json!({ "query": "needle" });
        let out = tool.execute(&inp.to_string()).await.unwrap();
        let hits: Vec<OwnedMemoryHit> = parse_hits(&out);
        assert_eq!(hits.len(), 3);
    }

    #[tokio::test]
    async fn memory_search_excerpt_includes_context_lines() {
        let (dir, layout) = setup();
        write_decision(
            dir.path(),
            "ctx",
            "line a\nline b\nNEEDLE here\nline d\nline e\n",
        );
        let cfg = SearchConfig {
            hit_limit: 5,
            excerpt_lines: 1,
        };
        let (_, tool) = memory_search_tool(layout, cfg)();
        let inp = serde_json::json!({ "query": "needle" });
        let out = tool.execute(&inp.to_string()).await.unwrap();
        let hits: Vec<OwnedMemoryHit> = parse_hits(&out);
        assert_eq!(hits.len(), 1);
        let e = &hits[0].excerpt;
        assert!(e.contains("line b"));
        assert!(e.contains("NEEDLE here"));
        assert!(e.contains("line d"));
        assert!(!e.contains("line a"));
        assert!(!e.contains("line e"));
    }

    #[tokio::test]
    async fn memory_search_empty_query_rejected() {
        let (_dir, layout) = setup();
        let (_, tool) = memory_search_tool(layout, SearchConfig::default())();
        let inp = serde_json::json!({ "query": "   " });
        let err = tool.execute(&inp.to_string()).await.unwrap_err();
        assert!(matches!(err, ToolError::InvalidArgument(_)));
    }

    #[tokio::test]
    async fn knowledge_search_returns_frontmatter_fields() {
        let (dir, layout) = setup();
        write_knowledge(
            dir.path(),
            "policy",
            "policy",
            "the policy doc",
            "Ollama first\n",
        );
        let (_, tool) = knowledge_search_tool(layout, SearchConfig::default())();
        let inp = serde_json::json!({ "query": "ollama" });
        let out = tool.execute(&inp.to_string()).await.unwrap();
        let hits: Vec<OwnedKnowledgeHit> = parse_hits(&out);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].slug, "policy");
        assert_eq!(hits[0].kind.as_deref(), Some("policy"));
        assert_eq!(hits[0].description.as_deref(), Some("the policy doc"));
        assert_eq!(hits[0].model_invokation, Some(false));
        assert!(hits[0].excerpt.to_lowercase().contains("ollama"));
    }

    #[tokio::test]
    async fn knowledge_search_kind_filter() {
        let (dir, layout) = setup();
        write_knowledge(dir.path(), "p1", "policy", "d1", "needle\n");
        write_knowledge(dir.path(), "h1", "howto", "d2", "needle\n");

        let (_, tool) = knowledge_search_tool(layout, SearchConfig::default())();
        let inp = serde_json::json!({ "query": "needle", "kind": "howto" });
        let out = tool.execute(&inp.to_string()).await.unwrap();
        let hits: Vec<OwnedKnowledgeHit> = parse_hits(&out);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].slug, "h1");
    }

    #[tokio::test]
    async fn knowledge_search_searches_frontmatter_too() {
        // Spec completion criteria: "frontmatter 含む全文から excerpt 付きでヒットが返る"
        let (dir, layout) = setup();
        write_knowledge(dir.path(), "p", "policy", "mentions xyzzy here", "body\n");

        let (_, tool) = knowledge_search_tool(layout, SearchConfig::default())();
        let inp = serde_json::json!({ "query": "xyzzy" });
        let out = tool.execute(&inp.to_string()).await.unwrap();
        let hits: Vec<OwnedKnowledgeHit> = parse_hits(&out);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].slug, "p");
    }

    #[tokio::test]
    async fn knowledge_search_no_matches_returns_empty() {
        let (dir, layout) = setup();
        write_knowledge(dir.path(), "p", "policy", "d", "no match\n");
        let (_, tool) = knowledge_search_tool(layout, SearchConfig::default())();
        let inp = serde_json::json!({ "query": "absent" });
        let out = tool.execute(&inp.to_string()).await.unwrap();
        let hits: Vec<OwnedKnowledgeHit> = parse_hits(&out);
        assert!(hits.is_empty());
    }
}
