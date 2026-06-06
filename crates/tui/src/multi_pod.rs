use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use client::ticket_role::{
    TicketRole, TicketRoleLaunchContext, TicketRoleLaunchError, TicketRoleLaunchResult,
    launch_ticket_role_pod,
};
use client::{PodRuntimeCommand, SpawnConfig, spawn_pod};
use crossterm::event::{Event as TermEvent, KeyCode, KeyEvent, KeyModifiers, poll, read};
use pod_store::FsPodStore;
use protocol::stream::{JsonLineReader, JsonLineWriter};
use protocol::{ErrorCode, Event, InvokeKind, Method, PodStatus, Segment};
use ratatui::Frame;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};
use session_store::FsStore;
use tokio::net::UnixStream;
use unicode_width::UnicodeWidthStr;

use crate::input::InputBuffer;
use crate::pod_list::{
    PodList, PodListEntry, PodVisibilitySource, StoredMetadataState, read_reachable_live_pod_infos,
    read_stored_pod_infos,
};
use crate::workspace_panel::{
    ActionPriority, ComposerTarget, NextUserAction, OrchestratorLifecyclePlan,
    OrchestratorPanelState, OrchestratorPanelStatus, OrchestratorPodPresence, PanelRow,
    PanelRowKey, TicketConfigAvailability, WorkspacePanelViewModel, bounded_panel_diagnostic,
    build_workspace_panel, decide_orchestrator_lifecycle, orchestrator_pod_presence,
    ticket_config_availability, workspace_orchestrator_pod_name,
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
    MultiPodApp::load(None, runtime_command).await
}

