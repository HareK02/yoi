//! `Grep` tool — recursive regex search powered by ripgrep's component crates.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use grep_regex::RegexMatcherBuilder;
use grep_searcher::sinks::UTF8 as UTF8Sink;
use grep_searcher::{BinaryDetection, Searcher, SearcherBuilder, Sink, SinkContext, SinkMatch};
use ignore::WalkBuilder;
use ignore::overrides::OverrideBuilder;
use ignore::types::TypesBuilder;
use llm_engine::tool::{Tool, ToolDefinition, ToolError, ToolMeta, ToolOutput};
use manifest::Scope;
use serde::Deserialize;

use crate::error::ToolsError;
use crate::scoped_fs::{ScopedFs, direct_symlink};

const DESCRIPTION: &str = "Recursive regex search across files, powered by \
ripgrep. Supports file filtering (`glob`, `type`), context lines, multiline \
matching, and three output modes: `files_with_matches` (default), `content`, \
and `count`. Honors .gitignore. Binary files are skipped. Paths must be \
absolute.";

const DEFAULT_HEAD_LIMIT: usize = 250;

#[derive(Debug, Clone, Copy, Deserialize, schemars::JsonSchema, Default, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum GrepOutputMode {
    #[default]
    FilesWithMatches,
    Content,
    Count,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(crate) struct GrepParams {
    /// Regex pattern to search for.
    pub pattern: String,
    /// Absolute path to search under. Defaults to the scope root.
    #[serde(default)]
    pub path: Option<PathBuf>,
    /// Glob filter applied to candidate files, e.g. `"*.rs"`.
    #[serde(default)]
    pub glob: Option<String>,
    /// File type filter, e.g. `"rust"` or `"py"`. See ripgrep's default types.
    #[serde(default, rename = "type")]
    pub file_type: Option<String>,
    /// Output mode: `files_with_matches` (default), `content`, or `count`.
    #[serde(default)]
    pub output_mode: Option<GrepOutputMode>,
    /// Show line numbers in content mode. Defaults to true.
    #[serde(default, rename = "-n")]
    pub line_numbers: Option<bool>,
    /// Case-insensitive matching.
    #[serde(default, rename = "-i")]
    pub case_insensitive: bool,
    /// Trailing context lines after each match.
    #[serde(default, rename = "-A")]
    pub after: Option<usize>,
    /// Leading context lines before each match.
    #[serde(default, rename = "-B")]
    pub before: Option<usize>,
    /// Context lines before AND after each match (overrides -A/-B when set).
    #[serde(default, rename = "-C")]
    pub context: Option<usize>,
    /// Allow patterns to match across newlines.
    #[serde(default)]
    pub multiline: bool,
    /// Maximum number of output entries. Defaults to 250.
    #[serde(default)]
    pub head_limit: Option<usize>,
    /// Skip the first N output entries (pagination).
    #[serde(default)]
    pub offset: Option<usize>,
}

pub(crate) struct GrepTool {
    fs: ScopedFs,
}

#[async_trait]
impl Tool for GrepTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: llm_engine::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let params: GrepParams = serde_json::from_str(input_json)
            .map_err(|e| ToolError::InvalidArgument(format!("invalid Grep input: {e}")))?;

        tracing::debug!(
            pattern = %params.pattern,
            mode = ?params.output_mode,
            "Grep"
        );

        let default_base = self.fs.cwd().to_path_buf();
        let scope = self.fs.scope().clone();
        let report = tokio::task::spawn_blocking(move || run_grep(default_base, params, &scope))
            .await
            .map_err(|e| ToolError::Internal(format!("spawn_blocking failed: {e}")))??;

        Ok(report.render())
    }
}

/// Factory for the `Grep` tool.
pub fn grep_tool(fs: ScopedFs) -> ToolDefinition {
    Arc::new(move || {
        let schema = schemars::schema_for!(GrepParams);
        let schema_value = serde_json::to_value(schema).unwrap_or(serde_json::json!({}));
        let meta = ToolMeta::new("Grep")
            .description(DESCRIPTION)
            .input_schema(schema_value);
        let tool: Arc<dyn Tool> = Arc::new(GrepTool { fs: fs.clone() });
        (meta, tool)
    })
}

// =============================================================================
// Implementation
// =============================================================================

struct ContentLine {
    path: PathBuf,
    line_number: Option<u64>,
    text: String,
    is_match: bool,
}

