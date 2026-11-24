// Real-store migration and transaction probes; no live DB or service is used.
mod addition_code_store {
    use super::*;

    #[test]
    fn addition_code_migration_preserves_oldest_identity_credentials_sessions_and_matches_fresh_schema()
     {
        let fresh = SqliteWorkspaceStore::in_memory_for_test().unwrap();
        for version in [50, 141] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("migration.sqlite");
            if version == 50 {
                prepare_schema_v50(&path, None);
            } else {
                prepare_retained_schema(&path, version);
            }
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch("INSERT OR IGNORE INTO accounts VALUES('addition-owner','user','addition-owner','Owner','1','1');
                INSERT INTO users VALUES('addition-user','addition-owner','addition-owner','Owner','1','1');
                INSERT INTO passkey_credentials(credential_id,user_id,public_key_cose,sign_count,created_at) VALUES('addition-key','addition-user','preserved',7,'1');
                INSERT INTO browser_sessions(session_id,token_hash,user_id,created_at,expires_at) VALUES('addition-session','addition-hash','addition-user','1','9999-01-01T00:00:00Z');").unwrap();
            // SQL fixture errors must not be mistaken for empty preservation evidence.
            assert_eq!(conn.query_row("SELECT public_key_cose FROM passkey_credentials WHERE credential_id='addition-key'",[],|r| r.get::<_,String>(0)).unwrap(),"preserved");
            drop(conn);
            let migrated = SqliteWorkspaceStore::open_for_test(&path).unwrap();
            let expected = fresh
                .with_conn(|conn| {
                    Ok(state_contract_schema_signature(
                        conn,
                        "passkey_addition_codes",
                    ))
                })
                .unwrap();
            migrated.with_conn(|conn| {
                assert_eq!(state_contract_schema_signature(conn,"passkey_addition_codes"),expected);
                assert_eq!(conn.query_row("SELECT sign_count FROM passkey_credentials WHERE credential_id='addition-key'",[],|r| r.get::<_,i64>(0))?,7);
                assert_eq!(conn.query_row("SELECT token_hash FROM browser_sessions WHERE session_id='addition-session'",[],|r| r.get::<_,String>(0))?,"addition-hash");
                assert_eq!(conn.query_row("SELECT account_id FROM users WHERE user_id='addition-user'",[],|r| r.get::<_,String>(0))?,"addition-owner");
                assert!(conn.execute("INSERT INTO passkey_addition_codes VALUES('missing','i','hash','https://a.test','a.test','expiry',0)",[]).is_err());
                conn.execute("INSERT INTO passkey_addition_codes VALUES('addition-user','i','hash','https://a.test','a.test','expiry',0)",[])?;
                assert!(conn.execute("UPDATE passkey_addition_codes SET failed_attempts=6",[]).is_err());
                assert!(conn.execute("INSERT INTO passkey_addition_codes VALUES('addition-user','j','hash','https://a.test','a.test','expiry',0)",[]).is_err());
                Ok(())
            }).unwrap();
        }
    }

    #[test]
    fn addition_code_migration_rolls_back_ddl_when_receipt_cannot_commit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("migration.sqlite");
        prepare_retained_schema(&path, 141);
        let conn = Connection::open(&path).unwrap();
        configure_sqlite(&conn).unwrap();
        conn.execute_batch("CREATE TRIGGER fail_addition_migration BEFORE INSERT ON __yoi_schema_migrations WHEN NEW.version=142 BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
        assert!(passkey_addition_code::migrate(&conn).is_err());
        assert_eq!(current_schema_version(&conn).unwrap(), 141);
        assert!(!table_exists(&conn, "passkey_addition_codes").unwrap());
        conn.execute_batch("DROP TRIGGER fail_addition_migration")
            .unwrap();
        passkey_addition_code::migrate(&conn).unwrap();
        assert!(table_exists(&conn, "passkey_addition_codes").unwrap());
    }

    #[test]
    fn addition_code_issuance_rechecks_session_and_stores_only_hash() {
        let store = SqliteWorkspaceStore::in_memory_for_test().unwrap();
        let now = crate::auth::now_rfc3339();
        store.with_conn(|conn| {
            conn.execute("INSERT INTO accounts VALUES('a','user','owner','Owner',?1,?1)",[&now])?;
            conn.execute("INSERT INTO users VALUES('u','a','owner','Owner',?1,?1)",[&now])?;
            conn.execute("INSERT INTO browser_sessions(session_id,token_hash,user_id,created_at,expires_at) VALUES('s','hash','u',?1,?2)",params![now,crate::auth::rfc3339_after(chrono::Duration::days(1))])?;
            Ok(())
        }).unwrap();
        let user = store.get_user("u").unwrap().unwrap();
        let issued = store
            .issue_addition_code(&user, "hash", "https://a.test", "a.test")
            .unwrap();
        let row = store
            .with_conn(|conn| {
                Ok(
                    conn.query_row("SELECT code_hash FROM passkey_addition_codes", [], |r| {
                        r.get::<_, String>(0)
                    })?,
                )
            })
            .unwrap();
        assert_eq!(row, crate::auth::token_hash(&issued.code));
        assert!(!format!("{issued:?}").contains(&issued.code));
        let expiry = chrono::DateTime::parse_from_rfc3339(&issued.expires_at).unwrap();
        assert!(
            expiry
                .signed_duration_since(chrono::Utc::now())
                .num_seconds()
                <= 900
        );
        assert!(
            expiry
                .signed_duration_since(chrono::Utc::now())
                .num_seconds()
                >= 895
        );
        store
            .with_conn(|conn| {
                conn.execute("UPDATE browser_sessions SET revoked_at=?1", [now])?;
                Ok(())
            })
            .unwrap();
        // Retain the previously read user/hash: the write must not trust those snapshots.
        assert!(
            store
                .issue_addition_code(&user, "hash", "https://a.test", "a.test")
                .is_err()
        );
        assert!(
            store
                .revoke_addition_code(&user, "hash", &issued.issuance_id)
                .is_err()
        );
        let current: String = store
            .with_conn(|conn| {
                Ok(
                    conn.query_row("SELECT issuance_id FROM passkey_addition_codes", [], |r| {
                        r.get(0)
                    })?,
                )
            })
            .unwrap();
        assert_eq!(current, issued.issuance_id);
    }
}
