//! Production Drive adapter fault injection. Only the Workspace transport boundary is substituted;
//! retained fixture effects and original-request inquiries are inspected independently of replies.
//! The HTTP case additionally connects the adapter to reqwest and an isolated loopback router.
use super::*;
use crate::feature::builtin::backend_diagnostic::{Reason, tests::Capture};
use crate::worker::{WorkspaceBinaryResponse, WorkspaceClientError, WorkspaceResponse};
use std::sync::{Condvar, Mutex};
use tracing::instrument::WithSubscriber;

const SECRET: &str = "secret-token /private/drive credential=unsafe";
const ID: &str = "host-request-1";

fn entry() -> DriveEntry {
    DriveEntry {
        entry: DriveEntryRef {
            workspace_id: "workspace".into(),
            node_id: "2".into(),
        },
        parent: Some(DriveEntryRef {
            workspace_id: "workspace".into(),
            node_id: "1".into(),
        }),
        name: SECRET.into(),
        kind: DriveEntryKind::File,
        last_mutation_id: "old-request".into(),
        size: Some(5),
        content_type: Some("text/plain".into()),
        updated_by: SECRET.into(),
        updated_at: SECRET.into(),
        latest_url: SECRET.into(),
    }
}
fn mutation() -> DriveMutationRequest {
    DriveMutationRequest {
        request_id: ID.into(),
        mutation: DriveMutation::UpdateText {
            id: entry().entry,
            expected_mutation_id: "old-request".into(),
            text: SECRET.into(),
            content_type: "text/plain".into(),
        },
    }
}
fn json(value: impl Serialize) -> WorkspaceResponse {
    WorkspaceResponse {
        status: 200,
        body: serde_json::to_string(&value).unwrap(),
    }
}
#[derive(Clone, Copy, Debug)]
enum Fault {
    Communication,
    Unavailable,
    InvalidPath,
    Decode,
    Size,
    Length,
    Identity,
    Metadata,
    Rejected,
    Unknown,
    Unrecognized,
    Incomplete,
    Panic,
    Deadline,
    None,
}
#[derive(Debug, Default)]
struct State {
    mutations: usize,
    saved: Option<String>,
    requests: Vec<WorkspaceRequest>,
    binary: Vec<WorkspaceBinaryRequest>,
    released: bool,
    started: bool,
    finished: bool,
}
#[derive(Debug)]
struct Client {
    fault: Fault,
    receipt: Fault,
    state: Mutex<State>,
    changed: Condvar,
    started: tokio::sync::Notify,
}
impl Client {
    fn new(fault: Fault, receipt: Fault) -> Arc<Self> {
        Arc::new(Self {
            fault,
            receipt,
            state: Mutex::new(State::default()),
            changed: Condvar::new(),
            started: tokio::sync::Notify::new(),
        })
    }
    fn fail(
        &self,
        fault: Fault,
        success: WorkspaceResponse,
    ) -> Result<WorkspaceResponse, WorkspaceClientError> {
        match fault {
            Fault::Communication => Err(WorkspaceClientError::Request(SECRET.into())),
            Fault::Unavailable => Err(WorkspaceClientError::Unavailable(SECRET.into())),
            Fault::InvalidPath => Err(WorkspaceClientError::InvalidPath(SECRET.into())),
            Fault::Panic => panic!("injected adapter task panic"),
            Fault::Decode => Ok(WorkspaceResponse {
                status: 200,
                body: SECRET.into(),
            }),
            Fault::Size => Ok(WorkspaceResponse {
                status: 200,
                body: SECRET.repeat(RESPONSE_LIMIT / SECRET.len() + 1),
            }),
            Fault::Rejected | Fault::Unknown => Ok(WorkspaceResponse {
                status: 409,
                body: serde_json::to_string(&DriveApiError {
                    code: DriveApiErrorCode::Conflict,
                    message: SECRET.into(),
                    classification: if matches!(fault, Fault::Unknown) {
                        DriveFailureClassification::Unknown
                    } else {
                        DriveFailureClassification::NotCommitted
                    },
                })
                .unwrap(),
            }),
            Fault::Unrecognized => Ok(WorkspaceResponse {
                status: 503,
                body: SECRET.into(),
            }),
            _ => Ok(success),
        }
    }
    fn release(&self) {
        self.state.lock().unwrap().released = true;
        self.changed.notify_all();
    }
    fn wait(&self, finished: bool) {
        let (state, timeout) = self
            .changed
            .wait_timeout_while(self.state.lock().unwrap(), Duration::from_secs(5), |s| {
                if finished { !s.finished } else { !s.started }
            })
            .unwrap();
        assert!(!timeout.timed_out());
        assert!(if finished {
            state.finished
        } else {
            state.started
        });
    }
}
struct Release(Arc<Client>);
impl Drop for Release {
    fn drop(&mut self) {
        self.0.release();
    }
}
impl WorkspaceClient for Client {
    fn workspace_id(&self) -> Option<&str> {
        Some("workspace")
    }
    fn kind(&self) -> &str {
        "drive-diagnostic-fixture"
    }
    fn is_available(&self) -> bool {
        true
    }
    fn execute(
        &self,
        request: WorkspaceRequest,
    ) -> Result<WorkspaceResponse, WorkspaceClientError> {
        if request.path.ends_with("/authorization/check") {
            let input: server_api::authorization::OperationAuthorizationRequest =
                serde_json::from_str(request.body.as_deref().unwrap()).unwrap();
            return Ok(WorkspaceResponse {
                status: 200,
                body: serde_json::json!({"scope": input.resource}).to_string(),
            });
        }
        let is_status = request.path.contains("/requests/");
        let fault = if is_status { self.receipt } else { self.fault };
        let mutation = request.method == WorkspaceRequestMethod::Post;
        {
            let mut state = self.state.lock().unwrap();
            if mutation
                && !matches!(
                    fault,
                    Fault::Rejected | Fault::InvalidPath | Fault::Unavailable
                )
            {
                let input: DriveMutationRequest =
                    serde_json::from_str(request.body.as_ref().unwrap()).unwrap();
                assert_eq!(input, super::tests::mutation());
                state.mutations += 1;
                if let DriveMutation::UpdateText { text, .. } = input.mutation {
                    state.saved = Some(text);
                }
            }
            state.requests.push(request);
            state.started = true;
            self.started.notify_one();
            self.changed.notify_all();
            if matches!(fault, Fault::Deadline) {
                let (_state, timeout) = self
                    .changed
                    .wait_timeout_while(state, Duration::from_secs(5), |s| !s.released)
                    .unwrap();
                assert!(
                    !timeout.timed_out(),
                    "transport must be explicitly released"
                );
            }
        }
        let mut metadata = entry();
        if matches!(fault, Fault::Identity) {
            metadata.entry.node_id = "3".into();
        }
        if matches!(fault, Fault::Metadata) {
            metadata.size = Some(DRIVE_FILE_MAX_BYTES + 1);
        }
        let completion = DriveMutationResponse {
            request_id: if matches!(fault, Fault::Identity) {
                "other-request"
            } else {
                ID
            }
            .into(),
            entry: Some(metadata.clone()),
        };
        let value = if is_status {
            json(DriveRequestStatusResponse {
                request_id: if matches!(fault, Fault::Identity) {
                    "other-request"
                } else {
                    ID
                }
                .into(),
                state: if matches!(
                    fault,
                    Fault::None | Fault::Identity | Fault::Metadata | Fault::Incomplete
                ) {
                    DriveRequestState::Committed
                } else {
                    DriveRequestState::Uncommitted
                },
                response: if matches!(fault, Fault::None | Fault::Identity | Fault::Metadata) {
                    Some(completion)
                } else {
                    None
                },
            })
        } else if mutation {
            json(completion)
        } else {
            json(metadata)
        };
        if !is_status {
            self.state.lock().unwrap().finished = true;
        }
        self.changed.notify_all();
        self.fail(fault, value)
    }
    fn execute_binary(
        &self,
        request: WorkspaceBinaryRequest,
    ) -> Result<WorkspaceBinaryResponse, WorkspaceClientError> {
        let upload = request.method == WorkspaceRequestMethod::Put;
        {
            let mut state = self.state.lock().unwrap();
            if upload {
                state.mutations += 1;
                state.saved = Some(SECRET.into());
            }
            state.binary.push(request);
        }
        let success = if upload {
            json(DriveMutationResponse {
                request_id: ID.into(),
                entry: Some(entry()),
            })
        } else {
            WorkspaceResponse {
                status: 200,
                body: if matches!(self.fault, Fault::Length) {
                    "x"
                } else {
                    "12345"
                }
                .into(),
            }
        };
        self.fail(self.fault, success)
            .map(|r| WorkspaceBinaryResponse {
                status: r.status,
                body: r.body.into_bytes(),
            })
    }
}
fn safe(capture: &Capture, reason: Reason, public: &str) {
    let text = capture.text();
    assert!(text.contains(&format!("reason={reason:?}")), "{text}");
    assert!(text.contains("adapter=Drive"), "{text}");
    assert!(text.contains("correlation="), "{text}");
    for secret in [
        SECRET,
        "secret-token",
        "/private/drive",
        "credential=unsafe",
    ] {
        assert!(!text.contains(secret), "internal diagnostic leaked content");
        assert!(!public.contains(secret), "public error leaked content");
    }
}
fn reason(fault: Fault) -> Reason {
    match fault {
        Fault::Communication => Reason::Communication,
        Fault::Unavailable => Reason::TransportUnavailable,
        Fault::InvalidPath => Reason::InvalidPath,
        Fault::Decode => Reason::Decode,
        Fault::Size => Reason::ResponseSize,
        Fault::Length => Reason::ResponseLength,
        Fault::Identity => Reason::Identity,
        Fault::Metadata => Reason::InvalidMetadata,
        Fault::Rejected => Reason::BackendRejected,
        Fault::Unknown => Reason::BackendUnknown,
        Fault::Unrecognized => Reason::UnrecognizedResponse,
        Fault::Panic => Reason::TaskJoin,
        Fault::Deadline => Reason::Deadline,
        Fault::Incomplete => Reason::IncompleteReceipt,
        Fault::None => panic!("no fault"),
    }
}
#[tokio::test]
async fn production_read_failures_retain_public_classification_and_safe_cause() {
    for fault in [
        Fault::Communication,
        Fault::Unavailable,
        Fault::InvalidPath,
        Fault::Decode,
        Fault::Size,
        Fault::Identity,
        Fault::Metadata,
        Fault::Rejected,
        Fault::Unknown,
        Fault::Unrecognized,
        Fault::Panic,
    ] {
        let client = Client::new(fault, Fault::None);
        let backend = DriveBackend::new(client.clone());
        let capture = Capture::default();
        let error = backend
            .metadata(&entry().entry)
            .with_subscriber(capture.dispatch())
            .await
            .unwrap_err();
        match fault {
            Fault::Size => assert!(matches!(error, DriveError::Limit)),
            Fault::Rejected => assert!(matches!(error, DriveError::Conflict)),
            Fault::Unknown => assert!(matches!(error, DriveError::OutcomeUnknown(_))),
            _ => assert!(matches!(error, DriveError::Unavailable)),
        }
        safe(&capture, reason(fault), &error.to_string());
        let state = client.state.lock().unwrap();
        assert_eq!(state.requests.len(), 1);
        assert_eq!(state.mutations, 0);
    }
}
#[tokio::test]
async fn production_mutation_loss_reconciles_only_original_identity_and_never_resends() {
    for fault in [
        Fault::Communication,
        Fault::Decode,
        Fault::Size,
        Fault::Identity,
        Fault::Metadata,
        Fault::Unknown,
        Fault::Unrecognized,
        Fault::Panic,
    ] {
        for receipt in [
            Fault::None,
            Fault::Identity,
            Fault::Metadata,
            Fault::Decode,
            Fault::Communication,
            Fault::Unrecognized,
            Fault::Length,
            Fault::Incomplete,
        ] {
            let client = Client::new(fault, receipt);
            let backend = DriveBackend::new(client.clone());
            let capture = Capture::default();
            let result = backend
                .mutate(mutation())
                .with_subscriber(capture.dispatch())
                .await;
            if matches!(receipt, Fault::None) {
                assert_eq!(result.unwrap().request_id, ID);
            } else {
                assert!(matches!(result, Err(DriveError::OutcomeUnknown(ref id)) if id == ID));
            }
            safe(&capture, reason(fault), "");
            let text = capture.text();
            if matches!(receipt, Fault::Length | Fault::Incomplete) {
                safe(
                    &capture,
                    if matches!(receipt, Fault::Length) {
                        Reason::NotCommittedSnapshot
                    } else {
                        Reason::IncompleteReceipt
                    },
                    "",
                );
            }
            assert!(text.contains("stage=Reconcile"), "{text}");
            let correlations: std::collections::BTreeSet<_> = text
                .lines()
                .filter_map(|l| l.split("correlation=").nth(1))
                .filter_map(|l| l.split(';').next())
                .collect();
            assert_eq!(
                correlations.len(),
                1,
                "original loss and inquiry must correlate"
            );
            assert!(
                !text.contains(ID),
                "request identity must be opaque in logs"
            );
            let state = client.state.lock().unwrap();
            assert_eq!(state.mutations, 1);
            assert_eq!(state.saved.as_deref(), Some(SECRET));
            assert_eq!(state.requests.len(), 2);
            assert_eq!(state.requests[1].method, WorkspaceRequestMethod::Get);
            assert!(state.requests[1].path.ends_with(&format!("/requests/{ID}")));
        }
    }
}
#[tokio::test]
async fn production_rejection_has_no_effect_or_status_inquiry() {
    let client = Client::new(Fault::Rejected, Fault::None);
    let backend = DriveBackend::new(client.clone());
    let capture = Capture::default();
    let error = backend
        .mutate(mutation())
        .with_subscriber(capture.dispatch())
        .await
        .unwrap_err();
    assert!(matches!(error, DriveError::Conflict));
    safe(&capture, Reason::BackendRejected, &error.to_string());
    let state = client.state.lock().unwrap();
    assert_eq!(state.mutations, 0);
    assert_eq!(state.requests.len(), 1);
}
#[tokio::test]
async fn production_binary_length_transport_and_upload_loss_are_diagnosed() {
    for fault in [
        Fault::Length,
        Fault::Communication,
        Fault::Unavailable,
        Fault::Panic,
    ] {
        let client = Client::new(fault, Fault::None);
        let backend = DriveBackend::new(client.clone());
        let capture = Capture::default();
        let error = backend
            .bytes(&entry(), 5)
            .with_subscriber(capture.dispatch())
            .await
            .unwrap_err();
        assert!(matches!(error, DriveError::Unavailable));
        safe(&capture, reason(fault), &error.to_string());
        assert_eq!(client.state.lock().unwrap().binary.len(), 1);
    }
    for fault in [Fault::Decode, Fault::Communication, Fault::Size] {
        let client = Client::new(fault, Fault::Unrecognized);
        let backend = DriveBackend::new(client.clone());
        let capture = Capture::default();
        let query = DriveUploadQuery {
            operation: DriveUploadOperation::Update,
            request_id: ID.into(),
            entry_workspace_id: "workspace".into(),
            parent_id: None,
            name: None,
            id: Some("2".into()),
            expected_mutation_id: Some("old-request".into()),
            content_type: "text/plain".into(),
            size: 5,
            sha256: "hash".into(),
        };
        let error = backend
            .upload(query, b"12345".to_vec())
            .with_subscriber(capture.dispatch())
            .await
            .unwrap_err();
        assert!(matches!(error, DriveError::OutcomeUnknown(ref id) if id == ID));
        safe(&capture, reason(fault), &error.to_string());
        let state = client.state.lock().unwrap();
        assert_eq!(state.mutations, 1);
        assert_eq!(state.binary.len(), 1);
        assert_eq!(state.requests.len(), 1);
        assert!(state.requests[0].path.ends_with(&format!("/requests/{ID}")));
    }
}
#[tokio::test]
async fn production_deadline_does_not_cancel_effect_or_infer_uncommitted_from_snapshot() {
    for mutation_call in [false, true] {
        let client = Client::new(Fault::Deadline, Fault::Unrecognized);
        let release = Release(client.clone());
        let mut backend = DriveBackend::new(client.clone());
        backend.deadline = Duration::from_secs(1);
        let capture = Capture::default();
        let request = async {
            if mutation_call {
                backend.mutate(mutation()).await.map(|_| ())
            } else {
                backend.metadata(&entry().entry).await.map(|_| ())
            }
        }
        .with_subscriber(capture.dispatch());
        tokio::pin!(request);
        tokio::select! {
            biased;
            _ = client.started.notified() => {},
            _ = &mut request => panic!("request must start before timeout"),
        }
        {
            let state = client.state.lock().unwrap();
            assert_eq!(state.mutations, usize::from(mutation_call));
            assert!(!state.released && !state.finished);
        }
        let result = request.await;
        let error = result.unwrap_err();
        assert!(if mutation_call {
            matches!(error, DriveError::OutcomeUnknown(ref id) if id == ID)
        } else {
            matches!(error, DriveError::Unavailable)
        });
        safe(&capture, Reason::Deadline, &error.to_string());
        drop(release);
        let waiting = client.clone();
        tokio::task::spawn_blocking(move || waiting.wait(true))
            .await
            .unwrap();
        let state = client.state.lock().unwrap();
        assert_eq!(state.mutations, usize::from(mutation_call));
        assert_eq!(state.saved.is_some(), mutation_call);
        assert_eq!(state.requests.len(), if mutation_call { 2 } else { 1 });
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn http_reply_loss_keeps_original_receipt_and_retained_effect_without_reexecution() {
    use axum::{
        Router,
        extract::State as HttpState,
        routing::{get, post},
    };
    #[derive(Default)]
    struct Stored {
        updates: usize,
        content: String,
        ids: Vec<String>,
    }
    struct Server(tokio::task::JoinHandle<()>);
    impl Drop for Server {
        fn drop(&mut self) {
            self.0.abort();
        }
    }
    let stored = Arc::new(Mutex::new(Stored::default()));
    let router = Router::new()
        .route("/api/w/workspace/authorization/check", post(|body: axum::body::Bytes| async move { let input: server_api::authorization::OperationAuthorizationRequest = serde_json::from_slice(&body).unwrap(); axum::Json(serde_json::json!({"scope":input.resource})) }))
        .route("/api/w/workspace/drive/metadata", get(|| async { SECRET }))
        .route(
            "/api/w/workspace/drive/mutate",
            post(
                |HttpState(state): HttpState<Arc<Mutex<Stored>>>,
                 body: axum::body::Bytes| async move {
                    let request: DriveMutationRequest = serde_json::from_slice(&body).unwrap();
                    let mut state = state.lock().unwrap();
                    state.ids.push(request.request_id);
                    if let DriveMutation::UpdateText {
                        text,
                        expected_mutation_id,
                        ..
                    } = request.mutation
                    {
                        assert_eq!(expected_mutation_id, "old-request");
                        state.content = text;
                        state.updates += 1;
                    } else {
                        panic!("unexpected mutation");
                    }
                    // The effect is retained; only the success response is malformed.
                    SECRET
                },
            ),
        )
        .route(
            "/api/w/workspace/drive/requests/{id}",
            get(
                |HttpState(state): HttpState<Arc<Mutex<Stored>>>,
                 axum::extract::Path(id): axum::extract::Path<String>| async move {
                    let state = state.lock().unwrap();
                    assert_eq!(state.ids, [id.clone()]);
                    axum::Json(DriveRequestStatusResponse {
                        request_id: id.clone(),
                        state: DriveRequestState::Committed,
                        response: Some(DriveMutationResponse {
                            request_id: id,
                            entry: Some(entry()),
                        }),
                    })
                },
            ),
        )
        .with_state(stored.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = Server(tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    }));
    let client = Arc::new(crate::worker::TestWorkspaceHttpClient::new(
        "workspace",
        format!("http://{address}"),
    ));
    let backend = DriveBackend::new(client);
    let capture = Capture::default();
    let error = backend
        .metadata(&entry().entry)
        .with_subscriber(capture.dispatch())
        .await
        .unwrap_err();
    assert!(matches!(error, DriveError::Unavailable), "{error:?}");
    safe(&capture, Reason::Decode, &error.to_string());
    let capture = Capture::default();
    let response = backend
        .mutate(mutation())
        .with_subscriber(capture.dispatch())
        .await
        .unwrap();
    assert_eq!(response.request_id, ID);
    safe(&capture, Reason::Decode, "");
    assert!(capture.text().contains("reason=Reconciled"));
    let state = stored.lock().unwrap();
    assert_eq!(state.updates, 1);
    assert_eq!(state.content, SECRET);
    assert_eq!(state.ids, [ID]);
    drop(state);
    drop(server);
}
