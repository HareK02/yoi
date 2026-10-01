//! Durable Backend-owned jobs executed by ordinary Runtime-managed Workers.
//!
//! A job is durable intent. An attempt binds that intent to one dedicated
//! Worker, and a result is accepted only through the fenced structured-result
//! API below. Runtime lifecycle and final prose are deliberately not success
//! authority.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use worker_runtime::identity::RuntimeWorkerRef;

use crate::{Error, Result};

pub const BACKEND_JOB_RUNTIME_ID: &str = crate::hosts::EMBEDDED_RUNTIME_ID;
pub const DEFAULT_MAX_CONCURRENT_JOBS: u16 = 8;
pub const ABSOLUTE_MAX_CONCURRENT_JOBS: u16 = 64;
pub const DEFAULT_JOB_TIMEOUT_SECONDS: u32 = 120;
pub const MAX_JOB_TIMEOUT_SECONDS: u32 = 600;
pub const DEFAULT_MAX_RESULT_BYTES: u32 = 16 * 1024;
pub const ABSOLUTE_MAX_RESULT_BYTES: u32 = 64 * 1024;
pub const DEFAULT_MAX_ATTEMPTS: u8 = 2;
pub const ABSOLUTE_MAX_ATTEMPTS: u8 = 3;
pub const MAX_JOB_INPUT_BYTES: usize = 128 * 1024;
pub const MAX_JOB_INSTRUCTION_BYTES: usize = 16 * 1024;
pub const MAX_JOB_PURPOSE_BYTES: usize = 120;
pub const MAX_JOB_REFERENCE_BYTES: usize = 512;
pub const MAX_JOB_FAILURE_BYTES: usize = 1024;
pub const MAX_JOB_DELIVERY_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BackendJobState {
    Pending,
    Completed,
    Failed,
    Unknown,
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
    Dispatched,
    Completed,
    Failed,
    Unknown,
}

