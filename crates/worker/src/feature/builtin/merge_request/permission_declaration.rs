//! Payload-free capabilities owned by this Feature, not by the legacy resource editor.

use crate::feature::permissions::{BuiltinPermissionDeclaration, BuiltinPermissionOperation};

pub(crate) const DECLARATION: BuiltinPermissionDeclaration = BuiltinPermissionDeclaration {
    feature: "merge-request",
    namespace: "merge_request",
    operations: &[
        BuiltinPermissionOperation {
            key: "merge_request.open",
            description: "Open or reuse a repository-scoped Merge Request for this Ticket.",
            sets: &["implementer", "companion", "work-execution"],
        },
        BuiltinPermissionOperation {
            key: "merge_request.read",
            description: "Read selected Merge Request and review evidence.",
            sets: &[
                "implementer",
                "companion",
                "work-coordination",
                "work-execution",
            ],
        },
        // Retained permission identities for stored policies/internal compatibility;
        // neither exposes a Tool, WIP operation, or HTTP route.
        BuiltinPermissionOperation {
            key: "merge_request.readiness",
            description: "Legacy readiness permission identity; readiness is now read through ShowMergeRequest.",
            sets: &[
                "implementer",
                "companion",
                "work-coordination",
                "work-execution",
            ],
        },
        BuiltinPermissionOperation {
            key: "merge_request.complete",
            description: "Legacy internal completion admission permission; Backend records automatic completion without a model-facing operation.",
            sets: &["implementer", "companion", "work-coordination"],
        },
        BuiltinPermissionOperation {
            key: "merge_request.review",
            description: "Submit a review only with independently bound review capability and current source.",
            sets: &["implementer", "companion", "work-execution"],
        },
        BuiltinPermissionOperation {
            key: "merge_request.assign",
            description: "Assign or release the current Worker for one Merge Request without changing its phase or approval.",
            sets: &["companion", "work-coordination"],
        },
        BuiltinPermissionOperation {
            key: "merge_request.comment",
            description: "Append current-Worker MR thread explanations with owner fencing and exact replay.",
            sets: &["implementer", "companion", "work-execution"],
        },
        BuiltinPermissionOperation {
            key: "merge_request.publish",
            description: "Allow Backend automatic publication of the independently approved fixed source to its MR target through durable normal fast-forward publication; no model-facing publish operation.",
            sets: &["implementer", "work-execution"],
        },
    ],
};
