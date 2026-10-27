//! Local stdio MCP server communication and process lifecycle.
//!
//! The Host resolves launch specs, clears the environment, and passes only explicit
//! values to child processes. Values resolved from SecretStore or inherited from
//! the environment are redacted in diagnostics. This is not a sandbox: child
//! execution capabilities depend on Host and OS user boundaries.
//!
//! Handles stdout as JSON-RPC and stderr as bounded diagnostics, separately.
//! Limits line lengths, response bytes, discovery pages, and request deadlines;
//! malformed communication is not registered as valid descriptors. Requests are
//! accepted only after the initialize/initialized handshake. Shutdown closes stdin,
//! waits for exit within a deadline, and kills/waits to reap the process if needed.
//! Startup failures also trigger cleanup; killing on Drop is a last resort.
//! A request shares one deadline across stdin ownership, write/flush, and matching response;
//! synchronous result decode is outside that wait. Interrupted write/flush closes stdin.
//! After a fully flushed frame, a missing response leaves execution unknown, not retry-safe.
//!
//! This module preserves Tool/resource/prompt wire values. `worker::feature::mcp`
//! owns Yoi naming, schema acceptance, model output limits, and Feature registration.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::env;
use std::fmt;
use std::path::PathBuf;
use std::process::ExitStatus;
use std::sync::Arc;
use std::time::Duration;

use manifest::{McpConfig, McpEnvValue, McpStdioCwdPolicy, McpStdioServerConfig};
use secrets::SecretStore;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStderr, ChildStdout, Command};
use tokio::sync::{Mutex, mpsc};
use tokio::task::JoinHandle;
use tokio::time::{Instant, timeout, timeout_at};

const MCP_PROTOCOL_VERSION: &str = "2025-11-25";
const CLIENT_NAME: &str = "yoi";
const CLIENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const JSONRPC_VERSION: &str = "2.0";
const ERR_METHOD_NOT_FOUND: i64 = -32601;

/// Resource limits for a local stdio MCP server lifecycle.
#[derive(Debug, Clone)]
pub struct McpStdioLimits {
    pub max_stdout_line_bytes: usize,
    pub max_stderr_line_bytes: usize,
    pub max_diagnostic_lines: usize,
    pub max_protocol_bytes: usize,
    pub startup_timeout: Duration,
    /// One budget for stdin ownership, frame write/flush, and response wait.
    pub request_timeout: Duration,
    /// Shared budget for stdin ownership/close and graceful process exit.
    pub shutdown_timeout: Duration,
    /// Budget for each fallback wait: terminate, then forced kill/reap.
    pub kill_timeout: Duration,
}

impl Default for McpStdioLimits {
    fn default() -> Self {
        Self {
            max_stdout_line_bytes: 1024 * 1024,
            max_stderr_line_bytes: 16 * 1024,
            max_diagnostic_lines: 32,
            max_protocol_bytes: 1024 * 1024,
            startup_timeout: Duration::from_secs(10),
            request_timeout: Duration::from_secs(10),
            shutdown_timeout: Duration::from_secs(2),
            kill_timeout: Duration::from_secs(2),
        }
    }
}

/// Host bounds for MCP `tools/list` pagination during discovery.
#[derive(Debug, Clone, Copy)]
pub struct McpToolListLimits {
    pub max_pages: usize,
    pub max_tools: usize,
}

