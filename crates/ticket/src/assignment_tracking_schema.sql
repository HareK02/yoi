-- Applied only by component 10 -> 11, never by request handlers.
ALTER TABLE ticket_worker_assignments ADD COLUMN tracking_json TEXT
    CHECK(tracking_json IS NULL OR (role = 'orchestrator' AND json_valid(tracking_json) AND json_type(tracking_json)='object'
      AND COALESCE(json_extract(tracking_json, '$.status') IN ('waiting', 'inprogress', 'done'), 0)));
ALTER TABLE ticket_worker_assignments ADD COLUMN last_tracking_event_id TEXT;
ALTER TABLE ticket_worker_assignments ADD COLUMN tracking_updated_at TEXT;
ALTER TABLE ticket_worker_assignments ADD COLUMN tracking_migration_diagnostic TEXT;
CREATE TABLE typed_ticket_assignment_tracking_history (
    workspace_id TEXT NOT NULL,
    ticket_id TEXT NOT NULL,
    assignment_id TEXT NOT NULL,
    event_id TEXT NOT NULL,
    operation_key TEXT NOT NULL,
    tracking_json TEXT NOT NULL CHECK(json_valid(tracking_json)),
    source_context_json TEXT NOT NULL CHECK(json_valid(source_context_json)),
    at TEXT NOT NULL,
    PRIMARY KEY(workspace_id, event_id),
    UNIQUE(workspace_id, assignment_id, operation_key),
    FOREIGN KEY(workspace_id, ticket_id, assignment_id)
      REFERENCES ticket_worker_assignments(workspace_id, ticket_id, assignment_id)
);
CREATE TABLE typed_ticket_assignment_tracking_operations (
    workspace_id TEXT NOT NULL,
    operation_key TEXT NOT NULL,
    ticket_id TEXT NOT NULL,
    input_json TEXT NOT NULL,
    source_context_json TEXT NOT NULL,
    result_json TEXT NOT NULL,
    PRIMARY KEY(workspace_id, operation_key)
);
CREATE TRIGGER typed_ticket_assignment_tracking_initialize
AFTER INSERT ON ticket_worker_assignments WHEN NEW.role = 'orchestrator'
BEGIN
    UPDATE ticket_worker_assignments SET tracking_json = '{"status":"waiting","waiting_reason":"Initial judgment pending","progress":null,"next_action":null,"result":null}', tracking_updated_at = NEW.assigned_at
    WHERE workspace_id = NEW.workspace_id AND assignment_id = NEW.assignment_id;
END;
CREATE TRIGGER typed_ticket_assignment_tracking_assigned
AFTER INSERT ON ticket_worker_assignment_events
WHEN NEW.role = 'orchestrator' AND NEW.action IN ('assigned', 'reassigned')
BEGIN
    UPDATE ticket_worker_assignments SET last_tracking_event_id = NEW.event_id, tracking_updated_at = NEW.created_at
    WHERE workspace_id = NEW.workspace_id AND assignment_id = NEW.assignment_id AND tracking_json IS NOT NULL;
    INSERT INTO typed_ticket_assignment_tracking_history
    SELECT workspace_id, ticket_id, assignment_id, NEW.event_id, COALESCE(NEW.operation_id, NEW.event_id), tracking_json,
        json_object('actor', NEW.actor, 'kind', 'assignment'), NEW.created_at
    FROM ticket_worker_assignments
    WHERE workspace_id = NEW.workspace_id AND assignment_id = NEW.assignment_id AND tracking_json IS NOT NULL;
