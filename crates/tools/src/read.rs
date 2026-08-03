//! `Read` tool — read a text file with offset/limit, return line-numbered output.

use std::sync::Arc;

use async_trait::async_trait;
use llm_engine::tool::{Tool, ToolDefinition, ToolError, ToolMeta, ToolOutput};
use serde::Deserialize;

use crate::error::ToolsError;
use crate::tracker::Tracker;
use workdir::{ReadRequest, WorkdirHandle, WorkdirPath};

const DESCRIPTION: &str = "Read a text file from the local filesystem. \
Supports offset/limit for large files. Returns line-numbered output (1-based). \
Directories cannot be read. The file must be read before Write or Edit can \
modify it. Paths are relative to the bound Workdir.";

const DEFAULT_LIMIT: usize = 2000;
const PROVIDER_BYTE_LIMIT: usize = 256 * 1024;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(crate) struct ReadParams {
    /// Logical path relative to the bound Workdir root.
    pub file_path: String,
    /// 0-based line offset from the start. Defaults to 0.
    #[serde(default)]
    pub offset: Option<usize>,
    /// Maximum number of lines to return. Defaults to 2000.
    #[serde(default)]
    pub limit: Option<usize>,
}

pub(crate) struct ReadTool {
    workdir: WorkdirHandle,
    tracker: Tracker,
}

#[async_trait]
impl Tool for ReadTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: llm_engine::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let params: ReadParams = serde_json::from_str(input_json)
            .map_err(|e| ToolError::InvalidArgument(format!("invalid Read input: {e}")))?;
        let offset = params.offset.unwrap_or(0);
        let limit = params.limit.unwrap_or(DEFAULT_LIMIT).max(1);

        let path = WorkdirPath::new(&params.file_path).map_err(ToolsError::from)?;
        tracing::debug!(path = %path, offset, limit, "Read");

        let result = self
            .workdir
            .read(ReadRequest {
                path: path.clone(),
                offset,
                limit,
                max_bytes: PROVIDER_BYTE_LIMIT,
            })
            .await
            .map_err(ToolsError::from)?;
        self.tracker.record_workdir_hash(&path, result.content_hash);

        let text = String::from_utf8_lossy(&result.bytes).into_owned();
        let rendered = render_provider_read(
            &text,
            result.start_line,
            result.total_lines,
            result.truncated,
        );

        let summary = if rendered.truncated {
            format!(
                "Read {} line(s) [{}..{}] of {} from {}",
                rendered.line_count,
                offset + 1,
                offset + rendered.line_count,
                rendered.total_lines,
                path
            )
        } else {
            format!("Read {} line(s) from {}", rendered.line_count, path)
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

fn render_provider_read(
    text: &str,
    start_line: usize,
    total_lines: usize,
    truncated: bool,
) -> Rendered {
    use std::fmt::Write as _;
    let lines = text.lines().collect::<Vec<_>>();
    let mut body = String::with_capacity(text.len().saturating_add(lines.len() * 8));
    for (index, line) in lines.iter().enumerate() {
        let _ = writeln!(&mut body, "{:>6}\t{}", start_line + index + 1, line);
    }
    Rendered {
        body,
        line_count: lines.len(),
        total_lines,
        truncated: start_line > 0 || truncated,
    }
}

/// Format a slice of lines from `text` with `cat -n` style 1-based line
/// numbers. Pure function — no I/O, no history touching.
#[cfg(test)]
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
pub fn read_tool(workdir: WorkdirHandle, tracker: Tracker) -> ToolDefinition {
    Arc::new(move || {
        let schema = schemars::schema_for!(ReadParams);
        let schema_value = serde_json::to_value(schema).unwrap_or(serde_json::json!({}));
        let meta = ToolMeta::new("Read")
            .description(DESCRIPTION)
            .input_schema(schema_value);
        let tool: Arc<dyn Tool> = Arc::new(ReadTool {
            workdir: workdir.clone(),
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
    use workdir::LocalWorkdir;

    fn setup() -> (TempDir, WorkdirHandle, Tracker) {
        let dir = TempDir::new().unwrap();
        let workdir: WorkdirHandle = Arc::new(LocalWorkdir::new(
            Scope::writable(dir.path()).unwrap(),
            dir.path().to_path_buf(),
        ));
        (dir, workdir, Tracker::new())
    }

    #[tokio::test]
    async fn read_tool_basic_records_history() {
        let (dir, fs, tracker) = setup();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, "alpha\nbeta\ngamma\n").unwrap();

        let def = read_tool(fs, tracker.clone());
        let (meta, tool) = def();
        assert_eq!(meta.name, "Read");

        let input = serde_json::json!({ "file_path": file.file_name().unwrap().to_str().unwrap() });
        let out = tool
            .execute(&input.to_string(), Default::default())
            .await
            .unwrap();
        assert!(out.summary.contains("Read 3 line(s)"));
        let body = out.content.unwrap();
        assert!(body.contains("     1\talpha"));
        assert!(body.contains("     3\tgamma"));

        // History recorded
        assert!(
            tracker
                .expected_workdir_hash(&WorkdirPath::new("a.txt").unwrap())
                .is_ok()
        );
    }

    #[tokio::test]
    async fn read_tool_offset_limit() {
        let (dir, fs, tracker) = setup();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, "1\n2\n3\n4\n5\n").unwrap();

        let def = read_tool(fs, tracker);
        let (_, tool) = def();
        let input = serde_json::json!({
            "file_path": file.file_name().unwrap().to_str().unwrap(),
            "offset": 1,
            "limit": 2,
        });
        let out = tool
            .execute(&input.to_string(), Default::default())
            .await
            .unwrap();
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
            "file_path": "nope.txt"
        });
        let err = tool
            .execute(&input.to_string(), Default::default())
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::ExecutionFailed(_)));
    }

    #[tokio::test]
    async fn read_tool_bad_json() {
        let (_dir, fs, tracker) = setup();
        let def = read_tool(fs, tracker);
        let (_, tool) = def();
        let err = tool
            .execute("not json", Default::default())
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidArgument(_)));
    }
}
