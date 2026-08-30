//! Ticket-role Worker launch planning and execution.
//!
//! This module keeps Ticket role configuration, generated first-run input, and
//! host-side Worker spawning behind the `client` crate so UI callers do not need to
//! depend on `worker` internals.

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use manifest::{ProfileDiscovery, ProfileResolveOptions, ProfileResolver, ProfileSelector};
use protocol::{ErrorCode, Event, InvokeKind, Method, Segment};
use thiserror::Error;
pub use ticket::config::TicketRole;
use ticket::config::{TicketConfig, TicketConfigError, TicketRoleLaunchConfigError};

use crate::{
    SpawnError, SpawnReady, WorkerClient, WorkerProcessLaunchConfig, WorkerProcessLaunchOptions,
    WorkerRuntimeCommand, spawn_worker_with_options,
};

const MAX_FIELD_CHARS: usize = 8_000;
const MAX_POD_NAME_CHARS: usize = 80;
const RUN_ACCEPTANCE_TIMEOUT: Duration = Duration::from_secs(10);
const PRE_RUN_ACTION_TIMEOUT: Duration = Duration::from_secs(5);

/// Ticket identifier carried by a role launch request.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TicketRef {
    pub id: Option<String>,
}

impl TicketRef {
    pub fn id(id: impl Into<String>) -> Self {
        Self {
            id: Some(id.into()),
        }
    }

    fn worker_name_seed(&self) -> Option<&str> {
        non_empty(self.id.as_deref())
    }

    fn append_submit_lines(&self, out: &mut String) {
        match non_empty(self.id.as_deref()) {
            None => out.push_str("Target Ticket: not specified\n"),
            Some(id) => {
                out.push_str("Target Ticket:\n");
                push_bounded_bullet(out, "id", id);
            }
        }
    }
}

/// Auditable panel handoff target included in a Ticket Intake launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketIntakeHandoff {
    pub workspace_orchestrator_worker: String,
    pub workspace_label: String,
}

impl TicketIntakeHandoff {
    pub fn new(
        workspace_orchestrator_worker: impl Into<String>,
        workspace_label: impl Into<String>,
    ) -> Self {
        Self {
            workspace_orchestrator_worker: workspace_orchestrator_worker.into(),
            workspace_label: workspace_label.into(),
        }
    }

    fn append_submit_lines(&self, out: &mut String) {
        out.push_str("\nPanel handoff:\n");
        push_bounded_bullet(out, "workspace", &self.workspace_label);
        push_bounded_bullet(
            out,
            "workspace_workspace_orchestrator_worker",
            &self.workspace_orchestrator_worker,
        );
    }
}

/// Typed input for constructing a Ticket role launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketRoleLaunchContext {
    pub workspace_root: PathBuf,
    pub cwd: Option<PathBuf>,
    pub original_workspace_root: Option<PathBuf>,
    pub target_workspace_root: Option<PathBuf>,
    pub role: TicketRole,
    pub worker_name: Option<String>,
    pub ticket: Option<TicketRef>,
    pub user_instruction: Option<String>,
    pub intake_handoff: Option<TicketIntakeHandoff>,
    pub ticket_record_language: Option<String>,
    pub intent_packet: Option<String>,
    pub worktree_path: Option<PathBuf>,
    pub branch: Option<String>,
    pub validation: Vec<String>,
    pub report_expectations: Vec<String>,
}

impl TicketRoleLaunchContext {
    pub fn new(workspace_root: impl Into<PathBuf>, role: TicketRole) -> Self {
        Self {
            workspace_root: workspace_root.into(),
            cwd: None,
            original_workspace_root: None,
            target_workspace_root: None,
            role,
            worker_name: None,
            ticket: None,
            user_instruction: None,
            intake_handoff: None,
            ticket_record_language: None,
            intent_packet: None,
            worktree_path: None,
            branch: None,
            validation: Vec::new(),
            report_expectations: Vec::new(),
        }
    }

    pub fn with_cwd(mut self, root: impl Into<PathBuf>) -> Self {
        self.cwd = Some(root.into());
        self
    }

    pub fn with_original_workspace_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.original_workspace_root = Some(root.into());
        self
    }

    pub fn with_target_workspace_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.target_workspace_root = Some(root.into());
        self
    }

    fn original_workspace_root(&self) -> &Path {
        self.original_workspace_root
            .as_deref()
            .unwrap_or(&self.workspace_root)
    }

    fn target_workspace_root(&self) -> &Path {
        self.target_workspace_root
            .as_deref()
            .unwrap_or_else(|| self.original_workspace_root())
    }

    fn implementation_worktree_root(&self) -> PathBuf {
        self.original_workspace_root().join(".worktree")
    }
}

/// Pure launch plan usable by TUI/CLI surfaces before executing the launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketRoleLaunchPlan {
    pub workspace_root: PathBuf,
    pub cwd: Option<PathBuf>,
    pub original_workspace_root: PathBuf,
    pub target_workspace_root: PathBuf,
    pub implementation_worktree_root: PathBuf,
    pub role: TicketRole,
    pub worker_name: String,
    pub profile: String,
    pub launch_prompt_ref: Option<String>,
    pub run_segments: Vec<Segment>,
}

impl TicketRoleLaunchPlan {
    pub fn run_method(&self) -> Method {
        Method::Run {
            input: self.run_segments.clone(),
        }
    }

