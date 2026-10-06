use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use agen::tool::{Tool, ToolDefinition, ToolError, ToolExecutionContext, ToolMeta, ToolOutput};
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use super::JobExecutionError;
use crate::feature::{
    FeatureDescriptor, FeatureInstallContext, FeatureInstallError, FeatureModule, ToolContribution,
    ToolDeclaration,
};

pub const SUBMIT_JOB_RESULT_TOOL: &str = "SubmitJobResult";
const DESCRIPTION: &str = "Submit this Host Job's bounded structured result. Job, attempt and input revision are immutable Host bindings. Only result is accepted as input; final prose is not success.";

/// Host acceptance authority. Implementations must validate the immutable fences and commit
/// acceptance durably before returning Ok. Err may mean an unknown outcome; identical retries
/// must be idempotent. This synchronous callback must be bounded and must not wait for async work.
pub trait JobResultSink: Send + Sync {
    fn submit(&self, submission: ::job::JobResultSubmission) -> Result<(), String>;
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SubmitJobResultInput {
    result: Value,
}

#[derive(Default)]
struct SubmissionState {
    sealed: Option<Value>,
    unconfirmed: Option<String>,
}

/// Single-purpose FeatureModule; Profile names never install it or confer result authority.
#[derive(Clone)]
pub struct JobResultFeature {
    job_id: String,
    attempt_id: String,
    input_revision: String,
    max_result_bytes: u32,
    sink: Arc<dyn JobResultSink>,
    state: Arc<Mutex<SubmissionState>>,
    accepted: Arc<Mutex<Option<::job::JobResultSubmission>>>,
    cancelled: Arc<Mutex<HashSet<String>>>,
}

impl JobResultFeature {
    pub fn new(
        request: &::job::JobRequest,
        attempt_id: String,
        sink: Arc<dyn JobResultSink>,
    ) -> Result<Self, JobExecutionError> {
        request.validate()?;
        ::job::validate_identifier("attempt_id", &attempt_id, 512)?;
        Ok(Self {
            job_id: request.job_id.clone(),
            attempt_id,
            input_revision: request.input_revision.clone(),
            max_result_bytes: request.limits.max_result_bytes,
            sink,
            state: Arc::default(),
            accepted: Arc::default(),
            cancelled: Arc::default(),
        })
    }

    pub(super) fn accepted(&self) -> Arc<Mutex<Option<::job::JobResultSubmission>>> {
        self.accepted.clone()
    }

    pub(super) fn unconfirmed(&self) -> Option<String> {
        self.state
            .lock()
            .expect("Job submission state poisoned")
            .unconfirmed
            .clone()
    }

    fn definition(&self) -> ToolDefinition {
        let feature = self.clone();
        Arc::new(move || {
            (
                ToolMeta::new(SUBMIT_JOB_RESULT_TOOL)
                    .description(DESCRIPTION)
                    .input_schema(
                        serde_json::to_value(schemars::schema_for!(SubmitJobResultInput))
                            .expect("Job result schema serializes"),
                    ),
                Arc::new(feature.clone()) as Arc<dyn Tool>,
            )
        })
    }
}

#[async_trait]
impl Tool for JobResultFeature {
    async fn execute(
        &self,
        input_json: &str,
        context: ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let input: SubmitJobResultInput = serde_json::from_str(input_json)
            .map_err(|error| ToolError::InvalidArgument(error.to_string()))?;
        ::job::result_digest(&input.result, self.max_result_bytes)
            .map_err(|error| ToolError::InvalidArgument(error.to_string()))?;
        let mut state = self.state.lock().expect("Job submission state poisoned");
        if self
            .cancelled
            .lock()
            .expect("Job cancellation state poisoned")
            .contains(&context.execution_id())
        {
            return Err(ToolError::Cancelled(ToolOutput {
                summary: "Job result submission cancelled".into(),
                content: None,
                attachments: vec![],
            }));
        }
        if let Some(sealed) = &state.sealed {
            if sealed != &input.result {
                return Err(ToolError::InvalidArgument(
                    "Job result is sealed; retry only the identical result".into(),
                ));
            }
        } else {
            // Seal before dispatch: a sink error can follow durable Host acceptance.
            state.sealed = Some(input.result.clone());
        }
        let submission = ::job::JobResultSubmission {
            job_id: self.job_id.clone(),
            attempt_id: self.attempt_id.clone(),
            input_revision: self.input_revision.clone(),
            result: input.result,
        };
        let mut accepted = self.accepted.lock().expect("Job result state poisoned");
        if accepted.is_none() {
            if let Err(error) = self.sink.submit(submission.clone()) {
                let error = ::job::bounded_failure_detail(&error);
                state.unconfirmed = Some(error.clone());
                return Err(ToolError::ExecutionFailed(format!(
                    "Host Job result acceptance was not confirmed; retry only the identical result: {error}"
                )));
            }
            state.unconfirmed = None;
            *accepted = Some(submission);
        }
        Ok(ToolOutput {
            summary: "Host Job result accepted".into(),
            content: None,
            attachments: vec![],
        })
    }

    async fn cancel_execution(&self, context: &ToolExecutionContext) -> Result<(), ToolError> {
        self.cancelled
            .lock()
            .expect("Job cancellation state poisoned")
            .insert(context.execution_id());
        Ok(())
    }
}

impl FeatureModule for JobResultFeature {
    fn descriptor(&self) -> FeatureDescriptor {
        FeatureDescriptor::builtin("host-job-result", "Host Job result")
            .with_description("Result-only immutable Host Job submission capability.")
            .with_tool(ToolDeclaration::new(SUBMIT_JOB_RESULT_TOOL, DESCRIPTION))
    }
    fn install(&self, context: &mut FeatureInstallContext<'_>) -> Result<(), FeatureInstallError> {
        context.tools().register(ToolContribution::new(
            SUBMIT_JOB_RESULT_TOOL,
            self.definition(),
        ))
    }
}
