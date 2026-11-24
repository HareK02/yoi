CREATE TABLE workspace_members (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    account_id TEXT NOT NULL REFERENCES accounts(account_id) ON DELETE CASCADE,
    membership_key TEXT NOT NULL,
    PRIMARY KEY (workspace_id, account_id)
);
CREATE TABLE workspace_membership_changes (
    workspace_id TEXT PRIMARY KEY REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    last_operation_key TEXT NOT NULL
);
CREATE TABLE workspace_membership_operations (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    operation_key TEXT NOT NULL,
    actor_account_id TEXT NOT NULL,
    request_json TEXT NOT NULL,
    receipt_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (workspace_id, operation_key)
);
