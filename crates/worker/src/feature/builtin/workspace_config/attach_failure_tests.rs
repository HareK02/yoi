//! Attach consumer integration using the parent router only at WorkspaceClient transport.
//! Real Feature/Worker receipts, FsStore, native WIP, common codec and Tool history are connected.
//! Backend ledger persistence and canonical authorization are tested by the Server suites.
use super::*;
use failure::{ConfigFailureDetails, ConfigFailureOperation, ConfigFailureReason};
use protocol::{FeatureInvocationResult, FeatureInvocationStatus, Segment};
use session_store::{FsStore, LogEntry, Store};

#[derive(Clone)]
struct NoModel;
#[async_trait::async_trait]
impl agen::llm_client::LlmClient for NoModel {
    fn clone_boxed(&self) -> Box<dyn agen::llm_client::LlmClient> {
        Box::new(self.clone())
    }
    async fn stream(
        &self,
        _: agen::llm_client::Request,
    ) -> Result<
        std::pin::Pin<
            Box<
                dyn futures::Stream<
                        Item = Result<
                            agen::llm_client::event::Event,
                            agen::llm_client::ClientError,
                        >,
                    > + Send,
            >,
        >,
        agen::llm_client::ClientError,
    > {
        panic!("failed attachment must not reach the dependent model")
    }
}

fn saved_results(entries: Vec<LogEntry>) -> Vec<FeatureInvocationResult> {
    entries
        .into_iter()
        .filter_map(|entry| {
            let LogEntry::AnnotatedSystemItem { entry, .. } = entry else {
                return None;
            };
            entry
                .metadata
                .input_metadata
                .as_ref()?
                .feature_payload()?
                .decode::<crate::feature::invocation::InvocationResultInput>(
                    "feature:invocation-result",
                    1,
                )
                .ok()
                .map(|input| input.result)
        })
        .collect()
}
fn cause(attachment: &protocol::startup_failure::StartupFailureAttachment) -> ConfigFailureDetails {
    let error_contract::DecodeFailure::Known(cause) = attachment.decode::<ConfigFailureDetails>()
    else {
        panic!("config attach lost typed cause")
    };
    *cause.facts().cause()
}
fn attach_posts(client: &Router) -> usize {
    client
        .state
        .lock()
        .unwrap()
        .requests
        .iter()
        .filter(|r| {
            r.method == WorkspaceRequestMethod::Post && r.path.ends_with("/workspace-config")
        })
        .count()
}

