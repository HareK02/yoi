-- Canonical composed Workspace Server schema. New databases are created directly at this generation.
-- Ticket creates its standalone schema first; replace those empty tables with the composed authority.
DROP TABLE typed_ticket_event_attributes;
DROP TABLE typed_ticket_event_references;
DROP TABLE typed_ticket_artifacts;
DROP TABLE typed_ticket_labels;
DROP TABLE typed_ticket_orchestration_plans;
DROP TABLE typed_ticket_raw_frontmatter;
DROP TABLE typed_ticket_relations;
DROP TABLE typed_ticket_risk_flags;
DROP TABLE typed_ticket_events;
DROP TABLE typed_tickets;
DROP TABLE workspace_resource_key_counters;
DROP TABLE workspace_resource_keys;
CREATE TABLE accounts (
    account_id TEXT PRIMARY KEY,
    kind TEXT NOT NULL CHECK (kind IN ('user', 'organization')),
    handle TEXT NOT NULL,
    display_name TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE (kind, handle)
);
CREATE TABLE api_tokens (
    token_id TEXT PRIMARY KEY,
    token_hash TEXT NOT NULL UNIQUE,
    user_id TEXT NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    label TEXT NOT NULL,
    created_at TEXT NOT NULL,
    expires_at TEXT,
    revoked_at TEXT,
    last_used_at TEXT
);
CREATE TABLE "artifacts" (
    workspace_id TEXT NOT NULL,
    artifact_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    uri TEXT NOT NULL,
    media_type TEXT,
    sha256 TEXT,
    size_bytes INTEGER,
    summary TEXT,
    created_at TEXT NOT NULL,
    created_by_kind TEXT NOT NULL,
    created_by_key TEXT NOT NULL,
    created_by_display TEXT NOT NULL,
    created_by_source_kind TEXT,
    created_by_source_key TEXT,
    ticket_id TEXT,
    objective_id TEXT,
    event_id TEXT,
    worker_ref_kind TEXT,
    worker_ref_key TEXT,
    worker_display TEXT,
    repository_id TEXT,
    source_kind TEXT,
    source_reference TEXT,
    PRIMARY KEY (workspace_id, artifact_id),
    CHECK ((worker_ref_kind IS NULL) = (worker_ref_key IS NULL)),
    FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    FOREIGN KEY (workspace_id, ticket_id)
        REFERENCES typed_tickets(workspace_id, ticket_id) ON DELETE CASCADE,
    FOREIGN KEY (workspace_id, objective_id)
        REFERENCES objectives(workspace_id, objective_id) ON DELETE CASCADE,
    FOREIGN KEY (workspace_id, repository_id)
        REFERENCES repositories(workspace_id, repository_id) ON DELETE RESTRICT
);
CREATE TABLE audit_events (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    audit_event_id TEXT PRIMARY KEY,
    created_at TEXT NOT NULL,
    actor_kind TEXT NOT NULL,
    actor_key TEXT NOT NULL,
    actor_display TEXT NOT NULL,
    actor_source_kind TEXT,
    actor_source_key TEXT,
    action TEXT NOT NULL,
    target_kind TEXT NOT NULL,
    target_id TEXT,
    outcome TEXT NOT NULL,
    request_id TEXT,
    summary TEXT
);
CREATE TABLE auth_challenges (
    challenge_id TEXT PRIMARY KEY,
    ceremony TEXT NOT NULL CHECK (ceremony IN ('passkey_registration', 'passkey_login')),
    challenge TEXT NOT NULL UNIQUE,
    user_id TEXT REFERENCES users(user_id) ON DELETE CASCADE,
    rp_id TEXT NOT NULL,
    origin TEXT NOT NULL,
    state_json TEXT,
    expires_at TEXT NOT NULL,
    created_at TEXT NOT NULL,
    consumed_at TEXT
);
CREATE TABLE browser_sessions (
    session_id TEXT PRIMARY KEY,
    token_hash TEXT NOT NULL UNIQUE,
    user_id TEXT NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    created_at TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    revoked_at TEXT
);
CREATE TABLE device_login_flows (
    device_code TEXT PRIMARY KEY,
    user_code TEXT NOT NULL UNIQUE,
    verification_uri TEXT NOT NULL,
    client_name TEXT,
    user_id TEXT REFERENCES users(user_id) ON DELETE SET NULL,
    api_token_id TEXT REFERENCES api_tokens(token_id) ON DELETE SET NULL,
    issued_access_token TEXT,
    created_at TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    approved_at TEXT,
    consumed_at TEXT
);
CREATE TABLE flow_source_contents (
    workspace_id TEXT NOT NULL,
    flow_id TEXT NOT NULL,
    content TEXT NOT NULL,
    content_digest TEXT NOT NULL,
    definition_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (workspace_id, flow_id, content_digest),
    FOREIGN KEY (workspace_id, flow_id)
        REFERENCES flow_sources(workspace_id, flow_id) ON DELETE CASCADE
);
CREATE TABLE flow_sources (
    workspace_id TEXT NOT NULL,
    flow_id TEXT NOT NULL,
    source_kind TEXT NOT NULL CHECK (source_kind IN ('builtin', 'workspace')),
    name TEXT NOT NULL,
    path TEXT NOT NULL,
    content TEXT NOT NULL,
    content_digest TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (workspace_id, flow_id),
    UNIQUE (workspace_id, source_kind, name),
    FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE
);
CREATE TABLE memory_staging_records (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    candidate_id TEXT NOT NULL,
    raw_json TEXT NOT NULL,
    source_path TEXT,
    imported_at TEXT NOT NULL,
    PRIMARY KEY (workspace_id, candidate_id)
);
CREATE TABLE memory_staging_resolutions (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    candidate_id TEXT NOT NULL,
    action TEXT NOT NULL,
    reason TEXT NOT NULL,
    affected_refs_json TEXT NOT NULL,
    staging_raw_json TEXT NOT NULL,
    source_path TEXT,
    imported_at TEXT NOT NULL,
    resolved_at TEXT NOT NULL,
    PRIMARY KEY (workspace_id, candidate_id, resolved_at)
);
CREATE TABLE "objective_events" (
    workspace_id TEXT NOT NULL,
    objective_id TEXT NOT NULL,
    event_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    body_md TEXT,
    created_at TEXT NOT NULL,
    PRIMARY KEY (workspace_id, event_id),
    FOREIGN KEY (workspace_id, objective_id)
        REFERENCES objectives(workspace_id, objective_id) ON DELETE CASCADE
);
CREATE TABLE "objective_resources" (
    workspace_id TEXT NOT NULL,
    objective_id TEXT NOT NULL,
    resource_path TEXT NOT NULL,
    body TEXT NOT NULL,
    media_type TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (workspace_id, objective_id, resource_path),
    FOREIGN KEY (workspace_id, objective_id)
        REFERENCES objectives(workspace_id, objective_id) ON DELETE CASCADE
);
CREATE TABLE "objective_ticket_links" (
    workspace_id TEXT NOT NULL,
    objective_id TEXT NOT NULL,
    ticket_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (workspace_id, objective_id, ticket_id, kind),
    FOREIGN KEY (workspace_id, objective_id)
        REFERENCES objectives(workspace_id, objective_id) ON DELETE CASCADE,
    FOREIGN KEY (workspace_id, ticket_id)
        REFERENCES typed_tickets(workspace_id, ticket_id) ON DELETE CASCADE
);
CREATE TABLE "objectives" (
    workspace_id TEXT NOT NULL,
    objective_id TEXT NOT NULL,
    title TEXT NOT NULL,
    state TEXT NOT NULL,
    body_md TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (workspace_id, objective_id),
    FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE
);
CREATE TABLE passkey_credentials (
    credential_id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    public_key_cose TEXT NOT NULL,
    transports_json TEXT,
    sign_count INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    last_used_at TEXT
);
CREATE TABLE "repositories" (
            workspace_id TEXT NOT NULL,
            repository_id TEXT NOT NULL PRIMARY KEY,
            repository_key TEXT NOT NULL
                CHECK(length(repository_key) BETWEEN 1 AND 64)
                CHECK(repository_key NOT GLOB '*[^a-z0-9-]*')
                CHECK(substr(repository_key, 1, 1) <> '-')
                CHECK(substr(repository_key, -1, 1) <> '-'),
            kind TEXT NOT NULL,
            provider TEXT,
            uri TEXT NOT NULL,
            default_ref TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            source_kind TEXT NOT NULL,
            source_uri TEXT NOT NULL,
            source_fingerprint TEXT NOT NULL,
            observed_status TEXT NOT NULL DEFAULT 'unverified',
            observed_at TEXT,
            UNIQUE(workspace_id, repository_key),
            UNIQUE(workspace_id, repository_id),
            FOREIGN KEY(workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE
        );
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
CREATE TABLE ticket_assignment_operations (
            workspace_id TEXT NOT NULL,
            operation_id TEXT NOT NULL,
            action TEXT NOT NULL CHECK (action IN ('assign', 'reassign', 'unassign')),
            ticket_id TEXT NOT NULL,
            role TEXT NOT NULL DEFAULT 'worker'
                CHECK(role IN ('orchestrator', 'worker', 'owner', 'contributor')),
            principal_kind TEXT NOT NULL DEFAULT 'worker'
                CHECK(principal_kind IN ('user', 'worker', 'workspace_agent')),
            principal_id TEXT,
            runtime_id TEXT,
            worker_id TEXT,
            assignment_id TEXT,
            expected_assignment_id TEXT,
            created_at TEXT NOT NULL,
            request_fingerprint TEXT,
            claim_state TEXT NOT NULL DEFAULT 'committed'
                CHECK(claim_state IN ('pending', 'failed', 'committed')),
            failure_reason TEXT,
            binding_recovery_json TEXT,
            PRIMARY KEY (workspace_id, operation_id),
            FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
            CHECK(claim_state = 'committed' OR assignment_id IS NULL),
            CHECK((claim_state = 'failed') = (failure_reason IS NOT NULL))
        );
CREATE TABLE ticket_assignment_ticket_tombstones (
    workspace_id TEXT NOT NULL,
    ticket_id TEXT NOT NULL,
    deleted_at TEXT NOT NULL,
    PRIMARY KEY (workspace_id, ticket_id),
    FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE
);
CREATE TABLE ticket_assignment_worker_tombstones (
    workspace_id TEXT NOT NULL,
    runtime_id TEXT NOT NULL,
    worker_id TEXT NOT NULL,
    deleted_at TEXT NOT NULL,
    PRIMARY KEY (workspace_id, runtime_id, worker_id),
    FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE
);
CREATE TABLE ticket_current_worker_assignments (
            workspace_id TEXT NOT NULL,
            ticket_id TEXT NOT NULL,
            role TEXT NOT NULL DEFAULT 'worker'
                CHECK(role IN ('orchestrator', 'worker', 'owner', 'contributor')),
            assignment_id TEXT NOT NULL,
            principal_kind TEXT NOT NULL DEFAULT 'worker'
                CHECK(principal_kind IN ('user', 'worker', 'workspace_agent')),
            principal_id TEXT,
            runtime_id TEXT,
            worker_id TEXT,
            updated_at TEXT NOT NULL,
            PRIMARY KEY (workspace_id, ticket_id, role, assignment_id),
            UNIQUE (workspace_id, assignment_id),
            FOREIGN KEY (workspace_id, ticket_id, role, assignment_id)
                REFERENCES ticket_worker_assignments(workspace_id, ticket_id, role, assignment_id)
                ON DELETE CASCADE,
            FOREIGN KEY (workspace_id, ticket_id)
                REFERENCES typed_tickets(workspace_id, ticket_id)
                ON DELETE CASCADE,
            CHECK(
                (principal_kind = 'worker' AND runtime_id IS NOT NULL AND worker_id IS NOT NULL AND principal_id IS NULL)
                OR
                (principal_kind != 'worker' AND runtime_id IS NULL AND worker_id IS NULL AND principal_id IS NOT NULL AND length(trim(principal_id)) > 0)
            )
        );
CREATE TABLE ticket_worker_assignment_events (
            workspace_id TEXT NOT NULL,
            ticket_id TEXT NOT NULL,
            role TEXT NOT NULL DEFAULT 'worker'
                CHECK(role IN ('orchestrator', 'worker', 'owner', 'contributor')),
            event_id TEXT NOT NULL,
            action TEXT NOT NULL CHECK (action IN ('assigned', 'reassigned', 'unassigned')),
            assignment_id TEXT,
            previous_assignment_id TEXT,
            actor TEXT NOT NULL,
            created_at TEXT NOT NULL,
            operation_id TEXT,
            reason TEXT,
            PRIMARY KEY (workspace_id, event_id),
            FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE
        );
CREATE TABLE ticket_worker_assignments (
            workspace_id TEXT NOT NULL,
            ticket_id TEXT NOT NULL,
            assignment_id TEXT NOT NULL,
            role TEXT NOT NULL DEFAULT 'worker'
                CHECK(role IN ('orchestrator', 'worker', 'owner', 'contributor')),
            principal_kind TEXT NOT NULL DEFAULT 'worker'
                CHECK(principal_kind IN ('user', 'worker', 'workspace_agent')),
            principal_id TEXT,
            runtime_id TEXT,
            worker_id TEXT,
            assigned_by TEXT NOT NULL,
            assigned_at TEXT NOT NULL,
            PRIMARY KEY (workspace_id, assignment_id),
            UNIQUE (workspace_id, ticket_id, assignment_id),
            UNIQUE (workspace_id, ticket_id, role, assignment_id),
            UNIQUE (workspace_id, ticket_id, role, assignment_id, principal_kind, principal_id, runtime_id, worker_id),
            FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
            CHECK(
                (principal_kind = 'worker' AND runtime_id IS NOT NULL AND worker_id IS NOT NULL AND principal_id IS NULL)
                OR
                (principal_kind != 'worker' AND runtime_id IS NULL AND worker_id IS NULL AND principal_id IS NOT NULL AND length(trim(principal_id)) > 0)
            )
        );
CREATE TABLE workspace_runtime_bindings (
    workspace_id TEXT NOT NULL,
    runtime_id TEXT NOT NULL,
    display_name TEXT NOT NULL,
    base_url TEXT NOT NULL,
    public_key TEXT NOT NULL,
    public_key_fingerprint TEXT NOT NULL,
    binding_id TEXT NOT NULL CHECK (length(binding_id) > 0),
    state TEXT NOT NULL CHECK (state IN ('configured', 'verified', 'revoked')),
    authentication_mode TEXT NOT NULL CHECK (authentication_mode IN ('legacy_server_issuer', 'workspace_identity')),
    workspace_key_id TEXT,
    workspace_public_key_fingerprint TEXT,
    workspace_trust_id TEXT CHECK (length(workspace_trust_id) > 0),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    revoked_at TEXT,
    PRIMARY KEY (workspace_id, runtime_id),
    UNIQUE (workspace_id, public_key_fingerprint),
    FOREIGN KEY(workspace_id) REFERENCES workspaces(workspace_id) ON DELETE RESTRICT,
    CHECK (
        (authentication_mode = 'legacy_server_issuer' AND workspace_key_id IS NULL AND workspace_public_key_fingerprint IS NULL AND workspace_trust_id IS NULL)
        OR
        (authentication_mode = 'workspace_identity' AND workspace_key_id IS NOT NULL AND (state != 'verified' OR (workspace_trust_id IS NOT NULL AND workspace_public_key_fingerprint IS NOT NULL)))
    ),
    CHECK (
        (state = 'revoked' AND revoked_at IS NOT NULL)
        OR
        (state != 'revoked' AND revoked_at IS NULL)
    )
);
CREATE TABLE workspace_runtime_verifications (
    workspace_id TEXT NOT NULL,
    runtime_id TEXT NOT NULL,
    binding_id TEXT NOT NULL CHECK(length(binding_id) > 0),
    workspace_key_id TEXT NOT NULL,
    workspace_public_key_fingerprint TEXT NOT NULL CHECK(length(workspace_public_key_fingerprint) > 0),
    workspace_trust_id TEXT NOT NULL CHECK(length(workspace_trust_id) > 0),
    runtime_public_key_fingerprint TEXT NOT NULL,
    challenge_id TEXT NOT NULL,
    state TEXT NOT NULL CHECK(state IN ('pending', 'verified', 'failed')),
    last_outcome TEXT NOT NULL,
    verified_at TEXT,
    checked_at TEXT NOT NULL,
    PRIMARY KEY(workspace_id, runtime_id),
    FOREIGN KEY(workspace_id, runtime_id)
        REFERENCES workspace_runtime_bindings(workspace_id, runtime_id) ON DELETE CASCADE,
    CHECK((state = 'verified' AND verified_at IS NOT NULL)
       OR (state != 'verified' AND verified_at IS NULL))
);
CREATE INDEX workspace_runtime_verifications_state_idx
    ON workspace_runtime_verifications(workspace_id, state, checked_at DESC);
CREATE TABLE workspace_runtime_binding_audit (
    workspace_id TEXT NOT NULL,
    runtime_id TEXT NOT NULL,
    actor_account_id TEXT NOT NULL,
    action TEXT NOT NULL CHECK (action IN ('created', 'replaced', 'reactivated', 'revoked')),
    old_fingerprint TEXT,
    new_fingerprint TEXT,
    binding_id TEXT NOT NULL CHECK (length(binding_id) > 0),
    at TEXT NOT NULL,
    PRIMARY KEY (workspace_id, runtime_id, binding_id),
    FOREIGN KEY(workspace_id, runtime_id)
        REFERENCES workspace_runtime_bindings(workspace_id, runtime_id) ON DELETE RESTRICT,
    FOREIGN KEY(actor_account_id) REFERENCES accounts(account_id) ON DELETE RESTRICT
);
CREATE INDEX idx_workspace_runtime_binding_audit_recent
    ON workspace_runtime_binding_audit(workspace_id, runtime_id, at DESC, binding_id DESC);
CREATE TABLE typed_ticket_artifacts (
    workspace_id TEXT NOT NULL, ticket_id TEXT NOT NULL, relative_path TEXT NOT NULL, content BLOB NOT NULL,
    PRIMARY KEY (workspace_id, ticket_id, relative_path),
    FOREIGN KEY (workspace_id, ticket_id) REFERENCES typed_tickets(workspace_id, ticket_id) ON DELETE CASCADE
);
CREATE TABLE typed_ticket_event_attributes (
    workspace_id TEXT NOT NULL, ticket_id TEXT NOT NULL, event_index INTEGER NOT NULL, key TEXT NOT NULL, value TEXT NOT NULL,
    PRIMARY KEY (workspace_id, ticket_id, event_index, key),
    FOREIGN KEY (workspace_id, ticket_id, event_index) REFERENCES typed_ticket_events(workspace_id, ticket_id, event_index) ON DELETE CASCADE
);
CREATE TABLE typed_ticket_event_references (
    workspace_id TEXT NOT NULL, ticket_id TEXT NOT NULL, event_index INTEGER NOT NULL, ordinal INTEGER NOT NULL, kind TEXT NOT NULL, target TEXT NOT NULL,
    PRIMARY KEY (workspace_id, ticket_id, event_index, ordinal),
    FOREIGN KEY (workspace_id, ticket_id, event_index) REFERENCES typed_ticket_events(workspace_id, ticket_id, event_index) ON DELETE CASCADE
);
CREATE TABLE typed_ticket_events (
    workspace_id TEXT NOT NULL,
    ticket_id TEXT NOT NULL,
    event_index INTEGER NOT NULL,
    kind TEXT NOT NULL,
    author TEXT,
    at TEXT,
    status TEXT,
    from_state TEXT,
    to_state TEXT,
    reason TEXT,
    state_field TEXT,
    heading TEXT,
    body TEXT NOT NULL,
    PRIMARY KEY (workspace_id, ticket_id, event_index),
    FOREIGN KEY (workspace_id, ticket_id) REFERENCES typed_tickets(workspace_id, ticket_id) ON DELETE CASCADE
);
CREATE TABLE typed_ticket_labels (
    workspace_id TEXT NOT NULL, ticket_id TEXT NOT NULL, ordinal INTEGER NOT NULL, label TEXT NOT NULL,
    PRIMARY KEY (workspace_id, ticket_id, ordinal),
    FOREIGN KEY (workspace_id, ticket_id) REFERENCES typed_tickets(workspace_id, ticket_id) ON DELETE CASCADE
);
CREATE TABLE typed_ticket_orchestration_plans (
    workspace_id TEXT NOT NULL,
    ticket_id TEXT NOT NULL,
    record_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    related_ticket TEXT,
    note TEXT,
    accepted_summary TEXT,
    accepted_branch TEXT,
    accepted_worktree TEXT,
    accepted_role_plan TEXT,
    author TEXT NOT NULL,
    at TEXT NOT NULL,
    PRIMARY KEY (workspace_id, ticket_id, record_id),
    FOREIGN KEY (workspace_id, ticket_id) REFERENCES typed_tickets(workspace_id, ticket_id) ON DELETE CASCADE
);
CREATE TABLE typed_ticket_raw_frontmatter (
    workspace_id TEXT NOT NULL, ticket_id TEXT NOT NULL, key TEXT NOT NULL, value TEXT NOT NULL,
    PRIMARY KEY (workspace_id, ticket_id, key),
    FOREIGN KEY (workspace_id, ticket_id) REFERENCES typed_tickets(workspace_id, ticket_id) ON DELETE CASCADE
);
CREATE TABLE "typed_ticket_relations" (
    workspace_id TEXT NOT NULL,
    ticket_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    target TEXT NOT NULL,
    note TEXT,
    author TEXT NOT NULL,
    at TEXT NOT NULL,
    PRIMARY KEY (workspace_id, ticket_id, kind, target),
    FOREIGN KEY (workspace_id, ticket_id)
        REFERENCES typed_tickets(workspace_id, ticket_id) ON DELETE CASCADE,
    FOREIGN KEY (workspace_id, target)
        REFERENCES typed_tickets(workspace_id, ticket_id) ON DELETE CASCADE
);
CREATE TABLE typed_ticket_risk_flags (
    workspace_id TEXT NOT NULL, ticket_id TEXT NOT NULL, ordinal INTEGER NOT NULL, risk_flag TEXT NOT NULL,
    PRIMARY KEY (workspace_id, ticket_id, ordinal),
    FOREIGN KEY (workspace_id, ticket_id) REFERENCES typed_tickets(workspace_id, ticket_id) ON DELETE CASCADE
);
CREATE TABLE "typed_tickets" (
    workspace_id TEXT NOT NULL,
    ticket_id TEXT NOT NULL,
    slug TEXT NOT NULL,
    title TEXT NOT NULL,
    status TEXT NOT NULL,
    kind TEXT NOT NULL,
    priority TEXT NOT NULL,
    body TEXT NOT NULL,
    created_at TEXT,
    updated_at TEXT,
    assignee TEXT,
    readiness TEXT,
    workflow_state TEXT NOT NULL,
    workflow_state_explicit INTEGER NOT NULL,
    queued_by TEXT,
    queued_at TEXT,
    resolution TEXT,
    repository_id TEXT,
    ref_selector TEXT,
    PRIMARY KEY (workspace_id, ticket_id),
    FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    FOREIGN KEY (workspace_id, repository_id)
        REFERENCES repositories(workspace_id, repository_id) ON DELETE RESTRICT
);
CREATE TABLE users (
    user_id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL UNIQUE REFERENCES accounts(account_id) ON DELETE CASCADE,
    handle TEXT NOT NULL UNIQUE,
    display_name TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
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
CREATE TABLE "workdir_registry" (
    workspace_id TEXT NOT NULL,
    workdir_id TEXT NOT NULL,
    display_name TEXT,
    source_kind TEXT NOT NULL CHECK (source_kind IN ('repository', 'external_grant', 'workspace_config')),
    runtime_id TEXT,
    repository_id TEXT,
    external_grant_id TEXT,
    creation_selector TEXT,
    creation_ref TEXT,
    materialization_status TEXT NOT NULL CHECK (materialization_status IN ('pending', 'present', 'not_found', 'corrupted', 'unknown', 'failed')),
    cleanliness TEXT NOT NULL CHECK (cleanliness IN ('clean', 'dirty', 'unknown')),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    current_selector TEXT,
    current_ref TEXT, creation_tree TEXT, current_tree TEXT, observed_at_epoch_seconds INTEGER,
    workspace_config_grant_id TEXT,
    PRIMARY KEY (workspace_id, workdir_id),
    CHECK (
        (source_kind = 'repository' AND runtime_id IS NOT NULL AND repository_id IS NOT NULL AND external_grant_id IS NULL AND workspace_config_grant_id IS NULL)
        OR (source_kind = 'external_grant' AND runtime_id IS NULL AND repository_id IS NULL AND external_grant_id IS NOT NULL AND workspace_config_grant_id IS NULL)
        OR (source_kind = 'workspace_config' AND runtime_id IS NULL AND repository_id IS NULL AND external_grant_id IS NULL AND workspace_config_grant_id IS NOT NULL)
    ),
    FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    FOREIGN KEY (workspace_id, repository_id)
        REFERENCES "repositories"(workspace_id, repository_id),
    FOREIGN KEY (workspace_id, external_grant_id)
        REFERENCES external_workdir_grants(workspace_id, grant_id) DEFERRABLE INITIALLY DEFERRED,
    FOREIGN KEY (workspace_id, workspace_config_grant_id)
        REFERENCES workspace_config_grants(workspace_id, grant_id) DEFERRABLE INITIALLY DEFERRED
);
CREATE TABLE external_workdir_grants (
    grant_id TEXT NOT NULL,
    workspace_id TEXT NOT NULL,
    workdir_id TEXT NOT NULL,
    provider_instance_id TEXT NOT NULL,
    display_name TEXT NOT NULL,
    permissions TEXT NOT NULL CHECK (permissions IN (
        'read_only', 'read_write', 'command_only',
        'read_command', 'read_write_command'
    )),
    created_by TEXT NOT NULL,
    created_at TEXT NOT NULL,
    expires_at TEXT,
    generation INTEGER NOT NULL CHECK (generation > 0),
    status TEXT NOT NULL CHECK (status IN ('pending', 'online', 'offline', 'revoked', 'expired')),
    updated_at TEXT NOT NULL,
    cleanup_state TEXT NOT NULL DEFAULT 'not_required' CHECK (cleanup_state IN ('not_required', 'pending', 'retry_required', 'completed')),
    cleanup_error TEXT,
    cleanup_updated_at TEXT,
    PRIMARY KEY (workspace_id, grant_id),
    UNIQUE (workspace_id, workdir_id),
    FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    FOREIGN KEY (workspace_id, workdir_id)
        REFERENCES workdir_registry(workspace_id, workdir_id) ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
);
CREATE TABLE workdir_removal_operations (
    workspace_id TEXT NOT NULL,
    operation_id TEXT NOT NULL,
    request_fingerprint TEXT NOT NULL,
    workdir_id TEXT NOT NULL,
    runtime_id TEXT NOT NULL,
    repository_id TEXT NOT NULL,
    materialization_fingerprint TEXT NOT NULL,
    source_actor TEXT NOT NULL,
    reason TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('pending', 'failed', 'completed')),
    attempt_id TEXT,
    retryable INTEGER NOT NULL CHECK (retryable IN (0, 1)),
    disposition TEXT CHECK (disposition IN ('removed', 'retained', 'attention_required')),
    failure_category TEXT,
    attempt_owner_pid INTEGER CHECK (attempt_owner_pid > 0),
    attempt_owner_start_marker INTEGER CHECK (attempt_owner_start_marker >= 0),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    completed_at TEXT,
    PRIMARY KEY (workspace_id, operation_id),
    FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE
);
CREATE TABLE "worker_control_grants" (
            grant_id TEXT NOT NULL,
            workspace_id TEXT NOT NULL,
            controller_runtime_id TEXT NOT NULL,
            controller_worker_id TEXT NOT NULL,
            subject_runtime_id TEXT NOT NULL,
            subject_worker_id TEXT NOT NULL,
            relation TEXT NOT NULL,
            origin TEXT NOT NULL,
            permissions_json TEXT NOT NULL,
            operation_id TEXT NOT NULL,
            created_at TEXT NOT NULL,
            revoked_at TEXT,
            PRIMARY KEY (workspace_id, grant_id),
            UNIQUE (workspace_id, controller_runtime_id, controller_worker_id, operation_id),
            FOREIGN KEY (workspace_id, controller_runtime_id, controller_worker_id)
                REFERENCES "worker_registry"(workspace_id, runtime_id, worker_id) ON DELETE CASCADE,
            FOREIGN KEY (workspace_id, subject_runtime_id, subject_worker_id)
                REFERENCES "worker_registry"(workspace_id, runtime_id, worker_id) ON DELETE CASCADE
        );
CREATE TABLE worker_create_reservations (
            workspace_id TEXT NOT NULL,
            allocation_key TEXT NOT NULL,
            worker_id TEXT NOT NULL,
            runtime_id TEXT NOT NULL,
            create_fingerprint TEXT NOT NULL,
            state TEXT NOT NULL CHECK (state IN ('reserved', 'created', 'removed')),
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL, request_fingerprint TEXT, memory_language TEXT,
            singleton_key TEXT,
            singleton_generation INTEGER CHECK (singleton_generation IS NULL OR singleton_generation > 0),
            PRIMARY KEY (workspace_id, allocation_key),
            UNIQUE (workspace_id, worker_id),
            FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
            CHECK ((singleton_key IS NULL) = (singleton_generation IS NULL))
        );
CREATE TRIGGER worker_create_reservation_singleton_lease_insert
BEFORE INSERT ON worker_create_reservations
WHEN (NEW.singleton_key IS NULL AND NEW.singleton_generation IS NOT NULL)
  OR (NEW.singleton_key IS NOT NULL AND NEW.singleton_generation IS NULL)
BEGIN
    SELECT RAISE(ABORT, 'worker_singleton_lease_incomplete');
END;
CREATE TRIGGER worker_create_reservation_singleton_lease_update
BEFORE UPDATE OF singleton_key, singleton_generation ON worker_create_reservations
WHEN (NEW.singleton_key IS NULL AND NEW.singleton_generation IS NOT NULL)
  OR (NEW.singleton_key IS NOT NULL AND NEW.singleton_generation IS NULL)
BEGIN
    SELECT RAISE(ABORT, 'worker_singleton_lease_incomplete');
END;
CREATE TABLE worker_singleton_owners (
            workspace_id TEXT NOT NULL,
            singleton_key TEXT NOT NULL,
            runtime_id TEXT NOT NULL,
            worker_id TEXT NOT NULL,
            generation INTEGER NOT NULL CHECK (generation > 0),
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            PRIMARY KEY (workspace_id, singleton_key),
            UNIQUE (workspace_id, worker_id),
            FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE
        );
CREATE TABLE worker_diagnostics_archives (
        operation_id TEXT PRIMARY KEY, workspace_id TEXT NOT NULL, runtime_id TEXT NOT NULL,
        worker_id TEXT NOT NULL, policy_id TEXT NOT NULL, policy_digest TEXT NOT NULL,
        committed_at TEXT NOT NULL, expires_at TEXT NOT NULL,
        FOREIGN KEY(operation_id) REFERENCES worker_removal_operations(operation_id),
        FOREIGN KEY(workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE);
CREATE TABLE worker_mutation_source_proof_jtis (
            workspace_id TEXT NOT NULL,
            runtime_id TEXT NOT NULL,
            jti TEXT NOT NULL,
            expires_at INTEGER NOT NULL,
            consumed_at TEXT NOT NULL,
            PRIMARY KEY (workspace_id, runtime_id, jti)
        );
CREATE TABLE worker_orphan_diagnostics (
        diagnostic_id TEXT PRIMARY KEY, workspace_id TEXT NOT NULL, runtime_id TEXT NOT NULL, worker_id TEXT NOT NULL,
        category TEXT NOT NULL, detail TEXT NOT NULL, observed_at TEXT NOT NULL,
        FOREIGN KEY(workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE);
CREATE TABLE "worker_registry" (
            workspace_id TEXT NOT NULL,
            worker_id TEXT NOT NULL,
            runtime_id TEXT NOT NULL,
            display_name TEXT NOT NULL,
            profile TEXT,
            retention_state TEXT NOT NULL CHECK (retention_state IN ('normal', 'pinned')),
            transcript_ref TEXT,
            session_ref TEXT,
            summary_ref TEXT,
            diagnostics_ref TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            PRIMARY KEY (workspace_id, runtime_id, worker_id),
            FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE
        );
CREATE TABLE backend_jobs (
            workspace_id TEXT NOT NULL,
            job_id TEXT NOT NULL,
            purpose TEXT NOT NULL,
            input_digest TEXT NOT NULL,
            input_ref TEXT NOT NULL,
            resource_key TEXT,
            request_json TEXT NOT NULL,
            intent_fingerprint TEXT NOT NULL,
            state TEXT NOT NULL CHECK (state IN ('pending', 'completed', 'failed', 'unknown')),
            current_attempt INTEGER NOT NULL CHECK (current_attempt > 0 AND current_attempt <= 3),
            result_json TEXT,
            result_digest TEXT,
            failure_category TEXT,
            failure_detail TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            completed_at TEXT,
            PRIMARY KEY (workspace_id, job_id),
            FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
            CHECK ((result_json IS NULL) = (result_digest IS NULL)),
            CHECK ((state = 'completed') = (result_json IS NOT NULL))
        );
CREATE INDEX backend_jobs_active
        ON backend_jobs(workspace_id, state, updated_at);
CREATE INDEX backend_jobs_resource
        ON backend_jobs(workspace_id, resource_key, created_at);
CREATE TABLE backend_job_attempts (
            workspace_id TEXT NOT NULL,
            job_id TEXT NOT NULL,
            attempt_id TEXT NOT NULL,
            attempt INTEGER NOT NULL CHECK (attempt > 0 AND attempt <= 3),
            input_digest TEXT NOT NULL,
            state TEXT NOT NULL CHECK (state IN ('reserved', 'dispatching', 'dispatched', 'completed', 'failed', 'unknown')),
            runtime_id TEXT,
            worker_id TEXT,
            runtime_run_id TEXT,
            dispatched_at TEXT,
            deadline_at TEXT NOT NULL,
            result_json TEXT,
            result_digest TEXT,
            failure_category TEXT,
            failure_detail TEXT,
            worker_cleanup_state TEXT CHECK (
                worker_cleanup_state IN ('pending', 'executing', 'completed', 'failed')
            ),
            worker_cleanup_failure_category TEXT,
            worker_cleanup_failure_detail TEXT,
            worker_cleanup_updated_at TEXT,
            worker_cleanup_completed_at TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            completed_at TEXT,
            PRIMARY KEY (workspace_id, job_id, attempt_id),
            UNIQUE (workspace_id, job_id, attempt),
            UNIQUE (workspace_id, runtime_id, worker_id),
            FOREIGN KEY (workspace_id, job_id)
                REFERENCES backend_jobs(workspace_id, job_id) ON DELETE CASCADE,
            CHECK ((runtime_id IS NULL) = (worker_id IS NULL)),
            CHECK ((result_json IS NULL) = (result_digest IS NULL)),
            CHECK ((state = 'completed') = (result_json IS NOT NULL))
        );
CREATE INDEX backend_job_attempts_recovery
        ON backend_job_attempts(workspace_id, state, deadline_at);
CREATE INDEX backend_job_attempts_worker_cleanup
        ON backend_job_attempts(workspace_id, worker_cleanup_state, updated_at);
CREATE TABLE backend_job_deliveries (
            workspace_id TEXT NOT NULL,
            delivery_id TEXT NOT NULL,
            job_id TEXT NOT NULL,
            attempt_id TEXT NOT NULL,
            target_runtime_id TEXT NOT NULL,
            target_worker_id TEXT NOT NULL,
            state TEXT NOT NULL CHECK (state IN ('pending', 'sending', 'completed', 'failed', 'unknown')),
            failure_category TEXT,
            failure_detail TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            delivered_at TEXT,
            PRIMARY KEY (workspace_id, delivery_id),
            FOREIGN KEY (workspace_id, job_id, attempt_id)
                REFERENCES backend_job_attempts(workspace_id, job_id, attempt_id) ON DELETE CASCADE
        );
CREATE INDEX backend_job_deliveries_pending
        ON backend_job_deliveries(workspace_id, state, updated_at);
CREATE TABLE worker_registry_observations (
    workspace_id TEXT NOT NULL,
    runtime_id TEXT NOT NULL,
    worker_id TEXT NOT NULL,
    availability TEXT NOT NULL CHECK (availability IN ('observed', 'unavailable')),
    worker_json TEXT,
    observed_at TEXT NOT NULL,
    PRIMARY KEY (workspace_id, runtime_id, worker_id),
    FOREIGN KEY (workspace_id, runtime_id, worker_id)
        REFERENCES worker_registry(workspace_id, runtime_id, worker_id) ON DELETE CASCADE
);
CREATE TABLE worker_registry_projection_removals (
    workspace_id TEXT NOT NULL,
    runtime_id TEXT NOT NULL,
    worker_id TEXT NOT NULL,
    PRIMARY KEY (workspace_id, runtime_id, worker_id)
);
CREATE TABLE worker_registry_projection_diagnostics (
    diagnostic_id INTEGER PRIMARY KEY AUTOINCREMENT,
    workspace_id TEXT NOT NULL,
    runtime_id TEXT NOT NULL,
    worker_id TEXT NOT NULL,
    reason TEXT NOT NULL,
    observed_at TEXT NOT NULL
);
CREATE TABLE worker_removal_operations (
        operation_id TEXT PRIMARY KEY, plan_id TEXT NOT NULL UNIQUE, input_fingerprint TEXT NOT NULL,
        workspace_id TEXT NOT NULL, runtime_id TEXT NOT NULL, worker_id TEXT NOT NULL,
        worker_updated_at TEXT NOT NULL,
        policy_id TEXT NOT NULL, policy_digest TEXT NOT NULL,
        session_disposition TEXT NOT NULL, metadata_disposition TEXT NOT NULL,
        archive_retention_kind TEXT NOT NULL, archive_retention_seconds INTEGER,
        diagnostics_disposition TEXT NOT NULL,
        diagnostics_retention_seconds INTEGER, archive_id TEXT UNIQUE, blockers_json TEXT NOT NULL,
        state TEXT NOT NULL CHECK(state IN ('planned','blocked','executing','failed','stale','succeeded')),
        reason TEXT NOT NULL, failure_category TEXT, created_at TEXT NOT NULL, updated_at TEXT NOT NULL,
        FOREIGN KEY(workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE);
CREATE TABLE worker_retention_audit_events (
        event_id TEXT PRIMARY KEY, operation_id TEXT NOT NULL, workspace_id TEXT NOT NULL,
        event_kind TEXT NOT NULL, detail TEXT NOT NULL, created_at TEXT NOT NULL,
        FOREIGN KEY(operation_id) REFERENCES worker_removal_operations(operation_id),
        FOREIGN KEY(workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE);
CREATE TABLE worker_session_archives (
        archive_id TEXT PRIMARY KEY, workspace_id TEXT NOT NULL, runtime_id TEXT NOT NULL, worker_id TEXT NOT NULL,
        session_id TEXT NOT NULL, checksum_sha256 TEXT NOT NULL, content_bytes INTEGER NOT NULL,
        policy_id TEXT NOT NULL, policy_digest TEXT NOT NULL, operation_id TEXT NOT NULL UNIQUE,
        committed_at TEXT NOT NULL, expires_at TEXT,
        FOREIGN KEY(workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
        FOREIGN KEY(operation_id) REFERENCES worker_removal_operations(operation_id));
CREATE TABLE worker_session_archive_observe_grants (
        workspace_id TEXT NOT NULL, archive_id TEXT NOT NULL,
        controller_runtime_id TEXT NOT NULL, controller_worker_id TEXT NOT NULL,
        subject_runtime_id TEXT NOT NULL, subject_worker_id TEXT NOT NULL,
        source_grant_id TEXT NOT NULL, granted_at TEXT NOT NULL, revoked_at TEXT,
        PRIMARY KEY(workspace_id,archive_id,controller_runtime_id,controller_worker_id,source_grant_id),
        FOREIGN KEY(workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
        FOREIGN KEY(archive_id) REFERENCES worker_session_archives(archive_id) ON DELETE CASCADE);
CREATE INDEX worker_session_archive_observe_grants_controller
        ON worker_session_archive_observe_grants(
            workspace_id,controller_runtime_id,controller_worker_id,archive_id);
CREATE TABLE worker_tombstones (
        workspace_id TEXT NOT NULL, runtime_id TEXT NOT NULL, worker_id TEXT NOT NULL,
        display_name TEXT NOT NULL, profile TEXT, worker_created_at TEXT NOT NULL, removed_at TEXT NOT NULL,
        archive_id TEXT, policy_id TEXT NOT NULL, policy_digest TEXT NOT NULL, operation_id TEXT NOT NULL UNIQUE,
        PRIMARY KEY(workspace_id,runtime_id,worker_id),
        FOREIGN KEY(workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
        FOREIGN KEY(archive_id) REFERENCES worker_session_archives(archive_id),
        FOREIGN KEY(operation_id) REFERENCES worker_removal_operations(operation_id));
CREATE TABLE "worker_workdir_attachment_reservations" (
    workspace_id TEXT NOT NULL,
    workdir_id TEXT NOT NULL,
    reservation_id TEXT NOT NULL,
    reserved_at TEXT NOT NULL,
    PRIMARY KEY (workspace_id, workdir_id),
    FOREIGN KEY (workspace_id, workdir_id)
        REFERENCES "workdir_registry"(workspace_id, workdir_id) ON DELETE CASCADE
);
CREATE TABLE "worker_workdir_links" (
            workspace_id TEXT NOT NULL,
            runtime_id TEXT NOT NULL,
            worker_id TEXT NOT NULL,
            workdir_id TEXT NOT NULL,
            alias TEXT NOT NULL,
            linked_at TEXT NOT NULL,
            unlinked_at TEXT,
            PRIMARY KEY (workspace_id, runtime_id, worker_id, workdir_id, alias),
            FOREIGN KEY (workspace_id, runtime_id, worker_id)
                REFERENCES "worker_registry"(workspace_id, runtime_id, worker_id) ON DELETE CASCADE,
            FOREIGN KEY (workspace_id, workdir_id)
                REFERENCES workdir_registry(workspace_id, workdir_id) ON DELETE CASCADE
        );
CREATE TABLE workspace_config_entries (
            workspace_id TEXT NOT NULL,
            path TEXT NOT NULL,
            content_type TEXT NOT NULL,
            content TEXT NOT NULL,
            content_digest TEXT NOT NULL,
            PRIMARY KEY (workspace_id, path),
            FOREIGN KEY (workspace_id) REFERENCES workspace_config_trees(workspace_id) ON DELETE CASCADE
        );
CREATE TABLE workspace_config_tree_history (
            workspace_id TEXT NOT NULL,
            content_digest TEXT NOT NULL,
            toolchain_fingerprint TEXT NOT NULL,
            projection_digest TEXT NOT NULL,
            manifest_json TEXT NOT NULL,
            created_at TEXT NOT NULL, schema_bundle_json TEXT NOT NULL DEFAULT '{"contributions":[],"source":"{}","fingerprint":""}',
            PRIMARY KEY (workspace_id, content_digest, toolchain_fingerprint, projection_digest),
            FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE
        );
CREATE TABLE workspace_config_trees (
            workspace_id TEXT PRIMARY KEY,
            content_digest TEXT NOT NULL,
            schema_version INTEGER NOT NULL,
            entrypoints_json TEXT NOT NULL,
            decodal_version TEXT NOT NULL,
            import_policy_version INTEGER NOT NULL,
            toolchain_fingerprint TEXT NOT NULL,
            projection_digest TEXT NOT NULL,
            updated_at TEXT NOT NULL, schema_bundle_json TEXT NOT NULL DEFAULT '{"contributions":[],"source":"{}","fingerprint":""}',
            FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE
        );
CREATE TABLE workspace_create_operations (
            operation_key TEXT PRIMARY KEY,
            request_fingerprint TEXT NOT NULL,
            workspace_id TEXT NOT NULL UNIQUE,
            created_at TEXT NOT NULL,
            FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE
        );
CREATE TABLE workspace_signing_identities (
            workspace_id TEXT PRIMARY KEY,
            key_id TEXT NOT NULL UNIQUE,
            algorithm TEXT NOT NULL CHECK (algorithm = 'ed25519'),
            public_key TEXT,
            public_key_fingerprint TEXT,
            private_material_ref TEXT NOT NULL UNIQUE,
            state TEXT NOT NULL CHECK (state IN ('pending_provisioning', 'active')),
            created_at TEXT NOT NULL,
            provisioned_at TEXT,
            updated_at TEXT NOT NULL,
            CHECK (
                (state = 'pending_provisioning' AND public_key IS NULL AND public_key_fingerprint IS NULL AND provisioned_at IS NULL)
                OR
                (state = 'active' AND public_key IS NOT NULL AND public_key_fingerprint IS NOT NULL AND provisioned_at IS NOT NULL)
            ),
            FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE
        );
CREATE TABLE workspace_signing_identity_provisioning_operations (
            operation_key TEXT PRIMARY KEY,
            request_fingerprint TEXT NOT NULL,
            operation_kind TEXT NOT NULL CHECK (operation_kind IN ('workspace_create', 'existing_workspace')),
            workspace_id TEXT NOT NULL UNIQUE,
            key_id TEXT NOT NULL UNIQUE,
            private_material_ref TEXT NOT NULL UNIQUE,
            actor_account_id TEXT NOT NULL,
            state TEXT NOT NULL CHECK (state IN ('pending', 'completed')),
            created_at TEXT NOT NULL,
            completed_at TEXT
        );
CREATE TABLE workspace_signing_identity_audit (
            event_id TEXT PRIMARY KEY,
            workspace_id TEXT NOT NULL,
            key_id TEXT NOT NULL,
            action TEXT NOT NULL CHECK (action IN ('provisioned')),
            public_key_fingerprint TEXT NOT NULL,
            actor_account_id TEXT NOT NULL,
            created_at TEXT NOT NULL,
            FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE
        );
CREATE INDEX workspace_signing_identity_audit_workspace_idx
            ON workspace_signing_identity_audit(workspace_id, created_at DESC);
CREATE TABLE workspace_memory_documents (
    workspace_id TEXT PRIMARY KEY REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    body_md TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE TABLE workspace_memory_settings (
            workspace_id TEXT PRIMARY KEY NOT NULL,
            language TEXT NOT NULL CHECK(length(trim(language)) > 0),
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE
        );
CREATE TABLE "workspace_resource_key_counters" (
    workspace_id TEXT NOT NULL,
    resource_kind TEXT NOT NULL CHECK (resource_kind IN ('ticket', 'objective', 'worker')),
    next_sequence INTEGER NOT NULL CHECK (next_sequence > 0),
    PRIMARY KEY (workspace_id, resource_kind),
    FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE
);
CREATE TABLE "workspace_resource_keys" (
    workspace_id TEXT NOT NULL,
    resource_kind TEXT NOT NULL CHECK (resource_kind IN ('ticket', 'objective', 'worker')),
    resource_id TEXT NOT NULL,
    sequence INTEGER NOT NULL CHECK (sequence > 0),
    resource_key TEXT NOT NULL,
    allocated_at TEXT NOT NULL,
    PRIMARY KEY (workspace_id, resource_kind, resource_id),
    UNIQUE (workspace_id, resource_kind, sequence),
    UNIQUE (workspace_id, resource_key),
    FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE
);
CREATE TABLE workspace_worker_retention_policies (
        workspace_id TEXT PRIMARY KEY, policy_id TEXT NOT NULL, policy_digest TEXT NOT NULL, updated_at TEXT NOT NULL,
        FOREIGN KEY(workspace_id,policy_id,policy_digest) REFERENCES workspace_worker_retention_policy_snapshots(workspace_id,policy_id,policy_digest),
        FOREIGN KEY(workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE);
CREATE TABLE workspace_worker_retention_policy_snapshots (
        workspace_id TEXT NOT NULL, policy_id TEXT NOT NULL, policy_digest TEXT NOT NULL CHECK(length(policy_digest)>0),
        session_disposition TEXT NOT NULL CHECK(session_disposition IN ('archive','purge')),
        metadata_disposition TEXT NOT NULL CHECK(metadata_disposition IN ('tombstone','purge')),
        archive_retention_kind TEXT NOT NULL CHECK(archive_retention_kind IN ('forever','for_seconds')),
        archive_retention_seconds INTEGER,
        diagnostics_disposition TEXT NOT NULL CHECK(diagnostics_disposition IN ('purge','retain')),
        diagnostics_retention_seconds INTEGER, created_at TEXT NOT NULL,
        PRIMARY KEY(workspace_id,policy_id,policy_digest),
        FOREIGN KEY(workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE);
CREATE TABLE "workspaces" (
            workspace_id TEXT PRIMARY KEY,
            display_name TEXT NOT NULL,
            state TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            owner_account_id TEXT NOT NULL,
            FOREIGN KEY (owner_account_id) REFERENCES accounts(account_id) ON DELETE RESTRICT
        );
CREATE INDEX idx_api_tokens_token_hash ON api_tokens(token_hash);
CREATE INDEX idx_artifacts_workspace_created
    ON artifacts(workspace_id, created_at DESC);
CREATE INDEX idx_browser_sessions_token_hash ON browser_sessions(token_hash);
CREATE INDEX idx_device_login_user_code ON device_login_flows(user_code);
CREATE INDEX idx_objective_events_workspace_created
    ON objective_events(workspace_id, created_at DESC);
CREATE INDEX idx_objective_resources_workspace_objective
    ON objective_resources(workspace_id, objective_id);
CREATE INDEX idx_objective_ticket_links_workspace_objective
    ON objective_ticket_links(workspace_id, objective_id);
CREATE INDEX idx_objective_ticket_links_workspace_ticket
    ON objective_ticket_links(workspace_id, ticket_id);
CREATE INDEX idx_objectives_workspace_updated
    ON objectives(workspace_id, updated_at DESC);
CREATE INDEX idx_repository_secret_audit_workspace_created
            ON repository_secret_audit_events(workspace_id, created_at, event_id);
CREATE INDEX idx_repository_ssh_credentials_workspace_status
            ON repository_ssh_credentials(workspace_id, status, credential_id);
CREATE INDEX idx_repository_ssh_host_trusts_workspace_host
            ON repository_ssh_host_trusts(workspace_id, hostname, port);
CREATE INDEX idx_ticket_assignment_operations_ticket
            ON ticket_assignment_operations(workspace_id, ticket_id, role, created_at DESC);
CREATE INDEX idx_ticket_worker_assignment_events_ticket
            ON ticket_worker_assignment_events(workspace_id, ticket_id, role, created_at DESC);
CREATE INDEX idx_ticket_worker_assignments_principal
            ON ticket_worker_assignments(workspace_id, role, principal_kind, principal_id, runtime_id, worker_id, assigned_at DESC);
CREATE INDEX idx_ticket_worker_assignments_ticket
            ON ticket_worker_assignments(workspace_id, ticket_id, role, assigned_at DESC);
CREATE INDEX idx_workspace_runtime_bindings_workspace
            ON workspace_runtime_bindings(workspace_id, revoked_at, runtime_id);
CREATE INDEX idx_typed_ticket_relations_workspace_target
    ON typed_ticket_relations(workspace_id, target, at DESC);
CREATE INDEX idx_typed_tickets_workspace_state_updated
    ON typed_tickets(workspace_id, workflow_state, updated_at DESC, ticket_id);
CREATE INDEX idx_typed_tickets_workspace_updated
    ON typed_tickets(workspace_id, updated_at DESC, ticket_id);
CREATE INDEX idx_workdir_registry_workspace_updated
    ON workdir_registry(workspace_id, updated_at DESC);
CREATE INDEX idx_external_workdir_grants_status_expiry
    ON external_workdir_grants(workspace_id, status, expires_at);
CREATE UNIQUE INDEX idx_workdir_removal_operations_one_pending
    ON workdir_removal_operations(workspace_id, workdir_id)
    WHERE state = 'pending';
CREATE INDEX idx_workdir_removal_operations_recovery
    ON workdir_removal_operations(workspace_id, state, retryable, updated_at);
CREATE INDEX idx_workdir_removal_operations_workdir
    ON workdir_removal_operations(workspace_id, workdir_id, created_at DESC);
CREATE INDEX idx_worker_mutation_source_proof_jtis_expiry
            ON worker_mutation_source_proof_jtis(expires_at);
CREATE UNIQUE INDEX idx_worker_registry_workspace_runtime_worker
    ON worker_registry(workspace_id, runtime_id, worker_id);
CREATE INDEX idx_workspace_config_entries_prefix
            ON workspace_config_entries(workspace_id, path);
CREATE INDEX idx_workspace_resource_keys_reverse
            ON workspace_resource_keys(workspace_id, resource_kind, resource_key);
CREATE INDEX repositories_workspace_provider_idx
            ON repositories(workspace_id, provider);
CREATE INDEX ticket_current_principal_idx
            ON ticket_current_worker_assignments(workspace_id, principal_kind, principal_id, runtime_id, worker_id);
CREATE UNIQUE INDEX ticket_current_singleton_role_idx
            ON ticket_current_worker_assignments(workspace_id, ticket_id, role)
            WHERE role IN ('orchestrator', 'worker');
CREATE INDEX typed_ticket_events_workspace_kind_ticket
            ON typed_ticket_events(workspace_id, kind, ticket_id, event_index);
CREATE UNIQUE INDEX ux_worker_workdir_attachment_reservation_id
    ON worker_workdir_attachment_reservations(workspace_id, reservation_id);
CREATE INDEX worker_control_grants_controller
            ON worker_control_grants(
                workspace_id, controller_runtime_id, controller_worker_id, revoked_at
            );
CREATE INDEX worker_control_grants_subject
            ON worker_control_grants(
                workspace_id, subject_runtime_id, subject_worker_id, revoked_at
            );
CREATE INDEX worker_create_reservations_worker
            ON worker_create_reservations(workspace_id, worker_id);
CREATE INDEX worker_registry_runtime
            ON worker_registry(workspace_id, runtime_id, worker_id);
CREATE INDEX worker_removal_operations_worker_idx ON worker_removal_operations(workspace_id,runtime_id,worker_id,created_at);
CREATE UNIQUE INDEX worker_workdir_links_active_workdir_unique
            ON worker_workdir_links(workspace_id, workdir_id)
            WHERE unlinked_at IS NULL;
CREATE UNIQUE INDEX worker_workdir_links_active_alias_unique
            ON worker_workdir_links(workspace_id, runtime_id, worker_id, alias)
            WHERE unlinked_at IS NULL;
CREATE INDEX worker_workdir_links_workdir
            ON worker_workdir_links(workspace_id, workdir_id);
CREATE TABLE runtime_removal_operations (
    operation_id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL,
    runtime_id TEXT NOT NULL,
    request_fingerprint TEXT NOT NULL,
    expected_binding_id TEXT NOT NULL CHECK(length(expected_binding_id)>0),
    config_digest TEXT NOT NULL CHECK(length(config_digest)>0),
    state TEXT NOT NULL CHECK (state IN ('pending', 'cleanup_pending', 'succeeded', 'failed')),
    failure_category TEXT,
    binding_removed INTEGER NOT NULL CHECK (binding_removed IN (0, 1)),
    runtime_registration_removed INTEGER CHECK (runtime_registration_removed IS NULL OR runtime_registration_removed IN (0, 1)),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    completed_at TEXT,
    FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE
);

CREATE UNIQUE INDEX runtime_removal_operations_one_active_runtime
ON runtime_removal_operations(runtime_id)
WHERE state IN ('pending', 'cleanup_pending');

CREATE INDEX runtime_removal_operations_workspace_state
ON runtime_removal_operations(workspace_id, state, updated_at);

CREATE TRIGGER runtime_binding_insert_blocked_by_removal
BEFORE INSERT ON workspace_runtime_bindings
FOR EACH ROW
WHEN EXISTS (
    SELECT 1
    FROM runtime_removal_operations operation
    WHERE operation.runtime_id = NEW.runtime_id
      AND operation.state IN ('pending', 'cleanup_pending')
)
BEGIN
    SELECT RAISE(ABORT, 'runtime_removal_in_progress');
END;

CREATE TRIGGER runtime_binding_update_blocked_by_removal
BEFORE UPDATE ON workspace_runtime_bindings
FOR EACH ROW
WHEN EXISTS (
    SELECT 1
    FROM runtime_removal_operations operation
    WHERE operation.runtime_id = NEW.runtime_id
      AND operation.state IN ('pending', 'cleanup_pending')
)
BEGIN
    SELECT RAISE(ABORT, 'runtime_removal_in_progress');
END;

CREATE TRIGGER worker_registry_insert_blocked_by_runtime_removal
BEFORE INSERT ON worker_registry FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM runtime_removal_operations operation
    WHERE operation.runtime_id = NEW.runtime_id
      AND operation.state IN ('pending', 'cleanup_pending')
)
BEGIN SELECT RAISE(ABORT, 'runtime_removal_in_progress'); END;

CREATE TRIGGER worker_registry_update_blocked_by_runtime_removal
BEFORE UPDATE ON worker_registry FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM runtime_removal_operations operation
    WHERE operation.runtime_id = NEW.runtime_id
      AND operation.state IN ('pending', 'cleanup_pending')
)
BEGIN SELECT RAISE(ABORT, 'runtime_removal_in_progress'); END;

CREATE TRIGGER workdir_registry_insert_blocked_by_runtime_removal
BEFORE INSERT ON workdir_registry FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM runtime_removal_operations operation
    WHERE operation.runtime_id = NEW.runtime_id
      AND operation.state IN ('pending', 'cleanup_pending')
)
BEGIN SELECT RAISE(ABORT, 'runtime_removal_in_progress'); END;

CREATE TRIGGER workdir_registry_update_blocked_by_runtime_removal
BEFORE UPDATE ON workdir_registry FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM runtime_removal_operations operation
    WHERE operation.runtime_id = NEW.runtime_id
      AND operation.state IN ('pending', 'cleanup_pending')
)
BEGIN SELECT RAISE(ABORT, 'runtime_removal_in_progress'); END;

CREATE TRIGGER worker_assignment_insert_blocked_by_runtime_removal
BEFORE INSERT ON ticket_current_worker_assignments FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM runtime_removal_operations operation
    WHERE operation.runtime_id = NEW.runtime_id
      AND operation.state IN ('pending', 'cleanup_pending')
)
BEGIN SELECT RAISE(ABORT, 'runtime_removal_in_progress'); END;

CREATE TRIGGER worker_assignment_update_blocked_by_runtime_removal
BEFORE UPDATE ON ticket_current_worker_assignments FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM runtime_removal_operations operation
    WHERE operation.runtime_id = NEW.runtime_id
      AND operation.state IN ('pending', 'cleanup_pending')
)
BEGIN SELECT RAISE(ABORT, 'runtime_removal_in_progress'); END;

CREATE TRIGGER workdir_attachment_insert_blocked_by_runtime_removal
BEFORE INSERT ON worker_workdir_links FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM runtime_removal_operations operation
    WHERE operation.runtime_id = NEW.runtime_id
      AND operation.state IN ('pending', 'cleanup_pending')
)
BEGIN SELECT RAISE(ABORT, 'runtime_removal_in_progress'); END;

CREATE TRIGGER workdir_attachment_update_blocked_by_runtime_removal
BEFORE UPDATE ON worker_workdir_links FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM runtime_removal_operations operation
    WHERE operation.runtime_id = NEW.runtime_id
      AND operation.state IN ('pending', 'cleanup_pending')
)
BEGIN SELECT RAISE(ABORT, 'runtime_removal_in_progress'); END;

CREATE TRIGGER worker_create_insert_blocked_by_runtime_removal
BEFORE INSERT ON worker_create_reservations FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM runtime_removal_operations operation
    WHERE operation.runtime_id = NEW.runtime_id
      AND operation.state IN ('pending', 'cleanup_pending')
)
BEGIN SELECT RAISE(ABORT, 'runtime_removal_in_progress'); END;

CREATE TRIGGER worker_create_update_blocked_by_runtime_removal
BEFORE UPDATE ON worker_create_reservations FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM runtime_removal_operations operation
    WHERE operation.runtime_id = NEW.runtime_id
      AND operation.state IN ('pending', 'cleanup_pending')
)
BEGIN SELECT RAISE(ABORT, 'runtime_removal_in_progress'); END;

CREATE TRIGGER workdir_create_insert_blocked_by_runtime_removal
BEFORE INSERT ON workdir_create_operations FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM runtime_removal_operations operation
    WHERE operation.runtime_id = NEW.resolved_runtime_id
      AND operation.state IN ('pending', 'cleanup_pending')
)
BEGIN SELECT RAISE(ABORT, 'runtime_removal_in_progress'); END;

