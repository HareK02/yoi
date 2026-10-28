//! Model-facing Inspect/Invoke integration through real WIP Client/Host, management Feature,
//! WorkspaceAttachedWorkdirSession, checkout/config projections and an isolated LocalWorkdirSession.
//! Only WorkspaceClient transport/Backend ledger is substituted; Backend persistence/authorization
//! is not reimplemented or certified here. Selected Entries come from published return declarations,
//! not result field names or path concatenation. This module belongs to the WIP binding test scope.
use super::*;
use crate::feature::builtin::manage_workdir::{ManageWorkdirFeature, wip::tests::CatalogClient};
use crate::feature::builtin::workspace_config::WorkspaceConfigFeature;
use crate::worker::{
    WorkspaceClient, WorkspaceClientError, WorkspaceRequest, WorkspaceRequestMethod,
    WorkspaceResponse,
};
use std::sync::atomic::{AtomicBool, AtomicUsize};
use tempfile::TempDir;
use workdir::{LocalWorkdirSession, WorkdirSessionCapabilities, WorkdirSessionHandle};

#[derive(Debug)]
struct ContentClient {
    catalog: Arc<CatalogClient>,
    session: WorkdirSessionHandle,
    reference_unavailable: AtomicBool,
    content_denied: AtomicBool,
    content_reads: AtomicUsize,
}
impl WorkspaceClient for ContentClient {
    fn workspace_id(&self) -> Option<&str> {
        Some("test-workspace")
    }
    fn kind(&self) -> &str {
        "workdir-model-transport"
    }
    fn is_available(&self) -> bool {
        true
    }
    fn execute(
        &self,
        request: WorkspaceRequest,
    ) -> Result<WorkspaceResponse, WorkspaceClientError> {
        if request.path.ends_with("/authorization/bound-check")
            && self.reference_unavailable.load(Ordering::SeqCst)
            && request
                .body
                .as_deref()
                .unwrap()
                .contains("workdir.attachment.read")
        {
            return Ok(WorkspaceResponse {
                status: 403,
                body: "denied".into(),
            });
        }
        if request.path.ends_with("/workdir-session/operations") {
            self.catalog
                .state
                .lock()
                .unwrap()
                .requests
                .push(request.clone());
            if self.content_denied.load(Ordering::SeqCst) {
                return Ok(WorkspaceResponse {
                    status: 403,
                    body: serde_json::to_string(
                        &workdir::http::WorkdirTransportError::from_workdir_error(
                            &workdir::WorkdirError::Denied("fixture".into()),
                        ),
                    )
                    .unwrap(),
                });
            }
            #[derive(serde::Deserialize)]
            struct SessionRequest {
                target_workdir: String,
                operation: workdir::http::WorkdirSessionOperation,
            }
            let operation: SessionRequest =
                serde_json::from_str(request.body.as_deref().unwrap()).unwrap();
            assert_eq!(operation.target_workdir, "checkout");
            // The synchronous WorkspaceClient boundary runs the actual async local provider on an
            // isolated thread/runtime, without nesting block_on in the model's runtime thread.
            if matches!(
                &operation.operation,
                workdir::http::WorkdirSessionOperation::CheckoutExecute(workdir::CheckoutRequest {
                    operation: workdir::CheckoutOperation::Read { .. }
                        | workdir::CheckoutOperation::ReadBytes { .. },
                    ..
                })
            ) {
                self.content_reads.fetch_add(1, Ordering::SeqCst);
            }
            let session = self.session.clone();
            let result = std::thread::spawn(move || {
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap()
                    .block_on(workdir::dispatch_workdir_session_operation(
                        session.as_ref(),
                        operation.operation,
                    ))
            })
            .join()
            .unwrap()
            .unwrap();
            return Ok(reply(&workdir::http::WorkdirSessionOperationResponse {
                result,
                audit_failures: vec![],
            }));
        }
        if request.path.contains("/workspace-config/") {
            self.catalog
                .state
                .lock()
                .unwrap()
                .requests
                .push(request.clone());
            let input: Json = serde_json::from_str(request.body.as_deref().unwrap()).unwrap();
            let current = self
                .catalog
                .state
                .lock()
                .unwrap()
                .config_current
                .clone()
                .unwrap();
            if input["connection_id"] != current["connection_id"]
                || self.content_denied.load(Ordering::SeqCst)
            {
                return Ok(WorkspaceResponse {
                    status: 403,
                    body: "denied".into(),
                });
            }
            let file = "main.dcdl";
            let text = "model = fixture\n";
            let digest = config_source::digest_bytes(text.as_bytes());
            let validator =
                |path: &str| format!("{}:{path}", current["connection_id"].as_str().unwrap());
            if request.path.ends_with("/observe") {
                let input: server_api::WorkspaceConfigObserveRequest =
                    serde_json::from_value(input).unwrap();
                let nodes = ["", file]
                    .into_iter()
                    .filter(|path| {
                        input.paths.iter().any(|selected| {
                            selected == path || selected.is_empty() && input.depth > 0
                        })
                    })
                    .map(|path| server_api::WorkspaceConfigNode {
                        path: path.into(),
                        kind: if path.is_empty() {
                            server_api::WorkspaceConfigNodeKind::Directory
                        } else {
                            server_api::WorkspaceConfigNodeKind::File
                        },
                        validator: validator(path),
                        digest: (!path.is_empty()).then(|| digest.clone()),
                        content_type: (!path.is_empty())
                            .then_some(server_api::ConfigContentType::Decodal),
                        operations: if path.is_empty() {
                            vec![]
                        } else {
                            vec!["read".into()]
                        },
                    })
                    .collect();
                return Ok(reply(&server_api::WorkspaceConfigObserveResponse {
                    connection_id: current["connection_id"].as_str().unwrap().into(),
                    validator: validator(""),
                    digest,
                    entrypoints: vec![file.into()],
                    nodes,
                }));
            }
            assert!(request.path.ends_with("/read"));
            let input: server_api::WorkspaceConfigReadRequest =
                serde_json::from_value(input).unwrap();
            assert_eq!(input.path, file);
            assert_eq!(input.validator, validator(file));
            return Ok(reply(&server_api::WorkspaceConfigReadResponse {
                path: input.path,
                validator: validator(file),
                content: text.into(),
                digest,
                content_type: server_api::ConfigContentType::Decodal,
            }));
        }
        self.catalog.execute(request)
    }
}
fn reply(value: &impl serde::Serialize) -> WorkspaceResponse {
    WorkspaceResponse {
        status: 200,
        body: serde_json::to_string(value).unwrap(),
    }
}

