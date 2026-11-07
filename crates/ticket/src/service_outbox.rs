//! Transactional, metadata-only facts for Host-owned Service delivery. No guest is
//! invoked here. Rows deliberately have no Ticket foreign key: deletion must not
//! erase delivery evidence or require resolving the deleted resource.

use rusqlite::{Connection, params};

use crate::{Result, TicketError, now_utc, sqlite_err, sqlite_ticket_content_digest};

/// Compose this schema in the Host's startup migration, not on request paths.
/// Installation creates an empty outbox; historical domain facts are not replayed.
pub const OUTBOX_SCHEMA_SQL: &str = include_str!("service_outbox.sql");

#[derive(Clone, Copy)]
pub(crate) enum EventType {
    Changed,
    Deleted,
}

pub(crate) fn insert(
    connection: &Connection,
    workspace_id: &str,
    ticket_id: &str,
    event_type: EventType,
    occurred_at: &str,
    domain_event_ref: &str,
) -> Result<String> {
    if connection.is_autocommit() {
        return Err(TicketError::Sqlite(
            "Ticket Service outbox insertion requires the domain write transaction".into(),
        ));
    }
    let digest = sqlite_ticket_content_digest(connection, workspace_id, ticket_id)?
        .ok_or_else(|| TicketError::NotFound(ticket_id.into()))?;
    // Identities, rather than arbitrary event attributes, are the only propagation
    // metadata. Never copy author, body, title or a free-form attribute map here.
    let event_id: String = connection
        .query_row("SELECT lower(hex(randomblob(16)))", [], |row| row.get(0))
        .map_err(sqlite_err)?;
    connection
        .execute(
            "INSERT INTO typed_ticket_service_outbox
         (event_id, workspace_id, ticket_id, event_type, occurred_at,
          domain_event_ref, content_digest, correlation_id, causation_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?1, ?6)",
            params![
                event_id,
                workspace_id,
                ticket_id,
                match event_type {
                    EventType::Changed => "changed",
                    EventType::Deleted => "deleted",
                },
                occurred_at,
                domain_event_ref,
                digest
            ],
        )
        .map_err(sqlite_err)?;
    Ok(event_id)
}

/// Record deletion metadata BEFORE removing the Ticket or its canonical initial
/// post/targets, in the SAME write transaction. The Host must supply the durable
/// deletion operation identity as `domain_event_ref`, then perform its fenced
/// delete; any failure must roll back both. This function neither deletes nor
/// commits. It fails closed outside a transaction or for an absent Ticket.
///
/// Direct SQL deletion without this call does not emit an event. There is no SQL
/// trigger/cache fallback: the digest must come from the still-live domain data.
pub fn record_ticket_deletion_in_transaction(
    connection: &Connection,
    workspace_id: &str,
    ticket_id: &str,
    domain_event_ref: &str,
) -> Result<String> {
    insert(
        connection,
        workspace_id,
        ticket_id,
        EventType::Deleted,
        &now_utc(),
        domain_event_ref,
    )
}

#[cfg(test)]
mod tests;
