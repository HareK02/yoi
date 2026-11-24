// Included below the reservation fixture in store::tests::reservation_initialization.
// Real SQLite reservation/binding/migration transactions and file reopen. No
// Runtime, public authorization or Feature-owner behavior is replaced/claimed.
mod public_feature_operation {
    use super::*;

    impl Fixture {
        fn feature_reserve(
            &self,
            operation: &str,
            caller: &str,
            intent: &str,
            singleton: Option<&str>,
            bundle: &ConfigBundle,
            data: &FeatureInitializations,
        ) -> Result<WorkerCreateReservation> {
            self.store.reserve_feature_worker_launch(
                "workspace-a",
                "runtime-a",
                operation,
                caller,
                intent,
                singleton,
                &self.memory,
                "builtin:coder",
                bundle,
                data,
            )
        }
    }

    fn frozen_bundle() -> ConfigBundle {
        // Reuse only the configuration fixture; never bind or issue a core grant.
        launch("not-issued").1
    }

    fn saved(
        store: &SqliteWorkspaceStore,
        operation: &str,
        caller: &str,
    ) -> Result<Option<SavedFeatureWorkerLaunch>> {
        store.saved_feature_worker_launch("workspace-a", operation, caller)
    }

    #[tokio::test]
    async fn feature_operation_reopen_preserves_frozen_profile_bundle_and_initialization_without_core_grant()
     {
        let fixture = Fixture::new().await;
        let bundle = frozen_bundle();
        let data = initialization(4, "frozen-connection");
        assert_eq!(
            saved(&fixture.store, "human-worker:owner:create", "full-input").unwrap(),
            None
        );
        let first = fixture
            .feature_reserve(
                "human-worker:owner:create",
                "full-input",
                "semantic",
                None,
                &bundle,
                &data,
            )
            .unwrap();
        assert_eq!(
            fixture
                .feature_reserve(
                    "human-worker:owner:create",
                    "full-input",
                    "semantic",
                    None,
                    &bundle,
                    &data
                )
                .unwrap(),
            first
        );
        for wrong in ["other-account-hash", "other-subject-input-hash"] {
            assert!(matches!(
                saved(&fixture.store, "human-worker:owner:create", wrong),
                Err(Error::RepositoryConflict(_))
            ));
            assert!(matches!(
                fixture.feature_reserve(
                    "human-worker:owner:create",
                    wrong,
                    "semantic",
                    None,
                    &bundle,
                    &data
                ),
                Err(Error::RepositoryConflict(_))
            ));
        }
        // Even an internally inconsistent request retaining the full caller hash
        // cannot silently change its actual reservation or frozen configuration.
        assert!(matches!(
            fixture.feature_reserve(
                "human-worker:owner:create",
                "full-input",
                "other-semantic",
                None,
                &bundle,
                &data
            ),
            Err(Error::RepositoryConflict(_))
        ));
        assert!(matches!(
            fixture.feature_reserve(
                "human-worker:owner:create",
                "full-input",
                "semantic",
                Some("subjektiv:other"),
                &bundle,
                &data
            ),
            Err(Error::RepositoryConflict(_))
        ));
        let mut changed = bundle.clone();
        changed.metadata.id = "new-default-profile".into();
        changed.metadata.digest = changed.computed_digest();
        assert!(matches!(
            fixture.feature_reserve(
                "human-worker:owner:create",
                "full-input",
                "semantic",
                None,
                &changed,
                &data
            ),
            Err(Error::RepositoryConflict(_))
        ));
        assert!(matches!(
            fixture.store.reserve_feature_worker_launch(
                "workspace-a",
                "runtime-a",
                "human-worker:owner:create",
                "full-input",
                "semantic",
                None,
                &fixture.memory,
                "builtin:reviewer",
                &bundle,
                &data,
            ),
            Err(Error::RepositoryConflict(_))
        ));
        let Fixture {
            store,
            _dir,
            path,
            memory: _,
        } = fixture;
        drop(store);
        let store = SqliteWorkspaceStore::open_for_test(&path).unwrap();
        store
            .update_workspace_memory_settings("workspace-a", "English", "Japanese")
            .unwrap();
        let launch = saved(&store, "human-worker:owner:create", "full-input")
            .unwrap()
            .unwrap();
        assert_eq!(
            launch,
            SavedFeatureWorkerLaunch {
                profile: "builtin:coder".into(),
                bundle: bundle.clone(),
                initialization: data.clone()
            }
        );
        let replay = store
            .reserve_feature_worker_launch(
                "workspace-a",
                "runtime-a",
                "human-worker:owner:create",
                "full-input",
                "semantic",
                None,
                &store.get_workspace_memory_settings("workspace-a").unwrap(),
                &launch.profile,
                &launch.bundle,
                &launch.initialization,
            )
            .unwrap();
        assert_eq!(replay, first);
        assert_eq!(replay.memory_settings.language, "English");
        assert_eq!(
            store
                .saved_feature_worker_launch(
                    "other-workspace",
                    "human-worker:owner:create",
                    "full-input"
                )
                .unwrap(),
            None
        );
        store.with_conn(|conn| {
            for table in ["core_worker_execution_grants", "core_worker_launch_bindings"] {
                assert_eq!(conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get::<_, i64>(0))?, 0);
            }
            let row: (String, String, String) = conn.query_row(
                "SELECT operation_id,allocation_key,caller_fingerprint FROM worker_create_operation_bindings", [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )?;
            assert_eq!(row, ("human-worker:owner:create".into(), "manual:human-worker:owner:create".into(), "full-input".into()));
            Ok(())
        }).unwrap();
    }

