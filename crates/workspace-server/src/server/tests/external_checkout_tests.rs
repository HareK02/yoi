//! External codec -> actual session dispatch -> mutation safety -> HTTP error.
//! Channel fixture only; no product process or real mutation.
use super::*;
use workdir::{
    CheckoutOperation as Op, CheckoutRequest, WorkdirDenialReason as Reason, WorkdirPath,
    WorkdirSession, WorkdirSessionCapability, external::ExternalWorkdirOperationOutcome,
    http::WorkdirTransportErrorCode as Code,
};

#[tokio::test]
async fn external_checkout_diagnostics_preserve_mutation_outcome_and_read_failures() {
    for code in [
        Code::Unavailable,
        Code::Transport,
        Code::Denied,
        Code::Conflict,
        Code::OutcomeUnknown,
    ] {
        for reason in [None, Some(Reason::ReadOnlySession)] {
            for operation in [
                Op::Read {
                    offset: 0,
                    limit: 1,
                    max_bytes: 10,
                },
                Op::Create {
                    path: WorkdirPath::new("new.txt").unwrap(),
                    content: b"fixture".to_vec(),
                },
                Op::Write {
                    content: b"fixture".to_vec(),
                    expected_hash: [0; 32],
                },
                Op::Edit {
                    old_string: "old".into(),
                    new_string: "new".into(),
                    replace_all: false,
                    expected_hash: [0; 32],
                },
            ] {
                let mutation = operation.capability() != WorkdirSessionCapability::Read;
                let expected_code =
                    if mutation && matches!(code, Code::Unavailable | Code::Transport) {
                        Code::OutcomeUnknown
                    } else {
                        code
                    };
                let (sender, mut commands) = tokio::sync::mpsc::channel(1);
                let connection = Arc::new(ExternalProviderConnection {
                    grant_id: "fixture-grant".into(),
                    workdir_id: "fixture-workdir".into(),
                    provider_instance_id: "fixture-provider".into(),
                    generation: 1,
                    expires_at: None,
                    capabilities: workdir::WorkdirSessionCapabilities::ALL,
                    admission: Arc::new(tokio::sync::Semaphore::new(16)),
                    shutdown_confirmed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                    sender,
                });
                let session = Arc::new(ExternalProviderWorkdirSession::new(connection, None));
                let caller = tokio::spawn(async move {
                    session
                        .checkout_execute(CheckoutRequest {
                            target: WorkdirPath::root(),
                            validator: vec![1],
                            operation,
                        })
                        .await
                });
                let command =
                    tokio::time::timeout(std::time::Duration::from_secs(5), commands.recv())
                        .await
                        .unwrap()
                        .unwrap();
                let ExternalProviderCommand::Operation {
                    operation,
                    response,
                    ..
                } = command
                else {
                    panic!("expected dispatched operation");
                };
                assert!(matches!(
                    operation,
                    WorkdirSessionOperation::CheckoutExecute(_)
                ));
                let failed = serde_json::from_value::<ExternalWorkdirOperationOutcome>(json!({
                    "outcome":"failed", "error":{"code":code, "denial_reason":reason},
                }))
                .unwrap();
                let ExternalWorkdirOperationOutcome::Failed { error } = failed else {
                    panic!("expected fixture failure");
                };
                response.send(Err(error.into_transport_error())).unwrap();
                let error = tokio::time::timeout(std::time::Duration::from_secs(5), caller)
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap_err();
                assert!(
                    matches!(
                        commands.try_recv(),
                        Err(tokio::sync::mpsc::error::TryRecvError::Empty
                            | tokio::sync::mpsc::error::TryRecvError::Disconnected)
                    ),
                    "must not retry"
                );
                let forwarded = WorkdirTransportError::from_workdir_error(&error);
                assert_eq!(
                    forwarded.code, expected_code,
                    "{code:?}, {reason:?}, mutation={mutation}: {error:?}"
                );
                assert_eq!(error.denial_reason(), reason);
                assert_eq!(forwarded.denial_reason, reason);
                if expected_code == Code::OutcomeUnknown {
                    assert!(forwarded.message.contains("inspect before retrying"));
                }
                let response = InternalWorkdirOperationError::Provider(forwarded).into_response();
                assert_eq!(response.status().as_u16(), expected_code.http_status());
                let body = axum::body::to_bytes(response.into_body(), 2048)
                    .await
                    .unwrap();
                let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
                assert_eq!(body["code"], expected_code.as_str());
                assert!(body.get("denial_reason").is_none());
            }
        }
    }
}
