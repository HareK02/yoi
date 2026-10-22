//! Connection tests for model-visible environments and actual routed file operations.
//! Runs the production SubWorker spawn/broker/Worker/prompt/tool path with temporary
//! LocalWorkdirSession providers; only the external LLM is scripted. Selected by
//! the normal worker lib suite, without network credentials or live Workspace data.

use super::*;
use agen::Item;
use serde_json::{Value, json};
use workdir::{WorkdirSessionCapability, WorkdirSessionRouter};

const TASK: &str = "Read marker.txt from each attached Workdir.";

fn prompt_environments(prompt: &str) -> Vec<Value> {
    let section = prompt
        .split_once("## Workdir execution environment\n")
        .expect("Worker must supply the execution environment independently of task text")
        .1;
    let json = section.split_once("```json\n").unwrap().1;
    serde_json::from_str(json.split_once("\n```").unwrap().0).unwrap()
}

#[derive(Clone, Default)]
struct EnvironmentClient {
    requests: Arc<Mutex<Vec<Request>>>,
    read_issued: Arc<AtomicBool>,
}

#[async_trait]
impl LlmClient for EnvironmentClient {
    fn clone_boxed(&self) -> Box<dyn LlmClient> {
        Box::new(self.clone())
    }

    async fn stream(
        &self,
        request: Request,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<LlmEvent, ClientError>> + Send>>, ClientError>
    {
        let environments = prompt_environments(request.system_prompt.as_deref().unwrap());
        self.requests.lock().unwrap().push(request);
        let mut events = Vec::new();
        if !self.read_issued.swap(true, Ordering::SeqCst) {
            // Use only the model-visible aliases and the same relative path in
            // every session. No test-side cwd prefix is passed to the Read tool.
            for (index, environment) in environments.iter().enumerate() {
                let alias = environment["alias"].as_str().unwrap();
                events.extend([
                    LlmEvent::tool_use_start(index, format!("read-{alias}"), "Read"),
                    LlmEvent::tool_input_delta(
                        index,
                        json!({"target_workdir": alias, "file_path": "marker.txt"}).to_string(),
                    ),
                    LlmEvent::tool_use_stop(index),
                ]);
            }
        } else {
            events.extend([
                LlmEvent::text_block_start(0),
                LlmEvent::text_delta(0, "read complete"),
                LlmEvent::text_block_stop(0, None),
            ]);
        }
        events.push(LlmEvent::Status(StatusEvent {
            status: ResponseStatus::Completed,
        }));
        Ok(Box::pin(futures::stream::iter(events.into_iter().map(Ok))))
    }
}

struct EnvironmentFixture {
    runtime: TempDir,
    root: PathBuf,
    manifest: WorkerManifest,
    profiles: AvailableProfiles,
    registry: Arc<SpawnedWorkerRegistry>,
    sessions: Arc<WorkdirSessionRouter>,
}

impl EnvironmentFixture {
    fn new(aliases: &[(&str, &str)]) -> Self {
        let runtime = TempDir::new().unwrap();
        let root = runtime.path().join("profile-root");
        // Profile discovery stays at this root, deliberately outside every
        // attached Workdir. Session cwd must never redirect it into work/nested.
        let profiles = write_project_profile_registry(&root, None, &[]);
        let mut manifest = parent_manifest(&root, None);
        manifest.feature.sub_worker.enabled = true;
        let registry = SpawnedWorkerRegistry::new_internal(
            "parent".into(),
            SharedScope::new(Scope::from_config(&manifest.scope).unwrap()),
        );
        let sessions = Arc::new(WorkdirSessionRouter::new());
        for (alias, cwd) in aliases {
            let checkout = runtime.path().join(alias);
            for path in ["", "work", "work/nested", "work/nested/deep", "other"] {
                std::fs::create_dir_all(checkout.join(path)).unwrap();
                std::fs::write(checkout.join(path).join("marker.txt"), marker(alias, path))
                    .unwrap();
            }
            let scope = SharedScope::new(Scope::writable(&checkout).unwrap());
            let session = Arc::new(workdir::LocalWorkdirSession::materialized_bound(
                workdir::Workdir::new(format!("fixture-{alias}")),
                checkout.clone(),
                checkout.join(cwd),
                scope,
                workdir::WorkdirSessionCapabilities::ALL,
            ));
            sessions
                .attach_at_cwd(
                    workdir::WorkdirAttachmentAlias::new(*alias).unwrap(),
                    session,
                    workdir::WorkdirPath::new(cwd).unwrap(),
                )
                .unwrap();
        }
        Self {
            runtime,
            root,
            manifest,
            profiles,
            registry,
            sessions,
        }
    }

