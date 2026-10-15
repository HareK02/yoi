// Component tests use production store methods and real isolated SQLite files/connections.
// WebAuthn verification is outside this suite: credentials enter at the verified-store boundary.
// Included in store::tests to reuse genuine retained migration fixtures. Run with `activation_`.

fn activation_fixture() -> (SqliteWorkspaceStore, server_api::RequestActor) {
    let store = SqliteWorkspaceStore::in_memory_for_test().unwrap();
    let admin = store.setup_hosting_admin("admin", true).unwrap();
    let actor = hosting_actor(&admin.user);
    (store, actor)
}

#[test]
fn activation_options_resume_without_consuming_and_completion_is_one_time() {
    let (store, admin) = activation_fixture();
    assert!(!store.hosting_self_registration_enabled().unwrap());
    let issue = store
        .create_provisional_user(&admin, "  New-Person  ")
        .unwrap();
    assert_eq!(issue.user.handle, "new-person");
    assert_eq!(issue.user.display_name, "new-person");
    assert!(
        store
            .is_user_pending_activation(&issue.user.user_id)
            .unwrap()
    );
    assert_eq!(
        store.get_user(&issue.user.user_id).unwrap(),
        Some(issue.user.clone())
    );
    let target = store
        .verify_activation_code("NEW-PERSON", &issue.code)
        .unwrap();
    assert_eq!(
        store
            .verify_activation_code("new-person", &issue.code)
            .unwrap(),
        target
    );
    assert!(store.verify_activation_code("admin", &issue.code).is_err());
    assert!(
        store
            .verify_activation_code("other-person", &issue.code)
            .is_err()
    );
    assert!(
        store
            .list_passkey_credentials_for_user(&issue.user.user_id)
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .is_user_pending_activation(&issue.user.user_id)
            .unwrap()
    );
    store
        .register_verified_passkey(&target, &hosting_credential(&issue.user, "first-key"))
        .unwrap();
    assert!(
        !store
            .is_user_pending_activation(&issue.user.user_id)
            .unwrap()
    );
    assert!(
        store
            .verify_activation_code("new-person", &issue.code)
            .is_err()
    );
    assert!(store.reissue_activation_code(&admin, "new-person").is_err());
    assert!(
        store
            .register_verified_passkey(&target, &hosting_credential(&issue.user, "second-key"))
            .is_err()
    );
    assert_eq!(
        store
            .list_passkey_credentials_for_user(&issue.user.user_id)
            .unwrap()
            .len(),
        1
    );
    assert!(!store.is_hosting_admin(&issue.user.user_id).unwrap());
}

#[test]
fn activation_invalid_attempts_commit_lockout_across_reopen_and_reissue_resets_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.db");
    let store = SqliteWorkspaceStore::open_for_test(&path).unwrap();
    let admin = hosting_actor(&store.setup_hosting_admin("admin", true).unwrap().user);
    let issue = store.create_provisional_user(&admin, "person").unwrap();
    let target = store.verify_activation_code("person", &issue.code).unwrap();
    for expected in 1..=5 {
        assert!(store.verify_activation_code("person", "wrong").is_err());
        store
            .with_conn(|conn| {
                let attempts: i64 = conn.query_row(
                    "SELECT failed_attempts FROM user_activations WHERE user_id=?1",
                    [&issue.user.user_id],
                    |r| r.get(0),
                )?;
                assert_eq!(attempts, expected);
                Ok(())
            })
            .unwrap();
    }
    drop(store);
    let store = SqliteWorkspaceStore::open_for_test(&path).unwrap();
    assert!(store.verify_activation_code("person", &issue.code).is_err());
    assert!(
        store
            .register_verified_passkey(&target, &hosting_credential(&issue.user, "locked-key"))
            .is_err()
    );
    let next = store.reissue_activation_code(&admin, "PERSON").unwrap();
    assert!(issue.code != next.code, "reissue must replace the secret");
    assert!(store.verify_activation_code("person", &issue.code).is_err());
    assert!(
        store
            .register_verified_passkey(&target, &hosting_credential(&issue.user, "old-key"))
            .is_err()
    );
    let target = store.verify_activation_code("person", &next.code).unwrap();
    store
        .register_verified_passkey(&target, &hosting_credential(&next.user, "fresh-key"))
        .unwrap();
}

