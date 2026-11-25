//! Subjektiv-owned initialization payloads. The common launch envelope does not
//! interpret these identities or infer attachment authority from their presence.
//!
//! Request decoding is not authorization. The Backend must resolve the current
//! Subject/Profile/singleton relationship before emitting a resolved connection;
//! Runtime must compare the saved connection with its fresh Host capability.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const FEATURE_ID: &str = "builtin:subjektiv";
pub const VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkerConnectionRequest {
    pub subject_id: String,
}

impl WorkerConnectionRequest {
    pub fn validate(&self) -> Result<(), String> {
        validate_identity(&self.subject_id, "Subject")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum WorkerConnectionKind {
    SubjectBody,
    MemoryConsolidation,
}

/// A stable binding, never a credential or cached authorization decision. The
/// Subject identity is required for both body and consolidation connections;
/// consolidation must also match the independent current Job grant. Ordinary
/// Memory Jobs have no Subjektiv connection rather than an unspecified Subject.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct ResolvedWorkerConnection {
    pub workspace_id: String,
    pub subject_id: Option<String>,
    pub kind: WorkerConnectionKind,
}

impl ResolvedWorkerConnection {
    pub fn validate(&self) -> Result<(), String> {
        validate_identity(&self.workspace_id, "Workspace")?;
        let subject = self.subject_id.as_deref().ok_or_else(|| {
            "Subjektiv initialization is missing its granted Subject identity".to_owned()
        })?;
        validate_identity(subject, "Subject")?;
        Ok(())
    }
}

fn validate_identity(value: &str, kind: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 256
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        return Err(format!("invalid {kind} initialization identity"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn public_payload_cannot_self_attest_a_resolved_connection() {
        assert!(
            serde_json::from_value::<WorkerConnectionRequest>(json!({
                "subject_id":"subject-a", "workspace_id":"workspace-a", "kind":"subject_body"
            }))
            .is_err()
        );
        assert!(serde_json::from_value::<ResolvedWorkerConnection>(json!({
            "workspace_id":"workspace-a", "subject_id":"subject-a", "kind":"subject_body", "credential":"forged"
        })).is_err());
        assert!(
            serde_json::from_value::<ResolvedWorkerConnection>(json!({
                "workspace_id":"workspace-a", "subject_id":"subject-a", "kind":"ordinary_worker"
            }))
            .is_err()
        );
    }

    #[test]
    fn resolved_body_requires_stable_subject_and_workspace_identities() {
        for (workspace, subject) in [
            ("", Some("subject-a")),
            ("workspace-a", None),
            ("workspace-a", Some(" ")),
            ("workspace-a", Some("subject\nsecret")),
        ] {
            assert!(
                ResolvedWorkerConnection {
                    workspace_id: workspace.into(),
                    subject_id: subject.map(str::to_owned),
                    kind: WorkerConnectionKind::SubjectBody
                }
                .validate()
                .is_err()
            );
        }
        assert!(
            ResolvedWorkerConnection {
                workspace_id: "workspace-a".into(),
                subject_id: Some("subject-a".into()),
                kind: WorkerConnectionKind::SubjectBody
            }
            .validate()
            .is_ok()
        );
        for kind in [
            WorkerConnectionKind::SubjectBody,
            WorkerConnectionKind::MemoryConsolidation,
        ] {
            let mut connection = ResolvedWorkerConnection {
                workspace_id: "workspace-a".into(),
                subject_id: None,
                kind,
            };
            assert!(connection.validate().is_err());
            connection.subject_id = Some("subject-a".into());
            assert!(connection.validate().is_ok());
        }
    }
}