    pub fn spawn_config(
        &self,
        runtime_command: WorkerRuntimeCommand,
    ) -> Result<WorkerProcessLaunchConfig, TicketRoleLaunchError> {
        if self.profile == "inherit" {
            return Err(TicketRoleLaunchError::UnsupportedInheritProfile);
        }
        Ok(WorkerProcessLaunchConfig {
            runtime_command,
            worker_name: self.worker_name.clone(),
            profile: Some(self.profile.clone()),
            workspace_root: self.workspace_root.clone(),
            cwd: self.cwd.clone(),
            resume_from: None,
        })
    }

    pub fn spawn_options(&self) -> WorkerProcessLaunchOptions {
        WorkerProcessLaunchOptions::default()
            .with_hidden_arg("--ticket-role", self.role.as_str().to_string())
    }
}

/// Result of executing a Ticket role launch.
#[derive(Debug, Clone)]
pub struct TicketRoleLaunchResult {
    pub plan: TicketRoleLaunchPlan,
    pub ready: SpawnReady,
    /// Evidence that the spawned worker accepted the initial Run request.
    /// This is intentionally distinct from process readiness: a socket
    /// snapshot only proves that the runtime is reachable, not that the
    /// worker operation was durably queued/started.
    pub acceptance_evidence: TicketRoleLaunchAcceptanceEvidence,
    pub pre_run_warnings: Vec<TicketRolePreRunWarning>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketRoleLaunchAcceptanceEvidence {
    pub worker_name: String,
    pub accepted_run_segments: usize,
    pub event: TicketRoleLaunchAcceptanceEvent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TicketRoleLaunchAcceptanceEvent {
    UserMessage,
    UserSendInvokeStart,
    TurnStart,
}

/// Non-fatal diagnostic produced by bounded pre-run launch actions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketRolePreRunWarning {
    pub message: String,
}

/// Optional bounded actions executed after spawn readiness and before the first Run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TicketRoleLaunchOptions {
    pub pre_run_peer_registrations: Vec<String>,
}

impl TicketRoleLaunchOptions {
    pub fn with_pre_run_peer_registration(mut self, worker_name: impl Into<String>) -> Self {
        self.pre_run_peer_registrations.push(worker_name.into());
        self
    }
}

