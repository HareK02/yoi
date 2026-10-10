// Frozen historical Workspace schema conversions. Keep legacy names and SQL immutable.

const WORKER_REGISTRY_PROJECTION_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS worker_registry_observations (
    workspace_id TEXT NOT NULL,
    runtime_id TEXT NOT NULL,
    worker_id TEXT NOT NULL,
    availability TEXT NOT NULL CHECK (availability IN ('observed', 'unavailable')),
    worker_json TEXT,
    connection_generation INTEGER NOT NULL,
    subject_revision INTEGER NOT NULL,
    snapshot_revision INTEGER NOT NULL,
    projection_revision INTEGER NOT NULL,
    observed_at TEXT NOT NULL,
    PRIMARY KEY (workspace_id, runtime_id, worker_id),
    FOREIGN KEY (workspace_id, worker_id)
        REFERENCES worker_registry(workspace_id, worker_id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS worker_registry_projection_cursors (
    workspace_id TEXT NOT NULL,
    runtime_id TEXT NOT NULL,
    connection_generation INTEGER NOT NULL,
    snapshot_revision INTEGER NOT NULL,
    PRIMARY KEY (workspace_id, runtime_id)
);
CREATE TABLE IF NOT EXISTS worker_registry_projection_revisions (
    workspace_id TEXT PRIMARY KEY,
    revision INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS worker_registry_projection_removals (
    workspace_id TEXT NOT NULL,
    runtime_id TEXT NOT NULL,
    worker_id TEXT NOT NULL,
    projection_revision INTEGER NOT NULL,
    PRIMARY KEY (workspace_id, runtime_id, worker_id)
);
CREATE TABLE IF NOT EXISTS worker_registry_projection_diagnostics (
    diagnostic_id INTEGER PRIMARY KEY AUTOINCREMENT,
    workspace_id TEXT NOT NULL,
    runtime_id TEXT NOT NULL,
    worker_id TEXT NOT NULL,
    reason TEXT NOT NULL,
    observed_at TEXT NOT NULL
);
"#;

fn migrate_workspace_drive_grants_v83_to_v84(conn: &Connection) -> Result<()> {
    if current_schema_version(conn)? != 83 {
        return Err(Error::Store(
            "Drive grants migration requires schema 83".into(),
        ));
    }
    let tx = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Exclusive)?;
    tx.execute_batch(include_str!("workspace_drive_grants.sql"))?;
    tx.execute(
        "INSERT INTO __yoi_schema_migrations(version,name) VALUES(?1,?2)",
        params![84_i64, WORKSPACE_DRIVE_GRANTS_MIGRATION_NAME],
    )?;
    tx.commit()?;
    Ok(())
}

fn migrate_worker_restore_intents_v79_to_v80(conn: &Connection) -> Result<()> {
    let tx = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Exclusive)?;
    tx.execute_batch("CREATE TABLE worker_restore_intents (
        workspace_id TEXT NOT NULL, runtime_id TEXT NOT NULL, worker_id TEXT NOT NULL,
        domain_key TEXT NOT NULL, request_id TEXT NOT NULL, expected_token TEXT NOT NULL,
        settled INTEGER NOT NULL CHECK(settled IN (0,1)),
        PRIMARY KEY(workspace_id,runtime_id,worker_id,request_id),
        FOREIGN KEY(workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE
    );
    CREATE UNIQUE INDEX worker_restore_intent_pending_domain ON worker_restore_intents(workspace_id,runtime_id,worker_id,domain_key) WHERE settled=0;")?;
    tx.execute(
        "INSERT INTO __yoi_schema_migrations(version,name) VALUES(?1,?2)",
        params![80_i64, WORKER_RESTORE_INTENTS_MIGRATION_NAME],
    )?;
    tx.commit()?;
    Ok(())
}

fn migrate_workspace_runtime_bindings_v50_to_v51(conn: &Connection) -> Result<()> {
    let tx = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Exclusive)?;
    let legacy_columns = table_columns(&tx, "trusted_runtime_records")?
        .into_iter()
        .collect::<BTreeSet<_>>();
    let expected_columns = [
        "runtime_id",
        "display_name",
        "base_url",
        "public_key",
        "created_at",
        "updated_at",
        "revoked_at",
        "workspace_id",
    ]
    .into_iter()
    .map(str::to_string)
    .collect::<BTreeSet<_>>();
    if legacy_columns != expected_columns {
        return Err(Error::Store(format!(
            "schema-50 trusted_runtime_records columns are not canonical"
        )));
    }

    let all_workspace_ids = {
        let mut stmt = tx.prepare("SELECT workspace_id FROM workspaces ORDER BY workspace_id")?;
        stmt.query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };

    let mut bindings = Vec::new();
    {
        let mut stmt = tx.prepare(
            r#"SELECT runtime_id, workspace_id, display_name, base_url, public_key,
                      created_at, updated_at, revoked_at
               FROM trusted_runtime_records ORDER BY runtime_id"#,
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, Option<String>>(7)?,
            ))
        })?;
        for row in rows {
            let (
                runtime_id,
                workspace_id,
                display_name,
                base_url,
                public_key,
                created_at,
                updated_at,
                revoked_at,
            ) = row?;
            let workspace_ids = if runtime_id == crate::hosts::EMBEDDED_RUNTIME_ID {
                if all_workspace_ids.is_empty() {
                    return Err(Error::Store(
                        "embedded Runtime migration requires at least one registered Workspace"
                            .to_string(),
                    ));
                }
                all_workspace_ids.clone()
            } else if let Some(workspace_id) = workspace_id.filter(|value| !value.trim().is_empty())
            {
                vec![workspace_id]
            } else {
                continue;
            };
            let (public_key, fingerprint) = normalize_runtime_public_key(&public_key)?;
            for workspace_id in workspace_ids {
                let workspace_exists = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM workspaces WHERE workspace_id = ?1)",
                    params![workspace_id],
                    |row| row.get::<_, i64>(0),
                )? != 0;
                if !workspace_exists {
                    return Err(Error::Store(format!(
                        "Runtime `{runtime_id}` references unknown Workspace `{workspace_id}`"
                    )));
                }
                bindings.push(WorkspaceRuntimeBindingV51 {
                    workspace_id,
                    runtime_id: runtime_id.clone(),
                    display_name: display_name.clone(),
                    base_url: base_url.clone(),
                    public_key: Some(public_key.clone()),
                    public_key_fingerprint: Some(fingerprint.clone()),
                    created_at: created_at.clone(),
                    updated_at: updated_at.clone(),
                    revoked_at: revoked_at.clone(),
                });
            }
        }
    }

    let mut binding_keys = HashSet::new();
    let mut trust_keys = HashSet::new();
    for binding in &bindings {
        if !binding_keys.insert((binding.workspace_id.clone(), binding.runtime_id.clone())) {
            return Err(Error::Store(format!(
                "duplicate Runtime binding `{}/{}` in schema-50",
                binding.workspace_id, binding.runtime_id
            )));
        }
        let fingerprint = binding
            .public_key_fingerprint
            .clone()
            .expect("normalized key");
        if !trust_keys.insert((binding.workspace_id.clone(), fingerprint.clone())) {
            return Err(Error::Store(format!(
                "duplicate Runtime trust fingerprint `{fingerprint}` in Workspace `{}`",
                binding.workspace_id
            )));
        }
    }

    let mut consumed_jtis = Vec::new();
    {
        let runtime_workspaces = bindings.iter().fold(
            HashMap::<&str, Vec<&str>>::new(),
            |mut workspaces, binding| {
                workspaces
                    .entry(binding.runtime_id.as_str())
                    .or_default()
                    .push(binding.workspace_id.as_str());
                workspaces
            },
        );
        let mut stmt = tx.prepare(
            "SELECT runtime_id, jti, expires_at, consumed_at FROM worker_mutation_source_proof_jtis",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;
        for row in rows {
            let (runtime_id, jti, expires_at, consumed_at) = row?;
            let Some(workspace_ids) = runtime_workspaces.get(runtime_id.as_str()) else {
                continue;
            };
            for workspace_id in workspace_ids {
                consumed_jtis.push((
                    (*workspace_id).to_string(),
                    runtime_id.clone(),
                    jti.clone(),
                    expires_at,
                    consumed_at.clone(),
                ));
            }
        }
    }

    tx.execute_batch(
        r#"
        ALTER TABLE worker_mutation_source_proof_jtis
            RENAME TO worker_mutation_source_proof_jtis_v50;
        ALTER TABLE trusted_runtime_records
            RENAME TO trusted_runtime_records_v50;

        CREATE TABLE workspace_runtime_bindings (
            workspace_id TEXT NOT NULL,
            runtime_id TEXT NOT NULL,
            display_name TEXT NOT NULL,
            base_url TEXT NOT NULL,
            public_key TEXT,
            public_key_fingerprint TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            revoked_at TEXT,
            PRIMARY KEY (workspace_id, runtime_id),
            UNIQUE (workspace_id, public_key_fingerprint),
            CHECK ((public_key IS NULL) = (public_key_fingerprint IS NULL)),
            FOREIGN KEY(workspace_id) REFERENCES workspaces(workspace_id) ON DELETE RESTRICT
        );
        CREATE INDEX idx_workspace_runtime_bindings_workspace
            ON workspace_runtime_bindings(workspace_id, revoked_at, runtime_id);
        CREATE TABLE worker_mutation_source_proof_jtis (
            workspace_id TEXT NOT NULL,
            runtime_id TEXT NOT NULL,
            jti TEXT NOT NULL,
            expires_at INTEGER NOT NULL,
            consumed_at TEXT NOT NULL,
            PRIMARY KEY (workspace_id, runtime_id, jti)
        );
        "#,
    )?;
    for binding in bindings {
        tx.execute(
            r#"INSERT INTO workspace_runtime_bindings (
                   workspace_id, runtime_id, display_name, base_url, public_key,
                   public_key_fingerprint, created_at, updated_at, revoked_at
               ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)"#,
            params![
                binding.workspace_id,
                binding.runtime_id,
                binding.display_name,
                binding.base_url,
                binding.public_key,
                binding.public_key_fingerprint,
                binding.created_at,
                binding.updated_at,
                binding.revoked_at,
            ],
        )?;
    }
    for (workspace_id, runtime_id, jti, expires_at, consumed_at) in consumed_jtis {
        tx.execute(
            "INSERT INTO worker_mutation_source_proof_jtis (
                workspace_id, runtime_id, jti, expires_at, consumed_at
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![workspace_id, runtime_id, jti, expires_at, consumed_at],
        )?;
    }
    tx.execute_batch(
        "DROP TABLE worker_mutation_source_proof_jtis_v50;
         DROP TABLE trusted_runtime_records_v50;",
    )?;
    let foreign_key_failures =
        tx.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
            row.get::<_, i64>(0)
        })?;
    if foreign_key_failures != 0 {
        return Err(Error::Store(format!(
            "schema-51 migration produced {foreign_key_failures} foreign-key violation(s)"
        )));
    }
    tx.execute(
        "INSERT INTO __yoi_schema_migrations (version, name) VALUES (?1, ?2)",
        params![51, WORKSPACE_RUNTIME_BINDINGS_MIGRATION_NAME],
    )?;
    tx.commit()?;
    Ok(())
}

