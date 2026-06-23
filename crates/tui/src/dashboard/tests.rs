use super::render::*;
use super::*;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
#[test]
fn orchestration_worktree_layout_is_stable_under_original_workspace_root() {
    let root = Path::new("/tmp/Yoi Workspace");
    let layout = orchestration_worktree_layout(root);
    assert_eq!(
        layout.path,
        PathBuf::from("/tmp/Yoi Workspace/.worktree/orchestration")
    );
    assert_eq!(layout.branch, "orchestration");
}

#[test]
fn orchestrator_launch_context_uses_original_root_for_runtime_workspace_and_worktree_cwd() {
    let original = PathBuf::from("/repo/yoi");
    let orchestration = original
        .join(".worktree")
        .join("orchestration")
        .join("yoi-orchestrator");
    let context = build_orchestrator_launch_context(&original, &orchestration, "yoi-orchestrator");
    assert_eq!(context.workspace_root, original);
    assert_eq!(context.cwd.as_deref(), Some(orchestration.as_path()));
    assert_eq!(
        context.original_workspace_root.as_deref(),
        Some(original.as_path())
    );
    assert_eq!(
        context.target_workspace_root.as_deref(),
        Some(original.as_path())
    );
}

#[test]
fn invalid_existing_orchestration_path_is_diagnostic_not_cleanup() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("repo");
    std::fs::create_dir_all(&root).unwrap();
    let layout = orchestration_worktree_layout(&root);
    std::fs::create_dir_all(&layout.path).unwrap();
    std::fs::write(layout.path.join("keep.txt"), "do not delete").unwrap();

    let err = ensure_orchestration_worktree(&root).unwrap_err();
    assert!(err.contains("not a Git worktree"));
    assert!(layout.path.join("keep.txt").exists());
}

#[test]
fn ensure_orchestration_worktree_creates_and_reuses_git_worktree() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("repo");
    std::fs::create_dir_all(&root).unwrap();
    run_test_git(&root, &["init"]).unwrap();
    std::fs::write(root.join("README.md"), "repo").unwrap();
    run_test_git(&root, &["add", "README.md"]).unwrap();
    run_test_git(
        &root,
        &[
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "user.name=Yoi Test",
            "commit",
            "-m",
            "init",
        ],
    )
    .unwrap();

    let created = ensure_orchestration_worktree(&root).unwrap();
    assert_eq!(created.status, OrchestrationWorktreeStatus::Created);
    assert!(created.layout.path.exists());
    assert!(git_inside_worktree(&created.layout.path));

    let reused = ensure_orchestration_worktree(&root).unwrap();
    assert_eq!(reused.status, OrchestrationWorktreeStatus::Reused);
    assert_eq!(reused.layout, created.layout);

    std::fs::write(created.layout.path.join("dirty.txt"), "dirty").unwrap();
    let err = ensure_orchestration_worktree(&root).unwrap_err();
    assert!(err.contains("dirty"));
    assert!(created.layout.path.join("dirty.txt").exists());
}

#[test]
fn ensure_and_restore_use_configured_orchestration_layout() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("repo");
    init_test_repo(&root);
    write_test_ticket_config(
        &root,
        r#"
[orchestration]
branch = "orchestration/custom-panel"
worktree_dir = "custom-worktrees"
worktree_name = "panel"
"#,
    );
    run_test_git(&root, &["add", ".yoi/ticket.config.toml"]).unwrap();
    run_test_git(&root, &["commit", "-m", "ticket config"]).unwrap();

    let resolved = resolved_orchestration_worktree_layout(&root).unwrap();
    assert_eq!(resolved.branch, "orchestration/custom-panel");
    assert!(resolved.path.ends_with("custom-worktrees/panel"));

    let created = ensure_orchestration_worktree(&root).unwrap();
    assert_eq!(created.status, OrchestrationWorktreeStatus::Created);
    assert_eq!(created.layout, resolved);
    let branch = run_test_git_output(&created.layout.path, &["branch", "--show-current"]).unwrap();
    assert_eq!(branch.trim(), "orchestration/custom-panel");

    let restored = prepare_orchestration_worktree_for_restore(&root).unwrap();
    assert_eq!(restored.status, OrchestrationWorktreeStatus::Reused);
    assert_eq!(restored.layout, created.layout);
}

#[test]
fn invalid_configured_orchestration_branch_is_rejected_before_git_worktree_operations() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("repo");
    std::fs::create_dir_all(&root).unwrap();
    write_test_ticket_config(
        &root,
        r#"
[orchestration]
branch = "orchestration/bad:branch"
"#,
    );

    let err = ensure_orchestration_worktree(&root).unwrap_err();
    assert!(err.contains("failed to load ticket config"));
    assert!(err.contains("git branch name"));
    assert!(!root.join(".worktree").exists());
}

#[test]
fn restore_rejects_mismatched_configured_orchestration_branch_without_checkout() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("repo");
    init_test_repo(&root);
    write_test_ticket_config(
        &root,
        r#"
[orchestration]
branch = "orchestration/custom-panel"
"#,
    );
    run_test_git(&root, &["add", ".yoi/ticket.config.toml"]).unwrap();
    run_test_git(&root, &["commit", "-m", "ticket config"]).unwrap();
    let layout = resolved_orchestration_worktree_layout(&root).unwrap();
    run_test_git(
        &root,
        &[
            "worktree",
            "add",
            &layout.path.display().to_string(),
            "-b",
            "orchestration/other-panel",
            "HEAD",
        ],
    )
    .unwrap();

    let err = prepare_orchestration_worktree_for_restore(&root).unwrap_err();
    assert!(err.contains("expected orchestration/custom-panel"));
    let branch = run_test_git_output(&layout.path, &["branch", "--show-current"]).unwrap();
    assert_eq!(branch.trim(), "orchestration/other-panel");
}

#[test]
fn restore_uses_existing_orchestration_worktree_even_when_dirty() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("repo");
    init_test_repo(&root);
    let created = ensure_orchestration_worktree(&root).unwrap();
    std::fs::write(created.layout.path.join("orchestrator-notes.txt"), "dirty").unwrap();

    let restored = prepare_orchestration_worktree_for_restore(&root).unwrap();

    assert_eq!(restored.status, OrchestrationWorktreeStatus::Reused);
    assert_eq!(restored.layout.path, created.layout.path);
    assert_ne!(restored.layout.path, root);
    assert!(restored.layout.path.ends_with(".worktree/orchestration"));
}

#[test]
fn existing_wrong_branch_worktree_is_rejected_without_cleanup() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("repo");
    init_test_repo(&root);
    let layout = orchestration_worktree_layout(&root);
    run_test_git(
        &root,
        &[
            "worktree",
            "add",
            &layout.path.display().to_string(),
            "-b",
            "wrong-branch",
            "HEAD",
        ],
    )
    .unwrap();
    std::fs::write(layout.path.join("keep.txt"), "keep").unwrap();
    run_test_git(&layout.path, &["add", "keep.txt"]).unwrap();
    run_test_git(
        &layout.path,
        &[
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "user.name=Yoi Test",
            "commit",
            "-m",
            "keep",
        ],
    )
    .unwrap();

    let err = ensure_orchestration_worktree(&root).unwrap_err();
    assert!(err.contains("expected orchestration"));
    assert!(layout.path.join("keep.txt").exists());
}

#[test]
fn existing_unrelated_repo_with_expected_branch_is_rejected_without_cleanup() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("repo");
    init_test_repo(&root);
    let layout = orchestration_worktree_layout(&root);
    std::fs::create_dir_all(&layout.path).unwrap();
    init_test_repo(&layout.path);
    run_test_git(&layout.path, &["checkout", "-b", &layout.branch]).unwrap();
    std::fs::write(layout.path.join("unrelated.txt"), "keep").unwrap();
    run_test_git(&layout.path, &["add", "unrelated.txt"]).unwrap();
    run_test_git(
        &layout.path,
        &[
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "user.name=Yoi Test",
            "commit",
            "-m",
            "unrelated",
        ],
    )
    .unwrap();

    let err = ensure_orchestration_worktree(&root).unwrap_err();
    assert!(err.contains("different Git repository"));
    assert!(layout.path.join("unrelated.txt").exists());
}

fn write_test_ticket_config(root: &Path, content: &str) {
    let config_dir = root.join(".yoi");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(config_dir.join("ticket.config.toml"), content).unwrap();
}

fn init_test_repo(root: &Path) {
    std::fs::create_dir_all(root).unwrap();
    run_test_git(root, &["init"]).unwrap();
    run_test_git(root, &["config", "user.email", "test@example.invalid"]).unwrap();
    run_test_git(root, &["config", "user.name", "Yoi Test"]).unwrap();
    std::fs::write(root.join("README.md"), "repo").unwrap();
    run_test_git(root, &["add", "README.md"]).unwrap();
    run_test_git(root, &["commit", "-m", "init"]).unwrap();
}

#[test]
fn inherited_parent_worktree_directory_is_rejected_without_cleanup() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("repo");
    init_test_repo(&root);
    let layout = orchestration_worktree_layout(&root);
    run_test_git(&root, &["checkout", "-b", &layout.branch]).unwrap();
    std::fs::create_dir_all(&layout.path).unwrap();
    std::fs::write(layout.path.join("plain.txt"), "keep").unwrap();

    let err = ensure_orchestration_worktree(&root).unwrap_err();
    assert!(err.contains("not the worktree root"));
    assert!(layout.path.join("plain.txt").exists());
}

fn run_test_git(root: &Path, args: &[&str]) -> Result<(), String> {
    let mut command = Command::new("git");
    command.arg("-C").arg(root).args(args);
    run_git_command(command, "run test git")
}

