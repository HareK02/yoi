//! `SpawnPod` tool — launch a new Pod process as a child of this one.
//!
//! Wires pod-registry delegation, child manifest-config construction, subprocess
//! launch, and socket handoff into a single `Tool` implementation. When
//! the LLM calls `SpawnPod`, a fresh Pod runtime command is exec'd in its own
//! process group, the pod-registry is updated atomically, and the child's
//! first turn is kicked off by handing its socket a `Method::Run`.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use client::PodRuntimeCommand;
use llm_worker::tool::{Tool, ToolDefinition, ToolError, ToolMeta, ToolOutput};
use manifest::{
    CompactionConfigPartial, DelegationScope, FileUploadLimitsPartial, Permission,
    PermissionConfigPartial, PodManifest, PodManifestConfig, PodMetaConfig, ProfileDiscovery,
    ProfileError, ProfileRegistry, ProfileRegistrySource, ProfileResolveOptions, ProfileResolver,
    ProfileSelector, Scope, ScopeConfig, ScopeRule, SessionConfigPartial, SharedScope,
    ToolOutputLimitsPartial, WorkerManifestConfig,
};
use serde::Deserialize;
use tokio::net::UnixStream;
use tokio::process::Command;
use tokio::time::sleep;

use crate::ipc::event;
use crate::prompt::catalog::PromptCatalog;
use crate::runtime::dir::SpawnedPodRecord;
use crate::runtime::pod_registry::{self, LockFileGuard, ScopeLockError};
use crate::spawn::comm_tools::{SendRunError, send_run_and_confirm};
use crate::spawn::registry::SpawnedPodRegistry;
use protocol::PodEvent;

/// How long we will wait for the spawned Pod's socket to become
/// connectable before treating the spawn as failed.
const SOCKET_WAIT_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct SpawnPodInput {
    /// Identifier for the spawned Pod. Must be unique machine-wide.
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
    /// root and grants no filesystem authority. When omitted, the spawned Pod
    /// starts in the spawner's current working directory.
    #[serde(default)]
    cwd: Option<PathBuf>,
    /// First message sent to the spawned Pod via `Method::Run`.
    task: String,
    /// Allow rules delegated to the spawned Pod. Must be a subset of the
    /// spawner's explicit delegation authority; direct tool scope alone is not
    /// sufficient.
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
        || raw.ends_with(".lua")
        || raw.ends_with(".nix")
    {
        return Err(format!(
            "SpawnPod.profile accepts `default`, `inherit`, or registry selectors only; path-like selector `{raw}` is not allowed"
        ));
    }
    if let Some((prefix, name)) = raw.split_once(':') {
        let source = match prefix {
            "builtin" => ProfileRegistrySource::Builtin,
            "user" => ProfileRegistrySource::User,
            "project" => ProfileRegistrySource::Project,
            _ => {
                return Err(format!(
                    "unsupported SpawnPod.profile selector prefix `{prefix}`; use builtin:, user:, project:, default, or inherit"
                ));
            }
        };
        if name.is_empty() {
            return Err("SpawnPod.profile registry selector has an empty profile name".into());
        }
        return Ok(SpawnProfileSelector::Registry(
            ProfileSelector::source_named(source, name),
        ));
    }
    Ok(SpawnProfileSelector::Registry(ProfileSelector::named(raw)))
}

