use std::path::{Path, PathBuf};

use protocol::PodStatus;
use ticket::config::{TICKET_CONFIG_RELATIVE_PATH, TicketConfig};
use ticket::{
    LocalTicketBackend, TicketBackend, TicketError, TicketEvent, TicketFilter, TicketIdOrSlug,
    TicketMeta, TicketRelationBlocker, TicketSummary, TicketWorkflowState,
};

use crate::pod_list::{PodList, PodListEntry, StoredMetadataState};
use crate::role_session_registry::{PanelRegistrySnapshot, PanelRegistryStore};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkspacePanelViewModel {
    pub(crate) header: WorkspacePanelHeader,
    pub(crate) rows: Vec<PanelRow>,
    pub(crate) composer: WorkspacePanelComposer,
}

impl WorkspacePanelViewModel {
    pub(crate) fn empty(workspace_root: &Path) -> Self {
        Self {
            header: WorkspacePanelHeader {
                workspace_label: workspace_root
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("workspace")
                    .to_string(),
                ticket_root: workspace_root
                    .join(ticket::config::DEFAULT_TICKET_BACKEND_RELATIVE_PATH),
                ticket_configured: false,
                companion: None,
                orchestrator: None,
                diagnostics: Vec::new(),
            },
            rows: Vec::new(),
            composer: WorkspacePanelComposer::companion_only(),
        }
    }

