//! Payload-free capabilities owned by this Feature, not by the legacy resource editor.

use crate::feature::permissions::{BuiltinPermissionDeclaration, BuiltinPermissionOperation};

pub(crate) const DECLARATION: BuiltinPermissionDeclaration = BuiltinPermissionDeclaration {
    feature: "journal-recording",
    namespace: "journal",
    operations: &[
        BuiltinPermissionOperation {
            key: "journal.startup.read",
            description: "Read this Subject's startup Journal context.",
            sets: &["implementer", "companion", "conversation"],
        },
        BuiltinPermissionOperation {
            key: "journal.startup.supply",
            description: "Supply protected startup Journal context to a model.",
            sets: &["implementer", "companion", "conversation"],
        },
        BuiltinPermissionOperation {
            key: "journal.compaction.read",
            description: "Read this Subject's compaction Journal context.",
            sets: &["implementer", "companion", "conversation"],
        },
        BuiltinPermissionOperation {
            key: "journal.compaction.supply",
            description: "Supply protected compaction context to a model.",
            sets: &["implementer", "companion", "conversation"],
        },
        BuiltinPermissionOperation {
            key: "journal.recording.request",
            description: "Request a bounded recording Job; does not grant arbitrary Job operations.",
            sets: &["implementer", "companion", "conversation"],
        },
        BuiltinPermissionOperation {
            key: "journal.job.input.read",
            description: "Read frozen input only with the bound current Journal Job capability.",
            sets: &["implementer", "companion", "conversation"],
        },
        BuiltinPermissionOperation {
            key: "journal.job.input.supply",
            description: "Supply frozen input only with current Job, origin and policy authority.",
            sets: &["implementer", "companion", "conversation"],
        },
        BuiltinPermissionOperation {
            key: "journal.memory.links.reconcile",
            description: "Reconcile this Subject's durable Memory/Journal links under the current body and source-presence constraints; not a Job or writer grant.",
            sets: &["implementer", "companion", "conversation"],
        },
        BuiltinPermissionOperation {
            key: "journal.memory.links.read",
            description: "Read this Subject's Memory/Journal link metadata without restoring a source Session or granting access to other Subjects.",
            sets: &["implementer", "companion", "conversation"],
        },
        BuiltinPermissionOperation {
            key: "journal.memory.read",
            description: "Read saved Journal evidence linked to this Subject's Memory through the current body reader context.",
            sets: &["implementer", "companion", "conversation"],
        },
        BuiltinPermissionOperation {
            key: "journal.memory.related.read",
            description: "Read related Memory/Journal evidence under the current Subject body and domain selector constraints.",
            sets: &["implementer", "companion", "conversation"],
        },
        BuiltinPermissionOperation {
            key: "journal.state.read",
            description: "Read this Subject's Journal state.",
            sets: &["implementer", "companion", "conversation"],
        },
        BuiltinPermissionOperation {
            key: "journal.text.read",
            description: "Read this Subject's Journal text.",
            sets: &["implementer", "companion", "conversation"],
        },
        BuiltinPermissionOperation {
            key: "journal.operation.query",
            description: "Query retained Journal operations.",
            sets: &["implementer", "companion", "conversation"],
        },
        BuiltinPermissionOperation {
            key: "journal.operation.list",
            description: "List retained Journal operations.",
            sets: &["implementer", "companion", "conversation"],
        },
        BuiltinPermissionOperation {
            key: "journal.operation.coverage.read",
            description: "Read operation coverage evidence.",
            sets: &["implementer", "companion", "conversation"],
        },
        BuiltinPermissionOperation {
            key: "journal.entry.create",
            description: "Create an entry within the bound Journal Job.",
            sets: &["implementer", "companion", "conversation"],
        },
        BuiltinPermissionOperation {
            key: "journal.entry.fix",
            description: "Fix an entry within the bound Journal Job.",
            sets: &["implementer", "companion", "conversation"],
        },
        BuiltinPermissionOperation {
            key: "journal.entry.rewrite",
            description: "Rewrite an entry within the bound Journal Job.",
            sets: &["implementer", "companion", "conversation"],
        },
        BuiltinPermissionOperation {
            key: "journal.position.update",
            description: "Update position within the current Journal writer/attempt.",
            sets: &["implementer", "companion", "conversation"],
        },
        BuiltinPermissionOperation {
            key: "journal.unchanged.confirm",
            description: "Confirm no change within the current Journal writer/attempt.",
            sets: &["implementer", "companion", "conversation"],
        },
        BuiltinPermissionOperation {
            key: "journal.checkpoint",
            description: "Checkpoint the current Journal recording Job.",
            sets: &["implementer", "companion", "conversation"],
        },
        BuiltinPermissionOperation {
            key: "journal.finish",
            description: "Finish the bound Journal Job; not a general work-completion judgment.",
            sets: &["implementer", "companion", "conversation"],
        },
    ],
};
