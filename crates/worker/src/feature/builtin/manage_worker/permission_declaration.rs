//! Payload-free capabilities owned by this Feature, not by the legacy resource editor.

use crate::feature::permissions::{BuiltinPermissionDeclaration, BuiltinPermissionOperation};

pub(crate) const DECLARATION: BuiltinPermissionDeclaration = BuiltinPermissionDeclaration {
    feature: "worker",
    namespace: "worker",
    operations: &[
        BuiltinPermissionOperation {
            key: "worker.list",
            description: "List known Workers. Discovery does not grant control.",
            sets: &["implementer"],
        },
        BuiltinPermissionOperation {
            key: "worker.discover",
            description: "Search bounded Worker metadata. Discovery does not grant control or observation.",
            sets: &["implementer"],
        },
        BuiltinPermissionOperation {
            key: "worker.observe.list",
            description: "List observation subjects; does not grant access to individual sessions.",
            sets: &["implementer"],
        },
        BuiltinPermissionOperation {
            key: "worker.create",
            description: "Use Profile-resolved Worker creation with explicit execution grants. Directional Backend grants separately control the created Worker and its message-only reply.",
            sets: &["implementer"],
        },
        BuiltinPermissionOperation {
            key: "worker.messaging",
            description: "Operation capability only; directional control grants and current lifecycle constraints remain required.",
            sets: &["implementer"],
        },
        BuiltinPermissionOperation {
            key: "worker.send_input",
            description: "Operation capability only; directional control grants and current lifecycle constraints remain required.",
            sets: &["implementer"],
        },
        BuiltinPermissionOperation {
            key: "worker.cancel",
            description: "Operation capability only; directional control grants and current lifecycle constraints remain required.",
            sets: &["implementer"],
        },
        BuiltinPermissionOperation {
            key: "worker.stop",
            description: "Operation capability only; directional control grants and current lifecycle constraints remain required.",
            sets: &["implementer"],
        },
        BuiltinPermissionOperation {
            key: "worker.restore",
            description: "Operation capability only; directional control grants and current lifecycle constraints remain required.",
            sets: &["implementer"],
        },
        BuiltinPermissionOperation {
            key: "worker.remove",
            description: "Operation capability only; directional control grants and current lifecycle constraints remain required.",
            sets: &["implementer"],
        },
        BuiltinPermissionOperation {
            key: "worker.observe",
            description: "Operation capability only; directional control grants and current lifecycle constraints remain required.",
            sets: &["implementer"],
        },
    ],
};

pub(crate) const SUBWORKER: BuiltinPermissionDeclaration = BuiltinPermissionDeclaration {
    feature: "sub-worker-control",
    namespace: "subworker",
    operations: &[
        BuiltinPermissionOperation {
            key: "subworker.spawn",
            description: "Parent-owned child operation; exact parent, name and delegated provider limits remain required.",
            sets: &["implementer"],
        },
        BuiltinPermissionOperation {
            key: "subworker.send_input",
            description: "Parent-owned child operation; exact parent, name and delegated provider limits remain required.",
            sets: &["implementer"],
        },
        BuiltinPermissionOperation {
            key: "subworker.stop",
            description: "Parent-owned child operation; exact parent, name and delegated provider limits remain required.",
            sets: &["implementer"],
        },
        BuiltinPermissionOperation {
            key: "subworker.observe",
            description: "Parent-owned child operation; exact parent, name and delegated provider limits remain required.",
            sets: &["implementer"],
        },
    ],
};
