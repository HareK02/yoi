//! T-709: real standalone Hosts and test-owned processes, with scripted models.
//! These tests do not create a Workspace or contact a model provider.

use std::collections::{BTreeMap, VecDeque};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use agen::llm_client::{
    LlmClient,
    error::ClientError,
    event::{Event as LlmEvent, StopReason},
    types::{Item, Request},
};
use async_trait::async_trait;
use feature_storage::FeatureStorage;
use futures::{Stream, stream};
use manifest::{ProfileRegistrySource, ProfileSelector};
use protocol::{Event, Method};
use serde_json::{Value, json};
use standalone::subjektiv::StandaloneSubjects;
use standalone::{
    ResolvedStandaloneLaunch, StandaloneHost, StandaloneLaunchConfig, StandaloneLaunchError,
    StandaloneStartupError, StandaloneWorkerStore,
};
use subjektiv::{SubjectStagingRecord, SubjektivStore};

#[derive(Clone)]
struct ScriptedClient {
    responses: Arc<Mutex<VecDeque<Vec<LlmEvent>>>>,
    requests: Arc<Mutex<Vec<Request>>>,
    consolidate: bool,
    extract: bool,
    commit_guard: Option<(PathBuf, String)>,
    fail_after_apply: bool,
    job_ids: Arc<Mutex<Vec<String>>>,
    job_started: Arc<tokio::sync::Notify>,
}

impl ScriptedClient {
    fn new(responses: Vec<Vec<LlmEvent>>) -> Self {
        Self {
            responses: Arc::new(Mutex::new(responses.into())),
            requests: Arc::new(Mutex::new(Vec::new())),
            consolidate: false,
            extract: false,
            commit_guard: None,
            fail_after_apply: false,
            job_ids: Arc::new(Mutex::new(Vec::new())),
            job_started: Arc::new(tokio::sync::Notify::new()),
        }
    }

    fn consolidating(responses: Vec<Vec<LlmEvent>>, fail_after_apply: bool) -> Self {
        Self {
            consolidate: true,
            fail_after_apply,
            ..Self::new(responses)
        }
    }

    async fn first_job(&self) -> String {
        tokio::time::timeout(Duration::from_secs(30), self.job_started.notified())
            .await
            .expect("lifecycle must auto-start a generic Job");
        self.job_ids.lock().unwrap()[0].clone()
    }

    fn job_response(&self, request: &Request) -> Result<Vec<LlmEvent>, ClientError> {
        assert!(
            request
                .system_prompt
                .as_deref()
                .unwrap()
                .contains("# Memory staging consolidater"),
            "generic Job must render the instruction of its selected consolidation Profile"
        );
        for forbidden in [
            "Read",
            "Bash",
            "SubWorkerSpawn",
            "SubjektivMemoryRemember",
            "SubjektivSessionRead",
        ] {
            assert!(
                !request.tools.iter().any(|tool| tool.name == forbidden),
                "Job received {forbidden}"
            );
        }
        let envelope = request
            .items
            .iter()
            .filter_map(Item::as_text)
            .find(|text| text.contains("input_json: "))
            .expect("immutable Job envelope");
        let input: Value = serde_json::from_str(
            envelope
                .lines()
                .find_map(|line| line.strip_prefix("input_json: "))
                .unwrap(),
        )
        .unwrap();
        let id = envelope
            .lines()
            .find_map(|line| line.strip_prefix("job_id: "))
            .unwrap()
            .to_owned();
        let mut ids = self.job_ids.lock().unwrap();
        if !ids.contains(&id) {
            ids.push(id);
            self.job_started.notify_one();
        }
        drop(ids);
        let result = |call: &str| {
            request.items.iter().find_map(|item| match item {
                Item::ToolResult {
                    call_id,
                    content,
                    is_error,
                    summary,
                    ..
                } if call_id == call => {
                    assert!(!is_error, "{call}: {summary}; {content:?}");
                    Some(content.as_deref().expect("structured domain result"))
                }
                _ => None,
            })
        };
        if let Some(Item::ToolResult {
            is_error,
            summary,
            content,
            ..
        }) = request.items.iter().find(
            |item| matches!(item, Item::ToolResult { call_id, .. } if call_id == "job-result"),
        ) {
            assert!(
                !is_error,
                "structured result rejected: {summary}; {content:?}"
            );
            assert_eq!(summary, "Host Job result accepted");
            // SubmitJobResult intentionally returns summary-only. Durable
            // structured acceptance is asserted against host.jobs().wait().
            return Ok(prose());
        }
        if let Some(applied) = result("apply") {
            let response: server_api::SubjektivMemoryBackendResponse =
                serde_json::from_str(applied).unwrap();
            let server_api::SubjektivMemoryBackendResponse::CandidateDecided(decision) = response
            else {
                panic!("candidate apply rejected: {applied}");
            };
            assert_eq!(
                decision.action,
                server_api::SubjektivMemoryCandidateResolutionAction::Applied
            );
            assert_eq!(decision.affected_memory.len(), 1);
            if self.fail_after_apply {
                return Err(ClientError::Config(
                    "simulated model transport loss after durable candidate apply".into(),
                ));
            }
            return Ok(tool(
                "job-result",
                "SubmitJobResult",
                json!({"result":input}),
            ));
        }
        if let Some(candidate) = result("candidate") {
            let response: server_api::SubjektivMemoryBackendResponse =
                serde_json::from_str(candidate).unwrap();
            let server_api::SubjektivMemoryBackendResponse::Candidate(candidate) = response else {
                panic!("candidate read rejected: {candidate}");
            };
            assert_eq!(input["candidate_ids"], json!([candidate.candidate_id]));
            assert!(
                !candidate.evidence.is_empty(),
                "Job must receive committed provenance"
            );
            return Ok(tool(
                "apply",
                "MemoryApplyCandidate",
                json!({
                    "request_id":format!("apply:{}", candidate.candidate_id),
                    "candidate_id":candidate.candidate_id, "reason":"Adopt the committed lesson",
                    "decision":{"kind":"apply", "target":{"kind":"create"}, "memory":{
                        "kind":candidate.kind, "state":"active", "claim":candidate.claim,
                        "body_md":candidate.claim, "why_useful":candidate.why_useful,
                        "change_reason":"Adopt committed standalone evidence"
                    }}
                }),
            ));
        }
        if let Some(list) = result("candidates") {
            let response: server_api::SubjektivMemoryBackendResponse =
                serde_json::from_str(list).unwrap();
            let server_api::SubjektivMemoryBackendResponse::Candidates(list) = response else {
                panic!("candidate list rejected: {list}");
            };
            assert_eq!(
                input["candidate_ids"],
                json!(
                    list.items
                        .iter()
                        .map(|c| &c.candidate_id)
                        .collect::<Vec<_>>()
                )
            );
            if let Some(candidate) = list.items.first() {
                return Ok(tool(
                    "candidate",
                    "MemoryStagingRead",
                    json!({"candidate_id":candidate.candidate_id}),
                ));
            }
            return Ok(tool(
                "job-result",
                "SubmitJobResult",
                json!({"result":input}),
            ));
        }
        Ok(tool("candidates", "MemoryStagingList", json!({})))
    }

