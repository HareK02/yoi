//! Durable Backend-owned jobs executed by ordinary Runtime-managed Workers.
//!
//! A job is durable intent. An attempt binds that intent to one dedicated
//! Worker, and a result is accepted only through the fenced structured-result
//! API below. Runtime lifecycle and final prose are deliberately not success
//! authority.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use worker_runtime::identity::RuntimeWorkerRef;

use crate::{Error, Result};

pub const BACKEND_JOB_RUNTIME_ID: &str = crate::hosts::EMBEDDED_RUNTIME_ID;
pub use job::{
    ABSOLUTE_MAX_ATTEMPTS, ABSOLUTE_MAX_CONCURRENT_JOBS, ABSOLUTE_MAX_RESULT_BYTES,
    DEFAULT_JOB_TIMEOUT_SECONDS, DEFAULT_MAX_ATTEMPTS, DEFAULT_MAX_CONCURRENT_JOBS,
    DEFAULT_MAX_RESULT_BYTES, JobResultSubmission as BackendJobResultSubmission,
    MAX_JOB_DELIVERY_BYTES, MAX_JOB_FAILURE_BYTES, MAX_JOB_INPUT_BYTES, MAX_JOB_INSTRUCTION_BYTES,
    MAX_JOB_PURPOSE_BYTES, MAX_JOB_REFERENCE_BYTES, MAX_JOB_TIMEOUT_SECONDS, attempt_id,
    bounded_failure_detail,
};

// Preserve the Backend InvalidInput surface and its exact historical messages.
fn backend_error(error: job::JobError) -> Error {
    match error {
        job::JobError::InvalidInput(detail) => {
            Error::InvalidInput(detail.replacen("Job ", "Backend Job ", 1))
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BackendJobState {
    Pending,
    Completed,
    Failed,
    Unknown,
}

impl From<BackendJobState> for job::JobState {
    fn from(state: BackendJobState) -> Self {
        match state {
            BackendJobState::Pending => Self::Pending,
            BackendJobState::Completed => Self::Completed,
            BackendJobState::Failed => Self::Failed,
            BackendJobState::Unknown => Self::Unknown,
        }
    }
}

impl BackendJobState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Unknown => "unknown",
        }
    }

    pub(crate) fn parse(value: &str) -> Result<Self> {
        match value {
            "pending" => Ok(Self::Pending),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            "unknown" => Ok(Self::Unknown),
            other => Err(Error::Store(format!("unknown Backend Job state `{other}`"))),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BackendJobAttemptState {
    Reserved,
    Dispatching,
    Dispatched,
    Completed,
    Failed,
    Unknown,
}

impl From<BackendJobAttemptState> for job::JobAttemptState {
    fn from(state: BackendJobAttemptState) -> Self {
        match state {
            BackendJobAttemptState::Reserved => Self::Reserved,
            BackendJobAttemptState::Dispatching => Self::Dispatching,
            BackendJobAttemptState::Dispatched => Self::Dispatched,
            BackendJobAttemptState::Completed => Self::Completed,
            BackendJobAttemptState::Failed => Self::Failed,
            BackendJobAttemptState::Unknown => Self::Unknown,
        }
    }
}

impl BackendJobAttemptState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Reserved => "reserved",
            Self::Dispatching => "dispatching",
            Self::Dispatched => "dispatched",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Unknown => "unknown",
        }
    }

    pub(crate) fn parse(value: &str) -> Result<Self> {
        match value {
            "reserved" => Ok(Self::Reserved),
            "dispatching" => Ok(Self::Dispatching),
            "dispatched" => Ok(Self::Dispatched),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            "unknown" => Ok(Self::Unknown),
            other => Err(Error::Store(format!(
                "unknown Backend Job attempt state `{other}`"
            ))),
        }
    }

    pub fn terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Unknown)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BackendJobDeliveryState {
    Pending,
    Sending,
    Completed,
    Failed,
    Unknown,
}

