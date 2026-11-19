//! Production Runtime -> HTTP router -> generated Runtime client. The execution
//! backend alone is substituted to inject exact command settlement evidence.
use super::*;
use protocol::{WorkerCommandDisposition as D, WorkerCommandKind as K, WorkerCommandResult as R};

struct CommandBackend(Mutex<WorkerExecutionResult>);
impl WorkerExecutionBackend for CommandBackend {
    fn backend_id(&self) -> &str {
        "command-result-http"
    }
    fn spawn_worker(&self, request: WorkerExecutionSpawnRequest) -> WorkerExecutionSpawnResult {
        AcceptingBackend.spawn_worker(request)
    }
    fn dispatch_input(
        &self,
        _worker_ref: &WorkerRef,
        _input: WorkerInput,
    ) -> WorkerExecutionResult {
        self.0.lock().unwrap().clone()
    }
    fn cancel_worker(&self, _worker_ref: &WorkerRef) -> WorkerExecutionResult {
        self.0.lock().unwrap().clone()
    }
}

#[tokio::test]
async fn command_admission_and_unknown_evidence_survive_runtime_http_client() {
    let backend = Arc::new(CommandBackend(Mutex::new(WorkerExecutionResult::accepted(
        WorkerExecutionOperation::Cancel,
    ))));
    let runtime =
        Runtime::with_execution_backend(RuntimeOptions::default(), backend.clone()).unwrap();
    runtime
        .store_config_bundle(test_bundle(ProfileSelector::Builtin(
            "builtin:coder".into(),
        )))
        .unwrap();
    let worker = runtime
        .create_worker_scoped(
            &RuntimeWorkspaceScope::new("local", "local-token"),
            task_request("command-http-evidence"),
        )
        .unwrap();
    let app = runtime_http_router(runtime.clone(), "command-token".into());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let client = runtime_api::RuntimeApiClient::builder(&format!("http://{address}"))
        .unwrap()
        .authorizer(TestRuntimeAuthorizer("command-token".into()))
        .build()
        .unwrap();
    let worker_id = worker.worker_ref.worker_id.to_string();
    for disposition in [D::Accepted, D::InvalidState, D::StaleCommandId, D::Conflict] {
        let command = R::Acknowledged {
            acknowledgement: protocol::WorkerCommandAcknowledgement {
                command_id: 44,
                command: K::Cancel,
                disposition,
                state: protocol::WorkerStateSnapshot::initial(),
            },
        };
        let mut result = if disposition == D::Accepted {
            WorkerExecutionResult::accepted(WorkerExecutionOperation::Cancel)
        } else {
            WorkerExecutionResult::rejected(WorkerExecutionOperation::Cancel, "not used as cause")
        };
        result.command = Some(command.clone());
        *backend.0.lock().unwrap() = result;
        let response = client
            .cancel_worker(
                worker_id.clone(),
                runtime_api::WorkerLifecycleRequest {
                    reason: None,
                    stop_intent: None,
                },
            )
            .await;
        if disposition == D::Accepted {
            assert_eq!(response.unwrap().ack.command, Some(command));
        } else {
            let runtime_api::client_support::ClientError::Public { error, .. } =
                response.unwrap_err()
            else {
                panic!("expected typed control failure")
            };
            assert_eq!(error.command_result(), Some(&command));
            assert_eq!(
                error.control_cause(),
                Some(protocol::control_failure::RuntimeControlCause::ExecutionRejected)
            );
        }
    }
    for reason in [
        protocol::WorkerCommandObservationFailure::TimedOut,
        protocol::WorkerCommandObservationFailure::Lagged,
        protocol::WorkerCommandObservationFailure::Closed,
    ] {
        let command = R::Unconfirmed {
            command: K::Cancel,
            command_id: Some(45),
            delivery: protocol::WorkerCommandDelivery::Enqueued,
            reason,
        };
        let mut result =
            WorkerExecutionResult::errored(WorkerExecutionOperation::Cancel, "unconfirmed");
        result.command = Some(command.clone());
        *backend.0.lock().unwrap() = result;
        let response = client
            .cancel_worker(
                worker_id.clone(),
                runtime_api::WorkerLifecycleRequest {
                    reason: None,
                    stop_intent: None,
                },
            )
            .await;
        let runtime_api::client_support::ClientError::Public { error, .. } = response.unwrap_err()
        else {
            panic!("expected typed uncertainty")
        };
        assert_eq!(error.command_result(), Some(&command));
        assert_eq!(
            error.control_cause(),
            Some(protocol::control_failure::RuntimeControlCause::ResultUnavailable)
        );
    }
    assert_eq!(
        runtime
            .worker_detail(&worker.worker_ref)
            .unwrap()
            .worker_state,
        worker.worker_state
    );
    server.abort();
    let _ = server.await;
}
