//! Built-in Ticket feature adapter.
//!
//! The ticket crate owns Ticket domain logic and Tool implementations. This
//! module only resolves the local backend root, declares the built-in feature,
//! and contributes those tools through the normal feature registry path.

use std::path::{Path, PathBuf};

use ticket::{
    LocalTicketBackend, MarkdownText, NewOrchestrationPlanRecord, NewTicket, NewTicketEvent,
    NewTicketRelation, OrchestrationPlanKind, OrchestrationPlanRecord, Result as TicketResult,
    Ticket, TicketBackend, TicketBackendHttpResponse, TicketBackendOperation,
    TicketBackendOperationResult, TicketDoctorReport, TicketError, TicketFilter, TicketIdOrSlug,
    TicketIntakeSummary, TicketRef, TicketRelation, TicketRelationKind, TicketRelationView,
    TicketReview, TicketStateChange, TicketSummary,
    config::{DEFAULT_TICKET_BACKEND_RELATIVE_PATH, TicketConfig},
    tool::{
        TICKET_BASE_READ_ONLY_TOOL_NAMES, TICKET_BASE_TOOL_NAMES,
        TICKET_ORCHESTRATION_READ_ONLY_TOOL_NAMES, TICKET_ORCHESTRATION_TOOL_NAMES,
        TICKET_READ_ONLY_TOOL_NAMES, TICKET_TOOL_NAMES, TicketToolBackend, ticket_tool_description,
        ticket_tools,
    },
};

use crate::feature::{
    FeatureDescriptor, FeatureDiagnostic, FeatureInstallContext, FeatureInstallError,
    FeatureModule, ToolContribution, ToolDeclaration,
};

const FEATURE_ID: &str = "ticket";
const FEATURE_NAME: &str = "Ticket tools";
const FEATURE_DESCRIPTION: &str = "Typed local Ticket work-item operations over a bounded backend root. \
The tools operate through the ticket crate backend and do not grant generic filesystem write scope.";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TicketFeatureAccess {
    /// Status/diagnostic access for views such as Companion that must not mutate Tickets.
    ReadOnly,
    /// Full Ticket lifecycle access, including the read-only tools and all mutating Ticket tools.
    Lifecycle,
}

impl TicketFeatureAccess {
    pub fn base_tool_names(self) -> &'static [&'static str] {
        match self {
            Self::ReadOnly => &TICKET_BASE_READ_ONLY_TOOL_NAMES,
            Self::Lifecycle => &TICKET_BASE_TOOL_NAMES,
        }
    }

    pub fn orchestration_tool_names(self) -> &'static [&'static str] {
        match self {
            Self::ReadOnly => &TICKET_ORCHESTRATION_READ_ONLY_TOOL_NAMES,
            Self::Lifecycle => &TICKET_ORCHESTRATION_TOOL_NAMES,
        }
    }
}

#[derive(Clone, Debug)]
pub enum TicketFeatureBackend {
    Local {
        root: PathBuf,
    },
    LocalWorkspace {
        workspace_root: PathBuf,
    },
    WorkspaceHttp {
        workspace_id: String,
        base_url: String,
    },
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
    include_base_tools: bool,
    include_orchestration_tools: bool,
}

impl TicketFeature {
    pub fn new(backend_root: impl Into<PathBuf>) -> Self {
        Self::new_with_access(backend_root, TicketFeatureAccess::Lifecycle)
    }

    pub fn new_with_access(backend_root: impl Into<PathBuf>, access: TicketFeatureAccess) -> Self {
        Self::new_with_options(backend_root, Some(access), true)
    }

    pub fn new_with_options(
        backend_root: impl Into<PathBuf>,
        access: Option<TicketFeatureAccess>,
        include_orchestration_tools: bool,
    ) -> Self {
        Self::with_backend(
            TicketFeatureBackend::Local {
                root: backend_root.into(),
            },
            access,
            include_orchestration_tools,
        )
    }

    pub fn with_backend(
        backend: TicketFeatureBackend,
        access: Option<TicketFeatureAccess>,
        include_orchestration_tools: bool,
    ) -> Self {
        if let TicketFeatureBackend::LocalWorkspace { workspace_root } = backend {
            return Self::for_workspace_with_options(
                workspace_root,
                access,
                include_orchestration_tools,
            );
        }
        Self {
            backend,
            record_language: None,
            config_error: None,
            access: access.unwrap_or(TicketFeatureAccess::Lifecycle),
            include_base_tools: access.is_some(),
            include_orchestration_tools,
        }
    }