END;
CREATE TRIGGER typed_ticket_assignment_tracking_history_no_update
BEFORE UPDATE ON typed_ticket_assignment_tracking_history
BEGIN SELECT RAISE(ABORT, 'Ticket assignment tracking history is immutable'); END;
CREATE TRIGGER typed_ticket_assignment_tracking_history_no_delete
BEFORE DELETE ON typed_ticket_assignment_tracking_history
BEGIN SELECT RAISE(ABORT, 'Ticket assignment tracking history is immutable'); END;
CREATE TRIGGER typed_ticket_assignment_tracking_operations_no_update
BEFORE UPDATE ON typed_ticket_assignment_tracking_operations
BEGIN SELECT RAISE(ABORT, 'Ticket assignment tracking receipts are immutable'); END;
CREATE TRIGGER typed_ticket_assignment_tracking_operations_no_delete
BEFORE DELETE ON typed_ticket_assignment_tracking_operations
BEGIN SELECT RAISE(ABORT, 'Ticket assignment tracking receipts are immutable'); END;
-- Original assignment identity/principal/audit is append-only. Tracking is the
-- only mutable assignment value; startup claims are mutable only while pending.
CREATE TRIGGER typed_ticket_assignment_identity_no_update
BEFORE UPDATE OF workspace_id,ticket_id,assignment_id,role,principal_kind,principal_id,runtime_id,worker_id,assigned_by,assigned_at ON ticket_worker_assignments
WHEN OLD.workspace_id IS NOT NEW.workspace_id OR OLD.ticket_id IS NOT NEW.ticket_id
 OR OLD.assignment_id IS NOT NEW.assignment_id OR OLD.role IS NOT NEW.role
 OR OLD.principal_kind IS NOT NEW.principal_kind OR OLD.principal_id IS NOT NEW.principal_id
 OR OLD.runtime_id IS NOT NEW.runtime_id OR OLD.worker_id IS NOT NEW.worker_id
 OR OLD.assigned_by IS NOT NEW.assigned_by OR OLD.assigned_at IS NOT NEW.assigned_at
BEGIN SELECT RAISE(ABORT, 'Ticket assignment identity is immutable'); END;
CREATE TRIGGER typed_ticket_assignment_identity_no_delete BEFORE DELETE ON ticket_worker_assignments
BEGIN SELECT RAISE(ABORT, 'Ticket assignment identity is immutable'); END;
CREATE TRIGGER typed_ticket_assignment_events_no_update BEFORE UPDATE ON ticket_worker_assignment_events
BEGIN SELECT RAISE(ABORT, 'Ticket assignment audit is immutable'); END;
CREATE TRIGGER typed_ticket_assignment_events_no_delete BEFORE DELETE ON ticket_worker_assignment_events
BEGIN SELECT RAISE(ABORT, 'Ticket assignment audit is immutable'); END;
CREATE TRIGGER typed_ticket_assignment_receipt_no_update BEFORE UPDATE ON ticket_assignment_operations
WHEN OLD.claim_state != 'pending'
BEGIN SELECT RAISE(ABORT, 'Ticket assignment terminal receipt is immutable'); END;
CREATE TRIGGER typed_ticket_assignment_receipt_no_delete BEFORE DELETE ON ticket_assignment_operations
WHEN OLD.claim_state != 'pending'
BEGIN SELECT RAISE(ABORT, 'Ticket assignment terminal receipt is immutable'); END;
CREATE TRIGGER typed_ticket_assignment_current_principal_insert BEFORE INSERT ON ticket_current_worker_assignments
WHEN NOT EXISTS (SELECT 1 FROM ticket_worker_assignments a WHERE a.workspace_id=NEW.workspace_id
 AND a.ticket_id=NEW.ticket_id AND a.assignment_id=NEW.assignment_id AND a.role=NEW.role
 AND a.principal_kind=NEW.principal_kind AND a.principal_id IS NEW.principal_id
 AND a.runtime_id IS NEW.runtime_id AND a.worker_id IS NEW.worker_id)
 OR EXISTS (SELECT 1 FROM ticket_worker_assignment_events e WHERE e.workspace_id=NEW.workspace_id
 AND ((e.action='unassigned' AND (e.assignment_id=NEW.assignment_id OR e.previous_assignment_id=NEW.assignment_id))
 OR (e.action='reassigned' AND e.previous_assignment_id=NEW.assignment_id)))