    async fn spawn(
        &self,
        spawner: &str,
        registry: Arc<SpawnedWorkerRegistry>,
        broker: workdir::WorkdirToolBrokerRouter,
        input: Value,
    ) -> (
        crate::spawn::registry::InternalSpawnedWorkerRecord,
        EnvironmentClient,
    ) {
        assert_eq!(input["task"], TASK);
        let client = EnvironmentClient::default();
        let mut manifest = self.manifest.clone();
        manifest.worker.name = spawner.into();
        let tool = SubWorkerSpawnTool::new(
            spawner.into(),
            crate::worker::WorkerWorkspaceContext::local_filesystem(None),
            ParentNotificationTarget::Durable(Arc::new(|_| {})),
            self.runtime.path().to_path_buf(),
            self.root.clone(),
            Some(broker),
            registry.clone(),
            manifest,
            PromptCatalogSource::builtins_only(),
            self.profiles.clone(),
        )
        .with_internal_client(Box::new(client.clone()));
        tool.execute(
            &input.to_string(),
            agen::tool::ToolExecutionContext::direct(),
        )
        .await
        .expect("spawn through production delegation and Worker construction");
        let record = registry
            .get_internal(input["name"].as_str().unwrap())
            .unwrap();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(5), record.session.wait_until_idle())
                .await
                .expect("scripted read turn must terminate"),
            InternalWorkerSessionStatus::Idle,
        );
        (record, client)
    }
}

fn marker(alias: &str, cwd: &str) -> String {
    format!(
        "{alias}:{}:session-marker",
        if cwd.is_empty() { "." } else { cwd }
    )
}

fn assert_environment_and_reads(
    router: &WorkdirSessionRouter,
    client: &EnvironmentClient,
    expected: &[(&str, &str, bool)],
) -> String {
    let requests = client.requests.lock().unwrap();
    assert!(
        requests.len() >= 2,
        "Read must execute and return to the model"
    );
    let prompt = requests[0].system_prompt.as_deref().unwrap();
    assert!(
        !prompt.contains("## Working boundaries"),
        "legacy Scope must not override the connected session environment"
    );
    let environments = prompt_environments(prompt);
    assert_eq!(environments.len(), expected.len());
    assert_eq!(router.len(), expected.len());
    for (alias, cwd, command) in expected {
        let environment = router.resolve(Some(alias)).unwrap().session.environment();
        assert_eq!(environment.cwd.as_str(), *cwd);
        assert_eq!(
            environment
                .capabilities
                .supports(WorkdirSessionCapability::Command),
            *command
        );
        let projected = environments
            .iter()
            .find(|env| env["alias"] == *alias)
            .unwrap();
        assert_eq!(projected["cwd"], if cwd.is_empty() { "." } else { *cwd });
        assert_eq!(
            projected["scope"],
            serde_json::to_value(&environment.scope).unwrap()
        );
        assert_eq!(
            projected["restrictions"],
            serde_json::to_value(&environment.restrictions).unwrap()
        );
        assert_eq!(projected["command"], *command);
        for (capability, name) in [
            (WorkdirSessionCapability::Read, "read"),
            (WorkdirSessionCapability::Write, "write"),
            (WorkdirSessionCapability::Edit, "edit"),
            (WorkdirSessionCapability::Glob, "glob"),
            (WorkdirSessionCapability::Grep, "grep"),
            (WorkdirSessionCapability::Command, "command"),
        ] {
            assert_eq!(
                projected["capabilities"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|value| value == name),
                environment.capabilities.supports(capability),
            );
        }
        let expected_marker = marker(alias, cwd);
        let observed_read = requests
            .iter()
            .flat_map(|request| &request.items)
            .any(|item| {
                matches!(item, Item::ToolResult { call_id, content: Some(content), .. }
                if call_id == &format!("read-{alias}") && content.contains(&expected_marker))
            });
        assert!(
            observed_read,
            "{alias}: relative Read did not return {expected_marker}"
        );
    }
    prompt.to_owned()
}

