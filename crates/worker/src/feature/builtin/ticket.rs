//! Built-in Ticket feature adapter.
//!
//! The ticket crate owns Ticket domain logic and Tool implementations. This
//! module only resolves the local backend root, declares the built-in feature,
//! and contributes those tools through the normal feature registry path.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use ticket::{
    LocalTicketBackend, MarkdownText, NewOrchestrationPlanRecord, NewTicket, NewTicketEvent,
    NewTicketRelation, OrchestrationPlanKind, OrchestrationPlanRecord, Result as TicketResult,
    Ticket, TicketBackend, TicketBackendOperation, TicketBackendOperationResult,
    TicketDoctorReport, TicketError, TicketIdOrSlug, TicketIntakeSummary, TicketListQuery,
    TicketRef, TicketRelation, TicketRelationKind, TicketRelationView, TicketStateChange,
    TicketSummary,
    config::{DEFAULT_TICKET_BACKEND_RELATIVE_PATH, TicketConfig},
    tool::{TICKET_TOOL_NAMES, TicketToolBackend, ticket_tool_description, ticket_tools},
};

use super::merge_request;
use crate::feature::{
    FeatureDescriptor, FeatureDiagnostic, FeatureInstallContext, FeatureInstallError,
    FeatureInstructionContribution, FeatureInstructionDeclaration, FeatureInstructionId,
    FeatureModule, ToolContribution, ToolDeclaration,
};
use crate::worker::{WorkspaceClient, WorkspaceRequest, WorkspaceRequestMethod};

const FEATURE_ID: &str = "ticket";
const FEATURE_NAME: &str = "Ticket tools";
const FEATURE_DESCRIPTION: &str = "Typed local Ticket work-item operations over a bounded backend root. \
The tools operate through the ticket crate backend and do not grant generic filesystem write scope.";
const TICKET_WORKFLOW_INSTRUCTION_ID: &str = "ticket.workflow";
const TICKET_WORKFLOW_PROMPT_REF: &str = "$yoi/common/tickets";

fn ticket_workflow_instruction() -> FeatureInstructionDeclaration {
    FeatureInstructionDeclaration::new(
        FeatureInstructionId::builtin(TICKET_WORKFLOW_INSTRUCTION_ID),
        TICKET_WORKFLOW_PROMPT_REF,
        "Typed Ticket workflow guidance",
    )
    .expect("static Ticket workflow instruction declaration is valid")
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TicketFeatureAccess {
    pub authoring: bool,
    pub thread: bool,
    pub intake: bool,
    pub orchestration_control: bool,
}

impl TicketFeatureAccess {
    pub const fn read_only() -> Self {
        Self {
            authoring: false,
            thread: false,
            intake: false,
            orchestration_control: false,
        }
    }

    pub const fn workspace_authoring() -> Self {
        Self {
            authoring: true,
            thread: true,
            intake: false,
            orchestration_control: false,
        }
    }

    pub const fn intake() -> Self {
        Self {
            authoring: true,
            thread: true,
            intake: true,
            orchestration_control: false,
        }
    }

    pub const fn orchestration_control() -> Self {
        Self {
            authoring: false,
            thread: true,
            intake: false,
            orchestration_control: true,
        }
    }

    pub const fn work_report() -> Self {
        Self {
            authoring: false,
            thread: true,
            intake: false,
            orchestration_control: false,
        }
    }

    pub const fn review() -> Self {
        Self {
            authoring: false,
            thread: false,
            intake: false,
            orchestration_control: false,
        }
    }

    pub fn tool_names(self) -> Vec<&'static str> {
        TICKET_TOOL_NAMES
            .iter()
            .copied()
            .filter(|name| self.allows_tool(name))
            .collect()
    }

    fn allows_tool(self, name: &str) -> bool {
        READ_ONLY_TOOL_NAMES.contains(&name)
            || (self.authoring && AUTHORING_TOOL_NAMES.contains(&name))
            || (self.thread && THREAD_TOOL_NAMES.contains(&name))
            || (self.intake && INTAKE_TOOL_NAMES.contains(&name))
            || (self.orchestration_control
                && ORCHESTRATION_CONTROL_ADDITIONAL_TOOL_NAMES.contains(&name))
    }
}

const READ_ONLY_TOOL_NAMES: &[&str] = &[
    "TicketList",
    "TicketShow",
    "TicketDependencyCheck",
    "TicketDoctor",
    "TicketRelationQuery",
    "TicketOrchestrationPlanQuery",
];

const AUTHORING_TOOL_NAMES: &[&str] = &[
    "TicketCreate",
    "TicketEditItem",
    "TicketQueue",
    "TicketClose",
    "TicketRelationRecord",
];

const THREAD_TOOL_NAMES: &[&str] = &["TicketComment"];

const INTAKE_TOOL_NAMES: &[&str] = &["TicketIntakeReady"];

#[cfg(test)]
const WORKSPACE_AUTHORING_TOOL_NAMES: &[&str] = &[
    "TicketCreate",
    "TicketEditItem",
    "TicketList",
    "TicketShow",
    "TicketComment",
    "TicketQueue",
    "TicketClose",
    "TicketDependencyCheck",
    "TicketDoctor",
    "TicketRelationRecord",
    "TicketRelationQuery",
    "TicketOrchestrationPlanQuery",
];

#[cfg(test)]
const ORCHESTRATION_CONTROL_TOOL_NAMES: &[&str] = &[
    "TicketList",
    "TicketShow",
    "TicketComment",
    "TicketWorkflowState",
    "TicketClose",
    "TicketDependencyCheck",
    "TicketDoctor",
    "TicketRelationRecord",
    "TicketRelationQuery",
    "TicketOrchestrationPlanRecord",
    "TicketOrchestrationPlanQuery",
];

const ORCHESTRATION_CONTROL_ADDITIONAL_TOOL_NAMES: &[&str] = &[
    "TicketWorkflowState",
    "TicketClose",
    "TicketRelationRecord",
    "TicketOrchestrationPlanRecord",
];

#[derive(Clone, Debug)]
pub enum TicketFeatureBackend {
    Local { root: PathBuf },
    WorkspaceClient(Arc<dyn WorkspaceClient>),
}

impl From<PathBuf> for TicketFeatureBackend {
    fn from(root: PathBuf) -> Self {
        Self::Local { root }
    }
}

