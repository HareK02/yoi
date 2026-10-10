//! Transport-neutral immutable Job intent and structured-result contract.
//!
//! Hosts own execution, authority, persistence and lifecycle state. This crate
//! deliberately contains no runner, state machine, Runtime or domain grants.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

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

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum JobError {
    #[error("{0}")]
    InvalidInput(String),
}

pub type Result<T> = std::result::Result<T, JobError>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct JobLimits {
    #[serde(default = "default_max_concurrent_jobs")]
    pub max_concurrent_jobs: u16,
    #[serde(default = "default_job_timeout_seconds")]
    pub timeout_seconds: u32,
    #[serde(default = "default_max_result_bytes")]
    pub max_result_bytes: u32,
    #[serde(default = "default_max_attempts")]
    pub max_attempts: u8,
}

impl Default for JobLimits {
    fn default() -> Self {
        Self {
            max_concurrent_jobs: DEFAULT_MAX_CONCURRENT_JOBS,
            timeout_seconds: DEFAULT_JOB_TIMEOUT_SECONDS,
            max_result_bytes: DEFAULT_MAX_RESULT_BYTES,
            max_attempts: DEFAULT_MAX_ATTEMPTS,
        }
    }
}

impl JobLimits {
    pub fn validate(&self) -> Result<()> {
        if self.max_concurrent_jobs == 0 || self.max_concurrent_jobs > ABSOLUTE_MAX_CONCURRENT_JOBS
        {
            return Err(JobError::InvalidInput(format!(
                "Job max_concurrent_jobs must be in 1..={ABSOLUTE_MAX_CONCURRENT_JOBS}"
            )));
        }
        if self.timeout_seconds == 0 || self.timeout_seconds > MAX_JOB_TIMEOUT_SECONDS {
            return Err(JobError::InvalidInput(format!(
                "Job timeout_seconds must be in 1..={MAX_JOB_TIMEOUT_SECONDS}"
            )));
        }
        if self.max_result_bytes == 0 || self.max_result_bytes > ABSOLUTE_MAX_RESULT_BYTES {
            return Err(JobError::InvalidInput(format!(
                "Job max_result_bytes must be in 1..={ABSOLUTE_MAX_RESULT_BYTES}"
            )));
        }
        if self.max_attempts == 0 || self.max_attempts > ABSOLUTE_MAX_ATTEMPTS {
            return Err(JobError::InvalidInput(format!(
                "Job max_attempts must be in 1..={ABSOLUTE_MAX_ATTEMPTS}"
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
pub struct JobRequest {
    /// Stable caller-selected idempotency key within the host's namespace.
    pub job_id: String,
    pub purpose: String,
    pub input_ref: String,
    /// Bounded immutable snapshot; must not contain credentials.
    pub input: Value,
    pub instruction: String,
    /// Manifest registry selector, never raw source or a filesystem path.
    pub profile: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serialization_key: Option<String>,
    #[serde(default)]
    pub limits: JobLimits,
}

impl JobRequest {
    pub fn validate(&self) -> Result<()> {
        self.validate_fields()?;
        self.validate_input()?;
        self.limits.validate()
    }

    /// Request metadata validation, separable so adapters can retain their
    /// domain-validation ordering before validating input and limits.
    pub fn validate_fields(&self) -> Result<()> {
        validate_identifier("job_id", &self.job_id, 256)?;
        validate_identifier("purpose", &self.purpose, MAX_JOB_PURPOSE_BYTES)?;
        validate_bounded_text("input_ref", &self.input_ref, MAX_JOB_REFERENCE_BYTES)?;
        validate_bounded_text("instruction", &self.instruction, MAX_JOB_INSTRUCTION_BYTES)?;
        self.profile_selector()?;
        if let Some(key) = &self.serialization_key {
            validate_identifier("serialization_key", key, MAX_JOB_REFERENCE_BYTES)?;
        }
        Ok(())
    }

    pub fn validate_input(&self) -> Result<()> {
        let input = serde_json::to_vec(&self.input)
            .map_err(|error| JobError::InvalidInput(format!("serialize Job input: {error}")))?;
        if input.len() > MAX_JOB_INPUT_BYTES {
            return Err(JobError::InvalidInput(format!(
                "Job input exceeds {MAX_JOB_INPUT_BYTES} bytes"
            )));
        }
        Ok(())
    }

    /// Syntax only; registry existence and resolution are host authority.
    pub fn profile_selector(&self) -> Result<manifest::ProfileSelector> {
        profile_selector(&self.profile)
    }

    pub fn fingerprint(&self) -> Result<String> {
        self.validate()?;
        fingerprint(self)
    }

    /// Content identity for result fencing; hosts compare it with the reserved
    /// attempt's input, never with a caller-selected counter.
    pub fn input_digest(&self) -> Result<String> {
        self.validate_input()?;
        fingerprint(&self.input)
    }

    pub fn worker_input(&self, attempt_id: &str) -> Result<String> {
        worker_input(self, attempt_id)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct JobResultSubmission {
    pub job_id: String,
    pub attempt_id: String,
    /// SHA-256 of the exact JSON encoding of the immutable input snapshot.
    pub input_digest: String,
    pub result: Value,
}

/// Client-facing lifecycle vocabulary, not a runner or transition policy.
/// A variant's presence does not imply every host supports that operation.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Pending,
    Completed,
    Failed,
    Unknown,
    Cancelled,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JobAttemptState {
    Reserved,
    Dispatching,
    Dispatched,
    Completed,
    Failed,
    Unknown,
    Cancelled,
}

/// Transport-neutral read snapshot. Job status/result and the corresponding
/// attempt identity/status remain distinct; host identities, delivery and
/// resource cleanup are deliberately not part of the model outcome.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct JobOutcome {
    pub job_id: String,
    /// SHA-256 of the exact JSON encoding of the immutable input snapshot.
    pub input_digest: String,
    pub attempt_id: String,
    pub attempt: u8,
    pub state: JobState,
    pub attempt_state: JobAttemptState,
    pub result: Option<Value>,
    pub failure_category: Option<String>,
    pub failure_detail: Option<String>,
}

pub fn profile_selector(profile: &str) -> Result<manifest::ProfileSelector> {
    use manifest::ProfileSelector;
    validate_identifier("profile", profile, 256)?;
    let selector = ProfileSelector::parse_cli(profile);
    let valid = match &selector {
        ProfileSelector::Default => true,
        ProfileSelector::Path { .. } => false,
        ProfileSelector::Named { source, name } => {
            !name.is_empty()
                && !name.starts_with('.')
                && !name.contains(['/', '\\', ':'])
                && name
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
                && ![".toml", ".json", ".dcdl", ".nix"]
                    .iter()
                    .any(|suffix| name.ends_with(suffix))
                && (source.is_some() || name != "inherit")
        }
    };
    if !valid {
        return Err(JobError::InvalidInput(
            "Job profile must be a manifest registry selector, not raw source or a path".into(),
        ));
    }
    Ok(selector)
}

/// Hash the full serialized host intent, not just its neutral projection.
/// Callers must validate the complete intent first. JSON field ordering and
/// omission rules are intentionally preserved for durable replay compatibility.
pub fn fingerprint<T: Serialize + ?Sized>(intent: &T) -> Result<String> {
    let encoded = serde_json::to_vec(intent)
        .map_err(|error| JobError::InvalidInput(format!("serialize Job intent: {error}")))?;
    Ok(sha256(&encoded))
}

pub fn worker_input(request: &JobRequest, attempt_id: &str) -> Result<String> {
    worker_input_with_label(request, attempt_id, "Job")
}

/// Adapter-specific capability label; only the host, never the model, selects it.
/// Keeps an existing host's prompt envelope byte-for-byte compatible.
pub fn worker_input_with_label(
    request: &JobRequest,
    attempt_id: &str,
    label: &str,
) -> Result<String> {
    let input = serde_json::to_string(&request.input)
        .map_err(|error| JobError::InvalidInput(format!("serialize Job input: {error}")))?;
    Ok(format!(
        "{}\n\n{label} envelope (immutable):\njob_id: {}\nattempt_id: {}\ninput_digest: {}\ninput_ref: {}\ninput_json: {}\n\nReturn success only through the structured {label} result capability. Final prose and Worker Idle/Stopped state are not result authority.",
        request.instruction,
        request.job_id,
        attempt_id,
        request.input_digest()?,
        request.input_ref,
        input
    ))
}

pub fn attempt_id(job_id: &str, attempt: u8) -> String {
    format!("{job_id}:attempt:{attempt}")
}

/// Bound the encoded JSON before hashing. Returns (digest, exact JSON).
pub fn result_digest(result: &Value, max_bytes: u32) -> Result<(String, String)> {
    let encoded = serde_json::to_string(result)
        .map_err(|error| JobError::InvalidInput(format!("serialize Job result: {error}")))?;
    if encoded.len() > max_bytes as usize {
        return Err(JobError::InvalidInput(format!(
            "Job result exceeds {max_bytes} bytes"
        )));
    }
    Ok((sha256(encoded.as_bytes()), encoded))
}

pub fn bounded_failure_detail(detail: &str) -> String {
    if detail.len() <= MAX_JOB_FAILURE_BYTES {
        return detail.to_string();
    }
    let mut end = MAX_JOB_FAILURE_BYTES;
    while !detail.is_char_boundary(end) {
        end -= 1;
    }
    detail[..end].to_string()
}

pub fn validate_identifier(field: &str, value: &str, max_bytes: usize) -> Result<()> {
    validate_bounded_text(field, value, max_bytes)?;
    if value.chars().any(char::is_control) {
        return Err(JobError::InvalidInput(format!(
            "Job {field} must not contain control characters"
        )));
    }
    Ok(())
}

pub fn validate_bounded_text(field: &str, value: &str, max_bytes: usize) -> Result<()> {
    if value.is_empty() || value.trim() != value {
        return Err(JobError::InvalidInput(format!(
            "Job {field} must be non-empty and trimmed"
        )));
    }
    if value.len() > max_bytes {
        return Err(JobError::InvalidInput(format!(
            "Job {field} exceeds {max_bytes} bytes"
        )));
    }
    Ok(())
}

pub fn sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(7 + digest.len() * 2);
    output.push_str("sha256:");
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
    }
    output
}
