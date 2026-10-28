//! Objective registration belongs to the Feature install scope, not the Engine
//! bootstrap. The Host supplies the live API; resolved enablement never grants
//! operation authority. The existing backend revalidates every access/effect.

use super::*;
use crate::feature::{
    FeatureDescriptor, FeatureInstallContext, FeatureInstallError, FeatureModule,
    FeatureRegistryBuilder, ServiceDeclaration, ServiceId, ServiceRequirement, ToolContribution,
    ToolDeclaration,
};

const BACKEND_SERVICE: &str = "objective.workspace-api";
const WIP_MOUNT: &str = "objective.native";

fn backend_declaration() -> ServiceDeclaration {
    ServiceDeclaration::new(
        ServiceId::builtin(BACKEND_SERVICE),
        "1",
        "Host-bound Objective Workspace API; registration does not authorize operations",
    )
}

struct ObjectiveBackendFeature(Arc<WorkspaceHttpObjectiveBackend>);

impl FeatureModule for ObjectiveBackendFeature {
    fn descriptor(&self) -> FeatureDescriptor {
        FeatureDescriptor::builtin("objective-backend", "Objective Backend")
            .with_provided_service(backend_declaration())
    }

    fn install(&self, context: &mut FeatureInstallContext<'_>) -> Result<(), FeatureInstallError> {
        context
            .services()
            .provide(backend_declaration(), self.0.clone())
    }
}

pub(crate) struct ObjectiveFeature {
    native: Option<(WipNamespaceRoute, Option<ToolPermissionConfig>)>,
}

impl FeatureModule for ObjectiveFeature {
    fn descriptor(&self) -> FeatureDescriptor {
        let mut descriptor = FeatureDescriptor::builtin("objective", "Objective")
            .with_description("Resolved Objective enablement publishes Tools and, in WIP mode, native projection; no separate instruction contribution")
            .with_service_requirement(ServiceRequirement::required(
                ServiceId::builtin(BACKEND_SERVICE),
                "Objective Tools and native WIP share the Host-bound backend",
            ));
        for name in OBJECTIVE_TOOL_NAMES {
            descriptor =
                descriptor.with_tool(ToolDeclaration::new(name, "Workspace Objective operation"));
        }
        if self.native.is_some() {
            descriptor = descriptor.with_wip_mount(WIP_MOUNT);
        }
        descriptor
    }

    fn install(&self, context: &mut FeatureInstallContext<'_>) -> Result<(), FeatureInstallError> {
        let backend = context
            .services()
            .require::<WorkspaceHttpObjectiveBackend>(&ServiceId::builtin(BACKEND_SERVICE))?;
        for definition in workspace_http_objective_tools(backend.client.clone()) {
            let name = definition().0.name;
            context
                .tools()
                .register(ToolContribution::new(name, definition))?;
        }
        if let Some((route, permissions)) = &self.native {
            context.wip().register(WIP_MOUNT, |registry| {
                mount_workspace_http_objective_wip(
                    registry,
                    backend.client.clone(),
                    permissions.clone(),
                    route,
                )
            })?;
        }
        Ok(())
    }
}

/// Trusted composition selects both provider and consumer. Ticket cross-domain
/// coordinates are allocated by the Host even when Objective publication is off.
/// A consumer neither enables another Feature nor constructs authority.
pub(crate) fn compose(
    registry: &mut FeatureRegistryBuilder,
    enabled: bool,
    client: Arc<dyn WorkspaceClient>,
    native: Option<(WipNamespaceRoute, Option<ToolPermissionConfig>)>,
) -> std::io::Result<()> {
    if !enabled {
        return Ok(());
    }
    if !client.is_available() || client.workspace_id().is_none() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "objective tools require Backend Workspace API authority",
        ));
    }
    registry.add_module(ObjectiveBackendFeature(Arc::new(
        WorkspaceHttpObjectiveBackend::new(client),
    )));
    registry.add_module(ObjectiveFeature { native });
    Ok(())
}
