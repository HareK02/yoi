//! Payload-free capabilities owned by this Feature, not by the legacy resource editor.

use crate::feature::permissions::{BuiltinPermissionDeclaration, BuiltinPermissionOperation};

pub(crate) const FILESYSTEM: BuiltinPermissionDeclaration = BuiltinPermissionDeclaration {
    feature: "workdir",
    namespace: "filesystem",
    operations: &[
        BuiltinPermissionOperation {
            key: "filesystem.read",
            description: "Read files and enumerate directories within the selected path and recursive scope; provider scope still applies.",
            sets: &["implementer"],
        },
        BuiltinPermissionOperation {
            key: "filesystem.write",
            description: "Create or edit files within the selected path and recursive scope; does not permit commands.",
            sets: &["implementer"],
        },
    ],
};

pub(crate) const DECLARATION: BuiltinPermissionDeclaration = BuiltinPermissionDeclaration {
    feature: "manage-workdir",
    namespace: "workdir",
    operations: &[
        BuiltinPermissionOperation {
            key: "workdir.read",
            description: "Read selected Workdir metadata; does not grant file access.",
            sets: &["implementer"],
        },
        BuiltinPermissionOperation {
            key: "workdir.attach",
            description: "Attach the selected Workdir subject to existing provider and attachment constraints.",
            sets: &["implementer"],
        },
        BuiltinPermissionOperation {
            key: "workdir.delete",
            description: "Delete the selected Workdir only if existing occupancy and retention conditions permit it.",
            sets: &["implementer"],
        },
        BuiltinPermissionOperation {
            key: "workdir.list",
            description: "List Workdir metadata; does not grant attachment or file access.",
            sets: &["implementer"],
        },
        BuiltinPermissionOperation {
            key: "workdir.attachment.list",
            description: "List Workdir attachment metadata.",
            sets: &["implementer"],
        },
        BuiltinPermissionOperation {
            key: "workdir.create",
            description: "Materialize selected repository using existing provider grants and explicit selectors.",
            sets: &["implementer"],
        },
        BuiltinPermissionOperation {
            key: "workdir.attachment.read",
            description: "Read metadata for this exact attachment connection.",
            sets: &["implementer"],
        },
        BuiltinPermissionOperation {
            key: "workdir.detach",
            description: "Detach this exact observed connection; stale aliases do not select a replacement.",
            sets: &["implementer"],
        },
    ],
};
