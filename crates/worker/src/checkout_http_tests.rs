//! Real provider HTTP -> RemoteWorkdirSession -> router -> native WIP Client.
//! This deliberately does not exercise the production Backend grant network.
use super::*;
use axum::{
    Json as AxumJson, Router,
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response as AxumResponse},
    routing::{delete, post},
};
use std::time::Duration;
use tempfile::TempDir;
use workdir::http::{
    OpenWorkdirSessionRequest, OpenWorkdirSessionResponse, RemoteWorkdirSession, WorkdirSessionId,
    WorkdirSessionOperation, WorkdirSessionOperationRequest, WorkdirTransportError,
    WorkdirTransportErrorCode,
};
use workdir::{
    CheckoutOperation, CheckoutRequest, CheckoutSearchOperation, LocalWorkdirSession,
    WorkdirAttachmentAlias, WorkdirError, WorkdirPath, WorkdirSession, WorkdirSessionCapabilities,
    WorkdirSessionHandle, WorkdirSessionRouter, dispatch_workdir_session_operation,
};

const TOKEN: &str = "checkout-http-test-private-bearer";
const SESSION: &str = "http-test-session";

#[derive(Clone, Copy, Debug)]
enum WriteResponseFault {
    Malformed,
    LostBody,
}

struct ProviderState {
    session: WorkdirSessionHandle,
    requests: Mutex<Vec<(String, String)>>,
    operations: Mutex<Vec<WorkdirSessionOperation>>,
    write_fault: Mutex<Option<WriteResponseFault>>,
}

impl ProviderState {
    fn authorize(&self, headers: &HeaderMap, method: &str, uri: &Uri) -> Result<(), AxumResponse> {
        self.requests
            .lock()
            .unwrap()
            .push((method.into(), uri.path().into()));
        let expected = format!("Bearer {TOKEN}");
        if headers.get("authorization").and_then(|h| h.to_str().ok()) != Some(expected.as_str()) {
            return Err((
                StatusCode::UNAUTHORIZED,
                AxumJson(WorkdirTransportError {
                    code: WorkdirTransportErrorCode::Denied,
                    message: "provider authorization denied".into(),
                }),
            )
                .into_response());
        }
        Ok(())
    }

    fn execute_calls(&self) -> usize {
        self.operations
            .lock()
            .unwrap()
            .iter()
            .filter(|op| matches!(op, WorkdirSessionOperation::CheckoutExecute(_)))
            .count()
    }

    fn write_calls(&self) -> usize {
        self.operations
            .lock()
            .unwrap()
            .iter()
            .filter(|op| {
                matches!(
                    op,
                    WorkdirSessionOperation::CheckoutExecute(CheckoutRequest {
                        operation: CheckoutOperation::Write { .. },
                        ..
                    })
                )
            })
            .count()
    }
}

fn provider_error(error: WorkdirError) -> AxumResponse {
    let error = WorkdirTransportError::from_workdir_error(&error);
    (
        StatusCode::from_u16(error.code.http_status()).unwrap(),
        AxumJson(error),
    )
        .into_response()
}

async fn open_session(
    State(state): State<Arc<ProviderState>>,
    Path(id): Path<String>,
    uri: Uri,
    headers: HeaderMap,
    AxumJson(_request): AxumJson<OpenWorkdirSessionRequest>,
) -> AxumResponse {
    if let Err(response) = state.authorize(&headers, "POST", &uri) {
        return response;
    }
    if id != state.session.workdir().id().as_str() {
        return provider_error(WorkdirError::NotFound("unknown workdir".into()));
    }
    AxumJson(OpenWorkdirSessionResponse {
        session_id: WorkdirSessionId::new(SESSION).unwrap(),
        workdir_id: state.session.workdir().id().clone(),
        capabilities: state.session.capabilities(),
    })
    .into_response()
}

