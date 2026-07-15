//! `MemoryQuery` tool.
//!
//! Performs a case-insensitive substring scan over markdown record
//! files. With a `query` set, returns `{slug, kind, ..., excerpt}` hits
//! with `excerpt_lines` lines of context around each match. With `query`
//! omitted, returns one entry per file (no excerpt) so the agent can
//! enumerate what records exist without knowing what's inside them.
//!
//! - `MemoryQuery` walks `.yoi/memory/{summary.md,decisions/,
//!   requests/}`. `.yoi/memory/_staging/`,
//!   `.yoi/memory/_usage/`, and `.yoi/memory/_logs/` are excluded
//!   by construction.
//!
//! No derived index — the file tree is the source of truth and is
//! re-scanned per call. 出現順: within a file by line order, across
//! files by sorted filename.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use llm_engine::tool::{Tool, ToolDefinition, ToolError, ToolMeta, ToolOutput};
use serde::{Deserialize, Serialize};

use crate::audit::{AuditStatus, RecordUsageAudit, append_record_usage};
use crate::workspace::WorkspaceLayout;

const DEFAULT_RESULT_LIMIT: usize = 20;
const DEFAULT_EXCERPT_LINES: usize = 3;

const MEMORY_QUERY_DESCRIPTION: &str = "Inspect memory records (summary / decisions / \
requests). With `query` set, returns substring hits as `{slug, kind, excerpt}` entries \
with line context. Omit `query` to list every record (one entry per file, no excerpt) \
when you don't yet know what's in there. Result count is capped (configurable via the \
manifest's `[memory]` section). Use the returned `slug` + `kind` with MemoryRead to fetch \
the full record. Workflow and staging directories are not visible.";

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

#[derive(Debug, Serialize)]
struct MemoryRecord {
    slug: String,
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    excerpt: Option<String>,
}

struct MemoryQueryTool {
    layout: WorkspaceLayout,
    config: QueryConfig,
}

#[async_trait]
impl Tool for MemoryQueryTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: llm_engine::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let params: MemoryQueryParams = serde_json::from_str(input_json)
            .map_err(|e| ToolError::InvalidArgument(format!("invalid MemoryQuery input: {e}")))?;
        let needle = match params.query.as_deref() {
            Some(q) => match validate_query(q) {
                Ok(q) => Some(q),
                Err(err) => {
                    let _ = append_record_usage(
                        &self.layout,
                        RecordUsageAudit {
                            op: "query".to_string(),
                            status: AuditStatus::Failed,
                            kind: "memory".to_string(),
                            slug: None,
                            path: None,
                            query: params.query.clone(),
                            result_count: None,
                            reason: Some(err.to_string()),
                        },
                    );
                    return Err(err);
                }
            },
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
        let _ = append_record_usage(
            &self.layout,
            RecordUsageAudit {
                op: "query".to_string(),
                status: AuditStatus::Success,
                kind: "memory".to_string(),
                slug: None,
                path: None,
                query: params.query.clone(),
                result_count: Some(records.len()),
                reason: if records.len() >= limit {
                    Some("result_limit_reached".to_string())
                } else {
                    None
                },
            },
        );
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
        std::fs::create_dir_all(dir.path().join(".yoi/memory/decisions")).unwrap();
        std::fs::create_dir_all(dir.path().join(".yoi/memory/requests")).unwrap();
        std::fs::create_dir_all(dir.path().join(".yoi/memory/_staging")).unwrap();
        (dir, layout)
    }

    fn write_decision(dir: &Path, slug: &str, body: &str) {
        let path = dir.join(".yoi/memory/decisions").join(format!("{slug}.md"));
        let content = format!(
            "---\ncreated_at: {n}\nupdated_at: {n}\nsources: []\nstatus: open\n---\n{body}",
            n = now()
        );
        std::fs::write(path, content).unwrap();
    }

    #[derive(Deserialize)]
    struct OwnedMemoryRecord {
        slug: String,
        kind: String,
        #[serde(default)]
        excerpt: Option<String>,
    }

    fn parse_records(out: &ToolOutput) -> Vec<OwnedMemoryRecord> {
        let text = out.content.as_ref().unwrap_or(&out.summary);
        serde_json::from_str(text).unwrap()
    }
    #[tokio::test]
    async fn memory_query_finds_decision_body() {
        let (dir, layout) = setup();
        write_decision(dir.path(), "alpha", "we chose Ollama because it works\n");
        write_decision(dir.path(), "beta", "no match here\n");
        let (_, tool) = memory_query_tool(layout, QueryConfig::default())();
        let inp = serde_json::json!({ "query": "ollama" });
        let out = tool
            .execute(&inp.to_string(), Default::default())
            .await
            .unwrap();
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
        let summary_path = dir.path().join(".yoi/memory/summary.md");
        std::fs::write(
            &summary_path,
            format!("---\nupdated_at: {n}\n---\nhello\n", n = now()),
        )
        .unwrap();

        let (_, tool) = memory_query_tool(layout, QueryConfig::default())();
        let out = tool.execute("{}", Default::default()).await.unwrap();
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
        let summary_path = dir.path().join(".yoi/memory/summary.md");
        std::fs::write(
            &summary_path,
            format!("---\nupdated_at: {n}\n---\nthe needle is here\n", n = now()),
        )
        .unwrap();
        let (_, tool) = memory_query_tool(layout, QueryConfig::default())();
        let inp = serde_json::json!({ "query": "needle" });
        let out = tool
            .execute(&inp.to_string(), Default::default())
            .await
            .unwrap();
        let records: Vec<OwnedMemoryRecord> = parse_records(&out);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].slug, "summary");
        assert_eq!(records[0].kind, "summary");
    }

    #[tokio::test]
    async fn query_hits_do_not_log_usage() {
        let (dir, layout) = setup();
        write_decision(dir.path(), "alpha", "needle line\n");
        let (_, memory_tool) = memory_query_tool(layout.clone(), QueryConfig::default())();
        let inp = serde_json::json!({ "query": "needle" });
        memory_tool
            .execute(&inp.to_string(), Default::default())
            .await
            .unwrap();

        let report = crate::usage::build_usage_report(&layout).unwrap();
        assert!(report.records.is_empty());
        assert!(!layout.usage_events_path().exists());
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
        let out = tool
            .execute(&inp.to_string(), Default::default())
            .await
            .unwrap();
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
        let out = tool
            .execute(&inp.to_string(), Default::default())
            .await
            .unwrap();
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
        let err = tool
            .execute(&inp.to_string(), Default::default())
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidArgument(_)));
    }
}
