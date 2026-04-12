//! `Read` tool — read a text file with offset/limit, return line-numbered output.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use llm_worker::tool::{Tool, ToolDefinition, ToolError, ToolMeta, ToolOutput};
use serde::Deserialize;

use crate::scoped_fs::ScopedFs;
use crate::tracker::Tracker;

const DESCRIPTION: &str = "Read a text file from the local filesystem. \
Supports offset/limit for large files. Returns line-numbered output (1-based). \
Directories cannot be read. The file must be read before Write or Edit can \
modify it. Paths must be absolute.";

const DEFAULT_LIMIT: usize = 2000;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(crate) struct ReadParams {
    /// Absolute path to the file.
    pub file_path: PathBuf,
    /// 0-based line offset from the start. Defaults to 0.
    #[serde(default)]
    pub offset: Option<usize>,
    /// Maximum number of lines to return. Defaults to 2000.
    #[serde(default)]
    pub limit: Option<usize>,
}

pub(crate) struct ReadTool {
    fs: ScopedFs,
    tracker: Tracker,
}

#[async_trait]
impl Tool for ReadTool {
    async fn execute(&self, input_json: &str) -> Result<ToolOutput, ToolError> {
        let params: ReadParams = serde_json::from_str(input_json)
            .map_err(|e| ToolError::InvalidArgument(format!("invalid Read input: {e}")))?;
        let offset = params.offset.unwrap_or(0);
        let limit = params.limit.unwrap_or(DEFAULT_LIMIT).max(1);

        tracing::debug!(
            path = %params.file_path.display(),
            offset,
            limit,
            "Read"
        );

        let bytes = self.fs.read_bytes(&params.file_path)?;
        // Record the raw bytes under the read-history so subsequent Edit /
        // Write can detect external modification.
        self.tracker.record(&params.file_path, &bytes);

        let text = String::from_utf8_lossy(&bytes).into_owned();
        let rendered = render_numbered(&text, offset, limit);

        let summary = if rendered.truncated {
            format!(
                "Read {} line(s) [{}..{}] of {} from {}",
                rendered.line_count,
                offset + 1,
                offset + rendered.line_count,
                rendered.total_lines,
                params.file_path.display()
            )
        } else {
            format!(
                "Read {} line(s) from {}",
                rendered.line_count,
                params.file_path.display()
            )
        };

        Ok(ToolOutput {
            summary,
            content: Some(rendered.body),
        })
    }
}

struct Rendered {
    body: String,
    line_count: usize,
    total_lines: usize,
    truncated: bool,
}

/// Format a slice of lines from `text` with `cat -n` style 1-based line
/// numbers. Pure function — no I/O, no history touching.
fn render_numbered(text: &str, offset: usize, limit: usize) -> Rendered {
    let all_lines: Vec<&str> = text.lines().collect();
    let total_lines = all_lines.len();
    let start = offset.min(total_lines);
    let end = start.saturating_add(limit).min(total_lines);
    let slice = &all_lines[start..end];
    let line_count = slice.len();

    use std::fmt::Write as _;
    let mut body = String::with_capacity(text.len().saturating_add(line_count * 8));
    for (i, line) in slice.iter().enumerate() {
        let lineno = start + i + 1;
        let _ = writeln!(&mut body, "{:>6}\t{}", lineno, line);
    }

    Rendered {
        body,
        line_count,
        total_lines,
        truncated: start > 0 || end < total_lines,
    }
}

/// Factory for the `Read` tool.
pub fn read_tool(fs: ScopedFs, tracker: Tracker) -> ToolDefinition {
    Arc::new(move || {
        let schema = schemars::schema_for!(ReadParams);
        let schema_value = serde_json::to_value(schema).unwrap_or(serde_json::json!({}));
        let meta = ToolMeta::new("Read")
            .description(DESCRIPTION)
            .input_schema(schema_value);
        let tool: Arc<dyn Tool> = Arc::new(ReadTool {
            fs: fs.clone(),
            tracker: tracker.clone(),
        });
        (meta, tool)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use manifest::Scope;
    use tempfile::TempDir;

    fn setup() -> (TempDir, ScopedFs, Tracker) {
        let dir = TempDir::new().unwrap();
        let fs = ScopedFs::new(Scope::new(dir.path()).unwrap());
        (dir, fs, Tracker::new())
    }

    #[tokio::test]
    async fn read_tool_basic_records_history() {
        let (dir, fs, tracker) = setup();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, "alpha\nbeta\ngamma\n").unwrap();

        let def = read_tool(fs, tracker.clone());
        let (meta, tool) = def();
        assert_eq!(meta.name, "Read");

        let input = serde_json::json!({ "file_path": file.to_str().unwrap() });
        let out = tool.execute(&input.to_string()).await.unwrap();
        assert!(out.summary.contains("Read 3 line(s)"));
        let body = out.content.unwrap();
        assert!(body.contains("     1\talpha"));
        assert!(body.contains("     3\tgamma"));

        // History recorded
        assert!(tracker.has(&file));
    }

    #[tokio::test]
    async fn read_tool_offset_limit() {
        let (dir, fs, tracker) = setup();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, "1\n2\n3\n4\n5\n").unwrap();

        let def = read_tool(fs, tracker);
        let (_, tool) = def();
        let input = serde_json::json!({
            "file_path": file.to_str().unwrap(),
            "offset": 1,
            "limit": 2,
        });
        let out = tool.execute(&input.to_string()).await.unwrap();
        assert!(out.summary.contains("[2..3] of 5"));
        let body = out.content.unwrap();
        assert!(body.contains("     2\t2"));
        assert!(body.contains("     3\t3"));
    }

    #[tokio::test]
    async fn read_tool_missing_file() {
        let (dir, fs, tracker) = setup();
        let def = read_tool(fs, tracker);
        let (_, tool) = def();
        let input = serde_json::json!({
            "file_path": dir.path().join("nope.txt").to_str().unwrap()
        });
        let err = tool.execute(&input.to_string()).await.unwrap_err();
        assert!(matches!(err, ToolError::ExecutionFailed(_)));
    }

    #[tokio::test]
    async fn read_tool_bad_json() {
        let (_dir, fs, tracker) = setup();
        let def = read_tool(fs, tracker);
        let (_, tool) = def();
        let err = tool.execute("not json").await.unwrap_err();
        assert!(matches!(err, ToolError::InvalidArgument(_)));
    }
}
