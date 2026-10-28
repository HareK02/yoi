//! Real SQLite plus disk-component tests. Only authentication/HTTP and execution
//! are outside this suite; packages use the checked-in real Rust PDK Component.
use super::*;
use crate::store::{ControlPlaneStore, WorkspaceRecord};
use tempfile::{TempDir, tempdir};

struct Fixture {
    dir: TempDir,
    store: SqliteWorkspaceStore,
    bytes: Vec<u8>,
}
impl Fixture {
    async fn new() -> Self {
        let dir = tempdir().unwrap();
        let store = SqliteWorkspaceStore::open_for_test(dir.path().join("server.db")).unwrap();
        for workspace in ["a", "b"] {
            store
                .upsert_workspace(&WorkspaceRecord {
                    workspace_id: workspace.into(),
                    owner_account_id: "owner-account".into(),
                    display_name: workspace.into(),
                    state: "active".into(),
                    created_at: "2026-10-01T00:00:00Z".into(),
                    updated_at: "2026-10-01T00:00:00Z".into(),
                })
                .await
                .unwrap();
        }
        Self {
            dir,
            store,
            bytes: package("0.1.0", ""),
        }
    }
    fn root(&self) -> PathBuf {
        self.dir.path().join("artifacts")
    }
    fn upload(&self) -> PluginUploadResponse {
        self.store
            .plugin_upload(&self.root(), "a", "actor", &self.bytes)
            .unwrap()
    }
    fn install(&self, upload: &PluginUploadResponse, alias: &str) -> PluginOperationResponse {
        self.store
            .plugin_install(
                "a",
                "actor",
                &PluginInstallRequest {
                    upload_id: upload.upload_id.clone(),
                    alias: alias.into(),
                    operation_key: alias.into(),
                },
            )
            .unwrap()
    }
    fn expire(&self) {
        self.store
            .with_conn(|c| {
                c.execute("UPDATE plugin_uploads SET expires_at=0", [])?;
                Ok(())
            })
            .unwrap();
    }
    fn count(&self, table: &str) -> i64 {
        self.store
            .with_conn(|c| {
                Ok(c.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))?)
            })
            .unwrap()
    }
}
fn package(version: &str, extra: &str) -> Vec<u8> {
    let dir = tempdir().unwrap();
    let manifest =
        include_str!("../../../../resources/plugin/templates/rust-component-tool/plugin.toml")
            .replace("version = \"0.1.0\"", &format!("version = \"{version}\""));
    fs::write(dir.path().join("plugin.toml"), manifest).unwrap();
    fs::write(
        dir.path().join("plugin.component.wasm"),
        include_bytes!("../../../plugin-pdk/tests/fixtures/tool-v2.component.wasm"),
    )
    .unwrap();
    if !extra.is_empty() {
        fs::write(dir.path().join("extra.txt"), extra).unwrap();
    }
    let parsed = plugin::read_plugin_directory(
        dir.path(),
        PluginSourceKind::Explicit,
        &PluginPackageLimits::default(),
    )
    .unwrap();
    plugin::pack_plugin_package(&parsed, &PluginPackageLimits::default())
        .unwrap()
        .bytes
}
fn error<T: std::fmt::Debug>(result: Result<T>, expected: PluginCatalogError) {
    assert!(
        matches!(result,Err(Error::PluginCatalog(actual)) if actual==expected),
        "expected {expected:?}, got {result:?}"
    );
}
fn reference(result: &PluginOperationResponse, kind: PluginReferenceKind) -> PluginReference {
    let artifact = &result.installation.artifacts[0];
    PluginReference {
        kind,
        owner: "owner/config-or-plan".into(),
        installation_id: result.installation.installation_id.clone(),
        expected_last_operation: result.operation_id.clone(),
        package_digest: artifact.package_digest.clone(),
        artifact_digest: artifact.artifact_digest.clone(),
    }
}
fn uninstall_request(result: &PluginOperationResponse, key: &str) -> PluginUninstallRequest {
    PluginUninstallRequest {
        expected_last_operation: result.operation_id.clone(),
        operation_key: key.into(),
    }
}

