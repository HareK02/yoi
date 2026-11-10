//! Current Feature-key assignments and separately retained legacy domain limits.
//!
//! This is a policy value, not an authorization receipt. Hosts resolve its lanes
//! against their current registered Feature declarations at every protected access.
use super::{
    PermissionReferenceEncoding, ScopedPermission, SubjectPermissions,
    migrate_legacy_subject_assignment,
};
use feature_permissions::{
    NamespacePermissions, PermissionAssignment, PermissionKey, PermissionReference,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Stored source representation is explicit; scalar legacy operations are not
/// inferred from an error emitted by a Feature-key consumer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SubjectPolicySource {
    Feature(CurrentSubjectPolicy),
    Legacy(SubjectPermissions),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct CurrentSubjectPolicy {
    pub change_id: String,
    pub execution: PermissionAssignment,
    pub delegation: PermissionAssignment,
    pub legacy_constraints: Option<SubjectPermissions>,
    #[serde(
        default,
        skip_serializing_if = "PermissionReferenceEncoding::is_legacy"
    )]
    pub reference_encoding: PermissionReferenceEncoding,
}

impl CurrentSubjectPolicy {
    /// Interpret stored references only at an effective-policy boundary. Raw history,
    /// exports and receipts must retain their original encoding and serialized bytes.
    pub fn into_current_reference_encoding(mut self) -> Self {
        if self.reference_encoding.is_legacy() {
            migrate_legacy_subject_assignment(&mut self.execution);
            migrate_legacy_subject_assignment(&mut self.delegation);
            self.reference_encoding = PermissionReferenceEncoding::FeatureSetsV2;
        }
        self
    }

    /// Representation-only projection. Exact old resource conditions remain a
    /// separate conjunct; unrepresentable names are diagnosed, never discarded.
    pub fn from_legacy(source: SubjectPermissions) -> Result<Self, String> {
        fn lane(permissions: &[ScopedPermission]) -> Result<PermissionAssignment, String> {
            let mut namespaces = BTreeMap::<_, Vec<_>>::new();
            for permission in permissions {
                let operation = super::canonical_worker_operation(&permission.operation)
                    .unwrap_or(&permission.operation);
                let key = PermissionKey::parse(operation.to_owned())?;
                namespaces.entry(key.namespace()).or_default().push(key);
            }
            Ok(PermissionAssignment {
                grants: namespaces
                    .into_iter()
                    .map(|(namespace, keys)| {
                        PermissionReference::Namespace(NamespacePermissions { namespace, keys })
                    })
                    .collect(),
                exclusions: vec![],
                revoked: false,
            })
        }
        Ok(Self {
            change_id: source.change_id.clone(),
            execution: lane(&source.execution)?,
            delegation: lane(&source.delegation)?,
            legacy_constraints: Some(source),
            reference_encoding: PermissionReferenceEncoding::FeatureSetsV2,
        })
    }
}
