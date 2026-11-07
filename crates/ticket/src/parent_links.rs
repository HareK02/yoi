//! Work decomposition between ordinary Tickets, independent of dependencies, case
//! judgments, assignments and grants. Only the child's parent link is authoritative;
//! children are a bounded reverse query, never another independently maintained list.
//!
//! A null parent retains its last mutation key. Parent identity alone cannot fence
//! A -> B -> A or link -> unlink races. Exact replay binds the original selector,
//! complete request and authenticated identity, returning the historical receipt,
//! not fresh observation. Reconcile uncertain outcomes by rereading and retaining
//! the same key/input for an exact retry; never mint a new operation blindly.
//!
//! All writers use SQLite's immediate transaction before CAS and the entire ancestor
//! walk. Serializing only the child row would permit concurrent reciprocal links.
//! The walk has no depth cutoff; a corrupt pre-existing cycle also fails closed.
//! Authorization belongs to the Host and must run before these storage methods.

use super::*;

#[cfg(test)]
#[path = "parent_links_tests.rs"]
mod tests;

pub(crate) const SCHEMA_SQL: &str = include_str!("parent_links.sql");

// Schemars' required validator unwraps Option unless schema generation stays explicit.
// Wire presence and nullability are independent: null is an observed domain value.
fn nullable_schema<T: schemars::JsonSchema>(
    generator: &mut schemars::SchemaGenerator,
) -> schemars::Schema {
    <Option<T> as schemars::JsonSchema>::json_schema(generator)
}

fn deserialize_observed_parent<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<String>, D::Error> {
    Option::<String>::deserialize(deserializer)
}

fn deserialize_desired_parent<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<String>, D::Error> {
    Option::<String>::deserialize(deserializer)
}

