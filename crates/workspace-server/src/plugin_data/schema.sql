-- Ticket children survive parent deletion for explicit Host retention/purge.
-- No guest SQL access; caller and triggers share the Ticket owner's DB fence.
CREATE TABLE plugin_data_cells (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    installation_id TEXT NOT NULL,
    ticket_id TEXT NOT NULL,
    collection TEXT NOT NULL,
    schema_id TEXT NOT NULL,
    schema_version TEXT NOT NULL,
    cell_key TEXT NOT NULL CHECK(length(CAST(cell_key AS BLOB)) BETWEEN 1 AND 512),
    last_write TEXT NOT NULL UNIQUE,
    value_json TEXT CHECK(value_json IS NULL OR (json_valid(value_json) AND length(CAST(value_json AS BLOB))<=65536)),
    PRIMARY KEY(workspace_id,installation_id,ticket_id,collection,schema_id,schema_version,cell_key),
    FOREIGN KEY(workspace_id,installation_id) REFERENCES plugin_installations(workspace_id,installation_id)
);
CREATE TRIGGER plugin_data_parent_insert BEFORE INSERT ON plugin_data_cells
WHEN NOT EXISTS(SELECT 1 FROM typed_tickets t JOIN workspaces w USING(workspace_id)
 WHERE t.workspace_id=NEW.workspace_id AND t.ticket_id=NEW.ticket_id AND w.state='active')
BEGIN SELECT RAISE(ABORT,'Plugin data parent unavailable'); END;
CREATE TRIGGER plugin_data_parent_update BEFORE UPDATE ON plugin_data_cells
WHEN NOT EXISTS(SELECT 1 FROM typed_tickets t JOIN workspaces w USING(workspace_id)
 WHERE t.workspace_id=NEW.workspace_id AND t.ticket_id=NEW.ticket_id AND w.state='active')
BEGIN SELECT RAISE(ABORT,'Plugin data parent unavailable'); END;

-- Random opaque pagination selectors only, never authorization. Expired rows
-- are discarded; API issuance is bounded per Workspace and cannot extend a
-- traversal deadline. Explicit purge invalidates installation selectors.
CREATE TABLE plugin_data_cursors (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    installation_id TEXT NOT NULL,
    cursor_token TEXT NOT NULL CHECK(length(cursor_token)=68 AND substr(cursor_token,1,4)='pdc-' AND substr(cursor_token,5) NOT GLOB '*[^0-9a-f]*'),
    namespace_digest TEXT NOT NULL CHECK(length(namespace_digest)=71),
    after_key TEXT NOT NULL CHECK(length(CAST(after_key AS BLOB)) BETWEEN 1 AND 256),
    expires_at_ms INTEGER NOT NULL CHECK(expires_at_ms>0),
    PRIMARY KEY(workspace_id,cursor_token),
    UNIQUE(workspace_id,namespace_digest,after_key,expires_at_ms),
    FOREIGN KEY(workspace_id,installation_id) REFERENCES plugin_installations(workspace_id,installation_id)
);
CREATE INDEX plugin_data_cursor_expiry ON plugin_data_cursors(workspace_id,expires_at_ms);

-- Conservatively retain each writing artifact until explicit installation purge,
-- even when a later artifact overwrites its cell. owner is exact namespace hash.
CREATE TABLE plugin_data_holders (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    installation_id TEXT NOT NULL,
    owner TEXT NOT NULL,
    artifact_digest TEXT NOT NULL,
    PRIMARY KEY(workspace_id,installation_id,owner,artifact_digest),
    FOREIGN KEY(workspace_id,installation_id,artifact_digest) REFERENCES plugin_artifacts(workspace_id,installation_id,artifact_digest)
);
CREATE TABLE plugin_data_receipts (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    invocation_id TEXT NOT NULL,
    operation_key TEXT NOT NULL CHECK(length(CAST(operation_key AS BLOB)) BETWEEN 1 AND 256),
    namespace_json TEXT NOT NULL CHECK(json_valid(namespace_json)),
    fingerprint TEXT NOT NULL CHECK(length(fingerprint)=71),
    mutation_id TEXT NOT NULL UNIQUE,
    result_digest TEXT NOT NULL CHECK(length(result_digest)=71),
    result_json TEXT CHECK(result_json IS NULL OR (json_valid(result_json) AND length(CAST(result_json AS BLOB))<=262144)),
    PRIMARY KEY(workspace_id,invocation_id,operation_key),
    FOREIGN KEY(workspace_id,invocation_id) REFERENCES plugin_invocations(workspace_id,invocation_id)
);
CREATE TRIGGER plugin_data_receipt_immutable BEFORE UPDATE ON plugin_data_receipts
WHEN NOT(OLD.result_json IS NOT NULL AND NEW.result_json IS NULL
 AND NEW.workspace_id=OLD.workspace_id AND NEW.invocation_id=OLD.invocation_id
 AND NEW.operation_key=OLD.operation_key AND NEW.namespace_json=OLD.namespace_json
 AND NEW.fingerprint=OLD.fingerprint AND NEW.mutation_id=OLD.mutation_id AND NEW.result_digest=OLD.result_digest
 AND EXISTS(SELECT 1 FROM plugin_invocations i WHERE i.workspace_id=OLD.workspace_id AND i.invocation_id=OLD.invocation_id AND i.expired=1))
