//! Component coverage: real consumer installation, planner and Tool dispatch with contract-only
//! test providers. No concrete provider constructor, Workspace client or spawned-worker registry is
//! passed to a consumer. Production authority/routing is covered by the providers' own suites.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

use super::*;
use crate::feature::builtin::{
    orchestration::orchestration_feature, worker_observation::WorkerObservationFeature,
};
use crate::feature::{
    FeatureDescriptor, FeatureInstallContext, FeatureInstallError, FeatureModule, FeaturePlanError,
    FeatureRegistryBuilder, ServiceDeclaration, ServiceId, ServiceRequirement,
};
use crate::hook::HookRegistryBuilder;
use agen::tool::{Tool, ToolDefinition, ToolExecutionContext};

struct TestServices {
    revoked: AtomicBool,
    captures: AtomicUsize,
    spawns: Mutex<Vec<WorkerLifecycleSpawnRequest>>,
}

impl Default for TestServices {
    fn default() -> Self {
        Self {
            revoked: AtomicBool::new(false),
            captures: AtomicUsize::new(0),
            spawns: Mutex::new(Vec::new()),
        }
    }
}

fn response() -> WorkspaceResponse {
    WorkspaceResponse {
        status: 200,
        body: r#"{"worker_id":"test-worker"}"#.into(),
    }
}

#[async_trait]
impl WorkerLifecycleService for TestServices {
    async fn spawn(
        &self,
        request: WorkerLifecycleSpawnRequest,
    ) -> Result<WorkspaceResponse, WorkspaceClientError> {
        self.spawns.lock().unwrap().push(request);
        Ok(response())
    }
}

impl TicketService for TestServices {
    fn ticket_handoff(&self, ticket_ref: &str) -> Result<TicketHandoff, TicketError> {
        assert_eq!(ticket_ref, "T-788");
        Ok(TicketHandoff {
            id: "ticket-internal-id".into(),
            resource_key: ticket_ref.into(),
            workflow_state: TicketState::Open,
        })
    }
}

#[async_trait]
impl WorkerControlService for TestServices {
    fn workspace_id(&self) -> &str {
        "test-workspace"
    }
    async fn list_workers(&self) -> Result<WorkspaceResponse, WorkspaceClientError> {
        unreachable!("not an observation operation")
    }
    async fn send_input(
        &self,
        _: WorkerControlSubject,
        _: String,
        _: WorkerControlInputKind,
    ) -> Result<WorkspaceResponse, WorkspaceClientError> {
        unreachable!("not a consumer operation")
    }
    async fn stop_worker(
        &self,
        _: WorkerControlSubject,
        _: WorkerControlStopKind,
        _: Option<String>,
    ) -> Result<WorkspaceResponse, WorkspaceClientError> {
        unreachable!("not a consumer operation")
    }
    async fn spawn_worker(
        &self,
        _: WorkerLifecycleSpawnRequest,
    ) -> Result<WorkspaceResponse, WorkspaceClientError> {
        unreachable!("orchestration must use lifecycle")
    }
    fn remove_runtime_worker(
        &self,
        _: &str,
        _: &str,
        _: &str,
    ) -> Result<WorkspaceResponse, WorkspaceClientError> {
        unreachable!("not a consumer operation")
    }
    async fn ensure_permission(
        &self,
        subject: &WorkerObservationSubjectRef,
        permission: &str,
    ) -> Result<(), WorkspaceClientError> {
        assert_eq!(permission, "observe");
        if !self.revoked.load(Ordering::SeqCst)
            && matches!(subject, WorkerObservationSubjectRef::SubWorker { name } if name == "child")
        {
            Ok(())
        } else {
            Err(WorkspaceClientError::Request("not granted".into()))
        }
    }
}

#[async_trait]
impl WorkerObservationProvider for TestServices {
    async fn list_worker_sessions(
        &self,
    ) -> Result<Vec<WorkerObservationSubject>, WorkerObservationError> {
        Ok(Vec::new())
    }
    async fn capture_worker_session(
        &self,
        _: &WorkerObservationSubjectRef,
    ) -> Result<WorkerSessionCapture, WorkerObservationError> {
        self.captures.fetch_add(1, Ordering::SeqCst);
        WorkerSessionCapture::from_log_entries("test-segment", &[])
            .map_err(WorkerObservationError::Unavailable)
    }
}