fn run_test_git_output(root: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .map_err(|error| format!("could not run test git: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "git failed to run test git: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

use crate::pod_list::{LivePodInfo, PodEntrySummary, StoredMetadataState, StoredPodInfo};
use std::fs;
use tempfile::TempDir;
use ticket::{
    LocalTicketBackend, MarkdownText, NewTicket, NewTicketEvent, TicketBackend, TicketEventKind,
    TicketWorkflowState,
};

fn ticket_workspace(
    title: &str,
    state: TicketWorkflowState,
    configure: impl FnOnce(&mut NewTicket),
) -> (TempDir, String, LocalTicketBackend) {
    let temp = TempDir::new().unwrap();
    fs::create_dir_all(temp.path().join(".yoi")).unwrap();
    fs::write(
        temp.path().join(".gitignore"),
        ".worktree/\n.yoi/tickets/.ticket-backend.lock\n",
    )
    .unwrap();
    fs::write(
        temp.path().join(".yoi/.gitignore"),
        "tickets/.ticket-backend.lock\n",
    )
    .unwrap();
    fs::write(
        temp.path().join(".yoi/ticket.config.toml"),
        "[backend]\nprovider = \"builtin:yoi_local\"\nroot = \".yoi/tickets\"\n",
    )
    .unwrap();
    let backend = LocalTicketBackend::new(temp.path().join(".yoi/tickets"));
    let mut input = NewTicket::new(title);
    input.body = MarkdownText::from("Ready for panel action");
    input.workflow_state = Some(state);
    configure(&mut input);
    let ticket = backend.create(input).unwrap();
    (temp, ticket.id, backend)
}

fn ready_ticket_workspace(title: &str) -> (TempDir, String, LocalTicketBackend) {
    ticket_workspace(title, TicketWorkflowState::Ready, |_| {})
}

fn ready_ticket_git_workspace(title: &str) -> (TempDir, String, LocalTicketBackend) {
    let (temp, ticket_id, backend) = ready_ticket_workspace(title);
    run_test_git(temp.path(), &["init"]).unwrap();
    run_test_git(
        temp.path(),
        &["config", "user.email", "test@example.invalid"],
    )
    .unwrap();
    run_test_git(temp.path(), &["config", "user.name", "Yoi Test"]).unwrap();
    run_test_git(temp.path(), &["add", "."]).unwrap();
    run_test_git(temp.path(), &["commit", "-m", "seed tickets"]).unwrap();
    ensure_orchestration_worktree(temp.path()).unwrap();
    (temp, ticket_id, backend)
}

fn done_ticket_workspace(title: &str) -> (TempDir, String, LocalTicketBackend) {
    ticket_workspace(title, TicketWorkflowState::Done, |_| {})
}

fn request_for(temp: &TempDir, ticket_id: String, action: NextUserAction) -> TicketActionRequest {
    TicketActionRequest {
        workspace_root: temp.path().to_path_buf(),
        ticket_id,
        action,
        orchestrator: None,
    }
}

fn planning_return_request(
    temp: &TempDir,
    ticket_id: String,
    instruction: &str,
) -> ReadyTicketPlanningReturnRequest {
    ReadyTicketPlanningReturnRequest {
        workspace_root: temp.path().to_path_buf(),
        ticket_id,
        user_instruction: instruction.to_string(),
        followup: ReadyTicketPlanningReturnFollowup::BlockedByStaleClaim {
            pod_name: "stale-intake".to_string(),
        },
    }
}

#[tokio::test]
async fn ready_ticket_planning_return_records_instruction_and_returns_to_planning_without_queueing()
{
    let (temp, ticket_id, backend) = ready_ticket_workspace("panel-refine-ready");

    let outcome = dispatch_ready_ticket_planning_return(planning_return_request(
        &temp,
        ticket_id.clone(),
        "please add acceptance detail before queueing",
    ))
    .await
    .unwrap();

    assert!(outcome.notice.contains("returned to planning"));
    assert!(outcome.notice.contains("instruction was recorded"));
    assert!(matches!(
        outcome.followup,
        ReadyTicketPlanningReturnAfterMutation::None
    ));
    let ticket = backend.show(TicketIdOrSlug::Id(ticket_id.clone())).unwrap();
    assert_eq!(ticket.meta.workflow_state, TicketWorkflowState::Planning);
    assert!(ticket.meta.queued_by.is_none());
    assert!(ticket.meta.queued_at.is_none());
    let state_change = ticket
        .events
        .iter()
        .find(|event| {
            event.kind == TicketEventKind::StateChanged
                && event.state_field.as_deref() == Some("state")
                && event.from.as_deref() == Some("ready")
                && event.to.as_deref() == Some("planning")
        })
        .expect("ready -> planning state_changed event is recorded");
    assert_eq!(state_change.author.as_deref(), Some("workspace-panel"));
    assert!(
        state_change
            .body
            .as_str()
            .contains("please add acceptance detail")
    );
    assert!(state_change.body.as_str().contains("not Queue routing"));
    assert!(
        state_change
            .body
            .as_str()
            .contains("must not start implementation")
    );
}

#[tokio::test]
async fn ready_ticket_planning_return_rejects_stale_non_ready_ticket() {
    let (temp, ticket_id, backend) =
        ticket_workspace("panel-refine-stale", TicketWorkflowState::Planning, |_| {});

    let error = dispatch_ready_ticket_planning_return(planning_return_request(
        &temp,
        ticket_id.clone(),
        "refine please",
    ))
    .await
    .unwrap_err();

    assert!(error.to_string().contains("expected ready"));
    let ticket = backend.show(TicketIdOrSlug::Id(ticket_id)).unwrap();
    assert_eq!(ticket.meta.workflow_state, TicketWorkflowState::Planning);
    assert!(
        ticket
            .events
            .iter()
            .all(|event| !(event.kind == TicketEventKind::StateChanged
                && event.from.as_deref() == Some("ready")
                && event.to.as_deref() == Some("planning")))
    );
}

#[test]
fn ready_ticket_intake_enter_prepares_planning_return_not_queue_or_generic_launch() {
    let mut panel = WorkspacePanelViewModel::empty(Path::new("test"));
    panel.composer = crate::workspace_panel::WorkspacePanelComposer::ticket_enabled();
    panel.rows.push(panel_test_ticket_row(
        "20260608-000123-ready",
        "Ready Ticket",
        ActionPriority::ReadyForQueue,
        NextUserAction::Queue,
        "ready",
    ));
    let mut app = app_with_panel(empty_test_list(), panel);
    app.select_next();
    app.cycle_composer_target();
    app.input.insert_str("clarify expected behavior");

    let request = match app.handle_key(key(KeyCode::Enter)) {
        DashboardAction::ReturnReadyTicketToPlanning(request) => request,
        _ => panic!("ready Ticket row with Ticket Intake text should return to planning"),
    };

    assert_eq!(request.ticket_id, "20260608-000123-ready");
    assert_eq!(request.user_instruction, "clarify expected behavior");
    assert!(matches!(
        request.followup,
        ReadyTicketPlanningReturnFollowup::LaunchIntake(_)
    ));
    assert!(app.sending);
    assert!(
        app.notice
            .as_deref()
            .unwrap()
            .contains("Returning ready Ticket")
    );
    assert_eq!(input_text(&app), "clarify expected behavior");
}

#[tokio::test]
async fn planning_return_with_launch_followup_changes_state_before_launch_followup() {
    let (temp, ticket_id, backend) = ready_ticket_workspace("panel-refine-launch");
    let request = ReadyTicketPlanningReturnRequest {
        workspace_root: temp.path().to_path_buf(),
        ticket_id: ticket_id.clone(),
        user_instruction: "launch intake after state change".to_string(),
        followup: ReadyTicketPlanningReturnFollowup::LaunchIntake(IntakeLaunchRequest {
            context: TicketRoleLaunchContext::new(temp.path().to_path_buf(), TicketRole::Intake),
            runtime_command: PodRuntimeCommand::for_executable("/tmp/yoi"),
            peer_registration: IntakePeerRegistrationRequest::Skip {
                reason: "test".to_string(),
            },
            registry_update: IntakeRegistryUpdate::ClaimLaunchedTicket {
                registry_root: temp.path().join(".yoi/local-role-sessions"),
                ticket_id: ticket_id.clone(),
                ticket_slug: None,
            },
        }),
    };

    let outcome = dispatch_ready_ticket_planning_return(request)
        .await
        .unwrap();

    assert!(matches!(
        outcome.followup,
        ReadyTicketPlanningReturnAfterMutation::LaunchIntake(_)
    ));
    let ticket = backend.show(TicketIdOrSlug::Id(ticket_id)).unwrap();
    assert_eq!(ticket.meta.workflow_state, TicketWorkflowState::Planning);
}

#[tokio::test]
async fn ticket_queue_action_transitions_ready_ticket_and_authorizes_orchestrator_routing() {
    let (temp, ticket_id, backend) = ready_ticket_git_workspace("panel-queue");
    let root_head_before = git_rev_parse(temp.path(), "HEAD").unwrap();

    let outcome =
        dispatch_ticket_action(request_for(&temp, ticket_id.clone(), NextUserAction::Queue))
            .await
            .unwrap();

    let root_head_after = git_rev_parse(temp.path(), "HEAD").unwrap();
    let layout = orchestration_worktree_layout(temp.path());
    let orchestration_head = git_rev_parse(&layout.path, "HEAD").unwrap();
    assert_ne!(root_head_after, root_head_before);
    assert_eq!(orchestration_head, root_head_after);
    assert!(outcome.notice.contains("Queued Ticket"));
    assert!(outcome.notice.contains(&root_head_after));
    assert!(outcome.notice.contains("root Queue commit"));
    assert!(outcome.notice.contains("ff-only synced"));
    assert!(
        outcome
            .notice
            .contains("Orchestrator routing is authorized")
    );
    assert!(outcome.notice.contains("queued -> inprogress acceptance"));
    assert!(!outcome.notice.contains("No implementation was started"));
    let ticket = backend.show(TicketIdOrSlug::Id(ticket_id.clone())).unwrap();
    assert_eq!(ticket.meta.workflow_state, TicketWorkflowState::Queued);
    assert_eq!(ticket.meta.queued_by.as_deref(), Some("workspace-panel"));
    assert!(ticket.meta.queued_at.is_some());
    let state_change = ticket
        .events
        .iter()
        .find(|event| {
            event.kind == TicketEventKind::StateChanged
                && event.state_field.as_deref() == Some("state")
                && event.from.as_deref() == Some("ready")
                && event.to.as_deref() == Some("queued")
        })
        .expect("queue state_changed event is recorded");
    assert_eq!(state_change.author.as_deref(), Some("workspace-panel"));
    let orchestration_backend = LocalTicketBackend::new(layout.path.join(".yoi/tickets"));
    let orchestration_ticket = orchestration_backend
        .show(TicketIdOrSlug::Id(ticket_id))
        .unwrap();
    assert_eq!(
        orchestration_ticket.meta.workflow_state,
        TicketWorkflowState::Queued
    );
}

#[tokio::test]
async fn ticket_queue_action_allows_unrelated_dirty_root_changes() {
    let (temp, ticket_id, backend) = ready_ticket_git_workspace("panel-dirty-root");
    fs::write(temp.path().join("README.md"), "dirty root notes\n").unwrap();
    fs::write(temp.path().join("dirty.txt"), "dirty").unwrap();

    let outcome =
        dispatch_ticket_action(request_for(&temp, ticket_id.clone(), NextUserAction::Queue))
            .await
            .unwrap();

    assert!(outcome.notice.contains("Queued Ticket"));
    assert!(temp.path().join("dirty.txt").is_file());
    let root_status = git_status_porcelain(temp.path()).unwrap();
    assert!(root_status.iter().any(|line| line.contains("README.md")));
    assert!(root_status.iter().any(|line| line.contains("dirty.txt")));
    let ticket = backend.show(TicketIdOrSlug::Id(ticket_id)).unwrap();
    assert_eq!(ticket.meta.workflow_state, TicketWorkflowState::Queued);
    assert_eq!(ticket.meta.queued_by.as_deref(), Some("workspace-panel"));
}

#[tokio::test]
async fn ticket_queue_action_blocks_preexisting_target_ticket_changes() {
    let (temp, ticket_id, backend) = ready_ticket_git_workspace("panel-dirty-ticket");
    fs::write(
        backend.root().join(&ticket_id).join("thread.md"),
        "local uncommitted ticket edit\n",
    )
    .unwrap();

    let error =
        dispatch_ticket_action(request_for(&temp, ticket_id.clone(), NextUserAction::Queue))
            .await
            .unwrap_err();
    let message = error.to_string();

    assert!(message.contains("root-ticket-clean"));
    assert!(message.contains(&ticket_id));
    assert!(message.contains("pre-existing changes"));
    let ticket = backend.show(TicketIdOrSlug::Id(ticket_id)).unwrap();
    assert_eq!(ticket.meta.workflow_state, TicketWorkflowState::Ready);
    assert!(ticket.meta.queued_by.is_none());
}

#[tokio::test]
async fn ticket_queue_action_merges_orchestration_branch_before_queue() {
    let (temp, ticket_id, backend) = ready_ticket_git_workspace("panel-diverged");
    let layout = orchestration_worktree_layout(temp.path());
    fs::write(layout.path.join("orchestrator-only.txt"), "diverged").unwrap();
    run_test_git(&layout.path, &["add", "orchestrator-only.txt"]).unwrap();
    run_test_git(&layout.path, &["commit", "-m", "orchestrator-only"]).unwrap();
    let orchestration_commit = git_rev_parse(&layout.path, "HEAD").unwrap();

    let outcome =
        dispatch_ticket_action(request_for(&temp, ticket_id.clone(), NextUserAction::Queue))
            .await
            .unwrap();

    let root_head = git_rev_parse(temp.path(), "HEAD").unwrap();
    let orchestration_head = git_rev_parse(&layout.path, "HEAD").unwrap();
    assert_eq!(root_head, orchestration_head);
    assert!(temp.path().join("orchestrator-only.txt").is_file());
    assert!(outcome.notice.contains("merged orchestration branch"));
    assert!(outcome.notice.contains(&orchestration_commit));
    assert!(outcome.notice.contains("root Queue commit"));
    let ticket = backend.show(TicketIdOrSlug::Id(ticket_id)).unwrap();
    assert_eq!(ticket.meta.workflow_state, TicketWorkflowState::Queued);
    assert_eq!(ticket.meta.queued_by.as_deref(), Some("workspace-panel"));
}

#[tokio::test]
async fn ticket_queue_action_blocks_conflicting_orchestration_merge_without_mutation() {
    let (temp, ticket_id, backend) = ready_ticket_git_workspace("panel-conflict");
    let layout = orchestration_worktree_layout(temp.path());
    fs::write(temp.path().join("README.md"), "root change\n").unwrap();
    run_test_git(temp.path(), &["add", "README.md"]).unwrap();
    run_test_git(temp.path(), &["commit", "-m", "root-change"]).unwrap();
    fs::write(layout.path.join("README.md"), "orchestration change\n").unwrap();
    run_test_git(&layout.path, &["add", "README.md"]).unwrap();
    run_test_git(&layout.path, &["commit", "-m", "orchestration-change"]).unwrap();
    let root_head_before = git_rev_parse(temp.path(), "HEAD").unwrap();

    let error =
        dispatch_ticket_action(request_for(&temp, ticket_id.clone(), NextUserAction::Queue))
            .await
            .unwrap_err();
    let message = error.to_string();

    assert!(message.contains("root-orchestration-merge"));
    assert!(message.contains("merge was aborted"));
    assert!(message.contains(&ticket_id));
    assert_eq!(
        git_rev_parse(temp.path(), "HEAD").unwrap(),
        root_head_before
    );
    assert!(git_status_porcelain(temp.path()).unwrap().is_empty());
    let ticket = backend.show(TicketIdOrSlug::Id(ticket_id)).unwrap();
    assert_eq!(ticket.meta.workflow_state, TicketWorkflowState::Ready);
    assert!(ticket.meta.queued_by.is_none());
}

#[tokio::test]
async fn ticket_close_action_blocks_non_done_ticket_without_mutation() {
    let (temp, ticket_id, backend) = ready_ticket_workspace("panel-not-done");

    let error =
        dispatch_ticket_action(request_for(&temp, ticket_id.clone(), NextUserAction::Close))
            .await
            .unwrap_err();

    assert!(error.to_string().contains("state is ready"));
    let ticket = backend.show(TicketIdOrSlug::Id(ticket_id)).unwrap();
    assert_eq!(ticket.meta.workflow_state, TicketWorkflowState::Ready);
    assert!(ticket.resolution.is_none());
}

#[tokio::test]
async fn ticket_action_rejects_stale_absent_config_without_mutation() {
    let (temp, ticket_id, backend) = ready_ticket_workspace("panel-no-config");
    fs::remove_file(temp.path().join(".yoi/ticket.config.toml")).unwrap();

    let error =
        dispatch_ticket_action(request_for(&temp, ticket_id.clone(), NextUserAction::Queue))
            .await
            .unwrap_err();

    assert!(error.to_string().contains("Ticket config is absent"));
    let ticket = backend.show(TicketIdOrSlug::Id(ticket_id)).unwrap();
    assert_eq!(ticket.meta.workflow_state, TicketWorkflowState::Ready);
    assert!(ticket.meta.queued_by.is_none());
    assert!(!ticket.events.iter().any(|event| {
        event.kind == TicketEventKind::StateChanged && event.state_field.as_deref() == Some("state")
    }));
}

#[tokio::test]
async fn ticket_close_action_closes_done_ticket_with_deterministic_resolution() {
    let (temp, ticket_id, backend) = done_ticket_workspace("panel-close");

    let outcome =
        dispatch_ticket_action(request_for(&temp, ticket_id.clone(), NextUserAction::Close))
            .await
            .unwrap();

    assert!(outcome.notice.contains("Closed Ticket"));
    assert!(outcome.notice.contains("state was already done"));
    let ticket = backend.show(TicketIdOrSlug::Id(ticket_id)).unwrap();
    assert_eq!(ticket.meta.workflow_state, TicketWorkflowState::Closed);
    let resolution = ticket
        .resolution
        .as_ref()
        .expect("Dashboard Close records resolution.md")
        .as_str();
    assert!(resolution.contains("state: done"));
    assert!(resolution.contains("No implementation work"));
    assert!(resolution.contains("state change"));
    assert!(resolution.contains("worker invocation"));
    assert!(ticket.events.iter().any(|event| {
        event.kind == TicketEventKind::Close && event.body.as_str().contains("workspace Dashboard")
    }));
}

#[tokio::test]
async fn ticket_close_action_blocks_existing_resolution_without_moving_ticket() {
    let (temp, ticket_id, backend) = done_ticket_workspace("panel-close-resolution");
    fs::write(
        temp.path()
            .join(".yoi/tickets")
            .join(&ticket_id)
            .join("resolution.md"),
        "Already resolved\n",
    )
    .unwrap();

    let error =
        dispatch_ticket_action(request_for(&temp, ticket_id.clone(), NextUserAction::Close))
            .await
            .unwrap_err();

    assert!(error.to_string().contains("resolution.md already exists"));
    let ticket = backend.show(TicketIdOrSlug::Id(ticket_id)).unwrap();
    assert_eq!(
        ticket.resolution.as_ref().unwrap().as_str(),
        "Already resolved\n"
    );
}

#[tokio::test]
async fn ticket_review_action_does_not_silently_approve() {
    let (temp, ticket_id, backend) = ready_ticket_workspace("panel-review");
    backend
        .add_event(
            TicketIdOrSlug::Id(ticket_id.clone()),
            NewTicketEvent::new(TicketEventKind::ImplementationReport, "implemented"),
        )
        .unwrap();

    let error = dispatch_ticket_action(request_for(&temp, ticket_id.clone(), NextUserAction::Wait))
        .await
        .unwrap_err();

    assert!(error.to_string().contains("current action is Queue"));
    let ticket = backend.show(TicketIdOrSlug::Id(ticket_id)).unwrap();
    assert!(
        !ticket
            .events
            .iter()
            .any(|event| event.kind == TicketEventKind::Review)
    );
}

#[test]
fn ticket_queue_notification_message_carries_routing_contract() {
    let row = panel_test_ticket_row(
        "00001KTTW04W2",
        "Route queued\nTicket",
        ActionPriority::ReadyForQueue,
        NextUserAction::Queue,
        "queued",
    );
    let ticket = row.ticket.as_ref().unwrap();

    let message = orchestrator_queue_notification_message(ticket);

    assert!(message.contains("Ticket `00001KTTW04W2`, title `Route queued Ticket`"));
    assert!(message.contains("human authorized Orchestrator routing"));
    assert!(message.contains("not an unattended scheduler"));
    assert!(message.contains("Read the Ticket"));
    assert!(message.contains("inspect current Orchestrator workspace state"));
    assert!(message.contains("transition state queued -> inprogress"));
    assert!(message.contains("before any worktree/SpawnPod implementation side effects"));
    assert!(message.contains("After inprogress acceptance"));
    assert!(message.contains("worktree-workflow"));
    assert!(message.contains("`.worktree/<task-name>`"));
    assert!(message.contains("tracked `.yoi` project records visible"));
    assert!(
        message.contains(
            "`.yoi/memory` plus local/runtime/log/lock/secret-like `.yoi` paths excluded"
        )
    );
    assert!(message.contains("multi-agent-workflow"));
    assert!(message.contains("sibling coder/reviewer Pods"));
    assert!(message.contains("coder narrow child-worktree write scope"));
    assert!(message.contains("reviewer read-only by default"));
    assert!(message.contains(
        "integrate the implementation branch into the orchestration branch automatically"
    ));
    assert!(message.contains("validate in the Orchestrator worktree"));
    assert!(message.contains("clean up only child implementation worktrees/branches"));
    assert!(message.contains("Do not read, write, validate, merge, clean up, or run git operations in the root/original workspace"));
    assert!(message.contains("If blocked, record a concise reason"));
    assert!(message.contains("leave the Ticket queued or return it to planning"));
    assert!(!message.contains("Do not start implementation directly"));
}

#[tokio::test]
async fn ticket_queue_notification_sends_notify_when_socket_available() {
    let temp = TempDir::new().unwrap();
    let socket_path = temp.path().join("orchestrator.sock");
    let listener = tokio::net::UnixListener::bind(&socket_path).unwrap();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let (reader, writer) = stream.into_split();
        let mut reader = JsonLineReader::new(reader);
        let mut writer = JsonLineWriter::new(writer);
        writer
            .write(&Event::Snapshot {
                entries: Vec::new(),
                greeting: protocol::Greeting {
                    pod_name: "test-orchestrator".to_string(),
                    cwd: temp.path().display().to_string(),
                    provider: "test".to_string(),
                    model: "test".to_string(),
                    scope_summary: "test".to_string(),
                    tools: Vec::new(),
                    context_window: 0,
                    context_tokens: 0,
                },
                status: PodStatus::Idle,
                in_flight: Default::default(),
            })
            .await
            .unwrap();
        reader.next::<Method>().await.unwrap().unwrap()
    });

    send_notify_only(&socket_path, "Dashboard Queue".to_string(), true)
        .await
        .unwrap();
    let method = server.await.unwrap();
    assert!(matches!(
        method,
        Method::Notify { message, auto_run: true } if message == "Dashboard Queue"
    ));
}

#[tokio::test]
async fn send_notify_only_can_deliver_weak_notification_without_auto_run() {
    let temp = TempDir::new().unwrap();
    let socket_path = temp.path().join("companion.sock");
    let listener = tokio::net::UnixListener::bind(&socket_path).unwrap();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let (reader, writer) = stream.into_split();
        let mut reader = JsonLineReader::new(reader);
        let mut writer = JsonLineWriter::new(writer);
        writer
            .write(&Event::Snapshot {
                entries: Vec::new(),
                greeting: protocol::Greeting {
                    pod_name: "yoi".to_string(),
                    cwd: temp.path().display().to_string(),
                    provider: "test".to_string(),
                    model: "test".to_string(),
                    scope_summary: "test".to_string(),
                    tools: Vec::new(),
                    context_window: 0,
                    context_tokens: 0,
                },
                status: PodStatus::Idle,
                in_flight: Default::default(),
            })
            .await
            .unwrap();
        reader.next::<Method>().await.unwrap().unwrap()
    });

    send_notify_only(&socket_path, "Dashboard progress".to_string(), false)
        .await
        .unwrap();
    let method = server.await.unwrap();
    assert!(matches!(
        method,
        Method::Notify { message, auto_run: false } if message == "Dashboard progress"
    ));
}

