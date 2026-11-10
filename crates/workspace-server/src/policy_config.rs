//! Shared Workspace permission configuration, separate from Subject policy storage.
//!
//! Workspace sets use the Feature graph resolver and cannot replace Feature-owned
//! sets or declare keys. `worker_default` selects shared references when a new
//! Worker creation caller omits its selection. Callers still enforce their own
//! launch authority; retries and restores retain issued grants. Omission selects
//! `implementer`, while an explicit empty assignment stays empty. Unknown
//! references are rejected even when revoked.
//!
//! Individual Subject records below retain the existing persisted JSON contract.
//! They are not current configuration input, and legacy decoding never authorizes
//! replaying assignments or removals over the current domain policy.

use std::collections::{BTreeMap, BTreeSet};

use config_source::{ConfigSchemaContribution, EvaluationResult};
use feature_permissions::{
    NamespaceId, NamespacePermissions, PermissionAssignment, PermissionDiagnostic, PermissionHost,
    PermissionKey, PermissionReference, PermissionSetId,
};
use serde::{Deserialize, Serialize};
use server_api::authorization::{
    PermissionReferenceEncoding, ScopedPermission, SubjectPermissions,
};

use crate::config_source::WorkspaceConfigSchemaProvider;
use crate::{Error, Result};

// Decodal 0.4 materializes object members before object-level defaults. Keep
// this optional selection abstract until supplied, then decode its closed shape
// with AssignmentSource. This distinguishes omission from an authored empty object.
const SCHEMA: &str = r#"{
    permissions = {
        sets = [...{
            id = String;
            sets = [...String] default [];
            namespaces = [...{ namespace = String; keys = [...String]; }] default [];
        }] default [];
        worker_default = Unknown default { sets = ["implementer"]; };
    };
}"#;

#[derive(Debug, Default)]
pub struct PermissionConfigSchemaProvider;