struct TestProvider {
    id: &'static str,
    services: Arc<TestServices>,
    declarations: Vec<ServiceDeclaration>,
    requirements: Vec<ServiceRequirement>,
    undeclared: bool,
    installs: Arc<AtomicUsize>,
}

impl TestProvider {
    fn new(id: &'static str, services: Arc<TestServices>) -> Self {
        Self {
            id,
            services,
            declarations: vec![
                ServiceDeclaration::new(
                    ServiceId::builtin(WORKER_CONTROL_SERVICE_ID),
                    WORKER_CONTROL_SERVICE_VERSION,
                    "test control",
                ),
                ServiceDeclaration::new(
                    ServiceId::builtin(WORKER_LIFECYCLE_SERVICE_ID),
                    WORKER_LIFECYCLE_SERVICE_VERSION,
                    "test lifecycle",
                ),
                ServiceDeclaration::new(
                    ServiceId::builtin(TICKET_SERVICE_ID),
                    TICKET_SERVICE_VERSION,
                    "test Ticket",
                ),
            ],
            requirements: Vec::new(),
            undeclared: false,
            installs: Arc::new(AtomicUsize::new(0)),
        }
    }
}

impl FeatureModule for TestProvider {
    fn descriptor(&self) -> FeatureDescriptor {
        let mut descriptor = FeatureDescriptor::builtin(self.id, "Contract test provider");
        descriptor.provides_services = self.declarations.clone();
        descriptor.requires_services = self.requirements.clone();
        descriptor
    }
    fn install(&self, context: &mut FeatureInstallContext<'_>) -> Result<(), FeatureInstallError> {
        self.installs.fetch_add(1, Ordering::SeqCst);
        for declaration in &self.declarations {
            match declaration.id.as_str() {
                "builtin:worker.control" => context.services().provide(
                    declaration.clone(),
                    self.services.clone() as Arc<dyn WorkerControlService>,
                )?,
                "builtin:worker.lifecycle" => context.services().provide(
                    declaration.clone(),
                    self.services.clone() as Arc<dyn WorkerLifecycleService>,
                )?,
                "builtin:ticket.authority" => context.services().provide(
                    declaration.clone(),
                    self.services.clone() as Arc<dyn TicketService>,
                )?,
                _ => unreachable!(),
            }
        }
        if self.undeclared {
            context.services().provide(
                ServiceDeclaration::new(ServiceId::builtin("hidden"), "1", "undeclared"),
                Arc::new(()),
            )?;
        }
        Ok(())
    }
}

fn consumers(services: Arc<TestServices>) -> FeatureRegistryBuilder {
    FeatureRegistryBuilder::new()
        .with_module(orchestration_feature())
        .with_module(WorkerObservationFeature::new(services))
}

fn tool(tools: &[ToolDefinition], name: &str) -> Arc<dyn Tool> {
    tools
        .iter()
        .map(|factory| factory())
        .find(|(meta, _)| meta.name == name)
        .unwrap()
        .1
}

#[tokio::test]
async fn consumers_install_and_execute_with_contract_only_test_provider() {
    let services = Arc::new(TestServices::default());
    let mut tools = Vec::new();
    let report = consumers(services.clone())
        .with_module(TestProvider::new("test-provider", services.clone()))
        .install_into_pending(&mut tools, &mut HookRegistryBuilder::default());
    assert!(!report.has_errors(), "{report:?}");
    let result = tool(&tools, "SpawnTicketWorker").execute(
        r#"{"ticket_id":"T-788","runtime_id":"runtime-test","profile":"builtin:coder","initial_request":"implement","orchestrator_assignment_id":"delegation-test"}"#,
        ToolExecutionContext::new("spawn-call", "batch", 0),
    ).await.unwrap();
    assert_eq!(result.summary, "Spawned Worker for Ticket T-788");
    {
        let requests = services.spawns.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].ticket_id.as_deref(), Some("ticket-internal-id"));
        assert_eq!(
            requests[0].orchestrator_assignment_id.as_deref(),
            Some("delegation-test")
        );
        assert_eq!(
            requests[0].operation_id.as_deref(),
            Some("spawn-ticket-worker:ticket-internal-id:spawn-call")
        );
        assert_eq!(
            requests[0].initial_submit,
            vec![
                Segment::Flow {
                    selector: "builtin:coder-review".into()
                },
                Segment::text("implement")
            ]
        );
    }

    let overview = tool(&tools, "ViewSessionOverview");
    let granted = r#"{"subject":{"kind":"sub_worker","name":"child"}}"#;
    overview
        .execute(granted, ToolExecutionContext::direct())
        .await
        .unwrap();
    assert_eq!(services.captures.load(Ordering::SeqCst), 1);
    services.revoked.store(true, Ordering::SeqCst);
    for denied in [
        granted,
        r#"{"subject":{"kind":"sub_worker","name":"non-child"}}"#,
    ] {
        let error = overview
            .execute(denied, ToolExecutionContext::direct())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("not found or is not accessible"));
    }
    assert_eq!(
        services.captures.load(Ordering::SeqCst),
        1,
        "revocation must fence the existing handle before capture"
    );
}