#[derive(Debug, Error)]
pub enum TicketRoleLaunchError {
    #[error(transparent)]
    Config(#[from] TicketConfigError),
    #[error(transparent)]
    LaunchConfig(#[from] TicketRoleLaunchConfigError),
    #[error(
        "Ticket role `{role}` profile selector `{selector}` is not resolvable before launch: {message}. Configure `[ticket.roles.{role}].profile` with an executable concrete profile selector such as `builtin:companion` or a project/user profile"
    )]
    ProfileResolution {
        role: TicketRole,
        selector: String,
        message: String,
    },
    #[error("Ticket role Worker name must not be empty")]
    EmptyWorkerName,
    #[error(
        "Ticket role profile 'inherit' cannot be used for top-level launch execution; configure a concrete role profile selector"
    )]
    UnsupportedInheritProfile,
    #[error(transparent)]
    Spawn(#[from] SpawnError),
    #[error("failed to connect to spawned Ticket role Worker at {}: {source}", .socket_path.display())]
    Connect {
        socket_path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to send first run input to spawned Ticket role Worker: {source}")]
    SendRun {
        #[source]
        source: io::Error,
    },
    #[error("Ticket role Worker rejected first run input with {code:?}: {message}")]
    RunRejected { code: ErrorCode, message: String },
    #[error("Ticket role Worker closed before confirming first run acceptance")]
    RunAcceptanceClosed,
    #[error("timed out waiting for Ticket role Worker to confirm first run acceptance")]
    RunAcceptanceTimeout,
}

/// Load Ticket policy from `.yoi/workspace.toml` and construct a launch plan.
pub fn plan_ticket_role_launch(
    context: TicketRoleLaunchContext,
) -> Result<TicketRoleLaunchPlan, TicketRoleLaunchError> {
    let config = TicketConfig::load_workspace(&context.workspace_root)?;
    plan_ticket_role_launch_with_config(context, &config)
}

/// Construct a launch plan from an already-loaded Ticket config.
pub fn plan_ticket_role_launch_with_config(
    mut context: TicketRoleLaunchContext,
    config: &TicketConfig,
) -> Result<TicketRoleLaunchPlan, TicketRoleLaunchError> {
    if context.ticket_record_language.is_none() {
        context.ticket_record_language = config.ticket_record_language().map(str::to_string);
    }
    let role_config = config.role_launch_config(context.role)?;
    let profile = role_config.profile.as_str().to_string();
    let launch_prompt_ref = role_config
        .launch_prompt
        .as_ref()
        .map(|prompt| prompt.as_str().to_string());
    let worker_name = match context.worker_name.as_deref().map(str::trim) {
        Some("") => return Err(TicketRoleLaunchError::EmptyWorkerName),
        Some(name) => name.to_string(),
        None => default_worker_name(context.role, context.ticket.as_ref()),
    };
    validate_ticket_role_profile(
        context.role,
        &profile,
        &context.workspace_root,
        &worker_name,
    )?;
    let prompt = build_launch_prompt(&context);

    let original_workspace_root = context.original_workspace_root().to_path_buf();
    let target_workspace_root = context.target_workspace_root().to_path_buf();
    let implementation_worktree_root = context.implementation_worktree_root();

    Ok(TicketRoleLaunchPlan {
        workspace_root: context.workspace_root,
        cwd: context.cwd,
        original_workspace_root,
        target_workspace_root,
        implementation_worktree_root,
        role: context.role,
        worker_name,
        profile,
        launch_prompt_ref,
        run_segments: vec![Segment::Text {
            content: format!("\n\n{prompt}"),
        }],
    })
}

fn validate_ticket_role_profile(
    role: TicketRole,
    profile: &str,
    workspace_root: &std::path::Path,
    worker_name: &str,
) -> Result<(), TicketRoleLaunchError> {
    let selector = ProfileSelector::parse_cli(profile);
    let registry = ProfileDiscovery::for_cwd(workspace_root)
        .discover()
        .map_err(|source| TicketRoleLaunchError::ProfileResolution {
            role,
            selector: profile.to_string(),
            message: source.to_string(),
        })?;
    ProfileResolver::new()
        .with_workspace_base(workspace_root)
        .resolve_from_registry(
            &selector,
            &registry,
            ProfileResolveOptions::with_worker_name(worker_name),
        )
        .map(|_| ())
        .map_err(|source| TicketRoleLaunchError::ProfileResolution {
            role,
            selector: profile.to_string(),
            message: source.to_string(),
        })
}

/// Spawn the Worker, connect to its socket, send the first `Method::Run` input,
/// and wait for bounded acceptance evidence from the Worker event stream.
pub async fn launch_ticket_role_worker<F>(
    context: TicketRoleLaunchContext,
    runtime_command: WorkerRuntimeCommand,
    progress: F,
) -> Result<TicketRoleLaunchResult, TicketRoleLaunchError>
where
    F: FnMut(&str),
{
    launch_ticket_role_worker_with_options(
        context,
        runtime_command,
        progress,
        TicketRoleLaunchOptions::default(),
    )
    .await
}

/// Spawn the Worker, run bounded pre-run launch options while it is still idle,
/// then send the first `Method::Run` input and wait for acceptance evidence.
pub async fn launch_ticket_role_worker_with_options<F>(
    context: TicketRoleLaunchContext,
    runtime_command: WorkerRuntimeCommand,
    progress: F,
    options: TicketRoleLaunchOptions,
) -> Result<TicketRoleLaunchResult, TicketRoleLaunchError>
where
    F: FnMut(&str),
{
    let plan = plan_ticket_role_launch(context)?;
    let spawn_config = plan.spawn_config(runtime_command)?;
    let spawn_options = plan.spawn_options();
    let ready = spawn_worker_with_options(spawn_config, spawn_options, progress).await?;
    let mut client = WorkerClient::connect(&ready.socket_path)
        .await
        .map_err(|source| TicketRoleLaunchError::Connect {
            socket_path: ready.socket_path.clone(),
            source,
        })?;
    let pre_run_warnings = run_pre_run_options_then_send_run(&mut client, &plan, &options).await?;
    let acceptance_event =
        wait_for_run_acceptance(&mut client, &plan.run_segments, RUN_ACCEPTANCE_TIMEOUT).await?;
    let acceptance_evidence = TicketRoleLaunchAcceptanceEvidence {
        worker_name: ready.worker_name.clone(),
        accepted_run_segments: plan.run_segments.len(),
        event: acceptance_event,
    };
    Ok(TicketRoleLaunchResult {
        plan,
        ready,
        acceptance_evidence,
        pre_run_warnings,
    })
}

async fn run_pre_run_options_then_send_run(
    client: &mut WorkerClient,
    plan: &TicketRoleLaunchPlan,
    options: &TicketRoleLaunchOptions,
) -> Result<Vec<TicketRolePreRunWarning>, TicketRoleLaunchError> {
    let pre_run_warnings = perform_pre_run_peer_registrations(
        client,
        &options.pre_run_peer_registrations,
        PRE_RUN_ACTION_TIMEOUT,
    )
    .await;
    client
        .send(&plan.run_method())
        .await
        .map_err(|source| TicketRoleLaunchError::SendRun { source })?;
    Ok(pre_run_warnings)
}

async fn perform_pre_run_peer_registrations(
    client: &mut WorkerClient,
    peer_names: &[String],
    timeout: Duration,
) -> Vec<TicketRolePreRunWarning> {
    let mut warnings = Vec::new();
    for peer_name in peer_names {
        if peer_name.trim().is_empty() {
            warnings.push(TicketRolePreRunWarning {
                message: "pre-run peer registration skipped: peer Worker name is empty".to_string(),
            });
            continue;
        }
        if let Err(message) = pre_run_register_peer(client, peer_name, timeout).await {
            warnings.push(TicketRolePreRunWarning { message });
        }
    }
    warnings
}

async fn pre_run_register_peer(
    client: &mut WorkerClient,
    peer_name: &str,
    timeout: Duration,
) -> Result<(), String> {
    if let Err(source) = client
        .send(&Method::RegisterPeer {
            name: peer_name.to_string(),
        })
        .await
    {
        return Err(format!(
            "pre-run peer registration for {peer_name} failed while sending request: {source}"
        ));
    }

    let wait = async {
        loop {
            let Some(event) = client.next_event().await else {
                return Err(format!(
                    "pre-run peer registration for {peer_name} failed: connection closed before response"
                ));
            };
            match event {
                Event::PeerRegistered { .. } => return Ok(()),
                Event::Error { code, message } => {
                    return Err(format!(
                        "pre-run peer registration for {peer_name} failed with {code:?}: {message}"
                    ));
                }
                _ => {}
            }
        }
    };

    tokio::time::timeout(timeout, wait)
        .await
        .unwrap_or_else(|_| {
            Err(format!(
                "pre-run peer registration for {peer_name} timed out before first Run"
            ))
        })
}

async fn wait_for_run_acceptance(
    client: &mut WorkerClient,
    expected_segments: &[Segment],
    timeout: Duration,
) -> Result<TicketRoleLaunchAcceptanceEvent, TicketRoleLaunchError> {
    let wait = async {
        loop {
            let Some(event) = client.next_event().await else {
                return Err(TicketRoleLaunchError::RunAcceptanceClosed);
            };
            match event {
                Event::UserMessage { segments } if segments == expected_segments => {
                    return Ok(TicketRoleLaunchAcceptanceEvent::UserMessage);
                }
                Event::InvokeStart {
                    kind: InvokeKind::UserSend,
                } => return Ok(TicketRoleLaunchAcceptanceEvent::UserSendInvokeStart),
                Event::TurnStart { .. } => return Ok(TicketRoleLaunchAcceptanceEvent::TurnStart),
                Event::Error { code, message } => {
                    return Err(TicketRoleLaunchError::RunRejected { code, message });
                }
                _ => {}
            }
        }
    };

    tokio::time::timeout(timeout, wait)
        .await
        .map_err(|_| TicketRoleLaunchError::RunAcceptanceTimeout)?
}

fn build_launch_prompt(context: &TicketRoleLaunchContext) -> String {
    let mut out = String::new();

    if let Some(ticket) = &context.ticket {
        ticket.append_submit_lines(&mut out);
    } else {
        out.push_str("Target Ticket: not specified\n");
    }

    if let Some(instruction) = non_empty(context.user_instruction.as_deref()) {
        push_bounded_section(&mut out, "Action instruction", instruction);
    }

    if let Some(handoff) = &context.intake_handoff {
        handoff.append_submit_lines(&mut out);
    }

    if let Some(intent_packet) = non_empty(context.intent_packet.as_deref()) {
        push_bounded_section(&mut out, "Intent packet", intent_packet);
    }

    append_operation_targets(&mut out, context);

    if context.worktree_path.is_some() || non_empty(context.branch.as_deref()).is_some() {
        out.push_str("\nWorktree target:\n");
        if let Some(path) = &context.worktree_path {
            push_bounded_bullet(&mut out, "path", &path.display().to_string());
        }
        if let Some(branch) = non_empty(context.branch.as_deref()) {
            push_bounded_bullet(&mut out, "branch", branch);
        }
    }

    if !context.validation.is_empty() {
        push_bounded_list(&mut out, "Validation expectations", &context.validation);
    }
    if !context.report_expectations.is_empty() {
        push_bounded_list(
            &mut out,
            "Report expectations",
            &context.report_expectations,
        );
    }

    out
}

fn append_operation_targets(out: &mut String, context: &TicketRoleLaunchContext) {
    if context.role != TicketRole::Orchestrator {
        return;
    }

    out.push_str("\nOrchestrator operation targets:\n");
    push_bounded_bullet(
        out,
        "implementation_worktree_root",
        &context.implementation_worktree_root().display().to_string(),
    );
}

fn default_worker_name(role: TicketRole, ticket: Option<&TicketRef>) -> String {
    let mut name = format!("ticket-{}", role.as_str());
    if let Some(seed) = ticket.and_then(TicketRef::worker_name_seed) {
        let suffix = sanitise_worker_name_component(seed);
        if !suffix.is_empty() {
            name.push('-');
            name.push_str(&suffix);
        }
    }
    name.chars().take(MAX_POD_NAME_CHARS).collect()
}

fn sanitise_worker_name_component(value: &str) -> String {
    let mut out = String::new();
    let mut last_was_dash = false;
    for ch in value.trim().chars() {
        let mapped = if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.') {
            Some(ch.to_ascii_lowercase())
        } else if ch == '-' || ch.is_whitespace() {
            Some('-')
        } else {
            Some('-')
        };
        if let Some(ch) = mapped {
            if ch == '-' {
                if !last_was_dash && !out.is_empty() {
                    out.push(ch);
                }
                last_was_dash = true;
            } else {
                out.push(ch);
                last_was_dash = false;
            }
        }
    }
    out.trim_matches(|ch| matches!(ch, '-' | '_' | '.'))
        .chars()
        .take(MAX_POD_NAME_CHARS)
        .collect()
}

