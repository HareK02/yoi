//! Native Ticket actions are defined by their subjects and preconditions, not Tool schemas.
//!
//! Collection/item separation is retained: discovery and creation have no existing subject;
//! judgments, authoring, delegation and relationships do. Assignment/post identifiers are
//! observations within a Ticket, not independently authorized child Objects. Keeping them in
//! records avoids implying that observing a child grants independent execution authority.
//! Read and mutation actions remain separate because their permissions and retry safety differ.
//!
//! The contract tree generates both the public declaration and closed input conversion. The
//! converted command is private transport to shared domain execution, never a second public API.
//! Backend validation owns string bounds, conditional business requirements, exact observations,
//! authorization and replay. Native type rejection precedes that boundary. The result projection
//! separates typed navigation from bounded domain JSON, independently of these input contracts.
//!
//! For example, Inspect the collection and its Interface, then invoke `query` with
//! `{"filter":{"states":["open"]},"page":{"limit":10}}`. Inspect a returned typed item Entry and
//! its Interface before `read` with `{"page":{"limit":5}}`, or `comment` with
//! `{"body":"Confirmed requirement"}`. A relation target is an Entry, not a Tool Ticket ID.
//! v2 replaces v1 native arguments; refresh Object/Interface observations, do not replay mutations.

use super::*;
#[cfg(test)]
mod tests;
use crate::feature::builtin::native_contract::{
    Case, Contract, Destination, Input, ReferenceKind, ReferenceRoots, Shape,
};

