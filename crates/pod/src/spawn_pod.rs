//! `SpawnPod` tool — launch a new Pod process as a child of this one.
//!
//! Wires scope-lock delegation, overlay-TOML construction, subprocess
//! launch, and socket handoff into a single `Tool` implementation. When
//! the LLM calls `SpawnPod`, a fresh `pod` binary is exec'd in its own
//! process group, the scope lock is updated atomically, and the child's
//! first turn is kicked off by handing its socket a `Method::Run`.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use llm_worker::tool::{Tool, ToolDefinition, ToolError, ToolMeta, ToolOutput};
use manifest::{
    ModelConfig, ModelConfigPartial, Permission, PodManifestConfig, PodMetaConfig, ScopeConfig,
    ScopeRule, WorkerManifestConfig,
};
use protocol::Method;
use protocol::stream::JsonLineWriter;
use serde::Deserialize;
use tokio::net::UnixStream;
use tokio::process::Command;
use tokio::time::sleep;

use crate::pod_events;
use crate::runtime_dir::SpawnedPodRecord;
use crate::scope_lock::{self, LockFileGuard, ScopeLockError};
use crate::spawned_pod_registry::SpawnedPodRegistry;
use protocol::PodEvent;

const DESCRIPTION: &str = "Spawn a new Pod process to work on a delegated task. \
The spawner's write scope is reduced by the scope passed here; the spawned \
Pod receives its own socket and starts running `task` immediately. The \
spawned Pod outlives the spawner's current turn and can be contacted again \
through its socket path.";

const DEFAULT_INSTRUCTION: &str = "$insomnia/default";

/// How long we will wait for the spawned Pod's socket to become
/// connectable before treating the spawn as failed.
const SOCKET_WAIT_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct SpawnPodInput {
    /// Identifier for the spawned Pod. Must be unique machine-wide.
    name: String,
    /// Instruction-file reference (e.g. `$insomnia/default`, `$user/my-agent`).
    #[serde(default)]
    instruction: Option<String>,
    /// First message sent to the spawned Pod via `Method::Run`.
    task: String,
    /// Allow rules delegated to the spawned Pod. Must be a subset of the
    /// spawner's effective write scope.
    scope: Vec<ScopeRuleInput>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct ScopeRuleInput {
    /// Absolute target path. Relative paths are rejected.
    target: PathBuf,
    /// `"read"` or `"write"`.
    permission: PermissionInput,
    /// When `false`, the rule matches the target itself and its direct
    /// children only. Defaults to `true`.
    #[serde(default = "default_true")]
    recursive: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema, Clone, Copy)]
#[serde(rename_all = "lowercase")]
enum PermissionInput {
    Read,
    Write,
}

fn default_true() -> bool {
    true
}

impl From<PermissionInput> for Permission {
    fn from(p: PermissionInput) -> Self {
        match p {
            PermissionInput::Read => Permission::Read,
            PermissionInput::Write => Permission::Write,
        }
    }
}

/// Runtime dependencies the `SpawnPod` tool needs in order to launch a
/// child Pod and record the handoff locally. Constructed by the Pod
/// controller once per Pod lifetime.
pub struct SpawnPodTool {
    /// Spawner's own pod name — becomes the spawned Pod's
    /// `delegated_from` in the scope-lock registry.
    spawner_name: String,
    /// Path to the spawner's Unix socket. Handed to the child via
    /// `--callback` so its `PodEvent` callbacks have somewhere to land.
    callback_socket: PathBuf,
    /// Root of the `$XDG_RUNTIME_DIR/insomnia/` tree, used to predict
    /// the spawned Pod's socket path before the child has bound it.
    runtime_base: PathBuf,
    /// Directory the spawned Pod should run in when the LLM did not
    /// override it. Defaults to the spawner's pwd — see module docs.
    spawner_pwd: PathBuf,
    /// Shared registry of spawned children, also used by the
    /// pod-comm tools (`SendToPod` / `ReadPodOutput` / `StopPod` /
    /// `ListPods`). Writes the list to `spawned_pods.json` on each add.
    registry: Arc<SpawnedPodRegistry>,
    /// THIS Pod's own parent-callback socket, if any. After a
    /// successful spawn we fire `PodEvent::ScopeSubDelegated` upward
    /// so the grandparent can register the grandchild directly.
    /// `None` for top-level Pods — in that case the re-emission is a
    /// no-op.
    parent_socket: Option<PathBuf>,
    /// Spawner's resolved provider config — copied into every spawned
    /// Pod's overlay TOML so the child does not need its own provider
    /// configuration in the manifest cascade. Per-spawn override is
    /// out of scope here (see `tickets/spawn-inherit-provider.md`).
    spawner_model: ModelConfig,
}