    fn surface_response(&self, request: &Request) -> Vec<LlmEvent> {
        assert_eq!(
            request
                .tools
                .iter()
                .map(|t| t.name.as_str())
                .collect::<Vec<_>>(),
            ["SubmitMemorySurface"]
        );
        assert!(
            !request.items.iter().any(
                |item| matches!(item, Item::ToolCall { name, .. } if name == "MemoryApplyCandidate")
            ),
            "surface editing must use a clean context, not consolidation history"
        );
        if let Some(Item::ToolResult {
            is_error,
            summary,
            content,
            ..
        }) = request
            .items
            .iter()
            .find(|item| matches!(item, Item::ToolResult { call_id, .. } if call_id == "surface"))
        {
            assert!(!is_error, "surface rejected: {summary}; {content:?}");
            assert_eq!(summary, "Accepted 1 grounded Memory surface point(s).");
            return prose();
        }
        let input: Value = request
            .items
            .iter()
            .filter_map(Item::as_text)
            .filter_map(|text| serde_json::from_str::<Value>(text).ok())
            .find(|value| value.get("materials").is_some())
            .expect("bounded confirmed surface materials");
        let materials = input["materials"].as_array().unwrap();
        assert_eq!(
            materials.len(),
            1,
            "only the confirmed lesson belongs in this surface"
        );
        let material = &materials[0];
        tool(
            "surface",
            "SubmitMemorySurface",
            json!({"points":[{
                "body_md":format!("- {}", material["body_md"].as_str().unwrap()),
                "memory_refs":[{"memory_id":material["memory_id"], "revision":material["revision"]}]
            }]}),
        )
    }

    fn extract_response(&self, request: &Request) -> Vec<LlmEvent> {
        assert!(self.extract, "unexpected extract request");
        for forbidden in [
            "Bash",
            "SubWorkerSpawn",
            "MemoryApplyCandidate",
            "SubmitJobResult",
            "SubjektivMemoryRemember",
        ] {
            assert!(
                !request.tools.iter().any(|tool| tool.name == forbidden),
                "extract received {forbidden}"
            );
        }
        let result = |id: &str| {
            request.items.iter().find_map(|item| match item {
                Item::ToolResult {
                    call_id,
                    is_error,
                    summary,
                    ..
                } if call_id == id => {
                    assert!(!is_error, "extract {id}: {summary}");
                    Some(summary)
                }
                _ => None,
            })
        };
        if result("extract-finish").is_some() {
            return prose();
        }
        if result("extract-stage").is_some() {
            return tool(
                "extract-finish",
                "FinishMemoryExtraction",
                json!({"staged_count":1}),
            );
        }
        let input = request
            .items
            .iter()
            .filter_map(Item::as_text)
            .find(|text| text.contains("# Initial session entry index"))
            .expect("committed extract view");
        let entry = input
            .lines()
            .filter(|line| line.contains("A useful lesson is worth retaining across Workers."))
            .find_map(|line| {
                line.strip_prefix("- [").and_then(|line| {
                    let (entry, rest) = line.split_once(' ')?;
                    (rest.starts_with("user ") || rest.starts_with("user]")).then_some(entry)
                })
            })
            .expect("user input reference");
        tool(
            "extract-stage",
            "StageMemoryCandidate",
            json!({
                "kind":"lesson", "claim":"Extract a committed lesson through the normal lifecycle",
                "why_useful":"Normal extraction must retain Host-validated provenance", "entry_refs":[entry]
            }),
        )
    }

    fn requests(&self) -> Vec<Request> {
        self.requests.lock().unwrap().clone()
    }
}

