CREATE TABLE repository_secret_audit_events (
            workspace_id TEXT NOT NULL,
            event_id TEXT NOT NULL,
            kind TEXT NOT NULL,
            resource_id TEXT NOT NULL,
            operation_id TEXT NOT NULL CHECK (length(operation_id) > 0),
            actor_account_id TEXT NOT NULL,
            created_at TEXT NOT NULL,
            PRIMARY KEY (workspace_id, event_id),
            FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE
        );
CREATE TABLE repository_secret_operations (
            workspace_id TEXT NOT NULL,
            operation_id TEXT NOT NULL,
            request_fingerprint TEXT NOT NULL,
            resource_kind TEXT NOT NULL CHECK (resource_kind IN ('credential', 'host_trust')),
            resource_id TEXT NOT NULL,
            result_operation_id TEXT NOT NULL CHECK (length(result_operation_id) > 0),
            created_at TEXT NOT NULL,
            PRIMARY KEY (workspace_id, operation_id),
            FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE
        );
CREATE TABLE repository_ssh_credential_keys (
            workspace_id TEXT NOT NULL,
            credential_id TEXT NOT NULL,
            operation_id TEXT NOT NULL CHECK (length(operation_id) > 0),
            public_key_algorithm TEXT NOT NULL,
            public_key_fingerprint TEXT NOT NULL,
            created_at TEXT NOT NULL,
            PRIMARY KEY (workspace_id, credential_id, operation_id),
            UNIQUE (workspace_id, credential_id, operation_id, public_key_fingerprint),
            FOREIGN KEY (workspace_id, credential_id)
                REFERENCES repository_ssh_credentials(workspace_id, credential_id)
                ON DELETE CASCADE
        );
CREATE TABLE repository_ssh_credentials (
            workspace_id TEXT NOT NULL,
            credential_id TEXT NOT NULL,
            name TEXT NOT NULL,
            public_key_algorithm TEXT NOT NULL,
            public_key_fingerprint TEXT NOT NULL,
            current_operation_id TEXT NOT NULL CHECK (length(current_operation_id) > 0),
            status TEXT NOT NULL CHECK (status IN ('active', 'revoked')),
            created_at TEXT NOT NULL,
            rotated_at TEXT,
            PRIMARY KEY (workspace_id, credential_id),
            FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE
        );
CREATE TABLE repository_ssh_host_trust_keys (
            workspace_id TEXT NOT NULL,
            host_trust_id TEXT NOT NULL,
            operation_id TEXT NOT NULL CHECK (length(operation_id) > 0),
            hostname TEXT NOT NULL,
            port INTEGER NOT NULL CHECK (port >= 1 AND port <= 65535),
            key_algorithm TEXT NOT NULL,
            host_key TEXT NOT NULL,
            fingerprint TEXT NOT NULL,
            created_at TEXT NOT NULL,
            PRIMARY KEY (workspace_id, host_trust_id, operation_id),
            FOREIGN KEY (workspace_id, host_trust_id)
                REFERENCES repository_ssh_host_trusts(workspace_id, host_trust_id)
                ON DELETE CASCADE
        );
CREATE TABLE repository_ssh_host_trusts (
            workspace_id TEXT NOT NULL,
            host_trust_id TEXT NOT NULL,
            hostname TEXT NOT NULL,
            port INTEGER NOT NULL CHECK (port >= 1 AND port <= 65535),
            key_algorithm TEXT NOT NULL,
            host_key TEXT NOT NULL,
            fingerprint TEXT NOT NULL,
            current_operation_id TEXT NOT NULL CHECK (length(current_operation_id) > 0),
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            PRIMARY KEY (workspace_id, host_trust_id),
            FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE
        );
CREATE TABLE server_secret_objects (
            workspace_id TEXT NOT NULL,
            secret_id TEXT NOT NULL,
            operation_id TEXT NOT NULL CHECK (length(operation_id) > 0),
            purpose TEXT NOT NULL CHECK (purpose IN ('private_key', 'passphrase')),
            encryption_algorithm TEXT NOT NULL CHECK (encryption_algorithm = 'aes-256-gcm-v1'),
            nonce BLOB NOT NULL CHECK (length(nonce) = 12),
            ciphertext BLOB NOT NULL,
            created_at TEXT NOT NULL,
            PRIMARY KEY (workspace_id, secret_id, operation_id, purpose),
            FOREIGN KEY (workspace_id, secret_id, operation_id)
                REFERENCES repository_ssh_credential_keys(workspace_id, credential_id, operation_id)
                ON DELETE CASCADE
        );
CREATE TABLE workdir_create_operations (
            workspace_id TEXT NOT NULL,
            operation_id TEXT NOT NULL,
            request_fingerprint TEXT NOT NULL,
            repository_id TEXT NOT NULL,
            selector TEXT,
            requested_runtime_id TEXT,
            resolved_runtime_id TEXT NOT NULL,
            config_projection_digest TEXT NOT NULL,
            working_directory_id TEXT NOT NULL,
            state TEXT NOT NULL CHECK (state IN ('pending', 'succeeded', 'failed')),
            failure TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL, source_kind TEXT, source_uri TEXT, source_fingerprint TEXT, credential_id TEXT, credential_fingerprint TEXT, host_trust_id TEXT, host_trust_fingerprint TEXT, repository_access_mode TEXT,
            PRIMARY KEY (workspace_id, operation_id),
            UNIQUE (workspace_id, working_directory_id)
        );
