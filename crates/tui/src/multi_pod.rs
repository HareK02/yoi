use std::collections::BTreeSet;
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
use ticket::config::TicketConfig;
use ticket::{LocalTicketBackend, TicketBackend, TicketIdOrSlug, TicketWorkflowState};
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
use crate::workspace_panel::{
    ActionPriority, CompanionLifecyclePlan, CompanionPanelState, CompanionPanelStatus,
    CompanionPodPresence, ComposerTarget, NextUserAction, OrchestratorLifecyclePlan,
    OrchestratorPanelState, OrchestratorPanelStatus, OrchestratorPodPresence, PanelRow,
    PanelRowKey, TicketConfigAvailability, TicketLocalClaimStatus, WorkspacePanelViewModel,
    bounded_panel_diagnostic, build_current_ticket_row, build_workspace_panel,
    companion_pod_presence, decide_companion_lifecycle, decide_orchestrator_lifecycle,
    local_claim_status_for_pod, orchestrator_pod_presence, ticket_config_availability,
    workspace_companion_pod_name, workspace_orchestrator_pod_name,
};

const MAX_ENTRIES: usize = 50;
const CLOSED_VISIBLE_ROWS: usize = 3;
const ORCHESTRATOR_IDLE_QUEUE_NOTICE_TEMPLATE: &str =
    include_str!("../../../resources/prompts/panel/orchestrator_idle_queue_notice.md");
const ORCHESTRATOR_QUEUE_ATTENTION_MAX_TICKETS: usize = 6;
const ORCHESTRATOR_QUEUE_ATTENTION_MAX_TEXT_CHARS: usize = 120;
const ORCHESTRATOR_QUEUE_ATTENTION_MAX_MESSAGE_CHARS: usize = 2_400;
const SOCKET_OP_TIMEOUT: Duration = Duration::from_secs(3);
const MULTI_POD_POLL_INTERVAL: Duration = Duration::from_millis(1_500);
const TERMINAL_EVENT_POLL_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Debug)]
pub(crate) enum MultiPodError {
    Io(io::Error),
    Store(session_store::StoreError),
    NoPods,
}

