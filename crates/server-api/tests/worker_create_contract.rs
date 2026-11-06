//! Generated Backend client → isolated loopback HTTP response → typed result.
//! The server response producer is substituted; authorization and persistence are
//! exercised by workspace-server's connected tests, not by this codec connection.
use runtime_api::startup::*;
use server_api::{CreateWorkspaceWorkerRequest, ServerApiClient, client_support, server_support};
use server_support::framework::{Body, Response, Router, post};

struct ServerTask(tokio::task::JoinHandle<()>);
impl Drop for ServerTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

fn request() -> CreateWorkspaceWorkerRequest {
    serde_json::from_value(serde_json::json!({
        "runtime_id": "runtime", "display_name": "Worker", "feature_connections": {}
    }))
    .unwrap()
}

#[tokio::test]
async fn every_creation_client_decodes_actual_problem_status_and_operation_facts() {
    let timeout = WorkerCreateFailure::from_cause(
        StartupCause::Boundary(StartupBoundaryCause {
            reason: StartupBoundaryReason::Timeout,
        }),
        StartupOperationFacts {
            acceptance: CreationAcceptance::Unknown,
            execution: ExecutionOutcome::Unknown,
            finalize: None,
            backend_compensation: None,
            correlation: None,
        },
    )
    .unwrap()
    .problem(
        StartupBoundary::Browser,
        &SessionFailureVisibility::default(),
        "",
    )
    .unwrap();
    for (status, body) in [
        (409, include_bytes!("../../runtime-api/tests/fixtures/startup-browser-session-record-denied.problem.json").to_vec()),
        (502, include_bytes!("../../runtime-api/tests/fixtures/startup-browser-registration-response-lost.problem.json").to_vec()),
        (504, timeout.into_body()),
    ] {
        let DecodeStartupProblem::Known(expected) = decode_startup_problem(&body, status, PROBLEM_CONTENT_TYPE, StartupBoundary::Browser) else {
            panic!("producer fixture must have a known typed cause");
        };
        let router = ["/api/workers", "/api/w/workspace/workers", "/api/w/workspace/worker-control/workers"]
            .into_iter().fold(Router::new(), |router, path| {
                let body = body.clone();
                router.route(path, post(move || {
                    let body = body.clone();
                    async move { Response::builder().status(status).header("content-type", PROBLEM_CONTENT_TYPE).body(Body::from(body)).unwrap() }
                }))
            });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let _server = ServerTask(tokio::spawn(async move {
            server_support::framework::serve(listener, router).await.unwrap();
        }));
        let client = ServerApiClient::builder(format!("http://{address}"))
            .unwrap().request_timeout(std::time::Duration::from_secs(3)).build().unwrap();
        for result in [
            client.workspace_worker_create("workspace".into(), request()).await,
            client.workspace_worker_create_alias(request()).await,
            client.worker_control_spawn("workspace".into(), request()).await,
        ] {
            let Err(client_support::ClientError::Public { status: actual_status, error }) = result else {
                panic!("generated route must invoke the Problem decoder, not legacy Deserialize");
            };
            assert_eq!(actual_status.as_u16(), status);
            let actual = error.failure().expect("retain typed non-HTTP failure");
            assert_eq!(actual.startup().primary, expected.startup().primary);
            assert_eq!(actual.public_facts(), expected.public_facts());
        }
    }
}

#[test]
fn creation_openapi_declares_gateway_timeout_with_both_error_representations() {
    let document: serde_json::Value = serde_json::from_str(
        &server_api::canonical_openapi_document()
            .unwrap()
            .to_json()
            .unwrap(),
    )
    .unwrap();
    for path in ["/api/workers", "/api/w/{workspace_id}/workers"] {
        let content = &document["paths"][path]["post"]["responses"]["504"]["content"];
        assert!(content.get("application/problem+json").is_some());
        assert!(content.get("application/json").is_some());
    }
}
