//! Production WorkspaceConfigBackend with only WorkspaceClient substituted. The transport fixture
//! applies effects before injected reply loss, retains its state for rereading, and gates blocking
//! completion explicitly. These ordinary worker lib tests do not claim Backend transaction coverage.
use super::*;
use crate::feature::builtin::backend_diagnostic::tests::Capture;
use crate::worker::WorkspaceResponse;
use std::sync::{Condvar, Mutex};
use tracing::instrument::WithSubscriber;

fn operation_body(mutation: bool) -> serde_json::Value {
    if mutation {
        serde_json::json!({"connection_id":"connection-1", "validator":"v", "request":{"base_digest":"d", "entrypoints":["main.dcdl"], "changes":[{"kind":"update", "path":"main.dcdl", "expected_digest":"d", "content":UNSAFE}]}})
    } else {
        serde_json::json!({"connection_id":"connection-1", "validator":"v", "path":"main.dcdl"})
    }
}

const UNSAFE: &str = "secret-token /private/config credential=unsafe";

#[derive(Clone, Copy, Debug)]
enum Fault {
    Unavailable,
    InvalidPath,
    Communication,
    Decode,
    Size,
    Rejected(u16),
    Unknown,
    Unrecognized,
    Identity,
    Panic,
    Deadline,
}

#[derive(Debug, Default)]
struct Effects {
    requests: usize,
    mutations: usize,
    saved: Option<String>,
    completed: bool,
    released: bool,
}

#[derive(Debug)]
struct Client {
    fault: Fault,
    state: Mutex<Effects>,
    changed: Condvar,
    started: tokio::sync::Notify,
}
impl Client {
    fn new(fault: Fault) -> Arc<Self> {
        Arc::new(Self {
            fault,
            state: Mutex::new(Effects::default()),
            changed: Condvar::new(),
            started: tokio::sync::Notify::new(),
        })
    }
    fn finish(&self) {
        self.state.lock().unwrap().completed = true;
        self.changed.notify_all();
    }
    fn release(&self) {
        self.state.lock().unwrap().released = true;
        self.changed.notify_all();
    }
    fn wait_completed(&self) {
        let (state, timeout) = self
            .changed
            .wait_timeout_while(self.state.lock().unwrap(), Duration::from_secs(5), |s| {
                !s.completed
            })
            .unwrap();
        assert!(
            !timeout.timed_out() && state.completed,
            "blocking request did not finish"
        );
    }
}
struct Release(Arc<Client>);
impl Drop for Release {
    fn drop(&mut self) {
        self.0.release();
    }
}

fn attachment() -> WorkspaceConfigAttachment {
    WorkspaceConfigAttachment {
        workspace_id: "workspace".into(),
        connection_id: "connection-1".into(),
        alias: ATTACHMENT_ALIAS.into(),
        content_path: CONTENT_ROOT.into(),
        working_directory_id: "grant".into(),
        access: WorkspaceConfigAccess::ReadWrite,
        name: UNSAFE.into(),
        purpose: UNSAFE.into(),
        already_attached: false,
    }
}

impl WorkspaceClient for Client {
    fn workspace_id(&self) -> Option<&str> {
        Some("workspace")
    }
    fn kind(&self) -> &str {
        "diagnostic-test-boundary"
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
        {
            let mut state = self.state.lock().unwrap();
            state.requests += 1;
            if request.method == WorkspaceRequestMethod::Post
                && !request.path.ends_with("/read")
                && !matches!(
                    self.fault,
                    Fault::Unavailable | Fault::InvalidPath | Fault::Rejected(_)
                )
            {
                state.mutations += 1;
                state.saved = request.body.clone();
            }
            self.started.notify_one();
            if matches!(self.fault, Fault::Deadline) {
                let (_state, timeout) = self
                    .changed
                    .wait_timeout_while(state, Duration::from_secs(5), |s| !s.released)
                    .unwrap();
                assert!(!timeout.timed_out(), "test must release blocked transport");
            }
        }
        self.finish();
        match self.fault {
            Fault::Unavailable => Err(WorkspaceClientError::Unavailable(UNSAFE.into())),
            Fault::InvalidPath => Err(WorkspaceClientError::InvalidPath(UNSAFE.into())),
            Fault::Communication => Err(WorkspaceClientError::Request(UNSAFE.into())),
            Fault::Panic => panic!("injected blocking task failure"),
            Fault::Decode => Ok(WorkspaceResponse {
                status: 200,
                body: UNSAFE.into(),
            }),
            Fault::Size => Ok(WorkspaceResponse {
                status: 200,
                body: UNSAFE.repeat(MAX_RESPONSE_BYTES / UNSAFE.len() + 1),
            }),
            Fault::Rejected(_) | Fault::Unknown => {
                let status = match self.fault {
                    Fault::Rejected(status) => status,
                    _ => 503,
                };
                Ok(WorkspaceResponse {
                    status,
                    body: serde_json::to_string(&WorkspaceConfigApiError {
                        status,
                        code: UNSAFE.into(),
                        message: UNSAFE.into(),
                        classification: if matches!(self.fault, Fault::Unknown) {
                            WorkspaceConfigFailureClassification::Unknown
                        } else {
                            WorkspaceConfigFailureClassification::NotCommitted
                        },
                    })
                    .unwrap(),
                })
            }
            Fault::Unrecognized => Ok(WorkspaceResponse {
                status: 503,
                body: UNSAFE.into(),
            }),
            Fault::Identity | Fault::Deadline => {
                let mut value = attachment();
                if matches!(self.fault, Fault::Identity) {
                    value.workspace_id = UNSAFE.into();
                }
                Ok(WorkspaceResponse {
                    status: 200,
                    body: serde_json::to_string(&value).unwrap(),
                })
            }
        }
    }
}

