//! New Subjects persist a shared-set reference, not a fixed copy of operation keys.
//! Feature declarations own its current contributions. This is creation input only,
//! never a launch, restore, migration or unset-policy fallback. Existing individual,
//! empty, excluded and revoked policies must remain unchanged.
//!
//! The legacy scoped helpers below remain for reading/migrating old callers. They
//! are not the catalog or the new Subject default. Subject/domain ownership, Job
//! kind/attempt/input/writer and provider constraints remain independent of keys.
use super::{PermissionResource, ScopedPermission};

pub const SUBJECT_CONVERSATION_PROFILE: &str = "builtin:async-chat";

/// Conversation and its retained context, contributed by the owning Features.
pub const CONVERSATION_PERMISSION_SET: &str = "conversation";
/// Coordination of work, without filesystem or command execution.
pub const WORK_COORDINATION_PERMISSION_SET: &str = "work-coordination";
/// Bounded implementation, investigation and review capabilities.
pub const WORK_EXECUTION_PERMISSION_SET: &str = "work-execution";
pub const COMPANION_PERMISSION_SET: &str = "companion";
pub const IMPLEMENTER_PERMISSION_SET: &str = "implementer";

/// Persist this reference when creating a new Subject. A shared-set reference is
/// intentionally live; limited child/Job grants must instead freeze issued keys.
pub fn subject_default_permission_assignment() -> feature_permissions::PermissionAssignment {
    shared_set_assignment(&[
        CONVERSATION_PERMISSION_SET,
        WORK_COORDINATION_PERMISSION_SET,
    ])
}

/// Creation-only delegation ceiling. Existing empty/revoked delegation stays empty;
/// each Worker request must still freeze explicit operation/resource grants, including
/// any bounded Reviewer redelegation, within the currently resolved children.
pub fn subject_default_delegation_assignment() -> feature_permissions::PermissionAssignment {
    shared_set_assignment(&[WORK_EXECUTION_PERMISSION_SET])
}

fn shared_set_assignment(sets: &[&str]) -> feature_permissions::PermissionAssignment {
    feature_permissions::PermissionAssignment {
        grants: sets
            .iter()
            .map(|set| {
                feature_permissions::PermissionReference::Set(
                    feature_permissions::PermissionSetId::parse(*set)
                        .expect("static permission set ID"),
                )
            })
            .collect(),
        ..Default::default()
    }
}

/// Legacy exact-Subject scoped operations retained for old policy migration.
/// New Subject creation must use `subject_default_permission_assignment` instead.
pub const SUBJECT_CONVERSATION_OPERATIONS: &[&str] = &[
    "session.record",
    "session.list",
    "session.search",
    "session.read",
    "chat.read",
    "chat.send",
    "memory.query",
    "memory.read",
    "memory.changes.list",
    "memory.resident.read",
    "memory.resident.supply",
    "memory.staging.write",
    "memory.staging.list",
    "memory.staging.read",
    "memory.receipt.read",
    "memory.proposal.validate",
    "memory.candidate.decide",
    "memory.extract.read",
    "memory.extract.supply",
    "memory.extract.commit",
    "memory.consolidation.request",
    "memory.consolidation.supply",
    "memory.surface.prepare",
    "memory.surface.supply",
    "memory.surface.publish",
    "memory.surface.fail",
    "journal.startup.read",
    "journal.startup.supply",
    "journal.compaction.read",
    "journal.compaction.supply",
    "journal.recording.request",
    "journal.job.input.read",
    "journal.job.input.supply",
    "journal.state.read",
    "journal.text.read",
    "journal.operation.query",
    "journal.operation.list",
    "journal.operation.coverage.read",
    "journal.entry.create",
    "journal.entry.fix",
    "journal.entry.rewrite",
    "journal.position.update",
    "journal.unchanged.confirm",
    "journal.checkpoint",
    "journal.finish",
];

pub fn subject_conversation_permissions(scope: &str, subject: &str) -> Vec<ScopedPermission> {
    SUBJECT_CONVERSATION_OPERATIONS
        .iter()
        .map(|operation| ScopedPermission {
            operation: (*operation).into(),
            resource: PermissionResource::Subject {
                workspace_id: scope.into(),
                subject_id: subject.into(),
            },
        })
        .collect()
}
