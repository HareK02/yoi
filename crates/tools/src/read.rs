//! `Read` tool — read a text file with offset/limit, return line-numbered output.

use std::sync::Arc;

use agen::tool::{Tool, ToolDefinition, ToolError, ToolMeta, ToolOutput};
use async_trait::async_trait;
use serde::Deserialize;

use crate::error::ToolsError;
use crate::tracker::Tracker;
use workdir::{WorkdirPath, WorkdirSessionHandle, WorkdirSessionRouter};

const DESCRIPTION: &str = "Read a text file from a Workdir attachment selected by its Worker-local alias. \
Supports offset/limit for large files. Returns line-numbered output (1-based). \
Directories cannot be read. The file must be read before Write or Edit can \
modify it. Paths are Workdir-relative unless an absolute path is explicitly readable. \
When exactly one Workdir is attached, target_workdir may be omitted.";

const DEFAULT_LIMIT: usize = 2000;
const PROVIDER_BYTE_LIMIT: usize = 256 * 1024;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(crate) struct ReadParams {
    /// Worker-local alias of the Workdir attachment to use.
    #[serde(default)]
    pub target_workdir: Option<String>,
    /// Workdir-relative path, or an absolute path covered by readable scope.
    pub file_path: String,
    #[serde(flatten)]
    pub read: fs_operation::text::LineReadArgs,
}

pub(crate) struct ReadTool {
    router: Arc<WorkdirSessionRouter>,
    tracker: Tracker,
}

#[async_trait]
impl Tool for ReadTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: agen::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let params: ReadParams = crate::error::decode_file_input(input_json, "Read")?;
        let selected = crate::routing::resolve_session(
            &self.router,
            params.target_workdir.as_deref(),
            workdir::WorkdirSessionCapability::Read,
        )?;
        let tracker = self
            .tracker
            .scoped_attachment(&selected.alias, selected.generation);
        Ok(execute_read(
            crate::file_target::FileTarget {
                session: selected.session,
                path: WorkdirPath::new_scoped(&params.file_path).map_err(ToolsError::from)?,
                validator: None,
            },
            tracker,
            params,
        )
        .await?
        .output)
    }
}

pub(crate) async fn execute_read(
    target: crate::file_target::FileTarget,
    tracker: Tracker,
    params: ReadParams,
) -> Result<crate::checkout::CheckoutToolOutput, ToolError> {
    let (offset, limit) = params.read.range(DEFAULT_LIMIT);
    let path = &target.path;
    tracing::debug!(path = %path, offset, limit, "Read");
    let (result, validator) = target.read(offset, limit, PROVIDER_BYTE_LIMIT).await?;
    tracker.record_workdir_observation(path, result.content_hash, result.total_lines);
    // Source/response bounds were enforced by the streaming provider. Lossy
    // UTF-8 conversion is the existing Tool presentation contract; rendering
    // below independently bounds its possible byte expansion.
    let text = fs_operation::text::read(
        String::from_utf8_lossy(&result.bytes).into_owned(),
        Default::default(),
    )
    .map_err(crate::error::text_error)?;
    let rendered = render_provider_read(
        &text.content,
        result.start_line,
        result.total_lines,
        result.truncated,
    );
    let summary = if rendered.truncated {
        format!(
            "Read {} line(s) [{}..{}] of {} from {}",
            rendered.line_count,
            offset.saturating_add(1),
            offset.saturating_add(rendered.line_count),
            rendered.total_lines,
            path
        )
    } else {
        format!("Read {} line(s) from {}", rendered.line_count, path)
    };
    Ok(crate::checkout::CheckoutToolOutput {
        output: ToolOutput {
            summary,
            content: Some(rendered.body),
            attachments: Vec::new(),
        },
        paths: Vec::new(),
        listing: None,
        validator,
    })
}

fn render_provider_read(
    text: &str,
    start_line: usize,
    total_lines: usize,
    truncated: bool,
) -> fs_operation::text::Rendered {
    fs_operation::text::render_numbered(
        text,
        start_line,
        total_lines,
        truncated,
        PROVIDER_BYTE_LIMIT,
    )
}

/// Factory for the `Read` tool bound to one compatibility session.
pub fn read_tool(session: WorkdirSessionHandle, tracker: Tracker) -> ToolDefinition {
    routed_read_tool(crate::routing::singleton_router(session), tracker)
}

pub(crate) fn routed_read_tool(
    router: Arc<WorkdirSessionRouter>,
    tracker: Tracker,
) -> ToolDefinition {
    Arc::new(move || {
        let schema = schemars::schema_for!(ReadParams);
        let schema_value = serde_json::to_value(schema).unwrap_or(serde_json::json!({}));
        let meta = ToolMeta::new("Read")
            .description(DESCRIPTION)
            .input_schema(schema_value);
        let tool: Arc<dyn Tool> = Arc::new(ReadTool {
            router: router.clone(),
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
    use workdir::LocalWorkdirSession;

    fn setup() -> (TempDir, WorkdirSessionHandle, Tracker) {
        let dir = TempDir::new().unwrap();
        let session: WorkdirSessionHandle = Arc::new(LocalWorkdirSession::new(
            Scope::writable(dir.path()).unwrap(),
            dir.path().to_path_buf(),
        ));
        (dir, session, Tracker::new())
    }

    #[test]
    fn read_numbering_and_unicode_expansion_keep_rendered_output_bounded() {
        for text in [
            "\n".repeat(PROVIDER_BYTE_LIMIT),
            "界".repeat(PROVIDER_BYTE_LIMIT),
            "\u{1}".repeat(PROVIDER_BYTE_LIMIT),
        ] {
            let rendered = render_provider_read(&text, 0, text.lines().count(), false);
            assert!(rendered.body.len() <= PROVIDER_BYTE_LIMIT);
            assert!(serde_json::to_vec(&rendered.body).unwrap().len() < 2 * 1024 * 1024);
        }
        let rendered = render_provider_read(
            &"\n".repeat(PROVIDER_BYTE_LIMIT),
            0,
            PROVIDER_BYTE_LIMIT,
            false,
        );
        assert!(rendered.truncated);
        assert!(rendered.line_count < PROVIDER_BYTE_LIMIT);
        assert!(
            rendered
                .body
                .contains("truncated at rendered output byte limit")
        );
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
                .scoped_attachment(&workdir::WorkdirAttachmentAlias::new("workdir").unwrap(), 0,)
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
        let (_dir, fs, tracker) = setup();
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