#[tokio::test]
async fn validated_upload_is_not_install_and_exact_published_bytes_survive_restart() {
    let f = Fixture::new().await;
    let upload = f.upload();
    assert_eq!(f.count("plugin_installations"), 0);
    let installed = f.install(&upload, "echo");
    let installation = &installed.installation.installation_id;
    let artifact = &upload.artifact;
    assert_ne!(artifact.package_digest, artifact.artifact_digest);
    assert_eq!(
        artifact.config_schema.digest,
        format!("sha256:{}", hash(artifact.config_schema.dcdl.as_bytes()))
    );
    let fetched = f
        .store
        .plugin_artifact_bytes(&f.root(), "a", installation, &artifact.artifact_digest)
        .unwrap();
    assert_eq!(fetched, f.bytes);
    assert_eq!(fetched.len() as u64, u64::from(artifact.artifact_size));
    plugin::verify_plugin_artifact(
        &fetched,
        &artifact.package_digest,
        &artifact.artifact_digest,
        u64::from(artifact.artifact_size),
        &PluginPackageLimits::default(),
    )
    .unwrap();
    let reopened = SqliteWorkspaceStore::open_for_test(f.dir.path().join("server.db")).unwrap();
    assert_eq!(
        reopened.plugin_show("a", installation).unwrap(),
        installed.installation
    );
    assert_eq!(
        reopened
            .plugin_artifact_bytes(&f.root(), "a", installation, &artifact.artifact_digest)
            .unwrap(),
        fetched
    );
    error(
        reopened.plugin_artifact_bytes(&f.root(), "b", installation, &artifact.artifact_digest),
        PluginCatalogError::NotFound,
    );
    error(
        reopened.plugin_operation("b", &installed.operation_id),
        PluginCatalogError::NotFound,
    );
    error(
        reopened.plugin_show("b", installation),
        PluginCatalogError::NotFound,
    );
}

#[tokio::test]
async fn replay_returns_original_receipt_not_latest_catalog_and_survives_uninstall() {
    let f = Fixture::new().await;
    let upload = f.upload();
    let first = f.install(&upload, "echo");
    let mut request = PluginInstallRequest {
        upload_id: upload.upload_id.clone(),
        alias: "echo".into(),
        operation_key: "echo".into(),
    };
    assert_eq!(
        f.store.plugin_install("a", "actor", &request).unwrap(),
        first
    );
    request.alias = "changed".into();
    error(
        f.store.plugin_install("a", "actor", &request),
        PluginCatalogError::Conflict,
    );
    request.alias = "echo".into();
    error(
        f.store.plugin_install("a", "other-actor", &request),
        PluginCatalogError::NotFound,
    );
    let next = f
        .store
        .plugin_upload(&f.root(), "a", "actor", &package("0.2.0", ""))
        .unwrap();
    let added = f
        .store
        .plugin_add_artifact(
            "a",
            "actor",
            &first.installation.installation_id,
            &PluginAddArtifactRequest {
                upload_id: next.upload_id,
                expected_last_operation: first.operation_id.clone(),
                operation_key: "add".into(),
            },
        )
        .unwrap();
    assert_eq!(
        f.store.plugin_install("a", "actor", &request).unwrap(),
        first
    );
    error(
        f.store.plugin_uninstall(
            "a",
            "actor",
            &first.installation.installation_id,
            &uninstall_request(&first, "remove"),
        ),
        PluginCatalogError::Conflict,
    );
    let uninstalled = f
        .store
        .plugin_uninstall(
            "a",
            "actor",
            &first.installation.installation_id,
            &uninstall_request(&added, "remove"),
        )
        .unwrap();
    assert!(uninstalled.installation.uninstalled);
    f.expire();
    f.store.plugin_gc(&f.root()).unwrap();
    assert_eq!(
        f.store.plugin_install("a", "actor", &request).unwrap(),
        first
    );
    assert_eq!(
        f.store
            .plugin_uninstall(
                "a",
                "actor",
                &first.installation.installation_id,
                &uninstall_request(&added, "remove")
            )
            .unwrap(),
        uninstalled
    );
    assert_eq!(
        f.store.plugin_operation("a", &first.operation_id).unwrap(),
        first
    );
    assert_eq!(f.count("plugin_operations"), 3);
    assert_eq!(f.count("plugin_artifacts"), 2);
}