fn assert_retained_environment(
    record: &crate::spawn::registry::InternalSpawnedWorkerRecord,
    prompt: &str,
) {
    let retained = record.session.entries().iter().any(|entry| {
        matches!(entry, session_store::LogEntry::AnnotatedSegmentStart { system_prompt: Some(saved), .. }
            if saved == prompt)
    });
    assert!(
        retained,
        "the model-visible environment must also be retained in session history"
    );
}

#[tokio::test]
async fn spawn_root_and_nonroot_cwd_are_projected_without_task_annotations() {
    for (name, cwd, command, permission) in [
        ("root-child", "", false, "read"),
        ("cwd-child", "work", true, "write"),
    ] {
        let fixture = EnvironmentFixture::new(&[("checkout", "")]);
        let (record, client) = fixture
            .spawn(
                "parent",
                fixture.registry.clone(),
                workdir::WorkdirToolBrokerRouter::new(fixture.sessions.clone()),
                json!({
                    "name": name, "task": TASK, "cwd": if cwd.is_empty() { "." } else { cwd },
                    "scope": [{"target": ".", "permission": permission}],
                    "command": command,
                }),
            )
            .await;
        let router = record.workdir_tool_scope.session_router();
        let prompt = assert_environment_and_reads(&router, &client, &[("checkout", cwd, command)]);
        assert_retained_environment(&record, &prompt);
        assert_eq!(
            record.installed_tools.iter().any(|tool| tool == "Bash"),
            command
        );
        fixture.registry.remove_internal(name).await.unwrap();
    }
}

#[tokio::test]
async fn spawn_multiple_aliases_and_nested_child_keep_real_session_environment() {
    let fixture = EnvironmentFixture::new(&[("alpha", ""), ("beta", "")]);
    let (child, child_client) = fixture
        .spawn(
            "parent",
            fixture.registry.clone(),
            workdir::WorkdirToolBrokerRouter::new(fixture.sessions.clone()),
            json!({
                "name": "child", "task": TASK, "cwd": "work", "command": true,
                "scope": [
                    {"target_workdir": "alpha", "target": ".", "permission": "write"},
                    {"target_workdir": "beta", "target": ".", "permission": "write"},
                ],
            }),
        )
        .await;
    let child_router = child.workdir_tool_scope.session_router();
    let child_prompt = assert_environment_and_reads(
        &child_router,
        &child_client,
        &[("alpha", "work", true), ("beta", "work", true)],
    );
    assert_retained_environment(&child, &child_prompt);
    assert!(
        child
            .installed_tools
            .iter()
            .any(|tool| tool == "SubWorkerSpawn")
    );

    // Reuse the actual child's registry and attenuated broker. The nested tool
    // receives the scripted LLM at the same boundary as the first spawn; session
    // construction, routing, delegation and prompt materialization remain real.
    let (nested, nested_client) = fixture
        .spawn(
            "child",
            child.child_registry.clone(),
            child.workdir_tool_scope.broker_router(),
            json!({
                "name": "nested", "task": TASK, "cwd": "work/nested",
                "scope": [
                    {"target_workdir": "alpha", "target": "work/nested", "permission": "read"},
                    {"target_workdir": "beta", "target": "work/nested", "permission": "read"},
                ],
            }),
        )
        .await;
    let nested_router = nested.workdir_tool_scope.session_router();
    let nested_prompt = assert_environment_and_reads(
        &nested_router,
        &nested_client,
        &[
            ("alpha", "work/nested", false),
            ("beta", "work/nested", false),
        ],
    );
    assert_retained_environment(&nested, &nested_prompt);
    assert!(!nested.installed_tools.iter().any(|tool| tool == "Bash"));
    for alias in ["alpha", "beta"] {
        let session = nested_router.resolve(Some(alias)).unwrap().session;
        let environment = session.environment();
        let scope = environment
            .scope
            .expect("nested session must remain scoped");
        assert_eq!(scope.len(), 1);
        assert_eq!(scope[0].target.as_str(), "work/nested");
        assert_eq!(
            scope[0].permission,
            workdir::WorkdirToolScopePermission::Read
        );
        assert!(
            session
                .read(workdir::ReadRequest {
                    path: workdir::WorkdirPath::new_scoped(
                        fixture
                            .runtime
                            .path()
                            .join(alias)
                            .join("marker.txt")
                            .display()
                            .to_string(),
                    )
                    .unwrap(),
                    offset: 0,
                    limit: 10,
                    max_bytes: 1024,
                })
                .await
                .is_err()
        );
    }
    // Grandchild cwd and reduced command authority must not rewrite its parent.
    assert_environment_and_reads(
        &child_router,
        &child_client,
        &[("alpha", "work", true), ("beta", "work", true)],
    );
    child
        .child_registry
        .remove_internal("nested")
        .await
        .unwrap();
    fixture.registry.remove_internal("child").await.unwrap();
}