#[async_trait]
impl LlmClient for ScriptedClient {
    async fn stream(
        &self,
        request: Request,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<LlmEvent, ClientError>> + Send>>, ClientError>
    {
        // Lifecycle-triggered consolidation uses a separate Internal Worker. It
        // must not consume the dialog script or silently become a live request.
        let job = request
            .tools
            .iter()
            .any(|tool| tool.name == "SubmitJobResult");
        self.requests.lock().unwrap().push(request.clone());
        if let Some((state, subject)) = &self.commit_guard {
            if request
                .items
                .iter()
                .any(|item| matches!(item,Item::ToolResult {call_id,..} if call_id=="remember"))
            {
                inspect_store(state, |store| {
                    assert!(
                        store
                            .pending_staging_candidates(subject, 100)
                            .unwrap()
                            .is_empty(),
                        "the open Run must not yet publish an explicit candidate"
                    )
                });
            }
        }
        let response = if job {
            if !self.consolidate {
                return Err(ClientError::Config("unscripted background Job".into()));
            }
            self.job_response(&request)?
        } else if request
            .tools
            .iter()
            .any(|tool| tool.name == "StageMemoryCandidate")
        {
            self.extract_response(&request)
        } else if request
            .tools
            .iter()
            .any(|tool| tool.name == "SubmitMemorySurface")
        {
            assert!(self.consolidate, "unexpected surface model invocation");
            self.surface_response(&request)
        } else {
            self.responses
                .lock()
                .unwrap()
                .pop_front()
                .expect("unexpected model request: dialog script exhausted")
        };
        Ok(Box::pin(stream::iter(response.into_iter().map(Ok))))
    }

    fn clone_boxed(&self) -> Box<dyn LlmClient> {
        Box::new(self.clone())
    }
}

fn prose() -> Vec<LlmEvent> {
    vec![
        LlmEvent::text_block_start(0),
        LlmEvent::text_delta(0, "scripted answer"),
        LlmEvent::text_block_stop(0, Some(StopReason::EndTurn)),
    ]
}

fn tool(id: &str, name: &str, input: Value) -> Vec<LlmEvent> {
    vec![
        LlmEvent::tool_use_start(0, id, name),
        LlmEvent::tool_input_delta(0, input.to_string()),
        LlmEvent::tool_use_stop(0),
    ]
}

fn launch(cwd: &Path, state: &Path, subject: &str, name: &str) -> ResolvedStandaloneLaunch {
    StandaloneLaunchConfig::new(
        cwd,
        state,
        ProfileSelector::source_named(ProfileRegistrySource::Builtin, "standalone-subjektiv"),
        name,
    )
    .with_subject(subject.to_owned())
    .resolve()
    .expect("resolve selected Subject launch")
}

async fn run(host: &StandaloneHost, text: &str) -> BTreeMap<String, Value> {
    let mut client = host.connect();
    client
        .send(&Method::submit_text(
            protocol::new_submission_request_id(),
            text,
        ))
        .await
        .expect("submit dialog input");
    let mut event_trace = Vec::new();
    let outcome = tokio::time::timeout(Duration::from_secs(30), async {
        let mut results = BTreeMap::new();
        let mut answered = false;
        loop {
            let event = client
                .next_event()
                .await
                .expect("protocol stream")
                .expect("event");
            if event_trace.len() < 100 {
                event_trace.push(format!("{event:?}"));
            }
            match event {
                Event::ToolResult {
                    id,
                    output,
                    summary,
                    is_error,
                    ..
                } => {
                    assert!(!is_error, "{id}: {summary}; {output:?}");
                    let value = serde_json::from_str(&output.expect("structured tool output"))
                        .expect("tool output JSON");
                    results.insert(id, value);
                }
                Event::Error { code, message } => {
                    panic!("dialog {text:?} failed: {code:?}: {message}")
                }
                Event::TextDelta { text } if text.contains("scripted answer") => answered = true,
                Event::RunEnd { .. } => {
                    assert!(
                        answered,
                        "a failed/interrupted Run is not successful execution"
                    );
                    return results;
                }
                _ => {}
            }
        }
    })
    .await;
    outcome.unwrap_or_else(|error| {
        panic!("dialog {text:?} did not complete: {error}; events: {event_trace:#?}")
    })
}

// The common typed store is only an assertion oracle here. Mutations under
// test always go through operator admin or the Host/model capability.
fn inspect_store<T>(state: &Path, inspect: impl FnOnce(&SubjektivStore) -> T) -> T {
    let root = state.join("subjektiv");
    let scope_id: String =
        serde_json::from_slice(&std::fs::read(root.join("scope.json")).unwrap()).unwrap();
    let manager = FeatureStorage::new(root.join("features"));
    let scope = manager.scope(&scope_id).unwrap();
    let registration = SubjektivStore::register(&scope).unwrap();
    let store = SubjektivStore::open(&scope, &registration).unwrap();
    let result = inspect(&store);
    drop(store);
    manager.shutdown().unwrap();
    result
}

#[test]
fn operator_subject_catalog_survives_close_and_reopen() {
    let temp = tempfile::tempdir().unwrap();
    let state = temp.path().join("state");
    let catalog = StandaloneSubjects::open(&state).unwrap();
    assert!(catalog.list().unwrap().is_empty());
    let first = catalog
        .create("coding", Some("Keep precise evidence."))
        .unwrap();
    let second = catalog.create("coding", None).unwrap();
    assert_ne!(
        first.id, second.id,
        "role/Profile/cwd must not derive Subject identity"
    );
    uuid::Uuid::parse_str(
        first
            .id
            .strip_prefix("subject-")
            .expect("common Subject issuer"),
    )
    .unwrap();
    uuid::Uuid::parse_str(
        second
            .id
            .strip_prefix("subject-")
            .expect("common Subject issuer"),
    )
    .unwrap();
    drop(catalog);
    let reopened = StandaloneSubjects::open(&state).unwrap();
    let items = reopened.list().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items.iter().find(|s| s.id == first.id).unwrap(), &first);
    assert_eq!(items.iter().find(|s| s.id == second.id).unwrap(), &second);
}