struct Fixture {
    _directory: TempDir,
    client: Arc<ContentClient>,
    runtime: Arc<WipRuntime>,
    tools: BTreeMap<String, Arc<dyn Tool>>,
}
impl Fixture {
    fn new(checkout: bool, config: bool, permissions: Option<ToolPermissionConfig>) -> Self {
        Self::with_config_content(checkout, config, true, permissions)
    }
    fn with_config_content(
        checkout: bool,
        config: bool,
        content: bool,
        permissions: Option<ToolPermissionConfig>,
    ) -> Self {
        let directory = TempDir::new().unwrap();
        std::fs::write(directory.path().join("sample.txt"), "isolated content\n").unwrap();
        let client = Arc::new(ContentClient {
            catalog: Arc::new(CatalogClient::default()),
            session: Arc::new(LocalWorkdirSession::materialized(
                directory.path().into(),
                directory.path().into(),
                manifest::SharedScope::new(manifest::Scope::writable(directory.path()).unwrap()),
                WorkdirSessionCapabilities::READ_ONLY,
            )),
            reference_unavailable: AtomicBool::new(false),
            content_denied: AtomicBool::new(false),
            content_reads: AtomicUsize::new(0),
        });
        let router = Arc::new(workdir::WorkdirSessionRouter::new());
        let feature = ManageWorkdirFeature::with_child_lifecycle(
            client.clone(),
            router.clone(),
            Arc::new(|| Box::pin(async { Ok(()) })),
            Arc::new(|| {}),
        );
        let mut registry = WipMountRegistry::new();
        if checkout {
            crate::checkout::mount_checkouts(
                &mut registry,
                router,
                tools::Tracker::new(),
                permissions.clone(),
            )
            .unwrap();
        }
        if config {
            let feature = WorkspaceConfigFeature::for_workspace(client.clone(), true);
            if content {
                crate::feature::builtin::workspace_config::wip::mount_workspace_config_wip(
                    &mut registry,
                    &feature,
                )
                .unwrap();
            } else {
                crate::feature::builtin::workspace_config::wip::mount_workspace_config_attach_wip(
                    &mut registry,
                    &feature,
                )
                .unwrap();
            }
        }
        crate::feature::builtin::manage_workdir::wip::mount_workspace_workdir_wip(
            &mut registry,
            &feature,
            true,
            true,
            permissions,
        )
        .unwrap();
        let runtime =
            Arc::new(WipRuntime::from_mounts(registry, "workdir-model-test".into()).unwrap());
        let tools = wip_tool_definitions(runtime.clone())
            .into_iter()
            .map(|definition| {
                let (meta, tool) = definition();
                (meta.name, tool)
            })
            .collect();
        Self {
            _directory: directory,
            client,
            runtime,
            tools,
        }
    }
    async fn execute(&self, tool: &str, input: Json) -> Result<Json, ToolError> {
        self.tools[tool]
            .execute(&input.to_string(), ToolExecutionContext::direct())
            .await
            .map(|output| serde_json::from_str(output.content.as_deref().unwrap()).unwrap())
    }
    async fn inspect(&self, path: &str) -> Json {
        self.execute("Inspect", json!({"path":path})).await.unwrap()
    }
    async fn request(&self, path: &str, operation: &str, arguments: Json) -> (Json, TypeExpr) {
        let object = self.inspect(path).await;
        for interface in object["interfaces"].as_array().unwrap() {
            let reference = InterfaceReference {
                scope: interface["reference"]["scope"].as_str().unwrap().into(),
                name: interface["reference"]["name"].as_str().unwrap().into(),
            };
            let descriptor = self
                .runtime
                .host
                .fetch_interface_live(&reference)
                .await
                .unwrap()
                .descriptor;
            if let Some(op) = descriptor.operations.iter().find(|op| op.name == operation) {
                let display = self
                    .execute("Inspect", json!({"path":interface["path"]}))
                    .await
                    .unwrap();
                assert_eq!(
                    display["interface_signature"],
                    wip_text_view::render_interface(&wip_text_view::Interface {
                        reference: &reference,
                        descriptor: &descriptor,
                    })
                    .unwrap()
                );
                return (
                    json!({"path":path,"interface":interface["reference"],"operation":operation,"arguments":arguments}),
                    op.returns.r#type.clone(),
                );
            }
        }
        panic!("no published {operation} for {path}")
    }
    async fn invoke(&self, path: &str, operation: &str, arguments: Json) -> (Json, Vec<String>) {
        let (request, ty) = self.request(path, operation, arguments).await;
        let value = self.execute("Invoke", request).await.unwrap();
        let mut entries = Vec::new();
        collect_entries(&ty, &value, &mut entries);
        (value, entries)
    }
    fn writes(&self, suffix: &str) -> usize {
        self.client
            .catalog
            .state
            .lock()
            .unwrap()
            .requests
            .iter()
            .filter(|request| {
                request.method != WorkspaceRequestMethod::Get && request.path.ends_with(suffix)
            })
            .count()
    }
}
fn collect_entries(ty: &TypeExpr, value: &Json, entries: &mut Vec<String>) {
    match ty {
        TypeExpr::Entry => entries.push(value.as_str().expect("Entry wire string").into()),
        TypeExpr::Record { fields } => {
            for field in fields {
                if let Some(value) = value.get(&field.name) {
                    collect_entries(&field.r#type, value, entries);
                }
            }
        }
        TypeExpr::List { items } => {
            for value in value.as_array().unwrap() {
                collect_entries(items, value, entries)
            }
        }
        TypeExpr::Union { cases } => {
            let case = cases
                .iter()
                .find(|case| value["$case"] == case.name)
                .unwrap();
            if let Some(payload) = &case.payload {
                collect_entries(payload, &value["value"], entries);
            }
        }
        // Neither ordinary Strings nor JSON objects containing path-shaped strings are Entries.
        _ => (),
    }
}
fn selected(entries: &[String], root: &str) -> String {
    entries
        .iter()
        .find(|entry| entry.starts_with(root) && entry.as_str() != root)
        .unwrap()
        .clone()
}

#[tokio::test]
async fn model_repository_create_attach_selected_entries_read_and_detach_preserve_workdir() {
    let f = Fixture::new(true, false, None);
    let (listed, entries) = f.invoke("/repositories", "list", json!({})).await;
    assert_eq!(entries.len(), 1);
    let repository = entries[0].clone();
    let (read, _) = f.invoke(&repository, "read", json!({})).await;
    assert_eq!(read["repository_key"], listed["items"][0]["repository_key"]);
    let (created, entries) = f
        .invoke(
            "/workdirs",
            "create",
            json!({"repository_key":read["repository_key"]}),
        )
        .await;
    assert_eq!(f.writes("working-directories"), 1);
    assert!(
        f.client
            .catalog
            .state
            .lock()
            .unwrap()
            .attachments
            .is_empty()
    );
    let workdir = selected(&entries, "/workdirs");
    assert!(entries.contains(&repository));
    let (attached, entries) = f
        .invoke(&workdir, "attach", json!({"alias":"checkout"}))
        .await;
    assert_eq!(attached["attached"], true);
    assert_eq!(attached["attachment"]["$case"], "available");
    assert_eq!(
        f.writes("workdir-session/operations"),
        0,
        "returning Entries must not observe content eagerly"
    );
    let attachment = selected(&entries, "/workdir-attachments");
    let checkout = selected(&entries, "/checkouts");
    let (_, read_entries) = f.invoke(&attachment, "read", json!({})).await;
    assert!(read_entries.contains(&checkout));
    let (_, files) = f.invoke(&checkout, "list", json!({})).await;
    let file = files.first().unwrap();
    let (read, _) = f.invoke(file, "read", json!({})).await;
    assert!(
        read["content"]
            .as_str()
            .unwrap()
            .contains("isolated content")
    );
    assert_eq!(f.writes("workdir-attachments"), 1);
    f.invoke(&attachment, "detach", json!({})).await;
    assert!(
        f.execute("Inspect", json!({"path":attachment,"refresh":true}))
            .await
            .is_err()
    );
    assert!(
        f.execute("Inspect", json!({"path":checkout,"refresh":true}))
            .await
            .is_err()
    );
    assert_eq!(f.client.catalog.state.lock().unwrap().workdirs.len(), 1);
    assert_eq!(created["item"]["working_directory_id"], "wd-created");
}

#[tokio::test]
async fn model_config_attachment_entry_reaches_logical_content_without_physical_session() {
    let f = Fixture::new(false, true, None);
    let current = json!({"workspace_id":"test-workspace", "alias":"workspace-config", "connection_id":"logical-connection", "working_directory_id":"logical-workdir", "access":"read_only", "name":"Config", "purpose":"Configuration", "content_path":"/workspace-config", "already_attached":true});
    {
        let mut state = f.client.catalog.state.lock().unwrap();
        state.config_current = Some(current.clone());
        state.attachments.push(json!({"alias":current["alias"], "connection_id":current["connection_id"], "working_directory_id":current["working_directory_id"], "capabilities":{"bits":25}}));
    }
    let (_, entries) = f.invoke("/workdir-attachments", "list", json!({})).await;
    let attachment = selected(&entries, "/workdir-attachments");
    let (_, entries) = f.invoke(&attachment, "read", json!({})).await;
    let entrance = entries
        .iter()
        .find(|entry| entry.as_str() == "/workspace-config")
        .unwrap();
    assert!(!entries.iter().any(|entry| entry.starts_with("/checkouts")));
    f.inspect(entrance).await;
    let tree = f
        .execute("Tree", json!({"path":entrance, "depth":1}))
        .await
        .unwrap();
    let file = tree["tree"]["children"][0]["path"].as_str().unwrap();
    let (read, _) = f.invoke(file, "read", json!({})).await;
    assert_eq!(read["content"], "model = fixture\n");
    assert_eq!(f.writes("workdir-session/operations"), 0);
    assert_eq!(f.writes("workdir-attachments"), 0);
    let (old_read, _) = f.request(file, "read", json!({})).await;
    {
        let mut state = f.client.catalog.state.lock().unwrap();
        state.config_current.as_mut().unwrap()["connection_id"] = json!("logical-reconnected");
        state.attachments[0]["connection_id"] = json!("logical-reconnected");
    }
    let reads = f.writes("workspace-config/read");
    assert!(f.execute("Invoke", old_read).await.is_err());
    assert_eq!(
        f.writes("workspace-config/read"),
        reads,
        "stale logical observation must not read the replacement connection"
    );
    assert!(
        f.execute("Inspect", json!({"path":attachment,"refresh":true}))
            .await
            .is_err()
    );
    f.execute("Inspect", json!({"path":file,"refresh":true}))
        .await
        .unwrap();
    f.invoke(file, "read", json!({})).await;
    f.client.catalog.state.lock().unwrap().config_current = None;
    assert!(
        f.execute("Inspect", json!({"path":file,"refresh":true}))
            .await
            .is_err()
    );
}

async fn create_and_attach(f: &Fixture) -> (String, Json, Vec<String>) {
    let (_, entries) = f
        .invoke(
            "/workdirs",
            "create",
            json!({"repository_key":"registered-key"}),
        )
        .await;
    let workdir = selected(&entries, "/workdirs");
    let (attached, entries) = f
        .invoke(&workdir, "attach", json!({"alias":"checkout"}))
        .await;
    (workdir, attached, entries)
}

#[tokio::test]
async fn model_successful_mutations_survive_unavailable_references_without_reexecution() {
    let f = Fixture::new(true, false, None);
    f.client.reference_unavailable.store(true, Ordering::SeqCst);
    let (workdir, attached, entries) = create_and_attach(&f).await;
    assert_eq!(attached["attached"], true);
    assert_eq!(attached["attachment"]["$case"], "unavailable");
    assert!(!entries.iter().any(|entry| entry.starts_with("/checkouts")));
    assert_eq!(f.writes("working-directories"), 1);
    assert_eq!(f.writes("workdir-attachments"), 1);
    f.client
        .reference_unavailable
        .store(false, Ordering::SeqCst);
    let (_, entries) = f.invoke("/workdir-attachments", "list", json!({})).await;
    let attachment = selected(&entries, "/workdir-attachments");
    let checkout = selected(&entries, "/checkouts");
    f.invoke(&attachment, "detach", json!({})).await;
    // Both successful mutation results remain successful when selected coordinates later disappear.
    assert!(
        f.execute("Inspect", json!({"path":checkout, "refresh":true}))
            .await
            .is_err()
    );
    f.client.catalog.state.lock().unwrap().workdirs.clear();
    assert!(
        f.execute("Inspect", json!({"path":workdir, "refresh":true}))
            .await
            .is_err()
    );
    assert_eq!(f.writes("working-directories"), 1);
    assert_eq!(f.writes("workdir-attachments"), 1);
    assert!(
        f.runtime
            .audit()
            .iter()
            .all(|audit| audit.outcome == WipAuditOutcome::Success)
    );
}

#[tokio::test]
async fn model_alias_reuse_rejects_old_attachment_and_content_observations() {
    let f = Fixture::new(true, false, None);
    let (workdir, _, entries) = create_and_attach(&f).await;
    let old_attachment = selected(&entries, "/workdir-attachments");
    let checkout = selected(&entries, "/checkouts");
    let (_, files) = f.invoke(&checkout, "list", json!({})).await;
    let (old_read, _) = f.request(&files[0], "read", json!({})).await;
    let (old_detach, _) = f.request(&old_attachment, "detach", json!({})).await;
    f.execute("Invoke", old_detach.clone()).await.unwrap();
    let (_, new_entries) = f
        .invoke(&workdir, "attach", json!({"alias":"checkout"}))
        .await;
    let new_attachment = selected(&new_entries, "/workdir-attachments");
    assert_ne!(old_attachment, new_attachment);
    assert!(
        new_entries.contains(&checkout),
        "coordinate reuse is not observation reuse"
    );
    let before = f.client.content_reads.load(Ordering::SeqCst);
    assert!(f.execute("Invoke", old_read).await.is_err());
    assert_eq!(f.client.content_reads.load(Ordering::SeqCst), before);
    assert!(f.execute("Invoke", old_detach).await.is_err());
    assert_eq!(f.client.catalog.state.lock().unwrap().attachments.len(), 1);
    assert!(
        f.execute("Inspect", json!({"path":old_attachment, "refresh":true}))
            .await
            .is_err()
    );
    f.execute("Inspect", json!({"path":checkout, "refresh":true}))
        .await
        .unwrap();
    let (_, files) = f.invoke(&checkout, "list", json!({})).await;
    // Explicit refresh observes the new generation rather than retrying the old call.
    f.execute("Inspect", json!({"path":files[0], "refresh":true}))
        .await
        .unwrap();
    let (read, _) = f.invoke(&files[0], "read", json!({})).await;
    assert!(
        read["content"]
            .as_str()
            .unwrap()
            .contains("isolated content")
    );
}

#[tokio::test]
async fn model_content_entry_never_grants_read_or_write_authority() {
    let f = Fixture::new(true, false, None);
    let (_, _, entries) = create_and_attach(&f).await;
    let checkout = selected(&entries, "/checkouts");
    f.client.content_denied.store(true, Ordering::SeqCst);
    assert!(
        f.execute("Inspect", json!({"path":checkout}))
            .await
            .is_err()
    );
    f.client.content_denied.store(false, Ordering::SeqCst);
    f.execute("Inspect", json!({"path":checkout, "refresh":true}))
        .await
        .unwrap();
    let (_, files) = f.invoke(&checkout, "list", json!({})).await;
    let object = f.inspect(&files[0]).await;
    for interface in object["interfaces"].as_array().unwrap() {
        let reference = InterfaceReference {
            scope: interface["reference"]["scope"].as_str().unwrap().into(),
            name: interface["reference"]["name"].as_str().unwrap().into(),
        };
        let descriptor = f
            .runtime
            .host
            .fetch_interface_live(&reference)
            .await
            .unwrap()
            .descriptor;
        assert!(
            !descriptor
                .operations
                .iter()
                .any(|operation| matches!(operation.name.as_str(), "write" | "edit" | "command"))
        );
    }
    let (read, _) = f.request(&files[0], "read", json!({})).await;
    f.client.content_denied.store(true, Ordering::SeqCst);
    assert!(
        f.execute("Invoke", read).await.is_err(),
        "current authority is rechecked"
    );
}

#[tokio::test]
async fn model_plain_path_strings_stay_data_and_pages_expose_only_selected_entries() {
    let f = Fixture::new(false, false, None);
    {
        let mut state = f.client.catalog.state.lock().unwrap();
        state.repositories[0]["display_name"] = json!("/workdirs/not-an-entry");
        state.repositories[0]["default_selector"] = json!("/checkouts/not-an-entry");
        state.repositories.push(json!({"repository_key":"second"}));
    }
    let (first, entries) = f.invoke("/repositories", "list", json!({"limit":1})).await;
    assert_eq!(entries.len(), 1);
    assert_eq!(first["has_more"], true);
    assert!(!entries.iter().any(|entry| entry.contains("not-an-entry")));
    let (second, entries) = f
        .invoke(
            "/repositories",
            "list",
            json!({"limit":1, "cursor":first["next_cursor"]}),
        )
        .await;
    assert_eq!(entries.len(), 1);
    assert_eq!(second["items"][0]["repository_key"], "second");
    let (empty, entries) = f.invoke("/workdirs", "list", json!({})).await;
    assert_eq!(empty["empty"], true);
    assert!(entries.is_empty());
    let (_, attached, entries) = create_and_attach(&f).await;
    assert_eq!(
        attached["attachment"]["value"]["content_state"],
        "unpublished"
    );
    assert!(!entries.iter().any(|entry| entry.starts_with("/checkouts")));
}

#[tokio::test]
async fn model_self_attach_only_root_does_not_publish_a_config_content_entry() {
    let f = Fixture::with_config_content(false, true, false, None);
    let current = json!({"workspace_id":"test-workspace", "alias":"workspace-config", "connection_id":"logical-connection", "working_directory_id":"logical-workdir", "access":"read_only", "name":"Config", "purpose":"Configuration", "content_path":"/workspace-config", "already_attached":true});
    {
        let mut state = f.client.catalog.state.lock().unwrap();
        state.config_current = Some(current.clone());
        state.attachments.push(json!({"alias":current["alias"], "connection_id":current["connection_id"], "working_directory_id":current["working_directory_id"], "capabilities":{"bits":25}}));
    }
    let object = f.inspect("/workspace-config").await;
    assert_eq!(object["path"], "/workspace-config");
    let (listed, entries) = f.invoke("/workdir-attachments", "list", json!({})).await;
    assert_eq!(listed["items"][0]["content_state"], "unpublished");
    assert!(!entries.contains(&"/workspace-config".into()));
}
