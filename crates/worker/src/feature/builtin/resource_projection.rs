use serde::Serialize;
use serde_json::{Map, Value};

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
    state: String,
    readiness: Option<String>,
    priority: Option<String>,
    created_at: Option<String>,
    updated_at: Option<String>,
    workspace_action_priority: Option<String>,
    matched_fields: Vec<String>,
    snippet: Option<String>,
    current_coder: Option<ModelWorkerSummary>,
    linked_objectives: Vec<String>,
    relation_count: usize,
    blocker_count: usize,
    unresolved_blocker_count: usize,
    unresolved_review_count: usize,
    evidence: Option<ModelTicketEvidence>,
    merge_request: Option<ModelMergeRequest>,
}

#[derive(Debug, Serialize)]
pub(super) struct ModelTicketDetail {
    ticket: String,
    title: String,
    body: String,
    state: String,
    readiness: Option<String>,
    priority: Option<String>,
    created_at: Option<String>,
    updated_at: Option<String>,
    thread: Vec<ModelTicketEvent>,
    relations: ModelTicketRelations,
    linked_objectives: Vec<ModelObjectiveSummary>,
    assignments: Vec<ModelAssignment>,
    current_coder: Option<ModelWorkerSummary>,
    implementation_reports: Vec<ModelEvidenceEvent>,
    merge_request: Option<ModelMergeRequest>,
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
    state: String,
    created_at: Option<String>,
    updated_at: Option<String>,
    linked_tickets: Vec<ModelTicketSummary>,
    events: Vec<ModelObjectiveEvent>,
    event_page: ModelObjectiveEventPage,
}

impl ModelObjectiveDetail {
    pub(super) fn objective_ref(&self) -> &str {
        &self.objective
    }
}

#[derive(Debug, Serialize)]
struct ModelWorkerSummary {
    worker: String,
}

#[derive(Debug, Serialize)]
struct ModelTicketEvent {
    sequence: usize,
    kind: String,
    body: Option<String>,
    created_at: Option<String>,
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
    resolved: bool,
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
    state: String,
    selector_from: Option<String>,
    selector_to: String,
    review_status: String,
    subject_ref: Option<String>,
    review_excerpt: Option<String>,
}

#[derive(Debug, Serialize)]
struct ModelTicketEvidence {
    has_merge_request: bool,
    has_current_subject_ref: bool,
    has_review_request: bool,
    has_commit: bool,
    review_status: Option<String>,
    approved_current_subject: bool,
    unresolved_request_changes: bool,
    complete_for_integration: bool,
    missing: Vec<String>,
}

#[derive(Debug, Serialize)]
struct ModelTicketActions {
    can_assign_orchestrator: bool,
    can_unassign_orchestrator: bool,
    can_queue: bool,
    can_start_manual_coder: bool,
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
        state: string_field(item, "state")?,
        readiness: optional_string(item, "readiness")?,
        priority: optional_string(item, "priority")?,
        created_at: optional_string(item, "created_at")?,
        updated_at: optional_string(item, "updated_at")?,
        workspace_action_priority: optional_string(item, "workspace_action_priority")?,
        matched_fields: string_array(item, "matched_fields")?,
        snippet: optional_string(item, "snippet")?,
        current_coder: item
            .get("current_coder")
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
        merge_request: item
            .get("merge_request")
            .filter(|value| !value.is_null())
            .map(project_merge_request)
            .transpose()?,
    })
}