    pub fn for_workspace(workspace: impl AsRef<Path>) -> Self {
        Self::for_workspace_with_access(workspace, TicketFeatureAccess::Lifecycle)
    }

    pub fn for_workspace_with_access(
        workspace: impl AsRef<Path>,
        access: TicketFeatureAccess,
    ) -> Self {
        Self::for_workspace_with_options(workspace, Some(access), true)
    }

    pub fn for_workspace_with_options(
        workspace: impl AsRef<Path>,
        access: Option<TicketFeatureAccess>,
        include_orchestration_tools: bool,
    ) -> Self {
        let workspace = workspace.as_ref();
        match TicketConfig::load_workspace(workspace) {
            Ok(config) => {
                let backend_root = config.backend_root().to_path_buf();
                let record_language = config.ticket_record_language().map(str::to_string);
                let mut feature =
                    Self::new_with_options(backend_root, access, include_orchestration_tools);
                feature.record_language = record_language;
                feature
            }
            Err(error) => {
                let access_value = access.unwrap_or(TicketFeatureAccess::Lifecycle);
                Self {
                    backend: TicketFeatureBackend::Local {
                        root: workspace.join(DEFAULT_TICKET_BACKEND_RELATIVE_PATH),
                    },
                    record_language: None,
                    config_error: Some(error.to_string()),
                    access: access_value,
                    include_base_tools: access.is_some(),
                    include_orchestration_tools,
                }
            }
        }
    }

    pub fn backend_root(&self) -> Option<&Path> {
        match &self.backend {
            TicketFeatureBackend::Local { root } => Some(root),
            TicketFeatureBackend::LocalWorkspace { workspace_root } => Some(workspace_root),
            TicketFeatureBackend::WorkspaceHttp { .. } => None,
        }
    }

    pub fn access(&self) -> TicketFeatureAccess {
        self.access
    }

    fn enabled_tool_names(&self) -> Vec<&'static str> {
        if self.include_base_tools && self.include_orchestration_tools {
            return match self.access {
                TicketFeatureAccess::ReadOnly => TICKET_READ_ONLY_TOOL_NAMES.to_vec(),
                TicketFeatureAccess::Lifecycle => TICKET_TOOL_NAMES.to_vec(),
            };
        }
        let mut names = Vec::new();
        if self.include_base_tools {
            names.extend_from_slice(self.access.base_tool_names());
        }
        if self.include_orchestration_tools {
            names.extend_from_slice(self.access.orchestration_tool_names());
        }
        names
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
            TicketFeatureBackend::Local { root: _ }
            | TicketFeatureBackend::LocalWorkspace { workspace_root: _ } => {
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
            TicketFeatureBackend::WorkspaceHttp {
                workspace_id,
                base_url,
            } => Some(
                TicketToolBackend::new(WorkspaceHttpTicketBackend::new(
                    workspace_id.clone(),
                    base_url.clone(),
                ))
                .with_record_language(self.record_language.as_deref()),
            ),
        }
    }
}

impl FeatureModule for TicketFeature {
    fn descriptor(&self) -> FeatureDescriptor {
        let mut descriptor = FeatureDescriptor::builtin(FEATURE_ID, FEATURE_NAME)
            .with_description(FEATURE_DESCRIPTION);
        let enabled_tool_names = self.enabled_tool_names();
        for name in &enabled_tool_names {
            descriptor = descriptor.with_tool(ToolDeclaration::new(
                *name,
                ticket_tool_description(name, self.record_language.as_deref()),
            ));
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
        Ok(())
    }
}

#[derive(Clone, Debug)]
struct WorkspaceHttpTicketBackend {
    workspace_id: String,
    base_url: String,
}

impl WorkspaceHttpTicketBackend {
    fn new(workspace_id: String, base_url: String) -> Self {
        Self {
            workspace_id,
            base_url: base_url.trim_end_matches('/').to_string(),
        }
    }

    fn endpoint(&self) -> String {
        format!(
            "{}/api/w/{}/tickets/backend",
            self.base_url, self.workspace_id
        )
    }

