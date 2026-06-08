//! Built-in Ticket feature adapter.
//!
//! The ticket crate owns Ticket domain logic and Tool implementations. This
//! module only resolves the local backend root, declares the built-in feature,
//! and contributes those tools through the normal feature registry path.

use std::path::{Path, PathBuf};

use ticket::{
    LocalTicketBackend,
    config::{DEFAULT_TICKET_BACKEND_RELATIVE_PATH, TicketConfig},
    tool::{TICKET_READ_ONLY_TOOL_NAMES, TICKET_TOOL_NAMES, ticket_tools},
};

use crate::feature::{
    FeatureDescriptor, FeatureDiagnostic, FeatureInstallContext, FeatureInstallError,
    FeatureModule, HostAuthority, HostAuthorityRequest, ToolContribution, ToolDeclaration,
};

const FEATURE_ID: &str = "ticket";
const FEATURE_NAME: &str = "Ticket tools";
const FEATURE_DESCRIPTION: &str = "Typed local Ticket work-item operations over a bounded backend root. \
The tools operate through the ticket crate backend and do not grant generic filesystem write scope.";
const AUTHORITY_REASON: &str = "Use a configured local Ticket backend root for typed work-item operations without generic filesystem write authority.";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TicketFeatureAccess {
    /// Status/diagnostic access for views such as Companion that must not mutate Tickets.
    ReadOnly,
    /// Full Ticket lifecycle access, including the read-only tools and all mutating Ticket tools.
    Lifecycle,
}

impl TicketFeatureAccess {
    pub fn tool_names(self) -> &'static [&'static str] {
        match self {
            Self::ReadOnly => &TICKET_READ_ONLY_TOOL_NAMES,
            Self::Lifecycle => &TICKET_TOOL_NAMES,
        }
    }
}

#[derive(Clone, Debug)]
pub struct TicketFeature {
    backend_root: PathBuf,
    record_language: Option<String>,
    config_error: Option<String>,
    access: TicketFeatureAccess,
}

impl TicketFeature {
    pub fn new(backend_root: impl Into<PathBuf>) -> Self {
        Self::new_with_access(backend_root, TicketFeatureAccess::Lifecycle)
    }

    pub fn new_with_access(backend_root: impl Into<PathBuf>, access: TicketFeatureAccess) -> Self {
        Self {
            backend_root: backend_root.into(),
            record_language: None,
            config_error: None,
            access,
        }
    }

    pub fn for_workspace(workspace: impl AsRef<Path>) -> Self {
        Self::for_workspace_with_access(workspace, TicketFeatureAccess::Lifecycle)
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
                backend_root: workspace.join(DEFAULT_TICKET_BACKEND_RELATIVE_PATH),
                record_language: None,
                config_error: Some(error.to_string()),
                access,
            },
        }
    }

    pub fn backend_root(&self) -> &Path {
        &self.backend_root
    }

    pub fn access(&self) -> TicketFeatureAccess {
        self.access
    }

    fn authority(&self) -> HostAuthority {
        HostAuthority::TicketBackend {
            root: self.backend_root.display().to_string(),
        }
    }

    fn usable_backend_root(&self) -> Result<PathBuf, String> {
        let root = self
            .backend_root
            .canonicalize()
            .map_err(|error| format!("ticket backend root is not usable: {error}"))?;
        if !root.is_dir() {
            return Err("ticket backend root is not a directory".to_string());
        }
        for status_dir in ["open", "pending", "closed"] {
            let dir = root.join(status_dir);
            if !dir.is_dir() {
                return Err(format!(
                    "ticket backend root is missing required {status_dir}/ directory"
                ));
            }
        }
        Ok(root)
    }
}