impl std::fmt::Display for MultiPodError {
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

impl std::error::Error for MultiPodError {}

impl From<io::Error> for MultiPodError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<session_store::StoreError> for MultiPodError {
    fn from(e: session_store::StoreError) -> Self {
        Self::Store(e)
    }
}

pub(crate) enum MultiPodOutcome {
    Quit,
    Open(OpenPodRequest),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OpenPodRequest {
    pub(crate) pod_name: String,
    pub(crate) socket_override: Option<PathBuf>,
}

pub(crate) async fn load_app(
    runtime_command: PodRuntimeCommand,
) -> Result<MultiPodApp, MultiPodError> {
    Ok(MultiPodApp::loading(runtime_command))
}

pub(crate) async fn run(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut MultiPodApp,
) -> Result<MultiPodOutcome, MultiPodError> {
    if app.panel.rows.is_empty()
        && app.panel.header.diagnostics.is_empty()
        && app.enter_reload.is_none()
    {
        return Err(MultiPodError::NoPods);
    }

    let mut pending_reload = PendingReload::default();
    let mut pending_queue_attention_notice = PendingQueueAttentionNotice::default();
    if let Some(mode) = app.enter_reload.take() {
        if pending_reload.start(mode) {
            app.refreshing = true;
        }
    }
    let mut next_poll = Instant::now() + MULTI_POD_POLL_INTERVAL;
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

        terminal.draw(|f| draw(f, app))?;
        if !emitted_panel_ready {
            crate::e2e_observer::emit("panel", "panel_ready", serde_json::json!({}));
            emitted_panel_ready = true;
        }
        app.emit_rows_rendered();

        let now = Instant::now();
        if now >= next_poll {
            pending_reload.start(OrchestratorLifecycleMode::Observe);
            next_poll = now + MULTI_POD_POLL_INTERVAL;
            continue;
        }

        let event_wait = TERMINAL_EVENT_POLL_INTERVAL.min(next_poll.saturating_duration_since(now));
        if !poll(event_wait)? {
            continue;
        }

        match read()? {
            TermEvent::Key(key) => match app.handle_key(key) {
                MultiPodAction::None => {}
                MultiPodAction::Quit => {
                    crate::e2e_observer::emit("panel", "quit_requested", serde_json::json!({}));
                    abort_panel_background_work_for_quit(
                        &mut pending_reload,
                        &mut pending_queue_attention_notice,
                    );
                    return Ok(MultiPodOutcome::Quit);
                }
                MultiPodAction::Open => {
                    crate::e2e_observer::emit(
                        "panel",
                        "action_requested",
                        serde_json::json!({ "action": "open" }),
                    );
                    if let Some(request) = app.prepare_open() {
                        terminal.draw(|f| draw(f, app))?;
                        return Ok(MultiPodOutcome::Open(request));
                    }
                }
                MultiPodAction::DispatchTicketAction(request) => {
                    crate::e2e_observer::emit(
                        "panel",
                        "action_requested",
                        serde_json::json!({ "action": "ticket_action" }),
                    );
                    pending_reload.abort();
                    pending_queue_attention_notice.abort();
                    terminal.draw(|f| draw(f, app))?;
                    let result = dispatch_ticket_action(request).await;
                    app.finish_ticket_action_dispatch(result);
                    if pending_reload.start(OrchestratorLifecycleMode::Observe) {
                        app.refreshing = true;
                    }
                    next_poll = Instant::now() + MULTI_POD_POLL_INTERVAL;
                }
                MultiPodAction::LaunchIntake(request) => {
                    crate::e2e_observer::emit(
                        "panel",
                        "action_requested",
                        serde_json::json!({ "action": "launch_intake" }),
                    );
                    pending_reload.abort();
                    pending_queue_attention_notice.abort();
                    terminal.draw(|f| draw(f, app))?;
                    let result = launch_intake_with_handoff(request).await;
                    app.finish_intake_launch(result);
                    if pending_reload.start(OrchestratorLifecycleMode::Observe) {
                        app.refreshing = true;
                    }
                    next_poll = Instant::now() + MULTI_POD_POLL_INTERVAL;
                }
                MultiPodAction::SendCompanion(request) => {
                    crate::e2e_observer::emit(
                        "panel",
                        "action_requested",
                        serde_json::json!({ "action": "send_companion" }),
                    );
                    pending_reload.abort();
                    pending_queue_attention_notice.abort();
                    terminal.draw(|f| draw(f, app))?;
                    let result = dispatch_companion_message(request).await;
                    app.finish_companion_send(result);
                    if pending_reload.start(OrchestratorLifecycleMode::Observe) {
                        app.refreshing = true;
                    }
                    next_poll = Instant::now() + MULTI_POD_POLL_INTERVAL;
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
    handle: Option<tokio::task::JoinHandle<Result<MultiPodSnapshot, MultiPodError>>>,
}

impl PendingReload {
    fn start(&mut self, lifecycle_mode: OrchestratorLifecycleMode) -> bool {
        if self.handle.is_some() {
            return false;
        }
        crate::e2e_observer::emit(
            "panel",
            "background_task_started",
            serde_json::json!({
                "task": "reload",
                "lifecycle_mode": format!("{lifecycle_mode:?}"),
            }),
        );
        self.handle = Some(tokio::spawn(async move {
            crate::e2e_observer::hold_background_task_if_requested("reload").await;
            load_multi_pod_snapshot(None, lifecycle_mode).await
        }));
        true
    }

    #[cfg(test)]
    fn start_with_handle(
        &mut self,
        handle: tokio::task::JoinHandle<Result<MultiPodSnapshot, MultiPodError>>,
    ) -> bool {
        if self.handle.is_some() {
            handle.abort();
            return false;
        }
        self.handle = Some(handle);
        true
    }

    async fn finish_if_ready(&mut self) -> Option<Result<MultiPodSnapshot, MultiPodError>> {
        if !self.handle.as_ref()?.is_finished() {
            return None;
        }
        let handle = self.handle.take()?;
        crate::e2e_observer::emit(
            "panel",
            "background_task_finished",
            serde_json::json!({ "task": "reload" }),
        );
        Some(match handle.await {
            Ok(result) => result,
            Err(e) => Err(MultiPodError::Io(io::Error::other(format!(
                "reload task failed: {e}"
            )))),
        })
    }

    fn abort(&mut self) {
        if let Some(handle) = self.handle.take() {
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

fn default_store_dir() -> Result<PathBuf, MultiPodError> {
    manifest::paths::sessions_dir().ok_or_else(|| {
        MultiPodError::Io(io::Error::new(
            io::ErrorKind::NotFound,
            "could not resolve sessions directory",
        ))
    })
}

fn default_pod_store_dir() -> Result<PathBuf, MultiPodError> {
    manifest::paths::data_dir()
        .map(|dir| dir.join("pods"))
        .ok_or_else(|| {
            MultiPodError::Io(io::Error::new(
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
    let registry_warning = commit_intake_registry_update(request.registry_update);
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

fn commit_intake_registry_update(update: IntakeRegistryUpdate) -> Option<String> {
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

#[derive(Debug, Serialize)]
struct PanelE2eRowKey {
    kind: &'static str,
    id: String,
}

#[derive(Debug, Serialize)]
struct PanelE2eRect {
    x: u16,
    y: u16,
    width: u16,
    height: u16,
}

#[derive(Debug, Serialize)]
struct PanelE2eRenderedRow {
    key: PanelE2eRowKey,
    title: String,
    status: Option<String>,
    action: Option<&'static str>,
    rect: PanelE2eRect,
}

#[derive(Debug, Serialize)]
struct PanelE2eRowsRendered {
    selected: Option<PanelE2eRowKey>,
    rows: Vec<PanelE2eRenderedRow>,
}

fn panel_e2e_row_key(key: &PanelRowKey) -> PanelE2eRowKey {
    match key {
        PanelRowKey::Ticket(id) => PanelE2eRowKey {
            kind: "ticket",
            id: id.clone(),
        },
        PanelRowKey::Pod(name) => PanelE2eRowKey {
            kind: "pod",
            id: name.clone(),
        },
    }
}

fn panel_e2e_rect(rect: Rect) -> PanelE2eRect {
    PanelE2eRect {
        x: rect.x,
        y: rect.y,
        width: rect.width,
        height: rect.height,
    }
}

pub(crate) struct MultiPodApp {
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
}

impl MultiPodApp {
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
        }
    }

    fn apply_reload_result(&mut self, result: Result<MultiPodSnapshot, MultiPodError>) {
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
        self.apply_reloaded_snapshot(MultiPodSnapshot { list, panel });
    }

    fn apply_reloaded_snapshot(&mut self, mut snapshot: MultiPodSnapshot) {
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
        match self.selected_row.as_ref() {
            Some(PanelRowKey::Pod(name)) => {
                self.list.entries.iter().find(|entry| &entry.name == name)
            }
            _ => None,
        }
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
        let entry = self.selected_pod_entry()?;
        if entry.actions.can_open {
            return None;
        }
        Some(open_disabled_reason(entry))
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
        if !matches!(event.kind, MouseEventKind::Down(MouseButton::Left)) {
            return false;
        }
        if self.panel_diagnostic_open {
            return false;
        }
        let Some(key) = self
            .row_hit_boxes
            .iter()
            .find(|hit| hit.contains(event.column, event.row))
            .map(|hit| hit.key.clone())
        else {
            return false;
        };
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

    fn emit_rows_rendered(&self) {
        let rows = self
            .row_hit_boxes
            .iter()
            .map(|hit| {
                let panel_row = self.panel.row(&hit.key);
                let (title, status, action) = match panel_row {
                    Some(row) => (
                        row.title.clone(),
                        Some(row.status.clone()),
                        row.next_action.map(NextUserAction::label),
                    ),
                    None => match &hit.key {
                        PanelRowKey::Pod(name) => (name.clone(), None, None),
                        PanelRowKey::Ticket(id) => (id.clone(), None, None),
                    },
                };
                PanelE2eRenderedRow {
                    key: panel_e2e_row_key(&hit.key),
                    title,
                    status,
                    action,
                    rect: panel_e2e_rect(hit.rect),
                }
            })
            .collect();
        crate::e2e_observer::emit(
            "panel",
            "rows_rendered",
            PanelE2eRowsRendered {
                selected: self.selected_row.as_ref().map(panel_e2e_row_key),
                rows,
            },
        );
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
                    PanelRowKey::Ticket(_) => true,
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
        let selected_key = key.clone();
        self.selected_row = Some(key);
        crate::e2e_observer::emit(
            "panel",
            "selection_changed",
            serde_json::json!({ "selected": panel_e2e_row_key(&selected_key) }),
        );
    }

    fn clear_panel_selection(&mut self) {
        self.selected_row = None;
        self.list.selected_name = None;
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

    fn handle_key(&mut self, key: KeyEvent) -> MultiPodAction {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if self.panel_diagnostic_open {
            return match key.code {
                KeyCode::Esc | KeyCode::F(2) => {
                    self.panel_diagnostic_open = false;
                    MultiPodAction::None
                }
                KeyCode::Char('d') if ctrl => MultiPodAction::Quit,
                KeyCode::Char('c') if ctrl => MultiPodAction::Quit,
                _ => MultiPodAction::None,
            };
        }

        match key.code {
            KeyCode::F(2) => {
                if self.panel_diagnostic.is_some() {
                    self.panel_diagnostic_open = true;
                } else {
                    self.notice = Some("No Panel diagnostic details yet".to_string());
                }
                MultiPodAction::None
            }
            KeyCode::Char('d') if ctrl => MultiPodAction::Quit,
            KeyCode::Char('c') if ctrl => MultiPodAction::Quit,
            KeyCode::Esc => {
                self.clear_panel_selection();
                self.notice = Some(
                    "Row selection cleared; composer draft and target are unchanged.".to_string(),
                );
                MultiPodAction::None
            }
            KeyCode::Tab => {
                // Completion owns Tab before panel target switching when a
                // completion popup exists. The workspace panel currently has
                // no completion source, so this is the target switch path.
                self.cycle_composer_target();
                MultiPodAction::None
            }
            KeyCode::Up if self.composer_is_blank() => {
                self.select_prev();
                MultiPodAction::None
            }
            KeyCode::Down if self.composer_is_blank() => {
                self.select_next();
                MultiPodAction::None
            }
            KeyCode::Enter
                if self.composer_is_blank()
                    && self.selected_ticket_action() == Some(NextUserAction::Clarify) =>
            {
                self.prepare_existing_ticket_intake_launch()
                    .map(MultiPodAction::LaunchIntake)
                    .unwrap_or(MultiPodAction::None)
            }
            KeyCode::Enter
                if self.composer_is_blank() && self.selected_ticket_action().is_some() =>
            {
                self.prepare_ticket_action_dispatch()
                    .map(MultiPodAction::DispatchTicketAction)
                    .unwrap_or(MultiPodAction::None)
            }
            KeyCode::Enter if self.composer_is_blank() => MultiPodAction::Open,
            KeyCode::Enter if self.composer_target == ComposerTarget::TicketIntake => self
                .prepare_intake_launch()
                .map(MultiPodAction::LaunchIntake)
                .unwrap_or(MultiPodAction::None),
            KeyCode::Enter => self
                .prepare_companion_send()
                .map(MultiPodAction::SendCompanion)
                .unwrap_or(MultiPodAction::None),
            _ if composer_edit_action(key).is_some() => {
                self.apply_composer_edit_action(composer_edit_action(key).expect("checked above"));
                MultiPodAction::None
            }
            _ => MultiPodAction::None,
        }
    }
}

enum MultiPodAction {
    None,
    Quit,
    Open,
    DispatchTicketAction(TicketActionRequest),
    LaunchIntake(IntakeLaunchRequest),
    SendCompanion(CompanionSendRequest),
}

#[derive(Debug, Clone)]
struct MultiPodSnapshot {
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

async fn load_multi_pod_snapshot(
    selected_name: Option<String>,
    lifecycle_mode: OrchestratorLifecycleMode,
) -> Result<MultiPodSnapshot, MultiPodError> {
    let workspace_root = current_workspace_root();
    let companion_pod_name = workspace_companion_pod_name(&workspace_root);
    let list_selected_name = selected_name
        .clone()
        .or_else(|| Some(companion_pod_name.clone()));
    let mut list = load_pod_list(list_selected_name.clone(), MAX_ENTRIES).await?;
    let companion_presence = load_exact_companion_pod_presence(&companion_pod_name).await?;
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
    if companion.reload_pods {
        list = load_pod_list(list_selected_name.clone(), MAX_ENTRIES).await?;
    }
    let config = ticket_config_availability(&workspace_root);
    let orchestrator_pod_name = workspace_orchestrator_pod_name(&workspace_root);
    let orchestrator_presence = match &config {
        TicketConfigAvailability::Absent | TicketConfigAvailability::Unusable(_) => None,
        TicketConfigAvailability::Usable => {
            Some(load_exact_pod_presence(&orchestrator_pod_name).await?)
        }
    };
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
    if orchestrator.reload_pods {
        list = load_pod_list(list_selected_name, MAX_ENTRIES).await?;
    }
    let mut panel = build_workspace_panel(&workspace_root, &list);
    panel.header.companion = companion.state;
    panel.header.diagnostics.extend(companion.diagnostics);
    panel.header.orchestrator = orchestrator.state;
    panel.header.diagnostics.extend(orchestrator.diagnostics);
    Ok(MultiPodSnapshot { list, panel })
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

fn orchestration_worktree_layout(workspace_root: &Path) -> OrchestrationWorktreeLayout {
    let stem = workspace_orchestrator_pod_name(workspace_root);
    OrchestrationWorktreeLayout {
        path: workspace_root
            .join(".worktree")
            .join("orchestration")
            .join(&stem),
        branch: format!("orchestration/{stem}"),
    }
}

fn build_orchestrator_launch_context(
    original_workspace_root: &Path,
    orchestration_workspace_root: &Path,
    pod_name: &str,
) -> TicketRoleLaunchContext {
    let mut context = TicketRoleLaunchContext::new(
        original_workspace_root.to_path_buf(),
        TicketRole::Orchestrator,
    )
    .with_cwd(orchestration_workspace_root.to_path_buf())
    .with_original_workspace_root(original_workspace_root.to_path_buf())
    .with_target_workspace_root(original_workspace_root.to_path_buf());
    context.pod_name = Some(pod_name.to_string());
    context.user_instruction = Some(
        "Workspace panel opened for this Ticket-enabled workspace. Coordinate Ticket routing and wait for explicit follow-up before spawning role Pods."
            .to_string(),
    );
    context
}

fn ensure_orchestration_worktree(
    workspace_root: &Path,
) -> Result<OrchestrationWorktreeReady, String> {
    let layout = orchestration_worktree_layout(workspace_root);
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
    let layout = orchestration_worktree_layout(workspace_root);
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
                                    "restored existing Pod state in orchestration worktree {}",
                                    worktree.layout.path.display()
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

async fn load_exact_companion_pod_presence(
    pod_name: &str,
) -> Result<CompanionPodPresence, MultiPodError> {
    let list = load_pod_list(Some(pod_name.to_string()), usize::MAX).await?;
    Ok(companion_pod_presence(pod_name, &list))
}

async fn load_exact_pod_presence(pod_name: &str) -> Result<OrchestratorPodPresence, MultiPodError> {
    let list = load_pod_list(Some(pod_name.to_string()), usize::MAX).await?;
    Ok(orchestrator_pod_presence(pod_name, &list))
}

async fn load_pod_list(
    selected_name: Option<String>,
    max_entries: usize,
) -> Result<PodList, MultiPodError> {
    let store_dir = default_store_dir()?;
    let store = FsStore::new(&store_dir)?;
    let pod_store = FsPodStore::new(default_pod_store_dir()?).map_err(io::Error::other)?;
    let stored = read_stored_pod_infos(&store, &pod_store)?;
    let live = read_reachable_live_pod_infos(&store)
        .await
        .unwrap_or_default();
    Ok(PodList::from_sources(
        PodVisibilitySource::ResumePicker,
        stored,
        live,
        selected_name,
        max_entries,
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
            if ticket.workflow_state == TicketWorkflowState::InProgress {
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
                "Ticket config is absent; workspace panel no longer exposes Ticket actions"
                    .to_string(),
            ));
        }
        TicketConfigAvailability::Unusable(message) => {
            return Err(TicketActionError::Stale(format!(
                "Ticket config is unusable; workspace panel no longer exposes Ticket actions: {message}"
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
        NextUserAction::Clarify
        | NextUserAction::Edit
        | NextUserAction::OpenPod
        | NextUserAction::Wait => Ok(TicketActionOutcome {
            notice: format!(
                "{} for Ticket {} has no safe inline workspace-panel dispatch; use the Ticket workflow.",
                request.action.label(),
                current_ticket.id
            ),
        }),
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

    let orchestration = orchestration_worktree_layout(&root_top_level);
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
            "Ticket `{}` (`{}`) はすでに `state: done` に到達していたため、workspace Panel から close しました。\n\nこの Close action によって、実装作業、state 変更、Orchestrator/Companion launch、worker invocation は開始されていません。\n",
            ticket.meta.id, ticket.meta.title
        ))
    } else {
        ticket::MarkdownText::new(format!(
            "Closed from the workspace Panel because Ticket `{}` (`{}`) had already reached `state: done`.\n\nNo implementation work, state change, Orchestrator/Companion launch, or worker invocation was started by this Close action.\n",
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
        "Workspace panel Queue for Ticket `{}`, title `{}`: human authorized Orchestrator routing; this is not an unattended scheduler. Read the Ticket and inspect current Orchestrator workspace state. If unblocked, record routing and transition state queued -> inprogress before any worktree/SpawnPod implementation side effects. After inprogress acceptance, use worktree-workflow for `.worktree/<task-name>` creation with tracked `.yoi` project records visible and `.yoi/memory` plus local/runtime/log/lock/secret-like `.yoi` paths excluded, then use multi-agent-workflow to run sibling coder/reviewer Pods (coder narrow child-worktree write scope, reviewer read-only by default). After reviewer approval and blocker resolution, integrate the implementation branch into the orchestration branch automatically, validate in the Orchestrator worktree, record the outcome, and clean up only child implementation worktrees/branches. Do not read, write, validate, merge, clean up, or run git operations in the root/original workspace. If blocked, record a concise reason and leave the Ticket queued or return it to planning with the missing-information reason.",
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
enum MultiPodSectionKind {
    Pending,
    Working,
    Closed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MultiPodSection {
    kind: MultiPodSectionKind,
    entries: Vec<usize>,
}

impl MultiPodSection {
    fn hidden_count(&self) -> usize {
        self.entries
            .len()
            .saturating_sub(visible_section_len(self.kind, self.entries.len()))
    }
}

fn classify_entry(entry: &PodListEntry) -> MultiPodSectionKind {
    if entry.live.is_some() {
        if entry.actions.can_send_now {
            MultiPodSectionKind::Pending
        } else {
            MultiPodSectionKind::Working
        }
    } else {
        MultiPodSectionKind::Closed
    }
}

fn sectioned_entries(list: &PodList) -> Vec<MultiPodSection> {
    let mut pending = MultiPodSection {
        kind: MultiPodSectionKind::Pending,
        entries: Vec::new(),
    };
    let mut working = MultiPodSection {
        kind: MultiPodSectionKind::Working,
        entries: Vec::new(),
    };
    let mut closed = MultiPodSection {
        kind: MultiPodSectionKind::Closed,
        entries: Vec::new(),
    };

    for (index, entry) in list.entries.iter().enumerate() {
        match classify_entry(entry) {
            MultiPodSectionKind::Pending => pending.entries.push(index),
            MultiPodSectionKind::Working => working.entries.push(index),
            MultiPodSectionKind::Closed => closed.entries.push(index),
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
        .filter(|row| row.is_ticket_action())
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

fn visible_section_indices(section: &MultiPodSection) -> Vec<usize> {
    section
        .entries
        .iter()
        .copied()
        .take(visible_section_len(section.kind, section.entries.len()))
        .collect()
}

fn visible_section_len(kind: MultiPodSectionKind, len: usize) -> usize {
    match kind {
        MultiPodSectionKind::Pending | MultiPodSectionKind::Working => len,
        MultiPodSectionKind::Closed => len.min(CLOSED_VISIBLE_ROWS),
    }
}

fn section_header_line(
    kind: MultiPodSectionKind,
    total: usize,
    hidden: usize,
    width: u16,
) -> Line<'static> {
    let label = match kind {
        MultiPodSectionKind::Pending => "pending",
        MultiPodSectionKind::Working => "working",
        MultiPodSectionKind::Closed => "closed",
    };
    let detail = if hidden > 0 {
        format!(" {total} total, +{hidden} hidden")
    } else {
        String::new()
    };
    let text = truncate_with_ellipsis(&format!("--{label}{detail}---"), width as usize);
    Line::from(Span::styled(
        text,
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    ))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MultiPodLayoutState {
    title: Rect,
    list: Rect,
    boundary: Rect,
    target_status: Rect,
    input: Rect,
    actionbar: Rect,
    list_draws_own_separator: bool,
}

fn multi_pod_layout(area: Rect, input_height: u16) -> MultiPodLayoutState {
    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(input_height),
        Constraint::Length(1),
    ])
    .split(area);

    MultiPodLayoutState {
        title: chunks[0],
        list: chunks[1],
        boundary: chunks[2],
        target_status: chunks[3],
        input: chunks[4],
        actionbar: chunks[5],
        list_draws_own_separator: false,
    }
}

fn draw(frame: &mut Frame<'_>, app: &mut MultiPodApp) {
    let area = frame.area();
    let input_content_width = area.width.saturating_sub(2).max(1);
    let mut input_render = app.input.render(input_content_width);
    let input_height = input_area_height(&input_render, area.height);
    app.input
        .apply_cursor_viewport(&mut input_render, input_height);
    let layout = multi_pod_layout(area, input_height);

    draw_title(frame, app, layout.title);
    draw_list(frame, app, layout.list);
    draw_separator(frame, layout.boundary);
    draw_target_status(frame, app, layout.target_status);
    draw_input(frame, &input_render, layout.input);
    draw_actionbar(frame, app, layout.actionbar);
    if app.panel_diagnostic_open {
        render_panel_diagnostic(frame, app, area);
    }
}

fn panel_diagnostic_area(area: Rect) -> Rect {
    let width = if area.width <= 20 {
        area.width
    } else {
        area.width.saturating_sub(4).min(100).max(20)
    };
    let height = if area.height <= 8 {
        area.height
    } else {
        area.height.saturating_sub(4).min(24).max(8)
    };
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    Rect::new(x, y, width, height)
}

fn render_panel_diagnostic(frame: &mut Frame<'_>, app: &MultiPodApp, area: Rect) {
    let Some(diagnostic) = app.panel_diagnostic.as_ref() else {
        return;
    };
    let popup_area = panel_diagnostic_area(area);
    let title = format!(" {} ", diagnostic.title);
    let text = format!("{}\n\nF2/Esc: close", diagnostic.details);
    let paragraph = Paragraph::new(text)
        .block(Block::default().title(title).borders(Borders::ALL))
        .wrap(Wrap { trim: false });
    frame.render_widget(Clear, popup_area);
    frame.render_widget(paragraph, popup_area);
}

fn input_area_height(render: &crate::input::InputRender, terminal_height: u16) -> u16 {
    let needed = render.lines.len().max(1) as u16;
    let cap = (terminal_height / 3).max(1).min(10);
    needed.clamp(1, cap)
}

fn draw_title(frame: &mut Frame<'_>, app: &MultiPodApp, area: Rect) {
    let guidance = if app
        .panel
        .composer
        .is_available(ComposerTarget::TicketIntake)
    {
        "  Row selection: blank Enter opens/dispatches · text Enter uses target · Tab target"
    } else if app.panel.header.ticket_configured {
        "  Row selection: blank Enter opens/dispatches · text Enter sends to Companion"
    } else {
        "  Pod-centric view · Row selection: blank Enter opens · text Enter sends to Companion"
    };
    let mut spans = vec![
        Span::styled(
            "workspace dashboard",
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::styled(guidance, Style::default().fg(Color::DarkGray)),
    ];
    if let Some(companion) = &app.panel.header.companion {
        spans.push(Span::styled(
            " · companion ",
            Style::default().fg(Color::DarkGray),
        ));
        spans.push(Span::styled(
            companion.status.label(),
            companion_status_style(companion.status),
        ));
        if let Some(detail) = companion.detail.as_deref() {
            spans.push(Span::styled(
                format!(" ({detail})"),
                Style::default().fg(Color::DarkGray),
            ));
        }
    }
    if let Some(orchestrator) = &app.panel.header.orchestrator {
        spans.push(Span::styled(
            " · orchestrator ",
            Style::default().fg(Color::DarkGray),
        ));
        spans.push(Span::styled(
            orchestrator.status.label(),
            orchestrator_status_style(orchestrator.status),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn companion_status_style(status: CompanionPanelStatus) -> Style {
    match status {
        CompanionPanelStatus::Live
        | CompanionPanelStatus::Restored
        | CompanionPanelStatus::Spawned => Style::default().fg(Color::Green),
        CompanionPanelStatus::Stopped | CompanionPanelStatus::Missing => {
            Style::default().fg(Color::Yellow)
        }
        CompanionPanelStatus::Unavailable => Style::default().fg(Color::Red),
    }
}

fn orchestrator_status_style(status: OrchestratorPanelStatus) -> Style {
    match status {
        OrchestratorPanelStatus::Live
        | OrchestratorPanelStatus::Restored
        | OrchestratorPanelStatus::Spawned => Style::default().fg(Color::Green),
        OrchestratorPanelStatus::Stopped | OrchestratorPanelStatus::Missing => {
            Style::default().fg(Color::Yellow)
        }
        OrchestratorPanelStatus::Unavailable => Style::default().fg(Color::Red),
    }
}

fn draw_list(frame: &mut Frame<'_>, app: &mut MultiPodApp, area: Rect) {
    if area.width == 0 || area.height == 0 {
        app.row_hit_boxes.clear();
        return;
    }
    let rows = list_rows(app, area.width, area.height);
    app.set_row_hit_boxes(&rows, area);
    let lines = rows.into_iter().map(|row| row.line).collect::<Vec<_>>();
    Paragraph::new(lines).render(area, frame.buffer_mut());
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PanelListRow {
    line: Line<'static>,
    key: Option<PanelRowKey>,
}

impl PanelListRow {
    fn inert(line: Line<'static>) -> Self {
        Self { line, key: None }
    }

    fn selectable(line: Line<'static>, key: PanelRowKey) -> Self {
        Self {
            line,
            key: Some(key),
        }
    }
}

#[cfg(test)]
fn list_lines(app: &MultiPodApp, width: u16, height: u16) -> Vec<Line<'static>> {
    list_rows(app, width, height)
        .into_iter()
        .map(|row| row.line)
        .collect()
}

fn list_rows(app: &MultiPodApp, width: u16, height: u16) -> Vec<PanelListRow> {
    let sections = sectioned_entries(&app.list);
    let selected = app.selected_row.as_ref();
    let diagnostic_rows = panel_diagnostic_lines(&app.panel, width)
        .into_iter()
        .map(PanelListRow::inert)
        .collect::<Vec<_>>();
    let action_rows = panel_action_rows(&app.panel, selected, width);
    let live_rows = sections
        .iter()
        .filter(|section| section.kind != MultiPodSectionKind::Closed)
        .flat_map(|section| section_rows(&app.list, section, selected, width))
        .collect::<Vec<_>>();
    let closed_rows = sections
        .iter()
        .find(|section| section.kind == MultiPodSectionKind::Closed)
        .map(|section| section_rows(&app.list, section, selected, width))
        .unwrap_or_default();

    let available = height as usize;
    let diagnostic_len = diagnostic_rows.len().min(available);
    let remaining_after_diagnostics = available.saturating_sub(diagnostic_len);
    let action_len = action_rows.len().min(remaining_after_diagnostics);
    let remaining_after_actions = remaining_after_diagnostics.saturating_sub(action_len);
    let closed_len = closed_rows.len().min(remaining_after_actions);
    let live_len = live_rows
        .len()
        .min(remaining_after_actions.saturating_sub(closed_len));
    let spacer_len = available.saturating_sub(diagnostic_len + action_len + live_len + closed_len);

    let mut rows = Vec::with_capacity(available);
    rows.extend(diagnostic_rows.into_iter().take(diagnostic_len));
    rows.extend(action_rows.into_iter().take(action_len));
    rows.extend(live_rows.into_iter().take(live_len));
    rows.extend(
        std::iter::repeat_with(|| PanelListRow::inert(Line::from(Span::raw("")))).take(spacer_len),
    );
    rows.extend(closed_rows.into_iter().take(closed_len));
    rows
}

fn row_hit_boxes(rows: &[PanelListRow], area: Rect) -> Vec<PanelRowHitBox> {
    if area.width == 0 || area.height == 0 {
        return Vec::new();
    }
    rows.iter()
        .enumerate()
        .filter_map(|(offset, row)| {
            let y = area.y.checked_add(offset as u16)?;
            if y >= area.y.saturating_add(area.height) {
                return None;
            }
            Some(PanelRowHitBox {
                rect: Rect::new(area.x, y, area.width, 1),
                key: row.key.clone()?,
            })
        })
        .collect()
}

fn panel_diagnostic_lines(panel: &WorkspacePanelViewModel, width: u16) -> Vec<Line<'static>> {
    panel
        .header
        .diagnostics
        .iter()
        .map(|diagnostic| {
            Line::from(vec![
                Span::styled("⚠ ", Style::default().fg(Color::Yellow)),
                Span::styled(
                    truncate_with_ellipsis(diagnostic, width.saturating_sub(2) as usize),
                    Style::default().fg(Color::Yellow),
                ),
            ])
        })
        .collect()
}

fn panel_action_rows(
    panel: &WorkspacePanelViewModel,
    selected: Option<&PanelRowKey>,
    width: u16,
) -> Vec<PanelListRow> {
    let rows = panel
        .rows
        .iter()
        .filter(|row| row.is_ticket_action())
        .collect::<Vec<_>>();
    if rows.is_empty() {
        return Vec::new();
    }
    let mut lines = Vec::with_capacity(rows.len() + 1);
    lines.push(PanelListRow::inert(panel_action_header_line(
        rows.len(),
        width,
    )));
    for row in rows {
        lines.push(PanelListRow::selectable(
            panel_row_line(row, selected == Some(&row.key), width),
            row.key.clone(),
        ));
    }
    lines
}

fn panel_action_header_line(total: usize, width: u16) -> Line<'static> {
    let detail = if total == 1 {
        " 1 row".to_string()
    } else {
        format!(" {total} rows")
    };
    let text = truncate_with_ellipsis(&format!("--tickets{detail}---"), width as usize);
    Line::from(Span::styled(
        text,
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    ))
}

const TICKET_STATE_COLUMN_WIDTH: usize = 10;
const TICKET_ID_COLUMN_WIDTH: usize = 13;
const POD_STATUS_COLUMN_WIDTH: usize = 18;

fn panel_row_line(row: &PanelRow, selected: bool, width: u16) -> Line<'static> {
    let marker = if selected { "▶ " } else { "  " };
    let title_style = if selected {
        Style::default()
            .fg(Color::Magenta)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Magenta)
    };
    let ticket_ref = panel_ticket_reference(row);
    let mut spans = Vec::new();
    let mut remaining = width as usize;

    push_bounded_span(
        &mut spans,
        marker,
        if selected {
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        },
        &mut remaining,
    );
    push_column_span(
        &mut spans,
        &row.status,
        TICKET_STATE_COLUMN_WIDTH,
        panel_priority_style(row.priority),
        &mut remaining,
    );
    push_column_span(
        &mut spans,
        &ticket_ref,
        TICKET_ID_COLUMN_WIDTH,
        Style::default().fg(Color::DarkGray),
        &mut remaining,
    );
    push_bounded_span(&mut spans, row.title.as_str(), title_style, &mut remaining);

    Line::from(spans)
}

fn panel_ticket_reference(row: &PanelRow) -> String {
    row.ticket
        .as_ref()
        .map(|ticket| ticket.id.clone())
        .unwrap_or_else(|| match &row.key {
            PanelRowKey::Ticket(id) => id.clone(),
            PanelRowKey::Pod(name) => name.clone(),
        })
}

fn push_column_span(
    spans: &mut Vec<Span<'static>>,
    value: &str,
    column_width: usize,
    style: Style,
    remaining: &mut usize,
) {
    if *remaining == 0 {
        return;
    }
    let mut content = padded_cell(value, column_width);
    content.push(' ');
    push_bounded_span(spans, &content, style, remaining);
}

fn push_bounded_span(
    spans: &mut Vec<Span<'static>>,
    value: &str,
    style: Style,
    remaining: &mut usize,
) {
    if *remaining == 0 || value.is_empty() {
        return;
    }
    let content = truncate_with_ellipsis(value, *remaining);
    *remaining = remaining.saturating_sub(content.width());
    spans.push(Span::styled(content, style));
}

fn padded_cell(value: &str, width: usize) -> String {
    let mut cell = truncate_with_ellipsis(value, width);
    let padding = width.saturating_sub(cell.width());
    cell.extend(std::iter::repeat_n(' ', padding));
    cell
}

fn panel_priority_style(priority: ActionPriority) -> Style {
    match priority {
        ActionPriority::UserReply => Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ActionPriority::ReadyForQueue => Style::default().fg(Color::Green),
        ActionPriority::ActiveWork => Style::default().fg(Color::Cyan),
        ActionPriority::Background => Style::default().fg(Color::DarkGray),
    }
}

fn section_rows(
    list: &PodList,
    section: &MultiPodSection,
    selected: Option<&PanelRowKey>,
    width: u16,
) -> Vec<PanelListRow> {
    let visible = visible_section_indices(section);
    if visible.is_empty() {
        return Vec::new();
    }

    let mut rows = Vec::with_capacity(visible.len() + 1);
    rows.push(PanelListRow::inert(section_header_line(
        section.kind,
        section.entries.len(),
        section.hidden_count(),
        width,
    )));
    for index in visible {
        if let Some(entry) = list.entries.get(index) {
            let key = PanelRowKey::Pod(entry.name.clone());
            let selected = selected == Some(&key);
            rows.push(PanelListRow::selectable(
                row_line(entry, selected, width),
                key,
            ));
        }
    }
    rows
}

fn row_line(entry: &PodListEntry, selected: bool, width: u16) -> Line<'static> {
    let marker = if selected { "▶ " } else { "  " };
    let name_style = if selected {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Cyan)
    };
    let (status, status_style) = row_status_label(entry);
    let mut spans = Vec::new();
    let mut remaining = width as usize;

    push_bounded_span(
        &mut spans,
        marker,
        if selected {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        },
        &mut remaining,
    );
    push_column_span(
        &mut spans,
        status,
        POD_STATUS_COLUMN_WIDTH,
        status_style,
        &mut remaining,
    );
    push_bounded_span(&mut spans, entry.name.as_str(), name_style, &mut remaining);

    Line::from(spans)
}

fn draw_separator(frame: &mut Frame<'_>, area: Rect) {
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "─".repeat(area.width as usize),
            Style::default().fg(Color::DarkGray),
        ))),
        area,
    );
}

fn draw_target_status(frame: &mut Frame<'_>, app: &MultiPodApp, area: Rect) {
    frame.render_widget(Paragraph::new(target_status_line(app)), area);
}

fn target_status_line(app: &MultiPodApp) -> Line<'static> {
    if !app.composer_is_blank() {
        return Line::from(vec![
            Span::styled("composer target ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                app.composer_target().label(),
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" · draft Enter ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                composer_enter_status_text(app),
                Style::default().fg(Color::Green),
            ),
            Span::styled(
                " · row selection waits until composer is blank",
                Style::default().fg(Color::DarkGray),
            ),
        ]);
    }

    if let Some(row) = app
        .selected_panel_row()
        .filter(|row| row.is_ticket_action())
    {
        let action = row.next_action.map(NextUserAction::label).unwrap_or("View");
        Line::from(vec![
            Span::styled("composer target ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                app.composer_target().label(),
                Style::default()
                    .fg(Color::Magenta)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" · selected Ticket ", Style::default().fg(Color::DarkGray)),
            Span::styled(row.status.clone(), panel_priority_style(row.priority)),
            Span::styled(" · blank Enter ", Style::default().fg(Color::DarkGray)),
            Span::styled(action, Style::default().fg(Color::Magenta)),
        ])
    } else if let Some(entry) = app.selected_pod_entry() {
        let (status, status_style) = row_status_label(entry);
        Line::from(vec![
            Span::styled("composer target ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                app.composer_target().label(),
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" · selected Pod ", Style::default().fg(Color::DarkGray)),
            Span::styled(status.to_string(), status_style),
            Span::styled(
                " · blank Enter open/attach",
                Style::default().fg(Color::DarkGray),
            ),
        ])
    } else {
        Line::from(vec![
            Span::styled("composer target ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                app.composer_target().label(),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(
                " · no row selected · ↑/↓ selects a row",
                Style::default().fg(Color::DarkGray),
            ),
        ])
    }
}

fn draw_input(frame: &mut Frame<'_>, render: &crate::input::InputRender, area: Rect) {
    let mut lines: Vec<Line<'static>> = Vec::with_capacity(render.lines.len());
    for (i, src) in render.lines.iter().enumerate() {
        let absolute_row = render.viewport_start_row as usize + i;
        let prefix = if absolute_row == 0 { "> " } else { "  " };
        let mut spans = vec![Span::styled(prefix, Style::default().fg(Color::DarkGray))];
        spans.extend(src.spans.iter().cloned());
        lines.push(Line::from(spans));
    }
    frame.render_widget(Paragraph::new(lines), area);

    let cursor_x = area.x + 2 + render.cursor_col;
    let cursor_y = area.y + render.cursor_row;
    if cursor_y < area.y + area.height {
        frame.set_cursor_position(Position::new(cursor_x, cursor_y));
    }
}

fn composer_enter_status_text(app: &MultiPodApp) -> String {
    match app.composer_target() {
        ComposerTarget::Companion => companion_enter_status_text(app),
        ComposerTarget::TicketIntake => "launch Intake with composer text".to_string(),
    }
}

fn composer_enter_actionbar_text(app: &MultiPodApp) -> String {
    match app.composer_target() {
        ComposerTarget::Companion => companion_enter_actionbar_text(app),
        ComposerTarget::TicketIntake => {
            "Ticket Intake target: Enter launches Intake with composer text".to_string()
        }
    }
}

fn companion_enter_status_text(app: &MultiPodApp) -> String {
    match companion_send_availability(app) {
        CompanionSendAvailability::Ready => "send composer text to workspace Companion".to_string(),
        CompanionSendAvailability::Unavailable(reason) => format!("keep draft; {reason}"),
    }
}

fn companion_enter_actionbar_text(app: &MultiPodApp) -> String {
    match companion_send_availability(app) {
        CompanionSendAvailability::Ready => {
            "Companion target: Enter sends composer text to workspace Companion".to_string()
        }
        CompanionSendAvailability::Unavailable(reason) => {
            format!("Companion target: Enter keeps draft; {reason}")
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CompanionSendAvailability {
    Ready,
    Unavailable(String),
}

fn companion_send_availability(app: &MultiPodApp) -> CompanionSendAvailability {
    let Some(companion) = app.panel.header.companion.as_ref() else {
        return CompanionSendAvailability::Unavailable(
            "workspace Companion is unavailable".to_string(),
        );
    };
    if matches!(
        companion.status,
        CompanionPanelStatus::Unavailable
            | CompanionPanelStatus::Missing
            | CompanionPanelStatus::Stopped
    ) {
        return CompanionSendAvailability::Unavailable(format!(
            "workspace Companion is {}",
            companion.status.label()
        ));
    }
    let Some(entry) = app
        .list
        .entries
        .iter()
        .find(|entry| entry.name == companion.pod_name)
    else {
        return CompanionSendAvailability::Unavailable(format!(
            "workspace Companion `{}` is not in the Pod list",
            companion.pod_name
        ));
    };
    let Some(live) = entry.live.as_ref() else {
        return CompanionSendAvailability::Unavailable(format!(
            "workspace Companion `{}` is stopped",
            companion.pod_name
        ));
    };
    if !live.reachable {
        return CompanionSendAvailability::Unavailable(format!(
            "workspace Companion `{}` is unreachable",
            companion.pod_name
        ));
    }
    if live.status == Some(PodStatus::Running) {
        return CompanionSendAvailability::Unavailable(format!(
            "workspace Companion `{}` is running",
            companion.pod_name
        ));
    }
    CompanionSendAvailability::Ready
}

fn actionbar_left_text(app: &MultiPodApp) -> String {
    if app.sending && app.composer_target() == ComposerTarget::TicketIntake {
        "launching Ticket Intake…".to_string()
    } else if app.sending {
        "working…".to_string()
    } else if app.refreshing {
        match app.notice.as_deref() {
            Some(notice) if notice.contains("Refreshing") || notice.contains("refreshing") => {
                notice.to_string()
            }
            Some(notice) => format!("{notice} Refreshing workspace…"),
            None => "Refreshing workspace…".to_string(),
        }
    } else if !app.composer_is_blank() {
        composer_enter_actionbar_text(app)
    } else if let Some(notice) = app.notice.as_deref() {
        notice.to_string()
    } else if let Some(reason) = app.selected_open_disabled_reason() {
        reason
    } else {
        match app.composer_target() {
            ComposerTarget::Companion => {
                "Composer target: Companion; type text to send, or use ↑/↓ then blank Enter for rows"
                    .to_string()
            }
            ComposerTarget::TicketIntake => {
                "Composer target: Ticket Intake; type a request, then Enter launches Intake".to_string()
            }
        }
    }
}

fn actionbar_right_text(app: &MultiPodApp) -> &'static str {
    if app.panel_diagnostic_open {
        "F2/Esc close details  Ctrl+C quit"
    } else if app.panel_diagnostic.is_some() {
        "F2 details  ↑/↓ select row  Enter selected row  Tab target  Esc clear selection  Left/Right cursor  Ctrl+C quit"
    } else if !app.composer_is_blank() {
        if app
            .panel
            .composer
            .is_available(ComposerTarget::TicketIntake)
        {
            "↑/↓ draft lines  Left/Right cursor  Enter composer target  Tab target  Esc clear selection  Ctrl+C quit"
        } else {
            "↑/↓ draft lines  Left/Right cursor  Enter composer target  Esc clear selection  Ctrl+C quit"
        }
    } else if app
        .panel
        .composer
        .is_available(ComposerTarget::TicketIntake)
    {
        "↑/↓ select row  Enter selected row  Tab target  Esc clear selection  Left/Right cursor  Ctrl+C quit"
    } else {
        "↑/↓ select row  Enter selected row  Esc clear selection  Left/Right cursor  Ctrl+C quit"
    }
}

fn draw_actionbar(frame: &mut Frame<'_>, app: &MultiPodApp, area: Rect) {
    let left = actionbar_left_text(app);
    let right = actionbar_right_text(app);
    let left_width = area
        .width
        .saturating_sub(right.width() as u16)
        .saturating_sub(2) as usize;
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            truncate_with_ellipsis(&left, left_width),
            Style::default().fg(Color::DarkGray),
        ))),
        area,
    );
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            right,
            Style::default().fg(Color::DarkGray),
        )))
        .alignment(ratatui::layout::Alignment::Right),
        area,
    );
}

fn truncate_with_ellipsis(s: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    if s.width() <= max_width {
        return s.to_string();
    }
    if max_width == 1 {
        return "…".to_string();
    }
    let mut out = String::new();
    let mut width = 0usize;
    for c in s.chars() {
        let cw = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        if width + cw > max_width - 1 {
            break;
        }
        out.push(c);
        width += cw;
    }
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
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
            PathBuf::from("/tmp/Yoi Workspace/.worktree/orchestration/yoi-workspace-orchestrator")
        );
        assert_eq!(layout.branch, "orchestration/yoi-workspace-orchestrator");
    }

    #[test]
    fn orchestrator_launch_context_uses_orchestration_root_for_runtime_workspace() {
        let original = PathBuf::from("/repo/yoi");
        let orchestration = original
            .join(".worktree")
            .join("orchestration")
            .join("yoi-orchestrator");
        let context =
            build_orchestrator_launch_context(&original, &orchestration, "yoi-orchestrator");
        assert_eq!(context.workspace_root, orchestration);
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
        assert!(
            restored
                .layout
                .path
                .ends_with(".worktree/orchestration/repo-orchestrator")
        );
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

    use crate::pod_list::{LivePodInfo, PodEntrySummary, StoredMetadataState, StoredPodInfo};
    use std::fs;
    use tempfile::TempDir;
    use ticket::{
        LocalTicketBackend, MarkdownText, NewTicket, NewTicketEvent, TicketBackend,
        TicketEventKind, TicketWorkflowState,
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

    fn request_for(
        temp: &TempDir,
        ticket_id: String,
        action: NextUserAction,
    ) -> TicketActionRequest {
        TicketActionRequest {
            workspace_root: temp.path().to_path_buf(),
            ticket_id,
            action,
            orchestrator: None,
        }
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
            event.kind == TicketEventKind::StateChanged
                && event.state_field.as_deref() == Some("state")
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
            .expect("Panel Close records resolution.md")
            .as_str();
        assert!(resolution.contains("state: done"));
        assert!(resolution.contains("No implementation work"));
        assert!(resolution.contains("state change"));
        assert!(resolution.contains("worker invocation"));
        assert!(ticket.events.iter().any(|event| {
            event.kind == TicketEventKind::Close && event.body.as_str().contains("workspace Panel")
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

        let error =
            dispatch_ticket_action(request_for(&temp, ticket_id.clone(), NextUserAction::Wait))
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
        assert!(message.contains(
            "`.yoi/memory` plus local/runtime/log/lock/secret-like `.yoi` paths excluded"
        ));
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
                })
                .await
                .unwrap();
            reader.next::<Method>().await.unwrap().unwrap()
        });

        send_notify_only(&socket_path, "panel Queue".to_string(), true)
            .await
            .unwrap();
        let method = server.await.unwrap();
        assert!(matches!(
            method,
            Method::Notify { message, auto_run: true } if message == "panel Queue"
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
                })
                .await
                .unwrap();
            reader.next::<Method>().await.unwrap().unwrap()
        });

        send_notify_only(&socket_path, "panel progress".to_string(), false)
            .await
            .unwrap();
        let method = server.await.unwrap();
        assert!(matches!(
            method,
            Method::Notify { message, auto_run: false } if message == "panel progress"
        ));
    }

    #[test]
    fn no_ticket_selection_keeps_enter_pod_centric() {
        let mut app = test_app(vec![live_info("alpha", PodStatus::Idle)]);

        assert!(matches!(
            app.handle_key(key(KeyCode::Enter)),
            MultiPodAction::Open
        ));
        assert!(app.prepare_ticket_action_dispatch().is_none());
        assert_eq!(app.notice.as_deref(), Some("No Ticket action is selected."));
    }

    #[test]
    fn multi_ticket_action_rows_precede_pods_and_pod_actions_still_work() {
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
            MultiPodAction::None
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
        assert_eq!(boxes[0].rect, Rect::new(3, 6, 80, 1));
        assert_eq!(boxes[1].key, PanelRowKey::Ticket("TICKET-2".into()));
        assert_eq!(boxes[1].rect, Rect::new(3, 7, 80, 1));
        assert_eq!(boxes[2].key, PanelRowKey::Pod("alpha".into()));
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

        assert!(app.handle_mouse_event(left_click(2, 2)));
        assert_eq!(
            app.selected_row,
            Some(PanelRowKey::Ticket("TICKET-2".into()))
        );
        assert_eq!(app.selected_panel_row().unwrap().title, "Queued");
        assert_eq!(app.selected_ticket_action(), Some(NextUserAction::Wait));
        assert!(plain_line(&target_status_line(&app)).contains("blank Enter Wait"));
        assert!(matches!(
            app.handle_key(key(KeyCode::Enter)),
            MultiPodAction::DispatchTicketAction(request) if request.ticket_id == "TICKET-2"
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

        assert!(app.handle_mouse_event(left_click(2, 2)));
        assert_eq!(
            app.selected_row,
            Some(PanelRowKey::Ticket("TICKET-2".into()))
        );
        app.input.insert_paste("hello".into());
        assert!(matches!(
            app.handle_key(key(KeyCode::Enter)),
            MultiPodAction::None
        ));
        assert_eq!(input_text(&app), "hello");
        assert!(matches!(
            app.handle_key(key(KeyCode::Esc)),
            MultiPodAction::None
        ));
        assert_eq!(app.selected_row, None);
        assert_eq!(input_text(&app), "hello");
        assert!(matches!(
            app.handle_key(key(KeyCode::Down)),
            MultiPodAction::None
        ));
        assert_eq!(app.selected_row, None);
    }

    #[test]
    fn selected_ticket_row_with_non_empty_composer_shows_composer_enter_behavior() {
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
        app.input.insert_str("draft to companion");

        assert_eq!(
            app.selected_ticket_action(),
            Some(NextUserAction::Queue),
            "selected row remains a Ticket action row"
        );
        let actionbar_left = actionbar_left_text(&app);
        let actionbar_right = actionbar_right_text(&app);
        let target_status = plain_line(&target_status_line(&app));

        assert!(actionbar_left.contains("Companion target: Enter sends composer text"));
        assert!(actionbar_right.contains("Enter composer target"));
        assert!(!actionbar_left.contains("Queue"));
        assert!(!actionbar_right.contains("selected row"));
        assert!(target_status.contains("composer target Companion"));
        assert!(target_status.contains("draft Enter send composer text to workspace Companion"));
        assert!(target_status.contains("row selection waits until composer is blank"));
        assert!(!target_status.contains("blank Enter Queue"));
    }

    #[test]
    fn multi_bare_panel_letters_append_to_composer_and_arrows_select_when_blank() {
        let mut app = test_app(vec![
            live_info("alpha", PodStatus::Idle),
            live_info("beta", PodStatus::Idle),
        ]);
        assert_eq!(app.list.selected_entry().unwrap().name, "alpha");

        for c in ['j', 'k', 'o', 'r'] {
            assert!(matches!(
                app.handle_key(key(KeyCode::Char(c))),
                MultiPodAction::None
            ));
        }

        assert_eq!(input_text(&app), "jkor");
        assert_eq!(app.list.selected_entry().unwrap().name, "alpha");

        assert!(matches!(
            app.handle_key(key(KeyCode::Down)),
            MultiPodAction::None
        ));
        assert_eq!(input_text(&app), "jkor");
        assert_eq!(app.list.selected_entry().unwrap().name, "alpha");

        app.input.clear();
        assert!(matches!(
            app.handle_key(key(KeyCode::Down)),
            MultiPodAction::None
        ));
        assert_eq!(input_text(&app), "");
        assert_eq!(app.list.selected_entry().unwrap().name, "beta");

        assert!(matches!(
            app.handle_key(key(KeyCode::Up)),
            MultiPodAction::None
        ));
        assert_eq!(input_text(&app), "");
        assert_eq!(app.list.selected_entry().unwrap().name, "alpha");
    }

    #[test]
    fn multi_selection_changes_preserve_composer_contents() {
        let mut app = test_app(vec![
            live_info("alpha", PodStatus::Idle),
            live_info("beta", PodStatus::Idle),
        ]);
        app.input.insert_str("draft message");
        let before = input_text(&app);

        app.select_next();

        assert_eq!(input_text(&app), before);
        assert_eq!(app.list.selected_entry().unwrap().name, "beta");
    }

    #[test]
    fn multi_poll_reload_preserves_selection_composer_and_notice() {
        let mut app = test_app(vec![
            live_info_with_updated_at("alpha", PodStatus::Idle, 10),
            live_info_with_updated_at("beta", PodStatus::Idle, 20),
        ]);
        app.select_next();
        assert_eq!(app.list.selected_entry().unwrap().name, "alpha");
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

        assert_eq!(app.list.selected_entry().unwrap().name, "alpha");
        assert_eq!(
            app.list
                .selected_entry()
                .unwrap()
                .live
                .as_ref()
                .unwrap()
                .status,
            Some(PodStatus::Running)
        );
        assert_eq!(input_text(&app), "draft survives polling");
        assert_eq!(app.notice.as_deref(), Some("keep this notice"));
    }

    #[test]
    fn multi_poll_reload_falls_back_when_selected_pod_disappears() {
        let mut app = test_app(vec![
            live_info_with_updated_at("alpha", PodStatus::Idle, 10),
            live_info_with_updated_at("beta", PodStatus::Running, 20),
        ]);
        assert_eq!(app.list.selected_entry().unwrap().name, "beta");
        let refreshed = PodList::from_sources(
            PodVisibilitySource::ResumePicker,
            vec![stopped_info_with_updated_at("closed", 30)],
            vec![live_info_with_updated_at("alpha", PodStatus::Idle, 40)],
            None,
            10,
        );

        app.apply_reloaded_list(refreshed);

        assert_eq!(app.list.selected_entry().unwrap().name, "alpha");
        assert_eq!(visible_entry_indices(&app.list), vec![0, 1]);
    }

    #[test]
    fn multi_poll_reload_error_keeps_previous_list_and_composer() {
        let mut app = test_app(vec![live_info("alpha", PodStatus::Idle)]);
        app.input.insert_str("keep draft");

        app.apply_reload_result(Err(MultiPodError::Io(io::Error::other("boom"))));

        assert_eq!(app.list.selected_entry().unwrap().name, "alpha");
        assert_eq!(input_text(&app), "keep draft");
        let notice = app.notice.as_deref().unwrap();
        assert!(notice.contains("Refresh failed"));
        assert!(notice.contains("boom"));
    }

    #[test]
    fn multi_orchestrator_failure_persists_over_plain_observe_missing() {
        let detail =
            "could not spawn workspace Orchestrator: delegated scope conflicts with writer";
        let mut app = app_with_panel(
            empty_test_list(),
            panel_with_orchestrator(OrchestratorPanelStatus::Unavailable, Some(detail)),
        );

        app.apply_reloaded_snapshot(MultiPodSnapshot {
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
    fn multi_orchestrator_plain_missing_remains_when_no_prior_failure_exists() {
        let mut app = app_with_panel(
            empty_test_list(),
            panel_with_orchestrator(OrchestratorPanelStatus::Missing, None),
        );

        app.apply_reloaded_snapshot(MultiPodSnapshot {
            list: empty_test_list(),
            panel: panel_with_orchestrator(OrchestratorPanelStatus::Missing, None),
        });

        let orchestrator = app.panel.header.orchestrator.as_ref().unwrap();
        assert_eq!(orchestrator.status, OrchestratorPanelStatus::Missing);
        assert!(orchestrator.detail.is_none());
        assert!(app.panel.header.diagnostics.is_empty());
    }

    #[test]
    fn multi_orchestrator_failure_clears_after_live_lifecycle() {
        let detail =
            "could not spawn workspace Orchestrator: delegated scope conflicts with writer";
        let mut app = app_with_panel(
            empty_test_list(),
            panel_with_orchestrator(OrchestratorPanelStatus::Unavailable, Some(detail)),
        );

        app.apply_reloaded_snapshot(MultiPodSnapshot {
            list: empty_test_list(),
            panel: panel_with_orchestrator(OrchestratorPanelStatus::Live, None),
        });
        assert_eq!(
            app.panel.header.orchestrator.as_ref().unwrap().status,
            OrchestratorPanelStatus::Live
        );

        app.apply_reloaded_snapshot(MultiPodSnapshot {
            list: empty_test_list(),
            panel: panel_with_orchestrator(OrchestratorPanelStatus::Missing, None),
        });

        let orchestrator = app.panel.header.orchestrator.as_ref().unwrap();
        assert_eq!(orchestrator.status, OrchestratorPanelStatus::Missing);
        assert!(orchestrator.detail.is_none());
    }

    #[test]
    fn multi_orchestrator_failure_supersedes_prior_failure() {
        let old_detail = "could not spawn workspace Orchestrator: old scope conflict";
        let new_detail = "could not restore workspace Orchestrator: socket refused";
        let mut app = app_with_panel(
            empty_test_list(),
            panel_with_orchestrator(OrchestratorPanelStatus::Unavailable, Some(old_detail)),
        );

        app.apply_reloaded_snapshot(MultiPodSnapshot {
            list: empty_test_list(),
            panel: panel_with_orchestrator(OrchestratorPanelStatus::Unavailable, Some(new_detail)),
        });
        app.apply_reloaded_snapshot(MultiPodSnapshot {
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
    async fn multi_poll_reload_does_not_overlap_in_flight_reload() {
        let mut app = test_app(vec![live_info("alpha", PodStatus::Idle)]);
        let mut pending = PendingReload::default();

        assert!(pending.start_with_handle(tokio::spawn(async {
            tokio::time::sleep(Duration::from_millis(10)).await;
            Err(MultiPodError::Io(io::Error::other("boom")))
        })));
        assert!(!pending.start_with_handle(tokio::spawn(async {
            let list = PodList::from_sources(
                PodVisibilitySource::ResumePicker,
                vec![],
                vec![live_info("beta", PodStatus::Idle)],
                None,
                10,
            );
            Ok(MultiPodSnapshot {
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
    async fn multi_quit_aborts_background_reload_and_notice_without_waiting() {
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
            Err(MultiPodError::Io(io::Error::other(
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
            while !(reload_cancelled.load(Ordering::SeqCst)
                && notice_cancelled.load(Ordering::SeqCst))
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
    fn multi_idle_live_selected_target_is_open_eligible() {
        let app = test_app(vec![live_info("idle", PodStatus::Idle)]);

        assert_eq!(app.selected_open_eligibility(), OpenEligibility::OpenNow);
        assert!(app.selected_open_disabled_reason().is_none());
    }

    #[test]
    fn multi_status_label_for_live_without_reported_status_is_softened() {
        let mut live = live_info("probing", PodStatus::Idle);
        live.status = None;
        let app = test_app(vec![live]);

        let (label, _) = row_status_label(app.list.selected_entry().unwrap());

        assert_eq!(label, "live");
    }

    #[test]
    fn multi_status_labels_preserve_explicit_live_statuses() {
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
    fn panel_ticket_rows_use_aligned_columns_before_title() {
        let review_row = panel_test_ticket_row(
            "00001KTX1QMG9",
            "Workspace panel composer targets",
            ActionPriority::ActiveWork,
            NextUserAction::Wait,
            "inprogress",
        );
        let ready_row = panel_test_ticket_row(
            "00001KTTB479X",
            "Long Ticket title that should be rendered after short columns",
            ActionPriority::ReadyForQueue,
            NextUserAction::Queue,
            "ready",
        );

        let review_line = plain_line(&panel_row_line(&review_row, true, 160));
        let ready_line = plain_line(&panel_row_line(&ready_row, false, 160));
        let state_start = 2;
        let id_start = state_start + TICKET_STATE_COLUMN_WIDTH + 1;
        let title_start = id_start + TICKET_ID_COLUMN_WIDTH + 1;

        assert!(!review_line.starts_with("▶ Workspace panel composer targets"));
        assert_eq!(display_column(&review_line, "inprogress"), state_start);
        assert_eq!(display_column(&ready_line, "ready"), state_start);
        let review_id = review_row.ticket.as_ref().unwrap().id.as_str();
        let ready_id = ready_row.ticket.as_ref().unwrap().id.as_str();
        assert_eq!(review_id.width(), TICKET_ID_COLUMN_WIDTH);
        assert_eq!(ready_id.width(), TICKET_ID_COLUMN_WIDTH);
        assert_eq!(display_column(&review_line, review_id), id_start);
        assert_eq!(display_column(&ready_line, ready_id), id_start);
        assert_eq!(
            display_column(&review_line, "Workspace panel composer targets"),
            title_start
        );
        assert_eq!(
            display_column(&ready_line, "Long Ticket title"),
            title_start
        );
    }

    #[test]
    fn panel_ticket_title_truncates_after_stable_columns() {
        let row = panel_test_ticket_row(
            "00001KTTB479X",
            "Very long Ticket title that should truncate only after the aligned short columns",
            ActionPriority::ReadyForQueue,
            NextUserAction::Queue,
            "ready",
        );

        let line = plain_line(&panel_row_line(&row, false, 58));
        let title_start = 2 + TICKET_STATE_COLUMN_WIDTH + 1 + TICKET_ID_COLUMN_WIDTH + 1;

        assert_eq!(line.width(), 58);
        let row_id = row.ticket.as_ref().unwrap().id.as_str();
        assert_eq!(row_id.width(), TICKET_ID_COLUMN_WIDTH);
        assert_eq!(
            display_column(&line, row_id),
            title_start - TICKET_ID_COLUMN_WIDTH - 1
        );
        assert_eq!(display_column(&line, "Very long Ticket"), title_start);
        assert!(line.ends_with('…'));
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
    fn multi_running_paused_and_stopped_targets_are_open_eligible() {
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

        assert_eq!(app.selected_open_eligibility(), OpenEligibility::OpenNow);
        assert!(app.selected_open_disabled_reason().is_none());
        app.select_next();
        assert_eq!(app.list.selected_entry().unwrap().name, "paused");
        assert_eq!(app.selected_open_eligibility(), OpenEligibility::OpenNow);
        assert!(app.selected_open_disabled_reason().is_none());
        app.select_next();
        assert_eq!(app.list.selected_entry().unwrap().name, "stopped");
        assert_eq!(app.selected_open_eligibility(), OpenEligibility::OpenNow);
        assert!(app.selected_open_disabled_reason().is_none());
    }

    #[test]
    fn multi_sections_classify_pending_working_and_closed() {
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
    fn multi_closed_section_is_limited_to_three_visible_rows() {
        let list = closed_list(5, Some("closed-0"));
        let visible = visible_entry_indices(&list)
            .into_iter()
            .map(|index| list.entries[index].name.clone())
            .collect::<Vec<_>>();
        let sections = sectioned_entries(&list);
        let closed = sections
            .iter()
            .find(|section| section.kind == MultiPodSectionKind::Closed)
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
    fn multi_selection_follows_visible_section_order_without_hidden_closed_rows() {
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
    fn multi_selection_does_not_default_to_orchestrator_only_row() {
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
    fn multi_selection_prefers_non_orchestrator_pod_by_default() {
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

        assert_eq!(app.list.selected_entry().unwrap().name, "worker");
    }

    #[test]
    fn multi_list_renders_workspace_diagnostics_before_rows() {
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
    fn multi_list_pins_closed_section_below_live_flexible_area() {
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
    fn multi_layout_uses_single_boundary_separator_between_list_and_composer() {
        let layout = multi_pod_layout(Rect::new(0, 0, 80, 24), 1);

        assert_eq!(layout.boundary.height, 1);
        assert!(!layout.list_draws_own_separator);
        assert_eq!(layout.boundary.y, layout.list.y + layout.list.height);
        assert_eq!(
            layout.target_status.y,
            layout.boundary.y + layout.boundary.height
        );
    }

    #[test]
    fn multi_companion_submit_routes_to_workspace_companion_not_selected_pod() {
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
            MultiPodAction::SendCompanion(request) => request,
            _ => panic!("Companion target should send to the workspace Companion"),
        };

        assert_eq!(request.pod_name, "yoi");
        assert_eq!(request.socket_path, PathBuf::from("/tmp/yoi.sock"));
        assert!(app.sending);
        assert_eq!(input_text(&app), "send to companion");
        assert!(app.notice.as_deref().unwrap().contains("Companion yoi"));
    }

    #[test]
    fn multi_companion_submit_unavailable_keeps_composer_contents() {
        let mut app = companion_app(vec![], CompanionPanelStatus::Missing);
        app.input.insert_str("keep me");
        let before = input_text(&app);

        assert!(matches!(
            app.handle_key(key(KeyCode::Enter)),
            MultiPodAction::None
        ));

        assert_eq!(input_text(&app), before);
        assert!(!app.sending);
        assert!(app.notice.as_deref().unwrap().contains("draft kept"));
    }

    #[test]
    fn multi_companion_submit_empty_reports_empty_composer() {
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
    fn multi_companion_finish_success_clears_composer() {
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
    fn multi_open_request_keeps_dashboard_state_for_nested_single_pod() {
        let mut app = test_app(vec![live_info("alpha", PodStatus::Idle)]);
        app.input.insert_str("draft survives open");

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
    fn multi_open_failure_keeps_composer_and_sets_notice() {
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
    fn multi_loading_app_defers_initial_snapshot_to_enter_reload() {
        let app = MultiPodApp::loading(PodRuntimeCommand::for_executable("/tmp/yoi"));

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
    fn multi_open_success_requests_background_reload_without_dropping_state() {
        let mut app = test_app(vec![live_info("alpha", PodStatus::Idle)]);
        app.input.insert_str("keep this draft");

        app.finish_open("alpha", Ok(()));

        assert_eq!(input_text(&app), "keep this draft");
        assert!(
            app.notice
                .as_deref()
                .unwrap()
                .contains("Refreshing workspace")
        );
        assert!(app.refreshing);
        assert!(matches!(
            app.enter_reload,
            Some(OrchestratorLifecycleMode::Observe)
        ));
    }

    #[test]
    fn multi_open_disabled_target_stays_in_dashboard() {
        let mut live = live_info("unreachable", PodStatus::Idle);
        live.reachable = false;
        live.status = None;
        let mut app = test_app(vec![live]);

        assert!(app.prepare_open().is_none());
        assert!(app.notice.as_deref().unwrap().contains("cannot be opened"));
    }

    #[test]
    fn multi_empty_enter_uses_open_action() {
        let mut app = test_app(vec![live_info("alpha", PodStatus::Idle)]);

        assert!(matches!(
            app.handle_key(key(KeyCode::Enter)),
            MultiPodAction::Open
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
    fn multi_whitespace_only_enter_uses_open_action() {
        let mut app = test_app(vec![live_info("alpha", PodStatus::Idle)]);
        app.input.insert_str("  \n\t");

        assert!(matches!(
            app.handle_key(key(KeyCode::Enter)),
            MultiPodAction::Open
        ));
        let request = app.prepare_open().unwrap();

        assert_eq!(request.pod_name, "alpha");
        assert_eq!(input_text(&app), "  \n\t");
    }

    #[test]
    fn multi_non_empty_enter_reports_companion_unavailable() {
        let mut app = test_app(vec![live_info("idle", PodStatus::Idle)]);
        app.input.insert_str("keep this draft");

        assert!(matches!(
            app.handle_key(key(KeyCode::Enter)),
            MultiPodAction::None
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
    fn multi_composer_shared_word_motion_and_delete_keys() {
        let mut app = ticket_enabled_app(vec![live_info("idle", PodStatus::Idle)]);
        app.input.insert_str("hello world");

        assert!(matches!(
            app.handle_key(modified_key(KeyCode::Left, KeyModifiers::CONTROL)),
            MultiPodAction::None
        ));
        assert!(matches!(
            app.handle_key(key(KeyCode::Char('!'))),
            MultiPodAction::None
        ));
        assert_eq!(input_text(&app), "hello !world");

        assert!(matches!(
            app.handle_key(modified_key(KeyCode::Right, KeyModifiers::CONTROL)),
            MultiPodAction::None
        ));
        assert!(matches!(
            app.handle_key(modified_key(KeyCode::Char('w'), KeyModifiers::CONTROL)),
            MultiPodAction::None
        ));
        assert_eq!(input_text(&app), "hello !");
    }

    #[test]
    fn multi_esc_clears_row_selection_without_quitting_and_preserves_draft() {
        let mut app = ticket_enabled_app(vec![live_info("alpha", PodStatus::Idle)]);
        app.input.insert_str("draft message");

        assert!(app.selected_row.is_some());
        assert!(matches!(
            app.handle_key(key(KeyCode::Esc)),
            MultiPodAction::None
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
            MultiPodAction::Quit
        ));
    }

    #[test]
    fn multi_composer_target_switch_preserves_typed_text() {
        let mut app = ticket_enabled_app(vec![live_info("idle", PodStatus::Idle)]);
        app.input.insert_str("draft intake request");

        assert!(matches!(app.composer_target(), ComposerTarget::Companion));
        let selected_before = app.selected_row.clone();
        assert!(matches!(
            app.handle_key(key(KeyCode::Tab)),
            MultiPodAction::None
        ));

        assert!(matches!(
            app.composer_target(),
            ComposerTarget::TicketIntake
        ));
        assert_eq!(app.selected_row, selected_before);
        assert_eq!(input_text(&app), "draft intake request");
    }

    #[test]
    fn multi_ctrl_t_does_not_switch_composer_target() {
        let mut app = ticket_enabled_app(vec![live_info("idle", PodStatus::Idle)]);
        app.input.insert_str("draft intake request");

        assert!(matches!(app.composer_target(), ComposerTarget::Companion));
        assert!(matches!(
            app.handle_key(modified_key(KeyCode::Char('t'), KeyModifiers::CONTROL)),
            MultiPodAction::None
        ));

        assert!(matches!(app.composer_target(), ComposerTarget::Companion));
        assert_eq!(input_text(&app), "draft intake request");
    }

    #[test]
    fn multi_no_ticket_workspace_exposes_only_companion_target() {
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
    fn multi_blank_ticket_intake_enter_uses_selected_row_and_preserves_input() {
        let mut app = ticket_enabled_app(vec![live_info("idle", PodStatus::Idle)]);
        app.cycle_composer_target();
        app.input.insert_str("  \n\t");

        assert!(matches!(
            app.handle_key(key(KeyCode::Enter)),
            MultiPodAction::Open
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
    fn multi_ticket_intake_enter_builds_launch_request_not_direct_send() {
        let mut app = ticket_enabled_app(vec![live_info("idle", PodStatus::Idle)]);
        app.cycle_composer_target();
        app.input.insert_str("please intake this work");

        let request = match app.handle_key(key(KeyCode::Enter)) {
            MultiPodAction::LaunchIntake(request) => request,
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
    fn multi_ticket_intake_handoff_skips_peer_registration_when_orchestrator_not_live() {
        let mut app = ticket_enabled_app_with_orchestrator(
            vec![live_info("idle", PodStatus::Idle)],
            OrchestratorPanelStatus::Unavailable,
        );
        app.cycle_composer_target();
        app.input.insert_str("please intake this work");

        let request = match app.handle_key(key(KeyCode::Enter)) {
            MultiPodAction::LaunchIntake(request) => request,
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
    fn multi_ticket_intake_finish_success_clears_composer_and_reports_pod() {
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
    fn multi_ticket_intake_finish_failure_keeps_composer() {
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

        assert!(commit_intake_registry_update(update.clone()).is_none());
        assert!(
            store
                .claim_for_ticket("20260608-000000-existing")
                .unwrap()
                .is_some(),
            "the claim is persisted only by the post-acceptance commit step"
        );

        assert!(commit_intake_registry_update(update).is_none());
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

        let warning = commit_intake_registry_update(IntakeRegistryUpdate::ClaimTicket {
            registry_root: root,
            ticket_id: "20260608-000001-existing".to_string(),
            ticket_slug: Some("existing".to_string()),
            pod_name: "second-intake".to_string(),
        })
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
    fn multi_empty_enter_on_non_openable_row_reports_open_diagnostic() {
        let mut app = test_app(vec![unreachable_live_info("unreachable")]);
        assert!(matches!(
            app.handle_key(key(KeyCode::Enter)),
            MultiPodAction::Open
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

    fn test_app(live: Vec<LivePodInfo>) -> MultiPodApp {
        app_with_list(PodList::from_sources(
            PodVisibilitySource::ResumePicker,
            vec![],
            live,
            None,
            10,
        ))
    }

    fn companion_app(live: Vec<LivePodInfo>, status: CompanionPanelStatus) -> MultiPodApp {
        let mut panel = WorkspacePanelViewModel::empty(Path::new("test"));
        panel.header.companion = Some(CompanionPanelState::new("yoi", status, None));
        app_with_panel(
            PodList::from_sources(PodVisibilitySource::ResumePicker, vec![], live, None, 10),
            panel,
        )
    }

    fn ticket_enabled_app(live: Vec<LivePodInfo>) -> MultiPodApp {
        ticket_enabled_app_with_orchestrator(live, OrchestratorPanelStatus::Live)
    }

    fn ticket_enabled_app_with_orchestrator(
        live: Vec<LivePodInfo>,
        orchestrator_status: OrchestratorPanelStatus,
    ) -> MultiPodApp {
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

    fn app_with_list(list: PodList) -> MultiPodApp {
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

    fn app_with_panel(list: PodList, panel: WorkspacePanelViewModel) -> MultiPodApp {
        let last_companion_lifecycle_failure = companion_lifecycle_failure_from_panel(&panel);
        let last_orchestrator_lifecycle_failure = orchestrator_lifecycle_failure_from_panel(&panel);
        let mut app = MultiPodApp {
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
            workflow_state: TicketWorkflowState::parse(state)
                .unwrap_or(TicketWorkflowState::Planning),
            workflow_state_explicit: true,
            next_action: Some(next_action),
            updated_at: None,
            latest_event_kind: Some("implementation_report".to_string()),
            latest_event_excerpt: Some("latest event stays out of the primary row".to_string()),
            blocked_reason: None,
            related_pods: Vec::new(),
            local_claim: None,
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

    fn live_info_with_updated_at(
        pod_name: &str,
        status: PodStatus,
        updated_at: u64,
    ) -> LivePodInfo {
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
            preview: None,
        }
    }

    fn section_names<'a>(list: &'a PodList, section: &MultiPodSection) -> Vec<&'a str> {
        section
            .entries
            .iter()
            .map(|index| list.entries[*index].name.as_str())
            .collect()
    }

    #[test]
    fn ticket_action_error_records_f2_diagnostic_details() {
        let mut app = MultiPodApp::loading(PodRuntimeCommand::for_executable("/tmp/yoi"));
        let long_error = "root-clean failed for Ticket 00001KTWPE3KQ at /home/hare/Projects/yoi: dirty file crates/tui/src/multi_pod.rs";

        app.finish_ticket_action_dispatch(Err(TicketActionError::Stale(long_error.to_string())));

        assert!(app.notice.as_deref().unwrap().contains("F2 details"));
        let diagnostic = app.panel_diagnostic.as_ref().expect("diagnostic");
        assert_eq!(diagnostic.title, "Ticket action rejected");
        assert_eq!(diagnostic.details, long_error);
        assert!(!app.panel_diagnostic_open);

        assert!(matches!(
            app.handle_key(key(KeyCode::F(2))),
            MultiPodAction::None
        ));
        assert!(app.panel_diagnostic_open);
        assert!(matches!(
            app.handle_key(key(KeyCode::Esc)),
            MultiPodAction::None
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

    fn input_text(app: &MultiPodApp) -> String {
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
}
