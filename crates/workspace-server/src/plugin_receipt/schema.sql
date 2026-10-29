-- Durable Host identity and all-method HTTPS receipts. No request body, raw
-- request headers, secret value, grant authority or catalog pin lives here.
CREATE TABLE plugin_invocations (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    invocation_id TEXT NOT NULL CHECK(length(CAST(invocation_id AS BLOB)) BETWEEN 1 AND 512),
    plan_id TEXT NOT NULL,
    plan_fingerprint TEXT NOT NULL CHECK(length(plan_fingerprint)=71 AND substr(plan_fingerprint,1,7)='sha256:' AND substr(plan_fingerprint,8) NOT GLOB '*[^0-9a-f]*'),
    installation_id TEXT NOT NULL,
    runtime_id TEXT NOT NULL CHECK(length(CAST(runtime_id AS BLOB)) BETWEEN 1 AND 512),
    worker_id TEXT NOT NULL CHECK(length(CAST(worker_id AS BLOB)) BETWEEN 1 AND 512),
    execution_json TEXT NOT NULL CHECK(json_valid(execution_json) AND json_type(execution_json)='object' AND length(CAST(execution_json AS BLOB)) <= 16384),
    consumer_key TEXT NOT NULL CHECK(json_valid(consumer_key) AND json_type(consumer_key)='array' AND length(CAST(consumer_key AS BLOB)) <= 8192),
    consumer_json TEXT NOT NULL CHECK(json_valid(consumer_json) AND json_type(consumer_json)='object' AND length(CAST(consumer_json AS BLOB)) <= 16384),
    created_at_ms INTEGER NOT NULL CHECK(created_at_ms >= 0),
    expires_at_ms INTEGER NOT NULL CHECK(expires_at_ms > created_at_ms),
    cancelled INTEGER NOT NULL DEFAULT 0 CHECK(cancelled IN (0,1)),
    expired INTEGER NOT NULL DEFAULT 0 CHECK(expired IN (0,1)),
    PRIMARY KEY(workspace_id, invocation_id),
    UNIQUE(workspace_id, runtime_id, worker_id, installation_id, consumer_key),
    FOREIGN KEY(workspace_id, plan_id) REFERENCES plugin_execution_plans(workspace_id, plan_id),
    FOREIGN KEY(workspace_id, installation_id) REFERENCES plugin_installations(workspace_id, installation_id)
);
CREATE INDEX plugin_invocation_expiry ON plugin_invocations(workspace_id, expires_at_ms) WHERE expired=0;
CREATE TRIGGER plugin_invocation_immutable
BEFORE UPDATE OF workspace_id,invocation_id,plan_id,plan_fingerprint,installation_id,runtime_id,worker_id,execution_json,consumer_key,consumer_json,created_at_ms,expires_at_ms ON plugin_invocations
BEGIN SELECT RAISE(ABORT,'Plugin invocation identity is immutable'); END;
CREATE TRIGGER plugin_invocation_terminal BEFORE UPDATE OF cancelled,expired ON plugin_invocations
WHEN NEW.cancelled < OLD.cancelled OR NEW.expired < OLD.expired
BEGIN SELECT RAISE(ABORT,'Plugin invocation cancellation and expiry are irreversible'); END;
CREATE TRIGGER plugin_invocation_retained BEFORE DELETE ON plugin_invocations
WHEN EXISTS(SELECT 1 FROM workspaces WHERE workspace_id=OLD.workspace_id)
BEGIN SELECT RAISE(ABORT,'Plugin invocation tombstone must be retained'); END;
-- REPLACE must not bypass tombstones even with recursive_triggers disabled.
CREATE TRIGGER plugin_invocation_no_replace BEFORE INSERT ON plugin_invocations
WHEN EXISTS(SELECT 1 FROM plugin_invocations WHERE workspace_id=NEW.workspace_id AND
 (invocation_id=NEW.invocation_id OR (runtime_id=NEW.runtime_id AND worker_id=NEW.worker_id AND installation_id=NEW.installation_id AND consumer_key=NEW.consumer_key)))
BEGIN SELECT RAISE(ABORT,'Plugin invocation identity cannot be reused'); END;

