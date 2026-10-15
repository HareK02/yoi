CREATE TABLE merge_request_source_observations (
 sequence INTEGER PRIMARY KEY AUTOINCREMENT,
 workspace_id TEXT NOT NULL, merge_request_id TEXT NOT NULL,
 subject_ref TEXT NOT NULL, previous_subject_ref TEXT,
 observed_at TEXT NOT NULL,
 FOREIGN KEY(workspace_id,merge_request_id) REFERENCES merge_requests(workspace_id,merge_request_id)
);
CREATE INDEX mr_source_observation_latest ON merge_request_source_observations(workspace_id,merge_request_id,sequence);
CREATE TRIGGER mr_source_observations_no_update BEFORE UPDATE ON merge_request_source_observations BEGIN SELECT RAISE(ABORT,'Source observations are immutable'); END;
CREATE TRIGGER mr_source_observations_no_delete BEFORE DELETE ON merge_request_source_observations BEGIN SELECT RAISE(ABORT,'Source observations are immutable'); END;
INSERT INTO merge_request_source_observations(workspace_id,merge_request_id,subject_ref,observed_at)
 SELECT e.workspace_id,e.merge_request_id,json_extract(e.payload_json,'$.subject_ref'),e.created_at
 FROM merge_request_thread_events e WHERE e.sequence=(SELECT MAX(x.sequence) FROM merge_request_thread_events x WHERE x.workspace_id=e.workspace_id AND x.merge_request_id=e.merge_request_id AND json_extract(x.payload_json,'$.subject_ref') IS NOT NULL);

-- Delegation is pinned when implementation responsibility is created, not at result arrival.
CREATE TABLE ticket_implementation_delegations (
 workspace_id TEXT NOT NULL, assignment_id TEXT NOT NULL, orchestrator_assignment_id TEXT,
 PRIMARY KEY(workspace_id,assignment_id),
 FOREIGN KEY(workspace_id,assignment_id) REFERENCES ticket_worker_assignments(workspace_id,assignment_id),
 FOREIGN KEY(workspace_id,orchestrator_assignment_id) REFERENCES ticket_worker_assignments(workspace_id,assignment_id)
);
CREATE TRIGGER implementation_delegation_capture AFTER INSERT ON ticket_worker_assignments
WHEN NEW.role='worker' BEGIN
 INSERT INTO ticket_implementation_delegations
 SELECT NEW.workspace_id,NEW.assignment_id,c.assignment_id FROM (SELECT 1) seed
 LEFT JOIN ticket_current_worker_assignments c ON c.workspace_id=NEW.workspace_id AND c.ticket_id=NEW.ticket_id AND c.role='orchestrator';
END;
-- Retain a pre-upgrade parent only when its committed startup operation has exactly
-- one authenticated delegation fence. Never infer ownership from time or comment prose.
-- Older ambiguous rows remain unknown; explicit reassignment can confirm that work.
INSERT INTO ticket_implementation_delegations
 SELECT w.workspace_id,w.assignment_id,
 (SELECT MIN(f.orchestrator_assignment_id) FROM ticket_assignment_operations o
  JOIN ticket_assignment_operation_fences f ON f.workspace_id=o.workspace_id AND f.operation_id=o.operation_id
  JOIN ticket_worker_assignments a ON a.workspace_id=f.workspace_id AND a.assignment_id=f.orchestrator_assignment_id AND a.ticket_id=w.ticket_id AND a.role='orchestrator'
  WHERE o.workspace_id=w.workspace_id AND o.assignment_id=w.assignment_id AND o.claim_state='committed'
  HAVING COUNT(DISTINCT f.orchestrator_assignment_id)=1)
 FROM ticket_worker_assignments w WHERE w.role='worker';
CREATE TABLE ticket_implementation_results (
 workspace_id TEXT NOT NULL, ticket_id TEXT NOT NULL, operation_key TEXT NOT NULL,
 result_id TEXT NOT NULL, assignment_id TEXT NOT NULL, orchestrator_assignment_id TEXT NOT NULL,
 runtime_id TEXT NOT NULL, worker_id TEXT NOT NULL, request_json TEXT NOT NULL CHECK(json_valid(request_json)),
 created_at TEXT NOT NULL,
 PRIMARY KEY(workspace_id,operation_key), UNIQUE(workspace_id,result_id),
 FOREIGN KEY(workspace_id,ticket_id,assignment_id) REFERENCES ticket_worker_assignments(workspace_id,ticket_id,assignment_id),
 FOREIGN KEY(workspace_id,ticket_id,orchestrator_assignment_id) REFERENCES ticket_worker_assignments(workspace_id,ticket_id,assignment_id)
);
CREATE TRIGGER implementation_results_no_update BEFORE UPDATE ON ticket_implementation_results BEGIN SELECT RAISE(ABORT,'Implementation results are immutable'); END;
CREATE TRIGGER implementation_results_no_delete BEFORE DELETE ON ticket_implementation_results BEGIN SELECT RAISE(ABORT,'Implementation results are immutable'); END;
CREATE TRIGGER implementation_delegations_no_update BEFORE UPDATE ON ticket_implementation_delegations BEGIN SELECT RAISE(ABORT,'Implementation delegation is immutable'); END;
CREATE TRIGGER implementation_delegations_no_delete BEFORE DELETE ON ticket_implementation_delegations BEGIN SELECT RAISE(ABORT,'Implementation delegation is immutable'); END;