pub(crate) async fn run(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut MultiPodApp,
) -> Result<MultiPodOutcome, MultiPodError> {
    if app.panel.rows.is_empty() && app.panel.header.diagnostics.is_empty() {
        return Err(MultiPodError::NoPods);
    }

    let mut pending_reload = PendingReload::default();
    let mut next_poll = Instant::now() + MULTI_POD_POLL_INTERVAL;

    loop {
        if let Some(result) = pending_reload.finish_if_ready().await {
            app.apply_reload_result(result);
        }

        terminal.draw(|f| draw(f, app))?;

        let now = Instant::now();
        if now >= next_poll {
            pending_reload.start();
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
                        return Ok(MultiPodOutcome::Open(request));
                    }
                }
                MultiPodAction::Refresh => {
                    if !pending_reload.start() {
                        app.notice = Some("Refresh already in progress.".to_string());
                    }
                }
                MultiPodAction::Send(request) => {
                    pending_reload.abort();
                    terminal.draw(|f| draw(f, app))?;
                    let result = send_run_and_confirm(&request.socket_path, request.segments).await;
                    app.finish_send(result);
                    app.reload_or_notice().await;
                    next_poll = Instant::now() + MULTI_POD_POLL_INTERVAL;
                }
                MultiPodAction::LaunchIntake(request) => {
                    pending_reload.abort();
                    terminal.draw(|f| draw(f, app))?;
                    let result =
                        launch_ticket_role_pod(request.context, request.runtime_command, |_| {})
                            .await;
                    app.finish_intake_launch(result);
                    app.reload_or_notice().await;
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
    fn start(&mut self) -> bool {
        if self.handle.is_some() {
            return false;
        }
        self.handle = Some(tokio::spawn(async {
            load_multi_pod_snapshot(None, OrchestratorLifecycleMode::Observe).await
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
pub(crate) enum SendEligibility {
    SendNow,
    Disabled,
}

#[derive(Debug)]
pub(crate) struct DirectSendRequest {
    socket_path: PathBuf,
    segments: Vec<Segment>,
}

#[derive(Debug)]
pub(crate) struct IntakeLaunchRequest {
    context: TicketRoleLaunchContext,
    runtime_command: PodRuntimeCommand,
}

pub(crate) struct MultiPodApp {
    pub(crate) list: PodList,
    pub(crate) panel: WorkspacePanelViewModel,
    pub(crate) input: InputBuffer,
    selected_row: Option<PanelRowKey>,
    composer_target: ComposerTarget,
    notice: Option<String>,
    sending: bool,
    runtime_command: PodRuntimeCommand,
}

impl MultiPodApp {
    async fn load(
        selected_name: Option<String>,
        runtime_command: PodRuntimeCommand,
    ) -> Result<Self, MultiPodError> {
        let snapshot = load_multi_pod_snapshot(
            selected_name,
            OrchestratorLifecycleMode::Ensure {
                runtime_command: runtime_command.clone(),
            },
        )
        .await?;
        let mut app = Self {
            list: snapshot.list,
            panel: snapshot.panel,
            input: InputBuffer::new(),
            selected_row: None,
            composer_target: ComposerTarget::Companion,
            notice: None,
            sending: false,
            runtime_command,
        };
        app.ensure_selection_visible();
        app.ensure_composer_target_available();
        Ok(app)
    }

    pub(crate) async fn reload_or_notice(&mut self) {
        let result = load_multi_pod_snapshot(None, OrchestratorLifecycleMode::Observe).await;
        self.apply_reload_result(result);
    }

    fn apply_reload_result(&mut self, result: Result<MultiPodSnapshot, MultiPodError>) {
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

    fn selected_panel_row(&self) -> Option<&PanelRow> {
        self.selected_row
            .as_ref()
            .and_then(|key| self.panel.row(key))
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
    pub(crate) fn selected_send_eligibility(&self) -> SendEligibility {
        match self.selected_pod_entry() {
            Some(entry) if entry.actions.can_send_now => SendEligibility::SendNow,
            _ => SendEligibility::Disabled,
        }
    }

    pub(crate) fn selected_send_disabled_reason(&self) -> Option<String> {
        if let Some(row) = self
            .selected_panel_row()
            .filter(|row| row.is_ticket_action())
        {
            return Some(
                row.disabled_reason
                    .clone()
                    .or_else(|| row.key_hint.clone())
                    .unwrap_or_else(|| {
                        "Ticket actions are display-only in this first panel slice.".to_string()
                    }),
            );
        }
        let entry = self.selected_pod_entry()?;
        if entry.actions.can_send_now {
            return None;
        }
        Some(send_disabled_reason(entry))
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
        let (pod_name, socket_override) = {
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
            (
                entry.name.clone(),
                entry.attach_socket_path().map(PathBuf::from),
            )
        };
        self.notice = Some(format!("Opening {pod_name}…"));
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
                self.notice = Some(format!("Returned from {pod_name}."));
            }
            Err(error) => {
                self.notice = Some(format!("Open failed for {pod_name}: {error}"));
            }
        }
    }

    fn composer_is_blank(&self) -> bool {
        segments_are_blank(&self.input.submit_segments())
    }

    pub(crate) fn prepare_send(&mut self) -> Option<DirectSendRequest> {
        let (target_name, socket_path) = {
            let entry = match self.selected_pod_entry() {
                Some(entry) => entry,
                None => {
                    self.notice = Some(selected_ticket_notice(self.selected_panel_row()));
                    return None;
                }
            };
            if !entry.actions.can_send_now {
                self.notice = Some(send_disabled_reason(entry));
                return None;
            }
            let Some(socket_path) = entry.attach_socket_path().map(PathBuf::from) else {
                self.notice = Some("Selected Pod has no reachable socket.".to_string());
                return None;
            };
            (entry.name.clone(), socket_path)
        };
        let segments = self.input.submit_segments();
        if segments_are_blank(&segments) {
            self.notice = Some("Composer is empty.".to_string());
            return None;
        }
        self.sending = true;
        self.notice = Some(format!("Sending to {target_name}…"));
        Some(DirectSendRequest {
            socket_path,
            segments,
        })
    }

    pub(crate) fn finish_send(&mut self, result: Result<(), DirectSendError>) {
        self.sending = false;
        match result {
            Ok(()) => {
                let target = self
                    .selected_pod_entry()
                    .map(|entry| entry.name.clone())
                    .unwrap_or_else(|| "selected Pod".to_string());
                self.input.clear();
                self.notice = Some(format!("Delivered to {target}."));
            }
            Err(e) => {
                self.notice = Some(format!("Delivery failed; composer kept: {e}"));
            }
        }
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
        context.user_instruction = Some(body);
        self.sending = true;
        self.notice = Some("Launching Ticket Intake…".to_string());
        Some(IntakeLaunchRequest {
            context,
            runtime_command: self.runtime_command.clone(),
        })
    }

    pub(crate) fn finish_intake_launch(
        &mut self,
        result: Result<TicketRoleLaunchResult, TicketRoleLaunchError>,
    ) {
        self.sending = false;
        match result {
            Ok(result) => {
                let pod_name = result.plan.pod_name;
                self.input.clear();
                self.notice = Some(bounded_panel_diagnostic(format!(
                    "Launched Ticket Intake Pod {pod_name}."
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
            KeyCode::Char('k') if !ctrl && !alt => {
                self.select_prev();
                MultiPodAction::None
            }
            KeyCode::Char('j') if !ctrl && !alt => {
                self.select_next();
                MultiPodAction::None
            }
            KeyCode::Char('t') if ctrl => {
                self.cycle_composer_target();
                MultiPodAction::None
            }
            KeyCode::Char('o') if !ctrl && !alt => MultiPodAction::Open,
            KeyCode::Char('r') if !ctrl && !alt => MultiPodAction::Refresh,
            KeyCode::Enter if alt => {
                self.input.insert_newline();
                MultiPodAction::None
            }
            KeyCode::Enter if self.composer_target == ComposerTarget::TicketIntake => self
                .prepare_intake_launch()
                .map(MultiPodAction::LaunchIntake)
                .unwrap_or(MultiPodAction::None),
            KeyCode::Enter if self.composer_is_blank() => MultiPodAction::Open,
            KeyCode::Enter => self
                .prepare_send()
                .map(MultiPodAction::Send)
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
    Refresh,
    Send(DirectSendRequest),
    LaunchIntake(IntakeLaunchRequest),
}

#[derive(Debug, Clone)]
struct MultiPodSnapshot {
    list: PodList,
    panel: WorkspacePanelViewModel,
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
    let mut list = load_pod_list(selected_name.clone(), MAX_ENTRIES).await?;
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
        list = load_pod_list(selected_name, MAX_ENTRIES).await?;
    }
    let mut panel = build_workspace_panel(&workspace_root, &list);
    panel.header.orchestrator = orchestrator.state;
    panel.header.diagnostics.extend(orchestrator.diagnostics);
    Ok(MultiPodSnapshot { list, panel })
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

async fn restore_orchestrator_pod(
    workspace_root: &Path,
    pod_name: &str,
    runtime_command: PodRuntimeCommand,
) -> Result<(), client::SpawnError> {
    let config = SpawnConfig {
        runtime_command,
        pod_name: pod_name.to_string(),
        profile: None,
        cwd: workspace_root.to_path_buf(),
        resume_from: None,
        resume_by_pod_name: true,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DirectSendError {
    AlreadyRunning,
    Rejected { code: ErrorCode, message: String },
    Io(String),
}

impl std::fmt::Display for DirectSendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AlreadyRunning => write!(f, "target Pod is already running"),
            Self::Rejected { code, message } => {
                write!(f, "target rejected run ({code:?}): {message}")
            }
            Self::Io(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for DirectSendError {}

async fn send_run_and_confirm(socket: &Path, input: Vec<Segment>) -> Result<(), DirectSendError> {
    let stream = tokio::time::timeout(SOCKET_OP_TIMEOUT, UnixStream::connect(socket))
        .await
        .map_err(|_| DirectSendError::Io("connect timed out".into()))?
        .map_err(|e| DirectSendError::Io(format!("connect: {e}")))?;
    let (reader, writer) = stream.into_split();
    let mut reader = JsonLineReader::new(reader);
    let mut writer = JsonLineWriter::new(writer);

    loop {
        let event = tokio::time::timeout(SOCKET_OP_TIMEOUT, reader.next::<Event>())
            .await
            .map_err(|_| DirectSendError::Io("read initial Snapshot timed out".into()))?
            .map_err(|e| DirectSendError::Io(format!("read initial Snapshot: {e}")))?;
        match event {
            Some(Event::Snapshot { .. }) => break,
            Some(Event::Alert(_)) => continue,
            Some(Event::Error {
                code: ErrorCode::AlreadyRunning,
                ..
            }) => return Err(DirectSendError::AlreadyRunning),
            Some(Event::Error { code, message }) => {
                return Err(DirectSendError::Rejected { code, message });
            }
            Some(_) => continue,
            None => {
                return Err(DirectSendError::Io(
                    "connection closed before initial Snapshot".into(),
                ));
            }
        }
    }

    tokio::time::timeout(SOCKET_OP_TIMEOUT, writer.write(&Method::Run { input }))
        .await
        .map_err(|_| DirectSendError::Io("write timed out".into()))?
        .map_err(|e| DirectSendError::Io(format!("write: {e}")))?;

    loop {
        let event = tokio::time::timeout(SOCKET_OP_TIMEOUT, reader.next::<Event>())
            .await
            .map_err(|_| DirectSendError::Io("read response timed out".into()))?
            .map_err(|e| DirectSendError::Io(format!("read response: {e}")))?;
        match event {
            Some(Event::Error {
                code: ErrorCode::AlreadyRunning,
                ..
            }) => return Err(DirectSendError::AlreadyRunning),
            Some(Event::Error { code, message }) => {
                return Err(DirectSendError::Rejected { code, message });
            }
            Some(Event::InvokeStart {
                kind: InvokeKind::UserSend,
            })
            | Some(Event::UserMessage { .. })
            | Some(Event::TurnStart { .. }) => return Ok(()),
            Some(_) => continue,
            None => {
                return Err(DirectSendError::Io(
                    "connection closed before response".into(),
                ));
            }
        }
    }
}

fn segments_are_blank(segments: &[Segment]) -> bool {
    segments.iter().all(|segment| match segment {
        Segment::Text { content } => content.trim().is_empty(),
        _ => false,
    })
}

fn send_disabled_reason(entry: &PodListEntry) -> String {
    if let Some(live) = entry.live.as_ref() {
        if !live.reachable {
            return "Selected live Pod is unreachable.".to_string();
        }
        return match live.status {
            Some(PodStatus::Running) => {
                "Selected Pod is running; direct send is disabled in multi-Pod view.".to_string()
            }
            Some(PodStatus::Paused) => {
                "Selected Pod is paused; open it explicitly to resume or start a new turn."
                    .to_string()
            }
            Some(PodStatus::Idle) => "Selected Pod is not send-eligible.".to_string(),
            None => "Selected Pod did not report a live status.".to_string(),
        };
    }
    if entry.stored.is_some() {
        return "Selected Pod is stopped; open/restore it before sending.".to_string();
    }
    entry
        .actions
        .disabled_reason
        .clone()
        .unwrap_or_else(|| "Selected Pod is not send-eligible.".to_string())
}

fn selected_ticket_notice(row: Option<&PanelRow>) -> String {
    match row {
        Some(row) if row.is_ticket_action() => {
            let action = row.next_action.map(NextUserAction::label).unwrap_or("View");
            format!(
                "{action} for Ticket '{}' is display-only in this slice; use Ticket commands/workflows after re-checking state.",
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
        "  Ticket actions are display-only · Enter uses composer target · Ctrl+T target · o open/attach · r refresh"
    } else if app.panel.header.ticket_configured {
        "  Ticket actions are display-only · Enter sends to selected idle Pod · o open/attach · r refresh"
    } else {
        "  Pod-centric view · Enter sends to selected idle Pod · o open/attach · r refresh"
    };
    let mut spans = vec![
        Span::styled(
            "workspace dashboard",
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::styled(guidance, Style::default().fg(Color::DarkGray)),
    ];
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
    let text = truncate_with_ellipsis(&format!("--actions{detail}---"), width as usize);
    Line::from(Span::styled(
        text,
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    ))
}

fn panel_row_line(row: &PanelRow, selected: bool, width: u16) -> Line<'static> {
    let marker = if selected { "▶ " } else { "  " };
    let action = row.next_action.map(NextUserAction::label).unwrap_or("View");
    let status_style = panel_priority_style(row.priority);
    let mut text = format!(
        "{marker}{}  [{}]  {action}: {}",
        row.title,
        row.priority.label(),
        row.status
    );
    if let Some(subtitle) = row.subtitle.as_deref() {
        text.push_str("  ");
        text.push_str(subtitle);
    }
    let truncated = truncate_with_ellipsis(&text, width as usize);
    let prefix = format!("{marker}{}  ", row.title);
    let status_prefix = format!("{prefix}[{}]", row.priority.label());
    let mut spans = Vec::new();
    spans.push(Span::styled(
        prefix.clone(),
        if selected {
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Magenta)
        },
    ));
    spans.push(Span::styled(
        format!("[{}]", row.priority.label()),
        status_style,
    ));
    let rest = truncated.strip_prefix(&status_prefix).unwrap_or("");
    spans.push(Span::styled(
        rest.to_string(),
        Style::default().fg(Color::DarkGray),
    ));
    Line::from(spans)
}

fn panel_priority_style(priority: ActionPriority) -> Style {
    match priority {
        ActionPriority::UserReply => Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ActionPriority::ReadyForGo => Style::default().fg(Color::Green),
        ActionPriority::Decision => Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
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
    let action = if entry.actions.can_send_now {
        "send"
    } else if entry.actions.can_open {
        "open"
    } else {
        "disabled"
    };
    let mut text = format!("{marker}{}  [{status}]  {action}", entry.name);
    if let Some(preview) = entry.summary.preview.as_deref() {
        text.push_str("  ");
        text.push_str(preview);
    }
    let truncated = truncate_with_ellipsis(&text, width as usize);
    let mut spans = Vec::new();
    let prefix = format!("{marker}{}", entry.name);
    let visible_prefix = format!("{marker}{}  ", entry.name);
    spans.push(Span::styled(visible_prefix, name_style));
    spans.push(Span::styled(format!("[{status}]"), status_style));
    let rest = truncated
        .strip_prefix(&format!("{prefix}  [{status}]"))
        .unwrap_or("");
    spans.push(Span::styled(
        rest.to_string(),
        Style::default().fg(Color::DarkGray),
    ));
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
    let mut line = if let Some(row) = app
        .selected_panel_row()
        .filter(|row| row.is_ticket_action())
    {
        Line::from(vec![
            Span::styled("action ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                row.title.clone(),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(
                format!("[{}]", row.priority.label()),
                panel_priority_style(row.priority),
            ),
            Span::raw("  "),
            Span::styled(
                row.next_action.map(NextUserAction::label).unwrap_or("View"),
                Style::default().fg(Color::Magenta),
            ),
            Span::styled(
                " display-only; re-check Ticket before dispatch",
                Style::default().fg(Color::DarkGray),
            ),
        ])
    } else {
        match app.selected_pod_entry() {
            Some(entry) => {
                let (status, status_style) = row_status_label(entry);
                let send_text = if entry.actions.can_send_now {
                    "send enabled"
                } else {
                    "send disabled"
                };
                Line::from(vec![
                    Span::styled("target ", Style::default().fg(Color::DarkGray)),
                    Span::styled(
                        entry.name.clone(),
                        Style::default().add_modifier(Modifier::BOLD),
                    ),
                    Span::raw("  "),
                    Span::styled(format!("[{status}]"), status_style),
                    Span::raw("  "),
                    Span::styled(
                        send_text,
                        if entry.actions.can_send_now {
                            Style::default().fg(Color::Green)
                        } else {
                            Style::default().fg(Color::DarkGray)
                        },
                    ),
                ])
            }
            None => Line::from(Span::styled(
                "target — none",
                Style::default().fg(Color::DarkGray),
            )),
        }
    };
    let mut prefix = vec![
        Span::styled("composer ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            app.composer_target().label(),
            Style::default()
                .fg(match app.composer_target() {
                    ComposerTarget::Companion => Color::Green,
                    ComposerTarget::TicketIntake => Color::Magenta,
                })
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("  ·  ", Style::default().fg(Color::DarkGray)),
    ];
    prefix.append(&mut line.spans);
    frame.render_widget(Paragraph::new(Line::from(prefix)), area);
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
        "sending…".to_string()
    } else if let Some(notice) = app.notice.as_deref() {
        notice.to_string()
    } else if let Some(reason) = app.selected_send_disabled_reason() {
        reason
    } else {
        match app.composer_target() {
            ComposerTarget::Companion => {
                "idle live target: Enter sends directly without opening conversation".to_string()
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
        "↑/↓ select  Enter use target  Ctrl+T target  o open  r refresh  Esc quit"
    } else {
        "↑/↓ select  Enter send  o open  r refresh  Esc quit"
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
    use ticket::{LocalTicketBackend, NewTicket, TicketBackend};

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
        assert_eq!(app.selected_send_eligibility(), SendEligibility::Disabled);
        let lines = list_lines(&app, 100, 6)
            .into_iter()
            .map(|line| plain_line(&line))
            .collect::<Vec<_>>();
        let ticket_line = lines
            .iter()
            .position(|line| line.contains("Needs Human Reply"))
            .unwrap();
        let pod_line = lines.iter().position(|line| line.contains("idle")).unwrap();
        assert!(ticket_line < pod_line);

        app.select_next();
        assert_eq!(app.list.selected_entry().unwrap().name, "idle");
        assert_eq!(app.selected_send_eligibility(), SendEligibility::SendNow);
        let open = app.prepare_open().unwrap();
        assert_eq!(open.pod_name, "idle");
        assert_eq!(open.socket_override, Some(PathBuf::from("/tmp/idle.sock")));

        app.input.insert_str("send after ticket row");
        let request = match app.handle_key(key(KeyCode::Enter)) {
            MultiPodAction::Send(request) => request,
            _ => panic!("Pod row should preserve direct send behavior"),
        };
        assert_eq!(request.socket_path, PathBuf::from("/tmp/idle.sock"));
        assert_eq!(
            Segment::flatten_to_text(&request.segments),
            "send after ticket row"
        );
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
    fn multi_idle_live_selected_target_is_send_eligible() {
        let app = test_app(vec![live_info("idle", PodStatus::Idle)]);

        assert_eq!(app.selected_send_eligibility(), SendEligibility::SendNow);
        assert!(app.selected_send_disabled_reason().is_none());
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
    fn multi_running_paused_and_stopped_targets_are_direct_send_disabled() {
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

        assert_eq!(app.selected_send_eligibility(), SendEligibility::Disabled);
        assert!(
            app.selected_send_disabled_reason()
                .unwrap()
                .contains("running")
        );
        app.select_next();
        assert_eq!(app.list.selected_entry().unwrap().name, "paused");
        assert_eq!(app.selected_send_eligibility(), SendEligibility::Disabled);
        assert!(
            app.selected_send_disabled_reason()
                .unwrap()
                .contains("paused")
        );
        app.select_next();
        assert_eq!(app.list.selected_entry().unwrap().name, "stopped");
        assert_eq!(app.selected_send_eligibility(), SendEligibility::Disabled);
        assert!(
            app.selected_send_disabled_reason()
                .unwrap()
                .contains("stopped")
        );
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
        assert_eq!(app.selected_send_eligibility(), SendEligibility::Disabled);
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
    fn multi_delivery_failure_keeps_composer_contents() {
        let mut app = test_app(vec![live_info("idle", PodStatus::Idle)]);
        app.input.insert_str("keep me");
        let before = input_text(&app);

        app.finish_send(Err(DirectSendError::Io("boom".to_string())));

        assert_eq!(input_text(&app), before);
        assert!(app.notice.as_deref().unwrap().contains("composer kept"));
    }

    #[test]
    fn multi_delivery_success_clears_composer_contents() {
        let mut app = test_app(vec![live_info("idle", PodStatus::Idle)]);
        app.input.insert_str("send me");

        app.finish_send(Ok(()));

        assert_eq!(input_text(&app), "");
        assert!(app.notice.as_deref().unwrap().contains("Delivered"));
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
        assert!(app.notice.as_deref().unwrap().contains("Opening alpha"));
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
        assert!(app.notice.as_deref().unwrap().contains("Opening alpha"));
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
    fn multi_non_empty_enter_uses_direct_send_action() {
        let mut app = test_app(vec![live_info("idle", PodStatus::Idle)]);
        app.input.insert_str("send me");

        let request = match app.handle_key(key(KeyCode::Enter)) {
            MultiPodAction::Send(request) => request,
            _ => panic!("non-empty Enter should direct-send"),
        };

        assert_eq!(request.socket_path, PathBuf::from("/tmp/idle.sock"));
        assert_eq!(Segment::flatten_to_text(&request.segments), "send me");
        assert!(app.sending);
        assert!(app.notice.as_deref().unwrap().contains("Sending to idle"));
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
            MultiPodAction::Send(_) => panic!("Ticket Intake target must not direct-send"),
            _ => panic!("Ticket Intake target should launch Intake"),
        };

        assert_eq!(request.context.role, TicketRole::Intake);
        assert_eq!(
            request.context.user_instruction.as_deref(),
            Some("please intake this work")
        );
        assert_eq!(request.runtime_command.program(), Path::new("/tmp/yoi"));
        assert!(app.sending);
        assert!(app.notice.as_deref().unwrap().contains("Launching"));
        assert_eq!(input_text(&app), "please intake this work");
    }

    #[test]
    fn multi_ticket_intake_finish_success_clears_composer_and_reports_pod() {
        let mut app = ticket_enabled_app(vec![live_info("idle", PodStatus::Idle)]);
        app.cycle_composer_target();
        app.input.insert_str("please intake this work");
        app.sending = true;

        app.finish_intake_launch(Ok(TicketRoleLaunchResult {
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
        }));

        assert!(!app.sending);
        assert_eq!(input_text(&app), "");
        assert!(app.notice.as_deref().unwrap().contains("intake-pod"));
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
    fn multi_empty_enter_on_non_openable_row_matches_o_diagnostic() {
        let mut enter_app = test_app(vec![unreachable_live_info("unreachable")]);
        assert!(matches!(
            enter_app.handle_key(key(KeyCode::Enter)),
            MultiPodAction::Open
        ));
        assert!(enter_app.prepare_open().is_none());
        let enter_notice = enter_app.notice.clone();

        let mut open_app = test_app(vec![unreachable_live_info("unreachable")]);
        assert!(matches!(
            open_app.handle_key(key(KeyCode::Char('o'))),
            MultiPodAction::Open
        ));
        assert!(open_app.prepare_open().is_none());

        assert_eq!(enter_notice, open_app.notice);
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

    fn ticket_enabled_app(live: Vec<LivePodInfo>) -> MultiPodApp {
        let mut panel = WorkspacePanelViewModel::empty(Path::new("test"));
        panel.composer = crate::workspace_panel::WorkspacePanelComposer::ticket_enabled();
        app_with_panel(
            PodList::from_sources(PodVisibilitySource::ResumePicker, vec![], live, None, 10),
            panel,
        )
    }

    fn app_with_list(list: PodList) -> MultiPodApp {
        app_with_panel(list, WorkspacePanelViewModel::empty(Path::new("test")))
    }

    fn app_with_panel(list: PodList, panel: WorkspacePanelViewModel) -> MultiPodApp {
        let mut app = MultiPodApp {
            list,
            panel,
            input: InputBuffer::new(),
            selected_row: None,
            composer_target: ComposerTarget::Companion,
            notice: None,
            sending: false,
            runtime_command: PodRuntimeCommand::for_executable("/tmp/yoi"),
        };
        app.ensure_selection_visible();
        app.ensure_composer_target_available();
        app
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