#[tokio::test]
async fn version_collision_and_revoke_are_not_overwrite_or_reactivation() {
    let f = Fixture::new().await;
    let upload = f.upload();
    let first = f.install(&upload, "echo");
    let installation = &first.installation.installation_id;
    let collision = f
        .store
        .plugin_upload(
            &f.root(),
            "a",
            "actor",
            &package("0.1.0", "different bytes"),
        )
        .unwrap();
    error(
        f.store.plugin_add_artifact(
            "a",
            "actor",
            installation,
            &PluginAddArtifactRequest {
                upload_id: collision.upload_id,
                expected_last_operation: first.operation_id.clone(),
                operation_key: "collide".into(),
            },
        ),
        PluginCatalogError::Conflict,
    );
    let revoked = f
        .store
        .plugin_revoke(
            "a",
            "actor",
            installation,
            &PluginRevokeRequest {
                artifact_digest: upload.artifact.artifact_digest.clone(),
                expected_last_operation: first.operation_id.clone(),
                reason: "withdrawn".into(),
                operation_key: "revoke".into(),
            },
        )
        .unwrap();
    assert!(revoked.installation.artifacts[0].revoked);
    let add = PluginAddArtifactRequest {
        upload_id: upload.upload_id.clone(),
        expected_last_operation: revoked.operation_id.clone(),
        operation_key: "reactivate".into(),
    };
    error(
        f.store
            .plugin_add_artifact("a", "actor", installation, &add),
        PluginCatalogError::Conflict,
    );
    error(
        f.store.plugin_artifact_bytes(
            &f.root(),
            "a",
            installation,
            &upload.artifact.artifact_digest,
        ),
        PluginCatalogError::Conflict,
    );
    let mut pin = reference(&revoked, PluginReferenceKind::Restorable);
    f.store
        .with_conn_mut(|c| {
            let tx = c.transaction()?;
            error(
                plugin_reference_put(&tx, "a", &pin),
                PluginCatalogError::Conflict,
            );
            Ok(())
        })
        .unwrap();
    let other = f.install(&upload, "new-installation");
    assert_ne!(other.installation.installation_id, *installation);
    pin.installation_id = other.installation.installation_id.clone();
    pin.expected_last_operation = other.operation_id;
    f.store
        .with_conn_mut(|c| {
            let tx = c.transaction()?;
            plugin_reference_put(&tx, "a", &pin)?;
            tx.commit()?;
            Ok(())
        })
        .unwrap();
    let rejected=f.store.with_conn(|c|{c.execute("UPDATE plugin_artifacts SET revoked=0,revocation_reason=NULL WHERE workspace_id='a' AND installation_id=?1",[installation])?;Ok(())});
    assert!(matches!(rejected, Err(Error::Sqlite(_))));
}

