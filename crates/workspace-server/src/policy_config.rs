//! Subject assignments in the existing Workspace virtual configuration.
//!
//! An omitted Subject is unchanged; removal is explicit. Assignments contain only
//! references to Feature-owned Sets and Namespaces. Legacy resource restrictions
//! remain separate domain constraints, never payloads on permission keys.

use std::collections::{BTreeMap, BTreeSet};

use config_source::{ConfigSchemaContribution, EvaluationResult};
use feature_permissions::{
    NamespaceId, NamespacePermissions, PermissionAssignment, PermissionDiagnostic, PermissionHost,
    PermissionKey, PermissionReference, PermissionSetId,
};
use serde::{Deserialize, Serialize};
use server_api::authorization::{ScopedPermission, SubjectPermissions};

use crate::config_source::WorkspaceConfigSchemaProvider;
use crate::{Error, Result};

const SCHEMA: &str = r#"{
    permissions = {
        subjects = [...{
            subject_id = String;
            execution = {
                sets = [...String] default [];
                namespaces = [...{ namespace = String; keys = [...String]; }] default [];
                exclude_sets = [...String] default [];
                exclude_namespaces = [...{ namespace = String; keys = [...String]; }] default [];
                revoked = Bool default false;
            } default {};
            delegation = {
                sets = [...String] default [];
                namespaces = [...{ namespace = String; keys = [...String]; }] default [];
                exclude_sets = [...String] default [];
                exclude_namespaces = [...{ namespace = String; keys = [...String]; }] default [];
                revoked = Bool default false;
            } default {};
        }] default [];
        remove_subjects = [...String] default [];
    };
}"#;

#[derive(Debug, Default)]
pub struct PermissionConfigSchemaProvider;

