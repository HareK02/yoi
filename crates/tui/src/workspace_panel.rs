use std::path::{Path, PathBuf};

use protocol::PodStatus;
use ticket::config::{TICKET_CONFIG_RELATIVE_PATH, TicketConfig};
use ticket::{
    ExtensibleTicketStatus, LocalTicketBackend, TicketBackend, TicketEvent, TicketEventKind,
    TicketFilter, TicketIdOrSlug, TicketReviewResult, TicketStatus, TicketSummary,
};

use crate::pod_list::{PodList, PodListEntry, StoredMetadataState};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkspacePanelViewModel {
    pub(crate) header: WorkspacePanelHeader,
    pub(crate) rows: Vec<PanelRow>,
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
                diagnostics: Vec::new(),
            },
            rows: Vec::new(),
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
    pub(crate) diagnostics: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum PanelRowKey {
    Ticket(String),
    Pod(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PanelRowKind {
    Intake,
    Ticket,
    Review,
    Blocked,
    ActiveWork,
    Pod,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ActionPriority {
    UserReply,
    ReadyForGo,
    Decision,
    Blocked,
    ActiveWork,
    Background,
}

impl ActionPriority {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::UserReply => "user action",
            Self::ReadyForGo => "ready",
            Self::Decision => "decision",
            Self::Blocked => "blocked",
            Self::ActiveWork => "active",
            Self::Background => "background",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NextUserAction {
    Clarify,
    ApproveIntake,
    Go,
    Review,
    Close,
    Defer,
    Edit,
    Wait,
    OpenPod,
    SendToPod,
}

impl NextUserAction {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Clarify => "Clarify",
            Self::ApproveIntake => "Approve",
            Self::Go => "Go",
            Self::Review => "Review",
            Self::Close => "Close",
            Self::Defer => "Defer",
            Self::Edit => "Edit",
            Self::Wait => "Wait",
            Self::OpenPod => "Open",
            Self::SendToPod => "Send",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TicketPanelPhase {
    Intake,
    RequirementsSync,
    Preflight,
    Spike,
    Implementing,
    Reviewing,
    CloseReady,
    Blocked,
    Open,
    Pending,
}

impl TicketPanelPhase {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Intake => "intake",
            Self::RequirementsSync => "requirements",
            Self::Preflight => "preflight",
            Self::Spike => "spike",
            Self::Implementing => "implementing",
            Self::Reviewing => "review",
            Self::CloseReady => "close-ready",
            Self::Blocked => "blocked",
            Self::Open => "open",
            Self::Pending => "pending",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TicketPanelEntry {
    pub(crate) id: String,
    pub(crate) slug: String,
    pub(crate) title: String,
    pub(crate) status: String,
    pub(crate) kind: String,
    pub(crate) priority: String,
    pub(crate) labels: Vec<String>,
    pub(crate) phase: TicketPanelPhase,
    pub(crate) next_action: Option<NextUserAction>,
    pub(crate) updated_at: Option<String>,
    pub(crate) latest_event_kind: Option<String>,
    pub(crate) latest_event_excerpt: Option<String>,
    pub(crate) blocked_reason: Option<String>,
    pub(crate) related_pods: Vec<String>,
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
            && (self.priority != ActionPriority::Background || self.next_action.is_some())
    }
}

pub(crate) fn build_workspace_panel(
    workspace_root: &Path,
    pods: &PodList,
) -> WorkspacePanelViewModel {
    let mut model = WorkspacePanelViewModel::empty(workspace_root);
    let ticket_config_path = workspace_root.join(TICKET_CONFIG_RELATIVE_PATH);
    if ticket_config_path.is_file() {
        if let Ok(config) = TicketConfig::load_workspace(workspace_root) {
            model.header.ticket_root = config.backend_root().to_path_buf();
            let backend = LocalTicketBackend::new(config.backend_root().to_path_buf());
            if let Ok(rows) = build_ticket_rows(&backend, pods) {
                model.rows.extend(rows);
            }
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

fn build_ticket_rows(
    backend: &LocalTicketBackend,
    pods: &PodList,
) -> ticket::Result<Vec<PanelRow>> {
    let mut rows = Vec::new();
    for summary in backend.list(TicketFilter::all())? {
        if summary.status.as_local() == Some(TicketStatus::Closed) {
            continue;
        }
        let ticket = backend.show(TicketIdOrSlug::Query(summary.slug.clone()))?;
        rows.push(ticket_row(summary, &ticket.events, pods));
    }
    Ok(rows)
}

fn ticket_row(summary: TicketSummary, events: &[TicketEvent], pods: &PodList) -> PanelRow {
    let related_pods = related_pods_for_ticket(&summary, pods);
    let derived = derive_ticket_state(&summary, events);
    let latest_event = events.last();
    let entry = TicketPanelEntry {
        id: summary.id.clone(),
        slug: summary.slug.clone(),
        title: summary.title.clone(),
        status: summary.status.as_str().to_string(),
        kind: summary.kind.clone(),
        priority: summary.priority.clone(),
        labels: summary.labels.clone(),
        phase: derived.phase,
        next_action: derived.action,
        updated_at: summary.updated_at.clone(),
        latest_event_kind: latest_event.map(|event| event.kind.as_str().to_string()),
        latest_event_excerpt: latest_event.and_then(|event| excerpt(event.body.as_str(), 72)),
        blocked_reason: derived.blocked_reason.clone(),
        related_pods: related_pods.clone(),
    };
    let subtitle = ticket_subtitle(&entry);
    PanelRow {
        key: PanelRowKey::Ticket(summary.id),
        kind: derived.kind,
        title: summary.title,
        subtitle,
        status: derived.status,
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
    phase: TicketPanelPhase,
    status: String,
    priority: ActionPriority,
    action: Option<NextUserAction>,
    disabled_reason: Option<String>,
    key_hint: Option<String>,
    blocked_reason: Option<String>,
}

fn derive_ticket_state(summary: &TicketSummary, events: &[TicketEvent]) -> DerivedTicketState {
    let action_required = summary.action_required.as_deref().map(str::trim);
    let action_required_lc = action_required.map(lowercase);
    let intake = is_intake_ticket(summary);
    let spike = is_spike_ticket(summary);

    if let Some(reason) = action_required_lc.as_deref() {
        if reason.contains("block") || reason.contains("blocked") {
            return DerivedTicketState {
                kind: PanelRowKind::Blocked,
                phase: TicketPanelPhase::Blocked,
                status: "blocked".to_string(),
                priority: ActionPriority::Blocked,
                action: Some(NextUserAction::Edit),
                disabled_reason: Some(
                    "Requires an explicit human/project decision before work continues."
                        .to_string(),
                ),
                key_hint: Some("Edit/decide in Ticket; no automatic unblock".to_string()),
                blocked_reason: action_required.map(ToOwned::to_owned),
            };
        }
        return DerivedTicketState {
            kind: if intake {
                PanelRowKind::Intake
            } else {
                PanelRowKind::Ticket
            },
            phase: if intake {
                TicketPanelPhase::Intake
            } else {
                TicketPanelPhase::RequirementsSync
            },
            status: action_required.unwrap_or("action required").to_string(),
            priority: ActionPriority::UserReply,
            action: Some(if intake {
                NextUserAction::ApproveIntake
            } else {
                NextUserAction::Clarify
            }),
            disabled_reason: None,
            key_hint: Some(
                "Human response is required; dispatch must re-check Ticket state".to_string(),
            ),
            blocked_reason: None,
        };
    }

    let latest_impl = latest_event_index(events, TicketEventKind::ImplementationReport);
    let latest_review = latest_event_index(events, TicketEventKind::Review);
    let latest_plan = latest_event_index(events, TicketEventKind::Plan);
    let latest_review_result = latest_review.and_then(|index| events[index].status.as_deref());

    if latest_review_result == Some(TicketReviewResult::Approve.as_str())
        && latest_review > latest_impl
    {
        return DerivedTicketState {
            kind: PanelRowKind::Review,
            phase: TicketPanelPhase::CloseReady,
            status: "review approved".to_string(),
            priority: ActionPriority::Decision,
            action: Some(NextUserAction::Close),
            disabled_reason: None,
            key_hint: Some("Close affordance only; closing must write a resolution".to_string()),
            blocked_reason: None,
        };
    }

    if latest_impl.is_some() && latest_impl > latest_review {
        return DerivedTicketState {
            kind: PanelRowKind::Review,
            phase: TicketPanelPhase::Reviewing,
            status: "implementation reported".to_string(),
            priority: ActionPriority::Decision,
            action: Some(NextUserAction::Review),
            disabled_reason: None,
            key_hint: Some("Review affordance only; inspect evidence before approving".to_string()),
            blocked_reason: None,
        };
    }

    if latest_review_result == Some(TicketReviewResult::RequestChanges.as_str()) {
        return DerivedTicketState {
            kind: PanelRowKind::ActiveWork,
            phase: TicketPanelPhase::Implementing,
            status: "changes requested".to_string(),
            priority: ActionPriority::ActiveWork,
            action: Some(NextUserAction::Wait),
            disabled_reason: Some("Waiting for implementation changes after review.".to_string()),
            key_hint: None,
            blocked_reason: None,
        };
    }

    if summary.status.as_local() == Some(TicketStatus::Pending) {
        return DerivedTicketState {
            kind: PanelRowKind::Blocked,
            phase: TicketPanelPhase::Pending,
            status: "pending/deferred".to_string(),
            priority: ActionPriority::Blocked,
            action: Some(NextUserAction::Defer),
            disabled_reason: Some(
                "Pending Ticket is shown for visibility; no automation is implied.".to_string(),
            ),
            key_hint: None,
            blocked_reason: None,
        };
    }

    if intake {
        return DerivedTicketState {
            kind: PanelRowKind::Intake,
            phase: TicketPanelPhase::Intake,
            status: "intake draft".to_string(),
            priority: ActionPriority::UserReply,
            action: Some(NextUserAction::ApproveIntake),
            disabled_reason: None,
            key_hint: Some("Approve/edit intake before routing".to_string()),
            blocked_reason: None,
        };
    }

    if looks_ready_for_go(summary) {
        return DerivedTicketState {
            kind: PanelRowKind::Ticket,
            phase: if summary.needs_preflight.unwrap_or(false) {
                TicketPanelPhase::Preflight
            } else {
                TicketPanelPhase::Open
            },
            status: "ready for Go".to_string(),
            priority: ActionPriority::ReadyForGo,
            action: Some(NextUserAction::Go),
            disabled_reason: None,
            key_hint: Some(
                "Go is an authorization affordance; routing/preflight gates still apply"
                    .to_string(),
            ),
            blocked_reason: None,
        };
    }

    if spike && latest_plan.is_some() {
        return DerivedTicketState {
            kind: PanelRowKind::ActiveWork,
            phase: TicketPanelPhase::Spike,
            status: "spike running".to_string(),
            priority: ActionPriority::ActiveWork,
            action: Some(NextUserAction::Wait),
            disabled_reason: Some("Spike has a plan but no implementation report yet.".to_string()),
            key_hint: None,
            blocked_reason: None,
        };
    }

    if spike {
        return DerivedTicketState {
            kind: PanelRowKind::Ticket,
            phase: TicketPanelPhase::Spike,
            status: "spike needed".to_string(),
            priority: ActionPriority::Background,
            action: None,
            disabled_reason: Some(
                "Spike candidate is shown as background until explicitly readied or planned."
                    .to_string(),
            ),
            key_hint: None,
            blocked_reason: None,
        };
    }

    if latest_plan.is_some() {
        return DerivedTicketState {
            kind: PanelRowKind::ActiveWork,
            phase: TicketPanelPhase::Implementing,
            status: "planned/active".to_string(),
            priority: ActionPriority::ActiveWork,
            action: Some(NextUserAction::Wait),
            disabled_reason: Some(
                "Ticket has a plan but no implementation report yet.".to_string(),
            ),
            key_hint: None,
            blocked_reason: None,
        };
    }

    DerivedTicketState {
        kind: PanelRowKind::Ticket,
        phase: TicketPanelPhase::Open,
        status: "open backlog".to_string(),
        priority: ActionPriority::Background,
        action: None,
        disabled_reason: Some(
            "Open Ticket is not marked ready; keep it out of the action section for now."
                .to_string(),
        ),
        key_hint: None,
        blocked_reason: None,
    }
}

fn looks_ready_for_go(summary: &TicketSummary) -> bool {
    summary
        .readiness
        .as_deref()
        .map(lowercase)
        .is_some_and(|value| value.contains("ready"))
        || summary.needs_preflight.unwrap_or(false)
        || summary
            .labels
            .iter()
            .any(|label| lowercase(label).contains("ready"))
}

fn is_intake_ticket(summary: &TicketSummary) -> bool {
    summary.kind == "intake"
        || summary.labels.iter().any(|label| label == "intake")
        || lowercase(&summary.slug).contains("intake")
        || lowercase(&summary.title).contains("intake")
}

fn is_spike_ticket(summary: &TicketSummary) -> bool {
    lowercase(&summary.kind).contains("spike")
        || summary
            .labels
            .iter()
            .any(|label| lowercase(label).contains("spike"))
        || lowercase(&summary.slug).contains("spike")
        || lowercase(&summary.title).contains("spike")
}

fn latest_event_index(events: &[TicketEvent], kind: TicketEventKind) -> Option<usize> {
    events.iter().rposition(|event| event.kind == kind)
}

fn related_pods_for_ticket(summary: &TicketSummary, pods: &PodList) -> Vec<String> {
    let slug = lowercase(&summary.slug);
    let id = lowercase(&summary.id);
    pods.entries
        .iter()
        .filter_map(|pod| {
            let name = lowercase(&pod.name);
            if (!slug.is_empty() && name.contains(&slug)) || (!id.is_empty() && name.contains(&id))
            {
                Some(pod.name.clone())
            } else {
                None
            }
        })
        .take(5)
        .collect()
}

fn ticket_subtitle(entry: &TicketPanelEntry) -> Option<String> {
    let mut parts = vec![format!(
        "{} · {} · {}",
        entry.slug,
        entry.phase.label(),
        entry.priority
    )];
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
    let next_action = if entry.actions.can_send_now {
        Some(NextUserAction::SendToPod)
    } else if entry.actions.can_open {
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
        key_hint: Some("Pod rows preserve existing open/direct-send behavior".to_string()),
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
fn _status_label(status: &ExtensibleTicketStatus) -> &str {
    status.as_str()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pod_list::{LivePodInfo, PodEntrySummary};
    use std::fs;
    use std::path::{Path, PathBuf};
    use tempfile::TempDir;
    use ticket::{MarkdownText, NewTicket, NewTicketEvent, TicketReview};

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
        slug: &str,
        configure: impl FnOnce(&mut NewTicket),
    ) {
        let mut input = NewTicket::new(title);
        input.slug = Some(slug.to_string());
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
        create_ticket(
            &backend,
            "Hidden Without Config",
            "hidden-without-config",
            |input| {
                input.action_required = Some("answer me".to_string());
            },
        );

        let model = build_workspace_panel(temp.path(), &live_pods(&["idle"]));

        assert!(model.header.diagnostics.is_empty());
        assert_eq!(model.rows.len(), 1);
        assert_eq!(model.rows[0].key, PanelRowKey::Pod("idle".to_string()));
        assert!(model.rows[0].ticket.is_none());
    }

    #[test]
    fn workspace_panel_prioritizes_human_actions_before_background_pods() {
        let temp = TempDir::new().unwrap();
        write_ticket_config(temp.path());
        let backend = LocalTicketBackend::new(temp.path().join(".yoi/tickets"));
        create_ticket(&backend, "Ready Ticket", "ready-ticket", |input| {
            input.readiness = Some("implementation-ready".to_string());
        });
        create_ticket(&backend, "Needs User", "needs-user", |input| {
            input.action_required = Some("answer clarification".to_string());
            input.labels = vec!["intake".to_string()];
        });

        let model = build_workspace_panel(temp.path(), &empty_pods());
        let rows = model
            .rows
            .iter()
            .map(|row| (row.title.as_str(), row.priority, row.next_action))
            .collect::<Vec<_>>();

        assert_eq!(rows[0].0, "Needs User");
        assert_eq!(rows[0].1, ActionPriority::UserReply);
        assert_eq!(rows[0].2, Some(NextUserAction::ApproveIntake));
        assert_eq!(rows[1].0, "Ready Ticket");
        assert_eq!(rows[1].1, ActionPriority::ReadyForGo);
        assert_eq!(rows[1].2, Some(NextUserAction::Go));
    }

    #[test]
    fn workspace_panel_derives_spike_phase_without_marking_unready_spikes_ready_for_go() {
        let temp = TempDir::new().unwrap();
        write_ticket_config(temp.path());
        let backend = LocalTicketBackend::new(temp.path().join(".yoi/tickets"));
        create_ticket(
            &backend,
            "Investigate Spike",
            "investigate-spike",
            |input| {
                input.labels = vec!["spike".to_string()];
            },
        );
        create_ticket(&backend, "Running Spike", "running-spike", |input| {
            input.kind = "spike".to_string();
        });
        backend
            .add_event(
                TicketIdOrSlug::Query("running-spike".to_string()),
                NewTicketEvent::new(TicketEventKind::Plan, "Run the spike."),
            )
            .unwrap();

        let model = build_workspace_panel(temp.path(), &empty_pods());
        let needed = model
            .rows
            .iter()
            .find(|row| row.title == "Investigate Spike")
            .unwrap();
        let running = model
            .rows
            .iter()
            .find(|row| row.title == "Running Spike")
            .unwrap();

        assert_eq!(
            needed.ticket.as_ref().unwrap().phase,
            TicketPanelPhase::Spike
        );
        assert_eq!(needed.priority, ActionPriority::Background);
        assert_eq!(needed.next_action, None);
        assert!(!needed.is_ticket_action());
        assert_eq!(
            running.ticket.as_ref().unwrap().phase,
            TicketPanelPhase::Spike
        );
        assert_eq!(running.priority, ActionPriority::ActiveWork);
        assert_eq!(running.next_action, Some(NextUserAction::Wait));
    }

    #[test]
    fn workspace_panel_keeps_ordinary_open_backlog_out_of_action_section() {
        let temp = TempDir::new().unwrap();
        write_ticket_config(temp.path());
        let backend = LocalTicketBackend::new(temp.path().join(".yoi/tickets"));
        create_ticket(&backend, "Plain Backlog", "plain-backlog", |_| {});

        let model = build_workspace_panel(temp.path(), &empty_pods());
        let row = model
            .rows
            .iter()
            .find(|row| row.title == "Plain Backlog")
            .unwrap();

        assert_eq!(row.priority, ActionPriority::Background);
        assert_eq!(row.next_action, None);
        assert!(!row.is_ticket_action());
    }

    #[test]
    fn workspace_panel_derives_review_and_close_actions_from_thread_roles() {
        let temp = TempDir::new().unwrap();
        write_ticket_config(temp.path());
        let backend = LocalTicketBackend::new(temp.path().join(".yoi/tickets"));
        create_ticket(&backend, "Needs Review", "needs-review", |_| {});
        create_ticket(&backend, "Close Ready", "close-ready", |_| {});
        backend
            .add_event(
                TicketIdOrSlug::Query("needs-review".to_string()),
                NewTicketEvent::new(TicketEventKind::ImplementationReport, "Implemented."),
            )
            .unwrap();
        backend
            .add_event(
                TicketIdOrSlug::Query("close-ready".to_string()),
                NewTicketEvent::new(TicketEventKind::ImplementationReport, "Implemented."),
            )
            .unwrap();
        backend
            .review(
                TicketIdOrSlug::Query("close-ready".to_string()),
                TicketReview::approve(MarkdownText::new("Approved.")),
            )
            .unwrap();

        let model = build_workspace_panel(temp.path(), &empty_pods());
        let review = model
            .rows
            .iter()
            .find(|row| row.title == "Needs Review")
            .unwrap();
        let close = model
            .rows
            .iter()
            .find(|row| row.title == "Close Ready")
            .unwrap();

        assert_eq!(review.priority, ActionPriority::Decision);
        assert_eq!(review.next_action, Some(NextUserAction::Review));
        assert_eq!(close.priority, ActionPriority::Decision);
        assert_eq!(close.next_action, Some(NextUserAction::Close));
    }
}
