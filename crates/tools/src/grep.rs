use std::sync::Arc;

use agen::tool::{Tool, ToolDefinition, ToolError, ToolMeta, ToolOutput};
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use workdir::{GrepOutputMode, GrepRequest, WorkdirPath, WorkdirSessionHandle};

use crate::ToolsError;

const DEFAULT_HEAD_LIMIT: usize = 250;

#[derive(Debug, Clone, Copy, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum OutputMode {
    Content,
    #[default]
    FilesWithMatches,
    Count,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct GrepParams {
    pattern: String,
    /// Workdir-relative path, or an absolute path covered by readable scope. Defaults to the Workdir root.
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    glob: Option<String>,
    #[serde(default, rename = "type")]
    file_type: Option<String>,
    #[serde(default)]
    case_insensitive: bool,
    #[serde(default, rename = "-B")]
    before: Option<usize>,
    #[serde(default, rename = "-A")]
    after: Option<usize>,
    #[serde(default, rename = "-C")]
    context: Option<usize>,
    #[serde(default)]
    multiline: bool,
    #[serde(default)]
    output_mode: Option<OutputMode>,
    #[serde(default)]
    head_limit: Option<usize>,
    #[serde(default)]
    offset: Option<usize>,
}

struct GrepTool {
    session: WorkdirSessionHandle,
}

#[async_trait]
impl Tool for GrepTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: agen::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let params: GrepParams = serde_json::from_str(input_json)
            .map_err(|error| ToolError::InvalidArgument(format!("invalid Grep input: {error}")))?;
        let path = match params.path {
            Some(path) => WorkdirPath::new_scoped(&path).map_err(ToolsError::from)?,
            None => WorkdirPath::root(),
        };
        let mode = match params.output_mode.unwrap_or_default() {
            OutputMode::FilesWithMatches => GrepOutputMode::FilesWithMatches,
            OutputMode::Content => GrepOutputMode::Content,
            OutputMode::Count => GrepOutputMode::Count,
        };
        let (before_context, after_context) = params
            .context
            .map(|context| (context, context))
            .unwrap_or((params.before.unwrap_or(0), params.after.unwrap_or(0)));
        let head_limit = params.head_limit.unwrap_or(DEFAULT_HEAD_LIMIT);
        let result = self
            .session
            .grep(GrepRequest {
                pattern: params.pattern,
                path,
                glob: params.glob,
                file_type: params.file_type,
                case_insensitive: params.case_insensitive,
                before_context,
                after_context,
                multiline: params.multiline,
                output_mode: mode,
                limit: head_limit,
                offset: params.offset.unwrap_or(0),
            })
            .await
            .map_err(ToolsError::from)?;

        let summary = if result.match_count == 0 {
            match mode {
                GrepOutputMode::Content => "No matches".to_owned(),
                _ => "No files matched".to_owned(),
            }
        } else {
            match mode {
                GrepOutputMode::FilesWithMatches => {
                    format!("Found matches in {} file(s)", result.matched_files)
                }
                GrepOutputMode::Count => format!(
                    "Found matches in {} file(s), {} total line(s)",
                    result.matched_files, result.match_count
                ),
                GrepOutputMode::Content => format!(
                    "{} matching line(s) in {} file(s)",
                    result.match_count, result.matched_files
                ),
            }
        };
        let summary = if result.truncated {
            format!("{summary} (truncated at {head_limit})")
        } else {
            summary
        };
        Ok(ToolOutput {
            summary,
            content: (!result.output.is_empty()).then_some(result.output),
            attachments: Vec::new(),
        })
    }
}

pub fn grep_tool(session: WorkdirSessionHandle) -> ToolDefinition {
    Arc::new(move || {
        let schema = schemars::schema_for!(GrepParams);
        let meta = ToolMeta::new("Grep")
            .description("Search a Workdir file or directory with a regex. Content results group lines by file; `>` marks matching lines and unmarked lines are context. Directory traversal executes inside the WorkdirSession provider. Results are bounded and Workdir-relative.")
            .input_schema(serde_json::to_value(schema).expect("Grep schema serialization"));
        let tool: Arc<dyn Tool> = Arc::new(GrepTool {
            session: session.clone(),
        });
        (meta, tool)
    })
}
