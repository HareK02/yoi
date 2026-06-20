use std::collections::{BTreeMap, VecDeque};
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
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};
use tokio::sync::{Mutex, mpsc};
use tokio::task::JoinHandle;
use tokio::time::{Instant, timeout};

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
    pub request_timeout: Duration,
    pub shutdown_timeout: Duration,
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

/// A running initialized stdio MCP client.
pub struct McpStdioClient {
    server_name: String,
    limits: McpStdioLimits,
    redactor: Redactor,
    diagnostics: Arc<Mutex<BoundedDiagnostics>>,
    stdin: Arc<Mutex<Option<ChildStdin>>>,
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
        let stdin = child.stdin.take().ok_or_else(|| {
            McpClientError::new(
                &spec.name,
                McpPhase::Spawn,
                McpErrorKind::Protocol("child stdin was not piped".into()),
            )
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            McpClientError::new(
                &spec.name,
                McpPhase::Spawn,
                McpErrorKind::Protocol("child stdout was not piped".into()),
            )
        })?;
        let stderr = child.stderr.take().ok_or_else(|| {
            McpClientError::new(
                &spec.name,
                McpPhase::Spawn,
                McpErrorKind::Protocol("child stderr was not piped".into()),
            )
        })?;

        let stdin = Arc::new(Mutex::new(Some(stdin)));
        let diagnostics = Arc::new(Mutex::new(BoundedDiagnostics::new(
            spec.name.clone(),
            limits.max_diagnostic_lines,
            redactor.clone(),
        )));
        let (tx, rx) = mpsc::channel(16);
        let reader_task = spawn_stdout_reader(
            spec.name.clone(),
            stdout,
            stdin.clone(),
            tx,
            limits.clone(),
            redactor.clone(),
        );
        let stderr_task = spawn_stderr_reader(stderr, diagnostics.clone(), limits.clone());

        Ok(Self {
            server_name: spec.name,
            limits,
            redactor,
            diagnostics,
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
        self.write_protocol(phase, &request).await?;
        let response = match timeout(
            self.limits.request_timeout,
            self.wait_for_response(id, phase),
        )
        .await
        {
            Ok(result) => result?,
            Err(_) => {
                return Err(McpClientError::new(
                    &self.server_name,
                    phase,
                    McpErrorKind::Timeout {
                        operation: method.to_owned(),
                        elapsed: self.limits.request_timeout,
                    },
                )
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
        self.write_protocol(
            phase,
            &ClientNotification {
                jsonrpc: JSONRPC_VERSION,
                method,
                params,
            },
        )
        .await
    }

    async fn write_protocol<T: Serialize>(
        &mut self,
        phase: McpPhase,
        value: &T,
    ) -> Result<(), McpClientError> {
        write_json_line(
            &self.server_name,
            phase,
            &self.stdin,
            value,
            self.limits.max_protocol_bytes,
            &self.redactor,
        )
        .await
    }

    /// Close stdin and wait for process exit, falling back to terminate and kill.
    pub async fn shutdown(&mut self) -> Result<ShutdownReport, McpClientError> {
        self.shutdown_started = true;
        {
            let mut stdin = self.stdin.lock().await;
            stdin.take();
        }

        let mut child = match self.child.take() {
            Some(child) => child,
            None => return Ok(ShutdownReport::already_finished()),
        };

        if let Ok(Some(status)) = child.try_wait() {
            self.reader_task.abort();
            self.stderr_task.abort();
            return Ok(ShutdownReport {
                exit_status: Some(status),
                terminated: false,
                killed: false,
            });
        }

        match timeout(self.limits.shutdown_timeout, child.wait()).await {
            Ok(Ok(status)) => {
                self.reader_task.abort();
                self.stderr_task.abort();
                Ok(ShutdownReport {
                    exit_status: Some(status),
                    terminated: false,
                    killed: false,
                })
            }
            Ok(Err(err)) => Err(McpClientError::new(
                &self.server_name,
                McpPhase::Shutdown,
                McpErrorKind::Io(self.redactor.redact(&err.to_string())),
            )
            .with_diagnostics(self.snapshot_diagnostics().await)),
            Err(_) => self.terminate_then_kill(child).await,
        }
    }

    async fn terminate_then_kill(
        &mut self,
        mut child: Child,
    ) -> Result<ShutdownReport, McpClientError> {
        let mut terminated = false;
        let mut killed = false;
        if send_terminate(&mut child).is_ok() {
            terminated = true;
        }
        match timeout(self.limits.kill_timeout, child.wait()).await {
            Ok(Ok(status)) => {
                self.reader_task.abort();
                self.stderr_task.abort();
                Ok(ShutdownReport {
                    exit_status: Some(status),
                    terminated,
                    killed,
                })
            }
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
                killed = true;
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
                self.reader_task.abort();
                self.stderr_task.abort();
                Ok(ShutdownReport {
                    exit_status: Some(status),
                    terminated,
                    killed,
                })
            }
        }
    }
}

impl Drop for McpStdioClient {
    fn drop(&mut self) {
        if !self.shutdown_started {
            if let Some(child) = &mut self.child {
                let _ = child.start_kill();
            }
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
#[error("MCP stdio server `{server_name}` failed during phase `{phase}`: {kind}")]
pub struct McpClientError {
    pub server_name: String,
    pub phase: McpPhase,
    pub kind: McpErrorKind,
    diagnostics: Option<McpDiagnostics>,
}

impl McpClientError {
    fn new(server_name: impl Into<String>, phase: McpPhase, kind: McpErrorKind) -> Self {
        Self {
            server_name: server_name.into(),
            phase,
            kind,
            diagnostics: None,
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
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    tx: mpsc::Sender<ReaderEvent>,
    limits: McpStdioLimits,
    redactor: Redactor,
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
    stdin: &Arc<Mutex<Option<ChildStdin>>>,
    tx: &mpsc::Sender<ReaderEvent>,
    limits: &McpStdioLimits,
    redactor: &Redactor,
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
            )
            .await;
        }
        return;
    }

    if message.method.is_some() {
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
    stdin: &Arc<Mutex<Option<ChildStdin>>>,
    value: &T,
    max_protocol_bytes: usize,
    redactor: &Redactor,
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
    let Some(stdin) = guard.as_mut() else {
        return Err(McpClientError::new(
            server_name,
            phase,
            McpErrorKind::Io("child stdin is closed".into()),
        ));
    };
    stdin.write_all(&bytes).await.map_err(|err| {
        McpClientError::new(
            server_name,
            phase,
            McpErrorKind::Io(redactor.redact(&err.to_string())),
        )
    })?;
    stdin.flush().await.map_err(|err| {
        McpClientError::new(
            server_name,
            phase,
            McpErrorKind::Io(redactor.redact(&err.to_string())),
        )
    })
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
