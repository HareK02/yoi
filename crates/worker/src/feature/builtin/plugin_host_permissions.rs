//! The Host service owns these capabilities, not the guest Plugin. An adopted
//! package/plan is not a permission to invoke the Host or another Feature's service.
//! Callees still check the original principal, plan ceiling and current domain state.

use crate::feature::permissions::{BuiltinPermissionDeclaration, BuiltinPermissionOperation};

pub(crate) const DECLARATION: BuiltinPermissionDeclaration = BuiltinPermissionDeclaration {
    feature: "plugin-host",
    namespace: "plugin",
    operations: &[
        BuiltinPermissionOperation {
            key: "plugin.admit",
            description: "Admit an adopted Plugin plan subject to current installation, artifact and plan constraints.",
            sets: &["companion", "implementer"],
        },
        BuiltinPermissionOperation {
            key: "plugin.artifact.read",
            description: "Read a Plugin artifact only through its currently authorized execution delivery and immutable grant.",
            sets: &["companion", "implementer"],
        },
        BuiltinPermissionOperation {
            key: "plugin.service.consume",
            description: "Consume a selected Plugin service; the Host operation and callee Feature independently recheck the original principal.",
            sets: &["companion", "implementer"],
        },
        BuiltinPermissionOperation {
            key: "plugin.log",
            description: "Submit a bounded Plugin log event within the current plan and quota limits.",
            sets: &["companion", "implementer"],
        },
        BuiltinPermissionOperation {
            key: "plugin.publish",
            description: "Publish through the Plugin Host within the current execution, plan and output constraints.",
            sets: &["companion", "implementer"],
        },
        BuiltinPermissionOperation {
            key: "plugin.connection.open",
            description: "Open a Plugin Host connection under the plan's connection and provider restrictions.",
            sets: &["companion", "implementer"],
        },
        BuiltinPermissionOperation {
            key: "plugin.http.send",
            description: "Send an HTTP request through the Plugin Host under the plan's current destination, redirect and provider restrictions.",
            sets: &["companion", "implementer"],
        },
    ],
};