/// Runtime dependencies the `SpawnPod` tool needs in order to launch a
/// child Pod and record the handoff locally. Constructed by the Pod
/// controller once per Pod lifetime.
pub struct SpawnPodTool {
    /// Spawner's own pod name — becomes the spawned Pod's
    /// `delegated_from` in the pod-registry.
    spawner_name: String,
    /// Path to the spawner's Unix socket. Handed to the child via
    /// `--callback` so its `PodEvent` callbacks have somewhere to land.
    callback_socket: PathBuf,
    /// Root of the `$XDG_RUNTIME_DIR/yoi/` tree, used to predict
    /// the spawned Pod's socket path before the child has bound it.
    runtime_base: PathBuf,
    /// Inherited runtime workspace root for Profile/project/Ticket/workflow/
    /// memory context. SpawnPod `cwd` must not affect this value.
    workspace_root: PathBuf,
    /// Directory the spawned Pod's tools should use when the LLM did not
    /// override it. Defaults to the spawner's cwd.
    spawner_cwd: PathBuf,
    /// Optional typed runtime command injected by tests. Production resolves
    /// the runtime command from `std::env::current_exe()` at launch time.
    runtime_command: Option<PodRuntimeCommand>,
    /// Shared registry of spawned children, also used by the
    /// pod-comm tools (`SendToPod` / `ReadPodOutput` / `StopPod`) and by
    /// Pod discovery. Writes the list to runtime and durable Pod state on
    /// each add.
    registry: Arc<SpawnedPodRegistry>,
    /// THIS Pod's own parent-callback socket, if any. After a
    /// successful spawn we fire `PodEvent::ScopeSubDelegated` upward
    /// so the grandparent can register the grandchild directly.
    /// `None` for top-level Pods — in that case the re-emission is a
    /// no-op.
    parent_socket: Option<PathBuf>,
    /// Spawner's resolved Manifest. `profile = "inherit"` derives the
    /// child config from reusable fields here, and selected profiles are
    /// merged into the same internal handoff shape before launch.
    spawner_manifest: PodManifest,
    /// Compact selector list shared by tool description and diagnostics.
    available_profiles: AvailableProfiles,
    /// Spawner's runtime scope. After a successful spawn, the
    /// `Permission::Write` rules in the delegated scope are revoked
    /// from the spawner's in-memory view (a `deny(Write, target)` is
    /// pushed on top, downgrading the spawner's effective access on
    /// those paths to `Read`). Mirrors the pod-registry's
    /// `effective_write` semantics: Write is the only permission
    /// tracked across Pods, so revocation only touches Write.
    spawner_scope: SharedScope,
    /// Filesystem scope this Pod is allowed to subdelegate to children.
    /// This is intentionally separate from `spawner_scope`, which authorizes
    /// the current Pod's own direct tools.
    delegation_scope: DelegationScope,
}

impl SpawnPodTool {
    fn new(
        spawner_name: String,
        callback_socket: PathBuf,
        runtime_base: PathBuf,
        workspace_root: PathBuf,
        spawner_cwd: PathBuf,
        registry: Arc<SpawnedPodRegistry>,
        parent_socket: Option<PathBuf>,
        spawner_manifest: PodManifest,
        available_profiles: AvailableProfiles,
        spawner_scope: SharedScope,
        delegation_scope: DelegationScope,
        runtime_command: Option<PodRuntimeCommand>,
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
impl Tool for SpawnPodTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: llm_worker::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
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
        let lock_path = pod_registry::default_registry_path()
            .map_err(|e| ToolError::ExecutionFailed(format!("pod-registry path: {e}")))?;

        // Reserve the allocation up front. Spawner's pid is a live
        // placeholder; the child will rewrite it via `adopt_allocation`.
        {
            let mut guard = LockFileGuard::open(&lock_path)
                .map_err(|e| ToolError::ExecutionFailed(format!("pod-registry open: {e}")))?;
            pod_registry::delegate_scope(
                &mut guard,
                &self.spawner_name,
                input.name.clone(),
                std::process::id(),
                predicted_socket.clone(),
                scope_allow.clone(),
            )
            .map_err(pod_registry_err_to_tool)?;
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

        let record = SpawnedPodRecord {
            pod_name: input.name.clone(),
            socket_path: predicted_socket.clone(),
            scope_delegated: scope_allow.clone(),
            callback_address: self.callback_socket.clone(),
        };
        self.registry
            .add(record)
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("write spawned pod registry: {e}")))?;