impl Default for McpToolListLimits {
    fn default() -> Self {
        Self {
            max_pages: 8,
            max_tools: 128,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpToolDefinition {
    pub name: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    pub input_schema: Value,
    #[serde(default)]
    pub output_schema: Option<Value>,
    #[serde(default)]
    pub annotations: Option<Value>,
    #[serde(default, rename = "_meta")]
    pub meta: Option<Value>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListToolsResult {
    #[serde(default)]
    pub tools: Vec<McpToolDefinition>,
    #[serde(default)]
    pub next_cursor: Option<String>,
    #[serde(default, rename = "_meta")]
    pub meta: Option<Value>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Copy)]
pub struct McpResourceListLimits {
    pub max_pages: usize,
    pub max_resources: usize,
    pub max_resource_templates: usize,
}

impl Default for McpResourceListLimits {
    fn default() -> Self {
        Self {
            max_pages: 8,
            max_resources: 128,
            max_resource_templates: 128,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct McpPromptListLimits {
    pub max_pages: usize,
    pub max_prompts: usize,
}

impl Default for McpPromptListLimits {
    fn default() -> Self {
        Self {
            max_pages: 8,
            max_prompts: 128,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpResourceDefinition {
    pub uri: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub mime_type: Option<String>,
    #[serde(default)]
    pub annotations: Option<Value>,
    #[serde(default, rename = "_meta")]
    pub meta: Option<Value>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpResourceTemplateDefinition {
    pub uri_template: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub mime_type: Option<String>,
    #[serde(default)]
    pub annotations: Option<Value>,
    #[serde(default, rename = "_meta")]
    pub meta: Option<Value>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListResourcesResult {
    #[serde(default)]
    pub resources: Vec<McpResourceDefinition>,
    #[serde(default)]
    pub resource_templates: Vec<McpResourceTemplateDefinition>,
    #[serde(default)]
    pub next_cursor: Option<String>,
    #[serde(default, rename = "_meta")]
    pub meta: Option<Value>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadResourceRequest {
    pub uri: String,
}

impl ReadResourceRequest {
    pub fn new(uri: impl Into<String>) -> Self {
        Self { uri: uri.into() }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpResourceContent {
    pub uri: String,
    #[serde(default)]
    pub mime_type: Option<String>,
    #[serde(default, rename = "_meta")]
    pub meta: Option<Value>,
    #[serde(flatten)]
    pub fields: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadResourceResult {
    #[serde(default)]
    pub contents: Vec<McpResourceContent>,
    #[serde(default, rename = "_meta")]
    pub meta: Option<Value>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpPromptArgumentDefinition {
    pub name: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub required: Option<bool>,
    #[serde(default, rename = "_meta")]
    pub meta: Option<Value>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpPromptDefinition {
    pub name: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub arguments: Vec<McpPromptArgumentDefinition>,
    #[serde(default, rename = "_meta")]
    pub meta: Option<Value>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListPromptsResult {
    #[serde(default)]
    pub prompts: Vec<McpPromptDefinition>,
    #[serde(default)]
    pub next_cursor: Option<String>,
    #[serde(default, rename = "_meta")]
    pub meta: Option<Value>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetPromptRequest {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments: Option<Value>,
}

impl GetPromptRequest {
    pub fn new(name: impl Into<String>, arguments: Option<Value>) -> Self {
        Self {
            name: name.into(),
            arguments,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpPromptMessage {
    pub role: String,
    pub content: McpContentBlock,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetPromptResult {
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub messages: Vec<McpPromptMessage>,
    #[serde(default, rename = "_meta")]
    pub meta: Option<Value>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CallToolRequest {
    pub name: String,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub arguments: Value,
}

impl CallToolRequest {
    pub fn new(name: impl Into<String>, arguments: Value) -> Self {
        Self {
            name: name.into(),
            arguments,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CallToolResult {
    #[serde(default)]
    pub content: Vec<McpContentBlock>,
    #[serde(default)]
    pub structured_content: Option<Value>,
    #[serde(default)]
    pub is_error: bool,
    #[serde(default, rename = "_meta")]
    pub meta: Option<Value>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// One untrusted MCP `tools/call` content block.
///
/// The `type` discriminator is kept explicit and all server-owned fields stay
/// data in `fields`; this crate does not turn rich MCP content into hidden host
/// context.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct McpContentBlock {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(flatten)]
    pub fields: BTreeMap<String, Value>,
}

/// MCP list surface whose `notifications/*/list_changed` signal was observed.
///
/// The notification is only a freshness signal. The stdio client records this
/// bounded enum state and deliberately ignores notification params so a server
/// cannot inject resource/prompt content or alter model-visible tool schemas
/// through an out-of-band notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum McpListChangedKind {
    Tools,
    Resources,
    Prompts,
}

impl McpListChangedKind {
    fn from_notification_method(method: &str) -> Option<Self> {
        match method {
            "notifications/tools/list_changed" => Some(Self::Tools),
            "notifications/resources/list_changed" => Some(Self::Resources),
            "notifications/prompts/list_changed" => Some(Self::Prompts),
            _ => None,
        }
    }

    pub fn notification_method(self) -> &'static str {
        match self {
            Self::Tools => "notifications/tools/list_changed",
            Self::Resources => "notifications/resources/list_changed",
            Self::Prompts => "notifications/prompts/list_changed",
        }
    }

    pub fn list_method(self) -> &'static str {
        match self {
            Self::Tools => "tools/list",
            Self::Resources => "resources/list",
            Self::Prompts => "prompts/list",
        }
    }
}

/// Bounded snapshot of list-change signals observed from one stdio server.
#[derive(Debug, Clone)]
pub struct McpListChangedSnapshot {
    pub server_name: String,
    kinds: BTreeSet<McpListChangedKind>,
}

impl McpListChangedSnapshot {
    pub fn is_empty(&self) -> bool {
        self.kinds.is_empty()
    }

    pub fn contains(&self, kind: McpListChangedKind) -> bool {
        self.kinds.contains(&kind)
    }

    pub fn kinds(&self) -> impl Iterator<Item = McpListChangedKind> + '_ {
        self.kinds.iter().copied()
    }
}

/// A resolved, explicit local stdio MCP server process specification.
#[derive(Clone)]
pub struct McpStdioServerSpec {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: BTreeMap<String, String>,
    redactions: Vec<String>,
}

impl McpStdioServerSpec {
    pub fn new(name: impl Into<String>, command: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            command: command.into(),
            args: Vec::new(),
            cwd: None,
            env: BTreeMap::new(),
            redactions: Vec::new(),
        }
    }

    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    pub fn args(mut self, args: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    pub fn cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    pub fn env(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        let value = value.into();
        self.redact_value(&value);
        self.env.insert(name.into(), value);
        self
    }

    pub fn redact_value(&mut self, value: &str) {
        if !value.is_empty() && !self.redactions.iter().any(|existing| existing == value) {
            self.redactions.push(value.to_owned());
        }
    }

    fn redactor(&self) -> Redactor {
        Redactor::new(self.redactions.clone())
    }
}

impl fmt::Debug for McpStdioServerSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let env: BTreeMap<&str, &str> = self
            .env
            .keys()
            .map(|name| (name.as_str(), "[redacted]"))
            .collect();
        f.debug_struct("McpStdioServerSpec")
            .field("name", &self.name)
            .field("command", &self.command)
            .field("args", &self.args)
            .field("cwd", &self.cwd)
            .field("env", &env)
            .field("redaction_count", &self.redactions.len())
            .finish()
    }
}

/// Resolve one explicitly named stdio server from typed MCP config.
pub fn resolve_named_stdio_server(
    config: &McpConfig,
    name: &str,
    workspace_root: impl Into<PathBuf>,
    secret_store: Option<&SecretStore>,
) -> Result<McpStdioServerSpec, McpClientError> {
    let server = config
        .stdio_servers
        .iter()
        .find(|server| server.name == name)
        .ok_or_else(|| {
            McpClientError::new(
                name,
                McpPhase::Spawn,
                McpErrorKind::Config(format!("stdio server `{name}` is not configured")),
            )
        })?;
    resolve_stdio_server(server, workspace_root, secret_store)
}

/// Resolve one typed stdio server into process IO settings without starting it.
pub fn resolve_stdio_server(
    server: &McpStdioServerConfig,
    workspace_root: impl Into<PathBuf>,
    secret_store: Option<&SecretStore>,
) -> Result<McpStdioServerSpec, McpClientError> {
    let mut spec = McpStdioServerSpec::new(server.name.clone(), server.command.clone())
        .args(server.args.clone());
    let _workspace_root = workspace_root.into();
    spec.cwd = match &server.cwd {
        Some(McpStdioCwdPolicy::Path { path }) => Some(path.clone()),
        Some(McpStdioCwdPolicy::Inherit) | None => None,
    };

    for name in &server.env.inherit {
        if let Ok(value) = env::var(name) {
            spec.redact_value(&value);
            spec.env.insert(name.clone(), value);
        }
    }

    for (name, value) in &server.env.set {
        let resolved = match value {
            McpEnvValue::Literal { value } => value.clone(),
            McpEnvValue::EnvRef { name } => env::var(name).map_err(|err| {
                McpClientError::new(
                    &server.name,
                    McpPhase::Spawn,
                    McpErrorKind::Config(format!(
                        "environment variable `{name}` is unavailable: {err}"
                    )),
                )
            })?,
            McpEnvValue::SecretRef { ref_ } => {
                let store = secret_store.ok_or_else(|| {
                    McpClientError::new(
                        &server.name,
                        McpPhase::Spawn,
                        McpErrorKind::Config(format!(
                            "secret `{ref_}` requires a configured secret store"
                        )),
                    )
                })?;
                store
                    .get(ref_)
                    .map_err(|err| {
                        McpClientError::new(
                            &server.name,
                            McpPhase::Spawn,
                            McpErrorKind::Config(format!(
                                "failed to resolve secret `{ref_}`: {err}"
                            )),
                        )
                    })?
                    .into_string()
            }
        };
        spec.redact_value(&resolved);
        spec.env.insert(name.clone(), resolved);
    }

    Ok(spec)
}

// The mutex orders entire frames, including replies emitted by the stdout reader.
type SharedStdin = Arc<Mutex<Option<Box<dyn AsyncWrite + Send + Unpin>>>>;

/// Last boundary reached by a failed request. A flushed frame does not prove
/// that the server executed the method; callers must not blindly retry it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpRequestStage {
    WaitingForStdin,
    WritingFrame,
    FlushingFrame,
    AwaitingResponse,
}

impl fmt::Display for McpRequestStage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::WaitingForStdin => "stdin ownership (method not sent)",
            Self::WritingFrame => "frame write (delivery unknown; stdin closed)",
            Self::FlushingFrame => "frame flush (delivery unknown; stdin closed)",
            Self::AwaitingResponse => "response (frame flushed; execution unknown; do not retry)",
        })
    }
}

/// An owned stdio MCP process, initialized by `connect` or explicitly after
/// `spawn_uninitialized` when startup cleanup must retain it before any await.
pub struct McpStdioClient {
    server_name: String,
    limits: McpStdioLimits,
    redactor: Redactor,
    diagnostics: Arc<Mutex<BoundedDiagnostics>>,
    list_changes: Arc<Mutex<BoundedListChanged>>,
    stdin: SharedStdin,
    child: Option<Child>,
    responses: mpsc::Receiver<ReaderEvent>,
    reader_task: JoinHandle<()>,
    stderr_task: JoinHandle<()>,
    next_id: u64,
    initialized: Option<InitializeResult>,
    shutdown_started: bool,
}

impl McpStdioClient {
    /// Spawn, initialize, negotiate capabilities, and send notifications/initialized.
    /// Initialization errors attempt shutdown, but this convenience API cannot
    /// return an unreaped client on failure/cancellation. Its error is not proof
    /// of cleanup; use `spawn_uninitialized` with retained ownership when needed.
    pub async fn connect(
        spec: McpStdioServerSpec,
        limits: McpStdioLimits,
    ) -> Result<Self, McpClientError> {
        let started = Instant::now();
        let mut client = Self::spawn(spec, limits).await?;
        match timeout(client.limits.startup_timeout, client.initialize()).await {
            Ok(Ok(())) => Ok(client),
            Ok(Err(err)) => {
                let _ = client.shutdown().await;
                Err(err.with_diagnostics(client.snapshot_diagnostics().await))
            }
            Err(_) => {
                let err = McpClientError::new(
                    &client.server_name,
                    McpPhase::Initialize,
                    McpErrorKind::Timeout {
                        operation: "startup".to_string(),
                        elapsed: started.elapsed(),
                    },
                )
                .with_diagnostics(client.snapshot_diagnostics().await);
                let _ = client.shutdown().await;
                Err(err)
            }
        }
    }

    async fn spawn(
        spec: McpStdioServerSpec,
        limits: McpStdioLimits,
    ) -> Result<Self, McpClientError> {
        Self::spawn_uninitialized(spec, limits)
    }

    /// Spawn without awaiting the handshake, so a cleanup owner can retain the
    /// client before initialization can fail or be cancelled. Call
    /// `initialize_with_timeout` before discovery; on error/cancellation, shut
    /// down this same client rather than spawning a replacement.
    ///
    /// Once the process spawns, this returns its owner even if an expected pipe
    /// is missing. Initialization then fails, but cleanup can still reap the
    /// actual child rather than relying on kill-on-drop for a post-spawn error.
    pub fn spawn_uninitialized(
        spec: McpStdioServerSpec,
        limits: McpStdioLimits,
    ) -> Result<Self, McpClientError> {
        let redactor = spec.redactor();
        let mut command = Command::new(&spec.command);
        command.args(&spec.args);
        if let Some(cwd) = &spec.cwd {
            command.current_dir(cwd);
        }
        command.env_clear();
        command.envs(&spec.env);
        command.stdin(std::process::Stdio::piped());
        command.stdout(std::process::Stdio::piped());
        command.stderr(std::process::Stdio::piped());
        command.kill_on_drop(true);

        let mut child = command.spawn().map_err(|err| {
            McpClientError::new(
                &spec.name,
                McpPhase::Spawn,
                McpErrorKind::Io(redactor.redact(&err.to_string())),
            )
        })?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let missing_pipe = if stdin.is_none() {
            Some("stdin")
        } else if stdout.is_none() {
            Some("stdout")
        } else if stderr.is_none() {
            Some("stderr")
        } else {
            None
        };

        let stdin: SharedStdin =
            Arc::new(Mutex::new(stdin.map(|stdin| {
                Box::new(stdin) as Box<dyn AsyncWrite + Send + Unpin>
            })));
        let diagnostics = Arc::new(Mutex::new(BoundedDiagnostics::new(
            spec.name.clone(),
            limits.max_diagnostic_lines,
            redactor.clone(),
        )));
        let list_changes = Arc::new(Mutex::new(BoundedListChanged::new(spec.name.clone())));
        let (tx, rx) = mpsc::channel(16);
        // Do not return an error that discards an already spawned child. Queue
        // setup failure before reader events so initialization cannot succeed.
        if let Some(pipe) = missing_pipe {
            tx.try_send(ReaderEvent::Error(McpClientError::new(
                &spec.name,
                McpPhase::Spawn,
                McpErrorKind::Protocol(format!("child {pipe} was not piped")),
            )))
            .expect("new response channel has capacity");
        }
        let reader_task = match stdout {
            Some(stdout) => spawn_stdout_reader(
                spec.name.clone(),
                stdout,
                stdin.clone(),
                tx,
                limits.clone(),
                redactor.clone(),
                list_changes.clone(),
            ),
            None => tokio::spawn(async {}),
        };
        let stderr_task = match stderr {
            Some(stderr) => spawn_stderr_reader(stderr, diagnostics.clone(), limits.clone()),
            None => tokio::spawn(async {}),
        };

        Ok(Self {
            server_name: spec.name,
            limits,
            redactor,
            diagnostics,
            list_changes,
            stdin,
            child: Some(child),
            responses: rx,
            reader_task,
            stderr_task,
            next_id: 1,
            initialized: None,
            shutdown_started: false,
        })
    }

    /// Perform the bounded handshake on an already retained client. Failure or
    /// cancellation does not drop or shut down the client: its owner must do so.
    /// Do not retry initialization on a partially initialized connection.
    pub async fn initialize_with_timeout(&mut self) -> Result<(), McpClientError> {
        let started = Instant::now();
        match timeout(self.limits.startup_timeout, self.initialize()).await {
            Ok(result) => result,
            Err(_) => Err(McpClientError::new(
                &self.server_name,
                McpPhase::Initialize,
                McpErrorKind::Timeout {
                    operation: "startup".to_string(),
                    elapsed: started.elapsed(),
                },
            )
            .with_diagnostics(self.snapshot_diagnostics().await)),
        }
    }

    async fn initialize(&mut self) -> Result<(), McpClientError> {
        let result: InitializeResult = self
            .request(
                McpPhase::Initialize,
                "initialize",
                json!({
                    "protocolVersion": MCP_PROTOCOL_VERSION,
                    "capabilities": {},
                    "clientInfo": {
                        "name": CLIENT_NAME,
                        "version": CLIENT_VERSION,
                    }
                }),
            )
            .await?;
        self.initialized = Some(result);
        self.write_notification(
            McpPhase::Initialized,
            "notifications/initialized",
            json!({}),
        )
        .await?;
        Ok(())
    }

    pub fn initialize_result(&self) -> Option<&InitializeResult> {
        self.initialized.as_ref()
    }

    /// Request one page of the MCP `tools/list` surface after initialization.
    ///
    /// This performs discovery only. It never sends `tools/call` and does not
    /// expose resources or prompts.
    pub async fn list_tools_page(
        &mut self,
        cursor: Option<String>,
    ) -> Result<ListToolsResult, McpClientError> {
        let params = cursor
            .map(|cursor| json!({ "cursor": cursor }))
            .unwrap_or_else(|| json!({}));
        self.request(McpPhase::Running, "tools/list", params).await
    }

    /// Execute an initialized MCP `tools/call` request.
    ///
    /// The caller is responsible for applying Yoi tool permissions before this
    /// method is reached and for bounding/serializing the untrusted result before
    /// it is exposed to model-visible tool history.
    ///
    /// Timeout or cancellation after writing begins can leave execution unknown;
    /// this method never retries. Interrupted write/flush closes stdin, and the
    /// caller must shut down the process rather than reuse that connection. A
    /// fully flushed frame permits later *different* requests with distinct IDs,
    /// but is not permission to replay this call when its response is missing.
    pub async fn call_tool(
        &mut self,
        request: CallToolRequest,
    ) -> Result<CallToolResult, McpClientError> {
        let params = serde_json::to_value(request).map_err(|err| {
            McpClientError::new(
                &self.server_name,
                McpPhase::Running,
                McpErrorKind::Protocol(format!("failed to serialize tools/call request: {err}")),
            )
        })?;
        self.request(McpPhase::Running, "tools/call", params).await
    }

    /// Request one page of the MCP `resources/list` surface after initialization.
    pub async fn list_resources_page(
        &mut self,
        cursor: Option<String>,
    ) -> Result<ListResourcesResult, McpClientError> {
        let params = cursor
            .map(|cursor| json!({ "cursor": cursor }))
            .unwrap_or_else(|| json!({}));
        self.request(McpPhase::Running, "resources/list", params)
            .await
    }

    /// Read one MCP resource by URI after initialization.
    pub async fn read_resource(
        &mut self,
        request: ReadResourceRequest,
    ) -> Result<ReadResourceResult, McpClientError> {
        let params = serde_json::to_value(request).map_err(|err| {
            McpClientError::new(
                &self.server_name,
                McpPhase::Running,
                McpErrorKind::Protocol(format!(
                    "failed to serialize resources/read request: {err}"
                )),
            )
        })?;
        self.request(McpPhase::Running, "resources/read", params)
            .await
    }

    /// Request one page of the MCP `prompts/list` surface after initialization.
    pub async fn list_prompts_page(
        &mut self,
        cursor: Option<String>,
    ) -> Result<ListPromptsResult, McpClientError> {
        let params = cursor
            .map(|cursor| json!({ "cursor": cursor }))
            .unwrap_or_else(|| json!({}));
        self.request(McpPhase::Running, "prompts/list", params)
            .await
    }

    /// Get one MCP prompt template by name after initialization.
    pub async fn get_prompt(
        &mut self,
        request: GetPromptRequest,
    ) -> Result<GetPromptResult, McpClientError> {
        let params = serde_json::to_value(request).map_err(|err| {
            McpClientError::new(
                &self.server_name,
                McpPhase::Running,
                McpErrorKind::Protocol(format!("failed to serialize prompts/get request: {err}")),
            )
        })?;
        self.request(McpPhase::Running, "prompts/get", params).await
    }

    /// Request pages from `tools/list` up to a host-supplied page/tool bound.
    ///
    /// Bounds are enforced by the host so a server cannot make startup discovery
    /// unbounded through pagination.
    pub async fn list_tools_bounded(
        &mut self,
        limits: McpToolListLimits,
    ) -> Result<ListToolsResult, McpClientError> {
        let mut tools = Vec::new();
        let mut cursor = None;
        let mut pages = 0usize;
        loop {
            if pages >= limits.max_pages {
                return Err(McpClientError::new(
                    &self.server_name,
                    McpPhase::Running,
                    McpErrorKind::Protocol(format!(
                        "tools/list exceeded {} page(s)",
                        limits.max_pages
                    )),
                )
                .with_diagnostics(self.snapshot_diagnostics().await));
            }
            pages += 1;
            let result = self.list_tools_page(cursor.take()).await?;
            for tool in result.tools {
                if tools.len() >= limits.max_tools {
                    return Err(McpClientError::new(
                        &self.server_name,
                        McpPhase::Running,
                        McpErrorKind::Protocol(format!(
                            "tools/list exceeded {} tool(s)",
                            limits.max_tools
                        )),
                    )
                    .with_diagnostics(self.snapshot_diagnostics().await));
                }
                tools.push(tool);
            }
            cursor = result.next_cursor;
            if cursor.is_none() {
                return Ok(ListToolsResult {
                    tools,
                    next_cursor: None,
                    meta: result.meta,
                    extra: BTreeMap::new(),
                });
            }
        }
    }

    pub async fn snapshot_diagnostics(&self) -> McpDiagnostics {
        self.diagnostics.lock().await.snapshot()
    }

    /// Return bounded list-change signals observed so far for this connection.
    ///
    /// This is diagnostic/freshness state only. It never contains notification
    /// params and must not be used to mutate an active run's model-visible tool
    /// schema outside an explicit safe boundary.
    pub async fn snapshot_list_changes(&self) -> McpListChangedSnapshot {
        self.list_changes.lock().await.snapshot()
    }

    /// Clear observed list-change signals before an explicit safe-boundary
    /// refresh. New notifications received after this call will be recorded.
    pub async fn clear_list_changes(&self) {
        self.list_changes.lock().await.clear();
    }

    pub async fn request<T: for<'de> Deserialize<'de>>(
        &mut self,
        phase: McpPhase,
        method: &str,
        params: Value,
    ) -> Result<T, McpClientError> {
        let id = self.next_id;
        self.next_id += 1;
        let request = ClientRequest {
            jsonrpc: JSONRPC_VERSION,
            id,
            method,
            params,
        };
        if self.shutdown_started {
            return Err(McpClientError::new(
                &self.server_name,
                phase,
                McpErrorKind::Io("client shutdown has started".into()),
            ));
        }
        let deadline = Instant::now() + self.limits.request_timeout;
        let mut stage = McpRequestStage::WaitingForStdin;
        // One budget covers ownership, write, flush, and the matching response.
        let response = match timeout_at(deadline, async {
            self.write_protocol(phase, &request, &mut stage).await?;
            stage = McpRequestStage::AwaitingResponse;
            self.wait_for_response(id, phase).await
        })
        .await
        {
            Ok(Ok(response)) => response,
            Ok(Err(err)) => return Err(err.with_request_stage(stage)),
            Err(_) => {
                return Err(McpClientError::new(
                    &self.server_name,
                    phase,
                    McpErrorKind::Timeout {
                        operation: self.redactor.redact(method),
                        elapsed: self.limits.request_timeout,
                    },
                )
                .with_request_stage(stage)
                .with_diagnostics(self.snapshot_diagnostics().await));
            }
        };
        if let Some(error) = response.error {
            return Err(McpClientError::new(
                &self.server_name,
                phase,
                McpErrorKind::JsonRpcError {
                    code: error.code,
                    message: self.redactor.redact(&error.message),
                },
            )
            .with_diagnostics(self.snapshot_diagnostics().await));
        }
        let result = response.result.ok_or_else(|| {
            McpClientError::new(
                &self.server_name,
                phase,
                McpErrorKind::Protocol(format!("response to `{method}` did not contain result")),
            )
        })?;
        serde_json::from_value(result).map_err(|err| {
            McpClientError::new(
                &self.server_name,
                phase,
                McpErrorKind::Protocol(format!("invalid `{method}` result: {err}")),
            )
        })
    }

    async fn wait_for_response(
        &mut self,
        id: u64,
        phase: McpPhase,
    ) -> Result<ServerResponse, McpClientError> {
        while let Some(event) = self.responses.recv().await {
            match event {
                ReaderEvent::Response(response) if response.id == id => return Ok(response),
                ReaderEvent::Response(_) | ReaderEvent::Notification => continue,
                ReaderEvent::Error(err) => return Err(err.with_phase(phase)),
                ReaderEvent::Eof => {
                    return Err(McpClientError::new(
                        &self.server_name,
                        phase,
                        McpErrorKind::Protocol("server stdout closed before response".into()),
                    )
                    .with_diagnostics(self.snapshot_diagnostics().await));
                }
            }
        }
        Err(McpClientError::new(
            &self.server_name,
            phase,
            McpErrorKind::Protocol("stdout reader stopped before response".into()),
        )
        .with_diagnostics(self.snapshot_diagnostics().await))
    }

    async fn write_notification<T: Serialize>(
        &mut self,
        phase: McpPhase,
        method: &str,
        params: T,
    ) -> Result<(), McpClientError> {
        let mut stage = McpRequestStage::WaitingForStdin;
        self.write_protocol(
            phase,
            &ClientNotification {
                jsonrpc: JSONRPC_VERSION,
                method,
                params,
            },
            &mut stage,
        )
        .await
    }

    async fn write_protocol<T: Serialize>(
        &mut self,
        phase: McpPhase,
        value: &T,
        stage: &mut McpRequestStage,
    ) -> Result<(), McpClientError> {
        write_json_line(
            &self.server_name,
            phase,
            &self.stdin,
            value,
            self.limits.max_protocol_bytes,
            &self.redactor,
            stage,
        )
        .await
    }

    /// Close stdin and wait for process exit, falling back to terminate and kill.
    /// Errors and cancellation retain the child for another attempt on this
    /// client. Only confirmed reaping releases the process handle.
    pub async fn shutdown(&mut self) -> Result<ShutdownReport, McpClientError> {
        self.shutdown_started = true;
        let deadline = Instant::now() + self.limits.shutdown_timeout;
        // Ownership and EOF share one graceful budget. A reader reply may own
        // stdin indefinitely; its lock is never required to terminate/kill.
        let closed = timeout_at(deadline, async {
            self.stdin.lock().await.take();
        })
        .await
        .is_ok();

        // Keep the actual process handle in the client across every await and
        // error. Taking it before wait loses retry/reaping authority on cancel.
        if self.child.is_none() {
            return Ok(ShutdownReport::already_finished());
        }
        if !closed {
            self.reader_task.abort();
            return self.terminate_then_kill().await;
        }

        let child = self.child.as_mut().expect("checked child");
        if let Ok(Some(status)) = child.try_wait() {
            return Ok(self.finish_shutdown(status, false, false));
        }

        match timeout_at(deadline, child.wait()).await {
            Ok(Ok(status)) => Ok(self.finish_shutdown(status, false, false)),
            Ok(Err(err)) => Err(McpClientError::new(
                &self.server_name,
                McpPhase::Shutdown,
                McpErrorKind::Io(self.redactor.redact(&err.to_string())),
            )
            .with_diagnostics(self.snapshot_diagnostics().await)),
            Err(_) => self.terminate_then_kill().await,
        }
    }

    fn finish_shutdown(
        &mut self,
        status: ExitStatus,
        terminated: bool,
        killed: bool,
    ) -> ShutdownReport {
        // No await separates confirmed reaping from releasing the handle.
        drop(self.child.take());
        self.reader_task.abort();
        self.stderr_task.abort();
        ShutdownReport {
            exit_status: Some(status),
            terminated,
            killed,
        }
    }

    async fn terminate_then_kill(&mut self) -> Result<ShutdownReport, McpClientError> {
        let child = self.child.as_mut().expect("shutdown retains child");
        let terminated = send_terminate(child).is_ok();
        match timeout(self.limits.kill_timeout, child.wait()).await {
            Ok(Ok(status)) => Ok(self.finish_shutdown(status, terminated, false)),
            Ok(Err(err)) => Err(McpClientError::new(
                &self.server_name,
                McpPhase::Shutdown,
                McpErrorKind::Io(self.redactor.redact(&err.to_string())),
            )
            .with_diagnostics(self.snapshot_diagnostics().await)),
            Err(_) => {
                child.start_kill().map_err(|err| {
                    McpClientError::new(
                        &self.server_name,
                        McpPhase::Shutdown,
                        McpErrorKind::Io(self.redactor.redact(&err.to_string())),
                    )
                })?;
                let status = timeout(self.limits.kill_timeout, child.wait())
                    .await
                    .map_err(|_| {
                        McpClientError::new(
                            &self.server_name,
                            McpPhase::Shutdown,
                            McpErrorKind::Timeout {
                                operation: "kill".to_string(),
                                elapsed: self.limits.kill_timeout,
                            },
                        )
                    })?
                    .map_err(|err| {
                        McpClientError::new(
                            &self.server_name,
                            McpPhase::Shutdown,
                            McpErrorKind::Io(self.redactor.redact(&err.to_string())),
                        )
                    })?;
                Ok(self.finish_shutdown(status, terminated, true))
            }
        }
    }
}

impl Drop for McpStdioClient {
    fn drop(&mut self) {
        // An attempted shutdown is not proof of exit. Errors and cancellation
        // leave the child here so Drop must still make a best-effort kill.
        if let Some(child) = &mut self.child {
            let _ = child.start_kill();
        }
        self.reader_task.abort();
        self.stderr_task.abort();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpPhase {
    Spawn,
    Initialize,
    Initialized,
    Running,
    Shutdown,
}

impl fmt::Display for McpPhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Spawn => f.write_str("spawn"),
            Self::Initialize => f.write_str("initialize"),
            Self::Initialized => f.write_str("initialized"),
            Self::Running => f.write_str("running"),
            Self::Shutdown => f.write_str("shutdown"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ShutdownReport {
    pub exit_status: Option<ExitStatus>,
    pub terminated: bool,
    pub killed: bool,
}

impl ShutdownReport {
    fn already_finished() -> Self {
        Self {
            exit_status: None,
            terminated: false,
            killed: false,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    pub protocol_version: String,
    #[serde(default)]
    pub capabilities: Value,
    pub server_info: ImplementationInfo,
    #[serde(default)]
    pub instructions: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImplementationInfo {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone)]
pub struct McpDiagnostics {
    pub server_name: String,
    pub stderr: Vec<String>,
    pub dropped_stderr_lines: usize,
    pub truncated_stderr_lines: usize,
}

#[derive(Debug, Error, Clone)]
#[error("MCP stdio server `{server_name}` failed during phase `{phase}`: {kind}{stage_detail}", stage_detail = request_stage.map(|stage| format!("; {stage}")).unwrap_or_default())]
pub struct McpClientError {
    pub server_name: String,
    pub phase: McpPhase,
    pub kind: McpErrorKind,
    diagnostics: Option<McpDiagnostics>,
    request_stage: Option<McpRequestStage>,
}

impl McpClientError {
    fn new(server_name: impl Into<String>, phase: McpPhase, kind: McpErrorKind) -> Self {
        Self {
            server_name: server_name.into(),
            phase,
            kind,
            diagnostics: None,
            request_stage: None,
        }
    }

    fn with_phase(mut self, phase: McpPhase) -> Self {
        self.phase = phase;
        self
    }

    pub fn with_diagnostics(mut self, diagnostics: McpDiagnostics) -> Self {
        self.diagnostics = Some(diagnostics);
        self
    }

    fn with_request_stage(mut self, stage: McpRequestStage) -> Self {
        self.request_stage = Some(stage);
        self
    }

    pub fn request_stage(&self) -> Option<McpRequestStage> {
        self.request_stage
    }

    pub fn diagnostics(&self) -> Option<&McpDiagnostics> {
        self.diagnostics.as_ref()
    }
}

#[derive(Debug, Error, Clone)]
pub enum McpErrorKind {
    #[error("configuration error: {0}")]
    Config(String),
    #[error("I/O error: {0}")]
    Io(String),
    #[error("protocol error: {0}")]
    Protocol(String),
    #[error("JSON-RPC error {code}: {message}")]
    JsonRpcError { code: i64, message: String },
    #[error("timed out during {operation} after {elapsed:?}")]
    Timeout {
        operation: String,
        elapsed: Duration,
    },
}

#[derive(Debug)]
enum ReaderEvent {
    Response(ServerResponse),
    Notification,
    Error(McpClientError),
    Eof,
}

#[derive(Debug, Serialize)]
struct ClientRequest<'a> {
    jsonrpc: &'static str,
    id: u64,
    method: &'a str,
    params: Value,
}

#[derive(Debug, Serialize)]
struct ClientNotification<'a, T> {
    jsonrpc: &'static str,
    method: &'a str,
    params: T,
}

#[derive(Debug, Deserialize)]
struct IncomingMessage {
    #[allow(dead_code)]
    jsonrpc: Option<String>,
    id: Option<Value>,
    method: Option<String>,
    result: Option<Value>,
    error: Option<RpcError>,
    #[allow(dead_code)]
    params: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct ServerResponse {
    id: u64,
    result: Option<Value>,
    error: Option<RpcError>,
}

#[derive(Debug, Deserialize)]
struct RpcError {
    code: i64,
    message: String,
    #[allow(dead_code)]
    data: Option<Value>,
}

#[derive(Debug, Serialize)]
struct ErrorResponse<'a> {
    jsonrpc: &'static str,
    id: Value,
    error: ErrorObject<'a>,
}

#[derive(Debug, Serialize)]
struct ErrorObject<'a> {
    code: i64,
    message: &'a str,
}

fn spawn_stdout_reader(
    server_name: String,
    stdout: ChildStdout,
    stdin: SharedStdin,
    tx: mpsc::Sender<ReaderEvent>,
    limits: McpStdioLimits,
    redactor: Redactor,
    list_changes: Arc<Mutex<BoundedListChanged>>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut stdout = BufReader::new(stdout);
        loop {
            match read_protocol_line(&mut stdout, limits.max_stdout_line_bytes).await {
                Ok(Some(line)) => match serde_json::from_slice::<IncomingMessage>(&line) {
                    Ok(message) => {
                        handle_incoming_message(
                            &server_name,
                            &stdin,
                            &tx,
                            &limits,
                            &redactor,
                            &list_changes,
                            message,
                        )
                        .await
                    }
                    Err(err) => {
                        let _ = tx
                            .send(ReaderEvent::Error(McpClientError::new(
                                &server_name,
                                McpPhase::Running,
                                McpErrorKind::Protocol(format!(
                                    "invalid stdout JSON-RPC message: {err}"
                                )),
                            )))
                            .await;
                        break;
                    }
                },
                Ok(None) => {
                    let _ = tx.send(ReaderEvent::Eof).await;
                    break;
                }
                Err(err) => {
                    let _ = tx
                        .send(ReaderEvent::Error(McpClientError::new(
                            &server_name,
                            McpPhase::Running,
                            McpErrorKind::Protocol(err),
                        )))
                        .await;
                    break;
                }
            }
        }
    })
}

async fn handle_incoming_message(
    server_name: &str,
    stdin: &SharedStdin,
    tx: &mpsc::Sender<ReaderEvent>,
    limits: &McpStdioLimits,
    redactor: &Redactor,
    list_changes: &Arc<Mutex<BoundedListChanged>>,
    message: IncomingMessage,
) {
    if message.method.is_some() && message.id.is_some() {
        if let Some(id) = message.id {
            let response = ErrorResponse {
                jsonrpc: JSONRPC_VERSION,
                id,
                error: ErrorObject {
                    code: ERR_METHOD_NOT_FOUND,
                    message: "server-to-client requests are not supported by this client",
                },
            };
            let _ = write_json_line(
                server_name,
                McpPhase::Running,
                stdin,
                &response,
                limits.max_protocol_bytes,
                redactor,
                &mut McpRequestStage::WaitingForStdin,
            )
            .await;
        }
        return;
    }

    if let Some(method) = message.method.as_deref() {
        if let Some(kind) = McpListChangedKind::from_notification_method(method) {
            list_changes.lock().await.mark(kind);
        }
        let _ = tx.send(ReaderEvent::Notification).await;
        return;
    }

    if let Some(id) = message.id.as_ref().and_then(Value::as_u64) {
        let _ = tx
            .send(ReaderEvent::Response(ServerResponse {
                id,
                result: message.result,
                error: message.error,
            }))
            .await;
        return;
    }

    let _ = tx
        .send(ReaderEvent::Error(McpClientError::new(
            server_name,
            McpPhase::Running,
            McpErrorKind::Protocol(
                "JSON-RPC response id was missing or not an unsigned integer".into(),
            ),
        )))
        .await;
}

#[derive(Debug)]
struct BoundedListChanged {
    server_name: String,
    kinds: BTreeSet<McpListChangedKind>,
}

impl BoundedListChanged {
    fn new(server_name: String) -> Self {
        Self {
            server_name,
            kinds: BTreeSet::new(),
        }
    }

    fn mark(&mut self, kind: McpListChangedKind) {
        self.kinds.insert(kind);
    }

    fn clear(&mut self) {
        self.kinds.clear();
    }

    fn snapshot(&self) -> McpListChangedSnapshot {
        McpListChangedSnapshot {
            server_name: self.server_name.clone(),
            kinds: self.kinds.clone(),
        }
    }
}

fn spawn_stderr_reader(
    stderr: ChildStderr,
    diagnostics: Arc<Mutex<BoundedDiagnostics>>,
    limits: McpStdioLimits,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut stderr = BufReader::new(stderr);
        loop {
            match read_diagnostic_line(&mut stderr, limits.max_stderr_line_bytes).await {
                Ok(Some((line, truncated))) => diagnostics.lock().await.push(line, truncated),
                Ok(None) => break,
                Err(err) => {
                    diagnostics
                        .lock()
                        .await
                        .push(format!("stderr read error: {err}"), false);
                    break;
                }
            }
        }
    })
}

async fn write_json_line<T: Serialize>(
    server_name: &str,
    phase: McpPhase,
    stdin: &SharedStdin,
    value: &T,
    max_protocol_bytes: usize,
    redactor: &Redactor,
    stage: &mut McpRequestStage,
) -> Result<(), McpClientError> {
    let mut bytes = serde_json::to_vec(value).map_err(|err| {
        McpClientError::new(
            server_name,
            phase,
            McpErrorKind::Protocol(format!("failed to encode JSON-RPC message: {err}")),
        )
    })?;
    if bytes.len() > max_protocol_bytes {
        return Err(McpClientError::new(
            server_name,
            phase,
            McpErrorKind::Protocol(format!(
                "JSON-RPC payload exceeded {max_protocol_bytes} bytes"
            )),
        ));
    }
    bytes.push(b'\n');
    let mut guard = stdin.lock().await;
    let Some(mut writer) = guard.take() else {
        return Err(McpClientError::new(
            server_name,
            phase,
            McpErrorKind::Io("child stdin is closed".into()),
        ));
    };
    // Keep ownership out of the slot across both awaits. Timeout/cancel or
    // I/O error drops the writer, closing the connection instead of allowing
    // the next frame to append to an unverified partial JSON line.
    *stage = McpRequestStage::WritingFrame;
    writer.write_all(&bytes).await.map_err(|err| {
        McpClientError::new(
            server_name,
            phase,
            McpErrorKind::Io(redactor.redact(&err.to_string())),
        )
    })?;
    *stage = McpRequestStage::FlushingFrame;
    writer.flush().await.map_err(|err| {
        McpClientError::new(
            server_name,
            phase,
            McpErrorKind::Io(redactor.redact(&err.to_string())),
        )
    })?;
    *guard = Some(writer);
    Ok(())
}

async fn read_protocol_line<R: AsyncRead + Unpin>(
    reader: &mut R,
    max_bytes: usize,
) -> Result<Option<Vec<u8>>, String> {
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        let read = reader
            .read(&mut byte)
            .await
            .map_err(|err| err.to_string())?;
        if read == 0 {
            return if buf.is_empty() {
                Ok(None)
            } else {
                Ok(Some(trim_newline(buf)))
            };
        }
        if byte[0] == b'\n' {
            return Ok(Some(trim_newline(buf)));
        }
        if buf.len() >= max_bytes {
            return Err(format!("stdout line exceeded {max_bytes} bytes"));
        }
        buf.push(byte[0]);
    }
}

async fn read_diagnostic_line<R: AsyncRead + Unpin>(
    reader: &mut R,
    max_bytes: usize,
) -> Result<Option<(String, bool)>, String> {
    let mut buf = Vec::new();
    let mut truncated = false;
    let mut byte = [0u8; 1];
    loop {
        let read = reader
            .read(&mut byte)
            .await
            .map_err(|err| err.to_string())?;
        if read == 0 {
            if buf.is_empty() && !truncated {
                return Ok(None);
            }
            return Ok(Some((
                String::from_utf8_lossy(&trim_newline(buf)).into_owned(),
                truncated,
            )));
        }
        if byte[0] == b'\n' {
            return Ok(Some((
                String::from_utf8_lossy(&trim_newline(buf)).into_owned(),
                truncated,
            )));
        }
        if buf.len() < max_bytes {
            buf.push(byte[0]);
        } else {
            truncated = true;
        }
    }
}

fn trim_newline(mut buf: Vec<u8>) -> Vec<u8> {
    if buf.last() == Some(&b'\r') {
        buf.pop();
    }
    buf
}

#[derive(Debug)]
struct BoundedDiagnostics {
    server_name: String,
    max_lines: usize,
    lines: VecDeque<String>,
    dropped_lines: usize,
    truncated_lines: usize,
    redactor: Redactor,
}

impl BoundedDiagnostics {
    fn new(server_name: String, max_lines: usize, redactor: Redactor) -> Self {
        Self {
            server_name,
            max_lines,
            lines: VecDeque::new(),
            dropped_lines: 0,
            truncated_lines: 0,
            redactor,
        }
    }

    fn push(&mut self, line: String, truncated: bool) {
        if truncated {
            self.truncated_lines += 1;
        }
        if self.max_lines == 0 {
            self.dropped_lines += 1;
            return;
        }
        if self.lines.len() == self.max_lines {
            self.lines.pop_front();
            self.dropped_lines += 1;
        }
        let suffix = if truncated { "… [truncated]" } else { "" };
        self.lines
            .push_back(format!("{}{suffix}", self.redactor.redact(&line)));
    }

    fn snapshot(&self) -> McpDiagnostics {
        McpDiagnostics {
            server_name: self.server_name.clone(),
            stderr: self.lines.iter().cloned().collect(),
            dropped_stderr_lines: self.dropped_lines,
            truncated_stderr_lines: self.truncated_lines,
        }
    }
}

#[derive(Debug, Clone)]
struct Redactor {
    values: Vec<String>,
}

impl Redactor {
    fn new(mut values: Vec<String>) -> Self {
        values.retain(|value| !value.is_empty());
        values.sort_by_key(|value| std::cmp::Reverse(value.len()));
        values.dedup();
        Self { values }
    }

    fn redact(&self, input: &str) -> String {
        let mut output = input.to_owned();
        for value in &self.values {
            output = output.replace(value, "[redacted]");
        }
        output
    }
}

#[cfg(unix)]
fn send_terminate(child: &mut Child) -> Result<(), ()> {
    let Some(pid) = child.id() else {
        return Err(());
    };
    let result = unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
    if result == 0 { Ok(()) } else { Err(()) }
}

#[cfg(not(unix))]
fn send_terminate(child: &mut Child) -> Result<(), ()> {
    child.start_kill().map_err(|_| ())
}

#[cfg(test)]
mod deadline_tests;

#[cfg(all(test, unix))]
mod shutdown_retention_tests {
    //! The real client and OS process are used; only the external server is a
    //! local shell that acknowledges initialization and ignores TERM/EOF.
    use super::*;
    use std::future::{Future, poll_fn};
    use std::task::Poll;

    #[tokio::test]
    async fn cancelled_graceful_or_terminate_wait_retries_reaping_the_same_child() {
        for cancel_in_fallback in [false, true] {
            let script = r#"trap '' TERM
read init || exit 1
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-11-25","capabilities":{},"serverInfo":{"name":"retained","version":"1"}}}'
while :; do :; done
"#;
            let mut client = McpStdioClient::connect(
                McpStdioServerSpec::new("retained", "/bin/sh").args(["-c", script]),
                McpStdioLimits {
                    shutdown_timeout: if cancel_in_fallback {
                        Duration::ZERO
                    } else {
                        Duration::from_secs(30)
                    },
                    kill_timeout: Duration::from_secs(30),
                    ..Default::default()
                },
            )
            .await
            .expect("initialize after installing TERM handler");
            let pid = client.child.as_ref().unwrap().id();
            let mut shutdown = Box::pin(client.shutdown());
            poll_fn(|cx| {
                assert!(shutdown.as_mut().poll(cx).is_pending());
                Poll::Ready(())
            })
            .await;
            drop(shutdown);
            assert_eq!(client.child.as_ref().and_then(Child::id), pid);
            assert!(client.shutdown_started);

            client.limits.shutdown_timeout = Duration::from_millis(20);
            client.limits.kill_timeout = Duration::from_millis(50);
            let report = timeout(Duration::from_secs(2), client.shutdown())
                .await
                .expect("retry bounded")
                .expect("retry reaps original child");
            assert!(
                report.exit_status.is_some(),
                "not an already-finished receipt"
            );
            assert!(report.killed, "TERM was ignored");
            assert!(client.child.is_none());
            let mut status = 0;
            assert_eq!(
                unsafe { libc::waitpid(pid.unwrap() as libc::pid_t, &mut status, libc::WNOHANG) },
                -1
            );
            assert_eq!(
                std::io::Error::last_os_error().raw_os_error(),
                Some(libc::ECHILD),
                "shutdown, not a later observer, reaped the original process"
            );
        }
    }
}