    pub(crate) fn row(&self, key: &PanelRowKey) -> Option<&PanelRow> {
        self.rows.iter().find(|row| &row.key == key)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkspacePanelHeader {
    pub(crate) workspace_label: String,
    pub(crate) ticket_root: PathBuf,
    pub(crate) ticket_configured: bool,
    pub(crate) companion: Option<CompanionPanelState>,
    pub(crate) orchestrator: Option<OrchestratorPanelState>,
    pub(crate) diagnostics: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ComposerTarget {
    Companion,
    TicketIntake,
}

impl ComposerTarget {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Companion => "Companion",
            Self::TicketIntake => "Ticket Planning",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkspacePanelComposer {
    pub(crate) available_targets: Vec<ComposerTarget>,
}

impl WorkspacePanelComposer {
    pub(crate) fn companion_only() -> Self {
        Self {
            available_targets: vec![ComposerTarget::Companion],
        }
    }

    pub(crate) fn ticket_enabled() -> Self {
        Self {
            available_targets: vec![ComposerTarget::Companion, ComposerTarget::TicketIntake],
        }
    }

    pub(crate) fn is_available(&self, target: ComposerTarget) -> bool {
        self.available_targets.contains(&target)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CompanionPanelState {
    pub(crate) pod_name: String,
    pub(crate) status: CompanionPanelStatus,
    pub(crate) detail: Option<String>,
}

impl CompanionPanelState {
    pub(crate) fn new(
        pod_name: impl Into<String>,
        status: CompanionPanelStatus,
        detail: Option<String>,
    ) -> Self {
        Self {
            pod_name: pod_name.into(),
            status,
            detail,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CompanionPanelStatus {
    Live,
    Restored,
    Spawned,
    Stopped,
    Missing,
    Unavailable,
}

impl CompanionPanelStatus {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Restored => "restored",
            Self::Spawned => "spawned",
            Self::Stopped => "stopped/restorable",
            Self::Missing => "missing",
            Self::Unavailable => "unavailable",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OrchestratorPanelState {
    pub(crate) pod_name: String,
    pub(crate) status: OrchestratorPanelStatus,
    pub(crate) detail: Option<String>,
}

impl OrchestratorPanelState {
    pub(crate) fn new(
        pod_name: impl Into<String>,
        status: OrchestratorPanelStatus,
        detail: Option<String>,
    ) -> Self {
        Self {
            pod_name: pod_name.into(),
            status,
            detail,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OrchestratorPanelStatus {
    Live,
    Restored,
    Spawned,
    Stopped,
    Missing,
    Unavailable,
}

impl OrchestratorPanelStatus {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Restored => "restored",
            Self::Spawned => "spawned",
            Self::Stopped => "stopped/restorable",
            Self::Missing => "missing",
            Self::Unavailable => "unavailable",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum PanelRowKey {
    Ticket(String),
    Pod(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PanelRowKind {
    Planning,
    Ticket,
    Review,
    ActiveWork,
    Pod,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ActionPriority {
    ReadyForQueue,
    ActiveWork,
    Background,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NextUserAction {
    Clarify,
    Queue,
    Close,
    Wait,
    OpenPod,
}

impl NextUserAction {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Clarify => "Clarify",
            Self::Queue => "Queue",
            Self::Close => "Close",
            Self::Wait => "Wait",
            Self::OpenPod => "Open",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TicketPanelEntry {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) priority: String,
    pub(crate) workflow_state: TicketWorkflowState,
    pub(crate) workflow_state_explicit: bool,
    pub(crate) next_action: Option<NextUserAction>,
    pub(crate) updated_at: Option<String>,
    pub(crate) latest_event_kind: Option<String>,
    pub(crate) latest_event_excerpt: Option<String>,
    pub(crate) blocked_reason: Option<String>,
    pub(crate) related_pods: Vec<String>,
    pub(crate) local_claim: Option<TicketLocalClaimEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TicketLocalClaimEntry {
    pub(crate) pod_name: String,
    pub(crate) role: String,
    pub(crate) status: TicketLocalClaimStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TicketLocalClaimStatus {
    Live,
    Restorable,
    Stale,
}

impl TicketLocalClaimStatus {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Restorable => "restorable",
            Self::Stale => "stale",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PanelRow {
    pub(crate) key: PanelRowKey,
    pub(crate) kind: PanelRowKind,
    pub(crate) title: String,
    pub(crate) subtitle: Option<String>,
    pub(crate) status: String,
    pub(crate) priority: ActionPriority,
    pub(crate) next_action: Option<NextUserAction>,
    pub(crate) ticket: Option<TicketPanelEntry>,
    pub(crate) related_pods: Vec<String>,
    pub(crate) disabled_reason: Option<String>,
    pub(crate) key_hint: Option<String>,
}

impl PanelRow {
    pub(crate) fn is_ticket_action(&self) -> bool {
        !matches!(self.kind, PanelRowKind::Pod)
    }
}

const MAX_POD_NAME_CHARS: usize = 80;
const ORCHESTRATOR_SUFFIX: &str = "-orchestrator";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TicketConfigAvailability {
    Absent,
    Usable,
    Unusable(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CompanionPodPresence {
    Live,
    Restorable,
    Missing,
    Unavailable(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CompanionLifecyclePlan {
    ReportLive,
    Restore,
    Spawn,
    Unavailable(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OrchestratorPodPresence {
    Live,
    Restorable,
    Missing,
    Unavailable(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OrchestratorLifecyclePlan {
    SkipNoTicketConfig,
    ReportLive,
    Restore,
    Spawn,
    Unavailable(String),
}

pub(crate) fn workspace_companion_pod_name(workspace_root: &Path) -> String {
    let seed = workspace_root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("workspace");
    sanitise_pod_name_component(seed, MAX_POD_NAME_CHARS)
        .filter(|component| !component.is_empty())
        .unwrap_or_else(|| "workspace".to_string())
}

pub(crate) fn workspace_orchestrator_pod_name(workspace_root: &Path) -> String {
    let seed = workspace_root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("workspace");
    let max_component_chars = MAX_POD_NAME_CHARS.saturating_sub(ORCHESTRATOR_SUFFIX.len());
    let component = sanitise_pod_name_component(seed, max_component_chars)
        .filter(|component| !component.is_empty())
        .unwrap_or_else(|| "workspace".to_string());
    format!("{component}{ORCHESTRATOR_SUFFIX}")
}

fn sanitise_pod_name_component(value: &str, max_chars: usize) -> Option<String> {
    let mut out = String::new();
    let mut last_was_dash = false;
    for ch in value.trim().chars() {
        let mapped = if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.') {
            ch.to_ascii_lowercase()
        } else {
            '-'
        };
        if mapped == '-' {
            if !last_was_dash && !out.is_empty() {
                out.push(mapped);
            }
            last_was_dash = true;
        } else {
            out.push(mapped);
            last_was_dash = false;
        }
    }
    let trimmed = out.trim_matches(|ch| matches!(ch, '-' | '_' | '.'));
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.chars().take(max_chars).collect())
    }
}

pub(crate) fn companion_pod_presence(pod_name: &str, pods: &PodList) -> CompanionPodPresence {
    let Some(entry) = pods.entries.iter().find(|entry| entry.name == pod_name) else {
        return CompanionPodPresence::Missing;
    };
    if entry.live.as_ref().is_some_and(|live| live.reachable) {
        return CompanionPodPresence::Live;
    }
    if entry.actions.can_restore {
        return CompanionPodPresence::Restorable;
    }
    let reason = entry
        .actions
        .disabled_reason
        .clone()
        .or_else(|| {
            entry
                .diagnostics
                .first()
                .map(|diagnostic| diagnostic.message.clone())
        })
        .unwrap_or_else(|| "pod state is not live, restorable, or spawn-safe".to_string());
    CompanionPodPresence::Unavailable(reason)
}

pub(crate) fn decide_companion_lifecycle(
    presence: &CompanionPodPresence,
) -> CompanionLifecyclePlan {
    match presence {
        CompanionPodPresence::Live => CompanionLifecyclePlan::ReportLive,
        CompanionPodPresence::Restorable => CompanionLifecyclePlan::Restore,
        CompanionPodPresence::Missing => CompanionLifecyclePlan::Spawn,
        CompanionPodPresence::Unavailable(message) => CompanionLifecyclePlan::Unavailable(format!(
            "Workspace Companion Pod state is unusable: {message}"
        )),
    }
}

pub(crate) fn orchestrator_pod_presence(pod_name: &str, pods: &PodList) -> OrchestratorPodPresence {
    let Some(entry) = pods.entries.iter().find(|entry| entry.name == pod_name) else {
        return OrchestratorPodPresence::Missing;
    };
    if entry.live.as_ref().is_some_and(|live| live.reachable) {
        return OrchestratorPodPresence::Live;
    }
    if entry.actions.can_restore {
        return OrchestratorPodPresence::Restorable;
    }
    let reason = entry
        .actions
        .disabled_reason
        .clone()
        .or_else(|| {
            entry
                .diagnostics
                .first()
                .map(|diagnostic| diagnostic.message.clone())
        })
        .unwrap_or_else(|| "pod state is not live, restorable, or spawn-safe".to_string());
    OrchestratorPodPresence::Unavailable(reason)
}

pub(crate) fn decide_orchestrator_lifecycle(
    config: &TicketConfigAvailability,
    presence: &OrchestratorPodPresence,
) -> OrchestratorLifecyclePlan {
    match config {
        TicketConfigAvailability::Absent => OrchestratorLifecyclePlan::SkipNoTicketConfig,
        TicketConfigAvailability::Unusable(message) => OrchestratorLifecyclePlan::Unavailable(
            format!("Ticket config is unusable; workspace Orchestrator not started: {message}"),
        ),
        TicketConfigAvailability::Usable => match presence {
            OrchestratorPodPresence::Live => OrchestratorLifecyclePlan::ReportLive,
            OrchestratorPodPresence::Restorable => OrchestratorLifecyclePlan::Restore,
            OrchestratorPodPresence::Missing => OrchestratorLifecyclePlan::Spawn,
            OrchestratorPodPresence::Unavailable(message) => {
                OrchestratorLifecyclePlan::Unavailable(format!(
                    "Workspace Orchestrator Pod state is unusable: {message}"
                ))
            }
        },
    }
}

pub(crate) fn ticket_config_availability(workspace_root: &Path) -> TicketConfigAvailability {
    let config_path = workspace_root.join(TICKET_CONFIG_RELATIVE_PATH);
    match config_path.symlink_metadata() {
        Ok(metadata) if !metadata.is_file() => TicketConfigAvailability::Unusable(format!(
            "{} exists but is not a regular file",
            TICKET_CONFIG_RELATIVE_PATH
        )),
        Ok(_) => match TicketConfig::load_workspace(workspace_root) {
            Ok(_) => TicketConfigAvailability::Usable,
            Err(error) => TicketConfigAvailability::Unusable(error.to_string()),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            TicketConfigAvailability::Absent
        }
        Err(error) => TicketConfigAvailability::Unusable(format!(
            "could not inspect {}: {error}",
            TICKET_CONFIG_RELATIVE_PATH
        )),
    }
}

pub(crate) fn bounded_panel_diagnostic(message: impl AsRef<str>) -> String {
    let collapsed = message
        .as_ref()
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    excerpt(&collapsed, 180).unwrap_or_else(|| "unknown diagnostic".to_string())
}

pub(crate) fn build_workspace_panel(
    workspace_root: &Path,
    pods: &PodList,
) -> WorkspacePanelViewModel {
    let registry = match PanelRegistryStore::default_for_workspace(workspace_root)
        .and_then(|store| store.snapshot())
    {
        Ok(snapshot) => snapshot,
        Err(error) => {
            let mut model = WorkspacePanelViewModel::empty(workspace_root);
            model
                .header
                .diagnostics
                .push(bounded_panel_diagnostic(format!(
                    "Panel local role registry unavailable: {error}"
                )));
            return build_workspace_panel_with_registry_model(
                model,
                workspace_root,
                pods,
                &PanelRegistrySnapshot::empty(),
            );
        }
    };
    build_workspace_panel_with_registry(workspace_root, pods, &registry)
}

fn build_workspace_panel_with_registry(
    workspace_root: &Path,
    pods: &PodList,
    registry: &PanelRegistrySnapshot,
) -> WorkspacePanelViewModel {
    let model = WorkspacePanelViewModel::empty(workspace_root);
    build_workspace_panel_with_registry_model(model, workspace_root, pods, registry)
}

fn build_workspace_panel_with_registry_model(
    mut model: WorkspacePanelViewModel,
    workspace_root: &Path,
    pods: &PodList,
    registry: &PanelRegistrySnapshot,
) -> WorkspacePanelViewModel {
    match ticket_config_availability(workspace_root) {
        TicketConfigAvailability::Absent => {}
        TicketConfigAvailability::Usable => {
            model.header.ticket_configured = true;
            model.composer = WorkspacePanelComposer::ticket_enabled();
            match TicketConfig::load_workspace(workspace_root) {
                Ok(config) => {
                    model.header.ticket_root = config.backend_root().to_path_buf();
                    let backend = LocalTicketBackend::new(config.backend_root().to_path_buf())
                        .with_record_language(config.ticket_record_language());
                    match build_ticket_rows(&backend, pods, registry) {
                        Ok(rows) => model.rows.extend(rows),
                        Err(error) => {
                            model
                                .header
                                .diagnostics
                                .push(bounded_panel_diagnostic(format!(
                                    "Ticket rows unavailable: {error}"
                                )))
                        }
                    }
                }
                Err(error) => model
                    .header
                    .diagnostics
                    .push(bounded_panel_diagnostic(format!(
                        "Ticket config is unusable: {error}"
                    ))),
            }
        }
        TicketConfigAvailability::Unusable(message) => {
            model.header.ticket_configured = true;
            model
                .header
                .diagnostics
                .push(bounded_panel_diagnostic(format!(
                    "Ticket config is unusable: {message}"
                )));
        }
    }

    model.rows.extend(pod_rows(pods));
    model.rows.sort_by(|a, b| {
        a.priority
            .cmp(&b.priority)
            .then_with(|| row_updated_at(b).cmp(row_updated_at(a)))
            .then_with(|| a.title.cmp(&b.title))
    });
    model
}

pub(crate) fn build_current_ticket_row(
    backend: &LocalTicketBackend,
    ticket_id: &str,
    pods: &PodList,
) -> ticket::Result<PanelRow> {
    let ticket = backend.show(TicketIdOrSlug::Id(ticket_id.to_owned()))?;
    if ticket.meta.workflow_state == TicketWorkflowState::Closed {
        return Err(TicketError::Conflict(format!(
            "Ticket {ticket_id} is already closed"
        )));
    }
    let summary = ticket_summary_from_meta(&ticket.meta);
    let registry = PanelRegistrySnapshot::empty();
    Ok(ticket_row(
        summary,
        &ticket.events,
        &ticket.relations.blockers,
        pods,
        &registry,
    ))
}

fn ticket_summary_from_meta(meta: &TicketMeta) -> TicketSummary {
    TicketSummary {
        id: meta.id.clone(),
        slug: meta.slug.clone(),
        title: meta.title.clone(),
        status: meta.status.clone(),
        kind: meta.kind.clone(),
        priority: meta.priority.clone(),
        labels: meta.labels.clone(),
        readiness: meta.readiness.clone(),
        workflow_state: meta.workflow_state,
        workflow_state_explicit: meta.workflow_state_explicit,
        queued_by: meta.queued_by.clone(),
        queued_at: meta.queued_at.clone(),
        updated_at: meta.updated_at.clone(),
    }
}

fn build_ticket_rows(
    backend: &LocalTicketBackend,
    pods: &PodList,
    registry: &PanelRegistrySnapshot,
) -> ticket::Result<Vec<PanelRow>> {
    let mut rows = Vec::new();
    for summary in backend.list(TicketFilter::all())? {
        if summary.workflow_state == TicketWorkflowState::Closed {
            continue;
        }
        let ticket = backend.show(TicketIdOrSlug::Query(summary.id.clone()))?;
        rows.push(ticket_row(
            summary,
            &ticket.events,
            &ticket.relations.blockers,
            pods,
            registry,
        ));
    }
    Ok(rows)
}

fn ticket_row(
    summary: TicketSummary,
    events: &[TicketEvent],
    relation_blockers: &[TicketRelationBlocker],
    pods: &PodList,
    registry: &PanelRegistrySnapshot,
) -> PanelRow {
    let local_claim = local_claim_for_ticket(&summary, pods, registry);
    let related_pods = related_pods_for_ticket(&summary, pods, registry);
    let derived = derive_ticket_state(&summary, relation_blockers);
    let latest_event = events.last();
    let entry = TicketPanelEntry {
        id: summary.id.clone(),
        title: summary.title.clone(),
        priority: summary.priority.clone(),
        workflow_state: summary.workflow_state,
        workflow_state_explicit: summary.workflow_state_explicit,
        next_action: derived.action,
        updated_at: summary.updated_at.clone(),
        latest_event_kind: latest_event.map(|event| event.kind.as_str().to_string()),
        latest_event_excerpt: latest_event.and_then(|event| excerpt(event.body.as_str(), 72)),
        blocked_reason: derived.blocked_reason.clone(),
        related_pods: related_pods.clone(),
        local_claim,
    };
    let subtitle = ticket_subtitle(&entry);
    PanelRow {
        key: PanelRowKey::Ticket(summary.id),
        kind: derived.kind,
        title: summary.title,
        subtitle,
        status: summary.workflow_state.as_str().to_string(),
        priority: derived.priority,
        next_action: derived.action,
        ticket: Some(entry),
        related_pods,
        disabled_reason: derived.disabled_reason,
        key_hint: derived.key_hint,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DerivedTicketState {
    kind: PanelRowKind,
    priority: ActionPriority,
    action: Option<NextUserAction>,
    disabled_reason: Option<String>,
    key_hint: Option<String>,
    blocked_reason: Option<String>,
}

fn derive_ticket_state(
    summary: &TicketSummary,
    relation_blockers: &[TicketRelationBlocker],
) -> DerivedTicketState {
    if !relation_blockers.is_empty() {
        let shown_blockers = relation_blockers.iter().take(3).count();
        let mut blockers = relation_blockers
            .iter()
            .take(3)
            .map(|blocker| {
                format!(
                    "{} via {} (state: {})",
                    blocker.blocking_ticket,
                    blocker.reason_kind,
                    blocker.blocking_state.as_str()
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        let remaining_blockers = relation_blockers.len().saturating_sub(shown_blockers);
        if remaining_blockers > 0 {
            blockers.push_str(&format!(" (+{remaining_blockers} more)"));
        }
        let waiting_reason = format!("waiting for {blockers}");
        return DerivedTicketState {
            kind: match summary.workflow_state {
                TicketWorkflowState::Planning => PanelRowKind::Planning,
                TicketWorkflowState::Queued | TicketWorkflowState::InProgress => {
                    PanelRowKind::ActiveWork
                }
                TicketWorkflowState::Done | TicketWorkflowState::Closed => PanelRowKind::Review,
                TicketWorkflowState::Ready => PanelRowKind::Ticket,
            },
            priority: match summary.workflow_state {
                TicketWorkflowState::Queued | TicketWorkflowState::InProgress => {
                    ActionPriority::ActiveWork
                }
                _ => ActionPriority::Background,
            },
            action: Some(NextUserAction::Wait),
            disabled_reason: Some(format!(
                "Queue disabled: {waiting_reason}. Resolve dependency/blocker before ready -> queued."
            )),
            key_hint: Some(format!("Gate: {waiting_reason}")),
            blocked_reason: Some(blockers),
        };
    }

    match summary.workflow_state {
        TicketWorkflowState::Ready => DerivedTicketState {
            kind: PanelRowKind::Ticket,
            priority: ActionPriority::ReadyForQueue,
            action: Some(NextUserAction::Queue),
            disabled_reason: None,
            key_hint: Some(
                "Queue transitions ready -> queued and may notify Orchestrator".to_string(),
            ),
            blocked_reason: None,
        },
        TicketWorkflowState::Queued => DerivedTicketState {
            kind: PanelRowKind::ActiveWork,
            priority: ActionPriority::ActiveWork,
            action: Some(NextUserAction::Wait),
            disabled_reason: Some("Ticket is queued for Orchestrator routing.".to_string()),
            key_hint: None,
            blocked_reason: None,
        },
        TicketWorkflowState::InProgress => DerivedTicketState {
            kind: PanelRowKind::ActiveWork,
            priority: ActionPriority::ActiveWork,
            action: Some(NextUserAction::Wait),
            disabled_reason: Some("Ticket is already in progress.".to_string()),
            key_hint: None,
            blocked_reason: None,
        },
        TicketWorkflowState::Done => DerivedTicketState {
            kind: PanelRowKind::Review,
            priority: ActionPriority::Background,
            action: Some(NextUserAction::Close),
            disabled_reason: Some(
                "state is done; close if a resolution is still missing.".to_string(),
            ),
            key_hint: None,
            blocked_reason: None,
        },
        TicketWorkflowState::Planning => DerivedTicketState {
            kind: PanelRowKind::Planning,
            priority: ActionPriority::Background,
            action: Some(NextUserAction::Clarify),
            disabled_reason: Some(
                "Ticket is still in planning; mark it ready before queueing.".to_string(),
            ),
            key_hint: Some("Planning/Intake helpers can set state = ready".to_string()),
            blocked_reason: None,
        },
        TicketWorkflowState::Closed => DerivedTicketState {
            kind: PanelRowKind::Review,
            priority: ActionPriority::Background,
            action: Some(NextUserAction::Wait),
            disabled_reason: Some("Ticket is closed.".to_string()),
            key_hint: None,
            blocked_reason: None,
        },
    }
}

fn related_pods_for_ticket(
    summary: &TicketSummary,
    pods: &PodList,
    registry: &PanelRegistrySnapshot,
) -> Vec<String> {
    let id = lowercase(&summary.id);
    let mut names = Vec::new();
    if let Some(claim) = registry.claim_for_ticket(&summary.id) {
        names.push(claim.pod_name.clone());
    }
    for pod in pods.entries.iter().filter_map(|pod| {
        let name = lowercase(&pod.name);
        if !id.is_empty() && name.contains(&id) {
            Some(pod.name.clone())
        } else {
            None
        }
    }) {
        if !names.iter().any(|existing| existing == &pod) {
            names.push(pod);
        }
        if names.len() >= 5 {
            break;
        }
    }
    names
}

fn local_claim_for_ticket(
    summary: &TicketSummary,
    pods: &PodList,
    registry: &PanelRegistrySnapshot,
) -> Option<TicketLocalClaimEntry> {
    let claim = registry.claim_for_ticket(&summary.id)?;
    let status = local_claim_status_for_pod(&claim.pod_name, pods);
    Some(TicketLocalClaimEntry {
        pod_name: claim.pod_name.clone(),
        role: claim.role.clone(),
        status,
    })
}

pub(crate) fn local_claim_status_for_pod(pod_name: &str, pods: &PodList) -> TicketLocalClaimStatus {
    let Some(entry) = pods.entries.iter().find(|entry| entry.name == pod_name) else {
        return TicketLocalClaimStatus::Stale;
    };
    if entry.live.as_ref().is_some_and(|live| live.reachable) {
        return TicketLocalClaimStatus::Live;
    }
    if entry.actions.can_restore {
        return TicketLocalClaimStatus::Restorable;
    }
    TicketLocalClaimStatus::Stale
}

fn ticket_subtitle(entry: &TicketPanelEntry) -> Option<String> {
    let mut parts = vec![format!("{} · {}", entry.id, entry.workflow_state.as_str())];
    if let Some(claim) = entry.local_claim.as_ref() {
        parts.push(format!(
            "claim: {} ({})",
            claim.pod_name,
            claim.status.label()
        ));
    }
    if !entry.related_pods.is_empty() {
        parts.push(format!("pods: {}", entry.related_pods.join(", ")));
    }
    if let Some(excerpt) = entry.latest_event_excerpt.as_ref() {
        parts.push(format!("latest: {excerpt}"));
    }
    Some(parts.join("  "))
}

fn pod_rows(pods: &PodList) -> Vec<PanelRow> {
    pods.entries.iter().map(pod_row).collect()
}

fn pod_row(entry: &PodListEntry) -> PanelRow {
    let status = pod_status_label(entry).to_string();
    let next_action = if entry.actions.can_open {
        Some(NextUserAction::OpenPod)
    } else {
        None
    };
    let mut subtitle = entry.summary.preview.clone();
    if subtitle.is_none()
        && entry
            .stored
            .as_ref()
            .is_some_and(|stored| matches!(stored.metadata_state, StoredMetadataState::Corrupt(_)))
    {
        subtitle = Some("metadata corrupt".to_string());
    }

    PanelRow {
        key: PanelRowKey::Pod(entry.name.clone()),
        kind: PanelRowKind::Pod,
        title: entry.name.clone(),
        subtitle,
        status,
        priority: ActionPriority::Background,
        next_action,
        ticket: None,
        related_pods: Vec::new(),
        disabled_reason: entry.actions.disabled_reason.clone(),
        key_hint: Some("Enter opens/attaches for inspection".to_string()),
    }
}

fn pod_status_label(entry: &PodListEntry) -> &'static str {
    if let Some(live) = entry.live.as_ref() {
        if !live.reachable {
            return "unreachable";
        }
        return match live.status {
            Some(PodStatus::Idle) => "live idle",
            Some(PodStatus::Running) => "live running",
            Some(PodStatus::Paused) => "live paused",
            None => "live",
        };
    }
    if entry
        .stored
        .as_ref()
        .is_some_and(|stored| matches!(stored.metadata_state, StoredMetadataState::Corrupt(_)))
    {
        "corrupt"
    } else {
        "stopped/restorable"
    }
}

fn row_updated_at(row: &PanelRow) -> &str {
    row.ticket
        .as_ref()
        .and_then(|ticket| ticket.updated_at.as_deref())
        .unwrap_or("")
}

fn excerpt(markdown: &str, max_chars: usize) -> Option<String> {
    let collapsed = markdown
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect::<Vec<_>>()
        .join(" ");
    if collapsed.is_empty() {
        None
    } else if collapsed.chars().count() <= max_chars {
        Some(collapsed)
    } else {
        let mut value = collapsed
            .chars()
            .take(max_chars.saturating_sub(1))
            .collect::<String>();
        value.push('…');
        Some(value)
    }
}

fn lowercase(value: &str) -> String {
    value.to_ascii_lowercase()
}

#[allow(dead_code)]
#[cfg(test)]
mod tests {
    use super::*;
    use crate::pod_list::{LivePodInfo, PodEntrySummary};
    use std::fs;
    use std::path::{Path, PathBuf};
    use tempfile::TempDir;
    use ticket::{NewTicket, NewTicketRelation, TicketRelationKind, TicketWorkflowState};

    fn empty_pods() -> PodList {
        PodList::from_sources(
            crate::pod_list::PodVisibilitySource::ResumePicker,
            vec![],
            vec![],
            None,
            10,
        )
    }

    fn create_ticket(
        backend: &LocalTicketBackend,
        title: &str,
        configure: impl FnOnce(&mut NewTicket),
    ) {
        let mut input = NewTicket::new(title);
        configure(&mut input);
        backend.create(input).unwrap();
    }

    fn write_ticket_config(workspace_root: &Path) {
        let config_dir = workspace_root.join(".yoi");
        fs::create_dir_all(&config_dir).unwrap();
        fs::write(
            config_dir.join("ticket.config.toml"),
            "[backend]\nprovider = \"builtin:yoi_local\"\nroot = \".yoi/tickets\"\n",
        )
        .unwrap();
    }

    fn live_pods(names: &[&str]) -> PodList {
        PodList::from_sources(
            crate::pod_list::PodVisibilitySource::ResumePicker,
            vec![],
            names
                .iter()
                .map(|name| LivePodInfo {
                    pod_name: (*name).to_string(),
                    socket_path: PathBuf::from(format!("/tmp/{name}.sock")),
                    status: Some(PodStatus::Idle),
                    reachable: true,
                    segment_id: None,
                    summary: PodEntrySummary::default(),
                })
                .collect(),
            None,
            10,
        )
    }

    #[test]
    fn workspace_panel_without_ticket_config_is_pod_only() {
        let temp = TempDir::new().unwrap();
        let backend = LocalTicketBackend::new(temp.path().join(".yoi/tickets"));
        create_ticket(&backend, "Hidden Without Config", |_| {});

        let model = build_workspace_panel(temp.path(), &live_pods(&["idle"]));

        assert!(model.header.diagnostics.is_empty());
        assert_eq!(
            model.composer.available_targets,
            vec![ComposerTarget::Companion]
        );
        assert_eq!(model.rows.len(), 1);
        assert_eq!(model.rows[0].key, PanelRowKey::Pod("idle".to_string()));
        assert!(model.rows[0].ticket.is_none());
    }

    #[test]
    fn workspace_panel_uses_explicit_workflow_state_for_queue_priority() {
        let temp = TempDir::new().unwrap();
        write_ticket_config(temp.path());
        let backend = LocalTicketBackend::new(temp.path().join(".yoi/tickets"));
        create_ticket(&backend, "Ready Ticket", |input| {
            input.workflow_state = Some(TicketWorkflowState::Ready);
        });
        create_ticket(&backend, "Planning Ticket", |_| {});

        let model = build_workspace_panel(temp.path(), &empty_pods());
        assert_eq!(
            model.composer.available_targets,
            vec![ComposerTarget::Companion, ComposerTarget::TicketIntake]
        );
        let row = model
            .rows
            .iter()
            .find(|row| row.title == "Ready Ticket")
            .unwrap();

        assert_eq!(row.status, "ready");
        assert_eq!(row.priority, ActionPriority::ReadyForQueue);
        assert_eq!(row.next_action, Some(NextUserAction::Queue));
    }

    #[test]
    fn workspace_panel_does_not_infer_workflow_state_from_readiness_or_title() {
        let temp = TempDir::new().unwrap();
        write_ticket_config(temp.path());
        let backend = LocalTicketBackend::new(temp.path().join(".yoi/tickets"));
        create_ticket(&backend, "Readiness Heuristic", |input| {
            input.readiness = Some("implementation-ready".to_string());
        });
        create_ticket(&backend, "Queued Words Are Not State", |_| {});
        create_ticket(&backend, "Queued Explicit", |input| {
            input.workflow_state = Some(TicketWorkflowState::Queued);
        });

        let model = build_workspace_panel(temp.path(), &empty_pods());
        let readiness = model
            .rows
            .iter()
            .find(|row| row.title == "Readiness Heuristic")
            .unwrap();
        let title = model
            .rows
            .iter()
            .find(|row| row.title == "Queued Words Are Not State")
            .unwrap();
        let queued = model
            .rows
            .iter()
            .find(|row| row.title == "Queued Explicit")
            .unwrap();

        assert_eq!(
            readiness.ticket.as_ref().unwrap().workflow_state,
            TicketWorkflowState::Planning
        );
        assert_eq!(readiness.next_action, Some(NextUserAction::Clarify));
        assert_eq!(
            title.ticket.as_ref().unwrap().workflow_state,
            TicketWorkflowState::Planning
        );
        assert_eq!(title.next_action, Some(NextUserAction::Clarify));
        assert_eq!(
            queued.ticket.as_ref().unwrap().workflow_state,
            TicketWorkflowState::Queued
        );
        assert_eq!(queued.next_action, Some(NextUserAction::Wait));
    }

    #[test]
    fn workspace_panel_marks_ready_ticket_with_unresolved_relation_waiting_gate() {
        let temp = TempDir::new().unwrap();
        write_ticket_config(temp.path());
        let backend = LocalTicketBackend::new(temp.path().join(".yoi/tickets"));
        let mut ready_input = NewTicket::new("Ready Blocked By Relation");
        ready_input.workflow_state = Some(TicketWorkflowState::Ready);
        let ready = backend.create(ready_input).unwrap();
        let dependency = backend
            .create(NewTicket::new("Relation Dependency"))
            .unwrap();
        backend
            .add_ticket_relation(
                TicketIdOrSlug::Id(ready.id.clone()),
                NewTicketRelation {
                    kind: TicketRelationKind::DependsOn,
                    target: dependency.id.clone(),
                    note: None,
                    author: Some("test".to_string()),
                },
            )
            .unwrap();

        let model = build_workspace_panel(temp.path(), &empty_pods());
        let row = model
            .rows
            .iter()
            .find(|row| row.title == "Ready Blocked By Relation")
            .unwrap();

        assert_eq!(row.kind, PanelRowKind::Ticket);
        assert_eq!(row.next_action, Some(NextUserAction::Wait));
        assert_eq!(row.priority, ActionPriority::Background);
        assert!(
            row.disabled_reason
                .as_deref()
                .unwrap()
                .contains("Queue disabled: waiting for")
        );
        assert!(
            row.ticket
                .as_ref()
                .unwrap()
                .blocked_reason
                .as_deref()
                .unwrap()
                .contains(&dependency.id)
        );
    }

    #[test]
    fn workspace_panel_defaults_missing_open_state_to_planning_and_displays_done_state() {
        let temp = TempDir::new().unwrap();
        write_ticket_config(temp.path());
        let backend = LocalTicketBackend::new(temp.path().join(".yoi/tickets"));
        create_ticket(&backend, "Plain Backlog", |_| {});
        create_ticket(&backend, "Done Explicit", |input| {
            input.workflow_state = Some(TicketWorkflowState::Done);
        });

        let model = build_workspace_panel(temp.path(), &empty_pods());
        let backlog = model
            .rows
            .iter()
            .find(|row| row.title == "Plain Backlog")
            .unwrap();
        let done = model
            .rows
            .iter()
            .find(|row| row.title == "Done Explicit")
            .unwrap();

        assert_eq!(backlog.status, "planning");
        assert_eq!(backlog.next_action, Some(NextUserAction::Clarify));
        assert!(backlog.is_ticket_action());
        assert_eq!(done.status, "done");
        assert_eq!(done.next_action, Some(NextUserAction::Close));
    }

    #[test]
    fn workspace_panel_displays_local_ticket_claim_status() {
        let temp = TempDir::new().unwrap();
        write_ticket_config(temp.path());
        let backend = LocalTicketBackend::new(temp.path().join(".yoi/tickets"));
        create_ticket(&backend, "Claimed Planning", |_| {});
        let summary = backend.list(TicketFilter::all()).unwrap().remove(0);
        let store = PanelRegistryStore::from_root(temp.path().join("local-registry"));
        store
            .claim_ticket(&summary.id, None, "ticket-claimed-intake", "intake")
            .unwrap();
        let registry = store.snapshot().unwrap();

        let model = build_workspace_panel_with_registry(
            temp.path(),
            &live_pods(&["ticket-claimed-intake"]),
            &registry,
        );
        let row = model
            .rows
            .iter()
            .find(|row| row.title == "Claimed Planning")
            .unwrap();
        let claim = row.ticket.as_ref().unwrap().local_claim.as_ref().unwrap();

        assert_eq!(claim.pod_name, "ticket-claimed-intake");
        assert_eq!(claim.status, TicketLocalClaimStatus::Live);
        assert_eq!(row.related_pods, vec!["ticket-claimed-intake"]);
        assert!(
            row.subtitle
                .as_deref()
                .unwrap()
                .contains("claim: ticket-claimed-intake (live)")
        );
    }

    #[test]
    fn workspace_companion_pod_name_is_workspace_basename_without_suffix() {
        assert_eq!(
            workspace_companion_pod_name(Path::new("/home/hare/Projects/yoi")),
            "yoi"
        );
        assert_eq!(
            workspace_companion_pod_name(Path::new("/tmp/Yoi Workspace")),
            "yoi-workspace"
        );
        assert_eq!(
            workspace_companion_pod_name(Path::new("/tmp/.strange_日本語!!")),
            "strange"
        );
        assert_eq!(
            workspace_companion_pod_name(Path::new("/tmp/___")),
            "workspace"
        );
        let long = "a".repeat(120);
        let name = workspace_companion_pod_name(&PathBuf::from(format!("/tmp/{long}")));
        assert_eq!(name.chars().count(), 80);
        assert!(!name.ends_with("-companion"));
    }

    #[test]
    fn companion_lifecycle_decisions_follow_pod_state_without_ticket_gate() {
        assert_eq!(
            decide_companion_lifecycle(&CompanionPodPresence::Live),
            CompanionLifecyclePlan::ReportLive
        );
        assert_eq!(
            decide_companion_lifecycle(&CompanionPodPresence::Restorable),
            CompanionLifecyclePlan::Restore
        );
        assert_eq!(
            decide_companion_lifecycle(&CompanionPodPresence::Missing),
            CompanionLifecyclePlan::Spawn
        );
        assert!(matches!(
            decide_companion_lifecycle(&CompanionPodPresence::Unavailable(
                "corrupt metadata".to_string()
            )),
            CompanionLifecyclePlan::Unavailable(message) if message.contains("corrupt metadata")
        ));
    }

    #[test]
    fn workspace_orchestrator_pod_name_is_stable_and_safe() {
        assert_eq!(
            workspace_orchestrator_pod_name(Path::new("/tmp/Yoi Workspace")),
            "yoi-workspace-orchestrator"
        );
        assert_eq!(
            workspace_orchestrator_pod_name(Path::new("/tmp/.strange_日本語!!")),
            "strange-orchestrator"
        );
        assert_eq!(
            workspace_orchestrator_pod_name(Path::new("/tmp/___")),
            "workspace-orchestrator"
        );
        let long = "a".repeat(120);
        let name = workspace_orchestrator_pod_name(&PathBuf::from(format!("/tmp/{long}")));
        assert_eq!(name.chars().count(), 80);
        assert!(name.ends_with("-orchestrator"));
    }

    #[test]
    fn orchestrator_lifecycle_decisions_follow_ticket_gate_and_pod_state() {
        assert_eq!(
            decide_orchestrator_lifecycle(
                &TicketConfigAvailability::Absent,
                &OrchestratorPodPresence::Missing,
            ),
            OrchestratorLifecyclePlan::SkipNoTicketConfig
        );
        assert!(matches!(
            decide_orchestrator_lifecycle(
                &TicketConfigAvailability::Unusable("bad config".to_string()),
                &OrchestratorPodPresence::Missing,
            ),
            OrchestratorLifecyclePlan::Unavailable(message) if message.contains("bad config")
        ));
        assert_eq!(
            decide_orchestrator_lifecycle(
                &TicketConfigAvailability::Usable,
                &OrchestratorPodPresence::Live,
            ),
            OrchestratorLifecyclePlan::ReportLive
        );
        assert_eq!(
            decide_orchestrator_lifecycle(
                &TicketConfigAvailability::Usable,
                &OrchestratorPodPresence::Restorable,
            ),
            OrchestratorLifecyclePlan::Restore
        );
        assert_eq!(
            decide_orchestrator_lifecycle(
                &TicketConfigAvailability::Usable,
                &OrchestratorPodPresence::Missing,
            ),
            OrchestratorLifecyclePlan::Spawn
        );
        assert!(matches!(
            decide_orchestrator_lifecycle(
                &TicketConfigAvailability::Usable,
                &OrchestratorPodPresence::Unavailable("corrupt metadata".to_string()),
            ),
            OrchestratorLifecyclePlan::Unavailable(message) if message.contains("corrupt metadata")
        ));
    }

    #[test]
    fn existing_non_file_ticket_config_is_unusable_not_absent() {
        let temp = TempDir::new().unwrap();
        let config_parent = temp.path().join(".yoi");
        fs::create_dir_all(&config_parent).unwrap();
        fs::create_dir(config_parent.join("ticket.config.toml")).unwrap();

        assert!(matches!(
            ticket_config_availability(temp.path()),
            TicketConfigAvailability::Unusable(message) if message.contains("not a regular file")
        ));
        let model = build_workspace_panel(temp.path(), &empty_pods());

        assert!(model.header.ticket_configured);
        assert_eq!(
            model.composer.available_targets,
            vec![ComposerTarget::Companion]
        );
        assert!(model.header.diagnostics.iter().any(|diagnostic| {
            diagnostic.contains("Ticket config is unusable")
                && diagnostic.contains("not a regular file")
        }));
    }

    #[test]
    fn orchestrator_presence_can_be_decided_from_untruncated_authority() {
        let live = (0..60)
            .map(|index| LivePodInfo {
                pod_name: format!("pod-{index:02}"),
                socket_path: PathBuf::from(format!("/tmp/pod-{index:02}.sock")),
                status: Some(PodStatus::Idle),
                reachable: true,
                segment_id: None,
                summary: PodEntrySummary::default(),
            })
            .chain(std::iter::once(LivePodInfo {
                pod_name: "zz-workspace-orchestrator".to_string(),
                socket_path: PathBuf::from("/tmp/zz-workspace-orchestrator.sock"),
                status: Some(PodStatus::Idle),
                reachable: true,
                segment_id: None,
                summary: PodEntrySummary::default(),
            }))
            .collect::<Vec<_>>();
        let visible = PodList::from_sources(
            crate::pod_list::PodVisibilitySource::ResumePicker,
            vec![],
            live.clone(),
            None,
            50,
        );
        let authority = PodList::from_sources(
            crate::pod_list::PodVisibilitySource::ResumePicker,
            vec![],
            live,
            None,
            usize::MAX,
        );

        assert_eq!(
            orchestrator_pod_presence("zz-workspace-orchestrator", &visible),
            OrchestratorPodPresence::Missing
        );
        assert_eq!(
            orchestrator_pod_presence("zz-workspace-orchestrator", &authority),
            OrchestratorPodPresence::Live
        );
        assert_eq!(
            decide_orchestrator_lifecycle(
                &TicketConfigAvailability::Usable,
                &orchestrator_pod_presence("zz-workspace-orchestrator", &authority),
            ),
            OrchestratorLifecyclePlan::ReportLive
        );
    }
}