impl FeatureModule for TicketFeature {
    fn descriptor(&self) -> FeatureDescriptor {
        let mut descriptor = FeatureDescriptor::builtin(FEATURE_ID, FEATURE_NAME)
            .with_description(FEATURE_DESCRIPTION)
            .with_host_authority(HostAuthorityRequest::required(
                self.authority(),
                AUTHORITY_REASON,
            ));
        for name in self.access.tool_names() {
            descriptor = descriptor.with_tool(ToolDeclaration::new(*name, tool_description(name)));
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
        let usable_root = match self.usable_backend_root() {
            Ok(root) => root,
            Err(reason) => {
                context
                    .diagnostics()
                    .push(FeatureDiagnostic::warning(format!(
                        "Ticket tools not registered: {reason}; root={} ",
                        self.backend_root.display()
                    )));
                return Ok(());
            }
        };
        let authority = self.authority();
        let backend = LocalTicketBackend::new(usable_root)
            .with_record_language(self.record_language.as_deref());
        let allowed_tool_names = self.access.tool_names();
        let mut tools = context.tools();
        for definition in ticket_tools(backend) {
            let (meta, _) = definition();
            let name = meta.name.clone();
            if !allowed_tool_names.contains(&name.as_str()) {
                continue;
            }
            tools.register(
                ToolContribution::new(name, definition)
                    .with_required_host_authorities(vec![authority.clone()]),
            )?;
        }
        Ok(())
    }
}

fn tool_description(name: &str) -> &'static str {
    match name {
        "TicketCreate" => "Create a Ticket through the typed local Ticket backend.",
        "TicketList" => "List Tickets through the typed local Ticket backend with bounded output.",
        "TicketShow" => {
            "Show one Ticket through the typed local Ticket backend with bounded output."
        }
        "TicketComment" => {
            "Append a comment/plan/decision/implementation_report event to a Ticket."
        }
        "TicketReview" => "Append an approve/request_changes review event to a Ticket.",
        "TicketIntakeReady" => {
            "Mark an intake Ticket ready and append the typed intake summary/state transition events."
        }
        "TicketWorkflowState" => {
            "Transition Ticket workflow_state; queued -> inprogress is the accepted implementation start, so implementation side effects should happen only after that transition is accepted and recorded."
        }
        "TicketStatus" => "Move a Ticket between open and pending; use TicketClose for closed.",
        "TicketClose" => "Close a Ticket with a resolution through the typed local Ticket backend.",
        "TicketDoctor" => "Run typed local Ticket backend consistency checks.",
        _ => "Typed Ticket backend tool.",
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feature::{FeatureRegistryBuilder, FeatureRuntimeKind};
    use crate::hook::HookRegistryBuilder;
    use tempfile::TempDir;

    fn make_ticket_root(root: &Path) {
        std::fs::create_dir_all(root.join("open")).unwrap();
        std::fs::create_dir_all(root.join("pending")).unwrap();
        std::fs::create_dir_all(root.join("closed")).unwrap();
    }

    fn write_ticket_config(workspace: &Path, content: &str) {
        let yoi_dir = workspace.join(".yoi");
        std::fs::create_dir_all(&yoi_dir).unwrap();
        std::fs::write(yoi_dir.join("ticket.config.toml"), content).unwrap();
    }

    #[test]
    fn descriptor_declares_ticket_tools_and_backend_authority() {
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
        assert_eq!(descriptor.requested_host_authorities.len(), 1);
        assert!(matches!(
            descriptor.requested_host_authorities[0].authority,
            HostAuthority::TicketBackend { .. }
        ));
    }

    #[test]
    fn read_only_descriptor_declares_only_status_tools() {
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
        assert_eq!(descriptor.requested_host_authorities.len(), 1);
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
[backend]
provider = "builtin:yoi_local"
root = "tickets"

[roles.coder]
profile = "project:coder"
"#,
        );
        make_ticket_root(&temp.path().join("tickets"));

        let feature = ticket_tools_feature(temp.path());
        assert_eq!(feature.backend_root(), temp.path().join("tickets"));

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
[roles.operator]
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
        assert!(message.contains("unknown Ticket role `operator`"));
    }

    #[test]
    fn unsupported_ticket_backend_provider_fails_closed() {
        let temp = TempDir::new().unwrap();
        make_ticket_root(&temp.path().join(DEFAULT_TICKET_BACKEND_RELATIVE_PATH));
        write_ticket_config(
            temp.path(),
            r#"
[backend]
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
    fn does_not_register_ticket_tools_when_root_lacks_status_dirs() {
        let temp = TempDir::new().unwrap();
        std::fs::create_dir_all(temp.path().join(DEFAULT_TICKET_BACKEND_RELATIVE_PATH)).unwrap();
        let mut pending_tools = Vec::new();
        let mut hooks = HookRegistryBuilder::default();
        let report = FeatureRegistryBuilder::new()
            .with_module(ticket_tools_feature(temp.path()))
            .install_into_pending(&mut pending_tools, &mut hooks);

        assert!(pending_tools.is_empty());
        assert!(report.reports[0].installed_tools.is_empty());
        assert!(
            report.reports[0].diagnostics[0]
                .message
                .contains("missing required open/ directory")
        );
    }
}
