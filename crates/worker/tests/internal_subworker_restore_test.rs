use std::sync::Arc;

use manifest::{Permission, Scope, ScopeConfig, ScopeRule, SharedScope};
use session_store::{
    CombinedStore, FsStore, FsWorkerStore, WorkerMetadata, WorkerMetadataStore, WorkerSpawnedChild,
    WorkerSpawnedScopeRule,
};
use tempfile::TempDir;
use worker::runtime::dir::RuntimeDir;
use worker::spawn::registry::SpawnedWorkerRegistry;

#[tokio::test]
async fn restore_reclaims_and_clears_legacy_process_children() {
    let runtime = TempDir::new().unwrap();
    let sessions = TempDir::new().unwrap();
    let scope_root = TempDir::new().unwrap();
    let store = CombinedStore::new(
        FsStore::new(sessions.path()).unwrap(),
        FsWorkerStore::new(sessions.path().join("workers")).unwrap(),
    );
    let mut metadata = WorkerMetadata::new("parent", None);
    metadata.spawned_children.push(WorkerSpawnedChild {
        worker_name: "legacy-child".into(),
        socket_path: runtime.path().join("legacy.sock"),
        callback_address: runtime.path().join("parent.sock"),
        scope_delegated: vec![WorkerSpawnedScopeRule {
            target: scope_root.path().to_path_buf(),
            permission: "write".into(),
            recursive: true,
        }],
    });
    store.write(&metadata).unwrap();

    let write_rule = ScopeRule {
        target: scope_root.path().to_path_buf(),
        permission: Permission::Write,
        recursive: true,
    };
    let parent_scope = SharedScope::new(
        Scope::from_config(&ScopeConfig {
            allow: vec![write_rule.clone()],
            deny: vec![write_rule],
        })
        .unwrap(),
    );
    assert!(!parent_scope.snapshot().is_writable(scope_root.path()));
    let runtime_dir = Arc::new(RuntimeDir::create(runtime.path(), "parent").await.unwrap());

    let loaded = SpawnedWorkerRegistry::load_from_worker_state_with_reclaim(
        runtime_dir.clone(),
        store.clone(),
        "parent".into(),
        Some(parent_scope.clone()),
    )
    .await
    .unwrap();

    assert!(loaded.reclaimed_unreachable);
    assert!(parent_scope.snapshot().is_writable(scope_root.path()));
    let metadata = store.read_by_name("parent").unwrap().unwrap();
    assert!(metadata.spawned_children.is_empty());
    assert_eq!(metadata.reclaimed_children.len(), 1);
    assert_eq!(metadata.reclaimed_children[0].worker_name, "legacy-child");
    let runtime_projection =
        std::fs::read_to_string(runtime_dir.path().join("spawned_workers.json")).unwrap();
    assert_eq!(runtime_projection.trim(), "[]");
}
