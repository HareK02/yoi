//! `SubWorkerSpawn` tool — start a parent-owned Internal Worker session.
//!
//! Resolves a child profile, validates filesystem delegation, constructs a normal Worker with the
//! parent's explicit Workspace authority, installs its enabled features, and hands it to the
//! in-process Internal Worker session actor. No Runtime Worker record, OS process, PID, Unix socket,
//! or machine-wide child allocation is created.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use llm_engine::tool::{Tool, ToolDefinition, ToolError, ToolMeta, ToolOutput};
use manifest::{
    CompactionConfigPartial, DelegationScope, EngineManifestConfig, FileUploadLimitsPartial,
    Permission, PermissionConfigPartial, ProfileDiscovery, ProfileError, ProfileRegistry,
    ProfileRegistrySource, ProfileResolveOptions, ProfileResolver, ProfileSelector, Scope,
    ScopeConfig, ScopeRule, SessionConfigPartial, SharedScope, ToolOutputLimitsPartial,
    WorkerManifest, WorkerManifestConfig, WorkerMetaConfig,
};
use serde::Deserialize;
use tokio::sync::mpsc;

use crate::PromptLoader;
use crate::controller::register_worker_tools;
use crate::internal_worker::{
    EphemeralSessionStore, InternalWorkerSessionStatus, prepare_internal_worker_session,
};
use crate::prompt::catalog::PromptCatalog;
use crate::spawn::registry::SpawnedWorkerRegistry;
use crate::worker::{Worker, WorkerFilesystemAuthority};
use protocol::Method;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct SubWorkerSpawnInput {
    /// Identifier for the spawned Internal SubWorker. Must be unique among this Worker's direct children.
    name: String,
    /// Profile selector for child role configuration. Omit or use `default`
    /// for the effective child default profile, use `inherit` to derive
    /// reusable config from the spawner, or use a registry selector such as
    /// `project:coder`, `project:reviewer`, `builtin:companion`, or an
    /// unambiguous profile slug. Raw/path selectors are rejected.
    #[serde(default)]
    profile: Option<String>,
    /// Instruction-file reference (e.g. `$yoi/default`, `$user/my-agent`).
    #[serde(default)]
    instruction: Option<String>,
    /// Child process/tool working directory. This is not the runtime workspace
    /// root and grants no filesystem authority. When omitted, the spawned SubWorker
    /// starts in the spawner's current working directory.
    #[serde(default)]
    cwd: Option<PathBuf>,
    /// First message sent to the spawned SubWorker via `Method::Run`.
    task: String,
    /// Allow rules delegated to the spawned SubWorker. Must be a subset of the
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
            "SubWorkerSpawn.profile accepts `default`, `inherit`, or registry selectors only; path-like selector `{raw}` is not allowed"
        ));
    }
    if let Some((prefix, name)) = raw.split_once(':') {
        let source = match prefix {
            "builtin" => ProfileRegistrySource::Builtin,
            "user" => ProfileRegistrySource::User,
            "project" => ProfileRegistrySource::Project,
            _ => {
                return Err(format!(
                    "unsupported SubWorkerSpawn.profile selector prefix `{prefix}`; use builtin:, user:, project:, default, or inherit"
                ));
            }
        };
        if name.is_empty() {
            return Err(
                "SubWorkerSpawn.profile registry selector has an empty profile name".into(),
            );
        }
        return Ok(SpawnProfileSelector::Registry(
            ProfileSelector::source_named(source, name),
        ));
    }
    Ok(SpawnProfileSelector::Registry(ProfileSelector::named(raw)))
}

#[derive(Clone)]
pub(crate) enum ParentNotificationTarget {
    Controller(mpsc::WeakSender<Method>),
    Buffer(crate::ipc::notify_buffer::NotifyBuffer),
}

impl ParentNotificationTarget {
    fn notify(&self, message: String, auto_run: bool) {
        match self {
            Self::Controller(parent_method_tx) => {
                let Some(parent_method_tx) = parent_method_tx.upgrade() else {
                    tracing::warn!(
                        "parent Worker controller closed before Internal SubWorker completion notification"
                    );
                    return;
                };
                tokio::spawn(async move {
                    if let Err(error) = parent_method_tx
                        .send(Method::Notify { message, auto_run })
                        .await
                    {
                        tracing::warn!(
                            %error,
                            "failed to notify parent Worker about Internal SubWorker completion"
                        );
                    }
                });
            }
            Self::Buffer(parent_notifies) => parent_notifies.push_notify(message, auto_run),
        }
    }
}

