CREATE TABLE worker_plugin_launches (
    workspace_id TEXT NOT NULL,
    worker_id TEXT NOT NULL,
    runtime_id TEXT NOT NULL,
    plan_id TEXT,
    PRIMARY KEY(workspace_id, worker_id),
    FOREIGN KEY(workspace_id, worker_id) REFERENCES worker_create_reservations(workspace_id, worker_id) ON DELETE CASCADE,
    FOREIGN KEY(workspace_id, plan_id) REFERENCES plugin_execution_plans(workspace_id, plan_id)
);
CREATE TRIGGER worker_plugin_launch_immutable BEFORE UPDATE ON worker_plugin_launches
BEGIN SELECT RAISE(ABORT,'Worker Plugin launch selection is immutable'); END;
CREATE TRIGGER worker_plugin_launch_retained BEFORE DELETE ON worker_plugin_launches
WHEN EXISTS(SELECT 1 FROM workspaces WHERE workspace_id=OLD.workspace_id)
BEGIN SELECT RAISE(ABORT,'Worker Plugin launch selection must be retained'); END;
CREATE TRIGGER worker_plugin_launch_identity BEFORE INSERT ON worker_plugin_launches
WHEN NOT EXISTS(SELECT 1 FROM worker_create_reservations WHERE workspace_id=NEW.workspace_id AND worker_id=NEW.worker_id AND runtime_id=NEW.runtime_id)
BEGIN SELECT RAISE(ABORT,'Worker Plugin launch reservation identity mismatch'); END;
