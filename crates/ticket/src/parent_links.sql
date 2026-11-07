CREATE TABLE typed_ticket_parent_links (
    workspace_id TEXT NOT NULL,
    ticket_id TEXT NOT NULL,
    parent_ticket_id TEXT,
    last_operation_key TEXT NOT NULL,
    PRIMARY KEY (workspace_id, ticket_id),
    FOREIGN KEY (workspace_id, ticket_id)
        REFERENCES typed_tickets(workspace_id, ticket_id) ON DELETE CASCADE,
    FOREIGN KEY (workspace_id, parent_ticket_id)
        REFERENCES typed_tickets(workspace_id, ticket_id),
    CHECK (parent_ticket_id IS NULL OR parent_ticket_id <> ticket_id),
    CHECK (length(last_operation_key) > 0)
);
CREATE INDEX typed_ticket_parent_links_children
    ON typed_ticket_parent_links(workspace_id, parent_ticket_id, ticket_id);
CREATE TABLE typed_ticket_parent_operations (
    workspace_id TEXT NOT NULL,
    operation_key TEXT NOT NULL,
    ticket_id TEXT NOT NULL,
    input_json TEXT NOT NULL,
    before_json TEXT NOT NULL,
    result_json TEXT NOT NULL,
    source_context_json TEXT NOT NULL,
    at TEXT NOT NULL,
    PRIMARY KEY (workspace_id, operation_key),
    CHECK (length(operation_key) > 0)
);