/// Runtime dependencies the `SubWorkerSpawn` tool needs in order to launch a
/// child SubWorker and record the handoff locally. Constructed by the Worker
/// controller once per Worker lifetime.
pub struct SubWorkerSpawnTool {
    /// Spawner's own Worker name, used for direct-child identity collision checks.
    spawner_name: String,
    workspace_context: crate::worker::WorkerWorkspaceContext,
    parent_notifications: ParentNotificationTarget,
    /// Runtime-owned root used only for bounded Internal Worker tool artifacts such as Bash spill
    /// output. It is not an Internal Worker identity or catalog location.
    runtime_base: PathBuf,
    /// Inherited runtime workspace root for Profile/project/Ticket/workflow/
    /// memory context. SubWorkerSpawn `cwd` must not affect this value.
    workspace_root: PathBuf,
    /// Directory the spawned SubWorker's tools should use when the LLM did not
    /// override it. Defaults to the spawner's cwd.
    spawner_cwd: PathBuf,
    /// Parent-owned in-memory registry shared by the five SubWorker tools.
    registry: Arc<SpawnedWorkerRegistry>,
    /// Spawner's resolved Manifest. `profile = "inherit"` derives the
    /// child config from reusable fields here, and selected profiles are
    /// merged into the same internal handoff shape before launch.
    spawner_manifest: WorkerManifest,
    prompt_loader: PromptLoader,
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
    internal_client_override: Option<Box<dyn llm_engine::llm_client::LlmClient>>,
}

impl SubWorkerSpawnTool {
    #[cfg(test)]
    fn with_internal_client(mut self, client: Box<dyn llm_engine::llm_client::LlmClient>) -> Self {
        self.internal_client_override = Some(client);
        self
    }

    fn new(
        spawner_name: String,
        workspace_context: crate::worker::WorkerWorkspaceContext,
        parent_notifications: ParentNotificationTarget,
        runtime_base: PathBuf,
        workspace_root: PathBuf,
        spawner_cwd: PathBuf,
        registry: Arc<SpawnedWorkerRegistry>,
        spawner_manifest: WorkerManifest,
        prompt_loader: PromptLoader,
        available_profiles: AvailableProfiles,
        spawner_scope: SharedScope,
        delegation_scope: DelegationScope,
    ) -> Self {
        Self {
            spawner_name,
            workspace_context,
            parent_notifications,
            runtime_base,
            workspace_root,
            spawner_cwd,
            registry,
            spawner_manifest,
            prompt_loader,
            available_profiles,
            spawner_scope,
            delegation_scope,
            internal_client_override: None,
        }
    }
}