fn migrate_workspace_runtime_bindings_v51_to_v52(conn: &Connection) -> Result<()> {
    let current = current_schema_version(conn)?;
    if current != 51 {
        return Err(Error::Store(format!(
            "expected schema version 51 before {RUNTIME_BINDING_AUDIT_MIGRATION_NAME} migration, found {current}"
        )));
    }

    let tx = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Exclusive)?;
    tx.execute_batch(
        r#"
        ALTER TABLE workspace_runtime_bindings
            ADD COLUMN binding_revision INTEGER NOT NULL DEFAULT 1 CHECK (binding_revision > 0);
        CREATE TABLE workspace_runtime_binding_audit (
            workspace_id TEXT NOT NULL,
            runtime_id TEXT NOT NULL,
            actor_account_id TEXT NOT NULL,
            action TEXT NOT NULL CHECK (action IN ('created', 'replaced', 'reactivated', 'revoked')),
            old_fingerprint TEXT,
            new_fingerprint TEXT,
            binding_revision INTEGER NOT NULL CHECK (binding_revision > 0),
            at TEXT NOT NULL,
            PRIMARY KEY (workspace_id, runtime_id, binding_revision),
            FOREIGN KEY(workspace_id, runtime_id)
                REFERENCES workspace_runtime_bindings(workspace_id, runtime_id) ON DELETE RESTRICT,
            FOREIGN KEY(actor_account_id) REFERENCES accounts(account_id) ON DELETE RESTRICT
        );
        CREATE INDEX idx_workspace_runtime_binding_audit_recent
            ON workspace_runtime_binding_audit(workspace_id, runtime_id, binding_revision DESC);
        "#,
    )?;
    verify_workspace_runtime_binding_schema(&tx)?;
    tx.execute(
        "INSERT INTO __yoi_schema_migrations (version, name) VALUES (?1, ?2)",
        params![52, RUNTIME_BINDING_AUDIT_MIGRATION_NAME],
    )?;
    tx.commit()?;
    Ok(())
}

fn migrate_workspace_deletion_v52_to_v53(conn: &Connection) -> Result<()> {
    let current = current_schema_version(conn)?;
    if current != 52 {
        return Err(Error::Store(format!(
            "expected schema version 52 before {WORKSPACE_DELETION_MIGRATION_NAME} migration, found {current}"
        )));
    }

    let tx = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Exclusive)?;
    tx.execute_batch(
        r#"
        CREATE TABLE worker_create_reservations_v53 (
            workspace_id TEXT NOT NULL,
            allocation_key TEXT NOT NULL,
            worker_id TEXT NOT NULL,
            runtime_id TEXT NOT NULL,
            create_fingerprint TEXT NOT NULL,
            state TEXT NOT NULL CHECK (state IN ('reserved', 'created', 'removed')),
            request_fingerprint TEXT,
            memory_settings_revision INTEGER,
            memory_language TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            PRIMARY KEY (workspace_id, allocation_key),
            UNIQUE (workspace_id, worker_id),
            FOREIGN KEY(workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
            CHECK (
                (request_fingerprint IS NULL AND memory_settings_revision IS NULL AND memory_language IS NULL)
                OR
                (request_fingerprint IS NOT NULL AND memory_settings_revision IS NOT NULL AND memory_settings_revision > 0 AND memory_language IS NOT NULL AND length(trim(memory_language)) > 0)
            )
        );
        INSERT INTO worker_create_reservations_v53 (
            workspace_id, allocation_key, worker_id, runtime_id, create_fingerprint,
            state, request_fingerprint, memory_settings_revision, memory_language,
            created_at, updated_at
        )
        SELECT workspace_id, allocation_key, worker_id, runtime_id, create_fingerprint,
               state,
               CASE WHEN request_fingerprint IS NOT NULL
                          AND memory_settings_revision IS NOT NULL
                          AND memory_settings_revision > 0
                          AND memory_language IS NOT NULL
                          AND length(trim(memory_language)) > 0
                    THEN request_fingerprint ELSE NULL END,
               CASE WHEN request_fingerprint IS NOT NULL
                          AND memory_settings_revision IS NOT NULL
                          AND memory_settings_revision > 0
                          AND memory_language IS NOT NULL
                          AND length(trim(memory_language)) > 0
                    THEN memory_settings_revision ELSE NULL END,
               CASE WHEN request_fingerprint IS NOT NULL
                          AND memory_settings_revision IS NOT NULL
                          AND memory_settings_revision > 0
                          AND memory_language IS NOT NULL
                          AND length(trim(memory_language)) > 0
                    THEN memory_language ELSE NULL END,
               created_at, updated_at
        FROM worker_create_reservations;
        DROP TABLE worker_create_reservations;
        ALTER TABLE worker_create_reservations_v53 RENAME TO worker_create_reservations;

        CREATE TABLE workspace_deletion_operations (
            operation_id TEXT PRIMARY KEY,
            request_fingerprint TEXT NOT NULL,
            workspace_id TEXT NOT NULL,
            workspace_display_name TEXT NOT NULL,
            workspace_revision TEXT NOT NULL,
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
        "#,
    )?;
    verify_workspace_deletion_schema(&tx)?;
    tx.execute(
        "INSERT INTO __yoi_schema_migrations (version, name) VALUES (?1, ?2)",
        params![53_i64, WORKSPACE_DELETION_MIGRATION_NAME],
    )?;
    tx.commit()?;
    Ok(())
}

fn migrate_workspace_signing_identity_v53_to_v54(conn: &Connection) -> Result<()> {
    let current = current_schema_version(conn)?;
    if current != 53 {
        return Err(Error::Store(format!(
            "expected schema version 53 before {WORKSPACE_SIGNING_IDENTITY_MIGRATION_NAME} migration, found {current}"
        )));
    }

    let tx = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Exclusive)?;
    tx.execute_batch(
        r#"
        CREATE TABLE workspace_signing_identities (
            workspace_id TEXT PRIMARY KEY,
            key_id TEXT NOT NULL UNIQUE,
            algorithm TEXT NOT NULL CHECK (algorithm = 'ed25519'),
            public_key TEXT,
            public_key_fingerprint TEXT,
            private_material_ref TEXT NOT NULL UNIQUE,
            revision INTEGER NOT NULL CHECK (revision >= 1),
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
            revision INTEGER NOT NULL CHECK (revision >= 1),
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
            revision INTEGER NOT NULL CHECK (revision >= 1),
            public_key_fingerprint TEXT NOT NULL,
            actor_account_id TEXT NOT NULL,
            created_at TEXT NOT NULL,
            FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE
        );
        CREATE INDEX workspace_signing_identity_audit_workspace_idx
            ON workspace_signing_identity_audit(workspace_id, created_at DESC);
        INSERT INTO workspace_signing_identities (
            workspace_id, key_id, algorithm, public_key, public_key_fingerprint,
            private_material_ref, revision, state, created_at, provisioned_at, updated_at
        )
        SELECT workspace_id,
               'WK-' || lower(hex(randomblob(16))),
               'ed25519', NULL, NULL,
               'workspace-signing/' || workspace_id || '/ed25519-v1',
               1, 'pending_provisioning', created_at, NULL, updated_at
        FROM workspaces;
        "#,
    )?;
    verify_workspace_signing_identity_schema(&tx)?;
    tx.execute(
        "INSERT INTO __yoi_schema_migrations (version, name) VALUES (?1, ?2)",
        params![54_i64, WORKSPACE_SIGNING_IDENTITY_MIGRATION_NAME],
    )?;
    tx.commit()?;
    Ok(())
}

fn migrate_workspace_runtime_binding_state_v54_to_v55(conn: &Connection) -> Result<()> {
    let current = current_schema_version(conn)?;
    if current != 54 {
        return Err(Error::Store(format!(
            "expected schema version 54 before {WORKSPACE_RUNTIME_BINDING_STATE_MIGRATION_NAME} migration, found {current}"
        )));
    }

    let tx = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Exclusive)?;
    let columns = table_columns(&tx, "workspace_runtime_bindings")?;
    let lifecycle_columns = [
        "state",
        "authentication_mode",
        "workspace_key_id",
        "workspace_key_generation",
    ];
    let present = lifecycle_columns
        .iter()
        .filter(|column| columns.iter().any(|existing| existing == **column))
        .count();
    if present == 0 {
        tx.execute_batch(
            r#"
            CREATE TABLE workspace_runtime_bindings_v55 (
                workspace_id TEXT NOT NULL,
                runtime_id TEXT NOT NULL,
                display_name TEXT NOT NULL,
                base_url TEXT NOT NULL,
                public_key TEXT NOT NULL,
                public_key_fingerprint TEXT NOT NULL,
                binding_revision INTEGER NOT NULL DEFAULT 1 CHECK (binding_revision > 0),
                state TEXT NOT NULL CHECK (state IN ('configured', 'verified', 'revoked')),
                authentication_mode TEXT NOT NULL CHECK (authentication_mode IN ('legacy_server_issuer', 'workspace_identity')),
                workspace_key_id TEXT,
                workspace_key_generation INTEGER CHECK (workspace_key_generation > 0),
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                revoked_at TEXT,
                PRIMARY KEY (workspace_id, runtime_id),
                UNIQUE (workspace_id, public_key_fingerprint),
                FOREIGN KEY(workspace_id) REFERENCES workspaces(workspace_id) ON DELETE RESTRICT,
                CHECK (
                    (authentication_mode = 'legacy_server_issuer' AND workspace_key_id IS NULL AND workspace_key_generation IS NULL)
                    OR
                    (authentication_mode = 'workspace_identity' AND workspace_key_id IS NOT NULL AND workspace_key_generation IS NOT NULL)
                ),
                CHECK (
                    (state = 'revoked' AND revoked_at IS NOT NULL)
                    OR
                    (state != 'revoked' AND revoked_at IS NULL)
                )
            );
            INSERT INTO workspace_runtime_bindings_v55 (
                workspace_id, runtime_id, display_name, base_url, public_key,
                public_key_fingerprint, binding_revision, state, authentication_mode,
                workspace_key_id, workspace_key_generation, created_at, updated_at, revoked_at
            )
            SELECT workspace_id, runtime_id, display_name, base_url, public_key,
                   public_key_fingerprint, binding_revision,
                   CASE WHEN revoked_at IS NULL THEN 'verified' ELSE 'revoked' END,
                   'legacy_server_issuer', NULL, NULL, created_at, updated_at, revoked_at
            FROM workspace_runtime_bindings;
            DROP TABLE workspace_runtime_bindings;
            ALTER TABLE workspace_runtime_bindings_v55 RENAME TO workspace_runtime_bindings;
            CREATE INDEX idx_workspace_runtime_bindings_workspace
                ON workspace_runtime_bindings(workspace_id, revoked_at, runtime_id);
            "#,
        )?;
    } else if present != lifecycle_columns.len() {
        return Err(Error::Store(
            "workspace_runtime_bindings has a partially applied lifecycle schema".to_string(),
        ));
    }
    verify_workspace_runtime_binding_schema(&tx)?;
    tx.execute(
        "INSERT INTO __yoi_schema_migrations (version, name) VALUES (?1, ?2)",
        params![55_i64, WORKSPACE_RUNTIME_BINDING_STATE_MIGRATION_NAME],
    )?;
    tx.commit()?;
    Ok(())
}

fn migrate_workspace_runtime_verification_v55_to_v56(conn: &Connection) -> Result<()> {
    let current = current_schema_version(conn)?;
    if current != 55 {
        return Err(Error::Store(format!(
            "expected schema version 55 before {WORKSPACE_RUNTIME_VERIFICATION_MIGRATION_NAME} migration, found {current}"
        )));
    }
    let tx = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Exclusive)?;
    tx.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS workspace_runtime_verifications (
            workspace_id TEXT NOT NULL,
            runtime_id TEXT NOT NULL,
            binding_revision INTEGER NOT NULL CHECK(binding_revision > 0),
            workspace_key_id TEXT NOT NULL,
            workspace_identity_revision INTEGER NOT NULL CHECK(workspace_identity_revision > 0),
            workspace_trust_generation INTEGER NOT NULL CHECK(workspace_trust_generation > 0),
            runtime_public_key_fingerprint TEXT NOT NULL,
            runtime_identity_revision INTEGER NOT NULL CHECK(runtime_identity_revision > 0),
            challenge_id TEXT NOT NULL,
            state TEXT NOT NULL CHECK(state IN ('pending', 'verified', 'failed')),
            last_outcome TEXT NOT NULL,
            verified_at TEXT,
            checked_at TEXT NOT NULL,
            PRIMARY KEY(workspace_id, runtime_id),
            FOREIGN KEY(workspace_id, runtime_id)
                REFERENCES workspace_runtime_bindings(workspace_id, runtime_id)
                ON DELETE CASCADE,
            CHECK((state = 'verified' AND verified_at IS NOT NULL)
               OR (state != 'verified' AND verified_at IS NULL))
        );
        CREATE INDEX IF NOT EXISTS workspace_runtime_verifications_state_idx
            ON workspace_runtime_verifications(workspace_id, state, checked_at DESC);
        "#,
    )?;
    verify_workspace_runtime_verification_schema(&tx)?;
    tx.execute(
        "INSERT INTO __yoi_schema_migrations (version, name) VALUES (?1, ?2)",
        params![56_i64, WORKSPACE_RUNTIME_VERIFICATION_MIGRATION_NAME],
    )?;
    tx.commit()?;
    Ok(())
}

