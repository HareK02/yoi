use serde::Serialize;
use serde_json::{Map, Value};
use server_api::TicketSourceRefObservation;

#[derive(Debug, Serialize)]
pub(super) struct ModelTicketQueryResponse {
    tickets: Vec<ModelTicketQueryItem>,
    next_cursor: Option<String>,
    has_more: bool,
}

#[derive(Debug, Serialize)]
struct ModelTicketQueryItem {
    ticket: String,
    title: String,
    state: ticket::TicketState,
    closure_operation_key: Option<String>,
    current_closure: Option<Value>,
    legacy_handoff: Option<ticket::TicketLegacyWorkflowSnapshot>,
    assignment_tracking: Vec<ModelAssignmentTracking>,
    tags: Vec<ticket::WorkspaceTag>,
    readiness: Option<String>,
    priority: Option<String>,
    created_at: Option<String>,
    updated_at: Option<String>,
    workspace_action_priority: Option<String>,
    matched_fields: Vec<String>,
    snippet: Option<String>,
    current_worker: Option<ModelWorkerSummary>,
    linked_objectives: Vec<String>,
    relation_count: usize,
    blocker_count: usize,
    unresolved_blocker_count: usize,
    unresolved_review_count: usize,
    evidence: Option<ModelTicketEvidence>,
    merge_requests: Vec<ModelMergeRequest>,
}

#[derive(Debug, serde::Deserialize, Serialize)]
struct ModelTicketTarget {
    repository_key: String,
    // Keep an unspecified selector explicit in the model-facing read contract.
    ref_selector: Option<String>,
    access: ticket::TicketTargetAccess,
}

#[derive(Debug, Serialize)]
pub(super) struct ModelTicketDetail {
    ticket: String,
    content_digest: String,
    title: String,
    initial_post: ModelTicketEvent,
    targets: Vec<ModelTicketTarget>,
    created_by: Option<String>,
    state: ticket::TicketState,
    closure_operation_key: Option<String>,
    current_closure: Option<Value>,
    legacy_handoff: Option<ticket::TicketLegacyWorkflowSnapshot>,
    assignment_tracking: Vec<ModelAssignmentTracking>,
    tags: Vec<ticket::WorkspaceTag>,
    readiness: Option<String>,
    priority: Option<String>,
    created_at: Option<String>,
    updated_at: Option<String>,
    closure_history: Vec<Value>,
    thread: Vec<ModelTicketEvent>,
    relations: ModelTicketRelations,
    linked_objectives: Vec<ModelObjectiveSummary>,
    assignments: Vec<ModelAssignment>,
    current_worker: Option<ModelWorkerSummary>,
    implementation_reports: Vec<ModelEvidenceEvent>,
    merge_requests: Vec<ModelMergeRequest>,
    evidence: Option<ModelTicketEvidence>,
    actions: Option<ModelTicketActions>,
    event_page: Option<ModelEventPage>,
}

#[derive(Debug, Serialize)]
pub(super) struct ModelObjectiveQueryResponse {
    objectives: Vec<ModelObjectiveQueryItem>,
    next_cursor: Option<String>,
    has_more: bool,
}

#[derive(Debug, Serialize)]
struct ModelObjectiveQueryItem {
    objective: String,
    title: String,
    summary: Option<String>,
    state: String,
    created_at: Option<String>,
    updated_at: Option<String>,
    linked_tickets: Vec<String>,
    linked_ticket_count: usize,
}

#[derive(Debug, Serialize)]
pub(super) struct ModelObjectiveDetail {
    objective: String,
    title: String,
    body: String,
    body_truncated: bool,
    state: String,
    created_at: Option<String>,
    updated_at: Option<String>,
    linked_tickets: Vec<ModelTicketSummary>,
    events: Vec<ModelObjectiveEvent>,
    event_page: ModelObjectiveEventPage,
}

#[derive(Debug, Serialize)]
struct ModelAssignmentTracking {
    assignment_id: String,
    role: String,
    principal: ticket::TicketAssignmentPrincipalSnapshot,
    assigned_by: String,
    assigned_at: String,
    release: Option<ticket::TicketAssignmentRelease>,
    current: bool,
    tracking: Option<ticket::TicketAssignmentTracking>,
    last_event_id: Option<String>,
    updated_at: Option<String>,
    migration_diagnostic: Option<String>,
}

#[derive(Debug, Serialize)]
struct ModelWorkerSummary {
    worker: String,
}

#[derive(Debug, Serialize)]
struct ModelTicketTagChange {
    before: Vec<ticket::WorkspaceTag>,
    after: Vec<ticket::WorkspaceTag>,
}

#[derive(Debug, Serialize)]
struct ModelTicketEvent {
    #[serde(skip_serializing_if = "Option::is_none")]
    tag_changes: Option<ModelTicketTagChange>,
    sequence: usize,
    event_ref: String,
    is_initial: bool,
    kind: String,
    body: Option<String>,
    body_truncated: bool,
    body_digest: String,
    editable: bool,
    author: Option<String>,
    created_at: Option<String>,
    edited_at: Option<String>,
    edited_by: Option<String>,
}

#[derive(Debug, Serialize, Default)]
struct ModelTicketRelations {
    outgoing: Vec<ModelRelation>,
    incoming: Vec<ModelRelation>,
    blockers: Vec<ModelBlocker>,
    notices: Vec<ModelNotice>,
}

#[derive(Debug, Serialize)]
struct ModelRelation {
    ticket: String,
    kind: String,
    note: Option<String>,
    created_at: Option<String>,
}

#[derive(Debug, Serialize)]
struct ModelBlocker {
    ticket: String,
    kind: String,
    state: Option<String>,
}

#[derive(Debug, Serialize)]
struct ModelNotice {
    kind: String,
}

#[derive(Debug, Serialize)]
struct ModelObjectiveSummary {
    objective: String,
    title: String,
    state: String,
}

#[derive(Debug, Serialize)]
struct ModelTicketSummary {
    ticket: String,
    title: String,
    state: String,
}

#[derive(Debug, Serialize)]
struct ModelAssignment {
    role: String,
    principal: String,
    assigned_at: String,
}

#[derive(Debug, Serialize)]
struct ModelEvidenceEvent {
    sequence: usize,
    kind: String,
    created_at: Option<String>,
    excerpt: String,
}

#[derive(Debug, Serialize)]
struct ModelMergeRequest {
    merge_request_id: String,
    repository_key: String,
    state: String,
    selector_from: Option<String>,
    selector_to: String,
    review_status: String,
    subject_ref: Option<String>,
    source_ref_observation: TicketSourceRefObservation,
    integration_evidence_error: Option<String>,
    review_excerpt: Option<String>,
}

#[derive(Debug, Serialize)]
struct ModelTicketEvidence {
    /// MR evidence is inapplicable without a linked MR, independent of Ticket decisions.
    integration_applicable: bool,
    has_merge_request: bool,
    has_current_subject_ref: bool,
    has_review_request: bool,
    has_commit: bool,
    review_status: Option<String>,
    approved_current_subject: bool,
    review_after_rescope: bool,
    unresolved_request_changes: bool,
    complete_for_integration: bool,
    missing: Vec<String>,
}

#[derive(Debug, Serialize)]
struct ModelTicketActions {
    can_assign_orchestrator: bool,
    can_unassign_orchestrator: bool,
    can_queue: bool,
    can_start_manual_worker: bool,
}

#[derive(Debug, Serialize)]
struct ModelEventPage {
    next_cursor: Option<String>,
    has_more: bool,
}

#[derive(Debug, Serialize)]
struct ModelObjectiveEvent {
    kind: String,
    created_at: String,
    body: Option<String>,
}

#[derive(Debug, Serialize)]
struct ModelObjectiveEventPage {
    next_cursor: Option<String>,
    has_more: bool,
}