#[tokio::test]
async fn references_compose_with_owner_transaction_rollback_and_fence_all_lifetimes() {
    let f = Fixture::new().await;
    let upload = f.upload();
    let first = f.install(&upload, "echo");
    let installation = &first.installation.installation_id;
    f.store
        .with_conn_mut(|c| {
            c.execute(
                "CREATE TABLE fixture_config_commits (content_digest TEXT PRIMARY KEY)",
                [],
            )?;
            let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
            tx.execute(
                "INSERT INTO fixture_config_commits VALUES ('uncommitted')",
                [],
            )?;
            plugin_reference_put(&tx, "a", &reference(&first, PluginReferenceKind::Config))?;
            tx.rollback()?;
            Ok(())
        })
        .unwrap();
    assert_eq!(f.count("plugin_references"), 0);
    assert_eq!(f.count("fixture_config_commits"), 0);
    for kind in [
        PluginReferenceKind::Config,
        PluginReferenceKind::Pending,
        PluginReferenceKind::Active,
        PluginReferenceKind::Restorable,
    ] {
        f.store
            .with_conn_mut(|c| {
                let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
                plugin_reference_put(&tx, "a", &reference(&first, kind))?;
                tx.commit()?;
                Ok(())
            })
            .unwrap();
        error(
            f.store.plugin_uninstall(
                "a",
                "actor",
                installation,
                &uninstall_request(&first, "remove"),
            ),
            PluginCatalogError::Referenced,
        );
    }
    let mut stale = reference(&first, PluginReferenceKind::Config);
    stale.expected_last_operation = "stale".into();
    f.store
        .with_conn_mut(|c| {
            let tx = c.transaction()?;
            error(
                plugin_reference_put(&tx, "a", &stale),
                PluginCatalogError::Conflict,
            );
            stale.expected_last_operation = first.operation_id.clone();
            stale.package_digest = "0".repeat(64);
            error(
                plugin_reference_put(&tx, "a", &stale),
                PluginCatalogError::Conflict,
            );
            error(
                plugin_reference_put(&tx, "b", &reference(&first, PluginReferenceKind::Config)),
                PluginCatalogError::NotFound,
            );
            Ok(())
        })
        .unwrap();
    let revoked = f
        .store
        .plugin_revoke(
            "a",
            "actor",
            installation,
            &PluginRevokeRequest {
                artifact_digest: upload.artifact.artifact_digest.clone(),
                expected_last_operation: first.operation_id.clone(),
                operation_key: "revoke".into(),
                reason: "withdrawn".into(),
            },
        )
        .unwrap();
    assert_eq!(f.count("plugin_references"), 4);
    error(
        f.store.plugin_uninstall(
            "a",
            "actor",
            installation,
            &uninstall_request(&revoked, "remove"),
        ),
        PluginCatalogError::Referenced,
    );
    f.store
        .with_conn_mut(|c| {
            let tx = c.transaction()?;
            for kind in [
                PluginReferenceKind::Config,
                PluginReferenceKind::Pending,
                PluginReferenceKind::Active,
                PluginReferenceKind::Restorable,
            ] {
                plugin_reference_remove(
                    &tx,
                    "a",
                    kind,
                    "owner/config-or-plan",
                    installation,
                    &upload.artifact.artifact_digest,
                )?;
            }
            tx.commit()?;
            Ok(())
        })
        .unwrap();
    let removed = f
        .store
        .plugin_uninstall(
            "a",
            "actor",
            installation,
            &uninstall_request(&revoked, "remove"),
        )
        .unwrap();
    f.store
        .with_conn_mut(|c| {
            let tx = c.transaction()?;
            let mut pin = reference(&removed, PluginReferenceKind::Pending);
            pin.expected_last_operation = removed.operation_id.clone();
            error(
                plugin_reference_put(&tx, "a", &pin),
                PluginCatalogError::NotFound,
            );
            Ok(())
        })
        .unwrap();
}

#[tokio::test]
async fn database_failure_rolls_back_domain_and_receipt_then_exact_retry_succeeds() {
    let f = Fixture::new().await;
    let upload = f.upload();
    f.store.with_conn(|c|{c.execute_batch("CREATE TRIGGER fail_plugin_receipt BEFORE INSERT ON plugin_operations BEGIN SELECT RAISE(ABORT,'injected receipt failure'); END;")?;Ok(())}).unwrap();
    let request = PluginInstallRequest {
        upload_id: upload.upload_id,
        alias: "echo".into(),
        operation_key: "install".into(),
    };
    assert!(matches!(
        f.store.plugin_install("a", "actor", &request),
        Err(Error::Sqlite(_))
    ));
    assert_eq!(f.count("plugin_installations"), 0);
    assert_eq!(f.count("plugin_artifacts"), 0);
    assert_eq!(f.count("plugin_operations"), 0);
    f.store
        .with_conn(|c| {
            c.execute_batch("DROP TRIGGER fail_plugin_receipt")?;
            Ok(())
        })
        .unwrap();
    let installed = f.store.plugin_install("a", "actor", &request).unwrap();
    assert_eq!(
        f.store.plugin_install("a", "actor", &request).unwrap(),
        installed
    );
}

