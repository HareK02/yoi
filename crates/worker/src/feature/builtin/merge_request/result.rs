//! Native MR results separate navigation from bounded domain evidence.
//!
//! `entry` is the route-bound subject (the response identity for open). `data` preserves the
//! Backend response, including display IDs, body/findings, repository selectors, page/state and
//! exact source/event IDs. In particular, a review's full linked-MR snapshot must remain intact:
//! its `subject_ref` strings are evidence, not navigable Objects. `links` selects only the
//! documented PublicMergeRequest.ticket_ids, Detail.linked_tickets[].ticket_id and
//! Review{Requested,}Event.ticket_merge_request_subjects[].merge_request_id fields. The latter
//! occur in bounded thread pages, readiness.review and review receipts; no arbitrary JSON, body,
//! finding path, or textual ID is searched recursively.
//!
//! References are coordinates, not existence observations or authority. Related Entries are
//! emitted only for roots in the final Host registry publication snapshot supplied on each call,
//! never from allocated input namespaces or an early mount-time snapshot. Missing publication or an
//! unroutable identity omits the optional Entry but retains already-disclosed domain IDs. There is
//! no prefetch, all-item registration, post-mutation target read, or retry: selected Entry Inspect
//! and read use the target's existing permission/deletion boundary independently of this success.

use super::*;
use std::collections::BTreeSet;
use wip_protocol::{FieldDeclaration, ReturnDeclaration, TypeExpr};

fn field(name: &str, required: bool, ty: TypeExpr) -> FieldDeclaration {
    FieldDeclaration {
        name: name.into(),
        required,
        documentation: None,
        r#type: ty,
    }
}

pub(super) fn declaration() -> ReturnDeclaration {
    let references = || TypeExpr::List {
        items: Box::new(TypeExpr::Record {
            fields: vec![
                field("id", true, TypeExpr::String),
                field("entry", false, TypeExpr::Entry),
            ],
        }),
    };
    ReturnDeclaration {
        documentation: Some(Documentation {
            summary: "Bounded MR result: entry is the subject; data retains domain evidence; links contains typed navigation only.".into(),
            details: Some("IDs, source refs, bodies, state and page metadata remain data, not inferred references. Related entry is omitted for unpublished spaces or unroutable IDs; publication is not existence, permission, or observed state. Inspect only selected Entries, then invoke their read operations. Failed target resolution never retries the originating mutation.".into()),
        }),
        r#type: TypeExpr::Record { fields: vec![
            field("entry", true, TypeExpr::Entry),
            field("data", true, TypeExpr::Json),
            field("links", true, TypeExpr::Record { fields: vec![
                field("tickets", true, references()),
                field("merge_requests", true, references()),
            ] }),
        ] },
    }
}

fn ticket_reference(id: &str) -> bool {
    // Match the Ticket item's public route grammar, not MR's broader single-segment grammar.
    id.strip_prefix("T-").is_some_and(|sequence| {
        !sequence.is_empty() && sequence.bytes().all(|byte| byte.is_ascii_digit())
    }) || ((8..=128).contains(&id.len()) && id.bytes().all(|byte| byte.is_ascii_alphanumeric()))
}

fn reference(id: &str, root: Option<&str>, routable: bool) -> Value {
    let mut value = json!({"id": id});
    if let Some(root) = root.filter(|_| routable) {
        value["entry"] = Value::String(format!("{root}/{id}"));
    }
    value
}

fn snapshot_ids(event: &Value, ids: &mut BTreeSet<String>) {
    if let Some(subjects) = event
        .get("ticket_merge_request_subjects")
        .and_then(Value::as_array)
    {
        for subject in subjects {
            if let Some(id) = subject.get("merge_request_id").and_then(Value::as_str) {
                ids.insert(id.into());
            }
        }
    }
}

pub(super) fn value(
    kind: Kind,
    data: Value,
    subject: &str,
    collection: &str,
    ticket_root: Option<&str>,
) -> Result<WipValue, WipOperationError> {
    let mut tickets = BTreeSet::new();
    let mut merge_requests = BTreeSet::new();
    match kind {
        Kind::Open | Kind::Show => {
            if let Some(ids) = data.get("ticket_ids").and_then(Value::as_array) {
                tickets.extend(ids.iter().filter_map(Value::as_str).map(str::to_string));
            }
            if kind == Kind::Show {
                if let Some(linked) = data.get("linked_tickets").and_then(Value::as_array) {
                    tickets.extend(
                        linked
                            .iter()
                            .filter_map(|ticket| ticket.get("ticket_id").and_then(Value::as_str))
                            .map(str::to_string),
                    );
                }
            }
            if let Some(thread) = data.get("thread").and_then(Value::as_array) {
                for event in thread {
                    if matches!(
                        event.get("kind").and_then(Value::as_str),
                        Some("review_requested" | "review")
                    ) {
                        snapshot_ids(event, &mut merge_requests);
                    }
                }
            }
        }
        Kind::Readiness => {
            if let Some(review) = data.get("review").filter(|review| !review.is_null()) {
                snapshot_ids(review, &mut merge_requests);
            }
        }
        Kind::Review => snapshot_ids(&data, &mut merge_requests),
        Kind::Complete => {}
    }
    json_to_wip(&json!({
        "entry": format!("{collection}/{subject}"),
        "data": data,
        "links": {
            "tickets": tickets.iter().map(|id| reference(id, ticket_root, ticket_reference(id))).collect::<Vec<_>>(),
            "merge_requests": merge_requests.iter().map(|id| reference(id, Some(collection), is_merge_request_route_reference(id))).collect::<Vec<_>>(),
        }
    })).map_err(WipOperationError::OutcomeUnknown)
}

#[cfg(test)]
mod tests;