async fn operate(
    State(state): State<Arc<ProviderState>>,
    Path(id): Path<String>,
    uri: Uri,
    headers: HeaderMap,
    AxumJson(request): AxumJson<WorkdirSessionOperationRequest>,
) -> AxumResponse {
    if let Err(response) = state.authorize(&headers, "POST", &uri) {
        return response;
    }
    if id != SESSION {
        return provider_error(WorkdirError::Unavailable("unknown session".into()));
    }
    let is_write = matches!(
        &request.operation,
        WorkdirSessionOperation::CheckoutExecute(CheckoutRequest {
            operation: CheckoutOperation::Write { .. },
            ..
        })
    );
    state
        .operations
        .lock()
        .unwrap()
        .push(request.operation.clone());
    let result =
        dispatch_workdir_session_operation(state.session.as_ref(), request.operation).await;
    match result {
        // Inject only AFTER the real LocalWorkdirSession successfully committed.
        Ok(result) => {
            let fault = if is_write {
                state.write_fault.lock().unwrap().take()
            } else {
                None
            };
            match fault {
                Some(WriteResponseFault::Malformed) => (
                    StatusCode::OK,
                    [("content-type", "application/json")],
                    "{broken-response",
                )
                    .into_response(),
                Some(WriteResponseFault::LostBody) => {
                    let body = Body::from_stream(futures::stream::once(async {
                        Err::<Vec<u8>, _>(std::io::Error::other("response lost after commit"))
                    }));
                    AxumResponse::builder()
                        .status(StatusCode::OK)
                        .header("content-type", "application/json")
                        .body(body)
                        .unwrap()
                }
                None => AxumJson(result).into_response(),
            }
        }
        Err(error) => provider_error(error),
    }
}

async fn close_session(
    State(state): State<Arc<ProviderState>>,
    Path(id): Path<String>,
    uri: Uri,
    headers: HeaderMap,
) -> AxumResponse {
    if let Err(response) = state.authorize(&headers, "DELETE", &uri) {
        return response;
    }
    if id != SESSION {
        return provider_error(WorkdirError::Unavailable("unknown session".into()));
    }
    match state.session.close().await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => provider_error(error),
    }
}

struct HttpFixture {
    dir: TempDir,
    state: Arc<ProviderState>,
    base_url: reqwest::Url,
    server: tokio::task::JoinHandle<()>,
}

impl Drop for HttpFixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl HttpFixture {
    async fn new(capabilities: WorkdirSessionCapabilities) -> Self {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("src/deep")).unwrap();
        std::fs::write(
            dir.path().join("src/deep/a.txt"),
            "first\nneedle needle\nlast\n",
        )
        .unwrap();
        let session = Arc::new(LocalWorkdirSession::materialized(
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
            manifest::SharedScope::new(manifest::Scope::writable(dir.path()).unwrap()),
            capabilities,
        ));
        let state = Arc::new(ProviderState {
            session,
            requests: Mutex::new(Vec::new()),
            operations: Mutex::new(Vec::new()),
            write_fault: Mutex::new(None),
        });
        let app = Router::new()
            .route(
                "/provider/v1/working-directories/{id}/sessions",
                post(open_session),
            )
            .route(
                "/provider/v1/workdir-sessions/{id}/operations",
                post(operate),
            )
            .route("/provider/v1/workdir-sessions/{id}", delete(close_session))
            .with_state(state.clone());
        // Binding before spawn makes readiness deterministic; no sleeps or polling.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}/provider/", listener.local_addr().unwrap())
            .parse()
            .unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            dir,
            state,
            base_url,
            server,
        }
    }

    async fn open(&self, token: &str) -> Result<RemoteWorkdirSession, WorkdirError> {
        RemoteWorkdirSession::open(
            reqwest::Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(5))
                .build()
                .unwrap(),
            self.base_url.clone(),
            token.to_owned(),
            self.state.session.workdir().id().clone(),
            OpenWorkdirSessionRequest::default(),
        )
        .await
    }

    fn public_output(&self, text: &str) {
        assert!(!text.contains(TOKEN), "bearer token leaked");
        assert!(
            !text.contains(self.dir.path().to_str().unwrap()),
            "host path leaked"
        );
        assert!(
            !text.contains("/provider/"),
            "transport prefix leaked into Worldspace"
        );
    }

    fn assert_endpoints(&self) {
        let open = format!(
            "/provider/v1/working-directories/{}/sessions",
            self.state.session.workdir().id()
        );
        let operations = format!("/provider/v1/workdir-sessions/{SESSION}/operations");
        let close = format!("/provider/v1/workdir-sessions/{SESSION}");
        let requests = self.state.requests.lock().unwrap();
        assert!(requests.contains(&("POST".into(), open.clone())));
        assert!(requests.contains(&("POST".into(), operations.clone())));
        assert_eq!(
            requests
                .iter()
                .filter(|(method, path)| method == "DELETE" && path == &close)
                .count(),
            1
        );
        assert!(requests.iter().all(|(method, path)| (method == "POST"
            && (path == &open || path == &operations))
            || (method == "DELETE" && path == &close)));
    }
}