fn assert_safe(capture: &Capture, reason: Reason, stage: Stage, public: &str) {
    let text = capture.text();
    assert!(
        text.contains(&format!("reason={reason:?}")),
        "missing reason: {text}"
    );
    assert!(
        text.contains(&format!("stage={stage:?}")),
        "missing stage: {text}"
    );
    assert!(text.contains("adapter=WorkspaceConfig"), "{text}");
    assert!(text.contains("correlation="), "{text}");
    for secret in [
        UNSAFE,
        "secret-token",
        "/private/config",
        "credential=unsafe",
    ] {
        assert!(
            !text.contains(secret),
            "diagnostics leaked unsafe boundary data"
        );
        assert!(
            !public.contains(secret),
            "public result leaked unsafe boundary data"
        );
    }
}
fn public_error(
    result: Result<serde_json::Value, WipOperationError>,
    code: Option<ProtocolErrorCode>,
) -> String {
    match (result, code) {
        (Err(WipOperationError::OutcomeUnknown(message)), None) => {
            assert!(message.contains("no automatic retry"));
            message
        }
        (Err(WipOperationError::Protocol(error)), Some(expected)) => {
            assert_eq!(error.code, expected);
            error.message
        }
        _ => panic!("unexpected public error classification"),
    }
}

#[tokio::test]
async fn production_reply_failures_retain_reasons_without_retrying_mutations() {
    let cases = [
        (
            Fault::Unavailable,
            Reason::TransportUnavailable,
            Stage::Request,
            ProtocolErrorCode::Internal,
            false,
        ),
        (
            Fault::InvalidPath,
            Reason::InvalidPath,
            Stage::Request,
            ProtocolErrorCode::Internal,
            false,
        ),
        (
            Fault::Communication,
            Reason::Communication,
            Stage::Request,
            ProtocolErrorCode::Internal,
            true,
        ),
        (
            Fault::Decode,
            Reason::Decode,
            Stage::Response,
            ProtocolErrorCode::Internal,
            true,
        ),
        (
            Fault::Size,
            Reason::ResponseSize,
            Stage::Response,
            ProtocolErrorCode::ResourceLimitExceeded,
            true,
        ),
        (
            Fault::Unknown,
            Reason::BackendUnknown,
            Stage::Response,
            ProtocolErrorCode::Internal,
            true,
        ),
        (
            Fault::Unrecognized,
            Reason::UnrecognizedResponse,
            Stage::Response,
            ProtocolErrorCode::Internal,
            true,
        ),
        (
            Fault::Panic,
            Reason::TaskJoin,
            Stage::Completion,
            ProtocolErrorCode::Internal,
            true,
        ),
    ];
    for (fault, reason, stage, read_code, applies) in cases {
        for mutation in [false, true] {
            let client = Client::new(fault);
            let backend = WorkspaceConfigBackend::new(client.clone());
            let capture = Capture::default();
            let result = backend
                .post::<serde_json::Value, _>(
                    if mutation { "/commit" } else { "/read" },
                    &operation_body(mutation),
                    mutation,
                )
                .with_subscriber(capture.dispatch())
                .await;
            let code = if (mutation && !matches!(fault, Fault::InvalidPath))
                || matches!(fault, Fault::Unknown)
            {
                None
            } else {
                Some(read_code)
            };
            let public = public_error(result, code);
            assert_safe(&capture, reason, stage, &public);
            let state = client.state.lock().unwrap();
            assert_eq!(state.requests, 1);
            assert_eq!(state.mutations, usize::from(mutation && applies));
            assert_eq!(state.saved.is_some(), mutation && applies);
        }
    }
}