impl SpawnPodTool {
    pub fn new(
        spawner_name: String,
        callback_socket: PathBuf,
        runtime_base: PathBuf,
        spawner_pwd: PathBuf,
        registry: Arc<SpawnedPodRegistry>,
        parent_socket: Option<PathBuf>,
        spawner_model: ModelConfig,
    ) -> Self {
        Self {
            spawner_name,
            callback_socket,
            runtime_base,
            spawner_pwd,
            registry,
            parent_socket,
            spawner_model,
        }
    }
}

#[async_trait]
impl Tool for SpawnPodTool {
    async fn execute(&self, input_json: &str) -> Result<ToolOutput, ToolError> {
        let input: SpawnPodInput = serde_json::from_str(input_json)
            .map_err(|e| ToolError::InvalidArgument(format!("invalid SpawnPod input: {e}")))?;

        // `delegate_scope` catches this too (as `DuplicatePodName`), but
        // the dedicated message is kinder to the LLM — which gets the
        // error back verbatim — than the generic duplicate-name error.
        if input.name == self.spawner_name {
            return Err(ToolError::InvalidArgument(format!(
                "spawned pod name `{}` collides with spawner's own name",
                input.name
            )));
        }

        let scope_allow = parse_scope(&input.scope)?;

        let instruction = input
            .instruction
            .clone()
            .unwrap_or_else(|| DEFAULT_INSTRUCTION.to_string());

        let predicted_socket = self.runtime_base.join(&input.name).join("sock");
        let lock_path = scope_lock::default_lock_path()
            .map_err(|e| ToolError::ExecutionFailed(format!("scope lock path: {e}")))?;

        // Reserve the allocation up front. Spawner's pid is a live
        // placeholder; the child will rewrite it via `adopt_allocation`.
        {
            let mut guard = LockFileGuard::open(&lock_path)
                .map_err(|e| ToolError::ExecutionFailed(format!("scope lock open: {e}")))?;
            scope_lock::delegate_scope(
                &mut guard,
                &self.spawner_name,
                input.name.clone(),
                std::process::id(),
                predicted_socket.clone(),
                scope_allow.clone(),
            )
            .map_err(scope_lock_err_to_tool)?;
        }

        // `start_outcome` covers steps that happen before the child is
        // observably alive (exec + socket bind). Once its socket is
        // listening, the child owns the allocation and we must not roll
        // it back — even if later steps (Method::Run delivery, record
        // write) fail, the child is running and will release its own
        // entry on exit.
        let overlay_toml = match build_overlay_toml(
            &input.name,
            &instruction,
            &scope_allow,
            &self.spawner_model,
        ) {
            Ok(s) => s,
            Err(e) => {
                self.release_reservation(&lock_path, &input.name);
                return Err(ToolError::ExecutionFailed(format!(
                    "overlay serialisation: {e}"
                )));
            }
        };

        let start_outcome = self
            .exec_child(&input.name, &overlay_toml, &predicted_socket)
            .await;
        if let Err(e) = start_outcome {
            self.release_reservation(&lock_path, &input.name);
            return Err(e);
        }

        // Child is live. Post-start errors propagate but do not roll
        // back the scope allocation — the child already owns it.
        send_run(&predicted_socket, &input.task).await?;

        let record = SpawnedPodRecord {
            pod_name: input.name.clone(),
            socket_path: predicted_socket.clone(),
            scope_delegated: scope_allow.clone(),
            callback_address: self.callback_socket.clone(),
        };
        self.registry
            .add(record)
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("write spawned_pods.json: {e}")))?;

        // Notify this Pod's own parent so the grandparent can register
        // the new grandchild directly. Fire-and-forget; top-level Pods
        // (with no parent) skip the send inside `fire_and_forget`.
        pod_events::fire_and_forget(
            self.parent_socket.clone(),
            PodEvent::ScopeSubDelegated {
                parent_pod: self.spawner_name.clone(),
                sub_pod: input.name.clone(),
                sub_socket: predicted_socket.clone(),
                scope: scope_allow,
            },
        );

        Ok(ToolOutput {
            summary: format!(
                "spawned pod `{}` listening on {}",
                input.name,
                predicted_socket.display()
            ),
            content: None,
        })
    }
}

