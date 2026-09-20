use std::sync::Arc;

use agen::tool::{Tool, ToolDefinition, ToolError, ToolMeta, ToolOutput};
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use workdir::{GlobRequest, WorkdirPath, WorkdirSessionHandle, WorkdirSessionRouter};

use crate::ToolsError;

const RESULT_LIMIT: usize = 1000;

#[derive(Debug, Deserialize, JsonSchema)]
struct GlobParams {
    /// Worker-local alias of the Workdir attachment to use.
    #[serde(default)]
    target_workdir: Option<String>,
    /// Glob pattern, for example `**/*.rs` or `src/**/test_*.py`.
    pattern: String,
    /// Logical Workdir-relative directory. Defaults to the Workdir root.
    #[serde(default)]
    path: Option<String>,
}

struct GlobTool {
    router: Arc<WorkdirSessionRouter>,
}

#[async_trait]
impl Tool for GlobTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: agen::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let params: GlobParams = serde_json::from_str(input_json)
            .map_err(|error| ToolError::InvalidArgument(format!("invalid Glob input: {error}")))?;
        let selected = crate::routing::resolve_session(
            &self.router,
            params.target_workdir.as_deref(),
            workdir::WorkdirSessionCapability::Glob,
        )?;
        let path = match params.path {
            Some(path) => WorkdirPath::new(&path).map_err(ToolsError::from)?,
            None => WorkdirPath::root(),
        };
        let pattern = params.pattern;
        tracing::debug!(%pattern, %path, "Glob");
        let result = selected
            .session
            .glob(GlobRequest {
                pattern: pattern.clone(),
                path,
                limit: RESULT_LIMIT,
            })
            .await
            .map_err(ToolsError::from)?;
        let mut body = result
            .paths
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        if !body.is_empty() {
            body.push('\n');
        }
        let summary = if result.paths.is_empty() {
            format!("No files found matching {pattern}")
        } else if result.truncated {
            format!(
                "Found {}+ files matching {pattern} (truncated to {RESULT_LIMIT})",
                result.paths.len()
            )
        } else {
            format!("Found {} file(s) matching {pattern}", result.paths.len())
        };
        Ok(ToolOutput {
            summary,
            content: (!body.is_empty()).then_some(body),
            attachments: Vec::new(),
        })
    }
}

pub fn glob_tool(session: WorkdirSessionHandle) -> ToolDefinition {
    routed_glob_tool(crate::routing::singleton_router(session))
}

pub(crate) fn routed_glob_tool(router: Arc<WorkdirSessionRouter>) -> ToolDefinition {
    Arc::new(move || {
        let schema = schemars::schema_for!(GlobParams);
        let meta = ToolMeta::new("Glob")
            .description("Find files matching a glob pattern inside the selected Workdir. Results are sorted and capped at 1000 entries. Paths are Workdir-relative.")
            .input_schema(serde_json::to_value(schema).expect("Glob schema serialization"));
        let tool: Arc<dyn Tool> = Arc::new(GlobTool {
            router: router.clone(),
        });
        (meta, tool)
    })
}