struct GrepReport {
    mode: GrepOutputMode,
    show_line_numbers: bool,
    files: Vec<PathBuf>,
    counts: Vec<(PathBuf, usize)>,
    lines: Vec<ContentLine>,
    truncated: bool,
    head_limit: usize,
}

impl GrepReport {
    fn render(self) -> ToolOutput {
        match self.mode {
            GrepOutputMode::FilesWithMatches => {
                if self.files.is_empty() {
                    return ToolOutput {
                        summary: "No files matched".into(),
                        content: None,
                    };
                }
                let mut body = String::new();
                for p in &self.files {
                    body.push_str(&p.display().to_string());
                    body.push('\n');
                }
                let mut summary = format!("Found matches in {} file(s)", self.files.len());
                if self.truncated {
                    summary.push_str(&format!(" (truncated at {})", self.head_limit));
                }
                ToolOutput {
                    summary,
                    content: Some(body),
                }
            }
            GrepOutputMode::Count => {
                if self.counts.is_empty() {
                    return ToolOutput {
                        summary: "No files matched".into(),
                        content: None,
                    };
                }
                let total_lines: usize = self.counts.iter().map(|(_, n)| *n).sum();
                let mut body = String::new();
                for (p, n) in &self.counts {
                    body.push_str(&format!("{}:{}\n", p.display(), n));
                }
                let mut summary = format!(
                    "Found matches in {} file(s), {} total line(s)",
                    self.counts.len(),
                    total_lines
                );
                if self.truncated {
                    summary.push_str(&format!(" (truncated at {})", self.head_limit));
                }
                ToolOutput {
                    summary,
                    content: Some(body),
                }
            }
            GrepOutputMode::Content => {
                if self.lines.is_empty() {
                    return ToolOutput {
                        summary: "No matches".into(),
                        content: None,
                    };
                }
                let match_count = self.lines.iter().filter(|l| l.is_match).count();
                let file_set: std::collections::BTreeSet<&Path> =
                    self.lines.iter().map(|l| l.path.as_path()).collect();
                let mut body = String::new();
                for line in &self.lines {
                    let sep = if line.is_match { ':' } else { '-' };
                    if self.show_line_numbers {
                        if let Some(n) = line.line_number {
                            body.push_str(&format!(
                                "{}{}{}{}{}\n",
                                line.path.display(),
                                sep,
                                n,
                                sep,
                                line.text
                            ));
                            continue;
                        }
                    }
                    body.push_str(&format!("{}{}{}\n", line.path.display(), sep, line.text));
                }
                let mut summary = format!(
                    "{} matching line(s) in {} file(s)",
                    match_count,
                    file_set.len()
                );
                if self.truncated {
                    summary.push_str(&format!(" (truncated at {})", self.head_limit));
                }
                ToolOutput {
                    summary,
                    content: Some(body),
                }
            }
        }
    }
}

