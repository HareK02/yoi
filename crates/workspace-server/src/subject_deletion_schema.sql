-- Content-free, monotonic main-DB fence. The Host installs the domain fence first
-- and holds its Subject gate across snapshot/admission. No Subject contents live here.
CREATE TABLE subject_deletions (
    workspace_id TEXT NOT NULL,
    subject_id TEXT NOT NULL,
    operation_id TEXT NOT NULL CHECK(length(operation_id)>0),
    completed INTEGER NOT NULL DEFAULT 0 CHECK(completed IN (0,1)),
    jobs_erased INTEGER NOT NULL DEFAULT 0 CHECK(jobs_erased IN (0,1)),
    PRIMARY KEY(workspace_id,subject_id),
    UNIQUE(workspace_id,operation_id),
    FOREIGN KEY(workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE
);
CREATE TABLE subject_deletion_workers (
    workspace_id TEXT NOT NULL,
    subject_id TEXT NOT NULL,
    runtime_id TEXT NOT NULL,
    worker_id TEXT NOT NULL,
    is_job INTEGER NOT NULL CHECK(is_job IN (0,1)),
    -- Exact canonical success already present at prepare, never a later receipt.
    prior_removal_operation_id TEXT REFERENCES worker_removal_operations(operation_id),
    -- Confirmed by the scoped Runtime storage owner, not catalog absence or Stop ACK.
    originals_absent INTEGER NOT NULL DEFAULT 0 CHECK(originals_absent IN (0,1)),
    PRIMARY KEY(workspace_id,subject_id,runtime_id,worker_id),
    FOREIGN KEY(workspace_id,subject_id) REFERENCES subject_deletions(workspace_id,subject_id) ON DELETE CASCADE
);
CREATE TRIGGER subject_deletion_worker_prior_insert BEFORE INSERT ON subject_deletion_workers
WHEN NEW.prior_removal_operation_id IS NOT NULL AND NOT EXISTS(
 SELECT 1 FROM worker_removal_operations r WHERE r.operation_id=NEW.prior_removal_operation_id
 AND r.workspace_id=NEW.workspace_id AND r.runtime_id=NEW.runtime_id
 AND r.worker_id=NEW.worker_id AND r.state='succeeded')
BEGIN SELECT RAISE(ABORT,'Subject deletion prior removal must be an exact canonical success'); END;
CREATE TRIGGER subject_deletion_worker_identity_immutable BEFORE UPDATE OF
 workspace_id,subject_id,runtime_id,worker_id,prior_removal_operation_id ON subject_deletion_workers
BEGIN SELECT RAISE(ABORT,'Subject deletion Worker snapshot identity is immutable'); END;
CREATE TRIGGER subject_deletion_original_absence_monotonic BEFORE UPDATE OF originals_absent ON subject_deletion_workers
WHEN OLD.originals_absent=1 AND NEW.originals_absent=0
BEGIN SELECT RAISE(ABORT,'Subject deletion original absence proof is permanent'); END;
-- Job/attempt identities remain after payload erasure, preventing stale allocation
-- or caller-selected Job-id reuse from recreating the deleted Subject's inputs.
CREATE TABLE subject_deletion_jobs (
    workspace_id TEXT NOT NULL,
    subject_id TEXT NOT NULL,
    job_id TEXT NOT NULL,
    PRIMARY KEY(workspace_id,job_id),
    FOREIGN KEY(workspace_id,subject_id) REFERENCES subject_deletions(workspace_id,subject_id) ON DELETE CASCADE
);
CREATE TABLE subject_deletion_allocations (
    workspace_id TEXT NOT NULL,
    subject_id TEXT NOT NULL,
    allocation_key TEXT NOT NULL,
    PRIMARY KEY(workspace_id,allocation_key),
    FOREIGN KEY(workspace_id,subject_id) REFERENCES subject_deletions(workspace_id,subject_id) ON DELETE CASCADE
);
CREATE TRIGGER subject_deleted_job_insert BEFORE INSERT ON backend_jobs
WHEN EXISTS(SELECT 1 FROM subject_deletions d WHERE d.workspace_id=NEW.workspace_id AND
 (d.subject_id=json_extract(NEW.request_json,'$.grants.subjektiv_journal.subject_id') OR
  d.subject_id=json_extract(NEW.request_json,'$.grants.subjektiv_consolidation.subject_id')))
 OR EXISTS(SELECT 1 FROM subject_deletion_jobs d WHERE d.workspace_id=NEW.workspace_id AND d.job_id=NEW.job_id)
BEGIN SELECT RAISE(ABORT,'subject_deleted: Job admission fenced'); END;
CREATE TRIGGER subject_deleted_job_result BEFORE UPDATE OF state,result_json ON backend_jobs
WHEN NEW.state!='failed' AND EXISTS(SELECT 1 FROM subject_deletion_jobs d WHERE d.workspace_id=NEW.workspace_id AND d.job_id=NEW.job_id)
BEGIN SELECT RAISE(ABORT,'subject_deleted: Job result fenced'); END;
CREATE TRIGGER subject_deleted_attempt_insert BEFORE INSERT ON backend_job_attempts
WHEN EXISTS(SELECT 1 FROM subject_deletion_jobs d WHERE d.workspace_id=NEW.workspace_id AND d.job_id=NEW.job_id)
BEGIN SELECT RAISE(ABORT,'subject_deleted: Job retry fenced'); END;
CREATE TRIGGER subject_deleted_attempt_result BEFORE UPDATE OF state,result_json,runtime_id,worker_id,runtime_run_id ON backend_job_attempts
WHEN NEW.state!='failed' AND EXISTS(SELECT 1 FROM subject_deletion_jobs d WHERE d.workspace_id=NEW.workspace_id AND d.job_id=NEW.job_id)
BEGIN SELECT RAISE(ABORT,'subject_deleted: Job attempt fenced'); END;
CREATE TRIGGER subject_deleted_worker_reservation BEFORE INSERT ON worker_create_reservations
WHEN EXISTS(SELECT 1 FROM subject_deletions d WHERE d.workspace_id=NEW.workspace_id AND
 NEW.singleton_key IN ('subjektiv:'||d.subject_id,'subjektiv-consolidation:'||d.subject_id))
 OR EXISTS(SELECT 1 FROM subject_deletion_allocations d WHERE d.workspace_id=NEW.workspace_id AND d.allocation_key=NEW.allocation_key)
BEGIN SELECT RAISE(ABORT,'subject_deleted: Worker allocation fenced'); END;