#[tokio::test]
async fn upload_and_publish_failures_leave_no_authority_and_restart_gc_recovers_orphans() {
    let f = Fixture::new().await;
    for bytes in [&b"not a zip"[..], &f.bytes[..f.bytes.len() - 2]] {
        error(
            f.store.plugin_upload(&f.root(), "a", "actor", bytes),
            PluginCatalogError::Invalid,
        );
    }
    error(
        f.store.plugin_upload(
            &f.root(),
            "a",
            "actor",
            &vec![0; PluginPackageLimits::default().max_package_size_bytes as usize + 1],
        ),
        PluginCatalogError::TooLarge,
    );
    assert_eq!(f.count("plugin_uploads"), 0);
    fs::write(f.dir.path().join("blocked"), "not a directory").unwrap();
    error(
        f.store
            .plugin_upload(&f.dir.path().join("blocked"), "a", "actor", &f.bytes),
        PluginCatalogError::Unavailable,
    );
    f.store.with_conn(|c|{c.execute_batch("CREATE TRIGGER fail_upload_commit BEFORE INSERT ON plugin_uploads BEGIN SELECT RAISE(ABORT,'injected upload DB failure'); END;")?;Ok(())}).unwrap();
    assert!(matches!(
        f.store.plugin_upload(&f.root(), "a", "actor", &f.bytes),
        Err(Error::Sqlite(_))
    ));
    assert_eq!(f.count("plugin_uploads"), 0);
    assert_eq!(fs::read_dir(f.root().join("blobs")).unwrap().count(), 1);
    fs::write(f.root().join("blobs/.pending-interrupted"), "partial").unwrap();
    let reopened = SqliteWorkspaceStore::open_for_test(f.dir.path().join("server.db")).unwrap();
    assert_eq!(reopened.plugin_gc(&f.root()).unwrap(), 2);
    assert_eq!(fs::read_dir(f.root().join("blobs")).unwrap().count(), 0);
    f.store
        .with_conn(|c| {
            c.execute_batch("DROP TRIGGER fail_upload_commit")?;
            Ok(())
        })
        .unwrap();
    let upload = f.upload();
    let first = f.install(&upload, "echo");
    fs::write(
        blob_path(&f.root(), &upload.artifact.artifact_digest).unwrap(),
        "corrupt",
    )
    .unwrap();
    error(
        f.store.plugin_artifact_bytes(
            &f.root(),
            "a",
            &first.installation.installation_id,
            &upload.artifact.artifact_digest,
        ),
        PluginCatalogError::Unavailable,
    );
    error(
        f.store.plugin_upload(&f.root(), "a", "actor", &f.bytes),
        PluginCatalogError::Unavailable,
    );
}

#[tokio::test]
async fn upload_actor_workspace_expiry_and_quotas_are_enforced_without_partial_rows() {
    let f = Fixture::new().await;
    let upload = f.upload();
    let request = PluginInstallRequest {
        upload_id: upload.upload_id.clone(),
        alias: "echo".into(),
        operation_key: "install".into(),
    };
    error(
        f.store.plugin_install("a", "other", &request),
        PluginCatalogError::NotFound,
    );
    error(
        f.store.plugin_install("b", "actor", &request),
        PluginCatalogError::NotFound,
    );
    f.expire();
    error(
        f.store.plugin_install("a", "actor", &request),
        PluginCatalogError::NotFound,
    );
    assert_eq!(f.count("plugin_installations"), 0);
    for _ in 0..MAX_ACTOR_UPLOADS {
        f.upload();
    }
    error(
        f.store.plugin_upload(&f.root(), "a", "actor", &f.bytes),
        PluginCatalogError::Quota,
    );
    f.store
        .plugin_upload(&f.root(), "a", "other", &f.bytes)
        .unwrap();
    f.store
        .plugin_upload(&f.root(), "b", "actor", &f.bytes)
        .unwrap();
    f.expire();
    f.upload();
    assert_eq!(f.count("plugin_operations"), 0);
}

#[tokio::test]
async fn global_gc_keeps_other_workspace_blob_and_historical_receipts_after_last_reference() {
    let f = Fixture::new().await;
    let a_upload = f.upload();
    let a = f.install(&a_upload, "echo");
    let b_upload = f
        .store
        .plugin_upload(&f.root(), "b", "actor", &f.bytes)
        .unwrap();
    let b = f
        .store
        .plugin_install(
            "b",
            "actor",
            &PluginInstallRequest {
                upload_id: b_upload.upload_id,
                alias: "echo".into(),
                operation_key: "install".into(),
            },
        )
        .unwrap();
    f.store
        .plugin_uninstall(
            "a",
            "actor",
            &a.installation.installation_id,
            &uninstall_request(&a, "remove"),
        )
        .unwrap();
    f.expire();
    assert_eq!(f.store.plugin_gc(&f.root()).unwrap(), 0);
    assert_eq!(
        f.store
            .plugin_artifact_bytes(
                &f.root(),
                "b",
                &b.installation.installation_id,
                &a_upload.artifact.artifact_digest
            )
            .unwrap(),
        f.bytes
    );
    f.store
        .plugin_uninstall(
            "b",
            "actor",
            &b.installation.installation_id,
            &uninstall_request(&b, "remove"),
        )
        .unwrap();
    assert_eq!(f.store.plugin_gc(&f.root()).unwrap(), 1);
    assert_eq!(f.store.plugin_operation("a", &a.operation_id).unwrap(), a);
    assert_eq!(f.count("plugin_artifacts"), 2);
}