fn run_grep(default_base: PathBuf, p: GrepParams, scope: &Scope) -> Result<GrepReport, ToolsError> {
    let matcher = RegexMatcherBuilder::new()
        .case_insensitive(p.case_insensitive)
        .multi_line(p.multiline)
        .dot_matches_new_line(p.multiline)
        .build(&p.pattern)
        .map_err(|e| ToolsError::InvalidRegex(e.to_string()))?;

    let (before, after) = match (p.before, p.after, p.context) {
        (_, _, Some(c)) => (c, c),
        (b, a, None) => (b.unwrap_or(0), a.unwrap_or(0)),
    };

    let mut sb = SearcherBuilder::new();
    sb.binary_detection(BinaryDetection::quit(b'\x00'))
        .line_number(p.line_numbers.unwrap_or(true))
        .multi_line(p.multiline)
        .before_context(before)
        .after_context(after);
    let mut searcher = sb.build();

    let base = p.path.unwrap_or(default_base);
    if !base.is_absolute() {
        return Err(ToolsError::RelativePath(base));
    }
    let symlink = direct_symlink(&base);
    if !scope.is_readable(&base) {
        return Err(if let Some(info) = symlink.as_ref() {
            let link_parent_readable = info
                .link_path
                .parent()
                .map(|parent| scope.is_readable(parent))
                .unwrap_or(false);
            if info.target_exists && link_parent_readable {
                ToolsError::SymlinkOutOfScope {
                    path: base.clone(),
                    target: info.resolved_path.clone(),
                    required_permission: "read",
                }
            } else {
                ToolsError::OutOfScope(base.clone())
            }
        } else {
            ToolsError::OutOfScope(base.clone())
        });
    }
    if let Some(info) = symlink.as_ref() {
        if !info.target_exists {
            return Err(ToolsError::BrokenSymlink {
                path: base.clone(),
                link: info.link_path.clone(),
                target: info.target_path.clone(),
            });
        }
    }
    let base_meta = std::fs::metadata(&base).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => ToolsError::NotFound(base.clone()),
        _ => ToolsError::io(&base, e),
    })?;
    if !base_meta.is_dir() {
        return Err(ToolsError::InvalidArgument(format!(
            "grep search path is not a directory: {}",
            base.display()
        )));
    }
    if let Some(info) = symlink.as_ref() {
        return Err(ToolsError::SymlinkDirectoryNotTraversed {
            tool: "Grep",
            path: base.clone(),
            target: info.resolved_path.clone(),
        });
    }

    let mut wb = WalkBuilder::new(&base);
    wb.hidden(true)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .ignore(true)
        .parents(true)
        .follow_links(false);

    if let Some(t) = p.file_type.as_deref() {
        let mut tb = TypesBuilder::new();
        tb.add_defaults();
        tb.select(t);
        let types = tb
            .build()
            .map_err(|e| ToolsError::InvalidArgument(format!("invalid type {t}: {e}")))?;
        wb.types(types);
    }
    if let Some(g) = p.glob.as_deref() {
        let mut ob = OverrideBuilder::new(&base);
        ob.add(g)
            .map_err(|e| ToolsError::InvalidGlob(e.to_string()))?;
        let ov = ob
            .build()
            .map_err(|e| ToolsError::InvalidGlob(e.to_string()))?;
        wb.overrides(ov);
    }

    let mode = p.output_mode.unwrap_or_default();
    let head_limit = p.head_limit.unwrap_or(DEFAULT_HEAD_LIMIT);
    let offset = p.offset.unwrap_or(0);
    let show_line_numbers = p.line_numbers.unwrap_or(true);

    let mut report = GrepReport {
        mode,
        show_line_numbers,
        files: Vec::new(),
        counts: Vec::new(),
        lines: Vec::new(),
        truncated: false,
        head_limit,
    };

    // Per-mode walker state.
    let mut matching_files_seen: usize = 0;
    let mut matches_seen: usize = 0;

    'walker: for entry in wb.build().flatten() {
        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        let path = entry.path();
        if !scope.is_readable(path) {
            continue;
        }

        match mode {
            GrepOutputMode::FilesWithMatches => {
                let hit = scan_any_match(&mut searcher, &matcher, path)?;
                if !hit {
                    continue;
                }
                if matching_files_seen >= offset {
                    report.files.push(path.to_path_buf());
                    if report.files.len() >= head_limit {
                        report.truncated = true;
                        break 'walker;
                    }
                }
                matching_files_seen += 1;
            }
            GrepOutputMode::Count => {
                let count = scan_count(&mut searcher, &matcher, path)?;
                if count == 0 {
                    continue;
                }
                if matching_files_seen >= offset {
                    report.counts.push((path.to_path_buf(), count));
                    if report.counts.len() >= head_limit {
                        report.truncated = true;
                        break 'walker;
                    }
                }
                matching_files_seen += 1;
            }
            GrepOutputMode::Content => {
                let before_count = matches_seen;
                let mut sink = ContentSink {
                    path: path.to_path_buf(),
                    lines: &mut report.lines,
                    matches_seen: &mut matches_seen,
                    offset,
                    head_limit,
                };
                searcher
                    .search_path(&matcher, path, &mut sink)
                    .map_err(|e| ToolsError::io(path, e))?;
                // If we hit head_limit during this file, stop walking.
                if matches_seen >= offset.saturating_add(head_limit) && matches_seen > before_count
                {
                    report.truncated = true;
                    break 'walker;
                }
            }
        }
    }

    Ok(report)
}

fn scan_any_match(
    searcher: &mut Searcher,
    matcher: &grep_regex::RegexMatcher,
    path: &Path,
) -> Result<bool, ToolsError> {
    let mut hit = false;
    let sink = UTF8Sink(|_, _| {
        hit = true;
        Ok(false) // stop searching this file immediately
    });
    searcher
        .search_path(matcher, path, sink)
        .map_err(|e| ToolsError::io(path, e))?;
    Ok(hit)
}

