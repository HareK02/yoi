//! `SpawnWorker` tool — launch a new Worker process as a child of this one.
//!
//! Wires worker-allocation delegation, child manifest-config construction, subprocess
//! launch, and socket handoff into a single `Tool` implementation. When
//! the LLM calls `SpawnWorker`, a fresh Worker runtime command is exec'd in its own
//! process group, the worker-allocation is updated atomically, and the child's
//! first turn is kicked off by handing its socket a `Method::Run`.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use client::WorkerRuntimeCommand;
use llm_engine::tool::{Tool, ToolDefinition, ToolError, ToolMeta, ToolOutput};
use manifest::{
    CompactionConfigPartial, DelegationScope, EngineManifestConfig, FileUploadLimitsPartial,
    Permission, PermissionConfigPartial, ProfileDiscovery, ProfileError, ProfileRegistry,
    ProfileRegistrySource, ProfileResolveOptions, ProfileResolver, ProfileSelector, Scope,
    ScopeConfig, ScopeRule, SessionConfigPartial, SharedScope, ToolOutputLimitsPartial,
    WorkerManifest, WorkerManifestConfig, WorkerMetaConfig,
};
use serde::Deserialize;
use tokio::net::UnixStream;
use tokio::process::Command;
use tokio::time::sleep;

use crate::ipc::event;
use crate::prompt::catalog::PromptCatalog;
use crate::runtime::dir::SpawnedWorkerRecord;
use crate::runtime::worker_allocation::{self, LockFileGuard, ScopeLockError};
use crate::spawn::comm_tools::{SendRunError, send_run_and_confirm};
use crate::spawn::registry::SpawnedWorkerRegistry;
use protocol::WorkerEvent;