impl From<&Path> for TicketFeatureBackend {
    fn from(root: &Path) -> Self {
        Self::Local {
            root: root.to_path_buf(),
        }
    }
}

impl From<&PathBuf> for TicketFeatureBackend {
    fn from(root: &PathBuf) -> Self {
        Self::Local { root: root.clone() }
    }
}

#[derive(Clone, Debug)]
pub struct TicketFeature {
    backend: TicketFeatureBackend,
    record_language: Option<String>,
    config_error: Option<String>,
    access: TicketFeatureAccess,
}

impl TicketFeature {
    pub fn new(backend_root: impl Into<PathBuf>) -> Self {
        Self::new_with_access(backend_root, TicketFeatureAccess::workspace_authoring())
    }

    pub fn new_with_access(backend_root: impl Into<PathBuf>, access: TicketFeatureAccess) -> Self {
        Self::with_backend(
            TicketFeatureBackend::Local {
                root: backend_root.into(),
            },
            access,
        )
    }

    pub fn with_backend(backend: TicketFeatureBackend, access: TicketFeatureAccess) -> Self {
        Self {
            backend,
            record_language: None,
            config_error: None,
            access,
        }
    }

    pub fn for_workspace(workspace: impl AsRef<Path>) -> Self {
        Self::for_workspace_with_access(workspace, TicketFeatureAccess::workspace_authoring())
    }

    pub fn for_workspace_with_access(
        workspace: impl AsRef<Path>,
        access: TicketFeatureAccess,
    ) -> Self {
        let workspace = workspace.as_ref();
        match TicketConfig::load_workspace(workspace) {
            Ok(config) => {
                let backend_root = config.backend_root().to_path_buf();
                let record_language = config.ticket_record_language().map(str::to_string);
                let mut feature = Self::new_with_access(backend_root, access);
                feature.record_language = record_language;
                feature
            }
            Err(error) => Self {
                backend: TicketFeatureBackend::Local {
                    root: workspace.join(DEFAULT_TICKET_BACKEND_RELATIVE_PATH),
                },
                record_language: None,
                config_error: Some(error.to_string()),
                access,
            },
        }
    }

    pub fn backend_root(&self) -> Option<&Path> {
        match &self.backend {
            TicketFeatureBackend::Local { root } => Some(root),
            TicketFeatureBackend::WorkspaceClient(_) => None,
        }
    }

    pub fn access(&self) -> TicketFeatureAccess {
        self.access
    }

    fn enabled_tool_names(&self) -> Vec<&'static str> {
        self.access.tool_names()
    }

    fn usable_backend_root(&self) -> Result<PathBuf, String> {
        let Some(root) = self.backend_root() else {
            return Err("ticket backend is not local filesystem backed".to_string());
        };
        let root = root
            .canonicalize()
            .map_err(|error| format!("ticket backend root is not usable: {error}"))?;
        if !root.is_dir() {
            return Err("ticket backend root is not a directory".to_string());
        }
        Ok(root)
    }
    fn tool_backend(&self, context: &mut FeatureInstallContext<'_>) -> Option<TicketToolBackend> {
        match &self.backend {
            TicketFeatureBackend::Local { root: _ } => {
                let usable_root = match self.usable_backend_root() {
                    Ok(root) => root,
                    Err(reason) => {
                        context
                            .diagnostics()
                            .push(FeatureDiagnostic::warning(format!(
                                "Ticket tools not registered: {reason}; root={} ",
                                self.backend_root()
                                    .map(|root| root.display().to_string())
                                    .unwrap_or_else(|| "<non-local>".to_string())
                            )));
                        return None;
                    }
                };
                Some(
                    LocalTicketBackend::new(usable_root)
                        .with_record_language(self.record_language.as_deref())
                        .into(),
                )
            }
            TicketFeatureBackend::WorkspaceClient(client) => Some(
                TicketToolBackend::new(WorkspaceHttpTicketBackend::new(client.clone()))
                    .with_record_language(self.record_language.as_deref()),
            ),
        }
    }
}

impl FeatureModule for TicketFeature {
    fn descriptor(&self) -> FeatureDescriptor {
        let mut descriptor = FeatureDescriptor::builtin(FEATURE_ID, FEATURE_NAME)
            .with_description(FEATURE_DESCRIPTION)
            .with_instruction(ticket_workflow_instruction());
        let enabled_tool_names = self.enabled_tool_names();
        for name in enabled_tool_names {
            descriptor = descriptor.with_tool(ToolDeclaration::new(
                name,
                ticket_tool_description(name, self.record_language.as_deref()),
            ));
        }
        if let TicketFeatureBackend::WorkspaceClient(client) = &self.backend {
            let names: Vec<&str> = if client.reviewer_attempt_context().is_some() {
                vec![
                    "MergeRequestShow",
                    merge_request::MERGE_REQUEST_REVIEW_TOOL_NAME,
                ]
            } else {
                merge_request::MERGE_REQUEST_COMMON_TOOL_NAMES.to_vec()
            };
            for name in names {
                descriptor = descriptor.with_tool(ToolDeclaration::new(
                    name,
                    merge_request::description(name).unwrap_or("Merge Request operation."),
                ));
            }
        }
        descriptor
    }