impl BackendJobDeliveryState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Sending => "sending",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Unknown => "unknown",
        }
    }

    pub(crate) fn parse(value: &str) -> Result<Self> {
        match value {
            "pending" => Ok(Self::Pending),
            "sending" => Ok(Self::Sending),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            "unknown" => Ok(Self::Unknown),
            other => Err(Error::Store(format!(
                "unknown Backend Job delivery state `{other}`"
            ))),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BackendJobLimits {
    #[serde(default = "default_max_concurrent_jobs")]
    pub max_concurrent_jobs: u16,
    #[serde(default = "default_job_timeout_seconds")]
    pub timeout_seconds: u32,
    #[serde(default = "default_max_result_bytes")]
    pub max_result_bytes: u32,
    #[serde(default = "default_max_attempts")]
    pub max_attempts: u8,
}

impl Default for BackendJobLimits {
    fn default() -> Self {
        Self {
            max_concurrent_jobs: DEFAULT_MAX_CONCURRENT_JOBS,
            timeout_seconds: DEFAULT_JOB_TIMEOUT_SECONDS,
            max_result_bytes: DEFAULT_MAX_RESULT_BYTES,
            max_attempts: DEFAULT_MAX_ATTEMPTS,
        }
    }
}

impl BackendJobLimits {
    pub fn validate(&self) -> Result<()> {
        job::JobLimits::from(self).validate().map_err(backend_error)
    }
}

impl From<&BackendJobLimits> for job::JobLimits {
    fn from(limits: &BackendJobLimits) -> Self {
        Self {
            max_concurrent_jobs: limits.max_concurrent_jobs,
            timeout_seconds: limits.timeout_seconds,
            max_result_bytes: limits.max_result_bytes,
            max_attempts: limits.max_attempts,
        }
    }
}

impl From<job::JobLimits> for BackendJobLimits {
    fn from(limits: job::JobLimits) -> Self {
        Self {
            max_concurrent_jobs: limits.max_concurrent_jobs,
            timeout_seconds: limits.timeout_seconds,
            max_result_bytes: limits.max_result_bytes,
            max_attempts: limits.max_attempts,
        }
    }
}

const fn default_max_concurrent_jobs() -> u16 {
    DEFAULT_MAX_CONCURRENT_JOBS
}
const fn default_job_timeout_seconds() -> u32 {
    DEFAULT_JOB_TIMEOUT_SECONDS
}
const fn default_max_result_bytes() -> u32 {
    DEFAULT_MAX_RESULT_BYTES
}
const fn default_max_attempts() -> u8 {
    DEFAULT_MAX_ATTEMPTS
}

/// Explicit Backend-issued domain capabilities, never inferred from purpose or profile.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BackendJobGrants {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subjektiv_consolidation: Option<SubjektivConsolidationGrant>,
}

impl BackendJobGrants {
    pub fn is_empty(&self) -> bool {
        self.subjektiv_consolidation.is_none()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SubjektivConsolidationGrant {
    pub subject_id: String,
    /// Exact immutable candidate scope. Empty is a surface-only refresh grant:
    /// the subject is still locked, but no candidate access is authorized.
    pub candidate_ids: Vec<String>,
}

/// Common runner lock identity for a subject, independent of the candidate snapshot.
pub fn subjektiv_serialization_key(subject_id: &str) -> String {
    format!("subjektiv-consolidation:{subject_id}")
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BackendJobRequest {
    /// Stable caller-selected idempotency key within one Workspace.
    pub job_id: String,
    /// Stable machine-readable use-site name, for example `ticket_item_check`.
    pub purpose: String,
    /// Durable domain reference used to reload or explain the input.
    pub input_ref: String,
    /// Bounded immutable input snapshot. This must not contain credentials.
    pub input: Value,
    /// Bounded task instruction; the profile supplies the system instruction.
    pub instruction: String,
    /// Existing configured profile selector. Job dispatch never accepts raw profile source.
    pub profile: String,
    /// Immutable domain grants; profile resolution alone never grants access.
    #[serde(default, skip_serializing_if = "BackendJobGrants::is_empty")]
    pub grants: BackendJobGrants,
    /// Optional common-runner single-flight resource, not a separate domain state machine.
    /// Consolidation grants always derive their subject lock; an explicit key must match it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serialization_key: Option<String>,
    /// Writer provenance and optional advisory destination, not parent ownership.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_worker: Option<RuntimeWorkerRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notification_target: Option<RuntimeWorkerRef>,
    #[serde(default)]
    pub limits: BackendJobLimits,
}

impl BackendJobRequest {
    /// Adapt neutral intent without granting domain access or writer/notification
    /// authority. Validation remains the Backend submission boundary's job;
    /// the caller-selected profile is retained exactly, with no fallback.
    pub fn from_job_request(request: job::JobRequest) -> Self {
        Self {
            job_id: request.job_id,
            purpose: request.purpose,
            input_ref: request.input_ref,
            input: request.input,
            instruction: request.instruction,
            profile: request.profile,
            grants: BackendJobGrants::default(),
            serialization_key: request.serialization_key,
            source_worker: None,
            notification_target: None,
            limits: request.limits.into(),
        }
    }

    pub fn validate(&self) -> Result<()> {
        let request = self.to_job_request();
        request.validate_fields().map_err(backend_error)?;
        if let Some(grant) = &self.grants.subjektiv_consolidation {
            validate_identifier("grant subject_id", &grant.subject_id, 256)?;
            if grant.candidate_ids.len() > 256 {
                return Err(Error::InvalidInput(
                    "Backend Job consolidation grant requires 0..=256 candidate ids".into(),
                ));
            }
            let mut seen = std::collections::HashSet::new();
            for id in &grant.candidate_ids {
                validate_identifier("grant candidate_id", id, 256)?;
                if !seen.insert(id) {
                    return Err(Error::InvalidInput(
                        "Backend Job consolidation grant has duplicate candidate ids".into(),
                    ));
                }
            }
            if let Some(key) = &self.serialization_key
                && *key != subjektiv_serialization_key(&grant.subject_id)
            {
                return Err(Error::InvalidInput(
                    "Backend Job serialization_key must match the consolidation subject lock"
                        .into(),
                ));
            }
        }
        request.validate_input().map_err(backend_error)?;
        self.limits.validate()
    }

    /// Transport-neutral projection only. Domain grants, writer authority and
    /// derived resource locks remain Backend-owned; never fingerprint this
    /// projection as a substitute for the full Backend intent.
    pub fn to_job_request(&self) -> job::JobRequest {
        job::JobRequest {
            job_id: self.job_id.clone(),
            purpose: self.purpose.clone(),
            input_ref: self.input_ref.clone(),
            input: self.input.clone(),
            instruction: self.instruction.clone(),
            profile: self.profile.clone(),
            serialization_key: self.serialization_key.clone(),
            limits: (&self.limits).into(),
        }
    }

    /// Validate selector syntax only. Registry existence and resolution are dispatch authority.
    pub fn profile_selector(&self) -> Result<manifest::ProfileSelector> {
        job::profile_selector(&self.profile).map_err(backend_error)
    }

    /// Effective single-flight key. Typed grants cannot bypass their subject fence.
    pub fn resource_key(&self) -> Option<String> {
        self.grants
            .subjektiv_consolidation
            .as_ref()
            .map(|grant| subjektiv_serialization_key(&grant.subject_id))
            .or_else(|| self.serialization_key.clone())
    }

    pub fn input_digest(&self) -> Result<String> {
        self.to_job_request().input_digest().map_err(backend_error)
    }

    pub fn fingerprint(&self) -> Result<String> {
        self.validate()?;
        job::fingerprint(self).map_err(backend_error)
    }

    pub fn worker_input(&self, attempt_id: &str) -> Result<String> {
        job::worker_input_with_label(&self.to_job_request(), attempt_id, "Backend Job")
            .map_err(backend_error)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BackendJobRecord {
    pub workspace_id: String,
    pub request: BackendJobRequest,
    pub intent_fingerprint: String,
    pub state: BackendJobState,
    pub current_attempt: u8,
    pub result: Option<Value>,
    pub result_digest: Option<String>,
    pub failure_category: Option<String>,
    pub failure_detail: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BackendJobWorkerCleanupState {
    Pending,
    Executing,
    Completed,
    Failed,
}

impl BackendJobWorkerCleanupState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Executing => "executing",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }

    pub(crate) fn parse(value: &str) -> Result<Self> {
        match value {
            "pending" => Ok(Self::Pending),
            "executing" => Ok(Self::Executing),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            other => Err(Error::Store(format!(
                "unknown Backend Job Worker cleanup state `{other}`"
            ))),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BackendJobAttemptRecord {
    pub workspace_id: String,
    pub job_id: String,
    pub attempt_id: String,
    pub attempt: u8,
    pub input_digest: String,
    pub state: BackendJobAttemptState,
    pub worker: Option<RuntimeWorkerRef>,
    pub runtime_run_id: Option<String>,
    pub dispatched_at: Option<String>,
    pub deadline_at: String,
    pub result: Option<Value>,
    pub result_digest: Option<String>,
    pub failure_category: Option<String>,
    pub failure_detail: Option<String>,
    pub worker_cleanup_state: Option<BackendJobWorkerCleanupState>,
    pub worker_cleanup_failure_category: Option<String>,
    pub worker_cleanup_failure_detail: Option<String>,
    pub worker_cleanup_updated_at: Option<String>,
    pub worker_cleanup_completed_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BackendJobReservation {
    pub job: BackendJobRecord,
    pub attempt: BackendJobAttemptRecord,
    /// Exact replay of the caller-selected immutable intent.
    pub replayed: bool,
    /// A different intent already fences this resource. The returned request is
    /// the original snapshot; the caller must defer its new input to a followup.
    #[serde(default)]
    pub resource_reused: bool,
}

impl BackendJobReservation {
    pub fn outcome(&self) -> job::JobOutcome {
        job_outcome(&self.job, &self.attempt)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BackendJobResultAcceptance {
    pub job: BackendJobRecord,
    pub attempt: BackendJobAttemptRecord,
    pub replayed: bool,
}

impl BackendJobResultAcceptance {
    pub fn outcome(&self) -> job::JobOutcome {
        job_outcome(&self.job, &self.attempt)
    }
}

// Project the durable Job outcome, not Worker lifecycle, cleanup or delivery.
// The paired attempt supplies its own identity, number and execution state.
fn job_outcome(job: &BackendJobRecord, attempt: &BackendJobAttemptRecord) -> job::JobOutcome {
    job::JobOutcome {
        job_id: job.request.job_id.clone(),
        input_digest: attempt.input_digest.clone(),
        attempt_id: attempt.attempt_id.clone(),
        attempt: attempt.attempt,
        state: job.state.into(),
        attempt_state: attempt.state.into(),
        result: job.result.clone(),
        failure_category: job.failure_category.clone(),
        failure_detail: job.failure_detail.clone(),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BackendJobWorkerBinding {
    pub job_id: String,
    pub attempt_id: String,
    pub purpose: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BackendJobDeliveryRecord {
    pub workspace_id: String,
    pub delivery_id: String,
    pub job_id: String,
    pub attempt_id: String,
    pub target: RuntimeWorkerRef,
    pub state: BackendJobDeliveryState,
    pub failure_category: Option<String>,
    pub failure_detail: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub delivered_at: Option<String>,
}

pub fn allocation_key(attempt_id: &str) -> String {
    format!("backend-job:{attempt_id}")
}

/// Fixed-size Runtime request identity for the exact Job attempt. Caller-selected
/// Job ids may be longer than the Worker protocol request-id bound, so never
/// embed them directly in tracked Runtime identities.
pub fn runtime_run_id(attempt_id: &str) -> String {
    tracked_request_id("run", attempt_id.as_bytes())
}

/// Fixed-size durable delivery identity, also reused as the Worker's tracked
/// notification request id so restart replay converges on one receipt.
pub fn delivery_id(job_id: &str, attempt_id: &str) -> String {
    let mut identity = Vec::with_capacity(job_id.len() + attempt_id.len() + 1);
    identity.extend_from_slice(job_id.as_bytes());
    identity.push(0);
    identity.extend_from_slice(attempt_id.as_bytes());
    tracked_request_id("notification", &identity)
}

fn tracked_request_id(kind: &str, identity: &[u8]) -> String {
    format!("backend-job-{kind}:{}", job::sha256(identity))
}

pub fn result_digest(result: &Value, max_bytes: u32) -> Result<(String, String)> {
    job::result_digest(result, max_bytes).map_err(backend_error)
}

fn validate_identifier(field: &str, value: &str, max_bytes: usize) -> Result<()> {
    job::validate_identifier(field, value, max_bytes).map_err(backend_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> BackendJobRequest {
        BackendJobRequest {
            job_id: "ticket-check:T-1:r7".to_string(),
            purpose: "ticket_item_check".to_string(),
            input_ref: "ticket://T-1/revisions/7".to_string(),
            input: serde_json::json!({"title": "Check me"}),
            instruction: "Check this immutable Ticket snapshot.".to_string(),
            profile: "builtin:backend-job".to_string(),
            grants: BackendJobGrants::default(),
            serialization_key: None,
            source_worker: Some(RuntimeWorkerRef::new("runtime-a", "worker-a")),
            notification_target: Some(RuntimeWorkerRef::new("runtime-a", "worker-a")),
            limits: BackendJobLimits::default(),
        }
    }

    fn reservation() -> BackendJobReservation {
        serde_json::from_value(serde_json::json!({
            "job": {
                "workspace_id": "workspace-a",
                "request": request(),
                "intent_fingerprint": request().fingerprint().unwrap(),
                "state": "pending",
                "current_attempt": 2,
                "created_at": "created",
                "updated_at": "updated"
            },
            "attempt": {
                "workspace_id": "workspace-a",
                "job_id": "ticket-check:T-1:r7",
                "attempt_id": "ticket-check:T-1:r7:attempt:2",
                "attempt": 2,
                "input_digest": "7",
                "state": "reserved",
                "worker": {"runtime_id": "runtime-a", "worker_id": "worker-a"},
                "runtime_run_id": "run-a",
                "deadline_at": "deadline",
                "created_at": "created",
                "updated_at": "updated"
            },
            "replayed": false,
            "resource_reused": false
        }))
        .unwrap()
    }

    #[test]
    fn backend_state_projections_preserve_existing_wire_vocabulary() {
        for state in [
            BackendJobState::Pending,
            BackendJobState::Completed,
            BackendJobState::Failed,
            BackendJobState::Unknown,
        ] {
            let shared = job::JobState::from(state);
            assert_eq!(
                serde_json::to_value(state).unwrap(),
                serde_json::to_value(shared).unwrap()
            );
        }
        for state in [
            BackendJobAttemptState::Reserved,
            BackendJobAttemptState::Dispatching,
            BackendJobAttemptState::Dispatched,
            BackendJobAttemptState::Completed,
            BackendJobAttemptState::Failed,
            BackendJobAttemptState::Unknown,
        ] {
            let shared = job::JobAttemptState::from(state);
            assert_eq!(
                serde_json::to_value(state).unwrap(),
                serde_json::to_value(shared).unwrap()
            );
        }
        // The shared read vocabulary does not add Backend cancellation support.
        assert!(BackendJobState::parse("cancelled").is_err());
        assert!(BackendJobAttemptState::parse("cancelled").is_err());
        assert!(serde_json::from_str::<BackendJobState>("\"cancelled\"").is_err());
        assert!(serde_json::from_str::<BackendJobAttemptState>("\"cancelled\"").is_err());
    }

    #[test]
    fn neutral_request_round_trip_does_not_infer_authority_or_replace_profile() {
        for profile in ["builtin:backend-job", "builtin:job", "project:custom-job"] {
            for key in [None, Some("resource:7".to_string())] {
                let mut neutral = request().to_job_request();
                neutral.profile = profile.into();
                neutral.serialization_key = key;
                neutral.limits = job::JobLimits {
                    max_concurrent_jobs: 3,
                    timeout_seconds: 17,
                    max_result_bytes: 777,
                    max_attempts: 1,
                };
                let backend = BackendJobRequest::from_job_request(neutral.clone());
                backend.validate().unwrap();
                assert_eq!(backend.to_job_request(), neutral);
                assert!(backend.grants.is_empty());
                assert!(backend.source_worker.is_none());
                assert!(backend.notification_target.is_none());
                let encoded = serde_json::to_value(&backend).unwrap();
                for field in ["grants", "source_worker", "notification_target"] {
                    assert!(encoded.get(field).is_none());
                }
                assert_eq!(encoded["profile"], profile);
            }
        }
        // Conversion is not validation and must not normalize an invalid request.
        let mut neutral = request().to_job_request();
        neutral.limits.timeout_seconds = 0;
        neutral.profile = "inherit".into();
        let backend = BackendJobRequest::from_job_request(neutral.clone());
        assert_eq!(backend.to_job_request(), neutral);
        assert!(backend.validate().is_err());
    }

    #[test]
    fn reservation_and_acceptance_outcomes_expose_durable_job_and_attempt_not_host_lifecycle() {
        let mut reservation = reservation();
        let encoded_before = serde_json::to_value(&reservation).unwrap();
        assert_eq!(
            reservation.outcome(),
            job::JobOutcome {
                job_id: reservation.job.request.job_id.clone(),
                input_digest: "7".into(),
                attempt_id: "ticket-check:T-1:r7:attempt:2".into(),
                attempt: 2,
                state: job::JobState::Pending,
                attempt_state: job::JobAttemptState::Reserved,
                result: None,
                failure_category: None,
                failure_detail: None,
            }
        );
        assert_eq!(serde_json::to_value(&reservation).unwrap(), encoded_before);
        for (state, attempt_state) in [
            (
                BackendJobState::Pending,
                BackendJobAttemptState::Dispatching,
            ),
            (BackendJobState::Pending, BackendJobAttemptState::Dispatched),
            (BackendJobState::Failed, BackendJobAttemptState::Failed),
            (BackendJobState::Unknown, BackendJobAttemptState::Unknown),
        ] {
            reservation.job.state = state;
            reservation.attempt.state = attempt_state;
            reservation.job.failure_category = Some("job-category".into());
            reservation.job.failure_detail = Some("job-detail".into());
            reservation.attempt.failure_category = Some("attempt-category".into());
            let outcome = reservation.outcome();
            assert_eq!(outcome.state, state.into());
            assert_eq!(outcome.attempt_state, attempt_state.into());
            assert_eq!(outcome.failure_category.as_deref(), Some("job-category"));
            assert_eq!(outcome.failure_detail.as_deref(), Some("job-detail"));
            assert!(outcome.result.is_none());
            let accepted = BackendJobResultAcceptance {
                job: reservation.job.clone(),
                attempt: reservation.attempt.clone(),
                replayed: true,
            };
            assert_eq!(accepted.outcome(), outcome);
        }
        reservation.job.state = BackendJobState::Completed;
        reservation.attempt.state = BackendJobAttemptState::Completed;
        reservation.job.result = Some(serde_json::json!({"ok": true}));
        reservation.job.failure_category = None;
        reservation.job.failure_detail = None;
        reservation.attempt.worker_cleanup_state = Some(BackendJobWorkerCleanupState::Failed);
        reservation.attempt.worker_cleanup_failure_detail =
            Some("cleanup is not model failure".into());
        reservation.replayed = true;
        reservation.resource_reused = true;
        let acceptance = BackendJobResultAcceptance {
            job: reservation.job.clone(),
            attempt: reservation.attempt.clone(),
            replayed: false,
        };
        let outcome = acceptance.outcome();
        assert_eq!(outcome, reservation.outcome());
        assert_eq!(outcome.state, job::JobState::Completed);
        assert_eq!(outcome.result, Some(serde_json::json!({"ok": true})));
        assert!(outcome.failure_category.is_none());
        assert!(outcome.failure_detail.is_none());
        let encoded = serde_json::to_value(outcome).unwrap();
        assert_eq!(encoded.as_object().unwrap().len(), 9);
        for field in [
            "workspace_id",
            "worker",
            "runtime_run_id",
            "request",
            "replayed",
            "resource_reused",
            "worker_cleanup_state",
        ] {
            assert!(encoded.get(field).is_none());
        }
    }

    #[test]
    fn extraction_preserves_backend_wire_fingerprint_and_worker_envelope() {
        let request = request();
        let encoded = serde_json::to_string(&request).unwrap();
        assert_eq!(
            encoded,
            r#"{"job_id":"ticket-check:T-1:r7","purpose":"ticket_item_check","input_ref":"ticket://T-1/revisions/7","input":{"title":"Check me"},"instruction":"Check this immutable Ticket snapshot.","profile":"builtin:backend-job","source_worker":{"runtime_id":"runtime-a","worker_id":"worker-a"},"notification_target":{"runtime_id":"runtime-a","worker_id":"worker-a"},"limits":{"max_concurrent_jobs":8,"timeout_seconds":120,"max_result_bytes":16384,"max_attempts":2}}"#
        );
        assert_eq!(
            request.fingerprint().unwrap(),
            "sha256:250b626572e954d2aa2228364274c946d9f6b176eb0f0a205157e55031581147"
        );
        let neutral = request.to_job_request();
        neutral.validate().unwrap();
        assert_ne!(
            request.fingerprint().unwrap(),
            neutral.fingerprint().unwrap()
        );
        assert_eq!(
            serde_json::to_value(&request.limits).unwrap(),
            serde_json::to_value(&neutral.limits).unwrap()
        );
        assert_eq!(
            request.worker_input("attempt-1").unwrap(),
            "Check this immutable Ticket snapshot.\n\nBackend Job envelope (immutable):\njob_id: ticket-check:T-1:r7\nattempt_id: attempt-1\ninput_digest: sha256:7d0f26b7fc612e982fd7acf2b64f29d2e8f65c7e1292683e36d7dcac38905c50\ninput_ref: ticket://T-1/revisions/7\ninput_json: {\"title\":\"Check me\"}\n\nReturn success only through the structured Backend Job result capability. Final prose and Worker Idle/Stopped state are not result authority."
        );
    }

    #[test]
    fn shared_result_contract_preserves_backend_wire_and_errors() {
        let submission = BackendJobResultSubmission {
            job_id: "job-1".into(),
            attempt_id: "job-1:attempt:1".into(),
            input_digest: "7".into(),
            result: serde_json::json!({"ok": true}),
        };
        let encoded = serde_json::to_string(&submission).unwrap();
        assert_eq!(
            encoded,
            r#"{"job_id":"job-1","attempt_id":"job-1:attempt:1","input_digest":"7","result":{"ok":true}}"#
        );
        let neutral: job::JobResultSubmission = serde_json::from_str(&encoded).unwrap();
        assert_eq!(neutral, submission);
        assert_eq!(
            result_digest(&submission.result, 128).unwrap(),
            job::result_digest(&submission.result, 128).unwrap()
        );
        assert!(
            matches!(result_digest(&submission.result, 4), Err(Error::InvalidInput(detail)) if detail == "Backend Job result exceeds 4 bytes")
        );
        let mut request = request();
        request.job_id = "".into();
        assert!(
            matches!(request.validate(), Err(Error::InvalidInput(detail)) if detail == "Backend Job job_id must be non-empty and trimmed")
        );
        request.job_id = "job-1".into();
        request.limits.timeout_seconds = 0;
        assert!(
            matches!(request.validate(), Err(Error::InvalidInput(detail)) if detail == "Backend Job timeout_seconds must be in 1..=600")
        );
        // Preserve domain-before-input/limits error ordering as well.
        request.grants.subjektiv_consolidation = Some(SubjektivConsolidationGrant {
            subject_id: "".into(),
            candidate_ids: vec![],
        });
        assert!(
            matches!(request.validate(), Err(Error::InvalidInput(detail)) if detail == "Backend Job grant subject_id must be non-empty and trimmed")
        );
    }

    #[test]
    fn fingerprint_covers_input_profile_limits_and_notification_provenance() {
        let request = request();
        let first = request.fingerprint().unwrap();
        assert_eq!(first, request.fingerprint().unwrap());
        for mutate in [
            |request: &mut BackendJobRequest| {
                request.input = serde_json::json!({"title":"changed"})
            },
            |request: &mut BackendJobRequest| request.purpose = "alternate_check".to_string(),
            |request: &mut BackendJobRequest| request.limits.max_attempts = 3,
            |request: &mut BackendJobRequest| request.notification_target = None,
        ] {
            let mut changed = request.clone();
            mutate(&mut changed);
            assert_ne!(first, changed.fingerprint().unwrap());
        }
    }

    #[test]
    fn registry_profile_syntax_accepts_configured_selectors_without_an_allowlist() {
        for profile in [
            "builtin:backend-job",
            "builtin:subjektiv-memory-consolidation",
            "project:custom",
            "user:custom",
            "custom",
            "default",
            "builtin:not-yet-configured",
        ] {
            let mut request = request();
            request.profile = profile.into();
            request.validate().unwrap();
        }
        for profile in [
            "",
            " builtin:backend-job",
            "path:custom",
            "./custom.toml",
            "/tmp/custom",
            "project:../custom",
            "project:custom.toml",
            "builtin:",
            "unknown:custom",
            "project:a:b",
            "C:\\custom",
            "{\"worker\":{}}",
            "inherit",
        ] {
            let mut request = request();
            request.profile = profile.into();
            assert!(request.validate().is_err(), "accepted {profile}");
        }
    }

    #[test]
    fn explicit_grants_are_immutable_bounded_and_derive_a_subject_fence() {
        let mut request = request();
        let original = request.fingerprint().unwrap();
        request.grants.subjektiv_consolidation = Some(SubjektivConsolidationGrant {
            subject_id: "subject-a".into(),
            candidate_ids: vec!["candidate-1".into()],
        });
        assert_ne!(original, request.fingerprint().unwrap());
        assert_eq!(
            request.resource_key(),
            Some(subjektiv_serialization_key("subject-a"))
        );
        let granted = request.fingerprint().unwrap();
        request
            .grants
            .subjektiv_consolidation
            .as_mut()
            .unwrap()
            .candidate_ids
            .push("candidate-2".into());
        assert_ne!(granted, request.fingerprint().unwrap());
        request.serialization_key = Some("another-subject".into());
        assert!(request.validate().is_err());
        request.serialization_key = None;
        request
            .grants
            .subjektiv_consolidation
            .as_mut()
            .unwrap()
            .candidate_ids
            .push("candidate-2".into());
        assert!(request.validate().is_err());
    }

    #[test]
    fn empty_consolidation_grant_keeps_subject_lock_without_candidate_authority() {
        let mut request = request();
        request.grants.subjektiv_consolidation = Some(SubjektivConsolidationGrant {
            subject_id: "subject-a".into(),
            candidate_ids: vec![],
        });
        request.validate().unwrap();
        assert!(
            !request.grants.is_empty(),
            "empty candidate scope still carries a subject grant"
        );
        assert_eq!(
            request.resource_key(),
            Some(subjektiv_serialization_key("subject-a"))
        );
        let encoded = serde_json::to_string(&request).unwrap();
        assert!(encoded.contains("\"candidate_ids\":[]"));
        let decoded: BackendJobRequest = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, request);
        let surface_only = request.fingerprint().unwrap();
        request
            .grants
            .subjektiv_consolidation
            .as_mut()
            .unwrap()
            .candidate_ids = (0..256).map(|index| format!("candidate-{index}")).collect();
        request.validate().unwrap();
        assert_ne!(surface_only, request.fingerprint().unwrap());
        request
            .grants
            .subjektiv_consolidation
            .as_mut()
            .unwrap()
            .candidate_ids
            .push("candidate-256".into());
        assert!(
            request.validate().is_err(),
            "candidate batch remains bounded"
        );
    }

    #[test]
    fn legacy_request_replay_keeps_its_exact_serialization() {
        let request = request();
        let original_json = serde_json::to_string(&request).unwrap();
        assert!(!original_json.contains("grants"));
        assert!(!original_json.contains("serialization_key"));
        let decoded: BackendJobRequest = serde_json::from_str(&original_json).unwrap();
        assert_eq!(decoded.grants, BackendJobGrants::default());
        assert_eq!(
            request.fingerprint().unwrap(),
            decoded.fingerprint().unwrap()
        );
    }

    #[test]
    fn tracked_runtime_ids_are_fixed_size_for_maximum_job_ids() {
        let job_id = "j".repeat(256);
        let attempt_id = attempt_id(&job_id, ABSOLUTE_MAX_ATTEMPTS);
        let run_id = runtime_run_id(&attempt_id);
        let notification_id = delivery_id(&job_id, &attempt_id);

        assert!(run_id.len() <= 128, "run id was {} bytes", run_id.len());
        assert!(
            notification_id.len() <= 128,
            "notification id was {} bytes",
            notification_id.len()
        );
        assert_eq!(run_id, runtime_run_id(&attempt_id));
        assert_eq!(notification_id, delivery_id(&job_id, &attempt_id));
        assert_ne!(run_id, runtime_run_id(&format!("{attempt_id}-other")));
        assert_ne!(
            notification_id,
            delivery_id(&job_id, &format!("{attempt_id}-other"))
        );
    }

    #[test]
    fn result_is_bounded_before_digesting() {
        let value = serde_json::json!({"ok": true});
        let (digest, encoded) = result_digest(&value, 128).unwrap();
        assert!(digest.starts_with("sha256:"));
        assert_eq!(encoded, r#"{"ok":true}"#);
        assert!(result_digest(&serde_json::json!({"body": "too long"}), 4).is_err());
    }

    #[test]
    fn failure_detail_is_bounded_by_utf8_bytes_without_splitting_code_points() {
        let detail = "界".repeat(MAX_JOB_FAILURE_BYTES);
        let bounded = bounded_failure_detail(&detail);
        assert!(bounded.len() <= MAX_JOB_FAILURE_BYTES);
        assert!(detail.starts_with(&bounded));
    }

    #[test]
    fn job_limits_reject_unbounded_execution_and_retry() {
        for limits in [
            BackendJobLimits {
                timeout_seconds: 0,
                ..Default::default()
            },
            BackendJobLimits {
                timeout_seconds: MAX_JOB_TIMEOUT_SECONDS + 1,
                ..Default::default()
            },
            BackendJobLimits {
                max_attempts: 0,
                ..Default::default()
            },
            BackendJobLimits {
                max_attempts: ABSOLUTE_MAX_ATTEMPTS + 1,
                ..Default::default()
            },
            BackendJobLimits {
                max_concurrent_jobs: 0,
                ..Default::default()
            },
        ] {
            assert!(limits.validate().is_err());
        }
    }
}
