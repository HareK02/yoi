CREATE TABLE worker_plugin_launches (
    workspace_id TEXT NOT NULL,
    worker_id TEXT NOT NULL,
    runtime_id TEXT NOT NULL,
    plan_id TEXT,
    PRIMARY KEY(workspace_id, worker_id),
    FOREIGN KEY(workspace_id, worker_id) REFERENCES worker_create_reservations(workspace_id, worker_id) ON DELETE CASCADE,
    FOREIGN KEY(workspace_id, plan_id) REFERENCES plugin_execution_plans(workspace_id, plan_id)
);