#[tokio::test]
async fn enabled_profile_without_subject_is_inert_and_disabled_selection_is_rejected() {
    let temp = tempfile::tempdir().unwrap();
    for profile in ["standalone-subjektiv", "standalone"] {
        let state = temp.path().join(profile);
        let launch = StandaloneLaunchConfig::new(
            temp.path(),
            &state,
            ProfileSelector::source_named(ProfileRegistrySource::Builtin, profile),
            "unattached",
        )
        .resolve()
        .unwrap();
        let model = ScriptedClient::new(vec![prose()]);
        let host = StandaloneHost::start_with_model_client(launch, model.clone())
            .await
            .unwrap();
        assert!(host.record().subject.is_none());
        run(
            &host,
            "An unattached policy must not manufacture a Subject.",
        )
        .await;
        let request = &model.requests()[0];
        assert!(request.tools.iter().any(|t| t.name == "Read"));
        assert!(
            !request
                .tools
                .iter()
                .any(|t| t.name.starts_with("Subjektiv"))
        );
        assert!(host.jobs().pending().unwrap().is_empty());
        host.shutdown().await.unwrap();
        assert!(
            !state.join("subjektiv").exists(),
            "policy alone must have no Subject storage side effect"
        );
    }
    let state = temp.path().join("rejected");
    let selected = uuid::Uuid::now_v7().to_string();
    let error = StandaloneLaunchConfig::new(
        temp.path(),
        &state,
        ProfileSelector::source_named(ProfileRegistrySource::Builtin, "standalone"),
        "disabled",
    )
    .with_subject(selected)
    .resolve()
    .err()
    .expect("disabled selection rejected");
    assert_eq!(error, StandaloneLaunchError::InvalidSubjectPolicy);
    assert!(!state.exists(), "admission rejection precedes persistence");
}

#[tokio::test]
async fn explicit_remember_is_committed_staging_and_sessions_follow_subject_across_workers() {
    let temp = tempfile::tempdir().unwrap();
    let state = temp.path().join("state");
    let catalog = StandaloneSubjects::open(&state).unwrap();
    let subject = catalog
        .create("coding", Some("Keep committed references."))
        .unwrap();
    let foreign = catalog.create("other", None).unwrap();
    drop(catalog);
    let claim = "Use immutable committed references for test evidence";
    let mut model = ScriptedClient::new(vec![
        tool(
            "remember",
            "SubjektivMemoryRemember",
            json!({
                "kind":"lesson", "claim":claim, "why_useful":"Avoid uncommitted evidence"
            }),
        ),
        tool("query", "SubjektivMemoryQuery", json!({"query":claim})),
        prose(),
    ]);
    model.commit_guard = Some((state.clone(), subject.id.clone()));
    let host = StandaloneHost::start_with_model_client(
        launch(temp.path(), &state, &subject.id, "first-body"),
        model.clone(),
    )
    .await
    .unwrap();
    let binding = host.record().subject.clone().unwrap();
    let worker_id = host.worker_id();
    let session_id = host.record().active_session_id;
    assert_eq!(binding.subject_id, subject.id);
    assert!(
        host.record()
            .manifest
            .feature
            .subjektiv
            .workspace_settings
            .is_none()
    );
    let results = run(
        &host,
        "Remember this lesson using committed model-origin evidence.",
    )
    .await;
    assert_eq!(results["remember"]["status"], "pending_commit");
    assert_eq!(
        results["query"]["items"],
        json!([]),
        "remember never writes confirmed Memory"
    );
    let request = &model.requests()[0];
    for name in [
        "SubjektivMemoryRemember",
        "SubjektivMemoryQuery",
        "SubjektivSessionSearch",
        "SubjektivSessionRead",
    ] {
        assert!(
            request.tools.iter().any(|t| t.name == name),
            "missing {name}"
        );
    }
    assert!(
        !request
            .tools
            .iter()
            .any(|t| t.name == "MemoryApplyCandidate"),
        "body must not receive consolidation authority"
    );
    host.shutdown().await.unwrap();

    let staged: SubjectStagingRecord = inspect_store(&state, |store| {
        assert!(store.list_memories(&subject.id).unwrap().is_empty());
        let candidates = store.pending_staging_candidates(&subject.id, 100).unwrap();
        assert_eq!(
            candidates.len(),
            1,
            "committed remember must become staging, not disappear"
        );
        let attribution = store
            .session_attribution(&session_id.to_string())
            .unwrap()
            .unwrap();
        assert_eq!(attribution.subject_id, subject.id);
        assert_eq!(attribution.runtime_id, binding.scope_id);
        assert_eq!(attribution.worker_id, worker_id.to_string());
        candidates.into_iter().next().unwrap()
    });
    assert_eq!(staged.claim, claim);
    assert_eq!(staged.subject_id, subject.id);
    assert!(!staged.evidence.is_empty());
    assert_eq!(staged.evidence.len(), staged.source_refs.len());
    let workers = StandaloneWorkerStore::open(&state).unwrap();
    let index = session_store::public_index::read_fs_session_public_index(
        &workers.sessions_dir(worker_id),
        session_id,
        Default::default(),
    )
    .unwrap();
    for evidence in &staged.evidence {
        let source = staged
            .source_refs
            .iter()
            .find(|s| s.evidence_id.as_deref() == Some(&evidence.id))
            .unwrap();
        assert_eq!(
            source.session_id.as_deref(),
            Some(session_id.to_string().as_str())
        );
        let entry = index
            .segments
            .iter()
            .filter(|segment| source.segment_id.as_deref() == Some(segment.segment_id.as_str()))
            .flat_map(|segment| &segment.entries)
            .find(|entry| entry.entry_ref == evidence.id)
            .expect("exact committed public reference");
        assert!(entry.compact_text.contains(claim));
        let [start, end] = evidence.entry_range.expect("committed range");
        assert!(
            start <= end,
            "committed source ranges use inclusive endpoints"
        );
        assert_eq!(source.entry_range, evidence.entry_range);
        let origin = evidence.origin.as_ref().unwrap();
        assert_eq!(origin.kind, memory::schema::EvidenceOriginKind::ModelOutput);
        assert!(
            origin.runtime_id.is_none(),
            "local scope is not a Runtime identity"
        );
        assert_eq!(
            origin.worker_id.as_deref(),
            Some(worker_id.to_string().as_str())
        );
        assert!(origin.workspace_id.is_none());
        assert!(origin.account_id.is_none());
        assert_eq!(source.origin.as_ref(), Some(origin));
    }
    let source = &staged.source_refs[0];
    let read_input = json!({
        "session_id":session_id.to_string(), "segment_id":source.segment_id,
        "entry_ref":source.evidence_id,
    });
    let restored_model = ScriptedClient::new(vec![
        tool("recall", "SubjektivSessionRead", read_input.clone()),
        tool("query-restored", "SubjektivMemoryQuery", json!({})),
        prose(),
    ]);
    let restored =
        StandaloneHost::restore_with_model_client(state.clone(), worker_id, restored_model.clone())
            .await
            .unwrap();
    assert_eq!(restored.record().subject.as_ref(), Some(&binding));
    assert_eq!(restored.record().active_session_id, session_id);
    let results = run(&restored, "Resume and recall the committed evidence.").await;
    assert_eq!(results["recall"]["status"], "ok");
    assert!(results["recall"].to_string().contains(claim));
    assert_eq!(results["query-restored"]["items"], json!([]));
    assert!(format!("{:?}", restored_model.requests()[0].items).contains("Remember this lesson"));
    restored.shutdown().await.unwrap();

    let second_model = ScriptedClient::new(vec![
        tool("prior-session", "SubjektivSessionRead", read_input.clone()),
        tool("sessions", "SubjektivSessionList", json!({})),
        prose(),
    ]);
    let second = StandaloneHost::start_with_model_client(
        launch(temp.path(), &state, &subject.id, "second-body"),
        second_model,
    )
    .await
    .unwrap();
    assert_ne!(second.worker_id(), worker_id);
    assert_eq!(second.record().subject.as_ref(), Some(&binding));
    let second_session = second.record().active_session_id;
    let results = run(
        &second,
        "Recall from another Worker, without a new Memory candidate.",
    )
    .await;
    assert_eq!(results["prior-session"]["status"], "ok");
    assert!(results["prior-session"].to_string().contains(claim));
    let sessions = results["sessions"].to_string();
    assert!(sessions.contains(&session_id.to_string()));
    assert!(
        sessions.contains(&second_session.to_string()),
        "Sessions with no candidates must be attributed too"
    );
    second.shutdown().await.unwrap();
    inspect_store(&state, |store| {
        assert_eq!(
            serde_json::to_value(store.pending_staging_candidates(&subject.id, 100).unwrap())
                .unwrap(),
            json!([staged])
        );
        assert_eq!(
            store
                .session_attribution(&second_session.to_string())
                .unwrap()
                .unwrap()
                .subject_id,
            subject.id
        );
    });

    let other = StandaloneHost::start_with_model_client(
        launch(temp.path(), &state, &foreign.id, "foreign-body"),
        ScriptedClient::new(vec![
            tool("foreign", "SubjektivSessionRead", read_input),
            prose(),
        ]),
    )
    .await
    .unwrap();
    let results = run(
        &other,
        "A known foreign Session reference is not authority.",
    )
    .await;
    assert_eq!(results["foreign"]["status"], "error");
    assert!(!results["foreign"].to_string().contains(claim));
    other.shutdown().await.unwrap();
}