CREATE TRIGGER workdir_create_update_blocked_by_runtime_removal
BEFORE UPDATE ON workdir_create_operations FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM runtime_removal_operations operation
    WHERE operation.runtime_id = NEW.resolved_runtime_id
      AND operation.state IN ('pending', 'cleanup_pending')
)
BEGIN SELECT RAISE(ABORT, 'runtime_removal_in_progress'); END;

CREATE TRIGGER worker_removal_insert_blocked_by_runtime_removal
BEFORE INSERT ON worker_removal_operations FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM runtime_removal_operations operation
    WHERE operation.runtime_id = NEW.runtime_id
      AND operation.state IN ('pending', 'cleanup_pending')
)
BEGIN SELECT RAISE(ABORT, 'runtime_removal_in_progress'); END;

CREATE TRIGGER worker_removal_update_blocked_by_runtime_removal
BEFORE UPDATE ON worker_removal_operations FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM runtime_removal_operations operation
    WHERE operation.runtime_id = NEW.runtime_id
      AND operation.state IN ('pending', 'cleanup_pending')
)
BEGIN SELECT RAISE(ABORT, 'runtime_removal_in_progress'); END;

CREATE TRIGGER workdir_removal_insert_blocked_by_runtime_removal
BEFORE INSERT ON workdir_removal_operations FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM runtime_removal_operations operation
    WHERE operation.runtime_id = NEW.runtime_id
      AND operation.state IN ('pending', 'cleanup_pending')
)
BEGIN SELECT RAISE(ABORT, 'runtime_removal_in_progress'); END;

