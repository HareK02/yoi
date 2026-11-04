//! The real authorization/factory fixture lives in workspace-server. This leaf
//! exercises only WorkspaceClient transfer failures and response validation; it
//! must not substitute simulated policy decisions for that connected fixture.
use super::*;
use crate::{WorkspaceClientError, WorkspaceResponse};
use protocol::session_failure::{SessionPrincipal, SessionRegistrationCause as Cause};
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Debug)]
struct Client {
    reply: Reply,
    calls: AtomicUsize,
}
#[derive(Debug)]
enum Reply {
    Response(u16, String),
    Unavailable,
    Request,
}
impl WorkspaceClient for Client {
    fn kind(&self) -> &str {
        "registration-transfer-test"
    }
    fn workspace_id(&self) -> Option<&str> {
        Some("workspace")
    }
    fn is_available(&self) -> bool {
        true
    }
    fn execute(
        &self,
        request: WorkspaceRequest,
    ) -> Result<WorkspaceResponse, WorkspaceClientError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        assert_eq!(request.method, WorkspaceRequestMethod::Post);
        assert_eq!(request.path, "/api/w/workspace/subjektiv/sessions");
        match &self.reply {
            Reply::Response(status, body) => Ok(WorkspaceResponse {
                status: *status,
                body: body.clone(),
            }),
            Reply::Unavailable => Err(WorkspaceClientError::Unavailable(
                "TEST_ONLY_CREDENTIAL /private/path raw_http_body".into(),
            )),
            Reply::Request => Err(WorkspaceClientError::Request(
                "TEST_ONLY_CREDENTIAL /private/path raw_http_body".into(),
            )),
        }
    }
}
fn client(reply: Reply) -> Client {
    Client {
        reply,
        calls: AtomicUsize::new(0),
    }
}
fn failure_body(cause: Cause) -> String {
    let mut error = RepositoryApiError::new(
        403,
        "ignored",
        "TEST_ONLY_CREDENTIAL /private/path raw_http_body",
        vec![],
    );
    error.failure =
        Some(protocol::session_failure::session_registration_attachment(cause).unwrap());
    serde_json::to_string(&error).unwrap()
}
fn assert_failure(client: Client, expected: Cause) -> SubjektivHostError {
    let error = record_backend_session(&client, "session", true).unwrap_err();
    assert_eq!(
        client.calls.load(Ordering::SeqCst),
        1,
        "classification grants no replay"
    );
    assert_eq!(error.session_registration_cause(), expected);
    let attachment = protocol::session_failure::session_registration_attachment(
        error.session_registration_cause(),
    )
    .unwrap();
    let encoded = serde_json::to_string(&attachment).unwrap();
    for secret in ["TEST_ONLY_CREDENTIAL", "/private/path", "raw_http_body"] {
        assert!(!encoded.contains(secret));
        assert!(!error.to_string().contains(secret));
    }
    error
}

#[test]
fn session_registration_http_reuses_typed_causes_not_status_or_prose() {
    for cause in [
        Cause::denied(SessionPrincipal::Operator),
        Cause::denied(SessionPrincipal::Subject),
        Cause::attribution_missing(),
        Cause::binding_mismatch(),
        Cause::outcome_unknown(),
    ] {
        for status in [403, 409, 500] {
            assert_failure(client(Reply::Response(status, failure_body(cause))), cause);
        }
    }
    // A legacy 403 without a domain attestation is not a certified denial.
    let body = serde_json::to_string(&RepositoryApiError::new(
        403,
        "permission_denied",
        "session.record denied",
        vec![],
    ))
    .unwrap();
    assert_failure(client(Reply::Response(403, body)), Cause::outcome_unknown());
}

#[test]
fn session_registration_transport_and_unknown_keep_error_sources_without_disclosure_or_retry() {
    for (reply, cause) in [
        (Reply::Unavailable, Cause::transport()),
        (Reply::Request, Cause::outcome_unknown()),
    ] {
        let error = assert_failure(client(reply), cause);
        assert!(
            std::error::Error::source(&error)
                .unwrap()
                .downcast_ref::<WorkspaceClientError>()
                .is_some()
        );
    }
    let legacy =
        SubjektivHostError::PermissionDenied("session.record denied for secret subject".into());
    assert_eq!(
        legacy.session_registration_cause(),
        Cause::outcome_unknown()
    );
}

#[test]
fn session_registration_rejects_malformed_incomplete_and_mismatched_responses() {
    for (status, body) in [
        (403, "not json"),
        (200, "not json"),
        (200, "{}"),
        (200, r#"{"session_id":"session","subject_id":""}"#),
        (
            200,
            r#"{"session_id":"other-session","subject_id":"subject"}"#,
        ),
    ] {
        assert_failure(
            client(Reply::Response(status, body.into())),
            Cause::invalid_response(),
        );
    }
    let allowed = client(Reply::Response(
        200,
        r#"{"session_id":"session","subject_id":"subject"}"#.into(),
    ));
    let output = record_backend_session(&allowed, "session", true).unwrap();
    assert_eq!(output.session_id, "session");
    assert_eq!(output.subject_id, "subject");
    assert_eq!(allowed.calls.load(Ordering::SeqCst), 1);
}
