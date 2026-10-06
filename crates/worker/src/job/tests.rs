//! Scripted in-process model executions, not provider/process tests.
use super::*;
use agen::llm_client::event::{Event, ResponseStatus, StatusEvent};
use agen::llm_client::{ClientError, Request};
use agen::tool::{Tool, ToolExecutionContext};
use async_trait::async_trait;
use futures::Stream;
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::Mutex;

#[derive(Clone)]
struct ScriptedClient {
    scripts: Arc<Mutex<VecDeque<Vec<Event>>>>,
    requests: Arc<Mutex<Vec<Request>>>,
    pending: bool,
    entered: Arc<tokio::sync::Notify>,
    fail: bool,
}
impl ScriptedClient {
    fn new(scripts: Vec<Vec<Event>>) -> Self {
        Self {
            scripts: Arc::new(Mutex::new(scripts.into())),
            requests: Arc::default(),
            pending: false,
            entered: Arc::default(),
            fail: false,
        }
    }
}
#[async_trait]
impl LlmClient for ScriptedClient {
    fn clone_boxed(&self) -> Box<dyn LlmClient> {
        Box::new(self.clone())
    }
    async fn stream(
        &self,
        request: Request,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<Event, ClientError>> + Send>>, ClientError> {
        self.requests.lock().unwrap().push(request);
        if self.fail && self.scripts.lock().unwrap().is_empty() {
            return Err(ClientError::Config("scripted failure".into()));
        }
        if self.pending && self.scripts.lock().unwrap().is_empty() {
            self.entered.notify_one();
            return Ok(Box::pin(futures::stream::pending()));
        }
        let events = self
            .scripts
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected extra model turn");
        Ok(Box::pin(futures::stream::iter(events.into_iter().map(Ok))))
    }
}
#[derive(Default)]
struct RecordingSink(Mutex<Vec<::job::JobResultSubmission>>);
impl JobResultSink for RecordingSink {
    fn submit(&self, result: ::job::JobResultSubmission) -> Result<(), String> {
        self.0.lock().unwrap().push(result);
        Ok(())
    }
}
fn prose() -> Vec<Event> {
    vec![
        Event::text_block_start(0),
        Event::text_delta(0, "success"),
        Event::text_block_stop(0, None),
        Event::Status(StatusEvent {
            status: ResponseStatus::Completed,
        }),
    ]
}
fn call(name: &str, input: Value) -> Vec<Event> {
    vec![
        Event::tool_use_start(0, "call", name),
        Event::tool_input_delta(0, input.to_string()),
        Event::tool_use_stop(0),
        Event::Status(StatusEvent {
            status: ResponseStatus::Completed,
        }),
    ]
}
fn request(profile: &str) -> ::job::JobRequest {
    ::job::JobRequest {
        job_id: "job-a".into(),
        purpose: "scripted".into(),
        input_revision: "rev-1".into(),
        input_ref: "snapshot:one".into(),
        input: json!({"text":"input"}),
        instruction: "Summarize the input".into(),
        profile: profile.into(),
        serialization_key: None,
        limits: Default::default(),
    }
}
fn artifact(instruction: &str, tokens: u32, turns: u32) -> Value {
    json!({"model":{"scheme":"anthropic", "model_id":"scripted-model", "auth":{"kind":"none"}},
        "engine":{"instruction":instruction, "language":"English", "max_tokens":tokens, "max_turns":turns}})
}
fn registry(root: &Path, a: Value, b: Value) -> manifest::ProfileRegistry {
    std::fs::write(root.join("a.json"), a.to_string()).unwrap();
    std::fs::write(root.join("b.json"), b.to_string()).unwrap();
    let settings = root.join("profiles.toml");
    std::fs::write(
        &settings,
        "default = 'user:a'\n[profile]\na = 'a.json'\nb = 'b.json'\n",
    )
    .unwrap();
    ProfileDiscovery::with_sources(Some(settings), None)
        .discover()
        .unwrap()
}
fn prepared(
    root: &Path,
    request: &::job::JobRequest,
    client: ScriptedClient,
) -> PreparedInternalJob {
    let registry = registry(
        root,
        artifact("default", 512, 3),
        artifact("internal.flow_verifier_system", 100000, 1000),
    );
    prepare_job_from_registry(request, root, Some(Box::new(client)), &registry).unwrap()
}

#[tokio::test]
async fn executes_selected_profiles_with_only_result_authority_and_bounded_policy() {
    let root = tempfile::tempdir().unwrap();
    let registry = registry(
        root.path(),
        artifact("default", 512, 3),
        artifact("internal.flow_verifier_system", 100000, 1000),
    );
    let contract = PromptCatalog::builtins_only()
        .unwrap()
        .render(WorkerPrompt::JobSystem, minijinja::context! {})
        .unwrap();
    let mut prompts = vec![];
    for (profile, tokens) in [
        ("user:a", 512),
        ("user:b", MAX_JOB_TOKENS),
        ("default", 512),
    ] {
        let client = ScriptedClient::new(vec![
            call(SUBMIT_JOB_RESULT_TOOL, json!({"result":{"ok":true}})),
            prose(),
        ]);
        let sink = Arc::new(RecordingSink::default());
        let job = prepare_job_from_registry(
            &request(profile),
            root.path(),
            Some(Box::new(client.clone())),
            &registry,
        )
        .unwrap();
        assert_eq!(
            job.manifest.engine.max_turns.unwrap().get(),
            if profile == "user:b" {
                MAX_JOB_TURNS
            } else {
                3
            }
        );
        let (tx, rx) = tokio::sync::oneshot::channel();
        let result = job
            .run("attempt-7".into(), sink.clone(), move |sender| {
                let _ = tx.send(sender);
            })
            .await
            .unwrap();
        assert!(
            rx.await.unwrap().is_closed(),
            "Worker must be released before returning acceptance"
        );
        assert_eq!(
            result.submission,
            ::job::JobResultSubmission {
                job_id: "job-a".into(),
                attempt_id: "attempt-7".into(),
                input_revision: "rev-1".into(),
                result: json!({"ok":true})
            }
        );
        assert_eq!(sink.0.lock().unwrap().len(), 1);
        let requests = client.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        for request in requests.iter() {
            assert_eq!(
                request
                    .tools
                    .iter()
                    .map(|tool| tool.name.as_str())
                    .collect::<Vec<_>>(),
                [SUBMIT_JOB_RESULT_TOOL]
            );
            assert_eq!(request.config.max_tokens, Some(tokens));
            assert_eq!(
                request.tools[0].input_schema["properties"]
                    .as_object()
                    .unwrap()
                    .keys()
                    .collect::<Vec<_>>(),
                ["result"]
            );
            assert!(request.system_prompt.as_ref().unwrap().ends_with(&contract));
        }
        prompts.push(requests[0].system_prompt.clone());
    }
    assert_ne!(prompts[0], prompts[1]);
    assert_eq!(prompts[0], prompts[2]);
}

#[tokio::test]
async fn selected_builtin_job_executes_with_catalog_contract_and_only_result_tool() {
    let root = tempfile::tempdir().unwrap();
    let registry = ProfileDiscovery::with_sources(None, None)
        .discover()
        .unwrap();
    let request = request("builtin:job");
    let client = ScriptedClient::new(vec![
        call(SUBMIT_JOB_RESULT_TOOL, json!({"result":{"ok":true}})),
        prose(),
    ]);
    let sink = Arc::new(RecordingSink::default());
    let job = prepare_job_from_registry(
        &request,
        root.path(),
        Some(Box::new(client.clone())),
        &registry,
    )
    .unwrap();
    assert_eq!(
        job.manifest.engine.instruction,
        WorkerPrompt::JobSystem.key()
    );
    assert_eq!(
        job.manifest.model.ref_.as_deref(),
        Some("codex-oauth/gpt-5.6-luna")
    );
    assert_eq!(job.manifest.engine.max_turns.unwrap().get(), MAX_JOB_TURNS);
    assert!(job.manifest.scope.allow.is_empty());
    assert!(job.manifest.delegation_scope.allow.is_empty());
    assert!(WorkerPrompt::ALL.contains(&WorkerPrompt::JobSystem));
    let contract = PromptCatalog::builtins_only()
        .unwrap()
        .render(WorkerPrompt::JobSystem, minijinja::context! {})
        .unwrap();
    assert!(job.system_prompt.ends_with(&contract));
    let (tx, rx) = tokio::sync::oneshot::channel();
    let result = job
        .run("builtin-attempt".into(), sink.clone(), move |sender| {
            let _ = tx.send(sender);
        })
        .await
        .unwrap();
    assert!(rx.await.unwrap().is_closed());
    assert_eq!(result.submission.job_id, request.job_id);
    assert_eq!(result.submission.attempt_id, "builtin-attempt");
    assert_eq!(result.submission.input_revision, request.input_revision);
    assert_eq!(result.submission.result, json!({"ok":true}));
    assert_eq!(sink.0.lock().unwrap().len(), 1);
    let requests = client.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    for request in requests.iter() {
        assert_eq!(
            request
                .tools
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            [SUBMIT_JOB_RESULT_TOOL]
        );
        assert_eq!(request.config.max_tokens, Some(MAX_JOB_TOKENS));
        assert!(request.system_prompt.as_ref().unwrap().ends_with(&contract));
    }
}

#[tokio::test]
async fn final_prose_and_ungranted_tool_calls_are_not_success() {
    let root = tempfile::tempdir().unwrap();
    for scripts in [
        vec![prose()],
        vec![
            call("Bash", json!({"command":"touch should-not-exist"})),
            prose(),
        ],
        vec![
            call("SubmitBackendJobResult", json!({"result":{"ok":true}})),
            prose(),
        ],
        vec![
            call(
                SUBMIT_JOB_RESULT_TOOL,
                json!({"job_id":"forged","result":{"ok":true}}),
            ),
            prose(),
        ],
    ] {
        let sink = Arc::new(RecordingSink::default());
        let outcome = prepared(
            root.path(),
            &request("user:a"),
            ScriptedClient::new(scripts),
        )
        .run("attempt".into(), sink.clone(), |_| {})
        .await;
        assert!(
            matches!(outcome, Err(JobExecutionError::MissingResult)),
            "{outcome:?}"
        );
        assert!(sink.0.lock().unwrap().is_empty());
        assert!(!root.path().join("should-not-exist").exists());
    }
}

#[tokio::test]
async fn result_size_and_identity_are_fenced_before_host_submission() {
    let root = tempfile::tempdir().unwrap();
    let mut request = request("user:a");
    request.limits.max_result_bytes = 5;
    let sink = Arc::new(RecordingSink::default());
    let job = prepared(
        root.path(),
        &request,
        ScriptedClient::new(vec![
            call(SUBMIT_JOB_RESULT_TOOL, json!({"result":"too big"})),
            prose(),
        ]),
    );
    assert!(matches!(
        job.run("attempt".into(), sink.clone(), |_| {}).await,
        Err(JobExecutionError::MissingResult)
    ));
    assert!(sink.0.lock().unwrap().is_empty());
}

#[tokio::test]
async fn cancellation_and_errors_return_without_acceptance() {
    let root = tempfile::tempdir().unwrap();
    let mut client = ScriptedClient::new(vec![]);
    client.pending = true;
    let entered = client.entered.clone();
    let job = prepared(root.path(), &request("user:a"), client);
    let sink = Arc::new(RecordingSink::default());
    let (tx, rx) = tokio::sync::oneshot::channel();
    let run = tokio::spawn(async move {
        job.run("attempt".into(), sink, move |sender| {
            let _ = tx.send(sender);
        })
        .await
    });
    let sender = rx.await.unwrap();
    entered.notified().await;
    sender.send(()).await.unwrap();
    assert!(matches!(
        tokio::time::timeout(std::time::Duration::from_secs(2), run)
            .await
            .unwrap()
            .unwrap(),
        Err(JobExecutionError::Cancelled)
    ));
    assert!(
        sender.is_closed(),
        "normal Worker release must close cancellation receiver"
    );
    let mut client = ScriptedClient::new(vec![]);
    client.fail = true;
    let (tx, rx) = tokio::sync::oneshot::channel();
    let outcome = prepared(root.path(), &request("user:a"), client)
        .run(
            "attempt".into(),
            Arc::new(RecordingSink::default()),
            move |sender| {
                let _ = tx.send(sender);
            },
        )
        .await;
    assert!(matches!(outcome, Err(JobExecutionError::Worker(_))));
    assert!(
        rx.await.unwrap().is_closed(),
        "error must release the Worker"
    );
}

#[tokio::test]
async fn accepted_result_survives_later_provider_error_and_cancellation() {
    let root = tempfile::tempdir().unwrap();
    for cancel in [false, true] {
        let mut client =
            ScriptedClient::new(vec![call(SUBMIT_JOB_RESULT_TOOL, json!({"result":true}))]);
        client.pending = cancel;
        client.fail = !cancel;
        let entered = client.entered.clone();
        let sink = Arc::new(RecordingSink::default());
        let job = prepared(root.path(), &request("user:a"), client);
        let (tx, rx) = tokio::sync::oneshot::channel();
        let recording = sink.clone();
        let run = tokio::spawn(async move {
            job.run("attempt".into(), recording, move |sender| {
                let _ = tx.send(sender);
            })
            .await
        });
        let sender = rx.await.unwrap();
        if cancel {
            entered.notified().await;
            assert_eq!(
                sink.0.lock().unwrap().len(),
                1,
                "acceptance must precede cancellation"
            );
            sender.send(()).await.unwrap();
        }
        let result = tokio::time::timeout(std::time::Duration::from_secs(2), run)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(result.submission.result, json!(true));
        assert_eq!(sink.0.lock().unwrap().len(), 1);
        assert!(sender.is_closed());
    }
}

#[tokio::test]
async fn turn_limit_is_bounded_and_accepted_result_survives_later_failure() {
    let root = tempfile::tempdir().unwrap();
    let registry = registry(
        root.path(),
        artifact("default", 512, 1),
        artifact("default", 512, 1),
    );
    for (tool, expected) in [("UnknownTool", false), (SUBMIT_JOB_RESULT_TOOL, true)] {
        let client = ScriptedClient::new(vec![call(tool, json!({"result":true}))]);
        let prepared = prepare_job_from_registry(
            &request("user:a"),
            root.path(),
            Some(Box::new(client.clone())),
            &registry,
        )
        .unwrap();
        let outcome = prepared
            .run("attempt".into(), Arc::new(RecordingSink::default()), |_| {})
            .await;
        assert_eq!(outcome.is_ok(), expected);
        assert_eq!(client.requests.lock().unwrap().len(), 1);
    }
}

#[test]
fn profiles_with_unavailable_requirements_are_rejected_without_name_allowlist_or_fallback() {
    let root = tempfile::tempdir().unwrap();
    let base = artifact("default", 512, 3);
    for (key, value, needle) in [
        ("feature", json!({"task":{"enabled":true}}), "feature.task"),
        (
            "feature",
            json!({"workspace_config":{"enabled":true}}),
            "feature.workspace_config",
        ),
        (
            "feature",
            json!({"subjektiv":{"consolidation_tools":true}}),
            "feature.subjektiv",
        ),
        (
            "feature",
            json!({"memory":{"staging_tools":true}}),
            "feature.memory",
        ),
        ("scope", json!("workspace_read"), "scope"),
        (
            "delegation_scope",
            json!("workspace_write"),
            "delegation_scope",
        ),
        (
            "session",
            json!({"record_event_trace":true}),
            "session.record_event_trace",
        ),
        ("web", json!({"enabled":true}), "web"),
        (
            "compaction",
            json!({"kind":"tokens","threshold":10000}),
            "compaction",
        ),
        ("worker", json!({"mode":"wip"}), "worker.mode=wip"),
        ("skills", json!({"directories":["skills"]}), "skills"),
        (
            "mcp",
            json!({"stdio_server":[{"name":"test","command":"fake"}]}),
            "mcp",
        ),
    ] {
        let mut profile = base.clone();
        profile[key] = value;
        let registry = registry(root.path(), profile, base.clone());
        let error = prepare_job_from_registry(
            &request("user:a"),
            root.path(),
            Some(Box::new(ScriptedClient::new(vec![]))),
            &registry,
        )
        .err()
        .unwrap();
        assert!(
            matches!(error, JobExecutionError::UnsupportedProfile(_)),
            "{error}"
        );
        assert!(error.to_string().contains(needle), "{error}");
    }
    for name in [
        "web",
        "image",
        "sub_worker",
        "flow",
        "worker",
        "workspace_worker_discovery",
        "objective",
        "manage_workdir",
        "ticket",
        "orchestration",
    ] {
        let mut profile = base.clone();
        profile["feature"] = json!({name:{"enabled":true}});
        let registry = registry(root.path(), profile, base.clone());
        assert!(matches!(
            prepare_job_from_registry(
                &request("user:a"),
                root.path(),
                Some(Box::new(ScriptedClient::new(vec![]))),
                &registry
            ),
            Err(JobExecutionError::UnsupportedProfile(_))
        ));
    }
    let registry = registry(root.path(), base.clone(), base);
    for selector in ["user:missing", "project:a", "./a.json", "inherit"] {
        assert!(
            prepare_job_from_registry(
                &request(selector),
                root.path(),
                Some(Box::new(ScriptedClient::new(vec![]))),
                &registry
            )
            .is_err()
        );
    }
    let builtin = request("builtin:backend-job");
    assert!(matches!(
        prepare_job_from_registry(
            &builtin,
            root.path(),
            Some(Box::new(ScriptedClient::new(vec![]))),
            &registry
        ),
        Err(JobExecutionError::UnsupportedProfile(_))
    ));
}

#[test]
fn unknown_prompt_and_model_are_not_substituted_even_with_injected_client() {
    let root = tempfile::tempdir().unwrap();
    let mut unknown_model = artifact("default", 512, 3);
    unknown_model["model"] = json!({"ref":"not-a-provider/not-a-model"});
    let registry = registry(
        root.path(),
        artifact("internal.missing", 512, 3),
        unknown_model,
    );
    for (profile, prompt_error) in [("user:a", true), ("user:b", false)] {
        let error = prepare_job_from_registry(
            &request(profile),
            root.path(),
            Some(Box::new(ScriptedClient::new(vec![]))),
            &registry,
        )
        .err()
        .unwrap();
        if prompt_error {
            assert!(
                matches!(error, JobExecutionError::Instruction(_)),
                "{error}"
            );
        } else {
            assert!(matches!(error, JobExecutionError::Model(_)), "{error}");
        }
    }
}

struct UnknownSink(Mutex<Vec<::job::JobResultSubmission>>);
impl JobResultSink for UnknownSink {
    fn submit(&self, value: ::job::JobResultSubmission) -> Result<(), String> {
        self.0.lock().unwrap().push(value);
        Err("unknown".into())
    }
}

#[tokio::test]
async fn unconfirmed_sink_acceptance_is_not_classified_as_missing_result_or_known_failure() {
    let root = tempfile::tempdir().unwrap();
    let sink = Arc::new(UnknownSink(Mutex::new(vec![])));
    let job = prepared(
        root.path(),
        &request("user:a"),
        ScriptedClient::new(vec![
            call(SUBMIT_JOB_RESULT_TOOL, json!({"result":true})),
            prose(),
        ]),
    );
    assert!(matches!(
        job.run("attempt".into(), sink.clone(), |_| {}).await,
        Err(JobExecutionError::ResultUnconfirmed(_))
    ));
    assert_eq!(sink.0.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn feature_registration_rejects_forged_fences_seals_unknown_outcomes_and_honors_cancel() {
    let sink = Arc::new(UnknownSink(Mutex::new(vec![])));
    let feature =
        JobResultFeature::new(&request("user:a"), "attempt".into(), sink.clone()).unwrap();
    let mut pending = vec![];
    let mut hooks = crate::HookRegistryBuilder::new();
    let report = FeatureRegistryBuilder::new()
        .with_module(feature.clone())
        .install_into_pending(&mut pending, &mut hooks);
    assert_eq!(report.installed_tool_names(), [SUBMIT_JOB_RESULT_TOOL]);
    let (_, tool) = pending.remove(0)();
    assert!(
        tool.execute(
            r#"{"attempt_id":"forged","result":true}"#,
            ToolExecutionContext::direct()
        )
        .await
        .is_err()
    );
    assert!(sink.0.lock().unwrap().is_empty());
    assert!(
        tool.execute(r#"{"result":true}"#, ToolExecutionContext::direct())
            .await
            .is_err()
    );
    assert!(
        tool.execute(r#"{"result":false}"#, ToolExecutionContext::direct())
            .await
            .is_err()
    );
    assert!(
        tool.execute(r#"{"result":true}"#, ToolExecutionContext::direct())
            .await
            .is_err()
    );
    assert_eq!(sink.0.lock().unwrap().len(), 2);
    let context = ToolExecutionContext::new("cancelled", "batch", 0);
    feature.cancel_execution(&context).await.unwrap();
    assert!(matches!(
        tool.execute(r#"{"result":true}"#, context).await,
        Err(agen::tool::ToolError::Cancelled(_))
    ));
    assert_eq!(sink.0.lock().unwrap().len(), 2);
}