/// How long we will wait for the spawned Worker's socket to become
/// connectable before treating the spawn as failed.
const SOCKET_WAIT_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct SpawnWorkerInput {
    /// Identifier for the spawned Worker. Must be unique machine-wide.
    name: String,
    /// Profile selector for child role configuration. Omit or use `default`
    /// for the effective child default profile, use `inherit` to derive
    /// reusable config from the spawner, or use a registry selector such as
    /// `project:coder`, `project:reviewer`, `builtin:default`, or an
    /// unambiguous profile slug. Raw/path selectors are rejected.
    #[serde(default)]
    profile: Option<String>,
    /// Instruction-file reference (e.g. `$yoi/default`, `$user/my-agent`).
    #[serde(default)]
    instruction: Option<String>,
    /// Child process/tool working directory. This is not the runtime workspace
    /// root and grants no filesystem authority. When omitted, the spawned Worker
    /// starts in the spawner's current working directory.
    #[serde(default)]
    cwd: Option<PathBuf>,
    /// First message sent to the spawned Worker via `Method::Run`.
    task: String,
    /// Allow rules delegated to the spawned Worker. Must be a subset of the
    /// spawner's explicit delegation authority; direct tool scope alone is not
    /// sufficient. Omit `recursive` for normal workspace/worktree delegation; it defaults to true.
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

#[derive(Debug, Clone)]
struct AvailableProfiles {
    registry: Option<ProfileRegistry>,
    diagnostic: Option<String>,
}

impl AvailableProfiles {
    fn discover(cwd: &Path) -> Self {
        match ProfileDiscovery::for_cwd(cwd).discover() {
            Ok(registry) => Self {
                registry: Some(registry),
                diagnostic: None,
            },
            Err(error) => Self {
                registry: None,
                diagnostic: Some(error.to_string()),
            },
        }
    }

    fn compact_list(&self) -> String {
        let Some(registry) = &self.registry else {
            return "- profile discovery failed; use `inherit` or retry after fixing discovery"
                .into();
        };
        if registry.entries().is_empty() {
            return "- no registry profiles discovered; `inherit` is still available".into();
        }
        registry
            .entries()
            .iter()
            .map(|entry| {
                let default = if entry.is_default { " (default)" } else { "" };
                let desc = entry
                    .description
                    .as_deref()
                    .map(|d| format!(" — {d}"))
                    .unwrap_or_default();
                format!("- `{}`{}{}", entry.qualified_name(), default, desc)
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn default_label(&self) -> String {
        self.registry
            .as_ref()
            .and_then(|registry| registry.default_entry().ok())
            .map(|entry| entry.qualified_name())
            .unwrap_or_else(|| "none resolved".into())
    }

    fn diagnostic(&self) -> &str {
        self.diagnostic.as_deref().unwrap_or("")
    }

    fn error_suffix(&self) -> String {
        format!(
            "\nUse `default`, `inherit`, or one of these registry selectors:\n{}",
            self.compact_list()
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SpawnProfileSelector {
    Default,
    Inherit,
    Registry(ProfileSelector),
}

fn parse_spawn_profile_selector(raw: Option<&str>) -> Result<SpawnProfileSelector, String> {
    let Some(raw) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(SpawnProfileSelector::Default);
    };
    if raw == "default" {
        return Ok(SpawnProfileSelector::Default);
    }
    if raw == "inherit" {
        return Ok(SpawnProfileSelector::Inherit);
    }
    if raw.starts_with("path:")
        || raw.starts_with('/')
        || raw.starts_with("./")
        || raw.starts_with("../")
        || raw.contains('/')
        || raw.ends_with(".dcdl")
        || raw.ends_with(".json")
        || raw.ends_with(".toml")
        || raw.ends_with(".nix")
    {
        return Err(format!(
            "SpawnWorker.profile accepts `default`, `inherit`, or registry selectors only; path-like selector `{raw}` is not allowed"
        ));
    }
    if let Some((prefix, name)) = raw.split_once(':') {
        let source = match prefix {
            "builtin" => ProfileRegistrySource::Builtin,
            "user" => ProfileRegistrySource::User,
            "project" => ProfileRegistrySource::Project,
            _ => {
                return Err(format!(
                    "unsupported SpawnWorker.profile selector prefix `{prefix}`; use builtin:, user:, project:, default, or inherit"
                ));
            }
        };
        if name.is_empty() {
            return Err("SpawnWorker.profile registry selector has an empty profile name".into());
        }
        return Ok(SpawnProfileSelector::Registry(
            ProfileSelector::source_named(source, name),
        ));
    }
    Ok(SpawnProfileSelector::Registry(ProfileSelector::named(raw)))
}

/// Runtime dependencies the `SpawnWorker` tool needs in order to launch a
/// child Worker and record the handoff locally. Constructed by the Worker
/// controller once per Worker lifetime.
pub struct SpawnWorkerTool {
    /// Spawner's own worker name — becomes the spawned Worker's
    /// `delegated_from` in the worker-allocation.
    spawner_name: String,
    /// Path to the spawner's Unix socket. Handed to the child via
    /// `--callback` so its `WorkerEvent` callbacks have somewhere to land.
    callback_socket: PathBuf,
    /// Root of the `$XDG_RUNTIME_DIR/yoi/` tree, used to predict
    /// the spawned Worker's socket path before the child has bound it.
    runtime_base: PathBuf,
    /// Inherited runtime workspace root for Profile/project/Ticket/workflow/
    /// memory context. SpawnWorker `cwd` must not affect this value.
    workspace_root: PathBuf,
    /// Directory the spawned Worker's tools should use when the LLM did not
    /// override it. Defaults to the spawner's cwd.
    spawner_cwd: PathBuf,
    /// Optional typed runtime command injected by tests. Production resolves
    /// the runtime command from `std::env::current_exe()` at launch time.
    runtime_command: Option<WorkerRuntimeCommand>,
    /// Shared registry of spawned children, also used by the
    /// worker-comm tools (`SendToWorker` / `ReadWorkerOutput` / `StopWorker`) and by
    /// Worker discovery. Writes the list to runtime and durable Worker state on
    /// each add.
    registry: Arc<SpawnedWorkerRegistry>,
    /// THIS Worker's own parent-callback socket, if any. After a
    /// successful spawn we fire `WorkerEvent::ScopeSubDelegated` upward
    /// so the grandparent can register the grandchild directly.
    /// `None` for top-level Workers — in that case the re-emission is a
    /// no-op.
    parent_socket: Option<PathBuf>,
    /// Spawner's resolved Manifest. `profile = "inherit"` derives the
    /// child config from reusable fields here, and selected profiles are
    /// merged into the same internal handoff shape before launch.
    spawner_manifest: WorkerManifest,
    /// Compact selector list shared by tool description and diagnostics.
    available_profiles: AvailableProfiles,
    /// Spawner's runtime scope. After a successful spawn, the
    /// `Permission::Write` rules in the delegated scope are revoked
    /// from the spawner's in-memory view (a `deny(Write, target)` is
    /// pushed on top, downgrading the spawner's effective access on
    /// those paths to `Read`). Mirrors the worker-allocation's
    /// `effective_write` semantics: Write is the only permission
    /// tracked across Workers, so revocation only touches Write.
    spawner_scope: SharedScope,
    /// Filesystem scope this Worker is allowed to subdelegate to children.
    /// This is intentionally separate from `spawner_scope`, which authorizes
    /// the current Worker's own direct tools.
    delegation_scope: DelegationScope,
}

impl SpawnWorkerTool {
    fn new(
        spawner_name: String,
        callback_socket: PathBuf,
        runtime_base: PathBuf,
        workspace_root: PathBuf,
        spawner_cwd: PathBuf,
        registry: Arc<SpawnedWorkerRegistry>,
        parent_socket: Option<PathBuf>,
        spawner_manifest: WorkerManifest,
        available_profiles: AvailableProfiles,
        spawner_scope: SharedScope,
        delegation_scope: DelegationScope,
        runtime_command: Option<WorkerRuntimeCommand>,
    ) -> Self {
        Self {
            spawner_name,
            callback_socket,
            runtime_base,
            workspace_root,
            spawner_cwd,
            runtime_command,
            registry,
            parent_socket,
            spawner_manifest,
            available_profiles,
            spawner_scope,
            delegation_scope,
        }
    }
}

#[async_trait]
impl Tool for SpawnWorkerTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: llm_engine::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let input: SpawnWorkerInput = serde_json::from_str(input_json)
            .map_err(|e| ToolError::InvalidArgument(format!("invalid SpawnWorker input: {e}")))?;

        // `delegate_scope` catches this too (as `DuplicateWorkerName`), but
        // the dedicated message is kinder to the LLM — which gets the
        // error back verbatim — than the generic duplicate-name error.
        if input.name == self.spawner_name {
            return Err(ToolError::InvalidArgument(format!(
                "spawned worker name `{}` collides with spawner's own name",
                input.name
            )));
        }

        let scope_allow = parse_scope(&input.scope)?;
        self.validate_delegation_scope(&scope_allow)?;
        let child_cwd = validate_spawn_cwd(input.cwd.as_deref(), &scope_allow, &self.spawner_cwd)?;

        let spawn_selector =
            parse_spawn_profile_selector(input.profile.as_deref()).map_err(|msg| {
                ToolError::InvalidArgument(format!(
                    "{msg}{}",
                    self.available_profiles.error_suffix()
                ))
            })?;
        let spawn_config_json = self
            .build_spawn_config_json(
                &input.name,
                input.instruction.as_deref(),
                &scope_allow,
                spawn_selector,
            )
            .map_err(|e| ToolError::InvalidArgument(format!("{e}")))?;

        let predicted_socket = self.runtime_base.join(&input.name).join("sock");
        let lock_path = worker_allocation::default_allocation_path()
            .map_err(|e| ToolError::ExecutionFailed(format!("worker-allocation path: {e}")))?;

        // Reserve the allocation up front. Spawner's pid is a live
        // placeholder; the child will rewrite it via `adopt_allocation`.
        {
            let mut guard = LockFileGuard::open(&lock_path)
                .map_err(|e| ToolError::ExecutionFailed(format!("worker-allocation open: {e}")))?;
            worker_allocation::delegate_scope(
                &mut guard,
                &self.spawner_name,
                input.name.clone(),
                std::process::id(),
                predicted_socket.clone(),
                scope_allow.clone(),
                &self.delegation_scope,
            )
            .map_err(worker_allocation_err_to_tool)?;
        }

        // `start_outcome` covers steps that happen before the child is
        // observably alive (exec + socket bind). Once its socket is
        // listening, the child owns the allocation and we must not roll
        // it back — even if later steps (Method::Run delivery, record
        // write) fail, the child is running and will release its own
        // entry on exit.

        let start_outcome = self
            .exec_child(
                &input.name,
                &spawn_config_json,
                &predicted_socket,
                &child_cwd,
            )
            .await;
        if let Err(e) = start_outcome {
            self.release_reservation(&lock_path, &input.name);
            return Err(e);
        }

        // Child is live. Post-start errors propagate but do not roll
        // back the scope allocation — the child already owns it.
        //
        // Mirror that ownership transfer in the spawner's in-memory
        // scope: every `Permission::Write` rule in the delegated scope
        // is shadowed by a `deny(Write, target)` so subsequent tool
        // calls (Edit/Write) on the delegated paths fail with
        // `ReadOnly`. Read access is left intact — the registry only
        // arbitrates Write, and keeping Read lets the spawner observe
        // the child's intermediate output through Read/Glob/Grep.
        let revoke_write: Vec<ScopeRule> = scope_allow
            .iter()
            .filter(|r| r.permission == Permission::Write)
            .cloned()
            .collect();
        if !revoke_write.is_empty() {
            self.spawner_scope
                .update(|cur| cur.with_added_deny_rules(revoke_write.clone()))
                .map_err(|e| ToolError::ExecutionFailed(format!("revoke spawner scope: {e}")))?;
        }

        let record = SpawnedWorkerRecord {
            worker_name: input.name.clone(),
            socket_path: predicted_socket.clone(),
            scope_delegated: scope_allow.clone(),
            callback_address: self.callback_socket.clone(),
        };
        self.registry.add(record).await.map_err(|e| {
            ToolError::ExecutionFailed(format!("write spawned worker registry: {e}"))
        })?;

        // Notify this Worker's own parent so the grandparent can register
        // the new grandchild directly. Fire-and-forget; top-level Workers
        // (with no parent) skip the send inside `fire_and_forget`.
        event::fire_and_forget(
            self.parent_socket.clone(),
            WorkerEvent::ScopeSubDelegated {
                parent_worker: self.spawner_name.clone(),
                sub_worker: input.name.clone(),
                sub_socket: predicted_socket.clone(),
                scope: scope_allow,
            },
        );

        send_run_and_confirm(&predicted_socket, input.task.clone())
            .await
            .map_err(|err| spawn_delivery_error(&input.name, err))?;

        Ok(ToolOutput {
            summary: format!(
                "spawned worker `{}` listening on {}",
                input.name,
                predicted_socket.display()
            ),
            content: None,
        })
    }
}

impl SpawnWorkerTool {
    async fn exec_child(
        &self,
        worker_name: &str,
        spawn_config_json: &str,
        predicted_socket: &Path,
        child_cwd: &Path,
    ) -> Result<(), ToolError> {
        let runtime_command = match &self.runtime_command {
            Some(command) => command.clone(),
            None => WorkerRuntimeCommand::resolve().map_err(|error| {
                ToolError::ExecutionFailed(format!(
                    "failed to resolve Worker runtime command: {error}"
                ))
            })?,
        };

        // Pre-create the child's runtime dir so we have a stable place to
        // capture its stderr before it has had a chance to bind anything.
        // The child's own `RuntimeDir::create` will `create_dir_all` the
        // same path again — that's idempotent. On clean exit the child's
        // RuntimeDir Drop tears the dir (and this log) down with it.
        let worker_runtime_dir = self.runtime_base.join(worker_name);
        tokio::fs::create_dir_all(&worker_runtime_dir)
            .await
            .map_err(|e| {
                ToolError::ExecutionFailed(format!(
                    "create runtime dir {}: {e}",
                    worker_runtime_dir.display()
                ))
            })?;
        let stderr_path = worker_runtime_dir.join("stderr.log");
        let stderr_file = std::fs::File::create(&stderr_path).map_err(|e| {
            ToolError::ExecutionFailed(format!("open {}: {e}", stderr_path.display()))
        })?;

        let mut cmd = Command::new(runtime_command.program());
        cmd.args(runtime_command.prefix_args())
            .arg("--adopt")
            .arg("--callback")
            .arg(&self.callback_socket)
            .arg("--spawn-config-json")
            .arg(spawn_config_json)
            .arg("--workspace")
            .arg(&self.workspace_root)
            .current_dir(child_cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::from(stderr_file))
            .process_group(0);

        let child = cmd.spawn().map_err(|e| {
            ToolError::ExecutionFailed(format!("failed to spawn `{runtime_command}`: {e}"))
        })?;

        // Default `kill_on_drop = false` keeps the process alive after
        // the `Child` is dropped. We intentionally do not `.wait()` —
        // when the spawner later exits, init adopts any remaining
        // orphans. Lifecycle tracking lives in `spawned_workers.json`.
        drop(child);

        match wait_for_socket(predicted_socket, SOCKET_WAIT_TIMEOUT).await {
            Ok(()) => Ok(()),
            Err(e) => Err(annotate_with_stderr(e, &stderr_path).await),
        }
    }

    fn validate_delegation_scope(&self, scope_allow: &[ScopeRule]) -> Result<(), ToolError> {
        if self.delegation_scope.is_empty() && !scope_allow.is_empty() {
            return Err(ToolError::InvalidArgument(
                "SpawnWorker requires delegation authority, but this Worker has no delegation scope grant; direct filesystem scope only authorizes this Worker's own tools".into(),
            ));
        }
        for rule in scope_allow {
            let allowed = self
                .delegation_scope
                .allows_rule(rule)
                .map_err(|error| ToolError::InvalidArgument(error.to_string()))?;
            if !allowed {
                return Err(ToolError::InvalidArgument(format!(
                    "requested child scope {} {:?} is outside this Worker's delegation scope grant",
                    rule.target.display(),
                    rule.permission
                )));
            }
        }
        Ok(())
    }

    fn release_reservation(&self, lock_path: &Path, worker_name: &str) {
        if let Ok(mut g) = LockFileGuard::open(lock_path) {
            let _ = worker_allocation::release_worker(&mut g, worker_name);
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

fn validate_spawn_cwd(
    cwd: Option<&Path>,
    scope_allow: &[ScopeRule],
    default_cwd: &Path,
) -> Result<PathBuf, ToolError> {
    let Some(cwd) = cwd else {
        return Ok(default_cwd.to_path_buf());
    };
    if !cwd.is_absolute() {
        return Err(ToolError::InvalidArgument(format!(
            "SpawnWorker.cwd must be absolute: {}",
            cwd.display()
        )));
    }
    let metadata = std::fs::metadata(cwd).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            ToolError::InvalidArgument(format!("SpawnWorker.cwd does not exist: {}", cwd.display()))
        } else {
            ToolError::InvalidArgument(format!(
                "SpawnWorker.cwd is not usable: {}: {e}",
                cwd.display()
            ))
        }
    })?;
    if !metadata.is_dir() {
        return Err(ToolError::InvalidArgument(format!(
            "SpawnWorker.cwd must be a directory: {}",
            cwd.display()
        )));
    }
    let canonical = std::fs::canonicalize(cwd).map_err(|e| {
        ToolError::InvalidArgument(format!(
            "SpawnWorker.cwd is not usable: {}: {e}",
            cwd.display()
        ))
    })?;
    let child_scope = Scope::from_config(&ScopeConfig {
        allow: scope_allow.to_vec(),
        deny: Vec::new(),
    })
    .map_err(|e| {
        ToolError::InvalidArgument(format!(
            "requested child scope cannot validate SpawnWorker.cwd: {e}"
        ))
    })?;
    if !child_scope.is_readable(&canonical) {
        return Err(ToolError::InvalidArgument(format!(
            "SpawnWorker.cwd {} is outside the child's delegated readable scope; cwd grants no authority, so add an explicit read or write scope rule covering it",
            cwd.display()
        )));
    }
    Ok(canonical)
}

/// Serialise the internal manifest config that gets handed to the child
/// Worker runtime process via the hidden `--spawn-config-json` flag.
/// `WorkerManifestConfig`'s `Serialize` impl is the single source of truth for the
/// internal handoff shape.
///
/// The child's tool working directory is carried separately through
/// the child runtime entrypoint; it is not part of the manifest.
impl SpawnWorkerTool {
    fn build_spawn_config_json(
        &self,
        name: &str,
        instruction_override: Option<&str>,
        scope_allow: &[ScopeRule],
        selector: SpawnProfileSelector,
    ) -> Result<String, String> {
        build_spawn_config_json_for_profile(
            &self.spawner_manifest,
            &self.available_profiles,
            &self.workspace_root,
            name,
            instruction_override,
            scope_allow,
            selector,
        )
    }
}

fn build_spawn_config_json_for_profile(
    spawner_manifest: &WorkerManifest,
    available_profiles: &AvailableProfiles,
    workspace_root: &Path,
    name: &str,
    instruction_override: Option<&str>,
    scope_allow: &[ScopeRule],
    selector: SpawnProfileSelector,
) -> Result<String, String> {
    let mut config = match selector {
        SpawnProfileSelector::Inherit => manifest_to_reusable_config(spawner_manifest),
        SpawnProfileSelector::Default | SpawnProfileSelector::Registry(_) => {
            let registry = available_profiles.registry.as_ref().ok_or_else(|| {
                format!(
                    "profile discovery failed for SpawnWorker: {}{}",
                    available_profiles.diagnostic().if_empty("unknown error"),
                    available_profiles.error_suffix()
                )
            })?;
            let profile_selector = match selector {
                SpawnProfileSelector::Default => ProfileSelector::Default,
                SpawnProfileSelector::Registry(selector) => selector,
                SpawnProfileSelector::Inherit => unreachable!(),
            };
            let resolved = ProfileResolver::new()
                .with_workspace_base(workspace_root)
                .resolve_from_registry(
                    &profile_selector,
                    registry,
                    ProfileResolveOptions::with_worker_name(name),
                )
                .map_err(|e| profile_error_with_available(e, available_profiles))?;
            manifest_to_reusable_config(&resolved.manifest)
        }
    };
    config.worker.name = Some(name.to_string());
    config.scope = ScopeConfig {
        allow: scope_allow.to_vec(),
        deny: Vec::new(),
    };
    if let Some(instruction) = instruction_override {
        config.engine.instruction = Some(instruction.to_string());
    }
    serde_json::to_string(&config).map_err(|e| format!("spawn config serialisation: {e}"))
}

#[cfg(test)]
fn build_spawn_config_json(
    name: &str,
    instruction: &str,
    scope_allow: &[ScopeRule],
    model: &manifest::ModelManifest,
    record_event_trace: bool,
) -> Result<String, serde_json::Error> {
    let config = WorkerManifestConfig {
        worker: WorkerMetaConfig {
            name: Some(name.to_string()),
            prompt_pack: None,
        },
        model: model.clone(),
        engine: EngineManifestConfig {
            instruction: Some(instruction.to_string()),
            ..Default::default()
        },
        scope: ScopeConfig {
            allow: scope_allow.to_vec(),
            deny: Vec::new(),
        },
        session: record_event_trace.then_some(SessionConfigPartial {
            record_event_trace: Some(true),
        }),
        ..Default::default()
    };
    serde_json::to_string(&config)
}

trait IfEmpty {
    fn if_empty(&self, fallback: &str) -> String;
}
impl IfEmpty for str {
    fn if_empty(&self, fallback: &str) -> String {
        if self.is_empty() {
            fallback.into()
        } else {
            self.into()
        }
    }
}

fn profile_error_with_available(error: ProfileError, available: &AvailableProfiles) -> String {
    format!(
        "invalid SpawnWorker.profile: {error}{}",
        available.error_suffix()
    )
}

fn manifest_to_reusable_config(manifest: &WorkerManifest) -> WorkerManifestConfig {
    WorkerManifestConfig {
        worker: WorkerMetaConfig {
            name: Some(manifest.worker.name.clone()),
            prompt_pack: manifest.worker.prompt_pack.clone(),
        },
        model: manifest.model.clone(),
        engine: EngineManifestConfig {
            instruction: Some(manifest.engine.instruction.clone()),
            language: Some(manifest.engine.language.clone()),
            max_tokens: manifest.engine.max_tokens,
            max_turns: manifest.engine.max_turns,
            temperature: manifest.engine.temperature,
            top_p: manifest.engine.top_p,
            top_k: manifest.engine.top_k,
            stop_sequences: (!manifest.engine.stop_sequences.is_empty())
                .then_some(manifest.engine.stop_sequences.clone()),
            reasoning: manifest.engine.reasoning.clone(),
            tool_output: ToolOutputLimitsPartial {
                default_max_bytes: Some(manifest.engine.tool_output.default_max_bytes),
                per_tool: manifest.engine.tool_output.per_tool.clone(),
            },
            file_upload: FileUploadLimitsPartial {
                max_bytes: Some(manifest.engine.file_upload.max_bytes),
            },
        },
        scope: ScopeConfig {
            allow: manifest.scope.allow.clone(),
            deny: manifest.scope.deny.clone(),
        },
        // `inherit` reuses behavioral configuration, not subdelegation authority.
        delegation_scope: ScopeConfig::default(),
        session: Some(SessionConfigPartial {
            record_event_trace: Some(manifest.session.record_event_trace),
        }),
        permissions: manifest
            .permissions
            .as_ref()
            .map(|p| PermissionConfigPartial {
                default_action: Some(p.default_action),
                rules: p.rules.clone(),
            }),
        feature: manifest.feature.clone().into(),
        plugins: manifest.plugins.clone(),
        mcp: manifest.mcp.clone(),
        compaction: manifest
            .compaction
            .as_ref()
            .map(|c| CompactionConfigPartial {
                prune_protected_tokens: Some(c.prune_protected_tokens),
                prune_min_savings: Some(c.prune_min_savings),
                threshold: c.threshold,
                request_threshold: c.request_threshold,
                retained_tokens: Some(c.retained_tokens),
                overview_target_tokens: Some(c.overview_target_tokens),
                overview_warning_tokens: Some(c.overview_warning_tokens),
                overview_deadline_tokens: Some(c.overview_deadline_tokens),
                worker_context_max_tokens: Some(c.worker_context_max_tokens),
                finish_warning_remaining_tokens: Some(c.finish_warning_remaining_tokens),
                final_reserve_tokens: Some(c.final_reserve_tokens),
                worker_max_turns: c.worker_max_turns,
                summary_target_tokens: Some(c.summary_target_tokens),
                summary_max_tokens: Some(c.summary_max_tokens),
                auto_read_budget_tokens: Some(c.auto_read_budget_tokens),
                result_context_max_tokens: Some(c.result_context_max_tokens),
                model: c.model.clone(),
            }),
        web: manifest.web.clone(),
        memory: manifest.memory.clone(),
        skills: manifest.skills.clone(),
    }
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
                "spawned worker socket did not appear within {timeout:?}: {}",
                path.display()
            )));
        }
        sleep(Duration::from_millis(50)).await;
    }
}

