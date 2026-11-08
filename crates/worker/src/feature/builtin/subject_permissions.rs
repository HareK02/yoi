//! Subject-owned semantic permissions. Explicit children reference operation
//! owners; they neither transfer namespace ownership nor bypass domain checks.
//! Only orchestration contributes to the retained subject-default shared set.
//! Delegation is a separate Namespace assignment, never a new top-level set.

use crate::feature::permissions::FeaturePermissionRegistrar;
use crate::feature::{
    FeatureDescriptor, FeatureInstallContext, FeatureInstallError, FeatureModule,
};
use feature_permissions::{
    NamespaceDeclaration, NamespaceId, NamespacePermissions, PermissionDefinition, PermissionKey,
    PermissionReference, PermissionSetId, SetContribution,
};
use server_api::authorization::{SUBJECT_DEFAULT_PERMISSION_SET, SUBJECT_DELEGATE_PERMISSION};

const ORCHESTRATE: &[&str] = &[
    "notification.diagnostics.read",
    "objective.query",
    "objective.create",
    "objective.read",
    "objective.edit",
    "objective.set_state",
    "objective.link_ticket",
    "objective.unlink_ticket",
    "worker.list",
    "worker.discover",
    "worker.observe.list",
    "worker.observe",
    "worker.create",
    "worker.messaging",
    "worker.send_input",
    "worker.cancel",
    "worker.stop",
    "worker.restore",
    "worker.remove",
    "workdir.read",
    "workdir.list",
    "workdir.attachment.list",
    "workdir.attachment.read",
    "workdir.create",
    "workdir.delete",
    "repository.list",
    "repository.read",
    "merge_request.read",
    "merge_request.readiness",
    "merge_request.complete",
    "ticket.create",
    "ticket.query",
    "ticket.list",
    "ticket.read",
    "ticket.edit_item",
    "ticket.edit_post",
    "ticket.event.add",
    "ticket.case.update",
    "ticket.dependency.check",
    "ticket.tag.list",
    "ticket.tag.create",
    "ticket.tag.rename",
    "ticket.tag.delete",
    "ticket.tags.set",
    "ticket.relation.query",
    "ticket.relation.read",
    "ticket.relation.record",
    "ticket.relation.remove",
    "ticket.assignment.assign",
    "ticket.assignment.clear",
    "ticket.assignment.update",
    "ticket.assignment.answer",
    "ticket.assignment.query",
    "ticket.assignment.history",
    "ticket.assignment.diagnostics",
    "ticket.implementation_result.write",
    "ticket.implementation_result.read",
];

const DELEGATE: &[&str] = &[
    "filesystem.read",
    "filesystem.write",
    "bash.execute",
    "web.fetch",
    "web.search",
    "objective.query",
    "objective.read",
    "repository.list",
    "repository.read",
    "workdir.read",
    "workdir.attachment.read",
    "worker.list",
    "worker.messaging",
    "subworker.spawn",
    "subworker.send_input",
    "subworker.observe",
    "subworker.stop",
    "ticket.query",
    "ticket.read",
    "ticket.relation.read",
    "ticket.event.add",
    "ticket.implementation_result.write",
    "ticket.implementation_result.read",
    "merge_request.open",
    "merge_request.read",
    "merge_request.readiness",
    "merge_request.review",
];

pub(crate) struct SubjectPermissionsFeature;

impl FeatureModule for SubjectPermissionsFeature {
    fn descriptor(&self) -> FeatureDescriptor {
        FeatureDescriptor::builtin("subject", "Subject permission semantics")
    }

    fn declare_permissions(
        &self,
        registrar: &mut FeaturePermissionRegistrar<'_>,
    ) -> Result<(), FeatureInstallError> {
        let namespace = NamespaceId::parse("subject").expect("static namespace");
        let handle = registrar.namespace(namespace.clone())?;
        let key = |value: &str| PermissionKey::parse(value).expect("static permission key");
        registrar.declare(&handle, NamespaceDeclaration {
            permissions: [
                ("subject.orchestrate", "Coordinate authorized work; body, target, control and lifecycle checks remain required.", ORCHESTRATE),
                (SUBJECT_DELEGATE_PERMISSION, "Ceiling for bounded implementation/review delegation; not body execution or an issued Worker grant.", DELEGATE),
            ].into_iter().map(|(name, description, children)| PermissionDefinition {
                key: key(name),
                description: description.into(),
                children: children.iter().map(|child| key(child)).collect(),
            }).collect(),
            sets: vec![SetContribution {
                set: PermissionSetId::parse(SUBJECT_DEFAULT_PERMISSION_SET).expect("static set"),
                children: vec![PermissionReference::Namespace(NamespacePermissions {
                    namespace,
                    keys: vec![key("subject.orchestrate")],
                })],
            }],
        })
    }

    fn install(&self, _: &mut FeatureInstallContext<'_>) -> Result<(), FeatureInstallError> {
        Ok(())
    }
}
