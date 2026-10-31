//! Explicit browser/component connection, not a separate product-process launcher.
//! Reuses ConnectedChat's real server/auth/storage/Runtime/installed chat Feature.
//! The browser uses the actual static Web build and real persisted session-cookie auth;
//! only the external model and response-loss/Busy timing are controlled. Shared Body
//! start/stop/restore and canonical/legacy Subject navigation stay real. Restore
//! requires the runner-owned isolation checked before either server starts.
//! Build `web/workspace` first, then explicitly select:
//! `cargo test -p yoi-workspace-server --lib chat_web_connected -- --ignored --test-threads=1`.
//! Uses the installed tools/web-ux Deno lock/Chromium; no browser download occurs.
//! It is not selected by the normal suite; a dedicated CI job is not yet wired.
use super::*;
use std::process::Stdio;
use tokio::io::AsyncWriteExt;

#[derive(Clone)]
struct BrowserModel {
    model: ChatModel,
    resumed: Arc<tokio::sync::Semaphore>,
}

#[async_trait::async_trait]
impl agen::llm_client::LlmClient for BrowserModel {
    fn clone_boxed(&self) -> Box<dyn agen::llm_client::LlmClient> {
        Box::new(self.clone())
    }

    async fn stream(
        &self,
        request: agen::llm_client::Request,
    ) -> std::result::Result<agen::llm_client::ResponseStream, agen::llm_client::ClientError> {
        let stream = self.model.stream(request).await?;
        if self.model.calls.load(std::sync::atomic::Ordering::SeqCst) == 6 {
            self.resumed.add_permits(1);
        }
        Ok(stream)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
#[ignore = "explicit Web build, Deno and installed Playwright Chromium required"]
async fn chat_web_connected_busy_unknown_replay_and_authorization_use_real_runtime_publications() {
    crate::restore_test_environment::require_restore_test_environment();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let assets = root.join("web/workspace/build");
    assert!(
        assets.join("index.html").is_file(),
        "build the production Workspace Web first"
    );
    let (inputs_tx, mut inputs) = tokio::sync::mpsc::channel(8);
    let conversation_id = Arc::new(Mutex::new(String::new()));
    let model = ChatModel {
        calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        conversation: conversation_id.clone(),
        inputs: inputs_tx,
        after_busy_boundary: Arc::new(tokio::sync::Semaphore::new(1)),
    };
    let calls = model.calls.clone();
    let resumed = Arc::new(tokio::sync::Semaphore::new(0));
    let model = BrowserModel {
        model,
        resumed: resumed.clone(),
    };
    let public_posts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let read_started = Arc::new(tokio::sync::Semaphore::new(0));
    let (release, wait) = tokio::sync::oneshot::channel::<()>();
    let release = Arc::new(Mutex::new(Some(release)));
    let wait = Arc::new(Mutex::new(Some(wait)));
    let f = ConnectedChat::with_web_assets(
        model,
        move |router| {
            // Observe the substituted model only. An accepted Notify or an Idle
            // snapshot alone cannot prove this new no-public-reply turn ran.
            let router = router.route(
                "/__fixture/model-resumed",
                axum::routing::get(move || {
                    let resumed = resumed.clone();
                    async move {
                        tokio::time::timeout(std::time::Duration::from_secs(30), resumed.acquire())
                            .await
                            .expect("restored input must reach the model")
                            .unwrap()
                            .forget();
                        axum::Json(json!({"observed":true}))
                    }
                }),
            );
            router.layer(axum::middleware::from_fn(
                move |request: axum::extract::Request, next: axum::middleware::Next| {
                    let public_posts = public_posts.clone();
                    let read_started = read_started.clone();
                    let release = release.clone();
                    let wait = wait.clone();
                    async move {
                        let path = request.uri().path();
                        let public = request.method() == axum::http::Method::POST
                            && path.starts_with(&conversations_path())
                            && path.ends_with("/messages");
                        let gate = if path == chat_worker_path() {
                            wait.lock().unwrap().take()
                        } else {
                            None
                        };
                        let response = next.run(request).await;
                        if let Some(gate) = gate {
                            assert!(response.status().is_success());
                            read_started.add_permits(1);
                            tokio::time::timeout(std::time::Duration::from_secs(60), gate)
                                .await
                                .expect("browser must submit the Busy follow-up")
                                .unwrap();
                        } else if public && response.status().is_success() {
                            let count =
                                public_posts.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                            if count == 2 {
                                tokio::time::timeout(
                                    std::time::Duration::from_secs(30),
                                    read_started.acquire(),
                                )
                                .await
                                .expect("real ConversationRead must start")
                                .unwrap()
                                .forget();
                            } else if count == 3 {
                                release.lock().unwrap().take().unwrap().send(()).unwrap();
                            }
                        }
                        response
                    }
                },
            ))
        },
        Some(assets),
    )
    .await;
    let subject = f.post(&format!("/api/w/{TEST_WORKSPACE_ID}/subjektiv/subjects"), json!({"operation_id":"browser-connected-subject","role":"Web connected companion","behavior_md":""})).await["id"].as_str().unwrap().to_owned();
    let other_subject = f.post(&format!("/api/w/{TEST_WORKSPACE_ID}/subjektiv/subjects"), json!({"operation_id":"browser-other-subject","role":"Other connected companion","behavior_md":""})).await["id"].as_str().unwrap().to_owned();
    let conversation = f.post(&conversations_path(), json!({"operation_id":format!("web-subject-conversation/v1/{subject}"),"subject_id":subject})).await;
    let c = conversation["id"].as_str().unwrap();
    *conversation_id.lock().unwrap() = c.into();
    // Let the browser use normal Body admission, including its default profile.
    // Remove only the unused fixture Runtime so the real signed HTTP path wins.
    assert!(matches!(
        f.api
            .runtime
            .unregister_if_idle(EMBEDDED_WORKER_RUNTIME_ID, 100)
            .unwrap(),
        crate::hosts::RuntimeRegistryUnregisterResult::Removed
    ));
    f.lose_next_input_ack
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let foreign = seed_test_api_token(f.api.store.as_ref(), "chat-connected-foreign");
    let mut sessions = Vec::new();
    for token in [&f.owner, &foreign] {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            format!("Bearer {token}").parse().unwrap(),
        );
        let actor = resolve_actor(&ServerAuthApi::from(&f.api), &headers)
            .await
            .unwrap()
            .unwrap();
        let session = uuid::Uuid::now_v7().to_string();
        f.api
            .store
            .create_browser_session(&BrowserSessionRecord {
                token_hash: crate::auth::token_hash(&session),
                session_id: uuid::Uuid::now_v7().to_string(),
                user_id: actor.user_id,
                created_at: now_registry_timestamp(),
                expires_at: "2099-01-01T00:00:00Z".into(),
                revoked_at: None,
            })
            .unwrap();
        sessions.push(session);
    }
    let mut child = tokio::process::Command::new("deno")
        .current_dir(&root)
        .args([
            "run",
            "--cached-only",
            "--config",
            "tools/web-ux/deno.json",
            "-A",
            "tools/web-ux/browser-tests/subject_conversation_connected.ts",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("explicit browser test requires Deno");
    let artifacts = root.join("target/t808-validation/body-connected");
    let wire = serde_json::to_vec(&json!({"origin":f.url,"workspace_id":TEST_WORKSPACE_ID,"subject_id":subject,"other_subject_id":other_subject,"subject_name":"Web connected companion","conversation_id":c,"owner_token":f.owner,"foreign_token":foreign,"owner_session":sessions[0],"foreign_session":sessions[1],"artifact_dir":artifacts})).unwrap();
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(&wire).await.unwrap();
    drop(stdin);
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(120),
        child.wait_with_output(),
    )
    .await
    .expect("connected browser deadline")
    .unwrap();
    let redact = |bytes: &[u8]| {
        String::from_utf8_lossy(bytes)
            .replace(&f.owner, "[redacted]")
            .replace(&foreign, "[redacted]")
            .replace(&sessions[0], "[redacted]")
            .replace(&sessions[1], "[redacted]")
    };
    assert!(
        output.status.success(),
        "connected browser failed:\n{}\n{}",
        redact(&output.stdout),
        redact(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout)
            .contains("real Web -> Backend -> Runtime -> explicit public replies passed")
    );
    let _ = next_chat_input(&mut inputs, 0).await;
    let busy = next_chat_input(&mut inputs, 1).await;
    assert!(
        busy.contains("WEB_CONNECTED_FIRST_INPUT") && busy.contains("WEB_CONNECTED_BUSY_INPUT")
    );
    for phase in 2..=4 {
        next_chat_input(&mut inputs, phase).await;
    }
    let resumed = next_chat_input(&mut inputs, 5).await;
    assert!(resumed.contains("WEB_CONNECTED_AFTER_RESTART"));
    assert!(resumed.contains("WEB_CONNECTED_FIRST_INPUT"));
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        6,
        "only the new post-restore input may reactivate the model"
    );
    let (status, public) = account(&f.api, &f.owner, "GET", &messages_path(c), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(public["messages"].as_array().unwrap().len(), 5);
    assert!(!public.to_string().contains("INTERNAL_ASSISTANT"));
    assert!(!public.to_string().contains("edited next draft"));
    let (status, body) = account(
        &f.api,
        &f.owner,
        "GET",
        &format!("/api/w/{TEST_WORKSPACE_ID}/subjektiv/subjects/{subject}/body"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["current"]["runtime_id"], "connected-chat");
    assert_eq!(body["observation"]["worker"]["state"], "stopped");
}
