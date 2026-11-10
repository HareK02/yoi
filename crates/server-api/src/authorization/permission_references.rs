//! Stored reference interpretation is explicit and never changes current catalog lookup.
use feature_permissions::{PermissionAssignment, PermissionReference, PermissionSetId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum PermissionReferenceEncoding {
    #[default]
    LegacySubjectV1,
    FeatureSetsV2,
}

impl PermissionReferenceEncoding {
    pub fn is_legacy(&self) -> bool {
        matches!(self, Self::LegacySubjectV1)
    }
}

/// Converts only the retired Subject reference vocabulary, in both assignment vectors.
/// Unknown references remain available for diagnostics; revocation and lane ownership
/// are unchanged. Callers must gate this storage adapter on the saved encoding.
pub fn migrate_legacy_subject_assignment(assignment: &mut PermissionAssignment) -> bool {
    fn set(name: &str) -> PermissionReference {
        PermissionReference::Set(PermissionSetId::parse(name).expect("valid built-in set ID"))
    }

    fn references(references: &mut Vec<PermissionReference>) -> bool {
        let mut changed = false;
        let mut current = Vec::with_capacity(references.len());
        for reference in std::mem::take(references) {
            match reference {
                PermissionReference::Set(id) if id.as_str() == "subject-default" => {
                    current.push(set("conversation"));
                    current.push(set("work-coordination"));
                    changed = true;
                }
                PermissionReference::Namespace(mut namespace)
                    if namespace.namespace.as_str() == "subject"
                        && namespace.keys.iter().any(|key| {
                            matches!(key.as_str(), "subject.orchestrate" | "subject.delegate")
                        }) =>
                {
                    let mut replacements = Vec::new();
                    namespace.keys.retain(|key| match key.as_str() {
                        "subject.orchestrate" => {
                            replacements.push(set("work-coordination"));
                            false
                        }
                        "subject.delegate" => {
                            replacements.push(set("work-execution"));
                            false
                        }
                        _ => true,
                    });
                    if !namespace.keys.is_empty() {
                        current.push(PermissionReference::Namespace(namespace));
                    }
                    current.extend(replacements);
                    changed = true;
                }
                reference => current.push(reference),
            }
        }
        *references = current;
        changed
    }

    let grants_changed = references(&mut assignment.grants);
    let exclusions_changed = references(&mut assignment.exclusions);
    grants_changed || exclusions_changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authorization::{
        CurrentSubjectPolicy, PermissionResource, ScopedPermission, SubjectPermissions,
        SubjectPolicySource,
    };
    use feature_permissions::{NamespaceId, NamespacePermissions, PermissionKey};

    fn set(name: &str) -> PermissionReference {
        PermissionReference::Set(PermissionSetId::parse(name).unwrap())
    }

    fn namespace(name: &str, keys: &[&str]) -> PermissionReference {
        PermissionReference::Namespace(NamespacePermissions {
            namespace: NamespaceId::parse(name).unwrap(),
            keys: keys
                .iter()
                .map(|key| PermissionKey::parse(*key).unwrap())
                .collect(),
        })
    }

    #[test]
    fn missing_stored_encoding_round_trips_without_changing_policy_bytes() {
        let raw = concat!(
            r#"{"change_id":"saved","execution":{"grants":[{"kind":"set","value":"subject-default"}],"exclusions":[],"revoked":false},"#,
            r#""delegation":{"grants":[{"kind":"namespace","value":{"namespace":"subject","keys":["subject.delegate"]}}],"exclusions":[],"revoked":false},"legacy_constraints":null}"#,
        );
        let source: SubjectPolicySource = serde_json::from_str(raw).unwrap();
        let SubjectPolicySource::Feature(policy) = &source else {
            panic!("Feature source expected");
        };
        assert_eq!(
            policy.reference_encoding,
            PermissionReferenceEncoding::LegacySubjectV1
        );
        assert_eq!(policy.execution.grants, vec![set("subject-default")]);
        assert_eq!(serde_json::to_string(&source).unwrap(), raw);
        assert_eq!(serde_json::to_vec(&source).unwrap(), raw.as_bytes());
    }

    #[test]
    fn legacy_conversion_maps_grants_and_exclusions_without_changing_revocation() {
        let references = vec![
            set("subject-default"),
            namespace("subject", &["subject.orchestrate", "subject.delegate"]),
        ];
        let expected = vec![
            set("conversation"),
            set("work-coordination"),
            set("work-coordination"),
            set("work-execution"),
        ];
        for revoked in [false, true] {
            let mut assignment = PermissionAssignment {
                grants: references.clone(),
                exclusions: references.clone(),
                revoked,
            };
            assert!(migrate_legacy_subject_assignment(&mut assignment));
            assert_eq!(assignment.grants, expected);
            assert_eq!(assignment.exclusions, expected);
            assert_eq!(assignment.revoked, revoked);
            let converted = assignment.clone();
            assert!(!migrate_legacy_subject_assignment(&mut assignment));
            assert_eq!(assignment, converted);
        }
    }

    #[test]
    fn legacy_conversion_keeps_unknown_sets_namespaces_and_mixed_keys() {
        let unknown = vec![
            set("subject-default-extra"),
            set("subject"),
            namespace("other", &["subject.orchestrate", "other.unknown"]),
            namespace("subject", &["subject.orchestrate.extra"]),
            namespace("subject", &[]),
        ];
        let mut assignment = PermissionAssignment {
            grants: unknown.clone(),
            exclusions: vec![namespace(
                "subject",
                &[
                    "subject.unknown",
                    "subject.orchestrate",
                    "other.unknown",
                    "subject.delegate",
                ],
            )],
            revoked: true,
        };
        assert!(migrate_legacy_subject_assignment(&mut assignment));
        assert_eq!(assignment.grants, unknown);
        assert_eq!(
            assignment.exclusions,
            vec![
                namespace("subject", &["subject.unknown", "other.unknown"]),
                set("work-coordination"),
                set("work-execution"),
            ]
        );
        let converted = assignment.clone();
        assert!(!migrate_legacy_subject_assignment(&mut assignment));
        assert_eq!(assignment, converted);
    }

    #[test]
    fn explicit_policy_normalization_preserves_lanes_change_and_scoped_constraints() {
        let constraints = SubjectPermissions {
            change_id: "original-constraints".into(),
            execution: vec![ScopedPermission {
                operation: "subject.orchestrate".into(),
                resource: PermissionResource::Subject {
                    workspace_id: "workspace".into(),
                    subject_id: "only-subject".into(),
                },
            }],
            delegation: vec![],
        };
        let raw = CurrentSubjectPolicy {
            change_id: "unchanged".into(),
            execution: PermissionAssignment {
                grants: vec![set("subject-default")],
                exclusions: vec![namespace("subject", &["subject.delegate"])],
                revoked: true,
            },
            delegation: PermissionAssignment::default(),
            legacy_constraints: Some(constraints.clone()),
            reference_encoding: PermissionReferenceEncoding::LegacySubjectV1,
        };
        let current = raw.clone().into_current_reference_encoding();
        assert_eq!(
            current.reference_encoding,
            PermissionReferenceEncoding::FeatureSetsV2
        );
        assert_eq!(current.change_id, raw.change_id);
        assert_eq!(current.legacy_constraints, Some(constraints));
        assert_eq!(current.delegation, raw.delegation);
        assert!(current.execution.revoked);
        assert_eq!(
            current.execution.grants,
            vec![set("conversation"), set("work-coordination")]
        );
        assert_eq!(current.execution.exclusions, vec![set("work-execution")]);
        assert_eq!(current.clone().into_current_reference_encoding(), current);
        assert_eq!(
            serde_json::to_value(&current).unwrap()["reference_encoding"],
            "feature_sets_v2"
        );

        let explicitly_current = CurrentSubjectPolicy {
            reference_encoding: PermissionReferenceEncoding::FeatureSetsV2,
            ..raw
        };
        assert_eq!(
            explicitly_current.clone().into_current_reference_encoding(),
            explicitly_current
        );
    }

    #[test]
    fn scoped_legacy_projection_is_current_without_expanding_exact_references() {
        let source = SubjectPermissions {
            change_id: "scoped".into(),
            execution: vec![ScopedPermission {
                operation: "subject.orchestrate".into(),
                resource: PermissionResource::Subject {
                    workspace_id: "workspace".into(),
                    subject_id: "only-subject".into(),
                },
            }],
            delegation: vec![],
        };
        let current = CurrentSubjectPolicy::from_legacy(source.clone()).unwrap();
        assert_eq!(
            current.reference_encoding,
            PermissionReferenceEncoding::FeatureSetsV2
        );
        assert_eq!(
            current.execution.grants,
            vec![namespace("subject", &["subject.orchestrate"])]
        );
        assert_eq!(current.legacy_constraints, Some(source));
        assert_eq!(current.clone().into_current_reference_encoding(), current);
    }
}