#[tokio::test]
async fn independent_connections_cannot_uninstall_or_gc_across_a_reference_transaction() {
    let f = Fixture::new().await;
    let upload = f.upload();
    let first = f.install(&upload, "echo");
    f.expire();
    let second = SqliteWorkspaceStore::open_for_test(f.dir.path().join("server.db")).unwrap();
    second
        .with_conn(|c| {
            c.busy_timeout(std::time::Duration::ZERO)?;
            Ok(())
        })
        .unwrap();
    f.store.with_conn_mut(|c|{
        let tx=c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        plugin_reference_put(&tx,"a",&reference(&first,PluginReferenceKind::Pending))?;
        let denied=second.plugin_uninstall("a","actor",&first.installation.installation_id,&uninstall_request(&first,"remove"));
        assert!(matches!(denied,Err(Error::Sqlite(rusqlite::Error::SqliteFailure(e,_))) if e.code==rusqlite::ErrorCode::DatabaseBusy));
        assert!(matches!(second.plugin_gc(&f.root()),Err(Error::Sqlite(rusqlite::Error::SqliteFailure(e,_))) if e.code==rusqlite::ErrorCode::DatabaseBusy));
        assert!(blob_path(&f.root(),&upload.artifact.artifact_digest)?.exists());
        tx.commit()?;Ok(())
    }).unwrap();
    error(
        second.plugin_uninstall(
            "a",
            "actor",
            &first.installation.installation_id,
            &uninstall_request(&first, "remove"),
        ),
        PluginCatalogError::Referenced,
    );
    assert_eq!(second.plugin_gc(&f.root()).unwrap(), 0);
    // Reverse ordering: after removal commits, a stale plan cannot recreate a pin.
    f.store
        .with_conn_mut(|c| {
            let tx = c.transaction()?;
            plugin_reference_remove(
                &tx,
                "a",
                PluginReferenceKind::Pending,
                "owner/config-or-plan",
                &first.installation.installation_id,
                &upload.artifact.artifact_digest,
            )?;
            tx.commit()?;
            Ok(())
        })
        .unwrap();
    second
        .plugin_uninstall(
            "a",
            "actor",
            &first.installation.installation_id,
            &uninstall_request(&first, "remove"),
        )
        .unwrap();
    f.store
        .with_conn_mut(|c| {
            let tx = c.transaction()?;
            error(
                plugin_reference_put(&tx, "a", &reference(&first, PluginReferenceKind::Active)),
                PluginCatalogError::NotFound,
            );
            Ok(())
        })
        .unwrap();
    assert_eq!(f.store.plugin_gc(&f.root()).unwrap(), 1);
}

#[tokio::test]
async fn bounded_opaque_list_cursor_is_workspace_bound_and_survives_catalog_changes() {
    let f = Fixture::new().await;
    let upload = f.upload();
    let first = f.install(&upload, "one");
    f.install(&upload, "two");
    let page = f
        .store
        .plugin_list(
            "a",
            &PluginListRequest {
                cursor: None,
                limit: Some(1),
            },
        )
        .unwrap();
    assert_eq!(page.installations.len(), 1);
    assert!(page.next_cursor.is_some());
    f.store
        .plugin_uninstall(
            "a",
            "actor",
            &first.installation.installation_id,
            &uninstall_request(&first, "remove"),
        )
        .unwrap();
    let next = f
        .store
        .plugin_list(
            "a",
            &PluginListRequest {
                cursor: page.next_cursor.clone(),
                limit: Some(1),
            },
        )
        .unwrap();
    assert_eq!(next.installations.len(), 1);
    assert!(next.next_cursor.is_none());
    error(
        f.store.plugin_list(
            "b",
            &PluginListRequest {
                cursor: page.next_cursor,
                limit: Some(1),
            },
        ),
        PluginCatalogError::Invalid,
    );
    error(
        f.store.plugin_list(
            "a",
            &PluginListRequest {
                cursor: None,
                limit: Some(101),
            },
        ),
        PluginCatalogError::Invalid,
    );
    error(
        f.store.plugin_list(
            "a",
            &PluginListRequest {
                cursor: Some("nonsense".into()),
                limit: None,
            },
        ),
        PluginCatalogError::Invalid,
    );
}

