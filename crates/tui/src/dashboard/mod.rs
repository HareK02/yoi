use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use client::ticket_role::{
    TicketIntakeHandoff, TicketRef, TicketRole, TicketRoleLaunchContext, TicketRoleLaunchError,
    TicketRoleLaunchOptions, TicketRoleLaunchResult, launch_ticket_role_pod,
    launch_ticket_role_pod_with_options, plan_ticket_role_launch,
};
use client::{PodRuntimeCommand, SpawnConfig, spawn_pod};
use crossterm::event::{
    Event as TermEvent, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    poll, read,
};
use pod_store::FsPodStore;
use protocol::stream::{JsonLineReader, JsonLineWriter};
use protocol::{ErrorCode, Event, Method, PodStatus, Segment};
use ratatui::Frame;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget, Wrap};
use serde::Serialize;
use session_store::FsStore;
use ticket::config::{GitBranchName, TicketConfig, TicketOrchestrationConfig};
use ticket::{
    LocalTicketBackend, MarkdownText, TicketBackend, TicketIdOrSlug, TicketStateChange,
    TicketWorkflowState,
};
use tokio::net::UnixStream;
use unicode_width::UnicodeWidthStr;

use crate::composer_keys::{ComposerEditAction, composer_edit_action};
use crate::input::InputBuffer;
use crate::pod_list::{
    PodList, PodListEntry, PodVisibilitySource, StoredMetadataState, read_reachable_live_pod_infos,
    read_stored_pod_infos,
};
use crate::role_session_registry::{
    PanelRegistryStore, RelatedTicketRef, RoleSessionOrigin, TicketClaimResult,
};
#[cfg(not(feature = "e2e-test"))]
use crate::workspace_panel::build_workspace_panel;
#[cfg(feature = "e2e-test")]
use crate::workspace_panel::build_workspace_panel_with_e2e_timings;
use crate::workspace_panel::{
    ActionPriority, CompanionLifecyclePlan, CompanionPanelState, CompanionPanelStatus,
    CompanionPodPresence, ComposerTarget, NextUserAction, OrchestratorLifecyclePlan,
    OrchestratorPanelState, OrchestratorPanelStatus, OrchestratorPodPresence, PanelRow,
    PanelRowKey, PanelRowKind, TicketConfigAvailability, TicketLocalClaimStatus,
    WorkspacePanelViewModel, bounded_panel_diagnostic, build_current_ticket_row,
    companion_pod_presence, decide_companion_lifecycle, decide_orchestrator_lifecycle,
    local_claim_status_for_pod, orchestrator_pod_presence, ticket_config_availability,
    workspace_companion_pod_name, workspace_orchestrator_pod_name,
};

mod render;

use render::{PanelListRow, row_hit_boxes};

const MAX_ENTRIES: usize = 50;
const CLOSED_VISIBLE_ROWS: usize = 3;
const ORCHESTRATOR_IDLE_QUEUE_NOTICE_TEMPLATE: &str =
    include_str!("../../../../resources/prompts/panel/orchestrator_idle_queue_notice.md");
const ORCHESTRATOR_QUEUE_ATTENTION_MAX_TICKETS: usize = 6;
const ORCHESTRATOR_QUEUE_ATTENTION_MAX_TEXT_CHARS: usize = 120;
const ORCHESTRATOR_QUEUE_ATTENTION_MAX_MESSAGE_CHARS: usize = 2_400;
const SOCKET_OP_TIMEOUT: Duration = Duration::from_secs(3);
const DASHBOARD_POLL_INTERVAL: Duration = Duration::from_millis(1_500);
const TERMINAL_EVENT_POLL_INTERVAL: Duration = Duration::from_millis(100);
const PANEL_READY_REFINEMENT_MAX_INSTRUCTION_CHARS: usize = 4_000;

#[derive(Debug)]
pub(crate) enum DashboardError {
    Io(io::Error),
    Store(session_store::StoreError),
    NoPods,
}

impl std::fmt::Display for DashboardError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "io error: {e}"),
            Self::Store(e) => write!(f, "session store error: {e}"),
            Self::NoPods => write!(
                f,
                "no Tickets or Pods found — create a Ticket with `yoi ticket create` or restore a Pod with `yoi -r`"
            ),
        }
    }
}

impl std::error::Error for DashboardError {}

impl From<io::Error> for DashboardError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<session_store::StoreError> for DashboardError {
    fn from(e: session_store::StoreError) -> Self {
        Self::Store(e)
    }
}

pub(crate) enum DashboardOutcome {
    Quit,
    Open(OpenPodRequest),
}

