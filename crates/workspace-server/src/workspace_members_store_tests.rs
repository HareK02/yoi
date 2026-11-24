// Real SQLite authority, receipt, restart and migration tests. No service or live DB.
mod membership_store {
    use super::*;
    use server_api::{
        WorkspaceMemberAddRequest as Add, WorkspaceMemberListQuery as List,
        WorkspaceMemberRemoveRequest as Remove,
    };

    fn seed(store: &SqliteWorkspaceStore) {
        store.with_conn(|conn| {
            for id in ["owner", "member", "outsider"] {
                conn.execute("INSERT INTO accounts VALUES(?1,'user',?1,?1,'1','1')",[id])?;
                conn.execute("INSERT INTO users VALUES(?1,?1,?1,?1,'1','1')",[id])?;
            }
            conn.execute_batch("INSERT INTO workspaces(workspace_id,owner_account_id,display_name,state,created_at,updated_at) VALUES('w','owner','Workspace','active','1','1'); INSERT INTO workspaces(workspace_id,owner_account_id,display_name,state,created_at,updated_at) VALUES('other','outsider','Other','active','1','1');")?;
            Ok(())
        }).unwrap();
    }
    fn add(key: &str, observed_change_key: Option<&str>) -> Add {
        Add {
            operation_key: key.into(),
            account_handle: "member".into(),
            expected_membership_change_key: observed_change_key.map(str::to_owned),
        }
    }
    fn remove(key: &str, member: &str) -> Remove {
        Remove {
            operation_key: key.into(),
            expected_membership_key: member.into(),
        }
    }