#[tokio::test]
async fn production_typed_rejections_preserve_public_codes_and_never_apply() {
    for (status, code) in [
        (403, ProtocolErrorCode::PermissionDenied),
        (409, ProtocolErrorCode::ValidatorMismatch),
        (413, ProtocolErrorCode::ResourceLimitExceeded),
        (422, ProtocolErrorCode::InvalidArguments),
        (500, ProtocolErrorCode::Internal),
    ] {
        for mutation in [false, true] {
            let client = Client::new(Fault::Rejected(status));
            let backend = WorkspaceConfigBackend::new(client.clone());
            let capture = Capture::default();
            let public = public_error(
                backend
                    .post::<serde_json::Value, _>(
                        if mutation { "/commit" } else { "/read" },
                        &operation_body(mutation),
                        mutation,
                    )
                    .with_subscriber(capture.dispatch())
                    .await,
                Some(code),
            );
            assert_safe(&capture, Reason::BackendRejected, Stage::Response, &public);
            assert!(capture.text().contains(&format!("status={status}")));
            let state = client.state.lock().unwrap();
            assert_eq!(state.requests, 1);
            assert_eq!(state.mutations, 0);
            assert!(state.saved.is_none());
        }
    }
}

#[tokio::test]
async fn production_deadline_is_unknown_after_effect_and_blocking_completion_is_not_retried() {
    for mutation in [false, true] {
        let client = Client::new(Fault::Deadline);
        let release = Release(client.clone());
        let backend =
            WorkspaceConfigBackend::new(client.clone()).with_deadline(Duration::from_secs(1));
        let capture = Capture::default();
        let body = operation_body(mutation);
        let request = backend
            .post::<serde_json::Value, _>(
                if mutation { "/commit" } else { "/read" },
                &body,
                mutation,
            )
            .with_subscriber(capture.dispatch());
        tokio::pin!(request);
        tokio::select! {
            biased;
            _ = client.started.notified() => {},
            _ = &mut request => panic!("request must start and apply its effect before timeout is observed"),
        }
        {
            let state = client.state.lock().unwrap();
            assert_eq!(state.mutations, usize::from(mutation));
            assert_eq!(state.saved.is_some(), mutation);
            assert!(!state.completed && !state.released);
        }
        let result = request.await;
        let public = public_error(
            result,
            if mutation {
                None
            } else {
                Some(ProtocolErrorCode::ResourceLimitExceeded)
            },
        );
        assert_safe(&capture, Reason::Deadline, Stage::Completion, &public);
        drop(release);
        let completed = client.clone();
        tokio::task::spawn_blocking(move || completed.wait_completed())
            .await
            .unwrap();
        let state = client.state.lock().unwrap();
        assert_eq!(state.requests, 1);
        assert_eq!(state.mutations, usize::from(mutation));
        assert_eq!(state.saved.is_some(), mutation);
    }
}

#[tokio::test]
async fn production_current_identity_and_task_loss_remain_safe() {
    for (fault, reason, stage, code) in [
        (
            Fault::Identity,
            Reason::Identity,
            Stage::Response,
            ProtocolErrorCode::PermissionDenied,
        ),
        (
            Fault::Panic,
            Reason::TaskJoin,
            Stage::Completion,
            ProtocolErrorCode::Internal,
        ),
        (
            Fault::Decode,
            Reason::Decode,
            Stage::Response,
            ProtocolErrorCode::Internal,
        ),
    ] {
        let client = Client::new(fault);
        let backend = WorkspaceConfigBackend::new(client.clone());
        let capture = Capture::default();
        let result = backend
            .current_async()
            .with_subscriber(capture.dispatch())
            .await;
        let message = match result {
            Err(WipOperationError::Protocol(error)) => {
                assert_eq!(error.code, code);
                error.message
            }
            _ => panic!("unexpected current classification"),
        };
        assert_safe(&capture, reason, stage, &message);
        assert_eq!(client.state.lock().unwrap().mutations, 0);
    }
}

