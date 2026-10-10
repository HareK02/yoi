// Included in store::tests so both paths use the real retained migration chain.
#[test]
fn fresh_and_upgraded_state_contract_schemas_enforce_the_same_constraints() {
    fn signature(conn: &Connection, table: &str) -> Vec<Vec<String>> {
        let queries = [
            "SELECT name, type, CAST(\"notnull\" AS TEXT), COALESCE(dflt_value, '<NULL>'), CAST(pk AS TEXT) FROM pragma_table_info(?1) ORDER BY name",
            "SELECT \"table\", \"from\", \"to\", on_update, on_delete, match, CAST(seq AS TEXT) FROM pragma_foreign_key_list(?1) ORDER BY \"table\", \"from\", \"to\", seq",
            "SELECT CAST(i.\"unique\" AS TEXT), CAST(i.partial AS TEXT), (SELECT group_concat(name, ',') FROM (SELECT name FROM pragma_index_info(i.name) ORDER BY seqno)) FROM pragma_index_list(?1) AS i ORDER BY 1, 2, 3",
            "SELECT name, sql FROM sqlite_schema WHERE type='trigger' AND tbl_name=?1 ORDER BY name",
        ];
        queries
            .into_iter()
            .flat_map(|sql| {
                let mut statement = conn.prepare(sql).unwrap();
                let count = statement.column_count();
                statement
                    .query_map([table], |row| {
                        (0..count)
                            .map(|i| row.get::<_, String>(i))
                            .collect::<rusqlite::Result<Vec<_>>>()
                    })
                    .unwrap()
                    .map(|row| {
                        row.unwrap()
                            .into_iter()
                            .map(|value| {
                                value
                                    .chars()
                                    .filter(|c| !c.is_whitespace() && *c != '"')
                                    .collect()
                            })
                            .collect()
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    }
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("upgraded.db");
    prepare_retained_schema(&path, 85);
    let upgraded = SqliteWorkspaceStore::open(&path).unwrap();
    let fresh = SqliteWorkspaceStore::in_memory().unwrap();
    let mut mismatches = Vec::new();
    for table in [
        "artifacts",
        "workspace_config_trees",
        "workspace_config_tree_history",
        "workspace_memory_settings",
        "worker_create_reservations",
        "backend_jobs",
        "backend_job_attempts",
        "flow_sources",
        "flow_source_contents",
        "repositories",
        "workspace_runtime_bindings",
        "workspace_runtime_binding_verification_archive",
        "workspace_runtime_verification_archive",
        "workspace_runtime_binding_audit",
        "workspace_signing_identities",
        "workspace_signing_identity_provisioning_operations",
        "workspace_signing_identity_audit",
        "runtime_removal_operations",
        "workdir_removal_operations",
        "workspace_deletion_operations",
        "workspace_worker_retention_policies",
        "workspace_worker_retention_policy_snapshots",
        "worker_removal_operations",
        "worker_session_archives",
        "worker_diagnostics_archives",
        "worker_tombstones",
        "repository_secret_audit_events",
        "repository_secret_operations",
        "repository_ssh_credential_keys",
        "repository_ssh_credentials",
        "repository_ssh_host_trust_keys",
        "repository_ssh_host_trusts",
        "server_secret_objects",
        "workdir_create_operations",
        "workdir_create_credential_candidates",
        "workdir_create_credential_retentions",
        "workdir_create_legacy_ssh_archives",
        "repository_secret_legacy_receipts",
    ] {
        let expected = fresh.with_conn(|conn| Ok(signature(conn, table))).unwrap();
        let actual = upgraded
            .with_conn(|conn| Ok(signature(conn, table)))
            .unwrap();
        assert!(!expected.is_empty(), "missing canonical table {table}");
        if actual != expected {
            mismatches.push(format!(
                "{table}:\n  upgraded-only: {:?}\n  fresh-only: {:?}",
                actual
                    .iter()
                    .filter(|row| !expected.contains(row))
                    .collect::<Vec<_>>(),
                expected
                    .iter()
                    .filter(|row| !actual.contains(row))
                    .collect::<Vec<_>>()
            ));
        }
    }
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
}
