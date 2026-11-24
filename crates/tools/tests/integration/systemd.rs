//! Opt-in Linux/systemd component integration. Production Bash collector -> real
//! HTTP client/dispatcher -> Runtime-style or pinned External provider -> scoped
//! process -> typed terminal -> retained Bash output. Only HTTP session opening is
//! pre-seeded; this does not claim Runtime authentication or External WebSocket
//! registration coverage. Temporary roots, listener/task and scopes are owned here.

use super::*;
use axum::{Json, Router, extract::State, routing::post};
use workdir::http::*;
use workdir::{Workdir, WorkdirSession, WorkdirSessionCapabilities};

struct Fixture {
    _root: TempDir,
    provider: Arc<LocalWorkdirSession>,
    remote: WorkdirSessionHandle,
    server: tokio::task::JoinHandle<()>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

async fn open(
    State(provider): State<Arc<LocalWorkdirSession>>,
) -> Json<OpenWorkdirSessionResponse> {
    Json(OpenWorkdirSessionResponse {
        session_id: WorkdirSessionId::new("isolated-session").unwrap(),
        workdir_id: provider.workdir().id().clone(),
        capabilities: provider.capabilities(),
        environment: provider.environment(),
    })
}

async fn operation(
    State(provider): State<Arc<LocalWorkdirSession>>,
    Json(request): Json<WorkdirSessionOperationRequest>,
) -> Json<WorkdirSessionOperationResult> {
    Json(
        dispatch_workdir_session_operation(provider.as_ref(), request.operation)
            .await
            .unwrap(),
    )
}

impl Fixture {
    async fn new(external: bool) -> Self {
        let root = TempDir::new().unwrap();
        let local = if external {
            LocalWorkdirSession::external_with_capabilities_pinned(
                Workdir::new("external-isolated"),
                workdir::ExternalWorkdirRoot::pin(root.path()).unwrap(),
                workdir::BoundedReadLimits::EXTERNAL_DEFAULT,
                WorkdirSessionCapabilities::ALL,
            )
            .unwrap()
        } else {
            LocalWorkdirSession::materialized(
                root.path().into(),
                root.path().into(),
                manifest::SharedScope::new(Scope::writable(root.path()).unwrap()),
                WorkdirSessionCapabilities::ALL,
            )
        }
        .with_command_host(workdir::CommandHost::systemd(32 * 1024 * 1024, 1).unwrap())
        .unwrap();
        let provider = Arc::new(local);
        let app = Router::new()
            .route("/v1/working-directories/{id}/sessions", post(open))
            .route("/v1/workdir-sessions/{id}/operations", post(operation))
            .with_state(provider.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let remote = Arc::new(
            RemoteWorkdirSession::open(
                reqwest::Client::builder().no_proxy().build().unwrap(),
                base.parse().unwrap(),
                "fixture-token",
                provider.workdir().id().clone(),
                OpenWorkdirSessionRequest {
                    session_id: WorkdirSessionId::new("isolated-session").unwrap(),
                    owner_worker_id: None,
                },
            )
            .await
            .unwrap(),
        );
        Self {
            _root: root,
            provider,
            remote,
            server,
        }
    }
}

#[tokio::test]
#[ignore = "requires Linux cgroup v2, user systemd and common user@.service OOMPolicy=continue"]
async fn systemd_oom_crosses_http_and_bash_for_both_host_provider_roots() {
    tokio::time::timeout(std::time::Duration::from_secs(60), async {
        for external in [false, true] {
            let fixture = Fixture::new(external).await;
            let view = tools::bash_output::BashOutputView::new();
            let (_, bash) = tools::bash_tool(fixture.remote.clone(), view.clone())();
            let output = call(
                &bash,
                json!({"cwd":".", "timeout":15,
                "command":"printf retained-prefix; x=$(head -c 67108864 /dev/zero | tr '\\0' x)"}),
            )
            .await;
            assert!(
                output.summary.contains("OOM killed: systemd unit evidence"),
                "{}",
                output.summary
            );
            let body: serde_json::Value = serde_json::from_str(&output.content.unwrap()).unwrap();
            assert_eq!(
                body["output"]["terminal"]["resource_outcome"],
                json!({"kind":"systemd", "oom":"systemd_oom_kill", "cleanup_confirmed":true})
            );
            assert_eq!(body["output"]["terminal"]["status"], "failed");
            assert_eq!(body["output"]["terminal"]["transport_complete"], true);
            assert_eq!(body["stdout_preview"], "retained-prefix");
            let id = body["output"]["id"].as_str().unwrap();
            assert_eq!(
                view.read(id, workdir::CommandStream::Stdout, 0, 65536)
                    .unwrap(),
                b"retained-prefix"
            );
            for (script, expected) in [
                ("kill -KILL $$", None),
                ("exit 137", None),
                ("printf after-oom", Some("after-oom")),
                ("printf '%s' '$HOME'", Some("$HOME")),
            ] {
                let output = call(&bash, json!({"cwd":".","command":script,"timeout":10})).await;
                assert!(!output.summary.contains("OOM killed"), "{}", output.summary);
                let body: serde_json::Value =
                    serde_json::from_str(&output.content.unwrap()).unwrap();
                assert_ne!(
                    body["output"]["terminal"]["resource_outcome"]["oom"],
                    "systemd_oom_kill"
                );
                if let Some(expected) = expected {
                    assert_eq!(body["output"]["terminal"]["status"], "completed");
                    assert_eq!(body["stdout_preview"], expected);
                }
            }
            fixture.provider.close().await.unwrap();
        }
    })
    .await
    .expect("bounded systemd/HTTP/Bash integration");
}