#[test]
fn no_ticket_selection_keeps_enter_pod_centric() {
    let mut app = test_app(vec![live_info("alpha", PodStatus::Idle)]);

    assert!(matches!(
        app.handle_key(key(KeyCode::Enter)),
        DashboardAction::Open
    ));
    assert!(app.prepare_ticket_action_dispatch().is_none());
    assert_eq!(app.notice.as_deref(), Some("No Ticket action is selected."));
}

#[test]
fn workspace_panel_initial_display_does_not_auto_select_visible_rows() {
    let mut panel = WorkspacePanelViewModel::empty(Path::new("test"));
    panel.rows.push(panel_test_ticket_row(
        "TICKET-1",
        "Ready",
        ActionPriority::ReadyForQueue,
        NextUserAction::Queue,
        "ready",
    ));
    let app = app_with_panel(
        PodList::from_sources(
            PodVisibilitySource::ResumePicker,
            vec![],
            vec![live_info("alpha", PodStatus::Idle)],
            None,
            10,
        ),
        panel,
    );

    assert!(visible_panel_keys(&app.panel, &app.list).len() > 1);
    assert!(app.selected_row.is_none());
    assert!(app.list.selected_name.is_none());
}

#[test]
fn workspace_panel_clear_selection_survives_reload_and_keeps_draft() {
    let mut app = test_app(vec![live_info_with_updated_at(
        "alpha",
        PodStatus::Idle,
        10,
    )]);
    app.select_next();
    assert_eq!(
        app.selected_row,
        Some(PanelRowKey::Pod("alpha".to_string()))
    );
    app.input.insert_str("draft survives");

    assert!(matches!(
        app.handle_key(key(KeyCode::Esc)),
        DashboardAction::None
    ));
    assert!(app.selected_row.is_none());
    assert!(app.list.selected_name.is_none());

    app.apply_reloaded_list(PodList::from_sources(
        PodVisibilitySource::ResumePicker,
        vec![],
        vec![live_info_with_updated_at("alpha", PodStatus::Running, 20)],
        None,
        10,
    ));

    assert!(app.selected_row.is_none());
    assert!(app.list.selected_name.is_none());
    assert_eq!(input_text(&app), "draft survives");
}

#[test]
fn workspace_panel_no_selection_ticket_intake_submit_uses_global_intake() {
    let mut panel = WorkspacePanelViewModel::empty(Path::new("test"));
    panel.composer = crate::workspace_panel::WorkspacePanelComposer::ticket_enabled();
    panel.rows.push(panel_test_ticket_row(
        "TICKET-1",
        "Ready",
        ActionPriority::ReadyForQueue,
        NextUserAction::Queue,
        "ready",
    ));
    let mut app = app_with_panel(empty_test_list(), panel);
    app.cycle_composer_target();
    app.input.insert_str("new planning request");

    let request = match app.handle_key(key(KeyCode::Enter)) {
        DashboardAction::LaunchIntake(request) => request,
        _ => panic!("no selection should launch global Intake"),
    };

    assert!(request.context.ticket.is_none());
    assert_eq!(
        request.context.user_instruction.as_deref(),
        Some("new planning request")
    );
}

#[test]
fn workspace_panel_keyboard_navigation_explicitly_creates_selection() {
    let mut app = test_app(vec![
        live_info("alpha", PodStatus::Idle),
        live_info("beta", PodStatus::Idle),
    ]);
    assert!(app.selected_row.is_none());

    app.select_next();
    assert_eq!(
        app.selected_row,
        Some(PanelRowKey::Pod("alpha".to_string()))
    );

    app.select_next();
    assert_eq!(app.selected_row, Some(PanelRowKey::Pod("beta".to_string())));
}

#[test]
fn dashboard_ticket_action_rows_precede_pods_and_pod_actions_still_work() {
    let temp = TempDir::new().unwrap();
    fs::create_dir_all(temp.path().join(".yoi")).unwrap();
    fs::write(
        temp.path().join(".yoi/ticket.config.toml"),
        "[backend]\nprovider = \"builtin:yoi_local\"\nroot = \".yoi/tickets\"\n",
    )
    .unwrap();
    let backend = LocalTicketBackend::new(temp.path().join(".yoi/tickets"));
    let mut ticket = NewTicket::new("Ready Ticket");
    ticket.workflow_state = Some(TicketWorkflowState::Ready);
    backend.create(ticket).unwrap();
    let list = PodList::from_sources(
        PodVisibilitySource::ResumePicker,
        vec![],
        vec![live_info("idle", PodStatus::Idle)],
        None,
        10,
    );
    let panel = build_workspace_panel(temp.path(), &list);
    let mut app = app_with_panel(list, panel);
    assert!(app.selected_row.is_none());
    app.select_next();

    assert_eq!(app.selected_panel_row().unwrap().title, "Ready Ticket");
    assert_eq!(app.selected_open_eligibility(), OpenEligibility::Disabled);
    let lines = list_lines(&app, 100, 6)
        .into_iter()
        .map(|line| plain_line(&line))
        .collect::<Vec<_>>();
    let ticket_line = lines
        .iter()
        .position(|line| line.contains("Ready Ticket"))
        .unwrap();
    let pod_line = lines.iter().position(|line| line.contains("idle")).unwrap();
    assert!(ticket_line < pod_line);

    app.select_next();
    assert_eq!(app.list.selected_entry().unwrap().name, "idle");
    assert_eq!(app.selected_open_eligibility(), OpenEligibility::OpenNow);
    let open = app.prepare_open().unwrap();
    assert_eq!(open.pod_name, "idle");
    assert_eq!(open.socket_override, Some(PathBuf::from("/tmp/idle.sock")));

    app.input.insert_str("draft after ticket row");
    assert!(matches!(
        app.handle_key(key(KeyCode::Enter)),
        DashboardAction::None
    ));
    assert!(!app.sending);
    assert_eq!(input_text(&app), "draft after ticket row");
    assert!(
        app.notice
            .as_deref()
            .unwrap()
            .contains("Workspace Companion is unavailable")
    );
}

#[test]
fn row_hit_testing_maps_only_visible_selectable_rows() {
    let mut panel = WorkspacePanelViewModel::empty(Path::new("test"));
    panel.rows.push(panel_test_ticket_row(
        "TICKET-1",
        "Ready",
        ActionPriority::ReadyForQueue,
        NextUserAction::Queue,
        "ready",
    ));
    panel.rows.push(panel_test_ticket_row(
        "TICKET-2",
        "Queued",
        ActionPriority::Background,
        NextUserAction::Wait,
        "queued",
    ));
    let list = PodList::from_sources(
        PodVisibilitySource::ResumePicker,
        vec![],
        vec![live_info("alpha", PodStatus::Idle)],
        None,
        10,
    );
    let app = app_with_panel(list, panel);

    let rows = list_rows(&app, 80, 8);
    let boxes = row_hit_boxes(&rows, Rect::new(3, 5, 80, 8));

    assert_eq!(boxes.len(), 3);
    assert_eq!(boxes[0].key, PanelRowKey::Ticket("TICKET-1".into()));
    assert_eq!(boxes[0].rect, Rect::new(3, 6, 80, 2));
    assert_eq!(boxes[1].key, PanelRowKey::Ticket("TICKET-2".into()));
    assert_eq!(boxes[1].rect, Rect::new(3, 8, 80, 2));
    assert_eq!(boxes[2].key, PanelRowKey::Pod("alpha".into()));
    assert_eq!(boxes[2].rect, Rect::new(3, 11, 80, 1));
    assert!(boxes.iter().all(|hit| !hit.contains(2, hit.rect.y)));
}

#[test]
fn mouse_click_selects_panel_row_for_blank_enter_action() {
    let mut panel = WorkspacePanelViewModel::empty(Path::new("test"));
    panel.rows.push(panel_test_ticket_row(
        "TICKET-1",
        "Ready",
        ActionPriority::ReadyForQueue,
        NextUserAction::Queue,
        "ready",
    ));
    panel.rows.push(panel_test_ticket_row(
        "TICKET-2",
        "Queued",
        ActionPriority::Background,
        NextUserAction::Wait,
        "queued",
    ));
    let mut app = app_with_panel(
        PodList::from_sources(PodVisibilitySource::ResumePicker, vec![], vec![], None, 10),
        panel,
    );
    let rows = list_rows(&app, 80, 6);
    app.set_row_hit_boxes(&rows, Rect::new(0, 0, 80, 6));

    assert!(app.handle_mouse_event(left_click(2, 3)));
    assert_eq!(
        app.selected_row,
        Some(PanelRowKey::Ticket("TICKET-2".into()))
    );
    app.selected_row = None;
    assert!(app.handle_mouse_event(left_click(2, 4)));
    assert_eq!(
        app.selected_row,
        Some(PanelRowKey::Ticket("TICKET-2".into()))
    );
    assert_eq!(app.selected_panel_row().unwrap().title, "Queued");
    assert_eq!(app.selected_ticket_action(), Some(NextUserAction::Wait));
    let selected_title =
        plain_line(&panel_row_lines(app.selected_panel_row().unwrap(), true, 80)[0]);
    assert!(selected_title.starts_with("▶ queued"));
    assert!(matches!(
        app.handle_key(key(KeyCode::Enter)),
        DashboardAction::DispatchTicketAction(request) if request.ticket_id == "TICKET-2"
    ));
}

#[test]
fn mouse_non_row_click_is_noop_and_preserves_composer_draft() {
    let mut panel = WorkspacePanelViewModel::empty(Path::new("test"));
    panel.rows.push(panel_test_ticket_row(
        "TICKET-1",
        "Ready",
        ActionPriority::ReadyForQueue,
        NextUserAction::Queue,
        "ready",
    ));
    let mut app = app_with_panel(
        PodList::from_sources(PodVisibilitySource::ResumePicker, vec![], vec![], None, 10),
        panel,
    );
    let rows = list_rows(&app, 80, 6);
    app.set_row_hit_boxes(&rows, Rect::new(10, 4, 80, 6));
    app.input.insert_paste("draft".into());
    let selected = app.selected_row.clone();

    assert!(!app.handle_mouse_event(left_click(9, 5)));
    assert_eq!(app.selected_row, selected);
    assert_eq!(input_text(&app), "draft");
}