fn migrate_legacy_external_runtime_bindings_v56_to_v57(conn: &Connection) -> Result<()> {
    let current = current_schema_version(conn)?;
    if current != 56 {
        return Err(Error::Store(format!(
            "expected schema version 56 before {LEGACY_EXTERNAL_RUNTIME_BINDING_CUTOVER_MIGRATION_NAME} migration, found {current}"
        )));
    }
    let tx = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Exclusive)?;
    let missing_workspace_identity_count = tx.query_row(
        "SELECT COUNT(*)
         FROM workspace_runtime_bindings binding
         LEFT JOIN workspace_signing_identities identity
           ON identity.workspace_id = binding.workspace_id
         WHERE binding.authentication_mode = 'legacy_server_issuer'
           AND binding.runtime_id <> ?1
           AND identity.workspace_id IS NULL",
        params![crate::hosts::EMBEDDED_RUNTIME_ID],
        |row| row.get::<_, i64>(0),
    )?;
    if missing_workspace_identity_count != 0 {
        return Err(Error::Store(
            "legacy external Runtime binding migration requires Workspace signing identity metadata"
                .to_string(),
        ));
    }
    tx.execute(
        "DELETE FROM workspace_runtime_verifications
         WHERE EXISTS (
             SELECT 1 FROM workspace_runtime_bindings binding
             WHERE binding.workspace_id = workspace_runtime_verifications.workspace_id
               AND binding.runtime_id = workspace_runtime_verifications.runtime_id
               AND binding.authentication_mode = 'legacy_server_issuer'
               AND binding.runtime_id <> ?1
         )",
        params![crate::hosts::EMBEDDED_RUNTIME_ID],
    )?;
    tx.execute(
        "UPDATE workspace_runtime_bindings
         SET state = CASE WHEN state = 'verified' THEN 'configured' ELSE state END,
             authentication_mode = 'workspace_identity',
             workspace_key_id = (
                 SELECT identity.key_id FROM workspace_signing_identities identity
                 WHERE identity.workspace_id = workspace_runtime_bindings.workspace_id
             ),
             workspace_key_generation = (
                 SELECT identity.revision FROM workspace_signing_identities identity
                 WHERE identity.workspace_id = workspace_runtime_bindings.workspace_id
             )
         WHERE authentication_mode = 'legacy_server_issuer'
           AND runtime_id <> ?1",
        params![crate::hosts::EMBEDDED_RUNTIME_ID],
    )?;
    let remaining_external_legacy_bindings = tx.query_row(
        "SELECT COUNT(*) FROM workspace_runtime_bindings
         WHERE authentication_mode = 'legacy_server_issuer'
           AND runtime_id <> ?1",
        params![crate::hosts::EMBEDDED_RUNTIME_ID],
        |row| row.get::<_, i64>(0),
    )?;
    if remaining_external_legacy_bindings != 0 {
        return Err(Error::Store(
            "legacy Server-issued external Runtime bindings remain after schema-57 migration"
                .to_string(),
        ));
    }
    verify_workspace_runtime_binding_schema(&tx)?;
    verify_workspace_runtime_verification_schema(&tx)?;
    let foreign_key_violations =
        tx.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
            row.get::<_, i64>(0)
        })?;
    if foreign_key_violations != 0 {
        return Err(Error::Store(format!(
            "schema-57 Runtime binding migration left {foreign_key_violations} foreign-key violation(s)"
        )));
    }
    tx.execute(
        "INSERT INTO __yoi_schema_migrations (version, name) VALUES (?1, ?2)",
        params![
            57_i64,
            LEGACY_EXTERNAL_RUNTIME_BINDING_CUTOVER_MIGRATION_NAME
        ],
    )?;
    tx.commit()?;
    Ok(())
}

fn migrate_workdir_cache_generation_v57_to_v58(conn: &Connection) -> Result<()> {
    let current = current_schema_version(conn)?;
    if current != 57 {
        return Err(Error::Store(format!(
            "expected schema version 57 before {REMOVE_WORKDIR_CACHE_GENERATION_MIGRATION_NAME} migration, found {current}"
        )));
    }
    let tx = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Exclusive)?;
    if table_columns(&tx, "workdir_create_operations")?.contains(&"cache_generation".to_string()) {
        tx.execute(
            "ALTER TABLE workdir_create_operations DROP COLUMN cache_generation",
            [],
        )?;
    }
    if table_columns(&tx, "workdir_create_operations")?.contains(&"cache_generation".to_string()) {
        return Err(Error::Store(
            "obsolete Workdir cache generation remains after schema-58 migration".to_string(),
        ));
    }
    tx.execute(
        "INSERT INTO __yoi_schema_migrations (version, name) VALUES (?1, ?2)",
        params![58_i64, REMOVE_WORKDIR_CACHE_GENERATION_MIGRATION_NAME],
    )?;
    tx.commit()?;
    Ok(())
}

fn migrate_runtime_removal_operations_v59_to_v60(conn: &Connection) -> Result<()> {
    let tx = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Exclusive)?;
    tx.execute_batch(
        r#"CREATE TABLE runtime_removal_operations (
             operation_id TEXT PRIMARY KEY,
             workspace_id TEXT NOT NULL,
             runtime_id TEXT NOT NULL,
             request_fingerprint TEXT NOT NULL,
             expected_binding_revision INTEGER NOT NULL,
             config_revision INTEGER NOT NULL,
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
         END;"#,
    )?;
    tx.execute_batch(
        r#"
        CREATE TRIGGER worker_registry_insert_blocked_by_runtime_removal
        BEFORE INSERT ON worker_registry FOR EACH ROW
        WHEN EXISTS (SELECT 1 FROM runtime_removal_operations operation WHERE operation.runtime_id = NEW.runtime_id AND operation.state IN ('pending', 'cleanup_pending'))
        BEGIN SELECT RAISE(ABORT, 'runtime_removal_in_progress'); END;
        CREATE TRIGGER worker_registry_update_blocked_by_runtime_removal
        BEFORE UPDATE ON worker_registry FOR EACH ROW
        WHEN EXISTS (SELECT 1 FROM runtime_removal_operations operation WHERE operation.runtime_id = NEW.runtime_id AND operation.state IN ('pending', 'cleanup_pending'))
        BEGIN SELECT RAISE(ABORT, 'runtime_removal_in_progress'); END;
        CREATE TRIGGER workdir_registry_insert_blocked_by_runtime_removal
        BEFORE INSERT ON workdir_registry FOR EACH ROW
        WHEN EXISTS (SELECT 1 FROM runtime_removal_operations operation WHERE operation.runtime_id = NEW.runtime_id AND operation.state IN ('pending', 'cleanup_pending'))
        BEGIN SELECT RAISE(ABORT, 'runtime_removal_in_progress'); END;
        CREATE TRIGGER workdir_registry_update_blocked_by_runtime_removal
        BEFORE UPDATE ON workdir_registry FOR EACH ROW
        WHEN EXISTS (SELECT 1 FROM runtime_removal_operations operation WHERE operation.runtime_id = NEW.runtime_id AND operation.state IN ('pending', 'cleanup_pending'))
        BEGIN SELECT RAISE(ABORT, 'runtime_removal_in_progress'); END;
        CREATE TRIGGER worker_assignment_insert_blocked_by_runtime_removal
        BEFORE INSERT ON ticket_current_worker_assignments FOR EACH ROW
        WHEN EXISTS (SELECT 1 FROM runtime_removal_operations operation WHERE operation.runtime_id = NEW.runtime_id AND operation.state IN ('pending', 'cleanup_pending'))
        BEGIN SELECT RAISE(ABORT, 'runtime_removal_in_progress'); END;
        CREATE TRIGGER worker_assignment_update_blocked_by_runtime_removal
        BEFORE UPDATE ON ticket_current_worker_assignments FOR EACH ROW
        WHEN EXISTS (SELECT 1 FROM runtime_removal_operations operation WHERE operation.runtime_id = NEW.runtime_id AND operation.state IN ('pending', 'cleanup_pending'))
        BEGIN SELECT RAISE(ABORT, 'runtime_removal_in_progress'); END;
        CREATE TRIGGER workdir_attachment_insert_blocked_by_runtime_removal
        BEFORE INSERT ON worker_workdir_links FOR EACH ROW
        WHEN EXISTS (SELECT 1 FROM runtime_removal_operations operation WHERE operation.runtime_id = NEW.runtime_id AND operation.state IN ('pending', 'cleanup_pending'))
        BEGIN SELECT RAISE(ABORT, 'runtime_removal_in_progress'); END;
        CREATE TRIGGER workdir_attachment_update_blocked_by_runtime_removal
        BEFORE UPDATE ON worker_workdir_links FOR EACH ROW
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
        CREATE TRIGGER workdir_create_insert_blocked_by_runtime_removal
        BEFORE INSERT ON workdir_create_operations FOR EACH ROW
        WHEN EXISTS (SELECT 1 FROM runtime_removal_operations operation WHERE operation.runtime_id = NEW.resolved_runtime_id AND operation.state IN ('pending', 'cleanup_pending'))
        BEGIN SELECT RAISE(ABORT, 'runtime_removal_in_progress'); END;
        CREATE TRIGGER workdir_create_update_blocked_by_runtime_removal
        BEFORE UPDATE ON workdir_create_operations FOR EACH ROW
        WHEN EXISTS (SELECT 1 FROM runtime_removal_operations operation WHERE operation.runtime_id = NEW.resolved_runtime_id AND operation.state IN ('pending', 'cleanup_pending'))
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
        "#,
    )?;
    tx.execute(
        "INSERT INTO __yoi_schema_migrations (version, name) VALUES (?1, ?2)",
        params![60, RUNTIME_REMOVAL_OPERATION_MIGRATION_NAME],
    )?;
    tx.commit()?;
    Ok(())
}

fn migrate_worker_run_generation_v60_to_v61(conn: &Connection) -> Result<()> {
    let tx = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Exclusive)?;
    tx.execute_batch("ALTER TABLE worker_removal_operations DROP COLUMN run_generation;")?;
    tx.execute(
        "INSERT INTO __yoi_schema_migrations (version, name) VALUES (?1, ?2)",
        params![61, REMOVE_WORKER_RUN_GENERATION_MIGRATION_NAME],
    )?;
    tx.commit()?;
    Ok(())
}

fn migrate_worker_registry_projection_v61_to_v62(conn: &Connection) -> Result<()> {
    let tx = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Exclusive)?;
    tx.execute_batch(WORKER_REGISTRY_PROJECTION_SCHEMA)?;
    tx.execute(
        "INSERT INTO __yoi_schema_migrations (version, name) VALUES (?1, ?2)",
        params![62, WORKER_REGISTRY_PROJECTION_MIGRATION_NAME],
    )?;
    tx.commit()?;
    Ok(())
}

fn migrate_multi_workdir_attachments_v62_to_v63(conn: &Connection) -> Result<()> {
    let tx = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Exclusive)?;
    if !column_exists(&tx, "workdir_registry", "display_name")? {
        tx.execute_batch("ALTER TABLE workdir_registry ADD COLUMN display_name TEXT;")?;
    }
    if column_exists(&tx, "worker_workdir_links", "role")?
        && !column_exists(&tx, "worker_workdir_links", "alias")?
    {
        tx.execute_batch("ALTER TABLE worker_workdir_links RENAME COLUMN role TO alias;")?;
    }
    tx.execute_batch(
        "DROP INDEX IF EXISTS worker_workdir_links_active_worker_unique;
         CREATE UNIQUE INDEX IF NOT EXISTS worker_workdir_links_active_alias_unique
             ON worker_workdir_links(workspace_id, worker_id, alias)
             WHERE unlinked_at IS NULL;",
    )?;
    tx.execute(
        "INSERT INTO __yoi_schema_migrations (version, name) VALUES (?1, ?2)",
        params![63, MULTI_WORKDIR_ATTACHMENTS_MIGRATION_NAME],
    )?;
    tx.commit()?;
    Ok(())
}