impl WorkspaceConfigSchemaProvider for PermissionConfigSchemaProvider {
    fn contribution(&self) -> Result<ConfigSchemaContribution> {
        ConfigSchemaContribution::new("builtin:permissions", "permissions", "5", SCHEMA)
            .map_err(|e| Error::Config(e.to_string()))
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssignmentSource {
    #[serde(default)]
    pub sets: Vec<PermissionSetId>,
    #[serde(default)]
    pub namespaces: Vec<NamespacePermissions>,
    #[serde(default)]
    pub exclude_sets: Vec<PermissionSetId>,
    #[serde(default)]
    pub exclude_namespaces: Vec<NamespacePermissions>,
    #[serde(default)]
    pub revoked: bool,
}

impl AssignmentSource {
    pub fn from_assignment(assignment: &PermissionAssignment) -> Self {
        let mut result = Self {
            revoked: assignment.revoked,
            ..Default::default()
        };
        for reference in &assignment.grants {
            match reference {
                PermissionReference::Set(set) => result.sets.push(set.clone()),
                PermissionReference::Namespace(namespace) => {
                    result.namespaces.push(namespace.clone())
                }
            }
        }
        for reference in &assignment.exclusions {
            match reference {
                PermissionReference::Set(set) => result.exclude_sets.push(set.clone()),
                PermissionReference::Namespace(namespace) => {
                    result.exclude_namespaces.push(namespace.clone())
                }
            }
        }
        result
    }

    pub fn assignment(&self) -> PermissionAssignment {
        let references = |sets: &[PermissionSetId], namespaces: &[NamespacePermissions]| {
            sets.iter()
                .cloned()
                .map(PermissionReference::Set)
                .chain(
                    namespaces
                        .iter()
                        .cloned()
                        .map(PermissionReference::Namespace),
                )
                .collect()
        };
        PermissionAssignment {
            grants: references(&self.sets, &self.namespaces),
            exclusions: references(&self.exclude_sets, &self.exclude_namespaces),
            revoked: self.revoked,
        }
    }
}

/// Storage-compatible Subject assignment record, not current config input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubjectAssignmentSource {
    pub subject_id: String,
    #[serde(default)]
    pub execution: AssignmentSource,
    #[serde(default)]
    pub delegation: AssignmentSource,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspacePermissionSetSource {
    pub id: PermissionSetId,
    #[serde(default)]
    pub sets: Vec<PermissionSetId>,
    #[serde(default)]
    pub namespaces: Vec<NamespacePermissions>,
}

impl WorkspacePermissionSetSource {
    fn contribution(&self) -> feature_permissions::SetContribution {
        feature_permissions::SetContribution {
            set: self.id.clone(),
            children: AssignmentSource {
                sets: self.sets.clone(),
                namespaces: self.namespaces.clone(),
                ..Default::default()
            }
            .assignment()
            .grants,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PermissionSource {
    #[serde(default)]
    pub sets: Vec<WorkspacePermissionSetSource>,
    #[serde(default = "default_worker_assignment")]
    pub worker_default: AssignmentSource,
}

fn default_worker_assignment() -> AssignmentSource {
    AssignmentSource {
        sets: vec![PermissionSetId::parse("implementer").expect("built-in set ID")],
        ..Default::default()
    }
}

impl Default for PermissionSource {
    fn default() -> Self {
        Self {
            sets: vec![],
            worker_default: default_worker_assignment(),
        }
    }
}

/// Decoder for persisted configuration evaluated with permission schema versions
/// through four. Never use this historical shape for current input.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LegacyPermissionSource {
    #[serde(default)]
    pub subjects: Vec<SubjectAssignmentSource>,
    #[serde(default)]
    pub remove_subjects: Vec<String>,
    #[serde(default)]
    pub sets: Vec<WorkspacePermissionSetSource>,
    #[serde(default)]
    pub human_worker_default: Option<AssignmentSource>,
}

impl LegacyPermissionSource {
    pub fn shared_source(&self) -> PermissionSource {
        PermissionSource {
            sets: self.sets.clone(),
            worker_default: self
                .human_worker_default
                .clone()
                .unwrap_or_else(default_worker_assignment),
        }
    }
}

/// Retained exact legacy restrictions belong to the domain, not the editable source.
/// Keeping the original change identity preserves the distinction between live and
/// already stale child grants during representation-only migration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdoptedSubjectPolicy {
    pub source: SubjectAssignmentSource,
    pub change_id: String,
    pub legacy_constraints: Option<SubjectPermissions>,
    // Missing metadata denotes the old encoding. Omitting it again on serialization
    // keeps preexisting frozen-plan digests stable until an explicit current edit.
    #[serde(
        default,
        skip_serializing_if = "PermissionReferenceEncoding::is_legacy"
    )]
    pub reference_encoding: PermissionReferenceEncoding,
}

/// Normalize only the retired Subject references, in every editable source slot.
/// Catalog validation deliberately follows this typed representation conversion.
pub(crate) fn migrate_source_references(source: &mut PermissionSource) -> bool {
    fn lane(source: &mut AssignmentSource) -> bool {
        let mut assignment = source.assignment();
        if server_api::authorization::migrate_legacy_subject_assignment(&mut assignment) {
            *source = AssignmentSource::from_assignment(&assignment);
            true
        } else {
            false
        }
    }
    let mut changed = lane(&mut source.worker_default);
    for set in &mut source.sets {
        let mut assignment = AssignmentSource {
            sets: set.sets.clone(),
            namespaces: set.namespaces.clone(),
            ..Default::default()
        };
        if lane(&mut assignment) {
            set.sets = assignment.sets;
            set.namespaces = assignment.namespaces;
            changed = true;
        }
    }
    changed
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", deny_unknown_fields)]
pub enum MigrationDiagnostic {
    Converted {
        subject_id: String,
        operation: String,
    },
    DomainConstraint {
        subject_id: String,
        permission: ScopedPermission,
    },
    ExplicitConfigurationRequired {
        subject_id: String,
        permission: ScopedPermission,
    },
}

pub fn legacy_source(evaluation: &EvaluationResult) -> Result<LegacyPermissionSource> {
    serde_json::from_value(permission_value(evaluation))
        .map_err(|e| Error::InvalidInput(format!("invalid legacy permission configuration: {e}")))
}

fn permission_value(evaluation: &EvaluationResult) -> serde_json::Value {
    evaluation
        .projections
        .first()
        .and_then(|p| p.data_json.get("permissions"))
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}))
}

pub fn source(evaluation: &EvaluationResult) -> Result<PermissionSource> {
    serde_json::from_value(permission_value(evaluation))
        .map_err(|e| Error::InvalidInput(format!("invalid permission configuration: {e}")))
}

fn reject_retired_references(assignment: &PermissionAssignment) -> Result<()> {
    let mut candidate = assignment.clone();
    if server_api::authorization::migrate_legacy_subject_assignment(&mut candidate) {
        return Err(Error::InvalidInput(
            "invalid permission references: retired Subject references are storage-only".into(),
        ));
    }
    Ok(())
}