impl BackendJobAttemptState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Reserved => "reserved",
            Self::Dispatched => "dispatched",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Unknown => "unknown",
        }
    }

    pub(crate) fn parse(value: &str) -> Result<Self> {
        match value {
            "reserved" => Ok(Self::Reserved),
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
    Completed,
    Failed,
}

impl BackendJobDeliveryState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }

    pub(crate) fn parse(value: &str) -> Result<Self> {
        match value {
            "pending" => Ok(Self::Pending),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
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
        if self.max_concurrent_jobs == 0 || self.max_concurrent_jobs > ABSOLUTE_MAX_CONCURRENT_JOBS
        {
            return Err(Error::InvalidInput(format!(
                "Backend Job max_concurrent_jobs must be in 1..={ABSOLUTE_MAX_CONCURRENT_JOBS}"
            )));
        }
        if self.timeout_seconds == 0 || self.timeout_seconds > MAX_JOB_TIMEOUT_SECONDS {
            return Err(Error::InvalidInput(format!(
                "Backend Job timeout_seconds must be in 1..={MAX_JOB_TIMEOUT_SECONDS}"
            )));
        }
        if self.max_result_bytes == 0 || self.max_result_bytes > ABSOLUTE_MAX_RESULT_BYTES {
            return Err(Error::InvalidInput(format!(
                "Backend Job max_result_bytes must be in 1..={ABSOLUTE_MAX_RESULT_BYTES}"
            )));
        }
        if self.max_attempts == 0 || self.max_attempts > ABSOLUTE_MAX_ATTEMPTS {
            return Err(Error::InvalidInput(format!(
                "Backend Job max_attempts must be in 1..={ABSOLUTE_MAX_ATTEMPTS}"
            )));
        }
        Ok(())
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BackendJobRequest {
    /// Stable caller-selected idempotency key within one Workspace.
    pub job_id: String,
    /// Stable machine-readable use-site name, for example `ticket_item_check`.
    pub purpose: String,
    /// Revision fenced by the structured result.
    pub input_revision: String,
    /// Durable domain reference used to reload or explain the input.
    pub input_ref: String,
    /// Bounded immutable input snapshot. This must not contain credentials.
    pub input: Value,
    /// Bounded task instruction; the profile supplies the system instruction.
    pub instruction: String,
    /// Existing configured profile selector. Job dispatch never accepts raw profile source.
    pub profile: String,
    /// Writer provenance and optional advisory destination, not parent ownership.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_worker: Option<RuntimeWorkerRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notification_target: Option<RuntimeWorkerRef>,
    #[serde(default)]
    pub limits: BackendJobLimits,
}

impl BackendJobRequest {
    pub fn validate(&self) -> Result<()> {
        validate_identifier("job_id", &self.job_id, 256)?;
        validate_identifier("purpose", &self.purpose, MAX_JOB_PURPOSE_BYTES)?;
        validate_identifier("input_revision", &self.input_revision, 256)?;
        validate_bounded_text("input_ref", &self.input_ref, MAX_JOB_REFERENCE_BYTES)?;
        validate_bounded_text("instruction", &self.instruction, MAX_JOB_INSTRUCTION_BYTES)?;
        validate_identifier("profile", &self.profile, 256)?;
        if self.profile != "builtin:backend-job" {
            return Err(Error::InvalidInput(
                "Backend Jobs must use the dedicated builtin:backend-job profile".to_string(),
            ));
        }
        let input = serde_json::to_vec(&self.input).map_err(|error| {
            Error::InvalidInput(format!("serialize Backend Job input: {error}"))
        })?;
        if input.len() > MAX_JOB_INPUT_BYTES {
            return Err(Error::InvalidInput(format!(
                "Backend Job input exceeds {MAX_JOB_INPUT_BYTES} bytes"
            )));
        }
        self.limits.validate()
    }

    pub fn fingerprint(&self) -> Result<String> {
        self.validate()?;
        let encoded = serde_json::to_vec(self).map_err(|error| {
            Error::InvalidInput(format!("serialize Backend Job intent: {error}"))
        })?;
        Ok(sha256(&encoded))
    }

    pub fn worker_input(&self, attempt_id: &str) -> Result<String> {
        let input = serde_json::to_string(&self.input).map_err(|error| {
            Error::InvalidInput(format!("serialize Backend Job input: {error}"))
        })?;
        Ok(format!(
            "{}\n\nBackend Job envelope (immutable):\njob_id: {}\nattempt_id: {}\ninput_revision: {}\ninput_ref: {}\ninput_json: {}\n\nReturn success only through the structured Backend Job result capability. Final prose and Worker Idle/Stopped state are not result authority.",
            self.instruction, self.job_id, attempt_id, self.input_revision, self.input_ref, input
        ))
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BackendJobAttemptRecord {
    pub workspace_id: String,
    pub job_id: String,
    pub attempt_id: String,
    pub attempt: u8,
    pub input_revision: String,
    pub state: BackendJobAttemptState,
    pub worker: Option<RuntimeWorkerRef>,
    pub runtime_run_id: Option<String>,
    pub dispatched_at: Option<String>,
    pub deadline_at: String,
    pub result: Option<Value>,
    pub result_digest: Option<String>,
    pub failure_category: Option<String>,
    pub failure_detail: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BackendJobReservation {
    pub job: BackendJobRecord,
    pub attempt: BackendJobAttemptRecord,
    pub replayed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BackendJobResultSubmission {
    pub job_id: String,
    pub attempt_id: String,
    pub input_revision: String,
    pub result: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BackendJobResultAcceptance {
    pub job: BackendJobRecord,
    pub attempt: BackendJobAttemptRecord,
    pub replayed: bool,
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

pub fn attempt_id(job_id: &str, attempt: u8) -> String {
    format!("{job_id}:attempt:{attempt}")
}

pub fn allocation_key(attempt_id: &str) -> String {
    format!("backend-job:{attempt_id}")
}

pub fn result_digest(result: &Value, max_bytes: u32) -> Result<(String, String)> {
    let encoded = serde_json::to_string(result)
        .map_err(|error| Error::InvalidInput(format!("serialize Backend Job result: {error}")))?;
    if encoded.len() > max_bytes as usize {
        return Err(Error::InvalidInput(format!(
            "Backend Job result exceeds {max_bytes} bytes"
        )));
    }
    Ok((sha256(encoded.as_bytes()), encoded))
}

pub(crate) fn bounded_failure_detail(detail: &str) -> String {
    if detail.len() <= MAX_JOB_FAILURE_BYTES {
        return detail.to_string();
    }
    let mut end = MAX_JOB_FAILURE_BYTES;
    while !detail.is_char_boundary(end) {
        end -= 1;
    }
    detail[..end].to_string()
}

fn validate_identifier(field: &str, value: &str, max_bytes: usize) -> Result<()> {
    validate_bounded_text(field, value, max_bytes)?;
    if value.chars().any(char::is_control) {
        return Err(Error::InvalidInput(format!(
            "Backend Job {field} must not contain control characters"
        )));
    }
    Ok(())
}

fn validate_bounded_text(field: &str, value: &str, max_bytes: usize) -> Result<()> {
    if value.is_empty() || value.trim() != value {
        return Err(Error::InvalidInput(format!(
            "Backend Job {field} must be non-empty and trimmed"
        )));
    }
    if value.len() > max_bytes {
        return Err(Error::InvalidInput(format!(
            "Backend Job {field} exceeds {max_bytes} bytes"
        )));
    }
    Ok(())
}

fn sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(7 + digest.len() * 2);
    output.push_str("sha256:");
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> BackendJobRequest {
        BackendJobRequest {
            job_id: "ticket-check:T-1:r7".to_string(),
            purpose: "ticket_item_check".to_string(),
            input_revision: "7".to_string(),
            input_ref: "ticket://T-1/revisions/7".to_string(),
            input: serde_json::json!({"title": "Check me"}),
            instruction: "Check this immutable Ticket snapshot.".to_string(),
            profile: "builtin:backend-job".to_string(),
            source_worker: Some(RuntimeWorkerRef::new("runtime-a", "worker-a")),
            notification_target: Some(RuntimeWorkerRef::new("runtime-a", "worker-a")),
            limits: BackendJobLimits::default(),
        }
    }

    #[test]
    fn fingerprint_covers_revision_profile_limits_and_notification_provenance() {
        let request = request();
        let first = request.fingerprint().unwrap();
        assert_eq!(first, request.fingerprint().unwrap());
        for mutate in [
            |request: &mut BackendJobRequest| request.input_revision = "8".to_string(),
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