fn migrate_external_workdir_grants_v63_to_v64(conn: &Connection) -> Result<()> {
    let tx = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Exclusive)?;
    // Migration-chain tests and repaired databases may carry the canonical v64
    // shape while the durable history still stops at v63. In that case only
    // advance the marker; rebuilding would discard External grant rows and
    // collide with the already-created table.
    if column_exists(&tx, "workdir_registry", "source_kind")?
        && table_exists(&tx, "external_workdir_grants")?
    {
        tx.execute(
            "INSERT INTO __yoi_schema_migrations (version, name) VALUES (?1, ?2)",
            params![64, EXTERNAL_WORKDIR_GRANTS_MIGRATION_NAME],
        )?;
        tx.commit()?;
        return Ok(());
    }
    tx.execute_batch(
        r#"
        PRAGMA defer_foreign_keys = ON;
        DROP TRIGGER IF EXISTS workdir_registry_insert_blocked_by_runtime_removal;
        DROP TRIGGER IF EXISTS workdir_registry_update_blocked_by_runtime_removal;
        CREATE TEMP TABLE worker_workdir_attachment_reservations_v64 AS
            SELECT workspace_id, workdir_id, reservation_id, reserved_at
            FROM worker_workdir_attachment_reservations;
        CREATE TEMP TABLE worker_workdir_links_v64 AS
            SELECT workspace_id, runtime_id, worker_id, workdir_id, alias, linked_at, unlinked_at
            FROM worker_workdir_links;
        DROP TABLE worker_workdir_attachment_reservations;
        DROP TABLE worker_workdir_links;
        CREATE TABLE external_workdir_grants (
            grant_id TEXT NOT NULL,
            workspace_id TEXT NOT NULL,
            workdir_id TEXT NOT NULL,
            provider_instance_id TEXT NOT NULL,
            display_name TEXT NOT NULL,
            permissions TEXT NOT NULL CHECK (permissions = 'read_only'),
            created_by TEXT NOT NULL,
            created_at TEXT NOT NULL,
            expires_at TEXT NOT NULL,
            generation INTEGER NOT NULL CHECK (generation > 0),
            status TEXT NOT NULL CHECK (status IN ('pending', 'online', 'offline', 'revoked', 'expired')),
            updated_at TEXT NOT NULL,
            PRIMARY KEY (workspace_id, grant_id),
            UNIQUE (workspace_id, workdir_id),
            FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
            FOREIGN KEY (workspace_id, workdir_id) REFERENCES workdir_registry_v64(workspace_id, workdir_id) ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
        );
        CREATE TABLE workdir_registry_v64 (
            workspace_id TEXT NOT NULL,
            workdir_id TEXT NOT NULL,
            display_name TEXT,
            source_kind TEXT NOT NULL CHECK (source_kind IN ('repository', 'external_grant')),
            runtime_id TEXT,
            repository_id TEXT,
            external_grant_id TEXT,
            creation_selector TEXT,
            creation_ref TEXT,
            creation_tree TEXT,
            current_selector TEXT,
            current_ref TEXT,
            current_tree TEXT,
            observed_at_epoch_seconds INTEGER,
            materialization_status TEXT NOT NULL CHECK (materialization_status IN ('pending', 'present', 'not_found', 'corrupted', 'unknown', 'failed')),
            cleanliness TEXT NOT NULL CHECK (cleanliness IN ('clean', 'dirty', 'unknown')),
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            PRIMARY KEY (workspace_id, workdir_id),
            CHECK (
                (source_kind = 'repository' AND runtime_id IS NOT NULL AND repository_id IS NOT NULL AND external_grant_id IS NULL)
                OR (source_kind = 'external_grant' AND runtime_id IS NULL AND repository_id IS NULL AND external_grant_id IS NOT NULL)
            ),
            FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
            FOREIGN KEY (workspace_id, repository_id) REFERENCES repositories(workspace_id, repository_id),
            FOREIGN KEY (workspace_id, external_grant_id) REFERENCES external_workdir_grants(workspace_id, grant_id) DEFERRABLE INITIALLY DEFERRED
        );
        INSERT INTO workdir_registry_v64 (
            workspace_id, workdir_id, display_name, source_kind, runtime_id, repository_id,
            external_grant_id, creation_selector, creation_ref, creation_tree, current_selector,
            current_ref, current_tree, observed_at_epoch_seconds, materialization_status,
            cleanliness, created_at, updated_at
        ) SELECT workspace_id, workdir_id, display_name, 'repository', runtime_id, repository_id,
                 NULL, creation_selector, creation_ref, creation_tree, current_selector,
                 current_ref, current_tree, observed_at_epoch_seconds, materialization_status,
                 cleanliness, created_at, updated_at
          FROM workdir_registry;
        DROP TABLE workdir_registry;
        ALTER TABLE workdir_registry_v64 RENAME TO workdir_registry;
        CREATE TABLE worker_workdir_attachment_reservations (
            workspace_id TEXT NOT NULL,
            workdir_id TEXT NOT NULL,
            reservation_id TEXT NOT NULL,
            reserved_at TEXT NOT NULL,
            PRIMARY KEY (workspace_id, workdir_id),
            FOREIGN KEY (workspace_id, workdir_id)
                REFERENCES workdir_registry(workspace_id, workdir_id) ON DELETE CASCADE
        );
        INSERT INTO worker_workdir_attachment_reservations
            (workspace_id, workdir_id, reservation_id, reserved_at)
            SELECT workspace_id, workdir_id, reservation_id, reserved_at
            FROM worker_workdir_attachment_reservations_v64;
        CREATE TABLE worker_workdir_links (
            workspace_id TEXT NOT NULL,
            runtime_id TEXT NOT NULL,
            worker_id TEXT NOT NULL,
            workdir_id TEXT NOT NULL,
            alias TEXT NOT NULL,
            linked_at TEXT NOT NULL,
            unlinked_at TEXT,
            PRIMARY KEY (workspace_id, worker_id, workdir_id, alias),
            FOREIGN KEY (workspace_id, worker_id)
                REFERENCES worker_registry(workspace_id, worker_id) ON DELETE CASCADE,
            FOREIGN KEY (workspace_id, workdir_id)
                REFERENCES workdir_registry(workspace_id, workdir_id) ON DELETE CASCADE
        );
        INSERT INTO worker_workdir_links
            (workspace_id, runtime_id, worker_id, workdir_id, alias, linked_at, unlinked_at)
            SELECT workspace_id, runtime_id, worker_id, workdir_id, alias, linked_at, unlinked_at
            FROM worker_workdir_links_v64;
        DROP TABLE worker_workdir_attachment_reservations_v64;
        DROP TABLE worker_workdir_links_v64;
        CREATE UNIQUE INDEX worker_workdir_links_active_workdir_unique
            ON worker_workdir_links(workspace_id, workdir_id) WHERE unlinked_at IS NULL;
        CREATE UNIQUE INDEX worker_workdir_links_active_alias_unique
            ON worker_workdir_links(workspace_id, worker_id, alias) WHERE unlinked_at IS NULL;
        CREATE INDEX worker_workdir_links_workdir
            ON worker_workdir_links(workspace_id, workdir_id);
        CREATE INDEX idx_workdir_registry_workspace_updated
            ON workdir_registry(workspace_id, updated_at DESC);
        CREATE INDEX idx_external_workdir_grants_status_expiry
            ON external_workdir_grants(workspace_id, status, expires_at);
        CREATE TRIGGER workdir_registry_insert_blocked_by_runtime_removal
        BEFORE INSERT ON workdir_registry FOR EACH ROW
        WHEN NEW.runtime_id IS NOT NULL AND EXISTS (
            SELECT 1 FROM runtime_removal_operations operation
            WHERE operation.runtime_id = NEW.runtime_id
              AND operation.state IN ('pending', 'cleanup_pending')
        )
        BEGIN SELECT RAISE(ABORT, 'runtime_removal_in_progress'); END;
        CREATE TRIGGER workdir_registry_update_blocked_by_runtime_removal
        BEFORE UPDATE ON workdir_registry FOR EACH ROW
        WHEN NEW.runtime_id IS NOT NULL AND EXISTS (
            SELECT 1 FROM runtime_removal_operations operation
            WHERE operation.runtime_id = NEW.runtime_id
              AND operation.state IN ('pending', 'cleanup_pending')
        )
        BEGIN SELECT RAISE(ABORT, 'runtime_removal_in_progress'); END;
        "#,
    )?;
    tx.execute(
        "INSERT INTO __yoi_schema_migrations (version, name) VALUES (?1, ?2)",
        params![64, EXTERNAL_WORKDIR_GRANTS_MIGRATION_NAME],
    )?;
    tx.commit()?;
    Ok(())
}

fn migrate_external_workdir_cleanup_v64_to_v65(conn: &Connection) -> Result<()> {
    let tx = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Exclusive)?;
    if !column_exists(&tx, "external_workdir_grants", "cleanup_state")? {
        tx.execute_batch(
            "ALTER TABLE external_workdir_grants ADD COLUMN cleanup_state TEXT NOT NULL DEFAULT 'not_required' CHECK (cleanup_state IN ('not_required', 'pending', 'retry_required', 'completed'));\
             ALTER TABLE external_workdir_grants ADD COLUMN cleanup_error TEXT;\
             ALTER TABLE external_workdir_grants ADD COLUMN cleanup_updated_at TEXT;",
        )?;
        tx.execute(
            "UPDATE external_workdir_grants
             SET cleanup_state = 'pending', cleanup_updated_at = updated_at
             WHERE status IN ('revoked', 'expired')",
            [],
        )?;
    }
    tx.execute(
        "INSERT INTO __yoi_schema_migrations (version, name) VALUES (?1, ?2)",
        params![65, EXTERNAL_WORKDIR_CLEANUP_MIGRATION_NAME],
    )?;
    tx.commit()?;
    Ok(())
}

fn migrate_ticket_targets_and_workdir_capabilities_v65_to_v66(conn: &Connection) -> Result<()> {
    let current = current_schema_version(conn)?;
    if current != 65 {
        return Err(Error::Store(format!(
            "expected schema version 65 before {TICKET_TARGETS_AND_WORKDIR_CAPABILITIES_MIGRATION_NAME} migration, found {current}"
        )));
    }
    apply_ticket_targets_and_workdir_capabilities_schema(conn)
}

