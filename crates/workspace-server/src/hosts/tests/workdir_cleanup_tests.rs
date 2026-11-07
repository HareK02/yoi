//! Exercises the production RemoteWorkerRuntime HTTP client against a gated loopback
//! provider. Only the provider is replaced; no live Runtime or Workdir is used.
//! Runs with the ordinary workspace-server crate tests.

use super::*;
use axum::{Json, Router, routing::delete};
use tokio::{sync::Notify, task::JoinSet, time::timeout};

#[tokio::test]
async fn workdir_cleanup_waits_past_ordinary_request_timeout_for_provider_confirmation() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let runtime = RemoteWorkerRuntime::new(
        RemoteRuntimeConfig::new(
            "runtime-test",
            "Test Runtime",
            format!("http://{}", listener.local_addr().unwrap()),
            Some("secret-token".into()),
        )
        .with_timeout(Duration::from_millis(100)),
        "workspace-test".into(),
        "http://127.0.0.1:1".into(),
    )
    .unwrap();
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let provider_entered = entered.clone();
    let provider_release = release.clone();
    let app = Router::new().route(
        "/v1/working-directories/workdir-test",
        delete(move |headers: axum::http::HeaderMap| async move {
            assert_eq!(headers[AUTHORIZATION], "Bearer secret-token");
            assert_eq!(headers["x-yoi-workspace-id"], "workspace-test");
            provider_entered.notify_one();
            // The fixture's deadline only bounds test failure/teardown. Completion
            // is controlled by the test after an ordinary request has timed out.
            let _ = timeout(Duration::from_secs(5), provider_release.notified()).await;
            Json(json!({
                "working_directory": { "summary": {
                    "working_directory_id": "workdir-test",
                    "repository_id": "main",
                    "materializer_kind": "runtime_git_clone",
                    "status": "not_found"
                }}
            }))
        })
        .get(|| async {
            let _ = timeout(Duration::from_secs(5), std::future::pending::<()>()).await;
            Json(json!({}))
        }),
    );
    let mut server = JoinSet::new();
    server.spawn(async move { axum::serve(listener, app).await.unwrap() });
    let cleanup_runtime = runtime.clone();
    let cleanup = tokio::task::spawn_blocking(move || {
        cleanup_runtime.cleanup_working_directory("workdir-test")
    });
    timeout(Duration::from_secs(5), entered.notified())
        .await
        .expect("provider received cleanup");

    let ordinary = timeout(
        Duration::from_secs(5),
        tokio::task::spawn_blocking(move || runtime.working_directory("workdir-test")),
    )
    .await
    .expect("ordinary request remains bounded")
    .unwrap();
    let cleanup_still_waiting = !cleanup.is_finished();
    release.notify_one();
    let removed = timeout(Duration::from_secs(5), cleanup)
        .await
        .expect("cleanup receives provider confirmation")
        .unwrap();
    server.shutdown().await;

    assert_eq!(ordinary.state, InternalWorkerOperationState::Rejected);
    assert!(
        ordinary
            .diagnostics
            .iter()
            .any(|d| d.code == "remote_runtime_timeout")
    );
    assert!(
        cleanup_still_waiting,
        "cleanup must not inherit the ordinary timeout"
    );
    assert_eq!(removed.state, InternalWorkerOperationState::Accepted);
    assert!(removed.diagnostics.is_empty());
    let summary = removed.working_directory.unwrap().summary;
    assert_eq!(summary.working_directory_id, "workdir-test");
    assert_eq!(
        summary.status,
        workdir::workspace::WorkingDirectoryStatusKind::NotFound
    );
}
