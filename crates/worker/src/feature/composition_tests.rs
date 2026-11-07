// Production Feature composition and installer; only the Workspace transport is
// replaced. Registration must not use it. Spawn execution/Host policy and saved
// bootstrap are covered by their owning suites, not simulated by this fixture.
use super::*;
use crate::feature::builtin::{manage_worker, objective, sub_worker};
use crate::spawn::{registry::SpawnedWorkerRegistry, tool::ParentNotificationTarget};
use crate::wip::WipMountRegistry;
use crate::worker::{
    WorkerWorkspaceContext, WorkspaceClient, WorkspaceClientError, WorkspaceRequest,
    WorkspaceResponse,
};
use manifest::{Scope, SharedScope, WorkerManifest};

#[derive(Debug)]
struct RegistrationClient {
    available: bool,
    identity: bool,
}
impl WorkspaceClient for RegistrationClient {
    fn workspace_id(&self) -> Option<&str> {
        self.identity.then_some("workspace")
    }
    fn kind(&self) -> &str {
        "composition-fixture"
    }
    fn is_available(&self) -> bool {
        self.available
    }
    fn execute(&self, _: WorkspaceRequest) -> Result<WorkspaceResponse, WorkspaceClientError> {
        panic!("registration must not access protected resources or ask for Feature activation")
    }
}

fn manifest() -> WorkerManifest {
    WorkerManifest::from_toml("[worker]\nname='composition-parent'\n[model]\nscheme='anthropic'\nmodel_id='fixture'\n[engine]\n[scope]\nallow=[]").unwrap()
}

fn compose_sub_worker(registry: &mut FeatureRegistryBuilder, enabled: bool, control: bool) {
    let mut manifest = manifest();
    manifest.feature.sub_worker.enabled = enabled;
    let children = SpawnedWorkerRegistry::new_internal(
        manifest.worker.name.clone(),
        SharedScope::new(Scope::empty()),
    );
    if control {
        registry.add_module(manage_worker::sub_worker_control_feature(
            Arc::new(RegistrationClient {
                available: false,
                identity: false,
            }),
            children.clone(),
        ));
    }
    let provider = sub_worker::compose(
        registry,
        manifest,
        WorkerWorkspaceContext::no_workspace(),
        ParentNotificationTarget::Durable(Arc::new(|_| {})),
        std::path::PathBuf::from("unused-runtime"),
        None,
        None,
        children,
        Arc::new(arc_swap::ArcSwap::from(
            crate::prompt::catalog::PromptCatalog::builtins_only().unwrap(),
        )),
    );
    assert_eq!(provider.is_some(), enabled);
}

fn install(
    registry: FeatureRegistryBuilder,
    pending: &mut Vec<ToolDefinition>,
    mounts: &mut WipMountRegistry,
) -> FeatureRegistryInstallReport {
    registry.install_into_pending_with_registered(
        pending,
        &mut HookRegistryBuilder::default(),
        HashMap::new(),
        Some(mounts),
    )
}

#[test]
fn objective_enablement_requires_live_workspace_binding_not_operation_grants() {
    for enabled in [false, true] {
        for available in [false, true] {
            for identity in [false, true] {
                for native in [false, true] {
                    let mut registry = FeatureRegistryBuilder::new();
                    let mut mounts = WipMountRegistry::new();
                    let route = mounts
                        .allocate_namespace("objective", "objectives")
                        .unwrap();
                    let result = objective::composition::compose(
                        &mut registry,
                        enabled,
                        Arc::new(RegistrationClient {
                            available,
                            identity,
                        }),
                        native.then_some((route, None)),
                    );
                    assert_eq!(result.is_ok(), !enabled || (available && identity));
                    if result.is_err() {
                        assert!(registry.is_empty());
                        continue;
                    }
                    let mut pending = Vec::new();
                    let report = install(registry, &mut pending, &mut mounts);
                    assert!(!report.has_errors(), "{}", report.error_message());
                    assert_eq!(pending.len(), if enabled { 7 } else { 0 });
                    assert_eq!(
                        mounts.routes().any(|route| route == "/objectives"),
                        enabled && native
                    );
                    if enabled {
                        let objective = report
                            .reports
                            .iter()
                            .find(|r| r.feature_id == FeatureId::builtin("objective"))
                            .unwrap();
                        assert_eq!(
                            objective.resolved_service_requirements[0].id,
                            ServiceId::builtin("objective.workspace-api")
                        );
                        assert_eq!(objective.installed_wip_mounts.len(), usize::from(native));
                        assert!(objective.installed_instructions.is_empty());
                    }
                }
            }
        }
    }
}

#[test]
fn sub_worker_enablement_publishes_spawn_and_instruction_without_delegation_or_workdir() {
    for enabled in [false, true] {
        let mut registry = FeatureRegistryBuilder::new();
        compose_sub_worker(&mut registry, enabled, enabled);
        let mut pending = Vec::new();
        let report = install(registry, &mut pending, &mut WipMountRegistry::new());
        assert!(!report.has_errors(), "{}", report.error_message());
        assert_eq!(
            pending
                .iter()
                .filter(|tool| tool().0.name == "SubWorkerSpawn")
                .count(),
            usize::from(enabled)
        );
        assert_eq!(
            report
                .installed_instruction_contributions()
                .iter()
                .filter(|instruction| instruction.id
                    == FeatureInstructionId::builtin("worker.orchestration"))
                .count(),
            usize::from(enabled)
        );
        if enabled {
            let feature = report
                .reports
                .iter()
                .find(|r| r.feature_id == FeatureId::builtin("sub-worker"))
                .unwrap();
            assert_eq!(feature.resolved_service_requirements.len(), 2);
            assert!(feature.installed_wip_mounts.is_empty());
        }
    }
}

