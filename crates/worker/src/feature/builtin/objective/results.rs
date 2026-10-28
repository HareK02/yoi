//! Native result references are coordinates, not observations or authority.
//!
//! `data` preserves the bounded Model projection, including bodies/events and their identifiers;
//! only the explicit subject and linked-Ticket fields supply navigable references. Query paging is
//! separate from item data, avoiding duplicate copies of the same bounded items. Related Entries use
//! only the current call context's committed published result namespaces, not mount-time allocations.
//! Unpublished spaces retain display IDs but omit Entries. No result construction resolves a related
//! resource, so a later denied/missing read cannot turn a successful mutation into a failed operation
//! or cause a replay.

use super::*;
use wip_protocol::FieldDeclaration;

#[derive(Clone)]
pub(super) struct Routes {
    pub objective: String,
}

fn field(name: &str, required: bool, ty: TypeExpr) -> FieldDeclaration {
    FieldDeclaration {
        name: name.into(),
        required,
        documentation: None,
        r#type: ty,
    }
}

pub(super) fn item_type() -> TypeExpr {
    TypeExpr::Record {
        fields: vec![
            field("data", true, TypeExpr::Json),
            field("entry", true, TypeExpr::Entry),
            field(
                "links",
                true,
                TypeExpr::Record {
                    fields: vec![field(
                        "tickets",
                        true,
                        list_of(TypeExpr::Record {
                            fields: vec![
                                field("id", true, TypeExpr::String),
                                field("entry", false, TypeExpr::Entry),
                            ],
                        }),
                    )],
                },
            ),
        ],
    }
}

pub(super) fn query_type() -> TypeExpr {
    TypeExpr::Record {
        fields: vec![
            field("items", true, list_of(item_type())),
            field("next_cursor", false, TypeExpr::String),
            field("has_more", true, TypeExpr::Boolean),
        ],
    }
}

pub(super) fn declaration(query: bool) -> ReturnDeclaration {
    ReturnDeclaration {
        documentation: Some(Documentation {
            summary: if query {
                "Bounded Objective items with typed references and separate paging metadata"
            } else {
                "Objective data, subject Entry, and explicit linked-Ticket coordinates"
            }
            .into(),
            details: Some(
                "Inspect entry, then its Interface before read. data is bounded domain JSON, not a reference contract. links.tickets[].entry is omitted when the Ticket result namespace is unpublished or the ID is unroutable. Namespace allocation alone is not publication. Publication does not attest existence, visibility or permission: selected reads may be denied or return NotFound. Follow-up failures never require replay of a successful mutation."
                    .into(),
            ),
        }),
        r#type: if query { query_type() } else { item_type() },
    }
}

impl Routes {
    pub(super) fn query(
        &self,
        response: &Json,
        ticket_root: Option<&str>,
    ) -> Result<Value, WipOperationError> {
        let items = response["objectives"]
            .as_array()
            .ok_or_else(|| invalid_projection("missing Model Objective query items"))?
            .iter()
            .map(|item| self.item(item, true, ticket_root))
            .collect::<Result<Vec<_>, _>>()?;
        let mut fields = BTreeMap::from([
            ("items".into(), Value::List(items)),
            (
                "has_more".into(),
                Value::Boolean(response["has_more"].as_bool().ok_or_else(|| {
                    invalid_projection("missing Model Objective query paging metadata")
                })?),
            ),
        ]);
        if let Some(cursor) = response["next_cursor"].as_str() {
            fields.insert("next_cursor".into(), Value::String(cursor.into()));
        }
        Ok(Value::Record(fields))
    }

    pub(super) fn detail(
        &self,
        response: &Json,
        ticket_root: Option<&str>,
    ) -> Result<Value, WipOperationError> {
        self.item(response, false, ticket_root)
    }

    fn item(
        &self,
        data: &Json,
        query: bool,
        ticket_root: Option<&str>,
    ) -> Result<Value, WipOperationError> {
        let id = objective_ref(data).map_err(invalid_projection)?;
        let tickets = data["linked_tickets"]
            .as_array()
            .ok_or_else(|| invalid_projection("missing Model Objective linked Tickets"))?
            .iter()
            .filter_map(|ticket| {
                // These are the two explicit Model fields, never arbitrary nested JSON/paths/text.
                if query {
                    ticket.as_str()
                } else {
                    ticket["ticket"].as_str()
                }
            })
            .map(|id| {
                let mut fields = BTreeMap::from([("id".into(), Value::String(id.into()))]);
                if let Some(root) = ticket_root
                    && is_canonical_resource_key(id, "T-")
                {
                    fields.insert("entry".into(), Value::String(format!("{root}/{id}")));
                }
                Value::Record(fields)
            })
            .collect();
        Ok(Value::Record(BTreeMap::from([
            (
                "data".into(),
                json_to_wip(data).map_err(WipOperationError::OutcomeUnknown)?,
            ),
            (
                "entry".into(),
                Value::String(format!("{}/{id}", self.objective)),
            ),
            (
                "links".into(),
                Value::Record(BTreeMap::from([("tickets".into(), Value::List(tickets))])),
            ),
        ])))
    }
}

fn invalid_projection(message: impl Into<String>) -> WipOperationError {
    WipOperationError::OutcomeUnknown(message.into())
}