fn native_runtime(remote: Arc<RemoteWorkdirSession>) -> (WipRuntime, Arc<WorkdirSessionRouter>) {
    let router = Arc::new(WorkdirSessionRouter::new());
    router
        .attach(WorkdirAttachmentAlias::new("main").unwrap(), remote)
        .unwrap();
    let mut registry = WipMountRegistry::new();
    crate::checkout::mount_checkouts(&mut registry, router.clone(), tools::Tracker::new(), None)
        .unwrap();
    let runtime = WipRuntime::new(
        WipHost::new(registry),
        SecurityContext::new("http-checkout-worker"),
        0,
    )
    .unwrap();
    (runtime, router)
}

async fn observe_interface(runtime: &WipRuntime, path: &str) -> wip_protocol::InterfaceReference {
    runtime.tree(path.into(), 0, true).await.unwrap();
    // Select from the real Client's Known Space, not a direct handler invocation.
    let interface = {
        let state = runtime.state.lock().unwrap();
        state
            .client
            .known_space(&state.session)
            .unwrap()
            .into_iter()
            .find(|o| o.path == path)
            .unwrap()
            .object
            .unwrap()
            .interfaces[0]
            .clone()
    };
    runtime.inspect(path.into(), true).await.unwrap();
    interface
}

async fn native_call(runtime: &WipRuntime, path: &str, operation: &str, args: Json) -> Value {
    let interface = observe_interface(runtime, path).await;
    runtime
        .call(
            path.into(),
            interface,
            operation.into(),
            args,
            Default::default(),
        )
        .await
        .unwrap();
    let state = runtime.state.lock().unwrap();
    match &state
        .client
        .call_history(&state.session)
        .unwrap()
        .back()
        .unwrap()
        .outcome
    {
        CallOutcome::Success(response) => response.result.clone(),
        outcome => panic!("expected native Client success, got {outcome:?}"),
    }
}

fn field<'a>(value: &'a Value, name: &str) -> &'a Value {
    let Value::Record(fields) = value else {
        panic!("expected typed Record")
    };
    fields.get(name).unwrap()
}

fn first_link(value: &Value) -> &str {
    let Value::List(items) = field(value, "items") else {
        panic!("expected typed link List")
    };
    assert_eq!(items.len(), 1);
    let Value::String(path) = field(&items[0], "path") else {
        panic!("expected typed link path")
    };
    path
}

async fn watchdog(future: impl std::future::Future<Output = ()>) {
    tokio::time::timeout(Duration::from_secs(20), future)
        .await
        .expect("HTTP checkout test watchdog");
}

