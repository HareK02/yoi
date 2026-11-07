CREATE TABLE typed_ticket_service_outbox (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    event_id TEXT NOT NULL UNIQUE CHECK(length(event_id) = 32 AND event_id NOT GLOB '*[^0-9a-f]*'),
    workspace_id TEXT NOT NULL CHECK(length(CAST(workspace_id AS BLOB)) BETWEEN 1 AND 256),
    ticket_id TEXT NOT NULL CHECK(length(CAST(ticket_id AS BLOB)) BETWEEN 1 AND 256),
    event_type TEXT NOT NULL CHECK(event_type IN ('changed', 'deleted')),
    occurred_at TEXT NOT NULL CHECK(length(CAST(occurred_at AS BLOB)) BETWEEN 1 AND 64),
    domain_event_ref TEXT NOT NULL CHECK(length(CAST(domain_event_ref AS BLOB)) BETWEEN 1 AND 512),
    content_digest TEXT NOT NULL CHECK(length(content_digest) = 64 AND content_digest NOT GLOB '*[^0-9a-f]*'),
    correlation_id TEXT NOT NULL CHECK(length(CAST(correlation_id AS BLOB)) BETWEEN 1 AND 512),
    causation_id TEXT NOT NULL CHECK(length(CAST(causation_id AS BLOB)) BETWEEN 1 AND 512)
);
CREATE INDEX typed_ticket_service_outbox_workspace_sequence
    ON typed_ticket_service_outbox(workspace_id, sequence);
