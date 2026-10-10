//! Workspace tag identity, display-name policy, and transactional SQLite authority.
//!
//! Display names use Rust `str::trim` (Unicode whitespace), then Unicode
//! `str::to_lowercase` for active-name uniqueness. No NFC/NFKC normalization or
//! full case folding is performed: canonically equivalent spellings can differ.
//! Any control character in the input is rejected, including trimmed edges.
//! The trimmed display name must be nonempty and at most 128 UTF-8 bytes.
//! IDs are immutable; deleted names may be recreated under a new ID.

use super::*;

#[cfg(test)]
#[path = "tags_tests.rs"]
mod tests;

/// Workspace-scoped immutable identity with a mutable display name.
/// Names are Unicode-whitespace trimmed, limited to 128 UTF-8 bytes, and
/// deduplicated among active tags using Rust Unicode lowercase (not full case
/// folding or NFC/NFKC). Empty names and any input control characters are invalid.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WorkspaceTag {
    pub workspace_id: String,
    pub id: String,
    pub name: String,
    pub last_operation_key: String,
    pub deleted: bool,
}

/// Active tags in ascending ID order, strictly after `after`; default/max 200.
/// Larger limits are clamped to 200; zero returns an empty page.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TagListRequest {
    pub after: Option<String>,
    #[schemars(range(min = 0, max = 200))]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceTagMutation {
    pub operation_key: String,
    pub action: WorkspaceTagAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkspaceTagAction {
    Create {
        name: String,
    },
    Rename {
        tag_id: String,
        expected_last_operation_key: String,
        name: String,
    },
    Delete {
        tag_id: String,
        expected_last_operation_key: String,
    },
}

/// Compare entire active ID set, then replace it atomically. IDs must be unique;
/// input order is immaterial to CAS, but remains significant for exact replay.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketTagSetRequest {
    pub expected_tag_ids: Vec<String>,
    pub tag_ids: Vec<String>,
    pub operation_key: String,
}

fn normalize_name(input: &str) -> Result<(String, String)> {
    let name = input.trim();
    if name.is_empty() || name.len() > 128 || input.chars().any(char::is_control) {
        return Err(TicketError::Conflict("tag name must be nonempty, contain no control characters, and be at most 128 UTF-8 bytes after trimming".into()));
    }
    Ok((name.to_owned(), name.to_lowercase()))
}

fn sorted_ids(ids: &[String]) -> Result<Vec<String>> {
    if ids.len() > 100 || ids.iter().any(|id| id.len() > 128) {
        return Err(TicketError::Conflict(
            "tag sets require at most 100 IDs of at most 128 bytes".into(),
        ));
    }
    let mut sorted = ids.to_vec();
    sorted.sort();
    if sorted.iter().any(|id| id.trim().is_empty())
        || sorted.windows(2).any(|pair| pair[0] == pair[1])
    {
        return Err(TicketError::Conflict(
            "tag IDs must be nonempty and unique".into(),
        ));
    }
    Ok(sorted)
}