CREATE TRIGGER workdir_removal_update_blocked_by_runtime_removal
BEFORE UPDATE ON workdir_removal_operations FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM runtime_removal_operations operation
    WHERE operation.runtime_id = NEW.runtime_id
      AND operation.state IN ('pending', 'cleanup_pending')
)
BEGIN SELECT RAISE(ABORT, 'runtime_removal_in_progress'); END;

CREATE TABLE workspace_deletion_operations (
    operation_id TEXT PRIMARY KEY,
    request_fingerprint TEXT NOT NULL,
    workspace_id TEXT NOT NULL,
    workspace_display_name TEXT NOT NULL,
    workspace_updated_at TEXT NOT NULL,
    owner_account_id TEXT NOT NULL,
    actor_account_id TEXT NOT NULL,
    state TEXT NOT NULL CHECK(state IN ('queued', 'running', 'blocked', 'failed', 'succeeded')),
    resource_counts_json TEXT NOT NULL,
    child_operation_ids_json TEXT NOT NULL,
    blockers_json TEXT NOT NULL,
    failure_category TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    completed_at TEXT,
    FOREIGN KEY(owner_account_id) REFERENCES accounts(account_id) ON DELETE RESTRICT,
    FOREIGN KEY(actor_account_id) REFERENCES accounts(account_id) ON DELETE RESTRICT
);
CREATE INDEX workspace_deletion_operations_workspace_recent
    ON workspace_deletion_operations(workspace_id, created_at DESC);