fn typed_field<T: serde::de::DeserializeOwned>(
    root: &Map<String, Value>,
    key: &str,
) -> Result<T, String> {
    let value = root
        .get(key)
        .ok_or_else(|| format!("Ticket projection requires {key}"))?;
    serde_json::from_value(value.clone()).map_err(|error| format!("Invalid Ticket {key}: {error}"))
}

fn closure_metadata<T: Serialize>(value: T) -> Result<Value, String> {
    let mut value = serde_json::to_value(value).map_err(|error| error.to_string())?;
    if let Some(object) = value.as_object_mut() {
        // The full explanation is available in thread history/post, not repeated
        // in current judgment and closure-index metadata as well.
        object.remove("explanation");
    }
    Ok(value)
}

fn project_current_closure(root: &Map<String, Value>) -> Result<Option<Value>, String> {
    let closure: Option<ticket::TicketClosureJudgment> = typed_field(root, "current_closure")?;
    closure.map(closure_metadata).transpose()
}

pub(super) fn project_ticket_query(value: Value) -> Result<ModelTicketQueryResponse, String> {
    let root = object(&value, "Ticket query response")?;
    let page = object_field(root, "page")?;
    let tickets = array_field(root, "items")?
        .iter()
        .map(project_ticket_query_item)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ModelTicketQueryResponse {
        tickets,
        next_cursor: optional_string(page, "next_cursor")?,
        has_more: bool_field(page, "has_more")?,
    })
}

fn project_ticket_query_item(value: &Value) -> Result<ModelTicketQueryItem, String> {
    let item = object(value, "Ticket query item")?;
    Ok(ModelTicketQueryItem {
        ticket: resource_ref(item, "resource_key", "T-")?,
        title: string_field(item, "title")?,
        state: typed_field(item, "state")?,
        closure_operation_key: typed_field(item, "closure_operation_key")?,
        current_closure: project_current_closure(item)?,
        legacy_handoff: typed_field(item, "legacy_handoff")?,
        assignment_tracking: project_assignment_tracking(item)?,
        tags: project_tags(item)?,
        readiness: optional_string(item, "readiness")?,
        priority: optional_string(item, "priority")?,
        created_at: optional_string(item, "created_at")?,
        updated_at: optional_string(item, "updated_at")?,
        workspace_action_priority: optional_string(item, "workspace_action_priority")?,
        matched_fields: string_array(item, "matched_fields")?,
        snippet: optional_string(item, "snippet")?,
        current_worker: item
            .get("current_worker")
            .filter(|value| !value.is_null())
            .map(project_worker)
            .transpose()?,
        linked_objectives: string_array(item, "linked_objective_keys")?
            .into_iter()
            .map(|key| validate_resource_ref(key, "O-"))
            .collect::<Result<Vec<_>, _>>()?,
        relation_count: usize_field(item, "relation_count")?,
        blocker_count: usize_field(item, "blocker_count")?,
        unresolved_blocker_count: usize_field(item, "unresolved_blocker_count")?,
        unresolved_review_count: usize_field(item, "unresolved_review_count")?,
        evidence: item.get("evidence").map(project_evidence).transpose()?,
        merge_requests: array_field(item, "merge_requests")?
            .iter()
            .map(project_merge_request)
            .collect::<Result<Vec<_>, _>>()?,
    })
}

/// Status is deliberately projected from authority fields, not from serialized detail:
/// prose/history size and history parsing have no effect on this response.
pub(super) fn project_ticket_status(value: Value) -> Result<Value, String> {
    use serde_json::json;
    let root = object(&value, "Ticket summary response")?;
    let state: ticket::TicketState = typed_field(root, "state")?;
    let closure_key: Option<String> = typed_field(root, "closure_operation_key")?;
    let closure: Option<ticket::TicketClosureJudgment> = typed_field(root, "current_closure")?;
    let targets: Vec<ModelTicketTarget> = typed_field(root, "targets")?;
    let tracking = project_assignment_tracking(root)?.into_iter().map(|row| json!({
        "assignment_id": row.assignment_id, "role": row.role, "principal": row.principal,
        "assigned_by": row.assigned_by, "assigned_at": row.assigned_at, "current": row.current,
        "status": row.tracking.map(|tracking| tracking.status),
        "last_event_id": row.last_event_id, "updated_at": row.updated_at,
    })).collect::<Vec<_>>();
    let merges = array_field(root, "merge_requests")?
        .iter()
        .map(|value| {
            let mut merge = serde_json::to_value(project_merge_request(value)?)
                .map_err(|error| error.to_string())?;
            merge
                .as_object_mut()
                .expect("typed merge object")
                .remove("review_excerpt");
            Ok(merge)
        })
        .collect::<Result<Vec<Value>, String>>()?;
    let latest_event_ref = array_field(root, "events")?
        .last()
        .map(|event| string_field(object(event, "Ticket event")?, "event_ref"))
        .transpose()?;
    Ok(json!({
        "ticket": resource_ref(root, "resource_key", "T-")?,
        "title": string_field(root, "title")?,
        "content_digest": string_field(root, "content_digest")?,
        "state": state, "closure_operation_key": closure_key,
        "current_closure": closure.map(|j| json!({"operation_key": j.operation_key, "reason": j.reason, "author": j.author, "at": j.at})),
        "targets": targets, "tags": project_tags(root)?,
        "readiness": optional_string(root, "readiness")?,
        "priority": optional_string(root, "priority")?,
        "updated_at": optional_string(root, "updated_at")?,
        "assignment_tracking": tracking,
        "current_worker": root.get("current_worker").filter(|value| !value.is_null()).map(project_worker).transpose()?,
        "merge_requests": merges,
        "evidence": root.get("evidence").map(project_evidence).transpose()?,
        "actions": root.get("action_eligibility").filter(|value| !value.is_null()).map(project_actions).transpose()?,
        "latest_event_ref": latest_event_ref,
    }))
}

pub(super) fn project_ticket_history(value: Value) -> Result<Value, String> {
    let root = object(&value, "Ticket history response")?;
    let thread = array_field(root, "events")?
        .iter()
        .filter(|event| event.get("is_initial") != Some(&Value::Bool(true)))
        .map(project_ticket_event)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(serde_json::json!({
        "ticket": resource_ref(root, "resource_key", "T-")?,
        "thread": thread,
        "event_page": project_event_page(root.get("event_page").ok_or("missing event_page")?)?,
    }))
}

pub(super) fn project_ticket_post(value: Value) -> Result<Value, String> {
    let post = project_ticket_event(&value)?;
    if post.body_truncated || post.body.is_none() {
        return Err(
            "Full-post read returned an absent or truncated body; do not use it for edit CAS"
                .into(),
        );
    }
    serde_json::to_value(post).map_err(|error| error.to_string())
}