#[test]
fn mouse_click_does_not_override_existing_composer_keyboard_behavior() {
    let mut panel = WorkspacePanelViewModel::empty(Path::new("test"));
    panel.rows.push(panel_test_ticket_row(
        "TICKET-1",
        "Ready",
        ActionPriority::ReadyForQueue,
        NextUserAction::Queue,
        "ready",
    ));
    panel.rows.push(panel_test_ticket_row(
        "TICKET-2",
        "Queued",
        ActionPriority::Background,
        NextUserAction::Wait,
        "queued",
    ));
    let mut app = app_with_panel(
        PodList::from_sources(PodVisibilitySource::ResumePicker, vec![], vec![], None, 10),
        panel,
    );
    let rows = list_rows(&app, 80, 6);
    app.set_row_hit_boxes(&rows, Rect::new(0, 0, 80, 6));

    assert!(app.handle_mouse_event(left_click(2, 4)));
    assert_eq!(
        app.selected_row,
        Some(PanelRowKey::Ticket("TICKET-2".into()))
    );
    app.input.insert_paste("hello".into());
    assert!(matches!(
        app.handle_key(key(KeyCode::Enter)),
        DashboardAction::None
    ));
    assert_eq!(input_text(&app), "hello");
    assert!(matches!(
        app.handle_key(key(KeyCode::Esc)),
        DashboardAction::None
    ));
    assert_eq!(app.selected_row, None);
    assert_eq!(input_text(&app), "hello");
    assert!(matches!(
        app.handle_key(key(KeyCode::Down)),
        DashboardAction::None
    ));
    assert_eq!(app.selected_row, None);
}

#[test]
fn selected_ticket_row_with_non_empty_composer_hides_redundant_status_hints() {
    let mut panel = WorkspacePanelViewModel::empty(Path::new("test"));
    panel.header.companion = Some(CompanionPanelState::new(
        "yoi",
        CompanionPanelStatus::Live,
        None,
    ));
    panel.rows.push(panel_test_ticket_row(
        "00001KTWPE3KQ",
        "Queue Me",
        ActionPriority::ReadyForQueue,
        NextUserAction::Queue,
        "ready",
    ));
    let list = PodList::from_sources(
        PodVisibilitySource::ResumePicker,
        vec![],
        vec![live_info("yoi", PodStatus::Idle)],
        None,
        10,
    );
    let mut app = app_with_panel(list, panel);
    app.select_next();
    app.input.insert_str("draft to companion");

    assert_eq!(
        app.selected_ticket_action(),
        Some(NextUserAction::Queue),
        "selected row remains a Ticket action row"
    );
    let actionbar_left = actionbar_left_text(&app);
    let actionbar_right = actionbar_right_text(&app);
    let target_status = plain_line(&target_status_line(&app));

    assert_eq!(actionbar_left, "");
    assert_eq!(actionbar_right, "");
    assert_eq!(target_status, "");
    let selected_title =
        plain_line(&panel_row_lines(app.selected_panel_row().unwrap(), true, 80)[0]);
    assert!(selected_title.starts_with("▶ ready"));
}

#[test]
fn dashboard_bare_panel_letters_append_to_composer_and_arrows_select_when_blank() {
    let mut app = test_app(vec![
        live_info("alpha", PodStatus::Idle),
        live_info("beta", PodStatus::Idle),
    ]);
    assert!(app.selected_row.is_none());

    for c in ['j', 'k', 'o', 'r'] {
        assert!(matches!(
            app.handle_key(key(KeyCode::Char(c))),
            DashboardAction::None
        ));
    }

    assert_eq!(input_text(&app), "jkor");
    assert!(app.selected_row.is_none());

    assert!(matches!(
        app.handle_key(key(KeyCode::Down)),
        DashboardAction::None
    ));
    assert_eq!(input_text(&app), "jkor");
    assert!(app.selected_row.is_none());

    app.input.clear();
    assert!(matches!(
        app.handle_key(key(KeyCode::Down)),
        DashboardAction::None
    ));
    assert_eq!(input_text(&app), "");
    assert_eq!(app.list.selected_entry().unwrap().name, "alpha");

    assert!(matches!(
        app.handle_key(key(KeyCode::Down)),
        DashboardAction::None
    ));
    assert_eq!(app.list.selected_entry().unwrap().name, "beta");

    assert!(matches!(
        app.handle_key(key(KeyCode::Up)),
        DashboardAction::None
    ));
    assert_eq!(input_text(&app), "");
    assert_eq!(app.list.selected_entry().unwrap().name, "alpha");
}

#[test]
fn dashboard_selection_changes_preserve_composer_contents() {
    let mut app = test_app(vec![
        live_info("alpha", PodStatus::Idle),
        live_info("beta", PodStatus::Idle),
    ]);
    app.input.insert_str("draft message");
    let before = input_text(&app);

    app.select_next();

    assert_eq!(input_text(&app), before);
    assert_eq!(app.list.selected_entry().unwrap().name, "alpha");
}

#[test]
fn dashboard_poll_reload_preserves_selection_composer_and_notice() {
    let mut app = test_app(vec![
        live_info_with_updated_at("alpha", PodStatus::Idle, 10),
        live_info_with_updated_at("beta", PodStatus::Idle, 20),
    ]);
    app.select_next();
    assert_eq!(app.list.selected_entry().unwrap().name, "beta");
    app.input.insert_str("draft survives polling");
    app.notice = Some("keep this notice".to_string());
    let refreshed = PodList::from_sources(
        PodVisibilitySource::ResumePicker,
        vec![],
        vec![
            live_info_with_updated_at("gamma", PodStatus::Idle, 60),
            live_info_with_updated_at("alpha", PodStatus::Running, 50),
            live_info_with_updated_at("beta", PodStatus::Idle, 40),
        ],
        None,
        10,
    );

    app.apply_reloaded_list(refreshed);

    assert_eq!(app.list.selected_entry().unwrap().name, "beta");
    assert_eq!(
        app.list
            .selected_entry()
            .unwrap()
            .live
            .as_ref()
            .unwrap()
            .status,
        Some(PodStatus::Idle)
    );
    assert_eq!(input_text(&app), "draft survives polling");
    assert_eq!(app.notice.as_deref(), Some("keep this notice"));
}

#[test]
fn dashboard_poll_reload_falls_back_when_selected_pod_disappears() {
    let mut app = test_app(vec![
        live_info_with_updated_at("alpha", PodStatus::Idle, 10),
        live_info_with_updated_at("beta", PodStatus::Running, 20),
    ]);
    app.select_next();
    app.select_next();
    assert_eq!(app.list.selected_entry().unwrap().name, "beta");
    let refreshed = PodList::from_sources(
        PodVisibilitySource::ResumePicker,
        vec![stopped_info_with_updated_at("closed", 30)],
        vec![live_info_with_updated_at("alpha", PodStatus::Idle, 40)],
        None,
        10,
    );

    app.apply_reloaded_list(refreshed);

    assert!(app.selected_row.is_none());
    assert!(app.list.selected_name.is_none());
    assert_eq!(visible_entry_indices(&app.list), vec![0, 1]);
}

#[test]
fn dashboard_poll_reload_error_keeps_previous_list_and_composer() {
    let mut app = test_app(vec![live_info("alpha", PodStatus::Idle)]);
    app.input.insert_str("keep draft");

    app.apply_reload_result(Err(DashboardError::Io(io::Error::other("boom"))));

    assert_eq!(app.list.selected_entry().unwrap().name, "alpha");
    assert_eq!(input_text(&app), "keep draft");
    let notice = app.notice.as_deref().unwrap();
    assert!(notice.contains("Refresh failed"));
    assert!(notice.contains("boom"));
}

#[test]
fn dashboard_orchestrator_failure_persists_over_plain_observe_missing() {
    let detail = "could not spawn workspace Orchestrator: delegated scope conflicts with writer";
    let mut app = app_with_panel(
        empty_test_list(),
        panel_with_orchestrator(OrchestratorPanelStatus::Unavailable, Some(detail)),
    );

    app.apply_reloaded_snapshot(DashboardSnapshot {
        list: empty_test_list(),
        panel: panel_with_orchestrator(OrchestratorPanelStatus::Missing, None),
    });

    let orchestrator = app.panel.header.orchestrator.as_ref().unwrap();
    assert_eq!(orchestrator.status, OrchestratorPanelStatus::Unavailable);
    assert_eq!(orchestrator.detail.as_deref(), Some(detail));
    assert_eq!(
        app.panel
            .header
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.as_str() == detail)
            .count(),
        1
    );
}

#[test]
fn dashboard_orchestrator_plain_missing_remains_when_no_prior_failure_exists() {
    let mut app = app_with_panel(
        empty_test_list(),
        panel_with_orchestrator(OrchestratorPanelStatus::Missing, None),
    );

    app.apply_reloaded_snapshot(DashboardSnapshot {
        list: empty_test_list(),
        panel: panel_with_orchestrator(OrchestratorPanelStatus::Missing, None),
    });

    let orchestrator = app.panel.header.orchestrator.as_ref().unwrap();
    assert_eq!(orchestrator.status, OrchestratorPanelStatus::Missing);
    assert!(orchestrator.detail.is_none());
    assert!(app.panel.header.diagnostics.is_empty());
}

#[test]
fn dashboard_orchestrator_failure_clears_after_live_lifecycle() {
    let detail = "could not spawn workspace Orchestrator: delegated scope conflicts with writer";
    let mut app = app_with_panel(
        empty_test_list(),
        panel_with_orchestrator(OrchestratorPanelStatus::Unavailable, Some(detail)),
    );

    app.apply_reloaded_snapshot(DashboardSnapshot {
        list: empty_test_list(),
        panel: panel_with_orchestrator(OrchestratorPanelStatus::Live, None),
    });
    assert_eq!(
        app.panel.header.orchestrator.as_ref().unwrap().status,
        OrchestratorPanelStatus::Live
    );

    app.apply_reloaded_snapshot(DashboardSnapshot {
        list: empty_test_list(),
        panel: panel_with_orchestrator(OrchestratorPanelStatus::Missing, None),
    });

    let orchestrator = app.panel.header.orchestrator.as_ref().unwrap();
    assert_eq!(orchestrator.status, OrchestratorPanelStatus::Missing);
    assert!(orchestrator.detail.is_none());
}

#[test]
fn dashboard_orchestrator_failure_supersedes_prior_failure() {
    let old_detail = "could not spawn workspace Orchestrator: old scope conflict";
    let new_detail = "could not restore workspace Orchestrator: socket refused";
    let mut app = app_with_panel(
        empty_test_list(),
        panel_with_orchestrator(OrchestratorPanelStatus::Unavailable, Some(old_detail)),
    );

    app.apply_reloaded_snapshot(DashboardSnapshot {
        list: empty_test_list(),
        panel: panel_with_orchestrator(OrchestratorPanelStatus::Unavailable, Some(new_detail)),
    });
    app.apply_reloaded_snapshot(DashboardSnapshot {
        list: empty_test_list(),
        panel: panel_with_orchestrator(OrchestratorPanelStatus::Missing, None),
    });

    let orchestrator = app.panel.header.orchestrator.as_ref().unwrap();
    assert_eq!(orchestrator.status, OrchestratorPanelStatus::Unavailable);
    assert_eq!(orchestrator.detail.as_deref(), Some(new_detail));
    assert!(
        !app.panel
            .header
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic == old_detail)
    );
    assert!(
        app.panel
            .header
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic == new_detail)
    );
}

#[tokio::test]
async fn dashboard_poll_reload_does_not_overlap_in_flight_reload() {
    let mut app = test_app(vec![live_info("alpha", PodStatus::Idle)]);
    let mut pending = PendingReload::default();

    assert!(pending.start_with_handle(tokio::spawn(async {
        tokio::time::sleep(Duration::from_millis(10)).await;
        Err(DashboardError::Io(io::Error::other("boom")))
    })));
    assert!(!pending.start_with_handle(tokio::spawn(async {
        let list = PodList::from_sources(
            PodVisibilitySource::ResumePicker,
            vec![],
            vec![live_info("beta", PodStatus::Idle)],
            None,
            10,
        );
        Ok(DashboardSnapshot {
            panel: WorkspacePanelViewModel::empty(Path::new("test")),
            list,
        })
    })));
    assert!(pending.finish_if_ready().await.is_none());

    tokio::time::sleep(Duration::from_millis(20)).await;
    let result = pending.finish_if_ready().await.unwrap();
    app.apply_reload_result(result);

    assert_eq!(app.list.selected_entry().unwrap().name, "alpha");
    assert!(app.notice.as_deref().unwrap().contains("Refresh failed"));
}

