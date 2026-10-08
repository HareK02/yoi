use super::*;
use workspace_drive::{Error as DriveError, Mutation};

#[test]
fn custom_database_does_not_change_explicit_drive_placement() {
    let temp = tempfile::tempdir().unwrap();
    let mut config = test_server_config(temp.path());
    config.database_path = temp.path().join("custom-authority/authority.sqlite");
    config.data_root = temp.path().join("explicit-yoi-data");
    let mut workspace = test_workspace();
    workspace.workspace_id = "other-internal-id".into();
    workspace.display_name = "../Not a storage key".into();
    let scoped = config
        .for_catalog_workspace(&workspace, Vec::new())
        .unwrap();
    assert_eq!(
        scoped.drive_blob_root().unwrap(),
        config.data_root.join("drives/other-internal-id/blobs")
    );
}

#[tokio::test]
async fn cached_drive_handle_rejects_mutation_after_deletion_reservation() {
    let temp = tempfile::tempdir().unwrap();
    let mut config = test_server_config(temp.path());
    config.repositories.clear();
    let store = Arc::new(test_control_store(&config));
    seed_test_registered_workspace(store.as_ref(), &config)
        .await
        .unwrap();
    let owner = format!("account-{}", config.workspace_id);
    store
        .upsert_workspace(&WorkspaceRecord {
            workspace_id: "retained-workspace".into(),
            owner_account_id: owner.clone(),
            display_name: "Retain".into(),
            state: "active".into(),
            created_at: "1".into(),
            updated_at: "1".into(),
        })
        .await
        .unwrap();
    let api = WorkspaceApi::new(config.clone(), store.clone())
        .await
        .unwrap();
    let cached = api.drive().await.unwrap();
    let for_root = cached.clone();
    let parent = tokio::task::spawn_blocking(move || for_root.root().unwrap().id)
        .await
        .unwrap();
    let preflight = store
        .workspace_deletion_preflight(&owner, &config.workspace_id)
        .unwrap();
    store
        .reserve_workspace_deletion(
            &owner,
            &config.workspace_id,
            &WorkspaceDeletionRequest {
                operation_id: "delete-cached-drive".into(),
                expected_revision: preflight.expected_revision,
                confirmation: preflight.display_name,
            },
        )
        .unwrap();
    // No WorkspaceApi access or host/domain fence between reservation and the
    // cached operation: the committed server authority must reject it directly.
    let result = tokio::task::spawn_blocking(move || {
        cached.mutate(
            "cached-write",
            "owner",
            Mutation::CreateFolder {
                parent,
                name: "forbidden".into(),
            },
        )
    })
    .await
    .unwrap();
    assert!(matches!(result, Err(DriveError::Fenced)), "{result:?}");
}

#[tokio::test]
async fn deleting_workspace_reopens_fenced_drive_and_purges_blobs_before_feature_storage() {
    let temp = tempfile::tempdir().unwrap();
    let mut config = test_server_config(temp.path());
    config.repositories.clear();
    let store = Arc::new(test_control_store(&config));
    seed_test_registered_workspace(store.as_ref(), &config)
        .await
        .unwrap();
    let owner = format!("account-{}", config.workspace_id);
    store
        .upsert_workspace(&WorkspaceRecord {
            workspace_id: "retained-workspace".into(),
            owner_account_id: owner.clone(),
            display_name: "Retain".into(),
            state: "active".into(),
            created_at: "1".into(),
            updated_at: "1".into(),
        })
        .await
        .unwrap();
    let root = config.drive_blob_root().unwrap();
    // Use a separate embedded-runtime namespace for the first API: this test
    // reconstructs Drive authority in-process rather than spawning/restarting
    // product processes (runtime observers may retain their namespace).
    let first_api = WorkspaceApi::new(config.clone(), store.clone())
        .await
        .unwrap();
    // Merely constructing a Workspace API must not create blob storage.
    assert!(!root.exists());
    let drive = first_api.drive().await.unwrap();
    let parent = tokio::task::spawn_blocking(move || {
        let parent = drive.root().unwrap().id;
        drive
            .mutate(
                "upload",
                "owner",
                Mutation::CreateFile {
                    parent,
                    name: "file".into(),
                    content_type: "text/plain".into(),
                    bytes: b"content".to_vec(),
                },
            )
            .unwrap();
        parent
    })
    .await
    .unwrap();
    let preflight = store
        .workspace_deletion_preflight(&owner, &config.workspace_id)
        .unwrap();
    store
        .reserve_workspace_deletion(
            &owner,
            &config.workspace_id,
            &WorkspaceDeletionRequest {
                operation_id: "delete-drive-workspace".into(),
                expected_revision: preflight.expected_revision,
                confirmation: preflight.display_name,
            },
        )
        .unwrap();
    // Blocked operations are not automatically resumed on restart, but their
    // Workspace remains deleting and must still reconstruct a fenced Drive.
    store
        .update_workspace_deletion_operation(
            "delete-drive-workspace",
            WorkspaceDeletionState::Blocked,
            &[],
            &[],
            None,
        )
        .unwrap();
    // Simulate a crash between the catalog reservation and Drive fencing.
    let feature_storage = first_api.feature_storage.clone();
    tokio::task::spawn_blocking(move || feature_storage.shutdown())
        .await
        .unwrap()
        .unwrap();
    drop(first_api);
    let recovered = WorkspaceServerApi::new(config.clone(), store.clone());
    let api = recovered
        .api_for_workspace(&config.workspace_id)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        api.drive().await,
        Err(Error::WorkspaceConfigConflict(_))
    ));
    let reopened = api
        .drive_host
        .open(
            &api.feature_storage,
            root.clone(),
            config.database_path.clone(),
        )
        .await
        .unwrap();
    let mutation = tokio::task::spawn_blocking(move || {
        reopened.mutate(
            "restart-write",
            "owner",
            Mutation::CreateFolder {
                parent,
                name: "forbidden".into(),
            },
        )
    })
    .await
    .unwrap();
    assert!(matches!(mutation, Err(DriveError::Fenced)), "{mutation:?}");
    let completed = recovered
        .execute_workspace_deletion("delete-drive-workspace")
        .await
        .unwrap();
    assert_eq!(completed.state, WorkspaceDeletionState::Succeeded);
    assert!(!root.exists());
    assert!(
        !config
            .database_path
            .parent()
            .unwrap()
            .join("feature-storage")
            .join(&config.workspace_id)
            .exists()
    );
    assert!(
        store
            .get_workspace("retained-workspace")
            .await
            .unwrap()
            .is_some()
    );
}
