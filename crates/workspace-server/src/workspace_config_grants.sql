CREATE TABLE workspace_config_grants (
    workspace_id TEXT NOT NULL,
    grant_id TEXT NOT NULL,
    runtime_id TEXT NOT NULL,
    worker_id TEXT NOT NULL,
    workdir_id TEXT NOT NULL,
    access TEXT NOT NULL CHECK(access IN ('read_only', 'read_write')),
    revoked INTEGER NOT NULL CHECK(revoked IN (0, 1)),
    created_by TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY(workspace_id, grant_id),
    UNIQUE(workspace_id, workdir_id),
    FOREIGN KEY(workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE
);
CREATE UNIQUE INDEX workspace_config_grants_active_worker
    ON workspace_config_grants(workspace_id, runtime_id, worker_id) WHERE revoked = 0;