impl SpawnPodTool {
    async fn exec_child(
        &self,
        pod_name: &str,
        overlay_toml: &str,
        predicted_socket: &Path,
    ) -> Result<(), ToolError> {
        let pod_command = std::env::var("INSOMNIA_POD_COMMAND").unwrap_or_else(|_| "pod".into());

        // Pre-create the child's runtime dir so we have a stable place to
        // capture its stderr before it has had a chance to bind anything.
        // The child's own `RuntimeDir::create` will `create_dir_all` the
        // same path again — that's idempotent. On clean exit the child's
        // RuntimeDir Drop tears the dir (and this log) down with it.
        let pod_runtime_dir = self.runtime_base.join(pod_name);
        tokio::fs::create_dir_all(&pod_runtime_dir)
            .await
            .map_err(|e| {
                ToolError::ExecutionFailed(format!(
                    "create runtime dir {}: {e}",
                    pod_runtime_dir.display()
                ))
            })?;
        let stderr_path = pod_runtime_dir.join("stderr.log");
        let stderr_file = std::fs::File::create(&stderr_path).map_err(|e| {
            ToolError::ExecutionFailed(format!("open {}: {e}", stderr_path.display()))
        })?;

        let mut cmd = Command::new(&pod_command);
        cmd.arg("--adopt")
            .arg("--callback")
            .arg(&self.callback_socket)
            .arg("--overlay")
            .arg(overlay_toml)
            .current_dir(&self.spawner_pwd)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::from(stderr_file))
            .process_group(0);

        let child = cmd
            .spawn()
            .map_err(|e| ToolError::ExecutionFailed(format!("failed to spawn `{pod_command}`: {e}")))?;

        // Default `kill_on_drop = false` keeps the process alive after
        // the `Child` is dropped. We intentionally do not `.wait()` —
        // when the spawner later exits, init adopts any remaining
        // orphans. Lifecycle tracking lives in `spawned_pods.json`.
        drop(child);

        match wait_for_socket(predicted_socket, SOCKET_WAIT_TIMEOUT).await {
            Ok(()) => Ok(()),
            Err(e) => Err(annotate_with_stderr(e, &stderr_path).await),
        }
    }

    fn release_reservation(&self, lock_path: &Path, pod_name: &str) {
        if let Ok(mut g) = LockFileGuard::open(lock_path) {
            let _ = scope_lock::release_pod(&mut g, pod_name);
        }
    }
}

fn parse_scope(rules: &[ScopeRuleInput]) -> Result<Vec<ScopeRule>, ToolError> {
    if rules.is_empty() {
        return Err(ToolError::InvalidArgument("scope must not be empty".into()));
    }
    rules
        .iter()
        .map(|r| {
            if !r.target.is_absolute() {
                return Err(ToolError::InvalidArgument(format!(
                    "scope.target must be absolute: {}",
                    r.target.display()
                )));
            }
            Ok(ScopeRule {
                target: r.target.clone(),
                permission: r.permission.into(),
                recursive: r.recursive,
            })
        })
        .collect()
}

/// Serialise the overlay TOML that gets handed to the child `pod`
/// binary via `--overlay`. `PodManifestConfig`'s `Serialize` impl is
/// the single source of truth for the on-disk manifest format.
///
/// The child's working directory is set separately via
/// `Command::current_dir` (see [`SpawnPodTool::exec_child`]) — it is
/// not part of the manifest.
fn build_overlay_toml(
    name: &str,
    instruction: &str,
    scope_allow: &[ScopeRule],
    model: &ModelConfig,
) -> Result<String, toml::ser::Error> {
    let overlay = PodManifestConfig {
        pod: PodMetaConfig {
            name: Some(name.to_string()),
        },
        model: ModelConfigPartial {
            scheme: Some(model.scheme),
            base_url: model.base_url.clone(),
            model_id: Some(model.model_id.clone()),
            auth: Some(model.auth.clone()),
        },
        worker: WorkerManifestConfig {
            instruction: Some(instruction.to_string()),
            ..Default::default()
        },
        scope: ScopeConfig {
            allow: scope_allow.to_vec(),
            deny: Vec::new(),
        },
        ..Default::default()
    };
    toml::to_string(&overlay)
}

/// Tail of the spawned child's `stderr.log` to splice into a startup
/// failure message. Capped so a chatty child can't blow up the LLM's
/// tool-result budget — debugging beyond this should read the file
/// directly.
const STDERR_TAIL_BYTES: usize = 4 * 1024;

