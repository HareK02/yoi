//! Installed host-feature execution, not lexical slash-command dispatch.
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use agen::Engine;
use agen::llm_client::event::{Event as LlmEvent, ResponseStatus, StatusEvent};
use agen::llm_client::{ClientError, LlmClient, Request};
use async_trait::async_trait;
use futures::Stream;
use protocol::{
    FeatureInvocation, FeatureInvocationDescriptor, FeatureInvocationIdentity,
    FeatureInvocationResult, FeatureInvocationStatus, FeatureInvocationSyntax,
    InvocationArgumentDescriptor, InvocationArgumentType, InvocationArgumentValue,
    InvocationCompletion, InvocationValue, Segment,
};
use session_store::{CombinedStore, FsStore, FsWorkerStore, LogEntry, SystemItem};
use worker::feature::{
    FeatureDescriptor, FeatureInstallContext, FeatureInstallError, FeatureInvocationContext,
    FeatureInvocationHandler, FeatureInvocationHandlerError, FeatureModule, FeatureRegistryBuilder,
};
use worker::{Worker, WorkerFilesystemAuthority, WorkerManifest, WorkerWorkspaceContext};

#[derive(Clone)]
struct Trace {
    calls: Arc<Mutex<Vec<String>>>,
    requests: Arc<Mutex<Vec<Request>>>,
    available: Arc<AtomicBool>,
}

impl Default for Trace {
    fn default() -> Self {
        Self {
            calls: Arc::default(),
            requests: Arc::default(),
            available: Arc::new(AtomicBool::new(true)),
        }
    }
}

#[async_trait]
impl LlmClient for Trace {
    fn clone_boxed(&self) -> Box<dyn LlmClient> {
        Box::new(self.clone())
    }
    async fn stream(
        &self,
        request: Request,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<LlmEvent, ClientError>> + Send>>, ClientError>
    {
        self.calls.lock().unwrap().push("llm".into());
        self.requests.lock().unwrap().push(request);
        Ok(Box::pin(futures::stream::iter(vec![
            Ok(LlmEvent::text_block_start(0)),
            Ok(LlmEvent::text_delta(0, "done")),
            Ok(LlmEvent::text_block_stop(0, None)),
            Ok(LlmEvent::Status(StatusEvent {
                status: ResponseStatus::Completed,
            })),
        ])))
    }
}

fn descriptor() -> FeatureInvocationDescriptor {
    FeatureInvocationDescriptor {
        identity: FeatureInvocationIdentity("builtin:fake/prepare".into()),
        name: "prepare".into(),
        aliases: vec!["prep".into()],
        display_name: "Prepare".into(),
        description: "Prepare test context".into(),
        syntax: FeatureInvocationSyntax::Parenthesized,
        arguments: vec![InvocationArgumentDescriptor {
            name: "value".into(),
            position: Some(0),
            required: true,
            value_type: InvocationArgumentType::String,
            completion: InvocationCompletion::None,
            description: None,
        }],
        client_adapter: None,
    }
}

#[derive(Clone)]
struct FakeFeature(Trace);
impl FeatureModule for FakeFeature {
    fn descriptor(&self) -> FeatureDescriptor {
        FeatureDescriptor::builtin("fake", "Fake").with_chat_invocation(descriptor())
    }
    fn install(&self, context: &mut FeatureInstallContext<'_>) -> Result<(), FeatureInstallError> {
        context
            .chat_invocations()
            .register(descriptor(), self.clone())
    }
}

#[async_trait]
impl FeatureInvocationHandler for FakeFeature {
    fn is_available(&self) -> bool {
        self.0.available.load(Ordering::SeqCst)
    }

    fn validate(
        &self,
        invocation: &FeatureInvocation,
    ) -> Result<(), FeatureInvocationHandlerError> {
        if invocation.arguments[0].value == InvocationValue::String("denied".into()) {
            return Err(FeatureInvocationHandlerError::failed(
                "host permission denied",
            ));
        }
        Ok(())
    }