fn spawn_delivery_error(worker_name: &str, err: SendRunError) -> ToolError {
    match err {
        SendRunError::AlreadyRunning => ToolError::ExecutionFailed(format!(
            "spawned worker `{worker_name}` rejected its initial task as already running; the worker remains registered and can be inspected or stopped"
        )),
        SendRunError::Rejected { code, message } => ToolError::ExecutionFailed(format!(
            "spawned worker `{worker_name}` rejected its initial task with {code:?}: {message}; the worker remains registered and can be inspected or stopped"
        )),
        SendRunError::Io(msg) => ToolError::ExecutionFailed(format!(
            "spawned worker `{worker_name}` did not confirm initial task delivery: {msg}; the worker remains registered and can be inspected or stopped"
        )),
    }
}

fn worker_allocation_err_to_tool(e: ScopeLockError) -> ToolError {
    match e {
        ScopeLockError::NotSubset { .. }
        | ScopeLockError::WriteConflict { .. }
        | ScopeLockError::DuplicateWorkerName(_)
        | ScopeLockError::UnknownWorker(_)
        | ScopeLockError::InvalidScope { .. }
        | ScopeLockError::SegmentConflict { .. } => ToolError::InvalidArgument(e.to_string()),
        ScopeLockError::Io(_) => ToolError::ExecutionFailed(e.to_string()),
    }
}

