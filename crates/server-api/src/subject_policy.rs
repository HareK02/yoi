//! Individual Subject policy transport, separate from Workspace configuration.
//! The Server binds management authority to authenticated request context; stored
//! encoding and legacy constraints are response data, not mutation inputs.
//!
//! Response `change_id` identifies the current state even when `policy` is absent.
//! It matches the policy's `change_id` when present; removal retains a tombstone
//! identity rather than resetting to `None`, preventing absent-state ABA on CAS.

use crate::authorization::CurrentSubjectPolicy;
use feature_permissions::PermissionAssignment;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct SubjectPermissionPolicyMutation {
    pub operation_id: String,
    /// Compare with the observed response state identity, including removal tombstones.
    pub expected_change_id: Option<String>,
    pub action: SubjectPermissionPolicyAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SubjectPermissionPolicyAction {
    Replace {
        execution: PermissionAssignment,
        delegation: PermissionAssignment,
    },
    Remove,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct SubjectPermissionPolicyResponse {
    pub change_id: Option<String>,
    pub policy: Option<CurrentSubjectPolicy>,
    pub can_manage: bool,
    pub catalog: feature_permissions::PermissionCatalog,
    pub execution_resolution: feature_permissions::PermissionResolution,
    pub delegation_resolution: feature_permissions::PermissionResolution,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct SubjectPermissionPolicyMutationResponse {
    pub change_id: Option<String>,
    pub policy: Option<CurrentSubjectPolicy>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn replace_mutation() -> serde_json::Value {
        json!({
            "operation_id": "replace-policy",
            "expected_change_id": "observed-change",
            "action": {
                "kind": "replace",
                "execution": {
                    "grants": [{"kind": "set", "value": "conversation"}],
                    "exclusions": [],
                    "revoked": false
                },
                "delegation": {
                    "grants": [{"kind": "set", "value": "work-execution"}],
                    "exclusions": [],
                    "revoked": true
                }
            }
        })
    }

    #[test]
    fn mutations_roundtrip_tagged_actions_and_policy_observation() {
        let mut recreate = replace_mutation();
        recreate["operation_id"] = json!("recreate-policy");
        recreate["expected_change_id"] = json!("removed-change");
        for value in [
            replace_mutation(),
            recreate,
            json!({
                "operation_id": "remove-policy",
                "expected_change_id": null,
                "action": {"kind": "remove"}
            }),
        ] {
            let parsed: SubjectPermissionPolicyMutation =
                serde_json::from_value(value.clone()).unwrap();
            assert_eq!(serde_json::to_value(parsed).unwrap(), value);
            let mut missing_operation = value;
            missing_operation
                .as_object_mut()
                .unwrap()
                .remove("operation_id");
            assert!(
                serde_json::from_value::<SubjectPermissionPolicyMutation>(missing_operation)
                    .is_err()
            );
        }
    }

    #[test]
    fn mutations_reject_stored_policy_metadata_and_actor_claims() {
        for (field, value) in [
            ("change_id", json!("forged-change")),
            ("legacy_constraints", json!(null)),
            ("reference_encoding", json!("feature_sets_v2")),
            ("policy", json!(null)),
            ("actor", json!("owner")),
        ] {
            for in_action in [false, true] {
                let mut invalid = replace_mutation();
                let target = if in_action {
                    &mut invalid["action"]
                } else {
                    &mut invalid
                };
                target[field] = value.clone();
                assert!(
                    serde_json::from_value::<SubjectPermissionPolicyMutation>(invalid).is_err(),
                    "{field}, in_action={in_action}"
                );
            }
        }
    }

    #[test]
    fn responses_roundtrip_initial_current_and_removed_state_identities() {
        let mutation = replace_mutation();
        let policy = json!({
            "change_id": "current-change",
            "execution": mutation["action"]["execution"],
            "delegation": mutation["action"]["delegation"],
            "legacy_constraints": null,
            "reference_encoding": "feature_sets_v2"
        });
        for (change_id, policy) in [
            (json!(null), json!(null)),
            (json!("current-change"), policy),
            (json!("removed-change"), json!(null)),
        ] {
            let response = json!({
                "change_id": change_id,
                "policy": policy,
                "can_manage": false,
                "catalog": {"workspace": "w", "namespaces": []},
                "execution_resolution": {"workspace": "w", "allowed": {}, "diagnostics": []},
                "delegation_resolution": {"workspace": "w", "allowed": {}, "diagnostics": []}
            });
            let parsed: SubjectPermissionPolicyResponse =
                serde_json::from_value(response.clone()).unwrap();
            assert_eq!(serde_json::to_value(parsed).unwrap(), response);
            let result = json!({"change_id": change_id, "policy": policy});
            let parsed: SubjectPermissionPolicyMutationResponse =
                serde_json::from_value(result.clone()).unwrap();
            assert_eq!(serde_json::to_value(parsed).unwrap(), result);
        }
    }
}