    async fn invoke(
        &self,
        context: FeatureInvocationContext,
        invocation: &FeatureInvocation,
    ) -> Result<FeatureInvocationResult, FeatureInvocationHandlerError> {
        assert_eq!(context.invocation_id, invocation.invocation_id);
        let InvocationValue::String(value) = &invocation.arguments[0].value else {
            panic!("validated string")
        };
        self.0.calls.lock().unwrap().push(value.clone());
        match value.as_str() {
            "fail" => {
                return Err(FeatureInvocationHandlerError::failed(
                    "test preparation failed",
                ));
            }
            "unknown" => {
                return Err(FeatureInvocationHandlerError::outcome_unknown(
                    "test effect uncertain",
                ));
            }
            "revoke-during-effect" => self.0.available.store(false, Ordering::SeqCst),
            "large-error" => return Err(FeatureInvocationHandlerError::failed("x".repeat(4097))),
            _ => {}
        }
        Ok(FeatureInvocationResult {
            invocation_id: if value == "wrong" {
                "wrong-id".into()
            } else {
                invocation.invocation_id.clone()
            },
            identity: invocation.identity.clone(),
            status: FeatureInvocationStatus::Succeeded,
            message: match value.as_str() {
                "large-message" => "é".repeat(2049),
                "limit-message" => "x".repeat(4096),
                _ => "prepared".into(),
            },
            context: Some(match value.as_str() {
                "large-context" => "é".repeat(8193),
                "limit-context" => "x".repeat(16384),
                _ => format!("trusted-preparation-{value}"),
            }),
        })
    }
}

fn invoke(id: &str, value: &str) -> Segment {
    Segment::FeatureInvoke {
        invocation: FeatureInvocation {
            invocation_id: id.into(),
            identity: descriptor().identity,
            name: "prepare".into(),
            arguments: vec![InvocationArgumentValue {
                name: "value".into(),
                value: InvocationValue::String(value.into()),
            }],
        },
    }
}

type TestStore = CombinedStore<FsStore, FsWorkerStore>;
async fn fixture(trace: Trace) -> (tempfile::TempDir, Worker<Trace, TestStore>) {
    let dir = tempfile::tempdir().unwrap();
    let manifest = WorkerManifest::from_toml(
        r#"
[worker]
name = "invocation-test"
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
    let store = CombinedStore::new(
        FsStore::new(dir.path().join("sessions")).unwrap(),
        FsWorkerStore::new(dir.path().join("workers")).unwrap(),
    );
    let mut worker = Worker::new(
        manifest,
        Engine::<_, agen::state::Mutable, worker::SessionHistoryMetadata>::new_annotated(
            trace.clone(),
        ),
        store,
        WorkerWorkspaceContext::no_workspace(),
        WorkerFilesystemAuthority::None,
        manifest::Scope::empty(),
    )
    .await
    .unwrap();
    let report =
        worker.install_features(FeatureRegistryBuilder::new().with_module(FakeFeature(trace)));
    assert!(!report.has_errors(), "{report:?}");
    (dir, worker)
}

fn results(worker: &Worker<Trace, TestStore>) -> Vec<FeatureInvocationResult> {
    worker
        .sink()
        .subscribe_with_snapshot()
        .0
        .into_iter()
        .filter_map(|entry| match entry {
            LogEntry::AnnotatedSystemItem { entry, .. } => match entry.item {
                SystemItem::FeatureInvocationResult { result, .. } => Some(result),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

#[derive(Clone)]
struct TracePromptHook {
    trace: Trace,
    cancel: bool,
}
#[async_trait]
impl worker::hook::Hook<worker::hook::OnPromptSubmit> for TracePromptHook {
    async fn call(
        &self,
        _input: &worker::hook::PromptSubmitInfo,
    ) -> Result<worker::hook::HookPromptAction, worker::hook::HookError> {
        self.trace.calls.lock().unwrap().push("prompt-hook".into());
        Ok(if self.cancel {
            worker::hook::HookPromptAction::Cancel("test cancellation".into())
        } else {
            worker::hook::HookPromptAction::Continue
        })
    }
}

#[tokio::test]
async fn installed_feature_invocations_execute_in_order_before_model_and_context_is_visible() {
    let trace = Trace::default();
    let (_dir, mut worker) = fixture(trace.clone()).await;
    worker.add_on_prompt_submit_hook(TracePromptHook {
        trace: trace.clone(),
        cancel: false,
    });
    worker
        .run(vec![
            invoke("one", "first"),
            Segment::text("explain"),
            invoke("two", "second"),
        ])
        .await
        .unwrap();
    assert_eq!(
        *trace.calls.lock().unwrap(),
        ["first", "second", "prompt-hook", "llm"]
    );
    let requests = trace.requests.lock().unwrap();
    let request = format!("{:?}", requests[0]);
    assert!(request.contains("trusted-preparation-first"));
    assert!(request.contains("trusted-preparation-second"));
    assert!(request.find("trusted-preparation-first") < request.find("trusted-preparation-second"));
    assert_eq!(results(&worker).len(), 2);
    // Preparation results must be model-visible in the same order as replay.
    let history = worker.history();
    let first = history
        .iter()
        .position(|item| {
            item.as_text()
                .is_some_and(|text| text.contains("trusted-preparation-first"))
        })
        .unwrap();
    let second = history
        .iter()
        .position(|item| {
            item.as_text()
                .is_some_and(|text| text.contains("trusted-preparation-second"))
        })
        .unwrap();
    assert!(first < second);
}

#[tokio::test]
async fn feature_failure_fences_later_invocations_model_resume_and_notifications() {
    for value in ["fail", "unknown", "wrong"] {
        let trace = Trace::default();
        let (_dir, mut worker) = fixture(trace.clone()).await;
        assert!(
            worker
                .run(vec![
                    invoke("one", "first"),
                    invoke("two", value),
                    invoke("three", "never")
                ])
                .await
                .is_err()
        );
        assert_eq!(*trace.calls.lock().unwrap(), ["first", value]);
        let results = results(&worker);
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].status, FeatureInvocationStatus::Succeeded);
        assert_eq!(
            results[1].status,
            if value == "fail" {
                FeatureInvocationStatus::Failed
            } else {
                FeatureInvocationStatus::OutcomeUnknown
            }
        );
        assert_eq!(results[1].invocation_id, "two");
        assert!(results[1].context.is_none());
        assert_eq!(results[2].status, FeatureInvocationStatus::Failed);
        assert!(worker.resume().await.is_err());
        assert!(
            worker
                .run_for_notification(protocol::InvokeKind::Notify)
                .await
                .is_err()
        );
        assert!(trace.requests.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn feature_replay_reuses_result_and_rejects_id_payload_changes() {
    let trace = Trace::default();
    let (_dir, mut worker) = fixture(trace.clone()).await;
    worker.run(vec![invoke("same", "first")]).await.unwrap();
    worker.run(vec![invoke("same", "first")]).await.unwrap();
    assert_eq!(*trace.calls.lock().unwrap(), ["first", "llm", "llm"]);
    assert!(worker.run(vec![invoke("same", "different")]).await.is_err());
    assert_eq!(*trace.calls.lock().unwrap(), ["first", "llm", "llm"]);
    assert_eq!(
        results(&worker).last().unwrap().status,
        FeatureInvocationStatus::Failed
    );
}

#[tokio::test]
async fn feature_success_replay_does_not_resurrect_disabled_installed_capability() {
    let trace = Trace::default();
    let (_dir, mut worker) = fixture(trace.clone()).await;
    worker.run(vec![invoke("same", "first")]).await.unwrap();
    assert!(
        !worker
            .install_features(FeatureRegistryBuilder::new())
            .has_errors()
    );
    assert!(
        worker
            .feature_invocations()
            .feature_completions("")
            .is_empty()
    );
    assert!(worker.run(vec![invoke("same", "first")]).await.is_err());
    assert_eq!(*trace.calls.lock().unwrap(), ["first", "llm"]);
    assert_eq!(
        results(&worker).last().unwrap().status,
        FeatureInvocationStatus::Failed
    );
}

#[tokio::test]
async fn feature_invocation_text_is_not_executable_and_duplicate_ids_are_rejected_before_effects() {
    let trace = Trace::default();
    let (_dir, mut worker) = fixture(trace.clone()).await;
    worker
        .run(vec![Segment::text("/prepare(first) quoted /prep(fail)")])
        .await
        .unwrap();
    assert_eq!(*trace.calls.lock().unwrap(), ["llm"]);
    assert!(results(&worker).is_empty());
    assert!(
        worker
            .run(vec![invoke("same", "first"), invoke("same", "second")])
            .await
            .is_err()
    );
    assert_eq!(*trace.calls.lock().unwrap(), ["llm"]);
}

#[tokio::test]
async fn feature_handler_validation_rejects_semantics_and_forged_authority_before_effects() {
    let trace = Trace::default();
    let (_dir, mut worker) = fixture(trace.clone()).await;
    assert!(worker.run(vec![invoke("denied", "denied")]).await.is_err());
    let mut forged = invoke("forged", "first");
    if let Segment::FeatureInvoke { invocation } = &mut forged {
        invocation.arguments.push(InvocationArgumentValue {
            name: "workspace_id".into(),
            value: InvocationValue::String("foreign".into()),
        });
    }
    assert!(worker.run(vec![forged]).await.is_err());
    assert!(trace.calls.lock().unwrap().is_empty());
    assert_eq!(results(&worker).len(), 2);
    assert!(
        results(&worker)
            .iter()
            .all(|result| result.status == FeatureInvocationStatus::Failed)
    );
}

#[tokio::test]
async fn feature_prompt_hook_cancellation_does_not_rollback_durable_business_preparation() {
    let trace = Trace::default();
    let (_dir, mut worker) = fixture(trace.clone()).await;
    worker.add_on_prompt_submit_hook(TracePromptHook {
        trace: trace.clone(),
        cancel: true,
    });
    let _ = worker.run(vec![invoke("one", "first")]).await;
    assert_eq!(*trace.calls.lock().unwrap(), ["first", "prompt-hook"]);
    assert_eq!(results(&worker).len(), 1);
    assert!(worker.history().iter().any(|item| {
        item.as_text()
            .is_some_and(|text| text.contains("trusted-preparation-first"))
    }));
    let _ = worker.run(vec![invoke("one", "first")]).await;
    assert_eq!(
        *trace.calls.lock().unwrap(),
        ["first", "prompt-hook", "prompt-hook"]
    );
}

#[tokio::test]
async fn feature_unknown_input_rejected_before_effects_commit_or_model() {
    let trace = Trace::default();
    let (_dir, mut worker) = fixture(trace.clone()).await;
    let unknown: Segment = serde_json::from_value(serde_json::json!({
        "kind": "future_side_effect", "args": { "target": "important" }
    }))
    .unwrap();
    assert!(matches!(unknown, Segment::Unknown));
    let before = worker.sink().subscribe_with_snapshot().0.len();
    assert!(
        worker
            .run(vec![
                invoke("one", "first"),
                unknown,
                Segment::text("explain")
            ])
            .await
            .is_err()
    );
    assert_eq!(worker.sink().subscribe_with_snapshot().0.len(), before);
    assert!(trace.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn feature_oversized_results_become_bounded_unknown_and_do_not_repeat_effects() {
    for value in ["large-message", "large-context", "large-error"] {
        let trace = Trace::default();
        let (_dir, mut worker) = fixture(trace.clone()).await;
        assert!(
            worker
                .run(vec![invoke("large", value), invoke("later", "never")])
                .await
                .is_err()
        );
        assert_eq!(*trace.calls.lock().unwrap(), [value]);
        for result in results(&worker) {
            assert!(result.message.len() <= 4096);
            assert!(result.context.is_none());
        }
        assert_eq!(
            results(&worker)[0].status,
            FeatureInvocationStatus::OutcomeUnknown
        );
        assert!(worker.run(vec![invoke("large", value)]).await.is_err());
        assert!(worker.resume().await.is_err());
        assert_eq!(*trace.calls.lock().unwrap(), [value]);
    }
    for value in ["limit-message", "limit-context"] {
        let trace = Trace::default();
        let (_dir, mut worker) = fixture(trace.clone()).await;
        worker.run(vec![invoke("limit", value)]).await.unwrap();
        worker.run(vec![invoke("limit", value)]).await.unwrap();
        assert_eq!(*trace.calls.lock().unwrap(), [value, "llm", "llm"]);
    }
}

#[tokio::test]
async fn feature_dynamic_permission_revocation_hides_discovery_and_fences_execution_and_replay() {
    let trace = Trace::default();
    let (_dir, mut worker) = fixture(trace.clone()).await;
    let registry = worker.feature_invocations();
    assert_eq!(registry.descriptors().len(), 1);
    worker.run(vec![invoke("same", "first")]).await.unwrap();
    trace.available.store(false, Ordering::SeqCst);
    assert!(registry.descriptors().is_empty());
    assert!(registry.descriptor(&descriptor().identity).is_none());
    assert!(registry.feature_completions("").is_empty());
    assert!(
        registry
            .argument_name_completions(&descriptor().identity, "")
            .is_empty()
    );
    assert!(
        registry
            .argument_completions(&descriptor().identity, "value", "")
            .await
            .is_empty()
    );
    assert!(worker.run(vec![invoke("same", "first")]).await.is_err());
    assert!(worker.run(vec![invoke("new", "second")]).await.is_err());
    assert!(worker.resume().await.is_err());
    assert_eq!(*trace.calls.lock().unwrap(), ["first", "llm"]);
    trace.available.store(true, Ordering::SeqCst);
    assert_eq!(registry.descriptors().len(), 1);
    worker.run(vec![invoke("same", "first")]).await.unwrap();
    assert_eq!(*trace.calls.lock().unwrap(), ["first", "llm", "llm"]);
}

#[tokio::test]
async fn feature_revocation_during_effect_withholds_preparation_without_retrying_committed_effect()
{
    let trace = Trace::default();
    let (_dir, mut worker) = fixture(trace.clone()).await;
    assert!(
        worker
            .run(vec![invoke("same", "revoke-during-effect")])
            .await
            .is_err()
    );
    assert_eq!(*trace.calls.lock().unwrap(), ["revoke-during-effect"]);
    let result = &results(&worker)[0];
    assert_eq!(result.status, FeatureInvocationStatus::Failed);
    assert!(result.context.is_none());
    assert!(result.message.contains("reported success"));
    trace.available.store(true, Ordering::SeqCst);
    assert!(
        worker
            .run(vec![invoke("same", "revoke-during-effect")])
            .await
            .is_err()
    );
    assert_eq!(*trace.calls.lock().unwrap(), ["revoke-during-effect"]);
}

#[test]
fn notify_rejects_typed_side_effect_message_payloads() {
    let methods = [
        protocol::Method::Notify {
            notification_request_id: "n-1".into(),
            message: "/prepare(first)".into(),
        },
        protocol::Method::NotifyTracked {
            notification_request_id: "n-1".into(),
            message: "/prepare(first)".into(),
            source: protocol::AuthenticatedInputSource::UntrustedWire,
        },
    ];
    for method in methods {
        // Notify accepts an advisory string, not typed input or invocation
        // objects. Ordinary text spelling an invocation remains advisory text.
        let mut request = serde_json::to_value(method).unwrap();
        assert!(serde_json::from_value::<protocol::Method>(request.clone()).is_ok());
        let mut with_input = request.clone();
        with_input["params"]["input"] = serde_json::json!([invoke("one", "first")]);
        assert!(
            serde_json::from_value::<protocol::Method>(with_input).is_err(),
            "Notify must reject typed input rather than silently discard it"
        );
        request["params"]["message"] = serde_json::json!([invoke("one", "first")]);
        assert!(serde_json::from_value::<protocol::Method>(request).is_err());
    }
}