CREATE TRIGGER seed_worker_retention_policy_after_workspace_insert AFTER INSERT ON workspaces BEGIN
        INSERT INTO workspace_worker_retention_policy_snapshots
          (workspace_id,policy_id,policy_digest,session_disposition,metadata_disposition,archive_retention_kind,archive_retention_seconds,diagnostics_disposition,diagnostics_retention_seconds,created_at)
          VALUES(NEW.workspace_id,'workspace-default-conservative','sha256:30d45d5d58a10dadf95c3bf831e589ea10c81dc76da9575bfd9fb13942b3d198','archive','tombstone','forever',NULL,'purge',NULL,NEW.created_at);
        INSERT INTO workspace_worker_retention_policies(workspace_id,policy_id,policy_digest,updated_at)
          VALUES(NEW.workspace_id,'workspace-default-conservative','sha256:30d45d5d58a10dadf95c3bf831e589ea10c81dc76da9575bfd9fb13942b3d198',NEW.created_at);
      END;
CREATE TRIGGER ticket_assignment_operations_validate_insert
        BEFORE INSERT ON ticket_assignment_operations
        WHEN NOT EXISTS (
            SELECT 1 FROM typed_tickets AS ticket
            WHERE ticket.workspace_id = NEW.workspace_id AND ticket.ticket_id = NEW.ticket_id
        )
        BEGIN
            SELECT RAISE(ABORT, 'Ticket assignment operation must reference a Ticket in the same Workspace');
        END;