    fn install(&self, context: &mut FeatureInstallContext<'_>) -> Result<(), FeatureInstallError> {
        if let Some(error) = &self.config_error {
            context
                .diagnostics()
                .push(FeatureDiagnostic::warning(format!(
                    "Ticket tools not registered: {error}"
                )));
            return Ok(());
        }
        let Some(backend) = self.tool_backend(context) else {
            return Ok(());
        };
        context
            .instructions()
            .register(FeatureInstructionContribution::new(
                ticket_workflow_instruction(),
            ))?;
        let allowed_tool_names = self.enabled_tool_names();
        let mut tools = context.tools();
        for definition in ticket_tools(backend) {
            let (meta, _) = definition();
            let name = meta.name.clone();
            if !allowed_tool_names
                .iter()
                .any(|allowed| *allowed == name.as_str())
            {
                continue;
            }
            tools.register(ToolContribution::new(name, definition))?;
        }
        if let TicketFeatureBackend::WorkspaceClient(client) = &self.backend {
            let definitions = if client.reviewer_attempt_context().is_some() {
                merge_request::reviewer_tools(client.clone())
            } else {
                merge_request::common_tools(client.clone())
            };
            for definition in definitions {
                let (meta, _) = definition();
                tools.register(ToolContribution::new(meta.name.clone(), definition))?;
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
struct WorkspaceHttpTicketBackend {
    client: Arc<dyn WorkspaceClient>,
}

impl WorkspaceHttpTicketBackend {
    fn new(client: Arc<dyn WorkspaceClient>) -> Self {
        Self { client }
    }

    fn invoke(
        &self,
        operation: TicketBackendOperation,
    ) -> TicketResult<TicketBackendOperationResult> {
        let client = self.client.clone();
        let workspace_id = self.client.workspace_id().unwrap_or_default().to_string();
        if tokio::runtime::Handle::try_current().is_ok() {
            return std::thread::spawn(move || {
                Self::invoke_client(client, workspace_id, operation)
            })
            .join()
            .map_err(|_| {
                TicketError::Conflict("ticket REST request thread panicked".to_string())
            })?;
        }
        Self::invoke_client(client, workspace_id, operation)
    }

    fn ticket_path(id: &TicketIdOrSlug) -> String {
        let value = match id {
            TicketIdOrSlug::Id(value)
            | TicketIdOrSlug::Slug(value)
            | TicketIdOrSlug::Query(value) => value,
        };
        let mut encoded = String::with_capacity(value.len());
        for byte in value.bytes() {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
                encoded.push(byte as char);
            } else {
                use std::fmt::Write as _;
                let _ = write!(encoded, "%{byte:02X}");
            }
        }
        encoded
    }

    fn request<T: serde::de::DeserializeOwned>(
        client: Arc<dyn WorkspaceClient>,
        method: WorkspaceRequestMethod,
        endpoint: String,
        body: Option<serde_json::Value>,
    ) -> TicketResult<T> {
        let request = match body {
            Some(body) => WorkspaceRequest::json(method, endpoint, body.to_string()),
            None if method == WorkspaceRequestMethod::Get => WorkspaceRequest::get(endpoint),
            None => WorkspaceRequest {
                method,
                path: endpoint,
                body: None,
            },
        };
        let response = client.execute(request).map_err(|error| {
            TicketError::Conflict(format!("ticket REST request failed: {error}"))
        })?;
        if !response.is_success() {
            return Err(TicketError::Conflict(format!(
                "ticket REST API returned HTTP {}: {}",
                response.status, response.body
            )));
        }
        serde_json::from_str(&response.body)
            .map_err(|error| TicketError::Conflict(format!("decode ticket REST response: {error}")))
    }

    fn request_unit(
        client: Arc<dyn WorkspaceClient>,
        method: WorkspaceRequestMethod,
        endpoint: String,
        body: Option<serde_json::Value>,
    ) -> TicketResult<TicketBackendOperationResult> {
        let request = match body {
            Some(body) => WorkspaceRequest::json(method, endpoint, body.to_string()),
            None => WorkspaceRequest {
                method,
                path: endpoint,
                body: None,
            },
        };
        let response = client.execute(request).map_err(|error| {
            TicketError::Conflict(format!("ticket REST request failed: {error}"))
        })?;
        if !response.is_success() {
            return Err(TicketError::Conflict(format!(
                "ticket REST API returned HTTP {}: {}",
                response.status, response.body
            )));
        }
        Ok(TicketBackendOperationResult::Unit)
    }

    fn invoke_client(
        client: Arc<dyn WorkspaceClient>,
        workspace_id: String,
        operation: TicketBackendOperation,
    ) -> TicketResult<TicketBackendOperationResult> {
        let base = format!("/api/w/{workspace_id}/tickets");
        match operation {
            TicketBackendOperation::DefaultIntakeReadyStateChangeBody { from } => {
                let value = Self::request::<String>(
                    client,
                    WorkspaceRequestMethod::Post,
                    format!("{base}/default-intake-ready-body"),
                    Some(serde_json::json!({ "from": from })),
                )?;
                Ok(TicketBackendOperationResult::Text(value))
            }
            TicketBackendOperation::List { filter } => {
                let state = match filter.state {
                    ticket::TicketStateSelector::Active => "active".to_string(),
                    ticket::TicketStateSelector::All => "all".to_string(),
                    ticket::TicketStateSelector::States(states) => states
                        .into_iter()
                        .map(|state| state.as_str().to_string())
                        .collect::<Vec<_>>()
                        .join(","),
                };
                let tickets = Self::request(
                    client,
                    WorkspaceRequestMethod::Get,
                    format!("{base}/search?state={state}"),
                    None,
                )?;
                Ok(TicketBackendOperationResult::Tickets(tickets))
            }
            TicketBackendOperation::Show { id } => {
                let ticket = Self::request(
                    client,
                    WorkspaceRequestMethod::Get,
                    format!("{base}/{}/record", Self::ticket_path(&id)),
                    None,
                )?;
                Ok(TicketBackendOperationResult::Ticket(ticket))
            }
            TicketBackendOperation::Create { input } => {
                let ticket = Self::request(
                    client,
                    WorkspaceRequestMethod::Post,
                    base,
                    Some(serde_json::to_value(input).map_err(|error| {
                        TicketError::Conflict(format!("serialize Ticket create: {error}"))
                    })?),
                )?;
                Ok(TicketBackendOperationResult::TicketRef(ticket))
            }
            TicketBackendOperation::EditItem { id, edit } => {
                let ticket = Self::request(
                    client,
                    WorkspaceRequestMethod::Patch,
                    format!("{base}/{}/item", Self::ticket_path(&id)),
                    Some(serde_json::to_value(edit).map_err(|error| {
                        TicketError::Conflict(format!("serialize Ticket edit: {error}"))
                    })?),
                )?;
                Ok(TicketBackendOperationResult::Ticket(ticket))
            }
            TicketBackendOperation::DependencyCheck { id } => {
                let check = Self::request(
                    client,
                    WorkspaceRequestMethod::Get,
                    format!("{base}/{}/dependency-check", Self::ticket_path(&id)),
                    None,
                )?;
                Ok(TicketBackendOperationResult::DependencyCheck(check))
            }
            TicketBackendOperation::AddEvent { id, event } => Self::request_unit(
                client,
                WorkspaceRequestMethod::Post,
                format!("{base}/{}/thread-events", Self::ticket_path(&id)),
                Some(serde_json::to_value(event).map_err(|error| {
                    TicketError::Conflict(format!("serialize Ticket event: {error}"))
                })?),
            ),
            TicketBackendOperation::AddStateChanged { id, change } => Self::request_unit(
                client,
                WorkspaceRequestMethod::Post,
                format!("{base}/{}/state-changes", Self::ticket_path(&id)),
                Some(serde_json::to_value(change).map_err(|error| {
                    TicketError::Conflict(format!("serialize Ticket state change: {error}"))
                })?),
            ),
            TicketBackendOperation::AddIntakeSummary { id, summary } => Self::request_unit(
                client,
                WorkspaceRequestMethod::Post,
                format!("{base}/{}/intake-summaries", Self::ticket_path(&id)),
                Some(serde_json::to_value(summary).map_err(|error| {
                    TicketError::Conflict(format!("serialize Ticket intake summary: {error}"))
                })?),
            ),
            TicketBackendOperation::SetStateField { id, field, change } => Self::request_unit(
                client,
                WorkspaceRequestMethod::Post,
                format!(
                    "{base}/{}/state-fields/{}",
                    Self::ticket_path(&id),
                    Self::ticket_path(&TicketIdOrSlug::Query(field))
                ),
                Some(serde_json::to_value(change).map_err(|error| {
                    TicketError::Conflict(format!("serialize Ticket state field change: {error}"))
                })?),
            ),
            TicketBackendOperation::SetWorkflowState { id, change } => Self::request_unit(
                client,
                WorkspaceRequestMethod::Post,
                format!("{base}/{}/workflow-state", Self::ticket_path(&id)),
                Some(serde_json::to_value(change).map_err(|error| {
                    TicketError::Conflict(format!("serialize Ticket workflow change: {error}"))
                })?),
            ),
            TicketBackendOperation::MarkIntakeReady {
                id,
                summary,
                change,
            } => Self::request_unit(
                client,
                WorkspaceRequestMethod::Post,
                format!("{base}/{}/intake-ready", Self::ticket_path(&id)),
                Some(serde_json::json!({ "summary": summary, "change": change })),
            ),
            TicketBackendOperation::QueueReady { id, .. } => Self::request_unit(
                client,
                WorkspaceRequestMethod::Post,
                format!("{base}/{}/workflow/queue", Self::ticket_path(&id)),
                None,
            ),
            TicketBackendOperation::Close { id, resolution } => Self::request_unit(
                client,
                WorkspaceRequestMethod::Post,
                format!("{base}/{}/workflow/close", Self::ticket_path(&id)),
                Some(serde_json::to_value(resolution).map_err(|error| {
                    TicketError::Conflict(format!("serialize Ticket close: {error}"))
                })?),
            ),
            TicketBackendOperation::AddTicketRelation { id, relation } => {
                let relation = Self::request(
                    client,
                    WorkspaceRequestMethod::Post,
                    format!("{base}/{}/relations", Self::ticket_path(&id)),
                    Some(serde_json::to_value(relation).map_err(|error| {
                        TicketError::Conflict(format!("serialize Ticket relation: {error}"))
                    })?),
                )?;
                Ok(TicketBackendOperationResult::Relation(relation))
            }
            TicketBackendOperation::QueryTicketRelations { ticket, kind } => {
                let relations = Self::request(
                    client,
                    WorkspaceRequestMethod::Post,
                    format!("{base}/relations/search"),
                    Some(serde_json::json!({ "ticket": ticket, "kind": kind })),
                )?;
                Ok(TicketBackendOperationResult::Relations(relations))
            }
            TicketBackendOperation::RelationView { id } => {
                let view = Self::request(
                    client,
                    WorkspaceRequestMethod::Get,
                    format!("{base}/{}/relation-view", Self::ticket_path(&id)),
                    None,
                )?;
                Ok(TicketBackendOperationResult::RelationView(view))
            }
            TicketBackendOperation::AddOrchestrationPlanRecord { id, record } => {
                let record = Self::request(
                    client,
                    WorkspaceRequestMethod::Post,
                    format!("{base}/{}/orchestration-plans", Self::ticket_path(&id)),
                    Some(serde_json::to_value(record).map_err(|error| {
                        TicketError::Conflict(format!(
                            "serialize Ticket orchestration plan: {error}"
                        ))
                    })?),
                )?;
                Ok(TicketBackendOperationResult::OrchestrationPlanRecord(
                    record,
                ))
            }
            TicketBackendOperation::QueryOrchestrationPlanRecords { ticket, kind } => {
                let records = Self::request(
                    client,
                    WorkspaceRequestMethod::Post,
                    format!("{base}/orchestration-plans/search"),
                    Some(serde_json::json!({ "ticket": ticket, "kind": kind })),
                )?;
                Ok(TicketBackendOperationResult::OrchestrationPlanRecords(
                    records,
                ))
            }
            TicketBackendOperation::Doctor => {
                let report = Self::request(
                    client,
                    WorkspaceRequestMethod::Get,
                    format!("{base}/doctor"),
                    None,
                )?;
                Ok(TicketBackendOperationResult::DoctorReport(report))
            }
        }
    }
}

macro_rules! expect_ticket_result {
    ($expr:expr, $variant:path) => {
        match $expr? {
            $variant(value) => Ok(value),
            other => Err(TicketError::Conflict(format!(
                "unexpected ticket backend response: {other:?}"
            ))),
        }
    };
}

impl TicketBackend for WorkspaceHttpTicketBackend {
    fn default_intake_ready_state_change_body(&self, from: &str) -> String {
        match self.invoke(TicketBackendOperation::DefaultIntakeReadyStateChangeBody {
            from: from.to_string(),
        }) {
            Ok(TicketBackendOperationResult::Text(value)) => value,
            Ok(other) => format!("unexpected ticket backend response: {other:?}"),
            Err(error) => error.to_string(),
        }
    }

    fn list(&self, filter: TicketListQuery) -> TicketResult<Vec<TicketSummary>> {
        expect_ticket_result!(
            self.invoke(TicketBackendOperation::List { filter }),
            TicketBackendOperationResult::Tickets
        )
    }

    fn show(&self, id: TicketIdOrSlug) -> TicketResult<Ticket> {
        expect_ticket_result!(
            self.invoke(TicketBackendOperation::Show { id }),
            TicketBackendOperationResult::Ticket
        )
    }

    fn create(&self, input: NewTicket) -> TicketResult<TicketRef> {
        expect_ticket_result!(
            self.invoke(TicketBackendOperation::Create { input }),
            TicketBackendOperationResult::TicketRef
        )
    }

    fn edit_item(&self, id: TicketIdOrSlug, edit: ticket::TicketItemEdit) -> TicketResult<Ticket> {
        expect_ticket_result!(
            self.invoke(TicketBackendOperation::EditItem { id, edit }),
            TicketBackendOperationResult::Ticket
        )
    }

    fn dependency_check(&self, id: TicketIdOrSlug) -> TicketResult<ticket::TicketDependencyCheck> {
        expect_ticket_result!(
            self.invoke(TicketBackendOperation::DependencyCheck { id }),
            TicketBackendOperationResult::DependencyCheck
        )
    }

    fn add_event(&self, id: TicketIdOrSlug, event: NewTicketEvent) -> TicketResult<()> {
        match self.invoke(TicketBackendOperation::AddEvent { id, event })? {
            TicketBackendOperationResult::Unit => Ok(()),
            other => Err(TicketError::Conflict(format!(
                "unexpected ticket backend response: {other:?}"
            ))),
        }
    }

    fn add_state_changed(&self, id: TicketIdOrSlug, change: TicketStateChange) -> TicketResult<()> {
        match self.invoke(TicketBackendOperation::AddStateChanged { id, change })? {
            TicketBackendOperationResult::Unit => Ok(()),
            other => Err(TicketError::Conflict(format!(
                "unexpected ticket backend response: {other:?}"
            ))),
        }
    }

    fn add_intake_summary(
        &self,
        id: TicketIdOrSlug,
        summary: TicketIntakeSummary,
    ) -> TicketResult<()> {
        match self.invoke(TicketBackendOperation::AddIntakeSummary { id, summary })? {
            TicketBackendOperationResult::Unit => Ok(()),
            other => Err(TicketError::Conflict(format!(
                "unexpected ticket backend response: {other:?}"
            ))),
        }
    }

    fn set_state_field(
        &self,
        id: TicketIdOrSlug,
        field: &str,
        change: TicketStateChange,
    ) -> TicketResult<()> {
        match self.invoke(TicketBackendOperation::SetStateField {
            id,
            field: field.to_string(),
            change,
        })? {
            TicketBackendOperationResult::Unit => Ok(()),
            other => Err(TicketError::Conflict(format!(
                "unexpected ticket backend response: {other:?}"
            ))),
        }
    }

    fn set_workflow_state(
        &self,
        id: TicketIdOrSlug,
        change: TicketStateChange,
    ) -> TicketResult<()> {
        match self.invoke(TicketBackendOperation::SetWorkflowState { id, change })? {
            TicketBackendOperationResult::Unit => Ok(()),
            other => Err(TicketError::Conflict(format!(
                "unexpected ticket backend response: {other:?}"
            ))),
        }
    }

    fn mark_intake_ready(
        &self,
        id: TicketIdOrSlug,
        summary: TicketIntakeSummary,
        change: TicketStateChange,
    ) -> TicketResult<()> {
        match self.invoke(TicketBackendOperation::MarkIntakeReady {
            id,
            summary,
            change,
        })? {
            TicketBackendOperationResult::Unit => Ok(()),
            other => Err(TicketError::Conflict(format!(
                "unexpected ticket backend response: {other:?}"
            ))),
        }
    }

    fn queue_ready(&self, id: TicketIdOrSlug, queued_by: &str) -> TicketResult<()> {
        match self.invoke(TicketBackendOperation::QueueReady {
            id,
            queued_by: queued_by.to_string(),
        })? {
            TicketBackendOperationResult::Unit => Ok(()),
            other => Err(TicketError::Conflict(format!(
                "unexpected ticket backend response: {other:?}"
            ))),
        }
    }

    fn close(&self, id: TicketIdOrSlug, resolution: MarkdownText) -> TicketResult<()> {
        match self.invoke(TicketBackendOperation::Close { id, resolution })? {
            TicketBackendOperationResult::Unit => Ok(()),
            other => Err(TicketError::Conflict(format!(
                "unexpected ticket backend response: {other:?}"
            ))),
        }
    }

    fn add_ticket_relation(
        &self,
        id: TicketIdOrSlug,
        relation: NewTicketRelation,
    ) -> TicketResult<TicketRelation> {
        expect_ticket_result!(
            self.invoke(TicketBackendOperation::AddTicketRelation { id, relation }),
            TicketBackendOperationResult::Relation
        )
    }

    fn query_ticket_relations(
        &self,
        ticket: Option<TicketIdOrSlug>,
        kind: Option<TicketRelationKind>,
    ) -> TicketResult<Vec<TicketRelation>> {
        expect_ticket_result!(
            self.invoke(TicketBackendOperation::QueryTicketRelations { ticket, kind }),
            TicketBackendOperationResult::Relations
        )
    }

    fn relation_view(&self, id: TicketIdOrSlug) -> TicketResult<TicketRelationView> {
        expect_ticket_result!(
            self.invoke(TicketBackendOperation::RelationView { id }),
            TicketBackendOperationResult::RelationView
        )
    }

    fn add_orchestration_plan_record(
        &self,
        id: TicketIdOrSlug,
        record: NewOrchestrationPlanRecord,
    ) -> TicketResult<OrchestrationPlanRecord> {
        expect_ticket_result!(
            self.invoke(TicketBackendOperation::AddOrchestrationPlanRecord { id, record }),
            TicketBackendOperationResult::OrchestrationPlanRecord
        )
    }

    fn query_orchestration_plan_records(
        &self,
        ticket: Option<TicketIdOrSlug>,
        kind: Option<OrchestrationPlanKind>,
    ) -> TicketResult<Vec<OrchestrationPlanRecord>> {
        expect_ticket_result!(
            self.invoke(TicketBackendOperation::QueryOrchestrationPlanRecords { ticket, kind }),
            TicketBackendOperationResult::OrchestrationPlanRecords
        )
    }

    fn doctor(&self) -> TicketResult<TicketDoctorReport> {
        expect_ticket_result!(
            self.invoke(TicketBackendOperation::Doctor),
            TicketBackendOperationResult::DoctorReport
        )
    }
}

pub fn ticket_tools_feature(workspace: impl AsRef<Path>) -> TicketFeature {
    TicketFeature::for_workspace(workspace)
}

pub fn ticket_tools_feature_with_access(
    workspace: impl AsRef<Path>,
    access: TicketFeatureAccess,
) -> TicketFeature {
    TicketFeature::for_workspace_with_access(workspace, access)
}

pub fn ticket_tools_feature_with_backend(
    backend: impl Into<TicketFeatureBackend>,
    access: TicketFeatureAccess,
) -> TicketFeature {
    TicketFeature::with_backend(backend.into(), access)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feature::{FeatureRegistryBuilder, FeatureRuntimeKind};
    use crate::hook::HookRegistryBuilder;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;
    use tempfile::TempDir;

    fn make_ticket_root(root: &Path) {
        std::fs::create_dir_all(root).unwrap();
    }

    fn write_ticket_config(workspace: &Path, content: &str) {
        let yoi_dir = workspace.join(".yoi");
        std::fs::create_dir_all(&yoi_dir).unwrap();
        std::fs::write(yoi_dir.join("workspace.toml"), content).unwrap();
    }

    fn pending_tool_description(
        pending_tools: &[llm_engine::tool::ToolDefinition],
        name: &str,
    ) -> String {
        pending_tools
            .iter()
            .find_map(|definition| {
                let (meta, _) = definition();
                (meta.name == name).then_some(meta.description)
            })
            .expect("tool exists")
    }

    #[test]
    fn descriptor_declares_ticket_tools() {
        let temp = TempDir::new().unwrap();
        let feature = ticket_tools_feature(temp.path());
        let descriptor = feature.descriptor();
        assert_eq!(descriptor.id.to_string(), "builtin:ticket");
        assert_eq!(descriptor.runtime, FeatureRuntimeKind::Builtin);
        assert_eq!(descriptor.tools.len(), WORKSPACE_AUTHORING_TOOL_NAMES.len());
        assert_eq!(
            descriptor
                .tools
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            WORKSPACE_AUTHORING_TOOL_NAMES
        );
    }

    #[test]
    fn read_only_descriptor_declares_only_state_tools() {
        let temp = TempDir::new().unwrap();
        let feature =
            ticket_tools_feature_with_access(temp.path(), TicketFeatureAccess::read_only());
        let descriptor = feature.descriptor();
        assert_eq!(feature.access(), TicketFeatureAccess::read_only());
        assert_eq!(descriptor.tools.len(), READ_ONLY_TOOL_NAMES.len());
        assert_eq!(
            descriptor
                .tools
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            READ_ONLY_TOOL_NAMES
        );
    }

    #[test]
    fn orchestration_control_descriptor_declares_orchestration_tools() {
        let temp = TempDir::new().unwrap();
        let feature = ticket_tools_feature_with_access(
            temp.path(),
            TicketFeatureAccess::orchestration_control(),
        );
        let descriptor = feature.descriptor();
        assert_eq!(
            feature.access(),
            TicketFeatureAccess::orchestration_control()
        );
        assert_eq!(
            descriptor
                .tools
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            ORCHESTRATION_CONTROL_TOOL_NAMES
        );
    }

    #[test]
    fn additive_ticket_capabilities_expose_expected_tool_surfaces() {
        let temp = TempDir::new().unwrap();

        let workspace_authoring = ticket_tools_feature_with_access(
            temp.path(),
            TicketFeatureAccess::workspace_authoring(),
        );
        let workspace_descriptor = workspace_authoring.descriptor();
        let workspace_tools = workspace_descriptor
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>();
        assert!(workspace_tools.contains(&"TicketCreate"));
        assert!(workspace_tools.contains(&"TicketEditItem"));
        assert!(workspace_tools.contains(&"TicketQueue"));
        assert!(!workspace_tools.contains(&"TicketWorkflowState"));

        let orchestration = ticket_tools_feature_with_access(
            temp.path(),
            TicketFeatureAccess::orchestration_control(),
        );
        let orchestration_descriptor = orchestration.descriptor();
        let orchestration_tools = orchestration_descriptor
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>();
        assert!(orchestration_tools.contains(&"TicketWorkflowState"));
        assert!(orchestration_tools.contains(&"TicketDependencyCheck"));
        assert!(orchestration_tools.contains(&"TicketRelationRecord"));
        assert!(orchestration_tools.contains(&"TicketOrchestrationPlanRecord"));
        assert!(!orchestration_tools.contains(&"TicketEditItem"));
        assert!(!orchestration_tools.contains(&"TicketQueue"));

        let work_report =
            ticket_tools_feature_with_access(temp.path(), TicketFeatureAccess::work_report());
        let work_report_descriptor = work_report.descriptor();
        let work_report_tools = work_report_descriptor
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>();
        assert!(work_report_tools.contains(&"TicketComment"));
        assert!(!work_report_tools.contains(&"TicketWorkflowState"));

        let review = ticket_tools_feature_with_access(temp.path(), TicketFeatureAccess::review());
        let review_descriptor = review.descriptor();
        let review_tools = review_descriptor
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>();
        assert!(!review_tools.contains(&"TicketWorkflowState"));
    }

    #[test]
    fn read_only_installation_does_not_expose_mutating_tools() {
        let temp = TempDir::new().unwrap();
        make_ticket_root(&temp.path().join(DEFAULT_TICKET_BACKEND_RELATIVE_PATH));
        let mut pending_tools = Vec::new();
        let mut hooks = HookRegistryBuilder::default();
        let report = FeatureRegistryBuilder::new()
            .with_module(ticket_tools_feature_with_access(
                temp.path(),
                TicketFeatureAccess::read_only(),
            ))
            .install_into_pending(&mut pending_tools, &mut hooks);

        assert_eq!(pending_tools.len(), READ_ONLY_TOOL_NAMES.len());
        assert_eq!(report.reports[0].installed_tools, READ_ONLY_TOOL_NAMES);
        let pending_names = pending_tools
            .iter()
            .map(|definition| definition().0.name)
            .collect::<Vec<_>>();
        assert_eq!(pending_names, READ_ONLY_TOOL_NAMES);
        for name in ticket::tool::TICKET_MUTATING_TOOL_NAMES {
            assert!(
                !report.reports[0]
                    .installed_tools
                    .iter()
                    .any(|tool| tool == name)
            );
            assert!(!pending_names.iter().any(|tool| tool == name));
        }
    }

    #[test]
    fn read_only_companion_style_context_exposes_ticket_language_guidance() {
        let temp = TempDir::new().unwrap();
        write_ticket_config(
            temp.path(),
            r#"
[ticket]
language = "Japanese"
"#,
        );
        make_ticket_root(&temp.path().join(DEFAULT_TICKET_BACKEND_RELATIVE_PATH));
        let feature =
            ticket_tools_feature_with_access(temp.path(), TicketFeatureAccess::read_only());
        let descriptor = feature.descriptor();
        let descriptor_description = descriptor
            .tools
            .iter()
            .find(|tool| tool.name == "TicketShow")
            .expect("TicketShow declared")
            .description
            .clone();
        assert!(descriptor_description.contains("Ticket record language: Japanese"));

        let mut pending_tools = Vec::new();
        let mut hooks = HookRegistryBuilder::default();
        let report = FeatureRegistryBuilder::new()
            .with_module(feature)
            .install_into_pending(&mut pending_tools, &mut hooks);

        assert_eq!(pending_tools.len(), READ_ONLY_TOOL_NAMES.len());
        assert_eq!(report.reports[0].installed_tools, READ_ONLY_TOOL_NAMES);
        let description = pending_tool_description(&pending_tools, "TicketShow");
        assert!(description.contains("Ticket record language: Japanese"));
        assert!(description.contains("distinct from worker.language"));
        assert!(description.contains("Preserve protocol literals"));
    }

    #[test]
    fn workspace_authoring_installation_exposes_authoring_tools() {
        let temp = TempDir::new().unwrap();
        make_ticket_root(&temp.path().join(DEFAULT_TICKET_BACKEND_RELATIVE_PATH));
        let mut pending_tools = Vec::new();
        let mut hooks = HookRegistryBuilder::default();
        let report = FeatureRegistryBuilder::new()
            .with_module(ticket_tools_feature_with_access(
                temp.path(),
                TicketFeatureAccess::workspace_authoring(),
            ))
            .install_into_pending(&mut pending_tools, &mut hooks);

        assert_eq!(pending_tools.len(), WORKSPACE_AUTHORING_TOOL_NAMES.len());
        let installed = report.reports[0]
            .installed_tools
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        assert_eq!(installed, WORKSPACE_AUTHORING_TOOL_NAMES);
        assert!(installed.iter().any(|tool| *tool == "TicketCreate"));
        assert!(installed.iter().any(|tool| *tool == "TicketEditItem"));
        assert!(installed.iter().any(|tool| *tool == "TicketQueue"));
        assert!(!installed.iter().any(|tool| *tool == "TicketIntakeReady"));
        assert!(!installed.iter().any(|tool| *tool == "TicketWorkflowState"));
        assert!(
            !installed
                .iter()
                .any(|tool| *tool == "TicketOrchestrationPlanRecord")
        );
    }

    #[test]
    fn workspace_authoring_context_exposes_ticket_language_guidance() {
        let temp = TempDir::new().unwrap();
        write_ticket_config(
            temp.path(),
            r#"
[ticket]
language = "Japanese"
"#,
        );
        make_ticket_root(&temp.path().join(DEFAULT_TICKET_BACKEND_RELATIVE_PATH));
        let mut pending_tools = Vec::new();
        let mut hooks = HookRegistryBuilder::default();
        let report = FeatureRegistryBuilder::new()
            .with_module(ticket_tools_feature_with_access(
                temp.path(),
                TicketFeatureAccess::workspace_authoring(),
            ))
            .install_into_pending(&mut pending_tools, &mut hooks);

        assert_eq!(pending_tools.len(), WORKSPACE_AUTHORING_TOOL_NAMES.len());
        assert_eq!(
            report.reports[0].installed_tools,
            WORKSPACE_AUTHORING_TOOL_NAMES
        );
        let description = pending_tool_description(&pending_tools, "TicketComment");
        assert!(description.contains("Ticket record language: Japanese"));
        assert!(description.contains("durable Ticket record and Ticket tool body text"));
        assert!(description.contains("distinct from worker.language"));
        assert!(description.contains("memory.language"));
    }

    #[test]
    fn installs_ticket_tools_when_default_root_is_usable() {
        let temp = TempDir::new().unwrap();
        make_ticket_root(&temp.path().join(DEFAULT_TICKET_BACKEND_RELATIVE_PATH));
        let mut pending_tools = Vec::new();
        let mut hooks = HookRegistryBuilder::default();
        let report = FeatureRegistryBuilder::new()
            .with_module(ticket_tools_feature(temp.path()))
            .install_into_pending(&mut pending_tools, &mut hooks);

        assert_eq!(pending_tools.len(), WORKSPACE_AUTHORING_TOOL_NAMES.len());
        assert_eq!(report.reports.len(), 1);
        assert!(report.reports[0].installed);
        assert_eq!(
            report.reports[0].installed_tools,
            WORKSPACE_AUTHORING_TOOL_NAMES
        );
        assert!(report.reports[0].skipped.is_empty());
    }

    #[test]
    fn installs_ticket_tools_with_configured_backend_root() {
        let temp = TempDir::new().unwrap();
        write_ticket_config(
            temp.path(),
            r#"
[ticket.backend]
provider = "builtin:yoi_local"
root = "tickets"

[ticket.roles.coder]
profile = "project:coder"
"#,
        );
        make_ticket_root(&temp.path().join("tickets"));

        let feature = ticket_tools_feature(temp.path());
        assert_eq!(
            feature.backend_root(),
            Some(temp.path().join("tickets").as_path())
        );

        let mut pending_tools = Vec::new();
        let mut hooks = HookRegistryBuilder::default();
        let report = FeatureRegistryBuilder::new()
            .with_module(feature)
            .install_into_pending(&mut pending_tools, &mut hooks);

        assert_eq!(pending_tools.len(), WORKSPACE_AUTHORING_TOOL_NAMES.len());
        assert!(report.reports[0].diagnostics.is_empty());
    }

    #[test]
    fn malformed_ticket_config_fails_closed() {
        let temp = TempDir::new().unwrap();
        make_ticket_root(&temp.path().join(DEFAULT_TICKET_BACKEND_RELATIVE_PATH));
        write_ticket_config(
            temp.path(),
            r#"
[ticket.roles.operator]
profile = "inherit"
"#,
        );
        let mut pending_tools = Vec::new();
        let mut hooks = HookRegistryBuilder::default();
        let report = FeatureRegistryBuilder::new()
            .with_module(ticket_tools_feature(temp.path()))
            .install_into_pending(&mut pending_tools, &mut hooks);

        assert!(pending_tools.is_empty());
        assert!(report.reports[0].installed_tools.is_empty());
        assert_eq!(report.reports[0].diagnostics.len(), 1);
        let message = &report.reports[0].diagnostics[0].message;
        assert!(message.contains("Ticket tools not registered"));
        assert!(message.contains("unsupported Ticket role `operator`"));
    }

    #[test]
    fn unsupported_ticket_backend_provider_fails_closed() {
        let temp = TempDir::new().unwrap();
        make_ticket_root(&temp.path().join(DEFAULT_TICKET_BACKEND_RELATIVE_PATH));
        write_ticket_config(
            temp.path(),
            r#"
[ticket.backend]
provider = "github"
"#,
        );
        let mut pending_tools = Vec::new();
        let mut hooks = HookRegistryBuilder::default();
        let report = FeatureRegistryBuilder::new()
            .with_module(ticket_tools_feature(temp.path()))
            .install_into_pending(&mut pending_tools, &mut hooks);

        assert!(pending_tools.is_empty());
        assert!(report.reports[0].installed_tools.is_empty());
        assert_eq!(report.reports[0].diagnostics.len(), 1);
        let message = &report.reports[0].diagnostics[0].message;
        assert!(message.contains("Ticket tools not registered"));
        assert!(message.contains("unsupported Ticket backend provider `github`"));
    }

    #[test]
    fn does_not_register_ticket_tools_when_root_is_missing() {
        let temp = TempDir::new().unwrap();
        let mut pending_tools = Vec::new();
        let mut hooks = HookRegistryBuilder::default();
        let report = FeatureRegistryBuilder::new()
            .with_module(ticket_tools_feature(temp.path()))
            .install_into_pending(&mut pending_tools, &mut hooks);

        assert!(pending_tools.is_empty());
        assert_eq!(report.reports.len(), 1);
        assert!(report.reports[0].installed);
        assert!(report.reports[0].installed_tools.is_empty());
        assert_eq!(report.reports[0].diagnostics.len(), 1);
        assert!(
            report.reports[0].diagnostics[0]
                .message
                .contains("Ticket tools not registered")
        );
    }

    #[test]
    fn registers_ticket_tools_for_flat_backend_root() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join(DEFAULT_TICKET_BACKEND_RELATIVE_PATH);
        std::fs::create_dir_all(&root).unwrap();
        let mut pending_tools = Vec::new();
        let mut hooks = HookRegistryBuilder::default();
        let report = FeatureRegistryBuilder::new()
            .with_module(ticket_tools_feature(temp.path()))
            .install_into_pending(&mut pending_tools, &mut hooks);

        assert_eq!(pending_tools.len(), WORKSPACE_AUTHORING_TOOL_NAMES.len());
        assert_eq!(
            report.reports[0].installed_tools,
            WORKSPACE_AUTHORING_TOOL_NAMES
        );
        assert!(report.reports[0].diagnostics.is_empty());
        assert!(!root.join("open").exists());
        assert!(!root.join("pending").exists());
        assert!(!root.join("closed").exists());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn workspace_http_backend_invoke_is_safe_inside_async_context() {
        let backend = WorkspaceHttpTicketBackend::new(Arc::new(
            crate::worker::TestWorkspaceHttpClient::new("workspace-a", "not-a-url"),
        ));

        let error = backend
            .invoke(TicketBackendOperation::DefaultIntakeReadyStateChangeBody {
                from: "planning".to_string(),
            })
            .unwrap_err();

        assert!(error.to_string().contains("ticket REST request failed"));
    }

    #[test]
    fn workspace_http_backend_posts_ticket_event_subresource() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buffer = [0_u8; 8192];
            let size = stream.read(&mut buffer).unwrap();
            let request = String::from_utf8_lossy(&buffer[..size]);
            assert!(
                request
                    .starts_with("POST /api/w/workspace-a/tickets/01TEST/thread-events HTTP/1.1")
            );
            assert!(!request.contains("\"operation\""));
            assert!(request.contains("\"kind\":\"comment\""));
            stream
                .write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n")
                .unwrap();
        });
        let client = Arc::new(crate::worker::TestWorkspaceHttpClient::new(
            "workspace-a",
            format!("http://{address}"),
        ));
        let backend = WorkspaceHttpTicketBackend::new(client);

        backend
            .add_event(
                TicketIdOrSlug::Id("01TEST".to_string()),
                NewTicketEvent::new(ticket::TicketEventKind::Comment, "REST comment"),
            )
            .unwrap();
        server.join().unwrap();
    }

    #[test]
    fn workspace_http_backend_executes_ticket_create_operation() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buffer = [0_u8; 8192];
            let len = stream.read(&mut buffer).unwrap();
            let request = String::from_utf8_lossy(&buffer[..len]);
            assert!(request.starts_with("POST /api/w/workspace-a/tickets HTTP/1.1"));
            assert!(!request.contains("\"operation\""));
            assert!(request.contains("\"title\":\"HTTP ticket\""));
            let response_body = serde_json::to_string(&TicketRef {
                id: "01TEST".to_string(),
                slug: "http-ticket".to_string(),
                status: ticket::TicketStatus::Open,
            })
            .unwrap();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                response_body.len(),
                response_body
            )
            .unwrap();
        });

        let backend = WorkspaceHttpTicketBackend::new(Arc::new(
            crate::worker::TestWorkspaceHttpClient::new("workspace-a", base_url),
        ));
        let created = backend.create(NewTicket::new("HTTP ticket")).unwrap();

        server.join().unwrap();
        assert_eq!(created.id, "01TEST");
        assert_eq!(created.slug, "http-ticket");
    }
}
