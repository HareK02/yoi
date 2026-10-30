//! Focused physical-lifetime/live-authority regressions through existing owners.
//! Storage counters observe real directory sync/open; they never replace that I/O.
use super::*;

pub(super) fn independent_store(
    api: &WorkspaceApi,
) -> (crate::FeatureStorage, crate::subjektiv::SubjektivStore) {
    let manager = crate::FeatureStorage::for_server_database(&api.config.database_path).unwrap();
    let scope = manager.workspace(TEST_WORKSPACE_ID).unwrap();
    let registration = crate::subjektiv::SubjektivStore::register(&scope).unwrap();
    let store = crate::subjektiv::SubjektivStore::open(&scope, &registration).unwrap();
    // Handles do not own manager lifetime: dropping it intentionally fences them.
    (manager, store)
}

fn permission(f: &Fixture) -> server_api::authorization::OperationAuthorizationRequest {
    server_api::authorization::OperationAuthorizationRequest {
        operation: "plugin.admit".into(),
        resource: PermissionResource::PluginInstallation {
            workspace_id: TEST_WORKSPACE_ID.into(),
            installation_id: f.installed.installation.installation_id.clone(),
        },
    }
}

fn revoke(store: &crate::subjektiv::SubjektivStore, subject: &str) {
    let previous = store.subject_permissions(subject).unwrap().unwrap();
    store
        .set_subject_permissions(
            subject,
            &SubjectPermissionsUpdate {
                operation_id: "external-revoke".into(),
                expected_change_id: Some(previous.change_id),
                execution: vec![],
                delegation: vec![],
            },
        )
        .unwrap();
}

#[tokio::test]
async fn sync_authority_checks_reduce_physical_initializations_without_skipping_live_reads() {
    let f = Fixture::new(None).await;
    // Call the unchanged synchronous owners both without and with physical-handle
    // retention. These are the identity and dual-permission portions of Host checks.
    for identity in [true, false] {
        let check = || {
            if identity {
                subject_authorization::plugin_execution_identity(&f.api, &f.source)
                    .map(|_| ())
                    .map_err(|e| e.error)
            } else {
                for operation in ["plugin.service.consume", "plugin.admit"] {
                    let mut request = permission(&f);
                    request.operation = operation.into();
                    subject_authorization::authorize_scope(&f.api, &f.source, &request)
                        .map_err(|e| e.error)?;
                }
                Ok(())
            }
        };
        let before = f.api.subjektiv_registration.open_observation();
        check().unwrap();
        let unretained = f
            .api
            .subjektiv_registration
            .open_observation()
            .since(before);
        assert!(unretained.physical_opens > 1, "{unretained:?}");
        assert_check_physical_initializations(&f, before, unretained.physical_opens);
        let before = f.api.subjektiv_registration.open_observation();
        plugin_host::with_authority_database(&f.api, check).unwrap();
        assert_check_physical_initializations(&f, before, 1);
        let retained = f
            .api
            .subjektiv_registration
            .open_observation()
            .since(before);
        println!(
            "authority_physical_io identity={identity} before={unretained:?} after={retained:?}"
        );
    }
    f.shutdown().await;
}

#[tokio::test]
async fn scoped_subject_database_rereads_policy_changed_on_independent_connection() {
    let f = Fixture::new(None).await;
    let (_manager, other) = independent_store(&f.api);
    let request = permission(&f);
    let before = f.api.subjektiv_registration.open_observation();
    plugin_host::with_authority_database(&f.api, || {
        subject_authorization::authorize_scope(&f.api, &f.source, &request).unwrap();
        revoke(&other, &f.subject);
        let error =
            subject_authorization::authorize_scope(&f.api, &f.source, &request).unwrap_err();
        assert!(matches!(error.error, Error::WorkspacePermissionDenied(_)));
        Ok(())
    })
    .unwrap();
    assert_check_physical_initializations(&f, before, 1);
    drop(other);
    f.shutdown().await;
}

#[tokio::test]
async fn scoped_subject_database_does_not_survive_deletion_or_refresh_itself_after_restore() {
    let f = Fixture::new(None).await;
    let snapshot_dir = tempfile::tempdir().unwrap();
    let snapshot = snapshot_dir.path().join("snapshot");
    f.api.feature_storage.backup(&snapshot).unwrap();
    let request = permission(&f);
    plugin_host::with_authority_database(&f.api, || {
        subject_authorization::authorize_scope(&f.api, &f.source, &request).unwrap();
        // The guard retains no connection lock: deletion completes synchronously.
        f.api.feature_storage.delete().unwrap();
        assert!(subject_authorization::authorize_scope(&f.api, &f.source, &request).is_err());
        assert!(f.api.feature_storage.restore(&snapshot).is_err());
        let fresh =
            crate::FeatureStorage::for_server_database(&f.api.config.database_path).unwrap();
        let scope = fresh.workspace(TEST_WORKSPACE_ID).unwrap();
        scope.restore(&snapshot).unwrap();
        let registration = crate::subjektiv::SubjektivStore::register(&scope).unwrap();
        let mut restored = f.api.clone();
        restored.feature_storage = scope;
        restored.subjektiv_registration = registration;
        // A fresh Host binding can read the restored DB; the old binding stays fenced.
        subject_authorization::authorize_scope(&restored, &f.source, &request).unwrap();
        assert!(subject_authorization::authorize_scope(&f.api, &f.source, &request).is_err());
        restored.feature_storage.shutdown().unwrap();
        Ok(())
    })
    .unwrap();
    f.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_authority_waiter_keeps_physical_database_until_accepted_check_finishes() {
    let f = Fixture::new(None).await;
    let (_manager, other) = independent_store(&f.api);
    let request = permission(&f);
    let executor = f.api.config_store.db_executor();
    let api = f.api.clone();
    let worker = f.source.clone();
    let (entered, started) = tokio::sync::oneshot::channel();
    let (release, released) = std::sync::mpsc::sync_channel(0);
    let denied = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let checked = denied.clone();
    let lane = executor.clone();
    let before = f.api.subjektiv_registration.open_observation();
    let waiter = tokio::spawn(async move {
        lane.run(move || {
            plugin_host::with_authority_database(&api, || {
                subject_authorization::authorize_scope(&api, &worker, &request).unwrap();
                entered.send(()).unwrap();
                released.recv().unwrap();
                checked.store(
                    subject_authorization::authorize_scope(&api, &worker, &request).is_err(),
                    std::sync::atomic::Ordering::SeqCst,
                );
                Ok(())
            })
        })
        .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(10), started)
        .await
        .unwrap()
        .unwrap();
    waiter.abort();
    assert!(waiter.await.unwrap_err().is_cancelled());
    // This open reuses the accepted job's physical handle even after its waiter dies.
    drop(
        f.api
            .feature_storage
            .open(&f.api.subjektiv_registration)
            .unwrap(),
    );
    assert_check_physical_initializations(&f, before, 1);
    revoke(&other, &f.subject);
    release.send(()).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(10), executor.drain())
        .await
        .unwrap()
        .unwrap();
    assert!(denied.load(std::sync::atomic::Ordering::SeqCst));
    assert_check_physical_initializations(&f, before, 1);
    // Draining the accepted work released the guard, rather than creating a cache.
    drop(
        f.api
            .feature_storage
            .open(&f.api.subjektiv_registration)
            .unwrap(),
    );
    assert_check_physical_initializations(&f, before, 2);
    drop(other);
    f.shutdown().await;
}
