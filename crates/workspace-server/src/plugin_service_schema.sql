CREATE TABLE plugin_service_consumers (
 workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
 consumer_id TEXT NOT NULL,
 runtime_id TEXT NOT NULL,
 worker_id TEXT NOT NULL,
 installation_id TEXT NOT NULL,
 plan_id TEXT NOT NULL,
 service_name TEXT NOT NULL,
 subscriptions_json TEXT NOT NULL CHECK(json_valid(subscriptions_json) AND json_type(subscriptions_json)='array' AND json_array_length(subscriptions_json) BETWEEN 1 AND 2),
 cursor INTEGER NOT NULL CHECK(cursor>=0),
 -- Explicit recovery may drain the already observed backlog, one event at a time.
 -- New backlog beyond this watermark is subject to ordinary backpressure again.
 drain_through INTEGER NOT NULL DEFAULT 0 CHECK(drain_through>=0),
 lease_token TEXT,
 lease_until_ms INTEGER NOT NULL DEFAULT 0,
 pause_reason TEXT CHECK(pause_reason IN ('rejected','attempts_exhausted','queue_full','retention_gap','indeterminate','authorization')),
 current_sequence INTEGER,
 current_event_id TEXT,
 invocation_id TEXT,
 attempt INTEGER NOT NULL DEFAULT 0 CHECK(attempt BETWEEN 0 AND 5),
 next_attempt_ms INTEGER NOT NULL DEFAULT 0,
 last_ack_event_id TEXT,
 PRIMARY KEY(workspace_id,consumer_id),
 UNIQUE(workspace_id,runtime_id,worker_id,installation_id,plan_id,service_name),
 FOREIGN KEY(workspace_id,plan_id) REFERENCES plugin_execution_plans(workspace_id,plan_id),
 FOREIGN KEY(workspace_id,installation_id) REFERENCES plugin_installations(workspace_id,installation_id),
 CHECK((current_sequence IS NULL AND current_event_id IS NULL AND invocation_id IS NULL) OR (current_sequence IS NOT NULL AND current_event_id IS NOT NULL))
);
-- Release proofs outlive lease expiry/rotation: a lost response can be retried
-- for the lifetime of the consumer. Only deleting that owner may discard proofs.
CREATE TABLE plugin_service_released_proofs (
 workspace_id TEXT NOT NULL,
 consumer_id TEXT NOT NULL,
 lease_token TEXT NOT NULL CHECK(length(CAST(lease_token AS BLOB)) BETWEEN 1 AND 512),
 released_at_ms INTEGER NOT NULL,
 PRIMARY KEY(workspace_id,consumer_id,lease_token),
 FOREIGN KEY(workspace_id,consumer_id) REFERENCES plugin_service_consumers(workspace_id,consumer_id) ON DELETE CASCADE
);
CREATE TRIGGER plugin_service_released_proof_immutable BEFORE UPDATE ON plugin_service_released_proofs
BEGIN SELECT RAISE(ABORT,'Service released proof is immutable'); END;
CREATE TRIGGER plugin_service_released_proof_retained BEFORE DELETE ON plugin_service_released_proofs
WHEN EXISTS(SELECT 1 FROM plugin_service_consumers WHERE workspace_id=OLD.workspace_id AND consumer_id=OLD.consumer_id)
BEGIN SELECT RAISE(ABORT,'Service released proof must be retained'); END;
CREATE TABLE plugin_service_retention (
 workspace_id TEXT PRIMARY KEY REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
 removed_through INTEGER NOT NULL CHECK(removed_through>=0)
);
CREATE TABLE plugin_service_recovery_audit (
 workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
 operation_key TEXT NOT NULL CHECK(length(CAST(operation_key AS BLOB)) BETWEEN 1 AND 128),
 consumer_id TEXT NOT NULL,
 fingerprint TEXT NOT NULL,
 actor TEXT NOT NULL,
 reason TEXT NOT NULL CHECK(length(CAST(reason AS BLOB)) BETWEEN 1 AND 1024),
 action_json TEXT NOT NULL CHECK(json_valid(action_json)),
 at_ms INTEGER NOT NULL,
 PRIMARY KEY(workspace_id,operation_key),
 FOREIGN KEY(workspace_id,consumer_id) REFERENCES plugin_service_consumers(workspace_id,consumer_id)
);
CREATE TRIGGER plugin_service_identity_immutable BEFORE UPDATE OF workspace_id,consumer_id,runtime_id,worker_id,installation_id,plan_id,service_name,subscriptions_json ON plugin_service_consumers
BEGIN SELECT RAISE(ABORT,'Service consumer identity is immutable'); END;
CREATE TRIGGER plugin_service_cursor_forward BEFORE UPDATE OF cursor ON plugin_service_consumers WHEN NEW.cursor<OLD.cursor
BEGIN SELECT RAISE(ABORT,'Service cursor cannot rewind'); END;
CREATE TRIGGER plugin_service_audit_immutable BEFORE UPDATE ON plugin_service_recovery_audit
BEGIN SELECT RAISE(ABORT,'Service recovery audit is immutable'); END;
CREATE TRIGGER plugin_service_audit_retained BEFORE DELETE ON plugin_service_recovery_audit WHEN EXISTS(SELECT 1 FROM workspaces WHERE workspace_id=OLD.workspace_id)
BEGIN SELECT RAISE(ABORT,'Service recovery audit must be retained'); END;