#[tokio::test]
async fn restore_keeps_saved_subject_and_rejects_replaced_storage_scope() {
    let temp = tempfile::tempdir().unwrap();
    let state = temp.path().join("state");
    let catalog = StandaloneSubjects::open(&state).unwrap();
    let subject = catalog.create("original", None).unwrap();
    let other = catalog.create("not-a-resume-selection", None).unwrap();
    drop(catalog);
    let host = StandaloneHost::start_with_model_client(
        launch(temp.path(), &state, &subject.id, "saved-binding"),
        ScriptedClient::new(Vec::new()),
    )
    .await
    .unwrap();
    let id = host.worker_id();
    let binding = host.record().subject.clone().unwrap();
    host.shutdown().await.unwrap();

    // Replace only the operator-managed storage root with a genuine, separately
    // issued scope. The saved Worker/cwd/session remains otherwise untouched.
    let foreign_state = temp.path().join("foreign-state");
    let catalog = StandaloneSubjects::open(&foreign_state).unwrap();
    catalog.create("foreign", None).unwrap();
    drop(catalog);
    let original_root = temp.path().join("saved-subjektiv");
    std::fs::rename(state.join("subjektiv"), &original_root).unwrap();
    std::fs::rename(foreign_state.join("subjektiv"), state.join("subjektiv")).unwrap();
    let error = StandaloneHost::restore_with_model_client(
        state.clone(),
        id,
        ScriptedClient::new(Vec::new()),
    )
    .await
    .err()
    .expect("foreign scope rejected");
    assert!(
        matches!(error, StandaloneStartupError::Subject(ref message) if message.contains("storage scope changed")),
        "{error:?}"
    );
    assert_eq!(
        StandaloneWorkerStore::open(&state)
            .unwrap()
            .load(id)
            .unwrap()
            .subject
            .as_ref(),
        Some(&binding)
    );
    std::fs::rename(state.join("subjektiv"), foreign_state.join("subjektiv")).unwrap();
    std::fs::rename(&original_root, state.join("subjektiv")).unwrap();
    let restored =
        StandaloneHost::restore_with_model_client(state, id, ScriptedClient::new(Vec::new()))
            .await
            .unwrap();
    assert_eq!(restored.record().subject.as_ref(), Some(&binding));
    assert_ne!(
        restored.record().subject.as_ref().unwrap().subject_id,
        other.id
    );
    restored.shutdown().await.unwrap();
}

// A failing test never leaves its test-owned process running. This guard can
// only address the Child returned by spawn, never a parent or dogfood Worker.
struct OwnedChild(Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}