pub fn validate_assignment(host: &PermissionHost, source: &AssignmentSource) -> Result<()> {
    // Validate even revoked assignments. A revocation must not hide a misspelled
    // reference which would become active on a later edit.
    let mut assignment = source.assignment();
    reject_retired_references(&assignment)?;
    assignment.revoked = false;
    let resolution = host.resolve(Some(&assignment));
    if resolution.projection().diagnostics.iter().any(|d| {
        matches!(
            d,
            PermissionDiagnostic::UnknownKey(_)
                | PermissionDiagnostic::UnknownNamespace(_)
                | PermissionDiagnostic::UnknownSet(_)
        )
    }) {
        return Err(Error::InvalidInput(format!(
            "invalid permission references: {:?}",
            resolution.projection().diagnostics
        )));
    }
    Ok(())
}

/// Construct a candidate catalog without mutating the current Host. Installation
/// is all-or-nothing, and both adoption and runtime reads use this same path.
/// No expanded permissions are persisted as a second source of truth.
pub fn configured_host(host: &PermissionHost, source: &PermissionSource) -> Result<PermissionHost> {
    for set in &source.sets {
        // Reserving the old ID prevents a new narrow custom set from being
        // interpreted as the historical broader reference on a later startup.
        if set.id.as_str() == "subject-default" {
            return Err(Error::InvalidInput(
                "invalid Workspace permission sets: retired Subject set ID is reserved".into(),
            ));
        }
        reject_retired_references(&PermissionAssignment {
            grants: set.contribution().children,
            ..Default::default()
        })?;
    }
    let mut configured = host.clone();
    configured
        .replace_workspace_sets(
            source
                .sets
                .iter()
                .map(WorkspacePermissionSetSource::contribution)
                .collect(),
        )
        .map_err(|error| {
            Error::InvalidInput(format!("invalid Workspace permission sets: {error}"))
        })?;
    validate_assignment(&configured, &source.worker_default)?;
    Ok(configured)
}

pub fn validate_source(
    host: &PermissionHost,
    source: &PermissionSource,
    _require_subject: impl FnMut(&str) -> Result<()>,
) -> Result<()> {
    configured_host(host, source)?;
    Ok(())
}

/// Convert exact legacy operations without expanding explicit children. Restrictions
/// which cannot be represented by the owning domain must be diagnosed before seal.
/// This routine does not issue a grant or install a policy.
pub fn migrate_subject(
    host: &PermissionHost,
    subject_id: &str,
    legacy: &SubjectPermissions,
) -> Result<(AdoptedSubjectPolicy, Vec<MigrationDiagnostic>)> {
    let mut diagnostics = vec![];
    let mut lane = |permissions: &[ScopedPermission]| -> Result<AssignmentSource> {
        let mut by_namespace: BTreeMap<NamespaceId, BTreeSet<PermissionKey>> = BTreeMap::new();
        for permission in permissions {
            let operation =
                server_api::authorization::canonical_worker_operation(&permission.operation)
                    .unwrap_or(&permission.operation);
            let Ok(key) = PermissionKey::parse(operation.to_owned()) else {
                diagnostics.push(MigrationDiagnostic::ExplicitConfigurationRequired {
                    subject_id: subject_id.into(),
                    permission: permission.clone(),
                });
                continue;
            };
            let assignment = PermissionAssignment {
                grants: vec![PermissionReference::Namespace(NamespacePermissions {
                    namespace: key.namespace(),
                    keys: vec![key.clone()],
                })],
                exclusions: vec![],
                revoked: false,
            };
            let resolution = host.resolve(Some(&assignment));
            let exact = resolution.projection().diagnostics.is_empty()
                && resolution.projection().allowed.len() == 1
                && resolution.projection().allowed.contains_key(&key);
            if !exact {
                diagnostics.push(MigrationDiagnostic::ExplicitConfigurationRequired {
                    subject_id: subject_id.into(),
                    permission: permission.clone(),
                });
                continue;
            }
            by_namespace.entry(key.namespace()).or_default().insert(key);
            diagnostics.push(MigrationDiagnostic::DomainConstraint {
                subject_id: subject_id.into(),
                permission: permission.clone(),
            });
        }
        Ok(AssignmentSource {
            namespaces: by_namespace
                .into_iter()
                .map(|(namespace, keys)| NamespacePermissions {
                    namespace,
                    keys: keys.into_iter().collect(),
                })
                .collect(),
            ..Default::default()
        })
    };
    let execution = lane(&legacy.execution)?;
    let delegation = lane(&legacy.delegation)?;
    Ok((
        AdoptedSubjectPolicy {
            source: SubjectAssignmentSource {
                subject_id: subject_id.into(),
                execution,
                delegation,
            },
            change_id: legacy.change_id.clone(),
            legacy_constraints: Some(legacy.clone()),
            reference_encoding: PermissionReferenceEncoding::FeatureSetsV2,
        },
        diagnostics,
    ))
}

