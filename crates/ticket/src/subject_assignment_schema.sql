-- Component 14: additive Subject responsibility guards. Historical identities remain readable.

DROP TRIGGER typed_ticket_assignment_orchestrator_principal_insert;
CREATE TRIGGER typed_ticket_assignment_orchestrator_principal_insert
BEFORE INSERT ON ticket_worker_assignments
WHEN NEW.role='orchestrator' AND NEW.principal_kind!='subject'
BEGIN SELECT RAISE(ABORT,'New Orchestrator responsibility requires a Subject principal'); END;

DROP TRIGGER typed_ticket_assignment_role_principal_insert;
CREATE TRIGGER typed_ticket_assignment_role_principal_insert
BEFORE INSERT ON ticket_worker_assignments
WHEN (NEW.role='owner' AND NEW.principal_kind!='user')
 OR (NEW.role='worker' AND NEW.principal_kind IN ('workspace_agent','subject'))
 OR (NEW.principal_kind='workspace_agent' AND NEW.principal_id!='workspace-orchestrator')
 OR (NEW.principal_kind='subject' AND NEW.role!='orchestrator')
BEGIN SELECT RAISE(ABORT,'Assignment role and principal are incompatible'); END;

CREATE TRIGGER ticket_subject_current_insert
BEFORE INSERT ON ticket_current_worker_assignments
WHEN (NEW.role='orchestrator' AND NEW.principal_kind!='subject')
 OR (NEW.principal_kind='subject' AND NEW.role!='orchestrator')
BEGIN SELECT RAISE(ABORT,'Current Orchestrator responsibility requires a Subject principal'); END;
CREATE TRIGGER ticket_subject_current_update
BEFORE UPDATE OF workspace_id,ticket_id,role,assignment_id,principal_kind,principal_id,runtime_id,worker_id
ON ticket_current_worker_assignments
WHEN (NEW.role='orchestrator' AND NEW.principal_kind!='subject')
 OR (NEW.principal_kind='subject' AND NEW.role!='orchestrator')
BEGIN SELECT RAISE(ABORT,'Current Orchestrator responsibility requires a Subject principal'); END;

CREATE TRIGGER ticket_subject_operation_insert
BEFORE INSERT ON ticket_assignment_operations
WHEN (NEW.role='orchestrator' AND NEW.action!='unassign' AND NEW.principal_kind!='subject')
 OR (NEW.principal_kind='subject' AND (NEW.role!='orchestrator'
   OR NEW.principal_id IS NULL OR length(trim(NEW.principal_id))=0
   OR NEW.runtime_id IS NOT NULL OR NEW.worker_id IS NOT NULL))
BEGIN SELECT RAISE(ABORT,'New Orchestrator assignment operation requires a Subject principal'); END;
CREATE TRIGGER ticket_subject_operation_update
BEFORE UPDATE ON ticket_assignment_operations
WHEN (NEW.role='orchestrator' AND NEW.action!='unassign' AND NEW.principal_kind!='subject')
 OR (NEW.principal_kind='subject' AND (NEW.role!='orchestrator'
   OR NEW.principal_id IS NULL OR length(trim(NEW.principal_id))=0
   OR NEW.runtime_id IS NOT NULL OR NEW.worker_id IS NOT NULL))
BEGIN SELECT RAISE(ABORT,'New Orchestrator assignment operation requires a Subject principal'); END;

CREATE TRIGGER ticket_legacy_orchestrator_tracking_no_update
BEFORE UPDATE OF tracking_json,last_tracking_event_id,tracking_updated_at ON ticket_worker_assignments
WHEN OLD.role='orchestrator' AND OLD.principal_kind!='subject'
BEGIN SELECT RAISE(ABORT,'Legacy Orchestrator responsibility is nonoperational'); END;
CREATE TRIGGER ticket_legacy_orchestrator_history_no_insert
BEFORE INSERT ON typed_ticket_assignment_tracking_history
WHEN EXISTS(SELECT 1 FROM ticket_worker_assignments a
 WHERE a.workspace_id=NEW.workspace_id AND a.assignment_id=NEW.assignment_id
 AND a.role='orchestrator' AND a.principal_kind!='subject')
BEGIN SELECT RAISE(ABORT,'Legacy Orchestrator responsibility is nonoperational'); END;
