// Included in store::tests. Exercise the production migration entrypoints on
// isolated SQLite files; both published branches are built from retained DDL.
mod schema_convergence {
    use super::*;
    use std::path::PathBuf;

    fn variant_fixture(job_variant: bool, baseline_only: bool) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("schema-103.sqlite");
        prepare_retained_schema(&path, 102);
        let conn = Connection::open(&path).unwrap();
        configure_sqlite(&conn).unwrap();
        if job_variant {
            (JOB_INPUT_RETENTION_SCHEMA_103.apply)(&conn).unwrap();
        } else {
            migrate_passkey_results_v102_to_v103(&conn).unwrap();
        }
        if baseline_only {
            conn.execute_batch("DELETE FROM __yoi_schema_migrations;
                INSERT INTO __yoi_schema_migrations(version,name) VALUES(103,'workspace schema baseline');").unwrap();
        }
        conn.execute_batch(r#"
            INSERT INTO accounts(account_id,kind,handle,display_name,created_at,updated_at)
                VALUES('owner','user','owner','Owner','1','2');
            INSERT INTO users(user_id,account_id,handle,display_name,created_at,updated_at)
                VALUES('user','owner','owner','Owner','1','2');
            INSERT INTO workspaces(workspace_id,owner_account_id,display_name,state,created_at,updated_at)
                VALUES('workspace','owner','Workspace','active','1','2');
            INSERT INTO passkey_credentials(credential_id,user_id,public_key_cose,transports_json,sign_count,created_at,last_used_at)
                VALUES('key','user','preserved-public-key','["internal"]',9,'1','2');
        "#).unwrap();
        if !job_variant {
            conn.execute_batch("UPDATE passkey_metadata SET key_reference='stable-reference',normal_login_verified_at='verified';
                INSERT INTO passkey_addition_receipts(challenge_id,credential_id,user_id,authentication_reference)
                VALUES('challenge','key','user','saved-authentication-reference');").unwrap();
        }
        for disposition in ["available", "reclaimed", "erased", "invalidated"] {
            let request = resource_job_request(disposition, disposition, "candidate");
            conn.execute("INSERT INTO backend_jobs(workspace_id,job_id,purpose,input_digest,input_ref,request_json,intent_fingerprint,state,current_attempt,result_json,result_digest,created_at,updated_at,completed_at)
                VALUES('workspace',?1,?2,?3,?4,?5,?6,'completed',1,'{\"saved\":true}','result-digest','1','2','2')",
                params![request.job_id, request.purpose, request.input_digest().unwrap(), request.input_ref, serde_json::to_string(&request).unwrap(), request.fingerprint().unwrap()]).unwrap();
            conn.execute("INSERT INTO backend_job_attempts(workspace_id,job_id,attempt_id,attempt,input_digest,state,deadline_at,result_json,result_digest,created_at,updated_at,completed_at)
                VALUES('workspace',?1,?1,1,?2,'completed','deadline','{\"saved\":true}','result-digest','1','2','2')",
                params![request.job_id, request.input_digest().unwrap()]).unwrap();
            if job_variant && disposition != "available" {
                conn.execute("UPDATE backend_jobs SET request_json=json_set(request_json,'$.input',json('null')),input_disposition=?1 WHERE job_id=?1", [disposition]).unwrap();
            }
        }
        verify_schema_history(&conn, 103).unwrap();
        (dir, path)
    }

    fn feature_rows(conn: &Connection, job_variant: bool) -> Vec<Vec<Vec<rusqlite::types::Value>>> {
        let mut tables = vec!["passkey_credentials", "backend_job_attempts"];
        if !job_variant {
            tables.extend(["passkey_metadata", "passkey_addition_receipts"]);
        }
        let mut rows: Vec<_> = tables
            .into_iter()
            .map(|table| stored_fixture_rows(conn, table))
            .collect();
        // A Passkey-branch Job gains only input_disposition; all old columns must
        // remain byte-for-byte equal, including body, digest and result receipts.
        rows.push(fixture_query_rows(conn, "SELECT workspace_id,job_id,purpose,input_digest,input_ref,resource_key,request_json,intent_fingerprint,state,current_attempt,result_json,result_digest,failure_category,failure_detail,created_at,updated_at,completed_at FROM backend_jobs ORDER BY job_id"));
        if job_variant {
            rows.push(fixture_query_rows(
                conn,
                "SELECT job_id,input_disposition FROM backend_jobs ORDER BY job_id",
            ));
        }
        rows
    }

    fn schema_rows(conn: &Connection) -> Vec<Vec<rusqlite::types::Value>> {
        fixture_query_rows(
            conn,
            "SELECT type,name,tbl_name,sql FROM sqlite_schema ORDER BY type,name",
        )
    }

    #[test]
    fn schema_103_variants_converge_preserving_history_values_and_constraints() {
        for job_variant in [false, true] {
            for baseline_only in [false, true] {
                let (_dir, path) = variant_fixture(job_variant, baseline_only);
                let conn = Connection::open(&path).unwrap();
                let history = workspace_schema_migration_history(&conn).unwrap();
                let rows = feature_rows(&conn, job_variant);
                let schema = schema_rows(&conn);
                let plan = SqliteWorkspaceStore::migration_plan(&path).unwrap();
                assert_eq!(plan.current_schema_version, 103);
                assert_eq!(
                    plan.migrations,
                    vec![
                        WorkspaceSchemaMigrationStep {
                            version: 104,
                            name: "Converge Passkey results and Job input retention".into(),
                        },
                        WorkspaceSchemaMigrationStep {
                            version: 105,
                            name: "Durable Subject deletion snapshots and Job admission fences".into(),
                        },
                        WorkspaceSchemaMigrationStep {
                            version: 106,
                            name: crate::plugin_catalog::MIGRATION_NAME.into(),
                        },
                        WorkspaceSchemaMigrationStep {
                            version: 107,
                            name: "Immutable Worker Plugin plans and retained pins".into(),
                        },
                        WorkspaceSchemaMigrationStep {
                            version: 108,
                            name: "Subject Ticket responsibility and current-body notification routing".into(),
                        },
                        WorkspaceSchemaMigrationStep {
                            version: 109,
                            name: "Durable Plugin Host invocations and HTTPS effect receipts".into(),
                        },
                    ]
                );
                assert_eq!(
                    schema_rows(&conn),
                    schema,
                    "preflight must not mutate the source"
                );
                assert_eq!(feature_rows(&conn, job_variant), rows);
                drop(conn);
                let upgraded = SqliteWorkspaceStore::open_for_test(&path).unwrap();
                let fresh = SqliteWorkspaceStore::in_memory_for_test().unwrap();
                upgraded.with_conn(|conn| {
                    assert_eq!(feature_rows(conn, job_variant), rows);
                    let mut expected = history.clone();
                    expected.push((104, "Converge Passkey results and Job input retention".into()));
                    expected.push((105, "Durable Subject deletion snapshots and Job admission fences".into()));
                    expected.push((106, crate::plugin_catalog::MIGRATION_NAME.into()));
                    expected.push((107, "Immutable Worker Plugin plans and retained pins".into()));
                    expected.push((108, "Subject Ticket responsibility and current-body notification routing".into()));
                    expected.push((109, "Durable Plugin Host invocations and HTTPS effect receipts".into()));
                    assert_eq!(workspace_schema_migration_history(conn)?, expected);
                    for table in ["passkey_credentials", "passkey_metadata", "passkey_addition_receipts", "backend_jobs", "backend_job_attempts"] {
                        assert_eq!(state_contract_schema_signature(conn, table), fresh.with_conn(|c| Ok(state_contract_schema_signature(c, table)))?, "{table}");
                    }
                    assert_eq!(conn.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| r.get::<_, i64>(0))?, 0);
                    assert!(conn.execute("UPDATE backend_jobs SET input_digest='changed'", []).is_err());
                    assert!(conn.execute("UPDATE backend_jobs SET input_disposition='unsupported'", []).is_err());
                    assert!(conn.execute("UPDATE backend_jobs SET request_json=json_set(request_json,'$.input','replacement')", []).is_err());
                    assert!(conn.execute("INSERT INTO passkey_metadata(credential_id,key_reference) VALUES('missing','new')", []).is_err());
                    assert!(conn.execute("INSERT INTO passkey_addition_receipts VALUES('other','missing','user','reference')", []).is_err());
                    let reference: (String, Option<String>) = conn.query_row("SELECT key_reference,normal_login_verified_at FROM passkey_metadata WHERE credential_id='key'", [], |r| Ok((r.get(0)?,r.get(1)?)))?;
                    if job_variant {
                        assert_eq!(reference.0.len(), 32);
                        assert_eq!(reference.1, None);
                    } else {
                        assert_eq!(reference, ("stable-reference".into(), Some("verified".into())));
                        assert_eq!(conn.query_row("SELECT count(*) FROM backend_jobs WHERE input_disposition='available'", [], |r| r.get::<_, i64>(0))?, 4);
                    }
                    Ok(())
                }).unwrap();
                let summaries = upgraded.list_passkey_summaries("user").unwrap();
                drop(upgraded);
                let reopened = SqliteWorkspaceStore::open_for_test(&path).unwrap();
                assert_eq!(reopened.list_passkey_summaries("user").unwrap(), summaries);
                reopened
                    .with_conn(|conn| {
                        assert_eq!(feature_rows(conn, job_variant), rows);
                        assert_eq!(
                            workspace_schema_migration_history(conn)?.len(),
                            history.len() + plan.migrations.len()
                        );
                        Ok(())
                    })
                    .unwrap();
            }
        }
    }

    #[test]
    fn schema_103_convergence_failure_rolls_back_both_variants_and_retries() {
        for job_variant in [false, true] {
            for baseline_only in [false, true] {
                let (_dir, path) = variant_fixture(job_variant, baseline_only);
                let conn = Connection::open(&path).unwrap();
                conn.execute_batch("CREATE TRIGGER interrupt_convergence BEFORE INSERT ON __yoi_schema_migrations WHEN NEW.version=104 BEGIN SELECT RAISE(ABORT,'injected convergence failure'); END;").unwrap();
                let schema = schema_rows(&conn);
                let history = workspace_schema_migration_history(&conn).unwrap();
                let rows = feature_rows(&conn, job_variant);
                drop(conn);
                let error = SqliteWorkspaceStore::open_for_test(&path)
                    .err()
                    .expect("migration must fail");
                assert!(
                    error.to_string().contains("injected convergence failure"),
                    "{error}"
                );
                let conn = Connection::open(&path).unwrap();
                assert_eq!(schema_rows(&conn), schema);
                assert_eq!(workspace_schema_migration_history(&conn).unwrap(), history);
                assert_eq!(feature_rows(&conn, job_variant), rows);
                conn.execute_batch("DROP TRIGGER interrupt_convergence")
                    .unwrap();
                drop(conn);
                let upgraded = SqliteWorkspaceStore::open_for_test(&path).unwrap();
                upgraded
                    .with_conn(|conn| {
                        assert_eq!(feature_rows(conn, job_variant), rows);
                        assert_eq!(current_schema_version(conn)?, LATEST_SCHEMA_VERSION);
                        Ok(())
                    })
                    .unwrap();
            }
        }
    }

    #[test]
    fn schema_103_unknown_migration_identity_is_rejected_without_changes() {
        for job_variant in [false, true] {
            let (_dir, path) = variant_fixture(job_variant, false);
            let conn = Connection::open(&path).unwrap();
            conn.execute("UPDATE __yoi_schema_migrations SET name='unrecognized schema 103' WHERE version=103", []).unwrap();
            let schema = schema_rows(&conn);
            let history = workspace_schema_migration_history(&conn).unwrap();
            let rows = feature_rows(&conn, job_variant);
            assert!(SqliteWorkspaceStore::migration_plan(&path).is_err());
            assert!(SqliteWorkspaceStore::open_for_test(&path).is_err());
            assert_eq!(schema_rows(&conn), schema);
            assert_eq!(workspace_schema_migration_history(&conn).unwrap(), history);
            assert_eq!(feature_rows(&conn, job_variant), rows);
        }
    }

    #[test]
    fn schema_103_mismatched_identity_or_incomplete_features_are_rejected_atomically() {
        for job_variant in [false, true] {
            for mismatched_identity in [false, true] {
                let (_dir, path) = variant_fixture(job_variant, false);
                let conn = Connection::open(&path).unwrap();
                if mismatched_identity {
                    let other = if job_variant {
                        "Passkey metadata and atomic addition results"
                    } else {
                        "Job input retirement independent of immutable receipts"
                    };
                    conn.execute(
                        "UPDATE __yoi_schema_migrations SET name=?1 WHERE version=103",
                        [other],
                    )
                    .unwrap();
                } else {
                    let trigger = if job_variant {
                        "backend_job_identity_immutable"
                    } else {
                        "passkey_metadata_insert"
                    };
                    conn.execute_batch(&format!("DROP TRIGGER {trigger}"))
                        .unwrap();
                }
                let schema = schema_rows(&conn);
                let history = workspace_schema_migration_history(&conn).unwrap();
                let rows = feature_rows(&conn, job_variant);
                assert!(SqliteWorkspaceStore::open_for_test(&path).is_err());
                assert_eq!(schema_rows(&conn), schema);
                assert_eq!(workspace_schema_migration_history(&conn).unwrap(), history);
                assert_eq!(feature_rows(&conn, job_variant), rows);
            }
        }
    }
}
