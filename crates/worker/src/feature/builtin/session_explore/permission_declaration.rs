//! Payload-free capabilities owned by this Feature, not by the legacy resource editor.

use crate::feature::permissions::{BuiltinPermissionDeclaration, BuiltinPermissionOperation};

pub(crate) const DECLARATION: BuiltinPermissionDeclaration = BuiltinPermissionDeclaration {
    feature: "session-explore",
    namespace: "session",
    operations: &[
        BuiltinPermissionOperation {
            key: "session.record",
            description: "Attribute and record sessions for this Subject; current body ownership is still required.",
            sets: &["implementer", "companion", "conversation"],
        },
        BuiltinPermissionOperation {
            key: "session.list",
            description: "List sessions attributed to this Subject.",
            sets: &["implementer", "companion", "conversation"],
        },
        BuiltinPermissionOperation {
            key: "session.search",
            description: "Search committed sessions attributed to this Subject.",
            sets: &["implementer", "companion", "conversation"],
        },
        BuiltinPermissionOperation {
            key: "session.read",
            description: "Read committed entries from this Subject's sessions.",
            sets: &["implementer", "companion", "conversation"],
        },
    ],
};
