CREATE TABLE plugin_execution_plans (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    plan_id TEXT NOT NULL,
    actor TEXT NOT NULL CHECK(length(actor) BETWEEN 1 AND 512),
    operation_key TEXT NOT NULL CHECK(length(operation_key) BETWEEN 1 AND 128),
    fingerprint TEXT NOT NULL CHECK(length(fingerprint)=71 AND substr(fingerprint,1,7)='sha256:' AND substr(fingerprint,8) NOT GLOB '*[^0-9a-f]*'),
    plan_json TEXT NOT NULL CHECK(json_valid(plan_json)),
    pin_state TEXT NOT NULL CHECK(pin_state IN ('pending','active','restorable','released')),
    PRIMARY KEY(workspace_id, plan_id),
    UNIQUE(workspace_id, actor, operation_key)
);
CREATE TRIGGER plugin_plan_snapshot_immutable
BEFORE UPDATE OF workspace_id,plan_id,actor,operation_key,fingerprint,plan_json ON plugin_execution_plans
BEGIN SELECT RAISE(ABORT,'Plugin execution plan is immutable'); END;
CREATE TRIGGER plugin_plan_retention_terminal BEFORE UPDATE OF pin_state ON plugin_execution_plans
WHEN NOT (
    OLD.pin_state=NEW.pin_state OR
    (OLD.pin_state='pending' AND NEW.pin_state IN ('active','released')) OR
    (OLD.pin_state='active' AND NEW.pin_state IN ('restorable','released')) OR
    (OLD.pin_state='restorable' AND NEW.pin_state IN ('active','released'))
)
BEGIN SELECT RAISE(ABORT,'invalid Plugin execution plan pin transition'); END;
-- Preserve the immutable create replay after retention release. Workspace deletion
-- may cascade; deleting an individual receipt must not make its key reusable.
CREATE TRIGGER plugin_plan_replay_retained BEFORE DELETE ON plugin_execution_plans
WHEN EXISTS(SELECT 1 FROM workspaces WHERE workspace_id=OLD.workspace_id)
BEGIN SELECT RAISE(ABORT,'Plugin execution plan replay must be retained'); END;
