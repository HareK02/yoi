-- Ordered component 11 -> 12. Existing event indices and child rows are never rewritten.
ALTER TABLE typed_tickets ADD COLUMN created_by TEXT;
ALTER TABLE typed_ticket_events ADD COLUMN event_ref TEXT NOT NULL DEFAULT '';
ALTER TABLE typed_ticket_events ADD COLUMN edited_at TEXT;
ALTER TABLE typed_ticket_events ADD COLUMN edited_by TEXT;
ALTER TABLE typed_ticket_events ADD COLUMN archived INTEGER NOT NULL DEFAULT 0 CHECK (archived IN (0, 1));
-- The retained SQLite loader published ticket_id:event_index even when a raw
-- event_id attribute existed. Preserve that identity, not a display ordinal or
-- the unpublished attribute; leave the stored attributes untouched as evidence.
UPDATE typed_ticket_events SET event_ref = ticket_id || ':' || event_index;
-- A retained row at the reserved initial index fails the enclosing transaction;
-- never silently rename its published identity or move its child rows.
CREATE UNIQUE INDEX typed_ticket_events_ref ON typed_ticket_events(workspace_id, ticket_id, event_ref);
UPDATE typed_tickets SET created_by = COALESCE((
    SELECT CASE WHEN trim(author) = '' THEN NULL ELSE author END FROM typed_ticket_events e
    WHERE e.workspace_id = typed_tickets.workspace_id AND e.ticket_id = typed_tickets.ticket_id
      AND e.kind = 'create' ORDER BY e.event_index LIMIT 1
), (
    SELECT CASE WHEN trim(value) = '' THEN NULL ELSE value END FROM typed_ticket_raw_frontmatter m
    WHERE m.workspace_id = typed_tickets.workspace_id AND m.ticket_id = typed_tickets.ticket_id
      AND m.key = 'created_by'
));
INSERT INTO typed_ticket_events (workspace_id, ticket_id, event_index, event_ref, kind, author, at, body)
SELECT t.workspace_id, t.ticket_id, -1, t.ticket_id || ':-1', 'comment', t.created_by,
    COALESCE((SELECT at FROM typed_ticket_events e
      WHERE e.workspace_id = t.workspace_id AND e.ticket_id = t.ticket_id AND e.kind = 'create'
      ORDER BY e.event_index LIMIT 1), t.created_at), t.body
FROM typed_tickets t;
-- A later title/target edit is not an edit of the current initial-post body.
-- Take both edit fields from the same last known body-edit record. Missing actor
-- or time stays unknown rather than falling back to the creator or an older edit.
WITH body_edits AS (
    SELECT e.workspace_id, e.ticket_id, e.at, e.author,
        ROW_NUMBER() OVER (
            PARTITION BY e.workspace_id, e.ticket_id ORDER BY e.event_index DESC
        ) AS recency
    FROM typed_ticket_events e
    JOIN typed_ticket_event_attributes a
      ON a.workspace_id = e.workspace_id AND a.ticket_id = e.ticket_id
     AND a.event_index = e.event_index AND a.key = 'changes'
    WHERE e.kind = 'item_edit' AND instr(',' || a.value || ',', ',body,') > 0
)
UPDATE typed_ticket_events SET (edited_at, edited_by) = (
    SELECT at, CASE WHEN trim(author) = '' THEN NULL ELSE author END FROM body_edits e
    WHERE e.workspace_id = typed_ticket_events.workspace_id
      AND e.ticket_id = typed_ticket_events.ticket_id AND e.recency = 1
) WHERE event_index = -1;
-- Retain the original records, attributes, references and stable identities as archive history.
UPDATE typed_ticket_events SET archived = 1 WHERE kind IN ('create', 'item_edit');
ALTER TABLE typed_tickets DROP COLUMN body;