CREATE TRIGGER ticket_assignment_ticket_parent_tombstone
        BEFORE DELETE ON typed_tickets
        WHEN EXISTS (
            SELECT 1 FROM ticket_worker_assignments AS assignment
            WHERE assignment.workspace_id = OLD.workspace_id
              AND assignment.ticket_id = OLD.ticket_id
        )
        BEGIN
            INSERT OR IGNORE INTO ticket_assignment_ticket_tombstones (
                workspace_id, ticket_id, deleted_at
            ) VALUES (OLD.workspace_id, OLD.ticket_id, CURRENT_TIMESTAMP);
        END;
CREATE TRIGGER ticket_assignment_user_principal_insert
        BEFORE INSERT ON ticket_current_worker_assignments
        WHEN NEW.principal_kind = 'user'
        BEGIN
            SELECT CASE WHEN NOT EXISTS (
                SELECT 1 FROM accounts
                WHERE account_id = NEW.principal_id AND kind = 'user'
            ) THEN RAISE(ABORT, 'Ticket assignment user principal is not a valid Account user') END;
        END;
CREATE TRIGGER ticket_assignment_user_principal_update
        BEFORE UPDATE ON ticket_current_worker_assignments
        WHEN NEW.principal_kind = 'user'
        BEGIN
            SELECT CASE WHEN NOT EXISTS (
                SELECT 1 FROM accounts
                WHERE account_id = NEW.principal_id AND kind = 'user'
            ) THEN RAISE(ABORT, 'Ticket assignment user principal is not a valid Account user') END;
        END;
