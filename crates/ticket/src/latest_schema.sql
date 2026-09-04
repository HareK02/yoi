-- Canonical standalone Ticket schema. Workspace Server composes stricter cross-domain authority.
CREATE TABLE typed_ticket_artifacts (
    workspace_id TEXT NOT NULL, ticket_id TEXT NOT NULL, relative_path TEXT NOT NULL, content BLOB NOT NULL,
    PRIMARY KEY (workspace_id, ticket_id, relative_path),
    FOREIGN KEY (workspace_id, ticket_id) REFERENCES typed_tickets(workspace_id, ticket_id) ON DELETE CASCADE
);
CREATE TABLE typed_ticket_event_attributes (
    workspace_id TEXT NOT NULL, ticket_id TEXT NOT NULL, event_index INTEGER NOT NULL, key TEXT NOT NULL, value TEXT NOT NULL,
    PRIMARY KEY (workspace_id, ticket_id, event_index, key),
    FOREIGN KEY (workspace_id, ticket_id, event_index) REFERENCES typed_ticket_events(workspace_id, ticket_id, event_index) ON DELETE CASCADE
);
CREATE TABLE typed_ticket_event_references (
    workspace_id TEXT NOT NULL, ticket_id TEXT NOT NULL, event_index INTEGER NOT NULL, ordinal INTEGER NOT NULL, kind TEXT NOT NULL, target TEXT NOT NULL,
    PRIMARY KEY (workspace_id, ticket_id, event_index, ordinal),
    FOREIGN KEY (workspace_id, ticket_id, event_index) REFERENCES typed_ticket_events(workspace_id, ticket_id, event_index) ON DELETE CASCADE
);
CREATE TABLE typed_ticket_events (
    workspace_id TEXT NOT NULL,
    ticket_id TEXT NOT NULL,
    event_index INTEGER NOT NULL,
    kind TEXT NOT NULL,
    author TEXT,
    at TEXT,
    status TEXT,
    from_state TEXT,
    to_state TEXT,
    reason TEXT,
    state_field TEXT,
    heading TEXT,
    body TEXT NOT NULL,
    PRIMARY KEY (workspace_id, ticket_id, event_index),
    FOREIGN KEY (workspace_id, ticket_id) REFERENCES typed_tickets(workspace_id, ticket_id) ON DELETE CASCADE
);
CREATE TABLE typed_ticket_labels (
    workspace_id TEXT NOT NULL, ticket_id TEXT NOT NULL, ordinal INTEGER NOT NULL, label TEXT NOT NULL,
    PRIMARY KEY (workspace_id, ticket_id, ordinal),
    FOREIGN KEY (workspace_id, ticket_id) REFERENCES typed_tickets(workspace_id, ticket_id) ON DELETE CASCADE
);
CREATE TABLE typed_ticket_orchestration_plans (
    workspace_id TEXT NOT NULL,
    ticket_id TEXT NOT NULL,
    record_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    related_ticket TEXT,
    note TEXT,
    accepted_summary TEXT,
    accepted_branch TEXT,
    accepted_worktree TEXT,
    accepted_role_plan TEXT,
    author TEXT NOT NULL,
    at TEXT NOT NULL,
    PRIMARY KEY (workspace_id, ticket_id, record_id),
    FOREIGN KEY (workspace_id, ticket_id) REFERENCES typed_tickets(workspace_id, ticket_id) ON DELETE CASCADE
);
CREATE TABLE typed_ticket_raw_frontmatter (
    workspace_id TEXT NOT NULL, ticket_id TEXT NOT NULL, key TEXT NOT NULL, value TEXT NOT NULL,
    PRIMARY KEY (workspace_id, ticket_id, key),
    FOREIGN KEY (workspace_id, ticket_id) REFERENCES typed_tickets(workspace_id, ticket_id) ON DELETE CASCADE
);
CREATE TABLE typed_ticket_relations (
    workspace_id TEXT NOT NULL, ticket_id TEXT NOT NULL, kind TEXT NOT NULL, target TEXT NOT NULL, note TEXT, author TEXT NOT NULL, at TEXT NOT NULL,
    PRIMARY KEY (workspace_id, ticket_id, kind, target),
    FOREIGN KEY (workspace_id, ticket_id) REFERENCES typed_tickets(workspace_id, ticket_id) ON DELETE CASCADE
);
CREATE TABLE typed_ticket_risk_flags (
    workspace_id TEXT NOT NULL, ticket_id TEXT NOT NULL, ordinal INTEGER NOT NULL, risk_flag TEXT NOT NULL,
    PRIMARY KEY (workspace_id, ticket_id, ordinal),
    FOREIGN KEY (workspace_id, ticket_id) REFERENCES typed_tickets(workspace_id, ticket_id) ON DELETE CASCADE
);
CREATE TABLE typed_tickets (
    workspace_id TEXT NOT NULL,
    ticket_id TEXT NOT NULL,
    slug TEXT NOT NULL,
    title TEXT NOT NULL,
    status TEXT NOT NULL,
    kind TEXT NOT NULL,
    priority TEXT NOT NULL,
    body TEXT NOT NULL,
    created_at TEXT,
    updated_at TEXT,
    assignee TEXT,
    readiness TEXT,
    workflow_state TEXT NOT NULL,
    workflow_state_explicit INTEGER NOT NULL,
    queued_by TEXT,
    queued_at TEXT,
    resolution TEXT, repository_id TEXT, ref_selector TEXT,
    PRIMARY KEY (workspace_id, ticket_id)
);
CREATE TABLE "workspace_resource_key_counters" (
            workspace_id TEXT NOT NULL,
            resource_kind TEXT NOT NULL CHECK (resource_kind IN ('ticket', 'objective', 'worker')),
            next_sequence INTEGER NOT NULL CHECK (next_sequence > 0),
            PRIMARY KEY (workspace_id, resource_kind)
        );
CREATE TABLE "workspace_resource_keys" (
            workspace_id TEXT NOT NULL,
            resource_kind TEXT NOT NULL CHECK (resource_kind IN ('ticket', 'objective', 'worker')),
            resource_id TEXT NOT NULL,
            sequence INTEGER NOT NULL CHECK (sequence > 0),
            resource_key TEXT NOT NULL,
            allocated_at TEXT NOT NULL,
            PRIMARY KEY (workspace_id, resource_kind, resource_id),
            UNIQUE (workspace_id, resource_kind, sequence),
            UNIQUE (workspace_id, resource_key)
        );
CREATE INDEX idx_workspace_resource_keys_reverse
            ON workspace_resource_keys(workspace_id, resource_kind, resource_key);
CREATE INDEX typed_ticket_events_workspace_kind_ticket
            ON typed_ticket_events(workspace_id, kind, ticket_id, event_index);
CREATE INDEX typed_ticket_relations_workspace_source_kind
            ON typed_ticket_relations(workspace_id, ticket_id, kind, target);
CREATE INDEX typed_ticket_relations_workspace_target_kind
            ON typed_ticket_relations(workspace_id, target, kind, ticket_id);
CREATE INDEX typed_tickets_workspace_created
            ON typed_tickets(workspace_id, created_at DESC, ticket_id);
CREATE INDEX typed_tickets_workspace_state_updated
            ON typed_tickets(workspace_id, workflow_state, updated_at DESC, ticket_id);
CREATE INDEX typed_tickets_workspace_title
            ON typed_tickets(workspace_id, title COLLATE NOCASE, ticket_id);
CREATE INDEX typed_tickets_workspace_updated
            ON typed_tickets(workspace_id, updated_at DESC, ticket_id);
