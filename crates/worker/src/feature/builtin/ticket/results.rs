//! Native result navigation is separate from bounded Ticket data.
//!
//! Only explicit domain reference fields and the route-bound subject produce Entries. IDs in
//! bodies, tracking prose, events, closure evidence, cursors, selectors and Worker/assignment
//! identities remain data; they are not independently published Ticket/Objective/MR Objects.
//! `data` retains the evolving bounded domain representation as Json, while `entry` and `links`
//! are declared Records/Entries. Consumers select typed fields, never parse data or signatures to
//! recover routes. Query pagination is declared separately; a missing cursor is not an empty ID.
//!
//! Cross-domain roots come from the final committed Host publication snapshot at dispatch, never
//! namespace allocation or intermediate mount order. Unpublished domains omit optional Entries
//! but keep already-visible IDs. References attest neither existence nor permission: Inspect may
//! describe the route-bound interface, and read still performs current authorization and can return
//! PermissionDenied/NotFound. No target is fetched here, indexed into Tree, or used to retry a
//! completed mutation. Historical dependencies and saved evidence are navigation, not start rights.
//!
//! Refresh native descriptors after upgrading. Query returns `items` containing `entry`, `data`
//! and `links`; creation and item operations return one such envelope. Notification diagnostics
//! have no independently published diagnostic item, so return only `data` and `links`. Tag catalog
//! operations remain Json because tags are classification data, not native Objects. Tool mode is
//! unchanged. Value dispatch precedes presentation; result mapping never reparses a ToolOutput.

use super::*;
use wip_protocol::{FieldDeclaration, ReturnDeclaration, TypeExpr};