BEGIN SELECT RAISE(ABORT,'Plugin data receipt immutable'); END;
CREATE TRIGGER plugin_data_receipt_no_replace BEFORE INSERT ON plugin_data_receipts
WHEN EXISTS(SELECT 1 FROM plugin_data_receipts WHERE (workspace_id=NEW.workspace_id AND invocation_id=NEW.invocation_id AND operation_key=NEW.operation_key) OR mutation_id=NEW.mutation_id)
BEGIN SELECT RAISE(ABORT,'Plugin data operation key retained'); END;
CREATE TRIGGER plugin_data_receipt_retained BEFORE DELETE ON plugin_data_receipts
WHEN EXISTS(SELECT 1 FROM workspaces WHERE workspace_id=OLD.workspace_id)
BEGIN SELECT RAISE(ABORT,'Plugin data receipt retained'); END;
CREATE TRIGGER plugin_data_receipt_expiry AFTER UPDATE OF expired ON plugin_invocations
WHEN NEW.expired=1 AND OLD.expired=0
BEGIN UPDATE plugin_data_receipts SET result_json=NULL WHERE workspace_id=NEW.workspace_id AND invocation_id=NEW.invocation_id AND result_json IS NOT NULL; END;

CREATE TRIGGER plugin_effect_data_collision BEFORE INSERT ON plugin_effect_receipts
WHEN EXISTS(SELECT 1 FROM plugin_data_receipts WHERE workspace_id=NEW.workspace_id AND invocation_id=NEW.invocation_id AND operation_key=NEW.effect_key)
BEGIN SELECT RAISE(ABORT,'Plugin operation key belongs to data'); END;
CREATE TRIGGER plugin_data_effect_collision BEFORE INSERT ON plugin_data_receipts
WHEN EXISTS(SELECT 1 FROM plugin_effect_receipts WHERE workspace_id=NEW.workspace_id AND invocation_id=NEW.invocation_id AND effect_key=NEW.operation_key)
BEGIN SELECT RAISE(ABORT,'Plugin operation key belongs to HTTP'); END;

-- Admin operations are not guest invocations; this is only an atomic purge
-- receipt, never an execution identity or saved authorization decision.
CREATE TABLE plugin_data_purges (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    actor TEXT NOT NULL,
    operation_key TEXT NOT NULL CHECK(length(CAST(operation_key AS BLOB)) BETWEEN 1 AND 256),
    installation_id TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    result_json TEXT NOT NULL CHECK(json_valid(result_json)),
    PRIMARY KEY(workspace_id,actor,operation_key)
);
CREATE TRIGGER plugin_data_purge_immutable BEFORE UPDATE ON plugin_data_purges
BEGIN SELECT RAISE(ABORT,'Plugin purge receipt immutable'); END;
CREATE TRIGGER plugin_data_purge_no_replace BEFORE INSERT ON plugin_data_purges
WHEN EXISTS(SELECT 1 FROM plugin_data_purges WHERE workspace_id=NEW.workspace_id AND actor=NEW.actor AND operation_key=NEW.operation_key)
BEGIN SELECT RAISE(ABORT,'Plugin purge key retained'); END;
CREATE TRIGGER plugin_data_purge_retained BEFORE DELETE ON plugin_data_purges
WHEN EXISTS(SELECT 1 FROM workspaces WHERE workspace_id=OLD.workspace_id)
BEGIN SELECT RAISE(ABORT,'Plugin purge receipt retained'); END;