#[async_trait]
impl Tool for SubWorkerSpawnTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: llm_engine::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let input: SubWorkerSpawnInput = serde_json::from_str(input_json).map_err(|e| {
            ToolError::InvalidArgument(format!("invalid SubWorkerSpawn input: {e}"))
        })?;

        // `delegate_scope` catches this too (as `DuplicateWorkerName`), but
        // the dedicated message is kinder to the LLM — which gets the
        // error back verbatim — than the generic duplicate-name error.
        if input.name == self.spawner_name {
            return Err(ToolError::InvalidArgument(format!(
                "spawned worker name `{}` collides with spawner's own name",
                input.name
            )));
        }
        let name_reservation = self
            .registry
            .reserve_internal_name(input.name.clone())
            .map_err(|error| ToolError::InvalidArgument(error.to_string()))?;

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

        let mut child_config: WorkerManifestConfig = serde_json::from_str(&spawn_config_json)
            .map_err(|error| {
                ToolError::ExecutionFailed(format!("resolve child manifest: {error}"))
            })?;
        child_config.delegation_scope = ScopeConfig {
            allow: scope_allow.clone(),
            deny: Vec::new(),
        };
        let child_manifest =
            WorkerManifest::try_from(WorkerManifestConfig::builtin_defaults().merge(child_config))
                .map_err(|error| {
                    ToolError::ExecutionFailed(format!("resolve child manifest: {error}"))
                })?;
        let store = EphemeralSessionStore::default();
        let filesystem_authority =
            WorkerFilesystemAuthority::local(self.workspace_root.clone(), child_cwd.clone());
        let mut child = Worker::<Box<dyn llm_engine::llm_client::LlmClient>, EphemeralSessionStore>::from_internal_manifest_with_context(
            child_manifest,
            store.clone(),
            self.prompt_loader.clone(),
            self.workspace_context.clone(),
            filesystem_authority,
            self.internal_client_override
                .as_ref()
                .map(|client| client.clone_boxed()),
        )
        .await
        .map_err(|error| ToolError::ExecutionFailed(format!("build Internal Worker: {error}")))?;
        let child_scope = child.scope().clone();
        let child_registry = SpawnedWorkerRegistry::new_internal(input.name.clone(), child_scope);
        register_worker_tools(
            &mut child,
            self.runtime_base
                .join("internal-workers")
                .join(&input.name)
                .join("bash-output"),
            self.runtime_base.clone(),
            child_registry,
            None,
        )
        .await
        .map_err(|error| {
            ToolError::ExecutionFailed(format!("install Internal Worker features: {error}"))
        })?;
        // Transfer delegated Write authority before the child accepts its first turn. This closes
        // the parallel-tool window where parent and child could otherwise both write the same path.
        // The machine-wide allocation remains owned by the parent Worker; no fake child PID/socket
        // identity is introduced.
        let revoke_write: Vec<ScopeRule> = scope_allow
            .iter()
            .filter(|rule| rule.permission == Permission::Write)
            .cloned()
            .collect();
        if !revoke_write.is_empty() {
            self.spawner_scope
                .update(|current| current.with_added_deny_rules(revoke_write.clone()))
                .map_err(|error| {
                    ToolError::ExecutionFailed(format!("revoke spawner scope: {error}"))
                })?;
        }

        let child_name = input.name.clone();
        let registry = Arc::downgrade(&self.registry);
        let parent_notifications = self.parent_notifications.clone();
        let session_result = prepare_internal_worker_session(
            child,
            store,
            Some(Arc::new(move |status| {
                if status == InternalWorkerSessionStatus::Failed {
                    if let Some(registry) = registry.upgrade() {
                        if let Err(error) = registry.reclaim_internal_scope(&child_name) {
                            tracing::warn!(
                                child_name,
                                %error,
                                "failed to reclaim delegated scope after Internal SubWorker failure"
                            );
                        }
                    }
                }
                let message = format!(
                    "SubWorker `{child_name}` turn ended with status {status:?}. Inspect its committed session with worker-observation tools before making completion decisions."
                );
                parent_notifications.notify(message, true);
            })),
        )
        .await;
        let session = match session_result {
            Ok(session) => session,
            Err(error) => {
                if !revoke_write.is_empty() {
                    let _ = self
                        .spawner_scope
                        .update(|current| current.with_removed_deny_rules(revoke_write.clone()));
                }
                return Err(ToolError::ExecutionFailed(format!(
                    "prepare Internal Worker session: {error}"
                )));
            }
        };

        let record = crate::spawn::registry::InternalSpawnedWorkerRecord::new(
            input.name.clone(),
            scope_allow,
            session.clone(),
        );
        if let Err(error) = name_reservation.commit(record) {
            let _ = session.stop().await;
            if !revoke_write.is_empty() {
                let _ = self
                    .spawner_scope
                    .update(|current| current.with_removed_deny_rules(revoke_write));
            }
            return Err(ToolError::ExecutionFailed(format!(
                "register Internal Worker session: {error}"
            )));
        }
        if let Err(error) = session.send(input.task).await {
            let _ = session.stop().await;
            let _ = self.registry.remove_internal(&input.name).await;
            return Err(ToolError::ExecutionFailed(format!(
                "start Internal Worker session: {error}"
            )));
        }

        Ok(ToolOutput {
            summary: format!("spawned internal worker `{}`", input.name),
            content: None,
        })
    }
}

