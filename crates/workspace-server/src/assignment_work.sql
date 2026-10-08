-- Responsibility is durable. A release only ends the operational authority of
-- this assignment identity; reopening a Ticket must not revive that authority.
CREATE TABLE ticket_assignment_work_releases (
    workspace_id TEXT NOT NULL,
    assignment_id TEXT NOT NULL,
    released_at TEXT NOT NULL,
    PRIMARY KEY (workspace_id, assignment_id),
    FOREIGN KEY (workspace_id, assignment_id)
        REFERENCES ticket_worker_assignments(workspace_id, assignment_id) ON DELETE CASCADE
);
CREATE VIEW ticket_active_worker_assignments AS
    SELECT current.* FROM ticket_current_worker_assignments AS current
    JOIN typed_tickets AS ticket
      ON ticket.workspace_id = current.workspace_id AND ticket.ticket_id = current.ticket_id
    WHERE ticket.workflow_state NOT IN ('done', 'closed')
      AND NOT EXISTS (SELECT 1 FROM ticket_assignment_work_releases AS released
        WHERE released.workspace_id = current.workspace_id
          AND released.assignment_id = current.assignment_id);
CREATE TRIGGER ticket_terminal_releases_work
AFTER UPDATE OF workflow_state ON typed_tickets
WHEN NEW.workflow_state IN ('done', 'closed')
BEGIN
    INSERT OR IGNORE INTO ticket_assignment_work_releases
        SELECT workspace_id, assignment_id, COALESCE(NEW.updated_at, updated_at)
        FROM ticket_current_worker_assignments
        WHERE workspace_id = NEW.workspace_id AND ticket_id = NEW.ticket_id;
END;
CREATE TRIGGER ticket_terminal_assignment_releases_work_insert
AFTER INSERT ON ticket_current_worker_assignments
WHEN EXISTS (SELECT 1 FROM typed_tickets WHERE workspace_id = NEW.workspace_id
    AND ticket_id = NEW.ticket_id AND workflow_state IN ('done', 'closed'))
BEGIN
    INSERT OR IGNORE INTO ticket_assignment_work_releases
        VALUES (NEW.workspace_id, NEW.assignment_id, NEW.updated_at);
END;
CREATE TRIGGER ticket_terminal_assignment_releases_work_update
AFTER UPDATE ON ticket_current_worker_assignments
WHEN EXISTS (SELECT 1 FROM typed_tickets WHERE workspace_id = NEW.workspace_id
    AND ticket_id = NEW.ticket_id AND workflow_state IN ('done', 'closed'))
BEGIN
    INSERT OR IGNORE INTO ticket_assignment_work_releases
        VALUES (NEW.workspace_id, NEW.assignment_id, NEW.updated_at);
END;
CREATE TRIGGER ticket_active_worker_role_insert
BEFORE INSERT ON ticket_current_worker_assignments
WHEN NEW.principal_kind = 'worker'
 AND EXISTS (SELECT 1 FROM typed_tickets WHERE workspace_id = NEW.workspace_id
    AND ticket_id = NEW.ticket_id AND workflow_state NOT IN ('done', 'closed'))
 AND NOT EXISTS (SELECT 1 FROM ticket_assignment_work_releases
    WHERE workspace_id = NEW.workspace_id AND assignment_id = NEW.assignment_id)
 AND EXISTS (SELECT 1 FROM ticket_active_worker_assignments
    WHERE workspace_id = NEW.workspace_id AND role = NEW.role
      AND runtime_id = NEW.runtime_id AND worker_id = NEW.worker_id
      AND assignment_id != NEW.assignment_id)
BEGIN SELECT RAISE(ABORT, 'Worker already has unfinished Ticket work in this role'); END;
CREATE TRIGGER ticket_active_worker_role_update
BEFORE UPDATE ON ticket_current_worker_assignments
WHEN NEW.principal_kind = 'worker'
 AND EXISTS (SELECT 1 FROM typed_tickets WHERE workspace_id = NEW.workspace_id
    AND ticket_id = NEW.ticket_id AND workflow_state NOT IN ('done', 'closed'))
 AND NOT EXISTS (SELECT 1 FROM ticket_assignment_work_releases
    WHERE workspace_id = NEW.workspace_id AND assignment_id = NEW.assignment_id)
 AND EXISTS (SELECT 1 FROM ticket_active_worker_assignments
    WHERE workspace_id = NEW.workspace_id AND role = NEW.role
      AND runtime_id = NEW.runtime_id AND worker_id = NEW.worker_id
      AND assignment_id != OLD.assignment_id)
BEGIN SELECT RAISE(ABORT, 'Worker already has unfinished Ticket work in this role'); END;
-- Keep archived responsibilities valid after Worker removal, but never allow
-- newly installed identities to refer to a removed / removal-reserved Worker.
CREATE TRIGGER ticket_active_worker_principal_insert
BEFORE INSERT ON ticket_current_worker_assignments
WHEN NEW.principal_kind = 'worker' AND (
 NOT EXISTS (SELECT 1 FROM worker_registry WHERE workspace_id = NEW.workspace_id
    AND runtime_id = NEW.runtime_id AND worker_id = NEW.worker_id)
 OR EXISTS (SELECT 1 FROM worker_removal_operations WHERE workspace_id = NEW.workspace_id
    AND runtime_id = NEW.runtime_id AND worker_id = NEW.worker_id
    AND state IN ('executing', 'failed', 'succeeded')))
BEGIN SELECT RAISE(ABORT, 'Worker is missing or removal is reserved'); END;
CREATE TRIGGER ticket_active_worker_principal_update
BEFORE UPDATE ON ticket_current_worker_assignments
WHEN NEW.principal_kind = 'worker' AND (
 NOT EXISTS (SELECT 1 FROM worker_registry WHERE workspace_id = NEW.workspace_id
    AND runtime_id = NEW.runtime_id AND worker_id = NEW.worker_id)
 OR EXISTS (SELECT 1 FROM worker_removal_operations WHERE workspace_id = NEW.workspace_id
    AND runtime_id = NEW.runtime_id AND worker_id = NEW.worker_id
    AND state IN ('executing', 'failed', 'succeeded')))
BEGIN SELECT RAISE(ABORT, 'Worker is missing or removal is reserved'); END;
CREATE TRIGGER ticket_unfinished_work_blocks_worker_delete
BEFORE DELETE ON worker_registry
WHEN EXISTS (SELECT 1 FROM ticket_active_worker_assignments
    WHERE workspace_id = OLD.workspace_id AND runtime_id = OLD.runtime_id
      AND worker_id = OLD.worker_id)
BEGIN SELECT RAISE(ABORT, 'Worker has unfinished Ticket work'); END;
CREATE TRIGGER ticket_unfinished_work_blocks_worker_move
BEFORE UPDATE OF runtime_id, worker_id, workspace_id ON worker_registry
WHEN (OLD.runtime_id != NEW.runtime_id OR OLD.worker_id != NEW.worker_id OR OLD.workspace_id != NEW.workspace_id)
 AND EXISTS (SELECT 1 FROM ticket_active_worker_assignments
    WHERE workspace_id = OLD.workspace_id AND runtime_id = OLD.runtime_id
      AND worker_id = OLD.worker_id)
BEGIN SELECT RAISE(ABORT, 'Worker has unfinished Ticket work'); END;
