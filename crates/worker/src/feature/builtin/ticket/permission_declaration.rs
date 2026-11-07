//! Payload-free capabilities owned by this Feature, not by the legacy resource editor.

use crate::feature::permissions::{BuiltinPermissionDeclaration, BuiltinPermissionOperation};

pub(crate) const DECLARATION: BuiltinPermissionDeclaration = BuiltinPermissionDeclaration {
    feature: "ticket",
    namespace: "ticket",
    operations: &[
        BuiltinPermissionOperation {
            key: "ticket.create",
            description: "Create open Tickets in this Workspace; no read or edit rights to existing Tickets are implied.",
            sets: &["implementer", "companion"],
        },
        BuiltinPermissionOperation {
            key: "ticket.query",
            description: "Query the Workspace Ticket collection.",
            sets: &["implementer", "companion", "subject-default"],
        },
        BuiltinPermissionOperation {
            key: "ticket.list",
            description: "List the Workspace Ticket collection.",
            sets: &["implementer", "companion"],
        },
        BuiltinPermissionOperation {
            key: "ticket.doctor",
            description: "Inspect Workspace Ticket diagnostics.",
            sets: &["implementer", "companion"],
        },
        BuiltinPermissionOperation {
            key: "ticket.intake.default_body",
            description: "Read the default intake transition body.",
            sets: &["implementer", "companion"],
        },
        BuiltinPermissionOperation {
            key: "ticket.tag.list",
            description: "Read Workspace classification tags.",
            sets: &["implementer", "companion"],
        },
        BuiltinPermissionOperation {
            key: "ticket.tag.create",
            description: "Create Workspace classification tags, not workflow authority.",
            sets: &["implementer", "companion"],
        },
        BuiltinPermissionOperation {
            key: "ticket.tag.rename",
            description: "Rename Workspace tags with existing replay and compare checks.",
            sets: &["implementer", "companion"],
        },
        BuiltinPermissionOperation {
            key: "ticket.tag.delete",
            description: "Delete Workspace tags and their memberships with existing compare checks.",
            sets: &["implementer", "companion"],
        },
        BuiltinPermissionOperation {
            key: "ticket.relation.query",
            description: "Query Workspace Ticket relation metadata.",
            sets: &["implementer", "companion"],
        },
        BuiltinPermissionOperation {
            key: "ticket.read",
            description: "Read the selected Ticket and its thread.",
            sets: &["implementer", "companion", "subject-default"],
        },
        BuiltinPermissionOperation {
            key: "ticket.extension.write",
            description: "Write Plugin-owned extension data on an authorized Ticket; the Plugin plan, schema and data CAS remain separate limits.",
            sets: &[],
        },
        BuiltinPermissionOperation {
            key: "ticket.edit_item",
            description: "Edit the selected Ticket content; actor and concurrency checks still apply.",
            sets: &["implementer", "companion"],
        },
        BuiltinPermissionOperation {
            key: "ticket.edit_post",
            description: "Edit an eligible post using its exact observed body.",
            sets: &["implementer", "companion"],
        },
        BuiltinPermissionOperation {
            key: "ticket.event.add",
            description: "Append an allowed typed event to the selected Ticket.",
            sets: &["implementer", "companion"],
        },
        BuiltinPermissionOperation {
            key: "ticket.case.update",
            description: "Close, reopen or correct closure with explicit judgment and current compare keys.",
            sets: &["implementer", "companion"],
        },
        BuiltinPermissionOperation {
            key: "ticket.dependency.check",
            description: "Inspect selected Ticket dependencies; does not start them.",
            sets: &["implementer", "companion"],
        },
        BuiltinPermissionOperation {
            key: "ticket.intake_summary.add",
            description: "Append an intake summary to the selected Ticket.",
            sets: &["implementer", "companion"],
        },
        BuiltinPermissionOperation {
            key: "ticket.tags.set",
            description: "Replace selected Ticket classification tags using the observed full set.",
            sets: &["implementer", "companion"],
        },
        BuiltinPermissionOperation {
            key: "ticket.relation.read",
            description: "Read selected Ticket relation and bounded parent/children link metadata; grants no access to relative Ticket bodies.",
            sets: &["implementer", "companion", "subject-default"],
        },
        BuiltinPermissionOperation {
            key: "ticket.relation.record",
            description: "Record an explicit relation or parent link on the authorized endpoints; reparenting also requires removal authority for the old link.",
            sets: &["implementer", "companion"],
        },
        BuiltinPermissionOperation {
            key: "ticket.relation.remove",
            description: "Remove an exact relation or observed parent link on the authorized endpoints.",
            sets: &["implementer", "companion"],
        },
        BuiltinPermissionOperation {
            key: "ticket.mark_ready",
            description: "Record an intake readiness change; does not grant execution authority.",
            sets: &["implementer", "companion"],
        },
        BuiltinPermissionOperation {
            key: "ticket.queue",
            description: "Use the existing authorized Ticket queue route; does not bypass actor restrictions.",
            sets: &["implementer", "companion"],
        },
        BuiltinPermissionOperation {
            key: "ticket.assignment.assign",
            description: "Assign to an authorized Subject; current body, Ticket targets and execution grants are checked separately. Assignment does not start a Worker.",
            sets: &["implementer", "companion"],
        },
        BuiltinPermissionOperation {
            key: "ticket.assignment.clear",
            description: "Release the observed current Subject assignment; does not stop Workers.",
            sets: &["implementer", "companion"],
        },
        BuiltinPermissionOperation {
            key: "ticket.assignment.update",
            description: "Report handling only for the authenticated authorized current delegation with event CAS.",
            sets: &["implementer", "companion", "subject-default"],
        },
        BuiltinPermissionOperation {
            key: "ticket.assignment.answer",
            description: "Append fenced continuing input to the current assignment; not a new assignment.",
            sets: &["implementer", "companion"],
        },
        BuiltinPermissionOperation {
            key: "ticket.assignment.query",
            description: "Read selected Ticket assignment tracking.",
            sets: &["implementer", "companion", "subject-default"],
        },
        BuiltinPermissionOperation {
            key: "ticket.assignment.history",
            description: "Read retained assignment tracking events.",
            sets: &["implementer", "companion", "subject-default"],
        },
        BuiltinPermissionOperation {
            key: "ticket.assignment.diagnostics",
            description: "Read retained delivery and start evidence, not execution authority.",
            sets: &["implementer", "companion", "subject-default"],
        },
        BuiltinPermissionOperation {
            key: "ticket.implementation_result.write",
            description: "Save a bounded result, handoff or blocker; not approval or closure.",
            sets: &["implementer", "companion", "subject-default"],
        },
        BuiltinPermissionOperation {
            key: "ticket.implementation_result.read",
            description: "Read a saved implementation result; not a new request.",
            sets: &["implementer", "companion", "subject-default"],
        },
    ],
};
