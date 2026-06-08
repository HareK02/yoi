use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use client::ticket_role::{
    TicketIntakeHandoff, TicketRef, TicketRole, TicketRoleLaunchContext, TicketRoleLaunchError,
    TicketRoleLaunchOptions, TicketRoleLaunchResult, launch_ticket_role_pod,
    launch_ticket_role_pod_with_options, plan_ticket_role_launch,
};
use client::{PodRuntimeCommand, SpawnConfig, spawn_pod};
use crossterm::event::{Event as TermEvent, KeyCode, KeyEvent, KeyModifiers, poll, read};
use pod_store::FsPodStore;
use protocol::stream::{JsonLineReader, JsonLineWriter};
use protocol::{ErrorCode, Event, Method, PodStatus, Segment};
use ratatui::Frame;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};
use session_store::FsStore;
use ticket::config::TicketConfig;
use ticket::{
    LocalTicketBackend, NewTicketEvent, TicketBackend, TicketEventKind, TicketIdOrSlug,
    TicketStatus, TicketWorkflowState,
};
use tokio::net::UnixStream;
use unicode_width::UnicodeWidthStr;

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
    if let Some(mode) = app.enter_reload.take() {
        if pending_reload.start(mode) {
            app.refreshing = true;
        }
    }
    let mut next_poll = Instant::now() + MULTI_POD_POLL_INTERVAL;

    loop {
        if let Some(result) = pending_reload.finish_if_ready().await {
            app.apply_reload_result(result);
        }

        terminal.draw(|f| draw(f, app))?;

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
                MultiPodAction::Quit => return Ok(MultiPodOutcome::Quit),
                MultiPodAction::Open => {
                    if let Some(request) = app.prepare_open() {
                        terminal.draw(|f| draw(f, app))?;
                        return Ok(MultiPodOutcome::Open(request));
                    }
                }
                MultiPodAction::DispatchTicketAction(request) => {
                    pending_reload.abort();
                    terminal.draw(|f| draw(f, app))?;
                    let result = dispatch_ticket_action(request).await;
                    app.finish_ticket_action_dispatch(result);
                    if pending_reload.start(OrchestratorLifecycleMode::Observe) {
                        app.refreshing = true;
                    }
                    next_poll = Instant::now() + MULTI_POD_POLL_INTERVAL;
                }
                MultiPodAction::LaunchIntake(request) => {
                    pending_reload.abort();
                    terminal.draw(|f| draw(f, app))?;
                    let result = launch_intake_with_handoff(request).await;
                    app.finish_intake_launch(result);
                    if pending_reload.start(OrchestratorLifecycleMode::Observe) {
                        app.refreshing = true;
                    }
                    next_poll = Instant::now() + MULTI_POD_POLL_INTERVAL;
                }
                MultiPodAction::SendCompanion(request) => {
                    pending_reload.abort();
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
        self.handle = Some(tokio::spawn(async move {
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
        Some(match handle.await {
            Ok(result) => result,
            Err(e) => Err(MultiPodError::Io(io::Error::other(format!(
                "reload task failed: {e}"
            )))),
        })
    }

    fn abort(&mut self) {
        if let Some(handle) = self.handle.take() {
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

pub(crate) struct MultiPodApp {
    pub(crate) list: PodList,
    pub(crate) panel: WorkspacePanelViewModel,
    pub(crate) input: InputBuffer,
    selected_row: Option<PanelRowKey>,
    composer_target: ComposerTarget,
    notice: Option<String>,
    sending: bool,
    refreshing: bool,
    enter_reload: Option<OrchestratorLifecycleMode>,
    runtime_command: PodRuntimeCommand,
    last_companion_lifecycle_failure: Option<CompanionPanelState>,
    last_orchestrator_lifecycle_failure: Option<OrchestratorPanelState>,
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
            composer_target: ComposerTarget::Companion,
            notice: None,
            sending: false,
            refreshing: true,
            enter_reload: Some(OrchestratorLifecycleMode::Ensure {
                runtime_command: runtime_command.clone(),
            }),
            runtime_command,
            last_companion_lifecycle_failure: None,
            last_orchestrator_lifecycle_failure: None,
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
                        "Empty Enter dispatches this Ticket action; stale Tickets are re-checked before any mutation."
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
        self.selected_row = Some(key);
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
        let (ticket_id, ticket_slug) = {
            let Some(ticket) = row.ticket.as_ref() else {
                self.notice = Some("No Ticket action is selected.".to_string());
                return None;
            };
            (ticket.id.clone(), ticket.slug.clone())
        };
        let orchestrator = ticket_action_orchestrator_target(&self.panel, &self.list);
        self.sending = true;
        self.notice = Some(format!(
            "Dispatching {} for Ticket {}…",
            action.label(),
            ticket_slug
        ));
        Some(TicketActionRequest {
            workspace_root: current_workspace_root(),
            ticket_id,
            action,
            orchestrator,
        })
    }

    pub(crate) fn finish_ticket_action_dispatch(
        &mut self,
        result: Result<TicketActionOutcome, TicketActionError>,
    ) {
        self.sending = false;
        self.notice = Some(match result {
            Ok(outcome) => outcome.notice,
            Err(error) => bounded_panel_diagnostic(error.to_string()),
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
        let ticket_slug = ticket.slug.clone();
        let mut context =
            TicketRoleLaunchContext::new(current_workspace_root(), TicketRole::Intake);
        context.ticket = Some(TicketRef {
            id: Some(ticket_id.clone()),
            slug: Some(ticket_slug.clone()),
        });
        context.user_instruction = Some(format!(
            "Continue Intake for existing Ticket {ticket_id} ({ticket_slug}). Do not create a duplicate Ticket unless the user explicitly requests one."
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
            ticket_slug, planned.pod_name
        ));
        Some(IntakeLaunchRequest {
            context,
            runtime_command: self.runtime_command.clone(),
            peer_registration,
            registry_update: IntakeRegistryUpdate::ClaimTicket {
                registry_root: store.root().to_path_buf(),
                ticket_id,
                ticket_slug: Some(ticket_slug),
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

    fn handle_key(&mut self, key: KeyEvent) -> MultiPodAction {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Char('d') if ctrl => MultiPodAction::Quit,
            KeyCode::Char('c') if ctrl => MultiPodAction::Quit,
            KeyCode::Esc => MultiPodAction::Quit,
            KeyCode::Up => {
                self.select_prev();
                MultiPodAction::None
            }
            KeyCode::Down => {
                self.select_next();
                MultiPodAction::None
            }
            KeyCode::Char('t') if ctrl => {
                self.cycle_composer_target();
                MultiPodAction::None
            }
            KeyCode::Enter if alt => {
                self.input.insert_newline();
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
            KeyCode::Enter if self.composer_target == ComposerTarget::TicketIntake => self
                .prepare_intake_launch()
                .map(MultiPodAction::LaunchIntake)
                .unwrap_or(MultiPodAction::None),
            KeyCode::Enter if self.composer_is_blank() => MultiPodAction::Open,
            KeyCode::Enter => self
                .prepare_companion_send()
                .map(MultiPodAction::SendCompanion)
                .unwrap_or(MultiPodAction::None),
            KeyCode::Backspace => {
                self.input.delete_before();
                MultiPodAction::None
            }
            KeyCode::Delete => {
                self.input.delete_after();
                MultiPodAction::None
            }
            KeyCode::Left => {
                self.input.move_left();
                MultiPodAction::None
            }
            KeyCode::Right => {
                self.input.move_right();
                MultiPodAction::None
            }
            KeyCode::Home => {
                self.input.move_home();
                MultiPodAction::None
            }
            KeyCode::End => {
                self.input.move_end();
                MultiPodAction::None
            }
            KeyCode::Char(c) if !ctrl => {
                self.input.insert_char(c);
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
            match restore_orchestrator_pod(workspace_root, &pod_name, runtime_command.clone()).await
            {
                Ok(()) => OrchestratorLifecycleReport::with_state(OrchestratorPanelState::new(
                    pod_name,
                    OrchestratorPanelStatus::Restored,
                    Some("restored existing Pod state".to_string()),
                ))
                .mark_reload(),
                Err(error) => OrchestratorLifecycleReport::unavailable(
                    pod_name,
                    format!("could not restore workspace Orchestrator: {error}"),
                ),
            }
        }
        OrchestratorLifecyclePlan::Spawn => {
            match spawn_orchestrator_pod(workspace_root, &pod_name, runtime_command).await {
                Ok(profile) => {
                    OrchestratorLifecycleReport::with_state(OrchestratorPanelState::new(
                        pod_name,
                        OrchestratorPanelStatus::Spawned,
                        Some(format!("launched with profile {profile}")),
                    ))
                    .mark_reload()
                }
                Err(error) => OrchestratorLifecycleReport::unavailable(
                    pod_name,
                    format!("could not spawn workspace Orchestrator: {error}"),
                ),
            }
        }
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
        resume_from: None,
    };
    spawn_pod(config, |_| {}).await.map(|_| ())
}

async fn restore_orchestrator_pod(
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
        resume_from: None,
    };
    spawn_pod(config, |_| {}).await.map(|_| ())
}

async fn spawn_orchestrator_pod(
    workspace_root: &Path,
    pod_name: &str,
    runtime_command: PodRuntimeCommand,
) -> Result<String, client::TicketRoleLaunchError> {
    let mut context =
        TicketRoleLaunchContext::new(workspace_root.to_path_buf(), TicketRole::Orchestrator);
    context.pod_name = Some(pod_name.to_string());
    context.user_instruction = Some(
        "Workspace panel opened for this Ticket-enabled workspace. Coordinate Ticket routing and wait for explicit follow-up before spawning role Pods."
            .to_string(),
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
            if current_ticket.workflow_state != TicketWorkflowState::Ready {
                return Err(TicketActionError::Stale(
                    "Queue is only valid while workflow_state is ready; reload and retry"
                        .to_string(),
                ));
            }
            backend
                .queue_ready(
                    TicketIdOrSlug::Id(request.ticket_id.clone()),
                    "workspace-panel",
                )
                .map_err(|error| TicketActionError::Ticket(error.to_string()))?;
            let notification =
                notify_workspace_orchestrator(request.orchestrator, current_ticket).await;
            Ok(TicketActionOutcome {
                notice: format!(
                    "Queued Ticket {}; {}. Orchestrator routing is authorized; implementation side effects still require queued -> inprogress acceptance.",
                    current_ticket.slug,
                    notification.sentence()
                ),
            })
        }
        NextUserAction::Defer => {
            append_panel_decision(
                &backend,
                &request.ticket_id,
                panel_defer_body(current_ticket),
            )?;
            let mut moved = false;
            if current_ticket
                .status
                .eq_ignore_ascii_case(TicketStatus::Open.as_str())
            {
                backend
                    .set_status(
                        TicketIdOrSlug::Id(request.ticket_id.clone()),
                        TicketStatus::Pending,
                    )
                    .map_err(|error| TicketActionError::Ticket(error.to_string()))?;
                moved = true;
            }
            let notice = if moved {
                format!(
                    "Recorded Panel Defer for Ticket {} and moved it to pending.",
                    current_ticket.slug
                )
            } else {
                format!(
                    "Recorded Panel Defer for Ticket {}; status was already {}.",
                    current_ticket.slug, current_ticket.status
                )
            };
            Ok(TicketActionOutcome { notice })
        }
        NextUserAction::Close => unreachable!("Close action is handled before row dispatch"),
        NextUserAction::Clarify
        | NextUserAction::Edit
        | NextUserAction::OpenPod
        | NextUserAction::Wait => Ok(TicketActionOutcome {
            notice: format!(
                "{} for Ticket {} has no safe inline workspace-panel dispatch; use the Ticket workflow.",
                request.action.label(),
                current_ticket.slug
            ),
        }),
    }
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
            "Closed Ticket {}; deterministic resolution recorded because workflow_state was already done.",
            ticket.meta.slug
        ),
    })
}

fn panel_close_blocker(ticket: &ticket::Ticket) -> Option<String> {
    let slug = ticket.meta.slug.as_str();
    if ticket.meta.status.as_local() != Some(TicketStatus::Open) {
        return Some(format!(
            "Close blocked for Ticket {slug}: local status is {}, expected open; no close was recorded.",
            ticket.meta.status.as_str()
        ));
    }
    if ticket.meta.workflow_state != TicketWorkflowState::Done {
        return Some(format!(
            "Close blocked for Ticket {slug}: workflow_state is {}, expected done; no close was recorded.",
            ticket.meta.workflow_state.as_str()
        ));
    }
    if let Some(reason) = non_empty_ticket_field(ticket.meta.attention_required.as_deref()) {
        return Some(format!(
            "Close blocked for Ticket {slug}: attention_required is set ({}); no close was recorded.",
            bounded_panel_diagnostic(reason)
        ));
    }
    if let Some(reason) = non_empty_ticket_field(ticket.meta.action_required.as_deref()) {
        return Some(format!(
            "Close blocked for Ticket {slug}: action_required is set ({}); no close was recorded.",
            bounded_panel_diagnostic(reason)
        ));
    }
    if ticket.resolution.is_some() {
        return Some(format!(
            "Close blocked for Ticket {slug}: resolution.md already exists; no close was recorded."
        ));
    }
    None
}

fn non_empty_ticket_field(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn panel_close_resolution(
    ticket: &ticket::Ticket,
    record_language: Option<&str>,
) -> ticket::MarkdownText {
    if is_japanese_ticket_record_language(record_language) {
        ticket::MarkdownText::new(format!(
            "Ticket `{}` (`{}`) はすでに `workflow_state: done` に到達していたため、workspace Panel から close しました。\n\nこの Close action によって、実装作業、workflow-state 変更、Orchestrator/Companion launch、worker invocation は開始されていません。\n",
            ticket.meta.slug, ticket.meta.id
        ))
    } else {
        ticket::MarkdownText::new(format!(
            "Closed from the workspace Panel because Ticket `{}` (`{}`) had already reached `workflow_state: done`.\n\nNo implementation work, workflow-state change, Orchestrator/Companion launch, or worker invocation was started by this Close action.\n",
            ticket.meta.slug, ticket.meta.id
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

fn append_panel_decision(
    backend: &LocalTicketBackend,
    ticket_id: &str,
    body: String,
) -> Result<(), TicketActionError> {
    let mut event = NewTicketEvent::new(TicketEventKind::Decision, body);
    event.author = Some("workspace-panel".to_string());
    backend
        .add_event(TicketIdOrSlug::Id(ticket_id.to_owned()), event)
        .map_err(|error| TicketActionError::Ticket(error.to_string()))
}

fn panel_defer_body(ticket: &crate::workspace_panel::TicketPanelEntry) -> String {
    format!(
        "Panel Defer recorded by a human for Ticket `{}` (`{}`). Keep this Ticket out of immediate Orchestrator routing until a later explicit Queue; no scheduler or implementation Pod was started.",
        ticket.slug, ticket.id
    )
}

fn orchestrator_queue_notification_message(
    ticket: &crate::workspace_panel::TicketPanelEntry,
) -> String {
    let title = ticket.title.replace(['\r', '\n'], " ");
    format!(
        "Workspace panel Queue for Ticket `{}` (`{}`), title `{}`: human authorized Orchestrator routing; this is not an unattended scheduler. Read the Ticket and inspect current workspace state. If unblocked, record routing and transition workflow_state queued -> inprogress before any worktree/SpawnPod implementation side effects. After inprogress acceptance, use worktree-workflow for `.worktree/<task-name>` creation with tracked `.yoi` project records visible and `.yoi/memory` plus local/runtime/log/lock/secret-like `.yoi` paths excluded, then use multi-agent-workflow to run sibling coder/reviewer Pods (coder narrow child-worktree write scope, reviewer read-only by default) and stop at a merge-ready dossier without merge/close/final approval. If blocked, record a concise reason and leave the Ticket queued or explicitly defer it.",
        ticket.slug,
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
    match send_notify_only(&target.socket_path, message).await {
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

async fn send_notify_only(socket: &Path, message: String) -> Result<(), NotifySendError> {
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

    tokio::time::timeout(SOCKET_OP_TIMEOUT, writer.write(&Method::Notify { message }))
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
                "Selected Pod is running; press empty Enter to open/attach.".to_string()
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
        return "Selected Pod is stopped; press empty Enter to restore/open.".to_string();
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
                "Press Enter to dispatch {action} for Ticket '{}' after re-checking current Ticket authority.",
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
        "  Empty Enter dispatches selected Ticket action/open · Ctrl+T target"
    } else if app.panel.header.ticket_configured {
        "  Empty Enter dispatches selected Ticket action/open Pod"
    } else {
        "  Pod-centric view · empty Enter open/attach selected Pod"
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

fn draw_list(frame: &mut Frame<'_>, app: &MultiPodApp, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let lines = list_lines(app, area.width, area.height);
    Paragraph::new(lines).render(area, frame.buffer_mut());
}

fn list_lines(app: &MultiPodApp, width: u16, height: u16) -> Vec<Line<'static>> {
    let sections = sectioned_entries(&app.list);
    let selected = app.selected_row.as_ref();
    let diagnostic_lines = panel_diagnostic_lines(&app.panel, width);
    let action_lines = panel_action_lines(&app.panel, selected, width);
    let live_lines = sections
        .iter()
        .filter(|section| section.kind != MultiPodSectionKind::Closed)
        .flat_map(|section| section_lines(&app.list, section, selected, width))
        .collect::<Vec<_>>();
    let closed_lines = sections
        .iter()
        .find(|section| section.kind == MultiPodSectionKind::Closed)
        .map(|section| section_lines(&app.list, section, selected, width))
        .unwrap_or_default();

    let available = height as usize;
    let diagnostic_len = diagnostic_lines.len().min(available);
    let remaining_after_diagnostics = available.saturating_sub(diagnostic_len);
    let action_len = action_lines.len().min(remaining_after_diagnostics);
    let remaining_after_actions = remaining_after_diagnostics.saturating_sub(action_len);
    let closed_len = closed_lines.len().min(remaining_after_actions);
    let live_len = live_lines
        .len()
        .min(remaining_after_actions.saturating_sub(closed_len));
    let spacer_len = available.saturating_sub(diagnostic_len + action_len + live_len + closed_len);

    let mut lines = Vec::with_capacity(available);
    lines.extend(diagnostic_lines.into_iter().take(diagnostic_len));
    lines.extend(action_lines.into_iter().take(action_len));
    lines.extend(live_lines.into_iter().take(live_len));
    lines.extend(std::iter::repeat_with(|| Line::from(Span::raw(""))).take(spacer_len));
    lines.extend(closed_lines.into_iter().take(closed_len));
    lines
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

fn panel_action_lines(
    panel: &WorkspacePanelViewModel,
    selected: Option<&PanelRowKey>,
    width: u16,
) -> Vec<Line<'static>> {
    let rows = panel
        .rows
        .iter()
        .filter(|row| row.is_ticket_action())
        .collect::<Vec<_>>();
    if rows.is_empty() {
        return Vec::new();
    }
    let mut lines = Vec::with_capacity(rows.len() + 1);
    lines.push(panel_action_header_line(rows.len(), width));
    for row in rows {
        lines.push(panel_row_line(row, selected == Some(&row.key), width));
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
const TICKET_ID_COLUMN_WIDTH: usize = 32;
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
        .map(|ticket| {
            if ticket.slug.is_empty() {
                ticket.id.clone()
            } else {
                ticket.slug.clone()
            }
        })
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
        ActionPriority::Blocked => Style::default().fg(Color::Red),
        ActionPriority::ActiveWork => Style::default().fg(Color::Cyan),
        ActionPriority::Background => Style::default().fg(Color::DarkGray),
    }
}

fn section_lines(
    list: &PodList,
    section: &MultiPodSection,
    selected: Option<&PanelRowKey>,
    width: u16,
) -> Vec<Line<'static>> {
    let visible = visible_section_indices(section);
    if visible.is_empty() {
        return Vec::new();
    }

    let mut lines = Vec::with_capacity(visible.len() + 1);
    lines.push(section_header_line(
        section.kind,
        section.entries.len(),
        section.hidden_count(),
        width,
    ));
    for index in visible {
        if let Some(entry) = list.entries.get(index) {
            let selected = selected == Some(&PanelRowKey::Pod(entry.name.clone()));
            lines.push(row_line(entry, selected, width));
        }
    }
    lines
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
    let target = if let Some(row) = app
        .selected_panel_row()
        .filter(|row| row.is_ticket_action())
    {
        let action = row.next_action.map(NextUserAction::label).unwrap_or("View");
        Line::from(vec![
            Span::styled("composer ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                app.composer_target().label(),
                Style::default()
                    .fg(Color::Magenta)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" · ticket ", Style::default().fg(Color::DarkGray)),
            Span::styled(row.status.clone(), panel_priority_style(row.priority)),
            Span::styled(" · ", Style::default().fg(Color::DarkGray)),
            Span::styled(action, Style::default().fg(Color::Magenta)),
        ])
    } else if let Some(entry) = app.selected_pod_entry() {
        let (status, status_style) = row_status_label(entry);
        Line::from(vec![
            Span::styled("composer ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                app.composer_target().label(),
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" · pod ", Style::default().fg(Color::DarkGray)),
            Span::styled(status.to_string(), status_style),
        ])
    } else {
        Line::from(vec![
            Span::styled("composer ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                app.composer_target().label(),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(" · no selection", Style::default().fg(Color::DarkGray)),
        ])
    };
    frame.render_widget(Paragraph::new(target), area);
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

fn draw_actionbar(frame: &mut Frame<'_>, app: &MultiPodApp, area: Rect) {
    let left = if app.sending && app.composer_target() == ComposerTarget::TicketIntake {
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
    } else if let Some(notice) = app.notice.as_deref() {
        notice.to_string()
    } else if let Some(reason) = app.selected_open_disabled_reason() {
        reason
    } else {
        match app.composer_target() {
            ComposerTarget::Companion => {
                "Companion target pending; non-empty Enter keeps draft and reports a diagnostic"
                    .to_string()
            }
            ComposerTarget::TicketIntake => {
                "Ticket Intake target: Enter launches Intake with composer text".to_string()
            }
        }
    };
    let right = if app
        .panel
        .composer
        .is_available(ComposerTarget::TicketIntake)
    {
        "↑/↓ select  Empty Enter target/open  Ctrl+T target  Esc quit"
    } else {
        "↑/↓ select  Empty Enter open  non-empty Enter diagnose  Esc quit"
    };
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
    use crate::pod_list::{LivePodInfo, PodEntrySummary, StoredMetadataState, StoredPodInfo};
    use std::fs;
    use tempfile::TempDir;
    use ticket::{
        LocalTicketBackend, MarkdownText, NewTicket, TicketBackend, TicketEventKind,
        TicketWorkflowState,
    };

    fn ticket_workspace(
        slug: &str,
        workflow_state: TicketWorkflowState,
        configure: impl FnOnce(&mut NewTicket),
    ) -> (TempDir, String, LocalTicketBackend) {
        let temp = TempDir::new().unwrap();
        fs::create_dir_all(temp.path().join(".yoi")).unwrap();
        fs::write(
            temp.path().join(".yoi/ticket.config.toml"),
            "[backend]\nprovider = \"builtin:yoi_local\"\nroot = \".yoi/tickets\"\n",
        )
        .unwrap();
        let backend = LocalTicketBackend::new(temp.path().join(".yoi/tickets"));
        let mut input = NewTicket {
            slug: Some(slug.to_string()),
            title: "Ready panel ticket".to_string(),
            body: MarkdownText::from("Ready for panel action"),
            kind: "task".to_string(),
            priority: "P2".to_string(),
            author: None,
            assignee: None,
            labels: Vec::new(),
            readiness: None,
            action_required: None,
            workflow_state: Some(workflow_state),
            attention_required: None,
            queued_by: None,
            queued_at: None,
            risk_flags: Vec::new(),
        };
        configure(&mut input);
        let ticket = backend.create(input).unwrap();
        (temp, ticket.id, backend)
    }

    fn ready_ticket_workspace(slug: &str) -> (TempDir, String, LocalTicketBackend) {
        ticket_workspace(slug, TicketWorkflowState::Ready, |_| {})
    }

    fn done_ticket_workspace(slug: &str) -> (TempDir, String, LocalTicketBackend) {
        ticket_workspace(slug, TicketWorkflowState::Done, |_| {})
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
        let (temp, ticket_id, backend) = ready_ticket_workspace("panel-queue");

        let outcome =
            dispatch_ticket_action(request_for(&temp, ticket_id.clone(), NextUserAction::Queue))
                .await
                .unwrap();

        assert!(outcome.notice.contains("Queued Ticket"));
        assert!(
            outcome
                .notice
                .contains("Orchestrator routing is authorized")
        );
        assert!(outcome.notice.contains("queued -> inprogress acceptance"));
        assert!(!outcome.notice.contains("No implementation was started"));
        let ticket = backend.show(TicketIdOrSlug::Id(ticket_id)).unwrap();
        assert_eq!(ticket.meta.status.as_local(), Some(TicketStatus::Open));
        assert_eq!(ticket.meta.workflow_state, TicketWorkflowState::Queued);
        assert_eq!(ticket.meta.queued_by.as_deref(), Some("workspace-panel"));
        assert!(ticket.meta.queued_at.is_some());
        let state_change = ticket
            .events
            .iter()
            .find(|event| {
                event.kind == TicketEventKind::StateChanged
                    && event.state_field.as_deref() == Some("workflow_state")
                    && event.from.as_deref() == Some("ready")
                    && event.to.as_deref() == Some("queued")
            })
            .expect("queue state_changed event is recorded");
        assert_eq!(state_change.author.as_deref(), Some("workspace-panel"));
    }

    #[tokio::test]
    async fn ticket_close_action_blocks_non_done_ticket_without_mutation() {
        let (temp, ticket_id, backend) = ready_ticket_workspace("panel-not-done");

        let error =
            dispatch_ticket_action(request_for(&temp, ticket_id.clone(), NextUserAction::Close))
                .await
                .unwrap_err();

        assert!(error.to_string().contains("workflow_state is ready"));
        let ticket = backend.show(TicketIdOrSlug::Id(ticket_id)).unwrap();
        assert_eq!(ticket.meta.status.as_local(), Some(TicketStatus::Open));
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
                && event.state_field.as_deref() == Some("workflow_state")
        }));
    }

    #[tokio::test]
    async fn ticket_defer_action_records_decision_for_pending_ticket() {
        let (temp, ticket_id, backend) = ready_ticket_workspace("panel-defer");
        backend
            .set_status(TicketIdOrSlug::Id(ticket_id.clone()), TicketStatus::Pending)
            .unwrap();

        let outcome =
            dispatch_ticket_action(request_for(&temp, ticket_id.clone(), NextUserAction::Defer))
                .await
                .unwrap();

        assert!(outcome.notice.contains("Recorded Panel Defer"));
        let ticket = backend.show(TicketIdOrSlug::Id(ticket_id)).unwrap();
        assert_eq!(ticket.meta.status.as_local(), Some(TicketStatus::Pending));
        assert!(ticket.events.iter().any(|event| {
            event.kind == TicketEventKind::Decision
                && event.body.as_str().contains("Panel Defer recorded")
        }));
    }

    #[tokio::test]
    async fn ticket_close_action_closes_done_ticket_with_deterministic_resolution() {
        let (temp, ticket_id, backend) = done_ticket_workspace("panel-close");

        let outcome =
            dispatch_ticket_action(request_for(&temp, ticket_id.clone(), NextUserAction::Close))
                .await
                .unwrap();

        assert!(outcome.notice.contains("Closed Ticket panel-close"));
        assert!(outcome.notice.contains("workflow_state was already done"));
        let ticket = backend.show(TicketIdOrSlug::Id(ticket_id)).unwrap();
        assert_eq!(ticket.meta.status.as_local(), Some(TicketStatus::Closed));
        assert_eq!(ticket.meta.workflow_state, TicketWorkflowState::Done);
        let resolution = ticket
            .resolution
            .as_ref()
            .expect("Panel Close records resolution.md")
            .as_str();
        assert!(resolution.contains("workflow_state: done"));
        assert!(resolution.contains("No implementation work"));
        assert!(resolution.contains("workflow-state change"));
        assert!(resolution.contains("worker invocation"));
        assert!(ticket.events.iter().any(|event| {
            event.kind == TicketEventKind::Close && event.body.as_str().contains("workspace Panel")
        }));
    }

    #[tokio::test]
    async fn ticket_close_action_blocks_action_required_without_mutation() {
        let (temp, ticket_id, backend) = ticket_workspace(
            "panel-close-action-required",
            TicketWorkflowState::Done,
            |input| {
                input.action_required = Some("human decision needed".to_string());
            },
        );

        let error =
            dispatch_ticket_action(request_for(&temp, ticket_id.clone(), NextUserAction::Close))
                .await
                .unwrap_err();

        assert!(error.to_string().contains("action_required is set"));
        assert!(error.to_string().contains("no close was recorded"));
        let ticket = backend.show(TicketIdOrSlug::Id(ticket_id)).unwrap();
        assert_eq!(ticket.meta.status.as_local(), Some(TicketStatus::Open));
        assert_eq!(ticket.meta.workflow_state, TicketWorkflowState::Done);
        assert!(ticket.resolution.is_none());
    }

    #[tokio::test]
    async fn ticket_close_action_blocks_attention_required_without_mutation() {
        let (temp, ticket_id, backend) = ticket_workspace(
            "panel-close-attention-required",
            TicketWorkflowState::Done,
            |input| {
                input.attention_required = Some("needs reply".to_string());
            },
        );

        let error =
            dispatch_ticket_action(request_for(&temp, ticket_id.clone(), NextUserAction::Close))
                .await
                .unwrap_err();

        assert!(error.to_string().contains("attention_required is set"));
        let ticket = backend.show(TicketIdOrSlug::Id(ticket_id)).unwrap();
        assert_eq!(ticket.meta.status.as_local(), Some(TicketStatus::Open));
        assert!(ticket.resolution.is_none());
    }

    #[tokio::test]
    async fn ticket_close_action_blocks_existing_resolution_without_moving_ticket() {
        let (temp, ticket_id, backend) = done_ticket_workspace("panel-close-resolution");
        fs::write(
            temp.path()
                .join(".yoi/tickets/open")
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
        assert_eq!(ticket.meta.status.as_local(), Some(TicketStatus::Open));
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
            "route-ticket",
            "Route queued\nTicket",
            ActionPriority::ReadyForQueue,
            NextUserAction::Queue,
            "queued",
        );
        let ticket = row.ticket.as_ref().unwrap();

        let message = orchestrator_queue_notification_message(ticket);

        assert!(message.contains("Ticket `route-ticket` (`20260606-000000-route-ticket`)"));
        assert!(message.contains("title `Route queued Ticket`"));
        assert!(message.contains("human authorized Orchestrator routing"));
        assert!(message.contains("not an unattended scheduler"));
        assert!(message.contains("Read the Ticket"));
        assert!(message.contains("inspect current workspace state"));
        assert!(message.contains("transition workflow_state queued -> inprogress"));
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
        assert!(message.contains("merge-ready dossier"));
        assert!(message.contains("without merge/close/final approval"));
        assert!(message.contains("If blocked, record a concise reason"));
        assert!(message.contains("leave the Ticket queued or explicitly defer"));
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

        send_notify_only(&socket_path, "panel Queue".to_string())
            .await
            .unwrap();
        let method = server.await.unwrap();
        assert!(matches!(
            method,
            Method::Notify { message } if message == "panel Queue"
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
        let mut ticket = NewTicket::new("Needs Human Reply");
        ticket.slug = Some("needs-human-reply".to_string());
        ticket.action_required = Some("answer intake question".to_string());
        ticket.labels = vec!["intake".to_string()];
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

        assert_eq!(app.selected_panel_row().unwrap().title, "Needs Human Reply");
        assert_eq!(app.selected_open_eligibility(), OpenEligibility::Disabled);
        let lines = list_lines(&app, 100, 6)
            .into_iter()
            .map(|line| plain_line(&line))
            .collect::<Vec<_>>();
        let ticket_line = lines
            .iter()
            .position(|line| line.contains("needs-human-reply"))
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
    fn multi_bare_panel_letters_append_to_composer_and_arrows_select() {
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
        assert_eq!(app.list.selected_entry().unwrap().name, "beta");

        assert!(matches!(
            app.handle_key(key(KeyCode::Up)),
            MultiPodAction::None
        ));
        assert_eq!(input_text(&app), "jkor");
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
            "workspace-panel-composer-targets",
            "Workspace panel composer targets",
            ActionPriority::ActiveWork,
            NextUserAction::Wait,
            "inprogress",
        );
        let ready_row = panel_test_ticket_row(
            "ticket-slug",
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
        assert_eq!(
            display_column(&review_line, "workspace-panel-composer-targets"),
            id_start
        );
        assert_eq!(display_column(&ready_line, "ticket-slug"), id_start);
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
            "ticket-slug",
            "Very long Ticket title that should truncate only after the aligned short columns",
            ActionPriority::ReadyForQueue,
            NextUserAction::Queue,
            "ready",
        );

        let line = plain_line(&panel_row_line(&row, false, 112));
        let title_start = 2 + TICKET_STATE_COLUMN_WIDTH + 1 + TICKET_ID_COLUMN_WIDTH + 1;

        assert_eq!(line.width(), 112);
        assert_eq!(
            display_column(&line, "ticket-slug"),
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
    fn multi_composer_target_switch_preserves_typed_text() {
        let mut app = ticket_enabled_app(vec![live_info("idle", PodStatus::Idle)]);
        app.input.insert_str("draft intake request");

        assert!(matches!(app.composer_target(), ComposerTarget::Companion));
        assert!(matches!(
            app.handle_key(modified_key(KeyCode::Char('t'), KeyModifiers::CONTROL)),
            MultiPodAction::None
        ));

        assert!(matches!(
            app.composer_target(),
            ComposerTarget::TicketIntake
        ));
        assert_eq!(input_text(&app), "draft intake request");
        assert!(app.notice.as_deref().unwrap().contains("Ticket Intake"));
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
    fn multi_ticket_intake_rejects_empty_input() {
        let mut app = ticket_enabled_app(vec![live_info("idle", PodStatus::Idle)]);
        app.cycle_composer_target();
        app.input.insert_str("  \n\t");

        assert!(matches!(
            app.handle_key(key(KeyCode::Enter)),
            MultiPodAction::None
        ));

        assert!(matches!(
            app.composer_target(),
            ComposerTarget::TicketIntake
        ));
        assert!(!app.sending);
        assert_eq!(input_text(&app), "  \n\t");
        assert!(app.notice.as_deref().unwrap().contains("input is empty"));
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
            composer_target: ComposerTarget::Companion,
            notice: None,
            sending: false,
            refreshing: false,
            enter_reload: None,
            runtime_command: PodRuntimeCommand::for_executable("/tmp/yoi"),
            last_companion_lifecycle_failure,
            last_orchestrator_lifecycle_failure,
        };
        app.ensure_selection_visible();
        app.ensure_composer_target_available();
        app
    }

    fn panel_test_ticket_row(
        slug: &str,
        title: &str,
        priority: ActionPriority,
        next_action: NextUserAction,
        status: &str,
    ) -> PanelRow {
        let ticket = crate::workspace_panel::TicketPanelEntry {
            id: format!("20260606-000000-{slug}"),
            slug: slug.to_string(),
            title: title.to_string(),
            status: "open".to_string(),
            kind: "task".to_string(),
            priority: "P2".to_string(),
            labels: Vec::new(),
            workflow_state: TicketWorkflowState::parse(status)
                .unwrap_or(TicketWorkflowState::Planning),
            workflow_state_explicit: true,
            attention_required: None,
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
            subtitle: Some("slug · priority · latest event".to_string()),
            status: status.to_string(),
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

    fn modified_key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }
}