#[tokio::test]
async fn dashboard_quit_aborts_background_reload_and_notice_without_waiting() {
    struct DropFlag(Arc<AtomicBool>);
    impl Drop for DropFlag {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    let reload_cancelled = Arc::new(AtomicBool::new(false));
    let notice_cancelled = Arc::new(AtomicBool::new(false));
    let mut pending_reload = PendingReload::default();
    let mut pending_notice = PendingQueueAttentionNotice::default();
    let (reload_started_tx, reload_started_rx) = tokio::sync::oneshot::channel();
    let (notice_started_tx, notice_started_rx) = tokio::sync::oneshot::channel();

    let reload_flag = Arc::clone(&reload_cancelled);
    assert!(pending_reload.start_with_handle(tokio::spawn(async move {
        let _drop_flag = DropFlag(reload_flag);
        let _ = reload_started_tx.send(());
        std::future::pending::<()>().await;
        Err(DashboardError::Io(io::Error::other(
            "unreachable reload completion",
        )))
    })));

    let notice_flag = Arc::clone(&notice_cancelled);
    assert!(pending_notice.start_with_handle(tokio::spawn(async move {
        let _drop_flag = DropFlag(notice_flag);
        let _ = notice_started_tx.send(());
        std::future::pending::<()>().await;
        OrchestratorQueueAttentionNoticeResult::failed(
            "unreachable".to_string(),
            "unreachable notice completion",
        )
    })));
    reload_started_rx.await.expect("reload task should start");
    notice_started_rx.await.expect("notice task should start");

    tokio::time::timeout(Duration::from_millis(20), async {
        abort_panel_background_work_for_quit(&mut pending_reload, &mut pending_notice);
    })
    .await
    .expect("quit abort should not wait for background task completion");

    tokio::time::timeout(Duration::from_millis(100), async {
        while !(reload_cancelled.load(Ordering::SeqCst) && notice_cancelled.load(Ordering::SeqCst))
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("quit abort should cancel reload and notice tasks");

    assert!(pending_reload.finish_if_ready().await.is_none());
    assert!(pending_notice.finish_if_ready().await.is_none());
}

#[test]
fn dashboard_idle_live_selected_target_is_open_eligible() {
    let mut app = test_app(vec![live_info("idle", PodStatus::Idle)]);
    app.select_next();

    assert_eq!(app.selected_open_eligibility(), OpenEligibility::OpenNow);
}

#[test]
fn dashboard_status_label_for_live_without_reported_status_is_softened() {
    let mut live = live_info("probing", PodStatus::Idle);
    live.status = None;
    let app = test_app(vec![live]);

    let (label, _) = row_status_label(app.list.selected_entry().unwrap());

    assert_eq!(label, "live");
}

#[test]
fn dashboard_status_labels_preserve_explicit_live_statuses() {
    for (status, expected_label) in [
        (PodStatus::Idle, "live idle"),
        (PodStatus::Running, "live running"),
        (PodStatus::Paused, "live paused"),
    ] {
        let app = test_app(vec![live_info("pod", status)]);
        let (label, _) = row_status_label(app.list.selected_entry().unwrap());

        assert_eq!(label, expected_label);
    }
}

#[test]
fn dashboard_title_omits_redundant_key_hint_guidance() {
    let mut panel = WorkspacePanelViewModel::empty(Path::new("test"));
    panel.header.ticket_configured = true;
    panel.header.companion = Some(CompanionPanelState::new(
        "yoi",
        CompanionPanelStatus::Live,
        Some("idle".to_string()),
    ));
    let app = app_with_panel(
        PodList::from_sources(
            PodVisibilitySource::ResumePicker,
            vec![],
            vec![live_info("yoi", PodStatus::Idle)],
            None,
            10,
        ),
        panel,
    );

    let title = plain_line(&title_line(&app));

    assert!(title.contains("workspace dashboard"));
    assert!(title.contains("companion live"));
    assert!(!title.contains("Row selection"));
    assert!(!title.contains("blank Enter"));
    assert!(!title.contains("Tab target"));
}

#[test]
fn panel_ticket_rows_render_state_title_then_detail_line() {
    let row = panel_test_ticket_row(
        "00001KTX1QMG9",
        "Workspace Dashboard composer targets",
        ActionPriority::ActiveWork,
        NextUserAction::Wait,
        "inprogress",
    );

    let lines = panel_row_lines(&row, true, 160);
    let (title, detail) = (&lines[0], &lines[1]);
    let title_line = plain_line(&title);
    let detail_line = plain_line(&detail);
    let state_start = 2;
    let title_start = state_start + TICKET_STATE_COLUMN_WIDTH + 1;
    let row_id = row.ticket.as_ref().unwrap().id.as_str();

    assert!(title_line.starts_with("▶ "));
    assert!(detail_line.starts_with("│ meta "));
    assert!(!title_line.contains(row_id));
    assert_eq!(display_column(&title_line, "inprogress"), state_start);
    assert_eq!(
        display_column(&title_line, "Workspace Dashboard composer targets"),
        title_start
    );
    assert!(detail_line.contains(row_id));
    assert!(detail_line.contains("Gate: clear"));
    assert!(detail_line.contains("Action: Wait"));
}

#[test]
fn panel_ticket_non_selected_rows_align_with_selected_marker_space() {
    let row = panel_test_ticket_row(
        "00001KTTB479X",
        "Long Ticket title that should be rendered after short columns",
        ActionPriority::ReadyForQueue,
        NextUserAction::Queue,
        "ready",
    );

    let lines = panel_row_lines(&row, false, 160);
    let (title, detail) = (&lines[0], &lines[1]);
    let title_line = plain_line(&title);
    let detail_line = plain_line(&detail);
    let state_start = 2;
    let title_start = state_start + TICKET_STATE_COLUMN_WIDTH + 1;

    assert!(title_line.starts_with("  ready"));
    assert!(detail_line.starts_with("  meta 00001KTTB479X"));
    assert_eq!(display_column(&title_line, "ready"), state_start);
    assert_eq!(
        display_column(&title_line, "Long Ticket title"),
        title_start
    );
}

#[test]
fn panel_ticket_title_truncates_after_state_column() {
    let row = panel_test_ticket_row(
        "00001KTTB479X",
        "Very long Ticket title that should truncate only after the state column",
        ActionPriority::ReadyForQueue,
        NextUserAction::Queue,
        "ready",
    );

    let lines = panel_row_lines(&row, false, 42);
    let (title, detail) = (&lines[0], &lines[1]);
    let title_line = plain_line(&title);
    let detail_line = plain_line(&detail);
    let title_start = 2 + TICKET_STATE_COLUMN_WIDTH + 1;

    assert_eq!(title_line.width(), 42);
    assert_eq!(display_column(&title_line, "Very long Ticket"), title_start);
    assert!(title_line.ends_with('…'));
    assert_eq!(detail_line.width(), 42);
    assert!(detail_line.starts_with("  meta 00001KTTB479X · Gate: clear"));
    assert!(detail_line.ends_with('…'));
}

#[test]
fn panel_orchestration_overlay_uses_compact_status_column_and_detail_line() {
    let mut row = panel_test_ticket_row(
        "00001OVERLAY",
        "Overlay column regression",
        ActionPriority::Background,
        NextUserAction::Wait,
        "queued",
    );
    row.kind = PanelRowKind::Review;
    row.status = "q→done".to_string();
    row.disabled_reason = Some(
        "orchestration worktree overlay shows Ticket state done; local state remains queued"
            .to_string(),
    );
    row.ticket.as_mut().unwrap().orchestration_overlay =
        Some(crate::workspace_panel::TicketStateOverlay {
            source: "orchestration".to_string(),
            workflow_state: TicketWorkflowState::Done,
        });

    let lines = panel_row_lines(&row, false, 160);
    let title_line = plain_line(&lines[0]);
    let detail_line = plain_line(&lines[1]);
    let state_start = 2;
    let title_start = state_start + TICKET_STATE_COLUMN_WIDTH + 1;

    assert!(row.status.width() <= TICKET_STATE_COLUMN_WIDTH);
    assert_eq!(display_column(&title_line, "q→done"), state_start);
    assert_eq!(
        display_column(&title_line, "Overlay column regression"),
        title_start
    );
    assert!(!title_line.contains("orchestration"));
    assert!(detail_line.contains("Overlay: local queued · orchestration done · merge pending"));
}

#[test]
fn ready_ticket_with_waiting_gate_shows_queue_disabled_reason() {
    let mut row = panel_test_ticket_row(
        "00001WAITING",
        "Ready but gated",
        ActionPriority::Background,
        NextUserAction::Wait,
        "ready",
    );
    row.disabled_reason = Some("Queue disabled: waiting for BLOCKER-1".to_string());
    row.ticket.as_mut().unwrap().blocked_reason = Some("BLOCKER-1 via depends_on".to_string());

    let lines = panel_row_lines(&row, true, 160);
    let detail = &lines[1];
    let detail_line = plain_line(&detail);

    assert!(detail_line.contains("Gate: waiting for BLOCKER-1 via depends_on"));
    assert!(detail_line.contains("Action: queue disabled"));
    assert!(detail_line.contains("Reason: Queue disabled: waiting for BLOCKER-1"));
}

#[test]
fn panel_ticket_intake_child_rows_render_as_indented_single_line() {
    let row = panel_test_intake_child_row(
        "00001TICKET",
        "intake-live",
        TicketLocalClaimStatus::Live,
        Some(NextUserAction::OpenPod),
    );

    let lines = panel_row_lines(&row, false, 160);
    assert_eq!(lines.len(), 1);
    let line = plain_line(&lines[0]);
    let status_start = 4;
    let title_start = status_start + TICKET_STATE_COLUMN_WIDTH + 1;

    assert!(line.starts_with("  └ live"));
    assert_eq!(display_column(&line, "live"), status_start);
    assert_eq!(
        display_column(&line, "Intake Pod: intake-live"),
        title_start
    );
    assert!(!line.starts_with("  live"));

    let selected_line = plain_line(&panel_row_lines(&row, true, 160)[0]);
    assert!(selected_line.starts_with("  ▶ live"));
    assert!(selected_line.contains("Intake Pod: intake-live"));
}

#[test]
fn selected_ticket_intake_child_keeps_row_marker_without_status_line() {
    let ticket_id = "00001TICKET";
    let pod_name = "intake-live";
    let mut panel = WorkspacePanelViewModel::empty(Path::new("test"));
    panel.rows.push(panel_test_intake_child_row(
        ticket_id,
        pod_name,
        TicketLocalClaimStatus::Live,
        Some(NextUserAction::OpenPod),
    ));
    let list = PodList::from_sources(
        PodVisibilitySource::ResumePicker,
        vec![],
        vec![live_info(pod_name, PodStatus::Idle)],
        None,
        10,
    );
    let mut app = app_with_panel(list, panel);
    app.select_panel_key(PanelRowKey::TicketIntakePod {
        ticket_id: ticket_id.to_string(),
        pod_name: pod_name.to_string(),
    });

    let status = plain_line(&target_status_line(&app));

    assert_eq!(status, "");

    let title_line = plain_line(&panel_row_lines(app.selected_panel_row().unwrap(), true, 80)[0]);
    assert!(title_line.starts_with("  ▶ live"));
    assert!(title_line.contains("Intake"));
}

#[test]
fn panel_pod_rows_use_aligned_columns_before_pod_name() {
    let app = test_app(vec![
        live_info("companion", PodStatus::Idle),
        live_info("very-long-background-worker-name", PodStatus::Running),
    ]);
    let idle = app
        .list
        .entries
        .iter()
        .find(|entry| entry.name == "companion")
        .unwrap();
    let running = app
        .list
        .entries
        .iter()
        .find(|entry| entry.name == "very-long-background-worker-name")
        .unwrap();

    let idle_line = plain_line(&row_line(idle, false, 120));
    let running_line = plain_line(&row_line(running, false, 120));
    let name_start = 2 + POD_STATUS_COLUMN_WIDTH + 1;

    assert!(!running_line.starts_with("  very-long-background-worker-name"));
    assert_eq!(display_column(&idle_line, "live idle"), 2);
    assert_eq!(display_column(&running_line, "live running"), 2);
    assert_eq!(display_column(&idle_line, "companion"), name_start);
    assert_eq!(
        display_column(&running_line, "very-long-background-worker-name"),
        name_start
    );
}

#[test]
fn panel_pod_name_truncates_after_status() {
    let app = test_app(vec![live_info(
        "very-long-background-worker-name-that-keeps-going",
        PodStatus::Running,
    )]);
    let entry = app.list.selected_entry().unwrap();

    let line = plain_line(&row_line(entry, false, 58));
    let name_start = 2 + POD_STATUS_COLUMN_WIDTH + 1;

    assert_eq!(line.width(), 58);
    assert_eq!(display_column(&line, "live running"), 2);
    assert_eq!(display_column(&line, "very-long"), name_start);
    assert!(line.ends_with('…'));
}

#[test]
fn dashboard_running_paused_and_stopped_targets_are_open_eligible() {
    let mut app = test_app(vec![
        live_info("running", PodStatus::Running),
        live_info("paused", PodStatus::Paused),
    ]);
    let stopped = stopped_info("stopped");
    app.list = PodList::from_sources(
        PodVisibilitySource::ResumePicker,
        vec![stopped],
        vec![
            live_info_with_updated_at("running", PodStatus::Running, 30),
            live_info_with_updated_at("paused", PodStatus::Paused, 20),
        ],
        Some("running".to_string()),
        10,
    );
    app.selected_row = None;
    app.ensure_selection_visible();

    assert_eq!(app.selected_open_eligibility(), OpenEligibility::Disabled);
    app.select_next();
    assert_eq!(app.list.selected_entry().unwrap().name, "running");
    assert_eq!(app.selected_open_eligibility(), OpenEligibility::OpenNow);
    app.select_next();
    assert_eq!(app.list.selected_entry().unwrap().name, "paused");
    assert_eq!(app.selected_open_eligibility(), OpenEligibility::OpenNow);
    app.select_next();
    assert_eq!(app.list.selected_entry().unwrap().name, "stopped");
    assert_eq!(app.selected_open_eligibility(), OpenEligibility::OpenNow);
}

#[test]
fn dashboard_sections_classify_pending_working_and_closed() {
    let list = PodList::from_sources(
        PodVisibilitySource::ResumePicker,
        vec![stopped_info_with_updated_at("closed", 60)],
        vec![
            live_info_with_updated_at("idle", PodStatus::Idle, 50),
            live_info_with_updated_at("running", PodStatus::Running, 40),
            live_info_with_updated_at("paused", PodStatus::Paused, 30),
        ],
        Some("idle".to_string()),
        10,
    );

    let sections = sectioned_entries(&list);

    assert_eq!(section_names(&list, &sections[0]), vec!["idle"]);
    assert_eq!(
        section_names(&list, &sections[1]),
        vec!["running", "paused"]
    );
    assert_eq!(section_names(&list, &sections[2]), vec!["closed"]);
}

#[test]
fn dashboard_closed_section_is_limited_to_three_visible_rows() {
    let list = closed_list(5, Some("closed-0"));
    let visible = visible_entry_indices(&list)
        .into_iter()
        .map(|index| list.entries[index].name.clone())
        .collect::<Vec<_>>();
    let sections = sectioned_entries(&list);
    let closed = sections
        .iter()
        .find(|section| section.kind == DashboardSectionKind::Closed)
        .unwrap();
    let app = app_with_list(list);
    let lines = list_lines(&app, 80, 8)
        .into_iter()
        .map(|line| plain_line(&line))
        .collect::<Vec<_>>();

    assert_eq!(visible, vec!["closed-0", "closed-1", "closed-2"]);
    assert_eq!(closed.hidden_count(), 2);
    assert!(
        lines
            .iter()
            .any(|line| line.contains("closed 5 total, +2 hidden"))
    );
    assert!(lines.iter().any(|line| line.contains("closed-2")));
    assert!(!lines.iter().any(|line| line.contains("closed-3")));
}

#[test]
fn dashboard_selection_follows_visible_section_order_without_hidden_closed_rows() {
    let list = PodList::from_sources(
        PodVisibilitySource::ResumePicker,
        (0..5)
            .map(|index| stopped_info_with_updated_at(&format!("closed-{index}"), 50 - index))
            .collect(),
        vec![
            live_info_with_updated_at("running", PodStatus::Running, 70),
            live_info_with_updated_at("idle", PodStatus::Idle, 60),
        ],
        Some("idle".to_string()),
        20,
    );
    let mut app = app_with_list(list);

    assert!(app.selected_row.is_none());
    app.select_next();
    assert_eq!(app.list.selected_entry().unwrap().name, "idle");
    app.select_next();
    assert_eq!(app.list.selected_entry().unwrap().name, "running");
    app.select_next();
    assert_eq!(app.list.selected_entry().unwrap().name, "closed-0");
    app.select_next();
    assert_eq!(app.list.selected_entry().unwrap().name, "closed-1");
    app.select_next();
    assert_eq!(app.list.selected_entry().unwrap().name, "closed-2");
    app.select_next();
    assert_eq!(app.list.selected_entry().unwrap().name, "closed-2");
}

#[test]
fn dashboard_selection_does_not_default_to_orchestrator_only_row() {
    let list = PodList::from_sources(
        PodVisibilitySource::ResumePicker,
        vec![],
        vec![live_info("test-orchestrator", PodStatus::Idle)],
        None,
        10,
    );
    let mut panel = WorkspacePanelViewModel::empty(Path::new("test"));
    panel.header.orchestrator = Some(OrchestratorPanelState::new(
        "test-orchestrator",
        OrchestratorPanelStatus::Live,
        None,
    ));
    let app = app_with_panel(list, panel);

    assert!(app.selected_row.is_none());
    assert!(app.list.selected_name.is_none());
    assert_eq!(app.selected_open_eligibility(), OpenEligibility::Disabled);
}

#[test]
fn dashboard_selection_has_no_default_when_orchestrator_pod_exists() {
    let list = PodList::from_sources(
        PodVisibilitySource::ResumePicker,
        vec![],
        vec![
            live_info_with_updated_at("test-orchestrator", PodStatus::Idle, 80),
            live_info_with_updated_at("worker", PodStatus::Idle, 70),
        ],
        None,
        10,
    );
    let mut panel = WorkspacePanelViewModel::empty(Path::new("test"));
    panel.header.orchestrator = Some(OrchestratorPanelState::new(
        "test-orchestrator",
        OrchestratorPanelStatus::Live,
        None,
    ));
    let app = app_with_panel(list, panel);

    assert!(app.selected_row.is_none());
    assert!(app.list.selected_name.is_none());
}

#[test]
fn dashboard_list_renders_workspace_diagnostics_before_rows() {
    let mut panel = WorkspacePanelViewModel::empty(Path::new("test"));
    panel
        .header
        .diagnostics
        .push("Ticket config is unusable".to_string());
    let app = app_with_panel(
        PodList::from_sources(
            PodVisibilitySource::ResumePicker,
            vec![],
            vec![live_info("idle", PodStatus::Idle)],
            None,
            10,
        ),
        panel,
    );
    let lines = list_lines(&app, 80, 4)
        .into_iter()
        .map(|line| plain_line(&line))
        .collect::<Vec<_>>();

    assert!(lines[0].contains("Ticket config is unusable"));
    assert!(lines.iter().any(|line| line.contains("idle")));
}

#[test]
fn dashboard_list_pins_closed_section_below_live_flexible_area() {
    let list = PodList::from_sources(
        PodVisibilitySource::ResumePicker,
        (0..3)
            .map(|index| stopped_info_with_updated_at(&format!("closed-{index}"), 50 - index))
            .collect(),
        vec![
            live_info_with_updated_at("running", PodStatus::Running, 70),
            live_info_with_updated_at("idle", PodStatus::Idle, 60),
        ],
        Some("idle".to_string()),
        20,
    );
    let app = app_with_list(list);
    let lines = list_lines(&app, 80, 12)
        .into_iter()
        .map(|line| plain_line(&line))
        .collect::<Vec<_>>();

    assert!(lines[0].contains("pending"));
    assert!(lines[2].contains("working"));
    assert!(lines[4].is_empty());
    assert!(lines[8].contains("closed"));
    assert!(lines[11].contains("closed-2"));
}

#[test]
fn dashboard_layout_uses_single_boundary_separator_between_list_and_composer() {
    let layout = dashboard_layout(Rect::new(0, 0, 80, 24), 1);

    assert_eq!(layout.boundary.height, 1);
    assert!(!layout.list_draws_own_separator);
    assert_eq!(layout.boundary.y, layout.list.y + layout.list.height);
    assert_eq!(
        layout.target_status.y,
        layout.boundary.y + layout.boundary.height
    );
}

#[test]
fn dashboard_companion_submit_routes_to_workspace_companion_not_selected_pod() {
    let mut app = companion_app(
        vec![
            live_info("alpha", PodStatus::Idle),
            live_info("yoi", PodStatus::Idle),
        ],
        CompanionPanelStatus::Live,
    );
    let alpha_index = app
        .list
        .entries
        .iter()
        .position(|entry| entry.name == "alpha")
        .unwrap();
    app.list.select_index(alpha_index);
    app.input.insert_str("send to companion");

    let request = match app.handle_key(key(KeyCode::Enter)) {
        DashboardAction::SendCompanion(request) => request,
        _ => panic!("Companion target should send to the workspace Companion"),
    };

    assert_eq!(request.pod_name, "yoi");
    assert_eq!(request.socket_path, PathBuf::from("/tmp/yoi.sock"));
    assert!(app.sending);
    assert_eq!(input_text(&app), "send to companion");
    assert!(app.notice.as_deref().unwrap().contains("Companion yoi"));
}

#[test]
fn dashboard_companion_submit_unavailable_keeps_composer_contents() {
    let mut app = companion_app(vec![], CompanionPanelStatus::Missing);
    app.input.insert_str("keep me");
    let before = input_text(&app);

    assert!(matches!(
        app.handle_key(key(KeyCode::Enter)),
        DashboardAction::None
    ));

    assert_eq!(input_text(&app), before);
    assert!(!app.sending);
    assert!(app.notice.as_deref().unwrap().contains("draft kept"));
}

#[test]
fn dashboard_companion_submit_empty_reports_empty_composer() {
    let mut app = companion_app(
        vec![live_info("yoi", PodStatus::Idle)],
        CompanionPanelStatus::Live,
    );

    assert!(app.prepare_companion_send().is_none());

    assert_eq!(input_text(&app), "");
    assert!(!app.sending);
    assert_eq!(app.notice.as_deref(), Some("Composer is empty."));
}

#[test]
fn dashboard_companion_finish_success_clears_composer() {
    let mut app = companion_app(
        vec![live_info("yoi", PodStatus::Idle)],
        CompanionPanelStatus::Live,
    );
    app.input.insert_str("done");
    app.sending = true;

    app.finish_companion_send(Ok(CompanionSendOutcome {
        notice: "Sent to Companion yoi.".to_string(),
    }));

    assert_eq!(input_text(&app), "");
    assert!(!app.sending);
    assert_eq!(app.notice.as_deref(), Some("Sent to Companion yoi."));
}

#[test]
fn dashboard_open_request_keeps_dashboard_state_for_nested_console() {
    let mut app = test_app(vec![live_info("alpha", PodStatus::Idle)]);
    app.input.insert_str("draft survives open");
    app.select_next();

    let request = app.prepare_open().unwrap();

    assert_eq!(request.pod_name, "alpha");
    assert_eq!(
        request.socket_override,
        Some(PathBuf::from("/tmp/alpha.sock"))
    );
    assert_eq!(app.list.selected_entry().unwrap().name, "alpha");
    assert_eq!(input_text(&app), "draft survives open");
    assert!(
        app.notice
            .as_deref()
            .unwrap()
            .contains("Attaching to alpha")
    );
}

#[test]
fn dashboard_open_failure_keeps_composer_and_sets_notice() {
    let mut app = test_app(vec![live_info("alpha", PodStatus::Idle)]);
    app.input.insert_str("keep this draft");
    let before = input_text(&app);
    let error = io::Error::other("boom");

    app.finish_open("alpha", Err(&error));

    assert_eq!(input_text(&app), before);
    assert_eq!(app.list.selected_entry().unwrap().name, "alpha");
    assert!(
        app.notice
            .as_deref()
            .unwrap()
            .contains("Open failed for alpha")
    );
    assert!(app.refreshing);
    assert!(matches!(
        app.enter_reload,
        Some(OrchestratorLifecycleMode::Observe)
    ));
}

#[test]
fn dashboard_loading_app_defers_initial_snapshot_to_enter_reload() {
    let app = DashboardApp::loading(PodRuntimeCommand::for_executable("/tmp/yoi"));

    assert!(app.panel.rows.is_empty());
    assert!(
        app.panel
            .header
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.contains("Loading workspace dashboard"))
    );
    assert!(app.refreshing);
    assert!(matches!(
        app.enter_reload,
        Some(OrchestratorLifecycleMode::Ensure { .. })
    ));
}

#[test]
fn dashboard_nested_console_success_continues_without_dropping_state() {
    let mut app = test_app(vec![
        live_info("alpha", PodStatus::Idle),
        live_info("beta", PodStatus::Idle),
    ]);
    app.select_next();
    app.select_next();
    app.input.insert_str("keep this draft");
    app.panel_diagnostic = Some(PanelDiagnostic {
        title: "diagnostic stays".to_string(),
        details: "details stay".to_string(),
    });
    app.panel_diagnostic_open = true;

    finish_nested_console_open(&mut app, "beta", Ok(())).unwrap();

    assert_eq!(input_text(&app), "keep this draft");
    assert_eq!(app.list.selected_entry().unwrap().name, "beta");
    assert_eq!(
        app.panel_diagnostic
            .as_ref()
            .map(|diagnostic| (diagnostic.title.as_str(), diagnostic.details.as_str(),)),
        Some(("diagnostic stays", "details stay"))
    );
    assert!(app.panel_diagnostic_open);
    assert!(
        app.notice
            .as_deref()
            .unwrap()
            .contains("Returned from beta. Refreshing workspace")
    );
    assert!(app.refreshing);
    assert!(matches!(
        app.enter_reload,
        Some(OrchestratorLifecycleMode::Observe)
    ));
}

#[test]
fn dashboard_nested_console_recoverable_failure_continues_without_dropping_state() {
    let mut app = test_app(vec![
        live_info("alpha", PodStatus::Idle),
        live_info("beta", PodStatus::Idle),
    ]);
    app.select_next();
    app.select_next();
    app.input.insert_str("keep this draft");
    app.panel_diagnostic = Some(PanelDiagnostic {
        title: "diagnostic stays".to_string(),
        details: "details stay".to_string(),
    });
    app.panel_diagnostic_open = true;
    let error = crate::spawn::SpawnError::Io(io::Error::other("spawn failed"));

    finish_nested_console_open(&mut app, "beta", Err(Box::new(error))).unwrap();

    assert_eq!(input_text(&app), "keep this draft");
    assert_eq!(app.list.selected_entry().unwrap().name, "beta");
    assert_eq!(
        app.panel_diagnostic
            .as_ref()
            .map(|diagnostic| (diagnostic.title.as_str(), diagnostic.details.as_str(),)),
        Some(("diagnostic stays", "details stay"))
    );
    assert!(app.panel_diagnostic_open);
    assert!(
        app.notice
            .as_deref()
            .unwrap()
            .contains("Open failed for beta: io error: spawn failed. Refreshing workspace")
    );
    assert!(app.refreshing);
    assert!(matches!(
        app.enter_reload,
        Some(OrchestratorLifecycleMode::Observe)
    ));
}

#[test]
fn dashboard_nested_console_nonrecoverable_failure_bubbles_without_state_finish() {
    let mut app = test_app(vec![live_info("alpha", PodStatus::Idle)]);
    app.input.insert_str("keep this draft");
    app.notice = Some("opening alpha".to_string());
    let error = io::Error::other("fatal console error");

    let err = finish_nested_console_open(&mut app, "alpha", Err(Box::new(error))).unwrap_err();

    assert_eq!(err.to_string(), "fatal console error");
    assert_eq!(input_text(&app), "keep this draft");
    assert_eq!(app.notice.as_deref(), Some("opening alpha"));
    assert!(!app.refreshing);
    assert!(app.enter_reload.is_none());
}

#[test]
fn dashboard_open_disabled_target_stays_in_dashboard() {
    let mut live = live_info("unreachable", PodStatus::Idle);
    live.reachable = false;
    live.status = None;
    let mut app = test_app(vec![live]);
    app.select_next();

    assert!(app.prepare_open().is_none());
    assert!(app.notice.as_deref().unwrap().contains("cannot be opened"));
}

#[test]
fn dashboard_empty_enter_uses_open_action() {
    let mut app = test_app(vec![live_info("alpha", PodStatus::Idle)]);
    app.select_next();

    assert!(matches!(
        app.handle_key(key(KeyCode::Enter)),
        DashboardAction::Open
    ));
    let request = app.prepare_open().unwrap();

    assert_eq!(request.pod_name, "alpha");
    assert_eq!(
        request.socket_override,
        Some(PathBuf::from("/tmp/alpha.sock"))
    );
    assert_eq!(input_text(&app), "");
    assert!(
        app.notice
            .as_deref()
            .unwrap()
            .contains("Attaching to alpha")
    );
}

#[test]
fn dashboard_whitespace_only_enter_uses_open_action() {
    let mut app = test_app(vec![live_info("alpha", PodStatus::Idle)]);
    app.select_next();
    app.input.insert_str("  \n\t");

    assert!(matches!(
        app.handle_key(key(KeyCode::Enter)),
        DashboardAction::Open
    ));
    let request = app.prepare_open().unwrap();

    assert_eq!(request.pod_name, "alpha");
    assert_eq!(input_text(&app), "  \n\t");
}

#[test]
fn dashboard_non_empty_enter_reports_companion_unavailable() {
    let mut app = test_app(vec![live_info("idle", PodStatus::Idle)]);
    app.input.insert_str("keep this draft");

    assert!(matches!(
        app.handle_key(key(KeyCode::Enter)),
        DashboardAction::None
    ));

    assert_eq!(input_text(&app), "keep this draft");
    assert!(!app.sending);
    assert!(
        app.notice
            .as_deref()
            .unwrap()
            .contains("Workspace Companion is unavailable")
    );
}

#[test]
fn dashboard_alt_enter_inserts_newline_without_companion_send() {
    let mut app = ticket_enabled_app(vec![live_info("idle", PodStatus::Idle)]);
    app.input.insert_str("first line");

    assert!(matches!(
        app.handle_key(modified_key(KeyCode::Enter, KeyModifiers::ALT)),
        DashboardAction::None
    ));

    assert_eq!(input_text(&app), "first line\n");
    assert!(!app.sending);
    assert!(app.notice.is_none());
}

#[test]
fn dashboard_alt_enter_on_blank_pod_selection_inserts_newline_without_opening() {
    let mut app = test_app(vec![live_info("alpha", PodStatus::Idle)]);
    let selected_before = app.selected_row.clone();

    assert!(matches!(
        app.handle_key(modified_key(KeyCode::Enter, KeyModifiers::ALT)),
        DashboardAction::None
    ));

    assert_eq!(input_text(&app), "\n");
    assert_eq!(app.selected_row, selected_before);
    assert!(app.notice.is_none());
}

#[test]
fn dashboard_alt_enter_on_blank_ticket_action_inserts_newline_without_dispatch() {
    let mut panel = WorkspacePanelViewModel::empty(Path::new("test"));
    panel.rows.push(panel_test_ticket_row(
        "TICKET-1",
        "Ready",
        ActionPriority::ReadyForQueue,
        NextUserAction::Queue,
        "ready",
    ));
    let mut app = app_with_panel(
        PodList::from_sources(PodVisibilitySource::ResumePicker, vec![], vec![], None, 10),
        panel,
    );
    app.select_next();
    let selected_before = app.selected_row.clone();
    assert_eq!(app.selected_ticket_action(), Some(NextUserAction::Queue));

    assert!(matches!(
        app.handle_key(modified_key(KeyCode::Enter, KeyModifiers::ALT)),
        DashboardAction::None
    ));

    assert_eq!(input_text(&app), "\n");
    assert_eq!(app.selected_row, selected_before);
    assert!(!app.sending);
    assert!(app.notice.is_none());
}

#[test]
fn dashboard_composer_shared_word_motion_and_delete_keys() {
    let mut app = ticket_enabled_app(vec![live_info("idle", PodStatus::Idle)]);
    app.input.insert_str("hello world");

    assert!(matches!(
        app.handle_key(modified_key(KeyCode::Left, KeyModifiers::CONTROL)),
        DashboardAction::None
    ));
    assert!(matches!(
        app.handle_key(key(KeyCode::Char('!'))),
        DashboardAction::None
    ));
    assert_eq!(input_text(&app), "hello !world");

    assert!(matches!(
        app.handle_key(modified_key(KeyCode::Right, KeyModifiers::CONTROL)),
        DashboardAction::None
    ));
    assert!(matches!(
        app.handle_key(modified_key(KeyCode::Char('w'), KeyModifiers::CONTROL)),
        DashboardAction::None
    ));
    assert_eq!(input_text(&app), "hello !");
}

#[test]
fn dashboard_esc_clears_row_selection_without_quitting_and_preserves_draft() {
    let mut app = ticket_enabled_app(vec![live_info("alpha", PodStatus::Idle)]);
    app.select_next();
    app.input.insert_str("draft message");

    assert!(app.selected_row.is_some());
    assert!(matches!(
        app.handle_key(key(KeyCode::Esc)),
        DashboardAction::None
    ));
    assert!(app.selected_row.is_none());
    assert_eq!(input_text(&app), "draft message");
    assert!(
        app.notice
            .as_deref()
            .unwrap()
            .contains("Row selection cleared")
    );
    assert!(matches!(
        app.handle_key(modified_key(KeyCode::Char('c'), KeyModifiers::CONTROL)),
        DashboardAction::Quit
    ));
}

#[test]
fn dashboard_composer_target_switch_preserves_typed_text() {
    let mut app = ticket_enabled_app(vec![live_info("idle", PodStatus::Idle)]);
    app.input.insert_str("draft intake request");

    assert!(matches!(app.composer_target(), ComposerTarget::Companion));
    let selected_before = app.selected_row.clone();
    assert!(matches!(
        app.handle_key(key(KeyCode::Tab)),
        DashboardAction::None
    ));

    assert!(matches!(
        app.composer_target(),
        ComposerTarget::TicketIntake
    ));
    assert_eq!(app.selected_row, selected_before);
    assert_eq!(input_text(&app), "draft intake request");
}

#[test]
fn dashboard_ctrl_t_does_not_switch_composer_target() {
    let mut app = ticket_enabled_app(vec![live_info("idle", PodStatus::Idle)]);
    app.input.insert_str("draft intake request");

    assert!(matches!(app.composer_target(), ComposerTarget::Companion));
    assert!(matches!(
        app.handle_key(modified_key(KeyCode::Char('t'), KeyModifiers::CONTROL)),
        DashboardAction::None
    ));

    assert!(matches!(app.composer_target(), ComposerTarget::Companion));
    assert_eq!(input_text(&app), "draft intake request");
}

#[test]
fn dashboard_no_ticket_workspace_exposes_only_companion_target() {
    let mut app = test_app(vec![live_info("idle", PodStatus::Idle)]);
    app.input.insert_str("draft message");

    app.cycle_composer_target();

    assert_eq!(
        app.panel.composer.available_targets,
        vec![ComposerTarget::Companion]
    );
    assert!(matches!(app.composer_target(), ComposerTarget::Companion));
    assert_eq!(input_text(&app), "draft message");
    assert!(app.notice.as_deref().unwrap().contains("unavailable"));
}

#[test]
fn dashboard_blank_ticket_intake_enter_uses_selected_row_and_preserves_input() {
    let mut app = ticket_enabled_app(vec![live_info("idle", PodStatus::Idle)]);
    app.cycle_composer_target();
    app.input.insert_str("  \n\t");

    assert!(matches!(
        app.handle_key(key(KeyCode::Enter)),
        DashboardAction::Open
    ));

    assert!(matches!(
        app.composer_target(),
        ComposerTarget::TicketIntake
    ));
    assert!(!app.sending);
    assert_eq!(input_text(&app), "  \n\t");
    assert!(
        !app.notice
            .as_deref()
            .unwrap_or_default()
            .contains("input is empty")
    );
}

#[test]
fn dashboard_ticket_intake_enter_builds_launch_request_not_direct_send() {
    let mut app = ticket_enabled_app(vec![live_info("idle", PodStatus::Idle)]);
    app.cycle_composer_target();
    app.input.insert_str("please intake this work");

    let request = match app.handle_key(key(KeyCode::Enter)) {
        DashboardAction::LaunchIntake(request) => request,
        _ => panic!("Ticket Intake target should launch Intake"),
    };

    assert_eq!(request.context.role, TicketRole::Intake);
    assert_eq!(
        request.context.user_instruction.as_deref(),
        Some("please intake this work")
    );
    assert_eq!(request.runtime_command.program(), Path::new("/tmp/yoi"));
    assert_eq!(
        request.context.intake_handoff,
        Some(TicketIntakeHandoff::new("test-orchestrator", "test"))
    );
    assert_eq!(
        request.peer_registration,
        IntakePeerRegistrationRequest::Register {
            orchestrator_pod: "test-orchestrator".to_string()
        }
    );
    assert!(app.sending);
    assert!(app.notice.as_deref().unwrap().contains("Launching"));
    assert_eq!(input_text(&app), "please intake this work");
}

#[test]
fn dashboard_ticket_intake_handoff_skips_peer_registration_when_orchestrator_not_live() {
    let mut app = ticket_enabled_app_with_orchestrator(
        vec![live_info("idle", PodStatus::Idle)],
        OrchestratorPanelStatus::Unavailable,
    );
    app.cycle_composer_target();
    app.input.insert_str("please intake this work");

    let request = match app.handle_key(key(KeyCode::Enter)) {
        DashboardAction::LaunchIntake(request) => request,
        _ => panic!("Ticket Intake target should launch Intake"),
    };

    assert_eq!(
        request.context.intake_handoff,
        Some(TicketIntakeHandoff::new("test-orchestrator", "test"))
    );
    match request.peer_registration {
        IntakePeerRegistrationRequest::Skip { reason } => {
            assert!(reason.contains("test-orchestrator"));
            assert!(reason.contains("unavailable"));
        }
        other => panic!("expected peer registration skip, got {other:?}"),
    }
}

#[test]
fn dashboard_ticket_intake_finish_success_clears_composer_and_reports_pod() {
    let mut app = ticket_enabled_app(vec![live_info("idle", PodStatus::Idle)]);
    app.cycle_composer_target();
    app.input.insert_str("please intake this work");
    app.sending = true;

    app.finish_intake_launch(Ok(IntakeLaunchOutcome {
        launch: TicketRoleLaunchResult {
            plan: client::ticket_role::TicketRoleLaunchPlan {
                workspace_root: PathBuf::from("/tmp/workspace"),
                cwd: None,
                original_workspace_root: PathBuf::from("/tmp/workspace"),
                target_workspace_root: PathBuf::from("/tmp/workspace"),
                implementation_worktree_root: PathBuf::from("/tmp/workspace/.worktree"),
                role: TicketRole::Intake,
                pod_name: "intake-pod".to_string(),
                profile: "builtin:default".to_string(),
                workflow: "ticket-intake-workflow".to_string(),
                launch_prompt_ref: None,
                run_segments: vec![],
            },
            ready: client::SpawnReady {
                pod_name: "intake-pod".to_string(),
                socket_path: PathBuf::from("/tmp/intake.sock"),
            },
            pre_run_warnings: vec![],
        },
        peer_registration: IntakePeerRegistrationStatus::Registered {
            orchestrator_pod: "test-orchestrator".to_string(),
        },
        registry_warning: None,
    }));

    assert!(!app.sending);
    assert_eq!(input_text(&app), "");
    let notice = app.notice.as_deref().unwrap();
    assert!(notice.contains("intake-pod"));
    assert!(notice.contains("Handoff peer registered"));
}

#[test]
fn dashboard_ticket_intake_finish_failure_keeps_composer() {
    let mut app = ticket_enabled_app(vec![live_info("idle", PodStatus::Idle)]);
    app.cycle_composer_target();
    app.input.insert_str("please keep this");
    app.sending = true;

    app.finish_intake_launch(Err(TicketRoleLaunchError::EmptyPodName));

    assert!(!app.sending);
    assert_eq!(input_text(&app), "please keep this");
    assert!(app.notice.as_deref().unwrap().contains("composer kept"));
}

#[test]
fn intake_registry_update_claim_is_durable_only_after_commit() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("registry");
    let store = PanelRegistryStore::from_root(root.clone());
    let update = IntakeRegistryUpdate::ClaimTicket {
        registry_root: root,
        ticket_id: "20260608-000000-existing".to_string(),
        ticket_slug: Some("existing".to_string()),
        pod_name: "existing-intake".to_string(),
    };

    assert!(
        store
            .claim_for_ticket("20260608-000000-existing")
            .unwrap()
            .is_none(),
        "holding a pending Intake registry update must not persist a Ticket claim"
    );

    assert!(commit_intake_registry_update(update.clone(), None).is_none());
    assert!(
        store
            .claim_for_ticket("20260608-000000-existing")
            .unwrap()
            .is_some(),
        "the claim is persisted only by the post-acceptance commit step"
    );

    assert!(commit_intake_registry_update(update, None).is_none());
    let snapshot = store.snapshot().unwrap();
    assert_eq!(snapshot.claims.len(), 1);
    assert_eq!(snapshot.sessions.len(), 1);
    assert_eq!(snapshot.sessions[0].pod_name, "existing-intake");
    assert_eq!(snapshot.sessions[0].origin, RoleSessionOrigin::TicketClaim);
    assert_eq!(snapshot.sessions[0].related_tickets.len(), 1);
    assert_eq!(
        snapshot.sessions[0].related_tickets[0].id,
        "20260608-000000-existing"
    );
}

#[test]
fn intake_registry_claims_launched_ticket_with_accepted_pod_name() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("registry");
    let store = PanelRegistryStore::from_root(root.clone());
    let update = IntakeRegistryUpdate::ClaimLaunchedTicket {
        registry_root: root,
        ticket_id: "20260608-000000-ready".to_string(),
        ticket_slug: None,
    };

    assert!(commit_intake_registry_update(update, Some("launched-intake")).is_none());

    let claim = store
        .claim_for_ticket("20260608-000000-ready")
        .unwrap()
        .expect("launched Intake Pod is claimed after accepted launch");
    assert_eq!(claim.pod_name, "launched-intake");
    let snapshot = store.snapshot().unwrap();
    assert_eq!(snapshot.claims.len(), 1);
    assert_eq!(snapshot.sessions.len(), 1);
    assert_eq!(snapshot.sessions[0].origin, RoleSessionOrigin::TicketClaim);
    assert_eq!(snapshot.sessions[0].pod_name, "launched-intake");
}

