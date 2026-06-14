//! Built-in Ticket feature adapter.
//!
//! The ticket crate owns Ticket domain logic and Tool implementations. This
//! module only resolves the local backend root, declares the built-in feature,
//! and contributes those tools through the normal feature registry path.

use std::path::{Path, PathBuf};

use ticket::{
    LocalTicketBackend,
    config::{DEFAULT_TICKET_BACKEND_RELATIVE_PATH, TicketConfig},
    tool::{
        TICKET_BASE_READ_ONLY_TOOL_NAMES, TICKET_BASE_TOOL_NAMES,
        TICKET_ORCHESTRATION_READ_ONLY_TOOL_NAMES, TICKET_ORCHESTRATION_TOOL_NAMES,
        TICKET_READ_ONLY_TOOL_NAMES, TICKET_TOOL_NAMES, ticket_tool_description, ticket_tools,
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
pub struct TicketFeature {
    backend_root: PathBuf,
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
        Self {
            backend_root: backend_root.into(),
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
                    backend_root: workspace.join(DEFAULT_TICKET_BACKEND_RELATIVE_PATH),
                    record_language: None,
                    config_error: Some(error.to_string()),
                    access: access_value,
                    include_base_tools: access.is_some(),
                    include_orchestration_tools,
                }
            }
        }
    }

    pub fn backend_root(&self) -> &Path {
        &self.backend_root
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
        let root = self
            .backend_root
            .canonicalize()
            .map_err(|error| format!("ticket backend root is not usable: {error}"))?;
        if !root.is_dir() {
            return Err("ticket backend root is not a directory".to_string());
        }
        Ok(root)
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
        let backend = LocalTicketBackend::new(usable_root)
            .with_record_language(self.record_language.as_deref());
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
    workspace: impl AsRef<Path>,
    access: Option<TicketFeatureAccess>,
    include_orchestration_tools: bool,
) -> TicketFeature {
    TicketFeature::for_workspace_with_options(workspace, access, include_orchestration_tools)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feature::{FeatureRegistryBuilder, FeatureRuntimeKind};
    use crate::hook::HookRegistryBuilder;
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
        std::fs::write(yoi_dir.join("ticket.config.toml"), content).unwrap();
    }

    fn pending_tool_description(
        pending_tools: &[llm_worker::tool::ToolDefinition],
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
        assert!(message.contains("unsupported Ticket role `operator`"));
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
}