CREATE TRIGGER ticket_assignment_worker_parent_tombstone_delete
        BEFORE DELETE ON worker_registry
        WHEN EXISTS (
            SELECT 1 FROM ticket_worker_assignments AS assignment
            WHERE assignment.workspace_id = OLD.workspace_id
              AND assignment.principal_kind = 'worker'
              AND assignment.runtime_id = OLD.runtime_id
              AND assignment.worker_id = OLD.worker_id
        )
        BEGIN
            INSERT OR IGNORE INTO ticket_assignment_worker_tombstones (
                workspace_id, runtime_id, worker_id, deleted_at
            ) VALUES (OLD.workspace_id, OLD.runtime_id, OLD.worker_id, CURRENT_TIMESTAMP);
        END;
CREATE TRIGGER ticket_assignment_worker_parent_tombstone_move
        BEFORE UPDATE OF runtime_id ON worker_registry
        WHEN OLD.runtime_id != NEW.runtime_id
         AND EXISTS (
            SELECT 1 FROM ticket_worker_assignments AS assignment
            WHERE assignment.workspace_id = OLD.workspace_id
              AND assignment.principal_kind = 'worker'
              AND assignment.runtime_id = OLD.runtime_id
              AND assignment.worker_id = OLD.worker_id
        )
        BEGIN
            INSERT OR IGNORE INTO ticket_assignment_worker_tombstones (
                workspace_id, runtime_id, worker_id, deleted_at
            ) VALUES (OLD.workspace_id, OLD.runtime_id, OLD.worker_id, CURRENT_TIMESTAMP);
        END;
