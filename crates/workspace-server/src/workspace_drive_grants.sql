CREATE TABLE workspace_drive_grants (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    grant_id INTEGER PRIMARY KEY AUTOINCREMENT,
    runtime_id TEXT NOT NULL,
    worker_id TEXT NOT NULL,
    access TEXT NOT NULL CHECK(access IN ('read_only', 'read_write')),
    revoked INTEGER NOT NULL DEFAULT 0 CHECK(revoked IN (0, 1)),
    created_by TEXT NOT NULL,
    created_at TEXT NOT NULL,
    revoked_by TEXT,
    revoked_at TEXT,
    CHECK((revoked=0 AND revoked_by IS NULL AND revoked_at IS NULL)
       OR (revoked=1 AND revoked_by IS NOT NULL AND revoked_at IS NOT NULL))
);
CREATE UNIQUE INDEX workspace_drive_grants_active_worker
    ON workspace_drive_grants(workspace_id, runtime_id, worker_id) WHERE revoked=0;
-- Retain grant audit across Worker deletion, but never carry its authority into
-- a later lifetime reusing the same Runtime/Worker identity.
CREATE TRIGGER workspace_drive_grants_worker_deleted BEFORE DELETE ON worker_registry BEGIN
    UPDATE workspace_drive_grants SET revoked=1, revoked_by='server:worker-removal',
        revoked_at=strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
    WHERE workspace_id=OLD.workspace_id AND runtime_id=OLD.runtime_id
      AND worker_id=OLD.worker_id AND revoked=0;
END;