fn child_command(mode: &str, cwd: &Path, state: &Path, subject: &str, ready: &Path) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", "subject_process_child", "--nocapture"])
        .env("YOI_SUBJEKTIV_TEST_CHILD_MODE", mode)
        .env("YOI_SUBJEKTIV_TEST_CWD", cwd)
        .env("YOI_SUBJEKTIV_TEST_STATE", state)
        .env("YOI_SUBJEKTIV_TEST_SUBJECT", subject)
        .env("YOI_SUBJEKTIV_TEST_READY", ready)
        .stdin(Stdio::piped());
    command
}

fn wait_child(child: &mut OwnedChild) -> std::process::ExitStatus {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            return status;
        }
        assert!(Instant::now() < deadline, "test-owned child did not exit");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[tokio::test]
async fn subject_process_child() {
    // The ordinary harness also discovers this subprocess entry point. Only a
    // parent semantic test supplies the private fixture paths and mode.
    let Ok(mode) = std::env::var("YOI_SUBJEKTIV_TEST_CHILD_MODE") else {
        return;
    };
    let cwd = PathBuf::from(std::env::var_os("YOI_SUBJEKTIV_TEST_CWD").unwrap());
    let state = PathBuf::from(std::env::var_os("YOI_SUBJEKTIV_TEST_STATE").unwrap());
    let subject = std::env::var("YOI_SUBJEKTIV_TEST_SUBJECT").unwrap();
    let result = StandaloneHost::start_with_model_client(
        launch(&cwd, &state, &subject, "test-process"),
        ScriptedClient::new(Vec::new()),
    )
    .await;
    match mode.as_str() {
        "contender" => {
            let error = result
                .err()
                .expect("another process owns the Subject kernel lease");
            assert!(
                matches!(error, StandaloneStartupError::Subject(ref message) if message.contains("already active")),
                "{error:?}"
            );
        }
        "abnormal-owner" => {
            let host = result.expect("child owns Subject");
            let ready = PathBuf::from(std::env::var_os("YOI_SUBJEKTIV_TEST_READY").unwrap());
            let mut file = std::fs::File::create(&ready).unwrap();
            file.write_all(host.worker_id().to_string().as_bytes())
                .unwrap();
            file.sync_all().unwrap();
            let mut byte = [0];
            std::io::stdin().read_exact(&mut byte).unwrap();
            assert_eq!(byte, [b'x']);
            // Deliberately omit Host shutdown and all Rust destructors. The
            // parent requests this exit only after proving cross-process lock.
            std::process::exit(86);
        }
        other => panic!("unknown child mode {other}"),
    }
}