#[tokio::test]
async fn production_attach_conversion_retains_original_reason_and_effect_count() {
    for (fault, reason, expected, mutations) in [
        (
            Fault::Communication,
            Reason::Communication,
            ConfigAttachError::OutcomeUnknown,
            1,
        ),
        (
            Fault::Identity,
            Reason::Identity,
            ConfigAttachError::OutcomeUnknown,
            1,
        ),
        (
            Fault::Rejected(403),
            Reason::BackendRejected,
            ConfigAttachError::Denied,
            0,
        ),
        (
            Fault::Rejected(422),
            Reason::BackendRejected,
            ConfigAttachError::InvalidRequest,
            0,
        ),
        (
            Fault::Rejected(500),
            Reason::BackendRejected,
            ConfigAttachError::Unavailable,
            0,
        ),
    ] {
        let client = Client::new(fault);
        let backend = WorkspaceConfigBackend::new(client.clone());
        let capture = Capture::default();
        assert_eq!(
            backend
                .attach("host-key-not-logged", None)
                .with_subscriber(capture.dispatch())
                .await,
            Err(expected)
        );
        assert_safe(&capture, reason, Stage::Attach, expected.message());
        let text = capture.text();
        if !matches!(fault, Fault::Identity) {
            assert!(text.contains("reason=AttachConversion"), "{text}");
        }
        assert!(!text.contains("host-key-not-logged"));
        let correlations: std::collections::BTreeSet<_> = text
            .lines()
            .filter_map(|line| line.split("correlation=").nth(1))
            .filter_map(|tail| tail.split(';').next())
            .collect();
        assert_eq!(
            correlations.len(),
            1,
            "request and attach conversion must correlate"
        );
        let state = client.state.lock().unwrap();
        assert_eq!(state.requests, 1);
        assert_eq!(state.mutations, mutations);
        assert_eq!(state.saved.is_some(), mutations == 1);
    }
}

#[tokio::test]
async fn production_oversized_request_is_rejected_before_dispatch() {
    let client = Client::new(Fault::Communication);
    let backend = WorkspaceConfigBackend::new(client.clone());
    let capture = Capture::default();
    let public = public_error(
        backend
            .post::<serde_json::Value, _>("/commit", &"x".repeat(MAX_RESPONSE_BYTES), true)
            .with_subscriber(capture.dispatch())
            .await,
        Some(ProtocolErrorCode::ResourceLimitExceeded),
    );
    assert_safe(&capture, Reason::RequestSize, Stage::Request, &public);
    let state = client.state.lock().unwrap();
    assert_eq!(state.requests, 0);
    assert_eq!(state.mutations, 0);
}

#[tokio::test]
async fn production_current_deadline_remains_bounded_and_has_no_mutation() {
    let client = Client::new(Fault::Deadline);
    let release = Release(client.clone());
    let backend = WorkspaceConfigBackend::new(client.clone()).with_deadline(Duration::from_secs(1));
    let capture = Capture::default();
    let request = backend.current_async().with_subscriber(capture.dispatch());
    tokio::pin!(request);
    tokio::select! {
        biased;
        _ = client.started.notified() => {},
        _ = &mut request => panic!("current request must enter the gate before timeout is observed"),
    }
    assert_eq!(client.state.lock().unwrap().mutations, 0);
    let error = request.await;
    let message = match error {
        Err(WipOperationError::Protocol(error)) => {
            assert_eq!(error.code, ProtocolErrorCode::ResourceLimitExceeded);
            error.message
        }
        _ => panic!("current deadline must remain a public resource-limit error"),
    };
    assert_safe(&capture, Reason::Deadline, Stage::Completion, &message);
    drop(release);
    let completed = client.clone();
    tokio::task::spawn_blocking(move || completed.wait_completed())
        .await
        .unwrap();
    let state = client.state.lock().unwrap();
    assert_eq!(state.requests, 1);
    assert_eq!(state.mutations, 0);
}

#[tokio::test]
async fn production_request_encoding_failure_is_safe_and_never_dispatched() {
    struct Unencodable;
    impl Serialize for Unencodable {
        fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
            Err(serde::ser::Error::custom(UNSAFE))
        }
    }
    let client = Client::new(Fault::Communication);
    let backend = WorkspaceConfigBackend::new(client.clone());
    let capture = Capture::default();
    let result = backend
        .post::<serde_json::Value, _>("/commit", &Unencodable, true)
        .with_subscriber(capture.dispatch())
        .await;
    let message = public_error(result, Some(ProtocolErrorCode::InvalidArguments));
    assert_safe(&capture, Reason::Decode, Stage::Request, &message);
    let state = client.state.lock().unwrap();
    assert_eq!(state.requests, 0);
    assert_eq!(state.mutations, 0);
}
