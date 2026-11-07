//! Payload-free capabilities owned by this Feature, not by the legacy resource editor.

use crate::feature::permissions::{BuiltinPermissionDeclaration, BuiltinPermissionOperation};

pub(crate) const DECLARATION: BuiltinPermissionDeclaration = BuiltinPermissionDeclaration {
    feature: "drive",
    namespace: "drive",
    operations: &[
        BuiltinPermissionOperation {
            key: "drive.capabilities",
            description: "Read provider-supported Drive operations.",
            sets: &["implementer", "companion"],
        },
        BuiltinPermissionOperation {
            key: "drive.root",
            description: "Read the Workspace Drive root reference.",
            sets: &["implementer", "companion"],
        },
        BuiltinPermissionOperation {
            key: "drive.search",
            description: "Search the Workspace Drive; provider visibility still applies.",
            sets: &["implementer", "companion"],
        },
        BuiltinPermissionOperation {
            key: "drive.metadata",
            description: "Read metadata for this exact entry.",
            sets: &["implementer", "companion"],
        },
        BuiltinPermissionOperation {
            key: "drive.list",
            description: "List children of this exact folder; does not include descendants implicitly.",
            sets: &["implementer", "companion"],
        },
        BuiltinPermissionOperation {
            key: "drive.read_text",
            description: "Read text of this exact entry.",
            sets: &["implementer", "companion"],
        },
        BuiltinPermissionOperation {
            key: "drive.read_chunk",
            description: "Read a bounded chunk of this exact entry.",
            sets: &["implementer", "companion"],
        },
        BuiltinPermissionOperation {
            key: "drive.create_folder",
            description: "Create a folder under this exact parent.",
            sets: &["implementer", "companion"],
        },
        BuiltinPermissionOperation {
            key: "drive.create_text",
            description: "Create a text entry under this exact parent.",
            sets: &["implementer", "companion"],
        },
        BuiltinPermissionOperation {
            key: "drive.update_text",
            description: "Update this exact entry with provider concurrency checks.",
            sets: &["implementer", "companion"],
        },
        BuiltinPermissionOperation {
            key: "drive.delete",
            description: "Delete this exact entry if provider conditions permit.",
            sets: &["implementer", "companion"],
        },
        BuiltinPermissionOperation {
            key: "drive.relocate",
            description: "Move an entry; source and destination each require explicit permission.",
            sets: &["implementer", "companion"],
        },
        BuiltinPermissionOperation {
            key: "drive.upload",
            description: "Upload under this exact parent, subject to provider limits.",
            sets: &["implementer", "companion"],
        },
        BuiltinPermissionOperation {
            key: "drive.request_status",
            description: "Read the status of this exact provider request; no new execution authority.",
            sets: &["implementer", "companion"],
        },
    ],
};