#[tokio::test]
async fn kernel_subject_lease_excludes_other_processes_and_recovers_after_abnormal_exit() {
    let temp = tempfile::tempdir().unwrap();
    let state = temp.path().join("state");
    let parent_cwd = temp.path().join("parent-project");
    let child_cwd = temp.path().join("child-project");
    std::fs::create_dir(&parent_cwd).unwrap();
    std::fs::create_dir(&child_cwd).unwrap();
    let catalog = StandaloneSubjects::open(&state).unwrap();
    let subject = catalog.create("singleton", None).unwrap();
    let independent = catalog.create("independent", None).unwrap();
    drop(catalog);
    let ready = temp.path().join("ready");
    let parent = StandaloneHost::start_with_model_client(
        launch(&parent_cwd, &state, &subject.id, "parent-owner"),
        ScriptedClient::new(Vec::new()),
    )
    .await
    .unwrap();
    let mut contender = OwnedChild(
        child_command("contender", &child_cwd, &state, &subject.id, &ready)
            .spawn()
            .unwrap(),
    );
    assert!(
        wait_child(&mut contender).success(),
        "a different cwd must not evade the Subject lock"
    );
    let independent_host = StandaloneHost::start_with_model_client(
        launch(&child_cwd, &state, &independent.id, "independent-owner"),
        ScriptedClient::new(Vec::new()),
    )
    .await
    .unwrap();
    independent_host.shutdown().await.unwrap();
    parent.shutdown().await.unwrap();

    let mut owner = OwnedChild(
        child_command("abnormal-owner", &child_cwd, &state, &subject.id, &ready)
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(30);
    let id = loop {
        if ready.exists() {
            let text = std::fs::read_to_string(&ready).unwrap();
            if let Ok(id) = text.parse::<standalone::WorkerId>() {
                break id;
            }
        }
        assert!(
            owner.0.try_wait().unwrap().is_none(),
            "owner exited before ready"
        );
        assert!(Instant::now() < deadline, "owner did not become ready");
        std::thread::sleep(Duration::from_millis(10));
    };
    let mut contender = OwnedChild(
        child_command("contender", &parent_cwd, &state, &subject.id, &ready)
            .spawn()
            .unwrap(),
    );
    assert!(wait_child(&mut contender).success());
    owner.0.stdin.as_mut().unwrap().write_all(b"x").unwrap();
    assert_eq!(
        wait_child(&mut owner).code(),
        Some(86),
        "exercise process death, not graceful shutdown"
    );
    let recovered =
        StandaloneHost::restore_with_model_client(state, id, ScriptedClient::new(Vec::new()))
            .await
            .unwrap();
    assert_eq!(recovered.worker_id(), id);
    assert_eq!(
        recovered.record().subject.as_ref().unwrap().subject_id,
        subject.id
    );
    recovered.shutdown().await.unwrap();
}

fn remember_script(claim: &str) -> Vec<Vec<LlmEvent>> {
    vec![
        tool(
            "remember",
            "SubjektivMemoryRemember",
            json!({
                "kind":"lesson", "claim":claim, "why_useful":"Retain standalone evidence"
            }),
        ),
        prose(),
    ]
}

#[tokio::test]
async fn automatic_generic_consolidation_applies_candidate_publishes_surface_and_follows_subject() {
    let temp = tempfile::tempdir().unwrap();
    let state = temp.path().join("state");
    let catalog = StandaloneSubjects::open(&state).unwrap();
    let subject = catalog.create("consolidation", None).unwrap();
    drop(catalog);
    let claim = "Validate the immutable result before reporting success";
    let model = ScriptedClient::consolidating(remember_script(claim), false);
    let host = StandaloneHost::start_with_model_client(
        launch(temp.path(), &state, &subject.id, "consolidation-body"),
        model.clone(),
    )
    .await
    .unwrap();
    let worker_id = host.worker_id();
    let results = run(
        &host,
        "Remember this lesson and let the Host consolidate its committed candidate.",
    )
    .await;
    assert_eq!(results["remember"]["status"], "pending_commit");
    let job_id = model.first_job().await;
    let completed = tokio::time::timeout(Duration::from_secs(30), host.jobs().wait(&job_id))
        .await
        .expect("Job cleanup completes")
        .unwrap();
    assert_eq!(
        completed.state,
        standalone::jobs::JobState::Completed,
        "{completed:?}"
    );
    assert_eq!(
        completed.request.profile,
        "builtin:standalone-subjektiv-consolidation"
    );
    assert_eq!(completed.request.purpose, "subjektiv_consolidation");
    assert!(
        completed.domain_grant.is_some(),
        "policy is not a domain capability"
    );
    let ids = completed.request.input["candidate_ids"].as_array().unwrap();
    assert_eq!(ids.len(), 1, "Job must snapshot the committed candidate");
    let accepted = completed
        .result
        .clone()
        .expect("structured result is durable success authority");
    assert_eq!(accepted["subject_id"], subject.id);
    assert_eq!(accepted["candidate_ids"], json!(ids));
    assert_eq!(
        accepted["candidate_dispositions"],
        json!([{"candidate_id":ids[0], "action":"applied"}])
    );
    assert_eq!(accepted["surface"]["availability"], "ready");
    assert!(accepted["surface"]["generation_id"].is_string());
    assert!(accepted["surface"]["snapshot_id"].is_string());
    // The Subject consumer may already have acknowledged successful cleanup.
    // Acknowledgement timing is not success authority; the immutable accepted
    // result above remains durable regardless of that scheduling race.
    let requests = model.requests();
    assert!(requests.iter().any(|r| {
        r.tools
            .iter()
            .any(|tool| tool.name == "MemoryApplyCandidate")
    }));
    assert!(requests.iter().any(|r| {
        r.tools
            .iter()
            .any(|tool| tool.name == "SubmitMemorySurface")
    }));
    assert!(
        state
            .join("subjektiv/jobs")
            .join(&subject.id)
            .join("jobs.sqlite3")
            .exists()
    );
    assert!(
        !state
            .join(worker_id.to_string())
            .join("jobs.sqlite3")
            .exists(),
        "selected Subject owns the shared generic Job ledger"
    );
    host.shutdown().await.unwrap();
    let memory = inspect_store(&state, |store| {
        assert!(
            store
                .pending_staging_candidates(&subject.id, 100)
                .unwrap()
                .is_empty()
        );
        let memory = store.list_memories(&subject.id).unwrap().pop().unwrap();
        assert_eq!(memory.claim, claim);
        assert_eq!(memory.revision, 1);
        assert_eq!(memory.source_candidate_ids, vec![ids[0].as_str().unwrap()]);
        let surface = store.resident_surface(&subject.id).unwrap();
        assert_eq!(surface.availability, subjektiv::SurfaceAvailability::Ready);
        let snapshot = surface.snapshot.unwrap();
        assert_eq!(
            snapshot.id,
            accepted["surface"]["snapshot_id"].as_str().unwrap()
        );
        assert!(snapshot.body_md.contains(claim));
        assert_eq!(
            snapshot.memory_refs,
            vec![subjektiv::MemoryRevisionRef {
                memory_id: memory.id.clone(),
                revision: 1
            }]
        );
        assert_eq!(
            snapshot.built_from_store_revision,
            accepted["surface"]["store_revision"].as_u64().unwrap()
        );
        memory
    });
    let next_model = ScriptedClient::new(vec![
        tool("confirmed", "SubjektivMemoryQuery", json!({"query":claim})),
        prose(),
    ]);
    let next = StandaloneHost::start_with_model_client(
        launch(temp.path(), &state, &subject.id, "next-body"),
        next_model.clone(),
    )
    .await
    .unwrap();
    assert_ne!(next.worker_id(), worker_id);
    let retained = next.jobs().get(&job_id).unwrap();
    assert_eq!(retained.request, completed.request);
    assert_eq!(retained.state, completed.state);
    assert_eq!(
        retained.result, completed.result,
        "result follows the Subject, not the old Worker"
    );
    let results = run(&next, "Recall confirmed Memory in the next Worker.").await;
    assert_eq!(results["confirmed"]["items"].as_array().unwrap().len(), 1);
    assert!(results["confirmed"].to_string().contains(&memory.id));
    assert!(
        next_model.requests()[0]
            .system_prompt
            .as_deref()
            .unwrap()
            .contains(claim),
        "fresh resident surface must be injected at new Worker creation"
    );
    assert_eq!(
        next.jobs().acknowledge(&job_id).unwrap().result,
        Some(accepted)
    );
    next.shutdown().await.unwrap();
}

#[tokio::test]
async fn transport_loss_after_candidate_apply_preserves_memory_and_later_job_only_rebuilds_surface()
{
    let temp = tempfile::tempdir().unwrap();
    let state = temp.path().join("state");
    let catalog = StandaloneSubjects::open(&state).unwrap();
    let subject = catalog.create("partial-apply", None).unwrap();
    drop(catalog);
    let claim = "Candidate receipts survive transport failure";
    let losing = ScriptedClient::consolidating(remember_script(claim), true);
    let host = StandaloneHost::start_with_model_client(
        launch(temp.path(), &state, &subject.id, "losing-body"),
        losing.clone(),
    )
    .await
    .unwrap();
    run(
        &host,
        "Stage a lesson whose Job loses its transport after applying it.",
    )
    .await;
    let first_id = losing.first_job().await;
    let failed = tokio::time::timeout(Duration::from_secs(30), host.jobs().wait(&first_id))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        failed.state,
        standalone::jobs::JobState::Failed,
        "{failed:?}"
    );
    assert!(
        failed.result.is_none(),
        "partial apply is not an accepted Job result"
    );
    host.shutdown().await.unwrap();
    let original = inspect_store(&state, |store| {
        assert!(
            store
                .pending_staging_candidates(&subject.id, 100)
                .unwrap()
                .is_empty()
        );
        let memory = store.list_memories(&subject.id).unwrap();
        assert_eq!(memory.len(), 1);
        assert_eq!(memory[0].claim, claim);
        assert_eq!(memory[0].revision, 1);
        assert_ne!(
            store.resident_surface(&subject.id).unwrap().availability,
            subjektiv::SurfaceAvailability::Ready
        );
        serde_json::to_value(&memory[0]).unwrap()
    });
    let rebuilding = ScriptedClient::consolidating(vec![prose()], false);
    let next = StandaloneHost::start_with_model_client(
        launch(temp.path(), &state, &subject.id, "rebuild-body"),
        rebuilding.clone(),
    )
    .await
    .unwrap();
    assert_eq!(next.jobs().get(&first_id).unwrap(), failed);
    run(
        &next,
        "A later explicit Host interaction rebuilds the missing surface.",
    )
    .await;
    let second_id = rebuilding.first_job().await;
    assert_ne!(
        first_id, second_id,
        "do not silently replay the failed immutable Job"
    );
    let completed = tokio::time::timeout(Duration::from_secs(30), next.jobs().wait(&second_id))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        completed.state,
        standalone::jobs::JobState::Completed,
        "{completed:?}"
    );
    assert_eq!(
        completed.request.input["candidate_ids"],
        json!([]),
        "resolved candidates cannot be applied again"
    );
    assert_eq!(
        completed.result.as_ref().unwrap()["surface"]["availability"],
        "ready"
    );
    assert!(
        !rebuilding
            .requests()
            .iter()
            .any(|request| request.items.iter().any(
                |item| matches!(item, Item::ToolCall { name, .. } if name == "MemoryApplyCandidate")
            ))
    );
    assert_eq!(
        next.jobs().get(&first_id).unwrap(),
        failed,
        "later success does not rewrite the original failed outcome"
    );
    next.shutdown().await.unwrap();
    inspect_store(&state, |store| {
        let memories = store.list_memories(&subject.id).unwrap();
        assert_eq!(memories.len(), 1);
        assert_eq!(
            serde_json::to_value(&memories[0]).unwrap(),
            original,
            "confirmed Memory is immutable across surface repair"
        );
        assert_eq!(
            store.resident_surface(&subject.id).unwrap().availability,
            subjektiv::SurfaceAvailability::Ready
        );
    });
}

