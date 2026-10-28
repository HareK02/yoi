//! Rechecks frozen Subject Job input before each model request, including restore.
use std::sync::Arc;

use async_trait::async_trait;

use crate::feature::{
    FeatureDescriptor, FeatureHookPoint, FeatureInstallContext, FeatureInstallError, FeatureModule,
    HookDeclaration,
};
use crate::hook::{
    Hook, HookError, HookErrorCategory, HookExecutionPolicy, HookPreRequestAction, PreLlmRequest,
    PreRequestContext,
};
use crate::worker::WorkspaceClient;

#[derive(Clone)]
pub(crate) struct SubjectJobInputFeature {
    client: Arc<dyn WorkspaceClient>,
    operation: &'static str,
}

impl SubjectJobInputFeature {
    pub(crate) fn new(client: Arc<dyn WorkspaceClient>, operation: &'static str) -> Self {
        Self { client, operation }
    }
}

impl FeatureModule for SubjectJobInputFeature {
    fn descriptor(&self) -> FeatureDescriptor {
        FeatureDescriptor::builtin("subject-job-input", "Subject Job input").with_hook(
            HookDeclaration::new("supply", FeatureHookPoint::PreLlmRequest),
        )
    }
    fn install(&self, context: &mut FeatureInstallContext<'_>) -> Result<(), FeatureInstallError> {
        context.hooks().add_pre_llm_request(
            "supply",
            HookExecutionPolicy::fail_closed(),
            self.clone(),
        )
    }
}

#[async_trait]
impl Hook<PreLlmRequest> for SubjectJobInputFeature {
    async fn call(&self, _: &PreRequestContext) -> Result<HookPreRequestAction, HookError> {
        self.client
            .authorize_subject_operation(self.operation)
            .map_err(|error| {
                HookError::new(
                    HookErrorCategory::Dependency,
                    format!(
                        "protected Job input supply rejected for {}: {error}",
                        self.operation
                    ),
                )
            })?;
        Ok(HookPreRequestAction::Continue)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feature::FeatureRegistryBuilder;
    use crate::hook::{HookRegistryBuilder, PreRequestInfo};
    use crate::worker::{WorkspaceClientError, WorkspaceRequest, WorkspaceResponse};
    use agen::llm_client::{ClientError, LlmClient, Request, ResponseStream};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    #[derive(Clone)]
    struct NoModel;
    #[async_trait]
    impl LlmClient for NoModel {
        fn clone_boxed(&self) -> Box<dyn LlmClient> {
            Box::new(self.clone())
        }
        async fn stream(&self, _: Request) -> Result<ResponseStream, ClientError> {
            panic!("registry fixture must not dispatch a provider request")
        }
    }

    #[derive(Debug)]
    struct Host {
        allowed: AtomicBool,
        calls: AtomicUsize,
        operation: &'static str,
    }
    impl WorkspaceClient for Host {
        fn workspace_id(&self) -> Option<&str> {
            Some("workspace")
        }
        fn kind(&self) -> &str {
            "job-input-fixture"
        }
        fn is_available(&self) -> bool {
            true
        }
        fn execute(
            &self,
            request: WorkspaceRequest,
        ) -> Result<WorkspaceResponse, WorkspaceClientError> {
            assert_eq!(request.path, "/api/w/workspace/authorization/bound-check");
            let input: server_api::authorization::BoundOperationAuthorizationRequest =
                serde_json::from_str(request.body.as_deref().unwrap()).unwrap();
            assert_eq!(input.operation, self.operation);
            assert_eq!(
                input.target,
                server_api::authorization::BoundAuthorizationTarget::Subject {}
            );
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(WorkspaceResponse {
                status: if self.allowed.load(Ordering::SeqCst) { 200 } else { 403 },
                body: r#"{"scope":{"kind":"subject","workspace_id":"workspace","subject_id":"bound-subject"}}"#.into(),
            })
        }
    }

    #[tokio::test]
    async fn direct_install_and_restore_recheck_each_frozen_job_input_supply() {
        for operation in ["memory.consolidation.supply", "journal.job.input.supply"] {
            let host = Arc::new(Host {
                allowed: AtomicBool::new(true),
                calls: AtomicUsize::new(0),
                operation,
            });
            let feature = SubjectJobInputFeature::new(host.clone(), operation);
            for restored in [false, true] {
                let mut engine = agen::Engine::new(NoModel);
                let mut hooks = HookRegistryBuilder::new();
                let mut registry = FeatureRegistryBuilder::new().with_module(feature.clone());
                if restored {
                    registry =
                        registry.with_installation(crate::feature::FeatureInstallation::Restore);
                }
                let report = registry.install_into_engine(&mut engine, &mut hooks);
                assert!(report.reports[0].installed);
                let hooks = hooks.build();
                let context = PreRequestContext::new(
                    PreRequestInfo {
                        item_count: 1,
                        estimated_tokens: None,
                        turn_index: 0,
                        tool_calls_this_turn: 0,
                    },
                    None,
                );
                host.allowed.store(true, Ordering::SeqCst);
                assert!(matches!(
                    hooks.pre_llm_request[0].call(&context).await.unwrap(),
                    HookPreRequestAction::Continue
                ));
                host.allowed.store(false, Ordering::SeqCst);
                let rejected = hooks.pre_llm_request[0].call(&context).await.unwrap_err();
                assert!(rejected.source.diagnostic.contains("403"));
                assert_eq!(rejected.policy, crate::hook::HookFailurePolicy::FailClosed);
            }
            assert_eq!(host.calls.load(Ordering::SeqCst), 4);
        }
    }
}
