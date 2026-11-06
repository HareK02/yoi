//! Payload-free capabilities owned by this Feature, not by the legacy resource editor.

use crate::feature::permissions::{BuiltinPermissionDeclaration, BuiltinPermissionOperation};

pub(crate) const DECLARATION: BuiltinPermissionDeclaration = BuiltinPermissionDeclaration {
    feature: "merge-request",
    namespace: "merge_request",
    operations: &[
        BuiltinPermissionOperation {
            key: "merge_request.open",
            description: "Open or reuse a repository-scoped Merge Request for this Ticket.",
            sets: &["implementer", "companion"],
        },
        BuiltinPermissionOperation {
            key: "merge_request.read",
            description: "Read selected Merge Request and review evidence.",
            sets: &["implementer", "companion"],
        },
        BuiltinPermissionOperation {
            key: "merge_request.readiness",
            description: "Check recording-workflow readiness; does not adopt changes.",
            sets: &["implementer", "companion"],
        },
        BuiltinPermissionOperation {
            key: "merge_request.review",
            description: "Submit a review only with independently bound review capability and current source.",
            sets: &["implementer", "companion"],
        },
        BuiltinPermissionOperation {
            key: "merge_request.complete",
            description: "Record an already-applied exact-source integration with existing approval and source checks.",
            sets: &["implementer", "companion"],
        },
    ],
};