/// Factory for the `SpawnWorker` tool.
pub fn spawn_worker_tool(
    spawner_name: String,
    callback_socket: PathBuf,
    runtime_base: PathBuf,
    workspace_root: PathBuf,
    spawner_cwd: PathBuf,
    registry: Arc<SpawnedWorkerRegistry>,
    parent_socket: Option<PathBuf>,
    spawner_manifest: WorkerManifest,
    spawner_scope: SharedScope,
    prompts: Arc<PromptCatalog>,
) -> ToolDefinition {
    spawn_worker_tool_impl(
        spawner_name,
        callback_socket,
        runtime_base,
        workspace_root,
        spawner_cwd,
        registry,
        parent_socket,
        spawner_manifest,
        spawner_scope,
        prompts,
        None,
    )
}

#[doc(hidden)]
pub fn spawn_worker_tool_with_runtime_command(
    spawner_name: String,
    callback_socket: PathBuf,
    runtime_base: PathBuf,
    workspace_root: PathBuf,
    spawner_cwd: PathBuf,
    registry: Arc<SpawnedWorkerRegistry>,
    parent_socket: Option<PathBuf>,
    spawner_manifest: WorkerManifest,
    spawner_scope: SharedScope,
    prompts: Arc<PromptCatalog>,
    runtime_command: WorkerRuntimeCommand,
) -> ToolDefinition {
    spawn_worker_tool_impl(
        spawner_name,
        callback_socket,
        runtime_base,
        workspace_root,
        spawner_cwd,
        registry,
        parent_socket,
        spawner_manifest,
        spawner_scope,
        prompts,
        Some(runtime_command),
    )
}

