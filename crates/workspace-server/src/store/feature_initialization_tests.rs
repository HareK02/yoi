// Included in store::tests: real reservation, grant and migration transactions on
// isolated SQLite files. No Runtime dispatch or Feature-owner authorization claim.
mod reservation_initialization {
    use super::*;
    use protocol::feature_initialization::FeatureInitialization;
    use server_api::authorization::{
        CoreWorkerExecutionGrant, CoreWorkerGrantSource, PermissionReferenceEncoding,
    };
    use worker_runtime::config_bundle::{
        ConfigBundle, ConfigBundleMetadata, ConfigBundleProvenance,
    };

    struct Fixture {
        _dir: tempfile::TempDir,
        path: std::path::PathBuf,
        store: SqliteWorkspaceStore,
        memory: WorkspaceMemorySettingsRecord,
    }

    impl Fixture {
        async fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("workspace.db");
            let store = SqliteWorkspaceStore::open_api_fixture(&path).unwrap();
            store
                .upsert_workspace(&WorkspaceRecord {
                    workspace_id: "workspace-a".into(),
                    owner_account_id: "owner-account".into(),
                    display_name: "Workspace".into(),
                    state: "active".into(),
                    created_at: "1".into(),
                    updated_at: "1".into(),
                })
                .await
                .unwrap();
            let memory = store.get_workspace_memory_settings("workspace-a").unwrap();
            Self {
                _dir: dir,
                path,
                store,
                memory,
            }
        }