fn migrate_external_workdir_access_and_optional_expiry_v66_to_v67(conn: &Connection) -> Result<()> {
    let current = current_schema_version(conn)?;
    if current != 66 {
        return Err(Error::Store(format!(
            "expected schema version 66 before {EXTERNAL_WORKDIR_ACCESS_AND_OPTIONAL_EXPIRY_MIGRATION_NAME} migration, found {current}"
        )));
    }
    let foreign_keys_enabled =
        conn.query_row("PRAGMA foreign_keys", [], |row| row.get::<_, i64>(0))? != 0;
    if foreign_keys_enabled {
        conn.execute_batch("PRAGMA foreign_keys = OFF;")?;
    }
    let result = (|| {
        let tx = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Exclusive)?;
        tx.execute_batch(
            r#"
            CREATE TABLE external_workdir_grants_v67 (
                grant_id TEXT NOT NULL,
                workspace_id TEXT NOT NULL,
                workdir_id TEXT NOT NULL,
                provider_instance_id TEXT NOT NULL,
                display_name TEXT NOT NULL,
                permissions TEXT NOT NULL CHECK (permissions IN ('read_only', 'read_write')),
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
            INSERT INTO external_workdir_grants_v67 (
                grant_id, workspace_id, workdir_id, provider_instance_id, display_name,
                permissions, created_by, created_at, expires_at, generation, status, updated_at,
                cleanup_state, cleanup_error, cleanup_updated_at
            ) SELECT grant_id, workspace_id, workdir_id, provider_instance_id, display_name,
                     permissions, created_by, created_at, expires_at, generation, status, updated_at,
                     cleanup_state, cleanup_error, cleanup_updated_at
                FROM external_workdir_grants;
            DROP TABLE external_workdir_grants;
            ALTER TABLE external_workdir_grants_v67 RENAME TO external_workdir_grants;
            CREATE INDEX idx_external_workdir_grants_status_expiry
                ON external_workdir_grants(workspace_id, status, expires_at);
            CREATE TABLE worker_workdir_links_v67 (
                workspace_id TEXT NOT NULL,
                runtime_id TEXT NOT NULL,
                worker_id TEXT NOT NULL,
                workdir_id TEXT NOT NULL,
                alias TEXT NOT NULL,
                capabilities TEXT NOT NULL CHECK (capabilities IN ('all', 'read_only', 'read_write')),
                linked_at TEXT NOT NULL,
                unlinked_at TEXT,
                PRIMARY KEY (workspace_id, worker_id, workdir_id, alias),
                FOREIGN KEY (workspace_id, worker_id)
                    REFERENCES worker_registry(workspace_id, worker_id) ON DELETE CASCADE,
                FOREIGN KEY (workspace_id, workdir_id)
                    REFERENCES workdir_registry(workspace_id, workdir_id) ON DELETE CASCADE
            );
            INSERT INTO worker_workdir_links_v67 (
                workspace_id, runtime_id, worker_id, workdir_id, alias, capabilities,
                linked_at, unlinked_at
            ) SELECT workspace_id, runtime_id, worker_id, workdir_id, alias, capabilities,
                     linked_at, unlinked_at
                FROM worker_workdir_links;
            DROP TABLE worker_workdir_links;
            ALTER TABLE worker_workdir_links_v67 RENAME TO worker_workdir_links;
            CREATE UNIQUE INDEX worker_workdir_links_active_workdir_unique
                ON worker_workdir_links(workspace_id, workdir_id) WHERE unlinked_at IS NULL;
            CREATE UNIQUE INDEX worker_workdir_links_active_alias_unique
                ON worker_workdir_links(workspace_id, worker_id, alias) WHERE unlinked_at IS NULL;
            CREATE INDEX worker_workdir_links_workdir
                ON worker_workdir_links(workspace_id, workdir_id);
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
            "#,
        )?;
        let violations =
            tx.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
                row.get::<_, i64>(0)
            })?;
        if violations != 0 {
            return Err(Error::Store(format!(
                "schema-67 migration left {violations} foreign-key violation(s)"
            )));
        }
        tx.execute(
            "INSERT INTO __yoi_schema_migrations (version, name) VALUES (?1, ?2)",
            params![
                67_i64,
                EXTERNAL_WORKDIR_ACCESS_AND_OPTIONAL_EXPIRY_MIGRATION_NAME
            ],
        )?;
        tx.commit()?;
        Ok(())
    })();
    let restore = if foreign_keys_enabled {
        conn.execute_batch("PRAGMA foreign_keys = ON;")
            .map_err(Error::from)
    } else {
        Ok(())
    };
    match (result, restore) {
        (Err(error), _) => Err(error),
        (Ok(()), Err(error)) => Err(error),
        (Ok(()), Ok(())) => Ok(()),
    }
}

fn migrate_external_workdir_command_permission_v67_to_v68(conn: &Connection) -> Result<()> {
    let current = current_schema_version(conn)?;
    if current != 67 {
        return Err(Error::Store(format!(
            "expected schema version 67 before {EXTERNAL_WORKDIR_COMMAND_PERMISSION_MIGRATION_NAME} migration, found {current}"
        )));
    }
    let foreign_keys_enabled =
        conn.query_row("PRAGMA foreign_keys", [], |row| row.get::<_, i64>(0))? != 0;
    if foreign_keys_enabled {
        conn.execute_batch("PRAGMA foreign_keys = OFF;")?;
    }
    let result = (|| {
        let tx = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Exclusive)?;
        tx.execute_batch(
            r#"
            CREATE TABLE external_workdir_grants_v68 (
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
            INSERT INTO external_workdir_grants_v68 (
                grant_id, workspace_id, workdir_id, provider_instance_id, display_name,
                permissions, created_by, created_at, expires_at, generation, status, updated_at,
                cleanup_state, cleanup_error, cleanup_updated_at
            ) SELECT grant_id, workspace_id, workdir_id, provider_instance_id, display_name,
                     permissions, created_by, created_at, expires_at, generation, status, updated_at,
                     cleanup_state, cleanup_error, cleanup_updated_at
                FROM external_workdir_grants;
            DROP TABLE external_workdir_grants;
            ALTER TABLE external_workdir_grants_v68 RENAME TO external_workdir_grants;
            CREATE INDEX idx_external_workdir_grants_status_expiry
                ON external_workdir_grants(workspace_id, status, expires_at);
            "#,
        )?;
        let violations =
            tx.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
                row.get::<_, i64>(0)
            })?;
        if violations != 0 {
            return Err(Error::Store(format!(
                "schema-68 migration left {violations} foreign-key violation(s)"
            )));
        }
        tx.execute(
            "INSERT INTO __yoi_schema_migrations (version, name) VALUES (?1, ?2)",
            params![68_i64, EXTERNAL_WORKDIR_COMMAND_PERMISSION_MIGRATION_NAME],
        )?;
        tx.commit()?;
        Ok(())
    })();
    let restore = if foreign_keys_enabled {
        conn.execute_batch("PRAGMA foreign_keys = ON;")
            .map_err(Error::from)
    } else {
        Ok(())
    };
    match (result, restore) {
        (Err(error), _) => Err(error),
        (Ok(()), Err(error)) => Err(error),
        (Ok(()), Ok(())) => Ok(()),
    }
}

fn migrate_workdir_command_attachment_capabilities_v68_to_v69(conn: &Connection) -> Result<()> {
    let current = current_schema_version(conn)?;
    if current != 68 {
        return Err(Error::Store(format!(
            "expected schema version 68 before {WORKDIR_COMMAND_ATTACHMENT_CAPABILITIES_MIGRATION_NAME} migration, found {current}"
        )));
    }
    let foreign_keys_enabled =
        conn.query_row("PRAGMA foreign_keys", [], |row| row.get::<_, i64>(0))? != 0;
    if foreign_keys_enabled {
        conn.execute_batch("PRAGMA foreign_keys = OFF;")?;
    }
    let result = (|| {
        let tx = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Exclusive)?;
        tx.execute_batch(
            r#"
            CREATE TABLE worker_workdir_links_v69 (
                workspace_id TEXT NOT NULL,
                runtime_id TEXT NOT NULL,
                worker_id TEXT NOT NULL,
                workdir_id TEXT NOT NULL,
                alias TEXT NOT NULL,
                capabilities TEXT NOT NULL CHECK (capabilities IN (
                    'all', 'read_only', 'read_write', 'command_only', 'read_command'
                )),
                linked_at TEXT NOT NULL,
                unlinked_at TEXT,
                PRIMARY KEY (workspace_id, worker_id, workdir_id, alias),
                FOREIGN KEY (workspace_id, worker_id)
                    REFERENCES worker_registry(workspace_id, worker_id) ON DELETE CASCADE,
                FOREIGN KEY (workspace_id, workdir_id)
                    REFERENCES workdir_registry(workspace_id, workdir_id) ON DELETE CASCADE
            );
            INSERT INTO worker_workdir_links_v69 (
                workspace_id, runtime_id, worker_id, workdir_id, alias, capabilities,
                linked_at, unlinked_at
            ) SELECT workspace_id, runtime_id, worker_id, workdir_id, alias, capabilities,
                     linked_at, unlinked_at
                FROM worker_workdir_links;
            DROP TABLE worker_workdir_links;
            ALTER TABLE worker_workdir_links_v69 RENAME TO worker_workdir_links;
            CREATE UNIQUE INDEX worker_workdir_links_active_workdir_unique
                ON worker_workdir_links(workspace_id, workdir_id) WHERE unlinked_at IS NULL;
            CREATE UNIQUE INDEX worker_workdir_links_active_alias_unique
                ON worker_workdir_links(workspace_id, worker_id, alias) WHERE unlinked_at IS NULL;
            CREATE INDEX worker_workdir_links_workdir
                ON worker_workdir_links(workspace_id, workdir_id);
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
            "#,
        )?;
        let violations =
            tx.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
                row.get::<_, i64>(0)
            })?;
        if violations != 0 {
            return Err(Error::Store(format!(
                "schema-69 migration left {violations} foreign-key violation(s)"
            )));
        }
        tx.execute(
            "INSERT INTO __yoi_schema_migrations (version, name) VALUES (?1, ?2)",
            params![
                69_i64,
                WORKDIR_COMMAND_ATTACHMENT_CAPABILITIES_MIGRATION_NAME
            ],
        )?;
        tx.commit()?;
        Ok(())
    })();
    let restore = if foreign_keys_enabled {
        conn.execute_batch("PRAGMA foreign_keys = ON;")
            .map_err(Error::from)
    } else {
        Ok(())
    };
    match (result, restore) {
        (Err(error), _) => Err(error),
        (Ok(()), Err(error)) => Err(error),
        (Ok(()), Ok(())) => Ok(()),
    }
}