pub(super) fn project_ticket_detail(value: Value) -> Result<ModelTicketDetail, String> {
    let root = object(&value, "Ticket detail response")?;
    let current_worker = root
        .get("current_worker")
        .filter(|value| !value.is_null())
        .map(project_worker)
        .transpose()?;
    let assignments = array_field(root, "assignments")?
        .iter()
        .map(|assignment| project_assignment(assignment, current_worker.as_ref()))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(ModelTicketDetail {
        ticket: resource_ref(root, "resource_key", "T-")?,
        content_digest: string_field(root, "content_digest")?,
        title: string_field(root, "title")?,
        initial_post: project_ticket_event(
            root.get("initial_post")
                .ok_or("Ticket detail response is missing initial_post")?,
        )?,
        targets: typed_field(root, "targets")?,
        created_by: optional_string(root, "created_by")?,
        state: typed_field(root, "state")?,
        closure_operation_key: typed_field(root, "closure_operation_key")?,
        current_closure: project_current_closure(root)?,
        legacy_handoff: typed_field(root, "legacy_handoff")?,
        closure_history: typed_field::<Vec<ticket::TicketCaseHistoryEntry>>(
            root,
            "closure_history",
        )?
        .into_iter()
        .map(closure_metadata)
        .collect::<Result<Vec<_>, _>>()?,
        assignment_tracking: project_assignment_tracking(root)?,
        tags: project_tags(root)?,
        readiness: optional_string(root, "readiness")?,
        priority: optional_string(root, "priority")?,
        created_at: optional_string(root, "created_at")?,
        updated_at: optional_string(root, "updated_at")?,
        thread: array_field(root, "events")?
            .iter()
            .filter(|event| event.get("is_initial") != Some(&Value::Bool(true)))
            .map(project_ticket_event)
            .collect::<Result<Vec<_>, _>>()?,
        relations: project_relations(root.get("relations"))?,
        linked_objectives: array_field(root, "linked_objectives")?
            .iter()
            .map(project_objective_summary)
            .collect::<Result<Vec<_>, _>>()?,
        assignments,
        current_worker,
        implementation_reports: array_field(root, "implementation_reports")?
            .iter()
            .map(project_evidence_event)
            .collect::<Result<Vec<_>, _>>()?,
        merge_requests: array_field(root, "merge_requests")?
            .iter()
            .map(project_merge_request)
            .collect::<Result<Vec<_>, _>>()?,
        evidence: root.get("evidence").map(project_evidence).transpose()?,
        actions: root
            .get("action_eligibility")
            .filter(|value| !value.is_null())
            .map(project_actions)
            .transpose()?,
        event_page: root
            .get("event_page")
            .filter(|value| !value.is_null())
            .map(project_event_page)
            .transpose()?,
    })
}

pub(super) fn project_objective_query(value: Value) -> Result<ModelObjectiveQueryResponse, String> {
    let root = object(&value, "Objective query response")?;
    let page = object_field(root, "page")?;
    Ok(ModelObjectiveQueryResponse {
        objectives: array_field(root, "items")?
            .iter()
            .map(project_objective_query_item)
            .collect::<Result<Vec<_>, _>>()?,
        next_cursor: optional_string(page, "next_cursor")?,
        has_more: bool_field(page, "has_more")?,
    })
}

fn project_objective_query_item(value: &Value) -> Result<ModelObjectiveQueryItem, String> {
    let item = object(value, "Objective query item")?;
    let linked_tickets = string_array(item, "linked_ticket_keys")?
        .into_iter()
        .map(|key| validate_resource_ref(key, "T-"))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ModelObjectiveQueryItem {
        objective: resource_ref(item, "resource_key", "O-")?,
        title: string_field(item, "title")?,
        summary: optional_string(item, "snippet")?,
        state: string_field(item, "state")?,
        created_at: optional_string(item, "created_at")?,
        updated_at: optional_string(item, "updated_at")?,
        linked_ticket_count: linked_tickets.len(),
        linked_tickets,
    })
}

pub(super) fn project_objective_detail(value: Value) -> Result<ModelObjectiveDetail, String> {
    let root = object(&value, "Objective detail response")?;
    Ok(ModelObjectiveDetail {
        objective: resource_ref(root, "resource_key", "O-")?,
        title: string_field(root, "title")?,
        body: string_field(root, "body")?,
        body_truncated: bool_field(root, "body_truncated")?,
        state: string_field(root, "state")?,
        created_at: optional_string(root, "created_at")?,
        updated_at: optional_string(root, "updated_at")?,
        linked_tickets: array_field(root, "linked_ticket_summaries")?
            .iter()
            .map(project_ticket_summary)
            .collect::<Result<Vec<_>, _>>()?,
        events: array_field(root, "events")?
            .iter()
            .map(project_objective_event)
            .collect::<Result<Vec<_>, _>>()?,
        event_page: project_objective_event_page(
            root.get("event_page")
                .ok_or_else(|| "Objective detail response is missing event_page".to_string())?,
        )?,
    })
}

fn project_worker(value: &Value) -> Result<ModelWorkerSummary, String> {
    let worker = object(value, "Worker summary")?;
    Ok(ModelWorkerSummary {
        worker: resource_ref(worker, "worker_resource_key", "W-")?,
    })
}

fn project_ticket_event(value: &Value) -> Result<ModelTicketEvent, String> {
    let event = object(value, "Ticket event")?;
    let tag_changes = if event.get("kind").and_then(Value::as_str) == Some("tags_changed") {
        let attributes = event
            .get("attributes")
            .ok_or("missing tag audit attributes")?;
        let snapshot = |field| -> Result<Vec<ticket::WorkspaceTag>, String> {
            let encoded = attributes
                .get(field)
                .and_then(Value::as_str)
                .ok_or("missing tag audit snapshot")?;
            serde_json::from_str(encoded)
                .map_err(|error| format!("invalid tag audit snapshot: {error}"))
        };
        Some(ModelTicketTagChange {
            before: snapshot("tags_before")?,
            after: snapshot("tags_after")?,
        })
    } else {
        None
    };
    Ok(ModelTicketEvent {
        tag_changes,
        sequence: usize_field(event, "sequence")?,
        event_ref: string_field(event, "event_ref")?,
        is_initial: bool_field(event, "is_initial")?,
        body_truncated: bool_field(event, "body_truncated")?,
        body_digest: string_field(event, "body_digest")?,
        editable: bool_field(event, "editable")?,
        author: optional_string(event, "author")?,
        edited_at: optional_string(event, "edited_at")?,
        edited_by: optional_string(event, "edited_by")?,
        kind: string_field(event, "kind")?,
        body: match event.get("body") {
            None | Some(Value::Null) => None,
            Some(Value::String(body)) => Some(body.clone()),
            Some(_) => return Err("invalid Ticket event body".to_string()),
        },
        created_at: optional_string(event, "at")?,
    })
}

fn project_relations(value: Option<&Value>) -> Result<ModelTicketRelations, String> {
    let Some(value) = value else {
        return Ok(ModelTicketRelations::default());
    };
    let relations = object(value, "Ticket relations")?;
    Ok(ModelTicketRelations {
        outgoing: array_field(relations, "outgoing")?
            .iter()
            .map(|value| project_relation(value, "target_resource_key", "kind"))
            .collect::<Result<Vec<_>, _>>()?,
        incoming: array_field(relations, "incoming")?
            .iter()
            .map(|value| project_relation(value, "source_resource_key", "forward_kind"))
            .collect::<Result<Vec<_>, _>>()?,
        blockers: array_field(relations, "blockers")?
            .iter()
            .map(project_blocker)
            .collect::<Result<Vec<_>, _>>()?,
        notices: array_field(relations, "notices")?
            .iter()
            .map(project_notice)
            .collect::<Result<Vec<_>, _>>()?,
    })
}

fn project_relation(
    value: &Value,
    ticket_key: &str,
    kind_key: &str,
) -> Result<ModelRelation, String> {
    let relation = object(value, "Ticket relation")?;
    Ok(ModelRelation {
        ticket: resource_ref(relation, ticket_key, "T-")?,
        kind: string_field(relation, kind_key)?,
        note: optional_string(relation, "note")?,
        created_at: optional_string(relation, "at")?,
    })
}

fn project_blocker(value: &Value) -> Result<ModelBlocker, String> {
    let blocker = object(value, "Ticket blocker")?;
    Ok(ModelBlocker {
        ticket: resource_ref(blocker, "blocking_resource_key", "T-")?,
        kind: string_field(blocker, "relation_kind")?,
        state: optional_string(blocker, "blocking_state")?,
    })
}

fn project_notice(value: &Value) -> Result<ModelNotice, String> {
    let notice = object(value, "Ticket notice")?;
    Ok(ModelNotice {
        kind: string_field(notice, "kind")?,
    })
}

fn project_objective_summary(value: &Value) -> Result<ModelObjectiveSummary, String> {
    let summary = object(value, "Objective summary")?;
    Ok(ModelObjectiveSummary {
        objective: resource_ref(summary, "resource_key", "O-")?,
        title: string_field(summary, "title")?,
        state: string_field(summary, "state")?,
    })
}