#[test]
fn intake_registry_launched_ticket_claim_without_pod_name_is_diagnostic() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("registry");
    let store = PanelRegistryStore::from_root(root.clone());

    let warning = commit_intake_registry_update(
        IntakeRegistryUpdate::ClaimLaunchedTicket {
            registry_root: root,
            ticket_id: "20260608-000000-ready".to_string(),
            ticket_slug: None,
        },
        None,
    )
    .expect("missing launched Pod name should be diagnostic");

    assert!(warning.contains("missing launched Pod name"));
    assert!(
        store
            .claim_for_ticket("20260608-000000-ready")
            .unwrap()
            .is_none()
    );
}

#[test]
fn intake_registry_update_claim_conflict_is_diagnostic_not_overwrite() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("registry");
    let store = PanelRegistryStore::from_root(root.clone());
    store
        .claim_ticket(
            "20260608-000001-existing",
            Some("existing"),
            "first-intake",
            TicketRole::Intake.as_str(),
        )
        .unwrap();

    let warning = commit_intake_registry_update(
        IntakeRegistryUpdate::ClaimTicket {
            registry_root: root,
            ticket_id: "20260608-000001-existing".to_string(),
            ticket_slug: Some("existing".to_string()),
            pod_name: "second-intake".to_string(),
        },
        None,
    )
    .expect("conflicting post-success claim should be reported");

    assert!(warning.contains("could not be committed"));
    let claim = store
        .claim_for_ticket("20260608-000001-existing")
        .unwrap()
        .unwrap();
    assert_eq!(claim.pod_name, "first-intake");
    let snapshot = store.snapshot().unwrap();
    assert_eq!(snapshot.claims.len(), 1);
    assert_eq!(snapshot.sessions.len(), 1);
    assert_eq!(snapshot.sessions[0].pod_name, "first-intake");
}

