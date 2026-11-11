CREATE TABLE workspace_configuration_policies (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE RESTRICT,
    subject_id TEXT NOT NULL,
    policy_json TEXT NOT NULL,
    PRIMARY KEY(workspace_id, subject_id)
);
CREATE TABLE workspace_configuration_receipts (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE RESTRICT,
    operation_id TEXT NOT NULL,
    request_fingerprint TEXT NOT NULL,
    result_json TEXT NOT NULL,
    PRIMARY KEY(workspace_id, operation_id)
);
CREATE TRIGGER workspace_configuration_receipts_no_update BEFORE UPDATE ON workspace_configuration_receipts BEGIN
    SELECT RAISE(ABORT, 'configuration adoption receipts are immutable');
END;
CREATE TRIGGER workspace_configuration_receipts_no_delete BEFORE DELETE ON workspace_configuration_receipts BEGIN
    SELECT RAISE(ABORT, 'configuration adoption receipts are retained');
END;
