CREATE TABLE subject_permission_changes (
    subject_id TEXT NOT NULL REFERENCES subjects(subject_id) ON DELETE RESTRICT,
    change_id TEXT NOT NULL CHECK (length(change_id) > 0),
    record_json TEXT NOT NULL,
    PRIMARY KEY (subject_id, change_id)
);
CREATE TABLE subject_permissions (
    subject_id TEXT PRIMARY KEY REFERENCES subjects(subject_id) ON DELETE RESTRICT,
    change_id TEXT NOT NULL,
    FOREIGN KEY (subject_id, change_id) REFERENCES subject_permission_changes(subject_id, change_id) ON DELETE RESTRICT
);
CREATE TABLE subject_permission_receipts (
    operation_id TEXT PRIMARY KEY CHECK (length(operation_id) > 0),
    subject_id TEXT NOT NULL,
    request_fingerprint TEXT NOT NULL,
    change_id TEXT NOT NULL,
    FOREIGN KEY (subject_id, change_id) REFERENCES subject_permission_changes(subject_id, change_id) ON DELETE RESTRICT
);
CREATE TABLE worker_execution_grants (
    runtime_id TEXT NOT NULL,
    worker_id TEXT NOT NULL,
    subject_id TEXT NOT NULL,
    source_change_id TEXT NOT NULL,
    record_json TEXT NOT NULL,
    PRIMARY KEY (runtime_id, worker_id),
    FOREIGN KEY (subject_id, source_change_id) REFERENCES subject_permission_changes(subject_id, change_id) ON DELETE RESTRICT
);
CREATE TRIGGER subject_permission_changes_no_update BEFORE UPDATE ON subject_permission_changes BEGIN
    SELECT RAISE(ABORT, 'Subject permission changes are immutable');
END;
CREATE TRIGGER subject_permission_changes_no_delete BEFORE DELETE ON subject_permission_changes BEGIN
    SELECT RAISE(ABORT, 'Subject permission changes are retained');
END;
CREATE TRIGGER subject_permission_receipts_no_update BEFORE UPDATE ON subject_permission_receipts BEGIN
    SELECT RAISE(ABORT, 'Subject permission receipts are immutable');
END;
CREATE TRIGGER subject_permission_receipts_no_delete BEFORE DELETE ON subject_permission_receipts BEGIN
    SELECT RAISE(ABORT, 'Subject permission receipts are retained');
END;
CREATE TRIGGER worker_execution_grants_no_update BEFORE UPDATE ON worker_execution_grants BEGIN
    SELECT RAISE(ABORT, 'Worker execution grants are immutable');
END;
CREATE TRIGGER worker_execution_grants_no_delete BEFORE DELETE ON worker_execution_grants BEGIN
    SELECT RAISE(ABORT, 'Worker execution grants are retained');
END;