#[test]
fn dashboard_empty_enter_on_non_openable_row_reports_open_diagnostic() {
    let mut app = test_app(vec![unreachable_live_info("unreachable")]);
    app.select_next();
    assert!(matches!(
        app.handle_key(key(KeyCode::Enter)),
        DashboardAction::Open
    ));
    assert!(app.prepare_open().is_none());

    assert!(app.notice.as_deref().unwrap().contains("cannot be opened"));
}

#[test]
fn idle_orchestrator_gets_bounded_attention_for_new_queued_work() {
    let mut app = ticket_enabled_app(vec![live_info("test-orchestrator", PodStatus::Idle)]);
    app.panel.rows = vec![panel_test_ticket_row(
        "00001QUEUE",
        "Queued work",
        ActionPriority::Background,
        NextUserAction::Wait,
        "queued",
    )];
    app.refresh_orchestrator_work_set();

    let request = app
        .prepare_orchestrator_queue_attention_notice()
        .expect("idle orchestrator should receive queued-work attention");

    assert_eq!(request.pod_name, "test-orchestrator");
    assert!(request.notice.message.contains("00001QUEUE"));
    assert!(request.notice.message.contains("new_queued"));
    assert!(request.notice.message.contains("queued -> inprogress"));
}

#[test]
fn active_inprogress_suppresses_queued_attention_and_retains_waiting_reason() {
    let mut app = ticket_enabled_app(vec![live_info("test-orchestrator", PodStatus::Idle)]);
    app.panel.rows = vec![
        panel_test_ticket_row(
            "00001ACTIVE",
            "Active work",
            ActionPriority::Background,
            NextUserAction::Wait,
            "inprogress",
        ),
        panel_test_ticket_row(
            "00001QUEUE",
            "Queued work",
            ActionPriority::Background,
            NextUserAction::Wait,
            "queued",
        ),
    ];
    app.refresh_orchestrator_work_set();
    app.apply_orchestrator_work_set_detail();

    assert!(app.prepare_orchestrator_queue_attention_notice().is_none());
    let queued = app
        .orchestrator_work_set
        .queued
        .iter()
        .find(|item| item.id == "00001QUEUE")
        .expect("queued item retained");
    assert_eq!(
        queued.classification,
        OrchestratorQueuedClassification::PlannedQueued
    );
    assert!(
        queued
            .waiting_reason
            .as_deref()
            .unwrap()
            .contains("active_inprogress")
    );
    assert!(
        app.panel
            .header
            .orchestrator
            .as_ref()
            .unwrap()
            .detail
            .as_deref()
            .unwrap()
            .contains("suppressed")
    );
}

