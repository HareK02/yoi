//! Payload-free capabilities owned by this Feature, not by the legacy resource editor.

use crate::feature::permissions::{BuiltinPermissionDeclaration, BuiltinPermissionOperation};

pub(crate) const DECLARATION: BuiltinPermissionDeclaration = BuiltinPermissionDeclaration {
    feature: "repository",
    namespace: "repository",
    operations: &[
        BuiltinPermissionOperation {
            key: "repository.list",
            description: "List repository metadata, not checkout or credential access.",
            sets: &["implementer", "companion"],
        },
        BuiltinPermissionOperation {
            key: "repository.read",
            description: "Read selected repository metadata without exposing secrets.",
            sets: &["implementer", "companion"],
        },
    ],
};