fn migrate_backend_job_runner_v69_to_v70(conn: &Connection) -> Result<()> {
    let current = current_schema_version(conn)?;
    if current != 69 {
        return Err(Error::Store(format!(
            "expected schema version 69 before {BACKEND_JOB_RUNNER_MIGRATION_NAME} migration, found {current}"
        )));
    }
    let tx = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Exclusive)?;
    tx.execute_batch(
        r#"
        CREATE TABLE backend_jobs (
            workspace_id TEXT NOT NULL,
            job_id TEXT NOT NULL,
            purpose TEXT NOT NULL,
            input_revision TEXT NOT NULL,
            input_ref TEXT NOT NULL,
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
        CREATE TABLE backend_job_attempts (
            workspace_id TEXT NOT NULL,
            job_id TEXT NOT NULL,
            attempt_id TEXT NOT NULL,
            attempt INTEGER NOT NULL CHECK (attempt > 0 AND attempt <= 3),
            input_revision TEXT NOT NULL,
            state TEXT NOT NULL CHECK (state IN ('reserved', 'dispatched', 'completed', 'failed', 'unknown')),
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
        CREATE TABLE backend_job_deliveries (
            workspace_id TEXT NOT NULL,
            delivery_id TEXT NOT NULL,
            job_id TEXT NOT NULL,
            attempt_id TEXT NOT NULL,
            target_runtime_id TEXT NOT NULL,
            target_worker_id TEXT NOT NULL,
            state TEXT NOT NULL CHECK (state IN ('pending', 'completed', 'failed')),
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
        "#,
    )?;
    tx.execute(
        "INSERT INTO __yoi_schema_migrations (version, name) VALUES (?1, ?2)",
        params![70_i64, BACKEND_JOB_RUNNER_MIGRATION_NAME],
    )?;
    tx.commit()?;
    Ok(())
}

fn migrate_backend_job_delivery_claims_v70_to_v71(conn: &Connection) -> Result<()> {
    let current = current_schema_version(conn)?;
    if current != 70 {
        return Err(Error::Store(format!(
            "expected schema version 70 before {BACKEND_JOB_DELIVERY_CLAIM_MIGRATION_NAME} migration, found {current}"
        )));
    }
    let tx = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Exclusive)?;
    tx.execute_batch(
        r#"
        DROP INDEX backend_job_deliveries_pending;
        ALTER TABLE backend_job_deliveries RENAME TO backend_job_deliveries_v70;
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
        INSERT INTO backend_job_deliveries (
            workspace_id, delivery_id, job_id, attempt_id, target_runtime_id,
            target_worker_id, state, failure_category, failure_detail,
            created_at, updated_at, delivered_at
        )
        SELECT workspace_id, delivery_id, job_id, attempt_id, target_runtime_id,
               target_worker_id, state, failure_category, failure_detail,
               created_at, updated_at, delivered_at
        FROM backend_job_deliveries_v70;
        DROP TABLE backend_job_deliveries_v70;
        CREATE INDEX backend_job_deliveries_pending
            ON backend_job_deliveries(workspace_id, state, updated_at);
        "#,
    )?;
    tx.execute(
        "INSERT INTO __yoi_schema_migrations (version, name) VALUES (?1, ?2)",
        params![71_i64, BACKEND_JOB_DELIVERY_CLAIM_MIGRATION_NAME],
    )?;
    tx.commit()?;
    Ok(())
}

fn migrate_backend_job_dispatch_queue_v71_to_v72(conn: &Connection) -> Result<()> {
    let current = current_schema_version(conn)?;
    if current != 71 {
        return Err(Error::Store(format!(
            "expected schema version 71 before {BACKEND_JOB_DISPATCH_QUEUE_MIGRATION_NAME} migration, found {current}"
        )));
    }
    let tx = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Exclusive)?;
    tx.execute_batch(
        r#"
        DROP INDEX backend_job_deliveries_pending;
        DROP INDEX backend_job_attempts_recovery;
        ALTER TABLE backend_job_deliveries RENAME TO backend_job_deliveries_v71;
        ALTER TABLE backend_job_attempts RENAME TO backend_job_attempts_v71;
        CREATE TABLE backend_job_attempts (
            workspace_id TEXT NOT NULL,
            job_id TEXT NOT NULL,
            attempt_id TEXT NOT NULL,
            attempt INTEGER NOT NULL CHECK (attempt > 0 AND attempt <= 3),
            input_revision TEXT NOT NULL,
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
        INSERT INTO backend_job_attempts SELECT * FROM backend_job_attempts_v71;
        CREATE INDEX backend_job_attempts_recovery
            ON backend_job_attempts(workspace_id, state, deadline_at);
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
        INSERT INTO backend_job_deliveries SELECT * FROM backend_job_deliveries_v71;
        CREATE INDEX backend_job_deliveries_pending
            ON backend_job_deliveries(workspace_id, state, updated_at);
        DROP TABLE backend_job_deliveries_v71;
        DROP TABLE backend_job_attempts_v71;
        "#,
    )?;
    tx.execute(
        "INSERT INTO __yoi_schema_migrations (version, name) VALUES (?1, ?2)",
        params![72_i64, BACKEND_JOB_DISPATCH_QUEUE_MIGRATION_NAME],
    )?;
    tx.commit()?;
    Ok(())
}

fn migrate_worker_singleton_ownership_v72_to_v73(conn: &Connection) -> Result<()> {
    let current = current_schema_version(conn)?;
    if current != 72 {
        return Err(Error::Store(format!(
            "expected schema version 72 before {WORKER_SINGLETON_OWNERSHIP_MIGRATION_NAME} migration, found {current}"
        )));
    }
    let tx = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Exclusive)?;
    if !column_exists(&tx, "worker_create_reservations", "singleton_key")? {
        tx.execute(
            "ALTER TABLE worker_create_reservations ADD COLUMN singleton_key TEXT",
            [],
        )?;
    }
    if !column_exists(&tx, "worker_create_reservations", "singleton_generation")? {
        tx.execute(
            "ALTER TABLE worker_create_reservations ADD COLUMN singleton_generation INTEGER \
             CHECK (singleton_generation IS NULL OR singleton_generation > 0)",
            [],
        )?;
    }
    tx.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS worker_singleton_owners (
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
        CREATE TRIGGER IF NOT EXISTS worker_create_reservation_singleton_lease_insert
        BEFORE INSERT ON worker_create_reservations
        WHEN (NEW.singleton_key IS NULL AND NEW.singleton_generation IS NOT NULL)
          OR (NEW.singleton_key IS NOT NULL AND NEW.singleton_generation IS NULL)
        BEGIN
            SELECT RAISE(ABORT, 'worker_singleton_lease_incomplete');
        END;
        CREATE TRIGGER IF NOT EXISTS worker_create_reservation_singleton_lease_update
        BEFORE UPDATE OF singleton_key, singleton_generation ON worker_create_reservations
        WHEN (NEW.singleton_key IS NULL AND NEW.singleton_generation IS NOT NULL)
          OR (NEW.singleton_key IS NOT NULL AND NEW.singleton_generation IS NULL)
        BEGIN
            SELECT RAISE(ABORT, 'worker_singleton_lease_incomplete');
        END;

        -- Preserve the two historical singleton conventions as Backend ownership. If an old
        -- database contains duplicates, deterministically fence every Worker except the oldest.
        INSERT INTO worker_singleton_owners (
            workspace_id, singleton_key, runtime_id, worker_id, generation, created_at, updated_at
        )
        SELECT worker.workspace_id, 'workspace-orchestrator', worker.runtime_id, worker.worker_id,
               1, worker.created_at, worker.updated_at
        FROM worker_registry worker
        WHERE worker.profile IN ('orchestrator', 'builtin:orchestrator')
          AND worker.display_name = 'Workspace Orchestrator'
          AND NOT EXISTS (
              SELECT 1 FROM worker_registry older
              WHERE older.workspace_id = worker.workspace_id
                AND older.profile IN ('orchestrator', 'builtin:orchestrator')
                AND older.display_name = 'Workspace Orchestrator'
                AND (older.created_at < worker.created_at
                     OR (older.created_at = worker.created_at AND older.worker_id < worker.worker_id))
          );
        INSERT INTO worker_singleton_owners (
            workspace_id, singleton_key, runtime_id, worker_id, generation, created_at, updated_at
        )
        SELECT worker.workspace_id, 'workspace-memory-consolidation', worker.runtime_id,
               worker.worker_id, 1, worker.created_at, worker.updated_at
        FROM worker_registry worker
        WHERE worker.profile IN ('memory-consolidation', 'builtin:memory-consolidation')
          AND NOT EXISTS (
              SELECT 1 FROM worker_registry older
              WHERE older.workspace_id = worker.workspace_id
                AND older.profile IN ('memory-consolidation', 'builtin:memory-consolidation')
                AND (older.created_at < worker.created_at
                     OR (older.created_at = worker.created_at AND older.worker_id < worker.worker_id))
          );
        INSERT INTO worker_create_reservations (
            workspace_id, allocation_key, worker_id, runtime_id, create_fingerprint,
            state, created_at, updated_at, request_fingerprint,
            memory_settings_revision, memory_language, singleton_key, singleton_generation
        )
        SELECT worker.workspace_id, 'migration-v73-singleton:' || worker.worker_id,
               worker.worker_id, worker.runtime_id,
               'migration-v73:' || CASE
                   WHEN worker.profile IN ('orchestrator', 'builtin:orchestrator')
                        AND worker.display_name = 'Workspace Orchestrator'
                       THEN 'workspace-orchestrator'
                   ELSE 'workspace-memory-consolidation'
               END,
               'created', worker.created_at, worker.updated_at, NULL, NULL, NULL,
               CASE
                   WHEN worker.profile IN ('orchestrator', 'builtin:orchestrator')
                        AND worker.display_name = 'Workspace Orchestrator'
                       THEN 'workspace-orchestrator'
                   ELSE 'workspace-memory-consolidation'
               END,
               1
        FROM worker_registry worker
        WHERE (
                (
                    worker.profile IN ('orchestrator', 'builtin:orchestrator')
                    AND worker.display_name = 'Workspace Orchestrator'
                )
                OR worker.profile IN ('memory-consolidation', 'builtin:memory-consolidation')
              )
          AND NOT EXISTS (
              SELECT 1 FROM worker_create_reservations reservation
              WHERE reservation.workspace_id = worker.workspace_id
                AND reservation.worker_id = worker.worker_id
          );
        UPDATE worker_create_reservations
        SET singleton_key = (
                SELECT CASE
                    WHEN worker.profile IN ('orchestrator', 'builtin:orchestrator')
                         AND worker.display_name = 'Workspace Orchestrator'
                        THEN 'workspace-orchestrator'
                    ELSE 'workspace-memory-consolidation'
                END
                FROM worker_registry worker
                WHERE worker.workspace_id = worker_create_reservations.workspace_id
                  AND worker.worker_id = worker_create_reservations.worker_id
                  AND (
                        (worker.profile IN ('orchestrator', 'builtin:orchestrator')
                         AND worker.display_name = 'Workspace Orchestrator')
                        OR worker.profile IN ('memory-consolidation', 'builtin:memory-consolidation')
                      )
            ),
            singleton_generation = 1
        WHERE EXISTS (
            SELECT 1 FROM worker_registry worker
            WHERE worker.workspace_id = worker_create_reservations.workspace_id
              AND worker.worker_id = worker_create_reservations.worker_id
              AND (
                    (worker.profile IN ('orchestrator', 'builtin:orchestrator')
                     AND worker.display_name = 'Workspace Orchestrator')
                    OR worker.profile IN ('memory-consolidation', 'builtin:memory-consolidation')
                  )
        );
        "#,
    )?;
    tx.execute(
        "INSERT INTO __yoi_schema_migrations (version, name) VALUES (?1, ?2)",
        params![73_i64, WORKER_SINGLETON_OWNERSHIP_MIGRATION_NAME],
    )?;
    tx.commit()?;
    Ok(())
}

fn migrate_ordered_worker_projection_v73_to_v74(conn: &Connection) -> Result<()> {
    let current = current_schema_version(conn)?;
    if current != 73 {
        return Err(Error::Store(format!(
            "expected schema version 73 before {ORDERED_WORKER_PROJECTION_MIGRATION_NAME} migration, found {current}"
        )));
    }
    let tx = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Exclusive)?;
    tx.execute_batch(
        r#"
        ALTER TABLE worker_registry_observations RENAME TO worker_registry_observations_v73;
        CREATE TABLE worker_registry_observations (
            workspace_id TEXT NOT NULL,
            runtime_id TEXT NOT NULL,
            worker_id TEXT NOT NULL,
            availability TEXT NOT NULL CHECK (availability IN ('observed', 'unavailable')),
            worker_json TEXT,
            observed_at TEXT NOT NULL,
            PRIMARY KEY (workspace_id, runtime_id, worker_id),
            FOREIGN KEY (workspace_id, worker_id)
                REFERENCES worker_registry(workspace_id, worker_id) ON DELETE CASCADE
        );
        INSERT INTO worker_registry_observations (
            workspace_id, runtime_id, worker_id, availability, worker_json, observed_at
        )
        SELECT workspace_id, runtime_id, worker_id, availability, worker_json, observed_at
        FROM worker_registry_observations_v73;
        DROP TABLE worker_registry_observations_v73;

        DROP TABLE IF EXISTS worker_registry_projection_cursors;
        DROP TABLE IF EXISTS worker_registry_projection_revisions;

        ALTER TABLE worker_registry_projection_removals
            RENAME TO worker_registry_projection_removals_v73;
        CREATE TABLE worker_registry_projection_removals (
            workspace_id TEXT NOT NULL,
            runtime_id TEXT NOT NULL,
            worker_id TEXT NOT NULL,
            PRIMARY KEY (workspace_id, runtime_id, worker_id)
        );
        INSERT INTO worker_registry_projection_removals (workspace_id, runtime_id, worker_id)
        SELECT workspace_id, runtime_id, worker_id
        FROM worker_registry_projection_removals_v73;
        DROP TABLE worker_registry_projection_removals_v73;
        "#,
    )?;
    tx.execute(
        "INSERT INTO __yoi_schema_migrations (version, name) VALUES (?1, ?2)",
        params![74_i64, ORDERED_WORKER_PROJECTION_MIGRATION_NAME],
    )?;
    tx.commit()?;
    Ok(())
}