#[test]
fn planned_queued_prompts_when_active_work_clears() {
    let mut app = ticket_enabled_app(vec![live_info("test-orchestrator", PodStatus::Idle)]);
    app.panel.rows = vec![
        panel_test_ticket_row(
            "00001ACTIVE",
            "Active work",
            ActionPriority::Background,
            NextUserAction::Wait,
            "inprogress",
        ),
        panel_test_ticket_row(
            "00001QUEUE",
            "Queued work",
            ActionPriority::Background,
            NextUserAction::Wait,
            "queued",
        ),
    ];
    app.refresh_orchestrator_work_set();
    assert!(app.prepare_orchestrator_queue_attention_notice().is_none());

    app.panel.rows = vec![panel_test_ticket_row(
        "00001QUEUE",
        "Queued work",
        ActionPriority::Background,
        NextUserAction::Wait,
        "queued",
    )];
    app.refresh_orchestrator_work_set();
    let request = app
        .prepare_orchestrator_queue_attention_notice()
        .expect("planned queued work should prompt after active work clears");

    assert!(request.notice.message.contains("planned_queued"));
    assert!(
        !request
            .notice
            .message
            .contains("waiting for active_inprogress")
    );
}

#[test]
fn queued_attention_is_suppressed_when_existing_claim_prevents_duplicate_start() {
    let mut app = ticket_enabled_app(vec![live_info("test-orchestrator", PodStatus::Idle)]);
    let mut row = panel_test_ticket_row(
        "00001QUEUE",
        "Queued work",
        ActionPriority::Background,
        NextUserAction::Wait,
        "queued",
    );
    row.ticket.as_mut().unwrap().local_claim =
        Some(crate::workspace_panel::TicketLocalClaimEntry {
            pod_name: "coder-00001QUEUE".to_string(),
            role: "coder".to_string(),
            status: TicketLocalClaimStatus::Live,
        });
    row.related_pods.push("reviewer-00001QUEUE".to_string());
    app.panel.rows = vec![row];
    app.refresh_orchestrator_work_set();
    app.apply_orchestrator_work_set_detail();

    assert!(app.prepare_orchestrator_queue_attention_notice().is_none());
    let waiting = app.orchestrator_work_set.queued[0]
        .waiting_reason
        .as_deref()
        .unwrap();
    assert!(waiting.contains("duplicate start"));
    assert!(waiting.contains("coder-00001QUEUE"));
}

#[test]
fn rediscovered_queued_work_is_actionable_when_session_work_set_is_empty() {
    let mut app = ticket_enabled_app(vec![live_info("test-orchestrator", PodStatus::Idle)]);
    app.orchestrator_work_set = OrchestratorWorkSet::default();
    app.panel.rows = vec![panel_test_ticket_row(
        "00001QUEUE",
        "Queued work",
        ActionPriority::Background,
        NextUserAction::Wait,
        "queued",
    )];

    let request = app
        .prepare_orchestrator_queue_attention_notice()
        .expect("queued ticket state should be rediscovered safely");

    assert!(request.notice.message.contains("new_queued"));
    assert!(request.notice.message.contains("00001QUEUE"));
}

#[test]
fn queued_attention_requires_idle_orchestrator_to_avoid_duplicate_rekick() {
    let mut app = ticket_enabled_app(vec![live_info("test-orchestrator", PodStatus::Running)]);
    app.panel.rows = vec![panel_test_ticket_row(
        "00001QUEUE",
        "Queued work",
        ActionPriority::Background,
        NextUserAction::Wait,
        "queued",
    )];
    app.refresh_orchestrator_work_set();

    assert!(app.prepare_orchestrator_queue_attention_notice().is_none());
}

fn test_app(live: Vec<LivePodInfo>) -> DashboardApp {
    app_with_list(PodList::from_sources(
        PodVisibilitySource::ResumePicker,
        vec![],
        live,
        None,
        10,
    ))
}

fn companion_app(live: Vec<LivePodInfo>, status: CompanionPanelStatus) -> DashboardApp {
    let mut panel = WorkspacePanelViewModel::empty(Path::new("test"));
    panel.header.companion = Some(CompanionPanelState::new("yoi", status, None));
    app_with_panel(
        PodList::from_sources(PodVisibilitySource::ResumePicker, vec![], live, None, 10),
        panel,
    )
}

fn ticket_enabled_app(live: Vec<LivePodInfo>) -> DashboardApp {
    ticket_enabled_app_with_orchestrator(live, OrchestratorPanelStatus::Live)
}

fn ticket_enabled_app_with_orchestrator(
    live: Vec<LivePodInfo>,
    orchestrator_status: OrchestratorPanelStatus,
) -> DashboardApp {
    let mut panel = WorkspacePanelViewModel::empty(Path::new("test"));
    panel.composer = crate::workspace_panel::WorkspacePanelComposer::ticket_enabled();
    panel.header.companion = Some(CompanionPanelState::new(
        "yoi",
        CompanionPanelStatus::Live,
        None,
    ));
    panel.header.orchestrator = Some(OrchestratorPanelState::new(
        "test-orchestrator",
        orchestrator_status,
        None,
    ));
    app_with_panel(
        PodList::from_sources(PodVisibilitySource::ResumePicker, vec![], live, None, 10),
        panel,
    )
}

fn app_with_list(list: PodList) -> DashboardApp {
    app_with_panel(list, WorkspacePanelViewModel::empty(Path::new("test")))
}

fn empty_test_list() -> PodList {
    PodList::from_sources(PodVisibilitySource::ResumePicker, vec![], vec![], None, 10)
}

fn panel_with_orchestrator(
    status: OrchestratorPanelStatus,
    detail: Option<&str>,
) -> WorkspacePanelViewModel {
    let mut panel = WorkspacePanelViewModel::empty(Path::new("test"));
    panel.header.orchestrator = Some(OrchestratorPanelState::new(
        "test-orchestrator",
        status,
        detail.map(str::to_string),
    ));
    if let Some(detail) = detail {
        panel.header.diagnostics.push(detail.to_string());
    }
    panel
}

fn app_with_panel(list: PodList, panel: WorkspacePanelViewModel) -> DashboardApp {
    let last_companion_lifecycle_failure = companion_lifecycle_failure_from_panel(&panel);
    let last_orchestrator_lifecycle_failure = orchestrator_lifecycle_failure_from_panel(&panel);
    let mut app = DashboardApp {
        list,
        panel,
        input: InputBuffer::new(),
        selected_row: None,
        row_hit_boxes: Vec::new(),
        composer_target: ComposerTarget::Companion,
        notice: None,
        panel_diagnostic: None,
        panel_diagnostic_open: false,
        sending: false,
        refreshing: false,
        enter_reload: None,
        runtime_command: PodRuntimeCommand::for_executable("/tmp/yoi"),
        last_companion_lifecycle_failure,
        last_orchestrator_lifecycle_failure,
        orchestrator_work_set: OrchestratorWorkSet::default(),
        orchestrator_queue_attention: None,
    };
    app.ensure_selection_visible();
    app.ensure_composer_target_available();
    app.refresh_orchestrator_work_set();
    app.apply_orchestrator_work_set_detail();
    app
}

fn panel_test_ticket_row(
    id: &str,
    title: &str,
    priority: ActionPriority,
    next_action: NextUserAction,
    state: &str,
) -> PanelRow {
    let ticket = crate::workspace_panel::TicketPanelEntry {
        id: id.to_string(),
        title: title.to_string(),
        priority: "P2".to_string(),
        workflow_state: TicketWorkflowState::parse(state).unwrap_or(TicketWorkflowState::Planning),
        workflow_state_explicit: true,
        orchestration_overlay: None,
        next_action: Some(next_action),
        updated_at: None,
        latest_event_kind: Some("implementation_report".to_string()),
        latest_event_excerpt: Some("latest event stays out of the primary row".to_string()),
        blocked_reason: None,
        related_pods: Vec::new(),
        local_claim: None,
        intake_pods: Vec::new(),
    };
    PanelRow {
        key: PanelRowKey::Ticket(ticket.id.clone()),
        kind: crate::workspace_panel::PanelRowKind::Ticket,
        title: title.to_string(),
        subtitle: Some("id · priority · latest event".to_string()),
        status: state.to_string(),
        priority,
        next_action: Some(next_action),
        ticket: Some(ticket),
        related_pods: Vec::new(),
        disabled_reason: None,
        key_hint: Some("Enter".to_string()),
    }
}

fn panel_test_intake_child_row(
    ticket_id: &str,
    pod_name: &str,
    status: TicketLocalClaimStatus,
    next_action: Option<NextUserAction>,
) -> PanelRow {
    PanelRow {
        key: PanelRowKey::TicketIntakePod {
            ticket_id: ticket_id.to_string(),
            pod_name: pod_name.to_string(),
        },
        kind: PanelRowKind::TicketIntakePod,
        title: format!("Intake Pod: {pod_name}"),
        subtitle: Some(format!("Intake claim for Ticket {ticket_id}")),
        status: status.label().to_string(),
        priority: match status {
            TicketLocalClaimStatus::Live | TicketLocalClaimStatus::Restorable => {
                ActionPriority::ActiveWork
            }
            TicketLocalClaimStatus::Stale => ActionPriority::Background,
        },
        next_action,
        ticket: None,
        related_pods: vec![pod_name.to_string()],
        disabled_reason: (status == TicketLocalClaimStatus::Stale)
            .then(|| "claim metadata is stale".to_string()),
        key_hint: Some(format!("Ticket {ticket_id} Intake Pod {pod_name}")),
    }
}

fn closed_list(count: usize, selected: Option<&str>) -> PodList {
    PodList::from_sources(
        PodVisibilitySource::ResumePicker,
        (0..count)
            .map(|index| {
                stopped_info_with_updated_at(&format!("closed-{index}"), 100 - index as u64)
            })
            .collect(),
        vec![],
        selected.map(str::to_string),
        count.max(1),
    )
}

fn live_info(pod_name: &str, status: PodStatus) -> LivePodInfo {
    live_info_with_updated_at(pod_name, status, 0)
}

fn unreachable_live_info(pod_name: &str) -> LivePodInfo {
    let mut live = live_info(pod_name, PodStatus::Idle);
    live.reachable = false;
    live.status = None;
    live
}

fn live_info_with_updated_at(pod_name: &str, status: PodStatus, updated_at: u64) -> LivePodInfo {
    LivePodInfo {
        pod_name: pod_name.to_string(),
        socket_path: PathBuf::from(format!("/tmp/{pod_name}.sock")),
        status: Some(status),
        reachable: true,
        segment_id: None,
        summary: PodEntrySummary {
            active_session_id: None,
            active_segment_id: None,
            updated_at,
            preview: None,
        },
    }
}

fn stopped_info(pod_name: &str) -> StoredPodInfo {
    stopped_info_with_updated_at(pod_name, 10)
}

fn stopped_info_with_updated_at(pod_name: &str, updated_at: u64) -> StoredPodInfo {
    StoredPodInfo {
        pod_name: pod_name.to_string(),
        metadata_state: StoredMetadataState::Present,
        active_session_id: None,
        active_segment_id: None,
        updated_at,
        workspace_root: None,
        preview: None,
    }
}

fn section_names<'a>(list: &'a PodList, section: &DashboardSection) -> Vec<&'a str> {
    section
        .entries
        .iter()
        .map(|index| list.entries[*index].name.as_str())
        .collect()
}

#[test]
fn ticket_action_error_records_f2_diagnostic_details() {
    let mut app = DashboardApp::loading(PodRuntimeCommand::for_executable("/tmp/yoi"));
    let long_error = "root-clean failed for Ticket 00001KTWPE3KQ at /home/hare/Projects/yoi: dirty file crates/tui/src/dashboard.rs";

    app.finish_ticket_action_dispatch(Err(TicketActionError::Stale(long_error.to_string())));

    assert!(app.notice.as_deref().unwrap().contains("F2 details"));
    let diagnostic = app.panel_diagnostic.as_ref().expect("diagnostic");
    assert_eq!(diagnostic.title, "Ticket action rejected");
    assert_eq!(diagnostic.details, long_error);
    assert!(!app.panel_diagnostic_open);

    assert!(matches!(
        app.handle_key(key(KeyCode::F(2))),
        DashboardAction::None
    ));
    assert!(app.panel_diagnostic_open);
    assert!(matches!(
        app.handle_key(key(KeyCode::Esc)),
        DashboardAction::None
    ));
    assert!(!app.panel_diagnostic_open);
}

fn plain_line(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

fn display_column(text: &str, needle: &str) -> usize {
    let byte_index = text.find(needle).unwrap();
    text[..byte_index].width()
}

fn input_text(app: &DashboardApp) -> String {
    Segment::flatten_to_text(&app.input.submit_segments())
}

fn key(code: KeyCode) -> KeyEvent {
    modified_key(code, KeyModifiers::NONE)
}

fn left_click(column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

fn modified_key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
    KeyEvent::new(code, modifiers)
}