fn p(name: &'static str, required: bool, ty: Shape) -> Input {
    Input {
        name,
        required,
        ty,
        destination: Destination::Same,
    }
}
fn rename(name: &'static str, destination: &'static str, required: bool, ty: Shape) -> Input {
    Input {
        name,
        required,
        ty,
        destination: Destination::Rename(destination),
    }
}
fn group(name: &'static str, required: bool, fields: Vec<Input>) -> Input {
    Input {
        name,
        required,
        ty: Shape::Record(fields),
        destination: Destination::Flatten,
    }
}
fn record(fields: Vec<Input>) -> Shape {
    Shape::Record(fields)
}
fn nullable(ty: Shape) -> Shape {
    Shape::Nullable(Box::new(ty))
}
fn list(ty: Shape) -> Shape {
    Shape::List(Box::new(ty))
}
fn strings() -> Shape {
    list(Shape::String)
}
fn enumeration(cases: &[&'static str]) -> Shape {
    Shape::Enum(cases.to_vec())
}
fn case(name: &'static str, payload: Option<Shape>) -> Case {
    Case { name, payload }
}
fn union(tag: &'static str, cases: Vec<Case>) -> Shape {
    Shape::Union { tag, cases }
}
fn state() -> Shape {
    enumeration(&["open", "closed"])
}
fn relation() -> Shape {
    enumeration(&[
        "depends_on",
        "blocks",
        "related",
        "supersedes",
        "duplicate_of",
    ])
}
fn ticket_ref() -> Shape {
    Shape::Reference(ReferenceKind::Ticket)
}
fn page(cursor: &'static str, limit: &'static str) -> Input {
    group(
        "page",
        false,
        vec![
            rename("cursor", cursor, false, Shape::String),
            rename("limit", limit, false, Shape::Integer),
        ],
    )
}
fn principal() -> Shape {
    union(
        "kind",
        vec![
            case(
                "subject",
                Some(record(vec![p("subject_id", true, Shape::String)])),
            ),
            case(
                "user",
                Some(record(vec![p("account_id", true, Shape::String)])),
            ),
            case(
                "worker",
                Some(record(vec![
                    p("runtime_id", true, Shape::String),
                    p("worker_id", true, Shape::String),
                ])),
            ),
            case(
                "workspace_agent",
                Some(record(vec![p("agent_key", true, Shape::String)])),
            ),
        ],
    )
}
fn assignment_principal() -> Shape {
    union(
        "kind",
        vec![case(
            "subject",
            Some(record(vec![p("subject_id", true, Shape::String)])),
        )],
    )
}
fn reason() -> Shape {
    // An externally tagged domain enum is encoded by this single native union definition.
    Shape::ExternalUnion {
        cases: vec![
            case("completed", None),
            case("not_planned", None),
            case("duplicate", None),
            case("unknown", None),
            case("other", Some(Shape::String)),
        ],
    }
}
fn target() -> Shape {
    record(vec![
        p("repository_key", true, Shape::String),
        p("ref_selector", false, nullable(Shape::String)),
        p("access", true, enumeration(&["read_only", "read_write"])),
    ])
}
fn references() -> Shape {
    list(record(vec![
        p("kind", true, Shape::String),
        p("target", true, Shape::String),
    ]))
}
fn assignment_observation() -> Input {
    group(
        "observed",
        true,
        vec![
            p("assignment_id", true, Shape::String),
            p("expected_last_event_id", true, Shape::String),
        ],
    )
}

pub(super) fn contract(op: &str) -> Contract {
    let (name, summary, parameters) = match op {
        "query" => (
            "query",
            "Discover bounded Tickets; Entry filters select related subjects, not authority. Backend caps pages and validates filter combinations.",
            vec![
                group(
                    "filter",
                    false,
                    vec![
                        p("query", false, Shape::String),
                        p("states", false, list(state())),
                        p("closure_reason", false, reason()),
                        p("assigned_principal", false, principal()),
                        p(
                            "handling_status",
                            false,
                            enumeration(&["waiting", "inprogress", "done"]),
                        ),
                        p("event_kinds", false, strings()),
                        p(
                            "evidence",
                            false,
                            list(enumeration(&["merge_request", "commit", "approved_review"])),
                        ),
                        p(
                            "review_status",
                            false,
                            enumeration(&[
                                "none",
                                "unknown",
                                "pending",
                                "approved",
                                "request_changes",
                                "unresolved_changes",
                            ]),
                        ),
                        p(
                            "attention",
                            false,
                            list(enumeration(&[
                                "unresolved_review",
                                "missing_commit",
                                "blocked",
                                "unblocked",
                                "awaiting_review",
                                "unresolved_changes",
                                "stale_after_rescope",
                                "missing_evidence",
                            ])),
                        ),
                        rename("related", "related_ticket_id", false, ticket_ref()),
                        p("relation_kind", false, relation()),
                        p("tag_ids", false, strings()),
                        p("tag_ids_any", false, strings()),
                        p("tag_ids_none", false, strings()),
                        rename(
                            "objective",
                            "linked_objective_id",
                            false,
                            Shape::Reference(ReferenceKind::Objective),
                        ),
                        p("updated_after", false, Shape::String),
                        p("updated_before", false, Shape::String),
                    ],
                ),
                p(
                    "sort",
                    false,
                    enumeration(&[
                        "relevance",
                        "updated_desc",
                        "created_desc",
                        "priority",
                        "title",
                    ]),
                ),
                page("cursor", "limit"),
            ],
        ),
        "create" => (
            "create",
            "Create a Ticket from content and optional ordered implementation targets. Selectors may remain unspecified while planning.",
            vec![
                group(
                    "content",
                    true,
                    vec![
                        p("title", true, Shape::String),
                        p("body", false, Shape::String),
                        p("state", false, state()),
                        p("assignee", false, Shape::String),
                        p("readiness", false, Shape::String),
                        p("risk_flags", false, strings()),
                        p("queued_at", false, Shape::String),
                    ],
                ),
                p("targets", false, list(target())),
            ],
        ),
        "read" => (
            "read",
            "Read this route-bound Ticket and a bounded thread page. Omit page for Backend defaults.",
            vec![page("event_cursor", "event_limit")],
        ),
        "edit" => (
            "edit",
            "Author this Ticket's content without appending discussion. Body replacement and substring editing are distinct choices; domain validates unique matches.",
            vec![
                p("title", false, Shape::String),
                Input {
                    name: "body",
                    required: false,
                    ty: union(
                        "mode",
                        vec![
                            case(
                                "replace",
                                Some(record(vec![rename("text", "body", true, Shape::String)])),
                            ),
                            case(
                                "splice",
                                Some(record(vec![
                                    rename("old", "old_string", true, Shape::String),
                                    rename("new", "new_string", true, Shape::String),
                                    p("replace_all", false, Shape::Boolean),
                                ])),
                            ),
                        ],
                    ),
                    destination: Destination::Flatten,
                },
                p(
                    "targets",
                    false,
                    union(
                        "action",
                        vec![
                            case("clear", None),
                            case(
                                "set",
                                Some(record(vec![p("targets", true, list(target()))])),
                            ),
                        ],
                    ),
                ),
            ],
        ),
        "edit_post" => (
            "edit_post",
            "Replace a persisted discussion post using its exact observed body. Truncated preimages and system events cannot authorize an edit.",
            vec![
                group(
                    "observed",
                    true,
                    vec![
                        p("event_ref", true, Shape::String),
                        p("expected_body", true, Shape::String),
                    ],
                ),
                p("body", true, Shape::String),
            ],
        ),
        "comment" => (
            "comment",
            "Append Markdown discussion to this Ticket; this is not a fenced assignment answer or result notification.",
            vec![p("body", true, Shape::String)],
        ),
        "case_update" => (
            "case_update",
            "Record a case judgment against exact observed content/state/closure key. Only completed asserts fulfillment. Reopen does not reissue work. Keep the operation key and full input on an uncertain outcome; never automatically replay.",
            vec![
                group(
                    "observed",
                    true,
                    vec![
                        rename(
                            "content_digest",
                            "expected_content_digest",
                            true,
                            Shape::String,
                        ),
                        rename("state", "expected_state", true, state()),
                        rename(
                            "closure_key",
                            "expected_closure_operation_key",
                            true,
                            nullable(Shape::String),
                        ),
                    ],
                ),
                p("operation_key", true, Shape::String),
                p(
                    "action",
                    true,
                    union(
                        "action",
                        vec![
                            case("close", Some(record(vec![p("reason", true, reason())]))),
                            case("reopen", None),
                            case(
                                "correct_closure",
                                Some(record(vec![p("reason", true, reason())])),
                            ),
                        ],
                    ),
                ),
                p("explanation", true, Shape::String),
                p("references", false, references()),
            ],
        ),
        "record_relation" => (
            "record_relation",
            "Record a forward Ticket relationship; inverse views are derived. Target Entry is distinct from this route's subject.",
            vec![
                p("kind", true, relation()),
                p("target", true, ticket_ref()),
                p("note", false, Shape::String),
            ],
        ),
        "remove_relation" => (
            "remove_relation",
            "Remove exactly the selected forward relationship, not the target Ticket.",
            vec![p("kind", true, relation()), p("target", true, ticket_ref())],
        ),
        "dependency_check" => (
            "dependency_check",
            "Read dependency diagnostics; do not start dependencies or infer execution permission.",
            vec![],
        ),
        "orchestrator_assign" => (
            "orchestrator_assign",
            "Delegate or explicitly reissue responsibility; no Worker starts. Observed null means unassigned, not unknown. Exact retries retain operation identity and full input.",
            vec![
                p("principal", true, assignment_principal()),
                group(
                    "observed",
                    true,
                    vec![rename(
                        "assignment_id",
                        "expected_assignment_id",
                        true,
                        nullable(Shape::String),
                    )],
                ),
                p("operation_id", true, Shape::String),
            ],
        ),
        "orchestrator_clear" => (
            "orchestrator_clear",
            "Release exactly the observed delegation, not Worker lifecycle or handling completion.",
            vec![
                p("assignment_id", true, Shape::String),
                p("operation_id", true, Shape::String),
            ],
        ),
        "assignments_query" => (
            "assignments_query",
            "List retained immutable delegation identities; history is not current execution authority.",
            vec![page("after_assignment_id", "limit")],
        ),
        "assignment_tracking_query" => (
            "assignment_tracking_query",
            "Read current delegation or the selected retained identity; unknown tracking is not waiting.",
            vec![p("assignment_id", false, Shape::String)],
        ),
        "assignment_tracking_history" => (
            "assignment_tracking_history",
            "Read immutable handling history for this delegation.",
            vec![
                p("assignment_id", true, Shape::String),
                page("after_event_id", "limit"),
            ],
        ),
        "assignment_tracking_update" => (
            "assignment_tracking_update",
            "Report the complete handling payload for the exact assignment/event observation. Domain validates waiting reasons and authenticated sender. Handling does not alter case state.",
            vec![
                assignment_observation(),
                p("operation_key", true, Shape::String),
                p(
                    "tracking",
                    true,
                    record(vec![
                        p(
                            "status",
                            true,
                            enumeration(&["waiting", "inprogress", "done"]),
                        ),
                        p("waiting_reason", false, nullable(Shape::String)),
                        p("progress", false, nullable(Shape::String)),
                        p("next_action", false, nullable(Shape::String)),
                        p("result", false, nullable(Shape::String)),
                    ]),
                ),
            ],
        ),
        "assignment_answer" => (
            "assignment_answer",
            "Continue the same waiting/inprogress assignment with fenced input, not automatic inprogress. Done needs explicit reissue.",
            vec![
                assignment_observation(),
                p("operation_key", true, Shape::String),
                p("body", true, Shape::String),
            ],
        ),
        "assignment_diagnostics" => (
            "assignment_diagnostics",
            "Read independent delivery/input/start evidence; acceptance is not successful processing.",
            vec![
                p("assignment_id", true, Shape::String),
                group(
                    "page",
                    false,
                    vec![
                        p("after_input_event_id", false, Shape::String),
                        p("after_start_operation_id", false, Shape::String),
                        p("limit", false, Shape::Integer),
                    ],
                ),
            ],
        ),
        "implementation_result" => (
            "implementation_result",
            "Publish a result, handoff or blocker with a Host-owned retry identity. This neither closes the case nor approves or integrates an MR; uncertain results are not automatically replayed.",
            vec![
                p("kind", true, enumeration(&["handoff", "result", "blocker"])),
                p("body", true, Shape::String),
                p(
                    "merge_request",
                    false,
                    record(vec![
                        rename(
                            "entry",
                            "merge_request_id",
                            true,
                            Shape::Reference(ReferenceKind::MergeRequest),
                        ),
                        p("subject_ref", true, Shape::String),
                    ]),
                ),
            ],
        ),
        "implementation_result_query" => (
            "implementation_result_query",
            "Read one retained result; it is historical evidence, not new execution authority.",
            vec![p("result_id", true, Shape::String)],
        ),
        "tag_list" => (
            "tag_list",
            "List active Workspace classification tags, not Ticket states.",
            vec![page("after", "limit")],
        ),
        "tag_create" => (
            "tag_create",
            "Create classification with stable retry identity; names never grant authority.",
            vec![
                p("name", true, Shape::String),
                p("operation_key", true, Shape::String),
            ],
        ),
        "tag_rename" => (
            "tag_rename",
            "Rename the observed stable tag without changing membership identity.",
            vec![
                group(
                    "observed",
                    true,
                    vec![
                        p("tag_id", true, Shape::String),
                        p("expected_last_operation_key", true, Shape::String),
                    ],
                ),
                p("name", true, Shape::String),
                p("operation_key", true, Shape::String),
            ],
        ),
        "tag_delete" => (
            "tag_delete",
            "Tombstone exactly the observed tag and remove membership atomically; this is not case closure.",
            vec![
                group(
                    "observed",
                    true,
                    vec![
                        p("tag_id", true, Shape::String),
                        p("expected_last_operation_key", true, Shape::String),
                    ],
                ),
                p("operation_key", true, Shape::String),
            ],
        ),
        "tags_set" => (
            "tags_set",
            "Replace the entire tag set against observed membership, not content CAS. Preserve unrelated classifications when reconciling conflicts.",
            vec![
                group(
                    "observed",
                    true,
                    vec![rename("tags", "expected_tag_ids", true, strings())],
                ),
                rename("tags", "tag_ids", true, strings()),
                p("operation_key", true, Shape::String),
            ],
        ),
        "notification_diagnostics" => (
            "notification_diagnostics",
            "Read sanitized routing/delivery evidence; exact origin filters are independently authorized. History and retry candidates are not execution permission.",
            vec![
                group(
                    "filter",
                    false,
                    vec![
                        p("assignment_id", false, Shape::String),
                        rename("ticket", "ticket", false, ticket_ref()),
                        rename(
                            "merge_request",
                            "merge_request_id",
                            false,
                            Shape::Reference(ReferenceKind::MergeRequest),
                        ),
                        p("job_id", false, Shape::String),
                        p("source_event_id", false, Shape::String),
                        p("delivery_id", false, Shape::String),
                        p("source_id", false, Shape::String),
                    ],
                ),
                page("cursor", "limit"),
            ],
        ),
        "record_orchestration_plan" => (
            "record_orchestration_plan",
            "Record project-level ordering/conflict/capacity/accepted-plan context without moving case state or starting work. Domain validates kind-specific requirements.",
            vec![
                p("kind", true, plan_kind()),
                p("related_ticket", false, ticket_ref()),
                p("note", false, Shape::String),
                p(
                    "accepted_plan",
                    false,
                    record(vec![
                        p("summary", true, Shape::String),
                        p("branch", false, Shape::String),
                        p("worktree", false, Shape::String),
                        p("role_plan", false, Shape::String),
                    ]),
                ),
            ],
        ),
        "query_orchestration_plans" => (
            "query_orchestration_plans",
            "Read bounded project-level plan records; planning history is not execution authority.",
            vec![
                rename("kind", "relation_kind", false, plan_kind()),
                p("limit", false, Shape::Integer),
            ],
        ),
        _ => unreachable!("every enabled native Ticket action has a contract: {op}"),
    };
    Contract {
        name,
        summary,
        parameters,
    }
}
fn plan_kind() -> Shape {
    enumeration(&[
        "before",
        "after",
        "blocked_by",
        "blocks",
        "conflicts_with",
        "do_not_parallelize",
        "waiting_capacity_note",
        "accepted_plan",
    ])
}

pub(super) fn input(
    op: &str,
    arguments: &BTreeMap<String, WipValue>,
    bound: Option<&str>,
    identity_field: Option<&str>,
    ticket_root: &str,
    mr_root: &str,
    objective_root: &str,
) -> Result<Value, WipOperationError> {
    let mut command = contract(op).convert(
        arguments,
        &ReferenceRoots {
            ticket: ticket_root,
            merge_request: mr_root,
            objective: objective_root,
        },
    )?;
    let fields = command.as_object_mut().expect("native command is a record");
    // Body alternatives are native editing choices, not domain fields.
    if op == "edit" {
        fields.remove("mode");
    }
    if let (Some(subject), Some(field)) = (bound, identity_field) {
        fields.insert(field.into(), json!(subject));
    }
    Ok(command)
}
