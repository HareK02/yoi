//! Ticket-role Pod launch planning and execution.
//!
//! This module keeps Ticket role configuration, generated first-run input, and
//! host-side Pod spawning behind the `client` crate so UI callers do not need to
//! depend on `pod` internals.

use std::io;
use std::path::PathBuf;
use std::time::Duration;

use manifest::{ProfileDiscovery, ProfileResolveOptions, ProfileResolver, ProfileSelector};
use protocol::{ErrorCode, Event, InvokeKind, Method, Segment};
use thiserror::Error;
pub use ticket::config::TicketRole;
use ticket::config::{TicketConfig, TicketConfigError, TicketRoleLaunchConfigError};

use crate::{PodClient, PodRuntimeCommand, SpawnConfig, SpawnError, SpawnReady, spawn_pod};

const MAX_FIELD_CHARS: usize = 8_000;
const MAX_POD_NAME_CHARS: usize = 80;
const RUN_ACCEPTANCE_TIMEOUT: Duration = Duration::from_secs(10);
const PRE_RUN_ACTION_TIMEOUT: Duration = Duration::from_secs(5);

/// Ticket identifier carried by a role launch request.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TicketRef {
    pub id: Option<String>,
    pub slug: Option<String>,
}

impl TicketRef {
    pub fn id(id: impl Into<String>) -> Self {
        Self {
            id: Some(id.into()),
            slug: None,
        }
    }

    pub fn slug(slug: impl Into<String>) -> Self {
        Self {
            id: None,
            slug: Some(slug.into()),
        }
    }

    pub fn id_slug(id: impl Into<String>, slug: impl Into<String>) -> Self {
        Self {
            id: Some(id.into()),
            slug: Some(slug.into()),
        }
    }

    fn pod_name_seed(&self) -> Option<&str> {
        non_empty(self.slug.as_deref()).or_else(|| non_empty(self.id.as_deref()))
    }

    fn append_prompt_lines(&self, out: &mut String) {
        match (
            non_empty(self.id.as_deref()),
            non_empty(self.slug.as_deref()),
        ) {
            (None, None) => out.push_str("Target Ticket: not specified\n"),
            (id, slug) => {
                out.push_str("Target Ticket:\n");
                if let Some(id) = id {
                    push_bounded_bullet(out, "id", id);
                }
                if let Some(slug) = slug {
                    push_bounded_bullet(out, "slug", slug);
                }
            }
        }
    }
}

/// Auditable panel handoff target included in a Ticket Intake launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketIntakeHandoff {
    pub orchestrator_pod: String,
    pub workspace_label: String,
}

impl TicketIntakeHandoff {
    pub fn new(orchestrator_pod: impl Into<String>, workspace_label: impl Into<String>) -> Self {
        Self {
            orchestrator_pod: orchestrator_pod.into(),
            workspace_label: workspace_label.into(),
        }
    }

    fn append_prompt_lines(&self, out: &mut String) {
        out.push_str("\nPanel handoff:\n");
        push_bounded_bullet(out, "workspace", &self.workspace_label);
        push_bounded_bullet(out, "workspace_orchestrator_pod", &self.orchestrator_pod);
        out.push_str("- When Intake has clarified the request and created/updated the Ticket, use the typed Ticket tool surface to append `intake_summary` and set `workflow_state = ready` when the Ticket is ready to queue.\n");
        out.push_str("- Handoff report fields: created_or_updated_ticket_id_or_slug, workflow_state, needs_preflight, risk_flags, intake_summary.\n");
        out.push_str("- Do not start implementation automatically; the user queues a ready Ticket via panel (`ready -> queued`), and Orchestrator treats `queued` as schedulable before moving it to `inprogress` when starting.\n");
    }
}

/// Typed input for constructing a Ticket role launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketRoleLaunchContext {
    pub workspace_root: PathBuf,
    pub role: TicketRole,
    pub pod_name: Option<String>,
    pub ticket: Option<TicketRef>,
    pub user_instruction: Option<String>,
    pub intake_handoff: Option<TicketIntakeHandoff>,
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
            role,
            pod_name: None,
            ticket: None,
            user_instruction: None,
            intake_handoff: None,
            intent_packet: None,
            worktree_path: None,
            branch: None,
            validation: Vec::new(),
            report_expectations: Vec::new(),
        }
    }
}

