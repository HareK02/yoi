CREATE TABLE subject_permission_operations (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE RESTRICT,
    operation_id TEXT NOT NULL,
    subject_id TEXT NOT NULL,
    account_id TEXT NOT NULL,
    request_json TEXT NOT NULL,
    result_json TEXT NOT NULL,
    PRIMARY KEY(workspace_id, operation_id)
);
CREATE INDEX subject_permission_operations_subject
    ON subject_permission_operations(workspace_id, subject_id);
CREATE TABLE subject_permission_enrollments (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE RESTRICT,
    subject_id TEXT NOT NULL,
    source_json TEXT NOT NULL,
    PRIMARY KEY(workspace_id, subject_id)
);
CREATE TABLE subject_permission_tombstones (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE RESTRICT,
    subject_id TEXT NOT NULL,
    change_id TEXT NOT NULL,
    PRIMARY KEY(workspace_id, subject_id)
);
CREATE TRIGGER subject_permission_operations_no_update BEFORE UPDATE ON subject_permission_operations BEGIN
    SELECT RAISE(ABORT, 'Subject permission operations are immutable');
END;
CREATE TRIGGER subject_permission_operations_no_delete BEFORE DELETE ON subject_permission_operations BEGIN
    SELECT RAISE(ABORT, 'Subject permission operations are retained');
END;
CREATE TRIGGER subject_permission_enrollments_no_update BEFORE UPDATE ON subject_permission_enrollments BEGIN
    SELECT RAISE(ABORT, 'Subject permission enrollments are immutable');
END;
CREATE TRIGGER subject_permission_enrollments_no_delete BEFORE DELETE ON subject_permission_enrollments BEGIN
    SELECT RAISE(ABORT, 'Subject permission enrollments are retained');
END;
