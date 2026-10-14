-- Schema 94 operational inputs: canonical edits no longer append Ticket events.
-- Record input identity in the content transaction, never reconstruct it from
-- mutable text/timestamps or turn an old migrated edit into a new delegation.
CREATE TABLE ticket_assignment_content_inputs (
    input_sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    workspace_id TEXT NOT NULL,
    ticket_id TEXT NOT NULL,
    assignment_id TEXT NOT NULL,
    after_ticket_event_index INTEGER NOT NULL,
    FOREIGN KEY(workspace_id,ticket_id,assignment_id)
        REFERENCES ticket_worker_assignments(workspace_id,ticket_id,assignment_id)
);
CREATE TRIGGER ticket_assignment_content_input_no_update
BEFORE UPDATE ON ticket_assignment_content_inputs
BEGIN SELECT RAISE(ABORT,'Assignment content input is immutable'); END;
CREATE TRIGGER ticket_assignment_content_input_no_delete
BEFORE DELETE ON ticket_assignment_content_inputs
BEGIN SELECT RAISE(ABORT,'Assignment content input is immutable'); END;
CREATE TRIGGER ticket_assignment_title_input
AFTER UPDATE OF title ON typed_tickets
WHEN NEW.title IS NOT OLD.title
BEGIN
    INSERT INTO ticket_assignment_content_inputs
        (workspace_id,ticket_id,assignment_id,after_ticket_event_index)
    SELECT a.workspace_id,a.ticket_id,a.assignment_id,
        COALESCE((SELECT MAX(event_index) FROM typed_ticket_events
            WHERE workspace_id=a.workspace_id AND ticket_id=a.ticket_id),-1)
    FROM ticket_current_worker_assignments c
    JOIN ticket_worker_assignments a USING(workspace_id,ticket_id,assignment_id)
    JOIN ticket_assignment_delivery_origins o USING(workspace_id,ticket_id,assignment_id)
    WHERE a.workspace_id=NEW.workspace_id AND a.ticket_id=NEW.ticket_id
        AND a.role='orchestrator' AND a.principal_kind='workspace_agent'
        AND a.principal_id='workspace-orchestrator' AND NEW.workflow_state='open'
        AND json_extract(a.tracking_json,'$.status') IN ('waiting','inprogress')
        AND NOT EXISTS(SELECT 1 FROM ticket_assignment_work_releases r
            WHERE r.workspace_id=a.workspace_id AND r.assignment_id=a.assignment_id);
END;
CREATE TRIGGER ticket_assignment_initial_post_input
AFTER UPDATE OF body ON typed_ticket_events
WHEN NEW.event_index=-1 AND NEW.body IS NOT OLD.body
BEGIN
    INSERT INTO ticket_assignment_content_inputs
        (workspace_id,ticket_id,assignment_id,after_ticket_event_index)
    SELECT a.workspace_id,a.ticket_id,a.assignment_id,
        COALESCE((SELECT MAX(event_index) FROM typed_ticket_events
            WHERE workspace_id=a.workspace_id AND ticket_id=a.ticket_id),-1)
    FROM ticket_current_worker_assignments c
    JOIN ticket_worker_assignments a USING(workspace_id,ticket_id,assignment_id)
    JOIN ticket_assignment_delivery_origins o USING(workspace_id,ticket_id,assignment_id)
    JOIN typed_tickets t USING(workspace_id,ticket_id)
    WHERE a.workspace_id=NEW.workspace_id AND a.ticket_id=NEW.ticket_id
        AND a.role='orchestrator' AND a.principal_kind='workspace_agent'
        AND a.principal_id='workspace-orchestrator' AND t.workflow_state='open'
        AND json_extract(a.tracking_json,'$.status') IN ('waiting','inprogress')
        AND NOT EXISTS(SELECT 1 FROM ticket_assignment_work_releases r
            WHERE r.workspace_id=a.workspace_id AND r.assignment_id=a.assignment_id);
END;
