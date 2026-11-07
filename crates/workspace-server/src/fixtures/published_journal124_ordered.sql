-- Frozen ordered Journal124 database produced by the ffa875411 owner.
-- Used only to create private migration-test inputs; never apply to adopted data.
PRAGMA foreign_keys=OFF;
BEGIN TRANSACTION;
CREATE TABLE __yoi_schema_migrations (
    version INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
INSERT INTO "__yoi_schema_migrations" ("version","name","applied_at") VALUES(52,'workspace schema baseline','2026-11-06 07:22:28');
INSERT INTO "__yoi_schema_migrations" ("version","name","applied_at") VALUES(53,'durable Workspace deletion operations','2026-11-06 07:22:28');
INSERT INTO "__yoi_schema_migrations" ("version","name","applied_at") VALUES(54,'Workspace signing identity authority','2026-11-06 07:22:28');
INSERT INTO "__yoi_schema_migrations" ("version","name","applied_at") VALUES(55,'Workspace Runtime binding state and identity mode','2026-11-06 07:22:28');
INSERT INTO "__yoi_schema_migrations" ("version","name","applied_at") VALUES(56,'Workspace-signed Runtime verification evidence','2026-11-06 07:22:28');
INSERT INTO "__yoi_schema_migrations" ("version","name","applied_at") VALUES(57,'convert legacy Server-issued Runtime bindings to Workspace identity','2026-11-06 07:22:28');
INSERT INTO "__yoi_schema_migrations" ("version","name","applied_at") VALUES(58,'remove obsolete Workdir Repository cache generation','2026-11-06 07:22:28');
INSERT INTO "__yoi_schema_migrations" ("version","name","applied_at") VALUES(59,'Workdir create credential candidate snapshots','2026-11-06 07:22:28');
INSERT INTO "__yoi_schema_migrations" ("version","name","applied_at") VALUES(60,'guarded Runtime removal operations','2026-11-06 07:22:28');
INSERT INTO "__yoi_schema_migrations" ("version","name","applied_at") VALUES(61,'remove obsolete Worker run generation','2026-11-06 07:22:28');
INSERT INTO "__yoi_schema_migrations" ("version","name","applied_at") VALUES(62,'durable Runtime-backed Worker registry projection','2026-11-06 07:22:28');
INSERT INTO "__yoi_schema_migrations" ("version","name","applied_at") VALUES(63,'multiple Worker Workdir attachments and Workdir display names','2026-11-06 07:22:28');
INSERT INTO "__yoi_schema_migrations" ("version","name","applied_at") VALUES(64,'typed client-hosted External Workdir grants','2026-11-06 07:22:28');
INSERT INTO "__yoi_schema_migrations" ("version","name","applied_at") VALUES(65,'durable External Workdir expiry cleanup state','2026-11-06 07:22:28');
INSERT INTO "__yoi_schema_migrations" ("version","name","applied_at") VALUES(66,'Ticket target collection and Workdir attachment capabilities','2026-11-06 07:22:28');
INSERT INTO "__yoi_schema_migrations" ("version","name","applied_at") VALUES(67,'External Workdir read-write access and optional expiry','2026-11-06 07:22:28');
INSERT INTO "__yoi_schema_migrations" ("version","name","applied_at") VALUES(68,'External Workdir explicit command permission','2026-11-06 07:22:28');
INSERT INTO "__yoi_schema_migrations" ("version","name","applied_at") VALUES(69,'External Workdir command attachment capabilities','2026-11-06 07:22:28');
INSERT INTO "__yoi_schema_migrations" ("version","name","applied_at") VALUES(70,'Backend-owned Runtime Worker jobs and structured results','2026-11-06 07:22:28');
INSERT INTO "__yoi_schema_migrations" ("version","name","applied_at") VALUES(71,'atomic Backend Job notification delivery claims','2026-11-06 07:22:28');
INSERT INTO "__yoi_schema_migrations" ("version","name","applied_at") VALUES(72,'durable bounded Backend Job dispatch queue','2026-11-06 07:22:28');
INSERT INTO "__yoi_schema_migrations" ("version","name","applied_at") VALUES(73,'durable Workspace Worker singleton ownership','2026-11-06 07:22:28');
INSERT INTO "__yoi_schema_migrations" ("version","name","applied_at") VALUES(74,'ordered Worker projection subscriptions without revision reconciliation','2026-11-06 07:22:28');
INSERT INTO "__yoi_schema_migrations" ("version","name","applied_at") VALUES(75,'Runtime-scoped durable Worker catalog identity','2026-11-06 07:22:28');
INSERT INTO "__yoi_schema_migrations" ("version","name","applied_at") VALUES(76,'preserve explicit observe grants for committed Worker Session archives','2026-11-06 07:22:28');
INSERT INTO "__yoi_schema_migrations" ("version","name","applied_at") VALUES(77,'durable terminal Backend Job Worker cleanup','2026-11-06 07:22:28');
INSERT INTO "__yoi_schema_migrations" ("version","name","applied_at") VALUES(78,'durable Workdir attachment connection identity','2026-11-06 07:22:28');
INSERT INTO "__yoi_schema_migrations" ("version","name","applied_at") VALUES(79,'Workspace config grants and logical Workdirs','2026-11-06 07:22:28');
INSERT INTO "__yoi_schema_migrations" ("version","name","applied_at") VALUES(80,'durable internal Worker Restore request identity','2026-11-06 07:22:28');
INSERT INTO "__yoi_schema_migrations" ("version","name","applied_at") VALUES(81,'Backend Job immutable resource serialization','2026-11-06 07:22:28');
INSERT INTO "__yoi_schema_migrations" ("version","name","applied_at") VALUES(82,'Ticket responsibility and unfinished work separation','2026-11-06 07:22:28');
INSERT INTO "__yoi_schema_migrations" ("version","name","applied_at") VALUES(83,'Generic Ticket Worker roles and durable claims','2026-11-06 07:22:28');
INSERT INTO "__yoi_schema_migrations" ("version","name","applied_at") VALUES(84,'durable Workspace Drive grants','2026-11-06 07:22:28');
INSERT INTO "__yoi_schema_migrations" ("version","name","applied_at") VALUES(85,'content-addressed Flow sources','2026-11-06 07:22:28');
INSERT INTO "__yoi_schema_migrations" ("version","name","applied_at") VALUES(86,'content-fenced config, Memory settings and Jobs','2026-11-06 07:22:28');
INSERT INTO "__yoi_schema_migrations" ("version","name","applied_at") VALUES(87,'Repository SSH key identities and encrypted operation objects','2026-11-06 07:22:28');
INSERT INTO "__yoi_schema_migrations" ("version","name","applied_at") VALUES(88,'Runtime binding identities and retention content contracts','2026-11-06 07:22:28');
INSERT INTO "__yoi_schema_migrations" ("version","name","applied_at") VALUES(89,'descriptive legacy evidence columns','2026-11-06 07:22:28');
INSERT INTO "__yoi_schema_migrations" ("version","name","applied_at") VALUES(90,'Runtime binding connection configuration and historical verification archive','2026-11-06 07:22:28');
INSERT INTO "__yoi_schema_migrations" ("version","name","applied_at") VALUES(91,'Ticket open/closed cases and retained routing evidence','2026-11-06 07:22:28');
INSERT INTO "__yoi_schema_migrations" ("version","name","applied_at") VALUES(92,'Ticket-owned assignment tracking','2026-11-06 07:22:28');
INSERT INTO "__yoi_schema_migrations" ("version","name","applied_at") VALUES(93,'Durable assignment deliveries and source-bound startup fences','2026-11-06 07:22:28');
INSERT INTO "__yoi_schema_migrations" ("version","name","applied_at") VALUES(94,'Canonical Ticket posts','2026-11-06 07:22:28');
INSERT INTO "__yoi_schema_migrations" ("version","name","applied_at") VALUES(95,'Common notification routing and durable Worker delivery','2026-11-06 07:22:29');
INSERT INTO "__yoi_schema_migrations" ("version","name","applied_at") VALUES(96,'Backend Job common notification routing','2026-11-06 07:22:29');
INSERT INTO "__yoi_schema_migrations" ("version","name","applied_at") VALUES(97,'MR notifications and authenticated implementation results','2026-11-06 07:22:29');
INSERT INTO "__yoi_schema_migrations" ("version","name","applied_at") VALUES(98,'Assignment-independent implementation reports','2026-11-06 07:22:29');
INSERT INTO "__yoi_schema_migrations" ("version","name","applied_at") VALUES(99,'Explicit hosting administrators and self-registration policy','2026-11-06 07:22:29');
INSERT INTO "__yoi_schema_migrations" ("version","name","applied_at") VALUES(100,'Provisional users and scoped passkey activation','2026-11-06 07:22:29');
INSERT INTO "__yoi_schema_migrations" ("version","name","applied_at") VALUES(101,'Durable explicit Subject body release','2026-11-06 07:22:29');
INSERT INTO "__yoi_schema_migrations" ("version","name","applied_at") VALUES(102,'Durable failed Subject creation recovery','2026-11-06 07:22:29');
INSERT INTO "__yoi_schema_migrations" ("version","name","applied_at") VALUES(103,'Passkey metadata and atomic addition results','2026-11-06 07:22:29');
INSERT INTO "__yoi_schema_migrations" ("version","name","applied_at") VALUES(104,'Converge Passkey results and Job input retention','2026-11-06 07:22:29');
INSERT INTO "__yoi_schema_migrations" ("version","name","applied_at") VALUES(105,'Durable Subject deletion snapshots and Job admission fences','2026-11-06 07:22:29');
INSERT INTO "__yoi_schema_migrations" ("version","name","applied_at") VALUES(106,'Workspace Plugin catalog and immutable artifact references','2026-11-06 07:22:29');
INSERT INTO "__yoi_schema_migrations" ("version","name","applied_at") VALUES(107,'Immutable Worker Plugin plans and retained pins','2026-11-06 07:22:29');
INSERT INTO "__yoi_schema_migrations" ("version","name","applied_at") VALUES(108,'Subject Ticket responsibility and current-body notification routing','2026-11-06 07:22:29');
INSERT INTO "__yoi_schema_migrations" ("version","name","applied_at") VALUES(109,'Durable Plugin Host invocations and HTTPS effect receipts','2026-11-06 07:22:29');
INSERT INTO "__yoi_schema_migrations" ("version","name","applied_at") VALUES(110,'Worker Plugin launch selection associations','2026-11-06 07:22:29');
INSERT INTO "__yoi_schema_migrations" ("version","name","applied_at") VALUES(111,'Typed Plugin Ticket extension data and atomic mutation receipts','2026-11-06 07:22:29');
INSERT INTO "__yoi_schema_migrations" ("version","name","applied_at") VALUES(112,'Ticket Service outbox and fenced consumers','2026-11-06 07:22:29');
INSERT INTO "__yoi_schema_migrations" ("version","name","applied_at") VALUES(113,'Ticket work-decomposition parent links','2026-11-06 07:22:29');
INSERT INTO "__yoi_schema_migrations" ("version","name","applied_at") VALUES(122,'Atomic Workspace source and Subject policy adoption','2026-11-06 07:22:31');
INSERT INTO "__yoi_schema_migrations" ("version","name","applied_at") VALUES(123,'Backend Job execution retirement and owned reconciliation','2026-11-06 07:22:31');
INSERT INTO "__yoi_schema_migrations" ("version","name","applied_at") VALUES(124,'Journal frozen scope on Worker removal intents','2026-11-06 07:22:31');
CREATE TABLE ticket_schema_migrations(version INTEGER PRIMARY KEY,name TEXT NOT NULL,applied_at TEXT NOT NULL);
INSERT INTO "ticket_schema_migrations" ("version","name","applied_at") VALUES(7,'ticket schema baseline','2026-11-06 07:22:28');
INSERT INTO "ticket_schema_migrations" ("version","name","applied_at") VALUES(10,'Ticket case and tag composition','2026-11-06T07:22:28.956566200+00:00');
INSERT INTO "ticket_schema_migrations" ("version","name","applied_at") VALUES(11,'Ticket-owned assignment tracking','2026-11-06T07:22:28.965228405+00:00');
INSERT INTO "ticket_schema_migrations" ("version","name","applied_at") VALUES(12,'Canonical Ticket posts','2026-11-06T07:22:28.986684691+00:00');
INSERT INTO "ticket_schema_migrations" ("version","name","applied_at") VALUES(13,'Ticket content change facts','2026-11-06T07:22:28.995203488+00:00');
INSERT INTO "ticket_schema_migrations" ("version","name","applied_at") VALUES(14,'Subject Ticket responsibility','2026-11-06T07:22:29.050638+00:00');
INSERT INTO "ticket_schema_migrations" ("version","name","applied_at") VALUES(15,'Ticket Service transactional outbox','2026-11-06T07:22:29.101823819+00:00');
INSERT INTO "ticket_schema_migrations" ("version","name","applied_at") VALUES(16,'Ticket work-decomposition parent links','2026-11-06T07:22:29.116979676+00:00');
CREATE TABLE merge_request_schema(singleton INTEGER PRIMARY KEY CHECK(singleton=1),version INTEGER NOT NULL);
INSERT INTO "merge_request_schema" ("singleton","version") VALUES(1,13);
CREATE TABLE merge_requests(workspace_id TEXT NOT NULL,merge_request_id TEXT NOT NULL,repository_id TEXT NOT NULL,state TEXT NOT NULL CHECK(state IN('open','merged','closed')),selector_from TEXT,selector_to TEXT NOT NULL,created_at TEXT NOT NULL,updated_at TEXT NOT NULL,PRIMARY KEY(workspace_id,merge_request_id),FOREIGN KEY(workspace_id,repository_id)REFERENCES repositories(workspace_id,repository_id));
CREATE TABLE merge_request_ticket_relations(workspace_id TEXT NOT NULL,merge_request_id TEXT NOT NULL,ticket_id TEXT NOT NULL,relation_kind TEXT NOT NULL CHECK(relation_kind='implements'),created_at TEXT NOT NULL,PRIMARY KEY(workspace_id,merge_request_id,ticket_id),FOREIGN KEY(workspace_id,merge_request_id)REFERENCES merge_requests(workspace_id,merge_request_id)ON DELETE CASCADE,FOREIGN KEY(workspace_id,ticket_id)REFERENCES typed_tickets(workspace_id,ticket_id)ON DELETE CASCADE);
CREATE TABLE merge_request_thread_events(workspace_id TEXT NOT NULL,merge_request_id TEXT NOT NULL,event_id TEXT NOT NULL,sequence INTEGER NOT NULL,kind TEXT NOT NULL CHECK(kind IN('review_requested','review','review_revoked','review_cancelled','comment','merge')),payload_json TEXT NOT NULL,operation_id TEXT,created_at TEXT NOT NULL,PRIMARY KEY(workspace_id,merge_request_id,event_id),UNIQUE(workspace_id,merge_request_id,sequence),FOREIGN KEY(workspace_id,merge_request_id)REFERENCES merge_requests(workspace_id,merge_request_id)ON DELETE CASCADE);
CREATE TABLE merge_request_review_grants(workspace_id TEXT NOT NULL,merge_request_id TEXT NOT NULL,request_event_id TEXT NOT NULL,subject_ref TEXT NOT NULL,reviewer_runtime_id TEXT NOT NULL,reviewer_worker_id TEXT NOT NULL,capability_token TEXT PRIMARY KEY,issued_at TEXT NOT NULL,consumed_at TEXT,revoked_at TEXT,status TEXT NOT NULL CHECK(status IN('issued','consumed','revoked')),FOREIGN KEY(workspace_id,merge_request_id,request_event_id)REFERENCES merge_request_thread_events(workspace_id,merge_request_id,event_id)ON DELETE CASCADE);
CREATE TABLE merge_request_reviewer_child_sessions(workspace_id TEXT NOT NULL,child_session_id TEXT NOT NULL,parent_runtime_id TEXT NOT NULL,parent_worker_id TEXT NOT NULL,reviewer_profile TEXT NOT NULL,registered_at TEXT NOT NULL,status TEXT NOT NULL CHECK(status IN('active','consumed')),PRIMARY KEY(workspace_id,child_session_id));
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
    body TEXT NOT NULL, event_ref TEXT NOT NULL DEFAULT '', edited_at TEXT, edited_by TEXT, archived INTEGER NOT NULL DEFAULT 0 CHECK (archived IN (0, 1)),
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
CREATE TABLE users (
    user_id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL UNIQUE REFERENCES accounts(account_id) ON DELETE CASCADE,
    handle TEXT NOT NULL UNIQUE,
    display_name TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
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
CREATE TABLE worker_registry_projection_diagnostics (
    diagnostic_id INTEGER PRIMARY KEY AUTOINCREMENT,
    workspace_id TEXT NOT NULL,
    runtime_id TEXT NOT NULL,
    worker_id TEXT NOT NULL,
    reason TEXT NOT NULL,
    observed_at TEXT NOT NULL
);
CREATE TABLE worker_retention_audit_events (
        event_id TEXT PRIMARY KEY, operation_id TEXT NOT NULL, workspace_id TEXT NOT NULL,
        event_kind TEXT NOT NULL, detail TEXT NOT NULL, created_at TEXT NOT NULL,
        FOREIGN KEY(operation_id) REFERENCES worker_removal_operations(operation_id),
        FOREIGN KEY(workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE);
CREATE TABLE worker_session_archive_observe_grants (
        workspace_id TEXT NOT NULL, archive_id TEXT NOT NULL,
        controller_runtime_id TEXT NOT NULL, controller_worker_id TEXT NOT NULL,
        subject_runtime_id TEXT NOT NULL, subject_worker_id TEXT NOT NULL,
        source_grant_id TEXT NOT NULL, granted_at TEXT NOT NULL, revoked_at TEXT,
        PRIMARY KEY(workspace_id,archive_id,controller_runtime_id,controller_worker_id,source_grant_id),
        FOREIGN KEY(workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
        FOREIGN KEY(archive_id) REFERENCES worker_session_archives(archive_id) ON DELETE CASCADE);
CREATE TABLE "worker_workdir_attachment_reservations" (
    workspace_id TEXT NOT NULL,
    workdir_id TEXT NOT NULL,
    reservation_id TEXT NOT NULL,
    reserved_at TEXT NOT NULL,
    PRIMARY KEY (workspace_id, workdir_id),
    FOREIGN KEY (workspace_id, workdir_id)
        REFERENCES "workdir_registry"(workspace_id, workdir_id) ON DELETE CASCADE
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
CREATE TABLE "workspaces" (
            workspace_id TEXT PRIMARY KEY,
            display_name TEXT NOT NULL,
            state TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            owner_account_id TEXT NOT NULL,
            FOREIGN KEY (owner_account_id) REFERENCES accounts(account_id) ON DELETE RESTRICT
        );
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
CREATE TABLE typed_tickets (
                    workspace_id TEXT NOT NULL,
                    ticket_id TEXT NOT NULL,
                    slug TEXT NOT NULL,
                    title TEXT NOT NULL,
                    status TEXT NOT NULL,
                    kind TEXT NOT NULL,
                    priority TEXT NOT NULL,
                    created_at TEXT,
                    updated_at TEXT,
                    assignee TEXT,
                    readiness TEXT,
                    workflow_state TEXT NOT NULL,
                    workflow_state_explicit INTEGER NOT NULL,
                    queued_by TEXT,
                    queued_at TEXT,
                    resolution TEXT, created_by TEXT,
                    PRIMARY KEY (workspace_id, ticket_id),
                    FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE
                );
CREATE TABLE typed_ticket_targets (
                    workspace_id TEXT NOT NULL,
                    ticket_id TEXT NOT NULL,
                    ordinal INTEGER NOT NULL,
                    repository_key TEXT NOT NULL,
                    ref_selector TEXT,
                    access TEXT NOT NULL CHECK (access IN ('read_only', 'read_write')),
                    PRIMARY KEY (workspace_id, ticket_id, ordinal),
                    UNIQUE (workspace_id, ticket_id, repository_key),
                    FOREIGN KEY (workspace_id, ticket_id)
                        REFERENCES typed_tickets(workspace_id, ticket_id) ON DELETE CASCADE,
                    FOREIGN KEY (workspace_id, repository_key)
                        REFERENCES repositories(workspace_id, repository_key) ON DELETE RESTRICT
                );
CREATE TABLE "external_workdir_grants" (
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
CREATE TABLE worker_registry_projection_removals (
            workspace_id TEXT NOT NULL,
            runtime_id TEXT NOT NULL,
            worker_id TEXT NOT NULL,
            PRIMARY KEY (workspace_id, runtime_id, worker_id)
        );
CREATE TABLE worker_registry (
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
CREATE TABLE worker_control_grants (
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
                    REFERENCES worker_registry(workspace_id, runtime_id, worker_id) ON DELETE CASCADE,
                FOREIGN KEY (workspace_id, subject_runtime_id, subject_worker_id)
                    REFERENCES worker_registry(workspace_id, runtime_id, worker_id) ON DELETE CASCADE
            );
CREATE TABLE worker_workdir_links (
                workspace_id TEXT NOT NULL,
                runtime_id TEXT NOT NULL,
                worker_id TEXT NOT NULL,
                workdir_id TEXT NOT NULL,
                alias TEXT NOT NULL,
                linked_at TEXT NOT NULL,
                unlinked_at TEXT,
                capabilities TEXT NOT NULL CHECK (capabilities IN (
                    'all', 'read_only', 'read_write', 'command_only', 'read_command'
                )), connection_id TEXT NOT NULL DEFAULT '',
                PRIMARY KEY (workspace_id, runtime_id, worker_id, workdir_id, alias),
                FOREIGN KEY (workspace_id, runtime_id, worker_id)
                    REFERENCES worker_registry(workspace_id, runtime_id, worker_id) ON DELETE CASCADE,
                FOREIGN KEY (workspace_id, workdir_id)
                    REFERENCES workdir_registry(workspace_id, workdir_id) ON DELETE CASCADE
            );
CREATE TABLE worker_restore_intents (
        workspace_id TEXT NOT NULL, runtime_id TEXT NOT NULL, worker_id TEXT NOT NULL,
        domain_key TEXT NOT NULL, request_id TEXT NOT NULL, expected_token TEXT NOT NULL,
        settled INTEGER NOT NULL CHECK(settled IN (0,1)),
        PRIMARY KEY(workspace_id,runtime_id,worker_id,request_id),
        FOREIGN KEY(workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE
    );
CREATE TABLE ticket_assignment_work_releases (
    workspace_id TEXT NOT NULL,
    assignment_id TEXT NOT NULL,
    released_at TEXT NOT NULL,
    PRIMARY KEY (workspace_id, assignment_id),
    FOREIGN KEY (workspace_id, assignment_id)
        REFERENCES ticket_worker_assignments(workspace_id, assignment_id) ON DELETE CASCADE
);
CREATE TABLE "ticket_assignment_operations" (
            workspace_id TEXT NOT NULL,
            operation_id TEXT NOT NULL,
            action TEXT NOT NULL CHECK (action IN ('assign', 'reassign', 'unassign')),
            ticket_id TEXT NOT NULL,
            role TEXT NOT NULL DEFAULT 'worker'
                CHECK(role IN ('orchestrator', 'worker', 'owner', 'contributor')),
            principal_kind TEXT NOT NULL DEFAULT 'worker'
                CHECK(principal_kind IN ('user', 'worker', 'workspace_agent', 'subject')),
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
CREATE TABLE "ticket_worker_assignments" (
            workspace_id TEXT NOT NULL,
            ticket_id TEXT NOT NULL,
            assignment_id TEXT NOT NULL,
            role TEXT NOT NULL DEFAULT 'worker'
                CHECK(role IN ('orchestrator', 'worker', 'owner', 'contributor')),
            principal_kind TEXT NOT NULL DEFAULT 'worker'
                CHECK(principal_kind IN ('user', 'worker', 'workspace_agent', 'subject')),
            principal_id TEXT,
            runtime_id TEXT,
            worker_id TEXT,
            assigned_by TEXT NOT NULL,
            assigned_at TEXT NOT NULL, tracking_json TEXT
    CHECK(tracking_json IS NULL OR (role = 'orchestrator' AND json_valid(tracking_json) AND json_type(tracking_json)='object'
      AND COALESCE(json_extract(tracking_json, '$.status') IN ('waiting', 'inprogress', 'done'), 0))), last_tracking_event_id TEXT, tracking_updated_at TEXT, tracking_migration_diagnostic TEXT,
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
CREATE TABLE "ticket_current_worker_assignments" (
            workspace_id TEXT NOT NULL,
            ticket_id TEXT NOT NULL,
            role TEXT NOT NULL DEFAULT 'worker'
                CHECK(role IN ('orchestrator', 'worker', 'owner', 'contributor')),
            assignment_id TEXT NOT NULL,
            principal_kind TEXT NOT NULL DEFAULT 'worker'
                CHECK(principal_kind IN ('user', 'worker', 'workspace_agent', 'subject')),
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
CREATE TABLE "ticket_worker_assignment_events" (
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
CREATE TABLE "worker_create_reservations" (
            workspace_id TEXT NOT NULL,
            allocation_key TEXT NOT NULL,
            worker_id TEXT NOT NULL,
            runtime_id TEXT NOT NULL,
            create_fingerprint TEXT NOT NULL,
            state TEXT NOT NULL CHECK (state IN ('reserved', 'created', 'removed')),
            request_fingerprint TEXT,
            memory_language TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL, singleton_key TEXT, singleton_generation INTEGER CHECK (singleton_generation IS NULL OR singleton_generation > 0),
            PRIMARY KEY (workspace_id, allocation_key),
            UNIQUE (workspace_id, worker_id),
            FOREIGN KEY(workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE
        );
CREATE TABLE backend_job_pre_content_history (
    workspace_id TEXT NOT NULL,
    job_id TEXT NOT NULL,
    purpose TEXT NOT NULL,
    input_revision TEXT NOT NULL,
    input_ref TEXT NOT NULL,
    resource_key TEXT,
    request_json TEXT NOT NULL CHECK (json_valid(request_json)),
    input_retired INTEGER NOT NULL CHECK (input_retired IN (0,1)),
    intent_fingerprint TEXT NOT NULL,
    current_attempt INTEGER NOT NULL,
    state TEXT NOT NULL,
    result_digest TEXT,
    failure_category TEXT,
    failure_detail TEXT,
    updated_at TEXT NOT NULL,
    completed_at TEXT,
    captured_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    PRIMARY KEY (workspace_id,job_id),
    FOREIGN KEY (workspace_id,job_id) REFERENCES backend_jobs(workspace_id,job_id) ON DELETE CASCADE
);
CREATE TABLE backend_job_attempt_pre_content_history (
    workspace_id TEXT NOT NULL,
    job_id TEXT NOT NULL,
    attempt_id TEXT NOT NULL,
    attempt INTEGER NOT NULL,
    input_revision TEXT NOT NULL,
    state TEXT NOT NULL,
    runtime_id TEXT,
    worker_id TEXT,
    runtime_run_id TEXT,
    dispatched_at TEXT,
    deadline_at TEXT NOT NULL,
    result_digest TEXT,
    failure_category TEXT,
    failure_detail TEXT,
    worker_cleanup_state TEXT,
    worker_cleanup_failure_category TEXT,
    worker_cleanup_failure_detail TEXT,
    worker_cleanup_updated_at TEXT,
    worker_cleanup_completed_at TEXT,
    updated_at TEXT NOT NULL,
    completed_at TEXT,
    captured_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    PRIMARY KEY (workspace_id,job_id,attempt_id),
    FOREIGN KEY (workspace_id,job_id,attempt_id) REFERENCES backend_job_attempts(workspace_id,job_id,attempt_id) ON DELETE CASCADE,
    FOREIGN KEY (workspace_id,job_id) REFERENCES backend_job_pre_content_history(workspace_id,job_id) ON DELETE CASCADE
);
CREATE TABLE backend_job_delivery_pre_content_history (
    workspace_id TEXT NOT NULL,
    delivery_id TEXT NOT NULL,
    job_id TEXT NOT NULL,
    attempt_id TEXT NOT NULL,
    target_runtime_id TEXT NOT NULL,
    target_worker_id TEXT NOT NULL,
    state TEXT NOT NULL,
    failure_category TEXT,
    failure_detail TEXT,
    updated_at TEXT NOT NULL,
    delivered_at TEXT,
    captured_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    PRIMARY KEY (workspace_id,delivery_id),
    FOREIGN KEY (workspace_id,delivery_id) REFERENCES backend_job_deliveries(workspace_id,delivery_id) ON DELETE CASCADE
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
CREATE TABLE workdir_create_legacy_ssh_archives (
    workspace_id TEXT NOT NULL,
    operation_id TEXT NOT NULL,
    operation_json TEXT NOT NULL CHECK (json_valid(operation_json)),
    candidates_json TEXT NOT NULL CHECK (json_valid(candidates_json)),
    PRIMARY KEY (workspace_id, operation_id),
    FOREIGN KEY (workspace_id, operation_id)
        REFERENCES workdir_create_operations(workspace_id, operation_id) ON DELETE CASCADE
);
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
CREATE TABLE "workspace_runtime_binding_audit" (
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
CREATE TABLE "workspace_signing_identities" (
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
CREATE TABLE "workspace_signing_identity_provisioning_operations" (
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
CREATE TABLE "workspace_signing_identity_audit" (
            event_id TEXT PRIMARY KEY,
            workspace_id TEXT NOT NULL,
            key_id TEXT NOT NULL,
            action TEXT NOT NULL CHECK (action IN ('provisioned')),
            public_key_fingerprint TEXT NOT NULL,
            actor_account_id TEXT NOT NULL,
            created_at TEXT NOT NULL,
            FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE
        );
CREATE TABLE "runtime_removal_operations" (
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
CREATE TABLE "workspace_worker_retention_policies" (
        workspace_id TEXT PRIMARY KEY, policy_id TEXT NOT NULL, policy_digest TEXT NOT NULL, updated_at TEXT NOT NULL,
        FOREIGN KEY(workspace_id,policy_id,policy_digest) REFERENCES workspace_worker_retention_policy_snapshots(workspace_id,policy_id,policy_digest),
        FOREIGN KEY(workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE);
CREATE TABLE "workspace_worker_retention_policy_snapshots" (
        workspace_id TEXT NOT NULL, policy_id TEXT NOT NULL, policy_digest TEXT NOT NULL CHECK(length(policy_digest)>0),
        session_disposition TEXT NOT NULL CHECK(session_disposition IN ('archive','purge')),
        metadata_disposition TEXT NOT NULL CHECK(metadata_disposition IN ('tombstone','purge')),
        archive_retention_kind TEXT NOT NULL CHECK(archive_retention_kind IN ('forever','for_seconds')),
        archive_retention_seconds INTEGER,
        diagnostics_disposition TEXT NOT NULL CHECK(diagnostics_disposition IN ('purge','retain')),
        diagnostics_retention_seconds INTEGER, created_at TEXT NOT NULL,
        PRIMARY KEY(workspace_id,policy_id,policy_digest),
        FOREIGN KEY(workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE);
CREATE TABLE "worker_removal_operations" (
        operation_id TEXT PRIMARY KEY, plan_id TEXT NOT NULL UNIQUE, input_fingerprint TEXT NOT NULL,
        workspace_id TEXT NOT NULL, runtime_id TEXT NOT NULL, worker_id TEXT NOT NULL,
        worker_updated_at TEXT NOT NULL,
        policy_id TEXT NOT NULL, policy_digest TEXT NOT NULL,
        session_disposition TEXT NOT NULL, metadata_disposition TEXT NOT NULL,
        archive_retention_kind TEXT NOT NULL, archive_retention_seconds INTEGER,
        diagnostics_disposition TEXT NOT NULL,
        diagnostics_retention_seconds INTEGER, archive_id TEXT UNIQUE, blockers_json TEXT NOT NULL,
        state TEXT NOT NULL CHECK(state IN ('planned','blocked','executing','failed','stale','succeeded')),
        reason TEXT NOT NULL, failure_category TEXT, created_at TEXT NOT NULL, updated_at TEXT NOT NULL, journal_removal_json TEXT
    CHECK(journal_removal_json IS NULL OR (
        json_valid(journal_removal_json)
        AND json_extract(journal_removal_json,'$.phase') IN
            ('awaiting_quiescence','recording','awaiting_effects_end','released','completed')
    )),
        FOREIGN KEY(workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE);
CREATE TABLE "worker_session_archives" (
        archive_id TEXT PRIMARY KEY, workspace_id TEXT NOT NULL, runtime_id TEXT NOT NULL, worker_id TEXT NOT NULL,
        session_id TEXT NOT NULL, checksum_sha256 TEXT NOT NULL, content_bytes INTEGER NOT NULL,
        policy_id TEXT NOT NULL, policy_digest TEXT NOT NULL, operation_id TEXT NOT NULL UNIQUE,
        committed_at TEXT NOT NULL, expires_at TEXT,
        FOREIGN KEY(workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
        FOREIGN KEY(operation_id) REFERENCES worker_removal_operations(operation_id));
CREATE TABLE "worker_diagnostics_archives" (
        operation_id TEXT PRIMARY KEY, workspace_id TEXT NOT NULL, runtime_id TEXT NOT NULL,
        worker_id TEXT NOT NULL, policy_id TEXT NOT NULL, policy_digest TEXT NOT NULL,
        committed_at TEXT NOT NULL, expires_at TEXT NOT NULL,
        FOREIGN KEY(operation_id) REFERENCES worker_removal_operations(operation_id),
        FOREIGN KEY(workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE);
CREATE TABLE "worker_tombstones" (
        workspace_id TEXT NOT NULL, runtime_id TEXT NOT NULL, worker_id TEXT NOT NULL,
        display_name TEXT NOT NULL, profile TEXT, worker_created_at TEXT NOT NULL, removed_at TEXT NOT NULL,
        archive_id TEXT, policy_id TEXT NOT NULL, policy_digest TEXT NOT NULL, operation_id TEXT NOT NULL UNIQUE,
        PRIMARY KEY(workspace_id,runtime_id,worker_id),
        FOREIGN KEY(workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
        FOREIGN KEY(archive_id) REFERENCES worker_session_archives(archive_id),
        FOREIGN KEY(operation_id) REFERENCES worker_removal_operations(operation_id));
CREATE TABLE "workdir_removal_operations" (
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
CREATE TABLE "workspace_deletion_operations" (
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
CREATE TABLE "workspace_config_tree_history" (
                workspace_id TEXT NOT NULL, content_digest TEXT NOT NULL,
                toolchain_fingerprint TEXT NOT NULL, projection_digest TEXT NOT NULL,
                manifest_json TEXT NOT NULL, created_at TEXT NOT NULL, schema_bundle_json TEXT NOT NULL DEFAULT '{"contributions":[],"source":"{}","fingerprint":""}',
                PRIMARY KEY(workspace_id,content_digest,toolchain_fingerprint,projection_digest),
                FOREIGN KEY(workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE);
CREATE TABLE workspace_runtime_binding_verification_archive (
    workspace_id TEXT NOT NULL,
    runtime_id TEXT NOT NULL,
    binding_id TEXT NOT NULL,
    state TEXT NOT NULL,
    workspace_key_id TEXT,
    workspace_public_key_fingerprint TEXT,
    workspace_trust_id TEXT,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (workspace_id, runtime_id, binding_id),
    FOREIGN KEY(workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE
);
CREATE TABLE workspace_runtime_verification_archive (
    workspace_id TEXT NOT NULL,
    runtime_id TEXT NOT NULL,
    binding_id TEXT NOT NULL,
    workspace_key_id TEXT NOT NULL,
    workspace_public_key_fingerprint TEXT NOT NULL,
    workspace_trust_id TEXT NOT NULL,
    runtime_public_key_fingerprint TEXT NOT NULL,
    challenge_id TEXT NOT NULL,
    state TEXT NOT NULL,
    last_outcome TEXT NOT NULL,
    verified_at TEXT,
    checked_at TEXT NOT NULL,
    PRIMARY KEY (workspace_id, runtime_id),
    FOREIGN KEY(workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE
);
CREATE TABLE "workspace_runtime_bindings" (
    workspace_id TEXT NOT NULL,
    runtime_id TEXT NOT NULL,
    display_name TEXT NOT NULL,
    base_url TEXT NOT NULL,
    public_key TEXT NOT NULL,
    public_key_fingerprint TEXT NOT NULL,
    binding_id TEXT NOT NULL CHECK (length(binding_id) > 0),
    authentication_mode TEXT NOT NULL CHECK (authentication_mode IN ('legacy_server_issuer', 'workspace_identity')),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    revoked_at TEXT,
    PRIMARY KEY (workspace_id, runtime_id),
    UNIQUE (workspace_id, public_key_fingerprint),
    FOREIGN KEY(workspace_id) REFERENCES workspaces(workspace_id) ON DELETE RESTRICT
);
CREATE TABLE workspace_tags (
    workspace_id TEXT NOT NULL,
    tag_id TEXT NOT NULL,
    name TEXT NOT NULL,
    normalized_name TEXT NOT NULL,
    last_operation_key TEXT NOT NULL,
    deleted INTEGER NOT NULL CHECK (deleted IN (0, 1)),
    PRIMARY KEY (workspace_id, tag_id)
);
CREATE TABLE typed_ticket_tags (
    workspace_id TEXT NOT NULL,
    ticket_id TEXT NOT NULL,
    tag_id TEXT NOT NULL,
    PRIMARY KEY (workspace_id, ticket_id, tag_id),
    FOREIGN KEY (workspace_id, ticket_id) REFERENCES typed_tickets(workspace_id, ticket_id) ON DELETE CASCADE,
    FOREIGN KEY (workspace_id, tag_id) REFERENCES workspace_tags(workspace_id, tag_id) ON DELETE RESTRICT
);
CREATE TABLE workspace_tag_operations (
    workspace_id TEXT NOT NULL,
    operation_key TEXT NOT NULL,
    kind TEXT NOT NULL,
    target_id TEXT NOT NULL,
    input_json TEXT NOT NULL,
    source_context_json TEXT NOT NULL,
    before_json TEXT NOT NULL,
    result_json TEXT NOT NULL,
    at TEXT NOT NULL,
    PRIMARY KEY (workspace_id, operation_key)
);
CREATE TABLE typed_ticket_case_metadata (
    workspace_id TEXT NOT NULL, ticket_id TEXT NOT NULL, closure_operation_key TEXT,
    PRIMARY KEY (workspace_id, ticket_id),
    FOREIGN KEY (workspace_id, ticket_id) REFERENCES typed_tickets(workspace_id, ticket_id) ON DELETE CASCADE
);
CREATE TABLE typed_ticket_case_history (
    workspace_id TEXT NOT NULL, ticket_id TEXT NOT NULL, ordinal INTEGER NOT NULL,
    operation_key TEXT NOT NULL, request_fingerprint TEXT NOT NULL, judgment_json TEXT NOT NULL,
    PRIMARY KEY (workspace_id, ticket_id, operation_key),
    UNIQUE (workspace_id, ticket_id, ordinal),
    FOREIGN KEY (workspace_id, ticket_id) REFERENCES typed_tickets(workspace_id, ticket_id) ON DELETE CASCADE
);
CREATE TABLE typed_ticket_legacy_workflow_snapshot (
    workspace_id TEXT NOT NULL, ticket_id TEXT NOT NULL, snapshot_json TEXT NOT NULL,
    PRIMARY KEY (workspace_id, ticket_id),
    FOREIGN KEY (workspace_id, ticket_id) REFERENCES typed_tickets(workspace_id, ticket_id) ON DELETE CASCADE
);
CREATE TABLE typed_ticket_assignment_tracking_history (
    workspace_id TEXT NOT NULL,
    ticket_id TEXT NOT NULL,
    assignment_id TEXT NOT NULL,
    event_id TEXT NOT NULL,
    operation_key TEXT NOT NULL,
    tracking_json TEXT NOT NULL CHECK(json_valid(tracking_json)),
    source_context_json TEXT NOT NULL CHECK(json_valid(source_context_json)),
    at TEXT NOT NULL,
    PRIMARY KEY(workspace_id, event_id),
    UNIQUE(workspace_id, assignment_id, operation_key),
    FOREIGN KEY(workspace_id, ticket_id, assignment_id)
      REFERENCES ticket_worker_assignments(workspace_id, ticket_id, assignment_id)
);
CREATE TABLE typed_ticket_assignment_tracking_operations (
    workspace_id TEXT NOT NULL,
    operation_key TEXT NOT NULL,
    ticket_id TEXT NOT NULL,
    input_json TEXT NOT NULL,
    source_context_json TEXT NOT NULL,
    result_json TEXT NOT NULL,
    PRIMARY KEY(workspace_id, operation_key)
);
CREATE TABLE ticket_assignment_delivery_origins (
 workspace_id TEXT NOT NULL, ticket_id TEXT NOT NULL, assignment_id TEXT NOT NULL,
 input_event_id TEXT NOT NULL, after_ticket_event_index INTEGER NOT NULL,
 PRIMARY KEY(workspace_id,assignment_id),
 FOREIGN KEY(workspace_id,ticket_id,assignment_id) REFERENCES ticket_worker_assignments(workspace_id,ticket_id,assignment_id)
);
CREATE TABLE ticket_assignment_deliveries (
 workspace_id TEXT NOT NULL, assignment_id TEXT NOT NULL, input_event_id TEXT NOT NULL,
 delivery_id TEXT NOT NULL, payload_json TEXT NOT NULL CHECK(json_valid(payload_json)),
 reserved_source_json TEXT NOT NULL CHECK(json_valid(reserved_source_json)),
 PRIMARY KEY(workspace_id,assignment_id,input_event_id), UNIQUE(workspace_id,delivery_id),
 FOREIGN KEY(workspace_id,assignment_id) REFERENCES ticket_worker_assignments(workspace_id,assignment_id)
);
CREATE TABLE ticket_assignment_delivery_requests (
 workspace_id TEXT NOT NULL, delivery_id TEXT NOT NULL, content TEXT NOT NULL,
 PRIMARY KEY(workspace_id,delivery_id),
 FOREIGN KEY(workspace_id,delivery_id) REFERENCES ticket_assignment_deliveries(workspace_id,delivery_id)
);
CREATE TABLE ticket_assignment_delivery_acknowledgements (
 workspace_id TEXT NOT NULL, delivery_id TEXT NOT NULL,
 source_json TEXT NOT NULL CHECK(json_valid(source_json)), PRIMARY KEY(workspace_id,delivery_id),
 FOREIGN KEY(workspace_id,delivery_id) REFERENCES ticket_assignment_deliveries(workspace_id,delivery_id)
);
CREATE TABLE ticket_assignment_operation_fences (
 workspace_id TEXT NOT NULL, operation_id TEXT NOT NULL, orchestrator_assignment_id TEXT NOT NULL,
 runtime_id TEXT NOT NULL, worker_id TEXT NOT NULL, agent_key TEXT NOT NULL,
 authentication_binding_id TEXT NOT NULL, source_json TEXT NOT NULL CHECK(json_valid(source_json)),
 PRIMARY KEY(workspace_id,operation_id),
 FOREIGN KEY(workspace_id,operation_id) REFERENCES ticket_assignment_operations(workspace_id,operation_id),
 FOREIGN KEY(workspace_id,orchestrator_assignment_id) REFERENCES ticket_worker_assignments(workspace_id,assignment_id)
);
CREATE TABLE ticket_assignment_content_inputs (
    input_sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    workspace_id TEXT NOT NULL,
    ticket_id TEXT NOT NULL,
    assignment_id TEXT NOT NULL,
    after_ticket_event_index INTEGER NOT NULL, actor TEXT, kind TEXT NOT NULL DEFAULT 'content_edit', source_change_id TEXT REFERENCES typed_ticket_content_changes(change_id),
    FOREIGN KEY(workspace_id,ticket_id,assignment_id)
        REFERENCES ticket_worker_assignments(workspace_id,ticket_id,assignment_id)
);
CREATE TABLE typed_ticket_content_changes (
    change_id TEXT NOT NULL PRIMARY KEY CHECK (length(trim(change_id)) > 0),
    workspace_id TEXT NOT NULL,
    ticket_id TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('content_edit', 'comment_edit')),
    actor TEXT CHECK (actor IS NULL OR length(trim(actor)) > 0),
    at TEXT NOT NULL,
    title_changed INTEGER NOT NULL CHECK (title_changed IN (0, 1)),
    body_changed INTEGER NOT NULL CHECK (body_changed IN (0, 1)),
    targets_changed INTEGER NOT NULL CHECK (targets_changed IN (0, 1)),
    source_event_ref TEXT,
    CHECK (title_changed + body_changed + targets_changed > 0),
    CHECK ((body_changed = 0 AND source_event_ref IS NULL) OR
           (body_changed = 1 AND source_event_ref IS NOT NULL AND length(trim(source_event_ref)) > 0)),
    CHECK (kind != 'comment_edit' OR (title_changed = 0 AND body_changed = 1 AND targets_changed = 0)),
    FOREIGN KEY (workspace_id, ticket_id) REFERENCES typed_tickets(workspace_id, ticket_id) ON DELETE CASCADE
);
CREATE TABLE notification_decisions (
 source_sequence INTEGER PRIMARY KEY REFERENCES notification_sources(sequence),
 workspace_id TEXT NOT NULL, delivery_id TEXT NOT NULL,
 disposition TEXT NOT NULL CHECK(disposition IN ('deliver','suppress','defer')),
 reason TEXT NOT NULL, purpose TEXT NOT NULL,
 last_offered INTEGER NOT NULL DEFAULT 0,
 UNIQUE(workspace_id,delivery_id)
);
CREATE TABLE notification_requests (
 workspace_id TEXT NOT NULL, delivery_id TEXT NOT NULL, content TEXT NOT NULL,
 target_json TEXT NOT NULL CHECK(json_valid(target_json)),
 PRIMARY KEY(workspace_id,delivery_id),
 FOREIGN KEY(workspace_id,delivery_id) REFERENCES notification_decisions(workspace_id,delivery_id)
);
CREATE TABLE notification_acknowledgements (
 workspace_id TEXT NOT NULL, delivery_id TEXT NOT NULL,
 target_json TEXT NOT NULL CHECK(json_valid(target_json)),
 PRIMARY KEY(workspace_id,delivery_id),
 FOREIGN KEY(workspace_id,delivery_id) REFERENCES notification_requests(workspace_id,delivery_id)
);
CREATE TABLE notification_failures (
 sequence INTEGER PRIMARY KEY AUTOINCREMENT, workspace_id TEXT NOT NULL,
 delivery_id TEXT NOT NULL, code TEXT NOT NULL,
 recorded_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
 FOREIGN KEY(workspace_id,delivery_id) REFERENCES notification_decisions(workspace_id,delivery_id)
);
CREATE TABLE notification_assignment_targets (
 workspace_id TEXT NOT NULL, delivery_id TEXT NOT NULL,
 runtime_id TEXT NOT NULL, worker_id TEXT NOT NULL, agent_key TEXT NOT NULL,
 authentication_binding_id TEXT NOT NULL,
 PRIMARY KEY(workspace_id,delivery_id,runtime_id,worker_id,agent_key,authentication_binding_id),
 FOREIGN KEY(workspace_id,delivery_id) REFERENCES ticket_assignment_deliveries(workspace_id,delivery_id)
);
CREATE TABLE "notification_sources" (
        sequence INTEGER PRIMARY KEY AUTOINCREMENT,
        workspace_id TEXT NOT NULL, ticket_id TEXT, input_event_id TEXT NOT NULL,
        role TEXT NOT NULL, assignment_id TEXT NOT NULL, kind TEXT NOT NULL,
        actor TEXT, event_index INTEGER, facts_json TEXT NOT NULL CHECK(json_valid(facts_json)),
        CHECK(ticket_id IS NOT NULL OR role='backend_job'),
        UNIQUE(workspace_id,ticket_id,input_event_id,role,assignment_id),
        FOREIGN KEY(workspace_id,ticket_id) REFERENCES typed_tickets(workspace_id,ticket_id));
CREATE TABLE notification_job_targets (
            workspace_id TEXT NOT NULL, delivery_id TEXT NOT NULL, target_json TEXT NOT NULL CHECK(json_valid(target_json)),
            PRIMARY KEY(workspace_id,delivery_id),
            FOREIGN KEY(workspace_id,delivery_id) REFERENCES notification_requests(workspace_id,delivery_id));
CREATE TABLE notification_job_legacy_receipts (
            workspace_id TEXT NOT NULL, delivery_id TEXT NOT NULL, record_json TEXT NOT NULL CHECK(json_valid(record_json)),
            runtime_accepted INTEGER NOT NULL CHECK(runtime_accepted IN (0,1)),
            PRIMARY KEY(workspace_id,delivery_id),
            FOREIGN KEY(workspace_id,delivery_id) REFERENCES notification_decisions(workspace_id,delivery_id));
CREATE TABLE notification_source_preparation_failures (
        sequence INTEGER PRIMARY KEY AUTOINCREMENT,
        source_sequence INTEGER NOT NULL REFERENCES notification_sources(sequence),
        code TEXT NOT NULL,
        recorded_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')));
CREATE TABLE notification_source_offers (
        source_sequence INTEGER PRIMARY KEY REFERENCES notification_sources(sequence),
        last_offered INTEGER NOT NULL CHECK(last_offered > 0));
CREATE TABLE merge_request_source_observations (
 sequence INTEGER PRIMARY KEY AUTOINCREMENT,
 workspace_id TEXT NOT NULL, merge_request_id TEXT NOT NULL,
 subject_ref TEXT NOT NULL, previous_subject_ref TEXT,
 observed_at TEXT NOT NULL,
 FOREIGN KEY(workspace_id,merge_request_id) REFERENCES merge_requests(workspace_id,merge_request_id)
);
CREATE TABLE ticket_implementation_delegations (
 workspace_id TEXT NOT NULL, assignment_id TEXT NOT NULL, orchestrator_assignment_id TEXT,
 PRIMARY KEY(workspace_id,assignment_id),
 FOREIGN KEY(workspace_id,assignment_id) REFERENCES ticket_worker_assignments(workspace_id,assignment_id),
 FOREIGN KEY(workspace_id,orchestrator_assignment_id) REFERENCES ticket_worker_assignments(workspace_id,assignment_id)
);
CREATE TABLE "ticket_implementation_results" (
 workspace_id TEXT NOT NULL, ticket_id TEXT NOT NULL, operation_key TEXT NOT NULL,
 result_id TEXT NOT NULL,
 runtime_id TEXT NOT NULL, worker_id TEXT NOT NULL, request_json TEXT NOT NULL CHECK(json_valid(request_json)),
 created_at TEXT NOT NULL,
 PRIMARY KEY(workspace_id,operation_key), UNIQUE(workspace_id,result_id),
 FOREIGN KEY(workspace_id,ticket_id) REFERENCES typed_tickets(workspace_id,ticket_id)
);
CREATE TABLE hosting_administrators (
            user_id TEXT PRIMARY KEY REFERENCES users(user_id) ON DELETE CASCADE,
            created_at TEXT NOT NULL
        );
CREATE TABLE hosting_settings (
            singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
            self_registration_enabled INTEGER NOT NULL DEFAULT 0 CHECK (self_registration_enabled IN (0, 1))
        );
INSERT INTO "hosting_settings" ("singleton","self_registration_enabled") VALUES(1,0);
CREATE TABLE user_activations (
    user_id TEXT PRIMARY KEY REFERENCES users(user_id) ON DELETE CASCADE,
    issuance_id TEXT NOT NULL UNIQUE,
    code_hash TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    failed_attempts INTEGER NOT NULL DEFAULT 0 CHECK (failed_attempts BETWEEN 0 AND 5)
);
CREATE TABLE subject_worker_releases (
    workspace_id TEXT NOT NULL,
    runtime_id TEXT NOT NULL,
    worker_id TEXT NOT NULL,
    singleton_key TEXT NOT NULL,
    singleton_generation INTEGER NOT NULL CHECK (singleton_generation > 0),
    binding_id TEXT NOT NULL,
    operation_id TEXT NOT NULL,
    proof TEXT CHECK (proof IN ('runtime_disconnected','erased')),
    completed INTEGER NOT NULL DEFAULT 0 CHECK (completed IN (0,1)),
    cancelled INTEGER NOT NULL DEFAULT 0 CHECK (cancelled IN (0,1)),
    PRIMARY KEY (workspace_id, operation_id),
    CHECK (cancelled = 0 OR (completed = 0 AND proof IS NULL)),
    FOREIGN KEY (workspace_id, worker_id) REFERENCES worker_create_reservations(workspace_id, worker_id) ON DELETE CASCADE
);
CREATE TABLE subject_create_recoveries (
    workspace_id TEXT NOT NULL,
    runtime_id TEXT NOT NULL,
    worker_id TEXT NOT NULL,
    singleton_key TEXT NOT NULL,
    singleton_generation INTEGER NOT NULL CHECK (singleton_generation > 0),
    operation_id TEXT NOT NULL CHECK (length(operation_id) > 0),
    create_fingerprint TEXT NOT NULL,
    completed INTEGER NOT NULL DEFAULT 0 CHECK (completed IN (0,1)),
    PRIMARY KEY (workspace_id, operation_id),
    UNIQUE (workspace_id, runtime_id, worker_id, singleton_key, singleton_generation),
    FOREIGN KEY (workspace_id, worker_id) REFERENCES worker_create_reservations(workspace_id, worker_id) ON DELETE CASCADE
);
CREATE TABLE passkey_metadata (
    credential_id TEXT PRIMARY KEY REFERENCES passkey_credentials(credential_id) ON DELETE CASCADE,
    key_reference TEXT NOT NULL UNIQUE,
    normal_login_verified_at TEXT
);
CREATE TABLE passkey_addition_receipts (
    challenge_id TEXT PRIMARY KEY,
    credential_id TEXT NOT NULL UNIQUE REFERENCES passkey_credentials(credential_id) ON DELETE CASCADE,
    user_id TEXT NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    authentication_reference TEXT NOT NULL
);
CREATE TABLE subject_deletions (
    workspace_id TEXT NOT NULL,
    subject_id TEXT NOT NULL,
    operation_id TEXT NOT NULL CHECK(length(operation_id)>0),
    completed INTEGER NOT NULL DEFAULT 0 CHECK(completed IN (0,1)),
    jobs_erased INTEGER NOT NULL DEFAULT 0 CHECK(jobs_erased IN (0,1)),
    PRIMARY KEY(workspace_id,subject_id),
    UNIQUE(workspace_id,operation_id),
    FOREIGN KEY(workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE
);
CREATE TABLE subject_deletion_workers (
    workspace_id TEXT NOT NULL,
    subject_id TEXT NOT NULL,
    runtime_id TEXT NOT NULL,
    worker_id TEXT NOT NULL,
    is_job INTEGER NOT NULL CHECK(is_job IN (0,1)),
    -- Exact canonical success already present at prepare, never a later receipt.
    prior_removal_operation_id TEXT REFERENCES worker_removal_operations(operation_id),
    -- Confirmed by the scoped Runtime storage owner, not catalog absence or Stop ACK.
    originals_absent INTEGER NOT NULL DEFAULT 0 CHECK(originals_absent IN (0,1)),
    PRIMARY KEY(workspace_id,subject_id,runtime_id,worker_id),
    FOREIGN KEY(workspace_id,subject_id) REFERENCES subject_deletions(workspace_id,subject_id) ON DELETE CASCADE
);
CREATE TABLE subject_deletion_removals (
    workspace_id TEXT NOT NULL,
    subject_id TEXT NOT NULL,
    runtime_id TEXT NOT NULL,
    worker_id TEXT NOT NULL,
    operation_id TEXT NOT NULL REFERENCES worker_removal_operations(operation_id),
    input_fingerprint TEXT NOT NULL,
    request_json TEXT NOT NULL CHECK(json_valid(request_json)),
    completed INTEGER NOT NULL DEFAULT 0 CHECK(completed IN (0,1)),
    job_purge_receipt_json TEXT CHECK(job_purge_receipt_json IS NULL OR json_valid(job_purge_receipt_json)),
    PRIMARY KEY(workspace_id,subject_id,runtime_id,worker_id),
    FOREIGN KEY(workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    FOREIGN KEY(workspace_id,subject_id,runtime_id,worker_id)
      REFERENCES subject_deletion_workers(workspace_id,subject_id,runtime_id,worker_id) ON DELETE CASCADE
);
CREATE TABLE subject_deletion_jobs (
    workspace_id TEXT NOT NULL,
    subject_id TEXT NOT NULL,
    job_id TEXT NOT NULL,
    PRIMARY KEY(workspace_id,job_id),
    FOREIGN KEY(workspace_id,subject_id) REFERENCES subject_deletions(workspace_id,subject_id) ON DELETE CASCADE
);
CREATE TABLE subject_deletion_allocations (
    workspace_id TEXT NOT NULL,
    subject_id TEXT NOT NULL,
    allocation_key TEXT NOT NULL,
    PRIMARY KEY(workspace_id,allocation_key),
    FOREIGN KEY(workspace_id,subject_id) REFERENCES subject_deletions(workspace_id,subject_id) ON DELETE CASCADE
);
CREATE TABLE plugin_installations (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    installation_id TEXT NOT NULL,
    alias TEXT NOT NULL CHECK(length(alias) BETWEEN 1 AND 128),
    package_id TEXT NOT NULL,
    last_operation TEXT NOT NULL,
    uninstalled INTEGER NOT NULL DEFAULT 0 CHECK(uninstalled IN (0,1)),
    PRIMARY KEY(workspace_id, installation_id)
);
CREATE TABLE plugin_artifacts (
    workspace_id TEXT NOT NULL,
    installation_id TEXT NOT NULL,
    artifact_digest TEXT NOT NULL CHECK(length(artifact_digest)=71 AND substr(artifact_digest,1,7)='sha256:' AND substr(artifact_digest,8) NOT GLOB '*[^0-9a-f]*'),
    package_digest TEXT NOT NULL CHECK(length(package_digest)=71 AND substr(package_digest,1,7)='sha256:' AND substr(package_digest,8) NOT GLOB '*[^0-9a-f]*'),
    version TEXT NOT NULL,
    artifact_size INTEGER NOT NULL CHECK(artifact_size > 0),
    metadata_json TEXT NOT NULL CHECK(json_valid(metadata_json)),
    revoked INTEGER NOT NULL DEFAULT 0 CHECK(revoked IN (0,1)),
    revocation_reason TEXT,
    CHECK((revoked=0 AND revocation_reason IS NULL) OR (revoked=1 AND revocation_reason IS NOT NULL)),
    PRIMARY KEY(workspace_id, installation_id, artifact_digest),
    UNIQUE(workspace_id, installation_id, version),
    FOREIGN KEY(workspace_id, installation_id) REFERENCES plugin_installations(workspace_id, installation_id) ON DELETE CASCADE
);
CREATE TABLE plugin_uploads (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    upload_id TEXT NOT NULL,
    actor TEXT NOT NULL,
    expires_at INTEGER NOT NULL,
    artifact_digest TEXT NOT NULL CHECK(length(artifact_digest)=71 AND substr(artifact_digest,1,7)='sha256:' AND substr(artifact_digest,8) NOT GLOB '*[^0-9a-f]*'),
    artifact_size INTEGER NOT NULL CHECK(artifact_size > 0),
    metadata_json TEXT NOT NULL CHECK(json_valid(metadata_json)),
    PRIMARY KEY(workspace_id, upload_id)
);
CREATE TABLE plugin_operations (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    operation_id TEXT NOT NULL,
    actor TEXT NOT NULL,
    kind TEXT NOT NULL CHECK(kind IN ('install','add_artifact','revoke','uninstall')),
    operation_key TEXT NOT NULL CHECK(length(operation_key) BETWEEN 1 AND 128),
    fingerprint TEXT NOT NULL CHECK(length(fingerprint)=64),
    result_json TEXT NOT NULL CHECK(json_valid(result_json)),
    PRIMARY KEY(workspace_id, operation_id),
    UNIQUE(workspace_id, actor, kind, operation_key)
);
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
CREATE TABLE worker_plugin_launches (
    workspace_id TEXT NOT NULL,
    worker_id TEXT NOT NULL,
    runtime_id TEXT NOT NULL,
    plan_id TEXT,
    PRIMARY KEY(workspace_id, worker_id),
    FOREIGN KEY(workspace_id, worker_id) REFERENCES worker_create_reservations(workspace_id, worker_id) ON DELETE CASCADE,
    FOREIGN KEY(workspace_id, plan_id) REFERENCES plugin_execution_plans(workspace_id, plan_id)
);
CREATE TABLE "plugin_references" (
    workspace_id TEXT NOT NULL,
    kind TEXT NOT NULL CHECK(kind IN ('config','pending','active','restorable','data')),
    owner TEXT NOT NULL CHECK(length(owner) BETWEEN 1 AND 512),
    installation_id TEXT NOT NULL,
    artifact_digest TEXT NOT NULL,
    PRIMARY KEY(workspace_id, kind, owner, installation_id, artifact_digest),
    FOREIGN KEY(workspace_id, installation_id, artifact_digest) REFERENCES plugin_artifacts(workspace_id, installation_id, artifact_digest) ON DELETE RESTRICT
);
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
CREATE TABLE plugin_data_purges (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    actor TEXT NOT NULL,
    operation_key TEXT NOT NULL CHECK(length(CAST(operation_key AS BLOB)) BETWEEN 1 AND 256),
    installation_id TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    result_json TEXT NOT NULL CHECK(json_valid(result_json)),
    PRIMARY KEY(workspace_id,actor,operation_key)
);
CREATE TABLE typed_ticket_service_outbox (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    event_id TEXT NOT NULL UNIQUE CHECK(length(event_id) = 32 AND event_id NOT GLOB '*[^0-9a-f]*'),
    workspace_id TEXT NOT NULL CHECK(length(CAST(workspace_id AS BLOB)) BETWEEN 1 AND 256),
    ticket_id TEXT NOT NULL CHECK(length(CAST(ticket_id AS BLOB)) BETWEEN 1 AND 256),
    event_type TEXT NOT NULL CHECK(event_type IN ('changed', 'deleted')),
    occurred_at TEXT NOT NULL CHECK(length(CAST(occurred_at AS BLOB)) BETWEEN 1 AND 64),
    domain_event_ref TEXT NOT NULL CHECK(length(CAST(domain_event_ref AS BLOB)) BETWEEN 1 AND 512),
    content_digest TEXT NOT NULL CHECK(length(content_digest) = 64 AND content_digest NOT GLOB '*[^0-9a-f]*'),
    correlation_id TEXT NOT NULL CHECK(length(CAST(correlation_id AS BLOB)) BETWEEN 1 AND 512),
    causation_id TEXT NOT NULL CHECK(length(CAST(causation_id AS BLOB)) BETWEEN 1 AND 512)
);
CREATE TABLE plugin_service_consumers (
 workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
 consumer_id TEXT NOT NULL,
 runtime_id TEXT NOT NULL,
 worker_id TEXT NOT NULL,
 installation_id TEXT NOT NULL,
 plan_id TEXT NOT NULL,
 service_name TEXT NOT NULL,
 subscriptions_json TEXT NOT NULL CHECK(json_valid(subscriptions_json) AND json_type(subscriptions_json)='array' AND json_array_length(subscriptions_json) BETWEEN 1 AND 2),
 cursor INTEGER NOT NULL CHECK(cursor>=0),
 -- Explicit recovery may drain the already observed backlog, one event at a time.
 -- New backlog beyond this watermark is subject to ordinary backpressure again.
 drain_through INTEGER NOT NULL DEFAULT 0 CHECK(drain_through>=0),
 lease_token TEXT,
 lease_until_ms INTEGER NOT NULL DEFAULT 0,
 pause_reason TEXT CHECK(pause_reason IN ('rejected','attempts_exhausted','queue_full','retention_gap','indeterminate','authorization')),
 current_sequence INTEGER,
 current_event_id TEXT,
 invocation_id TEXT,
 attempt INTEGER NOT NULL DEFAULT 0 CHECK(attempt BETWEEN 0 AND 5),
 next_attempt_ms INTEGER NOT NULL DEFAULT 0,
 last_ack_event_id TEXT,
 PRIMARY KEY(workspace_id,consumer_id),
 UNIQUE(workspace_id,runtime_id,worker_id,installation_id,plan_id,service_name),
 FOREIGN KEY(workspace_id,plan_id) REFERENCES plugin_execution_plans(workspace_id,plan_id),
 FOREIGN KEY(workspace_id,installation_id) REFERENCES plugin_installations(workspace_id,installation_id),
 CHECK((current_sequence IS NULL AND current_event_id IS NULL AND invocation_id IS NULL) OR (current_sequence IS NOT NULL AND current_event_id IS NOT NULL))
);
CREATE TABLE plugin_service_released_proofs (
 workspace_id TEXT NOT NULL,
 consumer_id TEXT NOT NULL,
 lease_token TEXT NOT NULL CHECK(length(CAST(lease_token AS BLOB)) BETWEEN 1 AND 512),
 released_at_ms INTEGER NOT NULL,
 PRIMARY KEY(workspace_id,consumer_id,lease_token),
 FOREIGN KEY(workspace_id,consumer_id) REFERENCES plugin_service_consumers(workspace_id,consumer_id) ON DELETE CASCADE
);
CREATE TABLE plugin_service_retention (
 workspace_id TEXT PRIMARY KEY REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
 removed_through INTEGER NOT NULL CHECK(removed_through>=0)
);
CREATE TABLE plugin_service_recovery_audit (
 workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
 operation_key TEXT NOT NULL CHECK(length(CAST(operation_key AS BLOB)) BETWEEN 1 AND 128),
 consumer_id TEXT NOT NULL,
 fingerprint TEXT NOT NULL,
 actor TEXT NOT NULL,
 reason TEXT NOT NULL CHECK(length(CAST(reason AS BLOB)) BETWEEN 1 AND 1024),
 action_json TEXT NOT NULL CHECK(json_valid(action_json)),
 at_ms INTEGER NOT NULL,
 PRIMARY KEY(workspace_id,operation_key),
 FOREIGN KEY(workspace_id,consumer_id) REFERENCES plugin_service_consumers(workspace_id,consumer_id)
);
CREATE TABLE typed_ticket_parent_links (
    workspace_id TEXT NOT NULL,
    ticket_id TEXT NOT NULL,
    parent_ticket_id TEXT,
    last_operation_key TEXT NOT NULL,
    PRIMARY KEY (workspace_id, ticket_id),
    FOREIGN KEY (workspace_id, ticket_id)
        REFERENCES typed_tickets(workspace_id, ticket_id) ON DELETE CASCADE,
    FOREIGN KEY (workspace_id, parent_ticket_id)
        REFERENCES typed_tickets(workspace_id, ticket_id),
    CHECK (parent_ticket_id IS NULL OR parent_ticket_id <> ticket_id),
    CHECK (length(last_operation_key) > 0)
);
CREATE TABLE typed_ticket_parent_operations (
    workspace_id TEXT NOT NULL,
    operation_key TEXT NOT NULL,
    ticket_id TEXT NOT NULL,
    input_json TEXT NOT NULL,
    before_json TEXT NOT NULL,
    result_json TEXT NOT NULL,
    source_context_json TEXT NOT NULL,
    at TEXT NOT NULL,
    PRIMARY KEY (workspace_id, operation_key),
    CHECK (length(operation_key) > 0)
);
CREATE TABLE content_objects (
    kind INTEGER NOT NULL CHECK(kind IN (1,2,3)),
    hash BLOB NOT NULL CHECK(length(hash)=32),
    bytes BLOB NOT NULL,
    PRIMARY KEY(kind,hash)
) WITHOUT ROWID;
CREATE TABLE content_scopes (
    workspace TEXT NOT NULL,
    target TEXT NOT NULL,
    head BLOB CHECK(head IS NULL OR length(head)=32),
    erased INTEGER NOT NULL DEFAULT 0 CHECK(erased IN (0,1)),
    PRIMARY KEY(workspace,target),
    CHECK(erased=0 OR head IS NULL)
) WITHOUT ROWID;
CREATE TABLE content_scope_objects (
    workspace TEXT NOT NULL,
    target TEXT NOT NULL,
    kind INTEGER NOT NULL,
    hash BLOB NOT NULL,
    PRIMARY KEY(workspace,target,kind,hash),
    FOREIGN KEY(workspace,target) REFERENCES content_scopes(workspace,target),
    FOREIGN KEY(kind,hash) REFERENCES content_objects(kind,hash)
) WITHOUT ROWID;
CREATE TABLE content_reachable (
    workspace TEXT NOT NULL,
    target TEXT NOT NULL,
    kind INTEGER NOT NULL,
    hash BLOB NOT NULL,
    PRIMARY KEY(workspace,target,kind,hash),
    FOREIGN KEY(workspace,target,kind,hash)
        REFERENCES content_scope_objects(workspace,target,kind,hash)
) WITHOUT ROWID;
CREATE TABLE workspace_configuration_authorities (
    workspace_id TEXT PRIMARY KEY REFERENCES workspaces(workspace_id) ON DELETE RESTRICT,
    transfer_identity TEXT NOT NULL,
    legacy_physical_identity TEXT NOT NULL,
    legacy_export_digest TEXT NOT NULL
);
CREATE TABLE workspace_configuration_policies (
    workspace_id TEXT NOT NULL REFERENCES workspace_configuration_authorities(workspace_id) ON DELETE RESTRICT,
    subject_id TEXT NOT NULL,
    policy_json TEXT NOT NULL,
    PRIMARY KEY(workspace_id, subject_id)
);
CREATE TABLE workspace_configuration_receipts (
    workspace_id TEXT NOT NULL REFERENCES workspace_configuration_authorities(workspace_id) ON DELETE RESTRICT,
    operation_id TEXT NOT NULL,
    request_fingerprint TEXT NOT NULL,
    result_json TEXT NOT NULL,
    PRIMARY KEY(workspace_id, operation_id)
);
CREATE TABLE backend_job_upgrade_history (
    workspace_id TEXT NOT NULL,
    job_id TEXT NOT NULL,
    state TEXT NOT NULL,
    failure_category TEXT,
    failure_detail TEXT,
    updated_at TEXT NOT NULL,
    completed_at TEXT,
    migrated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    PRIMARY KEY (workspace_id, job_id),
    FOREIGN KEY (workspace_id, job_id)
        REFERENCES backend_jobs(workspace_id, job_id) ON DELETE CASCADE
);
CREATE TABLE backend_job_attempt_upgrade_history (
    workspace_id TEXT NOT NULL,
    job_id TEXT NOT NULL,
    attempt_id TEXT NOT NULL,
    state TEXT NOT NULL,
    failure_category TEXT,
    failure_detail TEXT,
    updated_at TEXT NOT NULL,
    completed_at TEXT,
    migrated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    PRIMARY KEY (workspace_id, job_id, attempt_id),
    FOREIGN KEY (workspace_id, job_id, attempt_id)
        REFERENCES backend_job_attempts(workspace_id, job_id, attempt_id) ON DELETE CASCADE
);
CREATE TABLE backend_jobs (
            workspace_id TEXT NOT NULL,
            job_id TEXT NOT NULL,
            purpose TEXT NOT NULL,
            input_digest TEXT NOT NULL,
            input_ref TEXT NOT NULL,
            request_json TEXT NOT NULL,
            intent_fingerprint TEXT NOT NULL,
            state TEXT NOT NULL CHECK (state IN ('pending', 'completed', 'failed')),
            current_attempt INTEGER NOT NULL CHECK (current_attempt > 0 AND current_attempt <= 3),
            result_json TEXT,
            result_digest TEXT,
            failure_category TEXT,
            failure_detail TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            completed_at TEXT, resource_key TEXT, input_disposition TEXT NOT NULL DEFAULT 'available' CHECK (input_disposition IN ('available','reclaimed','erased','invalidated')),
            PRIMARY KEY (workspace_id, job_id),
            FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
            CHECK ((result_json IS NULL) = (result_digest IS NULL)),
            CHECK ((state = 'completed') = (result_json IS NOT NULL))
        );
CREATE TABLE backend_job_attempts (
            workspace_id TEXT NOT NULL,
            job_id TEXT NOT NULL,
            attempt_id TEXT NOT NULL,
            attempt INTEGER NOT NULL CHECK (attempt > 0 AND attempt <= 3),
            input_digest TEXT NOT NULL,
            state TEXT NOT NULL CHECK (state IN ('reserved', 'dispatching', 'dispatched', 'completed', 'failed')),
            runtime_id TEXT,
            worker_id TEXT,
            runtime_run_id TEXT,
            dispatched_at TEXT,
            deadline_at TEXT NOT NULL,
            result_json TEXT,
            result_digest TEXT,
            failure_category TEXT,
            failure_detail TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            completed_at TEXT, worker_cleanup_state TEXT
            CHECK (worker_cleanup_state IN ('pending', 'executing', 'completed', 'failed')), worker_cleanup_failure_category TEXT, worker_cleanup_failure_detail TEXT, worker_cleanup_updated_at TEXT, worker_cleanup_completed_at TEXT, allocation_precluded_at TEXT,
            PRIMARY KEY (workspace_id, job_id, attempt_id),
            UNIQUE (workspace_id, job_id, attempt),
            UNIQUE (workspace_id, runtime_id, worker_id),
            FOREIGN KEY (workspace_id, job_id)
                REFERENCES backend_jobs(workspace_id, job_id) ON DELETE CASCADE,
            CHECK ((runtime_id IS NULL) = (worker_id IS NULL)),
            CHECK ((result_json IS NULL) = (result_digest IS NULL)),
            CHECK ((state = 'completed') = (result_json IS NOT NULL))
        );
CREATE TABLE backend_job_reconciliation (
    workspace_id TEXT NOT NULL,
    job_id TEXT NOT NULL,
    attempt_id TEXT NOT NULL,
    cause TEXT NOT NULL CHECK(length(cause)>0),
    detail TEXT NOT NULL,
    next_action TEXT NOT NULL CHECK(length(next_action)>0),
    updated_at TEXT NOT NULL,
    PRIMARY KEY(workspace_id,job_id,attempt_id),
    FOREIGN KEY(workspace_id,job_id,attempt_id) REFERENCES backend_job_attempts(workspace_id,job_id,attempt_id) ON DELETE CASCADE
);
CREATE TABLE backend_job_execution_fences (
    workspace_id TEXT NOT NULL,
    job_id TEXT NOT NULL,
    attempt_id TEXT NOT NULL,
    owner_json TEXT NOT NULL CHECK(json_valid(owner_json)),
    history_json TEXT NOT NULL CHECK(json_valid(history_json)),
    controller_json TEXT NOT NULL CHECK(json_valid(controller_json)),
    host_json TEXT NOT NULL CHECK(json_valid(host_json)),
    recorded_at TEXT NOT NULL,
    PRIMARY KEY(workspace_id,job_id,attempt_id),
    FOREIGN KEY(workspace_id,job_id,attempt_id) REFERENCES backend_job_attempts(workspace_id,job_id,attempt_id) ON DELETE CASCADE
);
CREATE TABLE backend_job_recovery_probes (
    workspace_id TEXT NOT NULL,
    job_id TEXT NOT NULL,
    attempt_id TEXT NOT NULL,
    successful_writes INTEGER NOT NULL CHECK(successful_writes>0),
    PRIMARY KEY(workspace_id,job_id,attempt_id),
    FOREIGN KEY(workspace_id,job_id,attempt_id) REFERENCES backend_job_attempts(workspace_id,job_id,attempt_id) ON DELETE CASCADE
);
CREATE UNIQUE INDEX merge_request_merge_operations ON merge_request_thread_events(workspace_id,operation_id)WHERE operation_id IS NOT NULL;
CREATE INDEX worker_session_archive_observe_grants_controller
        ON worker_session_archive_observe_grants(
            workspace_id,controller_runtime_id,controller_worker_id,archive_id);
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
CREATE INDEX idx_typed_ticket_relations_workspace_target
    ON typed_ticket_relations(workspace_id, target, at DESC);
CREATE INDEX idx_workdir_registry_workspace_updated
    ON workdir_registry(workspace_id, updated_at DESC);
CREATE INDEX idx_worker_mutation_source_proof_jtis_expiry
            ON worker_mutation_source_proof_jtis(expires_at);
CREATE INDEX idx_workspace_config_entries_prefix
            ON workspace_config_entries(workspace_id, path);
CREATE INDEX idx_workspace_resource_keys_reverse
            ON workspace_resource_keys(workspace_id, resource_kind, resource_key);
CREATE INDEX repositories_workspace_provider_idx
            ON repositories(workspace_id, provider);
CREATE INDEX typed_ticket_events_workspace_kind_ticket
            ON typed_ticket_events(workspace_id, kind, ticket_id, event_index);
CREATE UNIQUE INDEX ux_worker_workdir_attachment_reservation_id
    ON worker_workdir_attachment_reservations(workspace_id, reservation_id);
CREATE UNIQUE INDEX workspace_config_grants_active_worker
    ON workspace_config_grants(workspace_id, runtime_id, worker_id) WHERE revoked = 0;
CREATE INDEX idx_typed_tickets_workspace_state_updated
                    ON typed_tickets(workspace_id, workflow_state, updated_at DESC, ticket_id);
CREATE INDEX idx_typed_tickets_workspace_updated
                    ON typed_tickets(workspace_id, updated_at DESC, ticket_id);
CREATE INDEX typed_ticket_targets_workspace_repository
                    ON typed_ticket_targets(workspace_id, repository_key, ticket_id);
CREATE INDEX idx_external_workdir_grants_status_expiry
                ON external_workdir_grants(workspace_id, status, expires_at);
CREATE INDEX backend_job_deliveries_pending
            ON backend_job_deliveries(workspace_id, state, updated_at);
CREATE INDEX worker_control_grants_controller ON worker_control_grants(
                workspace_id, controller_runtime_id, controller_worker_id, revoked_at
            );
CREATE INDEX worker_control_grants_subject ON worker_control_grants(
                workspace_id, subject_runtime_id, subject_worker_id, revoked_at
            );
CREATE UNIQUE INDEX worker_workdir_links_active_workdir_unique
                ON worker_workdir_links(workspace_id, workdir_id) WHERE unlinked_at IS NULL;
CREATE UNIQUE INDEX worker_workdir_links_active_alias_unique
                ON worker_workdir_links(workspace_id, runtime_id, worker_id, alias)
                WHERE unlinked_at IS NULL;
CREATE INDEX worker_workdir_links_workdir
                ON worker_workdir_links(workspace_id, workdir_id);
CREATE INDEX worker_registry_runtime
                ON worker_registry(workspace_id, runtime_id, worker_id);
CREATE UNIQUE INDEX worker_restore_intent_pending_domain ON worker_restore_intents(workspace_id,runtime_id,worker_id,domain_key) WHERE settled=0;
CREATE INDEX idx_ticket_assignment_operations_ticket
            ON ticket_assignment_operations(workspace_id, ticket_id, role, created_at DESC);
CREATE INDEX idx_ticket_worker_assignments_principal
            ON ticket_worker_assignments(workspace_id, role, principal_kind, principal_id, runtime_id, worker_id, assigned_at DESC);
CREATE INDEX idx_ticket_worker_assignments_ticket
            ON ticket_worker_assignments(workspace_id, ticket_id, role, assigned_at DESC);
CREATE INDEX ticket_current_principal_idx
            ON ticket_current_worker_assignments(workspace_id, principal_kind, principal_id, runtime_id, worker_id);
CREATE UNIQUE INDEX ticket_current_singleton_role_idx
            ON ticket_current_worker_assignments(workspace_id, ticket_id, role)
            WHERE role IN ('orchestrator', 'worker');
CREATE INDEX idx_ticket_worker_assignment_events_ticket
            ON ticket_worker_assignment_events(workspace_id, ticket_id, role, created_at DESC);
CREATE UNIQUE INDEX workspace_drive_grants_active_worker
    ON workspace_drive_grants(workspace_id, runtime_id, worker_id) WHERE revoked=0;
CREATE INDEX worker_create_reservations_worker ON worker_create_reservations(workspace_id, worker_id);
CREATE INDEX idx_workdir_create_credential_candidates_fingerprint
    ON workdir_create_credential_candidates(
        workspace_id, credential_id, credential_fingerprint
    );
CREATE INDEX idx_repository_secret_audit_workspace_created ON repository_secret_audit_events(workspace_id,created_at,event_id);
CREATE INDEX idx_repository_ssh_credentials_workspace_status ON repository_ssh_credentials(workspace_id,status,credential_id);
CREATE INDEX idx_repository_ssh_host_trusts_workspace_host ON repository_ssh_host_trusts(workspace_id,hostname,port);
CREATE INDEX idx_workspace_runtime_binding_audit_recent
    ON workspace_runtime_binding_audit(workspace_id, runtime_id, at DESC, binding_id DESC);
CREATE INDEX workspace_deletion_operations_workspace_recent
    ON workspace_deletion_operations(workspace_id, created_at DESC);
CREATE UNIQUE INDEX idx_workdir_removal_operations_one_pending
    ON workdir_removal_operations(workspace_id, workdir_id)
    WHERE state = 'pending';
CREATE INDEX idx_workdir_removal_operations_recovery
    ON workdir_removal_operations(workspace_id, state, retryable, updated_at);
CREATE INDEX idx_workdir_removal_operations_workdir
    ON workdir_removal_operations(workspace_id, workdir_id, created_at DESC);
CREATE INDEX worker_removal_operations_worker_idx ON worker_removal_operations(workspace_id,runtime_id,worker_id,created_at);
CREATE INDEX workspace_signing_identity_audit_workspace_idx
            ON workspace_signing_identity_audit(workspace_id, created_at DESC);
CREATE UNIQUE INDEX runtime_removal_operations_one_active_runtime
         ON runtime_removal_operations(runtime_id)
         WHERE state IN ('pending', 'cleanup_pending');
CREATE INDEX runtime_removal_operations_workspace_state
         ON runtime_removal_operations(workspace_id, state, updated_at);
CREATE INDEX idx_workspace_runtime_bindings_workspace
            ON workspace_runtime_bindings(workspace_id, revoked_at, runtime_id);
CREATE UNIQUE INDEX workspace_tags_active_name
    ON workspace_tags(workspace_id, normalized_name) WHERE deleted = 0;
CREATE INDEX typed_ticket_tags_workspace_tag
    ON typed_ticket_tags(workspace_id, tag_id, ticket_id);
CREATE VIEW ticket_active_worker_assignments AS
    SELECT current.* FROM ticket_current_worker_assignments AS current
    JOIN typed_tickets AS ticket
      ON ticket.workspace_id = current.workspace_id AND ticket.ticket_id = current.ticket_id
    WHERE ticket.workflow_state = 'open'
      AND NOT EXISTS (SELECT 1 FROM ticket_assignment_work_releases AS released
        WHERE released.workspace_id = current.workspace_id
          AND released.assignment_id = current.assignment_id);
CREATE INDEX typed_ticket_assignment_tracking_history_scope
ON typed_ticket_assignment_tracking_history(workspace_id,ticket_id,assignment_id);
CREATE UNIQUE INDEX typed_ticket_events_ref ON typed_ticket_events(workspace_id, ticket_id, event_ref);
CREATE INDEX notification_decisions_pending ON notification_decisions(workspace_id,disposition,last_offered,source_sequence);
CREATE INDEX notification_failures_delivery ON notification_failures(workspace_id,delivery_id,sequence);
CREATE INDEX notification_sources_assignment ON notification_sources(workspace_id,ticket_id,assignment_id,role,sequence);
CREATE INDEX notification_sources_unrouted ON notification_sources(workspace_id,sequence);
CREATE TRIGGER runtime_binding_insert_blocked_by_removal
         BEFORE INSERT ON workspace_runtime_bindings
         FOR EACH ROW
         WHEN EXISTS (
             SELECT 1 FROM runtime_removal_operations operation
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
             SELECT 1 FROM runtime_removal_operations operation
             WHERE operation.runtime_id = NEW.runtime_id
               AND operation.state IN ('pending', 'cleanup_pending')
         )
         BEGIN
             SELECT RAISE(ABORT, 'runtime_removal_in_progress');
         END;
CREATE TRIGGER workdir_registry_insert_blocked_by_runtime_removal
        BEFORE INSERT ON workdir_registry FOR EACH ROW
        WHEN EXISTS (SELECT 1 FROM runtime_removal_operations operation WHERE operation.runtime_id = NEW.runtime_id AND operation.state IN ('pending', 'cleanup_pending'))
        BEGIN SELECT RAISE(ABORT, 'runtime_removal_in_progress'); END;
CREATE TRIGGER workdir_registry_update_blocked_by_runtime_removal
        BEFORE UPDATE ON workdir_registry FOR EACH ROW
        WHEN EXISTS (SELECT 1 FROM runtime_removal_operations operation WHERE operation.runtime_id = NEW.runtime_id AND operation.state IN ('pending', 'cleanup_pending'))
        BEGIN SELECT RAISE(ABORT, 'runtime_removal_in_progress'); END;
CREATE TRIGGER worker_create_insert_blocked_by_runtime_removal
        BEFORE INSERT ON worker_create_reservations FOR EACH ROW
        WHEN EXISTS (SELECT 1 FROM runtime_removal_operations operation WHERE operation.runtime_id = NEW.runtime_id AND operation.state IN ('pending', 'cleanup_pending'))
        BEGIN SELECT RAISE(ABORT, 'runtime_removal_in_progress'); END;
CREATE TRIGGER worker_create_update_blocked_by_runtime_removal
        BEFORE UPDATE ON worker_create_reservations FOR EACH ROW
        WHEN EXISTS (SELECT 1 FROM runtime_removal_operations operation WHERE operation.runtime_id = NEW.runtime_id AND operation.state IN ('pending', 'cleanup_pending'))
        BEGIN SELECT RAISE(ABORT, 'runtime_removal_in_progress'); END;
CREATE TRIGGER worker_removal_insert_blocked_by_runtime_removal
        BEFORE INSERT ON worker_removal_operations FOR EACH ROW
        WHEN EXISTS (SELECT 1 FROM runtime_removal_operations operation WHERE operation.runtime_id = NEW.runtime_id AND operation.state IN ('pending', 'cleanup_pending'))
        BEGIN SELECT RAISE(ABORT, 'runtime_removal_in_progress'); END;
CREATE TRIGGER worker_removal_update_blocked_by_runtime_removal
        BEFORE UPDATE ON worker_removal_operations FOR EACH ROW
        WHEN EXISTS (SELECT 1 FROM runtime_removal_operations operation WHERE operation.runtime_id = NEW.runtime_id AND operation.state IN ('pending', 'cleanup_pending'))
        BEGIN SELECT RAISE(ABORT, 'runtime_removal_in_progress'); END;
CREATE TRIGGER workdir_removal_insert_blocked_by_runtime_removal
        BEFORE INSERT ON workdir_removal_operations FOR EACH ROW
        WHEN EXISTS (SELECT 1 FROM runtime_removal_operations operation WHERE operation.runtime_id = NEW.runtime_id AND operation.state IN ('pending', 'cleanup_pending'))
        BEGIN SELECT RAISE(ABORT, 'runtime_removal_in_progress'); END;
CREATE TRIGGER workdir_removal_update_blocked_by_runtime_removal
        BEFORE UPDATE ON workdir_removal_operations FOR EACH ROW
        WHEN EXISTS (SELECT 1 FROM runtime_removal_operations operation WHERE operation.runtime_id = NEW.runtime_id AND operation.state IN ('pending', 'cleanup_pending'))
        BEGIN SELECT RAISE(ABORT, 'runtime_removal_in_progress'); END;
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
CREATE TRIGGER ticket_assignment_operations_validate_insert
        BEFORE INSERT ON ticket_assignment_operations
        WHEN NOT EXISTS (
            SELECT 1 FROM typed_tickets AS ticket
            WHERE ticket.workspace_id = NEW.workspace_id AND ticket.ticket_id = NEW.ticket_id
        )
        BEGIN
            SELECT RAISE(ABORT, 'Ticket assignment operation must reference a Ticket in the same Workspace');
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
CREATE TRIGGER worker_assignment_insert_blocked_by_runtime_removal
        BEFORE INSERT ON ticket_current_worker_assignments FOR EACH ROW
        WHEN EXISTS (SELECT 1 FROM runtime_removal_operations operation WHERE operation.runtime_id = NEW.runtime_id AND operation.state IN ('pending', 'cleanup_pending'))
        BEGIN SELECT RAISE(ABORT, 'runtime_removal_in_progress'); END;
CREATE TRIGGER worker_assignment_update_blocked_by_runtime_removal
        BEFORE UPDATE ON ticket_current_worker_assignments FOR EACH ROW
        WHEN EXISTS (SELECT 1 FROM runtime_removal_operations operation WHERE operation.runtime_id = NEW.runtime_id AND operation.state IN ('pending', 'cleanup_pending'))
        BEGIN SELECT RAISE(ABORT, 'runtime_removal_in_progress'); END;
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
CREATE TRIGGER workspace_drive_grants_worker_deleted BEFORE DELETE ON worker_registry BEGIN
    UPDATE workspace_drive_grants SET revoked=1, revoked_by='server:worker-removal',
        revoked_at=strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
    WHERE workspace_id=OLD.workspace_id AND runtime_id=OLD.runtime_id
      AND worker_id=OLD.worker_id AND revoked=0;
END;
CREATE TRIGGER backend_job_pre_content_capture_boundary
BEFORE INSERT ON backend_job_pre_content_history
WHEN (SELECT max(version) FROM __yoi_schema_migrations) IS NOT 85
BEGIN SELECT RAISE(ABORT,'pre-content evidence requires the schema-85 transaction'); END;
CREATE TRIGGER backend_job_attempt_pre_content_capture_boundary
BEFORE INSERT ON backend_job_attempt_pre_content_history
WHEN (SELECT max(version) FROM __yoi_schema_migrations) IS NOT 85
BEGIN SELECT RAISE(ABORT,'pre-content evidence requires the schema-85 transaction'); END;
CREATE TRIGGER backend_job_delivery_pre_content_capture_boundary
BEFORE INSERT ON backend_job_delivery_pre_content_history
WHEN (SELECT max(version) FROM __yoi_schema_migrations) IS NOT 85
BEGIN SELECT RAISE(ABORT,'pre-content evidence requires the schema-85 transaction'); END;
CREATE TRIGGER backend_job_pre_content_identity_no_update
BEFORE UPDATE OF workspace_id,job_id,purpose,input_revision,input_ref,resource_key,intent_fingerprint,current_attempt,state,result_digest,failure_category,failure_detail,updated_at,completed_at,captured_at ON backend_job_pre_content_history
BEGIN SELECT RAISE(ABORT,'immutable pre-content Job evidence'); END;
CREATE TRIGGER backend_job_pre_content_request_retirement
BEFORE UPDATE OF request_json,input_retired ON backend_job_pre_content_history
WHEN (
    OLD.input_retired=0 AND NEW.input_retired=1
    AND json_type(NEW.request_json,'$.input')='null'
    AND json_remove(NEW.request_json,'$.input')=json_remove(OLD.request_json,'$.input')
    AND EXISTS(SELECT 1 FROM backend_jobs j WHERE j.workspace_id=OLD.workspace_id AND j.job_id=OLD.job_id
        AND COALESCE(json_type(j.request_json,'$.input'),'null')='null')
) IS NOT TRUE
BEGIN SELECT RAISE(ABORT,'pre-content Job body retires only with its owner'); END;
CREATE TRIGGER backend_job_attempt_pre_content_no_update
BEFORE UPDATE ON backend_job_attempt_pre_content_history
BEGIN SELECT RAISE(ABORT,'immutable pre-content attempt evidence'); END;
CREATE TRIGGER backend_job_delivery_pre_content_no_update
BEFORE UPDATE ON backend_job_delivery_pre_content_history
BEGIN SELECT RAISE(ABORT,'immutable pre-content delivery evidence'); END;
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
CREATE TRIGGER seed_worker_retention_policy_after_workspace_insert AFTER INSERT ON workspaces BEGIN
        INSERT INTO workspace_worker_retention_policy_snapshots
          (workspace_id,policy_id,policy_digest,session_disposition,metadata_disposition,archive_retention_kind,archive_retention_seconds,diagnostics_disposition,diagnostics_retention_seconds,created_at)
          VALUES(NEW.workspace_id,'workspace-default-conservative','sha256:30d45d5d58a10dadf95c3bf831e589ea10c81dc76da9575bfd9fb13942b3d198','archive','tombstone','forever',NULL,'purge',NULL,NEW.created_at);
        INSERT INTO workspace_worker_retention_policies(workspace_id,policy_id,policy_digest,updated_at)
          VALUES(NEW.workspace_id,'workspace-default-conservative','sha256:30d45d5d58a10dadf95c3bf831e589ea10c81dc76da9575bfd9fb13942b3d198',NEW.created_at);
      END;
CREATE TRIGGER typed_ticket_case_history_no_update BEFORE UPDATE ON typed_ticket_case_history
BEGIN SELECT RAISE(ABORT, 'Ticket case history is immutable'); END;
CREATE TRIGGER typed_ticket_case_history_no_delete BEFORE DELETE ON typed_ticket_case_history
BEGIN SELECT RAISE(ABORT, 'Ticket case history is immutable'); END;
CREATE TRIGGER typed_ticket_legacy_workflow_no_update BEFORE UPDATE ON typed_ticket_legacy_workflow_snapshot
BEGIN SELECT RAISE(ABORT, 'Ticket legacy workflow snapshot is immutable'); END;
CREATE TRIGGER typed_ticket_legacy_workflow_no_delete BEFORE DELETE ON typed_ticket_legacy_workflow_snapshot
BEGIN SELECT RAISE(ABORT, 'Ticket legacy workflow snapshot is immutable'); END;
CREATE TRIGGER typed_ticket_case_state_insert BEFORE INSERT ON typed_tickets
WHEN NEW.workflow_state NOT IN ('open', 'closed')
BEGIN SELECT RAISE(ABORT, 'Ticket case state must be open or closed'); END;
CREATE TRIGGER typed_ticket_case_state_update BEFORE UPDATE OF workflow_state ON typed_tickets
WHEN NEW.workflow_state NOT IN ('open', 'closed')
BEGIN SELECT RAISE(ABORT, 'Ticket case state must be open or closed'); END;
CREATE TRIGGER ticket_terminal_releases_work
AFTER UPDATE OF workflow_state ON typed_tickets
WHEN NEW.workflow_state = 'closed'
BEGIN
    INSERT OR IGNORE INTO ticket_assignment_work_releases
        SELECT workspace_id, assignment_id, COALESCE(NEW.updated_at, updated_at)
        FROM ticket_current_worker_assignments
        WHERE workspace_id = NEW.workspace_id AND ticket_id = NEW.ticket_id;
END;
CREATE TRIGGER ticket_terminal_assignment_releases_work_insert
AFTER INSERT ON ticket_current_worker_assignments
WHEN EXISTS (SELECT 1 FROM typed_tickets WHERE workspace_id = NEW.workspace_id
    AND ticket_id = NEW.ticket_id AND workflow_state = 'closed')
BEGIN
    INSERT OR IGNORE INTO ticket_assignment_work_releases
        VALUES (NEW.workspace_id, NEW.assignment_id, NEW.updated_at);
END;
CREATE TRIGGER ticket_terminal_assignment_releases_work_update
AFTER UPDATE ON ticket_current_worker_assignments
WHEN EXISTS (SELECT 1 FROM typed_tickets WHERE workspace_id = NEW.workspace_id
    AND ticket_id = NEW.ticket_id AND workflow_state = 'closed')
BEGIN
    INSERT OR IGNORE INTO ticket_assignment_work_releases
        VALUES (NEW.workspace_id, NEW.assignment_id, NEW.updated_at);
END;
CREATE TRIGGER ticket_active_worker_role_insert
BEFORE INSERT ON ticket_current_worker_assignments
WHEN NEW.principal_kind = 'worker'
 AND EXISTS (SELECT 1 FROM typed_tickets WHERE workspace_id = NEW.workspace_id
    AND ticket_id = NEW.ticket_id AND workflow_state = 'open')
 AND NOT EXISTS (SELECT 1 FROM ticket_assignment_work_releases
    WHERE workspace_id = NEW.workspace_id AND assignment_id = NEW.assignment_id)
 AND EXISTS (SELECT 1 FROM ticket_active_worker_assignments
    WHERE workspace_id = NEW.workspace_id AND role = NEW.role
      AND runtime_id = NEW.runtime_id AND worker_id = NEW.worker_id
      AND assignment_id != NEW.assignment_id)
BEGIN SELECT RAISE(ABORT, 'Worker already has unfinished Ticket work in this role'); END;
CREATE TRIGGER ticket_active_worker_role_update
BEFORE UPDATE ON ticket_current_worker_assignments
WHEN NEW.principal_kind = 'worker'
 AND EXISTS (SELECT 1 FROM typed_tickets WHERE workspace_id = NEW.workspace_id
    AND ticket_id = NEW.ticket_id AND workflow_state = 'open')
 AND NOT EXISTS (SELECT 1 FROM ticket_assignment_work_releases
    WHERE workspace_id = NEW.workspace_id AND assignment_id = NEW.assignment_id)
 AND EXISTS (SELECT 1 FROM ticket_active_worker_assignments
    WHERE workspace_id = NEW.workspace_id AND role = NEW.role
      AND runtime_id = NEW.runtime_id AND worker_id = NEW.worker_id
      AND assignment_id != OLD.assignment_id)
BEGIN SELECT RAISE(ABORT, 'Worker already has unfinished Ticket work in this role'); END;
CREATE TRIGGER ticket_active_worker_principal_insert
BEFORE INSERT ON ticket_current_worker_assignments
WHEN NEW.principal_kind = 'worker' AND (
 NOT EXISTS (SELECT 1 FROM worker_registry WHERE workspace_id = NEW.workspace_id
    AND runtime_id = NEW.runtime_id AND worker_id = NEW.worker_id)
 OR EXISTS (SELECT 1 FROM worker_removal_operations WHERE workspace_id = NEW.workspace_id
    AND runtime_id = NEW.runtime_id AND worker_id = NEW.worker_id
    AND state IN ('executing', 'failed', 'succeeded')))
BEGIN SELECT RAISE(ABORT, 'Worker is missing or removal is reserved'); END;
CREATE TRIGGER ticket_active_worker_principal_update
BEFORE UPDATE ON ticket_current_worker_assignments
WHEN NEW.principal_kind = 'worker' AND (
 NOT EXISTS (SELECT 1 FROM worker_registry WHERE workspace_id = NEW.workspace_id
    AND runtime_id = NEW.runtime_id AND worker_id = NEW.worker_id)
 OR EXISTS (SELECT 1 FROM worker_removal_operations WHERE workspace_id = NEW.workspace_id
    AND runtime_id = NEW.runtime_id AND worker_id = NEW.worker_id
    AND state IN ('executing', 'failed', 'succeeded')))
BEGIN SELECT RAISE(ABORT, 'Worker is missing or removal is reserved'); END;
CREATE TRIGGER ticket_unfinished_work_blocks_worker_delete
BEFORE DELETE ON worker_registry
WHEN EXISTS (SELECT 1 FROM ticket_active_worker_assignments
    WHERE workspace_id = OLD.workspace_id AND runtime_id = OLD.runtime_id
      AND worker_id = OLD.worker_id)
BEGIN SELECT RAISE(ABORT, 'Worker has unfinished Ticket work'); END;
CREATE TRIGGER ticket_unfinished_work_blocks_worker_move
BEFORE UPDATE OF runtime_id, worker_id, workspace_id ON worker_registry
WHEN (OLD.runtime_id != NEW.runtime_id OR OLD.worker_id != NEW.worker_id OR OLD.workspace_id != NEW.workspace_id)
 AND EXISTS (SELECT 1 FROM ticket_active_worker_assignments
    WHERE workspace_id = OLD.workspace_id AND runtime_id = OLD.runtime_id
      AND worker_id = OLD.worker_id)
BEGIN SELECT RAISE(ABORT, 'Worker has unfinished Ticket work'); END;
CREATE TRIGGER typed_ticket_assignment_tracking_initialize
AFTER INSERT ON ticket_worker_assignments WHEN NEW.role = 'orchestrator'
BEGIN
    UPDATE ticket_worker_assignments SET tracking_json = '{"status":"waiting","waiting_reason":"Initial judgment pending","progress":null,"next_action":null,"result":null}', tracking_updated_at = NEW.assigned_at
    WHERE workspace_id = NEW.workspace_id AND assignment_id = NEW.assignment_id;
END;
CREATE TRIGGER typed_ticket_assignment_tracking_assigned
AFTER INSERT ON ticket_worker_assignment_events
WHEN NEW.role = 'orchestrator' AND NEW.action IN ('assigned', 'reassigned')
BEGIN
    UPDATE ticket_worker_assignments SET last_tracking_event_id = NEW.event_id, tracking_updated_at = NEW.created_at
    WHERE workspace_id = NEW.workspace_id AND assignment_id = NEW.assignment_id AND tracking_json IS NOT NULL;
    INSERT INTO typed_ticket_assignment_tracking_history
    SELECT workspace_id, ticket_id, assignment_id, NEW.event_id, COALESCE(NEW.operation_id, NEW.event_id), tracking_json,
        json_object('actor', NEW.actor, 'kind', 'assignment'), NEW.created_at
    FROM ticket_worker_assignments
    WHERE workspace_id = NEW.workspace_id AND assignment_id = NEW.assignment_id AND tracking_json IS NOT NULL;
END;
CREATE TRIGGER typed_ticket_assignment_tracking_history_no_update
BEFORE UPDATE ON typed_ticket_assignment_tracking_history
BEGIN SELECT RAISE(ABORT, 'Ticket assignment tracking history is immutable'); END;
CREATE TRIGGER typed_ticket_assignment_tracking_history_no_delete
BEFORE DELETE ON typed_ticket_assignment_tracking_history
BEGIN SELECT RAISE(ABORT, 'Ticket assignment tracking history is immutable'); END;
CREATE TRIGGER typed_ticket_assignment_tracking_operations_no_update
BEFORE UPDATE ON typed_ticket_assignment_tracking_operations
BEGIN SELECT RAISE(ABORT, 'Ticket assignment tracking receipts are immutable'); END;
CREATE TRIGGER typed_ticket_assignment_tracking_operations_no_delete
BEFORE DELETE ON typed_ticket_assignment_tracking_operations
BEGIN SELECT RAISE(ABORT, 'Ticket assignment tracking receipts are immutable'); END;
CREATE TRIGGER typed_ticket_assignment_identity_no_update
BEFORE UPDATE OF workspace_id,ticket_id,assignment_id,role,principal_kind,principal_id,runtime_id,worker_id,assigned_by,assigned_at ON ticket_worker_assignments
WHEN OLD.workspace_id IS NOT NEW.workspace_id OR OLD.ticket_id IS NOT NEW.ticket_id
 OR OLD.assignment_id IS NOT NEW.assignment_id OR OLD.role IS NOT NEW.role
 OR OLD.principal_kind IS NOT NEW.principal_kind OR OLD.principal_id IS NOT NEW.principal_id
 OR OLD.runtime_id IS NOT NEW.runtime_id OR OLD.worker_id IS NOT NEW.worker_id
 OR OLD.assigned_by IS NOT NEW.assigned_by OR OLD.assigned_at IS NOT NEW.assigned_at
BEGIN SELECT RAISE(ABORT, 'Ticket assignment identity is immutable'); END;
CREATE TRIGGER typed_ticket_assignment_identity_no_delete BEFORE DELETE ON ticket_worker_assignments
BEGIN SELECT RAISE(ABORT, 'Ticket assignment identity is immutable'); END;
CREATE TRIGGER typed_ticket_assignment_events_no_update BEFORE UPDATE ON ticket_worker_assignment_events
BEGIN SELECT RAISE(ABORT, 'Ticket assignment audit is immutable'); END;
CREATE TRIGGER typed_ticket_assignment_events_no_delete BEFORE DELETE ON ticket_worker_assignment_events
BEGIN SELECT RAISE(ABORT, 'Ticket assignment audit is immutable'); END;
CREATE TRIGGER typed_ticket_assignment_receipt_no_update BEFORE UPDATE ON ticket_assignment_operations
WHEN OLD.claim_state != 'pending'
BEGIN SELECT RAISE(ABORT, 'Ticket assignment terminal receipt is immutable'); END;
CREATE TRIGGER typed_ticket_assignment_receipt_no_delete BEFORE DELETE ON ticket_assignment_operations
WHEN OLD.claim_state != 'pending'
BEGIN SELECT RAISE(ABORT, 'Ticket assignment terminal receipt is immutable'); END;
CREATE TRIGGER typed_ticket_assignment_current_principal_insert BEFORE INSERT ON ticket_current_worker_assignments
WHEN NOT EXISTS (SELECT 1 FROM ticket_worker_assignments a WHERE a.workspace_id=NEW.workspace_id
 AND a.ticket_id=NEW.ticket_id AND a.assignment_id=NEW.assignment_id AND a.role=NEW.role
 AND a.principal_kind=NEW.principal_kind AND a.principal_id IS NEW.principal_id
 AND a.runtime_id IS NEW.runtime_id AND a.worker_id IS NEW.worker_id)
 OR EXISTS (SELECT 1 FROM ticket_worker_assignment_events e WHERE e.workspace_id=NEW.workspace_id
 AND ((e.action='unassigned' AND (e.assignment_id=NEW.assignment_id OR e.previous_assignment_id=NEW.assignment_id))
 OR (e.action='reassigned' AND e.previous_assignment_id=NEW.assignment_id)))
BEGIN SELECT RAISE(ABORT, 'Ticket current assignment must match an unreleased immutable identity'); END;
CREATE TRIGGER typed_ticket_assignment_current_principal_update BEFORE UPDATE ON ticket_current_worker_assignments
WHEN NOT EXISTS (SELECT 1 FROM ticket_worker_assignments a WHERE a.workspace_id=NEW.workspace_id
 AND a.ticket_id=NEW.ticket_id AND a.assignment_id=NEW.assignment_id AND a.role=NEW.role
 AND a.principal_kind=NEW.principal_kind AND a.principal_id IS NEW.principal_id
 AND a.runtime_id IS NEW.runtime_id AND a.worker_id IS NEW.worker_id)
 OR EXISTS (SELECT 1 FROM ticket_worker_assignment_events e WHERE e.workspace_id=NEW.workspace_id
 AND ((e.action='unassigned' AND (e.assignment_id=NEW.assignment_id OR e.previous_assignment_id=NEW.assignment_id))
 OR (e.action='reassigned' AND e.previous_assignment_id=NEW.assignment_id)))
BEGIN SELECT RAISE(ABORT, 'Ticket current assignment must match an unreleased immutable identity'); END;
CREATE TRIGGER typed_ticket_assignment_released_tracking_no_update
BEFORE UPDATE OF tracking_json,last_tracking_event_id,tracking_updated_at ON ticket_worker_assignments
WHEN EXISTS (SELECT 1 FROM ticket_worker_assignment_events e WHERE e.workspace_id=OLD.workspace_id
 AND ((e.action='unassigned' AND (e.assignment_id=OLD.assignment_id OR e.previous_assignment_id=OLD.assignment_id))
 OR (e.action='reassigned' AND e.previous_assignment_id=OLD.assignment_id)))
BEGIN SELECT RAISE(ABORT, 'Released assignment tracking is immutable'); END;
CREATE TRIGGER typed_ticket_assignment_parent_insert BEFORE INSERT ON ticket_worker_assignments
WHEN NOT EXISTS (SELECT 1 FROM typed_tickets t WHERE t.workspace_id=NEW.workspace_id AND t.ticket_id=NEW.ticket_id)
BEGIN SELECT RAISE(ABORT, 'New assignment requires a Ticket in the same Workspace'); END;
CREATE TRIGGER typed_ticket_assignment_audit_parent_insert BEFORE INSERT ON ticket_worker_assignment_events
WHEN (NEW.assignment_id IS NOT NULL AND NOT EXISTS (SELECT 1 FROM ticket_worker_assignments a WHERE a.workspace_id=NEW.workspace_id AND a.ticket_id=NEW.ticket_id AND a.role=NEW.role AND a.assignment_id=NEW.assignment_id))
 OR (NEW.previous_assignment_id IS NOT NULL AND NOT EXISTS (SELECT 1 FROM ticket_worker_assignments a WHERE a.workspace_id=NEW.workspace_id AND a.ticket_id=NEW.ticket_id AND a.role=NEW.role AND a.assignment_id=NEW.previous_assignment_id))
BEGIN SELECT RAISE(ABORT, 'Assignment audit must match the same immutable Ticket role'); END;
CREATE TRIGGER typed_ticket_assignment_operation_parent_insert BEFORE INSERT ON ticket_assignment_operations
WHEN NOT EXISTS (SELECT 1 FROM typed_tickets t WHERE t.workspace_id=NEW.workspace_id AND t.ticket_id=NEW.ticket_id)
BEGIN SELECT RAISE(ABORT, 'New assignment operation requires a Ticket in the same Workspace'); END;
CREATE TRIGGER ticket_assignment_delivery_request_no_update BEFORE UPDATE ON ticket_assignment_delivery_requests
BEGIN SELECT RAISE(ABORT,'Assignment delivery request is immutable'); END;
CREATE TRIGGER ticket_assignment_delivery_request_no_delete BEFORE DELETE ON ticket_assignment_delivery_requests
BEGIN SELECT RAISE(ABORT,'Assignment delivery request is immutable'); END;
CREATE TRIGGER ticket_assignment_delivery_origin_insert
AFTER INSERT ON typed_ticket_assignment_tracking_history
WHEN json_extract(NEW.source_context_json,'$.kind')='assignment'
BEGIN
 INSERT INTO ticket_assignment_delivery_origins
 VALUES(NEW.workspace_id,NEW.ticket_id,NEW.assignment_id,'assignment-event:' || NEW.event_id,
 COALESCE((SELECT MAX(event_index) FROM typed_ticket_events
 WHERE workspace_id=NEW.workspace_id AND ticket_id=NEW.ticket_id),-1));
END;
CREATE TRIGGER ticket_assignment_delivery_payload_no_update BEFORE UPDATE ON ticket_assignment_deliveries
BEGIN SELECT RAISE(ABORT,'Assignment delivery payload is immutable'); END;
CREATE TRIGGER ticket_assignment_delivery_payload_no_delete BEFORE DELETE ON ticket_assignment_deliveries
BEGIN SELECT RAISE(ABORT,'Assignment delivery payload is immutable'); END;
CREATE TRIGGER ticket_assignment_delivery_ack_no_update BEFORE UPDATE ON ticket_assignment_delivery_acknowledgements
BEGIN SELECT RAISE(ABORT,'Assignment delivery acknowledgement is immutable'); END;
CREATE TRIGGER ticket_assignment_delivery_ack_no_delete BEFORE DELETE ON ticket_assignment_delivery_acknowledgements
BEGIN SELECT RAISE(ABORT,'Assignment delivery acknowledgement is immutable'); END;
CREATE TRIGGER ticket_assignment_operation_fence_no_update BEFORE UPDATE ON ticket_assignment_operation_fences
BEGIN SELECT RAISE(ABORT,'Assignment startup fence is immutable'); END;
CREATE TRIGGER ticket_assignment_operation_fence_no_delete BEFORE DELETE ON ticket_assignment_operation_fences
BEGIN SELECT RAISE(ABORT,'Assignment startup fence is immutable'); END;
CREATE TRIGGER ticket_assignment_delivery_origin_no_update BEFORE UPDATE ON ticket_assignment_delivery_origins
BEGIN SELECT RAISE(ABORT,'Assignment input origin is immutable'); END;
CREATE TRIGGER ticket_assignment_delivery_origin_no_delete BEFORE DELETE ON ticket_assignment_delivery_origins
BEGIN SELECT RAISE(ABORT,'Assignment input origin is immutable'); END;
CREATE TRIGGER ticket_assignment_content_input_no_update
BEFORE UPDATE ON ticket_assignment_content_inputs
BEGIN SELECT RAISE(ABORT,'Assignment content input is immutable'); END;
CREATE TRIGGER ticket_assignment_content_input_no_delete
BEFORE DELETE ON ticket_assignment_content_inputs
BEGIN SELECT RAISE(ABORT,'Assignment content input is immutable'); END;
CREATE TRIGGER typed_ticket_content_changes_no_update BEFORE UPDATE ON typed_ticket_content_changes
BEGIN SELECT RAISE(ABORT, 'Ticket content change facts are immutable'); END;
CREATE TRIGGER ticket_assignment_title_input AFTER INSERT ON typed_ticket_content_changes
WHEN NEW.title_changed=1 BEGIN
 INSERT INTO ticket_assignment_content_inputs(workspace_id,ticket_id,assignment_id,after_ticket_event_index,actor,kind,source_change_id)
 SELECT NEW.workspace_id,NEW.ticket_id,c.assignment_id,
 COALESCE((SELECT MAX(event_index) FROM typed_ticket_events WHERE workspace_id=NEW.workspace_id AND ticket_id=NEW.ticket_id),-1),NEW.actor,NEW.kind,NEW.change_id
 FROM ticket_current_worker_assignments c WHERE c.workspace_id=NEW.workspace_id AND c.ticket_id=NEW.ticket_id;
END;
CREATE TRIGGER ticket_assignment_initial_post_input AFTER INSERT ON typed_ticket_content_changes
WHEN NEW.title_changed=0 BEGIN
 INSERT INTO ticket_assignment_content_inputs(workspace_id,ticket_id,assignment_id,after_ticket_event_index,actor,kind,source_change_id)
 SELECT NEW.workspace_id,NEW.ticket_id,c.assignment_id,
 COALESCE((SELECT MAX(event_index) FROM typed_ticket_events WHERE workspace_id=NEW.workspace_id AND ticket_id=NEW.ticket_id),-1),NEW.actor,NEW.kind,NEW.change_id
 FROM ticket_current_worker_assignments c WHERE c.workspace_id=NEW.workspace_id AND c.ticket_id=NEW.ticket_id;
END;
CREATE TRIGGER notification_sources_no_update BEFORE update ON notification_sources BEGIN SELECT RAISE(ABORT,'Notification evidence is immutable'); END;
CREATE TRIGGER notification_assignment_targets_no_update BEFORE update ON notification_assignment_targets BEGIN SELECT RAISE(ABORT,'Notification evidence is immutable'); END;
CREATE TRIGGER notification_assignment_targets_no_delete BEFORE delete ON notification_assignment_targets BEGIN SELECT RAISE(ABORT,'Notification evidence is immutable'); END;
CREATE TRIGGER notification_requests_no_update BEFORE update ON notification_requests BEGIN SELECT RAISE(ABORT,'Notification evidence is immutable'); END;
CREATE TRIGGER notification_acknowledgements_no_update BEFORE update ON notification_acknowledgements BEGIN SELECT RAISE(ABORT,'Notification evidence is immutable'); END;
CREATE TRIGGER notification_failures_no_update BEFORE update ON notification_failures BEGIN SELECT RAISE(ABORT,'Notification evidence is immutable'); END;
CREATE TRIGGER notification_decisions_identity_no_update BEFORE UPDATE OF source_sequence,workspace_id,delivery_id,disposition,reason,purpose ON notification_decisions BEGIN SELECT RAISE(ABORT,'Notification decision is immutable'); END;
CREATE TRIGGER notification_job_source_guard BEFORE INSERT ON notification_sources WHEN NEW.role='backend_job' BEGIN
        SELECT CASE WHEN NOT EXISTS(SELECT 1 FROM backend_jobs j JOIN backend_job_attempts a USING(workspace_id,job_id)
        WHERE j.workspace_id=NEW.workspace_id AND j.job_id=json_extract(NEW.facts_json,'$.backend_job.job_id') AND a.attempt_id=json_extract(NEW.facts_json,'$.backend_job.attempt_id')
        AND j.state='completed' AND a.state='completed' AND a.attempt=j.current_attempt AND j.result_digest=json_extract(NEW.facts_json,'$.result_digest'))
        THEN RAISE(ABORT,'Job notification requires a committed current result') END;
        END;
CREATE UNIQUE INDEX notification_job_identity ON notification_sources(workspace_id,input_event_id) WHERE role='backend_job';
CREATE TRIGGER notification_job_targets_no_update BEFORE UPDATE ON notification_job_targets BEGIN SELECT RAISE(ABORT,'Job notification target is immutable'); END;
CREATE TRIGGER notification_job_legacy_no_update BEFORE UPDATE ON notification_job_legacy_receipts BEGIN SELECT RAISE(ABORT,'Legacy notification evidence is immutable'); END;
CREATE INDEX notification_source_preparation_failures_source ON notification_source_preparation_failures(source_sequence,sequence);
CREATE TRIGGER notification_source_preparation_failures_no_update BEFORE UPDATE ON notification_source_preparation_failures BEGIN SELECT RAISE(ABORT,'Source preparation evidence is immutable'); END;
CREATE INDEX mr_source_observation_latest ON merge_request_source_observations(workspace_id,merge_request_id,sequence);
CREATE TRIGGER mr_source_observations_no_update BEFORE UPDATE ON merge_request_source_observations BEGIN SELECT RAISE(ABORT,'Source observations are immutable'); END;
CREATE TRIGGER mr_source_observations_no_delete BEFORE DELETE ON merge_request_source_observations BEGIN SELECT RAISE(ABORT,'Source observations are immutable'); END;
CREATE TRIGGER implementation_delegations_no_update BEFORE UPDATE ON ticket_implementation_delegations BEGIN SELECT RAISE(ABORT,'Implementation delegation is immutable'); END;
CREATE TRIGGER implementation_delegations_no_delete BEFORE DELETE ON ticket_implementation_delegations BEGIN SELECT RAISE(ABORT,'Implementation delegation is immutable'); END;
CREATE TRIGGER implementation_results_no_update BEFORE UPDATE ON ticket_implementation_results BEGIN SELECT RAISE(ABORT,'Implementation results are immutable'); END;
CREATE TRIGGER implementation_results_no_delete BEFORE DELETE ON ticket_implementation_results BEGIN SELECT RAISE(ABORT,'Implementation results are immutable'); END;
CREATE UNIQUE INDEX subject_worker_release_active ON subject_worker_releases(workspace_id,worker_id) WHERE cancelled=0;
CREATE TRIGGER passkey_metadata_insert AFTER INSERT ON passkey_credentials
BEGIN
    INSERT INTO passkey_metadata(credential_id, key_reference)
        VALUES (NEW.credential_id, lower(hex(randomblob(16))));
END;
CREATE TRIGGER subject_deletion_worker_prior_insert BEFORE INSERT ON subject_deletion_workers
WHEN NEW.prior_removal_operation_id IS NOT NULL AND NOT EXISTS(
 SELECT 1 FROM worker_removal_operations r WHERE r.operation_id=NEW.prior_removal_operation_id
 AND r.workspace_id=NEW.workspace_id AND r.runtime_id=NEW.runtime_id
 AND r.worker_id=NEW.worker_id AND r.state='succeeded')
BEGIN SELECT RAISE(ABORT,'Subject deletion prior removal must be an exact canonical success'); END;
CREATE TRIGGER subject_deletion_worker_identity_immutable BEFORE UPDATE OF
 workspace_id,subject_id,runtime_id,worker_id,prior_removal_operation_id ON subject_deletion_workers
BEGIN SELECT RAISE(ABORT,'Subject deletion Worker snapshot identity is immutable'); END;
CREATE TRIGGER subject_deletion_original_absence_monotonic BEFORE UPDATE OF originals_absent ON subject_deletion_workers
WHEN OLD.originals_absent=1 AND NEW.originals_absent=0
BEGIN SELECT RAISE(ABORT,'Subject deletion original absence proof is permanent'); END;
CREATE TRIGGER subject_deletion_removal_insert BEFORE INSERT ON subject_deletion_removals
WHEN NEW.completed!=0 OR NEW.job_purge_receipt_json IS NOT NULL OR NOT EXISTS(
 SELECT 1 FROM subject_deletion_workers w JOIN worker_removal_operations r
 ON r.workspace_id=w.workspace_id AND r.runtime_id=w.runtime_id AND r.worker_id=w.worker_id
 WHERE w.workspace_id=NEW.workspace_id AND w.subject_id=NEW.subject_id
 AND w.runtime_id=NEW.runtime_id AND w.worker_id=NEW.worker_id AND w.prior_removal_operation_id IS NULL
 AND r.operation_id=NEW.operation_id AND r.input_fingerprint=NEW.input_fingerprint
 AND r.state IN ('planned','executing','failed')
 AND json_extract(NEW.request_json,'$.operation_id')=r.operation_id
 AND json_extract(NEW.request_json,'$.input_fingerprint')=r.input_fingerprint)
BEGIN SELECT RAISE(ABORT,'Subject deletion reconciliation must pin an unfinished canonical removal'); END;
CREATE TRIGGER subject_deletion_removal_identity_immutable BEFORE UPDATE OF
 workspace_id,subject_id,runtime_id,worker_id,operation_id,input_fingerprint,request_json ON subject_deletion_removals
BEGIN SELECT RAISE(ABORT,'Subject deletion removal request is immutable'); END;
CREATE TRIGGER subject_deletion_removal_completion_monotonic BEFORE UPDATE ON subject_deletion_removals
WHEN (OLD.completed=1 AND NEW.completed!=1)
 OR (OLD.job_purge_receipt_json IS NOT NULL AND NEW.job_purge_receipt_json IS NOT OLD.job_purge_receipt_json)
BEGIN SELECT RAISE(ABORT,'Subject deletion removal receipt is permanent'); END;
CREATE TRIGGER subject_deleted_worker_reservation BEFORE INSERT ON worker_create_reservations
WHEN EXISTS(SELECT 1 FROM subject_deletions d WHERE d.workspace_id=NEW.workspace_id AND
 NEW.singleton_key IN ('subjektiv:'||d.subject_id,'subjektiv-consolidation:'||d.subject_id))
 OR EXISTS(SELECT 1 FROM subject_deletion_allocations d WHERE d.workspace_id=NEW.workspace_id AND d.allocation_key=NEW.allocation_key)
BEGIN SELECT RAISE(ABORT,'subject_deleted: Worker allocation fenced'); END;
CREATE TRIGGER notification_sources_no_delete BEFORE DELETE ON notification_sources
            WHEN NOT EXISTS(SELECT 1 FROM notification_sources s JOIN subject_deletion_jobs j
              ON j.workspace_id=s.workspace_id AND j.job_id=json_extract(s.facts_json,'$.backend_job.job_id')
              WHERE s.role='backend_job' AND s.sequence=OLD.sequence)
            BEGIN SELECT RAISE(ABORT,'Notification evidence is immutable outside Subject deletion'); END;
CREATE TRIGGER notification_decisions_no_delete BEFORE DELETE ON notification_decisions
            WHEN NOT EXISTS(SELECT 1 FROM notification_sources s JOIN subject_deletion_jobs j
              ON j.workspace_id=s.workspace_id AND j.job_id=json_extract(s.facts_json,'$.backend_job.job_id')
              WHERE s.role='backend_job' AND s.sequence=OLD.source_sequence)
            BEGIN SELECT RAISE(ABORT,'Notification evidence is immutable outside Subject deletion'); END;
CREATE TRIGGER notification_requests_no_delete BEFORE DELETE ON notification_requests
            WHEN NOT EXISTS(SELECT 1 FROM notification_sources s JOIN subject_deletion_jobs j
              ON j.workspace_id=s.workspace_id AND j.job_id=json_extract(s.facts_json,'$.backend_job.job_id')
              WHERE s.role='backend_job' AND s.workspace_id=OLD.workspace_id AND s.input_event_id=OLD.delivery_id)
            BEGIN SELECT RAISE(ABORT,'Notification evidence is immutable outside Subject deletion'); END;
CREATE TRIGGER notification_acknowledgements_no_delete BEFORE DELETE ON notification_acknowledgements
            WHEN NOT EXISTS(SELECT 1 FROM notification_sources s JOIN subject_deletion_jobs j
              ON j.workspace_id=s.workspace_id AND j.job_id=json_extract(s.facts_json,'$.backend_job.job_id')
              WHERE s.role='backend_job' AND s.workspace_id=OLD.workspace_id AND s.input_event_id=OLD.delivery_id)
            BEGIN SELECT RAISE(ABORT,'Notification evidence is immutable outside Subject deletion'); END;
CREATE TRIGGER notification_failures_no_delete BEFORE DELETE ON notification_failures
            WHEN NOT EXISTS(SELECT 1 FROM notification_sources s JOIN subject_deletion_jobs j
              ON j.workspace_id=s.workspace_id AND j.job_id=json_extract(s.facts_json,'$.backend_job.job_id')
              WHERE s.role='backend_job' AND s.workspace_id=OLD.workspace_id AND s.input_event_id=OLD.delivery_id)
            BEGIN SELECT RAISE(ABORT,'Notification evidence is immutable outside Subject deletion'); END;
CREATE TRIGGER notification_job_targets_no_delete BEFORE DELETE ON notification_job_targets
            WHEN NOT EXISTS(SELECT 1 FROM notification_sources s JOIN subject_deletion_jobs j
              ON j.workspace_id=s.workspace_id AND j.job_id=json_extract(s.facts_json,'$.backend_job.job_id')
              WHERE s.role='backend_job' AND s.workspace_id=OLD.workspace_id AND s.input_event_id=OLD.delivery_id)
            BEGIN SELECT RAISE(ABORT,'Notification evidence is immutable outside Subject deletion'); END;
CREATE TRIGGER notification_job_legacy_no_delete BEFORE DELETE ON notification_job_legacy_receipts
            WHEN NOT EXISTS(SELECT 1 FROM notification_sources s JOIN subject_deletion_jobs j
              ON j.workspace_id=s.workspace_id AND j.job_id=json_extract(s.facts_json,'$.backend_job.job_id')
              WHERE s.role='backend_job' AND s.workspace_id=OLD.workspace_id AND s.input_event_id=OLD.delivery_id)
            BEGIN SELECT RAISE(ABORT,'Notification evidence is immutable outside Subject deletion'); END;
CREATE TRIGGER notification_source_preparation_failures_no_delete BEFORE DELETE ON notification_source_preparation_failures
            WHEN NOT EXISTS(SELECT 1 FROM notification_sources s JOIN subject_deletion_jobs j
              ON j.workspace_id=s.workspace_id AND j.job_id=json_extract(s.facts_json,'$.backend_job.job_id')
              WHERE s.role='backend_job' AND s.sequence=OLD.source_sequence)
            BEGIN SELECT RAISE(ABORT,'Notification evidence is immutable outside Subject deletion'); END;
CREATE TRIGGER subject_deleted_notification_source BEFORE INSERT ON notification_sources
        WHEN NEW.role='backend_job' AND EXISTS(SELECT 1 FROM subject_deletion_jobs j WHERE j.workspace_id=NEW.workspace_id AND j.job_id=json_extract(NEW.facts_json,'$.backend_job.job_id'))
        BEGIN SELECT RAISE(ABORT,'subject_deleted: Job notification source fenced'); END;
CREATE TRIGGER subject_deleted_notification_payload BEFORE INSERT ON notification_requests
        WHEN EXISTS(SELECT 1 FROM notification_sources s JOIN subject_deletion_jobs j ON j.workspace_id=s.workspace_id AND j.job_id=json_extract(s.facts_json,'$.backend_job.job_id') WHERE s.role='backend_job' AND s.workspace_id=NEW.workspace_id AND s.input_event_id=NEW.delivery_id)
        BEGIN SELECT RAISE(ABORT,'subject_deleted: Job notification payload fenced'); END;
CREATE TRIGGER subject_deleted_notification_send BEFORE INSERT ON notification_job_targets
        WHEN EXISTS(SELECT 1 FROM notification_sources s JOIN subject_deletion_jobs j ON j.workspace_id=s.workspace_id AND j.job_id=json_extract(s.facts_json,'$.backend_job.job_id') WHERE s.role='backend_job' AND s.workspace_id=NEW.workspace_id AND s.input_event_id=NEW.delivery_id)
        BEGIN SELECT RAISE(ABORT,'subject_deleted: Job notification send fenced'); END;
CREATE UNIQUE INDEX plugin_live_alias ON plugin_installations(workspace_id, alias) WHERE uninstalled=0;
CREATE INDEX plugin_upload_expiry ON plugin_uploads(expires_at);
CREATE TRIGGER plugin_revocation_irreversible BEFORE UPDATE OF revoked ON plugin_artifacts
WHEN OLD.revoked=1 AND NEW.revoked<>1 BEGIN SELECT RAISE(ABORT,'plugin revocation is irreversible'); END;
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
CREATE TRIGGER plugin_plan_replay_retained BEFORE DELETE ON plugin_execution_plans
WHEN EXISTS(SELECT 1 FROM workspaces WHERE workspace_id=OLD.workspace_id)
BEGIN SELECT RAISE(ABORT,'Plugin execution plan replay must be retained'); END;
CREATE TRIGGER typed_ticket_assignment_orchestrator_principal_insert
BEFORE INSERT ON ticket_worker_assignments
WHEN NEW.role='orchestrator' AND NEW.principal_kind!='subject'
BEGIN SELECT RAISE(ABORT,'New Orchestrator responsibility requires a Subject principal'); END;
CREATE TRIGGER typed_ticket_assignment_role_principal_insert
BEFORE INSERT ON ticket_worker_assignments
WHEN (NEW.role='owner' AND NEW.principal_kind!='user')
 OR (NEW.role='worker' AND NEW.principal_kind IN ('workspace_agent','subject'))
 OR (NEW.principal_kind='workspace_agent' AND NEW.principal_id!='workspace-orchestrator')
 OR (NEW.principal_kind='subject' AND NEW.role!='orchestrator')
BEGIN SELECT RAISE(ABORT,'Assignment role and principal are incompatible'); END;
CREATE TRIGGER ticket_subject_current_insert
BEFORE INSERT ON ticket_current_worker_assignments
WHEN (NEW.role='orchestrator' AND NEW.principal_kind!='subject')
 OR (NEW.principal_kind='subject' AND NEW.role!='orchestrator')
BEGIN SELECT RAISE(ABORT,'Current Orchestrator responsibility requires a Subject principal'); END;
CREATE TRIGGER ticket_subject_current_update
BEFORE UPDATE OF workspace_id,ticket_id,role,assignment_id,principal_kind,principal_id,runtime_id,worker_id
ON ticket_current_worker_assignments
WHEN (NEW.role='orchestrator' AND NEW.principal_kind!='subject')
 OR (NEW.principal_kind='subject' AND NEW.role!='orchestrator')
BEGIN SELECT RAISE(ABORT,'Current Orchestrator responsibility requires a Subject principal'); END;
CREATE TRIGGER ticket_subject_operation_insert
BEFORE INSERT ON ticket_assignment_operations
WHEN (NEW.role='orchestrator' AND NEW.action!='unassign' AND NEW.principal_kind!='subject')
 OR (NEW.principal_kind='subject' AND (NEW.role!='orchestrator'
   OR NEW.principal_id IS NULL OR length(trim(NEW.principal_id))=0
   OR NEW.runtime_id IS NOT NULL OR NEW.worker_id IS NOT NULL))
BEGIN SELECT RAISE(ABORT,'New Orchestrator assignment operation requires a Subject principal'); END;
CREATE TRIGGER ticket_subject_operation_update
BEFORE UPDATE ON ticket_assignment_operations
WHEN (NEW.role='orchestrator' AND NEW.action!='unassign' AND NEW.principal_kind!='subject')
 OR (NEW.principal_kind='subject' AND (NEW.role!='orchestrator'
   OR NEW.principal_id IS NULL OR length(trim(NEW.principal_id))=0
   OR NEW.runtime_id IS NOT NULL OR NEW.worker_id IS NOT NULL))
BEGIN SELECT RAISE(ABORT,'New Orchestrator assignment operation requires a Subject principal'); END;
CREATE TRIGGER ticket_legacy_orchestrator_tracking_no_update
BEFORE UPDATE OF tracking_json,last_tracking_event_id,tracking_updated_at ON ticket_worker_assignments
WHEN OLD.role='orchestrator' AND OLD.principal_kind!='subject'
BEGIN SELECT RAISE(ABORT,'Legacy Orchestrator responsibility is nonoperational'); END;
CREATE TRIGGER ticket_legacy_orchestrator_history_no_insert
BEFORE INSERT ON typed_ticket_assignment_tracking_history
WHEN EXISTS(SELECT 1 FROM ticket_worker_assignments a
 WHERE a.workspace_id=NEW.workspace_id AND a.assignment_id=NEW.assignment_id
 AND a.role='orchestrator' AND a.principal_kind!='subject')
BEGIN SELECT RAISE(ABORT,'Legacy Orchestrator responsibility is nonoperational'); END;
CREATE TRIGGER ticket_legacy_orchestrator_delivery_no_insert
BEFORE INSERT ON ticket_assignment_deliveries
WHEN EXISTS(SELECT 1 FROM ticket_worker_assignments a
 WHERE a.workspace_id=NEW.workspace_id AND a.assignment_id=NEW.assignment_id
 AND a.role='orchestrator' AND a.principal_kind!='subject')
BEGIN SELECT RAISE(ABORT,'Legacy Orchestrator responsibility is nonoperational'); END;
CREATE TRIGGER ticket_legacy_orchestrator_start_no_insert
BEFORE INSERT ON ticket_assignment_operation_fences
WHEN NOT EXISTS(SELECT 1 FROM ticket_worker_assignments a
 WHERE a.workspace_id=NEW.workspace_id AND a.assignment_id=NEW.orchestrator_assignment_id
 AND a.role='orchestrator' AND a.principal_kind='subject')
BEGIN SELECT RAISE(ABORT,'Ticket startup fence requires Subject responsibility'); END;
CREATE TRIGGER notification_ticket_event AFTER INSERT ON typed_ticket_events WHEN NEW.event_index>=0 BEGIN INSERT OR IGNORE INTO notification_sources(workspace_id,ticket_id,input_event_id,role,assignment_id,kind,actor,event_index,facts_json)
 SELECT NEW.workspace_id,NEW.ticket_id,'ticket-event:' || NEW.event_index,roles.role,COALESCE(a.assignment_id,''),CASE WHEN NEW.state_field='case' THEN 'case' WHEN NEW.state_field='tags' THEN 'tags' ELSE NEW.kind END,NEW.author,NEW.event_index,json_object('principal_kind',a.principal_kind,'principal_id',a.principal_id,'runtime_id',CASE WHEN a.principal_kind='subject' THEN (SELECT runtime_id FROM worker_singleton_owners WHERE workspace_id=a.workspace_id AND singleton_key='subjektiv:' || a.principal_id) ELSE a.runtime_id END,'worker_id',CASE WHEN a.principal_kind='subject' THEN (SELECT worker_id FROM worker_singleton_owners WHERE workspace_id=a.workspace_id AND singleton_key='subjektiv:' || a.principal_id) ELSE a.worker_id END,'status',json_extract(a.tracking_json,'$.status'),'case_state',t.workflow_state,'released',json(CASE WHEN EXISTS(SELECT 1 FROM ticket_assignment_work_releases r WHERE r.workspace_id=a.workspace_id AND r.assignment_id=a.assignment_id) THEN 'true' ELSE 'false' END))
 FROM (SELECT 'orchestrator' role UNION ALL SELECT 'worker') roles
 JOIN typed_tickets t ON t.workspace_id=NEW.workspace_id AND t.ticket_id=NEW.ticket_id
 LEFT JOIN ticket_current_worker_assignments c ON c.workspace_id=t.workspace_id AND c.ticket_id=t.ticket_id AND c.role=roles.role
 LEFT JOIN ticket_worker_assignments a ON a.workspace_id=c.workspace_id AND a.assignment_id=c.assignment_id; END;
CREATE TRIGGER notification_assignment_history AFTER INSERT ON typed_ticket_assignment_tracking_history BEGIN INSERT OR IGNORE INTO notification_sources(workspace_id,ticket_id,input_event_id,role,assignment_id,kind,actor,event_index,facts_json)
 SELECT NEW.workspace_id,NEW.ticket_id,'assignment-event:' || NEW.event_id,roles.role,COALESCE(a.assignment_id,''),CASE WHEN json_extract(NEW.source_context_json,'$.kind')='assignment' THEN 'assignment' ELSE 'tracking' END,json_extract(NEW.source_context_json,'$.actor'),NULL,json_object('principal_kind',a.principal_kind,'principal_id',a.principal_id,'runtime_id',CASE WHEN a.principal_kind='subject' THEN (SELECT runtime_id FROM worker_singleton_owners WHERE workspace_id=a.workspace_id AND singleton_key='subjektiv:' || a.principal_id) ELSE a.runtime_id END,'worker_id',CASE WHEN a.principal_kind='subject' THEN (SELECT worker_id FROM worker_singleton_owners WHERE workspace_id=a.workspace_id AND singleton_key='subjektiv:' || a.principal_id) ELSE a.worker_id END,'status',json_extract(a.tracking_json,'$.status'),'case_state',t.workflow_state,'released',json(CASE WHEN EXISTS(SELECT 1 FROM ticket_assignment_work_releases r WHERE r.workspace_id=a.workspace_id AND r.assignment_id=a.assignment_id) THEN 'true' ELSE 'false' END))
 FROM (SELECT 'orchestrator' role UNION ALL SELECT 'worker') roles
 JOIN typed_tickets t ON t.workspace_id=NEW.workspace_id AND t.ticket_id=NEW.ticket_id
 LEFT JOIN ticket_current_worker_assignments c ON c.workspace_id=t.workspace_id AND c.ticket_id=t.ticket_id AND c.role=roles.role
 LEFT JOIN ticket_worker_assignments a ON a.workspace_id=c.workspace_id AND a.assignment_id=c.assignment_id; END;
CREATE TRIGGER notification_assignment_change AFTER INSERT ON ticket_worker_assignment_events BEGIN INSERT OR IGNORE INTO notification_sources(workspace_id,ticket_id,input_event_id,role,assignment_id,kind,actor,event_index,facts_json)
 SELECT NEW.workspace_id,NEW.ticket_id,'assignment-change:' || NEW.event_id,roles.role,COALESCE(a.assignment_id,''),'assignment_changed',NEW.actor,NULL,json_object('principal_kind',a.principal_kind,'principal_id',a.principal_id,'runtime_id',CASE WHEN a.principal_kind='subject' THEN (SELECT runtime_id FROM worker_singleton_owners WHERE workspace_id=a.workspace_id AND singleton_key='subjektiv:' || a.principal_id) ELSE a.runtime_id END,'worker_id',CASE WHEN a.principal_kind='subject' THEN (SELECT worker_id FROM worker_singleton_owners WHERE workspace_id=a.workspace_id AND singleton_key='subjektiv:' || a.principal_id) ELSE a.worker_id END,'status',json_extract(a.tracking_json,'$.status'),'case_state',t.workflow_state,'released',json(CASE WHEN EXISTS(SELECT 1 FROM ticket_assignment_work_releases r WHERE r.workspace_id=a.workspace_id AND r.assignment_id=a.assignment_id) THEN 'true' ELSE 'false' END))
 FROM (SELECT 'orchestrator' role UNION ALL SELECT 'worker') roles
 JOIN typed_tickets t ON t.workspace_id=NEW.workspace_id AND t.ticket_id=NEW.ticket_id
 LEFT JOIN ticket_current_worker_assignments c ON c.workspace_id=t.workspace_id AND c.ticket_id=t.ticket_id AND c.role=roles.role
 LEFT JOIN ticket_worker_assignments a ON a.workspace_id=c.workspace_id AND a.assignment_id=c.assignment_id;
 INSERT OR IGNORE INTO notification_sources(workspace_id,ticket_id,input_event_id,role,assignment_id,kind,actor,event_index,facts_json)
 SELECT a.workspace_id,a.ticket_id,'assignment-change:' || NEW.event_id,a.role,a.assignment_id,'release',NEW.actor,NULL,json_set(json_object('principal_kind',a.principal_kind,'principal_id',a.principal_id,'runtime_id',CASE WHEN a.principal_kind='subject' THEN (SELECT runtime_id FROM worker_singleton_owners WHERE workspace_id=a.workspace_id AND singleton_key='subjektiv:' || a.principal_id) ELSE a.runtime_id END,'worker_id',CASE WHEN a.principal_kind='subject' THEN (SELECT worker_id FROM worker_singleton_owners WHERE workspace_id=a.workspace_id AND singleton_key='subjektiv:' || a.principal_id) ELSE a.worker_id END,'status',json_extract(a.tracking_json,'$.status'),'case_state',t.workflow_state,'released',json(CASE WHEN EXISTS(SELECT 1 FROM ticket_assignment_work_releases r WHERE r.workspace_id=a.workspace_id AND r.assignment_id=a.assignment_id) THEN 'true' ELSE 'false' END)),'$.released',json('true'))
 FROM ticket_worker_assignments a JOIN typed_tickets t ON t.workspace_id=a.workspace_id AND t.ticket_id=a.ticket_id
 WHERE a.workspace_id=NEW.workspace_id AND a.assignment_id=NEW.previous_assignment_id AND NEW.previous_assignment_id IS NOT NEW.assignment_id;
 END;
CREATE TRIGGER notification_assignment_release AFTER INSERT ON ticket_assignment_work_releases BEGIN INSERT OR IGNORE INTO notification_sources(workspace_id,ticket_id,input_event_id,role,assignment_id,kind,actor,event_index,facts_json)
 SELECT NEW.workspace_id,(SELECT ticket_id FROM ticket_worker_assignments WHERE workspace_id=NEW.workspace_id AND assignment_id=NEW.assignment_id),'assignment-release:' || NEW.assignment_id,roles.role,COALESCE(a.assignment_id,''),'release',NULL,NULL,json_object('principal_kind',a.principal_kind,'principal_id',a.principal_id,'runtime_id',CASE WHEN a.principal_kind='subject' THEN (SELECT runtime_id FROM worker_singleton_owners WHERE workspace_id=a.workspace_id AND singleton_key='subjektiv:' || a.principal_id) ELSE a.runtime_id END,'worker_id',CASE WHEN a.principal_kind='subject' THEN (SELECT worker_id FROM worker_singleton_owners WHERE workspace_id=a.workspace_id AND singleton_key='subjektiv:' || a.principal_id) ELSE a.worker_id END,'status',json_extract(a.tracking_json,'$.status'),'case_state',t.workflow_state,'released',json(CASE WHEN EXISTS(SELECT 1 FROM ticket_assignment_work_releases r WHERE r.workspace_id=a.workspace_id AND r.assignment_id=a.assignment_id) THEN 'true' ELSE 'false' END))
 FROM (SELECT 'orchestrator' role UNION ALL SELECT 'worker') roles
 JOIN typed_tickets t ON t.workspace_id=NEW.workspace_id AND t.ticket_id=(SELECT ticket_id FROM ticket_worker_assignments WHERE workspace_id=NEW.workspace_id AND assignment_id=NEW.assignment_id)
 LEFT JOIN ticket_current_worker_assignments c ON c.workspace_id=t.workspace_id AND c.ticket_id=t.ticket_id AND c.role=roles.role
 LEFT JOIN ticket_worker_assignments a ON a.workspace_id=c.workspace_id AND a.assignment_id=c.assignment_id; END;
CREATE TRIGGER notification_content_input AFTER INSERT ON ticket_assignment_content_inputs BEGIN
 INSERT INTO notification_sources(workspace_id,ticket_id,input_event_id,role,assignment_id,kind,actor,event_index,facts_json)
 SELECT NEW.workspace_id,NEW.ticket_id,'ticket-edit:' || printf('%020d',NEW.input_sequence),a.role,a.assignment_id,NEW.kind,NEW.actor,NULL,json_set(json_object('principal_kind',a.principal_kind,'principal_id',a.principal_id,'runtime_id',CASE WHEN a.principal_kind='subject' THEN (SELECT runtime_id FROM worker_singleton_owners WHERE workspace_id=a.workspace_id AND singleton_key='subjektiv:' || a.principal_id) ELSE a.runtime_id END,'worker_id',CASE WHEN a.principal_kind='subject' THEN (SELECT worker_id FROM worker_singleton_owners WHERE workspace_id=a.workspace_id AND singleton_key='subjektiv:' || a.principal_id) ELSE a.worker_id END,'status',json_extract(a.tracking_json,'$.status'),'case_state',t.workflow_state,'released',json(CASE WHEN EXISTS(SELECT 1 FROM ticket_assignment_work_releases r WHERE r.workspace_id=a.workspace_id AND r.assignment_id=a.assignment_id) THEN 'true' ELSE 'false' END)),'$.content_change_id',NEW.source_change_id)
 FROM ticket_worker_assignments a JOIN typed_tickets t ON t.workspace_id=a.workspace_id AND t.ticket_id=a.ticket_id
 WHERE a.workspace_id=NEW.workspace_id AND a.assignment_id=NEW.assignment_id;
 END;
CREATE TRIGGER notification_unassigned_content AFTER INSERT ON typed_ticket_content_changes BEGIN INSERT OR IGNORE INTO notification_sources(workspace_id,ticket_id,input_event_id,role,assignment_id,kind,actor,event_index,facts_json)
 SELECT NEW.workspace_id,NEW.ticket_id,'ticket-content:' || NEW.change_id,roles.role,COALESCE(a.assignment_id,''),NEW.kind,NEW.actor,NULL,json_object('principal_kind',a.principal_kind,'principal_id',a.principal_id,'runtime_id',CASE WHEN a.principal_kind='subject' THEN (SELECT runtime_id FROM worker_singleton_owners WHERE workspace_id=a.workspace_id AND singleton_key='subjektiv:' || a.principal_id) ELSE a.runtime_id END,'worker_id',CASE WHEN a.principal_kind='subject' THEN (SELECT worker_id FROM worker_singleton_owners WHERE workspace_id=a.workspace_id AND singleton_key='subjektiv:' || a.principal_id) ELSE a.worker_id END,'status',json_extract(a.tracking_json,'$.status'),'case_state',t.workflow_state,'released',json(CASE WHEN EXISTS(SELECT 1 FROM ticket_assignment_work_releases r WHERE r.workspace_id=a.workspace_id AND r.assignment_id=a.assignment_id) THEN 'true' ELSE 'false' END))
 FROM (SELECT 'orchestrator' role UNION ALL SELECT 'worker') roles
 JOIN typed_tickets t ON t.workspace_id=NEW.workspace_id AND t.ticket_id=NEW.ticket_id
 LEFT JOIN ticket_current_worker_assignments c ON c.workspace_id=t.workspace_id AND c.ticket_id=t.ticket_id AND c.role=roles.role
 LEFT JOIN ticket_worker_assignments a ON a.workspace_id=c.workspace_id AND a.assignment_id=c.assignment_id WHERE a.assignment_id IS NULL; END;
CREATE TRIGGER notification_mr_event AFTER INSERT ON merge_request_thread_events WHEN NEW.kind!='comment' BEGIN INSERT OR IGNORE INTO notification_sources(workspace_id,ticket_id,input_event_id,role,assignment_id,kind,actor,event_index,facts_json)
 SELECT NEW.workspace_id,r.ticket_id,'mr-event:' || NEW.merge_request_id || ':' || NEW.event_id,roles.role,COALESCE(a.assignment_id,''),'mr_' || NEW.kind,'worker:' || json_extract(NEW.payload_json,(CASE NEW.kind WHEN 'review_requested' THEN '$.requested_by' WHEN 'review' THEN '$.reviewer' WHEN 'review_revoked' THEN '$.revoked_by' WHEN 'merge' THEN '$.merged_by' END) || '.runtime_id') || '/' || json_extract(NEW.payload_json,(CASE NEW.kind WHEN 'review_requested' THEN '$.requested_by' WHEN 'review' THEN '$.reviewer' WHEN 'review_revoked' THEN '$.revoked_by' WHEN 'merge' THEN '$.merged_by' END) || '.worker_id'),NULL,
 json_set(json_object('principal_kind',a.principal_kind,'principal_id',a.principal_id,'runtime_id',CASE WHEN a.principal_kind='subject' THEN (SELECT runtime_id FROM worker_singleton_owners WHERE workspace_id=a.workspace_id AND singleton_key='subjektiv:' || a.principal_id) ELSE a.runtime_id END,'worker_id',CASE WHEN a.principal_kind='subject' THEN (SELECT worker_id FROM worker_singleton_owners WHERE workspace_id=a.workspace_id AND singleton_key='subjektiv:' || a.principal_id) ELSE a.worker_id END,'status',json_extract(a.tracking_json,'$.status'),'case_state',t.workflow_state,'released',json(CASE WHEN EXISTS(SELECT 1 FROM ticket_assignment_work_releases r WHERE r.workspace_id=a.workspace_id AND r.assignment_id=a.assignment_id) THEN 'true' ELSE 'false' END)),'$.merge_request_id',NEW.merge_request_id,'$.event_id',NEW.event_id,'$.subject_ref',COALESCE(json_extract(NEW.payload_json,'$.subject_ref'),json_extract(NEW.payload_json,'$.approved_source_ref')),'$.request_event_id',json_extract(NEW.payload_json,'$.request_event_id'))
 FROM merge_request_ticket_relations r JOIN typed_tickets t ON t.workspace_id=r.workspace_id AND t.ticket_id=r.ticket_id
 CROSS JOIN (SELECT 'orchestrator' role UNION ALL SELECT 'worker') roles
 LEFT JOIN ticket_current_worker_assignments c ON c.workspace_id=t.workspace_id AND c.ticket_id=t.ticket_id AND c.role=roles.role
 LEFT JOIN ticket_worker_assignments a ON a.workspace_id=c.workspace_id AND a.assignment_id=c.assignment_id
 WHERE r.workspace_id=NEW.workspace_id AND r.merge_request_id=NEW.merge_request_id; END;
CREATE TRIGGER notification_mr_source AFTER INSERT ON merge_request_source_observations WHEN NEW.previous_subject_ref IS NOT NULL BEGIN INSERT OR IGNORE INTO notification_sources(workspace_id,ticket_id,input_event_id,role,assignment_id,kind,actor,event_index,facts_json)
 SELECT NEW.workspace_id,r.ticket_id,'mr-source:' || NEW.merge_request_id || ':' || NEW.sequence,roles.role,COALESCE(a.assignment_id,''),'mr_source_changed',NULL,NULL,
 json_set(json_object('principal_kind',a.principal_kind,'principal_id',a.principal_id,'runtime_id',CASE WHEN a.principal_kind='subject' THEN (SELECT runtime_id FROM worker_singleton_owners WHERE workspace_id=a.workspace_id AND singleton_key='subjektiv:' || a.principal_id) ELSE a.runtime_id END,'worker_id',CASE WHEN a.principal_kind='subject' THEN (SELECT worker_id FROM worker_singleton_owners WHERE workspace_id=a.workspace_id AND singleton_key='subjektiv:' || a.principal_id) ELSE a.worker_id END,'status',json_extract(a.tracking_json,'$.status'),'case_state',t.workflow_state,'released',json(CASE WHEN EXISTS(SELECT 1 FROM ticket_assignment_work_releases r WHERE r.workspace_id=a.workspace_id AND r.assignment_id=a.assignment_id) THEN 'true' ELSE 'false' END)),'$.merge_request_id',NEW.merge_request_id,'$.event_id',NEW.sequence,'$.subject_ref',NEW.subject_ref,'$.request_event_id',NULL)
 FROM merge_request_ticket_relations r JOIN typed_tickets t ON t.workspace_id=r.workspace_id AND t.ticket_id=r.ticket_id
 CROSS JOIN (SELECT 'orchestrator' role UNION ALL SELECT 'worker') roles
 LEFT JOIN ticket_current_worker_assignments c ON c.workspace_id=t.workspace_id AND c.ticket_id=t.ticket_id AND c.role=roles.role
 LEFT JOIN ticket_worker_assignments a ON a.workspace_id=c.workspace_id AND a.assignment_id=c.assignment_id
 WHERE r.workspace_id=NEW.workspace_id AND r.merge_request_id=NEW.merge_request_id; END;
CREATE TRIGGER notification_implementation_result AFTER INSERT ON ticket_implementation_results BEGIN
 INSERT INTO notification_sources(workspace_id,ticket_id,input_event_id,role,assignment_id,kind,actor,event_index,facts_json)
 SELECT NEW.workspace_id,NEW.ticket_id,'implementation-result:' || NEW.result_id,'orchestrator',COALESCE(a.assignment_id,''),'implementation_result','worker:' || NEW.runtime_id || '/' || NEW.worker_id,NULL,
 json_set(json_object('principal_kind',a.principal_kind,'principal_id',a.principal_id,'runtime_id',CASE WHEN a.principal_kind='subject' THEN (SELECT runtime_id FROM worker_singleton_owners WHERE workspace_id=a.workspace_id AND singleton_key='subjektiv:' || a.principal_id) ELSE a.runtime_id END,'worker_id',CASE WHEN a.principal_kind='subject' THEN (SELECT worker_id FROM worker_singleton_owners WHERE workspace_id=a.workspace_id AND singleton_key='subjektiv:' || a.principal_id) ELSE a.worker_id END,'status',json_extract(a.tracking_json,'$.status'),'case_state',t.workflow_state,'released',json(CASE WHEN EXISTS(SELECT 1 FROM ticket_assignment_work_releases r WHERE r.workspace_id=a.workspace_id AND r.assignment_id=a.assignment_id) THEN 'true' ELSE 'false' END)),'$.result_id',NEW.result_id,'$.merge_request_id',json_extract(NEW.request_json,'$.merge_request.merge_request_id'))
 FROM typed_tickets t
 LEFT JOIN ticket_current_worker_assignments c ON c.workspace_id=t.workspace_id AND c.ticket_id=t.ticket_id AND c.role='orchestrator'
 LEFT JOIN ticket_worker_assignments a ON a.workspace_id=c.workspace_id AND a.assignment_id=c.assignment_id
 WHERE t.workspace_id=NEW.workspace_id AND t.ticket_id=NEW.ticket_id;
 END;
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
CREATE TRIGGER plugin_invocation_no_replace BEFORE INSERT ON plugin_invocations
WHEN EXISTS(SELECT 1 FROM plugin_invocations WHERE workspace_id=NEW.workspace_id AND
 (invocation_id=NEW.invocation_id OR (runtime_id=NEW.runtime_id AND worker_id=NEW.worker_id AND installation_id=NEW.installation_id AND consumer_key=NEW.consumer_key)))
BEGIN SELECT RAISE(ABORT,'Plugin invocation identity cannot be reused'); END;
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
CREATE TRIGGER worker_plugin_launch_immutable BEFORE UPDATE ON worker_plugin_launches
BEGIN SELECT RAISE(ABORT,'Worker Plugin launch selection is immutable'); END;
CREATE TRIGGER worker_plugin_launch_retained BEFORE DELETE ON worker_plugin_launches
WHEN EXISTS(SELECT 1 FROM workspaces WHERE workspace_id=OLD.workspace_id)
BEGIN SELECT RAISE(ABORT,'Worker Plugin launch selection must be retained'); END;
CREATE TRIGGER worker_plugin_launch_identity BEFORE INSERT ON worker_plugin_launches
WHEN NOT EXISTS(SELECT 1 FROM worker_create_reservations WHERE workspace_id=NEW.workspace_id AND worker_id=NEW.worker_id AND runtime_id=NEW.runtime_id)
BEGIN SELECT RAISE(ABORT,'Worker Plugin launch reservation identity mismatch'); END;
CREATE TRIGGER plugin_reference_live BEFORE INSERT ON plugin_references
WHEN NOT EXISTS(SELECT 1 FROM plugin_artifacts a JOIN plugin_installations i USING(workspace_id, installation_id)
 WHERE a.workspace_id=NEW.workspace_id AND a.installation_id=NEW.installation_id AND a.artifact_digest=NEW.artifact_digest AND a.revoked=0 AND i.uninstalled=0)
BEGIN SELECT RAISE(ABORT,'plugin reference target unavailable'); END;
CREATE TRIGGER plugin_uninstall_referenced BEFORE UPDATE OF uninstalled ON plugin_installations
WHEN NEW.uninstalled=1 AND EXISTS(SELECT 1 FROM plugin_references r WHERE r.workspace_id=NEW.workspace_id AND r.installation_id=NEW.installation_id)
BEGIN SELECT RAISE(ABORT,'plugin installation referenced'); END;
CREATE TRIGGER plugin_data_parent_insert BEFORE INSERT ON plugin_data_cells
WHEN NOT EXISTS(SELECT 1 FROM typed_tickets t JOIN workspaces w USING(workspace_id)
 WHERE t.workspace_id=NEW.workspace_id AND t.ticket_id=NEW.ticket_id AND w.state='active')
BEGIN SELECT RAISE(ABORT,'Plugin data parent unavailable'); END;
CREATE TRIGGER plugin_data_parent_update BEFORE UPDATE ON plugin_data_cells
WHEN NOT EXISTS(SELECT 1 FROM typed_tickets t JOIN workspaces w USING(workspace_id)
 WHERE t.workspace_id=NEW.workspace_id AND t.ticket_id=NEW.ticket_id AND w.state='active')
BEGIN SELECT RAISE(ABORT,'Plugin data parent unavailable'); END;
CREATE INDEX plugin_data_cursor_expiry ON plugin_data_cursors(workspace_id,expires_at_ms);
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
CREATE TRIGGER plugin_data_purge_immutable BEFORE UPDATE ON plugin_data_purges
BEGIN SELECT RAISE(ABORT,'Plugin purge receipt immutable'); END;
CREATE TRIGGER plugin_data_purge_no_replace BEFORE INSERT ON plugin_data_purges
WHEN EXISTS(SELECT 1 FROM plugin_data_purges WHERE workspace_id=NEW.workspace_id AND actor=NEW.actor AND operation_key=NEW.operation_key)
BEGIN SELECT RAISE(ABORT,'Plugin purge key retained'); END;
CREATE TRIGGER plugin_data_purge_retained BEFORE DELETE ON plugin_data_purges
WHEN EXISTS(SELECT 1 FROM workspaces WHERE workspace_id=OLD.workspace_id)
BEGIN SELECT RAISE(ABORT,'Plugin purge receipt retained'); END;
CREATE INDEX typed_ticket_service_outbox_workspace_sequence
    ON typed_ticket_service_outbox(workspace_id, sequence);
CREATE TRIGGER plugin_service_released_proof_immutable BEFORE UPDATE ON plugin_service_released_proofs
BEGIN SELECT RAISE(ABORT,'Service released proof is immutable'); END;
CREATE TRIGGER plugin_service_released_proof_retained BEFORE DELETE ON plugin_service_released_proofs
WHEN EXISTS(SELECT 1 FROM plugin_service_consumers WHERE workspace_id=OLD.workspace_id AND consumer_id=OLD.consumer_id)
BEGIN SELECT RAISE(ABORT,'Service released proof must be retained'); END;
CREATE TRIGGER plugin_service_identity_immutable BEFORE UPDATE OF workspace_id,consumer_id,runtime_id,worker_id,installation_id,plan_id,service_name,subscriptions_json ON plugin_service_consumers
BEGIN SELECT RAISE(ABORT,'Service consumer identity is immutable'); END;
CREATE TRIGGER plugin_service_cursor_forward BEFORE UPDATE OF cursor ON plugin_service_consumers WHEN NEW.cursor<OLD.cursor
BEGIN SELECT RAISE(ABORT,'Service cursor cannot rewind'); END;
CREATE TRIGGER plugin_service_audit_immutable BEFORE UPDATE ON plugin_service_recovery_audit
BEGIN SELECT RAISE(ABORT,'Service recovery audit is immutable'); END;
CREATE TRIGGER plugin_service_audit_retained BEFORE DELETE ON plugin_service_recovery_audit WHEN EXISTS(SELECT 1 FROM workspaces WHERE workspace_id=OLD.workspace_id)
BEGIN SELECT RAISE(ABORT,'Service recovery audit must be retained'); END;
CREATE INDEX typed_ticket_parent_links_children
    ON typed_ticket_parent_links(workspace_id, parent_ticket_id, ticket_id);
CREATE TRIGGER workspace_configuration_authorities_no_update BEFORE UPDATE ON workspace_configuration_authorities BEGIN
    SELECT RAISE(ABORT, 'configuration authority transfers are immutable');
END;
CREATE TRIGGER workspace_configuration_authorities_no_delete BEFORE DELETE ON workspace_configuration_authorities BEGIN
    SELECT RAISE(ABORT, 'configuration authority transfers are retained');
END;
CREATE TRIGGER workspace_configuration_receipts_no_update BEFORE UPDATE ON workspace_configuration_receipts BEGIN
    SELECT RAISE(ABORT, 'configuration adoption receipts are immutable');
END;
CREATE TRIGGER workspace_configuration_receipts_no_delete BEFORE DELETE ON workspace_configuration_receipts BEGIN
    SELECT RAISE(ABORT, 'configuration adoption receipts are retained');
END;
CREATE TRIGGER backend_job_upgrade_history_no_update
BEFORE UPDATE ON backend_job_upgrade_history
BEGIN SELECT RAISE(ABORT, 'immutable Backend Job upgrade history'); END;
CREATE TRIGGER backend_job_attempt_upgrade_history_no_update
BEFORE UPDATE ON backend_job_attempt_upgrade_history
BEGIN SELECT RAISE(ABORT, 'immutable Backend Job attempt upgrade history'); END;
CREATE INDEX backend_jobs_active
            ON backend_jobs(workspace_id, state, updated_at);
CREATE INDEX backend_jobs_resource ON backend_jobs(workspace_id, resource_key, created_at);
CREATE TRIGGER backend_job_identity_immutable BEFORE UPDATE OF
    workspace_id, job_id, purpose, input_digest, input_ref, resource_key, intent_fingerprint ON backend_jobs
BEGIN SELECT RAISE(ABORT, 'immutable Backend Job identity'); END;
CREATE TRIGGER backend_job_input_retirement BEFORE UPDATE OF request_json, input_disposition ON backend_jobs
WHEN (
    ((OLD.input_disposition = 'available' AND NEW.input_disposition IN ('reclaimed','erased','invalidated'))
      OR (OLD.input_disposition IN ('reclaimed','invalidated') AND NEW.input_disposition = 'erased'))
    AND json_remove(NEW.request_json,'$.input') = json_remove(OLD.request_json,'$.input')
    AND json_type(NEW.request_json,'$.input') = 'null'
) IS NOT TRUE
BEGIN SELECT RAISE(ABORT, 'immutable Backend Job input retirement'); END;
CREATE TRIGGER backend_job_pre_content_no_input_restore
BEFORE UPDATE OF request_json ON backend_jobs
WHEN COALESCE(json_type(NEW.request_json,'$.input'),'null')!='null'
 AND EXISTS(SELECT 1 FROM backend_job_pre_content_history h
            WHERE h.workspace_id=OLD.workspace_id AND h.job_id=OLD.job_id AND h.input_retired=1)
BEGIN SELECT RAISE(ABORT,'retired pre-content Job input cannot regenerate'); END;
CREATE TRIGGER backend_job_pre_content_retire_with_owner
AFTER UPDATE OF request_json ON backend_jobs
WHEN COALESCE(json_type(NEW.request_json,'$.input'),'null')='null'
BEGIN
    UPDATE backend_job_pre_content_history
    SET request_json=json_set(request_json,'$.input',json('null')),input_retired=1
    WHERE workspace_id=NEW.workspace_id AND job_id=NEW.job_id AND input_retired=0;
END;
CREATE TRIGGER subject_deleted_job_insert BEFORE INSERT ON backend_jobs
WHEN EXISTS(SELECT 1 FROM subject_deletions d WHERE d.workspace_id=NEW.workspace_id AND
 (d.subject_id=json_extract(NEW.request_json,'$.grants.subjektiv_journal.subject_id') OR
  d.subject_id=json_extract(NEW.request_json,'$.grants.subjektiv_consolidation.subject_id')))
 OR EXISTS(SELECT 1 FROM subject_deletion_jobs d WHERE d.workspace_id=NEW.workspace_id AND d.job_id=NEW.job_id)
BEGIN SELECT RAISE(ABORT,'subject_deleted: Job admission fenced'); END;
CREATE TRIGGER subject_deleted_job_result BEFORE UPDATE OF state,result_json ON backend_jobs
WHEN NEW.state!='failed' AND EXISTS(SELECT 1 FROM subject_deletion_jobs d WHERE d.workspace_id=NEW.workspace_id AND d.job_id=NEW.job_id)
BEGIN SELECT RAISE(ABORT,'subject_deleted: Job result fenced'); END;
CREATE INDEX backend_job_attempts_recovery
            ON backend_job_attempts(workspace_id, state, deadline_at);
CREATE INDEX backend_job_attempts_worker_cleanup
            ON backend_job_attempts(workspace_id, worker_cleanup_state, updated_at);
CREATE TRIGGER subject_deleted_attempt_insert BEFORE INSERT ON backend_job_attempts
WHEN EXISTS(SELECT 1 FROM subject_deletion_jobs d WHERE d.workspace_id=NEW.workspace_id AND d.job_id=NEW.job_id)
BEGIN SELECT RAISE(ABORT,'subject_deleted: Job retry fenced'); END;
CREATE TRIGGER subject_deleted_attempt_result BEFORE UPDATE OF state,result_json,runtime_id,worker_id,runtime_run_id ON backend_job_attempts
WHEN NEW.state!='failed' AND EXISTS(SELECT 1 FROM subject_deletion_jobs d WHERE d.workspace_id=NEW.workspace_id AND d.job_id=NEW.job_id)
BEGIN SELECT RAISE(ABORT,'subject_deleted: Job attempt fenced'); END;
CREATE TRIGGER backend_job_upgrade_no_resume
BEFORE UPDATE ON backend_jobs
WHEN EXISTS (SELECT 1 FROM backend_job_upgrade_history h
             WHERE h.workspace_id=OLD.workspace_id AND h.job_id=OLD.job_id)
 AND OLD.state IN ('completed','failed')
 AND (NEW.state IS NOT OLD.state OR NEW.current_attempt IS NOT OLD.current_attempt
      OR NEW.result_json IS NOT OLD.result_json OR NEW.result_digest IS NOT OLD.result_digest
      OR NEW.failure_category IS NOT OLD.failure_category OR NEW.failure_detail IS NOT OLD.failure_detail
      OR NEW.completed_at IS NOT OLD.completed_at)
BEGIN SELECT RAISE(ABORT, 'pre-upgrade Backend Job cannot resume'); END;
CREATE TRIGGER backend_job_upgrade_no_attempt
BEFORE INSERT ON backend_job_attempts
WHEN EXISTS (SELECT 1 FROM backend_job_upgrade_history h
             WHERE h.workspace_id=NEW.workspace_id AND h.job_id=NEW.job_id)
BEGIN SELECT RAISE(ABORT, 'pre-upgrade Backend Job cannot create attempts'); END;
CREATE TRIGGER backend_job_upgrade_no_attempt_reparent
BEFORE UPDATE OF workspace_id,job_id ON backend_job_attempts
WHEN (NEW.workspace_id IS NOT OLD.workspace_id OR NEW.job_id IS NOT OLD.job_id)
 AND EXISTS (SELECT 1 FROM backend_job_upgrade_history h
             WHERE h.workspace_id=NEW.workspace_id AND h.job_id=NEW.job_id)
BEGIN SELECT RAISE(ABORT, 'pre-upgrade Backend Job cannot adopt attempts'); END;
CREATE TRIGGER backend_job_attempt_upgrade_no_resume
BEFORE UPDATE ON backend_job_attempts
WHEN EXISTS (SELECT 1 FROM backend_job_attempt_upgrade_history h
             WHERE h.workspace_id=OLD.workspace_id AND h.job_id=OLD.job_id AND h.attempt_id=OLD.attempt_id)
 AND OLD.state IN ('completed','failed')
 AND (NEW.workspace_id IS NOT OLD.workspace_id OR NEW.job_id IS NOT OLD.job_id
      OR NEW.attempt_id IS NOT OLD.attempt_id OR NEW.attempt IS NOT OLD.attempt
      OR NEW.input_digest IS NOT OLD.input_digest OR NEW.state IS NOT OLD.state
      OR NEW.runtime_id IS NOT OLD.runtime_id OR NEW.worker_id IS NOT OLD.worker_id
      OR NEW.runtime_run_id IS NOT OLD.runtime_run_id OR NEW.dispatched_at IS NOT OLD.dispatched_at
      OR NEW.deadline_at IS NOT OLD.deadline_at
      OR NEW.result_json IS NOT OLD.result_json OR NEW.result_digest IS NOT OLD.result_digest
      OR NEW.failure_category IS NOT OLD.failure_category OR NEW.failure_detail IS NOT OLD.failure_detail
      OR NEW.completed_at IS NOT OLD.completed_at)
BEGIN SELECT RAISE(ABORT, 'pre-upgrade Backend Job attempt cannot resume'); END;
CREATE TRIGGER backend_job_allocation_precluded_no_insert
BEFORE INSERT ON backend_job_attempts
WHEN NEW.allocation_precluded_at IS NOT NULL
BEGIN SELECT RAISE(ABORT,'allocation preclusion requires the Reserved transition'); END;
CREATE TRIGGER backend_job_allocation_precluded_transition
BEFORE UPDATE ON backend_job_attempts
WHEN NEW.allocation_precluded_at IS NOT OLD.allocation_precluded_at AND (
    OLD.allocation_precluded_at IS NOT NULL
    OR OLD.state != 'reserved' OR NEW.state != 'failed'
    OR OLD.runtime_id IS NOT NULL OR OLD.worker_id IS NOT NULL
    OR OLD.runtime_run_id IS NOT NULL OR OLD.dispatched_at IS NOT NULL
    OR NEW.runtime_id IS NOT NULL OR NEW.worker_id IS NOT NULL
    OR NEW.runtime_run_id IS NOT NULL OR NEW.dispatched_at IS NOT NULL
    OR NEW.allocation_precluded_at IS NOT NEW.completed_at
    OR NEW.failure_category IS NULL OR length(trim(NEW.failure_category))=0
)
BEGIN SELECT RAISE(ABORT,'immutable pre-allocation rejection proof'); END;
CREATE TRIGGER backend_job_allocation_precluded_no_rebind
BEFORE UPDATE ON backend_job_attempts
WHEN OLD.allocation_precluded_at IS NOT NULL AND (
    NEW.state != 'failed' OR NEW.runtime_id IS NOT NULL OR NEW.worker_id IS NOT NULL
    OR NEW.runtime_run_id IS NOT NULL OR NEW.dispatched_at IS NOT NULL
    OR NEW.completed_at IS NOT OLD.completed_at
)
BEGIN SELECT RAISE(ABORT,'allocation was conclusively precluded'); END;
CREATE TRIGGER backend_job_reconciliation_no_legacy
BEFORE INSERT ON backend_job_reconciliation
WHEN EXISTS(SELECT 1 FROM backend_job_upgrade_history h WHERE h.workspace_id=NEW.workspace_id AND h.job_id=NEW.job_id)
BEGIN SELECT RAISE(ABORT,'pre-upgrade Backend Job cannot reconcile'); END;
CREATE TRIGGER backend_job_execution_fence_immutable
BEFORE UPDATE ON backend_job_execution_fences
BEGIN SELECT RAISE(ABORT,'immutable exact execution fence'); END;
CREATE UNIQUE INDEX worker_removal_journal_active
    ON worker_removal_operations(workspace_id,runtime_id,worker_id)
    WHERE journal_removal_json IS NOT NULL AND state IN ('planned','executing','failed');
CREATE TRIGGER worker_removal_journal_immutable
BEFORE UPDATE ON worker_removal_operations
WHEN OLD.journal_removal_json IS NOT NULL AND (
    NEW.journal_removal_json IS NULL
    OR NEW.operation_id!=OLD.operation_id OR NEW.plan_id!=OLD.plan_id
    OR NEW.input_fingerprint!=OLD.input_fingerprint
    OR NEW.workspace_id!=OLD.workspace_id OR NEW.runtime_id!=OLD.runtime_id
    OR NEW.worker_id!=OLD.worker_id
    OR json_extract(NEW.journal_removal_json,'$.binding')
        IS NOT json_extract(OLD.journal_removal_json,'$.binding')
    OR (json_type(OLD.journal_removal_json,'$.members')='array'
        AND json_extract(NEW.journal_removal_json,'$.members')
            IS NOT json_extract(OLD.journal_removal_json,'$.members'))
)
BEGIN SELECT RAISE(ABORT,'Journal removal identity is immutable'); END;
COMMIT;
PRAGMA foreign_keys=ON;