CREATE TRIGGER ticket_assignment_workspace_agent_insert
        BEFORE INSERT ON ticket_current_worker_assignments
        WHEN NEW.principal_kind = 'workspace_agent'
        BEGIN
            SELECT CASE WHEN NEW.principal_id != 'workspace-orchestrator'
                THEN RAISE(ABORT, 'Ticket assignment Workspace agent is not registered') END;
        END;
CREATE TRIGGER ticket_assignment_workspace_agent_update
        BEFORE UPDATE ON ticket_current_worker_assignments
        WHEN NEW.principal_kind = 'workspace_agent'
        BEGIN
            SELECT CASE WHEN NEW.principal_id != 'workspace-orchestrator'
                THEN RAISE(ABORT, 'Ticket assignment Workspace agent is not registered') END;
        END;
CREATE TRIGGER ticket_worker_assignment_events_validate_insert
        BEFORE INSERT ON ticket_worker_assignment_events
        WHEN (NEW.assignment_id IS NOT NULL AND NOT EXISTS (
                SELECT 1 FROM ticket_worker_assignments AS assignment
                WHERE assignment.workspace_id = NEW.workspace_id
                  AND assignment.ticket_id = NEW.ticket_id
                  AND assignment.role = NEW.role
                  AND assignment.assignment_id = NEW.assignment_id
            ))
            OR (NEW.previous_assignment_id IS NOT NULL AND NOT EXISTS (
                SELECT 1 FROM ticket_worker_assignments AS assignment
                WHERE assignment.workspace_id = NEW.workspace_id
                  AND assignment.ticket_id = NEW.ticket_id
                  AND assignment.role = NEW.role
                  AND assignment.assignment_id = NEW.previous_assignment_id
            ))
        BEGIN
            SELECT RAISE(ABORT, 'Ticket assignment event must reference the same Ticket role');
        END;