pub(super) fn project_ticket_detail(value: Value) -> Result<ModelTicketDetail, String> {
    let root = object(&value, "Ticket detail response")?;
    let current_coder = root
        .get("current_coder")
        .filter(|value| !value.is_null())
        .map(project_worker)
        .transpose()?;
    let assignments = array_field(root, "assignments")?
        .iter()
        .map(|assignment| project_assignment(assignment, current_coder.as_ref()))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(ModelTicketDetail {
        ticket: resource_ref(root, "resource_key", "T-")?,
        title: string_field(root, "title")?,
        body: string_field(root, "body")?,
        state: string_field(root, "state")?,
        readiness: optional_string(root, "readiness")?,
        priority: optional_string(root, "priority")?,
        created_at: optional_string(root, "created_at")?,
        updated_at: optional_string(root, "updated_at")?,
        thread: array_field(root, "events")?
            .iter()
            .map(project_ticket_event)
            .collect::<Result<Vec<_>, _>>()?,
        relations: project_relations(root.get("relations"))?,
        linked_objectives: array_field(root, "linked_objectives")?
            .iter()
            .map(project_objective_summary)
            .collect::<Result<Vec<_>, _>>()?,
        assignments,
        current_coder,
        implementation_reports: array_field(root, "implementation_reports")?
            .iter()
            .map(project_evidence_event)
            .collect::<Result<Vec<_>, _>>()?,
        merge_request: root
            .get("merge_request")
            .filter(|value| !value.is_null())
            .map(project_merge_request)
            .transpose()?,
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
    Ok(ModelTicketEvent {
        sequence: usize_field(event, "sequence")?,
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
        resolved: bool_field(blocker, "resolved")?,
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
    current_coder: Option<&ModelWorkerSummary>,
) -> Result<ModelAssignment, String> {
    let assignment = object(value, "Ticket assignment")?;
    let principal = object_field(assignment, "principal")?;
    let kind = string_field(principal, "kind")?;
    let principal = match kind.as_str() {
        "worker" => current_coder
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
        state: string_field(merge, "state")?,
        selector_from: optional_string(merge, "selector_from")?,
        selector_to: string_field(merge, "selector_to")?,
        review_status: string_field(merge, "review_status")?,
        subject_ref: optional_string(merge, "subject_ref")?,
        review_excerpt: optional_string(merge, "review_excerpt")?,
    })
}

fn project_evidence(value: &Value) -> Result<ModelTicketEvidence, String> {
    let evidence = object(value, "Ticket evidence")?;
    Ok(ModelTicketEvidence {
        has_merge_request: bool_field(evidence, "has_merge_request")?,
        has_current_subject_ref: bool_field(evidence, "has_current_subject_ref")?,
        has_review_request: bool_field(evidence, "has_review_request")?,
        has_commit: bool_field(evidence, "has_commit")?,
        review_status: optional_string(evidence, "review_status")?,
        approved_current_subject: bool_field(evidence, "approved_current_subject")?,
        unresolved_request_changes: bool_field(evidence, "unresolved_request_changes")?,
        complete_for_integration: bool_field(evidence, "complete_for_integration")?,
        missing: string_array(evidence, "missing")?,
    })
}

fn project_actions(value: &Value) -> Result<ModelTicketActions, String> {
    let actions = object(value, "Ticket actions")?;
    Ok(ModelTicketActions {
        can_assign_orchestrator: bool_field(actions, "can_assign_orchestrator")?,
        can_unassign_orchestrator: bool_field(actions, "can_unassign_orchestrator")?,
        can_queue: bool_field(actions, "can_queue")?,
        can_start_manual_coder: bool_field(actions, "can_start_manual_coder")?,
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

    #[test]
    fn objective_projection_exposes_only_resource_references() {
        let projected = project_objective_detail(json!({
            "id": "00001M10HW6BV",
            "resource_key": "O-543",
            "title": "Objective",
            "body": "Body",
            "state": "active",
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-02T00:00:00Z",
            "linked_tickets": ["00001M0E82D1V"],
            "linked_ticket_summaries": [{
                "id": "00001M0E82D1V",
                "resource_key": "T-496",
                "title": "Ticket",
                "state": "done",
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
                "resource_key": "T-543",
                "title": "Ticket",
                "state": "inprogress",
                "readiness": null,
                "priority": "high",
                "created_at": null,
                "updated_at": "2026-01-01T00:00:00Z",
                "workspace_action_priority": "active_work",
                "matched_fields": ["title"],
                "snippet": "Ticket",
                "current_coder": {"runtime_id": "runtime-internal", "worker_id": "worker-internal", "worker_resource_key": "W-12"},
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
                    "unresolved_request_changes": false,
                    "complete_for_integration": false,
                    "missing": ["merge_request"]
                },
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
}