fn field(name: &str, required: bool, r#type: TypeExpr) -> FieldDeclaration {
    FieldDeclaration {
        name: name.into(),
        documentation: None,
        required,
        r#type,
    }
}
fn record(fields: Vec<FieldDeclaration>) -> TypeExpr {
    TypeExpr::Record { fields }
}
fn list(items: TypeExpr) -> TypeExpr {
    TypeExpr::List {
        items: Box::new(items),
    }
}
fn link_type() -> TypeExpr {
    record(vec![
        field("id", true, TypeExpr::String),
        field("entry", false, TypeExpr::Entry),
    ])
}
fn item_type(subject: bool) -> TypeExpr {
    let mut fields = vec![
        field("data", true, TypeExpr::Json),
        field(
            "links",
            true,
            record(vec![
                field("tickets", true, list(link_type())),
                field("objectives", true, list(link_type())),
                field("merge_requests", true, list(link_type())),
            ]),
        ),
    ];
    if subject {
        fields.insert(0, field("entry", true, TypeExpr::Entry));
    }
    record(fields)
}
pub(super) fn declaration(operation: &str, item: bool) -> Option<ReturnDeclaration> {
    let ty = match operation {
        "query" => record(vec![
            field("items", true, list(item_type(true))),
            field("next_cursor", false, TypeExpr::String),
            field("has_more", true, TypeExpr::Boolean),
        ]),
        "create" => item_type(true),
        "notification_diagnostics" => item_type(false),
        _ if item => item_type(true),
        _ => return None,
    };
    Some(ReturnDeclaration{documentation:Some(Documentation{
        summary:"Typed subject/related Entries separate from bounded domain data and paging. Entries are lazy coordinates, never authority or existence attestations.".into(),
        details:Some("Inspect the selected entry, then its Interface, and read. IDs, cursors and body/event/evidence JSON in data are not navigation. Missing cross-domain publication omits link.entry; unpublished, deleted, or denied targets fail only when selected, without replaying the successful operation.".into()),
    }),r#type:ty})
}
fn failure(message: &str) -> WipOperationError {
    WipOperationError::OutcomeUnknown(format!("Ticket result projection: {message}"))
}
fn leaf(value: &Value, name: &str) -> Option<String> {
    value.get(name).and_then(Value::as_str).map(str::to_owned)
}
fn array<'a>(value: &'a Value, name: &str) -> &'a [Value] {
    value
        .get(name)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
}
fn add(ids: &mut BTreeSet<String>, value: &Value, name: &str) {
    if let Some(id) = leaf(value, name) {
        ids.insert(id);
    }
}
fn strings(ids: &mut BTreeSet<String>, value: &Value, name: &str) {
    ids.extend(
        array(value, name)
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned),
    );
}
fn ticket_leaf(id: &str) -> bool {
    is_ticket_route_reference(id)
}
fn mr_leaf(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id != "."
        && id != ".."
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~'))
}
fn objective_leaf(id: &str) -> bool {
    id.strip_prefix("O-")
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        || ((8..=128).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric()))
}
fn links(ids: BTreeSet<String>, root: &str, valid: fn(&str) -> bool) -> WipValue {
    WipValue::List(
        ids.into_iter()
            .map(|id| {
                let entry = (!root.is_empty() && valid(&id)).then(|| format!("{root}/{id}"));
                let mut fields = BTreeMap::from([("id".into(), WipValue::String(id))]);
                if let Some(entry) = entry {
                    fields.insert("entry".into(), WipValue::String(entry));
                }
                WipValue::Record(fields)
            })
            .collect(),
    )
}
#[derive(Default)]
struct References {
    tickets: BTreeSet<String>,
    objectives: BTreeSet<String>,
    mrs: BTreeSet<String>,
}
impl References {
    fn select(operation: &str, data: &Value) -> Self {
        let mut refs = Self::default();
        match operation {
            "query" | "read" => {
                // ModelTicketQueryItem/ModelTicketDetail contain redacted, bounded relationships.
                if let Some(relations) = data.get("relations") {
                    for kind in ["outgoing", "incoming", "blockers"] {
                        for relation in array(relations, kind) {
                            add(&mut refs.tickets, relation, "ticket");
                        }
                    }
                }
                for objective in array(data, "linked_objectives") {
                    if let Some(id) = objective.as_str() {
                        refs.objectives.insert(id.into());
                    } else {
                        add(&mut refs.objectives, objective, "objective");
                    }
                }
                for mr in array(data, "merge_requests") {
                    add(&mut refs.mrs, mr, "merge_request_id");
                }
                if let Some(mr) = data.get("merge_request") {
                    add(&mut refs.mrs, mr, "merge_request_id");
                }
            }
            "edit" | "edit_post" | "case_update" => {
                // Shared domain Ticket/detail results use a distinct, explicit relation schema.
                if let Some(relations) = data.get("relations") {
                    for relation in array(relations, "outgoing") {
                        add(&mut refs.tickets, relation, "ticket_id");
                        add(&mut refs.tickets, relation, "target");
                    }
                    for (kind, field) in [
                        ("incoming", "source_ticket"),
                        ("blockers", "blocking_ticket"),
                        ("notices", "related_ticket"),
                    ] {
                        for relation in array(relations, kind) {
                            add(&mut refs.tickets, relation, field);
                        }
                    }
                }
            }
            "record_relation" | "remove_relation" => {
                add(&mut refs.tickets, data, "ticket_id");
                add(&mut refs.tickets, data, "target");
            }
            "record_orchestration_plan" => {
                add(&mut refs.tickets, data, "ticket_id");
                add(&mut refs.tickets, data, "related_ticket");
            }
            "query_orchestration_plans" => {
                for plan in array(data, "records") {
                    add(&mut refs.tickets, plan, "ticket_id");
                    add(&mut refs.tickets, plan, "related_ticket");
                }
            }
            "dependency_check" => {
                if let Some(ticket) = data.get("ticket") {
                    if let Some(id) = leaf(ticket, "resource_key").or_else(|| leaf(ticket, "id")) {
                        refs.tickets.insert(id);
                    }
                }
                for blocker in array(data, "blockers") {
                    add(&mut refs.tickets, blocker, "blocking_ticket");
                }
                strings(&mut refs.tickets, data, "queue_tickets");
            }
            "implementation_result" | "implementation_result_query" => {
                add(&mut refs.tickets, data, "ticket_id");
                if let Some(mr) = data
                    .get("request")
                    .and_then(|request| request.get("merge_request"))
                {
                    add(&mut refs.mrs, mr, "merge_request_id");
                }
            }
            "assignment_tracking_query" => {
                // The domain's current/retained tracking lookup is a list, not an Object per row.
                if let Some(rows) = data.as_array() {
                    for row in rows {
                        add(&mut refs.tickets, row, "ticket_id");
                    }
                }
            }
            "assignment_tracking_update" => {
                add(&mut refs.tickets, data, "ticket_id");
            }
            "assignments_query" => {
                for row in array(data, "items") {
                    add(&mut refs.tickets, row, "ticket_id");
                }
            }
            "orchestrator_assign" | "orchestrator_clear" => {
                add(&mut refs.tickets, data, "ticket_id");
                if let Some(row) = data.get("assignment") {
                    add(&mut refs.tickets, row, "ticket_id");
                }
            }
            "notification_diagnostics" => {
                for row in array(data, "items") {
                    add(&mut refs.tickets, row, "ticket");
                    add(&mut refs.mrs, row, "merge_request_id");
                }
            }
            _ => {} // Tag/answer/tracking-history/delivery identifiers and prose are domain data.
        }
        refs
    }
}
fn envelope(
    operation: &str,
    data: Value,
    subject: Option<&str>,
    ticket_root: &str,
    mr_root: &str,
    objective_root: &str,
) -> Result<WipValue, WipOperationError> {
    let refs = References::select(operation, &data);
    let mut fields = BTreeMap::from([
        (
            "data".into(),
            json_to_wip(&data).map_err(WipOperationError::OutcomeUnknown)?,
        ),
        (
            "links".into(),
            WipValue::Record(BTreeMap::from([
                (
                    "tickets".into(),
                    links(refs.tickets, ticket_root, ticket_leaf),
                ),
                (
                    "objectives".into(),
                    links(refs.objectives, objective_root, objective_leaf),
                ),
                ("merge_requests".into(), links(refs.mrs, mr_root, mr_leaf)),
            ])),
        ),
    ]);
    if let Some(id) = subject {
        if !ticket_leaf(id) {
            return Err(failure("invalid subject identity"));
        }
        fields.insert(
            "entry".into(),
            WipValue::String(format!("{ticket_root}/{id}")),
        );
    }
    Ok(WipValue::Record(fields))
}
pub(super) fn project(
    operation: &str,
    data: Value,
    bound: Option<&str>,
    ticket_root: &str,
    mr_root: &str,
    objective_root: &str,
) -> Result<WipValue, WipOperationError> {
    match operation {
        "query" => {
            let items = data
                .get("tickets")
                .and_then(Value::as_array)
                .ok_or_else(|| failure("missing bounded items"))?
                .iter()
                .map(|data| {
                    let id = leaf(data, "ticket")
                        .filter(|id| is_canonical_ticket_resource_key(id))
                        .ok_or_else(|| failure("missing canonical subject"))?;
                    envelope(
                        "query",
                        data.clone(),
                        Some(&id),
                        ticket_root,
                        mr_root,
                        objective_root,
                    )
                })
                .collect::<Result<_, _>>()?;
            let has_more = data
                .get("has_more")
                .and_then(Value::as_bool)
                .ok_or_else(|| failure("missing paging state"))?;
            let mut fields = BTreeMap::from([
                ("items".into(), WipValue::List(items)),
                ("has_more".into(), WipValue::Boolean(has_more)),
            ]);
            if let Some(cursor) = leaf(&data, "next_cursor") {
                fields.insert("next_cursor".into(), WipValue::String(cursor));
            }
            Ok(WipValue::Record(fields))
        }
        "create" | "read" => {
            let id = leaf(
                &data,
                if operation == "create" {
                    "id"
                } else {
                    "ticket"
                },
            )
            .filter(|id| is_canonical_ticket_resource_key(id))
            .ok_or_else(|| failure("missing canonical subject"))?;
            envelope(
                operation,
                data,
                Some(&id),
                ticket_root,
                mr_root,
                objective_root,
            )
        }
        "notification_diagnostics" => {
            envelope(operation, data, None, ticket_root, mr_root, objective_root)
        }
        _ if bound.is_some() => {
            envelope(operation, data, bound, ticket_root, mr_root, objective_root)
        }
        _ => json_to_wip(&data).map_err(WipOperationError::OutcomeUnknown),
    }
}
#[cfg(test)]
mod tests;
