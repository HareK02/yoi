//! Payload-free capabilities owned by this Feature, not by the legacy resource editor.

use crate::feature::permissions::{BuiltinPermissionDeclaration, BuiltinPermissionOperation};

pub(crate) const DECLARATION: BuiltinPermissionDeclaration = BuiltinPermissionDeclaration {
    feature: "subjektiv-chat",
    namespace: "chat",
    operations: &[
        BuiltinPermissionOperation {
            key: "chat.read",
            description: "Read conversations belonging to the current Subject body; membership and current ownership remain required.",
            sets: &["implementer", "companion", "subject-default"],
        },
        BuiltinPermissionOperation {
            key: "chat.send",
            description: "Explicitly publish to a permitted conversation as the current Subject body. Background Worker messaging does not grant publication.",
            sets: &["implementer", "companion", "subject-default"],
        },
    ],
};
