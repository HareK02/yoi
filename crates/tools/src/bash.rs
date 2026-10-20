//! Bash Tool using WorkdirSession command capability.
//!
//! Workdir owns launch and byte transport; Bash owns bounded preview and retained evidence.
//! Bash is not a sandbox; the Runtime process and OS user define the execution boundary.
//! Per-execution handles and nonces prevent cancellation of another command. Dropping a future
//! does not confirm termination. Registrations remain during bounded cancellation/drain cleanup;
//! an unconfirmed provider outcome leaves partial evidence, never a fabricated terminal fence.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use crate::bash_output::{BashOutputMetadata, BashOutputView, BashOutputWriter};
use agen::tool::{Tool, ToolDefinition, ToolError, ToolMeta, ToolOutput};
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use workdir::{
    CommandHandle, CommandOutputRequest, CommandRequest, WorkdirPath, WorkdirSessionHandle,
    WorkdirSessionRouter,
};

const DEFAULT_TIMEOUT_SECS: u64 = 120;
const MAX_TIMEOUT_SECS: u64 = 600;
const INLINE_BYTE_BUDGET: usize = 12 * 1024;

#[derive(Debug, Deserialize, JsonSchema)]
struct BashParams {
    /// Worker-local alias of the Workdir attachment to use.
    #[serde(default)]
    target_workdir: Option<String>,
    /// Workdir-relative directory in which to execute the command.
    cwd: WorkdirPath,
    command: String,
    #[serde(default)]
    timeout: Option<u64>,
}

pub struct BashTool {
    router: Arc<WorkdirSessionRouter>,
    output: BashOutputView,
    state: Arc<Mutex<BashExecutionState>>,
}