#[tokio::test]
async fn normal_extract_stages_committed_sources_and_consolidates_through_generic_job() {
    let temp = tempfile::tempdir().unwrap();
    let state = temp.path().join("state");
    let catalog = StandaloneSubjects::open(&state).unwrap();
    let subject = catalog.create("extract", None).unwrap();
    drop(catalog);
    let mut model = ScriptedClient::consolidating(vec![prose()], false);
    model.extract = true;
    let mut selected = launch(temp.path(), &state, &subject.id, "extract-body");
    selected
        .profile
        .manifest
        .feature
        .subjektiv
        .profile
        .extraction
        .threshold = Some(1);
    // An explicit model override selects a different provider by contract.
    // Keep this mechanism test entirely on the injected scripted transport.
    selected
        .profile
        .manifest
        .feature
        .subjektiv
        .profile
        .extraction
        .model = None;
    let host = StandaloneHost::start_with_model_client(selected, model.clone())
        .await
        .unwrap();
    let session = host.record().active_session_id;
    run(&host, "A useful lesson is worth retaining across Workers.").await;
    let job_id = model.first_job().await;
    let finished = tokio::time::timeout(Duration::from_secs(30), host.jobs().wait(&job_id))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        finished.state,
        standalone::jobs::JobState::Completed,
        "{finished:?}"
    );
    assert_eq!(
        finished.result.as_ref().unwrap()["candidate_dispositions"][0]["action"],
        "applied"
    );
    let worker = host.worker_id();
    host.shutdown().await.unwrap();
    inspect_store(&state, |store| {
        let memory = store.list_memories(&subject.id).unwrap().pop().unwrap();
        assert_eq!(
            memory.claim,
            "Extract a committed lesson through the normal lifecycle"
        );
        let candidate = store
            .staging_candidate(&subject.id, &memory.source_candidate_ids[0])
            .unwrap()
            .unwrap();
        assert_eq!(
            candidate.source_refs[0].session_id,
            Some(session.to_string())
        );
        assert!(
            candidate
                .evidence
                .iter()
                .all(|e| e.origin.as_ref().unwrap().kind
                    == memory::schema::EvidenceOriginKind::HumanInput),
            "{candidate:#?}"
        );
        assert_eq!(
            store.resident_surface(&subject.id).unwrap().availability,
            subjektiv::SurfaceAvailability::Ready
        );
    });
    assert!(
        model
            .requests()
            .iter()
            .any(|r| r.tools.iter().any(|t| t.name == "StageMemoryCandidate")),
        "extract must use the injected scripted transport"
    );
    let log_root = state
        .join(worker.to_string())
        .join("sessions")
        .join(session.to_string());
    let text = std::fs::read_dir(log_root)
        .unwrap()
        .map(|entry| std::fs::read_to_string(entry.unwrap().path()).unwrap())
        .collect::<String>();
    assert!(
        text.contains("subjektiv.extract.v1"),
        "normal extraction must durably advance the existing append-only pointer"
    );
}
