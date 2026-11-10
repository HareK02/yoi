//! Payload-free capabilities owned by this Feature, not by the legacy resource editor.

use crate::feature::permissions::{BuiltinPermissionDeclaration, BuiltinPermissionOperation};

pub(crate) const DECLARATION: BuiltinPermissionDeclaration = BuiltinPermissionDeclaration {
    feature: "objective",
    namespace: "objective",
    operations: &[
        BuiltinPermissionOperation {
            key: "objective.query",
            description: "Query the Workspace Objective collection.",
            sets: &[
                "implementer",
                "companion",
                "work-coordination",
                "work-execution",
            ],
        },
        BuiltinPermissionOperation {
            key: "objective.create",
            description: "Create an Objective in this Workspace.",
            sets: &["implementer", "companion", "work-coordination"],
        },
        BuiltinPermissionOperation {
            key: "objective.read",
            description: "Read selected Objective and linked Ticket context.",
            sets: &[
                "implementer",
                "companion",
                "work-coordination",
                "work-execution",
            ],
        },
        BuiltinPermissionOperation {
            key: "objective.edit",
            description: "Edit selected Objective content.",
            sets: &["implementer", "companion", "work-coordination"],
        },
        BuiltinPermissionOperation {
            key: "objective.set_state",
            description: "Change selected Objective state.",
            sets: &["implementer", "companion", "work-coordination"],
        },
        BuiltinPermissionOperation {
            key: "objective.link_ticket",
            description: "Link a Ticket to this Objective; required Ticket read permission is separate.",
            sets: &["implementer", "companion", "work-coordination"],
        },
        BuiltinPermissionOperation {
            key: "objective.unlink_ticket",
            description: "Unlink a Ticket from this Objective.",
            sets: &["implementer", "companion", "work-coordination"],
        },
    ],
};