#[test]
fn activation_expiry_and_reissued_ceremonies_are_rechecked_at_completion() {
    let (store, admin) = activation_fixture();
    let issue = store.create_provisional_user(&admin, "person").unwrap();
    let old = store.verify_activation_code("person", &issue.code).unwrap();
    let next = store.reissue_activation_code(&admin, "person").unwrap();
    assert!(
        store
            .register_verified_passkey(&old, &hosting_credential(&issue.user, "old-key"))
            .is_err()
    );
    let target = store.verify_activation_code("person", &next.code).unwrap();
    store
        .with_conn(|conn| {
            conn.execute(
                "UPDATE user_activations SET expires_at='2000-01-01T00:00:00Z'",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    assert!(store.verify_activation_code("person", &next.code).is_err());
    assert!(
        store
            .register_verified_passkey(&target, &hosting_credential(&issue.user, "expired-key"))
            .is_err()
    );
    assert!(
        store
            .is_user_pending_activation(&issue.user.user_id)
            .unwrap()
    );
    assert!(
        store
            .list_passkey_credentials_for_user(&issue.user.user_id)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn activation_credential_failure_rolls_back_pending_removal_and_allows_retry() {
    let (store, admin) = activation_fixture();
    let issue = store.create_provisional_user(&admin, "person").unwrap();
    let target = store.verify_activation_code("person", &issue.code).unwrap();
    store.with_conn(|conn| { conn.execute_batch("CREATE TRIGGER reject_activation_credential BEFORE INSERT ON passkey_credentials BEGIN SELECT RAISE(ABORT,'injected credential failure'); END;")?; Ok(()) }).unwrap();
    assert!(
        store
            .register_verified_passkey(&target, &hosting_credential(&issue.user, "key"))
            .is_err()
    );
    assert!(
        store
            .is_user_pending_activation(&issue.user.user_id)
            .unwrap()
    );
    assert!(
        store
            .list_passkey_credentials_for_user(&issue.user.user_id)
            .unwrap()
            .is_empty()
    );
    store
        .with_conn(|conn| {
            conn.execute_batch("DROP TRIGGER reject_activation_credential")?;
            Ok(())
        })
        .unwrap();
    let target = store.verify_activation_code("person", &issue.code).unwrap();
    store
        .register_verified_passkey(&target, &hosting_credential(&issue.user, "key"))
        .unwrap();
    assert!(
        !store
            .is_user_pending_activation(&issue.user.user_id)
            .unwrap()
    );
}

#[test]
fn activation_completion_checks_account_issuance_user_and_absence_of_credentials() {
    let (store, admin) = activation_fixture();
    let issue = store.create_provisional_user(&admin, "person").unwrap();
    let original = store.verify_activation_code("person", &issue.code).unwrap();
    let PasskeyRegistrationTarget::Activation { user, issuance_id } = &original else {
        panic!("wrong purpose")
    };
    let mut wrong_user = user.clone();
    wrong_user.account_id = admin.account_id;
    let wrong = PasskeyRegistrationTarget::Activation {
        user: wrong_user,
        issuance_id: issuance_id.clone(),
    };
    assert!(
        store
            .register_verified_passkey(&wrong, &hosting_credential(user, "wrong-account"))
            .is_err()
    );
    let mut credential = hosting_credential(user, "wrong-user");
    credential.user_id = admin.user_id;
    assert!(
        store
            .register_verified_passkey(&original, &credential)
            .is_err()
    );
    // Existing credentials, even if injected outside public admission, may not become recovery.
    store.with_conn(|conn| { conn.execute("INSERT INTO passkey_credentials(credential_id,user_id,public_key_cose,sign_count,created_at) VALUES('existing',?1,'key',0,'1')", [&user.user_id])?; Ok(()) }).unwrap();
    assert!(
        store
            .register_verified_passkey(&original, &hosting_credential(user, "second"))
            .is_err()
    );
    assert!(
        store
            .reissue_activation_code(
                &hosting_actor(&store.get_user_by_handle("admin").unwrap().unwrap()),
                "person"
            )
            .is_err()
    );
    assert!(store.is_user_pending_activation(&user.user_id).unwrap());
}

#[test]
fn activation_admin_authority_is_live_and_duplicate_handles_preserve_accounts() {
    let (store, admin) = activation_fixture();
    let existing = store.setup_hosting_admin("existing", true).unwrap().user;
    let normal = hosting_actor(&existing);
    store
        .with_conn(|conn| {
            conn.execute(
                "DELETE FROM hosting_administrators WHERE user_id=?1",
                [&existing.user_id],
            )?;
            Ok(())
        })
        .unwrap();
    let issue = store.create_provisional_user(&admin, "person").unwrap();
    assert!(store.create_provisional_user(&normal, "forbidden").is_err());
    assert!(store.reissue_activation_code(&normal, "person").is_err());
    let mut fake = admin.clone();
    fake.account_id = existing.account_id.clone();
    assert!(store.create_provisional_user(&fake, "fake").is_err());
    assert!(store.reissue_activation_code(&fake, "person").is_err());
    for handle in ["existing", "EXISTING", "person"] {
        assert!(store.create_provisional_user(&admin, handle).is_err());
    }
    assert_eq!(store.get_user(&existing.user_id).unwrap(), Some(existing));
    assert!(store.reissue_activation_code(&admin, "existing").is_err());
    store
        .with_conn(|conn| {
            conn.execute(
                "DELETE FROM hosting_administrators WHERE user_id=?1",
                [&admin.user_id],
            )?;
            Ok(())
        })
        .unwrap();
    assert!(store.create_provisional_user(&admin, "revoked").is_err());
    assert!(store.reissue_activation_code(&admin, "person").is_err());
    assert_eq!(
        store.verify_activation_code("person", &issue.code).unwrap(),
        store.verify_activation_code("person", &issue.code).unwrap()
    );
}

#[test]
fn activation_pending_cannot_mint_or_resolve_normal_auth_or_gain_hosting_setup() {
    let (store, admin) = activation_fixture();
    let issue = store.create_provisional_user(&admin, "person").unwrap();
    let session = BrowserSessionRecord {
        session_id: "session".into(),
        token_hash: "session-hash".into(),
        user_id: issue.user.user_id.clone(),
        created_at: "1".into(),
        expires_at: crate::auth::rfc3339_after(chrono::Duration::hours(1)),
        revoked_at: None,
    };
    let token = ApiTokenRecord {
        token_id: "token".into(),
        token_hash: "token-hash".into(),
        user_id: issue.user.user_id.clone(),
        label: "test".into(),
        created_at: "1".into(),
        expires_at: None,
        revoked_at: None,
        last_used_at: None,
    };
    assert!(store.create_browser_session(&session).is_err());
    assert!(store.create_api_token(&token).is_err());
    assert!(
        store
            .upsert_passkey_credential(&hosting_credential(&issue.user, "bypass"))
            .is_err()
    );
    assert!(store.setup_hosting_admin("person", false).is_err());
    // Corrupt/legacy credential rows cannot bypass resolution or authorize Addition.
    store.with_conn(|conn| {
        conn.execute("INSERT INTO browser_sessions(session_id,token_hash,user_id,created_at,expires_at) VALUES(?1,?2,?3,?4,?5)", params![session.session_id,session.token_hash,session.user_id,session.created_at,session.expires_at])?;
        conn.execute("INSERT INTO api_tokens(token_id,token_hash,user_id,label,created_at) VALUES(?1,?2,?3,?4,?5)", params![token.token_id,token.token_hash,token.user_id,token.label,token.created_at])?;
        Ok(())
    }).unwrap();
    assert!(
        store
            .resolve_browser_session(&session.token_hash)
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .resolve_api_token(&token.token_hash)
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .register_verified_passkey(
                &PasskeyRegistrationTarget::Addition {
                    user: issue.user.clone(),
                    authentication: PasskeyAuthenticationBinding::ApiToken(
                        token.token_hash.clone()
                    )
                },
                &hosting_credential(&issue.user, "addition")
            )
            .is_err()
    );
    assert!(
        store
            .approve_device_login_flow(
                "unknown",
                &issue.user.user_id,
                &token.token_id,
                "secret",
                "1"
            )
            .is_err()
    );
    store
        .with_conn(|conn| {
            assert!(activation::require_active_account(conn, &issue.user.account_id).is_err());
            Ok(())
        })
        .unwrap();
    let target = store.verify_activation_code("person", &issue.code).unwrap();
    store
        .register_verified_passkey(&target, &hosting_credential(&issue.user, "key"))
        .unwrap();
    store
        .create_browser_session(&BrowserSessionRecord {
            session_id: "active".into(),
            token_hash: "active-hash".into(),
            ..session
        })
        .unwrap();
    store
        .create_api_token(&ApiTokenRecord {
            token_id: "active".into(),
            token_hash: "active-hash".into(),
            ..token
        })
        .unwrap();
}

#[test]
fn activation_plaintext_never_persists_and_pending_survives_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.db");
    let store = SqliteWorkspaceStore::open_for_test(&path).unwrap();
    let admin = hosting_actor(&store.setup_hosting_admin("admin", true).unwrap().user);
    let issue = store.create_provisional_user(&admin, "person").unwrap();
    assert!(!format!("{issue:?}").contains(&issue.code));
    let expiry = chrono::DateTime::parse_from_rfc3339(&issue.expires_at).unwrap();
    let seconds = (expiry.with_timezone(&chrono::Utc) - chrono::Utc::now()).num_seconds();
    assert!((890..=900).contains(&seconds));
    store
        .with_conn(|conn| {
            let hash: String = conn.query_row(
                "SELECT code_hash FROM user_activations WHERE user_id=?1",
                [&issue.user.user_id],
                |r| r.get(0),
            )?;
            assert_eq!(hash, crate::auth::token_hash(&issue.code));
            conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")?;
            Ok(())
        })
        .unwrap();
    drop(store);
    for entry in std::fs::read_dir(dir.path()).unwrap() {
        let bytes = std::fs::read(entry.unwrap().path()).unwrap();
        assert!(
            !bytes
                .windows(issue.code.len())
                .any(|window| window == issue.code.as_bytes())
        );
    }
    let reopened = SqliteWorkspaceStore::open_for_test(&path).unwrap();
    assert!(
        reopened
            .is_user_pending_activation(&issue.user.user_id)
            .unwrap()
    );
    assert!(
        reopened
            .verify_activation_code("person", &issue.code)
            .is_ok()
    );
}

#[test]
fn activation_parallel_options_complete_once_and_parallel_duplicate_create_has_one_winner() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.db");
    let first = SqliteWorkspaceStore::open_for_test(&path).unwrap();
    let second = SqliteWorkspaceStore::open_for_test(&path).unwrap();
    let admin = hosting_actor(&first.setup_hosting_admin("admin", true).unwrap().user);
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let jobs = [first.clone(), second.clone()]
        .into_iter()
        .map(|store| {
            let barrier = barrier.clone();
            let admin = admin.clone();
            std::thread::spawn(move || {
                barrier.wait();
                store.create_provisional_user(&admin, "person")
            })
        })
        .collect::<Vec<_>>();
    let results = jobs
        .into_iter()
        .map(|job| job.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    let issue = results.into_iter().find_map(|result| result.ok()).unwrap();
    let targets = [
        first.verify_activation_code("person", &issue.code).unwrap(),
        second
            .verify_activation_code("person", &issue.code)
            .unwrap(),
    ];
    let jobs = [first.clone(), second]
        .into_iter()
        .zip(targets)
        .enumerate()
        .map(|(index, (store, target))| {
            let barrier = barrier.clone();
            let credential = hosting_credential(&issue.user, &format!("key-{index}"));
            std::thread::spawn(move || {
                barrier.wait();
                store.register_verified_passkey(&target, &credential)
            })
        })
        .collect::<Vec<_>>();
    let results = jobs
        .into_iter()
        .map(|job| job.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert!(
        !first
            .is_user_pending_activation(&issue.user.user_id)
            .unwrap()
    );
    assert_eq!(
        first
            .list_passkey_credentials_for_user(&issue.user.user_id)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn activation_oldest_and_previous_schema_migrate_with_same_constraints_and_preserve_users() {
    fn schema(conn: &Connection) -> String {
        conn.query_row(
            "SELECT sql FROM sqlite_schema WHERE name='user_activations'",
            [],
            |r| r.get(0),
        )
        .unwrap()
    }
    let fresh = SqliteWorkspaceStore::in_memory_for_test().unwrap();
    let expected = fresh.with_conn(|conn| Ok(schema(conn))).unwrap();
    for baseline in [50, 99] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("server.db");
        if baseline == 50 {
            prepare_schema_v50(&path, None);
        } else {
            prepare_retained_schema(&path, baseline);
        }
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch("INSERT OR IGNORE INTO accounts VALUES('old-account','user','old','Old','1','1'); INSERT INTO users VALUES('old-user','old-account','old','Old','1','1');").unwrap();
        drop(conn);
        let store = SqliteWorkspaceStore::open_for_test(&path).unwrap();
        assert_eq!(
            store.get_user("old-user").unwrap().unwrap().display_name,
            "Old"
        );
        assert!(!store.is_user_pending_activation("old-user").unwrap());
        store.with_conn(|conn| {
            assert_eq!(schema(conn), expected);
            assert_eq!(current_schema_version(conn)?, 100);
            assert!(conn.execute("INSERT INTO user_activations VALUES('missing','issue','hash','future',0)", []).is_err());
            for attempts in [-1, 6] { assert!(conn.execute("INSERT INTO user_activations VALUES('old-user','issue','hash','future',?1)", [attempts]).is_err()); }
            Ok(())
        }).unwrap();
    }
}

#[test]
fn activation_migration_failure_rolls_back_table_and_history() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.db");
    prepare_retained_schema(&path, 99);
    let conn = Connection::open(&path).unwrap();
    configure_sqlite(&conn).unwrap();
    conn.execute_batch("CREATE TRIGGER reject_activation_migration BEFORE INSERT ON __yoi_schema_migrations WHEN NEW.version=100 BEGIN SELECT RAISE(ABORT,'injected migration failure'); END;").unwrap();
    assert!(activation::migrate(&conn).is_err());
    assert_eq!(current_schema_version(&conn).unwrap(), 99);
    assert!(!table_exists(&conn, "user_activations").unwrap());
    conn.execute_batch("DROP TRIGGER reject_activation_migration")
        .unwrap();
    activation::migrate(&conn).unwrap();
    assert!(table_exists(&conn, "user_activations").unwrap());
    assert_eq!(current_schema_version(&conn).unwrap(), 100);
}

#[tokio::test]
async fn activation_pending_workspace_owner_is_rejected_by_both_store_creation_paths() {
    let (store, admin) = activation_fixture();
    let issue = store.create_provisional_user(&admin, "person").unwrap();
    let workspace = WorkspaceRecord {
        workspace_id: "pending-workspace".into(),
        owner_account_id: issue.user.account_id.clone(),
        display_name: "Pending".into(),
        state: "active".into(),
        created_at: "1".into(),
        updated_at: "1".into(),
    };
    assert!(
        store
            .upsert_workspace(&workspace)
            .await
            .unwrap_err()
            .to_string()
            .contains("activation_required")
    );
    let signing = WorkspaceSigningIdentityActivation {
        workspace_id: workspace.workspace_id.clone(),
        key_id: "key".into(),
        public_key: "public".into(),
        public_key_fingerprint: "sha256:key".into(),
        private_material_ref: "private-ref".into(),
        provisioned_at: "1".into(),
    };
    let record = WorkspaceBootstrapRecord {
        operation_key: "operation".into(),
        request_fingerprint: "sha256:request".into(),
        workspace: workspace.clone(),
        repository: None,
    };
    assert!(
        store
            .create_workspace_bootstrap(&record, &signing, "provision")
            .unwrap_err()
            .to_string()
            .contains("activation_required")
    );
    assert!(
        store
            .get_workspace(&workspace.workspace_id)
            .await
            .unwrap()
            .is_none()
    );
    let target = store.verify_activation_code("person", &issue.code).unwrap();
    store
        .register_verified_passkey(&target, &hosting_credential(&issue.user, "key"))
        .unwrap();
    store.upsert_workspace(&workspace).await.unwrap();
    assert_eq!(
        store.get_workspace(&workspace.workspace_id).await.unwrap(),
        Some(workspace)
    );
}

#[test]
fn activation_pending_removal_failure_rolls_back_inserted_credential() {
    let (store, admin) = activation_fixture();
    let issue = store.create_provisional_user(&admin, "person").unwrap();
    let target = store.verify_activation_code("person", &issue.code).unwrap();
    store.with_conn(|conn| { conn.execute_batch("CREATE TRIGGER reject_pending_removal BEFORE DELETE ON user_activations BEGIN SELECT RAISE(ABORT,'injected activation failure'); END;")?; Ok(()) }).unwrap();
    assert!(
        store
            .register_verified_passkey(&target, &hosting_credential(&issue.user, "key"))
            .is_err()
    );
    assert!(
        store
            .is_user_pending_activation(&issue.user.user_id)
            .unwrap()
    );
    assert!(store.get_passkey_credential("key").unwrap().is_none());
    store
        .with_conn(|conn| {
            conn.execute_batch("DROP TRIGGER reject_pending_removal")?;
            Ok(())
        })
        .unwrap();
    store
        .register_verified_passkey(&target, &hosting_credential(&issue.user, "key"))
        .unwrap();
    assert!(
        !store
            .is_user_pending_activation(&issue.user.user_id)
            .unwrap()
    );
}

#[test]
fn activation_pending_device_token_is_not_delivered_even_with_injected_approval() {
    let (store, admin) = activation_fixture();
    let issue = store.create_provisional_user(&admin, "person").unwrap();
    let flow = DeviceLoginFlowRecord {
        device_code: "device".into(),
        user_code: "CODE".into(),
        verification_uri: "/device".into(),
        client_name: None,
        user_id: Some(issue.user.user_id.clone()),
        api_token_id: None,
        issued_access_token: Some("not-deliverable".into()),
        created_at: "1".into(),
        expires_at: crate::auth::rfc3339_after(chrono::Duration::minutes(5)),
        approved_at: Some("1".into()),
        consumed_at: None,
    };
    assert!(store.try_create_device_login_flow(&flow).is_err());
    store.with_conn(|conn| { conn.execute("INSERT INTO device_login_flows(device_code,user_code,verification_uri,user_id,issued_access_token,created_at,expires_at,approved_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)", params![flow.device_code,flow.user_code,flow.verification_uri,flow.user_id,flow.issued_access_token,flow.created_at,flow.expires_at,flow.approved_at])?; Ok(()) }).unwrap();
    assert!(
        store
            .consume_device_login_token("device", "2")
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .get_device_login_flow_by_device_code("device")
            .unwrap()
            .unwrap()
            .consumed_at
            .is_none()
    );
    assert!(!store.is_hosting_admin(&issue.user.user_id).unwrap());
}