#[derive(Clone)]
struct ActiveCommand {
    call_id: String,
    execution_nonce: u64,
    session: WorkdirSessionHandle,
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
    cursor: u64,
    retained: Option<BashOutputWriter>,
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
        let mut retained = self.retained.take();
        let cursor = self.cursor;
        // A dropped provider future is not terminal confirmation. Keep the live
        // execution registered until cleanup has both requested cancellation and
        // observed terminal command output, so cancellation/session teardown
        // cannot race with an apparently empty registry.
        tokio::spawn(async move {
            let result = tokio::time::timeout(std::time::Duration::from_secs(10), async {
                let _ = workdir.cancel_command(handle.clone()).await;
                let mut cursor = cursor;
                loop {
                    let page = command_page(
                        &workdir,
                        CommandOutputRequest {
                            handle: handle.clone(),
                            cursor,
                            limit: workdir::MAX_COMMAND_READ_BYTES,
                            wait: true,
                        },
                    )
                    .await?;
                    if let Some(writer) = &mut retained {
                        let mut next = cursor;
                        for chunk in &page.chunks {
                            if chunk.sequence != next {
                                return Err(workdir::WorkdirError::Unavailable(
                                    "command cleanup sequence gap".into(),
                                ));
                            }
                            writer
                                .append_chunk(chunk)
                                .map_err(workdir::WorkdirError::Unavailable)?;
                            next = next.checked_add(chunk.bytes.len() as u64).ok_or_else(|| {
                                workdir::WorkdirError::Unavailable(
                                    "command cleanup cursor overflow".into(),
                                )
                            })?;
                        }
                        if page.next_cursor != next {
                            return Err(workdir::WorkdirError::Unavailable(
                                "command cleanup cursor gap".into(),
                            ));
                        }
                    }
                    cursor = page.next_cursor;
                    if let Some(terminal) = page.terminal {
                        if let Some(writer) = &mut retained {
                            writer
                                .finish_transport(&terminal)
                                .map_err(workdir::WorkdirError::Unavailable)?;
                        }
                        return Ok::<(), workdir::WorkdirError>(());
                    }
                }
            })
            .await;
            // Bounded cleanup can end with an unknown provider outcome. The writer then
            // terminalizes partial evidence on drop; retaining a dead collector in `active`
            // would leak registrations and falsely suggest it could still collect output.
            let _ = result;
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

// Output pages are replayable only with the exact same command/cursor/limit.
// Retry read transport failures, never command start, and cap both attempts and
// backoff. The caller's deadline bounds a disconnected or unresponsive provider.
async fn command_page(
    session: &workdir::WorkdirSessionHandle,
    request: CommandOutputRequest,
) -> Result<workdir::CommandOutput, workdir::WorkdirError> {
    for attempt in 0..3 {
        match session.command_output(request.clone()).await {
            Err(
                workdir::WorkdirError::Transport(_)
                | workdir::WorkdirError::Unavailable(_)
                | workdir::WorkdirError::OutcomeUnknown(_),
            ) if attempt < 2 => {
                tokio::time::sleep(std::time::Duration::from_millis(100 << attempt)).await;
            }
            result => return result,
        }
    }
    unreachable!("bounded retry loop returns its last result")
}

impl BashTool {
    pub fn input_schema() -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(BashParams)).expect("Bash schema serialization")
    }

    pub fn new(router: Arc<WorkdirSessionRouter>, output: BashOutputView) -> Self {
        Self {
            router,
            output,
            state: Arc::new(Mutex::new(BashExecutionState::default())),
        }
    }

    pub async fn execute_result(
        &self,
        input_json: &str,
        ctx: agen::tool::ToolExecutionContext,
    ) -> Result<BashResult, ToolError> {
        let params: BashParams = serde_json::from_str(input_json)
            .map_err(|error| ToolError::InvalidArgument(format!("invalid Bash input: {error}")))?;
        let retained = self.output.begin().map_err(ToolError::ExecutionFailed)?;
        let selected = crate::routing::resolve_session(
            &self.router,
            params.target_workdir.as_deref(),
            workdir::WorkdirSessionCapability::Command,
        )?;
        let session = selected.session;
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
            session: session.clone(),
            state: self.state.clone(),
            execution_id: execution_id.clone(),
            execution_nonce,
            handle: None,
            cursor: 0,
            retained: Some(retained),
        };
        let handle = session
            .start_command(CommandRequest {
                command: params.command,
                timeout_secs,
                transport_byte_budget: self.output.byte_budget(),
                cwd: params.cwd,
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
                    session: session.clone(),
                    handle: handle.clone(),
                },
            );
            state.cancellation_requested.contains(&execution_id)
                || state.legacy_cancellation_requested.contains(&call_id)
        };
        guard.handle = Some(handle.clone());
        if cancel_after_start {
            session
                .cancel_command(handle.clone())
                .await
                .map_err(crate::ToolsError::from)?;
        }
        let mut cursor = 0;
        let mut stdout_preview = Vec::new();
        let mut stderr_preview = Vec::new();
        let mut seen = [0_u64; 2];
        let mut empty_polls = 0;
        let deadline =
            tokio::time::Instant::now() + std::time::Duration::from_secs(timeout_secs + 30);
        let terminal = loop {
            let page = tokio::time::timeout_at(
                deadline,
                command_page(
                    &session,
                    CommandOutputRequest {
                        handle: handle.clone(),
                        cursor,
                        limit: workdir::MAX_COMMAND_READ_BYTES,
                        wait: true,
                    },
                ),
            )
            .await
            .map_err(|_| {
                ToolError::ExecutionFailed(
                    "command byte transport deadline exceeded; terminal outcome unknown".into(),
                )
            })?
            .map_err(crate::ToolsError::from)?;
            let mut next = cursor;
            for chunk in &page.chunks {
                if chunk.sequence != next || chunk.bytes.is_empty() {
                    return Err(ToolError::ExecutionFailed(
                        "command byte transport sequence gap".into(),
                    ));
                }
                next = next
                    .checked_add(chunk.bytes.len() as u64)
                    .filter(|n| *n <= self.output.byte_budget())
                    .ok_or_else(|| {
                        ToolError::ExecutionFailed(
                            "command byte transport exceeds delivery budget".into(),
                        )
                    })?;
                let index = if chunk.stream == workdir::CommandStream::Stdout {
                    0
                } else {
                    1
                };
                if chunk.offset != seen[index] {
                    return Err(ToolError::ExecutionFailed(
                        "command byte transport offset gap".into(),
                    ));
                }
                seen[index] += chunk.bytes.len() as u64;
                guard
                    .retained
                    .as_mut()
                    .unwrap()
                    .append_chunk(chunk)
                    .map_err(ToolError::ExecutionFailed)?;
                let available =
                    INLINE_BYTE_BUDGET.saturating_sub(stdout_preview.len() + stderr_preview.len());
                let preview = if index == 0 {
                    &mut stdout_preview
                } else {
                    &mut stderr_preview
                };
                preview.extend_from_slice(&chunk.bytes[..available.min(chunk.bytes.len())]);
            }
            if page.next_cursor != next {
                return Err(ToolError::ExecutionFailed(
                    "command byte transport cursor does not fence the page".into(),
                ));
            }
            if page.chunks.is_empty() && page.terminal.is_none() {
                empty_polls += 1;
                if empty_polls > 32 {
                    return Err(ToolError::ExecutionFailed(
                        "command byte transport made no progress after bounded polls".into(),
                    ));
                }
            } else {
                empty_polls = 0;
            }
            cursor = page.next_cursor;
            guard.cursor = cursor;
            if let Some(terminal) = page.terminal {
                break terminal;
            }
        };
        let metadata = guard
            .retained
            .as_mut()
            .unwrap()
            .finish_transport(&terminal)
            .map_err(ToolError::ExecutionFailed)?;
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