CREATE TABLE plugin_effect_receipts (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    invocation_id TEXT NOT NULL,
    effect_key TEXT NOT NULL CHECK(length(CAST(effect_key AS BLOB)) BETWEEN 1 AND 128),
    fingerprint TEXT NOT NULL CHECK(length(fingerprint)=71 AND substr(fingerprint,1,7)='sha256:' AND substr(fingerprint,8) NOT GLOB '*[^0-9a-f]*'),
    admitted_at_ms INTEGER NOT NULL CHECK(admitted_at_ms >= 0),
    state TEXT NOT NULL CHECK(state IN ('pending','completed','expired')),
    response_status INTEGER CHECK(response_status BETWEEN 100 AND 599),
    response_headers TEXT CHECK(json_valid(response_headers) AND json_type(response_headers)='array' AND json_array_length(response_headers) <= 128 AND length(CAST(response_headers AS BLOB)) <= 65536),
    response_body BLOB CHECK(response_body IS NULL OR (typeof(response_body)='blob' AND length(response_body) <= 1048576)),
    error_json TEXT CHECK(json_valid(error_json) AND json_type(error_json)='object' AND length(CAST(error_json AS BLOB)) <= 4096),
    CHECK(COALESCE(length(response_body),0)+COALESCE(length(CAST(response_headers AS BLOB)),0) <= 1114112),
    CHECK(
      (state='pending' AND response_status IS NULL AND response_headers IS NULL AND response_body IS NULL AND error_json IS NOT NULL AND COALESCE(json_extract(error_json,'$.code'),'')='indeterminate' AND COALESCE(json_extract(error_json,'$.effect'),'')='indeterminate') OR
      (state='completed' AND ((response_status IS NOT NULL AND response_headers IS NOT NULL AND response_body IS NOT NULL AND error_json IS NULL) OR
                             (response_status IS NULL AND response_headers IS NULL AND response_body IS NULL AND error_json IS NOT NULL))) OR
      (state='expired' AND response_status IS NULL AND response_headers IS NULL AND response_body IS NULL AND error_json IS NULL)
    ),
    PRIMARY KEY(workspace_id, invocation_id, effect_key),
    FOREIGN KEY(workspace_id, invocation_id) REFERENCES plugin_invocations(workspace_id, invocation_id) ON DELETE CASCADE
);
CREATE TRIGGER plugin_effect_immutable BEFORE UPDATE OF workspace_id,invocation_id,effect_key,fingerprint,admitted_at_ms ON plugin_effect_receipts
BEGIN SELECT RAISE(ABORT,'Plugin effect identity is immutable'); END;
CREATE TRIGGER plugin_effect_transition BEFORE UPDATE ON plugin_effect_receipts
WHEN NOT (
 (OLD.state='pending' AND NEW.state='completed') OR
 (OLD.state IN ('pending','completed') AND NEW.state='expired' AND EXISTS(
   SELECT 1 FROM plugin_invocations WHERE workspace_id=OLD.workspace_id AND invocation_id=OLD.invocation_id AND expired=1))
)
BEGIN SELECT RAISE(ABORT,'Plugin effect result is immutable'); END;
CREATE TRIGGER plugin_effect_admission BEFORE INSERT ON plugin_effect_receipts
WHEN NEW.state <> 'pending' OR NOT EXISTS(
 SELECT 1 FROM plugin_invocations WHERE workspace_id=NEW.workspace_id AND invocation_id=NEW.invocation_id
 AND cancelled=0 AND expired=0 AND expires_at_ms > NEW.admitted_at_ms)
BEGIN SELECT RAISE(ABORT,'Plugin effect invocation is not admissible'); END;
CREATE TRIGGER plugin_effect_no_replace BEFORE INSERT ON plugin_effect_receipts
WHEN EXISTS(SELECT 1 FROM plugin_effect_receipts WHERE workspace_id=NEW.workspace_id AND invocation_id=NEW.invocation_id AND effect_key=NEW.effect_key)
BEGIN SELECT RAISE(ABORT,'Plugin effect key cannot be reused'); END;
CREATE TRIGGER plugin_effect_retained BEFORE DELETE ON plugin_effect_receipts
WHEN EXISTS(SELECT 1 FROM workspaces WHERE workspace_id=OLD.workspace_id)
BEGIN SELECT RAISE(ABORT,'Plugin effect tombstone must be retained'); END;