        // Notify this Pod's own parent so the grandparent can register
        // the new grandchild directly. Fire-and-forget; top-level Pods
        // (with no parent) skip the send inside `fire_and_forget`.
        event::fire_and_forget(
            self.parent_socket.clone(),
            PodEvent::ScopeSubDelegated {
                parent_pod: self.spawner_name.clone(),
                sub_pod: input.name.clone(),
                sub_socket: predicted_socket.clone(),
                scope: scope_allow,
            },
        );

        send_run_and_confirm(&predicted_socket, input.task.clone())
            .await
            .map_err(|err| spawn_delivery_error(&input.name, err))?;

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
        spawn_config_json: &str,
        predicted_socket: &Path,
        child_cwd: &Path,
    ) -> Result<(), ToolError> {
        let runtime_command = match &self.runtime_command {
            Some(command) => command.clone(),
            None => PodRuntimeCommand::resolve().map_err(|error| {
                ToolError::ExecutionFailed(format!(
                    "failed to resolve Pod runtime command: {error}"
                ))
            })?,
        };

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
        // orphans. Lifecycle tracking lives in `spawned_pods.json`.
        drop(child);

        match wait_for_socket(predicted_socket, SOCKET_WAIT_TIMEOUT).await {
            Ok(()) => Ok(()),
            Err(e) => Err(annotate_with_stderr(e, &stderr_path).await),
        }
    }

    fn validate_delegation_scope(&self, scope_allow: &[ScopeRule]) -> Result<(), ToolError> {
        if self.delegation_scope.is_empty() && !scope_allow.is_empty() {
            return Err(ToolError::InvalidArgument(
                "SpawnPod requires delegation authority, but this Pod has no delegation scope grant; direct filesystem scope only authorizes this Pod's own tools".into(),
            ));
        }
        for rule in scope_allow {
            let allowed = self
                .delegation_scope
                .allows_rule(rule)
                .map_err(|error| ToolError::InvalidArgument(error.to_string()))?;
            if !allowed {
                return Err(ToolError::InvalidArgument(format!(
                    "requested child scope {} {:?} is outside this Pod's delegation scope grant",
                    rule.target.display(),
                    rule.permission
                )));
            }
        }
        Ok(())
    }

    fn release_reservation(&self, lock_path: &Path, pod_name: &str) {
        if let Ok(mut g) = LockFileGuard::open(lock_path) {
            let _ = pod_registry::release_pod(&mut g, pod_name);
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
            "SpawnPod.cwd must be absolute: {}",
            cwd.display()
        )));
    }
    let metadata = std::fs::metadata(cwd).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            ToolError::InvalidArgument(format!("SpawnPod.cwd does not exist: {}", cwd.display()))
        } else {
            ToolError::InvalidArgument(format!(
                "SpawnPod.cwd is not usable: {}: {e}",
                cwd.display()
            ))
        }
    })?;
    if !metadata.is_dir() {
        return Err(ToolError::InvalidArgument(format!(
            "SpawnPod.cwd must be a directory: {}",
            cwd.display()
        )));
    }
    let canonical = std::fs::canonicalize(cwd).map_err(|e| {
        ToolError::InvalidArgument(format!(
            "SpawnPod.cwd is not usable: {}: {e}",
            cwd.display()
        ))
    })?;
    let child_scope = Scope::from_config(&ScopeConfig {
        allow: scope_allow.to_vec(),
        deny: Vec::new(),
    })
    .map_err(|e| {
        ToolError::InvalidArgument(format!(
            "requested child scope cannot validate SpawnPod.cwd: {e}"
        ))
    })?;
    if !child_scope.is_readable(&canonical) {
        return Err(ToolError::InvalidArgument(format!(
            "SpawnPod.cwd {} is outside the child's delegated readable scope; cwd grants no authority, so add an explicit read or write scope rule covering it",
            cwd.display()
        )));
    }
    Ok(canonical)
}