#[tokio::test]
async fn checkout_http_native_search_links_read_edit_write_create_and_close() {
    watchdog(async {
        let fixture = HttpFixture::new(WorkdirSessionCapabilities::READ_WRITE).await;
        let remote = Arc::new(fixture.open(TOKEN).await.unwrap());
        let (runtime, router) = native_runtime(remote);
        let glob = native_call(
            &runtime,
            "/checkouts/main",
            "glob",
            json!({"pattern":"**/*.txt"}),
        )
        .await;
        let file = first_link(&glob).to_owned();
        assert_eq!(file, "/checkouts/main/src/deep/a.txt");
        let grep = native_call(
            &runtime,
            "/checkouts/main/src",
            "grep",
            json!({
                "pattern":"needle", "output_mode":"files_with_matches"
            }),
        )
        .await;
        assert_eq!(first_link(&grep), file);
        let read = native_call(
            &runtime,
            first_link(&grep),
            "read",
            json!({"offset":1,"limit":1}),
        )
        .await;
        let Value::String(content) = field(&read, "content") else {
            panic!("expected line Read text")
        };
        assert_eq!(content, "     2\tneedle needle\n");
        let edit = native_call(
            &runtime,
            &file,
            "edit",
            json!({
                "old_string":"needle", "new_string":"done", "replace_all":true
            }),
        )
        .await;
        assert_eq!(
            std::fs::read_to_string(fixture.dir.path().join("src/deep/a.txt")).unwrap(),
            "first\ndone done\nlast\n"
        );
        let write = native_call(&runtime, &file, "write", json!({"content":"saved\n"})).await;
        assert_eq!(
            std::fs::read_to_string(fixture.dir.path().join("src/deep/a.txt")).unwrap(),
            "saved\n"
        );
        let create = native_call(
            &runtime,
            "/checkouts/main/src",
            "create_file",
            json!({
                "path":"new/deeper.txt", "content":"created\n"
            }),
        )
        .await;
        assert_eq!(
            std::fs::read_to_string(fixture.dir.path().join("src/new/deeper.txt")).unwrap(),
            "created\n"
        );
        let created = native_call(
            &runtime,
            "/checkouts/main/src/new/deeper.txt",
            "read",
            json!({}),
        )
        .await;
        for value in [&glob, &grep, &read, &edit, &write, &create, &created] {
            fixture.public_output(&serde_json::to_string(&wip_to_json(value).unwrap()).unwrap());
        }
        let operations = fixture.state.operations.lock().unwrap();
        assert!(
            operations
                .iter()
                .any(|op| matches!(op, WorkdirSessionOperation::CheckoutSearch(r)
            if matches!(r.operation, CheckoutSearchOperation::Glob(_))))
        );
        assert!(
            operations
                .iter()
                .any(|op| matches!(op, WorkdirSessionOperation::CheckoutSearch(r)
            if matches!(r.operation, CheckoutSearchOperation::Grep(_))))
        );
        assert!(operations.iter().any(
            |op| matches!(op, WorkdirSessionOperation::CheckoutExecute(r)
            if matches!(r.operation, CheckoutOperation::Edit { .. }))
        ));
        assert!(operations.iter().any(
            |op| matches!(op, WorkdirSessionOperation::CheckoutExecute(r)
            if matches!(r.operation, CheckoutOperation::Create { .. }))
        ));
        drop(operations);
        assert_eq!(fixture.state.write_calls(), 1);
        router
            .detach(&WorkdirAttachmentAlias::new("main").unwrap())
            .await
            .unwrap();
        fixture.assert_endpoints();
    })
    .await;
}

