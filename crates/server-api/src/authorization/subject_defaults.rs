//! Initial conversation authority, not a runtime fallback or a Profile capability.
//!
//! Hosts persist this set only while creating a new Subject. Existing Subjects opt in
//! with the normal administrator CAS replacement: read the current source, merge only
//! the desired entries into `execution`, preserve `delegation`, and submit that observed
//! change ID with a stable operation ID. Never apply it on launch, restore or migration;
//! an unset or explicitly emptied existing policy must remain denied. The Subject detail
//! permissions editor and `GET`/`POST /api/w/{workspace}/subjektiv/subjects/{subject}/permissions`
//! expose the same saved source and CAS. Replacing a source also invalidates previously
//! delegated children; an administrator must deliberately review that effect before applying.
//! No automatic opt-in or overwrite of individual exceptions is provided.
//!
//! `builtin:async-chat` requests the conversation composition and an explicit Subject
//! connection selects the body. Neither action writes policy. All entries below target
//! exactly that Subject in its storage scope; none covers Workspace, files, Workers or
//! other Subjects. Background Worker delegation is deliberately empty. Host-owned
//! Memory/Journal Jobs additionally require their kind/attempt/input/writer grants;
//! these source permissions alone do not create or authorize an arbitrary Job.
use super::{PermissionResource, ScopedPermission};

pub const SUBJECT_CONVERSATION_PROFILE: &str = "builtin:async-chat";

/// Basic body operations and the current-policy ceiling for standard memory Jobs.
/// Confirmed Memory mutation remains behind consolidation; explicit remembering stages
/// evidence, never directly edits confirmed Memory. Journal recording is a separate Job.
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