        fn reserve(
            &self,
            key: &str,
            input: &str,
            singleton: Option<&str>,
            data: &FeatureInitializations,
        ) -> Result<WorkerCreateReservation> {
            self.store.reserve_worker_create_with_initialization(
                "workspace-a",
                "runtime-a",
                key,
                input,
                singleton,
                &self.memory,
                data,
            )
        }
    }

    fn initialization(version: u32, connection: &str) -> FeatureInitializations {
        FeatureInitializations::new(vec![
            FeatureInitialization::encode(
                "test:connection",
                version,
                &serde_json::json!({"connection": connection}),
            )
            .unwrap(),
        ])
        .unwrap()
    }

    fn launch(operation: &str) -> (CoreWorkerExecutionGrant, ConfigBundle) {
        let mut bundle = ConfigBundle {
            metadata: ConfigBundleMetadata {
                id: "frozen-profile".into(),
                digest: String::new(),
                workspace_id: "workspace-a".into(),
                created_at: "1".into(),
                provenance: ConfigBundleProvenance {
                    source: "test".into(),
                    detail: None,
                },
            },
            profiles: vec![],
            declarations: vec![],
            prompt_catalog: None,
            profile_source_archive: None,
            profile_source_archive_handle: None,
        };
        bundle.metadata.digest = bundle.computed_digest();
        let grant = CoreWorkerExecutionGrant {
            grant_id: format!("grant-{operation}"),
            workspace_id: "workspace-a".into(),
            creation_operation_id: operation.into(),
            runtime_id: "runtime-a".into(),
            worker_id: "draft".into(),
            profile: "builtin:coder".into(),
            profile_digest: bundle.computed_digest(),
            assignment: Default::default(),
            reference_encoding: PermissionReferenceEncoding::FeatureSetsV2,
            issuance_ceiling: None,
            source: CoreWorkerGrantSource::Account {
                account_id: "owner-account".into(),
            },
        };
        (grant, bundle)
    }

    #[tokio::test]
    async fn reservation_reopen_reuses_saved_initialization_and_rejects_changed_intent_or_resolution()
     {
        let mut fixture = Fixture::new().await;
        let data = initialization(7, "stable-ref");
        let first = fixture.reserve("create", "intent", None, &data).unwrap();
        fixture.store.with_conn(|conn| {
            let saved: (String, String) = conn.query_row(
                "SELECT request_fingerprint,feature_initialization_json FROM worker_create_reservations WHERE allocation_key='create'", [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            assert_eq!(saved.0, "intent", "resolved binding must not rewrite caller intent");
            assert_eq!(serde_json::from_str::<FeatureInitializations>(&saved.1).unwrap(), data);
            Ok(())
        }).unwrap();
        let Fixture {
            store,
            _dir,
            path,
            memory,
        } = fixture;
        drop(store);
        fixture = Fixture {
            store: SqliteWorkspaceStore::open_for_test(&path).unwrap(),
            _dir,
            path,
            memory,
        };
        fixture.memory = fixture
            .store
            .update_workspace_memory_settings("workspace-a", "English", "Japanese")
            .unwrap();
        let replay = fixture.reserve("create", "intent", None, &data).unwrap();
        assert_eq!(replay, first);
        assert_eq!(
            fixture
                .store
                .saved_worker_feature_initialization("workspace-a", "create")
                .unwrap(),
            Some(data.clone())
        );
        assert_eq!(
            replay
                .feature_initialization
                .get("test:connection")
                .unwrap()
                .version(),
            7
        );
        assert_eq!(replay.memory_settings.language, "English");
        assert!(matches!(
            fixture.reserve("create", "changed-intent", None, &data),
            Err(Error::InvalidInput(_))
        ));
        for changed in [
            initialization(8, "stable-ref"),
            initialization(7, "other-ref"),
            FeatureInitializations::default(),
        ] {
            let error = fixture
                .reserve("create", "intent", None, &changed)
                .unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("different resolved Feature initialization"),
                "{error}"
            );
        }
        let other = fixture
            .reserve(
                "other-create",
                "intent",
                None,
                &initialization(8, "stable-ref"),
            )
            .unwrap();
        let same_memory = fixture
            .reserve("same-memory", "intent", None, &data)
            .unwrap();
        assert_ne!(same_memory.create_fingerprint, other.create_fingerprint);
        assert_eq!(
            fixture.reserve("create", "intent", None, &data).unwrap(),
            first
        );
    }

    #[tokio::test]
    async fn manual_singleton_reuse_compares_and_returns_original_initialization() {
        let fixture = Fixture::new().await;
        let data = initialization(2, "subject-connection");
        let first = fixture
            .reserve("manual:first", "intent", Some("subjektiv:test"), &data)
            .unwrap();
        assert_eq!(
            fixture
                .reserve("manual:retry", "intent", Some("subjektiv:test"), &data)
                .unwrap(),
            first
        );
        let error = fixture
            .reserve(
                "manual:drift",
                "intent",
                Some("subjektiv:test"),
                &initialization(3, "subject-connection"),
            )
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("different resolved Feature initialization"),
            "{error}"
        );
        fixture.store.with_conn(|conn| {
            assert_eq!(conn.query_row("SELECT COUNT(*) FROM worker_create_reservations", [], |row| row.get::<_,i64>(0))?, 1);
            assert_eq!(conn.query_row("SELECT generation FROM worker_singleton_owners WHERE singleton_key='subjektiv:test'", [], |row| row.get::<_,i64>(0))?, 1);
            Ok(())
        }).unwrap();
    }

    #[tokio::test]
    async fn core_reservation_replay_preserves_grant_bundle_and_initialization_binding() {
        let fixture = Fixture::new().await;
        let (grant, bundle) = launch("core-create");
        let data = initialization(9, "resolved");
        let reserve = |data: &FeatureInitializations| {
            fixture
                .store
                .reserve_worker_create_with_core_initialization(
                    "workspace-a",
                    "runtime-a",
                    "core-create",
                    "intent",
                    "caller-input",
                    None,
                    &fixture.memory,
                    &grant,
                    &bundle,
                    data,
                )
        };
        let first = reserve(&data).unwrap();
        assert_eq!(reserve(&data).unwrap(), first);
        let reopened = SqliteWorkspaceStore::open_for_test(&fixture.path).unwrap();
        assert_eq!(
            reopened
                .reserve_worker_create_with_core_initialization(
                    "workspace-a",
                    "runtime-a",
                    "core-create",
                    "intent",
                    "caller-input",
                    None,
                    &fixture.memory,
                    &grant,
                    &bundle,
                    &data,
                )
                .unwrap(),
            first
        );
        let error = reserve(&initialization(10, "resolved")).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("different resolved Feature initialization"),
            "{error}"
        );
        let (saved_grant, saved_bundle) = fixture
            .store
            .saved_core_worker_launch("workspace-a", "core-create", "caller-input")
            .unwrap()
            .unwrap();
        assert_eq!(saved_grant.worker_id, first.worker_id.to_string());
        assert_eq!(saved_bundle, bundle);
        fixture
            .store
            .with_conn(|conn| {
                assert_eq!(
                    conn.query_row(
                        "SELECT COUNT(*) FROM core_worker_execution_grants",
                        [],
                        |row| row.get::<_, i64>(0)
                    )?,
                    1
                );
                assert_eq!(
                    conn.query_row(
                        "SELECT caller_fingerprint FROM core_worker_launch_bindings",
                        [],
                        |row| row.get::<_, String>(0)
                    )?,
                    "caller-input"
                );
                Ok(())
            })
            .unwrap();
        fixture.store.with_conn(|conn| {
            conn.execute("UPDATE worker_create_reservations SET feature_initialization_json=NULL WHERE allocation_key='core-create'", [])?;
            Ok(())
        }).unwrap();
        let error = fixture
            .store
            .saved_core_worker_launch("workspace-a", "core-create", "caller-input")
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("worker_create_legacy_initialization_missing"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn failed_core_grant_binding_rolls_back_initialization_memory_resource_key_and_singleton()
    {
        let fixture = Fixture::new().await;
        let (mut grant, bundle) = launch("refused-core");
        grant.source = CoreWorkerGrantSource::Account {
            account_id: "not-owner".into(),
        };
        assert!(
            fixture
                .store
                .reserve_worker_create_with_core_initialization(
                    "workspace-a",
                    "runtime-a",
                    "refused-core",
                    "intent",
                    "caller",
                    Some("subjektiv:test"),
                    &fixture.memory,
                    &grant,
                    &bundle,
                    &initialization(1, "ref"),
                )
                .is_err()
        );
        fixture
            .store
            .with_conn(|conn| {
                for table in [
                    "worker_create_reservations",
                    "worker_singleton_owners",
                    "core_worker_execution_grants",
                    "core_worker_launch_bindings",
                ] {
                    assert_eq!(
                        conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row
                            .get::<_, i64>(0))?,
                        0,
                        "{table}"
                    );
                }
                Ok(())
            })
            .unwrap();
        // A failed completion must not consume the visible Worker key either.
        let first = fixture
            .reserve("after-refusal", "intent", None, &initialization(1, "ref"))
            .unwrap();
        assert_eq!(
            fixture
                .store
                .worker_resource_key(
                    "workspace-a",
                    &RuntimeWorkerRef::new("runtime-a", first.worker_id.to_string())
                )
                .unwrap()
                .as_deref(),
            Some("W-1")
        );
    }

    fn legacy_fixture() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("legacy.db");
        prepare_retained_schema(&path, 141);
        let conn = Connection::open(&path).unwrap();
        configure_sqlite(&conn).unwrap();
        conn.execute_batch("INSERT INTO accounts(account_id,kind,handle,display_name,created_at,updated_at) VALUES('owner','user','owner','Owner','1','1');
            INSERT INTO workspaces(workspace_id,owner_account_id,display_name,state,created_at,updated_at) VALUES('workspace-a','owner','Workspace','active','1','1');
            INSERT INTO workspace_memory_settings(workspace_id,language,created_at,updated_at) VALUES('workspace-a','English','1','1');
            INSERT INTO worker_create_reservations(workspace_id,allocation_key,worker_id,runtime_id,request_fingerprint,create_fingerprint,memory_language,state,created_at,updated_at,singleton_key,singleton_generation)
                VALUES('workspace-a','legacy-subject','00000000-0000-7000-8000-000000000001','runtime-a','old-intent','old-runtime-hash','English','reserved','1','1','subjektiv:legacy',1),
                      ('workspace-a','legacy-ordinary','00000000-0000-7000-8000-000000000002','runtime-a','old-intent','old-runtime-hash','English','reserved','1','1',NULL,NULL);").unwrap();
        (dir, path)
    }

    #[test]
    fn migration_preserves_legacy_hashes_and_reports_missing_resolution_without_issuing_authority()
    {
        let (_dir, path) = legacy_fixture();
        let conn = Connection::open(&path).unwrap();
        let rows = fixture_query_rows(
            &conn,
            "SELECT allocation_key,request_fingerprint,create_fingerprint FROM worker_create_reservations ORDER BY allocation_key",
        );
        let plan = SqliteWorkspaceStore::migration_plan(&path).unwrap();
        assert_eq!(plan.migrations.len(), 1);
        assert!(
            !column_exists(
                &conn,
                "worker_create_reservations",
                "feature_initialization_json"
            )
            .unwrap()
        );
        drop(conn);
        let store = SqliteWorkspaceStore::open_for_test(&path).unwrap();
        let memory = store.get_workspace_memory_settings("workspace-a").unwrap();
        for (key, singleton) in [
            ("legacy-subject", Some("subjektiv:legacy")),
            ("legacy-ordinary", None),
        ] {
            let error = store
                .reserve_worker_create_with_initialization(
                    "workspace-a",
                    "runtime-a",
                    key,
                    "old-intent",
                    singleton,
                    &memory,
                    &FeatureInitializations::default(),
                )
                .unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("worker_create_legacy_initialization_missing"),
                "{error}"
            );
        }
        store.with_conn(|conn| {
            assert_eq!(fixture_query_rows(conn, "SELECT allocation_key,request_fingerprint,create_fingerprint FROM worker_create_reservations ORDER BY allocation_key"), rows);
            assert_eq!(conn.query_row("SELECT COUNT(*) FROM worker_create_reservations WHERE feature_initialization_json IS NULL", [], |row| row.get::<_,i64>(0))?, 2);
            assert_eq!(conn.query_row("SELECT COUNT(*) FROM core_worker_execution_grants", [], |row| row.get::<_,i64>(0))?, 0);
            Ok(())
        }).unwrap();
    }

    #[test]
    fn initialization_migration_receipt_failure_rolls_back_column_and_preserves_legacy_rows() {
        let (_dir, path) = legacy_fixture();
        let conn = Connection::open(&path).unwrap();
        configure_sqlite(&conn).unwrap();
        let rows = stored_fixture_rows(&conn, "worker_create_reservations");
        conn.execute_batch("CREATE TEMP TRIGGER reject_feature_migration BEFORE INSERT ON __yoi_schema_migrations WHEN NEW.version=142 BEGIN SELECT RAISE(ABORT,'injected receipt failure'); END;").unwrap();
        assert!(feature_initialization::migrate(&conn).is_err());
        assert_eq!(current_schema_version(&conn).unwrap(), 141);
        assert!(
            !column_exists(
                &conn,
                "worker_create_reservations",
                "feature_initialization_json"
            )
            .unwrap()
        );
        assert_eq!(
            stored_fixture_rows(&conn, "worker_create_reservations"),
            rows
        );
        conn.execute_batch("DROP TRIGGER reject_feature_migration")
            .unwrap();
        feature_initialization::migrate(&conn).unwrap();
        assert_eq!(
            current_schema_version(&conn).unwrap(),
            LATEST_SCHEMA_VERSION
        );
    }

    #[tokio::test]
    async fn fresh_and_oldest_supported_upgrade_enforce_initialization_column_constraints() {
        let fresh = Fixture::new().await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("oldest.db");
        prepare_schema_v50(&path, Some("workspace-a"));
        let upgraded = SqliteWorkspaceStore::open_for_test(&path).unwrap();
        let fresh_signature = fresh
            .store
            .with_conn(|conn| {
                Ok(state_contract_schema_signature(
                    conn,
                    "worker_create_reservations",
                ))
            })
            .unwrap();
        let upgraded_signature = upgraded
            .with_conn(|conn| {
                Ok(state_contract_schema_signature(
                    conn,
                    "worker_create_reservations",
                ))
            })
            .unwrap();
        assert_eq!(fresh_signature, upgraded_signature);
        // The historical schema fixture inserts only the Workspace row. Complete
        // its normal registration before exercising the create-reservation API.
        let workspace = upgraded
            .get_workspace("workspace-a")
            .await
            .unwrap()
            .unwrap();
        upgraded.upsert_workspace(&workspace).await.unwrap();
        for store in [&fresh.store, &upgraded] {
            let memory = store.get_workspace_memory_settings("workspace-a").unwrap();
            let data = initialization(1, "resolved");
            store
                .reserve_worker_create_with_initialization(
                    "workspace-a",
                    "runtime-a",
                    "probe",
                    "intent",
                    None,
                    &memory,
                    &data,
                )
                .unwrap();
            store.with_conn(|conn| {
                for invalid in ["not-json".to_string(), "{}".to_string(), format!("[{}]", " ".repeat(131072))] {
                    assert!(conn.execute("UPDATE worker_create_reservations SET feature_initialization_json=?1 WHERE allocation_key='probe'", [&invalid]).is_err());
                }
                let saved: String = conn.query_row("SELECT feature_initialization_json FROM worker_create_reservations WHERE allocation_key='probe'", [], |row| row.get(0))?;
                assert_eq!(serde_json::from_str::<FeatureInitializations>(&saved).unwrap(), data);
                Ok(())
            }).unwrap();
        }
    }

    #[tokio::test]
    async fn tampered_initialization_is_not_replayed_under_the_old_create_fingerprint() {
        let fixture = Fixture::new().await;
        let data = initialization(1, "original");
        fixture.reserve("create", "intent", None, &data).unwrap();
        let changed = initialization(1, "changed");
        fixture.store.with_conn(|conn| {
            conn.execute("UPDATE worker_create_reservations SET feature_initialization_json=?1 WHERE allocation_key='create'", [serde_json::to_string(&changed).unwrap()])?;
            Ok(())
        }).unwrap();
        let error = fixture
            .reserve("create", "intent", None, &changed)
            .unwrap_err();
        assert!(
            error.to_string().contains("fingerprint does not bind"),
            "{error}"
        );
    }
}