CREATE TRIGGER ticket_worker_assignments_validate_insert
        BEFORE INSERT ON ticket_worker_assignments
        WHEN NOT EXISTS (
                SELECT 1 FROM typed_tickets AS ticket
                WHERE ticket.workspace_id = NEW.workspace_id AND ticket.ticket_id = NEW.ticket_id
            )
            OR (NEW.principal_kind = 'worker' AND NOT EXISTS (
                SELECT 1 FROM worker_registry AS worker
                WHERE worker.workspace_id = NEW.workspace_id
                  AND worker.runtime_id = NEW.runtime_id
                  AND worker.worker_id = NEW.worker_id
            ))
            OR (NEW.principal_kind = 'user' AND NOT EXISTS (
                SELECT 1 FROM accounts
                WHERE account_id = NEW.principal_id AND kind = 'user'
            ))
            OR (NEW.principal_kind = 'workspace_agent' AND NEW.principal_id != 'workspace-orchestrator')
        BEGIN
            SELECT RAISE(ABORT, 'Ticket assignment principal is not valid in this Workspace');
        END;
CREATE TRIGGER ticket_worker_assignments_validate_update
        BEFORE UPDATE OF workspace_id, ticket_id, principal_kind, principal_id, runtime_id, worker_id
        ON ticket_worker_assignments
        WHEN NOT EXISTS (
                SELECT 1 FROM typed_tickets AS ticket
                WHERE ticket.workspace_id = NEW.workspace_id AND ticket.ticket_id = NEW.ticket_id
            )
            OR (NEW.principal_kind = 'worker' AND NOT EXISTS (
                SELECT 1 FROM worker_registry AS worker
                WHERE worker.workspace_id = NEW.workspace_id
                  AND worker.runtime_id = NEW.runtime_id
                  AND worker.worker_id = NEW.worker_id
            ))
            OR (NEW.principal_kind = 'user' AND NOT EXISTS (
                SELECT 1 FROM accounts
                WHERE account_id = NEW.principal_id AND kind = 'user'
            ))
            OR (NEW.principal_kind = 'workspace_agent' AND NEW.principal_id != 'workspace-orchestrator')
        BEGIN
            SELECT RAISE(ABORT, 'Ticket assignment principal is not valid in this Workspace');
        END;

CREATE TABLE workspace_config_grants (
    workspace_id TEXT NOT NULL,
    grant_id TEXT NOT NULL,
    runtime_id TEXT NOT NULL,
    worker_id TEXT NOT NULL,
    workdir_id TEXT NOT NULL,
    access TEXT NOT NULL CHECK(access IN ('read_only', 'read_write')),
    revoked INTEGER NOT NULL CHECK(revoked IN (0, 1)),
    created_by TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY(workspace_id, grant_id),
    UNIQUE(workspace_id, workdir_id),
    FOREIGN KEY(workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE
);
CREATE UNIQUE INDEX workspace_config_grants_active_worker
    ON workspace_config_grants(workspace_id, runtime_id, worker_id) WHERE revoked = 0;

CREATE TABLE workspace_drive_grants (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    grant_id INTEGER PRIMARY KEY AUTOINCREMENT,
    runtime_id TEXT NOT NULL,
    worker_id TEXT NOT NULL,
    access TEXT NOT NULL CHECK(access IN ('read_only', 'read_write')),
    revoked INTEGER NOT NULL DEFAULT 0 CHECK(revoked IN (0, 1)),
    created_by TEXT NOT NULL,
    created_at TEXT NOT NULL,
    revoked_by TEXT,
    revoked_at TEXT,
    CHECK((revoked=0 AND revoked_by IS NULL AND revoked_at IS NULL)
       OR (revoked=1 AND revoked_by IS NOT NULL AND revoked_at IS NOT NULL))
);
CREATE UNIQUE INDEX workspace_drive_grants_active_worker
    ON workspace_drive_grants(workspace_id, runtime_id, worker_id) WHERE revoked=0;
-- Retain grant audit across Worker deletion, but never carry its authority into
-- a later lifetime reusing the same Runtime/Worker identity.
CREATE TRIGGER workspace_drive_grants_worker_deleted BEFORE DELETE ON worker_registry BEGIN
    UPDATE workspace_drive_grants SET revoked=1, revoked_by='server:worker-removal',
        revoked_at=strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
    WHERE workspace_id=OLD.workspace_id AND runtime_id=OLD.runtime_id
      AND worker_id=OLD.worker_id AND revoked=0;
END;
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
    legacy_result_counter INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    mutation_kind TEXT,
    legacy_expected_counter INTEGER,
    expected_operation_id TEXT,
    expected_key_fingerprint TEXT,
    PRIMARY KEY (workspace_id, operation_id),
    FOREIGN KEY (workspace_id, operation_id)
        REFERENCES repository_secret_operations(workspace_id, operation_id) ON DELETE CASCADE
);
