//! Real Local/scoped/read-only/router sessions across loopback HTTP. Only the
//! HTTP authentication boundary is a fixture; coordinate and scope checks are
//! production implementations. Runs in the normal http-client library suite.
#![cfg(all(feature = "http-client", target_os = "linux"))]

use crate::http::*;
use crate::*;
use axum::{
    Json, Router,
    extract::State,
    routing::{delete, post},
};
use std::sync::Arc;

struct Server(tokio::task::JoinHandle<()>);
impl Drop for Server {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn open(State(session): State<WorkdirSessionHandle>) -> Json<OpenWorkdirSessionResponse> {
    Json(OpenWorkdirSessionResponse {
        session_id: WorkdirSessionId::new("environment-session").unwrap(),
        workdir_id: session.workdir().id().clone(),
        capabilities: session.capabilities(),
        environment: session.environment(),
    })
}
async fn operation(
    State(session): State<WorkdirSessionHandle>,
    Json(request): Json<WorkdirSessionOperationRequest>,
) -> Result<
    Json<WorkdirSessionOperationResult>,
    (axum::http::StatusCode, Json<WorkdirTransportError>),
> {
    dispatch_workdir_session_operation(session.as_ref(), request.operation)
        .await
        .map(Json)
        .map_err(|error| {
            (
                axum::http::StatusCode::FORBIDDEN,
                Json(WorkdirTransportError::from_workdir_error(&error)),
            )
        })
}

#[tokio::test]
async fn remote_environment_matches_scoped_readonly_operations_without_rebasing_twice() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("nested")).unwrap();
    std::fs::write(root.path().join("nested/file.txt"), "nested marker").unwrap();
    std::fs::write(root.path().join("file.txt"), "root decoy").unwrap();
    let broker = WorkdirToolBroker::new(Arc::new(LocalWorkdirSession::new(
        manifest::Scope::writable(root.path()).unwrap(),
        root.path().to_path_buf(),
    )));
    let lease = broker
        .scope(WorkdirToolScope {
            cwd: WorkdirPath::new("nested").unwrap(),
            rules: vec![WorkdirToolScopeRule {
                target: WorkdirPath::new("nested").unwrap(),
                permission: WorkdirToolScopePermission::Write,
                recursive: true,
                symlink_policy: Default::default(),
            }],
            command: true,
        })
        .await
        .unwrap();
    let session: WorkdirSessionHandle = Arc::new(ReadOnlyWorkdirSession::new(lease.tool_session()));
    let expected = session.environment();
    let app = Router::new()
        .route("/v1/working-directories/{id}/sessions", post(open))
        .route("/v1/workdir-sessions/{id}/operations", post(operation))
        .route(
            "/v1/workdir-sessions/{id}",
            delete(|| async { axum::http::StatusCode::NO_CONTENT }),
        )
        .with_state(session.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let _server = Server(tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    }));
    let remote: WorkdirSessionHandle = Arc::new(
        RemoteWorkdirSession::open(
            reqwest::Client::builder().no_proxy().build().unwrap(),
            format!("http://{address}").parse().unwrap(),
            "fixture",
            session.workdir().id().clone(),
            OpenWorkdirSessionRequest::default(),
        )
        .await
        .unwrap(),
    );
    let router = WorkdirSessionRouter::new();
    router
        .attach(WorkdirAttachmentAlias::new("remote").unwrap(), remote)
        .unwrap();
    let selected = router.resolve(Some("remote")).unwrap().session;
    assert_eq!(selected.environment(), expected);
    assert_eq!(expected.cwd, WorkdirPath::new("nested").unwrap());
    assert!(
        !expected
            .capabilities
            .supports(WorkdirSessionCapability::Command)
    );
    assert!(
        expected
            .scope
            .unwrap()
            .iter()
            .all(|rule| rule.permission == WorkdirToolScopePermission::Read)
    );
    let found = selected
        .glob(GlobRequest {
            path: WorkdirPath::root(),
            pattern: "*.txt".into(),
            limit: 10,
        })
        .await
        .unwrap();
    assert_eq!(found.paths, vec![WorkdirPath::new("file.txt").unwrap()]);
    let path = found.paths[0].clone();
    let read = selected
        .read(ReadRequest {
            path: path.clone(),
            offset: 0,
            limit: 10,
            max_bytes: 100,
        })
        .await
        .unwrap();
    assert_eq!(read.path, path);
    assert_eq!(read.bytes, b"nested marker");
    assert!(
        selected
            .write(WriteRequest {
                path,
                content: b"denied".to_vec(),
                expected_hash: Some(read.content_hash)
            })
            .await
            .is_err()
    );
    assert!(
        selected
            .read(ReadRequest {
                path: WorkdirPath::new_scoped(root.path().join("file.txt").to_str().unwrap())
                    .unwrap(),
                offset: 0,
                limit: 10,
                max_bytes: 100
            })
            .await
            .is_err()
    );
    assert!(
        selected
            .start_command(CommandRequest {
                command: "touch escaped".into(),
                cwd: WorkdirPath::root(),
                timeout_secs: 5,
                transport_byte_budget: 100,
                tool_call_id: None
            })
            .await
            .is_err()
    );
    assert!(!root.path().join("nested/escaped").exists());
    router.close_all().await.unwrap();
    lease.close().await.unwrap();
    broker.close().await.unwrap();
}