/// Pure launch plan usable by TUI/CLI surfaces before executing the launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketRoleLaunchPlan {
    pub workspace_root: PathBuf,
    pub role: TicketRole,
    pub pod_name: String,
    pub profile: String,
    pub workflow: String,
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
        runtime_command: PodRuntimeCommand,
    ) -> Result<SpawnConfig, TicketRoleLaunchError> {
        if self.profile == "inherit" {
            return Err(TicketRoleLaunchError::UnsupportedInheritProfile);
        }
        Ok(SpawnConfig {
            runtime_command,
            pod_name: self.pod_name.clone(),
            profile: Some(self.profile.clone()),
            cwd: self.workspace_root.clone(),
            resume_from: None,
            resume_by_pod_name: false,
        })
    }
}

/// Result of executing a Ticket role launch.
#[derive(Debug, Clone)]
pub struct TicketRoleLaunchResult {
    pub plan: TicketRoleLaunchPlan,
    pub ready: SpawnReady,
    pub pre_run_warnings: Vec<TicketRolePreRunWarning>,
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
    pub fn with_pre_run_peer_registration(mut self, pod_name: impl Into<String>) -> Self {
        self.pre_run_peer_registrations.push(pod_name.into());
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
        "Ticket role `{role}` profile selector `{selector}` is not resolvable before launch: {message}. Configure `[roles.{role}].profile` with an executable concrete profile selector such as `builtin:default` or a project/user profile"
    )]
    ProfileResolution {
        role: TicketRole,
        selector: String,
        message: String,
    },
    #[error("Ticket role Pod name must not be empty")]
    EmptyPodName,
    #[error(
        "Ticket role profile 'inherit' cannot be used for top-level launch execution; configure a concrete role profile selector"
    )]
    UnsupportedInheritProfile,
    #[error(transparent)]
    Spawn(#[from] SpawnError),
    #[error("failed to connect to spawned Ticket role Pod at {}: {source}", .socket_path.display())]
    Connect {
        socket_path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to send first run input to spawned Ticket role Pod: {source}")]
    SendRun {
        #[source]
        source: io::Error,
    },
    #[error("Ticket role Pod rejected first run input with {code:?}: {message}")]
    RunRejected { code: ErrorCode, message: String },
    #[error("Ticket role Pod closed before confirming first run acceptance")]
    RunAcceptanceClosed,
    #[error("timed out waiting for Ticket role Pod to confirm first run acceptance")]
    RunAcceptanceTimeout,
}

/// Load `.yoi/ticket.config.toml` from the workspace and construct a launch plan.
pub fn plan_ticket_role_launch(
    context: TicketRoleLaunchContext,
) -> Result<TicketRoleLaunchPlan, TicketRoleLaunchError> {
    let config = TicketConfig::load_workspace(&context.workspace_root)?;
    plan_ticket_role_launch_with_config(context, &config)
}

/// Construct a launch plan from an already-loaded Ticket config.
pub fn plan_ticket_role_launch_with_config(
    context: TicketRoleLaunchContext,
    config: &TicketConfig,
) -> Result<TicketRoleLaunchPlan, TicketRoleLaunchError> {
    let role_config = config.role_launch_config(context.role)?;
    let profile = role_config.profile.as_str().to_string();
    let workflow = role_config.workflow.as_str().to_string();
    let launch_prompt_ref = role_config
        .launch_prompt
        .as_ref()
        .map(|prompt| prompt.as_str().to_string());
    let pod_name = match context.pod_name.as_deref().map(str::trim) {
        Some("") => return Err(TicketRoleLaunchError::EmptyPodName),
        Some(name) => name.to_string(),
        None => default_pod_name(context.role, context.ticket.as_ref()),
    };
    validate_ticket_role_profile(context.role, &profile, &context.workspace_root, &pod_name)?;
    let prompt = build_launch_prompt(&context, &profile, &workflow, launch_prompt_ref.as_deref());

    Ok(TicketRoleLaunchPlan {
        workspace_root: context.workspace_root,
        role: context.role,
        pod_name,
        profile,
        workflow: workflow.clone(),
        launch_prompt_ref,
        run_segments: vec![
            Segment::WorkflowInvoke { slug: workflow },
            Segment::Text {
                content: format!("\n\n{prompt}"),
            },
        ],
    })
}

fn validate_ticket_role_profile(
    role: TicketRole,
    profile: &str,
    workspace_root: &std::path::Path,
    pod_name: &str,
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
            ProfileResolveOptions::with_pod_name(pod_name),
        )
        .map(|_| ())
        .map_err(|source| TicketRoleLaunchError::ProfileResolution {
            role,
            selector: profile.to_string(),
            message: source.to_string(),
        })
}

