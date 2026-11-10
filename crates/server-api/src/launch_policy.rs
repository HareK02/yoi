//! Launch policies own Profile selection and permission references, not resource access.
//! These values are data, never credentials. Management requires the authenticated
//! Workspace owner; using a policy requires both worker.create and its exact usage key.

use crate::authorization::{PermissionReferenceEncoding, migrate_legacy_subject_assignment};
use feature_permissions::{PermissionAssignment, PermissionKey};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Authorization identity changes must remain exactly representable by JSON clients.
/// This identity is independent of display-name edits and never resets on content
/// reversion. Exhaustion fails closed instead of reusing an issued identity.
pub const MAX_AUTHORIZATION_REVISION: u64 = 9_007_199_254_740_991;

pub fn validate_authorization_revision(value: u64) -> Result<(), String> {
    if !(1..=MAX_AUTHORIZATION_REVISION).contains(&value) {
        return Err("policy authorization revision must be a positive JSON safe integer".into());
    }
    Ok(())
}

/// Shared wire decoder for API requests, persisted grants and model Tool inputs.
pub fn deserialize_authorization_revision<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<u64, D::Error> {
    let value = u64::deserialize(deserializer)?;
    validate_authorization_revision(value).map_err(serde::de::Error::custom)?;
    Ok(value)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum WorkerLaunchPolicyState {
    Active,
    Revoked,
    Deleted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkerLaunchPolicy {
    pub policy_id: String,
    pub name: String,
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    #[schemars(range(min = 1, max = 9_007_199_254_740_991_u64))]
    #[serde(deserialize_with = "crate::launch_policy::deserialize_authorization_revision")]
    pub authorization_revision: u64,
    pub state: WorkerLaunchPolicyState,
    pub allowed_profiles: Vec<String>,
    pub child_permissions: PermissionAssignment,
    #[serde(
        default,
        skip_serializing_if = "PermissionReferenceEncoding::is_legacy"
    )]
    pub reference_encoding: PermissionReferenceEncoding,
    pub last_operation_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkerLaunchPolicyObservation {
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    #[schemars(range(min = 1, max = 9_007_199_254_740_991_u64))]
    #[serde(deserialize_with = "crate::launch_policy::deserialize_authorization_revision")]
    pub authorization_revision: u64,
    pub last_operation_key: String,
}

impl WorkerLaunchPolicy {
    /// Current authorization projection; historical receipts retain their saved representation.
    pub fn effective_child_permissions(&self) -> PermissionAssignment {
        let mut assignment = self.child_permissions.clone();
        if self.reference_encoding.is_legacy() {
            migrate_legacy_subject_assignment(&mut assignment);
        }
        assignment
    }

    pub fn observation(&self) -> WorkerLaunchPolicyObservation {
        WorkerLaunchPolicyObservation {
            authorization_revision: self.authorization_revision,
            last_operation_key: self.last_operation_key.clone(),
        }
    }

    pub fn usage_key(&self) -> Result<PermissionKey, String> {
        launch_policy_usage_key(&self.policy_id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkerLaunchPolicyAction {
    Create {
        name: String,
        allowed_profiles: Vec<String>,
        child_permissions: PermissionAssignment,
    },
    Rename {
        name: String,
    },
    /// Every replacement advances the authorization revision, even for identical
    /// content. A revoked revision is never reactivated by reverting content.
    ReplaceAuthorization {
        allowed_profiles: Vec<String>,
        child_permissions: PermissionAssignment,
    },
    Revoke,
    Delete,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkerLaunchPolicyMutation {
    pub operation_key: String,
    pub policy_id: String,
    /// Null only for first creation. The retained operation key fences rename ABA.
    pub expected: Option<WorkerLaunchPolicyObservation>,
    pub action: WorkerLaunchPolicyAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkerLaunchPolicyReceipt {
    pub policy: WorkerLaunchPolicy,
    pub operation_key: String,
}

/// Policy selection is mandatory on Worker-originated creation. Human initial
/// assignment is a separate authenticated path: omission and explicit empty have
/// different meanings there, never on the policy path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkerCreationAuthorization {
    Policy {
        policy_id: String,
        #[cfg_attr(feature = "typescript", ts(type = "number"))]
        #[schemars(range(min = 1, max = 9_007_199_254_740_991_u64))]
        #[serde(deserialize_with = "crate::launch_policy::deserialize_authorization_revision")]
        policy_revision: u64,
    },
    Human {
        assignment: Option<PermissionAssignment>,
    },
}

pub fn validate_policy_id(value: &str) -> Result<(), String> {
    if value.len() > 96
        || value.is_empty()
        || !value.as_bytes()[0].is_ascii_lowercase()
        || !value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
    {
        return Err(
            "policy ID must be a lowercase stable permission-key segment (1..96 bytes)".into(),
        );
    }
    Ok(())
}

pub fn launch_policy_usage_key(policy_id: &str) -> Result<PermissionKey, String> {
    validate_policy_id(policy_id)?;
    PermissionKey::parse(format!("worker.launch.{policy_id}"))
}

/// Syntactic validation only. The Host must additionally resolve the exact
/// source-qualified selector in the current registry and bind the resolved bundle.
pub fn validate_canonical_profile(value: &str) -> Result<(), String> {
    let Some((source, name)) = value.split_once(':') else {
        return Err("Profile must have a canonical builtin: or project: selector".into());
    };
    if !matches!(source, "builtin" | "project")
        || name.is_empty()
        || value.len() > 256
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    {
        return Err("invalid canonical Profile selector".into());
    }
    Ok(())
}

impl WorkerLaunchPolicyMutation {
    pub fn validate(&self) -> Result<(), String> {
        validate_policy_id(&self.policy_id)?;
        if let Some(expected) = &self.expected {
            validate_authorization_revision(expected.authorization_revision)?;
        }
        if self.operation_key.trim().is_empty()
            || self.operation_key.len() > 256
            || self.operation_key.chars().any(char::is_control)
        {
            return Err("invalid launch policy operation key".into());
        }
        match &self.action {
            WorkerLaunchPolicyAction::Create {
                name,
                allowed_profiles,
                ..
            } => {
                validate_name(name)?;
                validate_profiles(allowed_profiles)?;
                if self.expected.is_some() {
                    return Err("create requires null observation".into());
                }
            }
            WorkerLaunchPolicyAction::Rename { name } => validate_name(name)?,
            WorkerLaunchPolicyAction::ReplaceAuthorization {
                allowed_profiles, ..
            } => validate_profiles(allowed_profiles)?,
            _ => {}
        }
        if !matches!(self.action, WorkerLaunchPolicyAction::Create { .. })
            && self.expected.is_none()
        {
            return Err("policy mutation requires observed revision and operation key".into());
        }
        Ok(())
    }
}

fn validate_name(name: &str) -> Result<(), String> {
    if name.trim().is_empty() || name.len() > 256 || name.chars().any(char::is_control) {
        return Err("invalid launch policy display name".into());
    }
    Ok(())
}

fn validate_profiles(profiles: &[String]) -> Result<(), String> {
    if profiles.is_empty() || profiles.len() > 256 {
        return Err("policy requires 1..256 explicit Profiles".into());
    }
    let mut unique = std::collections::BTreeSet::new();
    for profile in profiles {
        validate_canonical_profile(profile)?;
        if !unique.insert(profile) {
            return Err("duplicate policy Profile".into());
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkerLaunchPoliciesResponse {
    pub policies: Vec<WorkerLaunchPolicy>,
}

impl api_macros::openapi::OpenApiSchema for WorkerLaunchPoliciesResponse {}
impl api_macros::openapi::OpenApiSchema for WorkerLaunchPolicyMutation {}
impl api_macros::openapi::OpenApiSchema for WorkerLaunchPolicyReceipt {}

/// Transport only. The Workspace Host must authenticate owner management and
/// validate selectors/assignments against the current registry before mutation.
#[api_macros::api(reqwest, axum, openapi)]
pub trait WorkerLaunchPoliciesApi {
    #[get("/api/w/{workspace_id}/worker-launch-policies", status = 200, error_status = 403, additional_error_statuses = [400, 404, 500], bearer_auth = true, browser_auth = true)]
    async fn worker_launch_policies(
        &self,
        #[extension] context: crate::ServerRequestContext,
        #[path] workspace_id: String,
    ) -> Result<WorkerLaunchPoliciesResponse, crate::RepositoryApiError>;

    #[post("/api/w/{workspace_id}/worker-launch-policies", status = 200, error_status = 403, additional_error_statuses = [400, 404, 409, 500], bearer_auth = true, browser_auth = true, normalize_body_errors = true)]
    async fn mutate_worker_launch_policy(
        &self,
        #[extension] context: crate::ServerRequestContext,
        #[path] workspace_id: String,
        #[body] request: WorkerLaunchPolicyMutation,
    ) -> Result<WorkerLaunchPolicyReceipt, crate::RepositoryApiError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn policy_projection_only_interprets_retired_references_for_legacy_encoding() {
        let mut raw = json!({
            "policy_id":"coder", "name":"Coder", "authorization_revision":1,
            "state":"active", "allowed_profiles":["builtin:coder"],
            "child_permissions": {
                "grants":[{"kind":"set", "value":"subject-default"}],
                "exclusions":[{"kind":"namespace", "value": {
                    "namespace":"subject", "keys":["subject.delegate"]
                }}],
                "revoked":false
            },
            "last_operation_key":"create"
        });
        let legacy: WorkerLaunchPolicy = serde_json::from_value(raw.clone()).unwrap();
        assert!(legacy.reference_encoding.is_legacy());
        assert_ne!(
            legacy.effective_child_permissions(),
            legacy.child_permissions
        );
        assert!(
            serde_json::to_value(&legacy)
                .unwrap()
                .get("reference_encoding")
                .is_none()
        );
        raw["reference_encoding"] = json!("feature_sets_v2");
        let current: WorkerLaunchPolicy = serde_json::from_value(raw).unwrap();
        assert_eq!(
            current.effective_child_permissions(),
            current.child_permissions
        );
        assert_eq!(current.child_permissions, legacy.child_permissions);
    }

    #[test]
    fn worker_creation_wire_requires_policy_revision_and_rejects_arbitrary_grants() {
        for input in [
            json!({"kind":"policy", "policy_id":"coder"}),
            json!({"kind":"policy", "policy_id":"coder", "policy_revision":1, "execution_grant":{}}),
            json!({"kind":"human", "redelegation":[]}),
        ] {
            assert!(serde_json::from_value::<WorkerCreationAuthorization>(input).is_err());
        }
        let parsed: WorkerCreationAuthorization = serde_json::from_value(
            json!({"kind":"policy", "policy_id":"coder", "policy_revision":1}),
        )
        .unwrap();
        assert_eq!(
            parsed,
            WorkerCreationAuthorization::Policy {
                policy_id: "coder".into(),
                policy_revision: 1
            }
        );
    }

    #[test]
    fn policy_authorization_wire_bounds_match_schema_and_saved_observations() {
        for value in [0, MAX_AUTHORIZATION_REVISION + 1, u64::MAX] {
            assert!(
                serde_json::from_value::<WorkerCreationAuthorization>(json!({
                    "kind":"policy", "policy_id":"coder", "policy_revision":value
                }))
                .is_err()
            );
            assert!(
                serde_json::from_value::<WorkerLaunchPolicyObservation>(json!({
                    "authorization_revision":value, "last_operation_key":"create"
                }))
                .is_err()
            );
        }
        for value in [1, MAX_AUTHORIZATION_REVISION] {
            let request = json!({"kind":"policy", "policy_id":"coder", "policy_revision":value});
            let parsed: WorkerCreationAuthorization =
                serde_json::from_value(request.clone()).unwrap();
            assert_eq!(serde_json::to_value(parsed).unwrap(), request);
        }
        let schema =
            serde_json::to_value(schemars::schema_for!(WorkerLaunchPolicyObservation)).unwrap();
        assert_eq!(schema["properties"]["authorization_revision"]["minimum"], 1);
        assert_eq!(
            schema["properties"]["authorization_revision"]["maximum"],
            MAX_AUTHORIZATION_REVISION
        );
    }

    #[test]
    fn policy_selectors_require_exact_source_and_stable_key_segments() {
        for invalid in [
            "coder",
            "inherit",
            "default",
            "project:../coder",
            "builtin:",
            "file:coder",
        ] {
            assert!(validate_canonical_profile(invalid).is_err(), "{invalid}");
        }
        assert!(validate_canonical_profile("builtin:coder").is_ok());
        assert_ne!("builtin:coder", "project:coder");
        assert_eq!(
            launch_policy_usage_key("coder-v1").unwrap().as_str(),
            "worker.launch.coder-v1"
        );
        assert!(launch_policy_usage_key("coder.nested").is_err());
    }
}
