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
    /// Trusted Host grant, independent of reusable Profile policy and Job purpose.
    #[serde(default)]
    pub subjektiv_journal: bool,
}

impl BackendJobExecutionBinding {
    /// Subject maintenance Jobs are not the Subject's main conversation Session.
    pub(crate) fn has_subjektiv_job_grant(&self) -> bool {
        self.subjektiv_consolidation || self.subjektiv_journal
    }
    pub fn validate(&self) -> Result<(), &'static str> {
        if [&self.job_id, &self.attempt_id]
            .into_iter()
            .chain(self.input_digest.iter())
            .any(|value| value.trim().is_empty() || value.chars().any(char::is_control))
        {
            return Err("Backend Job binding contains an invalid identity or input digest");
        }
        if self.subjektiv_consolidation && self.subjektiv_journal {
            return Err("Backend Job cannot combine Memory consolidation and Journal grants");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn omitted_journal_grant_is_disabled_and_mixed_subject_grants_are_rejected() {
        let mut binding: BackendJobExecutionBinding = serde_json::from_value(serde_json::json!({
            "job_id":"job", "attempt_id":"attempt", "subjektiv_consolidation":true,
        }))
        .unwrap();
        assert!(!binding.subjektiv_journal);
        binding.subjektiv_journal = true;
        assert!(binding.validate().is_err());
        assert!(
            serde_json::from_value::<BackendJobExecutionBinding>(serde_json::json!({
                "job_id":"job", "attempt_id":"attempt", "purpose":"journal"
            }))
            .is_err(),
            "purpose is not a grant field"
        );
    }
}
