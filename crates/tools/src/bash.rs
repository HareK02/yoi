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
    output_dir: PathBuf,
    state: Arc<Mutex<BashExecutionState>>,
}

#[derive(Clone)]
struct ActiveCommand {
    call_id: String,
    execution_nonce: u64,
    handle: CommandHandle,
}

#[derive(Default)]
struct BashExecutionState {
    active: HashMap<String, ActiveCommand>,
    cancellation_requested: HashSet<String>,
    legacy_cancellation_requested: HashSet<String>,
    next_execution_nonce: u64,
}

struct CommandGuard {
    session: WorkdirSessionHandle,
    state: Arc<Mutex<BashExecutionState>>,
    execution_id: String,
    execution_nonce: u64,
    handle: Option<CommandHandle>,
}

impl Drop for CommandGuard {
    fn drop(&mut self) {
        let Some(handle) = self.handle.take() else {
            return;
        };
        let workdir = self.session.clone();
        let state = Arc::clone(&self.state);
        let execution_id = self.execution_id.clone();
        let execution_nonce = self.execution_nonce;
        // A dropped provider future is not terminal confirmation. Keep the live
        // execution registered until cleanup has both requested cancellation and
        // observed terminal command output, so cancellation/session teardown
        // cannot race with an apparently empty registry.
        tokio::spawn(async move {
            let _ = workdir.cancel_command(handle.clone()).await;
            let _ = workdir
                .command_output(CommandOutputRequest {
                    handle,
                    cursor: 0,
                    limit: INLINE_BYTE_BUDGET,
                    wait: true,
                })
                .await;
            let mut state = state.lock().unwrap();
            if state
                .active
                .get(&execution_id)
                .is_some_and(|active| active.execution_nonce == execution_nonce)
            {
                state.active.remove(&execution_id);
                state.cancellation_requested.remove(&execution_id);
            }
        });
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
        let execution_id = ctx.execution_id();
        let call_id = ctx.call_id;
        let execution_nonce = {
            let mut state = self.state.lock().unwrap();
            state.next_execution_nonce = state.next_execution_nonce.wrapping_add(1);
            state.next_execution_nonce
        };
        let mut guard = CommandGuard {
            session: self.session.clone(),
            state: self.state.clone(),
            execution_id: execution_id.clone(),
            execution_nonce,
            handle: None,
        };
        let handle = self
            .session
            .start_command(CommandRequest {
                command: params.command,
                timeout_secs,
                output_limit: INLINE_BYTE_BUDGET,
                spill_dir: Some(self.output_dir.clone()),
                tool_call_id: Some(call_id.clone()),
            })
            .await
            .map_err(crate::ToolsError::from)?;
        let cancel_after_start = {
            let mut state = self.state.lock().unwrap();
            state.active.insert(
                execution_id.clone(),
                ActiveCommand {
                    call_id: call_id.clone(),
                    execution_nonce,
                    handle: handle.clone(),
                },
            );
            state.cancellation_requested.contains(&execution_id)
                || state.legacy_cancellation_requested.contains(&call_id)
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
            let owns_registration = state
                .active
                .get(&execution_id)
                .is_some_and(|active| active.execution_nonce == execution_nonce);
            let exact = if owns_registration {
                state.active.remove(&execution_id);
                state.cancellation_requested.remove(&execution_id)
            } else {
                false
            };
            let legacy = state.legacy_cancellation_requested.remove(&call_id);
            exact || legacy
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
            let notice = match output.output_path {
                Some(path) => format!(
                    "[showing bounded WorkdirSession command output; full output saved to {}]",
                    path.display()
                ),
                None => "[showing bounded WorkdirSession command output; additional output was truncated]"
                    .to_owned(),
            };
            Some(format!("{notice}\n{}", output.content))
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
        let handles = {
            let mut state = self.state.lock().unwrap();
            state
                .legacy_cancellation_requested
                .insert(call_id.to_string());
            state
                .active
                .values()
                .filter(|active| active.call_id == call_id)
                .map(|active| active.handle.clone())
                .collect::<Vec<_>>()
        };
        for handle in handles {
            self.session
                .cancel_command(handle)
                .await
                .map_err(crate::ToolsError::from)?;
        }
        Ok(())
    }

    async fn cancel_execution(
        &self,
        ctx: &agen::tool::ToolExecutionContext,
    ) -> Result<(), ToolError> {
        let execution_id = ctx.execution_id();
        let handle = {
            let mut state = self.state.lock().unwrap();
            state.cancellation_requested.insert(execution_id.clone());
            state
                .active
                .get(&execution_id)
                .map(|active| active.handle.clone())
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

pub fn bash_tool(session: WorkdirSessionHandle, output_dir: PathBuf) -> ToolDefinition {
    Arc::new(move || {
        let schema = schemars::schema_for!(BashParams);
        let meta = ToolMeta::new("Bash")
            .description("Execute a shell command in the bound Workdir. Process start, bounded inline output, full-output spill, timeout and cancellation are owned by the WorkdirSession provider. This is not a sandbox.")
            .input_schema(serde_json::to_value(schema).expect("Bash schema serialization"));
        let tool: Arc<dyn Tool> = Arc::new(BashTool {
            session: session.clone(),
            output_dir: output_dir.clone(),
            state: Arc::new(Mutex::new(BashExecutionState::default())),
        });
        (meta, tool)
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use manifest::{Permission, Scope, ScopeConfig, ScopeRule};
    use tempfile::TempDir;
    use workdir::{LocalWorkdirSession, WorkdirSessionHandle};

    use super::bash_tool;
    use crate::{grep::grep_tool, read::read_tool, tracker::Tracker};

    fn session_with_output_scope(root: &TempDir, output: &TempDir) -> WorkdirSessionHandle {
        let scope = Scope::from_config(&ScopeConfig {
            allow: vec![
                ScopeRule {
                    target: root.path().to_path_buf(),
                    permission: Permission::Write,
                    recursive: true,
                },
                ScopeRule {
                    target: output.path().to_path_buf(),
                    permission: Permission::Read,
                    recursive: true,
                },
            ],
            deny: Vec::new(),
        })
        .unwrap();
        Arc::new(LocalWorkdirSession::new(scope, root.path().to_path_buf()))
    }

    #[tokio::test]
    async fn long_output_is_spilled_and_available_to_read_and_grep() {
        let root = TempDir::new().unwrap();
        let output = TempDir::new().unwrap();
        let session = session_with_output_scope(&root, &output);
        let (_, bash) = bash_tool(session.clone(), output.path().to_path_buf())();
        let command = "i=0; while [ $i -lt 2000 ]; do printf 'line-%04d\\n' \"$i\"; i=$((i+1)); done; printf 'FINAL-NEEDLE\\n'";
        let result = bash
            .execute(
                &serde_json::json!({ "command": command }).to_string(),
                Default::default(),
            )
            .await
            .unwrap();
        let rendered = result.content.expect("bounded Bash output");
        let artifact = std::fs::read_dir(output.path())
            .unwrap()
            .next()
            .expect("artifact entry")
            .unwrap()
            .path();

        assert!(rendered.contains("full output saved to"));
        assert!(rendered.contains(&artifact.display().to_string()));
        let retained = std::fs::read_to_string(&artifact).unwrap();
        assert!(retained.starts_with("line-0000\n"));
        assert!(retained.ends_with("FINAL-NEEDLE\n"));
        assert_eq!(retained.lines().count(), 2001);

        let (_, read) = read_tool(session.clone(), Tracker::new())();
        let read_result = read
            .execute(
                &serde_json::json!({
                    "file_path": artifact,
                    "offset": 2000,
                    "limit": 1,
                })
                .to_string(),
                Default::default(),
            )
            .await
            .unwrap();
        assert!(
            read_result
                .content
                .expect("Read content")
                .contains("FINAL-NEEDLE")
        );

        let (_, grep) = grep_tool(session)();
        let grep_result = grep
            .execute(
                &serde_json::json!({
                    "pattern": "FINAL-NEEDLE",
                    "path": artifact,
                    "output_mode": "content",
                })
                .to_string(),
                Default::default(),
            )
            .await
            .unwrap();
        let grep_content = grep_result.content.expect("Grep content");
        assert!(
            grep_content.contains("FINAL-NEEDLE"),
            "unexpected Grep content: {grep_content:?}"
        );
    }

    #[tokio::test]
    async fn short_output_does_not_leave_a_spill_artifact() {
        let root = TempDir::new().unwrap();
        let output = TempDir::new().unwrap();
        let session = session_with_output_scope(&root, &output);
        let (_, bash) = bash_tool(session, output.path().to_path_buf())();

        let result = bash
            .execute(
                &serde_json::json!({ "command": "printf short" }).to_string(),
                Default::default(),
            )
            .await
            .unwrap();

        assert_eq!(result.content.as_deref(), Some("short"));
        assert_eq!(std::fs::read_dir(output.path()).unwrap().count(), 0);
    }
}
