//! Built-in Ticket feature adapter.
//!
//! The ticket crate owns Ticket domain logic and Tool implementations. This
//! module only resolves the local backend root, declares the built-in feature,
//! and contributes those tools through the normal feature registry path.

use std::path::{Path, PathBuf};

use ticket::{LocalTicketBackend, tool::TICKET_TOOL_NAMES, tool::ticket_tools};

use crate::feature::{
    FeatureDescriptor, FeatureDiagnostic, FeatureInstallContext, FeatureInstallError,
    FeatureModule, HostAuthority, HostAuthorityRequest, ToolContribution, ToolDeclaration,
};

const FEATURE_ID: &str = "ticket";
const FEATURE_NAME: &str = "Ticket tools";
const FEATURE_DESCRIPTION: &str = "Typed local Ticket work-item operations over a bounded backend root. \
The tools operate through the ticket crate backend and do not grant generic filesystem write scope.";
const AUTHORITY_REASON: &str = "Use a configured local Ticket backend root for typed work-item operations without generic filesystem write authority.";

#[derive(Clone, Debug)]
pub struct TicketFeature {
    backend_root: PathBuf,
}

impl TicketFeature {
    pub fn new(backend_root: impl Into<PathBuf>) -> Self {
        Self {
            backend_root: backend_root.into(),
        }
    }

    pub fn for_workspace(workspace: impl AsRef<Path>) -> Self {
        Self::new(workspace.as_ref().join("work-items"))
    }

    pub fn backend_root(&self) -> &Path {
        &self.backend_root
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
        for name in TICKET_TOOL_NAMES {
            descriptor = descriptor.with_tool(ToolDeclaration::new(name, tool_description(name)));
        }
        descriptor
    }

    fn install(&self, context: &mut FeatureInstallContext<'_>) -> Result<(), FeatureInstallError> {
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
        let backend = LocalTicketBackend::new(usable_root);
        let mut tools = context.tools();
        for definition in ticket_tools(backend) {
            let (meta, _) = definition();
            let name = meta.name.clone();
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
        "TicketStatus" => "Move a Ticket between open and pending; use TicketClose for closed.",
        "TicketClose" => "Close a Ticket with a resolution through the typed local Ticket backend.",
        "TicketDoctor" => "Run typed local Ticket backend consistency checks.",
        _ => "Typed Ticket backend tool.",
    }
}

pub fn ticket_tools_feature(workspace: impl AsRef<Path>) -> TicketFeature {
    TicketFeature::for_workspace(workspace)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feature::{FeatureRegistryBuilder, FeatureRuntimeKind};
    use crate::hook::HookRegistryBuilder;
    use tempfile::TempDir;

    fn make_work_items(root: &Path) {
        std::fs::create_dir_all(root.join("open")).unwrap();
        std::fs::create_dir_all(root.join("pending")).unwrap();
        std::fs::create_dir_all(root.join("closed")).unwrap();
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
    fn installs_ticket_tools_when_work_items_root_is_usable() {
        let temp = TempDir::new().unwrap();
        make_work_items(&temp.path().join("work-items"));
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
        std::fs::create_dir_all(temp.path().join("work-items")).unwrap();
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