#[tokio::test]
async fn destructive_mutations_roll_back_when_their_atomic_receipt_cannot_commit() {
    let f = Fixture::new().await;
    let upload = f.upload();
    let installed = f.install(&upload, "echo");
    let installation = &installed.installation.installation_id;
    f.store.with_conn(|c| { c.execute_batch("CREATE TRIGGER fail_receipt BEFORE INSERT ON plugin_operations BEGIN SELECT RAISE(ABORT,'receipt storage failure'); END;")?; Ok(()) }).unwrap();
    let revoke = PluginRevokeRequest {
        artifact_digest: upload.artifact.artifact_digest,
        expected_last_operation: installed.operation_id.clone(),
        reason: "withdrawn".into(),
        operation_key: "revoke".into(),
    };
    assert!(matches!(
        f.store.plugin_revoke("a", "actor", installation, &revoke),
        Err(Error::Sqlite(_))
    ));
    assert_eq!(
        f.store.plugin_show("a", installation).unwrap(),
        installed.installation
    );
    let uninstall = uninstall_request(&installed, "remove");
    assert!(matches!(
        f.store
            .plugin_uninstall("a", "actor", installation, &uninstall),
        Err(Error::Sqlite(_))
    ));
    assert_eq!(
        f.store.plugin_show("a", installation).unwrap(),
        installed.installation
    );
    assert_eq!(f.count("plugin_operations"), 1);
    f.store
        .with_conn(|c| {
            c.execute_batch("DROP TRIGGER fail_receipt")?;
            Ok(())
        })
        .unwrap();
    let revoked = f
        .store
        .plugin_revoke("a", "actor", installation, &revoke)
        .unwrap();
    assert_eq!(
        f.store
            .plugin_revoke("a", "actor", installation, &revoke)
            .unwrap(),
        revoked
    );
    error(
        f.store
            .plugin_uninstall("a", "actor", installation, &uninstall),
        PluginCatalogError::Conflict,
    );
    let mut changed = revoke.clone();
    changed.reason = "changed reason".into();
    error(
        f.store.plugin_revoke("a", "actor", installation, &changed),
        PluginCatalogError::Conflict,
    );
}

#[tokio::test]
async fn upload_workspace_quota_counts_all_actors_and_installation_quota_is_atomic() {
    let f = Fixture::new().await;
    let upload = f.upload();
    // Clone real immutable upload references to the same published bytes to put
    // accounting precisely at its global boundary without allocating duplicate blobs.
    f.store.with_conn(|c| {
        for n in 1..MAX_UPLOADS {
            c.execute("INSERT INTO plugin_uploads SELECT workspace_id,?1,?2,expires_at,artifact_digest,artifact_size,metadata_json FROM plugin_uploads WHERE upload_id=?3",params![format!("upload-{n}"),format!("actor-{n}"),upload.upload_id])?;
        }
        Ok(())
    }).unwrap();
    error(
        f.store
            .plugin_upload(&f.root(), "a", "fresh-actor", &f.bytes),
        PluginCatalogError::Quota,
    );
    assert_eq!(f.count("plugin_uploads"), MAX_UPLOADS);
    for n in 0..MAX_INSTALLATIONS {
        f.install(&upload, &format!("install-{n}"));
    }
    let request = PluginInstallRequest {
        upload_id: upload.upload_id,
        alias: "excess".into(),
        operation_key: "excess".into(),
    };
    error(
        f.store.plugin_install("a", "actor", &request),
        PluginCatalogError::Quota,
    );
    assert_eq!(f.count("plugin_installations"), MAX_INSTALLATIONS);
    assert_eq!(f.count("plugin_operations"), MAX_INSTALLATIONS);
}