/// Spawn the Pod, connect to its socket, send the first `Method::Run` input,
/// and wait for bounded acceptance evidence from the Pod event stream.
pub async fn launch_ticket_role_pod<F>(
    context: TicketRoleLaunchContext,
    runtime_command: PodRuntimeCommand,
    progress: F,
) -> Result<TicketRoleLaunchResult, TicketRoleLaunchError>
where
    F: FnMut(&str),
{
    launch_ticket_role_pod_with_options(
        context,
        runtime_command,
        progress,
        TicketRoleLaunchOptions::default(),
    )
    .await
}

/// Spawn the Pod, run bounded pre-run launch options while it is still idle,
/// then send the first `Method::Run` input and wait for acceptance evidence.
pub async fn launch_ticket_role_pod_with_options<F>(
    context: TicketRoleLaunchContext,
    runtime_command: PodRuntimeCommand,
    progress: F,
    options: TicketRoleLaunchOptions,
) -> Result<TicketRoleLaunchResult, TicketRoleLaunchError>
where
    F: FnMut(&str),
{
    let plan = plan_ticket_role_launch(context)?;
    let ready = spawn_pod(plan.spawn_config(runtime_command)?, progress).await?;
    let mut client = PodClient::connect(&ready.socket_path)
        .await
        .map_err(|source| TicketRoleLaunchError::Connect {
            socket_path: ready.socket_path.clone(),
            source,
        })?;
    let pre_run_warnings = run_pre_run_options_then_send_run(&mut client, &plan, &options).await?;
    wait_for_run_acceptance(&mut client, &plan.run_segments, RUN_ACCEPTANCE_TIMEOUT).await?;
    Ok(TicketRoleLaunchResult {
        plan,
        ready,
        pre_run_warnings,
    })
}