/// Serialise the internal manifest config that gets handed to the child
/// Pod runtime process via the hidden `--spawn-config-json` flag.
/// `PodManifestConfig`'s `Serialize` impl is the single source of truth for the
/// internal handoff shape.
///
/// The child's tool working directory is carried separately through
/// the child runtime entrypoint; it is not part of the manifest.
impl SpawnPodTool {
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
    spawner_manifest: &PodManifest,
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
                    "profile discovery failed for SpawnPod: {}{}",
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
                    ProfileResolveOptions::with_pod_name(name),
                )
                .map_err(|e| profile_error_with_available(e, available_profiles))?;
            manifest_to_reusable_config(&resolved.manifest)
        }
    };
    config.pod.name = Some(name.to_string());
    config.scope = ScopeConfig {
        allow: scope_allow.to_vec(),
        deny: Vec::new(),
    };
    if let Some(instruction) = instruction_override {
        config.worker.instruction = Some(instruction.to_string());
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
    let config = PodManifestConfig {
        pod: PodMetaConfig {
            name: Some(name.to_string()),
            prompt_pack: None,
        },
        model: model.clone(),
        worker: WorkerManifestConfig {
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
        "invalid SpawnPod.profile: {error}{}",
        available.error_suffix()
    )
}

fn manifest_to_reusable_config(manifest: &PodManifest) -> PodManifestConfig {
    PodManifestConfig {
        pod: PodMetaConfig {
            name: Some(manifest.pod.name.clone()),
            prompt_pack: manifest.pod.prompt_pack.clone(),
        },
        model: manifest.model.clone(),
        worker: WorkerManifestConfig {
            instruction: Some(manifest.worker.instruction.clone()),
            language: Some(manifest.worker.language.clone()),
            max_tokens: manifest.worker.max_tokens,
            max_turns: manifest.worker.max_turns,
            temperature: manifest.worker.temperature,
            top_p: manifest.worker.top_p,
            top_k: manifest.worker.top_k,
            stop_sequences: (!manifest.worker.stop_sequences.is_empty())
                .then_some(manifest.worker.stop_sequences.clone()),
            reasoning: manifest.worker.reasoning.clone(),
            tool_output: ToolOutputLimitsPartial {
                default_max_bytes: Some(manifest.worker.tool_output.default_max_bytes),
                per_tool: manifest.worker.tool_output.per_tool.clone(),
            },
            file_upload: FileUploadLimitsPartial {
                max_bytes: Some(manifest.worker.file_upload.max_bytes),
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
                "spawned pod socket did not appear within {timeout:?}: {}",
                path.display()
            )));
        }
        sleep(Duration::from_millis(50)).await;
    }
}

fn spawn_delivery_error(pod_name: &str, err: SendRunError) -> ToolError {
    match err {
        SendRunError::AlreadyRunning => ToolError::ExecutionFailed(format!(
            "spawned pod `{pod_name}` rejected its initial task as already running; the pod remains registered and can be inspected or stopped"
        )),
        SendRunError::Rejected { code, message } => ToolError::ExecutionFailed(format!(
            "spawned pod `{pod_name}` rejected its initial task with {code:?}: {message}; the pod remains registered and can be inspected or stopped"
        )),
        SendRunError::Io(msg) => ToolError::ExecutionFailed(format!(
            "spawned pod `{pod_name}` did not confirm initial task delivery: {msg}; the pod remains registered and can be inspected or stopped"
        )),
    }
}

fn pod_registry_err_to_tool(e: ScopeLockError) -> ToolError {
    match e {
        ScopeLockError::NotSubset { .. }
        | ScopeLockError::WriteConflict { .. }
        | ScopeLockError::DuplicatePodName(_)
        | ScopeLockError::UnknownPod(_)
        | ScopeLockError::SegmentConflict { .. } => ToolError::InvalidArgument(e.to_string()),
        ScopeLockError::Io(_) => ToolError::ExecutionFailed(e.to_string()),
    }
}

