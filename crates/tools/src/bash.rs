use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

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
    state: Arc<Mutex<BashExecutionState>>,
}

#[derive(Default)]
struct BashExecutionState {
    active: HashMap<String, CommandHandle>,
    cancellation_requested: HashSet<String>,
}

struct CommandGuard {
    session: WorkdirSessionHandle,
    state: Arc<Mutex<BashExecutionState>>,
    call_id: String,
    handle: Option<CommandHandle>,
}

impl Drop for CommandGuard {
    fn drop(&mut self) {
        let mut state = self.state.lock().unwrap();
        state.active.remove(&self.call_id);
        state.cancellation_requested.remove(&self.call_id);
        drop(state);
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
        let call_id = ctx.call_id;
        let mut guard = CommandGuard {
            session: self.session.clone(),
            state: self.state.clone(),
            call_id: call_id.clone(),
            handle: None,
        };
        let handle = self
            .session
            .start_command(CommandRequest {
                command: params.command,
                timeout_secs,
                output_limit: INLINE_BYTE_BUDGET,
                tool_call_id: Some(call_id.clone()),
            })
            .await
            .map_err(crate::ToolsError::from)?;
        let cancel_after_start = {
            let mut state = self.state.lock().unwrap();
            state.active.insert(call_id.clone(), handle.clone());
            state.cancellation_requested.contains(&call_id)
        };
        guard.handle = Some(handle.clone());
        if cancel_after_start {
            self.session
                .cancel_command(handle.clone())
                .await
                .map_err(crate::ToolsError::from)?;
        }
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
        let cancellation_requested = {
            let mut state = self.state.lock().unwrap();
            state.active.remove(&call_id);
            state.cancellation_requested.remove(&call_id)
        };
        guard.handle = None;

        let timed_out = output.timed_out;
        let summary = if cancellation_requested {
            format!("$ {cmd_summary} (cancelled)")
        } else if output.timed_out {
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
        let output = ToolOutput {
            summary,
            content,
            attachments: Vec::new(),
        };
        if cancellation_requested {
            Err(ToolError::Cancelled(output))
        } else if timed_out {
            Err(ToolError::Interrupted(output))
        } else {
            Ok(output)
        }
    }

    async fn cancel(&self, call_id: &str) -> Result<(), ToolError> {
        let handle = {
            let mut state = self.state.lock().unwrap();
            state.cancellation_requested.insert(call_id.to_string());
            state.active.get(call_id).cloned()
        };
        if let Some(handle) = handle {
            self.session
                .cancel_command(handle)
                .await
                .map_err(crate::ToolsError::from)?;
        }
        Ok(())
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
            state: Arc::new(Mutex::new(BashExecutionState::default())),
        });
        (meta, tool)
    })
}
