use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
#[cfg(feature = "e2e-test")]
use std::time::Instant;

use protocol::PodStatus;
use ticket::config::{
    DEFAULT_TICKET_BACKEND_RELATIVE_PATH, TICKET_CONFIG_RELATIVE_PATH, TicketConfig,
    TicketOrchestrationConfig,
};
use ticket::{
    LocalTicketBackend, TicketBackend, TicketError, TicketEvent, TicketFilter, TicketIdOrSlug,
    TicketInvalidRecord, TicketMeta, TicketRelationBlocker, TicketSummary, TicketWorkflowState,
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
    InvalidTicket(String),
    TicketIntakePod { ticket_id: String, pod_name: String },
    Pod(String),
}

impl PanelRowKey {
    pub(crate) fn pod_name(&self) -> Option<&str> {
        match self {
            Self::Pod(name) | Self::TicketIntakePod { pod_name: name, .. } => Some(name),
            Self::Ticket(_) | Self::InvalidTicket(_) => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PanelRowKind {
    Planning,
    Ticket,
    Review,
    ActiveWork,
    TicketIntakePod,
    Pod,
    InvalidTicket,
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
    pub(crate) orchestration_overlay: Option<TicketStateOverlay>,
    pub(crate) next_action: Option<NextUserAction>,
    pub(crate) updated_at: Option<String>,
    pub(crate) latest_event_kind: Option<String>,
    pub(crate) latest_event_excerpt: Option<String>,
    pub(crate) blocked_reason: Option<String>,
    pub(crate) related_pods: Vec<String>,
    pub(crate) local_claim: Option<TicketLocalClaimEntry>,
    pub(crate) intake_pods: Vec<TicketAssociatedIntakeEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TicketStateOverlay {
    pub(crate) source: String,
    pub(crate) workflow_state: TicketWorkflowState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TicketAssociatedIntakeEntry {
    pub(crate) ticket_id: String,
    pub(crate) pod_name: String,
    pub(crate) status: TicketLocalClaimStatus,
    pub(crate) source: TicketAssociatedIntakeSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TicketAssociatedIntakeSource {
    LocalClaim,
    RelatedSession,
}

impl TicketAssociatedIntakeSource {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::LocalClaim => "local claim",
            Self::RelatedSession => "related session",
        }
    }
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
        matches!(
            self.kind,
            PanelRowKind::Planning
                | PanelRowKind::Ticket
                | PanelRowKind::Review
                | PanelRowKind::ActiveWork
        )
    }

    pub(crate) fn is_ticket_section_row(&self) -> bool {
        self.is_ticket_action()
            || matches!(
                self.kind,
                PanelRowKind::TicketIntakePod | PanelRowKind::InvalidTicket
            )
    }
}

const MAX_POD_NAME_CHARS: usize = 80;
const MAX_ASSOCIATED_INTAKE_ROWS_PER_TICKET: usize = 3;
const MAX_INVALID_TICKET_PLACEHOLDER_ROWS: usize = 5;
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct OrchestrationTicketOverlay {
    states: BTreeMap<String, TicketStateOverlay>,
    diagnostics: Vec<String>,
}

impl OrchestrationTicketOverlay {
    fn empty() -> Self {
        Self {
            states: BTreeMap::new(),
            diagnostics: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct OrchestrationWorktreeLayout {
    path: PathBuf,
    branch: String,
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

fn load_orchestration_ticket_overlay(
    workspace_root: &Path,
    config: &TicketConfig,
) -> OrchestrationTicketOverlay {
    let layout = orchestration_worktree_layout(workspace_root, &config.orchestration);
    if !layout.path.exists() {
        return OrchestrationTicketOverlay::empty();
    }
    match validate_orchestration_overlay_source(workspace_root, &layout) {
        Ok(()) => {
            load_orchestration_ticket_overlay_states(&layout.path, config.ticket_record_language())
                .unwrap_or_else(|message| OrchestrationTicketOverlay {
                    states: BTreeMap::new(),
                    diagnostics: vec![bounded_panel_diagnostic(format!(
                        "Orchestration Ticket overlay unavailable: {message}"
                    ))],
                })
        }
        Err(message) => OrchestrationTicketOverlay {
            states: BTreeMap::new(),
            diagnostics: vec![bounded_panel_diagnostic(format!(
                "Orchestration Ticket overlay unavailable: {message}"
            ))],
        },
    }
}

fn orchestration_worktree_layout(
    workspace_root: &Path,
    config: &TicketOrchestrationConfig,
) -> OrchestrationWorktreeLayout {
    OrchestrationWorktreeLayout {
        path: workspace_root
            .join(config.worktree_dir())
            .join(config.worktree_name()),
        branch: config.effective_branch_name().to_string(),
    }
}

fn load_orchestration_ticket_overlay_states(
    worktree_root: &Path,
    record_language: Option<&str>,
) -> Result<OrchestrationTicketOverlay, String> {
    let ticket_root = worktree_root.join(DEFAULT_TICKET_BACKEND_RELATIVE_PATH);
    if !ticket_root.is_dir() {
        return Ok(OrchestrationTicketOverlay {
            states: BTreeMap::new(),
            diagnostics: vec![bounded_panel_diagnostic(format!(
                "Orchestration worktree has no {} directory",
                DEFAULT_TICKET_BACKEND_RELATIVE_PATH
            ))],
        });
    }
    let backend = LocalTicketBackend::new(ticket_root).with_record_language(record_language);
    let partial = backend
        .list_partial(TicketFilter::all())
        .map_err(|error| error.to_string())?;
    let mut states = BTreeMap::new();
    for summary in partial.tickets {
        states.insert(
            summary.id,
            TicketStateOverlay {
                source: "orchestration".to_string(),
                workflow_state: summary.workflow_state,
            },
        );
    }
    let diagnostics = invalid_ticket_diagnostics(partial.invalid_records.len());
    Ok(OrchestrationTicketOverlay {
        states,
        diagnostics,
    })
}

fn validate_orchestration_overlay_source(
    workspace_root: &Path,
    layout: &OrchestrationWorktreeLayout,
) -> Result<(), String> {
    if !layout.path.exists() {
        return Err(format!(
            "expected worktree {} does not exist",
            layout.path.display()
        ));
    }
    if !layout.path.is_dir() {
        return Err(format!(
            "expected worktree {} is not a directory",
            layout.path.display()
        ));
    }
    let expected_path = layout
        .path
        .canonicalize()
        .map_err(|error| format!("could not canonicalize expected worktree: {error}"))?;
    let overlay_top = git_output(&layout.path, &["rev-parse", "--show-toplevel"])?;
    let overlay_top = PathBuf::from(overlay_top);
    let overlay_top = overlay_top
        .canonicalize()
        .map_err(|error| format!("could not canonicalize overlay git top-level: {error}"))?;
    if overlay_top != expected_path {
        return Err(format!(
            "overlay git top-level {} does not match expected path {}",
            overlay_top.display(),
            expected_path.display()
        ));
    }

    let current_common_dir = git_common_dir(workspace_root)?;
    let overlay_common_dir = git_common_dir(&layout.path)?;
    if current_common_dir != overlay_common_dir {
        return Err("expected worktree is from a different git common-dir".to_string());
    }

    let overlay_branch = git_output(&layout.path, &["branch", "--show-current"])?;
    if overlay_branch != layout.branch {
        return Err(format!(
            "expected branch {} but worktree is on {}",
            layout.branch, overlay_branch
        ));
    }

    Ok(())
}

fn git_common_dir(worktree_root: &Path) -> Result<PathBuf, String> {
    let common_dir = git_output(worktree_root, &["rev-parse", "--git-common-dir"])?;
    let common_dir_path = PathBuf::from(common_dir);
    let absolute = if common_dir_path.is_absolute() {
        common_dir_path
    } else {
        worktree_root.join(common_dir_path)
    };
    absolute
        .canonicalize()
        .map_err(|error| format!("could not canonicalize git common-dir: {error}"))
}

fn git_output(worktree_root: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(worktree_root)
        .output()
        .map_err(|error| format!("could not run git {}: {error}", args.join(" ")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let message = if stderr.is_empty() {
            format!("git {} exited with {}", args.join(" "), output.status)
        } else {
            format!("git {} failed: {stderr}", args.join(" "))
        };
        return Err(message);
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg_attr(feature = "e2e-test", allow(dead_code))]
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

#[cfg(feature = "e2e-test")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkspacePanelE2eSourceTiming {
    pub(crate) source: &'static str,
    pub(crate) elapsed_ms: u128,
}

#[cfg(feature = "e2e-test")]
pub(crate) fn build_workspace_panel_with_e2e_timings(
    workspace_root: &Path,
    pods: &PodList,
) -> (WorkspacePanelViewModel, Vec<WorkspacePanelE2eSourceTiming>) {
    let mut timings = Vec::new();
    let started = Instant::now();
    let registry = match PanelRegistryStore::default_for_workspace(workspace_root)
        .and_then(|store| store.snapshot())
    {
        Ok(snapshot) => snapshot,
        Err(error) => {
            timings.push(WorkspacePanelE2eSourceTiming {
                source: "local_claim_scan",
                elapsed_ms: started.elapsed().as_millis(),
            });
            let mut model = WorkspacePanelViewModel::empty(workspace_root);
            model
                .header
                .diagnostics
                .push(bounded_panel_diagnostic(format!(
                    "Panel local role registry unavailable: {error}"
                )));
            return (
                build_workspace_panel_with_registry_model(
                    model,
                    workspace_root,
                    pods,
                    &PanelRegistrySnapshot::empty(),
                ),
                timings,
            );
        }
    };
    timings.push(WorkspacePanelE2eSourceTiming {
        source: "local_claim_scan",
        elapsed_ms: started.elapsed().as_millis(),
    });

    let mut model = WorkspacePanelViewModel::empty(workspace_root);
    let started = Instant::now();
    let availability = ticket_config_availability(workspace_root);
    timings.push(WorkspacePanelE2eSourceTiming {
        source: "ticket_config_probe",
        elapsed_ms: started.elapsed().as_millis(),
    });
    match availability {
        TicketConfigAvailability::Absent => {}
        TicketConfigAvailability::Usable => {
            model.header.ticket_configured = true;
            model.composer = WorkspacePanelComposer::ticket_enabled();
            let started = Instant::now();
            match TicketConfig::load_workspace(workspace_root) {
                Ok(config) => {
                    timings.push(WorkspacePanelE2eSourceTiming {
                        source: "ticket_config_parse",
                        elapsed_ms: started.elapsed().as_millis(),
                    });
                    model.header.ticket_root = config.backend_root().to_path_buf();
                    let backend = LocalTicketBackend::new(config.backend_root().to_path_buf())
                        .with_record_language(config.ticket_record_language());
                    let started = Instant::now();
                    let orchestration_overlay =
                        load_orchestration_ticket_overlay(workspace_root, &config);
                    timings.push(WorkspacePanelE2eSourceTiming {
                        source: "orchestration_overlay_validation_read_git",
                        elapsed_ms: started.elapsed().as_millis(),
                    });
                    let started = Instant::now();
                    match build_ticket_rows(
                        &backend,
                        pods,
                        &registry,
                        &orchestration_overlay.states,
                    ) {
                        Ok(ticket_rows) => {
                            timings.push(WorkspacePanelE2eSourceTiming {
                                source: "ticket_scan_parse",
                                elapsed_ms: started.elapsed().as_millis(),
                            });
                            model.rows.extend(ticket_rows.rows);
                            model.header.diagnostics.extend(ticket_rows.diagnostics);
                            model
                                .header
                                .diagnostics
                                .extend(orchestration_overlay.diagnostics);
                        }
                        Err(error) => {
                            timings.push(WorkspacePanelE2eSourceTiming {
                                source: "ticket_scan_parse",
                                elapsed_ms: started.elapsed().as_millis(),
                            });
                            model
                                .header
                                .diagnostics
                                .push(bounded_panel_diagnostic(format!(
                                    "Ticket rows unavailable: {error}"
                                )))
                        }
                    }
                }
                Err(error) => {
                    timings.push(WorkspacePanelE2eSourceTiming {
                        source: "ticket_config_parse",
                        elapsed_ms: started.elapsed().as_millis(),
                    });
                    model
                        .header
                        .diagnostics
                        .push(bounded_panel_diagnostic(format!(
                            "Ticket config is unusable: {error}"
                        )))
                }
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

    let started = Instant::now();
    model.rows.extend(pod_rows(pods));
    timings.push(WorkspacePanelE2eSourceTiming {
        source: "pod_row_materialization",
        elapsed_ms: started.elapsed().as_millis(),
    });
    (model, timings)
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
                    let orchestration_overlay =
                        load_orchestration_ticket_overlay(workspace_root, &config);
                    match build_ticket_rows(&backend, pods, registry, &orchestration_overlay.states)
                    {
                        Ok(ticket_rows) => {
                            model.rows.extend(ticket_rows.rows);
                            model.header.diagnostics.extend(ticket_rows.diagnostics);
                            model
                                .header
                                .diagnostics
                                .extend(orchestration_overlay.diagnostics);
                        }
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
        None,
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct TicketRowsBuild {
    rows: Vec<PanelRow>,
    diagnostics: Vec<String>,
}

fn build_ticket_rows(
    backend: &LocalTicketBackend,
    pods: &PodList,
    registry: &PanelRegistrySnapshot,
    orchestration_overlay: &BTreeMap<String, TicketStateOverlay>,
) -> ticket::Result<TicketRowsBuild> {
    let partial = backend.list_partial(TicketFilter::all())?;
    let mut ticket_rows = Vec::new();
    let mut invalid_records = partial.invalid_records;
    for summary in partial.tickets {
        if summary.workflow_state == TicketWorkflowState::Closed {
            continue;
        }
        match backend.show_partial(TicketIdOrSlug::Query(summary.id.clone())) {
            Ok(ticket) => {
                let current_ticket_invalid = ticket
                    .invalid_records
                    .iter()
                    .any(|record| record.label == summary.id);
                invalid_records.extend(ticket.invalid_records);
                if current_ticket_invalid {
                    continue;
                }
                let overlay = orchestration_overlay.get(&summary.id);
                ticket_rows.push(ticket_row(
                    summary,
                    &ticket.ticket.events,
                    &ticket.ticket.relations.blockers,
                    pods,
                    registry,
                    overlay,
                ));
            }
            Err(_) => invalid_records.push(TicketInvalidRecord {
                label: summary.id,
                reason: "could not load ticket detail".to_string(),
            }),
        }
    }
    ticket_rows.sort_by(|a, b| {
        a.priority
            .cmp(&b.priority)
            .then_with(|| row_updated_at(b).cmp(row_updated_at(a)))
            .then_with(|| a.title.cmp(&b.title))
    });

    let mut rows = Vec::new();
    for row in ticket_rows {
        let intake_rows = ticket_intake_pod_rows(&row);
        rows.push(row);
        rows.extend(intake_rows);
    }

    let invalid_records = dedupe_invalid_ticket_records(invalid_records);
    let diagnostics = invalid_ticket_diagnostics(invalid_records.len());
    rows.extend(invalid_ticket_rows(&invalid_records));

    Ok(TicketRowsBuild { rows, diagnostics })
}

fn dedupe_invalid_ticket_records(records: Vec<TicketInvalidRecord>) -> Vec<TicketInvalidRecord> {
    let mut deduped = Vec::new();
    for record in records {
        if deduped
            .iter()
            .any(|existing: &TicketInvalidRecord| existing.label == record.label)
        {
            continue;
        }
        deduped.push(record);
    }
    deduped
}

fn invalid_ticket_diagnostics(invalid_count: usize) -> Vec<String> {
    if invalid_count == 0 {
        return Vec::new();
    }
    let suffix = if invalid_count > MAX_INVALID_TICKET_PLACEHOLDER_ROWS {
        format!(
            "; showing first {} placeholder rows",
            MAX_INVALID_TICKET_PLACEHOLDER_ROWS
        )
    } else {
        String::new()
    };
    vec![bounded_panel_diagnostic(format!(
        "Ticket records partially loaded: {invalid_count} invalid record(s) unavailable for actions{suffix}."
    ))]
}

fn invalid_ticket_rows(records: &[TicketInvalidRecord]) -> Vec<PanelRow> {
    records
        .iter()
        .take(MAX_INVALID_TICKET_PLACEHOLDER_ROWS)
        .map(invalid_ticket_row)
        .collect()
}

fn invalid_ticket_row(record: &TicketInvalidRecord) -> PanelRow {
    PanelRow {
        key: PanelRowKey::InvalidTicket(record.label.clone()),
        kind: PanelRowKind::InvalidTicket,
        title: format!("Invalid Ticket record: {}", record.label),
        subtitle: Some(record.reason.clone()),
        status: "invalid".to_string(),
        priority: ActionPriority::Background,
        next_action: None,
        ticket: None,
        related_pods: Vec::new(),
        disabled_reason: Some(
            "Invalid Ticket record is diagnostics-only; lifecycle actions are disabled."
                .to_string(),
        ),
        key_hint: Some(
            "Actions unavailable until the Ticket record is repaired manually.".to_string(),
        ),
    }
}

fn ticket_row(
    summary: TicketSummary,
    events: &[TicketEvent],
    relation_blockers: &[TicketRelationBlocker],
    pods: &PodList,
    registry: &PanelRegistrySnapshot,
    orchestration_overlay: Option<&TicketStateOverlay>,
) -> PanelRow {
    let local_claim = local_claim_for_ticket(&summary, pods, registry);
    let intake_pods =
        associated_intake_entries_for_ticket(&summary, pods, registry, local_claim.as_ref());
    let mut related_pods = Vec::new();
    if let Some(claim) = local_claim.as_ref() {
        related_pods.push(claim.pod_name.clone());
    }
    for pod_name in intake_pods.iter().map(|intake| intake.pod_name.clone()) {
        if !related_pods.iter().any(|existing| existing == &pod_name) {
            related_pods.push(pod_name);
        }
    }
    let visible_overlay = orchestration_overlay
        .filter(|overlay| {
            overlay_state_has_progressed(summary.workflow_state, overlay.workflow_state)
        })
        .cloned();
    let mut derived = derive_ticket_state(&summary, relation_blockers);
    if let Some(overlay) = visible_overlay.as_ref() {
        apply_orchestration_overlay_to_derived(&mut derived, summary.workflow_state, overlay);
    }
    let latest_event = events.last();
    let state_display = ticket_state_display(summary.workflow_state, visible_overlay.as_ref());
    let entry = TicketPanelEntry {
        id: summary.id.clone(),
        title: summary.title.clone(),
        priority: summary.priority.clone(),
        workflow_state: summary.workflow_state,
        workflow_state_explicit: summary.workflow_state_explicit,
        orchestration_overlay: visible_overlay,
        next_action: derived.action,
        updated_at: summary.updated_at.clone(),
        latest_event_kind: latest_event.map(|event| event.kind.as_str().to_string()),
        latest_event_excerpt: latest_event.and_then(|event| excerpt(event.body.as_str(), 72)),
        blocked_reason: derived.blocked_reason.clone(),
        related_pods: related_pods.clone(),
        local_claim,
        intake_pods,
    };
    let subtitle = ticket_subtitle(&entry);
    PanelRow {
        key: PanelRowKey::Ticket(summary.id),
        kind: derived.kind,
        title: summary.title,
        subtitle,
        status: state_display,
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

fn workflow_state_progress_rank(state: TicketWorkflowState) -> u8 {
    match state {
        TicketWorkflowState::Planning => 0,
        TicketWorkflowState::Ready => 1,
        TicketWorkflowState::Queued => 2,
        TicketWorkflowState::InProgress => 3,
        TicketWorkflowState::Done => 4,
        TicketWorkflowState::Closed => 5,
    }
}

fn overlay_state_has_progressed(local: TicketWorkflowState, overlay: TicketWorkflowState) -> bool {
    workflow_state_progress_rank(overlay) > workflow_state_progress_rank(local)
}

fn ticket_state_display(
    local: TicketWorkflowState,
    overlay: Option<&TicketStateOverlay>,
) -> String {
    match overlay {
        Some(overlay) => format!(
            "{}→{}",
            compact_ticket_state_label(local),
            compact_ticket_state_label(overlay.workflow_state)
        ),
        None => local.as_str().to_string(),
    }
}

fn compact_ticket_state_label(state: TicketWorkflowState) -> &'static str {
    match state {
        TicketWorkflowState::Planning => "plan",
        TicketWorkflowState::Ready => "ready",
        TicketWorkflowState::Queued => "q",
        TicketWorkflowState::InProgress => "prog",
        TicketWorkflowState::Done => "done",
        TicketWorkflowState::Closed => "cls",
    }
}

fn apply_orchestration_overlay_to_derived(
    derived: &mut DerivedTicketState,
    local: TicketWorkflowState,
    overlay: &TicketStateOverlay,
) {
    derived.action = Some(NextUserAction::Wait);
    let overlay_state = overlay.workflow_state.as_str();
    match overlay.workflow_state {
        TicketWorkflowState::Done | TicketWorkflowState::Closed => {
            derived.kind = PanelRowKind::Review;
            derived.priority = ActionPriority::Background;
            derived.disabled_reason = Some(format!(
                "{} worktree overlay shows Ticket state {overlay_state}; local state remains {} until merge/review/close authority updates the current branch.",
                overlay.source,
                local.as_str()
            ));
            derived.key_hint = Some(format!(
                "Merge pending: local: {} · {}: {overlay_state}",
                local.as_str(),
                overlay.source
            ));
        }
        TicketWorkflowState::InProgress | TicketWorkflowState::Queued => {
            derived.kind = PanelRowKind::ActiveWork;
            derived.priority = ActionPriority::ActiveWork;
            derived.disabled_reason = Some(format!(
                "{} worktree overlay shows Ticket state {overlay_state}; local state remains {} and duplicate queue/start actions are suppressed.",
                overlay.source,
                local.as_str()
            ));
            derived.key_hint = Some(format!(
                "Progress overlay: local: {} · {}: {overlay_state}",
                local.as_str(),
                overlay.source
            ));
        }
        TicketWorkflowState::Planning | TicketWorkflowState::Ready => {}
    }
}

fn format_relation_blockers(blockers: &[&TicketRelationBlocker]) -> String {
    let shown_blockers = blockers.iter().take(3).count();
    let mut formatted = blockers
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
    let remaining_blockers = blockers.len().saturating_sub(shown_blockers);
    if remaining_blockers > 0 {
        formatted.push_str(&format!(" (+{remaining_blockers} more)"));
    }
    formatted
}

fn relation_blocker_allows_ready_queue(blocker: &TicketRelationBlocker) -> bool {
    matches!(
        blocker.blocking_state,
        TicketWorkflowState::Queued | TicketWorkflowState::InProgress
    )
}

fn derive_ticket_state(
    summary: &TicketSummary,
    relation_blockers: &[TicketRelationBlocker],
) -> DerivedTicketState {
    if !relation_blockers.is_empty() {
        let active_blockers = relation_blockers
            .iter()
            .filter(|blocker| !relation_blocker_allows_ready_queue(blocker))
            .collect::<Vec<_>>();
        if !active_blockers.is_empty() || summary.workflow_state != TicketWorkflowState::Ready {
            let blockers_to_report = if active_blockers.is_empty() {
                relation_blockers.iter().collect::<Vec<_>>()
            } else {
                active_blockers
            };
            let blockers = format_relation_blockers(&blockers_to_report);
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

        let blockers = format_relation_blockers(
            &relation_blockers
                .iter()
                .collect::<Vec<&TicketRelationBlocker>>(),
        );
        return DerivedTicketState {
            kind: PanelRowKind::Ticket,
            priority: ActionPriority::ReadyForQueue,
            action: Some(NextUserAction::Queue),
            disabled_reason: None,
            key_hint: Some(format!(
                "Queue allowed: prerequisites are already queued/in progress; Orchestrator will preserve order ({blockers})."
            )),
            blocked_reason: None,
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

fn associated_intake_entries_for_ticket(
    summary: &TicketSummary,
    pods: &PodList,
    registry: &PanelRegistrySnapshot,
    local_claim: Option<&TicketLocalClaimEntry>,
) -> Vec<TicketAssociatedIntakeEntry> {
    let mut entries = Vec::new();
    if let Some(claim) = local_claim.filter(|claim| is_intake_role(&claim.role)) {
        entries.push(TicketAssociatedIntakeEntry {
            ticket_id: summary.id.clone(),
            pod_name: claim.pod_name.clone(),
            status: claim.status,
            source: TicketAssociatedIntakeSource::LocalClaim,
        });
    }

    let mut related_sessions = registry
        .sessions
        .iter()
        .filter(|session| {
            is_intake_role(&session.role)
                && session
                    .related_tickets
                    .iter()
                    .any(|related| related.id == summary.id.as_str())
        })
        .map(|session| session.pod_name.clone())
        .collect::<Vec<_>>();
    related_sessions.sort();
    related_sessions.dedup();

    for pod_name in related_sessions {
        if entries.iter().any(|entry| entry.pod_name == pod_name) {
            continue;
        }
        entries.push(TicketAssociatedIntakeEntry {
            ticket_id: summary.id.clone(),
            status: local_claim_status_for_pod(&pod_name, pods),
            pod_name,
            source: TicketAssociatedIntakeSource::RelatedSession,
        });
        if entries.len() >= MAX_ASSOCIATED_INTAKE_ROWS_PER_TICKET {
            break;
        }
    }

    entries.truncate(MAX_ASSOCIATED_INTAKE_ROWS_PER_TICKET);
    entries
}

fn is_intake_role(role: &str) -> bool {
    role.eq_ignore_ascii_case("intake")
}

fn ticket_intake_pod_rows(row: &PanelRow) -> Vec<PanelRow> {
    row.ticket
        .as_ref()
        .map(|ticket| {
            ticket
                .intake_pods
                .iter()
                .map(ticket_intake_pod_row)
                .collect()
        })
        .unwrap_or_default()
}

fn ticket_intake_pod_row(intake: &TicketAssociatedIntakeEntry) -> PanelRow {
    let stale = intake.status == TicketLocalClaimStatus::Stale;
    PanelRow {
        key: PanelRowKey::TicketIntakePod {
            ticket_id: intake.ticket_id.clone(),
            pod_name: intake.pod_name.clone(),
        },
        kind: PanelRowKind::TicketIntakePod,
        title: format!("Intake Pod: {}", intake.pod_name),
        subtitle: Some(format!(
            "Ticket {} · {} · {}",
            intake.ticket_id,
            intake.source.label(),
            intake.status.label()
        )),
        status: intake.status.label().to_string(),
        priority: ActionPriority::ActiveWork,
        next_action: if stale {
            None
        } else {
            Some(NextUserAction::OpenPod)
        },
        ticket: None,
        related_pods: vec![intake.pod_name.clone()],
        disabled_reason: if stale {
            Some(
                "Associated Intake Pod is stale; no live or restorable Pod entry is available."
                    .to_string(),
            )
        } else {
            None
        },
        key_hint: Some(if stale {
            "Stale Intake claim/session; restore is unavailable".to_string()
        } else {
            "Open/attach this Ticket's Intake Pod".to_string()
        }),
    }
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
    let mut parts = vec![format!(
        "{} · {}",
        entry.id,
        ticket_state_display(entry.workflow_state, entry.orchestration_overlay.as_ref())
    )];
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

#[allow(dead_code)]
#[cfg(test)]
mod tests {
    use super::*;
    use crate::pod_list::{LivePodInfo, PodEntrySummary};
    use crate::role_session_registry::{PanelRegistryStore, RelatedTicketRef, RoleSessionOrigin};
    use std::fs;
    use std::path::{Path, PathBuf};
    use tempfile::TempDir;
    use ticket::{
        NewTicket, NewTicketRelation, TicketIdOrSlug, TicketRelationKind, TicketStateChange,
        TicketWorkflowState,
    };

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
        create_ticket_with_id(backend, title, configure);
    }

    fn create_ticket_with_id(
        backend: &LocalTicketBackend,
        title: &str,
        configure: impl FnOnce(&mut NewTicket),
    ) -> String {
        let mut input = NewTicket::new(title);
        configure(&mut input);
        backend.create(input).unwrap().id
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

    fn run_git(workspace_root: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(workspace_root)
            .output()
            .unwrap_or_else(|error| panic!("failed to run git {args:?}: {error}"));
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn init_git_repo(workspace_root: &Path) {
        run_git(workspace_root, &["init", "--initial-branch", "main"]);
        run_git(workspace_root, &["config", "user.name", "Panel Test"]);
        run_git(
            workspace_root,
            &["config", "user.email", "panel-test@example.invalid"],
        );
        fs::write(workspace_root.join("README.md"), "panel test\n").unwrap();
        run_git(workspace_root, &["add", "README.md"]);
        run_git(workspace_root, &["commit", "-m", "init"]);
    }

    fn add_orchestration_worktree(workspace_root: &Path, branch: &str) -> PathBuf {
        let orchestration_root = workspace_root.join(".worktree/orchestration");
        fs::create_dir_all(orchestration_root.parent().unwrap()).unwrap();
        run_git(
            workspace_root,
            &[
                "worktree",
                "add",
                "-b",
                branch,
                orchestration_root.to_str().unwrap(),
                "HEAD",
            ],
        );
        orchestration_root
    }

    fn copy_ticket_to_overlay(workspace_root: &Path, orchestration_root: &Path, id: &str) {
        let local_ticket_dir = workspace_root.join(".yoi/tickets").join(id);
        let overlay_ticket_dir = orchestration_root.join(".yoi/tickets").join(id);
        fs::create_dir_all(overlay_ticket_dir.parent().unwrap()).unwrap();
        fs::create_dir_all(&overlay_ticket_dir).unwrap();
        fs::copy(
            local_ticket_dir.join("item.md"),
            overlay_ticket_dir.join("item.md"),
        )
        .unwrap();
        let local_thread = local_ticket_dir.join("thread.md");
        if local_thread.exists() {
            fs::copy(local_thread, overlay_ticket_dir.join("thread.md")).unwrap();
        }
    }

    fn set_ticket_state(backend: &LocalTicketBackend, id: &str, state: TicketWorkflowState) {
        loop {
            let ticket = backend.show(TicketIdOrSlug::Id(id.to_string())).unwrap();
            if ticket.meta.workflow_state == state {
                break;
            }
            let next = match (ticket.meta.workflow_state, state) {
                (TicketWorkflowState::Queued, TicketWorkflowState::InProgress)
                | (TicketWorkflowState::Queued, TicketWorkflowState::Done) => {
                    TicketWorkflowState::InProgress
                }
                (TicketWorkflowState::InProgress, TicketWorkflowState::Done) => {
                    TicketWorkflowState::Done
                }
                (from, to) => panic!("unsupported test transition {from} -> {to}"),
            };
            backend
                .set_workflow_state(
                    TicketIdOrSlug::Id(id.to_string()),
                    TicketStateChange::new(
                        ticket.meta.workflow_state.as_str(),
                        next.as_str(),
                        "test",
                        format!("test state -> {}", next.as_str()),
                    ),
                )
                .unwrap();
        }
    }

    fn ticket_row_by_title<'a>(model: &'a WorkspacePanelViewModel, title: &str) -> &'a PanelRow {
        model
            .rows
            .iter()
            .find(|row| row.title == title)
            .unwrap_or_else(|| panic!("missing row for {title}"))
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
    fn workspace_panel_joins_orchestration_overlay_by_ticket_id() {
        let temp = TempDir::new().unwrap();
        init_git_repo(temp.path());
        write_ticket_config(temp.path());
        let orchestration_root = add_orchestration_worktree(temp.path(), "orchestration");
        let backend = LocalTicketBackend::new(temp.path().join(".yoi/tickets"));
        let id = create_ticket_with_id(&backend, "Overlay Match", |input| {
            input.workflow_state = Some(TicketWorkflowState::Queued);
        });
        create_ticket(&backend, "Local Only", |input| {
            input.workflow_state = Some(TicketWorkflowState::Queued);
        });
        copy_ticket_to_overlay(temp.path(), &orchestration_root, &id);
        let overlay_backend = LocalTicketBackend::new(orchestration_root.join(".yoi/tickets"));
        set_ticket_state(&overlay_backend, &id, TicketWorkflowState::InProgress);
        create_ticket(&overlay_backend, "Overlay Only", |input| {
            input.workflow_state = Some(TicketWorkflowState::Done);
        });

        let model = build_workspace_panel(temp.path(), &empty_pods());

        let matched = ticket_row_by_title(&model, "Overlay Match");
        assert_eq!(matched.status, "q→prog");
        assert_eq!(
            matched.ticket.as_ref().unwrap().workflow_state,
            TicketWorkflowState::Queued
        );
        assert_eq!(
            matched
                .ticket
                .as_ref()
                .unwrap()
                .orchestration_overlay
                .as_ref()
                .unwrap()
                .workflow_state,
            TicketWorkflowState::InProgress
        );
        assert_eq!(ticket_row_by_title(&model, "Local Only").status, "queued");
        assert!(model.rows.iter().all(|row| row.title != "Overlay Only"));
    }

    #[test]
    fn workspace_panel_displays_queued_plus_orchestration_inprogress_without_mutating_local_ticket()
    {
        let temp = TempDir::new().unwrap();
        init_git_repo(temp.path());
        write_ticket_config(temp.path());
        let orchestration_root = add_orchestration_worktree(temp.path(), "orchestration");
        let backend = LocalTicketBackend::new(temp.path().join(".yoi/tickets"));
        let id = create_ticket_with_id(&backend, "Overlay In Progress", |input| {
            input.workflow_state = Some(TicketWorkflowState::Queued);
        });
        copy_ticket_to_overlay(temp.path(), &orchestration_root, &id);
        let overlay_backend = LocalTicketBackend::new(orchestration_root.join(".yoi/tickets"));
        set_ticket_state(&overlay_backend, &id, TicketWorkflowState::InProgress);
        let local_item = temp.path().join(".yoi/tickets").join(&id).join("item.md");
        let before = fs::read_to_string(&local_item).unwrap();

        let model = build_workspace_panel(temp.path(), &empty_pods());

        let row = ticket_row_by_title(&model, "Overlay In Progress");
        assert_eq!(row.status, "q→prog");
        assert_eq!(row.next_action, Some(NextUserAction::Wait));
        assert_eq!(row.kind, PanelRowKind::ActiveWork);
        assert_eq!(fs::read_to_string(&local_item).unwrap(), before);
        let local_ticket = backend.show(TicketIdOrSlug::Id(id)).unwrap();
        assert_eq!(
            local_ticket.meta.workflow_state,
            TicketWorkflowState::Queued
        );
    }

    #[test]
    fn workspace_panel_displays_queued_plus_orchestration_done_as_merge_pending_without_queue() {
        let temp = TempDir::new().unwrap();
        init_git_repo(temp.path());
        write_ticket_config(temp.path());
        let orchestration_root = add_orchestration_worktree(temp.path(), "orchestration");
        let backend = LocalTicketBackend::new(temp.path().join(".yoi/tickets"));
        let id = create_ticket_with_id(&backend, "Overlay Done", |input| {
            input.workflow_state = Some(TicketWorkflowState::Queued);
        });
        copy_ticket_to_overlay(temp.path(), &orchestration_root, &id);
        let overlay_backend = LocalTicketBackend::new(orchestration_root.join(".yoi/tickets"));
        set_ticket_state(&overlay_backend, &id, TicketWorkflowState::Done);

        let model = build_workspace_panel(temp.path(), &empty_pods());

        let row = ticket_row_by_title(&model, "Overlay Done");
        assert_eq!(row.status, "q→done");
        assert_eq!(row.kind, PanelRowKind::Review);
        assert_eq!(row.next_action, Some(NextUserAction::Wait));
        assert_ne!(row.next_action, Some(NextUserAction::Queue));
        assert!(
            row.disabled_reason
                .as_deref()
                .unwrap()
                .contains("merge/review/close authority")
        );
    }

    #[test]
    fn workspace_panel_ignores_orchestration_overlay_on_branch_mismatch() {
        let temp = TempDir::new().unwrap();
        init_git_repo(temp.path());
        write_ticket_config(temp.path());
        let orchestration_root = add_orchestration_worktree(temp.path(), "other-branch");
        let backend = LocalTicketBackend::new(temp.path().join(".yoi/tickets"));
        let id = create_ticket_with_id(&backend, "Branch Mismatch", |input| {
            input.workflow_state = Some(TicketWorkflowState::Queued);
        });
        copy_ticket_to_overlay(temp.path(), &orchestration_root, &id);
        let overlay_backend = LocalTicketBackend::new(orchestration_root.join(".yoi/tickets"));
        set_ticket_state(&overlay_backend, &id, TicketWorkflowState::Done);

        let model = build_workspace_panel(temp.path(), &empty_pods());

        let row = ticket_row_by_title(&model, "Branch Mismatch");
        assert_eq!(row.status, "queued");
        assert!(row.ticket.as_ref().unwrap().orchestration_overlay.is_none());
        let diagnostics = model.header.diagnostics.join("\n");
        assert!(diagnostics.contains("expected branch orchestration"));
    }

    #[test]
    fn workspace_panel_falls_back_when_orchestration_worktree_is_missing() {
        let temp = TempDir::new().unwrap();
        write_ticket_config(temp.path());
        let backend = LocalTicketBackend::new(temp.path().join(".yoi/tickets"));
        create_ticket(&backend, "Missing Overlay", |input| {
            input.workflow_state = Some(TicketWorkflowState::Queued);
        });

        let model = build_workspace_panel(temp.path(), &empty_pods());

        let row = ticket_row_by_title(&model, "Missing Overlay");
        assert_eq!(row.status, "queued");
        assert!(row.ticket.as_ref().unwrap().orchestration_overlay.is_none());
    }

    #[test]
    fn workspace_panel_keeps_valid_ticket_actions_with_invalid_records() {
        let temp = TempDir::new().unwrap();
        write_ticket_config(temp.path());
        let backend = LocalTicketBackend::new(temp.path().join(".yoi/tickets"));
        let mut ready_input = NewTicket::new("Ready Still Queueable");
        ready_input.workflow_state = Some(TicketWorkflowState::Ready);
        let ready = backend.create(ready_input).unwrap();
        backend
            .create(NewTicket::new("Planning Still Clarifies"))
            .unwrap();

        for index in 0..6 {
            let ticket = backend
                .create(NewTicket::new(format!("Leaked Secret Invalid {index}")))
                .unwrap();
            fs::write(
                temp.path()
                    .join(".yoi/tickets")
                    .join(&ticket.id)
                    .join("item.md"),
                format!(
                    "---\ntitle: Leaked Secret Invalid {index}\nstate: super-secret-invalid-{index}\n---\nbody\n"
                ),
            )
            .unwrap();
        }

        let registry = PanelRegistryStore::from_root(temp.path().join("registry"));
        registry
            .claim_ticket(&ready.id, None, "ready-intake", "intake")
            .unwrap();
        let model = build_workspace_panel_with_registry(
            temp.path(),
            &live_pods(&["ready-intake"]),
            &registry.snapshot().unwrap(),
        );

        let ready_index = model
            .rows
            .iter()
            .position(|row| row.title == "Ready Still Queueable")
            .unwrap();
        let ready_row = &model.rows[ready_index];
        assert_eq!(ready_row.next_action, Some(NextUserAction::Queue));
        assert!(ready_row.is_ticket_action());
        assert_eq!(
            model.rows[ready_index + 1].key,
            PanelRowKey::TicketIntakePod {
                ticket_id: ready.id.clone(),
                pod_name: "ready-intake".to_string(),
            }
        );

        let planning = model
            .rows
            .iter()
            .find(|row| row.title == "Planning Still Clarifies")
            .unwrap();
        assert_eq!(planning.next_action, Some(NextUserAction::Clarify));
        assert!(planning.is_ticket_action());

        let invalid_rows = model
            .rows
            .iter()
            .filter(|row| row.kind == PanelRowKind::InvalidTicket)
            .collect::<Vec<_>>();
        assert_eq!(invalid_rows.len(), MAX_INVALID_TICKET_PLACEHOLDER_ROWS);
        for row in invalid_rows {
            assert_eq!(row.status, "invalid");
            assert!(row.ticket.is_none());
            assert_eq!(row.next_action, None);
            assert!(!row.is_ticket_action());
            assert!(row.disabled_reason.as_deref().unwrap().contains("disabled"));
        }

        let diagnostics = model.header.diagnostics.join("\n");
        assert!(diagnostics.contains("Ticket records partially loaded: 6 invalid record"));
        assert!(diagnostics.contains("showing first 5"));
        assert!(!diagnostics.contains("super-secret-invalid"));
        assert!(
            !model
                .rows
                .iter()
                .any(|row| row.title.contains("Leaked Secret Invalid"))
        );
    }

    #[test]
    fn workspace_panel_disables_current_ticket_when_detail_artifact_is_invalid() {
        let temp = TempDir::new().unwrap();
        write_ticket_config(temp.path());
        let backend = LocalTicketBackend::new(temp.path().join(".yoi/tickets"));
        let mut corrupt_input = NewTicket::new("Ready With Corrupt Relations");
        corrupt_input.workflow_state = Some(TicketWorkflowState::Ready);
        let corrupt = backend.create(corrupt_input).unwrap();
        let mut other_input = NewTicket::new("Other Ready Still Queueable");
        other_input.workflow_state = Some(TicketWorkflowState::Ready);
        let other = backend.create(other_input).unwrap();

        let artifacts = temp
            .path()
            .join(".yoi/tickets")
            .join(&corrupt.id)
            .join("artifacts");
        fs::create_dir_all(&artifacts).unwrap();
        fs::write(artifacts.join("relations.json"), "{").unwrap();

        let model = build_workspace_panel(temp.path(), &empty_pods());

        let corrupt_placeholders = model
            .rows
            .iter()
            .filter(|row| row.key == PanelRowKey::InvalidTicket(corrupt.id.clone()))
            .collect::<Vec<_>>();
        assert_eq!(corrupt_placeholders.len(), 1);
        let corrupt_placeholder = corrupt_placeholders[0];
        assert_eq!(corrupt_placeholder.kind, PanelRowKind::InvalidTicket);
        assert_eq!(corrupt_placeholder.next_action, None);
        assert!(corrupt_placeholder.ticket.is_none());
        assert!(!corrupt_placeholder.is_ticket_action());

        assert!(
            !model
                .rows
                .iter()
                .any(|row| row.key == PanelRowKey::Ticket(corrupt.id.clone()))
        );

        let other_row = model
            .rows
            .iter()
            .find(|row| row.key == PanelRowKey::Ticket(other.id.clone()))
            .unwrap();
        assert_eq!(other_row.next_action, Some(NextUserAction::Queue));
        assert!(other_row.is_ticket_action());

        let diagnostics = model.header.diagnostics.join("\n");
        assert!(diagnostics.contains("Ticket records partially loaded: 1 invalid record"));
    }

    #[test]
    fn workspace_panel_keeps_backend_config_unusable_as_whole_ticket_degradation() {
        let temp = TempDir::new().unwrap();
        let config_dir = temp.path().join(".yoi");
        fs::create_dir_all(&config_dir).unwrap();
        fs::write(
            config_dir.join("ticket.config.toml"),
            "[backend]\nprovider = \"unknown:provider\"\nroot = \".yoi/tickets\"\n",
        )
        .unwrap();

        let model = build_workspace_panel(temp.path(), &live_pods(&["idle"]));

        let diagnostics = model.header.diagnostics.join("\n");
        assert!(diagnostics.contains("Ticket config is unusable"));
        assert!(
            model
                .rows
                .iter()
                .all(|row| row.kind != PanelRowKind::InvalidTicket)
        );
        assert_eq!(
            model.composer.available_targets,
            vec![ComposerTarget::Companion]
        );
        assert!(
            model
                .rows
                .iter()
                .any(|row| row.key == PanelRowKey::Pod("idle".to_string()))
        );
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
    fn workspace_panel_allows_ready_ticket_when_relation_prerequisite_is_queued() {
        let temp = TempDir::new().unwrap();
        write_ticket_config(temp.path());
        let backend = LocalTicketBackend::new(temp.path().join(".yoi/tickets"));
        let mut ready_input = NewTicket::new("Ready After Queued Relation");
        ready_input.workflow_state = Some(TicketWorkflowState::Ready);
        let ready = backend.create(ready_input).unwrap();
        let mut dependency_input = NewTicket::new("Queued Relation Dependency");
        dependency_input.workflow_state = Some(TicketWorkflowState::Queued);
        let dependency = backend.create(dependency_input).unwrap();
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
            .find(|row| row.title == "Ready After Queued Relation")
            .unwrap();

        assert_eq!(row.kind, PanelRowKind::Ticket);
        assert_eq!(row.next_action, Some(NextUserAction::Queue));
        assert_eq!(row.priority, ActionPriority::ReadyForQueue);
        assert!(row.disabled_reason.is_none());
        assert!(row.ticket.as_ref().unwrap().blocked_reason.is_none());
        assert!(
            row.key_hint
                .as_deref()
                .unwrap()
                .contains("Queue allowed: prerequisites are already queued/in progress")
        );
        assert!(row.key_hint.as_deref().unwrap().contains(&dependency.id));
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
    fn workspace_panel_shows_ticket_associated_intake_pods_adjacent_to_ticket() {
        let temp = TempDir::new().unwrap();
        write_ticket_config(temp.path());
        let backend = LocalTicketBackend::new(temp.path().join(".yoi/tickets"));
        create_ticket(&backend, "Ticket With Intake", |input| {
            input.workflow_state = Some(TicketWorkflowState::Ready);
        });
        let ticket_id = backend
            .list(TicketFilter::all())
            .unwrap()
            .into_iter()
            .find(|ticket| ticket.title == "Ticket With Intake")
            .unwrap()
            .id;
        let preticket_pod = format!("pre-{ticket_id}-intake");
        let registry = PanelRegistryStore::from_root(temp.path().join("registry"));
        registry
            .claim_ticket(&ticket_id, None, "claimed-intake", "intake")
            .unwrap();
        registry
            .record_session(
                "shared-intake",
                "intake",
                RoleSessionOrigin::RoleLaunch,
                None,
                [RelatedTicketRef {
                    id: ticket_id.clone(),
                    slug: None,
                }],
            )
            .unwrap();
        registry
            .record_session(
                &preticket_pod,
                "intake",
                RoleSessionOrigin::PreTicketIntake,
                None,
                [],
            )
            .unwrap();

        let pods = live_pods(&["claimed-intake", "shared-intake", &preticket_pod]);
        let model =
            build_workspace_panel_with_registry(temp.path(), &pods, &registry.snapshot().unwrap());

        let ticket_index = model
            .rows
            .iter()
            .position(|row| row.key == PanelRowKey::Ticket(ticket_id.clone()))
            .unwrap();
        let ticket_row = &model.rows[ticket_index];
        let ticket = ticket_row.ticket.as_ref().unwrap();
        assert_eq!(
            ticket
                .intake_pods
                .iter()
                .map(|entry| entry.pod_name.as_str())
                .collect::<Vec<_>>(),
            vec!["claimed-intake", "shared-intake"]
        );
        assert_eq!(ticket.related_pods, vec!["claimed-intake", "shared-intake"]);
        assert_eq!(
            model.rows[ticket_index + 1].key,
            PanelRowKey::TicketIntakePod {
                ticket_id: ticket_id.clone(),
                pod_name: "claimed-intake".to_string(),
            }
        );
        assert_eq!(
            model.rows[ticket_index + 1].kind,
            PanelRowKind::TicketIntakePod
        );
        assert_eq!(model.rows[ticket_index + 1].status, "live");
        assert_eq!(
            model.rows[ticket_index + 1].next_action,
            Some(NextUserAction::OpenPod)
        );
        assert_eq!(
            model.rows[ticket_index + 2].key,
            PanelRowKey::TicketIntakePod {
                ticket_id: ticket_id.clone(),
                pod_name: "shared-intake".to_string(),
            }
        );
        assert!(model.rows.iter().all(|row| {
            row.kind != PanelRowKind::TicketIntakePod
                || row.key.pod_name() != Some(preticket_pod.as_str())
        }));
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