CREATE TABLE workdir_create_credential_candidates (
    workspace_id TEXT NOT NULL,
    operation_id TEXT NOT NULL,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0 AND ordinal < 2),
    role TEXT NOT NULL CHECK (role IN ('primary', 'workspace_default_fallback')),
    credential_id TEXT NOT NULL CHECK (length(credential_id) BETWEEN 1 AND 128),
    credential_fingerprint TEXT NOT NULL CHECK (length(credential_fingerprint) > 0),
    PRIMARY KEY (workspace_id, operation_id, ordinal),
    UNIQUE (workspace_id, operation_id, ordinal, credential_id, credential_fingerprint),
    UNIQUE (workspace_id, operation_id, role),
    UNIQUE (workspace_id, operation_id, credential_id),
    FOREIGN KEY (workspace_id, operation_id)
        REFERENCES workdir_create_operations(workspace_id, operation_id)
        ON DELETE CASCADE
);
CREATE INDEX idx_workdir_create_credential_candidates_fingerprint
    ON workdir_create_credential_candidates(
        workspace_id, credential_id, credential_fingerprint
    );
CREATE TABLE workdir_create_credential_retentions (
    workspace_id TEXT NOT NULL,
    operation_id TEXT NOT NULL,
    ordinal INTEGER NOT NULL,
    credential_id TEXT NOT NULL,
    credential_fingerprint TEXT NOT NULL,
    credential_operation_id TEXT NOT NULL,
    PRIMARY KEY (workspace_id, operation_id, ordinal),
    FOREIGN KEY (workspace_id, operation_id, ordinal, credential_id, credential_fingerprint)
        REFERENCES workdir_create_credential_candidates(
            workspace_id, operation_id, ordinal, credential_id, credential_fingerprint
        )
        ON DELETE CASCADE,
    FOREIGN KEY (workspace_id, credential_id, credential_operation_id, credential_fingerprint)
        REFERENCES repository_ssh_credential_keys(workspace_id, credential_id, operation_id, public_key_fingerprint)
        ON DELETE RESTRICT
);
-- Frozen evidence only. Neither table is a key, lease, or mutation authority.
CREATE TABLE workdir_create_legacy_ssh_archives (
    workspace_id TEXT NOT NULL,
    operation_id TEXT NOT NULL,
    operation_json TEXT NOT NULL CHECK (json_valid(operation_json)),
    candidates_json TEXT NOT NULL CHECK (json_valid(candidates_json)),
    PRIMARY KEY (workspace_id, operation_id),
    FOREIGN KEY (workspace_id, operation_id)
        REFERENCES workdir_create_operations(workspace_id, operation_id) ON DELETE CASCADE
);
CREATE TRIGGER workdir_create_legacy_archive_cannot_authorize_update
BEFORE UPDATE ON workdir_create_operations FOR EACH ROW
WHEN EXISTS (SELECT 1 FROM workdir_create_legacy_ssh_archives a
    WHERE a.workspace_id=OLD.workspace_id AND a.operation_id=OLD.operation_id)
AND (NEW.state <> 'succeeded' OR NEW.credential_id IS NOT NULL
    OR NEW.credential_fingerprint IS NOT NULL OR NEW.host_trust_id IS NOT NULL
    OR NEW.host_trust_fingerprint IS NOT NULL OR NEW.repository_access_mode IS NOT NULL)
BEGIN SELECT RAISE(ABORT, 'archived_workdir_create_cannot_authorize'); END;
CREATE TRIGGER workdir_create_legacy_archive_blocks_candidates
BEFORE INSERT ON workdir_create_credential_candidates FOR EACH ROW
WHEN EXISTS (SELECT 1 FROM workdir_create_legacy_ssh_archives a
    WHERE a.workspace_id=NEW.workspace_id AND a.operation_id=NEW.operation_id)
BEGIN SELECT RAISE(ABORT, 'archived_workdir_create_cannot_authorize'); END;
CREATE TRIGGER workdir_create_legacy_archive_blocks_candidate_updates
BEFORE UPDATE ON workdir_create_credential_candidates FOR EACH ROW
WHEN EXISTS (SELECT 1 FROM workdir_create_legacy_ssh_archives a
    WHERE a.workspace_id=NEW.workspace_id AND a.operation_id=NEW.operation_id)
BEGIN SELECT RAISE(ABORT, 'archived_workdir_create_cannot_authorize'); END;
CREATE TABLE repository_secret_legacy_receipts (
    workspace_id TEXT NOT NULL,
    operation_id TEXT NOT NULL,
    request_fingerprint TEXT NOT NULL,
    resource_kind TEXT NOT NULL,
    resource_id TEXT NOT NULL,
    result_revision INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    mutation_kind TEXT,
    expected_revision INTEGER,
    expected_operation_id TEXT,
    expected_key_fingerprint TEXT,
    PRIMARY KEY (workspace_id, operation_id),
    FOREIGN KEY (workspace_id, operation_id)
        REFERENCES repository_secret_operations(workspace_id, operation_id) ON DELETE CASCADE
);
