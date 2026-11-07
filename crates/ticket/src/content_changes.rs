//! Immutable facts of committed edits, not notification decisions. The caller's write
//! transaction owns both the edit and this INSERT; hosts may capture assignment identity
//! with an INSERT trigger. No text snapshots or processing state are retained here.

use rusqlite::{Connection, params};

use crate::{Result, sqlite_err};

pub(crate) enum ContentChange<'a> {
    ContentEdit {
        title_changed: bool,
        body_event_ref: Option<&'a str>,
        targets_changed: bool,
    },
    CommentEdit {
        event_ref: &'a str,
    },
}

impl ContentChange<'_> {
    pub(crate) fn insert(
        self,
        conn: &Connection,
        workspace_id: &str,
        ticket_id: &str,
        actor: Option<&str>,
        at: &str,
    ) -> Result<()> {
        let (kind, title_changed, source_event_ref, targets_changed) = match self {
            Self::ContentEdit {
                title_changed,
                body_event_ref,
                targets_changed,
            } => (
                "content_edit",
                title_changed,
                body_event_ref,
                targets_changed,
            ),
            Self::CommentEdit { event_ref } => ("comment_edit", false, Some(event_ref), false),
        };
        let change_id: String = conn
            .query_row("SELECT lower(hex(randomblob(16)))", [], |row| row.get(0))
            .map_err(sqlite_err)?;
        conn.execute(
            "INSERT INTO typed_ticket_content_changes
             (change_id, workspace_id, ticket_id, kind, actor, at,
              title_changed, body_changed, targets_changed, source_event_ref)
             VALUES (?10, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                workspace_id,
                ticket_id,
                kind,
                actor,
                at,
                title_changed,
                source_event_ref.is_some(),
                targets_changed,
                source_event_ref,
                change_id
            ],
        )
        .map_err(sqlite_err)?;
        crate::service_outbox::insert(
            conn,
            workspace_id,
            ticket_id,
            crate::service_outbox::EventType::Changed,
            at,
            &change_id,
        )?;
        Ok(())
    }
}
