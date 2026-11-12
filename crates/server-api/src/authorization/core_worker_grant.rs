//! Host-issued ordinary Worker grants do not depend on a Subject foreign key.
//! Legacy Subject grants retain their original type and containment validation.

use super::PermissionReferenceEncoding;
use feature_permissions::{PermissionAssignment, PermissionKey};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CoreWorkerGrantParent {
    SubjectBody {
        subject_id: String,
        source_change_id: String,
        runtime_id: String,
        worker_id: String,
    },
    /// Preserves the legacy grant's Subject version and identity. The existing
    /// legacy ancestry/containment checks remain necessary at each effect.
    LegacySubjectGrant {
        subject_id: String,
        source_change_id: String,
        runtime_id: String,
        worker_id: String,
    },
    WorkerGrant {
        grant_id: String,
        runtime_id: String,
        worker_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CoreWorkerGrantSource {
    LaunchPolicy {
        policy_id: String,
        #[schemars(length(min = 1, max = 512))]
        #[serde(deserialize_with = "crate::launch_policy::deserialize_authorization_key")]
        policy_authorization_key: String,
        parent: CoreWorkerGrantParent,
    },
    Account {
        account_id: String,
    },
    Delegation {
        parent: CoreWorkerGrantParent,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct CoreWorkerExecutionGrant {
    pub grant_id: String,
    pub workspace_id: String,
    pub creation_operation_id: String,
    pub runtime_id: String,
    pub worker_id: String,
    pub profile: String,
    pub profile_digest: String,
    pub assignment: PermissionAssignment,
    pub reference_encoding: PermissionReferenceEncoding,
    /// Policy and assignment-delegated children always carry a finite ceiling,
    /// including explicit empty. Account root assignments may remain dynamic;
    /// None never applies to a Worker-originated child.
    pub issuance_ceiling: Option<Vec<PermissionKey>>,
    pub source: CoreWorkerGrantSource,
}

impl CoreWorkerExecutionGrant {
    /// Current authorization projection only; immutable binding/replay uses the raw grant.
    pub fn effective_assignment(&self) -> PermissionAssignment {
        self.assignment.clone()
    }

    pub fn validate(&self) -> Result<(), String> {
        for value in [
            &self.grant_id,
            &self.workspace_id,
            &self.creation_operation_id,
            &self.runtime_id,
            &self.worker_id,
            &self.profile_digest,
        ] {
            if value.trim().is_empty() || value.len() > 1024 || value.chars().any(char::is_control)
            {
                return Err("invalid core Worker grant identity/digest".into());
            }
        }
        crate::launch_policy::validate_canonical_profile(&self.profile)?;
        if let CoreWorkerGrantSource::LaunchPolicy {
            policy_id,
            policy_authorization_key,
            ..
        } = &self.source
        {
            crate::launch_policy::validate_policy_id(policy_id)?;
            crate::launch_policy::validate_authorization_key(policy_authorization_key)?;
        }
        match &self.source {
            CoreWorkerGrantSource::LaunchPolicy { parent, .. }
            | CoreWorkerGrantSource::Delegation { parent } => {
                if self.issuance_ceiling.is_none() {
                    return Err("Worker-origin grant requires a finite issuance ceiling".into());
                }
                let (runtime, worker, source_id) = match parent {
                    CoreWorkerGrantParent::SubjectBody {
                        subject_id,
                        source_change_id,
                        runtime_id,
                        worker_id,
                    }
                    | CoreWorkerGrantParent::LegacySubjectGrant {
                        subject_id,
                        source_change_id,
                        runtime_id,
                        worker_id,
                    } => {
                        if source_change_id.trim().is_empty() {
                            return Err("missing Subject policy change identity".into());
                        }
                        (runtime_id, worker_id, subject_id)
                    }
                    CoreWorkerGrantParent::WorkerGrant {
                        grant_id,
                        runtime_id,
                        worker_id,
                    } => (runtime_id, worker_id, grant_id),
                };
                if [runtime, worker, source_id]
                    .iter()
                    .any(|v| v.trim().is_empty())
                    || (runtime == &self.runtime_id && worker == &self.worker_id)
                {
                    return Err("invalid or self-referencing parent provenance".into());
                }
            }
            CoreWorkerGrantSource::Account { account_id } => {
                if account_id.trim().is_empty() {
                    return Err("missing authenticated account issuer".into());
                }
            }
        }
        if let Some(ceiling) = &self.issuance_ceiling {
            let unique: std::collections::BTreeSet<_> = ceiling.iter().collect();
            if unique.len() != ceiling.len() {
                return Err("duplicate issuance ceiling key".into());
            }
        }
        Ok(())
    }

    /// Intersects a *current* resolver result with the original ceiling. This
    /// does not validate policy state, ancestry, identity or resource access.
    pub fn bounds_operation(&self, key: &PermissionKey) -> bool {
        self.issuance_ceiling
            .as_ref()
            .is_none_or(|ceiling| ceiling.contains(key))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn delegation_wire_requires_finite_ceiling_and_retains_typed_parent() {
        let raw = serde_json::json!({
            "grant_id":"g", "workspace_id":"w", "creation_operation_id":"op",
            "runtime_id":"r", "worker_id":"child", "profile":"builtin:coder",
            "profile_digest":"digest", "reference_encoding":"feature_sets_v2",
            "assignment":{"grants":[], "exclusions":[], "revoked":false},
            "issuance_ceiling":null,
            "source":{"kind":"delegation", "parent":{
                "kind":"worker_grant", "grant_id":"parent-grant",
                "runtime_id":"r", "worker_id":"parent"
            }}
        });
        let mut grant: CoreWorkerExecutionGrant = serde_json::from_value(raw).unwrap();
        assert!(grant.validate().is_err());
        grant.issuance_ceiling = Some(vec![]);
        grant.validate().unwrap();
        assert!(!grant.bounds_operation(&PermissionKey::parse("workdir.list").unwrap()));
        assert_eq!(
            serde_json::to_value(&grant).unwrap()["source"],
            serde_json::json!({
                "kind":"delegation", "parent":{
                    "kind":"worker_grant", "grant_id":"parent-grant",
                    "runtime_id":"r", "worker_id":"parent"
                }
            })
        );
    }

    #[test]
    fn launch_policy_source_requires_valid_string_identity_without_legacy_fallback() {
        let source = serde_json::json!({
            "kind":"launch_policy", "policy_id":"coder", "policy_authorization_key":"auth-key",
            "parent":{"kind":"worker_grant", "grant_id":"parent-grant", "runtime_id":"r", "worker_id":"parent"}
        });
        for key in ["auth-key".to_owned(), "x".repeat(512), "é".repeat(256)] {
            let mut raw = source.clone();
            raw["policy_authorization_key"] = serde_json::json!(key);
            let parsed: CoreWorkerGrantSource = serde_json::from_value(raw.clone()).unwrap();
            assert_eq!(serde_json::to_value(parsed).unwrap(), raw);
        }
        for key in [
            serde_json::json!(1),
            serde_json::json!(null),
            serde_json::json!(""),
            serde_json::json!("x".repeat(513)),
            serde_json::json!("é".repeat(257)),
            serde_json::json!("bad\nkey"),
            serde_json::json!("bad\u{85}key"),
        ] {
            let mut raw = source.clone();
            raw["policy_authorization_key"] = key;
            assert!(serde_json::from_value::<CoreWorkerGrantSource>(raw).is_err());
        }
        for include_current in [false, true] {
            let mut raw = source.clone();
            if !include_current {
                raw.as_object_mut()
                    .unwrap()
                    .remove("policy_authorization_key");
            }
            raw["policy_revision"] = serde_json::json!(1);
            assert!(serde_json::from_value::<CoreWorkerGrantSource>(raw).is_err());
        }
        let mut missing = source;
        missing
            .as_object_mut()
            .unwrap()
            .remove("policy_authorization_key");
        assert!(serde_json::from_value::<CoreWorkerGrantSource>(missing).is_err());
        let schema = serde_json::to_value(schemars::schema_for!(CoreWorkerGrantSource)).unwrap();
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
    fn stored_policy_grant_requires_finite_ceiling_on_decode_validation() {
        let raw = serde_json::json!({"grant_id":"g", "workspace_id":"w", "creation_operation_id":"op", "runtime_id":"r", "worker_id":"child", "profile":"builtin:coder", "profile_digest":"digest", "reference_encoding":"feature_sets_v2", "assignment":{"grants":[], "exclusions":[], "revoked":false}, "issuance_ceiling":null, "source":{"kind":"launch_policy", "policy_id":"coder", "policy_authorization_key":"auth-key","parent":{"kind":"subject_body", "subject_id":"s", "source_change_id":"v1", "runtime_id":"r", "worker_id":"body"}}});
        let mut grant: CoreWorkerExecutionGrant = serde_json::from_value(raw).unwrap();
        assert!(grant.validate().is_err());
        grant.issuance_ceiling = Some(vec![]);
        grant.validate().unwrap();
        assert!(!grant.bounds_operation(&PermissionKey::parse("worker.create").unwrap()));
        for key in ["".to_owned(), "é".repeat(257), "bad\u{85}key".to_owned()] {
            let CoreWorkerGrantSource::LaunchPolicy {
                policy_authorization_key,
                ..
            } = &mut grant.source
            else {
                unreachable!();
            };
            *policy_authorization_key = key;
            assert!(grant.validate().is_err());
        }
    }
}
