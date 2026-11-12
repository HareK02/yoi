//! Launch policies own Profile selection and permission references, not resource access.
//! These values are data, never credentials. Management requires the authenticated
//! Workspace owner; using a policy requires both worker.create and its exact usage key.

use crate::authorization::PermissionReferenceEncoding;
use feature_permissions::{PermissionAssignment, PermissionKey};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Opaque authorization identity, independent of display-name edits and the
/// operation key used for mutation CAS. Content reversion must not reuse an identity.
pub fn validate_authorization_key(value: &str) -> Result<(), String> {
    if value.is_empty() || value.len() > 512 || value.chars().any(char::is_control) {
        return Err(
            "policy authorization key must contain 1..512 UTF-8 bytes and no control characters"
                .into(),
        );
    }
    Ok(())
}

/// Shared wire decoder for API requests, persisted grants and model Tool inputs.
pub fn deserialize_authorization_key<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<String, D::Error> {
    let value = String::deserialize(deserializer)?;
    validate_authorization_key(&value).map_err(serde::de::Error::custom)?;
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
    #[schemars(length(min = 1, max = 512))]
    #[serde(deserialize_with = "crate::launch_policy::deserialize_authorization_key")]
    pub authorization_key: String,
    pub state: WorkerLaunchPolicyState,
    pub allowed_profiles: Vec<String>,
    pub child_permissions: PermissionAssignment,
    pub reference_encoding: PermissionReferenceEncoding,
    /// Mutation CAS identity, changed by rename even when authorization is unchanged.
    pub last_operation_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkerLaunchPolicyObservation {
    #[schemars(length(min = 1, max = 512))]
    #[serde(deserialize_with = "crate::launch_policy::deserialize_authorization_key")]
    pub authorization_key: String,
    pub last_operation_key: String,
}

impl WorkerLaunchPolicy {
    /// Current authorization projection; historical receipts retain their saved representation.
    pub fn effective_child_permissions(&self) -> PermissionAssignment {
        self.child_permissions.clone()
    }

    pub fn observation(&self) -> WorkerLaunchPolicyObservation {
        WorkerLaunchPolicyObservation {
            authorization_key: self.authorization_key.clone(),
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
    /// Every replacement issues a new authorization identity, even for identical
    /// content. A revoked identity is never reactivated by reverting content.
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

/// Caller-neutral initial assignment selection. Omission selects the Workspace
/// Worker default; an explicit assignment (including empty) takes precedence.
/// Explicit policy selection remains a separate, usage-authorized launch path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkerCreationAuthorization {
    Policy {
        policy_id: String,
        #[schemars(length(min = 1, max = 512))]
        #[serde(deserialize_with = "crate::launch_policy::deserialize_authorization_key")]
        policy_authorization_key: String,
    },
    Assignment {
        assignment: Option<PermissionAssignment>,
    },
}

impl Default for WorkerCreationAuthorization {
    fn default() -> Self {
        Self::Assignment { assignment: None }
    }
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
            validate_authorization_key(&expected.authorization_key)?;
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
            return Err(
                "policy mutation requires observed authorization identity and operation key".into(),
            );
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
    fn worker_creation_wire_requires_authorization_key_and_rejects_legacy_fields() {
        for input in [
            json!({"kind":"policy", "policy_id":"coder"}),
            json!({"kind":"policy", "policy_id":"coder", "policy_revision":1}),
            json!({"kind":"policy", "policy_id":"coder", "policy_revision":"auth-key"}),
            json!({"kind":"policy", "policy_id":"coder", "policy_authorization_key":"auth-key", "policy_revision":1}),
            json!({"kind":"policy", "policy_id":"coder", "policy_authorization_key":"auth-key", "execution_grant":{}}),
            json!({"kind":"human", "assignment":null}),
            json!({"kind":"assignment", "redelegation":[]}),
        ] {
            assert!(serde_json::from_value::<WorkerCreationAuthorization>(input).is_err());
        }
        let parsed: WorkerCreationAuthorization = serde_json::from_value(
            json!({"kind":"policy", "policy_id":"coder", "policy_authorization_key":"auth-key"}),
        )
        .unwrap();
        assert_eq!(
            parsed,
            WorkerCreationAuthorization::Policy {
                policy_id: "coder".into(),
                policy_authorization_key: "auth-key".into(),
            }
        );
    }

    #[test]
    fn assignment_wire_distinguishes_default_from_explicit_empty_without_human_alias() {
        let omitted: WorkerCreationAuthorization =
            serde_json::from_value(json!({"kind":"assignment"})).unwrap();
        assert_eq!(omitted, WorkerCreationAuthorization::default());
        assert_eq!(
            serde_json::to_value(omitted).unwrap(),
            json!({
                "kind":"assignment", "assignment":null
            })
        );
        let empty = json!({"kind":"assignment", "assignment":{
            "grants":[], "exclusions":[], "revoked":false
        }});
        let parsed: WorkerCreationAuthorization = serde_json::from_value(empty.clone()).unwrap();
        assert_ne!(parsed, WorkerCreationAuthorization::default());
        assert_eq!(serde_json::to_value(parsed).unwrap(), empty);
        assert!(
            serde_json::from_value::<WorkerCreationAuthorization>(
                json!({"kind":"human", "assignment":null})
            )
            .is_err()
        );
    }

    fn policy_wire(authorization_key: serde_json::Value) -> serde_json::Value {
        json!({
            "policy_id":"coder", "name":"Coder", "authorization_key":authorization_key,
            "state":"active", "allowed_profiles":["builtin:coder"],
            "child_permissions":{"grants":[], "exclusions":[], "revoked":false},
            "reference_encoding":"feature_sets_v2", "last_operation_key":"create"
        })
    }

    #[test]
    fn policy_authorization_keys_validate_utf8_bytes_and_preserve_exact_identity() {
        for key in [
            "auth-key".to_owned(),
            "1".to_owned(),
            " key ".to_owned(),
            "x".repeat(512),
            "é".repeat(256),
            "🔑".repeat(128),
        ] {
            validate_authorization_key(&key).unwrap();
            let request =
                json!({"kind":"policy", "policy_id":"coder", "policy_authorization_key":key});
            let parsed: WorkerCreationAuthorization =
                serde_json::from_value(request.clone()).unwrap();
            assert_eq!(serde_json::to_value(parsed).unwrap(), request);
            let raw = policy_wire(json!(key));
            let policy: WorkerLaunchPolicy = serde_json::from_value(raw.clone()).unwrap();
            assert_eq!(serde_json::to_value(&policy).unwrap(), raw);
            let observation = policy.observation();
            assert_eq!(observation.authorization_key, key);
            let saved = serde_json::to_value(&observation).unwrap();
            assert_eq!(
                serde_json::from_value::<WorkerLaunchPolicyObservation>(saved).unwrap(),
                observation
            );
        }
        for key in [
            "".to_owned(),
            "x".repeat(513),
            "é".repeat(257),
            "🔑".repeat(129),
            "bad\nkey".to_owned(),
            "bad\0key".to_owned(),
            "bad\u{7f}key".to_owned(),
            "bad\u{85}key".to_owned(),
        ] {
            assert!(validate_authorization_key(&key).is_err());
            assert_invalid_authorization_key(json!(key));
        }
        for value in [json!(1), json!(null), json!(false), json!([]), json!({})] {
            assert_invalid_authorization_key(value);
        }
    }

    fn assert_invalid_authorization_key(value: serde_json::Value) {
        assert!(
            serde_json::from_value::<WorkerCreationAuthorization>(json!({
                "kind":"policy", "policy_id":"coder", "policy_authorization_key":value
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<WorkerLaunchPolicyObservation>(json!({
                "authorization_key":value, "last_operation_key":"create"
            }))
            .is_err()
        );
        assert!(serde_json::from_value::<WorkerLaunchPolicy>(policy_wire(value)).is_err());
    }

    #[test]
    fn policy_and_observation_reject_old_fields_even_with_current_identity() {
        for old_value in [json!(1), json!("auth-key")] {
            for include_current in [false, true] {
                let mut policy = policy_wire(json!("auth-key"));
                let mut observation =
                    json!({"authorization_key":"auth-key", "last_operation_key":"create"});
                for value in [&mut policy, &mut observation] {
                    if !include_current {
                        value.as_object_mut().unwrap().remove("authorization_key");
                    }
                    value["authorization_revision"] = old_value.clone();
                }
                assert!(serde_json::from_value::<WorkerLaunchPolicy>(policy).is_err());
                assert!(
                    serde_json::from_value::<WorkerLaunchPolicyObservation>(observation).is_err()
                );
            }
        }
    }

    #[test]
    fn policy_authorization_key_schema_exposes_string_bounds() {
        for schema in [
            serde_json::to_value(schemars::schema_for!(WorkerLaunchPolicy)).unwrap(),
            serde_json::to_value(schemars::schema_for!(WorkerLaunchPolicyObservation)).unwrap(),
        ] {
            let key = &schema["properties"]["authorization_key"];
            assert_eq!(key["type"], "string");
            assert_eq!(key["minLength"], 1);
            assert_eq!(key["maxLength"], 512);
            assert!(schema["properties"].get("authorization_revision").is_none());
        }
        let schema =
            serde_json::to_value(schemars::schema_for!(WorkerCreationAuthorization)).unwrap();
        let policy = schema["oneOf"]
            .as_array()
            .unwrap()
            .iter()
            .find(|variant| {
                variant["properties"]
                    .get("policy_authorization_key")
                    .is_some()
            })
            .unwrap();
        let key = &policy["properties"]["policy_authorization_key"];
        assert_eq!(key["type"], "string");
        assert_eq!(key["minLength"], 1);
        assert_eq!(key["maxLength"], 512);
        assert!(policy["properties"].get("policy_revision").is_none());
    }

    #[test]
    fn rename_observation_retains_distinct_authorization_and_operation_keys() {
        let mut policy: WorkerLaunchPolicy =
            serde_json::from_value(policy_wire(json!("auth-key"))).unwrap();
        let before = policy.observation();
        policy.name = "Renamed".into();
        policy.last_operation_key = "rename".into();
        let after = policy.observation();
        assert_eq!(before.authorization_key, after.authorization_key);
        assert_ne!(before.last_operation_key, after.last_operation_key);
        let mut mutation = WorkerLaunchPolicyMutation {
            operation_key: "rename-again".into(),
            policy_id: "coder".into(),
            expected: Some(after),
            action: WorkerLaunchPolicyAction::Rename {
                name: "Coder".into(),
            },
        };
        mutation.validate().unwrap();
        mutation
            .expected
            .as_mut()
            .unwrap()
            .authorization_key
            .clear();
        assert!(mutation.validate().is_err());
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