/// CAS observation. Both nullable fields must be explicitly observed on the wire.
/// A never-mutated Ticket has null/null; unlink preserves a non-null operation key.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketParentObservation {
    #[serde(deserialize_with = "deserialize_observed_parent")]
    #[schemars(required, schema_with = "nullable_schema::<String>")]
    pub parent_ticket_id: Option<String>,
    #[serde(deserialize_with = "deserialize_observed_parent")]
    #[schemars(required, schema_with = "nullable_schema::<String>")]
    pub last_operation_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketParentSetRequest {
    pub expected: TicketParentObservation,
    /// Null unlinks; references are resolved only inside the scoped write transaction.
    #[serde(deserialize_with = "deserialize_desired_parent")]
    #[schemars(required, schema_with = "nullable_schema::<String>")]
    pub parent: Option<String>,
    pub operation_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TicketParentLink {
    pub ticket_id: String,
    pub observation: TicketParentObservation,
    #[schemars(required, schema_with = "nullable_schema::<TicketLinkSummary>")]
    pub parent: Option<TicketLinkSummary>,
}

/// A bounded navigation projection, never a progress/fulfillment aggregate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TicketLinkSummary {
    pub id: String,
    pub resource_key: String,
    pub title: String,
    pub state: TicketState,
    #[schemars(required, schema_with = "nullable_schema::<TicketClosureReason>")]
    pub closure_reason: Option<TicketClosureReason>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TicketParentFilter {
    None {},
    Parent { ticket: String },
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ChildrenCursor {
    workspace_id: String,
    parent_ticket_id: String,
    after: String,
}

/// Live keyset page: ascending immutable child ID, strictly after its cursor.
/// Concurrent inserts/reparents may change later pages; this is not a tree snapshot.
/// The cursor is bound to the Workspace and parent and rejected in another scope.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketChildrenRequest {
    pub cursor: Option<String>,
    #[schemars(range(min = 1, max = 100))]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TicketChildrenPage {
    pub parent_ticket_id: String,
    pub items: Vec<TicketLinkSummary>,
    #[schemars(required, schema_with = "nullable_schema::<String>")]
    pub next_cursor: Option<String>,
    pub has_more: bool,
}

impl SqliteTicketBackend {
    /// Read only this Ticket's link metadata, without its body or relatives.
    /// Callers authorize the addressed Ticket before exposing the projection.
    pub fn read_link_summary(&self, id: TicketIdOrSlug) -> Result<TicketLinkSummary> {
        self.with_parent_read(|conn| {
            let id = self.resolve_parent_link_ticket(conn, id)?;
            self.load_link_summary(conn, &id)
        })
    }

    fn with_parent_read<R>(&self, op: impl FnOnce(&Connection) -> Result<R>) -> Result<R> {
        let conn = self.open_connection()?;
        conn.execute_batch("BEGIN").map_err(sqlite_err)?;
        finish_sqlite_transaction(&conn, op(&conn))
    }

    fn load_link_summary(&self, conn: &Connection, id: &str) -> Result<TicketLinkSummary> {
        let (title, state, reason): (String, String, Option<String>) = conn
            .query_row(
                "SELECT t.title,t.workflow_state,
             CASE WHEN t.workflow_state='closed' THEN (
                 SELECT json_extract(h.judgment_json,'$.action.reason')
                 FROM typed_ticket_case_metadata m JOIN typed_ticket_case_history h
                 ON h.workspace_id=m.workspace_id AND h.ticket_id=m.ticket_id
                 AND h.operation_key=m.closure_operation_key
                 WHERE m.workspace_id=t.workspace_id AND m.ticket_id=t.ticket_id
             ) ELSE NULL END
             FROM typed_tickets t WHERE t.workspace_id=?1 AND t.ticket_id=?2",
                params![self.workspace_id, id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(sqlite_err)?
            .ok_or_else(|| TicketError::NotFound(id.into()))?;
        // SQLite returns JSON strings without quotes but returns object reasons as
        // JSON text. Interpret exactly the persisted closure reason, never closed=success.
        let closure_reason = reason
            .map(|reason| {
                let value = if reason.starts_with('{') {
                    serde_json::from_str(&reason).map_err(sqlite_err)?
                } else {
                    serde_json::Value::String(reason)
                };
                serde_json::from_value(value).map_err(sqlite_err)
            })
            .transpose()?;
        let state = match state.as_str() {
            "open" => TicketState::Open,
            "closed" => TicketState::Closed,
            _ => return Err(TicketError::Conflict("invalid Ticket case state".into())),
        };
        let resource_key = Self::resource_key_for(conn, &self.workspace_id, id)?
            .ok_or_else(|| TicketError::Conflict("linked Ticket has no canonical key".into()))?;
        Ok(TicketLinkSummary {
            id: id.into(),
            resource_key,
            title,
            state,
            closure_reason,
        })
    }

    fn parent_projection(
        &self,
        conn: &Connection,
        ticket_id: String,
        observation: TicketParentObservation,
    ) -> Result<TicketParentLink> {
        let parent = observation
            .parent_ticket_id
            .as_deref()
            .map(|id| self.load_link_summary(conn, id))
            .transpose()?;
        Ok(TicketParentLink {
            ticket_id,
            observation,
            parent,
        })
    }

    fn resolve_parent_link_ticket(&self, conn: &Connection, id: TicketIdOrSlug) -> Result<String> {
        let ticket_id = self.resolve_ticket_id(conn, id)?;
        if !self.ticket_exists(conn, &ticket_id)? {
            return Err(TicketError::NotFound(ticket_id));
        }
        Ok(ticket_id)
    }

    fn load_parent_observation(
        &self,
        conn: &Connection,
        ticket_id: &str,
    ) -> Result<TicketParentObservation> {
        conn.query_row(
            "SELECT parent_ticket_id, last_operation_key FROM typed_ticket_parent_links
             WHERE workspace_id=?1 AND ticket_id=?2",
            params![self.workspace_id, ticket_id],
            |row| {
                Ok(TicketParentObservation {
                    parent_ticket_id: row.get(0)?,
                    last_operation_key: Some(row.get(1)?),
                })
            },
        )
        .optional()
        .map_err(sqlite_err)
        .map(Option::unwrap_or_default)
    }

    pub(crate) fn parent_link(&self, id: TicketIdOrSlug) -> Result<TicketParentLink> {
        self.with_parent_read(|conn| {
            let ticket_id = self.resolve_parent_link_ticket(conn, id)?;
            let observation = self.load_parent_observation(conn, &ticket_id)?;
            self.parent_projection(conn, ticket_id, observation)
        })
    }

    pub(crate) fn children_page(
        &self,
        parent: TicketIdOrSlug,
        request: TicketChildrenRequest,
    ) -> Result<TicketChildrenPage> {
        let limit = request.limit.unwrap_or(50);
        if !(1..=100).contains(&limit) {
            return Err(TicketError::Conflict(
                "children limit must be 1..=100".into(),
            ));
        }
        self.with_parent_read(|conn| {
            let parent_ticket_id = self.resolve_parent_link_ticket(conn, parent)?;
            let after = request
                .cursor
                .as_deref()
                .map(|cursor| {
                    if cursor.len() > 4096 {
                        return Err(TicketError::Conflict("invalid children cursor".into()));
                    }
                    let cursor: ChildrenCursor = serde_json::from_str(cursor)
                        .map_err(|_| TicketError::Conflict("invalid children cursor".into()))?;
                    if cursor.workspace_id != self.workspace_id
                        || cursor.parent_ticket_id != parent_ticket_id
                    {
                        return Err(TicketError::Conflict(
                            "children cursor belongs to another Workspace or parent".into(),
                        ));
                    }
                    Ok(cursor.after)
                })
                .transpose()?;
            let mut stmt = conn
                .prepare(
                    "SELECT ticket_id FROM typed_ticket_parent_links
                 WHERE workspace_id=?1 AND parent_ticket_id=?2
                 AND (?3 IS NULL OR ticket_id>?3) ORDER BY ticket_id LIMIT ?4",
                )
                .map_err(sqlite_err)?;
            let mut ticket_ids = stmt
                .query_map(
                    params![
                        self.workspace_id,
                        parent_ticket_id,
                        after,
                        (limit + 1) as i64
                    ],
                    |row| row.get::<_, String>(0),
                )
                .map_err(sqlite_err)?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(sqlite_err)?;
            let has_more = ticket_ids.len() > limit;
            ticket_ids.truncate(limit);
            let next_cursor = if has_more {
                Some(
                    serde_json::to_string(&ChildrenCursor {
                        workspace_id: self.workspace_id.clone(),
                        parent_ticket_id: parent_ticket_id.clone(),
                        after: ticket_ids.last().expect("nonempty bounded page").clone(),
                    })
                    .map_err(sqlite_err)?,
                )
            } else {
                None
            };
            let items = ticket_ids
                .iter()
                .map(|id| self.load_link_summary(conn, id))
                .collect::<Result<Vec<_>>>()?;
            Ok(TicketChildrenPage {
                parent_ticket_id,
                items,
                next_cursor,
                has_more,
            })
        })
    }

    fn require_acyclic_parent(
        &self,
        conn: &Connection,
        child: &str,
        parent: Option<&str>,
    ) -> Result<()> {
        let mut ancestor = parent.map(str::to_owned);
        let mut visited = BTreeSet::new();
        let mut stmt = conn
            .prepare(
                "SELECT parent_ticket_id FROM typed_ticket_parent_links
             WHERE workspace_id=?1 AND ticket_id=?2",
            )
            .map_err(sqlite_err)?;
        while let Some(id) = ancestor {
            if id == child || !visited.insert(id.clone()) {
                return Err(TicketError::Conflict(
                    "Ticket parent link would contain a cycle".into(),
                ));
            }
            ancestor = stmt
                .query_row(params![self.workspace_id, id], |row| {
                    row.get::<_, Option<String>>(0)
                })
                .optional()
                .map_err(sqlite_err)?
                .flatten();
        }
        Ok(())
    }

    pub(crate) fn set_parent_link(
        &self,
        id: TicketIdOrSlug,
        request: TicketParentSetRequest,
    ) -> Result<TicketParentLink> {
        validate_required_event_value("operation_key", &request.operation_key)?;
        self.with_write(|conn| {
            // Mutable assignment/role context is audit data, not replay identity.
            let identity: BTreeMap<_, _> = self.event_attributes.iter().filter(|(key, _)| {
                matches!(key.as_str(), "source_account_id" | "source_user_id" | "source_runtime_id" | "source_worker_id")
            }).collect();
            let input = serde_json::to_string(&(&id, &request, identity)).map_err(sqlite_err)?;
            let saved: Option<(String, String)> = conn.query_row(
                "SELECT input_json,result_json FROM typed_ticket_parent_operations
                 WHERE workspace_id=?1 AND operation_key=?2",
                params![self.workspace_id, request.operation_key],
                |row| Ok((row.get(0)?, row.get(1)?)),
            ).optional().map_err(sqlite_err)?;
            if let Some((original, result)) = saved {
                return if original == input {
                    serde_json::from_str(&result).map_err(sqlite_err)
                } else {
                    Err(TicketError::OperationFingerprintMismatch { operation_key: request.operation_key.clone() })
                };
            }
            let ticket_id = self.resolve_parent_link_ticket(conn, id)?;
            self.require_active_source_assignment(conn, &ticket_id)?;
            let before = self.load_parent_observation(conn, &ticket_id)?;
            if before != request.expected {
                return Err(TicketError::Conflict("stale Ticket parent observation; reread before editing".into()));
            }
            let parent_ticket_id = request.parent.clone().map(|parent| self.resolve_parent_link_ticket(conn, TicketIdOrSlug::Query(parent))).transpose()?;
            self.require_acyclic_parent(conn, &ticket_id, parent_ticket_id.as_deref())?;
            let changed = before.parent_ticket_id != parent_ticket_id;
            let observation = if changed {
                TicketParentObservation { parent_ticket_id, last_operation_key: Some(request.operation_key.clone()) }
            } else {
                before.clone()
            };
            let at = now_utc();
            if changed {
                conn.execute(
                    "INSERT INTO typed_ticket_parent_links (workspace_id,ticket_id,parent_ticket_id,last_operation_key)
                     VALUES (?1,?2,?3,?4) ON CONFLICT (workspace_id,ticket_id) DO UPDATE
                     SET parent_ticket_id=excluded.parent_ticket_id,last_operation_key=excluded.last_operation_key",
                    params![self.workspace_id, ticket_id, observation.parent_ticket_id, request.operation_key],
                ).map_err(sqlite_err)?;
                self.insert_event(conn, &ticket_id, &TicketEvent {
                    event_ref: String::new(), is_initial: false, edited_at: None, edited_by: None,
                    kind: TicketEventKind::ParentChanged,
                    author: Some(default_author()), at: Some(at.clone()), status: None,
                    from: None, to: None, reason: None, state_field: None,
                    heading: Some("Parent changed".into()), body: MarkdownText::new("Ticket parent changed"),
                    references: Vec::new(),
                    attributes: BTreeMap::from([
                        ("operation_key".into(), request.operation_key.clone()),
                        ("parent_before".into(), serde_json::to_string(&before).map_err(sqlite_err)?),
                        ("parent_after".into(), serde_json::to_string(&observation).map_err(sqlite_err)?),
                    ]),
                })?;
                self.touch_ticket(conn, &ticket_id, &at)?;
            }
            let result = self.parent_projection(conn, ticket_id, observation)?;
            conn.execute(
                "INSERT INTO typed_ticket_parent_operations
                 (workspace_id,operation_key,ticket_id,input_json,before_json,result_json,source_context_json,at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
                params![self.workspace_id, request.operation_key, result.ticket_id, input,
                    serde_json::to_string(&before).map_err(sqlite_err)?,
                    serde_json::to_string(&result).map_err(sqlite_err)?,
                    serde_json::to_string(&self.event_attributes).map_err(sqlite_err)?, at],
            ).map_err(sqlite_err)?;
            Ok(result)
        })
    }
}