#[tokio::test]
async fn config_attach_unknown_is_durable_replay_reprojects_current_authority_without_resending() {
    for fault in [AttachFault::ReplyLost, AttachFault::AliasMismatch] {
        let dir = tempfile::tempdir().unwrap();
        let client = Router::new();
        client.state.lock().unwrap().attach_fault = Some(fault);
        let feature = WorkspaceConfigFeature::for_workspace(client.clone(), true);
        let manifest = crate::WorkerManifest::from_toml(
            r#"
[worker]
name = "config-attach-result"
[model]
scheme = "anthropic"
model_id = "test-model"
[engine]
max_tokens = 100
[scope]
allow = []
"#,
        )
        .unwrap();
        let store = FsStore::new(dir.path().join("sessions")).unwrap();
        let mut worker = crate::Worker::new(
            manifest,
            agen::Engine::<_, agen::state::Mutable, crate::SessionHistoryMetadata>::new_annotated(
                NoModel,
            ),
            store.clone(),
            crate::WorkerWorkspaceContext::no_workspace(),
            crate::WorkerFilesystemAuthority::None,
            manifest::Scope::empty(),
        )
        .await
        .unwrap();
        assert!(
            !worker
                .install_features(FeatureRegistryBuilder::new().with_module(feature.clone()))
                .has_errors()
        );
        let invocation = invocation("attach-lost");
        let input = vec![Segment::FeatureInvoke {
            invocation: invocation.clone(),
        }];
        assert!(worker.run(input.clone()).await.is_err());
        let original = saved_results(
            store
                .read_all(worker.session_id(), worker.segment_id())
                .unwrap(),
        )
        .pop()
        .unwrap();
        assert_eq!(original.status, FeatureInvocationStatus::OutcomeUnknown);
        assert!(original.context.is_none());
        let expected_reason = match fault {
            AttachFault::ReplyLost => ConfigFailureReason::Communication,
            AttachFault::AliasMismatch => ConfigFailureReason::Identity,
        };
        let details = cause(original.failure.as_ref().unwrap());
        assert_eq!(details.operation, ConfigFailureOperation::Attach);
        assert_eq!(details.reason, expected_reason);
        assert!(
            !serde_json::to_string(&original)
                .unwrap()
                .contains("secret-token")
        );
        assert!(
            !serde_json::to_string(&original)
                .unwrap()
                .contains("/private/host")
        );
        assert_eq!(attach_posts(&client), 1);

        // Reopen the real store, not a serialization-only surrogate. A loaded
        // audience label/explanation is not permission for another disclosure.
        let reopened = FsStore::new(dir.path().join("sessions")).unwrap();
        let mut loaded = saved_results(
            reopened
                .read_all(worker.session_id(), worker.segment_id())
                .unwrap(),
        )
        .pop()
        .unwrap();
        assert_eq!(cause(loaded.failure.as_ref().unwrap()), details);
        let mut raw = serde_json::to_value(loaded.failure.as_ref().unwrap())
            .unwrap()
            .as_str()
            .unwrap()
            .to_owned();
        let mut value: Json = serde_json::from_str(&raw).unwrap();
        value["explanation"] = json!("secret-token /private/host forged explanation");
        raw = serde_json::to_string(&value).unwrap();
        loaded.failure = Some(serde_json::from_value(json!(raw)).unwrap());
        loaded.message = "forged historical explanation".into();
        worker
            .feature_invocations()
            .redisclose_failure(&invocation, &mut loaded);
        assert_eq!(loaded.status, FeatureInvocationStatus::OutcomeUnknown);
        assert_eq!(cause(loaded.failure.as_ref().unwrap()), details);
        assert!(!loaded.message.contains("forged"));
        for invalid in [
            "{secret-token",
            "{\"version\":999,\"secret\":\"secret-token\"}",
        ] {
            loaded.failure = Some(serde_json::from_value(json!(invalid)).unwrap());
            worker
                .feature_invocations()
                .redisclose_failure(&invocation, &mut loaded);
            assert!(loaded.failure.is_none());
            assert_eq!(loaded.status, FeatureInvocationStatus::OutcomeUnknown);
            assert!(!loaded.message.contains("secret-token"));
        }
        assert!(worker.run(input.clone()).await.is_err());
        assert_eq!(attach_posts(&client), 1);
        let current = backend::WorkspaceConfigBackend::new(client.clone())
            .current_async()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(current.alias, ATTACHMENT_ALIAS);
        assert_eq!(current.connection_id, "connection-1");
        client.state.lock().unwrap().operation_denied = true;
        assert!(worker.run(input.clone()).await.is_err());
        let withheld = saved_results(
            store
                .read_all(worker.session_id(), worker.segment_id())
                .unwrap(),
        )
        .pop()
        .unwrap();
        assert_eq!(withheld.status, FeatureInvocationStatus::OutcomeUnknown);
        assert!(withheld.failure.is_none() && withheld.context.is_none());
        client.state.lock().unwrap().operation_denied = false;
        assert!(worker.run(input).await.is_err());
        let reauthorized = saved_results(
            store
                .read_all(worker.session_id(), worker.segment_id())
                .unwrap(),
        )
        .pop()
        .unwrap();
        assert_eq!(cause(reauthorized.failure.as_ref().unwrap()), details);
        assert_eq!(attach_posts(&client), 1);
        assert_eq!(client.state.lock().unwrap().next_connection, 1);
    }
}

#[tokio::test]
async fn config_attach_native_unknown_keeps_same_cause_in_reopened_tool_history_and_current_alias()
{
    for fault in [AttachFault::ReplyLost, AttachFault::AliasMismatch] {
        let client = Router::new();
        client.state.lock().unwrap().attach_fault = Some(fault);
        let (_, runtime) = runtime(client.clone());
        observe_interface(&runtime, CONTENT_ROOT, false).await;
        let error = runtime
            .call(
                CONTENT_ROOT.into(),
                interface(CONTENT_ROOT, false),
                "attach".into(),
                json!({}),
                ToolExecutionContext::direct(),
            )
            .await
            .unwrap_err();
        let history = crate::test_support::reopen_result(agen::tool::ToolResult::from_error(
            "config-attach",
            error,
        ));
        let agen::Item::ToolResult {
            summary,
            content: Some(content),
            is_error: true,
            ..
        } = history
        else {
            panic!("native attach cause lost")
        };
        let report: Json = serde_json::from_str(&content).unwrap();
        let error_contract::DecodeFailure::Known(cause) =
            error_contract::decode_failure::<ConfigFailureDetails>(
                &serde_json::to_vec(&report["failure"]).unwrap(),
                error_contract::Budget::new(65536, 65536),
            )
        else {
            panic!("cause lost")
        };
        assert_eq!(
            cause.facts().cause().operation,
            ConfigFailureOperation::Attach
        );
        assert_eq!(
            cause.facts().cause().reason,
            match fault {
                AttachFault::ReplyLost => ConfigFailureReason::Communication,
                AttachFault::AliasMismatch => ConfigFailureReason::Identity,
            }
        );
        assert_eq!(report["result"]["kind"], "outcome_unknown");
        assert!(summary.contains("same alias") && summary.contains("do not resend"));
        assert!(!content.contains("secret-token") && !content.contains("/private/host"));
        assert_eq!(
            runtime.audit().last().unwrap().outcome,
            crate::wip::WipAuditOutcome::OutcomeUnknown
        );
        let attachment = backend::WorkspaceConfigBackend::new(client.clone())
            .current_async()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(attachment.alias, ATTACHMENT_ALIAS);
        assert_eq!(attachment.connection_id, "connection-1");
        assert_eq!(attach_posts(&client), 1);
        assert_eq!(client.state.lock().unwrap().next_connection, 1);
    }
}
