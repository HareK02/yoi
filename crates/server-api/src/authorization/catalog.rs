//! Management metadata over the existing policy, not an authorization source.
//!
//! Only the management replacement path validates against this catalog. Historical
//! deserialization and execution grants retain their exact spelling and scope. New
//! operations require an explicit catalog entry; dotted prefixes grant nothing.
use super::*;

#[cfg(test)]
#[path = "catalog_tests.rs"]
mod tests;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum PermissionResourceKind {
    PluginInstallation,
    Job,
    Ticket,
    Objective,
    MergeRequest,
    Subject,
    DriveEntry,
    DriveRequest,
    Repository,
    WorkdirAttachment,
    WorkspaceConfig,
    Network,
    SubWorker,
    Feature,
    Workspace,
    Workdir,
    Worker,
    WorkerOperation,
}

impl PermissionResource {
    pub fn kind(&self) -> PermissionResourceKind {
        use PermissionResourceKind as K;
        match self {
            Self::PluginInstallation { .. } => K::PluginInstallation,
            Self::Job { .. } => K::Job,
            Self::Ticket { .. } => K::Ticket,
            Self::Objective { .. } => K::Objective,
            Self::MergeRequest { .. } => K::MergeRequest,
            Self::Subject { .. } => K::Subject,
            Self::DriveEntry { .. } => K::DriveEntry,
            Self::DriveRequest { .. } => K::DriveRequest,
            Self::Repository { .. } => K::Repository,
            Self::WorkdirAttachment { .. } => K::WorkdirAttachment,
            Self::WorkspaceConfig { .. } => K::WorkspaceConfig,
            Self::Network { .. } => K::Network,
            Self::SubWorker { .. } => K::SubWorker,
            Self::Feature => K::Feature,
            Self::Workspace { .. } => K::Workspace,
            Self::Workdir { .. } => K::Workdir,
            Self::Worker { .. } => K::Worker,
            Self::WorkerOperation { .. } => K::WorkerOperation,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PermissionOperationAvailability {
    Available,
    Unavailable { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct PermissionOperationDescriptor {
    pub operation: String,
    pub label: String,
    pub description: String,
    /// Accepted management input kinds, not a claim that a target exists or is usable.
    pub resource_kinds: Vec<PermissionResourceKind>,
    pub availability: PermissionOperationAvailability,
}

/// Available means the operation/resource contract is implemented, not that the
/// current Profile, provider, target, assignment or Job permits its execution.
/// Body execution and delegation ceilings use the same descriptors, independently.
pub fn subject_permission_operations() -> Vec<PermissionOperationDescriptor> {
    use PermissionResourceKind as K;
    let mut catalog = Vec::new();
    let mut add = |kind, entries: &[(&str, &str, &str)]| {
        catalog.extend(entries.iter().map(|(operation, label, description)| {
            PermissionOperationDescriptor {
                operation: (*operation).into(),
                label: (*label).into(),
                description: (*description).into(),
                resource_kinds: vec![kind],
                availability: PermissionOperationAvailability::Available,
            }
        }));
    };
    add(
        K::Workdir,
        &[
            (
                "filesystem.read",
                "Read files",
                "Read files and enumerate directories within the selected path and recursive scope; provider scope still applies.",
            ),
            (
                "filesystem.write",
                "Edit files",
                "Create or edit files within the selected path and recursive scope; does not permit commands.",
            ),
            (
                "bash.execute",
                "Run commands",
                "Requires the whole Workdir: path '.' and recursive true. A shell is not confined to a subdirectory. Explicit provider command authority is also required.",
            ),
            (
                "workdir.read",
                "Inspect Workdir",
                "Read selected Workdir metadata; does not grant file access.",
            ),
            (
                "workdir.attach",
                "Attach Workdir",
                "Attach the selected Workdir subject to existing provider and attachment constraints.",
            ),
            (
                "workdir.delete",
                "Delete Workdir",
                "Delete the selected Workdir only if existing occupancy and retention conditions permit it.",
            ),
        ],
    );
    add(
        K::Network,
        &[
            (
                "web.fetch",
                "Fetch web page",
                "Fetch this exact HTTP(S) URL. Each redirect needs its own permission; private/local hosts remain denied.",
            ),
            (
                "web.search",
                "Search web",
                "Contact this exact search provider endpoint; credentials and network restrictions still apply.",
            ),
        ],
    );
    add(
        K::Workspace,
        &[
            (
                "ticket.create",
                "Create Tickets",
                "Create open Tickets in this Workspace; no read or edit rights to existing Tickets are implied.",
            ),
            (
                "ticket.query",
                "Search Tickets",
                "Query the Workspace Ticket collection.",
            ),
            (
                "ticket.list",
                "List Tickets",
                "List the Workspace Ticket collection.",
            ),
            (
                "ticket.doctor",
                "Diagnose Tickets",
                "Inspect Workspace Ticket diagnostics.",
            ),
            (
                "ticket.intake.default_body",
                "Read intake template",
                "Read the default intake transition body.",
            ),
            (
                "ticket.tag.list",
                "List classification tags",
                "Read Workspace classification tags.",
            ),
            (
                "ticket.tag.create",
                "Create classification tags",
                "Create Workspace classification tags, not workflow authority.",
            ),
            (
                "ticket.tag.rename",
                "Rename classification tags",
                "Rename Workspace tags with existing replay and compare checks.",
            ),
            (
                "ticket.tag.delete",
                "Delete classification tags",
                "Delete Workspace tags and their memberships with existing compare checks.",
            ),
            (
                "ticket.relation.query",
                "Search Ticket relations",
                "Query Workspace Ticket relation metadata.",
            ),
            (
                "ticket.orchestration_plan.query",
                "Read orchestration plans",
                "Query retained orchestration plan metadata; does not start work.",
            ),
            (
                "objective.query",
                "Search Objectives",
                "Query the Workspace Objective collection.",
            ),
            (
                "objective.create",
                "Create Objectives",
                "Create an Objective in this Workspace.",
            ),
            (
                "repository.list",
                "List repositories",
                "List repository metadata, not checkout or credential access.",
            ),
            (
                "workdir.list",
                "List Workdirs",
                "List Workdir metadata; does not grant attachment or file access.",
            ),
            (
                "workdir.attachment.list",
                "List attachments",
                "List Workdir attachment metadata.",
            ),
            (
                "worker.list",
                "List Workers",
                "List known Workers. Discovery does not grant control.",
            ),
            (
                "worker.discover",
                "Discover Workspace Workers",
                "Search bounded Worker metadata. Discovery does not grant control or observation.",
            ),
            (
                "worker.observe.list",
                "List observable Workers",
                "List observation subjects; does not grant access to individual sessions.",
            ),
            (
                "worker.create",
                "Create background Workers",
                "Use Profile-resolved Worker creation with explicit execution grants. Directional Backend grants separately control the created Worker and its message-only reply.",
            ),
            (
                "drive.capabilities",
                "Read Drive capabilities",
                "Read provider-supported Drive operations.",
            ),
            (
                "drive.root",
                "Find Drive root",
                "Read the Workspace Drive root reference.",
            ),
            (
                "drive.search",
                "Search Drive",
                "Search the Workspace Drive; provider visibility still applies.",
            ),
        ],
    );
    add(
        K::Ticket,
        &[
            (
                "ticket.read",
                "Read Ticket",
                "Read the selected Ticket and its thread.",
            ),
            (
                "ticket.edit_item",
                "Edit Ticket",
                "Edit the selected Ticket content; actor and concurrency checks still apply.",
            ),
            (
                "ticket.edit_post",
                "Edit Ticket post",
                "Edit an eligible post using its exact observed body.",
            ),
            (
                "ticket.event.add",
                "Add Ticket event",
                "Append an allowed typed event to the selected Ticket.",
            ),
            (
                "ticket.case.update",
                "Judge Ticket case",
                "Close, reopen or correct closure with explicit judgment and current compare keys.",
            ),
            (
                "ticket.dependency.check",
                "Check dependencies",
                "Inspect selected Ticket dependencies; does not start them.",
            ),
            (
                "ticket.intake_summary.add",
                "Add intake summary",
                "Append an intake summary to the selected Ticket.",
            ),
            (
                "ticket.tags.set",
                "Classify Ticket",
                "Replace selected Ticket classification tags using the observed full set.",
            ),
            (
                "ticket.relation.read",
                "Read Ticket relations",
                "Read selected Ticket relations.",
            ),
            (
                "ticket.relation.record",
                "Record Ticket relation",
                "Record an explicit relation from this Ticket.",
            ),
            (
                "ticket.relation.remove",
                "Remove Ticket relation",
                "Remove an exact relation from this Ticket.",
            ),
            (
                "ticket.orchestration_plan.record",
                "Record orchestration plan",
                "Record plan metadata for this Ticket; does not grant execution authority.",
            ),
            (
                "ticket.mark_ready",
                "Update intake readiness",
                "Record an intake readiness change; does not grant execution authority.",
            ),
            (
                "ticket.queue",
                "Request Ticket queueing",
                "Use the existing authorized Ticket queue route; does not bypass actor restrictions.",
            ),
            (
                "ticket.assignment.assign",
                "Delegate Ticket",
                "Assign to an authorized Subject; current body, Ticket targets and execution grants are checked separately. Assignment does not start a Worker.",
            ),
            (
                "ticket.assignment.clear",
                "Release Ticket delegation",
                "Release the observed current Subject assignment; does not stop Workers.",
            ),
            (
                "ticket.assignment.update",
                "Report assignment handling",
                "Report handling only for the authenticated authorized current delegation with event CAS.",
            ),
            (
                "ticket.assignment.answer",
                "Answer assignment",
                "Append fenced continuing input to the current assignment; not a new assignment.",
            ),
            (
                "ticket.assignment.query",
                "Read current assignment",
                "Read selected Ticket assignment tracking.",
            ),
            (
                "ticket.assignment.history",
                "Read assignment history",
                "Read retained assignment tracking events.",
            ),
            (
                "ticket.assignment.diagnostics",
                "Read assignment diagnostics",
                "Read retained delivery and start evidence, not execution authority.",
            ),
            (
                "ticket.implementation_result.write",
                "Report implementation result",
                "Save a bounded result, handoff or blocker; not approval or closure.",
            ),
            (
                "ticket.implementation_result.read",
                "Read implementation result",
                "Read a saved implementation result; not a new request.",
            ),
            (
                "merge_request.open",
                "Open Merge Request",
                "Open or reuse a repository-scoped Merge Request for this Ticket.",
            ),
        ],
    );
    add(
        K::Objective,
        &[
            (
                "objective.read",
                "Read Objective",
                "Read selected Objective and linked Ticket context.",
            ),
            (
                "objective.edit",
                "Edit Objective",
                "Edit selected Objective content.",
            ),
            (
                "objective.set_state",
                "Set Objective state",
                "Change selected Objective state.",
            ),
            (
                "objective.link_ticket",
                "Link Ticket",
                "Link a Ticket to this Objective; required Ticket read permission is separate.",
            ),
            (
                "objective.unlink_ticket",
                "Unlink Ticket",
                "Unlink a Ticket from this Objective.",
            ),
        ],
    );
    add(
        K::MergeRequest,
        &[
            (
                "merge_request.read",
                "Read Merge Request",
                "Read selected Merge Request and review evidence.",
            ),
            (
                "merge_request.readiness",
                "Check MR readiness",
                "Check recording-workflow readiness; does not adopt changes.",
            ),
            (
                "merge_request.review",
                "Review Merge Request",
                "Submit a review only with independently bound review capability and current source.",
            ),
            (
                "merge_request.complete",
                "Record MR integration",
                "Record an already-applied exact-source integration with existing approval and source checks.",
            ),
        ],
    );
    add(
        K::Repository,
        &[
            (
                "repository.read",
                "Read repository metadata",
                "Read selected repository metadata without exposing secrets.",
            ),
            (
                "workdir.create",
                "Create Workdir",
                "Materialize selected repository using existing provider grants and explicit selectors.",
            ),
        ],
    );
    add(
        K::WorkdirAttachment,
        &[
            (
                "workdir.attachment.read",
                "Read attachment",
                "Read metadata for this exact attachment connection.",
            ),
            (
                "workdir.detach",
                "Detach Workdir",
                "Detach this exact observed connection; stale aliases do not select a replacement.",
            ),
        ],
    );
    add(
        K::WorkspaceConfig,
        &[
            (
                "workspace_config.attach",
                "Attach configuration",
                "Attach the selected configuration path and recursive scope.",
            ),
            (
                "workspace_config.observe",
                "Observe configuration",
                "Observe selected configuration metadata within path scope.",
            ),
            (
                "workspace_config.read",
                "Read configuration",
                "Read selected configuration within path scope; secret rules still apply.",
            ),
            (
                "workspace_config.commit",
                "Commit configuration",
                "Commit selected configuration within path scope and provider compare checks.",
            ),
        ],
    );
    add(
        K::DriveEntry,
        &[
            (
                "drive.metadata",
                "Read Drive metadata",
                "Read metadata for this exact entry.",
            ),
            (
                "drive.list",
                "List Drive folder",
                "List children of this exact folder; does not include descendants implicitly.",
            ),
            (
                "drive.read_text",
                "Read Drive text",
                "Read text of this exact entry.",
            ),
            (
                "drive.read_chunk",
                "Read Drive chunk",
                "Read a bounded chunk of this exact entry.",
            ),
            (
                "drive.create_folder",
                "Create Drive folder",
                "Create a folder under this exact parent.",
            ),
            (
                "drive.create_text",
                "Create Drive text",
                "Create a text entry under this exact parent.",
            ),
            (
                "drive.update_text",
                "Update Drive text",
                "Update this exact entry with provider concurrency checks.",
            ),
            (
                "drive.delete",
                "Delete Drive entry",
                "Delete this exact entry if provider conditions permit.",
            ),
            (
                "drive.relocate",
                "Move Drive entry",
                "Move an entry; source and destination each require explicit permission.",
            ),
            (
                "drive.upload",
                "Upload Drive file",
                "Upload under this exact parent, subject to provider limits.",
            ),
        ],
    );
    add(
        K::DriveRequest,
        &[(
            "drive.request_status",
            "Read Drive request",
            "Read the status of this exact provider request; no new execution authority.",
        )],
    );
    add(
        K::Feature,
        &[(
            "mcp",
            "Enable MCP (temporary coarse permission)",
            "Retained MCP activation permission before stdio discovery. Not per-server, per-tool or per-call authorization; no equivalent gate is needed by other Features.",
        )],
    );
    add(
        K::Subject,
        &[
            (
                "session.record",
                "Record Subject session",
                "Attribute and record sessions for this Subject; current body ownership is still required.",
            ),
            (
                "session.list",
                "List Subject sessions",
                "List sessions attributed to this Subject.",
            ),
            (
                "session.search",
                "Search Subject sessions",
                "Search committed sessions attributed to this Subject.",
            ),
            (
                "session.read",
                "Read Subject session",
                "Read committed entries from this Subject's sessions.",
            ),
            (
                "memory.query",
                "Search Subject Memory",
                "Query this Subject's Memory, not Workspace Memory.",
            ),
            (
                "memory.read",
                "Read Subject Memory",
                "Read this Subject's selected Memory document.",
            ),
            (
                "memory.changes.list",
                "List Memory changes",
                "Read this Subject's retained Memory changes.",
            ),
            (
                "memory.resident.read",
                "Read resident Memory",
                "Read this Subject's resident Memory data.",
            ),
            (
                "memory.resident.supply",
                "Supply resident Memory",
                "Supply protected resident Memory to a model; read permission is separate.",
            ),
            (
                "memory.staging.write",
                "Stage Memory candidate",
                "Write a candidate to this Subject's staging area.",
            ),
            (
                "memory.staging.list",
                "List Memory candidates",
                "List this Subject's staging candidates.",
            ),
            (
                "memory.staging.read",
                "Read Memory candidate",
                "Read this Subject's staging candidate.",
            ),
            (
                "memory.candidate.decide",
                "Decide Memory candidate",
                "Decide a staged candidate using existing Job/domain constraints.",
            ),
            (
                "memory.proposal.validate",
                "Validate Memory proposal",
                "Validate a proposal for this Subject.",
            ),
            (
                "memory.receipt.read",
                "Read Memory receipt",
                "Read retained Memory operation evidence, not new authority.",
            ),
            (
                "memory.surface.prepare",
                "Prepare Memory surface",
                "Prepare this Subject's surface update under domain checks.",
            ),
            (
                "memory.surface.supply",
                "Supply Memory surface",
                "Supply protected surface input to a model.",
            ),
            (
                "memory.surface.publish",
                "Publish Memory surface",
                "Publish this Subject's prepared surface under writer and Job checks.",
            ),
            (
                "memory.surface.fail",
                "Record surface failure",
                "Record failure of this Subject's prepared surface.",
            ),
            (
                "memory.extract.read",
                "Read extraction input",
                "Read protected session input for Memory extraction.",
            ),
            (
                "memory.extract.supply",
                "Supply extraction input",
                "Supply protected extraction input to a model.",
            ),
            (
                "memory.extract.commit",
                "Commit extraction",
                "Commit extracted candidates under existing source/writer checks.",
            ),
            (
                "memory.consolidation.request",
                "Request consolidation",
                "Request a bounded consolidation Job; Job grants and origin constraints remain separate.",
            ),
            (
                "memory.consolidation.supply",
                "Supply consolidation input",
                "Supply frozen consolidation input only within its current Job capability.",
            ),
            (
                "journal.startup.read",
                "Read startup Journal",
                "Read this Subject's startup Journal context.",
            ),
            (
                "journal.startup.supply",
                "Supply startup Journal",
                "Supply protected startup Journal context to a model.",
            ),
            (
                "journal.compaction.read",
                "Read compaction Journal",
                "Read this Subject's compaction Journal context.",
            ),
            (
                "journal.compaction.supply",
                "Supply compaction Journal",
                "Supply protected compaction context to a model.",
            ),
            (
                "journal.recording.request",
                "Request Journal recording",
                "Request a bounded recording Job; does not grant arbitrary Job operations.",
            ),
            (
                "journal.job.input.read",
                "Read Journal Job input",
                "Read frozen input only with the bound current Journal Job capability.",
            ),
            (
                "journal.job.input.supply",
                "Supply Journal Job input",
                "Supply frozen input only with current Job, origin and policy authority.",
            ),
            (
                "journal.state.read",
                "Read Journal state",
                "Read this Subject's Journal state.",
            ),
            (
                "journal.text.read",
                "Read Journal text",
                "Read this Subject's Journal text.",
            ),
            (
                "journal.operation.query",
                "Query Journal operations",
                "Query retained Journal operations.",
            ),
            (
                "journal.operation.list",
                "List Journal operations",
                "List retained Journal operations.",
            ),
            (
                "journal.operation.coverage.read",
                "Read Journal coverage",
                "Read operation coverage evidence.",
            ),
            (
                "journal.entry.create",
                "Create Journal entry",
                "Create an entry within the bound Journal Job.",
            ),
            (
                "journal.entry.fix",
                "Fix Journal entry",
                "Fix an entry within the bound Journal Job.",
            ),
            (
                "journal.entry.rewrite",
                "Rewrite Journal entry",
                "Rewrite an entry within the bound Journal Job.",
            ),
            (
                "journal.position.update",
                "Update Journal position",
                "Update position within the current Journal writer/attempt.",
            ),
            (
                "journal.unchanged.confirm",
                "Confirm unchanged Journal",
                "Confirm no change within the current Journal writer/attempt.",
            ),
            (
                "journal.checkpoint",
                "Checkpoint Journal",
                "Checkpoint the current Journal recording Job.",
            ),
            (
                "journal.finish",
                "Finish Journal recording",
                "Finish the bound Journal Job; not a general work-completion judgment.",
            ),
        ],
    );
    add(
        K::Workspace,
        &[
            (
                "memory.document.read",
                "Read Workspace Memory document",
                "Read a legacy Workspace Memory document; not Subject Memory.",
            ),
            (
                "memory.document.update",
                "Update Workspace Memory document",
                "Update a legacy Workspace Memory document with existing storage constraints.",
            ),
            (
                "memory.audit.append",
                "Append Workspace Memory audit",
                "Append a Workspace Memory audit event.",
            ),
            (
                "memory.staging.close",
                "Close Workspace Memory candidate",
                "Close a legacy Workspace staging candidate.",
            ),
        ],
    );
    add(
        K::Subject,
        &[
            (
                "chat.read",
                "Read Subject conversations",
                "Read conversations belonging to the current Subject body; membership and current ownership remain required.",
            ),
            (
                "chat.send",
                "Send Subject message",
                "Explicitly publish to a permitted conversation as the current Subject body. Background Worker messaging does not grant publication.",
            ),
        ],
    );
    // The same exact operation may target either independent Memory domain. This
    // adds accepted kinds, never cross-kind containment or a second descriptor.
    for descriptor in &mut catalog {
        if matches!(
            descriptor.operation.as_str(),
            "memory.query"
                | "memory.resident.read"
                | "memory.resident.supply"
                | "memory.staging.write"
                | "memory.staging.list"
                | "memory.staging.read"
                | "memory.extract.read"
                | "memory.extract.supply"
                | "memory.extract.commit"
                | "memory.consolidation.request"
        ) {
            descriptor.resource_kinds.push(K::Workspace);
            descriptor.description.push_str(" The Workspace kind selects the separate legacy Workspace Memory domain, never all Subjects.");
        }
    }
    catalog.push(PermissionOperationDescriptor {
        operation: "notification.diagnostics.read".into(), label: "Read notification diagnostics".into(),
        description: "Read bounded saved delivery evidence for the exact source resource; Runtime acceptance does not prove processing.".into(),
        resource_kinds: vec![K::Workspace, K::Ticket, K::MergeRequest, K::Job],
        availability: PermissionOperationAvailability::Available,
    });
    // Operation use is Workspace-bound but never grants a target. Legacy Worker
    // scopes stay exact; management does not convert or discard their limits.
    for (operation, label) in [
        ("worker.messaging", "Message Workers"),
        ("worker.send_input", "Submit to Worker"),
        ("worker.cancel", "Cancel Worker turn"),
        ("worker.stop", "Stop Worker"),
        ("worker.restore", "Restore Worker"),
        ("worker.remove", "Remove Worker"),
        ("worker.observe", "Observe Worker session"),
    ] {
        catalog.push(PermissionOperationDescriptor {
            operation: operation.into(), label: label.into(),
            description: "Operation use permission and directional Backend target control grants are separate. Acceptance does not prove processing or success; lifecycle state conditions still apply.".into(),
            resource_kinds: vec![K::WorkerOperation],
            availability: PermissionOperationAvailability::Available,
        });
    }
    for (operation, label) in [
        ("subworker.spawn", "Create Internal SubWorker"),
        ("subworker.send_input", "Submit to Internal SubWorker"),
        ("subworker.stop", "Stop Internal SubWorker"),
        ("subworker.observe", "Observe Internal SubWorker"),
    ] {
        catalog.push(PermissionOperationDescriptor {
            operation: operation.into(), label: label.into(),
            description: "Parent-owned Internal SubWorkers are distinct from independently controlled Runtime Workers. Existing exact parent/name scopes are retained.".into(),
            resource_kinds: vec![],
            availability: PermissionOperationAvailability::Unavailable { reason: "A safe parent-bound management selector is not yet available. Existing scopes may be retained or removed, not widened by replacing parent identifiers.".into() },
        });
    }
    catalog
}

/// Validate additions only, after receipt lookup and current-policy CAS in the
/// management store transaction. Unsupported entries may be kept exactly in their
/// original lane or removed, never copied to another lane or broadened.
pub fn validate_managed_subject_permissions(
    current: Option<&SubjectPermissions>,
    update: &SubjectPermissionsUpdate,
) -> Result<(), String> {
    let catalog = subject_permission_operations();
    for (next, previous) in [
        (
            &update.execution,
            current.map(|p| p.execution.as_slice()).unwrap_or_default(),
        ),
        (
            &update.delegation,
            current.map(|p| p.delegation.as_slice()).unwrap_or_default(),
        ),
    ] {
        for permission in next {
            if previous.contains(permission) {
                continue;
            }
            permission.validate()?;
            let descriptor = catalog
                .iter()
                .find(|d| d.operation == permission.operation)
                .ok_or_else(|| format!("unknown management operation: {}", permission.operation))?;
            if !matches!(
                descriptor.availability,
                PermissionOperationAvailability::Available
            ) {
                return Err(format!(
                    "operation is unavailable for management: {}",
                    permission.operation
                ));
            }
            if !descriptor
                .resource_kinds
                .contains(&permission.resource.kind())
            {
                return Err(format!(
                    "invalid resource kind for operation: {}",
                    permission.operation
                ));
            }
            if permission.operation == "bash.execute"
                && !matches!(&permission.resource,
                PermissionResource::Workdir { path, recursive: true, .. } if path == ".")
            {
                return Err(
                    "bash.execute requires the whole Workdir (path '.' and recursive true)".into(),
                );
            }
        }
    }
    Ok(())
}