fn push_bounded_bullet(out: &mut String, label: &str, value: &str) {
    out.push_str("- ");
    out.push_str(label);
    out.push_str(": ");
    out.push_str(&bounded(value));
    out.push('\n');
}

fn push_bounded_section(out: &mut String, label: &str, value: &str) {
    out.push('\n');
    out.push_str(label);
    out.push_str(":\n");
    out.push_str(&bounded(value));
    out.push('\n');
}

fn push_bounded_list(out: &mut String, label: &str, values: &[String]) {
    out.push('\n');
    out.push_str(label);
    out.push_str(":\n");
    for value in values
        .iter()
        .filter_map(|value| non_empty(Some(value.as_str())))
    {
        out.push_str("- ");
        out.push_str(&bounded(value));
        out.push('\n');
    }
}

fn bounded(value: &str) -> String {
    let trimmed = value.trim();
    let mut out: String = trimmed.chars().take(MAX_FIELD_CHARS).collect();
    if trimmed.chars().count() > MAX_FIELD_CHARS {
        out.push_str("\n[truncated]");
    }
    out
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::{Greeting, WorkerStatus};
    use tempfile::TempDir;
    use tokio::io::{AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};
    use tokio::net::UnixListener;

    fn write_config(workspace: &std::path::Path, content: &str) {
        let dir = workspace.join(".yoi");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("workspace.toml"), content).unwrap();
    }

    fn write_builtin_role_config(workspace: &std::path::Path, roles: &[TicketRole]) {
        let mut config = String::from("[ticket]\n");
        for role in roles {
            config.push_str(&format!(
                "\n[ticket.roles.{role}]\nprofile = \"builtin:companion\"\n"
            ));
        }
        write_config(workspace, &config);
    }

    fn text_segment(plan: &TicketRoleLaunchPlan) -> &str {
        plan.run_segments
            .iter()
            .find_map(|segment| match segment {
                Segment::Text { content } => Some(content.as_str()),
                _ => None,
            })
            .expect("expected text segment")
    }

    async fn write_test_event<W>(writer: &mut W, event: Event)
    where
        W: AsyncWrite + Unpin,
    {
        writer
            .write_all(serde_json::to_string(&event).unwrap().as_bytes())
            .await
            .unwrap();
        writer.write_all(b"\n").await.unwrap();
    }

    fn test_snapshot() -> Event {
        Event::Snapshot {
            session: protocol::SessionSnapshot { entries: vec![] },
            greeting: Greeting {
                worker_name: "ticket-intake".to_string(),
                cwd: "/tmp".to_string(),
                provider: "test".to_string(),
                model: "test".to_string(),
                scope_summary: "test".to_string(),
                tools: vec![],
                context_window: 0,
                context_tokens: 0,
            },
            status: WorkerStatus::Idle,
            in_flight: protocol::InFlightSnapshot::default(),
            internal_workers: vec![],
        }
    }

    fn test_launch_plan(workspace: &std::path::Path) -> TicketRoleLaunchPlan {
        TicketRoleLaunchPlan {
            workspace_root: workspace.to_path_buf(),
            cwd: None,
            original_workspace_root: workspace.to_path_buf(),
            target_workspace_root: workspace.to_path_buf(),
            implementation_worktree_root: workspace.join(".worktree"),
            role: TicketRole::Intake,
            worker_name: "ticket-intake".to_string(),
            profile: "project:intake".to_string(),
            launch_prompt_ref: None,
            run_segments: vec![Segment::Text {
                content: "intake request".to_string(),
            }],
        }
    }

    #[tokio::test]
    async fn pre_run_peer_registration_is_sent_before_first_run_submission() {
        let temp = TempDir::new().unwrap();
        let socket_path = temp.path().join("worker.sock");
        let listener = UnixListener::bind(&socket_path).unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (reader, mut writer) = stream.into_split();
            write_test_event(&mut writer, test_snapshot()).await;
            let mut reader = BufReader::new(reader);

            let mut first = String::new();
            reader.read_line(&mut first).await.unwrap();
            match serde_json::from_str::<Method>(&first).unwrap() {
                Method::RegisterPeer { name } => assert_eq!(name, "workspace-orchestrator"),
                method => panic!("expected RegisterPeer before Run, got {method:?}"),
            }
            write_test_event(
                &mut writer,
                Event::PeerRegistered {
                    result: serde_json::json!({"peer": "workspace-orchestrator"}),
                },
            )
            .await;

            let mut second = String::new();
            reader.read_line(&mut second).await.unwrap();
            match serde_json::from_str::<Method>(&second).unwrap() {
                Method::Run { input } => {
                    assert_eq!(
                        input,
                        test_launch_plan(std::path::Path::new("/tmp")).run_segments
                    )
                }
                method => panic!("expected Run after pre-run RegisterPeer, got {method:?}"),
            }
        });

        let mut client = WorkerClient::connect(&socket_path).await.unwrap();
        let options = TicketRoleLaunchOptions::default()
            .with_pre_run_peer_registration("workspace-orchestrator");
        let warnings = run_pre_run_options_then_send_run(
            &mut client,
            &test_launch_plan(std::path::Path::new("/tmp")),
            &options,
        )
        .await
        .unwrap();

        server.await.unwrap();
        assert!(warnings.is_empty());
    }

    #[tokio::test]
    async fn pre_run_peer_registration_failure_warns_but_still_sends_run() {
        let temp = TempDir::new().unwrap();
        let socket_path = temp.path().join("worker.sock");
        let listener = UnixListener::bind(&socket_path).unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (reader, mut writer) = stream.into_split();
            write_test_event(&mut writer, test_snapshot()).await;
            let mut reader = BufReader::new(reader);

            let mut first = String::new();
            reader.read_line(&mut first).await.unwrap();
            assert!(matches!(
                serde_json::from_str::<Method>(&first).unwrap(),
                Method::RegisterPeer { .. }
            ));
            write_test_event(
                &mut writer,
                Event::Error {
                    code: protocol::ErrorCode::InvalidRequest,
                    message: "peer metadata unavailable".to_string(),
                },
            )
            .await;

            let mut second = String::new();
            reader.read_line(&mut second).await.unwrap();
            assert!(matches!(
                serde_json::from_str::<Method>(&second).unwrap(),
                Method::Run { .. }
            ));
        });

        let mut client = WorkerClient::connect(&socket_path).await.unwrap();
        let options = TicketRoleLaunchOptions::default()
            .with_pre_run_peer_registration("workspace-orchestrator");
        let warnings = run_pre_run_options_then_send_run(
            &mut client,
            &test_launch_plan(std::path::Path::new("/tmp")),
            &options,
        )
        .await
        .unwrap();

        server.await.unwrap();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].message.contains("InvalidRequest"));
        assert!(warnings[0].message.contains("workspace-orchestrator"));
    }

    #[test]
    fn default_config_role_launch_plan_requires_explicit_role_config() {
        let temp = TempDir::new().unwrap();
        let mut context = TicketRoleLaunchContext::new(temp.path(), TicketRole::Coder);
        context.ticket = Some(TicketRef::id("Ticket Role Worker Launcher"));

        let err = plan_ticket_role_launch(context).unwrap_err();

        assert!(
            err.to_string()
                .contains("Ticket role `coder` is not launch-configured")
        );
        assert!(err.to_string().contains("[ticket.roles.coder]"));
    }

    #[test]
    fn backend_only_config_is_not_sufficient_for_role_launch_plan() {
        let temp = TempDir::new().unwrap();
        write_config(
            temp.path(),
            r#"
[backend]
provider = "builtin:yoi_local"
root = ".yoi/tickets"
"#,
        );
        let context = TicketRoleLaunchContext::new(temp.path(), TicketRole::Intake);

        let err = plan_ticket_role_launch(context).unwrap_err();

        assert!(
            err.to_string()
                .contains("Ticket role `intake` is not launch-configured")
        );
    }

    #[test]
    fn explicit_inherit_profile_fails_before_launch_planning() {
        let temp = TempDir::new().unwrap();
        write_config(
            temp.path(),
            r#"
[ticket.roles.intake]
profile = "inherit"
"#,
        );
        let context = TicketRoleLaunchContext::new(temp.path(), TicketRole::Intake);

        let err = plan_ticket_role_launch(context).unwrap_err();

        assert!(err.to_string().contains("profile = \"inherit\""));
        assert!(err.to_string().contains("top-level Ticket role launch"));
    }

    #[test]
    fn unresolvable_profile_selector_fails_before_spawn() {
        let temp = TempDir::new().unwrap();
        write_config(
            temp.path(),
            r#"
[ticket.roles.intake]
profile = "project:no-such-ticket-role-profile"
"#,
        );
        let context = TicketRoleLaunchContext::new(temp.path(), TicketRole::Intake);

        let err = plan_ticket_role_launch(context).unwrap_err();

        assert!(
            err.to_string().contains(
                "profile selector `project:no-such-ticket-role-profile` is not resolvable"
            )
        );
        assert!(err.to_string().contains("[ticket.roles.intake].profile"));
    }

    #[test]
    fn full_concrete_role_config_allows_launch_planning() {
        let temp = TempDir::new().unwrap();
        write_builtin_role_config(temp.path(), &[TicketRole::Intake]);
        let context = TicketRoleLaunchContext::new(temp.path(), TicketRole::Intake);

        let plan = plan_ticket_role_launch(context).unwrap();

        assert_eq!(plan.role, TicketRole::Intake);
        assert_eq!(plan.profile, "builtin:companion");
    }

    #[test]
    fn ticket_record_language_stays_out_of_first_run_text() {
        let temp = TempDir::new().unwrap();
        write_config(
            temp.path(),
            r#"
[ticket]
language = "Japanese"

[ticket.roles.intake]
profile = "builtin:companion"
"#,
        );
        let context = TicketRoleLaunchContext::new(temp.path(), TicketRole::Intake);

        let plan = plan_ticket_role_launch(context).unwrap();
        let text = text_segment(&plan);

        assert!(!text.contains("Ticket record language"));
        assert!(!text.contains("Japanese"));
        assert!(!text.contains("write durable Ticket item/thread/resolution text"));
    }

    #[test]
    fn scaffold_config_allows_intake_and_orchestrator_launch_planning() {
        let temp = TempDir::new().unwrap();
        write_config(temp.path(), &ticket::config::ticket_config_scaffold());

        let intake = plan_ticket_role_launch(TicketRoleLaunchContext::new(
            temp.path(),
            TicketRole::Intake,
        ))
        .unwrap();
        assert_eq!(intake.role, TicketRole::Intake);
        assert_eq!(intake.profile, TicketRole::Intake.default_profile());

        let orchestrator = plan_ticket_role_launch(TicketRoleLaunchContext::new(
            temp.path(),
            TicketRole::Orchestrator,
        ))
        .unwrap();
        assert_eq!(orchestrator.role, TicketRole::Orchestrator);
        assert_eq!(
            orchestrator.profile,
            TicketRole::Orchestrator.default_profile()
        );
    }

    #[test]
    fn spawn_config_still_rejects_inherit_profile_defensively() {
        let temp = TempDir::new().unwrap();
        let mut plan = test_launch_plan(temp.path());
        plan.profile = "inherit".to_string();

        let err = plan
            .spawn_config(WorkerRuntimeCommand::for_executable("/bin/yoi"))
            .unwrap_err();

        assert!(matches!(
            err,
            TicketRoleLaunchError::UnsupportedInheritProfile
        ));
        assert!(err.to_string().contains("'inherit' cannot be used"));
    }

    #[test]
    fn configured_role_refs_are_plan_metadata_not_submit_text() {
        let temp = TempDir::new().unwrap();
        write_config(
            temp.path(),
            r#"
[ticket.roles.reviewer]
profile = "builtin:companion"
launch_prompt = "ticket.reviewer.launch"
"#,
        );
        let mut context = TicketRoleLaunchContext::new(temp.path(), TicketRole::Reviewer);
        context.worker_name = Some("reviewer-fixed".to_string());
        context.ticket = Some(TicketRef::id("20260605-190330-ticket-role-worker-launcher"));
        context.user_instruction = Some("Review the submitted implementation.".to_string());

        let plan = plan_ticket_role_launch(context).unwrap();
        let text = text_segment(&plan);

        assert_eq!(plan.worker_name, "reviewer-fixed");
        assert_eq!(plan.profile, "builtin:companion");
        assert_eq!(
            plan.launch_prompt_ref.as_deref(),
            Some("ticket.reviewer.launch")
        );
        assert!(matches!(&plan.run_segments[0], Segment::Text { .. }));
        assert!(!text.contains("Configured launch_prompt"));
        assert!(!text.contains("ticket.reviewer.launch"));
        assert!(!text.contains("Profile selector: builtin:companion"));
        assert!(!text.contains("Role: reviewer"));
        assert!(!text.contains("system_instruction"));
        assert!(text.contains("Target Ticket:"));
        assert!(text.contains("id: 20260605-190330-ticket-role-worker-launcher"));
        assert!(text.contains("Action instruction:"));
        assert!(text.contains("Review the submitted implementation."));
        let spawn = plan
            .spawn_config(WorkerRuntimeCommand::for_executable("/bin/yoi"))
            .unwrap();
        assert_eq!(spawn.worker_name, "reviewer-fixed");
        assert_eq!(spawn.profile.as_deref(), Some("builtin:companion"));
        assert_eq!(spawn.workspace_root, temp.path());
        assert!(spawn.cwd.is_none());
        assert_eq!(
            plan.spawn_options().extra_args,
            vec!["--ticket-role".to_string(), "reviewer".to_string()]
        );
    }

    #[test]
    fn submit_text_contains_only_ticket_action_and_per_launch_context() {
        let temp = TempDir::new().unwrap();
        write_builtin_role_config(
            temp.path(),
            &[
                TicketRole::Intake,
                TicketRole::Orchestrator,
                TicketRole::Coder,
                TicketRole::Reviewer,
            ],
        );

        let mut intake = TicketRoleLaunchContext::new(temp.path(), TicketRole::Intake);
        intake.user_instruction = Some("Clarify and materialize this request as a Ticket.".into());
        let intake_plan = plan_ticket_role_launch(intake).unwrap();
        let intake_text = text_segment(&intake_plan);
        assert!(intake_text.contains("Action instruction:"));
        assert!(intake_text.contains("Clarify and materialize"));
        assert!(!intake_text.contains("Workflow:"));
        assert!(!intake_text.contains("Profile selector:"));
        assert!(!intake_text.contains("Role:"));

        let mut handoff_intake = TicketRoleLaunchContext::new(temp.path(), TicketRole::Intake);
        handoff_intake.intake_handoff = Some(TicketIntakeHandoff::new(
            "panel-orchestrator-demo",
            "Demo workspace",
        ));
        let handoff_plan = plan_ticket_role_launch(handoff_intake).unwrap();
        let handoff_text = text_segment(&handoff_plan);
        assert!(handoff_text.contains("Panel handoff:"));
        assert!(
            handoff_text
                .contains("workspace_workspace_orchestrator_worker: panel-orchestrator-demo")
        );
        assert!(handoff_text.contains("workspace: Demo workspace"));
        assert!(!handoff_text.contains("created_or_updated_ticket_id"));
        assert!(!handoff_text.contains("Ticket tool surface"));
        assert!(!handoff_text.contains("ready -> queued"));

        let mut orchestrator = TicketRoleLaunchContext::new(temp.path(), TicketRole::Orchestrator);
        orchestrator.ticket = Some(TicketRef::id("launcher"));
        orchestrator.intent_packet = Some("Route to implementation after planning sync.".into());
        orchestrator.validation = vec!["cargo check --workspace --all-targets".into()];
        let orchestrator_plan = plan_ticket_role_launch(orchestrator).unwrap();
        let orchestrator_text = text_segment(&orchestrator_plan);
        assert!(orchestrator_text.contains("id: launcher"));
        assert!(orchestrator_text.contains("Route to implementation after planning sync."));
        assert!(orchestrator_text.contains("cargo check --workspace --all-targets"));
        assert!(!orchestrator_text.contains("state = inprogress"));
        assert!(!orchestrator_text.contains("root/original workspace reads"));
        assert!(!orchestrator_text.contains("role_workspace_root"));
        assert!(!orchestrator_text.contains("role_cwd"));

        let mut coder = TicketRoleLaunchContext::new(temp.path(), TicketRole::Coder);
        coder.ticket = Some(TicketRef::id("20260605-190330-ticket-role-worker-launcher"));
        coder.worktree_path = Some(PathBuf::from("/tmp/yoi-code"));
        coder.branch = Some("work/ticket-role-worker-launcher".into());
        coder.validation = vec!["cargo test -p client ticket_role".into()];
        coder.report_expectations = vec!["implementation report with validation".into()];
        let coder_plan = plan_ticket_role_launch(coder).unwrap();
        let coder_text = text_segment(&coder_plan);
        assert!(coder_text.contains("path: /tmp/yoi-code"));
        assert!(coder_text.contains("branch: work/ticket-role-worker-launcher"));
        assert!(coder_text.contains("cargo test -p client ticket_role"));
        assert!(coder_text.contains("implementation report with validation"));
        assert!(!coder_text.contains("provided child worktree/branch"));
        assert!(!coder_text.contains("choose local tactics"));
        assert!(!coder_text.contains("Do not merge, push"));

        let mut reviewer = TicketRoleLaunchContext::new(temp.path(), TicketRole::Reviewer);
        reviewer.ticket = Some(TicketRef::id("20260605-190330-ticket-role-worker-launcher"));
        reviewer.worktree_path = Some(PathBuf::from("/tmp/yoi-review"));
        reviewer.branch = Some("work/ticket-role-worker-launcher".into());
        reviewer.report_expectations = vec!["approve or request changes".into()];
        let reviewer_plan = plan_ticket_role_launch(reviewer).unwrap();
        let reviewer_text = text_segment(&reviewer_plan);
        assert!(reviewer_text.contains("path: /tmp/yoi-review"));
        assert!(reviewer_text.contains("branch: work/ticket-role-worker-launcher"));
        assert!(reviewer_text.contains("approve or request changes"));
        assert!(!reviewer_text.contains("read-only by default"));
        assert!(!reviewer_text.contains("Orchestrator-side integration"));
        assert!(!reviewer_text.contains("Do not merge, close"));
    }

    #[test]
    fn orchestrator_submit_exposes_operation_targets_without_runtime_workspace_context() {
        let temp = TempDir::new().unwrap();
        write_builtin_role_config(temp.path(), &[TicketRole::Orchestrator]);
        let mut orchestrator = TicketRoleLaunchContext::new(temp.path(), TicketRole::Orchestrator);
        orchestrator.ticket = Some(TicketRef::id("orchestrator-integration"));
        orchestrator.intent_packet = Some("Complete an already-reviewed Ticket.".into());
        orchestrator.validation = vec!["cargo test -p client ticket_role --lib".into()];
        orchestrator = orchestrator
            .with_original_workspace_root(temp.path().join("original"))
            .with_target_workspace_root(temp.path().join("target"));

        let plan = plan_ticket_role_launch(orchestrator).unwrap();
        let text = text_segment(&plan);
        assert_eq!(plan.workspace_root, temp.path());
        assert_eq!(plan.original_workspace_root, temp.path().join("original"));
        assert_eq!(
            plan.implementation_worktree_root,
            temp.path().join("original").join(".worktree")
        );
        assert_eq!(plan.target_workspace_root, temp.path().join("target"));
        let spawn_config = plan
            .spawn_config(WorkerRuntimeCommand::for_executable("/bin/yoi"))
            .unwrap();
        assert_eq!(spawn_config.workspace_root, temp.path());
        assert_eq!(spawn_config.cwd, None);

        assert!(text.contains("Orchestrator operation targets:"));
        assert!(text.contains("implementation_worktree_root"));
        assert!(!text.contains("merge_target_workspace_root"));
        assert!(!text.contains("Workspace routing context:"));
        assert!(!text.contains("role_workspace_root"));
        assert!(!text.contains("role_cwd"));
        assert!(!text.contains("original_workspace_root"));
        assert!(!text.contains("Orchestrator workspace on the orchestration branch"));
        assert!(!text.contains("Root/original workspace reads, writes, validation, cleanup, and git operations are prohibited"));
        assert!(!text.contains("Orchestrator implementation integration guidance"));
    }
    #[test]
    fn caller_provided_worker_name_is_used_exactly() {
        let temp = TempDir::new().unwrap();
        write_builtin_role_config(temp.path(), &[TicketRole::Intake]);
        let mut context = TicketRoleLaunchContext::new(temp.path(), TicketRole::Intake);
        context.worker_name = Some("custom-intake-worker".into());

        let plan = plan_ticket_role_launch(context).unwrap();

        assert_eq!(plan.worker_name, "custom-intake-worker");
    }

    #[test]
    fn malformed_ticket_config_surfaces_error() {
        let temp = TempDir::new().unwrap();
        write_config(
            temp.path(),
            r#"
[ticket.roles.coder]
profile = "./coder.toml"
"#,
        );
        let context = TicketRoleLaunchContext::new(temp.path(), TicketRole::Coder);

        let err = plan_ticket_role_launch(context).unwrap_err();

        assert!(err.to_string().contains("path selectors are not supported"));
    }

    #[test]
    fn system_instruction_is_not_a_launch_config_field() {
        let temp = TempDir::new().unwrap();
        write_config(
            temp.path(),
            r#"
[ticket.roles.coder]
profile = "inherit"
system_instruction = "unsupported"
"#,
        );
        let context = TicketRoleLaunchContext::new(temp.path(), TicketRole::Coder);

        let err = plan_ticket_role_launch(context).unwrap_err();

        assert!(err.to_string().contains("unknown field"));
        assert!(err.to_string().contains("system_instruction"));
    }
}
