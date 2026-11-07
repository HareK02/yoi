//! Payload-free capabilities owned by this Feature, not by the legacy resource editor.

use crate::feature::permissions::{BuiltinPermissionDeclaration, BuiltinPermissionOperation};

pub(crate) const DECLARATION: BuiltinPermissionDeclaration = BuiltinPermissionDeclaration {
    feature: "workspace-config",
    namespace: "workspace_config",
    operations: &[
        BuiltinPermissionOperation {
            key: "workspace_config.attach",
            description: "Attach the selected configuration path and recursive scope.",
            sets: &["implementer"],
        },
        BuiltinPermissionOperation {
            key: "workspace_config.observe",
            description: "Observe selected configuration metadata within path scope.",
            sets: &["implementer"],
        },
        BuiltinPermissionOperation {
            key: "workspace_config.read",
            description: "Read selected configuration within path scope; secret rules still apply.",
            sets: &["implementer"],
        },
        BuiltinPermissionOperation {
            key: "workspace_config.commit",
            description: "Commit selected configuration within path scope and provider compare checks.",
            sets: &["implementer"],
        },
    ],
};
