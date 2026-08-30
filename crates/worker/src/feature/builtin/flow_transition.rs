use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use agen::llm_client::client::LlmClient;
use agen::tool::{Tool, ToolDefinition, ToolError, ToolMeta, ToolOutput};
use async_trait::async_trait;
use flow::{
    ConditionVerdict, FlowTransitionAttempt, FlowTransitionResolution, FlowVerifierOutcome,
    TransitionConditionResult, TransitionId,
};
use manifest::{Scope, WorkerManifest};
use schemars::JsonSchema;
use serde::Deserialize;
use session_store::{SegmentId, SessionId, Store, collect_state};
use uuid::Uuid;

use crate::feature::builtin::session_explore::{SessionExploreFeature, SessionExploreState};
use crate::feature::{
    FeatureDescriptor, FeatureInstallContext, FeatureInstallError, FeatureModule,
    FeatureRegistryBuilder, ToolContribution, ToolDeclaration,
};
use crate::internal_worker::{
    InternalWorkerAuthority, InternalWorkerIdentity, InternalWorkerSpec, run_internal_worker,
};
use crate::session_capture::SessionCapture;
use crate::worker::{WorkerFilesystemAuthority, WorkerRunResult, WorkerWorkspaceContext};

const REQUEST_FLOW_TRANSITION_DESCRIPTION: &str = "Request evaluation of the current Flow state's outgoing conditions. The host selects the active Flow instance and captures the current state; the caller supplies only a concise reason.";
const FINISH_FLOW_VERIFICATION_DESCRIPTION: &str = "Finish one bounded Flow verification attempt with exactly one verdict for every captured outgoing transition.";

#[async_trait]
pub trait FlowCoordinatorClient: Send + Sync {
    async fn begin_transition(
        &self,
        attempt_id: &str,
        reason: &str,
    ) -> Result<FlowTransitionAttempt, String>;

    async fn resolve_transition(
        &self,
        attempt_id: &str,
        outcome: FlowVerifierOutcome,
    ) -> Result<FlowTransitionResolution, String>;
}

#[async_trait]
pub trait FlowConditionVerifier: Send + Sync {
    async fn verify(&self, attempt: &FlowTransitionAttempt) -> FlowVerifierOutcome;
}

pub trait FlowRuntimeStateCommitter: Send + Sync {
    fn commit(&self, state: &flow::FlowRuntimeState) -> Result<(), String>;
}

/// Runtime-local coordinator for Flow state durably owned by one Worker.
pub struct RuntimeFlowCoordinatorClient {
    state: Arc<std::sync::Mutex<Option<flow::FlowRuntimeState>>>,
    committer: Arc<dyn FlowRuntimeStateCommitter>,
}

impl RuntimeFlowCoordinatorClient {
    pub fn new(
        state: Arc<std::sync::Mutex<Option<flow::FlowRuntimeState>>>,
        committer: Arc<dyn FlowRuntimeStateCommitter>,
    ) -> Self {
        Self { state, committer }
    }
}

#[async_trait]
impl FlowCoordinatorClient for RuntimeFlowCoordinatorClient {
    async fn begin_transition(
        &self,
        attempt_id: &str,
        reason: &str,
    ) -> Result<FlowTransitionAttempt, String> {
        let mut guard = self.state.lock().expect("flow runtime state poisoned");
        let current = guard
            .as_ref()
            .ok_or_else(|| "Worker has no active Flow instance".to_string())?;
        if let Some(attempt) = &current.active_attempt {
            return Ok(attempt.clone());
        }
        let mut updated = current.clone();
        let attempt = updated
            .begin_or_recover_transition(attempt_id.to_string(), reason.to_string())
            .map_err(|error| error.to_string())?;
        self.committer.commit(&updated)?;
        *guard = Some(updated);
        Ok(attempt)
    }

