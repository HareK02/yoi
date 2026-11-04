-- These rows mark the migration boundary, including already terminal Jobs.
-- Do not duplicate request/result bodies: their existing retention/erasure owner
-- remains authoritative. Historical state spellings are not live execution states.
CREATE TABLE backend_job_upgrade_history (
    workspace_id TEXT NOT NULL,
    job_id TEXT NOT NULL,
    state TEXT NOT NULL,
    failure_category TEXT,
    failure_detail TEXT,
    updated_at TEXT NOT NULL,
    completed_at TEXT,
    migrated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    PRIMARY KEY (workspace_id, job_id),
    FOREIGN KEY (workspace_id, job_id)
        REFERENCES backend_jobs(workspace_id, job_id) ON DELETE CASCADE
);
CREATE TABLE backend_job_attempt_upgrade_history (
    workspace_id TEXT NOT NULL,
    job_id TEXT NOT NULL,
    attempt_id TEXT NOT NULL,
    state TEXT NOT NULL,
    failure_category TEXT,
    failure_detail TEXT,
    updated_at TEXT NOT NULL,
    completed_at TEXT,
    migrated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    PRIMARY KEY (workspace_id, job_id, attempt_id),
    FOREIGN KEY (workspace_id, job_id, attempt_id)
        REFERENCES backend_job_attempts(workspace_id, job_id, attempt_id) ON DELETE CASCADE
);
CREATE TRIGGER backend_job_upgrade_history_no_update
BEFORE UPDATE ON backend_job_upgrade_history
BEGIN SELECT RAISE(ABORT, 'immutable Backend Job upgrade history'); END;
CREATE TRIGGER backend_job_attempt_upgrade_history_no_update
BEFORE UPDATE ON backend_job_attempt_upgrade_history
BEGIN SELECT RAISE(ABORT, 'immutable Backend Job attempt upgrade history'); END;

-- FINAL_EXECUTION_GUARDS
-- Install these only after all captured pre-content values have been composed.
-- Input retirement and independently evidenced cleanup are deliberately separate.
CREATE TRIGGER backend_job_upgrade_no_resume
BEFORE UPDATE ON backend_jobs
WHEN EXISTS (SELECT 1 FROM backend_job_upgrade_history h
             WHERE h.workspace_id=OLD.workspace_id AND h.job_id=OLD.job_id)
 AND OLD.state IN ('completed','failed')
 AND (NEW.state IS NOT OLD.state OR NEW.current_attempt IS NOT OLD.current_attempt
      OR NEW.result_json IS NOT OLD.result_json OR NEW.result_digest IS NOT OLD.result_digest
      OR NEW.failure_category IS NOT OLD.failure_category OR NEW.failure_detail IS NOT OLD.failure_detail
      OR NEW.completed_at IS NOT OLD.completed_at)
BEGIN SELECT RAISE(ABORT, 'pre-upgrade Backend Job cannot resume'); END;
CREATE TRIGGER backend_job_upgrade_no_attempt
BEFORE INSERT ON backend_job_attempts
WHEN EXISTS (SELECT 1 FROM backend_job_upgrade_history h
             WHERE h.workspace_id=NEW.workspace_id AND h.job_id=NEW.job_id)
BEGIN SELECT RAISE(ABORT, 'pre-upgrade Backend Job cannot create attempts'); END;
CREATE TRIGGER backend_job_upgrade_no_attempt_reparent
BEFORE UPDATE OF workspace_id,job_id ON backend_job_attempts
WHEN (NEW.workspace_id IS NOT OLD.workspace_id OR NEW.job_id IS NOT OLD.job_id)
 AND EXISTS (SELECT 1 FROM backend_job_upgrade_history h
             WHERE h.workspace_id=NEW.workspace_id AND h.job_id=NEW.job_id)
BEGIN SELECT RAISE(ABORT, 'pre-upgrade Backend Job cannot adopt attempts'); END;
CREATE TRIGGER backend_job_attempt_upgrade_no_resume
BEFORE UPDATE ON backend_job_attempts
WHEN EXISTS (SELECT 1 FROM backend_job_attempt_upgrade_history h
             WHERE h.workspace_id=OLD.workspace_id AND h.job_id=OLD.job_id AND h.attempt_id=OLD.attempt_id)
 AND OLD.state IN ('completed','failed')
 AND (NEW.workspace_id IS NOT OLD.workspace_id OR NEW.job_id IS NOT OLD.job_id
      OR NEW.attempt_id IS NOT OLD.attempt_id OR NEW.attempt IS NOT OLD.attempt
      OR NEW.input_digest IS NOT OLD.input_digest OR NEW.state IS NOT OLD.state
      OR NEW.runtime_id IS NOT OLD.runtime_id OR NEW.worker_id IS NOT OLD.worker_id
      OR NEW.runtime_run_id IS NOT OLD.runtime_run_id OR NEW.dispatched_at IS NOT OLD.dispatched_at
      OR NEW.deadline_at IS NOT OLD.deadline_at
      OR NEW.result_json IS NOT OLD.result_json OR NEW.result_digest IS NOT OLD.result_digest
      OR NEW.failure_category IS NOT OLD.failure_category OR NEW.failure_detail IS NOT OLD.failure_detail
      OR NEW.completed_at IS NOT OLD.completed_at)
BEGIN SELECT RAISE(ABORT, 'pre-upgrade Backend Job attempt cannot resume'); END;
