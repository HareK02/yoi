// Frozen schema-87 -> 88 authority cutover. Old counters are never new IDs.
// Existing private material is unchanged; v1 material has its own frozen decoder.
fn migrate_runtime_bindings_v87_to_v88(conn: &Connection) -> Result<()> {
    if current_schema_version(conn)? != 87 {
        return Err(Error::Store(
            "Runtime binding cutover requires schema 87".into(),
        ));
    }
    let foreign_keys: bool = conn.query_row("PRAGMA foreign_keys", [], |r| r.get(0))?;
    let legacy_alter: bool = conn.query_row("PRAGMA legacy_alter_table", [], |r| r.get(0))?;
    conn.execute_batch("PRAGMA foreign_keys=OFF; PRAGMA legacy_alter_table=ON;")?;
    let result = (|| -> Result<()> {
        let tx = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Exclusive)?;
        // An executing/failed removal may already have a durable Runtime receipt,
        // with the source gone but Backend metadata not yet committed. Its legacy
        // request fingerprint includes ordinal policy/Worker evidence that cannot
        // be reconstructed as a new content contract. Never retire that recovery
        // authority, regenerate the request, or treat failure as proof of no effect.
        let unresolved_removal: Option<(String, String)> = tx
            .query_row(
                "SELECT operation_id,state FROM worker_removal_operations \
                 WHERE state IN ('executing','failed') ORDER BY operation_id LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((operation_id, state)) = unresolved_removal {
            return Err(Error::Store(format!(
                "Runtime binding cutover requires old-version reconciliation of Worker removal `{operation_id}` ({state}); reconcile using schema 87 before retrying the upgrade"
            )));
        }
        // Triggers may refer to a table while it is being replaced. Preserve all of them,
        // including cross-domain removal fences, and restore before checking constraints.
        let triggers = {
            let mut stmt = tx.prepare("SELECT name,sql FROM sqlite_schema WHERE type='trigger'")?;
            stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let indexes = {
            let mut stmt = tx.prepare("SELECT name,tbl_name,sql FROM sqlite_schema WHERE type='index' AND sql IS NOT NULL")?;
            stmt.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
        };
        for (name, _) in &triggers {
            tx.execute_batch(&format!("DROP TRIGGER \"{}\"", name.replace('"', "\"\"")))?;
        }
        for table in [
            "workspace_signing_identities",
            "workspace_signing_identity_provisioning_operations",
            "workspace_signing_identity_audit",
        ] {
            let unsupported: bool = tx.query_row(
                &format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE revision != 1)"),
                [],
                |r| r.get(0),
            )?;
            if unsupported {
                return Err(Error::Store(
                    "unsupported legacy Workspace signing identity revision".into(),
                ));
            }
        }
        tx.execute_batch("CREATE TEMP TABLE runtime_binding_cutover_ids(workspace_id TEXT,runtime_id TEXT,ordinal INTEGER,binding_id TEXT,PRIMARY KEY(workspace_id,runtime_id,ordinal));
            CREATE TEMP TABLE retention_cutover_digests(workspace_id TEXT,policy_id TEXT,ordinal INTEGER,policy_digest TEXT,PRIMARY KEY(workspace_id,policy_id,ordinal));")?;
        let identities = {
            let mut stmt=tx.prepare("SELECT workspace_id,runtime_id,binding_revision FROM workspace_runtime_bindings UNION SELECT workspace_id,runtime_id,binding_revision FROM workspace_runtime_binding_audit")?;
            stmt.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
        };
        for (workspace, runtime, ordinal) in identities {
            if ordinal <= 0 {
                return Err(Error::Store("invalid legacy binding ordinal".into()));
            }
            tx.execute(
                "INSERT INTO runtime_binding_cutover_ids VALUES(?1,?2,?3,?4)",
                params![workspace, runtime, ordinal, new_runtime_binding_id()],
            )?;
        }
        let policies = {
            let mut stmt=tx.prepare("SELECT workspace_id,policy_id,revision,session_disposition,metadata_disposition,archive_retention_kind,archive_retention_seconds,diagnostics_disposition,diagnostics_retention_seconds FROM workspace_worker_retention_policy_revisions")?;
            stmt.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, Option<u64>>(6)?,
                    r.get::<_, String>(7)?,
                    r.get::<_, Option<u64>>(8)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
        };
        for (
            workspace,
            id,
            ordinal,
            session,
            metadata,
            archive,
            seconds,
            diagnostics,
            diagnostics_seconds,
        ) in policies
        {
            let archive = match (archive.as_str(), seconds) {
                ("forever", None) => serde_json::json!({"kind":"forever"}),
                ("for_seconds", Some(seconds)) if seconds > 0 => {
                    serde_json::json!({"kind":"for_seconds","seconds":seconds})
                }
                _ => return Err(Error::Store("invalid legacy retention duration".into())),
            };
            let policy: crate::retention::WorkerRetentionPolicyUpdate=serde_json::from_value(serde_json::json!({
                "policy_id":id,"session_disposition":session,"metadata_disposition":metadata,
                "archive_retention":archive,"diagnostics_disposition":diagnostics,"diagnostics_retention_seconds":diagnostics_seconds
            })).map_err(|e|Error::Store(format!("invalid legacy retention policy: {e}")))?;
            let digest = crate::retention::worker_retention_policy_digest(&policy);
            tx.execute(
                "INSERT INTO retention_cutover_digests VALUES(?1,?2,?3,?4)",
                params![workspace, policy.policy_id, ordinal, digest],
            )?;
        }
        tx.execute_batch(r#"CREATE TABLE workspace_runtime_bindings_v88 (
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
);"#)?;
        tx.execute_batch(r#"CREATE TABLE workspace_runtime_verifications_v88 (
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
);"#)?;
        tx.execute_batch(
            r#"CREATE TABLE workspace_runtime_binding_audit_v88 (
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
);"#,
        )?;
        tx.execute_batch(r#"CREATE TABLE workspace_signing_identities_v88 (
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
        );"#)?;
        tx.execute_batch(r#"CREATE TABLE workspace_signing_identity_provisioning_operations_v88 (
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
        );"#)?;
        tx.execute_batch(
            r#"CREATE TABLE workspace_signing_identity_audit_v88 (
            event_id TEXT PRIMARY KEY,
            workspace_id TEXT NOT NULL,
            key_id TEXT NOT NULL,
            action TEXT NOT NULL CHECK (action IN ('provisioned')),
            public_key_fingerprint TEXT NOT NULL,
            actor_account_id TEXT NOT NULL,
            created_at TEXT NOT NULL,
            FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE
        );"#,
        )?;
        tx.execute_batch(r#"CREATE TABLE runtime_removal_operations_v88 (
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
);"#)?;
        tx.execute_batch(r#"CREATE TABLE workspace_worker_retention_policies_v88 (
        workspace_id TEXT PRIMARY KEY, policy_id TEXT NOT NULL, policy_digest TEXT NOT NULL, updated_at TEXT NOT NULL,
        FOREIGN KEY(workspace_id,policy_id,policy_digest) REFERENCES workspace_worker_retention_policy_snapshots(workspace_id,policy_id,policy_digest),
        FOREIGN KEY(workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE);"#)?;
        tx.execute_batch(r#"CREATE TABLE workspace_worker_retention_policy_snapshots_v88 (
        workspace_id TEXT NOT NULL, policy_id TEXT NOT NULL, policy_digest TEXT NOT NULL CHECK(length(policy_digest)>0),
        session_disposition TEXT NOT NULL CHECK(session_disposition IN ('archive','purge')),
        metadata_disposition TEXT NOT NULL CHECK(metadata_disposition IN ('tombstone','purge')),
        archive_retention_kind TEXT NOT NULL CHECK(archive_retention_kind IN ('forever','for_seconds')),
        archive_retention_seconds INTEGER,
        diagnostics_disposition TEXT NOT NULL CHECK(diagnostics_disposition IN ('purge','retain')),
        diagnostics_retention_seconds INTEGER, created_at TEXT NOT NULL,
        PRIMARY KEY(workspace_id,policy_id,policy_digest),
        FOREIGN KEY(workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE);"#)?;
        tx.execute_batch(r#"CREATE TABLE worker_removal_operations_v88 (
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
        FOREIGN KEY(workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE);"#)?;
        tx.execute_batch(r#"CREATE TABLE worker_session_archives_v88 (
        archive_id TEXT PRIMARY KEY, workspace_id TEXT NOT NULL, runtime_id TEXT NOT NULL, worker_id TEXT NOT NULL,
        session_id TEXT NOT NULL, checksum_sha256 TEXT NOT NULL, content_bytes INTEGER NOT NULL,
        policy_id TEXT NOT NULL, policy_digest TEXT NOT NULL, operation_id TEXT NOT NULL UNIQUE,
        committed_at TEXT NOT NULL, expires_at TEXT,
        FOREIGN KEY(workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
        FOREIGN KEY(operation_id) REFERENCES worker_removal_operations(operation_id));"#)?;
        tx.execute_batch(
            r#"CREATE TABLE worker_diagnostics_archives_v88 (
        operation_id TEXT PRIMARY KEY, workspace_id TEXT NOT NULL, runtime_id TEXT NOT NULL,
        worker_id TEXT NOT NULL, policy_id TEXT NOT NULL, policy_digest TEXT NOT NULL,
        committed_at TEXT NOT NULL, expires_at TEXT NOT NULL,
        FOREIGN KEY(operation_id) REFERENCES worker_removal_operations(operation_id),
        FOREIGN KEY(workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE);"#,
        )?;
        tx.execute_batch(r#"CREATE TABLE worker_tombstones_v88 (
        workspace_id TEXT NOT NULL, runtime_id TEXT NOT NULL, worker_id TEXT NOT NULL,
        display_name TEXT NOT NULL, profile TEXT, worker_created_at TEXT NOT NULL, removed_at TEXT NOT NULL,
        archive_id TEXT, policy_id TEXT NOT NULL, policy_digest TEXT NOT NULL, operation_id TEXT NOT NULL UNIQUE,
        PRIMARY KEY(workspace_id,runtime_id,worker_id),
        FOREIGN KEY(workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
        FOREIGN KEY(archive_id) REFERENCES worker_session_archives(archive_id),
        FOREIGN KEY(operation_id) REFERENCES worker_removal_operations(operation_id));"#)?;
        tx.execute_batch(
            r#"CREATE TABLE workdir_removal_operations_v88 (
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
);"#,
        )?;
        tx.execute_batch(
            r#"CREATE TABLE workspace_deletion_operations_v88 (
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
);"#,
        )?;
        {
            let columns = table_columns(&tx, "workspace_runtime_bindings_v88")?;
            let mut expressions = Vec::new();
            for column in &columns {
                let expression=match column.as_str() {
                    "binding_id" => "(SELECT binding_id FROM runtime_binding_cutover_ids m WHERE m.workspace_id=o.workspace_id AND m.runtime_id=o.runtime_id AND m.ordinal=o.binding_revision)".to_string(),
                    "workspace_trust_id" => "NULL".to_string(),
                    "workspace_public_key_fingerprint" => "(SELECT public_key_fingerprint FROM workspace_signing_identities i WHERE i.workspace_id=o.workspace_id AND i.key_id=o.workspace_key_id)".to_string(),
                    "state" => "CASE WHEN state='verified' AND authentication_mode='workspace_identity' THEN 'configured' ELSE state END".to_string(),
                    _ => format!("o.{column}"),
                };
                expressions.push(expression);
            }
            tx.execute_batch(&format!("INSERT INTO workspace_runtime_bindings_v88 ({}) SELECT {} FROM workspace_runtime_bindings o",columns.join(","),expressions.join(",")))?;
        }
        // Legacy proof cannot attest to a new binding/trust identity.
        {
            let columns = table_columns(&tx, "workspace_runtime_binding_audit_v88")?;
            let mut expressions = Vec::new();
            for column in &columns {
                let expression=match column.as_str() {
                    "binding_id" => "(SELECT binding_id FROM runtime_binding_cutover_ids m WHERE m.workspace_id=o.workspace_id AND m.runtime_id=o.runtime_id AND m.ordinal=o.binding_revision)".to_string(),
                    _ => format!("o.{column}"),
                };
                expressions.push(expression);
            }
            tx.execute_batch(&format!("INSERT INTO workspace_runtime_binding_audit_v88 ({}) SELECT {} FROM workspace_runtime_binding_audit o",columns.join(","),expressions.join(",")))?;
        }
        {
            let columns = table_columns(&tx, "workspace_signing_identities_v88")?;
            let mut expressions = Vec::new();
            for column in &columns {
                let expression = match column.as_str() {
                    _ => format!("o.{column}"),
                };
                expressions.push(expression);
            }
            tx.execute_batch(&format!("INSERT INTO workspace_signing_identities_v88 ({}) SELECT {} FROM workspace_signing_identities o",columns.join(","),expressions.join(",")))?;
        }
        {
            let columns = table_columns(
                &tx,
                "workspace_signing_identity_provisioning_operations_v88",
            )?;
            let mut expressions = Vec::new();
            for column in &columns {
                let expression = match column.as_str() {
                    _ => format!("o.{column}"),
                };
                expressions.push(expression);
            }
            tx.execute_batch(&format!("INSERT INTO workspace_signing_identity_provisioning_operations_v88 ({}) SELECT {} FROM workspace_signing_identity_provisioning_operations o",columns.join(","),expressions.join(",")))?;
            migrate_existing_workspace_provisioning_receipts_v87(&tx)?;
        }
        {
            let columns = table_columns(&tx, "workspace_signing_identity_audit_v88")?;
            let mut expressions = Vec::new();
            for column in &columns {
                let expression = match column.as_str() {
                    _ => format!("o.{column}"),
                };
                expressions.push(expression);
            }
            tx.execute_batch(&format!("INSERT INTO workspace_signing_identity_audit_v88 ({}) SELECT {} FROM workspace_signing_identity_audit o",columns.join(","),expressions.join(",")))?;
        }
        {
            let columns = table_columns(&tx, "runtime_removal_operations_v88")?;
            let mut expressions = Vec::new();
            for column in &columns {
                let expression=match column.as_str() {
                    "expected_binding_id" => "COALESCE((SELECT binding_id FROM runtime_binding_cutover_ids m WHERE m.workspace_id=o.workspace_id AND m.runtime_id=o.runtime_id AND m.ordinal=o.expected_binding_revision),'RB-retired-' || lower(hex(randomblob(16))))".to_string(),
                    "config_digest" => "(SELECT content_digest FROM workspace_config_trees t WHERE t.workspace_id=o.workspace_id)".to_string(),
                    "state" => "CASE WHEN state IN ('pending','failed') THEN 'failed' ELSE state END".to_string(),
                    "failure_category" => "CASE WHEN state IN ('pending','failed') THEN 'binding_identity_cutover' ELSE failure_category END".to_string(),
                    _ => format!("o.{column}"),
                };
                expressions.push(expression);
            }
            tx.execute_batch(&format!("INSERT INTO runtime_removal_operations_v88 ({}) SELECT {} FROM runtime_removal_operations o",columns.join(","),expressions.join(",")))?;
        }
        {
            let columns = table_columns(&tx, "workspace_worker_retention_policies_v88")?;
            let mut expressions = Vec::new();
            for column in &columns {
                let expression=match column.as_str() {
                    "policy_digest" => "(SELECT policy_digest FROM retention_cutover_digests m WHERE m.workspace_id=o.workspace_id AND m.policy_id=o.policy_id AND m.ordinal=o.revision)".to_string(),
                    _ => format!("o.{column}"),
                };
                expressions.push(expression);
            }
            tx.execute_batch(&format!("INSERT INTO workspace_worker_retention_policies_v88 ({}) SELECT {} FROM workspace_worker_retention_policies o",columns.join(","),expressions.join(",")))?;
        }
        {
            let columns = table_columns(&tx, "workspace_worker_retention_policy_snapshots_v88")?;
            let mut expressions = Vec::new();
            for column in &columns {
                let expression=match column.as_str() {
                    "policy_digest" => "(SELECT policy_digest FROM retention_cutover_digests m WHERE m.workspace_id=o.workspace_id AND m.policy_id=o.policy_id AND m.ordinal=o.revision)".to_string(),
                    _ => format!("o.{column}"),
                };
                expressions.push(expression);
            }
            tx.execute_batch(&format!("INSERT INTO workspace_worker_retention_policy_snapshots_v88 ({}) SELECT {} FROM workspace_worker_retention_policy_revisions o WHERE 1 ON CONFLICT(workspace_id,policy_id,policy_digest) DO NOTHING",columns.join(","),expressions.join(",")))?;
        }
        {
            let columns = table_columns(&tx, "worker_removal_operations_v88")?;
            let mut expressions = Vec::new();
            for column in &columns {
                let expression=match column.as_str() {
                    "policy_digest" => "(SELECT policy_digest FROM retention_cutover_digests m WHERE m.workspace_id=o.workspace_id AND m.policy_id=o.policy_id AND m.ordinal=o.policy_revision)".to_string(),
                    "worker_updated_at" => "worker_revision".to_string(),
                    // Only unexecuted plans lose admission authority. Executing/failed
                    // rows were rejected above; succeeded and already-stale evidence stays intact.
                    "state" => "CASE WHEN state IN ('planned','blocked') THEN 'stale' ELSE state END".to_string(),
                    "failure_category" => "CASE WHEN state IN ('planned','blocked') THEN 'policy_content_cutover' ELSE failure_category END".to_string(),
                    _ => format!("o.{column}"),
                };
                expressions.push(expression);
            }
            tx.execute_batch(&format!("INSERT INTO worker_removal_operations_v88 ({}) SELECT {} FROM worker_removal_operations o",columns.join(","),expressions.join(",")))?;
        }
        {
            let columns = table_columns(&tx, "worker_session_archives_v88")?;
            let mut expressions = Vec::new();
            for column in &columns {
                let expression=match column.as_str() {
                    "policy_digest" => "(SELECT policy_digest FROM retention_cutover_digests m WHERE m.workspace_id=o.workspace_id AND m.policy_id=o.policy_id AND m.ordinal=o.policy_revision)".to_string(),
                    _ => format!("o.{column}"),
                };
                expressions.push(expression);
            }
            tx.execute_batch(&format!("INSERT INTO worker_session_archives_v88 ({}) SELECT {} FROM worker_session_archives o",columns.join(","),expressions.join(",")))?;
        }
        {
            let columns = table_columns(&tx, "worker_diagnostics_archives_v88")?;
            let mut expressions = Vec::new();
            for column in &columns {
                let expression=match column.as_str() {
                    "policy_digest" => "(SELECT policy_digest FROM retention_cutover_digests m WHERE m.workspace_id=o.workspace_id AND m.policy_id=o.policy_id AND m.ordinal=o.policy_revision)".to_string(),
                    _ => format!("o.{column}"),
                };
                expressions.push(expression);
            }
            tx.execute_batch(&format!("INSERT INTO worker_diagnostics_archives_v88 ({}) SELECT {} FROM worker_diagnostics_archives o",columns.join(","),expressions.join(",")))?;
        }
        {
            let columns = table_columns(&tx, "worker_tombstones_v88")?;
            let mut expressions = Vec::new();
            for column in &columns {
                let expression=match column.as_str() {
                    "policy_digest" => "(SELECT policy_digest FROM retention_cutover_digests m WHERE m.workspace_id=o.workspace_id AND m.policy_id=o.policy_id AND m.ordinal=o.policy_revision)".to_string(),
                    _ => format!("o.{column}"),
                };
                expressions.push(expression);
            }
            tx.execute_batch(&format!(
                "INSERT INTO worker_tombstones_v88 ({}) SELECT {} FROM worker_tombstones o",
                columns.join(","),
                expressions.join(",")
            ))?;
        }
        {
            let columns = table_columns(&tx, "workdir_removal_operations_v88")?;
            let mut expressions = Vec::new();
            for column in &columns {
                let expression=match column.as_str() {
                    // This is a Backend-local callback fence, not Runtime receipt
                    // identity. Recovery still proves the prior owner is orphaned,
                    // observes the exact materialization (or authoritative absence),
                    // and uses the unchanged request/materialization fingerprints.
                    "attempt_id" => "CASE WHEN attempt_count>0 THEN 'WDRA-' || lower(hex(randomblob(16))) ELSE NULL END".to_string(),
                    _ => format!("o.{column}"),
                };
                expressions.push(expression);
            }
            tx.execute_batch(&format!("INSERT INTO workdir_removal_operations_v88 ({}) SELECT {} FROM workdir_removal_operations o",columns.join(","),expressions.join(",")))?;
        }
        {
            let columns = table_columns(&tx, "workspace_deletion_operations_v88")?;
            let mut expressions = Vec::new();
            for column in &columns {
                let expression = match column.as_str() {
                    "workspace_updated_at" => "workspace_revision".to_string(),
                    _ => format!("o.{column}"),
                };
                expressions.push(expression);
            }
            tx.execute_batch(&format!("INSERT INTO workspace_deletion_operations_v88 ({}) SELECT {} FROM workspace_deletion_operations o",columns.join(","),expressions.join(",")))?;
        }
        tx.execute_batch("DROP TABLE workspace_runtime_bindings; ALTER TABLE workspace_runtime_bindings_v88 RENAME TO workspace_runtime_bindings;")?;
        tx.execute_batch("DROP TABLE workspace_runtime_verifications; ALTER TABLE workspace_runtime_verifications_v88 RENAME TO workspace_runtime_verifications;")?;
        tx.execute_batch("DROP TABLE workspace_runtime_binding_audit; ALTER TABLE workspace_runtime_binding_audit_v88 RENAME TO workspace_runtime_binding_audit;")?;
        tx.execute_batch("DROP TABLE workspace_signing_identities; ALTER TABLE workspace_signing_identities_v88 RENAME TO workspace_signing_identities;")?;
        tx.execute_batch("DROP TABLE workspace_signing_identity_provisioning_operations; ALTER TABLE workspace_signing_identity_provisioning_operations_v88 RENAME TO workspace_signing_identity_provisioning_operations;")?;
        tx.execute_batch("DROP TABLE workspace_signing_identity_audit; ALTER TABLE workspace_signing_identity_audit_v88 RENAME TO workspace_signing_identity_audit;")?;
        tx.execute_batch("DROP TABLE runtime_removal_operations; ALTER TABLE runtime_removal_operations_v88 RENAME TO runtime_removal_operations;")?;
        tx.execute_batch("DROP TABLE workspace_worker_retention_policies; ALTER TABLE workspace_worker_retention_policies_v88 RENAME TO workspace_worker_retention_policies;")?;
        tx.execute_batch("DROP TABLE workspace_worker_retention_policy_revisions; ALTER TABLE workspace_worker_retention_policy_snapshots_v88 RENAME TO workspace_worker_retention_policy_snapshots;")?;
        tx.execute_batch("DROP TABLE worker_removal_operations; ALTER TABLE worker_removal_operations_v88 RENAME TO worker_removal_operations;")?;
        tx.execute_batch("DROP TABLE worker_session_archives; ALTER TABLE worker_session_archives_v88 RENAME TO worker_session_archives;")?;
        tx.execute_batch("DROP TABLE worker_diagnostics_archives; ALTER TABLE worker_diagnostics_archives_v88 RENAME TO worker_diagnostics_archives;")?;
        tx.execute_batch("DROP TABLE worker_tombstones; ALTER TABLE worker_tombstones_v88 RENAME TO worker_tombstones;")?;
        tx.execute_batch("DROP TABLE workdir_removal_operations; ALTER TABLE workdir_removal_operations_v88 RENAME TO workdir_removal_operations;")?;
        tx.execute_batch("DROP TABLE workspace_deletion_operations; ALTER TABLE workspace_deletion_operations_v88 RENAME TO workspace_deletion_operations;")?;
        tx.execute_batch(
            r#"CREATE INDEX workspace_runtime_verifications_state_idx
    ON workspace_runtime_verifications(workspace_id, state, checked_at DESC);"#,
        )?;
        tx.execute_batch(
            r#"CREATE INDEX idx_workspace_runtime_binding_audit_recent
    ON workspace_runtime_binding_audit(workspace_id, runtime_id, at DESC, binding_id DESC);"#,
        )?;
        tx.execute_batch(
            r#"CREATE INDEX idx_workspace_runtime_bindings_workspace
            ON workspace_runtime_bindings(workspace_id, revoked_at, runtime_id);"#,
        )?;
        tx.execute_batch(
            r#"CREATE INDEX workspace_deletion_operations_workspace_recent
    ON workspace_deletion_operations(workspace_id, created_at DESC);"#,
        )?;
        tx.execute_batch(
            r#"CREATE UNIQUE INDEX idx_workdir_removal_operations_one_pending
    ON workdir_removal_operations(workspace_id, workdir_id)
    WHERE state = 'pending';"#,
        )?;
        // Config content can be evaluated under multiple toolchains/schema bundles.
        // Preserve their distinct projection evidence rather than overwriting same-content rows.
        let has_history: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='workspace_config_tree_history')", [], |r|r.get(0))?;
        if has_history {
            tx.execute_batch(r#"CREATE TABLE workspace_config_tree_history_v88 (
                workspace_id TEXT NOT NULL, content_digest TEXT NOT NULL,
                toolchain_fingerprint TEXT NOT NULL, projection_digest TEXT NOT NULL,
                manifest_json TEXT NOT NULL, created_at TEXT NOT NULL, schema_bundle_json TEXT NOT NULL DEFAULT '{"contributions":[],"source":"{}","fingerprint":""}',
                PRIMARY KEY(workspace_id,content_digest,toolchain_fingerprint,projection_digest),
                FOREIGN KEY(workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE);
                INSERT INTO workspace_config_tree_history_v88 SELECT workspace_id,content_digest,toolchain_fingerprint,projection_digest,manifest_json,created_at,schema_bundle_json FROM workspace_config_tree_history;
                DROP TABLE workspace_config_tree_history;
                ALTER TABLE workspace_config_tree_history_v88 RENAME TO workspace_config_tree_history;"#)?;
        }
        for (name, table, sql) in indexes {
            let exists: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='index' AND name=?1)",
                [&name],
                |r| r.get(0),
            )?;
            if !exists {
                let sql = if table == "workspace_runtime_binding_audit" {
                    sql.replace("binding_revision DESC", "at DESC, binding_id DESC")
                        .replace("binding_revision", "binding_id")
                } else {
                    sql
                };
                tx.execute_batch(&sql)?;
            }
        }
        for (name, sql) in triggers {
            if name == "seed_worker_retention_policy_after_workspace_insert" {
                continue;
            }
            tx.execute_batch(&sql)?;
        }
        tx.execute_batch(r#"CREATE TRIGGER seed_worker_retention_policy_after_workspace_insert AFTER INSERT ON workspaces BEGIN
        INSERT INTO workspace_worker_retention_policy_snapshots
          (workspace_id,policy_id,policy_digest,session_disposition,metadata_disposition,archive_retention_kind,archive_retention_seconds,diagnostics_disposition,diagnostics_retention_seconds,created_at)
          VALUES(NEW.workspace_id,'workspace-default-conservative','sha256:30d45d5d58a10dadf95c3bf831e589ea10c81dc76da9575bfd9fb13942b3d198','archive','tombstone','forever',NULL,'purge',NULL,NEW.created_at);
        INSERT INTO workspace_worker_retention_policies(workspace_id,policy_id,policy_digest,updated_at)
          VALUES(NEW.workspace_id,'workspace-default-conservative','sha256:30d45d5d58a10dadf95c3bf831e589ea10c81dc76da9575bfd9fb13942b3d198',NEW.created_at);
      END;"#)?;
        tx.execute_batch(
            "DROP TABLE runtime_binding_cutover_ids; DROP TABLE retention_cutover_digests;",
        )?;
        let violation: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_foreign_key_check)",
            [],
            |r| r.get(0),
        )?;
        if violation {
            return Err(Error::Store(
                "Runtime binding cutover foreign-key violation".into(),
            ));
        }
        verify_runtime_binding_content_schema(&tx)?;
        tx.execute("INSERT INTO __yoi_schema_migrations(version,name) VALUES(88,'Runtime binding identities and retention content contracts')",[])?;
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

// Only the reservation lookup contract changes. Keep the same reserved key,
// material reference, actor, state and timestamps, including completed receipts.
// workspace_create receipts have an independent request contract and stay opaque.
fn migrate_existing_workspace_provisioning_receipts_v87(conn: &Connection) -> Result<()> {
    let receipts = {
        let mut stmt = conn.prepare(
            "SELECT operation_key,request_fingerprint,workspace_id,key_id,private_material_ref,state,completed_at \
             FROM workspace_signing_identity_provisioning_operations WHERE operation_kind='existing_workspace'",
        )?;
        stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, Option<String>>(6)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for (old_operation_key, old_fingerprint, workspace, key, material_ref, state, completed_at) in
        receipts
    {
        let identity: Option<(String, String, String)> = conn
            .query_row(
                "SELECT key_id,private_material_ref,state FROM workspace_signing_identities WHERE workspace_id=?1",
                [&workspace],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let expected_state = if state == "pending" {
            "pending_provisioning"
        } else {
            "active"
        };
        if old_operation_key != format!("existing-workspace:{workspace}:revision-1")
            || old_fingerprint
                != frozen_existing_workspace_provisioning_fingerprint_v1(&workspace, &key)
            || identity != Some((key.clone(), material_ref.clone(), expected_state.into()))
            || (state == "completed") != completed_at.is_some()
        {
            return Err(Error::Store(format!(
                "inconsistent legacy Workspace signing identity provisioning receipt `{old_operation_key}`"
            )));
        }
        conn.execute(
            "UPDATE workspace_signing_identity_provisioning_operations_v88 SET operation_key=?2,request_fingerprint=?3 WHERE operation_key=?1",
            params![
                old_operation_key,
                format!("existing-workspace:{workspace}:key-{key}"),
                crate::workspace_signing_identity::provisioning_fingerprint(&workspace, &key, &material_ref),
            ],
        )?;
    }
    Ok(())
}

fn frozen_existing_workspace_provisioning_fingerprint_v1(workspace: &str, key: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(workspace.as_bytes());
    hasher.update([0]);
    hasher.update(key.as_bytes());
    hasher.update([0]);
    hasher.update(1_u64.to_be_bytes());
    let digest: String = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("sha256:{digest}")
}

fn verify_runtime_binding_content_schema(conn: &Connection) -> Result<()> {
    for (table, required, forbidden) in [
        (
            "workspace_runtime_bindings",
            "binding_id",
            "binding_revision",
        ),
        (
            "workspace_runtime_verifications",
            "workspace_public_key_fingerprint",
            "runtime_identity_revision",
        ),
        ("workspace_signing_identities", "key_id", "revision"),
        (
            "runtime_removal_operations",
            "config_digest",
            "config_revision",
        ),
        (
            "workspace_worker_retention_policy_snapshots",
            "policy_digest",
            "revision",
        ),
        ("workdir_removal_operations", "attempt_id", "attempt_count"),
        (
            "workspace_deletion_operations",
            "workspace_updated_at",
            "workspace_revision",
        ),
    ] {
        let columns = table_columns(conn, table)?;
        if !columns.iter().any(|v| v == required) || columns.iter().any(|v| v == forbidden) {
            return Err(Error::Store(format!(
                "{table} does not match content/identity authority schema"
            )));
        }
    }
    Ok(())
}
