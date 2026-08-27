use std::path::PathBuf;
use std::sync::Arc;

use agen::tool::{Tool, ToolDefinition, ToolError, ToolMeta, ToolOutput};
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use workdir::{CommandHandle, CommandOutputRequest, CommandRequest, WorkdirSessionHandle};

const DEFAULT_TIMEOUT_SECS: u64 = 120;
const MAX_TIMEOUT_SECS: u64 = 600;
const INLINE_BYTE_BUDGET: usize = 12 * 1024;

#[derive(Debug, Deserialize, JsonSchema)]
struct BashParams {
    command: String,
    #[serde(default)]
    timeout: Option<u64>,
}

pub(crate) struct BashTool {
    session: WorkdirSessionHandle,
}

struct CommandGuard {
    session: WorkdirSessionHandle,
    handle: Option<CommandHandle>,
}

impl Drop for CommandGuard {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            let workdir = self.session.clone();
            tokio::spawn(async move {
                let _ = workdir.cancel_command(handle).await;
            });
        }
    }
}

#[async_trait]
impl Tool for BashTool {
    async fn execute(
        &self,
        input_json: &str,
        ctx: agen::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let params: BashParams = serde_json::from_str(input_json)
            .map_err(|error| ToolError::InvalidArgument(format!("invalid Bash input: {error}")))?;
        let timeout_secs = params
            .timeout
            .unwrap_or(DEFAULT_TIMEOUT_SECS)
            .clamp(1, MAX_TIMEOUT_SECS);
        let cmd_summary = truncate_for_summary(&params.command);
        let handle = self
            .session
            .start_command(CommandRequest {
                command: params.command,
                timeout_secs,
                output_limit: INLINE_BYTE_BUDGET,
                tool_call_id: Some(ctx.call_id),
            })
            .await
            .map_err(crate::ToolsError::from)?;
        let mut guard = CommandGuard {
            session: self.session.clone(),
            handle: Some(handle.clone()),
        };
        let output = self
            .session
            .command_output(CommandOutputRequest {
                handle,
                cursor: 0,
                limit: INLINE_BYTE_BUDGET,
                wait: true,
            })
            .await
            .map_err(crate::ToolsError::from)?;
        guard.handle = None;

        let summary = if output.timed_out {
            format!("$ {cmd_summary} (timed out after {timeout_secs}s)")
        } else {
            match output.exit_code {
                Some(0) => format!("$ {cmd_summary}"),
                Some(code) => format!("$ {cmd_summary} (exit {code})"),
                None => format!("$ {cmd_summary} (terminated)"),
            }
        };
        let content = if output.content.is_empty() {
            None
        } else if output.truncated {
            Some(format!(
                "[showing bounded WorkdirSession command output; additional output was truncated]\n{}",
                output.content
            ))
        } else {
            Some(output.content)
        };
        Ok(ToolOutput {
            summary,
            content,
            attachments: Vec::new(),
        })
    }
}

fn truncate_for_summary(command: &str) -> String {
    const MAX: usize = 100;
    if command.chars().count() <= MAX {
        return command.to_owned();
    }
    let mut summary = command.chars().take(MAX - 1).collect::<String>();
    summary.push('…');
    summary
}

pub fn bash_tool(session: WorkdirSessionHandle, _output_dir: PathBuf) -> ToolDefinition {
    Arc::new(move || {
        let schema = schemars::schema_for!(BashParams);
        let meta = ToolMeta::new("Bash")
            .description("Execute a shell command in the bound Workdir. Process start, bounded output, timeout and cancellation are owned by the WorkdirSession provider. This is not a sandbox.")
            .input_schema(serde_json::to_value(schema).expect("Bash schema serialization"));
        let tool: Arc<dyn Tool> = Arc::new(BashTool {
            session: session.clone(),
        });
        (meta, tool)
    })
}
