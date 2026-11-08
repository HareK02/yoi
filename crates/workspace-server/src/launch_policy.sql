CREATE TABLE worker_launch_policies (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id),
    policy_id TEXT NOT NULL,
    authorization_revision INTEGER NOT NULL CHECK (authorization_revision > 0),
    state TEXT NOT NULL CHECK (state IN ('active', 'revoked', 'deleted')),
    policy_json TEXT NOT NULL,
    last_operation_key TEXT NOT NULL,
    PRIMARY KEY (workspace_id, policy_id)
);

-- Immutable snapshots preserve revoked revisions and never recycle a usage key.
CREATE TABLE worker_launch_policy_history (
    workspace_id TEXT NOT NULL,
    policy_id TEXT NOT NULL,
    operation_key TEXT NOT NULL,
    authorization_revision INTEGER NOT NULL CHECK (authorization_revision > 0),
    policy_json TEXT NOT NULL,
    PRIMARY KEY (workspace_id, operation_key),
    FOREIGN KEY (workspace_id, policy_id) REFERENCES worker_launch_policies(workspace_id, policy_id)
);
CREATE INDEX worker_launch_policy_revision_history ON worker_launch_policy_history(workspace_id, policy_id, authorization_revision);

CREATE TABLE worker_launch_policy_operations (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id),
    operation_key TEXT NOT NULL,
    actor_account_id TEXT NOT NULL,
    request_json TEXT NOT NULL,
    receipt_json TEXT NOT NULL,
    PRIMARY KEY (workspace_id, operation_key)
);

-- Issued records bind reserved identities before Runtime execution. No Subject FK
-- and no duplicate resource attachment ledger: each Feature owns those records.
CREATE TABLE core_worker_execution_grants (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id),
    grant_id TEXT NOT NULL,
    creation_operation_id TEXT NOT NULL,
    runtime_id TEXT NOT NULL,
    worker_id TEXT NOT NULL,
    grant_json TEXT NOT NULL,
    policy_id TEXT,
    policy_revision INTEGER,
    revoked INTEGER NOT NULL DEFAULT 0 CHECK (revoked IN (0, 1)),
    PRIMARY KEY (workspace_id, grant_id),
    UNIQUE (workspace_id, creation_operation_id),
    UNIQUE (workspace_id, runtime_id, worker_id),
    FOREIGN KEY (workspace_id, policy_id) REFERENCES worker_launch_policies(workspace_id, policy_id),
    CHECK ((policy_id IS NULL AND policy_revision IS NULL) OR (policy_id IS NOT NULL AND policy_revision > 0))
);

CREATE TABLE core_worker_grant_revocations (
    workspace_id TEXT NOT NULL,
    operation_key TEXT NOT NULL,
    actor_account_id TEXT NOT NULL,
    grant_id TEXT NOT NULL,
    PRIMARY KEY (workspace_id, operation_key),
    FOREIGN KEY (workspace_id, grant_id) REFERENCES core_worker_execution_grants(workspace_id, grant_id)
);
