-- Frozen standalone component8 tag extension; combine with schema7.sql.
-- Workspace tag foundation (v8): shared by fresh schema and v7 upgrade.
CREATE TABLE workspace_tags (
    workspace_id TEXT NOT NULL,
    tag_id TEXT NOT NULL,
    name TEXT NOT NULL,
    normalized_name TEXT NOT NULL,
    last_operation_key TEXT NOT NULL,
    deleted INTEGER NOT NULL CHECK (deleted IN (0, 1)),
    PRIMARY KEY (workspace_id, tag_id)
);
CREATE UNIQUE INDEX workspace_tags_active_name
    ON workspace_tags(workspace_id, normalized_name) WHERE deleted = 0;
CREATE TABLE typed_ticket_tags (
    workspace_id TEXT NOT NULL,
    ticket_id TEXT NOT NULL,
    tag_id TEXT NOT NULL,
    PRIMARY KEY (workspace_id, ticket_id, tag_id),
    FOREIGN KEY (workspace_id, ticket_id) REFERENCES typed_tickets(workspace_id, ticket_id) ON DELETE CASCADE,
    FOREIGN KEY (workspace_id, tag_id) REFERENCES workspace_tags(workspace_id, tag_id) ON DELETE RESTRICT
);
CREATE INDEX typed_ticket_tags_workspace_tag
    ON typed_ticket_tags(workspace_id, tag_id, ticket_id);
-- Immutable operation receipt and Workspace audit: exact input includes actor
-- attributes; before/result preserve stable identities and display-name snapshots.
-- No foreign keys to mutable rows: history survives deletion of current links.
CREATE TABLE workspace_tag_operations (
    workspace_id TEXT NOT NULL,
    operation_key TEXT NOT NULL,
    kind TEXT NOT NULL,
    target_id TEXT NOT NULL,
    input_json TEXT NOT NULL,
    source_context_json TEXT NOT NULL,
    before_json TEXT NOT NULL,
    result_json TEXT NOT NULL,
    at TEXT NOT NULL,
    PRIMARY KEY (workspace_id, operation_key)
);
