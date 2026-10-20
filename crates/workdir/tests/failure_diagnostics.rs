use workdir::external::ExternalWorkdirOperationError;
use workdir::http::{WorkdirTransportError, WorkdirTransportErrorCode as Code};
use workdir::{WorkdirError, WorkdirFailureReason as Reason};

#[test]
fn failure_reasons_survive_internal_transports_without_provider_text() {
    for reason in [
        Reason::SessionClosed,
        Reason::ProviderUnavailable,
        Reason::CommandTerminalMissing,
        Reason::HttpTimeout,
        Reason::HttpConnect,
        Reason::HttpTransfer,
        Reason::HttpResponseInvalid,
        Reason::HttpResponseTooLarge,
    ] {
        let source = WorkdirError::Unavailable("secret-token /private/path".into())
            .with_failure_reason(reason);
        let transport = WorkdirTransportError::from_workdir_error(&source);
        let wire = serde_json::to_string(&transport).unwrap();
        assert!(!wire.contains("secret-token"));
        assert!(!wire.contains("/private/path"));
        assert_eq!(transport.code, Code::Unavailable);
        assert_eq!(transport.code.http_status(), 503);
        assert_eq!(transport.message, "Workdir session is unavailable");
        assert_eq!(serde_json::to_value(reason).unwrap(), reason.as_str());
        let decoded: WorkdirTransportError = serde_json::from_str(&wire).unwrap();
        let external = ExternalWorkdirOperationError::from_transport_error(decoded);
        let external_wire = serde_json::to_string(&external).unwrap();
        let external: ExternalWorkdirOperationError = serde_json::from_str(&external_wire).unwrap();
        let error = external.into_transport_error().into_workdir_error();
        assert_eq!(error.failure_reason(), Some(reason));
        assert!(matches!(
            error.classification_source(),
            WorkdirError::Unavailable(_)
        ));
        assert_eq!(
            WorkdirTransportError::from_workdir_error(&error).failure_reason,
            Some(reason)
        );
    }
}

#[test]
fn legacy_errors_and_nested_context_preserve_classification() {
    let wire = r#"{"code":"unavailable","message":"Workdir session is unavailable"}"#;
    let old: WorkdirTransportError = serde_json::from_str(wire).unwrap();
    assert!(old.failure_reason.is_none());
    assert!(matches!(
        old.into_workdir_error(),
        WorkdirError::Unavailable(_)
    ));
    let closed = WorkdirTransportError::from_workdir_error(&WorkdirError::SessionClosed);
    assert_eq!(closed.failure_reason, Some(Reason::SessionClosed));
    // Reason-looking text never becomes a typed diagnosis.
    let generic = WorkdirError::Unavailable("http_timeout".into());
    assert_eq!(generic.failure_reason(), Some(Reason::ProviderUnavailable));
    let error = WorkdirError::DenialContext {
        reason: workdir::WorkdirDenialReason::OsPermissionDenied,
        source: Box::new(
            WorkdirError::OutcomeUnknown("do not retry".into())
                .with_failure_reason(Reason::HttpTransfer),
        ),
    };
    let error = WorkdirTransportError::from_workdir_error(&error).into_workdir_error();
    assert!(matches!(
        error.classification_source(),
        WorkdirError::OutcomeUnknown(_)
    ));
    assert_eq!(
        error.denial_reason(),
        Some(workdir::WorkdirDenialReason::OsPermissionDenied)
    );
    assert_eq!(error.failure_reason(), Some(Reason::HttpTransfer));
}

#[test]
fn provider_cannot_inject_free_text_failure_reason() {
    assert!(
        serde_json::from_str::<WorkdirTransportError>(
            r#"{"code":"unavailable","message":"x","failure_reason":"secret /path"}"#
        )
        .is_err()
    );
}
