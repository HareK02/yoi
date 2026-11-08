CREATE TABLE core_worker_launch_bindings (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id),
    operation_id TEXT NOT NULL,
    caller_fingerprint TEXT NOT NULL,
    runtime_id TEXT NOT NULL,
    worker_id TEXT NOT NULL,
    config_bundle_json TEXT NOT NULL,
    PRIMARY KEY (workspace_id, operation_id),
    FOREIGN KEY (workspace_id, runtime_id, worker_id)
        REFERENCES core_worker_execution_grants(workspace_id, runtime_id, worker_id)
);