impl AdoptedSubjectPolicy {
    pub fn current(&self) -> server_api::authorization::CurrentSubjectPolicy {
        server_api::authorization::CurrentSubjectPolicy {
            change_id: self.change_id.clone(),
            execution: self.source.execution.assignment(),
            delegation: self.source.delegation.assignment(),
            legacy_constraints: self.legacy_constraints.clone(),
            reference_encoding: self.reference_encoding,
        }
        .into_current_reference_encoding()
    }

    /// Persist the current representation without changing authorization identity
    /// or any retained domain restriction. Historical/frozen values are not callers.
    pub(crate) fn migrate_reference_encoding(&mut self) -> bool {
        let current = Self::from_current(&self.source.subject_id, &self.current());
        if *self == current {
            false
        } else {
            *self = current;
            true
        }
    }

    pub fn from_current(
        subject_id: &str,
        current: &server_api::authorization::CurrentSubjectPolicy,
    ) -> Self {
        let current = current.clone().into_current_reference_encoding();
        Self {
            source: SubjectAssignmentSource {
                subject_id: subject_id.into(),
                execution: AssignmentSource::from_assignment(&current.execution),
                delegation: AssignmentSource::from_assignment(&current.delegation),
            },
            change_id: current.change_id.clone(),
            legacy_constraints: current.legacy_constraints.clone(),
            reference_encoding: PermissionReferenceEncoding::FeatureSetsV2,
        }
    }

    /// Read bridge for the old scoped-operation consumer. New payload-free
    /// consumers resolve `source` through the current Feature Host and also enforce
    /// retained domain constraints. This bridge cannot turn a key into an all-target
    /// legacy resource: an operation without a representable constraint is unavailable.
    pub fn legacy_read(&self, host: &PermissionHost) -> Result<SubjectPermissions> {
        let current = self.current();
        let execution = host.resolve(Some(&current.execution));
        let delegation = host.resolve(Some(&current.delegation));
        let filter = |allowed: &feature_permissions::PermissionResolution,
                      constraints: &[ScopedPermission]|
         -> Result<Vec<ScopedPermission>> {
            let mut represented = BTreeSet::new();
            let mut output = vec![];
            for permission in constraints {
                let operation =
                    server_api::authorization::canonical_worker_operation(&permission.operation)
                        .unwrap_or(&permission.operation);
                if let Ok(key) = PermissionKey::parse(operation.to_owned()) {
                    if allowed.allowed.contains_key(&key) {
                        represented.insert(key);
                        output.push(permission.clone());
                    }
                }
            }
            if allowed.allowed.keys().any(|key| !represented.contains(key)) {
                return Err(Error::InvalidInput("current key policy requires the payload-free operation consumer; no legacy all-target fallback is available".into()));
            }
            Ok(output)
        };
        let empty = SubjectPermissions {
            change_id: self.change_id.clone(),
            execution: vec![],
            delegation: vec![],
        };
        let retained = self.legacy_constraints.as_ref().unwrap_or(&empty);
        Ok(SubjectPermissions {
            change_id: self.change_id.clone(),
            execution: filter(execution.projection(), &retained.execution)?,
            delegation: filter(delegation.projection(), &retained.delegation)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_permission_decoder_distinguishes_missing_and_explicit_empty_default() {
        let missing: PermissionSource = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(
            missing.worker_default.sets,
            vec![PermissionSetId::parse("implementer").unwrap()]
        );
        let empty: PermissionSource =
            serde_json::from_value(serde_json::json!({"worker_default": {}})).unwrap();
        assert_eq!(empty.worker_default, AssignmentSource::default());
        for value in [
            serde_json::json!({"human_worker_default": {}}),
            serde_json::json!({"worker_default": {}, "human_worker_default": {}}),
        ] {
            assert!(
                serde_json::from_value::<PermissionSource>(value).is_err(),
                "the retired field is storage-only, not a current alias"
            );
        }
    }
}