BEGIN SELECT RAISE(ABORT, 'Ticket current assignment must match an unreleased immutable identity'); END;
CREATE TRIGGER typed_ticket_assignment_current_principal_update BEFORE UPDATE ON ticket_current_worker_assignments
WHEN NOT EXISTS (SELECT 1 FROM ticket_worker_assignments a WHERE a.workspace_id=NEW.workspace_id
 AND a.ticket_id=NEW.ticket_id AND a.assignment_id=NEW.assignment_id AND a.role=NEW.role
 AND a.principal_kind=NEW.principal_kind AND a.principal_id IS NEW.principal_id
 AND a.runtime_id IS NEW.runtime_id AND a.worker_id IS NEW.worker_id)
 OR EXISTS (SELECT 1 FROM ticket_worker_assignment_events e WHERE e.workspace_id=NEW.workspace_id
 AND ((e.action='unassigned' AND (e.assignment_id=NEW.assignment_id OR e.previous_assignment_id=NEW.assignment_id))
 OR (e.action='reassigned' AND e.previous_assignment_id=NEW.assignment_id)))
BEGIN SELECT RAISE(ABORT, 'Ticket current assignment must match an unreleased immutable identity'); END;
CREATE TRIGGER typed_ticket_assignment_orchestrator_principal_insert BEFORE INSERT ON ticket_worker_assignments
WHEN NEW.role='orchestrator' AND (NEW.principal_kind!='workspace_agent' OR NEW.principal_id!='workspace-orchestrator')
BEGIN SELECT RAISE(ABORT, 'Orchestrator assignment requires the stable Workspace agent principal'); END;
CREATE TRIGGER typed_ticket_assignment_released_tracking_no_update
BEFORE UPDATE OF tracking_json,last_tracking_event_id,tracking_updated_at ON ticket_worker_assignments
WHEN EXISTS (SELECT 1 FROM ticket_worker_assignment_events e WHERE e.workspace_id=OLD.workspace_id
 AND ((e.action='unassigned' AND (e.assignment_id=OLD.assignment_id OR e.previous_assignment_id=OLD.assignment_id))
 OR (e.action='reassigned' AND e.previous_assignment_id=OLD.assignment_id)))
BEGIN SELECT RAISE(ABORT, 'Released assignment tracking is immutable'); END;
CREATE TRIGGER typed_ticket_assignment_parent_insert BEFORE INSERT ON ticket_worker_assignments
WHEN NOT EXISTS (SELECT 1 FROM typed_tickets t WHERE t.workspace_id=NEW.workspace_id AND t.ticket_id=NEW.ticket_id)
BEGIN SELECT RAISE(ABORT, 'New assignment requires a Ticket in the same Workspace'); END;
CREATE TRIGGER typed_ticket_assignment_role_principal_insert BEFORE INSERT ON ticket_worker_assignments
WHEN (NEW.role='owner' AND NEW.principal_kind!='user')
 OR (NEW.role='worker' AND NEW.principal_kind='workspace_agent')
 OR (NEW.principal_kind='workspace_agent' AND NEW.principal_id!='workspace-orchestrator')
BEGIN SELECT RAISE(ABORT, 'Assignment role and principal are incompatible'); END;
CREATE TRIGGER typed_ticket_assignment_audit_parent_insert BEFORE INSERT ON ticket_worker_assignment_events
WHEN (NEW.assignment_id IS NOT NULL AND NOT EXISTS (SELECT 1 FROM ticket_worker_assignments a WHERE a.workspace_id=NEW.workspace_id AND a.ticket_id=NEW.ticket_id AND a.role=NEW.role AND a.assignment_id=NEW.assignment_id))
 OR (NEW.previous_assignment_id IS NOT NULL AND NOT EXISTS (SELECT 1 FROM ticket_worker_assignments a WHERE a.workspace_id=NEW.workspace_id AND a.ticket_id=NEW.ticket_id AND a.role=NEW.role AND a.assignment_id=NEW.previous_assignment_id))
BEGIN SELECT RAISE(ABORT, 'Assignment audit must match the same immutable Ticket role'); END;
CREATE TRIGGER typed_ticket_assignment_operation_parent_insert BEFORE INSERT ON ticket_assignment_operations
WHEN NOT EXISTS (SELECT 1 FROM typed_tickets t WHERE t.workspace_id=NEW.workspace_id AND t.ticket_id=NEW.ticket_id)
BEGIN SELECT RAISE(ABORT, 'New assignment operation requires a Ticket in the same Workspace'); END;

-- Equality scope keys leave SQLite's implicit rowid as stable append ordering.
CREATE INDEX typed_ticket_assignment_tracking_history_scope
ON typed_ticket_assignment_tracking_history(workspace_id,ticket_id,assignment_id);
