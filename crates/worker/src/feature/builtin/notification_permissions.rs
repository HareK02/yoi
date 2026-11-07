//! Payload-free capabilities owned by this Feature, not by the legacy resource editor.

use crate::feature::permissions::{BuiltinPermissionDeclaration, BuiltinPermissionOperation};

pub(crate) const DECLARATION: BuiltinPermissionDeclaration = BuiltinPermissionDeclaration {
    feature: "notification",
    namespace: "notification",
    operations: &[BuiltinPermissionOperation {
        key: "notification.diagnostics.read",
        description: "Read saved delivery evidence; acceptance is not processing or execution authority.",
        sets: &["implementer", "companion", "subject-default"],
    }],
};