#[tokio::test]
async fn spawn_multiple_aliases_keep_root_scope_independent_of_parent_session_cwds() {
    let fixture = EnvironmentFixture::new(&[("alpha", "work"), ("beta", "other")]);
    let (record, client) = fixture
        .spawn(
            "parent",
            fixture.registry.clone(),
            workdir::WorkdirToolBrokerRouter::new(fixture.sessions.clone()),
            json!({
                "name": "child", "task": TASK, "cwd": "work/nested",
                "scope": [
                    {"target_workdir": "alpha", "target": "work/nested", "permission": "read"},
                    {"target_workdir": "beta", "target": "work/nested", "permission": "read"},
                ],
            }),
        )
        .await;
    let prompt = assert_environment_and_reads(
        &record.workdir_tool_scope.session_router(),
        &client,
        &[
            ("alpha", "work/nested", false),
            ("beta", "work/nested", false),
        ],
    );
    assert_retained_environment(&record, &prompt);
    for (alias, cwd) in [("alpha", "work"), ("beta", "other")] {
        assert_eq!(
            fixture
                .sessions
                .resolve(Some(alias))
                .unwrap()
                .session
                .environment()
                .cwd
                .as_str(),
            cwd
        );
    }
    fixture.registry.remove_internal("child").await.unwrap();
}

#[tokio::test]
async fn nonroot_startup_then_spawn_and_nested_child_keep_root_relative_scope() {
    let fixture = EnvironmentFixture::new(&[("workdir", "")]);
    let checkout = fixture.runtime.path().join("workdir");
    let client = EnvironmentClient::default();
    let mut manifest = fixture.manifest.clone();
    manifest.scope.allow = vec![abs_rule(&checkout, Permission::Write)];
    let mut worker =
        Worker::<Box<dyn LlmClient>, EphemeralSessionStore>::from_internal_manifest_with_context(
            manifest,
            EphemeralSessionStore::default(),
            PromptCatalogSource::builtins_only(),
            crate::worker::WorkerWorkspaceContext::local_filesystem(None),
            WorkerFilesystemAuthority::local(checkout.clone(), checkout.join("work")),
            Some(Box::new(client.clone())),
        )
        .await
        .unwrap();
    // Do not bind a test router: exercise workdir_sessions_from_authority via
    // the real constructor and the normal controller's broker selection.
    let startup = worker.workdir_sessions();
    register_worker_tools(
        &mut worker,
        fixture.runtime.path().to_path_buf(),
        fixture.registry.clone(),
        None,
        None,
    )
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(5), worker.run_text(TASK))
        .await
        .expect("startup read turn must terminate")
        .unwrap();
    assert_environment_and_reads(&startup, &client, &[("workdir", "work", true)]);

    let (child, child_client) = fixture
        .spawn(
            "parent",
            fixture.registry.clone(),
            workdir::WorkdirToolBrokerRouter::new(startup.clone()),
            json!({
                "name": "child", "task": TASK, "cwd": "work/nested",
                "scope": [{"target": "work/nested", "permission": "read"}],
            }),
        )
        .await;
    let child_router = child.workdir_tool_scope.session_router();
    let child_prompt = assert_environment_and_reads(
        &child_router,
        &child_client,
        &[("workdir", "work/nested", false)],
    );
    assert_retained_environment(&child, &child_prompt);
    let (nested, nested_client) = fixture
        .spawn(
            "child",
            child.child_registry.clone(),
            child.workdir_tool_scope.broker_router(),
            json!({
                "name": "nested", "task": TASK, "cwd": "work/nested/deep",
                "scope": [{"target": "work/nested/deep", "permission": "read"}],
            }),
        )
        .await;
    let nested_router = nested.workdir_tool_scope.session_router();
    let nested_prompt = assert_environment_and_reads(
        &nested_router,
        &nested_client,
        &[("workdir", "work/nested/deep", false)],
    );
    assert_retained_environment(&nested, &nested_prompt);
    for (router, target) in [
        (&child_router, "work/nested"),
        (&nested_router, "work/nested/deep"),
    ] {
        let environment = router.resolve(None).unwrap().session.environment();
        let rules = environment.scope.unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].target.as_str(), target);
    }
    assert_eq!(
        startup
            .resolve(None)
            .unwrap()
            .session
            .environment()
            .cwd
            .as_str(),
        "work"
    );
    let alias = workdir::WorkdirAttachmentAlias::new("workdir").unwrap();
    startup.begin_detach(&alias).await.unwrap();
    // Both the parent tool view and descendant root-coordinate broker source
    // must share this fence, rather than bypassing it through a cached broker.
    for router in [&startup, &child_router, &nested_router] {
        let session = router.resolve(None).unwrap().session;
        assert!(
            session
                .read(workdir::ReadRequest {
                    path: workdir::WorkdirPath::new("marker.txt").unwrap(),
                    offset: 0,
                    limit: 10,
                    max_bytes: 1024,
                })
                .await
                .is_err()
        );
    }
    startup.cancel_detach(&alias);
    child
        .child_registry
        .remove_internal("nested")
        .await
        .unwrap();
    fixture.registry.remove_internal("child").await.unwrap();
}