    fn invoke(
        &self,
        operation: TicketBackendOperation,
    ) -> TicketResult<TicketBackendOperationResult> {
        let endpoint = self.endpoint();
        if tokio::runtime::Handle::try_current().is_ok() {
            return std::thread::spawn(move || Self::invoke_http(endpoint, operation))
                .join()
                .map_err(|_| {
                    TicketError::Conflict("ticket backend request thread panicked".to_string())
                })?;
        }
        Self::invoke_http(endpoint, operation)
    }

    fn invoke_http(
        endpoint: String,
        operation: TicketBackendOperation,
    ) -> TicketResult<TicketBackendOperationResult> {
        let body = serde_json::to_string(&operation).map_err(|error| {
            TicketError::Conflict(format!("serialize ticket operation: {error}"))
        })?;
        let response = reqwest::blocking::Client::new()
            .post(endpoint)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body)
            .send()
            .map_err(|error| {
                TicketError::Conflict(format!("ticket backend request failed: {error}"))
            })?;
        let status = response.status();
        let text = response.text().map_err(|error| {
            TicketError::Conflict(format!("ticket backend response failed: {error}"))
        })?;
        if !status.is_success() {
            return Err(TicketError::Conflict(format!(
                "ticket backend returned HTTP {status}: {text}"
            )));
        }
        match serde_json::from_str::<TicketBackendHttpResponse>(&text).map_err(|error| {
            TicketError::Conflict(format!("decode ticket backend response: {error}"))
        })? {
            TicketBackendHttpResponse::Ok { result } => Ok(result),
            TicketBackendHttpResponse::Error { message } => Err(TicketError::Conflict(message)),
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

    fn list(&self, filter: TicketFilter) -> TicketResult<Vec<TicketSummary>> {
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

    fn review(&self, id: TicketIdOrSlug, review: TicketReview) -> TicketResult<()> {
        match self.invoke(TicketBackendOperation::Review { id, review })? {
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

pub fn ticket_tools_feature_with_options(
    backend: impl Into<TicketFeatureBackend>,
    access: Option<TicketFeatureAccess>,
    include_orchestration_tools: bool,
) -> TicketFeature {
    TicketFeature::with_backend(backend.into(), access, include_orchestration_tools)
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
    use ticket::tool::{
        TICKET_BASE_TOOL_NAMES, TICKET_ORCHESTRATION_TOOL_NAMES, TICKET_READ_ONLY_TOOL_NAMES,
        TICKET_TOOL_NAMES,
    };

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
        assert_eq!(descriptor.tools.len(), TICKET_TOOL_NAMES.len());
        assert_eq!(
            descriptor
                .tools
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            TICKET_TOOL_NAMES
        );
    }

    #[test]
    fn read_only_descriptor_declares_only_state_tools() {
        let temp = TempDir::new().unwrap();
        let feature = ticket_tools_feature_with_access(temp.path(), TicketFeatureAccess::ReadOnly);
        let descriptor = feature.descriptor();
        assert_eq!(feature.access(), TicketFeatureAccess::ReadOnly);
        assert_eq!(descriptor.tools.len(), TICKET_READ_ONLY_TOOL_NAMES.len());
        assert_eq!(
            descriptor
                .tools
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            TICKET_READ_ONLY_TOOL_NAMES
        );
    }

    #[test]
    fn descriptor_can_expose_base_ticket_without_orchestration_tools() {
        let temp = TempDir::new().unwrap();
        let feature = ticket_tools_feature_with_options(
            temp.path(),
            Some(TicketFeatureAccess::Lifecycle),
            false,
        );
        let descriptor = feature.descriptor();
        assert_eq!(
            descriptor
                .tools
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            TICKET_BASE_TOOL_NAMES
        );
    }

    #[test]
    fn descriptor_can_expose_orchestration_only_tools() {
        let temp = TempDir::new().unwrap();
        let feature = ticket_tools_feature_with_options(temp.path(), None, true);
        let descriptor = feature.descriptor();
        assert_eq!(
            descriptor
                .tools
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            TICKET_ORCHESTRATION_TOOL_NAMES
        );
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
                TicketFeatureAccess::ReadOnly,
            ))
            .install_into_pending(&mut pending_tools, &mut hooks);

        assert_eq!(pending_tools.len(), TICKET_READ_ONLY_TOOL_NAMES.len());
        assert_eq!(
            report.reports[0].installed_tools,
            TICKET_READ_ONLY_TOOL_NAMES
        );
        let pending_names = pending_tools
            .iter()
            .map(|definition| definition().0.name)
            .collect::<Vec<_>>();
        assert_eq!(pending_names, TICKET_READ_ONLY_TOOL_NAMES);
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
        let feature = ticket_tools_feature_with_access(temp.path(), TicketFeatureAccess::ReadOnly);
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

        assert_eq!(pending_tools.len(), TICKET_READ_ONLY_TOOL_NAMES.len());
        assert_eq!(
            report.reports[0].installed_tools,
            TICKET_READ_ONLY_TOOL_NAMES
        );
        let description = pending_tool_description(&pending_tools, "TicketShow");
        assert!(description.contains("Ticket record language: Japanese"));
        assert!(description.contains("distinct from worker.language"));
        assert!(description.contains("Preserve protocol literals"));
    }

    #[test]
    fn lifecycle_installation_exposes_lifecycle_tools() {
        let temp = TempDir::new().unwrap();
        make_ticket_root(&temp.path().join(DEFAULT_TICKET_BACKEND_RELATIVE_PATH));
        let mut pending_tools = Vec::new();
        let mut hooks = HookRegistryBuilder::default();
        let report = FeatureRegistryBuilder::new()
            .with_module(ticket_tools_feature_with_access(
                temp.path(),
                TicketFeatureAccess::Lifecycle,
            ))
            .install_into_pending(&mut pending_tools, &mut hooks);

        assert_eq!(pending_tools.len(), TICKET_TOOL_NAMES.len());
        assert_eq!(report.reports[0].installed_tools, TICKET_TOOL_NAMES);
        for name in ticket::tool::TICKET_MUTATING_TOOL_NAMES {
            assert!(
                report.reports[0]
                    .installed_tools
                    .iter()
                    .any(|tool| tool == name)
            );
        }
        assert!(
            report.reports[0]
                .installed_tools
                .iter()
                .any(|tool| tool == "TicketIntakeReady")
        );
        assert!(
            report.reports[0]
                .installed_tools
                .iter()
                .any(|tool| tool == "TicketWorkflowState")
        );
    }

    #[test]
    fn lifecycle_ticket_role_style_context_exposes_ticket_language_guidance() {
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
                TicketFeatureAccess::Lifecycle,
            ))
            .install_into_pending(&mut pending_tools, &mut hooks);

        assert_eq!(pending_tools.len(), TICKET_TOOL_NAMES.len());
        assert_eq!(report.reports[0].installed_tools, TICKET_TOOL_NAMES);
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

        assert_eq!(pending_tools.len(), TICKET_TOOL_NAMES.len());
        assert_eq!(report.reports.len(), 1);
        assert!(report.reports[0].installed);
        assert_eq!(report.reports[0].installed_tools, TICKET_TOOL_NAMES);
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

        assert_eq!(pending_tools.len(), TICKET_TOOL_NAMES.len());
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

        assert_eq!(pending_tools.len(), TICKET_TOOL_NAMES.len());
        assert_eq!(report.reports[0].installed_tools, TICKET_TOOL_NAMES);
        assert!(report.reports[0].diagnostics.is_empty());
        assert!(!root.join("open").exists());
        assert!(!root.join("pending").exists());
        assert!(!root.join("closed").exists());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn workspace_http_backend_invoke_is_safe_inside_async_context() {
        let backend =
            WorkspaceHttpTicketBackend::new("workspace-a".to_string(), "not-a-url".to_string());

        let error = backend
            .invoke(TicketBackendOperation::DefaultIntakeReadyStateChangeBody {
                from: "planning".to_string(),
            })
            .unwrap_err();

        assert!(error.to_string().contains("ticket backend request failed"));
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
            assert!(request.starts_with("POST /api/w/workspace-a/tickets/backend HTTP/1.1"));
            assert!(request.contains("\"operation\":\"create\""));
            assert!(request.contains("\"title\":\"HTTP ticket\""));
            let response_body = serde_json::to_string(&TicketBackendHttpResponse::Ok {
                result: TicketBackendOperationResult::TicketRef(TicketRef {
                    id: "01TEST".to_string(),
                    slug: "http-ticket".to_string(),
                    status: ticket::TicketStatus::Open,
                }),
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

        let backend = WorkspaceHttpTicketBackend::new("workspace-a".to_string(), base_url);
        let created = backend.create(NewTicket::new("HTTP ticket")).unwrap();

        server.join().unwrap();
        assert_eq!(created.id, "01TEST");
        assert_eq!(created.slug, "http-ticket");
    }
}
