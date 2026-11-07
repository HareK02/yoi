//! Payload-free capabilities owned by this Feature, not by the legacy resource editor.

use crate::feature::permissions::{BuiltinPermissionDeclaration, BuiltinPermissionOperation};

pub(crate) const DECLARATION: BuiltinPermissionDeclaration = BuiltinPermissionDeclaration {
    feature: "memory",
    namespace: "memory",
    operations: &[
        BuiltinPermissionOperation {
            key: "memory.query",
            description: "Query this Subject's Memory, not Workspace Memory.",
            sets: &["implementer", "companion", "subject-default"],
        },
        BuiltinPermissionOperation {
            key: "memory.read",
            description: "Read this Subject's selected Memory document.",
            sets: &["implementer", "companion", "subject-default"],
        },
        BuiltinPermissionOperation {
            key: "memory.changes.list",
            description: "Read this Subject's retained Memory changes.",
            sets: &["implementer", "companion", "subject-default"],
        },
        BuiltinPermissionOperation {
            key: "memory.resident.read",
            description: "Read this Subject's resident Memory data.",
            sets: &["implementer", "companion", "subject-default"],
        },
        BuiltinPermissionOperation {
            key: "memory.resident.supply",
            description: "Supply protected resident Memory to a model; read permission is separate.",
            sets: &["implementer", "companion", "subject-default"],
        },
        BuiltinPermissionOperation {
            key: "memory.staging.write",
            description: "Write a candidate to this Subject's staging area.",
            sets: &["implementer", "companion", "subject-default"],
        },
        BuiltinPermissionOperation {
            key: "memory.staging.list",
            description: "List this Subject's staging candidates.",
            sets: &["implementer", "companion", "subject-default"],
        },
        BuiltinPermissionOperation {
            key: "memory.staging.read",
            description: "Read this Subject's staging candidate.",
            sets: &["implementer", "companion", "subject-default"],
        },
        BuiltinPermissionOperation {
            key: "memory.candidate.decide",
            description: "Decide a staged candidate using existing Job/domain constraints.",
            sets: &["implementer", "companion", "subject-default"],
        },
        BuiltinPermissionOperation {
            key: "memory.proposal.validate",
            description: "Validate a proposal for this Subject.",
            sets: &["implementer", "companion", "subject-default"],
        },
        BuiltinPermissionOperation {
            key: "memory.receipt.read",
            description: "Read retained Memory operation evidence, not new authority.",
            sets: &["implementer", "companion", "subject-default"],
        },
        BuiltinPermissionOperation {
            key: "memory.surface.prepare",
            description: "Prepare this Subject's surface update under domain checks.",
            sets: &["implementer", "companion", "subject-default"],
        },
        BuiltinPermissionOperation {
            key: "memory.surface.supply",
            description: "Supply protected surface input to a model.",
            sets: &["implementer", "companion", "subject-default"],
        },
        BuiltinPermissionOperation {
            key: "memory.surface.publish",
            description: "Publish this Subject's prepared surface under writer and Job checks.",
            sets: &["implementer", "companion", "subject-default"],
        },
        BuiltinPermissionOperation {
            key: "memory.surface.fail",
            description: "Record failure of this Subject's prepared surface.",
            sets: &["implementer", "companion", "subject-default"],
        },
        BuiltinPermissionOperation {
            key: "memory.extract.read",
            description: "Read protected session input for Memory extraction.",
            sets: &["implementer", "companion", "subject-default"],
        },
        BuiltinPermissionOperation {
            key: "memory.extract.supply",
            description: "Supply protected extraction input to a model.",
            sets: &["implementer", "companion", "subject-default"],
        },
        BuiltinPermissionOperation {
            key: "memory.extract.commit",
            description: "Commit extracted candidates under existing source/writer checks.",
            sets: &["implementer", "companion", "subject-default"],
        },
        BuiltinPermissionOperation {
            key: "memory.consolidation.request",
            description: "Request a bounded consolidation Job; Job grants and origin constraints remain separate.",
            sets: &["implementer", "companion", "subject-default"],
        },
        BuiltinPermissionOperation {
            key: "memory.consolidation.supply",
            description: "Supply frozen consolidation input only within its current Job capability.",
            sets: &["implementer", "companion", "subject-default"],
        },
        BuiltinPermissionOperation {
            key: "memory.document.read",
            description: "Read a legacy Workspace Memory document; not Subject Memory.",
            sets: &["implementer", "companion"],
        },
        BuiltinPermissionOperation {
            key: "memory.document.update",
            description: "Update a legacy Workspace Memory document with existing storage constraints.",
            sets: &["implementer", "companion"],
        },
        BuiltinPermissionOperation {
            key: "memory.audit.append",
            description: "Append a Workspace Memory audit event.",
            sets: &["implementer", "companion"],
        },
        BuiltinPermissionOperation {
            key: "memory.staging.close",
            description: "Close a legacy Workspace staging candidate.",
            sets: &["implementer", "companion"],
        },
    ],
};
