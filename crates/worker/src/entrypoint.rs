use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::{
    PromptLoader, Worker, WorkerController, WorkerFilesystemAuthority, WorkerWorkspaceContext,
    WorkspaceId,
};
use clap::{CommandFactory, FromArgMatches, Parser};
use manifest::{
    Permission, ProfileResolveOptions, ProfileResolver, ProfileSelector, ScopeConfig, ScopeRule,
    WorkerManifest, WorkerManifestConfig, paths,
    plugin::{PluginDiscoveryOptions, resolve_plugin_config_for_startup},
};
use session_store::{CombinedStore, FsWorkerStore, WorkerMetadataStore};
use session_store::{FsStore, SegmentId, Store};
use ticket::config::TicketRole;

#[derive(Debug, Parser)]
#[command(about = "Spawn a Worker process from a profile or a single manifest file")]
struct Cli {
    /// Profile to evaluate. Accepts an explicit path, `path:<path>`, a
    /// discovered profile name, `default`, or a source-qualified name such as
    /// `project:coder`.
    #[arg(
        long,
        value_name = "PROFILE",
        conflicts_with_all = ["manifest", "project", "session", "adopt"]
    )]
    profile: Option<String>,

    /// Runtime workspace root for profile discovery, default Worker naming, and process context.
    /// Defaults to the current directory.
    #[arg(long, value_name = "PATH")]
    workspace: Option<PathBuf>,

    /// Manifest TOML to use directly as a one-file compatibility/debug input.
    /// This bypasses profile discovery but still applies builtin defaults and
    /// the same required-field validation boundary.
    #[arg(long, value_name = "PATH", conflicts_with_all = ["project"])]
    manifest: Option<PathBuf>,

    /// Deprecated manifest-cascade project root flag. Ambient project/user
    /// manifest discovery has been removed; configure/select a profile instead.
    #[arg(long, value_name = "PATH")]
    project: Option<PathBuf>,

    /// Internal resolved manifest config for delegated child Worker spawning.
    #[arg(
        long,
        value_name = "JSON",
        requires = "adopt",
        conflicts_with_all = ["profile", "manifest", "project", "worker", "session"],
        hide = true
    )]
    spawn_config_json: Option<String>,

    /// Directory for session persistence. Defaults to
    /// `<data_dir>/sessions/` (see `manifest::paths`).
    #[arg(short, long)]
    store: Option<PathBuf>,

    /// Claim a scope allocation pre-registered by a spawning Worker, rather
    /// than installing a new top-level allocation. Used only when this
    /// process is launched by `SpawnWorker`; end users should never pass it.
    #[arg(long)]
    adopt: bool,

    /// Socket path of the spawning Worker, for delivering `Method::Notify`
    /// callbacks upward. Required alongside `--adopt`.
    #[arg(long, value_name = "PATH", requires = "adopt")]
    callback: Option<PathBuf>,

    /// Process-local Ticket role marker supplied by the Ticket role launcher.
    #[arg(long, hide = true)]
    ticket_role: Option<String>,

    /// Resume or create a Worker by name. If name-keyed Worker state exists,
    /// the active session/segment recorded there is restored; otherwise a
    /// fresh top-level Worker is created with this name.
    #[arg(long, value_name = "NAME", conflicts_with_all = ["adopt"])]
    worker: Option<String>,

    /// Require `--worker` to restore existing Worker state instead of creating a
    /// fresh Worker when no state exists. Used by Worker discovery restore flows.
    #[arg(long, requires = "worker")]
    require_worker_state: bool,

    /// Restore a Worker from an existing session. The Worker re-uses the
    /// given session id and appends new turns to the same jsonl;
    /// concurrent writers are prevented by the worker-allocation.
    /// Mutually exclusive with `--adopt` (spawned children always start
    /// fresh).
    #[arg(long, value_name = "UUID", conflicts_with_all = ["adopt"])]
    session: Option<SegmentId>,
}

fn runtime_workspace_root(cli: &Cli) -> Result<PathBuf, String> {
    let raw = cli.workspace.as_deref().unwrap_or_else(|| Path::new("."));
    if raw.is_absolute() {
        Ok(raw.to_path_buf())
    } else {
        std::env::current_dir()
            .map_err(|e| format!("failed to resolve current directory for workspace: {e}"))
            .map(|cwd| cwd.join(raw))
    }
}

fn runtime_workspace_context(workspace_root: &Path) -> WorkerWorkspaceContext {
    WorkerWorkspaceContext::local_filesystem(read_workspace_id_hint(workspace_root))
}

fn read_workspace_id_hint(workspace_root: &Path) -> Option<WorkspaceId> {
    let path = workspace_root.join(".yoi/workspace.toml");
    let contents = std::fs::read_to_string(path).ok()?;
    let value = toml::from_str::<toml::Value>(&contents).ok()?;
    let id = value.get("id")?.as_str()?.to_string();
    match WorkspaceId::new(id) {
        Ok(id) => Some(id),
        Err(err) => {
            tracing::warn!("ignoring invalid workspace id in .yoi/workspace.toml: {err}");
            None
        }
    }
}

fn runtime_worker_name(cli: &Cli, workspace_root: &Path) -> String {
    cli.worker
        .as_deref()
        .map(str::to_string)
        .unwrap_or_else(|| default_worker_name_for_workspace(workspace_root))
}

