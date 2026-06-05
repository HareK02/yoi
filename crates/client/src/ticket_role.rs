//! Ticket-role Pod launch planning and execution.
//!
//! This module keeps Ticket role configuration, generated first-run input, and
//! host-side Pod spawning behind the `client` crate so UI callers do not need to
//! depend on `pod` internals.

use std::io;
use std::path::PathBuf;
use std::time::Duration;

use protocol::{ErrorCode, Event, InvokeKind, Method, Segment};
use thiserror::Error;
use ticket::config::{TicketConfig, TicketConfigError, TicketRole};

use crate::{PodClient, PodRuntimeCommand, SpawnConfig, SpawnError, SpawnReady, spawn_pod};

const MAX_FIELD_CHARS: usize = 8_000;
const MAX_POD_NAME_CHARS: usize = 80;
const RUN_ACCEPTANCE_TIMEOUT: Duration = Duration::from_secs(10);

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

/// Typed input for constructing a Ticket role launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketRoleLaunchContext {
    pub workspace_root: PathBuf,
    pub role: TicketRole,
    pub pod_name: Option<String>,
    pub ticket: Option<TicketRef>,
    pub user_instruction: Option<String>,
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
}

#[derive(Debug, Error)]
pub enum TicketRoleLaunchError {
    #[error(transparent)]
    Config(#[from] TicketConfigError),
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
    let role_config = config.role(context.role);
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
    let plan = plan_ticket_role_launch(context)?;
    let ready = spawn_pod(plan.spawn_config(runtime_command)?, progress).await?;
    let mut client = PodClient::connect(&ready.socket_path)
        .await
        .map_err(|source| TicketRoleLaunchError::Connect {
            socket_path: ready.socket_path.clone(),
            source,
        })?;
    client
        .send(&plan.run_method())
        .await
        .map_err(|source| TicketRoleLaunchError::SendRun { source })?;
    wait_for_run_acceptance(&mut client, &plan.run_segments, RUN_ACCEPTANCE_TIMEOUT).await?;
    Ok(TicketRoleLaunchResult { plan, ready })
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
    use tempfile::TempDir;

    fn write_config(workspace: &std::path::Path, content: &str) {
        let dir = workspace.join(".yoi");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("ticket.config.toml"), content).unwrap();
    }

    fn text_segment(plan: &TicketRoleLaunchPlan) -> &str {
        match &plan.run_segments[1] {
            Segment::Text { content } => content,
            other => panic!("expected text segment, got {other:?}"),
        }
    }

    #[test]
    fn default_config_role_launch_plan_uses_defaults() {
        let temp = TempDir::new().unwrap();
        let mut context = TicketRoleLaunchContext::new(temp.path(), TicketRole::Coder);
        context.ticket = Some(TicketRef::slug("Ticket Role Pod Launcher"));

        let plan = plan_ticket_role_launch(context).unwrap();

        assert_eq!(plan.role, TicketRole::Coder);
        assert_eq!(plan.pod_name, "ticket-coder-ticket-role-pod-launcher");
        assert_eq!(plan.profile, "inherit");
        assert_eq!(plan.workflow, "multi-agent-workflow");
        assert_eq!(plan.launch_prompt_ref, None);
        assert!(matches!(
            &plan.run_segments[0],
            Segment::WorkflowInvoke { slug } if slug == "multi-agent-workflow"
        ));
        assert!(text_segment(&plan).contains("Profile selector: inherit"));
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
profile = "project:reviewer"
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
        assert_eq!(plan.profile, "project:reviewer");
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
        assert!(text.contains("Profile selector: project:reviewer"));
        assert!(!text.contains("system_instruction"));
        let spawn = plan
            .spawn_config(PodRuntimeCommand::for_executable("/bin/yoi"))
            .unwrap();
        assert_eq!(spawn.pod_name, "reviewer-fixed");
        assert_eq!(spawn.profile.as_deref(), Some("project:reviewer"));
        assert_eq!(spawn.cwd, temp.path());
    }

    #[test]
    fn generated_prompt_covers_intake_orchestrator_and_reviewer_context() {
        let temp = TempDir::new().unwrap();

        let mut intake = TicketRoleLaunchContext::new(temp.path(), TicketRole::Intake);
        intake.user_instruction = Some("Clarify and materialize this request as a Ticket.".into());
        let intake_plan = plan_ticket_role_launch(intake).unwrap();
        let intake_text = text_segment(&intake_plan);
        assert!(intake_text.contains("Role: intake"));
        assert!(intake_text.contains("Clarify and materialize"));
        assert!(intake_text.contains("Workflow: ticket-intake-workflow"));

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