    async fn resolve_transition(
        &self,
        attempt_id: &str,
        outcome: FlowVerifierOutcome,
    ) -> Result<FlowTransitionResolution, String> {
        let mut guard = self.state.lock().expect("flow runtime state poisoned");
        let current = guard
            .as_ref()
            .ok_or_else(|| "Worker has no active Flow instance".to_string())?;
        let mut updated = current.clone();
        let resolution = updated
            .resolve_active_transition(attempt_id, outcome)
            .map_err(|error| error.to_string())?;
        self.committer.commit(&updated)?;
        *guard = Some(updated);
        Ok(resolution)
    }
}

#[derive(Clone)]
pub struct FlowTransitionFeature {
    state: FlowTransitionState,
}

#[derive(Clone)]
pub struct FlowTransitionState {
    coordinator: Arc<dyn FlowCoordinatorClient>,
    verifier: Arc<dyn FlowConditionVerifier>,
    in_flight: Arc<AtomicBool>,
}

impl FlowTransitionState {
    pub fn new(
        coordinator: Arc<dyn FlowCoordinatorClient>,
        verifier: Arc<dyn FlowConditionVerifier>,
    ) -> Self {
        Self {
            coordinator,
            verifier,
            in_flight: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl FlowTransitionFeature {
    pub fn new(state: FlowTransitionState) -> Self {
        Self { state }
    }
}

impl FeatureModule for FlowTransitionFeature {
    fn descriptor(&self) -> FeatureDescriptor {
        FeatureDescriptor::builtin("flow-transition", "Flow Transition")
            .with_description(
                "Request host-authorized transitions for the Worker's active Flow instance.",
            )
            .with_tool(ToolDeclaration::new(
                "RequestFlowTransition",
                REQUEST_FLOW_TRANSITION_DESCRIPTION,
            ))
    }

    fn install(&self, context: &mut FeatureInstallContext<'_>) -> Result<(), FeatureInstallError> {
        context.tools().register(ToolContribution::new(
            "RequestFlowTransition",
            request_flow_transition_definition(self.state.clone()),
        ))?;
        Ok(())
    }
}

fn request_flow_transition_definition(state: FlowTransitionState) -> ToolDefinition {
    Arc::new(move || {
        let schema = serde_json::to_value(schemars::schema_for!(RequestFlowTransitionParams))
            .unwrap_or_else(|_| serde_json::json!({}));
        let meta = ToolMeta::new("RequestFlowTransition")
            .description(REQUEST_FLOW_TRANSITION_DESCRIPTION)
            .input_schema(schema);
        let tool: Arc<dyn Tool> = Arc::new(RequestFlowTransitionTool {
            state: state.clone(),
        });
        (meta, tool)
    })
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RequestFlowTransitionParams {
    reason: String,
}

struct RequestFlowTransitionTool {
    state: FlowTransitionState,
}

struct InFlightGuard(Arc<AtomicBool>);

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

#[async_trait]
impl Tool for RequestFlowTransitionTool {
    async fn execute(
        &self,
        input_json: &str,
        _context: agen::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let params: RequestFlowTransitionParams =
            serde_json::from_str(input_json).map_err(|error| {
                ToolError::InvalidArgument(format!("invalid RequestFlowTransition input: {error}"))
            })?;
        if params.reason.trim().is_empty() {
            return Err(ToolError::InvalidArgument(
                "RequestFlowTransition reason must not be empty".to_string(),
            ));
        }
        if self
            .state
            .in_flight
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(ToolError::ExecutionFailed(
                "a Flow transition verification is already in progress".to_string(),
            ));
        }
        let _guard = InFlightGuard(self.state.in_flight.clone());
        let attempt_id = Uuid::now_v7().to_string();
        let attempt = self
            .state
            .coordinator
            .begin_transition(&attempt_id, params.reason.trim())
            .await
            .map_err(|error| {
                ToolError::ExecutionFailed(format!("begin Flow transition: {error}"))
            })?;
        let active_attempt_id = attempt.attempt_id.clone();
        let outcome = self.state.verifier.verify(&attempt).await;
        let resolution = self
            .state
            .coordinator
            .resolve_transition(&active_attempt_id, outcome)
            .await
            .map_err(|error| {
                ToolError::ExecutionFailed(format!("resolve Flow transition: {error}"))
            })?;
        let content = serde_json::to_string_pretty(&resolution).map_err(|error| {
            ToolError::ExecutionFailed(format!("serialize Flow transition result: {error}"))
        })?;
        let summary = match (&resolution.entered_state, &resolution.rejection) {
            (Some(state), _) => format!("Flow entered state {state}."),
            (_, Some(rejection)) => {
                format!("Flow transition was rejected: {}", rejection.message)
            }
            _ => "Flow transition did not enter a state.".to_string(),
        };
        Ok(ToolOutput {
            summary,
            content: Some(content),
            attachments: Vec::new(),
        })
    }
}

#[derive(Clone)]
pub(crate) struct FinishFlowVerificationState {
    expected: Arc<BTreeSet<TransitionId>>,
    result: Arc<Mutex<Option<Vec<TransitionConditionResult>>>>,
}

impl FinishFlowVerificationState {
    pub(crate) fn new(attempt: &FlowTransitionAttempt) -> Self {
        Self {
            expected: Arc::new(
                attempt
                    .transitions
                    .iter()
                    .map(|transition| transition.transition_id.clone())
                    .collect(),
            ),
            result: Arc::new(Mutex::new(None)),
        }
    }

    pub(crate) fn take(&self) -> Option<Vec<TransitionConditionResult>> {
        self.result.lock().ok()?.take()
    }
}

#[derive(Clone)]
pub(crate) struct FinishFlowVerificationFeature {
    state: FinishFlowVerificationState,
}

impl FinishFlowVerificationFeature {
    pub(crate) fn new(state: FinishFlowVerificationState) -> Self {
        Self { state }
    }
}

impl FeatureModule for FinishFlowVerificationFeature {
    fn descriptor(&self) -> FeatureDescriptor {
        FeatureDescriptor::builtin("flow-verification-finish", "Flow Verification Finish")
            .with_description("Submit the complete structured Flow condition verdict set.")
            .with_tool(ToolDeclaration::new(
                "FinishFlowVerification",
                FINISH_FLOW_VERIFICATION_DESCRIPTION,
            ))
    }

    fn install(&self, context: &mut FeatureInstallContext<'_>) -> Result<(), FeatureInstallError> {
        context.tools().register(ToolContribution::new(
            "FinishFlowVerification",
            finish_flow_verification_definition(self.state.clone()),
        ))?;
        Ok(())
    }
}

fn finish_flow_verification_definition(state: FinishFlowVerificationState) -> ToolDefinition {
    Arc::new(move || {
        let schema = serde_json::to_value(schemars::schema_for!(FinishFlowVerificationParams))
            .unwrap_or_else(|_| serde_json::json!({}));
        let meta = ToolMeta::new("FinishFlowVerification")
            .description(FINISH_FLOW_VERIFICATION_DESCRIPTION)
            .input_schema(schema);
        let tool: Arc<dyn Tool> = Arc::new(FinishFlowVerificationTool {
            state: state.clone(),
        });
        (meta, tool)
    })
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct FinishFlowVerificationParams {
    results: Vec<FinishTransitionResult>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct FinishTransitionResult {
    transition_id: String,
    verdict: String,
    rationale: String,
}

struct FinishFlowVerificationTool {
    state: FinishFlowVerificationState,
}

#[async_trait]
impl Tool for FinishFlowVerificationTool {
    async fn execute(
        &self,
        input_json: &str,
        _context: agen::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let params: FinishFlowVerificationParams =
            serde_json::from_str(input_json).map_err(|error| {
                ToolError::InvalidArgument(format!("invalid FinishFlowVerification input: {error}"))
            })?;
        let mut seen = BTreeSet::new();
        let mut results = Vec::with_capacity(params.results.len());
        for result in params.results {
            let transition_id = self
                .state
                .expected
                .iter()
                .find(|expected| expected.as_str() == result.transition_id)
                .cloned()
                .ok_or_else(|| {
                    ToolError::InvalidArgument(format!(
                        "transition_id {:?} is not part of this attempt",
                        result.transition_id
                    ))
                })?;
            if !seen.insert(transition_id.clone()) {
                return Err(ToolError::InvalidArgument(format!(
                    "transition_id {:?} was returned more than once",
                    result.transition_id
                )));
            }
            if result.rationale.trim().is_empty() {
                return Err(ToolError::InvalidArgument(format!(
                    "rationale for transition {:?} must not be empty",
                    result.transition_id
                )));
            }
            let verdict = match result.verdict.as_str() {
                "met" => ConditionVerdict::Met,
                "not_met" => ConditionVerdict::NotMet,
                "indeterminate" => ConditionVerdict::Indeterminate,
                other => {
                    return Err(ToolError::InvalidArgument(format!(
                        "invalid verdict {other:?}; expected met, not_met, or indeterminate"
                    )));
                }
            };
            results.push(TransitionConditionResult {
                transition_id,
                verdict,
                rationale: result.rationale,
            });
        }
        if seen != *self.state.expected {
            let missing = self
                .state
                .expected
                .difference(&seen)
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            return Err(ToolError::InvalidArgument(format!(
                "FinishFlowVerification is missing transition verdict(s): {missing}"
            )));
        }
        let mut slot = self.state.result.lock().map_err(|_| {
            ToolError::ExecutionFailed("Flow verification result state is unavailable".to_string())
        })?;
        if slot.is_some() {
            return Err(ToolError::ExecutionFailed(
                "FinishFlowVerification was already completed".to_string(),
            ));
        }
        *slot = Some(results);
        Ok(ToolOutput {
            summary: "Recorded complete Flow verification verdicts.".to_string(),
            content: Some("{\"accepted\":true}".to_string()),
            attachments: Vec::new(),
        })
    }
}

#[derive(Clone)]
pub(crate) struct StoreFlowParentCapture<St>
where
    St: Store + Clone,
{
    store: St,
    session_id: SessionId,
    segment_id: SegmentId,
}

impl<St> StoreFlowParentCapture<St>
where
    St: Store + Clone,
{
    pub(crate) fn new(store: St, session_id: SessionId, segment_id: SegmentId) -> Self {
        Self {
            store,
            session_id,
            segment_id,
        }
    }

    fn capture(&self) -> Result<SessionCapture, String> {
        let entries = self
            .store
            .read_all(self.session_id, self.segment_id)
            .map_err(|error| format!("read committed parent session: {error}"))?;
        let restored = collect_state(&entries);
        Ok(SessionCapture::new(
            self.segment_id.to_string(),
            restored.history,
        ))
    }
}

pub(crate) struct WorkerBackedFlowVerifier<C, St>
where
    C: LlmClient + Clone,
    St: Store + Clone,
{
    client: C,
    manifest: WorkerManifest,
    capture: StoreFlowParentCapture<St>,
    read_only_tools: Vec<ToolDefinition>,
}

impl<C, St> WorkerBackedFlowVerifier<C, St>
where
    C: LlmClient + Clone,
    St: Store + Clone,
{
    pub(crate) fn new(
        client: C,
        manifest: WorkerManifest,
        capture: StoreFlowParentCapture<St>,
        read_only_tools: Vec<ToolDefinition>,
    ) -> Self {
        Self {
            client,
            manifest,
            capture,
            read_only_tools,
        }
    }
}

#[async_trait]
impl<C, St> FlowConditionVerifier for WorkerBackedFlowVerifier<C, St>
where
    C: LlmClient + Clone + Send + Sync + 'static,
    St: Store + Clone + Send + Sync + 'static,
{
    async fn verify(&self, attempt: &FlowTransitionAttempt) -> FlowVerifierOutcome {
        let capture = match self.capture.capture() {
            Ok(capture) => capture,
            Err(message) => return FlowVerifierOutcome::Failed { message },
        };
        let finish_state = FinishFlowVerificationState::new(attempt);
        let mut features = FeatureRegistryBuilder::new()
            .with_module(SessionExploreFeature::new(SessionExploreState::new(
                capture,
            )))
            .with_module(FinishFlowVerificationFeature::new(finish_state.clone()));
        if !self.read_only_tools.is_empty() {
            features = features.with_module(ReadOnlyFlowWorkdirFeature::new(
                self.read_only_tools.clone(),
            ));
        }

        let catalog = match crate::PromptCatalog::builtins_only() {
            Ok(catalog) => catalog,
            Err(error) => {
                return FlowVerifierOutcome::Failed {
                    message: format!("load Flow verifier prompt catalog: {error}"),
                };
            }
        };
        let prompt = match catalog.flow_verifier_system() {
            Ok(prompt) => prompt,
            Err(error) => {
                return FlowVerifierOutcome::Failed {
                    message: format!("render Flow verifier prompt: {error}"),
                };
            }
        };
        let manifest = self.manifest.clone();
        let input = match serde_json::to_string(&serde_json::json!({
            "current_state": attempt.from_state,
            "reason": attempt.reason,
            "transitions": attempt.transitions,
        })) {
            Ok(input) => input,
            Err(error) => {
                return FlowVerifierOutcome::Failed {
                    message: format!("encode Flow verifier input: {error}"),
                };
            }
        };
        let spec = InternalWorkerSpec {
            identity: InternalWorkerIdentity {
                kind: "flow-verifier",
                run_id: Uuid::now_v7(),
            },
            manifest,
            client: self.client.clone_boxed(),
            system_prompt: prompt,
            input,
            cache_key: Some(format!(
                "flow:{}:{}",
                attempt.instance_id, attempt.checked_state_revision
            )),
            max_turns: Some(12),
            engine_configurator: None,
            features,
            required_tools: &[
                "ShowOverview",
                "SearchEntries",
                "ReadEntry",
                "FinishFlowVerification",
            ],
            authority: InternalWorkerAuthority {
                workspace: WorkerWorkspaceContext::no_workspace(),
                filesystem: WorkerFilesystemAuthority::None,
                scope: Scope::empty(),
                workdir_session: None,
            },
        };
        match run_internal_worker(spec).await {
            Ok(result) if result.lifecycle == WorkerRunResult::RolledBack => {
                FlowVerifierOutcome::Cancelled
            }
            Ok(_) => match finish_state.take() {
                Some(results) => FlowVerifierOutcome::Completed { results },
                None => FlowVerifierOutcome::Failed {
                    message: "Flow verifier finished without FinishFlowVerification".to_string(),
                },
            },
            Err(error) => FlowVerifierOutcome::Failed {
                message: error.source.to_string(),
            },
        }
    }
}

#[derive(Clone)]
struct ReadOnlyFlowWorkdirFeature {
    tools: Vec<ToolDefinition>,
}

impl ReadOnlyFlowWorkdirFeature {
    fn new(tools: Vec<ToolDefinition>) -> Self {
        Self { tools }
    }
}

impl FeatureModule for ReadOnlyFlowWorkdirFeature {
    fn descriptor(&self) -> FeatureDescriptor {
        let mut descriptor =
            FeatureDescriptor::builtin("flow-read-only-workdir", "Flow read-only Workdir")
                .with_description(
                    "Read-only Workdir evidence tools for the internal Flow verifier.",
                );
        for definition in &self.tools {
            let (meta, _) = definition();
            descriptor = descriptor.with_tool(ToolDeclaration::new(meta.name, meta.description));
        }
        descriptor
    }

    fn install(&self, context: &mut FeatureInstallContext<'_>) -> Result<(), FeatureInstallError> {
        for definition in &self.tools {
            let (meta, _) = definition();
            context
                .tools()
                .register(ToolContribution::new(meta.name, definition.clone()))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;

    use agen::llm_client::client::{LlmClient, ResponseStream};
    use agen::llm_client::error::ClientError;
    use agen::llm_client::event::{
        BlockDelta, BlockMetadata, BlockStart, BlockStop, BlockType, DeltaContent,
        Event as LlmEvent, StopReason,
    };
    use agen::llm_client::types::Request;
    use agen::tool::ToolExecutionContext;
    use flow::{FlowAttemptStatus, FlowTransitionRejection, StateId, TransitionCheckSnapshot};
    use futures::stream;
    use manifest::WorkerManifest;
    use protocol::Segment;
    use session_store::{LogEntry, SegmentId, SessionId, Store};

    use super::*;
    use crate::feature::{FeatureRegistryBuilder, HookRegistryBuilder};

    fn attempt(id: &str) -> FlowTransitionAttempt {
        FlowTransitionAttempt {
            attempt_id: id.to_string(),
            instance_id: "instance-1".to_string(),
            definition_revision: 1,
            definition_digest: "sha256:test".to_string(),
            checked_state_revision: 0,
            from_state: StateId::new("work").unwrap(),
            reason: "ready".to_string(),
            transitions: vec![
                TransitionCheckSnapshot {
                    transition_id: TransitionId::new("done").unwrap(),
                    target: StateId::new("done").unwrap(),
                    condition: "done".to_string(),
                    synthetic: false,
                },
                TransitionCheckSnapshot {
                    transition_id: TransitionId::new("cancel").unwrap(),
                    target: StateId::new("cancelled").unwrap(),
                    condition: "exceptional".to_string(),
                    synthetic: true,
                },
            ],
            status: FlowAttemptStatus::Verifying,
        }
    }

    #[derive(Default)]
    struct RecordingFlowStateCommitter {
        states: Mutex<Vec<flow::FlowRuntimeState>>,
    }

    impl FlowRuntimeStateCommitter for RecordingFlowStateCommitter {
        fn commit(&self, state: &flow::FlowRuntimeState) -> Result<(), String> {
            self.states.lock().unwrap().push(state.clone());
            Ok(())
        }
    }

    fn runtime_flow_state() -> flow::FlowRuntimeState {
        let definition = flow::compile_flow_source(
            r#"{
                schema_version = 1;
                name = "test-flow";
                initial = "work";
                states = {
                    work = {
                        instructions = "Work.";
                        transitions = {
                            done = { target = "done"; condition = "Complete."; };
                        };
                    };
                    done = { instructions = "Done."; terminal = true; };
                };
            }"#,
        )
        .unwrap();
        flow::FlowRuntimeState::start(
            &flow::ResolvedFlowSource {
                selector: "workspace:test-flow".parse().unwrap(),
                workspace_id: "workspace-1".to_string(),
                flow_id: "flow-1".to_string(),
                revision: 1,
                content_digest: definition.content_digest.clone(),
                definition,
            },
            "instance-1",
        )
        .unwrap()
        .0
    }

    #[tokio::test]
    async fn runtime_coordinator_persists_and_recovers_active_attempt() {
        let state = Arc::new(Mutex::new(Some(runtime_flow_state())));
        let committer = Arc::new(RecordingFlowStateCommitter::default());
        let coordinator = RuntimeFlowCoordinatorClient::new(state.clone(), committer.clone());

        let attempt = coordinator
            .begin_transition("attempt-1", "ready")
            .await
            .unwrap();
        assert_eq!(attempt.attempt_id, "attempt-1");
        assert_eq!(committer.states.lock().unwrap().len(), 1);

        let restored = RuntimeFlowCoordinatorClient::new(state.clone(), committer.clone());
        let recovered = restored
            .begin_transition("attempt-2", "retry after restore")
            .await
            .unwrap();
        assert_eq!(recovered.attempt_id, "attempt-1");
        assert_eq!(committer.states.lock().unwrap().len(), 1);

        let results = recovered
            .transitions
            .iter()
            .map(|transition| TransitionConditionResult {
                transition_id: transition.transition_id.clone(),
                verdict: if transition.synthetic {
                    ConditionVerdict::NotMet
                } else {
                    ConditionVerdict::Met
                },
                rationale: "complete".to_string(),
            })
            .collect();
        let resolution = restored
            .resolve_transition("attempt-1", FlowVerifierOutcome::Completed { results })
            .await
            .unwrap();
        assert!(resolution.entered_state.is_some());
        assert_eq!(committer.states.lock().unwrap().len(), 2);
        assert_eq!(
            state.lock().unwrap().as_ref().unwrap().instance.status,
            flow::FlowInstanceStatus::Completed
        );
    }

    struct FakeCoordinator {
        begin_count: AtomicUsize,
        resolve_count: AtomicUsize,
    }

    #[async_trait]
    impl FlowCoordinatorClient for FakeCoordinator {
        async fn begin_transition(
            &self,
            attempt_id: &str,
            _reason: &str,
        ) -> Result<FlowTransitionAttempt, String> {
            self.begin_count.fetch_add(1, Ordering::SeqCst);
            Ok(attempt(attempt_id))
        }

        async fn resolve_transition(
            &self,
            attempt_id: &str,
            outcome: FlowVerifierOutcome,
        ) -> Result<FlowTransitionResolution, String> {
            self.resolve_count.fetch_add(1, Ordering::SeqCst);
            let FlowVerifierOutcome::Completed { results } = outcome else {
                return Err("unexpected verifier outcome".to_string());
            };
            Ok(FlowTransitionResolution {
                attempt: attempt(attempt_id),
                entered_state: None,
                state_instructions: None,
                rejection: Some(FlowTransitionRejection {
                    code: flow::FlowRejectionCode::NoConditionMet,
                    message: format!("{} condition(s) evaluated", results.len()),
                }),
                events: Vec::new(),
            })
        }
    }

    struct FakeVerifier;

    #[async_trait]
    impl FlowConditionVerifier for FakeVerifier {
        async fn verify(&self, attempt: &FlowTransitionAttempt) -> FlowVerifierOutcome {
            FlowVerifierOutcome::Completed {
                results: attempt
                    .transitions
                    .iter()
                    .map(|transition| TransitionConditionResult {
                        transition_id: transition.transition_id.clone(),
                        verdict: ConditionVerdict::NotMet,
                        rationale: "not met".to_string(),
                    })
                    .collect(),
            }
        }
    }

    #[tokio::test]
    async fn request_tool_owns_attempt_identity_and_runs_one_verifier() {
        let coordinator = Arc::new(FakeCoordinator {
            begin_count: AtomicUsize::new(0),
            resolve_count: AtomicUsize::new(0),
        });
        let state = FlowTransitionState::new(coordinator.clone(), Arc::new(FakeVerifier));
        let definition = request_flow_transition_definition(state);
        let (_, tool) = definition();
        let output = tool
            .execute(
                r#"{"reason":"implementation and validation are complete"}"#,
                ToolExecutionContext::default(),
            )
            .await
            .unwrap();
        assert!(output.summary.contains("rejected"));
        assert_eq!(coordinator.begin_count.load(Ordering::SeqCst), 1);
        assert_eq!(coordinator.resolve_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn finish_tool_requires_exact_transition_set() {
        let attempt = attempt("attempt-1");
        let state = FinishFlowVerificationState::new(&attempt);
        let definition = finish_flow_verification_definition(state.clone());
        let (_, tool) = definition();
        let error = tool
            .execute(
                r#"{"results":[{"transition_id":"done","verdict":"met","rationale":"evidence"}]}"#,
                ToolExecutionContext::default(),
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("missing transition"));
        assert!(state.take().is_none());
    }

    #[derive(Clone)]
    struct FinishVerifierClient {
        calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl LlmClient for FinishVerifierClient {
        async fn stream(&self, _request: Request) -> Result<ResponseStream, ClientError> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            if call == 0 {
                let input = serde_json::json!({
                    "results": [
                        {
                            "transition_id": "done",
                            "verdict": "met",
                            "rationale": "the captured implementation evidence is complete"
                        },
                        {
                            "transition_id": "cancel",
                            "verdict": "not_met",
                            "rationale": "no exceptional cancellation condition exists"
                        }
                    ]
                })
                .to_string();
                Ok(Box::pin(stream::iter(vec![
                    Ok(LlmEvent::BlockStart(BlockStart {
                        index: 0,
                        block_type: BlockType::ToolUse,
                        metadata: BlockMetadata::ToolUse {
                            id: "finish-flow".to_string(),
                            name: "FinishFlowVerification".to_string(),
                        },
                    })),
                    Ok(LlmEvent::BlockDelta(BlockDelta {
                        index: 0,
                        delta: DeltaContent::InputJson(input),
                    })),
                    Ok(LlmEvent::BlockStop(BlockStop {
                        index: 0,
                        block_type: BlockType::ToolUse,
                        reasoning: None,
                        stop_reason: Some(StopReason::ToolUse),
                    })),
                ])))
            } else {
                Ok(Box::pin(stream::iter(vec![
                    Ok(LlmEvent::BlockStart(BlockStart {
                        index: 0,
                        block_type: BlockType::Text,
                        metadata: BlockMetadata::Text,
                    })),
                    Ok(LlmEvent::BlockDelta(BlockDelta {
                        index: 0,
                        delta: DeltaContent::Text("verification submitted".to_string()),
                    })),
                    Ok(LlmEvent::BlockStop(BlockStop {
                        index: 0,
                        block_type: BlockType::Text,
                        reasoning: None,
                        stop_reason: Some(StopReason::EndTurn),
                    })),
                ])))
            }
        }

        fn clone_boxed(&self) -> Box<dyn LlmClient> {
            Box::new(self.clone())
        }
    }

    fn verifier_manifest() -> WorkerManifest {
        WorkerManifest::from_toml(
            r#"
[worker]
name = "flow-verifier-parent"

[model]
scheme = "anthropic"
model_id = "test-model"

[engine]

[[scope.allow]]
target = "/abs/scope"
permission = "read"
            "#,
        )
        .unwrap()
    }

    #[tokio::test]
    async fn worker_backed_verifier_requires_structured_finish_result() {
        let directory = tempfile::tempdir().unwrap();
        let store = session_store::FsStore::new(directory.path()).unwrap();
        let session_id: SessionId = Uuid::now_v7();
        let segment_id: SegmentId = Uuid::now_v7();
        store
            .append(
                session_id,
                segment_id,
                &LogEntry::AnnotatedUserInput {
                    ts: 1,
                    extensions: vec![],
                    history: vec![crate::session_history::test_logged_history_entry(
                        agen::Item::user_message("verify current Flow conditions"),
                    )],
                    segments: vec![Segment::Text {
                        content: "verify current Flow conditions".into(),
                    }],
                },
            )
            .unwrap();
        let verifier = WorkerBackedFlowVerifier::new(
            FinishVerifierClient {
                calls: Arc::new(AtomicUsize::new(0)),
            },
            verifier_manifest(),
            StoreFlowParentCapture::new(store, session_id, segment_id),
            Vec::new(),
        );
        let outcome = verifier.verify(&attempt("attempt-worker-backed")).await;
        let FlowVerifierOutcome::Completed { results } = outcome else {
            panic!("expected completed verifier outcome: {outcome:?}");
        };
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].verdict, ConditionVerdict::Met);
        assert_eq!(results[1].verdict, ConditionVerdict::NotMet);
    }

    #[test]
    fn feature_registers_only_request_tool() {
        let coordinator = Arc::new(FakeCoordinator {
            begin_count: AtomicUsize::new(0),
            resolve_count: AtomicUsize::new(0),
        });
        let state = FlowTransitionState::new(coordinator, Arc::new(FakeVerifier));
        let mut pending = Vec::new();
        let mut hooks = HookRegistryBuilder::new();
        let report = FeatureRegistryBuilder::new()
            .with_module(FlowTransitionFeature::new(state))
            .install_into_pending(&mut pending, &mut hooks);
        assert!(report.reports.iter().all(|report| report.installed));
        let names = pending
            .iter()
            .map(|definition| definition().0.name)
            .collect::<Vec<_>>();
        assert_eq!(names, ["RequestFlowTransition"]);
    }
}