async fn annotate_with_stderr(err: ToolError, stderr_path: &Path) -> ToolError {
    let tail = match tokio::fs::read(stderr_path).await {
        Ok(bytes) => {
            let start = bytes.len().saturating_sub(STDERR_TAIL_BYTES);
            String::from_utf8_lossy(&bytes[start..]).into_owned()
        }
        Err(_) => return err,
    };
    let trimmed = tail.trim();
    if trimmed.is_empty() {
        return err;
    }
    match err {
        ToolError::ExecutionFailed(msg) => ToolError::ExecutionFailed(format!(
            "{msg}\n--- child stderr ({}) ---\n{trimmed}",
            stderr_path.display()
        )),
        other => other,
    }
}

async fn wait_for_socket(path: &Path, timeout: Duration) -> Result<(), ToolError> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if path.exists() {
            if let Ok(stream) = UnixStream::connect(path).await {
                drop(stream);
                return Ok(());
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(ToolError::ExecutionFailed(format!(
                "spawned pod socket did not appear within {timeout:?}: {}",
                path.display()
            )));
        }
        sleep(Duration::from_millis(50)).await;
    }
}

async fn send_run(socket: &Path, task: &str) -> Result<(), ToolError> {
    let stream = UnixStream::connect(socket)
        .await
        .map_err(|e| ToolError::ExecutionFailed(format!("connect {}: {e}", socket.display())))?;
    let (_reader, writer) = stream.into_split();
    let mut w = JsonLineWriter::new(writer);
    w.write(&Method::Run {
        input: task.to_string(),
    })
    .await
    .map_err(|e| ToolError::ExecutionFailed(format!("send Method::Run: {e}")))?;
    // Drop the writer to close the socket's write half. The flush
    // inside `JsonLineWriter::write` has already pushed the bytes
    // across, so the child will see a complete method line followed by
    // EOF.
    drop(w);
    Ok(())
}

fn scope_lock_err_to_tool(e: ScopeLockError) -> ToolError {
    match e {
        ScopeLockError::NotSubset { .. }
        | ScopeLockError::WriteConflict { .. }
        | ScopeLockError::DuplicatePodName(_)
        | ScopeLockError::UnknownPod(_) => ToolError::InvalidArgument(e.to_string()),
        ScopeLockError::Io(_) => ToolError::ExecutionFailed(e.to_string()),
    }
}

/// Factory for the `SpawnPod` tool.
pub fn spawn_pod_tool(
    spawner_name: String,
    callback_socket: PathBuf,
    runtime_base: PathBuf,
    spawner_pwd: PathBuf,
    registry: Arc<SpawnedPodRegistry>,
    parent_socket: Option<PathBuf>,
    spawner_model: ModelConfig,
) -> ToolDefinition {
    Arc::new(move || {
        let schema = schemars::schema_for!(SpawnPodInput);
        let schema_value = serde_json::to_value(schema).unwrap_or(serde_json::json!({}));
        let meta = ToolMeta::new("SpawnPod")
            .description(DESCRIPTION)
            .input_schema(schema_value);
        let tool: Arc<dyn Tool> = Arc::new(SpawnPodTool::new(
            spawner_name.clone(),
            callback_socket.clone(),
            runtime_base.clone(),
            spawner_pwd.clone(),
            registry.clone(),
            parent_socket.clone(),
            spawner_model.clone(),
        ));
        (meta, tool)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use manifest::{AuthRef, SchemeKind};

    #[test]
    fn overlay_inherits_spawner_model() {
        let model = ModelConfig {
            scheme: SchemeKind::Anthropic,
            base_url: Some("https://example.test".into()),
            model_id: "claude-sonnet-4".into(),
            auth: AuthRef::ApiKey {
                env: None,
                file: Some(PathBuf::from("/etc/keys/anthropic")),
            },
        };

        let toml_str = build_overlay_toml("child", "$insomnia/default", &[], &model).unwrap();
        let parsed = PodManifestConfig::from_toml(&toml_str).unwrap();

        assert_eq!(parsed.model.scheme, Some(SchemeKind::Anthropic));
        assert_eq!(parsed.model.model_id.as_deref(), Some("claude-sonnet-4"));
        assert_eq!(parsed.model.base_url.as_deref(), Some("https://example.test"));
        let file = match parsed.model.auth {
            Some(AuthRef::ApiKey { file, .. }) => file,
            _ => panic!("expected ApiKey"),
        };
        assert_eq!(file.as_deref(), Some(Path::new("/etc/keys/anthropic")));
    }
}