#[tokio::test]
async fn startup_rejects_cwd_outside_workdir_root_without_running_the_model() {
    let fixture = EnvironmentFixture::new(&[("workdir", "")]);
    let client = EnvironmentClient::default();
    let mut manifest = fixture.manifest.clone();
    manifest.scope.allow = vec![abs_rule(fixture.runtime.path(), Permission::Write)];
    let error =
        Worker::<Box<dyn LlmClient>, EphemeralSessionStore>::from_internal_manifest_with_context(
            manifest,
            EphemeralSessionStore::default(),
            PromptCatalogSource::builtins_only(),
            crate::worker::WorkerWorkspaceContext::local_filesystem(None),
            WorkerFilesystemAuthority::local(
                fixture.runtime.path().join("workdir"),
                fixture.root.clone(),
            ),
            Some(Box::new(client.clone())),
        )
        .await
        .err()
        .expect("a session cwd must be representable within its Workdir root");
    assert!(matches!(error, crate::WorkerError::InvalidCwd { .. }));
    assert!(client.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn worker_multiple_aliases_project_distinct_session_cwds_not_discovery_root() {
    for prompt_override in [None, Some("Feature-owned instruction body")] {
        let fixture = EnvironmentFixture::new(&[("alpha", "work"), ("beta", "other")]);
        let client = EnvironmentClient::default();
        let mut worker = Worker::<Box<dyn LlmClient>, EphemeralSessionStore>::from_internal_manifest_with_context(
            fixture.manifest.clone(),
            EphemeralSessionStore::default(),
            PromptCatalogSource::builtins_only(),
            crate::worker::WorkerWorkspaceContext::local_filesystem(None),
            WorkerFilesystemAuthority::None,
            Some(Box::new(client.clone())),
        )
        .await
        .unwrap();
        worker.bind_workdir_sessions(fixture.sessions.clone());
        register_worker_tools(
            &mut worker,
            fixture.runtime.path().to_path_buf(),
            fixture.registry.clone(),
            None,
            Some(workdir::WorkdirToolBrokerRouter::new(
                fixture.sessions.clone(),
            )),
        )
        .await
        .unwrap();
        worker.install_system_prompt_contribution(None, prompt_override.map(str::to_owned));
        tokio::time::timeout(Duration::from_secs(5), worker.run_text(TASK))
            .await
            .expect("scripted worker turn must terminate")
            .unwrap();
        let prompt = assert_environment_and_reads(
            &fixture.sessions,
            &client,
            &[("alpha", "work", true), ("beta", "other", true)],
        );
        if let Some(body) = prompt_override {
            assert!(prompt.starts_with(body));
        }
    }
}
