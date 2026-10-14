-- Ordered component 12 -> 13. Facts have no delivery policy or mutable processing state.
CREATE TABLE typed_ticket_content_changes (
    change_id TEXT NOT NULL PRIMARY KEY CHECK (length(trim(change_id)) > 0),
    workspace_id TEXT NOT NULL,
    ticket_id TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('content_edit', 'comment_edit')),
    actor TEXT CHECK (actor IS NULL OR length(trim(actor)) > 0),
    at TEXT NOT NULL,
    title_changed INTEGER NOT NULL CHECK (title_changed IN (0, 1)),
    body_changed INTEGER NOT NULL CHECK (body_changed IN (0, 1)),
    targets_changed INTEGER NOT NULL CHECK (targets_changed IN (0, 1)),
    source_event_ref TEXT,
    CHECK (title_changed + body_changed + targets_changed > 0),
    CHECK ((body_changed = 0 AND source_event_ref IS NULL) OR
           (body_changed = 1 AND source_event_ref IS NOT NULL AND length(trim(source_event_ref)) > 0)),
    CHECK (kind != 'comment_edit' OR (title_changed = 0 AND body_changed = 1 AND targets_changed = 0)),
    FOREIGN KEY (workspace_id, ticket_id) REFERENCES typed_tickets(workspace_id, ticket_id) ON DELETE CASCADE
);
CREATE TRIGGER typed_ticket_content_changes_no_update BEFORE UPDATE ON typed_ticket_content_changes
BEGIN SELECT RAISE(ABORT, 'Ticket content change facts are immutable'); END;