fn migrate_runtime_scoped_worker_identity_v74_to_v75(conn: &Connection) -> Result<()> {
    let current = current_schema_version(conn)?;
    if current != 74 {
        return Err(Error::Store(format!(
            "expected schema version 74 before {RUNTIME_SCOPED_WORKER_IDENTITY_MIGRATION_NAME} migration, found {current}"
        )));
    }
    let foreign_keys_enabled =
        conn.query_row("PRAGMA foreign_keys", [], |row| row.get::<_, i64>(0))? != 0;
    let legacy_alter_table_enabled =
        conn.query_row("PRAGMA legacy_alter_table", [], |row| row.get::<_, i64>(0))? != 0;
    if foreign_keys_enabled {
        conn.execute_batch("PRAGMA foreign_keys = OFF;")?;
    }
    if !legacy_alter_table_enabled {
        conn.execute_batch("PRAGMA legacy_alter_table = ON;")?;
    }
    let result = (|| {
        let tx = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Exclusive)?;
        tx.execute_batch(
            r#"
            DROP INDEX worker_control_grants_controller;
            DROP INDEX worker_control_grants_subject;
            DROP INDEX worker_workdir_links_active_workdir_unique;
            DROP INDEX worker_workdir_links_active_alias_unique;
            DROP INDEX worker_workdir_links_workdir;
            DROP INDEX idx_worker_registry_workspace_runtime_worker;
            DROP INDEX worker_registry_runtime;

            ALTER TABLE worker_registry_observations RENAME TO worker_registry_observations_v74;
            ALTER TABLE worker_control_grants RENAME TO worker_control_grants_v74;
            ALTER TABLE worker_workdir_links RENAME TO worker_workdir_links_v74;
            ALTER TABLE worker_registry RENAME TO worker_registry_v74;

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
            INSERT INTO worker_registry
            SELECT * FROM worker_registry_v74;

            UPDATE workspace_resource_keys AS resource
            SET resource_id = COALESCE(
                (
                    SELECT hex(worker.runtime_id) || ':' || hex(worker.worker_id)
                    FROM worker_registry AS worker
                    WHERE worker.workspace_id = resource.workspace_id
                      AND worker.worker_id = resource.resource_id
                    LIMIT 1
                ),
                (
                    SELECT hex(reservation.runtime_id) || ':' || hex(reservation.worker_id)
                    FROM worker_create_reservations AS reservation
                    WHERE reservation.workspace_id = resource.workspace_id
                      AND reservation.worker_id = resource.resource_id
                    LIMIT 1
                )
            )
            WHERE resource.resource_kind = 'worker'
              AND (
                    EXISTS (
                        SELECT 1 FROM worker_registry AS worker
                        WHERE worker.workspace_id = resource.workspace_id
                          AND worker.worker_id = resource.resource_id
                    )
                    OR EXISTS (
                        SELECT 1 FROM worker_create_reservations AS reservation
                        WHERE reservation.workspace_id = resource.workspace_id
                          AND reservation.worker_id = resource.resource_id
                    )
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
            INSERT INTO worker_registry_observations
            SELECT * FROM worker_registry_observations_v74;

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
            INSERT INTO worker_control_grants
            SELECT * FROM worker_control_grants_v74;

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
                )),
                PRIMARY KEY (workspace_id, runtime_id, worker_id, workdir_id, alias),
                FOREIGN KEY (workspace_id, runtime_id, worker_id)
                    REFERENCES worker_registry(workspace_id, runtime_id, worker_id) ON DELETE CASCADE,
                FOREIGN KEY (workspace_id, workdir_id)
                    REFERENCES workdir_registry(workspace_id, workdir_id) ON DELETE CASCADE
            );
            INSERT INTO worker_workdir_links (
                workspace_id, runtime_id, worker_id, workdir_id, alias,
                linked_at, unlinked_at, capabilities
            )
            SELECT workspace_id, runtime_id, worker_id, workdir_id, alias,
                   linked_at, unlinked_at, capabilities
            FROM worker_workdir_links_v74;

            DROP TABLE worker_registry_observations_v74;
            DROP TABLE worker_control_grants_v74;
            DROP TABLE worker_workdir_links_v74;
            DROP TABLE worker_registry_v74;

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
            "#,
        )?;
        tx.execute(
            "INSERT INTO __yoi_schema_migrations (version, name) VALUES (?1, ?2)",
            params![75_i64, RUNTIME_SCOPED_WORKER_IDENTITY_MIGRATION_NAME],
        )?;
        tx.commit()?;
        Ok(())
    })();
    let restore_result = match (legacy_alter_table_enabled, foreign_keys_enabled) {
        (false, true) => {
            conn.execute_batch("PRAGMA legacy_alter_table = OFF; PRAGMA foreign_keys = ON;")
        }
        (false, false) => conn.execute_batch("PRAGMA legacy_alter_table = OFF;"),
        (true, true) => conn.execute_batch("PRAGMA foreign_keys = ON;"),
        (true, false) => Ok(()),
    }
    .map_err(Error::from);
    match (result, restore_result) {
        (Err(error), _) => Err(error),
        (Ok(()), Err(error)) => Err(error),
        (Ok(()), Ok(())) => {
            let violations = conn
                .prepare("PRAGMA foreign_key_check")?
                .query_map([], |_| Ok(()))?
                .count();
            if violations != 0 {
                return Err(Error::Store(format!(
                    "Runtime-scoped Worker identity migration left {violations} foreign key violation(s)"
                )));
            }
            Ok(())
        }
    }
}

fn migrate_archive_observe_grants_v75_to_v76(conn: &Connection) -> Result<()> {
    let current = current_schema_version(conn)?;
    if current != 75 {
        return Err(Error::Store(format!(
            "expected schema version 75 before {ARCHIVE_OBSERVE_GRANTS_MIGRATION_NAME} migration, found {current}"
        )));
    }
    let tx = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Exclusive)?;
    tx.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS worker_session_archive_observe_grants (
            workspace_id TEXT NOT NULL,
            archive_id TEXT NOT NULL,
            controller_runtime_id TEXT NOT NULL,
            controller_worker_id TEXT NOT NULL,
            subject_runtime_id TEXT NOT NULL,
            subject_worker_id TEXT NOT NULL,
            source_grant_id TEXT NOT NULL,
            granted_at TEXT NOT NULL,
            revoked_at TEXT,
            PRIMARY KEY (
                workspace_id, archive_id,
                controller_runtime_id, controller_worker_id,
                source_grant_id
            ),
            FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
            FOREIGN KEY (archive_id) REFERENCES worker_session_archives(archive_id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS worker_session_archive_observe_grants_controller
            ON worker_session_archive_observe_grants(
                workspace_id, controller_runtime_id, controller_worker_id, archive_id
            );
        "#,
    )?;
    tx.execute(
        "INSERT INTO __yoi_schema_migrations (version, name) VALUES (?1, ?2)",
        params![76_i64, ARCHIVE_OBSERVE_GRANTS_MIGRATION_NAME],
    )?;
    tx.commit()?;
    Ok(())
}

fn migrate_assignment_work_v81_to_v82(conn: &Connection) -> Result<()> {
    if current_schema_version(conn)? != 81 {
        return Err(Error::Store(
            "Ticket work separation requires schema 81".into(),
        ));
    }
    let foreign_keys =
        conn.pragma_query_value(None, "foreign_keys", |row| row.get::<_, bool>(0))?;
    let legacy =
        conn.pragma_query_value(None, "legacy_alter_table", |row| row.get::<_, bool>(0))?;
    conn.execute_batch("PRAGMA foreign_keys = OFF; PRAGMA legacy_alter_table = ON;")?;
    let result = (|| {
        let tx = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Exclusive)?;
        let dependents = {
            let mut stmt = tx.prepare("SELECT sql FROM sqlite_schema WHERE tbl_name='ticket_current_worker_assignments' AND type IN ('index','trigger') AND sql IS NOT NULL AND name != 'ticket_current_worker_role_idx'")?;
            let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        // The retained v82 migration keeps its original coder role contract.
        let schema = include_str!("frozen_schema_v85.sql")
            .replace(
                "role TEXT NOT NULL DEFAULT 'worker'",
                "role TEXT NOT NULL DEFAULT 'coder'",
            )
            .replace(
                "role IN ('orchestrator', 'worker'",
                "role IN ('orchestrator', 'coder'",
            );
        let start = schema
            .find("CREATE TABLE ticket_current_worker_assignments (")
            .ok_or_else(|| Error::Store("Missing assignment schema".into()))?;
        let end = schema[start..]
            .find("CREATE TABLE ticket_worker_assignment_events")
            .ok_or_else(|| Error::Store("Missing assignment schema end".into()))?
            + start;
        tx.execute_batch(&schema[start..end].replace(
            "CREATE TABLE ticket_current_worker_assignments",
            "CREATE TABLE ticket_current_worker_assignments_v82",
        ))?;
        tx.execute_batch("INSERT INTO ticket_current_worker_assignments_v82 SELECT * FROM ticket_current_worker_assignments;
            DROP TABLE ticket_current_worker_assignments;
            ALTER TABLE ticket_current_worker_assignments_v82 RENAME TO ticket_current_worker_assignments;")?;
        for sql in dependents {
            tx.execute_batch(&sql)?;
        }
        tx.execute_batch(include_str!("assignment_work.sql"))?;
        // Existing done/closed + current-assignment data is preserved, and a
        // durable release prevents a later reopen from resurrecting old work.
        tx.execute_batch("INSERT OR IGNORE INTO ticket_assignment_work_releases
            SELECT current.workspace_id, current.assignment_id, COALESCE(ticket.updated_at, current.updated_at)
            FROM ticket_current_worker_assignments current JOIN typed_tickets ticket
              ON ticket.workspace_id=current.workspace_id AND ticket.ticket_id=current.ticket_id
            WHERE ticket.workflow_state IN ('done','closed');")?;
        let legacy_plans = {
            let mut statement =
                tx.prepare("SELECT operation_id, blockers_json FROM worker_removal_operations")?;
            let rows = statement.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        for (operation_id, encoded) in legacy_plans {
            let mut blockers: Vec<serde_json::Value> = serde_json::from_str(&encoded)
                .map_err(|error| Error::Store(format!("invalid removal blockers: {error}")))?;
            let mut changed = false;
            for blocker in &mut blockers {
                if blocker.get("kind").and_then(serde_json::Value::as_str)
                    == Some("current_assignment")
                {
                    blocker["kind"] = serde_json::Value::String("unfinished_work".into());
                    changed = true;
                }
            }
            if changed {
                let encoded = serde_json::to_string(&blockers)
                    .map_err(|error| Error::Store(error.to_string()))?;
                // Historical blocked plans stay blocked; issued fingerprints
                // and identities are immutable. Fresh planning rereads active work.
                tx.execute(
                    "UPDATE worker_removal_operations SET blockers_json=?2 WHERE operation_id=?1",
                    params![operation_id, encoded],
                )?;
            }
        }
        let broken: i64 =
            tx.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
                row.get(0)
            })?;
        if broken != 0 {
            return Err(Error::Store(
                "Ticket work migration foreign-key verification failed".into(),
            ));
        }
        tx.execute("INSERT INTO __yoi_schema_migrations(version,name) VALUES (82,'Ticket responsibility and unfinished work separation')", [])?;
        tx.commit()?;
        Ok(())
    })();
    conn.pragma_update(None, "legacy_alter_table", legacy)?;
    conn.pragma_update(None, "foreign_keys", foreign_keys)?;
    result
}

fn migrate_ticket_worker_v82_to_v83(conn: &Connection) -> Result<()> {
    if current_schema_version(conn)? != 82 {
        return Err(Error::Store(
            "Ticket Worker migration requires schema 82".into(),
        ));
    }
    let foreign_keys =
        conn.pragma_query_value(None, "foreign_keys", |row| row.get::<_, bool>(0))?;
    let legacy =
        conn.pragma_query_value(None, "legacy_alter_table", |row| row.get::<_, bool>(0))?;
    conn.execute_batch("PRAGMA foreign_keys = OFF; PRAGMA legacy_alter_table = ON;")?;
    let result = (|| {
        let tx = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Exclusive)?;
        // Rebuild all role-bearing assignment tables together: merely renaming
        // the current role would break its historical composite foreign key.
        let schema = include_str!("frozen_schema_v85.sql");
        let mut dependents = Vec::new();
        for table in [
            "ticket_assignment_operations",
            "ticket_worker_assignments",
            "ticket_current_worker_assignments",
            "ticket_worker_assignment_events",
        ] {
            let mut stmt = tx.prepare("SELECT sql FROM sqlite_schema WHERE tbl_name=?1 AND type IN ('index','trigger') AND sql IS NOT NULL AND name != 'ticket_current_worker_role_idx'")?;
            let rows = stmt.query_map([table], |row| row.get::<_, String>(0))?;
            dependents.extend(rows.collect::<rusqlite::Result<Vec<_>>>()?);
            drop(stmt);
            let declaration = format!("CREATE TABLE {table} (");
            let start = schema
                .find(&declaration)
                .ok_or_else(|| Error::Store(format!("Missing assignment schema for {table}")))?;
            let end = schema[start..].find(';').ok_or_else(|| {
                Error::Store(format!("Missing assignment schema end for {table}"))
            })? + start
                + 1;
            let replacement = format!("{table}_v83");
            tx.execute_batch(&schema[start..end].replacen(
                &format!("CREATE TABLE {table}"),
                &format!("CREATE TABLE {replacement}"),
                1,
            ))?;
            let columns = table_columns(&tx, table)?;
            let values = columns
                .iter()
                .map(|column| {
                    if column == "role" {
                        "CASE role WHEN 'coder' THEN 'worker' ELSE role END".to_string()
                    } else {
                        format!("\"{column}\"")
                    }
                })
                .collect::<Vec<_>>()
                .join(", ");
            let column_names = columns
                .iter()
                .map(|column| format!("\"{column}\""))
                .collect::<Vec<_>>()
                .join(", ");
            // Only role-bearing historical columns are copied. New receipts
            // classify legacy unfinished assign operations conservatively.
            tx.execute_batch(&format!(
                "INSERT INTO {replacement} ({column_names}) SELECT {values} FROM {table};
                 DROP TABLE {table}; ALTER TABLE {replacement} RENAME TO {table};"
            ))?;
            if table == "ticket_assignment_operations"
                && !columns.iter().any(|c| c == "claim_state")
            {
                tx.execute(
                    "UPDATE ticket_assignment_operations SET claim_state='pending'
                    WHERE action IN ('assign','reassign') AND assignment_id IS NULL",
                    [],
                )?;
            }
        }
        for sql in dependents {
            tx.execute_batch(&sql.replace("'coder'", "'worker'"))?;
        }
        tx.execute(
            "UPDATE typed_ticket_event_attributes SET value='worker'
            WHERE key='assignment_role' AND value='coder'",
            [],
        )?;
        let broken: i64 =
            tx.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
                row.get(0)
            })?;
        if broken != 0 {
            return Err(Error::Store(
                "Ticket work migration foreign-key verification failed".into(),
            ));
        }
        tx.execute("INSERT INTO __yoi_schema_migrations(version,name) VALUES (83,'Generic Ticket Worker roles and durable claims')", [])?;
        tx.commit()?;
        Ok(())
    })();
    conn.pragma_update(None, "legacy_alter_table", legacy)?;
    conn.pragma_update(None, "foreign_keys", foreign_keys)?;
    result
}

