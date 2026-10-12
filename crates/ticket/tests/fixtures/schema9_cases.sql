-- Frozen standalone component9 case extension; combine with schema7.sql.
-- Ticket case authority. Legacy workflow data is a handoff snapshot, never current authority.
CREATE TABLE typed_ticket_case_metadata (
    workspace_id TEXT NOT NULL, ticket_id TEXT NOT NULL, closure_operation_key TEXT,
    PRIMARY KEY (workspace_id, ticket_id),
    FOREIGN KEY (workspace_id, ticket_id) REFERENCES typed_tickets(workspace_id, ticket_id) ON DELETE CASCADE
);
CREATE TABLE typed_ticket_case_history (
    workspace_id TEXT NOT NULL, ticket_id TEXT NOT NULL, ordinal INTEGER NOT NULL,
    operation_key TEXT NOT NULL, request_fingerprint TEXT NOT NULL, judgment_json TEXT NOT NULL,
    PRIMARY KEY (workspace_id, ticket_id, operation_key),
    UNIQUE (workspace_id, ticket_id, ordinal),
    FOREIGN KEY (workspace_id, ticket_id) REFERENCES typed_tickets(workspace_id, ticket_id) ON DELETE CASCADE
);
CREATE TABLE typed_ticket_legacy_workflow_snapshot (
    workspace_id TEXT NOT NULL, ticket_id TEXT NOT NULL, snapshot_json TEXT NOT NULL,
    PRIMARY KEY (workspace_id, ticket_id),
    FOREIGN KEY (workspace_id, ticket_id) REFERENCES typed_tickets(workspace_id, ticket_id) ON DELETE CASCADE
);
CREATE TRIGGER typed_ticket_case_history_no_update BEFORE UPDATE ON typed_ticket_case_history
BEGIN SELECT RAISE(ABORT, 'Ticket case history is immutable'); END;
CREATE TRIGGER typed_ticket_case_history_no_delete BEFORE DELETE ON typed_ticket_case_history
BEGIN SELECT RAISE(ABORT, 'Ticket case history is immutable'); END;
CREATE TRIGGER typed_ticket_legacy_workflow_no_update BEFORE UPDATE ON typed_ticket_legacy_workflow_snapshot
BEGIN SELECT RAISE(ABORT, 'Ticket legacy workflow snapshot is immutable'); END;
CREATE TRIGGER typed_ticket_legacy_workflow_no_delete BEFORE DELETE ON typed_ticket_legacy_workflow_snapshot
BEGIN SELECT RAISE(ABORT, 'Ticket legacy workflow snapshot is immutable'); END;
CREATE TRIGGER typed_ticket_case_state_insert BEFORE INSERT ON typed_tickets
WHEN NEW.workflow_state NOT IN ('open', 'closed')
BEGIN SELECT RAISE(ABORT, 'Ticket case state must be open or closed'); END;
CREATE TRIGGER typed_ticket_case_state_update BEFORE UPDATE OF workflow_state ON typed_tickets
WHEN NEW.workflow_state NOT IN ('open', 'closed')
BEGIN SELECT RAISE(ABORT, 'Ticket case state must be open or closed'); END;
