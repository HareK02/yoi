//! Host-authored Job authority, separate from reusable Profile policy.

use serde::{Deserialize, Serialize};

/// Immutable execution capability delivered only by a trusted Backend host.
/// Names, instructions and Profiles never confer this capability.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackendJobExecutionBinding {
    pub job_id: String,
    pub attempt_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_digest: Option<String>,
    #[serde(default)]
    pub subjektiv_consolidation: bool,
}

impl BackendJobExecutionBinding {
    pub fn validate(&self) -> Result<(), &'static str> {
        if [&self.job_id, &self.attempt_id]
            .into_iter()
            .chain(self.input_digest.iter())
            .any(|value| value.trim().is_empty() || value.chars().any(char::is_control))
        {
            return Err("Backend Job binding contains an invalid identity or input digest");
        }
        Ok(())
    }
}
