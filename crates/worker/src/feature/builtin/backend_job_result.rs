//! Structured completion capability for Backend-owned Job Workers.
//!
//! The tool intentionally accepts only the result value. Workspace and Worker
//! identity come from the authenticated Workspace client, while Job, attempt,
//! and input revision are resolved from the Backend-owned binding.

use crate::feature::background::BackgroundTaskCancellation;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use agen::tool::{Tool, ToolError, ToolExecutionContext, ToolMeta, ToolOutput};
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use crate::feature::builtin::memory_surface_lifecycle::SubjektivSurfaceLifecycleFeature;
use crate::feature::{
    FeatureDescriptor, FeatureInstallContext, FeatureInstallError, FeatureModule, ToolContribution,
    ToolDeclaration, ToolDefinition,
};
use crate::worker::{WorkspaceClient, WorkspaceRequest, WorkspaceRequestMethod};

const TOOL_NAME: &str = "SubmitBackendJobResult";
const TOOL_DESCRIPTION: &str = "Submit the bounded structured result for this Backend-owned Job attempt. Workspace, Job, attempt, input revision, and source Worker identity are resolved and fenced by Backend authority; normal final prose is not success.";

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SubmitResultInput {
    result: Value,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ConsolidationResult {
    subject_id: String,
    candidate_ids: Vec<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SubmitConsolidationResultInput {
    /// Closed candidate ids only. Host awaits and adds the actual surface outcome.
    result: ConsolidationResult,
}

#[derive(Default)]
struct ResultSubmissionState {
    sealed: Option<SealedResult>,
    reusable_surface: Option<Value>,
}

struct SealedResult {
    original: Value,
    completed: Value,
}

#[derive(Clone)]
struct SubmitResultTool {
    client: Arc<dyn WorkspaceClient>,
    surface: Option<SubjektivSurfaceLifecycleFeature>,
    submission: Arc<tokio::sync::Mutex<ResultSubmissionState>>,
    active: Arc<Mutex<HashMap<String, BackgroundTaskCancellation>>>,
}

struct ActiveResultGuard {
    id: String,
    active: Arc<Mutex<HashMap<String, BackgroundTaskCancellation>>>,
    cancellation: BackgroundTaskCancellation,
}
impl Drop for ActiveResultGuard {
    fn drop(&mut self) {
        self.cancellation.cancel();
        self.active
            .lock()
            .expect("Job result cancellation state poisoned")
            .remove(&self.id);
    }
}

fn cancelled_output() -> ToolError {
    ToolError::Cancelled(ToolOutput {
        summary: "Job result submission cancelled; committed candidate decisions are preserved"
            .into(),
        content: None,
        attachments: Vec::new(),
    })
}

#[async_trait]
impl Tool for SubmitResultTool {
    async fn execute(
        &self,
        input_json: &str,
        context: ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let input: SubmitResultInput = serde_json::from_str(input_json)
            .map_err(|error| ToolError::InvalidArgument(error.to_string()))?;
        let id = context.execution_id();
        let cancellation = self
            .active
            .lock()
            .expect("Job result cancellation state poisoned")
            .entry(id.clone())
            .or_default()
            .clone();
        let _active = ActiveResultGuard {
            id,
            active: self.active.clone(),
            cancellation: cancellation.clone(),
        };
        // Serialize result calls and retain the exact lifecycle outcome for ambiguous retries.
        let mut submission = tokio::select! {
            guard = self.submission.lock() => guard,
            _ = cancellation.cancelled() => return Err(cancelled_output()),
        };
        // An existing seal may already have had an effect, even if this replay
        // is now definitively rejected. It must never be reopened.
        let was_previously_sealed = submission.sealed.is_some();
        let result = if let Some(sealed) = submission.sealed.as_ref() {
            if sealed.original != input.result {
                return Err(ToolError::InvalidArgument(
                    "Job result is already sealed; retry the identical result".into(),
                ));
            }
            sealed.completed.clone()
        } else {
            let mut result = input.result.clone();
            if let Some(surface) = &self.surface {
                let checked = serde_json::from_str::<SubmitConsolidationResultInput>(input_json)
                    .map_err(|error| {
                        ToolError::InvalidArgument(format!(
                            "invalid consolidation result (surface is Host-generated): {error}"
                        ))
                    })?
                    .result;
                if checked.subject_id.trim().is_empty()
                    || checked.candidate_ids.iter().any(|id| id.trim().is_empty())
                {
                    return Err(ToolError::InvalidArgument(
                        "subject_id and candidate ids must not be empty".into(),
                    ));
                }
                let object = result
                    .as_object_mut()
                    .expect("typed consolidation result is an object");
                let outcome = surface
                    .complete_for_job(cancellation.clone(), submission.reusable_surface.as_ref())
                    .await
                    .map_err(|error| {
                        if cancellation.is_cancelled() {
                            cancelled_output()
                        } else {
                            ToolError::ExecutionFailed(error.to_string())
                        }
                    })?;
                submission.reusable_surface = Some(outcome.clone());
                object.insert("surface".into(), outcome);
            }
            result
        };
        if cancellation.is_cancelled() {
            return Err(cancelled_output());
        }
        let workspace_id = self.client.workspace_id().ok_or_else(|| {
            ToolError::ExecutionFailed(
                "Backend Job result submission requires Workspace identity".to_string(),
            )
        })?;
        let request = WorkspaceRequest::json(
            WorkspaceRequestMethod::Post,
            format!("/api/w/{workspace_id}/workers/self/backend-job-result"),
            serde_json::to_string(&serde_json::json!({ "result": result }))
                .map_err(|error| ToolError::Internal(error.to_string()))?,
        );
        // Seal before dispatch: a transport failure may follow durable acceptance.
        if submission.sealed.is_none() {
            submission.sealed = Some(SealedResult {
                original: input.result,
                completed: result,
            });
            submission.reusable_surface = None;
        }
        let response = match self.client.execute(request) {
            Ok(response) => response,
            Err(error) => {
                return Err(ToolError::ExecutionFailed(format!(
                    "Backend Job result acceptance is unknown; retry the identical result: {error}"
                )));
            }
        };
        let definitive_rejection = matches!(response.status, 400 | 403 | 409 | 422);
        if !response.is_success() {
            let sealed = submission.sealed.as_mut().expect("sealed before dispatch");
            if definitive_rejection && !was_previously_sealed {
                // Only a definitively unaccepted request can be corrected. Reuse
                // a ready snapshot after a new preparation confirms its revision;
                // a conflict invalidates even that snapshot. Failed outcomes are
                // rebuilt, since another generation can replace their marker.
                submission.reusable_surface = if response.status != 409 {
                    sealed
                        .completed
                        .get("surface")
                        .filter(|surface| surface["availability"] == "ready")
                        .cloned()
                } else {
                    None
                };
                submission.sealed = None;
                return Err(ToolError::ExecutionFailed(format!(
                    "Backend Job result was definitively rejected with HTTP {}; correct result fields or retry to rebuild a stale surface: {}",
                    response.status, response.body
                )));
            }
            return Err(ToolError::ExecutionFailed(format!(
                "Backend Job result acceptance remains sealed; retry the identical result (HTTP {}): {}",
                response.status, response.body
            )));
        }
        Ok(ToolOutput {
            summary: "Backend Job result accepted".to_string(),
            content: Some(response.body),
            attachments: Vec::new(),
        })
    }

    async fn cancel_execution(&self, context: &ToolExecutionContext) -> Result<(), ToolError> {
        self.active
            .lock()
            .expect("Job result cancellation state poisoned")
            .entry(context.execution_id())
            .or_default()
            .cancel();
        Ok(())
    }
}

fn definition(
    client: Arc<dyn WorkspaceClient>,
    surface: Option<SubjektivSurfaceLifecycleFeature>,
) -> ToolDefinition {
    let submission = Arc::new(tokio::sync::Mutex::new(ResultSubmissionState::default()));
    let active = Arc::new(Mutex::new(HashMap::new()));
    let schema = if surface.is_some() {
        serde_json::to_value(schemars::schema_for!(SubmitConsolidationResultInput))
    } else {
        serde_json::to_value(schemars::schema_for!(SubmitResultInput))
    }
    .expect("Backend Job result schema must serialize");
    Arc::new(move || {
        (
            ToolMeta::new(TOOL_NAME)
                .description(TOOL_DESCRIPTION)
                .input_schema(schema.clone()),
            Arc::new(SubmitResultTool {
                client: client.clone(),
                surface: surface.clone(),
                submission: submission.clone(),
                active: active.clone(),
            }) as Arc<dyn Tool>,
        )
    })
}

#[derive(Clone)]
pub struct BackendJobResultFeature {
    client: Arc<dyn WorkspaceClient>,
    surface: Option<SubjektivSurfaceLifecycleFeature>,
}

impl BackendJobResultFeature {
    pub fn new(client: Arc<dyn WorkspaceClient>) -> Self {
        Self {
            client,
            surface: None,
        }
    }
}

impl BackendJobResultFeature {
    pub(crate) fn with_surface(mut self, surface: SubjektivSurfaceLifecycleFeature) -> Self {
        self.surface = Some(surface);
        self
    }
}

impl FeatureModule for BackendJobResultFeature {
    fn descriptor(&self) -> FeatureDescriptor {
        FeatureDescriptor::builtin("backend-job-result", "Backend Job result")
            .with_description("Single-purpose structured result capability for Backend-owned Jobs.")
            .with_tool(ToolDeclaration::new(TOOL_NAME, TOOL_DESCRIPTION))
    }

    fn install(&self, context: &mut FeatureInstallContext<'_>) -> Result<(), FeatureInstallError> {
        context.tools().register(ToolContribution::new(
            TOOL_NAME,
            definition(self.client.clone(), self.surface.clone()),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feature::FeatureRegistryBuilder;
    use crate::hook::HookRegistryBuilder;
    use crate::worker::{WorkspaceClientError, WorkspaceResponse};
    use std::sync::Mutex;

    #[derive(Debug, Default)]
    struct RecordingClient {
        requests: Mutex<Vec<WorkspaceRequest>>,
    }

    impl WorkspaceClient for RecordingClient {
        fn workspace_id(&self) -> Option<&str> {
            Some("workspace-test")
        }
        fn kind(&self) -> &str {
            "test"
        }
        fn is_available(&self) -> bool {
            true
        }
        fn execute(
            &self,
            request: WorkspaceRequest,
        ) -> Result<WorkspaceResponse, WorkspaceClientError> {
            self.requests.lock().unwrap().push(request);
            Ok(WorkspaceResponse {
                status: 200,
                body: "{\"replayed\":false}".to_string(),
            })
        }
    }

    #[tokio::test]
    async fn feature_exposes_only_identity_free_structured_result_input() {
        let client = Arc::new(RecordingClient::default());
        let mut pending = Vec::new();
        let mut hooks = HookRegistryBuilder::default();
        let report = FeatureRegistryBuilder::new()
            .with_module(BackendJobResultFeature::new(client.clone()))
            .install_into_pending(&mut pending, &mut hooks);
        assert_eq!(report.installed_tool_names(), [TOOL_NAME]);
        let (meta, tool) = pending.remove(0)();
        assert_eq!(meta.input_schema["required"], serde_json::json!(["result"]));
        assert!(meta.input_schema["properties"].get("job_id").is_none());
        tool.execute(
            r#"{"result":{"valid":true}}"#,
            ToolExecutionContext::direct(),
        )
        .await
        .unwrap();
        let requests = client.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, WorkspaceRequestMethod::Post);
        assert!(
            requests[0]
                .path
                .ends_with("/workers/self/backend-job-result")
        );
    }
    #[tokio::test]
    async fn cancellation_before_execution_never_submits_a_result() {
        let client = Arc::new(RecordingClient::default());
        let (_, tool) = definition(client.clone(), None)();
        let context = ToolExecutionContext::new("result-call", "batch", 0);
        tool.cancel_execution(&context).await.unwrap();
        let outcome = tool.execute(r#"{"result":{"valid":true}}"#, context).await;
        assert!(matches!(outcome, Err(ToolError::Cancelled(_))));
        assert!(client.requests.lock().unwrap().is_empty());
    }
}
