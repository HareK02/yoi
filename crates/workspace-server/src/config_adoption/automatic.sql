CREATE TABLE workspace_permission_migration_plans (
    workspace_id TEXT PRIMARY KEY REFERENCES workspaces(workspace_id) ON DELETE RESTRICT,
    transfer_id TEXT NOT NULL,
    plan_json TEXT NOT NULL
);