fn project_ticket_summary(value: &Value) -> Result<ModelTicketSummary, String> {
    let summary = object(value, "Ticket summary")?;
    Ok(ModelTicketSummary {
        ticket: resource_ref(summary, "resource_key", "T-")?,
        title: string_field(summary, "title")?,
        state: string_field(summary, "state")?,
    })
}

fn project_assignment(
    value: &Value,
    current_worker: Option<&ModelWorkerSummary>,
) -> Result<ModelAssignment, String> {
    let assignment = object(value, "Ticket assignment")?;
    let principal = object_field(assignment, "principal")?;
    let kind = string_field(principal, "kind")?;
    let principal = match kind.as_str() {
        "worker" => current_worker
            .map(|coder| coder.worker.clone())
            .ok_or_else(|| "Worker assignment is missing a Workspace key projection".to_string())?,
        "workspace_agent" => format!("workspace-agent:{}", string_field(principal, "agent_key")?),
        "user" => "user".to_string(),
        other => format!("source:{other}"),
    };
    Ok(ModelAssignment {
        role: string_field(assignment, "role")?,
        principal,
        assigned_at: string_field(assignment, "assigned_at")?,
    })
}

fn project_evidence_event(value: &Value) -> Result<ModelEvidenceEvent, String> {
    let event = object(value, "Ticket evidence event")?;
    Ok(ModelEvidenceEvent {
        sequence: usize_field(event, "sequence")?,
        kind: string_field(event, "kind")?,
        created_at: optional_string(event, "at")?,
        excerpt: string_field(event, "excerpt")?,
    })
}

fn project_merge_request(value: &Value) -> Result<ModelMergeRequest, String> {
    let merge = object(value, "Merge Request summary")?;
    Ok(ModelMergeRequest {
        merge_request_id: string_field(merge, "merge_request_id")?,
        repository_key: string_field(merge, "repository_key")?,
        state: string_field(merge, "state")?,
        selector_from: optional_string(merge, "selector_from")?,
        selector_to: string_field(merge, "selector_to")?,
        review_status: string_field(merge, "review_status")?,
        subject_ref: optional_string(merge, "current_subject_ref")?,
        source_ref_observation: serde_json::from_value(
            merge
                .get("source_ref_observation")
                .ok_or_else(|| "missing source_ref_observation".to_string())?
                .clone(),
        )
        .map_err(|error| format!("invalid source_ref_observation: {error}"))?,
        integration_evidence_error: optional_string(merge, "integration_evidence_error")?,
        review_excerpt: optional_string(merge, "review_excerpt")?,
    })
}

fn project_evidence(value: &Value) -> Result<ModelTicketEvidence, String> {
    let evidence = object(value, "Ticket evidence")?;
    let has_merge_request = bool_field(evidence, "has_merge_request")?;
    let projected = ModelTicketEvidence {
        integration_applicable: has_merge_request,
        has_merge_request,
        has_current_subject_ref: bool_field(evidence, "has_current_subject_ref")?,
        has_review_request: bool_field(evidence, "has_review_request")?,
        has_commit: bool_field(evidence, "has_commit")?,
        review_status: optional_string(evidence, "review_status")?,
        approved_current_subject: bool_field(evidence, "approved_current_subject")?,
        review_after_rescope: bool_field(evidence, "review_after_rescope")?,
        unresolved_request_changes: bool_field(evidence, "unresolved_request_changes")?,
        complete_for_integration: bool_field(evidence, "complete_for_integration")?,
        missing: string_array(evidence, "missing")?,
    };
    if !has_merge_request {
        // Empty MR sets are neither missing evidence nor vacuous approval.
        return Ok(ModelTicketEvidence {
            review_status: None,
            approved_current_subject: false,
            review_after_rescope: false,
            unresolved_request_changes: false,
            complete_for_integration: false,
            missing: Vec::new(),
            ..projected
        });
    }
    Ok(projected)
}

fn project_actions(value: &Value) -> Result<ModelTicketActions, String> {
    let actions = object(value, "Ticket actions")?;
    Ok(ModelTicketActions {
        can_assign_orchestrator: bool_field(actions, "can_assign_orchestrator")?,
        can_unassign_orchestrator: bool_field(actions, "can_unassign_orchestrator")?,
        can_queue: bool_field(actions, "can_queue")?,
        can_start_manual_worker: bool_field(actions, "can_start_manual_worker")?,
    })
}

fn project_event_page(value: &Value) -> Result<ModelEventPage, String> {
    let page = object(value, "Ticket event page")?;
    Ok(ModelEventPage {
        next_cursor: optional_string(page, "next_cursor")?,
        has_more: bool_field(page, "has_more")?,
    })
}

fn project_objective_event(value: &Value) -> Result<ModelObjectiveEvent, String> {
    let event = object(value, "Objective event")?;
    let body = optional_string(event, "body")?;
    Ok(ModelObjectiveEvent {
        kind: string_field(event, "kind")?,
        created_at: string_field(event, "created_at")?,
        body,
    })
}

fn project_objective_event_page(value: &Value) -> Result<ModelObjectiveEventPage, String> {
    let page = object(value, "Objective event page")?;
    Ok(ModelObjectiveEventPage {
        next_cursor: optional_string(page, "next_cursor")?,
        has_more: bool_field(page, "has_more")?,
    })
}

fn object<'a>(value: &'a Value, context: &str) -> Result<&'a Map<String, Value>, String> {
    value
        .as_object()
        .ok_or_else(|| format!("{context} must be an object"))
}

fn object_field<'a>(
    object: &'a Map<String, Value>,
    key: &str,
) -> Result<&'a Map<String, Value>, String> {
    object
        .get(key)
        .and_then(Value::as_object)
        .ok_or_else(|| format!("missing or invalid {key}"))
}

fn project_assignment_tracking(
    object: &Map<String, Value>,
) -> Result<Vec<ModelAssignmentTracking>, String> {
    let values = array_field(object, "assignment_tracking")?;
    if values.len() > 1 {
        return Err("Ticket has more than one current Orchestrator delegation".into());
    }
    values
        .iter()
        .map(|value| {
            let projection: ticket::TicketAssignmentTrackingProjection =
                serde_json::from_value(value.clone())
                    .map_err(|error| format!("invalid assignment tracking: {error}"))?;
            Ok(ModelAssignmentTracking {
                assignment_id: projection.assignment_id,
                role: projection.role,
                principal: projection.principal,
                assigned_by: projection.assigned_by,
                assigned_at: projection.assigned_at,
                release: projection.release,
                current: projection.current,
                tracking: projection.tracking,
                last_event_id: projection.last_event_id,
                updated_at: projection.updated_at,
                migration_diagnostic: projection.migration_diagnostic,
            })
        })
        .collect()
}

fn project_tags(object: &Map<String, Value>) -> Result<Vec<ticket::WorkspaceTag>, String> {
    let values = array_field(object, "tags")?;
    if values.len() > 100 {
        return Err("Ticket projection exceeds 100 tags".into());
    }
    values
        .iter()
        .map(|value| {
            let tag: ticket::WorkspaceTag = serde_json::from_value(value.clone())
                .map_err(|error| format!("invalid Workspace tag: {error}"))?;
            if tag.deleted {
                return Err("current Ticket projection contains a deleted tag".into());
            }
            Ok(tag)
        })
        .collect()
}

fn array_field<'a>(object: &'a Map<String, Value>, key: &str) -> Result<&'a [Value], String> {
    object
        .get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .ok_or_else(|| format!("missing or invalid {key}"))
}

fn string_field(object: &Map<String, Value>, key: &str) -> Result<String, String> {
    object
        .get(key)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| format!("missing or invalid {key}"))
}

fn optional_string(object: &Map<String, Value>, key: &str) -> Result<Option<String>, String> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(format!("invalid {key}")),
    }
}

fn bool_field(object: &Map<String, Value>, key: &str) -> Result<bool, String> {
    object
        .get(key)
        .and_then(Value::as_bool)
        .ok_or_else(|| format!("missing or invalid {key}"))
}