fn default_worker_name_for_workspace(workspace_root: &Path) -> String {
    let raw = workspace_root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("workspace");
    sanitise_worker_name(raw)
}

fn sanitise_worker_name(raw: &str) -> String {
    let name: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '-'
            }
        })
        .collect();
    if name.chars().any(|c| c.is_ascii_alphanumeric()) {
        name
    } else {
        "workspace".to_string()
    }
}

fn resolve_manifest(cli: &Cli) -> Result<(WorkerManifest, PromptLoader), String> {
    resolve_manifest_with_profile_loader(cli, load_profile)
}

fn resolve_manifest_with_profile_loader<F>(
    cli: &Cli,
    load_profile_fn: F,
) -> Result<(WorkerManifest, PromptLoader), String>
where
    F: FnOnce(&ProfileSelector, &Path, &str) -> Result<(WorkerManifest, PromptLoader), String>,
{
    let workspace_root = runtime_workspace_root(cli)?;
    let runtime_worker_name = runtime_worker_name(cli, &workspace_root);
    let ((mut manifest, loader), manifest_source) =
        if let Some(config_json) = cli.spawn_config_json.as_deref() {
            (
                load_spawn_config_json(config_json)?,
                ManifestSource::SpawnConfig,
            )
        } else if let Some(profile) = &cli.profile {
            let selector = ProfileSelector::parse_cli(profile);
            (
                load_profile_fn(&selector, &workspace_root, &runtime_worker_name)?,
                ManifestSource::ProfileLaunch,
            )
        } else if let Some(path) = &cli.manifest {
            (
                load_single_manifest(path, cli.worker.as_deref(), &runtime_worker_name)?,
                ManifestSource::ManifestFile,
            )
        } else {
            if cli.project.is_some() {
                return Err(
                "--project is no longer supported; normal startup uses profile discovery/default, \
                 and --manifest <PATH> is the only one-file manifest mode"
                    .to_string(),
            );
            }
            let selector = ProfileSelector::Default;
            (
                load_profile_fn(&selector, &workspace_root, &runtime_worker_name)?,
                ManifestSource::ProfileLaunch,
            )
        };

    if manifest_source == ManifestSource::ProfileLaunch {
        apply_profile_launch_policy(&mut manifest, &workspace_root, cli.ticket_role.as_deref())?;
    }
    apply_session_restore_overrides(&mut manifest, cli)?;
    apply_plugin_resolution_plan(&mut manifest, &workspace_root);
    Ok((manifest, loader))
}

fn apply_plugin_resolution_plan(manifest: &mut WorkerManifest, workspace_root: &Path) {
    let options = PluginDiscoveryOptions::new(workspace_root);
    manifest.plugins = resolve_plugin_config_for_startup(&manifest.plugins, &options);
}

fn apply_session_restore_overrides(manifest: &mut WorkerManifest, cli: &Cli) -> Result<(), String> {
    if let Some(worker_name) = cli.worker.as_deref() {
        manifest.worker.name = worker_name.to_string();
    }
    Ok(())
}

fn load_spawn_config_json(config_json: &str) -> Result<(WorkerManifest, PromptLoader), String> {
    let config = serde_json::from_str::<WorkerManifestConfig>(config_json)
        .map_err(|e| format!("failed to parse --spawn-config-json: {e}"))?;
    let manifest = WorkerManifest::try_from(WorkerManifestConfig::builtin_defaults().merge(config))
        .map_err(|e| format!("failed to resolve --spawn-config-json: {e}"))?;
    Ok((manifest, PromptLoader::builtins_only()))
}

fn load_profile(
    selector: &ProfileSelector,
    workspace_root: &Path,
    worker_name: &str,
) -> Result<(WorkerManifest, PromptLoader), String> {
    let resolver = ProfileResolver::new().with_workspace_base(workspace_root);
    let options = ProfileResolveOptions::with_worker_name(worker_name);
    let resolved = resolver.resolve(selector, options).map_err(|e| {
        format!(
            "failed to resolve profile {}: {e}",
            selector.display_label()
        )
    })?;
    Ok((resolved.manifest, PromptLoader::builtins_only()))
}

pub fn resolve_runtime_profile_manifest(
    _profile: Option<&str>,
    _workspace_root: &Path,
    _worker_name: &str,
) -> Result<(WorkerManifest, PromptLoader), String> {
    Err(
        "runtime profile resolution requires a pre-resolved manifest/profile archive from Backend authority"
            .to_string(),
    )
}

pub fn resolve_runtime_profile_manifest_from_manifest(
    mut manifest: WorkerManifest,
    workspace_root: &Path,
    worker_name: &str,
) -> Result<(WorkerManifest, PromptLoader), String> {
    if manifest.worker.name.is_empty() {
        manifest.worker.name = worker_name.to_string();
    }
    apply_profile_launch_policy(&mut manifest, workspace_root, None)?;
    // Do not run plugin discovery here: runtime-created Workers receive their
    // resolved manifest/profile archive from Backend authority, not by scanning
    // the materialized workdir's `.yoi/plugins`.
    Ok((manifest, PromptLoader::builtins_only()))
}