fn migrate_backend_job_resources_v80_to_v81(conn: &Connection) -> Result<()> {
    if current_schema_version(conn)? != 80 {
        return Err(Error::Store(
            "Backend Job resource migration requires schema 80".into(),
        ));
    }
    let tx = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Exclusive)?;
    tx.execute_batch(
        "ALTER TABLE backend_jobs ADD COLUMN resource_key TEXT;
         CREATE INDEX backend_jobs_resource ON backend_jobs(workspace_id, resource_key, created_at);",
    )?;
    // Pre-grant requests had no resource lock. Preserve their exact stored JSON
    // and fingerprint; serde defaults read them as empty grants / no key.
    tx.execute(
        "INSERT INTO __yoi_schema_migrations(version, name) VALUES (81, 'Backend Job immutable resource serialization')",
        [],
    )?;
    tx.commit()?;
    Ok(())
}

fn migrate_backend_job_worker_cleanup_v76_to_v77(conn: &Connection) -> Result<()> {
    let current = current_schema_version(conn)?;
    if current != 76 {
        return Err(Error::Store(format!(
            "expected schema version 76 before {BACKEND_JOB_WORKER_CLEANUP_MIGRATION_NAME} migration, found {current}"
        )));
    }
    let tx = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Exclusive)?;
    tx.execute_batch(
        r#"
        ALTER TABLE backend_job_attempts ADD COLUMN worker_cleanup_state TEXT
            CHECK (worker_cleanup_state IN ('pending', 'executing', 'completed', 'failed'));
        ALTER TABLE backend_job_attempts ADD COLUMN worker_cleanup_failure_category TEXT;
        ALTER TABLE backend_job_attempts ADD COLUMN worker_cleanup_failure_detail TEXT;
        ALTER TABLE backend_job_attempts ADD COLUMN worker_cleanup_updated_at TEXT;
        ALTER TABLE backend_job_attempts ADD COLUMN worker_cleanup_completed_at TEXT;
        UPDATE backend_job_attempts
        SET worker_cleanup_state = CASE
                WHEN runtime_id IS NULL THEN 'completed'
                ELSE 'pending'
            END,
            worker_cleanup_updated_at = updated_at,
            worker_cleanup_completed_at = CASE
                WHEN runtime_id IS NULL THEN updated_at
                ELSE NULL
            END
        WHERE state IN ('completed', 'failed', 'unknown');
        CREATE INDEX backend_job_attempts_worker_cleanup
            ON backend_job_attempts(workspace_id, worker_cleanup_state, updated_at);
        "#,
    )?;
    tx.execute(
        "INSERT INTO __yoi_schema_migrations (version, name) VALUES (?1, ?2)",
        params![77_i64, BACKEND_JOB_WORKER_CLEANUP_MIGRATION_NAME],
    )?;
    tx.commit()?;
    Ok(())
}

fn migrate_workspace_config_v78_to_v79(conn: &Connection) -> Result<()> {
    if current_schema_version(conn)? != 78 {
        return Err(Error::Store(
            "Workspace config migration requires schema 78".into(),
        ));
    }
    let foreign_keys =
        conn.pragma_query_value(None, "foreign_keys", |row| row.get::<_, bool>(0))?;
    conn.execute_batch("PRAGMA foreign_keys = OFF;")?;
    let result = (|| {
        let tx = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Exclusive)?;
        let dependents = {
            let mut stmt = tx.prepare("SELECT sql FROM sqlite_schema WHERE tbl_name='workdir_registry' AND type IN ('index','trigger') AND sql IS NOT NULL")?;
            let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        let schema = include_str!("frozen_schema_v85.sql");
        let start = schema
            .find("CREATE TABLE \"workdir_registry\"")
            .ok_or_else(|| Error::Store("Missing Workdir schema".into()))?;
        let end = schema[start..]
            .find("CREATE TABLE external_workdir_grants")
            .ok_or_else(|| Error::Store("Missing Workdir schema end".into()))?
            + start;
        if column_exists(&tx, "workdir_registry", "workspace_config_grant_id")?
            && table_exists(&tx, "workspace_config_grants")?
        {
            tx.execute("INSERT INTO __yoi_schema_migrations(version,name) VALUES (79,'Workspace config grants and logical Workdirs')", [])?;
            tx.commit()?;
            return Ok(());
        }
        tx.execute_batch(include_str!("workspace_config_grants.sql"))?;
        tx.execute_batch(&schema[start..end].replace(
            "CREATE TABLE \"workdir_registry\"",
            "CREATE TABLE workdir_registry_v79",
        ))?;
        tx.execute_batch("INSERT INTO workdir_registry_v79 (workspace_id,workdir_id,display_name,source_kind,runtime_id,repository_id,external_grant_id,creation_selector,creation_ref,creation_tree,current_selector,current_ref,current_tree,observed_at_epoch_seconds,materialization_status,cleanliness,created_at,updated_at) SELECT workspace_id,workdir_id,display_name,source_kind,runtime_id,repository_id,external_grant_id,creation_selector,creation_ref,creation_tree,current_selector,current_ref,current_tree,observed_at_epoch_seconds,materialization_status,cleanliness,created_at,updated_at FROM workdir_registry; DROP TABLE workdir_registry; ALTER TABLE workdir_registry_v79 RENAME TO workdir_registry;")?;
        for sql in dependents {
            tx.execute_batch(&sql)?;
        }
        let broken: bool = {
            let mut stmt = tx.prepare("PRAGMA foreign_key_check")?;
            let mut rows = stmt.query([])?;
            rows.next()?.is_some()
        };
        if broken {
            return Err(Error::Store(
                "Workspace config migration foreign-key verification failed".into(),
            ));
        }
        tx.execute("INSERT INTO __yoi_schema_migrations(version,name) VALUES (79,'Workspace config grants and logical Workdirs')", [])?;
        tx.commit()?;
        Ok(())
    })();
    if foreign_keys {
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
    }
    result
}

fn migrate_workdir_connection_id_v77_to_v78(conn: &Connection) -> Result<()> {
    use rusqlite::TransactionBehavior;
    let current = current_schema_version(conn)?;
    if current != 77 {
        return Err(Error::Store(format!(
            "expected schema version 77 before {WORKDIR_CONNECTION_ID_MIGRATION_NAME} migration, found {current}"
        )));
    }
    let tx = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Exclusive)?;
    tx.execute_batch(
        "ALTER TABLE worker_workdir_links ADD COLUMN connection_id TEXT NOT NULL DEFAULT '';
         UPDATE worker_workdir_links SET connection_id = lower(hex(randomblob(16)));",
    )?;
    tx.execute(
        "INSERT INTO __yoi_schema_migrations (version, name) VALUES (?1, ?2)",
        params![78_i64, WORKDIR_CONNECTION_ID_MIGRATION_NAME],
    )?;
    tx.commit()?;
    Ok(())
}

fn migrate_workdir_credential_candidate_snapshots_v58_to_v59(conn: &Connection) -> Result<()> {
    let tx = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Exclusive)?;
    tx.execute_batch(
        r#"
        CREATE TABLE workdir_create_credential_candidates (
            workspace_id TEXT NOT NULL,
            operation_id TEXT NOT NULL,
            ordinal INTEGER NOT NULL CHECK (ordinal >= 0 AND ordinal < 2),
            role TEXT NOT NULL CHECK (role IN ('primary', 'workspace_default_fallback')),
            credential_id TEXT NOT NULL CHECK (length(credential_id) BETWEEN 1 AND 128),
            credential_revision INTEGER NOT NULL CHECK (credential_revision > 0),
            PRIMARY KEY (workspace_id, operation_id, ordinal),
            UNIQUE (workspace_id, operation_id, role),
            UNIQUE (workspace_id, operation_id, credential_id),
            FOREIGN KEY (workspace_id, operation_id)
                REFERENCES workdir_create_operations(workspace_id, operation_id)
                ON DELETE CASCADE
        );
        CREATE INDEX idx_workdir_create_credential_candidates_revision
            ON workdir_create_credential_candidates(
                workspace_id, credential_id, credential_revision
            );
        CREATE TABLE workdir_create_credential_revision_retentions (
            workspace_id TEXT NOT NULL,
            operation_id TEXT NOT NULL,
            ordinal INTEGER NOT NULL,
            credential_id TEXT NOT NULL,
            credential_revision INTEGER NOT NULL,
            PRIMARY KEY (workspace_id, operation_id, ordinal),
            FOREIGN KEY (workspace_id, operation_id, ordinal)
                REFERENCES workdir_create_credential_candidates(
                    workspace_id, operation_id, ordinal
                )
                ON DELETE CASCADE,
            FOREIGN KEY (workspace_id, credential_id, credential_revision)
                REFERENCES repository_ssh_credential_revisions(
                    workspace_id, credential_id, revision
                )
                ON DELETE RESTRICT
        );
        "#,
    )?;
    tx.execute(
        "INSERT INTO __yoi_schema_migrations (version, name) VALUES (?1, ?2)",
        params![59_i64, WORKDIR_CREDENTIAL_CANDIDATE_SNAPSHOT_MIGRATION_NAME],
    )?;
    tx.commit()?;
    Ok(())
}