fn scan_count(
    searcher: &mut Searcher,
    matcher: &grep_regex::RegexMatcher,
    path: &Path,
) -> Result<usize, ToolsError> {
    let mut count = 0usize;
    let sink = UTF8Sink(|_, _| {
        count += 1;
        Ok(true)
    });
    searcher
        .search_path(matcher, path, sink)
        .map_err(|e| ToolsError::io(path, e))?;
    Ok(count)
}

struct ContentSink<'a> {
    path: PathBuf,
    lines: &'a mut Vec<ContentLine>,
    matches_seen: &'a mut usize,
    offset: usize,
    head_limit: usize,
}

impl Sink for ContentSink<'_> {
    type Error = std::io::Error;

    fn matched(&mut self, _searcher: &Searcher, mat: &SinkMatch<'_>) -> Result<bool, Self::Error> {
        let idx = *self.matches_seen;
        *self.matches_seen += 1;

        // Skip matches before offset.
        if idx < self.offset {
            return Ok(true);
        }
        // Stop searching this file once we've filled the head_limit.
        if idx >= self.offset.saturating_add(self.head_limit) {
            return Ok(false);
        }

        let text = String::from_utf8_lossy(mat.bytes())
            .trim_end_matches('\n')
            .trim_end_matches('\r')
            .to_string();
        self.lines.push(ContentLine {
            path: self.path.clone(),
            line_number: mat.line_number(),
            text,
            is_match: true,
        });
        Ok(true)
    }

    fn context(
        &mut self,
        _searcher: &Searcher,
        ctx: &SinkContext<'_>,
    ) -> Result<bool, Self::Error> {
        let seen = *self.matches_seen;
        if seen < self.offset {
            return Ok(true);
        }
        if seen >= self.offset.saturating_add(self.head_limit) {
            return Ok(false);
        }
        let text = String::from_utf8_lossy(ctx.bytes())
            .trim_end_matches('\n')
            .trim_end_matches('\r')
            .to_string();
        self.lines.push(ContentLine {
            path: self.path.clone(),
            line_number: ctx.line_number(),
            text,
            is_match: false,
        });
        Ok(true)
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use manifest::Scope;
    use std::fs;
    use tempfile::TempDir;

    fn setup() -> (TempDir, ScopedFs) {
        let dir = TempDir::new().unwrap();
        let fs = ScopedFs::new(
            Scope::writable(dir.path()).unwrap(),
            dir.path().to_path_buf(),
        );
        (dir, fs)
    }

    fn touch(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    #[tokio::test]
    async fn grep_filters_results_by_scope_readability() {
        use manifest::{Permission, ScopeConfig, ScopeRule};

        let dir = TempDir::new().unwrap();
        let secret_dir = dir.path().join("secret");
        fs::create_dir(&secret_dir).unwrap();
        touch(&dir.path().join("visible.txt"), "needle\n");
        touch(&secret_dir.join("hidden.txt"), "needle\n");

        let cfg = ScopeConfig {
            allow: vec![ScopeRule {
                target: dir.path().to_path_buf(),
                permission: Permission::Write,
                recursive: true,
            }],
            deny: vec![ScopeRule {
                target: secret_dir.clone(),
                permission: Permission::Read,
                recursive: true,
            }],
        };
        let scope = Scope::from_config(&cfg).unwrap();
        let scoped = ScopedFs::new(scope, dir.path().to_path_buf());

        let def = grep_tool(scoped);
        let (_, tool) = def();
        let inp = serde_json::json!({ "pattern": "needle" });
        let out = tool
            .execute(&inp.to_string(), Default::default())
            .await
            .unwrap();
        let body = out.content.unwrap_or_default();
        assert!(body.contains("visible.txt"));
        assert!(
            !body.contains("hidden.txt"),
            "scope-denied file leaked into grep output: {body}"
        );
    }

    #[tokio::test]
    async fn grep_files_with_matches_default() {
        let (dir, fs) = setup();
        touch(&dir.path().join("a.txt"), "alpha\nbravo\n");
        touch(&dir.path().join("b.txt"), "charlie\n");

        let def = grep_tool(fs);
        let (meta, tool) = def();
        assert_eq!(meta.name, "Grep");

        let inp = serde_json::json!({ "pattern": "bravo" });
        let out = tool
            .execute(&inp.to_string(), Default::default())
            .await
            .unwrap();
        assert!(out.summary.contains("1 file"));
        assert!(out.content.unwrap().contains("a.txt"));
    }

    #[tokio::test]
    async fn grep_content_mode_with_line_numbers() {
        let (dir, fs) = setup();
        touch(&dir.path().join("a.txt"), "one\ntwo\nthree\n");

        let def = grep_tool(fs);
        let (_, tool) = def();
        let inp = serde_json::json!({
            "pattern": "two",
            "output_mode": "content",
        });
        let out = tool
            .execute(&inp.to_string(), Default::default())
            .await
            .unwrap();
        let body = out.content.unwrap();
        assert!(body.contains(":2:two"));
    }

    #[tokio::test]
    async fn grep_count_mode() {
        let (dir, fs) = setup();
        touch(&dir.path().join("a.txt"), "x\nx\nx\n");
        touch(&dir.path().join("b.txt"), "x\ny\n");

        let def = grep_tool(fs);
        let (_, tool) = def();
        let inp = serde_json::json!({
            "pattern": "x",
            "output_mode": "count",
        });
        let out = tool
            .execute(&inp.to_string(), Default::default())
            .await
            .unwrap();
        let body = out.content.unwrap();
        assert!(body.contains("a.txt:3"));
        assert!(body.contains("b.txt:1"));
        assert!(out.summary.contains("4 total"));
    }

    #[tokio::test]
    async fn grep_case_insensitive() {
        let (dir, fs) = setup();
        touch(&dir.path().join("a.txt"), "HELLO\n");

        let def = grep_tool(fs);
        let (_, tool) = def();
        let inp = serde_json::json!({
            "pattern": "hello",
            "-i": true,
            "output_mode": "content",
        });
        let out = tool
            .execute(&inp.to_string(), Default::default())
            .await
            .unwrap();
        assert!(out.content.unwrap().contains("HELLO"));
    }

    #[tokio::test]
    async fn grep_context_lines() {
        let (dir, fs) = setup();
        touch(
            &dir.path().join("a.txt"),
            "line1\nline2\nMATCH\nline4\nline5\n",
        );

        let def = grep_tool(fs);
        let (_, tool) = def();
        let inp = serde_json::json!({
            "pattern": "MATCH",
            "output_mode": "content",
            "-C": 1,
        });
        let out = tool
            .execute(&inp.to_string(), Default::default())
            .await
            .unwrap();
        let body = out.content.unwrap();
        // should contain: line2 (before context), MATCH, line4 (after context)
        assert!(body.contains("line2"));
        assert!(body.contains("MATCH"));
        assert!(body.contains("line4"));
        assert!(!body.contains("line1"));
        assert!(!body.contains("line5"));
    }

    #[tokio::test]
    async fn grep_multiline() {
        let (dir, fs) = setup();
        touch(&dir.path().join("a.txt"), "start\nfoo\nbar\nend\n");

        let def = grep_tool(fs);
        let (_, tool) = def();
        // Match across newlines: "foo" followed by "bar" on the next line
        let inp = serde_json::json!({
            "pattern": "foo[\\s\\S]*?bar",
            "multiline": true,
            "output_mode": "content",
        });
        let out = tool
            .execute(&inp.to_string(), Default::default())
            .await
            .unwrap();
        let body = out.content.unwrap();
        assert!(body.contains("foo"));
    }

    #[tokio::test]
    async fn grep_glob_filter() {
        let (dir, fs) = setup();
        touch(&dir.path().join("a.rs"), "target\n");
        touch(&dir.path().join("b.txt"), "target\n");

        let def = grep_tool(fs);
        let (_, tool) = def();
        let inp = serde_json::json!({
            "pattern": "target",
            "glob": "*.rs",
        });
        let out = tool
            .execute(&inp.to_string(), Default::default())
            .await
            .unwrap();
        let body = out.content.unwrap();
        assert!(body.contains("a.rs"));
        assert!(!body.contains("b.txt"));
    }

    #[tokio::test]
    async fn grep_type_filter() {
        let (dir, fs) = setup();
        touch(&dir.path().join("a.rs"), "target\n");
        touch(&dir.path().join("b.py"), "target\n");

        let def = grep_tool(fs);
        let (_, tool) = def();
        let inp = serde_json::json!({
            "pattern": "target",
            "type": "rust",
        });
        let out = tool
            .execute(&inp.to_string(), Default::default())
            .await
            .unwrap();
        let body = out.content.unwrap();
        assert!(body.contains("a.rs"));
        assert!(!body.contains("b.py"));
    }

    #[tokio::test]
    async fn grep_head_limit_truncates() {
        let (dir, fs) = setup();
        for i in 0..5 {
            touch(&dir.path().join(format!("f{i}.txt")), "x\n");
        }

        let def = grep_tool(fs);
        let (_, tool) = def();
        let inp = serde_json::json!({
            "pattern": "x",
            "head_limit": 2,
        });
        let out = tool
            .execute(&inp.to_string(), Default::default())
            .await
            .unwrap();
        let body = out.content.unwrap();
        assert_eq!(body.lines().count(), 2);
        assert!(out.summary.contains("truncated at 2"));
    }

    #[tokio::test]
    async fn grep_offset_paginates() {
        let (dir, fs) = setup();
        // Create 5 files, all matching, deterministically named
        for i in 0..5 {
            touch(&dir.path().join(format!("f{i}.txt")), "x\n");
        }

        let def = grep_tool(fs);
        let (_, tool) = def();
        let inp = serde_json::json!({
            "pattern": "x",
            "offset": 3,
            "head_limit": 10,
        });
        let out = tool
            .execute(&inp.to_string(), Default::default())
            .await
            .unwrap();
        let body = out.content.unwrap();
        // We skipped 3, so only 2 should remain.
        assert_eq!(body.lines().count(), 2);
    }

    #[tokio::test]
    async fn grep_binary_files_are_skipped() {
        let (dir, fs) = setup();
        let mut bin = Vec::from(b"\x00\x01\x02needle\n".as_slice());
        bin.extend(b"more\n");
        fs::write(dir.path().join("a.bin"), bin).unwrap();
        touch(&dir.path().join("b.txt"), "needle\n");

        let def = grep_tool(fs);
        let (_, tool) = def();
        let inp = serde_json::json!({ "pattern": "needle" });
        let out = tool
            .execute(&inp.to_string(), Default::default())
            .await
            .unwrap();
        let body = out.content.unwrap();
        assert!(body.contains("b.txt"));
        assert!(!body.contains("a.bin"));
    }

    #[tokio::test]
    async fn grep_invalid_regex() {
        let (_dir, fs) = setup();
        let def = grep_tool(fs);
        let (_, tool) = def();
        let inp = serde_json::json!({ "pattern": "(" });
        let err = tool
            .execute(&inp.to_string(), Default::default())
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidArgument(_)));
    }

    #[tokio::test]
    async fn grep_unknown_type() {
        let (_dir, fs) = setup();
        let def = grep_tool(fs);
        let (_, tool) = def();
        let inp = serde_json::json!({
            "pattern": "x",
            "type": "nonexistent",
        });
        let err = tool
            .execute(&inp.to_string(), Default::default())
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidArgument(_)));
    }

    #[tokio::test]
    async fn grep_no_matches() {
        let (dir, fs) = setup();
        touch(&dir.path().join("a.txt"), "nothing here\n");
        let def = grep_tool(fs);
        let (_, tool) = def();
        let inp = serde_json::json!({ "pattern": "zzz" });
        let out = tool
            .execute(&inp.to_string(), Default::default())
            .await
            .unwrap();
        assert_eq!(out.summary, "No files matched");
        assert!(out.content.is_none());
    }

    #[test]
    fn grep_schema_contains_dash_keys() {
        // Sanity check: schemars must preserve the `-n`, `-A`, etc. keys
        // from serde(rename). If this fails we need to rename the fields.
        let schema = schemars::schema_for!(GrepParams);
        let json = serde_json::to_value(&schema).unwrap();
        let json_str = json.to_string();
        assert!(json_str.contains("\"-n\""), "schema missing -n: {json_str}");
        assert!(json_str.contains("\"-A\""), "schema missing -A: {json_str}");
        assert!(json_str.contains("\"-B\""), "schema missing -B: {json_str}");
        assert!(json_str.contains("\"-C\""), "schema missing -C: {json_str}");
        assert!(json_str.contains("\"-i\""), "schema missing -i: {json_str}");
    }
}