#[tokio::test]
async fn checkout_http_wrong_bearer_and_readonly_caps_fail_closed() {
    watchdog(async {
        let fixture = HttpFixture::new(WorkdirSessionCapabilities::READ_ONLY).await;
        let error = fixture.open("wrong-bearer").await.unwrap_err();
        assert!(matches!(error, WorkdirError::Denied(_)));
        fixture.public_output(&error.to_string());
        assert!(fixture.state.operations.lock().unwrap().is_empty());
        let remote = Arc::new(fixture.open(TOKEN).await.unwrap());
        assert_eq!(remote.capabilities(), WorkdirSessionCapabilities::READ_ONLY);
        let (runtime, router) = native_runtime(remote.clone());
        let file = "/checkouts/main/src/deep/a.txt";
        native_call(&runtime, file, "read", json!({})).await;
        let interface = observe_interface(&runtime, file).await;
        {
            let state = runtime.state.lock().unwrap();
            let observed = state.client.interface(&state.session, &interface).unwrap();
            let descriptor = observed.descriptor.as_ref().unwrap();
            assert_eq!(
                descriptor
                    .operations
                    .iter()
                    .map(|op| op.name.as_str())
                    .collect::<Vec<_>>(),
                ["read"]
            );
        }
        let directory_interface = observe_interface(&runtime, "/checkouts/main").await;
        let before = fixture.state.execute_calls();
        for (path, interface, operation, args) in [
            (
                file,
                interface.clone(),
                "write",
                json!({"content":"forbidden"}),
            ),
            (
                file,
                interface,
                "edit",
                json!({"old_string":"needle", "new_string":"forbidden"}),
            ),
            (
                "/checkouts/main",
                directory_interface,
                "create_file",
                json!({"path":"forbidden.txt","content":"bad"}),
            ),
        ] {
            let error = runtime
                .call(
                    path.into(),
                    interface,
                    operation.into(),
                    args,
                    Default::default(),
                )
                .await
                .unwrap_err();
            fixture.public_output(&error.to_string());
        }
        // Preflight may observe objects, but no rejected mutation reaches execution.
        assert_eq!(fixture.state.execute_calls(), before);
        // A caller bypassing WIP publication still cannot mutate at the provider.
        let parent = remote.checkout_observe(WorkdirPath::root()).await.unwrap();
        let error = remote
            .checkout_execute(CheckoutRequest {
                target: parent.path,
                validator: parent.validator,
                operation: CheckoutOperation::Create {
                    path: WorkdirPath::new("forbidden.txt").unwrap(),
                    content: b"bad".to_vec(),
                },
            })
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            WorkdirError::Unsupported(_)
                | WorkdirError::UnsupportedOperation(_)
                | WorkdirError::ReadOnly(_)
                | WorkdirError::Denied(_)
        ));
        fixture.public_output(&error.to_string());
        assert_eq!(
            std::fs::read_to_string(fixture.dir.path().join("src/deep/a.txt")).unwrap(),
            "first\nneedle needle\nlast\n"
        );
        assert!(!fixture.dir.path().join("forbidden.txt").exists());
        router
            .detach(&WorkdirAttachmentAlias::new("main").unwrap())
            .await
            .unwrap();
        fixture.assert_endpoints();
    })
    .await;
}

async fn committed_write_unknown(fault: WriteResponseFault) {
    let fixture = HttpFixture::new(WorkdirSessionCapabilities::READ_WRITE).await;
    let remote = Arc::new(fixture.open(TOKEN).await.unwrap());
    let (runtime, router) = native_runtime(remote);
    let file = "/checkouts/main/src/deep/a.txt";
    native_call(&runtime, file, "read", json!({})).await;
    let interface = observe_interface(&runtime, file).await;
    *fixture.state.write_fault.lock().unwrap() = Some(fault);
    let error = runtime
        .call(
            file.into(),
            interface,
            "write".into(),
            json!({"content":"committed\n"}),
            Default::default(),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("outcome unknown"));
    assert!(error.to_string().contains("do not retry automatically"));
    fixture.public_output(&error.to_string());
    assert_eq!(
        runtime.audit().last().unwrap().outcome,
        WipAuditOutcome::OutcomeUnknown
    );
    {
        let state = runtime.state.lock().unwrap();
        assert!(matches!(
            state
                .client
                .call_history(&state.session)
                .unwrap()
                .back()
                .unwrap()
                .outcome,
            CallOutcome::Unknown { .. }
        ));
    }
    assert_eq!(
        std::fs::read_to_string(fixture.dir.path().join("src/deep/a.txt")).unwrap(),
        "committed\n"
    );
    assert_eq!(
        fixture.state.write_calls(),
        1,
        "committed Write must not be automatically retried"
    );
    // Explicit inspection of the effect is safe; it must not replay the mutation.
    let read = native_call(&runtime, file, "read", json!({})).await;
    fixture.public_output(&serde_json::to_string(&wip_to_json(&read).unwrap()).unwrap());
    let Value::String(content) = field(&read, "content") else {
        panic!("expected Read text")
    };
    assert!(content.contains("committed"));
    assert_eq!(fixture.state.write_calls(), 1);
    router
        .detach(&WorkdirAttachmentAlias::new("main").unwrap())
        .await
        .unwrap();
    fixture.assert_endpoints();
}

#[tokio::test]
async fn checkout_http_malformed_response_after_write_is_unknown_without_retry() {
    watchdog(committed_write_unknown(WriteResponseFault::Malformed)).await;
}

#[tokio::test]
async fn checkout_http_lost_response_after_write_is_unknown_without_retry() {
    watchdog(committed_write_unknown(WriteResponseFault::LostBody)).await;
}