        let timed_out = terminal.status == workdir::CommandStatus::TimedOut;
        let summary =
            if cancellation_requested || terminal.status == workdir::CommandStatus::Cancelled {
                format!("$ {cmd_summary} (cancelled)")
            } else if timed_out {
                format!("$ {cmd_summary} (timed out after {timeout_secs}s)")
            } else {
                match terminal.exit_code {
                    Some(0) => format!("$ {cmd_summary}"),
                    Some(code) => format!("$ {cmd_summary} (exit {code})"),
                    None => format!("$ {cmd_summary} (terminated)"),
                }
            };
        Ok(BashResult {
            summary,
            stdout_preview: String::from_utf8_lossy(&stdout_preview).into_owned(),
            stderr_preview: String::from_utf8_lossy(&stderr_preview).into_owned(),
            preview_omitted: terminal.stdout_bytes.saturating_add(terminal.stderr_bytes)
                > (stdout_preview.len() + stderr_preview.len()) as u64,
            metadata,
            cancelled: cancellation_requested
                || terminal.status == workdir::CommandStatus::Cancelled,
        })
    }
}

#[derive(Debug, Serialize)]
pub struct BashResult {
    pub summary: String,
    pub stdout_preview: String,
    pub stderr_preview: String,
    pub preview_omitted: bool,
    pub metadata: BashOutputMetadata,
    pub cancelled: bool,
}

impl BashResult {
    pub fn stdout_entry(&self) -> String {
        format!("/bash-output/{}/stdout", self.metadata.id)
    }
    pub fn stderr_entry(&self) -> String {
        format!("/bash-output/{}/stderr", self.metadata.id)
    }
    pub fn order_entry(&self) -> String {
        format!("/bash-output/{}/order", self.metadata.id)
    }
    pub fn tool_output(&self) -> ToolOutput {
        let content = serde_json::json!({
            "stdout_preview": self.stdout_preview, "stderr_preview": self.stderr_preview,
            "preview_omitted": self.preview_omitted, "output": self.metadata,
            "stdout_entry": self.stdout_entry(), "stderr_entry": self.stderr_entry(), "order_entry": self.order_entry(),
        });
        ToolOutput {
            summary: self.summary.clone(),
            content: Some(content.to_string()),
            attachments: vec![],
        }
    }
}

#[async_trait]
impl Tool for BashTool {
    async fn execute(
        &self,
        input: &str,
        ctx: agen::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let result = self.execute_result(input, ctx).await?;
        let output = result.tool_output();
        if result.cancelled {
            Err(ToolError::Cancelled(output))
        } else if result.metadata.terminal.status == Some(workdir::CommandStatus::TimedOut) {
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
                .cloned()
                .collect::<Vec<_>>()
        };
        for active in handles {
            active
                .session
                .cancel_command(active.handle)
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
            state.active.get(&execution_id).cloned()
        };
        if let Some(active) = handle {
            active
                .session
                .cancel_command(active.handle)
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

pub fn bash_tool(session: WorkdirSessionHandle, output: BashOutputView) -> ToolDefinition {
    routed_bash_tool(crate::routing::singleton_router(session), output)
}

pub(crate) fn routed_bash_tool(
    router: Arc<WorkdirSessionRouter>,
    output: BashOutputView,
) -> ToolDefinition {
    Arc::new(move || {
        let meta = ToolMeta::new("Bash")
            .description("Execute a shell command in one selected Workdir. Workdir owns launch, lossless bounded byte transport, timeout and cancel; Bash owns preview, quota-limited ephemeral output and read-only virtual references. This is not a sandbox.")
            .input_schema(BashTool::input_schema());
        let tool: Arc<dyn Tool> = Arc::new(BashTool::new(router.clone(), output.clone()));
        (meta, tool)
    })
}