fn spawn_worker_tool_impl(
    spawner_name: String,
    callback_socket: PathBuf,
    runtime_base: PathBuf,
    workspace_root: PathBuf,
    spawner_cwd: PathBuf,
    registry: Arc<SpawnedWorkerRegistry>,
    parent_socket: Option<PathBuf>,
    spawner_manifest: WorkerManifest,
    spawner_scope: SharedScope,
    prompts: Arc<PromptCatalog>,
    runtime_command: Option<WorkerRuntimeCommand>,
) -> ToolDefinition {
    Arc::new(move || {
        let schema = schemars::schema_for!(SpawnWorkerInput);
        let schema_value = serde_json::to_value(schema).unwrap_or(serde_json::json!({}));
        let available_profiles = AvailableProfiles::discover(&workspace_root);
        let description = prompts
            .spawn_worker_tool_description(
                &available_profiles.compact_list(),
                &available_profiles.default_label(),
                available_profiles.diagnostic(),
            )
            .unwrap_or_else(|e| {
                format!(
                    "Spawn a new Worker process to work on a delegated task. Profile description rendering failed: {e}. Available profiles:\n{}",
                    available_profiles.compact_list()
                )
            });
        let meta = ToolMeta::new("SpawnWorker")
            .description(description)
            .input_schema(schema_value);
        let tool: Arc<dyn Tool> = Arc::new(SpawnWorkerTool::new(
            spawner_name.clone(),
            callback_socket.clone(),
            runtime_base.clone(),
            workspace_root.clone(),
            spawner_cwd.clone(),
            registry.clone(),
            parent_socket.clone(),
            spawner_manifest.clone(),
            available_profiles,
            spawner_scope.clone(),
            DelegationScope::from_config(&spawner_manifest.delegation_scope)
                .expect("resolved Worker manifest has a valid delegation scope"),
            runtime_command.clone(),
        ));
        (meta, tool)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use manifest::{AuthRef, ModelManifest, SchemeKind, WorkerManifest};
    use tempfile::TempDir;

    fn abs_rule(path: &Path, permission: Permission) -> ScopeRule {
        ScopeRule {
            target: path.to_path_buf(),
            permission,
            recursive: true,
        }
    }

    #[test]
    fn spawn_worker_input_schema_includes_optional_cwd() {
        let schema = serde_json::to_value(schemars::schema_for!(SpawnWorkerInput)).unwrap();
        let properties = schema
            .get("properties")
            .and_then(serde_json::Value::as_object)
            .expect("schema properties");
        assert!(properties.contains_key("cwd"), "schema: {schema}");
        let required = schema
            .get("required")
            .and_then(serde_json::Value::as_array)
            .expect("schema required list");
        assert!(
            !required.iter().any(|value| value.as_str() == Some("cwd")),
            "cwd must remain optional: {schema}"
        );
    }

    #[test]
    fn spawn_worker_validate_cwd_requires_absolute_existing_directory_in_child_scope() {
        let root = TempDir::new().unwrap();
        let child_cwd = root.path().join("child");
        std::fs::create_dir(&child_cwd).unwrap();
        let file_path = root.path().join("file.txt");
        std::fs::write(&file_path, "not a dir").unwrap();
        let outside = TempDir::new().unwrap();
        let missing = root.path().join("missing");
        let rules = vec![abs_rule(root.path(), Permission::Write)];

        assert_eq!(
            validate_spawn_cwd(None, &rules, root.path()).unwrap(),
            root.path()
        );
        assert_eq!(
            validate_spawn_cwd(Some(&child_cwd), &rules, root.path()).unwrap(),
            std::fs::canonicalize(&child_cwd).unwrap()
        );

        for (cwd, expected) in [
            (Path::new("relative"), "must be absolute"),
            (missing.as_path(), "does not exist"),
            (file_path.as_path(), "must be a directory"),
            (
                outside.path(),
                "outside the child's delegated readable scope",
            ),
        ] {
            let err = validate_spawn_cwd(Some(cwd), &rules, root.path()).unwrap_err();
            match err {
                ToolError::InvalidArgument(message) => {
                    assert!(message.contains(expected), "{message}")
                }
                other => panic!("expected InvalidArgument, got {other:?}"),
            }
        }
    }

    #[test]
    fn orchestration_delegation_allows_root_read_and_worktree_writes_not_root_writes() {
        let tmp = TempDir::new().unwrap();
        let workspace_root = tmp.path().join("original");
        let implementation_worktree = workspace_root.join(".worktree/ticket-1");
        std::fs::create_dir_all(&implementation_worktree).unwrap();
        let delegation = DelegationScope::from_config(&ScopeConfig {
            allow: vec![
                abs_rule(&workspace_root, Permission::Read),
                abs_rule(&workspace_root.join(".worktree"), Permission::Write),
            ],
            deny: Vec::new(),
        })
        .unwrap();

        let coder_scope = vec![
            abs_rule(&workspace_root, Permission::Read),
            abs_rule(&implementation_worktree, Permission::Write),
        ];
        assert!(
            coder_scope
                .iter()
                .all(|rule| delegation.allows_rule(rule).unwrap())
        );

        let reviewer_scope = vec![abs_rule(&workspace_root, Permission::Read)];
        assert!(
            reviewer_scope
                .iter()
                .all(|rule| delegation.allows_rule(rule).unwrap())
        );

        let root_writer_scope = vec![abs_rule(&workspace_root, Permission::Write)];
        assert!(
            root_writer_scope
                .iter()
                .any(|rule| !delegation.allows_rule(rule).unwrap())
        );
    }

    fn parent_manifest(root: &Path, deny: Option<&Path>) -> WorkerManifest {
        WorkerManifestConfig {
            worker: WorkerMetaConfig {
                name: Some("parent".into()),
                prompt_pack: None,
            },
            model: ModelManifest {
                scheme: Some(SchemeKind::Anthropic),
                model_id: Some("parent-model".into()),
                auth: Some(AuthRef::None),
                ..Default::default()
            },
            engine: EngineManifestConfig {
                instruction: Some("$yoi/parent".into()),
                language: Some("Parentish".into()),
                max_tokens: Some(1234),
                stop_sequences: Some(vec!["STOP".into()]),
                ..Default::default()
            },
            scope: ScopeConfig {
                allow: vec![abs_rule(root, Permission::Write)],
                deny: deny
                    .map(|path| vec![abs_rule(path, Permission::Read)])
                    .unwrap_or_default(),
            },
            session: Some(SessionConfigPartial {
                record_event_trace: Some(true),
            }),
            ..Default::default()
        }
        .try_into()
        .unwrap()
    }

    fn write_project_profile_registry(
        project: &Path,
        default: Option<&str>,
        profiles: &[(&str, &str, &str)],
    ) -> AvailableProfiles {
        let yoi = project.join(".yoi");
        let profile_dir = yoi.join("profiles");
        std::fs::create_dir_all(&profile_dir).unwrap();
        let mut registry_toml = String::new();
        if let Some(default) = default {
            registry_toml.push_str(&format!("default = \"{default}\"\n"));
        }
        registry_toml.push_str("[profile]\n");
        for (name, file, body) in profiles {
            std::fs::write(profile_dir.join(file), body).unwrap();
            registry_toml.push_str(&format!("{name} = \"profiles/{file}\"\n"));
        }
        let registry_path = yoi.join("profiles.toml");
        std::fs::write(&registry_path, registry_toml).unwrap();
        AvailableProfiles {
            registry: Some(
                ProfileDiscovery::with_sources(None, Some(registry_path))
                    .discover()
                    .unwrap(),
            ),
            diagnostic: None,
        }
    }

    fn child_config_from_profile(
        spawner_manifest: &WorkerManifest,
        available: &AvailableProfiles,
        cwd: &Path,
        name: &str,
        instruction_override: Option<&str>,
        scope: &[ScopeRule],
        selector: SpawnProfileSelector,
    ) -> WorkerManifestConfig {
        let json = build_spawn_config_json_for_profile(
            spawner_manifest,
            available,
            cwd,
            name,
            instruction_override,
            scope,
            selector,
        )
        .unwrap();
        serde_json::from_str(&json).unwrap()
    }

    const CODER_PROFILE: &str = r#"
slug = "coder"
scope = "workspace_write"

[model]
scheme = "anthropic"
model_id = "coder-model"

[engine]
instruction = "$yoi/coder"
language = "Coderish"
max_tokens = 2222
"#;

    const REVIEWER_PROFILE: &str = r#"
slug = "reviewer"
scope = "workspace_write"

[model]
scheme = "anthropic"
model_id = "reviewer-model"

[engine]
instruction = "$yoi/reviewer"
language = "Reviewerish"
max_tokens = 3333
"#;

    #[test]
    fn spawn_config_inherits_inline_spawner_model() {
        let model = ModelManifest {
            scheme: Some(SchemeKind::Anthropic),
            base_url: Some("https://example.test".into()),
            model_id: Some("claude-sonnet-4".into()),
            auth: Some(AuthRef::ApiKey {
                file: Some(PathBuf::from("/etc/keys/anthropic")),
            }),
            ..Default::default()
        };

        let config_json =
            build_spawn_config_json("child", "$yoi/default", &[], &model, false).unwrap();
        let parsed: WorkerManifestConfig = serde_json::from_str(&config_json).unwrap();

        assert_eq!(parsed.model.scheme, Some(SchemeKind::Anthropic));
        assert_eq!(parsed.model.model_id.as_deref(), Some("claude-sonnet-4"));
        assert_eq!(
            parsed.model.base_url.as_deref(),
            Some("https://example.test")
        );
        let file = match parsed.model.auth {
            Some(AuthRef::ApiKey { file, .. }) => file,
            _ => panic!("expected ApiKey"),
        };
        assert_eq!(file.as_deref(), Some(Path::new("/etc/keys/anthropic")));
    }

    #[test]
    fn spawn_config_inherits_ref_spawner_model() {
        let model = ModelManifest {
            ref_: Some("anthropic/claude-sonnet-4-6".into()),
            ..Default::default()
        };
        let config_json =
            build_spawn_config_json("child", "$yoi/default", &[], &model, false).unwrap();
        let parsed: WorkerManifestConfig = serde_json::from_str(&config_json).unwrap();
        assert_eq!(
            parsed.model.ref_.as_deref(),
            Some("anthropic/claude-sonnet-4-6")
        );
    }

    #[test]
    fn spawn_config_preserves_record_event_trace_when_enabled() {
        let model = ModelManifest {
            ref_: Some("anthropic/claude-sonnet-4-6".into()),
            ..Default::default()
        };
        let scope = vec![ScopeRule {
            target: PathBuf::from("/tmp/child"),
            permission: Permission::Read,
            recursive: true,
        }];

        let config_json =
            build_spawn_config_json("child", "$yoi/default", &scope, &model, true).unwrap();
        let parsed: WorkerManifestConfig = serde_json::from_str(&config_json).unwrap();
        assert_eq!(
            parsed.session.as_ref().and_then(|s| s.record_event_trace),
            Some(true)
        );

        let manifest: WorkerManifest = WorkerManifestConfig::builtin_defaults()
            .merge(parsed)
            .try_into()
            .unwrap();
        assert!(manifest.session.record_event_trace);
    }

    #[test]
    fn spawn_config_omits_record_event_trace_when_disabled() {
        let model = ModelManifest {
            ref_: Some("anthropic/claude-sonnet-4-6".into()),
            ..Default::default()
        };
        let config_json =
            build_spawn_config_json("child", "$yoi/default", &[], &model, false).unwrap();
        let parsed: WorkerManifestConfig = serde_json::from_str(&config_json).unwrap();

        assert!(parsed.session.is_none());
    }

    #[test]
    fn omitted_profile_resolves_effective_registry_default() {
        let tmp = TempDir::new().unwrap();
        let project = tmp.path().join("project");
        let delegated = tmp.path().join("delegated");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(&delegated).unwrap();
        let available = write_project_profile_registry(
            &project,
            Some("reviewer"),
            &[
                ("coder", "coder.toml", CODER_PROFILE),
                ("reviewer", "reviewer.toml", REVIEWER_PROFILE),
            ],
        );
        let parent = parent_manifest(&project, None);
        let scope = vec![abs_rule(&delegated, Permission::Read)];

        let config = child_config_from_profile(
            &parent,
            &available,
            &project,
            "child-default",
            None,
            &scope,
            SpawnProfileSelector::Default,
        );

        assert_eq!(config.worker.name.as_deref(), Some("child-default"));
        assert_eq!(config.model.model_id.as_deref(), Some("reviewer-model"));
        assert_eq!(config.engine.instruction.as_deref(), Some("$yoi/reviewer"));
        assert_eq!(config.engine.language.as_deref(), Some("Reviewerish"));
        assert_eq!(config.scope.allow, scope);
        assert!(config.scope.deny.is_empty());
    }

    #[test]
    fn source_qualified_profile_role_config_reaches_spawn_config() {
        let tmp = TempDir::new().unwrap();
        let project = tmp.path().join("project");
        let delegated = tmp.path().join("delegated");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(&delegated).unwrap();
        let available = write_project_profile_registry(
            &project,
            Some("coder"),
            &[
                ("coder", "coder.toml", CODER_PROFILE),
                ("reviewer", "reviewer.toml", REVIEWER_PROFILE),
            ],
        );
        let parent = parent_manifest(&project, None);
        let scope = vec![abs_rule(&delegated, Permission::Write)];

        let config = child_config_from_profile(
            &parent,
            &available,
            &project,
            "review-child",
            None,
            &scope,
            SpawnProfileSelector::Registry(ProfileSelector::source_named(
                ProfileRegistrySource::Project,
                "reviewer",
            )),
        );

        assert_eq!(config.worker.name.as_deref(), Some("review-child"));
        assert_eq!(config.model.model_id.as_deref(), Some("reviewer-model"));
        assert_eq!(config.engine.instruction.as_deref(), Some("$yoi/reviewer"));
        assert_eq!(config.engine.language.as_deref(), Some("Reviewerish"));
        assert_eq!(config.engine.max_tokens, Some(3333));
        assert_eq!(config.scope.allow, scope);
        assert!(config.scope.deny.is_empty());
    }

    #[test]
    fn inherit_copies_reusable_parent_fields_and_replaces_runtime_authority() {
        let tmp = TempDir::new().unwrap();
        let parent_root = tmp.path().join("parent-root");
        let parent_deny = parent_root.join("secret");
        let delegated = tmp.path().join("delegated");
        std::fs::create_dir_all(&parent_deny).unwrap();
        std::fs::create_dir_all(&delegated).unwrap();
        let parent = parent_manifest(&parent_root, Some(&parent_deny));
        let scope = vec![abs_rule(&delegated, Permission::Read)];
        let available = AvailableProfiles {
            registry: None,
            diagnostic: None,
        };

        let config = child_config_from_profile(
            &parent,
            &available,
            tmp.path(),
            "inherited-child",
            None,
            &scope,
            SpawnProfileSelector::Inherit,
        );

        assert_eq!(config.worker.name.as_deref(), Some("inherited-child"));
        assert_eq!(config.model.model_id.as_deref(), Some("parent-model"));
        assert_eq!(config.engine.instruction.as_deref(), Some("$yoi/parent"));
        assert_eq!(config.engine.language.as_deref(), Some("Parentish"));
        assert_eq!(config.engine.max_tokens, Some(1234));
        assert_eq!(
            config.engine.stop_sequences.as_deref(),
            Some(&["STOP".to_string()][..])
        );
        assert_eq!(
            config.session.as_ref().and_then(|s| s.record_event_trace),
            Some(true)
        );
        assert_eq!(config.scope.allow, scope);
        assert!(config.scope.deny.is_empty());
    }

    #[test]
    fn instruction_override_changes_only_worker_instruction() {
        let tmp = TempDir::new().unwrap();
        let project = tmp.path().join("project");
        let delegated = tmp.path().join("delegated");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(&delegated).unwrap();
        let available = write_project_profile_registry(
            &project,
            Some("reviewer"),
            &[("reviewer", "reviewer.toml", REVIEWER_PROFILE)],
        );
        let parent = parent_manifest(&project, None);
        let scope = vec![abs_rule(&delegated, Permission::Write)];

        let config = child_config_from_profile(
            &parent,
            &available,
            &project,
            "override-child",
            Some("$user/custom-reviewer"),
            &scope,
            SpawnProfileSelector::Default,
        );

        assert_eq!(
            config.engine.instruction.as_deref(),
            Some("$user/custom-reviewer")
        );
        assert_eq!(config.model.model_id.as_deref(), Some("reviewer-model"));
        assert_eq!(config.engine.language.as_deref(), Some("Reviewerish"));
        assert_eq!(config.engine.max_tokens, Some(3333));
        assert_eq!(config.scope.allow, scope);
    }

    #[test]
    fn profile_and_inherited_scope_are_replaced_by_delegated_scope() {
        let tmp = TempDir::new().unwrap();
        let project = tmp.path().join("project");
        let delegated = tmp.path().join("delegated");
        let parent_root = tmp.path().join("parent-root");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(&delegated).unwrap();
        std::fs::create_dir_all(&parent_root).unwrap();
        let available = write_project_profile_registry(
            &project,
            Some("reviewer"),
            &[("reviewer", "reviewer.toml", REVIEWER_PROFILE)],
        );
        let parent = parent_manifest(&parent_root, Some(&parent_root.join("deny")));
        let scope = vec![abs_rule(&delegated, Permission::Read)];

        let profile_config = child_config_from_profile(
            &parent,
            &available,
            &project,
            "profile-child",
            None,
            &scope,
            SpawnProfileSelector::Default,
        );
        let inherit_config = child_config_from_profile(
            &parent,
            &available,
            &project,
            "inherit-child",
            None,
            &scope,
            SpawnProfileSelector::Inherit,
        );

        for config in [profile_config, inherit_config] {
            assert_eq!(config.scope.allow, scope);
            assert!(config.scope.deny.is_empty());
            assert!(!config.scope.allow.iter().any(|rule| rule.target == project));
            assert!(
                !config
                    .scope
                    .allow
                    .iter()
                    .any(|rule| rule.target == parent_root)
            );
        }
    }

    #[test]
    fn invalid_ambiguous_and_no_default_diagnostics_include_available_selectors() {
        let tmp = TempDir::new().unwrap();
        let project = tmp.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        let available = write_project_profile_registry(
            &project,
            None,
            &[("coder", "coder.toml", CODER_PROFILE)],
        );
        let parent = parent_manifest(&project, None);
        let scope = vec![abs_rule(&project, Permission::Read)];

        let invalid = parse_spawn_profile_selector(Some("./reviewer.toml"))
            .map_err(|msg| format!("{msg}{}", available.error_suffix()))
            .unwrap_err();
        assert!(invalid.contains("Use `default`, `inherit`"));
        assert!(invalid.contains("`project:coder`"));

        let default_config = build_spawn_config_json_for_profile(
            &parent,
            &available,
            &project,
            "child",
            None,
            &scope,
            SpawnProfileSelector::Default,
        )
        .unwrap();
        assert!(default_config.contains("\"name\":\"child\""));

        let user_config = tmp.path().join("user-profiles.toml");
        std::fs::write(&user_config, "[profile]\ncoder = \"user-coder.toml\"\n").unwrap();
        let project_config = project.join(".yoi/profiles.toml");
        let ambiguous = AvailableProfiles {
            registry: Some(
                ProfileDiscovery::with_sources(Some(user_config), Some(project_config))
                    .discover()
                    .unwrap(),
            ),
            diagnostic: None,
        };
        let ambiguous_error = build_spawn_config_json_for_profile(
            &parent,
            &ambiguous,
            &project,
            "child",
            None,
            &scope,
            SpawnProfileSelector::Registry(ProfileSelector::named("coder")),
        )
        .unwrap_err();
        assert!(ambiguous_error.contains("ambiguous"), "{ambiguous_error}");
        assert!(ambiguous_error.contains("user:coder"));
        assert!(ambiguous_error.contains("project:coder"));
        assert!(ambiguous_error.contains("Use `default`, `inherit`"));
    }

    #[test]
    fn spawn_profile_selector_rejects_path_like_values() {
        for raw in [
            "./reviewer.toml",
            "path:./reviewer.toml",
            "/tmp/reviewer.toml",
            "legacy.nix",
        ] {
            let err = parse_spawn_profile_selector(Some(raw)).unwrap_err();
            assert!(err.contains("registry selectors only"), "{raw}: {err}");
        }
    }

    #[test]
    fn spawn_profile_selector_accepts_default_inherit_and_registry() {
        assert_eq!(
            parse_spawn_profile_selector(None).unwrap(),
            SpawnProfileSelector::Default
        );
        assert_eq!(
            parse_spawn_profile_selector(Some("inherit")).unwrap(),
            SpawnProfileSelector::Inherit
        );
        assert_eq!(
            parse_spawn_profile_selector(Some("project:reviewer")).unwrap(),
            SpawnProfileSelector::Registry(ProfileSelector::source_named(
                ProfileRegistrySource::Project,
                "reviewer"
            ))
        );
        assert_eq!(
            parse_spawn_profile_selector(Some("coder")).unwrap(),
            SpawnProfileSelector::Registry(ProfileSelector::named("coder"))
        );
    }
}