pub(crate) async fn launch(runtime_command: PodRuntimeCommand) -> Result<(), Box<dyn Error>> {
    let mut app = load_app(runtime_command.clone()).await?;
    let mut terminal = crate::console::enter_dashboard_fullscreen()?;
    loop {
        match run_loop(&mut terminal, &mut app).await? {
            DashboardOutcome::Quit => {
                crate::console::leave_dashboard_fullscreen(&mut terminal)?;
                return Ok(());
            }
            DashboardOutcome::Open(request) => {
                let pod_name = request.pod_name.clone();
                let console_request = crate::console::DashboardConsoleOpenRequest {
                    pod_name: request.pod_name,
                    socket_override: request.socket_override,
                };
                let result = crate::console::open_from_dashboard(
                    &mut terminal,
                    console_request,
                    runtime_command.clone(),
                )
                .await;
                if let Err(error) = result {
                    app.finish_open(&pod_name, Err(&error));
                    crate::console::leave_dashboard_fullscreen(&mut terminal)?;
                    if crate::console::is_recoverable_dashboard_open_error(error.as_ref()) {
                        return Ok(());
                    }
                    return Err(error);
                }
                app.finish_open(&pod_name, Ok(()));
                app = load_app(runtime_command.clone()).await?;
                let key = PanelRowKey::Pod(pod_name);
                if app.panel.row(&key).is_some() {
                    app.select_panel_key(key);
                }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OpenPodRequest {
    pub(crate) pod_name: String,
    pub(crate) socket_override: Option<PathBuf>,
}

pub(crate) async fn load_app(
    runtime_command: PodRuntimeCommand,
) -> Result<DashboardApp, DashboardError> {
    Ok(DashboardApp::loading(runtime_command))
}

async fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut DashboardApp,
) -> Result<DashboardOutcome, DashboardError> {
    if app.panel.rows.is_empty()
        && app.panel.header.diagnostics.is_empty()
        && app.enter_reload.is_none()
    {
        return Err(DashboardError::NoPods);
    }

    let mut pending_reload = PendingReload::default();
    let mut pending_queue_attention_notice = PendingQueueAttentionNotice::default();
    let mut deferred_enter_reload = app.enter_reload.take();
    let mut next_poll = Instant::now() + DASHBOARD_POLL_INTERVAL;
    #[cfg(feature = "e2e-test")]
    let mut emitted_panel_ready = false;

    loop {
        if let Some(result) = pending_queue_attention_notice.finish_if_ready().await {
            app.finish_orchestrator_queue_attention_notice(result);
        }
        if let Some(result) = pending_reload.finish_if_ready().await {
            app.apply_reload_result(result);
            if let Some(request) = app.prepare_orchestrator_queue_attention_notice() {
                pending_queue_attention_notice.start(request);
            }
        }

        terminal.draw(|f| render::draw(f, app))?;
        #[cfg(feature = "e2e-test")]
        {
            if !emitted_panel_ready {
                // `panel_ready` is a first-visible-frame signal only. E2E tests that need
                // list/data readiness must wait for a concrete `rows_rendered` fixture row.
                crate::e2e_observer::emit("panel", "panel_ready", serde_json::json!({}));
                emitted_panel_ready = true;
            }
            // Emit every drawn row snapshot separately so tests can assert data-backed row
            // readiness without conflating it with the first frame.
            app.emit_rows_rendered();
        }

        if let Some(mode) = deferred_enter_reload.take() {
            if pending_reload.start(mode) {
                app.refreshing = true;
            }
        }

        let now = Instant::now();
        if now >= next_poll {
            pending_reload.start(OrchestratorLifecycleMode::Observe);
            next_poll = now + DASHBOARD_POLL_INTERVAL;
            continue;
        }

        let event_wait = TERMINAL_EVENT_POLL_INTERVAL.min(next_poll.saturating_duration_since(now));
        if !poll(event_wait)? {
            continue;
        }

        match read()? {
            TermEvent::Key(key) => match app.handle_key(key) {
                DashboardAction::None => {}
                DashboardAction::Quit => {
                    #[cfg(feature = "e2e-test")]
                    crate::e2e_observer::emit("panel", "quit_requested", serde_json::json!({}));
                    abort_panel_background_work_for_quit(
                        &mut pending_reload,
                        &mut pending_queue_attention_notice,
                    );
                    return Ok(DashboardOutcome::Quit);
                }
                DashboardAction::Open => {
                    #[cfg(feature = "e2e-test")]
                    crate::e2e_observer::emit(
                        "panel",
                        "action_requested",
                        serde_json::json!({ "action": "open" }),
                    );
                    if let Some(request) = app.prepare_open() {
                        terminal.draw(|f| render::draw(f, app))?;
                        return Ok(DashboardOutcome::Open(request));
                    }
                }
                DashboardAction::DispatchTicketAction(request) => {
                    #[cfg(feature = "e2e-test")]
                    crate::e2e_observer::emit(
                        "panel",
                        "action_requested",
                        serde_json::json!({ "action": "ticket_action" }),
                    );
                    pending_reload.abort();
                    pending_queue_attention_notice.abort();
                    terminal.draw(|f| render::draw(f, app))?;
                    let result = dispatch_ticket_action(request).await;
                    app.finish_ticket_action_dispatch(result);
                    if pending_reload.start(OrchestratorLifecycleMode::Observe) {
                        app.refreshing = true;
                    }
                    next_poll = Instant::now() + DASHBOARD_POLL_INTERVAL;
                }
                DashboardAction::ReturnReadyTicketToPlanning(request) => {
                    #[cfg(feature = "e2e-test")]
                    crate::e2e_observer::emit(
                        "panel",
                        "action_requested",
                        serde_json::json!({ "action": "return_ready_ticket_to_planning" }),
                    );
                    pending_reload.abort();
                    pending_queue_attention_notice.abort();
                    terminal.draw(|f| render::draw(f, app))?;
                    match dispatch_ready_ticket_planning_return(request).await {
                        Ok(outcome) => {
                            match app.finish_ready_ticket_planning_return_success(outcome) {
                                ReadyTicketPlanningReturnAfterMutation::LaunchIntake(request) => {
                                    terminal.draw(|f| render::draw(f, app))?;
                                    let planning_notice = app.notice.clone().unwrap_or_default();
                                    let result = launch_intake_with_handoff(request).await;
                                    app.finish_ready_ticket_planning_return_with_intake_launch(
                                        planning_notice,
                                        result,
                                    );
                                }
                                ReadyTicketPlanningReturnAfterMutation::OpenClaim(request) => {
                                    terminal.draw(|f| render::draw(f, app))?;
                                    return Ok(DashboardOutcome::Open(request));
                                }
                                ReadyTicketPlanningReturnAfterMutation::None => {}
                            }
                        }
                        Err(error) => app.finish_ready_ticket_planning_return_error(error),
                    }
                    if pending_reload.start(OrchestratorLifecycleMode::Observe) {
                        app.refreshing = true;
                    }
                    next_poll = Instant::now() + DASHBOARD_POLL_INTERVAL;
                }
                DashboardAction::LaunchIntake(request) => {
                    #[cfg(feature = "e2e-test")]
                    crate::e2e_observer::emit(
                        "panel",
                        "action_requested",
                        serde_json::json!({ "action": "launch_intake" }),
                    );
                    pending_reload.abort();
                    pending_queue_attention_notice.abort();
                    terminal.draw(|f| render::draw(f, app))?;
                    let result = launch_intake_with_handoff(request).await;
                    app.finish_intake_launch(result);
                    if pending_reload.start(OrchestratorLifecycleMode::Observe) {
                        app.refreshing = true;
                    }
                    next_poll = Instant::now() + DASHBOARD_POLL_INTERVAL;
                }
                DashboardAction::SendCompanion(request) => {
                    #[cfg(feature = "e2e-test")]
                    crate::e2e_observer::emit(
                        "panel",
                        "action_requested",
                        serde_json::json!({ "action": "send_companion" }),
                    );
                    pending_reload.abort();
                    pending_queue_attention_notice.abort();
                    terminal.draw(|f| render::draw(f, app))?;
                    let result = dispatch_companion_message(request).await;
                    app.finish_companion_send(result);
                    if pending_reload.start(OrchestratorLifecycleMode::Observe) {
                        app.refreshing = true;
                    }
                    next_poll = Instant::now() + DASHBOARD_POLL_INTERVAL;
                }
            },
            TermEvent::Paste(text) => app.input.insert_paste(text),
            TermEvent::Mouse(mouse) => {
                app.handle_mouse_event(mouse);
            }
            TermEvent::Resize(_, _) => {}
            _ => {}
        }
    }
}

struct PendingReload {
    handle: Option<tokio::task::JoinHandle<Result<DashboardSnapshot, DashboardError>>>,
}

impl PendingReload {
    fn start(&mut self, lifecycle_mode: OrchestratorLifecycleMode) -> bool {
        if self.handle.is_some() {
            return false;
        }
        #[cfg(feature = "e2e-test")]
        crate::e2e_observer::emit(
            "panel",
            "background_task_started",
            serde_json::json!({
                "task": "reload",
                "lifecycle_mode": format!("{lifecycle_mode:?}"),
            }),
        );
        self.handle = Some(tokio::spawn(async move {
            #[cfg(feature = "e2e-test")]
            crate::e2e_observer::hold_background_task_if_requested("reload").await;
            load_dashboard_snapshot(None, lifecycle_mode).await
        }));
        true
    }

    #[cfg(test)]
    fn start_with_handle(
        &mut self,
        handle: tokio::task::JoinHandle<Result<DashboardSnapshot, DashboardError>>,
    ) -> bool {
        if self.handle.is_some() {
            handle.abort();
            return false;
        }
        self.handle = Some(handle);
        true
    }

    async fn finish_if_ready(&mut self) -> Option<Result<DashboardSnapshot, DashboardError>> {
        if !self.handle.as_ref()?.is_finished() {
            return None;
        }
        let handle = self.handle.take()?;
        #[cfg(feature = "e2e-test")]
        crate::e2e_observer::emit(
            "panel",
            "background_task_finished",
            serde_json::json!({ "task": "reload" }),
        );
        Some(match handle.await {
            Ok(result) => result,
            Err(e) => Err(DashboardError::Io(io::Error::other(format!(
                "reload task failed: {e}"
            )))),
        })
    }

    fn abort(&mut self) {
        if let Some(handle) = self.handle.take() {
            #[cfg(feature = "e2e-test")]
            crate::e2e_observer::emit(
                "panel",
                "background_task_aborted",
                serde_json::json!({ "task": "reload" }),
            );
            handle.abort();
        }
    }
}

impl Default for PendingReload {
    fn default() -> Self {
        Self { handle: None }
    }
}

impl Drop for PendingReload {
    fn drop(&mut self) {
        self.abort();
    }
}

struct PendingQueueAttentionNotice {
    handle: Option<tokio::task::JoinHandle<OrchestratorQueueAttentionNoticeResult>>,
}

impl PendingQueueAttentionNotice {
    fn start(&mut self, request: OrchestratorQueueAttentionNoticeRequest) -> bool {
        if self.handle.is_some() {
            return false;
        }
        self.handle = Some(tokio::spawn(async move {
            dispatch_orchestrator_queue_attention_notice(request).await
        }));
        true
    }

    #[cfg(test)]
    fn start_with_handle(
        &mut self,
        handle: tokio::task::JoinHandle<OrchestratorQueueAttentionNoticeResult>,
    ) -> bool {
        if self.handle.is_some() {
            handle.abort();
            return false;
        }
        self.handle = Some(handle);
        true
    }

    async fn finish_if_ready(&mut self) -> Option<OrchestratorQueueAttentionNoticeResult> {
        if !self.handle.as_ref()?.is_finished() {
            return None;
        }
        let handle = self.handle.take()?;
        Some(match handle.await {
            Ok(result) => result,
            Err(e) => OrchestratorQueueAttentionNoticeResult::failed(
                String::new(),
                format!("queue-attention notice task failed: {e}"),
            ),
        })
    }

    fn abort(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.abort();
        }
    }
}

impl Default for PendingQueueAttentionNotice {
    fn default() -> Self {
        Self { handle: None }
    }
}

impl Drop for PendingQueueAttentionNotice {
    fn drop(&mut self) {
        self.abort();
    }
}

fn abort_panel_background_work_for_quit(
    pending_reload: &mut PendingReload,
    pending_queue_attention_notice: &mut PendingQueueAttentionNotice,
) {
    pending_reload.abort();
    pending_queue_attention_notice.abort();
}

fn default_store_dir() -> Result<PathBuf, DashboardError> {
    manifest::paths::sessions_dir().ok_or_else(|| {
        DashboardError::Io(io::Error::new(
            io::ErrorKind::NotFound,
            "could not resolve sessions directory",
        ))
    })
}

fn default_pod_store_dir() -> Result<PathBuf, DashboardError> {
    manifest::paths::data_dir()
        .map(|dir| dir.join("pods"))
        .ok_or_else(|| {
            DashboardError::Io(io::Error::new(
                io::ErrorKind::NotFound,
                "could not resolve pod state directory",
            ))
        })
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OpenEligibility {
    OpenNow,
    Disabled,
}

#[derive(Debug)]
pub(crate) struct IntakeLaunchRequest {
    context: TicketRoleLaunchContext,
    runtime_command: PodRuntimeCommand,
    peer_registration: IntakePeerRegistrationRequest,
    registry_update: IntakeRegistryUpdate,
}

#[derive(Debug, Clone)]
pub(crate) enum IntakeRegistryUpdate {
    RecordSession {
        registry_root: PathBuf,
        pod_name: String,
        origin: RoleSessionOrigin,
        related_tickets: Vec<RelatedTicketRef>,
    },
    ClaimTicket {
        registry_root: PathBuf,
        ticket_id: String,
        ticket_slug: Option<String>,
        pod_name: String,
    },
    ClaimLaunchedTicket {
        registry_root: PathBuf,
        ticket_id: String,
        ticket_slug: Option<String>,
    },
}

#[derive(Debug)]
pub(crate) struct ReadyTicketPlanningReturnRequest {
    workspace_root: PathBuf,
    ticket_id: String,
    user_instruction: String,
    followup: ReadyTicketPlanningReturnFollowup,
}

#[derive(Debug)]
pub(crate) enum ReadyTicketPlanningReturnFollowup {
    LaunchIntake(IntakeLaunchRequest),
    NotifyLiveClaimedIntake {
        pod_name: String,
        socket_path: PathBuf,
    },
    OpenRestorableClaimedIntake(OpenPodRequest),
    BlockedByStaleClaim {
        pod_name: String,
    },
}

#[derive(Debug)]
struct ReadyTicketPlanningReturnOutcome {
    notice: String,
    followup: ReadyTicketPlanningReturnAfterMutation,
}

#[derive(Debug)]
enum ReadyTicketPlanningReturnAfterMutation {
    LaunchIntake(IntakeLaunchRequest),
    OpenClaim(OpenPodRequest),
    None,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum IntakePeerRegistrationRequest {
    Register { orchestrator_pod: String },
    Skip { reason: String },
}

#[derive(Debug, Clone)]
pub(crate) struct IntakeLaunchOutcome {
    launch: TicketRoleLaunchResult,
    peer_registration: IntakePeerRegistrationStatus,
    registry_warning: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum IntakePeerRegistrationStatus {
    Registered { orchestrator_pod: String },
    Warning { message: String },
}

impl IntakePeerRegistrationStatus {
    fn warning(message: impl Into<String>) -> Self {
        Self::Warning {
            message: bounded_panel_diagnostic(message.into()),
        }
    }
}

pub(crate) type IntakeLaunchResult = Result<IntakeLaunchOutcome, TicketRoleLaunchError>;

pub(crate) async fn dispatch_companion_message(
    request: CompanionSendRequest,
) -> Result<CompanionSendOutcome, CompanionSendError> {
    let stream = tokio::time::timeout(SOCKET_OP_TIMEOUT, UnixStream::connect(&request.socket_path))
        .await
        .map_err(|_| CompanionSendError::Rejected {
            pod_name: request.pod_name.clone(),
            message: "connect timed out".to_string(),
        })?
        .map_err(|source| CompanionSendError::Connect {
            pod_name: request.pod_name.clone(),
            source,
        })?;
    let (read_half, write_half) = stream.into_split();
    let mut reader = JsonLineReader::new(read_half);
    let mut writer = JsonLineWriter::new(write_half);

    loop {
        let event = tokio::time::timeout(SOCKET_OP_TIMEOUT, reader.next::<Event>())
            .await
            .map_err(|_| CompanionSendError::Rejected {
                pod_name: request.pod_name.clone(),
                message: "initial Snapshot timed out".to_string(),
            })?
            .map_err(|source| CompanionSendError::Read {
                pod_name: request.pod_name.clone(),
                source,
            })?;
        match event {
            Some(Event::Snapshot { .. }) => break,
            Some(Event::Alert(_)) => continue,
            Some(Event::Error { message, .. }) => {
                return Err(CompanionSendError::Rejected {
                    pod_name: request.pod_name,
                    message,
                });
            }
            Some(_) => continue,
            None => {
                return Err(CompanionSendError::Closed {
                    pod_name: request.pod_name,
                });
            }
        }
    }

    tokio::time::timeout(
        SOCKET_OP_TIMEOUT,
        writer.write(&Method::Run {
            input: request.segments,
        }),
    )
    .await
    .map_err(|_| CompanionSendError::Rejected {
        pod_name: request.pod_name.clone(),
        message: "write timed out".to_string(),
    })?
    .map_err(|source| CompanionSendError::Write {
        pod_name: request.pod_name.clone(),
        source,
    })?;

    loop {
        match tokio::time::timeout(SOCKET_OP_TIMEOUT, reader.next::<Event>()).await {
            Ok(Ok(Some(Event::UserMessage { .. }))) => {
                return Ok(CompanionSendOutcome {
                    notice: format!("Sent to Companion {}.", request.pod_name),
                });
            }
            Ok(Ok(Some(Event::Error { message, .. }))) => {
                return Err(CompanionSendError::Rejected {
                    pod_name: request.pod_name,
                    message,
                });
            }
            Ok(Ok(Some(Event::Snapshot { .. } | Event::Alert(_)))) => continue,
            Ok(Ok(Some(_))) => continue,
            Ok(Ok(None)) => {
                return Err(CompanionSendError::Closed {
                    pod_name: request.pod_name,
                });
            }
            Ok(Err(source)) => {
                return Err(CompanionSendError::Read {
                    pod_name: request.pod_name,
                    source,
                });
            }
            Err(_) => {
                return Err(CompanionSendError::Rejected {
                    pod_name: request.pod_name,
                    message: "acceptance read timed out".to_string(),
                });
            }
        }
    }
}

async fn launch_intake_with_handoff(request: IntakeLaunchRequest) -> IntakeLaunchResult {
    let (options, orchestrator_pod, skip_warning) = match request.peer_registration.clone() {
        IntakePeerRegistrationRequest::Register { orchestrator_pod } => (
            TicketRoleLaunchOptions::default()
                .with_pre_run_peer_registration(orchestrator_pod.clone()),
            Some(orchestrator_pod),
            None,
        ),
        IntakePeerRegistrationRequest::Skip { reason } => (
            TicketRoleLaunchOptions::default(),
            None,
            Some(IntakePeerRegistrationStatus::warning(format!(
                "handoff peer registration skipped: {reason}"
            ))),
        ),
    };
    let launch = launch_ticket_role_pod_with_options(
        request.context,
        request.runtime_command,
        |_| {},
        options,
    )
    .await?;
    let registry_warning =
        commit_intake_registry_update(request.registry_update, Some(&launch.plan.pod_name));
    let peer_registration = match (orchestrator_pod, skip_warning) {
        (_, Some(warning)) => warning,
        (Some(orchestrator_pod), None) if launch.pre_run_warnings.is_empty() => {
            IntakePeerRegistrationStatus::Registered { orchestrator_pod }
        }
        (Some(_), None) => IntakePeerRegistrationStatus::warning(
            launch
                .pre_run_warnings
                .iter()
                .map(|warning| warning.message.as_str())
                .collect::<Vec<_>>()
                .join("; "),
        ),
        (None, None) => IntakePeerRegistrationStatus::warning(
            "handoff peer registration skipped: no Orchestrator target",
        ),
    };
    Ok(IntakeLaunchOutcome {
        launch,
        peer_registration,
        registry_warning,
    })
}

fn commit_intake_registry_update(
    update: IntakeRegistryUpdate,
    launched_pod_name: Option<&str>,
) -> Option<String> {
    match update {
        IntakeRegistryUpdate::RecordSession {
            registry_root,
            pod_name,
            origin,
            related_tickets,
        } => PanelRegistryStore::from_root(registry_root)
            .record_session(
                pod_name,
                TicketRole::Intake.as_str().to_string(),
                origin,
                None,
                related_tickets,
            )
            .err()
            .map(|error| {
                bounded_panel_diagnostic(format!(
                    "local role session registry could not be updated after Intake launch: {error}"
                ))
            }),
        IntakeRegistryUpdate::ClaimTicket {
            registry_root,
            ticket_id,
            ticket_slug,
            pod_name,
        } => match PanelRegistryStore::from_root(registry_root).claim_ticket(
            &ticket_id,
            ticket_slug.as_deref(),
            &pod_name,
            TicketRole::Intake.as_str(),
        ) {
            Ok(TicketClaimResult::Claimed) | Ok(TicketClaimResult::AlreadyOwned(_)) => None,
            Err(error) => Some(bounded_panel_diagnostic(format!(
                "local Ticket Intake claim could not be committed after launch acceptance: {error}"
            ))),
        },
        IntakeRegistryUpdate::ClaimLaunchedTicket {
            registry_root,
            ticket_id,
            ticket_slug,
        } => {
            let Some(pod_name) = launched_pod_name else {
                return Some(
                    "local Ticket Intake claim could not be committed after launch acceptance: missing launched Pod name"
                        .to_string(),
                );
            };
            match PanelRegistryStore::from_root(registry_root).claim_ticket(
                &ticket_id,
                ticket_slug.as_deref(),
                pod_name,
                TicketRole::Intake.as_str(),
            ) {
                Ok(TicketClaimResult::Claimed) | Ok(TicketClaimResult::AlreadyOwned(_)) => None,
                Err(error) => Some(bounded_panel_diagnostic(format!(
                    "local Ticket Intake claim could not be committed after launch acceptance: {error}"
                ))),
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PanelDiagnostic {
    title: String,
    details: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct OrchestratorWorkSet {
    active_inprogress: Vec<OrchestratorActiveWorkItem>,
    queued: Vec<OrchestratorQueuedWorkItem>,
    fingerprint: String,
}

impl OrchestratorWorkSet {
    fn is_empty(&self) -> bool {
        self.active_inprogress.is_empty() && self.queued.is_empty()
    }

    fn has_active_inprogress(&self) -> bool {
        !self.active_inprogress.is_empty()
    }

    fn planned_queued_ids(&self) -> BTreeSet<String> {
        self.queued.iter().map(|item| item.id.clone()).collect()
    }

    fn actionable_queued(&self) -> Vec<&OrchestratorQueuedWorkItem> {
        self.queued
            .iter()
            .filter(|item| item.waiting_reason.is_none())
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct OrchestratorActiveWorkItem {
    id: String,
    title: String,
    status: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct OrchestratorQueuedWorkItem {
    id: String,
    title: String,
    classification: OrchestratorQueuedClassification,
    waiting_reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OrchestratorQueuedClassification {
    NewQueued,
    PlannedQueued,
}

impl OrchestratorQueuedClassification {
    fn as_str(self) -> &'static str {
        match self {
            Self::NewQueued => "new_queued",
            Self::PlannedQueued => "planned_queued",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct OrchestratorQueueAttentionFreshness {
    fingerprint: String,
    updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct OrchestratorQueueAttentionNotice {
    message: String,
    fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct OrchestratorQueueAttentionNoticeRequest {
    pod_name: String,
    socket_path: PathBuf,
    notice: OrchestratorQueueAttentionNotice,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct OrchestratorQueueAttentionNoticeResult {
    fingerprint: String,
    updated_at: String,
    error: Option<String>,
}

impl OrchestratorQueueAttentionNoticeResult {
    fn sent(fingerprint: String, updated_at: String) -> Self {
        Self {
            fingerprint,
            updated_at,
            error: None,
        }
    }

    fn failed(fingerprint: String, error: impl Into<String>) -> Self {
        Self {
            fingerprint,
            updated_at: String::new(),
            error: Some(error.into()),
        }
    }
}

#[derive(Debug, Serialize)]
struct OrchestratorQueueTemplateContext {
    workspace: String,
    actionable_tickets: Vec<OrchestratorQueueTemplateTicket>,
    waiting_tickets: Vec<OrchestratorQueueTemplateTicket>,
    omitted_ticket_count: usize,
}

#[derive(Debug, Serialize)]
struct OrchestratorQueueTemplateTicket {
    id: String,
    title: String,
    classification: &'static str,
    waiting_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PanelRowHitBox {
    rect: Rect,
    key: PanelRowKey,
}

impl PanelRowHitBox {
    fn contains(&self, column: u16, row: u16) -> bool {
        column >= self.rect.x
            && column < self.rect.x.saturating_add(self.rect.width)
            && row >= self.rect.y
            && row < self.rect.y.saturating_add(self.rect.height)
    }
}

#[cfg(feature = "e2e-test")]
#[derive(Debug, Clone, Serialize)]
struct PanelE2eRowKey {
    kind: &'static str,
    id: String,
}

#[cfg(feature = "e2e-test")]
#[derive(Debug, Clone, Serialize)]
struct PanelE2eRect {
    x: u16,
    y: u16,
    width: u16,
    height: u16,
}

#[cfg(feature = "e2e-test")]
#[derive(Debug, Clone, Serialize)]
struct PanelE2eRenderedRow {
    key: PanelE2eRowKey,
    title: String,
    status: Option<String>,
    action: Option<&'static str>,
    disabled_reason: Option<String>,
    local_state: Option<String>,
    overlay_state: Option<String>,
    overlay_detail: Option<String>,
    rect: PanelE2eRect,
}

#[cfg(feature = "e2e-test")]
#[derive(Debug, Clone, Serialize)]
struct PanelE2eRowsRendered {
    selected: Option<PanelE2eRowKey>,
    header: PanelE2eDashboardHeader,
    rows: Vec<PanelE2eRenderedRow>,
}

#[cfg(feature = "e2e-test")]
#[derive(Debug, Clone, Serialize)]
struct PanelE2eDashboardHeader {
    ticket_configured: bool,
    companion: Option<PanelE2eCompanionState>,
    orchestrator: Option<PanelE2eOrchestratorState>,
    diagnostics: Vec<String>,
}

#[cfg(feature = "e2e-test")]
#[derive(Debug, Clone, Serialize)]
struct PanelE2eCompanionState {
    pod_name: String,
    status: &'static str,
}

#[cfg(feature = "e2e-test")]
#[derive(Debug, Clone, Serialize)]
struct PanelE2eOrchestratorState {
    pod_name: String,
    status: &'static str,
    detail: Option<String>,
}

#[cfg(feature = "e2e-test")]
#[derive(Debug, Serialize)]
struct PanelE2eDashboardContentReady {
    snapshot: PanelE2eDashboardSnapshot,
    categories: PanelE2eDashboardCategories,
}

#[cfg(feature = "e2e-test")]
#[derive(Debug, Clone, Serialize)]
struct PanelE2eDashboardSnapshot {
    header: PanelE2eDashboardHeader,
    rows: Vec<PanelE2eRenderedRow>,
}

#[cfg(feature = "e2e-test")]
#[derive(Debug, Serialize)]
struct PanelE2eDashboardCategories {
    ticket_rows: usize,
    ready_ticket_rows: usize,
    planning_ticket_rows: usize,
    pod_rows: usize,
    actionable_rows: usize,
}

#[cfg(feature = "e2e-test")]
#[derive(Debug, Serialize)]
struct PanelE2eSourceTiming {
    source: &'static str,
    elapsed_ms: u128,
}

#[cfg(feature = "e2e-test")]
#[derive(Debug, Serialize)]
struct PanelE2eDashboardSourceBreakdown {
    total_elapsed_ms: u128,
    sources: Vec<PanelE2eSourceTiming>,
    ticket_rows: usize,
    pod_rows: usize,
    diagnostics: usize,
}

#[cfg(feature = "e2e-test")]
fn panel_e2e_row_key(key: &PanelRowKey) -> PanelE2eRowKey {
    match key {
        PanelRowKey::Ticket(id) => PanelE2eRowKey {
            kind: "ticket",
            id: id.clone(),
        },
        PanelRowKey::InvalidTicket(label) => PanelE2eRowKey {
            kind: "invalid_ticket",
            id: label.clone(),
        },
        PanelRowKey::TicketIntakePod {
            ticket_id,
            pod_name,
        } => PanelE2eRowKey {
            kind: "ticket_intake_pod",
            id: format!("{ticket_id}:{pod_name}"),
        },
        PanelRowKey::Pod(name) => PanelE2eRowKey {
            kind: "pod",
            id: name.clone(),
        },
    }
}

#[cfg(feature = "e2e-test")]
fn panel_e2e_rect(rect: Rect) -> PanelE2eRect {
    PanelE2eRect {
        x: rect.x,
        y: rect.y,
        width: rect.width,
        height: rect.height,
    }
}

#[cfg(feature = "e2e-test")]
fn panel_e2e_dashboard_categories(rows: &[PanelE2eRenderedRow]) -> PanelE2eDashboardCategories {
    PanelE2eDashboardCategories {
        ticket_rows: rows.iter().filter(|row| row.key.kind == "ticket").count(),
        ready_ticket_rows: rows
            .iter()
            .filter(|row| row.key.kind == "ticket" && row.local_state.as_deref() == Some("ready"))
            .count(),
        planning_ticket_rows: rows
            .iter()
            .filter(|row| {
                row.key.kind == "ticket" && row.local_state.as_deref() == Some("planning")
            })
            .count(),
        pod_rows: rows.iter().filter(|row| row.key.kind == "pod").count(),
        actionable_rows: rows.iter().filter(|row| row.action.is_some()).count(),
    }
}

#[cfg(feature = "e2e-test")]
fn panel_e2e_dashboard_header(panel: &WorkspacePanelViewModel) -> PanelE2eDashboardHeader {
    PanelE2eDashboardHeader {
        ticket_configured: panel.header.ticket_configured,
        companion: panel
            .header
            .companion
            .as_ref()
            .map(|state| PanelE2eCompanionState {
                pod_name: state.pod_name.clone(),
                status: state.status.label(),
            }),
        orchestrator: panel
            .header
            .orchestrator
            .as_ref()
            .map(|state| PanelE2eOrchestratorState {
                pod_name: state.pod_name.clone(),
                status: state.status.label(),
                detail: state.detail.clone(),
            }),
        diagnostics: panel.header.diagnostics.clone(),
    }
}

#[cfg(feature = "e2e-test")]
fn panel_e2e_dashboard_content_is_ready(
    snapshot: &PanelE2eDashboardSnapshot,
    categories: &PanelE2eDashboardCategories,
) -> bool {
    snapshot.header.ticket_configured
        && snapshot.header.companion.is_some()
        && snapshot.header.orchestrator.is_some()
        && categories.ready_ticket_rows > 0
        && categories.planning_ticket_rows > 0
        && categories.pod_rows > 0
        && snapshot.rows.iter().any(|row| {
            row.key.kind == "ticket"
                && row.local_state.as_deref() == Some("ready")
                && row.overlay_state.is_some()
                && row.action.is_some()
                && row.disabled_reason.is_some()
        })
        && snapshot.rows.iter().any(|row| {
            row.key.kind == "ticket"
                && row.local_state.as_deref() == Some("planning")
                && row.action.is_some()
                && row.disabled_reason.is_some()
        })
}

pub(crate) struct DashboardApp {
    pub(crate) list: PodList,
    pub(crate) panel: WorkspacePanelViewModel,
    pub(crate) input: InputBuffer,
    selected_row: Option<PanelRowKey>,
    row_hit_boxes: Vec<PanelRowHitBox>,
    composer_target: ComposerTarget,
    notice: Option<String>,
    panel_diagnostic: Option<PanelDiagnostic>,
    panel_diagnostic_open: bool,
    sending: bool,
    refreshing: bool,
    enter_reload: Option<OrchestratorLifecycleMode>,
    runtime_command: PodRuntimeCommand,
    last_companion_lifecycle_failure: Option<CompanionPanelState>,
    last_orchestrator_lifecycle_failure: Option<OrchestratorPanelState>,
    orchestrator_work_set: OrchestratorWorkSet,
    orchestrator_queue_attention: Option<OrchestratorQueueAttentionFreshness>,
    #[cfg(feature = "e2e-test")]
    emitted_dashboard_content_ready: bool,
}

impl DashboardApp {
    fn loading(runtime_command: PodRuntimeCommand) -> Self {
        let workspace_root = current_workspace_root();
        let mut panel = WorkspacePanelViewModel::empty(&workspace_root);
        panel
            .header
            .diagnostics
            .push("Loading workspace dashboard…".to_string());
        Self {
            list: PodList::from_sources(
                PodVisibilitySource::ResumePicker,
                Vec::new(),
                Vec::new(),
                None,
                MAX_ENTRIES,
            ),
            panel,
            input: InputBuffer::new(),
            selected_row: None,
            row_hit_boxes: Vec::new(),
            composer_target: ComposerTarget::Companion,
            notice: None,
            panel_diagnostic: None,
            panel_diagnostic_open: false,
            sending: false,
            refreshing: true,
            enter_reload: Some(OrchestratorLifecycleMode::Ensure {
                runtime_command: runtime_command.clone(),
            }),
            runtime_command,
            last_companion_lifecycle_failure: None,
            last_orchestrator_lifecycle_failure: None,
            orchestrator_work_set: OrchestratorWorkSet::default(),
            orchestrator_queue_attention: None,
            #[cfg(feature = "e2e-test")]
            emitted_dashboard_content_ready: false,
        }
    }

    fn apply_reload_result(&mut self, result: Result<DashboardSnapshot, DashboardError>) {
        self.refreshing = false;
        match result {
            Ok(snapshot) => self.apply_reloaded_snapshot(snapshot),
            Err(error) => {
                self.notice = Some(format!("Refresh failed: {error}"));
            }
        }
    }

    #[cfg(test)]
    fn apply_reloaded_list(&mut self, mut list: PodList) {
        list.selected_name = self
            .list
            .selected_name
            .clone()
            .filter(|name| list.entries.iter().any(|entry| entry.name == *name))
            .or_else(|| list.entries.first().map(|entry| entry.name.clone()));
        let panel = build_workspace_panel(&current_workspace_root(), &list);
        self.apply_reloaded_snapshot(DashboardSnapshot { list, panel });
    }

    fn apply_reloaded_snapshot(&mut self, mut snapshot: DashboardSnapshot) {
        self.apply_companion_lifecycle_memory(&mut snapshot.panel);
        self.apply_orchestrator_lifecycle_memory(&mut snapshot.panel);
        let previous_selected_pod = self.list.selected_name.clone();
        snapshot.list.selected_name = previous_selected_pod
            .filter(|name| {
                snapshot
                    .list
                    .entries
                    .iter()
                    .any(|entry| entry.name == *name)
            })
            .or_else(|| {
                snapshot
                    .list
                    .entries
                    .first()
                    .map(|entry| entry.name.clone())
            });
        let previous_row = self.selected_row.clone();
        self.list = snapshot.list;
        self.panel = snapshot.panel;
        self.selected_row = previous_row.filter(|key| self.panel.row(key).is_some());
        self.ensure_selection_visible();
        self.ensure_composer_target_available();
        self.refresh_orchestrator_work_set();
        self.apply_orchestrator_work_set_detail();
    }

    fn prepare_orchestrator_queue_attention_notice(
        &mut self,
    ) -> Option<OrchestratorQueueAttentionNoticeRequest> {
        let target = orchestrator_queue_attention_notice_target(&self.panel, &self.list)?;
        if self.orchestrator_work_set.is_empty() {
            self.refresh_orchestrator_work_set();
        }
        let notice = orchestrator_queue_attention_notice(&self.panel, &self.orchestrator_work_set)?;
        if self
            .orchestrator_queue_attention
            .as_ref()
            .is_some_and(|freshness| freshness.fingerprint == notice.fingerprint)
        {
            self.apply_orchestrator_work_set_detail();
            return None;
        }
        Some(OrchestratorQueueAttentionNoticeRequest {
            pod_name: target.pod_name,
            socket_path: target.socket_path,
            notice,
        })
    }

    fn finish_orchestrator_queue_attention_notice(
        &mut self,
        result: OrchestratorQueueAttentionNoticeResult,
    ) {
        if let Some(error) = result.error {
            self.notice = Some(format!(
                "Orchestrator queued-work attention not delivered: {error}"
            ));
            return;
        }
        self.orchestrator_queue_attention = Some(OrchestratorQueueAttentionFreshness {
            fingerprint: result.fingerprint,
            updated_at: result.updated_at,
        });
        self.apply_orchestrator_work_set_detail();
    }

    fn refresh_orchestrator_work_set(&mut self) {
        let previous_planned = self.orchestrator_work_set.planned_queued_ids();
        self.orchestrator_work_set = derive_orchestrator_work_set(&self.panel, previous_planned);
    }

    fn apply_orchestrator_work_set_detail(&mut self) {
        let detail = orchestrator_work_set_detail(
            &self.orchestrator_work_set,
            self.orchestrator_queue_attention.as_ref(),
        );
        apply_orchestrator_detail(&mut self.panel, detail);
    }

    fn apply_companion_lifecycle_memory(&mut self, panel: &mut WorkspacePanelViewModel) {
        let Some(state) = panel.header.companion.as_ref() else {
            self.last_companion_lifecycle_failure = None;
            return;
        };

        match state.status {
            CompanionPanelStatus::Unavailable => {
                self.last_companion_lifecycle_failure =
                    companion_lifecycle_failure_from_panel(panel);
            }
            CompanionPanelStatus::Live
            | CompanionPanelStatus::Spawned
            | CompanionPanelStatus::Restored => {
                self.last_companion_lifecycle_failure = None;
            }
            CompanionPanelStatus::Missing | CompanionPanelStatus::Stopped => {
                if let Some(previous) = self.last_companion_lifecycle_failure.clone() {
                    if previous.pod_name == state.pod_name {
                        panel.header.companion = Some(previous.clone());
                        append_unique_diagnostic(panel, previous.detail.as_deref());
                    } else {
                        self.last_companion_lifecycle_failure = None;
                    }
                }
            }
        }
    }

    fn apply_orchestrator_lifecycle_memory(&mut self, panel: &mut WorkspacePanelViewModel) {
        let Some(state) = panel.header.orchestrator.as_ref() else {
            self.last_orchestrator_lifecycle_failure = None;
            return;
        };

        match state.status {
            OrchestratorPanelStatus::Unavailable => {
                self.last_orchestrator_lifecycle_failure =
                    orchestrator_lifecycle_failure_from_panel(panel);
            }
            OrchestratorPanelStatus::Live
            | OrchestratorPanelStatus::Spawned
            | OrchestratorPanelStatus::Restored => {
                self.last_orchestrator_lifecycle_failure = None;
            }
            OrchestratorPanelStatus::Missing | OrchestratorPanelStatus::Stopped => {
                if let Some(previous) = self.last_orchestrator_lifecycle_failure.clone() {
                    if previous.pod_name == state.pod_name {
                        panel.header.orchestrator = Some(previous.clone());
                        append_unique_diagnostic(panel, previous.detail.as_deref());
                    } else {
                        self.last_orchestrator_lifecycle_failure = None;
                    }
                }
            }
        }
    }

    fn selected_panel_row(&self) -> Option<&PanelRow> {
        self.selected_row
            .as_ref()
            .and_then(|key| self.panel.row(key))
    }

    fn selected_ticket_action(&self) -> Option<NextUserAction> {
        self.selected_panel_row()
            .filter(|row| row.is_ticket_action())
            .and_then(|row| row.next_action)
    }

    fn selected_pod_entry(&self) -> Option<&PodListEntry> {
        let name = self.selected_row.as_ref().and_then(PanelRowKey::pod_name)?;
        self.list.entries.iter().find(|entry| entry.name == name)
    }

    #[cfg(test)]
    pub(crate) fn selected_open_eligibility(&self) -> OpenEligibility {
        match self.selected_pod_entry() {
            Some(entry) if entry.actions.can_open => OpenEligibility::OpenNow,
            _ => OpenEligibility::Disabled,
        }
    }

    pub(crate) fn selected_open_disabled_reason(&self) -> Option<String> {
        if let Some(row) = self
            .selected_panel_row()
            .filter(|row| row.is_ticket_action())
        {
            return Some(
                row.disabled_reason
                    .clone()
                    .or_else(|| row.key_hint.clone())
                    .unwrap_or_else(|| {
                        "Enter dispatches this Ticket action after re-checking current Ticket authority."
                            .to_string()
                    }),
            );
        }
        if let Some(entry) = self.selected_pod_entry() {
            if entry.actions.can_open {
                return None;
            }
            return Some(open_disabled_reason(entry));
        }
        self.selected_panel_row()
            .and_then(|row| row.disabled_reason.clone().or_else(|| row.key_hint.clone()))
    }

    pub(crate) fn select_next(&mut self) {
        let visible = visible_panel_keys(&self.panel, &self.list);
        if visible.is_empty() {
            self.selected_row = None;
            self.list.selected_name = None;
            return;
        }
        let selected_pos = self
            .selected_row
            .as_ref()
            .and_then(|key| visible.iter().position(|visible_key| visible_key == key))
            .unwrap_or(0);
        let next_pos = (selected_pos + 1).min(visible.len() - 1);
        self.select_panel_key(visible[next_pos].clone());
    }

    pub(crate) fn select_prev(&mut self) {
        let visible = visible_panel_keys(&self.panel, &self.list);
        if visible.is_empty() {
            self.selected_row = None;
            self.list.selected_name = None;
            return;
        }
        let selected_pos = self
            .selected_row
            .as_ref()
            .and_then(|key| visible.iter().position(|visible_key| visible_key == key))
            .unwrap_or(0);
        self.select_panel_key(visible[selected_pos.saturating_sub(1)].clone());
    }

    fn handle_mouse_event(&mut self, event: MouseEvent) -> bool {
        if self.panel_diagnostic_open {
            return false;
        }
        match event.kind {
            MouseEventKind::ScrollDown => {
                #[cfg(feature = "e2e-test")]
                crate::e2e_observer::emit(
                    "panel",
                    "mouse_wheel",
                    serde_json::json!({
                        "column": event.column,
                        "row": event.row,
                        "direction": "down",
                    }),
                );
                self.select_next();
                return true;
            }
            MouseEventKind::ScrollUp => {
                #[cfg(feature = "e2e-test")]
                crate::e2e_observer::emit(
                    "panel",
                    "mouse_wheel",
                    serde_json::json!({
                        "column": event.column,
                        "row": event.row,
                        "direction": "up",
                    }),
                );
                self.select_prev();
                return true;
            }
            MouseEventKind::Down(MouseButton::Left) => {}
            _ => return false,
        }
        let Some(key) = self
            .row_hit_boxes
            .iter()
            .find(|hit| hit.contains(event.column, event.row))
            .map(|hit| hit.key.clone())
        else {
            return false;
        };
        #[cfg(feature = "e2e-test")]
        crate::e2e_observer::emit(
            "panel",
            "mouse_click",
            serde_json::json!({
                "column": event.column,
                "row": event.row,
                "target": panel_e2e_row_key(&key),
            }),
        );
        self.select_panel_key(key);
        true
    }

    fn set_row_hit_boxes(&mut self, rows: &[PanelListRow], area: Rect) {
        self.row_hit_boxes = row_hit_boxes(rows, area);
    }

    #[cfg(feature = "e2e-test")]
    fn emit_rows_rendered(&mut self) {
        let rows: Vec<_> = self
            .row_hit_boxes
            .iter()
            .map(|hit| {
                let panel_row = self.panel.row(&hit.key);
                let (
                    title,
                    status,
                    action,
                    disabled_reason,
                    local_state,
                    overlay_state,
                    overlay_detail,
                ) = match panel_row {
                    Some(row) => {
                        let ticket = row.ticket.as_ref();
                        (
                            row.title.clone(),
                            Some(row.status.clone()),
                            row.next_action.map(NextUserAction::label),
                            row.disabled_reason.clone(),
                            ticket.map(|ticket| ticket.workflow_state.as_str().to_string()),
                            ticket
                                .and_then(|ticket| ticket.orchestration_overlay.as_ref())
                                .map(|overlay| overlay.workflow_state.as_str().to_string()),
                            ticket
                                .and_then(|ticket| ticket.orchestration_overlay.as_ref())
                                .map(|overlay| {
                                    format!(
                                        "{}:{}",
                                        overlay.source,
                                        overlay.workflow_state.as_str()
                                    )
                                }),
                        )
                    }
                    None => match &hit.key {
                        PanelRowKey::Pod(name) => {
                            (name.clone(), None, None, None, None, None, None)
                        }
                        PanelRowKey::Ticket(id) | PanelRowKey::InvalidTicket(id) => {
                            (id.clone(), None, None, None, None, None, None)
                        }
                        PanelRowKey::TicketIntakePod { pod_name, .. } => {
                            (pod_name.clone(), None, None, None, None, None, None)
                        }
                    },
                };
                PanelE2eRenderedRow {
                    key: panel_e2e_row_key(&hit.key),
                    title,
                    status,
                    action,
                    disabled_reason,
                    local_state,
                    overlay_state,
                    overlay_detail,
                    rect: panel_e2e_rect(hit.rect),
                }
            })
            .collect();
        let selected = self.selected_row.as_ref().map(panel_e2e_row_key);
        let header = panel_e2e_dashboard_header(&self.panel);
        crate::e2e_observer::emit(
            "panel",
            "rows_rendered",
            PanelE2eRowsRendered {
                selected: selected.clone(),
                header: header.clone(),
                rows: rows.clone(),
            },
        );
        if !self.emitted_dashboard_content_ready {
            let categories = panel_e2e_dashboard_categories(&rows);
            let snapshot = PanelE2eDashboardSnapshot { header, rows };
            if panel_e2e_dashboard_content_is_ready(&snapshot, &categories) {
                crate::e2e_observer::emit(
                    "panel",
                    "dashboard_content_ready",
                    PanelE2eDashboardContentReady {
                        snapshot,
                        categories,
                    },
                );
                self.emitted_dashboard_content_ready = true;
            }
        }
    }

    fn ensure_selection_visible(&mut self) {
        let visible = visible_panel_keys(&self.panel, &self.list);
        if visible.is_empty() {
            self.selected_row = None;
            self.list.selected_name = None;
            return;
        }
        let selected_visible = self
            .selected_row
            .as_ref()
            .is_some_and(|key| visible.iter().any(|visible_key| visible_key == key));
        if !selected_visible {
            let has_action_rows = self.panel.rows.iter().any(|row| row.is_ticket_action());
            let orchestrator_pod_name = self
                .panel
                .header
                .orchestrator
                .as_ref()
                .map(|state| state.pod_name.as_str());
            if !has_action_rows {
                if let Some(selected_name) = self.list.selected_name.as_ref() {
                    if Some(selected_name.as_str()) != orchestrator_pod_name {
                        let key = PanelRowKey::Pod(selected_name.clone());
                        if visible.iter().any(|visible_key| visible_key == &key) {
                            self.select_panel_key(key);
                            return;
                        }
                    }
                }
                if let Some(key) = visible.iter().find(|key| match key {
                    PanelRowKey::Pod(name) => Some(name.as_str()) != orchestrator_pod_name,
                    PanelRowKey::Ticket(_)
                    | PanelRowKey::InvalidTicket(_)
                    | PanelRowKey::TicketIntakePod { .. } => true,
                }) {
                    self.select_panel_key(key.clone());
                    return;
                }
                self.selected_row = None;
                self.list.selected_name = None;
                return;
            }
            self.select_panel_key(visible[0].clone());
        } else if let Some(PanelRowKey::Pod(name)) = self.selected_row.as_ref() {
            self.list.selected_name = Some(name.clone());
        }
    }

    fn select_panel_key(&mut self, key: PanelRowKey) {
        if let PanelRowKey::Pod(name) = &key {
            self.list.selected_name = Some(name.clone());
        }
        #[cfg(feature = "e2e-test")]
        let selected_key = key.clone();
        self.selected_row = Some(key);
        #[cfg(feature = "e2e-test")]
        crate::e2e_observer::emit(
            "panel",
            "selection_changed",
            serde_json::json!({ "selected": panel_e2e_row_key(&selected_key) }),
        );
    }

    fn clear_panel_selection(&mut self) {
        self.selected_row = None;
        self.list.selected_name = None;
        #[cfg(feature = "e2e-test")]
        crate::e2e_observer::emit(
            "panel",
            "selection_changed",
            serde_json::json!({ "selected": serde_json::Value::Null }),
        );
    }

    fn ensure_composer_target_available(&mut self) {
        if !self.panel.composer.is_available(self.composer_target) {
            self.composer_target = ComposerTarget::Companion;
        }
    }

    pub(crate) fn cycle_composer_target(&mut self) {
        let targets = &self.panel.composer.available_targets;
        if targets.len() <= 1 {
            self.composer_target = ComposerTarget::Companion;
            self.notice = Some(
                "Ticket Intake target is unavailable without usable Ticket config.".to_string(),
            );
            return;
        }
        let current = targets
            .iter()
            .position(|target| *target == self.composer_target)
            .unwrap_or(0);
        let next = targets[(current + 1) % targets.len()];
        self.composer_target = next;
        self.notice = Some(format!("Composer target: {}", next.label()));
    }

    pub(crate) fn composer_target(&self) -> ComposerTarget {
        self.composer_target
    }

    pub(crate) fn prepare_open(&mut self) -> Option<OpenPodRequest> {
        let (pod_name, socket_override, progress) = {
            let entry = match self.selected_pod_entry() {
                Some(entry) => entry,
                None => {
                    self.notice = Some(selected_ticket_notice(self.selected_panel_row()));
                    return None;
                }
            };
            if !entry.actions.can_open {
                self.notice = Some("Selected Pod cannot be opened from this view.".to_string());
                return None;
            }
            let progress = if entry.live.as_ref().is_some_and(|live| live.reachable) {
                "Attaching to"
            } else if entry.stored.is_some() {
                "Restoring/opening"
            } else {
                "Opening"
            };
            (
                entry.name.clone(),
                entry.attach_socket_path().map(PathBuf::from),
                progress,
            )
        };
        self.notice = Some(format!("{progress} {pod_name}…"));
        Some(OpenPodRequest {
            pod_name,
            socket_override,
        })
    }

    pub(crate) fn finish_open(
        &mut self,
        pod_name: &str,
        result: Result<(), &dyn std::fmt::Display>,
    ) {
        match result {
            Ok(()) => {
                self.notice = Some(format!("Returned from {pod_name}. Refreshing workspace…"));
            }
            Err(error) => {
                self.notice = Some(format!(
                    "Open failed for {pod_name}: {error}. Refreshing workspace…"
                ));
            }
        }
        self.refreshing = true;
        self.enter_reload = Some(OrchestratorLifecycleMode::Observe);
    }

    fn composer_is_blank(&self) -> bool {
        segments_are_blank(&self.input.submit_segments())
    }

    pub(crate) fn prepare_companion_send(&mut self) -> Option<CompanionSendRequest> {
        let segments = self.input.submit_segments();
        if segments_are_blank(&segments) {
            self.notice = Some("Composer is empty.".to_string());
            return None;
        }
        let Some(companion) = self.panel.header.companion.as_ref() else {
            self.notice = Some("Workspace Companion is unavailable; draft kept.".to_string());
            return None;
        };
        if matches!(
            companion.status,
            CompanionPanelStatus::Unavailable
                | CompanionPanelStatus::Missing
                | CompanionPanelStatus::Stopped
        ) {
            let detail = companion
                .detail
                .as_deref()
                .unwrap_or("workspace Companion is not live yet");
            self.notice = Some(bounded_panel_diagnostic(format!(
                "Companion {} is {}: {detail}; draft kept.",
                companion.pod_name,
                companion.status.label()
            )));
            return None;
        }
        let Some(entry) = self
            .list
            .entries
            .iter()
            .find(|entry| entry.name == companion.pod_name)
        else {
            self.notice = Some(format!(
                "Companion {} is not in the current Pod list; refresh and retry. Draft kept.",
                companion.pod_name
            ));
            return None;
        };
        let Some(live) = entry.live.as_ref().filter(|live| live.reachable) else {
            self.notice = Some(format!(
                "Companion {} is not reachable; refresh and retry. Draft kept.",
                companion.pod_name
            ));
            return None;
        };
        if live.status == Some(PodStatus::Running) {
            self.notice = Some(format!(
                "Companion {} is busy; wait for it to become idle or open it for inspection. Draft kept.",
                companion.pod_name
            ));
            return None;
        }
        self.sending = true;
        self.notice = Some(format!("Sending to Companion {}…", companion.pod_name));
        Some(CompanionSendRequest {
            pod_name: companion.pod_name.clone(),
            socket_path: live.socket_path.clone(),
            segments,
        })
    }

    pub(crate) fn finish_companion_send(
        &mut self,
        result: Result<CompanionSendOutcome, CompanionSendError>,
    ) {
        self.sending = false;
        match result {
            Ok(outcome) => {
                self.input.clear();
                self.notice = Some(outcome.notice);
            }
            Err(error) => {
                self.notice = Some(bounded_panel_diagnostic(error.to_string()));
            }
        }
    }

    pub(crate) fn prepare_ticket_action_dispatch(&mut self) -> Option<TicketActionRequest> {
        let row = match self.selected_panel_row() {
            Some(row) if row.is_ticket_action() => row,
            Some(row) if row.ticket.is_some() => {
                self.notice = Some("Selected Ticket row has no inline action.".to_string());
                return None;
            }
            _ => {
                self.notice = Some("No Ticket action is selected.".to_string());
                return None;
            }
        };
        let Some(action) = row.next_action else {
            self.notice = Some("Selected Ticket row has no inline action.".to_string());
            return None;
        };
        let ticket_id = {
            let Some(ticket) = row.ticket.as_ref() else {
                self.notice = Some("No Ticket action is selected.".to_string());
                return None;
            };
            ticket.id.clone()
        };
        let orchestrator = ticket_action_orchestrator_target(&self.panel, &self.list);
        self.sending = true;
        self.notice = Some(format!(
            "Dispatching {} for Ticket {}…",
            action.label(),
            ticket_id
        ));
        Some(TicketActionRequest {
            workspace_root: current_workspace_root(),
            ticket_id,
            action,
            orchestrator,
        })
    }

    fn set_panel_diagnostic(
        &mut self,
        title: impl Into<String>,
        details: impl Into<String>,
    ) -> String {
        let title = title.into();
        let details = details.into();
        self.panel_diagnostic = Some(PanelDiagnostic {
            title: title.clone(),
            details: details.clone(),
        });
        self.panel_diagnostic_open = false;
        format!("{} (F2 details)", bounded_panel_diagnostic(&details))
    }

    pub(crate) fn finish_ticket_action_dispatch(
        &mut self,
        result: Result<TicketActionOutcome, TicketActionError>,
    ) {
        self.sending = false;
        self.notice = Some(match result {
            Ok(outcome) => outcome.notice,
            Err(error) => match error {
                TicketActionError::Stale(message) => {
                    self.set_panel_diagnostic("Ticket action rejected", message)
                }
                TicketActionError::BackendConfig(error) | TicketActionError::Ticket(error) => {
                    self.set_panel_diagnostic("Ticket action failed", error)
                }
            },
        });
    }

    pub(crate) fn prepare_intake_launch(&mut self) -> Option<IntakeLaunchRequest> {
        if !self
            .panel
            .composer
            .is_available(ComposerTarget::TicketIntake)
        {
            self.composer_target = ComposerTarget::Companion;
            self.notice = Some(
                "Ticket Intake target is unavailable without usable Ticket config.".to_string(),
            );
            return None;
        }
        let body = Segment::flatten_to_text(&self.input.submit_segments());
        if body.trim().is_empty() {
            self.notice = Some("Ticket Intake input is empty; type a request first.".to_string());
            return None;
        }
        let mut context =
            TicketRoleLaunchContext::new(current_workspace_root(), TicketRole::Intake);
        let pod_name = unique_preticket_intake_pod_name();
        context.pod_name = Some(pod_name.clone());
        context.user_instruction = Some(body);
        let store = match PanelRegistryStore::default_for_workspace(&context.workspace_root) {
            Ok(store) => store,
            Err(error) => {
                self.notice = Some(format!("Ticket Intake registry unavailable: {error}"));
                return None;
            }
        };
        let peer_registration = self.prepare_intake_peer_registration(&mut context);
        self.sending = true;
        self.notice = Some("Launching Ticket Intake…".to_string());
        Some(IntakeLaunchRequest {
            context,
            runtime_command: self.runtime_command.clone(),
            peer_registration,
            registry_update: IntakeRegistryUpdate::RecordSession {
                registry_root: store.root().to_path_buf(),
                pod_name,
                origin: RoleSessionOrigin::PreTicketIntake,
                related_tickets: Vec::new(),
            },
        })
    }

    fn prepare_intake_peer_registration(
        &self,
        context: &mut TicketRoleLaunchContext,
    ) -> IntakePeerRegistrationRequest {
        match self.panel.header.orchestrator.as_ref() {
            Some(orchestrator) => {
                context.intake_handoff = Some(TicketIntakeHandoff::new(
                    orchestrator.pod_name.clone(),
                    self.panel.header.workspace_label.clone(),
                ));
                if orchestrator_status_is_peer_reachable(orchestrator.status) {
                    IntakePeerRegistrationRequest::Register {
                        orchestrator_pod: orchestrator.pod_name.clone(),
                    }
                } else {
                    IntakePeerRegistrationRequest::Skip {
                        reason: format!(
                            "workspace Orchestrator {} is {}; launch input still carries the auditable handoff target",
                            orchestrator.pod_name,
                            orchestrator.status.label()
                        ),
                    }
                }
            }
            None => IntakePeerRegistrationRequest::Skip {
                reason: "workspace Orchestrator is not configured for this panel".to_string(),
            },
        }
    }

    pub(crate) fn prepare_existing_ticket_intake_launch(&mut self) -> Option<IntakeLaunchRequest> {
        if self.sending {
            self.notice = Some(
                "Ticket Intake launch is already in progress; wait for it to finish before retrying."
                    .to_string(),
            );
            return None;
        }
        let row = match self.selected_panel_row() {
            Some(row) if row.is_ticket_action() => row,
            Some(row) if row.ticket.is_some() => {
                self.notice = Some("Selected Ticket row has no Intake action.".to_string());
                return None;
            }
            _ => {
                self.notice = Some("No Ticket Intake action is selected.".to_string());
                return None;
            }
        };
        let Some(action) = row.next_action else {
            self.notice = Some("Selected Ticket row has no Intake action.".to_string());
            return None;
        };
        if action != NextUserAction::Clarify {
            self.notice = Some(format!(
                "{} is not handled by Ticket Intake launch.",
                action.label()
            ));
            return None;
        }
        let Some(ticket) = row.ticket.as_ref() else {
            self.notice = Some("No Ticket Intake action is selected.".to_string());
            return None;
        };
        let ticket_id = ticket.id.clone();
        let mut context =
            TicketRoleLaunchContext::new(current_workspace_root(), TicketRole::Intake);
        context.ticket = Some(TicketRef::id(ticket_id.clone()));
        context.user_instruction = Some(format!(
            "Continue Intake for existing Ticket {ticket_id}. Do not create a duplicate Ticket unless the user explicitly requests one. Read TicketShow body/thread/artifacts before making routing or requirements decisions."
        ));
        let store = match PanelRegistryStore::default_for_workspace(&context.workspace_root) {
            Ok(store) => store,
            Err(error) => {
                self.notice = Some(format!("Ticket Intake registry unavailable: {error}"));
                return None;
            }
        };
        match store.claim_for_ticket(&ticket_id) {
            Ok(Some(claim)) => {
                let status = local_claim_status_for_pod(&claim.pod_name, &self.list);
                self.notice = Some(existing_ticket_claim_notice(
                    &ticket_id,
                    &claim.pod_name,
                    status,
                ));
                return None;
            }
            Ok(None) => {}
            Err(error) => {
                self.notice = Some(format!("Ticket claim diagnostic required: {error}"));
                return None;
            }
        }
        let planned = match plan_ticket_role_launch(context.clone()) {
            Ok(plan) => plan,
            Err(error) => {
                self.notice = Some(format!(
                    "Ticket Intake launch plan failed; no claim written: {}",
                    bounded_panel_diagnostic(error.to_string())
                ));
                return None;
            }
        };
        context.pod_name = Some(planned.pod_name.clone());
        let pod_name = planned.pod_name.clone();
        let peer_registration = self.prepare_intake_peer_registration(&mut context);
        self.sending = true;
        self.notice = Some(format!(
            "Launching Ticket Intake for {} as {}…",
            ticket_id, planned.pod_name
        ));
        Some(IntakeLaunchRequest {
            context,
            runtime_command: self.runtime_command.clone(),
            peer_registration,
            registry_update: IntakeRegistryUpdate::ClaimTicket {
                registry_root: store.root().to_path_buf(),
                ticket_id,
                ticket_slug: None,
                pod_name,
            },
        })
    }

    pub(crate) fn prepare_ready_ticket_planning_return(
        &mut self,
    ) -> Option<ReadyTicketPlanningReturnRequest> {
        if self.sending {
            self.notice = Some(
                "Ticket refinement return is already in progress; wait for it to finish before retrying."
                    .to_string(),
            );
            return None;
        }
        if !self
            .panel
            .composer
            .is_available(ComposerTarget::TicketIntake)
        {
            self.composer_target = ComposerTarget::Companion;
            self.notice = Some(
                "Ticket Intake target is unavailable without usable Ticket config.".to_string(),
            );
            return None;
        }
        let body = Segment::flatten_to_text(&self.input.submit_segments());
        let user_instruction = bounded_refinement_instruction(body.trim());
        if user_instruction.is_empty() {
            self.notice = Some(
                "Type refinement instructions with the Ticket Planning target before returning a ready Ticket to planning."
                    .to_string(),
            );
            return None;
        }
        let row = match self.selected_panel_row() {
            Some(row) if row.is_ticket_action() => row,
            Some(row) if row.ticket.is_some() => {
                self.notice =
                    Some("Selected Ticket row has no refinement return action.".to_string());
                return None;
            }
            _ => {
                self.notice =
                    Some("Select a ready Ticket row before returning it to planning.".to_string());
                return None;
            }
        };
        if row.next_action != Some(NextUserAction::Queue) {
            self.notice = Some(
                "Only ready Ticket rows can be returned to planning from the Ticket Planning target."
                    .to_string(),
            );
            return None;
        }
        let Some(ticket) = row.ticket.as_ref() else {
            self.notice =
                Some("Select a ready Ticket row before returning it to planning.".to_string());
            return None;
        };
        let ticket_id = ticket.id.clone();
        if ticket.workflow_state != TicketWorkflowState::Ready {
            self.notice = Some(format!(
                "Ticket {} is {}; expected ready before returning to planning.",
                ticket_id,
                ticket.workflow_state.as_str()
            ));
            return None;
        }

        let workspace_root = current_workspace_root();
        let store = match PanelRegistryStore::default_for_workspace(&workspace_root) {
            Ok(store) => store,
            Err(error) => {
                self.notice = Some(format!("Ticket Intake registry unavailable: {error}"));
                return None;
            }
        };
        let followup = match store.claim_for_ticket(&ticket_id) {
            Ok(Some(claim)) => match local_claim_status_for_pod(&claim.pod_name, &self.list) {
                TicketLocalClaimStatus::Live => match self
                    .list
                    .entries
                    .iter()
                    .find(|entry| entry.name == claim.pod_name)
                    .and_then(PodListEntry::attach_socket_path)
                {
                    Some(socket_path) => {
                        ReadyTicketPlanningReturnFollowup::NotifyLiveClaimedIntake {
                            pod_name: claim.pod_name,
                            socket_path: socket_path.to_path_buf(),
                        }
                    }
                    None => ReadyTicketPlanningReturnFollowup::BlockedByStaleClaim {
                        pod_name: claim.pod_name,
                    },
                },
                TicketLocalClaimStatus::Restorable => {
                    ReadyTicketPlanningReturnFollowup::OpenRestorableClaimedIntake(OpenPodRequest {
                        pod_name: claim.pod_name,
                        socket_override: None,
                    })
                }
                TicketLocalClaimStatus::Stale => {
                    ReadyTicketPlanningReturnFollowup::BlockedByStaleClaim {
                        pod_name: claim.pod_name,
                    }
                }
            },
            Ok(None) => {
                let mut context =
                    TicketRoleLaunchContext::new(workspace_root.clone(), TicketRole::Intake);
                context.ticket = Some(TicketRef::id(ticket_id.clone()));
                context.user_instruction = Some(build_ready_ticket_refinement_launch_instruction(
                    &ticket_id,
                    &user_instruction,
                ));
                let peer_registration = self.prepare_intake_peer_registration(&mut context);
                ReadyTicketPlanningReturnFollowup::LaunchIntake(IntakeLaunchRequest {
                    context,
                    runtime_command: self.runtime_command.clone(),
                    peer_registration,
                    registry_update: IntakeRegistryUpdate::ClaimLaunchedTicket {
                        registry_root: store.root().to_path_buf(),
                        ticket_id: ticket_id.clone(),
                        ticket_slug: None,
                    },
                })
            }
            Err(error) => {
                self.notice = Some(format!("Ticket claim diagnostic required: {error}"));
                return None;
            }
        };

        self.sending = true;
        self.notice = Some(format!(
            "Returning ready Ticket {} to planning for refinement…",
            ticket_id
        ));
        Some(ReadyTicketPlanningReturnRequest {
            workspace_root,
            ticket_id,
            user_instruction,
            followup,
        })
    }

    fn finish_ready_ticket_planning_return_error(&mut self, error: TicketActionError) {
        self.sending = false;
        self.notice = Some(match error {
            TicketActionError::Stale(message) => {
                self.set_panel_diagnostic("Ticket planning return rejected", message)
            }
            TicketActionError::BackendConfig(error) | TicketActionError::Ticket(error) => {
                self.set_panel_diagnostic("Ticket planning return failed", error)
            }
        });
    }

    fn finish_ready_ticket_planning_return_success(
        &mut self,
        outcome: ReadyTicketPlanningReturnOutcome,
    ) -> ReadyTicketPlanningReturnAfterMutation {
        self.sending = false;
        self.input.clear();
        self.notice = Some(outcome.notice);
        outcome.followup
    }

    fn finish_ready_ticket_planning_return_with_intake_launch(
        &mut self,
        planning_notice: String,
        result: IntakeLaunchResult,
    ) {
        self.sending = false;
        self.input.clear();
        match result {
            Ok(result) => {
                let pod_name = result.launch.plan.pod_name;
                let peer_notice = match result.peer_registration {
                    IntakePeerRegistrationStatus::Registered { orchestrator_pod } => {
                        format!(" Handoff peer registered with {orchestrator_pod}.")
                    }
                    IntakePeerRegistrationStatus::Warning { message } => {
                        format!(" Handoff warning: {message}")
                    }
                };
                let registry_notice = result
                    .registry_warning
                    .map(|warning| format!(" Registry warning: {warning}"))
                    .unwrap_or_default();
                self.notice = Some(bounded_panel_diagnostic(format!(
                    "{planning_notice} Launched Ticket Intake Pod {pod_name}.{peer_notice}{registry_notice}"
                )));
            }
            Err(error) => {
                self.notice = Some(self.set_panel_diagnostic(
                    "Ticket Intake launch failed after planning return",
                    format!(
                        "{planning_notice} Intake launch/restore failed after Ticket was returned to planning; instruction was recorded in the Ticket thread. {}",
                        error
                    ),
                ));
            }
        }
    }

    pub(crate) fn finish_intake_launch(&mut self, result: IntakeLaunchResult) {
        self.sending = false;
        match result {
            Ok(result) => {
                let pod_name = result.launch.plan.pod_name;
                self.input.clear();
                let peer_notice = match result.peer_registration {
                    IntakePeerRegistrationStatus::Registered { orchestrator_pod } => {
                        format!(" Handoff peer registered with {orchestrator_pod}.")
                    }
                    IntakePeerRegistrationStatus::Warning { message } => {
                        format!(" Handoff warning: {message}")
                    }
                };
                let registry_notice = result
                    .registry_warning
                    .map(|warning| format!(" Registry warning: {warning}"))
                    .unwrap_or_default();
                self.notice = Some(bounded_panel_diagnostic(format!(
                    "Launched Ticket Intake Pod {pod_name}.{peer_notice}{registry_notice}"
                )));
            }
            Err(error) => {
                self.notice = Some(format!(
                    "Intake launch failed; composer kept: {}",
                    bounded_panel_diagnostic(error.to_string())
                ));
            }
        }
    }

    fn apply_composer_edit_action(&mut self, action: ComposerEditAction) {
        match action {
            ComposerEditAction::InsertChar(c) => self.input.insert_char(c),
            ComposerEditAction::InsertNewline => self.input.insert_newline(),
            ComposerEditAction::DeleteBefore => self.input.delete_before(),
            ComposerEditAction::DeleteAfter => self.input.delete_after(),
            ComposerEditAction::DeleteWordBefore => self.input.delete_word_before(),
            ComposerEditAction::MoveLeft => self.input.move_left(),
            ComposerEditAction::MoveRight => self.input.move_right(),
            ComposerEditAction::MoveWordLeft => self.input.move_word_left(),
            ComposerEditAction::MoveWordRight => self.input.move_word_right(),
            ComposerEditAction::MoveStart => self.input.move_start(),
            ComposerEditAction::MoveHome => self.input.move_home(),
            ComposerEditAction::MoveEnd => self.input.move_end(),
            ComposerEditAction::MoveUp => self.input.move_up(),
            ComposerEditAction::MoveDown => self.input.move_down(),
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> DashboardAction {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if self.panel_diagnostic_open {
            return match key.code {
                KeyCode::Esc | KeyCode::F(2) => {
                    self.panel_diagnostic_open = false;
                    DashboardAction::None
                }
                KeyCode::Char('d') if ctrl => DashboardAction::Quit,
                KeyCode::Char('c') if ctrl => DashboardAction::Quit,
                _ => DashboardAction::None,
            };
        }

        let composer_action = composer_edit_action(key);
        if let Some(action) = composer_action {
            if action.is_modifier_action() {
                self.apply_composer_edit_action(action);
                return DashboardAction::None;
            }
        }

        match key.code {
            KeyCode::F(2) => {
                if self.panel_diagnostic.is_some() {
                    self.panel_diagnostic_open = true;
                } else {
                    self.notice = Some("No Panel diagnostic details yet".to_string());
                }
                DashboardAction::None
            }
            KeyCode::Char('d') if ctrl => DashboardAction::Quit,
            KeyCode::Char('c') if ctrl => DashboardAction::Quit,
            KeyCode::Esc => {
                self.clear_panel_selection();
                self.notice = Some(
                    "Row selection cleared; composer draft and target are unchanged.".to_string(),
                );
                DashboardAction::None
            }
            KeyCode::Tab => {
                // Completion owns Tab before panel target switching when a
                // completion popup exists. The workspace Dashboard currently has
                // no completion source, so this is the target switch path.
                self.cycle_composer_target();
                DashboardAction::None
            }
            KeyCode::Up if self.composer_is_blank() => {
                self.select_prev();
                DashboardAction::None
            }
            KeyCode::Down if self.composer_is_blank() => {
                self.select_next();
                DashboardAction::None
            }
            KeyCode::Enter
                if self.composer_is_blank()
                    && self.selected_ticket_action() == Some(NextUserAction::Clarify) =>
            {
                self.prepare_existing_ticket_intake_launch()
                    .map(DashboardAction::LaunchIntake)
                    .unwrap_or(DashboardAction::None)
            }
            KeyCode::Enter
                if self.composer_is_blank() && self.selected_ticket_action().is_some() =>
            {
                self.prepare_ticket_action_dispatch()
                    .map(DashboardAction::DispatchTicketAction)
                    .unwrap_or(DashboardAction::None)
            }
            KeyCode::Enter if self.composer_is_blank() => DashboardAction::Open,
            KeyCode::Enter
                if self.composer_target == ComposerTarget::TicketIntake
                    && self.selected_ticket_action() == Some(NextUserAction::Queue) =>
            {
                self.prepare_ready_ticket_planning_return()
                    .map(DashboardAction::ReturnReadyTicketToPlanning)
                    .unwrap_or(DashboardAction::None)
            }
            KeyCode::Enter if self.composer_target == ComposerTarget::TicketIntake => self
                .prepare_intake_launch()
                .map(DashboardAction::LaunchIntake)
                .unwrap_or(DashboardAction::None),
            KeyCode::Enter => self
                .prepare_companion_send()
                .map(DashboardAction::SendCompanion)
                .unwrap_or(DashboardAction::None),
            _ => {
                if let Some(action) = composer_action {
                    self.apply_composer_edit_action(action);
                }
                DashboardAction::None
            }
        }
    }
}

enum DashboardAction {
    None,
    Quit,
    Open,
    DispatchTicketAction(TicketActionRequest),
    ReturnReadyTicketToPlanning(ReadyTicketPlanningReturnRequest),
    LaunchIntake(IntakeLaunchRequest),
    SendCompanion(CompanionSendRequest),
}

#[derive(Debug, Clone)]
struct DashboardSnapshot {
    list: PodList,
    panel: WorkspacePanelViewModel,
}

fn companion_lifecycle_failure_from_panel(
    panel: &WorkspacePanelViewModel,
) -> Option<CompanionPanelState> {
    let state = panel.header.companion.as_ref()?;
    if state.status == CompanionPanelStatus::Unavailable && state.detail.is_some() {
        Some(state.clone())
    } else {
        None
    }
}

fn orchestrator_lifecycle_failure_from_panel(
    panel: &WorkspacePanelViewModel,
) -> Option<OrchestratorPanelState> {
    let state = panel.header.orchestrator.as_ref()?;
    if state.status == OrchestratorPanelStatus::Unavailable && state.detail.is_some() {
        Some(state.clone())
    } else {
        None
    }
}

fn append_unique_diagnostic(panel: &mut WorkspacePanelViewModel, diagnostic: Option<&str>) {
    let Some(diagnostic) = diagnostic else {
        return;
    };
    if !panel
        .header
        .diagnostics
        .iter()
        .any(|existing| existing == diagnostic)
    {
        panel.header.diagnostics.push(diagnostic.to_string());
    }
}

#[derive(Debug, Clone)]
enum OrchestratorLifecycleMode {
    Ensure { runtime_command: PodRuntimeCommand },
    Observe,
}

async fn load_dashboard_snapshot(
    selected_name: Option<String>,
    lifecycle_mode: OrchestratorLifecycleMode,
) -> Result<DashboardSnapshot, DashboardError> {
    let workspace_root = current_workspace_root();
    #[cfg(feature = "e2e-test")]
    let load_started = Instant::now();
    #[cfg(feature = "e2e-test")]
    let mut source_timings = Vec::new();
    let companion_pod_name = workspace_companion_pod_name(&workspace_root);
    let list_selected_name = selected_name
        .clone()
        .or_else(|| Some(companion_pod_name.clone()));

    #[cfg(feature = "e2e-test")]
    let source_started = Instant::now();
    let mut list = load_pod_list(list_selected_name.clone(), MAX_ENTRIES).await?;
    #[cfg(feature = "e2e-test")]
    source_timings.push(PanelE2eSourceTiming {
        source: "pod_metadata_status_probe.initial",
        elapsed_ms: source_started.elapsed().as_millis(),
    });

    #[cfg(feature = "e2e-test")]
    let source_started = Instant::now();
    let companion_presence = companion_pod_presence(&companion_pod_name, &list);
    #[cfg(feature = "e2e-test")]
    source_timings.push(PanelE2eSourceTiming {
        source: "companion.presence.from_initial_list",
        elapsed_ms: source_started.elapsed().as_millis(),
    });

    #[cfg(feature = "e2e-test")]
    let source_started = Instant::now();
    let companion = match lifecycle_mode.clone() {
        OrchestratorLifecycleMode::Ensure { runtime_command } => {
            ensure_workspace_companion(
                &workspace_root,
                companion_pod_name,
                companion_presence,
                runtime_command,
            )
            .await
        }
        OrchestratorLifecycleMode::Observe => {
            observe_workspace_companion(companion_pod_name, companion_presence)
        }
    };
    #[cfg(feature = "e2e-test")]
    source_timings.push(PanelE2eSourceTiming {
        source: "companion.lifecycle",
        elapsed_ms: source_started.elapsed().as_millis(),
    });
    if companion.reload_pods {
        #[cfg(feature = "e2e-test")]
        let source_started = Instant::now();
        list = load_pod_list(list_selected_name.clone(), MAX_ENTRIES).await?;
        #[cfg(feature = "e2e-test")]
        source_timings.push(PanelE2eSourceTiming {
            source: "pod_metadata_status_probe.after_companion_reload",
            elapsed_ms: source_started.elapsed().as_millis(),
        });
    }

    #[cfg(feature = "e2e-test")]
    let source_started = Instant::now();
    let config = ticket_config_availability(&workspace_root);
    #[cfg(feature = "e2e-test")]
    source_timings.push(PanelE2eSourceTiming {
        source: "ticket_config_probe",
        elapsed_ms: source_started.elapsed().as_millis(),
    });

    let orchestrator_pod_name = workspace_orchestrator_pod_name(&workspace_root);
    #[cfg(feature = "e2e-test")]
    let source_started = Instant::now();
    let orchestrator_presence = match &config {
        TicketConfigAvailability::Absent | TicketConfigAvailability::Unusable(_) => None,
        TicketConfigAvailability::Usable => {
            Some(orchestrator_pod_presence(&orchestrator_pod_name, &list))
        }
    };
    #[cfg(feature = "e2e-test")]
    source_timings.push(PanelE2eSourceTiming {
        source: "orchestrator.presence.from_initial_list",
        elapsed_ms: source_started.elapsed().as_millis(),
    });

    #[cfg(feature = "e2e-test")]
    let source_started = Instant::now();
    let orchestrator = match lifecycle_mode {
        OrchestratorLifecycleMode::Ensure { runtime_command } => {
            ensure_workspace_orchestrator(
                &workspace_root,
                config,
                orchestrator_pod_name,
                orchestrator_presence,
                runtime_command,
            )
            .await
        }
        OrchestratorLifecycleMode::Observe => {
            observe_workspace_orchestrator(config, orchestrator_pod_name, orchestrator_presence)
        }
    };
    #[cfg(feature = "e2e-test")]
    source_timings.push(PanelE2eSourceTiming {
        source: "orchestrator.lifecycle",
        elapsed_ms: source_started.elapsed().as_millis(),
    });
    if orchestrator.reload_pods {
        #[cfg(feature = "e2e-test")]
        let source_started = Instant::now();
        list = load_pod_list(list_selected_name, MAX_ENTRIES).await?;
        #[cfg(feature = "e2e-test")]
        source_timings.push(PanelE2eSourceTiming {
            source: "pod_metadata_status_probe.after_orchestrator_reload",
            elapsed_ms: source_started.elapsed().as_millis(),
        });
    }

    #[cfg(feature = "e2e-test")]
    let source_started = Instant::now();
    #[cfg(feature = "e2e-test")]
    let (mut panel, panel_source_timings) =
        build_workspace_panel_with_e2e_timings(&workspace_root, &list);
    #[cfg(not(feature = "e2e-test"))]
    let mut panel = build_workspace_panel(&workspace_root, &list);
    panel.header.companion = companion.state;
    panel.header.diagnostics.extend(companion.diagnostics);
    panel.header.orchestrator = orchestrator.state;
    panel.header.diagnostics.extend(orchestrator.diagnostics);
    #[cfg(feature = "e2e-test")]
    {
        source_timings.push(PanelE2eSourceTiming {
            source: "workspace_panel.build.total",
            elapsed_ms: source_started.elapsed().as_millis(),
        });
        source_timings.extend(panel_source_timings.into_iter().map(|timing| {
            PanelE2eSourceTiming {
                source: timing.source,
                elapsed_ms: timing.elapsed_ms,
            }
        }));
    }

    #[cfg(feature = "e2e-test")]
    crate::e2e_observer::emit(
        "panel",
        "dashboard_source_breakdown",
        PanelE2eDashboardSourceBreakdown {
            total_elapsed_ms: load_started.elapsed().as_millis(),
            sources: source_timings,
            ticket_rows: panel
                .rows
                .iter()
                .filter(|row| row.is_ticket_action())
                .count(),
            pod_rows: list.entries.len(),
            diagnostics: panel.header.diagnostics.len(),
        },
    );
    Ok(DashboardSnapshot { list, panel })
}

#[derive(Debug, Clone)]
struct CompanionLifecycleReport {
    state: Option<CompanionPanelState>,
    diagnostics: Vec<String>,
    reload_pods: bool,
}

impl CompanionLifecycleReport {
    fn with_state(state: CompanionPanelState) -> Self {
        Self {
            state: Some(state),
            diagnostics: Vec::new(),
            reload_pods: false,
        }
    }

    fn unavailable(pod_name: String, detail: String) -> Self {
        let detail = bounded_panel_diagnostic(detail);
        Self {
            state: Some(CompanionPanelState::new(
                pod_name,
                CompanionPanelStatus::Unavailable,
                Some(detail.clone()),
            )),
            diagnostics: vec![detail],
            reload_pods: false,
        }
    }

    fn mark_reload(mut self) -> Self {
        self.reload_pods = true;
        self
    }
}

async fn ensure_workspace_companion(
    workspace_root: &Path,
    pod_name: String,
    presence: CompanionPodPresence,
    runtime_command: PodRuntimeCommand,
) -> CompanionLifecycleReport {
    match decide_companion_lifecycle(&presence) {
        CompanionLifecyclePlan::ReportLive => CompanionLifecycleReport::with_state(
            CompanionPanelState::new(pod_name, CompanionPanelStatus::Live, None),
        ),
        CompanionLifecyclePlan::Restore => {
            match restore_workspace_companion_pod(
                workspace_root,
                &pod_name,
                runtime_command.clone(),
            )
            .await
            {
                Ok(()) => CompanionLifecycleReport::with_state(CompanionPanelState::new(
                    pod_name,
                    CompanionPanelStatus::Restored,
                    Some("restored existing Pod state".to_string()),
                ))
                .mark_reload(),
                Err(error) => CompanionLifecycleReport::unavailable(
                    pod_name,
                    format!("could not restore workspace Companion: {error}"),
                ),
            }
        }
        CompanionLifecyclePlan::Spawn => {
            match spawn_workspace_companion_pod(workspace_root, &pod_name, runtime_command).await {
                Ok(()) => CompanionLifecycleReport::with_state(CompanionPanelState::new(
                    pod_name,
                    CompanionPanelStatus::Spawned,
                    Some("launched with default Companion profile".to_string()),
                ))
                .mark_reload(),
                Err(error) => CompanionLifecycleReport::unavailable(
                    pod_name,
                    format!("could not spawn workspace Companion: {error}"),
                ),
            }
        }
        CompanionLifecyclePlan::Unavailable(message) => {
            CompanionLifecycleReport::unavailable(pod_name, message)
        }
    }
}

fn observe_workspace_companion(
    pod_name: String,
    presence: CompanionPodPresence,
) -> CompanionLifecycleReport {
    match presence {
        CompanionPodPresence::Live => CompanionLifecycleReport::with_state(
            CompanionPanelState::new(pod_name, CompanionPanelStatus::Live, None),
        ),
        CompanionPodPresence::Restorable => CompanionLifecycleReport::with_state(
            CompanionPanelState::new(pod_name, CompanionPanelStatus::Stopped, None),
        ),
        CompanionPodPresence::Missing => CompanionLifecycleReport::with_state(
            CompanionPanelState::new(pod_name, CompanionPanelStatus::Missing, None),
        ),
        CompanionPodPresence::Unavailable(message) => {
            CompanionLifecycleReport::unavailable(pod_name, message)
        }
    }
}

#[derive(Debug, Clone)]
struct OrchestratorLifecycleReport {
    state: Option<OrchestratorPanelState>,
    diagnostics: Vec<String>,
    reload_pods: bool,
}

impl OrchestratorLifecycleReport {
    fn skipped() -> Self {
        Self {
            state: None,
            diagnostics: Vec::new(),
            reload_pods: false,
        }
    }

    fn with_state(state: OrchestratorPanelState) -> Self {
        Self {
            state: Some(state),
            diagnostics: Vec::new(),
            reload_pods: false,
        }
    }

    fn unavailable(pod_name: String, detail: String) -> Self {
        let detail = bounded_panel_diagnostic(detail);
        Self {
            state: Some(OrchestratorPanelState::new(
                pod_name,
                OrchestratorPanelStatus::Unavailable,
                Some(detail.clone()),
            )),
            diagnostics: vec![detail],
            reload_pods: false,
        }
    }

    fn mark_reload(mut self) -> Self {
        self.reload_pods = true;
        self
    }
}

async fn ensure_workspace_orchestrator(
    workspace_root: &Path,
    config: TicketConfigAvailability,
    pod_name: String,
    presence: Option<OrchestratorPodPresence>,
    runtime_command: PodRuntimeCommand,
) -> OrchestratorLifecycleReport {
    orchestrator_lifecycle(workspace_root, config, pod_name, presence, runtime_command).await
}

fn observe_workspace_orchestrator(
    config: TicketConfigAvailability,
    pod_name: String,
    presence: Option<OrchestratorPodPresence>,
) -> OrchestratorLifecycleReport {
    if matches!(config, TicketConfigAvailability::Absent) {
        return OrchestratorLifecycleReport::skipped();
    }
    if let TicketConfigAvailability::Unusable(message) = config {
        return OrchestratorLifecycleReport::unavailable(
            pod_name,
            format!("Ticket config is unusable; workspace Orchestrator not observed: {message}"),
        );
    }
    match presence.unwrap_or(OrchestratorPodPresence::Missing) {
        OrchestratorPodPresence::Live => OrchestratorLifecycleReport::with_state(
            OrchestratorPanelState::new(pod_name, OrchestratorPanelStatus::Live, None),
        ),
        OrchestratorPodPresence::Restorable => OrchestratorLifecycleReport::with_state(
            OrchestratorPanelState::new(pod_name, OrchestratorPanelStatus::Stopped, None),
        ),
        OrchestratorPodPresence::Missing => OrchestratorLifecycleReport::with_state(
            OrchestratorPanelState::new(pod_name, OrchestratorPanelStatus::Missing, None),
        ),
        OrchestratorPodPresence::Unavailable(message) => {
            OrchestratorLifecycleReport::unavailable(pod_name, message)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct OrchestrationWorktreeLayout {
    path: PathBuf,
    branch: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum OrchestrationWorktreeStatus {
    Created,
    Reused,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct OrchestrationWorktreeReady {
    layout: OrchestrationWorktreeLayout,
    status: OrchestrationWorktreeStatus,
}

fn orchestration_worktree_layout_for_config(
    workspace_root: &Path,
    orchestration: &TicketOrchestrationConfig,
) -> OrchestrationWorktreeLayout {
    OrchestrationWorktreeLayout {
        path: workspace_root
            .join(orchestration.worktree_dir())
            .join(orchestration.worktree_name()),
        branch: orchestration.effective_branch_name().to_string(),
    }
}

#[cfg(test)]
fn orchestration_worktree_layout(workspace_root: &Path) -> OrchestrationWorktreeLayout {
    OrchestrationWorktreeLayout {
        path: workspace_root.join(".worktree").join("orchestration"),
        branch: "orchestration".to_string(),
    }
}

fn resolved_orchestration_worktree_layout(
    workspace_root: &Path,
) -> Result<OrchestrationWorktreeLayout, String> {
    let config = TicketConfig::load_workspace(workspace_root)
        .map_err(|err| format!("failed to load ticket config for orchestration worktree: {err}"))?;
    let branch = config.orchestration.effective_branch_name();
    GitBranchName::new(branch.to_string())
        .map_err(|message| format!("invalid orchestration branch `{branch}`: {message}"))?;
    Ok(orchestration_worktree_layout_for_config(
        workspace_root,
        &config.orchestration,
    ))
}

fn build_orchestrator_launch_context(
    original_workspace_root: &Path,
    orchestration_workspace_root: &Path,
    pod_name: &str,
) -> TicketRoleLaunchContext {
    let mut context = TicketRoleLaunchContext::new(
        orchestration_workspace_root.to_path_buf(),
        TicketRole::Orchestrator,
    )
    .with_cwd(orchestration_workspace_root.to_path_buf())
    .with_original_workspace_root(original_workspace_root.to_path_buf())
    .with_target_workspace_root(original_workspace_root.to_path_buf());
    context.pod_name = Some(pod_name.to_string());
    context.user_instruction = Some(
        "Workspace Dashboard opened for this Ticket-enabled workspace. Coordinate Ticket routing and wait for explicit follow-up before spawning role Pods."
            .to_string(),
    );
    context
}

fn ensure_orchestration_worktree(
    workspace_root: &Path,
) -> Result<OrchestrationWorktreeReady, String> {
    let layout = resolved_orchestration_worktree_layout(workspace_root)?;
    if layout.path.exists() {
        if !layout.path.is_dir() {
            return Err(format!(
                "orchestration worktree path exists but is not a directory: {}",
                layout.path.display()
            ));
        }
        if git_inside_worktree(&layout.path) {
            validate_existing_orchestration_worktree(workspace_root, &layout)?;
            if !git_worktree_clean(&layout.path) {
                return Err(format!(
                    "orchestration worktree is dirty; refusing automatic reuse: {}",
                    layout.path.display()
                ));
            }
            return Ok(OrchestrationWorktreeReady {
                layout,
                status: OrchestrationWorktreeStatus::Reused,
            });
        }
        return Err(format!(
            "orchestration worktree path exists but is not a Git worktree: {}",
            layout.path.display()
        ));
    }

    if let Some(parent) = layout.path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            format!(
                "could not create orchestration worktree parent {}: {error}",
                parent.display()
            )
        })?;
    }

    let branch_exists = git_branch_exists(workspace_root, &layout.branch)?;
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(workspace_root)
        .arg("worktree")
        .arg("add");
    if branch_exists {
        command.arg(&layout.path).arg(&layout.branch);
    } else {
        command
            .arg(&layout.path)
            .arg("-b")
            .arg(&layout.branch)
            .arg("HEAD");
    }
    run_git_command(command, "create orchestration worktree")?;

    Ok(OrchestrationWorktreeReady {
        layout,
        status: OrchestrationWorktreeStatus::Created,
    })
}

fn prepare_orchestration_worktree_for_restore(
    workspace_root: &Path,
) -> Result<OrchestrationWorktreeReady, String> {
    let layout = resolved_orchestration_worktree_layout(workspace_root)?;
    if !layout.path.exists() {
        return Err(format!(
            "orchestration worktree is missing; cannot restore existing Pod state: {}",
            layout.path.display()
        ));
    }
    if !layout.path.is_dir() {
        return Err(format!(
            "orchestration worktree path exists but is not a directory: {}",
            layout.path.display()
        ));
    }
    if !git_inside_worktree(&layout.path) {
        return Err(format!(
            "orchestration worktree path exists but is not a Git worktree: {}",
            layout.path.display()
        ));
    }
    validate_existing_orchestration_worktree(workspace_root, &layout)?;
    Ok(OrchestrationWorktreeReady {
        layout,
        status: OrchestrationWorktreeStatus::Reused,
    })
}

fn validate_existing_orchestration_worktree(
    workspace_root: &Path,
    layout: &OrchestrationWorktreeLayout,
) -> Result<(), String> {
    let expected_top_level = layout.path.canonicalize().map_err(|error| {
        format!(
            "could not canonicalize orchestration worktree path {}: {error}",
            layout.path.display()
        )
    })?;
    let actual_top_level = git_top_level(&layout.path)?;
    if actual_top_level != expected_top_level {
        return Err(format!(
            "orchestration path {} is inside Git worktree {}, but is not the worktree root",
            layout.path.display(),
            actual_top_level.display()
        ));
    }

    let current_branch = git_current_branch(&layout.path)?;
    if current_branch.as_deref() != Some(layout.branch.as_str()) {
        return Err(format!(
            "orchestration worktree {} is on branch {:?}, expected {}",
            layout.path.display(),
            current_branch,
            layout.branch
        ));
    }

    let original_common = git_common_dir(workspace_root)?;
    let worktree_common = git_common_dir(&layout.path)?;
    if original_common != worktree_common {
        return Err(format!(
            "orchestration worktree {} belongs to a different Git repository ({} != {})",
            layout.path.display(),
            worktree_common.display(),
            original_common.display()
        ));
    }
    Ok(())
}

fn git_top_level(path: &Path) -> Result<PathBuf, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .arg("rev-parse")
        .arg("--show-toplevel")
        .output()
        .map_err(|error| {
            format!(
                "could not query Git top-level for {}: {error}",
                path.display()
            )
        })?;
    if !output.status.success() {
        return Err(format!(
            "could not query Git top-level for {}: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    PathBuf::from(String::from_utf8_lossy(&output.stdout).trim().to_string())
        .canonicalize()
        .map_err(|error| {
            format!(
                "could not canonicalize Git top-level for {}: {error}",
                path.display()
            )
        })
}

fn git_current_branch(path: &Path) -> Result<Option<String>, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .arg("branch")
        .arg("--show-current")
        .output()
        .map_err(|error| {
            format!(
                "could not query current branch for {}: {error}",
                path.display()
            )
        })?;
    if !output.status.success() {
        return Err(format!(
            "could not query current branch for {}: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok((!branch.is_empty()).then_some(branch))
}

fn git_common_dir(path: &Path) -> Result<PathBuf, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .arg("rev-parse")
        .arg("--git-common-dir")
        .output()
        .map_err(|error| {
            format!(
                "could not query Git common dir for {}: {error}",
                path.display()
            )
        })?;
    if !output.status.success() {
        return Err(format!(
            "could not query Git common dir for {}: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let raw = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim().to_string());
    let common = if raw.is_absolute() {
        raw
    } else {
        path.join(raw)
    };
    common.canonicalize().map_err(|error| {
        format!(
            "could not canonicalize Git common dir {}: {error}",
            common.display()
        )
    })
}

fn git_inside_worktree(path: &Path) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(path)
        .arg("rev-parse")
        .arg("--is-inside-work-tree")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn git_worktree_clean(path: &Path) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(path)
        .arg("status")
        .arg("--porcelain")
        .output()
        .map(|output| output.status.success() && output.stdout.is_empty())
        .unwrap_or(false)
}

fn git_branch_exists(workspace_root: &Path, branch: &str) -> Result<bool, String> {
    let status = Command::new("git")
        .arg("-C")
        .arg(workspace_root)
        .arg("show-ref")
        .arg("--verify")
        .arg("--quiet")
        .arg(format!("refs/heads/{branch}"))
        .status()
        .map_err(|error| format!("could not query orchestration branch `{branch}`: {error}"))?;
    Ok(status.success())
}

fn run_git_command(mut command: Command, action: &str) -> Result<(), String> {
    let output = command
        .output()
        .map_err(|error| format!("could not run git to {action}: {error}"))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let detail = if stderr.is_empty() { stdout } else { stderr };
    Err(format!("git failed to {action}: {detail}"))
}

async fn orchestrator_lifecycle(
    workspace_root: &Path,
    config: TicketConfigAvailability,
    pod_name: String,
    presence: Option<OrchestratorPodPresence>,
    runtime_command: PodRuntimeCommand,
) -> OrchestratorLifecycleReport {
    if matches!(config, TicketConfigAvailability::Absent) {
        return OrchestratorLifecycleReport::skipped();
    }
    let presence = presence.unwrap_or(OrchestratorPodPresence::Missing);
    match decide_orchestrator_lifecycle(&config, &presence) {
        OrchestratorLifecyclePlan::SkipNoTicketConfig => OrchestratorLifecycleReport::skipped(),
        OrchestratorLifecyclePlan::ReportLive => OrchestratorLifecycleReport::with_state(
            OrchestratorPanelState::new(pod_name, OrchestratorPanelStatus::Live, None),
        ),
        OrchestratorLifecyclePlan::Restore => {
            match prepare_orchestration_worktree_for_restore(workspace_root) {
                Ok(worktree) => {
                    match restore_orchestrator_pod(
                        workspace_root,
                        &worktree.layout.path,
                        &pod_name,
                        runtime_command.clone(),
                    )
                    .await
                    {
                        Ok(()) => {
                            OrchestratorLifecycleReport::with_state(OrchestratorPanelState::new(
                                pod_name,
                                OrchestratorPanelStatus::Restored,
                                Some(format!(
                                    "restored existing Pod state in orchestration worktree {} on branch {}",
                                    worktree.layout.path.display(),
                                    worktree.layout.branch
                                )),
                            ))
                            .mark_reload()
                        }
                        Err(error) => OrchestratorLifecycleReport::unavailable(
                            pod_name,
                            format!("could not restore workspace Orchestrator: {error}"),
                        ),
                    }
                }
                Err(error) => OrchestratorLifecycleReport::unavailable(
                    pod_name,
                    format!("could not prepare orchestration worktree for restore: {error}"),
                ),
            }
        }
        OrchestratorLifecyclePlan::Spawn => match ensure_orchestration_worktree(workspace_root) {
            Ok(worktree) => {
                let worktree_note = match worktree.status {
                    OrchestrationWorktreeStatus::Created => format!(
                        "created orchestration worktree {} on branch {}",
                        worktree.layout.path.display(),
                        worktree.layout.branch
                    ),
                    OrchestrationWorktreeStatus::Reused => format!(
                        "reused orchestration worktree {} on branch {}",
                        worktree.layout.path.display(),
                        worktree.layout.branch
                    ),
                };
                match spawn_orchestrator_pod(
                    workspace_root,
                    &worktree.layout.path,
                    &pod_name,
                    runtime_command,
                )
                .await
                {
                    Ok(profile) => {
                        OrchestratorLifecycleReport::with_state(OrchestratorPanelState::new(
                            pod_name,
                            OrchestratorPanelStatus::Spawned,
                            Some(format!("launched with profile {profile}; {worktree_note}")),
                        ))
                        .mark_reload()
                    }
                    Err(error) => OrchestratorLifecycleReport::unavailable(
                        pod_name,
                        format!("could not spawn workspace Orchestrator: {error}"),
                    ),
                }
            }
            Err(error) => OrchestratorLifecycleReport::unavailable(
                pod_name,
                format!("could not prepare orchestration worktree: {error}"),
            ),
        },
        OrchestratorLifecyclePlan::Unavailable(message) => {
            OrchestratorLifecycleReport::unavailable(pod_name, message)
        }
    }
}

async fn restore_workspace_companion_pod(
    workspace_root: &Path,
    pod_name: &str,
    runtime_command: PodRuntimeCommand,
) -> Result<(), client::SpawnError> {
    let config = SpawnConfig {
        runtime_command,
        pod_name: pod_name.to_string(),
        profile: None,
        ticket_role: None,
        workspace_root: workspace_root.to_path_buf(),
        cwd: None,
        resume_from: None,
    };
    spawn_pod(config, |_| {}).await.map(|_| ())
}

async fn spawn_workspace_companion_pod(
    workspace_root: &Path,
    pod_name: &str,
    runtime_command: PodRuntimeCommand,
) -> Result<(), client::SpawnError> {
    let config = SpawnConfig {
        runtime_command,
        pod_name: pod_name.to_string(),
        profile: None,
        ticket_role: None,
        workspace_root: workspace_root.to_path_buf(),
        cwd: None,
        resume_from: None,
    };
    spawn_pod(config, |_| {}).await.map(|_| ())
}

async fn restore_orchestrator_pod(
    original_workspace_root: &Path,
    workspace_root: &Path,
    pod_name: &str,
    runtime_command: PodRuntimeCommand,
) -> Result<(), client::SpawnError> {
    let config = SpawnConfig {
        runtime_command,
        pod_name: pod_name.to_string(),
        profile: None,
        ticket_role: Some("orchestrator".to_string()),
        workspace_root: original_workspace_root.to_path_buf(),
        cwd: Some(workspace_root.to_path_buf()),
        resume_from: None,
    };
    spawn_pod(config, |_| {}).await.map(|_| ())
}

async fn spawn_orchestrator_pod(
    original_workspace_root: &Path,
    orchestration_workspace_root: &Path,
    pod_name: &str,
    runtime_command: PodRuntimeCommand,
) -> Result<String, client::TicketRoleLaunchError> {
    let context = build_orchestrator_launch_context(
        original_workspace_root,
        orchestration_workspace_root,
        pod_name,
    );
    let result = launch_ticket_role_pod(context, runtime_command, |_| {}).await?;
    Ok(result.plan.profile)
}

fn current_workspace_root() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

fn orchestrator_status_is_peer_reachable(status: OrchestratorPanelStatus) -> bool {
    matches!(
        status,
        OrchestratorPanelStatus::Live
            | OrchestratorPanelStatus::Restored
            | OrchestratorPanelStatus::Spawned
    )
}

fn unique_preticket_intake_pod_name() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    format!("ticket-intake-{nanos:x}")
}

fn existing_ticket_claim_notice(
    ticket_id: &str,
    pod_name: &str,
    status: TicketLocalClaimStatus,
) -> String {
    match status {
        TicketLocalClaimStatus::Live | TicketLocalClaimStatus::Restorable => format!(
            "Ticket {ticket_id} is already claimed by local Intake Pod {pod_name} ({}); open that Pod instead of starting a second Intake.",
            status.label()
        ),
        TicketLocalClaimStatus::Stale => format!(
            "Ticket {ticket_id} has a stale local Intake claim for {pod_name}; explicit reclaim/diagnostic is required before starting a replacement."
        ),
    }
}

async fn load_pod_list(
    selected_name: Option<String>,
    max_entries: usize,
) -> Result<PodList, DashboardError> {
    let store_dir = default_store_dir()?;
    let store = FsStore::new(&store_dir)?;
    let pod_store = FsPodStore::new(default_pod_store_dir()?).map_err(io::Error::other)?;
    let stored = read_stored_pod_infos(&store, &pod_store)?;
    let live = read_reachable_live_pod_infos(&store)
        .await
        .unwrap_or_default();
    Ok(PodList::from_workspace_sources(
        PodVisibilitySource::ResumePicker,
        stored,
        live,
        selected_name,
        max_entries,
        &current_workspace_root(),
    ))
}

#[derive(Debug, Clone)]
pub(crate) struct CompanionSendRequest {
    pub(crate) pod_name: String,
    pub(crate) socket_path: PathBuf,
    pub(crate) segments: Vec<Segment>,
}

#[derive(Debug, Clone)]
pub(crate) struct CompanionSendOutcome {
    pub(crate) notice: String,
}

#[derive(Debug)]
pub(crate) enum CompanionSendError {
    Connect {
        pod_name: String,
        source: std::io::Error,
    },
    Write {
        pod_name: String,
        source: std::io::Error,
    },
    Read {
        pod_name: String,
        source: std::io::Error,
    },
    Rejected {
        pod_name: String,
        message: String,
    },
    Closed {
        pod_name: String,
    },
}

impl fmt::Display for CompanionSendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Connect { pod_name, source } => {
                write!(f, "Companion {pod_name} is unreachable: {source}")
            }
            Self::Write { pod_name, source } => {
                write!(f, "Failed to send to Companion {pod_name}: {source}")
            }
            Self::Read { pod_name, source } => {
                write!(f, "Failed while waiting for Companion {pod_name}: {source}")
            }
            Self::Rejected { pod_name, message } => {
                write!(f, "Companion {pod_name} rejected the message: {message}")
            }
            Self::Closed { pod_name } => {
                write!(
                    f,
                    "Companion {pod_name} closed the socket before accepting the message"
                )
            }
        }
    }
}

impl std::error::Error for CompanionSendError {}

#[derive(Debug, Clone)]
pub(crate) struct TicketActionRequest {
    workspace_root: PathBuf,
    ticket_id: String,
    action: NextUserAction,
    orchestrator: Option<OrchestratorNotifyTarget>,
}

#[derive(Debug, Clone)]
struct OrchestratorNotifyTarget {
    pod_name: String,
    socket_path: PathBuf,
}

fn orchestrator_queue_attention_notice_target(
    panel: &WorkspacePanelViewModel,
    list: &PodList,
) -> Option<OrchestratorNotifyTarget> {
    let orchestrator = panel.header.orchestrator.as_ref()?;
    if !matches!(orchestrator.status, OrchestratorPanelStatus::Live) {
        return None;
    }
    let entry = list
        .entries
        .iter()
        .find(|entry| entry.name == orchestrator.pod_name)?;
    if !entry.actions.can_open {
        return None;
    }
    let live = entry.live.as_ref()?;
    if !live.reachable || live.status != Some(PodStatus::Idle) {
        return None;
    }
    Some(OrchestratorNotifyTarget {
        pod_name: orchestrator.pod_name.clone(),
        socket_path: live.socket_path.clone(),
    })
}

fn derive_orchestrator_work_set(
    panel: &WorkspacePanelViewModel,
    previous_planned: BTreeSet<String>,
) -> OrchestratorWorkSet {
    let active_inprogress = panel
        .rows
        .iter()
        .filter_map(|row| {
            let ticket = row.ticket.as_ref()?;
            if let Some(overlay) = ticket
                .orchestration_overlay
                .as_ref()
                .filter(|overlay| overlay.workflow_state == TicketWorkflowState::InProgress)
            {
                Some(OrchestratorActiveWorkItem {
                    id: ticket.id.clone(),
                    title: ticket.title.clone(),
                    status: format!(
                        "local: {} · {}: {}",
                        ticket.workflow_state.as_str(),
                        overlay.source,
                        overlay.workflow_state.as_str()
                    ),
                })
            } else if ticket.workflow_state == TicketWorkflowState::InProgress {
                Some(OrchestratorActiveWorkItem {
                    id: ticket.id.clone(),
                    title: ticket.title.clone(),
                    status: ticket.workflow_state.as_str().to_string(),
                })
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    let active_wait = if active_inprogress.is_empty() {
        None
    } else {
        Some(format!(
            "waiting for active_inprogress: {}",
            active_inprogress
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ))
    };
    let queued = panel
        .rows
        .iter()
        .filter_map(|row| {
            let ticket = row.ticket.as_ref()?;
            if ticket.workflow_state != TicketWorkflowState::Queued {
                return None;
            }
            let duplicate_guard = queued_duplicate_guard(ticket, row);
            let waiting_reason = active_wait
                .clone()
                .or_else(|| {
                    ticket
                        .blocked_reason
                        .as_ref()
                        .map(|reason| format!("blocked by Ticket relation diagnostics: {reason}"))
                })
                .or(duplicate_guard);
            let classification =
                if waiting_reason.is_some() || previous_planned.contains(&ticket.id) {
                    OrchestratorQueuedClassification::PlannedQueued
                } else {
                    OrchestratorQueuedClassification::NewQueued
                };
            Some(OrchestratorQueuedWorkItem {
                id: ticket.id.clone(),
                title: ticket.title.clone(),
                classification,
                waiting_reason,
            })
        })
        .collect::<Vec<_>>();
    let fingerprint = orchestrator_work_set_fingerprint(&active_inprogress, &queued);
    OrchestratorWorkSet {
        active_inprogress,
        queued,
        fingerprint,
    }
}

fn queued_duplicate_guard(
    ticket: &crate::workspace_panel::TicketPanelEntry,
    row: &PanelRow,
) -> Option<String> {
    let mut guards = Vec::new();
    if let Some(claim) = ticket.local_claim.as_ref().filter(|claim| {
        matches!(
            claim.status,
            TicketLocalClaimStatus::Live | TicketLocalClaimStatus::Restorable
        )
    }) {
        guards.push(format!(
            "local {} claim {} ({})",
            claim.role,
            claim.pod_name,
            claim.status.label()
        ));
    }
    for pod in ticket.related_pods.iter().chain(row.related_pods.iter()) {
        if !guards.iter().any(|guard| guard.contains(pod)) {
            guards.push(format!("related pod/worktree {pod}"));
        }
    }
    if let Some(overlay) = ticket.orchestration_overlay.as_ref() {
        guards.push(format!(
            "{} worktree overlay shows state {}",
            overlay.source,
            overlay.workflow_state.as_str()
        ));
    }
    if guards.is_empty() {
        None
    } else {
        Some(format!(
            "waiting on existing role/session or visible pod/worktree before duplicate start: {}",
            guards.join(", ")
        ))
    }
}

fn orchestrator_work_set_fingerprint(
    active: &[OrchestratorActiveWorkItem],
    queued: &[OrchestratorQueuedWorkItem],
) -> String {
    let active = active
        .iter()
        .map(|item| format!("active:{}:{}", item.id, item.status))
        .collect::<Vec<_>>()
        .join("|");
    let queued = queued
        .iter()
        .map(|item| {
            format!(
                "queued:{}:{}:{}",
                item.id,
                item.classification.as_str(),
                item.waiting_reason.as_deref().unwrap_or("actionable")
            )
        })
        .collect::<Vec<_>>()
        .join("|");
    format!("active=[{active}];queued=[{queued}]")
}

fn orchestrator_queue_attention_notice(
    panel: &WorkspacePanelViewModel,
    work_set: &OrchestratorWorkSet,
) -> Option<OrchestratorQueueAttentionNotice> {
    if work_set.has_active_inprogress() {
        return None;
    }
    let actionable = work_set.actionable_queued();
    if actionable.is_empty() {
        return None;
    }
    let waiting = work_set
        .queued
        .iter()
        .filter(|item| item.waiting_reason.is_some())
        .collect::<Vec<_>>();
    let ticket_count = actionable.len() + waiting.len();
    let actionable_tickets = actionable
        .iter()
        .take(ORCHESTRATOR_QUEUE_ATTENTION_MAX_TICKETS)
        .map(|item| orchestrator_queue_template_ticket(item))
        .collect::<Vec<_>>();
    let remaining_capacity =
        ORCHESTRATOR_QUEUE_ATTENTION_MAX_TICKETS.saturating_sub(actionable_tickets.len());
    let waiting_tickets = waiting
        .iter()
        .take(remaining_capacity)
        .map(|item| orchestrator_queue_template_ticket(item))
        .collect::<Vec<_>>();
    let rendered =
        render_orchestrator_queue_attention_template(&OrchestratorQueueTemplateContext {
            workspace: bounded_progress_text(
                &panel.header.workspace_label,
                ORCHESTRATOR_QUEUE_ATTENTION_MAX_TEXT_CHARS,
            ),
            actionable_tickets,
            waiting_tickets,
            omitted_ticket_count: ticket_count
                .saturating_sub(ORCHESTRATOR_QUEUE_ATTENTION_MAX_TICKETS),
        })
        .ok()?;
    let message = bounded_progress_text(&rendered, ORCHESTRATOR_QUEUE_ATTENTION_MAX_MESSAGE_CHARS);
    let fingerprint = format!("idle-queue:{}", work_set.fingerprint);
    Some(OrchestratorQueueAttentionNotice {
        message,
        fingerprint,
    })
}

fn orchestrator_queue_template_ticket(
    item: &&OrchestratorQueuedWorkItem,
) -> OrchestratorQueueTemplateTicket {
    OrchestratorQueueTemplateTicket {
        id: bounded_progress_text(&item.id, ORCHESTRATOR_QUEUE_ATTENTION_MAX_TEXT_CHARS),
        title: bounded_progress_text(&item.title, ORCHESTRATOR_QUEUE_ATTENTION_MAX_TEXT_CHARS),
        classification: item.classification.as_str(),
        waiting_reason: item.waiting_reason.as_ref().map(|reason| {
            bounded_progress_text(reason, ORCHESTRATOR_QUEUE_ATTENTION_MAX_TEXT_CHARS)
        }),
    }
}

fn render_orchestrator_queue_attention_template(
    context: &OrchestratorQueueTemplateContext,
) -> Result<String, minijinja::Error> {
    let mut env = minijinja::Environment::new();
    env.set_undefined_behavior(minijinja::UndefinedBehavior::Strict);
    env.add_template(
        "orchestrator_idle_queue_notice",
        ORCHESTRATOR_IDLE_QUEUE_NOTICE_TEMPLATE,
    )?;
    env.get_template("orchestrator_idle_queue_notice")?
        .render(context)
}

fn orchestrator_work_set_detail(
    work_set: &OrchestratorWorkSet,
    freshness: Option<&OrchestratorQueueAttentionFreshness>,
) -> Option<String> {
    if work_set.is_empty() {
        return freshness.map(|freshness| {
            format!(
                "queued-work attention last sent at {} (idle auto-run notify)",
                freshness.updated_at
            )
        });
    }
    if work_set.has_active_inprogress() {
        let active = work_set
            .active_inprogress
            .iter()
            .take(3)
            .map(|item| item.id.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let queued = work_set
            .queued
            .iter()
            .take(3)
            .map(|item| match item.waiting_reason.as_deref() {
                Some(reason) => format!("{} ({reason})", item.id),
                None => item.id.clone(),
            })
            .collect::<Vec<_>>()
            .join(", ");
        return Some(bounded_panel_diagnostic(format!(
            "queued-work attention suppressed; active_inprogress: {active}; planned queued: {queued}"
        )));
    }
    let actionable = work_set.actionable_queued();
    let waiting = work_set.queued.len().saturating_sub(actionable.len());
    if !actionable.is_empty() {
        let classes = actionable
            .iter()
            .take(3)
            .map(|item| format!("{} ({})", item.id, item.classification.as_str()))
            .collect::<Vec<_>>()
            .join(", ");
        let sent = freshness
            .map(|freshness| format!("; last sent at {}", freshness.updated_at))
            .unwrap_or_default();
        return Some(bounded_panel_diagnostic(format!(
            "queued-work attention pending: {classes}; waiting queued: {waiting}{sent}"
        )));
    }
    let waiting = work_set
        .queued
        .iter()
        .take(3)
        .map(|item| match item.waiting_reason.as_deref() {
            Some(reason) => format!("{} ({reason})", item.id),
            None => item.id.clone(),
        })
        .collect::<Vec<_>>()
        .join(", ");
    Some(bounded_panel_diagnostic(format!(
        "queued-work attention waiting: {waiting}"
    )))
}

fn apply_orchestrator_detail(panel: &mut WorkspacePanelViewModel, detail: Option<String>) {
    let Some(orchestrator) = panel.header.orchestrator.as_mut() else {
        return;
    };
    if matches!(orchestrator.status, OrchestratorPanelStatus::Unavailable) {
        return;
    }
    if let Some(detail) = detail {
        orchestrator.detail = Some(detail);
    }
}

fn bounded_progress_text(input: &str, max_chars: usize) -> String {
    let mut output = String::new();
    for (idx, ch) in input.chars().enumerate() {
        if idx >= max_chars {
            output.push('…');
            return output;
        }
        let sanitized = if ch.is_control() && ch != '\n' && ch != '\t' {
            ' '
        } else {
            ch
        };
        output.push(sanitized);
    }
    output
}

fn progress_notice_timestamp() -> String {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => format!("unix:{}", duration.as_secs()),
        Err(_) => "unix:0".to_string(),
    }
}

async fn dispatch_orchestrator_queue_attention_notice(
    request: OrchestratorQueueAttentionNoticeRequest,
) -> OrchestratorQueueAttentionNoticeResult {
    let fingerprint = request.notice.fingerprint.clone();
    match send_notify_only(&request.socket_path, request.notice.message, true).await {
        Ok(()) => {
            OrchestratorQueueAttentionNoticeResult::sent(fingerprint, progress_notice_timestamp())
        }
        Err(err) => OrchestratorQueueAttentionNoticeResult::failed(
            fingerprint,
            format!("{}: {}", request.pod_name, err),
        ),
    }
}

fn bounded_refinement_instruction(input: &str) -> String {
    bounded_progress_text(input, PANEL_READY_REFINEMENT_MAX_INSTRUCTION_CHARS)
        .trim()
        .to_string()
}

fn build_ready_ticket_refinement_thread_body(ticket_id: &str, instruction: &str) -> String {
    format!(
        "Panel returned ready Ticket {ticket_id} to planning for requirements sync. This is not Queue routing and must not start implementation.\n\n## User refinement instruction\n\n{instruction}\n"
    )
}

fn build_ready_ticket_refinement_launch_instruction(ticket_id: &str, instruction: &str) -> String {
    format!(
        "Continue Ticket Intake / requirements sync for existing Ticket {ticket_id}. The Panel has returned the Ticket from ready to planning; do not queue the Ticket, do not route implementation, and do not create a duplicate unless the user explicitly asks for one. Read TicketShow body/thread/artifacts before making requirements or readiness decisions.\n\nUser refinement instruction:\n\n{instruction}"
    )
}

fn build_ready_ticket_refinement_notify(ticket_id: &str, instruction: &str) -> String {
    format!(
        "Ticket {ticket_id} was returned from ready to planning from the Panel for requirements sync. Continue Intake/refinement only; do not Queue or route implementation. Read the Ticket thread for the recorded state change and user instruction.\n\nUser refinement instruction:\n\n{instruction}"
    )
}

async fn dispatch_ready_ticket_planning_return(
    request: ReadyTicketPlanningReturnRequest,
) -> Result<ReadyTicketPlanningReturnOutcome, TicketActionError> {
    match ticket_config_availability(&request.workspace_root) {
        TicketConfigAvailability::Usable => {}
        TicketConfigAvailability::Absent => {
            return Err(TicketActionError::Stale(
                "Ticket config is absent; workspace Dashboard no longer exposes Ticket actions"
                    .to_string(),
            ));
        }
        TicketConfigAvailability::Unusable(message) => {
            return Err(TicketActionError::Stale(format!(
                "Ticket config is unusable; workspace Dashboard no longer exposes Ticket actions: {message}"
            )));
        }
    }
    let config = TicketConfig::load_workspace(&request.workspace_root)
        .map_err(|error| TicketActionError::BackendConfig(error.to_string()))?;
    let backend = LocalTicketBackend::new(config.backend_root())
        .with_record_language(config.ticket_record_language());
    let id = TicketIdOrSlug::Id(request.ticket_id.clone());
    let ticket = backend
        .show(id.clone())
        .map_err(|error| TicketActionError::Ticket(error.to_string()))?;
    if ticket.meta.workflow_state != TicketWorkflowState::Ready {
        return Err(TicketActionError::Stale(format!(
            "Ticket {} is {}; expected ready before returning it to planning. Refresh the panel and retry if appropriate.",
            ticket.meta.id,
            ticket.meta.workflow_state.as_str()
        )));
    }
    let mut change = TicketStateChange::new(
        TicketWorkflowState::Ready.as_str(),
        TicketWorkflowState::Planning.as_str(),
        "panel_return_to_planning",
        MarkdownText::from(build_ready_ticket_refinement_thread_body(
            &ticket.meta.id,
            &request.user_instruction,
        )),
    );
    change.author = Some("workspace-panel".to_string());
    backend
        .set_workflow_state(id, change)
        .map_err(|error| TicketActionError::Ticket(error.to_string()))?;

    let notice = match request.followup {
        ReadyTicketPlanningReturnFollowup::LaunchIntake(request) => {
            ReadyTicketPlanningReturnOutcome {
                notice: format!(
                    "Ticket {} returned to planning for refinement; launching Ticket Intake…",
                    ticket.meta.id
                ),
                followup: ReadyTicketPlanningReturnAfterMutation::LaunchIntake(request),
            }
        }
        ReadyTicketPlanningReturnFollowup::NotifyLiveClaimedIntake {
            pod_name,
            socket_path,
        } => {
            let message =
                build_ready_ticket_refinement_notify(&ticket.meta.id, &request.user_instruction);
            match send_notify_only(&socket_path, message, true).await {
                Ok(()) => ReadyTicketPlanningReturnOutcome {
                    notice: format!(
                        "Ticket {} returned to planning for refinement; notified live Intake Pod {}.",
                        ticket.meta.id, pod_name
                    ),
                    followup: ReadyTicketPlanningReturnAfterMutation::None,
                },
                Err(error) => ReadyTicketPlanningReturnOutcome {
                    notice: bounded_panel_diagnostic(format!(
                        "Ticket {} returned to planning and instruction was recorded, but notifying Intake Pod {} failed: {}",
                        ticket.meta.id, pod_name, error
                    )),
                    followup: ReadyTicketPlanningReturnAfterMutation::None,
                },
            }
        }
        ReadyTicketPlanningReturnFollowup::OpenRestorableClaimedIntake(request) => {
            let pod_name = request.pod_name.clone();
            ReadyTicketPlanningReturnOutcome {
                notice: format!(
                    "Ticket {} returned to planning for refinement; opening/restoring claimed Intake Pod {}…",
                    ticket.meta.id, pod_name
                ),
                followup: ReadyTicketPlanningReturnAfterMutation::OpenClaim(request),
            }
        }
        ReadyTicketPlanningReturnFollowup::BlockedByStaleClaim { pod_name } => {
            ReadyTicketPlanningReturnOutcome {
                notice: bounded_panel_diagnostic(format!(
                    "Ticket {} returned to planning and instruction was recorded, but Intake launch was not attempted because existing Intake claim {} is stale; inspect or clear the local claim before launching another Intake Pod.",
                    ticket.meta.id, pod_name
                )),
                followup: ReadyTicketPlanningReturnAfterMutation::None,
            }
        }
    };
    Ok(notice)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TicketActionOutcome {
    notice: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TicketActionError {
    BackendConfig(String),
    Ticket(String),
    Stale(String),
}

impl std::fmt::Display for TicketActionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BackendConfig(message) => write!(f, "Ticket action unavailable: {message}"),
            Self::Ticket(message) => write!(f, "Ticket action failed: {message}"),
            Self::Stale(message) => write!(f, "Ticket action rejected: {message}"),
        }
    }
}

impl std::error::Error for TicketActionError {}

#[derive(Debug, Clone, PartialEq, Eq)]
enum OrchestratorNotificationOutcome {
    Sent { pod_name: String },
    Skipped(String),
    Warning(String),
}

impl OrchestratorNotificationOutcome {
    fn sentence(&self) -> String {
        match self {
            Self::Sent { pod_name } => format!("workspace Orchestrator {pod_name} notified"),
            Self::Skipped(reason) => format!("workspace Orchestrator not notified: {reason}"),
            Self::Warning(message) => {
                format!("workspace Orchestrator notification warning: {message}")
            }
        }
    }
}

fn ticket_action_orchestrator_target(
    panel: &WorkspacePanelViewModel,
    list: &PodList,
) -> Option<OrchestratorNotifyTarget> {
    let orchestrator = panel.header.orchestrator.as_ref()?;
    if !orchestrator_status_is_peer_reachable(orchestrator.status) {
        return None;
    }
    let entry = list
        .entries
        .iter()
        .find(|entry| entry.name == orchestrator.pod_name)?;
    if !entry.actions.can_open {
        return None;
    }
    let live = entry.live.as_ref()?;
    if !live.reachable {
        return None;
    }
    Some(OrchestratorNotifyTarget {
        pod_name: orchestrator.pod_name.clone(),
        socket_path: live.socket_path.clone(),
    })
}

async fn dispatch_ticket_action(
    request: TicketActionRequest,
) -> Result<TicketActionOutcome, TicketActionError> {
    match ticket_config_availability(&request.workspace_root) {
        TicketConfigAvailability::Usable => {}
        TicketConfigAvailability::Absent => {
            return Err(TicketActionError::Stale(
                "Ticket config is absent; workspace Dashboard no longer exposes Ticket actions"
                    .to_string(),
            ));
        }
        TicketConfigAvailability::Unusable(message) => {
            return Err(TicketActionError::Stale(format!(
                "Ticket config is unusable; workspace Dashboard no longer exposes Ticket actions: {message}"
            )));
        }
    }
    let config = TicketConfig::load_workspace(&request.workspace_root)
        .map_err(|error| TicketActionError::BackendConfig(error.to_string()))?;
    let backend = LocalTicketBackend::new(config.backend_root())
        .with_record_language(config.ticket_record_language());
    if request.action == NextUserAction::Close {
        return dispatch_panel_close(&backend, &request.ticket_id);
    }
    let authority_pods = PodList::from_sources(
        PodVisibilitySource::ResumePicker,
        Vec::new(),
        Vec::new(),
        None,
        0,
    );
    let current_row = build_current_ticket_row(&backend, &request.ticket_id, &authority_pods)
        .map_err(|error| TicketActionError::Ticket(error.to_string()))?;
    let current_ticket = current_row
        .ticket
        .as_ref()
        .ok_or_else(|| TicketActionError::Stale("current row is not a Ticket".to_string()))?;
    let current_action = current_row.next_action.ok_or_else(|| {
        TicketActionError::Stale("current Ticket no longer has an inline action".to_string())
    })?;
    if current_action != request.action {
        return Err(TicketActionError::Stale(format!(
            "current action is {} but selected action was {}; reload and retry",
            current_action.label(),
            request.action.label()
        )));
    }

    match request.action {
        NextUserAction::Queue => {
            dispatch_panel_queue(
                &request.workspace_root,
                &backend,
                &request.ticket_id,
                request.orchestrator,
                current_ticket,
            )
            .await
        }
        NextUserAction::Close => unreachable!("Close action is handled before row dispatch"),
        NextUserAction::Clarify | NextUserAction::OpenPod | NextUserAction::Wait => {
            Ok(TicketActionOutcome {
                notice: format!(
                    "{} for Ticket {} has no safe inline workspace-panel dispatch; use the Ticket workflow.",
                    request.action.label(),
                    current_ticket.id
                ),
            })
        }
    }
}

async fn dispatch_panel_queue(
    workspace_root: &Path,
    backend: &LocalTicketBackend,
    ticket_id: &str,
    orchestrator: Option<OrchestratorNotifyTarget>,
    current_ticket: &crate::workspace_panel::TicketPanelEntry,
) -> Result<TicketActionOutcome, TicketActionError> {
    if current_ticket.workflow_state != TicketWorkflowState::Ready {
        return Err(TicketActionError::Stale(format!(
            "Queue handoff check `root-ticket-state` failed for Ticket {ticket_id} at {}: state is {}, expected ready; reload and retry",
            backend.root().display(),
            current_ticket.workflow_state.as_str()
        )));
    }

    let preflight = prepare_panel_queue_handoff(workspace_root, backend, ticket_id)?;
    let root_merge = sync_orchestration_to_root_before_queue(&preflight)?;
    ensure_ticket_state(
        backend,
        ticket_id,
        TicketWorkflowState::Ready,
        "root-ticket-state-after-orchestration-merge",
        &preflight.root_top_level,
    )?;
    backend
        .queue_ready(TicketIdOrSlug::Id(ticket_id.to_owned()), "workspace-panel")
        .map_err(|error| TicketActionError::Ticket(error.to_string()))?;
    let commit = commit_panel_queue_ticket_record(&preflight)?;
    let sync = sync_panel_queue_to_orchestration(&preflight, &commit)?;
    verify_panel_queue_synced(&preflight, &commit)?;
    let notification = notify_workspace_orchestrator(orchestrator, current_ticket).await;
    Ok(TicketActionOutcome {
        notice: format!(
            "Queued Ticket {}; root Queue commit {}; {}; orchestration sync {}; {}. Orchestrator routing is authorized; implementation side effects still require queued -> inprogress acceptance.",
            ticket_id,
            commit.sha,
            root_merge.sentence(),
            sync.sentence(),
            notification.sentence()
        ),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PanelQueueHandoffPreflight {
    ticket_id: String,
    root_top_level: PathBuf,
    orchestration: OrchestrationWorktreeLayout,
    ticket_record_dir: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PanelQueueCommit {
    sha: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PanelQueueRootMerge {
    branch: String,
    head: String,
    merge_commit: Option<String>,
}

impl PanelQueueRootMerge {
    fn sentence(&self) -> String {
        match &self.merge_commit {
            Some(commit) => format!(
                "merged orchestration branch {} ({}) into root before Queue: {}",
                self.branch, self.head, commit
            ),
            None => format!(
                "orchestration branch {} ({}) already present in root",
                self.branch, self.head
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PanelQueueSync {
    path: PathBuf,
    branch: String,
    head: String,
}

impl PanelQueueSync {
    fn sentence(&self) -> String {
        format!(
            "ff-only synced {} ({}) to {}",
            self.path.display(),
            self.branch,
            self.head
        )
    }
}

fn prepare_panel_queue_handoff(
    workspace_root: &Path,
    backend: &LocalTicketBackend,
    ticket_id: &str,
) -> Result<PanelQueueHandoffPreflight, TicketActionError> {
    let root_top_level = git_top_level(workspace_root).map_err(|message| {
        queue_check_failed("root-worktree-identity", ticket_id, workspace_root, message)
    })?;
    let expected_root = workspace_root.canonicalize().map_err(|error| {
        queue_check_failed(
            "root-worktree-identity",
            ticket_id,
            workspace_root,
            format!("could not canonicalize root workspace path: {error}"),
        )
    })?;
    if root_top_level != expected_root {
        return Err(queue_check_failed(
            "root-worktree-identity",
            ticket_id,
            workspace_root,
            format!(
                "Git top-level is {}, expected root workspace {}",
                root_top_level.display(),
                expected_root.display()
            ),
        ));
    }

    let root_branch = git_current_branch(&root_top_level).map_err(|message| {
        queue_check_failed("root-branch", ticket_id, &root_top_level, message)
    })?;
    let _root_branch = root_branch.ok_or_else(|| {
        queue_check_failed(
            "root-branch",
            ticket_id,
            &root_top_level,
            "root workspace is detached; expected merge target branch".to_string(),
        )
    })?;
    ensure_git_effective_user(&root_top_level).map_err(|message| {
        queue_check_failed("root-git-user", ticket_id, &root_top_level, message)
    })?;

    let orchestration =
        resolved_orchestration_worktree_layout(&root_top_level).map_err(|message| {
            queue_check_failed(
                "orchestration-branch-config",
                ticket_id,
                &root_top_level,
                message,
            )
        })?;
    if !orchestration.path.exists() {
        return Err(queue_check_failed(
            "orchestration-worktree-identity",
            ticket_id,
            &orchestration.path,
            "dedicated orchestration worktree is missing; open the Panel with Orchestrator support before Queue".to_string(),
        ));
    }
    validate_existing_orchestration_worktree(&root_top_level, &orchestration).map_err(
        |message| {
            queue_check_failed(
                "orchestration-worktree-identity",
                ticket_id,
                &orchestration.path,
                message,
            )
        },
    )?;
    ensure_git_clean("orchestration-clean", ticket_id, &orchestration.path)?;

    let orchestration_branch = git_current_branch(&orchestration.path).map_err(|message| {
        queue_check_failed(
            "orchestration-branch",
            ticket_id,
            &orchestration.path,
            message,
        )
    })?;
    if orchestration_branch.as_deref() != Some(orchestration.branch.as_str()) {
        return Err(queue_check_failed(
            "orchestration-branch",
            ticket_id,
            &orchestration.path,
            format!(
                "orchestration branch is {:?}, expected {}",
                orchestration_branch, orchestration.branch
            ),
        ));
    }

    let root_common = git_common_dir(&root_top_level).map_err(|message| {
        queue_check_failed("shared-common-dir", ticket_id, &root_top_level, message)
    })?;
    let orchestration_common = git_common_dir(&orchestration.path).map_err(|message| {
        queue_check_failed("shared-common-dir", ticket_id, &orchestration.path, message)
    })?;
    if root_common != orchestration_common {
        return Err(queue_check_failed(
            "shared-common-dir",
            ticket_id,
            &orchestration.path,
            format!(
                "orchestration common dir {} differs from root common dir {}",
                orchestration_common.display(),
                root_common.display()
            ),
        ));
    }

    ensure_ticket_state(
        backend,
        ticket_id,
        TicketWorkflowState::Ready,
        "root-ticket-state",
        &root_top_level,
    )?;

    let ticket_record_dir = backend.root().join(ticket_id);
    if !ticket_record_dir.join("item.md").is_file() {
        return Err(queue_check_failed(
            "target-ticket-record",
            ticket_id,
            &ticket_record_dir,
            "target Ticket item.md is missing".to_string(),
        ));
    }
    ensure_git_path_clean(
        "root-ticket-clean",
        ticket_id,
        &root_top_level,
        &ticket_record_dir,
    )?;

    Ok(PanelQueueHandoffPreflight {
        ticket_id: ticket_id.to_string(),
        root_top_level,
        orchestration,
        ticket_record_dir,
    })
}

fn sync_orchestration_to_root_before_queue(
    preflight: &PanelQueueHandoffPreflight,
) -> Result<PanelQueueRootMerge, TicketActionError> {
    let orchestration_head =
        git_rev_parse(&preflight.orchestration.path, "HEAD").map_err(|message| {
            queue_check_failed(
                "root-orchestration-merge",
                &preflight.ticket_id,
                &preflight.orchestration.path,
                message,
            )
        })?;
    let root_head = git_rev_parse(&preflight.root_top_level, "HEAD").map_err(|message| {
        queue_check_failed(
            "root-orchestration-merge",
            &preflight.ticket_id,
            &preflight.root_top_level,
            message,
        )
    })?;
    if ensure_git_ancestor(&preflight.root_top_level, &orchestration_head, &root_head).is_ok() {
        return Ok(PanelQueueRootMerge {
            branch: preflight.orchestration.branch.clone(),
            head: orchestration_head,
            merge_commit: None,
        });
    }

    let mut merge = Command::new("git");
    merge
        .arg("-C")
        .arg(&preflight.root_top_level)
        .arg("merge")
        .arg("--autostash")
        .arg("--no-ff")
        .arg(&preflight.orchestration.branch)
        .arg("-m")
        .arg(format!(
            "merge: sync orchestration before queue {}",
            preflight.ticket_id
        ));
    if let Err(message) =
        run_git_command(merge, "merge orchestration branch into root before Queue")
    {
        let mut abort = Command::new("git");
        abort
            .arg("-C")
            .arg(&preflight.root_top_level)
            .arg("merge")
            .arg("--abort");
        let abort_result = run_git_command(abort, "abort failed orchestration pre-Queue merge");
        let detail = match abort_result {
            Ok(()) => format!(
                "could not merge orchestration branch {} ({}) into root before Queue; merge was aborted: {}",
                preflight.orchestration.branch, orchestration_head, message
            ),
            Err(abort_error) => format!(
                "could not merge orchestration branch {} ({}) into root before Queue, and merge abort failed: {}; original merge error: {}",
                preflight.orchestration.branch, orchestration_head, abort_error, message
            ),
        };
        return Err(queue_check_failed(
            "root-orchestration-merge",
            &preflight.ticket_id,
            &preflight.root_top_level,
            detail,
        ));
    }

    let merge_commit = git_rev_parse(&preflight.root_top_level, "HEAD").map_err(|message| {
        queue_check_failed(
            "root-orchestration-merge",
            &preflight.ticket_id,
            &preflight.root_top_level,
            message,
        )
    })?;
    Ok(PanelQueueRootMerge {
        branch: preflight.orchestration.branch.clone(),
        head: orchestration_head,
        merge_commit: Some(merge_commit),
    })
}

fn commit_panel_queue_ticket_record(
    preflight: &PanelQueueHandoffPreflight,
) -> Result<PanelQueueCommit, TicketActionError> {
    let ticket_rel = path_relative_to_root(
        &preflight.root_top_level,
        &preflight.ticket_record_dir,
        "target-ticket-record",
        &preflight.ticket_id,
    )?;
    let mut add = Command::new("git");
    add.arg("-C")
        .arg(&preflight.root_top_level)
        .arg("add")
        .arg("--")
        .arg(&ticket_rel);
    run_git_command(add, "stage Queue Ticket record").map_err(|message| {
        queue_check_failed(
            "queue-commit-stage",
            &preflight.ticket_id,
            &preflight.ticket_record_dir,
            message,
        )
    })?;

    let ticket_rel_string = git_path_string(&ticket_rel);
    let staged = git_capture(
        &preflight.root_top_level,
        &[
            "diff",
            "--cached",
            "--name-only",
            "--",
            ticket_rel_string.as_str(),
        ],
        "list staged Queue Ticket files",
    )
    .map_err(|message| {
        queue_check_failed(
            "queue-commit-pathscope",
            &preflight.ticket_id,
            &preflight.root_top_level,
            message,
        )
    })?;
    let staged_paths = staged
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>();
    if staged_paths.is_empty() {
        return Err(queue_check_failed(
            "queue-commit-pathscope",
            &preflight.ticket_id,
            &preflight.ticket_record_dir,
            "Queue mutation produced no staged Ticket record changes".to_string(),
        ));
    }
    let message = format!("ticket: queue {}", preflight.ticket_id);
    let mut commit = Command::new("git");
    commit
        .arg("-C")
        .arg(&preflight.root_top_level)
        .arg("commit")
        .arg("--no-verify")
        .arg("-m")
        .arg(message)
        .arg("--")
        .arg(&ticket_rel);
    run_git_command(commit, "commit Queue Ticket record").map_err(|message| {
        queue_check_failed(
            "queue-commit-create",
            &preflight.ticket_id,
            &preflight.root_top_level,
            message,
        )
    })?;
    let sha = git_rev_parse(&preflight.root_top_level, "HEAD").map_err(|message| {
        queue_check_failed(
            "queue-commit-create",
            &preflight.ticket_id,
            &preflight.root_top_level,
            message,
        )
    })?;
    Ok(PanelQueueCommit { sha })
}

fn sync_panel_queue_to_orchestration(
    preflight: &PanelQueueHandoffPreflight,
    commit: &PanelQueueCommit,
) -> Result<PanelQueueSync, TicketActionError> {
    ensure_git_clean(
        "orchestration-clean-before-sync",
        &preflight.ticket_id,
        &preflight.orchestration.path,
    )?;
    let mut merge = Command::new("git");
    merge
        .arg("-C")
        .arg(&preflight.orchestration.path)
        .arg("merge")
        .arg("--ff-only")
        .arg(&commit.sha);
    run_git_command(
        merge,
        "ff-only sync Queue commit into orchestration worktree",
    )
    .map_err(|message| {
        queue_check_failed(
            "orchestration-ff-only-sync",
            &preflight.ticket_id,
            &preflight.orchestration.path,
            message,
        )
    })?;
    let head = git_rev_parse(&preflight.orchestration.path, "HEAD").map_err(|message| {
        queue_check_failed(
            "orchestration-ff-only-sync",
            &preflight.ticket_id,
            &preflight.orchestration.path,
            message,
        )
    })?;
    Ok(PanelQueueSync {
        path: preflight.orchestration.path.clone(),
        branch: preflight.orchestration.branch.clone(),
        head,
    })
}

fn verify_panel_queue_synced(
    preflight: &PanelQueueHandoffPreflight,
    commit: &PanelQueueCommit,
) -> Result<(), TicketActionError> {
    let head = git_rev_parse(&preflight.orchestration.path, "HEAD").map_err(|message| {
        queue_check_failed(
            "orchestration-sync-verify",
            &preflight.ticket_id,
            &preflight.orchestration.path,
            message,
        )
    })?;
    ensure_git_ancestor(&preflight.orchestration.path, &commit.sha, &head).map_err(|message| {
        queue_check_failed(
            "orchestration-sync-verify",
            &preflight.ticket_id,
            &preflight.orchestration.path,
            format!(
                "orchestration HEAD {head} does not contain Queue commit {}: {message}",
                commit.sha
            ),
        )
    })?;
    let config = TicketConfig::load_workspace(&preflight.orchestration.path).map_err(|error| {
        queue_check_failed(
            "orchestration-ticket-state",
            &preflight.ticket_id,
            &preflight.orchestration.path,
            error.to_string(),
        )
    })?;
    let backend = LocalTicketBackend::new(config.backend_root())
        .with_record_language(config.ticket_record_language());
    ensure_ticket_state(
        &backend,
        &preflight.ticket_id,
        TicketWorkflowState::Queued,
        "orchestration-ticket-state",
        &preflight.orchestration.path,
    )
}

fn ensure_ticket_state(
    backend: &LocalTicketBackend,
    ticket_id: &str,
    expected: TicketWorkflowState,
    check: &'static str,
    path: &Path,
) -> Result<(), TicketActionError> {
    let ticket = backend
        .show(TicketIdOrSlug::Id(ticket_id.to_string()))
        .map_err(|error| queue_check_failed(check, ticket_id, path, error.to_string()))?;
    if ticket.meta.workflow_state != expected {
        return Err(queue_check_failed(
            check,
            ticket_id,
            path,
            format!(
                "state is {}, expected {}",
                ticket.meta.workflow_state.as_str(),
                expected.as_str()
            ),
        ));
    }
    Ok(())
}

fn ensure_git_path_clean(
    check: &'static str,
    ticket_id: &str,
    root: &Path,
    path: &Path,
) -> Result<(), TicketActionError> {
    let rel = path_relative_to_root(root, path, check, ticket_id)?;
    let rel_string = git_path_string(&rel);
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .arg("status")
        .arg("--porcelain")
        .arg("--untracked-files=normal")
        .arg("--")
        .arg(&rel)
        .output()
        .map_err(|error| {
            queue_check_failed(
                check,
                ticket_id,
                path,
                format!("git status failed: {error}"),
            )
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(queue_check_failed(
            check,
            ticket_id,
            path,
            format!("git status failed: {}", stderr.trim()),
        ));
    }
    let status = String::from_utf8_lossy(&output.stdout);
    if !status.trim().is_empty() {
        return Err(queue_check_failed(
            check,
            ticket_id,
            path,
            format!(
                "target Ticket record {rel_string} has pre-existing changes; commit, stash, or revert them before Queue:\n{}",
                status.trim_end()
            ),
        ));
    }
    Ok(())
}

fn ensure_git_clean(
    check: &'static str,
    ticket_id: &str,
    path: &Path,
) -> Result<(), TicketActionError> {
    let status = git_status_porcelain(path)
        .map_err(|message| queue_check_failed(check, ticket_id, path, message))?;
    if status.is_empty() {
        return Ok(());
    }
    let detail = status.into_iter().take(6).collect::<Vec<_>>().join("; ");
    Err(queue_check_failed(
        check,
        ticket_id,
        path,
        format!("worktree is dirty: {detail}"),
    ))
}

fn ensure_git_effective_user(path: &Path) -> Result<(), String> {
    let name = git_capture(path, &["config", "user.name"], "read git user.name")?;
    let email = git_capture(path, &["config", "user.email"], "read git user.email")?;
    if name.trim().is_empty() || email.trim().is_empty() {
        return Err("git user.name and user.email must be configured before the Panel creates a Queue commit".to_string());
    }
    Ok(())
}

fn git_rev_parse(path: &Path, rev: &str) -> Result<String, String> {
    git_capture(path, &["rev-parse", rev], "resolve Git revision")
}

fn git_status_porcelain(path: &Path) -> Result<Vec<String>, String> {
    let output = git_capture(
        path,
        &["status", "--porcelain", "--untracked-files=normal"],
        "read Git status",
    )?;
    Ok(output.lines().map(|line| line.to_string()).collect())
}

fn ensure_git_ancestor(path: &Path, ancestor: &str, descendant: &str) -> Result<(), String> {
    let status = Command::new("git")
        .arg("-C")
        .arg(path)
        .arg("merge-base")
        .arg("--is-ancestor")
        .arg(ancestor)
        .arg(descendant)
        .status()
        .map_err(|error| format!("could not run git merge-base --is-ancestor: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "git merge-base --is-ancestor {ancestor} {descendant} exited with {status}"
        ))
    }
}

fn git_capture(path: &Path, args: &[&str], action: &str) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .map_err(|error| {
            format!(
                "could not run git to {action} at {}: {error}",
                path.display()
            )
        })?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).trim().to_string());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let detail = if stderr.is_empty() { stdout } else { stderr };
    Err(format!(
        "git failed to {action} at {}: {detail}",
        path.display()
    ))
}

fn path_relative_to_root(
    root: &Path,
    path: &Path,
    check: &'static str,
    ticket_id: &str,
) -> Result<PathBuf, TicketActionError> {
    let canonical = path.canonicalize().map_err(|error| {
        queue_check_failed(
            check,
            ticket_id,
            path,
            format!("could not canonicalize path: {error}"),
        )
    })?;
    canonical
        .strip_prefix(root)
        .map(PathBuf::from)
        .map_err(|_| {
            queue_check_failed(
                check,
                ticket_id,
                path,
                format!(
                    "path {} is outside root Git top-level {}",
                    canonical.display(),
                    root.display()
                ),
            )
        })
}

fn git_path_string(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn queue_check_failed(
    check: &'static str,
    ticket_id: &str,
    path: &Path,
    message: impl Into<String>,
) -> TicketActionError {
    TicketActionError::Stale(format!(
        "Queue handoff check `{check}` failed for Ticket {ticket_id} at {}: {}",
        path.display(),
        message.into()
    ))
}

fn dispatch_panel_close(
    backend: &LocalTicketBackend,
    ticket_id: &str,
) -> Result<TicketActionOutcome, TicketActionError> {
    let ticket = backend
        .show(TicketIdOrSlug::Id(ticket_id.to_owned()))
        .map_err(|error| TicketActionError::Ticket(error.to_string()))?;
    if let Some(blocker) = panel_close_blocker(&ticket) {
        return Err(TicketActionError::Stale(blocker));
    }

    let resolution = panel_close_resolution(&ticket, backend.record_language());
    backend
        .close(TicketIdOrSlug::Id(ticket_id.to_owned()), resolution)
        .map_err(|error| TicketActionError::Ticket(error.to_string()))?;

    Ok(TicketActionOutcome {
        notice: format!(
            "Closed Ticket {}; deterministic resolution recorded because state was already done.",
            ticket.meta.id
        ),
    })
}

fn panel_close_blocker(ticket: &ticket::Ticket) -> Option<String> {
    let ticket_id = ticket.meta.id.as_str();
    if ticket.meta.workflow_state == TicketWorkflowState::Closed {
        return Some(format!(
            "Close blocked for Ticket {ticket_id}: state is already closed; no close was recorded."
        ));
    }
    if ticket.meta.workflow_state != TicketWorkflowState::Done {
        return Some(format!(
            "Close blocked for Ticket {ticket_id}: state is {}, expected done; no close was recorded.",
            ticket.meta.workflow_state.as_str()
        ));
    }
    if ticket.resolution.is_some() {
        return Some(format!(
            "Close blocked for Ticket {ticket_id}: resolution.md already exists; no close was recorded."
        ));
    }
    None
}

fn panel_close_resolution(
    ticket: &ticket::Ticket,
    record_language: Option<&str>,
) -> ticket::MarkdownText {
    if is_japanese_ticket_record_language(record_language) {
        ticket::MarkdownText::new(format!(
            "Ticket `{}` (`{}`) はすでに `state: done` に到達していたため、workspace Dashboard から close しました。\n\nこの Close action によって、実装作業、state 変更、Orchestrator/Companion launch、worker invocation は開始されていません。\n",
            ticket.meta.id, ticket.meta.title
        ))
    } else {
        ticket::MarkdownText::new(format!(
            "Closed from the workspace Dashboard because Ticket `{}` (`{}`) had already reached `state: done`.\n\nNo implementation work, state change, Orchestrator/Companion launch, or worker invocation was started by this Close action.\n",
            ticket.meta.id, ticket.meta.title
        ))
    }
}

fn is_japanese_ticket_record_language(language: Option<&str>) -> bool {
    let Some(language) = language else {
        return false;
    };
    let language = language.trim();
    language.eq_ignore_ascii_case("japanese")
        || language.eq_ignore_ascii_case("ja")
        || language.eq_ignore_ascii_case("ja-JP")
        || language.contains("日本語")
}

fn orchestrator_queue_notification_message(
    ticket: &crate::workspace_panel::TicketPanelEntry,
) -> String {
    let title = ticket.title.replace(['\r', '\n'], " ");
    format!(
        "Workspace Dashboard Queue for Ticket `{}`, title `{}`: human authorized Orchestrator routing; this is not an unattended scheduler. Read the Ticket and inspect current Orchestrator workspace state. If unblocked, record routing and transition state queued -> inprogress before any worktree/SpawnPod implementation side effects. After inprogress acceptance, use worktree-workflow for `.worktree/<task-name>` creation with tracked `.yoi` project records visible and `.yoi/memory` plus local/runtime/log/lock/secret-like `.yoi` paths excluded, then use multi-agent-workflow to run sibling coder/reviewer Pods (coder narrow child-worktree write scope, reviewer read-only by default). After reviewer approval and blocker resolution, integrate the implementation branch into the orchestration branch automatically, validate in the Orchestrator worktree, record the outcome, and clean up only child implementation worktrees/branches. Do not read, write, validate, merge, clean up, or run git operations in the root/original workspace. If blocked, record a concise reason and leave the Ticket queued or return it to planning with the missing-information reason.",
        ticket.id,
        title.trim()
    )
}

async fn notify_workspace_orchestrator(
    target: Option<OrchestratorNotifyTarget>,
    ticket: &crate::workspace_panel::TicketPanelEntry,
) -> OrchestratorNotificationOutcome {
    let Some(target) = target else {
        return OrchestratorNotificationOutcome::Skipped(
            "no live reachable Orchestrator socket is available".to_string(),
        );
    };
    let message = orchestrator_queue_notification_message(ticket);
    match send_notify_only(&target.socket_path, message, true).await {
        Ok(()) => OrchestratorNotificationOutcome::Sent {
            pod_name: target.pod_name,
        },
        Err(error) => OrchestratorNotificationOutcome::Warning(format!(
            "{} at {}: {}",
            target.pod_name,
            target.socket_path.display(),
            error
        )),
    }
}

async fn send_notify_only(
    socket: &Path,
    message: String,
    auto_run: bool,
) -> Result<(), NotifySendError> {
    let stream = tokio::time::timeout(SOCKET_OP_TIMEOUT, UnixStream::connect(socket))
        .await
        .map_err(|_| NotifySendError::Io("connect timed out".into()))?
        .map_err(|e| NotifySendError::Io(format!("connect: {e}")))?;
    let (reader, writer) = stream.into_split();
    let mut reader = JsonLineReader::new(reader);
    let mut writer = JsonLineWriter::new(writer);

    loop {
        let event = tokio::time::timeout(SOCKET_OP_TIMEOUT, reader.next::<Event>())
            .await
            .map_err(|_| NotifySendError::Io("read initial Snapshot timed out".into()))?
            .map_err(|e| NotifySendError::Io(format!("read initial Snapshot: {e}")))?;
        match event {
            Some(Event::Snapshot { .. }) => break,
            Some(Event::Alert(_)) => continue,
            Some(Event::Error { code, message }) => {
                return Err(NotifySendError::Rejected { code, message });
            }
            Some(_) => continue,
            None => {
                return Err(NotifySendError::Io(
                    "connection closed before initial Snapshot".into(),
                ));
            }
        }
    }

    tokio::time::timeout(
        SOCKET_OP_TIMEOUT,
        writer.write(&Method::Notify { message, auto_run }),
    )
    .await
    .map_err(|_| NotifySendError::Io("write timed out".into()))?
    .map_err(|e| NotifySendError::Io(format!("write: {e}")))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NotifySendError {
    Rejected { code: ErrorCode, message: String },
    Io(String),
}

impl std::fmt::Display for NotifySendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rejected { code, message } => {
                write!(f, "target rejected method ({code:?}): {message}")
            }
            Self::Io(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for NotifySendError {}

fn segments_are_blank(segments: &[Segment]) -> bool {
    segments.iter().all(|segment| match segment {
        Segment::Text { content } => content.trim().is_empty(),
        _ => false,
    })
}

fn open_disabled_reason(entry: &PodListEntry) -> String {
    if let Some(live) = entry.live.as_ref() {
        if !live.reachable {
            return "Selected live Pod is unreachable.".to_string();
        }
        return match live.status {
            Some(PodStatus::Running) => {
                "Selected Pod is running; Enter opens/attaches for inspection.".to_string()
            }
            Some(PodStatus::Paused) => {
                "Selected Pod is paused; open it explicitly to resume or start a new turn."
                    .to_string()
            }
            Some(PodStatus::Idle) => "Selected Pod can be opened/attached.".to_string(),
            None => "Selected Pod did not report a live status.".to_string(),
        };
    }
    if entry.stored.is_some() {
        return "Selected Pod is stopped; Enter restores/opens for inspection.".to_string();
    }
    entry
        .actions
        .disabled_reason
        .clone()
        .unwrap_or_else(|| "Selected Pod cannot be opened from this row.".to_string())
}

fn selected_ticket_notice(row: Option<&PanelRow>) -> String {
    match row {
        Some(row) if row.is_ticket_action() => {
            let action = row.next_action.map(NextUserAction::label).unwrap_or("View");
            format!(
                "Enter dispatches {action} for Ticket '{}' after re-checking current Ticket authority.",
                row.title
            )
        }
        Some(row) if row.kind == PanelRowKind::TicketIntakePod => row
            .disabled_reason
            .clone()
            .or_else(|| row.key_hint.clone())
            .unwrap_or_else(|| {
                "Open/attach this Ticket's Intake Pod from the associated row.".to_string()
            }),
        Some(row) if row.kind == PanelRowKind::InvalidTicket => row
            .disabled_reason
            .clone()
            .or_else(|| row.key_hint.clone())
            .unwrap_or_else(|| "Invalid Ticket record placeholder has no actions.".to_string()),
        _ => "No Pod is selected.".to_string(),
    }
}

fn row_status_label(entry: &PodListEntry) -> (&'static str, Style) {
    if let Some(live) = entry.live.as_ref() {
        if !live.reachable {
            return (
                "unreachable",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            );
        }
        return match live.status {
            Some(PodStatus::Idle) => (
                "live idle",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            Some(PodStatus::Running) => (
                "live running",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Some(PodStatus::Paused) => (
                "live paused",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            None => ("live", Style::default().fg(Color::DarkGray)),
        };
    }
    if entry
        .stored
        .as_ref()
        .is_some_and(|stored| matches!(stored.metadata_state, StoredMetadataState::Corrupt(_)))
    {
        return (
            "corrupt",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        );
    }
    ("stopped/restorable", Style::default().fg(Color::Yellow))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DashboardSectionKind {
    Pending,
    Working,
    Closed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DashboardSection {
    kind: DashboardSectionKind,
    entries: Vec<usize>,
}

impl DashboardSection {
    fn hidden_count(&self) -> usize {
        self.entries
            .len()
            .saturating_sub(visible_section_len(self.kind, self.entries.len()))
    }
}

fn classify_entry(entry: &PodListEntry) -> DashboardSectionKind {
    if entry.live.is_some() {
        if entry.actions.can_send_now {
            DashboardSectionKind::Pending
        } else {
            DashboardSectionKind::Working
        }
    } else {
        DashboardSectionKind::Closed
    }
}

fn sectioned_entries(list: &PodList) -> Vec<DashboardSection> {
    let mut pending = DashboardSection {
        kind: DashboardSectionKind::Pending,
        entries: Vec::new(),
    };
    let mut working = DashboardSection {
        kind: DashboardSectionKind::Working,
        entries: Vec::new(),
    };
    let mut closed = DashboardSection {
        kind: DashboardSectionKind::Closed,
        entries: Vec::new(),
    };

    for (index, entry) in list.entries.iter().enumerate() {
        match classify_entry(entry) {
            DashboardSectionKind::Pending => pending.entries.push(index),
            DashboardSectionKind::Working => working.entries.push(index),
            DashboardSectionKind::Closed => closed.entries.push(index),
        }
    }

    vec![pending, working, closed]
}

fn visible_entry_indices(list: &PodList) -> Vec<usize> {
    sectioned_entries(list)
        .into_iter()
        .flat_map(|section| visible_section_indices(&section))
        .collect()
}

fn visible_panel_keys(panel: &WorkspacePanelViewModel, list: &PodList) -> Vec<PanelRowKey> {
    let mut keys = panel
        .rows
        .iter()
        .filter(|row| row.is_ticket_section_row())
        .map(|row| row.key.clone())
        .collect::<Vec<_>>();
    keys.extend(
        visible_entry_indices(list)
            .into_iter()
            .filter_map(|index| list.entries.get(index))
            .map(|entry| PanelRowKey::Pod(entry.name.clone())),
    );
    keys
}

fn visible_section_indices(section: &DashboardSection) -> Vec<usize> {
    section
        .entries
        .iter()
        .copied()
        .take(visible_section_len(section.kind, section.entries.len()))
        .collect()
}

fn visible_section_len(kind: DashboardSectionKind, len: usize) -> usize {
    match kind {
        DashboardSectionKind::Pending | DashboardSectionKind::Working => len,
        DashboardSectionKind::Closed => len.min(CLOSED_VISIBLE_ROWS),
    }
}

fn section_header_line(
    kind: DashboardSectionKind,
    total: usize,
    hidden: usize,
    width: u16,
) -> Line<'static> {
    let label = match kind {
        DashboardSectionKind::Pending => "pending",
        DashboardSectionKind::Working => "working",
        DashboardSectionKind::Closed => "closed",
    };
    let detail = if hidden > 0 {
        format!(" {total} total, +{hidden} hidden")
    } else {
        String::new()
    };
    let text = render::truncate_with_ellipsis(&format!("--{label}{detail}---"), width as usize);
    Line::from(Span::styled(
        text,
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    ))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DashboardLayoutState {
    title: Rect,
    list: Rect,
    boundary: Rect,
    target_status: Rect,
    input: Rect,
    actionbar: Rect,
    list_draws_own_separator: bool,
}

fn dashboard_layout(area: Rect, input_height: u16) -> DashboardLayoutState {
    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(input_height),
        Constraint::Length(1),
    ])
    .split(area);

    DashboardLayoutState {
        title: chunks[0],
        list: chunks[1],
        boundary: chunks[2],
        target_status: chunks[3],
        input: chunks[4],
        actionbar: chunks[5],
        list_draws_own_separator: false,
    }
}

// Rendering and layout composition live in dashboard/render.rs.
#[cfg(test)]
mod tests;
