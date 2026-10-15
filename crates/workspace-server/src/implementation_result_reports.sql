-- v98: result reports are Worker/Ticket facts, not assignment-completion operations.
-- Preserve legacy request bytes and already-captured notification evidence verbatim.
DROP TRIGGER notification_implementation_result;
DROP TRIGGER implementation_delegation_capture;
CREATE TABLE ticket_implementation_results_v98 (
 workspace_id TEXT NOT NULL, ticket_id TEXT NOT NULL, operation_key TEXT NOT NULL,
 result_id TEXT NOT NULL,
 runtime_id TEXT NOT NULL, worker_id TEXT NOT NULL, request_json TEXT NOT NULL CHECK(json_valid(request_json)),
 created_at TEXT NOT NULL,
 PRIMARY KEY(workspace_id,operation_key), UNIQUE(workspace_id,result_id),
 FOREIGN KEY(workspace_id,ticket_id) REFERENCES typed_tickets(workspace_id,ticket_id)
);
INSERT INTO ticket_implementation_results_v98
 SELECT workspace_id,ticket_id,operation_key,result_id,runtime_id,worker_id,request_json,created_at
 FROM ticket_implementation_results;
DROP TABLE ticket_implementation_results;
ALTER TABLE ticket_implementation_results_v98 RENAME TO ticket_implementation_results;
CREATE TRIGGER implementation_results_no_update BEFORE UPDATE ON ticket_implementation_results BEGIN SELECT RAISE(ABORT,'Implementation results are immutable'); END;
CREATE TRIGGER implementation_results_no_delete BEFORE DELETE ON ticket_implementation_results BEGIN SELECT RAISE(ABORT,'Implementation results are immutable'); END;
-- ticket_implementation_delegations remains read-only historical evidence only.
-- It is no longer populated or consulted by result reporting.
