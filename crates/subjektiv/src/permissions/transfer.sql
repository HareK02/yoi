-- Permission transfer domain v1, independent of published common migration numbers.
-- This receipt is preparation evidence only, never Server configuration adoption.
CREATE TABLE permission_source_transfer_receipt (
    singleton INTEGER PRIMARY KEY CHECK(singleton=1),
    schema_version INTEGER NOT NULL CHECK(schema_version=1),
    receipt_json TEXT NOT NULL
);
CREATE TRIGGER permission_source_transfer_no_replace BEFORE INSERT ON permission_source_transfer_receipt
WHEN EXISTS(SELECT 1 FROM permission_source_transfer_receipt)
BEGIN SELECT RAISE(ABORT, 'permission transfer receipt already exists'); END;
CREATE TRIGGER permission_source_transfer_no_update BEFORE UPDATE ON permission_source_transfer_receipt
BEGIN SELECT RAISE(ABORT, 'permission transfer receipt is immutable'); END;
CREATE TRIGGER permission_source_transfer_no_delete BEFORE DELETE ON permission_source_transfer_receipt
BEGIN SELECT RAISE(ABORT, 'permission transfer receipt is retained'); END;
-- Published Permission18's UPDATE/DELETE guards alone do not fence INSERT OR REPLACE.
CREATE TRIGGER permission_authority_seal_no_replace BEFORE INSERT ON permission_authority_seal
WHEN EXISTS(SELECT 1 FROM permission_authority_seal)
BEGIN SELECT RAISE(ABORT, 'permission authority seal already exists'); END;
CREATE TRIGGER subject_creation_receipts_no_replace BEFORE INSERT ON subject_creation_receipts
WHEN EXISTS(SELECT 1 FROM subject_creation_receipts WHERE operation_id=NEW.operation_id OR subject_id=NEW.subject_id)
BEGIN SELECT RAISE(ABORT, 'Subject creation receipt already exists'); END;

-- Execution binding evidence only, not a policy mirror. Canonical source IDs cannot reference
-- legacy subject_permission_changes. Preserve that table/FK and all old grant rows unchanged.
CREATE TABLE canonical_worker_execution_grants (
    runtime_id TEXT NOT NULL,
    worker_id TEXT NOT NULL,
    subject_id TEXT NOT NULL REFERENCES subjects(subject_id) ON DELETE RESTRICT,
    source_change_id TEXT NOT NULL,
    record_json TEXT NOT NULL,
    PRIMARY KEY(runtime_id,worker_id)
);
CREATE TRIGGER canonical_worker_execution_grants_no_replace BEFORE INSERT ON canonical_worker_execution_grants
WHEN EXISTS(SELECT 1 FROM canonical_worker_execution_grants WHERE runtime_id=NEW.runtime_id AND worker_id=NEW.worker_id)
 OR EXISTS(SELECT 1 FROM worker_execution_grants WHERE runtime_id=NEW.runtime_id AND worker_id=NEW.worker_id)
BEGIN SELECT RAISE(ABORT, 'Worker execution grant already exists'); END;
CREATE TRIGGER canonical_worker_execution_grants_no_update BEFORE UPDATE ON canonical_worker_execution_grants
BEGIN SELECT RAISE(ABORT, 'Worker execution grants are immutable'); END;
CREATE TRIGGER canonical_worker_execution_grants_no_delete BEFORE DELETE ON canonical_worker_execution_grants
WHEN NOT EXISTS(SELECT 1 FROM subject_deletions WHERE subject_id=OLD.subject_id)
BEGIN SELECT RAISE(ABORT, 'Worker execution grants are retained'); END;
CREATE TRIGGER canonical_worker_execution_grants_insert_guard BEFORE INSERT ON canonical_worker_execution_grants
WHEN NOT EXISTS(SELECT 1 FROM permission_authority_seal)
 OR EXISTS(SELECT 1 FROM subject_deletions WHERE subject_id=NEW.subject_id)
 OR EXISTS(SELECT 1 FROM subject_deleted_workers WHERE
    (runtime_id=NEW.runtime_id AND worker_id=NEW.worker_id)
    OR (runtime_id=json_extract(NEW.record_json,'$.origin_runtime_id')
        AND worker_id=json_extract(NEW.record_json,'$.origin_worker_id')))
BEGIN SELECT RAISE(ABORT, 'canonical grant requires sealed authority and live Subject execution identities'); END;