fn usize_field(object: &Map<String, Value>, key: &str) -> Result<usize, String> {
    object
        .get(key)
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| format!("missing or invalid {key}"))
}

fn string_array(object: &Map<String, Value>, key: &str) -> Result<Vec<String>, String> {
    array_field(object, key)?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(ToOwned::to_owned)
                .ok_or_else(|| format!("invalid {key}"))
        })
        .collect()
}

fn resource_ref(object: &Map<String, Value>, key: &str, prefix: &str) -> Result<String, String> {
    let value = object
        .get(key)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| format!("required {prefix} key is unavailable"))?;
    validate_resource_ref(value, prefix)
}

fn validate_resource_ref(value: String, prefix: &str) -> Result<String, String> {
    let valid = value.strip_prefix(prefix).is_some_and(|sequence| {
        !sequence.is_empty() && sequence.bytes().all(|byte| byte.is_ascii_digit())
    });
    if valid {
        Ok(value)
    } else {
        Err(format!("required {prefix} key is unavailable"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn post_fixture(initial: bool, body: &str) -> Value {
        json!({
            "sequence": if initial { 0 } else { 1 },
            "event_ref": if initial { "post-initial" } else { "post-discussion" },
            "is_initial": initial, "kind": "comment", "body": body,
            "body_truncated": false, "body_digest": "a".repeat(64), "editable": true,
            "author": "workspace-user:original", "at": "2026-01-01T00:00:00Z",
            "edited_at": "2026-01-02T00:00:00Z", "edited_by": "workspace-user:editor"
        })
    }

    #[test]
    fn mrless_evidence_is_inapplicable_not_missing_or_vacuously_approved() {
        for state in ["open", "closed"] {
            let projected = project_ticket_detail(json!({
                "resource_key": "T-718", "assignment_tracking": [], "tags": [], "content_digest": "rev-1", "title": "No Git required", "body": "Decision", "body_truncated": false, "targets": [], "initial_post": post_fixture(true, "Decision"),
                "closure_operation_key": null, "current_closure": null, "closure_history": [], "legacy_handoff": null,
                "state": state, "events": [post_fixture(false, "No Git needed")],
                "linked_objectives": [], "assignments": [], "implementation_reports": [],
                "merge_requests": [], "merge_request": null,
                "evidence": {
                    "has_merge_request": false, "has_current_subject_ref": false, "has_review_request": false, "has_commit": false,
                    "review_status": "approved", "approved_current_subject": true, "review_after_rescope": true,
                    "unresolved_request_changes": false, "complete_for_integration": true, "missing": ["merge_request"]
                }
            })).unwrap();
            let projected = serde_json::to_value(projected).unwrap();
            assert_eq!(projected["state"], state);
            assert_eq!(projected["thread"][0]["kind"], "comment");
            assert_eq!(projected["evidence"]["integration_applicable"], false);
            assert_eq!(projected["evidence"]["review_status"], Value::Null);
            assert_eq!(projected["evidence"]["approved_current_subject"], false);
            assert_eq!(projected["evidence"]["complete_for_integration"], false);
            assert_eq!(projected["evidence"]["missing"], json!([]));
        }
    }

    #[test]
    fn reopened_ticket_projection_preserves_closure_observation_history_and_legacy_handoff() {
        let history = json!([
            {
                "operation_key": "close-1", "previous_closure_operation_key": null,
                "action": {"action": "close", "reason": "not_planned"},
                "explanation": "Not proceeding", "references": [{"kind": "ticket", "target": "T-12"}],
                "author": "workspace-user:alice", "at": "2026-01-01T00:00:00Z"
            },
            {
                "operation_key": "reopen-1", "previous_closure_operation_key": "close-1",
                "action": {"action": "reopen"}, "explanation": "New information",
                "references": [], "author": "workspace-user:bob", "at": "2026-01-02T00:00:00Z"
            }
        ]);
        let legacy = json!({
            "workflow_state": "queued", "workflow_state_explicit": true, "status": "open",
            "queued_by": "workspace-user:alice", "queued_at": "2025-12-31T00:00:00Z", "resolution": null
        });
        let response = json!({
            "assignment_tracking": [], "tags": [{"workspace_id":"ws", "id":"tag-1", "name":"closed", "last_operation_key":"tag-create", "deleted":false}],
            "resource_key": "T-739", "content_digest": "content-digest", "title": "Case", "body": "Requirements", "body_truncated": false, "targets": [], "initial_post": post_fixture(true, "Requirements"),
            "state": "open", "closure_operation_key": "close-1", "current_closure": null,
            "closure_history": history, "legacy_handoff": legacy,
            "events": [], "linked_objectives": [], "assignments": [], "implementation_reports": [],
            "merge_requests": [], "merge_request": null
        });
        let projected =
            serde_json::to_value(project_ticket_detail(response.clone()).unwrap()).unwrap();
        assert_eq!(projected["state"], "open");
        assert_eq!(projected["closure_operation_key"], "close-1");
        assert_eq!(projected["current_closure"], Value::Null);
        assert_eq!(
            projected["closure_history"],
            json!(
                history
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(closure_metadata)
                    .collect::<Result<Vec<_>, _>>()
                    .unwrap()
            )
        );
        assert_eq!(projected["legacy_handoff"], legacy);
        assert_eq!(projected["tags"], response["tags"]);
        assert_eq!(projected["content_digest"], "content-digest");

        // Missing observations must not silently become null: the model must be
        // able to distinguish an observed never-closed case from unknown data.
        let mut missing = response;
        missing
            .as_object_mut()
            .unwrap()
            .remove("closure_operation_key");
        assert!(
            project_ticket_detail(missing)
                .unwrap_err()
                .contains("closure_operation_key")
        );
    }

    #[test]
    fn objective_projection_exposes_only_resource_references() {
        let projected = project_objective_detail(json!({
            "id": "00001M10HW6BV",
            "resource_key": "O-543",
            "title": "Objective",
            "body": "Body",
            "body_truncated": false,
            "state": "active",
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-02T00:00:00Z",
            "linked_tickets": ["00001M0E82D1V"],
            "linked_ticket_summaries": [{
                "id": "00001M0E82D1V",
                "resource_key": "T-496", "assignment_tracking": [], "tags": [],
                "title": "Ticket",
                "state": "closed",
                "updated_at": "2026-01-02T00:00:00Z"
            }],
            "events": [{
                "sequence": 3,
                "event_ref": "objective-event-3",
                "kind": "linked_ticket",
                "created_at": "2026-01-02T00:00:00Z",
                "body": "linked"
            }],
            "event_page": {"next_cursor": null, "has_more": false, "window_start_sequence": 3, "window_end_sequence": 3}
        })).expect("projection");
        let json = serde_json::to_value(projected).expect("serialize");
        let text = json.to_string();
        assert!(text.contains("O-543"));
        assert!(text.contains("T-496"));
        assert_eq!(json["body_truncated"], false);
        assert!(!text.contains("00001M10HW6BV"));
        assert!(!text.contains("00001M0E82D1V"));
        assert!(!text.contains("event_ref"));
    }

    #[test]
    fn query_projections_accept_workspace_api_shapes_and_scrub_internal_ids() {
        let ticket = project_ticket_query(json!({
            "page": {"next_cursor": null, "has_more": false},
            "record_authority": "workspace_sqlite",
            "items": [{
                "id": "00001TICKETINTERNAL",
                "resource_key": "T-543", "assignment_tracking": [], "tags": [],
                "title": "Ticket",
                "state": "open",
                "closure_operation_key": null, "current_closure": null, "legacy_handoff": null,
                "readiness": null,
                "priority": "high",
                "created_at": null,
                "updated_at": "2026-01-01T00:00:00Z",
                "workspace_action_priority": "active_work",
                "matched_fields": ["title"],
                "snippet": "Ticket",
                "current_worker": {"runtime_id": "runtime-internal", "worker_id": "worker-internal", "worker_resource_key": "W-12"},
                "linked_objective_ids": ["00001OBJECTIVEINTERNAL"],
                "linked_objective_keys": ["O-6"],
                "relation_count": 0,
                "blocker_count": 0,
                "unresolved_blocker_count": 0,
                "unresolved_review_count": 0,
                "evidence": {
                    "has_merge_request": false,
                    "has_current_subject_ref": false,
                    "has_review_request": false,
                    "has_commit": false,
                    "review_status": null,
                    "approved_current_subject": false,
                    "review_after_rescope": false,
                    "unresolved_request_changes": false,
                    "complete_for_integration": false,
                    "missing": ["merge_request"]
                },
                "merge_requests": [],
                "merge_request": null
            }]
        })).expect("Ticket query projection");
        let ticket_json = serde_json::to_string(&ticket).expect("serialize Ticket query");
        assert!(ticket_json.contains("T-543"));
        assert!(ticket_json.contains("O-6"));
        assert!(ticket_json.contains("W-12"));
        assert!(!ticket_json.contains("00001TICKETINTERNAL"));
        assert!(!ticket_json.contains("runtime-internal"));
        assert!(!ticket_json.contains("worker-internal"));

        let objective = project_objective_query(json!({
            "page": {"next_cursor": null, "has_more": false},
            "record_authority": "workspace_sqlite",
            "items": [{
                "id": "00001OBJECTIVEINTERNAL",
                "resource_key": "O-6",
                "title": "Objective",
                "state": "active",
                "created_at": null,
                "updated_at": null,
                "matched_fields": [],
                "snippet": null,
                "linked_ticket_count": 1,
                "linked_tickets": ["00001TICKETINTERNAL"],
                "linked_ticket_keys": ["T-543"]
            }]
        }))
        .expect("Objective query projection");
        let objective_json = serde_json::to_string(&objective).expect("serialize Objective query");
        assert!(objective_json.contains("O-6"));
        assert!(objective_json.contains("T-543"));
        assert!(objective_json.contains("\"summary\":null"));
        assert!(!objective_json.contains("00001OBJECTIVEINTERNAL"));
        assert!(!objective_json.contains("00001TICKETINTERNAL"));
    }

    #[test]
    fn ticket_detail_projection_accepts_current_blocker_shape() {
        let projected = project_ticket_detail(json!({
            "id": "internal-ticket",
            "resource_key": "T-588", "assignment_tracking": [], "tags": [],
            "content_digest": "ticket-content-digest",
                "closure_operation_key": null, "current_closure": null, "closure_history": [], "legacy_handoff": null,
            "title": "Queued Submit",
            "body": "Body", "body_truncated": false, "targets": [], "initial_post": post_fixture(true, "Body"),
            "state": "open",
            "readiness": null,
            "priority": "P2",
            "created_at": "2026-09-03T00:00:00Z",
            "updated_at": "2026-09-03T00:00:00Z",
            "events": [],
            "relations": {
                "outgoing": [],
                "incoming": [],
                "blockers": [{
                    "blocking_ticket": "internal-blocker",
                    "blocking_resource_key": "T-584",
                    "reason_kind": "depends_on",
                    "relation_kind": "depends_on",
                    "note": "required foundation",
                    "blocking_state": "open"
                }],
                "notices": []
            },
            "linked_objectives": [],
            "implementation_reports": [],
            "assignments": [],
            "current_worker": null,
            "merge_requests": [],
            "merge_request": null,
            "evidence": {
                "has_merge_request": false,
                "has_current_subject_ref": false,
                "has_review_request": false,
                "has_commit": false,
                "review_status": null,
                "approved_current_subject": false,
                "review_after_rescope": false,
                "unresolved_request_changes": false,
                "complete_for_integration": false,
                "missing": ["merge_request"]
            },
            "action_eligibility": {
                "can_assign_orchestrator": true,
                "can_unassign_orchestrator": false,
                "can_queue": false,
                "can_start_manual_worker": false
            },
            "event_page": {"next_cursor": null, "has_more": false}
        }))
        .expect("current Ticket blocker shape must project");

        let projected = serde_json::to_value(projected).expect("serialize Ticket detail");
        assert_eq!(projected["content_digest"], "ticket-content-digest");
        assert_eq!(
            projected["relations"]["blockers"],
            json!([{
                "ticket": "T-584",
                "kind": "depends_on",
                "state": "open"
            }])
        );
        assert!(!projected.to_string().contains("internal-blocker"));
    }

    #[test]
    fn ticket_projection_preserves_source_diagnostics_separately_from_rescope_attestation() {
        for (state, subject_ref, observation, integration_error, attested) in [
            (
                "merged",
                Some("immutable-approved-source"),
                json!({"status": "not_required"}),
                None,
                true,
            ),
            (
                "open",
                None,
                json!({"status": "unavailable", "code": "source_ref_unavailable"}),
                Some("source_ref_unavailable"),
                true,
            ),
            (
                "open",
                Some("observed-source"),
                json!({"status": "observed"}),
                None,
                false,
            ),
        ] {
            let merged = state == "merged";
            let merge_request = json!({
                "merge_request_id": "mr-source",
                "repository_key": "repository",
                "state": state,
                "selector_from": "source-branch",
                "selector_to": "main",
                "review_status": "approved",
                "current_subject_ref": subject_ref,
                "source_ref_observation": observation,
                "integration_evidence_error": integration_error,
                "review_excerpt": "Reviewed source"
            });
            let projected = project_ticket_detail(json!({
                "resource_key": "T-716", "assignment_tracking": [], "tags": [],
                "content_digest": "current-ticket-content-digest",
                "closure_operation_key": null, "current_closure": null, "closure_history": [], "legacy_handoff": null,
                "title": "Source evidence",
                "body": "Body", "body_truncated": false, "targets": [], "initial_post": post_fixture(true, "Body"),
                "state": "open",
                "events": [],
                "linked_objectives": [],
                "assignments": [],
                "implementation_reports": [],
                "merge_requests": [merge_request.clone()],
                "merge_request": merge_request,
                "evidence": {
                    "has_merge_request": true,
                    "has_current_subject_ref": subject_ref.is_some(),
                    "has_review_request": true,
                    "has_commit": subject_ref.is_some(),
                    "review_status": "approved",
                    "approved_current_subject": subject_ref.is_some(),
                    "review_after_rescope": attested,
                    "unresolved_request_changes": false,
                    "complete_for_integration": merged,
                    "missing": if subject_ref.is_none() {
                        vec!["current_subject_ref"]
                    } else {
                        vec![]
                    }
                }
            }))
            .expect("source evidence must project");
            let projected = serde_json::to_value(projected).expect("serialize Ticket detail");

            assert!(projected.get("merge_request").is_none());
            for merge in [&projected["merge_requests"][0]] {
                assert_eq!(merge["subject_ref"], json!(subject_ref));
                assert_eq!(merge["source_ref_observation"], observation);
                assert_eq!(
                    merge.get("integration_evidence_error"),
                    Some(&json!(integration_error))
                );
            }
            assert_eq!(projected["content_digest"], "current-ticket-content-digest");
            assert_eq!(projected["evidence"]["review_after_rescope"], attested);
            assert_eq!(projected["evidence"]["complete_for_integration"], merged);
        }
    }

    #[test]
    fn merge_request_projection_rejects_missing_or_invalid_source_observation() {
        let merge_request = json!({
            "merge_request_id": "mr-source",
            "repository_key": "repository",
            "state": "open",
            "selector_to": "main",
            "review_status": "none",
            "current_subject_ref": null,
            "integration_evidence_error": null
        });
        assert!(
            project_merge_request(&merge_request)
                .expect_err("source observation is required")
                .contains("source_ref_observation")
        );
        for observation in [
            Value::Null,
            json!({"status": "unknown"}),
            json!({"status": "unavailable"}),
            json!({"status": "unavailable", "code": 42}),
            json!({"status": "observed", "code": "unexpected"}),
        ] {
            let mut merge_request = merge_request.clone();
            merge_request["source_ref_observation"] = observation;
            assert!(
                project_merge_request(&merge_request)
                    .expect_err("invalid source observation must fail")
                    .contains("source_ref_observation")
            );
        }
    }

    #[test]
    fn relation_projection_accepts_current_workspace_api_shapes() {
        let outgoing = project_relation(
            &json!({
                "ticket_id": "internal-source-ticket",
                "kind": "depends_on",
                "target": "internal-target-ticket",
                "target_resource_key": "T-535",
                "note": "required foundation",
                "author": "internal-author",
                "at": "2026-08-22T00:00:00Z"
            }),
            "target_resource_key",
            "kind",
        )
        .expect("outgoing relation projection");
        let incoming = project_relation(
            &json!({
                "source_ticket": "internal-source-ticket",
                "source_resource_key": "T-536",
                "inverse_kind": "blocks",
                "forward_kind": "depends_on",
                "note": null,
                "author": "internal-author",
                "at": "2026-08-22T00:01:00Z"
            }),
            "source_resource_key",
            "forward_kind",
        )
        .expect("incoming relation projection");

        let outgoing = serde_json::to_value(outgoing).expect("serialize outgoing relation");
        assert_eq!(
            outgoing,
            json!({
                "ticket": "T-535",
                "kind": "depends_on",
                "note": "required foundation",
                "created_at": "2026-08-22T00:00:00Z"
            })
        );
        let incoming = serde_json::to_value(incoming).expect("serialize incoming relation");
        assert_eq!(
            incoming,
            json!({
                "ticket": "T-536",
                "kind": "depends_on",
                "note": null,
                "created_at": "2026-08-22T00:01:00Z"
            })
        );
        let projection = format!("{outgoing}{incoming}");
        for internal in [
            "internal-source-ticket",
            "internal-target-ticket",
            "internal-author",
        ] {
            assert!(!projection.contains(internal));
        }
    }

    #[test]
    fn relation_projection_rejects_missing_workspace_keys() {
        let outgoing = json!({
            "kind": "depends_on",
            "target": "internal-target-ticket",
            "note": null,
            "author": "internal-author",
            "at": "2026-08-22T00:00:00Z"
        });
        let incoming = json!({
            "source_resource_key": "not-a-ticket-key",
            "forward_kind": "depends_on",
            "note": null,
            "author": "internal-author",
            "at": "2026-08-22T00:01:00Z"
        });
        assert!(
            project_relation(&outgoing, "target_resource_key", "kind")
                .expect_err("missing outgoing key must fail")
                .contains("T-")
        );
        assert!(
            project_relation(&incoming, "source_resource_key", "forward_kind")
                .expect_err("invalid incoming key must fail")
                .contains("T-")
        );
    }

    #[test]
    fn resource_projection_rejects_noncanonical_keys() {
        for (key, prefix) in [("T-key", "T-"), ("O-", "O-"), ("W-1x", "W-")] {
            assert!(validate_resource_ref(key.to_string(), prefix).is_err());
        }
    }

    #[test]
    fn ticket_projection_fails_closed_without_worker_resource_key() {
        let error = project_worker(&json!({"worker_resource_key": null}))
            .expect_err("missing W-key must fail");
        assert!(error.contains("W-"));
    }
    #[test]
    fn ticket_query_and_detail_preserve_case_classification_and_delegation_independently() {
        let tracking = json!({"workspace_id":"ws", "ticket_id":"internal-id", "assignment_id":"delegation", "role":"orchestrator", "principal":{"kind":"workspace_agent","agent_key":"workspace-orchestrator"}, "assigned_by":"workspace-user:alice", "assigned_at":"2026-01-01T00:00:00Z", "release":null, "current":true, "last_event_id":"reported", "updated_at":"2026-01-01T00:00:00Z", "migration_diagnostic":null, "tracking":{"status":"waiting", "waiting_reason":"Human answer", "progress":null, "next_action":"Compare the response", "result":null}});
        let tag = json!({"workspace_id":"ws", "id":"tag-1", "name":"closed", "last_operation_key":"tag-create", "deleted":false});
        let closure = json!({"operation_key":"close-1", "reason":"not_planned", "explanation":"Not proceeding", "references":[{"kind":"ticket", "target":"T-12"}], "author":"workspace-user:alice", "at":"2026-01-01T00:00:00Z"});
        let history = json!([{"operation_key":"close-1", "previous_closure_operation_key":null, "action":{"action":"close", "reason":"not_planned"}, "explanation":"Not proceeding", "references":[{"kind":"ticket", "target":"T-12"}], "author":"workspace-user:alice", "at":"2026-01-01T00:00:00Z"}]);
        let detail = json!({
            "resource_key":"T-739", "title":"Case", "body":"Requirements", "body_truncated":false, "targets":[], "initial_post":post_fixture(true, "Requirements"), "created_by":"workspace-user:creator", "content_digest":"content-1",
            "state":"closed", "assignment_tracking": [tracking], "tags":[tag], "closure_operation_key":"close-1", "current_closure":closure,
            "legacy_handoff":null, "closure_history":history, "events":[], "linked_objectives":[], "assignments":[], "implementation_reports":[], "merge_requests":[]
        });
        let mut item = detail.clone();
        item["matched_fields"] = json!([]);
        item["linked_objective_keys"] = json!([]);
        for field in [
            "relation_count",
            "blocker_count",
            "unresolved_blocker_count",
            "unresolved_review_count",
        ] {
            item[field] = json!(0);
        }
        let projected_detail =
            serde_json::to_value(project_ticket_detail(detail.clone()).unwrap()).unwrap();
        let projected_query = serde_json::to_value(
            project_ticket_query(
                json!({"items":[item.clone()], "page":{"has_more":false, "next_cursor":null}}),
            )
            .unwrap(),
        )
        .unwrap();
        for field in [
            "tags",
            "state",
            "closure_operation_key",
            "current_closure",
            "legacy_handoff",
        ] {
            let expected = if field == "current_closure" {
                closure_metadata(&detail[field]).unwrap()
            } else {
                detail[field].clone()
            };
            assert_eq!(projected_detail[field], expected, "detail {field}");
            assert_eq!(
                projected_query["tickets"][0][field], expected,
                "query {field}"
            );
        }
        for projected in [&projected_detail, &projected_query["tickets"][0]] {
            let actual = &projected["assignment_tracking"][0];
            assert_eq!(actual["assignment_id"], "delegation");
            assert_eq!(actual["last_event_id"], "reported");
            assert_eq!(actual["tracking"], tracking["tracking"]);
            assert!(actual.get("workspace_id").is_none());
            assert!(actual.get("ticket_id").is_none());
        }
        assert_eq!(
            projected_detail["closure_history"],
            json!(
                history
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(closure_metadata)
                    .collect::<Result<Vec<_>, _>>()
                    .unwrap()
            )
        );
        assert_eq!(projected_detail["content_digest"], "content-1");
        assert_eq!(projected_detail["created_by"], "workspace-user:creator");
        assert_eq!(projected_detail["initial_post"]["body_truncated"], false);
        assert!(projected_detail.get("body").is_none());
        for initial in [true, false] {
            let input = post_fixture(initial, "The same post contract");
            let output = serde_json::to_value(project_ticket_event(&input).unwrap()).unwrap();
            for field in [
                "event_ref",
                "is_initial",
                "author",
                "edited_at",
                "edited_by",
                "body",
                "body_digest",
                "body_truncated",
                "editable",
            ] {
                assert_eq!(output[field], input[field], "post metadata {field}");
            }
            assert_eq!(output["created_at"], input["at"]);
        }
        assert_eq!(
            projected_detail["initial_post"]["author"],
            "workspace-user:original"
        );
        assert_eq!(
            projected_detail["initial_post"]["edited_by"],
            "workspace-user:editor"
        );
        let mut missing_post = detail.clone();
        missing_post.as_object_mut().unwrap().remove("initial_post");
        assert!(
            project_ticket_detail(missing_post)
                .unwrap_err()
                .contains("initial_post")
        );
        // Unknown observations must not be replaced with empty/null values.
        for field in [
            "tags",
            "closure_operation_key",
            "current_closure",
            "legacy_handoff",
            "state",
        ] {
            let mut missing_detail = detail.clone();
            missing_detail.as_object_mut().unwrap().remove(field);
            assert!(
                project_ticket_detail(missing_detail)
                    .unwrap_err()
                    .contains(field),
                "detail {field}"
            );
            let mut missing_item = item.clone();
            missing_item.as_object_mut().unwrap().remove(field);
            assert!(
                project_ticket_query(
                    json!({"items":[missing_item], "page":{"has_more":false, "next_cursor":null}})
                )
                .unwrap_err()
                .contains(field),
                "query {field}"
            );
        }
        for invalid_tags in [
            json!([{"id":"tag-1"}]),
            json!(vec![detail["tags"][0].clone(); 101]),
            {
                let mut deleted = detail["tags"].clone();
                deleted[0]["deleted"] = json!(true);
                deleted
            },
        ] {
            let mut malformed = detail.clone();
            malformed["tags"] = invalid_tags;
            assert!(project_ticket_detail(malformed).is_err());
        }
        for state in [
            "planning",
            "ready",
            "queued",
            "inprogress",
            "done",
            "unknown",
        ] {
            let mut malformed = detail.clone();
            malformed["state"] = json!(state);
            assert!(
                project_ticket_detail(malformed)
                    .unwrap_err()
                    .contains("state"),
                "{state}"
            );
        }
    }

    #[test]
    fn ticket_views_keep_prose_once_and_summary_independent_of_prose_and_history() {
        let body = "unique-long-requirement-".repeat(600);
        let post = post_fixture(true, &body);
        let closure = json!({"operation_key":"close", "reason":"completed", "explanation":"closure prose", "references":[], "author":"user", "at":"today"});
        let tracking = json!({"workspace_id":"ws", "ticket_id":"internal", "assignment_id":"assignment", "role":"orchestrator", "principal":{"kind":"workspace_agent","agent_key":"workspace-orchestrator"}, "assigned_by":"user", "assigned_at":"today", "release":null, "current":true, "last_event_id":"handling-event", "updated_at":"today", "migration_diagnostic":null, "tracking":{"status":"waiting", "waiting_reason":"prose", "progress":null, "next_action":"prose", "result":null}});
        let merge = json!({"merge_request_id":"MR-1", "repository_key":"main", "state":"open", "selector_from":"topic", "selector_to":"main", "review_status":"request_changes", "current_subject_ref":"sha1", "source_ref_observation":{"status":"observed"}, "integration_evidence_error":null, "review_excerpt":"review prose"});
        let mut input = json!({
            "resource_key":"T-848", "title":"Views", "body":body, "body_truncated":false, "initial_post":post,
            "content_digest":"content", "state":"closed", "closure_operation_key":"close", "current_closure":closure,
            "closure_history":[], "legacy_handoff":null, "targets":[], "tags":[], "assignment_tracking":[tracking],
            "events":[post, post_fixture(false, "discussion")], "assignments":[], "linked_objectives":[],
            "implementation_reports":[], "merge_requests":[merge.clone()], "merge_request":merge,
            "event_page":{"next_cursor":"stable-cursor", "has_more":true}
        });
        let detail = serde_json::to_value(project_ticket_detail(input.clone()).unwrap()).unwrap();
        assert_eq!(detail.to_string().matches(&body).count(), 1);
        assert!(detail.get("body").is_none());
        assert!(detail.get("merge_request").is_none());
        assert_eq!(detail["thread"].as_array().unwrap().len(), 1);
        assert_eq!(detail["initial_post"]["event_ref"], "post-initial");
        assert_eq!(detail["initial_post"]["body_digest"], "a".repeat(64));
        assert!(detail["current_closure"].get("explanation").is_none());
        let history = project_ticket_history(input.clone()).unwrap();
        assert_eq!(history.as_object().unwrap().len(), 3);
        assert_eq!(history["event_page"]["next_cursor"], "stable-cursor");
        let summary = project_ticket_status(input.clone()).unwrap();
        // Reconstruct the removed model-facing aliases on this same fixture.
        let mut duplicated = detail.clone();
        duplicated["body"] = detail["initial_post"]["body"].clone();
        duplicated["body_truncated"] = detail["initial_post"]["body_truncated"].clone();
        duplicated["merge_request"] = detail["merge_requests"][0].clone();
        duplicated["thread"]
            .as_array_mut()
            .unwrap()
            .insert(0, detail["initial_post"].clone());
        assert_eq!(duplicated.to_string().matches(&body).count(), 3);
        assert!(duplicated.to_string().len() > detail.to_string().len());
        let mut changed = input.clone();
        changed["content_digest"] = json!("new-requirements-digest");
        changed["merge_requests"][0]["current_subject_ref"] = json!("sha2");
        changed["merge_requests"][0]["review_status"] = json!("pending");
        let changed = project_ticket_status(changed).unwrap();
        assert_ne!(summary["content_digest"], changed["content_digest"]);
        assert_ne!(
            summary["merge_requests"][0]["subject_ref"],
            changed["merge_requests"][0]["subject_ref"]
        );
        assert_ne!(
            summary["merge_requests"][0]["review_status"],
            changed["merge_requests"][0]["review_status"]
        );
        assert_eq!(summary["content_digest"], "content");
        assert_eq!(summary["closure_operation_key"], "close");
        assert_eq!(summary["assignment_tracking"][0]["status"], "waiting");
        assert_eq!(
            summary["assignment_tracking"][0]["last_event_id"],
            "handling-event"
        );
        assert_eq!(summary["merge_requests"][0]["subject_ref"], "sha1");
        assert_eq!(
            summary["merge_requests"][0]["review_status"],
            "request_changes"
        );
        let huge = "全文🦀".repeat(100_000);
        input["body"] = json!(huge);
        input["initial_post"]["body"] = json!(huge);
        input["current_closure"]["explanation"] = json!(huge);
        input["assignment_tracking"][0]["tracking"]["waiting_reason"] = json!(huge);
        input["merge_requests"][0]["review_excerpt"] = json!(huge);
        input["closure_history"] = json!(vec![json!({"explanation":huge}); 20]);
        input["implementation_reports"] = json!(vec![json!({"excerpt":huge}); 20]);
        input["events"] = json!(vec![post_fixture(false, "history prose"); 1000]);
        let larger = project_ticket_status(input).unwrap();
        assert_eq!(summary, larger);
        eprintln!(
            "Ticket workspace bytes: duplicated fixture={}, detail={}, summary={} (identical with large prose and 1000 events)",
            duplicated.to_string().len(),
            detail.to_string().len(),
            summary.to_string().len()
        );
        let mut full = post_fixture(false, &huge);
        assert_eq!(project_ticket_post(full.clone()).unwrap()["body"], huge);
        full["body_truncated"] = json!(true);
        assert!(project_ticket_post(full).is_err());
    }

    #[test]
    fn ticket_tag_history_projection_preserves_observed_names_and_identities() {
        let tag = serde_json::json!({"workspace_id":"ws","id":"tag-1","name":"Old name","last_operation_key":"create","deleted":false});
        let mut event = post_fixture(false, "Ticket tags changed");
        event["kind"] = json!("tags_changed");
        event["editable"] = json!(false);
        event["attributes"] = serde_json::json!({"tags_before":serde_json::json!([tag.clone()]).to_string(),"tags_after":"[]"});
        let projected = serde_json::to_value(project_ticket_event(&event).unwrap()).unwrap();
        assert_eq!(projected["tag_changes"]["before"], serde_json::json!([tag]));
        assert_eq!(projected["tag_changes"]["after"], serde_json::json!([]));
        let ordinary = post_fixture(false, "hello");
        assert!(
            serde_json::to_value(project_ticket_event(&ordinary).unwrap())
                .unwrap()
                .get("tag_changes")
                .is_none()
        );
    }
}
