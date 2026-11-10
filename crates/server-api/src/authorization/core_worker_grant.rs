//! Host-issued ordinary Worker grants do not depend on a Subject foreign key.
//! Legacy Subject grants retain their original type and containment validation.

use super::{PermissionReferenceEncoding, migrate_legacy_subject_assignment};
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
        #[cfg_attr(feature = "typescript", ts(type = "number"))]
        #[schemars(range(min = 1, max = 9_007_199_254_740_991_u64))]
        #[serde(deserialize_with = "crate::launch_policy::deserialize_authorization_revision")]
        policy_revision: u64,
        parent: CoreWorkerGrantParent,
    },
    Account {
        account_id: String,
    },
    Delegation {
        parent: CoreWorkerGrantParent,
    },
    /// Historical issued grants only. Retain this wire spelling and raw identity
    /// for replay/restore; new creation requests and issuance never select it.
    Human {
        account_id: String,
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
    #[serde(
        default,
        skip_serializing_if = "PermissionReferenceEncoding::is_legacy"
    )]
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
        let mut assignment = self.assignment.clone();
        if self.reference_encoding.is_legacy() {
            migrate_legacy_subject_assignment(&mut assignment);
        }
        assignment
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
            policy_revision,
            ..
        } = &self.source
        {
            crate::launch_policy::validate_policy_id(policy_id)?;
            crate::launch_policy::validate_authorization_revision(*policy_revision)?;
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
                            return Err("missing Subject policy revision".into());
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
            CoreWorkerGrantSource::Account { account_id }
            | CoreWorkerGrantSource::Human { account_id } => {
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
    fn legacy_grant_projection_maps_both_vectors_without_changing_bytes_or_ceiling() {
        let raw = concat!(
            r#"{"grant_id":"g","workspace_id":"w","creation_operation_id":"op","runtime_id":"r","worker_id":"child","profile":"builtin:coder","profile_digest":"digest","assignment":{"grants":["#,
            r#"{"kind":"set","value":"subject-default"},{"kind":"namespace","value":{"namespace":"subject","keys":["subject.orchestrate","subject.delegate"]}}],"exclusions":["#,
            r#"{"kind":"set","value":"subject-default"},{"kind":"namespace","value":{"namespace":"subject","keys":["subject.orchestrate","subject.delegate"]}}],"revoked":true},"#,
            r#""issuance_ceiling":["worker.create"],"source":{"kind":"human","account_id":"owner"}}"#,
        );
        let mut grant: CoreWorkerExecutionGrant = serde_json::from_str(raw).unwrap();
        assert_eq!(
            grant.reference_encoding,
            PermissionReferenceEncoding::LegacySubjectV1
        );
        let effective = grant.effective_assignment();
        let expected = serde_json::json!([
            {"kind":"set", "value":"conversation"},
            {"kind":"set", "value":"work-coordination"},
            {"kind":"set", "value":"work-coordination"},
            {"kind":"set", "value":"work-execution"}
        ]);
        assert_eq!(serde_json::to_value(&effective.grants).unwrap(), expected);
        assert_eq!(effective.grants, effective.exclusions);
        assert!(effective.revoked);
        assert_eq!(serde_json::to_string(&grant).unwrap(), raw);
        let stop = PermissionKey::parse("worker.stop").unwrap();
        assert!(!grant.bounds_operation(&stop));
        grant.issuance_ceiling = Some(vec![]);
        grant.effective_assignment();
        assert_eq!(grant.issuance_ceiling, Some(vec![]));
        assert!(!grant.bounds_operation(&stop));
        grant.issuance_ceiling = None;
        grant.effective_assignment();
        assert_eq!(grant.issuance_ceiling, None);
        grant.reference_encoding = PermissionReferenceEncoding::FeatureSetsV2;
        assert_eq!(grant.effective_assignment(), grant.assignment);
        assert_eq!(
            serde_json::to_value(&grant).unwrap()["reference_encoding"],
            "feature_sets_v2"
        );
    }

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
    fn stored_policy_grant_requires_finite_ceiling_on_decode_validation() {
        let raw = serde_json::json!({"grant_id":"g", "workspace_id":"w", "creation_operation_id":"op", "runtime_id":"r", "worker_id":"child", "profile":"builtin:coder", "profile_digest":"digest", "assignment":{"grants":[], "exclusions":[], "revoked":false}, "issuance_ceiling":null, "source":{"kind":"launch_policy", "policy_id":"coder", "policy_revision":1,"parent":{"kind":"subject_body", "subject_id":"s", "source_change_id":"v1", "runtime_id":"r", "worker_id":"body"}}});
        let mut grant: CoreWorkerExecutionGrant = serde_json::from_value(raw).unwrap();
        assert!(grant.validate().is_err());
        grant.issuance_ceiling = Some(vec![]);
        grant.validate().unwrap();
        assert!(!grant.bounds_operation(&PermissionKey::parse("worker.create").unwrap()));
    }
}