    #[tokio::test]
    async fn feature_singleton_other_operation_saves_alias_to_original_allocation_and_full_input() {
        let fixture = Fixture::new().await;
        let bundle = frozen_bundle();
        let data = initialization(2, "subject-a");
        let first = fixture
            .feature_reserve(
                "human-worker:first",
                "caller-first",
                "semantic",
                Some("subjektiv:a"),
                &bundle,
                &data,
            )
            .unwrap();
        let second = fixture
            .feature_reserve(
                "human-worker:second",
                "caller-second",
                "semantic",
                Some("subjektiv:a"),
                &bundle,
                &data,
            )
            .unwrap();
        assert_eq!(second, first);
        assert_eq!(
            fixture
                .feature_reserve(
                    "human-worker:second",
                    "caller-second",
                    "semantic",
                    Some("subjektiv:a"),
                    &bundle,
                    &data
                )
                .unwrap(),
            first
        );
        let reopened = SqliteWorkspaceStore::open_for_test(&fixture.path).unwrap();
        for identity in ["human-worker:first", "human-worker:second", "manual:human-worker:first"] {
            assert_eq!(reopened.reserved_worker_create_runtime("workspace-a", identity).unwrap(), Some("runtime-a".into()));
            assert!(!reopened.worker_create_allocation_removed("workspace-a", identity).unwrap());
        }
        assert_eq!(reopened.reserved_worker_create_runtime("workspace-a", "missing").unwrap(), None);
        assert_eq!(
            saved(&reopened, "human-worker:second", "caller-second")
                .unwrap()
                .unwrap()
                .initialization,
            data
        );
        assert!(matches!(
            saved(&reopened, "human-worker:second", "caller-first"),
            Err(Error::RepositoryConflict(_))
        ));
        // A different Subject/full request cannot retarget this alias, even with
        // identical spawn semantics or after observing a different current owner.
        assert!(matches!(
            fixture.feature_reserve(
                "human-worker:second",
                "other-subject",
                "semantic",
                Some("subjektiv:b"),
                &bundle,
                &data
            ),
            Err(Error::RepositoryConflict(_))
        ));
        assert!(matches!(
            fixture.feature_reserve(
                "human-worker:second",
                "caller-second",
                "semantic",
                Some("subjektiv:b"),
                &bundle,
                &data
            ),
            Err(Error::RepositoryConflict(_))
        ));
        reopened.with_conn(|conn| {
            assert_eq!(conn.query_row("SELECT COUNT(*) FROM worker_create_reservations", [], |r| r.get::<_, i64>(0))?, 1);
            assert_eq!(conn.query_row("SELECT COUNT(*) FROM worker_create_reservations WHERE allocation_key='manual:human-worker:second'", [], |r| r.get::<_, i64>(0))?, 0);
            assert_eq!(conn.query_row("SELECT COUNT(*) FROM worker_create_operation_bindings WHERE allocation_key='manual:human-worker:first'", [], |r| r.get::<_, i64>(0))?, 2);
            assert_eq!(conn.query_row("SELECT generation FROM worker_singleton_owners WHERE singleton_key='subjektiv:a'", [], |r| r.get::<_, i64>(0))?, 1);
            assert_eq!(conn.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |r| r.get::<_, i64>(0))?, 0);
            Ok(())
        }).unwrap();
    }

    #[tokio::test]
    async fn removed_feature_operation_alias_never_moves_to_replacement_allocation() {
        let fixture = Fixture::new().await;
        let bundle = frozen_bundle();
        let data = initialization(1, "subject");
        let first = fixture
            .feature_reserve(
                "first",
                "caller-first",
                "semantic",
                Some("subjektiv:a"),
                &bundle,
                &data,
            )
            .unwrap();
        fixture
            .feature_reserve(
                "alias",
                "caller-alias",
                "semantic",
                Some("subjektiv:a"),
                &bundle,
                &data,
            )
            .unwrap();
        // Fixture the owner's completed removal/release. This test only claims
        // reservation identity retention, not the public release authorization.
        fixture.store.with_conn(|conn| {
            conn.execute("UPDATE worker_create_reservations SET state='removed' WHERE allocation_key='manual:first'", [])?;
            conn.execute("DELETE FROM worker_singleton_owners WHERE singleton_key='subjektiv:a'", [])?;
            Ok(())
        }).unwrap();
        let next = fixture
            .feature_reserve(
                "replacement",
                "caller-replacement",
                "semantic",
                Some("subjektiv:a"),
                &bundle,
                &data,
            )
            .unwrap();
        assert_ne!(next.worker_id, first.worker_id);
        for identity in ["first", "alias", "manual:first"] {
            assert!(fixture.store.worker_create_allocation_removed("workspace-a", identity).unwrap());
        }
        assert!(!fixture.store.worker_create_allocation_removed("workspace-a", "replacement").unwrap());
        assert!(matches!(
            saved(&fixture.store, "alias", "caller-alias"),
            Err(Error::RepositoryConflict(_))
        ));
        assert!(matches!(
            fixture.feature_reserve(
                "alias",
                "caller-alias",
                "semantic",
                Some("subjektiv:a"),
                &bundle,
                &data
            ),
            Err(Error::RepositoryConflict(_))
        ));
        fixture.store.with_conn(|conn| {
            assert_eq!(conn.query_row("SELECT allocation_key FROM worker_create_operation_bindings WHERE operation_id='alias'", [], |r| r.get::<_, String>(0))?, "manual:first");
            assert_eq!(conn.query_row("SELECT COUNT(*) FROM worker_create_reservations", [], |r| r.get::<_, i64>(0))?, 2);
            Ok(())
        }).unwrap();
    }

    #[tokio::test]
    async fn feature_operation_binding_failure_rolls_back_new_allocation_and_singleton_alias_reuse()
    {
        let fixture = Fixture::new().await;
        let bundle = frozen_bundle();
        let data = initialization(1, "resolved");
        fixture.store.with_conn(|conn| {
            conn.execute_batch("CREATE TEMP TRIGGER reject_public_binding BEFORE INSERT ON worker_create_operation_bindings WHEN NEW.operation_id LIKE 'refused-%' BEGIN SELECT RAISE(ABORT,'injected binding failure'); END;")?;
            Ok(())
        }).unwrap();
        assert!(
            fixture
                .feature_reserve(
                    "refused-new",
                    "caller-new",
                    "semantic",
                    Some("subjektiv:a"),
                    &bundle,
                    &data
                )
                .is_err()
        );
        fixture
            .store
            .with_conn(|conn| {
                for table in [
                    "worker_create_reservations",
                    "worker_singleton_owners",
                    "worker_create_operation_bindings",
                ] {
                    assert_eq!(
                        conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r
                            .get::<_, i64>(0))?,
                        0,
                        "{table}"
                    );
                }
                Ok(())
            })
            .unwrap();
        let first = fixture
            .feature_reserve(
                "accepted",
                "caller-first",
                "semantic",
                Some("subjektiv:a"),
                &bundle,
                &data,
            )
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
        let before = fixture
            .store
            .with_conn(|conn| Ok(stored_fixture_rows(conn, "worker_create_reservations")))
            .unwrap();
        assert!(
            fixture
                .feature_reserve(
                    "refused-reuse",
                    "caller-reuse",
                    "semantic",
                    Some("subjektiv:a"),
                    &bundle,
                    &data
                )
                .is_err()
        );
        assert_eq!(
            saved(&fixture.store, "refused-reuse", "caller-reuse").unwrap(),
            None
        );
        fixture
            .store
            .with_conn(|conn| {
                assert_eq!(
                    stored_fixture_rows(conn, "worker_create_reservations"),
                    before
                );
                assert_eq!(
                    conn.query_row(
                        "SELECT COUNT(*) FROM worker_create_operation_bindings",
                        [],
                        |r| r.get::<_, i64>(0)
                    )?,
                    1
                );
                assert_eq!(
                    conn.query_row("SELECT generation FROM worker_singleton_owners", [], |r| {
                        r.get::<_, i64>(0)
                    })?,
                    1
                );
                conn.execute_batch("DROP TRIGGER reject_public_binding")?;
                Ok(())
            })
            .unwrap();
        assert_eq!(
            fixture
                .feature_reserve(
                    "refused-reuse",
                    "caller-reuse",
                    "semantic",
                    Some("subjektiv:a"),
                    &bundle,
                    &data
                )
                .unwrap(),
            first
        );
        assert!(
            saved(&fixture.store, "refused-reuse", "caller-reuse")
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn binding_callback_commits_and_rolls_back_on_all_reservation_success_paths() {
        let fixture = Fixture::new().await;
        let data = initialization(1, "resolved");
        let reserve = |key: &str, fail: bool| {
            fixture.store.reserve_worker_create_with_binding(
                "workspace-a", "runtime-a", key, "semantic", Some("subjektiv:a"), &fixture.memory, &data,
                |_| Ok(None), |_, _| Ok(()),
                |tx, _| {
                    tx.execute("UPDATE workspaces SET display_name=display_name || '+' WHERE workspace_id='workspace-a'", [])?;
                    if fail { return Err(Error::RepositoryConflict("injected callback failure".into())); }
                    Ok(())
                },
            )
        };
        // New allocation, same-key replay and manual singleton reuse must each
        // commit callback writes; a callback failure must roll them back.
        for key in ["manual:first", "manual:first", "manual:alias"] {
            assert!(reserve(key, true).is_err());
            reserve(key, false).unwrap();
        }
        fixture
            .store
            .with_conn(|conn| {
                assert_eq!(
                    conn.query_row(
                        "SELECT display_name FROM workspaces WHERE workspace_id='workspace-a'",
                        [],
                        |r| r.get::<_, String>(0)
                    )?,
                    "Workspace+++"
                );
                assert_eq!(
                    conn.query_row("SELECT COUNT(*) FROM worker_create_reservations", [], |r| r
                        .get::<_, i64>(0))?,
                    1
                );
                Ok(())
            })
            .unwrap();
    }

    #[tokio::test]
    async fn stale_feature_operation_read_is_rechecked_inside_competing_connection_reservation() {
        let fixture = Fixture::new().await;
        let competing = SqliteWorkspaceStore::open_for_test(&fixture.path).unwrap();
        let bundle = frozen_bundle();
        let data = initialization(1, "resolved");
        // Deterministic competing-Server interleaving: both read absence, one
        // commits, the stale reader must not allocate from its earlier result.
        assert_eq!(saved(&fixture.store, "operation", "caller").unwrap(), None);
        assert_eq!(saved(&competing, "operation", "caller").unwrap(), None);
        let first = fixture
            .feature_reserve("operation", "caller", "semantic", None, &bundle, &data)
            .unwrap();
        let reserve = |caller: &str| {
            competing.reserve_feature_worker_launch(
                "workspace-a",
                "runtime-a",
                "operation",
                caller,
                "semantic",
                None,
                &fixture.memory,
                "builtin:coder",
                &bundle,
                &data,
            )
        };
        assert!(matches!(
            reserve("changed-full-input"),
            Err(Error::RepositoryConflict(_))
        ));
        assert_eq!(reserve("caller").unwrap(), first);
        competing
            .with_conn(|conn| {
                assert_eq!(
                    conn.query_row("SELECT COUNT(*) FROM worker_create_reservations", [], |r| r
                        .get::<_, i64>(0))?,
                    1
                );
                assert_eq!(
                    conn.query_row(
                        "SELECT COUNT(*) FROM worker_create_operation_bindings",
                        [],
                        |r| r.get::<_, i64>(0)
                    )?,
                    1
                );
                Ok(())
            })
            .unwrap();
    }

    #[tokio::test]
    async fn feature_operation_read_rejects_tampered_or_missing_initialization_and_invalid_bundle()
    {
        let fixture = Fixture::new().await;
        let bundle = frozen_bundle();
        let data = initialization(1, "original");
        fixture
            .feature_reserve("operation", "caller", "semantic", None, &bundle, &data)
            .unwrap();
        for tampered in [
            None,
            Some(serde_json::to_string(&initialization(2, "changed")).unwrap()),
        ] {
            fixture
                .store
                .with_conn(|conn| {
                    conn.execute(
                        "UPDATE worker_create_reservations SET feature_initialization_json=?1",
                        params![tampered],
                    )?;
                    Ok(())
                })
                .unwrap();
            assert!(saved(&fixture.store, "operation", "caller").is_err());
            assert!(
                fixture
                    .feature_reserve("operation", "caller", "semantic", None, &bundle, &data)
                    .is_err()
            );
        }
        fixture
            .store
            .with_conn(|conn| {
                conn.execute(
                    "UPDATE worker_create_reservations SET feature_initialization_json=?1",
                    [serde_json::to_string(&data).unwrap()],
                )?;
                Ok(())
            })
            .unwrap();
        let create_fingerprint = fixture
            .store
            .with_conn(|conn| {
                let original: String = conn.query_row(
                    "SELECT create_fingerprint FROM worker_create_reservations",
                    [],
                    |row| row.get(0),
                )?;
                conn.execute(
                    "UPDATE worker_create_reservations SET create_fingerprint='tampered'",
                    [],
                )?;
                Ok(original)
            })
            .unwrap();
        assert!(saved(&fixture.store, "operation", "caller").is_err());
        assert!(
            fixture
                .feature_reserve("operation", "caller", "semantic", None, &bundle, &data)
                .is_err()
        );
        fixture
            .store
            .with_conn(|conn| {
                conn.execute(
                    "UPDATE worker_create_reservations SET create_fingerprint=?1",
                    [&create_fingerprint],
                )?;
                Ok(())
            })
            .unwrap();
        // Even a self-consistent digest cannot move a saved bundle to a
        // different Workspace. A wrong declared digest is rejected separately.
        let mut other_workspace = bundle.clone();
        other_workspace.metadata.workspace_id = "other-workspace".into();
        other_workspace.metadata.digest = other_workspace.computed_digest();
        let mut wrong_digest = bundle.clone();
        wrong_digest.metadata.digest = "tampered-digest".into();
        for changed in [other_workspace, wrong_digest] {
            fixture
                .store
                .with_conn(|conn| {
                    conn.execute(
                        "UPDATE worker_create_operation_bindings SET config_bundle_json=?1",
                        [serde_json::to_string(&changed).unwrap()],
                    )?;
                    Ok(())
                })
                .unwrap();
            assert!(saved(&fixture.store, "operation", "caller").is_err());
            assert!(
                fixture
                    .feature_reserve("operation", "caller", "semantic", None, &changed, &data)
                    .is_err()
            );
        }
    }

    #[test]
    fn operation_binding_migration_preserves_legacy_rows_and_preflight_does_not_infer_aliases() {
        let (_dir, path) = legacy_fixture();
        let conn = Connection::open(&path).unwrap();
        configure_sqlite(&conn).unwrap();
        feature_initialization::migrate(&conn).unwrap();
        let rows = stored_fixture_rows(&conn, "worker_create_reservations");
        let plan = SqliteWorkspaceStore::migration_plan(&path).unwrap();
        assert_eq!(plan.current_schema_version, 142);
        assert_eq!(
            plan.migrations,
            vec![WorkspaceSchemaMigrationStep {
                version: 143,
                name: feature_worker_launch::MIGRATION_NAME.into()
            }]
        );
        assert_eq!(current_schema_version(&conn).unwrap(), 142);
        assert!(!table_exists(&conn, "worker_create_operation_bindings").unwrap());
        assert_eq!(
            stored_fixture_rows(&conn, "worker_create_reservations"),
            rows
        );
        drop(conn);
        let store = SqliteWorkspaceStore::open_for_test(&path).unwrap();
        assert_eq!(
            saved(&store, "legacy-subject", "new-full-input").unwrap(),
            None
        );
        store.with_conn(|conn| {
            assert_eq!(stored_fixture_rows(conn, "worker_create_reservations"), rows);
            for table in ["worker_create_operation_bindings", "core_worker_execution_grants"] {
                assert_eq!(conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get::<_, i64>(0))?, 0);
            }
            assert_eq!(conn.query_row("SELECT COUNT(*) FROM worker_create_reservations WHERE feature_initialization_json IS NULL", [], |r| r.get::<_, i64>(0))?, 2);
            Ok(())
        }).unwrap();
    }

    #[test]
    fn operation_binding_migration_receipt_failure_rolls_back_table_index_and_legacy_rows() {
        let (_dir, path) = legacy_fixture();
        let conn = Connection::open(&path).unwrap();
        configure_sqlite(&conn).unwrap();
        feature_initialization::migrate(&conn).unwrap();
        let rows = stored_fixture_rows(&conn, "worker_create_reservations");
        conn.execute_batch("CREATE TEMP TRIGGER reject_operation_migration BEFORE INSERT ON __yoi_schema_migrations WHEN NEW.version=143 BEGIN SELECT RAISE(ABORT,'injected receipt failure'); END;").unwrap();
        assert!(feature_worker_launch::migrate(&conn).is_err());
        assert_eq!(current_schema_version(&conn).unwrap(), 142);
        assert!(!table_exists(&conn, "worker_create_operation_bindings").unwrap());
        assert_eq!(conn.query_row("SELECT COUNT(*) FROM sqlite_schema WHERE name='worker_create_operation_allocation'", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
        assert_eq!(
            stored_fixture_rows(&conn, "worker_create_reservations"),
            rows
        );
        conn.execute_batch("DROP TRIGGER reject_operation_migration")
            .unwrap();
        feature_worker_launch::migrate(&conn).unwrap();
        assert_eq!(current_schema_version(&conn).unwrap(), 143);
        assert_eq!(
            stored_fixture_rows(&conn, "worker_create_reservations"),
            rows
        );
        assert!(
            feature_worker_launch::migrate(&conn).is_err(),
            "migration has an exact predecessor fence"
        );
    }

    #[tokio::test]
    async fn oldest_upgrade_and_fresh_operation_binding_schema_match_and_enforce_reservation_foreign_key()
     {
        let fresh = Fixture::new().await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("oldest.db");
        prepare_schema_v50(&path, Some("workspace-a"));
        let upgraded = SqliteWorkspaceStore::open_for_test(&path).unwrap();
        let workspace = upgraded
            .get_workspace("workspace-a")
            .await
            .unwrap()
            .unwrap();
        upgraded.upsert_workspace(&workspace).await.unwrap();
        let signature = |store: &SqliteWorkspaceStore| {
            store
                .with_conn(|conn| {
                    Ok(state_contract_schema_signature(
                        conn,
                        "worker_create_operation_bindings",
                    ))
                })
                .unwrap()
        };
        assert_eq!(signature(&fresh.store), signature(&upgraded));
        let bundle = frozen_bundle();
        let bundle_json = serde_json::to_string(&bundle).unwrap();
        for store in [&fresh.store, &upgraded] {
            let memory = store.get_workspace_memory_settings("workspace-a").unwrap();
            store
                .reserve_feature_worker_launch(
                    "workspace-a",
                    "runtime-a",
                    "operation",
                    "caller",
                    "semantic",
                    None,
                    &memory,
                    "builtin:coder",
                    &bundle,
                    &initialization(1, "resolved"),
                )
                .unwrap();
            store.with_conn(|conn| {
                assert!(conn.execute("INSERT INTO worker_create_operation_bindings(workspace_id,operation_id,caller_fingerprint,allocation_key,profile,config_bundle_json) VALUES('workspace-a','missing-reservation','caller','no-such-reservation','builtin:coder',?1)", [&bundle_json]).is_err());
                assert!(conn.execute("INSERT INTO worker_create_operation_bindings(workspace_id,operation_id,caller_fingerprint,allocation_key,profile,config_bundle_json) VALUES('other-workspace','cross-workspace','caller','manual:operation','builtin:coder',?1)", [&bundle_json]).is_err());
                for invalid in ["not-json", "[]", "null"] {
                    assert!(conn.execute("UPDATE worker_create_operation_bindings SET config_bundle_json=?1", [invalid]).is_err());
                }
                assert!(conn.execute("INSERT INTO worker_create_operation_bindings SELECT * FROM worker_create_operation_bindings", []).is_err());
                assert!(conn.execute("DELETE FROM worker_create_reservations WHERE allocation_key='manual:operation'", []).is_err(), "retained alias cannot dangle");
                assert_eq!(conn.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |r| r.get::<_, i64>(0))?, 0);
                Ok(())
            }).unwrap();
        }
    }
}
