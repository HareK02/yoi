//! Explicit real Browser/Backend/signed Runtime connection. Reuses ConnectedChat;
//! only the external model and one dropped WS ACK are substituted. Build Workspace Web,
//! then select `cargo test -p yoi-workspace-server --lib chat_web_connected -- --ignored`.
//! Browser tooling is opt-in; this target has no dedicated CI job yet.
use super::*;
use std::process::Stdio;
use tokio::io::AsyncWriteExt;

#[derive(Clone)]
struct MessagingModel {
    calls: Arc<std::sync::atomic::AtomicUsize>,
    conversation: Arc<Mutex<String>>,
}
#[async_trait::async_trait]
impl agen::llm_client::LlmClient for MessagingModel {
    fn clone_boxed(&self) -> Box<dyn agen::llm_client::LlmClient> {
        Box::new(self.clone())
    }
    async fn stream(
        &self,
        request: agen::llm_client::Request,
    ) -> std::result::Result<agen::llm_client::ResponseStream, agen::llm_client::ClientError> {
        use agen::llm_client::{
            event::{Event, StopReason},
            types::Item,
        };
        let phase = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        for item in &request.items {
            if let Item::ToolResult { is_error, .. } = item {
                assert!(!is_error, "actual chat tool failed: {item:?}");
            }
        }
        let events = match phase % 3 {
            0 => vec![
                Event::tool_use_start(0, format!("messaging-read-{phase}"), "ConversationRead"),
                Event::tool_input_delta(
                    0,
                    json!({"conversation_id":self.conversation.lock().unwrap().clone()})
                        .to_string(),
                ),
                Event::tool_use_stop(0),
            ],
            1 => {
                let page = request
                    .items
                    .iter()
                    .rev()
                    .find_map(|item| match item {
                        Item::ToolResult {
                            content: Some(content),
                            ..
                        } => serde_json::from_str::<ChatMessagePage>(content).ok(),
                        _ => None,
                    })
                    .expect("real ConversationRead must precede explicit publication");
                vec![Event::tool_use_start(0, format!("messaging-send-{phase}"), "ConversationSend"), Event::tool_input_delta(0, json!({"conversation_id":page.conversation.id,"operation_id":format!("messaging-public-{phase}"),"text":format!("explicit messaging reply {phase}"),"expected_last_user_message_id":page.conversation.last_user_message_id}).to_string()), Event::tool_use_stop(0)]
            }
            _ => vec![
                Event::text_block_start(0),
                Event::text_delta(0, "INTERNAL_ASSISTANT_NOT_PUBLIC"),
                Event::text_block_stop(0, Some(StopReason::EndTurn)),
            ],
        };
        Ok(Box::pin(futures::stream::iter(events.into_iter().map(Ok))))
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
#[ignore = "explicit Web build, Deno and installed Playwright Chromium required"]
async fn chat_web_connected_messaging_saved_replay_live_tool_reply_and_reconnect() {
    crate::restore_test_environment::require_restore_test_environment();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let baseline = std::env::var_os("T855_VISUAL_BASELINE").is_some();
    let assets = if baseline {
        root.join("target/t855-baseline/web/workspace/build")
    } else {
        root.join("web/workspace/build")
    };
    assert!(
        assets.join("index.html").is_file(),
        "build the selected production Web assets first"
    );
    let model = MessagingModel {
        calls: Arc::default(),
        conversation: Arc::default(),
    };
    let calls = model.calls.clone();
    let conversation_id = model.conversation.clone();
    let f = ConnectedChat::with_web_assets(model, |router| router, Some(assets)).await;
    let subject = f.post(&format!("/api/w/{TEST_WORKSPACE_ID}/subjektiv/subjects"), json!({"operation_id":"browser-connected-subject","role":"Web connected companion","behavior_md":""})).await["id"].as_str().unwrap().to_owned();
    adopt_chat_defaults(&f.api, &f.owner).await;
    let other_subject = f.post(&format!("/api/w/{TEST_WORKSPACE_ID}/subjektiv/subjects"), json!({"operation_id":"browser-other-subject","role":"Other connected companion","behavior_md":""})).await["id"].as_str().unwrap().to_owned();
    let conversation = f.post(&conversations_path(), json!({"operation_id":format!("web-subject-conversation/v1/{subject}"),"subject_id":subject})).await;
    let c = conversation["id"].as_str().unwrap();
    *conversation_id.lock().unwrap() = c.into();
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
    let wire = serde_json::to_vec(&json!({"origin":f.url,"workspace_id":TEST_WORKSPACE_ID,"subject_id":subject,"other_subject_id":other_subject,"subject_name":"Web connected companion","conversation_id":c,"owner_token":f.owner,"foreign_token":foreign,"owner_session":sessions[0],"foreign_session":sessions[1],"artifact_dir":root.join("target/t855-validation/browser"),"baseline":baseline})).unwrap();
    child.stdin.take().unwrap().write_all(&wire).await.unwrap();
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(150),
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
    if baseline {
        assert!(String::from_utf8_lossy(&output.stdout).contains("baseline captured"));
        return;
    }
    assert!(
        String::from_utf8_lossy(&output.stdout).contains(
            "real messaging Browser -> Backend -> Runtime -> explicit public reply passed"
        )
    );
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        3,
        "same Notify replay must not process twice"
    );
    let (status, public) = account(&f.api, &f.owner, "GET", &messages_path(c), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(public["messages"].as_array().unwrap().len(), 2);
    assert!(!public.to_string().contains("INTERNAL_ASSISTANT"));
}