    #[test]
    fn membership_replays_survive_restart_without_restoring_removed_or_replaced_grants() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("members.sqlite");
        let store = SqliteWorkspaceStore::open_for_test(&path).unwrap();
        seed(&store);
        assert!(store.workspace_account_has_access("w", "owner").unwrap());
        assert!(!store.workspace_account_has_access("w", "member").unwrap());
        let saved = store
            .workspace_members_add("w", "owner", &add("add", None))
            .unwrap();
        assert!(store.workspace_account_has_access("w", "member").unwrap());
        assert!(
            !store
                .workspace_account_has_access("other", "member")
                .unwrap()
        );
        assert!(
            store
                .workspace_members_add("w", "owner", &add("duplicate", Some("add")))
                .is_err()
        );
        store
            .workspace_members_remove("w", "owner", "member", &remove("remove", "add"))
            .unwrap();
        drop(store);
        let store = SqliteWorkspaceStore::open_for_test(&path).unwrap();
        let replay = store
            .workspace_members_add("w", "owner", &add("add", None))
            .unwrap();
        assert!(replay.replayed);
        assert_eq!(replay.membership_key, saved.membership_key);
        assert!(!store.workspace_account_has_access("w", "member").unwrap());
        assert!(
            store
                .workspace_members_add("w", "owner", &add("stale-absence", None))
                .is_err()
        );
        let mut changed = add("add", None);
        changed.account_handle = "outsider".into();
        assert!(store.workspace_members_add("w", "owner", &changed).is_err());
        store
            .workspace_members_add("w", "owner", &add("readd", Some("remove")))
            .unwrap();
        assert!(
            store
                .workspace_members_remove("w", "owner", "member", &remove("stale-remove", "add"))
                .is_err()
        );
        assert!(
            store
                .workspace_members_remove("w", "owner", "member", &remove("remove", "add"))
                .unwrap()
                .replayed
        );
        assert!(store.workspace_account_has_access("w", "member").unwrap());
        assert!(
            store
                .workspace_members_remove("w", "owner", "owner", &remove("owner-remove", "readd"))
                .is_err()
        );
        assert!(store.workspace_account_has_access("w", "owner").unwrap());
    }

    #[test]
    fn membership_requires_active_existing_user_and_owner_and_pages_consistently() {
        let store = SqliteWorkspaceStore::in_memory_for_test().unwrap();
        seed(&store);
        assert!(
            store
                .workspace_members_add("w", "outsider", &add("denied", None))
                .is_err()
        );
        assert!(
            store
                .workspace_members_list("w", "member", &List::default())
                .is_err()
        );
        let mut missing = add("missing", None);
        missing.account_handle = "unknown".into();
        assert!(matches!(
            store.workspace_members_add("w", "owner", &missing),
            Err(Error::WorkspaceMemberAccountNotFound)
        ));
        store
            .with_conn(|c| {
                c.execute_batch(
                    "INSERT INTO user_activations VALUES('member','activation','hash','expiry',0);",
                )?;
                Ok(())
            })
            .unwrap();
        assert!(matches!(
            store.workspace_members_add("w", "owner", &add("pending", None)),
            Err(Error::WorkspaceMemberAccountNotFound)
        ));
        store
            .with_conn(|c| {
                c.execute_batch("DELETE FROM user_activations;")?;
                Ok(())
            })
            .unwrap();
        store
            .workspace_members_add("w", "owner", &add("add", None))
            .unwrap();
        let first = store
            .workspace_members_list(
                "w",
                "owner",
                &List {
                    limit: Some(1),
                    after_account_id: None,
                },
            )
            .unwrap();
        assert_eq!(first.members.len(), 1);
        assert_eq!(first.members[0].account_id, "member");
        let second = store
            .workspace_members_list(
                "w",
                "owner",
                &List {
                    limit: Some(1),
                    after_account_id: first.next_after_account_id,
                },
            )
            .unwrap();
        assert_eq!(second.members[0].account_id, "owner");
        assert!(second.members[0].is_owner);
        assert_eq!(second.members[0].membership_key, None);
        assert!(second.next_after_account_id.is_none());
        assert_eq!(first.membership_change_key, second.membership_change_key);
        assert!(
            store
                .workspace_members_list(
                    "w",
                    "owner",
                    &List {
                        limit: Some(101),
                        after_account_id: None
                    }
                )
                .is_err()
        );
    }

    #[test]
    fn membership_parallel_adds_have_one_winner_and_failed_receipt_rolls_back_grant() {
        let store = Arc::new(SqliteWorkspaceStore::in_memory_for_test().unwrap());
        seed(&store);
        store.with_conn(|c| { c.execute_batch("CREATE TRIGGER fail_membership BEFORE INSERT ON workspace_membership_operations BEGIN SELECT RAISE(ABORT,'injected'); END;")?; Ok(()) }).unwrap();
        assert!(
            store
                .workspace_members_add("w", "owner", &add("add", None))
                .is_err()
        );
        assert!(!store.workspace_account_has_access("w", "member").unwrap());
        assert_eq!(
            store
                .workspace_members_list("w", "owner", &List::default())
                .unwrap()
                .membership_change_key,
            None
        );
        store
            .with_conn(|c| {
                c.execute_batch("DROP TRIGGER fail_membership;")?;
                Ok(())
            })
            .unwrap();
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let handles: Vec<_> = (0..2)
            .map(|i| {
                let store = store.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    store.workspace_members_add("w", "owner", &add(&format!("add-{i}"), None))
                })
            })
            .collect();
        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    }

    #[test]
    fn membership_migration_preserves_owners_without_granting_existing_users_and_matches_fresh() {
        let fresh = SqliteWorkspaceStore::in_memory_for_test().unwrap();
        for version in [50, 142] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("migration.sqlite");
            if version == 50 {
                prepare_schema_v50(&path, None);
            } else {
                prepare_retained_schema(&path, version);
            }
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch("INSERT INTO accounts VALUES('member-owner','user','member-owner','Owner','1','1'); INSERT INTO accounts VALUES('member-stranger','user','member-stranger','Stranger','1','1'); INSERT INTO workspaces(workspace_id,owner_account_id,display_name,state,created_at,updated_at) VALUES('membership-workspace','member-owner','Preserved','active','1','1');").unwrap();
            drop(conn);
            let plan = SqliteWorkspaceStore::migration_plan(&path).unwrap();
            assert_eq!(plan.target_schema_version, 143);
            let unchanged = Connection::open(&path).unwrap();
            assert_eq!(current_schema_version(&unchanged).unwrap(), version);
            assert!(!table_exists(&unchanged, "workspace_members").unwrap());
            drop(unchanged);
            let migrated = SqliteWorkspaceStore::open_for_test(&path).unwrap();
            assert!(
                migrated
                    .workspace_account_has_access("membership-workspace", "member-owner")
                    .unwrap()
            );
            assert!(
                !migrated
                    .workspace_account_has_access("membership-workspace", "member-stranger")
                    .unwrap()
            );
            for table in [
                "workspace_members",
                "workspace_membership_changes",
                "workspace_membership_operations",
            ] {
                let expected = fresh
                    .with_conn(|c| Ok(state_contract_schema_signature(c, table)))
                    .unwrap();
                migrated
                    .with_conn(|c| {
                        assert_eq!(state_contract_schema_signature(c, table), expected);
                        Ok(())
                    })
                    .unwrap();
            }
            migrated.with_conn(|c|{assert!(c.execute("INSERT INTO workspace_members VALUES('missing','member-owner','key')",[]).is_err());assert!(c.execute("INSERT INTO workspace_members VALUES('membership-workspace','missing','key')",[]).is_err());Ok(())}).unwrap();
        }
    }

    #[test]
    fn membership_migration_rolls_back_and_rejects_unreceipted_or_changed_schema() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("migration.sqlite");
        prepare_retained_schema(&path, 142);
        let c = Connection::open(&path).unwrap();
        configure_sqlite(&c).unwrap();
        c.execute_batch("CREATE TRIGGER fail_membership_migration BEFORE INSERT ON __yoi_schema_migrations WHEN NEW.version=143 BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
        assert!(workspace_members::migrate(&c).is_err());
        assert_eq!(current_schema_version(&c).unwrap(), 142);
        assert!(!table_exists(&c, "workspace_members").unwrap());
        c.execute_batch("DROP TRIGGER fail_membership_migration;")
            .unwrap();
        c.execute_batch(include_str!("workspace_members_schema.sql"))
            .unwrap();
        assert!(verify_schema_history(&c, 142).is_err());
        c.execute_batch("DROP TABLE workspace_members; DROP TABLE workspace_membership_changes; DROP TABLE workspace_membership_operations;").unwrap();
        workspace_members::migrate(&c).unwrap();
        verify_schema_history(&c, 143).unwrap();
        c.execute_batch("ALTER TABLE workspace_members ADD COLUMN unexpected TEXT;")
            .unwrap();
        assert!(verify_schema_history(&c, 143).is_err());
    }
}