fn read_tag(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorkspaceTag> {
    Ok(WorkspaceTag {
        workspace_id: row.get(0)?,
        id: row.get(1)?,
        name: row.get(2)?,
        last_operation_key: row.get(3)?,
        deleted: row.get::<_, i64>(4)? != 0,
    })
}

impl SqliteTicketBackend {
    pub(crate) fn load_ticket_tags(
        &self,
        conn: &Connection,
        ticket_id: &str,
    ) -> Result<Vec<WorkspaceTag>> {
        let mut statement = conn.prepare("SELECT tag.workspace_id,tag.tag_id,tag.name,tag.last_operation_key,tag.deleted FROM workspace_tags AS tag JOIN typed_ticket_tags AS link ON link.workspace_id=tag.workspace_id AND link.tag_id=tag.tag_id WHERE link.workspace_id=?1 AND link.ticket_id=?2 AND tag.deleted=0 ORDER BY tag.tag_id").map_err(sqlite_err)?;
        statement
            .query_map(params![self.workspace_id, ticket_id], read_tag)
            .map_err(sqlite_err)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(sqlite_err)
    }

    fn load_tag(&self, conn: &Connection, id: &str) -> Result<WorkspaceTag> {
        conn.query_row("SELECT workspace_id,tag_id,name,last_operation_key,deleted FROM workspace_tags WHERE workspace_id=?1 AND tag_id=?2", params![self.workspace_id,id], read_tag).optional().map_err(sqlite_err)?.ok_or_else(|| TicketError::NotFound(format!("workspace tag {id}")))
    }

    fn tag_input(&self, kind: &str, target: &str, request: &impl Serialize) -> Result<String> {
        // Bind authenticated identity, not its mutable assignment/role context:
        // a receipt is still the same request after the Worker is reassigned.
        // The complete original context is retained separately in the audit row.
        let identity: BTreeMap<_, _> = self
            .event_attributes
            .iter()
            .filter(|(key, _)| {
                matches!(
                    key.as_str(),
                    "source_account_id"
                        | "source_user_id"
                        | "source_runtime_id"
                        | "source_worker_id"
                )
            })
            .collect();
        serde_json::to_string(&(kind, target, request, identity)).map_err(sqlite_err)
    }

    fn replay_tag_operation<T: serde::de::DeserializeOwned>(
        &self,
        conn: &Connection,
        key: &str,
        input: &str,
    ) -> Result<Option<T>> {
        validate_required_event_value("operation_key", key)?;
        let receipt: Option<(String,String)> = conn.query_row("SELECT input_json,result_json FROM workspace_tag_operations WHERE workspace_id=?1 AND operation_key=?2",params![self.workspace_id,key],|row| Ok((row.get(0)?,row.get(1)?))).optional().map_err(sqlite_err)?;
        match receipt {
            Some((original, result)) if original == input => {
                Ok(Some(serde_json::from_str(&result).map_err(sqlite_err)?))
            }
            Some(_) => Err(TicketError::OperationFingerprintMismatch {
                operation_key: key.to_owned(),
            }),
            None => Ok(None),
        }
    }

    fn record_tag_operation(
        &self,
        conn: &Connection,
        key: &str,
        kind: &str,
        target: &str,
        input: &str,
        snapshots: (&impl Serialize, &impl Serialize),
    ) -> Result<()> {
        conn.execute("INSERT INTO workspace_tag_operations (workspace_id,operation_key,kind,target_id,input_json,before_json,result_json,at,source_context_json) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)", params![self.workspace_id,key,kind,target,input,serde_json::to_string(snapshots.0).map_err(sqlite_err)?,serde_json::to_string(snapshots.1).map_err(sqlite_err)?,now_utc(),serde_json::to_string(&self.event_attributes).map_err(sqlite_err)?]).map_err(sqlite_err)?;
        Ok(())
    }

    fn audit_ticket_tags(
        &self,
        conn: &Connection,
        ticket_id: &str,
        key: &str,
        before: &[WorkspaceTag],
        after: &[WorkspaceTag],
    ) -> Result<()> {
        self.insert_event(
            conn,
            ticket_id,
            &TicketEvent {
                kind: TicketEventKind::TagsChanged,
                author: Some(default_author()),
                at: Some(now_utc()),
                status: None,
                from: None,
                to: None,
                reason: None,
                state_field: None,
                heading: Some("Tags changed".into()),
                body: MarkdownText::new("Ticket tags changed"),
                references: Vec::new(),
                attributes: BTreeMap::from([
                    ("operation_key".into(), key.to_owned()),
                    (
                        "tags_before".into(),
                        serde_json::to_string(before).map_err(sqlite_err)?,
                    ),
                    (
                        "tags_after".into(),
                        serde_json::to_string(after).map_err(sqlite_err)?,
                    ),
                ]),
            },
        )
    }

    pub(crate) fn list_tags(&self, request: TagListRequest) -> Result<Vec<WorkspaceTag>> {
        self.with_read(|conn| {
            let mut statement = conn.prepare("SELECT workspace_id,tag_id,name,last_operation_key,deleted FROM workspace_tags WHERE workspace_id=?1 AND deleted=0 AND (?2 IS NULL OR tag_id>?2) ORDER BY tag_id LIMIT ?3").map_err(sqlite_err)?;
            statement.query_map(params![self.workspace_id,request.after,request.limit.unwrap_or(200).min(200) as i64],read_tag).map_err(sqlite_err)?.collect::<std::result::Result<Vec<_>,_>>().map_err(sqlite_err)
        })
    }

    pub(crate) fn mutate_tag(&self, request: WorkspaceTagMutation) -> Result<WorkspaceTag> {
        self.with_write(|conn| {
            let input = self.tag_input("mutate_workspace_tag", "", &request)?;
            if let Some(result) = self.replay_tag_operation(conn,&request.operation_key,&input)? { return Ok(result); }
            let (before, result, kind) = match &request.action {
                WorkspaceTagAction::Create { name } => {
                    let (name, normalized) = normalize_name(name)?;
                    self.require_unique_tag_name(conn,&normalized,None)?;
                    let millis = unix_epoch_millis_now().map_err(|error| TicketError::Conflict(format!("failed to read tag ID timestamp: {error}")))?;
                    let id = allocate_record_id(millis, |candidate| {
                        conn.query_row("SELECT 1 FROM workspace_tags WHERE workspace_id=?1 AND tag_id=?2",params![self.workspace_id,candidate],|_| Ok(())).optional().map(|row| row.is_some()).unwrap_or(true)
                    }).map_err(|error| TicketError::Conflict(error.to_string()))?;
                    let tag = WorkspaceTag { workspace_id: self.workspace_id.clone(), id, name, last_operation_key: request.operation_key.clone(), deleted: false };
                    conn.execute("INSERT INTO workspace_tags (workspace_id,tag_id,name,normalized_name,last_operation_key,deleted) VALUES (?1,?2,?3,?4,?5,0)",params![tag.workspace_id,tag.id,tag.name,normalized,tag.last_operation_key]).map_err(sqlite_err)?;
                    (None,tag,"create")
                }
                WorkspaceTagAction::Rename { tag_id,expected_last_operation_key,.. } | WorkspaceTagAction::Delete { tag_id,expected_last_operation_key } => {
                    let previous = self.load_tag(conn,tag_id)?;
                    if previous.deleted || &previous.last_operation_key != expected_last_operation_key {
                        return Err(TicketError::Conflict(format!("stale or deleted tag {tag_id}: expected operation {expected_last_operation_key:?}, actual {:?}",previous.last_operation_key)));
                    }
                    let mut tag = previous.clone();
                    tag.last_operation_key = request.operation_key.clone();
                    let kind = if let WorkspaceTagAction::Rename { name,.. } = &request.action {
                        let (name,normalized) = normalize_name(name)?;
                        self.require_unique_tag_name(conn,&normalized,Some(tag_id))?;
                        tag.name = name;
                        conn.execute("UPDATE workspace_tags SET name=?3,normalized_name=?4,last_operation_key=?5 WHERE workspace_id=?1 AND tag_id=?2",params![self.workspace_id,tag_id,tag.name,normalized,request.operation_key]).map_err(sqlite_err)?;
                        "rename"
                    } else {
                        // Capture active snapshots before tombstoning; links and audit are
                        // cleared/written under the same immediate lock as attachments.
                        let ticket_ids = {
                            let mut stmt = conn.prepare("SELECT ticket_id FROM typed_ticket_tags WHERE workspace_id=?1 AND tag_id=?2 ORDER BY ticket_id").map_err(sqlite_err)?;
                            stmt.query_map(params![self.workspace_id,tag_id],|row| row.get::<_,String>(0)).map_err(sqlite_err)?.collect::<std::result::Result<Vec<_>,_>>().map_err(sqlite_err)?
                        };
                        for ticket_id in ticket_ids {
                            let before = self.load_ticket_tags(conn,&ticket_id)?;
                            let after: Vec<_> = before.iter().filter(|tag| &tag.id != tag_id).cloned().collect();
                            conn.execute("DELETE FROM typed_ticket_tags WHERE workspace_id=?1 AND ticket_id=?2 AND tag_id=?3",params![self.workspace_id,ticket_id,tag_id]).map_err(sqlite_err)?;
                            self.audit_ticket_tags(conn,&ticket_id,&request.operation_key,&before,&after)?;
                        }
                        tag.deleted = true;
                        conn.execute("UPDATE workspace_tags SET deleted=1,last_operation_key=?3 WHERE workspace_id=?1 AND tag_id=?2",params![self.workspace_id,tag_id,request.operation_key]).map_err(sqlite_err)?;
                        "delete"
                    };
                    (Some(previous),tag,kind)
                }
            };
            self.record_tag_operation(conn,&request.operation_key,kind,&result.id,&input,(&before,&result))?;
            Ok(result)
        })
    }

    fn require_unique_tag_name(
        &self,
        conn: &Connection,
        normalized: &str,
        except: Option<&str>,
    ) -> Result<()> {
        let duplicate = conn.query_row("SELECT 1 FROM workspace_tags WHERE workspace_id=?1 AND normalized_name=?2 AND deleted=0 AND (?3 IS NULL OR tag_id<>?3)",params![self.workspace_id,normalized,except],|_| Ok(())).optional().map_err(sqlite_err)?.is_some();
        if duplicate {
            return Err(TicketError::Conflict(
                "active workspace tag name already exists".into(),
            ));
        }
        Ok(())
    }

    pub(crate) fn set_tags(
        &self,
        id: TicketIdOrSlug,
        request: TicketTagSetRequest,
    ) -> Result<Vec<WorkspaceTag>> {
        self.with_write(|conn| {
            // Preserve exact selector input, not just its resolved identity.
            let input = self.tag_input("set_ticket_tags", "", &(&id,&request))?;
            if let Some(result) = self.replay_tag_operation(conn,&request.operation_key,&input)? { return Ok(result); }
            let ticket_id = self.resolve_ticket_id(conn,id)?;
            self.require_active_source_assignment(conn,&ticket_id)?;
            let before = self.load_ticket_tags(conn,&ticket_id)?;
            let expected = sorted_ids(&request.expected_tag_ids)?;
            let desired = sorted_ids(&request.tag_ids)?;
            if before.iter().map(|tag| tag.id.clone()).collect::<Vec<_>>() != expected {
                return Err(TicketError::Conflict("stale Ticket tag set".into()));
            }
            let after = desired.iter().map(|id| {
                let tag = self.load_tag(conn,id)?;
                if tag.deleted { return Err(TicketError::Conflict(format!("workspace tag {id} is deleted"))); }
                Ok(tag)
            }).collect::<Result<Vec<_>>>()?;
            conn.execute("DELETE FROM typed_ticket_tags WHERE workspace_id=?1 AND ticket_id=?2",params![self.workspace_id,ticket_id]).map_err(sqlite_err)?;
            for tag in &after {
                conn.execute("INSERT INTO typed_ticket_tags (workspace_id,ticket_id,tag_id) VALUES (?1,?2,?3)",params![self.workspace_id,ticket_id,tag.id]).map_err(sqlite_err)?;
            }
            if before != after { self.audit_ticket_tags(conn,&ticket_id,&request.operation_key,&before,&after)?; }
            self.record_tag_operation(conn,&request.operation_key,"set_ticket_tags",&ticket_id,&input,(&before,&after))?;
            Ok(after)
        })
    }
}