pub fn resolve_runtime_profile_manifest_from_manifest_without_filesystem(
    mut manifest: WorkerManifest,
    _workspace_root: &Path,
    worker_name: &str,
) -> Result<(WorkerManifest, PromptLoader), String> {
    if manifest.worker.name.is_empty() {
        manifest.worker.name = worker_name.to_string();
    }
    manifest.scope = ScopeConfig::default();
    manifest.delegation_scope = ScopeConfig::default();
    // Same as the filesystem-capable runtime path: no local `.yoi` discovery.
    Ok((manifest, PromptLoader::builtins_only()))
}

fn load_single_manifest(
    path: &Path,
    explicit_worker_name: Option<&str>,
    default_worker_name: &str,
) -> Result<(WorkerManifest, PromptLoader), String> {
    let toml = std::fs::read_to_string(path)
        .map_err(|e| format!("failed to read manifest {}: {e}", path.display()))?;
    let absolute_path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| format!("failed to resolve current directory: {e}"))?
            .join(path)
    };
    let base_dir = absolute_path.parent().ok_or_else(|| {
        format!(
            "manifest path {} has no parent directory",
            absolute_path.display()
        )
    })?;
    let mut config = WorkerManifestConfig::builtin_defaults().merge(
        WorkerManifestConfig::from_toml(&toml)
            .map_err(|e| format!("failed to parse manifest {}: {e}", path.display()))?
            .resolve_paths(base_dir),
    );
    if let Some(worker_name) = explicit_worker_name {
        config.worker.name = Some(worker_name.to_string());
    } else if config.worker.name.is_none() {
        config.worker.name = Some(default_worker_name.to_string());
    }
    let manifest = WorkerManifest::try_from(config)
        .map_err(|e| format!("failed to resolve manifest {}: {e}", path.display()))?;
    if manifest.scope.allow.is_empty() {
        return Err(format!(
            "manifest {} must declare scope.allow; profile launches receive concrete scope from launch policy",
            path.display()
        ));
    }
    Ok((manifest, PromptLoader::builtins_only()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ManifestSource {
    ProfileLaunch,
    ManifestFile,
    SpawnConfig,
}

fn read_rule(target: PathBuf) -> ScopeRule {
    ScopeRule {
        target,
        permission: Permission::Read,
        recursive: true,
    }
}

fn write_rule(target: PathBuf) -> ScopeRule {
    ScopeRule {
        target,
        permission: Permission::Write,
        recursive: true,
    }
}

fn workspace_scope(
    workspace_root: &Path,
    permission: Permission,
    deny_write: &[PathBuf],
) -> ScopeConfig {
    let allow_rule = ScopeRule {
        target: workspace_root.to_path_buf(),
        permission,
        recursive: true,
    };
    let deny = deny_write
        .iter()
        .cloned()
        .map(write_rule)
        .collect::<Vec<_>>();
    ScopeConfig {
        allow: vec![allow_rule],
        deny,
    }
}

fn workspace_worktree_delegation(workspace_root: &Path) -> ScopeConfig {
    ScopeConfig {
        allow: vec![
            read_rule(workspace_root.to_path_buf()),
            write_rule(workspace_root.join(".worktree")),
        ],
        deny: Vec::new(),
    }
}

fn append_missing_rules(target: &mut Vec<ScopeRule>, defaults: Vec<ScopeRule>) {
    for rule in defaults {
        if !target.contains(&rule) {
            target.push(rule);
        }
    }
}

fn apply_scope_launch_defaults(scope: &mut ScopeConfig, defaults: ScopeConfig) {
    // Profile resolution has already applied explicit profile/workspace override scope rules.
    // Launch policy contributes runtime defaults on top rather than replacing those grants.
    append_missing_rules(&mut scope.allow, defaults.allow);
    append_missing_rules(&mut scope.deny, defaults.deny);
}

fn apply_profile_launch_policy(
    manifest: &mut WorkerManifest,
    workspace_root: &Path,
    ticket_role: Option<&str>,
) -> Result<(), String> {
    let role = match ticket_role {
        Some(raw) => {
            Some(TicketRole::parse(raw).ok_or_else(|| format!("invalid ticket role `{raw}`"))?)
        }
        None => None,
    };
    match role {
        Some(TicketRole::Orchestrator) => {
            let default_scope = workspace_scope(workspace_root, Permission::Read, &[]);
            apply_scope_launch_defaults(&mut manifest.scope, default_scope);
            manifest.delegation_scope = workspace_worktree_delegation(workspace_root);
        }
        Some(TicketRole::Intake) | Some(TicketRole::Reviewer) => {
            let default_scope = workspace_scope(workspace_root, Permission::Read, &[]);
            apply_scope_launch_defaults(&mut manifest.scope, default_scope);
            manifest.delegation_scope = ScopeConfig::default();
        }
        Some(TicketRole::Coder) => {
            let default_scope = workspace_scope(workspace_root, Permission::Write, &[]);
            apply_scope_launch_defaults(&mut manifest.scope, default_scope);
            manifest.delegation_scope = ScopeConfig::default();
        }
        None => {
            let worktree_root = workspace_root.join(".worktree");
            let default_scope = workspace_scope(
                workspace_root,
                Permission::Write,
                std::slice::from_ref(&worktree_root),
            );
            apply_scope_launch_defaults(&mut manifest.scope, default_scope);
            manifest.delegation_scope = workspace_worktree_delegation(workspace_root);
        }
    }
    Ok(())
}

pub async fn run_cli() -> ExitCode {
    run_cli_from("yoi worker", std::env::args_os().skip(1)).await
}

pub async fn run_cli_from<I, T>(bin_name: &'static str, args: I) -> ExitCode
where
    I: IntoIterator<Item = T>,
    T: Into<OsString>,
{
    let cli = match parse_cli_from(bin_name, args) {
        Ok(cli) => cli,
        Err(err) => {
            let code = err.exit_code();
            if let Err(print_err) = err.print() {
                eprintln!("error: failed to write CLI error: {print_err}");
            }
            return exit_code_from_i32(code);
        }
    };

    run_cli_inner(cli).await
}

fn parse_cli_from<I, T>(bin_name: &'static str, args: I) -> Result<Cli, clap::Error>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString>,
{
    let argv = std::iter::once(OsString::from(bin_name))
        .chain(args.into_iter().map(Into::into))
        .collect::<Vec<_>>();
    let matches = Cli::command()
        .name(bin_name)
        .bin_name(bin_name)
        .try_get_matches_from(argv)?;
    Cli::from_arg_matches(&matches)
}

fn exit_code_from_i32(code: i32) -> ExitCode {
    match code {
        0 => ExitCode::SUCCESS,
        1 => ExitCode::FAILURE,
        code => ExitCode::from(code.clamp(0, u8::MAX as i32) as u8),
    }
}

async fn run_cli_inner(cli: Cli) -> ExitCode {
    let cwd = match std::env::current_dir() {
        Ok(path) => path,
        Err(e) => {
            eprintln!("error: failed to resolve current directory: {e}");
            return ExitCode::FAILURE;
        }
    };
    let workspace_root = match runtime_workspace_root(&cli) {
        Ok(root) => root,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };
    let (mut manifest, loader) = match resolve_manifest(&cli) {
        Ok(pair) => pair,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };
    // Initialize persistent store. `paths::sessions_dir()` only
    // returns None when none of YOI_HOME / YOI_DATA_DIR /
    // HOME is set — surface that as a hard error to match the
    // runtime-dir resolution below, rather than silently writing to a
    // relative path under cwd.
    let store_dir = match cli.store.clone() {
        Some(p) => p,
        None => match paths::sessions_dir() {
            Some(d) => d,
            None => {
                eprintln!(
                    "error: could not resolve sessions directory \
                     (set --store, YOI_HOME, YOI_DATA_DIR, or HOME)"
                );
                return ExitCode::FAILURE;
            }
        },
    };
    let session_store = match FsStore::new(&store_dir) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: failed to initialize session store at {store_dir:?}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let worker_metadata_dir = match paths::data_dir() {
        Some(data_dir) => data_dir.join("workers"),
        None => store_dir
            .parent()
            .map(|parent| parent.join("workers"))
            .unwrap_or_else(|| PathBuf::from("workers")),
    };
    let worker_metadata_store = match FsWorkerStore::new(&worker_metadata_dir) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: failed to initialize worker store at {worker_metadata_dir:?}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let store = CombinedStore::new(session_store, worker_metadata_store);
    let filesystem_authority =
        WorkerFilesystemAuthority::local(workspace_root.clone(), cwd.clone());
    let workspace_context = runtime_workspace_context(&workspace_root);

    let mut worker = if cli.adopt {
        let callback = match cli.callback.clone() {
            Some(p) => p,
            None => {
                eprintln!("error: --adopt requires --callback");
                return ExitCode::FAILURE;
            }
        };
        match Worker::from_manifest_spawned_with_context(
            manifest,
            store,
            loader,
            callback,
            workspace_context.clone(),
            filesystem_authority.clone(),
        )
        .await
        {
            Ok(p) => p,
            Err(e) => {
                eprintln!("error: failed to create spawned worker: {e}");
                return ExitCode::FAILURE;
            }
        }
    } else if let Some(source_segment_id) = cli.session {
        let source_session_id = match store.lookup_session_of(source_segment_id) {
            Ok(Some(sid)) => sid,
            Ok(None) => {
                eprintln!(
                    "error: --session {source_segment_id}: segment is not registered to any session"
                );
                return ExitCode::FAILURE;
            }
            Err(e) => {
                eprintln!("error: lookup_session_of failed: {e}");
                return ExitCode::FAILURE;
            }
        };
        match Worker::restore_from_manifest_with_context(
            source_session_id,
            source_segment_id,
            manifest,
            store,
            loader,
            workspace_context.clone(),
            filesystem_authority.clone(),
        )
        .await
        {
            Ok(p) => p,
            Err(e) => {
                eprintln!("error: failed to restore worker: {e}");
                return ExitCode::FAILURE;
            }
        }
    } else if let Some(worker_name) = cli.worker.as_deref() {
        manifest.worker.name = worker_name.to_string();
        match store.read_by_name(worker_name) {
            Ok(Some(_)) => {
                match Worker::restore_from_worker_metadata_with_context(
                    worker_name,
                    manifest,
                    store,
                    loader,
                    workspace_context.clone(),
                    filesystem_authority.clone(),
                )
                .await
                {
                    Ok(p) => p,
                    Err(e) => {
                        eprintln!("error: failed to restore worker {worker_name}: {e}");
                        return ExitCode::FAILURE;
                    }
                }
            }
            Ok(None) if cli.require_worker_state => {
                eprintln!("error: worker state missing for {worker_name}");
                return ExitCode::FAILURE;
            }
            Ok(None) => {
                match Worker::from_manifest_with_context(
                    manifest,
                    store,
                    loader,
                    workspace_context.clone(),
                    filesystem_authority.clone(),
                )
                .await
                {
                    Ok(p) => p,
                    Err(e) => {
                        eprintln!("error: failed to create worker {worker_name}: {e}");
                        return ExitCode::FAILURE;
                    }
                }
            }
            Err(e) => {
                eprintln!("error: failed to read worker state for {worker_name}: {e}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        match Worker::from_manifest_with_context(
            manifest,
            store,
            loader,
            workspace_context.clone(),
            filesystem_authority.clone(),
        )
        .await
        {
            Ok(p) => p,
            Err(e) => {
                eprintln!("error: failed to create worker: {e}");
                return ExitCode::FAILURE;
            }
        }
    };
    if let Some(role) = cli.ticket_role.clone() {
        if TicketRole::parse(&role).is_none() {
            eprintln!("error: invalid --ticket-role {role:?}");
            return ExitCode::FAILURE;
        }
        worker.set_runtime_ticket_role(Some(role));
    }
    let worker_name = worker.manifest().worker.name.clone();
    // Spawn the controller (starts socket server)
    let runtime_base = match paths::runtime_dir() {
        Some(d) => d,
        None => {
            eprintln!(
                "error: could not resolve runtime directory \
                 (set YOI_HOME, YOI_RUNTIME_DIR, XDG_RUNTIME_DIR, or HOME)"
            );
            return ExitCode::FAILURE;
        }
    };
    let (handle, shutdown_rx) = match WorkerController::spawn(worker, &runtime_base).await {
        Ok(pair) => pair,
        Err(e) => {
            eprintln!("error: failed to start worker controller: {e}");
            return ExitCode::FAILURE;
        }
    };

    let socket_path = handle.runtime_dir.socket_path();
    // Machine-readable ready line for parents that spawned this Worker
    // (e.g. the TUI's interactive `spawn` flow). Tab-separated so a
    // worker name with spaces still parses cleanly. Emit before the
    // human line so a stderr-watching parent sees it first.
    eprintln!("YOI-READY\t{worker_name}\t{}", socket_path.display());
    eprintln!("worker: {worker_name} listening on {:?}", socket_path);

    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            eprintln!("worker: {worker_name} shutting down (signal)");
        }
        _ = shutdown_rx => {
            eprintln!("worker: {worker_name} shutting down (client request)");
        }
    }

    drop(handle);
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, contents).unwrap();
    }

    fn manifest_toml(name: &str, scope: &Path) -> String {
        format!(
            r#"
[worker]
name = "{name}"

[model]
scheme = "anthropic"
model_id = "test-model"

[engine]

[[scope.allow]]
target = "{scope}"
permission = "write"
"#,
            scope = scope.display()
        )
    }

    fn scope_rule(target: &Path, permission: Permission) -> ScopeRule {
        ScopeRule {
            target: target.to_path_buf(),
            permission,
            recursive: true,
        }
    }

    fn assert_scope_contains(rules: &[ScopeRule], target: &Path, permission: Permission) {
        let expected = scope_rule(target, permission);
        assert!(
            rules.contains(&expected),
            "expected scope rules to contain {expected:?}; got {rules:?}"
        );
    }

    #[test]
    fn user_manifest_flag_is_not_accepted() {
        let err =
            Cli::try_parse_from(["yoi worker", "--user-manifest", "manifest.toml"]).unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::UnknownArgument);
    }

    #[test]
    fn subcommand_help_uses_yoi_worker_invocation() {
        let err = parse_cli_from("yoi worker", ["--help"]).unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::DisplayHelp);
        let help = err.to_string();
        assert!(help.contains("Usage: yoi worker"), "{help}");
        assert!(help.contains("--worker <NAME>"), "{help}");
    }

    #[test]
    fn manifest_conflicts_with_project() {
        let project_err = Cli::try_parse_from([
            "yoi worker",
            "--manifest",
            "manifest.toml",
            "--project",
            ".",
        ])
        .unwrap_err();
        assert_eq!(project_err.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn overlay_flag_is_not_accepted() {
        let err =
            Cli::try_parse_from(["yoi worker", "--overlay", "worker.name = 'x'"]).unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::UnknownArgument);
    }

    #[test]
    fn manifest_loads_single_file_without_user_or_workspace_prompt_loader() {
        let tmp = TempDir::new().unwrap();
        let manifest = tmp.path().join("manifest.toml");
        write(&manifest, &manifest_toml("single", tmp.path()));
        let cli =
            Cli::try_parse_from(["yoi worker", "--manifest", manifest.to_str().unwrap()]).unwrap();

        let (manifest, loader) = resolve_manifest(&cli).unwrap();

        assert_eq!(manifest.worker.name, "single");
        assert!(loader.user_dir().is_none());
        assert!(loader.workspace_dir().is_none());
    }

    #[test]
    fn manifest_mode_does_not_apply_workspace_local_override() {
        let tmp = TempDir::new().unwrap();
        let yoi_dir = tmp.path().join(".yoi");
        std::fs::create_dir_all(&yoi_dir).unwrap();
        write(
            &yoi_dir.join("override.local.toml"),
            r#"
[worker]
name = "from-local-override"
[engine]
language = "override"
"#,
        );
        let manifest_path = tmp.path().join("manifest.toml");
        write(
            &manifest_path,
            &format!(
                r#"
[worker]
name = "from-single-file"

[model]
scheme = "anthropic"
model_id = "test-model"

[engine]
language = "manifest"

[[scope.allow]]
target = "{}"
permission = "write"
"#,
                tmp.path().display()
            ),
        );

        let cli =
            Cli::try_parse_from(["yoi worker", "--manifest", manifest_path.to_str().unwrap()])
                .unwrap();
        let (manifest, _loader) = resolve_manifest(&cli).unwrap();

        assert_eq!(manifest.worker.name, "from-single-file");
        assert_eq!(manifest.engine.language, "manifest");
    }

    #[test]
    fn profile_launch_preserves_workspace_override_scope_allow_in_final_manifest() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().join("runtime-workspace");
        let external = tmp.path().join("external-readable");
        let yoi_dir = workspace.join(".yoi");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(&external).unwrap();
        std::fs::create_dir_all(&yoi_dir).unwrap();
        write(
            &yoi_dir.join("override.local.toml"),
            &format!(
                r#"
[[scope.allow]]
target = "{}"
permission = "read"
recursive = true
"#,
                external.display()
            ),
        );
        let profile = tmp.path().join("profile.toml");
        write(
            &profile,
            r#"
slug = "override-scope"

[model]
scheme = "anthropic"
model_id = "test-model"
"#,
        );
        let cli = Cli::try_parse_from([
            "yoi worker",
            "--workspace",
            workspace.to_str().unwrap(),
            "--profile",
            profile.to_str().unwrap(),
        ])
        .unwrap();

        let (manifest, _loader) = resolve_manifest(&cli).unwrap();
        let snapshot = serde_json::to_value(&manifest).unwrap();
        let snapshot_scope: ScopeConfig =
            serde_json::from_value(snapshot["scope"].clone()).unwrap();

        assert_scope_contains(&manifest.scope.allow, &external, Permission::Read);
        assert_scope_contains(&manifest.scope.allow, &workspace, Permission::Write);
        assert_scope_contains(
            &manifest.scope.deny,
            &workspace.join(".worktree"),
            Permission::Write,
        );
        assert_scope_contains(&snapshot_scope.allow, &external, Permission::Read);
        assert_scope_contains(&snapshot_scope.allow, &workspace, Permission::Write);
        assert_scope_contains(
            &snapshot_scope.deny,
            &workspace.join(".worktree"),
            Permission::Write,
        );
    }

    #[test]
    fn profile_uses_selected_profile() {
        let tmp = TempDir::new().unwrap();
        let profile = tmp.path().join("profile.toml");
        let cli = Cli::try_parse_from([
            "yoi worker",
            "--profile",
            profile.to_str().unwrap(),
            "--worker",
            "from-profile-name",
        ])
        .unwrap();
        let mut called = false;

        let (manifest, loader) =
            resolve_manifest_with_profile_loader(&cli, |selector, _workspace_root, worker_name| {
                called = true;
                assert_eq!(selector, &ProfileSelector::path(profile.clone()));
                assert_eq!(worker_name, "from-profile-name");
                let mut manifest =
                    WorkerManifest::from_toml(&manifest_toml("from-profile", tmp.path())).unwrap();
                manifest.worker.name = worker_name.to_string();
                Ok((manifest, PromptLoader::builtins_only()))
            })
            .unwrap();

        assert!(called);
        assert_eq!(manifest.worker.name, "from-profile-name");
        assert!(loader.user_dir().is_none());
        assert!(loader.workspace_dir().is_none());
    }

    #[test]
    fn profile_accepts_source_qualified_discovered_name() {
        let tmp = TempDir::new().unwrap();
        let cli = Cli::try_parse_from([
            "yoi worker",
            "--profile",
            "project:coder",
            "--worker",
            "from-profile-name",
        ])
        .unwrap();
        let mut called = false;

        let (manifest, _loader) =
            resolve_manifest_with_profile_loader(&cli, |selector, _workspace_root, worker_name| {
                called = true;
                assert_eq!(
                    selector,
                    &ProfileSelector::source_named(
                        manifest::ProfileRegistrySource::Project,
                        "coder"
                    )
                );
                let mut manifest =
                    WorkerManifest::from_toml(&manifest_toml("from-profile", tmp.path())).unwrap();
                manifest.worker.name = worker_name.to_string();
                Ok((manifest, PromptLoader::builtins_only()))
            })
            .unwrap();

        assert!(called);
        assert_eq!(manifest.worker.name, "from-profile-name");
    }

    #[test]
    fn profile_without_explicit_worker_uses_workspace_basename_not_selector() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().join("other-workspace");
        std::fs::create_dir(&workspace).unwrap();
        let cli = Cli::try_parse_from([
            "yoi worker",
            "--workspace",
            workspace.to_str().unwrap(),
            "--profile",
            "project:companion",
        ])
        .unwrap();
        let mut called = false;

        let (manifest, _loader) =
            resolve_manifest_with_profile_loader(&cli, |selector, workspace_root, worker_name| {
                called = true;
                assert_eq!(
                    selector,
                    &ProfileSelector::source_named(
                        manifest::ProfileRegistrySource::Project,
                        "companion"
                    )
                );
                assert_eq!(workspace_root, workspace.as_path());
                assert_eq!(worker_name, "other-workspace");
                let mut manifest =
                    WorkerManifest::from_toml(&manifest_toml("profile-selector-name", tmp.path()))
                        .unwrap();
                manifest.worker.name = worker_name.to_string();
                Ok((manifest, PromptLoader::builtins_only()))
            })
            .unwrap();

        assert!(called);
        assert_eq!(manifest.worker.name, "other-workspace");
    }

    #[test]
    fn normal_startup_uses_default_profile() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().join("runtime-workspace");
        std::fs::create_dir(&workspace).unwrap();
        let cli = Cli::try_parse_from(["yoi worker", "--workspace", workspace.to_str().unwrap()])
            .unwrap();
        let mut called = false;

        let (manifest, _loader) =
            resolve_manifest_with_profile_loader(&cli, |selector, _workspace_root, worker_name| {
                called = true;
                assert_eq!(selector, &ProfileSelector::Default);
                assert_eq!(worker_name, "runtime-workspace");
                let mut manifest =
                    WorkerManifest::from_toml(&manifest_toml("from-default-profile", tmp.path()))
                        .unwrap();
                manifest.worker.name = worker_name.to_string();
                Ok((manifest, PromptLoader::builtins_only()))
            })
            .unwrap();

        assert!(called);
        assert_eq!(manifest.worker.name, "runtime-workspace");
        assert_eq!(manifest.scope.allow.len(), 2);
        assert_scope_contains(&manifest.scope.allow, tmp.path(), Permission::Write);
        assert_scope_contains(&manifest.scope.allow, &workspace, Permission::Write);
        assert_eq!(manifest.scope.deny.len(), 1);
        assert_scope_contains(
            &manifest.scope.deny,
            &tmp.path().join("runtime-workspace/.worktree"),
            Permission::Write,
        );
        assert_eq!(manifest.delegation_scope.allow.len(), 2);
        assert_eq!(
            manifest.delegation_scope.allow[0].target,
            tmp.path().join("runtime-workspace")
        );
        assert_eq!(
            manifest.delegation_scope.allow[0].permission,
            Permission::Read
        );
        assert_eq!(
            manifest.delegation_scope.allow[1].target,
            tmp.path().join("runtime-workspace/.worktree")
        );
        assert_eq!(
            manifest.delegation_scope.allow[1].permission,
            Permission::Write
        );
    }

    #[test]
    fn orchestrator_profile_launch_gets_read_root_and_worktree_delegation_from_launch_policy() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().join("original-workspace");
        std::fs::create_dir(&workspace).unwrap();
        let cli = Cli::try_parse_from([
            "yoi worker",
            "--workspace",
            workspace.to_str().unwrap(),
            "--profile",
            "builtin:orchestrator",
            "--ticket-role",
            "orchestrator",
        ])
        .unwrap();

        let (manifest, _loader) =
            resolve_manifest_with_profile_loader(&cli, |selector, _workspace_root, worker_name| {
                assert_eq!(
                    selector,
                    &ProfileSelector::source_named(
                        manifest::ProfileRegistrySource::Builtin,
                        "orchestrator"
                    )
                );
                let mut manifest = WorkerManifest::from_toml(&manifest_toml(
                    "from-orchestrator-profile",
                    tmp.path(),
                ))
                .unwrap();
                manifest.worker.name = worker_name.to_string();
                Ok((manifest, PromptLoader::builtins_only()))
            })
            .unwrap();

        assert_eq!(manifest.scope.allow.len(), 2);
        assert_scope_contains(&manifest.scope.allow, tmp.path(), Permission::Write);
        assert_scope_contains(&manifest.scope.allow, &workspace, Permission::Read);
        assert!(manifest.scope.deny.is_empty());
        assert_eq!(manifest.delegation_scope.allow.len(), 2);
        assert_eq!(
            manifest.delegation_scope.allow[0].target,
            tmp.path().join("original-workspace")
        );
        assert_eq!(
            manifest.delegation_scope.allow[0].permission,
            Permission::Read
        );
        assert_eq!(
            manifest.delegation_scope.allow[1].target,
            tmp.path().join("original-workspace/.worktree")
        );
        assert_eq!(
            manifest.delegation_scope.allow[1].permission,
            Permission::Write
        );
        assert!(
            !manifest
                .delegation_scope
                .allow
                .iter()
                .any(|rule| rule.target == tmp.path().join("original-workspace")
                    && rule.permission == Permission::Write)
        );
    }

    #[test]
    fn project_flag_no_longer_enables_ambient_manifest_cascade() {
        let cli = Cli::try_parse_from(["yoi worker", "--project", "."]).unwrap();
        let err = resolve_manifest_with_profile_loader(&cli, |_, _, _| {
            panic!("default profile loader must not run when deprecated --project is present")
        })
        .unwrap_err();
        assert!(err.contains("--project is no longer supported"));
    }

    #[test]
    fn worker_flag_is_runtime_identity_for_session_restore() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().join("explicit-workspace");
        let store = tmp.path().join("sessions");
        std::fs::create_dir(&workspace).unwrap();
        let segment_id = session_store::new_segment_id().to_string();
        let cli = Cli::try_parse_from([
            "yoi worker",
            "--workspace",
            workspace.to_str().unwrap(),
            "--session",
            &segment_id,
            "--worker",
            "explicit-name",
            "--store",
            store.to_str().unwrap(),
        ])
        .unwrap();

        assert_eq!(cli.session.unwrap().to_string(), segment_id);
        assert_eq!(cli.worker.as_deref(), Some("explicit-name"));
        assert_eq!(runtime_worker_name(&cli, &workspace), "explicit-name");
    }

    #[test]
    fn worker_flag_sets_requested_name_after_manifest_resolution() {
        let tmp = TempDir::new().unwrap();
        let manifest = tmp.path().join("manifest.toml");
        write(&manifest, &manifest_toml("from-file", tmp.path()));
        let cli = Cli::try_parse_from([
            "yoi worker",
            "--manifest",
            manifest.to_str().unwrap(),
            "--worker",
            "from-flag",
        ])
        .unwrap();

        let (manifest, _loader) = resolve_manifest(&cli).unwrap();

        assert_eq!(manifest.worker.name, "from-flag");
    }

    #[test]
    fn worker_flag_supplies_missing_name_for_single_manifest() {
        let tmp = TempDir::new().unwrap();
        let manifest = tmp.path().join("manifest.toml");
        write(
            &manifest,
            r#"
[engine]

[model]
scheme = "anthropic"
model_id = "test-model"

[[scope.allow]]
target = "."
permission = "write"
"#,
        );
        let cli = Cli::try_parse_from([
            "yoi worker",
            "--manifest",
            manifest.to_str().unwrap(),
            "--worker",
            "from-flag",
        ])
        .unwrap();

        let (manifest, _loader) = resolve_manifest(&cli).unwrap();

        assert_eq!(manifest.worker.name, "from-flag");
        assert_eq!(manifest.scope.allow[0].target, tmp.path());
    }

    #[test]
    fn worker_flag_with_no_manifest_creates_from_default_profile_with_typed_name() {
        let tmp = TempDir::new().unwrap();
        let cli = Cli::try_parse_from(["yoi worker", "--worker", "agent"]).unwrap();
        let mut called = false;

        let (manifest, _loader) =
            resolve_manifest_with_profile_loader(&cli, |selector, _workspace_root, worker_name| {
                called = true;
                assert_eq!(selector, &ProfileSelector::Default);
                assert_eq!(worker_name, "agent");
                let mut manifest =
                    WorkerManifest::from_toml(&manifest_toml("from-default-profile", tmp.path()))
                        .unwrap();
                manifest.worker.name = worker_name.to_string();
                Ok((manifest, PromptLoader::builtins_only()))
            })
            .unwrap();

        assert!(called);
        assert_eq!(manifest.worker.name, "agent");
    }

    #[test]
    fn profile_conflicts_with_manifest_and_restore_modes() {
        let segment_id = session_store::new_segment_id().to_string();
        for args in [
            vec!["yoi worker", "--profile", "p.toml", "--manifest", "m.toml"],
            vec![
                "yoi worker",
                "--profile",
                "p.toml",
                "--session",
                &segment_id,
            ],
        ] {
            let err = Cli::try_parse_from(args).unwrap_err();
            assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
        }
    }

    #[test]
    fn profile_and_worker_are_independent_startup_inputs() {
        let cli = Cli::try_parse_from(["yoi worker", "--profile", "p.toml", "--worker", "agent"])
            .unwrap();
        assert_eq!(cli.profile.as_deref(), Some("p.toml"));
        assert_eq!(cli.worker.as_deref(), Some("agent"));
    }

    #[test]
    fn old_session_worker_name_identity_alias_is_rejected() {
        let segment_id = session_store::new_segment_id().to_string();
        let err = Cli::try_parse_from([
            "yoi worker",
            "--session",
            &segment_id,
            "--session-worker-name",
            "agent",
        ])
        .unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::UnknownArgument);
    }

    #[test]
    fn removed_profile_worker_name_alias_is_rejected() {
        let err =
            Cli::try_parse_from(["yoi worker", "--profile-worker-name", "agent"]).unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::UnknownArgument);
    }

    #[test]
    fn manifest_mode_loads_single_file_with_minimal_prompt_loader() {
        let tmp = TempDir::new().unwrap();
        let single_manifest = tmp.path().join("single.toml");
        write(&single_manifest, &manifest_toml("single-file", tmp.path()));
        std::fs::create_dir_all(tmp.path().join("prompts")).unwrap();
        std::fs::create_dir_all(tmp.path().join(".yoi").join("prompts")).unwrap();
        let cli = Cli::try_parse_from([
            "yoi worker",
            "--manifest",
            single_manifest.to_str().unwrap(),
        ])
        .unwrap();

        let (manifest, loader) = resolve_manifest(&cli).unwrap();

        assert_eq!(manifest.worker.name, "single-file");
        assert!(loader.user_dir().is_none());
        assert!(loader.workspace_dir().is_none());
        assert!(loader.user_pack_file().is_none());
        assert!(loader.workspace_pack_file().is_none());
    }
}
