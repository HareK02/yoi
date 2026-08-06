//! Legacy process callback events are diagnostics only after Internal SubWorker migration.

use std::sync::Arc;

use protocol::{Permission, ScopeRule, WorkerEvent};
use tempfile::TempDir;
use worker::ipc::event::{apply_event_side_effects, render_event};
use worker::runtime::dir::RuntimeDir;
use worker::spawn::registry::SpawnedWorkerRegistry;

#[test]
fn render_event_keeps_bounded_legacy_diagnostics() {
    let rendered = render_event(&WorkerEvent::Errored {
        worker_name: "legacy-child".into(),
        message: "boom".into(),
    });
    assert!(rendered.contains("legacy-child"));
    assert!(rendered.contains("boom"));
}

#[tokio::test]
async fn legacy_callback_cannot_register_process_subworker_authority() {
    let runtime_base = TempDir::new().unwrap();
    let runtime_dir = Arc::new(
        RuntimeDir::create(runtime_base.path(), "parent")
            .await
            .unwrap(),
    );
    let registry = SpawnedWorkerRegistry::new(runtime_dir.clone());
    let scope_root = TempDir::new().unwrap();
    let event = WorkerEvent::ScopeSubDelegated {
        parent_worker: "legacy-parent".into(),
        sub_worker: "legacy-child".into(),
        sub_socket: "/tmp/legacy-child.sock".into(),
        scope: vec![ScopeRule {
            target: scope_root.path().to_path_buf(),
            permission: Permission::Write,
            recursive: true,
        }],
    };

    apply_event_side_effects(&event, &registry, "parent", &None).await;

    assert!(!runtime_dir.path().join("spawned_workers.json").exists());
}