/// Factory for the `SpawnPod` tool.
pub fn spawn_pod_tool(
    spawner_name: String,
    callback_socket: PathBuf,
    runtime_base: PathBuf,
    workspace_root: PathBuf,
    spawner_cwd: PathBuf,
    registry: Arc<SpawnedPodRegistry>,
    parent_socket: Option<PathBuf>,
    spawner_manifest: PodManifest,
    spawner_scope: SharedScope,
    prompts: Arc<PromptCatalog>,
) -> ToolDefinition {
    spawn_pod_tool_impl(
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
pub fn spawn_pod_tool_with_runtime_command(
    spawner_name: String,
    callback_socket: PathBuf,
    runtime_base: PathBuf,
    workspace_root: PathBuf,
    spawner_cwd: PathBuf,
    registry: Arc<SpawnedPodRegistry>,
    parent_socket: Option<PathBuf>,
    spawner_manifest: PodManifest,
    spawner_scope: SharedScope,
    prompts: Arc<PromptCatalog>,
    runtime_command: PodRuntimeCommand,
) -> ToolDefinition {
    spawn_pod_tool_impl(
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

fn spawn_pod_tool_impl(
    spawner_name: String,
    callback_socket: PathBuf,
    runtime_base: PathBuf,
    workspace_root: PathBuf,
    spawner_cwd: PathBuf,
    registry: Arc<SpawnedPodRegistry>,
    parent_socket: Option<PathBuf>,
    spawner_manifest: PodManifest,
    spawner_scope: SharedScope,
    prompts: Arc<PromptCatalog>,
    runtime_command: Option<PodRuntimeCommand>,
) -> ToolDefinition {
    Arc::new(move || {
        let schema = schemars::schema_for!(SpawnPodInput);
        let schema_value = serde_json::to_value(schema).unwrap_or(serde_json::json!({}));
        let available_profiles = AvailableProfiles::discover(&workspace_root);
        let description = prompts
            .spawn_pod_tool_description(
                &available_profiles.compact_list(),
                &available_profiles.default_label(),
                available_profiles.diagnostic(),
            )
            .unwrap_or_else(|e| {
                format!(
                    "Spawn a new Pod process to work on a delegated task. Profile description rendering failed: {e}. Available profiles:\n{}",
                    available_profiles.compact_list()
                )
            });
        let meta = ToolMeta::new("SpawnPod")
            .description(description)
            .input_schema(schema_value);
        let tool: Arc<dyn Tool> = Arc::new(SpawnPodTool::new(
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
                .expect("resolved Pod manifest has a valid delegation scope"),
            runtime_command.clone(),
        ));
        (meta, tool)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use manifest::{AuthRef, ModelManifest, PodManifest, SchemeKind};
    use tempfile::TempDir;

    fn abs_rule(path: &Path, permission: Permission) -> ScopeRule {
        ScopeRule {
            target: path.to_path_buf(),
            permission,
            recursive: true,
        }
    }

    #[test]
    fn spawn_pod_input_schema_includes_optional_cwd() {
        let schema = serde_json::to_value(schemars::schema_for!(SpawnPodInput)).unwrap();
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
    fn spawn_pod_validate_cwd_requires_absolute_existing_directory_in_child_scope() {
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

    fn parent_manifest(root: &Path, deny: Option<&Path>) -> PodManifest {
        PodManifestConfig {
            pod: PodMetaConfig {
                name: Some("parent".into()),
                prompt_pack: None,
            },
            model: ModelManifest {
                scheme: Some(SchemeKind::Anthropic),
                model_id: Some("parent-model".into()),
                auth: Some(AuthRef::None),
                ..Default::default()
            },
            worker: WorkerManifestConfig {
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
        spawner_manifest: &PodManifest,
        available: &AvailableProfiles,
        cwd: &Path,
        name: &str,
        instruction_override: Option<&str>,
        scope: &[ScopeRule],
        selector: SpawnProfileSelector,
    ) -> PodManifestConfig {
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
local profile = require("yoi.profile")
local scope = require("yoi.scope")
return profile {
  slug = "coder",
  model = { scheme = "anthropic", model_id = "coder-model" },
  worker = { instruction = "$yoi/coder", language = "Coderish", max_tokens = 2222 },
  scope = scope.workspace_write(),
}
"#;

    const REVIEWER_PROFILE: &str = r#"
local profile = require("yoi.profile")
local scope = require("yoi.scope")
return profile {
  slug = "reviewer",
  model = { scheme = "anthropic", model_id = "reviewer-model" },
  worker = { instruction = "$yoi/reviewer", language = "Reviewerish", max_tokens = 3333 },
  scope = scope.workspace_write(),
}
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
        let parsed: PodManifestConfig = serde_json::from_str(&config_json).unwrap();

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
        let parsed: PodManifestConfig = serde_json::from_str(&config_json).unwrap();
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
        let parsed: PodManifestConfig = serde_json::from_str(&config_json).unwrap();
        assert_eq!(
            parsed.session.as_ref().and_then(|s| s.record_event_trace),
            Some(true)
        );

        let manifest: PodManifest = PodManifestConfig::builtin_defaults()
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
        let parsed: PodManifestConfig = serde_json::from_str(&config_json).unwrap();

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
                ("coder", "coder.lua", CODER_PROFILE),
                ("reviewer", "reviewer.lua", REVIEWER_PROFILE),
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

        assert_eq!(config.pod.name.as_deref(), Some("child-default"));
        assert_eq!(config.model.model_id.as_deref(), Some("reviewer-model"));
        assert_eq!(config.worker.instruction.as_deref(), Some("$yoi/reviewer"));
        assert_eq!(config.worker.language.as_deref(), Some("Reviewerish"));
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
                ("coder", "coder.lua", CODER_PROFILE),
                ("reviewer", "reviewer.lua", REVIEWER_PROFILE),
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

        assert_eq!(config.pod.name.as_deref(), Some("review-child"));
        assert_eq!(config.model.model_id.as_deref(), Some("reviewer-model"));
        assert_eq!(config.worker.instruction.as_deref(), Some("$yoi/reviewer"));
        assert_eq!(config.worker.language.as_deref(), Some("Reviewerish"));
        assert_eq!(config.worker.max_tokens, Some(3333));
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

        assert_eq!(config.pod.name.as_deref(), Some("inherited-child"));
        assert_eq!(config.model.model_id.as_deref(), Some("parent-model"));
        assert_eq!(config.worker.instruction.as_deref(), Some("$yoi/parent"));
        assert_eq!(config.worker.language.as_deref(), Some("Parentish"));
        assert_eq!(config.worker.max_tokens, Some(1234));
        assert_eq!(
            config.worker.stop_sequences.as_deref(),
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
            &[("reviewer", "reviewer.lua", REVIEWER_PROFILE)],
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
            config.worker.instruction.as_deref(),
            Some("$user/custom-reviewer")
        );
        assert_eq!(config.model.model_id.as_deref(), Some("reviewer-model"));
        assert_eq!(config.worker.language.as_deref(), Some("Reviewerish"));
        assert_eq!(config.worker.max_tokens, Some(3333));
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
            &[("reviewer", "reviewer.lua", REVIEWER_PROFILE)],
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
            &[("coder", "coder.lua", CODER_PROFILE)],
        );
        let parent = parent_manifest(&project, None);
        let scope = vec![abs_rule(&project, Permission::Read)];

        let invalid = parse_spawn_profile_selector(Some("./reviewer.lua"))
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
        std::fs::write(&user_config, "[profile]\ncoder = \"user-coder.lua\"\n").unwrap();
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
            "./reviewer.lua",
            "path:./reviewer.lua",
            "/tmp/reviewer.lua",
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