impl SubWorkerSpawnTool {
    fn validate_delegation_scope(&self, scope_allow: &[ScopeRule]) -> Result<(), ToolError> {
        if self.delegation_scope.is_empty() && !scope_allow.is_empty() {
            return Err(ToolError::InvalidArgument(
                "SubWorkerSpawn requires delegation authority, but this Worker has no delegation scope grant; direct filesystem scope only authorizes this Worker's own tools".into(),
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
            "SubWorkerSpawn.cwd must be absolute: {}",
            cwd.display()
        )));
    }
    let metadata = std::fs::metadata(cwd).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            ToolError::InvalidArgument(format!(
                "SubWorkerSpawn.cwd does not exist: {}",
                cwd.display()
            ))
        } else {
            ToolError::InvalidArgument(format!(
                "SubWorkerSpawn.cwd is not usable: {}: {e}",
                cwd.display()
            ))
        }
    })?;
    if !metadata.is_dir() {
        return Err(ToolError::InvalidArgument(format!(
            "SubWorkerSpawn.cwd must be a directory: {}",
            cwd.display()
        )));
    }
    let canonical = std::fs::canonicalize(cwd).map_err(|e| {
        ToolError::InvalidArgument(format!(
            "SubWorkerSpawn.cwd is not usable: {}: {e}",
            cwd.display()
        ))
    })?;
    let child_scope = Scope::from_config(&ScopeConfig {
        allow: scope_allow.to_vec(),
        deny: Vec::new(),
    })
    .map_err(|e| {
        ToolError::InvalidArgument(format!(
            "requested child scope cannot validate SubWorkerSpawn.cwd: {e}"
        ))
    })?;
    if !child_scope.is_readable(&canonical) {
        return Err(ToolError::InvalidArgument(format!(
            "SubWorkerSpawn.cwd {} is outside the child's delegated readable scope; cwd grants no authority, so add an explicit read or write scope rule covering it",
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
impl SubWorkerSpawnTool {
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
                    "profile discovery failed for SubWorkerSpawn: {}{}",
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
        "invalid SubWorkerSpawn.profile: {error}{}",
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
/// Factory for the `SubWorkerSpawn` tool.
pub(crate) fn sub_worker_spawn_tool(
    spawner_name: String,
    workspace_context: crate::worker::WorkerWorkspaceContext,
    parent_notifications: ParentNotificationTarget,
    runtime_base: PathBuf,
    workspace_root: PathBuf,
    spawner_cwd: PathBuf,
    registry: Arc<SpawnedWorkerRegistry>,
    spawner_manifest: WorkerManifest,
    spawner_scope: SharedScope,
    prompts: Arc<PromptCatalog>,
) -> ToolDefinition {
    sub_worker_spawn_tool_impl(
        spawner_name,
        workspace_context,
        parent_notifications,
        runtime_base,
        workspace_root,
        spawner_cwd,
        registry,
        spawner_manifest,
        spawner_scope,
        prompts,
    )
}

fn sub_worker_spawn_tool_impl(
    spawner_name: String,
    workspace_context: crate::worker::WorkerWorkspaceContext,
    parent_notifications: ParentNotificationTarget,
    runtime_base: PathBuf,
    workspace_root: PathBuf,
    spawner_cwd: PathBuf,
    registry: Arc<SpawnedWorkerRegistry>,
    spawner_manifest: WorkerManifest,
    spawner_scope: SharedScope,
    prompts: Arc<PromptCatalog>,
) -> ToolDefinition {
    Arc::new(move || {
        let schema = schemars::schema_for!(SubWorkerSpawnInput);
        let schema_value = serde_json::to_value(schema).unwrap_or(serde_json::json!({}));
        let available_profiles = AvailableProfiles::discover(&workspace_root);
        let description = prompts
            .sub_worker_spawn_tool_description(
                &available_profiles.compact_list(),
                &available_profiles.default_label(),
                available_profiles.diagnostic(),
            )
            .unwrap_or_else(|e| {
                format!(
                    "Spawn an in-process Internal SubWorker session to split context for a delegated task. Profile description rendering failed: {e}. Available profiles:\n{}",
                    available_profiles.compact_list()
                )
            });
        let meta = ToolMeta::new("SubWorkerSpawn")
            .description(description)
            .input_schema(schema_value);
        let tool: Arc<dyn Tool> = Arc::new(SubWorkerSpawnTool::new(
            spawner_name.clone(),
            workspace_context.clone(),
            parent_notifications.clone(),
            runtime_base.clone(),
            workspace_root.clone(),
            spawner_cwd.clone(),
            registry.clone(),
            spawner_manifest.clone(),
            prompts.loader(),
            available_profiles,
            spawner_scope.clone(),
            DelegationScope::from_config(&spawner_manifest.delegation_scope)
                .expect("resolved Worker manifest has a valid delegation scope"),
        ));
        (meta, tool)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::Duration;

    use crate::WorkspaceId;
    use async_trait::async_trait;
    use futures::Stream;
    use llm_engine::llm_client::event::{Event as LlmEvent, ResponseStatus, StatusEvent};
    use llm_engine::llm_client::types::ContentPart;
    use llm_engine::llm_client::{ClientError, LlmClient, Request};
    use llm_engine::{Item, Role};
    use manifest::{AuthRef, ModelManifest, SchemeKind, WorkerManifest};
    use tempfile::TempDir;

    use crate::worker::{
        WorkspaceClient, WorkspaceClientError, WorkspaceRequest, WorkspaceResponse,
    };

    fn abs_rule(path: &Path, permission: Permission) -> ScopeRule {
        ScopeRule {
            target: path.to_path_buf(),
            permission,
            recursive: true,
        }
    }

    const INTERNAL_REVIEWER_PROFILE: &str = r#"
slug = "reviewer"
scope = "workspace_read"

[model]
scheme = "anthropic"
model_id = "reviewer-model"

[model.auth]
kind = "none"

[engine]
instruction = "$yoi/reviewer"
language = "Reviewerish"
max_tokens = 3333

[feature.ticket]
enabled = true
thread = true

[feature.memory]
enabled = true

[memory]
extract_threshold = 4000
"#;

    #[tokio::test]
    async fn parent_controller_notification_target_does_not_keep_channel_open() {
        let (parent_method_tx, mut parent_method_rx) = mpsc::channel(1);
        let target = ParentNotificationTarget::Controller(parent_method_tx.downgrade());

        drop(parent_method_tx);

        assert!(parent_method_rx.recv().await.is_none());
        target.notify("late completion".to_string(), true);
    }

    #[tokio::test]
    async fn reviewer_profile_spawns_and_notifies_parent_controller() {
        let runtime = TempDir::new().unwrap();
        let workspace_root = runtime.path().join("project");
        let available_profiles = write_project_profile_registry(
            &workspace_root,
            Some("reviewer"),
            &[("reviewer", "reviewer.toml", INTERNAL_REVIEWER_PROFILE)],
        );
        let mut manifest = parent_manifest(&workspace_root, None);
        manifest.delegation_scope = ScopeConfig {
            allow: vec![abs_rule(&workspace_root, Permission::Write)],
            deny: Vec::new(),
        };
        let spawner_scope = SharedScope::new(Scope::from_config(&manifest.scope).unwrap());
        let registry = SpawnedWorkerRegistry::new_internal("parent".into(), spawner_scope.clone());
        let workspace_context = crate::worker::WorkerWorkspaceContext::with_client(
            Some(WorkspaceId::new("workspace-test").unwrap()),
            Arc::new(AvailableWorkspaceClient),
        );
        let calls = Arc::new(AtomicUsize::new(0));
        let observed_parent_write_revoked = Arc::new(AtomicBool::new(false));
        let observed_instruction_override = Arc::new(AtomicBool::new(false));
        let fail_requests = Arc::new(AtomicBool::new(false));
        let workspace_prompts = runtime.path().join("workspace-prompts");
        std::fs::create_dir_all(&workspace_prompts).unwrap();
        std::fs::write(
            workspace_prompts.join("custom-reviewer.md"),
            "WORKSPACE REVIEWER OVERRIDE",
        )
        .unwrap();
        let prompt_loader = PromptLoader::new(None, Some(workspace_prompts));
        let (parent_method_tx, mut parent_method_rx) = mpsc::channel(8);
        let tool = SubWorkerSpawnTool::new(
            "parent".into(),
            workspace_context,
            ParentNotificationTarget::Controller(parent_method_tx.downgrade()),
            runtime.path().to_path_buf(),
            workspace_root.clone(),
            workspace_root.clone(),
            registry.clone(),
            manifest.clone(),
            prompt_loader,
            available_profiles,
            spawner_scope.clone(),
            DelegationScope::from_config(&manifest.delegation_scope).unwrap(),
        )
        .with_internal_client(Box::new(ScriptedInternalClient {
            calls: calls.clone(),
            parent_scope: spawner_scope.clone(),
            delegated_path: workspace_root.clone(),
            observed_parent_write_revoked: observed_parent_write_revoked.clone(),
            observed_instruction_override: observed_instruction_override.clone(),
            fail_requests: fail_requests.clone(),
        }));
        let input = serde_json::json!({
            "name": "reviewer-child",
            "profile": "project:reviewer",
            "instruction": "$workspace/custom-reviewer",
            "task": "review immutable commit",
            "scope": [{
                "target": workspace_root.clone(),
                "permission": "write",
                "recursive": true
            }]
        });

        assert!(spawner_scope.snapshot().is_writable(&workspace_root));

        let mut invalid_input = input.clone();
        invalid_input["scope"][0]["target"] =
            serde_json::json!(runtime.path().join("outside-parent-scope"));
        tool.execute(
            &serde_json::to_string(&invalid_input).unwrap(),
            llm_engine::tool::ToolExecutionContext::direct(),
        )
        .await
        .expect_err("invalid delegation must fail before child preparation");
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(spawner_scope.snapshot().is_writable(&workspace_root));

        let output = tool
            .execute(
                &serde_json::to_string(&input).unwrap(),
                llm_engine::tool::ToolExecutionContext::direct(),
            )
            .await
            .expect("spawn project reviewer as Internal Worker");
        assert!(output.summary.contains("internal worker `reviewer-child`"));
        assert!(!spawner_scope.snapshot().is_writable(&workspace_root));
        let record = registry
            .get_internal("reviewer-child")
            .expect("Internal reviewer registry record");
        assert_eq!(
            record.session.wait_until_idle().await,
            crate::internal_worker::InternalWorkerSessionStatus::Idle
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(observed_parent_write_revoked.load(Ordering::SeqCst));
        assert!(observed_instruction_override.load(Ordering::SeqCst));
        let completion = tokio::time::timeout(Duration::from_secs(1), parent_method_rx.recv())
            .await
            .expect("SubWorker completion must wake the parent method channel")
            .expect("parent method channel remains open");
        assert!(matches!(
            completion,
            Method::Notify {
                message,
                auto_run: true,
            } if message.contains("SubWorker `reviewer-child` turn ended with status Idle")
        ));
        assert!(!runtime.path().join("reviewer-child/sock").exists());

        let duplicate_error = tool
            .execute(
                &serde_json::to_string(&input).unwrap(),
                llm_engine::tool::ToolExecutionContext::direct(),
            )
            .await
            .expect_err("duplicate child name must be rejected before a first turn starts");
        assert!(
            format!("{duplicate_error:?}").contains("already registered"),
            "unexpected duplicate error: {duplicate_error:?}"
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "duplicate rejection must not invoke the child provider"
        );
        assert!(matches!(
            parent_method_rx.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty)
        ));

        let context = llm_engine::tool::ToolExecutionContext::direct();
        let list = (crate::spawn::comm_tools::sub_worker_list_tool(registry.clone()))().1;
        let listed = list.execute("{}", context.clone()).await.unwrap();
        assert!(
            listed
                .content
                .unwrap_or_default()
                .contains("reviewer-child")
        );

        let observation =
            crate::feature::builtin::worker_observation::SpawnedSubWorkerObservationProvider::new(
                registry.clone(),
            );
        let observed_child =
            crate::feature::builtin::worker_observation::WorkerObservationSubjectRef::SubWorker {
                name: "reviewer-child".to_string(),
            };
        let first_capture = crate::feature::builtin::worker_observation::WorkerObservationProvider::capture_worker_session(
            &observation,
            &observed_child,
        )
        .await
        .unwrap();
        assert!(first_capture.items.iter().any(|item| {
            matches!(item, Item::Message { role: Role::Assistant, content, .. } if content.iter().any(|part| matches!(part, ContentPart::Text { text } if text.contains("reviewed"))))
        }));

        let send = (crate::spawn::comm_tools::sub_worker_send_tool(registry.clone()))().1;
        send.execute(
            r#"{"name":"reviewer-child","message":"review follow-up"}"#,
            context.clone(),
        )
        .await
        .unwrap();
        assert_eq!(
            record.session.wait_until_idle().await,
            crate::internal_worker::InternalWorkerSessionStatus::Idle
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        let latest_capture = crate::feature::builtin::worker_observation::WorkerObservationProvider::capture_worker_session(
            &observation,
            &observed_child,
        )
        .await
        .unwrap();
        assert!(latest_capture.items.len() > first_capture.items.len());

        fail_requests.store(true, Ordering::SeqCst);
        send.execute(
            r#"{"name":"reviewer-child","message":"trigger terminal failure"}"#,
            context.clone(),
        )
        .await
        .unwrap();
        assert_eq!(
            record.session.wait_until_idle().await,
            InternalWorkerSessionStatus::Failed
        );
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        assert!(
            spawner_scope.snapshot().is_writable(&workspace_root),
            "Failed terminal child must automatically reclaim its delegated write scope"
        );
        assert!(registry.get_internal("reviewer-child").is_some());

        let stop = (crate::spawn::comm_tools::sub_worker_stop_tool(registry.clone()))().1;
        stop.execute(r#"{"name":"reviewer-child"}"#, context)
            .await
            .unwrap();
        assert!(registry.get_internal("reviewer-child").is_none());
        assert!(spawner_scope.snapshot().is_writable(&workspace_root));

        fail_requests.store(false, Ordering::SeqCst);
        let mut teardown_input = input;
        teardown_input["name"] = serde_json::json!("reviewer-child-parent-drop");
        tool.execute(
            &serde_json::to_string(&teardown_input).unwrap(),
            llm_engine::tool::ToolExecutionContext::direct(),
        )
        .await
        .unwrap();
        assert!(!spawner_scope.snapshot().is_writable(&workspace_root));
        drop(list);
        drop(send);
        drop(stop);
        drop(observation);
        drop(tool);
        drop(registry);
        assert!(spawner_scope.snapshot().is_writable(&workspace_root));
    }

    #[test]
    fn spawn_worker_input_schema_includes_optional_cwd() {
        let schema = serde_json::to_value(schemars::schema_for!(SubWorkerSpawnInput)).unwrap();
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

    #[derive(Clone)]
    struct ScriptedInternalClient {
        calls: Arc<AtomicUsize>,
        parent_scope: SharedScope,
        delegated_path: PathBuf,
        observed_parent_write_revoked: Arc<std::sync::atomic::AtomicBool>,
        observed_instruction_override: Arc<std::sync::atomic::AtomicBool>,
        fail_requests: Arc<AtomicBool>,
    }

    #[async_trait]
    impl LlmClient for ScriptedInternalClient {
        fn clone_boxed(&self) -> Box<dyn LlmClient> {
            Box::new(self.clone())
        }

        async fn stream(
            &self,
            request: Request,
        ) -> Result<Pin<Box<dyn Stream<Item = Result<LlmEvent, ClientError>> + Send>>, ClientError>
        {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.observed_parent_write_revoked.store(
                !self
                    .parent_scope
                    .snapshot()
                    .is_writable(&self.delegated_path),
                Ordering::SeqCst,
            );
            self.observed_instruction_override.store(
                request
                    .system_prompt
                    .as_deref()
                    .is_some_and(|prompt| prompt.contains("WORKSPACE REVIEWER OVERRIDE")),
                Ordering::SeqCst,
            );
            if self.fail_requests.load(Ordering::SeqCst) {
                return Err(ClientError::Config(
                    "scripted Internal Worker failure".into(),
                ));
            }
            Ok(Box::pin(futures::stream::iter(vec![
                Ok(LlmEvent::text_block_start(0)),
                Ok(LlmEvent::text_delta(0, "reviewed")),
                Ok(LlmEvent::text_block_stop(0, None)),
                Ok(LlmEvent::Status(StatusEvent {
                    status: ResponseStatus::Completed,
                })),
            ])))
        }
    }

    #[derive(Debug)]
    struct AvailableWorkspaceClient;

    impl WorkspaceClient for AvailableWorkspaceClient {
        fn workspace_id(&self) -> Option<&str> {
            Some("workspace-test")
        }

        fn kind(&self) -> &str {
            "test"
        }

        fn is_available(&self) -> bool {
            true
        }

        fn execute(
            &self,
            _request: WorkspaceRequest,
        ) -> Result<WorkspaceResponse, WorkspaceClientError> {
            Err(WorkspaceClientError::Unavailable("not invoked".into()))
        }
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

        let default_error = build_spawn_config_json_for_profile(
            &parent,
            &available,
            &project,
            "child",
            None,
            &scope,
            SpawnProfileSelector::Default,
        )
        .unwrap_err();
        assert!(default_error.contains("no default profile is configured"));

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