impl WorkspaceConfigSchemaProvider for PermissionConfigSchemaProvider {
    fn contribution(&self) -> Result<ConfigSchemaContribution> {
        ConfigSchemaContribution::new("builtin:permissions", "permissions", "1", SCHEMA)
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
    pub fn assignment(&self) -> PermissionAssignment {
        let references = |sets: &[PermissionSetId], namespaces: &[NamespacePermissions]| {
            sets.iter().cloned().map(PermissionReference::Set)
                .chain(namespaces.iter().cloned().map(PermissionReference::Namespace)).collect()
        };
        PermissionAssignment {
            grants: references(&self.sets, &self.namespaces),
            exclusions: references(&self.exclude_sets, &self.exclude_namespaces),
            revoked: self.revoked,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubjectAssignmentSource {
    pub subject_id: String,
    #[serde(default)]
    pub execution: AssignmentSource,
    #[serde(default)]
    pub delegation: AssignmentSource,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PermissionSource {
    #[serde(default)]
    pub subjects: Vec<SubjectAssignmentSource>,
    #[serde(default)]
    pub remove_subjects: Vec<String>,
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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", deny_unknown_fields)]
pub enum MigrationDiagnostic {
    Converted { subject_id: String, operation: String },
    DomainConstraint { subject_id: String, permission: ScopedPermission },
    ExplicitConfigurationRequired { subject_id: String, permission: ScopedPermission },
}

pub fn source(evaluation: &EvaluationResult) -> Result<PermissionSource> {
    let value = evaluation.projections.first()
        .and_then(|p| p.data_json.get("permissions"))
        .cloned().unwrap_or_else(|| serde_json::json!({}));
    serde_json::from_value(value).map_err(|e| Error::InvalidInput(format!("invalid Subject assignments: {e}")))
}

fn validate_assignment(host: &PermissionHost, source: &AssignmentSource) -> Result<()> {
    // Validate even revoked assignments. A revocation must not hide a misspelled
    // reference which would become active on a later edit.
    let mut assignment = source.assignment();
    assignment.revoked = false;
    let resolution = host.resolve(Some(&assignment));
    if resolution.projection().diagnostics.iter().any(|d| matches!(d,
        PermissionDiagnostic::UnknownKey(_) | PermissionDiagnostic::UnknownNamespace(_)
        | PermissionDiagnostic::UnknownSet(_))) {
        return Err(Error::InvalidInput(format!("invalid permission references: {:?}", resolution.projection().diagnostics)));
    }
    Ok(())
}

pub fn validate_source(
    host: &PermissionHost,
    source: &PermissionSource,
    mut require_subject: impl FnMut(&str) -> Result<()>,
) -> Result<()> {
    let mut subjects = BTreeSet::new();
    for item in &source.subjects {
        server_api::authorization::validate_authorization_identifier(&item.subject_id)
            .map_err(Error::InvalidInput)?;
        if !subjects.insert(&item.subject_id) {
            return Err(Error::InvalidInput("duplicate Subject assignment".into()));
        }
        require_subject(&item.subject_id)?;
        validate_assignment(host, &item.execution)?;
        validate_assignment(host, &item.delegation)?;
    }
    for subject in &source.remove_subjects {
        server_api::authorization::validate_authorization_identifier(subject)
            .map_err(Error::InvalidInput)?;
        if !subjects.insert(subject) {
            return Err(Error::InvalidInput("Subject both assigned and removed, or removed twice".into()));
        }
        require_subject(subject)?;
    }
    Ok(())
}

/// A patch deliberately does not replace the entire policy map. In particular a
/// schema default empty list never deletes omitted policies or reinstates defaults.
pub fn apply(
    policies: &mut BTreeMap<String, AdoptedSubjectPolicy>,
    source: &PermissionSource,
    change_id: &str,
) {
    for subject in &source.remove_subjects { policies.remove(subject); }
    for item in &source.subjects {
        match policies.get_mut(&item.subject_id) {
            Some(previous) if previous.source == *item => {},
            Some(previous) => {
                previous.source = item.clone();
                previous.change_id = change_id.to_owned();
            },
            None => {
                policies.insert(item.subject_id.clone(), AdoptedSubjectPolicy {
                    source: item.clone(), change_id: change_id.to_owned(), legacy_constraints: None,
                });
            },
        }
    }
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
            let operation = server_api::authorization::canonical_worker_operation(&permission.operation).unwrap_or(&permission.operation);
            let key = PermissionKey::parse(operation.to_owned()).map_err(Error::InvalidInput)?;
            let assignment = PermissionAssignment { grants: vec![PermissionReference::Namespace(NamespacePermissions {
                namespace: key.namespace(), keys: vec![key.clone()],
            })], exclusions: vec![], revoked: false };
            let resolution = host.resolve(Some(&assignment));
            let exact = resolution.projection().diagnostics.is_empty()
                && resolution.projection().allowed.len() == 1
                && resolution.projection().allowed.contains_key(&key);
            if !exact {
                diagnostics.push(MigrationDiagnostic::ExplicitConfigurationRequired {
                    subject_id: subject_id.into(), permission: permission.clone(),
                });
                continue;
            }
            by_namespace.entry(key.namespace()).or_default().insert(key);
            diagnostics.push(MigrationDiagnostic::DomainConstraint {
                subject_id: subject_id.into(), permission: permission.clone(),
            });
        }
        Ok(AssignmentSource { namespaces: by_namespace.into_iter().map(|(namespace, keys)| NamespacePermissions {
            namespace, keys: keys.into_iter().collect(),
        }).collect(), ..Default::default() })
    };
    let execution = lane(&legacy.execution)?;
    let delegation = lane(&legacy.delegation)?;
    Ok((AdoptedSubjectPolicy {
        source: SubjectAssignmentSource { subject_id: subject_id.into(), execution, delegation },
        change_id: legacy.change_id.clone(), legacy_constraints: Some(legacy.clone()),
    }, diagnostics))
}

impl AdoptedSubjectPolicy {
    /// Read bridge for the old scoped-operation consumer. New payload-free
    /// consumers resolve `source` through the current Feature Host and also enforce
    /// retained domain constraints. This bridge cannot turn a key into an all-target
    /// legacy resource: an operation without a representable constraint is unavailable.
    pub fn legacy_read(&self, host: &PermissionHost) -> Result<SubjectPermissions> {
        let execution = host.resolve(Some(&self.source.execution.assignment()));
        let delegation = host.resolve(Some(&self.source.delegation.assignment()));
        let filter = |allowed: &feature_permissions::PermissionResolution, constraints: &[ScopedPermission]| -> Result<Vec<ScopedPermission>> {
            let mut represented = BTreeSet::new();
            let mut output = vec![];
            for permission in constraints {
                let operation = server_api::authorization::canonical_worker_operation(&permission.operation)
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
        let empty = SubjectPermissions { change_id:self.change_id.clone(),execution:vec![],delegation:vec![] };
        let retained = self.legacy_constraints.as_ref().unwrap_or(&empty);
        Ok(SubjectPermissions {
            change_id:self.change_id.clone(),
            execution:filter(execution.projection(),&retained.execution)?,
            delegation:filter(delegation.projection(),&retained.delegation)?,
        })
    }
}