#[test]
fn missing_control_dependency_prevents_objective_and_spawn_publication() {
    let mut registry = FeatureRegistryBuilder::new();
    let mut mounts = WipMountRegistry::new();
    let route = mounts
        .allocate_namespace("objective", "objectives")
        .unwrap();
    objective::composition::compose(
        &mut registry,
        true,
        Arc::new(RegistrationClient {
            available: true,
            identity: true,
        }),
        Some((route, None)),
    )
    .unwrap();
    compose_sub_worker(&mut registry, true, false);
    let mut pending = vec![dummy_tool("Existing")];
    let report = install(registry, &mut pending, &mut mounts);
    assert!(matches!(
        report.plan_error,
        Some(FeaturePlanError::MissingServiceProvider { .. })
    ));
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0]().0.name, "Existing");
    assert!(mounts.routes().next().is_none());
    assert!(report.installed_instruction_contributions().is_empty());
}

#[test]
fn late_install_failure_rolls_back_migrated_tools_instructions_services_and_wip_only() {
    let client = Arc::new(RegistrationClient {
        available: true,
        identity: true,
    });
    let mut mounts = WipMountRegistry::new();
    let existing = mounts
        .allocate_namespace("objective", "existing-objectives")
        .unwrap();
    objective::mount_workspace_http_objective_wip(&mut mounts, client.clone(), None, &existing)
        .unwrap();
    let before = mounts.routes().map(str::to_owned).collect::<Vec<_>>();
    let mut registry = FeatureRegistryBuilder::new();
    let route = mounts
        .allocate_namespace("objective", "objectives")
        .unwrap();
    objective::composition::compose(&mut registry, true, client, Some((route, None))).unwrap();
    compose_sub_worker(&mut registry, true, true);
    registry.add_module(ToolFeature {
        descriptor: FeatureDescriptor::builtin("duplicate", "Duplicate")
            .with_tool(ToolDeclaration::new("QueryObjective", "late duplicate")),
        contribution_name: "QueryObjective",
        model_visible_name: "QueryObjective",
    });
    let mut pending = vec![dummy_tool("Existing")];
    let report = install(registry, &mut pending, &mut mounts);
    assert!(report.has_errors());
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0]().0.name, "Existing");
    assert_eq!(
        mounts.routes().map(str::to_owned).collect::<Vec<_>>(),
        before
    );
    assert!(report.installed_instruction_contributions().is_empty());
    assert!(
        !report
            .services
            .provides(&ServiceId::builtin("objective.workspace-api"))
    );
    assert!(
        !report
            .services
            .provides(&ServiceId::builtin("sub-worker.spawner"))
    );
    for report in report.reports {
        assert!(!report.installed);
        assert!(report.installed_tools.is_empty());
        assert!(report.installed_wip_mounts.is_empty());
    }
}

struct PartialWipFailure {
    route: crate::wip::WipNamespaceRoute,
    declare: bool,
}
impl FeatureModule for PartialWipFailure {
    fn descriptor(&self) -> FeatureDescriptor {
        let descriptor = FeatureDescriptor::builtin("partial-wip", "Partial WIP");
        if self.declare {
            descriptor.with_wip_mount("partial")
        } else {
            descriptor
        }
    }
    fn install(&self, context: &mut FeatureInstallContext<'_>) -> Result<(), FeatureInstallError> {
        context.wip().register("partial", |mounts| {
            let client = Arc::new(RegistrationClient {
                available: true,
                identity: true,
            });
            objective::mount_workspace_http_objective_wip(
                mounts,
                client.clone(),
                None,
                &self.route,
            )?;
            // Deterministic failure after the first mount changed publication metadata.
            objective::mount_workspace_http_objective_wip(mounts, client, None, &self.route)
        })
    }
}

#[test]
fn partial_or_undeclared_wip_failure_restores_host_mounts_and_discards_successful_siblings() {
    for declare in [false, true] {
        let mut mounts = WipMountRegistry::new();
        let route = mounts
            .allocate_namespace("objective", "partial-objectives")
            .unwrap();
        let mut registry = FeatureRegistryBuilder::new();
        compose_sub_worker(&mut registry, true, true);
        registry.add_module(PartialWipFailure { route, declare });
        let mut pending = vec![dummy_tool("Existing")];
        let report = install(registry, &mut pending, &mut mounts);
        assert!(report.has_errors());
        assert!(mounts.routes().next().is_none());
        assert_eq!(
            mounts.namespace_root("objective"),
            Some("/partial-objectives")
        );
        assert_eq!(pending.len(), 1);
        assert!(report.installed_instruction_contributions().is_empty());
        assert!(
            report
                .reports
                .iter()
                .all(|report| !report.installed && report.installed_wip_mounts.is_empty())
        );
        let failure = report
            .reports
            .iter()
            .find(|report| report.feature_id == FeatureId::builtin("partial-wip"))
            .unwrap();
        if !declare {
            assert!(
                failure
                    .skipped
                    .iter()
                    .any(|skip| skip.kind == FeatureContributionKind::WipMount)
            );
        }
    }
}
