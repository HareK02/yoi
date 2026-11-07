//! SubWorker publication is Worker-lifetime composition. Host-bound spawn
//! inputs retain delegation, command grants, notifications, and child ownership
//! in the existing spawn implementation. No scope or authority is inferred from
//! enablement, Profile identity, a service handle, or attachment presence.

use std::path::PathBuf;
use std::sync::Arc;

use agen::tool::ToolDefinition;
use arc_swap::ArcSwap;
use manifest::WorkerManifest;

use crate::feature::contracts::WORKER_CONTROL_SERVICE_ID;
use crate::feature::{
    FeatureDescriptor, FeatureInstallContext, FeatureInstallError, FeatureInstructionContribution,
    FeatureInstructionDeclaration, FeatureInstructionId, FeatureModule, FeatureRegistryBuilder,
    ServiceDeclaration, ServiceId, ServiceRequirement, ToolContribution, ToolDeclaration,
};
use crate::prompt::catalog::PromptCatalog;
use crate::spawn::registry::SpawnedWorkerRegistry;
use crate::spawn::tool::{ParentNotificationTarget, sub_worker_spawn_tool};
use crate::worker::WorkerWorkspaceContext;

const SPAWNER_SERVICE: &str = "sub-worker.spawner";

fn spawner_declaration() -> ServiceDeclaration {
    ServiceDeclaration::new(
        ServiceId::builtin(SPAWNER_SERVICE),
        "1",
        "Host-bound parent-owned spawner; live delegation and command checks remain per operation",
    )
}

fn instruction() -> FeatureInstructionDeclaration {
    FeatureInstructionDeclaration::new(
        FeatureInstructionId::builtin("worker.orchestration"),
        "common.worker_orchestration",
        "Worker orchestration guidance",
    )
    .expect("static Worker orchestration instruction declaration is valid")
}

struct HostSpawner(ToolDefinition);
struct SubWorkerHostFeature(Arc<HostSpawner>);

impl FeatureModule for SubWorkerHostFeature {
    fn descriptor(&self) -> FeatureDescriptor {
        FeatureDescriptor::builtin("sub-worker-host", "SubWorker Host")
            .with_provided_service(spawner_declaration())
    }
    fn install(&self, context: &mut FeatureInstallContext<'_>) -> Result<(), FeatureInstallError> {
        context
            .services()
            .provide(spawner_declaration(), self.0.clone())
    }
}

struct SubWorkerFeature;

impl FeatureModule for SubWorkerFeature {
    fn descriptor(&self) -> FeatureDescriptor {
        FeatureDescriptor::builtin("sub-worker", "SubWorker")
            .with_description("Resolved SubWorker enablement publishes spawn and orchestration guidance together; no native WIP projection (normal Tool compatibility applies)")
            .with_tool(ToolDeclaration::new("SubWorkerSpawn", "Start a parent-owned Internal SubWorker"))
            .with_instruction(instruction())
            .with_service_requirement(ServiceRequirement::required(ServiceId::builtin(SPAWNER_SERVICE),
                "Only the Host supplies live spawn inputs"))
            .with_service_requirement(ServiceRequirement::required(ServiceId::builtin(WORKER_CONTROL_SERVICE_ID),
                "Spawned children belong to the explicitly composed Worker control backend"))
    }
    fn install(&self, context: &mut FeatureInstallContext<'_>) -> Result<(), FeatureInstallError> {
        let spawner = context
            .services()
            .require::<HostSpawner>(&ServiceId::builtin(SPAWNER_SERVICE))?;
        context
            .tools()
            .register(ToolContribution::new("SubWorkerSpawn", spawner.0.clone()))?;
        context
            .instructions()
            .register(FeatureInstructionContribution::new(instruction()))
    }
}

/// Live handles are supplied by the Host. This product composition function
/// consumes the resolved saved manifest identically for create and restore.
#[allow(clippy::too_many_arguments)]
pub(crate) fn compose(
    registry: &mut FeatureRegistryBuilder,
    manifest: WorkerManifest,
    workspace_context: WorkerWorkspaceContext,
    parent_notifications: ParentNotificationTarget,
    runtime_base: PathBuf,
    workspace_root: Option<PathBuf>,
    broker: Option<workdir::WorkdirToolBrokerRouter>,
    children: Arc<SpawnedWorkerRegistry>,
    prompts: Arc<ArcSwap<PromptCatalog>>,
) -> Option<Arc<dyn crate::feature::contracts::WorkerObservationProvider>> {
    if !manifest.feature.sub_worker.enabled {
        return None;
    }
    let tool = sub_worker_spawn_tool(
        manifest.worker.name.clone(),
        workspace_context,
        parent_notifications,
        runtime_base,
        workspace_root.unwrap_or_else(|| PathBuf::from("/")),
        broker,
        children.clone(),
        manifest,
        prompts,
    );
    registry.add_module(SubWorkerHostFeature(Arc::new(HostSpawner(tool))));
    registry.add_module(SubWorkerFeature);
    Some(Arc::new(
        super::worker_observation::SpawnedSubWorkerObservationProvider::new(children),
    ))
}
