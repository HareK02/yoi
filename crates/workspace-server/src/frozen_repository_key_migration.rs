// Frozen schema-86 SSH cutover. No ordinal is used as a new authority token.
fn migrate_repository_keys_v86_to_v87(conn: &Connection) -> Result<()> {
    migrate_repository_keys_v86_to_v87_with_secret_source(
        conn,
        conn.path().filter(|path| !path.is_empty()).map(Path::new),
    )
}

// Validation may run against an in-memory backup. Its secret authority remains
// the original database's master key, not the candidate connection's location.
fn migrate_repository_keys_v86_to_v87_with_secret_source(
    conn: &Connection,
    secret_source: Option<&Path>,
) -> Result<()> {
    if current_schema_version(conn)? != 86 {
        return Err(Error::Store(
            "Repository key migration requires schema 86".into(),
        ));
    }
    if !conn.is_autocommit() {
        return Err(Error::Store(
            "Repository key migration requires an autocommit connection".into(),
        ));
    }
    let foreign_keys: bool = conn.query_row("PRAGMA foreign_keys", [], |r| r.get(0))?;
    let legacy_alter: bool = conn.query_row("PRAGMA legacy_alter_table", [], |r| r.get(0))?;
    conn.execute_batch("PRAGMA foreign_keys=OFF; PRAGMA legacy_alter_table=ON;")?;
    let result = (|| {
        let tx = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
        // Each stored key must resolve to one actual mutation receipt. Ambiguous or
        // missing evidence is not repaired by manufacturing an ID from its ordinal.
        tx.execute_batch(
            "CREATE TEMP TABLE repository_key_cutover (
            workspace_id TEXT NOT NULL, kind TEXT NOT NULL, resource_id TEXT NOT NULL,
            legacy_counter INTEGER NOT NULL, operation_id TEXT NOT NULL, fingerprint TEXT NOT NULL, created_at TEXT NOT NULL,
            PRIMARY KEY(workspace_id,kind,resource_id,legacy_counter));",
        )?;
        for (table, kind, id, fingerprint) in [
            (
                "repository_ssh_credential_revisions",
                "credential",
                "credential_id",
                "public_key_fingerprint",
            ),
            (
                "repository_ssh_host_trust_revisions",
                "host_trust",
                "host_trust_id",
                "fingerprint",
            ),
        ] {
            let rows = {
                let mut statement = tx.prepare(&format!(
                    "SELECT workspace_id,{id},revision,{fingerprint},created_at FROM {table}"
                ))?;
                statement
                    .query_map([], |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, i64>(2)?,
                            r.get::<_, String>(3)?,
                            r.get::<_, String>(4)?,
                        ))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?
            };
            for (workspace, resource, legacy_counter, fingerprint, created) in rows {
                let mut statement = tx.prepare("SELECT operation_id FROM repository_secret_operations WHERE workspace_id=?1 AND resource_kind=?2 AND resource_id=?3 AND result_revision=?4 AND created_at=?5")?;
                let matches = statement
                    .query_map(params![workspace, kind, resource, legacy_counter, created], |r| {
                        r.get::<_, String>(0)
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                if matches.len() != 1 || matches[0].is_empty() {
                    return Err(Error::Store("Repository key cutover requires one unambiguous mutation receipt per stored key".into()));
                }
                tx.execute(
                    "INSERT INTO repository_key_cutover VALUES(?1,?2,?3,?4,?5,?6,?7)",
                    params![
                        workspace,
                        kind,
                        resource,
                        legacy_counter,
                        matches[0],
                        fingerprint,
                        created
                    ],
                )?;
            }
        }
        // Current rows must agree with their pointed-to historical key, not merely
        // resolve to a nonempty receipt. FK checks alone cannot prove this identity.
        let inconsistent: bool = tx.query_row("SELECT EXISTS (
            SELECT 1 FROM repository_ssh_credentials c LEFT JOIN repository_ssh_credential_revisions k
            ON k.workspace_id=c.workspace_id AND k.credential_id=c.credential_id AND k.revision=c.current_revision
            WHERE k.revision IS NULL OR k.public_key_algorithm<>c.public_key_algorithm OR k.public_key_fingerprint<>c.public_key_fingerprint
            UNION ALL
            SELECT 1 FROM repository_ssh_host_trusts c LEFT JOIN repository_ssh_host_trust_revisions k
            ON k.workspace_id=c.workspace_id AND k.host_trust_id=c.host_trust_id AND k.revision=c.current_revision
            WHERE k.revision IS NULL OR k.hostname<>c.hostname OR k.port<>c.port OR k.key_algorithm<>c.key_algorithm OR k.host_key<>c.host_key OR k.fingerprint<>c.fingerprint
            UNION ALL
            SELECT 1 FROM workdir_create_credential_revision_retentions r LEFT JOIN workdir_create_credential_candidates c
            ON c.workspace_id=r.workspace_id AND c.operation_id=r.operation_id AND c.ordinal=r.ordinal
            WHERE c.ordinal IS NULL OR c.credential_id<>r.credential_id OR c.credential_revision<>r.credential_revision
        )", [], |r| r.get(0))?;
        if inconsistent {
            return Err(Error::Store(
                "Repository key cutover found inconsistent current or retained key evidence".into(),
            ));
        }
        for table in [
            "repository_secret_audit_events",
            "repository_secret_operations",
            "repository_ssh_credentials",
            "repository_ssh_credential_revisions",
            "repository_ssh_host_trusts",
            "repository_ssh_host_trust_revisions",
            "server_secret_versions",
            "workdir_create_operations",
            "workdir_create_credential_candidates",
            "workdir_create_credential_revision_retentions",
        ] {
            tx.execute_batch(&format!("ALTER TABLE {table} RENAME TO {table}_v86;"))?;
        }
        tx.execute_batch("DROP INDEX idx_workdir_create_credential_candidates_revision;")?;
        tx.execute_batch(include_str!("frozen_repository_key_schema.sql"))?;
        tx.execute_batch("INSERT INTO repository_secret_operations SELECT workspace_id,operation_id,request_fingerprint,resource_kind,resource_id,operation_id,created_at FROM repository_secret_operations_v86;
            INSERT INTO repository_ssh_credentials SELECT c.workspace_id,c.credential_id,c.name,c.public_key_algorithm,c.public_key_fingerprint,m.operation_id,c.status,c.created_at,c.rotated_at FROM repository_ssh_credentials_v86 c LEFT JOIN repository_key_cutover m ON m.workspace_id=c.workspace_id AND m.kind='credential' AND m.resource_id=c.credential_id AND m.legacy_counter=c.current_revision;
            INSERT INTO repository_ssh_credential_keys SELECT c.workspace_id,c.credential_id,m.operation_id,c.public_key_algorithm,c.public_key_fingerprint,c.created_at FROM repository_ssh_credential_revisions_v86 c LEFT JOIN repository_key_cutover m ON m.workspace_id=c.workspace_id AND m.kind='credential' AND m.resource_id=c.credential_id AND m.legacy_counter=c.revision;
            INSERT INTO repository_ssh_host_trusts SELECT c.workspace_id,c.host_trust_id,c.hostname,c.port,c.key_algorithm,c.host_key,c.fingerprint,m.operation_id,c.created_at,c.updated_at FROM repository_ssh_host_trusts_v86 c LEFT JOIN repository_key_cutover m ON m.workspace_id=c.workspace_id AND m.kind='host_trust' AND m.resource_id=c.host_trust_id AND m.legacy_counter=c.current_revision;
            INSERT INTO repository_ssh_host_trust_keys SELECT c.workspace_id,c.host_trust_id,m.operation_id,c.hostname,c.port,c.key_algorithm,c.host_key,c.fingerprint,c.created_at FROM repository_ssh_host_trust_revisions_v86 c LEFT JOIN repository_key_cutover m ON m.workspace_id=c.workspace_id AND m.kind='host_trust' AND m.resource_id=c.host_trust_id AND m.legacy_counter=c.revision;")?;
        let secrets = {
            let mut statement = tx.prepare("SELECT s.workspace_id,s.secret_id,s.revision,s.purpose,s.encryption_algorithm,s.nonce,s.ciphertext,s.created_at,m.operation_id FROM server_secret_versions_v86 s LEFT JOIN repository_key_cutover m ON m.workspace_id=s.workspace_id AND m.kind='credential' AND m.resource_id=s.secret_id AND m.legacy_counter=s.revision")?;
            statement
                .query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, u64>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, Vec<u8>>(5)?,
                        r.get::<_, Vec<u8>>(6)?,
                        r.get::<_, String>(7)?,
                        r.get::<_, String>(8)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        for (workspace, id, legacy_counter, purpose, algorithm, nonce, ciphertext, created, operation) in
            secrets
        {
            if algorithm != "aes-256-gcm-v1" {
                return Err(Error::Store(
                    "unsupported Repository secret envelope".into(),
                ));
            }
            let database = secret_source.ok_or_else(|| {
                Error::Store("Repository secret migration requires its database master key".into())
            })?;
            let (nonce, ciphertext) =
                crate::repository_access::migrate_legacy_repository_secret_envelope(
                    database,
                    &workspace,
                    &id,
                    legacy_counter,
                    &operation,
                    &purpose,
                    &nonce,
                    &ciphertext,
                )?;
            tx.execute(
                "INSERT INTO server_secret_objects VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
                params![
                    workspace, id, operation, purpose, algorithm, nonce, ciphertext, created
                ],
            )?;
        }
        let missing_secret: i64 = tx.query_row("SELECT count(*) FROM repository_ssh_credential_keys k WHERE NOT EXISTS (SELECT 1 FROM server_secret_objects s WHERE s.workspace_id=k.workspace_id AND s.secret_id=k.credential_id AND s.operation_id=k.operation_id AND s.purpose='private_key')", [], |r| r.get(0))?;
        if missing_secret != 0 {
            return Err(Error::Store(
                "Repository key migration found a missing private key envelope".into(),
            ));
        }
        // Audit events also require an actual receipt, including deletions whose key
        // material no longer exists. Keep their timestamps and actor attribution.
        let audits = {
            let mut statement=tx.prepare("SELECT workspace_id,event_id,kind,resource_id,revision,actor_account_id,created_at FROM repository_secret_audit_events_v86")?;
            statement
                .query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, i64>(4)?,
                        r.get::<_, String>(5)?,
                        r.get::<_, String>(6)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        for (workspace, event, kind, resource, legacy_counter, actor, created) in audits {
            let resource_kind = if kind.starts_with("credential_") {
                "credential"
            } else if kind.starts_with("host_trust_") {
                "host_trust"
            } else {
                return Err(Error::Store("unknown Repository audit kind".into()));
            };
            let mut statement=tx.prepare("SELECT operation_id FROM repository_secret_operations_v86 WHERE workspace_id=?1 AND resource_kind=?2 AND resource_id=?3 AND result_revision=?4 AND created_at=?5")?;
            let matches = statement
                .query_map(
                    params![workspace, resource_kind, resource, legacy_counter, created],
                    |r| r.get::<_, String>(0),
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            if matches.len() != 1 {
                return Err(Error::Store(
                    "Repository audit cutover requires one unambiguous mutation receipt".into(),
                ));
            }
            tx.execute(
                "INSERT INTO repository_secret_audit_events VALUES(?1,?2,?3,?4,?5,?6,?7)",
                params![workspace, event, kind, resource, matches[0], actor, created],
            )?;
        }
        // Successful unretained snapshots are history, not future lease input.
        // Archive all their SSH evidence, including counters, without interpreting
        // a reused credential ID/ordinal as a newer key. Failed creates are retryable.
        tx.execute_batch("CREATE TEMP TABLE workdir_terminal_archive_cutover AS
            SELECT o.workspace_id,o.operation_id FROM workdir_create_operations_v86 o
            WHERE o.state='succeeded'
            AND NOT EXISTS(SELECT 1 FROM workdir_create_credential_revision_retentions_v86 r WHERE r.workspace_id=o.workspace_id AND r.operation_id=o.operation_id)
            AND (o.credential_id IS NOT NULL OR o.host_trust_id IS NOT NULL OR EXISTS(SELECT 1 FROM workdir_create_credential_candidates_v86 c WHERE c.workspace_id=o.workspace_id AND c.operation_id=o.operation_id));
            INSERT INTO repository_secret_legacy_receipts
            SELECT e.workspace_id,e.operation_id,e.request_fingerprint,e.resource_kind,e.resource_id,e.result_revision,e.created_at,e.mutation_kind,
                CASE WHEN e.mutation_kind IN ('credential_created','host_trust_created') THEN 0
                     WHEN e.mutation_kind IN ('credential_rotated','host_trust_rotated') AND e.result_revision>1 THEN e.result_revision-1
                     WHEN e.mutation_kind IN ('credential_deleted','host_trust_deleted') THEN e.result_revision END,
                m.operation_id,m.fingerprint
            FROM (SELECT o.*, (SELECT min(a.kind) FROM repository_secret_audit_events_v86 a
                WHERE a.workspace_id=o.workspace_id AND a.resource_id=o.resource_id AND a.revision=o.result_revision AND a.created_at=o.created_at
                  AND ((o.resource_kind='credential' AND a.kind LIKE 'credential_%') OR (o.resource_kind='host_trust' AND a.kind LIKE 'host_trust_%'))
                HAVING count(*)=1) AS mutation_kind FROM repository_secret_operations_v86 o) e
            LEFT JOIN repository_key_cutover m ON m.workspace_id=e.workspace_id AND m.kind=e.resource_kind AND m.resource_id=e.resource_id
                AND m.created_at<=e.created_at
                AND m.legacy_counter=CASE WHEN e.mutation_kind IN ('credential_rotated','host_trust_rotated') THEN e.result_revision-1
                    WHEN e.mutation_kind IN ('credential_deleted','host_trust_deleted') THEN e.result_revision END;
            INSERT INTO workdir_create_operations
            SELECT o.workspace_id,o.operation_id,'workdir-create-v86:' || COALESCE(CAST(o.source_revision AS TEXT),'unknown') || ':' || o.request_fingerprint,
                o.repository_id,o.selector,o.requested_runtime_id,o.resolved_runtime_id,o.config_projection_digest,o.working_directory_id,o.state,o.failure,o.created_at,o.updated_at,
                o.source_kind,o.source_uri,o.source_fingerprint,
                CASE WHEN a.operation_id IS NULL THEN o.credential_id END,CASE WHEN a.operation_id IS NULL THEN c.fingerprint END,
                CASE WHEN a.operation_id IS NULL THEN o.host_trust_id END,CASE WHEN a.operation_id IS NULL THEN h.fingerprint END,
                CASE WHEN a.operation_id IS NULL THEN o.repository_access_mode END
            FROM workdir_create_operations_v86 o
            LEFT JOIN workdir_terminal_archive_cutover a ON a.workspace_id=o.workspace_id AND a.operation_id=o.operation_id
            LEFT JOIN repository_key_cutover c ON c.workspace_id=o.workspace_id AND c.kind='credential' AND c.resource_id=o.credential_id AND c.legacy_counter=o.credential_revision AND c.created_at<=o.updated_at
            LEFT JOIN repository_key_cutover h ON h.workspace_id=o.workspace_id AND h.kind='host_trust' AND h.resource_id=o.host_trust_id AND h.legacy_counter=o.host_trust_revision AND h.created_at<=o.updated_at;
            INSERT INTO workdir_create_credential_candidates
            SELECT c.workspace_id,c.operation_id,c.ordinal,c.role,c.credential_id,m.fingerprint
            FROM workdir_create_credential_candidates_v86 c JOIN workdir_create_operations_v86 o ON o.workspace_id=c.workspace_id AND o.operation_id=c.operation_id LEFT JOIN repository_key_cutover m
                ON m.workspace_id=c.workspace_id AND m.kind='credential' AND m.resource_id=c.credential_id AND m.legacy_counter=c.credential_revision AND m.created_at<=o.updated_at
            WHERE NOT EXISTS(SELECT 1 FROM workdir_terminal_archive_cutover a WHERE a.workspace_id=c.workspace_id AND a.operation_id=c.operation_id);
            INSERT INTO workdir_create_credential_retentions SELECT c.workspace_id,c.operation_id,c.ordinal,c.credential_id,m.fingerprint,m.operation_id FROM workdir_create_credential_revision_retentions_v86 c LEFT JOIN repository_key_cutover m ON m.workspace_id=c.workspace_id AND m.kind='credential' AND m.resource_id=c.credential_id AND m.legacy_counter=c.credential_revision;")?;
        let missing:i64=tx.query_row("SELECT count(*) FROM workdir_create_operations WHERE (credential_id IS NOT NULL AND credential_fingerprint IS NULL) OR (host_trust_id IS NOT NULL AND host_trust_fingerprint IS NULL)",[],|r|r.get(0))?;
        if missing != 0 {
            return Err(Error::Store(
                "Workdir SSH snapshot is missing key identity evidence".into(),
            ));
        }
        let archived = {
            let mut statement = tx.prepare(
                "SELECT workspace_id,operation_id FROM workdir_terminal_archive_cutover",
            )?;
            statement
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        for (workspace, operation) in archived {
            let old_operation=frozen_repository_json_rows(&tx,"SELECT * FROM workdir_create_operations_v86 WHERE workspace_id=?1 AND operation_id=?2",params![workspace,operation])?.pop().ok_or_else(|| Error::Store("archived Workdir operation disappeared".into()))?;
            let old_candidates = frozen_repository_json_rows(
                &tx,
                "SELECT * FROM workdir_create_credential_candidates_v86 WHERE workspace_id=?1 AND operation_id=?2 ORDER BY ordinal",
                params![workspace, operation],
            )?;
            tx.execute(
                "INSERT INTO workdir_create_legacy_ssh_archives VALUES(?1,?2,?3,?4)",
                params![
                    workspace,
                    operation,
                    serde_json::to_string(&old_operation)
                        .map_err(|e| Error::Store(e.to_string()))?,
                    serde_json::to_string(&old_candidates)
                        .map_err(|e| Error::Store(e.to_string()))?
                ],
            )?;
        }
        tx.execute_batch("DROP TABLE workdir_terminal_archive_cutover;")?;
        for table in [
            "workdir_create_credential_revision_retentions",
            "workdir_create_credential_candidates",
            "workdir_create_operations",
            "server_secret_versions",
            "repository_ssh_credential_revisions",
            "repository_ssh_host_trust_revisions",
            "repository_ssh_credentials",
            "repository_ssh_host_trusts",
            "repository_secret_operations",
            "repository_secret_audit_events",
        ] {
            tx.execute_batch(&format!("DROP TABLE {table}_v86;"))?;
        }
        tx.execute_batch("DROP TABLE repository_key_cutover;
            CREATE INDEX idx_repository_secret_audit_workspace_created ON repository_secret_audit_events(workspace_id,created_at,event_id);
            CREATE INDEX idx_repository_ssh_credentials_workspace_status ON repository_ssh_credentials(workspace_id,status,credential_id);
            CREATE INDEX idx_repository_ssh_host_trusts_workspace_host ON repository_ssh_host_trusts(workspace_id,hostname,port);")?;
        tx.execute_batch(
            r#"CREATE TRIGGER workdir_create_insert_blocked_by_runtime_removal
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

"#,
        )?;
        if tx.prepare("PRAGMA foreign_key_check")?.exists([])? {
            return Err(Error::Store(
                "Repository key migration foreign key check failed".into(),
            ));
        }
        if table_exists(&tx, "repositories")? {
            tx.execute_batch("ALTER TABLE repositories DROP COLUMN source_revision;")?;
        }
        tx.execute("INSERT INTO __yoi_schema_migrations(version,name) VALUES(87,'Repository SSH key identities and encrypted operation objects')",[])?;
        tx.commit()?;
        Ok(())
    })();
    let restore = conn
        .execute_batch(&format!(
            "PRAGMA legacy_alter_table={}; PRAGMA foreign_keys={};",
            i32::from(legacy_alter),
            i32::from(foreign_keys)
        ))
        .map_err(Error::from);
    result.and(restore)
}

// Preserve every SQLite field and NULL verbatim as typed JSON. Frozen archives
// are never read by credential lookup, lease construction, or a live CAS.
fn frozen_repository_json_rows(
    conn: &Connection,
    query: &str,
    params: &[&dyn rusqlite::ToSql],
) -> Result<Vec<serde_json::Value>> {
    let mut statement = conn.prepare(query)?;
    let columns = statement
        .column_names()
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
    let mut rows = statement.query(params)?;
    let mut result = Vec::new();
    while let Some(row) = rows.next()? {
        let mut object = serde_json::Map::new();
        for (index, name) in columns.iter().enumerate() {
            let value = match row.get_ref(index)? {
                rusqlite::types::ValueRef::Null => serde_json::Value::Null,
                rusqlite::types::ValueRef::Integer(value) => value.into(),
                rusqlite::types::ValueRef::Text(value) => serde_json::Value::String(
                    std::str::from_utf8(value)
                        .map_err(|e| Error::Store(e.to_string()))?
                        .to_string(),
                ),
                _ => {
                    return Err(Error::Store(
                        "unexpected value in frozen Repository archive".into(),
                    ));
                }
            };
            object.insert(name.clone(), value);
        }
        result.push(object.into());
    }
    Ok(result)
}