#[test]
fn consumer_plan_rejections_leave_no_partial_installation() {
    for condition in [
        "missing",
        "duplicate",
        "control-version",
        "lifecycle-version",
        "ticket-version",
        "cycle",
        "undeclared",
    ] {
        let services = Arc::new(TestServices::default());
        let mut provider = TestProvider::new("test-provider", services.clone());
        let installs = provider.installs.clone();
        let mut builder = consumers(services.clone());
        match condition {
            "missing" => {}
            "duplicate" => {
                builder.add_module(TestProvider::new("duplicate-provider", services.clone()));
            }
            "control-version" => provider.declarations[0].version = "incompatible".into(),
            "lifecycle-version" => provider.declarations[1].version = "incompatible".into(),
            "ticket-version" => provider.declarations[2].version = "incompatible".into(),
            "cycle" => {
                provider.requirements.push(ServiceRequirement::required(
                    ServiceId::builtin("consumer-output"),
                    "cycle",
                ));
                struct CyclicConsumer;
                impl FeatureModule for CyclicConsumer {
                    fn descriptor(&self) -> FeatureDescriptor {
                        FeatureDescriptor::builtin("cyclic-consumer", "Cycle")
                            .with_service_requirement(ServiceRequirement::required(
                                ServiceId::builtin(WORKER_CONTROL_SERVICE_ID),
                                "control",
                            ))
                            .with_provided_service(ServiceDeclaration::new(
                                ServiceId::builtin("consumer-output"),
                                "1",
                                "output",
                            ))
                    }
                    fn install(
                        &self,
                        _: &mut FeatureInstallContext<'_>,
                    ) -> Result<(), FeatureInstallError> {
                        panic!("cycle must fail preflight")
                    }
                }
                builder.add_module(CyclicConsumer);
            }
            "undeclared" => provider.undeclared = true,
            _ => unreachable!(),
        }
        if condition != "missing" {
            builder.add_module(provider);
        }
        let mut tools = Vec::new();
        let report = builder.install_into_pending(&mut tools, &mut HookRegistryBuilder::default());
        assert!(report.has_errors(), "{condition}: {report:?}");
        assert!(tools.is_empty(), "{condition}");
        assert!(report.services.providers().is_empty(), "{condition}");
        assert!(
            report.reports.iter().all(|entry| !entry.installed),
            "{condition}"
        );
        match (condition, report.plan_error) {
            ("missing", Some(FeaturePlanError::MissingServiceProvider { .. }))
            | ("duplicate", Some(FeaturePlanError::AmbiguousServiceProvider { .. }))
            | (
                "control-version" | "lifecycle-version" | "ticket-version",
                Some(FeaturePlanError::ServiceVersionMismatch { .. }),
            )
            | ("cycle", Some(FeaturePlanError::ServiceDependencyCycle { .. })) => {
                assert_eq!(installs.load(Ordering::SeqCst), 0)
            }
            ("undeclared", None) => {
                assert_eq!(installs.load(Ordering::SeqCst), 1);
                assert!(
                    report
                        .reports
                        .iter()
                        .flat_map(|entry| &entry.diagnostics)
                        .any(|diagnostic| diagnostic
                            .message
                            .contains("undeclared Service contribution"))
                );
            }
            (_, error) => panic!("{condition}: unexpected plan error {error:?}"),
        }
    }
}
