CREATE TABLE subject_permission_enrollment_sources (
    subject_id TEXT PRIMARY KEY,
    policy_json TEXT NOT NULL
);
CREATE TRIGGER subject_permission_enrollment_sources_no_update BEFORE UPDATE ON subject_permission_enrollment_sources BEGIN
    SELECT RAISE(ABORT, 'Subject creation enrollment is immutable');
END;
CREATE TRIGGER subject_permission_enrollment_sources_no_delete BEFORE DELETE ON subject_permission_enrollment_sources BEGIN
    SELECT RAISE(ABORT, 'Subject creation enrollment is retained');
END;
CREATE TRIGGER subject_permission_enrollment_sources_no_replace BEFORE INSERT ON subject_permission_enrollment_sources
WHEN EXISTS(SELECT 1 FROM subject_permission_enrollment_sources WHERE subject_id=NEW.subject_id)
BEGIN SELECT RAISE(ABORT, 'Subject creation enrollment already exists'); END;
CREATE TABLE canonical_worker_execution_grants (
    runtime_id TEXT NOT NULL,
    worker_id TEXT NOT NULL,
    subject_id TEXT NOT NULL REFERENCES subjects(subject_id) ON DELETE RESTRICT,
    source_change_id TEXT NOT NULL,
    record_json TEXT NOT NULL,
    PRIMARY KEY(runtime_id,worker_id)
);
CREATE TRIGGER canonical_worker_execution_grants_no_replace BEFORE INSERT ON canonical_worker_execution_grants
WHEN EXISTS(SELECT 1 FROM canonical_worker_execution_grants WHERE runtime_id=NEW.runtime_id AND worker_id=NEW.worker_id)
 OR EXISTS(SELECT 1 FROM worker_execution_grants WHERE runtime_id=NEW.runtime_id AND worker_id=NEW.worker_id)
BEGIN SELECT RAISE(ABORT, 'Worker execution grant already exists'); END;
CREATE TRIGGER canonical_worker_execution_grants_no_update BEFORE UPDATE ON canonical_worker_execution_grants
BEGIN SELECT RAISE(ABORT, 'Worker execution grants are immutable'); END;
CREATE TRIGGER canonical_worker_execution_grants_no_delete BEFORE DELETE ON canonical_worker_execution_grants
WHEN NOT EXISTS(SELECT 1 FROM subject_deletions WHERE subject_id=OLD.subject_id)
BEGIN SELECT RAISE(ABORT, 'Worker execution grants are retained'); END;
CREATE TRIGGER canonical_worker_execution_grants_insert_guard BEFORE INSERT ON canonical_worker_execution_grants
WHEN EXISTS(SELECT 1 FROM subject_deletions WHERE subject_id=NEW.subject_id)
 OR EXISTS(SELECT 1 FROM subject_deleted_workers WHERE
    (runtime_id=NEW.runtime_id AND worker_id=NEW.worker_id)
    OR (runtime_id=json_extract(NEW.record_json,'$.origin_runtime_id')
        AND worker_id=json_extract(NEW.record_json,'$.origin_worker_id')))
BEGIN SELECT RAISE(ABORT, 'Worker grant requires live Subject execution identities'); END;
