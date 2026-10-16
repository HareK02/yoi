CREATE TABLE subject_worker_releases (
    workspace_id TEXT NOT NULL,
    runtime_id TEXT NOT NULL,
    worker_id TEXT NOT NULL,
    singleton_key TEXT NOT NULL,
    singleton_generation INTEGER NOT NULL CHECK (singleton_generation > 0),
    binding_id TEXT NOT NULL,
    operation_id TEXT NOT NULL,
    proof TEXT CHECK (proof IN ('runtime_disconnected','erased')),
    completed INTEGER NOT NULL DEFAULT 0 CHECK (completed IN (0,1)),
    cancelled INTEGER NOT NULL DEFAULT 0 CHECK (cancelled IN (0,1)),
    PRIMARY KEY (workspace_id, operation_id),
    CHECK (cancelled = 0 OR (completed = 0 AND proof IS NULL)),
    FOREIGN KEY (workspace_id, worker_id) REFERENCES worker_create_reservations(workspace_id, worker_id) ON DELETE CASCADE
);
CREATE UNIQUE INDEX subject_worker_release_active ON subject_worker_releases(workspace_id,worker_id) WHERE cancelled=0;