async fn run_pre_run_options_then_send_run(
    client: &mut PodClient,
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
    client: &mut PodClient,
    peer_names: &[String],
    timeout: Duration,
) -> Vec<TicketRolePreRunWarning> {
    let mut warnings = Vec::new();
    for peer_name in peer_names {
        if peer_name.trim().is_empty() {
            warnings.push(TicketRolePreRunWarning {
                message: "pre-run peer registration skipped: peer Pod name is empty".to_string(),
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
    client: &mut PodClient,
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
    client: &mut PodClient,
    expected_segments: &[Segment],
    timeout: Duration,
) -> Result<(), TicketRoleLaunchError> {
    let wait = async {
        loop {
            let Some(event) = client.next_event().await else {
                return Err(TicketRoleLaunchError::RunAcceptanceClosed);
            };
            match event {
                Event::UserMessage { segments } if segments == expected_segments => return Ok(()),
                Event::InvokeStart {
                    kind: InvokeKind::UserSend,
                }
                | Event::TurnStart { .. } => return Ok(()),
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

fn build_launch_prompt(
    context: &TicketRoleLaunchContext,
    profile: &str,
    workflow: &str,
    launch_prompt_ref: Option<&str>,
) -> String {
    let mut out = String::new();
    out.push_str("# Ticket role launch\n\n");
    out.push_str("Profile supplies durable system/role behavior. The workflow segment supplies the procedural flow. This generated launch prompt supplies only the concrete Ticket/action context for the first committed user task.\n\n");
    push_bounded_field(&mut out, "Role", context.role.as_str());
    push_bounded_field(&mut out, "Profile selector", profile);
    push_bounded_field(&mut out, "Workflow", workflow);
    match launch_prompt_ref {
        Some(prompt_ref) => push_bounded_field(
            &mut out,
            "Configured launch_prompt ref (unresolved)",
            prompt_ref,
        ),
        None => out.push_str("Configured launch_prompt ref: none\n"),
    }
    out.push('\n');

    if let Some(ticket) = &context.ticket {
        ticket.append_prompt_lines(&mut out);
    } else {
        out.push_str("Target Ticket: not specified\n");
    }

    match non_empty(context.user_instruction.as_deref()) {
        Some(instruction) => push_bounded_section(&mut out, "User/action instruction", instruction),
        None => out.push_str("\nUser/action instruction: not specified\n"),
    }

    if let Some(handoff) = &context.intake_handoff {
        handoff.append_prompt_lines(&mut out);
    }

    if let Some(intent_packet) = non_empty(context.intent_packet.as_deref()) {
        push_bounded_section(&mut out, "Intent packet", intent_packet);
    }

    if context.worktree_path.is_some() || non_empty(context.branch.as_deref()).is_some() {
        out.push_str("\nWorktree context:\n");
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

fn default_pod_name(role: TicketRole, ticket: Option<&TicketRef>) -> String {
    let mut name = format!("ticket-{}", role.as_str());
    if let Some(seed) = ticket.and_then(TicketRef::pod_name_seed) {
        let suffix = sanitise_pod_name_component(seed);
        if !suffix.is_empty() {
            name.push('-');
            name.push_str(&suffix);
        }
    }
    name.chars().take(MAX_POD_NAME_CHARS).collect()
}

fn sanitise_pod_name_component(value: &str) -> String {
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

fn push_bounded_field(out: &mut String, label: &str, value: &str) {
    out.push_str(label);
    out.push_str(": ");
    out.push_str(&bounded(value));
    out.push('\n');
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
    use protocol::{Greeting, PodStatus};
    use tempfile::TempDir;
    use tokio::io::{AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};
    use tokio::net::UnixListener;

    fn write_config(workspace: &std::path::Path, content: &str) {
        let dir = workspace.join(".yoi");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("ticket.config.toml"), content).unwrap();
    }

    fn write_builtin_role_config(workspace: &std::path::Path, roles: &[TicketRole]) {
        let mut config = String::new();
        for role in roles {
            config.push_str(&format!(
                "\n[roles.{role}]\nprofile = \"builtin:default\"\n"
            ));
        }
        write_config(workspace, &config);
    }

    fn text_segment(plan: &TicketRoleLaunchPlan) -> &str {
        match &plan.run_segments[1] {
            Segment::Text { content } => content,
            other => panic!("expected text segment, got {other:?}"),
        }
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
            entries: vec![],
            greeting: Greeting {
                pod_name: "ticket-intake".to_string(),
                cwd: "/tmp".to_string(),
                provider: "test".to_string(),
                model: "test".to_string(),
                scope_summary: "test".to_string(),
                tools: vec![],
                context_window: 0,
                context_tokens: 0,
            },
            status: PodStatus::Idle,
        }
    }

    fn test_launch_plan(workspace: &std::path::Path) -> TicketRoleLaunchPlan {
        TicketRoleLaunchPlan {
            workspace_root: workspace.to_path_buf(),
            role: TicketRole::Intake,
            pod_name: "ticket-intake".to_string(),
            profile: "project:intake".to_string(),
            workflow: "ticket-intake-workflow".to_string(),
            launch_prompt_ref: None,
            run_segments: vec![Segment::Text {
                content: "intake request".to_string(),
            }],
        }
    }

    #[tokio::test]
    async fn pre_run_peer_registration_is_sent_before_first_run_submission() {
        let temp = TempDir::new().unwrap();
        let socket_path = temp.path().join("pod.sock");
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

        let mut client = PodClient::connect(&socket_path).await.unwrap();
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
        let socket_path = temp.path().join("pod.sock");
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

        let mut client = PodClient::connect(&socket_path).await.unwrap();
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
        context.ticket = Some(TicketRef::slug("Ticket Role Pod Launcher"));

        let err = plan_ticket_role_launch(context).unwrap_err();

        assert!(
            err.to_string()
                .contains("Ticket role `coder` is not launch-configured")
        );
        assert!(err.to_string().contains("[roles.coder]"));
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
[roles.intake]
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
[roles.intake]
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
        assert!(err.to_string().contains("[roles.intake].profile"));
    }

    #[test]
    fn full_concrete_role_config_allows_launch_planning() {
        let temp = TempDir::new().unwrap();
        write_builtin_role_config(temp.path(), &[TicketRole::Intake]);
        let context = TicketRoleLaunchContext::new(temp.path(), TicketRole::Intake);

        let plan = plan_ticket_role_launch(context).unwrap();

        assert_eq!(plan.role, TicketRole::Intake);
        assert_eq!(plan.profile, "builtin:default");
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
        assert_eq!(intake.profile, "builtin:default");
        assert_eq!(intake.workflow, TicketRole::Intake.default_workflow());

        let orchestrator = plan_ticket_role_launch(TicketRoleLaunchContext::new(
            temp.path(),
            TicketRole::Orchestrator,
        ))
        .unwrap();
        assert_eq!(orchestrator.role, TicketRole::Orchestrator);
        assert_eq!(orchestrator.profile, "builtin:default");
        assert_eq!(
            orchestrator.workflow,
            TicketRole::Orchestrator.default_workflow()
        );
    }

    #[test]
    fn spawn_config_still_rejects_inherit_profile_defensively() {
        let temp = TempDir::new().unwrap();
        let mut plan = test_launch_plan(temp.path());
        plan.profile = "inherit".to_string();

        let err = plan
            .spawn_config(PodRuntimeCommand::for_executable("/bin/yoi"))
            .unwrap_err();

        assert!(matches!(
            err,
            TicketRoleLaunchError::UnsupportedInheritProfile
        ));
        assert!(err.to_string().contains("'inherit' cannot be used"));
    }

    #[test]
    fn configured_role_refs_are_exposed_in_plan_and_prompt() {
        let temp = TempDir::new().unwrap();
        write_config(
            temp.path(),
            r#"
[roles.reviewer]
profile = "builtin:default"
launch_prompt = "$workspace/ticket/reviewer/launch"
workflow = "ticket-review-workflow"
"#,
        );
        let mut context = TicketRoleLaunchContext::new(temp.path(), TicketRole::Reviewer);
        context.pod_name = Some("reviewer-fixed".to_string());
        context.ticket = Some(TicketRef::id_slug(
            "20260605-190330-ticket-role-pod-launcher",
            "ticket-role-pod-launcher",
        ));
        context.user_instruction = Some("Review the submitted implementation.".to_string());

        let plan = plan_ticket_role_launch(context).unwrap();
        let text = text_segment(&plan);

        assert_eq!(plan.pod_name, "reviewer-fixed");
        assert_eq!(plan.profile, "builtin:default");
        assert_eq!(plan.workflow, "ticket-review-workflow");
        assert_eq!(
            plan.launch_prompt_ref.as_deref(),
            Some("$workspace/ticket/reviewer/launch")
        );
        assert!(matches!(
            &plan.run_segments[0],
            Segment::WorkflowInvoke { slug } if slug == "ticket-review-workflow"
        ));
        assert!(text.contains(
            "Configured launch_prompt ref (unresolved): $workspace/ticket/reviewer/launch"
        ));
        assert!(text.contains("Workflow: ticket-review-workflow"));
        assert!(text.contains("Profile selector: builtin:default"));
        assert!(!text.contains("system_instruction"));
        let spawn = plan
            .spawn_config(PodRuntimeCommand::for_executable("/bin/yoi"))
            .unwrap();
        assert_eq!(spawn.pod_name, "reviewer-fixed");
        assert_eq!(spawn.profile.as_deref(), Some("builtin:default"));
        assert_eq!(spawn.cwd, temp.path());
    }

    #[test]
    fn generated_prompt_covers_intake_orchestrator_and_reviewer_context() {
        let temp = TempDir::new().unwrap();
        write_builtin_role_config(
            temp.path(),
            &[
                TicketRole::Intake,
                TicketRole::Orchestrator,
                TicketRole::Reviewer,
            ],
        );

        let mut intake = TicketRoleLaunchContext::new(temp.path(), TicketRole::Intake);
        intake.user_instruction = Some("Clarify and materialize this request as a Ticket.".into());
        let intake_plan = plan_ticket_role_launch(intake).unwrap();
        let intake_text = text_segment(&intake_plan);
        assert!(intake_text.contains("Role: intake"));
        assert!(intake_text.contains("Clarify and materialize"));
        assert!(intake_text.contains("Workflow: ticket-intake-workflow"));

        let mut handoff_intake = TicketRoleLaunchContext::new(temp.path(), TicketRole::Intake);
        handoff_intake.intake_handoff = Some(TicketIntakeHandoff::new(
            "panel-orchestrator-demo",
            "Demo workspace",
        ));
        let handoff_plan = plan_ticket_role_launch(handoff_intake).unwrap();
        let handoff_text = text_segment(&handoff_plan);
        assert!(handoff_text.contains("Panel handoff:"));
        assert!(handoff_text.contains("workspace_orchestrator_pod: panel-orchestrator-demo"));
        assert!(handoff_text.contains("workspace: Demo workspace"));
        assert!(handoff_text.contains("created_or_updated_ticket_id_or_slug"));
        assert!(handoff_text.contains("workflow_state"));
        assert!(handoff_text.contains("Ticket tool surface"));
        assert!(handoff_text.contains("ready -> queued"));
        assert!(handoff_text.contains("queued` as schedulable"));
        assert!(!handoff_text.contains("user_go_required"));
        assert!(!handoff_text.contains("human Go gates"));

        let mut orchestrator = TicketRoleLaunchContext::new(temp.path(), TicketRole::Orchestrator);
        orchestrator.ticket = Some(TicketRef::slug("launcher"));
        orchestrator.intent_packet = Some("Route to implementation after preflight.".into());
        orchestrator.validation = vec!["cargo check --workspace --all-targets".into()];
        let orchestrator_plan = plan_ticket_role_launch(orchestrator).unwrap();
        let orchestrator_text = text_segment(&orchestrator_plan);
        assert!(orchestrator_text.contains("Role: orchestrator"));
        assert!(orchestrator_text.contains("Route to implementation after preflight."));
        assert!(orchestrator_text.contains("cargo check --workspace --all-targets"));

        let mut reviewer = TicketRoleLaunchContext::new(temp.path(), TicketRole::Reviewer);
        reviewer.ticket = Some(TicketRef::id("20260605-190330-ticket-role-pod-launcher"));
        reviewer.worktree_path = Some(PathBuf::from("/tmp/yoi-review"));
        reviewer.branch = Some("work/ticket-role-pod-launcher".into());
        reviewer.report_expectations = vec!["approve or request changes".into()];
        let reviewer_plan = plan_ticket_role_launch(reviewer).unwrap();
        let reviewer_text = text_segment(&reviewer_plan);
        assert!(reviewer_text.contains("Role: reviewer"));
        assert!(reviewer_text.contains("path: /tmp/yoi-review"));
        assert!(reviewer_text.contains("branch: work/ticket-role-pod-launcher"));
        assert!(reviewer_text.contains("approve or request changes"));
    }

    #[test]
    fn caller_provided_pod_name_is_used_exactly() {
        let temp = TempDir::new().unwrap();
        write_builtin_role_config(temp.path(), &[TicketRole::Intake]);
        let mut context = TicketRoleLaunchContext::new(temp.path(), TicketRole::Intake);
        context.pod_name = Some("custom-intake-pod".into());

        let plan = plan_ticket_role_launch(context).unwrap();

        assert_eq!(plan.pod_name, "custom-intake-pod");
    }

    #[test]
    fn malformed_ticket_config_surfaces_error() {
        let temp = TempDir::new().unwrap();
        write_config(
            temp.path(),
            r#"
[roles.coder]
profile = "./coder.lua"
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
[roles.coder]
profile = "inherit"
system_instruction = "$workspace/not-supported"
"#,
        );
        let context = TicketRoleLaunchContext::new(temp.path(), TicketRole::Coder);

        let err = plan_ticket_role_launch(context).unwrap_err();

        assert!(err.to_string().contains("unknown field"));
        assert!(err.to_string().contains("system_instruction"));
    }
}
